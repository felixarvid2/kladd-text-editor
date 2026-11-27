//! Bildövergångar (`p:transition`, även `p14:` som `xml.rs` väljer ur `mc:AlternateContent`) och
//! geometrin för att blanda två färdigrenderade bilder: övergången blir en lista med bitar (`Piece`),
//! var och en en konvex polygon i en texturs egna koordinater (0..1) med en affin avbildning till
//! målet. Vyn ritar bitarna som egui-nät, så GPU:n gör arbetet och inget renderas om under övergången.
//!
//! Samma avslöjningsformer (`Reveal`: torka, dela, cirkel, romb, plus, jalusi, schack, slumpstaplar,
//! upplösning, hjul, kil) används av animeringarnas in- och ut-effekter (`anim.rs`).
//!
//! Övergångar som saknar en enkel motsvarighet (`p14:vortex`, `ripple`, `glitter` …) blir en toning.
//! Morph (`p159:morph`) är ett eget slag (`Kind::Morph`): `morph.rs` paras ihop formerna och bildspelet
//! ritar dem som en forminterpolering. Utan färdiga lager (eller utan föregående bild) ritas den som en
//! toning (`pieces`).

use super::xml::Node;

/// Riktning för rörelser (PowerPoint skriver rörelsens riktning: `l` = mot vänster).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
    LeftUp,
    RightUp,
    LeftDown,
    RightDown,
}

impl Dir {
    fn parse(s: Option<&str>, default: Dir) -> Dir {
        match s {
            Some("l") => Dir::Left,
            Some("r") => Dir::Right,
            Some("u") => Dir::Up,
            Some("d") => Dir::Down,
            Some("lu") => Dir::LeftUp,
            Some("ru") => Dir::RightUp,
            Some("ld") => Dir::LeftDown,
            Some("rd") => Dir::RightDown,
            _ => default,
        }
    }

    /// Enhetsvektorn för rörelsen (y nedåt).
    pub fn vector(self) -> (f32, f32) {
        match self {
            Dir::Left => (-1.0, 0.0),
            Dir::Right => (1.0, 0.0),
            Dir::Up => (0.0, -1.0),
            Dir::Down => (0.0, 1.0),
            Dir::LeftUp => (-1.0, -1.0),
            Dir::RightUp => (1.0, -1.0),
            Dir::LeftDown => (-1.0, 1.0),
            Dir::RightDown => (1.0, 1.0),
        }
    }
}

/// En form som avslöjar (eller döljer) innehållet gradvis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reveal {
    /// Torka: kanten rör sig i riktningen (`Left` = från höger mot vänster).
    Wipe(Dir),
    /// Dela: från mitten ut (`out`) eller från kanterna in, vågrätt (`vertical` = lodrät springa).
    Split { vertical: bool, out: bool },
    Circle { out: bool },
    Diamond { out: bool },
    Plus { out: bool },
    /// Ruta (`box`): rektangel från mitten.
    Box { out: bool },
    /// Jalusi: band som vidgas (`vertical` = lodräta band).
    Blinds { vertical: bool },
    /// Schackrutor som torkas i rad.
    Checker { vertical: bool },
    /// Slumpvisa staplar.
    RandomBars { vertical: bool },
    /// Upplösning i små rutor.
    Dissolve,
    /// Hjul med ekrar (medurs).
    Wheel { spokes: u32 },
    /// Kil från mitten uppåt åt båda håll.
    Wedge,
    /// Diagonal torka från ett hörn (`strips`): riktningen är kantens rörelse.
    Strips(Dir),
}

/// Övergångens slag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Direkt byte (även genom svart).
    Cut { black: bool },
    /// Toning (även genom svart).
    Fade { black: bool },
    /// Den nya bilden skjuter undan den gamla.
    Push(Dir),
    /// Den nya bilden glider in över den gamla.
    Cover(Dir),
    /// Den gamla bilden glider undan och visar den nya (`pull`, `uncover`).
    Pull(Dir),
    /// Den nya bilden avslöjas med en form.
    Reveal(Reveal),
    /// Kam: varannan remsa glider in från var sitt håll.
    Comb { vertical: bool },
    /// Zoom: den nya bilden växer fram (in) eller den gamla krymper bort (ut).
    Zoom { inward: bool },
    /// Morph: formerna paras ihop och flyttas, ändrar storlek och vrids från den gamla bildens läge till
    /// den nya (se `morph.rs`).
    Morph(MorphBy),
}

/// Vad morph paras ihop efter (`option`). Ord och tecken paras för tillfället som hela objekt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MorphBy {
    Object,
    Word,
    Char,
}

/// En bilds övergång.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    pub kind: Kind,
    /// Längd i sekunder.
    pub dur: f32,
    /// Klick (eller tangent) går vidare (`advClick`).
    pub adv_click: bool,
    /// Gå vidare automatiskt efter så många sekunder (`advTm`).
    pub adv_after: Option<f32>,
}

impl Default for Transition {
    fn default() -> Self {
        Transition { kind: Kind::Cut { black: false }, dur: 0.0, adv_click: true, adv_after: None }
    }
}

impl Transition {
    /// Övergången för en bild (`p:sld/p:transition`); bilder utan övergång byts direkt.
    pub fn of_slide(root: &Node) -> Transition {
        root.child("p:transition").map(parse).unwrap_or_default()
    }

    /// Har övergången något att visa?
    pub fn animated(&self) -> bool {
        self.dur > 0.0 && !matches!(self.kind, Kind::Cut { black: false })
    }
}

/// Tolkar `p:transition`.
pub fn parse(n: &Node) -> Transition {
    let spd = match n.attr("spd") {
        Some("slow") => 1.0,
        Some("med") => 0.75,
        _ => 0.5,
    };
    // Morph har två sekunder som standard i PowerPoint (filen skriver bara `spd`).
    let is_morph = n.children.iter().any(|c| c.name == "p159:morph");
    let spd = if is_morph && n.attr("spd") != Some("fast") && n.attr("spd") != Some("med") { 2.0 } else { spd };
    let dur = n.attr_f64("p14:dur").map(|ms| (ms / 1000.0) as f32).unwrap_or(spd).clamp(0.0, 60.0);
    let adv_click = n.attr_bool("advClick").unwrap_or(true);
    let adv_after = n.attr_f64("advTm").map(|ms| (ms / 1000.0) as f32).filter(|t| t.is_finite() && *t >= 0.0);
    let el = n.children.iter().find(|c| !matches!(c.name.as_str(), "p:sndAc" | "p:extLst"));
    let kind = match el {
        Some(e) => kind_of(e),
        None => Kind::Cut { black: false },
    };
    let dur = if kind == (Kind::Cut { black: false }) { 0.0 } else { dur };
    Transition { kind, dur, adv_click, adv_after }
}

fn orient_vertical(e: &Node, attr: &str) -> bool {
    e.attr(attr) == Some("vert")
}

fn kind_of(e: &Node) -> Kind {
    let local = e.name.split_once(':').map_or(e.name.as_str(), |(_, l)| l);
    let thru_black = e.attr_bool("thruBlk").unwrap_or(false);
    let dir = |d: Dir| Dir::parse(e.attr("dir"), d);
    let out = e.attr("dir") != Some("in");
    match local {
        "cut" => Kind::Cut { black: thru_black },
        "fade" => Kind::Fade { black: thru_black },
        "push" | "pan" => Kind::Push(dir(Dir::Left)),
        "cover" => Kind::Cover(dir(Dir::Left)),
        "pull" | "uncover" => Kind::Pull(dir(Dir::Left)),
        "wipe" => Kind::Reveal(Reveal::Wipe(dir(Dir::Left))),
        "split" | "doors" | "window" => {
            let vertical = match local {
                "split" => orient_vertical(e, "orient"),
                // Dörrar öppnas på mitten (lodrät springa) om inget annat sägs.
                _ => e.attr("dir") != Some("horz"),
            };
            Kind::Reveal(Reveal::Split { vertical, out: local != "split" || out })
        }
        "circle" => Kind::Reveal(Reveal::Circle { out: true }),
        "diamond" => Kind::Reveal(Reveal::Diamond { out: true }),
        "plus" => Kind::Reveal(Reveal::Plus { out: true }),
        "zoom" => Kind::Zoom { inward: e.attr("dir") != Some("out") },
        "dissolve" => Kind::Reveal(Reveal::Dissolve),
        "checker" => Kind::Reveal(Reveal::Checker { vertical: orient_vertical(e, "dir") }),
        "blinds" => Kind::Reveal(Reveal::Blinds { vertical: orient_vertical(e, "dir") }),
        "randomBar" => Kind::Reveal(Reveal::RandomBars { vertical: orient_vertical(e, "dir") }),
        "comb" => Kind::Comb { vertical: orient_vertical(e, "dir") },
        "strips" => Kind::Reveal(Reveal::Strips(dir(Dir::LeftDown))),
        "wheel" | "wheelReverse" => Kind::Reveal(Reveal::Wheel { spokes: e.attr_i64("spokes").unwrap_or(4).clamp(1, 16) as u32 }),
        "wedge" => Kind::Reveal(Reveal::Wedge),
        "newsflash" => Kind::Zoom { inward: true },
        "morph" => Kind::Morph(match e.attr("option") {
            Some("byWord") => MorphBy::Word,
            Some("byChar") => MorphBy::Char,
            _ => MorphBy::Object,
        }),
        // p14: närmaste enkla.
        "reveal" => Kind::Fade { black: true },
        "conveyor" | "ferris" | "gallery" => Kind::Push(dir(Dir::Left)),
        "random" | "flash" | "vortex" | "switch" | "flip" | "ripple" | "honeycomb" | "prism" | "glitter" | "warp" | "flythrough" | "shred" => {
            Kind::Fade { black: false }
        }
        _ => Kind::Fade { black: false },
    }
}

// ------------------------------------------------------------ geometri

/// Vilken bild en bit visar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Old,
    New,
    /// Svart yta (toning genom svart).
    Black,
}

/// Affin avbildning i enhetskoordinater: (x, y) → (a·x + c·y + e, b·x + d·y + f).
pub type Map = [f32; 6];

pub const IDENTITY: Map = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

pub fn apply(m: &Map, p: [f32; 2]) -> [f32; 2] {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

fn translate(dx: f32, dy: f32) -> Map {
    [1.0, 0.0, 0.0, 1.0, dx, dy]
}

/// En bit av en bildruta: polygonen (konvex) i lagrets texturkoordinater 0..1, avbildad med `map` till
/// målets enhetskoordinater (bilden = 0..1), med genomskinligheten `alpha`.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub layer: Layer,
    pub poly: Vec<[f32; 2]>,
    pub map: Map,
    pub alpha: f32,
}

fn unit() -> Vec<[f32; 2]> {
    vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn full(layer: Layer, alpha: f32) -> Piece {
    Piece { layer, poly: unit(), map: IDENTITY, alpha }
}

/// Mjuk början och slut (som PowerPoints standardkurva för övergångar).
pub fn ease(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

/// Bitarna för övergången vid `p` (0..1); `aspect` = bildens bredd/höjd (för cirklar och romber).
pub fn pieces(kind: Kind, p: f32, aspect: f32) -> Vec<Piece> {
    let p = p.clamp(0.0, 1.0);
    if p >= 1.0 {
        return vec![full(Layer::New, 1.0)];
    }
    match kind {
        Kind::Cut { black: false } => vec![full(Layer::New, 1.0)],
        Kind::Cut { black: true } => {
            if p < 0.5 { vec![full(Layer::Black, 1.0)] } else { vec![full(Layer::New, 1.0)] }
        }
        Kind::Fade { black: false } | Kind::Morph(_) => vec![full(Layer::Old, 1.0), full(Layer::New, p)],
        Kind::Fade { black: true } => {
            if p < 0.5 {
                vec![full(Layer::Black, 1.0), full(Layer::Old, 1.0 - p * 2.0)]
            } else {
                vec![full(Layer::Black, 1.0), full(Layer::New, p * 2.0 - 1.0)]
            }
        }
        Kind::Push(d) => {
            let e = ease(p);
            let (vx, vy) = axis(d);
            vec![
                Piece { layer: Layer::Old, poly: unit(), map: translate(vx * e, vy * e), alpha: 1.0 },
                Piece { layer: Layer::New, poly: unit(), map: translate(vx * (e - 1.0), vy * (e - 1.0)), alpha: 1.0 },
            ]
        }
        Kind::Cover(d) => {
            let e = ease(p);
            let (vx, vy) = d.vector();
            vec![full(Layer::Old, 1.0), Piece { layer: Layer::New, poly: unit(), map: translate(vx * (e - 1.0), vy * (e - 1.0)), alpha: 1.0 }]
        }
        Kind::Pull(d) => {
            let e = ease(p);
            let (vx, vy) = d.vector();
            vec![full(Layer::New, 1.0), Piece { layer: Layer::Old, poly: unit(), map: translate(vx * e, vy * e), alpha: 1.0 }]
        }
        Kind::Comb { vertical } => {
            let n = 8;
            let e = ease(p);
            let mut out = vec![full(Layer::Old, 1.0)];
            for i in 0..n {
                let (a, b) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                let s = if i % 2 == 0 { -1.0 } else { 1.0 } * (1.0 - e);
                let (poly, map) = if vertical { (rect(a, 0.0, b, 1.0), translate(0.0, s)) } else { (rect(0.0, a, 1.0, b), translate(s, 0.0)) };
                out.push(Piece { layer: Layer::New, poly, map, alpha: 1.0 });
            }
            out
        }
        Kind::Zoom { inward: true } => {
            let s = ease(p).max(0.001);
            let map = [s, 0.0, 0.0, s, 0.5 - 0.5 * s, 0.5 - 0.5 * s];
            vec![full(Layer::Old, 1.0), Piece { layer: Layer::New, poly: unit(), map, alpha: p.min(1.0) }]
        }
        Kind::Zoom { inward: false } => {
            let s = (1.0 - ease(p)).max(0.001);
            let map = [s, 0.0, 0.0, s, 0.5 - 0.5 * s, 0.5 - 0.5 * s];
            vec![full(Layer::New, 1.0), Piece { layer: Layer::Old, poly: unit(), map, alpha: 1.0 - p }]
        }
        Kind::Reveal(r) => {
            let mut out = vec![full(Layer::Old, 1.0)];
            out.extend(reveal(r, p, aspect).into_iter().map(|poly| Piece { layer: Layer::New, poly, map: IDENTITY, alpha: 1.0 }));
            out
        }
    }
}

/// Enhetsvektorn längs en huvudaxel (push har bara fyra riktningar).
fn axis(d: Dir) -> (f32, f32) {
    match d {
        Dir::Left | Dir::LeftUp | Dir::LeftDown => (-1.0, 0.0),
        Dir::Right | Dir::RightUp | Dir::RightDown => (1.0, 0.0),
        Dir::Up => (0.0, -1.0),
        Dir::Down => (0.0, 1.0),
    }
}

/// Den synliga delen vid `p` (0 = inget, 1 = allt) som konvexa polygoner i 0..1 (utan överlapp).
pub fn reveal(r: Reveal, p: f32, aspect: f32) -> Vec<Vec<[f32; 2]>> {
    let p = p.clamp(0.0, 1.0);
    if p <= 0.0 {
        return Vec::new();
    }
    if p >= 1.0 {
        return vec![unit()];
    }
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
    let polys: Vec<Vec<[f32; 2]>> = match r {
        Reveal::Wipe(d) => match axis(d) {
            (x, _) if x < 0.0 => vec![rect(1.0 - p, 0.0, 1.0, 1.0)],
            (x, _) if x > 0.0 => vec![rect(0.0, 0.0, p, 1.0)],
            (_, y) if y < 0.0 => vec![rect(0.0, 1.0 - p, 1.0, 1.0)],
            _ => vec![rect(0.0, 0.0, 1.0, p)],
        },
        Reveal::Split { vertical, out } => {
            let h = p / 2.0;
            match (vertical, out) {
                (true, true) => vec![rect(0.5 - h, 0.0, 0.5 + h, 1.0)],
                (false, true) => vec![rect(0.0, 0.5 - h, 1.0, 0.5 + h)],
                (true, false) => vec![rect(0.0, 0.0, h, 1.0), rect(1.0 - h, 0.0, 1.0, 1.0)],
                (false, false) => vec![rect(0.0, 0.0, 1.0, h), rect(0.0, 1.0 - h, 1.0, 1.0)],
            }
        }
        Reveal::Box { out } => {
            if out {
                let h = p / 2.0;
                vec![rect(0.5 - h, 0.5 - h, 0.5 + h, 0.5 + h)]
            } else {
                frame_ring(p / 2.0)
            }
        }
        Reveal::Circle { out } => {
            // Radien i bildens höjdenheter så att cirkeln blir rund; full storlek täcker hörnen.
            let full_r = (aspect * aspect + 1.0).sqrt() / 2.0;
            if out {
                vec![ellipse(full_r * p, aspect, 48)]
            } else {
                // Inåt: allt utom en krympande cirkel (approximeras med en ring av fyrhörningar).
                ring(full_r * (1.0 - p), full_r, aspect, 48)
            }
        }
        Reveal::Diamond { out } => {
            let s = p * 1.0;
            if out {
                vec![vec![[0.5, 0.5 - s], [0.5 + s, 0.5], [0.5, 0.5 + s], [0.5 - s, 0.5]]]
            } else {
                diamond_ring(1.0 - p)
            }
        }
        Reveal::Plus { out } => {
            let h = p / 2.0;
            if out {
                vec![rect(0.5 - h, 0.0, 0.5 + h, 1.0), rect(0.0, 0.5 - h, 0.5 - h, 0.5 + h), rect(0.5 + h, 0.5 - h, 1.0, 0.5 + h)]
            } else {
                // Hörnen växer in mot mitten.
                vec![rect(0.0, 0.0, h, h), rect(1.0 - h, 0.0, 1.0, h), rect(0.0, 1.0 - h, h, 1.0), rect(1.0 - h, 1.0 - h, 1.0, 1.0)]
            }
        }
        Reveal::Blinds { vertical } => {
            let n = 6;
            (0..n)
                .map(|i| {
                    let a = i as f32 / n as f32;
                    let b = a + p / n as f32;
                    if vertical { rect(a, 0.0, b, 1.0) } else { rect(0.0, a, 1.0, b) }
                })
                .collect()
        }
        Reveal::Checker { vertical } => {
            // Rutor 10×8 som torkas var för sig; varannan rad börjar en halv ruta senare.
            let (cols, rows) = if vertical { (8usize, 10usize) } else { (10usize, 8usize) };
            let (cw, ch) = (1.0 / cols as f32, 1.0 / rows as f32);
            let q = p * 2.0;
            let mut v = Vec::new();
            for r in 0..rows {
                let off = if r % 2 == 1 { 0.5 } else { 0.0 };
                let shown = (q - off).clamp(0.0, 1.0) * cw;
                if shown <= 0.0 {
                    continue;
                }
                for c in 0..cols {
                    let x0 = c as f32 * cw;
                    let cell = rect(x0, r as f32 * ch, x0 + shown, (r + 1) as f32 * ch);
                    // Lodrätt: samma mönster speglat i diagonalen.
                    v.push(if vertical { cell.into_iter().rev().map(|[x, y]| [y, x]).collect() } else { cell });
                }
            }
            v
        }
        Reveal::RandomBars { vertical } => {
            let n = 64usize;
            (0..n)
                .filter(|&i| hash01(i as u32) < p)
                .map(|i| {
                    let (a, b) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                    if vertical { rect(a, 0.0, b, 1.0) } else { rect(0.0, a, 1.0, b) }
                })
                .collect()
        }
        Reveal::Dissolve => {
            let (cols, rows) = (40usize, 24usize);
            let mut v = Vec::new();
            for r in 0..rows {
                for c in 0..cols {
                    if hash01((r * cols + c) as u32 ^ 0x9e37) < p {
                        let (cw, ch) = (1.0 / cols as f32, 1.0 / rows as f32);
                        v.push(rect(c as f32 * cw, r as f32 * ch, (c + 1) as f32 * cw, (r + 1) as f32 * ch));
                    }
                }
            }
            v
        }
        Reveal::Wheel { spokes } => {
            let n = spokes.max(1);
            let span = std::f32::consts::TAU / n as f32;
            (0..n).flat_map(|k| sector(-std::f32::consts::FRAC_PI_2 + k as f32 * span, span * p, aspect)).collect()
        }
        Reveal::Wedge => {
            let half = std::f32::consts::PI * p;
            let top = -std::f32::consts::FRAC_PI_2;
            let mut v = sector(top - half, half, aspect);
            v.extend(sector(top, half, aspect));
            v
        }
        Reveal::Strips(d) => {
            // Diagonal kant som rör sig i riktningen; kanten går från hörn till hörn på två gånger vägen.
            let (vx, vy) = d.vector();
            let (vx, vy) = (if vx == 0.0 { 1.0 } else { vx }, if vy == 0.0 { 1.0 } else { vy });
            // Halvplanet där (x', y')·v ≤ t, med x' och y' räknade från starthörnet.
            let t = p * 2.0;
            let sx = if vx > 0.0 { 0.0 } else { 1.0 };
            let sy = if vy > 0.0 { 0.0 } else { 1.0 };
            let pts = [[sx, sy], [sx + vx.signum() * t, sy], [sx, sy + vy.signum() * t]];
            vec![pts.to_vec()]
        }
    };
    polys.into_iter().map(|p| clip_unit(&p)).filter(|p| p.len() >= 3).collect()
}

/// En ellips runt mitten med radien `r` i höjdenheter.
fn ellipse(r: f32, aspect: f32, n: usize) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [0.5 + r * a.cos() / aspect, 0.5 + r * a.sin()]
        })
        .collect()
}

/// Ringen mellan radierna `r0` och `r1` som fyrhörningar.
fn ring(r0: f32, r1: f32, aspect: f32, n: usize) -> Vec<Vec<[f32; 2]>> {
    let p = |r: f32, a: f32| [0.5 + r * a.cos() / aspect, 0.5 + r * a.sin()];
    (0..n)
        .map(|i| {
            let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            vec![p(r0, a0), p(r1, a0), p(r1, a1), p(r0, a1)]
        })
        .collect()
}

/// Allt utom en kvadrat med halva sidan `0.5 - h` i mitten: fyra band.
fn frame_ring(h: f32) -> Vec<Vec<[f32; 2]>> {
    let i = 0.5 - h;
    vec![rect(0.0, 0.0, 1.0, h), rect(0.0, 1.0 - h, 1.0, 1.0), rect(0.0, h, h, 1.0 - h), rect(1.0 - h, h, 1.0, 1.0 - h)]
        .into_iter()
        .filter(|_| i >= 0.0)
        .collect()
}

/// Allt utanför romben |x − ½| + |y − ½| ≤ `s`: varje kvadrant av bilden klippt med halvplanet utanför.
fn diamond_ring(s: f32) -> Vec<Vec<[f32; 2]>> {
    let mut out = Vec::new();
    for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let x0 = if sx < 0.0 { 0.0 } else { 0.5 };
        let y0 = if sy < 0.0 { 0.0 } else { 0.5 };
        let q = rect(x0, y0, x0 + 0.5, y0 + 0.5);
        // Behåll sx·(x − ½) + sy·(y − ½) ≥ s.
        let kept = clip_halfplane(&q, sx, sy, s + 0.5 * sx + 0.5 * sy);
        if kept.len() >= 3 {
            out.push(kept);
        }
    }
    out
}

/// Klipper en konvex polygon till halvplanet a·x + b·y ≥ c.
fn clip_halfplane(poly: &[[f32; 2]], a: f32, b: f32, c: f32) -> Vec<[f32; 2]> {
    let f = |p: &[f32; 2]| a * p[0] + b * p[1] - c;
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let cur = poly[i];
        let prev = poly[(i + poly.len() - 1) % poly.len()];
        let (fc, fp) = (f(&cur), f(&prev));
        if (fc >= 0.0) != (fp >= 0.0) {
            let t = fp / (fp - fc);
            out.push([prev[0] + (cur[0] - prev[0]) * t, prev[1] + (cur[1] - prev[1]) * t]);
        }
        if fc >= 0.0 {
            out.push(cur);
        }
    }
    out
}

/// En cirkelsektor från vinkeln `a0` med vidden `span` (radianer, medurs med y nedåt), delad i bitar
/// på högst 90° så att varje bit är konvex. Radien når hörnen.
fn sector(a0: f32, span: f32, aspect: f32) -> Vec<Vec<[f32; 2]>> {
    if span <= 0.0 {
        return Vec::new();
    }
    let r = (aspect * aspect + 1.0).sqrt();
    let parts = (span / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = span / parts as f32;
    (0..parts)
        .map(|k| {
            let mut poly = vec![[0.5, 0.5]];
            let sub = 6;
            for j in 0..=sub {
                let a = a0 + step * (k as f32 + j as f32 / sub as f32);
                poly.push([0.5 + r * a.cos() / aspect, 0.5 + r * a.sin()]);
            }
            poly
        })
        .collect()
}

/// Klipper en konvex polygon till enhetskvadraten (Sutherland–Hodgman).
pub fn clip_unit(poly: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = poly.to_vec();
    // (axel, gräns, behåll mindre än)
    for (axis, edge, keep_less) in [(0usize, 0.0f32, false), (0, 1.0, true), (1, 0.0, false), (1, 1.0, true)] {
        if out.is_empty() {
            break;
        }
        let inside = |p: &[f32; 2]| if keep_less { p[axis] <= edge } else { p[axis] >= edge };
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let cur = input[i];
            let prev = input[(i + input.len() - 1) % input.len()];
            let (ci, pi) = (inside(&cur), inside(&prev));
            if ci != pi {
                let t = (edge - prev[axis]) / (cur[axis] - prev[axis]);
                out.push([prev[0] + (cur[0] - prev[0]) * t, prev[1] + (cur[1] - prev[1]) * t]);
            }
            if ci {
                out.push(cur);
            }
        }
    }
    out
}

/// Ett deterministiskt "slumptal" i 0..1 (samma mönster varje gång).
fn hash01(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Polygonens yta (för prov).
#[cfg(test)]
fn area(p: &[[f32; 2]]) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (x0, y0) = (p[i][0], p[i][1]);
        let (x1, y1) = (p[(i + 1) % p.len()][0], p[(i + 1) % p.len()][1]);
        a += x0 * y1 - x1 * y0;
    }
    (a / 2.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(xml: &str) -> Transition {
        let x = format!(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" xmlns:p159="http://schemas.microsoft.com/office/powerpoint/2015/09/main">{xml}</p:sld>"#
        );
        let root = super::super::xml::parse(x.as_bytes()).unwrap();
        assert_eq!(root.name, "p:sld");
        Transition::of_slide(&root)
    }

    #[test]
    fn tolkar_övergångar_och_tider() {
        assert_eq!(tr("").kind, Kind::Cut { black: false });
        assert!(!tr("").animated());
        let t = tr(r#"<p:transition spd="slow"><p:fade thruBlk="1"/></p:transition>"#);
        assert_eq!(t.kind, Kind::Fade { black: true });
        assert_eq!(t.dur, 1.0);
        assert!(t.adv_click && t.adv_after.is_none());
        let t = tr(r#"<p:transition advClick="0" advTm="3000"><p:push dir="u"/></p:transition>"#);
        assert_eq!(t.kind, Kind::Push(Dir::Up));
        assert_eq!(t.dur, 0.5);
        assert!(!t.adv_click);
        assert_eq!(t.adv_after, Some(3.0));
        // p14 väljs ur mc:AlternateContent och p14:dur gäller.
        let t = tr(r#"<mc:AlternateContent><mc:Choice Requires="p14"><p:transition p14:dur="400"><p14:reveal/></p:transition></mc:Choice><mc:Fallback><p:transition><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>"#);
        assert_eq!(t.kind, Kind::Fade { black: true });
        assert!((t.dur - 0.4).abs() < 1e-6);
        // Morph väljs ur mc:AlternateContent (reserven är en toning); två sekunder om inget annat sägs.
        let t = tr(r#"<mc:AlternateContent><mc:Choice Requires="p159"><p:transition spd="slow"><p159:morph option="byObject"/></p:transition></mc:Choice><mc:Fallback><p:transition spd="slow"><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>"#);
        assert_eq!(t.kind, Kind::Morph(MorphBy::Object));
        assert_eq!(t.dur, 2.0);
        let t = tr(r#"<mc:AlternateContent><mc:Choice Requires="p159"><p:transition p14:dur="750"><p159:morph option="byWord"/></p:transition></mc:Choice><mc:Fallback><p:transition><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>"#);
        assert_eq!(t.kind, Kind::Morph(MorphBy::Word));
        assert!((t.dur - 0.75).abs() < 1e-6);
        let t = tr(r#"<mc:AlternateContent><mc:Choice Requires="p14"><p:transition p14:dur="1200"><p14:vortex dir="r"/></p:transition></mc:Choice><mc:Fallback><p:transition><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>"#);
        assert_eq!(t.kind, Kind::Fade { black: false });
        assert!((t.dur - 1.2).abs() < 1e-6);
        assert_eq!(tr(r#"<p:transition><p:wipe dir="d"/></p:transition>"#).kind, Kind::Reveal(Reveal::Wipe(Dir::Down)));
        assert_eq!(tr(r#"<p:transition><p:split orient="vert" dir="in"/></p:transition>"#).kind, Kind::Reveal(Reveal::Split { vertical: true, out: false }));
        assert_eq!(tr(r#"<p:transition><p:cover dir="rd"/></p:transition>"#).kind, Kind::Cover(Dir::RightDown));
        assert_eq!(tr(r#"<p:transition><p:pull/></p:transition>"#).kind, Kind::Pull(Dir::Left));
        assert_eq!(tr(r#"<p:transition><p:randomBar dir="vert"/></p:transition>"#).kind, Kind::Reveal(Reveal::RandomBars { vertical: true }));
        assert_eq!(tr(r#"<p:transition><p:zoom dir="out"/></p:transition>"#).kind, Kind::Zoom { inward: false });
        assert_eq!(tr(r#"<p:transition><p:wheel spokes="2"/></p:transition>"#).kind, Kind::Reveal(Reveal::Wheel { spokes: 2 }));
        // Direkt byte har ingen längd, men genom svart har det.
        assert_eq!(tr(r#"<p:transition><p:cut/></p:transition>"#).dur, 0.0);
        assert!(tr(r#"<p:transition><p:cut thruBlk="1"/></p:transition>"#).animated());
        // En övergång med bara ljud: direkt byte.
        assert_eq!(tr(r#"<p:transition><p:sndAc><p:stSnd><p:snd/></p:stSnd></p:sndAc></p:transition>"#).kind, Kind::Cut { black: false });
    }

    #[test]
    fn avslöjningen_växer_från_inget_till_allt() {
        let all = [
            Reveal::Wipe(Dir::Left),
            Reveal::Wipe(Dir::Down),
            Reveal::Split { vertical: true, out: true },
            Reveal::Split { vertical: false, out: false },
            Reveal::Circle { out: true },
            Reveal::Circle { out: false },
            Reveal::Diamond { out: true },
            Reveal::Diamond { out: false },
            Reveal::Plus { out: true },
            Reveal::Plus { out: false },
            Reveal::Box { out: true },
            Reveal::Box { out: false },
            Reveal::Blinds { vertical: false },
            Reveal::Checker { vertical: false },
            Reveal::Checker { vertical: true },
            Reveal::RandomBars { vertical: true },
            Reveal::Dissolve,
            Reveal::Wheel { spokes: 4 },
            Reveal::Wedge,
            Reveal::Strips(Dir::RightDown),
            Reveal::Strips(Dir::LeftUp),
        ];
        for r in all {
            let a = |p: f32| reveal(r, p, 16.0 / 9.0).iter().map(|q| area(q)).sum::<f32>();
            assert_eq!(a(0.0), 0.0, "{r:?}");
            assert!((a(1.0) - 1.0).abs() < 1e-4, "{r:?}");
            let (q1, q2, q3) = (a(0.25), a(0.5), a(0.75));
            assert!(q1 <= q2 + 1e-3 && q2 <= q3 + 1e-3, "{r:?}: {q1} {q2} {q3}");
            assert!(q2 > 0.05 && q2 < 0.95, "{r:?}: halvvägs {q2}");
            for q in reveal(r, 0.5, 16.0 / 9.0) {
                assert!(q.iter().all(|p| (-1e-4..=1.0001).contains(&p[0]) && (-1e-4..=1.0001).contains(&p[1])), "{r:?} utanför bilden");
            }
        }
        // Torka åt vänster: den högra delen syns först.
        let w = reveal(Reveal::Wipe(Dir::Left), 0.25, 1.0);
        assert!(w[0].iter().all(|p| p[0] >= 0.75 - 1e-6));
    }

    #[test]
    fn bitar_för_toning_push_och_slut() {
        let f = pieces(Kind::Fade { black: false }, 0.25, 1.0);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].layer, f[0].alpha), (Layer::Old, 1.0));
        assert_eq!((f[1].layer, f[1].alpha), (Layer::New, 0.25));
        let b = pieces(Kind::Fade { black: true }, 0.25, 1.0);
        assert_eq!(b[0].layer, Layer::Black);
        assert_eq!((b[1].layer, b[1].alpha), (Layer::Old, 0.5));
        let b = pieces(Kind::Fade { black: true }, 0.75, 1.0);
        assert_eq!((b[1].layer, b[1].alpha), (Layer::New, 0.5));
        // Push uppåt: halvvägs ligger den nya bildens överkant mitt på skärmen.
        let p = pieces(Kind::Push(Dir::Up), 0.5, 1.0);
        let new = p.iter().find(|x| x.layer == Layer::New).unwrap();
        assert!((apply(&new.map, [0.0, 0.0])[1] - 0.5).abs() < 1e-6);
        let old = p.iter().find(|x| x.layer == Layer::Old).unwrap();
        assert!((apply(&old.map, [0.0, 1.0])[1] - 0.5).abs() < 1e-6);
        // Klart: bara den nya bilden.
        for k in [Kind::Fade { black: false }, Kind::Push(Dir::Left), Kind::Reveal(Reveal::Dissolve), Kind::Zoom { inward: false }] {
            assert_eq!(pieces(k, 1.0, 1.0), vec![full(Layer::New, 1.0)]);
        }
    }

    #[test]
    fn klippning_till_enhetskvadraten() {
        let big = vec![[-1.0, -1.0], [2.0, -1.0], [2.0, 2.0], [-1.0, 2.0]];
        assert!((area(&clip_unit(&big)) - 1.0).abs() < 1e-6);
        let tri = vec![[0.5, 0.5], [3.0, 0.5], [0.5, 3.0]];
        assert!((area(&clip_unit(&tri)) - 0.25).abs() < 1e-6);
        assert!(clip_unit(&[[2.0, 2.0], [3.0, 2.0], [3.0, 3.0]]).is_empty());
    }
}
