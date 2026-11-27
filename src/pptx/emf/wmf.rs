//! WMF-poster (MS-WMF) → uppspelaren: samma delmängd som EMF (polygoner, linjer, rektanglar,
//! ellipser, bågar, pennor, penslar, typsnitt, `TextOut`/`ExtTextOut`, DIB-bitmappar, fönster,
//! `SaveDC`/`RestoreDC`, rektangelklipp). Koordinaterna är 16-bitars och i många poster omvänd
//! ordning (y före x).

use super::dib;
use super::player::{colorref, Brush, Font, Mapping, Obj, Pen, Player};
use super::records::Rd;

/// `Placeable`-huvudets nyckel.
const PLACEABLE: u32 = 0x9AC6_CDD7;

/// Ramen i logiska enheter ur `Placeable`-huvudet (eller `None`) och var posterna börjar.
pub fn header(bytes: &[u8]) -> Option<(Option<(f32, f32, f32, f32)>, usize)> {
    let r = Rd { b: bytes };
    let (bbox, start) = if r.u32(0) == PLACEABLE {
        let b = (r.i16(6) as f32, r.i16(8) as f32, r.i16(10) as f32, r.i16(12) as f32);
        (Some(b), 22)
    } else {
        (None, 0)
    };
    // META_HEADER: typ 1 (minne) eller 2 (fil), huvudstorlek 9 ord.
    let typ = r.u16(start);
    let hsize = r.u16(start + 2);
    if !(typ == 1 || typ == 2) || hsize != 9 {
        return None;
    }
    Some((bbox, start + 18))
}

pub fn mapping(bbox: Option<(f32, f32, f32, f32)>) -> Mapping {
    Mapping::Wmf { bbox: bbox.unwrap_or((0.0, 0.0, 1000.0, 1000.0)) }
}

/// Lägsta lediga plats i objekttabellen (WMF numrerar objekt så).
fn free_slot(p: &Player<'_>) -> u32 {
    (0u32..).find(|i| !p.objects.contains_key(i)).unwrap_or(0)
}

fn add(p: &mut Player<'_>, o: Obj) {
    let i = free_slot(p);
    if i < 65536 {
        p.objects.insert(i, o);
    }
}

pub fn play(bytes: &[u8], start: usize, p: &mut Player<'_>) {
    let mut off = start;
    let mut guard = 0usize;
    while off + 6 <= bytes.len() && guard < 2_000_000 {
        guard += 1;
        let r0 = Rd { b: bytes };
        let words = r0.u32(off) as usize;
        let func = r0.u16(off + 4);
        let size = words.saturating_mul(2);
        if func == 0 || size < 6 || off + size > bytes.len() {
            break;
        }
        // Parametrarna börjar efter storlek och funktion.
        let r = Rd { b: &bytes[off + 6..off + size] };
        record(func, &r, p);
        off += size;
    }
}

fn pts16(r: &Rd<'_>, off: usize, n: usize) -> Vec<(f32, f32)> {
    let n = n.min(r.b.len().saturating_sub(off) / 4);
    (0..n).map(|i| (r.i16(off + 4 * i) as f32, r.i16(off + 4 * i + 2) as f32)).collect()
}

/// Rektangel lagrad som (botten, höger, topp, vänster).
fn rect_rev(r: &Rd<'_>, o: usize) -> (f32, f32, f32, f32) {
    (r.i16(o + 6) as f32, r.i16(o + 4) as f32, r.i16(o + 2) as f32, r.i16(o) as f32)
}

fn ansi(b: &[u8]) -> String {
    b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
}

fn record(func: u16, r: &Rd<'_>, p: &mut Player<'_>) {
    match func {
        0x001E => p.save_dc(),
        0x0127 => p.restore_dc(r.i16(0) as i32),
        0x0102 => p.dc.bk_mode = r.u16(0) as u32,
        0x0103 => {
            p.dc.map_mode = r.u16(0) as u32;
            p.update_xf();
        }
        0x0106 => p.dc.even_odd = r.u16(0) != 2,
        0x012E => p.dc.text_align = r.u16(0) as u32,
        0x0201 => p.dc.bk_color = colorref(r.u32(0)),
        0x0209 => p.dc.text_color = colorref(r.u32(0)),
        0x020B => {
            p.dc.win_org = (r.i16(2) as f32, r.i16(0) as f32);
            p.dc.win_set = true;
            p.update_xf();
        }
        0x020C => {
            p.dc.win_ext = (r.i16(2) as f32, r.i16(0) as f32);
            p.dc.win_set = true;
            p.update_xf();
        }
        0x020D => p.dc.vp_org = (r.i16(2) as f32, r.i16(0) as f32),
        0x020E => p.dc.vp_ext = (r.i16(2) as f32, r.i16(0) as f32),
        0x0213 => p.line_to(r.i16(2) as f32, r.i16(0) as f32),
        0x0214 => p.move_to(r.i16(2) as f32, r.i16(0) as f32),
        0x0324 | 0x0325 => {
            let n = r.u16(0) as usize;
            let pts = pts16(r, 2, n);
            if func == 0x0324 {
                p.polygon(&pts);
            } else {
                p.polyline(&pts);
            }
        }
        0x0538 => {
            let np = (r.u16(0) as usize).min(r.b.len() / 2);
            let counts: Vec<usize> = (0..np).map(|i| r.u16(2 + 2 * i) as usize).collect();
            let mut off = 2 + 2 * np;
            let mut polys = Vec::new();
            for c in counts {
                let pts = pts16(r, off, c);
                off += 4 * c;
                if pts.is_empty() {
                    break;
                }
                polys.push(pts);
            }
            p.poly_polygon(&polys);
        }
        0x041B => {
            let (l, t, rr, b) = rect_rev(r, 0);
            p.rectangle(l, t, rr, b);
        }
        0x0418 => {
            let (l, t, rr, b) = rect_rev(r, 0);
            p.ellipse(l, t, rr, b);
        }
        0x061C => {
            let (l, t, rr, b) = rect_rev(r, 4);
            p.round_rect(l, t, rr, b, r.i16(2) as f32, r.i16(0) as f32);
        }
        0x0817 | 0x081A | 0x0830 => {
            let end = (r.i16(2) as f32, r.i16(0) as f32);
            let start = (r.i16(6) as f32, r.i16(4) as f32);
            let kind = match func {
                0x0817 => 0,
                0x0830 => 1,
                _ => 2,
            };
            p.arc(rect_rev(r, 8), start, end, kind);
        }
        0x0416 => {
            let (l, t, rr, b) = rect_rev(r, 0);
            p.clip_rect(l, t, rr, b);
        }
        0x02FA => {
            let style = r.u16(0) as u32;
            let w = r.i16(2) as f32;
            add(p, Obj::Pen(Pen { style, width: w, cosmetic: w <= 0.0, color: colorref(r.u32(6)) }));
        }
        0x02FC => {
            let b = match r.u16(0) {
                1 => Brush::Null,
                2 => Brush::Hatched(colorref(r.u32(2))),
                _ => Brush::Solid(colorref(r.u32(2))),
            };
            add(p, Obj::Brush(b));
        }
        0x02FB => {
            let face = ansi(r.slice(18, 32));
            let f = Font {
                height: r.i16(0) as f32,
                escapement: r.i16(4) as f32,
                weight: r.i16(8) as i32,
                italic: r.u8(10) != 0,
                underline: r.u8(11) != 0,
                strike: r.u8(12) != 0,
                face: if face.trim().is_empty() { "Arial".into() } else { face.trim().to_owned() },
            };
            add(p, Obj::Font(f));
        }
        0x0142 => {
            let c = dib::decode(r.slice(4, r.b.len()), &[]).map(|i| dib::average(&i)).unwrap_or(super::super::color::Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 });
            add(p, Obj::Brush(Brush::Pattern(c)));
        }
        0x00F7 | 0x01F9 | 0x06FF => add(p, Obj::Other),
        0x012D => p.select(r.u16(0) as u32),
        0x01F0 => {
            p.objects.remove(&(r.u16(0) as u32));
        }
        0x0521 => {
            let n = r.i16(0).max(0) as usize;
            let s = ansi(r.slice(2, n));
            let o = 2 + n.div_ceil(2) * 2;
            p.text((r.i16(o + 2) as f32, r.i16(o) as f32), &s, &[], None, None);
        }
        0x0A32 => {
            let y = r.i16(0) as f32;
            let x = r.i16(2) as f32;
            let n = r.i16(4).max(0) as usize;
            let opts = r.u16(6);
            let mut o = 8;
            let rc = if opts & 0x6 != 0 {
                let rc = (r.i16(8) as f32, r.i16(10) as f32, r.i16(12) as f32, r.i16(14) as f32);
                o += 8;
                Some(rc)
            } else {
                None
            };
            let s = ansi(r.slice(o, n));
            o += n.div_ceil(2) * 2;
            let dx: Vec<f32> = if o + 2 * n <= r.b.len() { (0..n).map(|i| r.i16(o + 2 * i) as f32).collect() } else { Vec::new() };
            let opaque = rc.filter(|_| opts & 0x2 != 0);
            let clip = rc.filter(|_| opts & 0x4 != 0);
            p.text((x, y), &s, &dx, opaque, clip);
        }
        0x0F43 => {
            // STRETCHDIB: rop, färganvändning, källa (h, b, y, x), mål (h, b, y, x), DIB.
            let src = (r.i16(12) as f32, r.i16(10) as f32, r.i16(8) as f32, r.i16(6) as f32);
            let dest = (r.i16(20) as f32, r.i16(18) as f32, r.i16(16) as f32, r.i16(14) as f32);
            if let Some(img) = dib::decode(r.slice(22, r.b.len()), &[]) {
                let src = (src.0, img.height as f32 - src.1 - src.3, src.2, src.3);
                p.image(img, dest, Some(src));
            }
        }
        0x0B41 | 0x0940 => {
            // DIBSTRETCHBLT / DIBBITBLT.
            let rop = r.u32(0);
            let stretch = func == 0x0B41;
            let (src, dest, dib_off) = if stretch {
                (
                    (r.i16(10) as f32, r.i16(8) as f32, r.i16(6) as f32, r.i16(4) as f32),
                    (r.i16(18) as f32, r.i16(16) as f32, r.i16(14) as f32, r.i16(12) as f32),
                    20,
                )
            } else {
                let d = (r.i16(14) as f32, r.i16(12) as f32, r.i16(10) as f32, r.i16(8) as f32);
                ((r.i16(6) as f32, r.i16(4) as f32, d.2, d.3), d, 16)
            };
            match dib::decode(r.slice(dib_off, r.b.len()), &[]) {
                Some(img) => {
                    let src = (src.0, img.height as f32 - src.1 - src.3, src.2, src.3);
                    p.image(img, dest, Some(src));
                }
                None => p.pattern_blt(dest, rop),
            }
        }
        _ => {}
    }
}
