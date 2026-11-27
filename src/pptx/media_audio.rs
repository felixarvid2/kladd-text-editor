//! Ljud i bildspelet: avkodning med symphonia (ren Rust: MP3, WAV/PCM och AAC i MP4), en enkel mixer
//! och utmatning via cpal.
//!
//! * `spawn` avkodar en källa i en egen tråd och skickar stereoblock (f32, källans takt) i en begränsad kö, så att
//!   minnet är litet även för långa filer och tråden slutar av sig själv när lyssnaren släppts.
//! * `Mixer` blandar röster till enhetens takt och kanaler (linjär omsampling). Den är ren och provas utan ljudenhet.
//! * `AudioOut` öppnar ljudenheten i en egen tråd (cpal-strömmen lämnar aldrig den tråden). Saknas enhet, eller
//!   tar öppnandet tid, blockerar inget: tillståndet är `Opening`, `Ready` eller `Failed` och uppspelningen faller
//!   tillbaka på väggklockan (se `media_play.rs`).

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Antal block i kön mellan avkodaren och mixern (varje block är ett paket, ca 20–30 ms).
const QUEUE: usize = 32;

/// Felet när källan saknar ljudspår.
const NO_AUDIO: &str = "inget ljudspår";

/// Där ljudet läses.
#[derive(Clone)]
pub enum Input {
    Bytes(Arc<Vec<u8>>, Option<String>),
    Path(PathBuf),
}

struct Bytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Vad som spelas ur källan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Startpunkt i källan (s).
    pub start_s: f64,
    /// Slutpunkt i källan (s); `None` = till slutet.
    pub end_s: Option<f64>,
    /// Så mycket (s) klipps bort i slutet av källan (`p14:trim/@end`), om längden är känd.
    pub trim_end_s: f64,
    pub looping: bool,
}

/// Från avkodaren till mixern.
pub enum Msg {
    /// Källans takt och längd (s, om den är känd); kommer före första blocket.
    Ready { rate: u32, total_s: Option<f64> },
    /// Stereo, omlott (L, R, L, R …).
    Chunk(Vec<f32>),
    End,
}

/// Startar avkodningen i en egen tråd.
pub fn spawn(input: Input, p: Params) -> Receiver<Msg> {
    let (tx, rx) = sync_channel(QUEUE);
    let _ = std::thread::Builder::new().name("pptx-ljud-avkodning".into()).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                match decode_once(&input, &p, &tx) {
                    Ok(true) if p.looping => continue,
                    Ok(_) => break,
                    // En video utan ljudspår är inget fel.
                    Err(e) if e == NO_AUDIO => break,
                    Err(e) => {
                        eprintln!("Kladd: ljudet kunde inte avkodas: {e}");
                        break;
                    }
                }
            }
        }));
        let _ = r;
        let _ = tx.send(Msg::End);
    });
    rx
}

/// Avkodar källan en gång. `Ok(true)` = nådde slutet, `Ok(false)` = lyssnaren försvann.
fn decode_once(input: &Input, p: &Params, tx: &SyncSender<Msg>) -> Result<bool, String> {
    let mut hint = Hint::new();
    let source: Box<dyn MediaSource> = match input {
        Input::Bytes(b, ext) => {
            if let Some(e) = ext {
                hint.with_extension(e);
            }
            Box::new(Cursor::new(Bytes(b.clone())))
        }
        Input::Path(path) => {
            if let Some(e) = path.extension().and_then(|e| e.to_str()) {
                hint.with_extension(e);
            }
            Box::new(std::fs::File::open(path).map_err(|e| e.to_string())?)
        }
    };
    let mss = MediaSourceStream::new(source, Default::default());
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions { enable_gapless: false, ..Default::default() }, &MetadataOptions::default())
        .map_err(|e| e.to_string())?;
    let mut format = probed.format;
    // Första spåret som går att avkoda (videospåret i en MP4 är okänt för symphonia och hoppas över).
    let (track_id, params, time_base) = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .map(|t| (t.id, t.codec_params.clone(), t.codec_params.time_base))
        .ok_or(NO_AUDIO)?;
    let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
    let rate = params.sample_rate.unwrap_or(44_100).max(1);
    let total_s = params.n_frames.map(|n| n as f64 / rate as f64);
    if tx.send(Msg::Ready { rate, total_s }).is_err() {
        return Ok(false);
    }
    // Hoppa till startpunkten: sök, annars avkoda och släng.
    let mut skip_frames = 0u64;
    let mut pos_frames = 0u64;
    if p.start_s > 0.01 {
        let t = Time::new(p.start_s.trunc() as u64, p.start_s.fract());
        match format.seek(SeekMode::Accurate, SeekTo::Time { time: t, track_id: Some(track_id) }) {
            Ok(s) => {
                decoder.reset();
                let behind = s.required_ts.saturating_sub(s.actual_ts);
                skip_frames = match time_base {
                    Some(tb) => {
                        let t = tb.calc_time(behind);
                        ((t.seconds as f64 + t.frac) * rate as f64).round() as u64
                    }
                    None => behind,
                };
                pos_frames = (p.start_s * rate as f64).round() as u64 - skip_frames.min((p.start_s * rate as f64).round() as u64);
            }
            Err(_) => skip_frames = (p.start_s * rate as f64).round() as u64,
        }
    }
    let cut = total_s.filter(|_| p.trim_end_s > 0.0).map(|t| (t - p.trim_end_s).max(0.0));
    let end_s = match (p.end_s, cut) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let end_frames = end_s.map(|e| (e * rate as f64).round() as u64);
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(pk) => pk,
            Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => return Ok(true),
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => return Ok(true),
            Err(e) => return Err(e.to_string()),
        };
        let spec = *decoded.spec();
        let frames = decoded.frames();
        if frames == 0 {
            continue;
        }
        let ch = spec.channels.count().max(1);
        if buf.as_ref().is_none_or(|b| b.capacity() < frames * ch) {
            buf = Some(SampleBuffer::<f32>::new(frames as u64, spec));
        }
        let Some(sb) = buf.as_mut() else { continue };
        sb.copy_interleaved_ref(decoded);
        let src = sb.samples();
        let mut out = Vec::with_capacity(frames * 2);
        for f in 0..frames {
            let (l, r) = match ch {
                1 => (src[f], src[f]),
                _ => (src[f * ch], src[f * ch + 1]),
            };
            out.push(l);
            out.push(r);
        }
        let mut frames_out = frames as u64;
        if skip_frames > 0 {
            let s = skip_frames.min(frames_out);
            out.drain(..s as usize * 2);
            skip_frames -= s;
            frames_out -= s;
            pos_frames += s;
            if frames_out == 0 {
                continue;
            }
        }
        let mut finished = false;
        if let Some(end) = end_frames {
            if pos_frames + frames_out >= end {
                let keep = end.saturating_sub(pos_frames);
                out.truncate(keep as usize * 2);
                finished = true;
            }
        }
        pos_frames += frames_out;
        if !out.is_empty() && tx.send(Msg::Chunk(out)).is_err() {
            return Ok(false);
        }
        if finished {
            return Ok(true);
        }
    }
}

// ------------------------------------------------------------------ mixern

/// En röst i mixern: en pågående uppspelning.
pub struct Voice {
    rx: Receiver<Msg>,
    rate: u32,
    chunk: Vec<f32>,
    idx: usize,
    cur: [f32; 2],
    next: [f32; 2],
    primed: bool,
    acc: f64,
    ended: bool,
    pub volume: f32,
    pub paused: bool,
    shared: Arc<VoiceShared>,
}

/// Det UI-tråden läser av en röst.
#[derive(Default)]
pub struct VoiceShared {
    /// Antal utmatade bildrutor (i enhetens takt) sedan start: ljudklockan.
    pub played: AtomicU64,
    /// Rösten har spelat klart (eller källan gick inte att avkoda).
    pub finished: AtomicBool,
    /// Någon ljuddata har kommit fram (annars kan klockan inte användas).
    pub started: AtomicBool,
    /// Källans längd i ms (0 = okänd).
    pub total_ms: AtomicU64,
}

impl Voice {
    pub fn new(rx: Receiver<Msg>, volume: f32) -> (Voice, Arc<VoiceShared>) {
        let shared = Arc::new(VoiceShared::default());
        let v = Voice {
            rx,
            rate: 0,
            chunk: Vec::new(),
            idx: 0,
            cur: [0.0; 2],
            next: [0.0; 2],
            primed: false,
            acc: 0.0,
            ended: false,
            volume,
            paused: false,
            shared: shared.clone(),
        };
        (v, shared)
    }

    /// Nästa källbildruta; `None` = kön är tom just nu (eller slut).
    fn fetch(&mut self) -> Option<[f32; 2]> {
        loop {
            if self.idx + 1 < self.chunk.len() {
                let f = [self.chunk[self.idx], self.chunk[self.idx + 1]];
                self.idx += 2;
                return Some(f);
            }
            if self.ended {
                return None;
            }
            match self.rx.try_recv() {
                Ok(Msg::Ready { rate, total_s }) => {
                    self.rate = rate.max(1);
                    self.shared.total_ms.store(total_s.map_or(0, |t| (t * 1000.0) as u64), Ordering::Relaxed);
                }
                Ok(Msg::Chunk(c)) => {
                    self.chunk = c;
                    self.idx = 0;
                    self.shared.started.store(true, Ordering::Relaxed);
                }
                Ok(Msg::End) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.ended = true,
                Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            }
        }
    }

    /// Lägger rösten till `out` (omlott, `channels` kanaler, takten `out_rate`).
    fn mix_into(&mut self, out: &mut [f32], channels: usize, out_rate: u32) {
        if self.paused || channels == 0 {
            return;
        }
        let frames = out.len() / channels;
        for n in 0..frames {
            if !self.primed {
                // Kräver två bildrutor för att kunna interpolera.
                let (Some(a), Some(b)) = (self.fetch(), self.fetch()) else {
                    if self.ended {
                        self.shared.finished.store(true, Ordering::Relaxed);
                    }
                    break;
                };
                self.cur = a;
                self.next = b;
                self.primed = true;
            }
            if self.rate == 0 {
                break;
            }
            let t = self.acc as f32;
            let l = (self.cur[0] + (self.next[0] - self.cur[0]) * t) * self.volume;
            let r = (self.cur[1] + (self.next[1] - self.cur[1]) * t) * self.volume;
            let o = &mut out[n * channels..(n + 1) * channels];
            if channels == 1 {
                o[0] += (l + r) * 0.5;
            } else {
                o[0] += l;
                o[1] += r;
            }
            self.acc += self.rate as f64 / out_rate.max(1) as f64;
            while self.acc >= 1.0 {
                self.acc -= 1.0;
                self.cur = self.next;
                match self.fetch() {
                    Some(f) => self.next = f,
                    None => {
                        if self.ended {
                            self.shared.finished.store(true, Ordering::Relaxed);
                            return self.count(n + 1);
                        }
                        // Kön är tom: stå still tills mer kommer (tystnad i stället för skräp).
                        self.next = self.cur;
                        self.primed = false;
                        self.acc = 0.0;
                        return self.count(n + 1);
                    }
                }
            }
        }
        self.count(frames);
    }

    fn count(&self, frames: usize) {
        self.shared.played.fetch_add(frames as u64, Ordering::Relaxed);
    }
}

/// Alla röster som spelas.
#[derive(Default)]
pub struct Mixer {
    voices: Vec<(u64, Voice)>,
    next_id: u64,
    pub out_rate: u32,
}

impl Mixer {
    pub fn add(&mut self, v: Voice) -> u64 {
        self.next_id += 1;
        self.voices.push((self.next_id, v));
        self.next_id
    }

    pub fn remove(&mut self, id: u64) {
        self.voices.retain(|(i, _)| *i != id);
    }

    pub fn voice(&mut self, id: u64) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|(i, _)| *i == id).map(|(_, v)| v)
    }

    /// Fyller `out` (nollställs först). Färdiga röster tas bort.
    pub fn mix(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let rate = self.out_rate;
        for (_, v) in &mut self.voices {
            v.mix_into(out, channels, rate);
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        self.voices.retain(|(_, v)| !(v.ended && v.idx + 1 >= v.chunk.len() && v.shared.finished.load(Ordering::Relaxed)));
    }
}

// ------------------------------------------------------------------ utmatning

/// Ljudenhetens tillstånd.
#[derive(Clone, Debug, PartialEq)]
pub enum Device {
    Opening,
    Ready,
    /// Ingen enhet (eller den gick inte att öppna): uppspelningen är tyst.
    Failed(String),
}

/// Öppnad (eller på väg att öppnas) ljudutmatning.
pub struct AudioOut {
    pub mixer: Arc<Mutex<Mixer>>,
    state: Arc<Mutex<Device>>,
    stop: Arc<AtomicBool>,
}

impl AudioOut {
    /// Startar en tråd som öppnar standardenheten och håller strömmen vid liv. Blockerar inte.
    pub fn open() -> AudioOut {
        let out = AudioOut { mixer: Arc::new(Mutex::new(Mixer::default())), state: Arc::new(Mutex::new(Device::Opening)), stop: Arc::new(AtomicBool::new(false)) };
        let (mixer, state, stop) = (out.mixer.clone(), out.state.clone(), out.stop.clone());
        let _ = std::thread::Builder::new().name("pptx-ljud-utmatning".into()).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_stream(&mixer, &state, &stop)));
            if r.is_err() {
                set_state(&state, Device::Failed("ljudsystemet kraschade".into()));
            }
        });
        out
    }

    /// En utmatning utan enhet (prov).
    #[cfg(test)]
    pub fn silent() -> AudioOut {
        AudioOut { mixer: Arc::new(Mutex::new(Mixer::default())), state: Arc::new(Mutex::new(Device::Failed("ingen ljudenhet".into()))), stop: Arc::new(AtomicBool::new(false)) }
    }

    /// En öppen enhet som provet själv matar: mixern körs för hand med `mixer.lock().mix(..)`.
    #[cfg(test)]
    pub fn test_ready(rate: u32) -> AudioOut {
        let a = AudioOut { mixer: Arc::new(Mutex::new(Mixer::default())), state: Arc::new(Mutex::new(Device::Ready)), stop: Arc::new(AtomicBool::new(false)) };
        a.mixer.lock().unwrap().out_rate = rate;
        a
    }

    pub fn device(&self) -> Device {
        self.state.lock().map(|s| s.clone()).unwrap_or(Device::Failed("låst".into()))
    }

    #[cfg(test)]
    pub fn is_ready(&self) -> bool {
        self.device() == Device::Ready
    }

    /// Enhetens takt (bildrutor per sekund) när den är öppen.
    pub fn rate(&self) -> u32 {
        self.mixer.lock().map(|m| m.out_rate).unwrap_or(0)
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn set_state(state: &Mutex<Device>, d: Device) {
    if let Ok(mut s) = state.lock() {
        *s = d;
    }
}

fn run_stream(mixer: &Arc<Mutex<Mixer>>, state: &Arc<Mutex<Device>>, stop: &Arc<AtomicBool>) {
    use cpal::SampleFormat;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        eprintln!("Kladd: ingen ljudenhet hittades, bildspelet är tyst");
        return set_state(state, Device::Failed("ingen ljudenhet".into()));
    };
    let cfg = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Kladd: ljudenheten kunde inte öppnas: {e}");
            return set_state(state, Device::Failed(e.to_string()));
        }
    };
    let format = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let channels = config.channels as usize;
    if let Ok(mut m) = mixer.lock() {
        m.out_rate = config.sample_rate;
    }
    let err_fn = |e: cpal::Error| eprintln!("Kladd: ljudfel: {e}");
    let built = match format {
        SampleFormat::F32 => build::<f32>(&device, config, channels, mixer, err_fn),
        SampleFormat::I16 => build::<i16>(&device, config, channels, mixer, err_fn),
        SampleFormat::U16 => build::<u16>(&device, config, channels, mixer, err_fn),
        other => Err(format!("ljudformatet {other} stöds inte")),
    };
    let stream = match built {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Kladd: ljudenheten kunde inte öppnas: {e}");
            return set_state(state, Device::Failed(e));
        }
    };
    if let Err(e) = stream.play() {
        eprintln!("Kladd: ljudenheten kunde inte startas: {e}");
        return set_state(state, Device::Failed(e.to_string()));
    }
    set_state(state, Device::Ready);
    // Strömmen måste leva i den här tråden tills utmatningen släpps.
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(stream);
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mixer: &Arc<Mutex<Mixer>>,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let mixer = mixer.clone();
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                scratch.resize(data.len(), 0.0);
                match mixer.try_lock() {
                    Ok(mut m) => m.mix(&mut scratch, channels),
                    Err(_) => scratch.fill(0.0),
                }
                for (d, s) in data.iter_mut().zip(&scratch) {
                    *d = T::from_sample(*s);
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// En WAV (16 bitar, `ch` kanaler) med en ton eller tystnad.
    pub fn wav(rate: u32, ch: u16, frames: usize, f: impl Fn(usize) -> f32) -> Vec<u8> {
        let data_len = (frames * ch as usize * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&ch.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * ch as u32 * 2).to_le_bytes());
        b.extend_from_slice(&(ch * 2).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let s = (f(i).clamp(-1.0, 1.0) * 32767.0) as i16;
            for _ in 0..ch {
                b.extend_from_slice(&s.to_le_bytes());
            }
        }
        b
    }

    /// Läser allt ur en avkodare: (takt, stereoprov).
    pub fn drain(rx: Receiver<Msg>) -> (u32, Vec<f32>) {
        let (mut rate, mut all) = (0, Vec::new());
        while let Ok(m) = rx.recv_timeout(std::time::Duration::from_secs(10)) {
            match m {
                Msg::Ready { rate: r, .. } => rate = r,
                Msg::Chunk(c) => all.extend(c),
                Msg::End => break,
            }
        }
        (rate, all)
    }

    fn input(b: Vec<u8>) -> Input {
        Input::Bytes(Arc::new(b), None)
    }

    #[test]
    fn wav_avkodas_till_stereo() {
        let (rate, s) = drain(spawn(input(wav(8000, 1, 800, |i| if i < 400 { 0.5 } else { -0.5 })), Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: false }));
        assert_eq!(rate, 8000);
        assert_eq!(s.len(), 1600);
        assert!((s[0] - 0.5).abs() < 0.01 && (s[1] - 0.5).abs() < 0.01);
        assert!((s[1599] + 0.5).abs() < 0.01);
    }

    #[test]
    fn start_och_slut_klipper() {
        // 1 s vid 8 kHz; spela 0,25–0,5 s = 2000 bildrutor.
        let w = wav(8000, 2, 8000, |i| i as f32 / 8000.0);
        let (_, s) = drain(spawn(input(w), Params { start_s: 0.25, end_s: Some(0.5), trim_end_s: 0.0, looping: false }));
        assert_eq!(s.len() / 2, 2000, "{}", s.len() / 2);
        // Första provet ligger vid 0,25 (kurvan växer linjärt).
        assert!((s[0] - 0.25).abs() < 0.01, "{}", s[0]);
    }

    #[test]
    fn trasig_fil_ger_slut_utan_panik() {
        let (rate, s) = drain(spawn(input(vec![1, 2, 3, 4, 5, 6, 7, 8]), Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: false }));
        assert_eq!((rate, s.len()), (0, 0));
        let (_, s) = drain(spawn(Input::Path("/finns/inte.mp3".into()), Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: false }));
        assert!(s.is_empty());
    }

    #[test]
    fn mp3_avkodas() {
        let (rate, s) = drain(spawn(input(include_bytes!("media_fixtures/ton.mp3").to_vec()), Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: false }));
        assert_eq!(rate, 22050);
        // 0,4 s ton (plus fyllnad i kodaren).
        assert!(s.len() / 2 >= 8000, "{}", s.len() / 2);
        let peak = s.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.05, "{peak}");
    }

    #[test]
    fn mp4_ljud_avkodas_utan_ffmpeg() {
        let (rate, s) = drain(spawn(input(include_bytes!("media_fixtures/film.mp4").to_vec()), Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: false }));
        assert_eq!(rate, 22050);
        assert!(s.len() / 2 >= 8000, "{}", s.len() / 2);
        assert!(s.iter().fold(0.0f32, |a, b| a.max(b.abs())) > 0.05);
    }

    #[test]
    fn loop_upprepar_tills_rösten_släpps() {
        let rx = spawn(input(wav(8000, 1, 400, |_| 0.3)), Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: true });
        let mut chunks = 0;
        let mut frames = 0;
        while let Ok(m) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
            if let Msg::Chunk(c) = m {
                chunks += 1;
                frames += c.len() / 2;
            }
            if frames > 1500 {
                break;
            }
        }
        assert!(chunks >= 3 && frames > 1500);
        drop(rx); // tråden ska sluta av sig själv
    }

    #[test]
    fn mixern_omsamplar_blandar_och_räknar() {
        let mut m = Mixer { out_rate: 16000, ..Default::default() };
        let rx = spawn(input(wav(8000, 1, 800, |_| 0.5)), Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: false });
        let (v, shared) = Voice::new(rx, 0.5);
        m.add(v);
        // Låt avkodaren hinna.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut out = vec![0.0f32; 4000 * 2];
        m.mix(&mut out, 2);
        // 800 källbildrutor i 8 kHz = 1600 i 16 kHz; resten är tystnad.
        let loud = out.chunks(2).filter(|f| f[0].abs() > 0.2).count();
        assert!((1500..=1610).contains(&loud), "{loud}");
        assert!((out[10] - 0.25).abs() < 0.01 && out[10] == out[11]);
        assert!(shared.finished.load(Ordering::Relaxed));
        assert!(shared.played.load(Ordering::Relaxed) >= 1590);
        // En röst som är färdig är borta.
        assert!(m.voices.is_empty());
    }

    #[test]
    fn pausad_röst_ger_tystnad_och_klockan_står() {
        let mut m = Mixer { out_rate: 8000, ..Default::default() };
        let rx = spawn(input(wav(8000, 1, 8000, |_| 0.5)), Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: false });
        let (mut v, shared) = Voice::new(rx, 1.0);
        v.paused = true;
        let id = m.add(v);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut out = vec![0.0f32; 200];
        m.mix(&mut out, 2);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(shared.played.load(Ordering::Relaxed), 0);
        m.voice(id).unwrap().paused = false;
        m.mix(&mut out, 2);
        assert!(out.iter().any(|s| *s > 0.4));
        assert_eq!(shared.played.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn utan_ljudenhet_blir_det_tyst_utan_hängning() {
        let a = AudioOut::silent();
        assert!(!a.is_ready());
        assert!(matches!(a.device(), Device::Failed(_)));
        // Den riktiga öppningen får aldrig blockera eller paniken fälla provet, med eller utan enhet.
        let b = AudioOut::open();
        let t = std::time::Instant::now();
        while b.device() == Device::Opening && t.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_ne!(b.device(), Device::Opening);
    }
}
