//! Markdown-analys och formateringsoperationer för dokumentläget.
//!
//! Ren logik utan gränssnitt. Texten är fortfarande Markdown (så som `docx::read` skriver
//! den); här läggs ett lager ovanpå som säger vilken stil varje byte har (`analyze`), vilken
//! formatering som gäller vid markören (`active`) och hur knapparna ändrar texten
//! (`toggle_inline`, `set_block`, `insert_table`). Alla offset är byteoffset och landar alltid
//! på teckengränser.
#![allow(dead_code)]

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub pieces: Vec<Piece>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Active {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub block: Block,
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

struct Parsed {
    pieces: Vec<Piece>,
    spans: Vec<Span>,
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
                    _ => html_u.clear(),
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
                        images.push(ImageRef { alt: String::new(), src: dest_url.to_string() });
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
                    _ => html_u.clear(),
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
                if t.eq_ignore_ascii_case("<u>") {
                    html_u.push(r.clone());
                } else if t.eq_ignore_ascii_case("</u>") {
                    if let Some(open) = html_u.pop() {
                        fill(&mut st, open.start..r.end, |s| s.underline = true);
                        fill(&mut st, open.clone(), |s| s.markup = true);
                        fill(&mut st, r.clone(), |s| s.markup = true);
                        spans.push(Span { kind: Inline::Underline, open, close: r.clone() });
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

    // Bitar: sammanhängande tecken med samma stil, aldrig delade mitt i ett tecken.
    let mut pieces: Vec<Piece> = Vec::new();
    let mut cur: Option<(usize, St)> = None;
    for (i, _) in text.char_indices() {
        match cur {
            Some((_, s)) if s == st[i] => {}
            Some((start, s)) => {
                pieces.push(make_piece(start..i, &s, &images));
                cur = Some((i, st[i]));
            }
            None => cur = Some((i, st[i])),
        }
    }
    if let Some((start, s)) = cur {
        pieces.push(make_piece(start..n, &s, &images));
    }
    Parsed { pieces, spans }
}

fn make_piece(range: Range<usize>, s: &St, images: &[ImageRef]) -> Piece {
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
    Analysis { pieces: parse(text).pieces }
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
    let mut r = Active { bold: false, italic: false, underline: false, strike: false, block };
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
        }
        return r;
    }
    let mut seen = false;
    let mut flags = [true; 4];
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
    let a = Analysis { pieces: parsed.pieces.clone() };
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

fn add_fmt(text: &str, parsed: &Parsed, a: &Analysis, m: Inline, s: usize, e: usize) -> Edit {
    let (open, close) = delims(m);
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
                && x.image.is_none();
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
        assert_eq!(p.image, Some(ImageRef { alt: "Sol".into(), src: "bild:image1.png".into() }));
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
}
