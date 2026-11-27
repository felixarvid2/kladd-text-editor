//! Tabeller (`a:tbl`): rutnät ur `a:tblGrid`, radhöjder (minsta höjd; raden växer med texten som i
//! PowerPoint), sammanslagna celler (`gridSpan`/`rowSpan`/`hMerge`/`vMerge`), cellmarginaler,
//! `anchor`, `vert`, cellfyllning, kantlinjer per sida inklusive diagonaler, och tabellformatet:
//! filens `ppt/tableStyles.xml` först, sedan PowerPoints inbyggda format (`table_builtin.rs`).
//!
//! Formatdelarna gäller i stigande prioritet `wholeTbl`, `band1H`/`band2H`, `band1V`/`band2V`,
//! `lastCol`, `firstCol`, `lastRow`, `firstRow`, `seCell`, `swCell`, `neCell`, `nwCell` (den ordning
//! PowerPoint skriver dem). Varje del gäller ett område (hela tabellen, en rad eller en kolumn): en
//! cells kant på områdets yttersida tar delens `left`/`right`/`top`/`bottom`, en kant inuti
//! området `insideV`/`insideH`. Cellens egna `a:lnL`… (även `a:noFill`) går före formatet. Varje
//! linje i rutnätet ritas en gång: av de två cellerna på var sida vinner den egna kanten, annars
//! kanten ur delen med högst prioritet.

use super::color::{self, Rgba};
use super::inherit::{self, Owner, TxKind};
use super::render::{emu, Builder, Frame};
use super::richtext::{self, TextEnv};
use super::scene::{Affine, Cap, Join, Paint, Path, Stroke};
use super::text;
use super::xml::Node;

/// Formatdelarna i stigande prioritet.
const PARTS: &[&str] = &["a:wholeTbl", "a:band1H", "a:band2H", "a:band1V", "a:band2V", "a:lastCol", "a:firstCol", "a:lastRow", "a:firstRow", "a:seCell", "a:swCell", "a:neCell", "a:nwCell"];

/// `a:tblPr`-flaggorna som väljer formatdelar.
#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    first_row: bool,
    last_row: bool,
    first_col: bool,
    last_col: bool,
    band_row: bool,
    band_col: bool,
}

impl Flags {
    fn of(tbl_pr: Option<&Node>) -> Flags {
        let on = |a: &str| tbl_pr.and_then(|p| p.attr_bool(a)).unwrap_or(false);
        Flags { first_row: on("firstRow"), last_row: on("lastRow"), first_col: on("firstCol"), last_col: on("lastCol"), band_row: on("bandRow"), band_col: on("bandCol") }
    }
}

/// Ett område i rutnätet (rader `r0..r1`, kolumner `c0..c1`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    r0: usize,
    r1: usize,
    c0: usize,
    c1: usize,
}

/// En formatdel som gäller en cell: noden, dess område och prioritet (index i `PARTS`).
#[derive(Clone, Copy)]
struct Part<'a> {
    node: &'a Node,
    area: Area,
    rank: usize,
}

/// Delarna av formatet som gäller cellen `cell` (sammanslagen: hela ytan), i stigande prioritet.
fn style_parts<'a>(style: Option<&'a Node>, fl: Flags, cell: Area, rows: usize, cols: usize) -> Vec<Part<'a>> {
    let Some(style) = style else { return Vec::new() };
    let whole = Area { r0: 0, r1: rows, c0: 0, c1: cols };
    let row = Area { r0: cell.r0, r1: cell.r0 + 1, c0: 0, c1: cols };
    let col = Area { r0: 0, r1: rows, c0: cell.c0, c1: cell.c0 + 1 };
    let mut out: Vec<(&str, Area)> = vec![("a:wholeTbl", whole)];
    let in_first_row = fl.first_row && cell.r0 == 0;
    let in_last_row = fl.last_row && cell.r1 == rows;
    let in_first_col = fl.first_col && cell.c0 == 0;
    let in_last_col = fl.last_col && cell.c1 == cols;
    if fl.band_row && !in_first_row && !in_last_row {
        let r = cell.r0 - usize::from(fl.first_row);
        out.push((if r % 2 == 0 { "a:band1H" } else { "a:band2H" }, row));
    }
    if fl.band_col && !in_first_col && !in_last_col {
        let c = cell.c0 - usize::from(fl.first_col);
        out.push((if c % 2 == 0 { "a:band1V" } else { "a:band2V" }, col));
    }
    if in_last_col {
        out.push(("a:lastCol", Area { c0: cols - 1, c1: cols, ..col }));
    }
    if in_first_col {
        out.push(("a:firstCol", col));
    }
    if in_last_row {
        out.push(("a:lastRow", Area { r0: rows - 1, r1: rows, ..row }));
    }
    if in_first_row {
        out.push(("a:firstRow", row));
    }
    let corner = Area { r0: cell.r0, r1: cell.r1, c0: cell.c0, c1: cell.c1 };
    if in_last_row && in_last_col {
        out.push(("a:seCell", corner));
    }
    if in_last_row && in_first_col {
        out.push(("a:swCell", corner));
    }
    if in_first_row && in_last_col {
        out.push(("a:neCell", corner));
    }
    if in_first_row && in_first_col {
        out.push(("a:nwCell", corner));
    }
    let mut parts: Vec<Part> = out
        .into_iter()
        .filter_map(|(name, area)| Some(Part { node: style.child(name)?, area, rank: PARTS.iter().position(|p| *p == name).unwrap_or(0) }))
        .collect();
    parts.sort_by_key(|p| p.rank);
    parts
}

/// Tabellformatet med id: filens `tableStyles.xml` först, sedan PowerPoints inbyggda.
fn table_style<'a>(b: &Builder<'a>, id: &str) -> Option<&'a Node> {
    let id = id.trim();
    let own = b.sheets.deck.table_styles.as_ref().and_then(|ts| ts.children.iter().find(|s| s.attr("styleId").is_some_and(|v| v.trim().eq_ignore_ascii_case(id))));
    own.or_else(|| super::table_builtin::style(id))
}

/// En linje att rita: `stroke` är `None` för en uttryckligen osynlig kant.
#[derive(Clone, Debug)]
struct Line {
    stroke: Option<Stroke>,
    /// Dubbel linje (`cmpd="dbl"`, och ungefär `thickThin`/`thinThick`/`tri`).
    double: bool,
    /// Antal gånger linjen ritas: 2 när båda cellerna vid linjen har en egen tunn kant (under 1 pt).
    /// PowerPoint ritar varje cells kanter, och en delad tunn linje blir då mörkare; i PowerPoints
    /// PDF i 72 dpi är den en hel svart bildpunkt. Tjockare linjer ritas en gång.
    times: u8,
}

impl Line {
    fn width(&self) -> f32 {
        self.stroke.as_ref().map_or(0.0, |s| s.width)
    }

    /// Samma utseende (för att slå ihop angränsande bitar till en linje).
    fn same(&self, o: &Line) -> bool {
        match (&self.stroke, &o.stroke) {
            (Some(a), Some(b)) => self.double == o.double && self.times == o.times && a.width == b.width && a.dash == b.dash && a.paint.average() == b.paint.average(),
            _ => false,
        }
    }
}

/// Streckmönster i linjebredder (`a:prstDash`), som för formernas linjer.
fn dash_pattern(name: &str) -> &'static [f32] {
    match name {
        "dot" => &[1.0, 3.0],
        "dash" => &[4.0, 3.0],
        "lgDash" => &[8.0, 3.0],
        "dashDot" => &[4.0, 3.0, 1.0, 3.0],
        "lgDashDot" => &[8.0, 3.0, 1.0, 3.0],
        "lgDashDotDot" => &[8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
        "sysDash" => &[3.0, 1.0],
        "sysDot" => &[1.0, 1.0],
        "sysDashDot" => &[3.0, 1.0, 1.0, 1.0],
        "sysDashDotDot" => &[3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        _ => &[],
    }
}

/// `a:ln` (eller cellens `a:lnL` …) → linje. `None` om linjen inte säger något om fyllningen.
fn line_of(b: &Builder<'_>, ln: &Node, ph: Option<Rgba>, f: &Frame, owner: Owner) -> Option<Line> {
    let fill = ln.children.iter().find(|n| inherit::FILLS.contains(&n.name.as_str()))?;
    let width = ln.attr_i64("w").map_or(0.75, emu).max(0.0);
    let stroke = b.paint(fill, ph, f, owner).filter(|_| width > 0.0).map(|paint| {
        let pattern = ln.child("a:prstDash").and_then(|d| d.attr("val")).map_or(&[][..], dash_pattern);
        // Punkter och streck under 1 pt syns inte som mönster (PowerPoints PDF i 72 dpi visar en hel linje).
        let pattern = if pattern.iter().copied().fold(f32::INFINITY, f32::min) * width < 1.0 { &[][..] } else { pattern };
        let dash = pattern.iter().map(|v| v * width).collect();
        Stroke { paint, width, dash, cap: Cap::Butt, join: Join::Miter }
    });
    Some(Line { stroke, double: matches!(ln.attr("cmpd"), Some("dbl" | "thickThin" | "thinThick" | "tri")), times: 1 })
}

/// En kant i formatet (`a:left` …): `a:ln` eller `a:lnRef` (temats linjestil med färgen).
fn style_edge(b: &Builder<'_>, edge: &Node, f: &Frame, owner: Owner) -> Option<Line> {
    if let Some(ln) = edge.child("a:ln") {
        return line_of(b, ln, None, f, owner).or(Some(Line { stroke: None, double: false, times: 1 }));
    }
    let r = edge.child("a:lnRef")?;
    let ph = color::first_color(r).and_then(|c| b.sheets.color(c, None));
    let idx = r.attr_i64("idx").unwrap_or(0);
    let ln = (idx >= 1).then(|| b.sheets.theme.line_styles.get((idx - 1) as usize)).flatten()?;
    line_of(b, ln, ph, f, owner)
}

/// Cellens fyra sidor.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// En sida av en cell: cellens egen linje och formatets (med delens prioritet).
#[derive(Clone, Default)]
struct Edge {
    own: Option<Line>,
    style: Option<(usize, Line)>,
}

fn resolve_side(b: &Builder<'_>, tcpr: Option<&Node>, parts: &[Part<'_>], cell: Area, side: Side, f: &Frame, owner: Owner) -> Edge {
    let own_name = match side {
        Side::Left => "a:lnL",
        Side::Right => "a:lnR",
        Side::Top => "a:lnT",
        Side::Bottom => "a:lnB",
    };
    let own = tcpr.and_then(|p| p.child(own_name)).and_then(|ln| line_of(b, ln, None, f, owner));
    let style = parts.iter().rev().find_map(|p| {
        let bdr = p.node.path(&["a:tcStyle", "a:tcBdr"])?;
        let a = p.area;
        let name = match side {
            Side::Left if cell.c0 == a.c0 => "a:left",
            Side::Right if cell.c1 == a.c1 => "a:right",
            Side::Top if cell.r0 == a.r0 => "a:top",
            Side::Bottom if cell.r1 == a.r1 => "a:bottom",
            Side::Left | Side::Right => "a:insideV",
            Side::Top | Side::Bottom => "a:insideH",
        };
        let e = bdr.child(name)?;
        style_edge(b, e, f, owner).map(|l| (p.rank, l))
    });
    Edge { own, style }
}

/// Linjen mellan två celler (eller längs tabellens kant): egna kanter före formatet, sedan
/// delen med högst prioritet; vid lika den synliga/bredare.
fn pick(a: Option<&Edge>, b: Option<&Edge>) -> Option<Line> {
    let wider = |x: Line, y: Line| if y.width() > x.width() { y } else { x };
    let owns: Vec<Line> = [a, b].iter().flatten().filter_map(|e| e.own.clone()).collect();
    if !owns.is_empty() {
        let both = owns.len() == 2 && owns.iter().all(|l| l.stroke.as_ref().is_some_and(|s| s.width < 1.0));
        return owns.into_iter().reduce(wider).map(|l| Line { times: if both { 2 } else { 1 }, ..l });
    }
    let styles: Vec<(usize, Line)> = [a, b].iter().flatten().filter_map(|e| e.style.clone()).collect();
    let top = styles.iter().map(|(r, _)| *r).max()?;
    styles.into_iter().filter(|(r, _)| *r == top).map(|(_, l)| l).reduce(wider)
}

/// Textegenskaperna ur formatets `a:tcTxStyle` (senare delar går före).
#[derive(Default)]
struct TxStyle<'a> {
    bold: Option<bool>,
    italic: Option<bool>,
    font: Option<&'static str>,
    color: Option<&'a Node>,
}

fn text_style<'a>(parts: &[Part<'a>]) -> TxStyle<'a> {
    let mut t = TxStyle::default();
    for p in parts {
        let Some(tx) = p.node.child("a:tcTxStyle") else { continue };
        match tx.attr("b") {
            Some("on") => t.bold = Some(true),
            Some("off") => t.bold = Some(false),
            _ => {}
        }
        match tx.attr("i") {
            Some("on") => t.italic = Some(true),
            Some("off") => t.italic = Some(false),
            _ => {}
        }
        let font_ref = tx.child("a:fontRef");
        match font_ref.and_then(|r| r.attr("idx")) {
            Some("major") => t.font = Some("+mj-lt"),
            Some("minor") => t.font = Some("+mn-lt"),
            _ => {}
        }
        if let Some(c) = color::first_color(tx).or_else(|| font_ref.and_then(color::first_color)) {
            t.color = Some(c);
        }
    }
    t
}

/// Formatets text som nivålista (`a:lvl1pPr` … `a:lvl9pPr` med `a:defRPr`), så att den ligger mellan
/// cellens egen `a:lstStyle` och mallens stilar i arvet.
fn text_list(t: &TxStyle<'_>) -> Node {
    let mut def = Node { name: "a:defRPr".into(), ..Default::default() };
    if let Some(v) = t.bold {
        def.attrs.push(("b".into(), if v { "1" } else { "0" }.into()));
    }
    if let Some(v) = t.italic {
        def.attrs.push(("i".into(), if v { "1" } else { "0" }.into()));
    }
    if let Some(c) = t.color {
        def.children.push(Node { name: "a:solidFill".into(), children: vec![c.clone()], ..Default::default() });
    }
    if let Some(face) = t.font {
        def.children.push(Node { name: "a:latin".into(), attrs: vec![("typeface".into(), face.into())], ..Default::default() });
    }
    Node {
        name: "a:lstStyle".into(),
        children: (1..=9).map(|l| Node { name: format!("a:lvl{l}pPr"), children: vec![def.clone()], ..Default::default() }).collect(),
        ..Default::default()
    }
}

/// En cell som ritas (sammanslagna celler: ursprungscellen med hela ytan).
struct Cell<'n> {
    tc: &'n Node,
    area: Area,
    frame: Option<text::Frame>,
    bp: richtext::BodyProps,
}

/// Fyllningen ur formatets `a:tcStyle` eller `a:tblBg`: `a:fill` eller `a:fillRef`. `Some(None)` = uttryckligen ingen.
fn style_fill(b: &Builder<'_>, n: &Node, f: &Frame, owner: Owner) -> Option<Option<Paint>> {
    if let Some(fill) = n.child("a:fill") {
        let fill = fill.children.iter().find(|n| inherit::FILLS.contains(&n.name.as_str()))?;
        return Some(b.paint(fill, None, f, owner));
    }
    let r = n.child("a:fillRef")?;
    let ph = color::first_color(r).and_then(|c| b.sheets.color(c, None));
    let theme = inherit::theme_fill(&b.sheets, r.attr_i64("idx").unwrap_or(0), ph);
    Some(match theme {
        Some(sr) => b.paint(sr.node, sr.ph, f, owner),
        None => ph.map(Paint::Solid),
    })
}

pub fn draw_table<'a>(b: &mut Builder<'a>, tbl: &'a Node, f: &Frame, owner: Owner) {
    let cols: Vec<f32> = tbl.child("a:tblGrid").map(|g| g.children_named("a:gridCol").map(|c| emu(c.attr_i64("w").unwrap_or(0)).max(0.0)).collect()).unwrap_or_default();
    if cols.is_empty() {
        return;
    }
    let rows: Vec<&Node> = tbl.children_named("a:tr").collect();
    if rows.is_empty() {
        return;
    }
    let tbl_pr = tbl.child("a:tblPr");
    let fl = Flags::of(tbl_pr);
    let style = tbl_pr.and_then(|p| p.child("a:tableStyleId")).and_then(|s| table_style(b, &s.text));
    let nrows = rows.len();
    let ncols = cols.len();
    let rtl_table = tbl_pr.and_then(|p| p.attr_bool("rtl")).unwrap_or(false);
    let col_x: Vec<f32> = std::iter::once(0.0)
        .chain(cols.iter().scan(0.0, |acc, w| {
            *acc += w;
            Some(*acc)
        }))
        .collect();
    let total_w = col_x[ncols];

    // Cellerna, deras textramar och vilken cell som täcker varje ruta i rutnätet.
    let base_lists = inherit::list_styles(&b.sheets, &[], TxKind::Other);
    let mut heights: Vec<f32> = rows.iter().map(|r| emu(r.attr_i64("h").unwrap_or(0)).max(0.0)).collect();
    let mut cells: Vec<Cell> = Vec::new();
    let mut grid: Vec<Vec<Option<usize>>> = vec![vec![None; ncols]; nrows];
    for (ri, r) in rows.iter().enumerate() {
        for (ci, tc) in r.children_named("a:tc").enumerate() {
            if ci >= ncols {
                break;
            }
            if tc.attr_bool("hMerge").unwrap_or(false) || tc.attr_bool("vMerge").unwrap_or(false) || grid[ri][ci].is_some() {
                continue;
            }
            let span_c = tc.attr_i64("gridSpan").unwrap_or(1).clamp(1, (ncols - ci) as i64) as usize;
            let span_r = tc.attr_i64("rowSpan").unwrap_or(1).clamp(1, (nrows - ri) as i64) as usize;
            let area = Area { r0: ri, r1: ri + span_r, c0: ci, c1: ci + span_c };
            let idx = cells.len();
            for row in grid.iter_mut().take(area.r1).skip(area.r0) {
                for g in row.iter_mut().take(area.c1).skip(area.c0) {
                    g.get_or_insert(idx);
                }
            }
            let tcpr = tc.child("a:tcPr");
            let parts = style_parts(style, fl, area, nrows, ncols);
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
            let frame = tc.child("a:txBody").map(|body| {
                let tx = text_list(&text_style(&parts));
                let mut lists: Vec<&Node> = Vec::new();
                if let Some(own) = body.child("a:lstStyle") {
                    lists.push(own);
                }
                lists.push(&tx);
                lists.extend(base_lists.iter().copied());
                let env = TextEnv { sheets: &b.sheets, lists, font_ref: None, font_color: None, font_scale: 1.0, slide_no: b.slide_no };
                richtext::frame(&env, body, &bp)
            });
            cells.push(Cell { tc, area, frame, bp });
        }
    }

    // Radhöjder: raden växer med texten. Celler över flera rader växer i sin sista rad.
    let vertical = |bp: &richtext::BodyProps| bp.vert != "horz";
    let mut order: Vec<&Cell> = cells.iter().filter(|c| c.frame.is_some() && !vertical(&c.bp)).collect();
    order.sort_by_key(|c| c.area.r1 - c.area.r0);
    for c in order {
        let Some(fr) = &c.frame else { continue };
        let w = col_x[c.area.c1] - col_x[c.area.c0];
        let laid = text::layout(fr, w, 1.0e6, &b.sheets.deck.fonts);
        let need = laid.height + fr.insets.1 + fr.insets.3;
        let have: f32 = heights[c.area.r0..c.area.r1].iter().sum();
        if need > have {
            heights[c.area.r1 - 1] += need - have;
        }
    }
    let row_y: Vec<f32> = std::iter::once(0.0)
        .chain(heights.iter().scan(0.0, |acc, h| {
            *acc += h;
            Some(*acc)
        }))
        .collect();
    let total_h = row_y[nrows];
    // Tabellens koordinater → bilden (från höger till vänster: första kolumnen längst till höger).
    let mx = |x: f32| if rtl_table { total_w - x } else { x };
    let tf = |p: Path| p.transformed(&f.t);

    // Tabellens bakgrund: `a:tblPr`-fyllningen eller formatets `a:tblBg`.
    let table_frame = Frame { w: total_w, h: total_h, ..*f };
    let bg = match tbl_pr.and_then(|p| p.children.iter().find(|n| inherit::FILLS.contains(&n.name.as_str()))) {
        Some(n) => b.paint(n, None, &table_frame, owner),
        None => style.and_then(|s| s.child("a:tblBg")).and_then(|n| style_fill(b, n, &table_frame, owner)).flatten(),
    };
    if let Some(p) = bg {
        b.scene.fill(tf(Path::rect(0.0, 0.0, total_w, total_h)), p);
    }

    // Kanterna per cell och sida.
    let mut edges: Vec<[Edge; 4]> = Vec::with_capacity(cells.len());
    for c in &cells {
        let tcpr = c.tc.child("a:tcPr");
        let parts = style_parts(style, fl, c.area, nrows, ncols);
        let side = |s| resolve_side(b, tcpr, &parts, c.area, s, &table_frame, owner);
        edges.push([side(Side::Left), side(Side::Right), side(Side::Top), side(Side::Bottom)]);
    }

    // Fyllningar och text.
    let mut diagonals: Vec<(Path, Line)> = Vec::new();
    for c in &cells {
        let (mut x0, mut x1) = (col_x[c.area.c0], col_x[c.area.c1]);
        if rtl_table {
            (x0, x1) = (mx(x1), mx(x0));
        }
        let (y0, y1) = (row_y[c.area.r0], row_y[c.area.r1]);
        let cell = Frame { t: Affine::translate(x0, y0).then(&f.t), w: x1 - x0, h: y1 - y0, x: f.x + x0, y: f.y + y0, ..*f };
        let rect = Path::rect(0.0, 0.0, cell.w, cell.h).transformed(&cell.t);
        let tcpr = c.tc.child("a:tcPr");
        let parts = style_parts(style, fl, c.area, nrows, ncols);
        // Fyllning: cellens egen (även `a:noFill`), annars formatets.
        let own_fill = tcpr.and_then(|p| p.children.iter().find(|n| inherit::FILLS.contains(&n.name.as_str())));
        let paint = match own_fill {
            Some(n) => b.paint(n, None, &cell, owner),
            None => parts.iter().rev().find_map(|p| p.node.child("a:tcStyle").and_then(|s| style_fill(b, s, &cell, owner))).flatten(),
        };
        if let Some(p) = paint {
            b.scene.fill(rect, p);
        }
        // Diagonaler: cellens egna före formatets.
        for (own, sty, down) in [("a:lnTlToBr", "a:tl2br", true), ("a:lnBlToTr", "a:tr2bl", false)] {
            let line = tcpr.and_then(|p| p.child(own)).and_then(|ln| line_of(b, ln, None, &cell, owner)).or_else(|| {
                parts.iter().rev().find_map(|p| p.node.path(&["a:tcStyle", "a:tcBdr"]).and_then(|bdr| bdr.child(sty)).and_then(|e| style_edge(b, e, &cell, owner)))
            });
            if let Some(line) = line.filter(|l| l.stroke.is_some()) {
                let mut p = Path::default();
                let (ya, yb) = if down { (0.0, cell.h) } else { (cell.h, 0.0) };
                p.move_to(0.0, ya);
                p.line_to(cell.w, yb);
                diagonals.push((p.transformed(&cell.t), line));
            }
        }
        if let Some(fr) = &c.frame {
            b.place_text(fr, &c.bp, &cell, (0.0, 0.0, cell.w, cell.h));
        }
    }

    // Rutnätets linjer: vågräta (rad `gr`, kolumn `c`) och lodräta (rad `r`, linje `gc`).
    let at = |r: usize, c: usize| grid.get(r).and_then(|row| row.get(c)).copied().flatten();
    let mut segs: Vec<((f32, f32), (f32, f32), Line, bool)> = Vec::new();
    for gr in 0..=nrows {
        for c in 0..ncols {
            let above = gr.checked_sub(1).and_then(|r| at(r, c));
            let below = (gr < nrows).then(|| at(gr, c)).flatten();
            if above.is_some() && above == below {
                continue;
            }
            let line = pick(above.map(|i| &edges[i][3]), below.map(|i| &edges[i][2]));
            if let Some(l) = line.filter(|l| l.stroke.is_some()) {
                let y = row_y[gr];
                segs.push(((mx(col_x[c]), y), (mx(col_x[c + 1]), y), l, true));
            }
        }
    }
    for gc in 0..=ncols {
        for r in 0..nrows {
            let left = gc.checked_sub(1).and_then(|c| at(r, c));
            let right = (gc < ncols).then(|| at(r, gc)).flatten();
            if left.is_some() && left == right {
                continue;
            }
            // Från höger till vänster byter vänster och höger sida plats, men linjen är densamma.
            let line = pick(left.map(|i| &edges[i][1]), right.map(|i| &edges[i][0]));
            if let Some(l) = line.filter(|l| l.stroke.is_some()) {
                let x = mx(col_x[gc]);
                segs.push(((x, row_y[r]), (x, row_y[r + 1]), l, false));
            }
        }
    }
    // Slå ihop bitar som fortsätter varandra med samma utseende.
    let mut merged: Vec<((f32, f32), (f32, f32), Line, bool)> = Vec::new();
    for s in segs {
        if let Some(last) = merged.last_mut() {
            let joins = last.3 == s.3 && last.2.same(&s.2) && (last.1 .0 - s.0 .0).abs() < 1e-3 && (last.1 .1 - s.0 .1).abs() < 1e-3;
            let joins_rev = last.3 == s.3 && last.2.same(&s.2) && (last.0 .0 - s.1 .0).abs() < 1e-3 && (last.0 .1 - s.1 .1).abs() < 1e-3;
            if joins {
                last.1 = s.1;
                continue;
            }
            if joins_rev {
                last.0 = s.0;
                continue;
            }
        }
        merged.push(s);
    }
    for (a, z, line, horizontal) in merged {
        let Some(stroke) = line.stroke else { continue };
        // Heldragna kanter får fyrkantiga ändar så att hörnen fylls; streckade behåller mönstret.
        let cap = if stroke.dash.is_empty() { Cap::Square } else { Cap::Butt };
        let lines: Vec<(f32, f32)> = if line.double { vec![(-stroke.width / 3.0, stroke.width / 3.0), (stroke.width / 3.0, stroke.width / 3.0)] } else { vec![(0.0, stroke.width)] };
        for (off, w) in lines.into_iter().flat_map(|l| std::iter::repeat_n(l, line.times as usize)) {
            let (dx, dy) = if horizontal { (0.0, off) } else { (off, 0.0) };
            let mut p = Path::default();
            p.move_to(a.0 + dx, a.1 + dy);
            p.line_to(z.0 + dx, z.1 + dy);
            b.scene.stroke(tf(p), Stroke { width: w, cap, ..stroke.clone() });
        }
    }
    for (p, line) in diagonals {
        if let Some(s) = line.stroke {
            b.scene.stroke(p, s);
        }
    }
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
