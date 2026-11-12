//! Syntaxfärgning med syntect, i Kladds egna färger, plus markering av sökträffar.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use egui::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat};
use egui::{Color32, FontFamily, FontId};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::theme::{self, Palette};

/// Filer större än så här färgas inte (för att editorn ska förbli snabb).
const MAX_HIGHLIGHT_BYTES: usize = 400_000;

pub struct Highlighter {
    syntaxes: SyntaxSet,
    dark: Theme,
    light: Theme,
    cache: HashMap<u64, (u64, LayoutJob)>,
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
            cache: HashMap::new(),
        }
    }

    pub fn forget(&mut self, doc_id: u64) {
        self.cache.remove(&doc_id);
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

    pub fn layout(&mut self, text: &str, p: &Params<'_>) -> LayoutJob {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        p.syntax_hint.hash(&mut h);
        p.palette.dark.hash(&mut h);
        p.font_size.to_bits().hash(&mut h);
        p.matches.hash(&mut h);
        let key = h.finish();

        if let Some((k, job)) = self.cache.get(&p.doc_id) {
            if *k == key {
                return job.clone();
            }
        }
        let job = self.build(text, p);
        self.cache.insert(p.doc_id, (key, job.clone()));
        job
    }

    fn build(&self, text: &str, p: &Params<'_>) -> LayoutJob {
        let base = TextFormat {
            font_id: FontId::new(p.font_size, FontFamily::Monospace),
            color: p.palette.text,
            line_height: Some((p.font_size * 1.6).round()),
            ..Default::default()
        };

        let mut job = LayoutJob { text: text.to_owned(), ..Default::default() };

        let syntax = (text.len() <= MAX_HIGHLIGHT_BYTES)
            .then(|| self.syntax_for(p.syntax_hint, text))
            .flatten();

        let mut colored = false;
        if let Some(syntax) = syntax {
            let theme = if p.palette.dark { &self.dark } else { &self.light };
            let mut hl = HighlightLines::new(syntax, theme);
            let start_ptr = text.as_ptr() as usize;
            let mut sections = Vec::new();
            let mut ok = true;
            'lines: for line in LinesWithEndings::from(text) {
                match hl.highlight_line(line, &self.syntaxes) {
                    Ok(ranges) => {
                        for (style, piece) in ranges {
                            if piece.is_empty() {
                                continue;
                            }
                            let a = piece.as_ptr() as usize - start_ptr;
                            let fg = style.foreground;
                            let mut fmt = base.clone();
                            fmt.color = Color32::from_rgb(fg.r, fg.g, fg.b);
                            if style.font_style.contains(FontStyle::BOLD) {
                                fmt.font_id.family = theme::mono_medium();
                            }
                            fmt.italics = style.font_style.contains(FontStyle::ITALIC);
                            sections.push(LayoutSection {
                                leading_space: 0.0,
                                byte_range: ByteIndex(a)..ByteIndex(a + piece.len()),
                                format: fmt,
                            });
                        }
                    }
                    Err(_) => {
                        ok = false;
                        break 'lines;
                    }
                }
            }
            if ok && !sections.is_empty() {
                job.sections = sections;
                colored = true;
            }
        }
        if !colored {
            job.sections.push(LayoutSection {
                leading_space: 0.0,
                byte_range: ByteIndex(0)..ByteIndex(text.len()),
                format: base,
            });
        }

        if !p.matches.is_empty() {
            apply_backgrounds(&mut job, p.matches, p.palette);
        }
        job
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
}
