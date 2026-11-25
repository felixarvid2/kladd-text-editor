//! Ett kapitel (XHTML) som Markdown för läsläget: rubriker, stycken, fet och kursiv stil, länkar, bilder,
//! listor, citat, kodblock, tabeller, upphöjt och nedsänkt. Skript, stilmallar och sidhuvud hoppas över.
//!
//! Källan är ofta slarvig, så träet byggs av den tåliga tolken i `paste.rs` och ingenting här får
//! paniken. Länkmål (alla element med `id`) bärs genom omvandlingen som osynliga markörer i texten och
//! läses av i slutet, så att varje mål får en byteposition i den färdiga Markdown-texten. Länkar
//! till andra delar av boken skrivs som `kl:KAPITEL:N` (se `Book::link`), bilder som `bytes://`-adresser.

use std::collections::HashSet;

use super::read::{dir_of, has_scheme, local, plain_text, resolve};
use crate::paste::{El, Node};

/// Markörer i texten under omvandlingen (privat teckenyta, finns inte i böcker).
const A_OPEN: char = '\u{E000}';
const A_CLOSE: char = '\u{E001}';
const HARD: char = '\u{E002}';

/// En länk till ett annat ställe i boken.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkRef {
    pub path: String,
    pub frag: Option<String>,
    /// En fotnotsreferens: klicket visar noten i en liten ruta.
    pub noteref: bool,
}

/// Omgivningen för ett kapitel.
pub struct Env<'a> {
    pub doc_id: u64,
    pub chapter: usize,
    /// Kapitelfilens sökväg i arkivet.
    pub file: &'a str,
    /// Sökväg i arkivet med arkivets egna versaler, om filen finns.
    pub canon: &'a dyn Fn(&str) -> Option<String>,
    /// Mål (`sökväg#id`) som fotnotsreferenser pekar på: deras text sparas som not.
    pub note_targets: &'a HashSet<String>,
}

#[derive(Default, Debug)]
pub struct Converted {
    pub md: String,
    /// Länkmål: id och byteposition i `md`.
    pub anchors: Vec<(String, usize)>,
    pub links: Vec<LinkRef>,
    /// Bildfiler som kapitlet visar (sökvägar i arkivet).
    pub images: Vec<String>,
    /// Noter: id och text (Markdown).
    pub notes: Vec<(String, String)>,
    /// Första rubriken.
    pub heading: Option<String>,
}

/// Adressen en bild i arkivet visas med i läsläget. SVG ritas om till PNG och får `.png` på slutet.
pub fn image_uri(doc_id: u64, path: &str) -> String {
    let mut enc = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => enc.push_str("%20"),
            '(' => enc.push_str("%28"),
            ')' => enc.push_str("%29"),
            '#' => enc.push_str("%23"),
            '?' => enc.push_str("%3F"),
            _ => enc.push(c),
        }
    }
    let svg = if path.to_ascii_lowercase().ends_with(".svg") { ".png" } else { "" };
    format!("bytes://kladd-epub/{doc_id}/{enc}{svg}")
}

// ------------------------------------------------------------------ fotnotsreferenser

fn has_word(attr: Option<&str>, word: &str) -> bool {
    attr.is_some_and(|a| a.split_whitespace().any(|w| w == word))
}

fn class_has(e: &El, needle: &str) -> bool {
    e.attr("class").is_some_and(|c| c.to_ascii_lowercase().contains(needle))
}

/// Kort text av siffror och symboler (`1`, `[12]`, `*`, `†`): ser ut som en notmarkör.
fn is_marker_text(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty()
        && t.chars().count() <= 5
        && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '[' | ']' | '(' | ')' | '*' | '†' | '‡' | '§' | '¶' | '.' | ',') || ('¹'..='⁹').contains(&c) || c == '⁰')
}

fn is_noteref(a: &El, in_sup: bool) -> bool {
    if has_word(a.attr("epub:type"), "noteref") || a.attr("role") == Some("doc-noteref") {
        return true;
    }
    if class_has(a, "noteref") || class_has(a, "footnote-ref") || class_has(a, "fnref") {
        return true;
    }
    let sup_inside = a.kids.iter().any(|k| matches!(k, Node::El(e) if local(&e.tag) == "sup"));
    (in_sup || sup_inside || class_has(a, "footnote")) && is_marker_text(&plain_text(a))
}

fn is_backlink(a: &El) -> bool {
    if has_word(a.attr("epub:type"), "backlink") || a.attr("role") == Some("doc-backlink") {
        return true;
    }
    if class_has(a, "backlink") || class_has(a, "footnote-back") || class_has(a, "fnback") {
        return true;
    }
    matches!(plain_text(a).trim(), "↩" | "↩︎" | "↑" | "⏎" | "^" | "[↩]" | "↵" | "⤴")
}

/// Mål som fotnotsreferenser i kapitlet pekar på (`sökväg#id`).
pub fn scan_note_targets(root: &El, file: &str, canon: &dyn Fn(&str) -> Option<String>, out: &mut HashSet<String>) {
    fn go(el: &El, in_sup: bool, file: &str, canon: &dyn Fn(&str) -> Option<String>, out: &mut HashSet<String>) {
        for k in &el.kids {
            let Node::El(e) = k else { continue };
            let tag = local(&e.tag);
            if tag == "a" {
                if let Some(h) = e.attr("href").filter(|h| !h.is_empty() && !has_scheme(h)) {
                    if is_noteref(e, in_sup) {
                        let (p, frag) = resolve(dir_of(file), h);
                        let p = if p.is_empty() { file.to_owned() } else { p };
                        if let (Some(frag), Some(c)) = (frag, canon(&p)) {
                            out.insert(format!("{c}#{frag}"));
                        }
                    }
                }
            }
            go(e, in_sup || tag == "sup", file, canon, out);
        }
    }
    go(root, false, file, canon, out);
}

// ------------------------------------------------------------------ omvandlingen

pub fn convert(root: &El, env: &Env) -> Converted {
    let mut c = Conv { env, out: Converted::default(), note_ids: Vec::new(), sup_depth: 0, script: None };
    let body = find_body(root).unwrap_or(root);
    let mut md = c.blocks(&body.kids);
    // Rubrikens eget id på <body>.
    if let Some(id) = body.attr("id") {
        md = format!("{A_OPEN}{id}{A_CLOSE}{md}");
    }
    c.finish(md)
}

fn find_body(el: &El) -> Option<&El> {
    for k in &el.kids {
        if let Node::El(e) = k {
            if local(&e.tag) == "body" {
                return Some(e);
            }
            if let Some(b) = find_body(e) {
                return Some(b);
            }
        }
    }
    None
}

struct Conv<'a, 'e> {
    env: &'a Env<'e>,
    out: Converted,
    /// Id:n på målelement i det stycke som byggs (noter som ska få styckets text).
    note_ids: Vec<String>,
    sup_depth: u32,
    /// Upphöjt (true) eller nedsänkt (false) text.
    script: Option<bool>,
}

fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "li" | "dl" | "dt" | "dd" | "blockquote" | "pre"
            | "hr" | "table" | "section" | "article" | "aside" | "header" | "footer" | "main" | "nav" | "figure"
            | "figcaption" | "address" | "center" | "form" | "fieldset" | "details" | "summary" | "body" | "html"
            | "caption" | "tr" | "td" | "th" | "thead" | "tbody" | "tfoot"
    )
}

fn has_block_desc(e: &El) -> bool {
    e.kids.iter().any(|k| matches!(k, Node::El(c) if is_block_tag(local(&c.tag)) || has_block_desc(c)))
}

/// Element vars innehåll aldrig visas.
fn is_skipped(e: &El) -> bool {
    let tag = local(&e.tag);
    if matches!(
        tag,
        "script" | "style" | "head" | "meta" | "link" | "title" | "rt" | "rp" | "audio" | "video" | "object" | "iframe"
            | "canvas" | "noscript" | "template" | "map" | "base" | "xml"
    ) {
        return true;
    }
    if e.attr("hidden").is_some() {
        return true;
    }
    if let Some(s) = e.attr("style") {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
        if s.contains("display:none") {
            return true;
        }
    }
    has_word(e.attr("epub:type"), "pagebreak") || e.attr("role") == Some("doc-pagebreak")
}

/// Är texten utan markörer tom?
fn visible_empty(s: &str) -> bool {
    let mut skip = false;
    for c in s.chars() {
        match c {
            A_OPEN => skip = true,
            A_CLOSE => skip = false,
            _ if skip => {}
            c if c.is_whitespace() || c == HARD => {}
            _ => return false,
        }
    }
    true
}

/// Bara markörerna i en text.
fn markers_of(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        match c {
            A_OPEN => {
                inside = true;
                out.push(c);
            }
            A_CLOSE => {
                inside = false;
                out.push(c);
            }
            _ if inside => out.push(c),
            _ => {}
        }
    }
    out
}

/// Börjar delen med en listpunkt som får avbryta ett stycke (`- ` eller `1. `)?
fn starts_list(part: &str) -> bool {
    let s = strip_markers(part);
    let s = s.trim_start();
    s.starts_with("- ") || s.starts_with("1. ")
}

fn ends_in_code(part: &str) -> bool {
    part.trim_end().ends_with("```")
}

fn strip_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        match c {
            A_OPEN => inside = true,
            A_CLOSE => inside = false,
            _ if inside => {}
            c => out.push(c),
        }
    }
    out
}

fn collapse_ws(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut sp = false;
    for c in t.chars() {
        // Osynliga tecken som böcker lägger in för radbrytning (ordbindare, mjuka bindestreck) visas inte.
        if matches!(c, '\u{feff}' | '\u{2060}' | '\u{200b}' | '\u{ad}') {
            continue;
        }
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c') {
            if !sp {
                out.push(' ');
                sp = true;
            }
        } else {
            out.push(c);
            sp = false;
        }
    }
    out
}

/// Skyddar Markdown-tecken i löptext.
fn escape(t: &str) -> String {
    let mut out = String::with_capacity(t.len() + 4);
    for c in t.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '&' | '~' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Skyddar tecken som i radens början blir Markdown-syntax (rubrik, citat, lista, rad).
fn escape_line_start(line: &str) -> String {
    // Första synliga tecknet: efter blanksteg och markörer.
    let mut start = line.len();
    let mut inside = false;
    for (idx, c) in line.char_indices() {
        match c {
            A_OPEN => inside = true,
            A_CLOSE => inside = false,
            _ if inside => {}
            ' ' => {}
            _ => {
                start = idx;
                break;
            }
        }
    }
    let rest = &line[start..];
    let Some(first) = rest.chars().next() else { return line.to_owned() };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let numbered = digits > 0
        && matches!(rest.as_bytes().get(digits), Some(b'.' | b')'))
        && rest[digits + 1..].chars().next().is_none_or(|c| c == ' ');
    if matches!(first, '#' | '>' | '-' | '+' | '=' | ':') {
        format!("{}\\{}", &line[..start], rest)
    } else if numbered {
        format!("{}{}\\{}", &line[..start], &rest[..digits], &rest[digits..])
    } else {
        line.to_owned()
    }
}

fn emph(inner: String, mark: &str) -> String {
    let is_pad = |c: char| c == ' ' || c == HARD;
    let core = inner.trim_matches(is_pad);
    if visible_empty(core) {
        return inner;
    }
    let lead_len = inner.len() - inner.trim_start_matches(is_pad).len();
    let trail_start = lead_len + core.len();
    format!("{}{mark}{core}{mark}{}", &inner[..lead_len], &inner[trail_start..])
}

fn code_span(text: &str) -> String {
    let text = text.replace(['\n', '\r'], " ");
    if text.trim().is_empty() {
        return String::new();
    }
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') { " " } else { "" };
    format!("{fence}{pad}{text}{pad}{fence}")
}

fn sup_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        ' ' => ' ',
        _ => return None,
    })
}

fn sub_char(c: char) -> Option<char> {
    Some(match c {
        '0'..='9' => char::from_u32('₀' as u32 + (c as u32 - '0' as u32))?,
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        ' ' => ' ',
        _ => return None,
    })
}

/// Upphöjd eller nedsänkt text med Unicode-tecken, om alla tecken finns; annars oförändrad.
fn script_text(t: &str, sup: bool) -> String {
    let map = |c| if sup { sup_char(c) } else { sub_char(c) };
    t.chars().map(map).collect::<Option<String>>().unwrap_or_else(|| t.to_owned())
}

/// Adress i en Markdown-länk.
fn url_enc(u: &str) -> String {
    let mut out = String::with_capacity(u.len());
    for c in u.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn style_has(e: &El, decl: &str) -> bool {
    e.attr("style").is_some_and(|s| s.to_ascii_lowercase().replace(' ', "").contains(decl))
}

fn alt_text(t: &str) -> String {
    t.chars().filter(|c| !matches!(c, '[' | ']' | '\n' | '\r')).collect::<String>().trim().to_owned()
}

impl Conv<'_, '_> {
    fn marker(&mut self, e: &El) -> String {
        let mut m = String::new();
        for key in ["id", "name"] {
            if let Some(id) = e.attr(key).filter(|i| !i.is_empty() && !i.contains([A_OPEN, A_CLOSE])) {
                if m.contains(&format!("{A_OPEN}{id}{A_CLOSE}")) {
                    continue;
                }
                m.push(A_OPEN);
                m.push_str(id);
                m.push(A_CLOSE);
                if self.env.note_targets.contains(&format!("{}#{id}", self.env.file)) {
                    self.note_ids.push(id.to_owned());
                }
            }
        }
        m
    }

    // ---------------------------------------------------------------- block

    /// Ett antal syskon som block, åtskilda av blankrader.
    fn blocks(&mut self, kids: &[Node]) -> String {
        self.blocks_ex(kids, false)
    }

    /// Som `blocks`; med `tight` följer en lista direkt på texten före (en nästlad lista i en listpunkt).
    fn blocks_ex(&mut self, kids: &[Node], tight: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut run: Vec<&Node> = Vec::new();
        let mut pending = String::new();
        for k in kids {
            match k {
                Node::Text(_) => run.push(k),
                Node::El(e) => {
                    if is_skipped(e) {
                        pending.push_str(&self.marker(e));
                        continue;
                    }
                    let tag = local(&e.tag);
                    if is_block_tag(tag) || has_block_desc(e) {
                        self.flush(&mut run, &mut parts, &mut pending);
                        self.block_el(e, &mut parts, &mut pending);
                    } else {
                        run.push(k);
                    }
                }
            }
        }
        self.flush(&mut run, &mut parts, &mut pending);
        if !pending.is_empty() {
            parts.push(std::mem::take(&mut pending));
        }
        let mut out = String::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                let glue = if tight && starts_list(p) && !starts_list(&parts[i - 1]) && !ends_in_code(&parts[i - 1]) { "\n" } else { "\n\n" };
                out.push_str(glue);
            }
            out.push_str(p);
        }
        out
    }

    /// Lägger en färdig del till; en tom del lämnar bara sina markörer till nästa.
    fn push_part(parts: &mut Vec<String>, pending: &mut String, text: String) {
        if visible_empty(&text) {
            pending.push_str(&markers_of(&text));
        } else {
            let mut t = std::mem::take(pending);
            t.push_str(&text);
            parts.push(t);
        }
    }

    /// Löpande text och inline-element som ett stycke.
    fn flush(&mut self, run: &mut Vec<&Node>, parts: &mut Vec<String>, pending: &mut String) {
        if run.is_empty() {
            return;
        }
        let mut raw = String::new();
        for k in run.drain(..) {
            self.inline_node(k, false, false, &mut raw);
        }
        let text = self.paragraph(&raw);
        self.register_notes(None, &text);
        Self::push_part(parts, pending, text);
    }

    /// Noter vars mål ligger i texten (eller är elementet självt) får texten.
    fn register_notes(&mut self, own: Option<&El>, text: &str) {
        if visible_empty(text) {
            return;
        }
        let mut ids = std::mem::take(&mut self.note_ids);
        if let Some(e) = own {
            if let Some(id) = e.attr("id").filter(|id| self.env.note_targets.contains(&format!("{}#{id}", self.env.file))) {
                if !ids.iter().any(|i| i == id) {
                    ids.push(id.to_owned());
                }
            }
        }
        if ids.is_empty() {
            return;
        }
        let clean = strip_markers(text).replace(HARD, " ");
        for id in ids {
            self.out.notes.push((id, clean.trim().to_owned()));
        }
    }

    /// Hopdraget inline-innehåll som ett Markdown-stycke.
    fn paragraph(&self, raw: &str) -> String {
        // Blanksteg: ett enda, inte kring hårda radbrytningar och inte först eller sist.
        let mut s = String::with_capacity(raw.len());
        let mut sp = false;
        for c in raw.chars() {
            if c == ' ' {
                if !sp {
                    s.push(' ');
                }
                sp = true;
            } else {
                if c == HARD {
                    while s.ends_with(' ') {
                        s.pop();
                    }
                    sp = true;
                } else {
                    sp = false;
                }
                s.push(c);
            }
        }
        // Radbrytningar först eller sist i stycket behövs inte.
        let s = s.trim_matches(|c| c == ' ' || c == HARD);
        // Markörer kan stå före inledande blanksteg.
        let lines: Vec<String> = s
            .split(HARD)
            .map(|l| escape_line_start(l.trim_start_matches(' ').trim_end_matches(' ')))
            .filter(|l| !l.is_empty())
            .collect();
        lines.join("\\\n")
    }

    fn block_el(&mut self, e: &El, parts: &mut Vec<String>, pending: &mut String) {
        let tag = local(&e.tag);
        let mark = self.marker(e);
        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = (tag.as_bytes()[1] - b'0') as usize;
                let mut raw = String::new();
                for k in &e.kids {
                    self.inline_node(k, false, false, &mut raw);
                }
                let text = self.paragraph(&raw.replace(HARD, " "));
                let text = text.replace("\\\n", " ");
                if visible_empty(&text) {
                    pending.push_str(&mark);
                    pending.push_str(&markers_of(&text));
                    return;
                }
                if self.out.heading.is_none() {
                    self.out.heading = Some(plain_text(e));
                }
                let t = format!("{mark}{} {}", "#".repeat(level), text);
                self.register_notes(Some(e), &t);
                Self::push_part(parts, pending, t);
            }
            "p" | "address" | "summary" | "caption" | "figcaption" | "dt" => {
                let mut raw = String::new();
                for k in &e.kids {
                    self.inline_node(k, false, false, &mut raw);
                }
                let mut text = self.paragraph(&raw);
                if tag == "dt" || style_has(e, "font-weight:bold") || style_has(e, "font-weight:700") {
                    text = emph(text, "**");
                }
                if style_has(e, "font-style:italic") {
                    text = emph(text, "*");
                }
                let text = format!("{mark}{text}");
                self.register_notes(Some(e), &text);
                Self::push_part(parts, pending, text);
            }
            "ul" | "ol" => {
                let text = self.list(e, tag == "ol");
                let text = format!("{mark}{text}");
                Self::push_part(parts, pending, text);
            }
            "blockquote" => {
                let inner = self.blocks(&e.kids);
                let text = quote(&inner);
                self.register_notes(Some(e), &text);
                let text = format!("{mark}{text}");
                Self::push_part(parts, pending, text);
            }
            "pre" => {
                let text = self.code_block(e);
                Self::push_part(parts, pending, format!("{mark}{text}"));
            }
            "hr" => Self::push_part(parts, pending, format!("{mark}---")),
            "table" => {
                let text = self.table(e);
                Self::push_part(parts, pending, format!("{mark}{text}"));
            }
            // Behållare: innehållet som egna block.
            _ => {
                let inner = self.blocks(&e.kids);
                self.register_notes(Some(e), &inner);
                let text = format!("{mark}{inner}");
                // Behållarens delar är redan åtskilda; en tom behållare lämnar bara markörer.
                Self::push_part(parts, pending, text);
            }
        }
    }

    fn list(&mut self, e: &El, ordered: bool) -> String {
        let start = e.attr("start").and_then(|s| s.trim().parse::<i64>().ok()).filter(|n| *n >= 0).unwrap_or(1);
        let mut items: Vec<String> = Vec::new();
        let mut n = start;
        let mut carry = String::new();
        for k in &e.kids {
            let Node::El(li) = k else { continue };
            if is_skipped(li) {
                carry.push_str(&self.marker(li));
                continue;
            }
            let mark = self.marker(li);
            let body = if local(&li.tag) == "li" {
                self.blocks_ex(&li.kids, true)
            } else {
                // Något annat än <li> i listan: som ett eget stycke.
                self.blocks(std::slice::from_ref(k))
            };
            self.register_notes(Some(li), &body);
            let body = format!("{carry}{mark}{body}");
            if visible_empty(&body) {
                carry = markers_of(&body);
                continue;
            }
            carry.clear();
            let marker = if ordered { format!("{n}. ") } else { "- ".to_owned() };
            n += 1;
            let indent = " ".repeat(marker.len());
            let mut out = String::new();
            for (i, line) in body.split('\n').enumerate() {
                if i == 0 {
                    out.push_str(&marker);
                    out.push_str(line);
                } else if line.is_empty() {
                    // blankrad
                } else {
                    out.push_str(&indent);
                    out.push_str(line);
                }
                out.push('\n');
            }
            items.push(out.trim_end_matches('\n').to_owned());
        }
        let mut s = items.join("\n");
        if !carry.is_empty() {
            s.push_str(&carry);
        }
        s
    }

    fn code_block(&mut self, e: &El) -> String {
        let mut text = String::new();
        raw_text(e, &mut text);
        let text = text.trim_matches('\n');
        if text.trim().is_empty() {
            return String::new();
        }
        let lang = find_code(e)
            .and_then(|c| c.attr("class"))
            .and_then(|c| c.split_whitespace().find_map(|w| w.strip_prefix("language-")))
            .unwrap_or("")
            .to_owned();
        let longest = text.lines().map(|l| l.trim_start().chars().take_while(|c| *c == '`').count()).max().unwrap_or(0);
        let fence = "`".repeat((longest + 1).max(3));
        format!("{fence}{lang}\n{text}\n{fence}")
    }

    fn table(&mut self, e: &El) -> String {
        let mut rows: Vec<(bool, Vec<String>)> = Vec::new();
        let mut caption = String::new();
        self.collect_rows(e, &mut rows, &mut caption);
        rows.retain(|(_, c)| !c.is_empty());
        if rows.is_empty() {
            return caption;
        }
        let cols = rows.iter().map(|(_, c)| c.len()).max().unwrap_or(1);
        let line = |cells: &[String]| {
            let mut s = String::from("|");
            for i in 0..cols {
                s.push(' ');
                s.push_str(cells.get(i).map_or("", String::as_str));
                s.push_str(" |");
            }
            s
        };
        let mut out = String::new();
        if !caption.is_empty() {
            out.push_str(&caption);
            out.push_str("\n\n");
        }
        out.push_str(&line(&rows[0].1));
        out.push('\n');
        out.push('|');
        for _ in 0..cols {
            out.push_str(" --- |");
        }
        for (_, cells) in &rows[1..] {
            out.push('\n');
            out.push_str(&line(cells));
        }
        out
    }

    fn collect_rows(&mut self, e: &El, rows: &mut Vec<(bool, Vec<String>)>, caption: &mut String) {
        for k in &e.kids {
            let Node::El(c) = k else { continue };
            match local(&c.tag) {
                "caption" => {
                    let mut raw = String::new();
                    for k in &c.kids {
                        self.inline_node(k, false, false, &mut raw);
                    }
                    let t = self.paragraph(&raw);
                    *caption = emph(t.replace("\\\n", " "), "*");
                }
                "thead" | "tbody" | "tfoot" => self.collect_rows(c, rows, caption),
                "tr" => {
                    let mut cells = Vec::new();
                    let mut header = true;
                    for k in &c.kids {
                        let Node::El(cell) = k else { continue };
                        let t = local(&cell.tag);
                        if t != "td" && t != "th" {
                            continue;
                        }
                        header &= t == "th";
                        let inner = self.blocks(&cell.kids);
                        let text = strip_markers(&inner).replace(HARD, " ").replace("\\\n", " ").replace('\n', " ");
                        cells.push(text.split_whitespace().collect::<Vec<_>>().join(" "));
                    }
                    rows.push((header, cells));
                }
                _ => {}
            }
        }
    }

    // ---------------------------------------------------------------- inline

    fn inline_node(&mut self, k: &Node, bold: bool, italic: bool, out: &mut String) {
        match k {
            Node::Text(t) => {
                let t = collapse_ws(t);
                match self.script {
                    Some(sup) => out.push_str(&escape(&script_text(&t, sup))),
                    None => out.push_str(&escape(&t)),
                }
            }
            Node::El(e) => self.inline_el(e, bold, italic, out),
        }
    }

    fn inline_kids(&mut self, e: &El, bold: bool, italic: bool) -> String {
        let mut s = String::new();
        for k in &e.kids {
            self.inline_node(k, bold, italic, &mut s);
        }
        s
    }

    fn inline_el(&mut self, e: &El, bold: bool, italic: bool, out: &mut String) {
        let tag = local(&e.tag);
        if is_skipped(e) {
            out.push_str(&self.marker(e));
            return;
        }
        let mark = self.marker(e);
        out.push_str(&mark);
        let style_bold = || {
            let s = e.attr("style").unwrap_or("").to_ascii_lowercase().replace(' ', "");
            s.contains("font-weight:bold") || s.contains("font-weight:700") || s.contains("font-weight:800") || s.contains("font-weight:900")
        };
        let style_italic = || e.attr("style").unwrap_or("").to_ascii_lowercase().replace(' ', "").contains("font-style:italic");
        match tag {
            "br" => out.push(HARD),
            "img" => out.push_str(&self.image(e.attr("src"), e.attr("alt").unwrap_or(""))),
            "svg" => {
                // SVG som omslag: <svg><image xlink:href="…"/></svg>.
                let mut href = None;
                find_svg_image(e, &mut href);
                if let Some(h) = href {
                    out.push_str(&self.image(Some(&h), ""));
                }
            }
            "a" => self.link(e, bold, italic, out),
            "b" | "strong" => {
                let inner = self.inline_kids(e, true, italic);
                out.push_str(&if bold { inner } else { emph(inner, "**") });
            }
            "i" | "em" | "cite" | "dfn" | "var" => {
                let inner = self.inline_kids(e, bold, true);
                out.push_str(&if italic { inner } else { emph(inner, "*") });
            }
            "s" | "strike" | "del" => {
                let inner = self.inline_kids(e, bold, italic);
                out.push_str(&emph(inner, "~~"));
            }
            "code" | "kbd" | "samp" | "tt" => {
                let mut t = String::new();
                raw_text(e, &mut t);
                out.push_str(&code_span(&t));
            }
            "sup" | "sub" => {
                let saved = (self.script, self.sup_depth);
                self.script = Some(tag == "sup");
                if tag == "sup" {
                    self.sup_depth += 1;
                }
                let inner = self.inline_kids(e, bold, italic);
                (self.script, self.sup_depth) = saved;
                out.push_str(&inner);
            }
            _ => {
                let b = bold || style_bold();
                let i = italic || style_italic();
                let inner = self.inline_kids(e, b, i);
                let inner = if style_bold() && !bold { emph(inner, "**") } else { inner };
                let inner = if style_italic() && !italic { emph(inner, "*") } else { inner };
                out.push_str(&inner);
                // Ett blockelement mitt i löptext (trasig markup): håll isär texten.
                if is_block_tag(tag) {
                    out.push(' ');
                }
            }
        }
    }

    fn image(&mut self, src: Option<&str>, alt: &str) -> String {
        let Some(src) = src.filter(|s| !s.is_empty() && !has_scheme(s)) else { return String::new() };
        let (p, _) = resolve(dir_of(self.env.file), src);
        let Some(canon) = (self.env.canon)(&p) else { return String::new() };
        if !self.out.images.contains(&canon) {
            self.out.images.push(canon.clone());
        }
        format!("![{}]({})", alt_text(alt), image_uri(self.env.doc_id, &canon))
    }

    fn link(&mut self, e: &El, bold: bool, italic: bool, out: &mut String) {
        if is_backlink(e) && e.attr("href").is_some() {
            return;
        }
        let inner = self.inline_kids(e, bold, italic);
        let href = e.attr("href").map(str::trim).filter(|h| !h.is_empty());
        let Some(href) = href else {
            out.push_str(&inner);
            return;
        };
        if visible_empty(&inner) && !inner.contains("![") {
            out.push_str(&markers_of(&inner));
            return;
        }
        // Ett utropstecken före en länk blir annars en bild.
        if out.ends_with('!') {
            out.pop();
            out.push_str("\\!");
        }
        let lower = href.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") || lower.starts_with("tel:") {
            out.push_str(&format!("[{}]({})", inner.trim(), url_enc(href)));
            return;
        }
        if has_scheme(href) {
            out.push_str(&inner);
            return;
        }
        let (p, frag) = resolve(dir_of(self.env.file), href);
        let p = if p.is_empty() { self.env.file.to_owned() } else { p };
        let page = {
            let l = p.to_ascii_lowercase();
            [".xhtml", ".html", ".htm", ".xml"].iter().any(|x| l.ends_with(x))
        };
        let Some(canon) = (self.env.canon)(&p).filter(|_| page) else {
            out.push_str(&inner);
            return;
        };
        let idx = self.out.links.len();
        let noteref = is_noteref(e, self.sup_depth > 0);
        self.out.links.push(LinkRef { path: canon, frag, noteref });
        out.push_str(&format!("[{}](kl:{}:{idx})", inner.trim(), self.env.chapter));
    }

    // ---------------------------------------------------------------- slut

    /// Tar bort markörerna och noterar var varje länkmål hamnade.
    fn finish(mut self, md: String) -> Converted {
        let mut text = String::with_capacity(md.len());
        let mut anchors = Vec::new();
        let mut id = String::new();
        let mut inside = false;
        for c in md.chars() {
            match c {
                A_OPEN => {
                    inside = true;
                    id.clear();
                }
                A_CLOSE => {
                    if inside {
                        anchors.push((std::mem::take(&mut id), text.len()));
                    }
                    inside = false;
                }
                _ if inside => id.push(c),
                HARD => text.push(' '),
                c => text.push(c),
            }
        }
        let trimmed = text.trim_start_matches('\n');
        let cut = text.len() - trimmed.len();
        for a in &mut anchors {
            a.1 = a.1.saturating_sub(cut);
        }
        let mut text = trimmed.trim_end().to_owned();
        text.push('\n');
        for a in &mut anchors {
            a.1 = a.1.min(text.len());
        }
        self.out.md = text;
        self.out.anchors = anchors;
        self.out
    }
}

fn quote(inner: &str) -> String {
    let mut out = String::new();
    for (i, line) in inner.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            out.push('>');
        } else {
            out.push_str("> ");
            out.push_str(line);
        }
    }
    out
}

/// Text med blanksteg och radbrytningar som de står (kodblock och kodtext).
fn raw_text(e: &El, out: &mut String) {
    for k in &e.kids {
        match k {
            Node::Text(t) => out.push_str(t),
            Node::El(c) => match local(&c.tag) {
                "br" => out.push('\n'),
                "script" | "style" => {}
                _ => raw_text(c, out),
            },
        }
    }
}

fn find_code(e: &El) -> Option<&El> {
    e.kids.iter().find_map(|k| match k {
        Node::El(c) if local(&c.tag) == "code" => Some(c),
        _ => None,
    })
}

fn find_svg_image(e: &El, out: &mut Option<String>) {
    for k in &e.kids {
        let Node::El(c) = k else { continue };
        if out.is_some() {
            return;
        }
        if local(&c.tag) == "image" {
            if let Some(h) = c.attr("xlink:href").or_else(|| c.attr("href")) {
                *out = Some(h.to_owned());
                return;
            }
        }
        find_svg_image(c, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paste::parse;

    fn conv(html: &str) -> Converted {
        conv_with(html, &HashSet::new())
    }

    fn conv_with(html: &str, notes: &HashSet<String>) -> Converted {
        let known = ["OEBPS/a.xhtml", "OEBPS/b.xhtml", "OEBPS/img/p 1.png", "OEBPS/cover.svg"];
        let canon = |p: &str| known.iter().find(|k| k.eq_ignore_ascii_case(p)).map(|k| (*k).to_owned());
        let env = Env { doc_id: 7, chapter: 2, file: "OEBPS/a.xhtml", canon: &canon, note_targets: notes };
        convert(&parse(html), &env)
    }

    #[test]
    fn stycken_rubriker_och_stilar() {
        let c = conv(
            "<html><head><title>X</title><style>p{}</style></head><body><h1 id=\"t\">Kapitel &#8212; ett</h1>\
             <p>Hej <b>fet</b> och <i>kursiv</i>, <em>  luft </em>x &amp; y &lt;z&gt; 2*3 a_b [c]</p><p>   </p><script>SKRIPT</script>\
             <p>H<sub>2</sub>O och m<sup>2</sup> samt 1<sup>st</sup></p></body></html>",
        );
        assert!(c.md.starts_with("# Kapitel — ett\n\n"), "{:?}", c.md);
        assert!(c.md.contains("Hej **fet** och *kursiv*, *luft* x \\& y \\<z\\> 2\\*3 a\\_b \\[c\\]"), "{:?}", c.md);
        assert!(c.md.contains("H₂O och m² samt 1st"), "{:?}", c.md);
        assert!(!c.md.contains("SKRIPT"), "skript hoppas över");
        assert_eq!(c.heading.as_deref(), Some("Kapitel — ett"));
        assert_eq!(c.anchors, vec![("t".to_owned(), 0)]);
    }

    #[test]
    fn rad_start_skyddas_och_radbrytning_blir_hård() {
        let c = conv("<body><p>1. Inte lista</p><p># Inte rubrik</p><p>- inte punkt</p><p>rad ett<br/>rad två<br/></p><p>&gt; inte citat</p></body>");
        assert!(c.md.contains("1\\. Inte lista"), "{:?}", c.md);
        assert!(c.md.contains("\\# Inte rubrik"), "{:?}", c.md);
        assert!(c.md.contains("\\- inte punkt"), "{:?}", c.md);
        assert!(c.md.contains("rad ett\\\nrad två\n"), "{:?}", c.md);
        assert!(c.md.contains("\\> inte citat"), "{:?}", c.md);
        // Tolkningen i läsläget ger rätt block.
        use pulldown_cmark::{Event, Parser, Tag};
        let blocks = Parser::new(&c.md).filter(|e| matches!(e, Event::Start(Tag::Paragraph))).count();
        assert_eq!(blocks, 5);
    }

    #[test]
    fn listor_nästlade_citat_kod_och_hr() {
        let c = conv(
            "<body><ul><li>ett<ul><li>ett-ett</li></ul></li><li>två</li></ul><ol start=\"3\"><li><p>a</p><p>b</p></li></ol>\
             <blockquote><p>citat</p><p>mer</p></blockquote><pre><code class=\"language-rs\">fn x() {}\n  y\n</code></pre><hr/></body>",
        );
        assert!(c.md.contains("- ett\n  - ett-ett\n- två"), "{:?}", c.md);
        assert!(c.md.contains("3. a\n\n   b"), "{:?}", c.md);
        assert!(c.md.contains("> citat\n>\n> mer"), "{:?}", c.md);
        assert!(c.md.contains("```rs\nfn x() {}\n  y\n```"), "{:?}", c.md);
        assert!(c.md.trim_end().ends_with("---"), "{:?}", c.md);
        use pulldown_cmark::{Event, Parser, Tag};
        let items = Parser::new(&c.md).filter(|e| matches!(e, Event::Start(Tag::Item))).count();
        assert_eq!(items, 4);
    }

    #[test]
    fn tabell_blir_gfm_med_rubrikrad() {
        let c = conv("<body><table><caption>Tab</caption><thead><tr><th>A</th><th>B | C</th></tr></thead><tbody><tr><td>1</td></tr><tr><td>2</td><td>3</td><td>4</td></tr></tbody></table></body>");
        assert!(c.md.contains("*Tab*\n\n| A | B \\| C |  |\n| --- | --- | --- |\n| 1 |  |  |\n| 2 | 3 | 4 |"), "{:?}", c.md);
        use pulldown_cmark::{Event, Options, Parser, Tag};
        let rows = Parser::new_ext(&c.md, Options::ENABLE_TABLES).filter(|e| matches!(e, Event::Start(Tag::TableRow))).count();
        assert_eq!(rows, 2);
    }

    #[test]
    fn länkar_bilder_och_mål() {
        let c = conv(
            "<body><p id=\"p1\">Se <a href=\"b.xhtml#x\">kapitel b</a>, <a href=\"#p1\">upp</a>, <a href=\"http://x.se/a b\">webb</a>, \
             <a href=\"saknas.xhtml\">död</a>, <a href=\"javascript:x()\">js</a>.</p>\
             <p><img src=\"img/p%201.png\" alt=\"En [bild]\"/> <svg><image xlink:href=\"cover.svg\"/></svg><img src=\"nej.png\"/></p>\
             <div><a id=\"mal\"/></div><p>Efter</p></body>",
        );
        assert!(c.md.contains("[kapitel b](kl:2:0)"), "{:?}", c.md);
        assert!(c.md.contains("[upp](kl:2:1)"), "{:?}", c.md);
        assert!(c.md.contains("[webb](http://x.se/a%20b)"), "{:?}", c.md);
        assert!(c.md.contains("död, js."), "{:?}", c.md);
        assert_eq!(c.links[0], LinkRef { path: "OEBPS/b.xhtml".into(), frag: Some("x".into()), noteref: false });
        assert_eq!(c.links[1], LinkRef { path: "OEBPS/a.xhtml".into(), frag: Some("p1".into()), noteref: false });
        assert!(c.md.contains("![En bild](bytes://kladd-epub/7/OEBPS/img/p%201.png)"), "{:?}", c.md);
        assert!(c.md.contains("(bytes://kladd-epub/7/OEBPS/cover.svg.png)"), "SVG-bilder får .png: {:?}", c.md);
        assert_eq!(c.images, vec!["OEBPS/img/p 1.png".to_owned(), "OEBPS/cover.svg".to_owned()]);
        let at = |id: &str| c.anchors.iter().find(|a| a.0 == id).map(|a| a.1).unwrap();
        assert_eq!(at("p1"), 0);
        // Det tomma målet hamnar där nästa stycke börjar.
        assert!(c.md[at("mal")..].starts_with("Efter"), "{:?} {}", c.md, at("mal"));
    }

    #[test]
    fn fotnoter_samlas_och_bakåtlänkar_tas_bort() {
        let targets: HashSet<String> = ["OEBPS/a.xhtml#fn1".to_owned(), "OEBPS/a.xhtml#fn2".to_owned()].into();
        let c = conv_with(
            "<body><p>Text<a epub:type=\"noteref\" href=\"#fn1\">1</a> och<sup><a href=\"#fn2\">2</a></sup>.</p>\
             <aside epub:type=\"footnote\" id=\"fn1\"><p>Första noten. <a href=\"#r1\">↩</a></p></aside>\
             <ol><li id=\"fn2\">Andra <i>noten</i> <a href=\"#r2\" class=\"backlink\">x</a></li></ol></body>",
            &targets,
        );
        assert!(c.links.iter().all(|l| l.noteref), "{:?}", c.links);
        assert_eq!(c.links.len(), 2);
        let note = |id: &str| c.notes.iter().find(|n| n.0 == id).map(|n| n.1.clone());
        assert_eq!(note("fn1").as_deref(), Some("Första noten."));
        assert_eq!(note("fn2").as_deref(), Some("Andra *noten*"));
        assert!(!c.md.contains('↩'), "{:?}", c.md);
        assert!(c.md.contains("[¹](kl:2:0)") || c.md.contains("[1](kl:2:0)"), "{:?}", c.md);
    }

    #[test]
    fn trasig_xhtml_ger_ingen_panik() {
        for html in [
            "",
            "<p>oavslutat <b>fet",
            "<table><tr><td>a<td>b<tr><td>c</table>",
            "<ul><li>a<li>b</ul></ul></p></div>",
            "<<>><p a='<b>x</p>&bogus; &#xFFFFFFFF; &",
            "<a href=''>tom</a><a>ingen</a><img><svg><image/></svg><h2></h2><p><br></p>",
            "<body><div><div><div><div></div></div></div></div><pre></pre><table></table><ol start=\"x\"><li></li></ol>",
        ] {
            let _ = conv(html);
        }
        let deep = "<div>".repeat(5000) + "x";
        let c = conv(&deep);
        assert!(c.md.contains('x'));
    }

    #[test]
    fn dolda_element_och_sidbrytningar_visas_inte() {
        let c = conv("<body><p>A<span epub:type=\"pagebreak\" title=\"12\">12</span>B</p><p hidden=\"\">dold</p><p style=\"display: none\">dold2</p><p style=\"font-weight:bold\">fet div</p></body>");
        assert!(c.md.contains("AB"), "{:?}", c.md);
        assert!(!c.md.contains("dold") && !c.md.contains("12"), "{:?}", c.md);
        assert!(c.md.contains("**fet div**"), "{:?}", c.md);
    }

    #[test]
    fn definitionslista_och_kodtext() {
        let c = conv("<body><dl><dt>Term</dt><dd>Förklaring</dd></dl><p>Kör <code>a`b</code> nu</p></body>");
        assert!(c.md.contains("**Term**\n\nFörklaring"), "{:?}", c.md);
        assert!(c.md.contains("``a`b``") || c.md.contains("`` a`b ``"), "{:?}", c.md);
    }
}
