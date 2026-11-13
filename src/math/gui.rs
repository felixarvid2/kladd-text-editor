//! Formler som egui-texturer: en alfamask per (formel, storlek) som tonas med textfärgen.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use egui::{Color32, ColorImage, Context, Pos2, Rect, TextureHandle, TextureId, TextureOptions, Vec2, pos2, vec2};

use super::{formula, raster};

/// Marginal runt formeln i bitmappen (pixlar), så att kantutjämning inte klipps.
const PAD: f32 = 2.0;

/// Mått i punkter för en formel i brödtext med `em` punkter: (bredd, höjd över baslinjen, djup under).
pub fn metrics(src: &str, display: bool, em: f32) -> Option<(f32, f32, f32)> {
    let f = formula(src, display)?;
    Some((f.layout.w * em, f.layout.asc * em, f.layout.desc * em))
}

struct Entry {
    tex: TextureHandle,
    /// Bitmappens storlek (punkter) och baslinjens avstånd från överkanten.
    size: Vec2,
    baseline: f32,
}

static TEXTURES: LazyLock<Mutex<HashMap<(String, bool, u32), Entry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub struct Tex {
    pub id: TextureId,
    pub size: Vec2,
    pub baseline: f32,
    /// Marginalen runt formeln (punkter).
    pub pad: f32,
}

/// Texturen för formeln vid `em` punkter per em (skärmens `pixels_per_point` räknas in).
pub fn texture(ctx: &Context, src: &str, display: bool, em: f32) -> Option<Tex> {
    let ppp = ctx.pixels_per_point();
    let ppe = (em * ppp * 4.0).round() / 4.0;
    if ppe < 1.0 {
        return None;
    }
    let key = (src.to_owned(), display, ppe.to_bits());
    let mut map = TEXTURES.lock().ok()?;
    if let Some(e) = map.get(&key) {
        return Some(Tex { id: e.tex.id(), size: e.size, baseline: e.baseline, pad: PAD / ppp });
    }
    let f = formula(src, display)?;
    let bmp = raster::rasterize(&f.layout, ppe, PAD);
    let pixels: Vec<Color32> = bmp.alpha.iter().map(|a| Color32::from_rgba_premultiplied(*a, *a, *a, *a)).collect();
    let img = ColorImage { size: [bmp.w, bmp.h], source_size: vec2(bmp.w as f32, bmp.h as f32), pixels };
    let tex = ctx.load_texture(format!("kladd-math-{}", map.len()), img, TextureOptions::LINEAR);
    if map.len() > 400 {
        map.clear();
    }
    let e = Entry { tex, size: vec2(bmp.w as f32 / ppp, bmp.h as f32 / ppp), baseline: bmp.baseline / ppp };
    let out = Tex { id: e.tex.id(), size: e.size, baseline: e.baseline, pad: PAD / ppp };
    map.insert(key, e);
    Some(out)
}

/// Ritar formeln med baslinjen vid `(x, baseline_y)` och vänsterkant `x`.
pub fn paint(ctx: &Context, painter: &egui::Painter, src: &str, display: bool, em: f32, at: Pos2, color: Color32) -> Option<Rect> {
    let t = texture(ctx, src, display, em)?;
    let min = pos2(at.x - t.pad, at.y - t.baseline);
    let rect = Rect::from_min_size(min, t.size);
    painter.image(t.id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
    Some(rect)
}
