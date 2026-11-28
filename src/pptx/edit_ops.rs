//! Redigeringens operationer på former och bilder: läge och storlek, ta bort, duplicera, ordning,
//! infoga textruta och bild; ny bild med layout, ta bort, flytta, duplicera och dölja bilder; anteckningar.
//!
//! Former adresseras med sitt läge bland bildens toppnivåformer i ritmodellen (där `mc:AlternateContent`
//! redan är upplöst) och `p:cNvPr/@id` som kontroll. I den förlustfria XML:en motsvaras läget av ett
//! barn till `p:spTree`, eller ett element i den valda grenen av ett `mc:AlternateContent` (då ändras
//! formen med samma id i alla grenar).

use std::collections::{HashMap, HashSet};

use super::edit::{self, ct, Txn, NS_A, NS_P, NS_R};
use super::edit_xml::{self, XDoc, XElem, XNode};
use super::inherit::{self, Owner, Sheets};
use super::model::Deck;
use super::render::emu;
use super::scene::Affine;
use super::xml::Node;
use crate::i18n::t;

/// En forms placering i EMU (`a:xfrm`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Xf {
    pub x: i64,
    pub y: i64,
    pub cx: i64,
    pub cy: i64,
    /// Vridning i 60 000-dels grader.
    pub rot: i64,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Xf {
    fn of(x: &Node) -> Option<Xf> {
        let off = x.child("a:off")?;
        let ext = x.child("a:ext")?;
        Some(Xf {
            x: off.attr_i64("x").unwrap_or(0),
            y: off.attr_i64("y").unwrap_or(0),
            cx: ext.attr_i64("cx").unwrap_or(0),
            cy: ext.attr_i64("cy").unwrap_or(0),
            rot: x.attr_i64("rot").unwrap_or(0),
            flip_h: x.attr_bool("flipH").unwrap_or(false),
            flip_v: x.attr_bool("flipV").unwrap_or(false),
        })
    }

    /// Rektangeln i punkter (utan vridning).
    pub fn rect_pt(&self) -> [f32; 4] {
        [emu(self.x), emu(self.y), emu(self.cx), emu(self.cy)]
    }

    pub fn rot_deg(&self) -> f32 {
        self.rot as f32 / 60000.0
    }
}

/// Vad en toppnivåform är.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shape,
    Connector,
    Picture,
    Group,
    /// Tabell, diagram, SmartArt eller inbäddat objekt.
    Frame,
    Other,
}

/// En toppnivåform på en bild (för träffprov, markering och redigering).
#[derive(Clone, Debug)]
pub struct ShapeInfo {
    /// Läget bland `p:spTree`:s barn i ritmodellen.
    pub index: usize,
    pub id: u32,
    /// `p:cNvPr/@name` (används av proven).
    #[cfg_attr(not(test), allow(dead_code))]
    pub name: String,
    pub kind: Kind,
    pub xf: Xf,
    /// Platshållartyp (`title`, `body` …) om formen är en platshållare.
    pub ph: Option<String>,
    /// Har formen text (eller kan få text: `p:sp`)?
    pub text: bool,
    /// Tom platshållare (ritas inte; i redigeringsläget visas ledtexten).
    pub empty_ph: bool,
    /// Formen ligger i ett `mc:AlternateContent` (text kan inte redigeras).
    pub alternate: bool,
}

impl ShapeInfo {
    /// Formens rektangel → bilden (punkter), med vridning och spegling.
    pub fn affine(&self) -> Affine {
        let [x, y, w, h] = self.xf.rect_pt();
        let mut t = Affine::IDENTITY;
        if self.xf.rot != 0 {
            t = t.then(&Affine::rotate_at(self.xf.rot_deg(), w / 2.0, h / 2.0));
        }
        t.then(&Affine::translate(x, y))
    }

    /// Ligger punkten (bildens punkter) i formen?
    pub fn hit(&self, px: f32, py: f32, slack: f32) -> bool {
        let Some(inv) = self.affine().invert() else { return false };
        let (lx, ly) = inv.apply(px, py);
        let [_, _, w, h] = self.xf.rect_pt();
        lx >= -slack && ly >= -slack && lx <= w + slack && ly <= h + slack
    }
}

fn kind_of(n: &Node) -> Kind {
    match n.name.as_str() {
        "p:sp" => Kind::Shape,
        "p:cxnSp" => Kind::Connector,
        "p:pic" => Kind::Picture,
        "p:grpSp" => Kind::Group,
        "p:graphicFrame" => Kind::Frame,
        _ => Kind::Other,
    }
}

fn nv_cnvpr(n: &Node) -> Option<&Node> {
    n.children.first().and_then(|nv| nv.child("p:cNvPr"))
}

/// Formens placering, med arv från layoutens och mallens platshållare.
pub fn effective_xf(sheets: &Sheets<'_>, n: &Node) -> Option<Xf> {
    match n.name.as_str() {
        "p:grpSp" => n.path(&["p:grpSpPr", "a:xfrm"]).and_then(Xf::of),
        "p:graphicFrame" => n.child("p:xfrm").and_then(Xf::of),
        _ => {
            let chain = inherit::shape_chain(sheets, n, Owner::Slide);
            chain.iter().find_map(|s| s.path(&["p:spPr", "a:xfrm"])).and_then(Xf::of)
        }
    }
}

fn has_text(n: &Node) -> bool {
    n.child("p:txBody").is_some_and(|b| b.children_named("a:p").any(|p| p.children.iter().any(|r| matches!(r.name.as_str(), "a:r" | "a:fld") && r.child("a:t").is_some_and(|t| !t.text.is_empty()))))
}

/// Bildens toppnivåformer i ritordning (dolda former utelämnas).
pub fn shapes(deck: &Deck, slide: usize) -> Vec<ShapeInfo> {
    let Some(s) = deck.slides.get(slide) else { return Vec::new() };
    let Some(tree) = s.root.path(&["p:cSld", "p:spTree"]) else { return Vec::new() };
    let alt = alternate_indices(deck, slide);
    let sheets = Sheets::for_slide(deck, slide);
    let mut out = Vec::new();
    for (index, c) in tree.children.iter().enumerate() {
        let kind = kind_of(c);
        if kind == Kind::Other {
            continue;
        }
        let Some(cnv) = nv_cnvpr(c) else { continue };
        if cnv.attr_bool("hidden") == Some(true) {
            continue;
        }
        let Some(xf) = effective_xf(&sheets, c) else { continue };
        let ph = inherit::placeholder(c).map(|p| p.typ);
        let text = has_text(c);
        out.push(ShapeInfo {
            index,
            id: cnv.attr_i64("id").unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
            name: cnv.attr("name").unwrap_or("").to_owned(),
            kind,
            xf,
            empty_ph: ph.is_some() && !text && kind == Kind::Shape,
            ph,
            text: text || kind == Kind::Shape,
            alternate: alt.contains(&index),
        });
    }
    out
}

/// Lägen i ritmodellen som kommer ur ett `mc:AlternateContent`.
fn alternate_indices(deck: &Deck, slide: usize) -> HashSet<usize> {
    let mut out = HashSet::new();
    let Some(bytes) = deck.slides.get(slide).and_then(|s| deck.pkg.read(&s.part)) else { return out };
    let Some(doc) = edit_xml::parse(bytes) else { return out };
    let scope = doc.root_scope();
    let Some(tree) = doc.root.path(&["p:cSld", "p:spTree"]) else { return out };
    let mut n = 0;
    for e in tree.elems() {
        if is_ac(e) {
            let k = ac_branch(e, &scope).map_or(0, |b| b.elems().count());
            out.extend(n..n + k);
            n += k;
        } else {
            n += 1;
        }
    }
    out
}

fn is_ac(e: &XElem) -> bool {
    e.name == "mc:AlternateContent"
}

/// Grenen som ritmodellen valde (samma regel som `xml.rs`: `mc:Choice` bara för p14/p15).
fn ac_branch<'e>(ac: &'e XElem, scope: &[(String, String)]) -> Option<&'e XElem> {
    const OK: [&str; 2] = ["http://schemas.microsoft.com/office/powerpoint/2010/main", "http://schemas.microsoft.com/office/powerpoint/2012/main"];
    let decls = |e: &XElem| -> Vec<(String, String)> {
        e.attrs.iter().filter_map(|a| a.qname.strip_prefix("xmlns:").map(|p| (p.to_owned(), edit_xml::unescape(&a.raw)))).collect()
    };
    let mut sc: Vec<(String, String)> = scope.to_vec();
    sc.extend(decls(ac));
    for c in ac.elems() {
        match c.local() {
            "Choice" => {
                let mut s2 = sc.clone();
                s2.extend(decls(c));
                let req = c.attr("Requires").unwrap_or_default();
                let ok = req.split_whitespace().all(|p| s2.iter().rev().find(|(q, _)| q == p).is_some_and(|(_, u)| OK.contains(&u.as_str())));
                if ok {
                    return Some(c);
                }
            }
            "Fallback" => return Some(c),
            _ => {}
        }
    }
    None
}

/// Var i `p:spTree` (förlustfritt) formen med läget `index` i ritmodellen finns: barnets index i
/// `children` och, i ett `mc:AlternateContent`, elementets index i den valda grenen.
fn locate(tree: &XElem, scope: &[(String, String)], index: usize) -> Option<(usize, Option<usize>)> {
    let mut n = 0;
    for (ci, c) in tree.children.iter().enumerate() {
        let XNode::Elem(e) = c else { continue };
        if is_ac(e) {
            let k = ac_branch(e, scope).map_or(0, |b| b.elems().count());
            if index < n + k {
                return Some((ci, Some(index - n)));
            }
            n += k;
        } else {
            if index == n {
                return Some((ci, None));
            }
            n += 1;
        }
    }
    None
}

fn elem_id(e: &XElem) -> Option<i64> {
    e.elems().next().and_then(|nv| nv.child("p:cNvPr")).and_then(|c| c.attr_i64("id"))
}

fn tree_mut(doc: &mut XDoc) -> Result<&mut XElem, String> {
    doc.root.path_mut(&["p:cSld", "p:spTree"]).ok_or_else(|| t("Bilden saknar formträd.").to_owned())
}

/// Kör `f` på formen (alla grenar om den ligger i ett `mc:AlternateContent`).
pub(super) fn with_shape(txn: &mut Txn, slide: usize, index: usize, id: u32, f: &mut dyn FnMut(&mut XElem, &XDoc) -> Result<(), String>) -> Result<(), String> {
    let part = slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    let scope = doc.root_scope();
    let snapshot = doc.clone();
    let tree = tree_mut(doc)?;
    let (ci, branch) = locate(tree, &scope, index).ok_or_else(|| t("Formen finns inte längre.").to_owned())?;
    let XNode::Elem(top) = &mut tree.children[ci] else { return Err(t("Formen finns inte längre.").to_owned()) };
    match branch {
        None => {
            if elem_id(top) != Some(id as i64) {
                return Err(t("Formen finns inte längre.").to_owned());
            }
            f(top, &snapshot)
        }
        Some(_) => {
            let mut found = false;
            for b in top.elems_mut() {
                for e in b.elems_mut() {
                    if elem_id(e) == Some(id as i64) {
                        f(e, &snapshot)?;
                        found = true;
                    }
                }
            }
            if found { Ok(()) } else { Err(t("Formen finns inte längre.").to_owned()) }
        }
    }
}

pub fn slide_part(deck: &Deck, slide: usize) -> Result<String, String> {
    deck.slides.get(slide).map(|s| s.part.clone()).ok_or_else(|| t("Bilden finns inte längre.").to_owned())
}

/// Prefixet för en namnrymd i dokumentet (läggs till på roten om det saknas).
fn pfx(doc: &mut XDoc, uri: &str, want: &str) -> String {
    doc.ensure_prefix(uri, want)
}

// -------------------------------------------------------------------- former

/// Sätter formens läge, storlek och vridning. `xf` är den nya placeringen; saknar formen egen
/// `a:xfrm` (ärvd från layouten) skapas en.
pub fn set_xf(txn: &mut Txn, slide: usize, index: usize, id: u32, xf: Xf) -> Result<(), String> {
    // DrawingML-prefixet kan vara deklarerat bara på enskilda element: deklarera det på roten.
    let part = slide_part(txn.deck, slide)?;
    txn.doc(&part)?.ensure_prefix(NS_A, "a");
    with_shape(txn, slide, index, id, &mut |e, doc| write_xf(e, doc, xf))
}

pub(super) fn write_xf(e: &mut XElem, doc: &XDoc, xf: Xf) -> Result<(), String> {
    let a = doc.prefix_for(NS_A).unwrap_or_else(|| "a".into());
    let p = doc.prefix_for(NS_P).unwrap_or_else(|| "p".into());
    let q = |pre: &str, l: &str| if pre.is_empty() { l.to_owned() } else { format!("{pre}:{l}") };
    let local = e.local().to_owned();
    let is_group = local == "grpSp";
    let xfrm: &mut XElem = if local == "graphicFrame" {
        if e.child("p:xfrm").is_none() {
            let el = doc_fragment(doc, &format!("<{}><{}/><{}/></{}>", q(&p, "xfrm"), q(&a, "off"), q(&a, "ext"), q(&p, "xfrm")))?;
            let at = e.index_of("a:graphic").unwrap_or(e.children.len());
            e.children.insert(at, XNode::Elem(el));
        }
        e.child_mut("p:xfrm").ok_or("xfrm")?
    } else {
        let pr_name = if is_group { "p:grpSpPr" } else { "p:spPr" };
        if e.child(pr_name).is_none() {
            let el = doc_fragment(doc, &format!("<{}/>", q(&p, if is_group { "grpSpPr" } else { "spPr" })))?;
            let at = 1.min(e.children.len());
            let at = e.children.iter().position(|c| matches!(c, XNode::Elem(x) if x.local().starts_with("nv"))).map_or(at, |i| i + 1);
            e.children.insert(at, XNode::Elem(el));
        }
        let pr = e.child_mut(pr_name).ok_or("spPr")?;
        if pr.child("a:xfrm").is_none() {
            let el = doc_fragment(doc, &format!("<{}><{}/><{}/></{}>", q(&a, "xfrm"), q(&a, "off"), q(&a, "ext"), q(&a, "xfrm")))?;
            // `a:xfrm` är första barnet i `spPr`.
            pr.children.insert(0, XNode::Elem(el));
        }
        pr.child_mut("a:xfrm").ok_or("xfrm")?
    };
    if is_group && xfrm.child("a:chOff").is_none() {
        // Utan barnkoordinater skulle barnen inte följa med; de blir gruppens gamla läge och storlek.
        let off = xfrm.child("a:off").map(|o| (o.attr_i64("x").unwrap_or(0), o.attr_i64("y").unwrap_or(0))).unwrap_or((0, 0));
        let ext = xfrm.child("a:ext").map(|o| (o.attr_i64("cx").unwrap_or(0), o.attr_i64("cy").unwrap_or(0))).unwrap_or((0, 0));
        let ch = doc_fragment(doc, &format!("<{} x=\"{}\" y=\"{}\"/>", q(&a, "chOff"), off.0, off.1))?;
        let ce = doc_fragment(doc, &format!("<{} cx=\"{}\" cy=\"{}\"/>", q(&a, "chExt"), ext.0, ext.1))?;
        xfrm.children.push(XNode::Elem(ch));
        xfrm.children.push(XNode::Elem(ce));
    }
    if xf.rot == 0 {
        xfrm.remove_attr("rot");
    } else if local != "graphicFrame" {
        xfrm.set_attr("rot", &xf.rot.rem_euclid(21_600_000).to_string());
    }
    let off = xfrm.child_mut("a:off").ok_or("off")?;
    off.set_attr("x", &xf.x.to_string());
    off.set_attr("y", &xf.y.to_string());
    let ext = xfrm.child_mut("a:ext").ok_or("ext")?;
    ext.set_attr("cx", &xf.cx.max(0).to_string());
    ext.set_attr("cy", &xf.cy.max(0).to_string());
    Ok(())
}

pub(super) fn doc_fragment(doc: &XDoc, xml: &str) -> Result<XElem, String> {
    doc.fragment(xml).into_iter().next().ok_or_else(|| format!("ogiltig XML: {xml}"))
}

/// Tar bort formen och det som pekar på den (animeringar, kopplingar).
pub fn delete_shape(txn: &mut Txn, slide: usize, index: usize, id: u32) -> Result<(), String> {
    let part = slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    let scope = doc.root_scope();
    let tree = tree_mut(doc)?;
    let (ci, branch) = locate(tree, &scope, index).ok_or_else(|| t("Formen finns inte längre.").to_owned())?;
    let XNode::Elem(top) = &tree.children[ci] else { return Err(t("Formen finns inte längre.").to_owned()) };
    if branch.is_none() && elem_id(top) != Some(id as i64) {
        return Err(t("Formen finns inte längre.").to_owned());
    }
    // Id:n för formen och dess barn (grupper): animeringar och kopplingar som pekar på dem tas bort.
    let mut ids: HashSet<String> = HashSet::new();
    top.walk(&mut |e| {
        if e.name == "p:cNvPr" {
            if let Some(i) = e.attr("id") {
                ids.insert(i);
            }
        }
    });
    tree.children.remove(ci);
    // Kopplingar som slutar i formen.
    tree.walk_mut(&mut |e| {
        if e.name == "p:cNvCxnSpPr" {
            e.children.retain(|c| !matches!(c, XNode::Elem(x) if (x.name == "a:stCxn" || x.name == "a:endCxn") && x.attr("id").is_some_and(|i| ids.contains(&i))));
        }
    });
    remove_timing_refs(&mut doc.root, &ids);
    Ok(())
}

/// Tar bort animeringar som riktar sig mot formerna (`p:spTgt/@spid`) och `p:bldP` för dem. Finns
/// referenser kvar (t.ex. utlösare) tas hela tidsträdet bort hellre än att lämna en ogiltig referens.
fn remove_timing_refs(root: &mut XElem, ids: &HashSet<String>) {
    let Some(ti) = root.index_of("p:timing") else { return };
    let refers = |e: &XElem| -> bool {
        let mut hit = false;
        e.walk(&mut |x| {
            if (x.name == "p:spTgt" || x.name == "p:bldP" || x.name == "p:bldGraphic" || x.name == "p:bldOleChart" || x.name == "p:bldDgm") && x.attr("spid").is_some_and(|s| ids.contains(&s)) {
                hit = true;
            }
        });
        hit
    };
    let XNode::Elem(timing) = &mut root.children[ti] else { return };
    if !refers(timing) {
        return;
    }
    // Effekterna (`p:par` vars `p:cTn` har `presetClass`) som rör formerna.
    fn prune(e: &mut XElem, ids: &HashSet<String>, refers: &dyn Fn(&XElem) -> bool) {
        e.children.retain(|c| match c {
            XNode::Elem(x) if x.name == "p:par" && x.child("p:cTn").is_some_and(|c| c.attr("presetClass").is_some()) => !refers(x),
            XNode::Elem(x) if matches!(x.name.as_str(), "p:bldP" | "p:bldGraphic" | "p:bldOleChart" | "p:bldDgm") => !x.attr("spid").is_some_and(|s| ids.contains(&s)),
            _ => true,
        });
        for c in e.elems_mut() {
            prune(c, ids, refers);
        }
        // Tomma grupper (en `p:par` vars `childTnLst` blev tom) tas bort.
        e.children.retain(|c| match c {
            XNode::Elem(x) if x.name == "p:par" => x.path(&["p:cTn", "p:childTnLst"]).is_none_or(|l| l.elems().next().is_some()),
            _ => true,
        });
    }
    prune(timing, ids, &refers);
    if refers(timing) {
        root.children.remove(ti);
    }
}

/// Största `p:cNvPr/@id` i dokumentet.
fn max_id(root: &XElem) -> i64 {
    let mut m = 0;
    root.walk(&mut |e| {
        if e.name == "p:cNvPr" {
            m = m.max(e.attr_i64("id").unwrap_or(0));
        }
    });
    m
}

/// Ger alla `p:cNvPr` i elementet nya id:n (samma gamla id → samma nya, så att grenarna i ett
/// `mc:AlternateContent` förblir samma form) och tar bort `a16:creationId`.
fn renumber(e: &mut XElem, next: &mut i64, map: &mut HashMap<String, String>) {
    e.walk_mut(&mut |x| {
        if x.name == "p:cNvPr" {
            let old = x.attr("id").unwrap_or_default();
            let new = map.entry(old).or_insert_with(|| {
                *next += 1;
                next.to_string()
            });
            x.set_attr("id", new);
            if let Some(ext) = x.child_mut("a:extLst") {
                ext.children.retain(|c| !matches!(c, XNode::Elem(y) if y.find("a16:creationId").is_some()));
            }
        }
    });
}

/// Duplicerar formen (förskjuten 0,25 tum) och ger kopians läge i ritmodellen.
pub fn duplicate_shape(txn: &mut Txn, slide: usize, index: usize, id: u32) -> Result<usize, String> {
    let part = slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    let scope = doc.root_scope();
    let snapshot = doc.clone();
    let mut next = max_id(&doc.root);
    let tree = tree_mut(doc)?;
    let (ci, branch) = locate(tree, &scope, index).ok_or_else(|| t("Formen finns inte längre.").to_owned())?;
    let XNode::Elem(top) = &tree.children[ci] else { return Err(t("Formen finns inte längre.").to_owned()) };
    if branch.is_none() && elem_id(top) != Some(id as i64) {
        return Err(t("Formen finns inte längre.").to_owned());
    }
    // Diagram, SmartArt och inbäddade objekt har egna delar som skulle delas av två ramar.
    let mut shared = false;
    top.walk(&mut |e| {
        if e.name == "a:graphicData" && !e.attr("uri").is_some_and(|u| u.ends_with("/table")) {
            shared = true;
        }
    });
    if shared {
        return Err(t("Diagram, SmartArt och inbäddade objekt kan inte dupliceras än (duplicera hela bilden i stället).").to_owned());
    }
    let mut copy = top.clone();
    renumber(&mut copy, &mut next, &mut HashMap::new());
    const STEP: i64 = 228_600;
    let shift = |e: &mut XElem| {
        let xfrm = match e.local() {
            "graphicFrame" => e.child_mut("p:xfrm"),
            "grpSp" => e.path_mut(&["p:grpSpPr", "a:xfrm"]),
            _ => e.path_mut(&["p:spPr", "a:xfrm"]),
        };
        if let Some(off) = xfrm.and_then(|x| x.child_mut("a:off")) {
            let (x, y) = (off.attr_i64("x").unwrap_or(0), off.attr_i64("y").unwrap_or(0));
            off.set_attr("x", &(x + STEP).to_string());
            off.set_attr("y", &(y + STEP).to_string());
        }
    };
    if is_ac(&copy) {
        for b in copy.elems_mut() {
            for e in b.elems_mut() {
                shift(e);
            }
        }
    } else {
        shift(&mut copy);
    }
    let _ = snapshot;
    tree.children.insert(ci + 1, XNode::Elem(copy));
    // Kopian har samma antal element i ritmodellen som originalet: den kommer direkt efter.
    let n = if branch.is_some() {
        let XNode::Elem(e) = &tree.children[ci] else { unreachable!() };
        ac_branch(e, &scope).map_or(1, |b| b.elems().count())
    } else {
        1
    };
    Ok(index - branch.unwrap_or(0) + n)
}

/// Ordning: `Front`/`Back` längst fram/bak, `Forward`/`Backward` ett steg.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Order {
    Front,
    Forward,
    Backward,
    Back,
}

/// Flyttar formen i ritordningen.
pub fn reorder_shape(txn: &mut Txn, slide: usize, index: usize, id: u32, order: Order) -> Result<(), String> {
    let part = slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    let scope = doc.root_scope();
    let tree = tree_mut(doc)?;
    let (ci, branch) = locate(tree, &scope, index).ok_or_else(|| t("Formen finns inte längre.").to_owned())?;
    if let XNode::Elem(top) = &tree.children[ci] {
        if branch.is_none() && elem_id(top) != Some(id as i64) {
            return Err(t("Formen finns inte längre.").to_owned());
        }
    }
    // Formelementen (allt utom gruppens egenskaper och extLst).
    let slots: Vec<usize> = tree
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c, XNode::Elem(e) if !matches!(e.name.as_str(), "p:nvGrpSpPr" | "p:grpSpPr" | "p:extLst")))
        .map(|(i, _)| i)
        .collect();
    let Some(pos) = slots.iter().position(|&i| i == ci) else { return Ok(()) };
    let target = match order {
        Order::Front => slots.len() - 1,
        Order::Back => 0,
        Order::Forward => (pos + 1).min(slots.len() - 1),
        Order::Backward => pos.saturating_sub(1),
    };
    if target == pos {
        return Ok(());
    }
    let node = tree.children.remove(ci);
    // Indexen efter borttagningen: platsen för målslotten.
    let dest = if target > pos { slots[target] } else { slots[target] };
    tree.children.insert(dest, node);
    Ok(())
}

/// En ny textruta (tom) med rektangeln i EMU; ger dess läge i ritmodellen.
pub fn insert_textbox(txn: &mut Txn, slide: usize, xf: Xf) -> Result<usize, String> {
    let part = slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    let (p, a) = (pfx(doc, NS_P, "p"), pfx(doc, NS_A, "a"));
    let id = max_id(&doc.root) + 1;
    let xml = format!(
        r#"<{p}:sp><{p}:nvSpPr><{p}:cNvPr id="{id}" name="{name}"/><{p}:cNvSpPr txBox="1"/><{p}:nvPr/></{p}:nvSpPr><{p}:spPr><{a}:xfrm><{a}:off x="{x}" y="{y}"/><{a}:ext cx="{cx}" cy="{cy}"/></{a}:xfrm><{a}:prstGeom prst="rect"><{a}:avLst/></{a}:prstGeom><{a}:noFill/></{p}:spPr><{p}:txBody><{a}:bodyPr wrap="square" rtlCol="0"><{a}:spAutoFit/></{a}:bodyPr><{a}:lstStyle/><{a}:p/></{p}:txBody></{p}:sp>"#,
        name = edit_xml::escape(&crate::tf!("Textruta {n}", n = id - 1)),
        x = xf.x,
        y = xf.y,
        cx = xf.cx,
        cy = xf.cy
    );
    append_shape(doc, &xml)
}

/// Lägger formen sist i formträdet (före `extLst`) och ger dess läge i ritmodellen.
fn append_shape(doc: &mut XDoc, xml: &str) -> Result<usize, String> {
    let el = doc_fragment(doc, xml)?;
    let scope = doc.root_scope();
    let tree = tree_mut(doc)?;
    let at = tree.index_of("p:extLst").unwrap_or(tree.children.len());
    tree.children.insert(at, XNode::Elem(el));
    // Läget i ritmodellen: antalet element före (med valda grenar för AlternateContent).
    let mut n = 0;
    for c in &tree.children[..at] {
        if let XNode::Elem(e) = c {
            n += if is_ac(e) { ac_branch(e, &scope).map_or(0, |b| b.elems().count()) } else { 1 };
        }
    }
    Ok(n)
}

/// Bildens innehållstyp och filändelse ur innehållet (PNG eller JPEG).
pub fn image_kind(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => Some(("png", "image/png")),
        image::ImageFormat::Jpeg => Some(("jpeg", "image/jpeg")),
        image::ImageFormat::Gif => Some(("gif", "image/gif")),
        _ => None,
    }
}

/// Infogar en bild (PNG/JPEG/GIF) mitt på bilden, högst 80 % av bildens storlek. Ger formens läge.
pub fn insert_picture(txn: &mut Txn, slide: usize, bytes: Vec<u8>, name: &str) -> Result<usize, String> {
    let (ext, mime) = image_kind(&bytes).ok_or_else(|| t("Bara PNG-, JPEG- och GIF-bilder kan infogas.").to_owned())?;
    let size = imagesize::blob_size(&bytes).map_err(|_| t("Bilden kan inte läsas.").to_owned())?;
    let part = slide_part(txn.deck, slide)?;
    let media = txn.unique_part("ppt/media/image", &format!(".{ext}"));
    txn.put_raw(&media, bytes);
    txn.ct_default(ext, mime)?;
    let rid = txn.add_rel(&part, "image", &media)?;
    let (sw, sh) = txn.deck.size_emu;
    // 96 dpi, nedskalad till 80 % av bilden.
    let (mut w, mut h) = (size.width as f64 * 9525.0, size.height as f64 * 9525.0);
    let k = (sw as f64 * 0.8 / w.max(1.0)).min(sh as f64 * 0.8 / h.max(1.0)).min(1.0);
    w *= k;
    h *= k;
    let (x, y) = (((sw as f64 - w) / 2.0) as i64, ((sh as f64 - h) / 2.0) as i64);
    let doc = txn.doc(&part)?;
    let (p, a, r) = (pfx(doc, NS_P, "p"), pfx(doc, NS_A, "a"), pfx(doc, NS_R, "r"));
    let id = max_id(&doc.root) + 1;
    let xml = format!(
        r#"<{p}:pic><{p}:nvPicPr><{p}:cNvPr id="{id}" name="{n}" descr="{n}"/><{p}:cNvPicPr><{a}:picLocks noChangeAspect="1"/></{p}:cNvPicPr><{p}:nvPr/></{p}:nvPicPr><{p}:blipFill><{a}:blip {r}:embed="{rid}"/><{a}:stretch><{a}:fillRect/></{a}:stretch></{p}:blipFill><{p}:spPr><{a}:xfrm><{a}:off x="{x}" y="{y}"/><{a}:ext cx="{w}" cy="{h}"/></{a}:xfrm><{a}:prstGeom prst="rect"><{a}:avLst/></{a}:prstGeom></{p}:spPr></{p}:pic>"#,
        n = edit_xml::escape(name),
        w = w as i64,
        h = h as i64
    );
    append_shape(doc, &xml)
}

// -------------------------------------------------------------------- bilder

/// Presentationsdelen och relations-id → bilddel.
fn pres_rels(txn: &Txn) -> (String, Vec<(String, String)>) {
    let pres = txn.pres_part();
    let rels = txn.rels(&pres).into_iter().filter(|r| r.kind == "slide").map(|r| (r.id, r.target)).collect();
    (pres, rels)
}

/// `p:sldIdLst` i presentationen (skapas på rätt plats om den saknas).
fn sld_id_lst<'d>(doc: &'d mut XDoc) -> Result<&'d mut XElem, String> {
    if doc.root.child("p:sldIdLst").is_none() {
        let p = doc.prefix_for(NS_P).unwrap_or_else(|| "p".into());
        let tag = if p.is_empty() { "sldIdLst".to_owned() } else { format!("{p}:sldIdLst") };
        let el = doc_fragment(doc, &format!("<{tag}></{tag}>"))?;
        // Efter `sldMasterIdLst`, `notesMasterIdLst`, `handoutMasterIdLst`.
        let after = ["p:sldMasterIdLst", "p:notesMasterIdLst", "p:handoutMasterIdLst"];
        let at = doc.root.children.iter().rposition(|c| matches!(c, XNode::Elem(e) if after.contains(&e.name.as_str()))).map_or(0, |i| i + 1);
        doc.root.children.insert(at, XNode::Elem(el));
    }
    doc.root.child_mut("p:sldIdLst").ok_or_else(|| "sldIdLst".to_owned())
}

/// Bildernas ordning som (r:id, sldId-elementets index) i `sldIdLst`.
fn slide_rid(txn: &Txn, slide: usize) -> Result<String, String> {
    let part = slide_part(txn.deck, slide)?;
    let (_, rels) = pres_rels(txn);
    rels.into_iter().find(|(_, t)| *t == part).map(|(id, _)| id).ok_or_else(|| t("Bilden finns inte längre.").to_owned())
}

fn sld_ids(lst: &XElem) -> Vec<(usize, String, String)> {
    lst.children
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match c {
            XNode::Elem(e) if e.local() == "sldId" => Some((i, e.attr("r:id").unwrap_or_default(), e.attr("id").unwrap_or_default())),
            _ => None,
        })
        .collect()
}

/// Uppdaterar antalet bilder i `docProps/app.xml` (om delen finns).
fn update_app_count(txn: &mut Txn, delta: i64) -> Result<(), String> {
    let app = txn.rels("").into_iter().find(|r| r.kind == "extended-properties").map(|r| r.target);
    let Some(app) = app.filter(|a| txn.exists(a)) else { return Ok(()) };
    let Ok(doc) = txn.doc(&app) else { return Ok(()) };
    if let Some(e) = doc.root.child_mut("Slides") {
        let n = e.text().trim().parse::<i64>().unwrap_or(0) + delta;
        e.set_text(&n.max(0).to_string());
    }
    Ok(())
}

/// Infogar `sldId` för en ny bilddel efter bilden `after` (eller först) och lägger den i samma avsnitt.
fn insert_sld_id(txn: &mut Txn, part: &str, after: Option<usize>) -> Result<(), String> {
    let (pres, _) = pres_rels(txn);
    let after_rid = match after {
        Some(i) => Some(slide_rid(txn, i)?),
        None => None,
    };
    let rid = txn.add_rel(&pres, "slide", part)?;
    txn.ct_override(part, ct::SLIDE)?;
    let doc = txn.doc(&pres)?;
    let r = pfx(doc, NS_R, "r");
    let p = doc.prefix_for(NS_P).unwrap_or_else(|| "p".into());
    let tag = if p.is_empty() { "sldId".to_owned() } else { format!("{p}:sldId") };
    let lst = sld_id_lst(doc)?;
    let ids = sld_ids(lst);
    // Största + 1; id:n får vara högst 2 147 483 647, annars det minsta lediga från 256.
    let used: HashSet<i64> = ids.iter().filter_map(|(_, _, i)| i.parse::<i64>().ok()).collect();
    let mut new_id = used.iter().copied().max().unwrap_or(255).max(255) + 1;
    if new_id >= 2_147_483_648 {
        new_id = (256..).find(|i| !used.contains(i)).unwrap_or(256);
    }
    let el = doc_fragment(doc, &format!("<{tag} id=\"{new_id}\" {r}:id=\"{rid}\"/>"))?;
    let lst = sld_id_lst(doc)?;
    let ids = sld_ids(lst);
    let (at, after_sld) = match &after_rid {
        Some(ar) => match ids.iter().find(|(_, r, _)| r == ar) {
            Some((i, _, sid)) => (i + 1, Some(sid.clone())),
            None => (lst.children.len(), None),
        },
        None => (ids.first().map_or(0, |(i, _, _)| *i), None),
    };
    lst.children.insert(at, XNode::Elem(el));
    // Avsnitt (`p14:sectionLst`): den nya bilden hamnar i samma avsnitt som bilden före.
    let first_sid = ids.first().map(|(_, _, s)| s.clone());
    sections_insert(&mut doc.root, after_sld.as_deref(), first_sid.as_deref(), &new_id.to_string());
    update_app_count(txn, 1)
}

/// Lägger in bild-id `new` efter `after` i dess avsnitt (eller först i första avsnittet).
fn sections_insert(root: &mut XElem, after: Option<&str>, first: Option<&str>, new: &str) {
    let mut done = false;
    root.walk_mut(&mut |e| {
        if done || e.local() != "sldIdLst" || !e.name.starts_with("p14:") {
            return;
        }
        let pos = e.children.iter().position(|c| matches!(c, XNode::Elem(x) if x.attr("id").as_deref() == after.or(first)));
        let Some(pos) = pos else { return };
        let Some(XNode::Elem(tmpl)) = e.children.get(pos).cloned() else { return };
        let mut el = tmpl;
        el.set_attr("id", new);
        let at = if after.is_some() { pos + 1 } else { pos };
        e.children.insert(at, XNode::Elem(el));
        done = true;
    });
}

fn sections_remove(root: &mut XElem, sid: &str) {
    root.walk_mut(&mut |e| {
        if e.local() == "sldIdLst" && e.name.starts_with("p14:") {
            e.children.retain(|c| !matches!(c, XNode::Elem(x) if x.attr("id").as_deref() == Some(sid)));
        }
    });
}

/// Ny bild efter `after` med layouten `layout` (index i `deck.layouts`); platshållarna skapas tomma
/// enligt layouten. Ger den nya bildens index.
pub fn new_slide(txn: &mut Txn, after: Option<usize>, layout: usize) -> Result<usize, String> {
    let lay = txn.deck.layouts.get(layout).ok_or_else(|| t("Layouten finns inte.").to_owned())?;
    let lay_part = lay.part.clone();
    let mut shapes = String::new();
    let mut id = 1;
    if let Some(tree) = lay.root.path(&["p:cSld", "p:spTree"]) {
        for c in &tree.children {
            if c.name != "p:sp" {
                continue;
            }
            let Some(ph) = inherit::nv_pr(c).and_then(|n| n.child("p:ph")) else { continue };
            let typ = ph.attr("type").unwrap_or("obj");
            if matches!(typ, "dt" | "ftr" | "sldNum" | "hdr") {
                continue;
            }
            id += 1;
            let mut attrs = String::new();
            for k in ["type", "orient", "sz", "idx"] {
                if let Some(v) = ph.attr(k) {
                    attrs.push_str(&format!(" {k}=\"{}\"", edit_xml::escape(v)));
                }
            }
            let name = nv_cnvpr(c).and_then(|n| n.attr("name")).unwrap_or("");
            let body = if c.child("p:txBody").is_some() { "<p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody>" } else { "" };
            shapes.push_str(&format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{}"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph{attrs}/></p:nvPr></p:nvSpPr><p:spPr/>{body}</p:sp>"#,
                edit_xml::escape(name)
            ));
        }
    }
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<p:sld xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#,
        "\r\n"
    );
    let part = txn.unique_part("ppt/slides/slide", ".xml");
    txn.put_xml(&part, &xml)?;
    txn.add_rel(&part, "slideLayout", &lay_part)?;
    insert_sld_id(txn, &part, after)?;
    Ok(after.map_or(0, |a| a + 1))
}

/// Relationstyper vars mål delas av en kopia av bilden (övriga delar kopieras).
const SHARED: &[&str] = &["slideLayout", "slideMaster", "notesMaster", "image", "hyperlink", "slide", "media", "video", "audio", "theme", "font", "customXml"];

/// Kopierar en del med dess relationer (rekursivt; delade mål pekas på). Ger den nya delens namn.
fn copy_part(txn: &mut Txn, part: &str, map: &mut HashMap<String, String>) -> Result<String, String> {
    if let Some(n) = map.get(part) {
        return Ok(n.clone());
    }
    let bytes = txn.read(part).ok_or_else(|| format!("{part} saknas"))?;
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let stem = file.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c.is_ascii_alphabetic() && false);
    let (base, ext) = match file.rfind('.') {
        Some(i) => (&file[..i], &file[i..]),
        None => (stem, ""),
    };
    let base = base.trim_end_matches(|c: char| c.is_ascii_digit());
    let new = txn.unique_part(&format!("{}{base}", if dir.is_empty() { String::new() } else { format!("{dir}/") }), ext);
    map.insert(part.to_owned(), new.clone());
    txn.put_raw(&new, bytes);
    if let Some(ty) = txn.content_type(part) {
        let by_default = txn.content_type(&format!("x{ext}")).is_some_and(|d| d == ty);
        if !by_default {
            txn.ct_override(&new, &ty)?;
        }
    }
    let new_dir = new.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
    for (id, ty, target, external) in txn.raw_rels(part) {
        let kind = ty.rsplit('/').next().unwrap_or("").to_owned();
        if external {
            txn.add_rel_raw(&new, &id, &ty, &target, true)?;
            continue;
        }
        let abs = super::package::resolve(dir, &target);
        let dest = match map.get(&abs).filter(|m| !m.is_empty()) {
            Some(m) => m.clone(),
            None if SHARED.contains(&kind.as_str()) || !txn.exists(&abs) => abs,
            None => copy_part(txn, &abs, map)?,
        };
        txn.add_rel_raw(&new, &id, &ty, &edit::relative(&new_dir, &dest), false)?;
    }
    Ok(new)
}

/// Duplicerar bilden (med anteckningar, diagram och SmartArt som egna kopior) direkt efter den.
pub fn duplicate_slide(txn: &mut Txn, slide: usize) -> Result<usize, String> {
    let part = slide_part(txn.deck, slide)?;
    let notes = txn.rels(&part).into_iter().find(|r| r.kind == "notesSlide").map(|r| r.target);
    let mut map = HashMap::new();
    // Anteckningarna kopieras för sig (deras relation tillbaka ska peka på kopian).
    if let Some(n) = &notes {
        map.insert(n.clone(), String::new());
    }
    let new = copy_part(txn, &part, &mut map)?;
    if let Some(n) = notes {
        // Kopians relation till anteckningarna pekar på originalets: byt mot en kopia.
        map.remove(&n);
        let rel = txn.rels(&new).into_iter().find(|r| r.kind == "notesSlide");
        if let Some(rel) = rel {
            txn.remove_rel(&new, &rel.id)?;
            let mut nmap = HashMap::new();
            nmap.insert(part.clone(), new.clone());
            let ncopy = copy_part(txn, &n, &mut nmap)?;
            let ty = edit::rel_type("notesSlide");
            let dir = new.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
            txn.add_rel_raw(&new, &rel.id, &ty, &edit::relative(&dir, &ncopy), false)?;
        }
    }
    // Ett nytt `p14:creationId` (samarbetsfunktionernas id).
    if let Ok(doc) = txn.doc(&new) {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1);
        doc.root.walk_mut(&mut |e| {
            if e.name == "p14:creationId" {
                e.set_attr("val", &(seed % 4_000_000_000 + 1).to_string());
            }
        });
    }
    insert_sld_id(txn, &new, Some(slide))?;
    Ok(slide + 1)
}

/// Tar bort bilden: `sldId`, relationen, delen, dess anteckningar, länkar till den från andra delar,
/// avsnitts- och anpassade visningar, och delar som bara bilden använde.
pub fn delete_slide(txn: &mut Txn, slide: usize) -> Result<(), String> {
    let part = slide_part(txn.deck, slide)?;
    let rid = slide_rid(txn, slide)?;
    let (pres, _) = pres_rels(txn);
    // Delar som bilden pekar på (kandidater att städa bort om inget annat använder dem).
    let mut candidates: Vec<String> = txn.rels(&part).into_iter().filter(|r| !r.external && !matches!(r.kind.as_str(), "slideLayout" | "slide" | "notesMaster")).map(|r| r.target).collect();
    let doc = txn.doc(&pres)?;
    let lst = sld_id_lst(doc)?;
    let sid = sld_ids(lst).into_iter().find(|(_, r, _)| *r == rid).map(|(_, _, s)| s);
    lst.children.retain(|c| !matches!(c, XNode::Elem(e) if e.local() == "sldId" && e.attr("r:id").as_deref() == Some(rid.as_str())));
    if let Some(s) = &sid {
        sections_remove(&mut doc.root, s);
    }
    // Anpassade visningar (`p:custShow/p:sldLst/p:sld r:id`).
    doc.root.walk_mut(&mut |e| {
        if e.name == "p:sldLst" {
            e.children.retain(|c| !matches!(c, XNode::Elem(x) if x.attr("r:id").as_deref() == Some(rid.as_str())));
        }
    });
    txn.remove_rel(&pres, &rid)?;
    // Länkar från andra delar till bilden.
    let rels_parts: Vec<String> = txn.deck.pkg.parts.keys().filter(|k| k.ends_with(".rels")).cloned().collect();
    for rp in rels_parts {
        let Some(owner) = owner_of_rels(&rp) else { continue };
        if owner == part || owner == pres || !txn.exists(&owner) {
            continue;
        }
        let ids: Vec<String> = txn.rels(&owner).into_iter().filter(|r| !r.external && r.target == part).map(|r| r.id).collect();
        for id in ids {
            txn.remove_rel(&owner, &id)?;
            if let Ok(d) = txn.doc(&owner) {
                d.root.walk_mut(&mut |e| {
                    e.children.retain(|c| !matches!(c, XNode::Elem(x) if x.attr("r:id").as_deref() == Some(id.as_str()) && (x.local().starts_with("hlink") || x.local() == "sld")));
                });
            }
        }
    }
    txn.remove_part(&part)?;
    gc(txn, &mut candidates)?;
    update_app_count(txn, -1)
}

/// `ppt/slides/_rels/slide1.xml.rels` → `ppt/slides/slide1.xml`.
fn owner_of_rels(rels: &str) -> Option<String> {
    let (dir, file) = rels.rsplit_once("/_rels/").map(|(d, f)| (d.to_owned(), f)).or_else(|| rels.strip_prefix("_rels/").map(|f| (String::new(), f)))?;
    let file = file.strip_suffix(".rels")?;
    Some(if dir.is_empty() { file.to_owned() } else { format!("{dir}/{file}") })
}

/// Tar bort kandidatdelar (och deras mål, rekursivt) som ingen kvarvarande relation pekar på.
fn gc(txn: &mut Txn, candidates: &mut Vec<String>) -> Result<(), String> {
    let mut guard = 0;
    while let Some(c) = candidates.pop() {
        guard += 1;
        if guard > 10_000 || !txn.exists(&c) {
            continue;
        }
        let rels_parts: Vec<String> = txn.deck.pkg.parts.keys().filter(|k| k.ends_with(".rels")).cloned().chain(std::iter::once("_rels/.rels".to_owned())).collect();
        let referenced = rels_parts.iter().any(|rp| {
            let Some(owner) = owner_of_rels(rp) else { return false };
            (owner.is_empty() || txn.exists(&owner)) && owner != c && txn.rels(&owner).iter().any(|r| !r.external && r.target == c)
        });
        if !referenced {
            let next: Vec<String> = txn.rels(&c).into_iter().filter(|r| !r.external && !SHARED[..3].contains(&r.kind.as_str())).map(|r| r.target).collect();
            txn.remove_part(&c)?;
            candidates.extend(next);
        }
    }
    Ok(())
}

/// Flyttar bilden `from` så att den hamnar på plats `to`.
pub fn move_slide(txn: &mut Txn, from: usize, to: usize) -> Result<(), String> {
    let n = txn.deck.slides.len();
    if from >= n || to >= n || from == to {
        return Ok(());
    }
    let rid = slide_rid(txn, from)?;
    let neighbour = if to > from { slide_rid(txn, to)? } else { slide_rid(txn, to)? };
    let (pres, _) = pres_rels(txn);
    let doc = txn.doc(&pres)?;
    let lst = sld_id_lst(doc)?;
    let ids = sld_ids(lst);
    let Some(&(src, _, ref sid)) = ids.iter().find(|(_, r, _)| *r == rid) else { return Ok(()) };
    let sid = sid.clone();
    let node = lst.children.remove(src);
    let ids = sld_ids(lst);
    let Some(&(dst, _, ref nsid)) = ids.iter().find(|(_, r, _)| *r == neighbour) else { return Ok(()) };
    let nsid = nsid.clone();
    let at = if to > from { dst + 1 } else { dst };
    lst.children.insert(at, XNode::Elem(match node {
        XNode::Elem(e) => e,
        _ => return Ok(()),
    }));
    // Avsnitten: bilden flyttar till grannens avsnitt, på motsvarande plats.
    let mut has_sections = false;
    doc.root.walk(&mut |e| has_sections |= e.local() == "sectionLst");
    if has_sections {
        sections_remove(&mut doc.root, &sid);
        if to > from {
            sections_insert(&mut doc.root, Some(&nsid), None, &sid);
        } else {
            // Före grannen: efter grannens föregångare i samma avsnitt, annars först i avsnittet.
            sections_insert_before(&mut doc.root, &nsid, &sid);
        }
    }
    Ok(())
}

fn sections_insert_before(root: &mut XElem, before: &str, new: &str) {
    let mut done = false;
    root.walk_mut(&mut |e| {
        if done || e.local() != "sldIdLst" || !e.name.starts_with("p14:") {
            return;
        }
        let Some(pos) = e.children.iter().position(|c| matches!(c, XNode::Elem(x) if x.attr("id").as_deref() == Some(before))) else { return };
        let Some(XNode::Elem(mut el)) = e.children.get(pos).cloned() else { return };
        el.set_attr("id", new);
        e.children.insert(pos, XNode::Elem(el));
        done = true;
    });
}

/// Döljer eller visar bilden (`show="0"`).
pub fn set_hidden(txn: &mut Txn, slide: usize, hidden: bool) -> Result<(), String> {
    let part = slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    if hidden {
        doc.root.set_attr("show", "0");
    } else {
        doc.root.remove_attr("show");
    }
    Ok(())
}

// -------------------------------------------------------------------- anteckningar

/// Sätter anteckningarnas text (en rad = ett stycke). Saknas anteckningssidan skapas den enligt
/// anteckningsmallen; saknas mallen skapas en enkel mall med presentationens tema.
pub fn set_notes(txn: &mut Txn, slide: usize, text: &str) -> Result<(), String> {
    let part = slide_part(txn.deck, slide)?;
    let notes = match txn.rels(&part).into_iter().find(|r| r.kind == "notesSlide").map(|r| r.target).filter(|n| txn.exists(n)) {
        Some(n) => n,
        None => {
            if text.trim().is_empty() {
                return Ok(());
            }
            create_notes_slide(txn, &part)?
        }
    };
    let doc = txn.doc(&notes)?;
    let (p, a) = (pfx(doc, NS_P, "p"), pfx(doc, NS_A, "a"));
    let tree = tree_mut(doc)?;
    // Brödtextplatshållaren (`type="body"`).
    let is_body = |e: &XElem| {
        e.local() == "sp" && e.path(&["p:nvSpPr", "p:nvPr", "p:ph"]).is_some_and(|ph| ph.attr("type").as_deref() == Some("body"))
    };
    let pos = tree.children.iter().position(|c| matches!(c, XNode::Elem(e) if is_body(e)));
    let pos = match pos {
        Some(i) => i,
        None => {
            let id = max_id(tree) + 1;
            let xml = format!(r#"<{p}:sp><{p}:nvSpPr><{p}:cNvPr id="{id}" name="Notes Placeholder"/><{p}:cNvSpPr><{a}:spLocks noGrp="1"/></{p}:cNvSpPr><{p}:nvPr><{p}:ph type="body" idx="1"/></{p}:nvPr></{p}:nvSpPr><{p}:spPr/></{p}:sp>"#);
            let scope = doc.root_scope();
            let el = edit_xml::fragment_in(&scope, &xml).into_iter().next().ok_or("anteckningar")?;
            let tree = tree_mut(doc)?;
            let at = tree.index_of("p:extLst").unwrap_or(tree.children.len());
            tree.children.insert(at, XNode::Elem(el));
            at
        }
    };
    let scope = doc.root_scope();
    let tree = tree_mut(doc)?;
    let XNode::Elem(sp) = &mut tree.children[pos] else { return Ok(()) };
    super::edit_text::set_plain_text(sp, &scope, &p, &a, text)
}

/// Skapar en anteckningssida för bilden och ger dess delnamn.
fn create_notes_slide(txn: &mut Txn, slide_part: &str) -> Result<String, String> {
    let pres = txn.pres_part();
    let master = match txn.rels(&pres).into_iter().find(|r| r.kind == "notesMaster").map(|r| r.target).filter(|m| txn.exists(m)) {
        Some(m) => m,
        None => create_notes_master(txn, &pres)?,
    };
    let part = txn.unique_part("ppt/notesSlides/notesSlide", ".xml");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<p:notes xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#,
        "\r\n"
    );
    txn.put_xml(&part, &xml)?;
    txn.ct_override(&part, ct::NOTES_SLIDE)?;
    txn.add_rel(&part, "notesMaster", &master)?;
    txn.add_rel(&part, "slide", slide_part)?;
    txn.add_rel(slide_part, "notesSlide", &part)?;
    Ok(part)
}

/// Ett enkelt tema (Office-färger, Calibri) för en anteckningsmall när presentationen saknar tema.
const MINIMAL_THEME: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office"><a:themeElements><a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#;

/// Skapar en enkel anteckningsmall (med en kopia av presentationens första tema) och ger dess delnamn.
fn create_notes_master(txn: &mut Txn, pres: &str) -> Result<String, String> {
    // En kopia av mallens tema (med dess relationer), annars ett enkelt tema.
    let theme_src = txn.deck.masters.first().and_then(|m| m.rels.iter().find(|r| r.kind == "theme")).map(|r| r.target.clone()).filter(|t| txn.exists(t));
    let theme = match theme_src {
        Some(src) => {
            let mut map = HashMap::new();
            let theme = copy_part(txn, &src, &mut map)?;
            txn.ct_override(&theme, ct::THEME)?;
            theme
        }
        None => {
            let theme = txn.unique_part("ppt/theme/theme", ".xml");
            txn.put_xml(&theme, MINIMAL_THEME)?;
            txn.ct_override(&theme, ct::THEME)?;
            theme
        }
    };
    let (cx, cy) = txn.deck.notes_size_emu;
    let (ix, iy, iw, ih) = (cx / 6, cy / 12, cx * 2 / 3, cx * 2 / 3 * txn.deck.size_emu.1 / txn.deck.size_emu.0.max(1));
    let (bx, by, bw, bh) = (cx / 10, iy + ih + cy / 24, cx * 4 / 5, cy - (iy + ih + cy / 24) - cy / 12);
    let lvl = |n: u32| format!(r#"<a:lvl{n}pPr marL="{}" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1"><a:defRPr sz="1200" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl{n}pPr>"#, (n - 1) * 457200);
    let style: String = (1..=9).map(lvl).collect();
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<p:notesMaster xmlns:a="{NS_A}" xmlns:r="{NS_R}" xmlns:p="{NS_P}"><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide Image Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg" idx="2"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="{ix}" y="{iy}"/><a:ext cx="{iw}" cy="{ih}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln w="12700"><a:solidFill><a:prstClr val="black"/></a:solidFill></a:ln></p:spPr></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" sz="quarter" idx="3"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="{bx}" y="{by}"/><a:ext cx="{bw}" cy="{bh}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0"/><a:lstStyle/><a:p><a:pPr lvl="0"/><a:r><a:rPr lang="sv-SE"/><a:t>Text</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:notesStyle>{style}</p:notesStyle></p:notesMaster>"#,
        "\r\n"
    );
    let part = txn.unique_part("ppt/notesMasters/notesMaster", ".xml");
    txn.put_xml(&part, &xml)?;
    txn.ct_override(&part, ct::NOTES_MASTER)?;
    txn.add_rel(&part, "theme", &theme)?;
    let rid = txn.add_rel(pres, "notesMaster", &part)?;
    let doc = txn.doc(pres)?;
    let r = pfx(doc, NS_R, "r");
    let p = doc.prefix_for(NS_P).unwrap_or_else(|| "p".into());
    let q = |l: &str| if p.is_empty() { l.to_owned() } else { format!("{p}:{l}") };
    let el = doc_fragment(doc, &format!("<{l}><{i} {r}:id=\"{rid}\"/></{l}>", l = q("notesMasterIdLst"), i = q("notesMasterId")))?;
    // Direkt efter `sldMasterIdLst`.
    let at = doc.root.index_of("p:sldMasterIdLst").map_or(0, |i| i + 1);
    doc.root.children.insert(at, XNode::Elem(el));
    Ok(part)
}

/// Lägen (ritmodellen) → `Node` för formen, för den som vill läsa formens egenskaper.
pub fn node<'d>(deck: &'d Deck, slide: usize, index: usize) -> Option<&'d Node> {
    deck.slides.get(slide)?.root.path(&["p:cSld", "p:spTree"])?.children.get(index)
}


/// Toppnivåformen med läget `index` i ritmodellen ur den förlustfria XML:en (inte i ett
/// `mc:AlternateContent`).
pub fn raw_shape(doc: &XDoc, index: usize) -> Option<&XElem> {
    let scope = doc.root_scope();
    let tree = doc.root.path(&["p:cSld", "p:spTree"])?;
    match locate(tree, &scope, index)? {
        (ci, None) => match &tree.children[ci] {
            XNode::Elem(e) => Some(e),
            _ => None,
        },
        _ => None,
    }
}

/// Skriver textmodellen till formens `p:txBody`.
pub fn set_text(txn: &mut Txn, slide: usize, index: usize, id: u32, model: &super::edit_text::TextModel) -> Result<(), String> {
    // Modellens prefix för DrawingML ska vara deklarerat på roten (det kan saknas där).
    let part = slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    if doc.prefix_for(NS_A).is_none() && !model.prefix().is_empty() {
        doc.root.set_attr(&format!("xmlns:{}", model.prefix()), NS_A);
    }
    with_shape(txn, slide, index, id, &mut |e, doc| {
        if e.local() != "sp" {
            return Err(t("Bara text i former kan redigeras.").to_owned());
        }
        let p = doc.prefix_for(NS_P).unwrap_or_else(|| "p".into());
        model.write_into(e, &p);
        Ok(())
    })
}
