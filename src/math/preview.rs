//! Formler i Markdown-förhandsvisningen: varje `$…$`/`$$…$$` byts mot en bild (PNG i minnet)
//! innan texten går till egui_commonmark, som inte kan matematik.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};

use egui::{Color32, Context};

use super::{formula, raster, spans};

static REGISTERED: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// PNG med formeln i `color`. `width`: bredd i px för en formel på egen rad (centreras), annars `None`.
pub fn png(src: &str, display: bool, em: f32, color: Color32, width: Option<f32>) -> Option<Vec<u8>> {
    let f = formula(src, display)?;
    let pad = 2.0;
    let bmp = raster::rasterize(&f.layout, em, pad);
    let extra_bottom = (0.2 * em).ceil() as usize;
    let top = if display { (0.3 * em).ceil() as usize } else { 0 };
    let w = width.map_or(bmp.w, |w| (w as usize).max(bmp.w));
    let h = bmp.h + extra_bottom + top * 2;
    let x0 = (w - bmp.w) / 2;
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..bmp.h {
        for x in 0..bmp.w {
            let a = bmp.alpha[y * bmp.w + x];
            let o = ((y + top) * w + x + x0) * 4;
            rgba[o..o + 4].copy_from_slice(&[color.r(), color.g(), color.b(), a]);
        }
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Svart PNG med formeln utan marginal (`ppe` px per em): ersättningsbild för ODF-formelobjekt.
pub fn png_exact(src: &str, display: bool, ppe: f32) -> Option<Vec<u8>> {
    let f = formula(src, display)?;
    let bmp = raster::rasterize(&f.layout, ppe, 0.0);
    let mut rgba = Vec::with_capacity(bmp.alpha.len() * 4);
    for a in &bmp.alpha {
        rgba.extend_from_slice(&[0, 0, 0, *a]);
    }
    let img = image::RgbaImage::from_raw(bmp.w as u32, bmp.h as u32, rgba)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Ersätter formlerna i `md` med bildlänkar och registrerar bilderna hos `ctx`.
pub fn replace(md: &str, ctx: &Context, em: f32, color: Color32, max_w: f32) -> String {
    let sp = spans(md);
    if sp.is_empty() {
        return md.to_owned();
    }
    let mut out = md.to_owned();
    for s in sp.iter().rev() {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&s.src, s.display, em.to_bits(), color, max_w.to_bits()).hash(&mut h);
        let id = h.finish();
        let uri = format!("bytes://kladd-math/{id:016x}.png");
        let fresh = REGISTERED.lock().is_ok_and(|mut r| r.insert(id));
        if fresh {
            match png(&s.src, s.display, em, color, s.display.then_some(max_w)) {
                Some(bytes) => ctx.include_bytes(uri.clone(), bytes),
                None => {
                    if let Ok(mut r) = REGISTERED.lock() {
                        r.remove(&id);
                    }
                    continue; // formeln går inte att tolka: källtexten står kvar
                }
            }
        }
        let rep = if s.display { format!("\n\n![formel]({uri})\n\n") } else { format!("![formel]({uri})") };
        out.replace_range(s.range.clone(), &rep);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_is_a_valid_image_with_the_requested_width() {
        let b = png(r"\frac{a}{b}", true, 20.0, Color32::BLACK, Some(400.0)).unwrap();
        assert_eq!(&b[1..4], b"PNG");
        let img = image::load_from_memory(&b).unwrap();
        assert_eq!(img.width(), 400);
        let inline = image::load_from_memory(&png("x^2", false, 20.0, Color32::BLACK, None).unwrap()).unwrap();
        assert!(inline.width() < 60);
        assert!(png(r"\bogus", false, 20.0, Color32::BLACK, None).is_none());
    }

    #[test]
    fn replaces_math_with_image_links_and_keeps_broken_math() {
        let ctx = Context::default();
        let md = "A $x^2$ b\n\n$$\\frac12$$\n\nc $\\bogus$ d";
        let r = replace(md, &ctx, 16.0, Color32::BLACK, 500.0);
        assert_eq!(r.matches("![formel](bytes://kladd-math/").count(), 2);
        assert!(r.contains("$\\bogus$"));
        assert!(!r.contains("x^2"));
    }
}
