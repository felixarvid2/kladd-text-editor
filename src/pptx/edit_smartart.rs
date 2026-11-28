//! Redigera text i SmartArt: dubbelklick på en form i ett diagram redigerar texten i den nod formen
//! visar, och Enter/Tab/Delete lägger till, flyttar och tar bort noder.
//!
//! - **Text:** formen hittas i ritdelen (PowerPoints sparade `dsp:`-ritdel, annars Kladds egen ur
//!   layoutmotorn) och noden via `presOf`-kopplingen (eller `kladdPt` i motorns ritdel). Texten skrivs
//!   till båda: datamodellens `dgm:pt/dgm:t` (bara texten; nodens egen formatering behålls) och
//!   ritdelens `dsp:sp/dsp:txBody` (hela modellen, som ritas). Utan sparad ritdel skrivs bara
//!   datamodellen och motorn lägger ut diagrammet igen när presentationen byggs om.
//! - **Noder:** Enter i slutet av en nod ger ett syskon efter den, Tab flyttar noden en nivå ned (blir
//!   barn till syskonet före), Skift+Tab en nivå upp, och Backsteg/Delete i en tom nod utan barn tar bort
//!   den. Ändringarna görs i datamodellen (`dgm:ptLst`, `parOf`-kopplingar med `srcOrd`, nya `modelId`)
//!   och den sparade ritdelen tas bort (delen, bladets relation och `dsp:dataModelExt`): Kladds motor
//!   lägger ut diagrammet, och PowerPoint lägger ut det igen ur datamodellen när filen öppnas.

use super::edit::Txn;
use super::edit_text::{Carets, Item, TPara, TextModel};
use super::edit_view::{Sel, TextState};
use super::edit_xml::{self, XDoc, XElem, XNode};
use super::model::Deck;
use super::render::frame_of;
use super::scene::Affine;
use super::view::PptxTab;
use super::xml::Node;
use super::{edit_ops, smartart};

const NS_DSP: &str = "http://schemas.microsoft.com/office/drawing/2008/diagram";
const NS_DGM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";

/// En nod i ett SmartArt-diagram under redigering.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub slide: usize,
    /// Diagrammets ram (toppnivåform).
    pub sel: Sel,
    /// Datamodellens del (`ppt/diagrams/dataN.xml`).
    pub data_part: String,
    /// PowerPoints sparade ritdel när diagrammet ritas ur den; `None` = Kladds layoutmotor.
    pub drawing_part: Option<String>,
    /// Formens `modelId` i ritdelen (presentationspunkten; tom i motorns ritdel).
    pub shape_id: String,
    /// Datapunkten (noden) vars text formen visar.
    pub pt: String,
}

/// Vad Enter, Tab och Delete gör med noden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// Nytt syskon efter noden.
    After,
    /// En nivå ned (barn till syskonet före).
    Demote,
    /// En nivå upp (syskon efter föräldern).
    Promote,
    /// Ta bort (tom nod utan barn).
    Remove,
}

/// Ett diagram på bilden: ramen, datadelen, ritdelen och avbildningen av ritdelens koordinater.
struct Diagram<'d> {
    sel: Sel,
    data_part: String,
    drawing_part: Option<String>,
    drawing: &'d Node,
    data: Node,
    t: Affine,
}

fn diagrams(deck: &Deck, slide: usize) -> Vec<Diagram<'_>> {
    let Some(s) = deck.slides.get(slide) else { return Vec::new() };
    let mut out = Vec::new();
    for info in edit_ops::shapes(deck, slide).into_iter().filter(|i| i.kind == edit_ops::Kind::Frame) {
        let Some(gf) = edit_ops::node(deck, slide, info.index) else { continue };
        let Some(gd) = gf.path(&["a:graphic", "a:graphicData"]).filter(|g| g.attr("uri").is_some_and(|u| u.ends_with("/diagram"))) else { continue };
        let Some(dm) = smartart::data_part(&s.rels, gd) else { continue };
        let Some(data) = deck.pkg.xml(&dm) else { continue };
        let saved = smartart::drawing_part_of(&s.rels, Some(&data)).filter(|p| deck.drawing(p).is_some_and(smartart::usable));
        let (drawing_part, drawing) = match &saved {
            Some(p) => (Some(p.clone()), deck.drawing(p)),
            None => (None, deck.drawing(&smartart::laid_out_key(&dm))),
        };
        let Some(drawing) = drawing else { continue };
        let Some(f) = gf.child("p:xfrm").and_then(|x| frame_of(x, &Affine::IDENTITY)) else { continue };
        out.push(Diagram { sel: Sel { slide, index: info.index, id: info.id }, data_part: dm, drawing_part, drawing: &drawing.root, data, t: smartart::drawing_transform(&f) });
    }
    out
}

/// Formerna i ritdelen (också i grupper), i ritordning.
fn shapes_of(root: &Node) -> Vec<&Node> {
    let mut out = Vec::new();
    root.walk(&mut |n| {
        if n.name == "p:sp" {
            out.push(n);
        }
    });
    out
}

/// Datapunkten formen visar: `kladdPt` i motorns ritdel, annars den enda nod (`node`/`asst`) som en
/// `presOf`-koppling till formens presentationspunkt kommer från.
fn point_of(data: &Node, sp: &Node) -> Option<String> {
    if let Some(p) = sp.attr("kladdPt") {
        return Some(p.to_owned());
    }
    let id = sp.attr("modelId").filter(|m| !m.is_empty())?;
    let kinds: std::collections::HashMap<&str, &str> = data.child("dgm:ptLst")?.children_named("dgm:pt").filter_map(|p| Some((p.attr("modelId")?, p.attr("type").unwrap_or("node")))).collect();
    let srcs: Vec<&str> = data.child("dgm:cxnLst")?.children_named("dgm:cxn").filter(|c| c.attr("type") == Some("presOf") && c.attr("destId") == Some(id)).filter_map(|c| c.attr("srcId")).collect();
    match srcs.as_slice() {
        [one] if matches!(kinds.get(one), Some(&"node") | Some(&"asst")) => Some((*one).to_owned()),
        _ => None,
    }
}

/// Formens textruta som en form för markörlägena: `dsp:txXfrm` (med formens vridning) och rektangel.
fn caret_node(sp: &Node) -> Node {
    let mut n = sp.clone();
    let Some(tx) = sp.child("p:txXfrm") else { return n };
    let rot = sp.path(&["p:spPr", "a:xfrm"]).and_then(|x| x.attr_i64("rot")).unwrap_or(0) + tx.attr_i64("rot").unwrap_or(0);
    let mut xfrm = tx.clone();
    xfrm.name = "a:xfrm".into();
    xfrm.attrs.retain(|a| a.0 != "rot");
    if rot != 0 {
        xfrm.attrs.push(("rot".into(), rot.to_string()));
    }
    if let Some(sppr) = n.children.iter_mut().find(|c| c.name == "p:spPr") {
        sppr.children.retain(|c| !matches!(c.name.as_str(), "a:xfrm" | "a:prstGeom" | "a:custGeom"));
        sppr.children.insert(0, xfrm);
        sppr.children.insert(1, super::dgm::emit::el("a:prstGeom", &[("prst", "rect".into())], vec![super::dgm::emit::el("a:avLst", &[], Vec::new())]));
    }
    n
}

/// Är punkten (bildens punkter) i formens textruta?
fn hit(sp: &Node, t: &Affine, x: f32, y: f32) -> bool {
    let n = caret_node(sp);
    let Some(f) = n.path(&["p:spPr", "a:xfrm"]).and_then(|x| frame_of(x, t)) else { return false };
    let Some(inv) = f.t.invert() else { return false };
    let (lx, ly) = inv.apply(x, y);
    lx >= 0.0 && ly >= 0.0 && lx <= f.w && ly <= f.h
}

/// Noden under punkten (överst ritad form först) i ett diagram på bilden.
pub fn target_at(deck: &Deck, slide: usize, x: f32, y: f32) -> Option<Target> {
    for d in diagrams(deck, slide).into_iter().rev() {
        for sp in shapes_of(d.drawing).into_iter().rev() {
            if !hit(sp, &d.t, x, y) {
                continue;
            }
            let Some(pt) = point_of(&d.data, sp) else { continue };
            return Some(Target {
                slide,
                sel: d.sel,
                data_part: d.data_part.clone(),
                drawing_part: d.drawing_part.clone(),
                shape_id: sp.attr("modelId").unwrap_or("").to_owned(),
                pt,
            });
        }
    }
    None
}

/// Formen för noden i diagrammets nuvarande ritdel (motorns ritdel byggs om efter varje ändring).
fn shape_for<'d>(d: &'d Diagram<'_>, t: &Target) -> Option<&'d Node> {
    shapes_of(d.drawing).into_iter().find(|sp| if d.drawing_part.is_some() { sp.attr("modelId") == Some(t.shape_id.as_str()) } else { sp.attr("kladdPt") == Some(t.pt.as_str()) })
}

/// Nodens text som modell: ur ritdelens `dsp:txBody` (det som ritas), annars ur datamodellens `dgm:t`.
pub fn model(deck: &Deck, t: &Target) -> Option<TextModel> {
    let part = t.drawing_part.as_deref().unwrap_or(&t.data_part);
    let doc = edit_xml::parse(deck.pkg.read(part)?)?;
    let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
    let body = match &t.drawing_part {
        Some(_) => find(&doc.root, "p:sp", &t.shape_id)?.child("p:txBody"),
        None => find(&doc.root, "dgm:pt", &t.pt)?.child("dgm:t"),
    };
    Some(TextModel::from_body(body, &doc.root_scope(), &a))
}

fn find<'e>(root: &'e XElem, name: &str, id: &str) -> Option<&'e XElem> {
    if root.name == name && root.attr("modelId").as_deref() == Some(id) {
        return Some(root);
    }
    root.elems().find_map(|c| find(c, name, id))
}

fn find_mut<'e>(root: &'e mut XElem, name: &str, id: &str) -> Option<&'e mut XElem> {
    if root.name == name && root.attr("modelId").as_deref() == Some(id) {
        return Some(root);
    }
    for c in root.elems_mut() {
        if let Some(f) = find_mut(c, name, id) {
            return Some(f);
        }
    }
    None
}

/// Markörlägena för modellen i nodens form. Utan sparad ritdel saknar datamodellens text oftast
/// storlek; den får då storleken motorn ritar med.
pub fn carets(deck: &Deck, t: &Target, model: &TextModel) -> Option<Carets> {
    let ds = diagrams(deck, t.slide);
    let d = ds.iter().find(|d| d.data_part == t.data_part)?;
    let sp = shape_for(d, t)?;
    let mut m = model.clone();
    if d.drawing_part.is_none() {
        let mut sz = None;
        sp.walk(&mut |n| {
            if sz.is_none() && n.name == "a:rPr" {
                sz = n.attr("sz").map(str::to_owned);
            }
        });
        if let Some(sz) = sz {
            with_size(&mut m, &sz);
        }
    }
    m.in_parent(d.t).carets(deck, t.slide, &caret_node(sp))
}

/// Sätter `sz` på körningar utan storlek (bara för markörlägena).
fn with_size(m: &mut TextModel, sz: &str) {
    let a = m.prefix().to_owned();
    let tag = if a.is_empty() { "rPr".to_owned() } else { format!("{a}:rPr") };
    let blank = m.frag(&format!("<{tag}/>"));
    for p in &mut m.paras {
        for it in &mut p.items {
            if let Item::Run { rpr, .. } = it {
                if rpr.is_none() {
                    *rpr = blank.clone();
                }
                if let Some(r) = rpr {
                    if r.attr("sz").is_none() {
                        r.set_attr("sz", sz);
                    }
                }
            }
        }
    }
}

/// Skriver modellens stycken i `dgm:t` (eller annan textkropp): `a:p` ersätts, resten behålls.
fn write_paras(body: &mut XElem, paras: Vec<XElem>) {
    let at = body.index_of("a:p").unwrap_or(body.children.len());
    body.remove_children("a:p");
    let at = at.min(body.children.len());
    for (k, p) in paras.into_iter().enumerate() {
        body.children.insert(at + k, XNode::Elem(p));
    }
}

/// Datamodellens stycken med texten ur `model` och formateringen ur nodens egen text (`old`):
/// ett stycke per stycke, styckeformat och körningsformat från motsvarande (annars första) stycke.
fn plain_paras(old: &TextModel, model: &TextModel) -> Vec<XElem> {
    let first_rpr = |p: &TPara| p.items.iter().find_map(|it| if let Item::Run { rpr, .. } = it { Some(rpr.clone()) } else { None }).flatten();
    let mut m = old.clone();
    let base = old.paras.first().cloned();
    m.paras = model
        .paras
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let src = old.paras.get(i).or(base.as_ref());
            let rpr = src.and_then(first_rpr).or_else(|| base.as_ref().and_then(first_rpr));
            let text = p.text().replace('\n', " ");
            let items = if text.is_empty() { Vec::new() } else { vec![Item::Run { rpr, text }] };
            // Ett nytt (ändrat) stycke: mallen ur en tom modell har inget original.
            let mut np = TextModel::from_body(None, &[], old.prefix()).paras.swap_remove(0);
            np.ppr = p.ppr.clone().or_else(|| src.and_then(|s| s.ppr.clone()));
            np.items = items;
            np.end = src.and_then(|s| s.end.clone());
            np
        })
        .collect();
    m.paras_xml()
}

/// Skriver nodens text: datamodellen och (om den finns) den sparade ritdelen.
pub fn set_text(txn: &mut Txn, t: &Target, model: &TextModel) -> Result<(), String> {
    let doc = txn.doc(&t.data_part)?;
    let scope = doc.root_scope();
    let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
    let pt = find_mut(&mut doc.root, "dgm:pt", &t.pt).ok_or("noden finns inte längre")?;
    if pt.child("dgm:t").is_none() {
        let dgm = pt.qname.split(':').next().filter(|_| pt.qname.contains(':')).map(|p| format!("{p}:")).unwrap_or_default();
        let q = |l: &str| if a.is_empty() { l.to_owned() } else { format!("{a}:{l}") };
        let el = edit_xml::fragment_in(&scope, &format!("<{dgm}t><{}/><{}/></{dgm}t>", q("bodyPr"), q("lstStyle")));
        pt.children.extend(el.into_iter().map(XNode::Elem));
    }
    let tbody = pt.child_mut("dgm:t").ok_or("dgm:t")?;
    let paras = match &t.drawing_part {
        Some(_) => plain_paras(&TextModel::from_body(Some(tbody), &scope, &a), model),
        None => model.paras_xml(),
    };
    write_paras(tbody, paras);
    if let Some(part) = &t.drawing_part {
        let doc = txn.doc(part)?;
        let dsp = doc.prefix_for(NS_DSP).unwrap_or_else(|| "dsp".into());
        let sp = find_mut(&mut doc.root, "p:sp", &t.shape_id).ok_or("formen finns inte längre i ritdelen")?;
        model.write_into(sp, &dsp);
    }
    Ok(())
}

// ------------------------------------------------------------------ noder

/// Ett nytt `modelId` som inte finns i datamodellen.
fn new_id(doc: &XDoc, salt: u64) -> String {
    let text = String::from_utf8_lossy(&doc.to_bytes()).into_owned();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    loop {
        for b in text.len().to_le_bytes().iter().chain(salt.to_le_bytes().iter()) {
            h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
        let g = h.rotate_left(17) ^ 0x5bd1_e995;
        let id = format!("{{{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}}}", h >> 32, (h >> 16) & 0xffff, g & 0xfff, (g >> 12) & 0xfff, (g >> 16) & 0xffff_ffff_ffff);
        if !text.contains(&id) {
            return id;
        }
        h = h.wrapping_add(1);
    }
}

fn frag1(doc: &XDoc, xml: &str) -> Result<XElem, String> {
    doc.fragment(xml).into_iter().next().ok_or_else(|| format!("XML: {xml}"))
}

/// `parOf`-kopplingarna (utan typ eller `parOf`) i `dgm:cxnLst`.
fn is_par_of(c: &XElem) -> bool {
    c.name == "dgm:cxn" && c.attr("type").is_none_or(|t| t == "parOf")
}

fn cxn_list(doc: &mut XDoc) -> Result<&mut XElem, String> {
    doc.root.child_mut("dgm:cxnLst").ok_or_else(|| "datamodellen saknar dgm:cxnLst".to_owned())
}

/// (förälder, srcOrd) för noden.
fn parent_of(doc: &XDoc, pt: &str) -> Option<(String, i64)> {
    doc.root.child("dgm:cxnLst")?.elems().find(|c| is_par_of(c) && c.attr("destId").as_deref() == Some(pt)).map(|c| (c.attr("srcId").unwrap_or_default(), c.attr_i64("srcOrd").unwrap_or(0)))
}

/// Barnen till `parent` i `srcOrd`-ordning: (barn, srcOrd).
fn children_of(doc: &XDoc, parent: &str) -> Vec<(String, i64)> {
    let mut v: Vec<(String, i64)> = doc.root.child("dgm:cxnLst").map(|l| l.elems().filter(|c| is_par_of(c) && c.attr("srcId").as_deref() == Some(parent)).map(|c| (c.attr("destId").unwrap_or_default(), c.attr_i64("srcOrd").unwrap_or(0))).collect()).unwrap_or_default();
    v.sort_by_key(|x| x.1);
    v
}

/// Ändrar `srcOrd` med `delta` för barnen till `parent` med `srcOrd` > `after`.
fn shift_after(doc: &mut XDoc, parent: &str, after: i64, delta: i64) -> Result<(), String> {
    for c in cxn_list(doc)?.elems_mut().filter(|c| is_par_of(c) && c.attr("srcId").as_deref() == Some(parent)) {
        let o = c.attr_i64("srcOrd").unwrap_or(0);
        if o > after {
            c.set_attr("srcOrd", &(o + delta).to_string());
        }
    }
    Ok(())
}

fn move_node(doc: &mut XDoc, pt: &str, parent: &str, ord: i64) -> Result<(), String> {
    let c = cxn_list(doc)?.elems_mut().find(|c| is_par_of(c) && c.attr("destId").as_deref() == Some(pt)).ok_or("kopplingen saknas")?;
    c.set_attr("srcId", parent);
    c.set_attr("srcOrd", &ord.to_string());
    Ok(())
}

/// Lägger till, flyttar eller tar bort noden i datamodellen och tar bort den sparade ritdelen. Ger
/// noden som redigeras sedan (den nya, den flyttade, eller den före den borttagna); `None` om inget
/// ändrades (t.ex. Tab på den första noden).
pub fn restructure(txn: &mut Txn, t: &Target, op: Op) -> Result<Option<String>, String> {
    let doc = txn.doc(&t.data_part)?;
    let (parent, ord) = parent_of(doc, &t.pt).ok_or("noden saknar förälder")?;
    let next = match op {
        Op::After => {
            let id = new_id(doc, 1);
            let par = new_id(doc, 2);
            let sib = new_id(doc, 3);
            let cxn = new_id(doc, 4);
            let dgm = doc.prefix_for(NS_DGM).map(|p| if p.is_empty() { p } else { format!("{p}:") }).unwrap_or_else(|| "dgm:".into());
            let a = doc.prefix_for(super::edit::NS_A).map(|p| if p.is_empty() { p } else { format!("{p}:") }).unwrap_or_else(|| "a:".into());
            // Den nya noden får nodens egna text- och styckeinställningar (utan text).
            let src = find(&doc.root, "dgm:pt", &t.pt).and_then(|p| p.child("dgm:t")).cloned();
            let mut tbody = match src {
                Some(t) => t,
                None => frag1(doc, &format!("<{dgm}t><{a}bodyPr/><{a}lstStyle/></{dgm}t>"))?,
            };
            // Det som skrivs i den nya noden får nodens körningsformat (första körningen), som i PowerPoint.
            let first_rpr = tbody.elems().filter(|e| e.name == "a:p").find_map(|p| p.elems().find(|r| r.name == "a:r").and_then(|r| r.child("a:rPr").cloned()));
            let end = first_rpr
                .map(|mut r| {
                    let q = if r.qname.contains(':') { format!("{}:endParaRPr", r.qname.split(':').next().unwrap_or("a")) } else { "endParaRPr".to_owned() };
                    r.rename(&q, "a:endParaRPr");
                    r.remove_attr("dirty");
                    r
                })
                .or_else(|| tbody.elems().filter(|e| e.name == "a:p").last().and_then(|p| p.child("a:endParaRPr").cloned()));
            // Egen textformatering (`custT`) följer med, annars skulle texten få motorns storlek.
            let cust = find(&doc.root, "dgm:pt", &t.pt).and_then(|p| p.child("dgm:prSet")).and_then(|s| s.attr("custT")).map(|v| format!(" custT=\"{v}\"")).unwrap_or_default();
            let ppr = tbody.elems().find(|e| e.name == "a:p").and_then(|p| p.child("a:pPr").cloned());
            tbody.remove_children("a:p");
            let mut para = frag1(doc, &format!("<{a}p/>"))?;
            para.children.extend(ppr.into_iter().map(XNode::Elem));
            let end = match end {
                Some(e) => e,
                None => frag1(doc, &format!("<{a}endParaRPr lang=\"sv-SE\"/>"))?,
            };
            para.children.push(XNode::Elem(end));
            tbody.children.push(XNode::Elem(para));
            let mut node = frag1(doc, &format!("<{dgm}pt modelId=\"{id}\"><{dgm}prSet phldrT=\"[Text]\"{cust}/><{dgm}spPr/></{dgm}pt>"))?;
            node.children.push(XNode::Elem(tbody));
            let trans = |kind: &str, tid: &str| format!("<{dgm}pt modelId=\"{tid}\" type=\"{kind}\" cxnId=\"{cxn}\"><{dgm}prSet/><{dgm}spPr/><{dgm}t><{a}bodyPr/><{a}lstStyle/><{a}p><{a}endParaRPr lang=\"sv-SE\"/></{a}p></{dgm}t></{dgm}pt>");
            let mut new_pts = vec![node];
            new_pts.extend(doc.fragment(&trans("parTrans", &par)));
            new_pts.extend(doc.fragment(&trans("sibTrans", &sib)));
            let c = frag1(doc, &format!("<{dgm}cxn modelId=\"{cxn}\" srcId=\"{parent}\" destId=\"{id}\" srcOrd=\"{}\" destOrd=\"0\" parTransId=\"{par}\" sibTransId=\"{sib}\"/>", ord + 1))?;
            shift_after(doc, &parent, ord, 1)?;
            let pts = doc.root.child_mut("dgm:ptLst").ok_or("datamodellen saknar dgm:ptLst")?;
            let at = pts.children.iter().position(|n| matches!(n, XNode::Elem(e) if e.attr("modelId").as_deref() == Some(t.pt.as_str()))).map_or(pts.children.len(), |i| i + 1);
            for (k, p) in new_pts.into_iter().enumerate() {
                pts.children.insert(at + k, XNode::Elem(p));
            }
            cxn_list(doc)?.children.push(XNode::Elem(c));
            Some(id)
        }
        Op::Demote => {
            let sibs = children_of(doc, &parent);
            let Some(prev) = sibs.iter().filter(|s| s.1 < ord).next_back().map(|s| s.0.clone()) else { return Ok(None) };
            let n = children_of(doc, &prev).last().map_or(0, |c| c.1 + 1);
            move_node(doc, &t.pt, &prev, n)?;
            shift_after(doc, &parent, ord, -1)?;
            Some(t.pt.clone())
        }
        Op::Promote => {
            let Some((grand, pord)) = parent_of(doc, &parent) else { return Ok(None) };
            shift_after(doc, &grand, pord, 1)?;
            move_node(doc, &t.pt, &grand, pord + 1)?;
            shift_after(doc, &parent, ord, -1)?;
            Some(t.pt.clone())
        }
        Op::Remove => {
            if !children_of(doc, &t.pt).is_empty() {
                return Ok(None);
            }
            let sibs = children_of(doc, &parent);
            let before = sibs.iter().filter(|s| s.1 < ord).next_back().map(|s| s.0.clone()).or_else(|| sibs.iter().find(|s| s.1 > ord).map(|s| s.0.clone()));
            // Kopplingen, övergångarna, punkten och dess presentationspunkter.
            let cx = cxn_list(doc)?.elems().find(|c| is_par_of(c) && c.attr("destId").as_deref() == Some(t.pt.as_str())).cloned();
            let mut gone: Vec<String> = vec![t.pt.clone()];
            if let Some(c) = &cx {
                gone.extend(c.attr("parTransId"));
                gone.extend(c.attr("sibTransId"));
            }
            let pts = doc.root.child("dgm:ptLst").ok_or("dgm:ptLst")?;
            let pres: Vec<String> = pts.elems().filter(|p| p.attr("type").as_deref() == Some("pres") && p.child("dgm:prSet").and_then(|s| s.attr("presAssocID")).is_some_and(|a| gone.contains(&a))).filter_map(|p| p.attr("modelId")).collect();
            gone.extend(pres);
            let cxn_id = cx.and_then(|c| c.attr("modelId"));
            doc.root.child_mut("dgm:ptLst").ok_or("dgm:ptLst")?.children.retain(|n| !matches!(n, XNode::Elem(e) if e.attr("modelId").is_some_and(|m| gone.contains(&m))));
            cxn_list(doc)?.children.retain(|n| {
                let XNode::Elem(e) = n else { return true };
                let touches = |k: &str| e.attr(k).is_some_and(|v| gone.contains(&v));
                !(touches("srcId") || touches("destId") || cxn_id.is_some() && e.attr("modelId") == cxn_id)
            });
            shift_after(doc, &parent, ord, -1)?;
            before.or(Some(parent.clone()))
        }
    };
    drop_drawing(txn, t)?;
    Ok(next)
}

/// Tar bort den sparade ritdelen: `dsp:dataModelExt` i datamodellen, bladets relation och delen (om
/// inget annat blad pekar på den). Kladds motor lägger sedan ut diagrammet; PowerPoint lägger ut det
/// igen ur datamodellen när filen öppnas.
fn drop_drawing(txn: &mut Txn, t: &Target) -> Result<(), String> {
    let Some(part) = &t.drawing_part else { return Ok(()) };
    let slide_part = edit_ops::slide_part(txn.deck, t.slide)?;
    let doc = txn.doc(&t.data_part)?;
    let mut rel_id = None;
    if let Some(ext) = doc.root.child_mut("dgm:extLst") {
        ext.children.retain(|n| {
            let XNode::Elem(e) = n else { return true };
            let mut found = None;
            e.walk(&mut |x| {
                if x.local() == "dataModelExt" {
                    found = x.attr("relId");
                }
            });
            if found.is_some() {
                rel_id = found;
                return false;
            }
            true
        });
    }
    if doc.root.child("dgm:extLst").is_some_and(|e| e.elems().next().is_none()) {
        doc.root.remove_children("dgm:extLst");
    }
    let rels = txn.rels(&slide_part);
    let rel = rel_id.and_then(|id| rels.iter().find(|r| r.id == id).cloned()).or_else(|| rels.iter().find(|r| r.kind == "diagramDrawing" && &r.target == part).cloned());
    if let Some(r) = rel {
        txn.remove_rel(&slide_part, &r.id)?;
    }
    // Delen tas bort om inget annat blad har en relation till den.
    let used_elsewhere = txn.deck.slides.iter().enumerate().any(|(i, s)| i != t.slide && s.rels.iter().any(|r| &r.target == part));
    if !used_elsewhere {
        txn.remove_part(part)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ redigeringsvyn

impl PptxTab {
    /// Börjar redigera SmartArt-noden under punkten; `false` om ingen nod finns där.
    pub(super) fn begin_smart_at(&mut self, slide: usize, x: f32, y: f32) -> bool {
        let Some(t) = target_at(&self.deck, slide, x, y) else { return false };
        if self.edit.text.as_ref().is_some_and(|ts| ts.smart.as_ref() == Some(&t)) {
            return true;
        }
        self.begin_smart(t, Some((x, y)));
        true
    }

    /// Börjar redigera noden (markören vid punkten, annars sist).
    pub(super) fn begin_smart(&mut self, t: Target, at: Option<(f32, f32)>) {
        let Some(model) = model(&self.deck, &t) else { return };
        let carets = carets(&self.deck, &t, &model);
        self.end_text();
        let mut caret = model.end();
        if let (Some(c), Some((x, y))) = (&carets, at) {
            if let Some(p) = c.hit(x, y) {
                caret = p;
            }
        }
        self.edit.sel = Some(t.sel);
        self.edit.more.clear();
        self.edit.text = Some(TextState::for_smart(t.sel, model, caret, carets, t));
    }

    /// Enter, Tab eller Delete på noden: ändrar datamodellen och börjar redigera noden som följer.
    pub(super) fn smart_op(&mut self, t: Target, op: Op) {
        self.end_text();
        let mut next = None;
        let ok = self.apply(None, |txn| {
            next = restructure(txn, &t, op)?;
            Ok(())
        });
        if !ok {
            return;
        }
        let Some(pt) = next else { return };
        // Motorn har lagt ut diagrammet igen: noden hittas i dess ritdel.
        let ds = diagrams(&self.deck, t.slide);
        let Some(d) = ds.iter().find(|d| d.data_part == t.data_part) else { return };
        let target = Target { drawing_part: None, shape_id: String::new(), pt, ..t.clone() };
        let found = shape_for(d, &target).is_some();
        drop(ds);
        if found {
            self.begin_smart(target, None);
        }
    }
}

/// Tangenten som ändrar noderna medan man skriver i en nod: Enter sist i noden (utan Skift) ger ett
/// syskon, Tab/Skift+Tab flyttar noden ned/upp en nivå och Backsteg/Delete i en tom nod tar bort den.
pub fn key_op(key: egui::Key, shift: bool, command: bool, at_end: bool, empty: bool) -> Option<Op> {
    use egui::Key;
    match key {
        Key::Enter if !shift && !command && at_end => Some(Op::After),
        Key::Tab if !command => Some(if shift { Op::Promote } else { Op::Demote }),
        Key::Backspace | Key::Delete if empty && !command => Some(Op::Remove),
        _ => None,
    }
}
