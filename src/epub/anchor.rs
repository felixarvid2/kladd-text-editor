//! Bokmärken och understrykningar i en bok: förankring som tål omläggning, ren text ur Markdown (för att hitta en
//! markering och för utdrag) och export som Markdown.
//!
//! En post (`Note`) pekar ut ett kapitel (index i `Book::chapters`, alltså läsordningens fil) och ett byteområde
//! i kapitlets Markdown, plus den markerade texten själv (`raw`) och några tecken före och efter. Stämmer
//! området fortfarande med utdraget används det; annars (boken lästes in på ett annat sätt, kapitlet delades
//! om, texten ändrades något) söks utdraget upp i kapitlet och det träffställe vars omgivning liknar mest väljs.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, TagEnd};
use serde::{Deserialize, Serialize};

/// Så många byte av omgivningen före och efter som sparas.
pub const CONTEXT: usize = 32;
/// Längsta markering (byte Markdown) som sparas.
pub const MAX_RAW: usize = 6000;
/// Längd (byte) på det utdrag ett bokmärke pekar på.
pub const BOOKMARK_SPAN: usize = 64;

/// Ett bokmärke eller en understrykning.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Note {
    pub id: u64,
    /// Bokmärke (annars understrykning).
    pub bookmark: bool,
    pub chapter: usize,
    /// Område i kapitlets Markdown.
    pub start: usize,
    pub end: usize,
    /// Markdown-texten i området, och omgivningen före och efter.
    pub raw: String,
    pub before: String,
    pub after: String,
    /// Texten som visas i listor (ren text).
    pub quote: String,
    /// Färg: 0 gul, 1 grön, 2 blå, 3 rosa.
    pub color: u8,
    /// Användarens anteckning.
    pub note: String,
    /// Skapad (sekunder sedan epoken).
    pub created: u64,
}

pub const COLORS: usize = 4;

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Ren text ur Markdown med källpositioner: ett tecken i taget, blanksteg hopdragna till ett.
#[derive(Default, Debug)]
pub struct Plain {
    pub chars: Vec<char>,
    /// Källbyte där tecknet börjar och slutar.
    pub start: Vec<usize>,
    pub end: Vec<usize>,
}

impl Plain {
    fn push(&mut self, ch: char, s: usize, e: usize) {
        if ch.is_whitespace() {
            if self.chars.last().is_none_or(|c| *c == ' ') {
                return;
            }
            self.chars.push(' ');
        } else {
            self.chars.push(ch);
        }
        self.start.push(s);
        self.end.push(e);
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect::<String>().trim().to_owned()
    }
}

pub fn plain_map(md: &str) -> Plain {
    let mut p = Plain::default();
    for (ev, range) in Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH).into_offset_iter() {
        match ev {
            Event::Text(t) | Event::Code(t) => {
                // Utan skyddstecken eller entiteter är texten och källan lika långa: tecken för tecken.
                let exact = t.len() == range.len();
                for (off, ch) in t.char_indices() {
                    let (s, e) = if exact { (range.start + off, range.start + off + ch.len_utf8()) } else { (range.start, range.end) };
                    p.push(ch, s, e);
                }
            }
            Event::SoftBreak | Event::HardBreak => p.push(' ', range.start, range.end),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableCell | TagEnd::TableRow | TagEnd::CodeBlock) => {
                p.push(' ', range.end, range.end)
            }
            _ => {}
        }
    }
    p
}

fn norm_chars(s: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in s.chars() {
        if c.is_whitespace() {
            if out.last().is_some_and(|l| *l != ' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    while out.last() == Some(&' ') {
        out.pop();
    }
    out
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Var i Markdown-texten `selected` (så som läsvyn ger den ur en markering) står. Finns den på flera ställen väljs
/// det som ligger närmast `near` (en byteposition i texten). Ger byteområdet i `md`.
pub fn locate(md: &str, selected: &str, near: usize) -> Option<Range<usize>> {
    let needle: Vec<char> = norm_chars(selected).into_iter().map(lower).collect();
    if needle.is_empty() {
        return None;
    }
    let plain = plain_map(md);
    let hay: Vec<char> = plain.chars.iter().map(|c| lower(*c)).collect();
    if hay.len() < needle.len() {
        return None;
    }
    let mut best: Option<(usize, usize)> = None; // (avstånd, index)
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            let d = plain.start[i].abs_diff(near);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, i));
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    let (_, i) = best?;
    let j = i + needle.len() - 1;
    let (s, e) = (plain.start[i], plain.end[j]);
    (s < e).then_some(s..e.min(md.len()))
}

/// Visningstext (ren text, högst `max` tecken) för ett stycke Markdown.
pub fn excerpt(md_slice: &str, max: usize) -> String {
    let t = plain_map(md_slice).text();
    if t.chars().count() <= max { t } else { format!("{}…", t.chars().take(max.saturating_sub(1)).collect::<String>().trim_end()) }
}

/// Visningstext för området `range` i `md`: bara det som syns (utan fet- och kursivtecken som hamnat i kanten av området).
pub fn excerpt_range(md: &str, range: Range<usize>, max: usize) -> String {
    let p = plain_map(md);
    let mut out = Plain::default();
    for i in 0..p.chars.len() {
        if p.start[i] >= range.start && p.end[i] <= range.end.max(range.start) {
            out.push(p.chars[i], p.start[i], p.end[i]);
        }
    }
    let t = out.text();
    if t.chars().count() <= max { t } else { format!("{}…", t.chars().take(max.saturating_sub(1)).collect::<String>().trim_end()) }
}

/// Bygger en post för området `range` i kapitel `chapter` vars Markdown är `md`.
pub fn make(id: u64, chapter: usize, md: &str, range: Range<usize>, bookmark: bool) -> Note {
    let start = floor_boundary(md, range.start);
    let mut end = ceil_boundary(md, range.end.max(start));
    if end - start > MAX_RAW {
        end = floor_boundary(md, start + MAX_RAW);
    }
    let raw = md[start..end].to_owned();
    let b0 = ceil_boundary(md, start.saturating_sub(CONTEXT));
    let a1 = floor_boundary(md, end + CONTEXT);
    Note {
        id,
        bookmark,
        chapter,
        start,
        end,
        quote: excerpt_range(md, start..end, if bookmark { 140 } else { 600 }),
        raw,
        before: md[b0..start].to_owned(),
        after: md[end..a1].to_owned(),
        color: 0,
        note: String::new(),
        created: now_secs(),
    }
}

/// Ett bokmärke vid `at`: de första tecknen därifrån (också över radbrytningar, så att en rubrik följs av början på texten).
pub fn bookmark_at(id: u64, chapter: usize, md: &str, at: usize) -> Note {
    let at = floor_boundary(md, at);
    let end = floor_boundary(md, (at + BOOKMARK_SPAN).min(md.len()));
    make(id, chapter, md, at..end.max(at), true)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn common_suffix(a: &str, b: &str) -> usize {
    a.chars().rev().zip(b.chars().rev()).take_while(|(x, y)| x == y).count()
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// Var posten står nu i kapitlets Markdown. `None` om utdraget inte längre finns.
pub fn resolve(md: &str, n: &Note) -> Option<Range<usize>> {
    if n.start <= n.end
        && n.end <= md.len()
        && md.is_char_boundary(n.start)
        && md.is_char_boundary(n.end)
        && md[n.start..n.end] == n.raw
    {
        return Some(n.start..n.end);
    }
    if n.raw.is_empty() {
        return None;
    }
    let mut best: Option<(i64, usize)> = None;
    for (i, _) in md.match_indices(&n.raw) {
        let before = &md[ceil_boundary(md, i.saturating_sub(CONTEXT * 2))..i];
        let after = &md[i + n.raw.len()..floor_boundary(md, i + n.raw.len() + CONTEXT * 2)];
        let ctx = (common_suffix(before, &n.before) + common_prefix(after, &n.after)) as i64;
        let dist = i.abs_diff(n.start).min(1_000_000) as i64;
        let score = ctx * 1_000_000 - dist;
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, i));
        }
    }
    best.map(|(_, i)| i..i + n.raw.len())
}

// ------------------------------------------------------------------ export

/// Anteckningarna som Markdown: bokens titel och författare, kapitelrubriker, citat (`>`) och anteckningar.
/// `chapters` är kapitlen i läsordning (index och titel); poster i kapitel som inte finns där hamnar sist.
pub fn export_markdown(title: &str, authors: &[String], chapters: &[(usize, String)], notes: &[Note]) -> String {
    let heading = |s: &str| s.replace('\n', " ").trim().to_owned();
    let mut out = String::new();
    let t = heading(title);
    out.push_str(&format!("# {}\n\n", if t.is_empty() { crate::i18n::t("Anteckningar").to_owned() } else { t }));
    if !authors.is_empty() {
        out.push_str(&format!("*{}*\n\n", authors.join(", ")));
    }
    let mut order: Vec<(usize, String)> = chapters.to_vec();
    for n in notes {
        if !order.iter().any(|(c, _)| *c == n.chapter) {
            order.push((n.chapter, crate::tf!("Kapitel {n}", n = n.chapter + 1)));
        }
    }
    let mut any = false;
    for (ch, ch_title) in &order {
        let mut here: Vec<&Note> = notes.iter().filter(|n| n.chapter == *ch).collect();
        if here.is_empty() {
            continue;
        }
        here.sort_by_key(|n| (n.start, n.id));
        any = true;
        out.push_str(&format!("## {}\n\n", heading(ch_title)));
        for n in here {
            let quote = if n.quote.trim().is_empty() { excerpt(&n.raw, 600) } else { n.quote.clone() };
            if n.bookmark {
                out.push_str(&format!("- {} {}\n\n", crate::i18n::t("Bokmärke:"), quote));
                continue;
            }
            for line in quote.lines() {
                out.push_str(&format!("> {line}\n"));
            }
            out.push('\n');
            if !n.note.trim().is_empty() {
                for (i, line) in n.note.trim().lines().enumerate() {
                    if i == 0 {
                        out.push_str(&format!("**{}** {}\n", crate::i18n::t("Anteckning:"), line));
                    } else {
                        out.push_str(&format!("{line}\n"));
                    }
                }
                out.push('\n');
            }
        }
    }
    if !any {
        out.push_str(&format!("{}\n", crate::i18n::t("Inga bokmärken eller anteckningar än.")));
    }
    out.trim_end().to_owned() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Ett\n\nDet var en gång en räv som bodde i en lund.\n\nRäven var **mycket** listig. Det var en gång en räv till.\n";

    #[test]
    fn markering_hittas_i_källtexten_och_närmaste_träff_väljs() {
        let r = locate(MD, "mycket listig", 0).unwrap();
        assert_eq!(&MD[r], "mycket** listig", "området täcker markeringen inklusive fetmarkeringens tecken");
        // Två förekomster: den närmast `near` väljs.
        let first = MD.find("Det var").unwrap();
        let second = MD.rfind("Det var").unwrap();
        let a = locate(MD, "Det var en gång en räv", first).unwrap();
        let b = locate(MD, "det var en gång en räv", second).unwrap();
        assert_eq!((a.start, b.start), (first, second), "gemener och versaler spelar ingen roll");
        // Radbrytningar i markeringen (flera stycken) och dubbla blanksteg.
        let r = locate(MD, "lund.\nRäven   var", 0).unwrap();
        assert!(MD[r].starts_with("lund.") && MD[..MD.len()].contains("Räven"));
        assert!(locate(MD, "finns inte", 0).is_none());
        assert!(locate(MD, "   ", 0).is_none());
    }

    #[test]
    fn förankringen_tål_att_texten_flyttas_eller_ändras_före() {
        let r = locate(MD, "Det var en gång en räv till", 0).unwrap();
        let n = make(1, 3, MD, r.clone(), false);
        assert_eq!(n.raw, "Det var en gång en räv till");
        assert_eq!(resolve(MD, &n), Some(r.clone()), "oförändrad text: samma område");
        // Text infogas först (omläggning): utdraget hittas igen vid sitt nya ställe.
        let moved = format!("Nytt stycke först.\n\n{MD}");
        let r2 = resolve(&moved, &n).unwrap();
        assert_eq!(&moved[r2.clone()], "Det var en gång en räv till");
        assert_eq!(r2.start, r.start + "Nytt stycke först.\n\n".len());
        // Samma ord står på två ställen: omgivningen avgör.
        let dup = format!("{MD}\nDet var en gång en räv till vid slutet.\n");
        let r3 = resolve(&dup, &n).unwrap();
        assert_eq!(r3.start, r.start, "första förekomsten har rätt omgivning");
        let dup2 = format!("Det var en gång en räv till vid början.\n\n{MD}");
        let r4 = resolve(&dup2, &n).unwrap();
        assert!(dup2[..r4.start].ends_with("listig. "), "rätt omgivning vinner över närmaste träff: {:?}", &dup2[..r4.start]);
        // Utdraget finns inte längre: ingen träff (posten finns kvar men går inte att rita).
        assert!(resolve("helt annan text", &n).is_none());
    }

    #[test]
    fn bokmärke_pekar_på_början_av_stycket_och_hittas_igen() {
        let at = MD.find("Räven var").unwrap();
        let n = bookmark_at(9, 1, MD, at);
        assert!(n.bookmark && n.start == at && n.raw.starts_with("Räven var"), "{n:?}");
        assert!(n.quote.starts_with("Räven var mycket listig."), "ren text utan **: {:?}", n.quote);
        assert!(n.end - n.start <= BOOKMARK_SPAN + 4);
        // En rubrik följs av början på texten, så att utdraget säger något.
        let h = bookmark_at(8, 1, MD, 0);
        assert!(h.quote.starts_with("Ett Det var en gång"), "{:?}", h.quote);
        let moved = format!("Inledning.\n\n{MD}");
        assert_eq!(resolve(&moved, &n).unwrap().start, at + "Inledning.\n\n".len());
        // Åäö vid gränserna: aldrig panic.
        let sv = "ÅÄÖ åäö ".repeat(30);
        for i in 0..sv.len() {
            let n = bookmark_at(1, 0, &sv, i);
            assert!(sv.is_char_boundary(n.start) && sv.is_char_boundary(n.end));
        }
    }

    #[test]
    fn sparade_poster_från_en_äldre_version_läses_med_standardvärden() {
        let n: Note = serde_json::from_str("{\"id\":5,\"chapter\":2,\"raw\":\"abc\"}").unwrap();
        assert_eq!((n.id, n.chapter, n.raw.as_str(), n.color, n.bookmark), (5, 2, "abc", 0, false));
    }

    #[test]
    fn export_ger_rubriker_citat_och_anteckningar_i_kapitelordning() {
        let mut a = make(1, 2, MD, locate(MD, "mycket listig", 0).unwrap(), false);
        a.note = "Bra ord.\nAndra raden.".into();
        let b = make(2, 0, MD, locate(MD, "en lund", 0).unwrap(), false);
        let bm = bookmark_at(3, 2, MD, MD.find("Räven").unwrap());
        let chapters = vec![(0, "Början".to_owned()), (2, "Slutet".to_owned())];
        let out = export_markdown("Räven & lunden", &["A. Författare".to_owned()], &chapters, &[a, b, bm]);
        assert!(out.starts_with("# Räven & lunden\n\n*A. Författare*\n\n## Början\n\n> en lund\n"), "{out}");
        let slutet = out.find("## Slutet").unwrap();
        assert!(out.find("> en lund").unwrap() < slutet, "kapitelordning");
        assert!(out[slutet..].contains("> mycket listig\n\n**Anteckning:** Bra ord.\nAndra raden.\n"), "{out}");
        assert!(out.contains("Bokmärke: Räven var mycket"), "{out}");
        // Utan poster.
        let tom = export_markdown("", &[], &[], &[]);
        assert!(tom.starts_with("# ") && tom.contains("Inga bokmärken"), "{tom}");
    }
}
