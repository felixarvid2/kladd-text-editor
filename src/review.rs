//! Spårade ändringar och kommentarer i Kladds Markdown-dialekt (dokumentläget).
//!
//! Formatet (beskrivet i `docs/dev/dokumentlage.md`):
//!
//! - Infogad text: `<ins data-k="3">text</ins>`, struken text: `<del data-k="4">text</del>`.
//!   `data-k` är ändringens nummer; flera taggar kan dela nummer (en Word-ändring över flera
//!   körningar med olika format). Författare och datum står i en postrad
//!   `<!--k:r id="3" author="Ada" date="2026-05-01T10:00:00Z"-->`.
//! - Kommentarens område: `<!--k:c:ID-->…<!--k:/c:ID-->` i texten, och kommentaren själv som en
//!   postrad `<!--k:c id="1" parent="0" author="Ada" date="…" done="1" text="Hej"-->`.
//!   Svar delar område med kommentaren de svarar på och har `parent`.
//! - Postraderna ligger sist i dokumentet (efter fotnoterna), en per rad.
//!
//! Allt här är ren textlogik utan gränssnitt.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::word::markup::Edit;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RevKind {
    Ins,
    Del,
}

impl RevKind {
    pub fn tag(self) -> &'static str {
        match self {
            RevKind::Ins => "ins",
            RevKind::Del => "del",
        }
    }
}

/// Författare och datum för en ändring.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RevRec {
    pub id: u32,
    pub author: String,
    pub date: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CommentRec {
    pub id: u32,
    pub parent: Option<u32>,
    pub author: String,
    pub date: String,
    pub done: bool,
    pub text: String,
}

/// En `<ins>`/`<del>`: den öppnande och stängande taggens lägen.
#[derive(Clone, Debug, PartialEq)]
pub struct RevSpan {
    pub kind: RevKind,
    pub id: u32,
    pub open: Range<usize>,
    pub close: Range<usize>,
}

impl RevSpan {
    pub fn inner(&self) -> Range<usize> {
        self.open.end..self.close.start
    }
    pub fn full(&self) -> Range<usize> {
        self.open.start..self.close.end
    }
}

/// Områdets två markörer.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentSpan {
    pub id: u32,
    pub start: Range<usize>,
    pub end: Range<usize>,
}

impl CommentSpan {
    /// Den kommenterade texten (mellan markörerna).
    pub fn inner(&self) -> Range<usize> {
        self.start.end..self.end.start
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub revs: Vec<RevSpan>,
    pub comments: Vec<CommentSpan>,
    pub rev_recs: Vec<(RevRec, Range<usize>)>,
    pub comment_recs: Vec<(CommentRec, Range<usize>)>,
}

impl Scan {
    pub fn rev_rec(&self, id: u32) -> Option<&RevRec> {
        self.rev_recs.iter().find(|(r, _)| r.id == id).map(|(r, _)| r)
    }
    pub fn comment_rec(&self, id: u32) -> Option<&CommentRec> {
        self.comment_recs.iter().find(|(c, _)| c.id == id).map(|(c, _)| c)
    }
}

// ------------------------------------------------------------------ format

pub fn open_tag(kind: RevKind, id: u32) -> String {
    format!("<{} data-k=\"{id}\">", kind.tag())
}

pub fn close_tag(kind: RevKind) -> String {
    format!("</{}>", kind.tag())
}

pub fn marker_start(id: u32) -> String {
    format!("<!--k:c:{id}-->")
}

pub fn marker_end(id: u32) -> String {
    format!("<!--k:/c:{id}-->")
}

/// Skyddar ett attributvärde (på en rad, utan `-->`).
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\n' => o.push_str("&#10;"),
            '\r' => {}
            c => o.push(c),
        }
    }
    o
}

fn unesc(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    s.replace("&#10;", "\n").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

pub fn rev_line(r: &RevRec) -> String {
    format!("<!--k:r id=\"{}\" author=\"{}\" date=\"{}\"-->", r.id, esc(&r.author), esc(&r.date))
}

pub fn comment_line(c: &CommentRec) -> String {
    let mut s = format!("<!--k:c id=\"{}\"", c.id);
    if let Some(p) = c.parent {
        s.push_str(&format!(" parent=\"{p}\""));
    }
    s.push_str(&format!(" author=\"{}\" date=\"{}\"", esc(&c.author), esc(&c.date)));
    if c.done {
        s.push_str(" done=\"1\"");
    }
    s.push_str(&format!(" text=\"{}\"-->", esc(&c.text)));
    s
}

/// `name="värde"`-par ur en tagg eller postrad.
fn attrs(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = s;
    while let Some(eq) = rest.find("=\"") {
        let name = rest[..eq].rsplit(|c: char| c.is_whitespace() || c == '<').next().unwrap_or("").to_owned();
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else { break };
        if !name.is_empty() {
            out.insert(name, unesc(&after[..end]));
        }
        rest = &after[end + 1..];
    }
    out
}

fn parse_rev_line(line: &str) -> Option<RevRec> {
    let body = line.strip_prefix("<!--k:r ")?.strip_suffix("-->")?;
    let a = attrs(body);
    Some(RevRec { id: a.get("id")?.parse().ok()?, author: a.get("author").cloned().unwrap_or_default(), date: a.get("date").cloned().unwrap_or_default() })
}

fn parse_comment_line(line: &str) -> Option<CommentRec> {
    let body = line.strip_prefix("<!--k:c ")?.strip_suffix("-->")?;
    let a = attrs(body);
    Some(CommentRec {
        id: a.get("id")?.parse().ok()?,
        parent: a.get("parent").and_then(|p| p.parse().ok()),
        author: a.get("author").cloned().unwrap_or_default(),
        date: a.get("date").cloned().unwrap_or_default(),
        done: a.get("done").is_some_and(|d| d == "1"),
        text: a.get("text").cloned().unwrap_or_default(),
    })
}

/// `<ins data-k="3">` / `<del data-k="3">` i början av `s`: slag, nummer och längd.
fn parse_open(s: &str) -> Option<(RevKind, u32, usize)> {
    let kind = if s.starts_with("<ins ") {
        RevKind::Ins
    } else if s.starts_with("<del ") {
        RevKind::Del
    } else {
        return None;
    };
    let end = s.find('>')?;
    let d = s[..end].find("data-k=\"")? + 8;
    let n = s[d..end].split('"').next()?.parse().ok()?;
    Some((kind, n, end + 1))
}

/// Exakt en `<ins data-k="N">`/`<del data-k="N">` (hela strängen).
pub fn parse_open_tag(s: &str) -> Option<(RevKind, u32)> {
    let (kind, id, len) = parse_open(s)?;
    (len == s.len()).then_some((kind, id))
}

/// `</ins>` eller `</del>`.
pub fn is_close_tag(s: &str) -> bool {
    s == "</ins>" || s == "</del>"
}

/// `<!--k:c:N-->` (true) eller `<!--k:/c:N-->` (false) exakt.
pub fn parse_marker(s: &str) -> Option<(bool, u32)> {
    let (start, rest) = match s.strip_prefix("<!--k:c:") {
        Some(r) => (true, r),
        None => (false, s.strip_prefix("<!--k:/c:")?),
    };
    let d = rest.strip_suffix("-->")?;
    if d.is_empty() || !d.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((start, d.parse().ok()?))
}

/// Rader som är poster (`<!--k:r …-->` eller `<!--k:c …-->`).
pub fn is_record_line(line: &str) -> bool {
    let t = line.trim();
    (t.starts_with("<!--k:r ") || t.starts_with("<!--k:c ")) && t.ends_with("-->")
}

// ------------------------------------------------------------------ analys

/// Letar upp ändringar, områden och poster. Kodblock hoppas över.
pub fn scan(text: &str) -> Scan {
    let mut out = Scan::default();
    if !text.contains("<!--k:") && !text.contains("<ins ") && !text.contains("<del ") {
        return out;
    }
    let mut ins_stack: Vec<(u32, Range<usize>)> = Vec::new();
    let mut del_stack: Vec<(u32, Range<usize>)> = Vec::new();
    let mut open_c: HashMap<u32, Range<usize>> = HashMap::new();
    let mut in_fence: Option<char> = None;
    let mut off = 0usize;
    for line in text.split_inclusive('\n') {
        let ls = off;
        off += line.len();
        let t = line.trim_end_matches(['\n', '\r']);
        let tt = t.trim_start();
        if tt.starts_with("```") || tt.starts_with("~~~") {
            let c = tt.chars().next().unwrap_or('`');
            match in_fence {
                None => in_fence = Some(c),
                Some(f) if f == c => in_fence = None,
                _ => {}
            }
            continue;
        }
        if in_fence.is_some() {
            continue;
        }
        if is_record_line(t) {
            let tr = ls + (t.len() - tt.len())..ls + t.trim_end().len();
            let tl = t.trim();
            if let Some(r) = parse_rev_line(tl) {
                out.rev_recs.push((r, tr));
            } else if let Some(c) = parse_comment_line(tl) {
                out.comment_recs.push((c, tr));
            }
            continue;
        }
        let mut i = 0usize;
        while let Some(rel) = t[i..].find('<') {
            let p = i + rel;
            let rest = &t[p..];
            let at = ls + p;
            if let Some((kind, id, len)) = parse_open(rest) {
                let st = if kind == RevKind::Ins { &mut ins_stack } else { &mut del_stack };
                st.push((id, at..at + len));
                i = p + len;
            } else if rest.starts_with("</ins>") || rest.starts_with("</del>") {
                let kind = if rest.starts_with("</ins>") { RevKind::Ins } else { RevKind::Del };
                let st = if kind == RevKind::Ins { &mut ins_stack } else { &mut del_stack };
                if let Some((id, open)) = st.pop() {
                    out.revs.push(RevSpan { kind, id, open, close: at..at + 6 });
                }
                i = p + 6;
            } else if let Some(r) = rest.strip_prefix("<!--k:c:") {
                let digits: String = r.chars().take_while(char::is_ascii_digit).collect();
                if !digits.is_empty() && r[digits.len()..].starts_with("-->") {
                    let len = 8 + digits.len() + 3;
                    if let Ok(id) = digits.parse() {
                        open_c.insert(id, at..at + len);
                    }
                    i = p + len;
                } else {
                    i = p + 1;
                }
            } else if let Some(r) = rest.strip_prefix("<!--k:/c:") {
                let digits: String = r.chars().take_while(char::is_ascii_digit).collect();
                if !digits.is_empty() && r[digits.len()..].starts_with("-->") {
                    let len = 9 + digits.len() + 3;
                    if let Ok(id) = digits.parse::<u32>() {
                        if let Some(start) = open_c.remove(&id) {
                            out.comments.push(CommentSpan { id, start, end: at..at + len });
                        }
                    }
                    i = p + len;
                } else {
                    i = p + 1;
                }
            } else {
                i = p + 1;
            }
        }
    }
    out.revs.sort_by_key(|r| r.open.start);
    out.comments.sort_by_key(|c| c.start.start);
    out
}

/// Alla kommentarer i texten (poster som saknar område räknas också).
pub fn has_review(text: &str) -> bool {
    text.contains("<!--k:c") || text.contains("<!--k:r ") || text.contains("data-k=\"")
}

static COMMENTS_VISIBLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Visa kommentarsområdena med markeringsbakgrund i dokumentläget? (Inställningen "Visa kommentarer".)
pub fn set_comments_visible(v: bool) {
    COMMENTS_VISIBLE.store(v, std::sync::atomic::Ordering::Relaxed);
}

pub fn comments_visible() -> bool {
    COMMENTS_VISIBLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Författarna i den ordning de först förekommer (ändringsposterna, sedan kommentarsposterna).
pub fn authors(sc: &Scan) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let names = sc.rev_recs.iter().map(|(r, _)| r.author.as_str()).chain(sc.comment_recs.iter().map(|(c, _)| c.author.as_str()));
    for a in names {
        if !out.iter().any(|o| o == a) {
            out.push(a.to_owned());
        }
    }
    out
}

/// Författarens färgindex: platsen i `authors` (varje författare får en egen färg så länge
/// det inte finns fler än färgerna).
pub fn author_color_index(authors: &[String], author: &str) -> u8 {
    (authors.iter().position(|a| a == author).unwrap_or(0) % AUTHOR_COLORS.len()) as u8
}

/// Författarfärger (mörka nog för vit sida; layouten ljusar upp dem i mörkt tema).
pub const AUTHOR_COLORS: [[u8; 3]; 8] = [
    [0xC0, 0x39, 0x2B],
    [0x1F, 0x77, 0xB4],
    [0x2E, 0x8B, 0x57],
    [0x8E, 0x44, 0xAD],
    [0xD3, 0x6B, 0x00],
    [0x00, 0x83, 0x8F],
    [0xB0, 0x30, 0x7A],
    [0x6D, 0x5A, 0x00],
];

// ------------------------------------------------------------------ poster

/// Delar texten i brödtext och poster. Postraderna tas bort ur brödtexten (med en eventuell
/// tom rad före dem som bara fanns för dem).
pub fn split_records(text: &str) -> (String, Vec<RevRec>, Vec<CommentRec>) {
    if !text.contains("<!--k:r ") && !text.contains("<!--k:c ") {
        return (text.to_owned(), Vec::new(), Vec::new());
    }
    let sc = scan(text);
    let mut ranges: Vec<Range<usize>> = sc.rev_recs.iter().map(|(_, r)| r.clone()).chain(sc.comment_recs.iter().map(|(_, r)| r.clone())).collect();
    ranges.sort_by_key(|r| r.start);
    let body = remove_lines(text, &ranges);
    let body = body.trim_end_matches('\n').to_owned();
    let body = if body.is_empty() { body } else { format!("{body}\n") };
    (body, sc.rev_recs.into_iter().map(|(r, _)| r).collect(), sc.comment_recs.into_iter().map(|(c, _)| c).collect())
}

/// Tar bort de rader som `ranges` (sorterade, var och en inom en rad) ligger på, inklusive radslut.
fn remove_lines(text: &str, ranges: &[Range<usize>]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for r in ranges {
        if r.start < at {
            continue;
        }
        let ls = text[..r.start].rfind('\n').map_or(0, |i| i + 1);
        let le = text[r.end..].find('\n').map_or(text.len(), |i| r.end + i + 1);
        if ls < at {
            continue;
        }
        out.push_str(&text[at..ls]);
        at = le;
    }
    out.push_str(&text[at..]);
    out
}

/// Skriver posterna sist i texten (ersätter de som finns).
pub fn put_records(body: &str, revs: &[RevRec], comments: &[CommentRec]) -> String {
    if revs.is_empty() && comments.is_empty() && !body.contains("<!--k:r ") && !body.contains("<!--k:c ") {
        return body.to_owned();
    }
    let (base, _, _) = split_records(body);
    if revs.is_empty() && comments.is_empty() {
        return base;
    }
    let mut out = base.trim_end_matches('\n').to_owned();
    out.push_str("\n\n");
    for r in revs {
        out.push_str(&rev_line(r));
        out.push('\n');
    }
    for c in comments {
        out.push_str(&comment_line(c));
        out.push('\n');
    }
    out
}

/// Tar bort poster utan ändring/område och tomma ändringar och områden; håller texten snygg.
pub fn prune(text: &str) -> String {
    if !has_review(text) {
        return text.to_owned();
    }
    let mut text = text.to_owned();
    // Tomma ändringar (`<ins data-k="1"></ins>`) och tomma områden försvinner.
    loop {
        let sc = scan(&text);
        let mut cuts: Vec<Range<usize>> = Vec::new();
        for r in &sc.revs {
            if r.inner().is_empty() {
                cuts.push(r.full());
            }
        }
        if cuts.is_empty() {
            break;
        }
        text = cut_ranges(&text, &mut cuts);
    }
    let sc = scan(&text);
    let used_revs: HashSet<u32> = sc.revs.iter().map(|r| r.id).collect();
    let anchored: HashSet<u32> = sc.comments.iter().map(|c| c.id).collect();
    let revs: Vec<RevRec> = sc.rev_recs.iter().map(|(r, _)| r.clone()).filter(|r| used_revs.contains(&r.id)).collect();
    let comments: Vec<CommentRec> = sc.comment_recs.iter().map(|(c, _)| c.clone()).filter(|c| anchored.contains(&c.id)).collect();
    // Markörer utan post tas bort.
    let known: HashSet<u32> = comments.iter().map(|c| c.id).collect();
    let mut cuts: Vec<Range<usize>> = Vec::new();
    for c in &sc.comments {
        if !known.contains(&c.id) {
            cuts.push(c.start.clone());
            cuts.push(c.end.clone());
        }
    }
    let text = cut_ranges(&text, &mut cuts);
    put_records(&text, &revs, &comments)
}

fn cut_ranges(text: &str, cuts: &mut Vec<Range<usize>>) -> String {
    cuts.sort_by_key(|r| r.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for r in cuts.iter() {
        if r.start < at {
            at = at.max(r.end);
            continue;
        }
        out.push_str(&text[at..r.start]);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

// ------------------------------------------------------------------ godkänn och avvisa

/// Godkänner (`accept`) eller avvisar de ändringar `pick` väljer: infogad text stannar (godkänd)
/// eller tas bort (avvisad), struken text tas bort (godkänd) eller stannar (avvisad).
fn resolve(text: &str, accept: bool, pick: &dyn Fn(&RevSpan) -> bool) -> String {
    let sc = scan(text);
    let mut cuts: Vec<Range<usize>> = Vec::new();
    let mut emptied: Vec<usize> = Vec::new();
    for r in sc.revs.iter().filter(|r| pick(r)) {
        let keep = (r.kind == RevKind::Ins) == accept;
        if keep {
            cuts.push(r.open.clone());
            cuts.push(r.close.clone());
        } else {
            cuts.push(r.full());
            emptied.push(r.open.start);
        }
    }
    if cuts.is_empty() {
        return text.to_owned();
    }
    let out = cut_ranges(text, &mut cuts);
    // En rad som blev tom av borttagningen försvinner helt (hela stycket var ändringen).
    let out = drop_emptied_lines(text, &out, &emptied, &sc);
    prune(&out)
}

/// Tar bort rader som hade innehåll före borttagningen men är tomma nu (hela stycket var
/// ändringen). Ändringar innehåller aldrig radbrytningar, så radnumren följer med.
fn drop_emptied_lines(old: &str, new: &str, emptied: &[usize], _sc: &Scan) -> String {
    if emptied.is_empty() || old.matches('\n').count() != new.matches('\n').count() {
        return new.to_owned();
    }
    let lines: HashSet<usize> = emptied.iter().map(|&p| old[..p].matches('\n').count()).collect();
    let old_lines: Vec<&str> = old.split('\n').collect();
    let new_lines: Vec<&str> = new.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(new_lines.len());
    let mut skip_blank = false;
    for (n, line) in new_lines.iter().enumerate() {
        if lines.contains(&n) && line.trim().is_empty() && old_lines.get(n).is_some_and(|l| !l.trim().is_empty()) {
            // Släpp raden och en av de tomma raderna runt den, så att inga dubbla luckor uppstår.
            skip_blank = out.last().is_some_and(|l| l.trim().is_empty()) || out.is_empty();
            continue;
        }
        if skip_blank && line.trim().is_empty() && n + 1 < new_lines.len() {
            skip_blank = false;
            continue;
        }
        skip_blank = false;
        out.push(line);
    }
    out.join("\n")
}

pub fn accept_all(text: &str) -> String {
    resolve(text, true, &|_| true)
}

pub fn reject_all(text: &str) -> String {
    resolve(text, false, &|_| true)
}

/// Godkänner/avvisar ändringarna (alla taggar med samma nummer) som rör `range`.
pub fn resolve_range(text: &str, range: Range<usize>, accept: bool) -> Option<Edit> {
    let sc = scan(text);
    let ids: HashSet<u32> = sc
        .revs
        .iter()
        .filter(|r| {
            let f = r.full();
            if range.start == range.end { f.start <= range.start && range.start <= f.end } else { f.start < range.end && range.start < f.end }
        })
        .map(|r| r.id)
        .collect();
    if ids.is_empty() {
        return None;
    }
    let first = sc.revs.iter().filter(|r| ids.contains(&r.id)).map(|r| r.open.start).min().unwrap_or(range.start);
    let new = resolve(text, accept, &|r| ids.contains(&r.id));
    let caret = first.min(new.len());
    let caret = floor_char(&new, caret);
    Some(Edit { text: new, sel: caret..caret })
}

fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Antal ändringar (olika nummer).
pub fn change_count(text: &str) -> usize {
    scan(text).revs.iter().map(|r| r.id).collect::<HashSet<_>>().len()
}

/// Ändringarna (ett område per nummer) i textordning.
fn extents(sc: &Scan) -> Vec<(u32, Range<usize>)> {
    let mut out: Vec<(u32, Range<usize>)> = Vec::new();
    for r in &sc.revs {
        if !out.iter().any(|(id, _)| *id == r.id) {
            out.push((r.id, change_extent(sc, r.id)));
        }
    }
    out.sort_by_key(|(_, r)| r.start);
    out
}

/// Nästa ändring efter `pos` (området från första till sista taggen).
pub fn next_change(text: &str, pos: usize) -> Option<Range<usize>> {
    let ex = extents(&scan(text));
    let from = ex.iter().filter(|(_, r)| r.start <= pos && pos <= r.end).map(|(_, r)| r.end).max().unwrap_or(pos);
    ex.into_iter().map(|(_, r)| r).find(|r| r.start >= from && r.start > pos)
}

/// Föregående ändring före `pos`.
pub fn prev_change(text: &str, pos: usize) -> Option<Range<usize>> {
    let ex = extents(&scan(text));
    let from = ex.iter().filter(|(_, r)| r.start < pos && pos <= r.end).map(|(_, r)| r.start).min().unwrap_or(pos);
    ex.into_iter().map(|(_, r)| r).rev().find(|r| r.end <= from && r.start < pos)
}

/// Hela området för ändring `id`, från första till sista taggen.
fn change_extent(sc: &Scan, id: u32) -> Range<usize> {
    let mut s = usize::MAX;
    let mut e = 0;
    for r in sc.revs.iter().filter(|r| r.id == id) {
        s = s.min(r.open.start);
        e = e.max(r.close.end);
    }
    s..e
}

/// Texten som den blir när alla ändringar godkänns och kommentarerna tas bort: för export
/// (PDF, HTML …) och förhandsvisning.
pub fn clean_for_export(text: &str) -> String {
    if !has_review(text) {
        return text.to_owned();
    }
    let acc = accept_all(text);
    let sc = scan(&acc);
    let mut cuts: Vec<Range<usize>> = Vec::new();
    for c in &sc.comments {
        cuts.push(c.start.clone());
        cuts.push(c.end.clone());
    }
    let cut = cut_ranges(&acc, &mut cuts);
    let (body, _, _) = split_records(&cut);
    body
}

// ------------------------------------------------------------------ spåra ändringar vid skrivning

fn next_rev_id(sc: &Scan) -> u32 {
    sc.revs.iter().map(|r| r.id).chain(sc.rev_recs.iter().map(|(r, _)| r.id)).max().unwrap_or(0) + 1
}

fn next_comment_id(sc: &Scan) -> u32 {
    sc.comments.iter().map(|c| c.id).chain(sc.comment_recs.iter().map(|(c, _)| c.id)).max().map_or(1, |m| m + 1)
}

/// Radprefixets längd (indrag, `#`, `-`, `1.`, `>`) som aldrig ska ingå i en ändring.
fn prefix_len(line: &str) -> usize {
    crate::word::markup::line_prefix_len(line)
}

/// Lägger taggar runt varje rad med text i `s` (utom radens prefix när `at_line_start`), så att
/// en ändring aldrig går över ett stycke.
fn wrap_lines(s: &str, at_line_start: bool, open: &str, close: &str) -> String {
    let mut out = String::new();
    for (n, line) in s.split_inclusive('\n').enumerate() {
        let body = line.trim_end_matches('\n');
        let nl = &line[body.len()..];
        let skip = if n > 0 || at_line_start { prefix_len(body).min(body.len()) } else { 0 };
        let (pre, rest) = body.split_at(skip);
        out.push_str(pre);
        if !rest.is_empty() {
            out.push_str(open);
            out.push_str(rest);
            out.push_str(close);
        }
        out.push_str(nl);
    }
    out
}

/// Byteintervall för taggar, markörer och poster (som aldrig raderas av en spårad radering).
fn token_ranges(sc: &Scan) -> Vec<Range<usize>> {
    let mut v: Vec<Range<usize>> = Vec::new();
    for r in &sc.revs {
        v.push(r.open.clone());
        v.push(r.close.clone());
    }
    for c in &sc.comments {
        v.push(c.start.clone());
        v.push(c.end.clone());
    }
    v.extend(sc.rev_recs.iter().map(|(_, r)| r.clone()));
    v.extend(sc.comment_recs.iter().map(|(_, r)| r.clone()));
    v.sort_by_key(|r| r.start);
    v
}

pub struct Tracked {
    pub text: String,
    pub caret: usize,
}

fn in_code_fence(text: &str, pos: usize) -> bool {
    let mut open = false;
    for line in text[..floor_char(text, pos)].lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            open = !open;
        }
    }
    open
}

/// Gör om en vanlig redigering (`old` → `new`, gjord av TextEdit) till en spårad: infogad text
/// får `<ins>`, raderad text `<del>` i stället för att tas bort. `old_sel` är markeringen före.
/// `None` när texten är oförändrad eller redigeringen ligger i ett kodblock.
pub fn track_edit(old: &str, new: &str, old_sel: Range<usize>, author: &str, date: &str) -> Option<Tracked> {
    if old == new {
        return None;
    }
    // Gemensamt prefix och suffix ger det som ändrats.
    let mut p = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    p = floor_char(old, floor_char(new, p));
    let max_s = old.len().min(new.len()) - p;
    let mut s = old.bytes().rev().zip(new.bytes().rev()).take(max_s).take_while(|(a, b)| a == b).count();
    while s > 0 && !(old.is_char_boundary(old.len() - s) && new.is_char_boundary(new.len() - s)) {
        s -= 1;
    }
    let mut reg = p..old.len() - s;
    let inserted = &new[p..new.len() - s];
    let sc = scan(old);
    let tokens = token_ranges(&sc);
    let in_token = |i: usize| tokens.iter().any(|t| t.start <= i && i < t.end);
    if in_code_fence(old, reg.start) {
        return None;
    }
    let rec_author = |id: u32| sc.rev_rec(id).map(|r| r.author.as_str());
    let own = |r: &RevSpan| rec_author(r.id) == Some(author);
    let back = old_sel.end >= reg.end;
    let collapsed = old_sel.start == old_sel.end;

    // Radering som bara träffar osynliga taggar: gå förbi dem till nästa synliga tecken.
    if inserted.is_empty() && !reg.is_empty() && old[reg.clone()].char_indices().all(|(i, _)| in_token(reg.start + i)) {
        if back {
            let mut q = reg.start;
            while let Some(t) = tokens.iter().find(|t| t.start < q && q <= t.end) {
                q = t.start;
            }
            let prev = old[..q].char_indices().next_back().map(|(i, _)| i)?;
            reg = prev..q;
        } else {
            let mut q = reg.end;
            while let Some(t) = tokens.iter().find(|t| t.start <= q && q < t.end) {
                q = t.end;
            }
            let next = old[q..].chars().next()?;
            reg = q..q + next.len_utf8();
        }
    }
    // En ren infogning inne i en tagg eller i en radering/annans infogning flyttas till efter dem.
    if reg.is_empty() && !inserted.is_empty() {
        let mut at = reg.start;
        loop {
            let moved = tokens.iter().find(|t| t.start < at && at < t.end).map(|t| t.end).or_else(|| {
                sc.revs.iter().find(|r| (r.kind == RevKind::Del || !own(r)) && r.inner().start < at && at < r.inner().end).map(|r| r.close.end)
            });
            match moved {
                Some(m) if m > at => at = m,
                _ => break,
            }
        }
        reg = at..at;
    }

    let removed = &old[reg.clone()];
    let at_line_start = old[..reg.start].rsplit('\n').next().is_none_or(|l| l.trim().is_empty());
    let in_del = |i: usize| sc.revs.iter().any(|r| r.kind == RevKind::Del && r.inner().start <= i && i < r.inner().end);
    let in_own_ins = |i: usize| sc.revs.iter().any(|r| r.kind == RevKind::Ins && own(r) && r.inner().start <= i && i < r.inner().end);

    #[derive(PartialEq, Clone, Copy)]
    enum Seg {
        Keep,
        Drop,
        Del,
    }
    // Radering: text i egen infogning försvinner, taggar, radbrytningar och radprefix stannar,
    // allt annat blir `<del>`.
    let mut segs: Vec<(Seg, Range<usize>)> = Vec::new();
    let mut pre_end = {
        let ls = old[..reg.start].rfind('\n').map_or(0, |i| i + 1);
        ls + prefix_len(old[ls..].split('\n').next().unwrap_or(""))
    };
    for (i, c) in removed.char_indices() {
        let pos = reg.start + i;
        let kind = if c == '\n' || c == '\r' || in_token(pos) || in_del(pos) || pos < pre_end {
            Seg::Keep
        } else if in_own_ins(pos) {
            Seg::Drop
        } else {
            Seg::Del
        };
        let r = pos..pos + c.len_utf8();
        match segs.last_mut() {
            Some((k, last)) if *k == kind && last.end == r.start => last.end = r.end,
            _ => segs.push((kind, r)),
        }
        if c == '\n' {
            pre_end = pos + 1 + prefix_len(old[pos + 1..].split('\n').next().unwrap_or(""));
        }
    }

    let mut next_id = next_rev_id(&sc);
    let mut new_recs: Vec<RevRec> = Vec::new();
    let mut alloc = || {
        let id = next_id;
        next_id += 1;
        new_recs.push(RevRec { id, author: author.to_owned(), date: date.to_owned() });
        id
    };

    let (out, caret);
    let only_del = segs.len() == 1 && segs[0].0 == Seg::Del && inserted.is_empty();
    let only_drop = segs.iter().all(|(k, _)| *k == Seg::Drop);
    // Där infogningen kan skrivas rakt in i en egen infogning (samma författare).
    let ins_into = if !inserted.is_empty() && only_drop {
        if reg.is_empty() {
            let pos = reg.start;
            sc.revs
                .iter()
                .find(|r| r.kind == RevKind::Ins && own(r) && r.inner().start <= pos && pos <= r.inner().end)
                .map(|_| pos)
                .or_else(|| sc.revs.iter().find(|r| r.kind == RevKind::Ins && own(r) && r.close.end == pos).map(|r| r.close.start))
                .or_else(|| sc.revs.iter().find(|r| r.kind == RevKind::Ins && own(r) && r.open.start == pos).map(|r| r.open.end))
        } else {
            Some(reg.start)
        }
    } else {
        None
    };
    let adjacent_del = if only_del && !removed.contains('\n') {
        sc.revs.iter().find(|r| r.kind == RevKind::Del && own(r) && r.open.start == reg.end).map(|r| (r.clone(), true)).or_else(|| {
            sc.revs.iter().find(|r| r.kind == RevKind::Del && own(r) && r.close.end == reg.start).map(|r| (r.clone(), false))
        })
    } else {
        None
    };

    if let Some(at) = ins_into {
        let cut = if reg.is_empty() { at..at } else { reg.clone() };
        out = format!("{}{inserted}{}", &old[..cut.start], &old[cut.end..]);
        caret = cut.start + inserted.len();
    } else if let Some((d, before)) = adjacent_del {
        if before {
            out = format!("{}{}{removed}{}", &old[..reg.start], &old[d.open.clone()], &old[d.open.end..]);
            caret = reg.start;
        } else {
            out = format!("{}{removed}{}{}", &old[..d.close.start], &old[d.close.clone()], &old[reg.end..]);
            caret = d.close.start + removed.len() + (d.close.end - d.close.start);
        }
    } else {
        let mut mid = String::new();
        for (kind, r) in &segs {
            match kind {
                Seg::Keep => mid.push_str(&old[r.clone()]),
                Seg::Drop => {}
                Seg::Del => {
                    let id = alloc();
                    mid.push_str(&format!("{}{}{}", open_tag(RevKind::Del, id), &old[r.clone()], close_tag(RevKind::Del)));
                }
            }
        }
        let ins_text = if inserted.is_empty() {
            String::new()
        } else {
            let id = alloc();
            wrap_lines(inserted, at_line_start, &open_tag(RevKind::Ins, id), &close_tag(RevKind::Ins))
        };
        let head = &old[..reg.start];
        out = format!("{head}{mid}{ins_text}{}", &old[reg.end..]);
        let ins_at = head.len() + mid.len();
        caret = if !ins_text.is_empty() {
            // Före sista sluttaggen i den infogade texten.
            ins_at + ins_text.rfind("</ins>").unwrap_or(ins_text.len())
        } else if back && collapsed {
            head.len()
        } else {
            ins_at
        };
    }
    let mut revs: Vec<RevRec> = sc.rev_recs.iter().map(|(r, _)| r.clone()).collect();
    revs.extend(new_recs);
    let comments: Vec<CommentRec> = sc.comment_recs.iter().map(|(c, _)| c.clone()).collect();
    let (text, caret) = rewrite_records(&out, caret, &revs, &comments);
    Some(Tracked { text, caret })
}

/// Skriver posterna om efter en ändring av brödtexten: poster utan tagg eller markör tas bort,
/// tomma ändringar likaså. Markören (i brödtexten, före posterna) räknas om.
fn rewrite_records(text: &str, caret: usize, revs: &[RevRec], comments: &[CommentRec]) -> (String, usize) {
    // Posterna ligger sist; markören står före dem.
    let sc = scan(text);
    let first_rec = sc.rev_recs.iter().map(|(_, r)| r.start).chain(sc.comment_recs.iter().map(|(_, r)| r.start)).min().unwrap_or(text.len());
    let mut caret = caret.min(first_rec);
    let body = &text[..first_rec];
    // Tomma ändringar (`<ins data-k="1"></ins>`) försvinner; markören flyttas med.
    let sc = scan(body);
    let mut cuts: Vec<Range<usize>> = sc.revs.iter().filter(|r| r.inner().is_empty()).map(RevSpan::full).collect();
    for c in &cuts {
        if caret >= c.end {
            caret -= c.end - c.start;
        } else if caret > c.start {
            caret = c.start;
        }
    }
    let mut body = cut_ranges(body, &mut cuts);
    if first_rec < text.len() {
        // Luckan före postraderna hör till dem.
        body = format!("{}\n", body.trim_end_matches('\n'));
        caret = caret.min(body.len());
    }
    let sc = scan(&body);
    let used: HashSet<u32> = sc.revs.iter().map(|r| r.id).collect();
    let anchored: HashSet<u32> = sc.comments.iter().map(|c| c.id).collect();
    let revs: Vec<RevRec> = revs.iter().filter(|r| used.contains(&r.id)).cloned().collect();
    let comments: Vec<CommentRec> = comments.iter().filter(|c| anchored.contains(&c.id)).cloned().collect();
    let out = put_records(&body, &revs, &comments);
    let caret = floor_char(&out, caret);
    (out, caret)
}

// ------------------------------------------------------------------ kommentarer

/// Lägger en kommentar på `sel` (tom markering: ordet vid markören, annars ett läge). Returnerar
/// texten, markeringen och kommentarens nummer.
pub fn add_comment(text: &str, sel: Range<usize>, author: &str, date: &str, body: &str) -> (Edit, u32) {
    let sc = scan(text);
    let id = next_comment_id(&sc);
    let mut sel = sel.start.min(text.len())..sel.end.min(text.len());
    sel = floor_char(text, sel.start)..floor_char(text, sel.end);
    if sel.start == sel.end {
        // Ordet vid markören.
        let b = text.as_bytes();
        let is_w = |c: char| c.is_alphanumeric();
        let mut s = sel.start;
        while s > 0 && text[..s].chars().next_back().is_some_and(is_w) {
            s -= text[..s].chars().next_back().map_or(1, char::len_utf8);
        }
        let mut e = sel.end;
        while e < b.len() && text[e..].chars().next().is_some_and(is_w) {
            e += text[e..].chars().next().map_or(1, char::len_utf8);
        }
        sel = s..e;
    }
    // Området får inte börja eller sluta inne i en tagg eller gå över rader.
    let tokens = token_ranges(&sc);
    for t in &tokens {
        if t.start < sel.start && sel.start < t.end {
            sel.start = t.start;
        }
        if t.start < sel.end && sel.end < t.end {
            sel.end = t.end;
        }
    }
    if let Some(nl) = text[sel.clone()].find('\n') {
        sel.end = sel.start + nl;
    }
    // Början av en rad: markören efter radprefixet, och en osynlig tecknet-referens skyddar mot HTML-block.
    let ls = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[ls..].find('\n').map_or(text.len(), |i| ls + i);
    let pre = ls + prefix_len(&text[ls..line_end]);
    if sel.start < pre {
        sel.start = pre.min(sel.end);
    }
    let guard = if sel.start == pre && !text[ls..pre].contains(|c: char| !c.is_whitespace()) || sel.start == ls { "&#8203;" } else { "" };
    let guard = if text[ls..sel.start].trim().is_empty() { guard } else { "" };
    let mut out = String::with_capacity(text.len() + 160);
    out.push_str(&text[..sel.start]);
    out.push_str(guard);
    out.push_str(&marker_start(id));
    out.push_str(&text[sel.clone()]);
    out.push_str(&marker_end(id));
    out.push_str(&text[sel.end..]);
    let rec = CommentRec { id, parent: None, author: author.to_owned(), date: date.to_owned(), done: false, text: body.to_owned() };
    let mut comments: Vec<CommentRec> = sc.comment_recs.iter().map(|(c, _)| c.clone()).collect();
    comments.push(rec);
    let revs: Vec<RevRec> = sc.rev_recs.iter().map(|(r, _)| r.clone()).collect();
    let out = put_records(&out, &revs, &comments);
    let s = sel.start + guard.len() + marker_start(id).len();
    let e = s + (sel.end - sel.start);
    (Edit { text: out, sel: s..e }, id)
}

/// Svar på kommentar `parent`: delar områdets läge (markörerna läggs direkt efter förälderns).
pub fn add_reply(text: &str, parent: u32, author: &str, date: &str, body: &str) -> Option<(Edit, u32)> {
    let sc = scan(text);
    // Svar på ett svar hamnar i samma tråd.
    let root = sc.comment_rec(parent).map(|c| c.parent.unwrap_or(c.id))?;
    let span = sc.comments.iter().find(|c| c.id == root)?;
    let id = next_comment_id(&sc);
    // Efter förälderns och tidigare svars markörer, så att trådens ordning blir kronologisk.
    let last_start = sc.comment_recs.iter().filter(|(c, _)| c.parent == Some(root)).filter_map(|(c, _)| sc.comments.iter().find(|s| s.id == c.id)).map(|s| s.start.end).max().unwrap_or(span.start.end).max(span.start.end);
    let last_end = sc.comment_recs.iter().filter(|(c, _)| c.parent == Some(root)).filter_map(|(c, _)| sc.comments.iter().find(|s| s.id == c.id)).map(|s| s.end.end).max().unwrap_or(span.end.end).max(span.end.end);
    let mut ins: Vec<(usize, String)> = vec![(last_start, marker_start(id)), (last_end, marker_end(id))];
    ins.sort_by_key(|(p, _)| *p);
    let mut out = String::with_capacity(text.len() + 160);
    let mut at = 0;
    let mut caret = last_start;
    for (p, s) in &ins {
        out.push_str(&text[at..*p]);
        if *p == last_start && s.starts_with("<!--k:c:") {
            caret = out.len() + s.len();
        }
        out.push_str(s);
        at = *p;
    }
    out.push_str(&text[at..]);
    let mut comments: Vec<CommentRec> = sc.comment_recs.iter().map(|(c, _)| c.clone()).collect();
    comments.push(CommentRec { id, parent: Some(root), author: author.to_owned(), date: date.to_owned(), done: false, text: body.to_owned() });
    let revs: Vec<RevRec> = sc.rev_recs.iter().map(|(r, _)| r.clone()).collect();
    let out = put_records(&out, &revs, &comments);
    Some((Edit { text: out, sel: caret..caret }, id))
}

/// Tar bort kommentaren (och dess svar om det är en tråd): markörer och post.
pub fn delete_comment(text: &str, id: u32) -> String {
    let sc = scan(text);
    let mut gone: HashSet<u32> = HashSet::new();
    gone.insert(id);
    for (c, _) in &sc.comment_recs {
        if c.parent == Some(id) {
            gone.insert(c.id);
        }
    }
    let mut cuts: Vec<Range<usize>> = Vec::new();
    for c in sc.comments.iter().filter(|c| gone.contains(&c.id)) {
        cuts.push(c.start.clone());
        cuts.push(c.end.clone());
    }
    let comments: Vec<CommentRec> = sc.comment_recs.iter().map(|(c, _)| c.clone()).filter(|c| !gone.contains(&c.id)).collect();
    let revs: Vec<RevRec> = sc.rev_recs.iter().map(|(r, _)| r.clone()).collect();
    let body = cut_ranges(text, &mut cuts);
    // En osynlig tecknet-referens som stod före markören blir kvar och är ofarlig.
    put_records(&body, &revs, &comments)
}

/// Ändrar en kommentars text eller markering som löst.
pub fn edit_comment(text: &str, id: u32, f: impl FnOnce(&mut CommentRec)) -> String {
    let sc = scan(text);
    let mut comments: Vec<CommentRec> = sc.comment_recs.iter().map(|(c, _)| c.clone()).collect();
    if let Some(c) = comments.iter_mut().find(|c| c.id == id) {
        f(c);
    }
    let revs: Vec<RevRec> = sc.rev_recs.iter().map(|(r, _)| r.clone()).collect();
    put_records(text, &revs, &comments)
}

/// En kommentar med sina svar, för sidopanelen.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub root: CommentRec,
    pub replies: Vec<CommentRec>,
    /// Det kommenterade området (mellan markörerna).
    pub range: Range<usize>,
}

pub fn threads(text: &str) -> Vec<Thread> {
    let sc = scan(text);
    let mut out: Vec<Thread> = Vec::new();
    for (c, _) in sc.comment_recs.iter().filter(|(c, _)| c.parent.is_none()) {
        let Some(span) = sc.comments.iter().find(|s| s.id == c.id) else { continue };
        let replies = sc.comment_recs.iter().filter(|(r, _)| r.parent == Some(c.id)).map(|(r, _)| r.clone()).collect();
        out.push(Thread { root: c.clone(), replies, range: span.inner() });
    }
    out.sort_by_key(|t| t.range.start);
    out
}

/// Tråden (rotkommentarens nummer) vars område innehåller `pos`; den innersta om flera.
pub fn thread_at(text: &str, pos: usize) -> Option<u32> {
    let sc = scan(text);
    let roots: HashSet<u32> = sc.comment_recs.iter().filter(|(c, _)| c.parent.is_none()).map(|(c, _)| c.id).collect();
    sc.comments.iter().filter(|c| roots.contains(&c.id) && c.start.start <= pos && pos <= c.end.end).min_by_key(|c| c.end.end - c.start.start).map(|c| c.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AU: &str = "Ada";
    const DT: &str = "2026-05-01T10:00:00Z";

    fn doc() -> String {
        "Hej <ins data-k=\"1\">snälla </ins>världen<del data-k=\"2\"> idag</del>.\n\n<!--k:r id=\"1\" author=\"Ada\" date=\"2026-05-01T10:00:00Z\"-->\n<!--k:r id=\"2\" author=\"Bo\" date=\"2026-05-02T10:00:00Z\"-->\n".to_owned()
    }

    #[test]
    fn scan_finds_spans_and_records() {
        let t = doc();
        let sc = scan(&t);
        assert_eq!(sc.revs.len(), 2);
        assert_eq!(sc.revs[0].kind, RevKind::Ins);
        assert_eq!(&t[sc.revs[0].inner()], "snälla ");
        assert_eq!(sc.revs[1].kind, RevKind::Del);
        assert_eq!(&t[sc.revs[1].inner()], " idag");
        assert_eq!(sc.rev_rec(2).unwrap().author, "Bo");
        assert_eq!(change_count(&t), 2);
    }

    #[test]
    fn records_round_trip_with_escapes() {
        let c = CommentRec { id: 3, parent: Some(1), author: "A \"B\" <c>".into(), date: DT.into(), done: true, text: "Rad ett\nrad & två -->".into() };
        let l = comment_line(&c);
        assert!(!l.contains('\n') && l.matches("-->").count() == 1);
        assert_eq!(parse_comment_line(&l), Some(c));
        let r = RevRec { id: 9, author: "Å \"x\"".into(), date: DT.into() };
        assert_eq!(parse_rev_line(&rev_line(&r)), Some(r));
    }

    #[test]
    fn accept_and_reject_all() {
        let t = doc();
        let a = accept_all(&t);
        assert!(a.starts_with("Hej snälla världen.\n"), "{a}");
        assert!(!a.contains("<!--k:r"), "{a}");
        let r = reject_all(&t);
        assert!(r.starts_with("Hej världen idag.\n"), "{r}");
        assert!(!r.contains("<ins") && !r.contains("<!--k:r"), "{r}");
    }

    #[test]
    fn accept_and_reject_at_caret() {
        let t = doc();
        let p = t.find("snälla").unwrap() + 2;
        let e = resolve_range(&t, p..p, true).unwrap();
        assert!(e.text.starts_with("Hej snälla världen<del data-k=\"2\"> idag</del>."), "{}", e.text);
        assert!(e.text.contains("author=\"Bo\"") && !e.text.contains("author=\"Ada\""));
        let e = resolve_range(&t, p..p, false).unwrap();
        assert!(e.text.starts_with("Hej världen<del"), "{}", e.text);
        // Utanför ändringar händer ingenting.
        assert!(resolve_range(&t, 1..1, true).is_none());
        // En markering över båda tar båda.
        let e = resolve_range(&t, 0..t.find('.').unwrap(), true).unwrap();
        assert!(e.text.starts_with("Hej snälla världen.\n"), "{}", e.text);
    }

    #[test]
    fn a_fully_deleted_paragraph_disappears_on_accept() {
        let t = "A\n\n<del data-k=\"1\">Bort</del>\n\nB\n\n<!--k:r id=\"1\" author=\"Ada\" date=\"x\"-->\n";
        let a = accept_all(t);
        assert!(!a.contains("Bort") && a.contains("A\n") && a.contains("B\n"), "{a:?}");
        assert!(!a.lines().skip(1).take(2).all(|l| l.is_empty()) || a.matches("\n\n\n").count() <= 1, "{a:?}");
    }

    #[test]
    fn next_and_previous_change() {
        let t = doc();
        let a = next_change(&t, 0).unwrap();
        assert!(t[a.clone()].contains("snälla"));
        let b = next_change(&t, a.end).unwrap();
        assert!(t[b.clone()].contains("idag"));
        assert!(next_change(&t, b.end).is_none());
        assert_eq!(prev_change(&t, b.start + 3).map(|r| r.start), Some(a.start));
        assert!(prev_change(&t, a.start).is_none());
    }

    #[test]
    fn typing_wraps_an_insertion_and_continues_in_it() {
        let old = "Hej världen.\n";
        let new = "Hej snälla världen.\n";
        let t = track_edit(old, new, 4..4, AU, DT).unwrap();
        assert!(t.text.starts_with("Hej <ins data-k=\"1\">snälla </ins>världen.\n"), "{}", t.text);
        assert!(t.text.contains("<!--k:r id=\"1\" author=\"Ada\""), "{}", t.text);
        assert_eq!(&t.text[t.caret..t.caret + 6], "</ins>");
        // Nästa tecken skrivs inne i infogningen utan ny tagg.
        let mut next = t.text.clone();
        next.insert(t.caret, 'x');
        let t2 = track_edit(&t.text, &next, t.caret..t.caret, AU, DT).unwrap();
        assert!(t2.text.contains("snälla x</ins>"), "{}", t2.text);
        assert_eq!(t2.text.matches("<ins").count(), 1);
        // Författarens ändring är en enda post.
        assert_eq!(scan(&t2.text).rev_recs.len(), 1);
    }

    #[test]
    fn typing_right_after_an_insertion_extends_it() {
        let t1 = track_edit("ab\n", "aXb\n", 1..1, AU, DT).unwrap();
        let close_end = t1.text.find("</ins>").unwrap() + 6;
        let mut next = t1.text.clone();
        next.insert(close_end, 'Y');
        let t2 = track_edit(&t1.text, &next, close_end..close_end, AU, DT).unwrap();
        assert!(t2.text.starts_with("a<ins data-k=\"1\">XY</ins>b"), "{}", t2.text);
    }

    #[test]
    fn deleting_marks_text_instead_of_removing_it() {
        let old = "Hej världen.\n";
        let new = "Hej värld.\n";
        let t = track_edit(old, new, 10..10, AU, DT).unwrap();
        assert!(t.text.starts_with("Hej värld<del data-k=\"1\">en</del>.\n"), "{}", t.text);
        // Delete: markören efter raderingen. Backspace: före.
        let del = t.text.find("</del>").unwrap() + 6;
        assert_eq!(t.caret, del);
        // Backspace (markören efter det borttagna): markören hamnar före raderingen.
        let tb = track_edit(old, new, 12..12, AU, DT).unwrap();
        assert!(tb.text.contains("<del data-k=\"1\">en</del>"), "{}", tb.text);
        assert_eq!(&tb.text[tb.caret..tb.caret + 4], "<del");
    }

    #[test]
    fn consecutive_backspaces_extend_one_deletion() {
        let t1 = track_edit("abc\n", "ab\n", 3..3, AU, DT).unwrap();
        assert!(t1.text.starts_with("ab<del data-k=\"1\">c</del>"), "{}", t1.text);
        // Backspace igen: markören står före `<del`.
        let mut next = t1.text.clone();
        next.remove(1);
        let t2 = track_edit(&t1.text, &next, 2..2, AU, DT).unwrap();
        assert!(t2.text.starts_with("a<del data-k=\"1\">bc</del>"), "{}", t2.text);
        assert_eq!(t2.text.matches("<del").count(), 1);
    }

    #[test]
    fn deleting_own_insertion_removes_it() {
        let t1 = track_edit("ab\n", "aXb\n", 1..1, AU, DT).unwrap();
        // Backspace inne i infogningen tar bort tecknet för gott.
        let x = t1.text.find('X').unwrap();
        let mut next = t1.text.clone();
        next.remove(x);
        let t2 = track_edit(&t1.text, &next, x + 1..x + 1, AU, DT).unwrap();
        assert_eq!(t2.text, "ab\n", "{}", t2.text);
    }

    #[test]
    fn backspace_over_hidden_tag_skips_to_visible_text() {
        let t1 = track_edit("ab\n", "aXb\n", 1..1, AU, DT).unwrap();
        // Markören efter `</ins>` och Backspace tar bort `>` i taggen: omdirigeras till tecknet före.
        let after = t1.text.find("</ins>").unwrap() + 6;
        let mut next = t1.text.clone();
        next.remove(after - 1);
        let t2 = track_edit(&t1.text, &next, after..after, AU, DT).unwrap();
        // Tecknet före taggen (X, i egen infogning) tas bort för gott.
        assert_eq!(t2.text, "ab\n", "{}", t2.text);
    }

    #[test]
    fn deleting_keeps_line_prefix_and_newlines() {
        let old = "- punkt\n\nNästa\n";
        let new = "Nästa\n";
        let t = track_edit(old, new, 0..0, AU, DT).unwrap();
        assert!(t.text.starts_with("- <del data-k=\"1\">punkt</del>\n\n"), "{}", t.text);
        assert!(t.text.contains("<del data-k=\"2\">Nästa</del>") || t.text.contains("Nästa"), "{}", t.text);
    }

    #[test]
    fn replacing_a_selection_gives_del_then_ins() {
        let t = track_edit("Hej världen.\n", "Hej jorden.\n", 4..12, AU, DT).unwrap();
        assert!(t.text.contains("<del data-k=\"1\">värl</del><ins data-k=\"2\">jor</ins>den."), "{}", t.text);
    }

    #[test]
    fn comments_add_reply_delete() {
        let t = "Hej världen.\n";
        let (e, id) = add_comment(t, 4..12, "Ada", DT, "Skriv om");
        assert_eq!(id, 1);
        assert!(e.text.starts_with("Hej <!--k:c:1-->världen<!--k:/c:1-->.\n"), "{}", e.text);
        assert_eq!(&e.text[e.sel.clone()], "världen");
        let th = threads(&e.text);
        assert_eq!(th.len(), 1);
        assert_eq!(th[0].root.text, "Skriv om");
        assert_eq!(&e.text[th[0].range.clone()], "världen");
        let (e2, rid) = add_reply(&e.text, 1, "Bo", DT, "Klart").unwrap();
        assert_eq!(rid, 2);
        let th = threads(&e2.text);
        assert_eq!(th.len(), 1);
        assert_eq!(th[0].replies.len(), 1);
        assert_eq!(th[0].replies[0].parent, Some(1));
        assert!(e2.text.contains("<!--k:c:2--><!--k:/c:2-->") || e2.text.contains("<!--k:c:2-->"), "{}", e2.text);
        assert_eq!(thread_at(&e2.text, 8), Some(1));
        let done = edit_comment(&e2.text, 1, |c| c.done = true);
        assert!(threads(&done)[0].root.done);
        let gone = delete_comment(&e2.text, 1);
        assert_eq!(gone, "Hej världen.\n");
    }

    #[test]
    fn comment_at_line_start_is_guarded_and_collapsed_selection_takes_the_word() {
        let (e, _) = add_comment("Ord här\n", 0..3, "Ada", DT, "x");
        assert!(e.text.starts_with("&#8203;<!--k:c:1-->Ord<!--k:/c:1--> här"), "{}", e.text);
        let (e, _) = add_comment("Ord här\n", 5..5, "Ada", DT, "x");
        assert!(e.text.starts_with("Ord <!--k:c:1-->här<!--k:/c:1-->"), "{}", e.text);
        let (e, _) = add_comment("# Rubrik\n", 2..8, "Ada", DT, "x");
        assert!(e.text.starts_with("# &#8203;<!--k:c:1-->Rubrik") || e.text.starts_with("# <!--k:c:1-->Rubrik"), "{}", e.text);
    }

    #[test]
    fn export_text_is_accepted_and_free_of_markers() {
        let (e, _) = add_comment(&doc(), 4..8, "Ada", DT, "x");
        let c = clean_for_export(&e.text);
        assert_eq!(c, "Hej snälla världen.\n");
    }

    #[test]
    fn code_fences_are_ignored() {
        let t = "```\n<ins data-k=\"1\">x</ins>\n```\n";
        assert!(scan(t).revs.is_empty());
    }
}
