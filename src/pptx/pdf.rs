//! Ritar `Scene` till PDF med krilla. Samma visningslista som rastret, så PDF:en blir lik skärmen;
//! glyferna läggs in som riktig text (med typsnittet inbäddat och delmängdat av krilla).
//!
//! Gränssnittet (Arkiv → Exportera som PDF för presentationer) görs av nästa omgång; provet
//! `pdf_har_en_sida_per_bild` kör vägen redan nu.

// Exporten kopplas till Arkiv-menyn av nästa omgång.
#![allow(dead_code)]

use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, LineCap, LineJoin, LinearGradient, RadialGradient, SpreadMethod, Stop as KStop, Stroke as KStroke, StrokeDash};
use krilla::Document;

use super::color::Rgba;
use super::model::Deck;
use super::scene::{Affine, Cap, Cmd, Item, Join, Paint, Path, Scene, Stroke};

fn tf(a: &Affine) -> Transform {
    Transform::from_row(a.a, a.b, a.c, a.d, a.e, a.f)
}

fn kpath(p: &Path) -> Option<krilla::geom::Path> {
    let mut b = PathBuilder::new();
    let mut open = false;
    for c in &p.cmds {
        match *c {
            Cmd::Move(x, y) => {
                b.move_to(x, y);
                open = true;
            }
            Cmd::Line(x, y) => {
                if open {
                    b.line_to(x, y)
                } else {
                    b.move_to(x, y);
                    open = true;
                }
            }
            Cmd::Quad(a, c2, x, y) if open => b.quad_to(a, c2, x, y),
            Cmd::Cubic(a, c2, d, e, x, y) if open => b.cubic_to(a, c2, d, e, x, y),
            Cmd::Close if open => b.close(),
            _ => {}
        }
    }
    b.finish()
}

fn kcolor(c: Rgba) -> rgb::Color {
    let [r, g, b, _] = c.to_u8();
    rgb::Color::new(r, g, b)
}

fn norm(v: f32) -> NormalizedF32 {
    NormalizedF32::new(v.clamp(0.0, 1.0)).unwrap_or(NormalizedF32::ONE)
}

fn kstops(stops: &[super::scene::Stop]) -> Vec<KStop> {
    stops.iter().map(|s| KStop { offset: norm(s.pos), color: kcolor(s.color).into(), opacity: norm(s.color.a) }).collect()
}

/// Paint → (krilla-paint, täckning). Bildfyllningar hanteras av anroparen.
fn kpaint(p: &Paint) -> Option<(krilla::paint::Paint, f32)> {
    Some(match p {
        Paint::Solid(c) => (kcolor(*c).into(), c.a),
        Paint::Linear { x0, y0, x1, y1, stops } => (
            LinearGradient { x1: *x0, y1: *y0, x2: *x1, y2: *y1, transform: Transform::identity(), spread_method: SpreadMethod::Pad, stops: kstops(stops), anti_alias: true }.into(),
            1.0,
        ),
        Paint::Radial { cx, cy, r, stops, t } => (
            RadialGradient { fx: *cx, fy: *cy, fr: 0.0, cx: *cx, cy: *cy, cr: r.max(0.01), transform: tf(t), spread_method: SpreadMethod::Pad, stops: kstops(stops), anti_alias: true }.into(),
            1.0,
        ),
        Paint::Image { .. } => (kcolor(p.average()).into(), 1.0),
    })
}

fn kstroke(s: &Stroke) -> Option<KStroke> {
    let (paint, opacity) = kpaint(&s.paint)?;
    Some(KStroke {
        paint,
        width: s.width.max(0.01),
        miter_limit: 4.0,
        line_cap: match s.cap {
            Cap::Butt => LineCap::Butt,
            Cap::Round => LineCap::Round,
            Cap::Square => LineCap::Square,
        },
        line_join: match s.join {
            Join::Miter => LineJoin::Miter,
            Join::Round => LineJoin::Round,
            Join::Bevel => LineJoin::Bevel,
        },
        opacity: norm(opacity),
        dash: (s.dash.len() >= 2).then(|| StrokeDash { array: s.dash.clone(), offset: 0.0 }),
    })
}

fn kimage(img: &super::scene::ImageData) -> Option<krilla::image::Image> {
    if let Some(j) = &img.jpeg {
        if let Ok(i) = krilla::image::Image::from_jpeg(krilla::Data::from(j.clone()), true) {
            return Some(i);
        }
    }
    Some(krilla::image::Image::from_rgba8(img.rgba.clone(), img.width, img.height))
}

/// Ritar en bild (enhetskvadrat → `t`, med beskärning) på ytan.
fn draw_image(s: &mut krilla::surface::Surface<'_>, image: krilla::image::Image, t: &Affine, crop: [f32; 4], alpha: f32) {
    let [l, tp, r, b] = crop;
    let cw = (1.0 - l - r).max(1e-4);
    let ch = (1.0 - tp - b).max(1e-4);
    let Some(clip) = kpath(&Path::rect(0.0, 0.0, 1.0, 1.0).transformed(t)) else { return };
    s.push_clip_path(&clip, &FillRule::NonZero);
    if alpha < 1.0 {
        s.push_opacity(norm(alpha));
    }
    let map = Affine::translate(-l / cw, -tp / ch).then(t);
    s.push_transform(&tf(&map));
    if let Some(size) = Size::from_wh(1.0 / cw, 1.0 / ch) {
        s.draw_image(image, size);
    }
    s.pop();
    if alpha < 1.0 {
        s.pop();
    }
    s.pop();
}

fn draw_scene(s: &mut krilla::surface::Surface<'_>, scene: &Scene) {
    let mut depth = 0usize;
    for (idx, it) in scene.items.iter().enumerate() {
        match it {
            Item::BeginEffect(effects) => {
                // Skugga, glöd och spegling som rasterbild under innehållet (144 dpi); innehållet ritas som vanligt.
                let end = super::raster::matching_end(&scene.items, idx).min(scene.items.len());
                if let Some((pm, t)) = super::effects::underlay(&scene.items[idx + 1..end], effects, scene.width, scene.height, 2.0) {
                    let (w, h, rgba) = super::raster::to_rgba(pm);
                    let img = super::scene::ImageData::new(w, h, rgba, None);
                    if let Some(k) = kimage(&img) {
                        draw_image(s, k, &Affine::scale(w as f32, h as f32).then(&t), [0.0; 4], 1.0);
                    }
                }
            }
            Item::EndEffect => {}
            Item::Fill { path, paint, even_odd } => {
                if let Paint::Image { image, alpha, tile: true, .. } = paint {
                    // Upprepad bild (mönster, `a:tile`): bildens medelfärg.
                    let n = (image.rgba.len() / 4).max(1) as f32;
                    let sum = image.rgba.chunks_exact(4).fold([0.0f32; 4], |a, p| [a[0] + p[0] as f32, a[1] + p[1] as f32, a[2] + p[2] as f32, a[3] + p[3] as f32]);
                    let c = Rgba { r: sum[0] / n / 255.0, g: sum[1] / n / 255.0, b: sum[2] / n / 255.0, a: sum[3] / n / 255.0 * alpha };
                    let (Some(p), Some((paint, opacity))) = (kpath(path), kpaint(&Paint::Solid(c))) else { continue };
                    s.set_stroke(None);
                    s.set_fill(Some(Fill { paint, opacity: norm(opacity), rule: if *even_odd { FillRule::EvenOdd } else { FillRule::NonZero } }));
                    s.draw_path(&p);
                    continue;
                }
                if let Paint::Image { image, t, alpha, .. } = paint {
                    let Some(clip) = kpath(path) else { continue };
                    s.push_clip_path(&clip, &FillRule::NonZero);
                    if let Some(img) = kimage(image) {
                        draw_image(s, img, t, [0.0; 4], *alpha);
                    }
                    s.pop();
                    continue;
                }
                let (Some(p), Some((paint, opacity))) = (kpath(path), kpaint(paint)) else { continue };
                s.set_stroke(None);
                s.set_fill(Some(Fill { paint, opacity: norm(opacity), rule: if *even_odd { FillRule::EvenOdd } else { FillRule::NonZero } }));
                s.draw_path(&p);
            }
            Item::Stroke { path, stroke } => {
                let (Some(p), Some(st)) = (kpath(path), kstroke(stroke)) else { continue };
                s.set_fill(None);
                s.set_stroke(Some(st));
                s.draw_path(&p);
                s.set_stroke(None);
            }
            Item::Image { image, t, crop, alpha } => {
                if let Some(img) = kimage(image) {
                    draw_image(s, img, t, *crop, *alpha);
                }
            }
            Item::Svg { tree, t, crop, alpha } => {
                use krilla_svg::SurfaceExt;
                let size = tree.size();
                let (w, h) = (size.width().max(1e-3), size.height().max(1e-3));
                let [l, tp, r, b] = *crop;
                let cw = (1.0 - l - r).max(1e-4);
                let ch = (1.0 - tp - b).max(1e-4);
                let Some(clip) = kpath(&Path::rect(0.0, 0.0, 1.0, 1.0).transformed(t)) else { continue };
                s.push_clip_path(&clip, &FillRule::NonZero);
                s.push_opacity(norm(*alpha));
                let map = Affine::translate(-l * w, -tp * h).then(&Affine::scale(1.0 / (w * cw), 1.0 / (h * ch))).then(t);
                s.push_transform(&tf(&map));
                if let Some(sz) = Size::from_wh(w, h) {
                    s.draw_svg(tree, sz, krilla_svg::SvgSettings::default());
                }
                s.pop();
                s.pop();
                s.pop();
            }
            Item::Glyphs(g) => {
                let Some(font) = g.font.pdf_font() else { continue };
                let Some((paint, opacity)) = kpaint(&g.paint) else { continue };
                if g.glyphs.is_empty() {
                    continue;
                }
                s.set_fill(Some(Fill { paint: paint.clone(), opacity: norm(opacity), rule: FillRule::NonZero }));
                s.set_stroke(if g.fake_bold {
                    Some(KStroke { paint, width: g.size * 0.035, opacity: norm(opacity), ..Default::default() })
                } else {
                    g.outline.as_ref().and_then(kstroke)
                });
                let skew = if g.fake_italic { Affine { c: -0.2, ..Affine::IDENTITY } } else { Affine::IDENTITY };
                let local = skew.then(&Affine::scale(g.x_scale, 1.0)).then(&g.t);
                s.push_transform(&tf(&local));
                let size = g.size.max(0.01);
                let text_len = g.text.len();
                let xs = g.x_scale.max(0.01);
                let glyphs: Vec<krilla::text::KrillaGlyph> = g
                    .glyphs
                    .iter()
                    .enumerate()
                    .map(|(i, pg)| {
                        let next_x = g.glyphs.get(i + 1).map(|n| n.x).unwrap_or(pg.x + pg.advance);
                        let adv = (next_x - pg.x) / xs / size;
                        // Hela körningens text hör till första glyfen (ett kluster för textlagret).
                        let range = if i == 0 { 0..text_len } else { 0..text_len };
                        krilla::text::KrillaGlyph::new(krilla::text::GlyphId::new(pg.id as u32), adv, 0.0, -pg.y / size, 0.0, range, None)
                    })
                    .collect();
                let start = Point::from_xy(g.glyphs[0].x / xs, 0.0);
                s.draw_glyphs(start, &glyphs, font, &g.text, size, false);
                s.pop();
                s.set_stroke(None);
            }
            Item::PushClip { path } => {
                if let Some(p) = kpath(path) {
                    s.push_clip_path(&p, &FillRule::NonZero);
                } else {
                    // Tom bana: klipp bort allt.
                    let Some(p) = kpath(&Path::rect(0.0, 0.0, 0.0001, 0.0001)) else { continue };
                    s.push_clip_path(&p, &FillRule::NonZero);
                }
                depth += 1;
            }
            Item::PopClip => {
                if depth > 0 {
                    s.pop();
                    depth -= 1;
                }
            }
        }
    }
    for _ in 0..depth {
        s.pop();
    }
}

/// Skriver bilderna `slides` (index) som en PDF.
pub fn write(deck: &Deck, slides: &[usize]) -> Result<Vec<u8>, String> {
    let mut doc = Document::new();
    for &i in slides {
        let scene = super::render::slide_scene(deck, i);
        let settings = PageSettings::from_wh(scene.width, scene.height).ok_or_else(|| crate::i18n::t("Ogiltig bildstorlek.").to_owned())?;
        let mut page = doc.start_page_with(settings);
        let mut s = page.surface();
        draw_scene(&mut s, &scene);
        s.finish();
        page.finish();
    }
    doc.finish().map_err(|e| crate::tf!("PDF:en kunde inte skapas: {e}", e = format!("{e:?}")))
}
