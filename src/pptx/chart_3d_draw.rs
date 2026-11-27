//! Ritningen av 3D-diagrammen (`chart_draw.rs` har den som undermodul och delar `Draw`): 3D-cirkeln
//! och stapel-, linje- och ytdiagram i en 3D-låda med golv och väggar. Geometrin, kameran och
//! belysningen finns i `chart_3d.rs`; här blir ytorna fyllda banor i seriens färg × ytans ljushet.

use super::super::chart_3d::{self as d3, BarShape, Camera, Face, FaceKind, Pie, Poly, Slice, V3};
use super::super::chart_fmt;
use super::super::chart_model::{Axis, Chart, Grouping, Kind, Plot};
use super::super::color::Rgba;
use super::super::scene::{Cap, Join, Paint, Path, Stop, Stroke};
use super::super::text::Align;
use super::{auto_scale, lum, Draw, Scale, R};

/// Cirkelns tjocklek som andel av radien vid `hPercent` 100 och cirkelns storlek som andel av det
/// största som får plats i ritytan. Valda mot PowerPoints PDF för de två 3D-cirklarna i materialet
/// (`msrc-win32k` bild 8, `rotX` 30, och `cppcon17-inherited-codebase-mac` bild 7, `rotX` 50): uppmätt
/// tjocklek 0,22–0,28 r, och 0,30 / 0,84 gav bäst jämförelsemått för båda.
const PIE_HEIGHT: f32 = 0.30;
const PIE_FIT: f32 = 0.84;

/// Färgen `c` med ljusheten `k` (över 1 = mot vitt).
pub(crate) fn shaded(c: Rgba, k: f32) -> Rgba {
    if k <= 1.0 {
        let k = k.max(0.0);
        Rgba { r: c.r * k, g: c.g * k, b: c.b * k, a: c.a }
    } else {
        let t = (k - 1.0).min(1.0);
        Rgba { r: c.r + (1.0 - c.r) * t, g: c.g + (1.0 - c.g) * t, b: c.b + (1.0 - c.b) * t, a: c.a }
    }
}

fn poly_path(pts: &[(f32, f32)]) -> Path {
    let mut p = Path::default();
    if let Some((&(x, y), rest)) = pts.split_first() {
        p.move_to(x, y);
        for &(x, y) in rest {
            p.line_to(x, y);
        }
        p.close();
    }
    p
}

fn bounds(pts: &[(f32, f32)]) -> R {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for &(x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if x0 > x1 { R { x: 0.0, y: 0.0, w: 1.0, h: 1.0 } } else { R { x: x0, y: y0, w: (x1 - x0).max(0.01), h: (y1 - y0).max(0.01) } }
}

impl Draw<'_, '_> {
    /// Fyller en yta i färgen `base` skuggad enligt ytan (vågrät övertoning för cirkelns sidor).
    fn face(&mut self, f: &Face, base: Rgba) {
        let path = poly_path(&f.pts);
        if f.shade_x.len() >= 2 {
            let (x0, x1) = (f.shade_x[0].0, f.shade_x[f.shade_x.len() - 1].0);
            if x1 - x0 > 0.5 {
                let (ax, ay) = self.f.t.apply(x0, 0.0);
                let (bx, by) = self.f.t.apply(x1, 0.0);
                let stops = f.shade_x.iter().map(|&(x, k)| Stop { pos: ((x - x0) / (x1 - x0)).clamp(0.0, 1.0), color: shaded(base, k) }).collect();
                self.fill(path, Paint::Linear { x0: ax, y0: ay, x1: bx, y1: by, stops });
                return;
            }
        }
        self.fill(path, Paint::Solid(shaded(base, f.shade)));
    }

    // ---- 3D-cirkel ----

    pub(super) fn pie3d(&mut self, c: &Chart<'_>, p: &Plot<'_>, area: R) {
        let Some(ser) = p.series.first() else { return };
        let vals: Vec<f64> = ser.vals.iter().map(|v| v.unwrap_or(0.0).abs()).collect();
        let total: f64 = vals.iter().sum();
        if total <= 0.0 {
            return;
        }
        let n = vals.len();
        let v = c.view3d.unwrap_or_default();
        let rot_x = v.rot_x.unwrap_or(30.0).clamp(10.0, 90.0);
        let tilt = rot_x.to_radians().sin();
        let cos_x = rot_x.to_radians().cos();
        let k_h = PIE_HEIGHT * v.h_percent.unwrap_or(100.0) / 100.0;
        let expl = |i: usize| ser.point(i).and_then(|d| d.explosion).unwrap_or(ser.explosion).clamp(0.0, 400.0) / 100.0;
        let max_e = (0..n).map(expl).fold(0.0f32, f32::max);
        let r = PIE_FIT * (area.w / (2.0 * (1.0 + max_e))).min(area.h / (2.0 * tilt * (1.0 + max_e) + k_h * cos_x));
        if r <= 0.5 {
            return;
        }
        let pie = Pie { cx: area.x + area.w / 2.0, cy: area.y + area.h / 2.0 - k_h * r * cos_x / 2.0, r, thickness: k_h * r, rot_x };
        let first = v.rot_y.unwrap_or(p.first_angle);
        let mut slices = Vec::with_capacity(n);
        let mut a = first;
        for (i, val) in vals.iter().enumerate() {
            let sweep = (*val / total) as f32 * 360.0;
            slices.push(Slice { start: a, sweep, explosion: expl(i) });
            a += sweep;
        }
        let colors: Vec<Rgba> = (0..n).map(|i| if p.vary_colors { self.auto_color(c.style, i, n) } else { self.auto_color(c.style, ser.idx, 1) }).collect();
        let sppr = |i: usize| ser.point(i).and_then(|x| x.sppr).or(ser.sppr);
        let faces = pie.faces(&slices);
        let full = R { x: pie.cx - r, y: pie.cy - r * tilt, w: 2.0 * r, h: 2.0 * r * tilt };
        for f in &faces {
            let i = f.owner;
            let base = self.fill_of(sppr(i), full).flatten().map(|p| p.average()).unwrap_or(colors[i]);
            match f.kind {
                FaceKind::Top => self.shape(poly_path(&f.pts), full, sppr(i), Some(Paint::Solid(colors[i])), None),
                _ => {
                    self.face(f, base);
                    // Linjen runt snittytorna (PowerPoint ramar in alla ytor med seriens linje).
                    if f.kind == FaceKind::Cut {
                        if let Some(Some(st)) = self.line_of(sppr(i), full) {
                            self.stroke(poly_path(&f.pts), st);
                        }
                    }
                }
            }
        }
        // Dataetiketter.
        for i in 0..n {
            let l = ser.labels_for(i);
            if !l.any() {
                continue;
            }
            let s = slices[i];
            let mid = s.start + s.sweep / 2.0;
            let off = pie.offset(&s);
            let Some(lbl) = self.data_label(c, p, ser, i, l, Some(vals[i] / total), true) else { continue };
            let pos = l.pos.as_deref().unwrap_or("bestFit");
            let t = mid.to_radians();
            let front = t.cos() < 0.0;
            let (lw, lh) = (lbl.w, lbl.h);
            let outside = |rr: f32| {
                let (x, y, _) = pie.point(mid, rr, if front { -pie.thickness } else { 0.0 }, off);
                (x, y, 0.5 - 0.5 * t.sin(), 0.5 + 0.5 * t.cos())
            };
            let inside = |rr: f32| {
                let (x, y, _) = pie.point(mid, rr, 0.0, off);
                (x, y, 0.5, 0.5)
            };
            let (x, y, ax, ay) = match pos {
                "outEnd" => outside(r * 1.05 + 2.0),
                "inEnd" => inside(r * 0.8),
                "ctr" => inside(r * 0.5),
                _ => {
                    let chord = 2.0 * r * 0.65 * (s.sweep.to_radians() / 2.0).sin().abs();
                    if lw < chord.max(r * 0.6) && lh < r * tilt * 0.5 && s.sweep > 25.0 { inside(r * 0.65) } else { outside(r * 1.05 + 2.0) }
                }
            };
            self.label_box(lbl, l, x, y, ax, ay);
        }
    }

    // ---- Stapel, linje och yta i 3D ----

    /// Ritar stapel-, linje- och ytdiagram med `c:view3D` i en 3D-låda: väggar och golv, rutnät på
    /// väggarna, serierna som lådor/cylindrar/koner/pyramider, band (linje) eller skivor (yta), och
    /// axeletiketterna längs lådans främre kanter.
    pub(super) fn axes_plot_3d(&mut self, c: &Chart<'_>, outer: R, inner_fixed: Option<R>) {
        let first = &c.plots[0];
        let horizontal = matches!(first.kind, Kind::Bar { horizontal: true });
        let find = |id: &str| c.axes.iter().find(|a| a.id == id);
        let cat_ax: Option<&Axis<'_>> = first.ax_ids.iter().filter_map(|id| find(id)).find(|a| a.kind == "catAx" || a.kind == "dateAx");
        let val_ax: Option<&Axis<'_>> = first.ax_ids.iter().filter_map(|id| find(id)).find(|a| a.kind == "valAx");
        let ser_ax: Option<&Axis<'_>> = first.ax_ids.iter().filter_map(|id| find(id)).find(|a| a.kind == "serAx");
        let ncat = c.plots.iter().flat_map(|p| p.series.iter()).map(|s| s.vals.len().max(s.cats.len())).max().unwrap_or(0).max(1);
        let cats: Vec<String> = c.plots.iter().flat_map(|p| p.series.iter()).map(|s| s.cats.clone()).find(|v| !v.is_empty()).unwrap_or_else(|| (1..=ncat).map(|i| i.to_string()).collect());
        // Serier på djupet: `standard`-gruppering för staplar, alltid för linjer, för ytor utom staplade.
        let standard = |p: &Plot<'_>| match p.kind {
            Kind::Bar { .. } => p.grouping == Grouping::Clustered && !p.series.is_empty() && c.axes.iter().any(|a| a.kind == "serAx" && p.ax_ids.contains(&a.id)),
            Kind::Line => true,
            Kind::Area => p.grouping == Grouping::Clustered,
            _ => false,
        };
        let rows: usize = c.plots.iter().map(|p| if standard(p) { p.series.len().max(1) } else { 1 }).sum::<usize>().max(1);

        // Värdeområdet (som i 2D).
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut percent = false;
        for p in &c.plots {
            if p.grouping == Grouping::Percent {
                percent = true;
                lo = lo.min(0.0);
                hi = hi.max(1.0);
                continue;
            }
            if p.grouping == Grouping::Stacked {
                for i in 0..ncat {
                    let (mut pos, mut neg) = (0.0, 0.0);
                    for s in &p.series {
                        let v = s.vals.get(i).copied().flatten().unwrap_or(0.0);
                        if v >= 0.0 { pos += v } else { neg += v }
                    }
                    lo = lo.min(neg);
                    hi = hi.max(pos);
                }
            } else {
                for v in p.series.iter().flat_map(|s| s.vals.iter().flatten()) {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
        }
        if lo > hi {
            (lo, hi) = (0.0, 1.0);
        }
        let val_style = self.style(&[val_ax.and_then(|a| a.txpr), c.space_txpr], 1000);
        let cat_style = self.style(&[cat_ax.and_then(|a| a.txpr), c.space_txpr], 1000);
        let vs = val_style.size;
        let cs = cat_style.size;
        let val_fmt = |v: f64| -> String {
            let code = match val_ax {
                Some(a) if !a.source_linked => a.num_fmt.clone().unwrap_or_else(|| "General".into()),
                _ if percent => "0%".into(),
                _ => first.series.first().map(|s| s.format.clone()).unwrap_or_else(|| "General".into()),
            };
            chart_fmt::format(v, &code)
        };
        let show_val_lbl = val_ax.is_some_and(|a| !a.deleted && a.tick_lbl_pos != "none");
        let show_cat_lbl = cat_ax.is_some_and(|a| !a.deleted && a.tick_lbl_pos != "none");
        let measure = |d: &Self, t: &str, st: &super::RunStyle| -> (f32, f32) {
            let l = d.lay(Self::plain_paras(t, st, Align::Left), f32::INFINITY);
            (l.w, l.h)
        };

        // Kameran och lådans mått. Lådan: bredd 1 (kategorierna), djup ur `depthPercent`, höjd ur
        // `hPercent` (eller vad som får plats vid rätvinkliga axlar).
        let v = c.view3d.unwrap_or_default();
        let right_angle = v.right_angle.unwrap_or(true);
        let rot_x = v.rot_x.unwrap_or(15.0).clamp(-90.0, 90.0);
        let rot_y = v.rot_y.unwrap_or(20.0).rem_euclid(360.0);
        let rot_y = if right_angle { rot_y.min(90.0) } else { rot_y };
        let band_frac = 1.0 / ncat as f32;
        let depth = (band_frac * rows as f32 * v.depth / 100.0).clamp(0.02, 4.0);
        let height = v.h_percent.map_or(0.6, |h| (h / 100.0).clamp(0.05, 5.0));
        // Etiketternas plats runt lådan.
        let left_w = if show_val_lbl && !horizontal {
            let s = auto_scale(lo, hi, val_ax.and_then(|a| a.min), val_ax.and_then(|a| a.max), val_ax.and_then(|a| a.major), &|_, _, _| true);
            [s.min, s.max].iter().map(|x| measure(self, &val_fmt(*x), &val_style).0).fold(0.0, f32::max) + 0.92 * vs
        } else if show_cat_lbl && horizontal {
            cats.iter().map(|t| measure(self, t, &cat_style).0).fold(0.0, f32::max) + 0.92 * cs
        } else {
            0.0
        };
        let bottom_h = if (show_cat_lbl && !horizontal) || (show_val_lbl && horizontal) { 1.81 * cs.max(vs) } else { 0.0 };
        let box_area = inner_fixed.unwrap_or(R { x: outer.x + left_w, y: outer.y + 0.45 * vs, w: (outer.w - left_w - 0.33 * cs).max(1.0), h: (outer.h - bottom_h - 0.45 * vs).max(1.0) });

        // Världen: x 0..1, y 0..H, z 0..D, centrerad för kameran. Vid rätvinkliga axlar utan `hPercent`
        // väljs höjden så att lådan fyller ytan.
        let (wx, dz) = (1.0f32, depth);
        let cam = Camera::new(rot_x, rot_y, right_angle, v.perspective.unwrap_or(30.0), 1.0f32.max(depth).max(height));
        let hy = if right_angle && v.h_percent.is_none() {
            let (dx, dyu) = cam.depth_dir();
            let s = box_area.w / (wx + dz * dx.abs());
            (box_area.h / s - dz * dyu.abs()).max(0.05)
        } else {
            height
        };
        let center = [wx / 2.0, hy / 2.0, dz / 2.0];
        let rel = |p: V3| [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
        let corners: Vec<V3> = [0.0, wx].iter().flat_map(|&x| [0.0, hy].into_iter().flat_map(move |y| [0.0, dz].into_iter().map(move |z| [x, y, z]))).collect();
        let proj: Vec<(f32, f32)> = corners.iter().map(|p| {
            let (x, y, _) = cam.project(rel(*p));
            (x, y)
        }).collect();
        let (mut bx0, mut bx1, mut by0, mut by1) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
        for &(x, y) in &proj {
            bx0 = bx0.min(x);
            bx1 = bx1.max(x);
            by0 = by0.min(y);
            by1 = by1.max(y);
        }
        let s = (box_area.w / (bx1 - bx0).max(1e-3)).min(box_area.h / (by1 - by0).max(1e-3));
        let ox = box_area.x + (box_area.w - (bx1 - bx0) * s) / 2.0 - bx0 * s;
        let oy = box_area.y + (box_area.h - (by1 - by0) * s) / 2.0 + by1 * s;
        let to_screen = move |x: f32, y: f32| (ox + x * s, oy - y * s);
        let at = |p: V3| -> (f32, f32) {
            let (x, y, _) = cam.project(rel(p));
            to_screen(x, y)
        };

        // Skalan efter lådans höjd på skärmen.
        let axis_len = if horizontal { (at([wx, 0.0, 0.0]).0 - at([0.0, 0.0, 0.0]).0).abs() } else { (at([0.0, 0.0, 0.0]).1 - at([0.0, hy, 0.0]).1).abs() };
        let (fmin, fmax, fstep) = val_ax.map_or((None, None, None), |a| (a.min, a.max, a.major));
        let (fmin, fmax) = if percent { (fmin.or(Some(0.0)), fmax.or(Some(1.0))) } else { (fmin, fmax) };
        let scale: Scale = if horizontal {
            let fits = |mn: f64, mx: f64, step: f64| -> bool {
                let n = ((mx - mn) / step).round().max(1.0);
                let wmax = [mn, mx].iter().map(|v| measure(self, &val_fmt(*v), &val_style).0).fold(0.0, f32::max);
                (axis_len as f64) / n >= (wmax + 2.0 * vs) as f64
            };
            auto_scale(lo, hi, fmin, fmax, fstep, &fits)
        } else {
            auto_scale(lo, hi, fmin, fmax, fstep, &|mn, mx, step| (axis_len as f64) / ((mx - mn) / step) >= 1.5 * vs as f64 * 0.98 || (mx - mn) / step <= 2.0)
        };
        let val_rev = val_ax.is_some_and(|a| a.reversed);
        // Värde → världens koordinat längs värdeaxeln (y för stående, x för liggande staplar).
        let vext = if horizontal { wx } else { hy };
        let vw = move |val: f64| -> f32 {
            let t = (((val - scale.min) / (scale.max - scale.min)) as f32).clamp(0.0, 1.0);
            (if val_rev { 1.0 - t } else { t }) * vext
        };
        let cat_rev = cat_ax.is_some_and(|a| a.reversed);
        let cext = if horizontal { hy } else { wx };
        let cband = cext / ncat as f32;
        // Kategori i (bandets början) längs kategoriaxeln.
        let cw = move |i: f32| -> f32 { if cat_rev { cext - (i + 1.0) * cband } else { i * cband } };
        let to_w = move |cat: f32, val: f32, z: f32| -> V3 { if horizontal { [val, cat, z] } else { [cat, val, z] } };

        // Väggar och golv.
        let walls: [(Option<&super::Node>, Vec<V3>, FaceKind); 3] = [
            (c.walls.back, vec![[0.0, 0.0, dz], [wx, 0.0, dz], [wx, hy, dz], [0.0, hy, dz]], FaceKind::Back),
            (c.walls.side, vec![[0.0, 0.0, 0.0], [0.0, 0.0, dz], [0.0, hy, dz], [0.0, hy, 0.0]], FaceKind::Wall),
            (c.walls.floor, vec![[0.0, 0.0, 0.0], [wx, 0.0, 0.0], [wx, 0.0, dz], [0.0, 0.0, dz]], FaceKind::Floor),
        ];
        for (sppr, pts, _) in &walls {
            let scr: Vec<(f32, f32)> = pts.iter().map(|p| at(*p)).collect();
            let r = bounds(&scr);
            self.shape(poly_path(&scr), r, *sppr, None, None);
        }

        // Rutnät på väggarna och golvets kanter.
        let grid_def = Stroke { paint: Paint::Solid(lum(self.scheme("tx1"), 0.15, 0.85)), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Round };
        if let Some(Some(st)) = val_ax.and_then(|a| a.major_grid).map(|sp| self.line_of(sp, box_area).unwrap_or(Some(grid_def.clone()))) {
            let mut path = Path::default();
            for t in scale.ticks() {
                let q = vw(t);
                // Längs bakväggen och sidoväggen (stående) eller golvet och bakväggen (liggande).
                let (a, b, e) = if horizontal { ([q, 0.0, 0.0], [q, 0.0, dz], [q, hy, dz]) } else { ([0.0, q, 0.0], [0.0, q, dz], [wx, q, dz]) };
                let (a, b, e) = (at(a), at(b), at(e));
                path.move_to(a.0, a.1);
                path.line_to(b.0, b.1);
                path.line_to(e.0, e.1);
            }
            self.stroke(path, st);
        }
        let axis_def = Stroke { paint: Paint::Solid(lum(self.scheme("tx1"), 0.25, 0.75)), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Round };

        // Serierna som föremål, ritade bakifrån och fram.
        let total_series: usize = c.plots.iter().map(|p| p.series.len()).sum();
        let mut objects: Vec<(f32, Vec<Face>, Rgba, Option<&super::Node>, R)> = Vec::new();
        let mut row0 = 0usize;
        let slot = dz / rows as f32;
        for p in &c.plots {
            let std_rows = standard(p);
            let m = p.series.len().max(1) as f32;
            let gap = p.gap / 100.0;
            let gap_d = p.gap_depth / 100.0;
            let clustered = p.grouping == Grouping::Clustered && !std_rows;
            let shape = BarShape::parse(&p.shape);
            let mut pos_acc = vec![0.0f64; ncat];
            let mut neg_acc = vec![0.0f64; ncat];
            let sums: Vec<f64> = (0..ncat).map(|i| p.series.iter().map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0).abs()).sum()).collect();
            for (k, ser) in p.series.iter().enumerate() {
                let row = if std_rows { row0 + k } else { row0 };
                // Seriens djup: raden minus mellanrummet på djupet.
                // Första serien närmast betraktaren.
                let zc = (row as f32 + 0.5) * slot;
                let zw = slot / (1.0 + gap_d);
                let (z0, z1) = if matches!(p.kind, Kind::Bar { .. }) { (zc - zw / 2.0, zc + zw / 2.0) } else { (zc - slot * 0.35, zc + slot * 0.35) };
                let col = self.series_paint(c, p, ser, None, total_series, ncat);
                match p.kind {
                    Kind::Bar { .. } => {
                        let w = if clustered { cband / (m + gap) } else { cband / (1.0 + gap) };
                        for i in 0..ncat {
                            let Some(val) = ser.vals.get(i).copied().flatten() else { continue };
                            let val = if p.grouping == Grouping::Percent { if sums[i] > 0.0 { val / sums[i] } else { 0.0 } } else { val };
                            let (a, b) = if p.grouping == Grouping::Clustered || std_rows {
                                (0.0, val)
                            } else if val >= 0.0 {
                                let a = pos_acc[i];
                                pos_acc[i] += val;
                                (a, a + val)
                            } else {
                                let a = neg_acc[i];
                                neg_acc[i] += val;
                                (a, a + val)
                            };
                            let base = scale.min.max(0.0).min(scale.max);
                            let (a, b) = if p.grouping == Grouping::Clustered || std_rows { (base, b) } else { (a, b) };
                            let start = cw(i as f32) + (cband - if clustered { w * m } else { w }) / 2.0;
                            let off = if clustered { k as f32 * w } else { 0.0 };
                            let (va, vb) = (vw(a), vw(b));
                            if (vb - va).abs() < 1e-6 && !matches!(shape, BarShape::Box) {
                                continue;
                            }
                            let lo3 = to_w(start + off, va.min(vb), z0);
                            let hi3 = to_w(start + off + w, va.max(vb), z1);
                            let tip = Some(vw(scale.max));
                            let polys: Vec<Poly> = d3::bar_polys(shape, lo3, hi3, !horizontal, vb < va, tip);
                            let polys: Vec<Poly> = polys.into_iter().map(|mut q| {
                                for pt in &mut q.pts {
                                    *pt = rel(*pt);
                                }
                                q
                            }).collect();
                            let depth = d3::object_depth(&cam, &polys);
                            let faces = d3::project_polys(&cam, &polys, i, &to_screen);
                            let sp = ser.point(i).and_then(|d| d.sppr).or(ser.sppr);
                            let pc = self.series_paint(c, p, ser, Some(i), total_series, ncat);
                            let scr: Vec<(f32, f32)> = faces.iter().flat_map(|f| f.pts.iter().copied()).collect();
                            objects.push((depth, faces, pc, sp, bounds(&scr)));
                        }
                    }
                    Kind::Line | Kind::Area => {
                        let area = p.kind == Kind::Area;
                        let mid = |i: usize| cw(i as f32) + cband / 2.0;
                        let base_v = vw(scale.min.max(0.0).min(scale.max));
                        // Staplade ytor: värdena läggs ovanpå varandra.
                        let vals: Vec<Option<f32>> = (0..ncat)
                            .map(|i| {
                                // Tomma punkter: luckor i linjen, noll i ytan (som Excel).
                                let v = match ser.vals.get(i).copied().flatten() {
                                    Some(v) => v,
                                    None if area && i < ser.vals.len().max(ser.cats.len()) => 0.0,
                                    None => return None,
                                };
                                let v = if p.grouping == Grouping::Percent { if sums[i] > 0.0 { v / sums[i] } else { 0.0 } } else { v };
                                if p.grouping != Grouping::Clustered && !std_rows {
                                    pos_acc[i] += v;
                                    Some(vw(pos_acc[i]))
                                } else {
                                    Some(vw(v))
                                }
                            })
                            .collect();
                        let mut polys: Vec<Poly> = Vec::new();
                        for i in 0..ncat.saturating_sub(1) {
                            let (Some(y0), Some(y1)) = (vals[i], vals[i + 1]) else { continue };
                            let (x0, x1) = (mid(i), mid(i + 1));
                            // Bandets ovansida.
                            polys.push(Poly { kind: FaceKind::Top, pts: vec![to_w(x0, y0, z0), to_w(x1, y1, z0), to_w(x1, y1, z1), to_w(x0, y0, z1)] });
                            if area {
                                polys.push(Poly { kind: FaceKind::Front, pts: vec![to_w(x0, base_v, z0), to_w(x1, base_v, z0), to_w(x1, y1, z0), to_w(x0, y0, z0)] });
                            } else {
                                // Linjen: bandets framkant som en smal list.
                                let t = (hy * 0.012).max(0.002);
                                polys.push(Poly { kind: FaceKind::Front, pts: vec![to_w(x0, y0 - t, z0), to_w(x1, y1 - t, z0), to_w(x1, y1, z0), to_w(x0, y0, z0)] });
                            }
                        }
                        if area {
                            // Ändarna.
                            if let (Some(i0), Some(i1)) = (vals.iter().position(|v| v.is_some()), vals.iter().rposition(|v| v.is_some())) {
                                for i in [i0, i1] {
                                    let y = vals[i].unwrap_or(base_v);
                                    polys.push(Poly { kind: FaceKind::Side, pts: vec![to_w(mid(i), base_v, z0), to_w(mid(i), y, z0), to_w(mid(i), y, z1), to_w(mid(i), base_v, z1)] });
                                }
                            }
                        }
                        // Normalerna utåt: ovansidan uppåt, framsidan framåt.
                        let polys: Vec<Poly> = polys.into_iter().map(|mut q| {
                            for pt in &mut q.pts {
                                *pt = rel(*pt);
                            }
                            q
                        }).collect();
                        let mut faces: Vec<Face> = Vec::new();
                        for q in &polys {
                            // Ett band är tunt: båda sidor kan synas, så ytan vänds mot betraktaren.
                            let mut q2 = q.clone();
                            let mut f = d3::project_polys(&cam, std::slice::from_ref(&q2), k, &to_screen);
                            if f.is_empty() {
                                q2.pts.reverse();
                                f = d3::project_polys(&cam, std::slice::from_ref(&q2), k, &to_screen);
                            }
                            faces.extend(f);
                        }
                        d3::painter_sort(&mut faces);
                        let depth = zc_depth(&cam, rel([0.5, 0.0, zc]));
                        let scr: Vec<(f32, f32)> = faces.iter().flat_map(|f| f.pts.iter().copied()).collect();
                        objects.push((depth, faces, col, ser.sppr, bounds(&scr)));
                    }
                    _ => {}
                }
            }
            row0 += if std_rows { p.series.len() } else { 1 };
        }
        objects.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, faces, col, sppr, r) in &objects {
            let base = self.fill_of(*sppr, *r).flatten().map(|p| p.average()).unwrap_or(*col);
            let line = self.line_of(*sppr, *r).flatten();
            // Rundade former (cylinder, kon) har många smala sidor: en tunn kant i samma färg döljer skarvarna.
            let many = faces.iter().filter(|f| f.kind == FaceKind::Side).count() > 4;
            for f in faces {
                self.face(f, base);
                if many && f.kind == FaceKind::Side && line.is_none() {
                    self.stroke(poly_path(&f.pts), Stroke { paint: Paint::Solid(shaded(base, f.shade)), width: 0.5, dash: Vec::new(), cap: Cap::Butt, join: Join::Round });
                }
                if let Some(st) = &line {
                    self.stroke(poly_path(&f.pts), st.clone());
                }
            }
        }

        // Axellinjer längs lådans främre kanter och etiketterna.
        let o = at([0.0, 0.0, 0.0]);
        if let Some(Some(st)) = val_ax.filter(|a| !a.deleted).map(|a| self.line_of(a.sppr, box_area).unwrap_or(Some(axis_def.clone()))) {
            let e = if horizontal { at([wx, 0.0, 0.0]) } else { at([0.0, hy, 0.0]) };
            let mut path = Path::default();
            path.move_to(o.0, o.1);
            path.line_to(e.0, e.1);
            self.stroke(path, st);
        }
        if let Some(Some(st)) = cat_ax.filter(|a| !a.deleted).map(|a| self.line_of(a.sppr, box_area).unwrap_or(Some(axis_def.clone()))) {
            let e = if horizontal { at([0.0, hy, 0.0]) } else { at([wx, 0.0, 0.0]) };
            let mut path = Path::default();
            path.move_to(o.0, o.1);
            path.line_to(e.0, e.1);
            self.stroke(path, st);
        }
        if show_val_lbl {
            for t in scale.ticks() {
                let q = vw(t);
                let l = self.lay(Self::plain_paras(&val_fmt(t), &val_style, if horizontal { Align::Center } else { Align::Right }), f32::INFINITY);
                if horizontal {
                    let p = at([q, 0.0, 0.0]);
                    self.place(l, p.0, p.1 + 0.61 * vs, 0.5, 0.0, 0.0);
                } else {
                    let p = at([0.0, q, 0.0]);
                    self.place(l, p.0 - 0.92 * vs, p.1 - 0.1 * vs, 1.0, 0.5, 0.0);
                }
            }
        }
        if show_cat_lbl {
            for (i, t) in cats.iter().enumerate() {
                let q = cw(i as f32) + cband / 2.0;
                if horizontal {
                    let p = at([0.0, q, 0.0]);
                    let l = self.lay(Self::plain_paras(t, &cat_style, Align::Right), f32::INFINITY);
                    self.place(l, p.0 - 0.92 * cs, p.1 - 0.1 * cs, 1.0, 0.5, 0.0);
                } else {
                    let p = at([q, 0.0, 0.0]);
                    let bw = (at([cw(i as f32) + cband, 0.0, 0.0]).0 - at([cw(i as f32), 0.0, 0.0]).0).abs();
                    let l = self.lay(Self::plain_paras(t, &cat_style, Align::Center), bw.max(cs * 3.0));
                    self.place(l, p.0, p.1 + 0.61 * cs, 0.5, 0.0, 0.0);
                }
            }
        }
        // Seriernas namn längs djupaxeln (`serAx`), till höger om lådan.
        if let Some(ax) = ser_ax.filter(|a| !a.deleted && a.tick_lbl_pos != "none") {
            let st = self.style(&[ax.txpr, c.space_txpr], 1000);
            let mut row = 0usize;
            for p in &c.plots {
                if !standard(p) {
                    row += 1;
                    continue;
                }
                for ser in &p.series {
                    let zc = (row as f32 + 0.5) * slot;
                    let q = if horizontal { at([wx, 0.0, zc]) } else { at([wx, 0.0, zc]) };
                    let l = self.lay(Self::plain_paras(&ser.name, &st, Align::Left), f32::INFINITY);
                    self.place(l, q.0 + 0.4 * st.size, q.1, 0.0, 0.5, 0.0);
                    row += 1;
                }
            }
        }
    }
}

/// Djupet för en punkt (för att ordna linjer/ytor).
fn zc_depth(cam: &Camera, p: V3) -> f32 {
    cam.view(p)[2]
}
