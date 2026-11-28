//! Text i tabellceller under redigering: cellernas geometri (kolumnbredder, radhöjder som växer med
//! texten som i `tables.rs`), träff på en cell, markörlägen för cellens text och skrivning av
//! cellens `a:txBody`. Samma textmodell som former (`TextModel`).
//!
//! Texten läggs ut som `tables.rs` ritar den: cellens `a:lstStyle`, tabellformatets textstil
//! (fetstil i rubrikraden, färger) och presentationens grundstilar. Tabeller som är vridna,
//! höger-till-vänster eller har lodrät text i cellen redigeras inte.

use super::edit::Txn;
use super::edit_ops;
use super::edit_text::{AutoFit, Carets, TextModel};
use super::edit_xml::XElem;
use super::inherit::{self, Sheets, TxKind};
use super::model::Deck;
use super::render::{emu, frame_of, Frame};
use super::richtext::{self, TextEnv};
use super::scene::Affine;
use super::text;
use super::xml::Node;

/// En cell: rad, kolumn (index bland radens `a:tc`).
pub type CellAddr = (usize, usize);

/// En ritad cell (ursprungscellen för sammanslagna).
struct CellGeo {
    addr: CellAddr,
    r0: usize,
    r1: usize,
    c0: usize,
    c1: usize,
}

/// Tabellens rutnät i punkter.
pub struct Geo {
    col_x: Vec<f32>,
    row_y: Vec<f32>,
    cells: Vec<CellGeo>,
}

/// Tabellen i toppnivåformen `index` (en `p:graphicFrame` med `a:tbl`) och dess placering.
fn table_of(deck: &Deck, slide: usize, index: usize) -> Option<(&Node, Frame)> {
    let gf = edit_ops::node(deck, slide, index)?;
    if gf.name != "p:graphicFrame" {
        return None;
    }
    let tbl = gf.path(&["a:graphic", "a:graphicData", "a:tbl"])?;
    let f = frame_of(gf.child("p:xfrm")?, &Affine::IDENTITY)?;
    // Vridna och speglade tabeller redigeras inte; höger-till-vänster inte heller.
    if f.rot != 0.0 || f.flip_h || f.flip_v || tbl.child("a:tblPr").and_then(|p| p.attr_bool("rtl")).unwrap_or(false) {
        return None;
    }
    Some((tbl, f))
}

/// Cellens textram: marginaler och ankare ur `a:tcPr`.
fn cell_props(tc: &Node) -> richtext::BodyProps {
    let tcpr = tc.child("a:tcPr");
    let mut bp = richtext::body_props(&[]);
    let m = |k: &str, d: i64| tcpr.and_then(|p| p.attr_i64(k)).map_or(emu(d), emu);
    bp.insets = (m("marL", 91440), m("marT", 45720), m("marR", 91440), m("marB", 45720));
    bp.anchor = match tcpr.and_then(|p| p.attr("anchor")) {
        Some("ctr") => text::Anchor::Middle,
        Some("b") => text::Anchor::Bottom,
        _ => text::Anchor::Top,
    };
    bp.anchor_ctr = tcpr.and_then(|p| p.attr_bool("anchorCtr")).unwrap_or(false);
    bp.wrap = true;
    bp.vert = tcpr.and_then(|p| p.attr("vert")).unwrap_or("horz").to_owned();
    bp
}

/// Rutnätet: kolumnbredder, radhöjder (minst `a:tr/@h`, raden växer med cellernas text) och cellerna.
fn geometry(deck: &Deck, slide: usize, tbl: &Node) -> Option<Geo> {
    let cols: Vec<f32> = tbl.child("a:tblGrid")?.children_named("a:gridCol").map(|c| emu(c.attr_i64("w").unwrap_or(0)).max(0.0)).collect();
    let rows: Vec<&Node> = tbl.children_named("a:tr").collect();
    if cols.is_empty() || rows.is_empty() {
        return None;
    }
    let (nrows, ncols) = (rows.len(), cols.len());
    let col_x: Vec<f32> = std::iter::once(0.0)
        .chain(cols.iter().scan(0.0, |a, w| {
            *a += w;
            Some(*a)
        }))
        .collect();
    let sheets = Sheets::for_slide(deck, slide);
    let base = inherit::list_styles(&sheets, &[], TxKind::Other);
    let mut heights: Vec<f32> = rows.iter().map(|r| emu(r.attr_i64("h").unwrap_or(0)).max(0.0)).collect();
    let mut grid = vec![vec![false; ncols]; nrows];
    let mut cells = Vec::new();
    let mut frames: Vec<Option<(text::Frame, bool)>> = Vec::new();
    for (ri, r) in rows.iter().enumerate() {
        for (ci, tc) in r.children_named("a:tc").enumerate().take(ncols) {
            if tc.attr_bool("hMerge").unwrap_or(false) || tc.attr_bool("vMerge").unwrap_or(false) || grid[ri][ci] {
                continue;
            }
            let sc = tc.attr_i64("gridSpan").unwrap_or(1).clamp(1, (ncols - ci) as i64) as usize;
            let sr = tc.attr_i64("rowSpan").unwrap_or(1).clamp(1, (nrows - ri) as i64) as usize;
            for row in grid.iter_mut().take(ri + sr).skip(ri) {
                for g in row.iter_mut().take(ci + sc).skip(ci) {
                    *g = true;
                }
            }
            let bp = cell_props(tc);
            let tx = super::tables::cell_style_list(deck, tbl, (ri, ri + sr, ci, ci + sc));
            let fr = tc.child("a:txBody").map(|body| {
                let mut lists: Vec<&Node> = body.child("a:lstStyle").into_iter().collect();
                lists.push(&tx);
                lists.extend(base.iter().copied());
                let env = TextEnv { sheets: &sheets, lists, font_ref: None, font_color: None, font_scale: 1.0, slide_no: deck.slide_number(slide) };
                (richtext::frame(&env, body, &bp), bp.vert != "horz")
            });
            frames.push(fr);
            cells.push(CellGeo { addr: (ri, ci), r0: ri, r1: ri + sr, c0: ci, c1: ci + sc });
        }
    }
    let mut order: Vec<usize> = (0..cells.len()).filter(|&i| frames[i].as_ref().is_some_and(|f| !f.1)).collect();
    order.sort_by_key(|&i| cells[i].r1 - cells[i].r0);
    for i in order {
        let c = &cells[i];
        let Some((fr, _)) = &frames[i] else { continue };
        let w = col_x[c.c1] - col_x[c.c0];
        let laid = text::layout(fr, w, 1.0e6, &deck.fonts);
        let need = laid.height + fr.insets.1 + fr.insets.3;
        let have: f32 = heights[c.r0..c.r1].iter().sum();
        if need > have {
            heights[c.r1 - 1] += need - have;
        }
    }
    let row_y: Vec<f32> = std::iter::once(0.0)
        .chain(heights.iter().scan(0.0, |a, h| {
            *a += h;
            Some(*a)
        }))
        .collect();
    Some(Geo { col_x, row_y, cells })
}

impl Geo {
    fn rect(&self, c: &CellGeo) -> (f32, f32, f32, f32) {
        (self.col_x[c.c0], self.row_y[c.r0], self.col_x[c.c1], self.row_y[c.r1])
    }
}

/// Cellen under punkten (bildens punkter) i tabellen i formen `index`.
pub fn cell_at(deck: &Deck, slide: usize, index: usize, x: f32, y: f32) -> Option<CellAddr> {
    let (tbl, f) = table_of(deck, slide, index)?;
    let g = geometry(deck, slide, tbl)?;
    let (lx, ly) = (x - f.x, y - f.y);
    g.cells.iter().find(|c| {
        let (x0, y0, x1, y1) = g.rect(c);
        lx >= x0 && lx < x1 && ly >= y0 && ly < y1
    }).map(|c| c.addr)
}

/// Är formen en tabell som går att redigera?
pub fn is_editable_table(deck: &Deck, slide: usize, index: usize) -> bool {
    table_of(deck, slide, index).is_some()
}

/// Nästa (eller föregående) cell i läsordning, utan sammanslagna celler; `None` efter den sista.
pub fn next_cell(deck: &Deck, slide: usize, index: usize, from: CellAddr, back: bool) -> Option<CellAddr> {
    let (tbl, _) = table_of(deck, slide, index)?;
    let g = geometry(deck, slide, tbl)?;
    let mut order: Vec<CellAddr> = g.cells.iter().map(|c| c.addr).collect();
    order.sort();
    let at = order.iter().position(|&a| a == from)?;
    if back { at.checked_sub(1).map(|i| order[i]) } else { order.get(at + 1).copied() }
}

/// Är `addr` tabellens sista cell i läsordning?
pub fn is_last_cell(deck: &Deck, slide: usize, index: usize, addr: CellAddr) -> bool {
    let Some((tbl, _)) = table_of(deck, slide, index) else { return false };
    let Some(g) = geometry(deck, slide, tbl) else { return false };
    g.cells.iter().map(|c| c.addr).max() == Some(addr)
}

/// Cellens text som modell (`a:txBody` i `a:tc`) ur XML:en i `doc`.
pub fn cell_model(doc: &super::edit_xml::XDoc, index: usize, addr: CellAddr, a: &str) -> Option<TextModel> {
    let tc = tc_of(edit_ops::raw_shape(doc, index)?, addr)?;
    Some(TextModel::from_body(tc.child("a:txBody"), &doc.root_scope(), a))
}

fn tc_of(gf: &XElem, (ri, ci): CellAddr) -> Option<&XElem> {
    let tbl = gf.path(&["a:graphic", "a:graphicData", "a:tbl"])?;
    tbl.elems().filter(|e| e.name == "a:tr").nth(ri)?.elems().filter(|e| e.name == "a:tc").nth(ci)
}

fn tc_of_mut(gf: &mut XElem, (ri, ci): CellAddr) -> Option<&mut XElem> {
    let tbl = gf.path_mut(&["a:graphic", "a:graphicData", "a:tbl"])?;
    tbl.elems_mut().filter(|e| e.name == "a:tr").nth(ri)?.elems_mut().filter(|e| e.name == "a:tc").nth(ci)
}

impl TextModel {
    /// Markörlägen för modellen som cellens text i tabellen i formen `index`.
    pub fn cell_carets(&self, deck: &Deck, slide: usize, index: usize, addr: CellAddr) -> Option<Carets> {
        let (tbl, f) = table_of(deck, slide, index)?;
        let g = geometry(deck, slide, tbl)?;
        let cell = g.cells.iter().find(|c| c.addr == addr)?;
        let tc = tbl.children_named("a:tr").nth(addr.0)?.children_named("a:tc").nth(addr.1)?;
        let bp = cell_props(tc);
        if bp.vert != "horz" {
            return None;
        }
        let sheets = Sheets::for_slide(deck, slide);
        let base = inherit::list_styles(&sheets, &[], TxKind::Other);
        let tx = super::tables::cell_style_list(deck, tbl, (cell.r0, cell.r1, cell.c0, cell.c1));
        let mut lists: Vec<&Node> = tc.child("a:txBody").and_then(|b| b.child("a:lstStyle")).into_iter().collect();
        lists.push(&tx);
        lists.extend(base.iter().copied());
        let env = TextEnv { sheets: &sheets, lists, font_ref: None, font_color: None, font_scale: 1.0, slide_no: deck.slide_number(slide) };
        let (x0, y0, x1, y1) = g.rect(cell);
        let cf = Frame { t: Affine::translate(x0, y0).then(&f.t), w: x1 - x0, h: y1 - y0, rot: 0.0, flip_h: false, flip_v: false, parent: f.t, x: x0, y: y0 };
        self.carets_core(deck, &env, &bp, (0.0, 0.0, cf.w, cf.h), &cf, AutoFit::None)
    }
}

/// Skriver modellen i cellens `a:txBody` (skapas före `a:tcPr` om den saknas).
pub fn set_cell_text(txn: &mut Txn, slide: usize, index: usize, id: u32, addr: CellAddr, model: &TextModel) -> Result<(), String> {
    let part = edit_ops::slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    if doc.prefix_for(super::edit::NS_A).is_none() && !model.prefix().is_empty() {
        doc.root.set_attr(&format!("xmlns:{}", model.prefix()), super::edit::NS_A);
    }
    edit_ops::with_shape(txn, slide, index, id, &mut |e, _doc| {
        let tc = tc_of_mut(e, addr).ok_or("cellen finns inte längre")?;
        model.write_cell_body(tc);
        Ok(())
    })
}


/// Lägger en ny rad sist i tabellen: en kopia av sista raden med tom text och samma cellegenskaper
/// (`a:tcPr`, styckeformat i `a:endParaRPr`). Formens höjd växer med radens. Ger den nya radens index.
pub fn add_row(txn: &mut Txn, slide: usize, index: usize, id: u32) -> Result<usize, String> {
    let mut new_row = 0;
    edit_ops::with_shape(txn, slide, index, id, &mut |e, doc| {
        let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
        let q = |l: &str| if a.is_empty() { l.to_owned() } else { format!("{a}:{l}") };
        let tbl = e.path_mut(&["a:graphic", "a:graphicData", "a:tbl"]).ok_or("tabellen finns inte längre")?;
        let last = tbl.elems().filter(|r| r.name == "a:tr").last().ok_or("tabellen saknar rader")?.clone();
        let h = last.attr_i64("h").unwrap_or(370_840);
        let mut row = last;
        // Ett unikt rad-id (a16:rowId) får inte kopieras; sammanslagning nedåt gäller inte den nya raden.
        row.remove_children("a:extLst");
        for tc in row.elems_mut().filter(|c| c.name == "a:tc") {
            tc.remove_attr("rowSpan");
            tc.remove_attr("vMerge");
            let end = tc.child("a:txBody").and_then(|b| b.elems().filter(|p| p.name == "a:p").last()).and_then(|p| p.child("a:endParaRPr").cloned());
            let mut para = doc.fragment(&format!("<{}/>", q("p"))).into_iter().next().ok_or("p")?;
            if let Some(mut en) = end {
                en.remove_attr("dirty");
                para.children.push(super::edit_xml::XNode::Elem(en));
            }
            if let Some(body) = tc.child_mut("a:txBody") {
                body.remove_children("a:p");
                body.children.push(super::edit_xml::XNode::Elem(para));
            }
        }
        let at = tbl.children.iter().rposition(|c| matches!(c, super::edit_xml::XNode::Elem(x) if x.name == "a:tr")).map_or(tbl.children.len(), |i| i + 1);
        new_row = tbl.elems().filter(|r| r.name == "a:tr").count();
        tbl.children.insert(at, super::edit_xml::XNode::Elem(row));
        // Formens höjd.
        if let Some(ext) = e.child_mut("p:xfrm").and_then(|x| x.child_mut("a:ext")) {
            let cy = ext.attr_i64("cy").unwrap_or(0) + h;
            ext.set_attr("cy", &cy.to_string());
        }
        Ok(())
    })?;
    Ok(new_row)
}
