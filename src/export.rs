//! Export av Markdown-dokument till en fristående webbsida (.html) och en e-bok (EPUB 3, .epub).
//!
//! Båda går via pulldown-cmarks HTML-skrivare med tabeller, genomstrykning, uppgiftslistor och
//! fotnoter. Kladds egna tillägg hanteras däremellan och efteråt:
//!
//! - Grundformatskommentaren på första raden (`<!-- kladd: … -->`) styr typsnitt, storlek och sida
//!   (`@page`); den och `<!--k:N-->`-markörerna syns aldrig i resultatet.
//! - Tabeller skrivs ut här (inte av pulldown): `||` ger `colspan`, antalet bindestreck i
//!   avgränsarraden ger `<col style="width:…%">`, `<br>` i celler behålls.
//! - `<!-- kladd: toc -->` blir en nästlad lista med länkar till rubrikerna (som får `id`).
//! - Bilder bäddas in som `data:`-URI:er (HTML) eller läggs som filer i `OEBPS/images/` (EPUB).
//! - Rå HTML (`<span style>`, `<u>`, `<div align>`, `<img align>`) går igenom en tolerant
//!   normalisering (`Fixer`) som ger välformad XHTML: slutna tomma element, balanserade taggar,
//!   citerade attribut och giltiga entiteter.
//! - Fotnoter blir `<aside class="footnote">` sist i dokumentet (HTML) eller sist i det kapitel där
//!   de först nämns (EPUB, med `epub:type="noteref"`/`"footnote"`).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::ops::Range;

use pulldown_cmark::{html::push_html, Alignment, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::docx::{Image, Written};
use crate::style::DocDefaults;

/// Cell som bara innehåller detta tecken är en sammanfogad cell (`||` i källan).
const SPAN: &str = "\u{E000}";
/// Platshållare för innehållsförteckningen medan texten byggs.
const TOC_MARK: &str = "<!--kladd-toc-->";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Html,
    Epub,
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES
}

// ---------------------------------------------------------------------------------------------
// Offentligt API
// ---------------------------------------------------------------------------------------------

/// Bygger en fristående HTML-fil (UTF-8) av Markdown-text.
pub fn html(markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<Image>) -> Result<Written, String> {
    let (text, defaults) = prepare(markdown);
    let mut imgs = Images::new(Mode::Html, resolve);
    let model = build(&text, Mode::Html, &mut imgs);
    let doc_title = doc_title(&model, title);

    let lay = Layout { mode: Mode::Html, preface: true };
    let mut fixer = Fixer::new(Mode::Html);
    let mut body = render_chunk(&model, &lay, 0..model.body.len(), None, &mut fixer, &mut imgs);
    body.push_str(&fixer.close_all());

    let out = format!(
        "<!doctype html>\n<html lang=\"sv\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>\n{}</style>\n</head>\n<body>\n{}</body>\n</html>\n",
        esc(&doc_title),
        css(&defaults, Mode::Html),
        body
    );
    Ok(Written { bytes: out.into_bytes(), missing_images: imgs.missing })
}

/// Bygger en EPUB 3-fil av Markdown-text. Kapitlen delas vid rubriker på nivå 1.
pub fn epub(markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<Image>) -> Result<Written, String> {
    let (text, defaults) = prepare(markdown);
    let mut imgs = Images::new(Mode::Epub, resolve);
    let model = build(&text, Mode::Epub, &mut imgs);
    let doc_title = doc_title(&model, title);

    let first_split = model.splits.first().copied().unwrap_or(model.body.len());
    let preface = model.splits.is_empty() || has_content(&model.body[..first_split]);
    let lay = Layout { mode: Mode::Epub, preface };

    // Kapitel: index 0 = text före första H1, sedan ett per H1.
    let count = model.splits.len() + 1;
    let mut chapters: Vec<(usize, String)> = Vec::new();
    let mut fixer = Fixer::new(Mode::Epub);
    for c in 0..count {
        if c == 0 && !preface {
            continue;
        }
        let start = if c == 0 { 0 } else { model.splits[c - 1] };
        let end = model.splits.get(c).copied().unwrap_or(model.body.len());
        let mut body = fixer.reopen();
        body.push_str(&render_chunk(&model, &lay, start..end, Some(c), &mut fixer, &mut imgs));
        body.push_str(&fixer.close_all());
        chapters.push((c, body));
    }

    let mut files: Vec<(String, String, String)> = Vec::new(); // (fil, id, xhtml)
    for (c, body) in &chapters {
        let name = lay.file(*c);
        let ctitle = model
            .headings
            .iter()
            .find(|h| h.chapter == *c && !h.text.is_empty())
            .map(|h| h.text.clone())
            .unwrap_or_else(|| doc_title.clone());
        let xhtml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
             <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"sv\" lang=\"sv\">\n\
             <head>\n<meta charset=\"utf-8\" />\n<title>{}</title>\n<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\" />\n</head>\n\
             <body>\n{}</body>\n</html>\n",
            esc(&ctitle),
            body
        );
        let id = name.trim_end_matches(".xhtml").to_owned();
        files.push((name, id, xhtml));
    }

    // Navigering (EPUB 3 nav): alla rubriker, annars en post för första kapitlet.
    let items: Vec<(u8, String, String)> = model
        .headings
        .iter()
        .filter(|h| !h.text.is_empty())
        .map(|h| (h.level, esc(&h.text), format!("{}#{}", lay.file(h.chapter), h.id)))
        .collect();
    let nav_list = if items.is_empty() {
        let first = files.first().map(|f| f.0.clone()).unwrap_or_else(|| "chapter-1.xhtml".into());
        format!("<ol><li><a href=\"{first}\">{}</a></li></ol>", esc(&doc_title))
    } else {
        nested_list(&items, "ol")
    };
    let nav = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"sv\" lang=\"sv\">\n\
         <head>\n<meta charset=\"utf-8\" />\n<title>{}</title>\n<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\" />\n</head>\n\
         <body>\n<nav epub:type=\"toc\" id=\"toc\">\n<h1>Innehåll</h1>\n{}\n</nav>\n</body>\n</html>\n",
        esc(&doc_title),
        nav_list
    );

    let mut manifest = String::new();
    manifest.push_str("<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n");
    manifest.push_str("<item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n");
    for (name, id, _) in &files {
        manifest.push_str(&format!("<item id=\"{id}\" href=\"{name}\" media-type=\"application/xhtml+xml\"/>\n"));
    }
    for (i, f) in imgs.files.iter().enumerate() {
        manifest.push_str(&format!(
            "<item id=\"image-{}\" href=\"images/{}\" media-type=\"{}\"/>\n",
            i + 1,
            f.name,
            f.media
        ));
    }
    let spine: String = files.iter().map(|(_, id, _)| format!("<itemref idref=\"{id}\"/>\n")).collect();
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"book-id\" xml:lang=\"sv\">\n\
         <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
         <dc:identifier id=\"book-id\">urn:uuid:{}</dc:identifier>\n<dc:title>{}</dc:title>\n<dc:language>sv</dc:language>\n\
         <meta property=\"dcterms:modified\">{}</meta>\n</metadata>\n\
         <manifest>\n{}</manifest>\n<spine>\n{}</spine>\n</package>\n",
        uuid_v4(&doc_title),
        esc(&doc_title),
        crate::docx::now_w3cdtf(),
        manifest,
        spine
    );
    let container = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
        <rootfiles>\n<rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/>\n</rootfiles>\n</container>\n";

    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);
        let deflated = stored.compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6));
        let put = |zip: &mut zip::ZipWriter<_>, name: &str, bytes: &[u8], opts| -> Result<(), String> {
            zip.start_file(name, opts).map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())
        };
        // `mimetype` först och okomprimerad, utan extra fält.
        put(&mut zip, "mimetype", b"application/epub+zip", stored)?;
        put(&mut zip, "META-INF/container.xml", container.as_bytes(), deflated)?;
        put(&mut zip, "OEBPS/content.opf", opf.as_bytes(), deflated)?;
        put(&mut zip, "OEBPS/nav.xhtml", nav.as_bytes(), deflated)?;
        put(&mut zip, "OEBPS/style.css", css(&defaults, Mode::Epub).as_bytes(), deflated)?;
        for (name, _, xhtml) in &files {
            put(&mut zip, &format!("OEBPS/{name}"), xhtml.as_bytes(), deflated)?;
        }
        for f in &imgs.files {
            put(&mut zip, &format!("OEBPS/images/{}", f.name), &f.bytes, stored)?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(Written { bytes: buf, missing_images: imgs.missing })
}

// ---------------------------------------------------------------------------------------------
// Förberedelse
// ---------------------------------------------------------------------------------------------

/// Grundformat från första raden, och texten med `||` i tabeller utbytta mot markörceller.
fn prepare(md: &str) -> (String, DocDefaults) {
    let md = md.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let defaults = md.lines().next().and_then(DocDefaults::from_comment).unwrap_or_default();
    (mark_colspans(&md), defaults)
}

/// Byter tomma celler utan blanktecken (`||`) i tabellrader mot en cell med bara `SPAN`.
fn mark_colspans(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<char> = None;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let marker = if t.starts_with("```") {
            Some('`')
        } else if t.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        if let Some(m) = marker {
            match fence {
                None => fence = Some(m),
                Some(f) if f == m => fence = None,
                _ => {}
            }
            out.push_str(line);
            continue;
        }
        let is_delimiter = t.trim().chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'));
        if fence.is_none() && t.starts_with('|') && t.contains("||") && !is_delimiter {
            out.push_str(&span_line(line));
        } else {
            out.push_str(line);
        }
    }
    out
}

fn span_line(line: &str) -> String {
    let (body, nl) = match line.strip_suffix('\n') {
        Some(b) => (b, "\n"),
        None => (line, ""),
    };
    let indent = body.len() - body.trim_start().len();
    let trimmed = body.trim_end();
    let segs = split_row(&trimmed[indent..]);
    let n = segs.len();
    let mut s = body[..indent].to_owned();
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            s.push('|');
        }
        if i > 0 && i != n - 1 && seg.is_empty() {
            s.push(' ');
            s.push_str(SPAN);
            s.push(' ');
        } else {
            s.push_str(seg);
        }
    }
    s.push_str(nl);
    s
}

/// Delar en tabellrad vid `|` utanför kodspann och utan `\|`.
fn split_row(row: &str) -> Vec<String> {
    let mut segs = vec![String::new()];
    let mut in_code = false;
    let mut chars = row.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let cur = segs.last_mut().unwrap();
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '`' => {
                in_code = !in_code;
                segs.last_mut().unwrap().push(c);
            }
            '|' if !in_code => segs.push(String::new()),
            _ => segs.last_mut().unwrap().push(c),
        }
    }
    segs
}

// ---------------------------------------------------------------------------------------------
// Bilder
// ---------------------------------------------------------------------------------------------

struct MediaFile {
    name: String,
    media: &'static str,
    bytes: Vec<u8>,
}

struct Images<'r> {
    mode: Mode,
    resolve: &'r dyn Fn(&str) -> Option<Image>,
    cache: HashMap<String, Option<String>>,
    files: Vec<MediaFile>,
    missing: Vec<String>,
}

impl<'r> Images<'r> {
    fn new(mode: Mode, resolve: &'r dyn Fn(&str) -> Option<Image>) -> Self {
        Self { mode, resolve, cache: HashMap::new(), files: Vec::new(), missing: Vec::new() }
    }

    /// Ny källa för en bild i texten, eller `None` om bilden ska utelämnas.
    fn src(&mut self, raw: &str) -> Option<String> {
        let raw = raw.trim();
        if let Some(hit) = self.cache.get(raw) {
            return hit.clone();
        }
        let result = self.resolve_new(raw);
        if result.is_none() && !self.missing.iter().any(|m| m == raw) {
            let short: String = raw.chars().take(60).collect();
            self.missing.push(short);
        }
        self.cache.insert(raw.to_owned(), result.clone());
        result
    }

    fn resolve_new(&mut self, raw: &str) -> Option<String> {
        if raw.is_empty() {
            return None;
        }
        // Redan omskriven (bilder går genom normaliseringen en gång till).
        if self.mode == Mode::Epub && raw.strip_prefix("images/").is_some_and(|n| self.files.iter().any(|f| f.name == n)) {
            return Some(raw.to_owned());
        }
        let remote = raw.starts_with("http://") || raw.starts_with("https://");
        if remote || raw.starts_with("data:") {
            // Fjärrbilder och data-URI:er följer med i HTML; en e-bok ska vara självständig.
            return (self.mode == Mode::Html).then(|| raw.to_owned());
        }
        let img = (self.resolve)(raw)?;
        let (mut bytes, mut media, mut ext) = sniff(&img.bytes)?;
        if self.mode == Mode::Html {
            return Some(format!("data:{media};base64,{}", base64(&bytes)));
        }
        if media == "image/bmp" {
            // BMP är inte ett kärnformat i EPUB: gör om till PNG.
            let decoded = image::load_from_memory(&bytes).ok()?;
            let mut png = Vec::new();
            decoded.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
            bytes = png;
            media = "image/png";
            ext = "png";
        }
        let name = format!("img{}.{ext}", self.files.len() + 1);
        self.files.push(MediaFile { name: name.clone(), media, bytes });
        Some(format!("images/{name}"))
    }
}

/// Bildtyp, MIME och filändelse utifrån innehållet.
fn sniff(b: &[u8]) -> Option<(Vec<u8>, &'static str, &'static str)> {
    let (media, ext) = if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        ("image/png", "png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        ("image/jpeg", "jpg")
    } else if b.starts_with(b"GIF8") {
        ("image/gif", "gif")
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        ("image/webp", "webp")
    } else if b.starts_with(b"BM") {
        ("image/bmp", "bmp")
    } else {
        let head = String::from_utf8_lossy(&b[..b.len().min(512)]).to_ascii_lowercase();
        if head.contains("<svg") {
            ("image/svg+xml", "svg")
        } else {
            return None;
        }
    };
    Some((b.to_vec(), media, ext))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Modell: händelser med Kladds tillägg
// ---------------------------------------------------------------------------------------------

struct Heading {
    level: u8,
    text: String,
    id: String,
    chapter: usize,
}

struct Note<'a> {
    chapter: usize,
    events: Vec<Event<'a>>,
}

struct Model<'a> {
    body: Vec<Event<'a>>,
    /// Index i `body` där kapitel 1, 2 … börjar (vid H1 på översta nivån).
    splits: Vec<usize>,
    headings: Vec<Heading>,
    /// Fotnoter i löpnummerordning (nummer = index + 1).
    notes: Vec<Note<'a>>,
}

struct Ctx<'i, 'r> {
    mode: Mode,
    imgs: &'i mut Images<'r>,
    used_ids: HashSet<String>,
    headings: Vec<Heading>,
    chapter: usize,
    splits: Vec<usize>,
    /// Etikett -> (nummer, kapitel där den först nämns).
    note_nums: HashMap<String, (usize, usize)>,
}

fn build<'a>(text: &'a str, mode: Mode, imgs: &mut Images) -> Model<'a> {
    let mut main = Vec::new();
    let mut defs: Vec<(String, Vec<(Event<'a>, Range<usize>)>)> = Vec::new();
    let mut cur: Option<usize> = None;
    for (ev, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match ev {
            Event::Start(Tag::FootnoteDefinition(label)) => {
                defs.push((label.to_string(), Vec::new()));
                cur = Some(defs.len() - 1);
            }
            Event::End(TagEnd::FootnoteDefinition) => cur = None,
            e => match cur {
                Some(c) => defs[c].1.push((e, range)),
                None => main.push((e, range)),
            },
        }
    }
    let mut cx = Ctx {
        mode,
        imgs,
        used_ids: HashSet::new(),
        headings: Vec::new(),
        chapter: 0,
        splits: Vec::new(),
        note_nums: HashMap::new(),
    };
    let body = map_events(&mut cx, &main, text, true);
    // Definitioner utan referens hamnar sist, i sista kapitlet.
    let mut mapped: Vec<(String, Vec<Event<'a>>)> = Vec::new();
    for (label, evs) in &defs {
        mapped.push((label.clone(), map_events(&mut cx, evs, text, false)));
    }
    let last = cx.chapter;
    let mut notes: Vec<(usize, Note<'a>)> = Vec::new();
    for (label, events) in mapped {
        let next = cx.note_nums.len() + 1;
        let &mut (num, chapter) = cx.note_nums.entry(label).or_insert((next, last));
        notes.push((num, Note { chapter, events }));
    }
    notes.sort_by_key(|(n, _)| *n);
    Model { body, splits: cx.splits, headings: cx.headings, notes: notes.into_iter().map(|(_, n)| n).collect() }
}

fn map_events<'a>(
    cx: &mut Ctx,
    evs: &[(Event<'a>, Range<usize>)],
    src: &'a str,
    top: bool,
) -> Vec<Event<'a>> {
    let mut out: Vec<Event<'a>> = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < evs.len() {
        let (ev, range) = &evs[i];
        match ev {
            Event::Start(Tag::Table(aligns)) => {
                let end = (i + 1..evs.len()).find(|&j| matches!(evs[j].0, Event::End(TagEnd::Table))).unwrap_or(evs.len() - 1);
                let table_src = src.get(range.clone()).unwrap_or("");
                let html = render_table(cx, aligns, &evs[i + 1..end], table_src, src);
                out.push(Event::Html(CowStr::from(html)));
                i = end + 1;
                continue;
            }
            Event::Start(Tag::Image { dest_url, title, id, link_type }) => {
                let end = (i + 1..evs.len()).find(|&j| matches!(evs[j].0, Event::End(TagEnd::Image))).unwrap_or(evs.len() - 1);
                if let Some(new) = cx.imgs.src(dest_url) {
                    out.push(Event::Start(Tag::Image {
                        link_type: *link_type,
                        dest_url: CowStr::from(new),
                        title: title.clone(),
                        id: id.clone(),
                    }));
                    for (e, _) in &evs[i + 1..=end] {
                        out.push(e.clone());
                    }
                }
                i = end + 1;
                continue;
            }
            Event::Start(Tag::Heading { level, classes, attrs, .. }) => {
                let end = (i + 1..evs.len()).find(|&j| matches!(evs[j].0, Event::End(TagEnd::Heading(_)))).unwrap_or(evs.len() - 1);
                let mut text = String::new();
                for (e, _) in &evs[i + 1..end] {
                    if let Event::Text(t) | Event::Code(t) = e {
                        text.push_str(t);
                    }
                }
                let text = text.trim().to_owned();
                let id = unique_id(&mut cx.used_ids, &text);
                let n = heading_n(*level);
                if top && depth == 0 && n == 1 {
                    cx.chapter += 1;
                    cx.splits.push(out.len());
                }
                if top {
                    cx.headings.push(Heading { level: n, text, id: id.clone(), chapter: cx.chapter });
                }
                out.push(Event::Start(Tag::Heading {
                    level: *level,
                    id: Some(CowStr::from(id)),
                    classes: classes.clone(),
                    attrs: attrs.clone(),
                }));
            }
            Event::FootnoteReference(label) => {
                let next = cx.note_nums.len() + 1;
                let chapter = cx.chapter;
                let first = !cx.note_nums.contains_key(label.as_ref());
                let &mut (n, _) = cx.note_nums.entry(label.to_string()).or_insert((next, chapter));
                let id = if first { format!(" id=\"fnref-{n}\"") } else { String::new() };
                let epub = if cx.mode == Mode::Epub { " epub:type=\"noteref\"" } else { "" };
                out.push(Event::Html(CowStr::from(format!(
                    "<sup class=\"footnote-reference\"><a{id} href=\"@@FN{n}@@\"{epub}>{n}</a></sup>"
                ))));
            }
            Event::Html(t) => {
                let inner = t.trim().strip_prefix("<!--").and_then(|s| s.strip_suffix("-->")).map(str::trim);
                if inner == Some("kladd: toc") {
                    out.push(Event::Html(CowStr::from(format!("{TOC_MARK}\n"))));
                } else {
                    out.push(ev.clone());
                }
            }
            Event::Start(Tag::BlockQuote(_) | Tag::List(_) | Tag::Item) => {
                depth += 1;
                out.push(ev.clone());
            }
            Event::End(TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item) => {
                depth = depth.saturating_sub(1);
                out.push(ev.clone());
            }
            _ => out.push(ev.clone()),
        }
        i += 1;
    }
    out
}

fn heading_n(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// `id` för en rubrik: små bokstäver och siffror, bindestreck mellan orden, unikt i dokumentet.
fn unique_id(used: &mut HashSet<String>, text: &str) -> String {
    let mut s = String::new();
    let mut dash = false;
    for c in text.chars() {
        if c.is_alphanumeric() && (c.is_ascii() || c as u32 >= 0xC0) {
            s.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !s.is_empty() {
            s.push('-');
            dash = true;
        }
    }
    let mut s = s.trim_end_matches('-').to_owned();
    if s.is_empty() {
        s = "rubrik".into();
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert_str(0, "h-");
    }
    let mut id = s.clone();
    let mut n = 2;
    while used.contains(&id) {
        id = format!("{s}-{n}");
        n += 1;
    }
    used.insert(id.clone());
    id
}

// ---------------------------------------------------------------------------------------------
// Tabeller
// ---------------------------------------------------------------------------------------------

struct Cell {
    html: String,
    span: usize,
}

fn render_table<'a>(
    cx: &mut Ctx,
    aligns: &[Alignment],
    evs: &[(Event<'a>, Range<usize>)],
    table_src: &str,
    src: &'a str,
) -> String {
    // Rader: (är rubrikrad, celler).
    let mut rows: Vec<(bool, Vec<Cell>)> = Vec::new();
    let mut in_head = false;
    let mut i = 0;
    while i < evs.len() {
        match &evs[i].0 {
            Event::Start(Tag::TableHead) => {
                in_head = true;
                rows.push((true, Vec::new()));
            }
            Event::End(TagEnd::TableHead) => in_head = false,
            Event::Start(Tag::TableRow) => rows.push((false, Vec::new())),
            Event::Start(Tag::TableCell) => {
                let end = (i + 1..evs.len()).find(|&j| matches!(evs[j].0, Event::End(TagEnd::TableCell))).unwrap_or(evs.len() - 1);
                let mapped = map_events(cx, &evs[i + 1..end], src, false);
                let is_span = matches!(mapped.as_slice(), [Event::Text(t)] if t.as_ref() == SPAN);
                if let Some((_, cells)) = rows.last_mut() {
                    if is_span {
                        match cells.last_mut() {
                            Some(last) => last.span += 1,
                            None => cells.push(Cell { html: String::new(), span: 1 }),
                        }
                    } else {
                        let mut html = String::new();
                        push_html(&mut html, mapped.into_iter());
                        cells.push(Cell { html, span: 1 });
                    }
                }
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    let _ = in_head;

    let widths = col_widths(table_src, aligns.len());
    let mut s = String::new();
    s.push_str(if widths.is_some() { "<table style=\"table-layout:fixed;width:100%\">\n" } else { "<table>\n" });
    if let Some(w) = widths {
        s.push_str("<colgroup>");
        for p in w {
            s.push_str(&format!("<col style=\"width:{p}%\" />"));
        }
        s.push_str("</colgroup>\n");
    }
    let mut section: Option<&str> = None;
    for (head, cells) in &rows {
        let want = if *head { "thead" } else { "tbody" };
        if section != Some(want) {
            if let Some(old) = section {
                s.push_str(&format!("</{old}>\n"));
            }
            s.push_str(&format!("<{want}>\n"));
            section = Some(want);
        }
        s.push_str("<tr>");
        let mut col = 0;
        for c in cells {
            let tag = if *head { "th" } else { "td" };
            let mut attrs = String::new();
            if c.span > 1 {
                attrs.push_str(&format!(" colspan=\"{}\"", c.span));
            }
            match aligns.get(col) {
                Some(Alignment::Left) => attrs.push_str(" style=\"text-align:left\""),
                Some(Alignment::Center) => attrs.push_str(" style=\"text-align:center\""),
                Some(Alignment::Right) => attrs.push_str(" style=\"text-align:right\""),
                _ => {}
            }
            s.push_str(&format!("<{tag}{attrs}>{}</{tag}>", c.html));
            col += c.span;
        }
        s.push_str("</tr>\n");
    }
    if let Some(old) = section {
        s.push_str(&format!("</{old}>\n"));
    }
    s.push_str("</table>\n");
    s
}

/// Kolumnbredder i procent från bindestrecken i avgränsarraden. `None` = automatisk bredd.
fn col_widths(table_src: &str, cols: usize) -> Option<Vec<String>> {
    let line = table_src.lines().nth(1)?;
    let mut parts = split_row(line.trim());
    if parts.first().is_some_and(|p| p.trim().is_empty()) {
        parts.remove(0);
    }
    if parts.last().is_some_and(|p| p.trim().is_empty()) {
        parts.pop();
    }
    if parts.len() != cols || cols == 0 {
        return None;
    }
    let dashes: Vec<usize> = parts.iter().map(|p| p.chars().filter(|&c| c == '-').count()).collect();
    if dashes.iter().any(|&d| d == 0) || dashes.iter().all(|&d| d == dashes[0]) {
        return None;
    }
    let total: usize = dashes.iter().sum();
    Some(dashes.iter().map(|&d| format!("{:.1}", d as f64 * 100.0 / total as f64)).collect())
}

// ---------------------------------------------------------------------------------------------
// Utskrift av kapitel
// ---------------------------------------------------------------------------------------------

struct Layout {
    mode: Mode,
    preface: bool,
}

impl Layout {
    /// Filnamn för kapitel `c` (tomt i HTML, där allt ligger i samma fil).
    fn file(&self, c: usize) -> String {
        if self.mode == Mode::Html {
            return String::new();
        }
        let n = if self.preface { c + 1 } else { c.max(1) };
        format!("chapter-{n}.xhtml")
    }
}

fn has_content(evs: &[Event]) -> bool {
    evs.iter().any(|e| match e {
        Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock) => false,
        Event::Html(t) => t.contains(TOC_MARK) || !strip_comments(t).trim().is_empty(),
        Event::Text(t) => !t.trim().is_empty(),
        _ => true,
    })
}

fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Skriver ut `range` av texten (och de fotnoter som hör till kapitlet) som normaliserad HTML.
fn render_chunk(
    m: &Model,
    lay: &Layout,
    range: Range<usize>,
    chapter: Option<usize>,
    fixer: &mut Fixer,
    imgs: &mut Images,
) -> String {
    let mut html = String::new();
    push_html(&mut html, m.body[range].iter().cloned());

    // Fotnoter: i HTML alla sist, i EPUB de som först nämns i kapitlet.
    let mut foot = String::new();
    for (idx, note) in m.notes.iter().enumerate() {
        if chapter.is_some_and(|c| c != note.chapter) {
            continue;
        }
        let n = idx + 1;
        let epub = if lay.mode == Mode::Epub { " epub:type=\"footnote\"" } else { "" };
        let mut evs: Vec<Event> = vec![Event::Html(CowStr::from(format!("<aside class=\"footnote\" id=\"fn-{n}\"{epub}>\n")))];
        let back = Event::Html(CowStr::from(format!("<a class=\"footnote-backref\" href=\"#fnref-{n}\">{n}.</a> ")));
        let mut placed = false;
        for e in &note.events {
            evs.push(e.clone());
            if !placed && matches!(e, Event::Start(Tag::Paragraph)) {
                evs.push(back.clone());
                placed = true;
            }
        }
        if !placed {
            evs.insert(1, back);
        }
        evs.push(Event::Html(CowStr::from("</aside>\n")));
        push_html(&mut foot, evs.into_iter());
    }
    if !foot.is_empty() {
        let epub = if lay.mode == Mode::Epub { " epub:type=\"footnotes\"" } else { "" };
        html.push_str(&format!("<section class=\"footnotes\"{epub}>\n{foot}</section>\n"));
    }

    if html.contains(TOC_MARK) {
        html = html.replace(TOC_MARK, &toc_html(m, lay));
    }
    for n in 1..=m.notes.len() {
        let key = format!("@@FN{n}@@");
        if html.contains(&key) {
            html = html.replace(&key, &format!("{}#fn-{n}", lay.file(m.notes[n - 1].chapter)));
        }
    }
    // Ett stycke som bara innehöll en saknad bild.
    let html = html.replace("<p></p>\n", "");
    fixer.process(&html, imgs)
}

fn toc_html(m: &Model, lay: &Layout) -> String {
    let items: Vec<(u8, String, String)> = m
        .headings
        .iter()
        .filter(|h| !h.text.is_empty())
        .map(|h| (h.level, esc(&h.text), format!("{}#{}", lay.file(h.chapter), h.id)))
        .collect();
    if items.is_empty() {
        return String::new();
    }
    format!("<nav class=\"toc\">{}</nav>\n", nested_list(&items, "ul"))
}

/// Nästlad lista av (nivå, text, href).
fn nested_list(items: &[(u8, String, String)], tag: &str) -> String {
    let mut out = String::new();
    let mut stack: Vec<u8> = Vec::new();
    for (level, text, href) in items {
        match stack.last() {
            None => {
                out.push_str(&format!("<{tag}>"));
                stack.push(*level);
            }
            Some(&top) if *level > top => {
                out.push_str(&format!("<{tag}>"));
                stack.push(*level);
            }
            Some(_) => {
                out.push_str("</li>");
                while stack.len() > 1 && *level < *stack.last().unwrap() {
                    out.push_str(&format!("</{tag}></li>"));
                    stack.pop();
                }
            }
        }
        out.push_str(&format!("<li><a href=\"{href}\">{text}</a>"));
    }
    if !stack.is_empty() {
        out.push_str("</li>");
        while stack.len() > 1 {
            out.push_str(&format!("</{tag}></li>"));
            stack.pop();
        }
        out.push_str(&format!("</{tag}>"));
    }
    out
}

fn doc_title(m: &Model, fallback: &str) -> String {
    m.headings
        .iter()
        .find(|h| !h.text.is_empty())
        .map(|h| h.text.clone())
        .or_else(|| Some(fallback.trim().to_owned()).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| "Dokument".into())
}

// ---------------------------------------------------------------------------------------------
// Normalisering av HTML till välformad (X)HTML
// ---------------------------------------------------------------------------------------------

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr",
];

struct Fixer {
    mode: Mode,
    /// Öppna element: (namn, öppningstagg som skrevs).
    stack: Vec<(String, String)>,
}

impl Fixer {
    fn new(mode: Mode) -> Self {
        Self { mode, stack: Vec::new() }
    }

    /// Slutar alla öppna element (stacken behålls, så att `reopen` kan öppna dem igen).
    fn close_all(&mut self) -> String {
        self.stack.iter().rev().map(|(n, _)| format!("</{n}>")).collect()
    }

    fn reopen(&self) -> String {
        self.stack.iter().map(|(_, open)| open.as_str()).collect()
    }

    fn process(&mut self, html: &str, imgs: &mut Images) -> String {
        let mut out = String::with_capacity(html.len());
        let mut rest = html;
        while let Some(lt) = rest.find('<') {
            out.push_str(&clean_text(&rest[..lt]));
            rest = &rest[lt..];
            if rest.starts_with("<!--") {
                rest = rest.find("-->").map_or("", |j| &rest[j + 3..]);
            } else if rest.starts_with("<!") || rest.starts_with("<?") {
                rest = rest.find('>').map_or("", |j| &rest[j + 1..]);
            } else if let Some(after) = rest.strip_prefix("</") {
                match after.find('>') {
                    Some(j) => {
                        let name = after[..j].trim().to_ascii_lowercase();
                        self.close(&name, &mut out);
                        rest = &after[j + 1..];
                    }
                    None => {
                        out.push_str("&lt;");
                        rest = &rest[1..];
                    }
                }
            } else if rest[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
                match parse_tag(rest) {
                    Some((tag, used)) => {
                        self.open(tag, &mut out, imgs);
                        rest = &rest[used..];
                    }
                    None => {
                        out.push_str("&lt;");
                        rest = &rest[1..];
                    }
                }
            } else {
                out.push_str("&lt;");
                rest = &rest[1..];
            }
        }
        out.push_str(&clean_text(rest));
        out
    }

    fn close(&mut self, name: &str, out: &mut String) {
        if VOID.contains(&name) || matches!(name, "html" | "head" | "body") {
            return;
        }
        let Some(pos) = self.stack.iter().rposition(|(n, _)| n == name) else { return };
        while self.stack.len() > pos {
            let (n, _) = self.stack.pop().unwrap();
            out.push_str(&format!("</{n}>"));
        }
    }

    fn open(&mut self, mut tag: RawTag, out: &mut String, imgs: &mut Images) {
        if matches!(tag.name.as_str(), "html" | "head" | "body") {
            return;
        }
        let epub = self.mode == Mode::Epub;
        match tag.name.as_str() {
            "img" => {
                let Some(src) = tag.get("src").map(str::to_owned).and_then(|s| imgs.src(&s)) else { return };
                tag.set("src", src);
                if tag.get("alt").is_none() {
                    tag.set("alt", String::new());
                }
                if let Some(a) = tag.get("align").map(|a| a.to_ascii_lowercase()) {
                    if a == "left" || a == "right" {
                        let side = if a == "left" { "0 1em .5em 0" } else { "0 0 .5em 1em" };
                        tag.add_style(&format!("float:{a};margin:{side}"));
                    }
                    if epub {
                        tag.remove("align");
                    }
                }
            }
            "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if let Some(a) = tag.get("align").map(|a| a.to_ascii_lowercase()) {
                    let a = if a == "middle" { "center".to_owned() } else { a };
                    if matches!(a.as_str(), "left" | "right" | "center" | "justify") {
                        tag.add_style(&format!("text-align:{a}"));
                    }
                    if epub {
                        tag.remove("align");
                    }
                }
            }
            _ => {}
        }
        let void = VOID.contains(&tag.name.as_str());
        let text = tag.render(void || tag.self_closing);
        out.push_str(&text);
        if tag.self_closing && !void {
            out.push_str(&format!("</{}>", tag.name));
        } else if !void {
            self.stack.push((tag.name.clone(), text));
        }
    }
}

struct RawTag {
    name: String,
    attrs: Vec<(String, String)>,
    self_closing: bool,
}

impl RawTag {
    fn get(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    fn set(&mut self, k: &str, v: String) {
        match self.attrs.iter_mut().find(|(n, _)| n == k) {
            Some(a) => a.1 = v,
            None => self.attrs.push((k.to_owned(), v)),
        }
    }

    fn remove(&mut self, k: &str) {
        self.attrs.retain(|(n, _)| n != k);
    }

    fn add_style(&mut self, css: &str) {
        let merged = match self.get("style").map(|s| s.trim().trim_end_matches(';').to_owned()) {
            Some(s) if !s.is_empty() => format!("{s}; {css}"),
            _ => css.to_owned(),
        };
        self.set("style", merged);
    }

    fn render(&self, void: bool) -> String {
        let mut s = format!("<{}", self.name);
        for (k, v) in &self.attrs {
            s.push_str(&format!(" {k}=\"{}\"", esc_attr(v)));
        }
        s.push_str(if void { " />" } else { ">" });
        s
    }
}

/// Läser en starttagg i början av `s` (som börjar med `<` och en bokstav). Ger taggen och antal
/// byte som den tog, eller `None` om `>` saknas.
fn parse_tag(s: &str) -> Option<(RawTag, usize)> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b':') {
        i += 1;
    }
    let name = s[1..i].to_ascii_lowercase();
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut self_closing = false;
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        match b.get(i)? {
            b'>' => {
                i += 1;
                break;
            }
            b'/' => {
                i += 1;
                if b.get(i) == Some(&b'>') {
                    self_closing = true;
                    i += 1;
                    break;
                }
            }
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
                    i += 1;
                }
                if i == start {
                    i += 1;
                    continue;
                }
                let key = s[start..i].to_ascii_lowercase();
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let mut val = key.clone();
                if b.get(i) == Some(&b'=') {
                    i += 1;
                    while i < b.len() && b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    match b.get(i)? {
                        q @ (b'"' | b'\'') => {
                            let from = i + 1;
                            let len = s[from..].find(*q as char)?;
                            val = decode_entities(&s[from..from + len]);
                            i = from + len + 1;
                        }
                        _ => {
                            let from = i;
                            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                                i += 1;
                            }
                            val = decode_entities(&s[from..i]);
                        }
                    }
                }
                let valid = key.starts_with(|c: char| c.is_ascii_alphabetic())
                    && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'));
                if valid && !attrs.iter().any(|(k, _)| *k == key) {
                    attrs.push((key, val));
                }
            }
        }
    }
    Some((RawTag { name, attrs, self_closing }, i))
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "bull" => '•',
        "middot" => '·',
        "times" => '×',
        "deg" => '°',
        "euro" => '€',
        "sect" => '§',
        "para" => '¶',
        _ => return None,
    })
}

/// Entitet i början av `s` (som börjar med `&`): tecknet och längden inklusive `;`.
fn entity_at(s: &str) -> Option<(char, usize)> {
    let end = s.find(';').filter(|&e| e <= 12)?;
    let body = &s[1..end];
    let c = if let Some(num) = body.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => num.parse().ok()?,
        };
        char::from_u32(code).filter(|c| *c != '\0')?
    } else {
        named_entity(body)?
    };
    Some((c, end + 1))
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match entity_at(rest) {
            Some((c, n)) => {
                out.push(c);
                rest = &rest[n..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text mellan taggar: entiteter blir tecken eller numeriska entiteter, lösa `&` blir `&amp;`.
fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match entity_at(rest) {
            Some((c, n)) => {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    c => out.push(c),
                }
                rest = &rest[n..];
            }
            None => {
                out.push_str("&amp;");
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn esc_attr(s: &str) -> String {
    esc(s).replace('"', "&quot;")
}

// ---------------------------------------------------------------------------------------------
// Formatmall
// ---------------------------------------------------------------------------------------------

/// Typsnittslista med reservtypsnitt som finns på vanliga system.
fn font_stack(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !matches!(c, '"' | '\'' | '\\' | '<' | '>' | ';' | '{' | '}')).collect();
    let l = clean.to_lowercase();
    let q = format!("\"{clean}\"");
    let has = |words: &[&str]| words.iter().any(|w| l.contains(w));
    if l == "calibri" || l == "carlito" {
        "Calibri, Carlito, \"Segoe UI\", \"Helvetica Neue\", Arial, sans-serif".into()
    } else if has(&["courier", "consolas", "mono", "menlo", "monaco", "lucida console"]) {
        format!("{q}, \"Liberation Mono\", \"DejaVu Sans Mono\", monospace")
    } else if has(&["times", "georgia", "cambria", "garamond", "palatino", "book antiqua", "serif", "minion", "baskerville"])
        && !has(&["sans"])
    {
        format!("{q}, \"Liberation Serif\", \"Times New Roman\", Times, serif")
    } else {
        format!("{q}, Carlito, Arial, \"Liberation Sans\", Helvetica, sans-serif")
    }
}

fn num(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Gemensam formatmall: rubriker, citat, kod och tabeller som i Kladds dokumentvy och Word.
fn css(d: &DocDefaults, mode: Mode) -> String {
    let (pw, ph) = d.page();
    let [mt, mr, mb, ml] = d.margins();
    let text_w = (pw - ml - mr).max(200.0);
    let mut s = String::new();
    if mode == Mode::Html {
        s.push_str(&format!(
            "@page {{ size: {}pt {}pt; margin: {}pt {}pt {}pt {}pt; }}\n",
            num(pw),
            num(ph),
            num(mt),
            num(mr),
            num(mb),
            num(ml)
        ));
        s.push_str(&format!(
            "body {{ font-family: {}; font-size: {}pt; line-height: 1.15; color: #1f1f1f; background: #fff; \
             max-width: {}pt; margin: 0 auto; padding: 2rem 1rem; overflow-wrap: break-word; }}\n",
            font_stack(d.font()),
            num(d.size()),
            num(text_w)
        ));
        s.push_str(
            "@media print { body { max-width: none; padding: 0; } a { color: inherit; text-decoration: none; } \
             h1, h2, h3, h4 { break-after: avoid; } table, img, pre, blockquote { break-inside: avoid; } }\n",
        );
        s.push_str("h1 { font-size: 18pt; color: #1F3864; } h2 { font-size: 15pt; } h3 { font-size: 13pt; } h4 { font-size: 12pt; } h5, h6 { font-size: 11pt; }\n");
    } else {
        s.push_str(&format!(
            "body {{ font-family: {}; line-height: 1.3; margin: 0; padding: 0 .5em; }}\n",
            font_stack(d.font())
        ));
        s.push_str("h1 { font-size: 1.6em; color: #1F3864; } h2 { font-size: 1.35em; } h3 { font-size: 1.2em; } h4 { font-size: 1.1em; } h5, h6 { font-size: 1em; }\n");
    }
    s.push_str(
        "h1, h2, h3, h4, h5, h6 { font-weight: bold; color: #2F5496; margin: 1.3em 0 .35em; line-height: 1.2; }\n\
         h1 { color: #1F3864; } h6 { color: #404040; }\n\
         p { margin: 0 0 .7em; }\n\
         a { color: #0563C1; }\n\
         blockquote { margin: .7em 0; padding: 0 0 0 1em; border-left: 3px solid #a0a0a0; color: #404040; font-style: italic; }\n\
         code, pre { font-family: Consolas, \"Liberation Mono\", \"DejaVu Sans Mono\", monospace; font-size: 10pt; }\n\
         pre { background: #f2f2f2; padding: .6em .8em; overflow: auto; margin: .7em 0; }\n\
         pre code { background: none; padding: 0; }\n\
         code { background: #f2f2f2; padding: 0 .2em; }\n\
         table { border-collapse: collapse; margin: .7em 0; }\n\
         th, td { border: 1px solid #7f7f7f; padding: .2em .5em; vertical-align: top; text-align: left; }\n\
         th { font-weight: bold; background: #f2f2f2; }\n\
         img { max-width: 100%; height: auto; }\n\
         hr { border: 0; border-top: 1px solid #a0a0a0; margin: 1.2em 0; }\n\
         del { color: #606060; }\n\
         sup.footnote-reference a { text-decoration: none; }\n\
         section.footnotes { border-top: 1px solid #a0a0a0; margin-top: 2em; padding-top: .5em; font-size: .9em; }\n\
         aside.footnote { margin: 0 0 .4em; }\n\
         aside.footnote > p:first-of-type { display: inline; }\n\
         nav.toc ul { list-style: none; padding-left: 1.2em; margin: .2em 0; }\n\
         nav.toc > ul { padding-left: 0; }\n",
    );
    s
}

// ---------------------------------------------------------------------------------------------
// Övrigt
// ---------------------------------------------------------------------------------------------

/// Slumpmässigt UUID (version 4) utan extra beroenden.
fn uuid_v4(seed: &str) -> String {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let mut bytes = [0u8; 16];
    for half in 0..2 {
        let mut h = RandomState::new().build_hasher();
        h.write(seed.as_bytes());
        h.write_u8(half);
        bytes[half as usize * 8..half as usize * 8 + 8].copy_from_slice(&h.finish().to_be_bytes());
    }
    bytes[6] = bytes[6] & 0x0F | 0x40;
    bytes[8] = bytes[8] & 0x3F | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

#[cfg(test)]
mod tests {
    /// Skriver HTML för `KLADD_MD_IN` till `KLADD_HTML_OUT` (bilder från mappen bredvid): för att
    /// titta på resultatet i en webbläsare.
    #[test]
    #[ignore]
    fn html_manual() {
        let (Ok(i), Ok(o)) = (std::env::var("KLADD_MD_IN"), std::env::var("KLADD_HTML_OUT")) else { return };
        let md = std::fs::read_to_string(&i).unwrap();
        let dir = std::path::Path::new(&i).parent().unwrap().to_path_buf();
        let w = html(&md, "Prov", &|src| std::fs::read(dir.join(src)).ok().map(|bytes| Image { bytes, extent: None })).unwrap();
        std::fs::write(o, w.bytes).unwrap();
    }

    use super::*;
    use std::io::Read;

    // 1x1 PNG.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
        0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn resolve(src: &str) -> Option<Image> {
        (src == "bild:a.png" || src == "lokal.png").then(|| Image { bytes: PNG.to_vec(), extent: None })
    }

    const RICH: &str = "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt; page-size: A4 -->\n\
<!-- kladd: toc -->\n\n\
# Första kapitlet\n\n\
Text med fotnot[^1] och **fet** och <u>understruken</u>.<!--k:3-->\n\n\
<div align=\"center\">\n\nCentrerat stycke\n\n</div>\n\n\
<img src=\"bild:a.png\" alt=\"Flytande\" width=\"100\" align=\"left\">\n\n\
![Vanlig](bild:a.png)\n\n\
![Saknad](bild:finns-inte.png)\n\n\
## Tabellen\n\n\
| Rubrik över två || Tredje |\n| ------ | -- | ---- |\n| a | b<br>rad | c |\n| x ||  z |\n\n\
# Andra kapitlet\n\n\
Slut & tecken < 5. Se [länk](https://exempel.se?a=1&b=2).\n\n\
### Under\n\n\
[^1]: Fotnotens text.\n";

    fn tag_balance(html: &str) {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = html;
        while let Some(i) = rest.find('<') {
            rest = &rest[i..];
            if rest.starts_with("<!") {
                rest = &rest[rest.find('>').unwrap() + 1..];
                continue;
            }
            let end = rest.find('>').expect("taggen slutar");
            let tag = &rest[1..end];
            rest = &rest[end + 1..];
            if let Some(name) = tag.strip_prefix('/') {
                assert_eq!(stack.pop().as_deref(), Some(name.trim()), "obalanserad tagg i {html}");
            } else if !tag.ends_with('/') {
                let name = tag.split_whitespace().next().unwrap().to_owned();
                if !VOID.contains(&name.as_str()) {
                    stack.push(name);
                }
            }
        }
        assert!(stack.is_empty(), "oslutna taggar: {stack:?}");
    }

    #[test]
    fn html_rich_document() {
        let w = html(RICH, "rapport", &resolve).unwrap();
        let s = String::from_utf8(w.bytes).unwrap();
        assert!(s.starts_with("<!doctype html>\n<html lang=\"sv\">"));
        assert!(s.contains("<title>Första kapitlet</title>"));
        assert!(!s.contains("<!-- kladd") && !s.contains("<!--k:") && !s.contains("<!--kladd"), "{s}");
        assert!(s.contains("src=\"data:image/png;base64,iVBOR"), "{s}");
        assert!(!s.contains("bild:"));
        assert_eq!(w.missing_images, vec!["bild:finns-inte.png".to_owned()]);
        assert!(s.contains("colspan=\"2\""), "{s}");
        assert!(s.contains("<col style=\"width:50.0%\" />") && s.contains("<col style=\"width:16.7%\" />"), "{s}");
        assert!(s.contains("b<br />rad"), "{s}");
        assert!(s.contains("<a href=\"#första-kapitlet\">") || s.contains("href=\"#f"), "{s}");
        assert!(s.contains("<h1 id=\"första-kapitlet\">") && s.contains("<h2 id=\"tabellen\">"), "{s}");
        assert!(s.contains("<a href=\"#tabellen\">Tabellen</a>"));
        assert!(s.contains("style=\"text-align:center\"") && s.contains("align=\"center\""), "{s}");
        assert!(s.contains("float:left"), "{s}");
        assert!(s.contains("<sup class=\"footnote-reference\"><a id=\"fnref-1\" href=\"#fn-1\">1</a></sup>"), "{s}");
        assert!(s.contains("<aside class=\"footnote\" id=\"fn-1\">") && s.contains("Fotnotens text."), "{s}");
        assert!(s.contains("Slut &amp; tecken &lt; 5"), "{s}");
        // Grundformat och sida.
        assert!(s.contains("font-size: 12pt") && s.contains("\"Times New Roman\", \"Liberation Serif\""), "{s}");
        assert!(s.contains("@page { size: 595.28pt 841.89pt; margin: 72pt 72pt 72pt 72pt; }"), "{s}");
        assert!(s.contains("max-width: 451.28pt"), "{s}");
        let body = &s[s.find("<body>").unwrap()..];
        tag_balance(&body[6..body.find("</body>").unwrap()]);
    }

    #[test]
    fn html_title_falls_back_to_name() {
        let w = html("Bara text.\n", "mitt dokument", &resolve).unwrap();
        let s = String::from_utf8(w.bytes).unwrap();
        assert!(s.contains("<title>mitt dokument</title>"));
    }

    #[test]
    fn heading_ids_unique() {
        let w = html("# Hej\n\n## Hej\n\n### 1. Siffra\n", "x", &resolve).unwrap();
        let s = String::from_utf8(w.bytes).unwrap();
        assert!(s.contains("id=\"hej\"") && s.contains("id=\"hej-2\"") && s.contains("id=\"h-1-siffra\""), "{s}");
    }

    #[test]
    fn colspan_not_in_code_and_escaped_pipe_kept() {
        let md = "```\n| a || b |\n```\n\n| a | b \\| c |\n|---|---|\n| 1 | 2 |\n";
        let s = String::from_utf8(html(md, "x", &resolve).unwrap().bytes).unwrap();
        assert!(s.contains("| a || b |"), "{s}");
        assert!(!s.contains("colspan"), "{s}");
        assert!(s.contains("b | c"), "{s}");
    }

    fn unzip(bytes: &[u8]) -> (Vec<String>, HashMap<String, Vec<u8>>) {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut order = Vec::new();
        let mut map = HashMap::new();
        for i in 0..z.len() {
            let mut f = z.by_index(i).unwrap();
            if i == 0 {
                assert_eq!(f.compression(), zip::CompressionMethod::Stored, "mimetype ska vara okomprimerad");
            }
            let mut b = Vec::new();
            f.read_to_end(&mut b).unwrap();
            order.push(f.name().to_owned());
            map.insert(f.name().to_owned(), b);
        }
        (order, map)
    }

    fn well_formed(name: &str, xml: &[u8]) {
        let mut r = quick_xml::Reader::from_reader(xml);
        let mut stack: Vec<String> = Vec::new();
        loop {
            match r.read_event().unwrap_or_else(|e| panic!("{name}: {e}")) {
                quick_xml::events::Event::Start(e) => stack.push(String::from_utf8_lossy(e.name().as_ref()).into_owned()),
                quick_xml::events::Event::End(e) => {
                    assert_eq!(stack.pop().as_deref(), Some(String::from_utf8_lossy(e.name().as_ref()).as_ref()), "{name}")
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        assert!(stack.is_empty(), "{name}");
    }

    #[test]
    fn epub_structure() {
        let w = epub(RICH, "rapport", &resolve).unwrap();
        let (order, files) = unzip(&w.bytes);
        assert_eq!(order[0], "mimetype");
        assert_eq!(files["mimetype"], b"application/epub+zip");
        let container = String::from_utf8_lossy(&files["META-INF/container.xml"]).into_owned();
        assert!(container.contains("full-path=\"OEBPS/content.opf\""));
        // Förordet (TOC-raden) kommer före första H1, så kapitlen är tre.
        let opf = String::from_utf8_lossy(&files["OEBPS/content.opf"]).into_owned();
        assert!(opf.contains("<dc:language>sv</dc:language>") && opf.contains("urn:uuid:") && opf.contains("dcterms:modified"));
        // Varje fil utom mimetype, container och opf listas i manifestet.
        for name in order.iter().filter(|n| n.starts_with("OEBPS/") && *n != "OEBPS/content.opf") {
            let href = name.trim_start_matches("OEBPS/");
            assert!(opf.contains(&format!("href=\"{href}\"")), "{href} saknas i manifestet");
        }
        assert!(opf.contains("media-type=\"image/png\"") && opf.contains("properties=\"nav\""));
        let spine: Vec<&str> = opf.split("<itemref idref=\"").skip(1).map(|s| s.split('"').next().unwrap()).collect();
        assert_eq!(spine, vec!["chapter-1", "chapter-2", "chapter-3"]);
        for (n, bytes) in &files {
            if n.ends_with(".xhtml") || n.ends_with(".opf") || n.ends_with(".xml") {
                well_formed(n, bytes);
            }
        }
        let nav = String::from_utf8_lossy(&files["OEBPS/nav.xhtml"]).into_owned();
        assert!(nav.contains("epub:type=\"toc\""));
        assert!(nav.contains("href=\"chapter-2.xhtml#första-kapitlet\"") && nav.contains("href=\"chapter-2.xhtml#tabellen\""), "{nav}");
        assert!(nav.contains("href=\"chapter-3.xhtml#andra-kapitlet\""), "{nav}");
        let c2 = String::from_utf8_lossy(&files["OEBPS/chapter-2.xhtml"]).into_owned();
        assert!(c2.contains("src=\"images/img1.png\"") && !c2.contains("data:") && !c2.contains("align=\""), "{c2}");
        assert!(c2.contains("epub:type=\"noteref\"") && c2.contains("epub:type=\"footnote\""), "{c2}");
        assert!(c2.contains("colspan=\"2\""), "{c2}");
        assert_eq!(files["OEBPS/images/img1.png"], PNG);
        let c3 = String::from_utf8_lossy(&files["OEBPS/chapter-3.xhtml"]).into_owned();
        assert!(c3.contains("&amp;b=2") && c3.contains("&lt; 5"), "{c3}");
        assert_eq!(w.missing_images.len(), 1, "{:?}", w.missing_images);
    }

    #[test]
    fn epub_without_h1_is_one_chapter() {
        let w = epub("## Bara en underrubrik\n\nText.\n", "x", &resolve).unwrap();
        let (order, files) = unzip(&w.bytes);
        assert!(order.contains(&"OEBPS/chapter-1.xhtml".to_owned()));
        assert!(!order.contains(&"OEBPS/chapter-2.xhtml".to_owned()));
        let nav = String::from_utf8_lossy(&files["OEBPS/nav.xhtml"]).into_owned();
        assert!(nav.contains("chapter-1.xhtml#bara-en-underrubrik"));
    }

    #[test]
    fn epub_open_div_carries_over_chapters() {
        let md = "<div align=\"center\">\n\n# Ett\n\ntext\n\n# Två\n\ntext\n\n</div>\n";
        let w = epub(md, "x", &resolve).unwrap();
        let (_, files) = unzip(&w.bytes);
        for n in files.keys().filter(|n| n.ends_with(".xhtml")) {
            well_formed(n, &files[n]);
        }
    }

    #[test]
    fn fixer_repairs_html() {
        let mut imgs = Images::new(Mode::Epub, &resolve);
        let mut f = Fixer::new(Mode::Epub);
        let out = f.process("<p>a<br>b &nbsp;&copy; &foo; <u>x</p><input disabled></i>", &mut imgs);
        assert_eq!(out, "<p>a<br />b \u{a0}© &amp;foo; <u>x</u></p><input disabled=\"disabled\" />");
        assert!(f.stack.is_empty());
    }

    #[test]
    fn save_to_dispatches_by_extension() {
        let dir = std::env::temp_dir().join("kladd-test-export");
        std::fs::create_dir_all(&dir).unwrap();
        let mut d = crate::doc::Document::new_untitled(1, 1);
        d.text = "# Hej\n\nEn **fet** rad.\n".into();
        let epub_path = dir.join("bok.epub");
        d.save_to(&epub_path).unwrap();
        assert!(std::fs::read(&epub_path).unwrap().starts_with(b"PK"));
        // Export: fliken behåller sitt (namnlösa) tillstånd.
        assert!(d.path.is_none());
        let mut md = crate::doc::Document::new_untitled(2, 1);
        md.text = d.text.clone();
        md.path = Some(dir.join("a.md"));
        let html_path = dir.join("a.html");
        md.save_to(&html_path).unwrap();
        let s = std::fs::read_to_string(&html_path).unwrap();
        assert!(s.starts_with("<!doctype html>") && s.contains("<strong>fet</strong>"));
        assert_eq!(md.path.as_deref(), Some(dir.join("a.md").as_path()));
        // Vanlig text sparas som den är, även med .html.
        let mut txt = crate::doc::Document::new_untitled(3, 1);
        txt.text = "<p>hej</p>".into();
        txt.save_to(&dir.join("b.html")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("b.html")).unwrap(), "<p>hej</p>");
    }

    /// Kör `epubcheck` om det är installerat: `cargo test -- --ignored epubcheck`.
    #[test]
    #[ignore]
    fn epubcheck() {
        let w = epub(RICH, "rapport", &resolve).unwrap();
        let p = std::env::temp_dir().join("kladd-epubcheck.epub");
        std::fs::write(&p, &w.bytes).unwrap();
        // Debians `epubcheck` är en jar-fil som saknar startskript på vissa system.
        let out = std::process::Command::new("epubcheck")
            .arg(&p)
            .output()
            .or_else(|_| std::process::Command::new("java").args(["-jar", "/usr/bin/epubcheck"]).arg(&p).output())
            .expect("epubcheck saknas");
        let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        println!("{log}");
        assert!(out.status.success(), "{log}");
    }
}
