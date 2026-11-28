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
        let styles = || emit::Styles { quick: parts.quick_style, colors: parts.colors };
        let emitter = |t: &tree::Tree<'_>, grow_mode: bool| -> Option<(Node, std::collections::HashMap<usize, f64>)> {
            let em = emit::Emitter { tree: t, sheets, styles: styles(), grow_mode, needs: Default::default() };
            let n = em.emit()?;
            Some((n, em.needs.into_inner()))
        };
        let mut t = tree::Tree::build(&model, parts.layout)?;
        let growing = (0..t.nodes.len()).any(|i| t.grows_with_text(i));
        let sp_tree = match growing.then(|| grow_and_shrink(&model, parts, w, h, &emitter)).flatten() {
            Some(n) => n,
            None => {
                t.run(w, h);
                emitter(&t, false)?.0
            }
        };
        if sp_tree.children.iter().all(|c| c.name != "p:sp") {
            return None;
        }
        Some(emit::el("p:drawing", &[], vec![sp_tree]))
    }))
    .ok()
    .flatten()
}

/// Rutor som växer med texten (`rule type="h" val="INF"`, t.ex. `vList2` och organisationsschemats
/// rutor): teckenstorleken och höjderna bestäms tillsammans. För en gemensam storlek (villkorens
/// `primFontSz`/`secFontSz` gånger `k`) läggs diagrammet ut, textrutorna växer till den höjd texten
/// behöver och diagrammet läggs ut igen; den största storleken (i halva punkter, ned till regelns minsta
/// eller 5 pt) där allt ryms i ramen utan krympning väljs. `None` om ingen storlek räcker (då krymps
/// diagrammet som förut).
#[allow(clippy::type_complexity)]
fn grow_and_shrink(
    model: &data::Model<'_>,
    parts: &Parts<'_>,
    w: f64,
    h: f64,
    emitter: &dyn Fn(&tree::Tree<'_>, bool) -> Option<(Node, std::collections::HashMap<usize, f64>)>,
) -> Option<Node> {
    // Utgångsstorleken: den största primära storleken bland rutorna som växer.
    let first = tree::Tree::build(model, parts.layout)?;
    let mut probe = first;
    probe.no_shrink = true;
    probe.run(w, h);
    let ids: Vec<usize> = (0..probe.nodes.len()).filter(|i| probe.grows_with_text(*i)).collect();
    let start = ids.iter().filter_map(|i| probe.bounded(*i, "primFontSz")).fold(0.0, f64::max);
    if start <= 0.0 {
        return None;
    }
    let min = ids.iter().flat_map(|i| probe.rules_for(*i)).filter(|r| r.attr("type") == Some("primFontSz")).filter_map(|r| r.attr_f64("val")).fold(5.0f64, f64::max).min(start);
    let attempt = |size: f64| -> Option<(tree::Tree<'_>, bool)> {
        let k = size / start;
        let mut t = tree::Tree::build(model, parts.layout)?;
        t.font_k = k;
        t.no_shrink = true;
        t.run(w, h);
        let (_, mut needs) = emitter(&t, true)?;
        // Rutor vars höjd ska vara lika (`h` med `op="equ"`) får den största höjden i gruppen.
        let mut group_max: std::collections::HashMap<(usize, usize), f64> = std::collections::HashMap::new();
        for (id, v) in &needs {
            if let Some(g) = t.nodes[*id].font_groups.get("h") {
                let e = group_max.entry(*g).or_insert(0.0);
                *e = e.max(*v);
            }
        }
        for (i, n) in t.nodes.iter().enumerate() {
            if let Some(m) = n.font_groups.get("h").and_then(|g| group_max.get(g)) {
                needs.insert(i, *m);
            }
        }
        let mut t2 = tree::Tree::build(model, parts.layout)?;
        t2.font_k = k;
        t2.no_shrink = true;
        t2.grow = needs;
        t2.run(w, h);
        let fits = t2.fits_frame(w, h);
        Some((t2, fits))
    };
    // Halva punkter från utgångsstorleken nedåt; binärsökning efter den största som ryms.
    let steps = ((start - min) * 2.0).floor().max(0.0) as i64;
    let size_at = |i: i64| ((start * 2.0).floor() - i as f64) / 2.0;
    let (mut lo, mut hi) = (0i64, steps);
    if !attempt(size_at(hi))?.1 {
        return None;
    }
    while lo < hi {
        let mid = (lo + hi) / 2;
        if attempt(size_at(mid))?.1 {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let (t, _) = attempt(size_at(lo))?;
    emitter(&t, true).map(|x| x.0)
}
