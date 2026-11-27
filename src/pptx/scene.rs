//! Visningslistan (`Scene`) som allt ritas genom: banor med fyllning/linje, bilder och
//! glyfkörningar med färdigplacerade glyfer. Både rastret (`raster.rs`, tiny-skia) och PDF:en
//! (`pdf.rs`, krilla) ritar samma `Scene`, så skärm och PDF blir lika.
//!
//! Koordinater är i punkter (1/72 tum) på bilden, origo uppe till vänster, y nedåt. Banor och
//! övertoningar lagras i bildkoordinater; bilder och glyfer har en egen transform.

use std::sync::{Arc, OnceLock};

use super::color::Rgba;
use super::fonts::FontFace;

/// Affin transform: x' = a·x + c·y + e, y' = b·x + d·y + f.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(x: f32, y: f32) -> Affine {
        Affine { e: x, f: y, ..Affine::IDENTITY }
    }

    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine { a: sx, d: sy, ..Affine::IDENTITY }
    }

    /// Rotation medurs (y nedåt) i grader.
    pub fn rotate(deg: f32) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }

    /// Rotation runt en punkt.
    pub fn rotate_at(deg: f32, cx: f32, cy: f32) -> Affine {
        Affine::translate(-cx, -cy).then(&Affine::rotate(deg)).then(&Affine::translate(cx, cy))
    }

    /// Först `self`, sedan `next`.
    pub fn then(&self, next: &Affine) -> Affine {
        let m = self;
        let n = next;
        Affine {
            a: n.a * m.a + n.c * m.b,
            b: n.b * m.a + n.d * m.b,
            c: n.a * m.c + n.c * m.d,
            d: n.b * m.c + n.d * m.d,
            e: n.a * m.e + n.c * m.f + n.e,
            f: n.b * m.e + n.d * m.f + n.f,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// Ungefärlig skalfaktor (för linjebredder).
    #[allow(dead_code)]
    pub fn scale_factor(&self) -> f32 {
        ((self.a * self.d - self.b * self.c).abs()).sqrt()
    }

    #[allow(dead_code)]
    pub fn invert(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let ia = self.d / det;
        let ib = -self.b / det;
        let ic = -self.c / det;
        let id = self.a / det;
        Some(Affine { a: ia, b: ib, c: ic, d: id, e: -(ia * self.e + ic * self.f), f: -(ib * self.e + id * self.f) })
    }
}

/// Ett banavsnitt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// En bana.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub cmds: Vec<Cmd>,
}

impl Path {
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.cmds.push(Cmd::Move(x, y));
    }
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.cmds.push(Cmd::Line(x, y));
    }
    pub fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.cmds.push(Cmd::Quad(x1, y1, x, y));
    }
    pub fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.cmds.push(Cmd::Cubic(x1, y1, x2, y2, x, y));
    }
    pub fn close(&mut self) {
        self.cmds.push(Cmd::Close);
    }

    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Path {
        let mut p = Path::default();
        p.move_to(x, y);
        p.line_to(x + w, y);
        p.line_to(x + w, y + h);
        p.line_to(x, y + h);
        p.close();
        p
    }

    pub fn is_empty(&self) -> bool {
        !self.cmds.iter().any(|c| !matches!(c, Cmd::Move(..) | Cmd::Close))
    }

    /// Banan med alla punkter transformerade.
    pub fn transformed(&self, t: &Affine) -> Path {
        let p = |x: f32, y: f32| t.apply(x, y);
        let cmds = self
            .cmds
            .iter()
            .map(|c| match *c {
                Cmd::Move(x, y) => {
                    let (x, y) = p(x, y);
                    Cmd::Move(x, y)
                }
                Cmd::Line(x, y) => {
                    let (x, y) = p(x, y);
                    Cmd::Line(x, y)
                }
                Cmd::Quad(a, b, x, y) => {
                    let (a, b) = p(a, b);
                    let (x, y) = p(x, y);
                    Cmd::Quad(a, b, x, y)
                }
                Cmd::Cubic(a, b, c2, d, x, y) => {
                    let (a, b) = p(a, b);
                    let (c2, d) = p(c2, d);
                    let (x, y) = p(x, y);
                    Cmd::Cubic(a, b, c2, d, x, y)
                }
                Cmd::Close => Cmd::Close,
            })
            .collect();
        Path { cmds }
    }

    /// Omslutande rektangel (x0, y0, x1, y1) över alla punkter (kontrollpunkter inräknade).
    #[allow(dead_code)]
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut b: Option<(f32, f32, f32, f32)> = None;
        let mut add = |x: f32, y: f32| {
            b = Some(match b {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        };
        for c in &self.cmds {
            match *c {
                Cmd::Move(x, y) | Cmd::Line(x, y) => add(x, y),
                Cmd::Quad(a, b2, x, y) => {
                    add(a, b2);
                    add(x, y)
                }
                Cmd::Cubic(a, b2, c2, d, x, y) => {
                    add(a, b2);
                    add(c2, d);
                    add(x, y)
                }
                Cmd::Close => {}
            }
        }
        b
    }
}

/// Ett stopp i en övertoning (läge 0..1).
#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub pos: f32,
    pub color: Rgba,
}

/// Hur en yta fylls.
#[derive(Clone, Debug)]
pub enum Paint {
    Solid(Rgba),
    /// Linjär övertoning mellan två punkter (bildkoordinater).
    Linear { x0: f32, y0: f32, x1: f32, y1: f32, stops: Vec<Stop> },
    /// Radiell övertoning (cirkel i bildkoordinater, `t` förvränger den till en ellips).
    Radial { cx: f32, cy: f32, r: f32, stops: Vec<Stop>, t: Affine },
    /// Bildfyllning: bilden ritas med `t` (enhetskvadrat → bild) och upprepas om `tile`.
    Image { image: Arc<ImageData>, t: Affine, tile: bool, alpha: f32 },
}

impl Paint {
    /// En ungefärlig enfärg (för PDF-text och reserver).
    pub fn average(&self) -> Rgba {
        match self {
            Paint::Solid(c) => *c,
            Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => {
                if stops.is_empty() {
                    return Rgba::BLACK;
                }
                let n = stops.len() as f32;
                let s = stops.iter().fold((0.0, 0.0, 0.0, 0.0), |a, s| (a.0 + s.color.r, a.1 + s.color.g, a.2 + s.color.b, a.3 + s.color.a));
                Rgba { r: s.0 / n, g: s.1 / n, b: s.2 / n, a: s.3 / n }
            }
            Paint::Image { .. } => Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Join {
    Miter,
    Round,
    Bevel,
}

/// Linjestil. Bredd och streckmönster i punkter.
#[derive(Clone, Debug)]
pub struct Stroke {
    pub paint: Paint,
    pub width: f32,
    pub dash: Vec<f32>,
    pub cap: Cap,
    pub join: Join,
}

/// En avkodad bild (RGBA, ej förmultiplicerad) med originalbyten för PDF.
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Originalfilen om den är JPEG (läggs in orörd i PDF:en).
    #[allow(dead_code)]
    pub jpeg: Option<Vec<u8>>,
    /// Förmultiplicerad pixmap för rastret (skapas vid första användning).
    pub(crate) pixmap: OnceLock<Option<resvg::tiny_skia::Pixmap>>,
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageData({}×{})", self.width, self.height)
    }
}

impl ImageData {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>, jpeg: Option<Vec<u8>>) -> ImageData {
        ImageData { width, height, rgba, jpeg, pixmap: OnceLock::new() }
    }
}

/// En glyf placerad relativt körningens origo (baslinjen), i punkter.
#[derive(Clone, Copy, Debug)]
pub struct PlacedGlyph {
    pub id: u16,
    pub x: f32,
    pub y: f32,
    /// Framflyttning (för PDF:ens textlager).
    #[allow(dead_code)]
    pub advance: f32,
}

/// En glyfkörning: samma typsnitt, storlek och fyllning.
#[derive(Clone, Debug)]
pub struct GlyphRun {
    pub font: Arc<FontFace>,
    pub size: f32,
    /// Körningens origo (baslinje) → bild.
    pub t: Affine,
    pub glyphs: Vec<PlacedGlyph>,
    pub paint: Paint,
    /// Konturlinje runt glyferna (`a:ln` på texten).
    pub outline: Option<Stroke>,
    pub text: String,
    /// Syntetisk fetstil (typsnittet saknar fet variant).
    pub fake_bold: bool,
    /// Syntetisk kursiv (lutning).
    pub fake_italic: bool,
    /// Vågrät skalning av glyferna (ersättningstypsnitt med annan teckenbredd).
    pub x_scale: f32,
}

/// En effekt på elementen mellan `Item::BeginEffect` och `Item::EndEffect` (DrawingML:s `a:effectLst`).
/// Mått i punkter, vinklar redan upplösta till förskjutningar.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Yttre skugga (`outerShdw`, även förinställda `prstShdw`): innehållets täckning, oskarp (`blur` =
    /// oskärperadien), färgad och förskjuten `dx`/`dy`. `t` skalar/snedvrider skuggan (`sx`/`sy`/`kx`/`ky`)
    /// runt justeringspunkten, i bildkoordinater, före förskjutningen.
    OuterShadow { dx: f32, dy: f32, blur: f32, color: Rgba, t: Affine },
    /// Inre skugga (`innerShdw`): skuggan av formens kant innanför formen.
    InnerShadow { dx: f32, dy: f32, blur: f32, color: Rgba },
    /// Glöd (`glow`): en oskarp gloria med radien `radius` utanför formen.
    Glow { radius: f32, color: Rgba },
    /// Mjuka kanter (`softEdge`): täckningen tonar ut inåt över `radius`.
    SoftEdge { radius: f32 },
    /// Spegling (`reflection`): innehållet speglat runt sin nederkant, `dist` nedanför, med genomskinlighet
    /// som går från `start_alpha` (vid `start_pos`) till `end_alpha` (vid `end_pos`, andelar av höjden).
    Reflection { blur: f32, dist: f32, start_alpha: f32, end_alpha: f32, start_pos: f32, end_pos: f32 },
    /// Tunnare text (syntetiskt tunt typsnitt, t.ex. Calibri Light): glyferna skrapas `width` punkter
    /// in från kanten.
    Thin { width: f32 },
}

/// Ett element i visningslistan.
#[derive(Clone, Debug)]
pub enum Item {
    Fill { path: Path, paint: Paint, even_odd: bool },
    Stroke { path: Path, stroke: Stroke },
    /// Bild: `t` avbildar enhetskvadraten på bilden; `crop` (vänster, topp, höger, botten) är andelar
    /// av bildens bredd/höjd som skärs bort (negativa värden ger luft).
    Image { image: Arc<ImageData>, t: Affine, crop: [f32; 4], alpha: f32 },
    /// SVG-bild (`asvg:svgBlip`): `t` avbildar enhetskvadraten på bilden.
    Svg { tree: Arc<resvg::usvg::Tree>, t: Affine, crop: [f32; 4], alpha: f32 },
    Glyphs(GlyphRun),
    /// Klipp till banan (bildkoordinater) tills motsvarande `PopClip`.
    PushClip { path: Path },
    PopClip,
    /// Elementen fram till motsvarande `EndEffect` ritas med effekterna (skugga, glöd …). Listan förblir
    /// platt: den som bara läser glyfkörningar (sökning) kan hoppa över markörerna.
    BeginEffect(Vec<Effect>),
    EndEffect,
}

/// En bild som visningslista.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    /// Storlek i punkter.
    pub width: f32,
    pub height: f32,
    pub items: Vec<Item>,
}

impl Scene {
    pub fn new(width: f32, height: f32) -> Scene {
        Scene { width, height, items: Vec::new() }
    }

    pub fn fill(&mut self, path: Path, paint: Paint) {
        if !path.is_empty() {
            self.items.push(Item::Fill { path, paint, even_odd: false });
        }
    }

    pub fn stroke(&mut self, path: Path, stroke: Stroke) {
        if !path.is_empty() && stroke.width > 0.0 {
            self.items.push(Item::Stroke { path, stroke });
        }
    }

    /// All text i visningslistan (för prov och sökning).
    #[allow(dead_code)]
    pub fn text(&self) -> String {
        let mut s = String::new();
        for it in &self.items {
            if let Item::Glyphs(g) = it {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(&g.text);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affin_ordning_och_invers() {
        let t = Affine::translate(10.0, 0.0).then(&Affine::scale(2.0, 2.0));
        assert_eq!(t.apply(1.0, 1.0), (22.0, 2.0));
        let r = Affine::rotate(90.0);
        let (x, y) = r.apply(1.0, 0.0);
        assert!((x - 0.0).abs() < 1e-6 && (y - 1.0).abs() < 1e-6, "medurs med y nedåt");
        let i = t.invert().unwrap();
        let (x, y) = i.apply(22.0, 2.0);
        assert!((x - 1.0).abs() < 1e-5 && (y - 1.0).abs() < 1e-5);
    }
}
