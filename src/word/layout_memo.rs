//! Återanvändning mellan omläggningar i stora dokument: styckena (`paragraphs`), fotnotsplanen, grupperna och
//! cachenycklarnas statiska del tar tid i proportion till dokumentet, men efter en liten ändring (inkrementell
//! analys, se `incr`) ändras bara stycken nära ändringen och nära markören. `prepare` bygger om bara dessa
//! fönster (`paragraphs_in`) och behåller resten från förra omläggningen; resultatet ska vara detsamma som en
//! full omräkning (tester i `memo_tests`).
//!
//! Fönstren görs större än det som ändrats: ett par innehållsstycken på var sida är kontrollstycken som räknas
//! om och jämförs med de gamla (får de annan luft, justering eller radbrytning har ändringen påverkat något längre
//! bort och allt räknas om), och ett ytterligare stycke längst ut räknas om bara för att kanterna ska bli rätt.

use super::*;

/// Det en omläggning behöver före utläggningen av grupperna.
pub(super) struct Prep {
    pub paras: Vec<Para>,
    pub defs: Vec<notes::Def>,
    pub groups: Vec<Group>,
    /// Statisk del av cachenyckeln (`unit_key_static`) för varje grupp.
    pub skeys: Vec<u64>,
    /// Antal poster i innehållsförteckningen om texten har en (styckenas höjd beror på det).
    pub toc_n: Option<usize>,
}

pub(super) type Group = (Range<usize>, Range<usize>, Option<usize>);

/// Det förra omläggningen lämnade efter sig, för texten som fanns då (`DocState::memo_text`).
pub(super) struct Memo {
    /// Allt utom texten som resultatet gäller för (parametrar, typsnitt, sidgeometri, bildstorlekar).
    pub sig: u64,
    /// `DocState::rev` då det byggdes.
    pub rev: u64,
    pub reveal: Range<usize>,
    pub prep: Prep,
}

// Varför `reuse` avstod (nummer i källan; bara för mätning och prov).
#[cfg(test)]
thread_local! {
    pub static WHY: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn no<T>(_why: usize) -> Option<T> {
    #[cfg(test)]
    WHY.with(|w| w.borrow_mut().push(_why));
    None
}

/// Hur många innehållsstycken (inte tomma eller dolda rader) på var sida om det ändrade som räknas om och jämförs.
const CHECK: usize = 3;
/// Fönstret får inte bli större än så här många stycken: då lönar det sig inte.
const MAX_PARAS: usize = 600;
/// Så många fönster (ändring, markörens gamla och nya rader) hanteras; fler ger full omräkning.
const MAX_REGIONS: usize = 3;

/// Slår ihop det ändrade området (i nuvarande text) med fönstret (`ws..we_new`, `we_old` i texten före) från
/// en inkrementell analys: ett område som täcker alla ändringar sedan förra omläggningen.
pub(super) fn compose_hull(hull: Option<(usize, usize)>, ws: usize, we_old: usize, we_new: usize) -> (usize, usize) {
    let Some((hs, he)) = hull else { return (ws, we_new) };
    let delta = we_new as isize - we_old as isize;
    let m = |x: usize| if x >= we_old { (x as isize + delta) as usize } else { x };
    (m(hs).min(ws), m(he).max(we_new))
}

/// Kan intervallet röra något stycke? (`0..0` är "ingen markör"; ett tomt intervall mitt i en rad räknas ändå, se `intersects`.)
fn shows(r: &Range<usize>) -> bool {
    r.start != 0 || r.end != 0
}

fn content(q: &Para) -> bool {
    !q.collapse && !q.range.is_empty()
}

/// Fotnotsetiketterna i `[^etikett]` i `w`: ändras de har definitionsplanen (`notes::plan`) ändrats.
fn note_refs(w: &str) -> Vec<&str> {
    w.match_indices("[^").map(|(i, _)| w[i + 2..].split(']').next().unwrap_or("")).collect()
}

/// Finns en kommentar eller markör (`<!--…`) som inte avslutas på samma rad? `pbox::scan` hoppar då över allt fram till
/// nästa `-->`, och en oavslutad HTML-kommentar sväljer text långt efter den (bitarna ändras utanför fönstret), vilket
/// ett fönster inte ser. (I appen ger det redan full analys, `incr::Facts`; här gäller det direkta användningar.)
fn broken_marker(w: &str) -> bool {
    w.match_indices("<!--").any(|(i, _)| !w[i..].split('\n').next().unwrap_or("").contains("-->"))
}

fn same_para(a: &Para, b: &Para) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

fn toc_count(text: &str, a: &Analysis) -> usize {
    crate::word::nav::toc_total(text, &crate::word::nav::headings(text, a))
}

/// Bygger allt från grunden.
pub(super) fn fresh(text: &str, a: &Analysis, p: &Params, planned: bool, grid: &[Table], cb: &[codeblock::Block]) -> Prep {
    let mut paras = paragraphs_in(text, a, p, None, cb);
    let defs = if planned { notes::plan(text, a, &mut paras, p.zoom) } else { Vec::new() };
    let groups = group_paras(text, &paras, grid);
    let skeys = groups.iter().map(|g| unit_key_static(text, a, p, &g.1)).collect();
    let toc_n = paras.iter().any(|q| q.toc_h > 0.0).then(|| toc_count(text, a));
    Prep { paras, defs, groups, skeys, toc_n }
}

/// Ett fönster av stycken (index i de gamla styckena) som ska räknas om.
struct Region {
    /// Ändrade stycken, och fönstret med kontrollstycken (`lo..=hi`; `lo` och `hi` räknas om men används inte).
    a: usize,
    b: usize,
    lo: usize,
    hi: usize,
    edit: bool,
}

/// Återanvänder `m` för den nya texten, eller `None` om det inte går att göra säkert (då byggs allt om).
/// `old_text` är texten som `m` gäller för och `hull` området som ändrats sedan dess (nuvarande text).
#[allow(clippy::too_many_arguments)]
pub(super) fn reuse(
    m: Memo,
    old_text: &str,
    hull: Option<(usize, usize)>,
    rev: u64,
    text: &str,
    a: &Analysis,
    p: &Params,
    sig: u64,
    grid: &[Table],
    cb: &[codeblock::Block],
) -> Option<Prep> {
    if m.sig != sig || !p.matches.is_empty() {
        return no(1);
    }
    let mut prep = m.prep;
    let changed = m.rev != rev;
    if !changed && p.reveal == m.reveal {
        return Some(prep);
    }
    let delta = text.len() as isize - old_text.len() as isize;
    // Ändringen i den gamla (`hs..he_old`) och nya (`hs..he_new`) texten.
    let edit: Option<(usize, usize, usize)> = if changed {
        let (hs, he_new) = hull.or_else(|| { no::<()>(90); None })?;
        let he_old = (he_new as isize - delta).try_into().ok()?;
        if hs > he_new || he_new > text.len() || hs > he_old || he_old > old_text.len() {
            return no(2);
        }
        // Kontroll: utanför området är texten verkligen densamma.
        if old_text.as_bytes()[..hs] != text.as_bytes()[..hs] || old_text.as_bytes()[he_old..] != text.as_bytes()[he_new..] {
            return no(3);
        }
        Some((hs, he_old, he_new))
    } else {
        None
    };
    if prep.paras.is_empty() {
        return no(4);
    }
    // Områden (i gamla texten) som ska räknas om: ändringen, markörens gamla och nya rader. En bit som är med i
    // markörens rader (`intersects(pc.range, reveal)` ingår i nyckeln för hela biten) kan sträcka sig över flera
    // stycken: områdena vidgas till de bitarna.
    let to_new = |x: usize, end: bool| -> usize {
        match edit {
            Some((hs, he_old, he_new)) => {
                if x <= hs {
                    x
                } else if x >= he_old {
                    (x as isize + delta) as usize
                } else if end {
                    he_new
                } else {
                    hs
                }
            }
            None => x,
        }
    };
    let to_old = |x: usize, end: bool| -> usize {
        match edit {
            Some((hs, he_old, he_new)) => {
                if x <= hs {
                    x
                } else if x >= he_new {
                    (x as isize - delta) as usize
                } else if end {
                    he_old
                } else {
                    hs
                }
            }
            None => x,
        }
    };
    let by_pieces = |s: usize, e: usize| -> (usize, usize) {
        let i0 = a.pieces.partition_point(|pc| pc.range.end < s);
        let i1 = a.pieces.partition_point(|pc| pc.range.start <= e);
        match (a.pieces.get(i0), i1.checked_sub(1).and_then(|i| a.pieces.get(i))) {
            (Some(f), Some(l)) if i0 < i1 => (f.range.start.min(s), l.range.end.max(e)),
            _ => (s, e),
        }
    };
    let mut ivs: Vec<(usize, usize, bool)> = Vec::new();
    if let Some((hs, he_old, _)) = edit {
        ivs.push((hs, he_old, true));
    }
    for (r, old_coords) in [(&m.reveal, true), (&p.reveal, false)] {
        if !shows(r) || (!old_coords && *r == m.reveal) {
            continue;
        }
        let (s, e) = if old_coords { (to_new(r.start, false), to_new(r.end, true)) } else { (r.start, r.end) };
        let (s, e) = by_pieces(s.min(text.len()), e.min(text.len()));
        ivs.push((to_old(s, false).min(old_text.len()), to_old(e, true).min(old_text.len()), false));
    }
    ivs.sort_unstable();
    let mut regions: Vec<Region> = Vec::new();
    let np = prep.paras.len();
    // `n` innehållsstycken bakåt/framåt från `i` (och hela vägen till dokumentets kant om det inte finns så många).
    let back = |mut i: usize| {
        let mut n = 0;
        while i > 0 && n <= CHECK {
            i -= 1;
            n += usize::from(content(&prep.paras[i]));
        }
        i
    };
    let fwd = |mut i: usize| {
        let mut n = 0;
        while i + 1 < np && n <= CHECK {
            i += 1;
            n += usize::from(content(&prep.paras[i]));
        }
        i
    };
    let marker = |i: usize| prep.paras[i].tbox.is_some() || prep.paras[i].tbox_end;
    for (s, e, is_edit) in ivs {
        let ai = prep.paras.partition_point(|q| q.range.end < s).min(np - 1);
        let bi = prep.paras.partition_point(|q| q.range.start <= e).saturating_sub(1).max(ai);
        // Fönstret ska innehålla hela textrutor (öppning till stängning) som det rör vid.
        let (mut a_ext, mut b_ext) = (ai, bi);
        let (mut lo, mut hi);
        loop {
            lo = back(a_ext);
            hi = fwd(b_ext);
            let (mut nlo, mut nhi) = (lo, hi);
            if let Some(m) = (0..=lo).rev().find(|&i| marker(i)) {
                if prep.paras[m].tbox.is_some() {
                    nlo = m;
                }
            }
            for i in lo..=hi {
                if prep.paras[i].tbox.is_some() {
                    if let Some(end) = (i + 1..np).find(|&j| marker(j)) {
                        nhi = nhi.max(end);
                    }
                }
            }
            if nlo == lo && nhi == hi || hi - lo + 1 > MAX_PARAS {
                break;
            }
            a_ext = a_ext.min(nlo);
            b_ext = b_ext.max(nhi);
        }
        match regions.last_mut() {
            Some(r) if lo <= r.hi + 1 => {
                r.hi = r.hi.max(hi);
                r.b = r.b.max(bi);
                r.edit |= is_edit;
            }
            _ => regions.push(Region { a: ai, b: bi, lo, hi, edit: is_edit }),
        }
    }
    if regions.len() > MAX_REGIONS || regions.iter().any(|r| r.hi - r.lo + 1 > MAX_PARAS) {
        return no(5);
    }
    // Rutor, fotnotsdefinitioner och dokumentets slut har sammanhang som fönstret inte ser.
    let last_def = prep.defs.iter().map(|d| d.para).max();
    for r in &regions {
        if prep.defs.iter().any(|d| d.para + 2 >= r.lo && d.para <= r.hi + 2) || last_def.is_some_and(|l| r.lo > l) {
            return no(7);
        }
    }
    // Fönstren i gamla och nya texten (byte).
    let after_edit = |r: &Region| edit.is_some() && !r.edit && prep.paras[r.lo].range.start >= edit.map_or(0, |e| e.1);
    let mut wins: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for r in &regions {
        let old = prep.paras[r.lo].range.start..prep.paras[r.hi].range.end;
        let shift_start = if after_edit(r) { delta } else { 0 };
        let shift_end = if after_edit(r) || r.edit { delta } else { 0 };
        let new = (old.start as isize + shift_start) as usize..(old.end as isize + shift_end) as usize;
        if new.end > text.len() || new.start > new.end {
            return no(8);
        }
        wins.push((old, new));
    }
    // Fotnotsreferenserna i ett ändrat fönster ska vara desamma (annars har definitionsplanen ändrats).
    for (r, (old, new)) in regions.iter().zip(&wins) {
        if r.edit && note_refs(&old_text[old.clone()]) != note_refs(&text[new.clone()]) {
            return no(9);
        }
        if broken_marker(&old_text[old.clone()]) || broken_marker(&text[new.clone()]) {
            return no(10);
        }
    }
    // Innehållsförteckningens höjd beror på antalet rubriker.
    if let Some(n) = prep.toc_n {
        if toc_count(text, a) != n {
            return no(11);
        }
    }
    // Räkna om fönstren.
    let mut news: Vec<Vec<Para>> = Vec::new();
    for (_, new) in &wins {
        news.push(paragraphs_in(text, a, p, Some(new.clone()), cb));
    }
    // Flytta styckena efter ändringen (före jämförelsen och inklistringen: de nya fönstren ligger i nya koordinater).
    let b_edit = regions.iter().find(|r| r.edit).map(|r| r.b);
    if let (Some(b), true) = (b_edit, delta != 0) {
        for q in &mut prep.paras[b + 1..] {
            q.range = (q.range.start as isize + delta) as usize..(q.range.end as isize + delta) as usize;
        }
    }
    // Kontrollera kontrollstyckena och sätt in de nya styckena (bakifrån så att index i gamla listan gäller).
    let mut dn: Vec<(usize, isize)> = Vec::new();
    for ((r, w), (_, new)) in regions.iter().zip(&news).zip(&wins).rev() {
        if w.is_empty() || w[0].range.start != new.start || w.last()?.range.end != new.end {
            return no(12);
        }
        let lo_t = if r.lo == 0 { 0 } else { r.lo + 1 };
        let hi_t = if r.hi + 1 == np { r.hi } else { r.hi - 1 };
        if lo_t > r.a || hi_t < r.b || hi_t < lo_t {
            return no(13);
        }
        // Före det ändrade.
        for i in lo_t..r.a {
            if !same_para(&w[i - r.lo], &prep.paras[i]) {
                return no(14);
            }
        }
        // Efter det ändrade: stycke `i` (gamla index) ligger `r.hi - i` från slutet av fönstret.
        for i in r.b + 1..=hi_t {
            let k = (r.hi - i) + 1;
            if w.len() < k || !same_para(&w[w.len() - k], &prep.paras[i]) {
                return no(15);
            }
        }
        let from = lo_t - r.lo;
        let to = w.len() - (r.hi - hi_t);
        if to < from {
            return no(16);
        }
        let old_n = hi_t - lo_t + 1;
        let new_n = to - from;
        // Utan ändring (markören flyttades) har fönstret samma antal stycken.
        if !r.edit && new_n != old_n {
            return no(17);
        }
        prep.paras.splice(lo_t..=hi_t, w[from..to].iter().cloned());
        dn.push((hi_t, new_n as isize - old_n as isize));
    }
    // Fotnotsdefinitionerna efter ändringen flyttas (inga definitioner ligger i eller nära fönstren).
    if let Some((_, he_old, _)) = edit {
        for d in &mut prep.defs {
            let shift: isize = dn.iter().filter(|(hi_t, _)| *hi_t < d.para).map(|(_, n)| *n).sum();
            let after = d.line.start >= he_old;
            d.para = (d.para as isize + shift) as usize;
            if after && delta != 0 {
                let mv = |x: usize| (x as isize + delta) as usize;
                d.line = mv(d.line.start)..mv(d.line.end);
                d.body = mv(d.body);
            }
        }
    }
    // Innehållsförteckningen kan ha tillkommit eller försvunnit i ett fönster.
    prep.toc_n = prep.paras.iter().any(|q| q.toc_h > 0.0).then(|| toc_count(text, a));
    // Grupper och statiska nycklar: oförändrade grupper utanför fönstren behåller sin nyckel.
    let old_groups = std::mem::take(&mut prep.groups);
    let old_keys = std::mem::take(&mut prep.skeys);
    let groups = group_paras(text, &prep.paras, grid);
    let dirty: Vec<Range<usize>> = wins.iter().map(|(_, n)| n.clone()).collect();
    let he_new = edit.map_or(usize::MAX, |e| e.2);
    let mut skeys: Vec<u64> = Vec::with_capacity(groups.len());
    let mut j = 0;
    for g in &groups {
        let w = &g.1;
        let i = dirty.partition_point(|r| r.end <= w.start);
        let clean = !dirty.get(i).is_some_and(|r| r.start < w.end);
        let old_start = if edit.is_some() && w.start >= he_new { (w.start as isize - delta) as usize } else { w.start };
        let mut key = None;
        if clean {
            while j < old_groups.len() && old_groups[j].1.start < old_start {
                j += 1;
            }
            if let Some(og) = old_groups.get(j) {
                if og.1.start == old_start && og.1.len() == w.len() && og.2.is_some() == g.2.is_some() && og.0.len() == g.0.len() {
                    key = Some(old_keys[j]);
                }
            }
        }
        skeys.push(key.unwrap_or_else(|| unit_key_static(text, a, p, w)));
    }
    prep.groups = groups;
    prep.skeys = skeys;
    Some(prep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::bench::Rng;
    use crate::word::{incr, pages};

    fn fam(_: Option<&str>, bold: bool, italic: bool) -> FontFamily {
        theme::doc_font(bold, italic)
    }

    /// Dokument i Word-dialekten: stycken med `<!--k:pp-->`-markörer, tomma stycken, rubriker, listor, tabeller,
    /// `<div align>`, sidbrytningar, innehållsförteckning, fotnoter (definitioner sist) och ibland en textruta.
    fn doc(r: &mut Rng, paras: usize, boxes: bool) -> String {
        let words = ["alfa", "beta", "gamma", "delta", "åäö", "text", "rad", "stycke", "dokument", "sida"];
        let mut s = String::from("<!-- kladd: sida=a4; h1: \"Calibri\" 20pt -->\n\n");
        let mut notes = 0;
        for i in 0..paras {
            let kind = r.below(24);
            let mut line = String::new();
            for _ in 0..(3 + r.below(40)) {
                let w = words[r.below(words.len())];
                match r.below(14) {
                    0 => line.push_str(&format!("**{w}** ")),
                    1 => line.push_str(&format!("*{w}* ")),
                    _ => line.push_str(&format!("{w} ")),
                }
            }
            match kind {
                0 => s.push_str(&format!("## Rubrik {i}")),
                1 => s.push_str("- punkt ett\n- punkt två\n- punkt tre"),
                2 => s.push_str("| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |"),
                3 => s.push_str("<!--k:ep-->"),
                13 => s.push_str("```\nkod rad ett\n\nkod rad två\n```"),
                4 => s.push_str("<!--k:ep sp=\"b=0 a=12\"-->"),
                5 => s.push_str("<!-- kladd: sidbrytning -->"),
                6 => s.push_str(&format!("<div align=\"justify\">\n\n{line}\n\n</div>")),
                7 | 8 => s.push_str(&format!("{line}<!--k:pp sp=\"b=0 a={}\"-->", r.below(12))),
                9 => s.push_str(&format!("{line}<!--k:pp bdr=\"t=single:8:4:156082 b=single:8:4:156082\" shd=\"E8F1F5\"-->")),
                10 if boxes => s.push_str(&format!("<!--k:tb 1 w=\"157.5\" h=\"90\" hrel=\"page\" x=\"400\" y=\"0\"-->\n\n{line}\n\n<!--k:/tb-->")),
                11 | 12 => {
                    notes += 1;
                    s.push_str(&format!("{line}[^{notes}]"));
                }
                _ => s.push_str(&line),
            }
            s.push_str("\n\n");
            if i == paras / 5 {
                s.push_str("<!-- kladd: toc -->\n\n");
            }
        }
        for n in 1..=notes {
            s.push_str(&format!("[^{n}]: Fotnot {n} med text.\n\n"));
        }
        s
    }

    fn edit(r: &mut Rng, text: &mut String) {
        let mut at = r.below(text.len());
        while !text.is_char_boundary(at) {
            at += 1;
        }
        match r.below(12) {
            0..=3 => text.insert(at, 'x'),
            4 => text.insert_str(at, " ord "),
            5 => text.insert_str(at, "\n\n"),
            6 => text.insert(at, '\n'),
            7 => text.insert_str(at, "<!--k:pp sp=\"b=3 a=3\"-->"),
            8 => text.insert_str(at, "# "),
            9 => {
                if let Some(c) = text[at..].chars().next() {
                    text.replace_range(at..at + c.len_utf8(), "");
                }
            }
            10 => {
                let mut e = (at + 1 + r.below(40)).min(text.len());
                while !text.is_char_boundary(e) {
                    e += 1;
                }
                text.replace_range(at..e, "");
            }
            _ => text.insert_str(at, "**"),
        }
    }

    fn defs_of(prep: &Prep) -> Vec<(String, usize, Range<usize>, usize)> {
        prep.defs.iter().map(|d| (d.label.clone(), d.para, d.line.clone(), d.body)).collect()
    }

    fn same(inc: &Prep, full: &Prep, step: &str) {
        assert_eq!(inc.paras.len(), full.paras.len(), "steg {step}: antal stycken");
        for (i, (x, y)) in inc.paras.iter().zip(&full.paras).enumerate() {
            assert_eq!(format!("{x:?}"), format!("{y:?}"), "steg {step}: stycke {i}");
        }
        assert_eq!(defs_of(inc), defs_of(full), "steg {step}: fotnotsdefinitioner");
        assert_eq!(inc.groups.len(), full.groups.len(), "steg {step}: antal grupper");
        assert!(inc.groups == full.groups, "steg {step}: grupper");
        let bad: Vec<usize> = (0..inc.skeys.len()).filter(|&i| inc.skeys[i] != full.skeys[i]).collect();
        assert!(bad.is_empty(), "steg {step}: nycklar skiljer i grupper {bad:?} (av {}), t.ex. {:?}", inc.skeys.len(), bad.first().map(|&i| &inc.groups[i]));
        assert_eq!(inc.toc_n, full.toc_n, "steg {step}: innehållsförteckning");
    }

    /// Återanvänd förberedelse (`reuse`) ska ge exakt det som en full omräkning ger.
    fn reuse_equals_fresh(seed: u64, paras: usize, boxes: bool, steps: usize) -> (usize, usize) {
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let measure = |_: &str, _: FontId| 8.0;
        let mut r = Rng(seed);
        let mut text = doc(&mut r, paras, boxes);
        let (mut reused, mut fell_back) = (0, 0);
        for step in 0..steps {
            let old = text.clone();
            for _ in 0..1 + r.below(2) {
                edit(&mut r, &mut text);
            }
            let rv = |r: &mut Rng, t: &str| -> Range<usize> {
                if r.below(3) == 0 {
                    return 0..0;
                }
                let mut a = r.below(t.len());
                while !t.is_char_boundary(a) {
                    a += 1;
                }
                crate::word::tables::reveal_range(t, a..a)
            };
            let (old_rv, new_rv) = (rv(&mut r, &old), rv(&mut r, &text));
            let mk = |reveal: Range<usize>| Params {
                palette: &pal,
                zoom: 1.0,
                reveal,
                matches: &[],
                image_size: &img,
                measure: &measure,
                font_family: &fam,
                fonts_gen: 0,
            };
            let (po, pn) = (mk(old_rv.clone()), mk(new_rv));
            let (ao, an) = (markup::analyze(&old), markup::analyze(&text));
            let (go, gn) = (tables(&old, &ao), tables(&text, &an));
            let (co, cn) = (codeblock::blocks(&old, &ao), codeblock::blocks(&text, &an));
            let memo = Memo { sig: 7, rev: 1, reveal: old_rv, prep: fresh(&old, &ao, &po, true, &go, &co) };
            let full = fresh(&text, &an, &pn, true, &gn, &cn);
            let ch = incr::changed_range(&old, &text);
            // Ett litet område (som `analyze_incremental` skulle ge), ibland utvidgat till hela styckena runt.
            let hull = if r.below(2) == 0 {
                (ch.start, ch.end)
            } else {
                let mut c0 = ch.start.min(old.len());
                while !old.is_char_boundary(c0) {
                    c0 -= 1;
                }
                let mut c1 = ch.end.min(text.len());
                while !text.is_char_boundary(c1) {
                    c1 += 1;
                }
                let s = old[..c0].rfind("\n\n").map_or(0, |i| i + 2);
                (s.min(ch.start), text[c1..].find("\n\n").map_or(text.len(), |i| c1 + i).max(ch.end))
            };
            let rev = if old == text { 1 } else { 2 };
            // `analyze_incremental` anger om fotnotsreferenserna ändrats (`Delta::notes_changed`); här jämförs hela analyserna.
            let notes = |a: &Analysis| -> Vec<(bool, String)> {
                let mut v: Vec<(bool, String)> = a.pieces.iter().flat_map(|p| p.footnote_ref.clone().map(|x| (true, x)).into_iter().chain(p.footnote_def.clone().map(|x| (false, x)))).collect();
                v.sort();
                v.dedup();
                v
            };
            if notes(&ao) != notes(&an) {
                fell_back += 1;
                continue;
            }
            match reuse(memo, &old, Some(hull), rev, &text, &an, &pn, 7, &gn, &cn) {
                Some(inc) => {
                    same(&inc, &full, &format!("seed {seed} steg {step} ch {ch:?} hull {hull:?} rv {:?}->{:?} | grupp ~ ", po.reveal, pn.reveal));
                    reused += 1;
                }
                None => fell_back += 1,
            }
        }
        (reused, fell_back)
    }

    #[test]
    fn reused_preparation_equals_full() {
        let (mut reused, mut back) = (0, 0);
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(6).max(6);
        for seed in 1..=seeds {
            let (a, b) = reuse_equals_fresh(seed * 7919, 260, false, 40);
            reused += a;
            back += b;
        }
        // De flesta ändringar ligger långt från fotnotsdefinitioner och rutor och ska gå att återanvända.
        assert!(reused > 2 * back, "återanvändes {reused}, full omräkning {back}");
    }

    #[test]
    fn reused_one_seed() {
        if let Some(seed) = std::env::var("MEMO_SEED").ok().and_then(|v| v.parse::<u64>().ok()) {
            reuse_equals_fresh(seed, 260, false, 40);
        }
    }

    #[test]
    fn reused_preparation_with_text_boxes() {
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).max(4);
        let (mut reused, mut back) = (0, 0);
        for seed in 1..=seeds {
            let (a, b) = reuse_equals_fresh(seed * 104_729, 200, true, 40);
            reused += a;
            back += b;
        }
        assert!(reused > back, "återanvändes {reused}, full omräkning {back}");
    }

    /// Hela vägen: galleriet efter inkrementell analys och återanvänd förberedelse är detsamma som i en ny cache.
    #[test]
    fn galley_after_edits_equals_fresh_cache() {
        incr::MIN_OVERRIDE.with(|m| m.set(Some(1000)));
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut r = Rng(4242);
        let mut text = doc(&mut r, 120, false);
        let mut cache = Cache::default();
        let a0 = markup::analyze(&text);
        let geom = pages::Geom::for_sections(1.0, &a0.defaults, &a0.sections);
        let shape = |g: &Galley| -> Vec<(i32, i32, usize, bool)> {
            g.rows.iter().map(|r| ((r.pos.x * 100.0) as i32, (r.pos.y * 100.0) as i32, r.char_count_including_newline().0, r.ends_with_newline)).collect()
        };
        let mut reused = 0;
        for step in 0..30 {
            if step > 0 {
                edit(&mut r, &mut text);
            }
            let reveal = if step % 4 == 1 { crate::word::tables::reveal_range(&text, text.len() / 2..text.len() / 2) } else { 0..0 };
            let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, egui::Color32::WHITE).size().x);
            let p = Params { palette: &pal, zoom: 1.0, reveal, matches: &[], image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
            let out = ctx.run_ui(Default::default(), |_| {
                let g = cache.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                reused += usize::from(cache.stats(1).reused);
                let mut fresh_cache = Cache::default();
                let f = fresh_cache.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                assert_eq!(shape(&g), shape(&f), "steg {step}");
                assert_eq!(g.rect, f.rect, "steg {step}");
                assert_eq!(cache.page_boxes(1, &text).len(), fresh_cache.page_boxes(1, &text).len(), "steg {step}: sidor");
            });
            std::mem::forget(out);
        }
        incr::MIN_OVERRIDE.with(|m| m.set(None));
        assert!(reused > 8, "återanvändes bara {reused} gånger");
    }
}
