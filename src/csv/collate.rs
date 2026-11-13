//! Sorteringsordning för celler: svensk kollation (å, ä, ö sist), naturlig ordning för tal i text.

use std::cmp::Ordering;

use super::stats::parse_number;

/// Hur en kolumn sorteras.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortMode {
    /// Tal om alla värden är tal, annars naturlig ordning.
    #[default]
    Auto,
    Number,
    /// Text där sifferföljder jämförs som tal ("fil2" före "fil10").
    Natural,
    Text,
}

/// Grundtecken (utan accent) och om tecknet hade accent.
fn fold(c: char) -> (char, bool) {
    let l = c.to_lowercase().next().unwrap_or(c);
    let b = match l {
        'à' | 'á' | 'â' | 'ã' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' => 'o',
        'ù' | 'ú' | 'û' => 'u',
        'ý' | 'ÿ' | 'ü' => 'y',
        'æ' => 'ä',
        'ø' => 'ö',
        'ß' => 's',
        o => o,
    };
    (b, b != l)
}

const NUM: u32 = 500;

fn primary(b: char) -> u32 {
    match b {
        'a'..='z' => 1000 + (b as u32 - 'a' as u32),
        'å' => 1026,
        'ä' => 1027,
        'ö' => 1028,
        '0'..='9' => NUM + (b as u32 - '0' as u32),
        c if c.is_alphabetic() => 2000 + c as u32,
        c if c.is_whitespace() => 10,
        c => 20 + (c as u32).min(400),
    }
}

/// Nyckel som jämförs lexikografiskt: först grundtecknen, sedan accenter och versaler som avgörare.
pub fn key(s: &str, natural: bool) -> Vec<u32> {
    let mut p: Vec<u32> = Vec::with_capacity(s.len() + 4);
    let mut t: Vec<u32> = Vec::new();
    let chars: Vec<char> = s.trim().chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if natural && c.is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let digits: Vec<char> = chars[i..j].iter().copied().skip_while(|d| *d == '0').collect();
            p.push(NUM);
            p.push(digits.len() as u32);
            p.extend(digits.iter().map(|d| *d as u32 - '0' as u32 + NUM));
            t.extend(chars[i..j].iter().map(|_| 0));
            i = j;
            continue;
        }
        let (b, accent) = fold(c);
        p.push(primary(b));
        t.push(u32::from(accent) * 2 + u32::from(c.is_uppercase()));
        i += 1;
    }
    p.push(0);
    p.extend(t);
    p
}

/// Jämför två texter i svensk ordning.
#[cfg_attr(not(test), allow(dead_code))]
pub fn compare(a: &str, b: &str, natural: bool) -> Ordering {
    key(a, natural).cmp(&key(b, natural)).then_with(|| a.cmp(b))
}

/// Sorteringsnyckel för ett cellvärde.
#[derive(Clone, Debug, PartialEq)]
pub enum CellKey {
    Empty,
    Num(f64),
    Str(Vec<u32>),
}

/// Väljer verkligt läge: `Auto` blir `Number` om alla icke-tomma värden är tal.
pub fn effective_mode(mode: SortMode, values: &[&str]) -> SortMode {
    if mode != SortMode::Auto {
        return mode;
    }
    let mut any = false;
    for v in values {
        if v.trim().is_empty() {
            continue;
        }
        if parse_number(v).is_none() {
            return SortMode::Natural;
        }
        any = true;
    }
    if any { SortMode::Number } else { SortMode::Natural }
}

pub fn cell_key(value: &str, mode: SortMode) -> CellKey {
    if value.trim().is_empty() {
        return CellKey::Empty;
    }
    match mode {
        SortMode::Number => match parse_number(value) {
            Some(n) => CellKey::Num(n),
            None => CellKey::Str(key(value, true)),
        },
        SortMode::Text => CellKey::Str(key(value, false)),
        _ => CellKey::Str(key(value, true)),
    }
}

/// Stigande ordning. Tomma värden hamnar alltid sist och hanteras av anroparen.
pub fn cmp_keys(a: &CellKey, b: &CellKey) -> Ordering {
    match (a, b) {
        (CellKey::Num(x), CellKey::Num(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (CellKey::Num(_), CellKey::Str(_)) => Ordering::Less,
        (CellKey::Str(_), CellKey::Num(_)) => Ordering::Greater,
        (CellKey::Str(x), CellKey::Str(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}
