//! Typsnitt, storlek, färger och justering från Word, skrivna som HTML i Markdown-texten.
//!
//! Formatet (samma i läsning, skrivning, PDF och dokumentläget):
//!
//! - Tecken: `<span style="font-family: Arial; font-size: 14pt; color: #C00000; background-color: #FFFF00">…</span>`.
//!   Bara egenskaper som skiljer sig från dokumentets grundformat skrivs ut.
//! - Stycken: `<div align="center">`, tom rad, styckena, tom rad, `</div>`.
//!   `left` är standard och skrivs aldrig ut.
//! - Dokumentets grundformat, första raden:
//!   `<!-- kladd: font-family: "Times New Roman"; font-size: 12pt -->`.
//!   Saknas raden gäller Calibri 11 pt.
#![allow(dead_code)]

/// Grundtypsnittet när dokumentet inte anger något (samma som `docx::write` skriver).
pub const DEFAULT_FONT: &str = "Calibri";
pub const DEFAULT_SIZE: f32 = 11.0;

/// Teckenformat från en `<span style="…">`. `None` = ärvs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpanStyle {
    pub font_family: Option<String>,
    /// Punkter.
    pub font_size: Option<f32>,
    pub color: Option<[u8; 3]>,
    pub background: Option<[u8; 3]>,
}

impl SpanStyle {
    pub fn is_empty(&self) -> bool {
        self.font_family.is_none() && self.font_size.is_none() && self.color.is_none() && self.background.is_none()
    }

    /// `inner` ovanpå `self` (inre span vinner).
    pub fn merged(&self, inner: &SpanStyle) -> SpanStyle {
        SpanStyle {
            font_family: inner.font_family.clone().or_else(|| self.font_family.clone()),
            font_size: inner.font_size.or(self.font_size),
            color: inner.color.or(self.color),
            background: inner.background.or(self.background),
        }
    }

    /// CSS för `style`-attributet, t.ex. `font-family: Arial; color: #C00000`.
    pub fn to_css(&self) -> String {
        let mut parts = Vec::new();
        if let Some(f) = &self.font_family {
            parts.push(format!("font-family: {}", css_font_name(f)));
        }
        if let Some(s) = self.font_size {
            parts.push(format!("font-size: {}pt", fmt_pt(s)));
        }
        if let Some(c) = self.color {
            parts.push(format!("color: {}", hex(c)));
        }
        if let Some(c) = self.background {
            parts.push(format!("background-color: {}", hex(c)));
        }
        parts.join("; ")
    }

    /// Hela öppningstaggen: `<span style="…">`.
    pub fn open_tag(&self) -> String {
        format!("<span style=\"{}\">", self.to_css().replace('"', "'"))
    }

    /// Läser `style`-attributets innehåll. Okända egenskaper ignoreras.
    pub fn from_css(css: &str) -> SpanStyle {
        let mut s = SpanStyle::default();
        for decl in css.split(';') {
            let Some((k, v)) = decl.split_once(':') else { continue };
            let v = v.trim();
            match k.trim().to_ascii_lowercase().as_str() {
                "font-family" => {
                    let first = split_font_list(v).into_iter().next();
                    s.font_family = first.filter(|f| !f.is_empty());
                }
                "font-size" => s.font_size = parse_size(v),
                "color" => s.color = parse_color(v),
                "background-color" | "background" => s.background = parse_color(v),
                _ => {}
            }
        }
        s
    }
}

/// Läser `style="…"` ur en öppningstagg som `<span style="color: red">`. `None` om det inte är en span.
pub fn parse_span_open(tag: &str) -> Option<SpanStyle> {
    let t = tag.trim();
    let lower = t.to_ascii_lowercase();
    if !(lower.starts_with("<span") && t.ends_with('>')) {
        return None;
    }
    let rest = &lower[5..];
    if !(rest.starts_with('>') || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    Some(attr(t, "style").map(|css| SpanStyle::from_css(&css)).unwrap_or_default())
}

pub fn is_span_close(tag: &str) -> bool {
    tag.trim().eq_ignore_ascii_case("</span>")
}

/// Styckejustering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    pub fn from_attr(v: &str) -> Align {
        match v.trim().to_ascii_lowercase().as_str() {
            "center" | "centre" | "middle" => Align::Center,
            "right" | "end" => Align::Right,
            "justify" | "both" | "distribute" => Align::Justify,
            _ => Align::Left,
        }
    }

    pub fn as_attr(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "justify",
        }
    }

    /// Words `w:jc`-värde.
    pub fn from_jc(v: &str) -> Align {
        match v {
            "center" => Align::Center,
            "right" | "end" => Align::Right,
            "both" | "distribute" | "lowKashida" | "mediumKashida" | "highKashida" | "thaiDistribute" => Align::Justify,
            _ => Align::Left,
        }
    }

    pub fn as_jc(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "both",
        }
    }
}

/// Läser en rad som `<div align="center">`. `None` om det inte är en div-öppning.
pub fn parse_div_open(line: &str) -> Option<Align> {
    let t = line.trim();
    let lower = t.to_ascii_lowercase();
    if !(lower.starts_with("<div") && t.ends_with('>')) {
        return None;
    }
    let rest = &lower[4..];
    if !(rest.starts_with('>') || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    if let Some(a) = attr(t, "align") {
        return Some(Align::from_attr(&a));
    }
    if let Some(css) = attr(t, "style") {
        for decl in css.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                if k.trim().eq_ignore_ascii_case("text-align") {
                    return Some(Align::from_attr(v));
                }
            }
        }
    }
    Some(Align::Left)
}

pub fn is_div_close(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case("</div>")
}

pub fn div_open(align: Align) -> String {
    format!("<div align=\"{}\">", align.as_attr())
}

/// Dokumentets grundformat (första raden `<!-- kladd: … -->`): typsnitt, storlek, sida,
/// marginaler, sidhuvud och sidfot.
///
/// ```text
/// <!-- kladd: font-family: "Times New Roman"; font-size: 12pt; page-size: 210mm 297mm;
///      page-margin: 25mm 20mm 25mm 20mm; header: "Rapport"; header-align: right;
///      footer: "Sida {sida} av {sidor}"; footer-align: center -->
/// ```
///
/// Allt står på en rad. `page-size` är `A4`, `A5`, `A3`, `Letter`, `Legal` eller bredd och höjd
/// (`mm`, `cm`, `in`, `pt`); `landscape` efter namnet vänder sidan. `page-margin` följer CSS:
/// ett värde för alla, eller topp höger botten vänster. I `header`/`footer` står `{sida}` för
/// sidnumret och `{sidor}` för antalet sidor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocDefaults {
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    /// Sidans bredd och höjd i pt.
    pub page_size: Option<(f32, f32)>,
    /// Marginaler i pt: topp, höger, botten, vänster.
    pub page_margin: Option<[f32; 4]>,
    pub header: Option<String>,
    pub header_align: Align,
    pub footer: Option<String>,
    pub footer_align: Align,
}

/// A4 i pt.
pub const A4: (f32, f32) = (595.28, 841.89);
/// En tum.
pub const DEFAULT_MARGIN: f32 = 72.0;

impl DocDefaults {
    pub fn is_empty(&self) -> bool {
        self.to_comment().is_empty()
    }

    pub fn font(&self) -> &str {
        self.font_family.as_deref().unwrap_or(DEFAULT_FONT)
    }

    pub fn size(&self) -> f32 {
        self.font_size.unwrap_or(DEFAULT_SIZE)
    }

    /// Sidans storlek i pt (A4 om inget anges).
    pub fn page(&self) -> (f32, f32) {
        self.page_size.unwrap_or(A4)
    }

    /// Marginaler i pt: topp, höger, botten, vänster (en tum om inget anges).
    pub fn margins(&self) -> [f32; 4] {
        self.page_margin.unwrap_or([DEFAULT_MARGIN; 4])
    }

    /// Raden som skrivs först i texten, utan radslut. Tom sträng om inget avviker från standard.
    pub fn to_comment(&self) -> String {
        let mut parts = Vec::new();
        if let Some(f) = self.font_family.as_deref().filter(|f| !f.eq_ignore_ascii_case(DEFAULT_FONT)) {
            parts.push(format!("font-family: {}", css_font_name(f)));
        }
        if let Some(s) = self.font_size.filter(|s| (s - DEFAULT_SIZE).abs() > 0.01) {
            parts.push(format!("font-size: {}pt", fmt_pt(s)));
        }
        if let Some((w, h)) = self.page_size.filter(|&p| !same_size(p, A4)) {
            parts.push(format!("page-size: {}", page_size_css(w, h)));
        }
        if let Some(m) = self.page_margin.filter(|m| m.iter().any(|v| (v - DEFAULT_MARGIN).abs() > 0.5)) {
            parts.push(format!("page-margin: {}", margin_css(m)));
        }
        for (name, text, align) in [
            ("header", &self.header, self.header_align),
            ("footer", &self.footer, self.footer_align),
        ] {
            if let Some(t) = text.as_deref().filter(|t| !t.is_empty()) {
                parts.push(format!("{name}: {}", quote(t)));
                if align != Align::Left {
                    parts.push(format!("{name}-align: {}", align.as_attr()));
                }
            }
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("<!-- kladd: {} -->", parts.join("; "))
    }

    /// Läser en rad som `<!-- kladd: font-family: "Times New Roman"; font-size: 12pt -->`.
    pub fn from_comment(line: &str) -> Option<DocDefaults> {
        let t = line.trim();
        let inner = t.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
        let css = inner.strip_prefix("kladd:")?;
        let mut d = DocDefaults::default();
        for (k, v) in declarations(css) {
            match k.as_str() {
                "font-family" => d.font_family = split_font_list(&v).into_iter().next().filter(|f| !f.is_empty()),
                "font-size" => d.font_size = parse_size(&v),
                "page-size" => d.page_size = parse_page_size(&v),
                "page-margin" => d.page_margin = parse_margins(&v),
                "header" => d.header = Some(unquote(&v)),
                "footer" => d.footer = Some(unquote(&v)),
                "header-align" => d.header_align = Align::from_attr(&v),
                "footer-align" => d.footer_align = Align::from_attr(&v),
                _ => {}
            }
        }
        Some(d)
    }
}

/// Text i sidhuvud eller sidfot med fälten ifyllda.
pub fn fill_fields(template: &str, page: usize, pages: usize) -> String {
    template.replace("{sida}", &page.to_string()).replace("{sidor}", &pages.to_string())
}

/// `namn: värde`-par separerade med `;`, där värden inom citattecken får innehålla `;`.
fn declarations(css: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in css.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            None if c == ';' => out.push(std::mem::take(&mut cur)),
            None => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter()
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect()
}

fn quote(t: &str) -> String {
    let mut s = String::from('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push(' '),
            // "--" skulle avsluta kommentaren i HTML.
            '-' if s.ends_with('-') => s.push_str("\\-"),
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    let Some(inner) = v.strip_prefix('"').and_then(|r| r.strip_suffix('"')) else {
        return v.to_owned();
    };
    let mut out = String::new();
    let mut esc = false;
    for c in inner.chars() {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// En längd som `25mm`, `2.5cm`, `1in`, `72pt` eller `96px`, i pt.
pub fn parse_length(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let (num, f) = if let Some(n) = v.strip_suffix("mm") {
        (n, 72.0 / 25.4)
    } else if let Some(n) = v.strip_suffix("cm") {
        (n, 72.0 / 2.54)
    } else if let Some(n) = v.strip_suffix("in") {
        (n, 72.0)
    } else if let Some(n) = v.strip_suffix("pt") {
        (n, 1.0)
    } else if let Some(n) = v.strip_suffix("px") {
        (n, 0.75)
    } else {
        (v.as_str(), 1.0)
    };
    let x: f32 = num.trim().parse().ok()?;
    (x >= 0.0 && x < 100_000.0).then_some(x * f)
}

fn parse_page_size(v: &str) -> Option<(f32, f32)> {
    let words: Vec<String> = v.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
    let landscape = words.iter().any(|w| w == "landscape");
    let named = words.iter().find_map(|w| {
        Some(match w.as_str() {
            "a3" => (841.89, 1190.55),
            "a4" => A4,
            "a5" => (419.53, 595.28),
            "letter" => (612.0, 792.0),
            "legal" => (612.0, 1008.0),
            _ => return None,
        })
    });
    let (w, h) = match named {
        Some(p) => p,
        None => {
            let lens: Vec<f32> = words.iter().filter_map(|w| parse_length(w)).collect();
            if lens.len() != 2 || lens[0] <= 0.0 || lens[1] <= 0.0 {
                return None;
            }
            (lens[0], lens[1])
        }
    };
    Some(if landscape && w < h { (h, w) } else { (w, h) })
}

fn parse_margins(v: &str) -> Option<[f32; 4]> {
    let l: Vec<f32> = v.split_whitespace().map(parse_length).collect::<Option<_>>()?;
    Some(match l.as_slice() {
        [a] => [*a; 4],
        [tb, rl] => [*tb, *rl, *tb, *rl],
        [t, rl, b] => [*t, *rl, *b, *rl],
        [t, r, b, l] => [*t, *r, *b, *l],
        _ => return None,
    })
}

fn same_size(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0
}

fn page_size_css(w: f32, h: f32) -> String {
    for (name, p) in [("A3", (841.89, 1190.55)), ("A4", A4), ("A5", (419.53, 595.28)), ("Letter", (612.0, 792.0)), ("Legal", (612.0, 1008.0))] {
        if same_size((w, h), p) {
            return name.to_owned();
        }
        if same_size((h, w), p) {
            return format!("{name} landscape");
        }
    }
    format!("{} {}", mm(w), mm(h))
}

fn margin_css(m: [f32; 4]) -> String {
    if m.iter().all(|v| (v - m[0]).abs() < 0.5) {
        mm(m[0])
    } else {
        m.iter().map(|v| mm(*v)).collect::<Vec<_>>().join(" ")
    }
}

/// pt som mm med högst en decimal.
fn mm(pt: f32) -> String {
    let v = (pt * 25.4 / 72.0 * 10.0).round() / 10.0;
    if v.fract() == 0.0 { format!("{}mm", v as i64) } else { format!("{v}mm") }
}

/// Words markeringsfärger (`w:highlight`) som RGB.
pub fn highlight_rgb(name: &str) -> Option<[u8; 3]> {
    Some(match name {
        "yellow" => [0xFF, 0xFF, 0x00],
        "green" => [0x00, 0xFF, 0x00],
        "cyan" => [0x00, 0xFF, 0xFF],
        "magenta" => [0xFF, 0x00, 0xFF],
        "blue" => [0x00, 0x00, 0xFF],
        "red" => [0xFF, 0x00, 0x00],
        "darkBlue" => [0x00, 0x00, 0x80],
        "darkCyan" => [0x00, 0x80, 0x80],
        "darkGreen" => [0x00, 0x80, 0x00],
        "darkMagenta" => [0x80, 0x00, 0x80],
        "darkRed" => [0x80, 0x00, 0x00],
        "darkYellow" => [0x80, 0x80, 0x00],
        "darkGray" => [0x80, 0x80, 0x80],
        "lightGray" => [0xC0, 0xC0, 0xC0],
        "black" => [0x00, 0x00, 0x00],
        "white" => [0xFF, 0xFF, 0xFF],
        _ => return None,
    })
}

/// Omvändningen av [`highlight_rgb`]: namnet om färgen är exakt en av Words markeringsfärger.
pub fn highlight_name(rgb: [u8; 3]) -> Option<&'static str> {
    const NAMES: [&str; 16] = [
        "yellow", "green", "cyan", "magenta", "blue", "red", "darkBlue", "darkCyan", "darkGreen", "darkMagenta",
        "darkRed", "darkYellow", "darkGray", "lightGray", "black", "white",
    ];
    NAMES.into_iter().find(|n| highlight_rgb(n) == Some(rgb))
}

pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// `#RGB`, `#RRGGBB`, `RRGGBB` (Words format), `rgb(r, g, b)` eller några färgnamn.
pub fn parse_color(v: &str) -> Option<[u8; 3]> {
    let v = v.trim().trim_end_matches("!important").trim();
    let lower = v.to_ascii_lowercase();
    if let Some(inner) = lower.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let n: Vec<u8> = inner.split(',').filter_map(|p| p.trim().parse::<u8>().ok()).collect();
        return (n.len() == 3).then(|| [n[0], n[1], n[2]]);
    }
    let h = lower.strip_prefix('#').unwrap_or(&lower);
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        return Some([b(0)?, b(2)?, b(4)?]);
    }
    if lower.starts_with('#') && h.len() == 3 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        let b = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|x| x * 17);
        return Some([b(0)?, b(1)?, b(2)?]);
    }
    Some(match lower.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "gray" | "grey" => [128, 128, 128],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        _ => return None,
    })
}

/// `14pt`, `14.5pt`, `20px` (omräknas till pt), `14` (pt).
pub fn parse_size(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let (num, factor) = if let Some(n) = v.strip_suffix("pt") {
        (n, 1.0)
    } else if let Some(n) = v.strip_suffix("px") {
        (n, 0.75)
    } else {
        (v.as_str(), 1.0)
    };
    let s: f32 = num.trim().parse().ok()?;
    let s = s * factor;
    (s > 0.0 && s < 1000.0).then_some(s)
}

fn fmt_pt(s: f32) -> String {
    let r = (s * 2.0).round() / 2.0;
    if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
}

/// Typsnittsnamn med citattecken om det innehåller mellanslag.
fn css_font_name(f: &str) -> String {
    if f.contains(char::is_whitespace) { format!("\"{}\"", f.replace('"', "")) } else { f.to_owned() }
}

/// `"Times New Roman", serif` → `["Times New Roman", "serif"]`.
fn split_font_list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|p| p.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Värdet av attributet `name` i en tagg, med eller utan citattecken.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at > 0 && lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let off = tag.len() - tag[from..].trim_start().len() + 1; // efter '='
        let val = tag[off..].trim_start();
        let (q, body) = match val.chars().next() {
            Some(q @ ('"' | '\'')) => (Some(q), &val[1..]),
            _ => (None, val),
        };
        let end = match q {
            Some(q) => body.find(q)?,
            None => body.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(body.len()),
        };
        return Some(body[..end].to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_round_trip() {
        let s = SpanStyle {
            font_family: Some("Times New Roman".into()),
            font_size: Some(14.0),
            color: Some([0xC0, 0, 0]),
            background: Some([0xFF, 0xFF, 0]),
        };
        let tag = s.open_tag();
        assert_eq!(
            tag,
            "<span style=\"font-family: 'Times New Roman'; font-size: 14pt; color: #C00000; background-color: #FFFF00\">"
        );
        assert_eq!(parse_span_open(&tag), Some(s));
        assert!(is_span_close("</SPAN>"));
        assert_eq!(parse_span_open("<spanner>"), None);
        assert_eq!(parse_span_open("<span>"), Some(SpanStyle::default()));
    }

    #[test]
    fn css_variants() {
        let s = SpanStyle::from_css("color: rgb(255, 0, 0); font-size: 16px; font-family: Arial, sans-serif; foo: bar");
        assert_eq!(s.color, Some([255, 0, 0]));
        assert_eq!(s.font_size, Some(12.0));
        assert_eq!(s.font_family.as_deref(), Some("Arial"));
        assert_eq!(parse_color("#abc"), Some([0xAA, 0xBB, 0xCC]));
        assert_eq!(parse_color("2F5496"), Some([0x2F, 0x54, 0x96]));
        assert_eq!(parse_size("10.5pt"), Some(10.5));
        assert_eq!(fmt_pt(10.5), "10.5");
    }

    #[test]
    fn merged_inner_wins() {
        let outer = SpanStyle { color: Some([1, 2, 3]), font_size: Some(12.0), ..Default::default() };
        let inner = SpanStyle { color: Some([9, 9, 9]), ..Default::default() };
        let m = outer.merged(&inner);
        assert_eq!(m.color, Some([9, 9, 9]));
        assert_eq!(m.font_size, Some(12.0));
    }

    #[test]
    fn div_align() {
        assert_eq!(parse_div_open("<div align=\"center\">"), Some(Align::Center));
        assert_eq!(parse_div_open("<div align=right>"), Some(Align::Right));
        assert_eq!(parse_div_open("<div style=\"text-align: justify\">"), Some(Align::Justify));
        assert_eq!(parse_div_open("<div>"), Some(Align::Left));
        assert_eq!(parse_div_open("<divider>"), None);
        assert_eq!(div_open(Align::Justify), "<div align=\"justify\">");
        assert!(is_div_close(" </div> "));
        assert_eq!(Align::from_jc("both"), Align::Justify);
        assert_eq!(Align::Center.as_jc(), "center");
    }

    #[test]
    fn defaults_comment() {
        let d = DocDefaults { font_family: Some("Times New Roman".into()), font_size: Some(12.0), ..Default::default() };
        let c = d.to_comment();
        assert_eq!(c, "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt -->");
        assert_eq!(DocDefaults::from_comment(&c), Some(d));
        assert_eq!(DocDefaults { font_family: Some("Calibri".into()), font_size: Some(11.0), ..Default::default() }.to_comment(), "");
        assert_eq!(DocDefaults::from_comment("<!-- något annat -->"), None);
    }

    #[test]
    fn page_setup_comment() {
        let d = DocDefaults {
            page_size: Some((612.0, 792.0)),
            page_margin: Some([72.0, 56.69, 72.0, 56.69]),
            header: Some("Rapport; \"utkast\" -- 2026".into()),
            header_align: Align::Right,
            footer: Some("Sida {sida} av {sidor}".into()),
            footer_align: Align::Center,
            ..Default::default()
        };
        let c = d.to_comment();
        assert!(c.contains("page-size: Letter"), "{c}");
        assert!(c.contains("page-margin: 25.4mm 20mm 25.4mm 20mm"), "{c}");
        assert!(c.contains("footer-align: center"), "{c}");
        assert!(!c[4..c.len() - 3].contains("--"), "kommentaren får inte innehålla --: {c}");
        let back = DocDefaults::from_comment(&c).unwrap();
        assert_eq!(back.header.as_deref(), Some("Rapport; \"utkast\" -- 2026"));
        assert_eq!(back.footer, d.footer);
        assert_eq!(back.header_align, Align::Right);
        assert_eq!(back.page_size, Some((612.0, 792.0)));
        let m = back.page_margin.unwrap();
        assert!((m[1] - 56.69).abs() < 0.2 && (m[0] - 72.0).abs() < 0.2);
        assert_eq!(fill_fields("Sida {sida} av {sidor}", 2, 5), "Sida 2 av 5");
    }

    #[test]
    fn page_sizes() {
        assert_eq!(parse_page_size("A4 landscape"), Some((841.89, 595.28)));
        assert_eq!(parse_page_size("210mm 297mm").map(|(w, h)| (w.round(), h.round())), Some((595.0, 842.0)));
        assert_eq!(parse_margins("1in 2cm"), Some([72.0, 72.0 / 2.54 * 2.0, 72.0, 72.0 / 2.54 * 2.0]));
        assert_eq!(DocDefaults { page_size: Some(A4), page_margin: Some([72.0; 4]), ..Default::default() }.to_comment(), "");
        assert_eq!(DocDefaults::default().page(), A4);
    }

    #[test]
    fn highlight_names() {
        assert_eq!(highlight_rgb("yellow"), Some([255, 255, 0]));
        assert_eq!(highlight_name([255, 255, 0]), Some("yellow"));
        assert_eq!(highlight_name([1, 2, 3]), None);
    }
}
