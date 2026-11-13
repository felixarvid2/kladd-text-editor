//! Spårade formateringsändringar (Words `w:rPrChange` och `w:pPrChange`).
//!
//! * Teckenformat: `<fmt data-k="5">text</fmt>` runt det som fick nytt format; det gamla formatet
//!   (fet, kursiv, understruken, struken, typsnitt, storlek, färg, markering) står i postradens `old`.
//! * Styckeformat: markören `<!--k:pm fmt 5-->` sist på stycket; postradens `old` har det gamla slaget
//!   (rubrik, lista, citat) och justeringen.
//! * Godkänn behåller det nya formatet (taggen/markören tas bort). Avvisa återställer det gamla.
//!
//! Ren textlogik; gränssnittet ligger i `word/reviewui.rs`.

use std::ops::Range;

use super::{RevKind, RevRec, cut_ranges, line_of, next_rev_id, pmark_text, scan};
use crate::i18n::t;
use crate::style::{Align, SpanStyle};
use crate::word::markup::{self, Analysis, Block, Inline, Piece, SpanProp};

/// Teckenformatet som spåras.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharFmt {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub span: SpanStyle,
}

impl CharFmt {
    pub fn of(p: &Piece) -> CharFmt {
        CharFmt { bold: p.bold, italic: p.italic, underline: p.underline, strike: p.strike, span: p.span.as_deref().cloned().unwrap_or_default() }
    }

    /// `bius;font-size: 12pt; color: #C00000` (flaggor, sedan CSS).
    pub fn encode(&self) -> String {
        let mut f = String::new();
        for (on, c) in [(self.bold, 'b'), (self.italic, 'i'), (self.underline, 'u'), (self.strike, 's')] {
            if on {
                f.push(c);
            }
        }
        format!("{f};{}", self.span.to_css())
    }

    pub fn decode(s: &str) -> CharFmt {
        let (flags, css) = s.split_once(';').unwrap_or((s, ""));
        CharFmt {
            bold: flags.contains('b'),
            italic: flags.contains('i'),
            underline: flags.contains('u'),
            strike: flags.contains('s'),
            span: SpanStyle::from_css(css),
        }
    }
}

/// Styckeformatet som spåras: slag och justering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParaFmt {
    pub block: Block,
    pub align: Align,
}

impl ParaFmt {
    /// `h2|center`, `bullet|left` …
    pub fn encode(&self) -> String {
        let b = match self.block {
            Block::Heading(n) => format!("h{n}"),
            Block::Quote => "quote".into(),
            Block::Bullet => "bullet".into(),
            Block::Numbered => "numbered".into(),
            Block::Code => "code".into(),
            _ => "normal".into(),
        };
        format!("{b}|{}", self.align.as_attr())
    }

    pub fn decode(s: &str) -> ParaFmt {
        let (b, a) = s.split_once('|').unwrap_or((s, "left"));
        let block = match b {
            "quote" => Block::Quote,
            "bullet" => Block::Bullet,
            "numbered" => Block::Numbered,
            "code" => Block::Code,
            h if h.starts_with('h') => h[1..].parse().map_or(Block::Normal, |n: u8| Block::Heading(n.clamp(1, 6))),
            _ => Block::Normal,
        };
        ParaFmt { block, align: Align::from_attr(a) }
    }
}

// ------------------------------------------------------------------ beskrivning (verktygstips)

/// Namnet på det som blev annorlunda (`Fet`, `Kursiv` …), för "Formaterat: Fet".
pub fn describe_char(old: &CharFmt, new: &CharFmt) -> String {
    let mut v: Vec<&'static str> = Vec::new();
    let flag = |v: &mut Vec<&'static str>, a: bool, b: bool, on: &'static str, off: &'static str| {
        if a != b {
            v.push(if b { on } else { off });
        }
    };
    flag(&mut v, old.bold, new.bold, t("Fet"), t("Inte fet"));
    flag(&mut v, old.italic, new.italic, t("Kursiv"), t("Inte kursiv"));
    flag(&mut v, old.underline, new.underline, t("Understruken"), t("Inte understruken"));
    flag(&mut v, old.strike, new.strike, t("Genomstruken"), t("Inte genomstruken"));
    if old.span.font_family != new.span.font_family {
        v.push(t("Typsnitt"));
    }
    if old.span.font_size != new.span.font_size {
        v.push(t("Teckenstorlek"));
    }
    if old.span.color != new.span.color {
        v.push(t("Textfärg"));
    }
    if old.span.background != new.span.background {
        v.push(t("Markeringsfärg"));
    }
    v.join(", ")
}

fn block_name(b: Block) -> String {
    match b {
        Block::Heading(n) => crate::tf!("Rubrik {n}", n = n),
        Block::Quote => t("Citat").to_owned(),
        Block::Bullet => t("Punktlista").to_owned(),
        Block::Numbered => t("Numrerad lista").to_owned(),
        Block::Code => t("Kod").to_owned(),
        _ => t("Brödtext").to_owned(),
    }
}

fn align_name(a: Align) -> &'static str {
    match a {
        Align::Left => t("Vänsterjusterad"),
        Align::Center => t("Centrerad"),
        Align::Right => t("Högerjusterad"),
        Align::Justify => t("Marginaljusterad"),
    }
}

/// Det nya stycketypen och/eller den nya justeringen, för "Formaterat: Rubrik 2".
pub fn describe_para(old: &ParaFmt, new: &ParaFmt) -> String {
    let mut v: Vec<String> = Vec::new();
    if old.block != new.block {
        v.push(block_name(new.block));
    }
    if old.align != new.align {
        v.push(align_name(new.align).to_owned());
    }
    v.join(", ")
}

// ------------------------------------------------------------------ spåra en formatering

/// Resultatet av [`track_format`]: texten med taggar och markörer, och var de lades in (läge i den
/// otaggade texten och längd), så att markeringen kan flyttas med.
pub struct Tagged {
    pub text: String,
    pub inserts: Vec<(usize, usize)>,
}

impl Tagged {
    /// Flyttar ett läge i den otaggade texten. `end` = slutet på en markering: taggar som börjar
    /// precis där hamnar efter det.
    pub fn map(&self, pos: usize, end: bool) -> usize {
        pos + self.inserts.iter().filter(|(p, _)| if end { *p < pos } else { *p <= pos }).map(|(_, l)| l).sum::<usize>()
    }
}

struct VChar {
    pos: usize,
    len: usize,
    fmt: usize,
    /// Tecknet spåras inte (struken text, egen infogning, redan formaterat). Radbrytningar tas inte med.
    skip: bool,
    ch: char,
}

fn eligible(p: &Piece) -> bool {
    !p.markup
        && !p.list_marker
        && p.image.is_none()
        && p.footnote_ref.is_none()
        && p.footnote_def.is_none()
        && !p.code
        && !matches!(p.block, Block::Table | Block::Code | Block::Footnote)
}

fn intern(v: &mut Vec<CharFmt>, f: CharFmt) -> usize {
    match v.iter().position(|x| *x == f) {
        Some(i) => i,
        None => {
            v.push(f);
            v.len() - 1
        }
    }
}

fn chars_of(text: &str, a: &Analysis, own_ins: &dyn Fn(u32) -> bool, fmts: &mut Vec<CharFmt>) -> Vec<VChar> {
    let mut out = Vec::new();
    for p in a.pieces.iter().filter(|p| eligible(p)) {
        let tracked = eligible(p)
            && !p.rev.is_some_and(|r| r.kind == RevKind::Del || (r.kind == RevKind::Ins && own_ins(r.id)))
            && p.fmt.is_none();
        let f = intern(fmts, CharFmt::of(p));
        for (i, ch) in text[p.range.clone()].char_indices().filter(|(_, c)| *c != '\n') {
            out.push(VChar { pos: p.range.start + i, len: ch.len_utf8(), fmt: f, skip: !tracked, ch });
        }
    }
    out
}

/// Stycketyp och justering per rad med synlig text (rader som är kod, tabell eller bara markup hoppas över):
/// (radens slut, format, har redan en styckeformatering eller är ett eget nytt stycke).
fn para_lines(text: &str, a: &Analysis, own_ins: &dyn Fn(u32) -> bool, sc: &super::Scan) -> Vec<(usize, ParaFmt, bool)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    for line in text.split_inclusive('\n') {
        let ls = off;
        off += line.len();
        let le = ls + line.trim_end_matches('\n').len();
        if text[ls..le].trim().is_empty() {
            continue;
        }
        let first = a.pieces.iter().find(|p| p.range.end > ls && p.range.start < le && !p.markup && !p.list_marker);
        let Some(p) = first else { continue };
        if !eligible(p) {
            continue;
        }
        let fixed = sc.pmarks.iter().any(|m| m.range.start >= ls && m.range.end <= le && (m.kind == RevKind::Fmt || m.kind == RevKind::Ins))
            || p.rev.is_some_and(|r| r.kind == RevKind::Ins && own_ins(r.id));
        out.push((le, ParaFmt { block: p.block, align: p.align }, fixed));
    }
    out
}

/// Gör om en formatering (`old` → `new`, gjord av en `markup`-operation som bara ändrar markup, inte text)
/// till en spårad: varje stycke text som fick ett annat teckenformat får `<fmt>` med det gamla formatet i
/// en post, och varje stycke som fick annan typ eller justering får `<!--k:pm fmt-->`. `None` om
/// inget spårbart ändrats (eller texten inte är densamma).
pub fn track_format(old: &str, new: &str, author: &str, date: &str) -> Option<Tagged> {
    if old == new {
        return None;
    }
    let (sc_old, sc_new) = (scan(old), scan(new));
    let own = |id: u32| sc_old.rev_rec(id).is_some_and(|r| r.author == author);
    let (a_old, a_new) = (markup::analyze(old), markup::analyze(new));
    let (mut f_old, mut f_new) = (Vec::new(), Vec::new());
    let (c_old, c_new) = (chars_of(old, &a_old, &own, &mut f_old), chars_of(new, &a_new, &own, &mut f_new));
    if c_old.len() != c_new.len() || c_old.iter().zip(&c_new).any(|(a, b)| a.ch != b.ch) {
        return None;
    }
    // Segment: följder av tecken med samma gamla och nya format, på en rad, utan taggar emellan.
    let mut segs: Vec<(usize, Range<usize>)> = Vec::new(); // (gammalt format, byteintervall i `new`)
    let mut cur: Option<(usize, usize, usize, usize)> = None; // (gammalt, nytt, start, slut)
    let flush = |cur: &mut Option<(usize, usize, usize, usize)>, segs: &mut Vec<(usize, Range<usize>)>| {
        if let Some((o, _, s, e)) = cur.take() {
            segs.push((o, s..e));
        }
    };
    for (o, n) in c_old.iter().zip(&c_new) {
        let changed = !o.skip && !n.skip && f_old[o.fmt] != f_new[n.fmt];
        if !changed {
            flush(&mut cur, &mut segs);
            continue;
        }
        match cur.as_mut() {
            Some((co, cn, _, end)) if *co == o.fmt && *cn == n.fmt && !new[*end..n.pos].contains(['<', '\n']) => *end = n.pos + n.len,
            _ => {
                flush(&mut cur, &mut segs);
                cur = Some((o.fmt, n.fmt, n.pos, n.pos + n.len));
            }
        }
    }
    flush(&mut cur, &mut segs);

    // Stycken.
    let (l_old, l_new) = (para_lines(old, &a_old, &own, &sc_old), para_lines(new, &a_new, &own, &sc_new));
    let mut para: Vec<(usize, ParaFmt)> = Vec::new(); // (radens slut i `new`, gammalt format)
    if l_old.len() == l_new.len() {
        for (o, n) in l_old.iter().zip(&l_new) {
            if o.1 != n.1 && !o.2 && !n.2 {
                para.push((n.0, o.1));
            }
        }
    }
    if segs.is_empty() && para.is_empty() {
        return None;
    }

    let first_new = next_rev_id(&sc_new);
    let mut next_id = first_new;
    let mut recs: Vec<RevRec> = sc_new.rev_recs.iter().map(|(r, _)| r.clone()).collect();
    // Ett nummer per gammalt format (teckenformat och styckeformat skiljs av sin kodning).
    let mut id_for = |enc: String, recs: &mut Vec<RevRec>| -> u32 {
        if let Some(r) = recs.iter().find(|r| r.id >= first_new && r.old.as_deref() == Some(enc.as_str())) {
            return r.id;
        }
        let id = next_id;
        next_id += 1;
        recs.push(RevRec { id, author: author.to_owned(), date: date.to_owned(), old: Some(enc) });
        id
    };
    // (läge, ordning, text): `</fmt>` före `<fmt>` före markören.
    let mut ins: Vec<(usize, u8, String)> = Vec::new();
    for (o, r) in &segs {
        let id = id_for(f_old[*o].encode(), &mut recs);
        ins.push((r.start, 1, super::open_tag(RevKind::Fmt, id)));
        ins.push((r.end, 0, super::close_tag(RevKind::Fmt)));
    }
    for (end, f) in &para {
        let id = id_for(f.encode(), &mut recs);
        ins.push((*end, 2, pmark_text(RevKind::Fmt, id, None)));
    }
    ins.sort_by_key(|(p, o, _)| (*p, *o));
    let mut out = String::with_capacity(new.len() + ins.len() * 24);
    let mut at = 0usize;
    let mut inserts = Vec::new();
    for (p, _, s) in &ins {
        out.push_str(&new[at..*p]);
        out.push_str(s);
        inserts.push((*p, s.len()));
        at = *p;
    }
    out.push_str(&new[at..]);
    let comments: Vec<_> = sc_new.comment_recs.iter().map(|(c, _)| c.clone()).collect();
    let text = super::put_records(&out, &recs, &comments);
    Some(Tagged { text, inserts })
}

// ------------------------------------------------------------------ avvisa: återställ det gamla formatet

fn flag_of(f: &CharFmt, m: Inline) -> bool {
    match m {
        Inline::Bold => f.bold,
        Inline::Italic => f.italic,
        Inline::Underline => f.underline,
        Inline::Strike => f.strike,
    }
}

/// Sätter teckenformatet på `range` till `want` med samma operationer som formateringsknapparna.
fn apply_char_fmt(text: &str, range: Range<usize>, want: &CharFmt) -> String {
    let mut t = text.to_owned();
    let mut r = range;
    for m in [Inline::Bold, Inline::Italic, Inline::Underline, Inline::Strike] {
        for _ in 0..2 {
            let a = markup::analyze(&t);
            let act = markup::active(&t, &a, r.clone());
            let cur = flag_of(&CharFmt { bold: act.bold, italic: act.italic, underline: act.underline, strike: act.strike, span: SpanStyle::default() }, m);
            if cur == flag_of(want, m) {
                break;
            }
            let e = markup::toggle_inline(&t, r.clone(), m);
            if e.text == t {
                break;
            }
            t = e.text;
            r = e.sel;
        }
    }
    let a = markup::analyze(&t);
    let act = markup::active(&t, &a, r.clone());
    let mut props: Vec<SpanProp> = Vec::new();
    if act.span.font_family != want.span.font_family {
        props.push(SpanProp::FontFamily(want.span.font_family.clone()));
    }
    if act.span.font_size != want.span.font_size {
        props.push(SpanProp::FontSize(want.span.font_size));
    }
    if act.span.color != want.span.color {
        props.push(SpanProp::Color(want.span.color));
    }
    if act.span.background != want.span.background {
        props.push(SpanProp::Background(want.span.background));
    }
    for p in props {
        let e = markup::set_span_prop(&t, r.clone(), p);
        t = e.text;
        r = e.sel;
    }
    t
}

/// Sätter stycketyp och justering för raden `line` till `want`.
fn apply_para_fmt(text: &str, line: Range<usize>, want: &ParaFmt) -> String {
    let a = markup::analyze(text);
    let cur = a.pieces.iter().find(|p| p.range.end > line.start && p.range.start < line.end && !p.markup && !p.list_marker).map(|p| (p.block, p.align));
    let Some((block, align)) = cur else { return text.to_owned() };
    let mut t = text.to_owned();
    let mut sel = line;
    if block != want.block && !matches!(want.block, Block::Code) && !matches!(block, Block::Code | Block::Table) {
        let e = markup::set_block(&t, sel.clone(), want.block);
        t = e.text;
        sel = e.sel;
    }
    if align != want.align {
        t = markup::set_align(&t, sel, want.align).text;
    }
    t
}

/// Avvisar formateringsändringarna som `pick` väljer: det gamla formatet sätts tillbaka och taggen
/// respektive markören tas bort.
pub(super) fn restore(text: &str, pick: &dyn Fn(u32) -> bool) -> String {
    if !text.contains("<fmt ") && !text.contains("<!--k:pm fmt ") {
        return text.to_owned();
    }
    let mut text = text.to_owned();
    // Varje varv tar bort en tagg eller markör, så slingan tar slut.
    loop {
        let sc = scan(&text);
        if let Some(r) = sc.revs.iter().find(|r| r.kind == RevKind::Fmt && pick(r.id)).cloned() {
            let old = sc.rev_rec(r.id).and_then(|x| x.old.as_deref()).map(CharFmt::decode);
            let len = r.inner().len();
            text = cut_ranges(&text, &mut vec![r.open.clone(), r.close.clone()]);
            if let Some(old) = old {
                text = apply_char_fmt(&text, r.open.start..r.open.start + len, &old);
            }
            continue;
        }
        if let Some(m) = sc.pmarks.iter().find(|m| m.kind == RevKind::Fmt && pick(m.id)).cloned() {
            let old = sc.rev_rec(m.id).and_then(|x| x.old.as_deref()).map(ParaFmt::decode);
            let line = line_of(&text, m.range.start);
            let after = line.start..line.end - m.range.len();
            text = cut_ranges(&text, &mut vec![m.range.clone()]);
            if let Some(old) = old {
                text = apply_para_fmt(&text, after, &old);
            }
            continue;
        }
        break;
    }
    text
}
