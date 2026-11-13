//! Korpus av riktiga dokument (`tests/corpus/`, källor och licenser i `SOURCES.md`).
//!
//! Varje fil öppnas (Word/ODT via våra läsare till Markdown, PDF via `pdf_read` och `PdfEngine`),
//! sparas oförändrad (läses om till samma Markdown), sparas efter en liten ändring (resten är
//! kvar) och, med LibreOffice installerat, öppnas som PDF där. Förväntningarna ligger i
//! `tests/corpus/manifest.toml`; `KLADD_CORPUS_BLESS=1 cargo test corpus` skriver om de uppmätta
//! värdena (rubriker, tappade delar, sidor) och lämnar handskrivna nycklar orörda.
use crate::docx::Image;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("corpus")
}

// ============================================================== manifest

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

/// Fil (relativ sökväg) → nyckel → värde, i filordning.
type Manifest = BTreeMap<String, BTreeMap<String, Val>>;

fn parse_string(s: &str) -> Option<(String, &str)> {
    let mut chars = s.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    let body = &s[1..];
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &body[i + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

fn parse_val(s: &str) -> Val {
    let s = s.trim();
    if let Some((v, _)) = parse_string(s) {
        return Val::Str(v);
    }
    if let Some(inner) = s.strip_prefix('[') {
        let mut rest = inner.trim_start();
        let mut list = Vec::new();
        while let Some((v, r)) = parse_string(rest) {
            list.push(v);
            rest = r.trim_start().trim_start_matches(',').trim_start();
        }
        return Val::List(list);
    }
    match s {
        "true" => Val::Bool(true),
        "false" => Val::Bool(false),
        _ => Val::Int(s.parse().unwrap_or_else(|_| panic!("manifestet: kan inte tolka värdet {s:?}"))),
    }
}

fn parse_manifest(text: &str) -> Manifest {
    let mut m = Manifest::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(h) = line.strip_prefix("[\"").and_then(|l| l.strip_suffix("\"]")) {
            m.entry(h.to_string()).or_default();
            cur = Some(h.to_string());
        } else if let Some((k, v)) = line.split_once('=') {
            let file = cur.as_ref().expect("nyckel utanför en filsektion i manifestet");
            m.get_mut(file).unwrap().insert(k.trim().to_string(), parse_val(v));
        } else {
            panic!("manifestet: oförståelig rad {line:?}");
        }
    }
    m
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
}

fn write_manifest(m: &Manifest) -> String {
    let mut out = String::from(
        "# Förväntningar för korpusen (se SOURCES.md för källor). En sektion per fil.\n\
         # Uppmätta värden (headings, dropped, pages) skrivs om med KLADD_CORPUS_BLESS=1 cargo test corpus.\n\
         # Handskrivna nycklar: error (förväntat läsfel), empty, note, skip_idempotent, skip_edit,\n\
         # skip_render, skip_soffice (värdet är orsaken).\n",
    );
    for (file, keys) in m {
        out.push_str(&format!("\n[\"{file}\"]\n"));
        for (k, v) in keys {
            let v = match v {
                Val::Str(s) => quote(s),
                Val::Int(i) => i.to_string(),
                Val::Bool(b) => b.to_string(),
                Val::List(l) => format!("[{}]", l.iter().map(|s| quote(s)).collect::<Vec<_>>().join(", ")),
            };
            out.push_str(&format!("{k} = {v}\n"));
        }
    }
    out
}

fn load_manifest() -> Manifest {
    parse_manifest(&std::fs::read_to_string(root().join("manifest.toml")).unwrap_or_default())
}

fn bless() -> bool {
    std::env::var_os("KLADD_CORPUS_BLESS").is_some()
}

/// Filer i en korpusmapp med given ändelse, sorterade.
fn files(dir: &str, ext: &str) -> Vec<(String, PathBuf)> {
    let mut v: Vec<_> = std::fs::read_dir(root().join(dir))
        .unwrap_or_else(|e| panic!("korpusmappen {dir} saknas: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .map(|p| (format!("{dir}/{}", p.file_name().unwrap().to_string_lossy()), p))
        .collect();
    v.sort();
    v
}

// ============================================================== gemensamt

const ADDED: &str = "Korpus-tillägg ÅÄÖ: ändrad rad.";

fn panic_text(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panik".into())
}

/// Kör `f` och gör en panik till ett fel.
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|e| format!("PANIK: {}", panic_text(e)))
}

/// Antal rubriker i Markdown (utanför kodblock).
fn heading_count(md: &str) -> i64 {
    let mut in_code = false;
    let mut n = 0;
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        } else if !in_code && line.starts_with('#') && line.trim_start_matches('#').starts_with(' ') {
            n += 1;
        }
    }
    n
}

/// Kontroller av att texten är rimlig: inga styrtecken, inget läckt XML.
fn sanity(md: &str) -> Result<(), String> {
    if let Some(c) = md.chars().find(|c| c.is_control() && !matches!(c, '\n' | '\t')) {
        return Err(format!("styrtecken U+{:04X} i texten", c as u32));
    }
    for marker in ["<w:", "</w:", "<text:", "</text:", "<office:", "xmlns:"] {
        // Text som handlar om XML står med snedstreck framför (`\<w:`).
        if md.match_indices(marker).any(|(i, _)| !md[..i].ends_with('\\')) {
            return Err(format!("läckt XML ({marker}) i texten"));
        }
    }
    Ok(())
}

/// Bildnamn är godtyckliga (`bild:1000…png` blir `bild:image1.png` vid sparning): jämför utan dem.
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s.trim_end();
    while let Some(i) = rest.find(crate::docx::IMAGE_SCHEME) {
        let end = rest[i..].find(|c: char| c.is_whitespace() || matches!(c, '"' | ')' | '>')).map_or(rest.len(), |e| i + e);
        out.push_str(&rest[..i]);
        out.push_str("bild:*");
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn same(a: &str, b: &str) -> bool {
    norm(a) == norm(b)
}

/// Texten utan grundformatsraden (`<!-- kladd: … -->`) överst.
fn body(md: &str) -> &str {
    match md.split_once('\n') {
        Some((first, rest)) if first.starts_with("<!-- kladd:") => rest.trim_start(),
        _ => md,
    }
}

fn same_body(a: &str, b: &str) -> bool {
    same(body(a), body(b))
}

/// Texten med det tillagda stycket (vilken rad som helst som innehåller `ADDED`) borttaget och
/// tomma rader ihopdragna, utan grundformatsraden.
fn without_added(md: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for l in body(md).lines().filter(|l| !l.contains(ADDED)) {
        if !(l.trim().is_empty() && out.last().is_none_or(|p| p.trim().is_empty())) {
            out.push(l);
        }
    }
    out.join("\n")
}

/// Lägger `ADDED` som ett eget stycke sist i brödtexten (före fotnotsdefinitioner m.m. som står sist).
fn with_added(md: &str) -> String {
    let lines: Vec<&str> = md.trim_end().lines().collect();
    let meta = |l: &&str| l.trim().is_empty() || l.starts_with("[^") || l.starts_with("    ") || l.starts_with("<!--k:c ");
    let mut cut = lines.len();
    while cut > 0 && meta(&lines[cut - 1]) {
        cut -= 1;
    }
    let (head, tail) = lines.split_at(cut);
    let mut out = head.join("\n");
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(ADDED);
    out.push('\n');
    if !tail.is_empty() {
        out.push_str(&format!("\n{}\n", tail.join("\n").trim_end()));
    }
    out
}

/// Första raden där två texter skiljer sig, för felmeddelanden.
fn first_diff(a: &str, b: &str) -> String {
    let (a, b) = (norm(a), norm(b));
    for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            let cut = |s: &str| s.chars().take(160).collect::<String>();
            let (x, y) = (cut(x), cut(y));
            return format!("rad {}: {:?} != {:?}", i + 1, x, y);
        }
    }
    format!("olika längd ({} mot {} rader)", a.lines().count(), b.lines().count())
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Docx,
    Odt,
}

struct Opened {
    markdown: String,
    images: Vec<(String, Image)>,
    dropped: Vec<&'static str>,
    source: Option<crate::docx::Source>,
}

fn open_doc(kind: Kind, bytes: &[u8]) -> Result<Opened, String> {
    let r = guarded(|| match kind {
        Kind::Docx => crate::docx::read(bytes),
        Kind::Odt => crate::odt::read(bytes),
    })?;
    r.map(|l| Opened { markdown: l.markdown, images: l.images, dropped: l.dropped, source: l.source })
}

fn save_doc(kind: Kind, md: &str, o: &Opened) -> Result<Vec<u8>, String> {
    let resolve = |src: &str| crate::doc::resolve_image(src, Path::new("."), &o.images);
    let r = guarded(|| match kind {
        Kind::Docx => crate::docx::write(md, "Korpus", &resolve, o.source.as_ref()),
        Kind::Odt => crate::odt::write(md, "Korpus", &resolve),
    })?;
    r.map(|w| w.bytes)
}

fn reread(kind: Kind, bytes: &[u8]) -> Result<String, String> {
    open_doc(kind, bytes).map(|o| o.markdown).map_err(|e| format!("sparad fil kan inte läsas om: {e}"))
}

fn is_zip_with(bytes: &[u8], part: &str) -> bool {
    zip::ZipArchive::new(std::io::Cursor::new(bytes)).is_ok_and(|mut z| z.by_name(part).is_ok())
}

/// Resultatet av att spara orört och efter en ändring, för LibreOffice-kontrollen.
struct Saved {
    name: String,
    same: Vec<u8>,
    edited: Vec<u8>,
}

fn check_doc(kind: Kind, name: &str, path: &Path, man: &mut Manifest, saved: &mut Vec<Saved>) -> Vec<String> {
    let mut problems = Vec::new();
    let entry = man.entry(name.to_string()).or_default();
    let bytes = std::fs::read(path).unwrap();
    let opened = open_doc(kind, &bytes);
    let o = match opened {
        Err(e) => {
            if e.starts_with("PANIK") {
                problems.push(format!("{name}: {e}"));
            } else if bless() {
                entry.entry("error".into()).or_insert(Val::Str(e.chars().take(60).collect()));
            } else if let Some(Val::Str(want)) = entry.get("error") {
                if !e.contains(want.as_str()) {
                    problems.push(format!("{name}: läsfelet {e:?} innehåller inte {want:?}"));
                }
            } else {
                problems.push(format!("{name}: kunde inte läsas: {e}"));
            }
            return problems;
        }
        Ok(o) => o,
    };
    if let Some(Val::Str(want)) = entry.get("error") {
        problems.push(format!("{name}: väntade läsfel ({want}) men filen lästes"));
    }
    if let Err(e) = sanity(&o.markdown) {
        problems.push(format!("{name}: {e}"));
    }
    if o.markdown.trim().is_empty() && !matches!(entry.get("empty"), Some(Val::Bool(true))) {
        problems.push(format!("{name}: tom text (lägg `empty = true` i manifestet om det är rätt)"));
    }

    // Uppmätta förväntningar.
    let headings = heading_count(&o.markdown);
    let dropped: Vec<String> = o.dropped.iter().map(|s| s.to_string()).collect();
    if bless() {
        entry.insert("headings".into(), Val::Int(headings));
        entry.insert("dropped".into(), Val::List(dropped.clone()));
        entry.retain(|k, _| k != "error");
    } else {
        match entry.get("headings") {
            Some(Val::Int(n)) if *n != headings => problems.push(format!("{name}: {headings} rubriker, manifestet säger {n}")),
            None => problems.push(format!("{name}: saknas i manifestet (kör med KLADD_CORPUS_BLESS=1)")),
            _ => {}
        }
        if let Some(Val::List(want)) = entry.get("dropped") {
            if *want != dropped {
                problems.push(format!("{name}: tappade delar {dropped:?}, manifestet säger {want:?}"));
            }
        }
    }

    // Spara oförändrad: giltigt paket och samma Markdown vid omläsning.
    let skip = |k: &str| match entry.get(k) {
        Some(Val::Str(_)) => true,
        _ => false,
    };
    let (skip_idem, skip_edit) = (skip("skip_idempotent"), skip("skip_edit"));
    let part = if kind == Kind::Docx { "[Content_Types].xml" } else { "content.xml" };
    let same_bytes = match save_doc(kind, &o.markdown, &o) {
        Err(e) => {
            problems.push(format!("{name}: spara oförändrad: {e}"));
            return problems;
        }
        Ok(b) => b,
    };
    if !is_zip_with(&same_bytes, part) {
        problems.push(format!("{name}: sparad fil är inget giltigt paket (saknar {part})"));
    }
    let o2 = match open_doc(kind, &same_bytes) {
        Err(e) => {
            problems.push(format!("{name}: sparad fil kan inte läsas om: {e}"));
            return problems;
        }
        Ok(o2) => o2,
    };
    let again = o2.markdown.clone();
    if !skip_idem && !same_body(&again, &o.markdown) {
        problems.push(format!("{name}: sparad oförändrad ger annan text: {}", first_diff(body(&o.markdown), body(&again))));
    }
    // Grundformatsraden får ändras av första sparningen (Kladds egna mallar läggs till), men en andra
    // sparning ska ge exakt samma text som den första omläsningen.
    if !skip_idem {
        match save_doc(kind, &again, &o2).and_then(|b| reread(kind, &b)) {
            Err(e) => problems.push(format!("{name}: andra sparningen: {e}")),
            Ok(third) if !same(&third, &again) => problems.push(format!("{name}: andra sparningen ändrar texten: {}", first_diff(&again, &third))),
            Ok(_) => {}
        }
    }

    // Spara efter en liten ändring: texten är den som sparades orörd plus tillägget.
    let edited_md = with_added(&o.markdown);
    let edited_bytes = match save_doc(kind, &edited_md, &o) {
        Err(e) => {
            problems.push(format!("{name}: spara efter ändring: {e}"));
            return problems;
        }
        Ok(b) => b,
    };
    match reread(kind, &edited_bytes) {
        Err(e) => problems.push(format!("{name}: efter ändring: {e}")),
        Ok(after) if !skip_edit => {
            if !after.contains(ADDED) {
                problems.push(format!("{name}: tillagd text saknas efter sparning"));
            } else if norm(&without_added(&after)) != norm(&without_added(&again)) {
                problems.push(format!("{name}: ändring tappade resten: {}", first_diff(&without_added(&again), &without_added(&after))));
            }
        }
        Ok(_) => {}
    }
    saved.push(Saved { name: name.to_string(), same: same_bytes, edited: edited_bytes });
    problems
}

fn run_doc_corpus(kind: Kind, dir: &str, ext: &str) -> Vec<Saved> {
    let mut man = load_manifest();
    let mut saved = Vec::new();
    let mut problems = Vec::new();
    let all = files(dir, ext);
    assert!(all.len() >= 10, "korpusmappen {dir} ser tom ut");
    for (name, path) in &all {
        problems.extend(check_doc(kind, name, path, &mut man, &mut saved));
    }
    finish(man, problems, dir);
    saved
}

/// Skriver manifestet (vid BLESS) eller faller med alla problem på en gång.
fn finish(man: Manifest, problems: Vec<String>, dir: &str) {
    if bless() {
        // Flera tester skriver till samma fil: läs om och slå ihop bara den här mappens sektioner.
        let mut cur = load_manifest();
        for (k, v) in man {
            if k.starts_with(&format!("{dir}/")) {
                cur.insert(k, v);
            }
        }
        let on_disk: Vec<String> = cur.keys().filter(|k| !root().join(k.as_str()).exists()).cloned().collect();
        for k in on_disk {
            cur.remove(&k);
        }
        std::fs::write(root().join("manifest.toml"), write_manifest(&cur)).unwrap();
        // Bara problem som inte är manifestavvikelser är fel också i bless-läge.
        let hard: Vec<_> = problems.into_iter().filter(|p| !p.contains("manifest")).collect();
        assert!(hard.is_empty(), "{} fel:\n{}", hard.len(), hard.join("\n"));
        return;
    }
    assert!(problems.is_empty(), "{} fel i {dir}:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn corpus_docx() {
    let saved = run_doc_corpus(Kind::Docx, "docx", "docx");
    assert!(saved.len() > 100);
}

#[test]
fn corpus_odt() {
    let saved = run_doc_corpus(Kind::Odt, "odt", "odt");
    assert!(saved.len() > 100);
}

// ============================================================== PDF

fn render_first_page(bytes: Vec<u8>) -> Result<(usize, (usize, usize)), String> {
    use crate::pdfview::engine::{EngineEvent, PdfEngine, Recolor, RenderRequest};
    let engine = PdfEngine::open(std::sync::Arc::new(bytes), None, egui::Context::default()).map_err(|e| format!("PdfEngine::open: {e:?}"))?;
    let pages = engine.page_count();
    let req = RenderRequest { page: 0, scale: 0.25, rotation: 0, recolor: Recolor::None };
    engine.request(vec![req]);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < end {
        for ev in engine.poll() {
            match ev {
                EngineEvent::Rendered { image, .. } => {
                    let size = (image.size[0], image.size[1]);
                    if size.0 == 0 || size.1 == 0 {
                        return Err("första sidan renderades med storlek 0".into());
                    }
                    return Ok((pages, size));
                }
                EngineEvent::Failed { error, .. } => return Err(format!("rendering misslyckades: {error}")),
                EngineEvent::Text { .. } => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Err("rendering av första sidan tog över 60 s".into())
}

fn check_pdf(name: &str, path: &Path, man: &mut Manifest) -> Vec<String> {
    use crate::pdf_read::ReadError;
    let mut problems = Vec::new();
    let entry = man.entry(name.to_string()).or_default();
    let bytes = std::fs::read(path).unwrap();
    let read = match guarded(|| crate::pdf_read::read(&bytes, None)) {
        Err(e) => return vec![format!("{name}: pdf_read {e}")],
        Ok(r) => r,
    };
    let encrypted = matches!(read, Err(ReadError::NeedsPassword));
    match &read {
        Err(ReadError::NeedsPassword) => {
            if bless() {
                entry.insert("error".into(), Val::Str("lösenord".into()));
            } else if !matches!(entry.get("error"), Some(Val::Str(s)) if s == "lösenord") {
                problems.push(format!("{name}: kräver lösenord men manifestet säger inte det"));
            }
        }
        Err(e) => {
            let msg = match e {
                ReadError::WrongPassword => "fel lösenord".to_string(),
                ReadError::Other(s) => s.clone(),
                ReadError::NeedsPassword => unreachable!(),
            };
            if bless() {
                entry.insert("error".into(), Val::Str(msg.chars().take(60).collect()));
            } else if !matches!(entry.get("error"), Some(Val::Str(s)) if msg.contains(s.as_str())) {
                problems.push(format!("{name}: pdf_read misslyckades oväntat: {msg}"));
            }
        }
        Ok(l) => {
            if let Err(e) = sanity(&l.markdown) {
                problems.push(format!("{name}: {e}"));
            }
            if let Some(Val::Str(want)) = entry.get("error") {
                problems.push(format!("{name}: väntade läsfel ({want}) men filen lästes"));
            }
            if bless() {
                entry.remove("error");
                entry.insert("headings".into(), Val::Int(heading_count(&l.markdown)));
                entry.insert("dropped".into(), Val::List(l.dropped.iter().map(|s| s.to_string()).collect()));
            } else {
                if let Some(Val::Int(n)) = entry.get("headings") {
                    if *n != heading_count(&l.markdown) {
                        problems.push(format!("{name}: {} rubriker, manifestet säger {n}", heading_count(&l.markdown)));
                    }
                }
                if let Some(Val::List(want)) = entry.get("dropped") {
                    let got: Vec<String> = l.dropped.iter().map(|s| s.to_string()).collect();
                    if *want != got {
                        problems.push(format!("{name}: tappade delar {got:?}, manifestet säger {want:?}"));
                    }
                }
            }
            // Läsning med och utan OCR-karta ska ge samma resultat när kartan är tom.
            if l.page_count == 0 {
                problems.push(format!("{name}: pdf_read ger 0 sidor"));
            }
        }
    }
    if encrypted {
        return problems; // PdfEngine::open ger NeedsPassword utan lösenord; täcks av andra tester
    }
    // Sidantal och rendering av första sidan.
    let pages_read = read.as_ref().ok().map(|l| l.page_count);
    let skip_render = matches!(entry.get("skip_render"), Some(Val::Str(_)));
    match guarded(|| render_first_page(bytes.clone())) {
        Err(e) => problems.push(format!("{name}: {e}")),
        Ok(Err(e)) => {
            if !skip_render {
                problems.push(format!("{name}: {e}"));
            }
        }
        Ok(Ok((pages, _size))) => {
            if let Some(n) = pages_read {
                if n != pages {
                    problems.push(format!("{name}: pdf_read ser {n} sidor, PdfEngine {pages}"));
                }
            }
            if bless() {
                entry.insert("pages".into(), Val::Int(pages as i64));
            } else if let Some(Val::Int(n)) = entry.get("pages") {
                if *n as usize != pages {
                    problems.push(format!("{name}: {pages} sidor, manifestet säger {n}"));
                }
            }
        }
    }
    problems
}

#[test]
fn corpus_pdf() {
    let mut man = load_manifest();
    let mut problems = Vec::new();
    let all = files("pdf", "pdf");
    assert!(all.len() >= 10, "korpusmappen pdf ser tom ut");
    for (name, path) in &all {
        problems.extend(check_pdf(name, path, &mut man));
    }
    finish(man, problems, "pdf");
}

// ============================================================== LibreOffice

fn soffice() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &["soffice.exe", "C:\\Program Files\\LibreOffice\\program\\soffice.exe"]
    } else if cfg!(target_os = "macos") {
        &["soffice", "/Applications/LibreOffice.app/Contents/MacOS/soffice"]
    } else {
        &["soffice", "libreoffice"]
    };
    candidates.iter().map(PathBuf::from).find(|c| {
        Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success())
    })
}

/// Kör `soffice --convert-to pdf` på alla filer i `dir` i ett anrop; ger de filer som inte blev en PDF.
fn convert_all(soffice: &Path, profile: &Path, dir: &Path, out: &Path, names: &[String]) -> Vec<String> {
    let _ = std::fs::create_dir_all(out);
    for chunk in names.chunks(80) {
        let mut cmd = Command::new(soffice);
        cmd.arg(format!("-env:UserInstallation=file://{}", profile.to_string_lossy().replace('\\', "/")))
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(out);
        for n in chunk {
            cmd.arg(dir.join(n));
        }
        let _ = cmd.output();
    }
    names
        .iter()
        .filter(|n| {
            let pdf = out.join(Path::new(n).with_extension("pdf"));
            !std::fs::read(&pdf).is_ok_and(|b| b.starts_with(b"%PDF"))
        })
        .cloned()
        .collect()
}

fn soffice_check(kind: Kind, dir: &str, ext: &str) {
    let required = std::env::var_os("KLADD_CORPUS_SOFFICE").is_some();
    let Some(soffice) = soffice() else {
        assert!(!required, "KLADD_CORPUS_SOFFICE är satt men soffice finns inte");
        eprintln!("soffice saknas: hoppar över LibreOffice-kontrollen");
        return;
    };
    let man = load_manifest();
    let mut man_scratch = man.clone();
    let mut saved = Vec::new();
    for (name, path) in files(dir, ext) {
        let entry = man.get(&name);
        if matches!(entry.and_then(|e| e.get("skip_soffice")), Some(Val::Str(_))) || matches!(entry.and_then(|e| e.get("error")), Some(Val::Str(_))) {
            continue;
        }
        check_doc(kind, &name, &path, &mut man_scratch, &mut saved);
    }
    let tmp = std::env::temp_dir().join(format!("kladd-corpus-{}-{dir}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (input, out) = (tmp.join("in"), tmp.join("out"));
    std::fs::create_dir_all(&input).unwrap();
    let mut names = Vec::new();
    for s in &saved {
        let base = s.name.rsplit('/').next().unwrap();
        let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
        for (suffix, bytes) in [("", &s.same), ("--andrad", &s.edited)] {
            let file = format!("{stem}{suffix}.{ext}");
            std::fs::write(input.join(&file), bytes).unwrap();
            names.push(file);
        }
    }
    let failed = convert_all(&soffice, &tmp.join("profil"), &input, &out, &names);
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(failed.is_empty(), "LibreOffice öppnade inte {} av {} sparade filer:\n{}", failed.len(), names.len(), failed.join("\n"));
}

/// Sparade `.docx` (orörda och ändrade) ska öppnas i LibreOffice. Körs med
/// `KLADD_CORPUS_SOFFICE=1 cargo test corpus_soffice -- --ignored`.
#[test]
#[ignore = "kräver LibreOffice (soffice)"]
fn corpus_soffice_docx() {
    soffice_check(Kind::Docx, "docx", "docx");
}

#[test]
#[ignore = "kräver LibreOffice (soffice)"]
fn corpus_soffice_odt() {
    soffice_check(Kind::Odt, "odt", "odt");
}

// ============================================================== manifestets läsare

#[test]
fn manifest_roundtrip() {
    let text = "[\"a/b.docx\"]\nheadings = 3\ndropped = [\"x\", \"y\\\"z\"]\nempty = true\nnote = \"hej\"\n";
    let m = parse_manifest(text);
    let e = &m["a/b.docx"];
    assert_eq!(e["headings"], Val::Int(3));
    assert_eq!(e["dropped"], Val::List(vec!["x".into(), "y\"z".into()]));
    assert_eq!(e["empty"], Val::Bool(true));
    assert_eq!(parse_manifest(&write_manifest(&m)), m);
}

/// Skriver ut Markdown före och efter sparning för en fil: `KLADD_CORPUS_DUMP=docx/namn.docx cargo test corpus_dump -- --ignored --nocapture`.
#[test]
#[ignore = "felsökningshjälp"]
fn corpus_dump() {
    let Ok(name) = std::env::var("KLADD_CORPUS_DUMP") else { return };
    let kind = if name.ends_with(".odt") { Kind::Odt } else { Kind::Docx };
    let o = open_doc(kind, &std::fs::read(root().join(&name)).unwrap()).unwrap();
    println!("===== läst\n{}\n===== dropped {:?}", o.markdown, o.dropped);
    let saved = save_doc(kind, &o.markdown, &o).unwrap();
    println!("===== omläst efter sparning\n{}", reread(kind, &saved).unwrap());
    let edited = save_doc(kind, &with_added(&o.markdown), &o).unwrap();
    println!("===== omläst efter ändring\n{}", reread(kind, &edited).unwrap());
}
