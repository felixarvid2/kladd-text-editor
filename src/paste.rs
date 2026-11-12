//! Inklistring av formaterad text i dokumentläget: HTML från Word, LibreOffice, Google Docs och
//! webbsidor blir Kladds Markdown-dialekt (se `docs/dev/dokumentlage.md`).
//!
//! HTML:en läses av en egen liten, förlåtande tolk (några hundra rader) i stället för en crate:
//! Words villkorskommentarer (`<![if !supportLists]>`), `mso-list` och urklippets fragment
//! kräver ändå särbehandling, och det ger noll extra binärstorlek. Urklippet läses med
//! `arboard`, som eframe redan använder.

use std::ops::Range;

use crate::docx::{self, Image};
use crate::style::{self, Align, DocDefaults, SpanStyle};
use crate::word::markup;

/// Resultatet av en konvertering.
#[derive(Debug, Default)]
pub struct Pasted {
    pub markdown: String,
    /// Inklistrade bilder: namn (`inklistrad-N.png`) och data.
    pub images: Vec<(String, Image)>,
}

/// HTML från systemets urklipp, om det finns någon.
pub fn clipboard_html() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let html = cb.get().html().ok()?;
    (!html.trim().is_empty()).then_some(html)
}

/// Gör bildnamnen unika mot dokumentets befintliga bilder (och skriver om Markdown:en).
pub fn make_names_unique(p: &mut Pasted, existing: &[(String, Image)]) {
    let taken = |n: &str, mine: &[(String, Image)]| existing.iter().chain(mine.iter()).any(|(m, _)| m == n);
    let mut done: Vec<(String, Image)> = Vec::new();
    let mut md = std::mem::take(&mut p.markdown);
    for (name, img) in std::mem::take(&mut p.images) {
        let mut new = name.clone();
        let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
        let mut n = 1;
        while taken(&new, &done) {
            n += 1;
            new = format!("{stem}-{n}.{ext}");
        }
        if new != name {
            md = md.replace(&format!("{}{name}", docx::IMAGE_SCHEME), &format!("{}{new}", docx::IMAGE_SCHEME));
        }
        done.push((new, img));
    }
    p.markdown = md;
    p.images = done;
}

/// Har den inklistrade Markdown:en blockstruktur (rubrik, lista, tabell, flera stycken …)?
/// Annars sätts den in direkt i löptexten.
fn is_block_markdown(md: &str) -> bool {
    if md.contains("\n\n") {
        return true;
    }
    let first = md.lines().next().unwrap_or("");
    let digits = first.chars().take_while(char::is_ascii_digit).count();
    first.starts_with('#')
        || first.starts_with("- ")
        || first.starts_with("> ")
        || first.starts_with('|')
        || first.starts_with("<div")
        || first.starts_with("```")
        || (digits > 0 && first[digits..].starts_with(". "))
}

/// Sätter in `md` vid markeringen. Blockinnehåll får tomma rader runt sig (och delar stycket
/// om markören står mitt i det); ett enda inline-stycke sätts in direkt.
pub fn insert(text: &str, sel: Range<usize>, md: &str) -> markup::Edit {
    let md = md.trim_matches('\n');
    let (before, after) = (&text[..sel.start], &text[sel.end..]);
    let (pre, post) = if is_block_markdown(md) {
        let pre = if before.is_empty() || before.ends_with("\n\n") {
            ""
        } else if before.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let post = if after.is_empty() || after.starts_with("\n\n") {
            if after.is_empty() { "\n" } else { "" }
        } else if after.starts_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        (pre, post)
    } else {
        ("", "")
    };
    let mut out = String::with_capacity(text.len() + md.len() + 4);
    out.push_str(before);
    out.push_str(pre);
    out.push_str(md);
    let caret = out.len();
    out.push_str(post);
    out.push_str(after);
    markup::Edit { text: out, sel: caret..caret }
}

// ============================================================== HTML-träd

#[derive(Debug)]
enum Node {
    Text(String),
    El(El),
}

#[derive(Debug)]
struct El {
    tag: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Node>,
}

const VOID: &[&str] = &[
    "br", "img", "hr", "meta", "link", "input", "col", "wbr", "area", "base", "source", "embed", "param", "track",
];
/// Element vars innehåll aldrig är text.
const RAW: &[&str] = &["style", "script", "xml", "title"];

impl El {
    fn new(tag: &str) -> El {
        El { tag: tag.to_owned(), attrs: Vec::new(), kids: Vec::new() }
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn style(&self) -> &str {
        self.attr("style").unwrap_or("")
    }

    fn has_block_desc(&self) -> bool {
        self.kids.iter().any(|k| matches!(k, Node::El(e) if is_block(&e.tag) || e.has_block_desc()))
    }
}

fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "li" | "table" | "tr" | "td" | "th"
            | "thead" | "tbody" | "tfoot" | "caption" | "blockquote" | "pre" | "hr" | "section" | "article"
            | "header" | "footer" | "main" | "aside" | "nav" | "figure" | "figcaption" | "form" | "fieldset"
            | "address" | "dl" | "dt" | "dd" | "center" | "body" | "html"
    )
}

fn parse(html: &str) -> El {
    let lower = html.to_ascii_lowercase();
    let b = html.as_bytes();
    let mut i = 0;
    let mut stack = vec![El::new("#root")];
    while i < b.len() {
        if b[i] != b'<' {
            let end = html[i..].find('<').map_or(b.len(), |p| i + p);
            push_text(&mut stack, &html[i..end]);
            i = end;
            continue;
        }
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(b.len(), |p| i + p + 3);
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            // Doctype och Words `<![if !supportLists]>` / `<![endif]>`: bara markörer.
            i = rest.find('>').map_or(b.len(), |p| i + p + 1);
        } else if rest.starts_with("</") {
            let end = rest.find('>').map_or(rest.len(), |p| p + 1);
            let name = rest[2..end].trim_end_matches('>').trim().to_ascii_lowercase();
            close(&mut stack, &name);
            i += end;
        } else if let Some((name, attrs, selfclose, len)) = parse_open(rest) {
            i += len;
            if RAW.contains(&name.as_str()) && !selfclose {
                let close = format!("</{name}");
                i = match lower[i..].find(&close) {
                    Some(p) => lower[i + p..].find('>').map_or(b.len(), |q| i + p + q + 1),
                    None => b.len(),
                };
                continue;
            }
            if is_block(&name) && stack.last().is_some_and(|t| t.tag == "p") {
                pop_attach(&mut stack);
            }
            match name.as_str() {
                "li" => close_to(&mut stack, &["li"], &["ul", "ol"]),
                "td" | "th" => close_to(&mut stack, &["td", "th"], &["tr", "table"]),
                "tr" => close_to(&mut stack, &["tr"], &["table"]),
                _ => {}
            }
            let el = El { tag: name.clone(), attrs, kids: Vec::new() };
            if VOID.contains(&name.as_str()) || selfclose {
                stack.last_mut().unwrap().kids.push(Node::El(el));
            } else if stack.len() < 256 {
                stack.push(el);
            }
        } else {
            push_text(&mut stack, "<");
            i += 1;
        }
    }
    while stack.len() > 1 {
        pop_attach(&mut stack);
    }
    stack.pop().unwrap()
}

/// `<tag a=b c="d">` → (namn, attribut, självstängd, längd).
#[allow(clippy::type_complexity)]
fn parse_open(rest: &str) -> Option<(String, Vec<(String, String)>, bool, usize)> {
    let b = rest.as_bytes();
    if b.len() < 2 || !b[1].is_ascii_alphabetic() {
        return None;
    }
    let mut j = 1;
    let stop = |c: u8| c.is_ascii_whitespace() || c == b'/' || c == b'>';
    while j < b.len() && !stop(b[j]) {
        j += 1;
    }
    let name = rest[1..j].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let mut selfclose = false;
    loop {
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= b.len() {
            return Some((name, attrs, selfclose, b.len()));
        }
        match b[j] {
            b'>' => return Some((name, attrs, selfclose, j + 1)),
            b'/' => {
                selfclose = true;
                j += 1;
                continue;
            }
            _ => selfclose = false,
        }
        let s = j;
        while j < b.len() && !(b[j].is_ascii_whitespace() || matches!(b[j], b'=' | b'>' | b'/')) {
            j += 1;
        }
        if j == s {
            j += 1;
            continue;
        }
        let key = rest[s..j].to_ascii_lowercase();
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let mut val = String::new();
        if j < b.len() && b[j] == b'=' {
            j += 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                let q = b[j];
                let vs = j + 1;
                let ve = b[vs..].iter().position(|&c| c == q).map_or(b.len(), |p| vs + p);
                val = decode(&rest[vs..ve]);
                j = (ve + 1).min(b.len());
            } else {
                let vs = j;
                while j < b.len() && !(b[j].is_ascii_whitespace() || b[j] == b'>') {
                    j += 1;
                }
                val = decode(&rest[vs..j]);
            }
        }
        attrs.push((key, val));
    }
}

fn push_text(stack: &mut [El], raw: &str) {
    let t = decode(raw);
    if t.is_empty() {
        return;
    }
    let top = stack.last_mut().unwrap();
    if let Some(Node::Text(prev)) = top.kids.last_mut() {
        prev.push_str(&t);
    } else {
        top.kids.push(Node::Text(t));
    }
}

fn pop_attach(stack: &mut Vec<El>) {
    if stack.len() > 1 {
        let e = stack.pop().unwrap();
        stack.last_mut().unwrap().kids.push(Node::El(e));
    }
}

/// Stänger det närmaste elementet med ett av namnen `targets` (och allt under det), men inte
/// förbi ett `stops`-element.
fn close_to(stack: &mut Vec<El>, targets: &[&str], stops: &[&str]) {
    for idx in (1..stack.len()).rev() {
        let t = stack[idx].tag.as_str();
        if stops.contains(&t) {
            return;
        }
        if targets.contains(&t) {
            while stack.len() > idx {
                pop_attach(stack);
            }
            return;
        }
    }
}

fn close(stack: &mut Vec<El>, name: &str) {
    if let Some(idx) = (1..stack.len()).rev().find(|&i| stack[i].tag == name) {
        while stack.len() > idx {
            pop_attach(stack);
        }
    }
}

fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let semi = rest.find(';').filter(|&e| e <= 10);
        let ch = semi.and_then(|e| entity(&rest[1..e]));
        match (semi, ch) {
            (Some(e), Some(c)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let n = match num.strip_prefix(['x', 'X']) {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(n);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "euro" => '€',
        "deg" => '°',
        "times" => '×',
        "aring" => 'å',
        "auml" => 'ä',
        "ouml" => 'ö',
        "Aring" => 'Å',
        "Auml" => 'Ä',
        "Ouml" => 'Ö',
        "eacute" => 'é',
        "egrave" => 'è',
        "uuml" => 'ü',
        "Uuml" => 'Ü',
        "szlig" => 'ß',
        _ => return None,
    })
}

/// Standard-base64 (och URL-säker variant); okända tecken hoppas över.
fn base64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// Värdet för CSS-egenskapen `prop` i ett `style`-attribut (gemener, sista deklarationen vinner).
fn css(style: &str, prop: &str) -> Option<String> {
    let mut r = None;
    for d in style.split(';') {
        if let Some((k, v)) = d.split_once(':')
            && k.trim().eq_ignore_ascii_case(prop)
        {
            r = Some(v.trim().trim_end_matches("!important").trim().to_ascii_lowercase());
        }
    }
    r
}

// ============================================================== konvertering

#[derive(Clone, Default, PartialEq)]
struct Fmt {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
    span: SpanStyle,
    link: Option<String>,
}

#[derive(Clone)]
enum Kind {
    Text(String),
    Break,
    /// Färdig Markdown (bilder).
    Raw(String),
}

#[derive(Clone)]
struct Run {
    kind: Kind,
    fmt: Fmt,
}

struct Block {
    align: Align,
    text: String,
}

struct Item {
    level: usize,
    marker: String,
    runs: Vec<Run>,
}

struct Conv<'a> {
    defaults: Option<&'a DocDefaults>,
    out: Vec<Block>,
    items: Vec<Item>,
    /// Senaste löpnumret per listnivå i Words låtsaslistor.
    last_num: Vec<u64>,
    images: Vec<(String, Image)>,
}

/// Konverterar HTML från urklipp till Kladds Markdown. `defaults` är måldokumentets grundformat:
/// typsnitt och storlek som är lika med det skrivs inte som span.
pub fn html_to_markdown(html: &str, defaults: Option<&DocDefaults>) -> Pasted {
    const START: &str = "<!--StartFragment-->";
    let html = match (html.find(START), html.find("<!--EndFragment-->")) {
        (Some(a), Some(b)) if a + START.len() <= b => &html[a + START.len()..b],
        _ => html,
    };
    let root = parse(html);
    let body = find_el(&root, "body").unwrap_or(&root);
    let mut c = Conv { defaults, out: Vec::new(), items: Vec::new(), last_num: Vec::new(), images: Vec::new() };
    c.blocks(&body.kids, Align::Left, &Fmt::default());
    c.flush_items();
    let markdown = assemble(&c.out);
    Pasted { markdown, images: c.images }
}

fn find_el<'a>(el: &'a El, tag: &str) -> Option<&'a El> {
    el.kids.iter().find_map(|k| match k {
        Node::El(e) if e.tag == tag => Some(e),
        Node::El(e) => find_el(e, tag),
        Node::Text(_) => None,
    })
}

/// Fogar ihop block; intilliggande block med samma justering delar en `<div>`.
fn assemble(blocks: &[Block]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        let al = blocks[i].align;
        let mut j = i;
        while j < blocks.len() && blocks[j].align == al {
            j += 1;
        }
        let body = blocks[i..j].iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n\n");
        parts.push(if al == Align::Left { body } else { format!("{}\n\n{body}\n\n</div>", style::div_open(al)) });
        i = j;
    }
    parts.join("\n\n")
}

fn skip(el: &El) -> bool {
    let t = el.tag.as_str();
    if matches!(t, "style" | "script" | "head" | "title" | "meta" | "link" | "xml" | "noscript" | "template") {
        return true;
    }
    // VML, Words fält m.m. (`o:p` är bara ett tomt omslag).
    if t.contains(':') && t != "o:p" {
        return true;
    }
    let st = el.style();
    css(st, "mso-list").is_some_and(|v| v.starts_with("ignore"))
        || css(st, "display").is_some_and(|v| v == "none")
        || css(st, "mso-hide").is_some_and(|v| v == "all")
}

fn own_align(el: &El) -> Option<Align> {
    if el.tag == "center" {
        return Some(Align::Center);
    }
    let v = el.attr("align").map(str::to_ascii_lowercase).or_else(|| css(el.style(), "text-align"))?;
    match v.trim() {
        "left" | "start" => Some(Align::Left),
        "center" | "right" | "justify" | "middle" | "centre" | "end" => Some(Align::from_attr(&v)),
        _ => None,
    }
}

fn heading_level(el: &El) -> Option<usize> {
    if let Some(n) = el.tag.strip_prefix('h').and_then(|n| n.parse::<usize>().ok())
        && (1..=6).contains(&n)
    {
        return Some(n);
    }
    let class = el.attr("class").unwrap_or("");
    if class.contains("MsoTitle") {
        return Some(1);
    }
    if class.contains("MsoSubtitle") {
        return Some(2);
    }
    if let Some(p) = class.find("MsoHeading")
        && let Some(n) = class[p + 10..].chars().next().and_then(|c| c.to_digit(10))
        && (1..=6).contains(&n)
    {
        return Some(n as usize);
    }
    let n: usize = css(el.style(), "mso-outline-level")?.parse().ok()?;
    (1..=6).contains(&n).then_some(n)
}

/// Nivå (0-baserad) om stycket är en av Words låtsaslistor.
fn fake_list_level(el: &El) -> Option<usize> {
    let v = css(el.style(), "mso-list")?;
    if v.is_empty() || v.starts_with("ignore") || v.starts_with("none") {
        return None;
    }
    let n = v.find("level").and_then(|p| {
        let d: String = v[p + 5..].chars().take_while(char::is_ascii_digit).collect();
        d.parse::<usize>().ok()
    });
    Some(n.unwrap_or(1).clamp(1, 9) - 1)
}

/// Texten i Words punkt/nummer (`<span style='mso-list:Ignore'>`).
fn ignored_bullet(el: &El) -> String {
    fn walk(el: &El, out: &mut String) {
        for k in &el.kids {
            match k {
                Node::Text(t) => out.push_str(t),
                Node::El(e) => walk(e, out),
            }
        }
    }
    fn find(el: &El) -> Option<String> {
        for k in &el.kids {
            if let Node::El(e) = k {
                if css(e.style(), "mso-list").is_some_and(|v| v.starts_with("ignore")) {
                    let mut s = String::new();
                    walk(e, &mut s);
                    return Some(s);
                }
                if let Some(s) = find(e) {
                    return Some(s);
                }
            }
        }
        None
    }
    find(el).unwrap_or_default().replace('\u{a0}', " ").trim().to_owned()
}

fn is_generic_font(f: &str) -> bool {
    let l = f.to_ascii_lowercase();
    l.starts_with('-')
        || l.starts_with("var(")
        || matches!(
            l.as_str(),
            "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui" | "inherit" | "initial"
                | "unset" | "ui-sans-serif" | "ui-serif" | "ui-monospace" | "blinkmacsystemfont"
        )
}

impl Conv<'_> {
    // ---------- teckenformat

    fn apply(&self, fmt: &Fmt, el: &El) -> Fmt {
        let mut f = fmt.clone();
        let st = el.style();
        match el.tag.as_str() {
            "b" | "strong" => f.bold = true,
            "i" | "em" | "cite" | "var" | "dfn" => f.italic = true,
            "u" | "ins" => f.underline = true,
            "s" | "strike" | "del" => f.strike = true,
            "code" | "tt" | "kbd" | "samp" => f.code = true,
            _ => {}
        }
        if let Some(w) = css(st, "font-weight") {
            match w.as_str() {
                "bold" | "bolder" => f.bold = true,
                "normal" | "lighter" => f.bold = false,
                n => {
                    if let Ok(n) = n.parse::<u32>() {
                        f.bold = n >= 600;
                    }
                }
            }
        }
        if let Some(s) = css(st, "font-style") {
            match s.as_str() {
                "italic" | "oblique" => f.italic = true,
                "normal" => f.italic = false,
                _ => {}
            }
        }
        if let Some(td) = css(st, "text-decoration").or_else(|| css(st, "text-decoration-line")) {
            if td.contains("underline") && el.tag != "a" {
                f.underline = true;
            }
            if td.contains("line-through") {
                f.strike = true;
            }
        }
        if el.tag == "a"
            && let Some(href) = el.attr("href")
        {
            let h = href.trim();
            let l = h.to_ascii_lowercase();
            if l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:") {
                f.link = Some(h.split_whitespace().collect::<Vec<_>>().join("%20"));
            }
        }
        let mut s = SpanStyle::from_css(st);
        if el.tag == "font" {
            if let Some(c) = el.attr("color").and_then(style::parse_color) {
                s.color = Some(c);
            }
            if let Some(face) = el.attr("face") {
                s.font_family = face.split(',').next().map(|x| x.trim().to_owned());
            }
        }
        if is_block(&el.tag) || el.tag == "a" {
            s.background = None;
        }
        if el.tag == "a" {
            s.color = None;
        }
        if s.color == Some([0, 0, 0]) {
            s.color = None;
        }
        if s.background == Some([255, 255, 255]) {
            s.background = None;
        }
        if s.font_family.as_deref().is_some_and(is_generic_font) {
            s.font_family = None;
        }
        if let Some(d) = self.defaults {
            if s.font_family.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(d.font())) {
                s.font_family = None;
            }
            if s.font_size.is_some_and(|x| (x - d.size()).abs() < 0.01) {
                s.font_size = None;
            }
        }
        f.span = f.span.merged(&s);
        f
    }

    // ---------- block

    fn push_block(&mut self, align: Align, text: String) {
        self.flush_items();
        self.out.push(Block { align, text });
    }

    fn blocks(&mut self, kids: &[Node], align: Align, fmt: &Fmt) {
        let mut runs: Vec<Run> = Vec::new();
        for k in kids {
            match k {
                Node::Text(t) => runs.push(Run { kind: Kind::Text(t.replace('\u{a0}', " ")), fmt: fmt.clone() }),
                Node::El(el) => {
                    if skip(el) {
                        continue;
                    }
                    if is_block(&el.tag) || el.has_block_desc() {
                        self.para(std::mem::take(&mut runs), align);
                        self.block(el, align, fmt);
                    } else {
                        self.inline(el, fmt, &mut runs, false);
                    }
                }
            }
        }
        self.para(runs, align);
    }

    fn block(&mut self, el: &El, align: Align, fmt: &Fmt) {
        let align = own_align(el).unwrap_or(align);
        let f = self.apply(fmt, el);
        match el.tag.as_str() {
            "ul" | "ol" => self.list(el, 0, &f),
            "table" => self.table(el, &f),
            "blockquote" => {
                self.flush_items();
                let start = self.out.len();
                self.blocks(&el.kids, align, &f);
                self.flush_items();
                let inner: Vec<String> = self.out.drain(start..).map(|b| b.text).collect();
                let text = inner
                    .join("\n\n")
                    .lines()
                    .map(|l| if l.is_empty() { ">".to_owned() } else { format!("> {l}") })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    self.push_block(Align::Left, text);
                }
            }
            "pre" => {
                let mut s = String::new();
                raw_text(el, &mut s);
                let s = s.trim_matches('\n').replace('\u{a0}', " ");
                if !s.trim().is_empty() {
                    let fence = if s.contains("```") { "~~~~" } else { "```" };
                    self.push_block(Align::Left, format!("{fence}\n{s}\n{fence}"));
                }
            }
            "hr" => self.flush_items(),
            "li" => self.list_item(el, 0, "-".to_owned(), &f),
            _ => {
                if let Some(n) = heading_level(el) {
                    let mut runs = Vec::new();
                    self.inline_kids(&el.kids, &f, &mut runs, true);
                    self.heading(runs, n, align);
                } else if let Some(level) = fake_list_level(el) {
                    let bullet = ignored_bullet(el);
                    let marker = self.fake_marker(&bullet, level);
                    let mut runs = Vec::new();
                    self.inline_kids(&el.kids, &f, &mut runs, true);
                    self.items.push(Item { level, marker, runs });
                } else {
                    self.blocks(&el.kids, align, &f);
                }
            }
        }
    }

    fn fake_marker(&mut self, bullet: &str, level: usize) -> String {
        self.last_num.resize(level + 1, 0);
        let trimmed = bullet.trim_end_matches(['.', ')']);
        let punct = trimmed.len() < bullet.len();
        let n = if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
            trimmed.parse::<u64>().ok()
        } else if punct && trimmed.len() <= 4 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
            Some(self.last_num[level] + 1)
        } else {
            None
        };
        match n {
            Some(n) => {
                self.last_num[level] = n;
                format!("{n}.")
            }
            None => {
                self.last_num[level] = 0;
                "-".to_owned()
            }
        }
    }

    fn para(&mut self, runs: Vec<Run>, align: Align) {
        let runs = collapse(runs);
        if !has_content(&runs) {
            return;
        }
        let text = render_runs(&runs).lines().map(|l| docx::escape_line_start(l.trim_end())).collect::<Vec<_>>().join("\n");
        self.push_block(align, text);
    }

    fn heading(&mut self, mut runs: Vec<Run>, n: usize, align: Align) {
        // Rubrikens eget utseende kommer från formatmallen.
        for r in &mut runs {
            r.fmt.bold = false;
            r.fmt.span.font_size = None;
            r.fmt.span.font_family = None;
        }
        let runs = collapse(runs);
        if !has_content(&runs) {
            return;
        }
        let text = render_runs(&runs).replace('\n', " ");
        self.push_block(align, format!("{} {}", "#".repeat(n), text.trim()));
    }

    // ---------- listor

    fn list(&mut self, el: &El, level: usize, fmt: &Fmt) {
        let ordered = el.tag == "ol";
        let mut n: u64 = el.attr("start").and_then(|s| s.trim().parse().ok()).unwrap_or(1);
        for k in &el.kids {
            let Node::El(k) = k else { continue };
            match k.tag.as_str() {
                "li" => {
                    let marker = if ordered { format!("{n}.") } else { "-".to_owned() };
                    n += 1;
                    // Google Dokument lägger varje nivå i en egen platt lista med `aria-level`.
                    let lvl = if level == 0 {
                        k.attr("aria-level").and_then(|a| a.parse::<usize>().ok()).map_or(0, |a| a.clamp(1, 9) - 1)
                    } else {
                        level
                    };
                    self.list_item(k, lvl, marker, fmt);
                }
                "ul" | "ol" => self.list(k, level + 1, fmt),
                _ => {}
            }
        }
    }

    fn list_item(&mut self, li: &El, level: usize, marker: String, fmt: &Fmt) {
        let f = self.apply(fmt, li);
        let mut runs = Vec::new();
        let mut nested: Vec<&El> = Vec::new();
        for k in &li.kids {
            match k {
                Node::Text(t) => runs.push(Run { kind: Kind::Text(t.replace('\u{a0}', " ")), fmt: f.clone() }),
                Node::El(e) if matches!(e.tag.as_str(), "ul" | "ol") => nested.push(e),
                Node::El(e) => self.inline(e, &f, &mut runs, true),
            }
        }
        self.items.push(Item { level, marker, runs });
        for n in nested {
            self.list(n, level + 1, &f);
        }
    }

    fn flush_items(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let items = std::mem::take(&mut self.items);
        self.last_num.clear();
        let mut lines: Vec<String> = Vec::new();
        let mut prev_level: Option<usize> = None;
        let mut top_ordered: Option<bool> = None;
        for it in items {
            let runs = collapse(it.runs);
            if !has_content(&runs) {
                continue;
            }
            // Ett hopp över en nivå skulle annars bli ett kodblock.
            let level = it.level.min(prev_level.map_or(0, |p| p + 1));
            prev_level = Some(level);
            // Byter den yttersta listan typ (punkter/nummer) blir det en ny lista, som i docx.rs.
            if level == 0 {
                let ordered = it.marker != "-";
                if top_ordered.is_some_and(|o| o != ordered) {
                    lines.push(String::new());
                }
                top_ordered = Some(ordered);
            }
            let indent = "    ".repeat(level);
            let cont = format!("{indent}{}", " ".repeat(it.marker.len() + 1));
            let md = render_runs(&runs);
            for (i, l) in md.lines().enumerate() {
                if i == 0 {
                    lines.push(format!("{indent}{} {}", it.marker, docx::escape_line_start(l.trim_end())));
                } else {
                    lines.push(format!("{cont}{}", l.trim_end()));
                }
            }
        }
        if !lines.is_empty() {
            self.out.push(Block { align: Align::Left, text: lines.join("\n") });
        }
    }

    // ---------- tabeller

    fn table(&mut self, el: &El, fmt: &Fmt) {
        let mut rows: Vec<Vec<String>> = Vec::new();
        self.table_rows(el, fmt, &mut rows);
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let line = |r: &[String]| {
            let mut cells: Vec<&str> = r.iter().map(String::as_str).collect();
            cells.resize(cols, "");
            format!("| {} |", cells.join(" | "))
        };
        let mut out = vec![line(&rows[0]), format!("| {} |", vec!["---"; cols].join(" | "))];
        out.extend(rows[1..].iter().map(|r| line(r)));
        self.push_block(Align::Left, out.join("\n"));
    }

    fn table_rows(&mut self, el: &El, fmt: &Fmt, rows: &mut Vec<Vec<String>>) {
        for k in &el.kids {
            let Node::El(k) = k else { continue };
            match k.tag.as_str() {
                "thead" | "tbody" | "tfoot" => self.table_rows(k, fmt, rows),
                "tr" => {
                    let rf = self.apply(fmt, k);
                    let mut row = Vec::new();
                    for c in &k.kids {
                        let Node::El(c) = c else { continue };
                        if !matches!(c.tag.as_str(), "td" | "th") || skip(c) {
                            continue;
                        }
                        let cf = self.apply(&rf, c);
                        let mut runs = Vec::new();
                        self.inline_kids(&c.kids, &cf, &mut runs, true);
                        let text = render_runs(&collapse(runs)).replace('\n', " ").replace('|', "\\|");
                        row.push(text.trim().to_owned());
                        let span: usize = c.attr("colspan").and_then(|s| s.trim().parse().ok()).unwrap_or(1);
                        for _ in 1..span.clamp(1, 50) {
                            row.push(String::new());
                        }
                    }
                    if !row.is_empty() {
                        rows.push(row);
                    }
                }
                _ => {}
            }
        }
    }

    // ---------- inline

    fn inline_kids(&mut self, kids: &[Node], fmt: &Fmt, runs: &mut Vec<Run>, flat: bool) {
        for k in kids {
            match k {
                Node::Text(t) => runs.push(Run { kind: Kind::Text(t.replace('\u{a0}', " ")), fmt: fmt.clone() }),
                Node::El(e) => self.inline(e, fmt, runs, flat),
            }
        }
    }

    /// `flat`: blockelement inuti blir bara ett mellanslag (tabellceller, listpunkter, rubriker).
    fn inline(&mut self, el: &El, fmt: &Fmt, runs: &mut Vec<Run>, flat: bool) {
        if skip(el) {
            return;
        }
        match el.tag.as_str() {
            "br" => runs.push(Run { kind: Kind::Break, fmt: fmt.clone() }),
            "wbr" => {}
            "img" => self.image(el, fmt, runs),
            tag => {
                let f = self.apply(fmt, el);
                let sep = flat && is_block(tag);
                let space = Run { kind: Kind::Text(" ".to_owned()), fmt: fmt.clone() };
                if sep {
                    runs.push(space.clone());
                }
                self.inline_kids(&el.kids, &f, runs, flat);
                if sep {
                    runs.push(space);
                }
            }
        }
    }

    fn image(&mut self, el: &El, fmt: &Fmt, runs: &mut Vec<Run>) {
        let src = el.attr("src").unwrap_or("").trim();
        let alt = el.attr("alt").unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
        let mut raw = None;
        if let Some(data) = src.strip_prefix("data:")
            && let Some((head, body)) = data.split_once(',')
            && head.contains("base64")
        {
            let bytes = base64_decode(body);
            // Bara de format Word kan visa.
            if let Some(ext) = docx::image_ext(&bytes).filter(|e| matches!(*e, "png" | "jpeg" | "gif" | "bmp")) {
                let name = format!("inklistrad-{}.{ext}", self.images.len() + 1);
                let width = img_width(el);
                raw = Some(match width {
                    Some(w) => format!(
                        "<img src=\"{}{name}\" alt=\"{}\" width=\"{w}\">",
                        docx::IMAGE_SCHEME,
                        alt.replace(['"', '<', '>'], "'")
                    ),
                    None => format!("![{}]({}{name})", alt.replace(['[', ']'], ""), docx::IMAGE_SCHEME),
                });
                self.images.push((name, Image { bytes, extent: None }));
            }
        }
        match raw {
            Some(r) => runs.push(Run { kind: Kind::Raw(r), fmt: fmt.clone() }),
            // Webbadresser och lokala filer hämtas inte: alternativtexten får räcka.
            None if !alt.is_empty() => runs.push(Run { kind: Kind::Text(alt), fmt: fmt.clone() }),
            None => {}
        }
    }
}

/// Bildbredd i CSS-pixlar från `width=` eller `style="width:…"`.
fn img_width(el: &El) -> Option<u32> {
    let px = |v: &str| -> Option<f32> {
        let v = v.trim().to_ascii_lowercase();
        if v.ends_with('%') {
            return None;
        }
        match v.strip_suffix("px") {
            Some(n) => n.trim().parse().ok(),
            None if v.bytes().all(|c| c.is_ascii_digit() || c == b'.') => v.parse().ok(),
            None => style::parse_length(&v).map(|pt| pt * 96.0 / 72.0),
        }
    };
    let w = css(el.style(), "width").and_then(|v| px(&v)).or_else(|| el.attr("width").and_then(px))?;
    (w >= 1.0 && w < 20000.0).then(|| w.round() as u32)
}

fn raw_text(el: &El, out: &mut String) {
    for k in &el.kids {
        match k {
            Node::Text(t) => out.push_str(t),
            Node::El(e) if e.tag == "br" => out.push('\n'),
            Node::El(e) => raw_text(e, out),
        }
    }
}

/// HTML:s blankhantering: sammanhängande blanksteg blir ett, och rader börjar och slutar utan.
fn collapse(runs: Vec<Run>) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut prev_space = true;
    for r in runs {
        match r.kind {
            Kind::Text(t) => {
                let mut s = String::new();
                for c in t.chars() {
                    if c.is_whitespace() {
                        if !prev_space {
                            s.push(' ');
                            prev_space = true;
                        }
                    } else {
                        s.push(c);
                        prev_space = false;
                    }
                }
                if !s.is_empty() {
                    out.push(Run { kind: Kind::Text(s), fmt: r.fmt });
                }
            }
            Kind::Break => {
                if out.is_empty() {
                    continue;
                }
                trim_end_space(&mut out);
                out.push(r);
                prev_space = true;
            }
            Kind::Raw(_) => {
                out.push(r);
                prev_space = false;
            }
        }
    }
    trim_end_space(&mut out);
    while matches!(out.last(), Some(Run { kind: Kind::Break, .. })) {
        out.pop();
    }
    out
}

fn trim_end_space(out: &mut Vec<Run>) {
    if let Some(Run { kind: Kind::Text(t), .. }) = out.last_mut() {
        if t.ends_with(' ') {
            t.pop();
        }
        if t.is_empty() {
            out.pop();
        }
    }
}

fn has_content(runs: &[Run]) -> bool {
    runs.iter().any(|r| match &r.kind {
        Kind::Text(t) => !t.trim().is_empty(),
        Kind::Raw(_) => true,
        Kind::Break => false,
    })
}

/// Omger texten med markörer; mellanslag i kanterna stannar utanför.
fn wrap(text: &str, open: &str, close: &str) -> String {
    let start = text.len() - text.trim_start().len();
    let end = text.trim_end().len();
    if start >= end {
        return text.to_owned();
    }
    format!("{}{open}{}{close}{}", &text[..start], &text[start..end], &text[end..])
}

fn render_runs(runs: &[Run]) -> String {
    let mut s = String::new();
    let mut i = 0;
    while i < runs.len() {
        match &runs[i].kind {
            Kind::Break => {
                s.push('\n');
                i += 1;
            }
            Kind::Raw(r) => {
                let fmt = &runs[i].fmt;
                match &fmt.link {
                    Some(u) => s.push_str(&format!("[{r}]({})", link_url(u))),
                    None => s.push_str(r),
                }
                i += 1;
            }
            Kind::Text(_) => {
                let fmt = &runs[i].fmt;
                let mut text = String::new();
                while i < runs.len() && runs[i].fmt == *fmt {
                    let Kind::Text(t) = &runs[i].kind else { break };
                    text.push_str(t);
                    i += 1;
                }
                s.push_str(&style_text(&text, fmt));
            }
        }
    }
    s
}

fn link_url(u: &str) -> String {
    u.replace(' ', "%20").replace(')', "%29")
}

/// Samma ordning som `docx.rs`: ~~ yttrest av tecknen, sedan *, **, `<u>`, span och länk.
fn style_text(text: &str, f: &Fmt) -> String {
    let mut s = if f.code && !text.contains('`') { wrap(text, "`", "`") } else { docx::escape_inline(text) };
    if f.strike {
        s = wrap(&s, "~~", "~~");
    }
    if f.italic {
        s = wrap(&s, "*", "*");
    }
    if f.bold {
        s = wrap(&s, "**", "**");
    }
    if f.underline {
        s = wrap(&s, "<u>", "</u>");
    }
    if !f.span.is_empty() {
        s = wrap(&s, &f.span.open_tag(), "</span>");
    }
    if let Some(u) = &f.link {
        s = wrap(&s, "[", &format!("]({})", link_url(u)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        html_to_markdown(html, None).markdown
    }

    /// Ett 1x1-PNG som data-URI.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    const WORD: &str = r#"<html xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:w="urn:schemas-microsoft-com:office:word">
<head><meta charset=utf-8><meta name=Generator content="Microsoft Word 15">
<!--[if gte mso 9]><xml><w:WordDocument><w:View>Normal</w:View></w:WordDocument></xml><![endif]-->
<style><!-- p.MsoNormal {margin:0cm; font-size:11.0pt} --></style></head>
<body lang=SV style='tab-interval:36.0pt'>
<!--StartFragment-->
<p class=MsoTitle><span style='font-size:28.0pt'>Veckoplan</span><o:p></o:p></p>
<p class=MsoNormal>Hej <b>fina</b> v&auml;rld, <span style='color:#C00000;font-weight:bold'>r&ouml;d och fet</span>&nbsp;och <i>kursiv</i>.<o:p></o:p></p>
<p class=MsoNormal><o:p>&nbsp;</o:p></p>
<p class=MsoNormal align=center style='text-align:center'>Centrerad rad</p>
<p class=MsoListParagraphCxSpFirst style='text-indent:-18.0pt;mso-list:l0 level1 lfo1'><![if !supportLists]><span style='font-family:Symbol;mso-fareast-font-family:Symbol;mso-list:Ignore'>&middot;<span style='font:7.0pt "Times New Roman"'>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp; </span></span><![endif]>F&ouml;rsta<o:p></o:p></p>
<p class=MsoListParagraphCxSpMiddle style='text-indent:-18.0pt;mso-list:l0 level2 lfo1'><![if !supportLists]><span style='mso-list:Ignore'>o<span style='font:7.0pt "Times New Roman"'>&nbsp;&nbsp; </span></span><![endif]>Under<o:p></o:p></p>
<p class=MsoListParagraphCxSpLast style='text-indent:-18.0pt;mso-list:l0 level1 lfo1'><![if !supportLists]><span style='mso-list:Ignore'>&middot;<span>&nbsp;</span></span><![endif]>Andra<o:p></o:p></p>
<p class=MsoListParagraph style='mso-list:l1 level1 lfo2'><![if !supportLists]><span style='mso-list:Ignore'>1.<span>&nbsp;&nbsp;</span></span><![endif]>Ett</p>
<p class=MsoListParagraph style='mso-list:l1 level1 lfo2'><![if !supportLists]><span style='mso-list:Ignore'>2.<span>&nbsp;&nbsp;</span></span><![endif]>Tv&aring;</p>
<table class=MsoTableGrid border=1 cellspacing=0 cellpadding=0>
 <tr><td width=100><p class=MsoNormal><b>Dag</b></p></td><td width=100><p class=MsoNormal><b>Vad</b></p></td></tr>
 <tr><td><p class=MsoNormal>M&aring;ndag</p></td><td><p class=MsoNormal>Bak<br>ning | kul</p></td></tr>
</table>
<!--EndFragment-->
</body></html>"#;

    #[test]
    fn word() {
        assert_eq!(
            md(WORD),
            "# Veckoplan\n\nHej **fina** värld, <span style=\"color: #C00000\">**röd och fet**</span> och *kursiv*.\n\n\
<div align=\"center\">\n\nCentrerad rad\n\n</div>\n\n\
- Första\n    - Under\n- Andra\n\n1. Ett\n2. Två\n\n\
| **Dag** | **Vad** |\n| --- | --- |\n| Måndag | Bak ning \\| kul |"
        );
    }

    #[test]
    fn libreoffice() {
        let html = r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.0//EN"><html><head><meta http-equiv="content-type" content="text/html; charset=utf-8"/><title></title><style type="text/css">@page { margin: 2cm } p { margin-bottom: 0.25cm }</style></head><body lang="sv-SE" dir="ltr">
<h1 class="western">Rubrik ett</h1>
<p align="justify" style="margin-bottom: 0cm"><font face="Arial, sans-serif"><font size="3" style="font-size: 14pt"><u>Understruket</u></font></font> och <strike>struket</strike> med <span style="background: #ffff00">gult</span>.</p>
<ol start="3"><li><p>Tre</p></li><li><p>Fyra <a href="https://example.com/a b">länk</a></p></li></ol>
</body></html>"#;
        assert_eq!(
            md(html),
            "# Rubrik ett\n\n<div align=\"justify\">\n\n<span style=\"font-family: Arial; font-size: 14pt\"><u>Understruket</u></span> och ~~struket~~ med <span style=\"background-color: #FFFF00\">gult</span>.\n\n</div>\n\n3. Tre\n4. Fyra [länk](https://example.com/a%20b)"
        );
    }

    #[test]
    fn google_docs() {
        let html = r#"<meta charset='utf-8'><meta charset="utf-8"><b style="font-weight:normal;" id="docs-internal-guid-1b2c3d4e-7fff-1234-abcd-0123456789ab"><h2 dir="ltr" style="line-height:1.38;margin-top:18pt;"><span style="font-size:16pt;font-family:Arial,sans-serif;color:#000000;font-weight:400;">Dagens m&aring;l</span></h2><p dir="ltr" style="line-height:1.38;text-align: center;"><span style="font-size:11pt;font-family:Arial,sans-serif;color:#ff0000;background-color:#ffff00;font-weight:700;font-style:italic;">Viktigt</span><span style="font-size:11pt;font-family:Arial,sans-serif;color:#000000;background-color:transparent;font-weight:400;"> att komma ih&aring;g &amp; &lt;se&gt;</span></p><ul style="margin-top:0;"><li dir="ltr" style="list-style-type:disc;" aria-level="1"><p dir="ltr" role="presentation"><span style="font-size:11pt;font-weight:400;text-decoration:underline;">Punkt</span></p></li><ul><li dir="ltr" aria-level="2"><p dir="ltr" role="presentation"><span style="font-size:11pt;">Under</span></p></li></ul></ul></b><br class="Apple-interchange-newline">"#;
        assert_eq!(
            md(html),
            "## Dagens mål\n\n<div align=\"center\">\n\n<span style=\"font-family: Arial; font-size: 11pt; color: #FF0000; background-color: #FFFF00\">***Viktigt***</span> <span style=\"font-family: Arial; font-size: 11pt\">att komma ihåg & \\<se></span>\n\n</div>\n\n- <span style=\"font-size: 11pt\"><u>Punkt</u></span>\n    - <span style=\"font-size: 11pt\">Under</span>"
        );
    }

    #[test]
    fn web() {
        let html = format!(
            r#"<h2 class="title">Nyhet</h2><p>Se <a href="https://example.org/x?a=1&amp;b=2" style="color:blue">sidan</a>   nu.
Ny rad.</p><ul><li>Ett</li><li>Tv&aring;<ul><li>Inre</li></ul></li></ul><p><img src="data:image/png;base64,{PNG}" width="120" alt="Liten"> <img src="https://example.org/b.png" alt="Webbild"></p><p><img src="data:image/png;base64,{PNG}"></p>"#
        );
        let p = html_to_markdown(&html, None);
        assert_eq!(
            p.markdown,
            "## Nyhet\n\nSe [sidan](https://example.org/x?a=1&b=2) nu. Ny rad.\n\n- Ett\n- Två\n    - Inre\n\n<img src=\"bild:inklistrad-1.png\" alt=\"Liten\" width=\"120\"> Webbild\n\n![](bild:inklistrad-2.png)"
        );
        assert_eq!(p.images.len(), 2);
        assert_eq!(p.images[0].0, "inklistrad-1.png");
        assert!(p.images[0].1.bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn base_format_is_ignored() {
        let d = DocDefaults { font_family: Some("Arial".into()), font_size: Some(12.0), ..Default::default() };
        let html = "<p><span style=\"font-family:Arial;font-size:12pt\">a</span><span style=\"font-family:Arial;font-size:14pt\">b</span></p>";
        assert_eq!(html_to_markdown(html, Some(&d)).markdown, "a<span style=\"font-size: 14pt\">b</span>");
    }

    #[test]
    fn escapes_markdown() {
        assert_eq!(
            md("<p>2*3 _x_ `k` [a]</p><p># inte rubrik</p><p>1. inte lista</p>"),
            "2\\*3 \\_x\\_ \\`k\\` \\[a\\]\n\n\\# inte rubrik\n\n1\\. inte lista"
        );
    }

    #[test]
    fn spaces_stay_outside_markers() {
        assert_eq!(md("<p>a<b> fet </b>b</p>"), "a **fet** b");
    }

    #[test]
    fn unique_image_names() {
        let mut p = html_to_markdown(&format!("<img src=\"data:image/png;base64,{PNG}\">"), None);
        let ex = vec![("inklistrad-1.png".to_owned(), Image { bytes: vec![], extent: None })];
        make_names_unique(&mut p, &ex);
        assert_eq!(p.images[0].0, "inklistrad-1-2.png");
        assert_eq!(p.markdown, "![](bild:inklistrad-1-2.png)");
    }

    #[test]
    fn insert_inline_and_block() {
        let e = insert("abc def", 3..4, "X");
        assert_eq!((e.text.as_str(), e.sel), ("abcXdef", 4..4));
        let e = insert("abcdef", 3..3, "# R");
        assert_eq!(e.text, "abc\n\n# R\n\ndef");
        let e = insert("", 0..0, "a\n\nb");
        assert_eq!(e.text, "a\n\nb\n");
        let e = insert("x\n\n", 3..3, "- a");
        assert_eq!(e.text, "x\n\n- a\n");
    }

    #[test]
    fn output_reanalyzes_cleanly() {
        let m = md(WORD);
        let a = markup::analyze(&m);
        assert!(a.pieces.iter().any(|p| !p.markup));
        // Alla `<` som finns kvar ska höra till våra egna taggar.
        let rest = m
            .replace("<span style=\"color: #C00000\">", "")
            .replace("</span>", "")
            .replace("<div align=\"center\">", "")
            .replace("</div>", "");
        assert!(!rest.contains('<'), "{rest}");
    }
}
