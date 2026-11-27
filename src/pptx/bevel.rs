//! 3D på former (`a:sp3d`, `a:scene3d`): fasning (`bevelT`) som en skuggad kantzon innanför formens
//! kontur, kontur (`contourW`) och kameraförinställningar som inte tittar rakt framifrån (formen
//! förvrängs affint och extruderas enkelt om `extrusionH` > 0).
//!
//! **Tolkning** (`shape_3d`): `a:scene3d` och `a:sp3d` tas var för sig ur första `p:spPr` i
//! platshållarkedjan som har dem, annars ur temats effektstil (`p:style/a:effectRef` →
//! `a:effectStyleLst`). En tom `<a:sp3d/>` gör ingenting; `<a:bevelT/>` är en fasning med
//! standardmåtten 6 × 6 pt och typen `circle`. `bevelB` och `extrusionH` syns inte framifrån.
//!
//! **Fasningen** ritas i rastret (`effects.rs`): avståndet till formens kant (exakt euklidiskt
//! avståndsfält ur lagrets täckning) ger var i profilen bildpunkten ligger, profilens lutning × höjd /
//! bredd ger normalen, och normalen ger ljusheten: omgivningsljus + riktat ljus (Lambert) + glans
//! (Blinn), relativt en plan yta. Konstanterna för `threePt` är anpassade till PowerPoints rastrering i
//! PDF:en för `cppcon14-parallel-overview` bild 14 (`coolSlant` på kvadrater): fasettens ljushet uppåt
//! 1,04, nedåt 0,52, åt vänster 0,81, åt höger 0,57 → omgivning 0,29, ljuset 25° till vänster om rakt upp
//! och 61° ut från planet (uppåt/nedåt träffas exakt; åt sidorna blir det 0,91/0,68, en ljuskälla räcker
//! inte för alla fyra). Ljuset vrids med formen (`rot`, även gruppens) men inte med speglingar, som i
//! PowerPoint (kvadraterna ligger i en grupp vriden 90° med `flipV`). Den synliga kantzonen är ungefär
//! halva `w` (`ZONE`), som i PDF:en; profilernas former (`BevelType::slope`) är uppskattade ur
//! PowerPoints galleri, bara `coolSlant` och `circle` är jämförda mot PDF:er.

use super::color::{self, Rgba};
use super::inherit::Sheets;
use super::render::{emu, Frame};
use super::scene::{Affine, Effect};
use super::xml::Node;

/// Fasningens profil (`a:bevelT/@prst`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BevelType {
    Circle,
    RelaxedInset,
    Cross,
    CoolSlant,
    Angle,
    SoftRound,
    Convex,
    Slope,
    Divot,
    Riblet,
    HardEdge,
    ArtDeco,
}

impl BevelType {
    pub fn parse(s: Option<&str>) -> BevelType {
        match s.unwrap_or("circle") {
            "relaxedInset" => BevelType::RelaxedInset,
            "cross" => BevelType::Cross,
            "coolSlant" => BevelType::CoolSlant,
            "angle" => BevelType::Angle,
            "softRound" => BevelType::SoftRound,
            "convex" => BevelType::Convex,
            "slope" => BevelType::Slope,
            "divot" => BevelType::Divot,
            "riblet" => BevelType::Riblet,
            "hardEdge" => BevelType::HardEdge,
            "artDeco" => BevelType::ArtDeco,
            _ => BevelType::Circle,
        }
    }

    /// Profilens lutning dP/dt vid `t` (0 = kanten, 1 = fasningens inre slut), där P är höjden som andel
    /// av `h`. Positiv = stiger inåt.
    pub fn slope(self, t: f32) -> f32 {
        use std::f32::consts::PI;
        if !(0.0..1.0).contains(&t) {
            return 0.0;
        }
        match self {
            BevelType::Circle => {
                let u = 1.0 - t;
                (u / (1.0 - u * u).max(1e-4).sqrt()).min(6.0)
            }
            BevelType::RelaxedInset => PI / 2.0 * (PI * t).sin(),
            BevelType::Cross => {
                if t < 0.5 { 2.0 } else { -2.0 }
            }
            BevelType::CoolSlant => {
                if t < 0.45 {
                    1.0 / 0.45
                } else if t < 0.6 {
                    (0.6 - t) / 0.15 / 0.45
                } else {
                    0.0
                }
            }
            BevelType::Angle => 1.0,
            BevelType::SoftRound => PI / 2.0 * (PI * t / 2.0).cos(),
            BevelType::Convex => 2.0 * (1.0 - t),
            BevelType::Slope => {
                if t < 0.8 { 1.25 } else { 0.0 }
            }
            BevelType::Divot => -2.0 * (PI * t).cos(),
            BevelType::Riblet => 3.0 * (3.0 * PI * t).cos(),
            BevelType::HardEdge => {
                if t < 0.3 { 1.0 / 0.3 } else { 0.0 }
            }
            BevelType::ArtDeco => {
                if (t * 3.0).fract() < 0.34 { 2.9 } else { 0.0 }
            }
        }
    }

    /// Profilens höjd P(t) (summan av lutningen), för proven.
    #[cfg(test)]
    pub fn height(self, t: f32) -> f32 {
        let n = 200;
        let t = t.clamp(0.0, 1.0);
        (0..n).map(|i| self.slope((i as f32 + 0.5) / n as f32 * t)).sum::<f32>() * t / n as f32
    }
}

/// Belysningen för en fasning: ljusets riktning i bildens koordinater (x åt höger, y nedåt, z mot
/// betraktaren), omgivningsljuset och glansen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    pub light: [f32; 3],
    pub ambient: f32,
    pub specular: f32,
}

impl Lighting {
    /// Ljusheten (multiplikativ) och glansen (additiv vitt) för normalen `n` (bildens koordinater).
    pub fn shade(&self, n: [f32; 3]) -> (f32, f32) {
        let l = self.light;
        let d = 1.0 - self.ambient;
        let flat = self.ambient + d * l[2].max(0.0);
        let ndl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
        let k = (self.ambient + d * ndl) / flat.max(1e-3);
        let h = norm3([l[0], l[1], l[2] + 1.0]);
        let p = 30;
        let ndh = (n[0] * h[0] + n[1] * h[1] + n[2] * h[2]).max(0.0);
        let spec = self.specular * (ndh.powi(p) - h[2].max(0.0).powi(p)).max(0.0);
        (k, spec)
    }
}

fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if l > 1e-9 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 0.0, 1.0] }
}

/// Ljusriggens riktning: vinkeln (grader medurs från rakt upp) relativt `dir`, höjden över planet
/// (grader) och omgivningsljuset. `None` = ingen skuggning (`flat`).
fn rig(name: &str) -> Option<(f32, f32, f32)> {
    match name {
        "flat" => None,
        // Anpassat till PowerPoints PDF (se modulens beskrivning).
        // Mjukt ljus runt om: svag skuggning (vald mot cppcon14-decomposing bild 30 och 39).
        "balanced" | "soft" => Some((-25.0, 50.0, 0.8)),
        "threePt" | "twoPt" | "legacyFlat1" | "legacyFlat2" | "legacyFlat3" | "legacyFlat4" => Some((-25.0, 61.0, 0.29)),
        "harsh" | "contrasting" | "legacyHarsh1" | "legacyHarsh2" | "legacyHarsh3" | "legacyHarsh4" => Some((-25.0, 50.0, 0.15)),
        "flood" | "brightRoom" | "glow" => Some((-15.0, 70.0, 0.45)),
        "legacyNormal1" | "legacyNormal2" | "legacyNormal3" | "legacyNormal4" => Some((-45.0, 55.0, 0.25)),
        _ => Some((-25.0, 61.0, 0.29)),
    }
}

/// Glansens styrka för ett material (`prstMaterial`; standard `warmMatte`).
fn material_specular(name: Option<&str>) -> f32 {
    match name.unwrap_or("warmMatte") {
        "flat" | "matte" | "dkEdge" | "legacyMatte" => 0.0,
        "softEdge" | "powder" | "translucentPowder" | "legacyPlastic" => 0.35,
        "metal" | "softmetal" | "legacyMetal" | "plastic" | "clear" | "legacyWireframe" => 1.0,
        _ => 0.77,
    }
}

fn dir_angle(d: Option<&str>) -> f32 {
    match d.unwrap_or("t") {
        "tr" => 45.0,
        "r" => 90.0,
        "br" => 135.0,
        "b" => 180.0,
        "bl" => 225.0,
        "l" => 270.0,
        "tl" => 315.0,
        _ => 0.0,
    }
}

/// Formens vridning i bilden utan speglingar (grader medurs): ljuset följer den.
pub fn rotation_without_flips(f: &Frame) -> f32 {
    // Bilden av den lokala x-axeln (vänd om formen själv är vågrätt speglad): en lodrät spegling
    // (formens eller gruppens, som `flipV` på en vriden grupp) ändrar den inte. En vågrätt speglad grupp
    // räknas som vriden 180° (oskiljbart från en lodrät spegling plus 180°).
    let fx = if f.flip_h { -1.0 } else { 1.0 };
    (f.t.b * fx).atan2(f.t.a * fx).to_degrees()
}

/// Ljusets riktning i bildens koordinater för en rigg (vinkel medurs från upp, höjd över planet).
pub fn light_dir(azimuth: f32, elevation: f32) -> [f32; 3] {
    let (sa, ca) = azimuth.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    [sa * ce, -ca * ce, se]
}

/// Första `name` i kedjans `spPr`, annars i temats effektstil.
fn pick<'a>(chain: &[&'a Node], theme_style: Option<&'a Node>, name: &str) -> Option<&'a Node> {
    chain.iter().find_map(|s| s.child("p:spPr").and_then(|p| p.child(name))).or_else(|| theme_style.and_then(|e| e.child(name)))
}

/// Den synliga kantzonen som andel av `w`. PowerPoints fasningar ser ut att vara ungefär hälften så
/// breda som `w` (kvadraterna i cppcon14-parallel-overview bild 14: 6–8 pt synligt för `w` = 13 pt), och
/// 0,5 gav bäst mått för alla fasade bilder i materialet (0,3–1,0 prövades).
pub const ZONE: f32 = 0.5;

/// Ska temats effektstil (`effectRef`) ge 3D? Mätt för sig, se `docs/dev/pptx.md`.
pub const THEME_3D: bool = true;

/// En forms 3D-effekter (fasning, kontur, kamera). `filled` = formen har en fyllning (fasningen görs
/// inte på bara linjer); `base` = formens fyllningsfärg (för extruderingens standardfärg).
pub fn shape_3d(sheets: &Sheets<'_>, chain: &[&Node], style: Option<&Node>, f: &Frame, filled: bool, base: Option<Rgba>) -> Vec<Effect> {
    let theme_style = if THEME_3D {
        style.and_then(|s| s.child("a:effectRef")).and_then(|r| r.attr_i64("idx")).filter(|i| *i >= 1).and_then(|i| sheets.theme.effect_styles.get(i as usize - 1))
    } else {
        None
    };
    let ph = style.and_then(|s| s.child("a:effectRef")).and_then(color::first_color).and_then(|c| sheets.color(c, None));
    let scene3d = pick(chain, theme_style, "a:scene3d");
    let sp3d = pick(chain, theme_style, "a:sp3d");
    effects_for(sheets, scene3d, sp3d, f, filled, base, ph)
}

/// Lägger formens 3D-effekter till `effects` (kroken i `render.rs`): `items` är formens ritade element
/// (fyllningen avgör om det finns något att fasa och ger extruderingens standardfärg).
pub fn with_3d(sheets: &Sheets<'_>, mut effects: Vec<Effect>, chain: &[&Node], style: Option<&Node>, f: &Frame, items: &[super::scene::Item]) -> Vec<Effect> {
    use super::scene::{Item, Paint};
    let filled = items.iter().any(|i| matches!(i, Item::Fill { .. } | Item::Image { .. } | Item::Svg { .. }));
    let base = items.iter().find_map(|i| if let Item::Fill { paint, .. } = i { Some(match paint { Paint::Solid(c) => *c, p => p.average() }) } else { None });
    effects.extend(shape_3d(sheets, chain, style, f, filled, base));
    effects
}

/// 3D-effekterna ur `a:scene3d` och `a:sp3d` (ena eller båda kan saknas).
pub fn effects_for(sheets: &Sheets<'_>, scene3d: Option<&Node>, sp3d: Option<&Node>, f: &Frame, filled: bool, base: Option<Rgba>, ph: Option<Rgba>) -> Vec<Effect> {
    let mut out = Vec::new();
    let Some(sp3d) = sp3d else { return out };
    let col = |n: &Node| color::first_color(n).and_then(|c| sheets.color(c, ph));
    // Kameran (bara annat än rakt framifrån gör något).
    let cam = scene3d.and_then(|s| s.child("a:camera"));
    let cam_name = cam.and_then(|c| c.attr("prst")).unwrap_or("orthographicFront");
    let cam_rot = cam.and_then(|c| c.child("a:rot"));
    // Ljuset.
    let lr = scene3d.and_then(|s| s.child("a:lightRig"));
    let rig_name = lr.and_then(|l| l.attr("rig")).unwrap_or("threePt");
    let rev = lr.and_then(|l| l.child("a:rot")).and_then(|r| r.attr_i64("rev")).unwrap_or(0) as f32 / 60000.0;
    let lighting = rig(rig_name).map(|(az, el, amb)| {
        // `rev` vrider riggen (threePt i SmartArt-pilarna i cppcon14-parallel-overview bild 14, 125°:
        // ljuset från sidan som i PDF:en). De jämna riggarna (`balanced` …) har ljus runt om och vrids
        // inte märkbart (en 145° vriden `balanced` i cppcon14-decomposing bild 39 är ljus upptill).
        let rev = if matches!(rig_name, "balanced" | "soft" | "flood" | "brightRoom" | "glow") { 0.0 } else { rev };
        let azimuth = dir_angle(lr.and_then(|l| l.attr("dir"))) + az + rev + rotation_without_flips(f);
        Lighting { light: light_dir(azimuth, el), ambient: amb, specular: material_specular(sp3d.attr("prstMaterial")) }
    });
    // Kontur.
    let cw = sp3d.attr_i64("contourW").map_or(0.0, emu);
    if cw > 0.0 {
        if let Some(c) = sp3d.child("a:contourClr").and_then(col) {
            out.push(Effect::Contour { width: cw, color: c });
        }
    }
    // Fasningen ovanpå.
    if let (Some(bt), Some(lighting), true) = (sp3d.child("a:bevelT"), lighting, filled) {
        let w = bt.attr_i64("w").map_or(6.0, emu) * ZONE;
        let h = bt.attr_i64("h").map_or(6.0, emu);
        if w > 0.05 && h > 0.0 {
            out.push(Effect::Bevel { width: w, height: h, kind: BevelType::parse(bt.attr("prst")), lighting });
        }
    }
    // Kameran och extruderingen.
    if let Some(t) = camera_affine(cam_name, cam_rot, f) {
        let ext = sp3d.attr_i64("extrusionH").map_or(0.0, emu);
        let (dx, dy) = camera_depth(cam_name, cam_rot);
        let color = sp3d.child("a:extrusionClr").and_then(col).or_else(|| base.map(|c| Rgba { r: c.r * 0.6, g: c.g * 0.6, b: c.b * 0.6, a: c.a })).unwrap_or(Rgba { r: 0.4, g: 0.4, b: 0.4, a: 1.0 });
        out.push(Effect::Camera { t, extrude: (dx * ext, dy * ext), color });
    }
    out
}

/// Kamerans vinklar (latitud, longitud, varv) i grader för en förinställning, eller `None` för de som
/// tittar rakt framifrån. Ungefärliga: isometriska 35°/45°, perspektiv 20–30°, sneda utan vridning.
fn camera_angles(name: &str, rot: Option<&Node>) -> Option<(f32, f32, f32)> {
    if let Some(r) = rot {
        let g = |k: &str| r.attr_i64(k).unwrap_or(0) as f32 / 60000.0;
        let (lat, lon, rev) = (g("lat"), g("lon"), g("rev"));
        if lat != 0.0 || lon != 0.0 || rev != 0.0 {
            return Some((lat, lon, rev));
        }
    }
    if matches!(name, "orthographicFront" | "perspectiveFront" | "legacyObliqueFront" | "legacyPerspectiveFront") || name.starts_with("oblique") {
        return None;
    }
    let has = |k: &str| name.contains(k);
    let a = if name.starts_with("isometric") { (35.26, 45.0) } else { (20.0, 25.0) };
    let lon = if has("Left") { a.1 } else if has("Right") { -a.1 } else { 0.0 };
    let lat = if has("Top") || has("Above") || has("Heroic") && !has("Below") { a.0 } else if has("Bottom") || has("Below") { -a.0 } else { 0.0 };
    let rev = if has("LeftUp") || has("RightDown") { -30.0 } else if has("LeftDown") || has("RightUp") { 30.0 } else { 0.0 };
    if lat == 0.0 && lon == 0.0 && rev == 0.0 {
        return None;
    }
    Some((lat, lon, rev))
}

/// Förvrängningen av formen för en kamera (i bildens koordinater, runt formens mitt).
pub fn camera_affine(name: &str, rot: Option<&Node>, f: &Frame) -> Option<Affine> {
    let (lat, lon, rev) = camera_angles(name, rot)?;
    let (sl, cl) = lon.to_radians().sin_cos();
    let (sa, ca) = lat.to_radians().sin_cos();
    // Planet vrids lon runt den lodräta axeln och lat runt den vågräta; parallellprojektion.
    // x' = x cos lon, y' = y cos lat + x sin lon sin lat.
    let lin = Affine { a: cl, b: sl * sa, c: 0.0, d: ca, e: 0.0, f: 0.0 }.then(&Affine::rotate(rev));
    let (cx, cy) = f.t.apply(f.w / 2.0, f.h / 2.0);
    Some(Affine::translate(-cx, -cy).then(&lin).then(&Affine::translate(cx, cy)))
}

/// Riktningen (per punkt extrudering) som djupet får på skärmen för en kamera.
pub fn camera_depth(name: &str, rot: Option<&Node>) -> (f32, f32) {
    if name.starts_with("oblique") {
        let has = |k: &str| name.contains(k);
        let x = if has("Left") { -0.5 } else if has("Right") { 0.5 } else { 0.0 };
        let y = if has("Top") { -0.5 } else if has("Bottom") { 0.5 } else { 0.0 };
        return (x, y);
    }
    match camera_angles(name, rot) {
        Some((lat, lon, _)) => {
            // Djupaxeln (bakåt) efter vridningen.
            let (sl, cl) = lon.to_radians().sin_cos();
            let (sa, _) = lat.to_radians().sin_cos();
            (sl, -sa * cl)
        }
        None => (0.0, 0.0),
    }
}

// ---------------------------------------------------------------------------------------------
// Rastret

use resvg::tiny_skia as sk;

/// Kvadrerat euklidiskt avståndsfält i en led (Felzenszwalb–Huttenlocher): `f` är 0 vid källorna och
/// stort annars; skrivs över med avståndet² till närmaste källa.
fn edt_1d(f: &mut [f64], v: &mut [usize], z: &mut [f64], out: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let sect = |f: &[f64], q: usize, p: usize| ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64));
    for q in 1..n {
        let mut s = sect(f, q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = sect(f, q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let d = q as f64 - v[k] as f64;
        out[q] = d * d + f[v[k]];
    }
    f.copy_from_slice(&out[..n]);
}

/// Avståndet (bildpunkter) från varje bildpunkt där `inside` är sant till närmaste punkt där den inte
/// är det (0 utanför). Exakt euklidiskt, O(bildpunkter).
pub fn distance_field(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    const BIG: f64 = 1e12;
    let mut d: Vec<f64> = inside.iter().map(|&i| if i { BIG } else { 0.0 }).collect();
    let n = w.max(h);
    let (mut v, mut z, mut out, mut col) = (vec![0usize; n + 1], vec![0.0f64; n + 2], vec![0.0f64; n], vec![0.0f64; h]);
    for x in 0..w {
        for y in 0..h {
            col[y] = d[y * w + x];
        }
        edt_1d(&mut col, &mut v, &mut z, &mut out);
        for y in 0..h {
            d[y * w + x] = col[y];
        }
    }
    for y in 0..h {
        edt_1d(&mut d[y * w..(y + 1) * w], &mut v, &mut z, &mut out);
    }
    d.iter().map(|v| v.min(BIG).sqrt() as f32).collect()
}

/// Skuggar lagret (förmultiplicerat) med fasningen: `width`/`height` i bildpunkter. `zone_only` gör
/// bildpunkterna utanför kantzonen genomskinliga (PDF:ens överlägg).
fn shade(l: &mut sk::Pixmap, width: f32, height: f32, kind: BevelType, light: &Lighting, zone_only: bool) {
    let (w, h) = (l.width() as usize, l.height() as usize);
    if w < 3 || h < 3 || width < 0.5 {
        return;
    }
    let inside: Vec<bool> = l.data().chunks_exact(4).map(|p| p[3] >= 128).collect();
    // Avståndet från bildpunktens mitt till kanten (en halv bildpunkt kortare än till närmaste utanför).
    let dist: Vec<f32> = distance_field(&inside, w, h).into_iter().map(|d| (d - 0.5).max(0.0)).collect();
    let ratio = height / width;
    let data = l.data_mut();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let a = data[i * 4 + 3];
            if a == 0 {
                continue;
            }
            let d = dist[i];
            if d >= width {
                if zone_only {
                    data[i * 4..i * 4 + 4].fill(0);
                }
                continue;
            }
            // Avståndsfältets lutning (inåt), centrala differenser.
            let g = |xx: usize, yy: usize| dist[yy * w + xx];
            let gx = g((x + 1).min(w - 1), y) - g(x.saturating_sub(1), y);
            let gy = g(x, (y + 1).min(h - 1)) - g(x, y.saturating_sub(1));
            let gl = (gx * gx + gy * gy).sqrt();
            let (gx, gy) = if gl > 0.2 { (gx / gl, gy / gl) } else { (0.0, 0.0) };
            let s = ratio * kind.slope(d / width);
            let n = norm3([-s * gx, -s * gy, 1.0]);
            let (k, spec) = light.shade(n);
            let af = a as f32;
            for c in 0..3 {
                let v = data[i * 4 + c] as f32 * k + spec * af;
                data[i * 4 + c] = v.clamp(0.0, af).round() as u8;
            }
        }
    }
}

/// Skuggar lagret med fasningen (rastret).
pub fn shade_layer(l: &mut sk::Pixmap, width: f32, height: f32, kind: BevelType, light: &Lighting) {
    shade(l, width, height, kind, light, false);
}

/// Som `shade_layer`, men bara kantzonen blir kvar (PDF:ens överlägg).
pub fn shade_layer_zone_only(l: &mut sk::Pixmap, width: f32, height: f32, kind: BevelType, light: &Lighting) {
    shade(l, width, height, kind, light, true);
}

/// Konturen: lagrets täckning vidgad `width` bildpunkter i färgen `color`, i en bild med `m`
/// bildpunkters marginal runt lagret.
pub fn contour(l: &sk::Pixmap, m: usize, width: f32, color: Rgba) -> Option<sk::Pixmap> {
    let (lw, lh) = (l.width() as usize, l.height() as usize);
    let (w, h) = (lw + 2 * m, lh + 2 * m);
    let mut outside = vec![true; w * h];
    let data = l.data();
    for y in 0..lh {
        for x in 0..lw {
            if data[(y * lw + x) * 4 + 3] >= 128 {
                outside[(y + m) * w + x + m] = false;
            }
        }
    }
    let d = distance_field(&outside, w, h);
    let mut pm = sk::Pixmap::new(w as u32, h as u32)?;
    let out = pm.data_mut();
    let ca = color.a.clamp(0.0, 1.0);
    for (i, &dv) in d.iter().enumerate() {
        // Kantutjämning över en bildpunkt.
        let cov = (width + 0.5 - dv).clamp(0.0, 1.0) * ca;
        if cov <= 0.0 {
            continue;
        }
        out[i * 4] = (color.r.clamp(0.0, 1.0) * cov * 255.0 + 0.5) as u8;
        out[i * 4 + 1] = (color.g.clamp(0.0, 1.0) * cov * 255.0 + 0.5) as u8;
        out[i * 4 + 2] = (color.b.clamp(0.0, 1.0) * cov * 255.0 + 0.5) as u8;
        out[i * 4 + 3] = (cov * 255.0 + 0.5) as u8;
    }
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avstandsfaltet_ar_euklidiskt() {
        // En 9 × 9 fylld ruta i ett 11 × 11-fält: mitten 5 bildpunkter från kanten, hörnet 1.
        let (w, h) = (11, 11);
        let inside: Vec<bool> = (0..w * h).map(|i| (1..10).contains(&(i % w)) && (1..10).contains(&(i / w))).collect();
        let d = distance_field(&inside, w, h);
        assert_eq!(d[5 * w + 5], 5.0);
        assert_eq!(d[w + 1], 1.0);
        assert_eq!(d[0], 0.0);
        assert_eq!(d[3 * w + 5], 3.0);
    }

    #[test]
    fn fasningen_skuggar_kanterna() {
        // En grå kvadrat: övre kanten ljusare, nedre mörkare, mitten oförändrad.
        let mut pm = sk::Pixmap::new(60, 60).unwrap();
        pm.fill_rect(sk::Rect::from_xywh(5.0, 5.0, 50.0, 50.0).unwrap(), &sk::Paint { shader: sk::Shader::SolidColor(sk::Color::from_rgba8(128, 128, 128, 255)), ..Default::default() }, sk::Transform::identity(), None);
        let (az, el, amb) = rig("threePt").unwrap();
        let light = Lighting { light: light_dir(az, el), ambient: amb, specular: 0.0 };
        shade_layer(&mut pm, 10.0, 6.0, BevelType::Angle, &light);
        let px = |x: u32, y: u32| pm.pixel(x, y).unwrap().red();
        assert_eq!(px(30, 30), 128);
        assert!(px(30, 8) > 135, "topp {}", px(30, 8));
        assert!(px(30, 51) < 110, "botten {}", px(30, 51));
        assert!(px(8, 30) > px(51, 30), "vänster ljusare än höger");
        // Kanten utanför formen orörd.
        assert_eq!(pm.pixel(2, 2).unwrap().alpha(), 0);
        // Överlägget har bara kantzonen.
        let mut pm2 = sk::Pixmap::new(60, 60).unwrap();
        pm2.fill_rect(sk::Rect::from_xywh(5.0, 5.0, 50.0, 50.0).unwrap(), &sk::Paint { shader: sk::Shader::SolidColor(sk::Color::from_rgba8(128, 128, 128, 255)), ..Default::default() }, sk::Transform::identity(), None);
        shade_layer_zone_only(&mut pm2, 10.0, 6.0, BevelType::Angle, &light);
        assert_eq!(pm2.pixel(30, 30).unwrap().alpha(), 0);
        assert_eq!(pm2.pixel(30, 8).unwrap().red(), px(30, 8));
    }

    fn corpus() -> crate::pptx::Deck {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus/pptx/pythonpptx-shp-autoshape-props.pptx")).unwrap();
        crate::pptx::Deck::open(&bytes).unwrap()
    }

    fn unit_frame() -> Frame {
        let n = crate::pptx::xml::parse(br#"<a:xfrm xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm>"#).unwrap();
        crate::pptx::render::frame_of(&n, &Affine::IDENTITY).unwrap()
    }

    fn sp_pr(inner: &str) -> Node {
        let x = format!(r#"<p:sp xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:spPr>{inner}</p:spPr></p:sp>"#);
        crate::pptx::xml::parse(x.as_bytes()).unwrap()
    }

    #[test]
    fn rakt_framifran_utan_fasning_ger_samma_bild() {
        let deck = corpus();
        let sheets = Sheets::for_slide(&deck, 0);
        let f = unit_frame();
        let fill = crate::pptx::scene::Item::Fill { path: crate::pptx::scene::Path::rect(0.0, 0.0, 100.0, 100.0), paint: crate::pptx::scene::Paint::Solid(Rgba::WHITE), even_odd: false };
        let items = [fill];
        // `orthographicFront` med tom `sp3d` (eller utan `sp3d`) lägger inga effekter till: visningslistan
        // och därmed bilden blir densamma som utan `scene3d`.
        for inner in [r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d/>"#, r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d>"#, r#"<a:sp3d extrusionH="76200"><a:bevelB/></a:sp3d>"#] {
            let sp = sp_pr(inner);
            assert!(with_3d(&sheets, Vec::new(), &[&sp], None, &f, &items).is_empty(), "{inner}");
        }
        // Med `bevelT` blir det en fasning (standardmåtten, typen circle, halva bredden synlig).
        let sp = sp_pr(r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d><a:bevelT/></a:sp3d>"#);
        let e = with_3d(&sheets, Vec::new(), &[&sp], None, &f, &items);
        assert!(matches!(e.as_slice(), [Effect::Bevel { width, height, kind: BevelType::Circle, .. }] if (*width - 6.0 * ZONE).abs() < 1e-3 && (*height - 6.0).abs() < 1e-3), "{e:?}");
        // Bara en linje: ingen fasning. `flat`-riggen: ingen skuggning.
        assert!(with_3d(&sheets, Vec::new(), &[&sp], None, &f, &[]).is_empty());
        let flat = sp_pr(r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="flat" dir="t"/></a:scene3d><a:sp3d><a:bevelT/></a:sp3d>"#);
        assert!(with_3d(&sheets, Vec::new(), &[&flat], None, &f, &items).is_empty());
        // Kontur och isometrisk kamera.
        let iso = sp_pr(r#"<a:scene3d><a:camera prst="isometricOffAxis1Right"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="127000" contourW="12700"><a:contourClr><a:srgbClr val="FF0000"/></a:contourClr></a:sp3d>"#);
        let e = with_3d(&sheets, Vec::new(), &[&iso], None, &f, &items);
        assert!(e.iter().any(|x| matches!(x, Effect::Contour { width, .. } if (*width - 1.0).abs() < 1e-3)));
        assert!(e.iter().any(|x| matches!(x, Effect::Camera { extrude, .. } if extrude.0.abs() + extrude.1.abs() > 1.0)));
    }

    #[test]
    fn fasning_kontur_och_kamera_i_rastret_och_pdfen() {
        use crate::pptx::scene::{Item, Paint, Path, Scene};
        let (az, el, amb) = rig("threePt").unwrap();
        let lighting = Lighting { light: light_dir(az, el), ambient: amb, specular: 0.77 };
        let scene_with = |e: Vec<Effect>| {
            let mut s = Scene::new(100.0, 100.0);
            s.fill(Path::rect(0.0, 0.0, 100.0, 100.0), Paint::Solid(Rgba::WHITE));
            s.items.push(Item::BeginEffect(e));
            s.fill(Path::rect(30.0, 30.0, 40.0, 40.0), Paint::Solid(Rgba { r: 0.5, g: 0.5, b: 0.5, a: 1.0 }));
            s.items.push(Item::EndEffect);
            s
        };
        let px = |pm: &sk::Pixmap, x: u32, y: u32| pm.pixel(x, y).unwrap().demultiply().red();
        let bevel = Effect::Bevel { width: 6.0, height: 4.0, kind: BevelType::Angle, lighting };
        let pm = crate::pptx::raster::render(&scene_with(vec![bevel.clone()]), 100, 100).unwrap();
        assert!(px(&pm, 50, 31) > px(&pm, 50, 68), "ljusare upptill");
        assert_eq!(px(&pm, 50, 50), 128);
        // PDF:ens överlägg: bara kantzonen.
        let s = scene_with(vec![bevel]);
        let (ov, _) = crate::pptx::effects::overlay(&s.items[2..3], &[match &s.items[1] { Item::BeginEffect(e) => e[0].clone(), _ => unreachable!() }], 100.0, 100.0, 2.0).unwrap();
        let (w, h) = (ov.width(), ov.height());
        assert_eq!(ov.pixel(w / 2, h / 2).unwrap().alpha(), 0);
        // Kontur: rött band utanför formen.
        let pm = crate::pptx::raster::render(&scene_with(vec![Effect::Contour { width: 3.0, color: Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 } }]), 100, 100).unwrap();
        let p = pm.pixel(28, 50).unwrap().demultiply();
        assert!(p.red() > 200 && p.green() < 60, "{p:?}");
        assert_eq!(px(&pm, 20, 50), 255);
        // Kamera: formen förvrängs (smalare) och extruderingen syns bredvid.
        let t = Affine::translate(-50.0, -50.0).then(&Affine::scale(0.5, 1.0)).then(&Affine::translate(50.0, 50.0));
        let s = scene_with(vec![Effect::Camera { t, extrude: (5.0, 0.0), color: Rgba { r: 0.0, g: 0.0, b: 1.0, a: 1.0 } }]);
        let pm = crate::pptx::raster::render(&s, 100, 100).unwrap();
        assert_eq!(px(&pm, 35, 50), 255, "formen är smalare");
        assert!(pm.pixel(62, 50).unwrap().demultiply().blue() > 200 && px(&pm, 62, 50) < 60, "extrudering till höger");
        assert!(crate::pptx::effects::rasterized(&s.items[2..3], &[Effect::Camera { t, extrude: (5.0, 0.0), color: Rgba::WHITE }], 100.0, 100.0, 2.0).is_some());
    }

    #[test]
    fn profilernas_form() {
        // Alla stigande profiler når ungefär full höjd vid t = 1 och börjar på 0.
        for k in [BevelType::Circle, BevelType::RelaxedInset, BevelType::CoolSlant, BevelType::Angle, BevelType::SoftRound, BevelType::Convex, BevelType::Slope, BevelType::HardEdge, BevelType::ArtDeco] {
            assert!(k.height(0.0).abs() < 1e-6);
            let h = k.height(1.0);
            assert!((0.9..1.25).contains(&h), "{k:?}: {h}");
            // Aldrig fallande.
            assert!((0..100).all(|i| k.slope(i as f32 / 100.0) >= 0.0), "{k:?}");
        }
        // Cirkeln är brantast vid kanten och plan inåt; vinkeln är rak.
        assert!(BevelType::Circle.slope(0.01) > 3.0 && BevelType::Circle.slope(0.99) < 0.2);
        assert_eq!(BevelType::Angle.slope(0.3), BevelType::Angle.slope(0.9));
        // coolSlant: brant ytter-fasett, plan efter 60 %.
        assert!(BevelType::CoolSlant.slope(0.2) > 2.0 && BevelType::CoolSlant.slope(0.7) == 0.0);
        // Kors och skåra: upp och ned (åsen respektive diket i mitten).
        assert!(BevelType::Cross.slope(0.25) > 0.0 && BevelType::Cross.slope(0.75) < 0.0);
        assert!(BevelType::Divot.slope(0.1) < 0.0 && BevelType::Divot.slope(0.9) > 0.0);
        assert!(BevelType::Cross.height(0.5) > BevelType::Cross.height(1.0));
        // Ribbor: flera åsar.
        let sign_changes = (1..100).filter(|i| (BevelType::Riblet.slope(*i as f32 / 100.0) > 0.0) != (BevelType::Riblet.slope((*i - 1) as f32 / 100.0) > 0.0)).count();
        assert!(sign_changes >= 2);
    }

    #[test]
    fn ljuset_som_i_powerpoint() {
        // threePt, dir t: fasettens ljushet (lutning ≈ 46°) uppåt > åt vänster > åt höger > nedåt.
        let (az, el, amb) = rig("threePt").unwrap();
        let l = Lighting { light: light_dir(az, el), ambient: amb, specular: 0.0 };
        let s = 46f32.to_radians().sin();
        let c = 46f32.to_radians().cos();
        let up = l.shade([0.0, -s, c]).0;
        let down = l.shade([0.0, s, c]).0;
        let left = l.shade([-s, 0.0, c]).0;
        let right = l.shade([s, 0.0, c]).0;
        assert!((up - 1.04).abs() < 0.08 && (down - 0.52).abs() < 0.08, "{up} {down}");
        // Åt sidorna blir det för ljust med en ljuskälla (0,91/0,68 mot uppmätt 0,81/0,57); ordningen stämmer.
        assert!((left - 0.81).abs() < 0.12 && (right - 0.57).abs() < 0.12 && left > right, "{left} {right}");
        // En plan yta är oförändrad, utan glans.
        let l = Lighting { specular: 0.77, ..l };
        let (k, sp) = l.shade([0.0, 0.0, 1.0]);
        assert!((k - 1.0).abs() < 1e-4 && sp.abs() < 1e-4);
        assert!(rig("flat").is_none());
    }

    #[test]
    fn ljuset_vrids_med_formen_men_inte_med_speglingar() {
        let frame = |rot: f32, flip_v: bool, parent: Affine| {
            let x = format!(r#"<a:xfrm xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" rot="{}" flipV="{}"><a:off x="0" y="0"/><a:ext cx="127000" cy="127000"/></a:xfrm>"#, (rot * 60000.0) as i64, if flip_v { 1 } else { 0 });
            let n = crate::pptx::xml::parse(x.as_bytes()).unwrap();
            crate::pptx::render::frame_of(&n, &parent).unwrap()
        };
        assert!(rotation_without_flips(&frame(0.0, false, Affine::IDENTITY)).abs() < 1e-3);
        assert!((rotation_without_flips(&frame(90.0, false, Affine::IDENTITY)) - 90.0).abs() < 1e-3);
        // Spegling ensam vrider inte ljuset.
        assert!(rotation_without_flips(&frame(0.0, true, Affine::IDENTITY)).abs() < 1e-3);
        // Gruppen vriden 90° och lodrätt speglad (som i PowerPoint-materialet): ljuset vrids 90°.
        let group = Affine::scale(1.0, -1.0).then(&Affine::rotate(90.0));
        assert!((rotation_without_flips(&frame(0.0, false, group)) - 90.0).abs() < 1e-3);
    }

    #[test]
    fn kameror() {
        let n = crate::pptx::xml::parse(br#"<a:xfrm xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm>"#).unwrap();
        let f = crate::pptx::render::frame_of(&n, &Affine::IDENTITY).unwrap();
        // Rakt framifrån och sneda kameror förvränger inte formen.
        for name in ["orthographicFront", "perspectiveFront", "obliqueTopLeft"] {
            assert!(camera_affine(name, None, &f).is_none(), "{name}");
        }
        // Isometrisk: mitten står still, formen trycks ihop.
        let t = camera_affine("isometricLeftDown", None, &f).unwrap();
        let (x, y) = t.apply(50.0, 50.0);
        assert!((x - 50.0).abs() < 1e-3 && (y - 50.0).abs() < 1e-3);
        let (x1, _) = t.apply(100.0, 50.0);
        assert!(x1 - 50.0 < 49.0);
        // Snett uppåt till vänster: djupet åt vänster och uppåt.
        let (dx, dy) = camera_depth("obliqueTopLeft", None);
        assert!(dx < 0.0 && dy < 0.0);
    }
}
