//! Syntaxfärgning av kodblock (```språk … ```) i dokumentläget och exporterna.
//!
//! Färgningen ger en lista tokens (byteintervall + färgklass) över blockets kod och ändrar aldrig
//! texten. Klasserna är oberoende av tema: dokumentläget, PDF och exporterna väljer färg per klass
//! ur [`CodeColors`], så att samma färgning gäller ljust och mörkt. Syntect-uppsättningen delas med
//! `highlight` (läses in i en bakgrundstråd vid start); tills den finns ger [`tokens`] `Pending` och
//! blocket ritas ofärgat. Färgningen cachas på (språk, kod): att skriva i ett annat stycke färgar
//! inte om blocket.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use egui::Color32;
use syntect::highlighting::{
    Color, FontStyle, HighlightIterator, HighlightState, Highlighter, ScopeSelectors, StyleModifier, Theme, ThemeItem,
    ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference};

use crate::theme::{self, Palette, Syntax};

/// Block större än så här färgas inte.
const MAX_CODE_BYTES: usize = 200_000;
/// Antal färgade block som cachas innan cachen töms.
const CACHE_MAX: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Plain = 0,
    Punct,
    Keyword,
    Str,
    Number,
    Function,
    Type,
    Tag,
    Attr,
    Comment,
    Heading,
    Muted,
    Faint,
    Error,
}

const CLASSES: [Class; 14] = [
    Class::Plain,
    Class::Punct,
    Class::Keyword,
    Class::Str,
    Class::Number,
    Class::Function,
    Class::Type,
    Class::Tag,
    Class::Attr,
    Class::Comment,
    Class::Heading,
    Class::Muted,
    Class::Faint,
    Class::Error,
];

/// En bit av koden med samma färgklass (byteintervall i kodtexten).
#[derive(Clone, Debug, PartialEq)]
pub struct Tok {
    pub range: Range<usize>,
    pub class: Class,
    pub bold: bool,
    pub italic: bool,
}

/// Resultatet av en färgning.
#[derive(Clone, Debug)]
pub enum Hl {
    /// Syntaxuppsättningen är inte inläst än: rita ofärgat nu och försök igen senare.
    Pending,
    /// Inget språk eller okänt språk: ofärgat.
    Plain,
    Tokens(Arc<Vec<Tok>>),
}

/// Färger för klasserna.
#[derive(Clone, Copy, Debug)]
pub struct CodeColors {
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub error: Color32,
    pub syntax: Syntax,
}

impl CodeColors {
    /// Färger för sidan i `pal` (dokumentläget): sidans textfärg och temats syntaxfärger.
    pub fn from_palette(pal: &Palette) -> Self {
        Self { text: pal.page_text, muted: pal.page_muted, faint: pal.page_markup, error: pal.error, syntax: pal.syntax }
    }

    /// Ljust tema på vitt papper, för PDF och filexporter.
    pub fn paper() -> Self {
        Self::from_palette(&theme::gryning())
    }

    pub fn color(&self, c: Class) -> Color32 {
        let s = &self.syntax;
        match c {
            Class::Plain => self.text,
            Class::Punct => s.punct,
            Class::Keyword => s.keyword,
            Class::Str => s.string,
            Class::Number => s.number,
            Class::Function => s.function,
            Class::Type => s.ty,
            Class::Tag => s.tag,
            Class::Attr => s.attr,
            Class::Comment => s.comment,
            Class::Heading => s.heading,
            Class::Muted => self.muted,
            Class::Faint => self.faint,
            Class::Error => self.error,
        }
    }

    pub fn rgb(&self, c: Class) -> [u8; 3] {
        let c = self.color(c);
        [c.r(), c.g(), c.b()]
    }
}

/// Är syntaxuppsättningen inläst (annars väntar färgningen)?
pub fn ready() -> bool {
    crate::highlight::syntaxes_ready()
}

/// Språknamnet ur en inhägnads infosträng: "rust", "rust,ignore", "{.rust}" och "Rust title=x" ger "rust".
pub fn lang_of(info: &str) -> String {
    let first = info.split(|c: char| c.is_whitespace() || c == ',' || c == '{' || c == '}').find(|s| !s.is_empty()).unwrap_or("");
    first.trim_start_matches('.').trim_matches(|c| c == '"' || c == '\'').to_lowercase()
}

/// Bokmärkesnamn som bär ett kodblocks språk i .docx och .odt (se `docs/dev/dokumentlage.md`): `_kladdlang_`
/// och språket (utan språk bara prefixet; .odt skriver det för att hålla intilliggande block isär), där allt utom bokstäver och siffror skrivs som `_` och två hexsiffror per byte. Ett
/// namn som börjar med `_` är dolt i Word, och bokmärken syns inte i texten i något program.
const BM_PREFIX: &str = "_kladdlang_";

pub fn lang_bookmark(lang: &str) -> Option<String> {
    let lang = lang_of(lang);
    let mut name = String::from(BM_PREFIX);
    for b in lang.bytes() {
        if b.is_ascii_alphanumeric() {
            name.push(b as char);
        } else {
            name.push_str(&format!("_{b:02X}"));
        }
    }
    (name.len() <= 40).then_some(name)
}

pub fn lang_from_bookmark(name: &str) -> Option<String> {
    let rest = name.strip_prefix(BM_PREFIX)?;
    let b = rest.as_bytes();
    let mut bytes = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'_' {
            bytes.push(u8::from_str_radix(rest.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            bytes.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok().filter(|l| lang_of(l) == *l)
}

/// Den dolda markörkörningen som bär språket från läsningen av ett kodstycke till Markdown.
const MARK_PREFIX: &str = "<!--kladd-lang:";

pub fn lang_marker(lang: &str) -> String {
    format!("{MARK_PREFIX}{lang}-->")
}

pub fn parse_lang_marker(s: &str) -> Option<&str> {
    s.strip_prefix(MARK_PREFIX)?.strip_suffix("-->")
}

enum Kind {
    Syn(&'static SyntaxReference),
    Toml,
}

fn kind_of(lang: &str) -> Option<Kind> {
    let l = lang_of(lang);
    let key = match l.as_str() {
        "" => return None,
        "toml" => return Some(Kind::Toml),
        "js" | "jsx" | "mjs" | "cjs" | "javascript" | "node" | "ts" | "tsx" | "typescript" => "js",
        "py" | "python" | "python3" | "py3" => "py",
        "sh" | "bash" | "shell" | "zsh" | "console" | "shellscript" | "ksh" => "sh",
        "c++" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "c#" | "csharp" | "cs" => "cs",
        "yml" | "yaml" => "yaml",
        "htm" | "html" | "xhtml" => "html",
        "xml" | "svg" | "xsd" | "xsl" | "plist" => "xml",
        "md" | "markdown" => "md",
        "golang" => "go",
        "jsonc" | "json5" | "geojson" => "json",
        "make" | "makefile" => "make",
        "tex" | "latex" => "tex",
        "patch" | "diff" => "diff",
        "cmd" | "bat" | "batch" => "bat",
        "rs" | "rust" => "rs",
        "rb" | "ruby" => "rb",
        other => other,
    };
    crate::highlight::syntaxes_if_ready()?.find_syntax_by_token(key).filter(|s| s.name != "Plain Text").map(Kind::Syn)
}

/// Är språket känt (för tester)?
#[cfg(test)]
pub fn known(lang: &str) -> bool {
    matches!(lang_of(lang).as_str(), "toml") || kind_of(lang).is_some()
}

static CACHE: OnceLock<Mutex<HashMap<u64, Arc<Vec<Tok>>>>> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

thread_local! {
    /// Antal färgningar som gjorts (inte hämtats ur cachen) på tråden; för tester.
    static RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Antal färgningar på den här tråden hittills (cacheträffar räknas inte).
#[cfg(test)]
pub fn runs() -> usize {
    RUNS.with(|r| r.get())
}

fn key(lang: &str, code: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lang_of(lang).hash(&mut h);
    code.hash(&mut h);
    h.finish()
}

/// Färgar `code` som `lang`. Utan språk, för okänt språk och för stora block ges `Plain`;
/// är syntaxuppsättningen inte inläst ges `Pending` (inget cachas då).
pub fn tokens(lang: &str, code: &str) -> Hl {
    if lang_of(lang).is_empty() || code.is_empty() || code.len() > MAX_CODE_BYTES {
        return Hl::Plain;
    }
    if !ready() && lang_of(lang) != "toml" {
        return Hl::Pending;
    }
    let k = key(lang, code);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(t) = cache.lock().ok().and_then(|c| c.get(&k).cloned()) {
        return if t.is_empty() { Hl::Plain } else { Hl::Tokens(t) };
    }
    RUNS.with(|r| r.set(r.get() + 1));
    let toks = match kind_of(lang) {
        Some(Kind::Toml) => toml_tokens(code),
        Some(Kind::Syn(syn)) => syntect_tokens(syn, code).unwrap_or_default(),
        None => Vec::new(),
    };
    let toks = Arc::new(toks);
    if let Ok(mut c) = cache.lock() {
        if c.len() >= CACHE_MAX {
            c.clear();
        }
        c.insert(k, toks.clone());
    }
    if toks.is_empty() { Hl::Plain } else { Hl::Tokens(toks) }
}

/// Som [`tokens`] men väntar på att syntaxuppsättningen läses in (för exporter).
pub fn tokens_wait(lang: &str, code: &str) -> Hl {
    if !lang_of(lang).is_empty() {
        crate::highlight::syntaxes_wait();
    }
    tokens(lang, code)
}

/// Temat där varje scope ger en färg vars röda kanal är färgklassen.
fn class_theme() -> &'static Theme {
    THEME.get_or_init(|| {
        let item = |sel: &str, c: Class, fs: Option<FontStyle>| ThemeItem {
            scope: ScopeSelectors::from_str(sel).expect("giltig scope-selektor"),
            style: StyleModifier { foreground: Some(Color { r: c as u8, g: 0, b: 0, a: 255 }), background: None, font_style: fs },
        };
        Theme {
            name: None,
            author: None,
            settings: ThemeSettings { foreground: Some(Color { r: 0, g: 0, b: 0, a: 255 }), ..Default::default() },
            scopes: vec![
                item("punctuation, meta.brace, keyword.operator", Class::Punct, None),
                item("keyword, storage, keyword.operator.word", Class::Keyword, None),
                item("string, constant.character.escape", Class::Str, None),
                item("constant.numeric, constant.language, constant.other, support.constant", Class::Number, None),
                item("entity.name.function, support.function, meta.function-call variable.function", Class::Function, None),
                item("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, support.type, support.class, storage.type.primitive", Class::Type, None),
                item("entity.name.tag, meta.tag punctuation.definition.tag", Class::Tag, None),
                item("entity.other.attribute-name, meta.attribute, variable.other.member, support.type.property-name", Class::Attr, None),
                item("comment, punctuation.definition.comment", Class::Comment, Some(FontStyle::ITALIC)),
                item("markup.heading, markup.heading punctuation, entity.name.section", Class::Heading, Some(FontStyle::BOLD)),
                item("markup.bold", Class::Plain, Some(FontStyle::BOLD)),
                item("markup.italic", Class::Plain, Some(FontStyle::ITALIC)),
                item("markup.raw, markup.inline.raw, markup.raw.block", Class::Str, None),
                item("markup.underline.link, string.other.link, meta.link", Class::Function, None),
                item("markup.list punctuation.definition, punctuation.definition.list_item, beginning.punctuation.definition.list", Class::Heading, None),
                item("markup.quote", Class::Muted, Some(FontStyle::ITALIC)),
                item("meta.separator, punctuation.definition.thematic-break", Class::Faint, None),
                item("invalid", Class::Error, None),
            ],
        }
    })
}

fn syntect_tokens(syn: &SyntaxReference, code: &str) -> Option<Vec<Tok>> {
    let ss = crate::highlight::syntaxes_if_ready()?;
    let hl = Highlighter::new(class_theme());
    let mut ps = ParseState::new(syn);
    let mut hs = HighlightState::new(&hl, ScopeStack::new());
    let mut out: Vec<Tok> = Vec::new();
    let mut pos = 0usize;
    while pos < code.len() {
        let end = code[pos..].find('\n').map_or(code.len(), |i| pos + i + 1);
        let line = &code[pos..end];
        let ops = ps.parse_line(line, ss).ok()?;
        for (style, piece) in HighlightIterator::new(&mut hs, &ops, line, &hl) {
            if piece.is_empty() {
                continue;
            }
            let class = CLASSES.get(usize::from(style.foreground.r)).copied().unwrap_or(Class::Plain);
            let at = pos + (piece.as_ptr() as usize - line.as_ptr() as usize);
            let tok = Tok {
                range: at..at + piece.len(),
                class,
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
            };
            push_merged(&mut out, tok);
        }
        pos = end;
    }
    Some(out)
}

fn push_merged(out: &mut Vec<Tok>, t: Tok) {
    if let Some(l) = out.last_mut() {
        if l.class == t.class && l.bold == t.bold && l.italic == t.italic && l.range.end == t.range.start {
            l.range.end = t.range.end;
            return;
        }
    }
    out.push(t);
}

/// TOML finns inte bland syntects grundsyntaxer: en liten radvis färgning (kommentarer, tabeller,
/// nycklar, strängar, tal och sanningsvärden).
fn toml_tokens(code: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    let mut add = |out: &mut Vec<Tok>, r: Range<usize>, class: Class| {
        if r.start < r.end {
            push_merged(out, Tok { range: r, class, bold: false, italic: class == Class::Comment });
        }
    };
    let mut pos = 0usize;
    for line in code.split_inclusive('\n') {
        let base = pos;
        pos += line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        let lead = body.len() - body.trim_start().len();
        let rest = &body[lead..];
        add(&mut out, base..base + lead, Class::Plain);
        if rest.starts_with('#') {
            add(&mut out, base + lead..base + body.len(), Class::Comment);
        } else if rest.starts_with('[') {
            let cut = rest.find('#').unwrap_or(rest.len());
            add(&mut out, base + lead..base + lead + cut, Class::Tag);
            add(&mut out, base + lead + cut..base + body.len(), Class::Comment);
        } else {
            // Nyckel (till första `=` utanför citattecken) och värde.
            let mut i = 0usize;
            let b = rest.as_bytes();
            let mut q: Option<u8> = None;
            let mut eq = None;
            while i < b.len() {
                match (q, b[i]) {
                    (None, b'"' | b'\'') => q = Some(b[i]),
                    (Some(c), x) if x == c => q = None,
                    (None, b'=') => {
                        eq = Some(i);
                        break;
                    }
                    _ => {}
                }
                i += 1;
            }
            let value_from = match eq {
                Some(e) => {
                    let key_end = rest[..e].trim_end().len();
                    add(&mut out, base + lead..base + lead + key_end, Class::Attr);
                    add(&mut out, base + lead + key_end..base + lead + e, Class::Plain);
                    add(&mut out, base + lead + e..base + lead + e + 1, Class::Punct);
                    e + 1
                }
                None => 0,
            };
            toml_value(&mut out, base + lead, rest, value_from, &mut add);
        }
        add(&mut out, base + body.len()..base + line.len(), Class::Plain);
    }
    out
}

fn toml_value(out: &mut Vec<Tok>, at: usize, s: &str, from: usize, add: &mut impl FnMut(&mut Vec<Tok>, Range<usize>, Class)) {
    let b = s.as_bytes();
    let mut i = from;
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c == b'#' {
            add(out, at + i..at + b.len(), Class::Comment);
            return;
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != c {
                if c == b'"' && b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            add(out, at + start..at + i, Class::Str);
        } else if c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'_' | b'.' | b':') {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'+' | b'-' | b'_' | b'.' | b':')) {
                i += 1;
            }
            let w = &s[start..i];
            let numeric = w.starts_with(|ch: char| ch.is_ascii_digit() || ch == '+' || ch == '-') || w == "true" || w == "false" || w == "inf" || w == "nan";
            add(out, at + start..at + i, if numeric { Class::Number } else { Class::Plain });
        } else if matches!(c, b'[' | b']' | b'{' | b'}' | b',') {
            i += 1;
            add(out, at + start..at + i, Class::Punct);
        } else {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
            add(out, at + start..at + i, Class::Plain);
        }
    }
}

/// Delar `code` i bitar `(byteintervall, färgklass, fet, kursiv)` som täcker hela koden utan glapp
/// (ofärgat = `Plain`); utan färgning blir det en enda bit.
pub fn segments(hl: &Hl, code: &str) -> Vec<(Range<usize>, Class, bool, bool)> {
    let Hl::Tokens(t) = hl else {
        return if code.is_empty() { Vec::new() } else { vec![(0..code.len(), Class::Plain, false, false)] };
    };
    let mut out = Vec::new();
    let mut at = 0usize;
    for tok in t.iter() {
        let r = tok.range.start.min(code.len())..tok.range.end.min(code.len());
        if r.start > at {
            out.push((at..r.start, Class::Plain, false, false));
        }
        if r.start < r.end {
            out.push((r.clone(), tok.class, tok.bold, tok.italic));
        }
        at = at.max(r.end);
    }
    if at < code.len() {
        out.push((at..code.len(), Class::Plain, false, false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait() {
        crate::highlight::syntaxes_wait();
    }

    fn toks(lang: &str, code: &str) -> Arc<Vec<Tok>> {
        wait();
        match tokens(lang, code) {
            Hl::Tokens(t) => t,
            other => panic!("väntade färgning av {lang}: {other:?}"),
        }
    }

    fn class_at(t: &[Tok], code: &str, needle: &str) -> Class {
        let at = code.find(needle).unwrap();
        t.iter().find(|k| k.range.contains(&at)).map_or(Class::Plain, |k| k.class)
    }

    #[test]
    fn lang_parsing() {
        assert_eq!(lang_of("rust"), "rust");
        assert_eq!(lang_of("Rust,ignore"), "rust");
        assert_eq!(lang_of("{.python .numberLines}"), "python");
        assert_eq!(lang_of("js title=\"a.js\""), "js");
        assert_eq!(lang_of(""), "");
    }

    #[test]
    fn segments_cover_text_without_changing_it() {
        let code = "fn main() {\n    println!(\"hej\"); // x\n}\n";
        let t = toks("rust", code);
        let segs = segments(&Hl::Tokens(t), code);
        let joined: String = segs.iter().map(|(r, ..)| &code[r.clone()]).collect();
        assert_eq!(joined, code);
        let mut at = 0;
        for (r, ..) in &segs {
            assert_eq!(r.start, at, "glapp eller överlapp");
            at = r.end;
        }
        assert_eq!(at, code.len());
    }

    #[test]
    fn rust_tokens_have_expected_classes() {
        let code = "fn main() {\n    let s = \"hej\"; // kommentar\n    let n = 42;\n}\n";
        let t = toks("rust", code);
        assert_eq!(class_at(&t, code, "fn"), Class::Keyword);
        assert_eq!(class_at(&t, code, "hej"), Class::Str);
        assert_eq!(class_at(&t, code, "kommentar"), Class::Comment);
        assert_eq!(class_at(&t, code, "42"), Class::Number);
    }

    #[test]
    fn aliases_and_other_languages() {
        let py = "def f(x):\n    return 'a' + str(1)\n";
        assert_eq!(class_at(&toks("python", py), py, "def"), Class::Keyword);
        assert_eq!(class_at(&toks("py", py), py, "a"), Class::Str);
        let js = "const a = \"s\";\n";
        assert_eq!(class_at(&toks("ts", js), js, "const"), Class::Keyword);
        let json = "{\"a\": 1, \"b\": true}\n";
        assert_eq!(class_at(&toks("json", json), json, "1"), Class::Number);
        let sh = "echo \"hej\" # x\n";
        assert_eq!(class_at(&toks("bash", sh), sh, "# x"), Class::Comment);
        let yaml = "a: 1\nb: \"x\"\n";
        assert_eq!(class_at(&toks("yml", yaml), yaml, "x"), Class::Str);
        let html = "<div class=\"a\">x</div>\n";
        assert_eq!(class_at(&toks("html", html), html, "div"), Class::Tag);
        let sql = "SELECT a FROM t WHERE b = 1;\n";
        assert_eq!(class_at(&toks("sql", sql), sql, "SELECT"), Class::Keyword);
        let css = "a { color: red; }\n";
        assert!(!toks("css", css).is_empty());
        for l in ["c", "cpp", "java", "go", "xml", "markdown"] {
            assert!(known(l), "{l}");
        }
    }

    #[test]
    fn toml_is_colored() {
        let code = "# kommentar\n[package]\nname = \"kladd\"\nversion = 2\nok = true\n";
        let t = toks("toml", code);
        assert_eq!(class_at(&t, code, "# kommentar"), Class::Comment);
        assert_eq!(class_at(&t, code, "[package]"), Class::Tag);
        assert_eq!(class_at(&t, code, "name"), Class::Attr);
        assert_eq!(class_at(&t, code, "kladd\""), Class::Str);
        assert_eq!(class_at(&t, code, "2"), Class::Number);
        assert_eq!(class_at(&t, code, "true"), Class::Number);
        let segs = segments(&Hl::Tokens(t), code);
        assert_eq!(segs.iter().map(|(r, ..)| &code[r.clone()]).collect::<String>(), code);
    }

    #[test]
    fn unknown_or_missing_language_is_plain() {
        wait();
        assert!(matches!(tokens("", "x = 1"), Hl::Plain));
        assert!(matches!(tokens("klingon", "x = 1"), Hl::Plain));
        let segs = segments(&Hl::Plain, "abc");
        assert_eq!(segs, vec![(0..3, Class::Plain, false, false)]);
    }

    #[test]
    fn coloring_is_cached_per_block() {
        wait();
        let code = "fn a() -> u32 { 1 }\n";
        let _ = tokens("rust", code);
        let before = runs();
        for _ in 0..5 {
            let _ = tokens("rust", code);
        }
        assert_eq!(runs(), before, "oförändrat block ska hämtas ur cachen");
        let _ = tokens("rust", "fn b() {}\n");
        assert_eq!(runs(), before + 1, "ett annat block färgas för sig");
    }

    #[test]
    fn language_bookmarks_round_trip() {
        for l in ["rust", "c++", "c#", "objective-c", "python3", "x_y"] {
            let n = lang_bookmark(l).unwrap();
            assert!(n.starts_with("_kladdlang_") && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'), "{n}");
            assert_eq!(lang_from_bookmark(&n).as_deref(), Some(l));
        }
        assert_eq!(lang_bookmark("").as_deref(), Some("_kladdlang_"));
        assert_eq!(lang_from_bookmark("_kladdlang_").as_deref(), Some(""));
        assert_eq!(lang_bookmark(&"a".repeat(60)), None);
        assert_eq!(lang_from_bookmark("_Toc123"), None);
        assert_eq!(parse_lang_marker(&lang_marker("rust")), Some("rust"));
    }

    #[test]
    fn colors_differ_between_classes() {
        let c = CodeColors::paper();
        assert_ne!(c.rgb(Class::Keyword), c.rgb(Class::Str));
        assert_ne!(c.rgb(Class::Plain), c.rgb(Class::Comment));
    }
}
