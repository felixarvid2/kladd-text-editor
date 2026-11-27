//! EMF/WMF-bilder (Windows-metafiler) ritade till visningslistan.
//!
//! `render` känner igen formatet (EMF-huvud, WMF med eller utan `Placeable`-huvud), spelar upp
//! posterna (`records.rs`/`wmf.rs`) med uppspelaren i `player.rs` och ger `Scene`-element i
//! bildkoordinater. EMF+ (i `GdiComment`) ignoreras: PowerPoints och Offices dubbla filer har alltid
//! EMF-reserven. Skrivet efter MS-EMF/MS-WMF med Apache POI:s `hemf`/`hwmf` som förebild för
//! beteendet (Apache-2.0, se `NOTICE.md`).

pub mod dib;
pub mod player;
pub mod records;
pub mod wmf;

use super::fonts::FontBook;
use super::scene::{Affine, Item};

/// Spelar upp en EMF/WMF i ramen `t` (enhetsramen 0..w × 0..h punkter → bild). `None` om filen
/// inte kan läsas eller inte ritar något (t.ex. bara EMF+).
pub fn render(bytes: &[u8], t: Affine, w: f32, h: f32, fonts: &FontBook) -> Option<Vec<Item>> {
    let (items, drawn) = if let Some(hdr) = records::header(bytes) {
        let mut p = player::Player::new(fonts, t, w, h, records::mapping(&hdr));
        records::play(bytes, &mut p);
        p.finish()
    } else {
        let (bbox, start) = wmf::header(bytes)?;
        let mut p = player::Player::new(fonts, t, w, h, wmf::mapping(bbox));
        wmf::play(bytes, start, &mut p);
        p.finish()
    };
    (drawn > 0).then_some(items)
}

/// Hjälp för prov: bygger EMF-filer.
#[cfg(test)]
pub(crate) mod tests_support {
    /// Bygger en EMF: huvud (ram 0..10 000 × 0..10 000 i 0,01 mm, enhet 1000 × 1000 bildpunkter
    /// på 100 × 100 mm, alltså 0,1 mm per bildpunkt) och posterna.
    pub fn emf(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut hdr = vec![0u8; 108];
        let put = |b: &mut Vec<u8>, o: usize, v: i32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut hdr, 0, 1);
        put(&mut hdr, 4, 108);
        put(&mut hdr, 16, 999);
        put(&mut hdr, 20, 999);
        put(&mut hdr, 32, 10_000);
        put(&mut hdr, 36, 10_000);
        hdr[40..44].copy_from_slice(b" EMF");
        put(&mut hdr, 44, 0x10000);
        put(&mut hdr, 72, 1000);
        put(&mut hdr, 76, 1000);
        put(&mut hdr, 80, 100);
        put(&mut hdr, 84, 100);
        put(&mut hdr, 100, 100_000);
        put(&mut hdr, 104, 100_000);
        out.extend(hdr);
        for (typ, body) in records.iter().chain(std::iter::once(&(14u32, vec![0u8; 12]))) {
            let mut body = body.clone();
            while body.len() % 4 != 0 {
                body.push(0);
            }
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
            out.extend(body);
        }
        out
    }

    pub fn le(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

}

#[cfg(test)]
mod tests {
    use super::tests_support::{emf, le};
    use super::*;
    use crate::pptx::color::Rgba;
    use crate::pptx::scene::{Cmd, Paint};

    fn fonts() -> FontBook {
        FontBook::default()
    }

    fn fills(items: &[Item]) -> Vec<(&crate::pptx::scene::Path, Rgba, bool)> {
        items
            .iter()
            .filter_map(|i| match i {
                Item::Fill { path, paint: Paint::Solid(c), even_odd } => Some((path, *c, *even_odd)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn polygon_med_pensel_hamnar_i_ramen() {
        // Röd pensel (objekt 1), NULL_PEN, en polygon 0..500 bildpunkter = halva ramen.
        let bytes = emf(&[
            (39, le(&[1, 0, 0x0000FF, 0])),
            (37, le(&[1])),
            (37, vec![8, 0, 0, 0x80]),
            (3, [le(&[0, 0, 500, 500]), le(&[4, 0, 0, 500, 0, 500, 500, 0, 500])].concat()),
        ]);
        let items = render(&bytes, Affine::translate(100.0, 50.0), 200.0, 100.0, &fonts()).expect("ritar");
        let f = fills(&items);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].1, Rgba::rgb8(255, 0, 0));
        let b = f[0].0.bounds().unwrap();
        assert!((b.0 - 100.0).abs() < 0.01 && (b.1 - 50.0).abs() < 0.01, "{b:?}");
        assert!((b.2 - 200.0).abs() < 0.01 && (b.3 - 100.0).abs() < 0.01, "{b:?}");
        // NULL_PEN: ingen linje.
        assert!(!items.iter().any(|i| matches!(i, Item::Stroke { .. })));
    }

    #[test]
    fn banparentes_jamn_udda_och_fonster() {
        // Fönster 0..100 → viewport 0..1000 (anisotropiskt): logiska 0..50 blir halva ramen.
        let bytes = emf(&[
            (17, le(&[8])),
            (9, le(&[100, 100])),
            (11, le(&[1000, 1000])),
            (19, le(&[1])),
            (59, vec![]),
            (27, le(&[0, 0])),
            (6, [le(&[0, 0, 0, 0]), le(&[3, 50, 0, 50, 50, 0, 50])].concat()),
            (61, vec![]),
            (60, vec![]),
            (62, le(&[0, 0, 0, 0])),
        ]);
        let items = render(&bytes, Affine::IDENTITY, 100.0, 100.0, &fonts()).expect("ritar");
        let f = fills(&items);
        assert_eq!(f.len(), 1, "{items:?}");
        assert!(f[0].2, "ALTERNATE = jämn/udda");
        let b = f[0].0.bounds().unwrap();
        assert!((b.2 - 50.0).abs() < 0.01 && (b.3 - 50.0).abs() < 0.01, "{b:?}");
        assert!(f[0].0.cmds.contains(&Cmd::Close));
    }

    #[test]
    fn klipp_ar_balanserade_med_savedc() {
        let bytes = emf(&[
            (33, vec![]),
            (30, le(&[0, 0, 100, 100])),
            (43, le(&[0, 0, 50, 50])),
            (34, le(&[-1])),
            (43, le(&[0, 0, 50, 50])),
            (33, vec![]),
            (30, le(&[10, 10, 20, 20])),
        ]);
        let items = render(&bytes, Affine::IDENTITY, 100.0, 100.0, &fonts()).expect("ritar");
        let push = items.iter().filter(|i| matches!(i, Item::PushClip { .. })).count();
        let pop = items.iter().filter(|i| matches!(i, Item::PopClip)).count();
        assert_eq!(push, 1, "klippet i sista SaveDC används aldrig");
        assert_eq!(push, pop);
        // Andra rektangeln ritas efter RestoreDC, utan klipp.
        let last_fill = items.iter().rposition(|i| matches!(i, Item::Fill { .. })).unwrap();
        let last_pop = items.iter().rposition(|i| matches!(i, Item::PopClip)).unwrap();
        assert!(last_pop < last_fill);
    }

    #[test]
    fn text_med_typsnitt_och_teckenavstand() {
        // Typsnitt (−100 = 10 mm em), svart text "Hej" med dx 100 vid (100, 500), baslinjejusterad.
        let mut font = le(&[2, -100, 0, 0, 0, 700]);
        font.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        let mut face = [0u8; 64];
        for (i, c) in "Arial".encode_utf16().enumerate() {
            face[2 * i..2 * i + 2].copy_from_slice(&c.to_le_bytes());
        }
        font.extend_from_slice(&face);
        font.extend(vec![0u8; 320]);
        let mut text = le(&[0, 0, 0, 0, 1]);
        text.extend(le(&[0, 0]));
        // EMRTEXT: ref, n, offString, options, rect, offDx.
        text.extend(le(&[100, 500, 3, 76, 0, 0, 0, 0, 0, 84]));
        for c in "Hej".encode_utf16() {
            text.extend_from_slice(&c.to_le_bytes());
        }
        text.extend([0u8, 0]);
        text.extend(le(&[100, 100, 100]));
        let bytes = emf(&[(82, font), (37, le(&[2])), (22, le(&[24])), (84, text)]);
        let items = render(&bytes, Affine::IDENTITY, 100.0, 100.0, &fonts()).expect("ritar");
        let runs: Vec<_> = items.iter().filter_map(|i| if let Item::Glyphs(g) = i { Some(g) } else { None }).collect();
        assert_eq!(runs.len(), 1);
        let g = runs[0];
        assert_eq!(g.text, "Hej");
        assert_eq!(g.glyphs.len(), 3);
        // Logisk enhet = 0,1 mm = 0,01 av ramen: em 100 → 10 pt i en 100 pt-ram, dx 100 → 10 pt.
        let (x0, y0) = g.t.apply(g.glyphs[0].x, g.glyphs[0].y);
        let (x1, _) = g.t.apply(g.glyphs[1].x, g.glyphs[1].y);
        assert!((x0 - 10.0).abs() < 0.01 && (y0 - 50.0).abs() < 0.01, "({x0}, {y0})");
        assert!((x1 - x0 - 10.0).abs() < 0.01);
        assert!((g.size * g.t.scale_factor() - 10.0).abs() < 0.01);
    }

    #[test]
    fn y_upp_vander_inte_texten() {
        // MM_HIMETRIC (y uppåt): texten ska ändå stå rättvänd (positiv determinant).
        let mut text = le(&[0, 0, 0, 0, 1]);
        text.extend(le(&[0, 0]));
        text.extend(le(&[100, -500, 1, 76, 0, 0, 0, 0, 0, 0]));
        text.extend_from_slice(&('A' as u16).to_le_bytes());
        text.extend([0u8, 0]);
        let bytes = emf(&[(17, le(&[3])), (84, text)]);
        let items = render(&bytes, Affine::IDENTITY, 100.0, 100.0, &fonts()).expect("ritar");
        let g = items.iter().find_map(|i| if let Item::Glyphs(g) = i { Some(g) } else { None }).unwrap();
        assert!(g.t.a * g.t.d - g.t.b * g.t.c > 0.0);
    }

    #[test]
    fn stretchdibits_ger_bild_och_bara_emfplus_ger_inget() {
        let mut bmi = le(&[40, 1, 1]);
        bmi.extend_from_slice(&1u16.to_le_bytes());
        bmi.extend_from_slice(&24u16.to_le_bytes());
        bmi.extend(vec![0u8; 24]);
        let mut rec = le(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 1]);
        rec.extend(le(&[80, 40, 120, 4, 0, 0xCC0020, 1000, 1000]));
        rec.extend(&bmi);
        rec.extend([0u8, 0, 255, 0]);
        let bytes = emf(&[(81, rec)]);
        let items = render(&bytes, Affine::IDENTITY, 50.0, 50.0, &fonts()).expect("ritar");
        assert!(matches!(&items[0], Item::Image { image, t, .. } if image.width == 1 && (t.a - 50.0).abs() < 0.01));
        // Bara en GdiComment (EMF+): inget ritat → None (platshållaren tar över).
        let bytes = emf(&[(70, [le(&[8]), b"EMF+".to_vec(), le(&[0])].concat())]);
        assert!(render(&bytes, Affine::IDENTITY, 50.0, 50.0, &fonts()).is_none());
    }

    #[test]
    fn wmf_med_placeable_och_polygon() {
        let mut b = Vec::new();
        b.extend_from_slice(&0x9AC6_CDD7u32.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        for v in [0i16, 0, 1000, 1000, 1440] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[0; 6]);
        // META_HEADER.
        for v in [1u16, 9, 0x300, 0, 0, 1, 0, 0, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let rec = |b: &mut Vec<u8>, func: u16, params: &[i16]| {
            b.extend_from_slice(&((3 + params.len()) as u32).to_le_bytes());
            b.extend_from_slice(&func.to_le_bytes());
            for p in params {
                b.extend_from_slice(&p.to_le_bytes());
            }
        };
        // Grön pensel (COLORREF 0x00FF00 = två ord), välj, polygon 0..500.
        rec(&mut b, 0x02FC, &[0, 0xFF00u16 as i16, 0, 0]);
        rec(&mut b, 0x012D, &[0]);
        rec(&mut b, 0x0324, &[3, 0, 0, 500, 0, 500, 500]);
        rec(&mut b, 0, &[]);
        let items = render(&b, Affine::IDENTITY, 100.0, 100.0, &fonts()).expect("ritar");
        let f = fills(&items);
        assert_eq!(f[0].1, Rgba::rgb8(0, 255, 0));
        let bb = f[0].0.bounds().unwrap();
        assert!((bb.2 - 50.0).abs() < 0.01 && (bb.3 - 50.0).abs() < 0.01, "{bb:?}");
    }

    #[test]
    fn trasiga_filer_panikar_inte() {
        let base = emf(&[
            (39, le(&[1, 0, 0xFF, 0])),
            (37, le(&[1])),
            (91, le(&[0, 0, 9, 9, 2, 6, 3, 3, 0, 0, 5, 5, 0, 0, 0, 0])),
            (84, le(&[0; 30])),
            (81, le(&[0; 30])),
        ]);
        let mut seed = 0x2545_F491u32;
        for _ in 0..300 {
            let mut b = base.clone();
            for _ in 0..8 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let i = 108 + (seed as usize % (b.len() - 108));
                b[i] = (seed >> 8) as u8;
            }
            let _ = render(&b, Affine::IDENTITY, 10.0, 10.0, &fonts());
            let cut = 108 + seed as usize % (b.len() - 108);
            let _ = render(&b[..cut], Affine::IDENTITY, 10.0, 10.0, &fonts());
        }
        assert!(render(b"inte en bild", Affine::IDENTITY, 1.0, 1.0, &fonts()).is_none());
    }
}
