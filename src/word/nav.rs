//! Rubriker för navigeringsfönstret och innehållsförteckningen. Ren logik utan UI.
//!
//! Innehållsförteckningen är raden `<!-- kladd: toc -->` (se docs/dev/dokumentlage.md, 1.7).

use std::ops::Range;

use super::markup::{Analysis, Block, DocDefaults, Edit};

/// Raden som ger en innehållsförteckning.
pub const TOC_LINE: &str = "<!-- kladd: toc -->";

/// Högsta rubriknivå som tas med i innehållsförteckningen (H1–H3).
pub const TOC_MAX_LEVEL: u8 = 3;

/// Vad en förteckning listar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TocKind {
    /// Rubrikerna (Words `TOC \o "1-3"`).
    #[default]
    Headings,
    /// Bildtexter som börjar med "Figur N" (Words `TOC \c "Figur"`).
    Figures,
    /// Bildtexter som börjar med "Tabell N" (Words `TOC \c "Tabell"`).
    Tables,
}

/// Raden `<!-- kladd: toc; levels: 1-3; kind: figures -->` som värden. Utan tillägg: rubriknivå 1–3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TocSpec {
    pub min: u8,
    pub max: u8,
    pub kind: TocKind,
}

impl Default for TocSpec {
    fn default() -> Self {
        Self { min: 1, max: TOC_MAX_LEVEL, kind: TocKind::Headings }
    }
}

impl TocSpec {
    /// Förteckningens rad: standardvalet skrivs som den korta `TOC_LINE`.
    pub fn line(&self) -> String {
        if *self == Self::default() {
            return TOC_LINE.to_owned();
        }
        let mut parts = vec!["kladd: toc".to_owned()];
        match self.kind {
            TocKind::Headings => {
                if (self.min, self.max) != (1, TOC_MAX_LEVEL) {
                    parts.push(if self.min == self.max { format!("levels: {}", self.min) } else { format!("levels: {}-{}", self.min, self.max) });
                }
            }
            TocKind::Figures => parts.push("kind: figures".to_owned()),
            TocKind::Tables => parts.push("kind: tables".to_owned()),
        }
        format!("<!-- {} -->", parts.join("; "))
    }

    /// Words `TOC`-instruktion för förteckningen.
    pub fn field_code(&self) -> String {
        match self.kind {
            TocKind::Headings => format!(" TOC \\o \"{}-{}\" \\h \\z \\u ", self.min, self.max),
            TocKind::Figures => " TOC \\h \\z \\c \"Figur\" ".to_owned(),
            TocKind::Tables => " TOC \\h \\z \\c \"Tabell\" ".to_owned(),
        }
    }

    /// Rubrik ovanför förteckningen.
    pub fn title(&self) -> &'static str {
        match self.kind {
            TocKind::Headings => "Innehåll",
            TocKind::Figures => "Figurförteckning",
            TocKind::Tables => "Tabellförteckning",
        }
    }

    /// Ur Words `TOC`-instruktion (` TOC \o "2-4" \h \z \u `). Okända omkopplare ignoreras; utan `\o` och `\c`
    /// gäller standardnivåerna (Words `\u` och `\t` ger fler rubriker än Kladd kan välja).
    pub fn from_field_code(instr: &str) -> Self {
        let mut spec = Self::default();
        let quoted = |after: &str| -> Option<String> {
            let a = after.trim_start();
            let a = a.strip_prefix('"')?;
            Some(a[..a.find('"')?].to_owned())
        };
        if let Some(i) = instr.find("\\c") {
            if let Some(label) = quoted(&instr[i + 2..]) {
                let l = label.to_lowercase();
                spec.kind = if l.starts_with("tab") { TocKind::Tables } else { TocKind::Figures };
                return spec;
            }
        }
        if let Some(range) = instr.find("\\o").and_then(|i| quoted(&instr[i + 2..])) {
            if let Some((a, b)) = parse_levels(&range) {
                (spec.min, spec.max) = (a, b);
            }
        }
        spec
    }
}

/// `1-3`, `2` eller `2–4` som nivåintervall (1..=6, stigande).
fn parse_levels(s: &str) -> Option<(u8, u8)> {
    let s = s.trim();
    let (a, b) = match s.split_once(['-', '–', ',']) {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (s, s),
    };
    let (a, b) = (a.parse::<u8>().ok()?, b.parse::<u8>().ok()?);
    ((1..=6).contains(&a) && (1..=6).contains(&b) && a <= b).then_some((a, b))
}

/// Tolkar en förteckningsrad: `<!-- kladd: toc -->` med valfria `; levels: A-B` och `; kind: figures|tables`.
pub fn parse_toc(line: &str) -> Option<TocSpec> {
    let inner = line.trim().strip_prefix("<!--")?.strip_suffix("-->")?;
    let mut parts = inner.split(';').map(str::trim);
    if parts.next()? != "kladd: toc" {
        return None;
    }
    let mut spec = TocSpec::default();
    for part in parts {
        match part.split_once(':').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("levels", v)) => {
                if let Some((a, b)) = parse_levels(v) {
                    (spec.min, spec.max) = (a, b);
                }
            }
            Some(("kind", "figures")) => spec.kind = TocKind::Figures,
            Some(("kind", "tables")) => spec.kind = TocKind::Tables,
            _ => {}
        }
    }
    Some(spec)
}

/// Är raden en innehållsförteckning (med eller utan nivåval)?
pub fn is_toc_line(line: &str) -> bool {
    line.contains("kladd: toc") && parse_toc(line).is_some()
}

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
    headings_in(text, &a.pieces)
}

/// Som `headings`, för en följd av bitar (som börjar vid ett styckes början, se `layout::Cache`).
pub fn headings_in(text: &str, pieces: &[super::markup::Piece]) -> Vec<Heading> {
    let mut out: Vec<Heading> = Vec::new();
    // Pågående rubrik: den slutar vid en radbrytning eller när biten inte är en rubrik.
    let mut open = false;
    for pc in pieces {
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
        // Titel och Underrubrik är inte med i innehållet (som i Words navigeringsfönster).
        if crate::style::titles::trailing_marker(rest).is_some() {
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
#[cfg(test)]
pub fn toc_lines(text: &str) -> Vec<Range<usize>> {
    toc_specs(text).into_iter().map(|(r, _)| r).collect()
}

/// Som `toc_lines`, med varje förteckningens val av nivåer och slag.
pub fn toc_specs(text: &str) -> Vec<(Range<usize>, TocSpec)> {
    let mut out = Vec::new();
    if !text.contains("kladd: toc") {
        return out;
    }
    let mut off = 0usize;
    for line in text.split_inclusive('\n') {
        let start = off;
        off += line.len();
        let t = line.trim_end_matches(['\n', '\r']);
        if let Some(spec) = parse_toc(t) {
            out.push((start..start + t.len(), spec));
        }
    }
    out
}

/// Sätter in en innehållsförteckning vid `at` som ett eget stycke (tomma rader runt om).
/// Markören hamnar efter den, så att förteckningen syns direkt.
#[cfg(test)]
pub fn insert_toc(text: &str, at: usize) -> Edit {
    insert_toc_spec(text, at, &TocSpec::default())
}

/// Som `insert_toc`, med val av nivåer eller slag.
pub fn insert_toc_spec(text: &str, at: usize, spec: &TocSpec) -> Edit {
    let line = spec.line();
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
    t.push_str(&line);
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
/// Det i bitarna `pieces` (ett fönster mellan stycken i `text`) som kan komma med i en innehållsförteckning: antalet
/// rubriker per nivå (1–9) och antalet bildtexter för figurer och tabeller. Förteckningens höjd beror bara på antalet
/// poster, och så länge detta är oförändrat i fönstret är antalet poster oförändrat för alla förteckningar (vilka nivåer
/// och slag de än har).
pub fn toc_sig_in(text: &str, pieces: &[super::markup::Piece]) -> [usize; 11] {
    let mut sig = [0usize; 11];
    let mut open = false;
    for pc in pieces {
        let Block::Heading(level) = pc.block else {
            open = false;
            continue;
        };
        if !open {
            sig[(level as usize).clamp(1, 9) - 1] += 1;
            open = true;
        }
        if text.get(pc.range.clone()).is_some_and(|b| b.ends_with('\n')) {
            open = false;
        }
    }
    if let (Some(first), Some(last)) = (pieces.first(), pieces.last()) {
        if let Some(w) = text.get(first.range.start..last.range.end.min(text.len())) {
            sig[9] = captions(w, TocKind::Figures).len();
            sig[10] = captions(w, TocKind::Tables).len();
        }
    }
    sig
}

/// Hur mycket det som kan komma med i en innehållsförteckning skiljer mellan två fönster (`toc_sig_in`); 0 = inget.
pub fn toc_sig_diff(a: &[usize; 11], b: &[usize; 11]) -> isize {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).sum::<usize>() as isize
}

#[cfg(test)]
pub fn toc_entries(h: &[Heading]) -> Vec<&Heading> {
    h.iter().filter(|x| x.level <= TOC_MAX_LEVEL).collect()
}

/// Posterna i en förteckning med val `spec`: rubriker i nivåintervallet, eller bildtexter ("Figur 1: …")
/// ur `text` (nivå 1; `start` är styckets byteposition).
pub fn toc_items(text: &str, h: &[Heading], spec: &TocSpec) -> Vec<Heading> {
    match spec.kind {
        TocKind::Headings => h.iter().filter(|x| (spec.min..=spec.max).contains(&x.level)).cloned().collect(),
        kind => captions(text, kind),
    }
}

/// Är texten (utan Markdown-tecken) en bildtext: etikett och tal ("Figur 3: …", "Tabell 2. …")?
pub fn caption_kind(plain: &str) -> Option<TocKind> {
    const FIGURE: [&str; 3] = ["figur", "figure", "bild"];
    const TABLE: [&str; 2] = ["tabell", "table"];
    let low = plain.trim_start().to_lowercase();
    let numbered = |labels: &[&str]| {
        labels.iter().any(|l| {
            low.strip_prefix(l).and_then(|r| r.strip_prefix(' ')).is_some_and(|r| r.trim_start().starts_with(|c: char| c.is_ascii_digit()))
        })
    };
    if numbered(&FIGURE) {
        Some(TocKind::Figures)
    } else if numbered(&TABLE) {
        Some(TocKind::Tables)
    } else {
        None
    }
}

/// Antalet poster i alla förteckningar i dokumentet (för att veta när layouten måste göras om).
pub fn toc_total(text: &str, h: &[Heading]) -> usize {
    // Räknas utan att kopiera rubrikerna (ett dokument kan ha många förteckningar och tusentals rubriker).
    toc_specs(text)
        .iter()
        .map(|(_, s)| match s.kind {
            TocKind::Headings => h.iter().filter(|x| (s.min..=s.max).contains(&x.level)).count(),
            _ => toc_items(text, h, s).len(),
        })
        .sum()
}

/// Stycken som är bildtexter av slaget `kind` (se `caption_kind`). Kodblock hoppas över.
pub fn captions(text: &str, kind: TocKind) -> Vec<Heading> {
    let mut out = Vec::new();
    let low = text.to_lowercase();
    let possible = match kind {
        TocKind::Headings => false,
        TocKind::Figures => low.contains("figur") || low.contains("bild"),
        TocKind::Tables => low.contains("tabell") || low.contains("table"),
    };
    if !possible {
        return out;
    }
    let mut fence: Option<char> = None;
    let mut off = 0usize;
    for line in text.split_inclusive('\n') {
        let start = off;
        off += line.len();
        let t = line.trim_end_matches(['\n', '\r']);
        let lead = t.trim_start_matches(' ');
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
        if fence.is_some() || t.len() - lead.len() > 3 || lead.starts_with(['#', '|', '>', '<']) {
            continue;
        }
        // Fetstil eller kursiv runt etiketten ("**Figur 1:** text") hör till bildtexten.
        let plain = lead.replace(['*', '_'], "");
        if caption_kind(&plain) == Some(kind) {
            out.push(Heading { level: 1, text: plain.trim().to_owned(), start });
        }
    }
    out
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
    fn toc_line_parses_levels_and_kind() {
        assert_eq!(parse_toc(TOC_LINE), Some(TocSpec::default()));
        let s = parse_toc("<!-- kladd: toc; levels: 2-4 -->").unwrap();
        assert_eq!((s.min, s.max, s.kind), (2, 4, TocKind::Headings));
        assert_eq!(parse_toc("<!-- kladd: toc; levels: 2 -->").map(|s| (s.min, s.max)), Some((2, 2)));
        assert_eq!(parse_toc("<!-- kladd: toc; kind: tables -->").map(|s| s.kind), Some(TocKind::Tables));
        // Ogiltigt nivåval ger standardnivåerna; andra rader är inga förteckningar.
        assert_eq!(parse_toc("<!-- kladd: toc; levels: 5-2 -->"), Some(TocSpec::default()));
        assert!(parse_toc("<!-- kladd: tocx -->").is_none() && parse_toc("<!-- kladd: font-size: 12pt -->").is_none() && parse_toc("text").is_none());
        // Raden skrivs tillbaka som den lästes.
        for l in [TOC_LINE, "<!-- kladd: toc; levels: 2-4 -->", "<!-- kladd: toc; levels: 2 -->", "<!-- kladd: toc; kind: figures -->"] {
            assert_eq!(parse_toc(l).unwrap().line(), l);
        }
        assert_eq!(TocSpec { min: 1, max: 3, kind: TocKind::Headings }.line(), TOC_LINE);
    }

    #[test]
    fn toc_field_codes_roundtrip_levels() {
        for (min, max) in [(1, 3), (2, 4), (1, 6)] {
            let s = TocSpec { min, max, kind: TocKind::Headings };
            assert_eq!(TocSpec::from_field_code(&s.field_code()), s);
        }
        assert_eq!(TocSpec::from_field_code(r#" TOC \o "1-3" \h \z \u "#), TocSpec::default());
        assert_eq!(TocSpec::from_field_code(r#" TOC \h \z \c "Figur" "#).kind, TocKind::Figures);
        assert_eq!(TocSpec::from_field_code(r#" TOC \h \z \c "Tabell" "#).kind, TocKind::Tables);
        assert_eq!(TocSpec::from_field_code(" TOC "), TocSpec::default());
    }

    #[test]
    fn toc_items_by_level_and_captions() {
        let t = "# Ett\n\n## Två\n\n### Tre\n\n#### Fyra\n\nFigur 1: Bild\n\n```\nFigur 9: i kod\n```\n\n**Tabell 2.** Siffror\n\nFigurer är bra.\n\n## Fig 3\n";
        let h = headings_scan(t);
        let lv = |min, max| toc_items(t, &h, &TocSpec { min, max, kind: TocKind::Headings }).iter().map(|x| x.text.clone()).collect::<Vec<_>>();
        assert_eq!(lv(1, 3), ["Ett", "Två", "Tre", "Fig 3"]);
        assert_eq!(lv(2, 4), ["Två", "Tre", "Fyra", "Fig 3"]);
        assert_eq!(lv(2, 2), ["Två", "Fig 3"]);
        let fig = toc_items(t, &h, &TocSpec { kind: TocKind::Figures, ..Default::default() });
        assert_eq!(fig.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["Figur 1: Bild"]);
        assert_eq!(&t[fig[0].start..fig[0].start + 5], "Figur");
        let tab = toc_items(t, &h, &TocSpec { kind: TocKind::Tables, ..Default::default() });
        assert_eq!(tab.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["Tabell 2. Siffror"]);
        // Två förteckningar: antalet poster summeras (layoutminnet jämför det).
        let two = format!("{t}\n<!-- kladd: toc -->\n\n<!-- kladd: toc; kind: figures -->\n");
        assert_eq!(toc_total(&two, &headings_scan(&two)), 4 + 1);
    }

    #[test]
    fn insert_toc_with_levels_writes_the_line() {
        let e = insert_toc_spec("Hej", 3, &TocSpec { min: 1, max: 2, kind: TocKind::Headings });
        assert_eq!(e.text, "Hej\n\n<!-- kladd: toc; levels: 1-2 -->\n\n");
        assert_eq!(toc_specs(&e.text).len(), 1);
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
