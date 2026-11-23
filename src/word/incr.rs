//! Inkrementell analys för stora dokument (hundratals sidor): efter en liten ändring tolkas bara
//! stycket med ändringen, plus ett stycke på var sida som "kanariefågel", i stället för hela texten.
//!
//! Resultatet ska vara exakt detsamma som `markup::analyze`. Därför återanvänds det gamla bara när
//! ändringen är liten och lokal och när kanariestyckena fick samma bitar som i den gamla analysen
//! (annars har ändringen påverkat något som sträcker sig över styckegränsen, t.ex. ett kodstaket
//! eller en `<span>`, och hela texten tolkas om). Konstruktioner som är beroende av hela dokumentet
//! (referenslänkar, kodstaket, indragen kod) ger alltid full analys. Fotnoter och avsnitt, som är vanliga i
//! Word-dokument, går bra så länge fönstret inte rör dem (fotnotsreferenser och -definitioner och avsnittsrader
//! finns bara i fönster som innehåller `[^` respektive `avsnitt`, och de ger full analys).

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
    #[cfg(test)]
    if DISABLED.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
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

/// Finns det en oavslutad (eller överflödigt avslutad) HTML-kommentar? En sådan sväljer text över blankrader och ändrar
/// tolkningen av stycken långt från ändringen.
fn html_comments_open(t: &str) -> bool {
    let b = t.as_bytes();
    t.match_indices("<!--").any(|(i, _)| !t[i..line_end(b, i)].contains("-->"))
}

/// HTML-block som kan fortsätta över blankrader (oavslutad kommentar, `<pre>`, `<script>` …) och därför gör att något
/// utanför fönstret kan tolkas annorlunda när en ändring avslutar eller öppnar dem.
fn html_ctx(w: &str) -> bool {
    if w.matches("<!--").count() != w.matches("-->").count() {
        return true;
    }
    if !w.contains('<') {
        return false;
    }
    let l = w.to_ascii_lowercase();
    ["<pre", "<script", "<style", "<textarea", "<?", "<![cdata["].iter().any(|m| l.contains(m))
}

/// Står det en fotnotsdefinition (`[^n]:` först på en rad) i `t[lo..hi]` eller på 4 kB när den?
fn near_def(t: &[u8], lo: usize, hi: usize) -> bool {
    const NEAR: usize = 4096;
    let (lo, hi) = (lo.saturating_sub(NEAR), (hi + NEAR).min(t.len()));
    t[lo..hi].windows(2).enumerate().any(|(i, w)| w == b"]:" && t[line_start(t, lo + i)..].starts_with(b"[^"))
}

/// Bitarna i `pieces` (sorterade, utan glapp) klippta till `from..to`.
fn clip(pieces: &[Piece], from: usize, to: usize) -> Vec<Piece> {
    let i0 = pieces.partition_point(|p| p.range.end <= from);
    let mut out = Vec::new();
    for p in &pieces[i0..] {
        if p.range.start >= to {
            break;
        }
        let mut q = p.clone();
        q.range = p.range.start.max(from)..p.range.end.min(to);
        out.push(q);
    }
    out
}

/// Finns det referenslänkar (`[namn]: adress`)? De ändrar tolkningen av `[text][namn]` var som helst i dokumentet.
/// Fotnotsdefinitioner (`[^n]: text`) räknas inte.
fn has_link_defs(text: &str) -> bool {
    let b = text.as_bytes();
    text.match_indices("]:").any(|(i, _)| !b[line_start(b, i)..].starts_with(b"[^"))
}

/// Finns det text (utanför grundformatsraden på första raden) i `prefix`? Avsnittsscanningen räknar en avsnittsrad
/// som första avsnittet om ingen text föregår den.
fn has_content_before(prefix: &str) -> bool {
    prefix.split_inclusive('\n').enumerate().any(|(i, l)| {
        let t = l.trim();
        !t.is_empty() && !(i == 0 && super::markup::DocDefaults::from_comment(t).is_some())
    })
}

/// Konstruktioner vars tolkning beror på resten av dokumentet: då görs full analys.
fn risky(s: &str) -> bool {
    if s.contains("```") || s.contains("~~~") || s.contains("]:") {
        return true;
    }
    // Indragen kod och fortsättningsrader i listor.
    s.starts_with("    ") || s.starts_with('\t') || s.contains("\n    ") || s.contains("\n\t")
}

/// Etiketterna i fotnotsreferenserna (`[^etikett]`) i `w`; `None` om någon är ofullständig eller konstig.
fn note_labels(w: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for (i, _) in w.match_indices("[^") {
        let rest = &w[i + 2..];
        let end = rest.find(']')?;
        let l = &rest[..end];
        if l.is_empty() || l.len() > 40 || l.chars().any(|c| c.is_whitespace() || matches!(c, '[' | ']' | '(' | ')' | '\\')) {
            return None;
        }
        out.push(l.to_owned());
    }
    Some(out)
}

/// Finns definitionen `[^etikett]:` först på en rad i texten?
fn has_def(t: &[u8], label: &str) -> bool {
    let pat = format!("\n[^{label}]:");
    t.starts_with(&pat.as_bytes()[1..]) || t.windows(pat.len()).any(|w| w == pat.as_bytes())
}

fn shifted(p: &Piece, delta: isize, base: usize) -> Piece {
    let mut q = p.clone();
    let mv = |x: usize| (x as isize + delta) as usize + base;
    q.range = mv(p.range.start)..mv(p.range.end);
    q
}

/// Mätningar (`bench`): slår av den inkrementella analysen för att jämföra med full analys vid varje ändring.
#[cfg(test)]
pub static DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    // Kodstaket och referenslänkar påverkar tolkningen över hela dokumentet (fotnotsdefinitioner `[^n]: text` gör inte det).
    if new_text.contains("```") || new_text.contains("~~~") || has_link_defs(new_text) || html_comments_open(new_text) {
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
    // Dokumentets grundformat står på första raden, som en liten ändring kan ändra.
    let first_line_end = line_end(ob, 0);
    if pre <= first_line_end {
        return decline(3);
    }
    // Ett första avsnitt som tagit över en avsnittsrad kan ändras av text som skrivs före raden.
    if old.sections.first().is_some_and(|s| s.marker.is_some()) {
        return decline(14);
    }
    let delta = nb.len() as isize - ob.len() as isize;

    // Fönstret: stycket med ändringen och ett stycke på var sida (kanariefåglar).
    let edit_para = para_start(ob, pre)?;
    let mut ws = if edit_para == 0 { 0 } else { para_start(ob, edit_para - 1)? };
    let mid_end = para_end(ob, old_changed.end.min(ob.len().saturating_sub(1)).max(pre))?.max(old_changed.end);
    // Stycket efter: hoppa över blankraderna och ta nästa stycke.
    let next_end = |from: usize| -> Option<usize> {
        if from >= ob.len() {
            return Some(ob.len());
        }
        let mut ls = line_start(ob, from);
        while ls < ob.len() && blank(ob, ls, line_end(ob, ls)) {
            ls = line_end(ob, ls) + 1;
        }
        if ls >= ob.len() { Some(ob.len()) } else { para_end(ob, ls) }
    };
    let mut we_old = next_end(mid_end)?;
    // `<div align>` runt stycken (justerad text i Word-dokument): fönstret ska innehålla hela `<div>`-block, annars
    // syns inte justeringen vid tolkning av fönstret för sig. Det vidgas till stycken tills blocken är hela.
    for _ in 0..8 {
        let w = &old_text[ws..we_old];
        let (open, close) = (w.matches("<div").count(), w.matches("</div").count());
        if open == close {
            break;
        }
        if open > close {
            if we_old >= ob.len() {
                return decline(16);
            }
            we_old = next_end(we_old)?;
        } else if ws == 0 {
            return decline(16);
        } else {
            ws = para_start(ob, ws - 1)?;
        }
    }
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
    // Giltigheten av en fotnotsreferens beror på att definitionen står först i ett stycke var som helst i dokumentet:
    // en ändring nära en definition (i praktiken längst ned i dokumentet) ger full analys.
    if near_def(ob, ws, we_old) || near_def(nb, ws, we_new) {
        return decline(17);
    }
    // Avsnittsrader i fönstret (eller en ny): avsnitten räknas om med full analys.
    if old_win.contains("avsnitt") || new_win.contains("avsnitt") {
        return decline(3);
    }
    // Texten före första avsnittsraden får inte ha blivit tom (då blir raden det första avsnittet).
    if let Some(m) = old.sections.iter().skip(1).find_map(|s| s.marker.as_ref()).map(|m| m.start) {
        if pre < m && !has_content_before(&new_text[..(m as isize + delta) as usize]) {
            return decline(14);
        }
    }
    // Fönstret slutar mellan stycken, men bitar slås ihop över styckegränser (vanlig text i flera stycken är en bit).
    // De gamla bitarna i fönstret klipps därför vid kanterna; delarna utanför behålls som egna bitar.
    let i0 = old.pieces.partition_point(|p| p.range.end <= ws);
    let i1 = old.pieces.partition_point(|p| p.range.start < we_old);
    if i0 >= i1 {
        return decline(7);
    }
    let mut ow: Vec<Piece> = old.pieces[i0..i1].to_vec();
    let head = (old.pieces[i0].range.start < ws).then(|| Piece { range: old.pieces[i0].range.start..ws, ..old.pieces[i0].clone() });
    let tail_piece = (old.pieces[i1 - 1].range.end > we_old)
        .then(|| shifted(&Piece { range: we_old..old.pieces[i1 - 1].range.end, ..old.pieces[i1 - 1].clone() }, delta, 0));
    if let Some(f) = ow.first_mut() {
        f.range.start = f.range.start.max(ws);
    }
    if let Some(l) = ow.last_mut() {
        l.range.end = l.range.end.min(we_old);
    }
    // Fortfarande ohela `<div>`-block (i den nya texten eller efter fönstrets vidgning): full analys.
    let unbalanced = |w: &str| w.matches("<div").count() != w.matches("</div").count();
    if unbalanced(old_win) || unbalanced(new_win) {
        return decline(16);
    }
    if html_ctx(old_win) || html_ctx(new_win) {
        return decline(18);
    }
    if ow.iter().any(|p| p.align != markup::Align::Left) && !old_win.contains("align") {
        return decline(19);
    }
    // Fotnotsreferenser (`[^n]`) tolkas bara om definitionen finns i dokumentet: fönstret tolkas med en definition per
    // etikett efter sig (som finns i texten, annars full analys) och bitarna efter fönstret kastas.
    let (Some(l_old), Some(l_new)) = (note_labels(old_win), note_labels(new_win)) else { return decline(20) };
    let mut labels = l_old;
    labels.extend(l_new);
    labels.sort_unstable();
    labels.dedup();
    if labels.iter().any(|l| !has_def(nb, l)) {
        return decline(20);
    }
    let mut mid: Vec<Piece> = if labels.is_empty() {
        markup::pieces_of(new_win)
    } else {
        let src = format!("{new_win}{}", labels.iter().map(|l| format!("\n\n[^{l}]: x")).collect::<String>());
        let mut v = markup::pieces_of(&src);
        v.retain(|p| p.range.start < new_win.len());
        if let Some(l) = v.last_mut() {
            l.range.end = l.range.end.min(new_win.len());
        }
        v
    };
    for p in &mut mid {
        *p = shifted(p, 0, ws);
    }
    // Kontroll: bitarna i raderna före och efter raden med ändringen ska bli desamma som i den gamla analysen (klippta
    // vid raderna, så att en lång bit som sträcker sig över dem också jämförs); annars har ändringen eller fönstrets
    // kant påverkat något som sträcker sig över styckegränsen.
    let before_end = line_start(ob, pre).max(ws);
    if clip(&ow, ws, before_end) != clip(&mid, ws, before_end) {
        return decline(8);
    }
    let after_old_start = (line_end(ob, old_changed.end.min(ob.len())) + 1).min(we_old);
    let after_new_start = (after_old_start as isize + delta) as usize;
    let after_old: Vec<Piece> = clip(&ow, after_old_start, we_old).iter().map(|p| shifted(p, delta, 0)).collect();
    if after_new_start > we_new || after_old != clip(&mid, after_new_start, we_new) {
        return decline(10);
    }
    // Täckning utan glapp (som `analyze` ger); annars full analys.
    if mid.first().is_none_or(|p| p.range.start != ws) || mid.last().is_none_or(|p| p.range.end != we_new) {
        return decline(12);
    }
    let delta_info = Delta {
        ws,
        touched_table: ow.iter().chain(mid.iter()).any(|p| p.block == markup::Block::Table),
        same_images: ow.iter().filter_map(|p| p.image.as_ref()).eq(mid.iter().filter_map(|p| p.image.as_ref())),
        fonts: super::layout::needed_fonts(&Analysis { pieces: mid.clone(), defaults: old.defaults.clone(), sections: Vec::new() }),
    };
    // Först nu ändras analysen: bitarna efter fönstret flyttas på plats, fönstrets byts ut.
    if delta != 0 {
        for p in &mut old.pieces[i1..] {
            p.range = (p.range.start as isize + delta) as usize..(p.range.end as isize + delta) as usize;
        }
    }
    let (n_head, n_mid) = (usize::from(head.is_some()), mid.len());
    let mut repl: Vec<Piece> = head.into_iter().chain(mid).chain(tail_piece).collect();
    let n_tail = repl.len() - n_head - n_mid;
    // Bitar med samma egenskaper bredvid varandra är en bit i `analyze`: slå ihop skarvarna.
    let same = |a: &Piece, b: &Piece| a.range.end == b.range.start && Piece { range: a.range.clone(), ..b.clone() } == *a;
    let joint = if n_tail == 1 { n_head + n_mid } else { repl.len() };
    let merge_right = if joint < repl.len() { same(&repl[joint - 1], &repl[joint]) } else { old.pieces.get(i1).is_some_and(|nx| same(&repl[repl.len() - 1], nx)) };
    let mut i1 = i1;
    if merge_right {
        if joint < repl.len() {
            let end = repl[joint].range.end;
            repl[joint - 1].range.end = end;
            repl.remove(joint);
        } else {
            let end = old.pieces[i1].range.end;
            let last = repl.len() - 1;
            repl[last].range.end = end;
            i1 += 1;
        }
    }
    let first_mid = n_head;
    let left_last = if n_head == 1 { Some(repl[0].clone()) } else { i0.checked_sub(1).map(|k| old.pieces[k].clone()) };
    if left_last.as_ref().is_some_and(|l| same(l, &repl[first_mid])) {
        let end = repl[first_mid].range.end;
        if n_head == 1 {
            repl[0].range.end = end;
            repl.remove(1);
        } else if let Some(k) = i0.checked_sub(1) {
            old.pieces[k].range.end = end;
            repl.remove(0);
        }
    }
    old.pieces.splice(i0..i1, repl);
    // Avsnitten efter fönstret flyttas med texten (fönstret har inga avsnittsrader).
    if delta != 0 {
        for sec in old.sections.iter_mut().filter(|s| s.start >= we_old) {
            sec.start = (sec.start as isize + delta) as usize;
            if let Some(m) = sec.marker.as_mut() {
                *m = (m.start as isize + delta) as usize..(m.end as isize + delta) as usize;
            }
        }
    }
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
        let mut notes = 0;
        for i in 0..paras {
            let kind = if rich { r.below(18) } else { r.below(4) };
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
                14 | 15 => {
                    notes += 1;
                    s.push_str(&format!("text med fotnot[^{notes}] och mer text"));
                }
                16 => s.push_str("<!-- kladd: avsnitt; columns: 2; type: continuous -->"),
                _ => s.push_str("![bild](a.png)"),
            }
            s.push_str("\n\n");
        }
        for n in 1..=notes {
            s.push_str(&format!("[^{n}]: Fotnot {n} med text.\n\n"));
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
            if ["```", "~~~"].iter().any(|m| t.contains(m)) || has_link_defs(&t) {
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
        let n: u64 = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        for seed in [3, 5, 8, 13, 21, 34].into_iter().chain(100..100 + n) {
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

    /// Word-dokument har fotnoter och avsnitt: en vanlig skrivändring ska ändå vara inkrementell och ge exakt `analyze`.
    #[test]
    fn footnotes_and_sections_keep_the_incremental_path() {
        let mut r = Rng(9);
        let mut text = doc(&mut r, 6000, false);
        let mut mid = text.len() / 2;
        while !text.is_char_boundary(mid) {
            mid += 1;
        }
        let at = text[mid..].find("\n\n").unwrap() + mid + 2;
        text.insert_str(at, "Med fotnot[^1] här.\n\n<!-- kladd: avsnitt; columns: 2; type: continuous -->\n\nTvå spalter.\n\n<!-- kladd: avsnitt; type: continuous -->\n\n");
        text.push_str("[^1]: Fotnotstext.\n\n");
        let a = markup::analyze(&text);
        assert_eq!(a.sections.len(), 3);
        for frac in [0.1, 0.3, 0.45, 0.6, 0.9] {
            let mut p = (text.len() as f64 * frac) as usize;
            while !text.is_char_boundary(p) || text.as_bytes()[p] == b'\n' {
                p += 1;
            }
            let mut t = text.clone();
            t.insert(p, 'x');
            let mut inc = a.clone();
            let d = analyze_incremental(&text, &mut inc, &t);
            assert!(d.is_some(), "bit {frac}: ska vara inkrementell ({:?})", WHY.with(|w| w.borrow().iter().rev().take(3).copied().collect::<Vec<_>>()));
            let full = markup::analyze(&t);
            assert_eq!(inc.pieces, full.pieces, "bit {frac}");
            assert_eq!(inc.sections, full.sections, "avsnitten flyttas med texten ({frac})");
        }
        // Rör ändringen en avsnittsrad görs full analys (och en ändring vid en fotnotsreferens ger samma bitar som `analyze`).
        let fnr = text.find("[^1] här").unwrap();
        let mut t = text.clone();
        t.insert(fnr + 1, 'x');
        let mut inc = a.clone();
        if analyze_incremental(&text, &mut inc, &t).is_some() {
            assert_eq!(inc.pieces, markup::analyze(&t).pieces);
        }
        let sec = text.find("<!-- kladd: avsnitt; columns").unwrap();
        let mut t = text.clone();
        t.insert(sec + 6, 'x');
        assert!(analyze_incremental(&text, &mut a.clone(), &t).is_none());
        // Referenslänkar gäller hela dokumentet.
        let mut t = text.clone();
        t.push_str("[namn]: http://x.se\n\n");
        let mut u = t.clone();
        u.insert(at - 3, 'x');
        assert!(analyze_incremental(&t, &mut markup::analyze(&t), &u).is_none());
    }
}
