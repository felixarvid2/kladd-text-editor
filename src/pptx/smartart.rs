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
use super::dgm;
use super::inherit::{self, Owner};
use super::model::Drawing;
use super::package;
use super::render::{Builder, Frame};
use super::scene::{Affine, Cap, Join, Paint, Path, Stroke};
use super::text;
use super::xml::Node;

/// Datamodellens del för ett SmartArt-objekt (`dgm:relIds@r:dm`).
pub(crate) fn data_part(rels: &[package::Rel], gd: &Node) -> Option<String> {
    gd.child("dgm:relIds")?.attr("r:dm").and_then(|id| package::rel(rels, id)).map(|r| r.target.clone())
}

/// Datamodellen för ett SmartArt-objekt.
fn data_model<'a>(b: &Builder<'a>, gd: &Node, owner: Owner) -> Option<Node> {
    b.sheets.deck.pkg.xml(&data_part(b.rels(owner), gd)?)
}

/// Nyckeln för en ritdel som layoutmotorn gjort (efter datamodellens del).
pub(crate) fn laid_out_key(data_part: &str) -> String {
    format!("{data_part}#kladd")
}

/// Har ritdelen några former? PowerPoint sparar ibland en tom `dsp:spTree`.
pub(crate) fn usable(d: &Drawing) -> bool {
    d.root.child("p:spTree").is_some_and(|t| t.children.iter().any(|c| matches!(c.name.as_str(), "p:sp" | "p:grpSp" | "p:cxnSp" | "p:pic")))
}

/// Ritdelens namn för diagrammet ur datamodellens `dsp:dataModelExt@relId` (eller den enda på bladet).
pub(crate) fn drawing_part_of(rels: &[package::Rel], data: Option<&Node>) -> Option<String> {
    let mut rel_id = None;
    if let Some(dm_xml) = data {
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
    let mut drawings = rels.iter().filter(|r| r.kind == "diagramDrawing");
    match (drawings.next(), drawings.next()) {
        (Some(d), None) => Some(d.target.clone()),
        _ => None,
    }
}

/// SmartArt-ramarna på bild `i` med gruppernas skalning (x, y).
pub fn diagram_frames(deck: &super::Deck, i: usize) -> Vec<(&Node, f64, f64)> {
    let mut frames = Vec::new();
    fn walk<'n>(n: &'n Node, sx: f64, sy: f64, out: &mut Vec<(&'n Node, f64, f64)>) {
        for c in &n.children {
            match c.name.as_str() {
                "p:grpSp" => {
                    let x = c.path(&["p:grpSpPr", "a:xfrm"]);
                    let k = |at: &str| -> Option<f64> {
                        let e = x?.child("a:ext")?.attr_f64(at)?;
                        let ch = x?.child("a:chExt")?.attr_f64(at)?;
                        (ch > 0.0).then_some(e / ch)
                    };
                    walk(c, sx * k("cx").unwrap_or(1.0), sy * k("cy").unwrap_or(1.0), out);
                }
                "p:graphicFrame" if c.path(&["a:graphic", "a:graphicData"]).is_some_and(|g| g.attr("uri").is_some_and(|u| u.ends_with("/diagram"))) => out.push((c, sx, sy)),
                _ => {}
            }
        }
    }
    if let Some(tree) = deck.slides.get(i).and_then(|s| s.root.path(&["p:cSld", "p:spTree"])) {
        walk(tree, 1.0, 1.0, &mut frames);
    }
    frames
}

/// Har diagrammet i ramen en användbar sparad ritdel?
pub fn has_drawing(deck: &super::Deck, i: usize, gf: &Node) -> bool {
    let Some(slide) = deck.slides.get(i) else { return false };
    let Some(gd) = gf.path(&["a:graphic", "a:graphicData"]) else { return false };
    let data = data_part(&slide.rels, gd).and_then(|dm| deck.pkg.xml(&dm));
    drawing_part_of(&slide.rels, data.as_ref()).and_then(|p| deck.drawing(&p)).is_some_and(usable)
}

/// Layoutmotorns ritdel för diagrammet i ramen (`sx`, `sy`: gruppernas skalning): (datamodellens
/// del, ritdelens rot).
pub fn lay_out_frame(deck: &super::Deck, i: usize, gf: &Node, sx: f64, sy: f64) -> Option<(String, Node)> {
    let slide = deck.slides.get(i)?;
    let gd = gf.path(&["a:graphic", "a:graphicData"])?;
    let ids = gd.child("dgm:relIds")?;
    let dm = data_part(&slide.rels, gd)?;
    let data = deck.pkg.xml(&dm)?;
    let part = |a: &str| ids.attr(a).and_then(|id| package::rel(&slide.rels, id)).and_then(|r| deck.pkg.xml(&r.target));
    let layout = part("r:lo")?;
    let (quick, colors) = (part("r:qs"), part("r:cs"));
    let ext = gf.path(&["p:xfrm", "a:ext"])?;
    let w = ext.attr_f64("cx").unwrap_or(0.0) / 12_700.0 * sx;
    let h = ext.attr_f64("cy").unwrap_or(0.0) / 12_700.0 * sy;
    let sheets = inherit::Sheets::for_slide(deck, i);
    let parts = dgm::Parts { data: &data, layout: &layout, quick_style: quick.as_ref(), colors: colors.as_ref() };
    dgm::lay_out(&sheets, &parts, w, h).map(|root| (dm, root))
}

/// Lägger ut SmartArt-diagram på bilderna som saknar en användbar ritdel (`dgm::lay_out`) när
/// presentationen läses in; resultatet ritas som en sparad ritdel. Ramens visade storlek
/// (med gruppernas skalning) är layoutens storlek, som för PowerPoints ritdel.
pub fn lay_out_missing(deck: &super::Deck) -> Vec<(String, Drawing)> {
    let mut out: Vec<(String, Drawing)> = Vec::new();
    for i in 0..deck.slides.len() {
        for (gf, sx, sy) in diagram_frames(deck, i) {
            if has_drawing(deck, i, gf) {
                continue;
            }
            if let Some((dm, root)) = lay_out_frame(deck, i, gf, sx, sy) {
                let key = laid_out_key(&dm);
                if !out.iter().any(|o| o.0 == key) {
                    let rels = deck.pkg.rels(&dm);
                    out.push((key, Drawing { root, rels }));
                }
            }
        }
    }
    out
}

/// Ritdelen för ett SmartArt-objekt: datamodellens `dsp:dataModelExt@relId` pekar på bladets relation.
fn drawing_part(b: &Builder<'_>, gd: &Node, owner: Owner) -> Option<String> {
    drawing_part_of(b.rels(owner), data_model(b, gd, owner).as_ref())
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

/// Avbildningen av ritdelens koordinater på bilden.
///
/// Ritdelens koordinater är relativa ramens övre vänstra hörn och i ramens **visade** storlek:
/// ligger SmartArt-ramen i en skalad grupp (`chExt` ≠ `ext`) har PowerPoint lagt ut och sparat
/// ritdelen i den storlek ramen har på bilden, så gruppens skalning gäller ramens läge och storlek
/// men inte formerna i ritdelen (kugghjulen i `msrc-win32k` bild 4–6 och 22 ligger i grupper
/// skalade 16–92 %). Grupprotation och spegling gäller fortfarande.
pub fn drawing_transform(f: &Frame) -> Affine {
    let p = &f.parent;
    let sx = p.a.hypot(p.b);
    let sy = p.c.hypot(p.d);
    let unscale = if sx.is_finite() && sy.is_finite() && sx > 1e-6 && sy > 1e-6 { Affine::scale(1.0 / sx, 1.0 / sy) } else { Affine::IDENTITY };
    unscale.then(&Affine::translate(f.x, f.y)).then(p)
}

pub fn draw_diagram<'a>(b: &mut Builder<'a>, gd: &'a Node, f: &Frame, owner: Owner) {
    // En ritdel utan former räknas som saknad; då används layoutmotorns ritdel om den finns.
    let drawing = drawing_part(b, gd, owner).and_then(|part| b.sheets.deck.drawing(&part)).filter(|d| usable(d));
    let drawing = drawing.or_else(|| data_part(b.rels(owner), gd).and_then(|dm| b.sheets.deck.drawing(&laid_out_key(&dm))));
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
    let t = drawing_transform(f);
    let saved = b.rels_override.replace(&d.rels);
    b.tree(tree, Owner::Slide, &t);
    b.rels_override = saved;
}
