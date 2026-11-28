//! Layoutmotor för SmartArt: lägger ut ett diagram ur datamodellen (`dataN.xml`), layout-
//! definitionen (`layoutN.xml`), stildefinitionen (`quickStyleN.xml`) och färgdefinitionen
//! (`colorsN.xml`) efter ECMA-376 del 1 §21.4, för diagram utan användbar sparad ritdel.
//!
//! - `data.rs`: punkterna, `parOf`-trädet och axlarna.
//! - `tree.rs`: layoutträdet (`layoutNode`, `forEach`, `choose`/`if`, `constrLst`) och algoritmerna
//!   (`composite`, `lin`, `snake`, `cycle`, `pyra`, `hierRoot`/`hierChild`, `tx`, `sp`, `conn`).
//! - `emit.rs`: formerna som en ritdel (`p:spTree`) med stil, färger och text i anpassad storlek.
//!
//! Resultatet ritas av samma kod som PowerPoints sparade ritdel (`smartart.rs`).

pub mod data;
pub mod emit;
pub mod tree;

use super::inherit::Sheets;
use super::xml::Node;

/// Delarna för ett diagram.
pub struct Parts<'x> {
    pub data: &'x Node,
    pub layout: &'x Node,
    pub quick_style: Option<&'x Node>,
    pub colors: Option<&'x Node>,
}

/// Lägger ut diagrammet i en ram på `w`×`h` punkter (ramens visade storlek) och ger ritdelens
/// rot (`p:drawing` med `p:spTree`), eller `None` om layouten inte gav några former eller inte
/// går att läsa (se `emit::Emitter::emit`).
pub fn lay_out(sheets: &Sheets<'_>, parts: &Parts<'_>, w: f64, h: f64) -> Option<Node> {
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    // Ett fel i motorn får aldrig hindra presentationen från att öppnas: då används reserven.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let model = data::Model::new(parts.data);
        let mut t = tree::Tree::build(&model, parts.layout)?;
        t.run(w, h);
        let em = emit::Emitter { tree: &t, sheets, styles: emit::Styles { quick: parts.quick_style, colors: parts.colors } };
        let sp_tree = em.emit()?;
        if sp_tree.children.iter().all(|c| c.name != "p:sp") {
            return None;
        }
        Some(emit::el("p:drawing", &[], vec![sp_tree]))
    }))
    .ok()
    .flatten()
}
