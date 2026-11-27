//! Formeffekter (`a:effectLst`, `p:style/a:effectRef` → temats `a:effectStyleLst`): yttre och inre
//! skugga, glöd, mjuka kanter och spegling, plus syntetiskt tunn text.
//!
//! `effect_list` gör om XML:en till `scene::Effect` (mått i punkter). Rastret ritar elementen mellan
//! `BeginEffect` och `EndEffect` i ett eget lager som bara är så stort som innehållet (plus effektens
//! räckvidd), och bygger effekterna ur lagrets täckning (alfa): skuggor och glöd är oskarpa, färgade
//! kopior under innehållet, mjuka kanter och inre skugga ändrar lagret innan det läggs på.
//!
//! Oskärpan är en gaussisk approximation (tre lådfilter), med σ = halva oskärperadien, som i
//! LibreOffice. PDF:en (`pdf.rs`) får effekterna under innehållet som en rasterbild (`underlay`).

use resvg::tiny_skia as sk;

use super::color::{self, Rgba};
use super::inherit::Sheets;
use super::render::emu;
use super::scene::{Affine, Effect, Item};
use super::xml::Node;

// ---------------------------------------------------------------------------------------------
// Tolkning

fn pct(n: &Node, name: &str, def: f32) -> f32 {
    n.attr(name).and_then(super::xml::parse_i64).map_or(def, |v| v as f32 / 100000.0)
}

fn len(n: &Node, name: &str) -> f32 {
    n.attr_i64(name).map_or(0.0, emu)
}

/// Förskjutning ur `dist` (EMU) och `dir` (60 000-dels grader, medurs från x-axeln).
fn offset(n: &Node) -> (f32, f32) {
    let d = len(n, "dist");
    let a = (n.attr_i64("dir").unwrap_or(0) as f32 / 60000.0).to_radians();
    (d * a.cos(), d * a.sin())
}

/// Justeringspunkten (`algn`) i en ruta `w`×`h`.
fn align_point(algn: Option<&str>, w: f32, h: f32) -> (f32, f32) {
    match algn.unwrap_or("b") {
        "tl" => (0.0, 0.0),
        "t" => (w / 2.0, 0.0),
        "tr" => (w, 0.0),
        "l" => (0.0, h / 2.0),
        "ctr" => (w / 2.0, h / 2.0),
        "r" => (w, h / 2.0),
        "bl" => (0.0, h),
        "br" => (w, h),
        _ => (w / 2.0, h),
    }
}

/// Effekterna i en `a:effectLst`. `frame` avbildar formens lokala ruta (0..w × 0..h) på bilden och
/// behövs för skuggans skalning runt justeringspunkten; `ph` är stilmatrisens platshållarfärg.
pub fn effect_list(sheets: &Sheets<'_>, lst: &Node, ph: Option<Rgba>, frame: (&Affine, f32, f32)) -> Vec<Effect> {
    let (ft, w, h) = frame;
    let col = |n: &Node| color::first_color(n).and_then(|c| sheets.color(c, ph));
    let mut out = Vec::new();
    for e in &lst.children {
        match e.name.as_str() {
            "a:outerShdw" | "a:prstShdw" => {
                let Some(c) = col(e) else { continue };
                let (dx, dy) = offset(e);
                let blur = len(e, "blurRad");
                let (sx, sy) = (pct(e, "sx", 1.0), pct(e, "sy", 1.0));
                let kx = (e.attr_i64("kx").unwrap_or(0) as f32 / 60000.0).to_radians().tan();
                let ky = (e.attr_i64("ky").unwrap_or(0) as f32 / 60000.0).to_radians().tan();
                let mut t = Affine::IDENTITY;
                if (sx - 1.0).abs() > 1e-4 || (sy - 1.0).abs() > 1e-4 || kx != 0.0 || ky != 0.0 {
                    // Skalning och skjuvning runt justeringspunkten, i formens koordinater.
                    let (ax, ay) = align_point(e.attr("algn"), w, h);
                    let local = Affine::translate(-ax, -ay).then(&Affine { a: sx, b: ky * sy, c: kx * sx, d: sy, e: 0.0, f: 0.0 }).then(&Affine::translate(ax, ay));
                    if let Some(inv) = ft.invert() {
                        t = inv.then(&local).then(ft);
                    }
                }
                out.push(Effect::OuterShadow { dx, dy, blur, color: c, t });
            }
            "a:innerShdw" => {
                let Some(c) = col(e) else { continue };
                let (dx, dy) = offset(e);
                out.push(Effect::InnerShadow { dx, dy, blur: len(e, "blurRad"), color: c });
            }
            "a:glow" => {
                let Some(c) = col(e) else { continue };
                let r = len(e, "rad");
                if r > 0.0 {
                    out.push(Effect::Glow { radius: r, color: c });
                }
            }
            "a:softEdge" => {
                let r = len(e, "rad");
                if r > 0.0 {
                    out.push(Effect::SoftEdge { radius: r });
                }
            }
            "a:reflection" => {
                // Bara den vanliga speglingen nedåt (dir 90°, sy −100 %).
                let dir = e.attr_i64("dir").unwrap_or(5_400_000);
                if (dir - 5_400_000).abs() > 60000 && e.attr("dir").is_some() {
                    continue;
                }
                out.push(Effect::Reflection {
                    blur: len(e, "blurRad"),
                    dist: len(e, "dist"),
                    start_alpha: pct(e, "stA", 1.0),
                    end_alpha: pct(e, "endA", 0.0),
                    start_pos: pct(e, "stPos", 0.0),
                    end_pos: pct(e, "endPos", 1.0),
                });
            }
            _ => {}
        }
    }
    out
}

/// En forms effekter med arv: första `a:effectLst`/`a:effectDag` i kedjans `spPr` (en tom lista stänger av
/// effekterna), annars `p:style/a:effectRef` mot temats `a:effectStyleLst`.
pub fn shape_effects(sheets: &Sheets<'_>, chain: &[&Node], style: Option<&Node>, frame: (&Affine, f32, f32)) -> Vec<Effect> {
    if let Some(lst) = chain.iter().find_map(|s| s.child("p:spPr").and_then(|p| p.children.iter().find(|c| matches!(c.name.as_str(), "a:effectLst" | "a:effectDag")))) {
        return if lst.name == "a:effectLst" { effect_list(sheets, lst, None, frame) } else { Vec::new() };
    }
    let Some(r) = style.and_then(|s| s.child("a:effectRef")) else { return Vec::new() };
    let idx = r.attr_i64("idx").unwrap_or(0);
    if idx < 1 {
        return Vec::new();
    }
    let ph = color::first_color(r).and_then(|c| sheets.color(c, None));
    let Some(es) = sheets.theme.effect_styles.get(idx as usize - 1) else { return Vec::new() };
    match es.child("a:effectLst") {
        Some(l) => effect_list(sheets, l, ph, frame),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// Rastret

/// Ungefärlig omslutande rektangel i bildpunkter för elementen.
fn bounds(items: &[Item], page: &Affine) -> Option<(f32, f32, f32, f32)> {
    let mut b: Option<(f32, f32, f32, f32)> = None;
    let mut add_rect = |t: &Affine, r: (f32, f32, f32, f32)| {
        for (x, y) in [(r.0, r.1), (r.2, r.1), (r.0, r.3), (r.2, r.3)] {
            let (x, y) = t.apply(x, y);
            b = Some(match b {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    };
    for it in items {
        match it {
            Item::Fill { path, .. } => {
                if let Some(r) = path.bounds() {
                    add_rect(page, r);
                }
            }
            Item::Stroke { path, stroke } => {
                if let Some(r) = path.bounds() {
                    let w = stroke.width * 2.0 + 2.0;
                    add_rect(page, (r.0 - w, r.1 - w, r.2 + w, r.3 + w));
                }
            }
            Item::Image { t, .. } | Item::Svg { t, .. } => add_rect(&t.then(page), (0.0, 0.0, 1.0, 1.0)),
            Item::Glyphs(g) => {
                let (Some(first), Some(last)) = (g.glyphs.first(), g.glyphs.last()) else { continue };
                let s = g.size;
                let x1 = last.x + last.advance.max(s) * g.x_scale.max(1.0);
                add_rect(&g.t.then(page), (first.x.min(0.0) - s * 0.3, -s * 1.3, x1 + s * 0.3, s * 0.6));
            }
            _ => {}
        }
    }
    b
}

/// En täckningsbuffert (0..1) med storlek.
struct Alpha {
    w: usize,
    h: usize,
    a: Vec<f32>,
}

impl Alpha {
    /// Lagrets alfa med `m` bildpunkters luft runt om (fylld med `outside`).
    fn from_layer(l: &sk::Pixmap, m: usize, outside: f32) -> Alpha {
        let (lw, lh) = (l.width() as usize, l.height() as usize);
        let (w, h) = (lw + 2 * m, lh + 2 * m);
        let mut a = vec![outside; w * h];
        let data = l.data();
        for y in 0..lh {
            for x in 0..lw {
                a[(y + m) * w + x + m] = data[(y * lw + x) * 4 + 3] as f32 / 255.0;
            }
        }
        Alpha { w, h, a }
    }

    /// Gaussisk oskärpa (tre lådfilter i varje led) med standardavvikelsen `sigma` bildpunkter.
    fn blur(&mut self, sigma: f32, outside: f32) {
        if sigma < 0.3 {
            return;
        }
        let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
        let r = ((ideal - 1.0) / 2.0).round().max(1.0) as usize;
        let mut tmp = vec![0.0f32; self.w.max(self.h)];
        for _ in 0..3 {
            for y in 0..self.h {
                box_line(&mut self.a[y * self.w..(y + 1) * self.w], 1, self.w, r, outside, &mut tmp);
            }
            for x in 0..self.w {
                box_line(&mut self.a[x..], self.w, self.h, r, outside, &mut tmp);
            }
        }
    }

    /// Färgad pixmap (förmultiplicerad) med täckningen × färgens alfa.
    fn colorize(&self, c: Rgba, gain: f32) -> Option<sk::Pixmap> {
        let mut pm = sk::Pixmap::new(self.w as u32, self.h as u32)?;
        let data = pm.data_mut();
        for (i, &v) in self.a.iter().enumerate() {
            let a = ((v * gain).clamp(0.0, 1.0) * c.a.clamp(0.0, 1.0)).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            data[i * 4] = (c.r.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
            data[i * 4 + 1] = (c.g.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
            data[i * 4 + 2] = (c.b.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
            data[i * 4 + 3] = (a * 255.0 + 0.5) as u8;
        }
        Some(pm)
    }
}

/// Lådfilter längs en rad/kolumn (`stride` mellan elementen), med `outside` utanför.
fn box_line(buf: &mut [f32], stride: usize, n: usize, r: usize, outside: f32, tmp: &mut [f32]) {
    let k = (2 * r + 1) as f32;
    let get = |buf: &[f32], i: isize| if i < 0 || i >= n as isize { outside } else { buf[i as usize * stride] };
    let mut acc = 0.0f32;
    for i in -(r as isize)..=(r as isize) {
        acc += get(buf, i);
    }
    for i in 0..n {
        tmp[i] = acc / k;
        acc += get(buf, i as isize + r as isize + 1) - get(buf, i as isize - r as isize);
    }
    for i in 0..n {
        buf[i * stride] = tmp[i];
    }
}

/// Hur långt effekterna når utanför innehållet (bildpunkter).
fn reach(effects: &[Effect], scale: f32) -> f32 {
    effects
        .iter()
        .map(|e| match e {
            Effect::OuterShadow { dx, dy, blur, .. } => (dx.abs().max(dy.abs()) + blur * 1.5) * scale + 2.0,
            Effect::Glow { radius, .. } => radius * 1.5 * scale + 2.0,
            Effect::Contour { width, .. } => width * scale + 2.0,
            Effect::Camera { extrude, .. } => extrude.0.abs().max(extrude.1.abs()) * scale + 2.0,
            _ => 0.0,
        })
        .fold(0.0f32, f32::max)
}

/// Innehållet ritat i ett eget lager: (lager, x0, y0) i bildpunkter.
fn layer(pm_w: u32, pm_h: u32, content: &[Item], effects: &[Effect], page: &Affine) -> Option<(sk::Pixmap, i32, i32)> {
    let scale = page.scale_factor();
    let (bx0, by0, bx1, by1) = bounds(content, page)?;
    let margin = reach(effects, scale) + 2.0;
    let x0 = (bx0 - 2.0).floor().max(-margin).max(-(pm_w as f32)) as i32;
    let y0 = (by0 - 2.0).floor().max(-margin).max(-(pm_h as f32)) as i32;
    let x1 = (bx1 + 2.0).ceil().min(pm_w as f32 + margin).min(2.0 * pm_w as f32) as i32;
    let y1 = (by1 + 2.0).ceil().min(pm_h as f32 + margin).min(2.0 * pm_h as f32) as i32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut l = sk::Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32)?;
    let lp = page.then(&Affine::translate(-x0 as f32, -y0 as f32));
    super::raster::draw_items(&mut l, content, &lp);
    // Tunn text: skrapa bort en rand innanför glyfernas kontur.
    for e in effects {
        if let Effect::Thin { width } = e {
            for it in content {
                if let Item::Glyphs(g) = it {
                    let path = super::raster::glyph_path(g);
                    let Some(p) = super::raster::to_sk_path(&path) else { continue };
                    let paint = sk::Paint { shader: sk::Shader::SolidColor(sk::Color::BLACK), anti_alias: true, blend_mode: sk::BlendMode::DestinationOut, ..Default::default() };
                    let st = sk::Stroke { width: width * 2.0, line_join: sk::LineJoin::Round, ..Default::default() };
                    let t = g.t.then(&lp);
                    l.stroke_path(&p, &paint, &st, sk::Transform::from_row(t.a, t.b, t.c, t.d, t.e, t.f), None);
                }
            }
        }
    }
    Some((l, x0, y0))
}

fn sk_t(a: &Affine) -> sk::Transform {
    sk::Transform::from_row(a.a, a.b, a.c, a.d, a.e, a.f)
}

/// Effekterna som ligger under innehållet (spegling, skugga, glöd) ritade på `pm`.
fn draw_under(pm: &mut sk::Pixmap, l: &sk::Pixmap, x0: i32, y0: i32, effects: &[Effect], page: &Affine, mask: Option<&sk::Mask>) {
    let scale = page.scale_factor();
    let pinv = page.invert().unwrap_or(Affine::IDENTITY);
    for e in effects {
        match *e {
            Effect::Reflection { blur, dist, start_alpha, end_alpha, start_pos, end_pos } => {
                let (lw, lh) = (l.width() as usize, l.height() as usize);
                let Some(mut r) = sk::Pixmap::new(lw as u32, lh as u32) else { continue };
                let src = l.data();
                let dst = r.data_mut();
                let span = (end_pos - start_pos).max(1e-3);
                for y in 0..lh {
                    // Avstånd från spegelaxeln (lagrets nederkant) som andel av höjden.
                    let f = (lh - y) as f32 / lh.max(1) as f32;
                    let k = if f > end_pos { 0.0 } else { start_alpha + (end_alpha - start_alpha) * ((f - start_pos) / span).clamp(0.0, 1.0) };
                    if k <= 0.0 {
                        continue;
                    }
                    for x in 0..lw {
                        let i = (y * lw + x) * 4;
                        for c in 0..4 {
                            dst[i + c] = (src[i + c] as f32 * k + 0.5) as u8;
                        }
                    }
                }
                let _ = blur;
                let t = Affine::scale(1.0, -1.0).then(&Affine::translate(x0 as f32, (y0 as f32) + 2.0 * lh as f32 + dist * scale - 4.0));
                pm.draw_pixmap(0, 0, r.as_ref(), &sk::PixmapPaint { quality: sk::FilterQuality::Bilinear, ..Default::default() }, sk_t(&t), mask);
            }
            Effect::OuterShadow { dx, dy, blur, color, t } => {
                let m = (blur * scale * 1.5).ceil() as usize + 2;
                let mut a = Alpha::from_layer(l, m, 0.0);
                a.blur(blur * scale / 2.0, 0.0);
                let Some(s) = a.colorize(color, 1.0) else { continue };
                // Lagret → bild (punkter) → skuggans skalning → förskjutning → bildpunkter.
                let to_dev = Affine::translate(x0 as f32 - m as f32, y0 as f32 - m as f32).then(&pinv).then(&t).then(&Affine::translate(dx, dy)).then(page);
                pm.draw_pixmap(0, 0, s.as_ref(), &sk::PixmapPaint { quality: sk::FilterQuality::Bilinear, ..Default::default() }, sk_t(&to_dev), mask);
            }
            Effect::Glow { radius, color } => {
                let m = (radius * scale * 1.5).ceil() as usize + 2;
                let mut a = Alpha::from_layer(l, m, 0.0);
                a.blur(radius * scale / 2.0, 0.0);
                let Some(s) = a.colorize(color, 2.0) else { continue };
                pm.draw_pixmap(x0 - m as i32, y0 - m as i32, s.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), mask);
            }
            Effect::Contour { width, color } => {
                let m = (width * scale).ceil() as i32 + 2;
                if let Some(c) = super::bevel::contour(l, m as usize, width * scale, color) {
                    pm.draw_pixmap(x0 - m, y0 - m, c.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), mask);
                }
            }
            Effect::Camera { extrude, color, .. } => {
                // Extruderingen: lagrets form i extruderingens färg, förskjuten bakåt steg för steg.
                let (ex, ey) = (extrude.0 * scale, extrude.1 * scale);
                let steps = ex.abs().max(ey.abs()).ceil().min(400.0) as usize;
                if steps > 0 {
                    let a = Alpha::from_layer(l, 0, 0.0);
                    if let Some(sh) = a.colorize(color, 1.0) {
                        for k in (1..=steps).rev() {
                            let f = k as f32 / steps as f32;
                            pm.draw_pixmap(0, 0, sh.as_ref(), &sk::PixmapPaint { quality: sk::FilterQuality::Bilinear, ..Default::default() }, sk::Transform::from_translate(x0 as f32 + ex * f, y0 as f32 + ey * f), mask);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Ändrar lagret: inre skugga och mjuka kanter.
fn modify_layer(l: &mut sk::Pixmap, effects: &[Effect], scale: f32) {
    for e in effects {
        match *e {
            Effect::SoftEdge { radius } => {
                let mut a = Alpha::from_layer(l, 0, 0.0);
                a.blur(radius * scale / 2.0, 0.0);
                let data = l.data_mut();
                for (i, &v) in a.a.iter().enumerate() {
                    let k = ((v - 0.5) * 2.0).clamp(0.0, 1.0);
                    for c in 0..4 {
                        data[i * 4 + c] = (data[i * 4 + c] as f32 * k + 0.5) as u8;
                    }
                }
            }
            Effect::InnerShadow { dx, dy, blur, color } => {
                let (lw, lh) = (l.width() as usize, l.height() as usize);
                let m = (blur * scale * 1.5 + dx.abs().max(dy.abs()) * scale).ceil() as usize + 2;
                let base = Alpha::from_layer(l, m, 0.0);
                // Skuggan av det som ligger utanför formen, förskjuten.
                let (sx, sy) = ((dx * scale).round() as isize, (dy * scale).round() as isize);
                let mut inv = Alpha { w: base.w, h: base.h, a: vec![1.0; base.w * base.h] };
                for y in 0..base.h as isize {
                    for x in 0..base.w as isize {
                        let (ux, uy) = (x - sx, y - sy);
                        let v = if ux < 0 || uy < 0 || ux >= base.w as isize || uy >= base.h as isize { 0.0 } else { base.a[uy as usize * base.w + ux as usize] };
                        inv.a[y as usize * base.w + x as usize] = 1.0 - v;
                    }
                }
                inv.blur(blur * scale / 2.0, 1.0);
                let data = l.data_mut();
                let ca = color.a.clamp(0.0, 1.0);
                for y in 0..lh {
                    for x in 0..lw {
                        let i = (y * lw + x) * 4;
                        let own = data[i + 3] as f32 / 255.0;
                        if own <= 0.0 {
                            continue;
                        }
                        let s = inv.a[(y + m) * inv.w + x + m] * ca * own;
                        // Källa över: skuggan (förmultiplicerad) över lagret.
                        let col = [color.r, color.g, color.b];
                        for c in 0..3 {
                            let d = data[i + c] as f32 / 255.0;
                            data[i + c] = ((col[c].clamp(0.0, 1.0) * s + d * (1.0 - s)) * 255.0 + 0.5) as u8;
                        }
                    }
                }
            }
            Effect::Bevel { width, height, kind, lighting } => super::bevel::shade_layer(l, width * scale, height * scale, kind, &lighting),
            _ => {}
        }
    }
}

/// Transformen för lagret: sidans, med en 3D-kameras förvrängning först.
fn camera_page(effects: &[Effect], page: &Affine) -> Affine {
    match effects.iter().find_map(|e| if let Effect::Camera { t, .. } = e { Some(*t) } else { None }) {
        Some(t) => t.then(page),
        None => *page,
    }
}

/// Ritar `content` med `effects` på `pm` (anropas av `raster::draw`).
pub(super) fn draw(pm: &mut sk::Pixmap, content: &[Item], effects: &[Effect], page: &Affine, mask: Option<&sk::Mask>) {
    let page = &camera_page(effects, page);
    let Some((mut l, x0, y0)) = layer(pm.width(), pm.height(), content, effects, page) else { return };
    draw_under(pm, &l, x0, y0, effects, page, mask);
    modify_layer(&mut l, effects, page.scale_factor());
    pm.draw_pixmap(x0, y0, l.as_ref(), &sk::PixmapPaint::default(), sk::Transform::identity(), mask);
}

/// Effekterna under innehållet som en egen bild (för PDF:en): (pixmap, transform pixmap → bild i punkter).
/// `ppt` är upplösningen i bildpunkter per punkt.
pub fn underlay(content: &[Item], effects: &[Effect], slide_w: f32, slide_h: f32, ppt: f32) -> Option<(sk::Pixmap, Affine)> {
    if !effects.iter().any(|e| matches!(e, Effect::OuterShadow { .. } | Effect::Glow { .. } | Effect::Reflection { .. } | Effect::Contour { .. })) {
        return None;
    }
    let page = Affine::scale(ppt, ppt);
    let (pw, ph) = ((slide_w * ppt).ceil().max(1.0) as u32, (slide_h * ppt).ceil().max(1.0) as u32);
    let (l, x0, y0) = layer(pw, ph, content, effects, &page)?;
    // Ett lager som täcker effekternas räckvidd.
    let margin = reach(effects, ppt).ceil() as i32 + 2;
    let (ox, oy) = (x0 - margin, y0 - margin);
    let mut out = sk::Pixmap::new(l.width() + 2 * margin as u32, l.height() + 2 * margin as u32 + if effects.iter().any(|e| matches!(e, Effect::Reflection { .. })) { l.height() } else { 0 })?;
    let shift = page.then(&Affine::translate(-ox as f32, -oy as f32));
    draw_under(&mut out, &l, x0 - ox, y0 - oy, effects, &shift, None);
    let t = Affine::translate(ox as f32, oy as f32).then(&Affine::scale(1.0 / ppt, 1.0 / ppt));
    Some((out, t))
}

/// Fasningen ovanpå innehållet som en egen bild (för PDF:en): bara kantzonens bildpunkter, resten
/// genomskinligt, så att det inre av formen förblir vektorer. (pixmap, transform pixmap → bild i punkter).
pub fn overlay(content: &[Item], effects: &[Effect], slide_w: f32, slide_h: f32, ppt: f32) -> Option<(sk::Pixmap, Affine)> {
    let Some(&Effect::Bevel { width, height, kind, lighting }) = effects.iter().find(|e| matches!(e, Effect::Bevel { .. })) else { return None };
    let page = Affine::scale(ppt, ppt);
    let (pw, ph) = ((slide_w * ppt).ceil().max(1.0) as u32, (slide_h * ppt).ceil().max(1.0) as u32);
    let (mut l, x0, y0) = layer(pw, ph, content, effects, &page)?;
    super::bevel::shade_layer_zone_only(&mut l, width * ppt, height * ppt, kind, &lighting);
    let t = Affine::translate(x0 as f32, y0 as f32).then(&Affine::scale(1.0 / ppt, 1.0 / ppt));
    Some((l, t))
}

/// Innehållet med alla effekter som en rasterbild (för PDF:en när en 3D-kamera förvränger formen):
/// (pixmap, transform pixmap → bild i punkter).
pub fn rasterized(content: &[Item], effects: &[Effect], slide_w: f32, slide_h: f32, ppt: f32) -> Option<(sk::Pixmap, Affine)> {
    if !effects.iter().any(|e| matches!(e, Effect::Camera { .. })) {
        return None;
    }
    let page = Affine::scale(ppt, ppt);
    let cp = camera_page(effects, &page);
    let (bx0, by0, bx1, by1) = bounds(content, &cp)?;
    let m = reach(effects, ppt).ceil() + 4.0;
    let (x0, y0) = ((bx0 - m).floor().max(-(slide_w * ppt)), (by0 - m).floor().max(-(slide_h * ppt)));
    let (x1, y1) = ((bx1 + m).ceil().min(2.0 * slide_w * ppt), (by1 + m).ceil().min(2.0 * slide_h * ppt));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut out = sk::Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32)?;
    draw(&mut out, content, effects, &page.then(&Affine::translate(-x0, -y0)), None);
    let t = Affine::translate(x0, y0).then(&Affine::scale(1.0 / ppt, 1.0 / ppt));
    Some((out, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::scene::{Paint, Path, Scene};

    fn scene_with(effects: Vec<Effect>) -> Scene {
        let mut s = Scene::new(100.0, 100.0);
        s.fill(Path::rect(0.0, 0.0, 100.0, 100.0), Paint::Solid(Rgba::WHITE));
        s.items.push(Item::BeginEffect(effects));
        s.fill(Path::rect(30.0, 30.0, 40.0, 40.0), Paint::Solid(Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }));
        s.items.push(Item::EndEffect);
        s
    }

    fn px(pm: &sk::Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = pm.pixel(x, y).unwrap().demultiply();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    #[test]
    fn skugga_hamnar_forskjuten_och_oskarp() {
        let black = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.5 };
        let s = scene_with(vec![Effect::OuterShadow { dx: 6.0, dy: 6.0, blur: 4.0, color: black, t: Affine::IDENTITY }]);
        let pm = super::super::raster::render(&s, 100, 100).unwrap();
        // Formen själv är röd.
        assert_eq!(px(&pm, 50, 50)[..3], [255, 0, 0]);
        // Skuggan syns nedanför/till höger, inte ovanför/till vänster.
        let below = px(&pm, 74, 74);
        assert!(below[0] < 200 && below[0] > 60, "skugga {below:?}");
        assert_eq!(px(&pm, 26, 26)[..3], [255, 255, 255]);
        // Oskarp: tonar ut mot kanten.
        let far = px(&pm, 78, 50);
        assert!(far[0] > below[0], "{far:?} {below:?}");
    }

    #[test]
    fn glod_mjuka_kanter_och_inre_skugga() {
        let s = scene_with(vec![Effect::Glow { radius: 6.0, color: Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 } }]);
        let pm = super::super::raster::render(&s, 100, 100).unwrap();
        let p = px(&pm, 50, 27);
        assert!(p[2] > p[0], "blå gloria ovanför formen: {p:?}");
        let s = scene_with(vec![Effect::SoftEdge { radius: 8.0 }]);
        let pm = super::super::raster::render(&s, 100, 100).unwrap();
        assert!(px(&pm, 31, 50)[1] > 150, "kanten tonar ut mot vitt");
        assert_eq!(px(&pm, 50, 50)[..3], [255, 0, 0]);
        let s = scene_with(vec![Effect::InnerShadow { dx: 3.0, dy: 3.0, blur: 4.0, color: Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 } }]);
        let pm = super::super::raster::render(&s, 100, 100).unwrap();
        assert!(px(&pm, 32, 32)[0] < 200, "mörkt innanför övre vänstra kanten");
        assert!(px(&pm, 67, 67)[0] > 240, "ljust vid nedre högra");
        assert_eq!(px(&pm, 20, 20)[..3], [255, 255, 255], "inget utanför formen");
    }

    fn corpus_deck() -> crate::pptx::Deck {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus/pptx/pythonpptx-shp-autoshape-props.pptx")).unwrap();
        crate::pptx::Deck::open(&bytes).unwrap()
    }

    #[test]
    fn effectref_ger_temats_skugga() {
        // Korpusfilen (python-pptx, MIT): formerna har `effectRef idx="2"`, temat en yttre skugga där.
        let deck = corpus_deck();
        let scene = crate::pptx::render::slide_scene(&deck, 0);
        let shadow = scene.items.iter().find_map(|it| match it {
            Item::BeginEffect(e) => e.iter().find_map(|x| if let Effect::OuterShadow { dy, blur, color, .. } = x { Some((*dy, *blur, color.a)) } else { None }),
            _ => None,
        });
        let (dy, blur, a) = shadow.expect("skugga ur effectStyleLst");
        // dist 23000 EMU nedåt, blurRad 40000 EMU, alfa 35 %.
        assert!((dy - 23000.0 / 12700.0).abs() < 0.01 && (blur - 40000.0 / 12700.0).abs() < 0.01 && (a - 0.35).abs() < 0.01, "{dy} {blur} {a}");
        // Markörerna kommer i par.
        let begins = scene.items.iter().filter(|i| matches!(i, Item::BeginEffect(_))).count();
        let ends = scene.items.iter().filter(|i| matches!(i, Item::EndEffect)).count();
        assert_eq!(begins, ends);
        // PDF:en klarar effekterna.
        assert!(crate::pptx::pdf::write(&deck, &[0]).is_ok());
    }

    #[test]
    fn effektlistans_tolkning() {
        let deck = corpus_deck();
        let sheets = Sheets::for_slide(&deck, 0);
        let xml = br#"<a:effectLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:glow rad="63500"><a:schemeClr val="accent1"><a:alpha val="40000"/></a:schemeClr></a:glow><a:innerShdw blurRad="63500" dist="50800" dir="13500000"><a:prstClr val="black"/></a:innerShdw><a:outerShdw dist="12700" dir="0" sx="50000" sy="50000" algn="tl"><a:srgbClr val="FF0000"/></a:outerShdw><a:softEdge rad="12700"/><a:reflection stA="50000" endPos="35000" dist="12700" dir="5400000" sy="-100000"/></a:effectLst>"#;
        let n = crate::pptx::xml::parse(xml).unwrap();
        let e = effect_list(&sheets, &n, None, (&Affine::translate(100.0, 100.0), 40.0, 20.0));
        assert_eq!(e.len(), 5);
        assert!(matches!(e[0], Effect::Glow { radius, color } if (radius - 5.0).abs() < 1e-3 && (color.a - 0.4).abs() < 1e-3));
        // dir 225°: uppåt till vänster.
        assert!(matches!(e[1], Effect::InnerShadow { dx, dy, .. } if dx < 0.0 && dy < 0.0));
        // Halva storleken runt övre vänstra hörnet (100, 100): (140, 120) → (120, 110).
        let Effect::OuterShadow { t, dx, .. } = &e[2] else { panic!() };
        assert!((dx - 1.0).abs() < 1e-3);
        let (x, y) = t.apply(140.0, 120.0);
        assert!((x - 120.0).abs() < 1e-3 && (y - 110.0).abs() < 1e-3, "{x} {y}");
        assert_eq!(e[3], Effect::SoftEdge { radius: 1.0 });
        assert!(matches!(e[4], Effect::Reflection { start_alpha, end_pos, .. } if (start_alpha - 0.5).abs() < 1e-3 && (end_pos - 0.35).abs() < 1e-3));
    }

    #[test]
    fn spegling_under_formen() {
        let s = scene_with(vec![Effect::Reflection { blur: 0.0, dist: 2.0, start_alpha: 0.5, end_alpha: 0.0, start_pos: 0.0, end_pos: 0.5 }]);
        let pm = super::super::raster::render(&s, 100, 100).unwrap();
        let p = px(&pm, 50, 74);
        assert!(p[1] < 250 && p[0] == 255, "rosa spegling: {p:?}");
        assert_eq!(px(&pm, 50, 98)[..3], [255, 255, 255]);
    }
}
