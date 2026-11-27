//! Förinställda former (`a:prstGeom`) och egna former (`a:custGeom`) enligt ECMA-376 del 1 §20.1.9.
//!
//! Definitionerna (`presetShapeDefinitions.xml`, från Apache POI, Apache-2.0, se `NOTICE.md`) bäddas
//! in och tolkas första gången en form behövs. Utvärderaren följer POI:s `sl/draw/geom`: formlerna
//! (`*/`, `+-`, `+/`, `?:`, `abs`, `at2`, `cat2`, `cos`, `max`, `min`, `mod`, `pin`, `sat2`, `sin`,
//! `sqrt`, `tan`, `val`), de inbyggda variablerna (`w`, `h`, `ss`, `hd2`, `cd4` …) och `arcTo`.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::scene::Path;
use super::xml::{self, Node};

const PRESETS_XML: &[u8] = include_bytes!("presetShapeDefinitions.xml");

/// Hur en delbana fylls (`a:path@fill`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FillMode {
    None,
    Norm,
    Lighten,
    LightenLess,
    Darken,
    DarkenLess,
}

/// En delbana i formens lokala koordinater (punkter, 0..w × 0..h).
#[derive(Clone, Debug)]
pub struct SubPath {
    pub path: Path,
    pub fill: FillMode,
    pub stroke: bool,
}

/// En utvärderad form.
#[derive(Clone, Debug)]
pub struct Geometry {
    pub paths: Vec<SubPath>,
    /// Textrektangeln (l, t, r, b) i lokala koordinater.
    pub text_rect: (f32, f32, f32, f32),
}

fn presets() -> &'static HashMap<String, Node> {
    static P: OnceLock<HashMap<String, Node>> = OnceLock::new();
    P.get_or_init(|| {
        let mut m = HashMap::new();
        if let Some(root) = xml::parse(PRESETS_XML) {
            for c in root.children {
                m.insert(c.local().to_owned(), c);
            }
        }
        m
    })
}

/// Finns en förinställd form med namnet?
#[allow(dead_code)]
pub fn has_preset(name: &str) -> bool {
    presets().contains_key(name)
}

/// Justeringarnas namn och standardformler (`a:avLst`) för en förinställd form, i ordning
/// (SmartArts `dgm:adj@idx` räknar i den ordningen).
pub fn adjust_names(name: &str) -> Vec<(String, String)> {
    let Some(av) = presets().get(name).and_then(|d| child_local(d, "avLst")) else { return Vec::new() };
    av.children.iter().filter(|c| c.local() == "gd").filter_map(|g| Some((g.attr("name")?.to_owned(), g.attr("fmla").unwrap_or("").to_owned()))).collect()
}

/// Utvärderar en förinställd form. `adjust` är formens egna `a:avLst`-värden (namn → formel).
pub fn preset(name: &str, adjust: Option<&Node>, w: f64, h: f64) -> Option<Geometry> {
    let def = presets().get(name)?;
    Some(evaluate(def, adjust, w, h))
}

/// Utvärderar `a:custGeom` (eller en definition ur tabellen).
pub fn evaluate(def: &Node, adjust: Option<&Node>, w: f64, h: f64) -> Geometry {
    let mut ctx = Ctx::new(w, h);
    // Standardvärden först, sedan formens egna justeringar, sedan guiderna.
    if let Some(av) = child_local(def, "avLst") {
        for gd in av.children.iter().filter(|c| c.local() == "gd") {
            ctx.guide(gd);
        }
    }
    if let Some(av) = adjust {
        for gd in av.children.iter().filter(|c| c.local() == "gd") {
            ctx.guide(gd);
        }
    }
    if let Some(gl) = child_local(def, "gdLst") {
        for gd in gl.children.iter().filter(|c| c.local() == "gd") {
            ctx.guide(gd);
        }
    }
    let text_rect = match child_local(def, "rect") {
        Some(r) => (
            ctx.value(r.attr("l").unwrap_or("l")) as f32,
            ctx.value(r.attr("t").unwrap_or("t")) as f32,
            ctx.value(r.attr("r").unwrap_or("r")) as f32,
            ctx.value(r.attr("b").unwrap_or("b")) as f32,
        ),
        None => (0.0, 0.0, w as f32, h as f32),
    };
    let mut paths = Vec::new();
    if let Some(pl) = child_local(def, "pathLst") {
        for p in pl.children.iter().filter(|c| c.local() == "path") {
            paths.push(ctx.path(p));
        }
    }
    Geometry { paths, text_rect }
}

fn child_local<'a>(n: &'a Node, local: &str) -> Option<&'a Node> {
    n.children.iter().find(|c| c.local() == local)
}

/// 60 000-dels grader → radianer.
fn rad(v: f64) -> f64 {
    (v / 60000.0).to_radians()
}

struct Ctx {
    w: f64,
    h: f64,
    vars: HashMap<String, f64>,
}

impl Ctx {
    fn new(w: f64, h: f64) -> Ctx {
        Ctx { w, h, vars: HashMap::new() }
    }

    fn guide(&mut self, gd: &Node) {
        let (Some(name), Some(f)) = (gd.attr("name"), gd.attr("fmla")) else { return };
        let v = self.formula(f);
        self.vars.insert(name.to_owned(), if v.is_finite() { v } else { 0.0 });
    }

    /// Värdet av en variabel eller ett tal.
    fn value(&self, s: &str) -> f64 {
        if let Some(v) = self.vars.get(s) {
            return *v;
        }
        if let Some(v) = self.builtin(s) {
            return v;
        }
        s.trim().parse::<f64>().unwrap_or(0.0)
    }

    fn builtin(&self, s: &str) -> Option<f64> {
        let (w, h) = (self.w, self.h);
        let ss = w.min(h);
        Some(match s {
            "w" | "r" => w,
            "h" | "b" => h,
            "t" | "l" => 0.0,
            "hc" => w / 2.0,
            "vc" => h / 2.0,
            "ss" => ss,
            "ls" => w.max(h),
            "cd2" => 10_800_000.0,
            "cd4" => 5_400_000.0,
            "cd8" => 2_700_000.0,
            "3cd4" => 16_200_000.0,
            "3cd8" => 8_100_000.0,
            "5cd8" => 13_500_000.0,
            "7cd8" => 18_900_000.0,
            _ => {
                if let Some(n) = s.strip_prefix("wd") {
                    w / n.parse::<f64>().ok()?
                } else if let Some(n) = s.strip_prefix("hd") {
                    h / n.parse::<f64>().ok()?
                } else if let Some(n) = s.strip_prefix("ssd") {
                    ss / n.parse::<f64>().ok()?
                } else {
                    return None;
                }
            }
        })
    }

    fn formula(&self, f: &str) -> f64 {
        let mut it = f.split_whitespace();
        let op = it.next().unwrap_or("val");
        let args: Vec<f64> = it.map(|a| self.value(a)).collect();
        let a = |i: usize| args.get(i).copied().unwrap_or(0.0);
        let div = |x: f64, y: f64| if y == 0.0 { 0.0 } else { x / y };
        match op {
            "*/" => div(a(0) * a(1), a(2)),
            "+-" => a(0) + a(1) - a(2),
            "+/" => div(a(0) + a(1), a(2)),
            "?:" => {
                if a(0) > 0.0 {
                    a(1)
                } else {
                    a(2)
                }
            }
            "abs" => a(0).abs(),
            "at2" => a(1).atan2(a(0)).to_degrees() * 60000.0,
            "cat2" => a(0) * a(2).atan2(a(1)).cos(),
            "cos" => a(0) * rad(a(1)).cos(),
            "max" => a(0).max(a(1)),
            "min" => a(0).min(a(1)),
            "mod" => (a(0) * a(0) + a(1) * a(1) + a(2) * a(2)).sqrt(),
            "pin" => {
                if a(1) < a(0) {
                    a(0)
                } else if a(1) > a(2) {
                    a(2)
                } else {
                    a(1)
                }
            }
            "sat2" => a(0) * a(2).atan2(a(1)).sin(),
            "sin" => a(0) * rad(a(1)).sin(),
            "sqrt" => a(0).max(0.0).sqrt(),
            "tan" => a(0) * rad(a(1)).tan(),
            "val" => a(0),
            _ => 0.0,
        }
    }

    fn path(&self, p: &Node) -> SubPath {
        let pw = p.attr_f64("w").filter(|v| *v > 0.0);
        let ph = p.attr_f64("h").filter(|v| *v > 0.0);
        let sx = pw.map_or(1.0, |pw| self.w / pw);
        let sy = ph.map_or(1.0, |ph| self.h / ph);
        let fill = match p.attr("fill") {
            Some("none") => FillMode::None,
            Some("lighten") => FillMode::Lighten,
            Some("lightenLess") => FillMode::LightenLess,
            Some("darken") => FillMode::Darken,
            Some("darkenLess") => FillMode::DarkenLess,
            _ => FillMode::Norm,
        };
        let stroke = p.attr_bool("stroke").unwrap_or(true);
        let mut out = Path::default();
        let mut cur = (0.0f64, 0.0f64);
        let mut start = (0.0f64, 0.0f64);
        let pt = |n: &Node| -> (f64, f64) {
            match n.children.iter().find(|c| c.local() == "pt") {
                Some(q) => (self.value(q.attr("x").unwrap_or("0")) * sx, self.value(q.attr("y").unwrap_or("0")) * sy),
                None => (0.0, 0.0),
            }
        };
        for c in &p.children {
            match c.local() {
                "moveTo" => {
                    cur = pt(c);
                    start = cur;
                    out.move_to(cur.0 as f32, cur.1 as f32);
                }
                "lnTo" => {
                    if out.cmds.is_empty() {
                        out.move_to(cur.0 as f32, cur.1 as f32);
                    }
                    cur = pt(c);
                    out.line_to(cur.0 as f32, cur.1 as f32);
                }
                "cubicBezTo" => {
                    let pts: Vec<(f64, f64)> = c
                        .children
                        .iter()
                        .filter(|q| q.local() == "pt")
                        .map(|q| (self.value(q.attr("x").unwrap_or("0")) * sx, self.value(q.attr("y").unwrap_or("0")) * sy))
                        .collect();
                    if pts.len() == 3 {
                        if out.cmds.is_empty() {
                            out.move_to(cur.0 as f32, cur.1 as f32);
                        }
                        out.cubic_to(pts[0].0 as f32, pts[0].1 as f32, pts[1].0 as f32, pts[1].1 as f32, pts[2].0 as f32, pts[2].1 as f32);
                        cur = pts[2];
                    }
                }
                "quadBezTo" => {
                    let pts: Vec<(f64, f64)> = c
                        .children
                        .iter()
                        .filter(|q| q.local() == "pt")
                        .map(|q| (self.value(q.attr("x").unwrap_or("0")) * sx, self.value(q.attr("y").unwrap_or("0")) * sy))
                        .collect();
                    if pts.len() == 2 {
                        if out.cmds.is_empty() {
                            out.move_to(cur.0 as f32, cur.1 as f32);
                        }
                        out.quad_to(pts[0].0 as f32, pts[0].1 as f32, pts[1].0 as f32, pts[1].1 as f32);
                        cur = pts[1];
                    }
                }
                "arcTo" => {
                    // Radierna skalas inte med banans w/h i POI; ECMA säger att de är i banans
                    // koordinatsystem, så de skalas här som punkterna.
                    let wr = self.value(c.attr("wR").unwrap_or("0")) * sx;
                    let hr = self.value(c.attr("hR").unwrap_or("0")) * sy;
                    let st = self.value(c.attr("stAng").unwrap_or("0"));
                    let sw = self.value(c.attr("swAng").unwrap_or("0"));
                    if out.cmds.is_empty() {
                        out.move_to(cur.0 as f32, cur.1 as f32);
                    }
                    cur = arc_to(&mut out, cur, wr, hr, st, sw);
                }
                "close" => {
                    out.close();
                    cur = start;
                }
                _ => {}
            }
        }
        SubPath { path: out, fill, stroke }
    }
}

/// Ritar en ellipsbåge från `cur` med radierna `wr`/`hr`, startvinkel `st` och svep `sw`
/// (60 000-dels grader, medurs). Vinklarna är synliga vinklar; de görs om till ellipsens parameter
/// som i POI:s `ArcToCommand`. Returnerar slutpunkten.
pub fn arc_to(out: &mut Path, cur: (f64, f64), wr: f64, hr: f64, st: f64, sw: f64) -> (f64, f64) {
    if wr.abs() < 1e-9 || hr.abs() < 1e-9 {
        // Urartad båge: en rak linje till slutpunkten.
        return cur;
    }
    let param = |deg: f64| -> f64 {
        let a = deg.to_radians();
        let p = (wr * a.sin()).atan2(hr * a.cos());
        // Samma varv som den synliga vinkeln.
        let mut d = p - a;
        while d > std::f64::consts::PI {
            d -= 2.0 * std::f64::consts::PI;
        }
        while d < -std::f64::consts::PI {
            d += 2.0 * std::f64::consts::PI;
        }
        a + d
    };
    let t0 = param(st / 60000.0);
    let t1 = param((st + sw) / 60000.0);
    let cx = cur.0 - wr * t0.cos();
    let cy = cur.1 - hr * t0.sin();
    let sweep = t1 - t0;
    let n = ((sweep.abs() / (std::f64::consts::FRAC_PI_2)).ceil() as usize).clamp(1, 16);
    let step = sweep / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut a = t0;
    for _ in 0..n {
        let b = a + step;
        let (sa, ca) = a.sin_cos();
        let (sb, cb) = b.sin_cos();
        let p1 = (cx + wr * (ca - k * sa), cy + hr * (sa + k * ca));
        let p2 = (cx + wr * (cb + k * sb), cy + hr * (sb - k * cb));
        let p3 = (cx + wr * cb, cy + hr * sb);
        out.cubic_to(p1.0 as f32, p1.1 as f32, p2.0 as f32, p2.1 as f32, p3.0 as f32, p3.1 as f32);
        a = b;
    }
    (cx + wr * t1.cos(), cy + hr * t1.sin())
}

#[cfg(test)]
mod tests {
    use super::super::scene::Cmd;
    use super::*;

    #[test]
    fn alla_former_tolkas() {
        let p = presets();
        assert_eq!(p.len(), 187, "{} former", p.len());
        for (name, def) in p {
            let g = evaluate(def, None, 200.0, 100.0);
            assert!(!g.paths.is_empty(), "{name} saknar banor");
            for sp in &g.paths {
                for c in &sp.path.cmds {
                    let ok = match *c {
                        Cmd::Move(x, y) | Cmd::Line(x, y) => x.is_finite() && y.is_finite(),
                        Cmd::Quad(a, b, x, y) => [a, b, x, y].iter().all(|v| v.is_finite()),
                        Cmd::Cubic(a, b, c2, d, x, y) => [a, b, c2, d, x, y].iter().all(|v| v.is_finite()),
                        Cmd::Close => true,
                    };
                    assert!(ok, "{name}: ogiltig punkt");
                }
            }
        }
        assert!(has_preset("upArrow") && has_preset("rect") && has_preset("roundRect"));
    }

    #[test]
    fn rektangel_och_ellips() {
        let g = preset("rect", None, 100.0, 50.0).unwrap();
        let b = g.paths[0].path.bounds().unwrap();
        assert_eq!(b, (0.0, 0.0, 100.0, 50.0));
        assert_eq!(g.text_rect, (0.0, 0.0, 100.0, 50.0));
        let e = preset("ellipse", None, 100.0, 50.0).unwrap();
        let (x0, y0, x1, y1) = e.paths[0].path.bounds().unwrap();
        assert!(x0.abs() < 0.01 && (x1 - 100.0).abs() < 0.01 && y0.abs() < 0.01 && (y1 - 50.0).abs() < 0.01, "{x0} {y0} {x1} {y1}");
        // Ellipsens textrektangel är den inskrivna rektangeln.
        let (l, t, r, _) = e.text_rect;
        assert!((l - 14.64).abs() < 0.1 && (t - 7.32).abs() < 0.1 && (r - 85.36).abs() < 0.1, "{:?}", e.text_rect);
    }

    #[test]
    fn justering_andrar_rundad_rektangel() {
        let def = preset("roundRect", None, 100.0, 100.0).unwrap();
        // Standard adj = 16667 → hörnradie 16,667.
        let first = def.paths[0].path.cmds[0];
        assert!(matches!(first, Cmd::Move(x, y) if x.abs() < 0.01 && (y - 16.667).abs() < 0.01), "{first:?}");
        let av = xml::parse(br#"<a:avLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:gd name="adj" fmla="val 50000"/></a:avLst>"#).unwrap();
        let g = preset("roundRect", Some(&av), 100.0, 100.0).unwrap();
        assert!(matches!(g.paths[0].path.cmds[0], Cmd::Move(x, y) if x.abs() < 0.01 && (y - 50.0).abs() < 0.01));
    }

    #[test]
    fn pie_textrektangel_ratt() {
        let g = preset("pie", None, 100.0, 100.0).unwrap();
        let (l, t, r, b) = g.text_rect;
        assert!(l < r && t < b, "{:?}", g.text_rect);
    }

    #[test]
    fn egen_form_med_banstorlek() {
        let cg = xml::parse(br#"<a:custGeom xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:avLst/><a:gdLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst><a:path w="10" h="10"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="10" y="5"/></a:lnTo><a:lnTo><a:pt x="0" y="10"/></a:lnTo><a:close/></a:path></a:pathLst></a:custGeom>"#).unwrap();
        let g = evaluate(&cg, None, 200.0, 100.0);
        assert_eq!(g.paths[0].path.bounds().unwrap(), (0.0, 0.0, 200.0, 100.0));
    }
}
