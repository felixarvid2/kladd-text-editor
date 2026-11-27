//! Fyllningar som byggs som bilder: mönster (`a:pattFill`, de 48 förinställda mönstren som 8×8-rutor),
//! upprepad bildfyllning (`a:tile`) och rektangulära banövertoningar (`a:path path="rect"/"shape"`).
//!
//! Mönsterrutorna är Kladds egna 8×8-bitmappar efter mönstrens namn och utseende i PowerPoint
//! (punkttätheten för `pctNN` görs med en Bayer-matris). En ruta är 8 bildpunkter vid 96 dpi = 6 pt,
//! som i PowerPoint vid 100 %.

use std::sync::Arc;

use super::color::Rgba;
use super::scene::{Affine, ImageData, Paint, Stop};
use super::xml::Node;

/// Rutans storlek i punkter.
pub const PATTERN_CELL: f32 = 6.0;

/// 8×8 Bayer-matris (tröskel 0..63) för jämnt utspridda punkter.
const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Mönstrets rader (bit 7 = vänstra bildpunkten, 1 = förgrund).
pub fn pattern_bits(prst: &str) -> [u8; 8] {
    if let Some(p) = prst.strip_prefix("pct").and_then(|p| p.parse::<u32>().ok()) {
        let n = (p * 64 + 50) / 100;
        let mut rows = [0u8; 8];
        for (y, row) in rows.iter_mut().enumerate() {
            for x in 0..8 {
                if (BAYER[y][x] as u32) < n {
                    *row |= 0x80 >> x;
                }
            }
        }
        return rows;
    }
    let rev = |r: [u8; 8]| r.map(|b| b.reverse_bits());
    match prst {
        "horz" => [0xFF, 0, 0, 0, 0, 0, 0, 0],
        "vert" => [0x80; 8],
        "ltHorz" => [0xFF, 0, 0, 0, 0xFF, 0, 0, 0],
        "ltVert" => [0x88; 8],
        "dkHorz" => [0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0],
        "dkVert" => [0xCC; 8],
        "narHorz" => [0xFF, 0, 0xFF, 0, 0xFF, 0, 0xFF, 0],
        "narVert" => [0xAA; 8],
        "dashHorz" => [0xF0, 0, 0, 0, 0x0F, 0, 0, 0],
        "dashVert" => [0x80, 0x80, 0x80, 0x80, 0x08, 0x08, 0x08, 0x08],
        "cross" | "lgGrid" => [0xFF, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80],
        "smGrid" => [0xFF, 0x88, 0x88, 0x88, 0xFF, 0x88, 0x88, 0x88],
        "dnDiag" => [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01],
        "upDiag" => rev([0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01]),
        "ltDnDiag" => [0x88, 0x44, 0x22, 0x11, 0x88, 0x44, 0x22, 0x11],
        "ltUpDiag" => rev([0x88, 0x44, 0x22, 0x11, 0x88, 0x44, 0x22, 0x11]),
        "dkDnDiag" => [0xCC, 0x66, 0x33, 0x99, 0xCC, 0x66, 0x33, 0x99],
        "dkUpDiag" => rev([0xCC, 0x66, 0x33, 0x99, 0xCC, 0x66, 0x33, 0x99]),
        "wdDnDiag" => [0xC1, 0xE0, 0x70, 0x38, 0x1C, 0x0E, 0x07, 0x83],
        "wdUpDiag" => rev([0xC1, 0xE0, 0x70, 0x38, 0x1C, 0x0E, 0x07, 0x83]),
        "dashDnDiag" => [0x88, 0x44, 0x22, 0x11, 0, 0, 0, 0],
        "dashUpDiag" => rev([0x88, 0x44, 0x22, 0x11, 0, 0, 0, 0]),
        "diagCross" => [0x81, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0x81],
        "smCheck" => [0xCC, 0xCC, 0x33, 0x33, 0xCC, 0xCC, 0x33, 0x33],
        "lgCheck" => [0xF0, 0xF0, 0xF0, 0xF0, 0x0F, 0x0F, 0x0F, 0x0F],
        "dotGrid" => [0xAA, 0, 0x80, 0, 0x80, 0, 0x80, 0],
        "smConfetti" => [0x80, 0x10, 0x02, 0x20, 0x01, 0x08, 0x40, 0x04],
        "lgConfetti" => [0xB1, 0x30, 0x03, 0x1B, 0xD8, 0xC0, 0x0C, 0x8D],
        "horzBrick" => [0xFF, 0x80, 0x80, 0x80, 0xFF, 0x08, 0x08, 0x08],
        "diagBrick" => [0x80, 0x40, 0x20, 0x10, 0x18, 0x24, 0x42, 0x81],
        "solidDmnd" => [0x10, 0x38, 0x7C, 0xFE, 0x7C, 0x38, 0x10, 0],
        "openDmnd" => [0x80, 0x41, 0x22, 0x14, 0x08, 0x14, 0x22, 0x41],
        "dotDmnd" => [0x80, 0, 0x22, 0, 0x08, 0, 0x22, 0],
        "plaid" => [0xAA, 0x55, 0xAA, 0x55, 0xF0, 0xF0, 0xF0, 0xF0],
        "sphere" => [0x77, 0x89, 0x8F, 0x8F, 0x77, 0x98, 0xF8, 0xF8],
        "weave" => [0x88, 0x54, 0x22, 0x45, 0x88, 0x15, 0x22, 0x51],
        "divot" => [0, 0x08, 0x04, 0x08, 0, 0x80, 0x40, 0x80],
        "shingle" => [0x03, 0x84, 0x48, 0x30, 0x0C, 0x02, 0x01, 0x01],
        "wave" => [0, 0x18, 0xA4, 0x03, 0, 0x18, 0xA4, 0x03],
        "trellis" => [0xFF, 0x66, 0xFF, 0x99, 0xFF, 0x66, 0xFF, 0x99],
        "zigZag" => [0x81, 0x42, 0x24, 0x18, 0x81, 0x42, 0x24, 0x18],
        // Okänt: 50 %.
        _ => [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55],
    }
}

fn rgba8(c: Rgba) -> [u8; 4] {
    c.to_u8()
}

/// Mönsterfyllning som upprepad 8×8-bild. `origin` avbildar bildens koordinater (punkter) på bilden;
/// mönstret följer bilden (inte formens rotation), som i PowerPoint.
pub fn pattern_paint(prst: &str, fg: Rgba, bg: Rgba, origin: &Affine) -> Paint {
    let bits = pattern_bits(prst);
    let (f, b) = (rgba8(fg), rgba8(bg));
    let mut rgba = Vec::with_capacity(8 * 8 * 4);
    for row in bits {
        for x in 0..8 {
            rgba.extend_from_slice(if row & (0x80 >> x) != 0 { &f } else { &b });
        }
    }
    let img = Arc::new(ImageData::new(8, 8, rgba, None));
    let t = Affine::scale(PATTERN_CELL, PATTERN_CELL).then(origin);
    Paint::Image { image: img, t, tile: true, alpha: 1.0 }
}

/// Bilden speglad enligt `flip` (`x`, `y`, `xy`): en dubbelt så stor ruta med speglade kopior.
fn flipped(img: &ImageData, fx: bool, fy: bool) -> ImageData {
    let (w, h) = (img.width as usize, img.height as usize);
    let (nw, nh) = (if fx { 2 * w } else { w }, if fy { 2 * h } else { h });
    let mut rgba = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let sy = if y < h { y } else { 2 * h - 1 - y };
        for x in 0..nw {
            let sx = if x < w { x } else { 2 * w - 1 - x };
            let s = (sy * w + sx) * 4;
            let d = (y * nw + x) * 4;
            rgba[d..d + 4].copy_from_slice(&img.rgba[s..s + 4]);
        }
    }
    ImageData::new(nw as u32, nh as u32, rgba, None)
}

/// `a:tile` för en bildfyllning i en ruta `w`×`h` (formens lokala koordinater, `ft` → bilden).
/// Rutans storlek är bildens storlek vid 96 dpi gånger `sx`/`sy`; justeringen (`algn`) placerar
/// första rutan och `tx`/`ty` förskjuter den.
pub fn tile_paint(tile: &Node, img: Arc<ImageData>, w: f32, h: f32, ft: &Affine, alpha: f32) -> Paint {
    let pct = |k: &str| tile.attr(k).and_then(super::xml::parse_i64).map_or(1.0, |v| v as f32 / 100000.0);
    let emu = |k: &str| tile.attr_i64(k).map_or(0.0, super::render::emu);
    let (sx, sy) = (pct("sx"), pct("sy"));
    let flip = tile.attr("flip").unwrap_or("none");
    let (fx, fy) = (flip.contains('x'), flip.contains('y'));
    let (iw, ih) = (img.width as f32 * 0.75 * sx, img.height as f32 * 0.75 * sy);
    let img = if fx || fy { Arc::new(flipped(&img, fx, fy)) } else { img };
    let (tw, th) = (if fx { 2.0 * iw } else { iw }, if fy { 2.0 * ih } else { ih });
    let (ax, ay) = match tile.attr("algn").unwrap_or("tl") {
        "t" => ((w - iw) / 2.0, 0.0),
        "tr" => (w - iw, 0.0),
        "l" => (0.0, (h - ih) / 2.0),
        "ctr" => ((w - iw) / 2.0, (h - ih) / 2.0),
        "r" => (w - iw, (h - ih) / 2.0),
        "bl" => (0.0, h - ih),
        "b" => ((w - iw) / 2.0, h - ih),
        "br" => (w - iw, h - ih),
        _ => (0.0, 0.0),
    };
    let t = Affine::scale(tw.abs().max(0.01), th.abs().max(0.01)).then(&Affine::translate(ax + emu("tx"), ay + emu("ty"))).then(ft);
    Paint::Image { image: img, t, tile: true, alpha }
}

/// Färgen vid läget `p` (0..1) bland stoppen (sorterade).
fn at(stops: &[Stop], p: f32) -> Rgba {
    let first = &stops[0];
    if p <= first.pos {
        return first.color;
    }
    for w in stops.windows(2) {
        if p <= w[1].pos {
            let k = if w[1].pos > w[0].pos { (p - w[0].pos) / (w[1].pos - w[0].pos) } else { 1.0 };
            let (a, b) = (w[0].color, w[1].color);
            return Rgba { r: a.r + (b.r - a.r) * k, g: a.g + (b.g - a.g) * k, b: a.b + (b.b - a.b) * k, a: a.a + (b.a - a.a) * k };
        }
    }
    stops[stops.len() - 1].color
}

/// Rektangulär banövertoning (`path="rect"`, och `shape` ungefärligt som rektangel): läget 0 i
/// fokusrektangeln (`focus` = vänster, topp, höger, botten som andelar) och 1 vid rutans kant, med
/// rektangulära nivåkurvor. `stops` har läget räknat från fokus (0) utåt (1). Bilden täcker formens ruta.
pub fn rect_gradient(stops: &[Stop], focus: (f32, f32, f32, f32), w: f32, h: f32, ft: &Affine) -> Option<Paint> {
    if stops.is_empty() || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (l, t, r, b) = focus;
    let (fx0, fy0, fx1, fy1) = (l.clamp(0.0, 1.0), t.clamp(0.0, 1.0), (1.0 - r).clamp(0.0, 1.0), (1.0 - b).clamp(0.0, 1.0));
    let n = 128usize;
    let (pw, ph) = if w >= h { (n, ((n as f32 * h / w).round() as usize).clamp(2, n)) } else { (((n as f32 * w / h).round() as usize).clamp(2, n), n) };
    let mut rgba = Vec::with_capacity(pw * ph * 4);
    for y in 0..ph {
        let v = (y as f32 + 0.5) / ph as f32;
        let dy = if v < fy0 { (fy0 - v) / fy0.max(1e-4) } else if v > fy1 { (v - fy1) / (1.0 - fy1).max(1e-4) } else { 0.0 };
        for x in 0..pw {
            let u = (x as f32 + 0.5) / pw as f32;
            let dx = if u < fx0 { (fx0 - u) / fx0.max(1e-4) } else if u > fx1 { (u - fx1) / (1.0 - fx1).max(1e-4) } else { 0.0 };
            rgba.extend_from_slice(&at(stops, dx.max(dy).clamp(0.0, 1.0)).to_u8());
        }
    }
    let img = Arc::new(ImageData::new(pw as u32, ph as u32, rgba, None));
    Some(Paint::Image { image: img, t: Affine::scale(w, h).then(ft), tile: false, alpha: 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monstrens_tathet() {
        let count = |p: &str| pattern_bits(p).iter().map(|r| r.count_ones()).sum::<u32>();
        assert_eq!(count("pct50"), 32);
        assert_eq!(count("pct25"), 16);
        assert_eq!(count("pct5"), 3);
        assert_eq!(count("pct90"), 58);
        assert_eq!(count("horz"), 8);
        assert_eq!(count("vert"), 8);
        // Uppåtdiagonalen är nedåtdiagonalen speglad.
        assert_eq!(pattern_bits("upDiag")[0], 0x01);
        assert_ne!(pattern_bits("trellis"), pattern_bits("weave"));
    }

    #[test]
    fn monster_upprepas_i_rastret() {
        let mut s = super::super::scene::Scene::new(24.0, 24.0);
        let p = pattern_paint("vert", Rgba::BLACK, Rgba::WHITE, &Affine::IDENTITY);
        s.fill(super::super::scene::Path::rect(0.0, 0.0, 24.0, 24.0), p);
        // 4 bildpunkter per punkt: en ruta (6 pt) = 24 bildpunkter, linjen 3 bildpunkter bred.
        let pm = super::super::raster::render(&s, 96, 96).unwrap();
        let g = |x: u32, y: u32| pm.pixel(x, y).unwrap().red();
        assert!(g(1, 10) < 60, "linje i rutans vänsterkant");
        assert!(g(12, 10) > 200);
        assert!(g(25, 50) < 60, "nästa ruta");
    }

    #[test]
    fn rektangulär_övertoning() {
        let stops = vec![Stop { pos: 0.0, color: Rgba::BLACK }, Stop { pos: 1.0, color: Rgba::WHITE }];
        let Some(Paint::Image { image, .. }) = rect_gradient(&stops, (0.5, 0.5, 0.5, 0.5), 100.0, 100.0, &Affine::IDENTITY) else { panic!() };
        let px = |x: usize, y: usize| image.rgba[(y * image.width as usize + x) * 4];
        assert!(px(64, 64) < 10, "mörkt i fokus");
        assert!(px(0, 64) > 245 && px(64, 0) > 245, "ljust vid kanten");
        // Rektangulära nivåkurvor: samma värde längs en kvadrat runt mitten.
        assert!((px(32, 32) as i32 - px(32, 64) as i32).abs() < 4);
    }
}
