//! Diagram (`c:chart`): diagramdelen läses (`chart_model.rs`) och ritas ur de cachade värdena
//! (`chart_draw.rs`). Diagram som inte kan läsas, okända typer och `cx:` (chartex) ger en platshållare.

use super::inherit::Owner;
use super::render::{Builder, Frame};
use super::xml::Node;

pub fn draw_chart(b: &mut Builder<'_>, gd: &Node, f: &Frame, owner: Owner) {
    let part = gd.child("c:chart").and_then(|c| c.attr("r:id")).and_then(|id| super::package::rel(b.rels(owner), id)).map(|r| r.target.clone());
    let root = part.and_then(|p| b.sheets.deck.pkg.xml(&p));
    let items = root.as_ref().and_then(super::chart_model::parse).map(|c| super::chart_draw::draw(b, &c, f, owner));
    match items {
        Some(items) => b.scene.items.extend(items),
        None => b.placeholder_box(f, crate::i18n::t("Diagram")),
    }
}
