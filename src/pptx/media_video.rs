//! Video i bildspelet via ffmpeg (om det är installerat). Kladd länkar inte in någon videoavkodare: ffmpeg körs
//! som en separat process som skriver råa RGBA-bildrutor på stdout i formens storlek och en fast bildfrekvens, så
//! att bildrutans index ger tiden utan ffprobe. Utan ffmpeg visas videons stillbild och en notis (`media_play.rs`).
//!
//! Processen dödas (och avvaktas, så att inga zombier blir kvar) när uppspelningen stoppas, bilden byts eller
//! bildspelet slutar. Inbäddade filer skrivs till en temporär fil (en MP4 med `moov` sist går varken att söka i
//! eller läsa ur ett rör) som tas bort på samma ställen.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

/// Bildfrekvensen som ffmpeg ger ut.
pub const FPS: f64 = 25.0;
/// Största bildruta (bredd eller höjd) i bildpunkter.
const MAX_SIDE: u32 = 1920;

/// Platserna där ffmpeg kan ligga utöver `PATH` (appar startade från Finder har kort `PATH` på macOS).
const EXTRA_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

fn exe_name() -> &'static str {
    if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }
}

/// Första `ffmpeg` i `dirs`.
pub fn find_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(exe_name())).find(|p| p.is_file())
}

/// ffmpeg i `PATH` och på de vanliga platserna. Slås upp en gång per körning.
pub fn find_ffmpeg() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
            if cfg!(unix) {
                dirs.extend(EXTRA_DIRS.iter().map(PathBuf::from));
            }
            find_in(&dirs)
        })
        .clone()
}

/// En temporär fil som tas bort när den släpps.
pub struct TempFile(PathBuf);

impl TempFile {
    /// Skriver `bytes` till en ny temporär fil med ändelsen `ext`.
    pub fn write(bytes: &[u8], ext: &str) -> std::io::Result<TempFile> {
        static N: AtomicU32 = AtomicU32::new(0);
        let ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
        let path = std::env::temp_dir().join(format!("kladd-media-{}-{}.{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed), if ext.is_empty() { "bin" } else { &ext }));
        std::fs::write(&path, bytes)?;
        Ok(TempFile(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Bildrutans storlek i bildpunkter för en yta: begränsad och minst 2 × 2.
pub fn frame_size(w: f32, h: f32) -> (u32, u32) {
    let (mut w, mut h) = (w.max(2.0), h.max(2.0));
    let k = (MAX_SIDE as f32 / w.max(h)).min(1.0);
    w *= k;
    h *= k;
    (w.round().max(2.0) as u32, h.round().max(2.0) as u32)
}

/// En pågående avkodning med ffmpeg.
pub struct VideoPlayer {
    child: Child,
    rx: Option<Receiver<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    _temp: Option<TempFile>,
    pub size: (u32, u32),
    /// Bildrutan som visas och dess tid (s) sedan start.
    cur: Option<Vec<u8>>,
    /// Nästa bildruta som väntar på sin tid.
    peek: Option<Vec<u8>>,
    /// Index på `peek`.
    next_idx: u64,
    /// Antal gånger bildrutan som visas har bytts (för att veta när texturen ska uppdateras).
    shown: u64,
    eof: bool,
}

impl VideoPlayer {
    /// Startar ffmpeg på `src` från `from_s`, högst `len_s` sekunder, i storleken `size`. `temp` hålls vid liv tills
    /// uppspelningen är slut (filen raderas då).
    pub fn start(ffmpeg: &Path, src: &Path, temp: Option<TempFile>, from_s: f64, len_s: Option<f64>, size: (u32, u32)) -> Result<VideoPlayer, String> {
        let (w, h) = size;
        let mut cmd = Command::new(ffmpeg);
        cmd.args(["-nostdin", "-loglevel", "error", "-ss", &format!("{:.3}", from_s.max(0.0)), "-i"]).arg(src);
        if let Some(l) = len_s {
            cmd.args(["-t", &format!("{:.3}", l.max(0.0))]);
        }
        let vf = format!("fps={FPS},scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black");
        cmd.args(["-an", "-vf", &vf, "-pix_fmt", "rgba", "-f", "rawvideo", "-"]);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let Some(mut out) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("ffmpeg gav ingen utdata".into());
        };
        let frame_len = w as usize * h as usize * 4;
        let (tx, rx) = sync_channel::<Vec<u8>>(6);
        let reader = std::thread::Builder::new()
            .name("pptx-video-lasning".into())
            .spawn(move || {
                loop {
                    let mut buf = vec![0u8; frame_len];
                    if out.read_exact(&mut buf).is_err() || tx.send(buf).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(VideoPlayer { child, rx: Some(rx), reader: Some(reader), _temp: temp, size, cur: None, peek: None, next_idx: 0, shown: 0, eof: false })
    }

    /// Bildrutan som ska synas `t` sekunder efter start (den senaste vars tid har kommit).
    pub fn frame_at(&mut self, t: f64) -> Option<&[u8]> {
        while let Some(rx) = &self.rx {
            if self.peek.is_none() {
                match rx.try_recv() {
                    Ok(f) => self.peek = Some(f),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.eof = true;
                        break;
                    }
                }
            }
            if self.next_idx as f64 / FPS <= t {
                self.cur = self.peek.take();
                self.next_idx += 1;
                self.shown += 1;
            } else {
                break;
            }
        }
        self.cur.as_deref()
    }

    /// Antal bildrutebyten hittills.
    pub fn shown(&self) -> u64 {
        self.shown
    }

    /// Alla bildrutor är visade och ffmpeg är klar.
    pub fn finished(&mut self) -> bool {
        if !self.eof && self.peek.is_none() {
            if let Some(rx) = &self.rx {
                if let Err(TryRecvError::Disconnected) = rx.try_recv().map(|f| self.peek = Some(f)) {
                    self.eof = true;
                }
            }
        }
        self.eof && self.peek.is_none()
    }

    /// Processens id (för prov).
    #[cfg(test)]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Dödar processen och väntar in den och läsartråden.
    fn shut_down(&mut self) {
        self.rx = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.shut_down();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        include_bytes!("media_fixtures/film.mp4").to_vec()
    }

    #[test]
    fn ffmpeg_hittas_i_katalogerna() {
        let d = std::env::temp_dir().join(format!("kladd-ffprov-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(find_in(std::slice::from_ref(&d)), None);
        std::fs::write(d.join(exe_name()), b"#!/bin/sh\n").unwrap();
        assert_eq!(find_in(&[PathBuf::from("/finns/inte"), d.clone()]), Some(d.join(exe_name())));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bildrutans_storlek_begränsas() {
        assert_eq!(frame_size(640.0, 360.0), (640, 360));
        assert_eq!(frame_size(0.0, 0.0), (2, 2));
        let (w, h) = frame_size(4000.0, 2000.0);
        assert_eq!((w, h), (1920, 960));
    }

    #[test]
    fn utan_ffmpeg_ger_start_fel_och_inte_panik() {
        let t = TempFile::write(&fixture(), "mp4").unwrap();
        let r = VideoPlayer::start(Path::new("/finns/inte/ffmpeg"), t.path(), None, 0.0, None, (80, 45));
        assert!(r.is_err());
    }

    #[test]
    fn ffmpeg_ger_bildrutor_i_rätt_storlek_och_dödas() {
        let Some(ff) = find_ffmpeg() else {
            eprintln!("hoppar över: ffmpeg saknas");
            return;
        };
        let t = TempFile::write(&fixture(), "mp4").unwrap();
        let path = t.path().to_path_buf();
        let mut p = VideoPlayer::start(&ff, &path, Some(t), 0.0, None, (80, 44)).unwrap();
        let pid = p.pid();
        let (mut first, mut late) = (None, None);
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_secs() < 20 && (first.is_none() || late.is_none()) {
            if first.is_none() {
                first = p.frame_at(0.0).map(|f| f.to_vec());
            } else {
                late = p.frame_at(2.0).map(|f| f.to_vec());
                // Rutan för tiden 2,0 s kommer sist i kön: vänta tills den skiljer sig från den första.
                if late.as_ref() == first.as_ref() {
                    late = None;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (first, late) = (first.expect("första bildrutan"), late.expect("en senare bildruta"));
        assert_eq!(first.len(), 80 * 44 * 4);
        assert_ne!(first, late);
        assert!(path.exists());
        drop(p);
        // Processen är död och avvaktad, den temporära filen är borta.
        assert!(!path.exists());
        #[cfg(target_os = "linux")]
        assert!(!Path::new(&format!("/proc/{pid}")).exists(), "processen lever kvar");
        let _ = pid;
    }

    #[test]
    fn ffmpeg_start_senare_i_filen_och_slut() {
        let Some(ff) = find_ffmpeg() else { return };
        let t = TempFile::write(&fixture(), "mp4").unwrap();
        let path = t.path().to_path_buf();
        // Sista sekunden: ca 25 bildrutor, sedan slut.
        let mut p = VideoPlayer::start(&ff, &path, Some(t), 2.0, Some(1.0), (32, 18)).unwrap();
        let t0 = std::time::Instant::now();
        while !p.finished() && t0.elapsed().as_secs() < 20 {
            let _ = p.frame_at(100.0);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(p.finished());
        assert!(p.frame_at(100.0).is_some());
    }
}
