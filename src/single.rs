//! Ett enda Kladd-fönster: om Kladd redan körs skickas filerna dit och
//! öppnas som nya flikar i stället för i ett nytt fönster.
//!
//! Transporten är `interprocess`-lokala uttag: Unix-socket i en fil på Linux/macOS och
//! namngiven pipe på Windows. Namnet är per användare.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use interprocess::local_socket::{Listener, ListenerOptions, Name, Stream, prelude::*};

/// Namnet på uttaget. Unix: en fil (`$XDG_RUNTIME_DIR`, annars tempmappen, som på macOS
/// är per användare) med uid i namnet. Windows: pipen `kladd-<användare>`.
fn socket_name() -> std::io::Result<Name<'static>> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::GenericFilePath;
        let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        // SAFETY: getuid har inga förutsättningar.
        let uid = unsafe { libc::getuid() };
        dir.join(format!("kladd-{uid}.sock")).to_fs_name::<GenericFilePath>()
    }
    #[cfg(windows)]
    {
        use interprocess::local_socket::GenericNamespaced;
        let user = std::env::var("USERNAME").unwrap_or_default();
        format!("kladd-{user}").to_ns_name::<GenericNamespaced>()
    }
}

#[cfg(unix)]
fn socket_file() -> Option<PathBuf> {
    // Samma sökväg som i `socket_name`, för att kunna städa bort filen.
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    // SAFETY: getuid har inga förutsättningar.
    Some(dir.join(format!("kladd-{}.sock", unsafe { libc::getuid() })))
}

/// Sökvägar som byte: på Unix exakt, på Windows UTF-8 (sökvägar som inte är giltig
/// Unicode, vilket är ovanligt, får ersättningstecken).
fn encode(files: &[PathBuf]) -> Vec<u8> {
    let mut msg = Vec::new();
    for f in files {
        let abs = f.canonicalize().unwrap_or_else(|_| f.clone());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            msg.extend_from_slice(abs.as_os_str().as_bytes());
        }
        #[cfg(not(unix))]
        msg.extend_from_slice(abs.to_string_lossy().as_bytes());
        msg.push(0);
    }
    msg
}

fn decode(buf: &[u8]) -> Vec<PathBuf> {
    buf.split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                PathBuf::from(std::ffi::OsStr::from_bytes(s))
            }
            #[cfg(not(unix))]
            PathBuf::from(String::from_utf8_lossy(s).into_owned())
        })
        .collect()
}

/// Försöker lämna över filerna till ett redan körande Kladd.
/// Returnerar `true` om det lyckades (då ska den här processen avslutas).
pub fn hand_over(files: &[PathBuf]) -> bool {
    let Ok(name) = socket_name() else { return false };
    let Ok(mut stream) = Stream::connect(name) else {
        return false;
    };
    stream.write_all(&encode(files)).is_ok()
}

pub enum Incoming {
    /// Filer att öppna (kan vara tomt: då ska fönstret bara visas).
    Open(Vec<PathBuf>),
}

fn bind() -> Option<Listener> {
    let name = socket_name().ok()?;
    match ListenerOptions::new().name(name).create_sync() {
        Ok(l) => return Some(l),
        Err(e) => {
            // Finns någon och lyssnar redan? Då är vi inte första fönstret.
            if socket_name().ok().is_some_and(|n| Stream::connect(n).is_ok()) {
                return None;
            }
            // Annars ligger en gammal socket kvar från en krasch: skriv över den.
            let retry = socket_name().ok().and_then(|n| ListenerOptions::new().name(n).try_overwrite(true).create_sync().ok());
            if retry.is_none() {
                eprintln!("kladd: kan inte lyssna efter andra fönster: {e}");
            }
            retry
        }
    }
}

/// Börjar lyssna efter andra Kladd-processer. Anropas av det första fönstret.
pub fn listen(tx: Sender<Incoming>, ctx: egui::Context) -> bool {
    let Some(listener) = bind() else { return false };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            // En tråd per anslutning, så att en hängande klient inte blockerar nästa.
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = Vec::new();
                if stream.read_to_end(&mut buf).is_ok() && tx.send(Incoming::Open(decode(&buf))).is_ok() {
                    ctx.request_repaint();
                }
            });
        }
    });
    true
}

/// Tar bort socketfilen när fönstret stängs (på Windows försvinner pipen av sig själv).
pub fn cleanup() {
    #[cfg(unix)]
    if let Some(p) = socket_file() {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip() {
        let dir = std::env::temp_dir();
        let files = vec![dir.join("a b.txt"), dir.join("åäö.md")];
        let back = decode(&encode(&files));
        assert_eq!(back.len(), 2);
        assert!(back[0].ends_with("a b.txt"));
        assert!(back[1].ends_with("åäö.md"));
    }
}
