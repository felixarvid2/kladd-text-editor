//! SmartArt (`a:graphicData uri=…/diagram`): ritar den sparade ritdelen (`ppt/diagrams/drawingN.xml`,
//! `dsp:`-former) som en grupp, som PowerPoint, POI och LibreOffice gör. Formerna ritas av samma kod
//! som bladets former: text i `dsp:txXfrm`, vridning, `dsp:style` (även textfärgen ur `a:fontRef`).
//!
//! Saknas ritdelen (vanligt i filer från PowerPoint 2007) ritas datamodellens text
//! (`ppt/diagrams/dataN.xml`) som en punktlista i en ruta, i stället för ingenting: noderna
//! (`dgm:pt` utan typ eller med `node`/`asst`) i den ordning `parOf`-kopplingarna anger, med en
//! nivå per led. PowerPoints egen layout (`layoutN.xml`) räknas inte om.

use std::collections::{HashMap, HashSet};

use super::color::Rgba;
use super::inherit::{self, Owner};
use super::package;
use super::render::{Builder, Frame};
use super::scene::{Affine, Cap, Join, Paint, Path, Stroke};
use super::text;
use super::xml::Node;

/// Datamodellen för ett SmartArt-objekt (`dgm:relIds@r:dm`).
fn data_model<'a>(b: &Builder<'a>, gd: &Node, owner: Owner) -> Option<Node> {
    let rels = b.rels(owner);
    let dm = gd.child("dgm:relIds")?.attr("r:dm").and_then(|id| package::rel(rels, id))?;
    b.sheets.deck.pkg.xml(&dm.target)
}

/// Ritdelen för ett SmartArt-objekt: datamodellens `dsp:dataModelExt@relId` pekar på bladets relation.
fn drawing_part(b: &Builder<'_>, gd: &Node, owner: Owner) -> Option<String> {
    let rels = b.rels(owner);
    let mut rel_id = None;
    if let Some(dm_xml) = data_model(b, gd, owner) {
        dm_xml.walk(&mut |n| {
            if n.local() == "dataModelExt" {
                if let Some(r) = n.attr("relId") {
                    rel_id = Some(r.to_owned());
                }
            }
        });
    }
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

/// Datamodellens text i ordning: (nivå, text). Nivå 0 är noderna direkt under dokumentet.
pub fn outline(data: &Node) -> Vec<(usize, String)> {
    let Some(pts) = data.child("dgm:ptLst") else { return Vec::new() };
    let mut text: HashMap<&str, String> = HashMap::new();
    let mut roots: Vec<&str> = Vec::new();
    for pt in pts.children_named("dgm:pt") {
        let Some(id) = pt.attr("modelId") else { continue };
        match pt.attr("type").unwrap_or("node") {
            "doc" => roots.push(id),
            "node" | "asst" => {
                let lines: Vec<String> = pt
                    .child("dgm:t")
                    .map(|t| t.children_named("a:p").map(|p| p.children.iter().filter(|r| matches!(r.name.as_str(), "a:r" | "a:fld")).filter_map(|r| r.child("a:t")).map(|t| t.text.as_str()).collect::<String>()).filter(|s| !s.trim().is_empty()).collect())
                    .unwrap_or_default();
                text.insert(id, lines.join("\n"));
            }
            _ => {}
        }
    }
    // `parOf`-kopplingar (utan typ): förälder → barn i `srcOrd`-ordning.
    let mut kids: HashMap<&str, Vec<(i64, &str)>> = HashMap::new();
    if let Some(cxns) = data.child("dgm:cxnLst") {
        for c in cxns.children_named("dgm:cxn") {
            if c.attr("type").is_some_and(|t| t != "parOf") {
                continue;
            }
            if let (Some(s), Some(d)) = (c.attr("srcId"), c.attr("destId")) {
                kids.entry(s).or_default().push((c.attr_i64("srcOrd").unwrap_or(0), d));
            }
        }
    }
    for v in kids.values_mut() {
        v.sort_by_key(|k| k.0);
    }
    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    fn visit<'n>(id: &'n str, depth: usize, kids: &HashMap<&'n str, Vec<(i64, &'n str)>>, text: &HashMap<&'n str, String>, seen: &mut HashSet<&'n str>, out: &mut Vec<(usize, String)>) {
        if !seen.insert(id) || depth > 16 {
            return;
        }
        for (_, k) in kids.get(id).map(Vec::as_slice).unwrap_or(&[]) {
            if let Some(t) = text.get(k) {
                if !t.is_empty() {
                    out.push((depth, t.clone()));
                }
                visit(k, depth + 1, kids, text, seen, out);
            }
        }
    }
    for r in &roots {
        visit(r, 0, &kids, &text, &mut seen, &mut out);
    }
    out
}

/// Reserv utan ritdel: datamodellens text som punktlista i en ruta med temats accentfärg.
fn draw_outline(b: &mut Builder<'_>, items: &[(usize, String)], f: &Frame) {
    let accent = b.sheets.colors.scheme_color("accent1").unwrap_or(Rgba { r: 0.27, g: 0.45, b: 0.77, a: 1.0 });
    let ink = b.sheets.colors.scheme_color("tx1").unwrap_or(Rgba::BLACK);
    let tint = Rgba { r: accent.r + (1.0 - accent.r) * 0.85, g: accent.g + (1.0 - accent.g) * 0.85, b: accent.b + (1.0 - accent.b) * 0.85, a: 1.0 };
    let rect = Path::rect(0.0, 0.0, f.w, f.h).transformed(&f.t);
    b.scene.fill(rect.clone(), Paint::Solid(tint));
    b.scene.stroke(rect, Stroke { paint: Paint::Solid(accent), width: 1.0, dash: Vec::new(), cap: Cap::Butt, join: Join::Miter });
    let latin = inherit::theme_font(b.sheets.theme, "+mn-lt", None);
    let frame_at = |size: f32| {
        let paras = items
            .iter()
            .flat_map(|(lvl, t)| t.split('\n').map(move |line| (*lvl, line)))
            .map(|(lvl, line)| {
                let style = text::RunStyle { latin: latin.clone(), size, color: ink, bold: lvl == 0, ..Default::default() };
                let bullet = (lvl > 0).then(|| text::Bullet { text: "•".into(), style: style.clone() });
                let ind = size * 0.9;
                text::Para {
                    runs: vec![text::Run { text: line.to_owned(), style: style.clone(), line_break: false }],
                    mar_l: ind * lvl as f32,
                    indent: if lvl > 0 { -ind * 0.8 } else { 0.0 },
                    spc_bef: text::Spacing::Pts(if lvl == 0 { size * 0.3 } else { 0.0 }),
                    bullet,
                    end_style: style,
                    ..Default::default()
                }
            })
            .collect();
        text::Frame { paras, insets: (7.2, 3.6, 7.2, 3.6), anchor: text::Anchor::Middle, wrap: true, ..Default::default() }
    };
    // Största storleken (18 pt nedåt) där texten får plats.
    let mut size = 18.0;
    let mut fr = frame_at(size);
    while size > 6.0 && text::layout(&fr, f.w, 1.0e6, &b.sheets.deck.fonts).height + 7.2 > f.h {
        size *= 0.9;
        fr = frame_at(size);
    }
    let laid = text::layout(&fr, f.w, f.h, &b.sheets.deck.fonts);
    b.scene.items.extend(text::transform_items(laid.items, &f.t));
}

/// Rättar textfärgen i en sparad ritdel efter datamodellen; anropas när ritdelen läses in.
///
/// Ritdelen slår ihop en nod och dess text till en form med formens `a:fontRef`-färg (ofta `lt1`).
/// Visar layouten texten i en egen textpunkt (`presOf` till en annan presentationspunkt, t.ex.
/// `revTx` i pyramiden) ritar PowerPoint texten med den punktens färg ur färgdefinitionen
/// (`dgm:txFillClrLst`, för `revTx` `tx1`). Den färgen skrivs då in i formens `a:fontRef`.
pub fn prepare_drawing(pkg: &package::Package, part: &str, root: &mut Node) {
    let mut ids: HashSet<String> = HashSet::new();
    root.walk(&mut |n| {
        if n.name == "p:sp" {
            if let Some(id) = n.attr("modelId") {
                ids.insert(id.to_owned());
            }
        }
    });
    if ids.is_empty() {
        return;
    }
    // Datamodellen som hör till ritdelen: den vars presentationspunkter har formernas id.
    // Delen med samma nummer (`drawing3.xml` ↔ `data3.xml`) provas först, så att varje del oftast tolkas en gång.
    let number: String = part.rsplit('/').next().unwrap_or("").chars().filter(char::is_ascii_digit).collect();
    let by_number = |prefix: &str| {
        let mut names: Vec<&String> = pkg.parts.keys().filter(|k| k.starts_with(prefix) && k.ends_with(".xml")).collect();
        let same = format!("{prefix}{number}.xml");
        names.sort_by_key(|k| (**k != same, (*k).clone()));
        names
    };
    let names = by_number("ppt/diagrams/data");
    // Bara delar som nämner en av formerna (eller färgdefinitionens id) tolkas.
    let has = |raw: &[u8], needle: &str| !needle.is_empty() && raw.windows(needle.len()).any(|w| w == needle.as_bytes());
    let mentions = |part: &str, needle: &str| pkg.read(part).is_some_and(|raw| has(raw, needle));
    let Some(data) = names.iter().filter(|n| pkg.read(n).is_some_and(|raw| ids.iter().any(|id| has(raw, id)))).filter_map(|n| pkg.xml(n)).find(|d| d.child("dgm:ptLst").is_some_and(|l| l.children.iter().any(|p| p.attr("type") == Some("pres") && p.attr("modelId").is_some_and(|m| ids.contains(m))))) else {
        return;
    };
    let Some(pts) = data.child("dgm:ptLst") else { return };
    // Presentationspunkt → (nod, stilnamn).
    let mut pres: HashMap<&str, (&str, &str)> = HashMap::new();
    let mut colors_id = None;
    for p in pts.children_named("dgm:pt") {
        let set = p.child("dgm:prSet");
        match p.attr("type") {
            Some("pres") => {
                if let (Some(id), Some(s)) = (p.attr("modelId"), set) {
                    pres.insert(id, (s.attr("presAssocID").unwrap_or(""), s.attr("presStyleLbl").unwrap_or("")));
                }
            }
            Some("doc") => colors_id = set.and_then(|s| s.attr("csTypeId")),
            _ => {}
        }
    }
    let mut pres_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in data.child("dgm:cxnLst").map(|l| l.children_named("dgm:cxn").collect::<Vec<_>>()).unwrap_or_default() {
        if c.attr("type") == Some("presOf") {
            if let (Some(s), Some(d)) = (c.attr("srcId"), c.attr("destId")) {
                pres_of.entry(s).or_default().push(d);
            }
        }
    }
    // Färgdefinitionen med samma id som datamodellens `csTypeId`.
    let Some(colors_id) = colors_id else { return };
    let Some(colors) = by_number("ppt/diagrams/colors").into_iter().filter(|k| mentions(k, colors_id)).filter_map(|k| pkg.xml(k)).find(|c| c.attr("uniqueId") == Some(colors_id)) else {
        return;
    };
    let text_color = |label: &str| -> Option<Node> {
        let lbl = colors.children_named("dgm:styleLbl").find(|s| s.attr("name") == Some(label))?;
        super::color::first_color(lbl.child("dgm:txFillClrLst")?).cloned()
    };
    fn each_sp(n: &mut Node, f: &mut dyn FnMut(&mut Node)) {
        if n.name == "p:sp" {
            f(n);
        }
        for c in &mut n.children {
            each_sp(c, f);
        }
    }
    each_sp(root, &mut |sp| {
        let Some(id) = sp.attr("modelId").map(str::to_owned) else { return };
        let Some((node, _)) = pres.get(id.as_str()) else { return };
        let override_color = pres_of.get(node).into_iter().flatten().filter(|t| **t != id).filter_map(|t| pres.get(t)).find_map(|(_, label)| text_color(label));
        let Some(c) = override_color else { return };
        if let Some(fr) = sp.children.iter_mut().find(|n| n.name == "p:style").and_then(|s| s.children.iter_mut().find(|n| n.name == "a:fontRef")) {
            fr.children = vec![c];
        }
    });
}

pub fn draw_diagram<'a>(b: &mut Builder<'a>, gd: &'a Node, f: &Frame, owner: Owner) {
    let drawing = drawing_part(b, gd, owner).and_then(|part| b.sheets.deck.drawing(&part));
    let Some(d) = drawing else {
        let items = data_model(b, gd, owner).map(|dm| outline(&dm)).unwrap_or_default();
        if items.is_empty() {
            b.placeholder_box(f, crate::i18n::t("SmartArt"));
        } else {
            draw_outline(b, &items, f);
        }
        return;
    };
    let Some(tree) = d.root.child("p:spTree") else { return };
    // Ritdelens koordinater är relativa ramens övre vänstra hörn.
    let t = Affine::translate(f.x, f.y).then(&f.parent);
    let saved = b.rels_override.replace(&d.rels);
    b.tree(tree, Owner::Slide, &t);
    b.rels_override = saved;
}
