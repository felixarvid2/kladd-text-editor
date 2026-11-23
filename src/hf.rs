//! Sidhuvud och sidfot som i Word: mallens bildmärken och sidnummerformat, varianter per avsnitt
//! (annorlunda första sida, udda/jämna sidor, arv från föregående avsnitt), sidnumrering som
//! börjar om och uppställningen av en rad (vänster, mitten, höger, med bilder) som både
//! dokumentläget och PDF-exporten ritar efter.
//!
//! Mallens text (se `style::DocDefaults`) har fälten `{sida}`, `{sidor}`, `{datum}`, `{titel}`
//! och `{fil}`. Här läggs till:
//! - `{sida:roman}`, `{sida:ROMAN}`, `{sida:abc}`, `{sida:ABC}`, `{sida:1}`: sidnumret i annat
//!   format (Words `\* roman`, `\* ROMAN`, `\* alphabetic`, `\* ALPHABETIC`, `\* Arabic`); samma för `{sidor}`.
//! - `{avsnittssidor}`: antalet sidor i avsnittet (SECTIONPAGES).
//! - `{bild:namn 80x30}`: en bild på raden (bredd x höjd i pt), `{bild:namn 80x30 @12,20}` en förankrad
//!   bild vars övre vänstra hörn ligger 12 pt från sidans vänsterkant och 20 pt från överkanten.
//!   `namn` är en av dokumentets bilder (`bild:namn`); mellanslag i namnet skrivs `%20`.
//! - Inline-formatering per körning (`hf_inline.rs`): `**fet**`, `*kursiv*`, `<u>understruken</u>` och
//!   `<span style="color: #C00000; font-size: 14pt">…</span>`, som i brödtexten. `header-style` är
//!   standardstilen för hela sidhuvudet; en körning lägger bara till det som avviker.

pub use crate::hf_inline::RunFmt;
use crate::style::{Align, DocDefaults, Fields};
use crate::word::sections::Section;

/// Sidnumrets format (`w:pgNumType w:fmt`, `\* roman` …).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NumFmt {
    #[default]
    Decimal,
    LowerRoman,
    UpperRoman,
    LowerAlpha,
    UpperAlpha,
}

impl NumFmt {
    /// Siffran som text. Romerska tal över 3999 och nollan skrivs som vanliga siffror.
    pub fn format(self, n: usize) -> String {
        match self {
            NumFmt::Decimal => n.to_string(),
            NumFmt::LowerRoman => roman(n).map_or_else(|| n.to_string(), |r| r.to_lowercase()),
            NumFmt::UpperRoman => roman(n).unwrap_or_else(|| n.to_string()),
            NumFmt::LowerAlpha => alpha(n, false),
            NumFmt::UpperAlpha => alpha(n, true),
        }
    }

    /// Namnet i mallen (`{sida:roman}`) och på raden (`page-format: roman`).
    pub fn name(self) -> &'static str {
        match self {
            NumFmt::Decimal => "1",
            NumFmt::LowerRoman => "roman",
            NumFmt::UpperRoman => "ROMAN",
            NumFmt::LowerAlpha => "abc",
            NumFmt::UpperAlpha => "ABC",
        }
    }

    /// `style:num-format` i ODF.
    pub fn odf(self) -> &'static str {
        match self {
            NumFmt::Decimal => "1",
            NumFmt::LowerRoman => "i",
            NumFmt::UpperRoman => "I",
            NumFmt::LowerAlpha => "a",
            NumFmt::UpperAlpha => "A",
        }
    }

    pub fn from_name(s: &str) -> Option<NumFmt> {
        Some(match s.trim() {
            "1" | "123" | "arabic" | "Arabic" | "decimal" => NumFmt::Decimal,
            "roman" | "i" => NumFmt::LowerRoman,
            "ROMAN" | "I" => NumFmt::UpperRoman,
            "abc" | "a" | "alphabetic" => NumFmt::LowerAlpha,
            "ABC" | "A" | "ALPHABETIC" => NumFmt::UpperAlpha,
            _ => return None,
        })
    }

    /// `w:fmt` i `w:pgNumType`.
    pub fn from_docx(v: &str) -> Option<NumFmt> {
        Some(match v {
            "decimal" => NumFmt::Decimal,
            "lowerRoman" => NumFmt::LowerRoman,
            "upperRoman" => NumFmt::UpperRoman,
            "lowerLetter" => NumFmt::LowerAlpha,
            "upperLetter" => NumFmt::UpperAlpha,
            _ => return None,
        })
    }

    pub fn docx_val(self) -> &'static str {
        match self {
            NumFmt::Decimal => "decimal",
            NumFmt::LowerRoman => "lowerRoman",
            NumFmt::UpperRoman => "upperRoman",
            NumFmt::LowerAlpha => "lowerLetter",
            NumFmt::UpperAlpha => "upperLetter",
        }
    }

    /// Fältomkopplaren efter fältkoden (`PAGE \* roman`).
    pub fn switch(self) -> Option<&'static str> {
        match self {
            NumFmt::Decimal => None,
            NumFmt::LowerRoman => Some("\\* roman"),
            NumFmt::UpperRoman => Some("\\* ROMAN"),
            NumFmt::LowerAlpha => Some("\\* alphabetic"),
            NumFmt::UpperAlpha => Some("\\* ALPHABETIC"),
        }
    }

    /// Formatet i en fältkod (`PAGE \* ROMAN` → versaler); `None` om koden inte väljer något.
    pub fn from_switch(instr: &str) -> Option<NumFmt> {
        let mut it = instr.split_whitespace();
        while let Some(w) = it.next() {
            if w == "\\*" {
                return match it.next()? {
                    "roman" => Some(NumFmt::LowerRoman),
                    "ROMAN" => Some(NumFmt::UpperRoman),
                    "alphabetic" => Some(NumFmt::LowerAlpha),
                    "ALPHABETIC" => Some(NumFmt::UpperAlpha),
                    "Arabic" | "arabic" => Some(NumFmt::Decimal),
                    _ => None,
                };
            }
        }
        None
    }
}

fn roman(mut n: usize) -> Option<String> {
    if n == 0 || n > 3999 {
        return None;
    }
    let mut s = String::new();
    for (v, r) in [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")] {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    Some(s)
}

/// a..z, därefter aa, bb, cc … (som Word).
fn alpha(n: usize, upper: bool) -> String {
    if n == 0 {
        return "0".to_owned();
    }
    let c = (b'a' + ((n - 1) % 26) as u8) as char;
    let c = if upper { c.to_ascii_uppercase() } else { c };
    std::iter::repeat_n(c, (n - 1) / 26 + 1).collect()
}

/// Utseendet på sidhuvudets eller sidfotens text: storlek, färg och en linje mot brödtexten
/// (`w:pBdr`). Står som `header-style: "9pt #666666 bold line=#999999/0.5pt"`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HfStyle {
    pub size: Option<f32>,
    pub color: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Linje under sidhuvudet (över sidfoten): färg och tjocklek i pt.
    pub line: Option<([u8; 3], f32)>,
}

impl HfStyle {
    pub fn is_default(&self) -> bool {
        *self == HfStyle::default()
    }

    pub fn to_text(&self) -> String {
        let mut p: Vec<String> = Vec::new();
        if let Some(s) = self.size {
            p.push(format!("{}pt", trim_num(s)));
        }
        if let Some(c) = self.color {
            p.push(hex(c));
        }
        if self.bold {
            p.push("bold".into());
        }
        if self.italic {
            p.push("italic".into());
        }
        if self.underline {
            p.push("underline".into());
        }
        if let Some((c, w)) = self.line {
            p.push(format!("line={}/{}pt", hex(c), trim_num(w)));
        }
        p.join(" ")
    }

    pub fn from_text(v: &str) -> HfStyle {
        let mut s = HfStyle::default();
        for w in v.split_whitespace() {
            if let Some(h) = w.strip_prefix('#') {
                s.color = parse_hex(h);
            } else if w == "bold" {
                s.bold = true;
            } else if w == "italic" {
                s.italic = true;
            } else if w == "underline" {
                s.underline = true;
            } else if let Some(l) = w.strip_prefix("line=") {
                let (c, wd) = l.split_once('/').unwrap_or((l, "0.5pt"));
                if let Some(c) = parse_hex(c.trim_start_matches('#')) {
                    s.line = Some((c, crate::style::parse_size(wd).unwrap_or(0.5)));
                }
            } else if let Some(sz) = crate::style::parse_size(w) {
                s.size = Some(sz);
            }
        }
        s
    }
}

fn trim_num(v: f32) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
}

pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

pub fn parse_hex(h: &str) -> Option<[u8; 3]> {
    let h = h.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

/// Sidhuvud och sidfot som ett avsnitt själv anger (på avsnittsraden). `None` = ärv från
/// föregående avsnitt (som i Word när en referens saknas); `Some("")` = ingen text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SecHf {
    pub header: Option<String>,
    pub footer: Option<String>,
    pub header_first: Option<String>,
    pub footer_first: Option<String>,
    pub header_even: Option<String>,
    pub footer_even: Option<String>,
    /// Annorlunda första sida (`w:titlePg`); `None` = som standard (av för senare avsnitt).
    pub title_pg: Option<bool>,
    /// Sidnumret börjar om på detta nummer (`w:pgNumType w:start`).
    pub page_start: Option<u32>,
    pub page_fmt: Option<NumFmt>,
}

impl SecHf {
    /// `namn: värde`-delarna som skrivs på avsnittsraden.
    pub fn to_parts(&self) -> Vec<String> {
        let mut v = Vec::new();
        for (k, t) in [
            ("header", &self.header),
            ("footer", &self.footer),
            ("header-first", &self.header_first),
            ("footer-first", &self.footer_first),
            ("header-even", &self.header_even),
            ("footer-even", &self.footer_even),
        ] {
            if let Some(t) = t {
                v.push(format!("{k}: {}", crate::style::quote(t)));
            }
        }
        if let Some(b) = self.title_pg {
            v.push(format!("title-page: {}", if b { "yes" } else { "no" }));
        }
        if let Some(s) = self.page_start {
            v.push(format!("page-start: {s}"));
        }
        if let Some(f) = self.page_fmt {
            v.push(format!("page-format: {}", f.name()));
        }
        v
    }

    /// Läser en deklaration; sant om nyckeln var en av dessa.
    pub fn read_decl(&mut self, k: &str, v: &str) -> bool {
        let text = || Some(crate::style::unquote(v));
        match k {
            "header" => self.header = text(),
            "footer" => self.footer = text(),
            "header-first" => self.header_first = text(),
            "footer-first" => self.footer_first = text(),
            "header-even" => self.header_even = text(),
            "footer-even" => self.footer_even = text(),
            "title-page" => self.title_pg = Some(matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "on" | "1")),
            "page-start" => self.page_start = v.trim().parse().ok(),
            "page-format" => self.page_fmt = NumFmt::from_name(v),
            _ => return false,
        }
        true
    }
}

/// Ett avsnitts sidhuvuden och sidfötter med arvet utrett.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolved {
    pub header: Option<String>,
    pub footer: Option<String>,
    pub header_first: Option<String>,
    pub footer_first: Option<String>,
    pub header_even: Option<String>,
    pub footer_even: Option<String>,
    pub title_pg: bool,
    pub start: Option<u32>,
    pub fmt: NumFmt,
}

/// Sidhuvud och sidfot för hela dokumentet: ett `Resolved` per avsnitt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub secs: Vec<Resolved>,
    /// Udda och jämna sidor har olika sidhuvud/sidfot (`w:evenAndOddHeaders`).
    pub even_odd: bool,
    pub header_align: Align,
    pub footer_align: Align,
}

/// Vilka sidor som hör till vilket avsnitt och deras sidnummer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PagePlan {
    /// Visat sidnummer per ark.
    pub numbers: Vec<usize>,
    /// Avsnittet per ark.
    pub sec: Vec<usize>,
    /// Första arket i sitt avsnitt?
    pub first: Vec<bool>,
    /// Antal ark i avsnittet, per ark.
    pub sec_pages: Vec<usize>,
}

/// Det som ska stå på ett ark.
#[derive(Clone, Debug, PartialEq)]
pub struct PageHf<'a> {
    pub header: Option<&'a str>,
    pub footer: Option<&'a str>,
    pub header_align: Align,
    pub footer_align: Align,
    /// Fält med sidnummer, antal sidor och formatet för det här arket.
    pub fields: Fields,
}

impl Plan {
    /// `secs` är dokumentets avsnitt (tomt för ett dokument utan avsnittsrader).
    pub fn new(d: &DocDefaults, secs: &[Section]) -> Plan {
        let mut out: Vec<Resolved> = Vec::new();
        let n = secs.len().max(1);
        for i in 0..n {
            let own = secs.get(i).map(|s| &s.props.hf);
            let mut r = match out.last() {
                Some(prev) => Resolved { title_pg: false, start: None, fmt: NumFmt::Decimal, ..prev.clone() },
                None => Resolved {
                    header: d.header.clone(),
                    footer: d.footer.clone(),
                    header_first: d.header_first.clone(),
                    footer_first: d.footer_first.clone(),
                    header_even: d.header_even.clone(),
                    footer_even: d.footer_even.clone(),
                    title_pg: d.different_first(),
                    start: d.page_start,
                    fmt: d.page_format.unwrap_or_default(),
                },
            };
            if let Some(o) = own {
                if o.header.is_some() {
                    r.header = o.header.clone();
                }
                if o.footer.is_some() {
                    r.footer = o.footer.clone();
                }
                if o.header_first.is_some() {
                    r.header_first = o.header_first.clone();
                }
                if o.footer_first.is_some() {
                    r.footer_first = o.footer_first.clone();
                }
                if o.header_even.is_some() {
                    r.header_even = o.header_even.clone();
                }
                if o.footer_even.is_some() {
                    r.footer_even = o.footer_even.clone();
                }
                if let Some(t) = o.title_pg {
                    r.title_pg = t;
                }
                if o.page_start.is_some() {
                    r.start = o.page_start;
                }
                if let Some(f) = o.page_fmt {
                    r.fmt = f;
                }
            }
            out.push(r);
        }
        Plan {
            secs: out,
            even_odd: d.header_even.is_some() || d.footer_even.is_some(),
            header_align: d.header_align,
            footer_align: d.footer_align,
        }
    }

    /// Minsta marginal (pt) uppifrån och nedifrån för avsnitt `i`: sidhuvudet/sidfoten och dess
    /// avstånd från kanten får inte täcka brödtexten (Word skjuter ned texten). Räknas på den
    /// högsta varianten (vanlig, första sida, jämna sidor).
    pub fn reserve(&self, d: &DocDefaults, i: usize) -> [f32; 2] {
        let Some(r) = self.secs.get(i.min(self.secs.len().saturating_sub(1))) else { return [0.0; 2] };
        let height = |tpls: [&Option<String>; 3], st: &HfStyle, dist: Option<f32>| {
            let mut h: f32 = 0.0;
            for t in tpls.into_iter().flatten().filter(|t| !t.is_empty()) {
                let size = st.size.unwrap_or(d.size() * 0.9);
                // Varje rad (stycke) är minst en textrad hög, och högre med en bild på raden.
                let line: f32 = rows(t)
                    .into_iter()
                    .map(|row| {
                        // Större tecken i en körning (`font-size`) gör raden högre.
                        let mut line = size.max(crate::hf_inline::max_size(row).unwrap_or(0.0)) * LINE_FACTOR;
                        for p in pieces(row) {
                            if let Piece::Image(tok) = p {
                                if tok.at.is_none() {
                                    line = line.max(tok.h);
                                }
                            }
                        }
                        line
                    })
                    .sum();
                h = h.max(line);
            }
            if h == 0.0 {
                return 0.0;
            }
            // Linjen mot texten tar några punkter till (och lite luft under den).
            dist.unwrap_or(DEFAULT_DIST) + h + st.line.map_or(0.0, |(_, w)| 6.0 + w)
        };
        let none = None;
        let first_h = if r.title_pg { &r.header_first } else { &none };
        let even_h = if self.even_odd { &r.header_even } else { &none };
        let first_f = if r.title_pg { &r.footer_first } else { &none };
        let even_f = if self.even_odd { &r.footer_even } else { &none };
        [
            height([&r.header, first_h, even_h], &d.header_style, d.header_dist),
            height([&r.footer, first_f, even_f], &d.footer_style, d.footer_dist),
        ]
    }

    /// Sidnummer och avsnitt för arken; `page_secs` är avsnittet varje ark börjar i.
    pub fn pages(&self, page_secs: &[usize]) -> PagePlan {
        let n = page_secs.len();
        let sec: Vec<usize> = page_secs.iter().map(|s| (*s).min(self.secs.len().saturating_sub(1))).collect();
        let mut numbers = Vec::with_capacity(n);
        let mut first = Vec::with_capacity(n);
        for k in 0..n {
            let is_first = k == 0 || sec[k] != sec[k - 1];
            let start = self.secs.get(sec[k]).and_then(|r| r.start);
            let num = match (is_first, start) {
                (true, Some(s)) => s as usize,
                _ if k == 0 => 1,
                _ => numbers[k - 1] + 1,
            };
            numbers.push(num);
            first.push(is_first);
        }
        let mut sec_pages = vec![0; n];
        let mut k = 0;
        while k < n {
            let mut e = k + 1;
            while e < n && sec[e] == sec[k] {
                e += 1;
            }
            // Ett avsnitt som återkommer (efter ett annat) räknas för sig, som SECTIONPAGES.
            for s in &mut sec_pages[k..e] {
                *s = e - k;
            }
            k = e;
        }
        PagePlan { numbers, sec, first, sec_pages }
    }

    /// Sidhuvud och sidfot för ark `k` (0-baserat). `base` har titel, filnamn, datum och antal ark.
    pub fn page<'a>(&'a self, pp: &PagePlan, k: usize, base: &Fields) -> PageHf<'a> {
        let s = pp.sec.get(k).and_then(|i| self.secs.get(*i)).or(self.secs.first());
        let number = pp.numbers.get(k).copied().unwrap_or(k + 1);
        let first = pp.first.get(k).copied().unwrap_or(k == 0);
        let (h, f) = match s {
            Some(s) if s.title_pg && first => (s.header_first.as_deref(), s.footer_first.as_deref()),
            Some(s) if self.even_odd && number % 2 == 0 => (s.header_even.as_deref(), s.footer_even.as_deref()),
            Some(s) => (s.header.as_deref(), s.footer.as_deref()),
            None => (None, None),
        };
        let mut fields = base.at(number, base.pages);
        fields.sec_pages = pp.sec_pages.get(k).copied().unwrap_or(base.pages);
        fields.fmt = s.map_or_else(NumFmt::default, |s| s.fmt);
        PageHf {
            header: h.filter(|t| !t.is_empty()),
            footer: f.filter(|t| !t.is_empty()),
            header_align: self.header_align,
            footer_align: self.footer_align,
            fields,
        }
    }
}

// ---- bildmärken

/// En bild på raden.
#[derive(Clone, Debug, PartialEq)]
pub struct ImgTok {
    /// Källan som `doc::resolve_image` förstår (`bild:namn`).
    pub src: String,
    /// Storlek i pt (0 = bildens egen).
    pub w: f32,
    pub h: f32,
    /// Förankrad: övre vänstra hörnet från sidans övre vänstra hörn (pt).
    pub at: Option<(f32, f32)>,
}

impl ImgTok {
    pub fn to_token(&self) -> String {
        let name = self.src.strip_prefix(crate::docx::IMAGE_SCHEME).unwrap_or(&self.src).replace(' ', "%20");
        let mut t = format!("{{bild:{name} {}x{}", trim_num(self.w), trim_num(self.h));
        if let Some((x, y)) = self.at {
            t.push_str(&format!(" @{},{}", trim_num(x), trim_num(y)));
        }
        t.push('}');
        t
    }

    /// Läser innehållet mellan klamrarna (`bild:logo.png 80x30 @12,20`).
    pub fn parse(inner: &str) -> Option<ImgTok> {
        let rest = inner.strip_prefix("bild:")?;
        let mut it = rest.split_whitespace();
        let name = it.next()?;
        let (mut w, mut h, mut at) = (0.0, 0.0, None);
        for w_ in it {
            if let Some(p) = w_.strip_prefix('@') {
                let (x, y) = p.split_once(',')?;
                at = Some((x.parse().ok()?, y.parse().ok()?));
            } else if let Some((a, b)) = w_.split_once('x') {
                w = a.parse().ok()?;
                h = b.parse().ok()?;
            }
        }
        Some(ImgTok { src: format!("{}{name}", crate::docx::IMAGE_SCHEME), w, h, at })
    }
}

/// Ett stycke av en rad: text eller en bild.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Image(ImgTok),
}

/// Delar en text (med fälten ifyllda) i text och bilder.
pub fn pieces(s: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("{bild:") {
        let Some(j) = rest[i..].find('}') else { break };
        match ImgTok::parse(&rest[i + 1..i + j]) {
            Some(tok) => {
                if i > 0 {
                    out.push(Piece::Text(rest[..i].to_owned()));
                }
                out.push(Piece::Image(tok));
            }
            None => out.push(Piece::Text(rest[..i + j + 1].to_owned())),
        }
        rest = &rest[i + j + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest.to_owned()));
    }
    out
}

/// Bildkällorna i en mall.
pub fn image_srcs(template: &str) -> Vec<String> {
    pieces(template).into_iter().filter_map(|p| if let Piece::Image(t) = p { Some(t.src) } else { None }).collect()
}

/// Mallarna i dokumentets grundformat och avsnitt som har bilder, för att läsa in dem.
pub fn all_image_srcs(d: &DocDefaults, secs: &[Section]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut add = |t: &Option<String>| {
        if let Some(t) = t {
            for s in image_srcs(t) {
                if !v.contains(&s) {
                    v.push(s);
                }
            }
        }
    };
    for t in [&d.header, &d.footer, &d.header_first, &d.footer_first, &d.header_even, &d.footer_even] {
        add(t);
    }
    for s in secs {
        let h = &s.props.hf;
        for t in [&h.header, &h.footer, &h.header_first, &h.footer_first, &h.header_even, &h.footer_even] {
            add(t);
        }
    }
    v
}

/// Texten med fälten ifyllda; bildmärkena lämnas orörda (`pieces` delar dem sedan). `\|` blir `|`.
pub fn fill(template: &str, f: &Fields) -> String {
    let mut out = String::with_capacity(template.len() + 8);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            rest = &rest[i..];
            break;
        };
        let inner = &rest[i + 1..i + j];
        let (name, fmt_s) = match inner.split_once(':') {
            Some((n, f)) => (n, Some(f)),
            None => (inner, None),
        };
        // `{sida:roman}`: ett okänt format lämnar märket orört.
        let fmt = fmt_s.map(NumFmt::from_name);
        let value: Option<String> = match (name, fmt) {
            ("sida", None) => Some(f.fmt.format(f.page)),
            ("sida", Some(Some(fm))) => Some(fm.format(f.page)),
            ("sidor", None) => Some(f.pages.to_string()),
            ("sidor", Some(Some(fm))) => Some(fm.format(f.pages)),
            ("avsnittssidor", None) => Some(f.sec_pages.to_string()),
            ("datum", None) => Some(f.date.clone()),
            ("titel", None) => Some(f.title.clone()),
            ("fil", None) => Some(f.file.clone()),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[i..i + j + 1]),
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out.replace("\\|", "|")
}

// ---- uppställning av en rad

/// Något som ska ritas: koordinaterna i enheten som anroparen valt (pixlar eller pt), från
/// områdets övre vänstra hörn (förankrade bilder från arkets övre vänstra hörn).
#[derive(Clone, Debug, PartialEq)]
pub enum Placed {
    /// `fmt` är körningens format ovanpå `header-style` (färg `None` = sidhuvudets standardfärg).
    Text { x: f32, y: f32, text: String, size: f32, fmt: RunFmt },
    Image { x: f32, y: f32, w: f32, h: f32, src: String, anchored: bool },
}

/// Resultatet av uppställningen av ett sidhuvud eller en sidfot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Zone {
    pub items: Vec<Placed>,
    /// Radens höjd.
    pub height: f32,
}

/// Radhöjden som faktor av teckenstorleken.
pub const LINE_FACTOR: f32 = 1.2;

/// Ett stycke av en rad med sitt format (ovanpå `header-style`).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub piece: Piece,
    pub fmt: RunFmt,
}

/// En rad: vänster, mitten och höger som körningar.
pub type RowRuns = [Vec<Run>; 3];

/// Ställer upp en rad av `parts` (vänster, mitten, höger; fälten ifyllda, se `filled_rows`). `size_pt` är
/// standardstorleken, `scale` enheter per pt, `width` textbredden i samma enhet, `measure(text, storlek, format)`
/// ger textens bredd och `natural(src)` bildens egna storlek i pt (för bilder utan egen storlek i märket).
pub fn layout_zone(
    parts: &RowRuns,
    size_pt: f32,
    scale: f32,
    width: f32,
    measure: &dyn Fn(&str, f32, &RunFmt) -> f32,
    natural: &dyn Fn(&str) -> Option<(f32, f32)>,
) -> Zone {
    struct Seg {
        piece: Piece,
        fmt: RunFmt,
        size: f32,
        w: f32,
        h: f32,
    }
    let text_h = size_pt * scale * LINE_FACTOR;
    let mut cols: Vec<Vec<Seg>> = Vec::new();
    let mut line_h = text_h;
    let mut anchored: Vec<Placed> = Vec::new();
    for part in parts {
        let mut segs = Vec::new();
        for run in part {
            match &run.piece {
                Piece::Text(t) => {
                    // En körning kan ha egen storlek; raden är så hög som den högsta.
                    let size = run.fmt.size.unwrap_or(size_pt) * scale;
                    let w = measure(t, size, &run.fmt);
                    let h = size * LINE_FACTOR;
                    line_h = line_h.max(h);
                    segs.push(Seg { piece: run.piece.clone(), fmt: run.fmt.clone(), size, w, h });
                }
                Piece::Image(tok) => {
                    let mut tok = tok.clone();
                    if tok.w <= 0.0 || tok.h <= 0.0 {
                        let (nw, nh) = natural(&tok.src).unwrap_or((48.0, 48.0));
                        let k = if tok.w > 0.0 { tok.w / nw.max(1.0) } else if tok.h > 0.0 { tok.h / nh.max(1.0) } else { 1.0 };
                        tok.w = nw * k;
                        tok.h = nh * k;
                    }
                    let (w, h) = (tok.w * scale, tok.h * scale);
                    if let Some((x, y)) = tok.at {
                        anchored.push(Placed::Image { x: x * scale, y: y * scale, w, h, src: tok.src, anchored: true });
                    } else {
                        line_h = line_h.max(h);
                        segs.push(Seg { piece: Piece::Image(tok), fmt: run.fmt.clone(), size: 0.0, w, h });
                    }
                }
            }
        }
        cols.push(segs);
    }
    let mut items = anchored;
    for (i, segs) in cols.iter().enumerate() {
        let total: f32 = segs.iter().map(|s| s.w).sum();
        let mut x = match i {
            1 => ((width - total) / 2.0).max(0.0),
            2 => (width - total).max(0.0),
            _ => 0.0,
        };
        for s in segs {
            match &s.piece {
                Piece::Text(t) => {
                    if !t.trim().is_empty() {
                        // Underkanterna ligger på samma linje (grundlinjerna nästan, som i Word).
                        items.push(Placed::Text { x, y: line_h - s.h, text: t.clone(), size: s.size, fmt: s.fmt.clone() });
                    }
                }
                Piece::Image(tok) => items.push(Placed::Image { x, y: line_h - s.h, w: s.w, h: s.h, src: tok.src.clone(), anchored: false }),
            }
            x += s.w;
        }
    }
    Zone { items, height: line_h }
}

/// Raderna (styckena) i en mall: `\n` skiljer dem åt. Varje rad har egna delar (vänster|mitten|höger).
pub fn rows(template: &str) -> Vec<&str> {
    template.split('\n').collect()
}

/// En del av en rad som körningar: markupen tolkas (`hf_inline::parse_inline`) och fälten fylls i per
/// körning, så att ett fältvärde aldrig blir markup. `st` är sidhuvudets standardstil.
pub fn runs_of(part: &str, f: &Fields, st: &HfStyle) -> Vec<Run> {
    let mut out = Vec::new();
    for (text, fmt) in crate::hf_inline::parse_inline(part) {
        let eff = fmt.over(st);
        for piece in pieces(&fill(&text, f)) {
            out.push(Run { piece, fmt: eff.clone() });
        }
    }
    out
}

/// Mallens rader med fälten ifyllda, var och en som vänster, mitten, höger i körningar. `align` gäller
/// en rad med ett enda värde utan `|`; `st` är standardstilen (`header-style`) som körningarna läggs ovanpå.
pub fn filled_rows(template: &str, align: Align, f: &Fields, st: &HfStyle) -> Vec<RowRuns> {
    rows(template).into_iter().map(|r| crate::style::parts_of(r, align).map(|p| runs_of(&p, f, st))).collect()
}

/// Bildkällorna på raderna.
pub fn row_image_srcs(rows: &[RowRuns]) -> Vec<String> {
    rows.iter()
        .flatten()
        .flatten()
        .filter_map(|r| if let Piece::Image(t) = &r.piece { Some(t.src.clone()) } else { None })
        .collect()
}

/// Ställer upp flera rader under varandra (se `layout_zone`); `height` är summan av radhöjderna.
pub fn layout_rows(
    rows: &[RowRuns],
    size_pt: f32,
    scale: f32,
    width: f32,
    measure: &dyn Fn(&str, f32, &RunFmt) -> f32,
    natural: &dyn Fn(&str) -> Option<(f32, f32)>,
) -> Zone {
    let mut out = Zone::default();
    for r in rows {
        let z = layout_zone(r, size_pt, scale, width, measure, natural);
        for mut it in z.items {
            match &mut it {
                Placed::Text { y, .. } => *y += out.height,
                Placed::Image { y, anchored: false, .. } => *y += out.height,
                Placed::Image { .. } => {}
            }
            out.items.push(it);
        }
        out.height += z.height;
    }
    out
}

/// Standardavstånd från sidkanten till sidhuvudet och sidfoten (Words 1,25 cm = 708 twips).
pub const DEFAULT_DIST: f32 = 35.4;

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(page: usize, pages: usize) -> Fields {
        Fields::new(page, pages, "rapport.docx")
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(NumFmt::LowerRoman.format(4), "iv");
        assert_eq!(NumFmt::UpperRoman.format(1994), "MCMXCIV");
        assert_eq!(NumFmt::UpperRoman.format(0), "0");
        assert_eq!(NumFmt::LowerAlpha.format(1), "a");
        assert_eq!(NumFmt::UpperAlpha.format(27), "AA");
        assert_eq!(NumFmt::UpperAlpha.format(53), "AAA");
        assert_eq!(NumFmt::from_switch("PAGE \\* ROMAN \\* MERGEFORMAT"), Some(NumFmt::UpperRoman));
        assert_eq!(NumFmt::from_switch(" PAGE "), None);
        assert_eq!(NumFmt::from_docx("lowerLetter"), Some(NumFmt::LowerAlpha));
    }

    #[test]
    fn fill_handles_formats_and_keeps_images() {
        let mut f = fields(3, 10);
        f.sec_pages = 4;
        assert_eq!(fill("Sida {sida} av {sidor}", &f), "Sida 3 av 10");
        assert_eq!(fill("{sida:roman}/{sidor:ROMAN}", &f), "iii/X");
        assert_eq!(fill("{avsnittssidor} sidor", &f), "4 sidor");
        f.fmt = NumFmt::UpperRoman;
        assert_eq!(fill("{sida}", &f), "III");
        assert_eq!(fill("{sida:1}", &f), "3");
        assert_eq!(fill("{bild:logo.png 80x30} a\\|b {okänt}", &f), "{bild:logo.png 80x30} a|b {okänt}");
    }

    #[test]
    fn image_tokens_round_trip() {
        let t = ImgTok { src: "bild:my logo.png".into(), w: 80.0, h: 30.5, at: Some((12.0, 20.0)) };
        let s = t.to_token();
        assert_eq!(s, "{bild:my%20logo.png 80x30.5 @12,20}");
        let back = ImgTok::parse(&s[1..s.len() - 1]).unwrap();
        assert_eq!(back.w, 80.0);
        assert_eq!(back.at, Some((12.0, 20.0)));
        let ps = pieces(&format!("A{s}B"));
        assert_eq!(ps.len(), 3);
        assert!(matches!(&ps[1], Piece::Image(i) if i.src == "bild:my%20logo.png"));
        assert_eq!(image_srcs("{bild:a.png 1x1}|x|{bild:b.png 2x2}"), ["bild:a.png", "bild:b.png"]);
        // Trasigt märke blir text.
        assert_eq!(pieces("{bild:}"), [Piece::Text("{bild:}".into())]);
    }

    #[test]
    fn style_text_round_trips() {
        let s = HfStyle { size: Some(9.0), color: Some([0x66, 0x66, 0x66]), bold: true, italic: false, underline: false, line: Some(([0x99, 0x99, 0x99], 0.5)) };
        let t = s.to_text();
        assert_eq!(t, "9pt #666666 bold line=#999999/0.5pt");
        assert_eq!(HfStyle::from_text(&t), s);
        let u = HfStyle { italic: true, underline: true, ..Default::default() };
        assert_eq!(u.to_text(), "italic underline");
        assert_eq!(HfStyle::from_text("italic underline"), u);
        assert!(HfStyle::from_text("").is_default());
    }

    fn sec(hf: SecHf) -> Section {
        let mut s = Section::resolve(&crate::word::sections::SectionProps::default(), &DocDefaults::default());
        s.props.hf = hf;
        s
    }

    #[test]
    fn sections_inherit_headers_and_restart_numbers() {
        let d = DocDefaults { header: Some("Rapport".into()), footer: Some("Sida {sida} av {sidor}".into()), ..Default::default() };
        let secs = [
            sec(SecHf::default()),
            sec(SecHf { header: Some("Bilagor".into()), page_start: Some(1), page_fmt: Some(NumFmt::LowerRoman), ..Default::default() }),
            sec(SecHf::default()),
        ];
        let plan = Plan::new(&d, &secs);
        // Avsnitt 3 ärver avsnitt 2: huvud, men inte sidnumreringens start.
        assert_eq!(plan.secs[2].header.as_deref(), Some("Bilagor"));
        assert_eq!(plan.secs[2].start, None);
        assert_eq!(plan.secs[1].footer.as_deref(), Some("Sida {sida} av {sidor}"));
        let pp = plan.pages(&[0, 0, 1, 1, 1, 2]);
        assert_eq!(pp.numbers, [1, 2, 1, 2, 3, 4]);
        assert_eq!(pp.sec_pages, [2, 2, 3, 3, 3, 1]);
        let base = fields(1, 6);
        let p = plan.page(&pp, 3, &base);
        assert_eq!(p.header, Some("Bilagor"));
        assert_eq!(fill(p.footer.unwrap(), &p.fields), "Sida ii av 6");
    }

    #[test]
    fn first_page_and_even_odd_variants() {
        let d = DocDefaults {
            header: Some("Vanlig".into()),
            header_first: Some("".into()),
            footer: Some("{sida}".into()),
            header_even: Some("Jämn".into()),
            footer_even: Some("".into()),
            ..Default::default()
        };
        let plan = Plan::new(&d, &[]);
        assert!(plan.even_odd);
        let pp = plan.pages(&[0, 0, 0, 0]);
        let base = fields(1, 4);
        // Första sidan: tomt sidhuvud, ingen sidfot (titlePg utan egen sidfot).
        let p = plan.page(&pp, 0, &base);
        assert_eq!((p.header, p.footer), (None, None));
        assert_eq!(plan.page(&pp, 1, &base).header, Some("Jämn"));
        assert_eq!(plan.page(&pp, 1, &base).footer, None);
        assert_eq!(plan.page(&pp, 2, &base).header, Some("Vanlig"));
        assert_eq!(plan.page(&pp, 2, &base).footer, Some("{sida}"));
    }

    #[test]
    fn later_section_title_page_inherits_first_header() {
        let d = DocDefaults { header: Some("A".into()), header_first: Some("Omslag".into()), ..Default::default() };
        let secs = [sec(SecHf::default()), sec(SecHf { title_pg: Some(true), ..Default::default() })];
        let plan = Plan::new(&d, &secs);
        let pp = plan.pages(&[0, 1, 1]);
        let base = fields(1, 3);
        assert_eq!(plan.page(&pp, 0, &base).header, Some("Omslag"));
        assert_eq!(plan.page(&pp, 1, &base).header, Some("Omslag"));
        assert_eq!(plan.page(&pp, 2, &base).header, Some("A"));
    }

    fn runs3(p: [&str; 3]) -> RowRuns {
        p.map(|t| runs_of(t, &fields(1, 1), &HfStyle::default()))
    }

    #[test]
    fn layout_places_parts_and_images() {
        let parts = runs3(["{bild:l.png 40x30}", "Titel", "Sida 1"]);
        let z = layout_zone(&parts, 10.0, 1.0, 400.0, &|t, s, _| t.chars().count() as f32 * s * 0.5, &|_| None);
        assert_eq!(z.height, 30.0);
        let img = z.items.iter().find_map(|i| if let Placed::Image { x, y, w, .. } = i { Some((*x, *y, *w)) } else { None }).unwrap();
        assert_eq!(img, (0.0, 0.0, 40.0));
        let titel = z.items.iter().find_map(|i| if let Placed::Text { x, text, .. } = i { (text == "Titel").then_some(*x) } else { None }).unwrap();
        assert_eq!(titel, (400.0 - 25.0) / 2.0);
        let right = z.items.iter().find_map(|i| if let Placed::Text { x, text, .. } = i { (text == "Sida 1").then_some(*x) } else { None }).unwrap();
        assert_eq!(right, 400.0 - 30.0);
    }

    #[test]
    fn anchored_images_do_not_grow_the_line() {
        let parts = runs3(["{bild:l.png 40x80 @10,5}", "X", ""]);
        let z = layout_zone(&parts, 10.0, 2.0, 400.0, &|_, _, _| 10.0, &|_| None);
        assert_eq!(z.height, 24.0);
        assert!(matches!(z.items[0], Placed::Image { x, y, anchored: true, .. } if x == 20.0 && y == 10.0));
    }

    #[test]
    fn rows_stack_and_reserve_adds_up() {
        let f = fields(1, 2);
        let rows = filled_rows("Skola\n||{sida}\nA|B|C", Align::Center, &f, &HfStyle::default());
        assert_eq!(rows.len(), 3);
        // Ett enkelt värde följer justeringen, en rad med `|` har egna delar.
        let text = |r: &RowRuns| r.clone().map(|p| p.into_iter().map(|r| if let Piece::Text(t) = r.piece { t } else { String::new() }).collect::<String>());
        assert_eq!(text(&rows[0]), ["", "Skola", ""].map(String::from));
        assert_eq!(text(&rows[1]), ["", "", "1"].map(String::from));
        assert_eq!(text(&rows[2]), ["A", "B", "C"].map(String::from));
        let z = layout_rows(&rows, 10.0, 1.0, 400.0, &|t, s, _| t.chars().count() as f32 * s * 0.5, &|_| None);
        assert_eq!(z.height, 36.0);
        let ys: Vec<f32> = z.items.iter().filter_map(|i| if let Placed::Text { y, .. } = i { Some(*y) } else { None }).collect();
        assert_eq!(ys, [0.0, 12.0, 24.0, 24.0, 24.0]);
        // Marginalen räknas på alla rader: avstånd + tre rader.
        let d = DocDefaults { header: Some("a\nb\nc".into()), header_style: HfStyle { size: Some(10.0), ..Default::default() }, ..Default::default() };
        let plan = Plan::new(&d, &[]);
        assert!((plan.reserve(&d, 0)[0] - (DEFAULT_DIST + 36.0)).abs() < 0.01);
    }

    #[test]
    fn inline_formatting_gives_runs_with_the_header_style_underneath() {
        let f = fields(2, 5);
        let st = HfStyle { size: Some(9.0), color: Some([0x66, 0x66, 0x66]), italic: true, ..Default::default() };
        let rows = filled_rows("**Skolrapport** · Klass 9B||Sida **{sida}** av {sidor}", Align::Left, &f, &st);
        let l = &rows[0][0];
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].piece, Piece::Text("Skolrapport".into()));
        // Fet ovanpå standardstilen (kursiv, grå, 9 pt).
        assert_eq!(l[0].fmt, RunFmt { bold: true, italic: true, color: Some([0x66, 0x66, 0x66]), size: Some(9.0), ..Default::default() });
        assert!(!l[1].fmt.bold && l[1].fmt.italic);
        // Fältet fylls i inuti märkena; värdet är aldrig markup.
        let r = &rows[0][2];
        let texts: Vec<_> = r.iter().map(|x| if let Piece::Text(t) = &x.piece { (t.as_str(), x.fmt.bold) } else { ("", false) }).collect();
        assert_eq!(texts, [("Sida ", false), ("2", true), (" av 5", false)]);
        let mut g = fields(1, 1);
        g.title = "a*b*c".into();
        let rows = filled_rows("**{titel}**", Align::Left, &g, &HfStyle::default());
        assert_eq!(rows[0][0].len(), 1);
        assert_eq!(rows[0][0][0].piece, Piece::Text("a*b*c".into()));
    }

    #[test]
    fn a_larger_run_makes_the_line_taller_and_sits_on_the_same_bottom_edge() {
        let rows = filled_rows("liten <span style=\"font-size: 20pt\">STOR</span>", Align::Left, &fields(1, 1), &HfStyle::default());
        let z = layout_rows(&rows, 10.0, 1.0, 400.0, &|t, s, f| t.chars().count() as f32 * s * if f.bold { 0.6 } else { 0.5 }, &|_| None);
        assert_eq!(z.height, 24.0);
        let ys: Vec<(String, f32, f32)> = z.items.iter().filter_map(|i| if let Placed::Text { y, text, size, .. } = i { Some((text.clone(), *y, *size)) } else { None }).collect();
        assert_eq!(ys, [("liten ".to_owned(), 12.0, 10.0), ("STOR".to_owned(), 0.0, 20.0)]);
        // Marginalen räknar med den största körningen.
        let d = DocDefaults { header: Some("a <span style=\"font-size: 20pt\">b</span>".into()), header_style: HfStyle { size: Some(10.0), ..Default::default() }, ..Default::default() };
        let plan = Plan::new(&d, &[]);
        assert!((plan.reserve(&d, 0)[0] - (DEFAULT_DIST + 24.0)).abs() < 0.01);
    }

    #[test]
    fn old_templates_without_markup_lay_out_as_before() {
        let f = fields(1, 1);
        let rows = filled_rows("A * B|{sida} < 2|5 ** 3", Align::Left, &f, &HfStyle { bold: true, ..Default::default() });
        assert_eq!(rows[0][0].len(), 1);
        assert_eq!(rows[0][0][0].piece, Piece::Text("A * B".into()));
        assert!(rows[0][0][0].fmt.bold);
        assert_eq!(rows[0][2][0].piece, Piece::Text("5 ** 3".into()));
    }

    #[test]
    fn multi_line_templates_survive_the_comment() {
        let d = DocDefaults { header: Some("Rad ett\n||Rad \"två\"".into()), header_align: Align::Center, ..Default::default() };
        let c = d.to_comment();
        assert!(c.contains("header: \"Rad ett\\n||Rad \\\"två\\\"\""), "{c}");
        let back = DocDefaults::from_comment(&c).unwrap();
        assert_eq!(back.header, d.header);
    }
}
