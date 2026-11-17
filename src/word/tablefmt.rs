//! Tabellformat från Word/LibreOffice: skuggning, ramar, bredd, justering, radhöjd, lodrät
//! justering. Markdown-tabellen har bara text, så formatet ligger i en egen kommentarsrad
//! rakt före tabellen (separerad med en tom rad, dold i dokumentläget som avsnittsraderna):
//!
//! ```text
//! <!-- kladd-tabell: width 60%; align center; border top double 6 C00000; row 0 h=45; cell 0.0 fill=5B3A9E va=center -->
//! ```
//!
//! Orden separeras med `;`; `rad.kolumn` räknas som i tabellens Markdown (rubrikraden är rad 0,
//! avgränsarraden räknas inte, kolumnen är cellens första rutnätskolumn). En tabell utan raden
//! visas som förut: rutnät runt alla celler och fet rubrikrad.
//!
//! Gränsregler (efter Word och WordCrafts `crates/layout`): en cells egen ram går före tabellens,
//! kantlinjen mellan två celler avgörs av den kraftigaste (bredd, sedan stil, sedan mörkast).

use std::collections::BTreeMap;

/// Linjestil. `None` = ingen linje.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BStyle {
    #[default]
    None,
    Single,
    Double,
    Dashed,
    Dotted,
}

impl BStyle {
    pub fn name(self) -> &'static str {
        match self {
            BStyle::None => "none",
            BStyle::Single => "single",
            BStyle::Double => "double",
            BStyle::Dashed => "dashed",
            BStyle::Dotted => "dotted",
        }
    }

    pub fn parse(s: &str) -> Option<BStyle> {
        Some(match s {
            "none" | "nil" => BStyle::None,
            "single" => BStyle::Single,
            "double" => BStyle::Double,
            "dashed" => BStyle::Dashed,
            "dotted" => BStyle::Dotted,
            _ => return None,
        })
    }

    /// `w:val` i Word.
    pub fn from_word(v: &str) -> BStyle {
        match v {
            "nil" | "none" => BStyle::None,
            "double" | "triple" | "thinThickSmallGap" | "thickThinSmallGap" | "thinThickThinSmallGap" | "thinThickMediumGap" | "thickThinMediumGap"
            | "thinThickThinMediumGap" | "thinThickLargeGap" | "thickThinLargeGap" | "thinThickThinLargeGap" => BStyle::Double,
            "dashed" | "dashSmallGap" | "dashDotStroked" | "dotDash" | "dotDotDash" | "dashDot" | "dashDotDot" => BStyle::Dashed,
            "dotted" => BStyle::Dotted,
            _ => BStyle::Single,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            BStyle::None => "nil",
            BStyle::Single => "single",
            BStyle::Double => "double",
            BStyle::Dashed => "dashed",
            BStyle::Dotted => "dotted",
        }
    }

    fn rank(self) -> u8 {
        match self {
            BStyle::None => 0,
            BStyle::Dotted => 1,
            BStyle::Dashed => 2,
            BStyle::Single => 3,
            BStyle::Double => 4,
        }
    }
}

/// En kantlinje: stil, bredd i åttondels punkter (Words `w:sz`) och färg (`None` = automatisk).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Border {
    pub style: BStyle,
    pub sz: u16,
    pub color: Option<[u8; 3]>,
}

impl Border {
    pub const NONE: Border = Border { style: BStyle::None, sz: 0, color: None };
    /// Rutnätet i Kladds egna tabeller (Word: Table Grid).
    pub const GRID: Border = Border { style: BStyle::Single, sz: 4, color: None };

    pub fn visible(&self) -> bool {
        self.style != BStyle::None
    }

    /// Bredd i punkter.
    pub fn pt(&self) -> f32 {
        f32::from(self.sz.max(2)) / 8.0
    }

    fn weight(&self) -> (u16, u8, u32) {
        let c = self.color.map_or(0, |c| 765 - u32::from(c[0]) - u32::from(c[1]) - u32::from(c[2]));
        (if self.visible() { self.sz.max(2) } else { 0 }, self.style.rank(), c)
    }

    /// Den kraftigaste av två kantlinjer på samma kant.
    pub fn stronger(a: Border, b: Border) -> Border {
        if b.weight() > a.weight() { b } else { a }
    }

    fn to_words(self) -> String {
        format!("{} {} {}", self.style.name(), self.sz, color_hex(self.color))
    }

    fn parse(style: &str, sz: &str, color: &str) -> Option<Border> {
        Some(Border { style: BStyle::parse(style)?, sz: sz.parse().ok()?, color: parse_color(color) })
    }
}

pub fn color_hex(c: Option<[u8; 3]>) -> String {
    match c {
        Some(c) => format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2]),
        None => "auto".to_owned(),
    }
}

pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Tabellens kanter: `[överkant, vänster, underkant, höger]` och mellan raderna/kolumnerna.
pub const TOP: usize = 0;
pub const LEFT: usize = 1;
pub const BOTTOM: usize = 2;
pub const RIGHT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Width {
    /// Efter innehållet (eller kolumnstrecken, som förut).
    #[default]
    Auto,
    /// Andel av textbredden i procent.
    Pct(u16),
    /// Fast bredd i punkter.
    Pt(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CellFmt {
    pub fill: Option<[u8; 3]>,
    pub valign: VAlign,
    /// Textens vågräta justering i cellen (när alla styckena har samma).
    pub halign: TAlign,
    /// Egna kantlinjer (`[top, left, bottom, right]`); `None` = tabellens.
    pub borders: [Option<Border>; 4],
}

impl CellFmt {
    fn is_empty(&self) -> bool {
        *self == CellFmt::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RowFmt {
    /// Radhöjd i punkter (0 = automatisk).
    pub h: u16,
    pub exact: bool,
    /// Rubrikrad som upprepas på varje sida (`w:tblHeader`).
    pub head: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableFmt {
    /// `[top, left, bottom, right, insideH, insideV]`.
    pub borders: [Border; 6],
    pub width: Width,
    pub fixed: bool,
    pub align: TAlign,
    /// Tabellens indrag från vänstermarginalen (pt).
    pub indent: f32,
    /// Cellmarginal vänster, höger, överkant och nederkant (pt).
    pub pad: [f32; 4],
    /// Första raden görs fet av Kladd (som i Kladds egna tabeller).
    pub header_bold: bool,
    pub rows: BTreeMap<usize, RowFmt>,
    pub cells: BTreeMap<(usize, usize), CellFmt>,
}

pub const DEFAULT_PAD: f32 = 5.4;
pub const DEFAULT_PADS: [f32; 4] = [DEFAULT_PAD, DEFAULT_PAD, 0.0, 0.0];

impl Default for TableFmt {
    /// Tabellen utan format: rutnät och fet rubrikrad (Kladds egna tabeller).
    fn default() -> Self {
        TableFmt {
            borders: [Border::GRID; 6],
            width: Width::Auto,
            fixed: false,
            align: TAlign::Left,
            indent: 0.0,
            pad: DEFAULT_PADS,
            header_bold: true,
            rows: BTreeMap::new(),
            cells: BTreeMap::new(),
        }
    }
}

pub const COMMENT_KEY: &str = "kladd-tabell:";

fn num(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl TableFmt {
    /// Ett format som Word-tabeller börjar från: inga ramar, ingen automatisk fetstil.
    pub fn word_base() -> Self {
        TableFmt { borders: [Border::NONE; 6], header_bold: false, ..Default::default() }
    }

    /// Sant om formatet skiljer sig från det en tabell utan kommentar har (då skrivs raden).
    pub fn is_default(&self) -> bool {
        *self == TableFmt::default()
    }

    pub fn cell(&self, r: usize, c: usize) -> Option<&CellFmt> {
        self.cells.get(&(r, c))
    }

    pub fn cell_mut(&mut self, r: usize, c: usize) -> &mut CellFmt {
        self.cells.entry((r, c)).or_default()
    }

    /// Cellens fyllning (cell, sedan inget).
    pub fn fill(&self, r: usize, c: usize) -> Option<[u8; 3]> {
        self.cell(r, c).and_then(|f| f.fill)
    }

    pub fn valign(&self, r: usize, c: usize) -> VAlign {
        self.cell(r, c).map_or(VAlign::Top, |f| f.valign)
    }

    /// Cellens egen kant (`side` = `TOP..=RIGHT`), om den har någon.
    pub fn own_border(&self, r: usize, c: usize, side: usize) -> Option<Border> {
        self.cell(r, c).and_then(|f| f.borders[side])
    }

    /// Kanten mellan celler: `a` är cellen före (ovanför/till vänster) och `b` den efter; `None`
    /// är tabellens yttre sida. `inside` väljer tabellens inre rad/kolumnlinje.
    pub fn edge(&self, side: usize, a_own: Option<Border>, b_own: Option<Border>, inside: bool) -> Border {
        let table = match (side, inside) {
            (TOP | BOTTOM, true) => self.borders[4],
            (LEFT | RIGHT, true) => self.borders[5],
            (s, false) => self.borders[s],
            _ => Border::NONE,
        };
        match (a_own, b_own) {
            (None, None) => table,
            (Some(x), None) | (None, Some(x)) => x,
            (Some(x), Some(y)) => Border::stronger(x, y),
        }
    }

    // ---------------------------------------------------------------- text

    /// Raden `<!-- kladd-tabell: … -->`.
    pub fn to_comment(&self) -> String {
        let d = TableFmt::default();
        let mut w: Vec<String> = Vec::new();
        match self.width {
            Width::Auto => {}
            Width::Pct(p) => w.push(format!("width {p}%")),
            Width::Pt(p) => w.push(format!("width {p}pt")),
        }
        if self.fixed {
            w.push("layout fixed".into());
        }
        match self.align {
            TAlign::Left => {}
            TAlign::Center => w.push("align center".into()),
            TAlign::Right => w.push("align right".into()),
        }
        if self.indent.abs() > 0.01 {
            w.push(format!("indent {}pt", num(self.indent)));
        }
        if self.pad != d.pad {
            w.push(format!("pad {} {} {} {}", num(self.pad[0]), num(self.pad[1]), num(self.pad[2]), num(self.pad[3])));
        }
        if !self.header_bold {
            w.push("header off".into());
        }
        if self.borders != d.borders {
            for (i, name) in ["top", "left", "bottom", "right", "h", "v"].iter().enumerate() {
                w.push(format!("border {name} {}", self.borders[i].to_words()));
            }
        }
        for (r, f) in &self.rows {
            let mut s = format!("row {r}");
            if f.h > 0 {
                s.push_str(&format!(" h={}{}", f.h, if f.exact { " exact" } else { "" }));
            }
            if f.head {
                s.push_str(" head");
            }
            w.push(s);
        }
        for ((r, c), f) in &self.cells {
            if f.is_empty() {
                continue;
            }
            let mut s = format!("cell {r}.{c}");
            if let Some(c) = f.fill {
                s.push_str(&format!(" fill={}", color_hex(Some(c))));
            }
            match f.valign {
                VAlign::Top => {}
                VAlign::Center => s.push_str(" va=center"),
                VAlign::Bottom => s.push_str(" va=bottom"),
            }
            match f.halign {
                TAlign::Left => {}
                TAlign::Center => s.push_str(" ha=center"),
                TAlign::Right => s.push_str(" ha=right"),
            }
            for (i, k) in ["bt", "bl", "bb", "br"].iter().enumerate() {
                if let Some(b) = f.borders[i] {
                    s.push_str(&format!(" {k}={}/{}/{}", b.style.name(), b.sz, color_hex(b.color)));
                }
            }
            w.push(s);
        }
        format!("<!-- {} {} -->", COMMENT_KEY, w.join("; "))
    }

    /// Läser en rad; `None` om den inte är en tabellrad. Okända ord hoppas över.
    pub fn from_comment(line: &str) -> Option<TableFmt> {
        let inner = line.trim().strip_prefix("<!--")?.strip_suffix("-->")?.trim();
        let rest = inner.strip_prefix(COMMENT_KEY)?;
        // En kommentar utan format är en tabell med de vanliga rutnätsegenskaperna.
        let mut f = TableFmt { borders: [Border::NONE; 6], ..Default::default() };
        let mut explicit_borders = false;
        for word in rest.split(';') {
            let t: Vec<&str> = word.split_whitespace().collect();
            match t.as_slice() {
                ["width", v] => {
                    f.width = if let Some(p) = v.strip_suffix('%') {
                        p.parse().map_or(Width::Auto, Width::Pct)
                    } else if let Some(p) = v.strip_suffix("pt") {
                        p.parse::<f32>().map_or(Width::Auto, |x| Width::Pt(x.round().clamp(1.0, 6000.0) as u16))
                    } else {
                        Width::Auto
                    }
                }
                ["layout", "fixed"] => f.fixed = true,
                ["align", v] => {
                    f.align = match *v {
                        "center" => TAlign::Center,
                        "right" => TAlign::Right,
                        _ => TAlign::Left,
                    }
                }
                ["indent", v] => f.indent = v.trim_end_matches("pt").parse().unwrap_or(0.0),
                ["pad", a, b, c, d] => {
                    if let (Ok(a), Ok(b), Ok(c), Ok(d)) = (a.parse(), b.parse(), c.parse(), d.parse()) {
                        f.pad = [a, b, c, d];
                    }
                }
                ["header", "off"] => f.header_bold = false,
                ["border", side, style, sz, color] => {
                    let i = ["top", "left", "bottom", "right", "h", "v"].iter().position(|s| s == side);
                    if let (Some(i), Some(b)) = (i, Border::parse(style, sz, color)) {
                        f.borders[i] = b;
                        explicit_borders = true;
                    }
                }
                ["row", n, rest @ ..] => {
                    if let Ok(n) = n.parse::<usize>() {
                        let rf = f.rows.entry(n).or_default();
                        for a in rest {
                            if let Some(h) = a.strip_prefix("h=") {
                                rf.h = h.parse().unwrap_or(0);
                            } else if *a == "exact" {
                                rf.exact = true;
                            } else if *a == "head" {
                                rf.head = true;
                            }
                        }
                    }
                }
                ["cell", pos, rest @ ..] => {
                    let Some((r, c)) = pos.split_once('.').and_then(|(r, c)| Some((r.parse::<usize>().ok()?, c.parse::<usize>().ok()?))) else {
                        continue;
                    };
                    let cf = f.cells.entry((r, c)).or_default();
                    for a in rest {
                        let Some((k, v)) = a.split_once('=') else { continue };
                        match k {
                            "fill" => cf.fill = parse_color(v),
                            "va" => {
                                cf.valign = match v {
                                    "center" => VAlign::Center,
                                    "bottom" => VAlign::Bottom,
                                    _ => VAlign::Top,
                                }
                            }
                            "ha" => {
                                cf.halign = match v {
                                    "center" => TAlign::Center,
                                    "right" => TAlign::Right,
                                    _ => TAlign::Left,
                                }
                            }
                            "bt" | "bl" | "bb" | "br" => {
                                let side = ["bt", "bl", "bb", "br"].iter().position(|s| *s == k).unwrap_or(0);
                                let p: Vec<&str> = v.split('/').collect();
                                if let [s, z, c] = p.as_slice() {
                                    cf.borders[side] = Border::parse(s, z, c);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        // `border`-ord utelämnas när rutnätet är oförändrat.
        if !explicit_borders && !rest.contains("border") {
            f.borders = [Border::GRID; 6];
        }
        Some(f)
    }

    // ---------------------------------------------------------------- ändringar vid radoperationer

    /// En rad har satts in före `at`: formatet för raderna från `at` flyttas ned.
    pub fn shift_rows(&mut self, at: usize, delta: isize) {
        let mv = |r: usize| -> Option<usize> {
            if r < at {
                Some(r)
            } else if delta >= 0 {
                Some(r + delta as usize)
            } else {
                let d = (-delta) as usize;
                if r < at + d { None } else { Some(r - d) }
            }
        };
        self.rows = std::mem::take(&mut self.rows).into_iter().filter_map(|(r, f)| mv(r).map(|n| (n, f))).collect();
        self.cells = std::mem::take(&mut self.cells).into_iter().filter_map(|((r, c), f)| mv(r).map(|n| ((n, c), f))).collect();
    }

    /// Samma för kolumner.
    pub fn shift_cols(&mut self, at: usize, delta: isize) {
        let mv = |c: usize| -> Option<usize> {
            if c < at {
                Some(c)
            } else if delta >= 0 {
                Some(c + delta as usize)
            } else {
                let d = (-delta) as usize;
                if c < at + d { None } else { Some(c - d) }
            }
        };
        self.cells = std::mem::take(&mut self.cells).into_iter().filter_map(|((r, c), f)| mv(c).map(|n| ((r, n), f))).collect();
    }
}

// ------------------------------------------------------------------ hitta raden i texten

/// Kommentarsraden som hör till tabellen som börjar på `table_start`: raden närmast före, med
/// högst en tom rad emellan. Ger radens byteintervall (utan radslut) och formatet.
pub fn before(text: &str, table_start: usize) -> Option<(std::ops::Range<usize>, TableFmt)> {
    let head = text.get(..table_start)?;
    let head = head.strip_suffix('\n')?;
    let head = head.strip_suffix('\n').unwrap_or(head);
    let start = head.rfind('\n').map_or(0, |i| i + 1);
    let line = &head[start..];
    if !line.trim_start().starts_with("<!--") {
        return None;
    }
    TableFmt::from_comment(line).map(|f| (start..head.len(), f))
}

/// Är `line` en tabellkommentar?
pub fn is_comment(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("<!--") && t.ends_with("-->") && t[4..].trim_start().starts_with(COMMENT_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TableFmt {
        let mut f = TableFmt::word_base();
        f.width = Width::Pct(60);
        f.align = TAlign::Center;
        f.borders[TOP] = Border { style: BStyle::Double, sz: 6, color: Some([0xC0, 0, 0]) };
        f.borders[4] = Border { style: BStyle::Dashed, sz: 4, color: None };
        f.rows.insert(0, RowFmt { h: 45, exact: false, head: true });
        let c = f.cell_mut(0, 0);
        c.fill = Some([0x5B, 0x3A, 0x9E]);
        c.valign = VAlign::Center;
        c.borders[BOTTOM] = Some(Border { style: BStyle::Single, sz: 12, color: Some([1, 2, 3]) });
        f
    }

    #[test]
    fn comment_round_trips() {
        let f = sample();
        let s = f.to_comment();
        assert!(s.starts_with("<!-- kladd-tabell: "), "{s}");
        assert_eq!(TableFmt::from_comment(&s), Some(f));
    }

    #[test]
    fn default_has_grid_and_bold_header() {
        let d = TableFmt::default();
        assert!(d.is_default() && d.header_bold && d.borders.iter().all(|b| *b == Border::GRID));
        // En tom kommentar är rutnätet.
        assert_eq!(TableFmt::from_comment("<!-- kladd-tabell:  -->"), Some(d.clone()));
        assert_eq!(TableFmt::from_comment(&d.to_comment()), Some(d));
    }

    #[test]
    fn borderless_table_keeps_no_borders() {
        let f = TableFmt::word_base();
        let back = TableFmt::from_comment(&f.to_comment()).unwrap();
        assert!(back.borders.iter().all(|b| !b.visible()));
        assert!(!back.header_bold);
    }

    #[test]
    fn other_comments_are_not_table_comments() {
        assert!(TableFmt::from_comment("<!-- kladd: avsnitt -->").is_none());
        assert!(TableFmt::from_comment("<!-- kladd: font-size: 12pt -->").is_none());
        assert!(!is_comment("<!-- kladd: toc -->"));
        assert!(is_comment(&sample().to_comment()));
    }

    #[test]
    fn finds_the_comment_before_a_table() {
        let c = sample().to_comment();
        let t = format!("Text\n\n{c}\n\n| a | b |\n| - | - |\n");
        let at = t.find("| a").unwrap();
        let (r, f) = before(&t, at).unwrap();
        assert_eq!(&t[r], c);
        assert_eq!(f, sample());
        assert!(before("Text\n\n| a |\n| - |\n", 6).is_none());
        let first = format!("{c}\n\n| a |\n| - |\n");
        assert!(before(&first, first.find("| a").unwrap()).is_some());
    }

    #[test]
    fn stronger_border_wins() {
        let thin = Border { style: BStyle::Single, sz: 4, color: None };
        let thick = Border { style: BStyle::Single, sz: 12, color: Some([255, 0, 0]) };
        let dbl = Border { style: BStyle::Double, sz: 12, color: Some([0, 0, 0]) };
        assert_eq!(Border::stronger(thin, thick), thick);
        assert_eq!(Border::stronger(thick, thin), thick);
        assert_eq!(Border::stronger(thick, Border::NONE), thick);
        assert_eq!(Border::stronger(Border::NONE, thin), thin);
        assert_eq!(Border::stronger(thick, dbl), dbl);
    }

    #[test]
    fn edge_prefers_cell_over_table() {
        let mut f = TableFmt::word_base();
        f.borders[4] = Border::GRID;
        let own = Border { style: BStyle::Single, sz: 12, color: None };
        assert_eq!(f.edge(BOTTOM, None, None, true), Border::GRID);
        assert_eq!(f.edge(BOTTOM, Some(own), None, true), own);
        assert_eq!(f.edge(LEFT, None, None, false), Border::NONE);
        assert_eq!(f.edge(BOTTOM, Some(Border::NONE), Some(own), true), own);
    }

    #[test]
    fn shifting_rows_and_columns_moves_the_formats() {
        let mut f = sample();
        f.shift_rows(0, 1);
        assert!(f.fill(1, 0).is_some() && f.fill(0, 0).is_none() && f.rows.contains_key(&1));
        f.shift_rows(1, -1);
        assert!(f.fill(1, 0).is_none() && f.rows.is_empty());
        let mut g = sample();
        g.shift_cols(0, 1);
        assert!(g.fill(0, 1).is_some());
    }
}
