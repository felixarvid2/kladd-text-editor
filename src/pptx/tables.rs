//! Tabeller (`a:tbl`): grundstöd i den här omgången. Rutnät ur `a:tblGrid`, radhöjder som växer
//! med texten, sammanslagna celler, cellernas egna fyllningar och kantlinjer (`a:tcPr`) och en
//! enkel tolkning av tabellformatet (`tableStyles.xml`: `wholeTbl`, `firstRow`, `band1H`,
//! `firstCol`, `lastRow` med fyllning, text och kanter).
//!
//! Hook för nästa omgång: inbyggda format när GUID:en saknas i filen, alla delar av formatet
//! (`band2H`, `band1V`, `seCell` …), diagonala linjer, cellernas text i lodrät riktning.

use super::color::{self, Rgba};
use super::inherit::{self, Owner, TxKind};
use super::render::{emu, Builder, Frame};
use super::richtext::{self, TextEnv};
use super::scene::{Affine, Cap, Join, Paint, Path, Stroke};
use super::text;
use super::xml::Node;

/// Delarna av tabellformatet som gäller för en cell, i stigande prioritet.
fn style_parts<'a>(style: Option<&'a Node>, tbl_pr: Option<&Node>, row: usize, col: usize, rows: usize, cols: usize) -> Vec<&'a Node> {
    let Some(style) = style else { return Vec::new() };
    let on = |a: &str| tbl_pr.and_then(|p| p.attr_bool(a)).unwrap_or(false);
    let mut names = vec!["a:wholeTbl"];
    let first_row = on("firstRow");
    let first_col = on("firstCol");
    if on("bandRow") {
        let r = if first_row { row as i64 - 1 } else { row as i64 };
        if r >= 0 {
            names.push(if r % 2 == 0 { "a:band1H" } else { "a:band2H" });
        }
    }
    if on("bandCol") {
        let c = if first_col { col as i64 - 1 } else { col as i64 };
        if c >= 0 {
            names.push(if c % 2 == 0 { "a:band1V" } else { "a:band2V" });
        }
    }
    if on("lastCol") && col + 1 == cols {
        names.push("a:lastCol");
    }
    if first_col && col == 0 {
        names.push("a:firstCol");
    }
    if on("lastRow") && row + 1 == rows {
        names.push("a:lastRow");
    }
    if first_row && row == 0 {
        names.push("a:firstRow");
    }
    names.iter().filter_map(|n| style.child(n)).collect()
}

/// Tabellformatet med id ur `tableStyles.xml`.
fn table_style<'a>(b: &Builder<'a>, id: &str) -> Option<&'a Node> {
    let ts = b.sheets.deck.table_styles.as_ref()?;
    ts.children.iter().find(|s| s.attr("styleId").is_some_and(|v| v.eq_ignore_ascii_case(id)))
}

/// En färg ur en formatdel (`a:fillRef`/`a:fill`/`a:fontRef`).
fn part_color(b: &Builder<'_>, n: &Node) -> Option<Rgba> {
    color::first_color(n).and_then(|c| b.sheets.color(c, None))
}

pub fn draw_table<'a>(b: &mut Builder<'a>, tbl: &'a Node, f: &Frame, owner: Owner) {
    let cols: Vec<f32> = tbl.child("a:tblGrid").map(|g| g.children_named("a:gridCol").map(|c| emu(c.attr_i64("w").unwrap_or(0))).collect()).unwrap_or_default();
    if cols.is_empty() {
        return;
    }
    let rows: Vec<&Node> = tbl.children_named("a:tr").collect();
    let tbl_pr = tbl.child("a:tblPr");
    let style = tbl_pr.and_then(|p| p.child("a:tableStyleId")).and_then(|s| table_style(b, s.text.trim()));
    let nrows = rows.len();
    let rtl_table = tbl_pr.and_then(|p| p.attr_bool("rtl")).unwrap_or(false);
    let ncols = cols.len();
    let col_x: Vec<f32> = std::iter::once(0.0).chain(cols.iter().scan(0.0, |acc, w| {
        *acc += w;
        Some(*acc)
    })).collect();

    // Textramar per cell och radhöjder som rymmer texten.
    let lists = inherit::list_styles(&b.sheets, &[], TxKind::Other);
    let mut heights: Vec<f32> = rows.iter().map(|r| emu(r.attr_i64("h").unwrap_or(0))).collect();
    struct CellLay<'n> {
        tc: &'n Node,
        row: usize,
        col: usize,
        span_c: usize,
        span_r: usize,
        frame: text::Frame,
    }
    let mut cells: Vec<CellLay> = Vec::new();
    for (ri, r) in rows.iter().enumerate() {
        for (ci, tc) in r.children_named("a:tc").enumerate() {
            if ci >= ncols {
                break;
            }
            if tc.attr_bool("hMerge").unwrap_or(false) || tc.attr_bool("vMerge").unwrap_or(false) {
                continue;
            }
            let span_c = tc.attr_i64("gridSpan").unwrap_or(1).clamp(1, (ncols - ci) as i64) as usize;
            let span_r = tc.attr_i64("rowSpan").unwrap_or(1).clamp(1, (nrows - ri) as i64) as usize;
            let tcpr = tc.child("a:tcPr");
            let parts = style_parts(style, tbl_pr, ri, ci, nrows, ncols);
            // Textfärg och fetstil ur formatets tcTxStyle.
            let mut font_color = None;
            let mut bold = None;
            for p in &parts {
                if let Some(tx) = p.child("a:tcTxStyle") {
                    if let Some(c) = part_color(b, tx).or_else(|| tx.child("a:fontRef").and_then(|r| part_color(b, r))) {
                        font_color = Some(c);
                    }
                    if let Some(v) = tx.attr("b") {
                        bold = Some(v == "on");
                    }
                }
            }
            let m = |k: &str, d: i64| tcpr.and_then(|p| p.attr_i64(k)).map_or(emu(d), emu);
            let bp = richtext::BodyProps {
                insets: (m("marL", 91440), m("marT", 45720), m("marR", 91440), m("marB", 45720)),
                anchor: match tcpr.and_then(|p| p.attr("anchor")) {
                    Some("ctr") => text::Anchor::Middle,
                    Some("b") => text::Anchor::Bottom,
                    _ => text::Anchor::Top,
                },
                anchor_ctr: false,
                wrap: true,
                vert: "horz".into(),
                rot: 0.0,
                upright: false,
                font_scale: 1.0,
                ln_spc_reduction: 0.0,
                columns: (1, 0.0),
                sp_auto_fit: false,
            };
            let env = TextEnv { sheets: &b.sheets, lists: lists.clone(), font_ref: None, font_color, font_scale: 1.0, slide_no: b.slide_no };
            let Some(body) = tc.child("a:txBody") else { continue };
            let mut frame = richtext::frame(&env, body, &bp);
            if let Some(bo) = bold {
                for p in &mut frame.paras {
                    for r in &mut p.runs {
                        r.style.bold = r.style.bold || bo;
                    }
                }
            }
            cells.push(CellLay { tc, row: ri, col: ci, span_c, span_r, frame });
        }
    }
    for c in &cells {
        if c.span_r != 1 {
            continue;
        }
        let w = col_x[c.col + c.span_c] - col_x[c.col];
        let laid = text::layout(&c.frame, w, 1.0e6, &b.sheets.deck.fonts);
        let need = laid.height + c.frame.insets.1 + c.frame.insets.3;
        // Raden växer med texten (den sparade höjden är ofta lägre än den PowerPoint visar).
        if need > heights[c.row] {
            heights[c.row] = need;
        }
    }
    let row_y: Vec<f32> = std::iter::once(0.0).chain(heights.iter().scan(0.0, |acc, h| {
        *acc += h;
        Some(*acc)
    })).collect();

    // Fyllningar, text, sedan kanter.
    let mut borders: Vec<(Path, Stroke)> = Vec::new();
    for c in &cells {
        let (mut x0, mut x1) = (col_x[c.col], col_x[c.col + c.span_c]);
        // Tabell från höger till vänster: första kolumnen längst till höger.
        if rtl_table {
            let total = col_x[ncols];
            (x0, x1) = (total - x1, total - x0);
        }
        let (y0, y1) = (row_y[c.row], row_y[(c.row + c.span_r).min(nrows)]);
        let cell = Frame { t: Affine::translate(x0, y0).then(&f.t), w: x1 - x0, h: y1 - y0, x: f.x + x0, y: f.y + y0, ..*f };
        let rect = Path::rect(0.0, 0.0, cell.w, cell.h).transformed(&cell.t);
        let tcpr = c.tc.child("a:tcPr");
        let parts = style_parts(style, tbl_pr, c.row, c.col, nrows, ncols);
        // Fyllning: cellens egen, annars formatets.
        let own_fill = tcpr.and_then(|p| p.children.iter().find(|n| inherit::FILLS.contains(&n.name.as_str())));
        let paint = match own_fill {
            Some(n) => b.paint(n, None, &cell, owner),
            None => parts.iter().rev().find_map(|p| {
                let tcs = p.child("a:tcStyle")?;
                if let Some(fill) = tcs.child("a:fill") {
                    let n = fill.children.iter().find(|n| inherit::FILLS.contains(&n.name.as_str()))?;
                    return Some(b.paint(n, None, &cell, owner));
                }
                tcs.child("a:fillRef").map(|r| part_color(b, r).map(Paint::Solid))
            }).flatten(),
        };
        if let Some(p) = paint {
            b.scene.fill(rect.clone(), p);
        }
        // Kanter: cellens egna `a:lnL`… går före formatets.
        let edges = [("a:lnL", "a:left", (0.0, 0.0, 0.0, 1.0)), ("a:lnR", "a:right", (1.0, 0.0, 1.0, 1.0)), ("a:lnT", "a:top", (0.0, 0.0, 1.0, 0.0)), ("a:lnB", "a:bottom", (0.0, 1.0, 1.0, 1.0))];
        for (own, sty, (ax, ay, bx, by)) in edges {
            let inside = match sty {
                "a:left" => c.col > 0,
                "a:right" => c.col + c.span_c < ncols,
                "a:top" => c.row > 0,
                _ => c.row + c.span_r < nrows,
            };
            let ln = tcpr.and_then(|p| p.child(own)).or_else(|| {
                parts.iter().rev().find_map(|p| {
                    let bdr = p.path(&["a:tcStyle", "a:tcBdr"])?;
                    let name = if inside { if sty == "a:left" || sty == "a:right" { "a:insideV" } else { "a:insideH" } } else { sty };
                    bdr.child(name).and_then(|e| e.child("a:ln"))
                })
            });
            let Some(ln) = ln else { continue };
            let Some(fill) = ln.children.iter().find(|n| inherit::FILLS.contains(&n.name.as_str())) else { continue };
            let Some(paint) = b.paint(fill, None, &cell, owner) else { continue };
            let width = ln.attr_i64("w").map_or(1.0, emu);
            let mut p = Path::default();
            p.move_to(ax * cell.w, ay * cell.h);
            p.line_to(bx * cell.w, by * cell.h);
            borders.push((p.transformed(&cell.t), Stroke { paint, width, dash: Vec::new(), cap: Cap::Butt, join: Join::Miter }));
        }
        let bp = richtext::BodyProps {
            insets: c.frame.insets,
            anchor: c.frame.anchor,
            anchor_ctr: false,
            wrap: true,
            vert: tcpr.and_then(|p| p.attr("vert")).unwrap_or("horz").to_owned(),
            rot: 0.0,
            upright: false,
            font_scale: 1.0,
            ln_spc_reduction: 0.0,
            columns: (1, 0.0),
            sp_auto_fit: false,
        };
        b.place_text(&c.frame, &bp, &cell, (0.0, 0.0, cell.w, cell.h));
    }
    for (p, s) in borders {
        b.scene.stroke(p, s);
    }
}
