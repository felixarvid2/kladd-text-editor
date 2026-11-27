//! Morph (`p159:morph`): formerna på den gamla och den nya bilden paras ihop och interpoleras.
//!
//! * `infos` läser toppnivåformerna ur bildens formträd (namn, slag, text, geometri/bildrelation, `xfrm`).
//! * `pair` paras ihop som PowerPoint: namn som börjar med `!!` bara mot samma namn, annars samma
//!   namn och samma slags objekt, därefter samma bild eller geometri och samma text. (`byWord` och
//!   `byChar` paras som `byObject`.)
//! * `Plan` ger för varje förlopp 0..1 en lista av ritsteg (`Op`): vilken sida (gamla/nya bilden),
//!   vilken form, hur den flyttas (`Affine` i punkter på bilden) och genomskinligheten. Paren flyttas,
//!   skalas och vrids från sitt gamla till sitt nya läge med mjuk början och slut (`transition::ease`)
//!   och de två renderingarna läggs över varandra; former utan par tonas ut respektive in.
//! * `Side::build` renderar en bild med ett lager per toppnivåform (och per animerat stycke, så att
//!   bildspelets animeringstillstånd gäller även här) med `anim_layers`.
//!
//! Ritstegen används både av bildspelet (`presenter.rs`, GPU-nät) och av `compose` (tiny-skia, för prov).
//! Vid förloppet 0 ger stegen exakt den gamla bilden, vid 1 exakt den nya.

use std::collections::HashMap;

#[cfg(test)]
use resvg::tiny_skia as sk;

use super::anim::Target;
use super::anim_layers::{self, LayerKind, Rendered};
use super::inherit::{Owner, Sheets};
use super::render::{emu, slide_scene};
use super::scene::{Affine, Item};
use super::transition::ease;
use super::xml::Node;
use super::Deck;

/// Slag av objekt; bara samma slag paras ihop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Shape,
    Text,
    Picture,
    Group,
    Table,
    Diagram,
    /// Diagram (`c:chart`) och annat i en grafikram.
    Other,
}

/// Läget för en form i punkter på bilden: mitt, storlek (före vridning) och vridning (grader medurs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geo {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub rot: f32,
}

/// En toppnivåform ur bildens formträd.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub id: u32,
    pub name: String,
    pub kind: ShapeKind,
    pub text: String,
    /// Bildens relationsmål (bilder) eller förinställd geometri (former); tom annars.
    pub key: String,
    /// Formens egen `a:xfrm` (saknas för platshållare som ärver läget).
    pub xfrm: Option<Geo>,
}

fn text_of(n: &Node, out: &mut String) {
    if n.name == "a:t" {
        out.push_str(&n.text);
    }
    for c in &n.children {
        text_of(c, out);
    }
}

fn geo_of(x: &Node) -> Option<Geo> {
    let (o, e) = (x.child("a:off")?, x.child("a:ext")?);
    let (w, h) = (emu(e.attr_i64("cx")?), emu(e.attr_i64("cy")?));
    let (x0, y0) = (emu(o.attr_i64("x")?), emu(o.attr_i64("y")?));
    let rot = x.attr_i64("rot").unwrap_or(0) as f32 / 60000.0;
    Some(Geo { cx: x0 + w / 2.0, cy: y0 + h / 2.0, w, h, rot })
}

/// Toppnivåformerna på bild `idx` i ritordning: samma urval som får `BeginShape` i `render::Builder::tree`
/// (med id, inte dolda).
pub fn infos(deck: &Deck, idx: usize) -> Vec<Info> {
    let sheets = Sheets::for_slide(deck, idx);
    let Some(tree) = sheets.tree(Owner::Slide) else { return Vec::new() };
    let rels = sheets.rels(Owner::Slide);
    let mut out = Vec::new();
    for c in &tree.children {
        if !matches!(c.name.as_str(), "p:sp" | "p:cxnSp" | "p:pic" | "p:grpSp" | "p:graphicFrame") {
            continue;
        }
        let Some(nv) = c.children.first().and_then(|nv| nv.child("p:cNvPr")) else { continue };
        if nv.attr_bool("hidden") == Some(true) {
            continue;
        }
        let Some(id) = nv.attr_i64("id").and_then(|i| u32::try_from(i).ok()) else { continue };
        let name = nv.attr("name").unwrap_or("").to_owned();
        let sp_pr = c.child("p:spPr");
        let (kind, xf, key) = match c.name.as_str() {
            "p:sp" => {
                let text_box = c.children.first().and_then(|nv| nv.child("p:cNvSpPr")).is_some_and(|n| n.attr_bool("txBox") == Some(true));
                let prst = sp_pr.and_then(|s| s.child("a:prstGeom")).and_then(|g| g.attr("prst")).unwrap_or("cust").to_owned();
                (if text_box { ShapeKind::Text } else { ShapeKind::Shape }, sp_pr.and_then(|s| s.child("a:xfrm")), prst)
            }
            "p:cxnSp" => (ShapeKind::Shape, sp_pr.and_then(|s| s.child("a:xfrm")), "line".to_owned()),
            "p:pic" => {
                let rid = c.path(&["p:blipFill", "a:blip"]).and_then(|b| b.attr("r:embed")).unwrap_or("");
                let target = rels.iter().find(|r| r.id == rid).map_or_else(String::new, |r| r.target.clone());
                (ShapeKind::Picture, sp_pr.and_then(|s| s.child("a:xfrm")), target)
            }
            "p:grpSp" => (ShapeKind::Group, c.child("p:grpSpPr").and_then(|s| s.child("a:xfrm")), String::new()),
            _ => {
                let uri = c.path(&["a:graphic", "a:graphicData"]).and_then(|d| d.attr("uri")).unwrap_or("");
                let kind = if uri.ends_with("/table") {
                    ShapeKind::Table
                } else if uri.ends_with("/diagram") {
                    ShapeKind::Diagram
                } else {
                    ShapeKind::Other
                };
                (kind, c.child("p:xfrm"), String::new())
            }
        };
        let mut text = String::new();
        if let Some(b) = c.child("p:txBody") {
            text_of(b, &mut text);
        }
        out.push(Info { id, name, kind, text: text.trim().to_owned(), key, xfrm: xf.and_then(geo_of) });
    }
    out
}

/// Namn som börjar med `!!` paras bara mot samma namn.
fn forced(name: &str) -> bool {
    name.starts_with("!!")
}

/// Paren: för varje form i `b` (i ordning) indexet av den form i `a` den paras med.
///
/// 1. `!!`-namn mot samma namn (och slag). 2. Samma namn och slag (flera: samma bild/geometri och text
/// först, sedan ordningen). 3. Resten av samma slag: samma bild (bilder), eller samma geometri och
/// samma text (andra former), eller bara samma text (textrutor).
pub fn pair(a: &[Info], b: &[Info]) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = vec![None; b.len()];
    let mut used = vec![false; a.len()];
    let take = |out: &mut Vec<Option<usize>>, used: &mut Vec<bool>, bi: usize, cands: Vec<usize>, score: &dyn Fn(&Info, &Info) -> i32| {
        // Högst poäng, vid lika den första i ordningen.
        let best = cands.into_iter().max_by_key(|&ai| (score(&a[ai], &b[bi]), std::cmp::Reverse(ai)));
        if let Some(ai) = best {
            out[bi] = Some(ai);
            used[ai] = true;
        }
    };
    let similar = |x: &Info, y: &Info| i32::from(!x.key.is_empty() && x.key == y.key) * 2 + i32::from(x.text == y.text);
    for pass in 0..3 {
        for bi in 0..b.len() {
            if out[bi].is_some() {
                continue;
            }
            let bf = forced(&b[bi].name);
            let free = |ai: &usize| !used[*ai] && a[*ai].kind == b[bi].kind;
            let cands: Vec<usize> = match pass {
                0 | 1 => {
                    // Passet 0: `!!`-namn, pass 1: vanliga namn.
                    if (pass == 0) != bf {
                        continue;
                    }
                    (0..a.len()).filter(free).filter(|&ai| a[ai].name == b[bi].name).collect()
                }
                _ => {
                    if bf {
                        continue;
                    }
                    let (y, k) = (&b[bi], b[bi].kind);
                    (0..a.len())
                        .filter(free)
                        .filter(|&ai| {
                            let x = &a[ai];
                            !forced(&x.name)
                                && match k {
                                    ShapeKind::Picture => !y.key.is_empty() && x.key == y.key,
                                    ShapeKind::Text => !y.text.is_empty() && x.text == y.text,
                                    _ => !y.text.is_empty() && x.text == y.text && x.key == y.key,
                                }
                        })
                        .collect()
                }
            };
            take(&mut out, &mut used, bi, cands, &similar);
        }
    }
    out
}

/// En sida (den gamla eller den nya bilden) av morphen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Old,
    New,
}

/// Ett ritsteg: lagren för `shape` (`None` = det fasta: bakgrund, mall och layout) på sidan `which`,
/// flyttade med `map` (punkter på bilden → punkter på bilden) och med genomskinligheten `alpha`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Op {
    pub which: Which,
    pub shape: Option<u32>,
    pub map: Affine,
    pub alpha: f32,
}

/// En form i planen: dess id och läge, och läget den ska flyttas till om den har ett par.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Post {
    id: u32,
    from: Geo,
    to: Option<Geo>,
}

/// Hur formerna rör sig mellan två bilder. Den gamla bildens former ligger i sin egen ritordning och den
/// nya bildens i sin, den nya ovanpå: vid förloppet 0 ger det exakt den gamla bilden, vid 1 den nya.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Plan {
    old: Vec<Post>,
    new: Vec<Post>,
}

/// Genomskinligheterna för ett par vid den mjukade förloppet `e`: den ena sidan är alltid täckande så att
/// en oförändrad form inte blir genomskinlig på vägen; ändpunkterna är exakta.
fn pair_alpha(e: f32) -> (f32, f32) {
    ((2.0 * (1.0 - e)).min(1.0), (2.0 * e).min(1.0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Vinkeln `b` räknad som närmaste väg från `a` (grader).
fn near(a: f32, b: f32) -> f32 {
    a + (b - a + 180.0).rem_euclid(360.0) - 180.0
}

/// Avbildningen som flyttar en rendering av en form med läget `from` till läget vid förloppet `e` mellan
/// `from` och `to`.
fn morph_map(from: &Geo, to: &Geo, e: f32) -> Affine {
    let (cx, cy) = (lerp(from.cx, to.cx, e), lerp(from.cy, to.cy, e));
    let (w, h) = (lerp(from.w, to.w, e), lerp(from.h, to.h, e));
    let rot = lerp(from.rot, near(from.rot, to.rot), e);
    let ratio = |now: f32, was: f32| if was.abs() < 1e-3 { 1.0 } else { (now / was).clamp(0.001, 1000.0) };
    let m = Affine::translate(-from.cx, -from.cy)
        .then(&Affine::rotate(-from.rot))
        .then(&Affine::scale(ratio(w, from.w), ratio(h, from.h)))
        .then(&Affine::rotate(rot))
        .then(&Affine::translate(cx, cy));
    // Ingen rörelse: exakt identitet, så att ändpunkterna inte omsamplas.
    let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
    if close(m.a, 1.0) && close(m.d, 1.0) && close(m.b, 0.0) && close(m.c, 0.0) && close(m.e, 0.0) && close(m.f, 0.0) { Affine::IDENTITY } else { m }
}

impl Plan {
    /// Planen för morph från bilden med formerna `old` till den med `new`. `*_box` ger formens
    /// omslutande rektangel (x0, y0, x1, y1) i punkter ur renderingen (används där `xfrm` saknas);
    /// former utan rektangel ritar inget och är med varken som par eller ensamma.
    pub fn new(old: &[Info], old_box: &HashMap<u32, (f32, f32, f32, f32)>, new: &[Info], new_box: &HashMap<u32, (f32, f32, f32, f32)>) -> Plan {
        let bbox_geo = |r: &(f32, f32, f32, f32)| Geo { cx: (r.0 + r.2) / 2.0, cy: (r.1 + r.3) / 2.0, w: r.2 - r.0, h: r.3 - r.1, rot: 0.0 };
        let paired = pair(old, new);
        let mut partner: Vec<Option<usize>> = vec![None; old.len()];
        for (bi, ai) in paired.iter().enumerate() {
            if let Some(ai) = ai {
                partner[*ai] = Some(bi);
            }
        }
        // Båda har eget läge: använd det (ger vridningen och formens storlek utan text och skuggor).
        let geos = |ai: usize, bi: usize| -> Option<(Geo, Geo)> {
            let (x, y) = (&old[ai], &new[bi]);
            let (ra, rb) = (old_box.get(&x.id)?, new_box.get(&y.id)?);
            Some(match (x.xfrm, y.xfrm) {
                (Some(ga), Some(gb)) => (ga, gb),
                _ => (bbox_geo(ra), bbox_geo(rb)),
            })
        };
        // Former med dubblerade id får bara en post (lagren för id:t ritas ändå alla).
        let mut seen = std::collections::HashSet::new();
        let mut plan = Plan::default();
        for (ai, x) in old.iter().enumerate() {
            let Some(ra) = old_box.get(&x.id) else { continue };
            if !seen.insert(x.id) {
                continue;
            }
            let pg = partner[ai].and_then(|bi| geos(ai, bi));
            plan.old.push(Post { id: x.id, from: pg.map_or_else(|| bbox_geo(ra), |g| g.0), to: pg.map(|g| g.1) });
        }
        seen.clear();
        for (bi, y) in new.iter().enumerate() {
            let Some(rb) = new_box.get(&y.id) else { continue };
            if !seen.insert(y.id) {
                continue;
            }
            let pg = paired[bi].and_then(|ai| geos(ai, bi));
            plan.new.push(Post { id: y.id, from: pg.map_or_else(|| bbox_geo(rb), |g| g.1), to: pg.map(|g| g.0) });
        }
        plan
    }

    /// Ritstegen vid förloppet `p` (0..1), underst först.
    pub fn ops(&self, p: f32) -> Vec<Op> {
        let p = p.clamp(0.0, 1.0);
        let e = ease(p);
        let id = Affine::IDENTITY;
        let mut ops = vec![Op { which: Which::Old, shape: None, map: id, alpha: 1.0 }, Op { which: Which::New, shape: None, map: id, alpha: e }];
        let (pa, pb) = pair_alpha(e);
        for it in &self.old {
            match &it.to {
                Some(to) => ops.push(Op { which: Which::Old, shape: Some(it.id), map: morph_map(&it.from, to, e), alpha: pa }),
                None => ops.push(Op { which: Which::Old, shape: Some(it.id), map: id, alpha: 1.0 - e }),
            }
        }
        for it in &self.new {
            match &it.to {
                // Från det nya läget mot det gamla, så att den hamnar på samma ställe som den gamla.
                Some(to) => ops.push(Op { which: Which::New, shape: Some(it.id), map: morph_map(&it.from, to, 1.0 - e), alpha: pb }),
                None => ops.push(Op { which: Which::New, shape: Some(it.id), map: id, alpha: e }),
            }
        }
        ops
    }
}

/// En bild renderad för morph: lager per toppnivåform och formerna.
pub struct Side {
    pub infos: Vec<Info>,
    pub layers: Vec<Rendered>,
    /// Punkter på bilden.
    pub size: (f32, f32),
}

impl Side {
    /// Renderar bild `idx` i `width`×`height` med ett lager per toppnivåform och per mål i `targets`
    /// (bildspelets animeringsmål, så att deras tillstånd kan läggas på lagren).
    pub fn build(deck: &Deck, idx: usize, targets: &[Target], width: u32, height: u32) -> Side {
        let scene = slide_scene(deck, idx);
        let mut all: Vec<Target> = targets.to_vec();
        all.extend(scene.shape_ranges().iter().map(|(id, _)| Target { shape: *id, para: None }));
        all.sort();
        all.dedup();
        let mut split = anim_layers::split(&scene, &all);
        // Lagrens yta räknar textens bredd med en uppskattning som ibland blir för snäv; i en morph syns
        // lagret hela tiden, så texten får luft åt sidorna (bara ytan, inte innehållsrutan).
        for l in &mut split {
            let size = l.items.iter().filter_map(|it| if let Item::Glyphs(g) = it { Some(g.size) } else { None }).fold(0.0f32, f32::max);
            if size > 0.0 && matches!(l.kind, LayerKind::Target(_)) {
                let a = &mut l.area;
                *a = ((a.0 - size).max(0.0), (a.1 - size * 0.25).max(0.0), (a.2 + size * 2.0).min(scene.width), (a.3 + size * 0.5).min(scene.height));
            }
        }
        let layers = anim_layers::render(&scene, &split, width, height);
        Side { infos: infos(deck, idx), layers, size: (scene.width, scene.height) }
    }

    /// Formernas omslutande rektanglar (punkter) ur lagrens innehåll.
    pub fn boxes(&self) -> HashMap<u32, (f32, f32, f32, f32)> {
        let mut m: HashMap<u32, (f32, f32, f32, f32)> = HashMap::new();
        for l in &self.layers {
            let LayerKind::Target(t) = l.kind else { continue };
            let (sw, sh) = self.size;
            let g = l.geom;
            let r = ((g.cx - g.w / 2.0) * sw, (g.cy - g.h / 2.0) * sh, (g.cx + g.w / 2.0) * sw, (g.cy + g.h / 2.0) * sh);
            let e = m.entry(t.shape).or_insert(r);
            *e = (e.0.min(r.0), e.1.min(r.1), e.2.max(r.2), e.3.max(r.3));
        }
        m
    }
}

/// Planen mellan två renderade sidor.
pub fn plan(old: &Side, new: &Side) -> Plan {
    Plan::new(&old.infos, &old.boxes(), &new.infos, &new.boxes())
}

/// Lagren som hör till ritsteget: det fasta (`shape == None`) eller formens lager (även stycken).
#[cfg(test)]
pub fn layers_of<'a>(layers: &'a [Rendered], shape: Option<u32>) -> impl Iterator<Item = &'a Rendered> + 'a {
    layers.iter().filter(move |l| belongs(l.kind, shape))
}

/// Hör lagret till ritsteget `shape` (`None` = det fasta)?
pub fn belongs(kind: LayerKind, shape: Option<u32>) -> bool {
    match (kind, shape) {
        (LayerKind::Static, None) => true,
        (LayerKind::Target(t), Some(s)) => t.shape == s,
        _ => false,
    }
}

/// Lägger ritstegen över varandra med tiny-skia (`w`×`h` bildpunkter): samma bild som bildspelet ritar
/// (utan animeringstillstånd). Används av proven.
#[cfg(test)]
pub fn compose(plan: &Plan, old: &Side, new: &Side, p: f32, w: u32, h: u32) -> Option<sk::Pixmap> {
    let mut acc = sk::Pixmap::new(w, h)?;
    let (sw, sh) = old.size;
    let (kx, ky) = (w as f32 / sw.max(1.0), h as f32 / sh.max(1.0));
    for op in plan.ops(p) {
        if op.alpha <= 0.0 {
            continue;
        }
        let side = if op.which == Which::Old { old } else { new };
        // Bildpunkter → punkter → flyttning → bildpunkter.
        let m = Affine::scale(1.0 / kx, 1.0 / ky).then(&op.map).then(&Affine::scale(kx, ky));
        for l in layers_of(&side.layers, op.shape) {
            let Some(size) = sk::IntSize::from_wh(l.px.2, l.px.3) else { continue };
            let Some(pm) = sk::Pixmap::from_vec(l.rgba.clone(), size) else { continue };
            let t = sk::Transform::from_row(m.a, m.b, m.c, m.d, m.e, m.f).pre_translate(l.px.0 as f32, l.px.1 as f32);
            let paint = sk::PixmapPaint { opacity: op.alpha.clamp(0.0, 1.0), quality: sk::FilterQuality::Bilinear, ..Default::default() };
            acc.draw_pixmap(0, 0, pm.as_ref(), &paint, t, None);
        }
    }
    Some(acc)
}

#[cfg(test)]
#[path = "morph_tests.rs"]
mod tests;
