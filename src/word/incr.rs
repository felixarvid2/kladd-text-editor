//! Inkrementell analys för stora dokument (hundratals sidor): efter en liten ändring tolkas bara
//! stycket med ändringen, plus ett stycke på var sida som "kanariefågel", i stället för hela texten.
//!
//! Resultatet ska vara exakt detsamma som `markup::analyze`. Därför återanvänds det gamla bara när
//! ändringen är liten och lokal och när kanariestyckena fick samma bitar som i den gamla analysen
//! (annars har ändringen påverkat något som sträcker sig över styckegränsen, t.ex. ett kodstaket
//! eller en `<span>`, och hela texten tolkas om). Konstruktioner som är beroende av hela dokumentet
//! (referenslänkar, fotnoter, avsnitt, kodstaket, indragen kod) ger alltid full analys.

use std::ops::Range;

use super::markup::{self, Analysis, Piece};

/// Under så här många byte tolkas alltid hela texten (små dokument beter sig som förut).
pub const MIN_BYTES: usize = 200_000;
/// Största ändring (byte) och största fönster som tolkas om.
const MAX_EDIT: usize = 4_000;
const MAX_WINDOW: usize = 120_000;

/// Vad som ändrades vid en inkrementell analys, så att cachar som beror på analysen kan behållas.
#[derive(Debug, Default)]
pub struct Delta {
    /// Fönstrets början (byte); allt före är oförändrat och ligger kvar på samma plats.
    pub ws: usize,
    /// Någon tabellbit fanns i det omtolkade fönstret (före eller efter).
    pub touched_table: bool,
    /// Bilderna i fönstret är desamma som förut (i samma ordning).
    pub same_images: bool,
    /// Typsnitten som fönstrets nya bitar behöver.
    pub fonts: std::collections::BTreeSet<(String, bool, bool)>,
}

/// Uppdaterar `old` (analysen av `old_text`) på plats så att den blir analysen av `new_text`.
/// `None` (och `old` orörd) om det inte går att göra säkert; då görs en full analys.
pub fn analyze_incremental(old_text: &str, old: &mut Analysis, new_text: &str) -> Option<Delta> {
    incremental(old_text, old, new_text, MIN_BYTES)
}

/// Längden på det gemensamma prefixet (byte).
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + 64 <= n && a[i..i + 64] == b[i..i + 64] {
        i += 64;
    }
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

fn common_suffix(a: &[u8], b: &[u8], limit: usize) -> usize {
    let n = a.len().min(b.len()).min(limit);
    let mut i = 0;
    while i + 64 <= n && a[a.len() - i - 64..a.len() - i] == b[b.len() - i - 64..b.len() - i] {
        i += 64;
    }
    while i < n && a[a.len() - 1 - i] == b[b.len() - 1 - i] {
        i += 1;
    }
    i
}

fn line_start(t: &[u8], pos: usize) -> usize {
    t[..pos.min(t.len())].iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1)
}

/// Slutet (exklusivt, utan radbrytningen) på raden som börjar vid `ls`.
fn line_end(t: &[u8], ls: usize) -> usize {
    t[ls..].iter().position(|&c| c == b'\n').map_or(t.len(), |i| ls + i)
}

fn blank(t: &[u8], a: usize, b: usize) -> bool {
    t[a..b].iter().all(|c| matches!(c, b' ' | b'\t' | b'\r'))
}

/// Början av stycket (raderna efter en blankrad) som innehåller `pos`.
fn para_start(t: &[u8], pos: usize) -> Option<usize> {
    let mut ls = line_start(t, pos);
    let floor = ls.saturating_sub(MAX_WINDOW);
    while ls > 0 {
        let prev = line_start(t, ls - 1);
        if blank(t, prev, ls - 1) {
            return Some(ls);
        }
        if prev < floor {
            return None;
        }
        ls = prev;
    }
    Some(0)
}

/// Slutet på stycket som innehåller `pos`: början på blankraden efter det (eller textens slut).
fn para_end(t: &[u8], pos: usize) -> Option<usize> {
    let mut ls = line_start(t, pos);
    let ceil = pos + MAX_WINDOW;
    loop {
        let le = line_end(t, ls);
        if blank(t, ls, le) && ls > 0 && ls != line_start(t, pos) {
            return Some(ls);
        }
        if le >= t.len() {
            return Some(t.len());
        }
        ls = le + 1;
        if ls > ceil {
            return None;
        }
    }
}

/// Konstruktioner vars tolkning beror på resten av dokumentet: då görs full analys.
fn risky(s: &str) -> bool {
    if s.contains("```") || s.contains("~~~") || s.contains("[^") || s.contains("]:") {
        return true;
    }
    // Indragen kod och fortsättningsrader i listor.
    s.starts_with("    ") || s.starts_with('\t') || s.contains("\n    ") || s.contains("\n\t")
}

fn shifted(p: &Piece, delta: isize, base: usize) -> Piece {
    let mut q = p.clone();
    let mv = |x: usize| (x as isize + delta) as usize + base;
    q.range = mv(p.range.start)..mv(p.range.end);
    q
}

#[cfg(test)]
thread_local! {
    pub static WHY: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Avstår från den inkrementella vägen (i tester noteras varför).
fn decline(_why: usize) -> Option<Delta> {
    #[cfg(test)]
    WHY.with(|w| w.borrow_mut().push(_why));
    None
}

fn incremental(old_text: &str, old: &mut Analysis, new_text: &str, min_bytes: usize) -> Option<Delta> {
    if old_text.len() < min_bytes || new_text.len() < min_bytes {
        return decline(1);
    }
    // Kodstaket, fotnoter och referenslänkar påverkar tolkningen över hela dokumentet.
    if ["```", "~~~", "[^", "]:"].iter().any(|m| new_text.contains(m)) {
        return decline(13);
    }
    let (ob, nb) = (old_text.as_bytes(), new_text.as_bytes());
    let pre = common_prefix(ob, nb);
    if pre == ob.len() && pre == nb.len() {
        return Some(Delta { ws: new_text.len(), same_images: true, ..Default::default() });
    }
    let suf = common_suffix(ob, nb, ob.len().min(nb.len()) - pre);
    let (old_changed, new_changed) = (pre..ob.len() - suf, pre..nb.len() - suf);
    if old_changed.len() > MAX_EDIT || new_changed.len() > MAX_EDIT {
        return decline(2);
    }
    // Dokumentets grundformat och avsnitt står på/styrs av rader som en liten ändring kan ändra.
    let first_line_end = line_end(ob, 0);
    if pre <= first_line_end || old_text.contains("kladd: avsnitt") || new_text.contains("kladd: avsnitt") {
        return decline(3);
    }
    let delta = nb.len() as isize - ob.len() as isize;

    // Fönstret: stycket med ändringen och ett stycke på var sida (kanariefåglar).
    let edit_para = para_start(ob, pre)?;
    let mut ws = if edit_para == 0 { 0 } else { para_start(ob, edit_para - 1)? };
    let mid_end = para_end(ob, old_changed.end.min(ob.len().saturating_sub(1)).max(pre))?.max(old_changed.end);
    // Stycket efter: hoppa över blankraderna och ta nästa stycke.
    let mut we_old = if mid_end >= ob.len() {
        ob.len()
    } else {
        let mut ls = line_start(ob, mid_end);
        while ls < ob.len() && blank(ob, ls, line_end(ob, ls)) {
            ls = line_end(ob, ls) + 1;
        }
        if ls >= ob.len() { ob.len() } else { para_end(ob, ls)? }
    };
    // Blankrader hör till de närliggande bitarna: fönstret ska börja och sluta mellan bitar.
    let mut i0 = old.pieces.partition_point(|p| p.range.start < ws);
    if i0 > 0 && old.pieces[i0 - 1].range.end > ws {
        ws = old.pieces[i0 - 1].range.start;
        i0 -= 1;
    }
    let mut i1 = old.pieces.partition_point(|p| p.range.start < we_old);
    if i1 > 0 && old.pieces[i1 - 1].range.end > we_old {
        we_old = old.pieces[i1 - 1].range.end;
    }
    i1 = old.pieces.partition_point(|p| p.range.start < we_old);
    if we_old - ws > MAX_WINDOW {
        return decline(4);
    }
    let we_new = (we_old as isize + delta) as usize;
    if we_new < ws || we_new > nb.len() || !new_text.is_char_boundary(ws) || !new_text.is_char_boundary(we_new) {
        return decline(5);
    }
    let (old_win, new_win) = (&old_text[ws..we_old], &new_text[ws..we_new]);
    if risky(old_win) || risky(new_win) {
        return decline(6);
    }
    if (i0 > 0 && old.pieces[i0 - 1].range.end > ws) || (i1 > 0 && old.pieces[i1 - 1].range.end > we_old) {
        return decline(7);
    }
    let mut mid: Vec<Piece> = markup::pieces_of(new_win);
    for p in &mut mid {
        *p = shifted(p, 0, ws);
    }
    // Kontroll: bitarna före och efter ändringen (utom de som gränsar till den) ska bli desamma
    // som i den gamla analysen; annars har ändringen påverkat något över styckegränsen.
    let pre_n = old.pieces.partition_point(|p| p.range.end < pre).saturating_sub(i0);
    if mid.len() < pre_n || old.pieces[i0..i0 + pre_n] != mid[..pre_n] {
        return decline(8);
    }
    let jb = old.pieces.partition_point(|p| p.range.start <= old_changed.end).max(i0 + pre_n);
    let post_n = i1.saturating_sub(jb);
    if mid.len() < pre_n + post_n {
        return decline(9);
    }
    let tail = &mid[mid.len() - post_n..];
    for (o, n) in old.pieces[jb..i1].iter().zip(tail) {
        if shifted(o, delta, 0) != *n {
            return decline(10);
        }
    }
    if we_old >= ob.len() && mid.last().is_some_and(|p| p.range.end != new_text.len()) {
        return decline(11);
    }
    // Täckning utan glapp (som `analyze` ger); annars full analys.
    if (i0 == 0 && mid.first().is_some_and(|p| p.range.start != 0)) || (i1 == old.pieces.len() && mid.last().is_some_and(|p| p.range.end != new_text.len())) {
        return decline(12);
    }
    let window = &old.pieces[i0..i1];
    let delta_info = Delta {
        ws,
        touched_table: window.iter().chain(mid.iter()).any(|p| p.block == markup::Block::Table),
        same_images: window.iter().filter_map(|p| p.image.as_ref()).eq(mid.iter().filter_map(|p| p.image.as_ref())),
        fonts: super::layout::needed_fonts(&Analysis { pieces: mid.clone(), defaults: old.defaults.clone(), sections: Vec::new() }),
    };
    // Först nu ändras analysen: bitarna efter fönstret flyttas på plats, fönstrets byts ut.
    if delta != 0 {
        for p in &mut old.pieces[i1..] {
            p.range = (p.range.start as isize + delta) as usize..(p.range.end as isize + delta) as usize;
        }
    }
    old.pieces.splice(i0..i1, mid);
    Some(delta_info)
}

/// Hjälp för tester och mätning: ändringens byteintervall mellan två texter.
#[allow(dead_code)]
pub fn changed_range(old: &str, new: &str) -> Range<usize> {
    let pre = common_prefix(old.as_bytes(), new.as_bytes());
    let suf = common_suffix(old.as_bytes(), new.as_bytes(), old.len().min(new.len()) - pre);
    pre..new.len() - suf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::bench::Rng;

    fn doc(r: &mut Rng, paras: usize, rich: bool) -> String {
        let mut s = String::from("<!-- kladd: sida=a4 -->\n\n");
        let words = ["alfa", "beta", "gamma", "delta", "åäö", "text", "rad", "stycke"];
        for i in 0..paras {
            let kind = if rich { r.below(14) } else { r.below(4) };
            match kind {
                0 | 1 | 2 => {
                    for k in 0..(3 + r.below(12)) {
                        let w = words[r.below(words.len())];
                        match r.below(12) {
                            0 => s.push_str(&format!("**{w}** ")),
                            1 => s.push_str(&format!("*{w}* ")),
                            2 => s.push_str(&format!("<u>{w}</u> ")),
                            3 => s.push_str(&format!("[{w}](http://x.se/{k}) ")),
                            _ => s.push_str(&format!("{w} ")),
                        }
                    }
                }
                3 => s.push_str(&format!("## Rubrik {i}")),
                4 => s.push_str("- punkt ett\n- punkt två\n- punkt tre"),
                5 => s.push_str("1. först\n2. sedan"),
                6 => s.push_str("| a | b |\n| --- | --- |\n| 1 | 2 |"),
                7 => s.push_str("> citat rad\n> mer"),
                8 => s.push_str("kod ** här"),
                9 => s.push_str("<span style=\"color:#ff0000\">röd text</span> och mer"),
                10 => s.push_str("<div align=\"center\">\n\nmittad\n\n</div>"),
                11 => s.push_str("text med fotnot"),
                12 => s.push_str("    indragen kod"),
                _ => s.push_str("![bild](a.png)"),
            }
            s.push_str("\n\n");
        }
        s
    }

    /// Slumpmässiga små ändringar: antingen blir resultatet exakt `analyze`, eller så avstår
    /// inkrementell analys (`None`).
    fn text_around(t: &str, p: &[Piece], i: usize) -> String {
        t.get(p[i].range.clone()).unwrap_or("").to_owned()
    }

    fn fuzz(rich: bool, seed: u64) -> (usize, usize) {
        let mut r = Rng(seed);
        let mut text = doc(&mut r, 120, rich);
        let mut a = markup::analyze(&text);
        let (mut used, mut declined) = (0, 0);
        let edits = ["x", " ", "\n", "\n\n", "**", "*", "<u>", "</u>", "`", "```", "<", ">", "[", "]", "(", ")", "#", "- ", "| ", "ä", "<span>", "</span>", "<div>", "</div>", "\n    ", "[^", "]:"];
        for _ in 0..400 {
            let mut t = text.clone();
            let at = {
                let mut p = r.below(t.len());
                while !t.is_char_boundary(p) {
                    p -= 1;
                }
                p
            };
            match r.below(3) {
                0 => t.insert_str(at, edits[r.below(edits.len())]),
                1 => {
                    let mut e = (at + 1 + r.below(6)).min(t.len());
                    while !t.is_char_boundary(e) {
                        e += 1;
                    }
                    t.replace_range(at..e, "");
                }
                _ => t.insert(at, 'x'),
            }
            if ["```", "~~~", "[^", "]:"].iter().any(|m| t.contains(m)) {
                continue;
            }
            let full = markup::analyze(&t);
            let mut inc = a.clone();
            match incremental(&text, &mut inc, &t, 0) {
                Some(_) => {
                    if inc.pieces != full.pieces {
                        let i = inc.pieces.iter().zip(&full.pieces).position(|(a, b)| a != b).unwrap_or(0);
                        let lo = text_around(&t, &full.pieces, i);
                        panic!("första skillnad vid bit {i} ({} vs {} bitar): inc {:?} full {:?}; text {lo:?}; ändring {:?} i {:?}", inc.pieces.len(), full.pieces.len(), inc.pieces.get(i), full.pieces.get(i), changed_range(&text, &t), &t[at.saturating_sub(120)..(at + 120).min(t.len())]);
                    }
                    assert_eq!(inc.pieces, full.pieces, "avvikelse efter ändring vid {at} ({:?})", changed_range(&text, &t));
                    assert_eq!(inc.defaults, full.defaults);
                    assert_eq!(inc.sections, full.sections);
                    used += 1;
                }
                None => declined += 1,
            }
            text = t;
            a = full;
        }
        WHY.with(|w| {
            let mut c = std::collections::BTreeMap::new();
            for x in w.borrow().iter() {
                *c.entry(*x).or_insert(0) += 1;
            }
            eprintln!("avstår: {c:?}");
        });
        (used, declined)
    }

    #[test]
    fn incremental_equals_full_on_plain_documents() {
        let (used, _) = fuzz(false, 11);
        assert!(used > 100, "den inkrementella vägen används ({used})");
    }

    #[test]
    fn incremental_equals_full_on_rich_documents() {
        for seed in [3, 5, 8, 13] {
            fuzz(true, seed);
        }
    }

    #[test]
    fn small_documents_and_big_edits_use_full_analysis() {
        let mut r = Rng(1);
        let text = doc(&mut r, 20, false);
        let a = markup::analyze(&text);
        assert!(analyze_incremental(&text, &mut a.clone(), &format!("{text}x")).is_none(), "under tröskeln");
        let big = doc(&mut r, 8000, false);
        let ab = markup::analyze(&big);
        assert!(big.len() > MIN_BYTES);
        let mut t = big.clone();
        t.insert_str(big.len() / 2, "x");
        assert!(analyze_incremental(&big, &mut ab.clone(), &t).is_some());
        let mut u = big.clone();
        u.insert_str(big.len() / 2, &"y".repeat(MAX_EDIT + 1));
        assert!(analyze_incremental(&big, &mut ab.clone(), &u).is_none(), "stor ändring");
        // Ändring på första raden (grundformatet).
        let mut f = big.clone();
        f.insert(3, 'q');
        assert!(analyze_incremental(&big, &mut ab.clone(), &f).is_none());
    }

    #[test]
    fn open_fence_across_the_boundary_falls_back() {
        let mut r = Rng(2);
        let mut text = doc(&mut r, 8000, false);
        let mid = text.len() / 2;
        let at = text[mid..].find("\n\n").unwrap() + mid + 2;
        text.insert_str(at, "kod\n\n");
        let a = markup::analyze(&text);
        // Att öppna ett kodstaket påverkar allt efter det: aldrig en inkrementell väg.
        let mut t = text.clone();
        t.insert_str(at, "```");
        assert!(analyze_incremental(&text, &mut a.clone(), &t).is_none());
    }
}
