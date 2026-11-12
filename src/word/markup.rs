//! Markdown-analys och formateringsoperationer för dokumentläget.
//!
//! Ren logik utan gränssnitt. Texten är fortfarande Markdown (så som `docx::read` skriver
//! den); här läggs ett lager ovanpå som säger vilken stil varje byte har (`analyze`), vilken
//! formatering som gäller vid markören (`active`) och hur knapparna ändrar texten
//! (`toggle_inline`, `set_block`, `insert_table`). Alla offset är byteoffset och landar alltid
//! på teckengränser.
#![allow(dead_code)]

use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};

use crate::style::{div_open, is_div_close, is_span_close, parse_div_open, parse_span_open};
pub use crate::style::{Align, DocDefaults, SpanStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Normal,
    Heading(u8),
    Quote,
    Bullet,
    Numbered,
    Code,
    Table,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inline {
    Bold,
    Italic,
    Underline,
    Strike,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageRef {
    pub alt: String,
    pub src: String,
    /// Bredd i CSS-pixlar; bara satt för `<img width>` (`![alt](src)` ger `None`).
    pub width: Option<f32>,
    /// Höjd i CSS-pixlar; bara satt för `<img height>`.
    pub height: Option<f32>,
}

/// En bit av texten med enhetlig stil. Bitarna täcker hela texten i ordning, utan glapp.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub range: Range<usize>,
    pub block: Block,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
    /// Markdown-syntax som döljs: `**`, `# `, `](url)`, `<u>` …
    pub markup: bool,
    /// "- " / "1. " i början av ett listobjekt (`markup` är sann bara för "- ").
    pub list_marker: bool,
    pub image: Option<ImageRef>,
    /// `Some(N)` för objektmarkören `<!--k:N-->` (en osynlig bit med `markup: true`).
    pub object: Option<u32>,
    /// Sammanslaget stilvärde för alla omslutande `<span style="…">` (den inre vinner); `None` om ingen.
    pub span: Option<Arc<SpanStyle>>,
    /// Justering från omslutande `<div align>`; `Left` annars.
    pub align: Align,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub pieces: Vec<Piece>,
    /// Dokumentets grundformat från kommentaren `<!-- kladd: … -->` på första raden.
    pub defaults: DocDefaults,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Active {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub block: Block,
    /// Varje egenskap är `Some` bara om den är enhetlig över markeringen (eller vid markören).
    pub span: SpanStyle,
    /// Justeringen på markörens rad (markeringens början).
    pub align: Align,
}

/// En egenskap hos `<span style>` som `set_span_prop` ändrar. `None` tar bort egenskapen.
#[derive(Debug, Clone, PartialEq)]
pub enum SpanProp {
    FontFamily(Option<String>),
    FontSize(Option<f32>),
    Color(Option<[u8; 3]>),
    Background(Option<[u8; 3]>),
}

impl SpanProp {
    fn apply(&self, s: &mut SpanStyle) {
        match self {
            SpanProp::FontFamily(v) => s.font_family = v.clone(),
            // Samma avrundning som `SpanStyle::to_css` skriver, så att jämförelser stämmer med texten.
            SpanProp::FontSize(v) => s.font_size = v.map(|x| (x * 2.0).round() / 2.0),
            SpanProp::Color(v) => s.color = *v,
            SpanProp::Background(v) => s.background = *v,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub text: String,
    pub sel: Range<usize>,
}

// ============================================================== analys

/// Stilen för en enskild byte; bitarna byggs av jämna löpningar.
#[derive(Clone, Copy, PartialEq)]
struct St {
    block: Block,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
    link: bool,
    markup: bool,
    list_marker: bool,
    image: Option<u32>,
    object: Option<u32>,
    span: Option<u32>,
    align: Align,
}

impl Default for St {
    fn default() -> Self {
        St {
            block: Block::Normal,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            code: false,
            link: false,
            markup: false,
            list_marker: false,
            image: None,
            object: None,
            span: None,
            align: Align::Left,
        }
    }
}

/// En formateringsspann med sina avgränsare (för att kunna ta bort eller flytta dem).
#[derive(Debug, Clone)]
struct Span {
    kind: Inline,
    open: Range<usize>,
    close: Range<usize>,
}

/// Ett parat `<span style>`-par med taggarnas lägen och stilen.
#[derive(Debug, Clone)]
struct HSpan {
    open: Range<usize>,
    close: Range<usize>,
    style: SpanStyle,
}

struct Parsed {
    pieces: Vec<Piece>,
    spans: Vec<Span>,
    hspans: Vec<HSpan>,
    defaults: DocDefaults,
}

/// Ger `s` ett löpnummer i tabellen (lika stilar delar nummer, så att bitar slås ihop rätt).
fn intern(styles: &mut Vec<Arc<SpanStyle>>, s: SpanStyle) -> u32 {
    if let Some(i) = styles.iter().position(|x| **x == s) {
        return i as u32;
    }
    styles.push(Arc::new(s));
    (styles.len() - 1) as u32
}

fn fill(st: &mut [St], r: Range<usize>, f: impl Fn(&mut St)) {
    let end = r.end.min(st.len());
    if r.start >= end {
        return;
    }
    for s in &mut st[r.start..end] {
        f(s);
    }
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Längden på en följd blanksteg/tabbar från `from`.
fn ws_run(b: &[u8], from: usize) -> usize {
    b.get(from..).map_or(0, |s| s.iter().take_while(|c| is_ws(**c)).count())
}

fn line_start(text: &str, p: usize) -> usize {
    text[..p].rfind('\n').map_or(0, |i| i + 1)
}

/// Slutet på raden (före `\n`).
fn line_end(text: &str, p: usize) -> usize {
    text[p..].find('\n').map_or(text.len(), |i| p + i)
}

fn parse(text: &str) -> Parsed {
    let n = text.len();
    let tb = text.as_bytes();
    let mut st = vec![St::default(); n];
    let mut spans: Vec<Span> = Vec::new();
    let mut images: Vec<ImageRef> = Vec::new();
    let mut lists: Vec<bool> = Vec::new(); // true = numrerad
    // Pågående länk: (område, typ, slutet på senaste barnet).
    let mut link: Option<(Range<usize>, LinkType, usize)> = None;
    let mut html_u: Vec<Range<usize>> = Vec::new();
    // Öppna `<span>`/`<div>` som väntar på sin stängning.
    let mut span_stack: Vec<(Range<usize>, SpanStyle)> = Vec::new();
    let mut hspans: Vec<HSpan> = Vec::new();
    let mut div_stack: Vec<(Range<usize>, Align)> = Vec::new();
    let mut divs: Vec<(Range<usize>, Range<usize>, Align)> = Vec::new();
    let mut defaults = DocDefaults::default();
    let mut image_depth = 0usize;
    let mut in_code_block = false;
    let mut last_text_end = 0usize;

    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for (ev, r) in Parser::new_ext(text, opts).into_offset_iter() {
        let r = r.start.min(n)..r.end.min(n);
        if image_depth > 0 {
            match &ev {
                Event::End(TagEnd::Image) => image_depth -= 1,
                Event::Start(Tag::Image { .. }) => image_depth += 1,
                Event::Text(t) | Event::Code(t) => {
                    if let Some(i) = images.last_mut() {
                        i.alt.push_str(t);
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if let Some(i) = images.last_mut() {
                        i.alt.push(' ');
                    }
                }
                _ => {}
            }
            continue;
        }
        if let Some(l) = link.as_mut() {
            if !matches!(ev, Event::End(TagEnd::Link)) {
                l.2 = l.2.max(r.end);
            }
        }
        match ev {
            Event::Start(tag) => {
                match &tag {
                    Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. } => {}
                    _ => {
                        html_u.clear();
                        span_stack.clear();
                    }
                }
                match tag {
                    Tag::Heading { level, .. } => {
                        let lvl = level as u8;
                        fill(&mut st, r.clone(), |s| s.block = Block::Heading(lvl));
                        mark_heading(text, &mut st, &r);
                    }
                    Tag::BlockQuote(_) => {
                        fill(&mut st, r.clone(), |s| s.block = Block::Quote);
                        mark_quote(text, &mut st, &r);
                    }
                    Tag::CodeBlock(kind) => {
                        in_code_block = true;
                        fill(&mut st, r.clone(), |s| s.block = Block::Code);
                        match kind {
                            CodeBlockKind::Fenced(_) => mark_fenced(text, &mut st, &r),
                            CodeBlockKind::Indented => fill(&mut st, r.clone(), |s| s.code = true),
                        }
                    }
                    Tag::List(start) => lists.push(start.is_some()),
                    Tag::Item => {
                        let numbered = lists.last().copied().unwrap_or(false);
                        let blk = if numbered { Block::Numbered } else { Block::Bullet };
                        // Indragen hör till objektet.
                        let ls = line_start(text, r.start);
                        let from = if tb[ls..r.start].iter().all(|c| is_ws(*c)) { ls } else { r.start };
                        fill(&mut st, from..r.end, |s| s.block = blk);
                        mark_item(text, &mut st, r.start);
                    }
                    Tag::Table(_) => {
                        fill(&mut st, r.clone(), |s| s.block = Block::Table);
                        mark_table(text, &mut st, &r);
                    }
                    Tag::Strong => push_span(&mut st, &mut spans, Inline::Bold, &r, 2),
                    Tag::Emphasis => push_span(&mut st, &mut spans, Inline::Italic, &r, 1),
                    Tag::Strikethrough => {
                        let d = tb[r.clone()].iter().take_while(|c| **c == b'~').count().min(2).max(1);
                        push_span(&mut st, &mut spans, Inline::Strike, &r, d);
                    }
                    Tag::Link { link_type, .. } => link = Some((r.clone(), link_type, r.start + 1)),
                    Tag::Image { dest_url, .. } => {
                        images.push(ImageRef { alt: String::new(), src: dest_url.to_string(), width: None, height: None });
                        let idx = (images.len() - 1) as u32;
                        fill(&mut st, r.clone(), |s| {
                            s.image = Some(idx);
                            s.markup = true;
                        });
                        image_depth = 1;
                    }
                    _ => {}
                }
            }
            Event::End(end) => {
                match &end {
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image => {}
                    _ => {
                        html_u.clear();
                        span_stack.clear();
                    }
                }
                match end {
                    TagEnd::CodeBlock => in_code_block = false,
                    TagEnd::List(_) => {
                        lists.pop();
                    }
                    TagEnd::Link => {
                        if let Some((lr, lt, last)) = link.take() {
                            finish_link(text, &mut st, &lr, lt, last);
                        }
                    }
                    _ => {}
                }
            }
            Event::Code(_) => {
                let d = tb[r.clone()].iter().take_while(|c| **c == b'`').count();
                if r.len() >= 2 * d {
                    fill(&mut st, r.start..r.start + d, |s| s.markup = true);
                    fill(&mut st, r.end - d..r.end, |s| s.markup = true);
                    fill(&mut st, r.start + d..r.end - d, |s| s.code = true);
                }
            }
            Event::InlineHtml(h) => {
                let t = h.trim();
                if let Some(img) = parse_img(t) {
                    images.push(img);
                    let idx = (images.len() - 1) as u32;
                    fill(&mut st, r.clone(), |s| {
                        s.image = Some(idx);
                        s.markup = true;
                    });
                } else if let Some(n) = parse_object(t) {
                    fill(&mut st, r.clone(), |s| {
                        s.object = Some(n);
                        s.markup = true;
                    });
                } else if t.eq_ignore_ascii_case("<u>") {
                    html_u.push(r.clone());
                } else if t.eq_ignore_ascii_case("</u>") {
                    if let Some(open) = html_u.pop() {
                        fill(&mut st, open.start..r.end, |s| s.underline = true);
                        fill(&mut st, open.clone(), |s| s.markup = true);
                        fill(&mut st, r.clone(), |s| s.markup = true);
                        spans.push(Span { kind: Inline::Underline, open, close: r.clone() });
                    }
                } else if let Some(style) = parse_span_open(t) {
                    span_stack.push((r.clone(), style));
                } else if is_span_close(t) {
                    if let Some((open, style)) = span_stack.pop() {
                        fill(&mut st, open.clone(), |s| s.markup = true);
                        fill(&mut st, r.clone(), |s| s.markup = true);
                        hspans.push(HSpan { open, close: r.clone(), style });
                    }
                }
            }
            Event::Html(_) => {
                // Hela HTML-blocket kan komma som en händelse: gå igenom det rad för rad.
                let mut off = r.start;
                for line in text[r.clone()].split_inclusive('\n') {
                    let ls = off;
                    off += line.len();
                    let t = line.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let ts = ls + (line.len() - line.trim_start().len());
                    let tr = ts..ts + t.len();
                    if t.matches('>').count() == 1 && parse_div_open(t).is_some() {
                        div_stack.push((tr, parse_div_open(t).unwrap_or_default()));
                    } else if is_div_close(t) {
                        if let Some((open, a)) = div_stack.pop() {
                            fill(&mut st, open.clone(), |s| s.markup = true);
                            fill(&mut st, tr.clone(), |s| s.markup = true);
                            divs.push((open, tr, a));
                        }
                    } else if let Some(toks) = html_tokens(t) {
                        // Rad med bara `<img …>` och/eller `<!--k:N-->`.
                        for (rel, tok) in toks {
                            let rr = tr.start + rel.start..tr.start + rel.end;
                            match tok {
                                HtmlTok::Img(img) => {
                                    images.push(img);
                                    let idx = (images.len() - 1) as u32;
                                    fill(&mut st, rr, |s| {
                                        s.image = Some(idx);
                                        s.markup = true;
                                    });
                                }
                                HtmlTok::Object(n) => fill(&mut st, rr, |s| {
                                    s.object = Some(n);
                                    s.markup = true;
                                }),
                            }
                        }
                    } else if t == super::nav::TOC_LINE {
                        // Innehållsförteckningen: en dold rad som dokumentläget ersätter med rubrikerna.
                        fill(&mut st, tr, |s| s.markup = true);
                    } else if ls == 0 {
                        if let Some(d) = DocDefaults::from_comment(t) {
                            defaults = d;
                            fill(&mut st, tr, |s| s.markup = true);
                        }
                    }
                }
            }
            Event::Text(t) => {
                if !in_code_block
                    && r.start >= 1
                    && tb[r.start - 1] == b'\\'
                    && r.start - 1 >= last_text_end
                    && t.chars().next().is_some_and(|c| c.is_ascii_punctuation())
                {
                    fill(&mut st, r.start - 1..r.start, |s| s.markup = true);
                }
                last_text_end = r.end;
            }
            Event::HardBreak => {
                if tb.get(r.start) == Some(&b'\\') {
                    fill(&mut st, r.start..r.start + 1, |s| s.markup = true);
                }
            }
            _ => {}
        }
    }

    // Spans och divar läggs på efter att alla händelser lästs: yttre först, så att den inre vinner.
    let mut styles: Vec<Arc<SpanStyle>> = Vec::new();
    hspans.sort_by_key(|h| h.open.start);
    for h in &hspans {
        let mut cache: Vec<(Option<u32>, u32)> = Vec::new();
        for i in h.open.end..h.close.start.min(n) {
            let old = st[i].span;
            let new = match cache.iter().find(|c| c.0 == old) {
                Some(c) => c.1,
                None => {
                    let base = old.map(|o| (*styles[o as usize]).clone()).unwrap_or_default();
                    let id = intern(&mut styles, base.merged(&h.style));
                    cache.push((old, id));
                    id
                }
            };
            st[i].span = Some(new);
        }
    }
    divs.sort_by_key(|d| d.0.start);
    for (open, close, a) in &divs {
        fill(&mut st, open.end..close.start, |s| s.align = *a);
    }

    // Bitar: sammanhängande tecken med samma stil, aldrig delade mitt i ett tecken.
    let mut pieces: Vec<Piece> = Vec::new();
    let mut cur: Option<(usize, St)> = None;
    for (i, _) in text.char_indices() {
        match cur {
            Some((_, s)) if s == st[i] => {}
            Some((start, s)) => {
                pieces.push(make_piece(start..i, &s, &images, &styles));
                cur = Some((i, st[i]));
            }
            None => cur = Some((i, st[i])),
        }
    }
    if let Some((start, s)) = cur {
        pieces.push(make_piece(start..n, &s, &images, &styles));
    }
    Parsed { pieces, spans, hspans, defaults }
}

fn make_piece(range: Range<usize>, s: &St, images: &[ImageRef], styles: &[Arc<SpanStyle>]) -> Piece {
    Piece {
        range,
        block: s.block,
        bold: s.bold,
        italic: s.italic,
        underline: s.underline,
        strike: s.strike,
        code: s.code,
        link: s.link,
        markup: s.markup,
        list_marker: s.list_marker,
        image: s.image.and_then(|i| images.get(i as usize).cloned()),
        object: s.object,
        span: s.span.and_then(|i| styles.get(i as usize).cloned()),
        align: s.align,
    }
}

fn push_span(st: &mut [St], spans: &mut Vec<Span>, kind: Inline, r: &Range<usize>, d: usize) {
    if r.len() < 2 * d {
        return;
    }
    fill(st, r.clone(), |s| match kind {
        Inline::Bold => s.bold = true,
        Inline::Italic => s.italic = true,
        Inline::Underline => s.underline = true,
        Inline::Strike => s.strike = true,
    });
    let open = r.start..r.start + d;
    let close = r.end - d..r.end;
    fill(st, open.clone(), |s| s.markup = true);
    fill(st, close.clone(), |s| s.markup = true);
    spans.push(Span { kind, open, close });
}

fn finish_link(text: &str, st: &mut [St], r: &Range<usize>, lt: LinkType, last: usize) {
    if r.len() < 2 {
        return;
    }
    if matches!(lt, LinkType::Autolink | LinkType::Email) {
        fill(st, r.start..r.start + 1, |s| s.markup = true);
        fill(st, r.end - 1..r.end, |s| s.markup = true);
        fill(st, r.start + 1..r.end - 1, |s| s.link = true);
        return;
    }
    if !text[r.clone()].starts_with('[') {
        return;
    }
    let last = last.clamp(r.start + 1, r.end);
    fill(st, r.start..r.start + 1, |s| s.markup = true);
    fill(st, r.start + 1..last, |s| s.link = true);
    fill(st, last..r.end, |s| s.markup = true);
}

fn mark_heading(text: &str, st: &mut [St], r: &Range<usize>) {
    let s = &text[r.clone()];
    let b = s.as_bytes();
    let indent = ws_run(b, 0);
    if b.get(indent) == Some(&b'#') {
        let hashes = b[indent..].iter().take_while(|c| **c == b'#').count();
        let open_end = indent + hashes + ws_run(b, indent + hashes);
        fill(st, r.start..r.start + open_end, |s| s.markup = true);
        // Avslutande ##.
        let line = s.find('\n').map_or(s, |i| &s[..i]).trim_end();
        let core = line.trim_end_matches('#');
        let k = core.len();
        if k < line.len() && k >= open_end.min(line.len()) {
            let before_ws = core.as_bytes().last().is_none_or(|c| is_ws(*c)) || k == open_end;
            if before_ws {
                let ws_start = core.trim_end_matches([' ', '\t']).len().max(open_end.min(k));
                fill(st, r.start + ws_start..r.start + line.len(), |s| s.markup = true);
            }
        }
    } else {
        // Setext: sista raden är understrykningen.
        let trimmed = s.trim_end_matches(['\n', '\r', ' ', '\t']);
        if let Some(i) = trimmed.rfind('\n') {
            fill(st, r.start + i + 1..r.start + trimmed.len(), |s| s.markup = true);
        }
    }
}

fn mark_quote(text: &str, st: &mut [St], r: &Range<usize>) {
    let mut pos = r.start;
    while pos < r.end {
        let line_end = text[pos..r.end].find('\n').map_or(r.end, |i| pos + i + 1);
        let b = text.as_bytes();
        let mut p = pos;
        loop {
            p += ws_run(b, p).min(line_end - p);
            if p < line_end && b[p] == b'>' {
                let mut q = p + 1;
                if q < line_end && b[q] == b' ' {
                    q += 1;
                }
                fill(st, p..q, |s| s.markup = true);
                p = q;
            } else {
                break;
            }
        }
        pos = line_end;
    }
}

fn mark_fenced(text: &str, st: &mut [St], r: &Range<usize>) {
    let s = &text[r.clone()];
    let first_end = s.find('\n').unwrap_or(s.len());
    let fence_line = s[..first_end].trim_start_matches([' ', '\t', '>']);
    let fch = fence_line.chars().next().unwrap_or('`');
    let flen = fence_line.chars().take_while(|c| *c == fch).count();
    fill(st, r.start..r.start + first_end, |s| s.markup = true);
    let trimmed = s.trim_end_matches(['\n', '\r', ' ', '\t']);
    let mut content_end = r.end;
    if let Some(i) = trimmed.rfind('\n') {
        if i >= first_end {
            let last = &trimmed[i + 1..];
            let core = last.trim_start_matches([' ', '\t', '>']);
            if core.len() >= 3 && core.len() >= flen && core.chars().all(|c| c == fch) {
                fill(st, r.start + i + 1..r.start + trimmed.len(), |s| s.markup = true);
                content_end = r.start + i + 1;
            }
        }
    }
    let content_start = (r.start + first_end + 1).min(content_end);
    fill(st, content_start..content_end, |s| s.code = true);
}

fn mark_item(text: &str, st: &mut [St], at: usize) {
    let b = text.as_bytes();
    // Ett nästlat objekts intervall kan börja på radbrytningen före (tabb) eller på radens indrag.
    let at = if b.get(at) == Some(&b'\n') { at + 1 } else { at };
    let at = at + ws_run(b, at);
    let Some(&c) = b.get(at) else { return };
    let (marker_len, bullet) = if matches!(c, b'-' | b'*' | b'+') {
        (1, true)
    } else if c.is_ascii_digit() {
        let d = b[at..].iter().take_while(|c| c.is_ascii_digit()).count();
        if matches!(b.get(at + d), Some(b'.') | Some(b')')) {
            (d + 1, false)
        } else {
            return;
        }
    } else {
        return;
    };
    let ws = ws_run(b, at + marker_len);
    let ws = if ws > 4 { 1 } else { ws };
    let end = at + marker_len + ws;
    fill(st, at..end, |s| {
        s.list_marker = true;
        s.markup = bullet;
    });
}

fn mark_table(text: &str, st: &mut [St], r: &Range<usize>) {
    let b = text.as_bytes();
    let mut pos = r.start;
    let mut idx = 0;
    while pos < r.end {
        let le = text[pos..r.end].find('\n').map_or(r.end, |i| pos + i);
        let mut p = pos;
        while p < le && (is_ws(b[p]) || b[p] == b'>') {
            p += 1;
        }
        if idx == 1 {
            fill(st, p..le, |s| s.markup = true);
        } else {
            let mut in_code = false;
            let mut i = p;
            while i < le {
                match b[i] {
                    b'`' => in_code = !in_code,
                    b'|' if !in_code && (i == 0 || b[i - 1] != b'\\') => fill(st, i..i + 1, |s| s.markup = true),
                    _ => {}
                }
                i += 1;
            }
        }
        idx += 1;
        pos = le + 1;
    }
}

pub fn analyze(text: &str) -> Analysis {
    let p = parse(text);
    Analysis { pieces: p.pieces, defaults: p.defaults }
}

// ============================================================== bilder och objektmarkörer (HTML)

enum HtmlTok {
    Img(ImageRef),
    Object(u32),
}

/// Slutet (exklusivt) på taggen som börjar på `s[0]`: första `>` utanför citattecken.
fn tag_end(s: &str) -> Option<usize> {
    let mut q: Option<u8> = None;
    for (i, c) in s.bytes().enumerate() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => q = Some(c),
            (None, b'>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// Attributen i en tagg som (namn med gemener, värde): med eller utan citattecken, i valfri ordning.
fn html_attrs(tag: &str) -> Vec<(String, String)> {
    let b = tag.as_bytes();
    let mut i = tag.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(tag.len());
    let mut out = Vec::new();
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() || b[i] == b'>' {
            break;
        }
        let ns = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        if i == ns {
            i += 1;
            continue;
        }
        let name = tag[ns..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i) != Some(&b'=') {
            out.push((name, String::new()));
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let val = match b.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                let vs = i + 1;
                let ve = tag[vs..].find(q as char).map_or(tag.len(), |k| vs + k);
                i = (ve + 1).min(tag.len());
                &tag[vs..ve]
            }
            _ => {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                // `<img src=x/>`: avslutande snedstrecket hör inte till värdet.
                if i < b.len() && b[i] == b'>' && i > vs && b[i - 1] == b'/' {
                    &tag[vs..i - 1]
                } else {
                    &tag[vs..i]
                }
            }
        };
        out.push((name, decode_entities(val)));
    }
    out
}

fn decode_entities(v: &str) -> String {
    if !v.contains('&') {
        return v.to_string();
    }
    v.replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn encode_attr(v: &str) -> String {
    v.replace('&', "&amp;").replace('"', "&quot;")
}

/// "240" eller "240px" till CSS-pixlar. Procent och annat ger `None`.
fn px_value(v: &str) -> Option<f32> {
    let t = v.trim().to_ascii_lowercase();
    let t = t.strip_suffix("px").unwrap_or(&t).trim();
    t.parse::<f32>().ok().filter(|x| x.is_finite() && *x > 0.0)
}

/// Läser en hel `<img …>`-tagg. Kräver `src`.
fn parse_img(tag: &str) -> Option<ImageRef> {
    if !tag.get(..4)?.eq_ignore_ascii_case("<img") || tag_end(tag)? != tag.len() {
        return None;
    }
    if !tag[4..].starts_with(|c: char| c.is_whitespace() || c == '/' || c == '>') {
        return None;
    }
    let attrs = html_attrs(tag);
    let get = |n: &str| attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    Some(ImageRef {
        src: get("src")?,
        alt: get("alt").unwrap_or_default(),
        width: get("width").and_then(|v| px_value(&v)),
        height: get("height").and_then(|v| px_value(&v)),
    })
}

/// `<!--k:N-->` exakt (siffror, inga blanksteg).
fn parse_object(tag: &str) -> Option<u32> {
    let d = tag.strip_prefix("<!--k:")?.strip_suffix("-->")?;
    if d.is_empty() || !d.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    d.parse().ok()
}

/// En rad som bara består av `<img …>` och/eller `<!--k:N-->` (med blanksteg emellan):
/// bitarna och deras lägen relativt radens början. `None` om något annat finns på raden.
fn html_tokens(line: &str) -> Option<Vec<(Range<usize>, HtmlTok)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        if rest.starts_with(char::is_whitespace) {
            i += rest.len() - rest.trim_start().len();
            continue;
        }
        let end = if rest.starts_with("<!--") { rest.find("-->")? + 3 } else { tag_end(rest)? };
        let tag = &rest[..end];
        let tok = match parse_object(tag) {
            Some(n) => HtmlTok::Object(n),
            None => HtmlTok::Img(parse_img(tag)?),
        };
        out.push((i..i + end, tok));
        i += end;
    }
    (!out.is_empty()).then_some(out)
}

/// `![alt](src)`; `src` med blanksteg eller parenteser skrivs som `<src>`.
fn md_image(img: &ImageRef) -> String {
    let alt: String = img.alt.chars().flat_map(|c| if matches!(c, '[' | ']' | '\\') { vec!['\\', c] } else { vec![c] }).collect();
    let src = if img.src.contains(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        format!("<{}>", img.src)
    } else {
        img.src.clone()
    };
    format!("![{alt}]({src})")
}

/// Bilden vid `pos`: biten som innehåller `pos`, annars biten som slutar där (markören direkt efter).
pub fn image_at(text: &str, a: &Analysis, pos: usize) -> Option<(Range<usize>, ImageRef)> {
    let pos = floor_boundary(text, pos);
    let hit = |p: &Piece| p.image.clone().map(|i| (p.range.clone(), i));
    if let Some(h) = piece_at(a, pos).and_then(hit) {
        return Some(h);
    }
    let (prev, _) = text[..pos].char_indices().next_back()?;
    piece_at(a, prev).and_then(hit)
}

/// Ändrar bildens bredd: `Some(w)` skriver `<img src alt width>` (hela pixlar; en uttryckligen
/// satt höjd skalas med, annars utelämnas den så att proportionerna bevaras), `None` skriver
/// tillbaka `![alt](src)` (höjden försvinner). `image_range` är bitens område; träffar det
/// ingen bild, eller blir resultatet detsamma, lämnas texten orörd. Markeringen blir bilden.
pub fn set_image_size(text: &str, image_range: Range<usize>, width: Option<f32>) -> Edit {
    let keep = || Edit { text: text.to_string(), sel: image_range.clone() };
    let a = analyze(text);
    let Some(img) = a.pieces.iter().find(|p| p.range == image_range).and_then(|p| p.image.clone()) else {
        return keep();
    };
    let new = match width {
        Some(w) if w.is_finite() && w >= 1.0 => {
            let w = w.round();
            let h = img.height.map(|h| match img.width {
                Some(ow) if ow > 0.0 => (h * w / ow).round().max(1.0),
                _ => h.round().max(1.0),
            });
            let mut s = format!("<img src=\"{}\" alt=\"{}\" width=\"{}\"", encode_attr(&img.src), encode_attr(&img.alt), w);
            if let Some(h) = h {
                s.push_str(&format!(" height=\"{h}\""));
            }
            s.push('>');
            s
        }
        Some(_) => return keep(),
        None if text[image_range.clone()].starts_with("![") => return keep(),
        None => md_image(&img),
    };
    let mut t = String::with_capacity(text.len() + new.len());
    t.push_str(&text[..image_range.start]);
    t.push_str(&new);
    t.push_str(&text[image_range.end..]);
    Edit { text: t, sel: image_range.start..image_range.start + new.len() }
}

// ============================================================== aktiv formatering

fn piece_index(a: &Analysis, pos: usize) -> Option<usize> {
    let i = a.pieces.partition_point(|p| p.range.end <= pos);
    a.pieces.get(i).filter(|p| p.range.start <= pos).map(|_| i)
}

fn piece_at(a: &Analysis, pos: usize) -> Option<&Piece> {
    piece_index(a, pos).map(|i| &a.pieces[i])
}

/// Blocket för raden som börjar på `ls`: blocket för första tecknet som inte är blanksteg.
fn block_of_line(a: &Analysis, text: &str, ls: usize) -> Block {
    let line = &text[ls..line_end(text, ls)];
    match line.find(|c: char| !c.is_whitespace()) {
        None => Block::Normal,
        Some(o) => piece_at(a, ls + o).map_or(Block::Normal, |p| p.block),
    }
}

fn floor_boundary(text: &str, p: usize) -> usize {
    let mut p = p.min(text.len());
    while !text.is_char_boundary(p) {
        p -= 1;
    }
    p
}

fn norm_sel(text: &str, sel: &Range<usize>) -> (usize, usize) {
    let s = floor_boundary(text, sel.start);
    let e = floor_boundary(text, sel.end);
    if s <= e { (s, e) } else { (e, s) }
}

fn flag_of(p: &Piece, m: Inline) -> bool {
    match m {
        Inline::Bold => p.bold,
        Inline::Italic => p.italic,
        Inline::Underline => p.underline,
        Inline::Strike => p.strike,
    }
}

fn is_content(p: &Piece) -> bool {
    !p.markup && !p.list_marker
}

pub fn active(text: &str, a: &Analysis, sel: Range<usize>) -> Active {
    let (s, e) = norm_sel(text, &sel);
    let ls = line_start(text, s);
    let block = block_of_line(a, text, ls);
    let mut r = Active {
        bold: false,
        italic: false,
        underline: false,
        strike: false,
        block,
        span: SpanStyle::default(),
        align: piece_at(a, ls).map_or(Align::Left, |p| p.align),
    };
    let all = [Inline::Bold, Inline::Italic, Inline::Underline, Inline::Strike];
    let set = |r: &mut Active, m: Inline, v: bool| match m {
        Inline::Bold => r.bold = v,
        Inline::Italic => r.italic = v,
        Inline::Underline => r.underline = v,
        Inline::Strike => r.strike = v,
    };
    if s == e {
        let left = text[..s].char_indices().next_back().and_then(|(i, _)| piece_at(a, i));
        let right = if s < text.len() { piece_at(a, s) } else { None };
        let src = match (left, right) {
            (Some(l), _) if is_content(l) => Some(l),
            (_, Some(r)) if is_content(r) => Some(r),
            _ => None,
        };
        if let Some(p) = src {
            for m in all {
                set(&mut r, m, flag_of(p, m));
            }
            r.span = p.span.as_deref().cloned().unwrap_or_default();
        }
        return r;
    }
    let mut seen = false;
    let mut flags = [true; 4];
    let mut span: Option<SpanStyle> = None;
    for p in &a.pieces {
        if p.range.end <= s {
            continue;
        }
        if p.range.start >= e {
            break;
        }
        if !is_content(p) {
            continue;
        }
        let lo = p.range.start.max(s);
        let hi = p.range.end.min(e);
        if text[lo..hi].chars().all(char::is_whitespace) {
            continue;
        }
        seen = true;
        for (i, m) in all.iter().enumerate() {
            flags[i] &= flag_of(p, *m);
        }
        let sty = p.span.as_deref().cloned().unwrap_or_default();
        span = Some(match span {
            None => sty,
            Some(u) => SpanStyle {
                font_family: if u.font_family == sty.font_family { u.font_family } else { None },
                font_size: if u.font_size == sty.font_size { u.font_size } else { None },
                color: if u.color == sty.color { u.color } else { None },
                background: if u.background == sty.background { u.background } else { None },
            },
        });
    }
    if let Some(u) = span {
        r.span = u;
    }
    if seen {
        for (i, m) in all.iter().enumerate() {
            set(&mut r, *m, flags[i]);
        }
    }
    r
}

// ============================================================== redigering

enum Part {
    Copy(Range<usize>),
    Lit(String),
}

/// Bygger en ny text av ändringar (`område` → delar) och räknar om markeringen:
/// `from` flyttas framåt förbi inskjutna tecken, `to` bakåt.
fn apply(text: &str, mut edits: Vec<(Range<usize>, Vec<Part>)>, from: usize, to: usize) -> Edit {
    edits.sort_by_key(|e| e.0.start);
    let mut out = String::new();
    let mut copies: Vec<(usize, usize, usize)> = Vec::new();
    fn copy(text: &str, out: &mut String, copies: &mut Vec<(usize, usize, usize)>, r: Range<usize>) {
        if r.is_empty() {
            return;
        }
        copies.push((r.start, r.end, out.len()));
        out.push_str(&text[r]);
    }
    let mut pos = 0;
    for (r, parts) in edits {
        if r.start < pos {
            continue;
        }
        copy(text, &mut out, &mut copies, pos..r.start);
        for part in parts {
            match part {
                Part::Copy(c) => copy(text, &mut out, &mut copies, c),
                Part::Lit(l) => out.push_str(&l),
            }
        }
        pos = r.end;
    }
    copy(text, &mut out, &mut copies, pos..text.len());
    let start = copies
        .iter()
        .find(|c| from < c.1)
        .map_or(out.len(), |c| c.2 + from.saturating_sub(c.0));
    let end = copies
        .iter()
        .rev()
        .find(|c| c.0 < to)
        .map_or(0, |c| c.2 + to.min(c.1) - c.0);
    Edit { text: out, sel: start..end.max(start) }
}

fn delims(m: Inline) -> (&'static str, &'static str) {
    match m {
        Inline::Bold => ("**", "**"),
        Inline::Italic => ("*", "*"),
        Inline::Underline => ("<u>", "</u>"),
        Inline::Strike => ("~~", "~~"),
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '-'
}

fn word_around(text: &str, p: usize) -> Option<Range<usize>> {
    let l = text[..p].chars().next_back().is_some_and(is_word);
    let r = text[p..].chars().next().is_some_and(is_word);
    if !l && !r {
        return None;
    }
    let mut s = p;
    for (i, c) in text[..p].char_indices().rev() {
        if is_word(c) {
            s = i;
        } else {
            break;
        }
    }
    let mut e = p;
    for (i, c) in text[p..].char_indices() {
        if is_word(c) {
            e = p + i + c.len_utf8();
        } else {
            break;
        }
    }
    Some(s..e)
}

fn trim_range(text: &str, r: Range<usize>) -> Range<usize> {
    let s = &text[r.clone()];
    let t = s.trim_start();
    let start = r.start + (s.len() - t.len());
    let t = t.trim_end();
    start..start + t.len()
}

/// `open` + innehåll + `close`, med blanksteg i kanterna utanför avgränsarna
/// (annars tolkas inte `**a **` som fetstil). Tomt eller bara blanksteg ger ingen formatering.
fn wrap_parts(text: &str, open: &str, close: &str, r: Range<usize>) -> Vec<Part> {
    if r.is_empty() {
        return vec![];
    }
    let core = trim_range(text, r.clone());
    if core.is_empty() {
        return vec![Part::Copy(r)];
    }
    let mut v = Vec::new();
    if r.start < core.start {
        v.push(Part::Copy(r.start..core.start));
    }
    v.push(Part::Lit(open.to_string()));
    v.push(Part::Copy(core.clone()));
    v.push(Part::Lit(close.to_string()));
    if core.end < r.end {
        v.push(Part::Copy(core.end..r.end));
    }
    v
}

/// Längden på radens blockprefix (indrag, `> `, `# `, `- `, `1. `, `[ ] `).
fn prefix_len(line: &str) -> usize {
    let mut p = 0;
    while p < line.len() {
        let pre = parse_prefix(&line[p..]);
        if pre.kind.is_none() || pre.end == 0 {
            break;
        }
        p += pre.end;
    }
    let rest = &line[p..];
    if (rest.starts_with("[ ] ") || rest.starts_with("[x] ") || rest.starts_with("[X] ")) && p > 0 {
        p += 4;
    }
    p
}

struct Prefix {
    /// Byte där prefixet slutar (inklusive indrag).
    end: usize,
    indent: usize,
    kind: Option<Block>,
}

fn parse_prefix(line: &str) -> Prefix {
    let b = line.as_bytes();
    let indent = ws_run(b, 0);
    let rb = &b[indent..];
    let none = Prefix { end: indent, indent, kind: None };
    let Some(&first) = rb.first() else { return none };
    if first == b'>' {
        let e = if rb.get(1) == Some(&b' ') { 2 } else { 1 };
        return Prefix { end: indent + e, indent, kind: Some(Block::Quote) };
    }
    if first == b'#' {
        let h = rb.iter().take_while(|c| **c == b'#').count();
        if h <= 6 && (h == rb.len() || is_ws(rb[h])) {
            return Prefix { end: indent + h + ws_run(rb, h), indent, kind: Some(Block::Heading(h as u8)) };
        }
        return none;
    }
    if matches!(first, b'-' | b'*' | b'+') && (rb.len() == 1 || is_ws(rb[1])) {
        let ws = ws_run(rb, 1);
        let ws = if ws > 4 { 1 } else { ws };
        return Prefix { end: indent + 1 + ws, indent, kind: Some(Block::Bullet) };
    }
    let d = rb.iter().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&d) && matches!(rb.get(d), Some(b'.') | Some(b')')) && (rb.len() == d + 1 || is_ws(rb[d + 1])) {
        let ws = ws_run(rb, d + 1);
        let ws = if ws > 4 { 1 } else { ws };
        return Prefix { end: indent + d + 1 + ws, indent, kind: Some(Block::Numbered) };
    }
    none
}

fn is_table_delim_row(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

pub fn toggle_inline(text: &str, sel: Range<usize>, m: Inline) -> Edit {
    let (mut s, mut e) = norm_sel(text, &sel);
    let (open, close) = delims(m);
    if s == e {
        if let Some(w) = word_around(text, s) {
            s = w.start;
            e = w.end;
        } else {
            let ch = open.chars().next().unwrap();
            let single = m != Inline::Underline;
            let in_pair = text[..s].ends_with(open)
                && text[s..].starts_with(close)
                && !(single
                    && (text[..s - open.len()].ends_with(ch) || text[s + close.len()..].starts_with(ch)));
            if in_pair {
                let mut t = String::with_capacity(text.len());
                t.push_str(&text[..s - open.len()]);
                t.push_str(&text[s + close.len()..]);
                let c = s - open.len();
                return Edit { text: t, sel: c..c };
            }
            let mut t = String::with_capacity(text.len() + open.len() + close.len());
            t.push_str(&text[..s]);
            t.push_str(open);
            t.push_str(close);
            t.push_str(&text[s..]);
            let c = s + open.len();
            return Edit { text: t, sel: c..c };
        }
    }
    let parsed = parse(text);
    let a = Analysis { pieces: parsed.pieces.clone(), defaults: parsed.defaults.clone() };
    let act = active(text, &a, s..e);
    let on = match m {
        Inline::Bold => act.bold,
        Inline::Italic => act.italic,
        Inline::Underline => act.underline,
        Inline::Strike => act.strike,
    };
    if on {
        remove_fmt(text, &parsed, m, s, e)
    } else {
        add_fmt(text, &parsed, &a, m, s, e)
    }
}

fn remove_fmt(text: &str, parsed: &Parsed, m: Inline, s: usize, e: usize) -> Edit {
    let (open, close) = delims(m);
    let mut edits = Vec::new();
    let mut last_end = 0;
    let mut spans: Vec<&Span> = parsed.spans.iter().filter(|sp| sp.kind == m).collect();
    spans.sort_by_key(|sp| sp.open.start);
    for sp in spans {
        let c = sp.open.end..sp.close.start;
        if !(c.start < e && s < c.end) || sp.open.start < last_end {
            continue;
        }
        let ss = s.max(c.start);
        let ee = e.min(c.end);
        let mut parts = wrap_parts(text, open, close, c.start..ss);
        parts.push(Part::Copy(ss..ee));
        parts.extend(wrap_parts(text, open, close, ee..c.end));
        last_end = sp.close.end;
        edits.push((sp.open.start..sp.close.end, parts));
    }
    apply(text, edits, s, e)
}

/// Innehållet som en formatering på `s..e` berör: en trimmad bit per rad (per cell i tabeller),
/// efter blockprefix och utan kodrader och tabellens avgränsarrad.
fn content_segments(text: &str, a: &Analysis, s: usize, e: usize) -> Vec<Range<usize>> {
    let mut segs: Vec<Range<usize>> = Vec::new();
    let mut ls = line_start(text, s);
    while ls <= e {
        let le = line_end(text, ls);
        let lo = s.max(ls);
        let hi = e.min(le);
        if lo < hi {
            let line = &text[ls..le];
            let block = block_of_line(a, text, ls);
            if block != Block::Code {
                let lo = lo.max(ls + prefix_len(line));
                if lo < hi {
                    let mut ranges = Vec::new();
                    if block == Block::Table {
                        if !is_table_delim_row(line) {
                            let mut st = lo;
                            for (i, c) in text[lo..hi].char_indices() {
                                if c == '|' {
                                    ranges.push(st..lo + i);
                                    st = lo + i + 1;
                                }
                            }
                            ranges.push(st..hi);
                        }
                    } else {
                        ranges.push(lo..hi);
                    }
                    for r in ranges {
                        let t = trim_range(text, r);
                        if !t.is_empty() {
                            segs.push(t);
                        }
                    }
                }
            }
        }
        if le >= text.len() {
            break;
        }
        ls = le + 1;
    }
    segs
}

fn add_fmt(text: &str, parsed: &Parsed, a: &Analysis, m: Inline, s: usize, e: usize) -> Edit {
    let (open, close) = delims(m);
    let mut segs = content_segments(text, a, s, e);
    if segs.is_empty() {
        return Edit { text: text.to_string(), sel: s..e };
    }
    // Spann av samma slag som sticker ut ur ett segment dras med (som i Word).
    let same: Vec<&Span> = parsed.spans.iter().filter(|sp| sp.kind == m).collect();
    for seg in &mut segs {
        loop {
            let mut changed = false;
            for sp in &same {
                let (os, oe) = (sp.open.start, sp.close.end);
                if os < seg.end && seg.start < oe {
                    if os < seg.start {
                        seg.start = os;
                        changed = true;
                    }
                    if oe > seg.end {
                        seg.end = oe;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    segs.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in segs {
        match merged.last_mut() {
            Some(l) if r.start < l.end => l.end = l.end.max(r.end),
            _ => merged.push(r),
        }
    }
    let mut edits = Vec::new();
    for seg in &merged {
        let mut dels: Vec<Range<usize>> = Vec::new();
        for sp in &same {
            if sp.open.start >= seg.start && sp.close.end <= seg.end {
                dels.push(sp.open.clone());
                dels.push(sp.close.clone());
            }
        }
        dels.sort_by_key(|d| d.start);
        let mut parts = vec![Part::Lit(open.to_string())];
        let mut cur = seg.start;
        for d in dels {
            if d.start >= cur {
                parts.push(Part::Copy(cur..d.start));
                cur = d.end;
            }
        }
        parts.push(Part::Copy(cur..seg.end));
        parts.push(Part::Lit(close.to_string()));
        edits.push((seg.clone(), parts));
    }
    let from = merged[0].start;
    let to = merged.last().unwrap().end;
    apply(text, edits, from, to)
}

pub fn set_block(text: &str, sel: Range<usize>, b: Block) -> Edit {
    let (s, e) = norm_sel(text, &sel);
    if matches!(b, Block::Code | Block::Table) {
        return Edit { text: text.to_string(), sel: s..e };
    }
    let a = analyze(text);
    // Rader som markeringen berör.
    let mut lines: Vec<usize> = Vec::new();
    let mut ls = line_start(text, s);
    loop {
        if ls > s && ls >= e && e > s && ls == e {
            break;
        }
        lines.push(ls);
        let le = line_end(text, ls);
        if le >= text.len() || le >= e {
            break;
        }
        ls = le + 1;
    }
    let eligible: Vec<(usize, Block)> = lines
        .into_iter()
        .filter_map(|ls| {
            let line = &text[ls..line_end(text, ls)];
            let t = line.trim();
            if t.is_empty() {
                return None;
            }
            let blk = block_of_line(&a, text, ls);
            if matches!(blk, Block::Code | Block::Table) {
                return None;
            }
            // Setext-understrykning är inte en egen rad att göra om.
            if matches!(blk, Block::Heading(_)) && (t.chars().all(|c| c == '=') || t.chars().all(|c| c == '-')) {
                return None;
            }
            Some((ls, blk))
        })
        .collect();
    if eligible.is_empty() {
        return Edit { text: text.to_string(), sel: s..e };
    }
    let target = if eligible.iter().all(|(_, blk)| *blk == b) { Block::Normal } else { b };
    let target_list = matches!(target, Block::Bullet | Block::Numbered);
    let mut counter = 0;
    let mut edits = Vec::new();
    for (ls, _) in &eligible {
        let le = line_end(text, *ls);
        let pre = parse_prefix(&text[*ls..le]);
        let old_list = matches!(pre.kind, Some(Block::Bullet | Block::Numbered));
        let marker = match target {
            Block::Heading(n) => format!("{} ", "#".repeat(n as usize)),
            Block::Quote => "> ".to_string(),
            Block::Bullet => "- ".to_string(),
            Block::Numbered => {
                counter += 1;
                format!("{counter}. ")
            }
            _ => String::new(),
        };
        let (range, new) = if pre.kind.is_some() {
            let keep = if old_list && target_list { text[*ls..*ls + pre.indent].to_string() } else { String::new() };
            (*ls..*ls + pre.end, format!("{keep}{marker}"))
        } else {
            (*ls + pre.indent..*ls + pre.indent, marker)
        };
        edits.push((range, vec![Part::Lit(new)]));
    }
    let mut r = apply(text, edits, s, e);
    if s == e {
        r.sel = r.sel.start..r.sel.start;
    }
    r
}

pub fn insert_table(text: &str, at: usize) -> Edit {
    let at = floor_boundary(text, at);
    let before = &text[..at];
    let after = &text[at..];
    let lead = if before.is_empty() || before.ends_with("\n\n") {
        ""
    } else if before.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let trail = if after.is_empty() || after.starts_with('\n') { "" } else { "\n" };
    let head = "| Kolumn 1 | Kolumn 2 |\n| --- | --- |\n";
    let snippet = format!("{lead}{head}|  |  |\n{trail}");
    let caret = at + lead.len() + head.len() + 2;
    let mut t = String::with_capacity(text.len() + snippet.len());
    t.push_str(before);
    t.push_str(&snippet);
    t.push_str(after);
    Edit { text: t, sel: caret..caret }
}

// ============================================================== tabellmodell

/// En Markdown-tabell som rutnät, för redigering i dokumentläget.
///
/// Alla områden är byteoffset i texten. Rutnätet har rubrikraden först (`rows[0]`) och sedan
/// brödraderna; avgränsningsraden (`---`) ingår inte. Rader kan ha färre eller fler celler än
/// rubriken (som i GFM); `cols` är antalet rubrikceller. Tabeller utan yttre `|` och med
/// escapade `\|` i cellerna förstås. Tabeller i citat (`>`) stöds inte.
#[derive(Debug, Clone, PartialEq)]
pub struct TableInfo {
    /// Från början av rubrikradens rad till slutet av sista radens text, utan det sista
    /// radslutet. En markör på någon rad i tabellen, även precis före dess `\n`, räknas dit.
    pub range: Range<usize>,
    /// Cellernas innehåll (utan blanksteg runt). En tom cell får ett tomt område där text
    /// skrivs in: efter det första blanksteget i `|  |`, annars direkt efter `|`.
    pub rows: Vec<Vec<Range<usize>>>,
    /// Avgränsningsradens rad.
    pub delim_row: Range<usize>,
    pub cols: usize,
    /// Hela raden (utan `\n`) för varje rad i `rows`.
    pub row_lines: Vec<Range<usize>>,
    /// Hela cellen (mellan två `|`, med blanksteg) för varje cell i `rows`.
    pub cell_spans: Vec<Vec<Range<usize>>>,
}

/// En rad i en tabell, delad i celler.
struct TLine {
    start: usize,
    end: usize,
    /// Texten utan blanksteg i början och slutet (`c0..c1`).
    c0: usize,
    c1: usize,
    /// Börjar raden med `|` / slutar den med `|`?
    lead: bool,
    trail: bool,
    segs: Vec<Range<usize>>,
}

fn parse_tline(text: &str, start: usize, end: usize) -> TLine {
    let b = text.as_bytes();
    let mut c1 = end;
    while c1 > start && matches!(b[c1 - 1], b' ' | b'\t' | b'\r') {
        c1 -= 1;
    }
    let mut c0 = start;
    while c0 < c1 && is_ws(b[c0]) {
        c0 += 1;
    }
    let mut pipes = Vec::new();
    let mut i = c0;
    while i < c1 {
        match b[i] {
            b'\\' => i += 1,
            b'|' => pipes.push(i),
            _ => {}
        }
        i += 1;
    }
    let lead = pipes.first() == Some(&c0);
    let trail = pipes.last() == Some(&(c1.wrapping_sub(1))) && !(lead && pipes.len() == 1);
    let mut segs = Vec::new();
    let mut prev = if lead { c0 + 1 } else { c0 };
    for &p in pipes.iter().skip(lead as usize) {
        segs.push(prev..p);
        prev = p + 1;
    }
    if !trail && c1 > prev {
        segs.push(prev..c1);
    }
    TLine { start, end, c0, c1, lead, trail, segs }
}

/// Cellens redigerbara område: hela cellen minus ett blanksteg av utfyllnaden på varje sida.
fn seg_bounds(text: &str, seg: &Range<usize>) -> Range<usize> {
    let b = text.as_bytes();
    let s = seg.start + (seg.start < seg.end && b[seg.start] == b' ') as usize;
    let e = seg.end - (seg.end > s && b[seg.end - 1] == b' ') as usize;
    s..e.max(s)
}

fn seg_content(text: &str, seg: &Range<usize>) -> Range<usize> {
    let t = &text[seg.clone()];
    let lead = t.len() - t.trim_start_matches([' ', '\t']).len();
    let core = t.trim_matches([' ', '\t']);
    if core.is_empty() {
        let p = seg_bounds(text, seg).start;
        return p..p;
    }
    seg.start + lead..seg.start + lead + core.len()
}

/// Tabellens rader (rubrik, avgränsning, brödrader) och dess modell.
fn table_parse(text: &str, pos: usize) -> Option<(TableInfo, Vec<TLine>)> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for (ev, r) in Parser::new_ext(text, opts).into_offset_iter() {
        if !matches!(ev, Event::Start(Tag::Table(_))) {
            continue;
        }
        let first = line_start(text, r.start.min(text.len()));
        let end = text[..r.end.min(text.len())].trim_end_matches(['\n', '\r']).len();
        if pos < first || pos > end || end <= first {
            continue;
        }
        if text[first..].trim_start().starts_with('>') {
            return None;
        }
        let mut lines = Vec::new();
        let mut off = first;
        for l in text[first..end].split('\n') {
            lines.push(parse_tline(text, off, off + l.len()));
            off += l.len() + 1;
        }
        if lines.len() < 2 {
            return None;
        }
        let model: Vec<&TLine> = lines.iter().enumerate().filter(|(i, _)| *i != 1).map(|(_, l)| l).collect();
        let info = TableInfo {
            range: first..end,
            rows: model.iter().map(|l| l.segs.iter().map(|s| seg_content(text, s)).collect()).collect(),
            delim_row: lines[1].start..lines[1].end,
            cols: lines[0].segs.len(),
            row_lines: model.iter().map(|l| l.start..l.end).collect(),
            cell_spans: model.iter().map(|l| l.segs.clone()).collect(),
        };
        return Some((info, lines));
    }
    None
}

/// Tabellen som raden vid `pos` (byteoffset) ligger i.
pub fn table_at(text: &str, pos: usize) -> Option<TableInfo> {
    table_parse(text, floor_boundary(text, pos)).map(|t| t.0)
}

/// Kolumnen i `segs` för `pos`: cellen vars slut `pos` inte passerat; efter sista cellen den sista.
fn col_of(segs: &[Range<usize>], pos: usize) -> Option<usize> {
    if segs.is_empty() {
        return None;
    }
    Some(segs.iter().position(|s| pos <= s.end).unwrap_or(segs.len() - 1))
}

/// (rad, kolumn) i `rows` för markören; `None` utanför rubrik- och brödraderna (till exempel på
/// avgränsningsraden). Före första `|` räknas till första cellen, efter sista till sista.
pub fn table_cell_at(t: &TableInfo, pos: usize) -> Option<(usize, usize)> {
    let r = t.row_lines.iter().position(|l| l.start <= pos && pos <= l.end)?;
    Some((r, col_of(&t.cell_spans[r], pos)?))
}

/// Cellens redigerbara område vid `pos`: cellen minus ett blanksteg utfyllnad på varje sida
/// (så att blanksteg man skriver själv ryms). Vyn håller markören här och spärrar
/// Backspace/Delete vid kanterna. `None` utanför rubrik- och brödraderna.
pub fn table_cell_bounds(text: &str, pos: usize) -> Option<Range<usize>> {
    let pos = floor_boundary(text, pos);
    let t = table_at(text, pos)?;
    let (r, c) = table_cell_at(&t, pos)?;
    Some(seg_bounds(text, &t.cell_spans[r][c]))
}

/// Markörens läge i tabellen: modellens rad, radindex bland alla rader (0 rubrik, 1 avgränsning)
/// och kolumn. Avgränsningsraden räknas som rubrikraden.
struct TCtx {
    t: TableInfo,
    lines: Vec<TLine>,
    li: usize,
    row: usize,
    col: usize,
}

fn tctx(text: &str, pos: usize) -> Option<TCtx> {
    let pos = floor_boundary(text, pos);
    let (t, lines) = table_parse(text, pos)?;
    let li = lines.iter().position(|l| l.start <= pos && pos <= l.end)?;
    let col = col_of(&lines[li].segs, pos).unwrap_or(0);
    let row = if li <= 1 { 0 } else { li - 1 };
    Some(TCtx { t, lines, li, row, col })
}

fn unchanged(text: &str, pos: usize) -> Edit {
    let p = floor_boundary(text, pos);
    Edit { text: text.to_string(), sel: p..p }
}

/// Utför ersättningar (ordnade stigande, utan överlapp).
fn splice(text: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (r, s) in edits {
        out.push_str(&text[at..r.start]);
        out.push_str(s);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

/// Ny text med markeringen på cellen (rad, kolumn) i tabellen som börjar på `start`.
/// Finns inte cellen tas närmaste i raden; saknas raden hamnar markören på `fallback`.
fn with_cell_sel(new: String, start: usize, row: usize, col: usize, fallback: usize) -> Edit {
    let sel = table_at(&new, start).and_then(|t| {
        let r = t.rows.get(row.min(t.rows.len().saturating_sub(1)))?;
        r.get(col.min(r.len().saturating_sub(1))).cloned()
    });
    let fb = fallback.min(new.len());
    Edit { sel: sel.unwrap_or(fb..fb), text: new }
}

/// En tom rad `|  |  |` med `cols` celler, indragen som rubrikraden.
fn empty_row(text: &str, lines: &[TLine], cols: usize) -> String {
    format!("{}|{}", &text[lines[0].start..lines[0].c0], "  |".repeat(cols.max(1)))
}

/// Tab / Shift+Tab: nästa (föregående) cell. Texten är oförändrad och markeringen hela cellens
/// innehåll (tom i början av en tom cell) — utom efter sista cellen, då en ny tom rad läggs
/// till, och när en rad har färre celler än rubriken, då fylls den ut. Första cellen bakåt
/// stannar kvar. `None` utanför en tabell.
pub fn table_next_cell(text: &str, pos: usize, back: bool) -> Option<Edit> {
    let c = tctx(text, pos)?;
    let cols = c.t.cols.max(1);
    let nrows = c.t.rows.len();
    let (tr, tc) = if back {
        if c.col > 0 {
            (c.row, c.col - 1)
        } else if c.row > 0 {
            (c.row - 1, cols - 1)
        } else {
            (0, 0)
        }
    } else if c.col + 1 < cols {
        (c.row, c.col + 1)
    } else if c.row + 1 < nrows {
        (c.row + 1, 0)
    } else {
        return Some(table_insert_row(text, c.t.row_lines[nrows - 1].start, true));
    };
    let have = c.t.rows[tr].len();
    if tc >= have && have > 0 {
        // Raden är kortare än rubriken: fyll ut med tomma celler.
        let l = &c.lines[if tr == 0 { 0 } else { tr + 1 }];
        let pad = format!("{}{}", if l.trail { "" } else { " |" }, "  |".repeat(cols - have));
        let new = splice(text, &[(l.c1..l.c1, pad)]);
        return Some(with_cell_sel(new, c.t.range.start, tr, tc, pos));
    }
    let sel = c.t.rows[tr].get(tc).cloned().unwrap_or(pos..pos);
    Some(Edit { text: text.to_string(), sel })
}

/// Lägger till en tom rad ovanför eller under raden vid `pos`; markören hamnar i dess första cell.
/// På rubrik- och avgränsningsraden läggs den alltid som första brödrad (en rad kan inte
/// läggas ovanför rubriken). Utanför en tabell orört.
pub fn table_insert_row(text: &str, pos: usize, below: bool) -> Edit {
    let Some(c) = tctx(text, pos) else { return unchanged(text, pos) };
    let row = empty_row(text, &c.lines, c.t.cols);
    let (edit, new_row) = if c.li <= 1 {
        let e = c.lines[1].end;
        ((e..e, format!("\n{row}")), 1)
    } else if below {
        let e = c.lines[c.li].end;
        ((e..e, format!("\n{row}")), c.row + 1)
    } else {
        let s = c.lines[c.li].start;
        ((s..s, format!("{row}\n")), c.row)
    };
    let new = splice(text, &[edit]);
    with_cell_sel(new, c.t.range.start, new_row, 0, pos)
}

/// Lägger till en kolumn till vänster eller höger om markörens: rubrik "Kolumn N" (N = antalet
/// kolumner efteråt), `---` i avgränsningen och tomma celler. Övriga celler lämnas orörda.
/// Markören hamnar i den nya cellen på markörens rad. Utanför en tabell orört.
pub fn table_insert_col(text: &str, pos: usize, right: bool) -> Edit {
    let Some(c) = tctx(text, pos) else { return unchanged(text, pos) };
    let idx = if right { c.col + 1 } else { c.col };
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for (li, l) in c.lines.iter().enumerate() {
        let cell = match li {
            0 => format!("Kolumn {}", c.t.cols + 1),
            1 => "---".to_string(),
            _ => String::new(),
        };
        let m = l.segs.len();
        if m == 0 || idx > m {
            continue;
        }
        let e = if idx < m {
            let a = l.segs[idx].start;
            if idx == 0 && !l.lead {
                // Utan inledande `|` får raden ett, annars skulle den nya cellen bli en del av nästa.
                (a..a, format!("| {cell} | "))
            } else {
                (a..a, format!(" {cell} |"))
            }
        } else if l.trail {
            (l.c1..l.c1, format!(" {cell} |"))
        } else {
            (l.c1..l.c1, format!(" | {cell} |"))
        };
        edits.push(e);
    }
    let new = splice(text, &edits);
    with_cell_sel(new, c.t.range.start, c.row, idx, pos)
}

/// Tar bort raden vid `pos`. Rubrikraden: nästa rad blir rubrik (finns ingen tas tabellen
/// bort). Enda brödraden töms i stället för att tas bort. Markören hamnar i samma kolumn på
/// raden som tog platsen (eller den sista). Utanför en tabell orört.
pub fn table_delete_row(text: &str, pos: usize) -> Edit {
    let Some(c) = tctx(text, pos) else { return unchanged(text, pos) };
    let l = &c.lines;
    let edits = if c.li <= 1 {
        if l.len() < 3 {
            return table_delete(text, pos);
        }
        // Första brödraden flyttas upp till rubriken.
        vec![(l[0].start..l[0].end, text[l[2].start..l[2].end].to_string()), (l[2].start - 1..l[2].end, String::new())]
    } else if l.len() == 3 {
        vec![(l[2].start..l[2].end, empty_row(text, l, c.t.cols))]
    } else {
        vec![(l[c.li].start - 1..l[c.li].end, String::new())]
    };
    let new = splice(text, &edits);
    with_cell_sel(new, c.t.range.start, c.row, c.col, pos)
}

/// Tar bort kolumnen vid `pos` (alla rader inklusive avgränsningen). Är det den sista
/// kolumnen tas hela tabellen bort. Utanför en tabell orört.
pub fn table_delete_col(text: &str, pos: usize) -> Edit {
    let Some(c) = tctx(text, pos) else { return unchanged(text, pos) };
    if c.t.cols <= 1 {
        return table_delete(text, pos);
    }
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for l in &c.lines {
        let m = l.segs.len();
        if c.col >= m {
            continue;
        }
        let seg = &l.segs[c.col];
        edits.push(if m == 1 {
            // Radens enda cell: raden måste finnas kvar.
            (l.c0..l.c1, "|  |".to_string())
        } else if c.col + 1 < m || l.trail {
            (seg.start..seg.end + 1, String::new())
        } else {
            (seg.start - 1..l.c1, String::new())
        });
    }
    let new = splice(text, &edits);
    let cols = c.t.cols - 1;
    with_cell_sel(new, c.t.range.start, c.row, c.col.min(cols - 1), pos)
}

/// Tar bort hela tabellen (och en av de omgivande tomma raderna). Markören hamnar där den var.
/// Utanför en tabell orört.
pub fn table_delete(text: &str, pos: usize) -> Edit {
    let Some((t, _)) = table_parse(text, floor_boundary(text, pos)) else { return unchanged(text, pos) };
    let before = &text[..t.range.start];
    let mut after = &text[t.range.end..];
    after = after.strip_prefix("\r\n").or_else(|| after.strip_prefix('\n')).unwrap_or(after);
    while (before.is_empty() || before.ends_with("\n\n")) && after.starts_with('\n') {
        after = &after[1..];
    }
    Edit { text: format!("{before}{after}"), sel: before.len()..before.len() }
}

// ============================================================== spans (typsnitt, storlek, färg)

/// Delar som inte får få en taggräns mitt i sig: bilder, inline-kod (med backtecken),
/// länkadresser `](url)` och autolänkar.
fn atoms(text: &str, a: &Analysis) -> Vec<Range<usize>> {
    let ps = &a.pieces;
    let mut v = Vec::new();
    for (i, p) in ps.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| &ps[j]);
        let next = ps.get(i + 1);
        if p.image.is_some() {
            v.push(p.range.clone());
        } else if p.code && p.block != Block::Code {
            let mut r = p.range.clone();
            if let Some(pm) = prev.filter(|x| x.markup && text[x.range.clone()].ends_with('`')) {
                r.start = pm.range.start;
            }
            if let Some(nm) = next.filter(|x| x.markup && text[x.range.clone()].starts_with('`')) {
                r.end = nm.range.end;
            }
            v.push(r);
        } else if p.link {
            let auto_open = prev.filter(|x| x.markup && text[x.range.clone()].ends_with('<'));
            let auto_close = next.filter(|x| x.markup && text[x.range.clone()].starts_with('>'));
            if let (Some(pm), Some(nm)) = (auto_open, auto_close) {
                v.push(pm.range.start..nm.range.end);
            } else if let Some(nm) = next.filter(|x| x.markup && text[x.range.clone()].starts_with(']')) {
                v.push(nm.range.clone());
            }
        }
    }
    v
}

/// Blanksteg bakåt från `x` (högst ner till `floor`). Radbrytning passeras bara om raden
/// efter den saknar blockprefix, annars skulle prefixet hamna fel.
fn ws_before(text: &str, x: usize, floor: usize) -> usize {
    let b = text.as_bytes();
    let mut p = x;
    while p > floor {
        if is_ws(b[p - 1]) {
            p -= 1;
        } else if b[p - 1] == b'\n' && p - 1 > floor && prefix_len(&text[p..line_end(text, p)]) == 0 {
            p -= 1;
            if p > floor && b[p - 1] == b'\r' {
                p -= 1;
            }
        } else {
            break;
        }
    }
    p
}

/// Som [`ws_before`] men framåt, högst till `ceil`.
fn ws_after(text: &str, x: usize, ceil: usize) -> usize {
    let b = text.as_bytes();
    let mut p = x;
    while p < ceil {
        if is_ws(b[p]) || b[p] == b'\r' && b.get(p + 1) == Some(&b'\n') {
            p += 1;
        } else if b[p] == b'\n' && p + 1 < ceil && prefix_len(&text[p + 1..line_end(text, p + 1)]) == 0 {
            p += 1;
        } else {
            break;
        }
    }
    p
}

/// Sätter (eller tar bort) en teckenegenskap på markeringen genom att skriva om `<span style>`-taggarna
/// runt den. Spans som rör markeringen plattas ut till en följd av spans med enhetlig stil,
/// tomma spans försvinner och intilliggande spans med samma stil slås ihop. Spans läggs utanför
/// `**`, `*`, `<u>` och `~~`; skär en gräns genom en sådan stängs den och öppnas igen runt gränsen.
pub fn set_span_prop(text: &str, sel: Range<usize>, prop: SpanProp) -> Edit {
    let (mut s, mut e) = norm_sel(text, &sel);
    let unchanged = |s: usize, e: usize| Edit { text: text.to_string(), sel: s..e };
    if s == e {
        match word_around(text, s) {
            Some(w) => {
                s = w.start;
                e = w.end;
            }
            None => return unchanged(s, e),
        }
    }
    let parsed = parse(text);
    let a = Analysis { pieces: parsed.pieces.clone(), defaults: parsed.defaults.clone() };
    let mut segs: Vec<Range<usize>> = content_segments(text, &a, s, e)
        .into_iter()
        // Rader som bara är markup (div-taggar, kommentarer) får ingen formatering.
        .filter(|g| !piece_at(&a, g.start).is_some_and(|p| p.markup && p.range.end >= g.end))
        .collect();
    if segs.is_empty() {
        return unchanged(s, e);
    }
    // Bilder, kod och länkadresser tas med hela.
    let atoms = atoms(text, &a);
    for g in &mut segs {
        for at in &atoms {
            if at.start < g.start && g.start < at.end {
                g.start = at.start;
            }
            if at.start < g.end && g.end < at.end {
                g.end = at.end;
            }
        }
    }
    segs.sort_by_key(|g| g.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for g in segs {
        match merged.last_mut() {
            Some(l) if g.start < l.end => l.end = l.end.max(g.end),
            _ => merged.push(g),
        }
    }
    let segs = merged;
    let from = segs[0].start;
    let to = segs.last().map_or(from, |g| g.end);

    // Spans som berör eller överlappar markeringen skrivs om (och de som överlappar dem).
    let hs = &parsed.hspans;
    let mut inc: Vec<usize> = (0..hs.len())
        .filter(|&i| segs.iter().any(|g| hs[i].open.start <= g.end && g.start <= hs[i].close.end))
        .collect();
    loop {
        let more: Vec<usize> = (0..hs.len())
            .filter(|i| !inc.contains(i))
            .filter(|&i| inc.iter().any(|&j| hs[i].open.start < hs[j].close.end && hs[j].open.start < hs[i].close.end))
            .collect();
        if more.is_empty() {
            break;
        }
        inc.extend(more);
    }
    inc.sort_by_key(|&i| hs[i].open.start);

    // Zoner: sammanhängande områden av markeringen och de omskrivna spansen.
    let mut ivs: Vec<Range<usize>> = segs.clone();
    ivs.extend(inc.iter().map(|&i| hs[i].open.start..hs[i].close.end));
    ivs.sort_by_key(|r| r.start);
    let mut zones: Vec<Range<usize>> = Vec::new();
    for r in ivs {
        match zones.last_mut() {
            Some(z) if r.start <= z.end => z.end = z.end.max(r.end),
            _ => zones.push(r),
        }
    }
    let mut dels: Vec<Range<usize>> = Vec::new();
    for &i in &inc {
        dels.push(hs[i].open.clone());
        dels.push(hs[i].close.clone());
    }
    let tags = dels.clone();
    let deleted = |j: usize| tags.iter().any(|d| d.start <= j && j < d.end);
    let style_at = |j: usize| {
        let mut st = SpanStyle::default();
        for &i in &inc {
            if hs[i].open.end <= j && j < hs[i].close.start {
                st = st.merged(&hs[i].style);
            }
        }
        if segs.iter().any(|g| g.start <= j && j < g.end) {
            prop.apply(&mut st);
        }
        st
    };
    // Flytta en gräns ut ur avgränsare (`**`, `<u>`, …) så att den aldrig hamnar mitt i dem.
    let snap = |mut x: usize| {
        for _ in 0..16 {
            let mut changed = false;
            for em in &parsed.spans {
                if em.open.start < x && x <= em.open.end {
                    x = em.open.start;
                    changed = true;
                } else if em.close.start <= x && x < em.close.end {
                    x = em.close.end;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        x
    };

    let mut ins: Vec<(usize, String)> = Vec::new();
    let mut tag_pos: Vec<usize> = Vec::new();
    for z in &zones {
        // Gränser där stilen byter: (läge, stil före, stil efter).
        let mut bounds: Vec<(usize, SpanStyle, SpanStyle)> = Vec::new();
        let mut prev = SpanStyle::default();
        for (i, _) in text[z.clone()].char_indices() {
            let pos = z.start + i;
            // Avgränsare och borttagna taggar följer sina grannar: bara innehållet avgör stilen.
            if deleted(pos) || parsed.spans.iter().any(|em| em.open.contains(&pos) || em.close.contains(&pos)) {
                continue;
            }
            let cur = style_at(pos);
            if cur != prev {
                bounds.push((pos, std::mem::replace(&mut prev, cur.clone()), cur));
            }
        }
        if !prev.is_empty() {
            bounds.push((z.end, prev, SpanStyle::default()));
        }
        let mut norm: Vec<(usize, SpanStyle, SpanStyle)> = Vec::new();
        for (x, before, after) in bounds {
            let x = snap(x).max(norm.last().map_or(0, |n| n.0));
            match norm.last_mut() {
                Some(l) if l.0 == x => l.2 = after,
                _ => norm.push((x, before, after)),
            }
        }
        norm.retain(|n| n.1 != n.2);
        for (k, (x, before, after)) in norm.iter().enumerate() {
            let mut across: Vec<&Span> =
                parsed.spans.iter().filter(|em| em.open.start < *x && *x < em.close.end).collect();
            across.sort_by_key(|em| em.open.start);
            let prev_x = if k > 0 { norm[k - 1].0 } else { 0 };
            let next_x = norm.get(k + 1).map_or(text.len(), |n| n.0);
            let floor = across.iter().map(|em| em.open.end).max().unwrap_or(0).max(prev_x).min(*x);
            let ceil = across.iter().map(|em| em.close.start).min().unwrap_or(text.len()).min(next_x).max(*x);
            let (xc, xo) = if across.is_empty() { (*x, *x) } else { (ws_before(text, *x, floor), ws_after(text, *x, ceil)) };
            let closers: String = across.iter().rev().map(|em| &text[em.close.clone()]).collect();
            let reopen: String = across.iter().map(|em| &text[em.open.clone()]).collect();
            let mut tag = String::new();
            if !before.is_empty() {
                tag.push_str("</span>");
            }
            if !after.is_empty() {
                tag.push_str(&after.open_tag());
            }
            ins.push((xc, closers));
            ins.push((*x, tag));
            ins.push((xo, reopen));
            tag_pos.push(*x);
        }
    }

    // När span-taggarna försvunnit kan två lika betoningar mötas (`**a****b**`): slå ihop dem.
    for e1 in &parsed.spans {
        for e2 in &parsed.spans {
            if e1.kind != e2.kind
                || e1.close.end >= e2.open.start
                || text[e1.close.clone()] != text[e2.open.clone()]
                || tag_pos.iter().any(|x| (e1.close.end..=e2.open.start).contains(x))
            {
                continue;
            }
            if (e1.close.end..e2.open.start).all(deleted) {
                dels.push(e1.close.clone());
                dels.push(e2.open.clone());
            }
        }
    }

    ins.sort_by_key(|i| i.0);
    let mut edits: Vec<(Range<usize>, Vec<Part>)> = Vec::new();
    for (pos, lit) in ins {
        if lit.is_empty() {
            continue;
        }
        match edits.last_mut() {
            Some((r, parts)) if r.start == pos => parts.push(Part::Lit(lit)),
            _ => edits.push((pos..pos, vec![Part::Lit(lit)])),
        }
    }
    // Infogningar först: vid samma läge ska de komma före en borttagning som börjar där.
    dels.sort_by_key(|d| d.start);
    dels.dedup();
    for d in dels {
        edits.push((d, vec![]));
    }
    apply(text, edits, from, to)
}

// ============================================================== justering

#[derive(Clone, Copy, PartialEq)]
enum ItemKind {
    /// Ett stycke, en lista, en rubrik …
    Content,
    /// Grundformatkommentaren på första raden.
    Meta,
    DivOpen(Align),
    DivClose,
}

/// Ett block på översta nivån (eller en `<div>`-rad).
struct Item {
    range: Range<usize>,
    kind: ItemKind,
}

/// Blocken på översta nivån i ordning. Varje `<div>`/`</div>`-rad är ett eget block; en
/// `<div>` utan motsvarande stängning (eller tvärtom) räknas som vanlig text.
fn top_items(text: &str) -> Vec<Item> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut items: Vec<Item> = Vec::new();
    let mut depth = 0usize;
    let trimmed = |r: Range<usize>| -> Option<Range<usize>> {
        let start = if text.as_bytes()[line_start(text, r.start)..r.start].iter().all(|c| is_ws(*c)) {
            line_start(text, r.start)
        } else {
            r.start
        };
        let end = start + text[start..r.end.max(start)].trim_end().len();
        (end > start).then_some(start..end)
    };
    for (ev, r) in Parser::new_ext(text, opts).into_offset_iter() {
        let r = r.start.min(text.len())..r.end.min(text.len());
        match ev {
            Event::Start(tag) => {
                if depth == 0 {
                    if matches!(tag, Tag::HtmlBlock) {
                        // Rad för rad: div-taggarna är egna block, resten ett block.
                        let mut off = r.start;
                        let mut other: Option<Range<usize>> = None;
                        let flush = |items: &mut Vec<Item>, other: &mut Option<Range<usize>>| {
                            if let Some(o) = other.take() {
                                if let Some(t) = trimmed(o) {
                                    let meta = t.start == 0 && DocDefaults::from_comment(&text[t.clone()]).is_some();
                                    items.push(Item { range: t, kind: if meta { ItemKind::Meta } else { ItemKind::Content } });
                                }
                            }
                        };
                        for line in text[r.clone()].split_inclusive('\n') {
                            let ls = off;
                            off += line.len();
                            let t = line.trim();
                            if t.is_empty() {
                                continue;
                            }
                            let ts = ls + (line.len() - line.trim_start().len());
                            let tr = ts..ts + t.len();
                            let kind = if t.matches('>').count() == 1 && parse_div_open(t).is_some() {
                                Some(ItemKind::DivOpen(parse_div_open(t).unwrap_or_default()))
                            } else if is_div_close(t) {
                                Some(ItemKind::DivClose)
                            } else {
                                None
                            };
                            match kind {
                                Some(k) => {
                                    flush(&mut items, &mut other);
                                    items.push(Item { range: tr, kind: k });
                                }
                                None => match other.as_mut() {
                                    Some(o) => o.end = tr.end,
                                    None => other = Some(tr),
                                },
                            }
                        }
                        flush(&mut items, &mut other);
                    } else if let Some(t) = trimmed(r) {
                        items.push(Item { range: t, kind: ItemKind::Content });
                    }
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Rule if depth == 0 => {
                if let Some(t) = trimmed(r) {
                    items.push(Item { range: t, kind: ItemKind::Content });
                }
            }
            _ => {}
        }
    }
    // Parar ihop div-rader; oparade blir vanlig text.
    let mut stack: Vec<usize> = Vec::new();
    let mut matched = vec![false; items.len()];
    for i in 0..items.len() {
        match items[i].kind {
            ItemKind::DivOpen(_) => stack.push(i),
            ItemKind::DivClose => {
                if let Some(o) = stack.pop() {
                    matched[o] = true;
                    matched[i] = true;
                }
            }
            _ => {}
        }
    }
    for (i, it) in items.iter_mut().enumerate() {
        if matches!(it.kind, ItemKind::DivOpen(_) | ItemKind::DivClose) && !matched[i] {
            it.kind = ItemKind::Content;
        }
    }
    items
}

/// Ändrar justeringen på alla block (stycken, listor, rubriker …) som markeringen berör.
/// Justering skrivs som `<div align="…">` med tomma rader runt; ligger blocken redan i en div
/// ändras den (delas om den täcker fler block), `Left` tar bort divven. Intilliggande divar med
/// samma justering slås ihop. Divar inuti divar plattas ut (den inre vinner).
pub fn set_align(text: &str, sel: Range<usize>, align: Align) -> Edit {
    let (s, e) = norm_sel(text, &sel);
    let unchanged = || Edit { text: text.to_string(), sel: s..e };
    let items = top_items(text);
    let n = items.len();
    // Pardiv: för varje open/close-rad index till den andra raden.
    let mut pair: Vec<Option<usize>> = vec![None; n];
    // De omslutande divarnas öppningsrader (yttersta först) för varje block.
    let mut encl: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut stack: Vec<usize> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        match it.kind {
            ItemKind::DivOpen(_) => stack.push(i),
            ItemKind::DivClose => {
                if let Some(o) = stack.pop() {
                    pair[o] = Some(i);
                    pair[i] = Some(o);
                }
            }
            _ => encl[i] = stack.clone(),
        }
    }
    let old_align = |i: usize| match encl[i].last().map(|&o| items[o].kind) {
        Some(ItemKind::DivOpen(a)) => a,
        _ => Align::Left,
    };
    // Markeringen som slutar i början av en rad räknas inte med den raden.
    let e_eff = if e > s && text.as_bytes()[e - 1] == b'\n' { e - 1 } else { e };
    let touched: Vec<usize> = (0..n)
        .filter(|&i| items[i].kind == ItemKind::Content)
        .filter(|&i| items[i].range.start <= e_eff && s <= items[i].range.end)
        .collect();
    if touched.is_empty() || touched.iter().all(|&i| old_align(i) == align) {
        return unchanged();
    }
    let new_align = |i: usize| if touched.contains(&i) { align } else { old_align(i) };

    // Området som skrivs om: de berörda blocken och hela deras yttre divar.
    let mut lo = n;
    let mut hi = 0;
    for &i in &touched {
        let first = encl[i].first().copied().unwrap_or(i);
        let last = encl[i].first().and_then(|&o| pair[o]).unwrap_or(i);
        lo = lo.min(first);
        hi = hi.max(last);
    }
    let content_in = |a: usize, b: usize| -> Vec<usize> { (a..=b).filter(|&i| items[i].kind == ItemKind::Content).collect() };
    // En angränsande div med samma justering som kanten dras med.
    loop {
        let mut grew = false;
        let inner = content_in(lo, hi);
        if lo > 0 && items[lo - 1].kind == ItemKind::DivClose {
            if let Some(o) = pair[lo - 1] {
                let last = content_in(o, lo - 1).last().copied();
                if let (Some(l), Some(&f)) = (last, inner.first()) {
                    if old_align(l) == new_align(f) && new_align(f) != Align::Left {
                        lo = o;
                        grew = true;
                    }
                }
            }
        }
        if hi + 1 < n && matches!(items[hi + 1].kind, ItemKind::DivOpen(_)) {
            if let Some(c) = pair[hi + 1] {
                let first = content_in(hi + 1, c).first().copied();
                if let (Some(f), Some(&l)) = (first, inner.last()) {
                    if old_align(f) == new_align(l) && new_align(l) != Align::Left {
                        hi = c;
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }

    // Gruppera blocken efter justering och skriv dem.
    let mut groups: Vec<(Align, Vec<usize>)> = Vec::new();
    for i in content_in(lo, hi) {
        let a = new_align(i);
        match groups.last_mut() {
            Some((ga, v)) if *ga == a => v.push(i),
            _ => groups.push((a, vec![i])),
        }
    }
    let mut out = String::new();
    let mut new_start = vec![0usize; n];
    for (a, v) in &groups {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        if *a != Align::Left {
            out.push_str(&div_open(*a));
            out.push_str("\n\n");
        }
        for (k, &i) in v.iter().enumerate() {
            if k > 0 {
                out.push_str("\n\n");
            }
            new_start[i] = out.len();
            out.push_str(&text[items[i].range.clone()]);
        }
        if *a != Align::Left {
            out.push_str("\n\n</div>");
        }
    }
    let r0 = items[lo].range.start;
    let r1 = items[hi].range.end;
    let before = &text[..r0];
    let pad_before = if before.is_empty() || before.ends_with("\n\n") { "" } else { "\n" };
    let rest = &text[r1..];
    let lead = rest.bytes().take_while(|c| *c == b'\n').count();
    let pad_after = if rest.trim().is_empty() || lead >= 2 { "" } else if lead == 1 { "\n" } else { "\n\n" };
    let mut t = String::with_capacity(text.len() + out.len());
    t.push_str(before);
    t.push_str(pad_before);
    let base = t.len();
    t.push_str(&out);
    t.push_str(pad_after);
    t.push_str(rest);
    // Markeringen följer med blocken.
    let map = |pos: usize, i: usize| {
        let it = &items[i].range;
        base + new_start[i] + pos.clamp(it.start, it.end) - it.start
    };
    let first = touched.iter().copied().find(|&i| items[i].range.end >= s).unwrap_or(touched[0]);
    let last = touched.iter().rev().copied().find(|&i| items[i].range.start <= e).unwrap_or(touched[touched.len() - 1]);
    let ns = map(s, first);
    let ne = map(e, last).max(ns);
    Edit { text: t, sel: ns..ne }
}

// ============================================================== räkning

pub fn count(_text: &str, a: &Analysis) -> (usize, usize) {
    let mut vis = String::new();
    let mut chars = 0;
    for p in &a.pieces {
        if p.image.is_some() {
            continue;
        }
        if p.markup {
            // Tabellens `|` skiljer ord åt.
            if p.block == Block::Table {
                vis.push(' ');
            }
            continue;
        }
        let s = &_text[p.range.clone()];
        vis.push_str(s);
        chars += s.chars().filter(|c| *c != '\n' && *c != '\r').count();
    }
    (vis.split_whitespace().count(), chars)
}

pub fn reveal_range(text: &str, sel: Range<usize>) -> Range<usize> {
    let (s, e) = norm_sel(text, &sel);
    line_start(text, s)..line_end(text, e)
}

// ============================================================== tester

#[cfg(test)]
mod tests {
    use super::*;

    /// Biten som innehåller första förekomsten av `needle` (som också ska rymmas i den).
    fn pc(text: &str, needle: &str) -> Piece {
        let a = analyze(text);
        let i = text.find(needle).unwrap_or_else(|| panic!("{needle:?} saknas"));
        let p = piece_at(&a, i).unwrap().clone();
        assert!(p.range.end >= i + needle.len(), "{needle:?} delas över flera bitar: {a:?}");
        p
    }

    fn markup_str(text: &str) -> String {
        analyze(text).pieces.iter().filter(|p| p.markup).map(|p| &text[p.range.clone()]).collect::<Vec<_>>().join("|")
    }

    fn covers(text: &str) {
        let a = analyze(text);
        let mut pos = 0;
        for p in &a.pieces {
            assert_eq!(p.range.start, pos, "glapp i {text:?}: {a:?}");
            assert!(p.range.end > p.range.start);
            assert!(text.is_char_boundary(p.range.start) && text.is_char_boundary(p.range.end));
            pos = p.range.end;
        }
        assert_eq!(pos, text.len(), "täcker inte allt i {text:?}");
        for w in a.pieces.windows(2) {
            let (x, y) = (&w[0], &w[1]);
            let same = x.block == y.block
                && x.bold == y.bold
                && x.italic == y.italic
                && x.underline == y.underline
                && x.strike == y.strike
                && x.code == y.code
                && x.link == y.link
                && x.markup == y.markup
                && x.list_marker == y.list_marker
                && x.image == y.image
                && x.object == y.object
                && x.span == y.span
                && x.align == y.align
                && x.image.is_none()
                && x.object.is_none();
            assert!(!same, "obearbetade grannar i {text:?}");
        }
    }

    // ---- analyze

    #[test]
    fn tom_text() {
        assert!(analyze("").pieces.is_empty());
    }

    #[test]
    fn vanlig_text_en_bit() {
        let a = analyze("Hej världen");
        assert_eq!(a.pieces.len(), 1);
        assert_eq!(a.pieces[0].block, Block::Normal);
        assert!(!a.pieces[0].markup);
    }

    #[test]
    fn fetstil() {
        let t = "a **fet** b";
        assert!(pc(t, "fet").bold);
        assert!(!pc(t, "a ").bold);
        assert_eq!(markup_str(t), "**|**");
        assert!(pc(t, "**").markup);
        let t = "a __fet__ b";
        assert!(pc(t, "fet").bold);
        assert_eq!(markup_str(t), "__|__");
    }

    #[test]
    fn kursiv_och_genomstruken() {
        let t = "a *kur* b _siv_ c ~~bort~~ d";
        assert!(pc(t, "kur").italic);
        assert!(pc(t, "siv").italic);
        assert!(pc(t, "bort").strike);
        assert_eq!(markup_str(t), "*|*|_|_|~~|~~");
        assert!(!pc(t, "kur").bold);
    }

    #[test]
    fn understrykning_html() {
        let t = "a <u>under</u> b";
        let p = pc(t, "under");
        assert!(p.underline && !p.markup);
        assert_eq!(markup_str(t), "<u>|</u>");
        let t = "<U>x</U>";
        assert!(pc(t, "x").underline);
        // Oparad tagg är vanlig text.
        let t = "a <u>b";
        assert!(!pc(t, "b").underline);
    }

    #[test]
    fn nästlade_kombinationer() {
        let p = pc("***x***", "x");
        assert!(p.bold && p.italic);
        assert_eq!(markup_str("***x***"), "*|**|**|*");
        let t = "**a *b* c**";
        assert!(pc(t, "a ").bold && !pc(t, "a ").italic);
        assert!(pc(t, "b").bold && pc(t, "b").italic);
        let t = "<u>**x**</u>";
        let p = pc(t, "x");
        assert!(p.underline && p.bold && !p.markup);
        assert_eq!(markup_str(t), "<u>|**|**|</u>");
    }

    #[test]
    fn inline_kod() {
        let t = "se `kod` här";
        assert!(pc(t, "kod").code);
        assert_eq!(markup_str(t), "`|`");
        assert!(!pc(t, "se ").code);
    }

    #[test]
    fn kodblock_med_staket() {
        let t = "text\n\n```rust\nfn a() {}\n```\n\nslut";
        let p = pc(t, "fn a() {}");
        assert!(p.code && p.block == Block::Code && !p.markup);
        assert_eq!(markup_str(t), "```rust|```");
        assert_eq!(pc(t, "```rust").block, Block::Code);
        assert_eq!(pc(t, "slut").block, Block::Normal);
        let t = "~~~\nx\n~~~";
        assert!(pc(t, "x").code);
        // Utan avslutande staket är resten kod.
        let t = "```\nx\ny";
        assert!(pc(t, "x\ny").code);
    }

    #[test]
    fn indenterat_kodblock() {
        let t = "text\n\n    kod här\n\nslut";
        assert_eq!(pc(t, "kod här").block, Block::Code);
        assert_eq!(pc(t, "slut").block, Block::Normal);
    }

    #[test]
    fn rubriker() {
        let t = "## Rubrik\n\ntext";
        let p = pc(t, "Rubrik");
        assert_eq!(p.block, Block::Heading(2));
        assert!(!p.markup);
        assert!(pc(t, "## ").markup);
        assert_eq!(pc(t, "text").block, Block::Normal);
        assert_eq!(markup_str(t), "## ");
        // Avslutande hashar.
        let t = "# Titel #\n";
        assert_eq!(markup_str(t), "# | #");
        assert_eq!(pc(t, "Titel").block, Block::Heading(1));
        // Nivå 6 och inte-rubrik.
        assert_eq!(pc("###### x", "x").block, Block::Heading(6));
        assert_eq!(pc("####### x", "x").block, Block::Normal);
        assert_eq!(pc("#x", "x").block, Block::Normal);
    }

    #[test]
    fn rubrik_med_åäö() {
        let t = "# Åäö och ö\n";
        covers(t);
        assert_eq!(pc(t, "Åäö").block, Block::Heading(1));
    }

    #[test]
    fn setext_rubrik() {
        let t = "Titel\n=====\n\ntext";
        assert_eq!(pc(t, "Titel").block, Block::Heading(1));
        let u = pc(t, "=====");
        assert!(u.markup && u.block == Block::Heading(1));
        let t = "Titel\n-----\n";
        assert_eq!(pc(t, "Titel").block, Block::Heading(2));
        assert!(pc(t, "-----").markup);
    }

    #[test]
    fn citat() {
        let t = "> citerad text\n> mer\n\nvanlig";
        let p = pc(t, "citerad");
        assert_eq!(p.block, Block::Quote);
        assert!(pc(t, "> ").markup);
        assert_eq!(markup_str(t), "> |> ");
        assert_eq!(pc(t, "vanlig").block, Block::Normal);
        let t = "> > djupt\n";
        assert_eq!(markup_str(t), "> > ");
    }

    #[test]
    fn punktlista() {
        let t = "- ett\n- två\n\nslut";
        let m = pc(t, "- ");
        assert!(m.markup && m.list_marker && m.block == Block::Bullet);
        let p = pc(t, "ett");
        assert_eq!(p.block, Block::Bullet);
        assert!(!p.markup && !p.list_marker);
        assert_eq!(pc(t, "slut").block, Block::Normal);
        let t = "* a\n+ b\n";
        assert_eq!(markup_str(t), "* |+ ");
    }

    #[test]
    fn numrerad_lista() {
        let t = "1. ett\n2. två\n";
        let m = pc(t, "1. ");
        assert!(m.list_marker && !m.markup && m.block == Block::Numbered);
        assert_eq!(pc(t, "två").block, Block::Numbered);
        assert!(!pc(t, "två").list_marker);
        assert_eq!(markup_str(t), "");
    }

    #[test]
    fn nästlad_lista_med_fyra_blanksteg_eller_tabb() {
        for t in ["- a\n    - b\n", "- a\n\t- b\n"] {
            let a = analyze(t);
            let i = t.find("- b").unwrap();
            let m = piece_at(&a, i).unwrap();
            assert!(m.markup && m.list_marker, "{t:?}: {a:?}");
            covers(t);
        }
    }

    #[test]
    fn nästlad_lista_indrag_är_vanligt() {
        let t = "- a\n  - b\n";
        let a = analyze(t);
        let i = t.find("  - b").unwrap();
        let p = piece_at(&a, i).unwrap();
        assert!(!p.markup && !p.list_marker);
        assert_eq!(p.block, Block::Bullet);
        let m = piece_at(&a, t.find("- b").unwrap()).unwrap();
        assert!(m.markup && m.list_marker);
        assert_eq!(pc(t, "b").block, Block::Bullet);
        covers(t);
    }

    #[test]
    fn uppgiftslista() {
        let t = "- [ ] gör\n- [x] klar\n";
        assert_eq!(markup_str(t), "- |- ");
        assert!(!pc(t, "[ ] gör").markup);
        assert_eq!(pc(t, "klar").block, Block::Bullet);
    }

    #[test]
    fn tabell() {
        let t = "| A | B |\n| --- | --- |\n| ö | **x** |\n\nefter";
        assert_eq!(pc(t, " A ").block, Block::Table);
        assert!(!pc(t, " A ").markup);
        assert!(pc(t, "| --- | --- |").markup);
        assert!(pc(t, "x").bold);
        assert_eq!(pc(t, "x").block, Block::Table);
        assert_eq!(pc(t, "efter").block, Block::Normal);
        let m = markup_str(t);
        assert!(m.starts_with("|"));
        covers(t);
    }

    #[test]
    fn länkar() {
        let t = "se [text](http://x.se) nu";
        let p = pc(t, "text");
        assert!(p.link && !p.markup);
        assert!(pc(t, "[").markup);
        assert!(pc(t, "](http://x.se)").markup);
        assert!(!pc(t, "se ").link);
        assert_eq!(markup_str(t), "[|](http://x.se)");
        // Fet länktext.
        let t = "[**f**](u)";
        assert!(pc(t, "f").link && pc(t, "f").bold);
    }

    #[test]
    fn autolänk() {
        let t = "gå till <http://x.se> nu";
        assert!(pc(t, "http://x.se").link);
        assert_eq!(markup_str(t), "<|>");
    }

    #[test]
    fn bilder() {
        let t = "före\n\n![Sol](bild:image1.png)\n\nefter";
        let p = pc(t, "![Sol](bild:image1.png)");
        assert!(p.markup);
        assert_eq!(p.image, Some(ImageRef { alt: "Sol".into(), src: "bild:image1.png".into(), width: None, height: None }));
        assert_eq!(p.range.len(), "![Sol](bild:image1.png)".len());
        // Två bilder intill varandra blir två bitar.
        let t = "![a](1.png)![b](2.png)";
        let a = analyze(t);
        assert_eq!(a.pieces.len(), 2);
        assert_eq!(a.pieces[1].image.as_ref().unwrap().alt, "b");
        // Bildtext med formatering ger fortfarande en bit.
        let t = "![**a** b](x.png)";
        assert_eq!(analyze(t).pieces.len(), 1);
    }

    #[test]
    fn backstreck_escape() {
        let t = r"a \*inte\* b";
        assert!(!pc(t, "inte").italic);
        assert_eq!(markup_str(t), "\\|\\");
        let t = r"a\\b";
        assert_eq!(markup_str(t), "\\");
    }

    #[test]
    fn annat_är_vanligt() {
        let t = "<div>x</div>\n\nrad\n\n---\n";
        let a = analyze(t);
        assert!(a.pieces.iter().all(|p| p.block == Block::Normal && !p.bold));
    }

    #[test]
    fn åäö_i_formatering() {
        let t = "åäö **fåglar** ö_ö";
        covers(t);
        assert!(pc(t, "fåglar").bold);
        assert_eq!(pc(t, "fåglar").range.len(), "fåglar".len());
    }

    #[test]
    fn täcker_exakt_på_exempeldokument() {
        let docs = [
            "",
            "x",
            "\n",
            "\n\n\n",
            "# Rubrik 1\n\nBrödtext med **fet**, *kursiv*, ~~struken~~ och <u>understruken</u> text.\n\n## Rubrik 2\n\n- punkt ett\n- punkt **två**\n  - nästlad\n\n1. första\n2. andra\n\n> citat med *betoning*\n\n| Namn | Ålder |\n| --- | --- |\n| Åsa | 31 |\n| Örjan | **42** |\n\nEn [länk](https://example.com/a_b) och en bild:\n\n![Sol](bild:image1.png)\n\n```\nkod\n```\n\nSlut.\n",
            "- [ ] a\n- [x] b\n",
            "Titel\n=====\n\ntext\n",
            "hej\\\nvärlden  \nrad",
            "**a**b*c*`d`~~e~~<u>f</u>[g](h)![i](j)",
            "> - lista i citat\n> - två\n",
            "1. a\n\n   fortsättning\n\n2. b\n",
            "<b>html</b> &amp; åäö\n",
            "Text utan slutradbrytning **fet",
            "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt -->\n\n<span style=\"color: #FF0000\">röd <u>**åäö**</u></span> och <span style=\"font-size: 14pt\">stor</span>\n",
            "<div align=\"center\">\n\nMitt\n\n<div align=\"right\">\n\nHöger\n\n</div>\n\n</div>\n\nSlut\n",
            "<div align=\"center\">\ntext utan tomma rader\n</div>\n",
            "<span>oparad\n\n</span> <div>oparad\n\n</div>\n\n</div>",
            "<span style=\"color: red\">a\n\nb</span>\n\n- <span style=\"color: blue\">punkt</span>\n",
        ];
        for d in docs {
            covers(d);
        }
    }

    // ---- active

    #[test]
    fn active_vid_markören() {
        let t = "a **fet** b";
        let a = analyze(t);
        // Inuti ordet.
        assert!(active(t, &a, 5..5).bold);
        // Precis inuti början `**|fet**` och slutet `**fet|**`.
        assert!(active(t, &a, 4..4).bold);
        assert!(active(t, &a, 7..7).bold);
        // Utanför: före `**` och efter `**`.
        assert!(!active(t, &a, 2..2).bold);
        assert!(!active(t, &a, 9..9).bold);
        assert!(!active(t, &a, 0..0).bold);
        assert!(!active(t, &a, t.len()..t.len()).bold);
    }

    #[test]
    fn active_flera_flaggor() {
        let t = "***x***";
        let a = analyze(t);
        let r = active(t, &a, 4..4);
        assert!(r.bold && r.italic && !r.underline && !r.strike);
        let t = "<u>x</u>";
        let a = analyze(t);
        assert!(active(t, &a, 4..4).underline);
        let t = "~~x~~";
        let a = analyze(t);
        assert!(active(t, &a, 3..3).strike);
    }

    #[test]
    fn active_markering() {
        let t = "**ab** cd **ef**";
        let a = analyze(t);
        assert!(active(t, &a, 2..4).bold);
        // Hela, med markup och blanksteg emellan: bara innehållet räknas men "cd" är inte fet.
        assert!(!active(t, &a, 0..t.len()).bold);
        // Markering som bara har fet text + blanksteg + markup.
        assert!(active(t, &a, 0..6).bold);
        assert!(!active(t, &a, 0..9).bold);
        // Bara blanksteg ger falskt.
        assert!(!active(t, &a, 6..7).bold);
    }

    #[test]
    fn active_block() {
        let t = "# Rubrik\n\n> citat\n- punkt\n1. nr\n";
        let a = analyze(t);
        assert_eq!(active(t, &a, 3..3).block, Block::Heading(1));
        assert_eq!(active(t, &a, 0..0).block, Block::Heading(1));
        assert_eq!(active(t, &a, 9..9).block, Block::Normal);
        let q = t.find("citat").unwrap();
        assert_eq!(active(t, &a, q..q + 2).block, Block::Quote);
        let b = t.find("punkt").unwrap();
        assert_eq!(active(t, &a, b..b).block, Block::Bullet);
        let n = t.find("nr").unwrap();
        assert_eq!(active(t, &a, n..n + 2).block, Block::Numbered);
        // Nästlad rad: indraget tillhör objektet.
        let t = "- a\n  - b\n";
        let a = analyze(t);
        assert_eq!(active(t, &a, 4..4).block, Block::Bullet);
    }

    #[test]
    fn active_åäö() {
        let t = "**åäö**";
        let a = analyze(t);
        assert!(active(t, &a, 4..4).bold); // mellan å och ä
        assert!(active(t, &a, 2..t.len() - 2).bold);
    }

    // ---- toggle_inline

    fn tog(t: &str, sel: Range<usize>, m: Inline) -> Edit {
        toggle_inline(t, sel, m)
    }

    fn sel_str<'a>(e: &'a Edit) -> &'a str {
        &e.text[e.sel.clone()]
    }

    #[test]
    fn toggle_lägger_till_fetstil() {
        let e = tog("hej världen", 0..3, Inline::Bold);
        assert_eq!(e.text, "**hej** världen");
        assert_eq!(sel_str(&e), "hej");
        assert!(pc(&e.text, "hej").bold);
    }

    #[test]
    fn toggle_alla_slag_och_omanalys() {
        for (m, open, close) in [
            (Inline::Bold, "**", "**"),
            (Inline::Italic, "*", "*"),
            (Inline::Underline, "<u>", "</u>"),
            (Inline::Strike, "~~", "~~"),
        ] {
            let t = "före mitt åäö efter";
            let s = t.find("mitt åäö").unwrap();
            let e = tog(t, s..s + "mitt åäö".len(), m);
            assert_eq!(e.text, format!("före {open}mitt åäö{close} efter"));
            assert_eq!(sel_str(&e), "mitt åäö");
            let p = pc(&e.text, "mitt åäö");
            assert!(flag_of(&p, m), "{m:?}");
            assert!(!pc(&e.text, "före").bold);
            // Och tillbaka.
            let back = tog(&e.text, e.sel.clone(), m);
            assert_eq!(back.text, t, "{m:?}");
            assert_eq!(sel_str(&back), "mitt åäö");
        }
    }

    #[test]
    fn toggle_tom_markering_i_ord_utvidgas() {
        let t = "ett ord här";
        let e = tog(t, 6..6, Inline::Bold);
        assert_eq!(e.text, "ett **ord** här");
        assert_eq!(sel_str(&e), "ord");
        let t2 = "ett Åsa's-bil här";
        let e = tog(t2, 6..6, Inline::Italic);
        assert_eq!(e.text, "ett *Åsa's-bil* här");
        // Tillbaka med markören kvar i ordet.
        let c = e.sel.start + 2;
        let back = tog(&e.text, c..c, Inline::Italic);
        assert_eq!(back.text, t2);
    }

    #[test]
    fn toggle_tom_markering_utanför_ord_sätter_in_par() {
        let t = "a  b";
        let e = tog(t, 2..2, Inline::Bold);
        assert_eq!(e.text, "a **** b");
        assert_eq!(e.sel, 4..4);
        let e = tog(t, 2..2, Inline::Underline);
        assert_eq!(e.text, "a <u></u> b");
        assert_eq!(e.sel, 5..5);
        // Tom text.
        let e = tog("", 0..0, Inline::Italic);
        assert_eq!(e.text, "**");
        assert_eq!(e.sel, 1..1);
    }

    #[test]
    fn toggle_tom_markering_i_tomt_par_tar_bort_det() {
        let e = tog("a **** b", 4..4, Inline::Bold);
        assert_eq!(e.text, "a  b");
        assert_eq!(e.sel, 2..2);
        let e = tog("a <u></u> b", 5..5, Inline::Underline);
        assert_eq!(e.text, "a  b");
        let e = tog("a ~~~~ b", 4..4, Inline::Strike);
        assert_eq!(e.text, "a  b");
        // Kursiv inuti ett tomt fetpar är inte "sitt eget" tomma par.
        let e = tog("a **** b", 4..4, Inline::Italic);
        assert_eq!(e.text, "a ****** b");
        assert_eq!(e.sel, 5..5);
    }

    #[test]
    fn toggle_ta_bort_delar_av_spann() {
        let e = tog("**abc**", 3..4, Inline::Bold);
        assert_eq!(e.text, "**a**b**c**");
        assert_eq!(sel_str(&e), "b");
        let a = analyze(&e.text);
        assert!(pc(&e.text, "a").bold && pc(&e.text, "c").bold);
        assert!(!pc(&e.text, "b").bold);
        let _ = a;
        // Början och slutet.
        let e = tog("**abc**", 2..3, Inline::Bold);
        assert_eq!(e.text, "a**bc**");
        assert_eq!(sel_str(&e), "a");
        let e = tog("**abc**", 4..5, Inline::Bold);
        assert_eq!(e.text, "**ab**c");
        // Hela spannet: inga tomma `****` kvar.
        let e = tog("x **abc** y", 4..7, Inline::Bold);
        assert_eq!(e.text, "x abc y");
    }

    #[test]
    fn toggle_ta_bort_flyttar_blanksteg_utanför() {
        let e = tog("**a b**", 4..5, Inline::Bold);
        assert_eq!(e.text, "**a** b");
        assert!(pc(&e.text, "a").bold);
        assert!(!pc(&e.text, "b").bold);
    }

    #[test]
    fn toggle_ta_bort_flera_spann() {
        let t = "**a** och **b**";
        // Helt fet markering (blanksteg mellan spannen räknas inte).
        let e = tog(t, 0..5, Inline::Bold);
        assert_eq!(e.text, "a och **b**");
        let t = "**a** **b**";
        let e = tog(t, 0..t.len(), Inline::Bold);
        assert_eq!(e.text, "a b");
        assert_eq!(sel_str(&e), "a b");
        // Delvis fet: allt blir ett spann.
        let t = "**a** och **b**";
        let e = tog(t, 0..t.len(), Inline::Bold);
        assert_eq!(e.text, "**a och b**");
    }

    #[test]
    fn toggle_ta_bort_kursiv_ur_fetkursiv() {
        let e = tog("***x***", 3..4, Inline::Italic);
        let p = pc(&e.text, "x");
        assert!(p.bold && !p.italic, "{:?}", e.text);
        let e = tog("***x***", 3..4, Inline::Bold);
        let p = pc(&e.text, "x");
        assert!(!p.bold && p.italic, "{:?}", e.text);
    }

    #[test]
    fn toggle_flera_rader() {
        let t = "rad ett\nrad två\n\nrad tre";
        let e = tog(t, 0..t.len(), Inline::Bold);
        assert_eq!(e.text, "**rad ett**\n**rad två**\n\n**rad tre**");
        assert_eq!(e.sel.start, 2);
        assert_eq!(&e.text[e.sel.end - 3..e.sel.end], "tre");
        for w in ["rad ett", "rad två", "rad tre"] {
            assert!(pc(&e.text, w).bold, "{w}");
        }
        // Tillbaka.
        let back = tog(&e.text, e.sel.clone(), Inline::Bold);
        assert_eq!(back.text, t);
    }

    #[test]
    fn toggle_hoppar_över_blockprefix() {
        let t = "# Rubrik\n> citat\n- punkt\n1. nr";
        let e = tog(t, 0..t.len(), Inline::Italic);
        assert_eq!(e.text, "# *Rubrik*\n> *citat*\n- *punkt*\n1. *nr*");
        assert_eq!(pc(&e.text, "Rubrik").block, Block::Heading(1));
        assert!(pc(&e.text, "Rubrik").italic);
        assert!(pc(&e.text, "citat").italic);
        assert!(pc(&e.text, "punkt").italic);
        assert!(pc(&e.text, "nr").italic);
        assert_eq!(pc(&e.text, "nr").block, Block::Numbered);
        // Markering som börjar inne i prefixet.
        let e = tog("## Hej", 0..6, Inline::Bold);
        assert_eq!(e.text, "## **Hej**");
    }

    #[test]
    fn toggle_trimmar_blanksteg() {
        let e = tog("a  hej  b", 1..7, Inline::Bold);
        assert_eq!(e.text, "a  **hej**  b");
        assert_eq!(sel_str(&e), "hej");
        // Bara blanksteg: ingen ändring.
        let e = tog("a   b", 1..4, Inline::Bold);
        assert_eq!(e.text, "a   b");
    }

    #[test]
    fn toggle_ingen_dubbel_nästling() {
        let t = "a **b** c";
        // Delvis fet markering: allt blir fett, utan `****`.
        let e = tog(t, 0..t.len(), Inline::Bold);
        assert_eq!(e.text, "**a b c**");
        // Markering som bara delvis täcker ett fett spann drar med hela spannet.
        let e = tog("**ab** cd", 3..9, Inline::Bold);
        assert_eq!(e.text, "**ab cd**");
        let p = pc(&e.text, "ab cd");
        assert!(p.bold);
        // Kursiv i fet påverkas inte.
        let e = tog("**a *b* c**", 0..11, Inline::Bold);
        assert!(!e.text.contains("****"));
        let e = tog("a *b* c", 0..7, Inline::Italic);
        assert_eq!(e.text, "*a b c*");
    }

    #[test]
    fn toggle_lägger_fet_runt_kursiv() {
        let e = tog("*kur*", 1..4, Inline::Bold);
        assert_eq!(e.text, "***kur***");
        let p = pc(&e.text, "kur");
        assert!(p.bold && p.italic);
        let e = tog("<u>x</u>", 3..4, Inline::Bold);
        assert_eq!(e.text, "<u>**x**</u>");
        let p = pc(&e.text, "x");
        assert!(p.bold && p.underline);
    }

    #[test]
    fn toggle_rundtur_enkla_fall() {
        let cases = [
            ("ett två tre", 4..7),
            ("åäö ÅÄÖ", 0..7),
            ("en rad\nandra raden", 3..12),
            ("- punkt ett", 2..11),
            ("# Rubrik", 2..8),
        ];
        for m in [Inline::Bold, Inline::Italic, Inline::Underline, Inline::Strike] {
            for (t, s) in cases.clone() {
                let a = tog(t, s, m);
                assert_ne!(a.text, t, "{t:?} {m:?}");
                let b = tog(&a.text, a.sel.clone(), m);
                assert_eq!(b.text, t, "{t:?} {m:?}");
            }
        }
    }

    #[test]
    fn toggle_i_tabell_per_cell() {
        let t = "| A | B |\n| --- | --- |\n| x | y |\n";
        let e = tog(t, 0..t.len(), Inline::Bold);
        assert!(e.text.contains("| **A** | **B** |"), "{}", e.text);
        assert!(e.text.contains("| --- | --- |"));
        assert!(e.text.contains("| **x** | **y** |"));
        assert!(pc(&e.text, "x").bold);
        assert_eq!(pc(&e.text, "x").block, Block::Table);
    }

    #[test]
    fn toggle_hoppar_över_kodrader() {
        let t = "```\nkod\n```\ntext";
        let e = tog(t, 0..t.len(), Inline::Bold);
        assert_eq!(e.text, "```\nkod\n```\n**text**");
    }

    #[test]
    fn toggle_klarar_felaktiga_offset() {
        let t = "åäö";
        let e = tog(t, 1..2, Inline::Bold); // mitt i å
        assert!(e.text.is_char_boundary(e.sel.start) && e.text.is_char_boundary(e.sel.end));
        let e = tog(t, 50..60, Inline::Bold);
        assert!(!e.text.is_empty());
        let e = tog(t, 4..0, Inline::Italic);
        assert!(e.text.contains('*'));
    }

    // ---- set_block

    fn blk(t: &str, sel: Range<usize>, b: Block) -> Edit {
        set_block(t, sel, b)
    }

    #[test]
    fn block_rubrik_på_rad() {
        let e = blk("Hej\nvärlden", 1..1, Block::Heading(2));
        assert_eq!(e.text, "## Hej\nvärlden");
        assert_eq!(e.sel, 4..4);
        assert_eq!(pc(&e.text, "Hej").block, Block::Heading(2));
        assert_eq!(pc(&e.text, "världen").block, Block::Normal);
    }

    #[test]
    fn block_växla_av_ger_normal() {
        let e = blk("## Hej", 4..4, Block::Heading(2));
        assert_eq!(e.text, "Hej");
        assert_eq!(e.sel, 1..1);
        let e = blk("> a\n> b", 0..7, Block::Quote);
        assert_eq!(e.text, "a\nb");
        let e = blk("- a\n- b", 0..7, Block::Bullet);
        assert_eq!(e.text, "a\nb");
        // Blandade rader: alla får blocket.
        let e = blk("- a\n\nb", 0..6, Block::Bullet);
        assert_eq!(e.text, "- a\n\n- b");
        // Normal på normal text är ingen ändring.
        let e = blk("a\nb", 0..3, Block::Normal);
        assert_eq!(e.text, "a\nb");
    }

    #[test]
    fn block_byter_prefix() {
        let e = blk("# Hej", 0..0, Block::Heading(3));
        assert_eq!(e.text, "### Hej");
        let e = blk("> Hej", 0..0, Block::Heading(1));
        assert_eq!(e.text, "# Hej");
        let e = blk("1. Hej", 0..0, Block::Quote);
        assert_eq!(e.text, "> Hej");
        let e = blk("# Hej", 0..0, Block::Bullet);
        assert_eq!(e.text, "- Hej");
    }

    #[test]
    fn block_lista_till_rubrik_och_tillbaka() {
        let e = blk("  - a", 4..5, Block::Heading(1));
        assert_eq!(e.text, "# a");
        assert_eq!(pc(&e.text, "a").block, Block::Heading(1));
        // Indraget behålls bara mellan listor.
        let e = blk("  - a", 4..5, Block::Numbered);
        assert_eq!(e.text, "  1. a");
        let e = blk("  1. a", 5..6, Block::Bullet);
        assert_eq!(e.text, "  - a");
        let e = blk("a", 0..0, Block::Bullet);
        assert_eq!(e.text, "- a");
    }

    #[test]
    fn block_numrerar_löpande() {
        let t = "a\nb\n\nc\n- d";
        let e = blk(t, 0..t.len(), Block::Numbered);
        assert_eq!(e.text, "1. a\n2. b\n\n3. c\n4. d");
        assert_eq!(pc(&e.text, "d").block, Block::Numbered);
        // Omnumrering vid byte från punktlista.
        let e = blk("- x\n- y\n- z", 0..11, Block::Numbered);
        assert_eq!(e.text, "1. x\n2. y\n3. z");
        assert_eq!(e.sel.start, 3);
        assert_eq!(&e.text[e.sel.end - 1..e.sel.end], "z");
    }

    #[test]
    fn block_hoppar_över_tomma_kod_och_tabell() {
        let t = "a\n\n```\nkod\n```\n\n| A |\n| - |\n| x |\n\nb";
        let e = blk(t, 0..t.len(), Block::Quote);
        assert_eq!(e.text, "> a\n\n```\nkod\n```\n\n| A |\n| - |\n| x |\n\n> b");
    }

    #[test]
    fn block_code_och_table_är_ingen_åtgärd() {
        let e = blk("a", 0..1, Block::Code);
        assert_eq!(e.text, "a");
        assert_eq!(e.sel, 0..1);
        let e = blk("a", 0..1, Block::Table);
        assert_eq!(e.text, "a");
    }

    #[test]
    fn block_markering_slutar_vid_radstart() {
        // Markeringen slutar precis efter radbrytningen: sista raden berörs inte.
        let e = blk("a\nb", 0..2, Block::Heading(1));
        assert_eq!(e.text, "# a\nb");
    }

    #[test]
    fn block_åäö_och_markering() {
        let t = "åäö\nöl";
        let e = blk(t, 0..t.len(), Block::Quote);
        assert_eq!(e.text, "> åäö\n> öl");
        assert_eq!(e.sel.start, 2);
        assert_eq!(e.sel.end, e.text.len());
        assert!(e.text.is_char_boundary(e.sel.start));
    }

    #[test]
    fn block_ändrar_inte_inline() {
        let e = blk("**fet** text", 0..0, Block::Heading(1));
        assert_eq!(e.text, "# **fet** text");
        assert!(pc(&e.text, "fet").bold);
    }

    // ---- insert_table

    fn kontrollera_tabell(e: &Edit) {
        let a = analyze(&e.text);
        let i = e.text.find("Kolumn 1").unwrap();
        assert_eq!(piece_at(&a, i).unwrap().block, Block::Table);
        let c = e.sel.start;
        assert_eq!(e.sel.start, e.sel.end);
        assert_eq!(piece_at(&a, c).unwrap().block, Block::Table);
        // Markören ligger inuti första cellen på bodyraden.
        let ls = line_start(&e.text, c);
        assert_eq!(&e.text[ls..line_end(&e.text, c)], "|  |  |");
        assert_eq!(c - ls, 2);
    }

    #[test]
    fn tabell_i_tom_text() {
        let e = insert_table("", 0);
        assert_eq!(e.text, "| Kolumn 1 | Kolumn 2 |\n| --- | --- |\n|  |  |\n");
        kontrollera_tabell(&e);
    }

    #[test]
    fn tabell_efter_text_får_tom_rad() {
        let e = insert_table("hej", 3);
        assert_eq!(e.text, "hej\n\n| Kolumn 1 | Kolumn 2 |\n| --- | --- |\n|  |  |\n");
        kontrollera_tabell(&e);
        let e = insert_table("hej\n", 4);
        assert_eq!(e.text, "hej\n\n| Kolumn 1 | Kolumn 2 |\n| --- | --- |\n|  |  |\n");
        let e = insert_table("hej\n\n", 5);
        assert_eq!(e.text, "hej\n\n| Kolumn 1 | Kolumn 2 |\n| --- | --- |\n|  |  |\n");
    }

    #[test]
    fn tabell_före_text_får_tom_rad() {
        let e = insert_table("hej", 0);
        assert_eq!(e.text, "| Kolumn 1 | Kolumn 2 |\n| --- | --- |\n|  |  |\n\nhej");
        kontrollera_tabell(&e);
        // Mitt i en text.
        let e = insert_table("ab\ncd", 3);
        assert_eq!(e.text, "ab\n\n| Kolumn 1 | Kolumn 2 |\n| --- | --- |\n|  |  |\n\ncd");
        let a = analyze(&e.text);
        let i = e.text.find("cd").unwrap();
        assert_eq!(piece_at(&a, i).unwrap().block, Block::Normal);
        kontrollera_tabell(&e);
        // Rad som slutar före radbrytning.
        let e = insert_table("åäö\nx", "åäö".len());
        kontrollera_tabell(&e);
        assert!(e.text.ends_with("\nx"));
    }

    // ---- count och reveal_range

    #[test]
    fn räkna_ord_och_tecken() {
        let t = "Hej **fina** världen";
        let a = analyze(t);
        assert_eq!(count(t, &a), (3, "Hej fina världen".chars().count()));
        let t = "# Titel\n\n- ett\n- två tre\n";
        let a = analyze(t);
        assert_eq!(count(t, &a), (4, "Titel".len() + "ett".len() + "två tre".chars().count()));
        // Bilder räknas inte.
        let t = "a ![x y z](bild.png) b";
        let a = analyze(t);
        assert_eq!(count(t, &a), (2, "a ".len() + " b".len()));
        // Tabellens streck delar ord.
        let t = "|a|b|\n|-|-|\n|c|d|\n";
        let a = analyze(t);
        assert_eq!(count(t, &a).0, 4);
        assert_eq!(count("", &analyze("")), (0, 0));
        // Länk: bara texten.
        let t = "[ett ord](http://länk.se/lång/väg)";
        assert_eq!(count(t, &analyze(t)), (2, "ett ord".len()));
        // Åäö räknas som ett tecken var.
        let t = "åäö";
        assert_eq!(count(t, &analyze(t)), (1, 3));
    }

    #[test]
    fn avslöja_hela_rader() {
        let t = "ab\ncd\nef";
        assert_eq!(reveal_range(t, 0..0), 0..2);
        assert_eq!(reveal_range(t, 4..4), 3..5);
        assert_eq!(reveal_range(t, 1..7), 0..8);
        assert_eq!(reveal_range(t, 3..3), 3..5);
        assert_eq!(reveal_range("", 0..0), 0..0);
        let t = "åäö\nöl";
        assert_eq!(reveal_range(t, 2..2), 0..6);
        assert_eq!(reveal_range(t, 8..9), 7..10);
    }

    // ---- spans, justering och grundformat (1.5)

    const RED: &str = "<span style=\"color: #FF0000\">";

    fn red() -> SpanStyle {
        SpanStyle { color: Some([255, 0, 0]), ..Default::default() }
    }

    /// Texten med varje sammanhängande bit av `needle`-stil avgränsad, för att läsa av resultat.
    fn color_of(t: &str, needle: &str) -> Option<[u8; 3]> {
        pc(t, needle).span.and_then(|s| s.color)
    }

    #[test]
    fn analys_span_markup_och_stil() {
        let t = format!("a {RED}röd</span> b");
        let p = pc(&t, "röd");
        assert_eq!(p.span.as_deref(), Some(&red()));
        assert!(!p.markup);
        assert!(pc(&t, RED).markup && pc(&t, "</span>").markup);
        assert_eq!(markup_str(&t), format!("{RED}|</span>"));
        assert_eq!(pc(&t, "a ").span, None);
        assert_eq!(pc(&t, " b").span, None);
        covers(&t);
    }

    #[test]
    fn analys_span_nästlade_slås_ihop_och_inre_vinner() {
        let t = "<span style=\"color: #FF0000; font-size: 12pt\">a <span style=\"color: #0000FF\">b</span> c</span>";
        let a = pc(t, "a ").span.unwrap();
        assert_eq!((a.color, a.font_size), (Some([255, 0, 0]), Some(12.0)));
        let b = pc(t, "b").span.unwrap();
        assert_eq!((b.color, b.font_size), (Some([0, 0, 255]), Some(12.0)));
        let c = pc(t, " c").span.unwrap();
        assert_eq!(c.color, Some([255, 0, 0]));
        assert_eq!(analyze(t).pieces.iter().filter(|p| p.markup).count(), 4);
        covers(t);
    }

    #[test]
    fn analys_span_med_betoning_och_oparad_span() {
        let t = format!("{RED}<u>**x**</u></span>");
        let p = pc(&t, "x");
        assert!(p.bold && p.underline && p.span.is_some() && !p.markup);
        assert_eq!(markup_str(&t), format!("{RED}|<u>|**|**|</u>|</span>"));
        // Oparad span är vanlig text.
        let t = format!("{RED}x");
        assert_eq!(pc(&t, "x").span, None);
        assert!(!analyze(&t).pieces.iter().any(|p| p.markup));
        // Span utan style ger en tom stil men är ändå markup.
        let t = "<span>x</span>";
        assert!(pc(t, "x").span.is_some_and(|s| s.is_empty()));
    }

    #[test]
    fn analys_span_över_rader_och_i_lista() {
        let t = format!("- {RED}punkt</span>\n- annan\n");
        assert_eq!(color_of(&t, "punkt"), Some([255, 0, 0]));
        assert!(pc(&t, "annan").span.is_none());
        assert_eq!(pc(&t, "punkt").block, Block::Bullet);
        covers(&t);
    }

    #[test]
    fn analys_div_justering() {
        let t = "före\n\n<div align=\"center\">\n\nmitten\n\n</div>\n\nefter\n";
        assert_eq!(pc(t, "mitten").align, Align::Center);
        assert_eq!(pc(t, "före").align, Align::Left);
        assert_eq!(pc(t, "efter").align, Align::Left);
        let o = pc(t, "<div align=\"center\">");
        assert!(o.markup && o.block == Block::Normal);
        assert!(pc(t, "</div>").markup);
        assert_eq!(markup_str(t), "<div align=\"center\">|</div>");
        covers(t);
    }

    #[test]
    fn analys_div_nästlade_innersta_vinner() {
        let t = "<div align=\"center\">\n\nA\n\n<div align=\"right\">\n\nB\n\n</div>\n\nC\n\n</div>\n";
        assert_eq!(pc(t, "A").align, Align::Center);
        assert_eq!(pc(t, "B").align, Align::Right);
        assert_eq!(pc(t, "C").align, Align::Center);
        assert_eq!(analyze(t).pieces.iter().filter(|p| p.markup).count(), 4);
        covers(t);
    }

    #[test]
    fn analys_div_utan_tomma_rader_och_oparad() {
        let t = "<div align=\"right\">\nrad ett\nrad två\n</div>\n";
        assert_eq!(pc(t, "rad ett").align, Align::Right);
        assert!(pc(t, "<div align=\"right\">").markup);
        // Oparad div är vanlig text.
        let t = "<div align=\"right\">\n\ntext\n";
        assert_eq!(pc(t, "text").align, Align::Left);
        assert!(!analyze(t).pieces.iter().any(|p| p.markup));
        let t = "text\n\n</div>\n";
        assert!(!analyze(t).pieces.iter().any(|p| p.markup));
    }

    #[test]
    fn analys_grundformat_i_kommentar() {
        let t = "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt -->\n\ntext\n";
        let a = analyze(t);
        assert_eq!(a.defaults.font_family.as_deref(), Some("Times New Roman"));
        assert_eq!(a.defaults.font_size, Some(12.0));
        assert!(a.pieces[0].markup);
        assert_eq!(markup_str(t), "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt -->");
        covers(t);
        // Saknas raden är det standard; andra kommentarer och andra rader är vanlig text.
        assert!(analyze("text\n").defaults.is_empty());
        let t = "<!-- en annan kommentar -->\n\ntext\n";
        assert!(analyze(t).defaults.is_empty());
        assert!(!analyze(t).pieces.iter().any(|p| p.markup));
        let t = "text\n\n<!-- kladd: font-size: 20pt -->\n";
        assert!(analyze(t).defaults.is_empty());
        assert!(!analyze(t).pieces.iter().any(|p| p.markup));
    }

    #[test]
    fn räkna_ignorerar_nya_taggar() {
        let t = format!("<!-- kladd: font-size: 12pt -->\n\n<div align=\"center\">\n\n{RED}två ord</span>\n\n</div>\n");
        let (words, chars) = count(&t, &analyze(&t));
        assert_eq!(words, 2);
        assert_eq!(chars, "två ord".chars().count());
    }

    #[test]
    fn active_span_enhetlig_och_blandad() {
        let t = format!("{RED}röd</span> och {RED}röd2</span>");
        let a = analyze(&t);
        // Vid markören.
        let i = t.find("röd").unwrap() + 2;
        assert_eq!(active(&t, &a, i..i).span.color, Some([255, 0, 0]));
        let j = t.find("och").unwrap() + 1;
        assert_eq!(active(&t, &a, j..j).span.color, None);
        // Enhetlig över markeringen (ordet, utan det mellanliggande).
        let w = t.find("röd").unwrap()..t.find("</span>").unwrap();
        assert_eq!(active(&t, &a, w).span, red());
        // Blandad: röd + ingen stil.
        let all = 0..t.len();
        assert_eq!(active(&t, &a, all).span.color, None);
        // Två olika färger men samma storlek.
        let t = "<span style=\"color: #FF0000; font-size: 12pt\">a</span><span style=\"color: #0000FF; font-size: 12pt\">b</span>";
        let a = analyze(t);
        let sp = active(t, &a, 0..t.len()).span;
        assert_eq!((sp.color, sp.font_size), (None, Some(12.0)));
    }

    #[test]
    fn active_justering_på_markörens_rad() {
        let t = "vänster\n\n<div align=\"center\">\n\nmitten\n\n</div>\n";
        let a = analyze(t);
        let i = t.find("mitten").unwrap() + 2;
        assert_eq!(active(t, &a, i..i).align, Align::Center);
        assert_eq!(active(t, &a, 2..2).align, Align::Left);
        // Markeringens början avgör.
        assert_eq!(active(t, &a, i..t.len()).align, Align::Center);
        assert_eq!(active(t, &a, 0..i).align, Align::Left);
    }

    // ---- set_span_prop

    fn sp_edit(t: &str, needle: &str, prop: SpanProp) -> Edit {
        let i = t.find(needle).unwrap();
        set_span_prop(t, i..i + needle.len(), prop)
    }

    #[test]
    fn span_sätt_färg_på_markering() {
        let t = "ab cd ef";
        let r = sp_edit(t, "cd", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("ab {RED}cd</span> ef"));
        assert_eq!(&r.text[r.sel.clone()], "cd");
        assert_eq!(color_of(&r.text, "cd"), Some([255, 0, 0]));
        covers(&r.text);
    }

    #[test]
    fn span_tom_markering_tar_ordet() {
        let t = "ab cd ef";
        let r = set_span_prop(t, 4..4, SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("ab {RED}cd</span> ef"));
        // Inget ord: oförändrad.
        let r = set_span_prop("ab  cd", 3..3, SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, "ab  cd");
        let r = set_span_prop("", 0..0, SpanProp::FontSize(Some(14.0)));
        assert_eq!(r.text, "");
    }

    #[test]
    fn span_flera_egenskaper_i_en_tagg() {
        let t = "ab";
        let r = set_span_prop(t, 0..2, SpanProp::FontFamily(Some("Times New Roman".into())));
        let r = set_span_prop(&r.text, r.sel, SpanProp::FontSize(Some(14.0)));
        let r = set_span_prop(&r.text, r.sel, SpanProp::Background(Some([255, 255, 0])));
        assert_eq!(
            r.text,
            "<span style=\"font-family: 'Times New Roman'; font-size: 14pt; background-color: #FFFF00\">ab</span>"
        );
        assert_eq!(&r.text[r.sel.clone()], "ab");
    }

    #[test]
    fn span_dela_befintlig_span() {
        let t = format!("{RED}ab</span>");
        let r = sp_edit(&t, "b", SpanProp::FontSize(Some(14.0)));
        assert_eq!(
            r.text,
            format!("{RED}a</span><span style=\"font-size: 14pt; color: #FF0000\">b</span>")
        );
        assert_eq!(&r.text[r.sel.clone()], "b");
        covers(&r.text);
        // Mitten av tre.
        let t = format!("{RED}abc</span>");
        let r = sp_edit(&t, "b", SpanProp::Color(Some([0, 0, 255])));
        assert_eq!(
            r.text,
            format!("{RED}a</span><span style=\"color: #0000FF\">b</span>{RED}c</span>")
        );
    }

    #[test]
    fn span_byt_värde_på_hel_span() {
        let t = format!("x {RED}ab</span> y");
        let r = sp_edit(&t, "ab", SpanProp::Color(Some([0, 0, 255])));
        assert_eq!(r.text, "x <span style=\"color: #0000FF\">ab</span> y");
    }

    #[test]
    fn span_ta_bort_egenskap_packar_upp_tom_span() {
        let t = format!("x {RED}ab</span> y");
        let r = sp_edit(&t, "ab", SpanProp::Color(None));
        assert_eq!(r.text, "x ab y");
        assert_eq!(&r.text[r.sel.clone()], "ab");
        assert!(analyze(&r.text).pieces.iter().all(|p| !p.markup));
        // En kvarvarande egenskap behåller spanen.
        let t = "<span style=\"font-size: 14pt; color: #FF0000\">ab</span>";
        let r = sp_edit(t, "ab", SpanProp::Color(None));
        assert_eq!(r.text, "<span style=\"font-size: 14pt\">ab</span>");
        // Ta bort ur en del: resten behåller stilen.
        let r = sp_edit(&format!("{RED}abc</span>"), "b", SpanProp::Color(None));
        assert_eq!(r.text, format!("{RED}a</span>b{RED}c</span>"));
    }

    #[test]
    fn span_intilliggande_med_samma_stil_slås_ihop() {
        let t = format!("{RED}a</span>b");
        let r = sp_edit(&t, "b", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("{RED}ab</span>"));
        let t = format!("a{RED}b</span>");
        let r = sp_edit(&t, "a", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("{RED}ab</span>"));
        // Mellan två lika spans.
        let t = format!("{RED}a</span>b{RED}c</span>");
        let r = sp_edit(&t, "b", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("{RED}abc</span>"));
    }

    #[test]
    fn span_ligger_utanför_betoning_som_ryms() {
        let t = "a **fet** b";
        let r = set_span_prop(t, 2..9, SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("a {RED}**fet**</span> b"));
        // Bara ordet inuti: spanen utanför.
        let r = sp_edit(t, "fet", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("a {RED}**fet**</span> b"));
        assert_eq!(&r.text[r.sel.clone()], "fet");
        let p = pc(&r.text, "fet");
        assert!(p.bold && p.span.is_some());
        covers(&r.text);
        // Understruken och genomstruken.
        let r = sp_edit("<u>x</u> ~~y~~", "y", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("<u>x</u> {RED}~~y~~</span>"));
    }

    #[test]
    fn span_skär_genom_betoning() {
        let t = "**xqw**";
        let r = sp_edit(t, "q", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("**x**{RED}**q**</span>**w**"));
        assert_eq!(&r.text[r.sel.clone()], "q");
        for (n, c) in [("x", false), ("q", true), ("w", false)] {
            let p = pc(&r.text, n);
            assert!(p.bold, "{n}");
            assert_eq!(p.span.is_some(), c, "{n}");
        }
        // Början och slutet.
        let r = sp_edit(t, "xq", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("{RED}**xq**</span>**w**"));
        let r = sp_edit(t, "qw", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("**x**{RED}**qw**</span>"));
        // Hela innehållet.
        let r = sp_edit(t, "xqw", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("{RED}**xqw**</span>"));
    }

    #[test]
    fn span_skär_genom_betoning_med_blanksteg() {
        // Stängningen flyttas före blanksteget så att `**` fortfarande är en giltig stängning.
        let t = "**xq wk**";
        let r = sp_edit(t, "wk", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("**xq** {RED}**wk**</span>"));
        assert!(pc(&r.text, "xq").bold && pc(&r.text, "wk").bold);
        let r = sp_edit(t, "xq", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("{RED}**xq**</span> **wk**"));
        assert!(pc(&r.text, "wk").bold && pc(&r.text, "wk").span.is_none());
        // Kursiv inuti fet.
        let t = "**x *q* w**";
        let r = sp_edit(t, "q", SpanProp::Color(Some([255, 0, 0])));
        let p = pc(&r.text, "q");
        assert!(p.bold && p.italic && p.span.is_some());
        assert!(!pc(&r.text, "x").italic && pc(&r.text, "x").span.is_none());
        assert!(pc(&r.text, "w").bold && pc(&r.text, "w").span.is_none());
    }

    #[test]
    fn span_rundtur_med_betoning() {
        let t = "**xqw**";
        let r = sp_edit(t, "q", SpanProp::Color(Some([255, 0, 0])));
        let r2 = set_span_prop(&r.text, r.sel.clone(), SpanProp::Color(None));
        assert_eq!(r2.text, t);
        assert_eq!(&r2.text[r2.sel.clone()], "q");
    }

    #[test]
    fn span_rundtur_enkel() {
        for (t, n) in [("ab cd ef", "cd"), ("åäö ÅÄÖ", "ÅÄÖ"), ("ett *två* tre", "två"), ("- punkt ett\n- punkt två\n", "ett")] {
            let i = t.find(n).unwrap();
            let r = set_span_prop(t, i..i + n.len(), SpanProp::Color(Some([255, 0, 0])));
            assert_ne!(r.text, t);
            assert_eq!(&r.text[r.sel.clone()], n);
            assert!(analyze(&r.text).pieces.iter().any(|p| p.span.is_some()));
            let back = set_span_prop(&r.text, r.sel.clone(), SpanProp::Color(None));
            assert_eq!(back.text, t, "rundtur {t:?}");
            assert_eq!(&back.text[back.sel.clone()], n);
        }
    }

    #[test]
    fn span_flera_rader() {
        let t = "ett\ntvå\ntre";
        let r = set_span_prop(t, 0..t.len(), SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("{RED}ett</span>\n{RED}två</span>\n{RED}tre</span>"));
        assert_eq!(&r.text[r.sel.clone()], format!("ett</span>\n{RED}två</span>\n{RED}tre"));
        covers(&r.text);
        // Hoppar över blockprefix.
        let t = "# Rubrik\n\n- punkt\n\n> citat\n";
        let r = set_span_prop(t, 0..t.len(), SpanProp::FontSize(Some(16.0)));
        let sz = "<span style=\"font-size: 16pt\">";
        assert_eq!(
            r.text,
            format!("# {sz}Rubrik</span>\n\n- {sz}punkt</span>\n\n> {sz}citat</span>\n")
        );
        assert_eq!(pc(&r.text, "Rubrik").block, Block::Heading(1));
        assert_eq!(pc(&r.text, "punkt").span.as_ref().and_then(|s| s.font_size), Some(16.0));
    }

    #[test]
    fn span_markering_över_span_gräns() {
        // Börjar före en span och slutar mitt i den.
        let t = format!("x {RED}qw</span> k");
        let r = set_span_prop(&t, 0..t.find('w').unwrap(), SpanProp::FontSize(Some(14.0)));
        let a = analyze(&r.text);
        let sz = |n: &str| pc(&r.text, n).span.as_ref().and_then(|s| s.font_size);
        assert_eq!(sz("x"), Some(14.0));
        assert_eq!(sz("q"), Some(14.0));
        assert_eq!(color_of(&r.text, "q"), Some([255, 0, 0]));
        assert_eq!(color_of(&r.text, "x"), None);
        assert_eq!(sz("w"), None);
        assert_eq!(color_of(&r.text, "w"), Some([255, 0, 0]));
        assert_eq!(sz("k"), None);
        assert!(a.pieces.len() > 4);
        covers(&r.text);
    }

    #[test]
    fn span_nästlade_plattas_ut() {
        let t = "<span style=\"color: #FF0000\">x <span style=\"font-size: 12pt\">q</span> w</span>";
        let r = sp_edit(t, "q", SpanProp::Color(Some([0, 0, 255])));
        let p = |n: &str| pc(&r.text, n).span.clone().map(|s| (s.color, s.font_size));
        assert_eq!(p("x "), Some((Some([255, 0, 0]), None)));
        assert_eq!(p("q"), Some((Some([0, 0, 255]), Some(12.0))));
        assert_eq!(p(" w"), Some((Some([255, 0, 0]), None)));
        covers(&r.text);
    }

    #[test]
    fn span_i_tabell_per_cell_och_hoppar_över_kod() {
        let t = "| a | b |\n| --- | --- |\n| c | d |\n";
        let r = set_span_prop(t, 0..t.len(), SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(
            r.text,
            format!("| {RED}a</span> | {RED}b</span> |\n| --- | --- |\n| {RED}c</span> | {RED}d</span> |\n")
        );
        assert_eq!(analyze(&r.text).pieces.iter().filter(|p| p.block == Block::Table && p.span.is_some() && !p.markup).count(), 4);
        let t = "```\nkod\n```\n";
        assert_eq!(set_span_prop(t, 0..t.len(), SpanProp::Color(Some([255, 0, 0]))).text, t);
    }

    #[test]
    fn span_rör_inte_inline_kod_länkar_eller_div_rader() {
        // Delad kod tas med hel.
        let t = "se `kod` här";
        let r = set_span_prop(t, 5..7, SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("se {RED}`kod`</span> här"));
        // Länkadress delas inte.
        let t = "[länk](http://x.se) slut";
        let r = set_span_prop(t, 0..t.find("http").unwrap() + 2, SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("{RED}[länk](http://x.se)</span> slut"));
        // Div-rader formateras inte.
        let t = "<div align=\"center\">\n\ntext\n\n</div>\n";
        let r = set_span_prop(t, 0..t.len(), SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("<div align=\"center\">\n\n{RED}text</span>\n\n</div>\n"));
        assert_eq!(pc(&r.text, "text").align, Align::Center);
    }

    #[test]
    fn span_åäö_och_felaktiga_offset() {
        let t = "åäö ÖÄÅ";
        let r = set_span_prop(t, 1..1, SpanProp::Color(Some([255, 0, 0]))); // mitt i å: ordet
        assert_eq!(r.text, format!("{RED}åäö</span> ÖÄÅ"));
        let r = set_span_prop(t, 1..4, SpanProp::Color(Some([255, 0, 0]))); // börjar mitt i å
        assert_eq!(r.text, format!("{RED}åä</span>ö ÖÄÅ"));
        let r = set_span_prop(t, 100..200, SpanProp::Color(Some([255, 0, 0])));
        assert!(analyze(&r.text).pieces.len() >= 1);
        covers(&r.text);
    }

    // ---- set_align

    fn al(t: &str, needle: &str, a: Align) -> Edit {
        let i = t.find(needle).unwrap();
        set_align(t, i..i + needle.len(), a)
    }

    const CENTER: &str = "<div align=\"center\">";

    #[test]
    fn align_omslut_stycke() {
        let t = "a\n\nb\n\nc";
        let r = al(t, "b", Align::Center);
        assert_eq!(r.text, format!("a\n\n{CENTER}\n\nb\n\n</div>\n\nc"));
        assert_eq!(&r.text[r.sel.clone()], "b");
        assert_eq!(pc(&r.text, "b").align, Align::Center);
        assert_eq!(pc(&r.text, "a").align, Align::Left);
        covers(&r.text);
        // Markör i stycket, i början och i slutet av texten.
        let r = set_align("hej", 1..1, Align::Right);
        assert_eq!(r.text, "<div align=\"right\">\n\nhej\n\n</div>");
        assert_eq!(r.sel, r.text.find("hej").unwrap() + 1..r.text.find("hej").unwrap() + 1);
    }

    #[test]
    fn align_flera_stycken_en_div() {
        let t = "a\n\nb\n\nc\n\nd";
        let i = t.find('b').unwrap();
        let j = t.find('c').unwrap() + 1;
        let r = set_align(t, i..j, Align::Justify);
        assert_eq!(r.text, "a\n\n<div align=\"justify\">\n\nb\n\nc\n\n</div>\n\nd");
        assert_eq!(&r.text[r.sel.clone()], "b\n\nc");
    }

    #[test]
    fn align_byt_befintlig() {
        let t = format!("{CENTER}\n\nmitten\n\n</div>\n");
        let r = al(&t, "mitten", Align::Right);
        assert_eq!(r.text, "<div align=\"right\">\n\nmitten\n\n</div>\n");
        assert_eq!(&r.text[r.sel.clone()], "mitten");
        assert_eq!(pc(&r.text, "mitten").align, Align::Right);
        // Samma justering: oförändrat.
        let r = al(&t, "mitten", Align::Center);
        assert_eq!(r.text, t);
    }

    #[test]
    fn align_vänster_tar_bort_div() {
        let t = format!("före\n\n{CENTER}\n\nmitten\n\n</div>\n\nefter\n");
        let r = al(&t, "mitten", Align::Left);
        assert_eq!(r.text, "före\n\nmitten\n\nefter\n");
        assert!(analyze(&r.text).pieces.iter().all(|p| !p.markup));
        assert_eq!(&r.text[r.sel.clone()], "mitten");
        // Vänster på vanlig text är ingen åtgärd.
        assert_eq!(al("a\n\nb", "b", Align::Left).text, "a\n\nb");
    }

    #[test]
    fn align_dela_div() {
        let t = format!("{CENTER}\n\nxa\n\nxb\n\nxc\n\n</div>\n");
        let r = al(&t, "xb", Align::Right);
        assert_eq!(
            r.text,
            format!("{CENTER}\n\nxa\n\n</div>\n\n<div align=\"right\">\n\nxb\n\n</div>\n\n{CENTER}\n\nxc\n\n</div>\n")
        );
        assert_eq!(&r.text[r.sel.clone()], "xb");
        let al_of = |n: &str| pc(&r.text, n).align;
        assert_eq!((al_of("xa"), al_of("xb"), al_of("xc")), (Align::Center, Align::Right, Align::Center));
        covers(&r.text);
        // Första stycket tas ut: ingen tom före-del.
        let r = al(&t, "xa", Align::Left);
        assert_eq!(r.text, format!("xa\n\n{CENTER}\n\nxb\n\nxc\n\n</div>\n"));
        // Sista stycket.
        let r = al(&t, "xc", Align::Left);
        assert_eq!(r.text, format!("{CENTER}\n\nxa\n\nxb\n\n</div>\n\nxc\n"));
    }

    #[test]
    fn align_slå_ihop_med_intilliggande() {
        let t = format!("{CENTER}\n\na\n\n</div>\n\nb\n\n{CENTER}\n\nc\n\n</div>\n");
        let r = al(&t, "b", Align::Center);
        assert_eq!(r.text, format!("{CENTER}\n\na\n\nb\n\nc\n\n</div>\n"));
        assert_eq!(&r.text[r.sel.clone()], "b");
        covers(&r.text);
        // Bara ena sidan.
        let t = format!("{CENTER}\n\na\n\n</div>\n\nb\n\nc");
        let r = al(&t, "b", Align::Center);
        assert_eq!(r.text, format!("{CENTER}\n\na\n\nb\n\n</div>\n\nc"));
        // Något annat emellan: ingen sammanslagning.
        let t = format!("{CENTER}\n\na\n\n</div>\n\nx\n\nb");
        let r = al(&t, "b", Align::Center);
        assert_eq!(r.text, format!("{CENTER}\n\na\n\n</div>\n\nx\n\n{CENTER}\n\nb\n\n</div>"));
        // Annan justering slås inte ihop.
        let t = format!("{CENTER}\n\na\n\n</div>\n\nb");
        let r = al(&t, "b", Align::Right);
        assert_eq!(r.text, format!("{CENTER}\n\na\n\n</div>\n\n<div align=\"right\">\n\nb\n\n</div>"));
    }

    #[test]
    fn align_lista_är_ett_block() {
        let t = "text\n\n- ett\n- två\n- tre\n\nslut";
        // Markeringen rör bara ett objekt, hela listan omsluts.
        let r = al(t, "två", Align::Center);
        assert_eq!(r.text, format!("text\n\n{CENTER}\n\n- ett\n- två\n- tre\n\n</div>\n\nslut"));
        assert_eq!(&r.text[r.sel.clone()], "två");
        assert_eq!(pc(&r.text, "tre").align, Align::Center);
        assert_eq!(pc(&r.text, "tre").block, Block::Bullet);
        assert_eq!(pc(&r.text, "slut").align, Align::Left);
        // Lös lista (tomma rader mellan objekt) är fortfarande ett block.
        let t = "1. a\n\n2. b\n\nslut";
        let r = al(t, "a", Align::Right);
        assert_eq!(r.text, "<div align=\"right\">\n\n1. a\n\n2. b\n\n</div>\n\nslut");
    }

    #[test]
    fn align_rubrik_citat_och_blanka_rader() {
        let t = "# Rubrik\n\n> citat\n\ntext";
        let r = al(t, "Rubrik", Align::Center);
        assert_eq!(r.text, format!("{CENTER}\n\n# Rubrik\n\n</div>\n\n> citat\n\ntext"));
        assert_eq!(pc(&r.text, "Rubrik").block, Block::Heading(1));
        assert_eq!(pc(&r.text, "Rubrik").align, Align::Center);
        assert_eq!(pc(&r.text, "citat").align, Align::Left);
        // Rubrik direkt följd av stycke utan tom rad: tomma rader runt divven.
        let t = "# Rubrik\ntext\n";
        let r = al(t, "text", Align::Right);
        assert_eq!(r.text, "# Rubrik\n\n<div align=\"right\">\n\ntext\n\n</div>\n");
        // Markering i tomrad mellan stycken berör inget.
        let t = "a\n\n\n\nb";
        assert_eq!(set_align(t, 2..2, Align::Center).text, t);
    }

    #[test]
    fn align_nästlad_div_ändrar_den_innersta() {
        let t = format!("{CENTER}\n\nA\n\n<div align=\"right\">\n\nB\n\n</div>\n\n</div>\n");
        let r = al(&t, "B", Align::Left);
        // Vänster betyder verkligen vänster: B hamnar utanför den yttre divven.
        assert_eq!(r.text, format!("{CENTER}\n\nA\n\n</div>\n\nB\n"));
        assert_eq!(pc(&r.text, "B").align, Align::Left);
        assert_eq!(pc(&r.text, "A").align, Align::Center);
    }

    #[test]
    fn align_markering_över_flera_block_med_div_emellan() {
        let t = format!("a\n\n{CENTER}\n\nb\n\n</div>\n\nc\n");
        let r = set_align(&t, 0..t.rfind('c').unwrap() + 1, Align::Right);
        assert_eq!(r.text, "<div align=\"right\">\n\na\n\nb\n\nc\n\n</div>\n");
        assert_eq!(&r.text[r.sel.clone()], "a\n\nb\n\nc");
        covers(&r.text);
    }

    #[test]
    fn align_grundformatkommentaren_rörs_inte() {
        let t = "<!-- kladd: font-size: 12pt -->\n\ntext\n";
        let r = al(t, "text", Align::Center);
        assert_eq!(r.text, format!("<!-- kladd: font-size: 12pt -->\n\n{CENTER}\n\ntext\n\n</div>\n"));
        assert_eq!(analyze(&r.text).defaults.font_size, Some(12.0));
        // Markering som bara täcker kommentaren: ingen åtgärd.
        assert_eq!(set_align(t, 0..5, Align::Center).text, t);
    }

    #[test]
    fn align_rundtur_och_omanalys() {
        for a in [Align::Center, Align::Right, Align::Justify] {
            let t = "ett\n\ntvå **fet**\n\ntre";
            let r = al(t, "två", a);
            assert_eq!(pc(&r.text, "två").align, a);
            assert!(pc(&r.text, "fet").bold);
            let back = set_align(&r.text, r.sel.clone(), Align::Left);
            assert_eq!(back.text, t);
            assert_eq!(&back.text[back.sel.clone()], "två");
        }
    }

    #[test]
    fn align_åäö_och_felaktiga_offset() {
        let t = "åäö\n\nöl";
        let r = set_align(t, 1..1, Align::Center);
        assert_eq!(r.text, format!("{CENTER}\n\nåäö\n\n</div>\n\nöl"));
        assert!(r.text.is_char_boundary(r.sel.start));
        assert_eq!(set_align("", 0..0, Align::Center).text, "");
        let r = set_align(t, 100..200, Align::Center);
        covers(&r.text);
    }

    #[test]
    fn span_och_justering_tillsammans() {
        let t = "a\n\nb";
        let r = al(t, "b", Align::Center);
        let r = sp_edit(&r.text, "b", SpanProp::Color(Some([255, 0, 0])));
        assert_eq!(r.text, format!("a\n\n{CENTER}\n\n{RED}b</span>\n\n</div>"));
        let p = pc(&r.text, "b");
        assert!(p.span.is_some() && p.align == Align::Center);
        let a = active(&r.text, &analyze(&r.text), r.sel.clone());
        assert_eq!((a.span, a.align), (red(), Align::Center));
    }

    #[test]
    fn nya_åtgärder_tål_alla_markeringar() {
        let docs = [
            "**abc** *d* <u>e f</u>\n\n- punkt `kod` [l](http://x.se)\n\n| a | b |\n| - | - |\n| c | d |\n",
            "<div align=\"center\">\n\nåäö <span style=\"color: #FF0000\">r**ö**d</span>\n\n</div>\n\n> citat ![b](x.png)\n",
            "<!-- kladd: font-size: 12pt -->\n\n**ab\ncd** ef\n\n```\nkod\n```\n",
        ];
        for d in docs {
            let idx: Vec<usize> = d.char_indices().map(|(i, _)| i).chain([d.len()]).collect();
            for &s in &idx {
                for &e in idx.iter().filter(|&&e| e >= s) {
                    let r = set_span_prop(d, s..e, SpanProp::Color(Some([1, 2, 3])));
                    covers(&r.text);
                    assert!(r.text.is_char_boundary(r.sel.start) && r.text.is_char_boundary(r.sel.end));
                    let r = set_span_prop(d, s..e, SpanProp::FontSize(None));
                    covers(&r.text);
                    let r = set_align(d, s..e, Align::Right);
                    covers(&r.text);
                    assert!(r.text.is_char_boundary(r.sel.start) && r.sel.start <= r.sel.end);
                }
            }
        }
    }

    // ---- bilder (HTML) och objektmarkörer

    fn iref(alt: &str, src: &str, width: Option<f32>, height: Option<f32>) -> ImageRef {
        ImageRef { alt: alt.into(), src: src.into(), width, height }
    }

    fn at(t: &str, needle: &str) -> usize {
        t.find(needle).unwrap_or_else(|| panic!("{needle:?} saknas"))
    }

    #[test]
    fn img_tagg_inline() {
        let tag = "<img src=\"x.png\" alt=\"Sol\" width=\"240\">";
        let t = format!("a {tag} b");
        let p = pc(&t, tag);
        assert!(p.markup);
        assert_eq!(p.range.len(), tag.len());
        assert_eq!(p.image, Some(iref("Sol", "x.png", Some(240.0), None)));
        covers(&t);
    }

    #[test]
    fn img_tagg_varianter() {
        let check = |tag: &str, want: Option<ImageRef>| {
            let t = format!("ord {tag} ord");
            assert_eq!(pc(&t, tag).image, want, "{tag}");
        };
        // Utan citattecken, annan ordning, px-enhet och höjd.
        check("<img width=100px height=50 alt=Sol src=x.png>", Some(iref("Sol", "x.png", Some(100.0), Some(50.0))));
        // Enkla citattecken, versaler, mellanslag i adressen.
        check("<IMG SRC='a b.png' ALT='Hej'>", Some(iref("Hej", "a b.png", None, None)));
        // Självstängande, utan alt.
        check("<img src=\"x.png\" />", Some(iref("", "x.png", None, None)));
        check("<img src=x.png/>", Some(iref("", "x.png", None, None)));
        // Entiteter i alt, '>' inom citattecken.
        check("<img src=\"x.png\" alt=\"Tom &amp; &quot;Jerry&quot; &gt;\">", Some(iref("Tom & \"Jerry\" >", "x.png", None, None)));
        // Procent och skräp ger ingen storlek.
        check("<img src=\"x.png\" width=\"50%\" height=\"abc\">", Some(iref("", "x.png", None, None)));
        // Utan src är det ingen bild.
        check("<img alt=\"x\">", None);
        // Annan tagg som börjar likadant.
        check("<imgx src=\"a\">", None);
    }

    #[test]
    fn img_tagg_i_htmlblock() {
        let t = "före\n\n<img src=\"a.png\" width=\"10\">\n\nefter";
        let p = pc(t, "<img");
        assert_eq!(p.image, Some(iref("", "a.png", Some(10.0), None)));
        assert!(p.markup);
        assert_eq!(p.block, Block::Normal);
        assert_eq!(&t[p.range.clone()], "<img src=\"a.png\" width=\"10\">");
        // Två taggar på en rad: två bilder med blanksteget emellan som vanlig text.
        let t = "<img src=\"a.png\"> <img src=\"b.png\">";
        let a = analyze(t);
        assert_eq!(a.pieces.len(), 3);
        assert_eq!(a.pieces[0].image.as_ref().unwrap().src, "a.png");
        assert!(a.pieces[1].image.is_none() && !a.pieces[1].markup);
        assert_eq!(a.pieces[2].image.as_ref().unwrap().src, "b.png");
        // Rad med annan text blir inte bild i blocket; nästa rad är det fortfarande.
        let t = "<img src=\"a.png\">\n<img src=\"b.png\"> text";
        let a = analyze(t);
        assert_eq!(a.pieces.iter().filter(|p| p.image.is_some()).count(), 1);
        covers(t);
    }

    #[test]
    fn img_i_formaterad_text() {
        let t = "**fet <img src=\"a.png\" width=\"5\"> fet**";
        let p = pc(t, "<img");
        assert!(p.bold && p.markup && p.image.is_some());
        covers(t);
    }

    #[test]
    fn objektmarkör_inline() {
        let t = "före<!--k:12-->efter";
        let a = analyze(t);
        assert_eq!(a.pieces.len(), 3);
        let m = &a.pieces[1];
        assert_eq!(m.object, Some(12));
        assert!(m.markup);
        assert_eq!(&t[m.range.clone()], "<!--k:12-->");
        assert!(a.pieces[0].object.is_none() && a.pieces[2].object.is_none());
        assert!(!a.pieces[0].markup);
        // Två olika markörer intill varandra är två bitar.
        let t = "<!--k:1--><!--k:2-->";
        let a = analyze(t);
        assert_eq!(a.pieces.iter().map(|p| p.object).collect::<Vec<_>>(), vec![Some(1), Some(2)]);
        // Formatering går igenom markören.
        let t = "**a<!--k:3-->b**";
        let a = analyze(t);
        let m = a.pieces.iter().find(|p| p.object == Some(3)).unwrap();
        assert!(m.bold && m.markup);
        covers(t);
    }

    #[test]
    fn objektmarkör_block() {
        let t = "text\n\n<!--k:7-->\n\nmer text";
        let p = pc(t, "<!--k:7-->");
        assert_eq!(p.object, Some(7));
        assert!(p.markup);
        assert_eq!(p.block, Block::Normal);
        assert_eq!(&t[p.range.clone()], "<!--k:7-->");
        // Markör och bild på samma rad.
        let t = "<!--k:1--> <img src=\"a.png\">";
        let a = analyze(t);
        assert_eq!(a.pieces[0].object, Some(1));
        assert!(a.pieces.last().unwrap().image.is_some());
        covers(t);
    }

    #[test]
    fn objektmarkör_bara_exakt_form() {
        for t in ["a <!--k: 3--> b", "a <!--k:x--> b", "a <!--k:--> b", "a <!-- k:3 --> b", "a <!--k:3 --> b"] {
            let a = analyze(t);
            assert!(a.pieces.iter().all(|p| p.object.is_none()), "{t}");
        }
    }

    #[test]
    fn count_ignorerar_markörer_och_bilder() {
        assert_eq!(count("ett<!--k:1--> två", &analyze("ett<!--k:1--> två")), (2, 7));
        let t = "ett <img src=\"a.png\" width=\"4\"> två\n\n<!--k:2-->";
        assert_eq!(count(t, &analyze(t)), (2, 8));
    }

    #[test]
    fn bildstorlek_rundtur() {
        let t = "före\n\n![Sol](bild:image1.png)\n\nefter";
        let r = pc(t, "![Sol]").range;
        let e = set_image_size(t, r, Some(240.4));
        let tag = "<img src=\"bild:image1.png\" alt=\"Sol\" width=\"240\">";
        assert_eq!(e.text, format!("före\n\n{tag}\n\nefter"));
        assert_eq!(&e.text[e.sel.clone()], tag);
        assert_eq!(pc(&e.text, tag).image, Some(iref("Sol", "bild:image1.png", Some(240.0), None)));
        // Ny bredd på en <img>, sedan tillbaka till Markdown.
        let e2 = set_image_size(&e.text, e.sel.clone(), Some(100.0));
        assert!(e2.text.contains("width=\"100\">") && !e2.text.contains("240"));
        let e3 = set_image_size(&e2.text, e2.sel.clone(), None);
        assert_eq!(e3.text, t);
        assert_eq!(&e3.text[e3.sel.clone()], "![Sol](bild:image1.png)");
        covers(&e.text);
        covers(&e3.text);
    }

    #[test]
    fn bildstorlek_höjd_skalas_med() {
        let t = "<img src=\"a.png\" alt=\"\" width=\"200\" height=\"100\">";
        let r = 0..t.len();
        let e = set_image_size(t, r.clone(), Some(100.0));
        assert_eq!(e.text, "<img src=\"a.png\" alt=\"\" width=\"100\" height=\"50\">");
        // Höjd utan känd bredd behålls; ingen höjd ger ingen höjd.
        let t2 = "<img src=\"a.png\" height=\"80\">";
        assert_eq!(set_image_size(t2, 0..t2.len(), Some(40.0)).text, "<img src=\"a.png\" alt=\"\" width=\"40\" height=\"80\">");
        // Tillbaka till Markdown tappar höjden.
        assert_eq!(set_image_size(t, r, None).text, "![](a.png)");
    }

    #[test]
    fn bildstorlek_specialfall() {
        // Markdown utan bredd är redan som det ska vara (även med titel): orört.
        let t = "![a](x.png \"titel\")";
        assert_eq!(set_image_size(t, 0..t.len(), None).text, t);
        // Fel område, ogiltig bredd: orört.
        let t = "ord ![a](x.png)";
        assert_eq!(set_image_size(t, 0..3, Some(10.0)).text, t);
        assert_eq!(set_image_size(t, 4..t.len(), Some(0.0)).text, t);
        assert_eq!(set_image_size(t, 4..t.len(), Some(f32::NAN)).text, t);
        // Citattecken och & i alt/src skrivs som entiteter och läses tillbaka.
        let t = "![Tom \"&\" Jerry](a&b.png)";
        let e = set_image_size(t, 0..t.len(), Some(50.0));
        assert_eq!(e.text, "<img src=\"a&amp;b.png\" alt=\"Tom &quot;&amp;&quot; Jerry\" width=\"50\">");
        assert_eq!(set_image_size(&e.text, e.sel, None).text, t);
        // Hakparenteser i alt och blanksteg i adressen.
        let t = "<img src=\"a b.png\" alt=\"a [b]\" width=\"9\">";
        let m = set_image_size(t, 0..t.len(), None);
        assert_eq!(m.text, "![a \\[b\\]](<a b.png>)");
        assert_eq!(pc(&m.text, "![").image, Some(iref("a [b]", "a b.png", None, None)));
        // Svenska tecken före bilden.
        let t = "Rådjur ö ![ä](å.png)";
        let r = pc(t, "![ä]").range;
        let e = set_image_size(t, r, Some(30.0));
        assert_eq!(e.text, "Rådjur ö <img src=\"å.png\" alt=\"ä\" width=\"30\">");
    }

    #[test]
    fn bild_vid_markör() {
        let t = "ord ![Sol](x.png) mer";
        let a = analyze(t);
        let s = at(t, "![");
        let e = s + "![Sol](x.png)".len();
        for pos in [s, s + 3, e] {
            let (r, img) = image_at(t, &a, pos).unwrap_or_else(|| panic!("pos {pos}"));
            assert_eq!(r, s..e);
            assert_eq!(img.alt, "Sol");
        }
        assert!(image_at(t, &a, 1).is_none());
        assert!(image_at(t, &a, e + 2).is_none());
        assert!(image_at(t, &a, 0).is_none());
        assert!(image_at("", &analyze(""), 0).is_none());
        let h = "<img src=\"x.png\" width=\"7\">";
        assert_eq!(image_at(h, &analyze(h), h.len()).unwrap().1.width, Some(7.0));
    }

    #[test]
    fn justering_av_bildstycke() {
        let t = "före\n\n![a](x.png)\n\nefter";
        let s = at(t, "![");
        let e = set_align(t, s..s, Align::Center);
        assert_eq!(e.text, "före\n\n<div align=\"center\">\n\n![a](x.png)\n\n</div>\n\nefter");
        let p = pc(&e.text, "![a]");
        assert_eq!(p.align, Align::Center);
        // Tillbaka.
        let b = set_align(&e.text, e.sel.clone(), Align::Left);
        assert_eq!(b.text, t);
        // Bara en bild i dokumentet, och en <img>-rad.
        let t = "![a](x.png)";
        assert_eq!(set_align(t, 0..0, Align::Right).text, "<div align=\"right\">\n\n![a](x.png)\n\n</div>");
        let t = "<img src=\"x.png\" width=\"20\">";
        let e = set_align(t, 3..3, Align::Center);
        assert_eq!(e.text, "<div align=\"center\">\n\n<img src=\"x.png\" width=\"20\">\n\n</div>");
        assert_eq!(pc(&e.text, "<img").align, Align::Center);
        // Storleksändring inne i en div bevarar justeringen.
        let r = pc(&e.text, "<img").range;
        let e2 = set_image_size(&e.text, r, None);
        assert_eq!(e2.text, "<div align=\"center\">\n\n![](x.png)\n\n</div>");
        covers(&e2.text);
    }

    // ---- tabeller

    const TB: &str = "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |";

    fn cell_text<'a>(text: &'a str, t: &TableInfo) -> Vec<Vec<&'a str>> {
        t.rows.iter().map(|r| r.iter().map(|c| &text[c.clone()]).collect()).collect()
    }

    #[test]
    fn tabell_med_ytterrör() {
        let t = "före\n\n| Namn | Ålder |\n| --- | --- |\n| Åsa | 31 |\n| Örjan | 4 |\n\nefter";
        let ti = table_at(t, at(t, "Åsa")).unwrap();
        assert_eq!(&t[ti.range.clone()], "| Namn | Ålder |\n| --- | --- |\n| Åsa | 31 |\n| Örjan | 4 |");
        assert_eq!(ti.cols, 2);
        assert_eq!(&t[ti.delim_row.clone()], "| --- | --- |");
        assert_eq!(cell_text(t, &ti), vec![vec!["Namn", "Ålder"], vec!["Åsa", "31"], vec!["Örjan", "4"]]);
        // Varje markör på en tabellrad, även precis före radbrytningen och på avgränsningen.
        for pos in [ti.range.start, ti.range.start + 3, at(t, "---"), ti.range.end] {
            assert_eq!(table_at(t, pos).as_ref(), Some(&ti), "pos {pos}");
        }
        // Utanför.
        assert!(table_at(t, 2).is_none());
        assert!(table_at(t, ti.range.end + 1).is_none());
        assert!(table_at(t, t.len()).is_none());
        assert!(table_at("bara text", 3).is_none());
        assert!(table_at("", 0).is_none());
    }

    #[test]
    fn tabell_utan_ytterrör() {
        let t = "Namn | Ålder\n--- | ---\nÅsa | 31\nÖrjan|4";
        let ti = table_at(t, 0).unwrap();
        assert_eq!(ti.cols, 2);
        assert_eq!(cell_text(t, &ti), vec![vec!["Namn", "Ålder"], vec!["Åsa", "31"], vec!["Örjan", "4"]]);
        assert_eq!(ti.range, 0..t.len());
        assert_eq!(&t[ti.delim_row.clone()], "--- | ---");
        // Bara inledande eller bara avslutande rör.
        let t = "| A | B\n--|--\n| 1 | 2";
        assert_eq!(cell_text(t, &table_at(t, 0).unwrap()), vec![vec!["A", "B"], vec!["1", "2"]]);
        let t = "A | B |\n--|--\n1 | 2 |";
        assert_eq!(cell_text(t, &table_at(t, 0).unwrap()), vec![vec!["A", "B"], vec!["1", "2"]]);
    }

    #[test]
    fn tabell_escapade_rör_och_tomma_celler() {
        let t = "| a | b \\| c |\n| --- | --- |\n| x \\\\| y | z \\| |\n|  |  |";
        let ti = table_at(t, 0).unwrap();
        assert_eq!(ti.cols, 2);
        assert_eq!(cell_text(t, &ti)[0], vec!["a", "b \\| c"]);
        // `\\|` är ett escapat snedstreck följt av ett riktigt rör.
        assert_eq!(ti.rows[1].len(), 3);
        assert_eq!(cell_text(t, &ti)[1][0], "x \\\\");
        // Tom cell: tomt område efter det första blanksteget.
        let p = at(t, "|  |") + 2;
        assert_eq!(ti.rows[2], vec![p..p, p + 3..p + 3]);
        let t = "| A | B |\n|---|---|\n||x|";
        let ti = table_at(t, 0).unwrap();
        let p = at(t, "||") + 1;
        assert_eq!(ti.rows[1][0], p..p);
        assert_eq!(&t[ti.rows[1][1].clone()], "x");
    }

    #[test]
    fn tabell_cell_vid_markör() {
        let t = "| Förnamn | Efternamn |\n| --- | --- |\n| Åsa | Öberg |";
        let ti = table_at(t, 0).unwrap();
        assert_eq!(table_cell_at(&ti, at(t, "Förnamn")), Some((0, 0)));
        assert_eq!(table_cell_at(&ti, at(t, "Efternamn") + 2), Some((0, 1)));
        // Före första röret hör till första cellen, efter sista till sista; kanten mellan två
        // celler tillhör cellen markören står i slutet av.
        assert_eq!(table_cell_at(&ti, 0), Some((0, 0)));
        assert_eq!(table_cell_at(&ti, at(t, "|\n|")+1), Some((0, 1)));
        let mid = at(t, "| Efternamn");
        assert_eq!(table_cell_at(&ti, mid), Some((0, 0)));
        assert_eq!(table_cell_at(&ti, mid + 1), Some((0, 1)));
        assert_eq!(table_cell_at(&ti, at(t, "Öberg")), Some((1, 1)));
        // Avgränsningsraden och utanför.
        assert_eq!(table_cell_at(&ti, at(t, "---")), None);
        assert_eq!(table_cell_at(&ti, t.len() + 5), None);
    }

    #[test]
    fn tabell_cellgränser() {
        let t = "| Åsa | 31 |\n| --- | --- |\n|  Öberg  |  |\n|x|";
        let b = |pos| table_cell_bounds(t, pos).map(|r| &t[r]);
        assert_eq!(b(at(t, "Åsa")), Some("Åsa"));
        assert_eq!(b(at(t, "31")), Some("31"));
        // Egna blanksteg ryms i området, bara en utfyllnad på varje sida räknas bort.
        assert_eq!(b(at(t, "Öberg")), Some(" Öberg "));
        // Tom cell: en punkt.
        let tom = at(t, "|  |\n") + 2;
        assert_eq!(table_cell_bounds(t, tom + 1), Some(tom..tom));
        // Utan utfyllnad.
        assert_eq!(b(at(t, "x")), Some("x"));
        // Avgränsningsraden och text utanför.
        assert_eq!(b(at(t, "---")), None);
        assert_eq!(table_cell_bounds("ingen tabell", 3), None);
    }

    #[test]
    fn tabell_nästa_cell() {
        let sel = |e: &Edit| e.text[e.sel.clone()].to_string();
        let next = |pos: usize, back: bool| table_next_cell(TB, pos, back).unwrap();
        // Framåt: i raden, radbyte, texten orörd.
        let e = next(at(TB, "A"), false);
        assert_eq!((e.text.as_str(), sel(&e).as_str()), (TB, "B"));
        assert_eq!(sel(&next(at(TB, "B"), false)), "1");
        assert_eq!(sel(&next(at(TB, "2"), false)), "3");
        // Från avgränsningsraden räknas rubrikraden.
        assert_eq!(sel(&next(at(TB, "---"), false)), "B");
        // Bakåt.
        assert_eq!(sel(&next(at(TB, "2"), true)), "1");
        assert_eq!(sel(&next(at(TB, "1"), true)), "B");
        let e = next(at(TB, "3"), true);
        assert_eq!(sel(&e), "2");
        // Första cellen bakåt stannar.
        let e = next(at(TB, "A"), true);
        assert_eq!((e.text.as_str(), sel(&e).as_str()), (TB, "A"));
        // Sista cellen framåt lägger till en rad; markören i dess första (tomma) cell.
        let e = next(at(TB, "4"), false);
        assert_eq!(e.text, format!("{TB}\n|  |  |"));
        assert_eq!(e.sel, e.text.len() - 7 + 2..e.text.len() - 7 + 2);
        // Utanför en tabell.
        assert!(table_next_cell("text", 2, false).is_none());
        // Tomma celler: markeringen är en punkt i cellen.
        let t = "| A | B |\n| --- | --- |\n|  |  |";
        let e = table_next_cell(t, at(t, "A"), false).unwrap();
        assert_eq!(&e.text[e.sel.clone()], "B");
        let e = table_next_cell(t, at(t, "B"), false).unwrap();
        assert_eq!(e.text, t);
        assert_eq!(e.sel, at(t, "|  |") + 2..at(t, "|  |") + 2);
    }

    #[test]
    fn tabell_nästa_cell_kort_rad_fylls_ut() {
        let t = "| A | B |\n| --- | --- |\n| 1 |";
        let e = table_next_cell(t, at(t, "1"), false).unwrap();
        assert_eq!(e.text, "| A | B |\n| --- | --- |\n| 1 |  |");
        let p = e.text.len() - 2;
        assert_eq!(e.sel, p..p);
        // Utan avslutande rör.
        let t = "A | B\n--|--\n1";
        let e = table_next_cell(t, at(t, "1"), false).unwrap();
        assert_eq!(e.text, "A | B\n--|--\n1 |  |");
        assert_eq!(table_at(&e.text, 0).unwrap().rows[1].len(), 2);
    }

    #[test]
    fn tabell_nästa_cell_svenska_och_slut_utan_radslut() {
        let t = "Text\n\n| Förnamn | Efternamn |\n| --- | --- |\n| Åsa | Öberg |\n\nEfter";
        let e = table_next_cell(t, at(t, "Öberg"), false).unwrap();
        // Ny rad läggs in direkt efter tabellen, före den tomma raden.
        assert_eq!(e.text, "Text\n\n| Förnamn | Efternamn |\n| --- | --- |\n| Åsa | Öberg |\n|  |  |\n\nEfter");
        assert!(table_at(&e.text, e.sel.start).is_some());
        let e = table_next_cell(t, at(t, "Åsa"), false).unwrap();
        assert_eq!(&e.text[e.sel.clone()], "Öberg");
    }

    #[test]
    fn tabell_infoga_rad() {
        // Under en brödrad.
        let e = table_insert_row(TB, at(TB, "1"), true);
        assert_eq!(e.text, "| A | B |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n| 3 | 4 |");
        let p = at(&e.text, "|  |") + 2;
        assert_eq!(e.sel, p..p);
        // Över en brödrad.
        let e = table_insert_row(TB, at(TB, "3"), false);
        assert_eq!(e.text, "| A | B |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n| 3 | 4 |");
        let e = table_insert_row(TB, at(TB, "1"), false);
        assert_eq!(e.text, "| A | B |\n| --- | --- |\n|  |  |\n| 1 | 2 |\n| 3 | 4 |");
        assert_eq!(e.sel.start, at(&e.text, "|  |") + 2);
        // Under sista raden, utan avslutande radbrytning.
        let e = table_insert_row(TB, at(TB, "4"), true);
        assert_eq!(e.text, format!("{TB}\n|  |  |"));
        assert_eq!(e.sel.start, e.text.len() - 7 + 2);
        // På rubrik- och avgränsningsraden blir det första brödraden.
        for (pos, below) in [(at(TB, "A"), true), (at(TB, "A"), false), (at(TB, "---"), true), (at(TB, "---"), false)] {
            let e = table_insert_row(TB, pos, below);
            assert_eq!(e.text, "| A | B |\n| --- | --- |\n|  |  |\n| 1 | 2 |\n| 3 | 4 |");
        }
        // Text efter tabellen och en enkolumnstabell.
        let t = "| A |\n| --- |\n| 1 |\n\nefter";
        let e = table_insert_row(t, at(t, "1"), true);
        assert_eq!(e.text, "| A |\n| --- |\n| 1 |\n|  |\n\nefter");
        // Utanför en tabell.
        let e = table_insert_row("text", 2, true);
        assert_eq!((e.text.as_str(), e.sel), ("text", 2..2));
    }

    #[test]
    fn tabell_infoga_rad_i_tabell_utan_ytterrör() {
        let t = "A | B | C\n--|--|--\n1 | 2 | 3";
        let e = table_insert_row(t, at(t, "1"), true);
        assert_eq!(e.text, "A | B | C\n--|--|--\n1 | 2 | 3\n|  |  |  |");
        assert_eq!(table_at(&e.text, 0).unwrap().rows.len(), 3);
    }

    #[test]
    fn tabell_infoga_kolumn() {
        // Till höger om A: rubrik, avgränsning och tomma celler; övriga celler orörda.
        let e = table_insert_col(TB, at(TB, "A"), true);
        assert_eq!(e.text, "| A | Kolumn 3 | B |\n| --- | --- | --- |\n| 1 |  | 2 |\n| 3 |  | 4 |");
        assert_eq!(&e.text[e.sel.clone()], "Kolumn 3");
        // Till vänster om A.
        let e = table_insert_col(TB, at(TB, "A"), false);
        assert_eq!(e.text, "| Kolumn 3 | A | B |\n| --- | --- | --- |\n|  | 1 | 2 |\n|  | 3 | 4 |");
        // Till höger om sista kolumnen, markören i en brödrad: i dess nya tomma cell.
        let e = table_insert_col(TB, at(TB, "4"), true);
        assert_eq!(e.text, "| A | B | Kolumn 3 |\n| --- | --- | --- |\n| 1 | 2 |  |\n| 3 | 4 |  |");
        assert_eq!(e.sel.start, e.text.len() - 2);
        assert!(e.sel.is_empty());
        // Enkolumnstabell.
        let t = "| A |\n| --- |\n| 1 |";
        assert_eq!(table_insert_col(t, at(t, "A"), true).text, "| A | Kolumn 2 |\n| --- | --- |\n| 1 |  |");
        // Från avgränsningsraden räknas rubrikens kolumn.
        let e = table_insert_col(TB, at(TB, "---"), true);
        assert_eq!(&e.text[e.sel.clone()], "Kolumn 3");
        // Utanför en tabell.
        assert_eq!(table_insert_col("text", 1, true).text, "text");
    }

    #[test]
    fn tabell_infoga_kolumn_utan_ytterrör() {
        let t = "A | B\n--|--\n1 | 2";
        let e = table_insert_col(t, at(t, "B"), true);
        assert_eq!(e.text, "A | B | Kolumn 3 |\n--|-- | --- |\n1 | 2 |  |");
        assert_eq!(table_at(&e.text, 0).unwrap().cols, 3);
        let e = table_insert_col(t, at(t, "A"), false);
        assert_eq!(e.text, "| Kolumn 3 | A | B\n| --- | --|--\n|  | 1 | 2");
        let ti = table_at(&e.text, 0).unwrap();
        assert_eq!(ti.cols, 3);
        assert_eq!(cell_text(&e.text, &ti)[1], vec!["", "1", "2"]);
        // Mitt emellan.
        let e = table_insert_col(t, at(t, "A"), true);
        assert_eq!(cell_text(&e.text, &table_at(&e.text, 0).unwrap())[0], vec!["A", "Kolumn 3", "B"]);
    }

    #[test]
    fn tabell_infoga_kolumn_escapat_rör() {
        let t = "| a \\| b | c |\n| --- | --- |\n| x | y \\| z |";
        let e = table_insert_col(t, at(t, "a"), true);
        assert_eq!(e.text, "| a \\| b | Kolumn 3 | c |\n| --- | --- | --- |\n| x |  | y \\| z |");
    }

    #[test]
    fn tabell_ta_bort_rad() {
        let sel = |e: &Edit| e.text[e.sel.clone()].to_string();
        // Mitt i: raden efter tar platsen, samma kolumn.
        let e = table_delete_row(TB, at(TB, "2"));
        assert_eq!(e.text, "| A | B |\n| --- | --- |\n| 3 | 4 |");
        assert_eq!(sel(&e), "4");
        // Sista raden: markören på den nya sista.
        let e = table_delete_row(TB, at(TB, "3"));
        assert_eq!(e.text, "| A | B |\n| --- | --- |\n| 1 | 2 |");
        assert_eq!(sel(&e), "1");
        // Rubriken: första brödraden blir rubrik.
        let e = table_delete_row(TB, at(TB, "B"));
        assert_eq!(e.text, "| 1 | 2 |\n| --- | --- |\n| 3 | 4 |");
        assert_eq!(sel(&e), "2");
        // Avgränsningsraden räknas som rubriken.
        assert_eq!(table_delete_row(TB, at(TB, "---")).text, "| 1 | 2 |\n| --- | --- |\n| 3 | 4 |");
        // Enda brödraden töms.
        let t = "| A | B |\n| --- | --- |\n| 1 | 2 |";
        let e = table_delete_row(t, at(t, "1"));
        assert_eq!(e.text, "| A | B |\n| --- | --- |\n|  |  |");
        assert!(table_at(&e.text, 0).is_some());
        let p = at(&e.text, "|  |") + 2;
        assert_eq!(e.sel, p..p);
        // Bara rubrik: hela tabellen försvinner.
        let t = "före\n\n| A | B |\n| --- | --- |\n\nefter";
        assert_eq!(table_delete_row(t, at(t, "A")).text, "före\n\nefter");
        // Tabell med text efter, utan ytterrör.
        let t = "A | B\n--|--\n1 | 2\n3 | 4\n\nefter";
        assert_eq!(table_delete_row(t, at(t, "1")).text, "A | B\n--|--\n3 | 4\n\nefter");
        // Utanför.
        assert_eq!(table_delete_row("text", 1).text, "text");
    }

    #[test]
    fn tabell_ta_bort_kolumn() {
        let e = table_delete_col(TB, at(TB, "B"));
        assert_eq!(e.text, "| A |\n| --- |\n| 1 |\n| 3 |");
        assert_eq!(&e.text[e.sel.clone()], "A");
        let e = table_delete_col(TB, at(TB, "3"));
        assert_eq!(e.text, "| B |\n| --- |\n| 2 |\n| 4 |");
        assert_eq!(&e.text[e.sel.clone()], "4");
        // Mellersta av tre; övriga celler orörda.
        let t = "| A | B | C |\n| --- | --- | --- |\n| 1 | 2   | 3 |";
        assert_eq!(table_delete_col(t, at(t, "B")).text, "| A | C |\n| --- | --- |\n| 1 | 3 |");
        // Utan ytterrör: sista kolumnen tar föregående rör med sig.
        let t = "A | B | C\n--|--|--\n1 | 2 | 3";
        assert_eq!(table_delete_col(t, at(t, "C")).text, "A | B \n--|--\n1 | 2 ");
        assert_eq!(table_delete_col(t, at(t, "A")).text, " B | C\n--|--\n 2 | 3");
        // Escapat rör i en cell som blir kvar.
        let t = "| a \\| b | c |\n| --- | --- |\n| x | y |";
        assert_eq!(table_delete_col(t, at(t, "c")).text, "| a \\| b |\n| --- |\n| x |");
        // Från avgränsningsraden.
        assert_eq!(table_delete_col(TB, at(TB, "---")).text, "| B |\n| --- |\n| 2 |\n| 4 |");
    }

    #[test]
    fn tabell_ta_bort_enda_kolumnen_tar_tabellen() {
        let t = "före\n\n| A |\n| --- |\n| 1 |\n\nefter";
        let e = table_delete_col(t, at(t, "1"));
        assert_eq!(e.text, "före\n\nefter");
        assert_eq!(e.sel, 7..7);
    }

    #[test]
    fn tabell_ta_bort_tabell() {
        let mid = "före\n\n| A |\n| --- |\n| 1 |\n\nefter";
        let e = table_delete(mid, at(mid, "1"));
        assert_eq!(e.text, "före\n\nefter");
        assert_eq!(e.sel, 7..7);
        let start = "| A |\n| --- |\n| 1 |\n\nefter";
        assert_eq!(table_delete(start, 2).text, "efter");
        let end = "före\n\n| A |\n| --- |\n| 1 |";
        assert_eq!(table_delete(end, 8).text, "före\n\n");
        assert_eq!(table_delete("| A |\n| --- |", 1).text, "");
        // Tabellen direkt följd av text på nästa stycke efter en tom rad.
        let two = "| A |\n| --- |\n| 1 |\n\n| B |\n| --- |\n| 2 |";
        assert_eq!(table_delete(two, at(two, "B")).text, "| A |\n| --- |\n| 1 |\n\n");
        assert_eq!(table_delete("text", 1).text, "text");
    }

    #[test]
    fn tabell_opererar_på_rätt_tabell() {
        let t = "| A | B |\n| --- | --- |\n| 1 | 2 |\n\ntext\n\n| C | D |\n| --- | --- |\n| 3 | 4 |";
        let e = table_insert_row(t, at(t, "3"), true);
        assert_eq!(e.text, format!("{t}\n|  |  |"));
        let e = table_delete_col(t, at(t, "A"));
        assert!(e.text.starts_with("| B |\n| --- |\n| 2 |\n\ntext\n\n| C | D |"));
    }

    #[test]
    fn modell_täcker_tabeller_bilder_och_markörer() {
        for d in [
            TB,
            "| A | B |\n| --- | --- |\n|  |  |",
            "A | B\n--|--\n1 | 2",
            "| a \\| b | c |\n| --- | --- |\n| x | y \\| z |\n\ntext",
            "text\n\n| Förnamn | Efternamn |\n| --- | --- |\n| Åsa | **Öberg** |\n\nSlut\n",
            "<img src=\"a.png\" alt=\"Sol\" width=\"240\">",
            "text <IMG SRC='a.png' ALT='ö'> text\n\n<img width=5 src=x.png>\n\nslut",
            "a<!--k:1-->b<!--k:2-->\n\n<!--k:3-->\n\n<img src=\"a.png\"> <!--k:4-->\n",
            "<div align=\"center\">\n\n<img src=\"a.png\" width=\"20\">\n\n</div>\n\n- punkt <!--k:5-->\n",
            "| A <!--k:1--> | B |\n| --- | --- |\n| <img src=\"a.png\"> | 2 |",
        ] {
            covers(d);
        }
    }
}
