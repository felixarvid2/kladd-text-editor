//! Gamla dokumentformat (.doc, .rtf, .wpd, .wps): konverteras till .docx med LibreOffice i en
//! bakgrundstråd och öppnas sedan i dokumentläget. Utan LibreOffice ger Kladd ett tydligt fel;
//! .rtf läses då med en enkel inbyggd läsare (text, fet, kursiv, stycken).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use egui::{Align2, Color32, Context, FontFamily, FontId, Rect, Stroke, pos2, vec2};

use crate::docx::{escape_inline, escape_line_start, wrap};
use crate::theme::Palette;
use crate::{i18n::t, tf};

/// Filändelser som konverteras med LibreOffice.
pub const EXTENSIONS: &[&str] = &["doc", "rtf", "wpd", "wps"];

/// Så länge får en konvertering ta innan den avbryts.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// Gammal Word-fil (OLE2-behållare).
const OLE_MAGIC: [u8; 4] = [0xD0, 0xCF, 0x11, 0xE0];

/// Ska filen konverteras? Avgörs på filändelse och de första byten: en .doc som egentligen är
/// en zip (docx) eller vanlig text går den vanliga vägen.
pub fn needs_conversion(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return false;
    }
    let mut head = [0u8; 8];
    let n = std::fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    classify(&ext, &head[..n])
}

fn classify(ext: &str, head: &[u8]) -> bool {
    match ext {
        "doc" => head.starts_with(&OLE_MAGIC),
        "rtf" => head.starts_with(b"{\\rtf"),
        "wpd" | "wps" => true,
        _ => false,
    }
}

// ------------------------------------------------------------------ hitta LibreOffice

/// Fasta platser där LibreOffice brukar ligga om det inte finns i `PATH`.
fn fixed_locations() -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![
            PathBuf::from(r"C:\Program Files\LibreOffice\program\soffice.exe"),
            PathBuf::from(r"C:\Program Files (x86)\LibreOffice\program\soffice.exe"),
        ]
    } else if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice")]
    } else {
        vec![PathBuf::from("/usr/lib/libreoffice/program/soffice"), PathBuf::from("/opt/libreoffice/program/soffice")]
    }
}

/// LibreOffice på den här datorn (`soffice`/`libreoffice` i `PATH`, annars de fasta platserna).
pub fn find_soffice() -> Option<PathBuf> {
    find_in(std::env::var_os("PATH").as_deref(), &fixed_locations())
}

/// Letar efter `soffice` eller `libreoffice` i katalogerna i `path` och sedan i `extras`.
pub fn find_in(path: Option<&OsStr>, extras: &[PathBuf]) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["soffice.exe", "libreoffice.exe"] } else { &["soffice", "libreoffice"] };
    let in_path = path.into_iter().flat_map(std::env::split_paths).flat_map(|dir| names.iter().map(move |n| dir.join(n)));
    in_path.chain(extras.iter().cloned()).find(|p| p.is_file())
}

// ------------------------------------------------------------------ konvertering

/// `file://`-adress för en katalog (LibreOffices `-env:UserInstallation`).
pub fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if s.starts_with('/') { "file://" } else { "file:///" });
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Argumenten till `soffice`. Eget användarprofil-bibliotek gör att en redan öppen LibreOffice inte
/// tar över anropet (då skulle konverteringen aldrig köras).
pub fn convert_args(profile: &Path, outdir: &Path, input: &Path) -> Vec<OsString> {
    vec![
        format!("-env:UserInstallation={}", file_url(profile)).into(),
        "--headless".into(),
        "--norestore".into(),
        "--nolockcheck".into(),
        "--convert-to".into(),
        "docx".into(),
        "--outdir".into(),
        outdir.into(),
        input.into(),
    ]
}

/// Temporär katalog som tas bort när den släpps.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("kladd-lo-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Konverterar `input` till .docx med `soffice` och ger filens innehåll. Avbryts efter `timeout`.
pub fn convert(soffice: &Path, input: &Path, timeout: Duration) -> Result<Vec<u8>, String> {
    let tmp = TempDir::new().map_err(|e| tf!("Kunde inte skapa en temporär katalog: {e}", e = e))?;
    let out = tmp.0.join("ut");
    std::fs::create_dir_all(&out).map_err(|e| tf!("Kunde inte skapa en temporär katalog: {e}", e = e))?;
    let mut child = Command::new(soffice)
        .args(convert_args(&tmp.0.join("profil"), &out, input))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| tf!("Kunde inte starta LibreOffice: {e}", e = e))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(tf!("LibreOffice svarade inte inom {s} sekunder. Konverteringen avbröts.", s = timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(tf!("Kunde inte starta LibreOffice: {e}", e = e)),
        }
    }
    let stem = input.file_stem().unwrap_or_default();
    let result = out.join(Path::new(stem).with_extension("docx"));
    std::fs::read(&result).map_err(|_| t("LibreOffice kunde inte konvertera filen.").to_owned())
}

/// Felet när LibreOffice saknas.
pub fn missing_message(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    tf!("{name} kan bara öppnas med LibreOffice, som inte hittades. Installera det från libreoffice.org eller öppna filen i Word och spara som .docx.", name = name)
}

/// Notisen när en konverterad fil har öppnats.
pub fn converted_note(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    tf!("{name} konverterades med LibreOffice. Originalet ändras inte: Spara som skapar en .docx.", name = name)
}

// ------------------------------------------------------------------ bakgrundstråd

type Outcome = Result<Vec<u8>, String>;

struct Job {
    path: PathBuf,
    started: Instant,
    rx: Receiver<Outcome>,
}

/// Konverteringar som pågår.
#[derive(Default)]
pub struct State {
    jobs: Vec<Job>,
}

impl State {
    pub fn is_converting(&self, path: &Path) -> bool {
        self.jobs.iter().any(|j| j.path == path)
    }

    pub fn spawn(&mut self, ctx: &Context, soffice: PathBuf, path: &Path) {
        let (tx, rx) = channel();
        let (p, ctx2) = (path.to_path_buf(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(convert(&soffice, &p, TIMEOUT));
            ctx2.request_repaint();
        });
        self.jobs.push(Job { path: path.to_path_buf(), started: Instant::now(), rx });
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    /// Färdiga konverteringar (och fel).
    pub fn poll(&mut self, ctx: &Context) -> Vec<(PathBuf, Outcome)> {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.jobs.len() {
            match self.jobs[i].rx.try_recv() {
                Ok(r) => done.push((self.jobs.remove(i).path, r)),
                Err(TryRecvError::Empty) => i += 1,
                Err(TryRecvError::Disconnected) => {
                    done.push((self.jobs.remove(i).path, Err(t("Konverteringen avbröts.").to_owned())));
                }
            }
        }
        if !self.jobs.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        done
    }

    /// Ruta nere till vänster ("Konverterar …") med en löpande stapel och tidsgränsen.
    pub fn show_progress(&self, ctx: &Context, p: &Palette) {
        let Some(j) = self.jobs.first() else { return };
        let name = j.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let secs = j.started.elapsed().as_secs();
        egui::Area::new(egui::Id::unique("kladd-legacy-progress"))
            .anchor(Align2::LEFT_BOTTOM, vec2(16.0, -44.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let font = FontId::new(13.0, FontFamily::Proportional);
                let msg = tf!("Konverterar {name} med LibreOffice … {s} s", name = name, s = secs);
                let galley = ui.painter().layout_no_wrap(msg, font, p.text);
                let size = vec2(galley.size().x.max(220.0) + 24.0, galley.size().y + 30.0);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 8.0, p.surface);
                ui.painter().rect_stroke(rect, 8.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
                ui.painter().galley(pos2(rect.left() + 12.0, rect.top() + 8.0), galley, p.text);
                let bar = Rect::from_min_size(pos2(rect.left() + 12.0, rect.bottom() - 12.0), vec2(size.x - 24.0, 4.0));
                ui.painter().rect_filled(bar, 2.0, Color32::from_rgba_unmultiplied(128, 128, 128, 60));
                // Vet inte hur lång tid det tar: stapeln visar tiden mot tidsgränsen.
                let frac = (j.started.elapsed().as_secs_f32() / TIMEOUT.as_secs_f32()).clamp(0.02, 1.0);
                ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, 4.0)), 2.0, p.accent);
            });
    }
}

// ------------------------------------------------------------------ enkel RTF-läsare

/// Enkel läsning av RTF: text, fet, kursiv och stycken. Ger Markdown.
pub fn rtf_to_markdown(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(b"{\\rtf") {
        return None;
    }
    #[derive(Clone, Default)]
    struct Group {
        bold: bool,
        italic: bool,
        skip: bool,
        /// Antal tecken efter `\uN` som är reservtext och hoppas över.
        uc: usize,
    }
    /// (text, fet, kursiv) i det stycke som byggs.
    type Span = (String, bool, bool);
    fn flush(spans: &mut Vec<Span>, out: &mut Vec<String>) {
        let mut line = String::new();
        let mut i = 0;
        while i < spans.len() {
            let (b, it) = (spans[i].1, spans[i].2);
            let mut text = String::new();
            while i < spans.len() && spans[i].1 == b && spans[i].2 == it {
                text.push_str(&escape_inline(&spans[i].0));
                i += 1;
            }
            if b {
                text = wrap(&text, "**");
            }
            if it {
                text = wrap(&text, "*");
            }
            line.push_str(&text);
        }
        spans.clear();
        let line = line.trim().to_owned();
        if !line.is_empty() {
            out.push(escape_line_start(&line));
        }
    }

    let mut stack: Vec<Group> = Vec::new();
    let mut cur = Group { uc: 1, ..Default::default() };
    let mut spans: Vec<Span> = Vec::new();
    let mut paras: Vec<String> = Vec::new();
    let push = |cur: &Group, spans: &mut Vec<Span>, s: &str| {
        if !cur.skip && !s.is_empty() {
            spans.push((s.to_owned(), cur.bold, cur.italic));
        }
    };
    let mut skip_chars = 0usize;
    let mut i = 0;
    let mut first_word = false;
    let n = bytes.len();
    while i < n {
        let c = bytes[i];
        match c {
            b'{' => {
                stack.push(cur.clone());
                first_word = true;
                i += 1;
            }
            b'}' => {
                cur = stack.pop().unwrap_or_default();
                first_word = false;
                i += 1;
            }
            b'\r' | b'\n' => i += 1,
            b'\\' => {
                i += 1;
                let Some(&d) = bytes.get(i) else { break };
                if !d.is_ascii_alphabetic() {
                    i += 1;
                    match d {
                        b'\'' => {
                            let hex = bytes.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                            i += 2;
                            if skip_chars > 0 {
                                skip_chars -= 1;
                            } else if let Some(b) = hex {
                                let one = [b];
                                let (s, _, _) = encoding_rs::WINDOWS_1252.decode(&one);
                                push(&cur, &mut spans, &s);
                            }
                        }
                        b'*' => cur.skip = true,
                        b'\\' | b'{' | b'}' => {
                            if skip_chars > 0 {
                                skip_chars -= 1;
                            } else {
                                push(&cur, &mut spans, &(d as char).to_string());
                            }
                        }
                        b'~' => push(&cur, &mut spans, "\u{a0}"),
                        b'_' => push(&cur, &mut spans, "-"),
                        _ => {}
                    }
                    first_word = false;
                    continue;
                }
                let start = i;
                while i < n && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                let num_start = i;
                if i < n && bytes[i] == b'-' {
                    i += 1;
                }
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let param: Option<i32> = std::str::from_utf8(&bytes[num_start..i]).ok().and_then(|s| s.parse().ok());
                if i < n && bytes[i] == b' ' {
                    i += 1;
                }
                if first_word {
                    first_word = false;
                    if matches!(word, "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "header" | "footer" | "footnote" | "object" | "fldinst" | "listtable" | "listoverridetable" | "generator" | "themedata" | "datastore") {
                        cur.skip = true;
                    }
                }
                match word {
                    "par" | "sect" | "page" => flush(&mut spans, &mut paras),
                    "line" => push(&cur, &mut spans, " "),
                    "tab" => push(&cur, &mut spans, " "),
                    "b" => cur.bold = param != Some(0),
                    "i" => cur.italic = param != Some(0),
                    "plain" => {
                        cur.bold = false;
                        cur.italic = false;
                    }
                    "uc" => cur.uc = param.unwrap_or(1).max(0) as usize,
                    "u" => {
                        let v = param.unwrap_or(0);
                        let v = if v < 0 { v + 65536 } else { v } as u32;
                        if let Some(ch) = char::from_u32(v) {
                            push(&cur, &mut spans, &ch.to_string());
                        }
                        skip_chars = cur.uc;
                    }
                    "emdash" => push(&cur, &mut spans, "\u{2014}"),
                    "endash" => push(&cur, &mut spans, "\u{2013}"),
                    "lquote" => push(&cur, &mut spans, "\u{2018}"),
                    "rquote" => push(&cur, &mut spans, "\u{2019}"),
                    "ldblquote" => push(&cur, &mut spans, "\u{201c}"),
                    "rdblquote" => push(&cur, &mut spans, "\u{201d}"),
                    "bullet" => push(&cur, &mut spans, "\u{2022}"),
                    _ => {}
                }
            }
            _ => {
                first_word = false;
                if skip_chars > 0 {
                    skip_chars -= 1;
                    i += 1;
                    continue;
                }
                // Ett helt tecken (UTF-8 förekommer i moderna filer; annars ISO-8859-1).
                let len = match c {
                    0xC0..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    0xF0..=0xF7 => 4,
                    _ => 1,
                };
                let end = (i + len).min(n);
                match std::str::from_utf8(&bytes[i..end]) {
                    Ok(s) => push(&cur, &mut spans, s),
                    Err(_) => {
                        let (s, _, _) = encoding_rs::WINDOWS_1252.decode(&bytes[i..i + 1]);
                        push(&cur, &mut spans, &s);
                        i += 1;
                        continue;
                    }
                }
                i = end;
            }
        }
    }
    flush(&mut spans, &mut paras);
    Some(paras.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;

    #[test]
    fn detects_what_to_convert() {
        assert!(classify("doc", &OLE_MAGIC));
        assert!(!classify("doc", b"PK\x03\x04"));
        assert!(!classify("doc", b"Vanlig text"));
        assert!(classify("rtf", b"{\\rtf1\\ansi"));
        assert!(!classify("rtf", b"just text"));
        assert!(classify("wpd", b"\xFFWPC"));
        assert!(!classify("txt", &OLE_MAGIC));
    }

    #[test]
    fn needs_conversion_reads_the_file() {
        let dir = TempDir::new().unwrap();
        let ole = dir.0.join("a.DOC");
        std::fs::write(&ole, [0xD0, 0xCF, 0x11, 0xE0, 0, 0, 0, 0, 1]).unwrap();
        assert!(needs_conversion(&ole));
        let zip = dir.0.join("b.doc");
        std::fs::write(&zip, b"PK\x03\x04....").unwrap();
        assert!(!needs_conversion(&zip));
        assert!(!needs_conversion(&dir.0.join("saknas.rtf")));
        assert!(!needs_conversion(&dir.0.join("c.txt")));
    }

    #[test]
    fn finds_soffice_in_path_and_extras() {
        let dir = TempDir::new().unwrap();
        let bin = dir.0.join(if cfg!(windows) { "soffice.exe" } else { "soffice" });
        std::fs::write(&bin, b"").unwrap();
        let other = dir.0.join("tom");
        std::fs::create_dir_all(&other).unwrap();
        let path = std::env::join_paths([&other, &dir.0]).unwrap();
        assert_eq!(find_in(Some(&path), &[]), Some(bin.clone()));
        assert_eq!(find_in(None, std::slice::from_ref(&bin)), Some(bin.clone()));
        assert_eq!(find_in(Some(other.as_os_str()), &[other.join("nej")]), None);
        assert_eq!(find_in(None, &[]), None);
    }

    #[test]
    fn builds_the_command() {
        assert_eq!(file_url(Path::new("/tmp/a b/profil")), "file:///tmp/a%20b/profil");
        assert_eq!(file_url(Path::new(r"C:\Temp\p")), "file:///C:/Temp/p");
        let args = convert_args(Path::new("/tmp/x/profil"), Path::new("/tmp/x/ut"), Path::new("/h/brev.doc"));
        let s: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(s[0], "-env:UserInstallation=file:///tmp/x/profil");
        assert!(s.contains(&"--headless".to_owned()));
        let at = s.iter().position(|a| a == "--convert-to").unwrap();
        assert_eq!(s[at + 1], "docx");
        let at = s.iter().position(|a| a == "--outdir").unwrap();
        assert_eq!(s[at + 1], "/tmp/x/ut");
        assert_eq!(s.last().unwrap(), "/h/brev.doc");
    }

    #[test]
    fn missing_libreoffice_error_is_clear() {
        let m = missing_message(Path::new("/h/brev.doc"));
        assert!(m.contains("brev.doc") && m.contains("LibreOffice") && m.contains("libreoffice.org"), "{m}");
    }

    #[test]
    fn failing_converter_gives_an_error_not_a_panic() {
        let dir = TempDir::new().unwrap();
        let input = dir.0.join("x.doc");
        std::fs::write(&input, [0xD0, 0xCF, 0x11, 0xE0]).unwrap();
        // Finns inte alls.
        assert!(convert(&dir.0.join("saknas"), &input, Duration::from_secs(5)).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Ett program som lyckas men inte skriver någon fil.
            let ok = dir.0.join("true.sh");
            std::fs::write(&ok, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(0o755)).unwrap();
            let e = convert(&ok, &input, Duration::from_secs(5)).unwrap_err();
            assert!(e.contains("LibreOffice"), "{e}");
            // Ett program som hänger sig: tidsgränsen avbryter det.
            let hang = dir.0.join("hang.sh");
            std::fs::write(&hang, "#!/bin/sh\nexec sleep 30\n").unwrap();
            std::fs::set_permissions(&hang, std::fs::Permissions::from_mode(0o755)).unwrap();
            let t0 = Instant::now();
            let e = convert(&hang, &input, Duration::from_millis(300)).unwrap_err();
            assert!(t0.elapsed() < Duration::from_secs(10) && e.contains("svarade inte"), "{e}");
            // Ett program som skriver resultatet.
            let fake = dir.0.join("fake.sh");
            std::fs::write(&fake, "#!/bin/sh\nfor a; do last=\"$a\"; done\nprev=\"\"\nfor a; do [ \"$prev\" = \"--outdir\" ] && out=\"$a\"; prev=\"$a\"; done\nprintf 'DOCX' > \"$out/x.docx\"\n").unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(convert(&fake, &input, Duration::from_secs(5)).unwrap(), b"DOCX");
        }
    }

    #[test]
    fn rtf_reader_gets_text_styles_and_paragraphs() {
        let rtf = br"{\rtf1\ansi\deff0{\fonttbl{\f0 Arial;}}{\colortbl;\red0\green0\blue0;}{\*\generator Hej;}\pard Hej \b v\'e4rlden\b0  och \i kursivt\i0 .\par Andra stycket med \u246?l.\par}";
        let md = rtf_to_markdown(rtf).unwrap();
        assert_eq!(md, "Hej **världen** och *kursivt*.\n\nAndra stycket med \u{f6}l.");
        assert!(rtf_to_markdown(b"inte rtf").is_none());
    }

    #[test]
    fn rtf_reader_escapes_markdown_and_skips_groups() {
        let md = rtf_to_markdown(br"{\rtf1{\info{\title Dold}}1. * punkt\par # rubrik}").unwrap();
        assert!(!md.contains("Dold"));
        assert_eq!(md, "1\\. \\* punkt\n\n\\# rubrik");
    }

    fn data(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
    }

    #[test]
    fn builtin_rtf_reads_the_fixture() {
        let md = rtf_to_markdown(&std::fs::read(data("aldre-brev.rtf")).unwrap()).unwrap();
        assert!(md.contains("**skörden**") && md.contains("*någonsin*") && md.contains("Åsa och Örjan från Västerås"), "{md}");
        let doc = Document::from_rtf_text(1, &data("aldre-brev.rtf"), &std::fs::read(data("aldre-brev.rtf")).unwrap()).unwrap();
        assert!(doc.docx && doc.needs_save_as() && !doc.dirty);
    }

    #[test]
    fn plain_loader_gives_a_clear_error_for_doc() {
        let e = Document::load(1, &data("aldre-brev.doc")).err().unwrap();
        assert!(e.contains("LibreOffice"), "{e}");
    }

    /// Konverterar de riktiga filerna med LibreOffice (kör med `cargo test -- --ignored`).
    #[test]
    #[ignore = "kräver LibreOffice"]
    fn soffice_converts_doc_and_rtf() {
        let Some(soffice) = find_soffice() else {
            eprintln!("hoppar över: LibreOffice saknas");
            return;
        };
        for name in ["aldre-brev.doc", "aldre-brev.rtf"] {
            let path = data(name);
            assert!(needs_conversion(&path), "{name}");
            let docx = convert(&soffice, &path, TIMEOUT).unwrap_or_else(|e| panic!("{name}: {e}"));
            let doc = Document::from_converted(1, &path, docx).unwrap();
            assert!(doc.text.contains("Brev om äppelskörden") && doc.text.contains("Västerås"), "{name}: {}", doc.text);
            assert!(doc.docx && doc.word_view && doc.needs_save_as() && !doc.dirty, "{name}");
            assert!(doc.open_note.as_deref().is_some_and(|n| n.contains("LibreOffice")));
            assert_eq!(doc.path.as_deref(), Some(path.as_path()));
        }
    }
}
