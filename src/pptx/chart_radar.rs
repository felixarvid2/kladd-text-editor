//! Radardiagram (`c:radarChart`): en eker per kategori medurs från klockan tolv, värdeaxeln längs den
//! första ekern, rutnätet som månghörningar (värdeaxelns `majorGridlines`) och ekrar (kategoriaxelns),
//! serierna som slutna linjer (`radarStyle` `standard`/`marker`) eller fyllda ytor (`filled`).
//! Undermodul till `chart_draw.rs` (delar `Draw`).

use super::super::chart_fmt;
use super::super::chart_model::{Axis, Chart, Kind, Plot};
use super::super::scene::{Cap, Join, Paint, Path, Stroke};
use super::super::text::Align;
use super::{auto_scale, lum, polyline, Draw, R};

/// Punkten för värdet `rho` (radie) på eker `i` av `n`.
pub fn spoke(cx: f32, cy: f32, rho: f32, i: usize, n: usize) -> (f32, f32) {
    let t = (i as f32 / n.max(1) as f32 * 360.0).to_radians();
    (cx + rho * t.sin(), cy - rho * t.cos())
}

impl Draw<'_, '_> {
    pub(super) fn radar(&mut self, c: &Chart<'_>, area: R) {
        let plots: Vec<&Plot<'_>> = c.plots.iter().filter(|p| matches!(p.kind, Kind::Radar { .. })).collect();
        let Some(first) = plots.first() else { return };
        let find = |id: &str| c.axes.iter().find(|a| a.id == id);
        let cat_ax: Option<&Axis<'_>> = first.ax_ids.iter().filter_map(|id| find(id)).find(|a| a.kind != "valAx");
        let val_ax: Option<&Axis<'_>> = first.ax_ids.iter().filter_map(|id| find(id)).find(|a| a.kind == "valAx");
        let n = plots.iter().flat_map(|p| p.series.iter()).map(|s| s.vals.len().max(s.cats.len())).max().unwrap_or(0);
        if n < 2 {
            return;
        }
        let cats: Vec<String> = plots.iter().flat_map(|p| p.series.iter()).map(|s| s.cats.clone()).find(|v| !v.is_empty()).unwrap_or_else(|| (1..=n).map(|i| i.to_string()).collect());
        let (lo, hi, _) = super::value_range(plots.iter().copied(), n);
        let cat_style = self.style(&[cat_ax.and_then(|a| a.txpr), c.space_txpr], 1000);
        let val_style = self.style(&[val_ax.and_then(|a| a.txpr), c.space_txpr], 1000);
        let (cs, vs) = (cat_style.size, val_style.size);
        let show_cat = cat_ax.is_none_or(|a| !a.deleted && a.tick_lbl_pos != "none");
        let show_val = val_ax.is_some_and(|a| !a.deleted && a.tick_lbl_pos != "none");
        // Plats för kategorietiketterna runt om.
        let labels: Vec<super::Label> = if show_cat { cats.iter().take(n).map(|t| self.lay(Self::plain_paras(t, &cat_style, Align::Center), f32::INFINITY)).collect() } else { Vec::new() };
        let lw = labels.iter().map(|l| l.w).fold(0.0, f32::max);
        let lh = labels.iter().map(|l| l.h).fold(0.0, f32::max);
        let r = ((area.w / 2.0 - lw - 0.5 * cs).min(area.h / 2.0 - lh - 0.5 * cs)).max(5.0);
        let (cx, cy) = (area.x + area.w / 2.0, area.y + area.h / 2.0);
        let (fmin, fmax, fstep) = val_ax.map_or((None, None, None), |a| (a.min, a.max, a.major));
        let scale = auto_scale(lo, hi, fmin, fmax, fstep, &|mn, mx, step| (r as f64) / ((mx - mn) / step) >= 1.5 * vs as f64 * 0.98 || (mx - mn) / step <= 2.0);
        let rad = move |v: f64| -> f32 { (((v - scale.min) / (scale.max - scale.min)) as f32).clamp(0.0, 1.0) * r };
        let bounds = R { x: cx - r, y: cy - r, w: 2.0 * r, h: 2.0 * r };

        // Rutnät: månghörningar vid strecken och ekrar.
        let grid_def = Stroke { paint: Paint::Solid(lum(self.scheme("tx1"), 0.15, 0.85)), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Round };
        if let Some(sp) = val_ax.and_then(|a| a.major_grid) {
            if let Some(st) = self.line_of(sp, bounds).unwrap_or(Some(grid_def.clone())) {
                let mut path = Path::default();
                for t in scale.ticks() {
                    let rr = rad(t);
                    for i in 0..=n {
                        let (x, y) = spoke(cx, cy, rr, i % n, n);
                        if i == 0 { path.move_to(x, y) } else { path.line_to(x, y) }
                    }
                }
                self.stroke(path, st);
            }
        }
        let spokes = cat_ax.map_or(Some(None), |a| a.major_grid);
        if let Some(sp) = spokes {
            if let Some(st) = self.line_of(sp, bounds).unwrap_or(Some(grid_def.clone())) {
                let mut path = Path::default();
                for i in 0..n {
                    let (x, y) = spoke(cx, cy, r, i, n);
                    path.move_to(cx, cy);
                    path.line_to(x, y);
                }
                self.stroke(path, st);
            }
        }

        // Serierna.
        let total: usize = plots.iter().map(|p| p.series.len()).sum();
        for p in &plots {
            let filled = p.kind == (Kind::Radar { filled: true });
            for ser in &p.series {
                let col = self.series_paint(c, p, ser, None, total, n);
                let pts: Vec<Option<(f32, f32)>> = (0..n).map(|i| ser.vals.get(i).copied().flatten().map(|v| spoke(cx, cy, rad(v), i, n))).collect();
                if filled {
                    let mut path = Path::default();
                    for (i, q) in pts.iter().enumerate() {
                        let (x, y) = q.unwrap_or((cx, cy));
                        if i == 0 { path.move_to(x, y) } else { path.line_to(x, y) }
                    }
                    path.close();
                    self.shape(path, bounds, ser.sppr, Some(Paint::Solid(col)), None);
                } else {
                    let def = Stroke { paint: Paint::Solid(col), width: 2.25, dash: Vec::new(), cap: Cap::Round, join: Join::Round };
                    let st = self.line_of(ser.sppr, bounds).unwrap_or_else(|| {
                        let w = ser.sppr.and_then(|s| s.child("a:ln")).and_then(|l| l.attr_i64("w")).map(super::emu);
                        Some(Stroke { width: w.unwrap_or(2.25), ..def })
                    });
                    if let Some(st) = st {
                        // Sluten: tillbaka till första punkten.
                        let mut closed = pts.clone();
                        closed.push(pts[0]);
                        self.stroke(polyline(&closed, false), st);
                    }
                    self.markers(p, ser, &pts, col, total);
                }
            }
        }

        // Värdeaxeln längs den första ekern, med etiketterna till vänster.
        if let Some(ax) = val_ax.filter(|a| !a.deleted) {
            let axis_def = Stroke { paint: Paint::Solid(lum(self.scheme("tx1"), 0.25, 0.75)), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Round };
            if let Some(st) = self.line_of(ax.sppr, bounds).unwrap_or(Some(axis_def)) {
                let mut path = Path::default();
                path.move_to(cx, cy);
                path.line_to(cx, cy - r);
                self.stroke(path, st);
            }
            if show_val {
                let code = if !ax.source_linked { ax.num_fmt.clone().unwrap_or_else(|| "General".into()) } else { first.series.first().map(|s| s.format.clone()).unwrap_or_else(|| "General".into()) };
                for t in scale.ticks() {
                    let l = self.lay(Self::plain_paras(&chart_fmt::format(t, &code), &val_style, Align::Right), f32::INFINITY);
                    self.place(l, cx - 0.3 * vs, cy - rad(t), 1.0, 0.5, 0.0);
                }
            }
        }
        // Kategorietiketterna utanför ekrarna.
        for (i, l) in labels.into_iter().enumerate() {
            let t = (i as f32 / n as f32 * 360.0).to_radians();
            let (x, y) = spoke(cx, cy, r + 0.4 * cs, i, n);
            self.place(l, x, y, 0.5 - 0.5 * t.sin(), 0.5 + 0.5 * t.cos(), 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::spoke;

    #[test]
    fn ekrarna_medurs_fran_klockan_tolv() {
        let near = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4;
        assert!(near(spoke(0.0, 0.0, 10.0, 0, 4), (0.0, -10.0)));
        assert!(near(spoke(0.0, 0.0, 10.0, 1, 4), (10.0, 0.0)));
        assert!(near(spoke(0.0, 0.0, 10.0, 2, 4), (0.0, 10.0)));
        assert!(near(spoke(0.0, 0.0, 10.0, 3, 4), (-10.0, 0.0)));
    }
}
