//! Tabellformat i Word: läser `w:tblPr`/`w:trPr`/`w:tcPr` och tabellmallarna i `styles.xml`
//! (inklusive villkorlig formatering, `w:tblStylePr` med `w:tblLook`) till Kladds `TableFmt`,
//! och skriver ett `TableFmt` tillbaka som tabellegenskaper. Utläggningen av villkorsmallarna
//! (vilken som gäller för en cell) följer Words ordning, som också WordCrafts `crates/layout` använder:
//! hela tabellen, kolumnband, radband, första/sista kolumn, första/sista rad, hörn.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::word::tablefmt::{BOTTOM, BStyle, Border, CellFmt, DEFAULT_PADS, LEFT, RIGHT, RowFmt, TAlign, TOP, TableFmt, VAlign, Width, parse_color};

// ------------------------------------------------------------------ litet XML-träd

#[derive(Default, Debug)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Node>,
}

fn local(name: &[u8]) -> String {
    let i = name.iter().rposition(|&b| b == b':').map_or(0, |i| i + 1);
    String::from_utf8_lossy(&name[i..]).into_owned()
}

fn node_of(e: &BytesStart<'_>) -> Node {
    let attrs = e
        .attributes()
        .flatten()
        .map(|a| (local(a.key.as_ref()), String::from_utf8_lossy(&a.value).into_owned()))
        .collect();
    Node { name: local(e.name().as_ref()), attrs, kids: Vec::new() }
}

/// Hur djupt trädet byggs. Tabellformatet ligger några nivåer ner (tbl › tr › tc › tcPr › …);
/// djupare element (t.ex. tusentals nästlade tabeller i en trasig fil) hoppas över så att
/// trädet aldrig blir så djupt att rekursion (drop, kloning) spränger stacken.
const MAX_DEPTH: usize = 48;

/// Läser `xml` (ett element eller ett helt dokument) till ett träd. Text hoppas över.
fn parse_tree(xml: &str) -> Option<Node> {
    let mut r = Reader::from_str(xml);
    let mut stack: Vec<Node> = vec![Node::default()];
    // Antal öppna element som hoppas över för att de ligger djupare än MAX_DEPTH.
    let mut skipped = 0usize;
    loop {
        match r.read_event() {
            Ok(Event::Start(_)) if skipped > 0 || stack.len() > MAX_DEPTH => skipped += 1,
            Ok(Event::Start(e)) => stack.push(node_of(&e)),
            Ok(Event::Empty(_)) if skipped > 0 || stack.len() > MAX_DEPTH => {}
            Ok(Event::Empty(e)) => stack.last_mut()?.kids.push(node_of(&e)),
            Ok(Event::End(_)) if skipped > 0 => skipped -= 1,
            Ok(Event::End(_)) => {
                let n = stack.pop()?;
                stack.last_mut()?.kids.push(n);
                if stack.len() == 1 && stack[0].kids.len() == 1 && stack[0].name.is_empty() {
                    // Fortsätt läsa: ett dokument har ett rotelement.
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let mut root = stack.into_iter().next()?;
    if root.kids.len() == 1 { root.kids.pop() } else { Some(root) }
}

impl Node {
    fn a(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    fn kid(&self, k: &str) -> Option<&Node> {
        self.kids.iter().find(|n| n.name == k)
    }

    fn kids_named<'a>(&'a self, k: &'a str) -> impl Iterator<Item = &'a Node> {
        self.kids.iter().filter(move |n| n.name == k)
    }

    fn on(&self) -> bool {
        !matches!(self.a("val"), Some("0" | "false" | "off" | "none"))
    }
}

// ------------------------------------------------------------------ egenskaper

fn border_of(n: &Node) -> Border {
    let style = n.a("val").map_or(BStyle::Single, BStyle::from_word);
    let sz = n.a("sz").and_then(|v| v.parse::<u16>().ok()).unwrap_or(4);
    let color = n.a("color").and_then(parse_color);
    Border { style, sz: if style == BStyle::None { 0 } else { sz.clamp(2, 96) }, color }
}

fn fill_of(n: &Node) -> Option<Option<[u8; 3]>> {
    if let Some(f) = n.a("fill") {
        if let Some(c) = parse_color(f) {
            return Some(Some(c));
        }
        if f == "auto" && n.a("val") != Some("solid") {
            return Some(None);
        }
    }
    if n.a("val") == Some("solid") {
        return Some(n.a("color").and_then(parse_color));
    }
    None
}

fn twips_pt(v: Option<&str>) -> Option<f32> {
    v.and_then(|v| v.parse::<f32>().ok()).map(|t| t / 20.0)
}

/// Det en tabellmall (eller villkorsdel) anger.
#[derive(Default, Clone, Debug)]
struct Props {
    /// top, left, bottom, right, insideH, insideV.
    tbl_borders: [Option<Border>; 6],
    cell_borders: [Option<Border>; 4],
    fill: Option<Option<[u8; 3]>>,
    /// vänster, höger, överkant, nederkant.
    pad: [Option<f32>; 4],
    bold: Option<bool>,
    italic: Option<bool>,
    color: Option<[u8; 3]>,
}

impl Props {
    fn overlay(&mut self, o: &Props) {
        for i in 0..6 {
            self.tbl_borders[i] = o.tbl_borders[i].or(self.tbl_borders[i]);
        }
        for i in 0..4 {
            self.cell_borders[i] = o.cell_borders[i].or(self.cell_borders[i]);
        }
        self.fill = o.fill.or(self.fill);
        for i in 0..4 {
            self.pad[i] = o.pad[i].or(self.pad[i]);
        }
        self.bold = o.bold.or(self.bold);
        self.italic = o.italic.or(self.italic);
        self.color = o.color.or(self.color);
    }
}

fn side_index(name: &str) -> Option<usize> {
    Some(match name {
        "top" => 0,
        "left" | "start" => 1,
        "bottom" => 2,
        "right" | "end" => 3,
        "insideH" => 4,
        "insideV" => 5,
        _ => return None,
    })
}

/// Egenskaperna i ett element som innehåller `tblPr`, `tcPr` och `rPr` (en mall eller `tblStylePr`).
fn props_of(n: &Node) -> Props {
    let mut p = Props::default();
    for kid in &n.kids {
        match kid.name.as_str() {
            "tblPr" => {
                if let Some(b) = kid.kid("tblBorders") {
                    for s in &b.kids {
                        if let Some(i) = side_index(&s.name) {
                            p.tbl_borders[i] = Some(border_of(s));
                        }
                    }
                }
                if let Some(m) = kid.kid("tblCellMar") {
                    for s in &m.kids {
                        match s.name.as_str() {
                            "left" | "start" => p.pad[0] = twips_pt(s.a("w")),
                            "right" | "end" => p.pad[1] = twips_pt(s.a("w")),
                            "top" => p.pad[2] = twips_pt(s.a("w")),
                            "bottom" => p.pad[3] = twips_pt(s.a("w")),
                            _ => {}
                        }
                    }
                }
            }
            "tcPr" => cell_props(kid, &mut p),
            "rPr" => {
                if let Some(b) = kid.kid("b") {
                    p.bold = Some(b.on());
                }
                if let Some(i) = kid.kid("i") {
                    p.italic = Some(i.on());
                }
                p.color = kid.kid("color").and_then(|c| c.a("val")).and_then(parse_color);
            }
            _ => {}
        }
    }
    p
}

fn cell_props(tc_pr: &Node, p: &mut Props) {
    if let Some(s) = tc_pr.kid("shd") {
        p.fill = fill_of(s);
    }
    if let Some(b) = tc_pr.kid("tcBorders") {
        for s in &b.kids {
            if let Some(i) = side_index(&s.name).filter(|&i| i < 4) {
                p.cell_borders[i] = Some(border_of(s));
            }
        }
    }
}

/// En tabellmall.
#[derive(Default, Clone, Debug)]
struct TStyle {
    based_on: Option<String>,
    base: Props,
    cond: HashMap<String, Props>,
}

/// Tabellmallarna i `styles.xml`.
#[derive(Clone, Debug)]
pub struct TableStyles {
    map: HashMap<String, TStyle>,
    default: Option<String>,
}

impl Default for TableStyles {
    fn default() -> Self {
        Self::parse("")
    }
}

impl TableStyles {
    /// Mallen "Table Grid" som Word själv skulle lägga till: enkla ramar överallt. Den används också
    /// när en fil pekar på `TableGrid` utan att definiera den (sparar Kladd filen får den mallen).
    fn table_grid() -> TStyle {
        let mut base = Props::default();
        base.tbl_borders = [Some(Border::GRID); 6];
        TStyle { based_on: None, base, cond: HashMap::new() }
    }

    pub fn parse(styles_xml: &str) -> TableStyles {
        let mut out = TableStyles { map: HashMap::new(), default: None };
        out.map.insert("TableGrid".to_owned(), Self::table_grid());
        if !styles_xml.contains("w:type=\"table\"") {
            return out;
        }
        let Some(root) = parse_tree(styles_xml) else { return out };
        for st in root.kids_named("style").filter(|s| s.a("type") == Some("table")) {
            let Some(id) = st.a("styleId") else { continue };
            let mut ts = TStyle { based_on: st.kid("basedOn").and_then(|b| b.a("val")).map(str::to_owned), base: props_of(st), cond: HashMap::new() };
            for c in st.kids_named("tblStylePr") {
                if let Some(t) = c.a("type") {
                    ts.cond.insert(t.to_owned(), props_of(c));
                }
            }
            if matches!(st.a("default"), Some("1" | "true")) {
                out.default = Some(id.to_owned());
            }
            out.map.insert(id.to_owned(), ts);
        }
        out
    }

    /// Kedjan från den mest grundläggande mallen till `id`.
    fn chain(&self, id: Option<&str>) -> Vec<&TStyle> {
        let mut v = Vec::new();
        let mut cur = id.or(self.default.as_deref());
        while let Some(i) = cur {
            if v.len() >= 8 {
                break;
            }
            let Some(s) = self.map.get(i) else { break };
            v.push(s);
            cur = s.based_on.as_deref();
        }
        v.reverse();
        v
    }
}

// ------------------------------------------------------------------ läsa en tabell

/// Det en mall lägger till i cellens text (fetstil, kursiv, färg), för celler utan egna val.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct CellRpr {
    pub bold: bool,
    pub italic: bool,
    pub color: Option<[u8; 3]>,
}

#[derive(Debug, Clone)]
pub struct TableRead {
    pub fmt: TableFmt,
    /// Formatmallens textformat per rad och cell (i dokumentordning, utan platshållare).
    pub rpr: Vec<Vec<CellRpr>>,
    /// `w:tblW` som procent (hundradelar av en procent) om den är en procentsats.
    pub pct: Option<u16>,
}

struct TblLook {
    first_row: bool,
    last_row: bool,
    first_col: bool,
    last_col: bool,
    no_hband: bool,
    no_vband: bool,
}

fn look_of(n: Option<&Node>) -> TblLook {
    let mut l = TblLook { first_row: false, last_row: false, first_col: false, last_col: false, no_hband: false, no_vband: false };
    let Some(n) = n else { return l };
    let flag = |k: &str, bit: u32| -> bool {
        match n.a(k) {
            Some(v) => matches!(v, "1" | "true" | "on"),
            None => n.a("val").and_then(|v| u32::from_str_radix(v, 16).ok()).is_some_and(|m| m & bit != 0),
        }
    };
    l.first_row = flag("firstRow", 0x20);
    l.last_row = flag("lastRow", 0x40);
    l.first_col = flag("firstColumn", 0x80);
    l.last_col = flag("lastColumn", 0x100);
    l.no_hband = flag("noHBand", 0x200);
    l.no_vband = flag("noVBand", 0x400);
    l
}

/// Läser en tabell (`<w:tbl>…</w:tbl>` för toppnivåtabellen; nästlade tabeller räknas inte).
pub fn read(tbl_xml: &str, styles: &TableStyles) -> Option<TableRead> {
    let root = parse_tree(tbl_xml)?;
    if root.name != "tbl" {
        return None;
    }
    let tbl_pr = root.kid("tblPr");
    let style_id = tbl_pr.and_then(|p| p.kid("tblStyle")).and_then(|s| s.a("val"));
    let chain = styles.chain(style_id);
    let mut base = Props::default();
    for s in &chain {
        base.overlay(&s.base);
    }
    let direct = tbl_pr.map(|p| props_of(&Node { kids: vec![clone_node(p)], ..Default::default() })).unwrap_or_default();
    let mut table_level = base.clone();
    table_level.overlay(&direct);

    let mut fmt = TableFmt::word_base();
    for i in 0..6 {
        fmt.borders[i] = table_level.tbl_borders[i].unwrap_or(Border::NONE);
    }
    for i in 0..4 {
        fmt.pad[i] = table_level.pad[i].unwrap_or(DEFAULT_PADS[i]);
    }
    // Marginaler på cellerna (`w:tcMar`) utan egen tabellmarginal (`w:tblCellMar`): den första cellens gäller hela tabellen.
    if direct.pad.iter().all(Option::is_none) {
        if let Some(m) = root.kids_named("tr").flat_map(|r| r.kids_named("tc")).find_map(|c| c.kid("tcPr").and_then(|p| p.kid("tcMar"))) {
            for s in &m.kids {
                let i = match s.name.as_str() {
                    "left" | "start" => 0,
                    "right" | "end" => 1,
                    "top" => 2,
                    "bottom" => 3,
                    _ => continue,
                };
                if let Some(v) = twips_pt(s.a("w")) {
                    fmt.pad[i] = v;
                }
            }
        }
    }

    let grid: Vec<f64> = root.kid("tblGrid").map(|g| g.kids_named("gridCol").map(|c| c.a("w").and_then(|v| v.parse().ok()).unwrap_or(0.0)).collect()).unwrap_or_default();
    let grid_sum: f64 = grid.iter().sum();
    let mut pct = None;
    if let Some(p) = tbl_pr {
        if let Some(w) = p.kid("tblW") {
            let v = w.a("w").and_then(|v| v.trim_end_matches('%').parse::<f32>().ok()).unwrap_or(0.0);
            match w.a("type") {
                Some("pct") if v > 0.0 => {
                    let pc = if w.a("w").is_some_and(|s| s.ends_with('%')) { v } else { v / 50.0 };
                    pct = Some((pc * 100.0).round() as u16);
                    fmt.width = Width::Pct(pc.round().clamp(1.0, 1000.0) as u16);
                }
                Some("dxa") if v > 0.0 => fmt.width = Width::Pt((v / 20.0).round().clamp(1.0, 6000.0) as u16),
                _ => {}
            }
        }
        match p.kid("jc").and_then(|j| j.a("val")) {
            Some("center") => fmt.align = TAlign::Center,
            Some("right" | "end") => fmt.align = TAlign::Right,
            _ => {}
        }
        if let Some(i) = p.kid("tblInd") {
            fmt.indent = twips_pt(i.a("w")).unwrap_or(0.0);
        }
        fmt.fixed = p.kid("tblLayout").and_then(|l| l.a("type")) == Some("fixed");
    }
    // Utan egen bredd följer en tabell som får växa innehållet; med fast utläggning gäller rutnätet.
    if fmt.width == Width::Auto && fmt.fixed && grid_sum > 0.0 {
        fmt.width = Width::Pt((grid_sum / 20.0).round().clamp(1.0, 6000.0) as u16);
    }

    let look = look_of(tbl_pr.and_then(|p| p.kid("tblLook")));
    let rows: Vec<&Node> = root.kids_named("tr").collect();
    let nrows = rows.len();
    let mut rpr: Vec<Vec<CellRpr>> = Vec::new();
    for (ri, tr) in rows.iter().enumerate() {
        let mut rf = RowFmt::default();
        if let Some(pr) = tr.kid("trPr") {
            if let Some(h) = pr.kid("trHeight") {
                rf.h = twips_pt(h.a("val")).map_or(0, |v| v.round().clamp(0.0, 1000.0) as u16);
                rf.exact = h.a("hRule") == Some("exact");
            }
            rf.head = pr.kid("tblHeader").is_some_and(Node::on);
        }
        if rf != RowFmt::default() {
            fmt.rows.insert(ri, rf);
        }
        let cells: Vec<&Node> = tr.kids_named("tc").collect();
        let ncells = cells.len();
        let mut col = 0usize;
        let mut row_rpr = Vec::new();
        for (ci, tc) in cells.iter().enumerate() {
            let pr = tc.kid("tcPr");
            let span = pr.and_then(|p| p.kid("gridSpan")).and_then(|g| g.a("val")).and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, 63);
            // Villkorsdelar som gäller cellen, lägst först.
            let mut names: Vec<&str> = vec!["wholeTable"];
            if !look.no_vband {
                names.push(match col.checked_sub(usize::from(look.first_col)) {
                    Some(c) if c % 2 == 0 => "band1Vert",
                    _ => "band2Vert",
                });
            }
            if !look.no_hband && ri >= usize::from(look.first_row) {
                names.push(if (ri - usize::from(look.first_row)) % 2 == 0 { "band1Horz" } else { "band2Horz" });
            }
            let is_first_col = look.first_col && ci == 0;
            let is_last_col = look.last_col && ci + 1 == ncells;
            let is_first_row = look.first_row && ri == 0;
            let is_last_row = look.last_row && ri + 1 == nrows;
            if is_first_col {
                names.push("firstCol");
            }
            if is_last_col {
                names.push("lastCol");
            }
            if is_first_row {
                names.push("firstRow");
            }
            if is_last_row {
                names.push("lastRow");
            }
            for (a, b, n) in [(is_first_row, is_first_col, "nwCell"), (is_first_row, is_last_col, "neCell"), (is_last_row, is_first_col, "swCell"), (is_last_row, is_last_col, "seCell")] {
                if a && b {
                    names.push(n);
                }
            }
            let mut cp = Props::default();
            cp.overlay(&Props { fill: base.fill, bold: base.bold, italic: base.italic, color: base.color, cell_borders: base.cell_borders, ..Default::default() });
            for n in &names {
                for s in &chain {
                    if let Some(c) = s.cond.get(*n) {
                        cp.overlay(&Props { tbl_borders: Default::default(), pad: Default::default(), ..c.clone() });
                    }
                }
            }
            row_rpr.push(CellRpr { bold: cp.bold == Some(true), italic: cp.italic == Some(true), color: cp.color });
            // Cellens egna val går före mallens.
            let mut own = Props::default();
            if let Some(p) = pr {
                cell_props(p, &mut own);
            }
            let mut all = cp.clone();
            all.overlay(&own);
            let mut cf = CellFmt { fill: all.fill.flatten(), valign: VAlign::Top, halign: halign_of(tc), borders: all.cell_borders };
            if let Some(v) = pr.and_then(|p| p.kid("vAlign")).and_then(|v| v.a("val")) {
                cf.valign = match v {
                    "center" => VAlign::Center,
                    "bottom" => VAlign::Bottom,
                    _ => VAlign::Top,
                };
            }
            // Lodrätt sammanfogade celler: ingen linje mellan dem.
            if let Some(vm) = pr.and_then(|p| p.kid("vMerge")) {
                if vm.a("val") != Some("restart") {
                    cf.borders[TOP] = Some(Border::NONE);
                }
            }
            if cf != CellFmt::default() {
                fmt.cells.insert((ri, col), cf);
            }
            col += span;
        }
        rpr.push(row_rpr);
    }
    // Cellens egna ramar på sidor som är lika med tabellens behövs inte.
    Some(TableRead { fmt, rpr, pct })
}

/// Gemensam justering för cellens stycken (`w:jc`), annars vänster.
fn halign_of(tc: &Node) -> TAlign {
    let mut got: Option<TAlign> = None;
    for p in tc.kids_named("p") {
        let a = match p.kid("pPr").and_then(|x| x.kid("jc")).and_then(|j| j.a("val")) {
            Some("center") => TAlign::Center,
            Some("right" | "end") => TAlign::Right,
            _ => TAlign::Left,
        };
        match got {
            None => got = Some(a),
            Some(g) if g != a => return TAlign::Left,
            _ => {}
        }
    }
    got.unwrap_or(TAlign::Left)
}

fn clone_node(n: &Node) -> Node {
    Node { name: n.name.clone(), attrs: n.attrs.clone(), kids: n.kids.iter().map(clone_node).collect() }
}

// ------------------------------------------------------------------ till Markdown

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        match rest[i..].find('>') {
            Some(j) if rest[i..].starts_with("<span") || rest[i..].starts_with("</span") || rest[i..].starts_with("<u>") || rest[i..].starts_with("</u>") => {
                rest = &rest[i + j + 1..];
            }
            _ => {
                out.push('<');
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Är hela cellens text fet (varje stycke, skilda med `<br>`)?
fn all_bold(cell: &str) -> bool {
    cell.split("<br>").all(|part| {
        let t = strip_tags(part);
        let t = t.trim();
        t.is_empty() || (t.len() > 4 && t.starts_with("**") && t.ends_with("**"))
    })
}

/// Tar bort fetstilen i en helt fet cell (Kladd gör rubrikraden fet själv).
fn unbold(cell: &str) -> String {
    cell.replace("**", "")
}

/// Lägger formatmallens text (fetstil, kursiv, färg) på celler som inte har egna val.
fn overlay_text(cell: &str, r: &CellRpr) -> String {
    let t = cell.trim();
    if t.is_empty() {
        return cell.to_owned();
    }
    let mut s = t.to_owned();
    if r.italic && !s.contains('*') {
        s = format!("*{s}*");
    }
    if r.bold && !s.contains("**") {
        s = format!("**{s}**");
    }
    if let Some(c) = r.color.filter(|_| !s.contains("color:")) {
        s = format!("<span style=\"color: #{}\">{s}</span>", crate::word::tablefmt::color_hex(Some(c)));
    }
    s
}

/// Tabellens celler och format färdiga för Markdown: mallens text läggs på cellerna, en helt fet
/// rubrikrad görs till Kladds egen fetstil, och bredden avgör kolumnstrecken. Ger formatet om
/// det skiljer sig från en tabell utan format, och kolumnbredderna (tjugondels punkter).
pub fn prepare(rows: &mut [Vec<String>], spans: &[Vec<usize>], read: Option<&TableRead>, grid: &[f64], text_w: Option<f64>) -> (Option<TableFmt>, Vec<f64>) {
    let Some(read) = read else {
        let narrow = text_w.is_some_and(|tw| tw > 0.0 && grid.iter().sum::<f64>() < tw * 0.9);
        return (None, if narrow { Vec::new() } else { grid.to_vec() });
    };
    for (ri, row) in rows.iter_mut().enumerate() {
        let mut idx = 0;
        for (k, cell) in row.iter_mut().enumerate() {
            if spans.get(ri).and_then(|s| s.get(k)).copied() == Some(0) {
                continue;
            }
            if let Some(r) = read.rpr.get(ri).and_then(|r| r.get(idx)).filter(|r| **r != CellRpr::default()) {
                *cell = overlay_text(cell, r);
            }
            idx += 1;
        }
    }
    let mut fmt = read.fmt.clone();
    fmt.header_bold = !rows.is_empty() && rows[0].iter().all(|c| c.is_empty() || all_bold(c)) && rows[0].iter().any(|c| !c.is_empty());
    if fmt.header_bold {
        for c in rows[0].iter_mut() {
            *c = unbold(c);
        }
    }
    // Rubrikradens upprepning är Kladds standard.
    if fmt.rows.get(&0).is_some_and(|r| r.head && r.h == 0) {
        fmt.rows.remove(&0);
    }
    let sum: f64 = grid.iter().sum();
    let wide = text_w.is_some_and(|tw| tw > 0.0 && sum >= tw * 0.9);
    // Full bredd är det en tabell utan egen bredd får; den skrivs bara när formatet ändå har en rad.
    // Utan egen bredd men med ett rutnät som fyller sidan (python-docx, LibreOffice) gäller rutnätet.
    if fmt.width == Width::Auto && wide {
        fmt.width = Width::Pt((sum / 20.0).round().clamp(1.0, 6000.0) as u16);
    }
    let mut plain = fmt.clone();
    if matches!(plain.width, Width::Pct(100)) || (wide && matches!(plain.width, Width::Pt(_)) && read.fmt.width != Width::Auto) {
        plain.width = Width::Auto;
    }
    let widths = if plain.width == Width::Auto && fmt.width == Width::Auto && !wide && text_w.is_some() && read.pct.is_none() { Vec::new() } else { grid.to_vec() };
    if plain.is_default() {
        return (None, widths);
    }
    (Some(fmt), widths)
}

// ------------------------------------------------------------------ skriva

fn color_attr(c: Option<[u8; 3]>) -> String {
    crate::word::tablefmt::color_hex(c)
}

fn border_xml(tag: &str, b: Border) -> String {
    if b.style == BStyle::None {
        return format!("<w:{tag} w:val=\"nil\"/>");
    }
    format!("<w:{tag} w:val=\"{}\" w:sz=\"{}\" w:space=\"0\" w:color=\"{}\"/>", b.style.word(), b.sz.max(2), color_attr(b.color))
}

/// `w:tblPr`-innehållet (utan `tblStyle`, `tblLook`) för ett format; `total` är textbredden i twips.
pub fn tbl_pr_xml(fmt: &TableFmt) -> String {
    let mut s = String::new();
    s.push_str("<w:tblStyle w:val=\"TableGrid\"/>");
    match fmt.width {
        Width::Pct(p) => s.push_str(&format!("<w:tblW w:w=\"{}\" w:type=\"pct\"/>", u32::from(p) * 50)),
        Width::Pt(p) => s.push_str(&format!("<w:tblW w:w=\"{}\" w:type=\"dxa\"/>", u32::from(p) * 20)),
        Width::Auto => s.push_str("<w:tblW w:w=\"5000\" w:type=\"pct\"/>"),
    }
    match fmt.align {
        TAlign::Left => {}
        TAlign::Center => s.push_str("<w:jc w:val=\"center\"/>"),
        TAlign::Right => s.push_str("<w:jc w:val=\"right\"/>"),
    }
    if fmt.indent.abs() > 0.01 {
        s.push_str(&format!("<w:tblInd w:w=\"{}\" w:type=\"dxa\"/>", (fmt.indent * 20.0).round() as i32));
    }
    // Ramar: alltid utskrivna så att ett format utan ramar inte får rutnätet från tabellmallen.
    s.push_str("<w:tblBorders>");
    for (i, tag) in ["top", "left", "bottom", "right", "insideH", "insideV"].iter().enumerate() {
        s.push_str(&border_xml(tag, fmt.borders[i]));
    }
    s.push_str("</w:tblBorders>");
    if fmt.fixed {
        s.push_str("<w:tblLayout w:type=\"fixed\"/>");
    }
    if fmt.pad != DEFAULT_PADS {
        let t = |v: f32| (v * 20.0).round() as i32;
        s.push_str(&format!(
            "<w:tblCellMar><w:top w:w=\"{}\" w:type=\"dxa\"/><w:left w:w=\"{}\" w:type=\"dxa\"/><w:bottom w:w=\"{}\" w:type=\"dxa\"/><w:right w:w=\"{}\" w:type=\"dxa\"/></w:tblCellMar>",
            t(fmt.pad[2]), t(fmt.pad[0]), t(fmt.pad[3]), t(fmt.pad[1])
        ));
    }
    s
}

/// `w:trPr`-innehållet för rad `r`.
pub fn tr_pr_xml(fmt: &TableFmt, r: usize, head: bool) -> String {
    let rf = fmt.rows.get(&r).copied().unwrap_or_default();
    let mut s = String::new();
    if rf.h > 0 {
        s.push_str(&format!("<w:trHeight w:val=\"{}\" w:hRule=\"{}\"/>", u32::from(rf.h) * 20, if rf.exact { "exact" } else { "atLeast" }));
    }
    if head || rf.head {
        s.push_str("<w:tblHeader/>");
    }
    s
}

/// Cellens `w:tcPr`-innehåll efter `tcW`/`gridSpan`: ramar, skuggning, lodrät justering (i schemats ordning).
pub fn tc_pr_xml(fmt: &TableFmt, r: usize, c: usize) -> String {
    let Some(cf) = fmt.cell(r, c) else { return String::new() };
    let mut s = String::new();
    if cf.borders.iter().any(Option::is_some) {
        s.push_str("<w:tcBorders>");
        for (i, tag) in [(TOP, "top"), (LEFT, "left"), (BOTTOM, "bottom"), (RIGHT, "right")] {
            if let Some(b) = cf.borders[i] {
                s.push_str(&border_xml(tag, b));
            }
        }
        s.push_str("</w:tcBorders>");
    }
    if let Some(f) = cf.fill {
        s.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>", color_attr(Some(f))));
    }
    match cf.valign {
        VAlign::Top => {}
        VAlign::Center => s.push_str("<w:vAlign w:val=\"center\"/>"),
        VAlign::Bottom => s.push_str("<w:vAlign w:val=\"bottom\"/>"),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblCellMar><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>
<w:style w:type="table" w:styleId="GridTable4-Accent1"><w:name w:val="Grid Table 4 Accent 1"/><w:basedOn w:val="TableNormal"/>
<w:tblPr><w:tblStyleRowBandSize w:val="1"/><w:tblBorders><w:top w:val="single" w:sz="4" w:color="8EAADB"/><w:left w:val="single" w:sz="4" w:color="8EAADB"/><w:bottom w:val="single" w:sz="4" w:color="8EAADB"/><w:right w:val="single" w:sz="4" w:color="8EAADB"/><w:insideH w:val="single" w:sz="4" w:color="8EAADB"/><w:insideV w:val="single" w:sz="4" w:color="8EAADB"/></w:tblBorders></w:tblPr>
<w:tblStylePr w:type="firstRow"><w:rPr><w:b/><w:bCs/><w:color w:val="FFFFFF"/></w:rPr><w:tcPr><w:tcBorders><w:top w:val="single" w:sz="4" w:color="4472C4"/></w:tcBorders><w:shd w:val="clear" w:color="auto" w:fill="4472C4"/></w:tcPr></w:tblStylePr>
<w:tblStylePr w:type="firstCol"><w:rPr><w:b/><w:bCs/></w:rPr></w:tblStylePr>
<w:tblStylePr w:type="band1Horz"><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/></w:tcPr></w:tblStylePr>
</w:style></w:styles>"#;

    fn tbl(look: &str, body: &str) -> String {
        format!(
            r#"<w:tbl xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:tblPr><w:tblStyle w:val="GridTable4-Accent1"/><w:tblW w:w="5000" w:type="pct"/>{look}</w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="3000"/></w:tblGrid>{body}</w:tbl>"#
        )
    }

    fn row(cells: usize) -> String {
        format!("<w:tr>{}</w:tr>", "<w:tc><w:tcPr/><w:p/></w:tc>".repeat(cells))
    }

    #[test]
    fn djupt_nästlade_tabeller_spränger_inte_stacken() {
        // En trasig fil med tusentals nästlade tabeller i en cell ska läsas utan stack overflow,
        // även på en tråd med liten stack.
        let n = 20_000;
        let inner = format!("{}{}", "<w:tbl><w:tr><w:tc>".repeat(n), "</w:tc></w:tr></w:tbl>".repeat(n));
        let xml = tbl("", &format!("<w:tr><w:tc><w:tcPr/><w:p/>{inner}</w:tc><w:tc><w:p/></w:tc></w:tr>"));
        let t = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || read(&xml, &TableStyles::parse("")).is_some())
            .unwrap()
            .join()
            .expect("stack overflow eller panik");
        assert!(t);
    }

    #[test]
    fn style_borders_and_conditional_header() {
        let st = TableStyles::parse(STYLES);
        let look = r#"<w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/>"#;
        let t = read(&tbl(look, &format!("{}{}{}", row(2), row(2), row(2))), &st).unwrap();
        assert_eq!(t.fmt.borders[0].color, Some([0x8E, 0xAA, 0xDB]));
        assert!(t.fmt.borders.iter().all(Border::visible));
        // Rubrikraden: fyllning och vit fet text ur mallen.
        assert_eq!(t.fmt.fill(0, 0), Some([0x44, 0x72, 0xC4]));
        assert_eq!(t.fmt.fill(0, 2 - 1), Some([0x44, 0x72, 0xC4]));
        assert_eq!(t.rpr[0][0], CellRpr { bold: true, italic: false, color: Some([255, 255, 255]) });
        // Första kolumnen fet, band1Horz på rad 1 (första raden efter rubriken), inte rad 2.
        assert!(t.rpr[1][0].bold && !t.rpr[1][1].bold);
        assert_eq!(t.fmt.fill(1, 0), Some([0xD9, 0xE2, 0xF3]));
        assert_eq!(t.fmt.fill(2, 0), None);
        assert_eq!(t.fmt.width, Width::Pct(100));
    }

    #[test]
    fn look_flags_switch_conditional_formatting_off() {
        let st = TableStyles::parse(STYLES);
        let look = r#"<w:tblLook w:val="0000" w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/>"#;
        let t = read(&tbl(look, &format!("{}{}", row(2), row(2))), &st).unwrap();
        assert_eq!(t.fmt.fill(0, 0), None);
        assert!(!t.rpr[0][0].bold);
    }

    #[test]
    fn direct_cell_properties_and_table_properties() {
        let st = TableStyles::parse(STYLES);
        let x = r#"<w:tbl xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:tblPr><w:tblW w:w="6000" w:type="dxa"/><w:jc w:val="center"/><w:tblInd w:w="240" w:type="dxa"/><w:tblLayout w:type="fixed"/>
<w:tblBorders><w:top w:val="double" w:sz="6" w:color="C00000"/><w:insideH w:val="dashed" w:sz="4" w:color="808080"/></w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>
<w:tr><w:trPr><w:trHeight w:val="900" w:hRule="exact"/><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/><w:tcBorders><w:bottom w:val="single" w:sz="12" w:color="FF0000"/></w:tcBorders><w:shd w:val="clear" w:color="auto" w:fill="EDE7F6"/><w:vAlign w:val="center"/></w:tcPr><w:p/></w:tc><w:tc><w:tcPr><w:gridSpan w:val="1"/><w:vMerge w:val="restart"/></w:tcPr><w:p/></w:tc></w:tr>
<w:tr><w:tc><w:p/></w:tc><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#;
        let t = read(x, &st).unwrap();
        let f = &t.fmt;
        assert_eq!(f.width, Width::Pt(300));
        assert_eq!(f.align, TAlign::Center);
        assert!((f.indent - 12.0).abs() < 0.01);
        assert!(f.fixed);
        assert_eq!(f.borders[0].style, BStyle::Double);
        assert_eq!(f.borders[4].style, BStyle::Dashed);
        // Tabellen utan mall och utan egna ramar för vänster/höger: inga linjer.
        assert!(!f.borders[1].visible());
        assert_eq!(f.rows[&0], RowFmt { h: 45, exact: true, head: true });
        let c = f.cell(0, 0).unwrap();
        assert_eq!(c.fill, Some([0xED, 0xE7, 0xF6]));
        assert_eq!(c.valign, VAlign::Center);
        assert_eq!(c.borders[BOTTOM].unwrap().sz, 12);
        // Lodrät sammanfogning: fortsättningscellen har ingen överkant.
        assert_eq!(f.cell(1, 1).unwrap().borders[TOP], Some(Border::NONE));
    }

    #[test]
    fn xml_round_trip_of_properties() {
        let st = TableStyles::parse(STYLES);
        let mut f = TableFmt::word_base();
        f.width = Width::Pct(60);
        f.align = TAlign::Right;
        f.borders[0] = Border { style: BStyle::Double, sz: 6, color: Some([0xC0, 0, 0]) };
        f.cell_mut(0, 0).fill = Some([1, 2, 3]);
        f.cell_mut(0, 0).valign = VAlign::Bottom;
        f.rows.insert(0, RowFmt { h: 30, exact: false, head: false });
        let body = format!(
            "<w:tr><w:trPr>{}</w:trPr><w:tc><w:tcPr>{}</w:tcPr><w:p/></w:tc></w:tr>",
            tr_pr_xml(&f, 0, false),
            tc_pr_xml(&f, 0, 0)
        );
        let x = format!(
            "<w:tbl xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:tblPr>{}</w:tblPr><w:tblGrid><w:gridCol w:w=\"3000\"/></w:tblGrid>{body}</w:tbl>",
            tbl_pr_xml(&f)
        );
        let back = read(&x, &st).unwrap().fmt;
        assert_eq!(back.width, Width::Pct(60));
        assert_eq!(back.align, TAlign::Right);
        assert_eq!(back.borders, f.borders);
        assert_eq!(back.cells, f.cells);
        assert_eq!(back.rows, f.rows);
    }

    #[test]
    fn style_without_borders_gives_a_borderless_table() {
        let st = TableStyles::parse(STYLES);
        let x = r#"<w:tbl xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#;
        let t = read(x, &st).unwrap();
        assert!(t.fmt.borders.iter().all(|b| !b.visible()));
        assert_eq!(t.fmt.width, Width::Auto);
    }
}


