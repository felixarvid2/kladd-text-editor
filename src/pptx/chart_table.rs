//! Datatabellen under ritytan (`c:dTable`): en rubrikrad med kategorierna (i kategoriernas band, i stället
//! för kategorietiketterna) och en rad per serie med seriens namn (och förklaringsnyckel) till vänster och
//! värdena i seriens talformat. Kantlinjer efter `showHorzBorder`/`showVertBorder`/`showOutline`.
//! Undermodul till `chart_draw.rs` (delar `Draw`).

use super::super::chart_fmt;
use super::super::chart_model::{Chart, DataTable, Kind};
use super::super::scene::{Cap, Join, Paint, Path, Stroke};
use super::super::text::{Align, RunStyle};
use super::{lum, Draw, R};

/// Datatabellen med kategorierna och textstilen.
pub(super) struct Table<'n> {
    pub t: DataTable<'n>,
    pub cats: Vec<String>,
    pub style: RunStyle,
}

impl Table<'_> {
    fn row_h(&self) -> f32 {
        self.style.size * 1.5
    }

    fn key_w(&self) -> f32 {
        if self.t.keys { self.style.size * 1.3 } else { 0.0 }
    }

    /// (namnkolumnens bredd, tabellens höjd).
    pub fn size(&self, d: &Draw<'_, '_>, c: &Chart<'_>) -> (f32, f32) {
        let names = c.plots.iter().flat_map(|p| p.series.iter()).map(|s| d.lay(Draw::plain_paras(&s.name, &self.style, Align::Left), f32::INFINITY).w).fold(0.0, f32::max);
        let rows = c.plots.iter().map(|p| p.series.len()).sum::<usize>() + 1;
        (names + self.key_w() + 0.8 * self.style.size, self.row_h() * rows as f32)
    }
}

impl Draw<'_, '_> {
    pub(super) fn data_table(&mut self, c: &Chart<'_>, t: &Table<'_>, inner: R, band: f32, cpos: &dyn Fn(f32) -> f32) {
        let (name_w, h) = t.size(self, c);
        let rh = t.row_h();
        let s = t.style.size;
        let top = inner.bottom();
        let left = inner.x - name_w;
        let total: usize = c.plots.iter().map(|p| p.series.len()).sum();
        let ncat = t.cats.len().max(c.plots.iter().flat_map(|p| p.series.iter()).map(|s| s.vals.len()).max().unwrap_or(0));
        // Kantlinjerna.
        let grid_def = Stroke { paint: Paint::Solid(lum(self.scheme("tx1"), 0.15, 0.85)), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Miter };
        let bounds = R { x: left, y: top, w: inner.right() - left, h };
        if let Some(st) = self.line_of(t.t.sppr, bounds).unwrap_or(Some(grid_def)) {
            let mut path = Path::default();
            if t.t.horz {
                for r in 1..=(total + 1) {
                    let y = top + rh * r as f32;
                    path.move_to(left, y);
                    path.line_to(inner.right(), y);
                }
                path.move_to(inner.x, top);
                path.line_to(inner.right(), top);
            }
            if t.t.vert {
                for k in 0..=ncat {
                    let x = cpos(k as f32 - 0.5);
                    path.move_to(x, top);
                    path.line_to(x, top + h);
                }
            }
            if t.t.outline {
                path.move_to(left, top + rh);
                path.line_to(left, top + h);
                path.line_to(inner.right(), top + h);
                path.line_to(inner.right(), top);
                path.line_to(inner.x, top);
                path.line_to(inner.x, top + rh);
                path.line_to(left, top + rh);
            }
            self.stroke(path, st);
        }
        // Rubrikraden: kategorierna.
        for (i, cat) in t.cats.iter().enumerate().take(ncat) {
            let l = self.lay(Self::plain_paras(cat, &t.style, Align::Center), band * 0.96);
            self.place(l, cpos(i as f32), top + rh / 2.0, 0.5, 0.5, 0.0);
        }
        // En rad per serie.
        let mut row = 1;
        for p in &c.plots {
            for ser in &p.series {
                let y = top + rh * row as f32 + rh / 2.0;
                let mut x = left + 0.3 * s;
                if t.t.keys {
                    let col = self.series_paint(c, p, ser, None, total, ncat);
                    let k = 0.55 * s;
                    if matches!(p.kind, Kind::Line | Kind::Scatter) {
                        let mut path = Path::default();
                        path.move_to(x, y);
                        path.line_to(x + 1.0 * s, y);
                        let def = Stroke { paint: Paint::Solid(col), width: 2.25, dash: Vec::new(), cap: Cap::Round, join: Join::Round };
                        if let Some(st) = self.line_of(ser.sppr, bounds).unwrap_or(Some(def)) {
                            self.stroke(path, st);
                        }
                    } else {
                        let r = R { x, y: y - k / 2.0, w: k, h: k };
                        self.shape(Path::rect(r.x, r.y, r.w, r.h), r, ser.sppr, Some(Paint::Solid(col)), None);
                    }
                    x += t.key_w();
                }
                let l = self.lay(Self::plain_paras(&ser.name, &t.style, Align::Left), f32::INFINITY);
                self.place(l, x, y, 0.0, 0.5, 0.0);
                for i in 0..ncat {
                    let Some(v) = ser.vals.get(i).copied().flatten() else { continue };
                    let l = self.lay(Self::plain_paras(&chart_fmt::format(v, &ser.format), &t.style, Align::Center), f32::INFINITY);
                    self.place(l, cpos(i as f32), y, 0.5, 0.5, 0.0);
                }
                row += 1;
            }
        }
    }
}
