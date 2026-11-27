//! Bilden delad i lager för animeringar: varje animerat mål (form eller stycke, `anim::Target`) blir
//! ett eget lager, och det som inte animeras blir fasta lager mellan dem i ritordning (så att en
//! animerad form under en fast form fortfarande hamnar under den). Lagren renderas var för sig med
//! tiny-skia, beskurna till sitt innehåll (med marginal för skugga och glöd) och i samma bildpunktsrutnät
//! som hela bilden, så att lagren ovanpå varandra blir samma bild som en vanlig rendering.
//!
//! Formerna hittas med `Scene::shape_ranges` (`Item::BeginShape`), styckena med `Item::Para`.

use std::collections::HashSet;

use resvg::tiny_skia as sk;

use super::anim::{Geom, Target};
use super::scene::{Affine, Effect, Item, Scene};

/// Vad ett lager är.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    /// Det som inte animeras (det första fasta lagret har bakgrunden).
    Static,
    Target(Target),
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
pub fn split(scene: &Scene, targets: &[Target]) -> Vec<LayerItems> {
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
    }
    flush(&mut stat, &mut out, &mut first_static);
    if out.is_empty() {
        out.push(LayerItems { kind: LayerKind::Static, items: Vec::new(), content: page, area: page });
    }
    out
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

    /// Alla presentationer i `KLADD_PPTX_REFS`: tidsträd och övergångar tolkas och lagren delas utan
    /// panik; skriver hur många mål som hittas i visningslistan (resten visas alltid).
    #[test]
    #[ignore]
    fn referensmaterialets_animeringar() {
        let Ok(dir) = std::env::var("KLADD_PPTX_REFS") else { return };
        let mut decks: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path().join("deck.pptx")).filter(|p| p.exists()).collect();
        decks.sort();
        let (mut targets, mut found, mut clicks, mut trans) = (0usize, 0usize, 0usize, 0usize);
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        for p in decks {
            let Ok(deck) = crate::pptx::Deck::open(&std::fs::read(&p).unwrap()) else { continue };
            for (i, s) in deck.slides.iter().enumerate() {
                let tl = super::super::anim::Timeline::of_slide(&s.root);
                let tr = super::super::transition::Transition::of_slide(&s.root);
                if tr.animated() {
                    trans += 1;
                    *kinds.entry(format!("{:?}", tr.kind)).or_default() += 1;
                }
                clicks += tl.clicks();
                let ts = tl.targets();
                if ts.is_empty() {
                    continue;
                }
                let scene = super::super::render::slide_scene(&deck, i);
                let layers = split(&scene, &ts);
                let got: HashSet<Target> = layers.iter().filter_map(|l| if let LayerKind::Target(t) = l.kind { Some(t) } else { None }).collect();
                targets += ts.len();
                found += got.len();
                // Lagren kan renderas.
                let _ = render(&scene, &layers, 64, 48);
                // Aldrig för tidigt, aldrig aldrig: dolda mål syns inte i början, mål utan ut-effekt syns
                // helt när alla steg är klara; alla tillstånd är ändliga.
                let last = tl.groups_at_step(tl.clicks());
                for l in &layers {
                    let LayerKind::Target(t) = l.kind else { continue };
                    let g = l.geom(scene.width, scene.height);
                    let start = tl.chain(0).start;
                    let st0 = tl.state(t, g, start..start, f32::INFINITY);
                    assert!(!(tl.initially_hidden(t) && st0.visible), "{} bild {}: {t:?} syns före sin in-effekt", p.display(), i + 1);
                    let exits = tl.groups.iter().flat_map(|gr| &gr.effects).any(|e| e.target == t && e.class == super::super::anim::Class::Exit);
                    let end = tl.state(t, g, last..last, f32::INFINITY);
                    if !exits {
                        assert!(end.visible && end.opacity > 0.99 && end.reveal.is_none(), "{} bild {}: {t:?} syns inte till slut: {end:?}", p.display(), i + 1);
                    }
                    for k in 0..=tl.clicks() {
                        let d = tl.chain_dur(k);
                        for tt in [-1.0, 0.0, d / 2.0, f32::INFINITY] {
                            let st = tl.state(t, g, tl.chain(k), tt);
                            assert!([st.opacity, st.dx, st.dy, st.sx, st.sy, st.rot].iter().all(|v| v.is_finite()), "{} bild {}: {t:?} {st:?}", p.display(), i + 1);
                        }
                    }
                }
                for t in &ts {
                    if !got.contains(t) {
                        println!("{} bild {}: målet {t:?} finns inte i visningslistan", p.display(), i + 1);
                    }
                }
            }
        }
        println!("övergångar {trans} {kinds:?}; klick {clicks}; mål {targets}, hittade {found}");
    }
}
