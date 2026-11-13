//! Positioner och textändringar för LSP: radindex, UTF-16-konvertering, diff och tillämpning av
//! `TextEdit`. Editorn räknar i teckenindex (som egui:s `CCursor`), LSP i (rad, UTF-16-kodenhet),
//! så alla gränser mot servern går via den här filen. Allt är rena funktioner och testas utan gränssnitt.

/// Var raderna börjar, i byte och i tecken. Byggs en gång per text och används för många omvandlingar.
pub struct LineIndex {
    bytes: Vec<usize>,
    chars: Vec<usize>,
    len_chars: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut bytes = vec![0];
        let mut chars = vec![0];
        let mut n = 0;
        for (b, c) in text.char_indices() {
            n += 1;
            if c == '\n' {
                bytes.push(b + 1);
                chars.push(n);
            }
        }
        LineIndex { bytes, chars, len_chars: n }
    }

    #[cfg(test)]
    pub fn len_chars(&self) -> usize {
        self.len_chars
    }

    /// Rad och UTF-16-kolumn till teckenindex. Rader efter slutet ger textens slut, en kolumn efter
    /// radens slut ger radens slut och en kolumn mitt i ett surrogatpar ger tecknets början.
    pub fn pos_to_char(&self, text: &str, line: u32, col16: u32) -> usize {
        let line = line as usize;
        if line >= self.bytes.len() {
            return self.len_chars;
        }
        let mut n = 0;
        let mut u = 0u32;
        for c in text[self.bytes[line]..].chars() {
            if c == '\n' || u + c.len_utf16() as u32 > col16 {
                break;
            }
            u += c.len_utf16() as u32;
            n += 1;
        }
        self.chars[line] + n
    }

    /// Teckenindex till (rad, UTF-16-kolumn). Index efter slutet klipps till slutet.
    pub fn char_to_pos(&self, text: &str, ci: usize) -> (u32, u32) {
        let ci = ci.min(self.len_chars);
        let line = self.chars.partition_point(|&s| s <= ci) - 1;
        let skip = ci - self.chars[line];
        let col: usize = text[self.bytes[line]..].chars().take(skip).map(char::len_utf16).sum();
        (line as u32, col as u32)
    }

    /// Byteindex till teckenindex.
    pub fn byte_to_char(&self, text: &str, b: usize) -> usize {
        let b = b.min(text.len());
        let line = self.bytes.partition_point(|&s| s <= b) - 1;
        self.chars[line] + text[self.bytes[line]..b].chars().count()
    }

    pub fn char_to_byte(&self, text: &str, ci: usize) -> usize {
        let ci = ci.min(self.len_chars);
        let line = self.chars.partition_point(|&s| s <= ci) - 1;
        let skip = ci - self.chars[line];
        let rest = &text[self.bytes[line]..];
        self.bytes[line] + rest.char_indices().nth(skip).map_or(rest.len(), |(b, _)| b)
    }
}

/// Enstaka omvandling (bygger ett radindex: använd `LineIndex` för flera).
pub fn char_to_pos(text: &str, ci: usize) -> (u32, u32) {
    LineIndex::new(text).char_to_pos(text, ci)
}

// ------------------------------------------------------------------ ändringar

/// En ändring på LSP-vis: halvöppet intervall (rad, UTF-16-kolumn).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: (u32, u32),
    pub end: (u32, u32),
    pub new_text: String,
}

/// En ändring i teckenindex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Gör om serverns ändringar till teckenintervall i `text` (som den såg ut när servern räknade).
pub fn to_cedits(text: &str, edits: &[Edit]) -> Vec<CEdit> {
    let idx = LineIndex::new(text);
    edits
        .iter()
        .map(|e| {
            let a = idx.pos_to_char(text, e.start.0, e.start.1);
            let b = idx.pos_to_char(text, e.end.0, e.end.1).max(a);
            CEdit { start: a, end: b, text: e.new_text.clone() }
        })
        .collect()
}

/// Tillämpar ändringar (alla angivna mot samma ursprungstext). Överlappande ändringar hoppas över.
pub fn apply_cedits(text: &mut String, edits: &[CEdit]) {
    let mut order: Vec<&CEdit> = edits.iter().collect();
    // Stabil sortering: ändringar på samma plats behåller serverns ordning.
    order.sort_by_key(|e| (e.start, e.end));
    let mut keep: Vec<&CEdit> = Vec::new();
    for e in order {
        if keep.last().is_some_and(|l| e.start < l.end) {
            continue;
        }
        keep.push(e);
    }
    let idx = LineIndex::new(text);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for e in keep {
        let (b0, b1) = (idx.char_to_byte(text, e.start), idx.char_to_byte(text, e.end));
        out.push_str(&text[at..b0]);
        out.push_str(&e.text);
        at = b1;
    }
    out.push_str(&text[at..]);
    *text = out;
}

/// Var ett teckenindex hamnar efter att `edits` tillämpats (index inuti en ersatt bit hamnar efter den).
pub fn map_index(edits: &[CEdit], ci: usize) -> usize {
    let mut order: Vec<&CEdit> = edits.iter().collect();
    order.sort_by_key(|e| (e.start, e.end));
    let mut delta: isize = 0;
    let mut last_end = 0;
    for e in order {
        if e.start < last_end {
            continue;
        }
        last_end = e.end;
        let new_len = e.text.chars().count();
        if ci >= e.end {
            delta += new_len as isize - (e.end - e.start) as isize;
        } else if ci > e.start {
            return (e.start as isize + delta) as usize + new_len;
        } else {
            break;
        }
    }
    (ci as isize + delta).max(0) as usize
}

/// Skillnaden mellan två texter som en enda ändring (gemensam början och slut utanför).
#[derive(Debug, PartialEq, Eq)]
pub struct Diff {
    /// Byteintervall i den gamla texten.
    pub old: (usize, usize),
    /// Byteintervall i den nya texten.
    pub new: (usize, usize),
}

pub fn diff(old: &str, new: &str) -> Diff {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while p > 0 && !(old.is_char_boundary(p) && new.is_char_boundary(p)) {
        p -= 1;
    }
    let max_s = a.len().min(b.len()) - p;
    let mut s = a.iter().rev().zip(b.iter().rev()).take(max_s).take_while(|(x, y)| x == y).count();
    while s > 0 && !(old.is_char_boundary(a.len() - s) && new.is_char_boundary(b.len() - s)) {
        s -= 1;
    }
    Diff { old: (p, a.len() - s), new: (p, b.len() - s) }
}

/// Textens skillnad som LSP-ändring (mot `old`).
pub fn diff_edit(old: &str, new: &str) -> Edit {
    let d = diff(old, new);
    let idx = LineIndex::new(old);
    let at = |b: usize| idx.char_to_pos(old, idx.byte_to_char(old, b));
    Edit { start: at(d.old.0), end: at(d.old.1), new_text: new[d.new.0..d.new.1].to_owned() }
}


/// Hur teckenindex flyttas när `old` blivit `new` (en enda ersatt bit): före ändringen oförändrat,
/// efter den förskjutet, inuti den till slutet av den nya biten.
#[derive(Clone, Copy, Debug)]
pub struct Remap {
    cs: usize,
    ce: usize,
    new_len: usize,
}

impl Remap {
    pub fn new(old: &str, new: &str) -> Self {
        let d = diff(old, new);
        let cs = old[..d.old.0].chars().count();
        let ce = cs + old[d.old.0..d.old.1].chars().count();
        Remap { cs, ce, new_len: new[d.new.0..d.new.1].chars().count() }
    }

    pub fn map(&self, p: usize) -> usize {
        if p <= self.cs {
            p
        } else if p >= self.ce {
            p + self.new_len - (self.ce - self.cs)
        } else {
            self.cs + self.new_len
        }
    }
}

// ------------------------------------------------------------------ snippets

/// Plattar ut en LSP-snippet till vanlig text: platshållare ger sin standardtext, `$0` (eller första
/// stopp) blir markörens plats. Ger texten och markörens plats i tecken från början.
pub fn flatten_snippet(s: &str) -> (String, usize) {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut stops: Vec<(u32, usize)> = Vec::new();
    let mut i = 0;
    parse_snippet(&chars, &mut i, &mut out, &mut stops, false);
    let n = out.chars().count();
    let pick = stops.iter().find(|(k, _)| *k == 0).or_else(|| stops.iter().min_by_key(|(k, _)| *k));
    (out, pick.map_or(n, |&(_, at)| at))
}

fn parse_snippet(c: &[char], i: &mut usize, out: &mut String, stops: &mut Vec<(u32, usize)>, nested: bool) {
    while *i < c.len() {
        match c[*i] {
            '\\' if *i + 1 < c.len() && matches!(c[*i + 1], '$' | '}' | '\\' | ',' | '|') => {
                out.push(c[*i + 1]);
                *i += 2;
            }
            '}' if nested => return,
            '$' if *i + 1 < c.len() => {
                *i += 1;
                if c[*i].is_ascii_digit() {
                    let mut n = 0u32;
                    while *i < c.len() && c[*i].is_ascii_digit() {
                        n = n.saturating_mul(10).saturating_add(c[*i] as u32 - '0' as u32);
                        *i += 1;
                    }
                    stops.push((n, out.chars().count()));
                } else if c[*i] == '{' {
                    *i += 1;
                    let mut name = String::new();
                    while *i < c.len() && (c[*i].is_ascii_alphanumeric() || c[*i] == '_') {
                        name.push(c[*i]);
                        *i += 1;
                    }
                    if let Ok(n) = name.parse::<u32>() {
                        stops.push((n, out.chars().count()));
                    }
                    match c.get(*i) {
                        Some(':') => {
                            *i += 1;
                            parse_snippet(c, i, out, stops, true);
                        }
                        Some('|') => {
                            // Val: första alternativet.
                            *i += 1;
                            let mut first = true;
                            while *i < c.len() && c[*i] != '|' {
                                if c[*i] == ',' {
                                    first = false;
                                } else if first {
                                    out.push(c[*i]);
                                }
                                *i += 1;
                            }
                            *i += 1;
                        }
                        Some('/') => {
                            // Transformering: hoppa över till `}`.
                            while *i < c.len() && c[*i] != '}' {
                                *i += 1;
                            }
                        }
                        _ => {}
                    }
                    if c.get(*i) == Some(&'}') {
                        *i += 1;
                    }
                } else if c[*i].is_ascii_alphabetic() || c[*i] == '_' {
                    // Variabel ($TM_FILENAME): ingen känd, ger tomt.
                    while *i < c.len() && (c[*i].is_ascii_alphanumeric() || c[*i] == '_') {
                        *i += 1;
                    }
                } else {
                    out.push('$');
                }
            }
            ch => {
                out.push(ch);
                *i += 1;
            }
        }
    }
}

// ------------------------------------------------------------------ URI

/// `file://`-adress för en sökväg (procentkodad).
pub fn path_to_uri(path: &std::path::Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
            // Enhetsbokstav på Windows: `C:`.
            b':' if i == 1 && !s.starts_with('/') => out.push(':'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Sökvägen i en `file://`-adress.
pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
        {
            raw.push(v);
            i += 3;
            continue;
        }
        raw.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(raw).ok()?;
    // `/C:/dir` på Windows.
    let b = s.as_bytes();
    if cfg!(windows) && b.len() > 2 && b[0] == b'/' && b[2] == b':' {
        return Some(std::path::PathBuf::from(&s[1..]));
    }
    Some(std::path::PathBuf::from(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "fn å() {\n    let s = \"ä😀ö\";\n}\n";

    #[test]
    fn utf16_columns() {
        // Rad 1: `    let s = "ä😀ö";`. Emojin är ett tecken men två UTF-16-enheter.
        let idx = LineIndex::new(T);
        let col_a = "    let s = \"".encode_utf16().count() as u32;
        let ci_a = idx.pos_to_char(T, 1, col_a);
        assert_eq!(T.chars().nth(ci_a), Some('ä'));
        let ci_emoji = idx.pos_to_char(T, 1, col_a + 1);
        assert_eq!(T.chars().nth(ci_emoji), Some('😀'));
        let ci_o = idx.pos_to_char(T, 1, col_a + 3);
        assert_eq!(T.chars().nth(ci_o), Some('ö'));
        assert_eq!(idx.char_to_pos(T, ci_o), (1, col_a + 3));
        assert_eq!(idx.char_to_pos(T, ci_emoji), (1, col_a + 1));
        // Mitt i surrogatparet: tecknets början.
        assert_eq!(idx.pos_to_char(T, 1, col_a + 2), ci_emoji);
    }

    #[test]
    fn clamping() {
        let idx = LineIndex::new("ab\ncd");
        assert_eq!(idx.pos_to_char("ab\ncd", 0, 99), 2);
        assert_eq!(idx.pos_to_char("ab\ncd", 1, 99), 5);
        assert_eq!(idx.pos_to_char("ab\ncd", 9, 0), 5);
        assert_eq!(idx.char_to_pos("ab\ncd", 99), (1, 2));
        assert_eq!(char_to_pos("", 0), (0, 0));
        assert_eq!(idx.len_chars(), 5);
    }

    #[test]
    fn byte_and_char_roundtrip() {
        let idx = LineIndex::new(T);
        for (ci, (b, _)) in T.char_indices().enumerate() {
            assert_eq!(idx.char_to_byte(T, ci), b);
            assert_eq!(idx.byte_to_char(T, b), ci);
            let (l, c) = idx.char_to_pos(T, ci);
            assert_eq!(idx.pos_to_char(T, l, c), ci);
        }
    }

    #[test]
    fn edits_apply() {
        let mut t = "åäö 😀 text\nrad två".to_owned();
        let edits = [
            Edit { start: (0, 1), end: (0, 2), new_text: "A".into() },
            Edit { start: (0, 4), end: (0, 6), new_text: "<emoji>".into() },
            Edit { start: (1, 4), end: (1, 7), new_text: "TVÅ".into() },
        ];
        let c = to_cedits(&t, &edits);
        apply_cedits(&mut t, &c);
        assert_eq!(t, "åAö <emoji> text\nrad TVÅ");
    }

    #[test]
    fn edits_order_independent_and_overlap_skipped() {
        let mut t = "abcdef".to_owned();
        let e = [
            CEdit { start: 4, end: 5, text: "X".into() },
            CEdit { start: 0, end: 1, text: "Y".into() },
            CEdit { start: 0, end: 3, text: "overlap".into() },
        ];
        apply_cedits(&mut t, &e);
        assert_eq!(t, "YbcdXf");
    }

    #[test]
    fn insert_at_same_place_keeps_order() {
        let mut t = "ab".to_owned();
        let e = [CEdit { start: 1, end: 1, text: "1".into() }, CEdit { start: 1, end: 1, text: "2".into() }];
        apply_cedits(&mut t, &e);
        assert_eq!(t, "a12b");
    }

    #[test]
    fn index_mapping() {
        let e = [CEdit { start: 2, end: 4, text: "xyz".into() }];
        assert_eq!(map_index(&e, 1), 1);
        assert_eq!(map_index(&e, 4), 5);
        assert_eq!(map_index(&e, 3), 5);
        assert_eq!(map_index(&e, 10), 11);
    }

    #[test]
    fn diff_finds_single_change() {
        let d = diff("hello world", "hello brave world");
        assert_eq!(&"hello brave world"[d.new.0..d.new.1], "brave ");
        assert_eq!(d.old.0, d.old.1);
        let d = diff("abc", "abc");
        assert_eq!(d.old.0, d.old.1);
        assert_eq!(d.new.0, d.new.1);
    }

    #[test]
    fn diff_respects_char_boundaries() {
        // `ä` och `å` delar första UTF-8-byten.
        let (a, b) = ("xä", "xå");
        let d = diff(a, b);
        assert!(a.is_char_boundary(d.old.0) && a.is_char_boundary(d.old.1));
        assert!(b.is_char_boundary(d.new.0) && b.is_char_boundary(d.new.1));
        assert_eq!(&b[d.new.0..d.new.1], "å");
        let e = diff_edit("a😀b", "a😁b");
        assert_eq!(e.start, (0, 1));
        assert_eq!(e.end, (0, 3));
        assert_eq!(e.new_text, "😁");
    }

    #[test]
    fn remap_positions() {
        let r = Remap::new("abcdef", "abXYZdef");
        assert_eq!((r.map(1), r.map(3), r.map(4), r.map(6)), (1, 5, 6, 8));
        let r = Remap::new("same", "same");
        assert_eq!((r.map(0), r.map(4)), (0, 4));
    }

    #[test]
    fn diff_edit_roundtrip() {
        let (old, new) = ("rad ett\nrad åäö\nslut", "rad ett\nrad åxö\nny\nslut");
        let e = diff_edit(old, new);
        let mut t = old.to_owned();
        apply_cedits(&mut t, &to_cedits(old, &[e]));
        assert_eq!(t, new);
    }

    #[test]
    fn snippets() {
        assert_eq!(flatten_snippet("println!(\"$1\")$0"), ("println!(\"\")".into(), 12));
        assert_eq!(flatten_snippet("foo(${1:a}, ${2:b})"), ("foo(a, b)".into(), 4));
        assert_eq!(flatten_snippet("fn ${1:name}($2) {\n\t$0\n}"), ("fn name() {\n\t\n}".into(), 13));
        assert_eq!(flatten_snippet("${1|x,y|}-${2:${3:inner}}"), ("x-inner".into(), 0));
        assert_eq!(flatten_snippet("cost \\$5 \\} $TM_FILENAME."), ("cost $5 } .".into(), 11));
        assert_eq!(flatten_snippet("plain"), ("plain".into(), 5));
        assert_eq!(flatten_snippet("a$"), ("a$".into(), 2));
    }

    #[test]
    fn uris() {
        let p = std::path::Path::new("/tmp/mapp med mellanslag/åäö.rs");
        let u = path_to_uri(p);
        assert_eq!(u, "file:///tmp/mapp%20med%20mellanslag/%C3%A5%C3%A4%C3%B6.rs");
        assert_eq!(uri_to_path(&u).as_deref(), Some(p));
        assert_eq!(uri_to_path("http://x"), None);
        assert_eq!(uri_to_path("file:///a%2").as_deref(), Some(std::path::Path::new("/a%2")));
    }
}
