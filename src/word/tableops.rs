//! Tabellformat som redigeringsoperationer: skuggning och ramar från högerklicksmenyn, och att
//! kommentarsraden före tabellen (`tablefmt`) följer med när rader och kolumner läggs till eller
//! tas bort. Allt är rena funktioner text -> `Edit`, enhetstestade.

use std::ops::Range;

use super::markup::{self, Edit};
use super::tablefmt::{self, Border, TableFmt};

/// Formatoperationerna i menyn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatOp {
    /// Skugga de markerade cellerna.
    Shade([u8; 3]),
    Unshade,
    /// Rutnät runt alla celler.
    BordersAll,
    /// Inga ramar.
    BordersNone,
}

/// Standardfärgen för "Skugga celler" (ljusblå, som Words tabellmallar).
pub const SHADE: [u8; 3] = [0xDD, 0xEB, 0xF7];

/// Skriver `fmt` som kommentarsrad före tabellen som börjar på `table_start` (ersätter den som
/// finns, tar bort den om formatet blivit standard). `sel` flyttas med.
fn put(text: &str, table_start: usize, fmt: &TableFmt, sel: Range<usize>) -> Edit {
    let (cut, new): (Range<usize>, String) = match tablefmt::before(text, table_start) {
        Some((r, _)) if fmt.is_default() => (r.start..table_start, String::new()),
        Some((r, _)) => (r, fmt.to_comment()),
        None if fmt.is_default() => return Edit { text: text.to_owned(), sel },
        None => (table_start..table_start, format!("{}\n\n", fmt.to_comment())),
    };
    let mut out = String::with_capacity(text.len() + new.len());
    out.push_str(&text[..cut.start]);
    out.push_str(&new);
    out.push_str(&text[cut.end..]);
    let shift = |p: usize| if p >= cut.end { p + new.len() - (cut.end - cut.start) } else { p.min(cut.start) };
    Edit { text: out, sel: shift(sel.start)..shift(sel.end) }
}

/// Cellerna (rad, kolumn) som markeringen täcker, i tabellen som börjar på `t.range.start`.
fn cells_in(text: &str, sel: &Range<usize>) -> Option<(usize, Vec<(usize, usize)>)> {
    let t = markup::table_at(text, sel.start)?;
    let a = markup::table_cell_at(&t, sel.start)?;
    let b = markup::table_cell_at(&t, sel.end).unwrap_or(a);
    let (r0, r1) = (a.0.min(b.0), a.0.max(b.0));
    let (c0, c1) = (a.1.min(b.1), a.1.max(b.1));
    let mut cells = Vec::new();
    for r in r0..=r1 {
        for c in c0..=c1 {
            if t.spans.get(r).and_then(|s| s.get(c)).is_some_and(|&n| n > 0) {
                cells.push((r, c));
            }
        }
    }
    Some((t.range.start, cells))
}

/// Utför en formatoperation på markeringen `sel` (den cell markören står i, eller cellerna
/// markeringen spänner över). Utanför en tabell orört.
pub fn apply_format(text: &str, sel: Range<usize>, op: FormatOp) -> Edit {
    let keep = Edit { text: text.to_owned(), sel: sel.clone() };
    let Some((start, cells)) = cells_in(text, &sel) else { return keep };
    let mut fmt = tablefmt::before(text, start).map(|(_, f)| f).unwrap_or_default();
    match op {
        FormatOp::Shade(c) => {
            for (r, col) in cells {
                fmt.cell_mut(r, col).fill = Some(c);
            }
        }
        FormatOp::Unshade => {
            for (r, col) in cells {
                if let Some(c) = fmt.cells.get_mut(&(r, col)) {
                    c.fill = None;
                }
            }
            fmt.cells.retain(|_, c| *c != Default::default());
        }
        FormatOp::BordersAll => {
            fmt.borders = [Border::GRID; 6];
            for c in fmt.cells.values_mut() {
                c.borders = [None; 4];
            }
        }
        FormatOp::BordersNone => {
            fmt.borders = [Border::NONE; 6];
            for c in fmt.cells.values_mut() {
                c.borders = [None; 4];
            }
            // En tabell utan rutnät och utan egen fetstil på rubriken ser ut som i Word.
        }
    }
    put(text, start, &fmt, sel)
}

/// Raden före tabellen följer med en struktur- eller raderingsoperation: `before` är texten och
/// markören före operationen, `after` resultatet.
pub fn follow_structure(before: &str, pos: usize, op: super::tables::TableOp, after: Edit) -> Edit {
    use super::tables::TableOp;
    let Some(t) = markup::table_at(before, pos) else { return after };
    let Some((meta, mut fmt)) = tablefmt::before(before, t.range.start) else { return after };
    let cell = markup::table_cell_at(&t, pos);
    // Tabellen raderas: raden hör inte längre till något.
    if op == TableOp::DelTable {
        let end = (meta.end + 2).min(after.text.len());
        let mut text = after.text.clone();
        if text.get(meta.start..end).is_some_and(|s| s.starts_with("<!--")) || text.get(meta.start..).is_some_and(|s| s.starts_with(&before[meta.clone()])) {
            // `table_delete` låter raden stå kvar: ta bort den och tomraden efter.
            let tail = text[meta.start..].strip_prefix(&before[meta.clone()]).unwrap_or("");
            let tail = tail.strip_prefix("\n\n").unwrap_or(tail.strip_prefix('\n').unwrap_or(tail));
            text = format!("{}{tail}", &text[..meta.start]);
            let p = after.sel.start.min(meta.start);
            return Edit { text, sel: p..p };
        }
        return after;
    }
    let Some((r, c)) = cell else { return after };
    let span = t.span(r, c);
    match op {
        TableOp::RowAbove => fmt.shift_rows(r, 1),
        TableOp::RowBelow => fmt.shift_rows(r + 1, 1),
        TableOp::DelRow => fmt.shift_rows(r, -1),
        TableOp::ColLeft => fmt.shift_cols(c, 1),
        TableOp::ColRight => fmt.shift_cols(c + span, 1),
        TableOp::DelCol => fmt.shift_cols(c, -1),
        _ => return after,
    }
    // Raden ligger före tabellen och är orörd av operationen: byt ut den på samma plats.
    if after.text.get(meta.clone()) != before.get(meta.clone()) {
        return after;
    }
    let mut out = after.text.clone();
    let new = fmt.to_comment();
    out.replace_range(meta.clone(), &new);
    let shift = |p: usize| if p >= meta.end { p + new.len() - meta.len() } else { p };
    Edit { text: out, sel: shift(after.sel.start)..shift(after.sel.end) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::tables::TableOp;

    const T: &str = "Före\n\n| Namn | Id |\n| --- | --- |\n| Alex | 7 |\n\nEfter\n";

    fn at(t: &str, n: &str) -> usize {
        t.find(n).unwrap()
    }

    #[test]
    fn shading_a_cell_adds_a_format_line_and_removing_it_takes_it_away() {
        let p = at(T, "Namn") + 1;
        let e = apply_format(T, p..p, FormatOp::Shade(SHADE));
        assert!(e.text.contains("<!-- kladd-tabell: cell 0.0 fill=DDEBF7 -->\n\n| Namn"), "{}", e.text);
        // Markören följer med.
        assert_eq!(&e.text[e.sel.start - 1..e.sel.start + 3], "Namn");
        let q = at(&e.text, "Namn") + 1;
        let e2 = apply_format(&e.text, q..q, FormatOp::Unshade);
        assert_eq!(e2.text, T);
    }

    #[test]
    fn shading_a_selection_covers_all_its_cells() {
        let e = apply_format(T, at(T, "Namn")..at(T, "7") + 1, FormatOp::Shade([1, 2, 3]));
        let f = tablefmt::before(&e.text, at(&e.text, "| Namn")).unwrap().1;
        assert_eq!(f.cells.len(), 4, "{:?}", f.cells);
        assert!(f.cells.values().all(|c| c.fill == Some([1, 2, 3])));
    }

    #[test]
    fn borders_none_and_all() {
        let p = at(T, "Alex");
        let none = apply_format(T, p..p, FormatOp::BordersNone);
        assert!(none.text.contains("border top none 0 auto"), "{}", none.text);
        let q = at(&none.text, "Alex");
        let all = apply_format(&none.text, q..q, FormatOp::BordersAll);
        assert_eq!(all.text, T);
    }

    #[test]
    fn outside_a_table_nothing_happens() {
        let e = apply_format(T, 1..1, FormatOp::Shade(SHADE));
        assert_eq!(e.text, T);
    }

    fn with_fill() -> String {
        let p = at(T, "Namn");
        apply_format(T, p..p, FormatOp::Shade(SHADE)).text
    }

    fn fill_at(text: &str, r: usize, c: usize) -> bool {
        let start = text.find("| ").unwrap();
        tablefmt::before(text, start).is_some_and(|(_, f)| f.fill(r, c).is_some())
    }

    #[test]
    fn row_and_column_operations_move_the_shading() {
        let t = with_fill();
        let (at_namn, at_alex) = (at(&t, "Namn"), at(&t, "Alex"));
        // Rad nedanför rubrikraden: skuggningen ligger kvar på rad 0.
        let raw = TableOp::RowBelow.apply_unformatted(&t, at_namn..at_namn);
        let e = follow_structure(&t, at_namn, TableOp::RowBelow, raw);
        assert!(fill_at(&e.text, 0, 0), "{}", e.text);
        // Rad ovanför rubriken: skuggningen flyttas till rad 1.
        let raw = TableOp::RowAbove.apply_unformatted(&t, at_namn..at_namn);
        let e = follow_structure(&t, at_namn, TableOp::RowAbove, raw);
        assert!(fill_at(&e.text, 1, 0) && !fill_at(&e.text, 0, 0), "{}", e.text);
        // Kolumn till vänster om första: skuggningen hamnar i kolumn 1.
        let raw = TableOp::ColLeft.apply_unformatted(&t, at_namn..at_namn);
        let e = follow_structure(&t, at_namn, TableOp::ColLeft, raw);
        assert!(fill_at(&e.text, 0, 1) && !fill_at(&e.text, 0, 0), "{}", e.text);
        // Ta bort rubrikraden: skuggningen försvinner med den (och raden med, när formatet blir standard).
        let raw = TableOp::DelRow.apply_unformatted(&t, at_namn..at_namn);
        let e = follow_structure(&t, at_namn, TableOp::DelRow, raw);
        assert!(!fill_at(&e.text, 0, 0), "{}", e.text);
        // En rad i en annan rad än den skuggade rör inte skuggningen.
        let raw = TableOp::DelRow.apply_unformatted(&t, at_alex..at_alex);
        let e = follow_structure(&t, at_alex, TableOp::DelRow, raw);
        assert!(fill_at(&e.text, 0, 0));
    }

    #[test]
    fn deleting_the_table_deletes_its_format_line() {
        let t = with_fill();
        let p = at(&t, "Namn");
        let raw = TableOp::DelTable.apply_unformatted(&t, p..p);
        let e = follow_structure(&t, p, TableOp::DelTable, raw);
        assert!(!e.text.contains("kladd-tabell") && !e.text.contains("Namn"), "{}", e.text);
        assert!(e.text.contains("Före") && e.text.contains("Efter"));
    }
}
