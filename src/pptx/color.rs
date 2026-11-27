//! DrawingML-färger: `srgbClr`, `schemeClr`, `sysClr`, `prstClr`, `hslClr`, `scrgbClr` och
//! modifierarna (`lumMod`, `lumOff`, `tint`, `shade`, `satMod`, `alpha` …).
//!
//! Modifierarna tillämpas i dokumentordning. `tint`/`shade` räknas i linjärt RGB (scRGB) och
//! `lum`/`sat`/`hue` i HSL, som i Apache POI:s `DrawPaint.applyColorTransform` (Apache-2.0).

use std::collections::HashMap;

use super::xml::Node;

/// En färg i sRGB, komponenter 0..1, med täckning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: Rgba = Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    pub fn from_hex(s: &str) -> Option<Rgba> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Rgba::rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    pub fn rgb8(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
    }

    #[allow(dead_code)]
    pub fn to_u8(self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [c(self.r), c(self.g), c(self.b), c(self.a)]
    }

    /// Ljushet (för grå bilder och kontrast).
    #[allow(dead_code)]
    pub fn luma(self) -> f32 {
        0.299 * self.r + 0.587 * self.g + 0.114 * self.b
    }
}

/// Det som behövs för att lösa upp temafärger: temats färgschema och mallens `clrMap`
/// (med `clrMapOvr` från layout/bild redan tillämpad).
#[derive(Clone, Debug, Default)]
pub struct ColorCtx {
    /// `dk1`, `lt1`, `dk2`, `lt2`, `accent1`…`accent6`, `hlink`, `folHlink`.
    pub scheme: HashMap<String, Rgba>,
    /// `bg1` → `lt1`, `tx1` → `dk1` …
    pub map: HashMap<String, String>,
}

impl ColorCtx {
    /// En schemafärg efter namn (`tx1`, `accent2`, `dk1` …).
    pub fn scheme_color(&self, name: &str) -> Option<Rgba> {
        let mapped = self.map.get(name).map(|s| s.as_str()).unwrap_or(match name {
            // Standardmappning om clrMap saknas.
            "bg1" => "lt1",
            "tx1" => "dk1",
            "bg2" => "lt2",
            "tx2" => "dk2",
            other => other,
        });
        self.scheme.get(mapped).or_else(|| self.scheme.get(name)).copied()
    }
}

/// Är noden ett färgelement?
pub fn is_color(n: &Node) -> bool {
    matches!(n.name.as_str(), "a:srgbClr" | "a:schemeClr" | "a:sysClr" | "a:prstClr" | "a:hslClr" | "a:scrgbClr")
}

/// Första färgelementet bland barnen (t.ex. i `a:solidFill`).
pub fn first_color(parent: &Node) -> Option<&Node> {
    parent.children.iter().find(|c| is_color(c))
}

/// Löser en färg. `ph` är platshållarfärgen (`phClr`) för temats stilmatris.
pub fn resolve(n: &Node, ctx: &ColorCtx, ph: Option<Rgba>) -> Option<Rgba> {
    let base = match n.name.as_str() {
        "a:srgbClr" => Rgba::from_hex(n.attr("val")?)?,
        "a:schemeClr" => {
            let v = n.attr("val")?;
            if v == "phClr" { ph.unwrap_or(Rgba::BLACK) } else { ctx.scheme_color(v)? }
        }
        "a:sysClr" => match n.attr("lastClr").and_then(Rgba::from_hex) {
            Some(c) => c,
            None => system_color(n.attr("val").unwrap_or(""))?,
        },
        "a:prstClr" => preset_color(n.attr("val")?)?,
        "a:hslClr" => {
            let h = n.attr_f64("hue").unwrap_or(0.0) / 60000.0;
            let s = n.attr("sat").and_then(super::xml::parse_i64).unwrap_or(0) as f64 / 100000.0;
            let l = n.attr("lum").and_then(super::xml::parse_i64).unwrap_or(0) as f64 / 100000.0;
            let (r, g, b) = hsl_to_rgb(h, s, l);
            Rgba { r: r as f32, g: g as f32, b: b as f32, a: 1.0 }
        }
        "a:scrgbClr" => {
            let c = |k: &str| linear_to_srgb(n.attr(k).and_then(super::xml::parse_i64).unwrap_or(0) as f64 / 100000.0) as f32;
            Rgba { r: c("r"), g: c("g"), b: c("b"), a: 1.0 }
        }
        _ => return None,
    };
    Some(apply_mods(base, &n.children))
}

/// Tillämpar modifierarna i ordning.
pub fn apply_mods(c: Rgba, mods: &[Node]) -> Rgba {
    let mut rgb = [c.r as f64, c.g as f64, c.b as f64];
    let mut a = c.a as f64;
    for m in mods {
        let v = || m.attr("val").and_then(super::xml::parse_i64).unwrap_or(0) as f64 / 100000.0;
        match m.local() {
            "alpha" => a = v(),
            "alphaMod" => a *= v(),
            "alphaOff" => a += v(),
            "tint" => {
                let t = v();
                for x in &mut rgb {
                    let lin = srgb_to_linear(*x);
                    *x = linear_to_srgb(1.0 - (1.0 - lin) * t);
                }
            }
            "shade" => {
                let s = v();
                for x in &mut rgb {
                    let lin = srgb_to_linear(*x);
                    *x = linear_to_srgb((lin * s).clamp(0.0, 1.0));
                }
            }
            "lumMod" | "lumOff" | "lum" | "satMod" | "satOff" | "sat" | "hueMod" | "hueOff" | "hue" => {
                let (mut h, mut s, mut l) = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
                match m.local() {
                    "lumMod" => l *= v(),
                    "lumOff" => l += v(),
                    "lum" => l = v(),
                    "satMod" => s *= v(),
                    "satOff" => s += v(),
                    "sat" => s = v(),
                    "hueMod" => h *= v(),
                    "hueOff" => h += m.attr_f64("val").unwrap_or(0.0) / 60000.0,
                    "hue" => h = m.attr_f64("val").unwrap_or(0.0) / 60000.0,
                    _ => {}
                }
                let (r, g, b) = hsl_to_rgb(h.rem_euclid(360.0), s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
                rgb = [r, g, b];
            }
            "comp" => {
                let (h, s, l) = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
                let (r, g, b) = hsl_to_rgb((h + 180.0).rem_euclid(360.0), s, l);
                rgb = [r, g, b];
            }
            "inv" => {
                for x in &mut rgb {
                    *x = 1.0 - *x;
                }
            }
            "gray" => {
                let y = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
                rgb = [y, y, y];
            }
            "red" => rgb[0] = linear_to_srgb(v()),
            "green" => rgb[1] = linear_to_srgb(v()),
            "blue" => rgb[2] = linear_to_srgb(v()),
            "redMod" => rgb[0] = linear_to_srgb(srgb_to_linear(rgb[0]) * v()),
            "greenMod" => rgb[1] = linear_to_srgb(srgb_to_linear(rgb[1]) * v()),
            "blueMod" => rgb[2] = linear_to_srgb(srgb_to_linear(rgb[2]) * v()),
            "redOff" => rgb[0] = linear_to_srgb(srgb_to_linear(rgb[0]) + v()),
            "greenOff" => rgb[1] = linear_to_srgb(srgb_to_linear(rgb[1]) + v()),
            "blueOff" => rgb[2] = linear_to_srgb(srgb_to_linear(rgb[2]) + v()),
            "gamma" => {
                for x in &mut rgb {
                    *x = linear_to_srgb(*x);
                }
            }
            "invGamma" => {
                for x in &mut rgb {
                    *x = srgb_to_linear(*x);
                }
            }
            _ => {}
        }
        for x in &mut rgb {
            *x = x.clamp(0.0, 1.0);
        }
    }
    Rgba { r: rgb[0] as f32, g: rgb[1] as f32, b: rgb[2] as f32, a: a.clamp(0.0, 1.0) as f32 }
}

pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0 {
        0.0
    } else if c <= 0.0031308 {
        c * 12.92
    } else if c < 1.0 {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    } else {
        1.0
    }
}

/// RGB (0..1) → (nyans 0..360, mättnad 0..1, ljushet 0..1).
pub fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-12 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    if s <= 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let h = h / 360.0;
    let f = |t: f64| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

fn system_color(name: &str) -> Option<Rgba> {
    Some(match name {
        "windowText" | "menuText" | "captionText" | "btnText" | "infoText" => Rgba::BLACK,
        "window" | "menu" | "btnHighlight" | "highlightText" => Rgba::WHITE,
        "btnFace" | "3dLight" | "menuBar" => Rgba::rgb8(0xF0, 0xF0, 0xF0),
        "btnShadow" | "grayText" => Rgba::rgb8(0xA0, 0xA0, 0xA0),
        "highlight" | "hotLight" => Rgba::rgb8(0x00, 0x78, 0xD7),
        "3dDkShadow" => Rgba::rgb8(0x69, 0x69, 0x69),
        "infoBk" => Rgba::rgb8(0xFF, 0xFF, 0xE1),
        _ => return None,
    })
}

/// `ST_PresetColorVal`: CSS-namnen med OOXML:s stavning (`dkBlue`, `ltGray` …).
fn preset_color(name: &str) -> Option<Rgba> {
    let n = name.to_lowercase();
    // OOXML skriver "dk"/"lt"/"med" där CSS skriver "dark"/"light"/"medium".
    let css = if let Some(r) = n.strip_prefix("dk") {
        format!("dark{r}")
    } else if let Some(r) = n.strip_prefix("lt") {
        format!("light{r}")
    } else if let Some(r) = n.strip_prefix("med") {
        if r.starts_with("ium") { n.clone() } else { format!("medium{r}") }
    } else {
        n.clone()
    };
    let hex = match css.as_str() {
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        "red" => 0xFF0000,
        "green" => 0x008000,
        "blue" => 0x0000FF,
        "yellow" => 0xFFFF00,
        "cyan" | "aqua" => 0x00FFFF,
        "magenta" | "fuchsia" => 0xFF00FF,
        "gray" | "grey" => 0x808080,
        "darkgray" | "darkgrey" => 0xA9A9A9,
        "lightgray" | "lightgrey" => 0xD3D3D3,
        "silver" => 0xC0C0C0,
        "maroon" => 0x800000,
        "darkred" => 0x8B0000,
        "orange" => 0xFFA500,
        "darkorange" => 0xFF8C00,
        "gold" => 0xFFD700,
        "navy" => 0x000080,
        "darkblue" => 0x00008B,
        "mediumblue" => 0x0000CD,
        "lightblue" => 0xADD8E6,
        "skyblue" => 0x87CEEB,
        "steelblue" => 0x4682B4,
        "royalblue" => 0x4169E1,
        "darkgreen" => 0x006400,
        "lightgreen" => 0x90EE90,
        "lime" => 0x00FF00,
        "olive" => 0x808000,
        "teal" => 0x008080,
        "purple" => 0x800080,
        "darkmagenta" => 0x8B008B,
        "violet" => 0xEE82EE,
        "pink" => 0xFFC0CB,
        "brown" => 0xA52A2A,
        "tan" => 0xD2B48C,
        "beige" => 0xF5F5DC,
        "ivory" => 0xFFFFF0,
        "indigo" => 0x4B0082,
        "crimson" => 0xDC143C,
        "coral" => 0xFF7F50,
        "salmon" => 0xFA8072,
        "khaki" => 0xF0E68C,
        "turquoise" => 0x40E0D0,
        "darkcyan" => 0x008B8B,
        "lightyellow" => 0xFFFFE0,
        "lightcyan" => 0xE0FFFF,
        "chocolate" => 0xD2691E,
        "firebrick" => 0xB22222,
        "forestgreen" => 0x228B22,
        "seagreen" => 0x2E8B57,
        "slategray" | "slategrey" => 0x708090,
        "darkslategray" | "darkslategrey" => 0x2F4F4F,
        "dimgray" | "dimgrey" => 0x696969,
        "whitesmoke" => 0xF5F5F5,
        "gainsboro" => 0xDCDCDC,
        "orangered" => 0xFF4500,
        "tomato" => 0xFF6347,
        "plum" => 0xDDA0DD,
        "orchid" => 0xDA70D6,
        "darkviolet" => 0x9400D3,
        "midnightblue" => 0x191970,
        "cornflowerblue" => 0x6495ED,
        "dodgerblue" => 0x1E90FF,
        "deepskyblue" => 0x00BFFF,
        "limegreen" => 0x32CD32,
        "yellowgreen" => 0x9ACD32,
        "darkolivegreen" => 0x556B2F,
        "sienna" => 0xA0522D,
        "peru" => 0xCD853F,
        "wheat" => 0xF5DEB3,
        "lavender" => 0xE6E6FA,
        _ => return None,
    };
    Some(Rgba::rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ColorCtx {
        let mut c = ColorCtx::default();
        c.scheme.insert("dk1".into(), Rgba::BLACK);
        c.scheme.insert("lt1".into(), Rgba::WHITE);
        c.scheme.insert("accent1".into(), Rgba::from_hex("4472C4").unwrap());
        c.scheme.insert("dk2".into(), Rgba::from_hex("44546A").unwrap());
        c
    }

    fn node(xml: &str) -> Node {
        let full = format!(r#"<a:x xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">{xml}</a:x>"#);
        super::super::xml::parse(full.as_bytes()).unwrap().children.remove(0)
    }

    fn hex(c: Rgba) -> String {
        let [r, g, b, _] = c.to_u8();
        format!("{r:02X}{g:02X}{b:02X}")
    }

    #[test]
    fn schema_med_lummod_lumoff() {
        // Office-temats "Blå, dekorfärg 1, ljusare 40 %": 4472C4 → 8FAADC.
        let n = node(r#"<a:schemeClr val="accent1"><a:lumMod val="60000"/><a:lumOff val="40000"/></a:schemeClr>"#);
        assert_eq!(hex(resolve(&n, &ctx(), None).unwrap()), "8FAADC");
        // "mörkare 25 %": 4472C4 → 2F5597.
        let n = node(r#"<a:schemeClr val="accent1"><a:lumMod val="75000"/></a:schemeClr>"#);
        assert_eq!(hex(resolve(&n, &ctx(), None).unwrap()), "2F5597");
    }

    #[test]
    fn clrmap_och_alpha() {
        let n = node(r#"<a:schemeClr val="tx1"><a:alpha val="50000"/></a:schemeClr>"#);
        let c = resolve(&n, &ctx(), None).unwrap();
        assert_eq!(hex(c), "000000");
        assert!((c.a - 0.5).abs() < 1e-6);
        let n = node(r#"<a:schemeClr val="tx2"/>"#);
        assert_eq!(hex(resolve(&n, &ctx(), None).unwrap()), "44546A");
    }

    #[test]
    fn tint_shade_och_platshallare() {
        let n = node(r#"<a:schemeClr val="phClr"><a:shade val="50000"/></a:schemeClr>"#);
        let c = resolve(&n, &ctx(), Some(Rgba::WHITE)).unwrap();
        // Skugga 50 % i linjärt RGB: 1.0*0.5 → sRGB ≈ 0.735 (BC).
        assert_eq!(hex(c), "BCBCBC");
        let n = node(r#"<a:srgbClr val="000000"><a:tint val="50000"/></a:srgbClr>"#);
        assert_eq!(hex(resolve(&n, &ctx(), None).unwrap()), "BCBCBC");
    }

    #[test]
    fn ovriga_typer() {
        assert_eq!(hex(resolve(&node(r#"<a:sysClr val="windowText" lastClr="000000"/>"#), &ctx(), None).unwrap()), "000000");
        assert_eq!(hex(resolve(&node(r#"<a:sysClr val="window"/>"#), &ctx(), None).unwrap()), "FFFFFF");
        assert_eq!(hex(resolve(&node(r#"<a:prstClr val="dkBlue"/>"#), &ctx(), None).unwrap()), "00008B");
        assert_eq!(hex(resolve(&node(r#"<a:hslClr hue="0" sat="100000" lum="50000"/>"#), &ctx(), None).unwrap()), "FF0000");
        assert_eq!(hex(resolve(&node(r#"<a:scrgbClr r="100000" g="0" b="0"/>"#), &ctx(), None).unwrap()), "FF0000");
    }
}
