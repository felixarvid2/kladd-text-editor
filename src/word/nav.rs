//! Rubriker för navigeringsfönstret och innehållsförteckningen. Ren logik utan UI.
//!
//! Innehållsförteckningen är raden `<!-- kladd: toc -->` (se docs/dev/dokumentlage.md, 1.7).

use std::ops::Range;

use super::markup::{Analysis, Block, DocDefaults, Edit};

/// Raden som ger en innehållsförteckning.
pub const TOC_LINE: &str = "<!-- kladd: toc -->";

/// Högsta rubriknivå som tas med i innehållsförteckningen (H1–H3).
pub const TOC_MAX_LEVEL: u8 = 3;

/// En rubrik i dokumentet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1..=6.
    pub level: u8,
    /// Den synliga texten (utan Markdown-syntax).
    pub text: String,
    /// Byteoffset där rubrikraden börjar.
    pub start: usize,
}

/// Rubrikerna i ett dokument, från den färdiga analysen (dokumentläget).
pub fn headings(text: &str, a: &Analysis) -> Vec<Heading> {
    let mut out: Vec<Heading> = Vec::new();
    // Pågående rubrik: den slutar vid en radbrytning eller när biten inte är en rubrik.
    let mut open = false;
    for pc in &a.pieces {
        let Block::Heading(level) = pc.block else {
            open = false;
            continue;
        };
        let body = &text[pc.range.clone()];
        if !open {
            out.push(Heading { level, text: String::new(), start: pc.range.start });
            open = true;
        }
        let h = out.last_mut().expect("just pushed");
        if !pc.markup {
            h.text.push_str(body.trim_end_matches(['\n', '\r']));
        }
        if body.ends_with('\n') {
            open = false;
        }
    }
    for h in &mut out {
        let t = h.text.trim();
        if t.len() != h.text.len() {
            h.text = t.to_owned();
        }
    }
    out
}

/// Rubrikerna i en vanlig Markdown-text (textläget): en billig radskanning utan analys.
/// ATX-rubriker (`# Rubrik`) utanför kodstaket.
pub fn headings_scan(text: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    let mut off = 0usize;
    for line in text.split_inclusive('\n') {
        let start = off;
        off += line.len();
        let t = line.trim_end_matches(['\n', '\r']);
        let lead = t.trim_start_matches(' ');
        if t.len() - lead.len() > 3 {
            continue;
        }
        if let Some(c) = lead.chars().next().filter(|c| matches!(c, '`' | '~')) {
            if lead.starts_with(&c.to_string().repeat(3)) {
                match fence {
                    None => fence = Some(c),
                    Some(f) if f == c => fence = None,
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_some() || !lead.starts_with('#') {
            continue;
        }
        let hashes = lead.bytes().take_while(|b| *b == b'#').count();
        let rest = &lead[hashes..];
        if hashes > 6 || !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
            continue;
        }
        let rest = rest.trim();
        // Avslutande ##.
        let core = rest.trim_end_matches('#');
        let rest = if core.len() < rest.len() && (core.is_empty() || core.ends_with([' ', '\t'])) { core.trim() } else { rest };
        let text = rest.replace("**", "").replace('`', "");
        out.push(Heading { level: hashes as u8, text, start });
    }
    out
}

/// Index i `h` för rubriken som innehåller markören (den sista som börjar vid eller före den).
pub fn current(h: &[Heading], caret: usize) -> Option<usize> {
    match h.partition_point(|x| x.start <= caret) {
        0 => None,
        n => Some(n - 1),
    }
}

/// Rubrikerna som visas i trädet, som index i `h`. Med filter visas alla träffar (oavsett
/// hopfällning); annars hoppas underrubriker till hopfällda rubriker över.
pub fn visible(h: &[Heading], filter: &str, collapsed: &dyn Fn(&Heading) -> bool) -> Vec<usize> {
    let q = filter.trim().to_lowercase();
    if !q.is_empty() {
        return (0..h.len()).filter(|&i| h[i].text.to_lowercase().contains(&q)).collect();
    }
    let mut out = Vec::with_capacity(h.len());
    let mut skip_from: Option<u8> = None;
    for (i, x) in h.iter().enumerate() {
        if let Some(l) = skip_from {
            if x.level > l {
                continue;
            }
            skip_from = None;
        }
        out.push(i);
        if collapsed(x) && h.get(i + 1).is_some_and(|n| n.level > x.level) {
            skip_from = Some(x.level);
        }
    }
    out
}

/// Har rubrik `i` underrubriker (så att den kan fällas ihop)?
pub fn has_children(h: &[Heading], i: usize) -> bool {
    h.get(i + 1).is_some_and(|n| n.level > h[i].level)
}

// ------------------------------------------------------------------ innehållsförteckning

/// Byteintervallen (utan radbrytning) för alla rader som är innehållsförteckningar.
pub fn toc_lines(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if !text.contains("kladd: toc") {
        return out;
    }
    let mut off = 0usize;
    for line in text.split_inclusive('\n') {
        let start = off;
        off += line.len();
        let t = line.trim_end_matches(['\n', '\r']);
        if t.trim() == TOC_LINE {
            out.push(start..start + t.len());
        }
    }
    out
}

/// Sätter in en innehållsförteckning vid `at` som ett eget stycke (tomma rader runt om).
/// Markören hamnar efter den, så att förteckningen syns direkt.
pub fn insert_toc(text: &str, at: usize) -> Edit {
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
    let mut t = String::with_capacity(text.len() + TOC_LINE.len() + 4);
    t.push_str(before);
    t.push_str(lead);
    t.push_str(TOC_LINE);
    t.push_str(&trail);
    let toc_end = t.len() - trail.len();
    t.push_str(after);
    let caret = (toc_end + 2).min(t.len());
    Edit { text: t, sel: caret..caret }
}

/// Mått för en innehållsförteckning vid zoom `z`: (titelns höjd, radhöjd, luft under).
pub fn toc_metrics(z: f32, d: &DocDefaults) -> (f32, f32, f32) {
    let px = |pt: f32| pt * 96.0 / 72.0 * z;
    (px(18.0) * 1.3 + px(8.0), px(d.size()) * 1.5, px(10.0))
}

/// Rubrikerna som tas med i innehållsförteckningen.
pub fn toc_entries(h: &[Heading]) -> Vec<&Heading> {
    h.iter().filter(|x| x.level <= TOC_MAX_LEVEL).collect()
}

/// Antal rader (poster) som visas och förteckningens höjd i px. En förteckning är en enda
/// rad i layouten och får inte bli högre än en sida: längre listor kortas med en sista rad "…".
pub fn toc_size(z: f32, d: &DocDefaults, entries: usize, page_text_h: f32) -> (usize, f32) {
    let (title, row, gap) = toc_metrics(z, d);
    let fit = (((page_text_h - title - gap) / row).floor().max(1.0)) as usize;
    // För många poster: den sista raden blir "…". Inga rubriker: en rad med hjälptext.
    let (shown, rows) = if entries > fit { (fit.saturating_sub(1), fit) } else { (entries, entries.max(1)) };
    (shown, title + rows as f32 * row + gap)
}

/// Samma som `toc_size` men med dokumentets egna sidmått.
pub fn toc_size_for(z: f32, d: &DocDefaults, entries: usize) -> (usize, f32) {
    toc_size(z, d, entries, super::pages::Geom::new(z, d).text_h())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::markup;

    const DOC: &str = "# Ett\n\nText\n\n## Två **fet**\n\n```\n# inte rubrik\n```\n\n### Tre\n\n# Fyra ##\n";

    #[test]
    fn doc_headings_from_analysis() {
        let a = markup::analyze(DOC);
        let h = headings(DOC, &a);
        let got: Vec<(u8, &str)> = h.iter().map(|x| (x.level, x.text.as_str())).collect();
        assert_eq!(got, vec![(1, "Ett"), (2, "Två fet"), (3, "Tre"), (1, "Fyra")]);
        assert_eq!(h[0].start, 0);
        assert_eq!(&DOC[h[1].start..h[1].start + 2], "##");
    }

    #[test]
    fn scan_equals_analysis_for_atx() {
        let a = markup::analyze(DOC);
        assert_eq!(headings(DOC, &a), headings_scan(DOC));
    }

    #[test]
    fn scan_skips_fences_and_non_headings() {
        let t = "#nej\n####### sju\n```\n# kod\n```\n~~~\n# kod\n~~~\n    # indragen\n#\n# ok";
        let h = headings_scan(t);
        let got: Vec<(u8, &str)> = h.iter().map(|x| (x.level, x.text.as_str())).collect();
        assert_eq!(got, vec![(1, ""), (1, "ok")]);
    }

    #[test]
    fn current_heading_follows_caret() {
        let h = headings_scan(DOC);
        assert_eq!(current(&h, 0), Some(0));
        assert_eq!(current(&h, DOC.find("Text").unwrap()), Some(0));
        assert_eq!(current(&h, DOC.find("Två").unwrap()), Some(1));
        assert_eq!(current(&h, DOC.len()), Some(3));
        assert_eq!(current(&headings_scan("text\n# A"), 1), None);
    }

    #[test]
    fn filter_and_collapse() {
        let h = headings_scan(DOC);
        let none = |_: &Heading| false;
        assert_eq!(visible(&h, "", &none), vec![0, 1, 2, 3]);
        assert_eq!(visible(&h, "  TRE", &none), vec![2]);
        assert_eq!(visible(&h, "e", &none), vec![0, 1, 2]);
        // H1 "Ett" fälld döljer H2 och H3 men inte nästa H1.
        let ett = |x: &Heading| x.text == "Ett";
        assert_eq!(visible(&h, "", &ett), vec![0, 3]);
        // Filter ignorerar hopfällning.
        assert_eq!(visible(&h, "tre", &ett), vec![2]);
        assert!(has_children(&h, 0) && !has_children(&h, 2) && !has_children(&h, 3));
    }

    #[test]
    fn insert_toc_makes_own_paragraph() {
        let e = insert_toc("Hej", 3);
        assert_eq!(e.text, format!("Hej\n\n{TOC_LINE}\n\n"));
        assert_eq!(e.sel, e.text.len()..e.text.len());
        let e = insert_toc("A\n\nB", 3);
        assert_eq!(e.text, format!("A\n\n{TOC_LINE}\n\nB"));
        assert_eq!(&e.text[e.sel.start..], "B");
        let e = insert_toc("A\nB", 2);
        assert_eq!(e.text, format!("A\n\n{TOC_LINE}\n\nB"));
        let e = insert_toc("", 0);
        assert_eq!(e.text, format!("{TOC_LINE}\n\n"));
        assert_eq!(toc_lines(&format!("a\n\n{TOC_LINE}\n\nb")).len(), 1);
    }

    #[test]
    fn toc_size_caps_at_one_page() {
        let d = DocDefaults::default();
        let (shown, h) = toc_size(1.0, &d, 3, 900.0);
        assert_eq!(shown, 3);
        let (title, row, gap) = toc_metrics(1.0, &d);
        assert!((h - (title + 3.0 * row + gap)).abs() < 0.01);
        let (shown, h) = toc_size(1.0, &d, 500, 900.0);
        assert!(shown < 500 && h <= 900.0);
        let (shown, _) = toc_size(1.0, &d, 0, 900.0);
        assert_eq!(shown, 0);
    }
}
