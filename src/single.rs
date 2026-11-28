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

/// Största meddelande vi tar emot (en lista sökvägar; 1 MiB räcker för tusentals filer).
const MAX_MESSAGE: u64 = 1 << 20;

/// Läser ett meddelande, högst `MAX_MESSAGE` byte. `None` om det är för stort, trasigt eller inte går att läsa.
fn read_message(r: impl Read) -> Option<Vec<PathBuf>> {
    let mut buf = Vec::new();
    r.take(MAX_MESSAGE + 1).read_to_end(&mut buf).ok()?;
    parse(&buf)
}

/// Tomt meddelande = bara visa fönstret. Annars måste varje sökväg avslutas med NUL (som `encode` gör);
/// ett avkapat meddelande kastas hellre än att öppna en halv sökväg.
fn parse(buf: &[u8]) -> Option<Vec<PathBuf>> {
    if buf.len() as u64 > MAX_MESSAGE || buf.last().is_some_and(|&b| b != 0) {
        return None;
    }
    Some(decode(buf))
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
                if let Some(files) = read_message(stream) {
                    if tx.send(Incoming::Open(files)).is_ok() {
                        ctx.request_repaint();
                    }
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

    #[test]
    fn empty_message_means_just_show_the_window() {
        assert_eq!(read_message(&b""[..]), Some(vec![]));
    }

    #[test]
    fn valid_message_is_accepted() {
        let msg = encode(&[std::env::temp_dir().join("a.txt")]);
        let files = read_message(&msg[..]).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("a.txt"));
    }

    #[test]
    fn truncated_message_is_dropped() {
        assert_eq!(read_message(&b"/tmp/a.txt"[..]), None);
    }

    #[test]
    fn oversized_message_is_dropped() {
        let big = vec![b'a'; MAX_MESSAGE as usize + 1];
        assert_eq!(read_message(&big[..]), None);
        // Precis på gränsen går bra (här utan NUL i slutet är det trasigt, så avsluta med NUL).
        let mut ok = vec![b'a'; MAX_MESSAGE as usize - 1];
        ok.push(0);
        assert_eq!(read_message(&ok[..]).map(|f| f.len()), Some(1));
        // Läsningen slutar vid gränsen även för en oändlig ström.
        assert_eq!(read_message(std::io::repeat(b'a')), None);
    }
}
