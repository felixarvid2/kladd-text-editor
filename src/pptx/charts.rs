//! Diagram (`c:chart`, `cx:chart`): bara en platshållare i den här omgången.
//!
//! Hook för nästa omgång: stapel/linje/cirkel ur `c:numCache`/`c:strCache` med den gemensamma
//! textmotorn för etiketter (se `docs/dev/pptx.md`).

use super::inherit::Owner;
use super::render::{Builder, Frame};
use super::xml::Node;

pub fn draw_chart(b: &mut Builder<'_>, _gd: &Node, f: &Frame, _owner: Owner) {
    b.placeholder_box(f, crate::i18n::t("Diagram"));
}
