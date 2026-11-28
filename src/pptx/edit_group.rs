//! Text i former inuti grupper: ett barn adresseras med gruppens läge på bilden och barnets
//! `p:cNvPr/@id`. Barnets koordinater går genom gruppens `a:xfrm` (`a:off`/`a:ext` mot `a:chOff`/
//! `a:chExt`) som `render::group` ritar dem, så markören står där texten ritas. Former i
//! `mc:AlternateContent` i en grupp redigeras inte.

use super::edit::Txn;
use super::edit_fit::{write_norm, Fit};
use super::edit_ops;
use super::edit_text::TextModel;
use super::edit_xml::{XDoc, XElem, XNode};
use super::model::Deck;
use super::render::{emu, frame_of};
use super::scene::Affine;
use super::xml::Node;

/// Barnens koordinater → bilden för gruppen `g` i en förälder med transformen `parent`
/// (samma uträkning som `render::group`).
fn child_transform(g: &Node, parent: &Affine) -> Option<Affine> {
    let Some(x) = g.child("p:grpSpPr").and_then(|p| p.child("a:xfrm")) else { return Some(*parent) };
    let f = frame_of(x, parent)?;
    let (cox, coy, cw, ch) = match (x.child("a:chOff"), x.child("a:chExt")) {
        (Some(o), Some(e)) => (emu(o.attr_i64("x").unwrap_or(0)), emu(o.attr_i64("y").unwrap_or(0)), emu(e.attr_i64("cx").unwrap_or(0)), emu(e.attr_i64("cy").unwrap_or(0))),
        _ => (0.0, 0.0, f.w, f.h),
    };
    let sx = if cw > 0.0 { f.w / cw } else { 1.0 };
    let sy = if ch > 0.0 { f.h / ch } else { 1.0 };
    Some(Affine::translate(-cox, -coy).then(&Affine::scale(sx, sy)).then(&f.t))
}

fn cnv_id(n: &Node) -> Option<i64> {
    n.children.first().and_then(|nv| nv.child("p:cNvPr")).and_then(|c| c.attr_i64("id"))
}

/// Besöker alla `p:sp` i gruppen (också i undergrupper) med transformen för deras förälder.
fn walk<'a>(g: &'a Node, ct: &Affine, f: &mut dyn FnMut(&'a Node, &Affine)) {
    for c in &g.children {
        match c.name.as_str() {
            "p:sp" => f(c, ct),
            "p:grpSp" => {
                if let Some(t) = child_transform(c, ct) {
                    walk(c, &t, f);
                }
            }
            _ => {}
        }
    }
}

fn group_of(deck: &Deck, slide: usize, index: usize) -> Option<&Node> {
    let g = edit_ops::node(deck, slide, index)?;
    (g.name == "p:grpSp").then_some(g)
}

/// Barnet `child` i gruppen på toppnivå `index` och transformen för dess förälder.
pub fn find_child(deck: &Deck, slide: usize, index: usize, child: u32) -> Option<(&Node, Affine)> {
    let g = group_of(deck, slide, index)?;
    let ct = child_transform(g, &Affine::IDENTITY)?;
    let mut found = None;
    walk(g, &ct, &mut |n, t| {
        if cnv_id(n) == Some(child as i64) {
            found = Some((n, *t));
        }
    });
    found
}

/// Översta formen med text i gruppen under punkten (bildens punkter).
pub fn child_at(deck: &Deck, slide: usize, index: usize, x: f32, y: f32) -> Option<u32> {
    let g = group_of(deck, slide, index)?;
    let ct = child_transform(g, &Affine::IDENTITY)?;
    let mut hit = None;
    walk(g, &ct, &mut |n, t| {
        let Some(f) = n.path(&["p:spPr", "a:xfrm"]).and_then(|x| frame_of(x, t)) else { return };
        let Some(inv) = f.t.invert() else { return };
        let (lx, ly) = inv.apply(x, y);
        if lx >= 0.0 && ly >= 0.0 && lx <= f.w && ly <= f.h {
            hit = cnv_id(n).and_then(|i| u32::try_from(i).ok());
        }
    });
    hit
}

/// Är formen en grupp?
pub fn is_group(deck: &Deck, slide: usize, index: usize) -> bool {
    group_of(deck, slide, index).is_some()
}

fn find_xml<'a>(g: &'a XElem, id: i64) -> Option<&'a XElem> {
    for c in g.elems() {
        match c.name.as_str() {
            "p:sp" if c.child("p:nvSpPr").and_then(|n| n.child("p:cNvPr")).and_then(|c| c.attr_i64("id")) == Some(id) => return Some(c),
            "p:grpSp" => {
                if let Some(r) = find_xml(c, id) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_path(g: &XElem, id: i64, path: &mut Vec<usize>) -> bool {
    for (i, c) in g.children.iter().enumerate() {
        let XNode::Elem(e) = c else { continue };
        path.push(i);
        let is_it = e.name == "p:sp" && e.child("p:nvSpPr").and_then(|n| n.child("p:cNvPr")).and_then(|c| c.attr_i64("id")) == Some(id);
        if is_it || (e.name == "p:grpSp" && find_path(e, id, path)) {
            return true;
        }
        path.pop();
    }
    false
}

fn find_xml_mut<'a>(g: &'a mut XElem, id: i64) -> Option<&'a mut XElem> {
    let mut path = Vec::new();
    if !find_path(g, id, &mut path) {
        return None;
    }
    let mut cur = g;
    for i in path {
        let XNode::Elem(e) = &mut cur.children[i] else { return None };
        cur = e;
    }
    Some(cur)
}

/// Barnets text som modell ur XML:en i `doc` (gruppen på toppnivå `index`), med föräldratransformen.
pub fn child_model(deck: &Deck, doc: &XDoc, slide: usize, index: usize, child: u32, a: &str) -> Option<TextModel> {
    let (_, parent) = find_child(deck, slide, index, child)?;
    let sp = find_xml(edit_ops::raw_shape(doc, index)?, child as i64)?;
    Some(TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), a).in_parent(parent))
}

/// Skriver modellen i barnets `p:txBody` (och `a:normAutofit` om `fit` är det).
pub fn set_child_text(txn: &mut Txn, slide: usize, index: usize, id: u32, child: u32, model: &TextModel, fit: Option<Fit>) -> Result<(), String> {
    let part = edit_ops::slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    if doc.prefix_for(super::edit::NS_A).is_none() && !model.prefix().is_empty() {
        doc.root.set_attr(&format!("xmlns:{}", model.prefix()), super::edit::NS_A);
    }
    edit_ops::with_shape(txn, slide, index, id, &mut |e, doc| {
        let p = doc.prefix_for(super::edit::NS_P).unwrap_or_else(|| "p".into());
        let sp = find_xml_mut(e, child as i64).ok_or("formen i gruppen finns inte längre")?;
        model.write_into(sp, &p);
        if let Some(Fit::Norm { scale, red }) = fit {
            write_norm(sp, doc, scale, red)?;
        }
        Ok(())
    })
}
