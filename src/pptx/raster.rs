//! Ritar en `Scene` till en pixmap med tiny-skia (skärm, miniatyrer, bildspel och jämförelseprov).

use resvg::tiny_skia as sk;

use super::scene::{Affine, Cap, Cmd, GlyphRun, ImageData, Item, Join, Paint, Path, Scene, Stroke};

fn ts(a: &Affine) -> sk::Transform {
    sk::Transform::from_row(a.a, a.b, a.c, a.d, a.e, a.f)
}

pub(super) fn to_sk_path(p: &Path) -> Option<sk::Path> {
    sk_path(p)
}

fn sk_path(p: &Path) -> Option<sk::Path> {
    let mut b = sk::PathBuilder::new();
    let mut open = false;
    for c in &p.cmds {
        match *c {
            Cmd::Move(x, y) => {
                b.move_to(x, y);
                open = true;
            }
            Cmd::Line(x, y) => {
                if !open {
                    b.move_to(x, y);
                    open = true;
                } else {
                    b.line_to(x, y);
                }
            }
            Cmd::Quad(a, c2, x, y) => {
                if open {
                    b.quad_to(a, c2, x, y);
                }
            }
            Cmd::Cubic(a, c2, d, e, x, y) => {
                if open {
                    b.cubic_to(a, c2, d, e, x, y);
                }
            }
            Cmd::Close => {
                if open {
                    b.close();
                }
            }
        }
    }
    b.finish()
}

fn color(c: super::color::Rgba) -> sk::Color {
    sk::Color::from_rgba(c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0), c.a.clamp(0.0, 1.0)).unwrap_or(sk::Color::BLACK)
}

fn stops(s: &[super::scene::Stop]) -> Vec<sk::GradientStop> {
    s.iter().map(|s| sk::GradientStop::new(s.pos, color(s.color))).collect()
}

/// Bildens förmultiplicerade pixmap (skapas en gång).
fn image_pixmap(img: &ImageData) -> Option<&sk::Pixmap> {
    img.pixmap
        .get_or_init(|| {
            let mut data = img.rgba.clone();
            for p in data.chunks_exact_mut(4) {
                let a = p[3] as u32;
                if a < 255 {
                    p[0] = ((p[0] as u32 * a + 127) / 255) as u8;
                    p[1] = ((p[1] as u32 * a + 127) / 255) as u8;
                    p[2] = ((p[2] as u32 * a + 127) / 255) as u8;
                }
            }
            sk::Pixmap::from_vec(data, sk::IntSize::from_wh(img.width, img.height)?)
        })
        .as_ref()
}

fn shader<'a>(p: &'a Paint) -> Option<sk::Shader<'a>> {
    Some(match p {
        Paint::Solid(c) => sk::Shader::SolidColor(color(*c)),
        Paint::Linear { x0, y0, x1, y1, stops: s } => {
            sk::LinearGradient::new(sk::Point::from_xy(*x0, *y0), sk::Point::from_xy(*x1, *y1), stops(s), sk::SpreadMode::Pad, sk::Transform::identity())
                .unwrap_or_else(|| sk::Shader::SolidColor(color(p.average())))
        }
        Paint::Radial { cx, cy, r, stops: s, t } => sk::RadialGradient::new(sk::Point::from_xy(*cx, *cy), 0.0, sk::Point::from_xy(*cx, *cy), r.max(0.01), stops(s), sk::SpreadMode::Pad, ts(t))
            .unwrap_or_else(|| sk::Shader::SolidColor(color(p.average()))),
        Paint::Image { image, t, tile, alpha } => {
            let pm = image_pixmap(image)?;
            let unit = Affine::scale(1.0 / image.width as f32, 1.0 / image.height as f32).then(t);
            sk::Pattern::new(pm.as_ref(), if *tile { sk::SpreadMode::Repeat } else { sk::SpreadMode::Pad }, sk::FilterQuality::Bilinear, *alpha, ts(&unit))
        }
    })
}

fn sk_stroke(s: &Stroke) -> sk::Stroke {
    sk::Stroke {
        width: s.width.max(0.01),
        miter_limit: 4.0,
        line_cap: match s.cap {
            Cap::Butt => sk::LineCap::Butt,
            Cap::Round => sk::LineCap::Round,
            Cap::Square => sk::LineCap::Square,
        },
        line_join: match s.join {
            Join::Miter => sk::LineJoin::Miter,
            Join::Round => sk::LineJoin::Round,
            Join::Bevel => sk::LineJoin::Bevel,
        },
        dash: if s.dash.len() >= 2 && s.dash.len() % 2 == 0 { sk::StrokeDash::new(s.dash.clone(), 0.0) } else { None },
    }
}

/// Banan för en glyfkörning i körningens koordinater (punkter, y nedåt).
pub fn glyph_path(g: &GlyphRun) -> Path {
    let mut out = Path::default();
    let skew = if g.fake_italic { 0.2 } else { 0.0 };
    for pg in &g.glyphs {
        let Some(o) = g.font.outline(pg.id) else { continue };
        let s = g.size;
        let xs = g.x_scale;
        let m = |x: f32, y: f32| (pg.x + (x + skew * y) * s * xs, pg.y - y * s);
        for c in &o.cmds {
            match *c {
                Cmd::Move(x, y) => {
                    let (x, y) = m(x, y);
                    out.move_to(x, y)
                }
                Cmd::Line(x, y) => {
                    let (x, y) = m(x, y);
                    out.line_to(x, y)
                }
                Cmd::Quad(a, b, x, y) => {
                    let (a, b) = m(a, b);
                    let (x, y) = m(x, y);
                    out.quad_to(a, b, x, y)
                }
                Cmd::Cubic(a, b, c2, d, x, y) => {
                    let (a, b) = m(a, b);
                    let (c2, d) = m(c2, d);
                    let (x, y) = m(x, y);
                    out.cubic_to(a, b, c2, d, x, y)
                }
                Cmd::Close => out.close(),
            }
        }
    }
    out
}

/// Ritar bilden i `w`×`h` bildpunkter (bilden skalas så att den fyller ytan).
pub fn render(scene: &Scene, w: u32, h: u32) -> Option<sk::Pixmap> {
    let mut pm = sk::Pixmap::new(w.max(1), h.max(1))?;
    pm.fill(sk::Color::WHITE);
    let sx = w as f32 / scene.width.max(1.0);
    let sy = h as f32 / scene.height.max(1.0);
    let page = Affine::scale(sx, sy);
    draw(&mut pm, scene, &page);
    Some(pm)
}

/// Ritar visningslistan med transformen `page` (punkter → bildpunkter).
pub fn draw(pm: &mut sk::Pixmap, scene: &Scene, page: &Affine) {
    draw_items(pm, &scene.items, page);
}

/// Index för `EndEffect` som hör till `BeginEffect` på plats `i` (eller listans slut).
pub(super) fn matching_end(items: &[Item], i: usize) -> usize {
    let mut depth = 0usize;
    for (k, it) in items.iter().enumerate().skip(i) {
        match it {
            Item::BeginEffect(_) => depth += 1,
            Item::EndEffect => {
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

pub(super) fn draw_items(pm: &mut sk::Pixmap, items: &[Item], page: &Affine) {
    let pt = ts(page);
    let mut clips: Vec<sk::Mask> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let it = &items[i];
        i += 1;
        let mask = clips.last();
        match it {
            Item::BeginEffect(effects) => {
                let end = matching_end(items, i - 1);
                super::effects::draw(pm, &items[i..end.min(items.len())], effects, page, mask);
                i = end + 1;
            }
            Item::EndEffect | Item::BeginShape { .. } | Item::EndShape | Item::Para(_) => {}
            Item::Fill { path, paint, even_odd } => {
                let Some(p) = sk_path(path) else { continue };
                let Some(sh) = shader(paint) else { continue };
                let paint = sk::Paint { shader: sh, anti_alias: true, ..Default::default() };
                pm.fill_path(&p, &paint, if *even_odd { sk::FillRule::EvenOdd } else { sk::FillRule::Winding }, pt, mask);
            }
            Item::Stroke { path, stroke } => {
                let Some(p) = sk_path(path) else { continue };
                let Some(sh) = shader(&stroke.paint) else { continue };
                let paint = sk::Paint { shader: sh, anti_alias: true, ..Default::default() };
                pm.stroke_path(&p, &paint, &sk_stroke(stroke), pt, mask);
            }
            Item::Image { image, t, crop, alpha } => {
                let Some(img) = image_pixmap(image) else { continue };
                let [l, tp, r, b] = *crop;
                let cw = (1.0 - l - r).max(1e-4);
                let ch = (1.0 - tp - b).max(1e-4);
                // Bildpunkt → enhetskvadrat (med beskärning) → bild → skärm.
                let map = Affine::translate(-l * image.width as f32, -tp * image.height as f32)
                    .then(&Affine::scale(1.0 / (image.width as f32 * cw), 1.0 / (image.height as f32 * ch)))
                    .then(t);
                // tiny-skia lägger sidans transform (`pt`) på mönstret också.
                // Nedskalning med bilinjär filtrering blir kornig: välj bikubisk då.
                let dev = map.then(page);
                let scale = (dev.a * dev.a + dev.b * dev.b).sqrt();
                let q = if scale < 0.8 { sk::FilterQuality::Bicubic } else { sk::FilterQuality::Bilinear };
                let sh = sk::Pattern::new(img.as_ref(), sk::SpreadMode::Pad, q, *alpha, ts(&map));
                let unit = Path::rect(0.0, 0.0, 1.0, 1.0).transformed(t);
                let Some(p) = sk_path(&unit) else { continue };
                let paint = sk::Paint { shader: sh, anti_alias: true, ..Default::default() };
                pm.fill_path(&p, &paint, sk::FillRule::Winding, pt, mask);
            }
            Item::Svg { tree, t, crop, alpha } => {
                let size = tree.size();
                let (w, h) = (size.width().max(1e-3), size.height().max(1e-3));
                let [l, tp, r, b] = *crop;
                let cw = (1.0 - l - r).max(1e-4);
                let ch = (1.0 - tp - b).max(1e-4);
                let map = Affine::translate(-l * w, -tp * h).then(&Affine::scale(1.0 / (w * cw), 1.0 / (h * ch))).then(t).then(page);
                if crop.iter().all(|v| v.abs() < 1e-6) && mask.is_none() && *alpha >= 1.0 {
                    resvg::render(tree, ts(&map), &mut pm.as_mut());
                } else {
                    // Beskuren eller klippt: rita i ett eget lager och klipp.
                    let Some(mut layer) = sk::Pixmap::new(pm.width(), pm.height()) else { continue };
                    resvg::render(tree, ts(&map), &mut layer.as_mut());
                    let Some(mut m) = sk::Mask::new(pm.width(), pm.height()) else { continue };
                    if let Some(p) = sk_path(&Path::rect(0.0, 0.0, 1.0, 1.0).transformed(t)) {
                        m.fill_path(&p, sk::FillRule::Winding, true, pt);
                    }
                    if let Some(outer) = mask {
                        let mut combined = outer.clone();
                        combine(&mut combined, &m);
                        m = combined;
                    }
                    let paint = sk::PixmapPaint { opacity: alpha.clamp(0.0, 1.0), ..Default::default() };
                    pm.draw_pixmap(0, 0, layer.as_ref(), &paint, sk::Transform::identity(), Some(&m));
                }
            }
            Item::Glyphs(g) => {
                let path = glyph_path(g);
                let Some(p) = sk_path(&path) else { continue };
                let Some(sh) = shader(&g.paint) else { continue };
                let paint = sk::Paint { shader: sh, anti_alias: true, ..Default::default() };
                let t = ts(&g.t.then(page));
                pm.fill_path(&p, &paint, sk::FillRule::Winding, t, mask);
                if g.fake_bold {
                    let s = sk::Stroke { width: g.size * 0.035, ..Default::default() };
                    pm.stroke_path(&p, &paint, &s, t, mask);
                }
                if let Some(o) = &g.outline {
                    if let Some(sh) = shader(&o.paint) {
                        let paint = sk::Paint { shader: sh, anti_alias: true, ..Default::default() };
                        pm.stroke_path(&p, &paint, &sk_stroke(o), t, mask);
                    }
                }
            }
            Item::PushClip { path } => {
                let Some(mut m) = sk::Mask::new(pm.width(), pm.height()) else { continue };
                if let Some(p) = sk_path(path) {
                    m.fill_path(&p, sk::FillRule::Winding, true, pt);
                }
                if let Some(outer) = clips.last() {
                    let mut combined = outer.clone();
                    combine(&mut combined, &m);
                    m = combined;
                }
                clips.push(m);
            }
            Item::PopClip => {
                clips.pop();
            }
        }
    }
}

/// `a` ∩ `b` (multiplicerar täckningen).
fn combine(a: &mut sk::Mask, b: &sk::Mask) {
    for (x, y) in a.data_mut().iter_mut().zip(b.data()) {
        *x = ((*x as u32 * *y as u32 + 127) / 255) as u8;
    }
}

/// Pixmapen som RGBA utan förmultiplicering (för egui-texturer och PNG).
pub fn to_rgba(pm: sk::Pixmap) -> (u32, u32, Vec<u8>) {
    let (w, h) = (pm.width(), pm.height());
    (w, h, pm.take_demultiplied())
}
