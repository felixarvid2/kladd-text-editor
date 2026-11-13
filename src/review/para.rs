//! Spårade stycke- och radbrytningar (`<!--k:pm ins 5-->` / `<!--k:pm del 5-->`): Enter, Backspace och
//! Delete över en radbrytning, och hopslagning när en infogad brytning avvisas eller en struken
//! godkänns. Se `docs/dev/dokumentlage.md`, avsnittet Granska.
//!
//! Markören står sist på raden, direkt före radbrytningen (en eller flera `\n`), och gäller den.
//! `n=` anger antalet radbrytningar Kladd själv infogade (så att en avvisning tar bort just dem);
//! en markör utan `n` (från Word) gäller hela brytningen.

use std::ops::Range;

use super::{PMark, RevKind, RevRec, Scan, Tracked, floor_char, in_code_fence, next_rev_id, pmark_text, prefix_len, rewrite_records, scan, token_ranges};

/// Radbrytningarna (`\n`) som börjar på `at` (antal).
fn run_at(text: &str, at: usize) -> usize {
    text[at..].bytes().take_while(|&b| b == b'\n').count()
}

/// Början på radbrytningsföljden som `pos` ligger i eller vid, och dess slut.
fn run_around(text: &str, pos: usize) -> Range<usize> {
    let b = text.as_bytes();
    let mut s = pos;
    while s > 0 && b[s - 1] == b'\n' {
        s -= 1;
    }
    let mut e = pos;
    while e < b.len() && b[e] == b'\n' {
        e += 1;
    }
    s..e
}

/// Är raden som `pos` ligger på en tabellrad? (Radbrytningar i tabeller spåras inte.)
fn in_table_line(text: &str, pos: usize) -> bool {
    let s = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    text[s..].trim_start().starts_with('|')
}

/// Markören som direkt föregår radbrytningsföljden som börjar på `run_start` (bara dolda taggar och
/// markörer emellan), om den är ett `ins` av `author`.
fn own_ins_before<'a>(text: &str, sc: &'a Scan, tokens: &[Range<usize>], run_start: usize, author: &str) -> Option<&'a PMark> {
    marker_before(text, sc, tokens, run_start, |m| m.kind == RevKind::Ins && sc.rev_rec(m.id).is_some_and(|r| r.author == author))
}

fn marker_before<'a>(text: &str, sc: &'a Scan, tokens: &[Range<usize>], run_start: usize, pick: impl Fn(&PMark) -> bool) -> Option<&'a PMark> {
    let hidden = |a: usize, b: usize| {
        let mut p = a;
        while p < b {
            match tokens.iter().find(|t| t.start == p) {
                Some(t) => p = t.end,
                None => return false,
            }
        }
        let _ = text;
        true
    };
    sc.pmarks.iter().rev().filter(|m| m.range.end <= run_start && m.kind != RevKind::Fmt).find(|m| hidden(m.range.end, run_start)).filter(|m| pick(m))
}

fn finish(out: String, caret: usize, sc: &Scan, new_recs: Vec<RevRec>) -> Tracked {
    let mut revs: Vec<RevRec> = sc.rev_recs.iter().map(|(r, _)| r.clone()).collect();
    revs.extend(new_recs);
    let comments = sc.comment_recs.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>();
    let (text, caret) = rewrite_records(&out, caret, &revs, &comments);
    Tracked { text, caret }
}

/// En redigering som bara infogar eller tar bort radbrytningar (Enter, Backspace, Delete) görs om till
/// en spårad: en infogad brytning får `<!--k:pm ins-->`, en struken `<!--k:pm del-->`. `None` om
/// redigeringen inte är en ren radbrytning (den hanteras då som vanligt) eller inte kan spåras.
pub(super) fn track_newlines(old: &str, new: &str, old_sel: &Range<usize>, author: &str, date: &str) -> Option<Tracked> {
    let mut p = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    p = floor_char(old, floor_char(new, p));
    let max_s = old.len().min(new.len()) - p;
    let mut s = old.bytes().rev().zip(new.bytes().rev()).take(max_s).take_while(|(a, b)| a == b).count();
    while s > 0 && !(old.is_char_boundary(old.len() - s) && new.is_char_boundary(new.len() - s)) {
        s -= 1;
    }
    let reg = p..old.len() - s;
    let inserted = &new[p..new.len() - s];
    let removed = &old[reg.clone()];
    let only_nl = |t: &str| !t.is_empty() && t.bytes().all(|b| b == b'\n');
    let insert = reg.is_empty() && only_nl(inserted);
    let delete = inserted.is_empty() && only_nl(removed);
    if !insert && !delete {
        return None;
    }
    let sc = scan(old);
    let tokens = token_ranges(&sc);
    let in_token = |i: usize| tokens.iter().any(|t| t.start < i && i < t.end);
    let mut new_recs: Vec<RevRec> = Vec::new();
    let mut next_id = next_rev_id(&sc);
    let mut alloc = || {
        let id = next_id;
        next_id += 1;
        new_recs.push(RevRec { id, author: author.to_owned(), date: date.to_owned(), old: None });
        id
    };
    let own = |id: u32| sc.rev_rec(id).is_some_and(|r| r.author == author);

    if insert {
        let k = inserted.len();
        // Där Enter trycktes: markören före redigeringen om den passar, annars där skillnaden börjar.
        let mut q = old_sel.start;
        if old_sel.start != old_sel.end || q > old.len() || !old.is_char_boundary(q) || format!("{}{inserted}{}", &old[..q], &old[q..]) != new {
            q = reg.start;
        }
        if in_token(q) {
            q = tokens.iter().find(|t| t.start < q && q < t.end).map_or(q, |t| t.end);
        }
        if in_code_fence(old, q) || in_table_line(old, q) {
            return None;
        }
        let nl = "\n".repeat(k);
        let run = run_around(old, q);
        if run.is_empty() {
            // Inne i en egen infogning delas den, så att en ändring aldrig går över en radbrytning.
            let own_ins = sc.revs.iter().find(|r| r.kind == RevKind::Ins && own(r.id) && r.inner().start <= q && q <= r.inner().end);
            let id = alloc();
            let mark = pmark_text(RevKind::Ins, id, Some(k as u32));
            let (out, caret) = match own_ins {
                Some(r) if q == r.inner().end => {
                    let at = r.close.end;
                    (format!("{}{mark}{nl}{}", &old[..at], &old[at..]), at + mark.len() + k)
                }
                Some(r) if q == r.inner().start => {
                    let at = r.open.start;
                    (format!("{}{mark}{nl}{}", &old[..at], &old[at..]), at + mark.len() + k)
                }
                Some(r) => {
                    let reopen = super::open_tag(RevKind::Ins, r.id);
                    let close = super::close_tag(RevKind::Ins);
                    (format!("{}{close}{mark}{nl}{reopen}{}", &old[..q], &old[q..]), q + close.len() + mark.len() + k + reopen.len())
                }
                None => (format!("{}{mark}{nl}{}", &old[..q], &old[q..]), q + mark.len() + k),
            };
            // Markören hamnar efter radens sista tecken; en rad som börjar med den skyddas inte (ovanligt).
            return Some(finish(out, caret, &sc, new_recs));
        }
        // Intill en befintlig brytning: den räknas upp om den är vår egen infogning.
        if let Some(m) = own_ins_before(old, &sc, &tokens, run.start, author) {
            let out = match m.n {
                Some(n) => {
                    let mark = pmark_text(RevKind::Ins, m.id, Some(n + k as u32));
                    format!("{}{mark}{}{nl}{}", &old[..m.range.start], &old[m.range.end..q], &old[q..])
                }
                None => format!("{}{nl}{}", &old[..q], &old[q..]),
            };
            let caret = out.len() - (old.len() - q);
            return Some(finish(out, caret, &sc, new_recs));
        }
        if run.start == 0 {
            return None;
        }
        let id = alloc();
        let mark = pmark_text(RevKind::Ins, id, Some(k as u32));
        let out = format!("{}{mark}{}{nl}{}", &old[..run.start], &old[run.start..q], &old[q..]);
        let caret = out.len() - (old.len() - q);
        return Some(finish(out, caret, &sc, new_recs));
    }

    // Radering av radbrytningar (Backspace/Delete).
    let run = run_around(old, reg.start);
    if run.is_empty() || run.end < reg.end || in_code_fence(old, reg.start) || in_table_line(old, run.start) {
        return None;
    }
    let back = old_sel.end >= reg.end && old_sel.start != reg.start || old_sel.start == reg.end;
    let k = reg.len();
    if let Some(m) = own_ins_before(old, &sc, &tokens, run.start, author) {
        // Egen infogad brytning: den tas bort på riktigt.
        let left = run.len() - k;
        let n = m.n.unwrap_or(run.len() as u32);
        let new_n = n.saturating_sub(k as u32);
        let mark = if left == 0 || new_n == 0 { String::new() } else { pmark_text(RevKind::Ins, m.id, m.n.map(|_| new_n)) };
        let out = format!("{}{mark}{}{}", &old[..m.range.start], &old[m.range.end..reg.start], &old[reg.end..]);
        let caret = m.range.start + mark.len() + (reg.start - m.range.end);
        return Some(finish(out, caret, &sc, new_recs));
    }
    let next_prefix = {
        let ls = run.end;
        let le = old[ls..].find('\n').map_or(old.len(), |i| ls + i);
        prefix_len(&old[ls..le])
    };
    if let Some(m) = marker_before(old, &sc, &tokens, run.start, |m| m.kind == RevKind::Del) {
        // Redan struken: bara markören flyttas förbi.
        let caret = if back { m.range.start } else { run.end + next_prefix };
        return Some(finish(old.to_owned(), caret, &sc, new_recs));
    }
    if run.start == 0 {
        return None;
    }
    let id = alloc();
    let mark = pmark_text(RevKind::Del, id, None);
    let out = format!("{}{mark}{}", &old[..run.start], &old[run.start..]);
    let caret = if back { run.start } else { run.end + mark.len() + next_prefix };
    Some(finish(out, caret, &sc, new_recs))
}

/// Kan nästa rad slås ihop med raden före (är den vanlig text, rubrik, listpunkt eller citat)?
fn joinable(line: &str) -> bool {
    let t = line.trim_start();
    !(t.starts_with("<!--k:r ")
        || t.starts_with("<!--k:c ")
        || t.starts_with("```")
        || t.starts_with("~~~")
        || t.starts_with('|')
        || t.starts_with("<div")
        || t.starts_with("</div")
        || t.starts_with("<!-- kladd:")
        || t.starts_with("<img")
        || t == "---"
        || t == "***")
}

/// Markörer av slaget `fmt` i `s` (en rads slut) tas bort.
fn strip_fmt_marks(s: &str) -> String {
    let sc = scan(s);
    let mut cuts: Vec<Range<usize>> = sc.pmarks.iter().filter(|m| m.kind == RevKind::Fmt).map(|m| m.range.clone()).collect();
    super::cut_ranges(s, &mut cuts)
}

/// Slår ihop stycken där en infogad brytning avvisas (`accept == false`, markör `ins`) eller en struken
/// godkänns (`accept == true`, markör `del`) för de ändringar `pick` väljer. Markören och brytningen
/// (med nästa radprefix) försvinner; nästa rads text fortsätter på raden. Är nästa block något som
/// inte kan slås ihop (tabell, kodstaket, avsnittsrad) tas bara markören bort.
pub(super) fn join_marks(text: &str, accept: bool, pick: &dyn Fn(u32) -> bool) -> String {
    let want = if accept { RevKind::Del } else { RevKind::Ins };
    let mut text = text.to_owned();
    // Bakifrån, så att lägena före inte flyttas; varje varv tar bort en markör.
    loop {
        let sc = scan(&text);
        let Some(m) = sc.pmarks.iter().rev().find(|m| m.kind == want && pick(m.id)).cloned() else { break };
        text = join_one(&text, &m);
    }
    text
}

fn join_one(text: &str, m: &PMark) -> String {
    let le = text[m.range.end..].find('\n').map_or(text.len(), |i| m.range.end + i);
    let k = run_at(text, le);
    let tail = &text[m.range.end..le];
    // Antal brytningar att ta bort: de som Kladd infogade, annars hela följden.
    let n = if m.kind == RevKind::Ins { m.n.map_or(k, |n| (n as usize).min(k)) } else { k };
    if k == 0 || n == 0 {
        return format!("{}{tail}{}", &text[..m.range.start], &text[le..]);
    }
    let after = le + k;
    let next_end = text[after..].find('\n').map_or(text.len(), |i| after + i);
    let next = &text[after..next_end];
    let line_start = text[..m.range.start].rfind('\n').map_or(0, |i| i + 1);
    let this_line_ok = joinable(&text[line_start..m.range.start]) || text[line_start..m.range.start].trim().is_empty();
    if n < k {
        // Brytningen var bara en del av följden: resten stannar.
        return format!("{}{tail}{}{}", &text[..m.range.start], &text[le..le + (k - n)], &text[after..]);
    }
    if !this_line_ok || !joinable(next) {
        return format!("{}{tail}{}", &text[..m.range.start], &text[le..]);
    }
    // Ihopslagning: radens egen formatmarkör hör till brytningen som försvinner.
    let tail = strip_fmt_marks(tail);
    // Bara riktiga prefix (`#`, `-`, `1.`, `>`) tas bort, inte vanligt indrag.
    let pl = prefix_len(next).min(next.len());
    let skip = if next[..pl].trim().is_empty() { 0 } else { pl };
    format!("{}{tail}{}", &text[..m.range.start], &text[after + skip..])
}
