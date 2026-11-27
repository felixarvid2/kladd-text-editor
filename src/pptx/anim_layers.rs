//! Bilden delad i lager för animeringar: varje animerat mål (form eller stycke, `anim::Target`) blir
//! ett eget lager, och det som inte animeras blir fasta lager mellan dem i ritordning (så att en
//! animerad form under en fast form fortfarande hamnar under den). Lagren renderas var för sig med
//! tiny-skia, beskurna till sitt innehåll (med marginal för skugga och glöd) och i samma bildpunktsrutnät
//! som hela bilden, så att lagren ovanpå varandra blir samma bild som en vanlig rendering.
//!
//! Formerna hittas med `Scene::shape_ranges` (`Item::BeginShape`), styckena med `Item::Para`.
//!
//! Bokstav för bokstav och ord för ord (`p:iterate`, `p:charRg`): för en text som delas i delar
//! (`anim::UnitScope`) finns förutom helhetslagret (`LayerKind::Target`) ett lager per del
//! (`LayerKind::Piece`, glyferna med `PlacedGlyph::ch`/`word` i delen) och ett lager med resten
//! (`LayerKind::Rest`: punkt, understrykning, markering). `plan` väljer vilka som ritas: har alla delar
//! samma tillstånd ritas helhetslagret (exakt samma bild som utan delning), annars delarna.

use std::collections::{BTreeMap, HashMap, HashSet};

use resvg::tiny_skia as sk;

use super::anim::{self, Geom, State, Target, UnitKind, UnitScope};
use super::scene::{Affine, Effect, GlyphRun, Item, NO_CHAR, PlacedGlyph, Scene};

/// Vad ett lager är.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    /// Det som inte animeras (det första fasta lagret har bakgrunden).
    Static,
    Target(Target),
    /// En del (bokstav eller ord, `unit`) av texten `scope`.
    Piece { scope: Target, unit: u32 },
    /// Det som inte hör till någon del av texten `scope` (punkt, understrykning, markering).
    Rest(Target),
}

/// Ett lagers element och mått (punkter på bilden).
#[derive(Clone, Debug)]
pub struct LayerItems {
    pub kind: LayerKind,
    pub items: Vec<Item>,
    /// Innehållets omslutande rektangel (x0, y0, x1, y1) utan marginal (för formlerna).
    pub content: (f32, f32, f32, f32),
    /// Det som ska renderas: innehållet med marginal för effekter, inom bilden.
    pub area: (f32, f32, f32, f32),
}

impl LayerItems {
    /// Lagrets läge och storlek i andelar av bilden.
    pub fn geom(&self, sw: f32, sh: f32) -> Geom {
        let (x0, y0, x1, y1) = self.content;
        let (sw, sh) = (sw.max(1.0), sh.max(1.0));
        Geom { cx: (x0 + x1) / 2.0 / sw, cy: (y0 + y1) / 2.0 / sh, w: (x1 - x0) / sw, h: (y1 - y0) / sh }
    }
}

/// Delar bilden i lager. Utan mål blir det ett enda fast lager med allt.
#[allow(dead_code)]
pub fn split(scene: &Scene, targets: &[Target]) -> Vec<LayerItems> {
    split_units(scene, targets, &[])
}

/// Som `split`, och dessutom delar för texterna i `scopes` (lager efter formens vanliga lager).
pub fn split_units(scene: &Scene, targets: &[Target], scopes: &[UnitScope]) -> Vec<LayerItems> {
    let shapes: HashSet<u32> = targets.iter().map(|t| t.shape).collect();
    let whole: HashSet<u32> = targets.iter().filter(|t| t.para.is_none()).map(|t| t.shape).collect();
    let page = (0.0, 0.0, scene.width, scene.height);
    let mut out: Vec<LayerItems> = Vec::new();
    let mut stat: Vec<Item> = Vec::new();
    let mut first_static = true;
    let flush = |stat: &mut Vec<Item>, out: &mut Vec<LayerItems>, first: &mut bool| {
        if stat.is_empty() {
            return;
        }
        let items = std::mem::take(stat);
        if *first {
            out.push(LayerItems { kind: LayerKind::Static, items, content: page, area: page });
            *first = false;
        } else if let Some((content, area)) = extent(&items, scene) {
            out.push(LayerItems { kind: LayerKind::Static, items, content, area });
        }
    };
    let mut i = 0;
    let items = &scene.items;
    while i < items.len() {
        let Item::BeginShape { id } = items[i] else {
            if !matches!(items[i], Item::EndShape | Item::Para(_)) {
                stat.push(items[i].clone());
            }
            i += 1;
            continue;
        };
        let end = matching_end_shape(items, i);
        let body = &items[i + 1..end];
        i = end + 1;
        if !shapes.contains(&id) {
            stat.extend(strip_markers(body));
            continue;
        }
        let mut paras: Vec<u32> = targets.iter().filter(|t| t.shape == id).filter_map(|t| t.para).collect();
        paras.sort_unstable();
        paras.dedup();
        // Styckena som finns i formen (mål för stycken som inte finns hoppas över).
        let present: HashSet<u32> = body.iter().filter_map(|it| if let Item::Para(n) = it { Some(*n) } else { None }).collect();
        paras.retain(|p| present.contains(p));
        let rest = filter(body, |p| p.is_none_or(|p| !paras.contains(&p)));
        if whole.contains(&id) {
            flush(&mut stat, &mut out, &mut first_static);
            if let Some((content, area)) = extent(&rest, scene) {
                out.push(LayerItems { kind: LayerKind::Target(Target { shape: id, para: None }), items: rest, content, area });
            }
        } else {
            stat.extend(rest);
        }
        if !paras.is_empty() {
            flush(&mut stat, &mut out, &mut first_static);
            for p in paras {
                let only = filter(body, |q| q == Some(p));
                if let Some((content, area)) = extent(&only, scene) {
                    out.push(LayerItems { kind: LayerKind::Target(Target { shape: id, para: Some(p) }), items: only, content, area });
                }
            }
        }
        for sc in scopes.iter().filter(|s| s.shape == id) {
            // Delarna hör till ett lager som finns (formen eller stycket); annars är de onödiga.
            let base = Target { shape: sc.shape, para: sc.para };
            let has_base = out.iter().any(|l| l.kind == LayerKind::Target(base)) && targets.contains(&base);
            if !has_base {
                continue;
            }
            let pieces = piece_layers(body, sc, scene);
            if !pieces.is_empty() {
                flush(&mut stat, &mut out, &mut first_static);
                out.extend(pieces);
            }
        }
    }
    flush(&mut stat, &mut out, &mut first_static);
    if out.is_empty() {
        out.push(LayerItems { kind: LayerKind::Static, items: Vec::new(), content: page, area: page });
    }
    out
}

/// Lagren för delarna av texten `sc` i formens element `body`: ett per bokstav eller ord och ett med
/// resten. Tomt om texten inte har några tecken eller har för många delar.
fn piece_layers(body: &[Item], sc: &UnitScope, scene: &Scene) -> Vec<LayerItems> {
    let scope = Target { shape: sc.shape, para: sc.para };
    let in_scope = |cur: Option<u32>| sc.para.is_none_or(|q| cur == Some(q));
    // Glyferna per stycke (i ordning): (element, glyf, delens nummer inom stycket).
    let mut per_para: Vec<Vec<(usize, usize, u32)>> = Vec::new();
    let mut cur: Option<u32> = None;
    let mut open = false;
    for (i, it) in body.iter().enumerate() {
        match it {
            Item::Para(n) => {
                cur = Some(*n);
                open = false;
            }
            Item::Glyphs(g) if in_scope(cur) => {
                for (k, gl) in g.glyphs.iter().enumerate() {
                    let key = if sc.kind == UnitKind::Letters { gl.ch } else { gl.word };
                    if gl.ch == NO_CHAR || key == NO_CHAR {
                        continue;
                    }
                    if !open {
                        per_para.push(Vec::new());
                        open = true;
                    }
                    per_para.last_mut().unwrap().push((i, k, key));
                }
            }
            _ => {}
        }
    }
    // Delens nummer; för hela formens text räknas styckena efter varandra.
    let mut assign: HashMap<(usize, usize), u32> = HashMap::new();
    let mut offset = 0u32;
    for list in &per_para {
        let count = list.iter().map(|e| e.2).max().map_or(0, |m| m + 1);
        for &(i, k, key) in list {
            assign.insert((i, k), offset + key);
        }
        if sc.para.is_none() {
            offset += count;
        }
    }
    let units: std::collections::BTreeSet<u32> = assign.values().copied().collect();
    if units.is_empty() || units.len() > MAX_PIECES {
        return Vec::new();
    }
    let mut rest: Vec<Item> = Vec::new();
    let mut by_unit: BTreeMap<u32, Vec<Item>> = units.iter().map(|&u| (u, Vec::new())).collect();
    cur = None;
    for (i, it) in body.iter().enumerate() {
        match it {
            Item::Para(n) => cur = Some(*n),
            Item::BeginShape { .. } | Item::EndShape => {}
            Item::BeginEffect(_) | Item::EndEffect | Item::PushClip { .. } | Item::PopClip => {
                rest.push(it.clone());
                for v in by_unit.values_mut() {
                    v.push(it.clone());
                }
            }
            Item::Glyphs(g) if in_scope(cur) && g.glyphs.iter().enumerate().any(|(k, _)| assign.contains_key(&(i, k))) => {
                let mut per: BTreeMap<u32, Vec<PlacedGlyph>> = BTreeMap::new();
                let mut left: Vec<PlacedGlyph> = Vec::new();
                for (k, gl) in g.glyphs.iter().enumerate() {
                    match assign.get(&(i, k)) {
                        Some(u) => per.entry(*u).or_default().push(*gl),
                        None => left.push(*gl),
                    }
                }
                let part = |glyphs: Vec<PlacedGlyph>| Item::Glyphs(GlyphRun { glyphs, text: String::new(), ..g.clone() });
                for (u, glyphs) in per {
                    if let Some(v) = by_unit.get_mut(&u) {
                        v.push(part(glyphs));
                    }
                }
                if !left.is_empty() {
                    rest.push(part(left));
                }
            }
            other if in_scope(cur) => rest.push(other.clone()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (unit, items) in by_unit {
        if let Some((content, area)) = extent(&items, scene) {
            out.push(LayerItems { kind: LayerKind::Piece { scope, unit }, items, content, area });
        }
    }
    if let Some((content, area)) = extent(&rest, scene) {
        out.push(LayerItems { kind: LayerKind::Rest(scope), items: rest, content, area });
    }
    out
}

/// Flest delar per text som delas upp i lager (fler ritas som helhet).
const MAX_PIECES: usize = 600;

/// Vilket tillstånd varje lager ritas med (`None` = ritas inte). `state_of(i)` är lager `i`:s eget
/// tillstånd. En text som delats i delar ritas som helhet (helhetslagret med delarnas gemensamma
/// tillstånd) när alla delar har samma tillstånd, annars ritas delarna och resten var för sig.
pub fn plan(kinds: &[LayerKind], state_of: &dyn Fn(usize) -> State) -> Vec<Option<State>> {
    let mut scopes: HashMap<Target, Vec<usize>> = HashMap::new();
    for (i, k) in kinds.iter().enumerate() {
        if let LayerKind::Piece { scope, .. } = k {
            scopes.entry(*scope).or_default().push(i);
        }
    }
    let uniform: HashMap<Target, Option<State>> = scopes.iter().map(|(sc, ix)| (*sc, anim::uniform(&ix.iter().map(|&i| state_of(i)).collect::<Vec<_>>()))).collect();
    kinds
        .iter()
        .enumerate()
        .map(|(i, k)| match k {
            LayerKind::Static => Some(state_of(i)),
            LayerKind::Target(t) => match uniform.get(t) {
                Some(u) => *u,
                None => Some(state_of(i)),
            },
            LayerKind::Piece { scope, .. } | LayerKind::Rest(scope) => match uniform.get(scope) {
                Some(Some(_)) => None,
                _ => Some(state_of(i)),
            },
        })
        .collect()
}

/// Index för `EndShape` som hör till `BeginShape` på plats `i` (eller listans slut).
fn matching_end_shape(items: &[Item], i: usize) -> usize {
    let mut depth = 0usize;
    for (k, it) in items.iter().enumerate().skip(i) {
        match it {
            Item::BeginShape { .. } => depth += 1,
            Item::EndShape => {
                depth -= 1;
                if depth == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    items.len()
}

fn strip_markers(items: &[Item]) -> impl Iterator<Item = Item> + '_ {
    items.iter().filter(|it| !matches!(it, Item::BeginShape { .. } | Item::EndShape | Item::Para(_))).cloned()
}

/// Elementen vars stycke (`None` = före första `Para`, t.ex. formens geometri) godtas av `keep`.
/// Effekt- och klippmarkörer behålls alltid så att listan förblir balanserad.
pub fn filter(items: &[Item], keep: impl Fn(Option<u32>) -> bool) -> Vec<Item> {
    let mut cur = None;
    let mut out = Vec::new();
    for it in items {
        match it {
            Item::Para(n) => cur = Some(*n),
            Item::BeginShape { .. } | Item::EndShape => {}
            Item::BeginEffect(_) | Item::EndEffect | Item::PushClip { .. } | Item::PopClip => out.push(it.clone()),
            _ => {
                if keep(cur) {
                    out.push(it.clone());
                }
            }
        }
    }
    out
}

/// Innehållets rektangel och renderingsytan (med marginal för effekter, inom bilden). `None` om inget
/// ritas.
fn extent(items: &[Item], scene: &Scene) -> Option<((f32, f32, f32, f32), (f32, f32, f32, f32))> {
    let mut b: Option<(f32, f32, f32, f32)> = None;
    let mut add = |x: f32, y: f32| {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        b = Some(match b {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    };
    let mut pad = 1.0f32;
    let mut reflect = 0.0f32;
    for it in items {
        match it {
            Item::Fill { path, .. } => {
                if let Some((x0, y0, x1, y1)) = path.bounds() {
                    add(x0, y0);
                    add(x1, y1);
                }
            }
            Item::Stroke { path, stroke } => {
                if let Some((x0, y0, x1, y1)) = path.bounds() {
                    // Breda linjer och pilspetsar.
                    let w = stroke.width * 2.0;
                    add(x0 - w, y0 - w);
                    add(x1 + w, y1 + w);
                }
            }
            Item::Image { t, .. } | Item::Svg { t, .. } => {
                for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                    let (x, y) = t.apply(x, y);
                    add(x, y);
                }
            }
            Item::Glyphs(g) => {
                let (mut gx0, mut gx1) = (f32::MAX, f32::MIN);
                let (mut gy0, mut gy1) = (f32::MAX, f32::MIN);
                for gl in &g.glyphs {
                    gx0 = gx0.min(gl.x);
                    gx1 = gx1.max(gl.x + gl.advance.max(g.size * 0.6));
                    gy0 = gy0.min(gl.y - g.size * 1.1);
                    gy1 = gy1.max(gl.y + g.size * 0.4);
                }
                if gx0 <= gx1 {
                    let xs = g.x_scale.max(0.1);
                    let o = g.outline.as_ref().map_or(0.0, |s| s.width);
                    for (x, y) in [(gx0 * xs - o - g.size * 0.3, gy0 - o), (gx1 * xs + o + g.size * 0.3, gy0 - o), (gx1 * xs + o + g.size * 0.3, gy1 + o), (gx0 * xs - o - g.size * 0.3, gy1 + o)] {
                        let (x, y) = g.t.apply(x, y);
                        add(x, y);
                    }
                }
            }
            Item::BeginEffect(es) => {
                for e in es {
                    match e {
                        Effect::OuterShadow { dx, dy, blur, t, .. } => {
                            let skew = if *t != Affine::IDENTITY { 40.0 } else { 0.0 };
                            pad = pad.max(dx.abs().max(dy.abs()) + blur * 2.0 + 2.0 + skew);
                        }
                        Effect::InnerShadow { .. } | Effect::SoftEdge { .. } | Effect::Thin { .. } | Effect::Bevel { .. } => {}
                        Effect::Contour { width, .. } => pad = pad.max(width + 2.0),
                        // Förvrängningen kan flytta formen var som helst på bilden: lagret täcker hela bilden.
                        Effect::Camera { .. } => pad = pad.max(scene.width.max(scene.height)),
                        Effect::Glow { radius, .. } => pad = pad.max(radius * 2.0 + 2.0),
                        Effect::Reflection { blur, dist, .. } => reflect = reflect.max(dist + blur * 2.0 + 2.0),
                    }
                }
            }
            _ => {}
        }
    }
    let content = b?;
    let (x0, y0, x1, y1) = content;
    let area = ((x0 - pad).max(0.0), (y0 - pad).max(0.0), (x1 + pad).min(scene.width), (y1 + pad + if reflect > 0.0 { (y1 - y0) + reflect } else { 0.0 }).min(scene.height));
    if area.2 <= area.0 || area.3 <= area.1 {
        return None;
    }
    Some((content, area))
}

/// Ett renderat lager: förmultiplicerad RGBA i bildpunktsrektangeln (x, y, b, h) av hela bildens
/// raster (`width`×`height`).
pub struct Rendered {
    pub kind: LayerKind,
    pub px: (u32, u32, u32, u32),
    pub rgba: Vec<u8>,
    pub geom: Geom,
}

/// Renderar lagren i `width`×`height` bildpunkter för hela bilden.
pub fn render(scene: &Scene, layers: &[LayerItems], width: u32, height: u32) -> Vec<Rendered> {
    let (w, h) = (width.max(1), height.max(1));
    let sx = w as f32 / scene.width.max(1.0);
    let sy = h as f32 / scene.height.max(1.0);
    let mut out = Vec::new();
    for (n, l) in layers.iter().enumerate() {
        let (ax0, ay0, ax1, ay1) = l.area;
        let x0 = ((ax0 * sx).floor().max(0.0) as u32).min(w - 1);
        let y0 = ((ay0 * sy).floor().max(0.0) as u32).min(h - 1);
        let x1 = ((ax1 * sx).ceil() as u32).clamp(x0 + 1, w);
        let y1 = ((ay1 * sy).ceil() as u32).clamp(y0 + 1, h);
        let Some(mut pm) = sk::Pixmap::new(x1 - x0, y1 - y0) else { continue };
        if n == 0 && l.kind == LayerKind::Static {
            pm.fill(sk::Color::WHITE);
        }
        let page = Affine::scale(sx, sy).then(&Affine::translate(-(x0 as f32), -(y0 as f32)));
        let sub = Scene { width: scene.width, height: scene.height, items: l.items.clone() };
        super::raster::draw(&mut pm, &sub, &page);
        out.push(Rendered { kind: l.kind, px: (x0, y0, x1 - x0, y1 - y0), rgba: pm.take(), geom: l.geom(scene.width, scene.height) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::color::Rgba;
    use crate::pptx::scene::{Paint, Path};

    fn rect_fill(x: f32, y: f32, c: Rgba) -> Item {
        Item::Fill { path: Path::rect(x, y, 10.0, 10.0), paint: Paint::Solid(c), even_odd: false }
    }

    fn scene() -> Scene {
        let red = Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
        let blue = Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
        let mut s = Scene::new(100.0, 50.0);
        s.items.push(Item::Fill { path: Path::rect(0.0, 0.0, 100.0, 50.0), paint: Paint::Solid(Rgba::WHITE), even_odd: false });
        s.items.push(Item::BeginShape { id: 2 });
        s.items.push(rect_fill(10.0, 10.0, red));
        s.items.push(Item::EndShape);
        s.items.push(Item::BeginShape { id: 3 });
        s.items.push(rect_fill(15.0, 15.0, blue));
        s.items.push(Item::Para(0));
        s.items.push(rect_fill(40.0, 10.0, red));
        s.items.push(Item::Para(1));
        s.items.push(Item::BeginEffect(vec![Effect::Glow { radius: 3.0, color: red }]));
        s.items.push(rect_fill(40.0, 30.0, blue));
        s.items.push(Item::EndEffect);
        s.items.push(Item::EndShape);
        s.items.push(Item::BeginShape { id: 4 });
        s.items.push(rect_fill(70.0, 10.0, blue));
        s.items.push(Item::EndShape);
        s
    }

    #[test]
    fn formintervall() {
        let s = scene();
        let r = s.shape_ranges();
        assert_eq!(r.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![2, 3, 4]);
        assert_eq!(r[0].1, 2..3);
    }

    #[test]
    fn utan_mål_ett_lager() {
        let s = scene();
        let l = split(&s, &[]);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].kind, LayerKind::Static);
        assert!(!l[0].items.iter().any(|i| matches!(i, Item::BeginShape { .. } | Item::EndShape | Item::Para(_))));
    }

    #[test]
    fn animerad_form_under_fast_form_får_eget_lager_i_ordning() {
        let s = scene();
        let t2 = Target { shape: 2, para: None };
        let l = split(&s, &[t2]);
        let kinds: Vec<LayerKind> = l.iter().map(|x| x.kind).collect();
        assert_eq!(kinds, vec![LayerKind::Static, LayerKind::Target(t2), LayerKind::Static]);
        assert_eq!(l[1].content, (10.0, 10.0, 20.0, 20.0));
        let g = l[1].geom(100.0, 50.0);
        assert!((g.cx - 0.15).abs() < 1e-6 && (g.w - 0.1).abs() < 1e-6);
    }

    #[test]
    fn stycken_för_sig_och_formen_kvar_i_det_fasta() {
        let s = scene();
        let p1 = Target { shape: 3, para: Some(1) };
        let l = split(&s, &[p1, Target { shape: 3, para: Some(7) }]);
        let kinds: Vec<LayerKind> = l.iter().map(|x| x.kind).collect();
        // Stycke 7 finns inte och hoppas över.
        assert_eq!(kinds, vec![LayerKind::Static, LayerKind::Target(p1), LayerKind::Static]);
        // Styckelagret har effektgruppen kvar (balanserad) och bara sitt stycke.
        let fills = l[1].items.iter().filter(|i| matches!(i, Item::Fill { .. })).count();
        assert_eq!(fills, 1);
        assert!(matches!(l[1].items[0], Item::BeginEffect(_)));
        // Glöden ger marginal.
        assert!(l[1].area.0 < l[1].content.0 - 5.0);
        // Det fasta lagret före har formens geometri och stycke 0, och effektgruppen tom.
        let f0 = l[0].items.iter().filter(|i| matches!(i, Item::Fill { .. })).count();
        assert_eq!(f0, 4, "bakgrund, form 2, formens geometri, stycke 0");
    }

    #[test]
    fn lagren_ger_samma_bild_som_hela_renderingen() {
        let s = scene();
        let (w, h) = (200u32, 100u32);
        let whole = super::super::raster::render(&s, w, h).unwrap();
        for targets in [vec![], vec![Target { shape: 2, para: None }], vec![Target { shape: 3, para: Some(1) }, Target { shape: 4, para: None }]] {
            let layers = split(&s, &targets);
            let rendered = render(&s, &layers, w, h);
            let mut acc = sk::Pixmap::new(w, h).unwrap();
            for r in &rendered {
                let pm = sk::Pixmap::from_vec(r.rgba.clone(), sk::IntSize::from_wh(r.px.2, r.px.3).unwrap()).unwrap();
                acc.draw_pixmap(r.px.0 as i32, r.px.1 as i32, pm.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), None);
            }
            let diff = whole.data().iter().zip(acc.data()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
            assert!(diff <= 2, "{targets:?}: största skillnad {diff}");
        }
    }

    #[test]
    fn tom_effektgrupp_ritar_inget() {
        let mut s = Scene::new(20.0, 20.0);
        s.items.push(Item::BeginEffect(vec![Effect::OuterShadow { dx: 2.0, dy: 2.0, blur: 3.0, color: Rgba::BLACK, t: Affine::IDENTITY }]));
        s.items.push(Item::EndEffect);
        let mut pm = sk::Pixmap::new(20, 20).unwrap();
        super::super::raster::draw(&mut pm, &s, &Affine::IDENTITY);
        assert!(pm.data().iter().all(|&b| b == 0));
        assert!(extent(&s.items, &s).is_none());
    }

    /// `*.pptx`/`*.ppsx` under `dir` (rekursivt), sorterade.
    fn decks_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                decks_under(&p, out);
            } else if p.extension().is_some_and(|x| x == "pptx" || x == "ppsx") {
                out.push(p);
            }
        }
        out.sort();
    }

    /// Alla presentationer under `KLADD_PPTX_REFS` (även i undermappar): tidsträd och övergångar tolkas
    /// och lagren delas utan panik; skriver hur många mål som hittas i visningslistan (resten visas alltid).
    /// Kontrollerar också utlösare, `iterate`/`charRg` och mediakommandon: inget syns före sin in-effekt,
    /// allt utan ut-effekt syns till slut (huvudsekvensen och alla utlösare klara; en uppdelad text ritas då
    /// som helhet), alla `p:cmd` blir mediakommandon, och att materialet har sådana.
    #[test]
    #[ignore]
    fn referensmaterialets_animeringar() {
        use super::super::anim::{Class, Timeline};
        let Ok(dir) = std::env::var("KLADD_PPTX_REFS") else { return };
        let mut decks = Vec::new();
        decks_under(std::path::Path::new(&dir), &mut decks);
        let (mut targets, mut found, mut clicks, mut trans) = (0usize, 0usize, 0usize, 0usize);
        let (mut iterate, mut scopes, mut scopes_found, mut triggers, mut dropped, mut cmds, mut pieces, mut panics) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        for p in decks {
            let Ok(bytes) = std::fs::read(&p) else { continue };
            let Ok(deck) = std::panic::catch_unwind(|| crate::pptx::Deck::open(&bytes)).unwrap_or(Err(String::new())) else { continue };
            for (i, s) in deck.slides.iter().enumerate() {
                let mut tl = Timeline::of_slide(&s.root);
                let tr = super::super::transition::Transition::of_slide(&s.root);
                if tr.animated() {
                    trans += 1;
                    *kinds.entry(format!("{:?}", tr.kind)).or_default() += 1;
                }
                clicks += tl.clicks();
                // Alla `p:cmd` med anrop och känt kommando blir mediakommandon.
                let mut raw = 0usize;
                s.root.walk(&mut |n| {
                    if n.name == "p:cmd" && matches!(n.attr("type"), None | Some("call")) && n.attr("cmd").is_some_and(|c| ["playfrom", "play", "togglepause", "stop", "pause", "resume"].contains(&c.split('(').next().unwrap_or("").trim().to_ascii_lowercase().as_str())) {
                        raw += 1;
                    }
                });
                let got_cmds = (0..=tl.clicks()).map(|k| tl.chain_cmds(k).len()).sum::<usize>() + tl.triggers.iter().map(|t| t.cmds().len()).sum::<usize>();
                assert_eq!(got_cmds, raw, "{} bild {}: mediakommandon", p.display(), i + 1);
                cmds += got_cmds;
                iterate += tl.all_effects().filter(|e| e.iter.is_some() || e.chars.is_some()).count();
                // Utlösarna som går att träffa behålls (som `ShowView::new`).
                let before = tl.triggers.len();
                if before > 0 {
                    let ok: HashSet<u32> = super::super::edit_ops::shapes(&deck, i).iter().filter(|h| !h.empty_ph && h.xf.cx > 0 && h.xf.cy > 0).map(|h| h.id).collect();
                    tl.retain_triggers(|id| ok.contains(&id));
                }
                triggers += tl.triggers.len();
                dropped += before - tl.triggers.len();
                let ts = tl.targets();
                if ts.is_empty() {
                    continue;
                }
                let sc = tl.unit_scopes();
                let Ok(built) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let scene = super::super::render::slide_scene(&deck, i);
                    let layers = split_units(&scene, &ts, &sc);
                    // Lagren kan renderas.
                    let _ = render(&scene, &layers, 64, 48);
                    (scene, layers)
                })) else {
                    panics += 1;
                    println!("{} bild {}: panik vid lagerdelning", p.display(), i + 1);
                    continue;
                };
                let (scene, layers) = built;
                let got: HashSet<Target> = layers.iter().filter_map(|l| if let LayerKind::Target(t) = l.kind { Some(t) } else { None }).collect();
                targets += ts.len();
                found += got.len();
                scopes += sc.len();
                scopes_found += sc.iter().filter(|c| layers.iter().any(|l| matches!(l.kind, LayerKind::Piece { scope, .. } if scope.shape == c.shape && scope.para == c.para))).count();
                pieces += layers.iter().filter(|l| matches!(l.kind, LayerKind::Piece { .. })).count();
                // Aldrig för tidigt, aldrig aldrig: dolda mål syns inte i början, mål utan ut-effekt syns
                // helt när alla steg och utlösare är klara; alla tillstånd är ändliga.
                let last = tl.groups_at_step(tl.clicks());
                let start = tl.chain(0).start;
                let idle = vec![-1.0f32; tl.triggers.len()];
                let done = vec![f32::INFINITY; tl.triggers.len()];
                let unit_of = |k: LayerKind| match k {
                    LayerKind::Target(t) => Some((t, None)),
                    LayerKind::Piece { scope, unit } => Some((scope, Some(unit))),
                    LayerKind::Rest(scope) => Some((scope, Some(0))),
                    LayerKind::Static => None,
                };
                let state_at = |l: &LayerItems, chain: std::ops::Range<usize>, t: f32, trig: &[f32]| {
                    let Some((tg, u)) = unit_of(l.kind) else { return State::SHOWN };
                    tl.layer_state(tg, u, l.geom(scene.width, scene.height), chain, t, trig)
                };
                let kinds_v: Vec<LayerKind> = layers.iter().map(|l| l.kind).collect();
                let final_plan = plan(&kinds_v, &|i| state_at(&layers[i], last..last, f32::INFINITY, &done));
                for (li, l) in layers.iter().enumerate() {
                    let Some((t, u)) = unit_of(l.kind) else { continue };
                    let scope_has_chars = tl.all_effects().any(|e| e.target == t && e.chars.is_some());
                    // Före: bara huvudsekvensens första läge och orörda utlösare.
                    let st0 = state_at(l, start..start, f32::INFINITY, &idle);
                    let hidden = tl.initially_hidden(t) || tl.triggers.iter().any(|tr| tr.affects(t) && !tr.state(t, u, l.geom(scene.width, scene.height), -1.0).visible);
                    if !scope_has_chars && matches!(l.kind, LayerKind::Target(_) | LayerKind::Piece { .. }) {
                        assert!(!(hidden && st0.visible), "{} bild {}: {:?} syns före sin in-effekt", p.display(), i + 1, l.kind);
                    }
                    let exits = tl.all_effects().any(|e| e.target == t && e.class == Class::Exit);
                    if !exits {
                        // Slutet: en uppdelad text ritas som helhet, annars lagret självt.
                        if let LayerKind::Target(_) = l.kind {
                            let whole = state_at(l, last..last, f32::INFINITY, &done);
                            // Ett slutläge utanför ursprungsplatsen (t.ex. spiral in med ett absolut slutvärde,
                            // så för hela formen också) delas inte av delarna: då ritas de var för sig.
                            let moved = whole.dx.abs() > 1e-3 || whole.dy.abs() > 1e-3;
                            if !moved {
                                let s = final_plan[li].expect("helhetslagret ritas i slutet");
                                assert!(s.visible && s.opacity > 0.99 && s.reveal.is_none(), "{} bild {}: {t:?} syns inte till slut: {s:?}", p.display(), i + 1);
                            }
                        }
                        if let LayerKind::Piece { scope, .. } | LayerKind::Rest(scope) = l.kind {
                            let moved = layers.iter().position(|x| x.kind == LayerKind::Target(scope)).is_some_and(|k| {
                                let w = state_at(&layers[k], last..last, f32::INFINITY, &done);
                                w.dx.abs() > 1e-3 || w.dy.abs() > 1e-3
                            });
                            assert!(moved || final_plan[li].is_none(), "{} bild {}: delarna ritas i slutet", p.display(), i + 1);
                        }
                    }
                    for k in 0..=tl.clicks() {
                        let d = tl.chain_dur(k);
                        for tt in [-1.0, 0.0, d / 2.0, f32::INFINITY] {
                            let st = state_at(l, tl.chain(k), tt, &idle);
                            assert!([st.opacity, st.dx, st.dy, st.sx, st.sy, st.rot].iter().all(|v| v.is_finite()), "{} bild {}: {t:?} {st:?}", p.display(), i + 1);
                        }
                    }
                }
                for t in &ts {
                    if !got.contains(t) && !(t.para.is_none() && tl.all_effects().any(|e| e.target == *t && e.chars.is_some())) {
                        println!("{} bild {}: målet {t:?} finns inte i visningslistan", p.display(), i + 1);
                    }
                }
            }
        }
        println!("övergångar {trans} {kinds:?}; klick {clicks}; mål {targets}, hittade {found}");
        println!("uppdelade effekter {iterate}, texter {scopes} (med delar {scopes_found}, {pieces} delar); utlösare {triggers} (borttagna {dropped}); mediakommandon {cmds}; paniker {panics}");
        assert!(iterate > 0 && triggers > 0 && cmds > 0, "materialet saknar iterate/utlösare/kommandon: {iterate} {triggers} {cmds}");
        assert_eq!(panics, 0);
    }
}
