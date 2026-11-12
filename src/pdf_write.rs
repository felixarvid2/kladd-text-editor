//! Sparar Markdown som PDF.
//!
//! Upplägget följer WordCraft (github.com/storytold/wordcraft, MIT/Apache-2.0), som
//! också skriver PDF med `krilla`: typsnitten bäddas in som delmängder, texten går att
//! markera och söka i, rubriker blir bokmärken och länkar blir klickbara.
//!
//! Kladd har en egen enkel sidlayout: A4, IBM Plex Sans för brödtext och rubriker,
//! IBM Plex Mono för kod, radbrytning mellan ord (mätt med rustybuzz, samma formning
//! som krilla använder när texten ritas) och sidnummer i sidfoten.

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

// --- sidmått i punkter (1/72 tum) -------------------------------------------
const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const MARGIN_X: f32 = 64.0;
const MARGIN_TOP: f32 = 68.0;
const MARGIN_BOTTOM: f32 = 72.0;
const CONTENT_W: f32 = PAGE_W - 2.0 * MARGIN_X;
const BODY_SIZE: f32 = 10.5;
const LINE: f32 = 1.5;
const PARA_GAP: f32 = 7.5;
const INDENT: f32 = 16.0;

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
    Table { rows: Vec<Vec<Vec<Span>>> },
    Rule,
    Image { src: String, alt: String, indent: f32 },
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

fn parse_blocks(md: &str) -> Vec<Block> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);

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

    for ev in Parser::new_ext(md, opts) {
        let align = aligns.last().copied().unwrap_or_default();
        style.span = span_stack.iter().fold(SpanStyle::default(), |acc, s| acc.merged(s));
        style.bold = bold > 0;
        style.italic = italic > 0;
        style.strike = strike > 0;
        style.underline = underline > 0;
        match ev {
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
                            blocks.push(Block::Table { rows });
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
                        blocks.push(Block::Image { src, alt, indent: indent_now(&lists, quote) });
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
            MdEvent::FootnoteReference(t) => push_text(&mut spans, &format!("[{t}]"), &style),
            MdEvent::InlineMath(t) | MdEvent::DisplayMath(t) => push_text(&mut spans, &t, &style),
        }
    }
    flush(&mut spans, &mut blocks, heading, &mut pending_marker, &lists, quote, aligns.last().copied().unwrap_or_default());
    blocks
}

// ============================================================== sidlayout

/// En ritinstruktion på en sida.
enum Op {
    Text { x: f32, y: f32, face: Face, size: f32, text: String, color: (u8, u8, u8), italic: bool },
    Rect { x: f32, y: f32, w: f32, h: f32, color: (u8, u8, u8) },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: (u8, u8, u8) },
    Image { x: f32, y: f32, w: f32, h: f32, index: usize },
    Link { x: f32, y: f32, w: f32, h: f32, url: String },
}

struct PageOps {
    ops: Vec<Op>,
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
    /// Grundtypsnitt för brödtext (Mono om dokumentets typsnitt är ett monospace).
    base: Face,
    /// Justering för blocket som läggs ut just nu.
    align: Align,
}

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

impl Layout<'_> {
    fn page(&mut self) -> &mut PageOps {
        self.pages.last_mut().expect("minst en sida")
    }

    fn new_page(&mut self) {
        self.pages.push(PageOps { ops: Vec::new() });
        self.y = MARGIN_TOP;
    }

    fn ensure(&mut self, h: f32) {
        if self.y + h > PAGE_H - MARGIN_BOTTOM && self.y > MARGIN_TOP + 0.5 {
            self.new_page();
        }
    }

    /// Delar upp textstyckena i ord som kan radbrytas.
    fn pieces(&mut self, spans: &[Span], base: Face, size: f32) -> Vec<Piece> {
        let mut out = Vec::new();
        for s in spans {
            let face = face_for(&s.style, base);
            let sz = s.style.span.font_size.unwrap_or(size);
            let sz = if s.style.code { sz * 0.9 } else { sz };
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
        let lines = self.break_lines(spans, base, size, width);
        let n = lines.len();
        for (i, mut line) in lines.into_iter().enumerate() {
            let msz = line_size(&line, size);
            let line_h = msz * LINE;
            self.ensure(line_h);
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
                Align::Justify if i + 1 < n && !line.hard => {
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
                self.page().ops.push(Op::Text { x: rx, y: baseline, face, size: sz, text, color: c, italic: st.italic || italic_all });
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
        }
    }

    fn image(&mut self, src: &str, alt: &str, indent: f32) {
        let Some(img) = (self.resolve)(src) else {
            self.missing.push(src.to_owned());
            let label = if alt.is_empty() { format!("[bild: {src}]") } else { format!("[bild: {alt}]") };
            let spans = vec![Span { text: label, style: Style::default() }];
            self.text_block(&spans, MARGIN_X + indent, CONTENT_W - indent, self.base, self.body, MUTED, false, None);
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
        let max_w = CONTENT_W - indent;
        if w > max_w {
            h *= max_w / w;
            w = max_w;
        }
        let max_h = PAGE_H - MARGIN_TOP - MARGIN_BOTTOM;
        if h > max_h {
            w *= max_h / h;
            h = max_h;
        }
        self.ensure(h);
        let index = self.images.len();
        self.images.push(image);
        let (x, y) = (MARGIN_X + indent, self.y);
        self.page().ops.push(Op::Image { x, y, w, h, index });
        self.y += h + PARA_GAP;
    }

    fn table(&mut self, rows: &[Vec<Vec<Span>>]) {
        let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let size = self.body * 0.95;
        let pad = 5.0;
        // Kolumnbredd efter innehållets naturliga bredd, inom sidbredden.
        let mut natural = vec![24.0f32; cols];
        for r in rows {
            for (i, cell) in r.iter().enumerate() {
                let text: String = cell.iter().map(|s| s.text.as_str()).collect();
                let w = self.m.width(self.base, size, &text) + 2.0 * pad + 2.0;
                natural[i] = natural[i].max(w.min(CONTENT_W));
            }
        }
        let total: f32 = natural.iter().sum();
        let widths: Vec<f32> = if total <= CONTENT_W {
            natural
        } else {
            natural.iter().map(|w| (w / total * CONTENT_W).max(30.0)).collect()
        };
        let line_h = size * LINE;
        for (ri, r) in rows.iter().enumerate() {
            // Mät radens höjd genom att lägga ut cellerna på en provsida.
            let mut heights = Vec::new();
            for (ci, cell) in r.iter().enumerate() {
                let base = if ri == 0 { Face::SansSemi } else { self.base };
                heights.push(self.cell_height(cell, widths[ci] - 2.0 * pad, size, base));
            }
            let row_h = heights.iter().copied().fold(line_h, f32::max) + 2.0 * pad;
            self.ensure(row_h);
            let top = self.y;
            if ri == 0 {
                self.page().ops.push(Op::Rect { x: MARGIN_X, y: top, w: widths.iter().sum(), h: row_h, color: CODE_BG });
            }
            let mut x = MARGIN_X;
            for (ci, w) in widths.iter().enumerate() {
                if let Some(cell) = r.get(ci) {
                    self.y = top + pad;
                    let face = if ri == 0 { Face::SansSemi } else { self.base };
                    self.text_block(cell, x + pad, w - 2.0 * pad, face, size, INK, false, None);
                }
                x += w;
            }
            // Rutnät
            let total_w: f32 = widths.iter().sum();
            let page = self.page();
            page.ops.push(Op::Line { x1: MARGIN_X, y1: top, x2: MARGIN_X + total_w, y2: top, width: 0.6, color: RULE });
            page.ops.push(Op::Line { x1: MARGIN_X, y1: top + row_h, x2: MARGIN_X + total_w, y2: top + row_h, width: 0.6, color: RULE });
            let mut x = MARGIN_X;
            for w in widths.iter().chain(std::iter::once(&0.0)) {
                page.ops.push(Op::Line { x1: x, y1: top, x2: x, y2: top + row_h, width: 0.6, color: RULE });
                x += w;
            }
            self.y = top + row_h;
        }
        self.y += PARA_GAP;
    }

    /// Höjden en cell får när texten bryts i `width`.
    fn cell_height(&mut self, spans: &[Span], width: f32, size: f32, base: Face) -> f32 {
        self.break_lines(spans, base, size, width).iter().map(|l| line_size(l, size) * LINE).sum()
    }

    fn code(&mut self, text: &str, indent: f32) {
        let size = self.body * 0.86;
        let line_h = size * 1.45;
        let pad = 8.0;
        let x = MARGIN_X + indent;
        let w = CONTENT_W - indent;
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
            // Så många rader som får plats på sidan.
            let room = ((PAGE_H - MARGIN_BOTTOM - self.y - 2.0 * pad) / line_h).floor().max(1.0) as usize;
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
                    if bi > 0 && self.y > MARGIN_TOP + 0.5 {
                        self.y += before;
                    }
                    // Håll ihop rubriken med minst två rader text efter.
                    self.ensure(size * LINE + self.body * LINE * 2.0);
                    let text: String = spans.iter().map(|s| s.text.as_str()).collect::<String>().replace('\n', " ");
                    self.headings.push(Heading { level: *level, text: text.trim().to_owned(), page: self.pages.len() - 1, y: self.y });
                    let spans: Vec<Span> = spans.iter().map(|s| Span { text: s.text.replace('\n', " "), style: s.style.clone() }).collect();
                    self.text_block(&spans, MARGIN_X, CONTENT_W, face, size, INK, false, None);
                    if *level == 1 {
                        let y = self.y + 2.0;
                        self.page().ops.push(Op::Rect { x: MARGIN_X, y, w: 36.0, h: 2.2, color: ACCENT });
                        self.y += 9.0;
                    } else {
                        self.y += 3.0;
                    }
                }
                Block::Para { spans, indent, quote, align } => {
                    self.align = *align;
                    let top = self.y;
                    let page_before = self.pages.len();
                    let (color, italic) = if *quote > 0 { (MUTED, true) } else { (INK, false) };
                    self.text_block(spans, MARGIN_X + indent, CONTENT_W - indent, self.base, self.body, color, italic, None);
                    if *quote > 0 {
                        let x = MARGIN_X + indent - 9.0;
                        let start = if self.pages.len() == page_before { top } else { MARGIN_TOP };
                        let end = self.y;
                        self.page().ops.push(Op::Rect { x, y: start + 2.0, w: 2.0, h: (end - start - 2.0).max(1.0), color: RULE });
                    }
                    self.y += PARA_GAP;
                }
                Block::Item { spans, indent, marker, quote, align } => {
                    self.align = *align;
                    let mface = if marker.ends_with('.') { Face::Sans } else { Face::SansMedium };
                    let color = if *quote > 0 { MUTED } else { INK };
                    self.text_block(spans, MARGIN_X + indent + 4.0, CONTENT_W - indent - 4.0, self.base, self.body, color, *quote > 0, Some((marker.clone(), mface)));
                    let next_is_item = matches!(blocks.get(bi + 1), Some(Block::Item { .. }));
                    self.y += if next_is_item { 2.0 } else { PARA_GAP };
                }
                Block::Code { text, indent } => self.code(text, *indent),
                Block::Table { rows } => self.table(rows),
                Block::Rule => {
                    self.ensure(16.0);
                    let y = self.y + 6.0;
                    self.page().ops.push(Op::Line { x1: MARGIN_X, y1: y, x2: MARGIN_X + CONTENT_W, y2: y, width: 0.8, color: RULE });
                    self.y += 16.0;
                }
                Block::Image { src, alt, indent } => self.image(src, alt, *indent),
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

/// Bygger en PDF av Markdown-text.
pub fn write(markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<DocImage>) -> Result<Written, String> {
    let blocks = parse_blocks(markdown);
    let defaults = doc_defaults(markdown);
    let body = defaults.font_size.unwrap_or(BODY_SIZE);
    let base = match defaults.font_family.as_deref().map(classify_family) {
        Some(Family::Mono) => Face::Mono,
        _ => Face::Sans,
    };
    let mut lay = Layout {
        m: Measurer::new(),
        pages: Vec::new(),
        y: MARGIN_TOP,
        headings: Vec::new(),
        images: Vec::new(),
        resolve,
        missing: Vec::new(),
        body,
        scale: body / BODY_SIZE,
        base,
        align: Align::Left,
    };
    lay.new_page();
    lay.run(&blocks);

    let fonts: HashMap<Face, Font> = [Face::Sans, Face::SansMedium, Face::SansSemi, Face::Mono, Face::MonoMedium]
        .into_iter()
        .map(|f| (f, Font::new(krilla::Data::from(f.data()), 0).expect("inbyggt typsnitt")))
        .collect();

    let mut doc = Document::new();
    let total = lay.pages.len();
    let images = std::mem::take(&mut lay.images);
    for (pi, page_ops) in lay.pages.iter().enumerate() {
        let mut page = doc.start_page_with(PageSettings::from_wh(PAGE_W, PAGE_H).expect("A4"));
        let mut links = Vec::new();
        {
            let mut s = page.surface();
            for op in &page_ops.ops {
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
                }
            }
            // Sidfot med sidnummer.
            if total > 1 {
                let label = format!("{} av {}", pi + 1, total);
                let mut m = Measurer::new();
                let lw = m.width(Face::Sans, 8.5, &label);
                s.set_fill(Some(fill(MUTED)));
                s.draw_text(
                    Point::from_xy(PAGE_W - MARGIN_X - lw, PAGE_H - 36.0),
                    fonts[&Face::Sans].clone(),
                    8.5,
                    &label,
                    false,
                    TextDirection::Auto,
                );
            }
            s.finish();
        }
        for (x, y, w, h, url) in links {
            if let Some(rect) = Rect::from_xywh(x, y, w.max(1.0), h) {
                let target = krilla::annotation::Target::Action(krilla::action::Action::Link(krilla::action::LinkAction::new(url)));
                page.add_annotation(krilla::annotation::Annotation::new_link(krilla::annotation::LinkAnnotation::new(rect, target), None));
            }
        }
        page.finish();
    }

    // Bokmärken från rubrikerna (nivå 1–3), nästlade efter nivå.
    let mut outline = krilla::outline::Outline::new();
    let mut stack: Vec<(u8, krilla::outline::OutlineNode)> = Vec::new();
    for h in lay.headings.iter().filter(|h| h.level <= 3 && !h.text.is_empty()) {
        let dest = krilla::destination::XyzDestination::new(h.page, Point::from_xy(MARGIN_X, h.y));
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

    #[test]
    fn writes_a_pdf() {
        let md = "# Rubrik\n\nEn **fet** och *kursiv* rad med [länk](https://example.com) och `kod`.\n\n- ett\n- två\n    1. nästlad\n\n> citat\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n```\nkod\n```\n\n---\n\n![saknas](finns-inte.png)\n";
        let w = write(md, "Test", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF-"));
        assert_eq!(w.missing_images, vec!["finns-inte.png".to_owned()]);
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
        Layout {
            m: Measurer::new(),
            pages: Vec::new(),
            y: MARGIN_TOP,
            headings: Vec::new(),
            images: Vec::new(),
            resolve,
            missing: Vec::new(),
            body: BODY_SIZE,
            scale: 1.0,
            base: Face::Sans,
            align: Align::Left,
        }
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
