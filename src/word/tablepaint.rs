//! Ritar tabellernas skuggning och ramar i dokumentläget (formatet ur `tablefmt`). Skuggningen
//! läggs bakom texten (egen ritlucka), ramarna ovanpå. En tabell utan format får rutnätet runt
//! alla celler som förut; en tabell utan ramar får bara en svag hjälplinje medan markören står i den.

use egui::{Color32, Pos2, Rect, Shape, Stroke, Vec2, pos2};

use super::grid::{CellGeom, TableGeom};
use super::layout::{adapt_background, adapt_text_color, px};
use super::tablefmt::{BOTTOM, Border, BStyle, LEFT, RIGHT, TOP, TableFmt};
use crate::theme::Palette;

#[derive(Default)]
pub struct Shapes {
    /// Cellskuggning, bakom texten.
    pub fills: Vec<Shape>,
    /// Ramar, ovanpå texten.
    pub lines: Vec<Shape>,
}

/// Cellen på raden `r` som täcker kolumn `col`.
fn covering(t: &TableGeom, r: usize, col: usize) -> Option<&CellGeom> {
    t.rows.get(r)?.cells.iter().find(|c| c.col <= col && col < c.col + c.span)
}

/// Ritar tabellen. `exts` är radernas (överkant, nederkant) i galleriet, `origin` galleriets
/// läge och `guides` om hjälplinjer ska visas där ramar saknas.
pub fn paint(t: &TableGeom, exts: &[(f32, f32)], origin: Vec2, pal: &Palette, zoom: f32, clip: Rect, guides: bool) -> Shapes {
    let mut out = Shapes::default();
    let default = TableFmt::default();
    let (fmt, plain) = match t.fmt.as_deref() {
        Some(f) => (f, false),
        None => (&default, true),
    };
    let snap = |v: f32| (v - 0.5).round() + 0.5;
    for (ri, row) in t.rows.iter().enumerate() {
        let Some(&(top, bottom)) = exts.get(ri) else { continue };
        // Radens överkant: texten ligger `pad[2]` under den (nederkanten ingår i den sista radens höjd).
        let top = top - t.pad[2];
        let (y0, y1) = (origin.y + top, origin.y + t.row_bottom(ri, top, bottom));
        if y1 < clip.top() || y0 > clip.bottom() {
            continue;
        }
        for (ci, c) in row.cells.iter().enumerate() {
            let (cx0, cx1) = t.cell_x(c);
            let (x0, x1) = (origin.x + cx0, origin.x + cx1);
            if let Some(f) = fmt.fill(ri, c.col) {
                out.fills.push(Shape::rect_filled(Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), 0.0, adapt_background(f, pal.dark)));
            }
            let own = |r: usize, col: usize, side: usize| fmt.own_border(r, col, side);
            // Överkant (mot cellen ovanför) och vänsterkant (mot cellen till vänster).
            let above = ri.checked_sub(1).and_then(|r| covering(t, r, c.col).map(|n| (r, n.col)));
            let top = fmt.edge(TOP, above.and_then(|(r, col)| own(r, col, BOTTOM)), own(ri, c.col, TOP), ri > 0);
            edge(&mut out, top, [pos2(snap(x0), snap(y0)), pos2(snap(x1), snap(y0))], false, pal, zoom, plain, guides);
            let left_nb = ci.checked_sub(1).map(|k| row.cells[k].col);
            let left = fmt.edge(LEFT, left_nb.and_then(|col| own(ri, col, RIGHT)), own(ri, c.col, LEFT), ci > 0);
            edge(&mut out, left, [pos2(snap(x0), snap(y0)), pos2(snap(x0), snap(y1))], true, pal, zoom, plain, guides);
            if ci + 1 == row.cells.len() {
                let right = fmt.edge(RIGHT, own(ri, c.col, RIGHT), None, false);
                edge(&mut out, right, [pos2(snap(x1), snap(y0)), pos2(snap(x1), snap(y1))], true, pal, zoom, plain, guides);
            }
            // Nederkant: sist i tabellen, eller där raden under saknar cell.
            let below = covering(t, ri + 1, c.col);
            if below.is_none() {
                let b = fmt.edge(BOTTOM, own(ri, c.col, BOTTOM), None, false);
                edge(&mut out, b, [pos2(snap(x0), snap(y1)), pos2(snap(x1), snap(y1))], false, pal, zoom, plain, guides);
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn edge(out: &mut Shapes, b: Border, pts: [Pos2; 2], vertical: bool, pal: &Palette, zoom: f32, plain: bool, guides: bool) {
    if !b.visible() {
        // Hjälplinje (som Words rutnätslinjer) bara när man redigerar i tabellen.
        if guides {
            let c = pal.page_text.gamma_multiply(0.18);
            out.lines.extend(Shape::dashed_line(&pts, Stroke::new(1.0, c), 3.0, 3.0));
        }
        return;
    }
    let color: Color32 = match (b.color, plain) {
        (Some(c), _) => adapt_text_color(c, pal.page, pal.dark),
        (None, true) => pal.page_text.gamma_multiply(0.6),
        (None, false) => pal.page_text,
    };
    let lw = px(zoom, b.pt()).max(1.0).round();
    let normal = if vertical { Vec2::X } else { Vec2::Y };
    match b.style {
        BStyle::None => {}
        BStyle::Single => out.lines.push(Shape::line_segment(pts, Stroke::new(lw, color))),
        BStyle::Double => {
            // Två linjer med lika brett mellanrum, jämnt fördelade kring kanten.
            let off = normal * lw;
            out.lines.push(Shape::line_segment([pts[0] - off, pts[1] - off], Stroke::new(lw, color)));
            out.lines.push(Shape::line_segment([pts[0] + off, pts[1] + off], Stroke::new(lw, color)));
        }
        BStyle::Dashed => out.lines.extend(Shape::dashed_line(&pts, Stroke::new(lw, color), 4.0 * lw, 3.0 * lw)),
        BStyle::Dotted => out.lines.extend(Shape::dashed_line(&pts, Stroke::new(lw, color), lw, 2.0 * lw)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::tablefmt::{Border, TableFmt};
    use std::sync::Arc;

    fn geom(fmt: Option<TableFmt>) -> TableGeom {
        let cell = |col: usize| CellGeom { col, span: 1, chunk: 0..0, seg: 0..0 };
        TableGeom {
            region: 0..0,
            col_x: vec![0.0, 100.0, 200.0],
            rows: (0..2).map(|_| super::super::grid::RowGeom { line: 0..0, cells: vec![cell(0), cell(1)] }).collect(),
            delim: None,
            fmt: fmt.map(Arc::new),
            pad: [5.0, 5.0, 0.0, 0.0],
            min_h: vec![0.0, 0.0],
        }
    }

    fn pal() -> Palette {
        crate::theme::palette(false)
    }

    fn run(t: &TableGeom, guides: bool) -> Shapes {
        paint(t, &[(0.0, 20.0), (20.0, 40.0)], Vec2::ZERO, &pal(), 1.0, Rect::EVERYTHING, guides)
    }

    #[test]
    fn grid_table_has_lines_and_no_fill() {
        let s = run(&geom(None), false);
        assert!(s.fills.is_empty());
        // Rad 0: topp + 2 vänsterkanter + höger; rad 1: topp + 2 vänsterkanter + höger + 2 nederkanter.
        assert_eq!(s.lines.len(), 5 + 5 + 2);
    }

    #[test]
    fn borderless_table_draws_nothing_but_guides_on_demand() {
        let t = geom(Some(TableFmt::word_base()));
        assert!(run(&t, false).lines.is_empty());
        assert!(!run(&t, true).lines.is_empty());
    }

    #[test]
    fn fills_follow_cell_formats() {
        let mut f = TableFmt::word_base();
        f.cell_mut(0, 1).fill = Some([0x5B, 0x3A, 0x9E]);
        let s = run(&geom(Some(f)), false);
        assert_eq!(s.fills.len(), 1);
        match &s.fills[0] {
            Shape::Rect(r) => assert_eq!(r.rect, Rect::from_min_max(pos2(100.0, 0.0), pos2(200.0, 20.0))),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn row_min_height_stretches_the_fill() {
        let mut f = TableFmt::word_base();
        f.cell_mut(0, 0).fill = Some([1, 2, 3]);
        let mut t = geom(Some(f));
        t.min_h = vec![50.0, 0.0];
        let s = run(&t, false);
        match &s.fills[0] {
            Shape::Rect(r) => assert_eq!(r.rect.height(), 50.0),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn double_border_is_two_lines() {
        let mut f = TableFmt::word_base();
        f.borders[TOP] = Border { style: BStyle::Double, sz: 6, color: None };
        let s = run(&geom(Some(f)), false);
        assert_eq!(s.lines.len(), 4);
    }
}
