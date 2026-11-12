//! Syntaxfärgning med syntect, i Kladds egna färger, plus markering av sökträffar.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;

use egui::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat};
use egui::{Color32, FontFamily, FontId, Galley};
use syntect::highlighting::Highlighter as SynHighlighter;
use syntect::highlighting::{
    Color, FontStyle, HighlightIterator, HighlightState, ScopeSelectors, StyleModifier, Theme, ThemeItem,
    ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::theme::{self, Palette};

/// Filer större än så här färgas inte (för att editorn ska förbli snabb).
const MAX_HIGHLIGHT_BYTES: usize = 400_000;

pub struct Highlighter {
    syntaxes: SyntaxSet,
    dark: Theme,
    light: Theme,
    docs: HashMap<u64, DocHl>,
}

/// En färgad bit av en rad (relativt radens början).
#[derive(Clone)]
struct Sp {
    start: u32,
    end: u32,
    fg: [u8; 3],
    bold: bool,
    italic: bool,
}

/// Färgning av en rad och parsningstillstånden *före* raden: en ändring behöver bara göra
/// om raderna från den ändrade tills tillståndet efter en rad är detsamma som förut.
struct LineInfo {
    start: usize,
    parse: ParseState,
    hl: HighlightState,
    spans: Vec<Sp>,
}

/// Allt som cachas för ett dokument i textläget.
struct DocHl {
    /// Senast sedda texten (ändringsdetektor: jämförelse, inte hash).
    text: String,
    rev: u64,
    /// Syntax, tema och storlek som färgningen gjordes med.
    style_key: u64,
    /// Rader (och en avslutande tom rad som bär sluttillståndet); `None` = ingen färgning.
    lines: Option<Vec<LineInfo>>,
    sections: Vec<LayoutSection>,
    newlines: usize,
    galley: Option<(u64, Arc<Galley>)>,
}

pub struct Params<'a> {
    pub doc_id: u64,
    pub syntax_hint: &'a str,
    pub palette: &'a Palette,
    pub font_size: f32,
    /// Sökträffar som `(start, slut, är_aktuell)` (se `search::highlights`).
    pub matches: &'a [(usize, usize, bool)],
}

fn to_color(c: Color32) -> Color {
    Color { r: c.r(), g: c.g(), b: c.b(), a: 255 }
}

fn build_theme(p: &Palette) -> Theme {
    let s = &p.syntax;
    let item = |sel: &str, c: Color32, fs: Option<FontStyle>| ThemeItem {
        scope: ScopeSelectors::from_str(sel).expect("giltig scope-selektor"),
        style: StyleModifier { foreground: Some(to_color(c)), background: None, font_style: fs },
    };
    Theme {
        name: None,
        author: None,
        settings: ThemeSettings { foreground: Some(to_color(p.text)), ..Default::default() },
        scopes: vec![
            item("punctuation, meta.brace, keyword.operator", s.punct, None),
            item("keyword, storage, keyword.operator.word", s.keyword, None),
            item("string, constant.character.escape", s.string, None),
            item("constant.numeric, constant.language, constant.other, support.constant", s.number, None),
            item("entity.name.function, support.function, meta.function-call variable.function", s.function, None),
            item("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, support.type, support.class, storage.type.primitive", s.ty, None),
            item("entity.name.tag, meta.tag punctuation.definition.tag", s.tag, None),
            item("entity.other.attribute-name, meta.attribute, variable.other.member, support.type.property-name", s.attr, None),
            item("comment, punctuation.definition.comment", s.comment, Some(FontStyle::ITALIC)),
            item("markup.heading, markup.heading punctuation, entity.name.section", s.heading, Some(FontStyle::BOLD)),
            item("markup.bold", p.text, Some(FontStyle::BOLD)),
            item("markup.italic", p.text, Some(FontStyle::ITALIC)),
            item("markup.raw, markup.inline.raw, markup.raw.block", s.string, None),
            item("markup.underline.link, string.other.link, meta.link", s.function, None),
            item("markup.list punctuation.definition, punctuation.definition.list_item, beginning.punctuation.definition.list", s.heading, None),
            item("markup.quote", p.muted, Some(FontStyle::ITALIC)),
            item("meta.separator, punctuation.definition.thematic-break", p.faint, None),
            item("invalid", p.error, None),
        ],
    }
}

/// Hittar alla träffar av `needle` i `text` som byte-intervall.
pub(crate) fn find_all(text: &str, needle: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    if case_sensitive {
        let mut start = 0;
        while let Some(i) = text[start..].find(needle) {
            let a = start + i;
            out.push((a, a + needle.len()));
            start = a + needle.len();
        }
        return out;
    }
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let mut i = 0;
    while i < text.len() {
        let mut matched = 0;
        let mut end = i;
        for c in text[i..].chars() {
            let mut ok = true;
            for lc in c.to_lowercase() {
                if matched < needle.len() && needle[matched] == lc {
                    matched += 1;
                } else {
                    ok = false;
                    break;
                }
            }
            if !ok {
                matched = usize::MAX;
                break;
            }
            end += c.len_utf8();
            if matched == needle.len() {
                break;
            }
        }
        if matched == needle.len() {
            out.push((i, end));
            i = end;
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            dark: build_theme(&theme::skymning()),
            light: build_theme(&theme::gryning()),
            docs: HashMap::new(),
        }
    }

    pub fn forget(&mut self, doc_id: u64) {
        self.docs.remove(&doc_id);
    }

    fn syntax_for(&self, hint: &str, text: &str) -> Option<&SyntaxReference> {
        if !hint.is_empty() {
            if let Some(s) = self
                .syntaxes
                .find_syntax_by_extension(hint)
                .or_else(|| self.syntaxes.find_syntax_by_name(hint))
            {
                return Some(s);
            }
        }
        let first_line = text.lines().next().unwrap_or("");
        if first_line.starts_with("#!") {
            return self.syntaxes.find_syntax_by_first_line(first_line);
        }
        None
    }

    /// Håller dokumentets färgning i takt med texten. Oförändrad text kostar en jämförelse.
    fn sync(&mut self, text: &str, p: &Params<'_>) {
        let mut h = Fx::default();
        p.syntax_hint.hash(&mut h);
        p.palette.dark.hash(&mut h);
        p.font_size.to_bits().hash(&mut h);
        // Första raden kan välja syntax (`#!`): ändras den färgas allt om.
        text.lines().next().filter(|l| l.starts_with("#!")).hash(&mut h);
        let style_key = h.finish();
        if let Some(d) = self.docs.get(&p.doc_id) {
            if d.style_key == style_key && d.text == text {
                return;
            }
        }
        let mut d = self.docs.remove(&p.doc_id).unwrap_or_else(|| DocHl {
            text: String::new(),
            rev: 0,
            style_key: 0,
            lines: None,
            sections: Vec::new(),
            newlines: 0,
            galley: None,
        });
        let old = if d.style_key == style_key { d.lines.take() } else { None };
        let lines = if text.len() <= MAX_HIGHLIGHT_BYTES {
            self.syntax_for(p.syntax_hint, text).and_then(|syn| self.highlight(syn, text, p, &d.text, old))
        } else {
            None
        };
        d.sections = self.sections(text, lines.as_deref(), p);
        d.lines = lines;
        d.newlines = text.bytes().filter(|&b| b == b'\n').count();
        d.text.clear();
        d.text.push_str(text);
        d.style_key = style_key;
        d.rev += 1;
        self.docs.insert(p.doc_id, d);
    }

    /// Färgar `text`; med `old` (förra färgningen av `old_text`) görs bara raderna om från den
    /// första ändrade tills tillståndet stämmer med det förra igen. `None` vid parsningsfel.
    fn highlight(
        &self,
        syn: &SyntaxReference,
        text: &str,
        p: &Params<'_>,
        old_text: &str,
        old: Option<Vec<LineInfo>>,
    ) -> Option<Vec<LineInfo>> {
        let theme = if p.palette.dark { &self.dark } else { &self.light };
        let hl = SynHighlighter::new(theme);
        let (ob, nb) = (old_text.as_bytes(), text.as_bytes());
        let mut out: Vec<LineInfo> = Vec::new();
        let mut tail: Vec<LineInfo> = Vec::new();
        let mut delta = 0isize;
        let mut suf = 0usize;
        if let Some(mut old) = old.filter(|o| !o.is_empty()) {
            let pre = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
            let n = old.len() - 1; // utan avslutande raden
            let mut k = old[1..].partition_point(|l| l.start <= pre);
            // Sista raden utan radbrytning är inte klar: den kan ha fått mer text.
            if n > 0 && !old_text.ends_with('\n') {
                k = k.min(n - 1);
            }
            tail = old.split_off(k);
            out = old;
            suf = ob[pre..].iter().rev().zip(nb[pre..].iter().rev()).take_while(|(a, b)| a == b).count();
            delta = nb.len() as isize - ob.len() as isize;
        }
        let (mut ps, mut hs, mut pos) = match tail.first() {
            Some(l) => (l.parse.clone(), l.hl.clone(), l.start),
            None => (ParseState::new(syn), HighlightState::new(&hl, ScopeStack::new()), 0),
        };
        loop {
            // Samma slut som förut och samma tillstånd: resten av färgningen gäller fortfarande.
            if !tail.is_empty() && (pos == nb.len() || pos + suf > nb.len()) {
                let os = pos as isize - delta;
                if os >= 0 {
                    if let Ok(j) = tail.binary_search_by(|l| l.start.cmp(&(os as usize))) {
                        if tail[j].parse == ps && tail[j].hl == hs {
                            let mut rest = tail.split_off(j);
                            for l in &mut rest {
                                l.start = (l.start as isize + delta) as usize;
                            }
                            out.extend(rest);
                            return Some(out);
                        }
                    }
                }
            }
            let mut info = LineInfo { start: pos, parse: ps.clone(), hl: hs.clone(), spans: Vec::new() };
            if pos >= nb.len() {
                out.push(info);
                return Some(out);
            }
            let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1);
            let line = &text[pos..end];
            let ops = ps.parse_line(line, &self.syntaxes).ok()?;
            let base = line.as_ptr() as usize;
            for (style, piece) in HighlightIterator::new(&mut hs, &ops, line, &hl) {
                if piece.is_empty() {
                    continue;
                }
                let a = piece.as_ptr() as usize - base;
                let fg = style.foreground;
                info.spans.push(Sp {
                    start: a as u32,
                    end: (a + piece.len()) as u32,
                    fg: [fg.r, fg.g, fg.b],
                    bold: style.font_style.contains(FontStyle::BOLD),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                });
            }
            out.push(info);
            pos = end;
        }
    }

    /// Sektionerna för hela texten (utan sökmarkering).
    fn sections(&self, text: &str, lines: Option<&[LineInfo]>, p: &Params<'_>) -> Vec<LayoutSection> {
        let base = TextFormat {
            font_id: FontId::new(p.font_size, FontFamily::Monospace),
            color: p.palette.text,
            line_height: Some((p.font_size * 1.6).round()),
            ..Default::default()
        };
        let plain = || {
            vec![LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(text.len()), format: base.clone() }]
        };
        let Some(lines) = lines else { return plain() };
        let mut secs: Vec<LayoutSection> = Vec::new();
        let mut last: Option<([u8; 3], bool, bool)> = None;
        for l in lines {
            for sp in &l.spans {
                let (a, b) = (l.start + sp.start as usize, l.start + sp.end as usize);
                let style = (sp.fg, sp.bold, sp.italic);
                if last == Some(style) {
                    if let Some(s) = secs.last_mut() {
                        if s.byte_range.end.0 == a {
                            s.byte_range.end = ByteIndex(b);
                            continue;
                        }
                    }
                }
                let mut fmt = base.clone();
                fmt.color = Color32::from_rgb(sp.fg[0], sp.fg[1], sp.fg[2]);
                if sp.bold {
                    fmt.font_id.family = theme::mono_medium();
                }
                fmt.italics = sp.italic;
                secs.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(a)..ByteIndex(b), format: fmt });
                last = Some(style);
            }
        }
        if secs.is_empty() {
            return plain();
        }
        secs
    }

    /// Antal radbrytningar i `text` (cachat: räknas om bara när texten ändrats).
    pub fn newlines(&mut self, text: &str, p: &Params<'_>) -> usize {
        self.sync(text, p);
        self.docs[&p.doc_id].newlines
    }

    #[cfg(test)]
    pub fn layout(&mut self, text: &str, p: &Params<'_>) -> LayoutJob {
        self.sync(text, p);
        let d = &self.docs[&p.doc_id];
        let mut job = LayoutJob { text: text.to_owned(), sections: d.sections.clone(), ..Default::default() };
        if !p.matches.is_empty() {
            apply_backgrounds(&mut job, p.matches, p.palette);
        }
        job
    }

    /// Färdigt galleri för editorn. Oförändrat läge ger samma galleri utan att något byggs eller
    /// hashas; efter en ändring färgas bara de ändrade raderna om.
    pub fn galley(&mut self, ctx: &egui::Context, text: &str, p: &Params<'_>, max_width: f32) -> Arc<Galley> {
        self.sync(text, p);
        let ppp = ctx.pixels_per_point();
        let families = ctx.fonts(|f| f.families().len());
        let d = &self.docs[&p.doc_id];
        let mut h = Fx::default();
        d.rev.hash(&mut h);
        p.matches.hash(&mut h);
        max_width.to_bits().hash(&mut h);
        ppp.to_bits().hash(&mut h);
        families.hash(&mut h);
        let key = h.finish();
        if let Some((k, g)) = &d.galley {
            if *k == key {
                return g.clone();
            }
        }
        let mut job = LayoutJob { text: text.to_owned(), sections: d.sections.clone(), ..Default::default() };
        if !p.matches.is_empty() {
            apply_backgrounds(&mut job, p.matches, p.palette);
        }
        job.wrap.max_width = max_width;
        let g = ctx.fonts_mut(|f| f.layout_job(job));
        if let Some(d) = self.docs.get_mut(&p.doc_id) {
            d.galley = Some((key, g.clone()));
        }
        g
    }
}

/// Snabb hashfunktion (FxHash) för cachenycklar.
#[derive(Default)]
struct Fx(u64);

impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}


/// Delar upp sektionerna så att sökträffar får bakgrundsfärg.
pub(crate) fn apply_backgrounds(
    job: &mut LayoutJob,
    matches: &[(usize, usize, bool)],
    palette: &Palette,
) {
    let mut out = Vec::with_capacity(job.sections.len() + matches.len() * 2);
    let mut mi = 0;
    for sec in job.sections.drain(..) {
        let (s, e) = (sec.byte_range.start.0, sec.byte_range.end.0);
        let mut pos = s;
        while mi < matches.len() && matches[mi].1 <= pos {
            mi += 1;
        }
        let mut k = mi;
        while pos < e {
            let (next, bg) = match matches.get(k) {
                Some(&(ma, mb, cur)) if ma <= pos => {
                    let bg = if cur {
                        palette.match_current_bg
                    } else {
                        palette.match_bg
                    };
                    (mb.min(e), Some(bg))
                }
                Some(&(ma, _, _)) => (ma.min(e), None),
                None => (e, None),
            };
            let mut fmt = sec.format.clone();
            if let Some(bg) = bg {
                fmt.background = bg;
            }
            out.push(LayoutSection {
                leading_space: 0.0,
                byte_range: ByteIndex(pos)..ByteIndex(next),
                format: fmt,
            });
            pos = next;
            if let Some(&(_, mb, _)) = matches.get(k) {
                if mb <= pos {
                    k += 1;
                }
            }
        }
    }
    job.sections = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_case_insensitive() {
        let t = "Åsa åt ÅSA-glass, åsa!";
        let m = find_all(t, "åsa", false);
        assert_eq!(m.len(), 3);
        for (a, b) in &m {
            assert_eq!(t[*a..*b].to_lowercase(), "åsa");
        }
        assert_eq!(find_all(t, "Åsa", true).len(), 1);
        assert!(find_all(t, "", false).is_empty());
    }

    /// Inkrementell färgning (bara ändrade rader) ska ge exakt samma sektioner som full färgning.
    #[test]
    fn incremental_highlight_equals_full() {
        use crate::word::bench::{big_markdown, Rng};
        let pal = theme::palette(true);
        let mut inc = Highlighter::new();
        let mut full = Highlighter::new();
        let mut rng = Rng(99);
        for (hint, base) in [
            ("md", format!("```rust\nfn a() {{}}\n```\n\n{}", big_markdown(10_000, 5))),
            ("rs", "fn main() {\n    let s = \"hej\";\n    /* kommentar\n       mer */\n    println!(\"{}\", s);\n}\n".repeat(30)),
        ] {
            let mut text = base;
            for step in 0..30 {
                let mut at = rng.below(text.len() + 1);
                while !text.is_char_boundary(at) {
                    at += 1;
                }
                match rng.below(6) {
                    0 => text.insert(at, 'x'),
                    1 => text.insert_str(at, "\n"),
                    2 => text.insert_str(at, "```\n"),
                    3 => text.insert_str(at, "/*"),
                    4 => text.insert_str(at, "\""),
                    _ => {
                        if let Some(c) = text[at..].chars().next() {
                            text.replace_range(at..at + c.len_utf8(), "");
                        }
                    }
                }
                if step == 15 {
                    text.push_str("slut utan radbrytning");
                }
                let params = |id| Params { doc_id: id, syntax_hint: hint, palette: &pal, font_size: 14.0, matches: &[] };
                let a = inc.layout(&text, &params(1));
                full.forget(2);
                let b = full.layout(&text, &params(2));
                assert_eq!(a.sections.len(), b.sections.len(), "{hint} steg {step}");
                for (x, y) in a.sections.iter().zip(&b.sections) {
                    assert_eq!((&x.byte_range, x.format.color, x.format.italics), (&y.byte_range, y.format.color, y.format.italics), "{hint} steg {step}");
                }
            }
        }
    }
}
