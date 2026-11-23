//! Formatering per körning i sidhuvud och sidfot: fet, kursiv, understruken, färg, storlek och
//! typsnitt för en del av texten (som `w:rPr` per `w:r` i Word och `text:span` i ODF).
//!
//! Mallens text har samma inline-syntax som dokumentets brödtext (se `docs/dev/dokumentlage.md`):
//! `**fet**`, `*kursiv*`, `***fet och kursiv***`, `<u>understruken</u>` och
//! `<span style="color: #C00000; font-size: 14pt; font-family: Arial">…</span>`. Fälten
//! (`{sida}` …) och bildmärkena (`{bild:…}`) kan stå inuti och lämnas orörda. `header-style` är
//! standardstilen för hela sidhuvudet; en körning lägger bara till det som avviker.
//!
//! Ett märke utan par (en ensam `*`, ett `<u>` utan `</u>`) är vanlig text, så att gamla mallar
//! fungerar som förut; `\*` och `\<` skriver ett tecken som annars skulle bli märke.
//!
//! Läsarna (docx, odt) lägger körningarnas format som privata märken i texten medan de läser
//! (`RunMarks`) och skriver ut markupen när sidhuvudets standardstil är känd (`RunMarks::render`).

use crate::hf::HfStyle;
use crate::style::SpanStyle;

/// Formatet för en körning. Fet, kursiv och understruken läggs till standardstilen; färg, storlek
/// och typsnitt ersätter den.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunFmt {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Option<[u8; 3]>,
    /// Punkter.
    pub size: Option<f32>,
    pub family: Option<String>,
}

fn same_size(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => (x - y).abs() < 0.05,
        (None, None) => true,
        _ => false,
    }
}

impl RunFmt {
    pub fn is_default(&self) -> bool {
        *self == RunFmt::default()
    }

    /// Körningens format ovanpå sidhuvudets standardstil (`header-style`).
    pub fn over(&self, base: &HfStyle) -> RunFmt {
        RunFmt {
            bold: self.bold || base.bold,
            italic: self.italic || base.italic,
            underline: self.underline || base.underline,
            color: self.color.or(base.color),
            size: self.size.or(base.size),
            family: self.family.clone(),
        }
    }

    /// Bara det som skiljer sig från standardstilen (det som skrivs som markup).
    fn minus(&self, base: &HfStyle, base_family: Option<&str>, black_none: bool) -> RunFmt {
        RunFmt {
            bold: self.bold && !base.bold,
            italic: self.italic && !base.italic,
            underline: self.underline && !base.underline,
            // Utan färg (Words "automatisk") mot en färgad standardstil är svart.
            color: match (self.color, base.color) {
                (Some(c), b) if b != Some(c) => Some(c),
                (None, Some(_)) if black_none => Some([0, 0, 0]),
                _ => None,
            },
            size: self.size.filter(|s| !same_size(Some(*s), base.size)),
            family: self.family.clone().filter(|f| base_family != Some(f.as_str())),
        }
    }

    fn span(&self) -> SpanStyle {
        SpanStyle { font_family: self.family.clone(), font_size: self.size, color: self.color, background: None }
    }

    fn has_span(&self) -> bool {
        self.family.is_some() || self.size.is_some() || self.color.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Strong,
    Em,
    Under,
    Span,
}

enum Tok {
    Text(String),
    /// Öppnande märke med sin ursprungliga text (blir vanlig text om det saknar par).
    Open(Kind, String, Option<SpanStyle>),
    Close(Kind),
}

/// Är `<` i `rest` början på ett märke (`<u>`, `</u>`, `<span …>`, `</span>`)? Ger sort, om det stänger,
/// spanens stil, märkets text och längd.
fn tag_at(rest: &str) -> Option<(Kind, bool, Option<SpanStyle>, usize)> {
    let end = rest.find('>')? + 1;
    let tag = &rest[..end];
    match tag.to_ascii_lowercase().as_str() {
        "<u>" => Some((Kind::Under, false, None, end)),
        "</u>" => Some((Kind::Under, true, None, end)),
        "</span>" => Some((Kind::Span, true, None, end)),
        _ => crate::style::parse_span_open(tag).map(|s| (Kind::Span, false, Some(s), end)),
    }
}

/// Delar en mall (eller en del av den) i text med format. Fälten och bildmärkena ligger kvar i
/// texten; `\*` och `\<` är utskrivna som `*` och `<`. En tom text ger inga körningar.
pub fn parse_inline(s: &str) -> Vec<(String, RunFmt)> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut text = String::new();
    let mut open: Vec<(Kind, usize)> = Vec::new();
    let mut matched: Vec<bool> = Vec::new();
    let flush = |text: &mut String, toks: &mut Vec<Tok>, matched: &mut Vec<bool>| {
        if !text.is_empty() {
            toks.push(Tok::Text(std::mem::take(text)));
            matched.push(false);
        }
    };
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap_or(' ');
        let step = c.len_utf8();
        match c {
            '\\' if rest[1..].starts_with(['*', '<']) => {
                text.push(rest.as_bytes()[1] as char);
                i += 2;
            }
            // Fält och bildmärken är hela: markup inuti dem tolkas inte.
            '{' => match rest.find('}') {
                Some(j) if !rest[..j].contains('\n') => {
                    text.push_str(&rest[..=j]);
                    i += j + 1;
                }
                _ => {
                    text.push(c);
                    i += step;
                }
            },
            '<' => match tag_at(rest) {
                Some((kind, true, _, len)) => {
                    match open.iter().rposition(|(k, _)| *k == kind) {
                        Some(p) => {
                            let (_, at) = open.remove(p);
                            matched[at] = true;
                            flush(&mut text, &mut toks, &mut matched);
                            toks.push(Tok::Close(kind));
                            matched.push(false);
                        }
                        None => text.push_str(&rest[..len]),
                    }
                    i += len;
                }
                Some((kind, false, sp, len)) => {
                    flush(&mut text, &mut toks, &mut matched);
                    open.push((kind, toks.len()));
                    toks.push(Tok::Open(kind, rest[..len].to_owned(), sp));
                    matched.push(false);
                    i += len;
                }
                None => {
                    text.push(c);
                    i += step;
                }
            },
            '*' => {
                let n = rest.chars().take_while(|c| *c == '*').count();
                let before = s[..i].chars().next_back();
                let after = rest[n..].chars().next();
                let can_open = after.is_some_and(|c| !c.is_whitespace());
                let can_close = before.is_some_and(|c| !c.is_whitespace());
                let kinds: &[Kind] = match n {
                    1 => &[Kind::Em],
                    2 => &[Kind::Strong],
                    3 => &[Kind::Strong, Kind::Em],
                    _ => &[],
                };
                let all_open = !kinds.is_empty() && kinds.iter().all(|k| open.iter().any(|(o, _)| o == k));
                if kinds.is_empty() || !(can_open || (can_close && all_open)) {
                    text.push_str(&rest[..n]);
                } else if can_close && all_open {
                    flush(&mut text, &mut toks, &mut matched);
                    for k in kinds.iter().rev() {
                        if let Some(p) = open.iter().rposition(|(o, _)| o == k) {
                            let (_, at) = open.remove(p);
                            matched[at] = true;
                        }
                        toks.push(Tok::Close(*k));
                        matched.push(false);
                    }
                } else {
                    flush(&mut text, &mut toks, &mut matched);
                    for k in kinds {
                        open.push((*k, toks.len()));
                        toks.push(Tok::Open(*k, if *k == Kind::Strong { "**" } else { "*" }.to_owned(), None));
                        matched.push(false);
                    }
                }
                i += n;
            }
            _ => {
                text.push(c);
                i += step;
            }
        }
    }
    flush(&mut text, &mut toks, &mut matched);
    // Körningarna: tillstånd efter varje märke; märken utan par är text.
    let (mut strong, mut em, mut under) = (0u32, 0u32, 0u32);
    let mut spans: Vec<SpanStyle> = Vec::new();
    let mut out: Vec<(String, RunFmt)> = Vec::new();
    for (t, m) in toks.into_iter().zip(matched) {
        let piece = match t {
            Tok::Text(t) => Some(t),
            Tok::Open(_, lit, _) if !m => Some(lit),
            Tok::Open(k, _, sp) => {
                match k {
                    Kind::Strong => strong += 1,
                    Kind::Em => em += 1,
                    Kind::Under => under += 1,
                    Kind::Span => spans.push(sp.unwrap_or_default()),
                }
                None
            }
            Tok::Close(k) => {
                match k {
                    Kind::Strong => strong = strong.saturating_sub(1),
                    Kind::Em => em = em.saturating_sub(1),
                    Kind::Under => under = under.saturating_sub(1),
                    Kind::Span => {
                        spans.pop();
                    }
                }
                None
            }
        };
        let Some(piece) = piece else { continue };
        let sp = spans.iter().fold(SpanStyle::default(), |a, b| a.merged(b));
        let fmt = RunFmt { bold: strong > 0, italic: em > 0, underline: under > 0, color: sp.color, size: sp.font_size, family: sp.font_family };
        match out.last_mut() {
            Some((t, f)) if *f == fmt => t.push_str(&piece),
            _ => out.push((piece, fmt)),
        }
    }
    out
}

/// Gör tecknen som annars skulle bli märke till `\*` och `\<` (fält och bildmärken lämnas orörda).
pub fn escape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().unwrap_or(' ');
        match c {
            '{' => match rest.find('}') {
                Some(j) if !rest[..j].contains('\n') => {
                    out.push_str(&rest[..=j]);
                    i += j + 1;
                    continue;
                }
                _ => out.push(c),
            },
            '*' => out.push_str("\\*"),
            '<' if tag_at(rest).is_some() => out.push_str("\\<"),
            c => out.push(c),
        }
        i += c.len_utf8();
    }
    out
}

/// `text` (redan med `\*` för tecken som inte ska bli märke) med körningens format som markup.
/// Blanksteg i kanterna ligger utanför märkena, och delarnas skiljetecken (`|`, radbyte) och bildmärken
/// är inte en del av körningen.
pub fn wrap(text: &str, f: &RunFmt) -> String {
    if f.is_default() {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut seg = String::new();
    let flush = |seg: &mut String, out: &mut String| {
        out.push_str(&wrap_one(seg, f));
        seg.clear();
    };
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap_or(' ');
        match c {
            '\\' if rest[1..].starts_with('|') => {
                seg.push_str("\\|");
                i += 2;
            }
            '|' | '\n' => {
                flush(&mut seg, &mut out);
                out.push(c);
                i += 1;
            }
            '{' if rest.starts_with("{bild:") && rest.contains('}') => {
                let j = rest.find('}').unwrap_or(0);
                flush(&mut seg, &mut out);
                out.push_str(&rest[..=j]);
                i += j + 1;
            }
            c => {
                seg.push(c);
                i += c.len_utf8();
            }
        }
    }
    flush(&mut seg, &mut out);
    out
}

fn wrap_one(text: &str, f: &RunFmt) -> String {
    let inner = text.trim();
    if inner.is_empty() {
        return text.to_owned();
    }
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    let mut s = inner.to_owned();
    let stars = match (f.bold, f.italic) {
        (true, true) => "***",
        (true, false) => "**",
        (false, true) => "*",
        _ => "",
    };
    s = format!("{stars}{s}{stars}");
    if f.underline {
        s = format!("<u>{s}</u>");
    }
    if f.has_span() {
        s = format!("{}{s}</span>", f.span().open_tag());
    }
    format!("{lead}{s}{trail}")
}

/// Sätter märken runt `sel` (en markering, byte-intervall) i `text`: fet, kursiv eller understruken.
/// Ligger markeringen redan inom samma märke tas det bort. Ger ny text och ny markering.
pub fn toggle(text: &str, sel: std::ops::Range<usize>, open: &str, close: &str) -> (String, std::ops::Range<usize>) {
    let (a, b) = (sel.start.min(text.len()), sel.end.min(text.len()));
    if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
        return (text.to_owned(), a..a);
    }
    let (pre, mid, post) = (&text[..a], &text[a..b], &text[b..]);
    // Redan omsluten av märket (utanför markeringen): ta bort det.
    if pre.ends_with(open) && post.starts_with(close) {
        let new = format!("{}{mid}{}", &pre[..pre.len() - open.len()], &post[close.len()..]);
        let start = a - open.len();
        return (new, start..start + mid.len());
    }
    // Märket ingår i markeringen: ta bort det.
    if mid.len() >= open.len() + close.len() && mid.starts_with(open) && mid.ends_with(close) {
        let inner = &mid[open.len()..mid.len() - close.len()];
        return (format!("{pre}{inner}{post}"), a..a + inner.len());
    }
    // Blanksteg i markeringens kanter ligger utanför märkena.
    let t = mid.trim();
    if t.is_empty() {
        return (text.to_owned(), a..b);
    }
    let lead = mid.len() - mid.trim_start().len();
    let new = format!("{pre}{}{open}{t}{close}{}{post}", &mid[..lead], &mid[lead + t.len()..]);
    let start = a + lead + open.len();
    (new, start..start + t.len())
}

// ---- märken i texten medan en fil läses

const MARK_OPEN: char = '\u{E000}';
const MARK_CLOSE: char = '\u{E001}';

/// Körningsformaten som läsarna hittat. Index 0 är standard. I texten står `\u{E000}<index>\u{E001}`
/// där formatet byter.
#[derive(Clone, Debug)]
pub struct RunMarks {
    fmts: Vec<RunFmt>,
    /// En körning utan färg mot en standardstil med färg är svart (inte standardfärgen).
    pub black_none: bool,
}

impl Default for RunMarks {
    fn default() -> Self {
        RunMarks { fmts: vec![RunFmt::default()], black_none: false }
    }
}

impl RunMarks {
    fn idx(&mut self, f: &RunFmt) -> usize {
        match self.fmts.iter().position(|x| x == f) {
            Some(i) => i,
            None => {
                self.fmts.push(f.clone());
                self.fmts.len() - 1
            }
        }
    }

    /// Formatet med index `i` (standard om det saknas).
    pub fn fmt_at(&self, i: usize) -> RunFmt {
        self.fmts.get(i).cloned().unwrap_or_default()
    }

    fn put(&self, out: &mut String, i: usize) {
        out.push(MARK_OPEN);
        out.push_str(&i.to_string());
        out.push(MARK_CLOSE);
    }

    /// Lägger `text` (vanlig text, märkena `*` och `<` skyddas) med format `f` sist i `out`. `state` är
    /// formatet som gäller i `out` (0 i en ny text).
    pub fn push_text(&mut self, out: &mut String, state: &mut usize, f: &RunFmt, text: &str) {
        self.push_raw(out, state, f, &escape_inline(text));
    }

    /// Som `push_text` men texten är redan mall (fält, bildmärken).
    pub fn push_raw(&mut self, out: &mut String, state: &mut usize, f: &RunFmt, text: &str) {
        if text.is_empty() {
            return;
        }
        let i = self.idx(f);
        if i != *state {
            self.put(out, i);
            *state = i;
        }
        out.push_str(text);
    }

    /// Återgår till standardformat i slutet av en text (innan den skarvas ihop med andra).
    pub fn close(&self, out: &mut String, state: &mut usize) {
        if *state != 0 {
            self.put(out, 0);
            *state = 0;
        }
    }

    /// Texten som markup. `base` är sidhuvudets standardstil och `base_family` typsnittet som inte
    /// skrivs; körningarna skrivs bara med det som avviker. Utan märken ges texten orörd.
    pub fn render(&self, s: &str, base: &HfStyle, base_family: Option<&str>) -> String {
        if !s.contains(MARK_OPEN) {
            return s.to_owned();
        }
        let mut runs: Vec<(RunFmt, String)> = Vec::new();
        let mut cur = 0usize;
        let mut buf = String::new();
        let mut chars = s.chars();
        let push = |runs: &mut Vec<(RunFmt, String)>, cur: usize, buf: &mut String| {
            if buf.is_empty() {
                return;
            }
            let f = self.fmts.get(cur).cloned().unwrap_or_default().minus(base, base_family, self.black_none);
            match runs.last_mut() {
                Some((lf, t)) if *lf == f => t.push_str(buf),
                _ => runs.push((f, buf.clone())),
            }
            buf.clear();
        };
        while let Some(c) = chars.next() {
            if c == MARK_OPEN {
                push(&mut runs, cur, &mut buf);
                let n: String = chars.by_ref().take_while(|c| *c != MARK_CLOSE).collect();
                cur = n.parse().unwrap_or(0);
            } else {
                buf.push(c);
            }
        }
        push(&mut runs, cur, &mut buf);
        runs.iter().map(|(f, t)| wrap(t, f)).collect()
    }
}

/// Körningarnas format medan en del läses: märken i texten och vad stycket innehåller, så att
/// standardstilen (`header-style`) kan väljas när stycket är läst.
#[derive(Default)]
pub struct RunAcc {
    pub marks: RunMarks,
    /// Formatet som gäller sist i texten som byggs.
    pub state: usize,
    /// Format på styckets synliga innehåll (en post per körning med text, fält).
    pub para: Vec<RunFmt>,
    /// Typsnittet i första körningen med text; skrivs inte, bara avvikande typsnitt blir `font-family`.
    pub base_family: Option<String>,
    pub family_set: bool,
}

impl RunAcc {
    pub fn new_para(&mut self) {
        self.state = 0;
        self.para.clear();
    }

    /// Lägger text (`raw`: redan mall, t.ex. ett fält) med körningens format sist i `cur`.
    pub fn put(&mut self, cur: &mut String, fmt: &RunFmt, text: &str, raw: bool) {
        // Blanksteg följer med i formatet som redan gäller (ett fetstilsmellanrum ska inte bryta en körning).
        let blank = text.trim().is_empty();
        let f = if blank { self.marks.fmt_at(self.state) } else { fmt.clone() };
        if !blank {
            if !self.family_set {
                self.family_set = true;
                self.base_family = fmt.family.clone();
            }
            self.para.push(fmt.clone());
        }
        if raw {
            self.marks.push_raw(cur, &mut self.state, &f, text)
        } else {
            self.marks.push_text(cur, &mut self.state, &f, text)
        }
    }

    /// Slutet på en text som ska skarvas ihop med andra.
    pub fn close(&mut self, cur: &mut String) {
        self.marks.close(cur, &mut self.state);
    }
}

/// `trim` som lämnar märkena kvar: blanksteg i kanterna tas bort även när ett märke står emellan.
/// En text utan synliga tecken blir tom.
pub fn trim_marked(s: &str) -> String {
    if !s.contains(MARK_OPEN) {
        return s.trim().to_owned();
    }
    // Delar: (ett märke, ett tecken).
    let mut parts: Vec<(bool, String)> = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == MARK_OPEN {
            let mut m = String::from(MARK_OPEN);
            for c in chars.by_ref() {
                m.push(c);
                if c == MARK_CLOSE {
                    break;
                }
            }
            parts.push((true, m));
        } else {
            parts.push((false, c.to_string()));
        }
    }
    let visible = |p: &(bool, String)| !p.0 && !p.1.trim().is_empty();
    let (Some(first), Some(last)) = (parts.iter().position(visible), parts.iter().rposition(visible)) else { return String::new() };
    parts.into_iter().enumerate().filter(|(i, p)| p.0 || (*i >= first && *i <= last)).map(|(_, p)| p.1).collect()
}

/// Texten utan märken.
pub fn strip_marks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut skip = false;
    for c in s.chars() {
        match c {
            MARK_OPEN => skip = true,
            MARK_CLOSE => skip = false,
            c if !skip => out.push(c),
            _ => {}
        }
    }
    out
}

/// Största teckenstorleken (pt) som en mall anger med `font-size`, för radhöjden.
pub fn max_size(template: &str) -> Option<f32> {
    if !template.contains("<span") {
        return None;
    }
    parse_inline(template).into_iter().filter_map(|(_, f)| f.size).reduce(f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(s: &str) -> Vec<(String, RunFmt)> {
        parse_inline(s)
    }

    fn b() -> RunFmt {
        RunFmt { bold: true, ..Default::default() }
    }

    #[test]
    fn plain_text_is_one_default_run() {
        assert_eq!(runs("Skolrapport"), [("Skolrapport".to_owned(), RunFmt::default())]);
        assert!(runs("").is_empty());
        // Gamla mallar med märkestecken utan par är orörda.
        for t in ["5 * 3 = 15", "Konfidentiellt *", "a < b", "* punkt", "<u>aldrig stängd", "2 ** 3", "{sida} * {sidor}"] {
            assert_eq!(runs(t), [(t.to_owned(), RunFmt::default())], "{t}");
        }
    }

    #[test]
    fn inline_markup_gives_runs() {
        let r = runs("**Skolrapport** · Klass 9B");
        assert_eq!(r, [("Skolrapport".to_owned(), b()), (" · Klass 9B".to_owned(), RunFmt::default())]);
        let r = runs("Sida **{sida}** av *{sidor}*");
        assert_eq!(r.len(), 4);
        assert_eq!(r[1], ("{sida}".to_owned(), b()));
        assert_eq!(r[3], ("{sidor}".to_owned(), RunFmt { italic: true, ..Default::default() }));
        let r = runs("<u>a</u>***b***");
        assert_eq!(r[0].1, RunFmt { underline: true, ..Default::default() });
        assert_eq!(r[1].1, RunFmt { bold: true, italic: true, ..Default::default() });
        let r = runs("<span style=\"color: #C00000; font-size: 14pt; font-family: Arial\"><u>**x**</u></span>y");
        assert_eq!(r[0].0, "x");
        assert_eq!(r[0].1, RunFmt { bold: true, underline: true, color: Some([0xC0, 0, 0]), size: Some(14.0), family: Some("Arial".into()), ..Default::default() });
        assert_eq!(r[1], ("y".to_owned(), RunFmt::default()));
    }

    #[test]
    fn escapes_and_tokens_are_literal() {
        assert_eq!(runs("a \\* b \\* c")[0].0, "a * b * c");
        assert_eq!(runs("\\<u>x</u>")[0].0, "<u>x</u>");
        // Markup inuti ett bildmärke tolkas inte.
        let r = runs("**{bild:a*b*.png 10x10}**");
        assert_eq!(r, [("{bild:a*b*.png 10x10}".to_owned(), b())]);
        assert_eq!(escape_inline("2 * 3 <u>x</u> {bild:a*b.png 1x1}"), "2 \\* 3 \\<u>x\\</u> {bild:a*b.png 1x1}");
        assert_eq!(runs(&escape_inline("2 * 3 <u>x</u> *")), [("2 * 3 <u>x</u> *".to_owned(), RunFmt::default())]);
    }

    #[test]
    fn wrap_round_trips_and_hoists_blanks() {
        let f = RunFmt { bold: true, italic: true, underline: true, color: Some([0x11, 0x22, 0x33]), size: Some(12.0), family: None };
        let w = wrap("Hej", &f);
        assert_eq!(w, "<span style=\"font-size: 12pt; color: #112233\"><u>***Hej***</u></span>");
        assert_eq!(runs(&w), [("Hej".to_owned(), f.clone())]);
        assert_eq!(wrap(" Hej ", &b()), " **Hej** ");
        assert_eq!(wrap("   ", &b()), "   ");
        assert_eq!(wrap("x", &RunFmt::default()), "x");
    }

    #[test]
    fn toggle_wraps_and_unwraps_selection() {
        let (t, sel) = toggle("Skolrapport Klass", 0..11, "**", "**");
        assert_eq!(t, "**Skolrapport** Klass");
        assert_eq!(sel, 2..13);
        // Samma markering igen tar bort märket.
        let (t2, sel2) = toggle(&t, sel, "**", "**");
        assert_eq!(t2, "Skolrapport Klass");
        assert_eq!(sel2, 0..11);
        // Blanksteg i kanten hamnar utanför.
        assert_eq!(toggle("a b c", 1..4, "*", "*").0, "a *b* c");
        assert_eq!(toggle("abc", 1..1, "*", "*").0, "abc");
    }

    #[test]
    fn marks_render_relative_to_the_base_style() {
        let mut m = RunMarks::default();
        let mut out = String::new();
        let mut st = 0;
        m.push_text(&mut out, &mut st, &b(), "Skolrapport ");
        m.push_text(&mut out, &mut st, &RunFmt::default(), "· Klass 9B");
        m.push_raw(&mut out, &mut st, &b(), "{sida}");
        m.close(&mut out, &mut st);
        assert_eq!(m.render(&out, &HfStyle::default(), None), "**Skolrapport** · Klass 9B**{sida}**");
        // Fet standardstil: bara det som avviker skrivs.
        let base = HfStyle { bold: true, ..Default::default() };
        assert_eq!(m.render(&out, &base, None), "Skolrapport · Klass 9B{sida}");
        assert_eq!(m.render("ingen markup", &base, None), "ingen markup");
        assert_eq!(strip_marks(&out), "Skolrapport · Klass 9B{sida}");
        assert!(trim_marked(&format!("  {out}")).starts_with(MARK_OPEN));
        assert_eq!(trim_marked(&format!(" {MARK_OPEN}1{MARK_CLOSE}  ")), "");
    }

    #[test]
    fn max_size_reads_spans() {
        assert_eq!(max_size("a <span style=\"font-size: 16pt\">b</span> <span style=\"font-size: 12pt\">c</span>"), Some(16.0));
        assert_eq!(max_size("**a**"), None);
    }
}
