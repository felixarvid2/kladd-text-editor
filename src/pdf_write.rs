//! Sparar Markdown som PDF.
//!
//! Upplägget följer WordCraft (github.com/storytold/wordcraft, MIT/Apache-2.0), som
//! också skriver PDF med `krilla`: typsnitten bäddas in som delmängder, texten går att
//! markera och söka i, rubriker blir bokmärken och länkar blir klickbara.
//!
//! Kladd har en egen enkel sidlayout: sidstorlek och marginaler från grundformatraden,
//! IBM Plex Sans för brödtext och rubriker,
//! IBM Plex Mono för kod, radbrytning mellan ord (mätt med rustybuzz, samma formning
//! som krilla använder när texten ritas), sidhuvud och sidfot med sidnummer.

use std::collections::HashMap;

use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, LineCap, LineJoin, Stroke};
use krilla::text::{Font, TextDirection};
use krilla::Document;
use krilla::image::Image;
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::docx::Image as DocImage;
use crate::style::{self, Align, DocDefaults, SpanStyle};
use crate::theme;
use crate::word::markup::Float;
use crate::word::sections::{self, Section, SectionProps};

// --- mått i punkter (1/72 tum); sidstorlek och marginaler kommer från dokumentet ---
const BODY_SIZE: f32 = 10.5;
const LINE: f32 = 1.5;
const PARA_GAP: f32 = 7.5;
const INDENT: f32 = 16.0;
/// Radhöjd för enkelt radavstånd i formatmallarnas `line-spacing`, gånger teckenstorleken.
const LINE_SINGLE: f32 = 1.3;

// --- färger --------------------------------------------------------------------
const INK: (u8, u8, u8) = (0x1E, 0x28, 0x36);
const MUTED: (u8, u8, u8) = (0x66, 0x74, 0x8A);
const RULE: (u8, u8, u8) = (0xD0, 0xD8, 0xE3);
const LINK: (u8, u8, u8) = (0x1F, 0x63, 0xA8);
const CODE_BG: (u8, u8, u8) = (0xF1, 0xF4, 0xF8);
const ACCENT: (u8, u8, u8) = (0xB8, 0x65, 0x0F);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Face {
    Sans,
    SansMedium,
    SansSemi,
    Mono,
    MonoMedium,
}

impl Face {
    fn data(self) -> &'static [u8] {
        match self {
            Face::Sans => theme::PLEX_SANS,
            Face::SansMedium => theme::PLEX_SANS_MEDIUM,
            Face::SansSemi => theme::PLEX_SANS_SEMIBOLD,
            Face::Mono => theme::PLEX_MONO,
            Face::MonoMedium => theme::PLEX_MONO_MEDIUM,
        }
    }
}

/// Mäter textbredd med samma formning (rustybuzz) som krilla använder.
struct Measurer {
    faces: HashMap<Face, rustybuzz::Face<'static>>,
    cache: HashMap<(Face, String), f32>,
}

impl Measurer {
    fn new() -> Self {
        let mut faces = HashMap::new();
        for f in [Face::Sans, Face::SansMedium, Face::SansSemi, Face::Mono, Face::MonoMedium] {
            faces.insert(f, rustybuzz::Face::from_slice(f.data(), 0).expect("inbyggt typsnitt"));
        }
        Self { faces, cache: HashMap::new() }
    }

    /// Bredd i punkter för `text` i storlek `size`.
    fn width(&mut self, face: Face, size: f32, text: &str) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let key = (face, text.to_owned());
        let units = if let Some(w) = self.cache.get(&key) {
            *w
        } else {
            let f = &self.faces[&face];
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(text);
            buf.guess_segment_properties();
            let out = rustybuzz::shape(f, &[], buf);
            let w: i32 = out.glyph_positions().iter().map(|g| g.x_advance).sum();
            let w = w as f32 / f.units_per_em() as f32;
            if self.cache.len() < 50_000 {
                self.cache.insert(key, w);
            }
            w
        };
        units * size
    }
}

// ============================================================== blockmodell

#[derive(Clone, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    underline: bool,
    link: Option<String>,
    /// Färg, bakgrund, storlek och typsnittsklass från omgivande `<span style="…">`.
    span: SpanStyle,
    /// Fotnotsnummer (1-baserat): texten ritas som litet upphöjt nummer.
    fnref: Option<usize>,
}

#[derive(Clone)]
struct Span {
    text: String,
    style: Style,
}

enum Block {
    Heading(u8, Vec<Span>, Align),
    Para { spans: Vec<Span>, indent: f32, quote: u8, align: Align },
    Item { spans: Vec<Span>, indent: f32, marker: String, quote: u8, align: Align },
    Code { text: String, indent: f32 },
    /// `spans`: 1 vanlig cell, n sammanfogad, 0 täckt; `weights`: relativ kolumnbredd (streck).
    Table { rows: Vec<Vec<Vec<Span>>>, spans: Vec<Vec<usize>>, weights: Option<Vec<usize>> },
    Rule,
    Image { src: String, alt: String, indent: f32, size: ImgSize, align: Align, float: Option<Float> },
    /// Innehållsförteckning (`<!-- kladd: toc -->`).
    Toc,
    /// Nytt avsnitt (`<!-- kladd: avsnitt; … -->`): egen sidstorlek, marginaler och spalter.
    SectionBreak(SectionProps),
    /// Spaltbrytning (`<!-- kladd: spaltbrytning -->`).
    ColumnBreak,
}

/// Tar bort `<!--k:N-->` (ogenomskinliga Word-objekt) som aldrig ska skrivas ut.
fn strip_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<!--k:") {
        let after = &rest[i + 6..];
        match after.find("-->") {
            Some(j) if after[..j].chars().all(|c| c.is_ascii_digit()) => {
                out.push_str(&rest[..i]);
                rest = &after[j + 3..];
            }
            _ => {
                out.push_str(&rest[..i + 6]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Värdet av attributet `name` i en tagg (med citattecken).
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[at + name.len()..].trim_start();
        if before_ok {
            if let Some(r) = rest.strip_prefix('=') {
                let r = r.trim_start();
                let q = r.chars().next()?;
                if q == '"' || q == '\'' {
                    let end = r[1..].find(q)?;
                    return Some(r[1..1 + end].to_owned());
                }
            }
        }
        from = at + name.len();
    }
    None
}

/// `<img src="…" alt="…" width="W" [height="H"]>` med storlek i CSS-px (1/96 tum) omräknad till pt.
fn parse_img(tag: &str) -> Option<(String, String, ImgSize, Option<Float>)> {
    if !tag.get(..4)?.eq_ignore_ascii_case("<img") || !tag.ends_with('>') {
        return None;
    }
    let src = attr(tag, "src")?;
    let alt = attr(tag, "alt").unwrap_or_default();
    let px = |n: &str| attr(tag, n).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| *v > 0.0).map(|v| v * 0.75);
    let float = match attr(tag, "align").map(|a| a.trim().to_ascii_lowercase()).as_deref() {
        Some("left") => Some(Float::Left),
        Some("right") => Some(Float::Right),
        _ => None,
    };
    Some((src, alt, ImgSize { w: px("width"), h: px("height") }, float))
}

fn heading_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
fn parse_blocks(md: &str) -> Vec<Block> {
    parse_blocks_notes(md).0
}

/// Blocken och fotnoterna (text per fotnot, i den ordning referenserna kommer).
fn parse_blocks_notes(md: &str) -> (Vec<Block>, Vec<Vec<Span>>) {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    // Fotnoter: etiketter i ordning (nummer = plats + 1), definitionernas text och pågående definition.
    let mut note_labels: Vec<String> = Vec::new();
    let mut note_defs: Vec<(String, Vec<Span>)> = Vec::new();
    let mut note_cur: Option<(String, Vec<Block>)> = None;

    let mut blocks = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut bold = 0u32;
    let mut italic = 0u32;
    let mut strike = 0u32;
    let mut underline = 0u32;
    // Öppna `<span style>` (inre ovanpå yttre) och `<div align>` (innersta gäller).
    let mut span_stack: Vec<SpanStyle> = Vec::new();
    let mut aligns: Vec<Align> = Vec::new();
    let mut heading: Option<u8> = None;
    let mut quote = 0u8;
    // Listor: nästa nummer (None = punktlista)
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut pending_marker: Option<String> = None;
    let mut code: Option<String> = None;
    let mut image: Option<(String, String)> = None;
    let mut table: Option<Vec<Vec<Vec<Span>>>> = None;
    let mut row: Vec<Vec<Span>> = Vec::new();

    let indent_now = |lists: &Vec<Option<u64>>, quote: u8| lists.len() as f32 * INDENT + quote as f32 * 14.0;

    let flush = |spans: &mut Vec<Span>,
                 blocks: &mut Vec<Block>,
                 heading: Option<u8>,
                 pending_marker: &mut Option<String>,
                 lists: &Vec<Option<u64>>,
                 quote: u8,
                 align: Align| {
        if spans.iter().all(|s| s.text.trim().is_empty()) && pending_marker.is_none() {
            spans.clear();
            return;
        }
        let taken = std::mem::take(spans);
        let indent = indent_now(lists, quote);
        if let Some(h) = heading {
            blocks.push(Block::Heading(h, taken, align));
        } else if let Some(m) = pending_marker.take() {
            blocks.push(Block::Item { spans: taken, indent, marker: m, quote, align });
        } else {
            blocks.push(Block::Para { spans: taken, indent, quote, align });
        }
    };

    let push_text = |spans: &mut Vec<Span>, text: &str, style: &Style| {
        if let Some(last) = spans.last_mut() {
            if last.style == *style {
                last.text.push_str(text);
                return;
            }
        }
        spans.push(Span { text: text.to_owned(), style: style.clone() });
    };

    let mut table_src: Option<crate::word::markup::TableSrc> = None;
    for (ev, range) in Parser::new_ext(md, opts).into_offset_iter() {
        let align = aligns.last().copied().unwrap_or_default();
        style.span = span_stack.iter().fold(SpanStyle::default(), |acc, s| acc.merged(s));
        style.bold = bold > 0;
        style.italic = italic > 0;
        style.strike = strike > 0;
        style.underline = underline > 0;
        match ev {
            MdEvent::Start(Tag::FootnoteDefinition(label)) => {
                flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                note_cur = Some((label.to_string(), std::mem::take(&mut blocks)));
            }
            MdEvent::End(TagEnd::FootnoteDefinition) => {
                flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                if let Some((label, saved)) = note_cur.take() {
                    // Styckena i definitionen läggs efter varandra med ett mellanslag emellan.
                    let mut text: Vec<Span> = Vec::new();
                    for b in std::mem::replace(&mut blocks, saved) {
                        if let Block::Para { spans, .. } | Block::Item { spans, .. } = b {
                            if !text.is_empty() {
                                text.push(Span { text: " ".into(), style: Style::default() });
                            }
                            text.extend(spans);
                        }
                    }
                    note_defs.push((label, text));
                }
            }
            MdEvent::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    heading = Some(heading_num(level));
                }
                Tag::Paragraph => {
                    if pending_marker.is_none() {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    }
                }
                Tag::BlockQuote(_) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    quote += 1;
                }
                Tag::CodeBlock(_) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    code = Some(String::new());
                }
                Tag::List(start) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    lists.push(start);
                }
                Tag::Item => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    let depth = lists.len().saturating_sub(1);
                    pending_marker = Some(match lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}.");
                            *n += 1;
                            m
                        }
                        _ => ["•", "–", "·"][depth % 3].to_owned(),
                    });
                }
                Tag::Table(_) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    table_src = Some(crate::word::markup::table_src(md.get(range.clone()).unwrap_or("")));
                    table = Some(Vec::new());
                }
                Tag::TableHead | Tag::TableRow => row = Vec::new(),
                Tag::TableCell => spans.clear(),
                Tag::Emphasis => italic += 1,
                Tag::Strong => bold += 1,
                Tag::Strikethrough => strike += 1,
                Tag::Link { dest_url, .. } => {
                    let url = dest_url.to_string();
                    // Bara säkra länktyper blir klickbara (som i WordCraft).
                    let lower = url.to_ascii_lowercase();
                    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
                        style.link = Some(url);
                    }
                }
                Tag::Image { dest_url, .. } => image = Some((dest_url.to_string(), String::new())),
                _ => {}
            },
            MdEvent::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    flush(&mut spans, &mut blocks, heading, &mut pending_marker, &lists, quote, align);
                    heading = None;
                }
                TagEnd::Paragraph => flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align),
                TagEnd::BlockQuote(_) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    quote = quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    if let Some(c) = code.take() {
                        blocks.push(Block::Code {
                            text: c.trim_end_matches('\n').to_owned(),
                            indent: indent_now(&lists, quote),
                        });
                    }
                }
                TagEnd::Item => flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align),
                TagEnd::List(_) => {
                    lists.pop();
                }
                TagEnd::TableCell => row.push(std::mem::take(&mut spans)),
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some(t) = table.as_mut() {
                        t.push(std::mem::take(&mut row));
                    }
                }
                TagEnd::Table => {
                    if let Some(rows) = table.take() {
                        if !rows.is_empty() {
                            let src = table_src.take().unwrap_or(crate::word::markup::TableSrc { cols: 0, spans: Vec::new(), weights: None });
                            blocks.push(Block::Table { rows, spans: src.spans, weights: src.weights });
                        }
                    }
                }
                TagEnd::Emphasis => italic = italic.saturating_sub(1),
                TagEnd::Strong => bold = bold.saturating_sub(1),
                TagEnd::Strikethrough => strike = strike.saturating_sub(1),
                TagEnd::Link => style.link = None,
                TagEnd::Image => {
                    if let Some((src, alt)) = image.take() {
                        // En bild i en tabellcell eller mitt i text ritas som eget block efter stycket.
                        flush(&mut spans, &mut blocks, heading, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::Image { src, alt, indent: indent_now(&lists, quote), size: ImgSize::default(), align: Align::Left, float: None });
                    }
                }
                _ => {}
            },
            MdEvent::Text(t) => {
                if let Some(c) = code.as_mut() {
                    c.push_str(&t);
                } else if let Some((_, alt)) = image.as_mut() {
                    alt.push_str(&t);
                } else {
                    push_text(&mut spans, &t, &style);
                }
            }
            MdEvent::Code(t) => {
                let mut s = style.clone();
                s.code = true;
                push_text(&mut spans, &t, &s);
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => push_text(&mut spans, "\n", &style),
            MdEvent::Html(t) | MdEvent::InlineHtml(t) => {
                // Ett HTML-block kan ha flera rader; varje rad som är en känd tagg tolkas för sig.
                let mut first = true;
                let t = strip_markers(&t);
                for line in t.split('\n') {
                    let tag = line.trim();
                    if tag.is_empty() {
                        continue;
                    }
                    if tag.eq_ignore_ascii_case("<br>") || tag.eq_ignore_ascii_case("<br/>") {
                        push_text(&mut spans, "\n", &style);
                    } else if tag.eq_ignore_ascii_case("<u>") {
                        underline += 1;
                    } else if tag.eq_ignore_ascii_case("</u>") {
                        underline = underline.saturating_sub(1);
                    } else if let Some(st) = style::parse_span_open(tag) {
                        span_stack.push(st);
                        style.span = span_stack.iter().fold(SpanStyle::default(), |acc, s| acc.merged(s));
                    } else if style::is_span_close(tag) {
                        span_stack.pop();
                        style.span = span_stack.iter().fold(SpanStyle::default(), |acc, s| acc.merged(s));
                    } else if let Some(a) = style::parse_div_open(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        aligns.push(a);
                    } else if style::is_div_close(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        aligns.pop();
                    } else if let Some((src, alt, size, float)) = parse_img(tag) {
                        // `<img>` med storlek blir ett eget bildblock, justerat av omgivande `<div align>`.
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::Image { src, alt, indent: indent_now(&lists, quote), size, align, float });
                    } else if tag == crate::word::nav::TOC_LINE {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::Toc);
                    } else if let Some(props) = SectionProps::from_comment(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::SectionBreak(props));
                    } else if sections::is_column_break(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::ColumnBreak);
                    } else if tag.starts_with("<!--") && tag.ends_with("-->") {
                        // Kommentarer (också grundformatraden) skrivs inte ut.
                    } else {
                        if !first {
                            push_text(&mut spans, "\n", &style);
                        }
                        push_text(&mut spans, line.trim_end_matches('\r'), &style);
                        first = false;
                    }
                }
            }
            MdEvent::Rule => {
                flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                blocks.push(Block::Rule);
            }
            MdEvent::TaskListMarker(done) => push_text(&mut spans, if done { "☑ " } else { "☐ " }, &style),
            MdEvent::FootnoteReference(t) if note_cur.is_some() => push_text(&mut spans, &format!("[{t}]"), &style),
            MdEvent::FootnoteReference(t) => {
                // Samma etikett flera gånger är samma fotnot (som i Markdown).
                let n = match note_labels.iter().position(|l| l.as_str() == &*t) {
                    Some(i) => i + 1,
                    None => {
                        note_labels.push(t.to_string());
                        note_labels.len()
                    }
                };
                let st = Style { fnref: Some(n), ..style.clone() };
                spans.push(Span { text: n.to_string(), style: st });
            }
            MdEvent::InlineMath(t) | MdEvent::DisplayMath(t) => push_text(&mut spans, &t, &style),
        }
    }
    flush(&mut spans, &mut blocks, heading, &mut pending_marker, &lists, quote, aligns.last().copied().unwrap_or_default());
    let notes = note_labels
        .iter()
        .map(|l| note_defs.iter().find(|(d, _)| d == l).map(|(_, t)| t.clone()).unwrap_or_default())
        .collect();
    (blocks, notes)
}

// ============================================================== sidlayout

/// En ritinstruktion på en sida.
enum Op {
    Text { x: f32, y: f32, face: Face, size: f32, text: String, color: (u8, u8, u8), italic: bool },
    Rect { x: f32, y: f32, w: f32, h: f32, color: (u8, u8, u8) },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: (u8, u8, u8) },
    Image { x: f32, y: f32, w: f32, h: f32, index: usize },
    Link { x: f32, y: f32, w: f32, h: f32, url: String },
    /// Intern länk till en plats i dokumentet (sida från 0, y).
    Goto { x: f32, y: f32, w: f32, h: f32, page: usize, to_y: f32 },
}

/// Sidans mått (pt): bredd, höjd, marginaler och textytans vänsterkant och bredd (alla spalter).
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageGeom {
    pw: f32,
    ph: f32,
    mt: f32,
    mb: f32,
    ml: f32,
    cw: f32,
}

struct PageOps {
    ops: Vec<Op>,
    geom: PageGeom,
}

struct Heading {
    level: u8,
    text: String,
    page: usize,
    y: f32,
}

struct Layout<'a> {
    m: Measurer,
    pages: Vec<PageOps>,
    y: f32,
    headings: Vec<Heading>,
    images: Vec<Image>,
    resolve: &'a dyn Fn(&str) -> Option<DocImage>,
    missing: Vec<String>,
    /// Brödtextens storlek (dokumentets grundstorlek) och faktorn mot `BODY_SIZE`
    /// som rubriker, tabeller och kod skalas med.
    body: f32,
    scale: f32,
    /// Dokumentets formatmallar (`h1: …`, `body: …`, `quote: …`).
    styles: style::StyleSet,
    /// Radhöjd som faktor av teckenstorleken (`LINE` om formatmallen inte anger något).
    line: f32,
    /// Indrag av nästa blocks första rad (formatmallens `indent`).
    first_indent: f32,
    /// Grundtypsnitt för brödtext (Mono om dokumentets typsnitt är ett monospace).
    base: Face,
    /// Justering för blocket som läggs ut just nu.
    align: Align,
    /// Sidans mått och marginaler i pt (från grundformatraden): bredd, höjd, topp, botten, vänster
    /// och textbredd.
    pw: f32,
    ph: f32,
    mt: f32,
    mb: f32,
    ml: f32,
    cw: f32,
    /// Avsnittet: dokumentets grundformat, antal spalter, aktuell spalt (0-baserad), mellanrum, textytans
    /// vänsterkant och bredd, spaltens bredd och var spalterna börjar (y) på sidan.
    defaults: DocDefaults,
    cols: usize,
    col: usize,
    gap: f32,
    text_l: f32,
    text_w: f32,
    col_w: f32,
    rt: f32,
    /// Längsta y som använts i spalter som lämnats på sidan (för löpande avsnitt).
    page_ymax: f32,
    /// Räknas upp vid varje spalt- eller sidbyte.
    breaks: usize,
    /// Fotnoternas text (nummer − 1), och de som hör till sidan som läggs ut just nu.
    notes: Vec<Vec<Span>>,
    page_notes: Vec<usize>,
    /// Höjd längst ned på sidan som är reserverad för fotnoter (avskiljare och text).
    reserved: f32,
    /// Sant medan fotnoterna ritas (deras nummer är inga nya referenser).
    in_notes: bool,
    /// Flytande bilder som texten flyter runt just nu.
    floats: Vec<FloatBox>,
    /// Rubrikerna (nivå 1–3) som innehållsförteckningen listar, och var de hamnade i förra
    /// passet (sida från 0, y). Antalet rader är detsamma i båda passen.
    toc_entries: Vec<(u8, String)>,
    toc_pos: Vec<(usize, f32)>,
}

/// En flytande bild: sida, upptagen bredd (bild + luft), sidan (0-baserad) och understa kanten (pt).
struct FloatBox {
    side: Float,
    w: f32,
    page: usize,
    bottom: f32,
}

/// Luft mellan en flytande bild och texten (12 px).
const FLOAT_GAP: f32 = 9.0;
/// Minsta textbredd bredvid en flytande bild; smalare och bilden blir ett vanligt block.
const FLOAT_MIN_ROOM: f32 = 72.0;

/// En färdigbruten rad. `hard` = raden slutar med radbrytning (ska inte marginaljusteras).
struct Line {
    pieces: Vec<Piece>,
    hard: bool,
}

/// Typsnittsklass av ett typsnittsnamn från Word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    Sans,
    Serif,
    Mono,
}

/// Grov klassning på namnet: Kladd bäddar bara in IBM Plex Sans och Mono.
fn classify_family(name: &str) -> Family {
    let n = name.to_ascii_lowercase();
    const MONO: [&str; 8] = ["courier", "consolas", "mono", "menlo", "monaco", "lucida console", "typewriter", "fixed"];
    const SERIF: [&str; 14] = [
        "times", "georgia", "garamond", "cambria", "serif", "book", "palatino", "minion", "baskerville", "century",
        "didot", "constantia", "caslon", "roman",
    ];
    if MONO.iter().any(|m| n.contains(m)) {
        Family::Mono
    } else if n.contains("sans") {
        // "sans-serif" ska inte bli serif.
        Family::Sans
    } else if SERIF.iter().any(|m| n.contains(m)) {
        Family::Serif
    } else {
        Family::Sans
    }
}

/// Ett ord eller mellanslag med sin stil, klart att placera på en rad.
struct Piece {
    text: String,
    face: Face,
    size: f32,
    width: f32,
    style: Style,
    space: bool,
    newline: bool,
}

fn face_for(style: &Style, base: Face) -> Face {
    let base_mono = matches!(base, Face::Mono | Face::MonoMedium);
    let mono = style.code
        || match style.span.font_family.as_deref().map(classify_family) {
            Some(Family::Mono) => true,
            Some(_) => false,
            None => base_mono,
        };
    if mono {
        return if style.bold { Face::MonoMedium } else { Face::Mono };
    }
    // Ingen serif är inbäddad: serif-namn behåller sans.
    let base = if base_mono { Face::Sans } else { base };
    match (style.bold, base) {
        (true, Face::Sans) => Face::SansSemi,
        _ => base,
    }
}

/// Fördelar överskottet till `width` på mellanrummen i raden (marginaljustering).
fn justify(pieces: &mut [Piece], width: f32) {
    let total: f32 = pieces.iter().map(|p| p.width).sum();
    let gaps = pieces.iter().filter(|p| p.space).count();
    let extra = width - total;
    if gaps == 0 || extra <= 0.0 {
        return;
    }
    let add = extra / gaps as f32;
    for p in pieces.iter_mut().filter(|p| p.space) {
        p.width += add;
    }
}

/// Radens höjdgivande storlek: den största spannen på raden.
fn line_size(l: &Line, size: f32) -> f32 {
    let m = l.pieces.iter().map(|p| p.size).fold(0.0, f32::max);
    if m > 0.0 { m } else { size }
}

/// Bildens storlek enligt `width`/`height` i `<img>`, i pt.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ImgSize {
    w: Option<f32>,
    h: Option<f32>,
}

impl<'a> Layout<'a> {
    fn new(resolve: &'a dyn Fn(&str) -> Option<DocImage>, defaults: &DocDefaults) -> Self {
        let body = defaults.size();
        let base = match defaults.font_family.as_deref().map(classify_family) {
            Some(Family::Mono) => Face::Mono,
            _ => Face::Sans,
        };
        let (pw, ph) = defaults.page();
        let [mt, mr, mb, ml] = defaults.margins();
        // Orimliga marginaler (större än sidan) ger ändå en användbar textyta.
        let cw = (pw - ml - mr).max(36.0);
        Layout {
            m: Measurer::new(),
            pages: Vec::new(),
            y: mt,
            headings: Vec::new(),
            images: Vec::new(),
            resolve,
            missing: Vec::new(),
            body,
            scale: body / BODY_SIZE,
            styles: defaults.styles.clone(),
            line: LINE,
            first_indent: 0.0,
            base,
            align: Align::Left,
            pw,
            ph,
            mt,
            mb,
            ml,
            cw,
            defaults: defaults.clone(),
            cols: 1,
            col: 0,
            gap: 0.0,
            text_l: ml,
            text_w: cw,
            col_w: cw,
            rt: mt,
            page_ymax: 0.0,
            breaks: 0,
            notes: Vec::new(),
            page_notes: Vec::new(),
            reserved: 0.0,
            in_notes: false,
            floats: Vec::new(),
            toc_entries: Vec::new(),
            toc_pos: Vec::new(),
        }
    }

    /// Ritar innehållsförteckningen: rubriken "Innehåll" och en rad per rubrik med prickad
    /// ledlinje, sidnummer till höger och en intern länk till rubriken.
    fn toc(&mut self) {
        let size = 20.0 * self.scale;
        self.ensure(size * LINE + self.body * LINE * 3.0);
        let (ml, cw, body) = (self.ml, self.cw, self.body);
        let title = vec![Span { text: "Innehåll".to_owned(), style: Style::default() }];
        self.text_block(&title, ml, cw, Face::SansSemi, size, INK, false, None);
        self.y += 6.0;
        let face = self.base;
        let line_h = body * LINE;
        let entries = self.toc_entries.clone();
        if entries.is_empty() {
            let spans = vec![Span { text: "Inga rubriker i dokumentet".to_owned(), style: Style::default() }];
            self.text_block(&spans, ml, cw, face, body, MUTED, true, None);
            self.y += PARA_GAP;
            return;
        }
        for (i, (level, text)) in entries.iter().enumerate() {
            self.ensure(line_h);
            let indent = f32::from(level.saturating_sub(1)) * 14.0;
            let baseline = self.y + body * 1.08;
            let num = self.toc_pos.get(i).map(|(p, _)| (p + 1).to_string()).unwrap_or_default();
            let num_w = self.m.width(face, body, &num);
            let x = ml + indent;
            // Titeln kortas med "…" om den inte ryms bredvid sidnumret.
            let avail = (cw - indent - num_w - 16.0).max(20.0);
            let mut title = text.clone();
            if self.m.width(face, body, &title) > avail {
                while !title.is_empty() && self.m.width(face, body, &format!("{title}…")) > avail {
                    title.pop();
                }
                title.push('…');
            }
            let title_w = self.m.width(face, body, &title);
            let dot = self.m.width(face, body, ". ").max(1.0);
            let gap = cw - indent - title_w - num_w - 12.0;
            let dots = if gap > dot { ". ".repeat((gap / dot) as usize) } else { String::new() };
            let dots_w = self.m.width(face, body, &dots);
            let page = self.pages.len() - 1;
            let ops = &mut self.page().ops;
            ops.push(Op::Text { x, y: baseline, face, size: body, text: title, color: INK, italic: false });
            if !dots.is_empty() {
                ops.push(Op::Text { x: ml + cw - num_w - 6.0 - dots_w, y: baseline, face, size: body, text: dots, color: MUTED, italic: false });
            }
            if !num.is_empty() {
                ops.push(Op::Text { x: ml + cw - num_w, y: baseline, face, size: body, text: num, color: INK, italic: false });
            }
            if let Some((p, y)) = self.toc_pos.get(i).copied() {
                let top = self.y;
                self.pages[page].ops.push(Op::Goto { x, y: top, w: cw - indent, h: line_h, page: p, to_y: y });
            }
            self.y += line_h + 2.0;
        }
        self.y += PARA_GAP;
    }

    /// Sidhuvud och sidfot för sidan `f.page` (1-baserad) av `f.pages`, som ritinstruktioner.
    /// Baslinjen ligger halvvägs mellan sidkanten och marginalen på sidans egen storlek (avsnitt kan ha
    /// olika). Delarna vänster|mitten|höger ritas var för sig.
    fn header_footer(&mut self, d: &DocDefaults, f: &style::Fields) -> Vec<Op> {
        let size = self.body * 0.9;
        let g = self.pages.get(f.page.wrapping_sub(1)).map_or(
            PageGeom { pw: self.pw, ph: self.ph, mt: self.mt, mb: self.mb, ml: self.text_l, cw: self.text_w },
            |p| p.geom,
        );
        let mut out = Vec::new();
        let first = f.page == 1 && d.different_first();
        let items = [
            (d.header_template(f.page - 1).map(str::to_owned), d.header_align, g.mt / 2.0),
            (
                // Utan eget sidfotstecken visas ändå "N av M" till höger när dokumentet har flera sidor.
                d.footer_template(f.page - 1)
                    .map(str::to_owned)
                    .or_else(|| (d.footer.is_none() && !first && f.pages > 1).then(|| "{sida} av {sidor}".to_owned())),
                if d.footer.is_some() || first { d.footer_align } else { Align::Right },
                g.ph - g.mb / 2.0,
            ),
        ];
        for (tpl, align, y) in items {
            let Some(tpl) = tpl else { continue };
            for (i, part) in style::parts_of(&tpl, align).iter().enumerate() {
                let text = style::fill_fields(part, f).replace('\n', " ");
                if text.trim().is_empty() {
                    continue;
                }
                let w = self.m.width(self.base, size, &text);
                let x = match i {
                    1 => g.ml + ((g.cw - w) / 2.0).max(0.0),
                    2 => g.ml + (g.cw - w).max(0.0),
                    _ => g.ml,
                };
                out.push(Op::Text { x, y, face: self.base, size, text, color: MUTED, italic: false });
            }
        }
        out
    }

    /// Flyttar ner till under alla flytande bilder (block som inte flyter runt dem) och glömmer dem.
    fn clear_floats(&mut self) {
        self.prune_floats();
        if let Some(b) = self.floats.iter().map(|f| f.bottom).reduce(f32::max) {
            self.y = self.y.max(b);
        }
        self.floats.clear();
    }

    /// Tar bort flytande bilder som är passerade (annan sida, eller texten har kommit under dem).
    fn prune_floats(&mut self) {
        let (page, y) = (self.pages.len().wrapping_sub(1), self.y);
        self.floats.retain(|f| f.page == page && y < f.bottom - 0.5);
    }

    /// Utrymmet vid aktuell rad: (förskjutning från vänster, bredd som tas av flytande bilder).
    fn float_room(&mut self) -> (f32, f32) {
        self.prune_floats();
        let sum = |side: Float| self.floats.iter().filter(|f| f.side == side).map(|f| f.w).sum::<f32>();
        (sum(Float::Left), sum(Float::Left) + sum(Float::Right))
    }

    fn page(&mut self) -> &mut PageOps {
        self.pages.last_mut().expect("minst en sida")
    }

    /// Nästa spalt i avsnittet, annars en ny sida.
    fn new_page(&mut self) {
        if self.cols > 1 && self.col + 1 < self.cols && !self.pages.is_empty() {
            self.page_ymax = self.page_ymax.max(self.y);
            self.col += 1;
            self.ml = self.text_l + self.col as f32 * (self.col_w + self.gap);
            self.cw = self.col_w;
            self.y = self.rt;
            self.floats.clear();
            self.breaks += 1;
        } else {
            self.real_page();
        }
    }

    fn page_geom(&self) -> PageGeom {
        PageGeom { pw: self.pw, ph: self.ph, mt: self.mt, mb: self.mb, ml: self.text_l, cw: self.text_w }
    }

    fn real_page(&mut self) {
        self.flush_notes();
        let geom = self.page_geom();
        self.pages.push(PageOps { ops: Vec::new(), geom });
        self.y = self.mt;
        self.rt = self.mt;
        self.page_ymax = 0.0;
        self.col = 0;
        self.ml = self.text_l;
        self.cw = self.col_w;
        self.breaks += 1;
    }

    /// Tar avsnittets sidstorlek, marginaler och spalter i bruk (sidan eller spalten byts av anroparen).
    fn set_section(&mut self, sec: &Section) {
        let [mt, mr, mb, ml] = sec.margin;
        self.pw = sec.page.0;
        self.ph = sec.page.1;
        self.mt = mt;
        self.mb = mb;
        self.text_l = ml;
        self.text_w = (sec.page.0 - ml - mr).max(36.0);
        self.cols = sec.columns.max(1);
        let n = self.cols as f32;
        self.gap = if self.cols > 1 { sec.gap.min(((self.text_w - 18.0 * n) / (n - 1.0)).max(0.0)) } else { 0.0 };
        self.col_w = ((self.text_w - self.gap * (n - 1.0)) / n).max(18.0);
        self.col = 0;
        self.ml = ml;
        self.cw = self.col_w;
    }

    /// Avsnittsbrytning: ny sida med avsnittets mått, eller (löpande, samma sidstorlek) vidare på samma sida.
    fn section_break(&mut self, props: &SectionProps) {
        let sec = Section::resolve(props, &self.defaults);
        let page_empty = self.pages.last().is_some_and(|p| p.ops.is_empty()) && self.col == 0 && (self.y - self.rt).abs() < 0.5;
        let same_sheet = [(self.pw, sec.page.0), (self.ph, sec.page.1), (self.mt, sec.margin[0]), (self.mb, sec.margin[2]), (self.text_l, sec.margin[3])]
            .iter()
            .all(|(a, b)| (a - b).abs() < 0.5);
        if page_empty {
            // Början av dokumentet (eller två rader i rad): sidan görs om i stället för att lämnas tom.
            self.set_section(&sec);
            let geom = self.page_geom();
            if let Some(p) = self.pages.last_mut() {
                p.geom = geom;
            }
            self.y = self.mt;
            self.rt = self.mt;
            self.page_ymax = 0.0;
        } else if sec.continuous && same_sheet && !self.pages.is_empty() {
            let y = self.y.max(self.page_ymax);
            self.set_section(&sec);
            self.y = y;
            self.rt = y;
            self.page_ymax = 0.0;
            self.floats.clear();
            self.breaks += 1;
        } else {
            self.flush_notes();
            self.set_section(&sec);
            self.real_page();
        }
    }

    fn ensure(&mut self, h: f32) {
        if self.y + h + self.reserved > self.ph - self.mb && self.y > self.rt + 0.5 {
            self.new_page();
        }
    }

    /// Fotnotens rader: litet nummer, mellanslag och texten.
    fn note_spans(&self, n: usize) -> Vec<Span> {
        let mut v = vec![
            Span { text: n.to_string(), style: Style { fnref: Some(n), ..Style::default() } },
            Span { text: " ".into(), style: Style::default() },
        ];
        v.extend(self.notes.get(n - 1).cloned().unwrap_or_default());
        v
    }

    /// Hur mycket plats fotnoterna `refs` tar längst ned på sidan (med avskiljaren om det är de första).
    fn note_cost(&mut self, refs: &[usize]) -> f32 {
        let size = self.body * 0.85;
        let mut h = if self.page_notes.is_empty() && !refs.is_empty() { 9.0 } else { 0.0 };
        for &n in refs {
            let spans = self.note_spans(n);
            let width = self.cw;
            let lines = self.break_lines(&spans, self.base, size, width);
            h += lines.iter().map(|l| line_size(l, size) * LINE).sum::<f32>() + 2.0;
        }
        h
    }

    /// Ritar sidans fotnoter längst ned: en kort linje och sedan noterna i liten stil.
    fn flush_notes(&mut self) {
        if self.page_notes.is_empty() || self.pages.is_empty() {
            return;
        }
        let notes = std::mem::take(&mut self.page_notes);
        let top = self.ph - self.mb - self.reserved;
        self.reserved = 0.0;
        let (ml, cw) = (self.text_l, self.text_w);
        let (saved_y, saved_align) = (self.y, self.align);
        self.page().ops.push(Op::Line { x1: ml, y1: top + 3.0, x2: ml + cw / 3.0, y2: top + 3.0, width: 0.6, color: MUTED });
        self.y = top + 9.0;
        self.align = Align::Left;
        self.in_notes = true;
        let size = self.body * 0.85;
        for n in notes {
            let spans = self.note_spans(n);
            self.text_block(&spans, ml, cw, self.base, size, INK, false, None);
            self.y += 2.0;
        }
        self.in_notes = false;
        self.y = saved_y;
        self.align = saved_align;
    }

    /// Delar upp textstyckena i ord som kan radbrytas.
    fn pieces(&mut self, spans: &[Span], base: Face, size: f32) -> Vec<Piece> {
        let mut out = Vec::new();
        for s in spans {
            let face = face_for(&s.style, base);
            let sz = s.style.span.font_size.unwrap_or(size);
            let sz = if s.style.code { sz * 0.9 } else { sz };
            let sz = if s.style.fnref.is_some() { sz * 0.65 } else { sz };
            for (li, line) in s.text.split('\n').enumerate() {
                if li > 0 {
                    out.push(Piece { text: String::new(), face, size: sz, width: 0.0, style: s.style.clone(), space: false, newline: true });
                }
                let mut word = String::new();
                for c in line.chars() {
                    if c == ' ' || c == '\t' {
                        if !word.is_empty() {
                            let w = self.m.width(face, sz, &word);
                            out.push(Piece { text: std::mem::take(&mut word), face, size: sz, width: w, style: s.style.clone(), space: false, newline: false });
                        }
                        let sp = if c == '\t' { "    " } else { " " };
                        let w = self.m.width(face, sz, sp);
                        out.push(Piece { text: sp.to_owned(), face, size: sz, width: w, style: s.style.clone(), space: true, newline: false });
                    } else {
                        word.push(c);
                    }
                }
                if !word.is_empty() {
                    let w = self.m.width(face, sz, &word);
                    out.push(Piece { text: word, face, size: sz, width: w, style: s.style.clone(), space: false, newline: false });
                }
            }
        }
        out
    }

    /// Bryter ord för långa för raden i tecken.
    fn split_long(&mut self, p: Piece, max: f32) -> Vec<Piece> {
        if p.width <= max || p.space {
            return vec![p];
        }
        let mut out = Vec::new();
        let mut cur = String::new();
        for c in p.text.chars() {
            let mut next = cur.clone();
            next.push(c);
            if !cur.is_empty() && self.m.width(p.face, p.size, &next) > max {
                let w = self.m.width(p.face, p.size, &cur);
                out.push(Piece { text: std::mem::take(&mut cur), width: w, style: p.style.clone(), ..p_clone(&p) });
                cur.push(c);
            } else {
                cur = next;
            }
        }
        if !cur.is_empty() {
            let w = self.m.width(p.face, p.size, &cur);
            out.push(Piece { text: cur, width: w, style: p.style.clone(), ..p_clone(&p) });
        }
        out
    }

    /// Bryter stycket i rader som ryms i `width`. Efterföljande mellanslag tas bort.
    fn break_lines(&mut self, spans: &[Span], base: Face, size: f32, width: f32) -> Vec<Line> {
        let pieces = self.pieces(spans, base, size);
        let mut lines = vec![Line { pieces: Vec::new(), hard: false }];
        let mut line_w = 0.0;
        for p in pieces {
            if p.newline {
                lines.last_mut().unwrap().hard = true;
                lines.push(Line { pieces: Vec::new(), hard: false });
                line_w = 0.0;
                continue;
            }
            for p in self.split_long(p, width) {
                if !p.space && line_w + p.width > width && lines.last().is_some_and(|l| !l.pieces.is_empty()) {
                    lines.push(Line { pieces: Vec::new(), hard: false });
                    line_w = 0.0;
                }
                if p.space && lines.last().is_some_and(|l| l.pieces.is_empty()) {
                    continue; // inget mellanslag först på en rad
                }
                line_w += p.width;
                lines.last_mut().unwrap().pieces.push(p);
            }
        }
        for l in &mut lines {
            while l.pieces.last().is_some_and(|p| p.space) {
                l.pieces.pop();
            }
        }
        lines
    }

    /// Nästa rad ur `pieces` från `from`, bruten i `width`: raden och index för första ordet efter den.
    /// Samma girighet som `break_lines`, men en rad i taget så att bredden kan variera med flytande bilder.
    fn take_line(pieces: &[Piece], from: usize, width: f32) -> (Line, usize) {
        let mut line = Line { pieces: Vec::new(), hard: false };
        let (mut w, mut i) = (0.0, from);
        while i < pieces.len() {
            let p = &pieces[i];
            if p.newline {
                line.hard = true;
                i += 1;
                break;
            }
            if !p.space && w + p.width > width && !line.pieces.is_empty() {
                break;
            }
            i += 1;
            if p.space && line.pieces.is_empty() {
                continue; // inget mellanslag först på en rad
            }
            w += p.width;
            line.pieces.push(Piece { text: p.text.clone(), width: p.width, ..p_clone(p) });
        }
        while line.pieces.last().is_some_and(|p| p.space) {
            line.pieces.pop();
        }
        (line, i)
    }

    /// Lägger ut text i en ruta. Returnerar höjden som användes.
    #[allow(clippy::too_many_arguments)]
    fn text_block(
        &mut self,
        spans: &[Span],
        x: f32,
        width: f32,
        base: Face,
        size: f32,
        color: (u8, u8, u8),
        italic_all: bool,
        first_line_extra: Option<(String, Face)>,
    ) {
        let pieces = self.pieces(spans, base, size);
        let pieces: Vec<Piece> = pieces.into_iter().flat_map(|p| self.split_long(p, width)).collect();
        let (mut next, mut i) = (0usize, 0usize);
        let ml_entry = self.ml;
        loop {
            // Flytande bilder gör raden smalare (och förskjuter den vid vänsterbilder).
            let (mut fx, mut fw) = self.float_room();
            let ind0 = if i == 0 { self.first_indent } else { 0.0 };
            let (mut line, mut after) = Self::take_line(&pieces, next, (width - fw.min(width - 24.0) - ind0).max(1.0));
            let page_before = self.breaks;
            // Fotnoterna på raden ska ha plats längst ned på samma sida som raden.
            let refs_of = |this: &Self, line: &Line| -> Vec<usize> {
                if this.in_notes {
                    Vec::new()
                } else {
                    line.pieces.iter().filter_map(|p| p.style.fnref).filter(|n| !this.page_notes.contains(n)).collect()
                }
            };
            let mut refs = refs_of(self, &line);
            let line_h0 = line_size(&line, size) * self.line;
            let need0 = self.note_cost(&refs);
            if need0 > 0.0 && self.y + line_h0 + self.reserved + need0 > self.ph - self.mb && self.y > self.rt + 0.5 {
                self.new_page();
            }
            self.ensure(line_h0);
            // Texten följer med till nästa spalt.
            let x = x + (self.ml - ml_entry);
            if self.breaks != page_before {
                (fx, fw) = self.float_room();
                (line, after) = Self::take_line(&pieces, next, (width - fw.min(width - 24.0) - ind0).max(1.0));
                refs = refs_of(self, &line);
            }
            let need = self.note_cost(&refs);
            self.reserved += need;
            self.page_notes.extend(refs);
            let (x, width) = (x + fx + ind0, width - fw.min(width - 24.0) - ind0);
            let last = after >= pieces.len() && !line.hard;
            let msz = line_size(&line, size);
            let line_h = msz * self.line;
            let baseline = self.y + msz * 1.08;
            if i == 0 {
                if let Some((marker, mface)) = &first_line_extra {
                    let mw = self.m.width(*mface, size, marker);
                    self.page().ops.push(Op::Text {
                        x: x - mw - 6.0,
                        y: baseline,
                        face: *mface,
                        size,
                        text: marker.clone(),
                        color: MUTED,
                        italic: false,
                    });
                }
            }
            let lw: f32 = line.pieces.iter().map(|p| p.width).sum();
            let mut dx = 0.0;
            let mut stretched = false;
            match self.align {
                Align::Center => dx = ((width - lw) / 2.0).max(0.0),
                Align::Right => dx = (width - lw).max(0.0),
                Align::Justify if !last && !line.hard => {
                    justify(&mut line.pieces, width);
                    stretched = true;
                }
                _ => {}
            }
            // Slå ihop ord med samma stil till en körning.
            let mut cx = x + dx;
            let mut run: Option<(f32, String, Face, f32, Style, f32)> = None;
            let mut runs = Vec::new();
            for p in line.pieces {
                match run.as_mut() {
                    Some((_, text, f, sz, st, w)) if !stretched && *f == p.face && *sz == p.size && *st == p.style => {
                        text.push_str(&p.text);
                        *w += p.width;
                    }
                    _ => {
                        if let Some(r) = run.take() {
                            runs.push(r);
                        }
                        run = Some((cx, p.text.clone(), p.face, p.size, p.style.clone(), p.width));
                    }
                }
                cx += p.width;
            }
            if let Some(r) = run.take() {
                runs.push(r);
            }
            for (rx, text, face, sz, st, w) in runs {
                if st.code {
                    self.page().ops.push(Op::Rect { x: rx - 1.5, y: baseline - sz * 0.95, w: w + 3.0, h: sz * 1.3, color: CODE_BG });
                }
                if let Some(bg) = st.span.background {
                    self.page().ops.push(Op::Rect { x: rx, y: baseline - sz * 0.9, w, h: sz * 1.2, color: (bg[0], bg[1], bg[2]) });
                }
                let c = match (st.link.is_some(), st.span.color) {
                    (true, _) => LINK,
                    (false, Some(sc)) => (sc[0], sc[1], sc[2]),
                    (false, None) => color,
                };
                // Fotnotsnummer ritas upphöjda.
                let ty = if st.fnref.is_some() { baseline - size * 0.38 } else { baseline };
                self.page().ops.push(Op::Text { x: rx, y: ty, face, size: sz, text, color: c, italic: st.italic || italic_all });
                if st.link.is_some() {
                    self.page().ops.push(Op::Line { x1: rx, y1: baseline + 1.6, x2: rx + w, y2: baseline + 1.6, width: 0.6, color: LINK });
                }
                if st.underline && st.link.is_none() {
                    let uy = baseline + 1.6;
                    self.page().ops.push(Op::Line { x1: rx, y1: uy, x2: rx + w, y2: uy, width: 0.6, color: c });
                }
                if st.strike {
                    let sy = baseline - sz * 0.3;
                    self.page().ops.push(Op::Line { x1: rx, y1: sy, x2: rx + w, y2: sy, width: 0.7, color: c });
                }
                if let Some(url) = &st.link {
                    let y0 = self.y;
                    self.page().ops.push(Op::Link { x: rx, y: y0, w, h: line_h, url: url.clone() });
                }
            }
            self.y += line_h;
            next = after;
            i += 1;
            if last {
                break;
            }
        }
    }

    fn image(&mut self, src: &str, alt: &str, indent: f32, size: ImgSize, align: Align, float: Option<Float>) {
        // Bilder som inte flyter, och texten om bilden saknas, kommer under flytande bilder.
        let float = float.filter(|_| indent == 0.0);
        if float.is_none() {
            self.clear_floats();
        }
        let Some(img) = (self.resolve)(src) else {
            self.missing.push(src.to_owned());
            let label = if alt.is_empty() { format!("[bild: {src}]") } else { format!("[bild: {alt}]") };
            let spans = vec![Span { text: label, style: Style::default() }];
            self.text_block(&spans, self.ml + indent, self.cw - indent, self.base, self.body, MUTED, false, None);
            return;
        };
        let Some(image) = make_image(&img.bytes) else {
            self.missing.push(src.to_owned());
            return;
        };
        let (pw, ph) = image.size();
        // Storlek: originalets storlek i Word om den finns, annars 96 dpi, högst sidbredden.
        let (mut w, mut h) = match img.extent {
            Some((cx, cy)) => (cx as f32 / 12_700.0, cy as f32 / 12_700.0),
            None => (pw as f32 * 0.75, ph as f32 * 0.75),
        };
        // Angiven storlek (`width`/`height` i CSS-px, redan omräknat till pt); proportionerna bevaras
        // när bara en sida anges.
        let aspect = if pw > 0 { ph as f32 / pw as f32 } else { 1.0 };
        match (size.w, size.h) {
            (Some(sw), Some(sh)) => (w, h) = (sw, sh),
            (Some(sw), None) => (w, h) = (sw, sw * aspect),
            (None, Some(sh)) => (w, h) = (sh / aspect.max(0.0001), sh),
            _ => {}
        }
        let max_w = self.cw - indent;
        if w > max_w {
            h *= max_w / w;
            w = max_w;
        }
        let max_h = self.ph - self.mt - self.mb;
        if h > max_h {
            w *= max_h / h;
            h = max_h;
        }
        // Flytande bild: vid vänster/höger textkant, text runt; ryms den inte flyttas den till nästa sida.
        let mut float = float;
        if float.is_some() && (w + FLOAT_GAP > max_w - FLOAT_MIN_ROOM || !self.floats.is_empty() && self.floats.iter().any(|f| Some(f.side) == float)) {
            self.clear_floats();
            float = None;
        }
        self.ensure(h);
        let index = self.images.len();
        self.images.push(image);
        if let Some(side) = float {
            let (x, y) = (if side == Float::Left { self.ml } else { self.ml + self.cw - w }, self.y);
            self.page().ops.push(Op::Image { x, y, w, h, index });
            let page = self.pages.len() - 1;
            self.floats.push(FloatBox { side, w: w + FLOAT_GAP, page, bottom: y + h });
            return;
        }
        let free = (self.cw - indent - w).max(0.0);
        let dx = match align {
            Align::Center => free / 2.0,
            Align::Right => free,
            _ => 0.0,
        };
        let (x, y) = (self.ml + indent + dx, self.y);
        self.page().ops.push(Op::Image { x, y, w, h, index });
        self.y += h + PARA_GAP;
    }

    fn table(&mut self, rows: &[Vec<Vec<Span>>], spans: &[Vec<usize>], weights: Option<&[usize]>) {
        self.clear_floats();
        let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let size = self.body * 0.95;
        let pad = 5.0;
        let span_of = |ri: usize, ci: usize| spans.get(ri).and_then(|r| r.get(ci)).copied().unwrap_or(1);
        // Cellens text rad för rad (`<br>` är ett radslut).
        let widest = |this: &mut Self, cell: &[Span]| -> f32 {
            let text: String = cell.iter().map(|s| s.text.as_str()).collect();
            text.split('\n').map(|l| this.m.width(this.base, size, l)).fold(0.0, f32::max)
        };
        // Kolumnbredd: antalet streck i avgränsarraden, annars innehållets naturliga bredd inom sidbredden.
        let widths: Vec<f32> = match weights.filter(|w| w.len() == cols) {
            Some(w) => {
                let sum: usize = w.iter().sum();
                w.iter().map(|&n| self.cw * n as f32 / sum as f32).collect()
            }
            None => {
                let mut natural = vec![24.0f32; cols];
                let mut wide: Vec<(usize, usize, f32)> = Vec::new();
                for (ri, r) in rows.iter().enumerate() {
                    for (i, cell) in r.iter().enumerate() {
                        let w = widest(self, cell) + 2.0 * pad + 2.0;
                        match span_of(ri, i) {
                            0 => {}
                            1 => natural[i] = natural[i].max(w.min(self.cw)),
                            n => wide.push((i, n, w)),
                        }
                    }
                }
                for (i, n, w) in wide {
                    let have: f32 = natural.iter().skip(i).take(n).sum();
                    if have < w {
                        let add = (w.min(self.cw) - have) / n as f32;
                        for c in natural.iter_mut().skip(i).take(n) {
                            *c += add.max(0.0);
                        }
                    }
                }
                let total: f32 = natural.iter().sum();
                if total <= self.cw {
                    natural
                } else {
                    // Proportionellt, men ingen kolumn smalare än 30 pt; resten delar på det som blir över.
                    let mut w: Vec<f32> = natural.iter().map(|w| w / total * self.cw).collect();
                    let small: f32 = w.iter().filter(|x| **x < 30.0).count() as f32 * 30.0;
                    let big: f32 = w.iter().filter(|x| **x >= 30.0).sum();
                    let scale = if big > 0.0 { ((self.cw - small) / big).min(1.0) } else { 1.0 };
                    for x in &mut w {
                        *x = if *x < 30.0 { 30.0 } else { *x * scale };
                    }
                    w
                }
            }
        };
        let line_h = size * LINE;
        let total_w: f32 = widths.iter().sum();
        // Cellernas x och bredd (täckta celler utelämnas, ägaren får hela bredden).
        let cells_of = |ri: usize, len: usize| -> Vec<(usize, f32, f32)> {
            let mut out = Vec::new();
            let mut x = 0.0;
            for ci in 0..len.min(cols) {
                let n = span_of(ri, ci);
                let w: f32 = widths.iter().skip(ci).take(n.max(1)).sum();
                if n > 0 {
                    out.push((ci, x, w));
                }
                x += widths[ci];
            }
            out
        };
        for (ri, r) in rows.iter().enumerate() {
            let face = if ri == 0 { Face::SansSemi } else { self.base };
            let layout = cells_of(ri, r.len());
            // Mät radens höjd genom att lägga ut cellerna på en provsida.
            let mut heights = Vec::new();
            for &(ci, _, w) in &layout {
                heights.push(self.cell_height(&r[ci], w - 2.0 * pad, size, face));
            }
            let row_h = heights.iter().copied().fold(line_h, f32::max) + 2.0 * pad;
            self.ensure(row_h);
            let top = self.y;
            if ri == 0 {
                let ml = self.ml;
                self.page().ops.push(Op::Rect { x: ml, y: top, w: total_w, h: row_h, color: CODE_BG });
            }
            for &(ci, x, w) in &layout {
                self.y = top + pad;
                let ml = self.ml;
                self.text_block(&r[ci], ml + x + pad, w - 2.0 * pad, face, size, INK, false, None);
            }
            // Rutnät: kanter runt varje cell (en sammanfogad cell har inga streck inuti).
            let ml = self.ml;
            let right = layout.last().map_or(total_w, |&(_, x, w)| x + w);
            let page = self.page();
            page.ops.push(Op::Line { x1: ml, y1: top, x2: ml + right, y2: top, width: 0.6, color: RULE });
            page.ops.push(Op::Line { x1: ml, y1: top + row_h, x2: ml + right, y2: top + row_h, width: 0.6, color: RULE });
            for &(_, x, _) in &layout {
                page.ops.push(Op::Line { x1: ml + x, y1: top, x2: ml + x, y2: top + row_h, width: 0.6, color: RULE });
            }
            page.ops.push(Op::Line { x1: ml + right, y1: top, x2: ml + right, y2: top + row_h, width: 0.6, color: RULE });
            self.y = top + row_h;
        }
        self.y += PARA_GAP;
    }

    /// Höjden en cell får när texten bryts i `width`.
    fn cell_height(&mut self, spans: &[Span], width: f32, size: f32, base: Face) -> f32 {
        self.break_lines(spans, base, size, width).iter().map(|l| line_size(l, size) * LINE).sum()
    }

    fn code(&mut self, text: &str, indent: f32) {
        self.clear_floats();
        let size = self.body * 0.86;
        let line_h = size * 1.45;
        let pad = 8.0;
        let w = self.cw - indent;
        let mut lines = Vec::new();
        for raw in text.split('\n') {
            let raw = raw.replace('\t', "    ");
            // Långa kodrader bryts på tecken.
            let mut cur = String::new();
            for c in raw.chars() {
                cur.push(c);
                if self.m.width(Face::Mono, size, &cur) > w - 2.0 * pad {
                    let last = cur.pop().unwrap_or(' ');
                    lines.push(std::mem::take(&mut cur));
                    cur.push(last);
                }
            }
            lines.push(cur);
        }
        let mut i = 0;
        while i < lines.len() {
            self.ensure(line_h + 2.0 * pad);
            let x = self.ml + indent;
            // Så många rader som får plats på sidan.
            let room = ((self.ph - self.mb - self.reserved - self.y - 2.0 * pad) / line_h).floor().max(1.0) as usize;
            let chunk = &lines[i..(i + room).min(lines.len())];
            let h = chunk.len() as f32 * line_h + 2.0 * pad;
            let top = self.y;
            self.page().ops.push(Op::Rect { x, y: top, w, h, color: CODE_BG });
            for (j, l) in chunk.iter().enumerate() {
                if !l.is_empty() {
                    let y = top + pad + j as f32 * line_h + size * 1.05;
                    self.page().ops.push(Op::Text { x: x + pad, y, face: Face::Mono, size, text: l.clone(), color: INK, italic: false });
                }
            }
            i += chunk.len();
            self.y = top + h;
        }
        self.y += PARA_GAP;
    }

    fn run(&mut self, blocks: &[Block]) {
        for (bi, b) in blocks.iter().enumerate() {
            self.align = Align::Left;
            match b {
                Block::Heading(level, spans, align) => {
                    self.align = *align;
                    let (size, face, before) = match level {
                        1 => (22.0, Face::SansSemi, 14.0),
                        2 => (16.5, Face::SansSemi, 14.0),
                        3 => (13.5, Face::SansSemi, 10.0),
                        _ => (11.5, Face::SansSemi, 8.0),
                    };
                    let size = size * self.scale;
                    // Formatmallens utseende (typsnittet är alltid det inbäddade).
                    let st = self.styles.get(style::StyleId::Heading((*level).clamp(1, 6))).clone();
                    let size = st.size.unwrap_or(size);
                    let face = if st.bold == Some(false) { Face::Sans } else { face };
                    let ink = st.color.map_or(INK, |c| (c[0], c[1], c[2]));
                    if *align == Align::Left {
                        self.align = st.align.unwrap_or(Align::Left);
                    }
                    self.line = st.line_spacing.map_or(LINE, |m| m * LINE_SINGLE);
                    self.first_indent = st.first_indent.unwrap_or(0.0);
                    let before = before + st.space_before.unwrap_or(0.0);
                    if bi > 0 && self.y > self.rt + 0.5 {
                        self.y += before;
                    }
                    // Håll ihop rubriken med minst två rader text efter.
                    self.ensure(size * LINE + self.body * LINE * 2.0);
                    let text: String = spans.iter().map(|s| s.text.as_str()).collect::<String>().replace('\n', " ");
                    self.headings.push(Heading { level: *level, text: text.trim().to_owned(), page: self.pages.len() - 1, y: self.y });
                    let spans: Vec<Span> = spans.iter().map(|s| Span { text: s.text.replace('\n', " "), style: s.style.clone() }).collect();
                    self.text_block(&spans, self.ml, self.cw, face, size, ink, st.italic == Some(true), None);
                    self.line = LINE;
                    self.first_indent = 0.0;
                    self.y += st.space_after.unwrap_or(0.0);
                    if *level == 1 {
                        let y = self.y + 2.0;
                        let ml = self.ml;
                        self.page().ops.push(Op::Rect { x: ml, y, w: 36.0, h: 2.2, color: ACCENT });
                        self.y += 9.0;
                    } else {
                        self.y += 3.0;
                    }
                }
                Block::Para { spans, indent, quote, align } => {
                    self.align = *align;
                    let page_before = self.breaks;
                    let (color, italic) = if *quote > 0 { (MUTED, true) } else { (INK, false) };
                    let st = self.styles.get(if *quote > 0 { style::StyleId::Quote } else { style::StyleId::Body }).clone();
                    let (color, italic) = (st.color.map_or(color, |c| (c[0], c[1], c[2])), st.italic.unwrap_or(italic));
                    let size = st.size.unwrap_or(self.body);
                    let face = if st.bold == Some(true) && self.base == Face::Sans { Face::SansSemi } else { self.base };
                    if *align == Align::Left {
                        self.align = st.align.unwrap_or(Align::Left);
                    }
                    self.line = st.line_spacing.map_or(LINE, |m| m * LINE_SINGLE);
                    self.first_indent = st.first_indent.unwrap_or(0.0);
                    self.y += st.space_before.unwrap_or(0.0);
                    let top = self.y;
                    self.text_block(spans, self.ml + indent, self.cw - indent, face, size, color, italic, None);
                    self.line = LINE;
                    self.first_indent = 0.0;
                    self.y += st.space_after.unwrap_or(0.0);
                    if *quote > 0 {
                        let x = self.ml + indent - 9.0;
                        let start = if self.breaks == page_before { top } else { self.rt };
                        let end = self.y;
                        self.page().ops.push(Op::Rect { x, y: start + 2.0, w: 2.0, h: (end - start - 2.0).max(1.0), color: RULE });
                    }
                    self.y += PARA_GAP;
                }
                Block::Item { spans, indent, marker, quote, align } => {
                    self.align = *align;
                    let mface = if marker.ends_with('.') { Face::Sans } else { Face::SansMedium };
                    let color = if *quote > 0 { MUTED } else { INK };
                    self.text_block(spans, self.ml + indent + 4.0, self.cw - indent - 4.0, self.base, self.body, color, *quote > 0, Some((marker.clone(), mface)));
                    let next_is_item = matches!(blocks.get(bi + 1), Some(Block::Item { .. }));
                    self.y += if next_is_item { 2.0 } else { PARA_GAP };
                }
                Block::Code { text, indent } => self.code(text, *indent),
                Block::Table { rows, spans, weights } => self.table(rows, spans, weights.as_deref()),
                Block::Rule => {
                    self.clear_floats();
                    self.ensure(16.0);
                    let y = self.y + 6.0;
                    let (ml, cw) = (self.ml, self.cw);
                    self.page().ops.push(Op::Line { x1: ml, y1: y, x2: ml + cw, y2: y, width: 0.8, color: RULE });
                    self.y += 16.0;
                }
                Block::Image { src, alt, indent, size, align, float } => self.image(src, alt, *indent, *size, *align, *float),
                Block::Toc => self.toc(),
                Block::SectionBreak(props) => self.section_break(props),
                Block::ColumnBreak => self.new_page(),
            }
        }
    }
}

fn p_clone(p: &Piece) -> Piece {
    Piece { text: String::new(), face: p.face, size: p.size, width: 0.0, style: p.style.clone(), space: p.space, newline: p.newline }
}

fn make_image(bytes: &[u8]) -> Option<Image> {
    let data = || krilla::Data::from(bytes.to_vec());
    match crate::docx::image_ext(bytes) {
        Some("png") => Image::from_png(data(), true).ok(),
        Some("jpeg") => Image::from_jpeg(data(), true).ok(),
        Some("gif") => Image::from_gif(data(), true).ok(),
        _ => {
            // BMP och annat: avkoda till RGBA.
            let img = image::load_from_memory(bytes).ok()?.to_rgba8();
            let (w, h) = img.dimensions();
            Some(Image::from_rgba8(img.into_raw(), w, h))
        }
    }
}

fn fill(c: (u8, u8, u8)) -> Fill {
    Fill { paint: rgb::Color::new(c.0, c.1, c.2).into(), opacity: NormalizedF32::ONE, rule: FillRule::NonZero }
}

/// Resultatet av att spara: filens innehåll och bilder som inte kunde bäddas in.
pub struct Written {
    pub bytes: Vec<u8>,
    pub missing_images: Vec<String>,
}

/// Dokumentets grundformat från första raden (`<!-- kladd: … -->`).
fn doc_defaults(md: &str) -> DocDefaults {
    let first = md.trim_start_matches('\u{feff}').lines().next().unwrap_or("");
    DocDefaults::from_comment(first).unwrap_or_default()
}

/// Lägger ut alla block. Med en innehållsförteckning görs två passager: förteckningens höjd beror
/// bara på antalet rubriker, så sidnumren från första passet stämmer i det andra.
fn lay_out<'a>(blocks: &[Block], notes: &[Vec<Span>], defaults: &DocDefaults, resolve: &'a dyn Fn(&str) -> Option<DocImage>) -> Layout<'a> {
    // Rubrikerna H1–H3 till en eventuell innehållsförteckning (samma urval som bokmärkena).
    let toc_entries: Vec<(u8, String)> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Heading(l, spans, _) if *l <= 3 => {
                let t: String = spans.iter().map(|s| s.text.as_str()).collect::<String>().replace('\n', " ");
                (!t.trim().is_empty()).then(|| (*l, t.trim().to_owned()))
            }
            _ => None,
        })
        .collect();
    let mut lay = Layout::new(resolve, defaults);
    lay.toc_entries = toc_entries.clone();
    lay.notes = notes.to_vec();
    lay.new_page();
    lay.run(blocks);
    lay.flush_notes();
    if blocks.iter().any(|b| matches!(b, Block::Toc)) {
        let pos: Vec<(usize, f32)> =
            lay.headings.iter().filter(|h| h.level <= 3 && !h.text.is_empty()).map(|h| (h.page, h.y)).collect();
        let mut second = Layout::new(resolve, defaults);
        second.toc_entries = toc_entries;
        second.toc_pos = pos;
        second.notes = notes.to_vec();
        second.new_page();
        second.run(blocks);
        second.flush_notes();
        lay = second;
    }
    lay
}

/// Bygger en PDF av Markdown-text.
pub fn write(markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<DocImage>) -> Result<Written, String> {
    // Ändringar godkänns och kommentarer utelämnas i PDF.
    let cleaned = crate::review::clean_for_export(markdown);
    let markdown = cleaned.as_str();
    let (blocks, notes) = parse_blocks_notes(markdown);
    let defaults = doc_defaults(markdown);
    let mut lay = lay_out(&blocks, &notes, &defaults, resolve);

    let fonts: HashMap<Face, Font> = [Face::Sans, Face::SansMedium, Face::SansSemi, Face::Mono, Face::MonoMedium]
        .into_iter()
        .map(|f| (f, Font::new(krilla::Data::from(f.data()), 0).expect("inbyggt typsnitt")))
        .collect();

    let mut doc = Document::new();
    let total = lay.pages.len();
    let images = std::mem::take(&mut lay.images);
    // Sidhuvud och sidfot ritas sist, när sidantalet är känt.
    let fields = style::Fields::for_export(title, "pdf");
    let extras: Vec<Vec<Op>> = (0..total).map(|pi| lay.header_footer(&defaults, &fields.at(pi + 1, total))).collect();
    for (pi, page_ops) in lay.pages.iter().enumerate() {
        let mut page = doc.start_page_with(PageSettings::from_wh(page_ops.geom.pw, page_ops.geom.ph).ok_or("ogiltig sidstorlek")?);
        let mut links = Vec::new();
        let mut gotos = Vec::new();
        {
            let mut s = page.surface();
            for op in page_ops.ops.iter().chain(extras[pi].iter()) {
                match op {
                    Op::Rect { x, y, w, h, color } => {
                        if let Some(r) = Rect::from_xywh(*x, *y, *w, *h) {
                            let mut pb = PathBuilder::new();
                            pb.push_rect(r);
                            if let Some(path) = pb.finish() {
                                s.set_fill(Some(fill(*color)));
                                s.set_stroke(None);
                                s.draw_path(&path);
                            }
                        }
                    }
                    Op::Line { x1, y1, x2, y2, width, color } => {
                        let mut pb = PathBuilder::new();
                        pb.move_to(*x1, *y1);
                        pb.line_to(*x2, *y2);
                        if let Some(path) = pb.finish() {
                            s.set_fill(None);
                            s.set_stroke(Some(Stroke {
                                paint: rgb::Color::new(color.0, color.1, color.2).into(),
                                width: *width,
                                miter_limit: 4.0,
                                line_cap: LineCap::Butt,
                                line_join: LineJoin::Miter,
                                opacity: NormalizedF32::ONE,
                                dash: None,
                            }));
                            s.draw_path(&path);
                        }
                    }
                    Op::Text { x, y, face, size, text, color, italic } => {
                        s.set_stroke(None);
                        s.set_fill(Some(fill(*color)));
                        if *italic {
                            // Plex finns inte i kursiv här: luta texten 12° (falsk kursiv).
                            s.push_transform(&Transform::from_row(1.0, 0.0, -0.21, 1.0, 0.21 * y, 0.0));
                        }
                        s.draw_text(Point::from_xy(*x, *y), fonts[face].clone(), *size, text, false, TextDirection::Auto);
                        if *italic {
                            s.pop();
                        }
                    }
                    Op::Image { x, y, w, h, index } => {
                        if let Some(size) = Size::from_wh(*w, *h) {
                            s.push_transform(&Transform::from_translate(*x, *y));
                            s.draw_image(images[*index].clone(), size);
                            s.pop();
                        }
                    }
                    Op::Link { x, y, w, h, url } => links.push((*x, *y, *w, *h, url.clone())),
                    Op::Goto { x, y, w, h, page, to_y } => gotos.push((*x, *y, *w, *h, *page, *to_y)),
                }
            }
            s.finish();
        }
        for (x, y, w, h, url) in links {
            if let Some(rect) = Rect::from_xywh(x, y, w.max(1.0), h) {
                let target = krilla::annotation::Target::Action(krilla::action::Action::Link(krilla::action::LinkAction::new(url)));
                page.add_annotation(krilla::annotation::Annotation::new_link(krilla::annotation::LinkAnnotation::new(rect, target), None));
            }
        }
        for (x, y, w, h, to_page, to_y) in gotos {
            if let Some(rect) = Rect::from_xywh(x, y, w.max(1.0), h) {
                let dest = krilla::destination::XyzDestination::new(to_page, Point::from_xy(lay.ml, to_y));
                let target = krilla::annotation::Target::Destination(krilla::destination::Destination::Xyz(dest));
                page.add_annotation(krilla::annotation::Annotation::new_link(krilla::annotation::LinkAnnotation::new(rect, target), None));
            }
        }
        page.finish();
    }

    // Bokmärken från rubrikerna (nivå 1–3), nästlade efter nivå.
    let mut outline = krilla::outline::Outline::new();
    let mut stack: Vec<(u8, krilla::outline::OutlineNode)> = Vec::new();
    for h in lay.headings.iter().filter(|h| h.level <= 3 && !h.text.is_empty()) {
        let dest = krilla::destination::XyzDestination::new(h.page, Point::from_xy(lay.ml, h.y));
        let node = krilla::outline::OutlineNode::new(h.text.clone(), dest);
        while stack.last().is_some_and(|(l, _)| *l >= h.level) {
            let (_, done) = stack.pop().unwrap();
            match stack.last_mut() {
                Some((_, parent)) => parent.push_child(done),
                None => outline.push_child(done),
            }
        }
        stack.push((h.level, node));
    }
    while let Some((_, done)) = stack.pop() {
        match stack.last_mut() {
            Some((_, parent)) => parent.push_child(done),
            None => outline.push_child(done),
        }
    }
    doc.set_outline(outline);

    let heading_title = lay.headings.first().map(|h| h.text.clone());
    doc.set_metadata(
        krilla::metadata::Metadata::new()
            .title(heading_title.unwrap_or_else(|| title.to_owned()))
            .creator("Kladd".to_owned())
            .producer(format!("Kladd {}", env!("CARGO_PKG_VERSION")))
            .language("sv".to_owned()),
    );

    let bytes = doc.finish().map_err(|e| format!("PDF:en kunde inte skapas: {e:?}"))?;
    Ok(Written { bytes, missing_images: lay.missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lägger ut Markdown och ger sidans ritinstruktioner (för kontroll av tabellernas geometri).
    fn first_page_ops(md: &str) -> (Vec<(f32, f32, f32, f32)>, Vec<(f32, f32, String)>, f32, f32) {
        let blocks = parse_blocks(md);
        let defaults = doc_defaults(md);
        let resolve = |_: &str| None;
        let mut lay = Layout::new(&resolve, &defaults);
        lay.new_page();
        lay.run(&blocks);
        let mut vlines = Vec::new();
        let mut texts = Vec::new();
        for op in &lay.pages[0].ops {
            match op {
                Op::Line { x1, y1, x2, y2, .. } if (x1 - x2).abs() < 0.01 => vlines.push((*x1, *y1, *x2, *y2)),
                Op::Text { x, y, text, .. } => texts.push((*x, *y, text.clone())),
                _ => {}
            }
        }
        (vlines, texts, lay.ml, lay.cw)
    }

    #[test]
    fn table_columns_follow_dash_counts() {
        let (v, _, ml, cw) = first_page_ops("| A | B | C |\n| ---- | -- | -- |\n| 1 | 2 | 3 |\n");
        let mut xs: Vec<f32> = v.iter().map(|l| l.0).collect();
        xs.sort_by(|a, b| a.total_cmp(b));
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert_eq!(xs.len(), 4, "{xs:?}");
        assert!((xs[0] - ml).abs() < 0.01 && (xs[3] - (ml + cw)).abs() < 0.01, "{xs:?} {ml} {cw}");
        let w: Vec<f32> = xs.windows(2).map(|p| p[1] - p[0]).collect();
        assert!((w[0] / w[1] - 2.0).abs() < 0.01 && (w[1] - w[2]).abs() < 0.01, "{w:?}");
    }

    #[test]
    fn table_merged_cells_have_no_inner_border() {
        let (v, texts, ml, _) = first_page_ops("| Två || x |\n| --- | --- | --- |\n| a | b | c |\n");
        // Rubrikradens lodräta streck: vänster, mellan sammanfogad cell och x, höger. Brödraden har fyra.
        let tops: Vec<f32> = {
            let mut t: Vec<f32> = v.iter().map(|l| l.1).collect();
            t.sort_by(|a, b| a.total_cmp(b));
            t.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            t
        };
        assert_eq!(tops.len(), 2, "{tops:?}");
        let head = v.iter().filter(|l| (l.1 - tops[0]).abs() < 0.01).count();
        let body = v.iter().filter(|l| (l.1 - tops[1]).abs() < 0.01).count();
        assert_eq!((head, body), (3, 4), "{v:?}");
        // Texten i den sammanfogade cellen börjar i första kolumnen och "x" efter den.
        let tva = texts.iter().find(|t| t.2 == "Två").unwrap();
        let x = texts.iter().find(|t| t.2 == "x").unwrap();
        assert!(tva.0 < ml + 10.0 && x.0 > tva.0 + 30.0, "{tva:?} {x:?}");
    }

    #[test]
    fn table_cells_wrap_and_break_lines() {
        let one = first_page_ops("| A | B |\n| --- | --- |\n| kort | kort |\n").0;
        let height = |v: &[(f32, f32, f32, f32)], row: usize| {
            let mut t: Vec<&(f32, f32, f32, f32)> = v.iter().collect();
            t.sort_by(|a, b| a.1.total_cmp(&b.1));
            let l = t[row * 3];
            l.3 - l.1
        };
        let h1 = height(&one, 1);
        // `<br>` ger tre rader i cellen.
        let (v, texts, _, _) = first_page_ops("| A | B |\n| --- | --- |\n| a<br>b<br>c | kort |\n");
        assert!(height(&v, 1) > 2.0 * h1, "{} vs {h1}", height(&v, 1));
        let ys: Vec<f32> = ["a", "b", "c"].iter().map(|n| texts.iter().find(|t| t.2 == *n).unwrap().1).collect();
        assert!(ys[0] < ys[1] && ys[1] < ys[2], "{ys:?}");
        // Lång text bryts inom sin kolumn i stället för att göra tabellen bredare.
        let long = "ord ".repeat(60);
        let (v, _, ml, cw) = first_page_ops(&format!("| A | B |\n| --- | --- |\n| {long} | kort |\n"));
        assert!(height(&v, 1) > 2.0 * h1, "{}", height(&v, 1));
        assert!(v.iter().all(|l| l.0 <= ml + cw + 0.01), "tabellen är bredare än texten: {v:?} {ml} {cw}");
    }

    #[test]
    fn writes_a_pdf() {
        let md = "# Rubrik\n\nEn **fet** och *kursiv* rad med [länk](https://example.com) och `kod`.\n\n- ett\n- två\n    1. nästlad\n\n> citat\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n```\nkod\n```\n\n---\n\n![saknas](finns-inte.png)\n";
        let w = write(md, "Test", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF-"));
        assert_eq!(w.missing_images, vec!["finns-inte.png".to_owned()]);
    }

    #[test]
    fn toc_has_title_entries_and_real_page_numbers() {
        let mut md = String::from("<!-- kladd: toc -->\n\n");
        for i in 1..=4 {
            md.push_str(&format!("# Kapitel {i}\n\n"));
            for _ in 0..22 {
                md.push_str("Brödtext som fyller ut sidan så att kapitlen hamnar på olika sidor i dokumentet. Och lite till så att raden bryts.\n\n");
            }
            md.push_str(&format!("## Avsnitt {i}\n\nKort.\n\n"));
        }
        let w = write(&md, "T", &|_| None).unwrap();
        // Rubrikernas riktiga sidor, ur layouten.
        let blocks = parse_blocks(&md);
        assert!(blocks.iter().any(|b| matches!(b, Block::Toc)));
        let lay = lay_out(&blocks, &[], &doc_defaults(&md), &|_| None);
        let pages: Vec<usize> = lay.headings.iter().map(|h| h.page + 1).collect();
        assert_eq!(pages.len(), 8);
        assert!(pages.last().unwrap() > &2, "{pages:?}");
        // PDF:en innehåller förteckningen med de sidnummer rubrikerna faktiskt fick.
        let back = crate::pdf_read::read(&w.bytes, None).ok().expect("läsbar PDF");
        assert!(back.markdown.contains("Innehåll"), "{}", back.markdown);
        for (name, page) in ["Kapitel 1", "Avsnitt 1", "Kapitel 2", "Avsnitt 2", "Kapitel 3", "Avsnitt 3", "Kapitel 4", "Avsnitt 4"].iter().zip(&pages) {
            let at = back.markdown.find(&format!("{name} .")).unwrap_or_else(|| panic!("{name} saknas i förteckningen:\n{}", back.markdown));
            let rest = back.markdown[at + name.len()..].trim_start_matches([' ', '.']);
            let num: String = rest.chars().take_while(char::is_ascii_digit).collect();
            assert_eq!(num, page.to_string(), "{name}:\n{}", back.markdown);
        }
        assert!(back.page_count >= 3);
    }

    #[test]
    fn underline_tags_are_not_literal() {
        let w = write("En <u>understruken</u> text.\n", "Test", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF-"));
        let blocks = parse_blocks("En <u>understruken</u> text.\n");
        let Some(Block::Para { spans, .. }) = blocks.first() else { panic!("inget stycke") };
        assert!(spans.iter().all(|s| !s.text.contains("<u>") && !s.text.contains("</u>")));
        let under: String = spans.iter().filter(|s| s.style.underline).map(|s| s.text.as_str()).collect();
        assert_eq!(under, "understruken");
    }

    fn all_spans(blocks: &[Block]) -> Vec<&Span> {
        let mut out = Vec::new();
        for b in blocks {
            match b {
                Block::Heading(_, s, _) | Block::Para { spans: s, .. } | Block::Item { spans: s, .. } => out.extend(s.iter()),
                _ => {}
            }
        }
        out
    }

    const FULL: &str = "<!-- kladd: font-family: \"Courier New\"; font-size: 12pt -->\n\n# Rubrik\n\n<div align=\"center\">\n\nMitten med <span style=\"color: #C00000; font-size: 20pt\">röd stor</span> text.\n\n</div>\n\n<div align=\"justify\">\n\nEtt <span style=\"background-color: #FFFF00\">gult <span style=\"font-family: Times New Roman\">inre</span></span> stycke som är långt nog att bryta över flera rader så att marginaljusteringen får något att göra med mellanrummen.\n\n- punkt i <span style=\"font-family: Consolas\">mono</span>\n\n</div>\n\n<div align=\"right\">\n\nHöger\n\n</div>\n\n<!-- annan kommentar -->\n\nSlut.\n";

    #[test]
    fn span_colour_size_background_are_carried() {
        let blocks = parse_blocks("A <span style=\"color: #C00000; font-size: 20pt; background-color: #FFFF00\">röd <b>x</b></span> B.\n");
        let spans = all_spans(&blocks);
        let red: Vec<_> = spans.iter().filter(|s| s.style.span.color.is_some()).collect();
        assert!(!red.is_empty());
        for s in &red {
            assert_eq!(s.style.span.color, Some([0xC0, 0, 0]));
            assert_eq!(s.style.span.font_size, Some(20.0));
            assert_eq!(s.style.span.background, Some([0xFF, 0xFF, 0]));
        }
        assert!(spans.iter().any(|s| s.style.span.is_empty() && s.text.contains('B')));
    }

    #[test]
    fn nested_spans_merge_and_pop() {
        let blocks = parse_blocks("<span style=\"color: red\">a <span style=\"font-size: 9pt\">b</span> c</span> d\n");
        let spans = all_spans(&blocks);
        let b = spans.iter().find(|s| s.text.contains('b')).unwrap();
        assert_eq!(b.style.span.color, Some([255, 0, 0]));
        assert_eq!(b.style.span.font_size, Some(9.0));
        let c = spans.iter().find(|s| s.text.contains('c')).unwrap();
        assert_eq!(c.style.span.font_size, None);
        let d = spans.iter().find(|s| s.text.contains('d')).unwrap();
        assert!(d.style.span.is_empty());
    }

    #[test]
    fn div_alignment_is_carried_and_tags_hidden() {
        let blocks = parse_blocks(FULL);
        let mut aligns = Vec::new();
        for b in &blocks {
            match b {
                Block::Heading(_, _, a) | Block::Para { align: a, .. } | Block::Item { align: a, .. } => aligns.push(*a),
                _ => {}
            }
        }
        // rubrik, mitten, justerat stycke, punkt, höger, slut
        assert_eq!(aligns, vec![Align::Left, Align::Center, Align::Justify, Align::Justify, Align::Right, Align::Left]);
        for s in all_spans(&blocks) {
            for bad in ["<span", "</span", "<div", "</div", "<!--", "-->"] {
                assert!(!s.text.contains(bad), "{bad} i {:?}", s.text);
            }
        }
    }

    #[test]
    fn family_classification() {
        assert_eq!(classify_family("Times New Roman"), Family::Serif);
        assert_eq!(classify_family("Georgia"), Family::Serif);
        assert_eq!(classify_family("Courier New"), Family::Mono);
        assert_eq!(classify_family("Consolas"), Family::Mono);
        assert_eq!(classify_family("Arial"), Family::Sans);
        assert_eq!(classify_family("Noto Sans"), Family::Sans);
        let mut st = Style::default();
        st.span.font_family = Some("Courier New".into());
        assert_eq!(face_for(&st, Face::Sans), Face::Mono);
        st.span.font_family = Some("Georgia".into());
        assert_eq!(face_for(&st, Face::Sans), Face::Sans);
    }

    #[test]
    fn doc_defaults_from_first_line() {
        let d = doc_defaults(FULL);
        assert_eq!(d.font_size, Some(12.0));
        assert_eq!(d.font_family.as_deref(), Some("Courier New"));
        assert!(doc_defaults("# Hej\n").is_empty());
    }

    fn test_layout(resolve: &dyn Fn(&str) -> Option<DocImage>) -> Layout<'_> {
        Layout::new(resolve, &DocDefaults { font_size: Some(BODY_SIZE), ..Default::default() })
    }

    #[test]
    fn justify_fills_every_line_but_the_last() {
        let resolve = |_: &str| None;
        let mut lay = test_layout(&resolve);
        let text = "Det här är en ganska lång mening som måste brytas på flera rader för att vi ska kunna mäta hur marginaljusteringen fördelar utrymmet.";
        let spans = vec![Span { text: text.into(), style: Style::default() }];
        let width = 200.0;
        let lines = lay.break_lines(&spans, Face::Sans, BODY_SIZE, width);
        assert!(lines.len() >= 3);
        let n = lines.len();
        for (i, mut l) in lines.into_iter().enumerate() {
            let before: f32 = l.pieces.iter().map(|p| p.width).sum();
            assert!(before <= width + 0.01);
            if i + 1 < n && !l.hard {
                justify(&mut l.pieces, width);
                let after: f32 = l.pieces.iter().map(|p| p.width).sum();
                assert!((after - width).abs() < 0.01, "rad {i}: {after}");
            }
        }
    }

    #[test]
    fn bigger_span_raises_the_line() {
        let resolve = |_: &str| None;
        let mut lay = test_layout(&resolve);
        let mut big = Style::default();
        big.span.font_size = Some(24.0);
        let spans = vec![Span { text: "liten ".into(), style: Style::default() }, Span { text: "stor".into(), style: big }];
        let lines = lay.break_lines(&spans, Face::Sans, BODY_SIZE, 400.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_size(&lines[0], BODY_SIZE), 24.0);
    }

    #[test]
    fn style_overrides_change_pdf_layout() {
        let resolve = |_: &str| None;
        let ys = |md: &str| {
            let (lay, _) = laid_out(md, &resolve);
            (lay.headings[0].y, lay.y)
        };
        let doc = "Före\n\n# Rubrik\n\nEfter\n";
        let (h0, end0) = ys(doc);
        // Luft före rubriken flyttar den, luft efter flyttar texten under.
        let (h1, end1) = ys(&format!("<!-- kladd: h1: space-before 20pt space-after 10pt -->\n\n{doc}"));
        assert!((h1 - h0 - 20.0).abs() < 0.01, "{h0} {h1}");
        assert!((end1 - end0 - 30.0).abs() < 0.01, "{end0} {end1}");
        // Radavstånd i brödtexten och större rubrikstorlek ger en högre sida.
        let (_, end2) = ys(&format!("<!-- kladd: body: line-spacing 2 -->\n\n{doc}"));
        assert!(end2 > end0, "{end0} {end2}");
        let big = ys(&format!("<!-- kladd: h1: 40pt -->\n\n{doc}")).1;
        assert!(big > end0);
        // Hela dokumentet blir ändå en PDF.
        let w = write("<!-- kladd: h1: 30pt bold #112233 center indent 10pt; body: italic justify indent 12pt; quote: 9pt -->\n\n# R\n\nText\n\n> citat\n", "x", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn document_with_all_features_makes_a_pdf() {
        let w = write(FULL, "Allt", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF"));
        let w = write("<div align=\"justify\">\n\nEn <span style=\"background-color: #FF0\">gul</span> rad.\n\n</div>\n", "x", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn long_documents_get_several_pages() {
        let md: String = (0..400).map(|i| format!("Stycke {i} med lite text som fyller sidan.\n\n")).collect();
        let w = write(&md, "Lång", &|_| None).unwrap();
        let s = String::from_utf8_lossy(&w.bytes);
        assert!(s.matches("/Type /Page").count() > 3 || s.matches("/Type/Page").count() > 3);
    }

    // --- sidformat, sidhuvud/sidfot, bildstorlekar ---------------------------------

    fn laid_out<'a>(md: &str, resolve: &'a dyn Fn(&str) -> Option<DocImage>) -> (Layout<'a>, DocDefaults) {
        let d = doc_defaults(md);
        let mut lay = Layout::new(resolve, &d);
        lay.new_page();
        lay.run(&parse_blocks(md));
        (lay, d)
    }

    fn texts(ops: &[Op]) -> Vec<(f32, f32, String)> {
        ops.iter()
            .filter_map(|o| match o {
                Op::Text { x, y, text, .. } => Some((*x, *y, text.clone())),
                _ => None,
            })
            .collect()
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(w, h))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn images_of(lay: &Layout) -> Vec<(f32, f32, f32, f32)> {
        lay.pages
            .iter()
            .flat_map(|p| p.ops.iter())
            .filter_map(|o| match o {
                Op::Image { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn page_size_and_margins_follow_the_comment() {
        let md = "<!-- kladd: page-size: letter landscape; page-margin: 36pt 50pt 40pt 60pt -->\n\nHej\n";
        let (lay, _) = laid_out(md, &|_| None);
        assert!((lay.pw - 792.0).abs() < 0.5 && (lay.ph - 612.0).abs() < 0.5, "{} x {}", lay.pw, lay.ph);
        assert_eq!((lay.mt, lay.mb, lay.ml), (36.0, 40.0, 60.0));
        assert!((lay.cw - (792.0 - 110.0)).abs() < 0.5);
        let t = texts(&lay.pages[0].ops);
        assert!((t[0].0 - 60.0).abs() < 0.01, "texten börjar i vänstermarginalen");
        assert!(write(md, "x", &|_| None).unwrap().bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn header_and_footer_on_every_page_with_page_numbers() {
        let md = "<!-- kladd: header: \"Rapport\"; header-align: right; footer: \"Sida {sida} av {sidor}\"; footer-align: center -->\n\n".to_owned()
            + &(0..150).map(|i| format!("Stycke {i} med lite text som fyller sidan.\n\n")).collect::<String>();
        let (mut lay, d) = laid_out(&md, &|_| None);
        let total = lay.pages.len();
        assert!(total >= 3, "{total} sidor");
        for p in 1..=total {
            let ops = lay.header_footer(&d, &style::Fields::new(p, total, "x"));
            let t = texts(&ops);
            assert_eq!(t.len(), 2);
            assert_eq!(t[0].2, "Rapport");
            assert_eq!(t[1].2, format!("Sida {p} av {total}"));
            // Baslinjer halvvägs i topp- och bottenmarginalen.
            assert!((t[0].1 - lay.mt / 2.0).abs() < 0.01);
            assert!((t[1].1 - (lay.ph - lay.mb / 2.0)).abs() < 0.01);
            // Sidfoten är centrerad, sidhuvudet högerställt.
            let fw = lay.m.width(lay.base, lay.body * 0.9, &t[1].2);
            assert!((t[1].0 + fw / 2.0 - (lay.ml + lay.cw / 2.0)).abs() < 0.01);
            let hw = lay.m.width(lay.base, lay.body * 0.9, "Rapport");
            assert!((t[0].0 + hw - (lay.ml + lay.cw)).abs() < 0.01);
        }
        assert!(write(&md, "x", &|_| None).unwrap().bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn header_parts_fields_and_first_page_in_pdf() {
        let md = "<!-- kladd: header: \"Rapport|{titel}|{datum}\"; footer: \"{sida}/{sidor}\"; footer-align: right; header-first: \"\"; footer-first: \"Omslag\" -->\n\n".to_owned()
            + &(0..150).map(|i| format!("Stycke {i} med lite text som fyller sidan.\n\n")).collect::<String>();
        let (mut lay, d) = laid_out(&md, &|_| None);
        let total = lay.pages.len();
        assert!(total >= 3);
        let f = |p: usize| style::Fields { page: p, pages: total, title: "Min rapport".into(), file: "Min rapport.pdf".into(), date: "2026-11-13".into() };
        // Första sidan: tomt sidhuvud och egen sidfot.
        let t1 = texts(&lay.header_footer(&d, &f(1)));
        assert_eq!(t1.iter().map(|t| t.2.as_str()).collect::<Vec<_>>(), ["Omslag"]);
        // Övriga sidor: tre delar i sidhuvudet (vänster, mitten, höger) och sidfot med fält.
        let t2 = texts(&lay.header_footer(&d, &f(2)));
        assert_eq!(t2.iter().map(|t| t.2.as_str()).collect::<Vec<_>>(), ["Rapport", "Min rapport", "2026-11-13", format!("2/{total}").as_str()]);
        let (l, c, r) = (&t2[0], &t2[1], &t2[2]);
        assert!((l.0 - lay.ml).abs() < 0.01);
        let cw = lay.m.width(lay.base, lay.body * 0.9, "Min rapport");
        assert!((c.0 + cw / 2.0 - (lay.ml + lay.cw / 2.0)).abs() < 0.01);
        let rw = lay.m.width(lay.base, lay.body * 0.9, "2026-11-13");
        assert!((r.0 + rw - (lay.ml + lay.cw)).abs() < 0.01);
        assert!(write(&md, "Min rapport", &|_| None).unwrap().bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn no_header_means_no_header_text() {
        let (mut lay, d) = laid_out("Hej\n", &|_| None);
        assert!(texts(&lay.header_footer(&d, &style::Fields::new(1, 1, "x"))).is_empty());
    }

    #[test]
    fn img_width_sets_size_keeping_aspect_and_clamps() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None });
        let (lay, _) = laid_out("<img src=\"bild:a.png\" alt=\"A\" width=\"192\">\n", &resolve);
        let im = images_of(&lay);
        assert_eq!(im.len(), 1);
        assert!((im[0].2 - 144.0).abs() < 0.01 && (im[0].3 - 72.0).abs() < 0.01, "{im:?}");
        // Både bredd och höjd.
        let (lay, _) = laid_out("<img src=\"a\" width=\"96\" height=\"96\">\n", &resolve);
        let im = images_of(&lay);
        assert!((im[0].2 - 72.0).abs() < 0.01 && (im[0].3 - 72.0).abs() < 0.01);
        // För bred bild krymps till textbredden.
        let (lay, _) = laid_out("<img src=\"a\" width=\"5000\">\n", &resolve);
        let im = images_of(&lay);
        assert!((im[0].2 - lay.cw).abs() < 0.01 && (im[0].3 - lay.cw / 2.0).abs() < 0.01);
        // Utan storlek: som förut (96 dpi).
        let (lay, _) = laid_out("![A](a)\n", &resolve);
        let im = images_of(&lay);
        assert!((im[0].2 - 75.0).abs() < 0.01);
    }

    #[test]
    fn div_align_places_an_image() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None });
        let md = "<div align=\"center\">\n\n<img src=\"a\" width=\"96\">\n\n</div>\n\n<div align=\"right\">\n\n<img src=\"a\" width=\"96\">\n\n</div>\n";
        let (lay, _) = laid_out(md, &resolve);
        let im = images_of(&lay);
        assert_eq!(im.len(), 2);
        assert!((im[0].0 - (lay.ml + (lay.cw - 72.0) / 2.0)).abs() < 0.01, "{im:?}");
        assert!((im[1].0 - (lay.ml + lay.cw - 72.0)).abs() < 0.01);
    }

    const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.";

    #[test]
    fn floats_wrap_text_line_by_line() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None });
        for (side, tag) in [(Float::Left, "left"), (Float::Right, "right")] {
            let md = format!("<img src=\"a\" width=\"192\" align=\"{tag}\">\n\n{LOREM}\n\n{LOREM}\n\n{LOREM}\n\n{LOREM}\n");
            let (mut lay, _) = laid_out(&md, &resolve);
            let im = images_of(&lay);
            assert_eq!(im.len(), 1);
            let (ix, iy, iw, ih) = im[0];
            let want_x = if side == Float::Left { lay.ml } else { lay.ml + lay.cw - iw };
            assert!((ix - want_x).abs() < 0.01, "{side:?}: bild vid {ix}");
            let body = lay.body;
            let (mut beside, mut below) = (0, 0);
            for (x, y, t) in texts(&lay.pages[0].ops) {
                let w = lay.m.width(Face::Sans, body, &t);
                if y < iy + ih {
                    beside += 1;
                    let (lo, hi) = if side == Float::Left { (lay.ml + iw + FLOAT_GAP, lay.ml + lay.cw) } else { (lay.ml, lay.ml + lay.cw - iw - FLOAT_GAP) };
                    assert!(x >= lo - 0.01 && x + w <= hi + 0.5, "{side:?}: rad vid {x}..{} (y {y}) utanför {lo}..{hi}", x + w);
                } else {
                    below += 1;
                    assert!((x - lay.ml).abs() < 0.01, "{side:?}: rad under bilden vid {x}");
                }
            }
            assert!(beside >= 3 && below >= 3, "{side:?}: {beside} bredvid, {below} under");
            // Bilden tar ingen egen höjd: första textraden ligger överst.
            let first = texts(&lay.pages[0].ops).first().map(|t| t.1).unwrap();
            assert!(first < lay.mt + 20.0, "första raden y = {first}");
        }
    }

    #[test]
    fn float_that_does_not_fit_moves_to_the_next_page() {
        let data = png(100, 100);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None });
        // Fyll sidan nästan helt, sedan en bild på 150 pt som inte ryms.
        let filler: Vec<String> = (0..12).map(|_| LOREM.to_owned()).collect();
        let md = format!("{}\n\n<img src=\"a\" width=\"200\" align=\"left\">\n\n{LOREM}\n\n{LOREM}\n", filler.join("\n\n"));
        let (lay, _) = laid_out(&md, &resolve);
        assert!(lay.pages.len() >= 2);
        let on_page = |k: usize| lay.pages[k].ops.iter().filter(|o| matches!(o, Op::Image { .. })).count();
        assert_eq!((on_page(0), on_page(1)), (0, 1));
        // Texten efter bilden står bredvid den på sida 2.
        let t = texts(&lay.pages[1].ops);
        assert!(t.iter().any(|(x, _, _)| *x > lay.ml + 100.0), "{t:?}");
    }

    #[test]
    fn float_without_room_is_a_plain_block() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None });
        let md = format!("<img src=\"a\" width=\"700\" align=\"left\">\n\n{LOREM}\n");
        let (lay, _) = laid_out(&md, &resolve);
        let (_, iy, _, ih) = images_of(&lay)[0];
        assert!(texts(&lay.pages[0].ops).iter().all(|(x, y, _)| (*x - lay.ml).abs() < 0.01 && *y > iy + ih - 1.0));
    }

    #[test]
    fn k_markers_are_never_printed() {
        for md in ["Före <!--k:1--> efter\n", "<!--k:12-->\n\nText\n", "<!--k:3--> rad\n"] {
            let (lay, _) = laid_out(md, &|_| None);
            let all: String = texts(&lay.pages[0].ops).iter().map(|t| t.2.clone()).collect::<Vec<_>>().join(" ");
            assert!(!all.contains("<!--") && !all.contains("k:"), "{md:?} -> {all:?}");
        }
        assert_eq!(strip_markers("a<!--k:1-->b<!-- x -->"), "ab<!-- x -->");
    }
}

/// `KLADD_MD_IN=a.md KLADD_PDF_OUT=b.pdf cargo test pdf_manual -- --ignored --nocapture`
#[cfg(test)]
mod manual {
    #[test]
    #[ignore]
    fn pdf_manual() {
        let (Ok(i), Ok(o)) = (std::env::var("KLADD_MD_IN"), std::env::var("KLADD_PDF_OUT")) else { return };
        let md = std::fs::read_to_string(&i).unwrap();
        let dir = std::path::Path::new(&i).parent().unwrap().to_path_buf();
        let w = super::write(&md, "Prov", &|src| {
            std::fs::read(dir.join(src)).ok().map(|bytes| crate::docx::Image { bytes, extent: None })
        })
        .unwrap();
        println!("saknade bilder: {:?}", w.missing_images);
        std::fs::write(o, w.bytes).unwrap();
    }
}

#[cfg(test)]
#[path = "pdf_fn_tests.rs"]
mod fn_tests;

#[cfg(test)]
#[path = "pdf_section_tests.rs"]
mod section_tests;
