//! Uppspelaren för EMF/WMF: ritytans tillstånd (penna, pensel, typsnitt, färger, fönster/viewport,
//! världstransform, klippning, banor) och ritoperationerna. Tolkarna i `records.rs` (EMF) och
//! `wmf.rs` (WMF) läser posterna och anropar operationerna här.
//!
//! Punkter transformeras direkt när de läses (som i GDI): logiska koordinater → världstransform →
//! sidtransform (fönster/viewport och mappningsläge) → enhetens bildpunkter → bildens ram i
//! punkter. Resultatet är `scene::Item`s i bildkoordinater, så klippning och text blir rätt i
//! både rastret och PDF:en.
//!
//! Beteendet (stockobjekt, banparentes, textjustering, kosmetiska pennor) följer MS-EMF/MS-WMF och
//! Apache POI:s `hwmf`/`hemf` (Apache-2.0, se `NOTICE.md`).

use std::collections::HashMap;
use std::sync::Arc;

use super::super::color::Rgba;
use super::super::fonts::FontBook;
use super::super::scene::{Affine, Cap, GlyphRun, ImageData, Item, Join, Paint, Path, PlacedGlyph, Stroke};

/// Mappningslägen (`SetMapMode`).
pub const MM_TEXT: u32 = 1;
pub const MM_LOMETRIC: u32 = 2;
pub const MM_HIMETRIC: u32 = 3;
pub const MM_LOENGLISH: u32 = 4;
pub const MM_HIENGLISH: u32 = 5;
pub const MM_TWIPS: u32 = 6;
pub const MM_ISOTROPIC: u32 = 7;
pub const MM_ANISOTROPIC: u32 = 8;

/// En linjestil för en penna.
#[derive(Clone, Debug)]
pub struct Pen {
    pub style: u32,
    /// Bredd i logiska enheter; 0 eller kosmetisk = en bildpunkt.
    pub width: f32,
    pub cosmetic: bool,
    pub color: Rgba,
}

impl Pen {
    pub const NULL: u32 = 5;

    fn visible(&self) -> bool {
        self.style & 0xF != Pen::NULL && self.color.a > 0.0
    }
}

/// En pensel.
#[derive(Clone, Debug)]
pub enum Brush {
    Solid(Rgba),
    Null,
    /// Streckad pensel: ritas som en blandning av färgen och bakgrunden (mönstret görs inte).
    Hatched(Rgba),
    /// Mönster ur en bitmapp: ritas som bitmappens medelfärg.
    Pattern(Rgba),
}

/// Ett logiskt typsnitt (`LOGFONT`).
#[derive(Clone, Debug)]
pub struct Font {
    pub height: f32,
    /// Vridning i tiondels grader, moturs.
    pub escapement: f32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub face: String,
}

impl Default for Font {
    fn default() -> Font {
        Font { height: 0.0, escapement: 0.0, weight: 400, italic: false, underline: false, strike: false, face: "Arial".into() }
    }
}

/// Ett objekt i objekttabellen.
#[derive(Clone, Debug)]
pub enum Obj {
    Pen(Pen),
    Brush(Brush),
    Font(Font),
    /// Palett, region och annat som bara tar en plats.
    Other,
}

/// En klippbana med ett id (för att jämföra klipplistor billigt).
#[derive(Clone, Debug)]
struct Clip {
    id: u32,
    path: Path,
}

/// Ritytans tillstånd (det som `SaveDC`/`RestoreDC` sparar).
#[derive(Clone, Debug)]
pub struct Dc {
    pub pen: Pen,
    pub brush: Brush,
    pub font: Font,
    pub text_color: Rgba,
    pub bk_color: Rgba,
    /// `OPAQUE` (2) eller `TRANSPARENT` (1).
    pub bk_mode: u32,
    pub text_align: u32,
    pub even_odd: bool,
    pub map_mode: u32,
    pub win_org: (f32, f32),
    pub win_ext: (f32, f32),
    pub vp_org: (f32, f32),
    pub vp_ext: (f32, f32),
    /// Har fönstret satts (WMF utan `Placeable`-huvud behöver det)?
    pub win_set: bool,
    pub world: Affine,
    clips: Vec<Clip>,
}

impl Default for Dc {
    fn default() -> Dc {
        Dc {
            pen: Pen { style: 0, width: 0.0, cosmetic: true, color: Rgba::BLACK },
            brush: Brush::Solid(Rgba::WHITE),
            font: Font::default(),
            text_color: Rgba::BLACK,
            bk_color: Rgba::WHITE,
            bk_mode: 2,
            text_align: 0,
            even_odd: true,
            map_mode: MM_TEXT,
            win_org: (0.0, 0.0),
            win_ext: (1.0, 1.0),
            vp_org: (0.0, 0.0),
            vp_ext: (1.0, 1.0),
            win_set: false,
            world: Affine::IDENTITY,
            clips: Vec::new(),
        }
    }
}

/// Hur enhetens koordinater hamnar i bildens ram.
#[derive(Clone, Copy, Debug)]
pub enum Mapping {
    /// EMF: bildpunkter → 0,01 mm (`mm100` per bildpunkt) → ramen `frame` (0,01 mm, l, t, r, b).
    Emf { mm100: (f32, f32), frame: (f32, f32, f32, f32) },
    /// WMF: logiska enheter i `bbox` (l, t, r, b) → ramen; fönstret gäller om det har satts.
    Wmf { bbox: (f32, f32, f32, f32) },
}

/// Uppspelaren.
pub struct Player<'f> {
    fonts: &'f FontBook,
    /// Ramen på bilden: enhetsramen (0..w × 0..h punkter) → bild.
    frame_t: Affine,
    frame_w: f32,
    frame_h: f32,
    mapping: Mapping,
    pub dc: Dc,
    saved: Vec<Dc>,
    pub objects: HashMap<u32, Obj>,
    /// Aktuell punkt i logiska koordinater.
    pub cur: (f32, f32),
    /// Banparentes (`BeginPath` … `EndPath`) i bildkoordinater.
    pub bracket: Option<Path>,
    /// Den färdiga banan efter `EndPath`.
    pub last_path: Option<Path>,
    /// Klipp som just nu ligger i `items` (id:n i ordning).
    emitted: Vec<u32>,
    next_clip: u32,
    /// Cachad transform logiskt → bild.
    xf: Affine,
    pub items: Vec<Item>,
    /// Antal ritade element (0 = inget att visa).
    pub drawn: usize,
}

impl<'f> Player<'f> {
    pub fn new(fonts: &'f FontBook, frame_t: Affine, frame_w: f32, frame_h: f32, mapping: Mapping) -> Player<'f> {
        let mut p = Player {
            fonts,
            frame_t,
            frame_w,
            frame_h,
            mapping,
            dc: Dc::default(),
            saved: Vec::new(),
            objects: HashMap::new(),
            cur: (0.0, 0.0),
            bracket: None,
            last_path: None,
            emitted: Vec::new(),
            next_clip: 1,
            xf: Affine::IDENTITY,
            items: Vec::new(),
            drawn: 0,
        };
        p.update_xf();
        p
    }

    /// Avslutar uppspelningen: stänger öppna klipp.
    pub fn finish(mut self) -> (Vec<Item>, usize) {
        for _ in 0..self.emitted.len() {
            self.items.push(Item::PopClip);
        }
        self.emitted.clear();
        (self.items, self.drawn)
    }

    // ---- Transformer ----

    /// Sidtransformen (logiskt efter världstransformen → enhet).
    fn page(&self) -> Affine {
        let d = &self.dc;
        let (wox, woy) = d.win_org;
        let (vox, voy) = d.vp_org;
        let metric = |mm_per_unit: f32| -> Affine {
            // Bildpunkter per logisk enhet ur enhetens storlek; WMF räknar i ramens enheter direkt.
            let (px, py) = match self.mapping {
                Mapping::Emf { mm100, .. } => (mm_per_unit * 100.0 / mm100.0.max(1e-6), mm_per_unit * 100.0 / mm100.1.max(1e-6)),
                Mapping::Wmf { .. } => (1.0, 1.0),
            };
            Affine::translate(-wox, -woy).then(&Affine::scale(px, -py)).then(&Affine::translate(vox, voy))
        };
        match d.map_mode {
            MM_LOMETRIC => metric(0.1),
            MM_HIMETRIC => metric(0.01),
            MM_LOENGLISH => metric(0.254),
            MM_HIENGLISH => metric(0.0254),
            MM_TWIPS => metric(25.4 / 1440.0),
            MM_ISOTROPIC | MM_ANISOTROPIC => {
                let (wex, wey) = d.win_ext;
                let (vex, vey) = d.vp_ext;
                let mut sx = if wex.abs() > 1e-9 { vex / wex } else { 1.0 };
                let mut sy = if wey.abs() > 1e-9 { vey / wey } else { 1.0 };
                if d.map_mode == MM_ISOTROPIC {
                    let m = sx.abs().min(sy.abs());
                    sx = m * sx.signum();
                    sy = m * sy.signum();
                }
                Affine::translate(-wox, -woy).then(&Affine::scale(sx, sy)).then(&Affine::translate(vox, voy))
            }
            _ => Affine::translate(-wox + vox, -woy + voy),
        }
    }

    /// Enhet → bild.
    fn device(&self) -> Affine {
        match self.mapping {
            Mapping::Emf { mm100, frame } => {
                let (l, t, r, b) = frame;
                let fw = (r - l).abs().max(1e-6);
                let fh = (b - t).abs().max(1e-6);
                Affine::scale(mm100.0, mm100.1)
                    .then(&Affine::translate(-l, -t))
                    .then(&Affine::scale(self.frame_w / fw, self.frame_h / fh))
                    .then(&self.frame_t)
            }
            Mapping::Wmf { bbox } => {
                // WMF har ingen enhet: fönstret (eller `Placeable`-ramen) fyller bildens ram.
                let (l, t, r, b) = if self.dc.win_set {
                    let (ox, oy) = self.dc.win_org;
                    let (ex, ey) = self.dc.win_ext;
                    (ox, oy, ox + ex, oy + ey)
                } else {
                    bbox
                };
                let fw = if (r - l).abs() > 1e-6 { r - l } else { 1.0 };
                let fh = if (b - t).abs() > 1e-6 { b - t } else { 1.0 };
                Affine::translate(-l, -t).then(&Affine::scale(self.frame_w / fw, self.frame_h / fh)).then(&self.frame_t)
            }
        }
    }

    /// Räknar om den cachade transformen (efter ändrat fönster, viewport, mappning eller värld).
    pub fn update_xf(&mut self) {
        self.xf = match self.mapping {
            // WMF: fönstret avbildas direkt på ramen (ingen viewport).
            Mapping::Wmf { .. } => self.dc.world.then(&self.device()),
            Mapping::Emf { .. } => self.dc.world.then(&self.page()).then(&self.device()),
        };
    }

    pub fn pt(&self, x: f32, y: f32) -> (f32, f32) {
        self.xf.apply(x, y)
    }

    /// Längden av en logisk vektor i bilden (för pennbredder och textstorlek).
    fn vec_len(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = self.xf.apply(0.0, 0.0);
        let (x1, y1) = self.xf.apply(x, y);
        ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt()
    }

    // ---- Tillstånd ----

    pub fn save_dc(&mut self) {
        if self.saved.len() < 1024 {
            self.saved.push(self.dc.clone());
        }
    }

    /// `RestoreDC(n)`: negativt n = relativt, positivt = absolut nivå.
    pub fn restore_dc(&mut self, n: i32) {
        let len = self.saved.len() as i64;
        let target = if n < 0 { len + n as i64 } else { n as i64 - 1 };
        if target < 0 || target >= len {
            return;
        }
        self.saved.truncate(target as usize + 1);
        if let Some(dc) = self.saved.pop() {
            self.dc = dc;
        }
        self.update_xf();
    }

    pub fn select(&mut self, index: u32) {
        if index & 0x8000_0000 != 0 {
            self.stock(index);
            return;
        }
        match self.objects.get(&index) {
            Some(Obj::Pen(p)) => self.dc.pen = p.clone(),
            Some(Obj::Brush(b)) => self.dc.brush = b.clone(),
            Some(Obj::Font(f)) => self.dc.font = f.clone(),
            _ => {}
        }
    }

    /// Stockobjekten (`0x80000000 | n`).
    fn stock(&mut self, index: u32) {
        let gray = |v: f32| Brush::Solid(Rgba { r: v, g: v, b: v, a: 1.0 });
        match index & 0x7FFF_FFFF {
            0 => self.dc.brush = gray(1.0),
            1 => self.dc.brush = gray(0.753),
            2 => self.dc.brush = gray(0.502),
            3 => self.dc.brush = gray(0.251),
            4 => self.dc.brush = gray(0.0),
            5 => self.dc.brush = Brush::Null,
            6 => self.dc.pen = Pen { style: 0, width: 0.0, cosmetic: true, color: Rgba::WHITE },
            7 => self.dc.pen = Pen { style: 0, width: 0.0, cosmetic: true, color: Rgba::BLACK },
            8 => self.dc.pen = Pen { style: Pen::NULL, width: 0.0, cosmetic: true, color: Rgba::BLACK },
            10 | 11 | 16 => self.dc.font = Font { face: "Courier New".into(), ..Font::default() },
            12..=14 | 17 => self.dc.font = Font::default(),
            _ => {}
        }
    }

    // ---- Klippning ----

    fn new_clip(&mut self, path: Path) -> Clip {
        let id = self.next_clip;
        self.next_clip += 1;
        Clip { id, path }
    }

    /// Snittar klippningen med en bana i bildkoordinater.
    pub fn clip_intersect(&mut self, path: Path) {
        if self.dc.clips.len() < 64 {
            let c = self.new_clip(path);
            self.dc.clips.push(c);
        }
    }

    /// Ersätter klippningen (`RGN_COPY`); `None` tar bort den.
    pub fn clip_replace(&mut self, path: Option<Path>) {
        self.dc.clips.clear();
        if let Some(p) = path {
            self.clip_intersect(p);
        }
    }

    /// Klipp med en logisk rektangel.
    pub fn clip_rect(&mut self, l: f32, t: f32, r: f32, b: f32) {
        let p = self.rect_path(l, t, r, b);
        self.clip_intersect(p);
    }

    /// Ser till att klippen i `items` motsvarar tillståndets (före varje ritning).
    fn sync_clips(&mut self) {
        let want: Vec<u32> = self.dc.clips.iter().map(|c| c.id).collect();
        let common = self.emitted.iter().zip(&want).take_while(|(a, b)| a == b).count();
        for _ in common..self.emitted.len() {
            self.items.push(Item::PopClip);
        }
        self.emitted.truncate(common);
        for c in &self.dc.clips[common..] {
            self.items.push(Item::PushClip { path: c.path.clone() });
            self.emitted.push(c.id);
        }
    }

    // ---- Banor ----

    /// En polygon eller polylinje (logiska punkter) som bana i bildkoordinater.
    pub fn poly_path(&self, pts: &[(f32, f32)], close: bool) -> Path {
        let mut p = Path::default();
        for (i, &(x, y)) in pts.iter().enumerate() {
            let (x, y) = self.pt(x, y);
            if i == 0 {
                p.move_to(x, y);
            } else {
                p.line_to(x, y);
            }
        }
        if close && pts.len() > 1 {
            p.close();
        }
        p
    }

    /// En rektangel i enhetens koordinater (bildpunkter; regioner i EMF) som bana i bilden.
    pub fn device_rect_path(&self, l: f32, t: f32, r: f32, b: f32) -> Path {
        let d = self.device();
        let mut p = Path::default();
        for (i, &(x, y)) in [(l, t), (r, t), (r, b), (l, b)].iter().enumerate() {
            let (x, y) = d.apply(x, y);
            if i == 0 {
                p.move_to(x, y)
            } else {
                p.line_to(x, y)
            }
        }
        p.close();
        p
    }

    pub fn rect_path(&self, l: f32, t: f32, r: f32, b: f32) -> Path {
        self.poly_path(&[(l, t), (r, t), (r, b), (l, b)], true)
    }

    /// Lägger en bana i banparentesen eller ritar den direkt.
    pub fn emit(&mut self, path: Path, fill: bool, stroke: bool) {
        if let Some(b) = &mut self.bracket {
            b.cmds.extend(path.cmds);
            return;
        }
        if fill {
            self.fill(path.clone());
        }
        if stroke {
            self.stroke(path);
        }
    }

    pub fn fill(&mut self, path: Path) {
        let color = match &self.dc.brush {
            Brush::Null => return,
            Brush::Solid(c) | Brush::Pattern(c) => *c,
            Brush::Hatched(c) => {
                let bk = if self.dc.bk_mode == 2 { self.dc.bk_color } else { Rgba::WHITE };
                mix(bk, *c, 0.4)
            }
        };
        if path.is_empty() {
            return;
        }
        self.sync_clips();
        self.items.push(Item::Fill { path, paint: Paint::Solid(color), even_odd: self.dc.even_odd });
        self.drawn += 1;
    }

    pub fn stroke(&mut self, path: Path) {
        let pen = self.dc.pen.clone();
        if !pen.visible() || path.is_empty() {
            return;
        }
        // Kosmetiska pennor är en bildpunkt breda oavsett skala (≈ 0,75 pt på en 96 dpi-skärm).
        let width = if pen.cosmetic || pen.width <= 0.0 { 0.75 } else { self.vec_len(pen.width, 0.0).max(0.1) };
        let dash: Vec<f32> = match pen.style & 0xF {
            1 => vec![3.0, 1.0],
            2 => vec![1.0, 1.0],
            3 => vec![3.0, 1.0, 1.0, 1.0],
            4 => vec![3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            _ => Vec::new(),
        }
        .into_iter()
        .map(|v| v * width.max(0.75))
        .collect();
        let cap = match pen.style & 0xF00 {
            0x100 => Cap::Square,
            0x200 => Cap::Butt,
            _ => Cap::Round,
        };
        let join = match pen.style & 0xF000 {
            0x1000 => Join::Bevel,
            0x2000 => Join::Miter,
            _ => Join::Round,
        };
        self.sync_clips();
        self.items.push(Item::Stroke { path, stroke: Stroke { paint: Paint::Solid(pen.color), width, dash, cap, join } });
        self.drawn += 1;
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.cur = (x, y);
        if let Some(b) = &mut self.bracket {
            let (x, y) = self.xf.apply(x, y);
            b.move_to(x, y);
        }
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        let from = self.cur;
        self.cur = (x, y);
        if let Some(b) = &mut self.bracket {
            if b.cmds.is_empty() {
                let (fx, fy) = self.xf.apply(from.0, from.1);
                b.move_to(fx, fy);
            }
            let (x, y) = self.xf.apply(x, y);
            b.line_to(x, y);
            return;
        }
        let p = self.poly_path(&[from, (x, y)], false);
        self.stroke(p);
    }

    /// `PolylineTo`/`PolyBezierTo` från aktuell punkt.
    pub fn poly_to(&mut self, pts: &[(f32, f32)], bezier: bool) {
        let mut p = Path::default();
        let (cx, cy) = self.pt(self.cur.0, self.cur.1);
        let in_bracket = self.bracket.as_ref().is_some_and(|b| !b.cmds.is_empty());
        if !in_bracket {
            p.move_to(cx, cy);
        }
        if bezier {
            for c in pts.chunks_exact(3) {
                let a = self.pt(c[0].0, c[0].1);
                let b = self.pt(c[1].0, c[1].1);
                let e = self.pt(c[2].0, c[2].1);
                p.cubic_to(a.0, a.1, b.0, b.1, e.0, e.1);
                self.cur = c[2];
            }
        } else {
            for &(x, y) in pts {
                let (tx, ty) = self.pt(x, y);
                p.line_to(tx, ty);
                self.cur = (x, y);
            }
        }
        self.emit(p, false, true);
    }

    /// `PolyBezier` (första punkten är start).
    pub fn bezier(&mut self, pts: &[(f32, f32)]) {
        let Some(&first) = pts.first() else { return };
        let mut p = Path::default();
        let (x, y) = self.pt(first.0, first.1);
        p.move_to(x, y);
        for c in pts[1..].chunks_exact(3) {
            let a = self.pt(c[0].0, c[0].1);
            let b = self.pt(c[1].0, c[1].1);
            let e = self.pt(c[2].0, c[2].1);
            p.cubic_to(a.0, a.1, b.0, b.1, e.0, e.1);
        }
        self.emit(p, false, true);
    }

    pub fn polygon(&mut self, pts: &[(f32, f32)]) {
        let p = self.poly_path(pts, true);
        self.emit(p, true, true);
    }

    pub fn polyline(&mut self, pts: &[(f32, f32)]) {
        let p = self.poly_path(pts, false);
        self.emit(p, false, true);
    }

    /// `PolyPolygon`: alla delar fylls som en bana (jämn/udda eller vindning efter läget).
    pub fn poly_polygon(&mut self, polys: &[Vec<(f32, f32)>]) {
        let mut p = Path::default();
        for poly in polys {
            p.cmds.extend(self.poly_path(poly, true).cmds);
        }
        self.emit(p, true, true);
    }

    pub fn poly_polyline(&mut self, polys: &[Vec<(f32, f32)>]) {
        let mut p = Path::default();
        for poly in polys {
            p.cmds.extend(self.poly_path(poly, false).cmds);
        }
        self.emit(p, false, true);
    }

    pub fn rectangle(&mut self, l: f32, t: f32, r: f32, b: f32) {
        let p = self.rect_path(l, t, r, b);
        self.emit(p, true, true);
    }

    /// Punkter längs en ellips (logiskt) från vinkel `a0` med svepet `sweep` (radianer, y nedåt).
    fn ellipse_pts(cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, sweep: f32) -> Vec<(f32, f32)> {
        let n = ((sweep.abs() / std::f32::consts::TAU) * 64.0).ceil().clamp(4.0, 256.0) as usize;
        (0..=n)
            .map(|i| {
                let a = a0 + sweep * i as f32 / n as f32;
                (cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect()
    }

    pub fn ellipse(&mut self, l: f32, t: f32, r: f32, b: f32) {
        let pts = Self::ellipse_pts((l + r) / 2.0, (t + b) / 2.0, (r - l).abs() / 2.0, (b - t).abs() / 2.0, 0.0, std::f32::consts::TAU);
        let p = self.poly_path(&pts, true);
        self.emit(p, true, true);
    }

    pub fn round_rect(&mut self, l: f32, t: f32, r: f32, b: f32, cw: f32, ch: f32) {
        let (l, r) = (l.min(r), l.max(r));
        let (t, b) = (t.min(b), t.max(b));
        let rx = (cw.abs() / 2.0).min((r - l) / 2.0);
        let ry = (ch.abs() / 2.0).min((b - t) / 2.0);
        use std::f32::consts::{FRAC_PI_2, PI};
        let mut pts = Vec::new();
        pts.extend(Self::ellipse_pts(r - rx, t + ry, rx, ry, -FRAC_PI_2, FRAC_PI_2));
        pts.extend(Self::ellipse_pts(r - rx, b - ry, rx, ry, 0.0, FRAC_PI_2));
        pts.extend(Self::ellipse_pts(l + rx, b - ry, rx, ry, FRAC_PI_2, FRAC_PI_2));
        pts.extend(Self::ellipse_pts(l + rx, t + ry, rx, ry, PI, FRAC_PI_2));
        let p = self.poly_path(&pts, true);
        self.emit(p, true, true);
    }

    /// `Arc`/`Chord`/`Pie`/`ArcTo`: bågen moturs (som GDI:s standardriktning) från start- till slutpunktens vinkel.
    /// `kind`: 0 båge, 1 korda, 2 tårtbit, 3 båge från aktuell punkt.
    pub fn arc(&mut self, rect: (f32, f32, f32, f32), start: (f32, f32), end: (f32, f32), kind: u8) {
        let (l, t, r, b) = rect;
        let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
        let (rx, ry) = ((r - l).abs() / 2.0, (b - t).abs() / 2.0);
        if rx < 1e-6 || ry < 1e-6 {
            return;
        }
        let a0 = (start.1 - cy).atan2(start.0 - cx);
        let a1 = (end.1 - cy).atan2(end.0 - cx);
        // Ellipsens parametervinkel för punktens riktning; moturs på skärmen (y nedåt) = minskande vinkel.
        let param = |a: f32| (a.sin() / ry).atan2(a.cos() / rx);
        let p0 = param(a0);
        let mut ps = param(a1) - p0;
        if ps >= 0.0 {
            ps -= std::f32::consts::TAU;
        }
        let mut pts = Self::ellipse_pts(cx, cy, rx, ry, p0, ps);
        match kind {
            2 => {
                pts.push((cx, cy));
                let p = self.poly_path(&pts, true);
                self.emit(p, true, true);
            }
            1 => {
                let p = self.poly_path(&pts, true);
                self.emit(p, true, true);
            }
            3 => {
                let last = *pts.last().unwrap_or(&self.cur);
                pts.insert(0, self.cur);
                let p = self.poly_path(&pts, false);
                self.emit(p, false, true);
                self.cur = last;
            }
            _ => {
                let p = self.poly_path(&pts, false);
                self.emit(p, false, true);
            }
        }
    }

    pub fn begin_path(&mut self) {
        self.bracket = Some(Path::default());
    }

    pub fn end_path(&mut self) {
        self.last_path = self.bracket.take();
    }

    pub fn close_figure(&mut self) {
        if let Some(b) = &mut self.bracket {
            b.close();
        }
    }

    /// `FillPath`/`StrokePath`/`StrokeAndFillPath`.
    pub fn draw_path(&mut self, fill: bool, stroke: bool) {
        // En bana som inte avslutats med EndPath används också (trasiga filer).
        let path = self.last_path.take().or_else(|| self.bracket.take());
        if let Some(p) = path {
            if fill {
                self.fill(p.clone());
            }
            if stroke {
                self.stroke(p);
            }
        }
    }

    /// `SelectClipPath`.
    pub fn select_clip_path(&mut self, mode: u32) {
        let Some(p) = self.last_path.take().or_else(|| self.bracket.take()) else { return };
        match mode {
            // RGN_COPY
            5 => self.clip_replace(Some(p)),
            // RGN_AND (och övriga som snitt: närmast utan regionlogik).
            _ => self.clip_intersect(p),
        }
    }

    // ---- Bilder ----

    /// Ritar en bitmapp i den logiska rektangeln (x, y, w, h); `src` är källrektangeln i bildpunkter.
    pub fn image(&mut self, img: ImageData, dest: (f32, f32, f32, f32), src: Option<(f32, f32, f32, f32)>) {
        let (x, y, w, h) = dest;
        let (iw, ih) = (img.width as f32, img.height as f32);
        if iw <= 0.0 || ih <= 0.0 {
            return;
        }
        let (p0x, p0y) = self.pt(x, y);
        let (p1x, p1y) = self.pt(x + w, y);
        let (p2x, p2y) = self.pt(x, y + h);
        let t = Affine { a: p1x - p0x, b: p1y - p0y, c: p2x - p0x, d: p2y - p0y, e: p0x, f: p0y };
        let crop = match src {
            Some((sx, sy, sw, sh)) if sw > 0.0 && sh > 0.0 => [sx / iw, sy / ih, 1.0 - (sx + sw) / iw, 1.0 - (sy + sh) / ih],
            _ => [0.0; 4],
        };
        self.sync_clips();
        self.items.push(Item::Image { image: Arc::new(img), t, crop, alpha: 1.0 });
        self.drawn += 1;
    }

    /// `BitBlt` utan källa: fyller rektangeln enligt rasteroperationen.
    pub fn pattern_blt(&mut self, dest: (f32, f32, f32, f32), rop: u32) {
        let (x, y, w, h) = dest;
        let p = self.rect_path(x, y, x + w, y + h);
        let old = self.dc.brush.clone();
        match rop {
            // BLACKNESS
            0x0000_0042 => self.dc.brush = Brush::Solid(Rgba::BLACK),
            // WHITENESS
            0x00FF_0062 => self.dc.brush = Brush::Solid(Rgba::WHITE),
            // DSTINVERT, NOP: inget som går att efterlikna.
            0x0055_0009 | 0x00AA_0029 => return,
            _ => {}
        }
        self.fill(p);
        self.dc.brush = old;
    }

    // ---- Text ----

    /// Ritar text vid den logiska punkten `at` med teckenavstånden `dx` (logiska enheter, kan saknas).
    /// `opaque`/`clip` är rektangeln för `ETO_OPAQUE`/`ETO_CLIPPED`.
    pub fn text(&mut self, at: (f32, f32), text: &str, dx: &[f32], opaque: Option<(f32, f32, f32, f32)>, clip: Option<(f32, f32, f32, f32)>) {
        let update_cp = self.dc.text_align & 1 != 0;
        let at = if update_cp { self.cur } else { at };
        if let Some((l, t, r, b)) = opaque {
            let p = self.rect_path(l, t, r, b);
            let old = std::mem::replace(&mut self.dc.brush, Brush::Solid(self.dc.bk_color));
            self.fill(p);
            self.dc.brush = old;
        }
        let chars: Vec<char> = text.chars().collect();
        if chars.iter().all(|c| c.is_whitespace() || c.is_control()) {
            return;
        }
        let font = self.dc.font.clone();
        let res = self.fonts.resolve(&font.face, font.weight >= 600, font.italic);
        let face = res.face.clone();
        let cell = face.win_ascent + face.win_descent;
        // Negativ höjd = teckenhöjd (em), positiv = cellhöjd.
        let em = if font.height < 0.0 {
            -font.height
        } else if font.height > 0.0 {
            font.height / cell.max(0.5)
        } else {
            12.0
        };
        // Glyfer och lägen i logiska enheter längs baslinjen.
        struct G {
            face: std::sync::Arc<super::super::fonts::FontFace>,
            id: u16,
            x: f32,
            adv: f32,
            x_scale: f32,
        }
        let mut glyphs: Vec<G> = Vec::new();
        let mut x = 0.0f32;
        for (i, &c) in chars.iter().enumerate() {
            let (gface, xs) = match face.face().and_then(|f| f.glyph_index(c)) {
                Some(_) => (face.clone(), res.x_scale),
                None => match self.fonts.fallback(c, &face) {
                    Some(f) => (f, 1.0),
                    None => (face.clone(), res.x_scale),
                },
            };
            let (id, nat) = gface
                .face()
                .map(|f| {
                    let id = f.glyph_index(c).map_or(0, |g| g.0);
                    let adv = f.glyph_hor_advance(ttf_parser::GlyphId(id)).unwrap_or(0) as f32 / f.units_per_em() as f32;
                    (id, adv * em * xs)
                })
                .unwrap_or((0, em * 0.5));
            let adv = dx.get(i).copied().unwrap_or(nat);
            if !c.is_whitespace() && !c.is_control() {
                glyphs.push(G { face: gface, id, x, adv, x_scale: xs });
            }
            x += adv;
        }
        let total = x;
        let ta = self.dc.text_align;
        let x0 = match ta & 6 {
            6 => -total / 2.0,
            2 => -total,
            _ => 0.0,
        };
        let y0 = match ta & 24 {
            24 => 0.0,
            8 => -face.win_descent * em,
            _ => face.win_ascent * em,
        };
        if update_cp {
            // Aktuell punkt flyttas längs baslinjen (bara vid vänsterjustering).
            if ta & 6 == 0 {
                self.cur = (at.0 + total, at.1);
            }
        }
        // Körningens koordinater (logiska enheter, x längs texten, y nedåt) → bild. Texten speglas
        // aldrig: "nedåt" tas vinkelrätt mot x-riktningen även när sidtransformen vänder y-axeln.
        let (px, py) = self.pt(at.0, at.1);
        let (ux, uy) = {
            let (a, b) = self.pt(at.0 + 1.0, at.1);
            (a - px, b - py)
        };
        let vy_len = self.vec_len(0.0, 1.0);
        let ul = (ux * ux + uy * uy).sqrt().max(1e-9);
        let (nx, ny) = (ux / ul, uy / ul);
        let (dx_, dy_) = (-ny * vy_len, nx * vy_len);
        let esc = Affine::rotate(-font.escapement / 10.0);
        let run_t = esc.then(&Affine { a: ux, b: uy, c: dx_, d: dy_, e: px, f: py });
        let color = self.dc.text_color;
        let clip_depth = self.dc.clips.len();
        if let Some((l, tp, r, b)) = clip {
            let p = self.rect_path(l, tp, r, b);
            self.clip_intersect(p);
        }
        self.sync_clips();
        // En körning per typsnitt.
        let mut i = 0;
        while i < glyphs.len() {
            let f0 = glyphs[i].face.id;
            let xs = glyphs[i].x_scale;
            let mut placed = Vec::new();
            let mut s = String::new();
            while i < glyphs.len() && glyphs[i].face.id == f0 {
                let g = &glyphs[i];
                placed.push(PlacedGlyph { id: g.id, x: x0 + g.x, y: y0, advance: g.adv, ch: super::super::scene::NO_CHAR, word: super::super::scene::NO_CHAR });
                i += 1;
            }
            if f0 == face.id {
                s = text.trim().to_owned();
            }
            let gface = glyphs[i - 1].face.clone();
            self.items.push(Item::Glyphs(GlyphRun {
                font: gface,
                size: em,
                t: run_t,
                glyphs: placed,
                paint: Paint::Solid(color),
                outline: None,
                text: s,
                fake_bold: res.fake_bold,
                fake_italic: res.fake_italic,
                x_scale: xs,
            }));
            self.drawn += 1;
        }
        // Understrykning och genomstrykning.
        for (on, off) in [(font.underline, em * 0.12), (font.strike, -em * 0.28)] {
            if on {
                let th = (em * 0.06).max(0.5);
                let mut p = Path::default();
                let pts = [(x0, y0 + off), (x0 + total, y0 + off), (x0 + total, y0 + off + th), (x0, y0 + off + th)];
                for (k, &(a, b)) in pts.iter().enumerate() {
                    let (a, b) = run_t.apply(a, b);
                    if k == 0 {
                        p.move_to(a, b)
                    } else {
                        p.line_to(a, b)
                    }
                }
                p.close();
                self.items.push(Item::Fill { path: p, paint: Paint::Solid(color), even_odd: false });
            }
        }
        self.dc.clips.truncate(clip_depth);
    }
}

pub fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    Rgba { r: a.r + (b.r - a.r) * k, g: a.g + (b.g - a.g) * k, b: a.b + (b.b - a.b) * k, a: 1.0 }
}

/// `COLORREF` (0x00BBGGRR).
pub fn colorref(v: u32) -> Rgba {
    Rgba::rgb8((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8)
}
