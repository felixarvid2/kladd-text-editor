//! Matematiktypsnittet (STIX Two Math, OFL) och dess OpenType MATH-tabell: konstanter, kursivkorrektion,
//! accentfästen och storleksvarianter/sammansättningar för sträckbara tecken.
//!
//! `ttf-parser` har inget stöd för MATH, så tabellen läses här direkt ur bytena.

use std::collections::HashMap;
use std::sync::LazyLock;

use ttf_parser::{Face, GlyphId, OutlineBuilder, Tag};

pub const FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/STIXTwoMath-Regular.otf");

/// Index i `MathFont::k` (ordningen i MathConstants).
#[allow(dead_code)]
pub mod k {
    pub const AXIS: usize = 1;
    pub const ACCENT_BASE: usize = 2;
    pub const FLAT_ACCENT_BASE: usize = 3;
    pub const SUB_SHIFT_DOWN: usize = 4;
    pub const SUB_TOP_MAX: usize = 5;
    pub const SUB_DROP_MIN: usize = 6;
    pub const SUP_SHIFT_UP: usize = 7;
    pub const SUP_SHIFT_UP_CRAMPED: usize = 8;
    pub const SUP_BOTTOM_MIN: usize = 9;
    pub const SUP_DROP_MAX: usize = 10;
    pub const SUBSUP_GAP_MIN: usize = 11;
    pub const SUP_BOTTOM_MAX_WITH_SUB: usize = 12;
    pub const SPACE_AFTER_SCRIPT: usize = 13;
    pub const UPPER_LIMIT_GAP: usize = 14;
    pub const UPPER_LIMIT_RISE: usize = 15;
    pub const LOWER_LIMIT_GAP: usize = 16;
    pub const LOWER_LIMIT_DROP: usize = 17;
    pub const STACK_TOP_UP: usize = 18;
    pub const STACK_TOP_DISPLAY_UP: usize = 19;
    pub const STACK_BOTTOM_DOWN: usize = 20;
    pub const STACK_BOTTOM_DISPLAY_DOWN: usize = 21;
    pub const STACK_GAP: usize = 22;
    pub const STACK_DISPLAY_GAP: usize = 23;
    pub const FRAC_NUM_UP: usize = 28;
    pub const FRAC_NUM_DISPLAY_UP: usize = 29;
    pub const FRAC_DEN_DOWN: usize = 30;
    pub const FRAC_DEN_DISPLAY_DOWN: usize = 31;
    pub const FRAC_NUM_GAP: usize = 32;
    pub const FRAC_NUM_DISPLAY_GAP: usize = 33;
    pub const FRAC_RULE: usize = 34;
    pub const FRAC_DEN_GAP: usize = 35;
    pub const FRAC_DEN_DISPLAY_GAP: usize = 36;
    pub const OVERBAR_GAP: usize = 39;
    pub const OVERBAR_RULE: usize = 40;
    pub const OVERBAR_EXTRA: usize = 41;
    pub const UNDERBAR_GAP: usize = 42;
    pub const UNDERBAR_RULE: usize = 43;
    pub const UNDERBAR_EXTRA: usize = 44;
    pub const RADICAL_GAP: usize = 45;
    pub const RADICAL_DISPLAY_GAP: usize = 46;
    pub const RADICAL_RULE: usize = 47;
    pub const RADICAL_EXTRA: usize = 48;
    pub const RADICAL_KERN_BEFORE: usize = 49;
    pub const RADICAL_KERN_AFTER: usize = 50;
}

/// En del i en sammansättning av ett sträckbart tecken.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub gid: u16,
    pub start: f32,
    pub end: f32,
    pub full: f32,
    pub extender: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Construction {
    /// Varianter i växande storlek: (glyf, mått längs riktningen i em).
    pub variants: Vec<(u16, f32)>,
    pub assembly: Vec<Part>,
}

pub struct MathFont {
    pub face: Face<'static>,
    pub upem: f32,
    vals: Vec<f32>,
    pub script_pct: f32,
    pub script_script_pct: f32,
    pub delimited_min: f32,
    pub display_op_min: f32,
    pub radical_raise_pct: f32,
    pub min_overlap: f32,
    italic: HashMap<u16, f32>,
    accent: HashMap<u16, f32>,
    vert: HashMap<u16, Construction>,
    horiz: HashMap<u16, Construction>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn i16_at(d: &[u8], o: usize) -> Option<i16> {
    u16_at(d, o).map(|v| v as i16)
}

/// Täckningstabell: glyf -> index.
fn coverage(d: &[u8], o: usize) -> Option<HashMap<u16, usize>> {
    let mut m = HashMap::new();
    match u16_at(d, o)? {
        1 => {
            let n = u16_at(d, o + 2)? as usize;
            for i in 0..n {
                m.insert(u16_at(d, o + 4 + 2 * i)?, i);
            }
        }
        2 => {
            let n = u16_at(d, o + 2)? as usize;
            for i in 0..n {
                let r = o + 4 + 6 * i;
                let (s, e, idx) = (u16_at(d, r)?, u16_at(d, r + 2)?, u16_at(d, r + 4)? as usize);
                for g in s..=e {
                    m.insert(g, idx + (g - s) as usize);
                }
            }
        }
        _ => return None,
    }
    Some(m)
}

/// `count` MathValueRecord (4 byte) med täckningstabell: glyf -> värde.
fn value_map(d: &[u8], o: usize, upem: f32) -> Option<HashMap<u16, f32>> {
    let cov = coverage(d, o + u16_at(d, o)? as usize)?;
    let n = u16_at(d, o + 2)? as usize;
    let mut m = HashMap::new();
    for (g, i) in cov {
        if i < n {
            m.insert(g, i16_at(d, o + 4 + 4 * i)? as f32 / upem);
        }
    }
    Some(m)
}

fn constructions(d: &[u8], base: usize, cov_off: usize, count: usize, offs_at: usize, upem: f32) -> Option<HashMap<u16, Construction>> {
    let mut out = HashMap::new();
    if cov_off == 0 {
        return Some(out);
    }
    let cov = coverage(d, base + cov_off)?;
    for (g, i) in cov {
        if i >= count {
            continue;
        }
        let c = base + u16_at(d, offs_at + 2 * i)? as usize;
        let asm_off = u16_at(d, c)? as usize;
        let nv = u16_at(d, c + 2)? as usize;
        let mut con = Construction::default();
        for v in 0..nv {
            let r = c + 4 + 4 * v;
            con.variants.push((u16_at(d, r)?, u16_at(d, r + 2)? as f32 / upem));
        }
        if asm_off != 0 {
            let a = c + asm_off;
            let np = u16_at(d, a + 4)? as usize;
            for p in 0..np {
                let r = a + 6 + 10 * p;
                con.assembly.push(Part {
                    gid: u16_at(d, r)?,
                    start: u16_at(d, r + 2)? as f32 / upem,
                    end: u16_at(d, r + 4)? as f32 / upem,
                    full: u16_at(d, r + 6)? as f32 / upem,
                    extender: u16_at(d, r + 8)? & 1 != 0,
                });
            }
        }
        out.insert(g, con);
    }
    Some(out)
}

impl MathFont {
    fn load() -> Option<MathFont> {
        let face = Face::parse(FONT_BYTES, 0).ok()?;
        let upem = f32::from(face.units_per_em());
        let t = face.raw_face().table(Tag::from_bytes(b"MATH"))?;
        let consts = u16_at(t, 4)? as usize;
        let info = u16_at(t, 6)? as usize;
        let variants = u16_at(t, 8)? as usize;
        let mut vals = Vec::new();
        for i in 0..51 {
            vals.push(i16_at(t, consts + 8 + 4 * i)? as f32 / upem);
        }
        let (italic, accent) = if info != 0 {
            let ic = u16_at(t, info)? as usize;
            let ac = u16_at(t, info + 2)? as usize;
            (
                if ic != 0 { value_map(t, info + ic, upem)? } else { HashMap::new() },
                if ac != 0 { value_map(t, info + ac, upem)? } else { HashMap::new() },
            )
        } else {
            (HashMap::new(), HashMap::new())
        };
        let (mut vert, mut horiz, mut overlap) = (HashMap::new(), HashMap::new(), 0.0);
        if variants != 0 {
            overlap = u16_at(t, variants)? as f32 / upem;
            let (vc, hc) = (u16_at(t, variants + 2)? as usize, u16_at(t, variants + 4)? as usize);
            let (vn, hn) = (u16_at(t, variants + 6)? as usize, u16_at(t, variants + 8)? as usize);
            vert = constructions(t, variants, vc, vn, variants + 10, upem)?;
            horiz = constructions(t, variants, hc, hn, variants + 10 + 2 * vn, upem)?;
        }
        Some(MathFont {
            face,
            upem,
            vals,
            script_pct: i16_at(t, consts)? as f32 / 100.0,
            script_script_pct: i16_at(t, consts + 2)? as f32 / 100.0,
            delimited_min: u16_at(t, consts + 4)? as f32 / upem,
            display_op_min: u16_at(t, consts + 6)? as f32 / upem,
            radical_raise_pct: i16_at(t, consts + 8 + 4 * 51)? as f32 / 100.0,
            min_overlap: overlap,
            italic,
            accent,
            vert,
            horiz,
        })
    }

    /// Konstant nummer `i` (se `k`) i em.
    pub fn k(&self, i: usize) -> f32 {
        self.vals.get(i).copied().unwrap_or(0.0)
    }

    pub fn glyph(&self, c: char) -> u16 {
        self.face.glyph_index(c).map_or(0, |g| g.0)
    }

    pub fn has(&self, c: char) -> bool {
        self.face.glyph_index(c).is_some()
    }

    /// Framflyttning i em.
    pub fn advance(&self, g: u16) -> f32 {
        self.face.glyph_hor_advance(GlyphId(g)).map_or(0.0, |a| f32::from(a) / self.upem)
    }

    /// Syns rektangel (x0, y0, x1, y1) i em, y uppåt.
    pub fn bbox(&self, g: u16) -> Option<(f32, f32, f32, f32)> {
        let r = self.face.glyph_bounding_box(GlyphId(g))?;
        let u = self.upem;
        Some((f32::from(r.x_min) / u, f32::from(r.y_min) / u, f32::from(r.x_max) / u, f32::from(r.y_max) / u))
    }

    /// Kursivkorrektion i em (0 om glyfen saknar).
    pub fn italic_correction(&self, g: u16) -> f32 {
        self.italic.get(&g).copied().unwrap_or(0.0)
    }

    /// Fästpunkt för accent ovanpå glyfen (x i em), om typsnittet anger en.
    pub fn top_accent(&self, g: u16) -> Option<f32> {
        self.accent.get(&g).copied()
    }

    pub fn vertical(&self, g: u16) -> Option<&Construction> {
        self.vert.get(&g)
    }

    pub fn horizontal(&self, g: u16) -> Option<&Construction> {
        self.horiz.get(&g)
    }

    /// Tecknets kontur i typsnittsenheter (y uppåt).
    pub fn outline<B: OutlineBuilder>(&self, g: u16, b: &mut B) {
        self.face.outline_glyph(GlyphId(g), b);
    }
}

static FONT: LazyLock<Option<MathFont>> = LazyLock::new(MathFont::load);

/// Det inbäddade typsnittet. `None` bara om bytena är trasiga (testas).
pub fn font() -> Option<&'static MathFont> {
    FONT.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_table_is_read() {
        let f = font().expect("typsnittet laddas");
        assert!((f.upem - 1000.0).abs() < 1.0 || f.upem > 0.0);
        // Axelhöjd och bråkstreck ligger i rimliga em.
        let axis = f.k(k::AXIS);
        assert!(axis > 0.15 && axis < 0.4, "axis {axis}");
        let rule = f.k(k::FRAC_RULE);
        assert!(rule > 0.02 && rule < 0.1, "rule {rule}");
        assert!(f.script_pct > 0.6 && f.script_pct < 0.8);
        assert!(f.script_script_pct > 0.4 && f.script_script_pct < 0.6);
        assert!(f.delimited_min > 0.5);
        assert!(f.min_overlap > 0.0);
    }

    #[test]
    #[ignore]
    fn debug_integral() {
        let f = font().unwrap();
        let g = f.glyph('∫');
        let con = f.vertical(g).unwrap();
        println!("base adv {} ital {} bbox {:?}", f.advance(g), f.italic_correction(g), f.bbox(g));
        for (v, a) in &con.variants {
            println!("var {v} adv_measure {a} adv {} ital {} bbox {:?}", f.advance(*v), f.italic_correction(*v), f.bbox(*v));
        }
    }

    #[test]
    fn stretchy_parentheses_and_italic_corrections() {
        let f = font().unwrap();
        let g = f.glyph('(');
        let con = f.vertical(g).expect("parentesen har varianter");
        assert!(con.variants.len() >= 3);
        assert!(con.variants.windows(2).all(|w| w[0].1 <= w[1].1));
        assert!(!con.assembly.is_empty());
        assert!(f.vertical(f.glyph('√')).is_some());
        assert!(f.horizontal(f.glyph('\u{302}')).is_some() || f.horizontal(f.glyph('\u{2c6}')).is_some());
        let integral = f.glyph('∫');
        assert!(f.italic_correction(integral) > 0.0 || f.vertical(integral).is_some());
        assert!(f.has('𝑥') && f.has('α') && f.has('∑'));
    }
}
