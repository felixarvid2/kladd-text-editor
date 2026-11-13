//! Avgränsad text som tabell. Tolkningen sparar var varje cell ligger i texten (byteintervall),
//! och alla ändringar blir `Patch`er på texten som bara rör de berörda raderna. Avgränsare,
//! citattecken, radslut och avslutande radbrytning bevaras, och orörda rader förblir identiska.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;

use super::collate::{self, SortMode};

/// Avgränsare och citattecken (båda ASCII).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Format {
    pub delim: u8,
    pub quote: u8,
}

impl Format {
    pub const COMMA: Format = Format { delim: b',', quote: b'"' };
    pub const TAB: Format = Format { delim: b'\t', quote: b'"' };
}

/// En tolkad text: radernas och cellernas lägen. Håller inte själva texten.
#[derive(Clone, Debug)]
pub struct Table {
    pub fmt: Format,
    cells: Vec<Range<usize>>,
    /// `first[r]..first[r + 1]` är radens celler i `cells`.
    first: Vec<usize>,
    rows: Vec<Range<usize>>,
    /// Radbrytningen efter varje rad (tom på sista raden utan avslutande radbrytning).
    terms: Vec<Range<usize>>,
    ncols: usize,
    eol: &'static str,
}

/// En ändring av texten: byt ut `del` på `pos` mot `ins`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub pos: usize,
    pub del: String,
    pub ins: String,
}

/// Ändringar i stigande ordning utan överlapp, med lägen i texten före ändringen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patch {
    pub edits: Vec<Edit>,
}

impl Patch {
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Stämmer texten på varje plats med det som ska bytas ut?
    pub fn fits(&self, text: &str) -> bool {
        let mut last = 0;
        self.edits.iter().all(|e| {
            let ok = e.pos >= last && text.get(e.pos..e.pos + e.del.len()) == Some(e.del.as_str());
            last = e.pos + e.del.len();
            ok
        })
    }

    pub fn apply(&self, text: &mut String) {
        if self.edits.len() <= 8 {
            for e in self.edits.iter().rev() {
                text.replace_range(e.pos..e.pos + e.del.len(), &e.ins);
            }
            return;
        }
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for e in &self.edits {
            out.push_str(&text[at..e.pos]);
            out.push_str(&e.ins);
            at = e.pos + e.del.len();
        }
        out.push_str(&text[at..]);
        *text = out;
    }

    /// Ändringen som ångrar den här (lägen i texten efter ändringen).
    pub fn inverse(&self) -> Patch {
        let mut shift: isize = 0;
        let edits = self
            .edits
            .iter()
            .map(|e| {
                let pos = (e.pos as isize + shift) as usize;
                shift += e.ins.len() as isize - e.del.len() as isize;
                Edit { pos, del: e.ins.clone(), ins: e.del.clone() }
            })
            .collect();
        Patch { edits }
    }

    /// Byte i texten som ändrats mest, för att veta om minnet för ångra är stort.
    pub fn size(&self) -> usize {
        self.edits.iter().map(|e| e.del.len() + e.ins.len() + 24).sum()
    }
}

impl Table {
    pub fn parse(text: &str, fmt: Format) -> Table {
        let b = text.as_bytes();
        let n = b.len();
        let mut t = Table {
            fmt,
            cells: Vec::new(),
            first: Vec::new(),
            rows: Vec::new(),
            terms: Vec::new(),
            ncols: 0,
            eol: "\n",
        };
        let mut eol_found = false;
        let mut i = 0;
        while i < n {
            let row_start = i;
            t.first.push(t.cells.len());
            loop {
                let fstart = i;
                if i < n && b[i] == fmt.quote {
                    i += 1;
                    loop {
                        match b[i..].iter().position(|&c| c == fmt.quote) {
                            None => {
                                i = n;
                                break;
                            }
                            Some(k) => {
                                i += k + 1;
                                if i < n && b[i] == fmt.quote {
                                    i += 1;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                }
                // Resten av cellen (hela cellen om den inte börjar med citattecken).
                while i < n && b[i] != fmt.delim && b[i] != b'\n' && !(b[i] == b'\r' && i + 1 < n && b[i + 1] == b'\n') {
                    i += 1;
                }
                t.cells.push(fstart..i);
                if i < n && b[i] == fmt.delim {
                    i += 1;
                    continue;
                }
                break;
            }
            let row_end = i;
            let term = if i < n && b[i] == b'\n' {
                1
            } else if i + 1 < n && b[i] == b'\r' && b[i + 1] == b'\n' {
                2
            } else {
                0
            };
            if term == 2 && !eol_found {
                t.eol = "\r\n";
            }
            if term > 0 {
                eol_found = true;
            }
            i += term;
            t.terms.push(row_end..i);
            t.rows.push(row_start..row_end);
            t.ncols = t.ncols.max(t.cells.len() - t.first[t.rows.len() - 1]);
        }
        t.first.push(t.cells.len());
        t
    }

    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    pub fn ncols(&self) -> usize {
        self.ncols
    }

    /// Antal celler i en rad (kortare rader är tillåtna).
    pub fn row_len(&self, r: usize) -> usize {
        self.first[r + 1] - self.first[r]
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn eol(&self) -> &'static str {
        self.eol
    }

    pub fn row_span(&self, r: usize) -> Range<usize> {
        self.rows[r].clone()
    }

    pub fn cell_span(&self, r: usize, c: usize) -> Option<Range<usize>> {
        (r < self.rows() && c < self.row_len(r)).then(|| self.cells[self.first[r] + c].clone())
    }

    /// Cellens text som den står i filen (med citattecken). Tom om cellen saknas.
    pub fn raw<'a>(&self, text: &'a str, r: usize, c: usize) -> &'a str {
        self.cell_span(r, c).map_or("", |s| &text[s])
    }

    /// Cellens värde utan citattecken.
    pub fn value<'a>(&self, text: &'a str, r: usize, c: usize) -> Cow<'a, str> {
        unquote(self.raw(text, r, c), self.fmt.quote)
    }

    /// Raden som cellens råtext.
    fn raw_row<'a>(&self, text: &'a str, r: usize) -> Vec<&'a str> {
        (0..self.row_len(r)).map(|c| self.raw(text, r, c)).collect()
    }

    /// Cellen som innehåller byteläget `pos` (för att hoppa från en sökträff).
    pub fn cell_at(&self, pos: usize) -> Option<(usize, usize)> {
        if self.rows.is_empty() {
            return None;
        }
        let r = self.rows.partition_point(|s| s.end < pos).min(self.rows.len() - 1);
        let (lo, hi) = (self.first[r], self.first[r + 1]);
        let k = self.cells[lo..hi].partition_point(|s| s.end < pos).min(hi - lo - 1);
        Some((r, k))
    }

    // ------------------------------------------------------------ ändringar

    fn row_text(&self, cells: &[String]) -> String {
        cells.join(self.delim_str())
    }

    fn delim_str(&self) -> &'static str {
        match self.fmt.delim {
            b'\t' => "\t",
            b';' => ";",
            b'|' => "|",
            b' ' => " ",
            _ => ",",
        }
    }

    fn blank_row(&self) -> String {
        self.delim_str().repeat(self.ncols.max(1) - 1)
    }

    /// Lägger rader sist i texten. Avslutande radbrytning bevaras som den var.
    fn append_rows(&self, text: &str, rows: &[String]) -> Edit {
        let eol = self.eol;
        let ins = if self.rows.is_empty() || !self.terms[self.rows.len() - 1].is_empty() {
            rows.iter().map(|r| format!("{r}{eol}")).collect::<String>()
        } else {
            format!("{eol}{}", rows.join(eol))
        };
        Edit { pos: text.len(), del: String::new(), ins }
    }

    /// Byter ut hela rader (råtext per cell). Rader efter den sista läggs till, med tomma rader emellan.
    fn replace_rows(&self, text: &str, changes: BTreeMap<usize, Vec<String>>) -> Patch {
        let mut edits = Vec::new();
        let mut appended: Vec<String> = Vec::new();
        for (r, cells) in changes {
            let new = self.row_text(&cells);
            if r < self.rows() {
                let span = self.row_span(r);
                if text[span.clone()] != new {
                    edits.push(Edit { pos: span.start, del: text[span].to_owned(), ins: new });
                }
            } else {
                while self.rows() + appended.len() < r {
                    appended.push(self.blank_row());
                }
                appended.push(new);
            }
        }
        if !appended.is_empty() {
            edits.push(self.append_rows(text, &appended));
        }
        Patch { edits }
    }

    /// Sätter cellvärden. Citerade celler förblir citerade; andra citeras bara när det behövs.
    /// Celler och rader utanför tabellen skapas.
    pub fn set_values(&self, text: &str, changes: &[(usize, usize, String)]) -> Patch {
        let mut by_row: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (r, c, v) in changes {
            let row = by_row.entry(*r).or_insert_with(|| {
                if *r < self.rows() { self.raw_row(text, *r).into_iter().map(str::to_owned).collect() } else { vec![String::new(); self.ncols] }
            });
            if row.len() <= *c {
                row.resize(*c + 1, String::new());
            }
            let was_quoted = row[*c].as_bytes().first() == Some(&self.fmt.quote);
            row[*c] = encode_cell(v, self.fmt, was_quoted);
        }
        self.replace_rows(text, by_row)
    }

    /// Infogar `count` tomma rader före rad `at` (sist i filen om `at` är efter sista raden).
    pub fn insert_rows(&self, text: &str, at: usize, count: usize) -> Patch {
        if count == 0 {
            return Patch::default();
        }
        if at >= self.rows() {
            let rows = vec![self.blank_row(); count];
            return Patch { edits: vec![self.append_rows(text, &rows)] };
        }
        let line = format!("{}{}", self.blank_row(), self.eol);
        Patch { edits: vec![Edit { pos: self.rows[at].start, del: String::new(), ins: line.repeat(count) }] }
    }

    /// Tar bort rader. Saknade filen avslutande radbrytning gör den fortfarande det efteråt.
    pub fn delete_rows(&self, text: &str, rows: &[usize]) -> Patch {
        let n = self.rows();
        let mut del: Vec<usize> = rows.iter().copied().filter(|r| *r < n).collect();
        del.sort_unstable();
        del.dedup();
        if del.is_empty() {
            return Patch::default();
        }
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for &r in &del {
            let rg = self.rows[r].start..self.terms[r].end;
            match ranges.last_mut() {
                Some(l) if l.end == rg.start => l.end = rg.end,
                _ => ranges.push(rg),
            }
        }
        if del.last() == Some(&(n - 1)) && self.terms[n - 1].is_empty() {
            // Raden före de sista borttagna förlorar sin radbrytning.
            let mut k = n - 1;
            while k > 0 && del.binary_search(&(k - 1)).is_ok() {
                k -= 1;
            }
            if k > 0 {
                if let Some(l) = ranges.last_mut() {
                    l.start = self.terms[k - 1].start;
                }
            }
        }
        Patch { edits: ranges.into_iter().map(|a| Edit { pos: a.start, del: text[a].to_owned(), ins: String::new() }).collect() }
    }

    /// Bygger om de rader `f` ger nya celler för.
    fn map_rows(&self, text: &str, mut f: impl FnMut(usize, &[&str]) -> Option<Vec<String>>) -> Patch {
        let mut changes = BTreeMap::new();
        for r in 0..self.rows() {
            let raw = self.raw_row(text, r);
            if let Some(new) = f(r, &raw) {
                changes.insert(r, new);
            }
        }
        self.replace_rows(text, changes)
    }

    /// Infogar `count` tomma kolumner före kolumn `at`.
    pub fn insert_cols(&self, text: &str, at: usize, count: usize) -> Patch {
        self.map_rows(text, |_, raw| {
            if raw.len() < at {
                return None;
            }
            let mut v: Vec<String> = raw.iter().map(|s| (*s).to_owned()).collect();
            for _ in 0..count {
                v.insert(at, String::new());
            }
            Some(v)
        })
    }

    /// Tar bort kolumner.
    pub fn delete_cols(&self, text: &str, cols: &[usize]) -> Patch {
        self.map_rows(text, |_, raw| {
            if !cols.iter().any(|c| *c < raw.len()) {
                return None;
            }
            let v: Vec<String> =
                raw.iter().enumerate().filter(|(i, _)| !cols.contains(i)).map(|(_, s)| (*s).to_owned()).collect();
            // En rad utan celler blir en tom rad.
            Some(if v.is_empty() { vec![String::new()] } else { v })
        })
    }

    /// Sorterar raderna `from..` på kolumn `col`. Tomma värden hamnar sist. `None` om ordningen inte ändras.
    pub fn sort_rows(&self, text: &str, col: usize, desc: bool, mode: SortMode, from: usize) -> Option<Patch> {
        let n = self.rows();
        if from + 1 >= n {
            return None;
        }
        let values: Vec<Cow<str>> = (from..n).map(|r| self.value(text, r, col)).collect();
        let refs: Vec<&str> = values.iter().map(|v| v.as_ref()).collect();
        let mode = collate::effective_mode(mode, &refs);
        let keys: Vec<collate::CellKey> = refs.iter().map(|v| collate::cell_key(v, mode)).collect();
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by(|&a, &b| {
            use std::cmp::Ordering::*;
            match (&keys[a], &keys[b]) {
                (collate::CellKey::Empty, collate::CellKey::Empty) => Equal,
                (collate::CellKey::Empty, _) => Greater,
                (_, collate::CellKey::Empty) => Less,
                (x, y) => {
                    let o = collate::cmp_keys(x, y);
                    if desc { o.reverse() } else { o }
                }
            }
        });
        if order.iter().enumerate().all(|(i, o)| i == *o) {
            return None;
        }
        let start = self.rows[from].start;
        let end = self.terms[n - 1].end;
        let mut out = String::with_capacity(end - start);
        for (slot, &o) in order.iter().enumerate() {
            let r = from + o;
            out.push_str(&text[self.rows[r].clone()]);
            out.push_str(&text[self.terms[from + slot].clone()]);
        }
        let old = &text[start..end];
        if old == out {
            return None;
        }
        Some(Patch { edits: vec![Edit { pos: start, del: old.to_owned(), ins: out }] })
    }
}

/// Värdet i en cell utan citattecken (`""` blir `"`). Text efter avslutande citattecken behålls.
pub fn unquote(raw: &str, quote: u8) -> Cow<'_, str> {
    let q = quote as char;
    if !raw.starts_with(q) {
        return Cow::Borrowed(raw);
    }
    let mut out = String::with_capacity(raw.len());
    let mut it = raw[1..].chars().peekable();
    while let Some(c) = it.next() {
        if c == q {
            if it.peek() == Some(&q) {
                out.push(q);
                it.next();
            } else {
                out.extend(it);
                break;
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Skriver ett värde som cell: med citattecken när det behövs (eller `force`).
pub fn encode_cell(value: &str, fmt: Format, force: bool) -> String {
    let (d, q) = (fmt.delim as char, fmt.quote as char);
    let need = force || value.contains([d, q, '\n', '\r']);
    if !need {
        return value.to_owned();
    }
    let mut s = String::with_capacity(value.len() + 2);
    s.push(q);
    for c in value.chars() {
        if c == q {
            s.push(q);
        }
        s.push(c);
    }
    s.push(q);
    s
}

/// Cellvärden som text att klistra in i Excel eller LibreOffice (tabbar och radbrytningar).
pub fn to_tsv(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|r| r.iter().map(|v| encode_cell(v, Format::TAB, false)).collect::<Vec<_>>().join("\t"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tolkar inklistrad text (tabbavgränsad, med citerade celler) till värden.
pub fn from_tsv(text: &str) -> Vec<Vec<String>> {
    let t = Table::parse(text, Format::TAB);
    (0..t.rows()).map(|r| (0..t.row_len(r)).map(|c| t.value(text, r, c).into_owned()).collect()).collect()
}
