//! Bygger en bilds `Scene`: bakgrund, mallens och layoutens former, bildens former (grupper,
//! bilder, linjer, text) och ramar (tabeller, SmartArt, diagram via hookarna i `tables.rs`,
//! `smartart.rs` och `charts.rs`).

use std::sync::Arc;

use super::color::{self, Rgba};
use super::geom::{self, FillMode};
use super::inherit::{self, Owner, Sheets};
use super::model::{Deck, EMU_PER_PT};
use super::package;
use super::richtext::{self, TextEnv};
use super::scene::{Affine, Cap, Item, Join, Paint, Path, Scene, Stop, Stroke};
use super::text;
use super::xml::Node;

pub(crate) fn emu(v: i64) -> f32 {
    (v as f64 / EMU_PER_PT) as f32
}

/// En forms placering: transform från lokala koordinater (punkter, 0..w × 0..h) till bilden.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub t: Affine,
    pub w: f32,
    pub h: f32,
    pub rot: f32,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Föräldrarnas transform (för text som inte ska speglas).
    pub parent: Affine,
    pub x: f32,
    pub y: f32,
}

/// `a:xfrm` → placering i förälderns koordinater.
pub fn frame_of(xfrm: &Node, parent: &Affine) -> Option<Frame> {
    let off = xfrm.child("a:off")?;
    let ext = xfrm.child("a:ext")?;
    let x = emu(off.attr_i64("x").unwrap_or(0));
    let y = emu(off.attr_i64("y").unwrap_or(0));
    let w = emu(ext.attr_i64("cx").unwrap_or(0)).max(0.0);
    let h = emu(ext.attr_i64("cy").unwrap_or(0)).max(0.0);
    let rot = xfrm.attr_i64("rot").unwrap_or(0) as f32 / 60000.0;
    let flip_h = xfrm.attr_bool("flipH").unwrap_or(false);
    let flip_v = xfrm.attr_bool("flipV").unwrap_or(false);
    let mut t = Affine::IDENTITY;
    if flip_h || flip_v {
        t = t.then(&Affine::translate(-w / 2.0, -h / 2.0)).then(&Affine::scale(if flip_h { -1.0 } else { 1.0 }, if flip_v { -1.0 } else { 1.0 })).then(&Affine::translate(w / 2.0, h / 2.0));
    }
    if rot != 0.0 {
        t = t.then(&Affine::rotate_at(rot, w / 2.0, h / 2.0));
    }
    t = t.then(&Affine::translate(x, y)).then(parent);
    Some(Frame { t, w, h, rot, flip_h, flip_v, parent: *parent, x, y })
}

/// Transformen för text i en form: roteras med formen men speglas inte (vänd lodrätt = 180°).
fn text_affine(f: &Frame, extra_rot: f32) -> Affine {
    let mut rot = f.rot + extra_rot;
    if f.flip_v {
        rot += 180.0;
    }
    Affine::rotate_at(rot, f.w / 2.0, f.h / 2.0).then(&Affine::translate(f.x, f.y)).then(&f.parent)
}

/// Fyllning för gruppformer (`a:grpFill`).
#[derive(Clone)]
struct GroupFill<'a> {
    node: Option<&'a Node>,
    owner: Owner,
}

/// Bygger visningslistan för bild `idx`.
pub fn slide_scene(deck: &Deck, idx: usize) -> Scene {
    let (w, h) = deck.size_pt();
    let mut b = Builder { sheets: Sheets::for_slide(deck, idx), scene: Scene::new(w, h), slide_no: deck.slide_number(idx), group_fill: Vec::new(), rels_override: None, bg_paint: None };
    b.background();
    let slide_shows_master = b.sheets.slide.is_none_or(|s| s.root.attr("showMasterSp") != Some("0"));
    let layout_shows_master = b.sheets.layout.is_none_or(|l| l.root.attr("showMasterSp") != Some("0"));
    if slide_shows_master && layout_shows_master {
        if let Some(t) = b.sheets.tree(Owner::Master) {
            b.tree(t, Owner::Master, &Affine::IDENTITY);
        }
    }
    if slide_shows_master {
        if let Some(t) = b.sheets.tree(Owner::Layout) {
            b.tree(t, Owner::Layout, &Affine::IDENTITY);
        }
    }
    if let Some(t) = b.sheets.tree(Owner::Slide) {
        b.tree(t, Owner::Slide, &Affine::IDENTITY);
    }
    b.scene
}

pub struct Builder<'a> {
    pub sheets: Sheets<'a>,
    pub scene: Scene,
    pub slide_no: i64,
    group_fill: Vec<GroupFill<'a>>,
    /// Relationer som gäller i stället för bladets (SmartArts ritdel har egna).
    pub rels_override: Option<&'a [package::Rel]>,
    /// Bildens bakgrund (för `useBgFill`).
    bg_paint: Option<Paint>,
}

impl<'a> Builder<'a> {
    /// Relationerna för former från `owner`.
    pub fn rels(&self, owner: Owner) -> &'a [package::Rel] {
        self.rels_override.unwrap_or_else(|| self.sheets.rels(owner))
    }

    fn background(&mut self) {
        let (w, h) = (self.scene.width, self.scene.height);
        let page = Path::rect(0.0, 0.0, w, h);
        // Vit grund (som PowerPoint när bakgrund saknas).
        self.scene.fill(page.clone(), Paint::Solid(Rgba::WHITE));
        let Some((bg, owner)) = inherit::background(&self.sheets) else { return };
        let frame = Frame { t: Affine::IDENTITY, w, h, rot: 0.0, flip_h: false, flip_v: false, parent: Affine::IDENTITY, x: 0.0, y: 0.0 };
        if let Some(pr) = bg.child("p:bgPr") {
            if let Some(f) = pr.children.iter().find(|c| inherit::FILLS.contains(&c.name.as_str())) {
                if let Some(p) = self.paint(f, None, &frame, owner) {
                    self.bg_paint = Some(p.clone());
                    self.fill_paint(page, p);
                }
            }
        } else if let Some(r) = bg.child("p:bgRef") {
            let ph = color::first_color(r).and_then(|c| self.sheets.color(c, None));
            if let Some(sr) = inherit::theme_fill(&self.sheets, r.attr_i64("idx").unwrap_or(0), ph) {
                if let Some(p) = self.paint(sr.node, sr.ph, &frame, owner) {
                    self.bg_paint = Some(p.clone());
                    self.fill_paint(page, p);
                } else if let Some(c) = ph {
                    self.bg_paint = Some(Paint::Solid(c));
                    self.scene.fill(page, Paint::Solid(c));
                }
            } else if let Some(c) = ph {
                self.scene.fill(page, Paint::Solid(c));
            }
        }
    }

    /// Fyller en bana; bildfyllningar klipps till banan.
    fn fill_paint(&mut self, path: Path, paint: Paint) {
        if let Paint::Image { image, t, alpha, .. } = &paint {
            self.scene.items.push(Item::PushClip { path: path.clone() });
            self.scene.items.push(Item::Image { image: image.clone(), t: *t, crop: [0.0; 4], alpha: *alpha });
            self.scene.items.push(Item::PopClip);
            return;
        }
        self.scene.fill(path, paint);
    }

    /// Ritar ett formträd. `parent` avbildar trädets koordinater på bilden.
    pub fn tree(&mut self, tree: &'a Node, owner: Owner, parent: &Affine) {
        for c in &tree.children {
            // Platshållare på mall och layout ritas inte (bara deras utseende ärvs).
            if owner != Owner::Slide && inherit::placeholder(c).is_some() {
                continue;
            }
            // Dolda former (`p:cNvPr hidden="1"`).
            if c.children.first().and_then(|nv| nv.child("p:cNvPr")).is_some_and(|n| n.attr_bool("hidden") == Some(true)) {
                continue;
            }
            match c.name.as_str() {
                "p:sp" | "p:cxnSp" => self.shape(c, owner, parent),
                "p:pic" => self.picture(c, owner, parent),
                "p:grpSp" => self.group(c, owner, parent),
                "p:graphicFrame" => self.graphic_frame(c, owner, parent),
                _ => {}
            }
        }
    }

    fn group(&mut self, g: &'a Node, owner: Owner, parent: &Affine) {
        let Some(gpr) = g.child("p:grpSpPr") else { return };
        let child_t = match gpr.child("a:xfrm") {
            Some(x) => {
                let Some(f) = frame_of(x, parent) else { return };
                let (cox, coy, cw, ch) = match (x.child("a:chOff"), x.child("a:chExt")) {
                    (Some(o), Some(e)) => (
                        emu(o.attr_i64("x").unwrap_or(0)),
                        emu(o.attr_i64("y").unwrap_or(0)),
                        emu(e.attr_i64("cx").unwrap_or(0)),
                        emu(e.attr_i64("cy").unwrap_or(0)),
                    ),
                    _ => (0.0, 0.0, f.w, f.h),
                };
                let sx = if cw > 0.0 { f.w / cw } else { 1.0 };
                let sy = if ch > 0.0 { f.h / ch } else { 1.0 };
                Affine::translate(-cox, -coy).then(&Affine::scale(sx, sy)).then(&f.t)
            }
            None => *parent,
        };
        let fill = gpr.children.iter().find(|c| inherit::FILLS.contains(&c.name.as_str()));
        self.group_fill.push(GroupFill { node: fill, owner });
        self.tree(g, owner, &child_t);
        self.group_fill.pop();
    }

    /// Kedjans första `spPr`-barn med ett av namnen.
    fn sppr_child(chain: &[&'a Node], names: &[&str]) -> Option<&'a Node> {
        chain.iter().find_map(|s| s.child("p:spPr").and_then(|p| p.children.iter().find(|c| names.contains(&c.name.as_str()))))
    }

    fn shape(&mut self, sp: &'a Node, owner: Owner, parent: &Affine) {
        let chain = inherit::shape_chain(&self.sheets, sp, owner);
        let is_ph = inherit::placeholder(sp).is_some();
        // Tomma platshållare syns inte i bildspel.
        let body = sp.child("p:txBody");
        let has_text = body.is_some_and(|b| b.children_named("a:p").any(|p| p.children.iter().any(|r| matches!(r.name.as_str(), "a:r" | "a:fld") && r.child("a:t").is_some_and(|t| !t.text.is_empty()))));
        if is_ph && owner == Owner::Slide && !has_text {
            return;
        }
        let Some(xfrm) = Self::sppr_child(&chain, &["a:xfrm"]) else { return };
        let Some(f) = frame_of(xfrm, parent) else { return };
        let style = chain.iter().find_map(|s| s.child("p:style"));
        let geom = self.geometry(&chain, f.w, f.h);
        self.draw_geometry(&chain, style, &f, &geom, owner);
        if let Some(body) = body.filter(|_| has_text) {
            let mut tr = geom.as_ref().map(|g| g.text_rect).unwrap_or((0.0, 0.0, f.w, f.h));
            // SmartArt-ritningar har en egen textram (`dsp:txXfrm`).
            let mut tf = f;
            // Dess vridning är relativ formens (formen 90°, texten −90° ger vågrät text).
            if let Some(tx) = sp.child("p:txXfrm").and_then(|x| frame_of(x, parent)) {
                tf = Frame { rot: f.rot + tx.rot, flip_h: false, flip_v: false, ..tx };
                tr = (0.0, 0.0, tx.w, tx.h);
            }
            self.text_body(sp, &chain, body, style, &tf, tr, owner);
        }
    }

    /// Formens geometri (förinställd eller egen), eller en rektangel.
    fn geometry(&self, chain: &[&'a Node], w: f32, h: f32) -> Option<geom::Geometry> {
        let g = Self::sppr_child(chain, &["a:prstGeom", "a:custGeom"]);
        match g {
            Some(n) if n.name == "a:prstGeom" => {
                let name = n.attr("prst").unwrap_or("rect");
                geom::preset(name, n.child("a:avLst"), w as f64, h as f64).or_else(|| geom::preset("rect", None, w as f64, h as f64))
            }
            Some(n) => Some(geom::evaluate(n, None, w as f64, h as f64)),
            None => geom::preset("rect", None, w as f64, h as f64),
        }
    }

    fn draw_geometry(&mut self, chain: &[&'a Node], style: Option<&'a Node>, f: &Frame, geom: &Option<geom::Geometry>, owner: Owner) {
        let Some(geom) = geom else { return };
        // Fyllning.
        let fill_node = Self::sppr_child(chain, inherit::FILLS);
        let use_bg = chain.first().is_some_and(|s| s.attr_bool("useBgFill") == Some(true));
        let paint = match fill_node {
            Some(n) => self.paint(n, None, f, owner),
            None if use_bg => self.bg_paint.clone().or(Some(Paint::Solid(Rgba::WHITE))),
            None => inherit::style_fill(&self.sheets, style).and_then(|sr| self.paint(sr.node, sr.ph, f, owner)),
        };
        if let Some(paint) = paint {
            for sp in geom.paths.iter().filter(|p| p.fill != FillMode::None) {
                let p = match (&paint, sp.fill) {
                    (Paint::Solid(c), FillMode::Darken) => Paint::Solid(scale_lum(*c, 0.6)),
                    (Paint::Solid(c), FillMode::DarkenLess) => Paint::Solid(scale_lum(*c, 0.8)),
                    (Paint::Solid(c), FillMode::Lighten) => Paint::Solid(lighten(*c, 0.4)),
                    (Paint::Solid(c), FillMode::LightenLess) => Paint::Solid(lighten(*c, 0.2)),
                    (p, _) => p.clone(),
                };
                self.fill_paint(sp.path.transformed(&f.t), p);
            }
        }
        // Linje.
        if let Some(stroke) = self.line(chain, style, f, owner) {
            for sp in geom.paths.iter().filter(|p| p.stroke) {
                let path = sp.path.transformed(&f.t);
                self.arrows(chain, &path, &stroke);
                self.scene.stroke(path, stroke.clone());
            }
        }
    }

    /// Linjen med arv: formens `a:ln` (och platshållarnas) över temats `lnRef`.
    fn line(&self, chain: &[&'a Node], style: Option<&'a Node>, f: &Frame, owner: Owner) -> Option<Stroke> {
        let mut lns: Vec<(&Node, Option<Rgba>)> = chain.iter().filter_map(|s| s.path(&["p:spPr", "a:ln"])).map(|n| (n, None)).collect();
        if let Some(sr) = inherit::style_line(&self.sheets, style) {
            lns.push((sr.node, sr.ph));
        }
        if lns.is_empty() {
            return None;
        }
        let fill = lns.iter().find_map(|(n, ph)| n.children.iter().find(|c| inherit::FILLS.contains(&c.name.as_str())).map(|c| (c, *ph)));
        let (fill, ph) = fill?;
        let paint = self.paint(fill, ph, f, owner)?;
        let width = lns.iter().find_map(|(n, _)| n.attr_i64("w")).map_or(0.75, emu).max(0.0);
        let width = if width == 0.0 { 0.5 } else { width };
        let dash_name = lns.iter().find_map(|(n, _)| n.child("a:prstDash").and_then(|d| d.attr("val"))).unwrap_or("solid");
        let cap = match lns.iter().find_map(|(n, _)| n.attr("cap")) {
            Some("rnd") => Cap::Round,
            Some("sq") => Cap::Square,
            _ => Cap::Butt,
        };
        let join = match lns.iter().find_map(|(n, _)| n.children.iter().find(|c| matches!(c.name.as_str(), "a:round" | "a:bevel" | "a:miter")).map(|c| c.name.as_str())) {
            Some("a:bevel") => Join::Bevel,
            Some("a:miter") => Join::Miter,
            _ => Join::Round,
        };
        let pattern: &[f32] = match dash_name {
            "dot" => &[1.0, 3.0],
            "dash" => &[4.0, 3.0],
            "lgDash" => &[8.0, 3.0],
            "dashDot" => &[4.0, 3.0, 1.0, 3.0],
            "lgDashDot" => &[8.0, 3.0, 1.0, 3.0],
            "lgDashDotDot" => &[8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
            "sysDash" => &[3.0, 1.0],
            "sysDot" => &[1.0, 1.0],
            "sysDashDot" => &[3.0, 1.0, 1.0, 1.0],
            "sysDashDotDot" => &[3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            _ => &[],
        };
        let dash = pattern.iter().map(|v| v * width).collect();
        Some(Stroke { paint, width, dash, cap, join })
    }

    /// Pilspetsar (`a:headEnd`/`a:tailEnd`) som fyllda trianglar.
    fn arrows(&mut self, chain: &[&'a Node], path: &Path, stroke: &Stroke) {
        let ln: Vec<&Node> = chain.iter().filter_map(|s| s.path(&["p:spPr", "a:ln"])).collect();
        let end = |name: &str| ln.iter().find_map(|n| n.child(name)).filter(|e| e.attr("type").is_some_and(|t| t != "none"));
        let size = |e: &Node, a: &str| match e.attr(a) {
            Some("sm") => 2.0,
            Some("lg") => 5.0,
            _ => 3.0,
        };
        let pts: Vec<(f32, f32)> = path
            .cmds
            .iter()
            .filter_map(|c| match *c {
                super::scene::Cmd::Move(x, y) | super::scene::Cmd::Line(x, y) => Some((x, y)),
                super::scene::Cmd::Cubic(_, _, _, _, x, y) | super::scene::Cmd::Quad(_, _, x, y) => Some((x, y)),
                _ => None,
            })
            .collect();
        if pts.len() < 2 {
            return;
        }
        let color = stroke.paint.average();
        let mut draw = |tip: (f32, f32), from: (f32, f32), e: &Node| {
            let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-3 {
                return;
            }
            let (ux, uy) = (dx / len, dy / len);
            let l = size(e, "len") * stroke.width.max(1.0);
            let w = size(e, "w") * stroke.width.max(1.0) / 2.0;
            let base = (tip.0 - ux * l, tip.1 - uy * l);
            let mut p = Path::default();
            match e.attr("type") {
                Some("oval") => {
                    let r = w;
                    let c = (tip.0 - ux * r, tip.1 - uy * r);
                    p = ellipse(c.0, c.1, r, r);
                }
                Some("diamond") => {
                    let mid = (tip.0 - ux * l / 2.0, tip.1 - uy * l / 2.0);
                    p.move_to(tip.0, tip.1);
                    p.line_to(mid.0 - uy * w, mid.1 + ux * w);
                    p.line_to(base.0, base.1);
                    p.line_to(mid.0 + uy * w, mid.1 - ux * w);
                    p.close();
                }
                _ => {
                    p.move_to(tip.0, tip.1);
                    p.line_to(base.0 - uy * w, base.1 + ux * w);
                    if e.attr("type") == Some("stealth") {
                        p.line_to(tip.0 - ux * l * 0.6, tip.1 - uy * l * 0.6);
                    }
                    p.line_to(base.0 + uy * w, base.1 - ux * w);
                    p.close();
                }
            }
            self.scene.fill(p, Paint::Solid(color));
        };
        if let Some(e) = end("a:headEnd") {
            draw(pts[0], pts[1], e);
        }
        if let Some(e) = end("a:tailEnd") {
            draw(pts[pts.len() - 1], pts[pts.len() - 2], e);
        }
    }

    /// Gör en fyllningsnod till en `Paint`. `ph` är platshållarfärgen från stilmatrisen.
    pub fn paint(&self, n: &Node, ph: Option<Rgba>, f: &Frame, owner: Owner) -> Option<Paint> {
        match n.name.as_str() {
            "a:noFill" => None,
            "a:solidFill" => color::first_color(n).and_then(|c| self.sheets.color(c, ph)).map(Paint::Solid),
            "a:gradFill" => self.gradient(n, ph, f),
            "a:blipFill" => {
                let rels = self.rels(owner);
                let id = n.child("a:blip").and_then(|b| b.attr("r:embed"))?;
                let r = package::rel(rels, id)?;
                let img = self.sheets.deck.image(&r.target)?;
                let alpha = n.path(&["a:blip", "a:alphaModFix"]).and_then(|a| a.attr("amt").and_then(super::xml::parse_i64)).map_or(1.0, |v| v as f32 / 100000.0);
                let t = Affine::scale(f.w, f.h).then(&f.t);
                Some(Paint::Image { image: img, t, tile: n.child("a:tile").is_some(), alpha })
            }
            "a:pattFill" => {
                let fg = n.child("a:fgClr").and_then(color::first_color).and_then(|c| self.sheets.color(c, ph)).unwrap_or(Rgba::BLACK);
                let bg = n.child("a:bgClr").and_then(color::first_color).and_then(|c| self.sheets.color(c, ph)).unwrap_or(Rgba::WHITE);
                let prst = n.attr("prst").unwrap_or("pct50");
                let k = prst.strip_prefix("pct").and_then(|p| p.parse::<f32>().ok()).map_or(0.3, |p| p / 100.0);
                Some(Paint::Solid(mix(bg, fg, k)))
            }
            "a:grpFill" => {
                // Närmaste grupp med en riktig fyllning (en grupp kan själv ha grpFill).
                let g = self.group_fill.iter().rev().find(|g| g.node.is_some_and(|n| n.name != "a:grpFill"))?.clone();
                let node = g.node?;
                self.paint(node, ph, f, g.owner)
            }
            _ => None,
        }
    }

    fn gradient(&self, n: &Node, ph: Option<Rgba>, f: &Frame) -> Option<Paint> {
        let mut stops: Vec<Stop> = n
            .child("a:gsLst")?
            .children
            .iter()
            .filter_map(|gs| {
                let pos = gs.attr("pos").and_then(super::xml::parse_i64).unwrap_or(0) as f32 / 100000.0;
                let c = color::first_color(gs).and_then(|c| self.sheets.color(c, ph))?;
                Some(Stop { pos: pos.clamp(0.0, 1.0), color: c })
            })
            .collect();
        if stops.is_empty() {
            return None;
        }
        stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        if stops.len() == 1 {
            return Some(Paint::Solid(stops[0].color));
        }
        let (w, h) = (f.w.max(0.01), f.h.max(0.01));
        if let Some(path) = n.child("a:path") {
            // Radiell (circle/rect/shape): centrum i fillToRect, stopp 0 i mitten.
            let ftr = path.child("a:fillToRect");
            let v = |k: &str| ftr.and_then(|r| r.attr(k)).and_then(super::xml::parse_i64).unwrap_or(50000) as f32 / 100000.0;
            let (l, t, r, b) = if ftr.is_some() { (v("l"), v("t"), v("r"), v("b")) } else { (0.5, 0.5, 0.5, 0.5) };
            let cx = (l + (1.0 - r)) / 2.0 * w;
            let cy = (t + (1.0 - b)) / 2.0 * h;
            let rx = cx.max(w - cx);
            let ry = cy.max(h - cy);
            let (rad, t) = if path.attr("path") == Some("circle") {
                let rr = (rx * rx + ry * ry).sqrt();
                (rr, Affine::IDENTITY)
            } else {
                // Rektangel/form: en ellips som når hörnen, ungefär.
                let rr = rx.max(1e-3);
                (rr * std::f32::consts::SQRT_2, Affine::translate(-cx, -cy).then(&Affine::scale(1.0, ry / rx.max(1e-3))).then(&Affine::translate(cx, cy)))
            };
            // Stopp 0 ligger ytterst i OOXML:s banövertoning: vänd.
            let stops: Vec<Stop> = stops.into_iter().rev().map(|s| Stop { pos: 1.0 - s.pos, color: s.color }).collect();
            return Some(Paint::Radial { cx, cy, r: rad, stops, t: t.then(&f.t) });
        }
        let ang = n.child("a:lin").and_then(|l| l.attr_i64("ang")).unwrap_or(0) as f32 / 60000.0;
        let a = ang.to_radians();
        let (dx, dy) = (a.cos(), a.sin());
        let half = (w * dx.abs() + h * dy.abs()) / 2.0;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let p0 = f.t.apply(cx - dx * half, cy - dy * half);
        let p1 = f.t.apply(cx + dx * half, cy + dy * half);
        Some(Paint::Linear { x0: p0.0, y0: p0.1, x1: p1.0, y1: p1.1, stops })
    }

    #[allow(clippy::too_many_arguments)]
    fn text_body(&mut self, sp: &'a Node, chain: &[&'a Node], body: &'a Node, style: Option<&'a Node>, f: &Frame, rect: (f32, f32, f32, f32), _owner: Owner) {
        let bodies: Vec<&Node> = chain.iter().filter_map(|s| s.path(&["p:txBody", "a:bodyPr"])).collect();
        let bp = richtext::body_props(&bodies);
        let kind = inherit::tx_kind(sp);
        let lists = inherit::list_styles(&self.sheets, chain, kind);
        let (font_ref, font_color) = inherit::style_font(&self.sheets, style);
        let env = TextEnv { sheets: &self.sheets, lists, font_ref, font_color, font_scale: bp.font_scale, slide_no: self.slide_no };
        let fr = richtext::frame(&env, body, &bp);
        self.place_text(&fr, &bp, f, rect);
    }

    /// Lägger ut en textram i rektangeln `rect` (lokala koordinater) i formen `f`.
    pub fn place_text(&mut self, fr: &text::Frame, bp: &richtext::BodyProps, f: &Frame, rect: (f32, f32, f32, f32)) {
        // Textrektangeln speglas med formen, men texten själv speglas inte.
        let (mut l, t, mut r, b) = rect;
        if f.flip_h {
            (l, r) = (f.w - r, f.w - l);
        }
        let (mut tw, mut th) = ((r - l).max(1.0), (b - t).max(1.0));
        let mut extra = bp.rot;
        let vertical = match bp.vert.as_str() {
            "vert" | "eaVert" | "wordArtVert" | "mongolianVert" => Some(90.0),
            "vert270" => Some(270.0),
            _ => None,
        };
        if let Some(v) = vertical {
            extra += v;
            std::mem::swap(&mut tw, &mut th);
        }
        let laid = text::layout(fr, tw, th, &self.sheets.deck.fonts);
        // Rutans mitt i formens lokala koordinater; texten läggs ut runt sin egen mitt och roteras.
        let (cx, cy) = (l + (r - l) / 2.0, t + (b - t) / 2.0);
        let local = Affine::translate(-tw / 2.0, -th / 2.0).then(&Affine::rotate(extra)).then(&Affine::translate(cx, cy));
        let ta = local.then(&text_affine(f, 0.0));
        self.scene.items.extend(text::transform_items(laid.items, &ta));
    }

    fn picture(&mut self, pic: &'a Node, owner: Owner, parent: &Affine) {
        let chain = inherit::shape_chain(&self.sheets, pic, owner);
        let Some(xfrm) = Self::sppr_child(&chain, &["a:xfrm"]) else { return };
        let Some(f) = frame_of(xfrm, parent) else { return };
        let Some(bf) = pic.child("p:blipFill") else { return };
        // Bildformens egen fyllning ligger bakom bilden (syns genom genomskinliga bilder och metafiler).
        if let Some(paint) = Self::sppr_child(&chain[..chain.len().min(1)], inherit::FILLS).and_then(|n| self.paint(n, None, &f, owner)) {
            for sp in self.geometry(&chain, f.w, f.h).iter().flat_map(|g| g.paths.iter()).filter(|p| p.fill != FillMode::None) {
                self.fill_paint(sp.path.transformed(&f.t), paint.clone());
            }
        }
        self.draw_blip(bf, &chain, &f, owner);
        // Ram runt bilden.
        let style = chain.iter().find_map(|s| s.child("p:style"));
        if let Some(stroke) = self.line(&chain, style, &f, owner) {
            if let Some(g) = self.geometry(&chain, f.w, f.h) {
                for sp in g.paths.iter().filter(|p| p.stroke) {
                    self.scene.stroke(sp.path.transformed(&f.t), stroke.clone());
                }
            }
        }
    }

    /// Ritar `p:blipFill` (bild, SVG eller platshållare för EMF/WMF) i formen `f`.
    pub fn draw_blip(&mut self, bf: &'a Node, chain: &[&'a Node], f: &Frame, owner: Owner) {
        let rels = self.rels(owner);
        let Some(blip) = bf.child("a:blip") else { return };
        let crop = bf.child("a:srcRect").map_or([0.0; 4], |s| {
            let v = |k: &str| s.attr(k).and_then(super::xml::parse_i64).unwrap_or(0) as f32 / 100000.0;
            [v("l"), v("t"), v("r"), v("b")]
        });
        let alpha = blip.child("a:alphaModFix").and_then(|a| a.attr("amt").and_then(super::xml::parse_i64)).map_or(1.0, |v| v as f32 / 100000.0);
        let clip = self.geometry(chain, f.w, f.h).filter(|_| Self::sppr_child(chain, &["a:prstGeom", "a:custGeom"]).is_some_and(|g| g.attr("prst") != Some("rect")));
        let t = Affine::scale(f.w, f.h).then(&f.t);
        // SVG (asvg:svgBlip i blipens extLst) ritas som vektor.
        let svg_id = blip.find("asvg:svgBlip").and_then(|s| s.attr("r:embed"));
        let mut pushed = false;
        if let Some(g) = &clip {
            if let Some(p) = g.paths.first() {
                self.scene.items.push(Item::PushClip { path: p.path.transformed(&f.t) });
                pushed = true;
            }
        }
        let mut drawn = false;
        if let Some(r) = svg_id.and_then(|id| package::rel(rels, id)) {
            if let Some(tree) = self.sheets.deck.svg(&r.target) {
                self.scene.items.push(Item::Svg { tree, t, crop, alpha });
                drawn = true;
            }
        }
        if !drawn {
            if let Some(r) = blip.attr("r:embed").and_then(|id| package::rel(rels, id)) {
                let low = r.target.to_lowercase();
                if low.ends_with(".emf") || low.ends_with(".wmf") {
                    super::media::metafile(self, &r.target, f, crop);
                } else if let Some(img) = self.sheets.deck.image(&r.target) {
                    let img = apply_blip_effects(blip, img);
                    self.scene.items.push(Item::Image { image: img, t, crop, alpha });
                } else if low.ends_with(".svg") {
                    if let Some(tree) = self.sheets.deck.svg(&r.target) {
                        self.scene.items.push(Item::Svg { tree, t, crop, alpha });
                    }
                }
            }
        }
        if pushed {
            self.scene.items.push(Item::PopClip);
        }
    }

    fn graphic_frame(&mut self, gf: &'a Node, owner: Owner, parent: &Affine) {
        let chain = inherit::shape_chain(&self.sheets, gf, owner);
        let xfrm = chain.iter().find_map(|s| s.child("p:xfrm"));
        let Some(f) = xfrm.and_then(|x| frame_of(x, parent)) else { return };
        let Some(gd) = gf.path(&["a:graphic", "a:graphicData"]) else { return };
        let uri = gd.attr("uri").unwrap_or("");
        if uri.ends_with("/table") {
            if let Some(tbl) = gd.child("a:tbl") {
                super::tables::draw_table(self, tbl, &f, owner);
            }
        } else if uri.ends_with("/diagram") {
            super::smartart::draw_diagram(self, gd, &f, owner);
        } else if uri.ends_with("/chart") || uri.contains("chartex") {
            super::charts::draw_chart(self, gd, &f, owner);
        } else {
            // OLE-objekt och annat: förhandsbilden (p:pic) om den finns.
            if let Some(pic) = gd.find("p:pic") {
                if let Some(bf) = pic.child("p:blipFill") {
                    self.draw_blip(bf, &[pic], &f, owner);
                }
            }
        }
    }

    /// En grå ram med en etikett (för innehåll som ännu inte ritas: diagram, EMF …).
    pub fn placeholder_box(&mut self, f: &Frame, label: &str) {
        let path = Path::rect(0.0, 0.0, f.w, f.h).transformed(&f.t);
        self.scene.fill(path.clone(), Paint::Solid(Rgba { r: 0.94, g: 0.94, b: 0.94, a: 1.0 }));
        self.scene.stroke(path, Stroke { paint: Paint::Solid(Rgba { r: 0.6, g: 0.6, b: 0.6, a: 1.0 }), width: 0.75, dash: vec![3.0, 2.0], cap: Cap::Butt, join: Join::Miter });
        let mut style = text::RunStyle { size: (f.h * 0.12).clamp(8.0, 18.0), color: Rgba { r: 0.4, g: 0.4, b: 0.4, a: 1.0 }, ..Default::default() };
        style.latin = "Calibri".into();
        let fr = text::Frame {
            paras: vec![text::Para { runs: vec![text::Run { text: label.to_owned(), style, line_break: false }], align: text::Align::Center, ..Default::default() }],
            insets: (4.0, 4.0, 4.0, 4.0),
            anchor: text::Anchor::Middle,
            wrap: true,
            ..Default::default()
        };
        let laid = text::layout(&fr, f.w, f.h, &self.sheets.deck.fonts);
        self.scene.items.extend(text::transform_items(laid.items, &f.t));
    }
}

/// Bildeffekter på `a:blip` som är billiga: gråskala och dubbelton görs inte än; `lum` (ljusstyrka/kontrast) inte.
fn apply_blip_effects(blip: &Node, img: Arc<super::scene::ImageData>) -> Arc<super::scene::ImageData> {
    if blip.child("a:grayscl").is_some() {
        let mut rgba = img.rgba.clone();
        for p in rgba.chunks_exact_mut(4) {
            let y = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as u8;
            p[0] = y;
            p[1] = y;
            p[2] = y;
        }
        return Arc::new(super::scene::ImageData::new(img.width, img.height, rgba, None));
    }
    img
}

fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    Rgba { r: a.r + (b.r - a.r) * k, g: a.g + (b.g - a.g) * k, b: a.b + (b.b - a.b) * k, a: a.a + (b.a - a.a) * k }
}

fn scale_lum(c: Rgba, k: f32) -> Rgba {
    Rgba { r: c.r * k, g: c.g * k, b: c.b * k, a: c.a }
}

fn lighten(c: Rgba, k: f32) -> Rgba {
    mix(c, Rgba { a: c.a, ..Rgba::WHITE }, k)
}

/// En ellips som bana.
pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
    let mut p = Path::default();
    p.move_to(cx + rx, cy);
    geom::arc_to(&mut p, ((cx + rx) as f64, cy as f64), rx as f64, ry as f64, 0.0, 21_600_000.0);
    p.close();
    p
}
