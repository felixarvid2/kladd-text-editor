//! Diagrammets inbäddade arbetsbok (`ppt/embeddings/*.xlsx`): förlustfri skrivning av celler, så att
//! PowerPoints *Redigera data* visar samma värden som diagrammets cache.
//!
//! Bara delarna som ändras skrivs om: bladen med ändrade celler (`xl/worksheets/sheetN.xml`, tolkade
//! förlustfritt med `edit_xml.rs`), `xl/sharedStrings.xml` (nya strängar läggs sist; finns delen inte blir
//! texten en `inlineStr`) och tabellerna (`xl/tables/tableN.xml`) som täcker cellerna: deras område växer
//! med nya kolumner/rader och kolumnnamnen följer rubrikcellerna (Excel kräver att de stämmer). Övriga
//! poster kopieras komprimerade som de är. En cell som hade en formel blir ett värde, och då tas
//! `xl/calcChain.xml` bort (med relation och innehållstyp) så att Excel bygger om den.
//! Cellernas format (`s`) behålls. Rader skapas i ordning och celler i kolumnordning; radernas
//! `spans` (en ledtråd) tas bort på ändrade rader, och bladets `dimension` växer.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Write as _};

use super::edit_chart::{col_name, num_text};
use super::edit_xml::{self, escape, XDoc, XElem, XNode};

const NS_S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

/// Ett cellvärde.
#[derive(Clone, Debug, PartialEq)]
pub enum CellVal {
    Num(f64),
    Str(String),
    /// Tom cell (värdet tas bort, formatet står kvar).
    Empty,
}

/// En cell att skriva: bladets namn, kolumn och rad (1-baserade).
#[derive(Clone, Debug, PartialEq)]
pub struct Write {
    pub sheet: String,
    pub col: u32,
    pub row: u32,
    pub val: CellVal,
}

/// Prefix + lokalt namn (`""` = standardnamnrymden).
fn q(p: &str, local: &str) -> String {
    if p.is_empty() { local.to_owned() } else { format!("{p}:{local}") }
}

fn frag(doc: &XDoc, xml: &str) -> Result<XElem, String> {
    doc.fragment(xml).into_iter().next().ok_or_else(|| "ogiltig XML i arbetsboken".to_owned())
}

/// `xl/workbook.xml` + `worksheets/sheet1.xml` → `xl/worksheets/sheet1.xml`.
fn resolve(owner: &str, target: &str) -> String {
    if let Some(t) = target.strip_prefix('/') {
        return t.to_owned();
    }
    let mut parts: Vec<&str> = owner.rsplit_once('/').map_or(Vec::new(), |(d, _)| d.split('/').collect());
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn rels_of(entries: &BTreeMap<String, Vec<u8>>, owner: &str) -> Vec<(String, String, String)> {
    let name = super::edit::rels_name(owner);
    let Some(d) = entries.get(&name).and_then(|b| edit_xml::parse(b)) else { return Vec::new() };
    d.root.elems().filter(|e| e.local() == "Relationship").filter_map(|e| Some((e.attr("Id")?, e.attr("Type").unwrap_or_default(), resolve(owner, &e.attr("Target")?)))).collect()
}

/// Strängens text i ett `si`-element (även formaterad text i `r/t`).
fn si_text(si: &XElem) -> String {
    let mut s = String::new();
    si.walk(&mut |e| {
        if e.local() == "t" {
            s.push_str(&e.text());
        }
    });
    if let Some(t) = si.elems().find(|e| e.local() == "t") {
        if s.is_empty() {
            s = t.text();
        }
    }
    s
}

/// Delade strängar: dokumentet, prefixet och sträng → index.
struct Sst {
    doc: XDoc,
    p: String,
    index: HashMap<String, usize>,
    count: usize,
    added: usize,
    changed: bool,
}

impl Sst {
    fn new(doc: XDoc) -> Sst {
        let p = doc.prefix_for(NS_S).unwrap_or_default();
        let mut index = HashMap::new();
        let mut count = 0;
        for si in doc.root.elems().filter(|e| e.local() == "si") {
            index.entry(si_text(si)).or_insert(count);
            count += 1;
        }
        Sst { doc, p, index, count, added: 0, changed: false }
    }

    fn id(&mut self, s: &str) -> Result<usize, String> {
        if let Some(i) = self.index.get(s) {
            self.added += 1;
            return Ok(*i);
        }
        let space = if s.trim() != s || s.contains('\n') { " xml:space=\"preserve\"" } else { "" };
        let el = frag(&self.doc, &format!("<{si}><{t}{space}>{}</{t}></{si}>", escape(s), si = q(&self.p, "si"), t = q(&self.p, "t")))?;
        self.doc.root.children.push(XNode::Elem(el));
        self.index.insert(s.to_owned(), self.count);
        self.count += 1;
        self.added += 1;
        self.changed = true;
        Ok(self.count - 1)
    }

    fn finish(&mut self) {
        if !self.changed {
            return;
        }
        self.doc.root.set_attr("uniqueCount", &self.count.to_string());
        let c = self.doc.root.attr_i64("count").unwrap_or(0).max(0) as usize + self.added;
        self.doc.root.set_attr("count", &c.max(self.count).to_string());
    }
}

/// Cellens kolumn och rad ur `r` (`B2`).
fn addr(c: &XElem) -> Option<(u32, u32)> {
    super::edit_chart::cell(&c.attr("r")?)
}

/// Skriver cellerna i ett blad. Ger `true` om någon cell hade en formel.
fn write_sheet(doc: &mut XDoc, writes: &[&Write], sst: &mut Option<Sst>) -> Result<bool, String> {
    let p = doc.prefix_for(NS_S).unwrap_or_default();
    let mut had_formula = false;
    let mut extent: Option<(u32, u32, u32, u32)> = None;
    for w in writes {
        // Strängens index först (sharedStrings är ett annat dokument).
        let s_id = match (&w.val, sst.as_mut()) {
            (CellVal::Str(s), Some(sst)) => Some(sst.id(s)?),
            _ => None,
        };
        let r_txt = format!("{}{}", col_name(w.col), w.row);
        let row_el = frag(doc, &format!("<{} r=\"{}\"/>", q(&p, "row"), w.row))?;
        let cell_el = frag(doc, &format!("<{} r=\"{r_txt}\"/>", q(&p, "c")))?;
        let v_xml = match (&w.val, s_id) {
            (CellVal::Num(v), _) => Some(format!("<{v}>{}</{v}>", num_text(*v), v = q(&p, "v"))),
            (CellVal::Str(_), Some(i)) => Some(format!("<{v}>{i}</{v}>", v = q(&p, "v"))),
            (CellVal::Str(s), None) => Some(format!("<{is}><{t}>{}</{t}></{is}>", escape(s), is = q(&p, "is"), t = q(&p, "t"))),
            (CellVal::Empty, _) => None,
        };
        let v_el = match &v_xml {
            Some(x) => Some(frag(doc, x)?),
            None => None,
        };
        let data = doc.root.elems_mut().find(|e| e.local() == "sheetData").ok_or("sheetData saknas")?;
        // Raden (i ordning).
        let ri = match data.children.iter().position(|n| matches!(n, XNode::Elem(e) if e.local() == "row" && e.attr_i64("r") == Some(w.row as i64))) {
            Some(i) => i,
            None => {
                if matches!(w.val, CellVal::Empty) {
                    continue;
                }
                let at = data.children.iter().position(|n| matches!(n, XNode::Elem(e) if e.local() == "row" && e.attr_i64("r").unwrap_or(0) > w.row as i64)).unwrap_or(data.children.len());
                data.children.insert(at, XNode::Elem(row_el));
                at
            }
        };
        let XNode::Elem(row) = &mut data.children[ri] else { continue };
        if row.elems().any(|c| c.local() == "c" && c.attr("r").is_none()) {
            return Err("celler utan adress stöds inte".into());
        }
        row.remove_attr("spans");
        let ci = match row.children.iter().position(|n| matches!(n, XNode::Elem(e) if e.local() == "c" && addr(e).map(|a| a.0) == Some(w.col))) {
            Some(i) => i,
            None => {
                if matches!(w.val, CellVal::Empty) {
                    continue;
                }
                let at = row.children.iter().position(|n| matches!(n, XNode::Elem(e) if e.local() == "c" && addr(e).is_some_and(|a| a.0 > w.col))).unwrap_or(row.children.len());
                row.children.insert(at, XNode::Elem(cell_el));
                at
            }
        };
        let XNode::Elem(c) = &mut row.children[ci] else { continue };
        if c.elems().any(|e| e.local() == "f") {
            had_formula = true;
        }
        c.children.retain(|n| !matches!(n, XNode::Elem(e) if matches!(e.local(), "f" | "v" | "is")));
        match (&w.val, s_id) {
            (CellVal::Num(_), _) => c.remove_attr("t"),
            (CellVal::Str(_), Some(_)) => c.set_attr("t", "s"),
            (CellVal::Str(_), None) => c.set_attr("t", "inlineStr"),
            (CellVal::Empty, _) => c.remove_attr("t"),
        }
        // `v`/`is` före `extLst`.
        if let Some(v) = v_el {
            let at = c.children.iter().position(|n| matches!(n, XNode::Elem(e) if e.local() == "extLst")).unwrap_or(c.children.len());
            c.children.insert(at, XNode::Elem(v));
        } else if c.children.is_empty() {
            c.set_text("");
        }
        if !matches!(w.val, CellVal::Empty) {
            extent = Some(match extent {
                None => (w.col, w.row, w.col, w.row),
                Some((a, b, cc, d)) => (a.min(w.col), b.min(w.row), cc.max(w.col), d.max(w.row)),
            });
        }
    }
    // Bladets omfång växer med de skrivna cellerna.
    if let Some((c0, r0, c1, r1)) = extent {
        if let Some(dim) = doc.root.elems_mut().find(|e| e.local() == "dimension") {
            let cur = dim.attr("ref").unwrap_or_default();
            let (a, b) = cur.split_once(':').unwrap_or((cur.as_str(), cur.as_str()));
            if let (Some((x0, y0)), Some((x1, y1))) = (super::edit_chart::cell(a), super::edit_chart::cell(b)) {
                let n = (x0.min(c0), y0.min(r0), x1.max(c1), y1.max(r1));
                dim.set_attr("ref", &format!("{}{}:{}{}", col_name(n.0), n.1, col_name(n.2), n.3));
            }
        }
    }
    Ok(had_formula)
}

/// Tabellerna i bladet: området växer med skrivna celler intill, kolumnnamnen följer rubrikcellerna.
fn write_table(doc: &mut XDoc, writes: &[&Write]) -> Result<bool, String> {
    let p = doc.prefix_for(NS_S).unwrap_or_default();
    let Some(r) = doc.root.attr("ref") else { return Ok(false) };
    let (a, b) = r.split_once(':').unwrap_or((r.as_str(), r.as_str()));
    let (Some((c0, r0)), Some((mut c1, mut r1))) = (super::edit_chart::cell(a), super::edit_chart::cell(b)) else { return Ok(false) };
    let header = doc.root.attr_i64("headerRowCount").unwrap_or(1) > 0;
    let mut changed = false;
    // Växer: skrivna värden precis till höger om eller under området (inom det andra ledet).
    let filled: Vec<&&Write> = writes.iter().filter(|w| !matches!(w.val, CellVal::Empty)).collect();
    while filled.iter().any(|w| w.col == c1 + 1 && (r0..=r1.max(r0)).contains(&w.row)) {
        c1 += 1;
        changed = true;
    }
    while filled.iter().any(|w| w.row == r1 + 1 && (c0..=c1).contains(&w.col)) {
        r1 += 1;
        changed = true;
    }
    let ncols = (c1 - c0 + 1) as usize;
    // Kolumnnamn: rubrikcellerna som skrivs.
    let names: HashMap<usize, String> = if header {
        writes.iter().filter(|w| w.row == r0 && (c0..=c1).contains(&w.col)).filter_map(|w| match &w.val {
            CellVal::Str(s) => Some(((w.col - c0) as usize, s.clone())),
            CellVal::Num(v) => Some(((w.col - c0) as usize, num_text(*v))),
            CellVal::Empty => None,
        }).collect()
    } else {
        HashMap::new()
    };
    let cols_el = doc.root.elems().find(|e| e.local() == "tableColumns").cloned();
    let Some(mut cols) = cols_el else { return Ok(false) };
    let mut existing: Vec<String> = cols.elems().filter(|e| e.local() == "tableColumn").map(|e| e.attr("name").unwrap_or_default()).collect();
    let mut max_id = cols.elems().filter_map(|e| e.attr_i64("id")).max().unwrap_or(0);
    for i in existing.len()..ncols {
        max_id += 1;
        let mut name = names.get(&i).cloned().unwrap_or_else(|| format!("Kolumn{}", i + 1));
        let mut k = 2;
        while existing.contains(&name) {
            name = format!("{}{k}", names.get(&i).cloned().unwrap_or_else(|| "Kolumn".into()));
            k += 1;
        }
        let el = frag(doc, &format!("<{} id=\"{max_id}\" name=\"{}\"/>", q(&p, "tableColumn"), escape(&name)))?;
        cols.children.push(XNode::Elem(el));
        existing.push(name);
        changed = true;
    }
    for (i, e) in cols.elems_mut().filter(|e| e.local() == "tableColumn").enumerate() {
        if let Some(n) = names.get(&i) {
            if e.attr("name").as_deref() != Some(n.as_str()) {
                e.set_attr("name", n);
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(false);
    }
    let count = cols.elems().filter(|e| e.local() == "tableColumn").count();
    cols.set_attr("count", &count.to_string());
    let new_ref = format!("{}{}:{}{}", col_name(c0), r0, col_name(c1), r1);
    doc.root.set_attr("ref", &new_ref);
    for n in doc.root.children.iter_mut() {
        if let XNode::Elem(e) = n {
            if e.local() == "tableColumns" {
                *e = cols.clone();
            } else if e.local() == "autoFilter" {
                e.set_attr("ref", &new_ref);
            }
        }
    }
    Ok(true)
}

/// Skriver cellerna i arbetsboken (zip-byte) och ger den nya arbetsboken.
pub fn edit(xlsx: &[u8], writes: &[Write]) -> Result<Vec<u8>, String> {
    if writes.is_empty() {
        return Ok(xlsx.to_vec());
    }
    let mut zin = zip::ZipArchive::new(Cursor::new(xlsx)).map_err(|e| e.to_string())?;
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut order = Vec::new();
    for i in 0..zin.len() {
        let mut f = zin.by_index(i).map_err(|e| e.to_string())?;
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(|e| e.to_string())?;
        order.push(f.name().to_owned());
        entries.insert(f.name().to_owned(), b);
    }
    let wb = rels_of(&entries, "").into_iter().find(|r| r.1.ends_with("/officeDocument")).map(|r| r.2).unwrap_or_else(|| "xl/workbook.xml".into());
    let wb_doc = edit_xml::parse(entries.get(&wb).ok_or("arbetsboken saknar workbook.xml")?).ok_or("workbook.xml kan inte tolkas")?;
    let wb_rels = rels_of(&entries, &wb);
    let sheets: Vec<(String, String)> = wb_doc
        .root
        .elems()
        .filter(|e| e.local() == "sheets")
        .flat_map(|s| s.elems().filter(|e| e.local() == "sheet"))
        .filter_map(|e| Some((e.attr("name")?, wb_rels.iter().find(|r| Some(&r.0) == e.attr("r:id").as_ref())?.2.clone())))
        .collect();
    let sst_part = wb_rels.iter().find(|r| r.1.ends_with("/sharedStrings")).map(|r| r.2.clone());
    let mut sst = match &sst_part {
        Some(p) => entries.get(p).and_then(|b| edit_xml::parse(b)).map(Sst::new),
        None => None,
    };
    let mut changed: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut drop_calc = false;
    // Bladen (i bladens ordning; okända blad hoppas över).
    let mut by_sheet: BTreeMap<String, Vec<&Write>> = BTreeMap::new();
    for w in writes {
        by_sheet.entry(w.sheet.to_lowercase()).or_default().push(w);
    }
    for (name, ws) in &by_sheet {
        let Some((_, part)) = sheets.iter().find(|(n, _)| n.to_lowercase() == *name) else { continue };
        let Some(mut doc) = entries.get(part).and_then(|b| edit_xml::parse(b)) else { continue };
        drop_calc |= write_sheet(&mut doc, ws, &mut sst)?;
        changed.insert(part.clone(), doc.to_bytes());
        // Tabellerna i bladet.
        for (_, ty, t) in rels_of(&entries, part) {
            if !ty.ends_with("/table") {
                continue;
            }
            if let Some(mut td) = entries.get(&t).and_then(|b| edit_xml::parse(b)) {
                if write_table(&mut td, ws)? {
                    changed.insert(t, td.to_bytes());
                }
            }
        }
    }
    if let (Some(s), Some(p)) = (sst.as_mut(), &sst_part) {
        s.finish();
        if s.changed {
            changed.insert(p.clone(), s.doc.to_bytes());
        }
    }
    // Formler som blev värden: beräkningskedjan byggs om av Excel.
    let mut removed: Vec<String> = Vec::new();
    if drop_calc {
        if let Some(cc) = wb_rels.iter().find(|r| r.1.ends_with("/calcChain")) {
            removed.push(cc.2.clone());
            let rels = super::edit::rels_name(&wb);
            if let Some(mut d) = entries.get(&rels).and_then(|b| edit_xml::parse(b)) {
                d.root.children.retain(|n| !matches!(n, XNode::Elem(e) if e.local() == "Relationship" && e.attr("Id").as_deref() == Some(cc.0.as_str())));
                changed.insert(rels, d.to_bytes());
            }
            if let Some(mut d) = entries.get("[Content_Types].xml").and_then(|b| edit_xml::parse(b)) {
                let pn = format!("/{}", cc.2);
                d.root.children.retain(|n| !matches!(n, XNode::Elem(e) if e.local() == "Override" && e.attr("PartName").is_some_and(|x| x.eq_ignore_ascii_case(&pn))));
                changed.insert("[Content_Types].xml".into(), d.to_bytes());
            }
        }
    }
    // Ny zip: ändrade poster skrivs, övriga kopieras som de är.
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6));
    for (i, name) in order.iter().enumerate() {
        if removed.contains(name) {
            continue;
        }
        match changed.get(name) {
            Some(b) => {
                out.start_file(name.as_str(), opts).map_err(|e| e.to_string())?;
                out.write_all(b).map_err(|e| e.to_string())?;
            }
            None => {
                let f = zin.by_index_raw(i).map_err(|e| e.to_string())?;
                out.raw_copy_file(f).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(out.finish().map_err(|e| e.to_string())?.into_inner())
}

/// Läser en cells värde (för proven): tal, delad sträng eller `inlineStr`.
#[cfg(test)]
pub fn read_cell(xlsx: &[u8], sheet: &str, col: u32, row: u32) -> Option<CellVal> {
    let mut z = zip::ZipArchive::new(Cursor::new(xlsx)).ok()?;
    let mut entries = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).ok()?;
        let mut b = Vec::new();
        f.read_to_end(&mut b).ok()?;
        entries.insert(f.name().to_owned(), b);
    }
    let wb = "xl/workbook.xml";
    let wb_doc = edit_xml::parse(entries.get(wb)?)?;
    let wb_rels = rels_of(&entries, wb);
    let rid = wb_doc.root.elems().filter(|e| e.local() == "sheets").flat_map(|s| s.elems()).find(|e| e.attr("name").as_deref() == Some(sheet))?.attr("r:id")?;
    let part = wb_rels.iter().find(|r| r.0 == rid)?.2.clone();
    let doc = edit_xml::parse(entries.get(&part)?)?;
    let data = doc.root.elems().find(|e| e.local() == "sheetData")?;
    let row = data.elems().find(|e| e.attr_i64("r") == Some(row as i64))?;
    let c = row.elems().find(|e| addr(e).map(|a| a.0) == Some(col))?;
    match c.attr("t").as_deref() {
        Some("s") => {
            let i: usize = c.elems().find(|e| e.local() == "v")?.text().parse().ok()?;
            let sp = wb_rels.iter().find(|r| r.1.ends_with("/sharedStrings"))?.2.clone();
            let sd = edit_xml::parse(entries.get(&sp)?)?;
            Some(CellVal::Str(si_text(sd.root.elems().filter(|e| e.local() == "si").nth(i)?)))
        }
        Some("inlineStr") => Some(CellVal::Str(si_text(c.elems().find(|e| e.local() == "is")?))),
        Some("str") => Some(CellVal::Str(c.elems().find(|e| e.local() == "v")?.text())),
        _ => match c.elems().find(|e| e.local() == "v") {
            Some(v) => v.text().parse().ok().map(CellVal::Num),
            None => Some(CellVal::Empty),
        },
    }
}
