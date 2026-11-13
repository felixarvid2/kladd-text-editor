//! Avsnitt (som Words sektioner): en del av dokumentet kan ha egen sidstorlek, liggande sidor,
//! egna marginaler och flera spalter. Ren logik utan UI; layouten, Word/ODT och PDF läser
//! avsnitten härifrån. Se docs/dev/dokumentlage.md, "Avsnitt".
//!
//! Ett avsnitt börjar vid en egen rad i Markdown-dialekten:
//!
//! ```text
//! <!-- kladd: avsnitt; page: A4 landscape; page-margin: 20mm; columns: 2; column-gap: 12mm; type: continuous -->
//! ```
//!
//! Egenskaper som saknas ärver dokumentets grundformat (första radens kommentar), inte det
//! föregående avsnittet. `type: continuous` låter avsnittet börja på samma sida (för byte av
//! spalter); annars börjar det på en ny sida. En rad `<!-- kladd: spaltbrytning -->` flyttar texten
//! till nästa spalt. En avsnittsrad som står före all text gäller första avsnittet.

use std::ops::Range;

use super::markup::{DocDefaults, Edit};
use crate::style;

/// Nyckelordet i raden som börjar ett avsnitt.
pub const SECTION_KEY: &str = "avsnitt";
/// Raden som flyttar texten till nästa spalt.
pub const COLUMN_BREAK_LINE: &str = "<!-- kladd: spaltbrytning -->";
/// Mellanrum mellan spalter (pt) när `column-gap` saknas (som Words 1,25 cm = 708 twips).
pub const DEFAULT_GAP: f32 = 35.4;
/// Största antal spalter.
pub const MAX_COLUMNS: usize = 6;

/// Sant om `css` (texten efter `kladd:`) börjar en avsnittsrad.
pub fn is_section_keyword(css: &str) -> bool {
    let t = css.trim_start();
    for key in [SECTION_KEY, "spaltbrytning"] {
        if let Some(rest) = t.strip_prefix(key) {
            if rest.is_empty() || rest.starts_with(|c: char| c == ';' || c.is_whitespace()) {
                return true;
            }
        }
    }
    false
}

/// Sant om `line` är spaltbrytningsraden.
pub fn is_column_break(line: &str) -> bool {
    line.trim() == COLUMN_BREAK_LINE
}

/// Egenskaperna som står på en avsnittsrad. `None` = ärv dokumentets grundformat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SectionProps {
    /// Sidans bredd och höjd i pt.
    pub page_size: Option<(f32, f32)>,
    /// Marginaler i pt: topp, höger, botten, vänster.
    pub page_margin: Option<[f32; 4]>,
    pub columns: Option<usize>,
    /// Mellanrum mellan spalter i pt.
    pub column_gap: Option<f32>,
    /// Börjar på samma sida som föregående avsnitt (annars på en ny sida).
    pub continuous: bool,
}

impl SectionProps {
    /// Raden som skrivs i texten, utan radslut.
    pub fn to_comment(&self) -> String {
        let mut parts = vec![SECTION_KEY.to_owned()];
        if let Some((w, h)) = self.page_size {
            parts.push(format!("page: {}", style::page_size_css(w, h)));
        }
        if let Some(m) = self.page_margin {
            parts.push(format!("page-margin: {}", style::margin_css(m)));
        }
        if let Some(c) = self.columns.filter(|c| *c > 1) {
            parts.push(format!("columns: {c}"));
        }
        if let Some(g) = self.column_gap {
            parts.push(format!("column-gap: {}", style::mm(g)));
        }
        if self.continuous {
            parts.push("type: continuous".to_owned());
        }
        format!("<!-- kladd: {} -->", parts.join("; "))
    }

    /// Läser en rad som `<!-- kladd: avsnitt; page: A4 landscape; columns: 2 -->`.
    pub fn from_comment(line: &str) -> Option<SectionProps> {
        let t = line.trim();
        let inner = t.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
        let css = inner.strip_prefix("kladd:")?;
        let head = css.trim_start();
        let rest = head.strip_prefix(SECTION_KEY)?;
        if !(rest.is_empty() || rest.starts_with(|c: char| c == ';' || c.is_whitespace())) {
            return None;
        }
        let mut p = SectionProps::default();
        for (k, v) in style::declarations(css) {
            match k.as_str() {
                "page" | "page-size" => p.page_size = style::parse_page_size(&v),
                "page-margin" | "margin" => p.page_margin = style::parse_margins(&v),
                "columns" => p.columns = v.trim().parse::<usize>().ok().map(|c| c.clamp(1, MAX_COLUMNS)),
                "column-gap" => p.column_gap = style::parse_length(&v),
                "type" => p.continuous = v.trim().eq_ignore_ascii_case("continuous"),
                _ => {}
            }
        }
        Some(p)
    }
}

/// Ett avsnitt med egenskaperna uträknade mot dokumentets grundformat.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// Byteoffset där avsnittet börjar (avsnittsradens början; 0 för det första).
    pub start: usize,
    /// Avsnittsraden (utan radslut); `None` för ett första avsnitt utan egen rad.
    pub marker: Option<Range<usize>>,
    pub props: SectionProps,
    /// Sidans bredd och höjd i pt.
    pub page: (f32, f32),
    /// Marginaler i pt: topp, höger, botten, vänster.
    pub margin: [f32; 4],
    pub columns: usize,
    /// Mellanrum mellan spalter i pt.
    pub gap: f32,
    pub continuous: bool,
}

impl Section {
    /// Räknar ut ett avsnitt av radens egenskaper och dokumentets grundformat.
    pub fn resolve(props: &SectionProps, d: &DocDefaults) -> Section {
        Section {
            start: 0,
            marker: None,
            props: props.clone(),
            page: props.page_size.unwrap_or_else(|| d.page()),
            margin: props.page_margin.unwrap_or_else(|| d.margins()),
            columns: props.columns.unwrap_or(1).clamp(1, MAX_COLUMNS),
            gap: props.column_gap.unwrap_or(DEFAULT_GAP).max(0.0),
            continuous: props.continuous,
        }
    }

    pub fn landscape(&self) -> bool {
        self.page.0 > self.page.1 + 0.5
    }
}

/// Avsnitten i en text, minst ett. Rader i kodstaket räknas inte.
pub fn scan(text: &str, d: &DocDefaults) -> Vec<Section> {
    let mut out = vec![Section::resolve(&SectionProps::default(), d)];
    if !text.contains("kladd: avsnitt") {
        return out;
    }
    let mut off = 0usize;
    let mut fence = false;
    // Har det kommit någon text före raden? Annars gäller en avsnittsrad första avsnittet.
    let mut content = false;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let start = off;
        off += line.len();
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            content = true;
            continue;
        }
        if fence {
            continue;
        }
        if t.starts_with("<!--") {
            if let Some(props) = SectionProps::from_comment(t) {
                let marker = start..start + line.trim_end_matches(['\n', '\r']).len();
                let mut s = Section::resolve(&props, d);
                s.marker = Some(marker.clone());
                if !content && out.len() == 1 && out[0].marker.is_none() {
                    s.start = 0;
                    s.continuous = false;
                    out[0] = s;
                } else {
                    s.start = marker.start;
                    out.push(s);
                }
                continue;
            }
        }
        if !t.is_empty() && !(i == 0 && DocDefaults::from_comment(t).is_some()) {
            content = true;
        }
    }
    out
}

/// Indexet för avsnittet som `byte` ligger i.
pub fn index_at(secs: &[Section], byte: usize) -> usize {
    secs.partition_point(|s| s.start <= byte).saturating_sub(1)
}

// ------------------------------------------------------------------ redigering

/// Sätter in `line` som ett eget stycke vid `at` (tomma rader runt om). Markören hamnar efter den.
fn insert_line(text: &str, at: usize, line: &str) -> Edit {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let before = &text[..at];
    let after = &text[at..];
    let lead = if before.is_empty() || before.ends_with("\n\n") {
        ""
    } else if before.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let newlines = after.bytes().take_while(|b| *b == b'\n').count().min(2);
    let trail = "\n".repeat(2 - newlines);
    let mut t = String::with_capacity(text.len() + line.len() + 4);
    t.push_str(before);
    t.push_str(lead);
    t.push_str(line);
    t.push_str(&trail);
    let end = t.len() - trail.len();
    t.push_str(after);
    let caret = (end + 2).min(t.len());
    Edit { text: t, sel: caret..caret }
}

/// Börjar ett nytt avsnitt med egenskaperna `props` vid `at`.
pub fn insert_break(text: &str, at: usize, props: &SectionProps) -> Edit {
    insert_line(text, at, &props.to_comment())
}

/// Sätter in en spaltbrytning vid `at`.
pub fn insert_column_break(text: &str, at: usize) -> Edit {
    insert_line(text, at, COLUMN_BREAK_LINE)
}

/// Egenskaperna för avsnittet som `at` ligger i, och avsnittets index.
pub fn current(text: &str, at: usize, d: &DocDefaults) -> (usize, Section) {
    let secs = scan(text, d);
    let i = index_at(&secs, at);
    (i, secs[i].clone())
}

/// Ändrar egenskaperna för avsnittet som `at` ligger i: avsnittsraden skrivs om, och det första
/// avsnittet får en rad före all text om det saknar en. Ger tillbaka `caret` flyttad.
pub fn update_current(text: &str, at: usize, d: &DocDefaults, f: impl FnOnce(&mut SectionProps)) -> Edit {
    let secs = scan(text, d);
    let i = index_at(&secs, at);
    let sec = &secs[i];
    let mut props = sec.props.clone();
    f(&mut props);
    if i == 0 {
        props.continuous = false;
    }
    let shift = |b: usize, old: &Range<usize>, new_len: usize| -> usize {
        if b >= old.end {
            b - old.len() + new_len
        } else if b > old.start {
            old.start + new_len
        } else {
            b
        }
    };
    match &sec.marker {
        Some(m) if i == 0 && props == SectionProps::default() => {
            // Tillbaka till grundformatet: raden och den tomma raden efter den försvinner.
            let mut end = m.end;
            for _ in 0..2 {
                if text[end..].starts_with("\r\n") {
                    end += 2;
                } else if text[end..].starts_with('\n') {
                    end += 1;
                }
            }
            let mut t = String::with_capacity(text.len());
            t.push_str(&text[..m.start]);
            t.push_str(&text[end..]);
            let cut = end - m.start;
            let s = |b: usize| if b >= end { b - cut } else { b.min(m.start) };
            Edit { text: t, sel: s(at)..s(at) }
        }
        Some(m) => {
            let line = props.to_comment();
            let mut t = String::with_capacity(text.len() + 32);
            t.push_str(&text[..m.start]);
            t.push_str(&line);
            t.push_str(&text[m.end..]);
            let c = shift(at, m, line.len());
            Edit { text: t, sel: c..c }
        }
        None => {
            // En rad före all text, efter grundformatraden.
            let first_end = text.find('\n').unwrap_or(text.len());
            let mut pos = 0;
            if DocDefaults::from_comment(&text[..first_end]).is_some() {
                pos = first_end;
                for _ in 0..2 {
                    if text[pos..].starts_with("\r\n") {
                        pos += 2;
                    } else if text[pos..].starts_with('\n') {
                        pos += 1;
                    }
                }
            }
            let line = props.to_comment();
            let mut t = String::with_capacity(text.len() + line.len() + 2);
            t.push_str(&text[..pos]);
            t.push_str(&line);
            t.push_str("\n\n");
            t.push_str(&text[pos..]);
            let add = line.len() + 2;
            let c = if at >= pos { at + add } else { at };
            Edit { text: t, sel: c..c }
        }
    }
}

/// Liggande (eller stående) sidor från `at`: ändrar avsnittet om det inte har någon text före
/// `at`, annars börjar ett nytt avsnitt där.
pub fn set_landscape_from(text: &str, at: usize, d: &DocDefaults, landscape: bool) -> Edit {
    let (i, sec) = current(text, at, d);
    let flip = |p: &mut SectionProps, sec: &Section| {
        let (w, h) = sec.page;
        let (a, b) = (w.min(h), w.max(h));
        p.page_size = Some(if landscape { (b, a) } else { (a, b) });
    };
    let body_start = sec.marker.as_ref().map_or(sec.start, |m| m.end);
    let before_blank = text[body_start.min(at)..at].trim().is_empty();
    // Ingen text före markören i första avsnittet: bara avsnittets sidor ändras.
    let lead_blank = i == 0 && sec.marker.is_none() && text[..at].lines().all(|l| l.trim().is_empty() || DocDefaults::from_comment(l).is_some());
    if before_blank && (sec.marker.is_some() || lead_blank) {
        return update_current(text, at, d, |p| flip(p, &sec));
    }
    let mut props = sec.props.clone();
    props.continuous = false;
    flip(&mut props, &sec);
    insert_break(text, at, &props)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::markup;

    const MARK: &str = "<!-- kladd: avsnitt; page: A4 landscape; columns: 2; column-gap: 12mm -->";

    #[test]
    fn marker_roundtrip() {
        let p = SectionProps::from_comment(MARK).unwrap();
        assert_eq!(p.page_size, Some((841.89, 595.28)));
        assert_eq!(p.columns, Some(2));
        assert!((p.column_gap.unwrap() - 12.0 * 72.0 / 25.4).abs() < 0.01);
        assert!(!p.continuous);
        assert_eq!(p.to_comment(), MARK);
        let c = SectionProps {
            page_margin: Some([36.0, 72.0, 36.0, 72.0]),
            continuous: true,
            columns: Some(3),
            ..Default::default()
        };
        let back = SectionProps::from_comment(&c.to_comment()).unwrap();
        assert_eq!(back.columns, Some(3));
        assert!(back.continuous);
        assert_eq!(back.page_margin.map(|m| m.map(|v| v.round())), Some([36.0, 72.0, 36.0, 72.0]));
        assert_eq!(SectionProps::default().to_comment(), "<!-- kladd: avsnitt -->");
    }

    #[test]
    fn other_comments_are_not_sections() {
        assert!(SectionProps::from_comment("<!-- kladd: toc -->").is_none());
        assert!(SectionProps::from_comment("<!-- kladd: avsnittx -->").is_none());
        assert!(SectionProps::from_comment("<!-- vanlig kommentar -->").is_none());
        assert!(SectionProps::from_comment(COLUMN_BREAK_LINE).is_none());
        assert!(is_column_break(&format!("  {COLUMN_BREAK_LINE}  ")));
        // En avsnittsrad är aldrig dokumentets grundformat.
        assert!(DocDefaults::from_comment(MARK).is_none());
        assert!(DocDefaults::from_comment(COLUMN_BREAK_LINE).is_none());
    }

    #[test]
    fn scan_inherits_defaults_and_splits() {
        let text = format!("<!-- kladd: page-size: A5; page-margin: 20mm -->\n\nEtt\n\n{MARK}\n\nTvå\n\n<!-- kladd: avsnitt -->\n\nTre\n");
        let a = markup::analyze(&text);
        let secs = &a.sections;
        assert_eq!(secs.len(), 3);
        assert!(secs[0].marker.is_none());
        assert!((secs[0].page.0 - 419.53).abs() < 0.1);
        assert_eq!(secs[0].columns, 1);
        // Andra avsnittet: liggande A4, egna spalter, marginalerna ärvs.
        assert!((secs[1].page.0 - 841.89).abs() < 0.1);
        assert_eq!(secs[1].columns, 2);
        assert!((secs[1].margin[0] - 20.0 * 72.0 / 25.4).abs() < 0.1);
        // Tredje: ingenting angivet, så tillbaka till grundformatet (A5), inte föregående avsnitt.
        assert!((secs[2].page.0 - 419.53).abs() < 0.1);
        assert_eq!(secs[2].columns, 1);
        assert_eq!(&text[secs[1].marker.clone().unwrap()], MARK);
        assert_eq!(secs[1].start, secs[1].marker.as_ref().unwrap().start);
        assert_eq!(index_at(secs, text.find("Två").unwrap()), 1);
        assert_eq!(index_at(secs, text.find("Ett").unwrap()), 0);
        assert_eq!(index_at(secs, text.find("Tre").unwrap()), 2);
    }

    #[test]
    fn leading_marker_belongs_to_first_section() {
        let text = format!("<!-- kladd: font-size: 12pt -->\n\n{MARK}\n\nText\n");
        let a = markup::analyze(&text);
        assert_eq!(a.sections.len(), 1);
        assert_eq!(a.sections[0].columns, 2);
        assert_eq!(a.sections[0].start, 0);
        // Rader i kodstaket räknas inte.
        let code = format!("```\n{MARK}\n```\n");
        assert_eq!(scan(&code, &DocDefaults::default()).len(), 1);
    }

    #[test]
    fn hidden_in_analysis_and_not_text() {
        let text = format!("Före\n\n{MARK}\n\nEfter\n");
        let a = markup::analyze(&text);
        let at = text.find("<!-- kladd: avsnitt").unwrap();
        assert!(a.pieces.iter().filter(|p| p.range.start >= at && p.range.end <= at + MARK.len()).all(|p| p.markup));
        let (words, _) = markup::count(&text, &a);
        assert_eq!(words, 2);
    }

    #[test]
    fn insert_break_makes_own_paragraph() {
        let p = SectionProps { columns: Some(2), ..Default::default() };
        let e = insert_break("Hej", 3, &p);
        assert_eq!(e.text, "Hej\n\n<!-- kladd: avsnitt; columns: 2 -->\n\n");
        let e = insert_break("A\n\nB", 3, &p);
        assert_eq!(e.text, "A\n\n<!-- kladd: avsnitt; columns: 2 -->\n\nB");
        assert_eq!(&e.text[e.sel.start..], "B");
        let e = insert_column_break("A\nB", 2);
        assert_eq!(e.text, format!("A\n\n{COLUMN_BREAK_LINE}\n\nB"));
    }

    #[test]
    fn update_current_rewrites_or_adds_marker() {
        let d = DocDefaults::default();
        // Första avsnittet utan rad: en rad före all text.
        let e = update_current("Text\n", 2, &d, |p| p.columns = Some(2));
        assert_eq!(e.text, "<!-- kladd: avsnitt; columns: 2 -->\n\nText\n");
        assert_eq!(e.sel.start, 2 + "<!-- kladd: avsnitt; columns: 2 -->\n\n".len());
        // Efter grundformatraden.
        let t = "<!-- kladd: font-size: 12pt -->\n\nText\n";
        let e = update_current(t, t.len(), &d, |p| p.columns = Some(2));
        assert_eq!(e.text, "<!-- kladd: font-size: 12pt -->\n\n<!-- kladd: avsnitt; columns: 2 -->\n\nText\n");
        // Och tillbaka: raden försvinner.
        let e2 = update_current(&e.text, e.text.len(), &d, |p| p.columns = None);
        assert_eq!(e2.text, t);
        // Ett senare avsnitt skrivs om på plats och behåller sin brytning.
        let t = format!("A\n\n{MARK}\n\nB\n");
        let e = update_current(&t, t.len() - 1, &d, |p| p.columns = Some(3));
        assert!(e.text.contains("columns: 3"));
        assert!(e.text.contains("page: A4 landscape"));
        assert_eq!(scan(&e.text, &d).len(), 2);
        let e = update_current(&t, t.len() - 1, &d, |p| *p = SectionProps::default());
        assert!(e.text.contains("<!-- kladd: avsnitt -->"));
        assert_eq!(scan(&e.text, &d).len(), 2);
    }

    #[test]
    fn landscape_from_here() {
        let d = DocDefaults::default();
        // Mitt i texten: ett nytt avsnitt.
        let e = set_landscape_from("Ett\n\nTvå\n", 5, &d, true);
        let secs = scan(&e.text, &d);
        assert_eq!(secs.len(), 2);
        assert!(secs[1].landscape());
        assert!(!secs[0].landscape());
        // I början av dokumentet: första avsnittet blir liggande, ingen ny sida.
        let e = set_landscape_from("Ett\n", 0, &d, true);
        let secs = scan(&e.text, &d);
        assert_eq!(secs.len(), 1);
        assert!(secs[0].landscape());
        // Stående igen i ett senare avsnitt.
        let t = "Ett\n\n<!-- kladd: avsnitt; page: A4 landscape -->\n\nTvå\n";
        let e = set_landscape_from(t, t.len(), &d, false);
        let secs = scan(&e.text, &d);
        assert_eq!(secs.len(), 3);
        assert!(secs[1].landscape() && !secs[2].landscape());
    }

}
