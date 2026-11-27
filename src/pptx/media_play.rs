//! Uppspelning av ljud och video i bildspelet och föredragsvyn.
//!
//! `MediaHub` ägs av `ShowView` och håller allt som spelas: ett ljud per röst i mixern (`media_audio.rs`), en
//! ffmpeg-process per video (`media_video.rs`) och övergångsljudet. Den matas med kommandon (`MediaCmd`: spela från
//! tid, växla paus, stopp …) och med bildspelets läge (bild, klick-steg). Varje bildruta uppdaterar den klockorna,
//! laddar upp nya videobildrutor som textur och ritar videon och kontrollerna över mediaformen.
//!
//! Tystnad är alltid ett giltigt resultat: saknas ljudenhet spelas inget ljud men videon går på väggklockan;
//! saknas ffmpeg visas videons stillbild (den som redan ritas) och en notis en gång per bildspel; ett fel i en
//! källa loggas en gång och spelar ingenting.
//!
//! Klockan: har ett ljud börjat spelas (ljudenheten öppen) är dess antal utmatade bildrutor klockan, annars
//! väggklockan. Videon visar den bildruta vars tid har kommit.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use egui::{Color32, CursorIcon, FontId, Painter, Pos2, Rect, Stroke, TextureHandle, pos2, vec2};

use super::media_audio::{AudioOut, Device, Input, Params, Voice, VoiceShared};
use super::anim::{MediaCmd, MediaKind};
use super::media_model::{self, Kind, MediaShape, SlideMedia, Source};
use super::media_video::{self, TempFile, VideoPlayer};
use super::model::Deck;
use crate::i18n::t;

/// Så länge (s) en video väntar på att ljudet ska börja innan den går på väggklockan.
const AUDIO_WAIT: f64 = 1.5;
/// Så länge notisen om ffmpeg visas.
const NOTICE_FOR: Duration = Duration::from_secs(8);
/// Kontrollfältets höjd (punkter).
const BAR_H: f32 = 30.0;

/// En röst i mixern och dess delade värden.
struct VoiceRef {
    id: u64,
    shared: Arc<VoiceShared>,
}

/// Ett medium som spelas eller står på paus.
struct Active {
    slide: usize,
    shape: MediaShape,
    paused: bool,
    /// Hubbens bildbyteräknare när det startade (för `numSld`).
    born: u32,
    voice: Option<VoiceRef>,
    video: Option<VideoPlayer>,
    tex: Option<TextureHandle>,
    shown: u64,
    /// Var i mediet (s, efter urklipp) uppspelningen började.
    from_s: f64,
    /// Tiden i mediet vid `resumed` (väggklockans ankare).
    acc_s: f64,
    resumed: Instant,
    started: Instant,
}

impl Active {
    /// Tiden (s) i mediet nu: ljudklockan när den går (och väggklockan ankras då i den), annars väggklockan.
    fn elapsed(&mut self, now: Instant, rate: u32) -> f64 {
        if self.paused {
            return self.acc_s;
        }
        if let Some(v) = &self.voice {
            let finished = v.shared.finished.load(Ordering::Relaxed);
            if v.shared.started.load(Ordering::Relaxed) && !finished && rate > 0 {
                let a = self.from_s + v.shared.played.load(Ordering::Relaxed) as f64 / rate as f64;
                self.acc_s = a;
                self.resumed = now;
                return a;
            }
            // Ljudet har inte börjat än: stå still en stund så att videon inte springer ifrån.
            if !finished && now.duration_since(self.started).as_secs_f64() < AUDIO_WAIT {
                self.resumed = now;
                return self.acc_s;
            }
        }
        self.acc_s + now.duration_since(self.resumed).as_secs_f64()
    }

    /// Längden (s) efter urklipp, om den är känd (ur ljudspåret).
    fn total_s(&self) -> Option<f64> {
        let ms = self.voice.as_ref()?.shared.total_ms.load(Ordering::Relaxed);
        (ms > 0).then(|| self.shape.trimmed_len(ms as f64 / 1000.0))
    }
}

/// Övergångsljudet som spelas.
struct Sfx {
    voice: VoiceRef,
}

/// Allt som spelas i ett bildspel.
pub struct MediaHub {
    deck: Arc<Deck>,
    /// Falskt: inga kommandon ur animeringarna och inga övergångsljud (klick på formen spelar ändå).
    animate: bool,
    audio: Option<AudioOut>,
    /// Ersätter `media_video::find_ffmpeg` (prov).
    ffmpeg: Option<Option<PathBuf>>,
    info: HashMap<usize, Arc<SlideMedia>>,
    active: Vec<Active>,
    sfx: Option<Sfx>,
    /// Bild och klick-steg senast.
    at: Option<(usize, usize)>,
    /// Antal bildbyten hittills.
    changes: u32,
    /// Bildpunkter per punkt på bilden (för videons storlek).
    scale: f32,
    notice: Option<(String, Instant)>,
    notified: bool,
}

impl MediaHub {
    pub fn new(deck: Arc<Deck>, animate: bool) -> MediaHub {
        MediaHub {
            deck,
            animate,
            audio: None,
            ffmpeg: None,
            info: HashMap::new(),
            active: Vec::new(),
            sfx: None,
            at: None,
            changes: 0,
            scale: 2.0,
            notice: None,
            notified: false,
        }
    }

    /// Prov: egen ljudutmatning och ffmpeg-sökväg (`None` = saknas).
    #[cfg(test)]
    pub fn with_backends(mut self, audio: AudioOut, ffmpeg: Option<PathBuf>) -> MediaHub {
        self.audio = Some(audio);
        self.ffmpeg = Some(ffmpeg);
        self
    }

    fn ffmpeg(&self) -> Option<PathBuf> {
        match &self.ffmpeg {
            Some(f) => f.clone(),
            None => media_video::find_ffmpeg(),
        }
    }

    /// Ljudutmatningen; öppnas första gången något ska låta (tråden blockerar inte).
    fn audio(&mut self) -> &AudioOut {
        self.audio.get_or_insert_with(AudioOut::open)
    }

    fn info_of(&mut self, slide: usize) -> Arc<SlideMedia> {
        let deck = self.deck.clone();
        self.info.entry(slide).or_insert_with(|| Arc::new(media_model::of_slide(&deck, slide))).clone()
    }

    /// Notisen som ska visas just nu.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().filter(|(_, at)| at.elapsed() < NOTICE_FOR).map(|(s, _)| s.as_str())
    }

    // ------------------------------------------------------------ läge och kommandon

    /// Bildspelet står på bild `slide`, klick-steg `step` (`None`: ingen bild visas, t.ex. svart skärm).
    /// Bildbyte: medier som inte ska ffortsätta stoppas och övergångsljudet spelas. Kommandona (`p:cmd`) kommer
    /// från uppspelningen (`take_media`) och ges till `apply`.
    pub fn on_position(&mut self, pos: Option<(usize, usize)>) {
        let Some((slide, step)) = pos else { return };
        if self.at == Some((slide, step)) {
            return;
        }
        let prev = self.at.replace((slide, step));
        let info = self.info_of(slide);
        let new_slide = prev.map(|p| p.0) != Some(slide);
        if new_slide {
            self.changes += 1;
            self.expire();
            if self.animate {
                self.transition_sound(&info);
            }
        }
    }

    /// Stoppar medier som gått över sin sista bild (`numSld`).
    fn expire(&mut self) {
        let changes = self.changes;
        let mixer = self.audio.as_ref().map(|a| a.mixer.clone());
        self.active.retain(|a| {
            let keep = changes.saturating_sub(a.born) < a.shape.num_slides;
            if !keep {
                remove_voice(&mixer, a.voice.as_ref());
            }
            keep
        });
    }

    fn transition_sound(&mut self, info: &SlideMedia) {
        let mixer = self.audio.as_ref().map(|a| a.mixer.clone());
        if info.sound.is_some() || info.end_sound {
            if let Some(s) = self.sfx.take() {
                remove_voice(&mixer, Some(&s.voice));
            }
        }
        let Some(snd) = &info.sound else { return };
        let Some(bytes) = self.deck.pkg.parts.get(&snd.part).cloned() else { return };
        let ext = snd.part.rsplit('.').next().map(str::to_owned);
        let p = Params { start_s: 0.0, end_s: None, trim_end_s: 0.0, looping: snd.looping };
        if let Some(voice) = self.new_voice(Input::Bytes(bytes, ext), p, 1.0) {
            self.sfx = Some(Sfx { voice });
        }
    }

    /// Ett kommando till en mediaform på bild `slide`.
    pub fn apply(&mut self, ctx: &egui::Context, slide: usize, cmd: MediaCmd) {
        let info = self.info_of(slide);
        let Some(shape) = info.shape(cmd.shape_id) else { return };
        let idx = self.active.iter().position(|a| a.slide == slide && a.shape.id == shape.id);
        match (cmd.kind, idx) {
            (MediaKind::Play { from_s }, _) => self.start(ctx, slide, shape.clone(), from_s as f64),
            (MediaKind::TogglePause, Some(i)) => {
                let p = !self.active[i].paused;
                self.set_paused(i, p);
            }
            (MediaKind::TogglePause, None) => self.start(ctx, slide, shape.clone(), 0.0),
            (MediaKind::Pause, Some(i)) => self.set_paused(i, true),
            (MediaKind::Resume, Some(i)) => self.set_paused(i, false),
            (MediaKind::Resume, None) => self.start(ctx, slide, shape.clone(), 0.0),
            (MediaKind::Stop, Some(i)) => self.remove(i),
            _ => {}
        }
        ctx.request_repaint();
    }

    fn set_paused(&mut self, i: usize, pause: bool) {
        let now = Instant::now();
        let rate = self.rate();
        let a = &mut self.active[i];
        if a.paused == pause {
            return;
        }
        if pause {
            a.acc_s = a.elapsed(now, rate);
        } else {
            a.resumed = now;
        }
        a.paused = pause;
        if let (Some(v), Some(audio)) = (&a.voice, &self.audio) {
            if let Ok(mut m) = audio.mixer.lock() {
                if let Some(voice) = m.voice(v.id) {
                    voice.paused = pause;
                }
            }
        }
    }

    fn remove(&mut self, i: usize) {
        let a = self.active.remove(i);
        let mixer = self.audio.as_ref().map(|a| a.mixer.clone());
        remove_voice(&mixer, a.voice.as_ref());
        // `a.video` släpps här: ffmpeg dödas och den temporära filen tas bort.
    }

    fn rate(&self) -> u32 {
        self.audio.as_ref().map_or(0, |a| a.rate())
    }

    /// En ny röst i mixern; `None` utan ljudenhet (tyst) eller om källan inte går att läsa.
    fn new_voice(&mut self, input: Input, p: Params, volume: f32) -> Option<VoiceRef> {
        let audio = self.audio();
        if let Device::Failed(_) = audio.device() {
            return None;
        }
        let rx = super::media_audio::spawn(input, p);
        let (voice, shared) = Voice::new(rx, volume.clamp(0.0, 1.0));
        let id = audio.mixer.lock().ok()?.add(voice);
        Some(VoiceRef { id, shared })
    }

    /// Startar mediet från `from_s` (sekunder efter urklippets början); ett medium som redan spelas börjar om.
    fn start(&mut self, ctx: &egui::Context, slide: usize, shape: MediaShape, from_s: f64) {
        if let Some(i) = self.active.iter().position(|a| a.slide == slide && a.shape.id == shape.id) {
            self.remove(i);
        }
        let from_s = from_s.max(0.0);
        let abs = shape.trim_start_ms / 1000.0 + from_s;
        // Källan: inbäddad del eller länkad fil.
        let (input, path, ext) = match &shape.source {
            Source::Part(p) => match self.deck.pkg.parts.get(p).cloned().or_else(|| self.deck.pkg.read(p).map(|b| Arc::new(b.to_vec()))) {
                Some(b) => {
                    let ext = p.rsplit('.').next().map(str::to_owned);
                    (Input::Bytes(b, ext.clone()), None, ext)
                }
                None => return,
            },
            Source::File(f) => {
                let path = PathBuf::from(f.strip_prefix("file:///").map(|r| format!("/{r}")).unwrap_or_else(|| f.clone()));
                if !path.is_file() {
                    eprintln!("Kladd: den länkade mediafilen finns inte: {f}");
                    return;
                }
                let ext = path.extension().and_then(|e| e.to_str()).map(str::to_owned);
                (Input::Path(path.clone()), Some(path), ext)
            }
        };
        let volume = if shape.mute { 0.0 } else { shape.volume };
        let p = Params { start_s: abs, end_s: None, trim_end_s: shape.trim_end_ms / 1000.0, looping: false };
        let voice = self.new_voice(input.clone(), p, volume);
        // Videon: ffmpeg om det finns, annars stillbilden och en notis.
        let mut video = None;
        if shape.kind == Kind::Video {
            match self.ffmpeg() {
                Some(ff) => video = self.start_video(&ff, &input, path.as_deref(), ext.as_deref(), &shape, abs),
                None => self.notify(t("Video spelas med ffmpeg, som inte är installerat. Videons stillbild visas.")),
            }
        }
        // Varken ljud eller video att spela (tyst miljö utan ffmpeg): inget att hålla igång.
        if voice.is_none() && video.is_none() {
            return;
        }
        let now = Instant::now();
        self.active.push(Active {
            slide,
            shape,
            paused: false,
            born: self.changes,
            voice,
            video,
            tex: None,
            shown: 0,
            from_s,
            acc_s: from_s,
            resumed: now,
            started: now,
        });
        ctx.request_repaint();
    }

    fn start_video(&mut self, ff: &std::path::Path, input: &Input, path: Option<&std::path::Path>, ext: Option<&str>, shape: &MediaShape, from_s: f64) -> Option<VideoPlayer> {
        let [_, _, w, h] = shape.rect_pt?;
        let size = media_video::frame_size(w * self.scale, h * self.scale);
        let (src, temp) = match (path, input) {
            (Some(p), _) => (p.to_path_buf(), None),
            (None, Input::Bytes(b, _)) => {
                let t = TempFile::write(b, ext.unwrap_or("mp4")).map_err(|e| eprintln!("Kladd: videon kunde inte skrivas till en temporär fil: {e}")).ok()?;
                (t.path().to_path_buf(), Some(t))
            }
            _ => return None,
        };
        match VideoPlayer::start(ff, &src, temp, from_s, None, size) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Kladd: ffmpeg kunde inte startas: {e}");
                self.notify(t("Videon kunde inte spelas (ffmpeg startade inte). Videons stillbild visas."));
                None
            }
        }
    }

    fn notify(&mut self, s: &'static str) {
        if !self.notified {
            self.notified = true;
            self.notice = Some((s.to_owned(), Instant::now()));
        }
    }

    // ------------------------------------------------------------ varje bildruta

    /// Uppdaterar klockor och videobildrutor, tar bort det som är klart och ber om omritning medan något går.
    pub fn update(&mut self, ctx: &egui::Context) {
        if self.active.is_empty() && self.sfx.is_none() {
            return;
        }
        let now = Instant::now();
        let rate = self.rate();
        let mixer = self.audio.as_ref().map(|a| a.mixer.clone());
        let mut i = 0;
        let mut any_video = false;
        while i < self.active.len() {
            let a = &mut self.active[i];
            let el = a.elapsed(now, rate);
            let mut done = false;
            if let Some(v) = &mut a.video {
                if !a.paused {
                    any_video = true;
                }
                let t = (el - a.from_s).max(0.0);
                v.frame_at(t);
                if v.shown() != a.shown {
                    a.shown = v.shown();
                    let (w, h) = v.size;
                    if let Some(frame) = v.frame_at(t) {
                        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], frame);
                        match &mut a.tex {
                            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                            None => a.tex = Some(ctx.load_texture(format!("pptx-video-{}-{}", a.slide, a.shape.id), img, egui::TextureOptions::LINEAR)),
                        }
                    }
                }
                done = v.finished();
            } else if let Some(v) = &a.voice {
                done = v.shared.finished.load(Ordering::Relaxed);
            }
            // Urklippet slut i ljudet (video med spår): mediet är slut.
            if let Some(v) = &a.voice {
                if a.video.is_some() && a.shape.trim_end_ms > 0.0 && v.shared.finished.load(Ordering::Relaxed) {
                    done = true;
                }
            }
            if done && !a.paused {
                let a = self.active.remove(i);
                remove_voice(&mixer, a.voice.as_ref());
                continue;
            }
            i += 1;
        }
        if let Some(s) = &self.sfx {
            if s.voice.shared.finished.load(Ordering::Relaxed) {
                remove_voice(&mixer, Some(&s.voice));
                self.sfx = None;
            }
        }
        if any_video {
            ctx.request_repaint_after(Duration::from_millis(15));
        } else if !self.active.is_empty() || self.sfx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.notice.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    // ------------------------------------------------------------ rita och klicka

    /// Mediaformens rektangel på skärmen.
    fn screen_rect(&self, slide_rect: Rect, slide_w_pt: f32, shape: &MediaShape) -> Option<Rect> {
        let [x, y, w, h] = shape.rect_pt?;
        let k = slide_rect.width() / slide_w_pt.max(1.0);
        Some(Rect::from_min_size(slide_rect.min + vec2(x * k, y * k), vec2(w * k, h * k)))
    }

    /// Mediaformerna på bilden med sina skärmrektanglar (översta sist).
    fn shapes_on(&self, slide: usize, slide_rect: Rect, slide_w_pt: f32) -> Vec<(MediaShape, Rect)> {
        let Some(info) = self.info.get(&slide) else { return Vec::new() };
        info.shapes.iter().filter_map(|s| Some((s.clone(), self.screen_rect(slide_rect, slide_w_pt, s)?))).collect()
    }

    fn active_of(&self, slide: usize, id: u32) -> Option<&Active> {
        self.active.iter().find(|a| a.slide == slide && a.shape.id == id)
    }

    /// Ritar videobildrutorna, kontrollerna för formen under pekaren och notisen. `slide_rect` är bildens yta.
    /// `opacity(id)` ger formens genomskinlighet ur animeringen (0 = dold: då ritas varken video eller kontroller).
    pub fn paint(&self, painter: &Painter, slide: usize, slide_rect: Rect, slide_w_pt: f32, hover: Option<Pos2>, opacity: &dyn Fn(u32) -> f32) {
        for (shape, rect) in self.shapes_on(slide, slide_rect, slide_w_pt) {
            let alpha = opacity(shape.id).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let act = self.active_of(slide, shape.id);
            if let Some(tex) = act.and_then(|a| a.tex.as_ref()) {
                painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::from_white_alpha((alpha * 255.0) as u8));
            }
            if hover.is_some_and(|p| rect.contains(p)) {
                self.paint_controls(painter, rect, act, hover);
            }
        }
        if let Some(msg) = self.notice() {
            let font = FontId::proportional(14.0);
            let galley = painter.layout_no_wrap(msg.to_owned(), font, Color32::WHITE);
            let r = Rect::from_center_size(pos2(slide_rect.center().x, slide_rect.bottom() - 28.0), galley.size() + vec2(24.0, 12.0));
            painter.rect_filled(r, 6.0, Color32::from_black_alpha(190));
            painter.galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE);
        }
    }

    /// Kontrollfältet: spela/paus och tidslinje (knappen ensam i små former som ljudikoner).
    fn controls_layout(rect: Rect) -> (Rect, Rect, Option<Rect>) {
        let small = rect.width() < 140.0;
        let bar_h = BAR_H.min(rect.height());
        let bar = if small { Rect::from_center_size(rect.center(), vec2(BAR_H, BAR_H)) } else { Rect::from_min_max(pos2(rect.left(), rect.bottom() - bar_h), rect.max) };
        let btn = Rect::from_center_size(pos2(bar.left() + bar_h / 2.0 + if small { BAR_H / 2.0 - bar_h / 2.0 } else { 0.0 }, bar.center().y), vec2(bar_h, bar_h));
        let line = (!small).then(|| Rect::from_min_max(pos2(btn.right() + 4.0, bar.center().y - 8.0), pos2(bar.right() - 10.0, bar.center().y + 8.0)));
        (bar, btn, line)
    }

    fn paint_controls(&self, painter: &Painter, rect: Rect, act: Option<&Active>, hover: Option<Pos2>) {
        let (bar, btn, line) = Self::controls_layout(rect);
        painter.rect_filled(bar, if line.is_none() { BAR_H / 2.0 } else { 0.0 }, Color32::from_black_alpha(150));
        let playing = act.is_some_and(|a| !a.paused);
        let c = btn.center();
        let ink = if hover.is_some_and(|p| btn.contains(p)) { Color32::from_rgb(255, 215, 120) } else { Color32::WHITE };
        if playing {
            for dx in [-4.0, 1.5] {
                painter.rect_filled(Rect::from_min_size(pos2(c.x + dx, c.y - 6.0), vec2(2.5, 12.0)), 0.0, ink);
            }
        } else {
            painter.add(egui::Shape::convex_polygon(vec![pos2(c.x - 4.0, c.y - 7.0), pos2(c.x - 4.0, c.y + 7.0), pos2(c.x + 7.0, c.y)], ink, Stroke::NONE));
        }
        if let Some(line) = line {
            let track = Rect::from_center_size(line.center(), vec2(line.width(), 4.0));
            painter.rect_filled(track, 2.0, Color32::from_white_alpha(70));
            if let Some(a) = act {
                if let Some(total) = a.total_s().filter(|t| *t > 0.0) {
                    let f = (a.acc_s / total).clamp(0.0, 1.0) as f32;
                    painter.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * f, 4.0)), 2.0, Color32::WHITE);
                }
            }
        }
    }

    /// Pekaren över en mediaform: handpekare.
    pub fn hover(&self, ctx: &egui::Context, slide: usize, slide_rect: Rect, slide_w_pt: f32, pos: Option<Pos2>) {
        if let Some(p) = pos {
            if self.shapes_on(slide, slide_rect, slide_w_pt).iter().any(|(_, r)| r.contains(p)) {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
        }
    }

    /// Klick vid `pos` på bild `slide`. Sant om det träffade en mediaform (klicket går då inte vidare till nästa
    /// steg): knappen och formen växlar spela/paus, tidslinjen hoppar.
    pub fn click(&mut self, ctx: &egui::Context, slide: usize, slide_rect: Rect, slide_w_pt: f32, pos: Pos2, opacity: &dyn Fn(u32) -> f32) -> bool {
        self.scale = (ctx.pixels_per_point() * slide_rect.width() / slide_w_pt.max(1.0)).clamp(0.5, 8.0);
        let shapes = self.shapes_on(slide, slide_rect, slide_w_pt);
        let Some((shape, rect)) = shapes.into_iter().rev().find(|(s, r)| r.contains(pos) && opacity(s.id) > 0.0) else { return false };
        let (_, _, line) = Self::controls_layout(rect);
        let act = self.active_of(slide, shape.id);
        if let (Some(line), Some(total)) = (line, act.and_then(|a| a.total_s())) {
            if line.contains(pos) && total > 0.0 {
                let f = ((pos.x - line.left()) / line.width().max(1.0)).clamp(0.0, 1.0) as f64;
                self.apply(ctx, slide, MediaCmd { shape_id: shape.id, kind: MediaKind::Play { from_s: (f * total) as f32 } });
                return true;
            }
        }
        self.apply(ctx, slide, MediaCmd { shape_id: shape.id, kind: MediaKind::TogglePause });
        true
    }

    /// Provhjälp: formerna på bild `slide` som spelas eller står på paus.
    #[cfg(test)]
    pub fn playing_on(&self, slide: usize) -> Vec<u32> {
        self.active.iter().filter(|a| a.slide == slide).map(|a| a.shape.id).collect()
    }

    /// Provhjälp: inget spelas.
    #[cfg(test)]
    pub fn is_quiet(&self) -> bool {
        self.active.is_empty() && self.sfx.is_none()
    }

    /// Provhjälp: ffmpeg-processens id för formen.
    #[cfg(test)]
    pub fn video_pid(&self, slide: usize, id: u32) -> Option<u32> {
        self.active_of(slide, id)?.video.as_ref().map(|v| v.pid())
    }

    /// Provhjälp: formen har fått en videobildruta som textur.
    #[cfg(test)]
    pub fn has_frame(&self, slide: usize, id: u32) -> bool {
        self.active_of(slide, id).is_some_and(|a| a.tex.is_some())
    }
}

fn remove_voice(mixer: &Option<Arc<std::sync::Mutex<super::media_audio::Mixer>>>, v: Option<&VoiceRef>) {
    if let (Some(m), Some(v)) = (mixer, v) {
        if let Ok(mut m) = m.lock() {
            m.remove(v.id);
        }
    }
}

impl Drop for MediaHub {
    fn drop(&mut self) {
        // Rösterna försvinner med mixern; videoprocesserna dödas när `Active` släpps.
        self.active.clear();
        self.sfx = None;
    }
}
