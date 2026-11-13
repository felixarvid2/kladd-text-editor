//! Rastrering av en formel till en alfamask (nollskild lindning, 4×4 delprov). Används av
//! dokumentläget och förhandsvisningen, som tonar masken med textfärgen.

use super::font::font;
use super::layout::{Item, Layout};
use ttf_parser::OutlineBuilder;

pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    /// En byte täckning per pixel, rad för rad uppifrån.
    pub alpha: Vec<u8>,
    /// Baslinjens avstånd från överkanten (px).
    pub baseline: f32,
}

#[derive(Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// Samlar kontur som linjesegment (pixelrymd, y nedåt).
struct Flatten<'a> {
    edges: &'a mut Vec<Edge>,
    ox: f32,
    oy: f32,
    k: f32,
    start: (f32, f32),
    cur: (f32, f32),
}

impl Flatten<'_> {
    fn pt(&self, x: f32, y: f32) -> (f32, f32) {
        (self.ox + x * self.k, self.oy - y * self.k)
    }

    fn seg(&mut self, to: (f32, f32)) {
        let (a, b) = (self.cur, to);
        if a != b {
            self.edges.push(Edge { x0: a.0, y0: a.1, x1: b.0, y1: b.1 });
        }
        self.cur = to;
    }
}

impl OutlineBuilder for Flatten<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = self.pt(x, y);
        self.cur = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.seg(p);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, p1, p2) = (self.cur, self.pt(x1, y1), self.pt(x, y));
        const N: usize = 8;
        for i in 1..=N {
            let t = i as f32 / N as f32;
            let u = 1.0 - t;
            let q = (u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0, u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1);
            self.seg(q);
        }
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p0, p1, p2, p3) = (self.cur, self.pt(x1, y1), self.pt(x2, y2), self.pt(x, y));
        const N: usize = 10;
        for i in 1..=N {
            let t = i as f32 / N as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.seg((a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1));
        }
    }

    fn close(&mut self) {
        let s = self.start;
        self.seg(s);
    }
}

/// Rastrerar `l` med `ppe` pixlar per em och `pad` pixlar marginal runt om.
pub fn rasterize(l: &Layout, ppe: f32, pad: f32) -> Bitmap {
    let w = ((l.w * ppe).ceil() + 2.0 * pad).max(1.0) as usize;
    let h = (((l.asc + l.desc) * ppe).ceil() + 2.0 * pad).max(1.0) as usize;
    let baseline = pad + l.asc * ppe;
    let mut edges: Vec<Edge> = Vec::new();
    if let Some(f) = font() {
        for it in &l.items {
            match *it {
                Item::Glyph { gid, x, y, s } => {
                    let mut fl = Flatten {
                        edges: &mut edges,
                        ox: pad + x * ppe,
                        oy: baseline - y * ppe,
                        k: s * ppe / f.upem,
                        start: (0.0, 0.0),
                        cur: (0.0, 0.0),
                    };
                    f.outline(gid, &mut fl);
                }
                Item::Rule { x, y, w: rw, h: rh } => {
                    let (x0, x1) = (pad + x * ppe, pad + (x + rw) * ppe);
                    let (y1, y0) = (baseline - y * ppe, baseline - (y + rh) * ppe);
                    // Streck tunnare än en halv pixel blir en halv pixel, så att de syns.
                    let (y0, y1) = if y1 - y0 < 0.6 { (y0 - (0.6 - (y1 - y0)) / 2.0, y1 + (0.6 - (y1 - y0)) / 2.0) } else { (y0, y1) };
                    for (a, b) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
                        edges.push(Edge { x0: a.0, y0: a.1, x1: b.0, y1: b.1 });
                    }
                }
            }
        }
    }
    let mut cov = vec![0.0f32; w * h];
    const SUB: usize = 4;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    // Kanter sorterade på överkant för att hoppa över dem som ligger under raden.
    let mut es: Vec<(f32, f32, f32, f32, i32)> = edges
        .iter()
        .filter(|e| e.y0 != e.y1)
        .map(|e| if e.y0 < e.y1 { (e.y0, e.y1, e.x0, e.x1, 1) } else { (e.y1, e.y0, e.x1, e.x0, -1) })
        .collect();
    es.sort_by(|a, b| a.0.total_cmp(&b.0));
    for row in 0..h {
        for sub in 0..SUB {
            let sy = row as f32 + (sub as f32 + 0.5) / SUB as f32;
            xs.clear();
            for &(ya, yb, xa, xb, dir) in &es {
                if ya > sy {
                    break;
                }
                if sy < yb {
                    let t = (sy - ya) / (yb - ya);
                    xs.push((xa + (xb - xa) * t, dir));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            for i in 0..xs.len() {
                wind += xs[i].1;
                if wind != 0 && i + 1 < xs.len() {
                    let (a, b) = (xs[i].0.max(0.0), xs[i + 1].0.min(w as f32));
                    if b > a {
                        add_span(&mut cov[row * w..(row + 1) * w], a, b, 1.0 / SUB as f32);
                    }
                }
            }
        }
    }
    let alpha = cov.iter().map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
    Bitmap { w, h, alpha, baseline }
}

/// Lägger täckningen `amount` (per pixel, bråkdel i ändarna) för spannet [a, b) på raden.
fn add_span(row: &mut [f32], a: f32, b: f32, amount: f32) {
    let (ia, ib) = (a.floor() as usize, b.floor() as usize);
    if ia >= row.len() {
        return;
    }
    if ia == ib {
        row[ia] += (b - a) * amount;
        return;
    }
    row[ia] += (ia as f32 + 1.0 - a) * amount;
    let end = ib.min(row.len());
    for p in row.iter_mut().take(end).skip(ia + 1) {
        *p += amount;
    }
    if ib < row.len() {
        row[ib] += (b - ib as f32) * amount;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{layout::layout, parse::parse};

    fn bmp(src: &str, ppe: f32) -> Bitmap {
        rasterize(&layout(&parse(src).unwrap(), true).unwrap(), ppe, 2.0)
    }

    /// Skriver formlerna i `KLADD_MATH_DUMP/in.txt` (en per rad, `i:` = löpande) som `out.png`, för ögonkontroll.
    #[test]
    #[ignore]
    fn dump_png() {
        let Ok(dir) = std::env::var("KLADD_MATH_DUMP") else { return };
        let src = std::fs::read_to_string(format!("{dir}/in.txt")).unwrap();
        let mut rows: Vec<Bitmap> = Vec::new();
        for line in src.lines().filter(|l| !l.trim().is_empty()) {
            let display = !line.starts_with("i:");
            let l = line.trim_start_matches("i:");
            rows.push(rasterize(&layout(&parse(l).unwrap_or_else(|e| panic!("{l}: {e}")), display).unwrap(), 48.0, 6.0));
        }
        let w = rows.iter().map(|b| b.w).max().unwrap_or(1);
        let h: usize = rows.iter().map(|b| b.h).sum();
        let mut img = image::GrayImage::from_pixel(w as u32, h as u32, image::Luma([255]));
        let mut y0 = 0;
        for b in &rows {
            for y in 0..b.h {
                for x in 0..b.w {
                    img.put_pixel(x as u32, (y0 + y) as u32, image::Luma([255 - b.alpha[y * b.w + x]]));
                }
            }
            y0 += b.h;
        }
        img.save(format!("{dir}/out.png")).unwrap();
    }

    #[test]
    fn rasterizes_ink_inside_the_bitmap() {
        let b = bmp(r"\frac{a}{b}+\sqrt{x^2}", 32.0);
        assert!(b.w > 60 && b.h > 30);
        assert_eq!(b.alpha.len(), b.w * b.h);
        let ink: usize = b.alpha.iter().filter(|a| **a > 128).count();
        assert!(ink > 200, "ink {ink}");
        // Marginalen är tom.
        assert!(b.alpha[..b.w].iter().all(|a| *a == 0));
        assert!(b.baseline > 2.0 && b.baseline < b.h as f32);
    }

    #[test]
    fn rule_is_solid_and_holes_stay_open() {
        let b = bmp(r"\overline{o}", 40.0);
        // Det finns en rad där nästan hela bredden är täckt (streck).
        let full = (0..b.h).any(|y| b.alpha[y * b.w..(y + 1) * b.w].iter().filter(|a| **a > 200).count() as f32 > b.w as f32 * 0.5);
        assert!(full);
        // Mitten av 'o' är hålig: någon rad har både mörk-ljus-mörk.
        let hole = (0..b.h).any(|y| {
            let r = &b.alpha[y * b.w..(y + 1) * b.w];
            let on: Vec<usize> = r.iter().enumerate().filter(|(_, a)| **a > 200).map(|(i, _)| i).collect();
            on.windows(2).any(|w| w[1] - w[0] > 6) && on.len() < b.w / 2
        });
        assert!(hole);
    }
}
