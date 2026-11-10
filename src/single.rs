//! Ett enda Kladd-fönster: om Kladd redan körs skickas filerna dit och
//! öppnas som nya flikar i stället för i ett nytt fönster.

use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let user = std::env::var("USER").unwrap_or_default();
    dir.join(format!("kladd-{user}.sock"))
}

/// Försöker lämna över filerna till ett redan körande Kladd.
/// Returnerar `true` om det lyckades (då ska den här processen avslutas).
pub fn hand_over(files: &[PathBuf]) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut msg = Vec::new();
    for f in files {
        let abs = f.canonicalize().unwrap_or_else(|_| f.clone());
        msg.extend_from_slice(abs.as_os_str().as_bytes());
        msg.push(0);
    }
    stream.write_all(&msg).is_ok()
}

pub enum Incoming {
    /// Filer att öppna (kan vara tomt: då ska fönstret bara visas).
    Open(Vec<PathBuf>),
}

/// Börjar lyssna efter andra Kladd-processer. Anropas av det första fönstret.
pub fn listen(tx: Sender<Incoming>, ctx: egui::Context) -> bool {
    let path = socket_path();
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(_) => {
            // En gammal socket finns kvar från en krasch: ta bort och försök igen.
            if UnixStream::connect(&path).is_ok() {
                return false; // någon annan lyssnar redan
            }
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("kladd: kan inte lyssna på {}: {e}", path.display());
                    return false;
                }
            }
        }
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            let files = buf
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| PathBuf::from(std::ffi::OsStr::from_bytes(s)))
                .collect();
            if tx.send(Incoming::Open(files)).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    true
}

/// Tar bort socketen när fönstret stängs.
pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}
