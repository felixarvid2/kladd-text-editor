//! EMF/WMF-bilder: spelas upp med `emf::render` (vektorer, text, bitmappar). Ritar uppspelningen
//! ingenting (t.ex. en fil med bara EMF+) men filen bär en bitmapp, ritas den största bitmappen;
//! annars en platshållare.

use std::sync::Arc;

use super::render::{Builder, Frame};
use super::scene::{Affine, Item};

/// Bitmappen i en EMF som bara består av en bild (största `EMR_STRETCHDIBITS`).
pub fn emf_bitmap(bytes: &[u8]) -> Option<super::scene::ImageData> {
    let u32le = |o: usize| -> Option<usize> { bytes.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) };
    if u32le(0)? != 1 || bytes.get(40..44)? != b" EMF" {
        return None;
    }
    let mut off = 0usize;
    let mut best: Option<(usize, Vec<u8>)> = None;
    let mut vector_records = 0usize;
    while off + 8 <= bytes.len() {
        let typ = u32le(off)?;
        let size = u32le(off + 4)?;
        if size < 8 || off + size > bytes.len() {
            break;
        }
        match typ {
            // EMR_STRETCHDIBITS
            81 => {
                let bmi_off = u32le(off + 48)?;
                let bmi_len = u32le(off + 52)?;
                let bits_off = u32le(off + 56)?;
                let bits_len = u32le(off + 60)?;
                let rec = bytes.get(off..off + size)?;
                if let (Some(bmi), Some(bits)) = (rec.get(bmi_off..bmi_off + bmi_len), rec.get(bits_off..bits_off + bits_len)) {
                    let mut bmp = Vec::with_capacity(14 + bmi.len() + bits.len());
                    bmp.extend_from_slice(b"BM");
                    bmp.extend_from_slice(&((14 + bmi.len() + bits.len()) as u32).to_le_bytes());
                    bmp.extend_from_slice(&[0, 0, 0, 0]);
                    bmp.extend_from_slice(&((14 + bmi.len()) as u32).to_le_bytes());
                    bmp.extend_from_slice(bmi);
                    bmp.extend_from_slice(bits);
                    if best.as_ref().is_none_or(|(l, _)| bits.len() > *l) {
                        best = Some((bits.len(), bmp));
                    }
                }
            }
            // Vanliga ritposter (polygoner, linjer, text): då är det inte bara en bitmapp.
            2..=8 | 54 | 55 | 56..=59 | 84 | 85..=92 | 96 | 97 => vector_records += 1,
            14 => break,
            _ => {}
        }
        off += size;
    }
    if vector_records > 2 {
        return None;
    }
    let (_, bmp) = best?;
    let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(super::scene::ImageData::new(w, h, img.into_raw(), None))
}

/// Ritar metafilen `part` i formen `f`; `crop` är `a:srcRect` (vänster, topp, höger, botten) som andelar,
/// negativa värden ger luft runt bilden.
pub fn metafile(b: &mut Builder<'_>, part: &str, f: &Frame, crop: [f32; 4]) {
    let deck = b.sheets.deck;
    let [l, t, r, bo] = crop;
    let (vw, vh) = ((1.0 - l - r).max(1e-3), (1.0 - t - bo).max(1e-3));
    // Hela bilden i formens koordinater; den synliga delen fyller formen.
    let (w, h) = (f.w / vw, f.h / vh);
    let t_full = Affine::translate(-l * w, -t * h).then(&f.t);
    if let Some(items) = deck.pkg.read(part).and_then(|bytes| super::emf::render(bytes, t_full, w, h, &deck.fonts)) {
        let clip = crop.iter().any(|&c| c > 0.0);
        if clip {
            b.scene.items.push(Item::PushClip { path: super::scene::Path::rect(0.0, 0.0, f.w, f.h).transformed(&f.t) });
        }
        b.scene.items.extend(items);
        if clip {
            b.scene.items.push(Item::PopClip);
        }
        return;
    }
    if let Some(img) = b.sheets.deck.pkg.read(part).and_then(emf_bitmap) {
        let t = Affine::scale(f.w, f.h).then(&f.t);
        b.scene.items.push(Item::Image { image: Arc::new(img), t, crop, alpha: 1.0 });
        return;
    }
    let label = if part.to_lowercase().ends_with(".wmf") { crate::i18n::t("Bild (WMF)") } else { crate::i18n::t("Bild (EMF)") };
    b.placeholder_box(f, label);
}
