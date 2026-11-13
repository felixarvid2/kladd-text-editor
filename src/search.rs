//! Sökmotorn bakom sök och ersätt: alternativ (versaler, hela ord, reguljära uttryck),
//! sökning i dokumentlägets SYNLIGA text och sökning/ersättning med formatering.
//!
//! Ren logik utan gränssnitt. Textläget söker direkt i källtexten. Dokumentläget bygger en
//! synlig text av analysens bitar (ingen markup, `\n` mellan stycken) med en karta tillbaka till
//! källans byteintervall, så att "fet text" hittar `**fet** text` och träffar aldrig hamnar
//! inuti markup.
use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::highlight::find_all;
use crate::i18n::t;
use crate::tf;
use crate::word::markup::{self, Analysis, Block, Inline, SpanProp};

/// Formatfilter för dokumentläget: en träff räknas bara om varje synligt tecken i den har
/// all vald formatering.
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct FormatFilter {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Valfri bakgrundsmarkering.
    pub highlight: bool,
    pub color: Option<[u8; 3]>,
    /// Rubriknivå 1–3.
    pub heading: Option<u8>,
}

impl FormatFilter {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Kort beskrivning till menyknappen, t.ex. "Fetstil, Kursiv".
    pub fn label(&self) -> String {
        let mut v = Vec::new();
        if self.bold {
            v.push(t("Fetstil").to_owned());
        }
        if self.italic {
            v.push(t("Kursiv").to_owned());
        }
        if self.underline {
            v.push(t("Understruken").to_owned());
        }
        if self.strike {
            v.push(t("Genomstruken").to_owned());
        }
        if self.highlight {
            v.push(t("Markerad").to_owned());
        }
        if self.color.is_some() {
            v.push(t("Textfärg").to_owned());
        }
        if let Some(n) = self.heading {
            v.push(tf!("Rubrik {n}", n = n));
        }
        if v.is_empty() { t("Format").to_owned() } else { v.join(", ") }
    }

    fn accepts(&self, s: &Sty) -> bool {
        (!self.bold || s.bold)
            && (!self.italic || s.italic)
            && (!self.underline || s.underline)
            && (!self.strike || s.strike)
            && (!self.highlight || s.bg.is_some())
            && (self.color.is_none() || self.color == s.color)
            && (self.heading.is_none() || self.heading == s.heading)
    }
}

/// Allt som styr en sökning.
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct Spec {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Bara i dokumentläget.
    pub fmt: FormatFilter,
}

/// En träff. `start`/`end` är källans byteintervall (från första till sista synliga tecknet);
/// `segs` är de synliga delarna i källan (utan markup mellan dem).
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    pub segs: Vec<Range<usize>>,
    /// Intervallet i den synliga texten (för ersättning med fångstgrupper).
    vis: Range<usize>,
}

#[derive(Default, Debug)]
pub struct Results {
    pub matches: Vec<Match>,
    /// Ogiltigt reguljärt uttryck: felet, och då finns inga träffar.
    pub error: Option<String>,
    /// Sökningen är inte klar eller avbröts vid taket (stora filer): träffarna är de första.
    pub partial: bool,
}

/// Formatering som "Ersätt" kan lägga på träffen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Apply {
    Bold,
    Italic,
    Underline,
    Color([u8; 3]),
    Highlight([u8; 3]),
    Clear,
}

// ============================================================== motor

enum Engine {
    None,
    /// Vanlig text: samma jämförelse som tidigare (`find_all`).
    Plain(String, bool),
    Re(Regex),
}

fn engine(spec: &Spec) -> Result<Engine, String> {
    if spec.query.is_empty() {
        return Ok(Engine::None);
    }
    if !spec.regex && !spec.whole_word {
        return Ok(Engine::Plain(spec.query.clone(), spec.case_sensitive));
    }
    let mut pat = if spec.regex { spec.query.clone() } else { regex::escape(&spec.query) };
    if spec.whole_word {
        pat = format!(r"\b(?:{pat})\b");
    }
    RegexBuilder::new(&pat)
        .case_insensitive(!spec.case_sensitive)
        .multi_line(true)
        .size_limit(10 << 20)
        .build()
        .map(Engine::Re)
        .map_err(|e| {
            let s = e.to_string();
            // Sista raden i regex-felet är den läsbara förklaringen.
            s.lines().last().unwrap_or(&s).trim_start_matches("error: ").to_owned()
        })
}

impl Engine {
    /// Träffar i `hay` (tomma träffar hoppas över).
    fn find(&self, hay: &str) -> Vec<Range<usize>> {
        match self {
            Engine::None => Vec::new(),
            Engine::Plain(n, case) => find_all(hay, n, *case).into_iter().map(|(a, b)| a..b).collect(),
            Engine::Re(re) => re.find_iter(hay).filter(|m| !m.is_empty()).map(|m| m.range()).collect(),
        }
    }

    /// Ersättningstexten för träffen `r` i `hay`: `$1`/`${namn}` i regexläge, annars bokstavlig.
    fn expand(&self, hay: &str, r: &Range<usize>, repl: &str) -> String {
        if let Engine::Re(re) = self {
            if let Some(c) = re.captures_at(hay, r.start) {
                if c.get(0).is_some_and(|m| m.range() == *r) {
                    let mut out = String::new();
                    c.expand(repl, &mut out);
                    return out;
                }
            }
        }
        repl.to_owned()
    }
}

// ============================================================== synlig text

#[derive(Clone, Copy, PartialEq, Debug)]
struct Sty {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    heading: Option<u8>,
    color: Option<[u8; 3]>,
    bg: Option<[u8; 3]>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Text,
    /// Radbrytning i innehållet (kan ersättas/raderas).
    Newline,
    /// Tecken utan motsvarighet i källan (styckebrytning vid markup, tabellcellens skiljetecken).
    Virtual,
}

#[derive(Clone, Debug)]
struct Seg {
    vis: Range<usize>,
    src: Range<usize>,
    kind: Kind,
    sty: Sty,
}

struct Visible {
    text: String,
    segs: Vec<Seg>,
}

impl Visible {
    fn build(text: &str, a: &Analysis) -> Visible {
        let plain = Sty { bold: false, italic: false, underline: false, strike: false, heading: None, color: None, bg: None };
        let mut v = Visible { text: String::with_capacity(text.len()), segs: Vec::new() };
        for p in &a.pieces {
            if p.image.is_some() || p.object.is_some() {
                continue;
            }
            let src = &text[p.range.clone()];
            if p.markup || p.list_marker {
                // Markup syns inte; radbrytningar i den är styckegränser och `|` skiljer tabellceller.
                for (i, c) in src.char_indices() {
                    let at = p.range.start + i;
                    if c == '\n' {
                        v.push_virtual('\n', at, plain);
                    } else if c == '|' && p.block == Block::Table && !v.text.is_empty() && !v.text.ends_with('\n') {
                        v.push_virtual('\t', at, plain);
                    }
                }
                continue;
            }
            let sty = Sty {
                bold: p.bold,
                italic: p.italic,
                underline: p.underline,
                strike: p.strike,
                heading: match p.block {
                    Block::Heading(n) => Some(n),
                    _ => None,
                },
                color: p.span.as_ref().and_then(|s| s.color),
                bg: p.span.as_ref().and_then(|s| s.background),
            };
            let mut pos = p.range.start;
            for line in src.split_inclusive('\n') {
                let body = line.strip_suffix('\n').unwrap_or(line);
                if !body.is_empty() {
                    v.push_text(body, pos, Kind::Text, sty);
                }
                if body.len() != line.len() {
                    v.push_text("\n", pos + body.len(), Kind::Newline, sty);
                }
                pos += line.len();
            }
        }
        v
    }

    fn push_text(&mut self, s: &str, src_at: usize, kind: Kind, sty: Sty) {
        let a = self.text.len();
        self.text.push_str(s);
        self.segs.push(Seg { vis: a..self.text.len(), src: src_at..src_at + s.len(), kind, sty });
    }

    fn push_virtual(&mut self, c: char, src_at: usize, sty: Sty) {
        let a = self.text.len();
        self.text.push(c);
        self.segs.push(Seg { vis: a..self.text.len(), src: src_at..src_at, kind: Kind::Virtual, sty });
    }

    /// Segmenten som överlappar `r`.
    fn overlapping(&self, r: &Range<usize>) -> &[Seg] {
        let lo = self.segs.partition_point(|s| s.vis.end <= r.start);
        let hi = self.segs.partition_point(|s| s.vis.start < r.end);
        &self.segs[lo..hi.max(lo)]
    }

    /// Källans intervall för ett synligt intervall (utan virtuella tecken), hopslagna när de gränsar.
    fn source_ranges(&self, r: &Range<usize>) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for s in self.overlapping(r) {
            if s.kind == Kind::Virtual {
                continue;
            }
            let a = s.vis.start.max(r.start);
            let b = s.vis.end.min(r.end);
            if a >= b {
                continue;
            }
            let src = s.src.start + (a - s.vis.start)..s.src.start + (b - s.vis.start);
            match out.last_mut() {
                Some(l) if l.end == src.start => l.end = src.end,
                _ => out.push(src),
            }
        }
        out
    }

    /// Sant om varje textteckens format uppfyller filtret (radbrytningar och skiljetecken räknas inte).
    fn passes(&self, r: &Range<usize>, f: &FormatFilter) -> bool {
        let mut any = false;
        for s in self.overlapping(r) {
            if s.kind != Kind::Text {
                continue;
            }
            if !f.accepts(&s.sty) {
                return false;
            }
            any = true;
        }
        any
    }

    /// Alla sammanhängande löpningar (inom ett stycke/en cell) där filtret gäller.
    fn runs(&self, f: &FormatFilter) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut cur: Option<Range<usize>> = None;
        let mut flush = |cur: &mut Option<Range<usize>>, text: &str| {
            if let Some(r) = cur.take() {
                if !text[r.clone()].trim().is_empty() {
                    out.push(r);
                }
            }
        };
        for s in &self.segs {
            if s.kind == Kind::Text && f.accepts(&s.sty) {
                match &mut cur {
                    Some(r) => r.end = s.vis.end,
                    None => cur = Some(s.vis.clone()),
                }
            } else {
                flush(&mut cur, &self.text);
            }
        }
        flush(&mut cur, &self.text);
        out
    }
}

// ============================================================== sökning

fn plain_match(r: Range<usize>) -> Match {
    Match { start: r.start, end: r.end, segs: vec![r.clone()], vis: r }
}

/// Textläget: söker direkt i texten. Formatfiltret används inte.
pub fn search_text(text: &str, spec: &Spec) -> Results {
    match engine(spec) {
        Ok(e) => Results { matches: e.find(text).into_iter().map(plain_match).collect(), ..Default::default() },
        Err(err) => Results { error: Some(err), ..Default::default() },
    }
}

/// Förberedd textsökning för stora filer: söker i bitar (se `bigfiles`).
pub struct TextSearcher(Engine);

impl TextSearcher {
    pub fn new(spec: &Spec) -> Result<Self, String> {
        engine(spec).map(Self)
    }

    /// Träffar som börjar i `from..to`, sökta i `from..scan_to` (så att en träff får sluta efter `to`).
    pub fn find_chunk(&self, text: &str, from: usize, to: usize, scan_to: usize) -> Vec<Match> {
        let hay = &text[from..scan_to];
        self.0
            .find(hay)
            .into_iter()
            .filter(|r| from + r.start < to)
            .map(|r| plain_match(from + r.start..from + r.end))
            .collect()
    }
}

struct DocHits {
    vis: Visible,
    engine: Engine,
    matches: Vec<Match>,
}

fn doc_hits(text: &str, a: &Analysis, spec: &Spec) -> Result<DocHits, String> {
    let engine = engine(spec)?;
    let vis = Visible::build(text, a);
    let ranges: Vec<Range<usize>> = if spec.query.is_empty() {
        if spec.fmt.is_empty() { Vec::new() } else { vis.runs(&spec.fmt) }
    } else {
        let mut r = engine.find(&vis.text);
        if !spec.fmt.is_empty() {
            r.retain(|m| vis.passes(m, &spec.fmt));
        }
        r
    };
    let matches = ranges
        .into_iter()
        .filter_map(|r| {
            let segs = vis.source_ranges(&r);
            let (start, end) = (segs.first()?.start, segs.last()?.end);
            Some(Match { start, end, segs, vis: r })
        })
        .collect();
    Ok(DocHits { vis, engine, matches })
}

/// Dokumentläget: söker i den synliga texten.
pub fn search_doc(text: &str, a: &Analysis, spec: &Spec) -> Results {
    match doc_hits(text, a, spec) {
        Ok(h) => Results { matches: h.matches, ..Default::default() },
        Err(err) => Results { error: Some(err), ..Default::default() },
    }
}

/// Bakgrundsintervall för layouten: `(start, slut, är_aktuell)`, ett per synlig del av varje träff.
pub fn highlights(r: &Results, current: Option<usize>) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    for m in &r.matches {
        let cur = current == Some(m.start);
        for s in &m.segs {
            out.push((s.start, s.end, cur));
        }
    }
    out
}

// ============================================================== ersättning

/// Resultatet av en ersättning.
pub struct Replaced {
    pub text: String,
    /// Byte där markören hamnar (efter den första ersättningen).
    pub caret: usize,
    pub count: usize,
}

/// Textläget. `only` = källbyte där träffen som ska ersättas börjar; `None` ersätter alla.
pub fn replace_text(text: &str, spec: &Spec, repl: &str, only: Option<usize>) -> Option<Replaced> {
    let eng = engine(spec).ok()?;
    let hits: Vec<Range<usize>> = eng
        .find(text)
        .into_iter()
        .filter(|r| only.is_none_or(|o| r.start == o))
        .collect();
    if hits.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut caret = None;
    for r in &hits {
        out.push_str(&text[last..r.start]);
        out.push_str(&eng.expand(text, r, repl));
        caret.get_or_insert(out.len());
        last = r.end;
    }
    out.push_str(&text[last..]);
    Some(Replaced { text: out, caret: caret.unwrap_or(0), count: hits.len() })
}

/// Hur en dokumentlägesersättning går till.
pub struct DocReplace<'a> {
    pub repl: &'a str,
    /// Lämna texten orörd och bara formatera träffarna.
    pub keep_text: bool,
    pub apply: Option<Apply>,
    /// Som `replace_text`: bara träffen som börjar här.
    pub only: Option<usize>,
}

/// Dokumentläget: ersätter den synliga texten och behåller första teckenets formatering (via
/// källändringar på de synliga delarna), och lägger sedan på `apply`. Träffarna tas bakifrån så
/// att tidigare offset förblir giltiga. Returnerar ny text, markör och antal träffar.
pub fn replace_doc(text: &str, spec: &Spec, o: &DocReplace) -> Option<(markup::Edit, usize)> {
    let a = markup::analyze(text);
    let hits = doc_hits(text, &a, spec).ok()?;
    let chosen: Vec<&Match> = hits.matches.iter().filter(|m| o.only.is_none_or(|s| m.start == s)).collect();
    if chosen.is_empty() {
        return None;
    }
    let expanded: Vec<String> = chosen.iter().map(|m| hits.engine.expand(&hits.vis.text, &m.vis, o.repl)).collect();
    let mut t = text.to_owned();
    for (m, repl) in chosen.iter().zip(&expanded).rev() {
        let range = if o.keep_text {
            m.start..m.end
        } else {
            for (i, s) in m.segs.iter().enumerate().rev() {
                t.replace_range(s.clone(), if i == 0 { repl } else { "" });
            }
            m.start..m.start + repl.len()
        };
        if let Some(ap) = &o.apply {
            if !range.is_empty() {
                apply_format(&mut t, range, ap);
            }
        }
    }
    // Markören hamnar efter den första ersättningen, så att nästa sökning går vidare.
    let at = chosen[0].start + if o.keep_text { 0 } else { expanded[0].len() };
    Some((markup::Edit { text: t, sel: at..at }, chosen.len()))
}

fn apply_format(t: &mut String, r: Range<usize>, ap: &Apply) {
    let inline = |t: &mut String, m: Inline, want: bool| {
        let a = markup::analyze(t);
        let act = markup::active(t, &a, r.clone());
        let on = match m {
            Inline::Bold => act.bold,
            Inline::Italic => act.italic,
            Inline::Underline => act.underline,
            Inline::Strike => act.strike,
        };
        if on != want {
            *t = markup::toggle_inline(t, r.clone(), m).text;
        }
    };
    let prop = |t: &mut String, p: SpanProp| *t = markup::set_span_prop(t, r.clone(), p).text;
    match ap {
        Apply::Bold => inline(t, Inline::Bold, true),
        Apply::Italic => inline(t, Inline::Italic, true),
        Apply::Underline => inline(t, Inline::Underline, true),
        Apply::Color(c) => prop(t, SpanProp::Color(Some(*c))),
        Apply::Highlight(c) => prop(t, SpanProp::Background(Some(*c))),
        Apply::Clear => {
            for m in [Inline::Bold, Inline::Italic, Inline::Underline, Inline::Strike] {
                inline(t, m, false);
            }
            for p in [SpanProp::Color(None), SpanProp::Background(None), SpanProp::FontFamily(None), SpanProp::FontSize(None)] {
                prop(t, p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(q: &str) -> Spec {
        Spec { query: q.to_owned(), ..Default::default() }
    }

    fn doc(text: &str, s: &Spec) -> Results {
        search_doc(text, &markup::analyze(text), s)
    }

    fn found<'a>(text: &'a str, r: &Results) -> Vec<&'a str> {
        r.matches.iter().map(|m| &text[m.start..m.end]).collect()
    }

    fn rep(text: &str, s: &Spec, repl: &str) -> String {
        let o = DocReplace { repl, keep_text: false, apply: None, only: None };
        replace_doc(text, s, &o).map_or_else(|| text.to_owned(), |r| r.0.text)
    }

    #[test]
    fn text_mode_is_unchanged_for_plain_files() {
        let t = "Åsa åsa ASA\nasa";
        let r = search_text(t, &spec("åsa"));
        assert_eq!(r.matches.len(), 2);
        let mut s = spec("Åsa");
        s.case_sensitive = true;
        assert_eq!(search_text(t, &s).matches.len(), 1);
        assert!(search_text(t, &spec("")).matches.is_empty());
        let r = search_text("a.b axb", &spec("a.b"));
        assert_eq!(r.matches.len(), 1, "punkt är bokstavlig utan regex");
        assert_eq!((r.matches[0].start, r.matches[0].end), (0, 3));
    }

    #[test]
    fn whole_word_option() {
        let t = "cat concat cat.";
        let mut s = spec("cat");
        assert_eq!(search_text(t, &s).matches.len(), 3);
        s.whole_word = true;
        assert_eq!(search_text(t, &s).matches.len(), 2);
        // Specialtecken är bokstavliga även med hela ord.
        let mut s = spec("a+b");
        s.whole_word = true;
        assert_eq!(search_text("a+b ab", &s).matches.len(), 1);
        // Svenska bokstäver räknas som ordtecken.
        let mut s = spec("ö");
        s.whole_word = true;
        assert_eq!(search_text("ö bör ö", &s).matches.len(), 2);
    }

    #[test]
    fn regex_and_case_options() {
        let mut s = spec(r"\d+");
        s.regex = true;
        assert_eq!(found("a12 b3", &search_text("a12 b3", &s)), ["12", "3"]);
        let mut s = spec("ab+");
        s.regex = true;
        assert_eq!(search_text("AB abb", &s).matches.len(), 2);
        s.case_sensitive = true;
        assert_eq!(search_text("AB abb", &s).matches.len(), 1);
        // ^ och $ gäller per rad; tomma träffar hoppas över.
        let mut s = spec("^x");
        s.regex = true;
        assert_eq!(search_text("x\nx\ny", &s).matches.len(), 2);
        let mut s = spec("z*");
        s.regex = true;
        assert!(search_text("abc", &s).matches.is_empty());
    }

    #[test]
    fn invalid_regex_gives_error_and_no_matches() {
        let mut s = spec("(abc");
        s.regex = true;
        let r = search_text("(abc", &s);
        assert!(r.matches.is_empty());
        assert!(r.error.as_deref().is_some_and(|e| !e.is_empty()));
        assert!(replace_text("(abc", &s, "x", None).is_none());
        assert!(doc("(abc", &s).error.is_some());
    }

    #[test]
    fn captures_in_replacement() {
        let mut s = spec(r"(\w+)@(?P<dom>\w+)");
        s.regex = true;
        let r = replace_text("kalle@x anna@y", &s, "${dom}:$1", None).unwrap();
        assert_eq!(r.text, "x:kalle y:anna");
        assert_eq!(r.count, 2);
        // Bokstavlig ersättning utan regex: $ betyder inget.
        let r = replace_text("a b", &spec("a"), "$1", None).unwrap();
        assert_eq!(r.text, "$1 b");
        // Bara en träff.
        let r = replace_text("a a a", &spec("a"), "b", Some(2)).unwrap();
        assert_eq!(r.text, "a b a");
        assert_eq!(r.caret, 3);
    }

    #[test]
    fn visible_text_matches_across_markup() {
        let t = "Det är **fet** text och *kursiv*.\n";
        let r = doc(t, &spec("fet text"));
        assert_eq!(r.matches.len(), 1);
        let m = &r.matches[0];
        assert_eq!(&t[m.start..m.end], "fet** text");
        // Två synliga delar: "fet" och " text".
        let parts: Vec<&str> = m.segs.iter().map(|s| &t[s.clone()]).collect();
        assert_eq!(parts, ["fet", " text"]);
        // Markup kan aldrig hittas.
        assert!(doc(t, &spec("**")).matches.is_empty());
        assert!(doc(t, &spec("fet**")).matches.is_empty());
        assert_eq!(doc(t, &spec("kursiv")).matches.len(), 1);
    }

    #[test]
    fn matches_across_spans_links_and_paragraphs() {
        let t = "A <u>under</u> och <span style=\"color: #FF0000\">röd</span> text.\n\nSe [länk](http://x.se) nu\n";
        assert_eq!(doc(t, &spec("under och röd text")).matches.len(), 1);
        assert_eq!(doc(t, &spec("länk nu")).matches.len(), 1);
        assert_eq!(doc(t, &spec("Se länk nu")).matches.len(), 1);
        assert!(doc(t, &spec("http")).matches.is_empty(), "länkadressen är markup");
        // Styckebrytning syns som \n.
        let mut s = spec(r"text\.\n\nSe");
        s.regex = true;
        assert_eq!(doc(t, &s).matches.len(), 1);
    }

    #[test]
    fn matches_in_tables() {
        let t = "| Namn | Ålder |\n| --- | --- |\n| Kalle | 42 |\n";
        assert_eq!(doc(t, &spec("kalle")).matches.len(), 1);
        assert!(doc(t, &spec("---")).matches.is_empty(), "avgränsarraden är markup");
        assert!(doc(t, &spec("|")).matches.is_empty());
        // Celler skiljs åt av tabb, så ord i olika celler slås inte ihop.
        assert!(doc(t, &spec("Kalle 42")).matches.is_empty());
        let r = rep(t, &spec("Kalle"), "Pelle");
        assert!(r.contains("| Pelle | 42 |"), "{r}");
    }

    #[test]
    fn highlight_ranges_follow_visible_segments() {
        let t = "Det är **fet** text\n";
        let r = doc(t, &spec("fet text"));
        let h = highlights(&r, Some(r.matches[0].start));
        assert_eq!(h.len(), 2);
        assert!(h.iter().all(|x| x.2));
        assert_eq!(&t[h[0].0..h[0].1], "fet");
        assert_eq!(&t[h[1].0..h[1].1], " text");
        assert!(highlights(&r, None).iter().all(|x| !x.2));
    }

    #[test]
    fn replace_keeps_formatting_of_first_char() {
        let t = "Det är **fet** text.\n";
        let out = rep(t, &spec("fet text"), "tjock sak");
        assert_eq!(out, "Det är **tjock sak**.\n");
        // Ersättning mitt i fet text.
        let out = rep("a **bcd** e\n", &spec("c"), "XY");
        assert_eq!(out, "a **bXYd** e\n");
        // Flera träffar och regex med fångster.
        let mut s = spec(r"(\w)\w*");
        s.regex = true;
        let out = rep("ab *cd*\n", &s, "$1");
        assert_eq!(out, "a *c*\n");
        // Bara en träff.
        let o = DocReplace { repl: "Z", keep_text: false, apply: None, only: Some(2) };
        let (e, n) = replace_doc("x x x\n", &spec("x"), &o).unwrap();
        assert_eq!((e.text.as_str(), n), ("x Z x\n", 1));
    }

    fn red(t: &str) -> String {
        markup::set_span_prop(t, 0..t.len(), SpanProp::Color(Some([255, 0, 0]))).text
    }

    const FMT: &str = "vanlig **fet** *kursiv* <u>under</u> ~~struken~~ ***fk***\n\n# Rubrik\n\nslut\n";

    fn with_fmt(f: impl FnOnce(&mut FormatFilter)) -> Spec {
        let mut s = Spec::default();
        f(&mut s.fmt);
        s
    }

    #[test]
    fn format_filters_find_runs() {
        let bold = doc(FMT, &with_fmt(|f| f.bold = true));
        assert_eq!(found(FMT, &bold), ["fet", "fk"]);
        let it = doc(FMT, &with_fmt(|f| f.italic = true));
        assert_eq!(found(FMT, &it), ["kursiv", "fk"]);
        let both = doc(FMT, &with_fmt(|f| {
            f.bold = true;
            f.italic = true;
        }));
        assert_eq!(found(FMT, &both), ["fk"]);
        assert_eq!(found(FMT, &doc(FMT, &with_fmt(|f| f.underline = true))), ["under"]);
        assert_eq!(found(FMT, &doc(FMT, &with_fmt(|f| f.strike = true))), ["struken"]);
        assert_eq!(found(FMT, &doc(FMT, &with_fmt(|f| f.heading = Some(1)))), ["Rubrik"]);
        assert!(doc(FMT, &with_fmt(|f| f.heading = Some(2))).matches.is_empty());
    }

    #[test]
    fn format_filters_with_text_and_color() {
        // Text + format: varje tecken i träffen måste vara fet.
        let mut s = with_fmt(|f| f.bold = true);
        s.query = "fet".to_owned();
        assert_eq!(doc(FMT, &s).matches.len(), 1);
        s.query = "vanlig fet".to_owned();
        assert!(doc(FMT, &s).matches.is_empty());
        s.query = "slut".to_owned();
        assert!(doc(FMT, &s).matches.is_empty());
        // Färg och markering.
        let t = red("röd text\n");
        let s = with_fmt(|f| f.color = Some([255, 0, 0]));
        assert_eq!(found(&t, &doc(&t, &s)), ["röd text"]);
        let s = with_fmt(|f| f.color = Some([0, 0, 255]));
        assert!(doc(&t, &s).matches.is_empty());
        let h = markup::set_span_prop("a gul b\n", 2..5, SpanProp::Background(Some([255, 255, 0]))).text;
        assert_eq!(found(&h, &doc(&h, &with_fmt(|f| f.highlight = true))), ["gul"]);
    }

    #[test]
    fn apply_format_to_all_matches() {
        let t = "ett två ett tre ett\n";
        let o = DocReplace { repl: "", keep_text: true, apply: Some(Apply::Bold), only: None };
        let (e, n) = replace_doc(t, &spec("ett"), &o).unwrap();
        assert_eq!(n, 3);
        assert_eq!(e.text, "**ett** två **ett** tre **ett**\n");
        // Verifiera med ny analys + formatsökning.
        let r = doc(&e.text, &with_fmt(|f| f.bold = true));
        assert_eq!(found(&e.text, &r).len(), 3);
        assert_eq!(doc(&e.text, &spec("två")).matches.len(), 1);
        // Redan fet text blir inte dubbelt formaterad.
        let (e2, _) = replace_doc(&e.text, &spec("ett"), &o).unwrap();
        assert_eq!(e2.text, e.text);
        // Färg och rensning.
        let o = DocReplace { repl: "", keep_text: true, apply: Some(Apply::Color([255, 0, 0])), only: None };
        let (c, _) = replace_doc("a ett b\n", &spec("ett"), &o).unwrap();
        assert!(doc(&c.text, &with_fmt(|f| f.color = Some([255, 0, 0]))).matches.len() == 1);
        let o = DocReplace { repl: "", keep_text: true, apply: Some(Apply::Clear), only: None };
        let (clr, _) = replace_doc("a **fet** b\n", &spec("fet"), &o).unwrap();
        assert_eq!(markup::analyze(&clr.text).pieces.iter().filter(|p| p.bold).count(), 0, "{}", clr.text);
    }

    #[test]
    fn replace_text_and_apply_format_together() {
        let o = DocReplace { repl: "nytt", keep_text: false, apply: Some(Apply::Italic), only: None };
        let (e, n) = replace_doc("a gammalt b gammalt\n", &spec("gammalt"), &o).unwrap();
        assert_eq!(n, 2);
        assert_eq!(e.text, "a *nytt* b *nytt*\n");
    }

    #[test]
    fn empty_query_without_filter_finds_nothing() {
        assert!(doc("abc", &Spec::default()).matches.is_empty());
        assert!(replace_doc("abc", &Spec::default(), &DocReplace { repl: "x", keep_text: false, apply: None, only: None }).is_none());
    }
}
