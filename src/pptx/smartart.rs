//! SmartArt (`a:graphicData uri=…/diagram`): ritar den sparade ritdelen (`ppt/diagrams/drawingN.xml`,
//! `dsp:`-former) som en grupp, som PowerPoint, POI och LibreOffice gör. Saknas ritdelen (vanligt i
//! filer från PowerPoint 2007) ritas en platshållare.
//!
//! Hook för nästa omgång: omlayout från datamodellen och bättre reserv utan ritdel.

use super::inherit::Owner;
use super::package;
use super::render::{Builder, Frame};
use super::scene::Affine;
use super::xml::Node;

/// Ritdelen för ett SmartArt-objekt: datamodellens `dsp:dataModelExt@relId` pekar på bladets relation.
fn drawing_part(b: &Builder<'_>, gd: &Node, owner: Owner) -> Option<String> {
    let rels = b.rels(owner);
    let ids = gd.child("dgm:relIds")?;
    let dm = ids.attr("r:dm").and_then(|id| package::rel(rels, id))?;
    let dm_xml = b.sheets.deck.pkg.xml(&dm.target)?;
    let mut rel_id = None;
    dm_xml.walk(&mut |n| {
        if n.local() == "dataModelExt" {
            if let Some(r) = n.attr("relId") {
                rel_id = Some(r.to_owned());
            }
        }
    });
    if let Some(r) = rel_id.and_then(|id| package::rel(rels, &id).map(|r| r.target.clone())) {
        return Some(r);
    }
    // Reserv: den enda ritdelen på bladet.
    let mut drawings = rels.iter().filter(|r| r.kind == "diagramDrawing");
    match (drawings.next(), drawings.next()) {
        (Some(d), None) => Some(d.target.clone()),
        _ => None,
    }
}

pub fn draw_diagram<'a>(b: &mut Builder<'a>, gd: &'a Node, f: &Frame, owner: Owner) {
    let Some(part) = drawing_part(b, gd, owner) else {
        b.placeholder_box(f, crate::i18n::t("SmartArt"));
        return;
    };
    let Some(d) = b.sheets.deck.drawing(&part) else {
        b.placeholder_box(f, crate::i18n::t("SmartArt"));
        return;
    };
    let Some(tree) = d.root.child("p:spTree") else { return };
    // Ritdelens koordinater är relativa ramens övre vänstra hörn.
    let t = Affine::translate(f.x, f.y).then(&f.parent);
    let saved = b.rels_override.replace(&d.rels);
    b.tree(tree, Owner::Slide, &t);
    b.rels_override = saved;
}
