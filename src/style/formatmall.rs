//! Formatmallar: dokumentets egna utseende för Brödtext, Rubrik 1-6 och Citat.
//!
//! De ligger i `<!-- kladd: … -->`-raden, en deklaration per formatmall, med ett kompakt värde:
//!
//! ```text
//! h1: "Calibri Light" 16pt bold #2F5496 space-before 12pt space-after 6pt;
//! body: justify line-spacing 1.15 indent 18pt; quote: italic #404040
//! ```
//!
//! Värdet är ord i valfri ordning: ett citerat typsnitt, storlek (`16pt`), `bold`/`nobold`,
//! `italic`/`noitalic`, färg (`#RRGGBB`), `left|center|right|justify`, `space-before L`,
//! `space-after L`, `line-spacing N` (gånger enkelt radavstånd) och `indent L` (indrag av första
//! raden). Det som inte anges ärvs från Kladds standardutseende, så ett dokument utan
//! formatmallar ser ut som förut.
use super::{Align, hex, parse_color, parse_length, quote, unquote};
use crate::i18n::t;
use crate::tf;
use crate::word::markup::Block;

/// En formatmall som går att ändra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleId {
    Body,
    Heading(u8),
    Quote,
}

/// Alla formatmallar i den ordning de visas och skrivs.
pub const STYLE_IDS: [StyleId; 8] = [
    StyleId::Body,
    StyleId::Heading(1),
    StyleId::Heading(2),
    StyleId::Heading(3),
    StyleId::Heading(4),
    StyleId::Heading(5),
    StyleId::Heading(6),
    StyleId::Quote,
];

impl StyleId {
    /// Nyckeln i kommentarraden.
    pub fn key(self) -> &'static str {
        match self {
            StyleId::Body => "body",
            StyleId::Heading(1) => "h1",
            StyleId::Heading(2) => "h2",
            StyleId::Heading(3) => "h3",
            StyleId::Heading(4) => "h4",
            StyleId::Heading(5) => "h5",
            StyleId::Heading(_) => "h6",
            StyleId::Quote => "quote",
        }
    }

    pub fn from_key(k: &str) -> Option<StyleId> {
        STYLE_IDS.iter().copied().find(|s| s.key() == k)
    }

    /// Namnet i gränssnittet.
    pub fn label(self) -> String {
        match self {
            StyleId::Body => t("Brödtext").into(),
            StyleId::Heading(n) => tf!("Rubrik {n}", n = n),
            StyleId::Quote => t("Citat").into(),
        }
    }

    fn index(self) -> usize {
        match self {
            StyleId::Body => 0,
            StyleId::Heading(n) => n.clamp(1, 6) as usize,
            StyleId::Quote => 7,
        }
    }

    /// Formatmallen för ett stycke av slaget `b` (`None` för kod, tabeller och fotnoter).
    pub fn for_block(b: Block) -> Option<StyleId> {
        match b {
            Block::Normal | Block::Bullet | Block::Numbered => Some(StyleId::Body),
            Block::Heading(n) => Some(StyleId::Heading(n.clamp(1, 6))),
            Block::Quote => Some(StyleId::Quote),
            Block::Code | Block::Table | Block::Footnote => None,
        }
    }

    /// Kladds standardstorlek (pt) och om mallen är fet/kursiv, för förhandsvisning och
    /// "Återställ". `body` = dokumentets brödtextstorlek.
    pub fn default_size(self, body: f32) -> f32 {
        match self {
            StyleId::Heading(1) => 18.0,
            StyleId::Heading(2) => 15.0,
            StyleId::Heading(3) => 13.0,
            StyleId::Heading(4) => 12.0,
            StyleId::Heading(_) => 11.0,
            _ => body,
        }
    }
}

/// Det en formatmall ändrar. `None` = Kladds standard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleOverride {
    pub font_family: Option<String>,
    /// Punkter.
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<[u8; 3]>,
    pub align: Option<Align>,
    /// Punkter.
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    /// Gånger enkelt radavstånd (1,0 = enkelt, 1,15 = Words standard).
    pub line_spacing: Option<f32>,
    /// Indrag av första raden, punkter.
    pub first_indent: Option<f32>,
}

impl StyleOverride {
    pub fn is_empty(&self) -> bool {
        *self == StyleOverride::default()
    }

    /// Det som skiljer sig från `base` (lika egenskaper blir `None`).
    pub fn minus(&self, base: &StyleOverride) -> StyleOverride {
        fn keep<T: PartialEq + Clone>(a: &Option<T>, b: &Option<T>) -> Option<T> {
            if a == b { None } else { a.clone() }
        }
        StyleOverride {
            font_family: keep(&self.font_family, &base.font_family),
            size: keep(&self.size, &base.size),
            bold: keep(&self.bold, &base.bold),
            italic: keep(&self.italic, &base.italic),
            color: keep(&self.color, &base.color),
            align: keep(&self.align, &base.align),
            space_before: keep(&self.space_before, &base.space_before),
            space_after: keep(&self.space_after, &base.space_after),
            line_spacing: keep(&self.line_spacing, &base.line_spacing),
            first_indent: keep(&self.first_indent, &base.first_indent),
        }
    }

    /// Värdet som skrivs efter `h1:` (tomt om inget avviker).
    pub fn to_value(&self) -> String {
        let mut w: Vec<String> = Vec::new();
        if let Some(f) = self.font_family.as_deref().filter(|f| !f.is_empty()) {
            w.push(quote(f));
        }
        if let Some(s) = self.size {
            w.push(format!("{}pt", fmt_num(s)));
        }
        match self.bold {
            Some(true) => w.push("bold".into()),
            Some(false) => w.push("nobold".into()),
            None => {}
        }
        match self.italic {
            Some(true) => w.push("italic".into()),
            Some(false) => w.push("noitalic".into()),
            None => {}
        }
        if let Some(c) = self.color {
            w.push(hex(c));
        }
        if let Some(a) = self.align {
            w.push(a.as_attr().into());
        }
        if let Some(v) = self.space_before {
            w.push(format!("space-before {}pt", fmt_num(v)));
        }
        if let Some(v) = self.space_after {
            w.push(format!("space-after {}pt", fmt_num(v)));
        }
        if let Some(v) = self.line_spacing {
            w.push(format!("line-spacing {}", fmt_num(v)));
        }
        if let Some(v) = self.first_indent {
            w.push(format!("indent {}pt", fmt_num(v)));
        }
        w.join(" ")
    }

    /// Läser ett värde som `"Calibri Light" 16pt bold #2F5496 space-before 12pt`. Okända ord hoppas över.
    pub fn from_value(v: &str) -> StyleOverride {
        let mut o = StyleOverride::default();
        let toks = tokens(v);
        let mut i = 0;
        while i < toks.len() {
            let t = toks[i].as_str();
            let lower = t.to_ascii_lowercase();
            match lower.as_str() {
                "bold" => o.bold = Some(true),
                "nobold" => o.bold = Some(false),
                "italic" => o.italic = Some(true),
                "noitalic" => o.italic = Some(false),
                "left" | "center" | "right" | "justify" => o.align = Some(Align::from_attr(&lower)),
                "space-before" | "space-after" | "indent" => {
                    i += 1;
                    let len = toks.get(i).and_then(|a| parse_length(a));
                    match lower.as_str() {
                        "space-before" => o.space_before = len,
                        "space-after" => o.space_after = len,
                        _ => o.first_indent = len,
                    }
                }
                "line-spacing" => {
                    i += 1;
                    o.line_spacing = toks.get(i).and_then(|a| a.parse::<f32>().ok()).filter(|x| *x > 0.0 && *x < 20.0);
                }
                _ if t.starts_with('"') => o.font_family = Some(unquote(t)).filter(|f| !f.is_empty()),
                _ if t.starts_with('#') => o.color = parse_color(t),
                _ => {
                    if let Some(pt) = lower.strip_suffix("pt").and_then(|n| n.parse::<f32>().ok()).filter(|s| *s > 0.0 && *s < 1000.0) {
                        o.size = Some(pt);
                    }
                }
            }
            i += 1;
        }
        o
    }
}

/// Tal med högst två decimaler (`16`, `1.15`).
fn fmt_num(v: f32) -> String {
    let r = (v * 100.0).round() / 100.0;
    format!("{r}")
}

/// Ord separerade av blanksteg; ett citerat ord (typsnittsnamn) får innehålla blanksteg.
fn tokens(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut esc = false;
    for c in v.chars() {
        if in_q {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_q = false;
            }
        } else if c == '"' {
            in_q = true;
            cur.push(c);
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Dokumentets alla formatmallsändringar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleSet {
    items: [StyleOverride; 8],
}

impl StyleSet {
    pub fn is_empty(&self) -> bool {
        self.items.iter().all(StyleOverride::is_empty)
    }

    pub fn get(&self, id: StyleId) -> &StyleOverride {
        &self.items[id.index()]
    }

    pub fn set(&mut self, id: StyleId, o: StyleOverride) {
        self.items[id.index()] = o;
    }

    /// Det som skiljer sig från `base`, formatmall för formatmall.
    pub fn minus(&self, base: &StyleSet) -> StyleSet {
        let mut out = StyleSet::default();
        for id in STYLE_IDS {
            out.set(id, self.get(id).minus(base.get(id)));
        }
        out
    }

    /// Ändringarna för ett stycke av slaget `b`, om det har någon formatmall med ändringar.
    pub fn for_block(&self, b: Block) -> Option<&StyleOverride> {
        StyleId::for_block(b).map(|id| self.get(id)).filter(|o| !o.is_empty())
    }

    /// `nyckel: värde` för varje formatmall som ändrats, till kommentarraden.
    pub fn to_parts(&self) -> Vec<String> {
        STYLE_IDS
            .iter()
            .filter_map(|&id| {
                let v = self.get(id).to_value();
                (!v.is_empty()).then(|| format!("{}: {v}", id.key()))
            })
            .collect()
    }

    /// Tar emot en deklaration ur kommentarraden. Sant om nyckeln var en formatmall.
    pub fn read_decl(&mut self, key: &str, value: &str) -> bool {
        match StyleId::from_key(key) {
            Some(id) => {
                self.set(id, StyleOverride::from_value(value));
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_round_trip() {
        let o = StyleOverride {
            font_family: Some("Calibri Light".into()),
            size: Some(16.0),
            bold: Some(true),
            italic: Some(false),
            color: Some([0x2F, 0x54, 0x96]),
            align: Some(Align::Center),
            space_before: Some(12.0),
            space_after: Some(6.0),
            line_spacing: Some(1.15),
            first_indent: Some(18.0),
        };
        let v = o.to_value();
        assert_eq!(
            v,
            "\"Calibri Light\" 16pt bold noitalic #2F5496 center space-before 12pt space-after 6pt line-spacing 1.15 indent 18pt"
        );
        assert_eq!(StyleOverride::from_value(&v), o);
    }

    #[test]
    fn unknown_words_are_skipped() {
        let o = StyleOverride::from_value("blink 12pt wibble bold");
        assert_eq!(o, StyleOverride { size: Some(12.0), bold: Some(true), ..Default::default() });
    }

    #[test]
    fn style_set_keys() {
        let mut s = StyleSet::default();
        assert!(s.read_decl("h1", "16pt bold"));
        assert!(!s.read_decl("font-size", "12pt"));
        assert_eq!(s.get(StyleId::Heading(1)).size, Some(16.0));
        assert_eq!(s.to_parts(), vec!["h1: 16pt bold".to_owned()]);
        assert!(StyleSet::default().is_empty());
    }
}
