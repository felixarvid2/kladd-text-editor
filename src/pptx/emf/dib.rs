//! Enhetsoberoende bitmappar (DIB) i EMF/WMF: `BITMAPINFO` + bildpunkter → RGBA.
//!
//! Bitmappen görs om till en BMP-fil i minnet och avkodas med `image` (1/4/8/16/24/32 bitar, RLE).
//! `BI_JPEG`/`BI_PNG` (komprimering 4/5) bär en hel JPEG/PNG-fil.

use super::super::scene::ImageData;

/// Avkodar en DIB. `bits` kan vara tom när bildpunkterna följer direkt efter huvudet i `bmi`.
pub fn decode(bmi: &[u8], bits: &[u8]) -> Option<ImageData> {
    if bmi.len() < 12 {
        return None;
    }
    let hdr = u32::from_le_bytes([bmi[0], bmi[1], bmi[2], bmi[3]]) as usize;
    if hdr >= 40 && bmi.len() >= 20 {
        let compression = u32::from_le_bytes([bmi[16], bmi[17], bmi[18], bmi[19]]);
        if compression == 4 || compression == 5 {
            let data = if bits.is_empty() { bmi.get(hdr.min(bmi.len())..)? } else { bits };
            let img = image::load_from_memory(data).ok()?.to_rgba8();
            let (w, h) = img.dimensions();
            return Some(ImageData::new(w, h, img.into_raw(), None));
        }
    }
    let total = 14 + bmi.len() + bits.len();
    let mut bmp = Vec::with_capacity(total);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(total as u32).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    // Bildpunkterna börjar efter huvudet och färgtabellen; utan separata bildpunkter räknas
    // tabellens storlek ut ur huvudet.
    let pixel_offset = if bits.is_empty() { 14 + header_and_palette_len(bmi)? } else { 14 + bmi.len() };
    bmp.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
    bmp.extend_from_slice(bmi);
    bmp.extend_from_slice(bits);
    let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    Some(ImageData::new(w, h, img.into_raw(), None))
}

/// Storleken på huvud + färgtabell i en packad DIB.
fn header_and_palette_len(bmi: &[u8]) -> Option<usize> {
    let u16le = |o: usize| bmi.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32le = |o: usize| bmi.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let hdr = u32le(0)?;
    if hdr == 12 {
        // BITMAPCOREHEADER: RGBTRIPLE-tabell.
        let bpp = u16le(10)?;
        let n = if bpp <= 8 { 1usize << bpp } else { 0 };
        return Some(12 + n * 3);
    }
    let bpp = u16le(14)?;
    let compression = u32le(16)?;
    let used = u32le(32).unwrap_or(0);
    let n = if used > 0 { used.min(256) } else if bpp <= 8 { 1usize << bpp } else { 0 };
    // BI_BITFIELDS med ett 40-byteshuvud: tre masker efter huvudet.
    let masks = if hdr == 40 && compression == 3 { 12 } else { 0 };
    Some(hdr + masks + n * 4)
}

/// Bitmappens medelfärg (för mönsterpenslar).
pub fn average(img: &ImageData) -> super::super::color::Rgba {
    let n = (img.width as usize * img.height as usize).max(1) as f32;
    let mut s = [0f32; 3];
    for p in img.rgba.chunks_exact(4) {
        s[0] += p[0] as f32;
        s[1] += p[1] as f32;
        s[2] += p[2] as f32;
    }
    super::super::color::Rgba { r: s[0] / n / 255.0, g: s[1] / n / 255.0, b: s[2] / n / 255.0, a: 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// En 2×1 bitmapp med 24 bitar (rad nerifrån och upp, utfylld till 4 byte).
    #[test]
    fn avkodar_24_bitar_och_packad_dib() {
        let mut bmi = Vec::new();
        bmi.extend_from_slice(&40u32.to_le_bytes());
        bmi.extend_from_slice(&2i32.to_le_bytes());
        bmi.extend_from_slice(&1i32.to_le_bytes());
        bmi.extend_from_slice(&1u16.to_le_bytes());
        bmi.extend_from_slice(&24u16.to_le_bytes());
        bmi.extend_from_slice(&[0u8; 24]);
        // Blå, röd (BGR) + 2 byte utfyllnad.
        let bits = [255u8, 0, 0, 0, 0, 255, 0, 0];
        let img = decode(&bmi, &bits).expect("bild");
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(&img.rgba[0..4], &[0, 0, 255, 255]);
        assert_eq!(&img.rgba[4..8], &[255, 0, 0, 255]);
        // Packad (huvud och bildpunkter i samma block, som i WMF).
        let mut packed = bmi.clone();
        packed.extend_from_slice(&bits);
        let img2 = decode(&packed, &[]).expect("packad");
        assert_eq!(img2.rgba, img.rgba);
        let avg = average(&img);
        assert!((avg.r - 0.5).abs() < 0.01 && (avg.b - 0.5).abs() < 0.01);
    }

    #[test]
    fn trasig_dib_ger_inget() {
        assert!(decode(&[1, 2, 3], &[]).is_none());
        assert!(decode(&[40, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0x7F, 1, 0, 0, 0, 1, 0, 24, 0], &[1, 2]).is_none());
    }
}
