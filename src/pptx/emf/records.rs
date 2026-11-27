//! EMF-poster (MS-EMF) → uppspelaren. Den vanliga delmängden: polygoner, polylinjer, Bézier,
//! rektanglar, ellipser, bågar, banor (`BeginPath` … `FillPath`/`StrokePath`/`SelectClipPath`),
//! pennor, penslar, typsnitt och `ExtTextOutW`, bitmappar (`StretchDIBits`, `BitBlt`, `StretchBlt`,
//! `SetDIBitsToDevice`, `AlphaBlend`), fönster/viewport, mappningslägen, världstransform,
//! `SaveDC`/`RestoreDC` och rektangelklipp. EMF+ (i `GdiComment`) hoppas över: dubbla filer har alltid
//! en EMF-reserv, och filer med bara EMF+ ritar då ingenting (anroparen visar en platshållare).

use super::super::scene::Affine;
use super::dib;
use super::player::{colorref, Brush, Font, Mapping, Obj, Pen, Player};

/// Läsare med gränskontroll (inget panikar på trasiga filer; utanför posten läses 0).
#[derive(Clone, Copy)]
pub struct Rd<'a> {
    pub b: &'a [u8],
}

impl<'a> Rd<'a> {
    pub fn u8(&self, o: usize) -> u8 {
        self.b.get(o).copied().unwrap_or(0)
    }
    pub fn u16(&self, o: usize) -> u16 {
        self.b.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn i16(&self, o: usize) -> i16 {
        self.u16(o) as i16
    }
    pub fn u32(&self, o: usize) -> u32 {
        self.b.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn i32(&self, o: usize) -> i32 {
        self.u32(o) as i32
    }
    pub fn f32(&self, o: usize) -> f32 {
        let v = f32::from_bits(self.u32(o));
        if v.is_finite() { v } else { 0.0 }
    }
    pub fn slice(&self, o: usize, len: usize) -> &'a [u8] {
        self.b.get(o..o.saturating_add(len)).unwrap_or(&[])
    }
    /// UTF-16LE-text med `n` tecken.
    pub fn utf16(&self, o: usize, n: usize) -> String {
        let n = n.min(self.b.len().saturating_sub(o) / 2);
        let units: Vec<u16> = (0..n).map(|i| self.u16(o + 2 * i)).collect();
        String::from_utf16_lossy(&units)
    }
}

/// EMF-huvudet: ramen (0,01 mm) och enhetens storlek.
pub struct Header {
    pub bounds: (i32, i32, i32, i32),
    pub frame: (f32, f32, f32, f32),
    pub device: (f32, f32),
    pub millimeters: (f32, f32),
    pub micrometers: Option<(f32, f32)>,
}

/// Läser EMF-huvudet (`None` om det inte är en EMF).
pub fn header(bytes: &[u8]) -> Option<Header> {
    let r = Rd { b: bytes };
    if r.u32(0) != 1 || bytes.get(40..44)? != b" EMF" {
        return None;
    }
    let size = r.u32(4) as usize;
    let micrometers = (size >= 108).then(|| (r.i32(100) as f32, r.i32(104) as f32)).filter(|(x, y)| *x > 0.0 && *y > 0.0);
    Some(Header {
        bounds: (r.i32(8), r.i32(12), r.i32(16), r.i32(20)),
        frame: (r.i32(24) as f32, r.i32(28) as f32, r.i32(32) as f32, r.i32(36) as f32),
        device: (r.i32(72) as f32, r.i32(76) as f32),
        millimeters: (r.i32(80) as f32, r.i32(84) as f32),
        micrometers,
    })
}

/// Mappningen enhet → ram för huvudet.
pub fn mapping(h: &Header) -> Mapping {
    // 0,01 mm per bildpunkt ur enhetens storlek (mikrometer om de finns, de är noggrannare).
    let (dw, dh) = (h.device.0.max(1.0), h.device.1.max(1.0));
    let mm100 = match h.micrometers {
        Some((uw, uh)) => (uw / 10.0 / dw, uh / 10.0 / dh),
        None if h.millimeters.0 > 0.0 && h.millimeters.1 > 0.0 => (h.millimeters.0 * 100.0 / dw, h.millimeters.1 * 100.0 / dh),
        // Utan enhet: anta 96 dpi.
        None => (2540.0 / 96.0, 2540.0 / 96.0),
    };
    let (l, t, r, b) = h.frame;
    let frame = if r > l && b > t {
        (l, t, r, b)
    } else {
        // Ingen ram: rita gränsrektangeln (bildpunkter) i ramen.
        let (bl, bt, br, bb) = h.bounds;
        (bl as f32 * mm100.0, bt as f32 * mm100.1, (br + 1) as f32 * mm100.0, (bb + 1) as f32 * mm100.1)
    };
    Mapping::Emf { mm100, frame }
}

fn points32(r: &Rd<'_>, off: usize, n: usize) -> Vec<(f32, f32)> {
    let n = n.min(r.b.len().saturating_sub(off) / 8);
    (0..n).map(|i| (r.i32(off + 8 * i) as f32, r.i32(off + 8 * i + 4) as f32)).collect()
}

fn points16(r: &Rd<'_>, off: usize, n: usize) -> Vec<(f32, f32)> {
    let n = n.min(r.b.len().saturating_sub(off) / 4);
    (0..n).map(|i| (r.i16(off + 4 * i) as f32, r.i16(off + 4 * i + 2) as f32)).collect()
}

/// `PolyPolygon`/`PolyPolyline` (16 eller 32 bitar).
fn poly_poly(r: &Rd<'_>, small: bool) -> Vec<Vec<(f32, f32)>> {
    let npolys = (r.u32(24) as usize).min(r.b.len() / 4);
    let total = r.u32(28) as usize;
    let counts: Vec<usize> = (0..npolys).map(|i| r.u32(32 + 4 * i) as usize).collect();
    let mut off = 32 + 4 * npolys;
    let all = if small { points16(r, off, total) } else { points32(r, off, total) };
    let mut out = Vec::new();
    off = 0;
    for c in counts {
        let end = (off + c).min(all.len());
        if off >= end {
            break;
        }
        out.push(all[off..end].to_vec());
        off = end;
    }
    out
}

fn rect(r: &Rd<'_>, o: usize) -> (f32, f32, f32, f32) {
    (r.i32(o) as f32, r.i32(o + 4) as f32, r.i32(o + 8) as f32, r.i32(o + 12) as f32)
}

fn xform(r: &Rd<'_>, o: usize) -> Affine {
    Affine { a: r.f32(o), b: r.f32(o + 4), c: r.f32(o + 8), d: r.f32(o + 12), e: r.f32(o + 16), f: r.f32(o + 20) }
}

/// `LOGFONTW` från offset `o`.
pub fn logfont(r: &Rd<'_>, o: usize) -> Font {
    let face = r.utf16(o + 28, 32);
    let face = face.split('\0').next().unwrap_or("").trim().to_owned();
    Font {
        height: r.i32(o) as f32,
        escapement: r.i32(o + 8) as f32,
        weight: r.i32(o + 16),
        italic: r.u8(o + 20) != 0,
        underline: r.u8(o + 21) != 0,
        strike: r.u8(o + 22) != 0,
        face: if face.is_empty() { "Arial".into() } else { face },
    }
}

/// En bitmapp ur en post: (bmi-offset, bmi-längd, bits-offset, bits-längd) i posten.
fn record_dib(r: &Rd<'_>, offs: (usize, usize, usize, usize)) -> Option<super::super::scene::ImageData> {
    let (ob, cb, ox, cx) = (r.u32(offs.0) as usize, r.u32(offs.1) as usize, r.u32(offs.2) as usize, r.u32(offs.3) as usize);
    if cb == 0 {
        return None;
    }
    dib::decode(r.slice(ob, cb), r.slice(ox, cx))
}

/// Spelar upp alla poster i en EMF.
pub fn play(bytes: &[u8], p: &mut Player<'_>) {
    let mut off = 0usize;
    let mut guard = 0usize;
    while off + 8 <= bytes.len() && guard < 2_000_000 {
        guard += 1;
        let typ = u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
        let size = u32::from_le_bytes([bytes[off + 4], bytes[off + 5], bytes[off + 6], bytes[off + 7]]) as usize;
        if size < 8 || off + size > bytes.len() {
            break;
        }
        let r = Rd { b: &bytes[off..off + size] };
        if typ == 14 {
            break;
        }
        record(typ, &r, p);
        off += size;
    }
}

fn record(typ: u32, r: &Rd<'_>, p: &mut Player<'_>) {
    match typ {
        // POLYBEZIER, POLYGON, POLYLINE, POLYBEZIERTO, POLYLINETO (32 bitar) och 16-bitarsvarianterna.
        2..=6 | 85..=89 => {
            let n = r.u32(24) as usize;
            let pts = if typ >= 85 { points16(r, 28, n) } else { points32(r, 28, n) };
            match if typ >= 85 { typ - 83 } else { typ } {
                2 => p.bezier(&pts),
                3 => p.polygon(&pts),
                4 => p.polyline(&pts),
                5 => p.poly_to(&pts, true),
                _ => p.poly_to(&pts, false),
            }
        }
        // POLYPOLYLINE, POLYPOLYGON (+16).
        7 | 8 | 90 | 91 => {
            let polys = poly_poly(r, typ >= 90);
            if typ == 7 || typ == 90 {
                p.poly_polyline(&polys);
            } else {
                p.poly_polygon(&polys);
            }
        }
        9 => {
            p.dc.win_ext = (r.i32(8) as f32, r.i32(12) as f32);
            p.dc.win_set = true;
            p.update_xf();
        }
        10 => {
            p.dc.win_org = (r.i32(8) as f32, r.i32(12) as f32);
            p.dc.win_set = true;
            p.update_xf();
        }
        11 => {
            p.dc.vp_ext = (r.i32(8) as f32, r.i32(12) as f32);
            p.update_xf();
        }
        12 => {
            p.dc.vp_org = (r.i32(8) as f32, r.i32(12) as f32);
            p.update_xf();
        }
        17 => {
            p.dc.map_mode = r.u32(8);
            p.update_xf();
        }
        18 => p.dc.bk_mode = r.u32(8),
        19 => p.dc.even_odd = r.u32(8) != 2,
        22 => p.dc.text_align = r.u32(8),
        24 => p.dc.text_color = colorref(r.u32(8)),
        25 => p.dc.bk_color = colorref(r.u32(8)),
        27 => p.move_to(r.i32(8) as f32, r.i32(12) as f32),
        // EXCLUDECLIPRECT: kan inte uttryckas som snitt; hoppas över.
        29 => {}
        30 => {
            let (l, t, rr, b) = rect(r, 8);
            p.clip_rect(l, t, rr, b);
        }
        33 => p.save_dc(),
        34 => p.restore_dc(r.i32(8)),
        35 => {
            p.dc.world = xform(r, 8);
            p.update_xf();
        }
        36 => {
            let x = xform(r, 8);
            p.dc.world = match r.u32(32) {
                1 => Affine::IDENTITY,
                // MWT_LEFTMULTIPLY: ny = x · gammal (x först).
                2 => x.then(&p.dc.world),
                3 => p.dc.world.then(&x),
                4 => x,
                _ => p.dc.world,
            };
            p.update_xf();
        }
        37 => p.select(r.u32(8)),
        38 => {
            // CREATEPEN: LOGPEN (stil, bredd som POINTL, färg).
            let style = r.u32(12);
            let w = r.i32(16) as f32;
            p.objects.insert(r.u32(8), Obj::Pen(Pen { style, width: w, cosmetic: w <= 0.0, color: colorref(r.u32(24)) }));
        }
        39 => {
            let b = match r.u32(12) {
                1 => Brush::Null,
                2 => Brush::Hatched(colorref(r.u32(16))),
                _ => Brush::Solid(colorref(r.u32(16))),
            };
            p.objects.insert(r.u32(8), Obj::Brush(b));
        }
        40 => {
            p.objects.remove(&r.u32(8));
        }
        42 => {
            let (l, t, rr, b) = rect(r, 8);
            p.ellipse(l, t, rr, b);
        }
        43 => {
            let (l, t, rr, b) = rect(r, 8);
            p.rectangle(l, t, rr, b);
        }
        44 => {
            let (l, t, rr, b) = rect(r, 8);
            p.round_rect(l, t, rr, b, r.i32(24) as f32, r.i32(28) as f32);
        }
        // ARC, CHORD, PIE, ARCTO.
        45..=47 | 55 => {
            let kind = match typ {
                45 => 0,
                46 => 1,
                47 => 2,
                _ => 3,
            };
            let start = (r.i32(24) as f32, r.i32(28) as f32);
            let end = (r.i32(32) as f32, r.i32(36) as f32);
            p.arc(rect(r, 8), start, end, kind);
        }
        54 => p.line_to(r.i32(8) as f32, r.i32(12) as f32),
        59 => p.begin_path(),
        60 => p.end_path(),
        61 => p.close_figure(),
        62 => p.draw_path(true, false),
        63 => p.draw_path(true, true),
        64 => p.draw_path(false, true),
        67 => p.select_clip_path(r.u32(8)),
        // ABORTPATH
        68 => {
            p.bracket = None;
            p.last_path = None;
        }
        75 => {
            // EXTSELECTCLIPRGN: RGN_COPY utan region tar bort klippningen; annars rektanglarnas
            // omslutning (en region är oftast en rektangel).
            let cb = r.u32(8) as usize;
            let mode = r.u32(12);
            if cb < 32 {
                if mode == 5 {
                    p.clip_replace(None);
                }
                return;
            }
            let n = r.u32(16 + 8) as usize;
            if n == 0 {
                if mode == 5 {
                    p.clip_replace(None);
                }
                return;
            }
            // Regionen är i enhetskoordinater; som bana i bilden.
            let mut path = super::super::scene::Path::default();
            for i in 0..n.min(4096) {
                let o = 16 + 32 + 16 * i;
                if o + 16 > r.b.len() {
                    break;
                }
                let (l, t, rr, b) = rect(r, o);
                path.cmds.extend(p.device_rect_path(l, t, rr, b).cmds);
            }
            if mode == 5 {
                p.clip_replace(Some(path));
            } else if mode == 1 {
                p.clip_intersect(path);
            }
        }
        76 | 77 | 114 => {
            // BITBLT, STRETCHBLT, ALPHABLEND.
            let dest = (r.i32(24) as f32, r.i32(28) as f32, r.i32(32) as f32, r.i32(36) as f32);
            let rop = r.u32(40);
            match record_dib(r, (84, 88, 92, 96)) {
                Some(img) => {
                    let src = if typ == 76 {
                        (r.i32(44) as f32, r.i32(48) as f32, dest.2.abs(), dest.3.abs())
                    } else {
                        (r.i32(44) as f32, r.i32(48) as f32, r.i32(100) as f32, r.i32(104) as f32)
                    };
                    p.image(img, dest, Some(src));
                }
                None if typ != 114 => p.pattern_blt(dest, rop),
                None => {}
            }
        }
        80 => {
            // SETDIBITSTODEVICE: källrektangeln i bildpunkter, målet lika stort.
            if let Some(img) = record_dib(r, (48, 52, 56, 60)) {
                let src = (r.i32(32) as f32, r.i32(36) as f32, r.i32(40) as f32, r.i32(44) as f32);
                let dest = (r.i32(24) as f32, r.i32(28) as f32, src.2, src.3);
                let src = src_from_top(r, 48, img.height, src);
                p.image(img, dest, Some(src));
            }
        }
        81 => {
            // STRETCHDIBITS
            let rop = r.u32(68);
            if let Some(img) = record_dib(r, (48, 52, 56, 60)) {
                let src = (r.i32(32) as f32, r.i32(36) as f32, r.i32(40) as f32, r.i32(44) as f32);
                let dest = (r.i32(24) as f32, r.i32(28) as f32, r.i32(72) as f32, r.i32(76) as f32);
                let src = src_from_top(r, 48, img.height, src);
                match rop {
                    // SRCAND (masken i ett mask/bild-par) ritas inte.
                    0x0088_00C6 => {}
                    // SRCPAINT: svart blir genomskinligt.
                    0x00EE_0086 => p.image(black_transparent(img), dest, Some(src)),
                    _ => p.image(img, dest, Some(src)),
                }
            }
        }
        82 => {
            let font = logfont(r, 12);
            p.objects.insert(r.u32(8), Obj::Font(font));
        }
        // EXTTEXTOUTA/W
        83 | 84 => text_out(r, p, typ == 84),
        94 | 93 => {
            // CREATEDIBPATTERNBRUSHPT / CREATEMONOBRUSH: bitmappens medelfärg.
            let c = record_dib(r, (16, 20, 24, 28)).map(|i| dib::average(&i)).unwrap_or(super::super::color::Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 });
            p.objects.insert(r.u32(8), Obj::Brush(Brush::Pattern(c)));
        }
        95 => {
            // EXTCREATEPEN: LOGPENEX efter bitmappsfälten.
            let style = r.u32(28);
            let w = r.u32(32) as f32;
            let geometric = style & 0x0001_0000 != 0;
            let brush_style = r.u32(36);
            let color = colorref(r.u32(40));
            let style = if brush_style == 1 { Pen::NULL } else { style };
            p.objects.insert(r.u32(8), Obj::Pen(Pen { style, width: w, cosmetic: !geometric || w <= 0.0, color }));
        }
        // CREATEPALETTE m.fl.: tar bara en plats.
        49 => {
            p.objects.insert(r.u32(8), Obj::Other);
        }
        _ => {}
    }
}

/// Källrektangeln uppifrån räknat: i en DIB som lagras nerifrån och upp (positiv höjd) räknas
/// `StretchDIBits`/`SetDIBitsToDevice`:s y från nederkanten.
fn src_from_top(r: &Rd<'_>, bmi_off: usize, img_h: u32, src: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let bmi = r.u32(bmi_off) as usize;
    let bottom_up = r.u32(bmi) >= 40 && r.i32(bmi + 8) > 0;
    if bottom_up {
        (src.0, img_h as f32 - src.1 - src.3, src.2, src.3)
    } else {
        src
    }
}

fn black_transparent(mut img: super::super::scene::ImageData) -> super::super::scene::ImageData {
    for px in img.rgba.chunks_exact_mut(4) {
        if px[0] < 8 && px[1] < 8 && px[2] < 8 {
            px[3] = 0;
        }
    }
    img
}

/// `EMR_EXTTEXTOUTW`/`A`.
fn text_out(r: &Rd<'_>, p: &mut Player<'_>, wide: bool) {
    // EMRTEXT börjar på 36.
    let at = (r.i32(36) as f32, r.i32(40) as f32);
    let n = (r.u32(44) as usize).min(8192);
    let off_str = r.u32(48) as usize;
    let options = r.u32(52);
    let rc = rect(r, 56);
    let off_dx = r.u32(72) as usize;
    // ETO_GLYPH_INDEX: texten är glyfnummer i typsnittet, inte tecken; kan inte ritas säkert.
    if options & 0x10 != 0 {
        return;
    }
    let text = if wide { r.utf16(off_str, n) } else { r.slice(off_str, n).iter().map(|&b| b as char).collect() };
    let pdy = options & 0x2000 != 0;
    let dx: Vec<f32> = if off_dx > 0 {
        (0..n).map(|i| r.i32(off_dx + if pdy { 8 * i } else { 4 * i }) as f32).take_while(|_| off_dx + 4 * n <= r.b.len()).collect()
    } else {
        Vec::new()
    };
    let has_rect = rc.2 > rc.0 && rc.3 > rc.1;
    let opaque = (options & 0x2 != 0 && has_rect).then_some(rc);
    let clip = (options & 0x4 != 0 && has_rect).then_some(rc);
    p.text(at, &text, &dx, opaque, clip);
}
