//! Gissar avgränsare, citattecken och rubrikrad i avgränsad text.

use std::collections::HashMap;

use super::model::{Format, Table};
use super::stats::parse_number;

pub const DELIMS: [u8; 4] = [b',', b';', b'\t', b'|'];

/// Hur sannolikt något ska vara en tabell innan det räknas som en.
const MIN_CONSISTENCY: f32 = 0.85;
const SAMPLE_BYTES: usize = 64 * 1024;
const SAMPLE_ROWS: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sniffed {
    pub fmt: Format,
    /// Första raden ser ut som rubriker.
    pub header: bool,
    /// Antal kolumner i de flesta rader.
    pub cols: usize,
}

/// Filändelser som visas som tabell från början.
pub fn is_table_ext(ext: &str) -> bool {
    matches!(ext, "csv" | "tsv" | "tab" | "psv")
}

/// Filändelser (och namnlösa flikar) där tabellvyn bara erbjuds om innehållet ser ut som en tabell.
pub fn may_be_table_ext(ext: &str) -> bool {
    is_table_ext(ext) || matches!(ext, "txt" | "dat" | "")
}

fn sample(text: &str) -> (&str, bool) {
    if text.len() <= SAMPLE_BYTES {
        return (text, false);
    }
    let mut end = SAMPLE_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // Sista raden är kanske avskuren: sluta efter sista radbrytningen.
    let cut = text[..end].rfind('\n').map_or(end, |i| i + 1);
    (&text[..cut], true)
}

fn quote_char(sample: &str) -> u8 {
    let mut single = 0;
    for line in sample.lines().take(SAMPLE_ROWS) {
        for f in line.split([',', ';', '\t', '|']) {
            let f = f.trim();
            if f.len() >= 2 && f.starts_with('\'') && f.ends_with('\'') {
                single += 1;
            }
            if f.starts_with('"') {
                return b'"';
            }
        }
    }
    if single >= 3 { b'\'' } else { b'"' }
}

/// Andel av kommatecknen som står mellan två siffror ("1,5").
fn decimalish(sample: &str) -> f32 {
    let b = sample.as_bytes();
    let (mut tot, mut dec) = (0, 0);
    for (i, &c) in b.iter().enumerate() {
        if c == b',' {
            tot += 1;
            if i > 0 && i + 1 < b.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit() {
                dec += 1;
            }
        }
    }
    if tot == 0 { 0.0 } else { dec as f32 / tot as f32 }
}

/// (andel rader med vanligaste antalet celler, det antalet)
fn consistency(sample: &str, fmt: Format) -> (f32, usize) {
    let t = Table::parse(sample, fmt);
    let mut counts: HashMap<usize, usize> = HashMap::new();
    let mut rows = 0;
    for r in 0..t.rows().min(SAMPLE_ROWS) {
        // Tomma rader räknas inte.
        if t.row_len(r) == 1 && t.raw(sample, r, 0).trim().is_empty() {
            continue;
        }
        *counts.entry(t.row_len(r)).or_default() += 1;
        rows += 1;
    }
    if rows == 0 {
        return (0.0, 0);
    }
    let (n, c) = counts.into_iter().max_by_key(|(n, c)| (*c, *n)).unwrap_or((0, 0));
    (c as f32 / rows as f32, n)
}

/// Gissar tabellformatet. `None` om texten inte ser ut som en tabell.
pub fn sniff(text: &str, ext: &str) -> Option<Sniffed> {
    let (s, _) = sample(text);
    if !is_table_ext(ext) && s.lines().filter(|l| !l.trim().is_empty()).take(3).count() < 3 {
        return None;
    }
    let quote = quote_char(s);
    let dec = decimalish(s);
    let mut best: Option<(f32, usize, u8)> = None;
    for &d in &DELIMS {
        let (cons, cols) = consistency(s, Format { delim: d, quote });
        if cols < 2 || cons < MIN_CONSISTENCY {
            continue;
        }
        let mut score = cons;
        if d == b',' {
            // "1,5;2,5": kommatecknen är decimaltecken, inte avgränsare.
            score *= 1.0 - 0.5 * dec;
        }
        if (d == b'\t' && ext == "tsv") || (d == b',' && ext == "csv") {
            score += 0.01;
        }
        if best.is_none_or(|(bs, bc, _)| score > bs + 1e-6 || ((score - bs).abs() <= 1e-6 && cols > bc)) {
            best = Some((score, cols, d));
        }
    }
    let (delim, cols) = match best {
        Some((_, cols, d)) => (d, cols),
        None => {
            if !is_table_ext(ext) {
                return None;
            }
            // En kolumn eller oregelbunden fil med tabellfiländelse: använd filändelsens avgränsare.
            let d = if ext == "tsv" || ext == "tab" { b'\t' } else if ext == "psv" { b'|' } else { b',' };
            (d, Table::parse(s, Format { delim: d, quote }).ncols().max(1))
        }
    };
    let fmt = Format { delim, quote };
    Some(Sniffed { fmt, header: guess_header(s, fmt), cols })
}

/// Ser första raden ut som rubriker? Rubriker är text (inga tal), och följande rader har
/// någon kolumn där innehållet är av annan typ, eller så är rubrikerna alla olika.
pub fn guess_header(text: &str, fmt: Format) -> bool {
    let t = Table::parse(text, fmt);
    if t.rows() == 0 {
        return false;
    }
    let first: Vec<String> = (0..t.row_len(0)).map(|c| t.value(text, 0, c).trim().to_owned()).collect();
    if first.iter().all(|c| c.is_empty()) || first.iter().any(|c| parse_number(c).is_some()) {
        return false;
    }
    if t.rows() == 1 {
        return true;
    }
    let mut type_differs = false;
    for c in 0..first.len() {
        if first[c].is_empty() {
            continue;
        }
        let mut nums = 0;
        let mut seen = 0;
        for r in 1..t.rows().min(30) {
            let v = t.value(text, r, c);
            if v.trim().is_empty() {
                continue;
            }
            seen += 1;
            if parse_number(&v).is_some() {
                nums += 1;
            }
        }
        if seen > 0 && nums * 2 > seen {
            type_differs = true;
        }
    }
    let mut uniq = first.clone();
    uniq.sort();
    uniq.dedup();
    let distinct = uniq.len() == first.len() && first.iter().all(|c| !c.is_empty());
    type_differs || distinct
}
