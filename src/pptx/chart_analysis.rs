//! Trendlinjer (`c:trendline`) och felstaplar (`c:errBars`) i diagram. Undermodul till `chart_draw.rs`
//! (delar `Draw`); matematiken (minsta kvadrat, R², felstaplarnas belopp) är rena funktioner med prov.
//!
//! - **Trendlinjer** som Excel: x är kategorins nummer (1, 2, 3 …) i stapel-, linje- och ytdiagram och
//!   x-värdet i punktdiagram. Linjär och polynom (grad 2–6) med minsta kvadrat, exponentiell (ln y linjärt
//!   i x), logaritmisk (y linjärt i ln x), potens (ln y linjärt i ln x), glidande medel. R² räknas som Excel
//!   i den linjäriserade formen (ln y för exponentiell och potens). `forward`/`backward` förlänger linjen i
//!   x-axelns enheter, `intercept` låser skärningen med y-axeln. Linjen klipps till ritytan. Ekvationen och
//!   R² (`dispEq`/`dispRSqr`) skrivs vid linjens slut, med exponenter upphöjda.
//! - **Felstaplar:** fast värde, procent, standardavvikelse (som Excel: kring seriens medelvärde),
//!   standardfel och egna värden (`c:plus`/`c:minus`), åt båda hållen eller bara plus/minus, med eller utan
//!   ändstreck; i värdeled för stapel/linje/yta och i båda leden för punktdiagram.

use super::super::chart_fmt;
use super::super::chart_model::{ErrBars, Series, TrendKind, Trendline};
use super::super::scene::{Cap, Item, Join, Paint, Path, Stroke};
use super::super::text::{Align, Para, Run};
use super::{lum, Draw, R};

/// En anpassad trendlinje.
#[derive(Clone, Debug, PartialEq)]
pub enum Model {
    /// y = c0 + c1·x + c2·x² …
    Poly(Vec<f64>),
    /// y = c·e^(b·x)
    Exp { c: f64, b: f64 },
    /// y = a + b·ln x
    Log { a: f64, b: f64 },
    /// y = c·x^b
    Power { c: f64, b: f64 },
}

impl Model {
    pub fn eval(&self, x: f64) -> Option<f64> {
        let y = match self {
            Model::Poly(c) => c.iter().rev().fold(0.0, |acc, k| acc * x + k),
            Model::Exp { c, b } => c * (b * x).exp(),
            Model::Log { a, b } => {
                if x <= 0.0 {
                    return None;
                }
                a + b * x.ln()
            }
            Model::Power { c, b } => {
                if x <= 0.0 {
                    return None;
                }
                c * x.powf(*b)
            }
        };
        y.is_finite().then_some(y)
    }

    /// Ekvationen som delar (text, upphöjd), t.ex. `y = 0,5x + 1,2`.
    pub fn equation(&self) -> Vec<(String, bool)> {
        let num = |v: f64| fmt_coef(v);
        let mut out: Vec<(String, bool)> = vec![("y = ".into(), false)];
        match self {
            Model::Poly(c) => {
                let mut first = true;
                for (k, v) in c.iter().enumerate().rev() {
                    if *v == 0.0 && !(k == 0 && first) {
                        continue;
                    }
                    let mag = num(v.abs());
                    let sign = if first { if *v < 0.0 { "-" } else { "" } } else if *v < 0.0 { " - " } else { " + " };
                    let body = match k {
                        0 => mag,
                        _ if mag == "1" => String::new(),
                        _ => mag,
                    };
                    out.push((format!("{sign}{body}{}", if k >= 1 { "x" } else { "" }), false));
                    if k >= 2 {
                        out.push((k.to_string(), true));
                    }
                    first = false;
                }
            }
            Model::Exp { c, b } => {
                out.push((format!("{}e", num(*c)), false));
                out.push((format!("{}x", num(*b)), true));
            }
            Model::Log { a, b } => {
                out.push((format!("{}ln(x)", num(*b)), false));
                if *a != 0.0 {
                    out.push((format!(" {} {}", if *a < 0.0 { "-" } else { "+" }, num(a.abs())), false));
                }
            }
            Model::Power { c, b } => {
                out.push((format!("{}x", num(*c)), false));
                out.push((num(*b), true));
            }
        }
        out
    }
}

/// En koefficient som i Excels ekvationer (fyra decimaler, utan överflödiga nollor).
fn fmt_coef(v: f64) -> String {
    let r = if v.abs() >= 1e-4 { (v * 1e4).round() / 1e4 } else { v };
    chart_fmt::general(r)
}

/// Löser A·x = b (Gauss med pivotering). `None` om systemet är singulärt.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap_or(std::cmp::Ordering::Equal))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in 0..n {
            if r != col {
                let f = a[r][col] / a[col][col];
                for k in col..n {
                    a[r][k] -= f * a[col][k];
                }
                b[r] -= f * b[col];
            }
        }
    }
    Some((0..n).map(|i| b[i] / a[i][i]).collect())
}

/// Polynom av graden `deg` med minsta kvadrat; `intercept` låser c0.
fn poly_fit(xs: &[f64], ys: &[f64], deg: usize, intercept: Option<f64>) -> Option<Vec<f64>> {
    let start = usize::from(intercept.is_some());
    let k = deg + 1 - start;
    if xs.len() < k || k == 0 {
        return None;
    }
    let y0 = intercept.unwrap_or(0.0);
    let mut a = vec![vec![0.0; k]; k];
    let mut b = vec![0.0; k];
    for (&x, &y) in xs.iter().zip(ys) {
        let pw: Vec<f64> = (start..=deg).map(|p| x.powi(p as i32)).collect();
        for i in 0..k {
            for j in 0..k {
                a[i][j] += pw[i] * pw[j];
            }
            b[i] += pw[i] * (y - y0);
        }
    }
    let sol = solve(a, b)?;
    let mut c = vec![y0; start];
    c.extend(sol);
    Some(c)
}

fn r_squared(ys: &[f64], fit: &[f64]) -> f64 {
    let n = ys.len().max(1) as f64;
    let mean = ys.iter().sum::<f64>() / n;
    let tot: f64 = ys.iter().map(|y| (y - mean).powi(2)).sum();
    let res: f64 = ys.iter().zip(fit).map(|(y, f)| (y - f).powi(2)).sum();
    if tot <= 0.0 { 1.0 } else { 1.0 - res / tot }
}

/// Anpassar en trendlinje till punkterna; ger modellen och R². Glidande medel ger `None` (se `moving_average`).
pub fn fit(kind: TrendKind, pts: &[(f64, f64)], intercept: Option<f64>) -> Option<(Model, f64)> {
    let (xs, ys): (Vec<f64>, Vec<f64>) = pts.iter().copied().unzip();
    match kind {
        TrendKind::Linear | TrendKind::Poly(_) => {
            let deg = if let TrendKind::Poly(d) = kind { d } else { 1 };
            let c = poly_fit(&xs, &ys, deg, intercept)?;
            let m = Model::Poly(c);
            let f: Vec<f64> = xs.iter().map(|x| m.eval(*x).unwrap_or(0.0)).collect();
            let r2 = r_squared(&ys, &f);
            Some((m, r2))
        }
        TrendKind::Exp => {
            let (lx, ly): (Vec<f64>, Vec<f64>) = pts.iter().filter(|p| p.1 > 0.0).map(|p| (p.0, p.1.ln())).unzip();
            let c = poly_fit(&lx, &ly, 1, intercept.filter(|v| *v > 0.0).map(f64::ln))?;
            let f: Vec<f64> = lx.iter().map(|x| c[0] + c[1] * x).collect();
            Some((Model::Exp { c: c[0].exp(), b: c[1] }, r_squared(&ly, &f)))
        }
        TrendKind::Log => {
            let (lx, y): (Vec<f64>, Vec<f64>) = pts.iter().filter(|p| p.0 > 0.0).map(|p| (p.0.ln(), p.1)).unzip();
            let c = poly_fit(&lx, &y, 1, None)?;
            let f: Vec<f64> = lx.iter().map(|x| c[0] + c[1] * x).collect();
            Some((Model::Log { a: c[0], b: c[1] }, r_squared(&y, &f)))
        }
        TrendKind::Power => {
            let (lx, ly): (Vec<f64>, Vec<f64>) = pts.iter().filter(|p| p.0 > 0.0 && p.1 > 0.0).map(|p| (p.0.ln(), p.1.ln())).unzip();
            let c = poly_fit(&lx, &ly, 1, None)?;
            let f: Vec<f64> = lx.iter().map(|x| c[0] + c[1] * x).collect();
            Some((Model::Power { c: c[0].exp(), b: c[1] }, r_squared(&ly, &f)))
        }
        TrendKind::MovingAvg(_) => None,
    }
}

/// Glidande medel över `period` punkter (i ordning): punkterna från den `period`:e.
pub fn moving_average(pts: &[(f64, f64)], period: usize) -> Vec<(f64, f64)> {
    let p = period.max(1);
    if pts.len() < p {
        return Vec::new();
    }
    (p - 1..pts.len()).map(|i| (pts[i].0, pts[i + 1 - p..=i].iter().map(|q| q.1).sum::<f64>() / p as f64)).collect()
}

/// Felstaplarnas belopp (minus, plus) för punkt `i` med värdet `v`; `all` är seriens alla värden.
/// För standardavvikelse ger `center` seriens medelvärde (Excel ritar dem kring medelvärdet).
pub fn error_amounts(e: &ErrBars<'_>, i: usize, v: f64, all: &[f64]) -> (f64, f64, Option<f64>) {
    let n = all.len().max(1) as f64;
    let mean = all.iter().sum::<f64>() / n;
    let sd = if all.len() > 1 { (all.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt() } else { 0.0 };
    let (mut lo, mut hi, center) = match e.val_type.as_str() {
        "percentage" => (v.abs() * e.val / 100.0, v.abs() * e.val / 100.0, None),
        "stdDev" => (sd * e.val, sd * e.val, Some(mean)),
        "stdErr" => (sd / n.sqrt(), sd / n.sqrt(), None),
        "cust" => (e.minus.get(i).copied().flatten().unwrap_or(0.0), e.plus.get(i).copied().flatten().unwrap_or(0.0), None),
        _ => (e.val, e.val, None),
    };
    match e.kind.as_str() {
        "plus" => lo = 0.0,
        "minus" => hi = 0.0,
        _ => {}
    }
    (lo.abs(), hi.abs(), center)
}

/// En series punkter för trendlinjer och felstaplar: data-x, värdet och läget längs kategoriaxeln.
pub(super) struct SeriesPoints {
    /// (data-x, värde, läge längs kategori-/x-axeln i diagrammets koordinater).
    pub pts: Vec<(f64, f64, f32)>,
    /// Data-x → läge längs kategori-/x-axeln (linjärt).
    pub x0: f32,
    pub dx: f32,
}

impl Draw<'_, '_> {
    /// Trendlinjer och felstaplar för en serie. `vpos` ger värdets läge, `horizontal` = liggande staplar
    /// (värdet längs x), `xpos` = punktdiagrammets x-värde → läge (för felstaplar i x-led).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn analysis(&mut self, ser: &Series<'_>, sp: &SeriesPoints, vpos: &dyn Fn(f64) -> f32, horizontal: bool, inner: R, color: super::Rgba, xpos: Option<&dyn Fn(f64) -> f32>) {
        let xy = |along: f32, val: f32| if horizontal { (val, along) } else { (along, val) };
        // Felstaplar.
        for e in &ser.err_bars {
            let def = Stroke { paint: Paint::Solid(lum(self.scheme("tx1"), 0.65, 0.35)), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Miter };
            let Some(st) = self.line_of(e.sppr, inner).unwrap_or(Some(def)) else { continue };
            let cap = 2.5 + st.width;
            let all: Vec<f64> = sp.pts.iter().map(|p| if e.dir_x { p.0 } else { p.1 }).collect();
            let mut path = Path::default();
            for (i, &(x, v, along)) in sp.pts.iter().enumerate() {
                if e.dir_x {
                    // x-led (punktdiagram): längs x-axeln, vid värdet.
                    let Some(xp) = xpos else { continue };
                    let (lo, hi, center) = error_amounts(e, i, x, &all);
                    let c = center.unwrap_or(x);
                    let y = vpos(v);
                    let (a, b) = (xp(c - lo), xp(c + hi));
                    path.move_to(a, y);
                    path.line_to(b, y);
                    if !e.no_end_cap {
                        for (q, amt) in [(a, lo), (b, hi)] {
                            if amt > 0.0 {
                                path.move_to(q, y - cap);
                                path.line_to(q, y + cap);
                            }
                        }
                    }
                } else {
                    let (lo, hi, center) = error_amounts(e, i, v, &all);
                    let c = center.unwrap_or(v);
                    let (a, b) = (vpos(c - lo), vpos(c + hi));
                    let (p0, p1) = (xy(along, a), xy(along, b));
                    path.move_to(p0.0, p0.1);
                    path.line_to(p1.0, p1.1);
                    if !e.no_end_cap {
                        for (q, amt) in [(a, lo), (b, hi)] {
                            if amt > 0.0 {
                                let (s, t) = (xy(along - cap, q), xy(along + cap, q));
                                path.move_to(s.0, s.1);
                                path.line_to(t.0, t.1);
                            }
                        }
                    }
                }
            }
            self.stroke(path, st);
        }
        // Trendlinjer.
        let data: Vec<(f64, f64)> = sp.pts.iter().map(|p| (p.0, p.1)).collect();
        if data.len() < 2 {
            return;
        }
        let (xmin, xmax) = data.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| (a.0.min(p.0), a.1.max(p.0)));
        for t in &ser.trendlines {
            let def = Stroke { paint: Paint::Solid(color), width: 2.25, dash: vec![0.0, 2.0 * 2.25], cap: Cap::Round, join: Join::Round };
            let st = self.line_of(t.sppr, inner).unwrap_or_else(|| {
                let w = t.sppr.and_then(|s| s.child("a:ln")).and_then(|l| l.attr_i64("w")).map(super::emu);
                Some(Stroke { width: w.unwrap_or(2.25), ..def.clone() })
            });
            let Some(st) = st else { continue };
            let to = |x: f64, y: f64| xy(sp.x0 + sp.dx * x as f32, vpos(y));
            let mut path = Path::default();
            let mut started = false;
            let mut last: Option<(f32, f32)> = None;
            let mut before_last: Option<(f32, f32)> = None;
            let mut fitted: Option<(Model, f64)> = None;
            if let TrendKind::MovingAvg(p) = t.kind {
                for (x, y) in moving_average(&data, p) {
                    let q = to(x, y);
                    if started { path.line_to(q.0, q.1) } else { path.move_to(q.0, q.1) }
                    started = true;
                    last = Some(q);
                }
            } else if let Some((m, r2)) = fit(t.kind, &data, t.intercept) {
                let (a, b) = (xmin - t.backward, xmax + t.forward);
                let n = 120;
                for k in 0..=n {
                    let x = a + (b - a) * k as f64 / n as f64;
                    let Some(y) = m.eval(x) else {
                        started = false;
                        continue;
                    };
                    let q = to(x, y);
                    if !q.0.is_finite() || !q.1.is_finite() || q.0.abs() > 1e5 || q.1.abs() > 1e5 {
                        started = false;
                        continue;
                    }
                    if started { path.line_to(q.0, q.1) } else { path.move_to(q.0, q.1) }
                    started = true;
                    before_last = last;
                    last = Some(q);
                }
                fitted = Some((m, r2));
            }
            // Klipp till ritytan.
            let clip = self.to_slide(Path::rect(inner.x, inner.y, inner.w, inner.h));
            self.items.push(Item::PushClip { path: clip });
            self.stroke(path, st);
            self.items.push(Item::PopClip);
            // Ekvation och R².
            if let (Some((m, r2)), Some(end)) = (fitted, last) {
                if t.disp_eq || t.disp_r2 {
                    self.trend_label(t, &m, r2, end, before_last.unwrap_or(end), inner);
                }
            }
        }
    }

    fn trend_label(&mut self, t: &Trendline<'_>, m: &Model, r2: f64, end: (f32, f32), prev: (f32, f32), inner: R) {
        let chain = [t.label.and_then(|l| l.child("c:txPr"))];
        let style = self.style(&chain, 1000);
        let mut runs: Vec<Run> = Vec::new();
        if t.disp_eq {
            for (txt, sup) in m.equation() {
                let mut st = style.clone();
                if sup {
                    st.baseline = 0.3;
                }
                runs.push(Run { text: txt, style: st, line_break: false });
            }
        }
        let mut paras = vec![Para { runs, align: Align::Left, end_style: style.clone(), ..Para::default() }];
        if t.disp_r2 {
            let r = Run { text: format!("R² = {}", fmt_coef(r2)), style: style.clone(), line_break: false };
            if t.disp_eq {
                paras.push(Para { runs: vec![r], align: Align::Left, end_style: style.clone(), ..Para::default() });
            } else {
                paras[0].runs.push(r);
            }
        }
        let l = self.lay(paras, f32::INFINITY);
        // Vid linjens slut, inåt (till vänster) och på den sida av linjen där den inte går: ovanför en
        // stigande linje, under en fallande. Hela etiketten inom ritytan, med lite luft.
        let pad = 0.3 * style.size;
        let rising = prev.1 >= end.1;
        let want_y = if rising { end.1 - l.h - pad } else { end.1 + pad };
        let y = want_y.clamp(inner.y + pad, (inner.bottom() - l.h - pad).max(inner.y + pad));
        // Får etiketten inte plats ovanför (under) slutet flyttas den åt vänster tills linjen har sjunkit
        // (stigit) under (över) den: linjens lutning ur de två sista punkterna.
        let mut right = end.0 - pad;
        let dx = (end.0 - prev.0).abs().max(1e-3);
        let k = (prev.1 - end.1) / dx;
        if (y - want_y).abs() > 0.5 && k.abs() > 1e-3 {
            let need = if rising { (y + l.h + pad) - end.1 } else { end.1 - (y - pad) };
            right = right.min(end.0 - need / k.abs());
        }
        let x = (right - l.w).clamp(inner.x + pad, (inner.right() - l.w - pad).max(inner.x + pad));
        self.place(l, x, y, 0.0, 0.0, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn linjar_och_polynom() {
        let pts = [(1.0, 3.0), (2.0, 5.0), (3.0, 7.0), (4.0, 9.0)];
        let (m, r2) = fit(TrendKind::Linear, &pts, None).unwrap();
        let Model::Poly(c) = m else { panic!() };
        assert!(near(c[0], 1.0) && near(c[1], 2.0) && near(r2, 1.0), "{c:?}");
        // Låst skärning: y = 0 + b·x.
        let (m, _) = fit(TrendKind::Linear, &pts, Some(0.0)).unwrap();
        let Model::Poly(c) = m else { panic!() };
        assert!(near(c[0], 0.0) && (c[1] - 2.3333333).abs() < 1e-4, "{c:?}");
        // Polynom av grad 2 genom en parabel.
        let pts: Vec<(f64, f64)> = (1..=5).map(|x| (x as f64, 2.0 * (x * x) as f64 - x as f64 + 1.0)).collect();
        let (m, r2) = fit(TrendKind::Poly(2), &pts, None).unwrap();
        let Model::Poly(c) = m.clone() else { panic!() };
        assert!(near(c[0], 1.0) && near(c[1], -1.0) && near(c[2], 2.0), "{c:?}");
        assert!(near(r2, 1.0));
        let eq: String = m.equation().into_iter().map(|(t, sup)| if sup { format!("^{t}") } else { t }).collect();
        assert_eq!(eq, "y = 2x^2 - x + 1");
    }

    #[test]
    fn exponentiell_logaritmisk_potens_och_r2() {
        let pts: Vec<(f64, f64)> = (1..=5).map(|x| (x as f64, 3.0 * (0.5 * x as f64).exp())).collect();
        let (m, r2) = fit(TrendKind::Exp, &pts, None).unwrap();
        let Model::Exp { c, b } = m else { panic!() };
        assert!((c - 3.0).abs() < 1e-9 && (b - 0.5).abs() < 1e-9 && near(r2, 1.0));
        let pts: Vec<(f64, f64)> = (1..=5).map(|x| (x as f64, 2.0 + 4.0 * (x as f64).ln())).collect();
        let (m, _) = fit(TrendKind::Log, &pts, None).unwrap();
        assert!(matches!(m, Model::Log { a, b } if (a - 2.0).abs() < 1e-9 && (b - 4.0).abs() < 1e-9));
        let pts: Vec<(f64, f64)> = (1..=5).map(|x| (x as f64, 1.5 * (x as f64).powf(0.7))).collect();
        let (m, _) = fit(TrendKind::Power, &pts, None).unwrap();
        assert!(matches!(m, Model::Power { c, b } if (c - 1.5).abs() < 1e-9 && (b - 0.7).abs() < 1e-9));
        // R² för brusiga punkter: mellan 0 och 1 (Excel: 0,8 för de här).
        let pts = [(1.0, 1.0), (2.0, 3.0), (3.0, 2.0), (4.0, 5.0), (5.0, 4.0)];
        let (_, r2) = fit(TrendKind::Linear, &pts, None).unwrap();
        assert!((r2 - 0.64).abs() < 1e-9, "{r2}");
        // Glidande medel över två.
        assert_eq!(moving_average(&pts, 2), vec![(2.0, 2.0), (3.0, 2.5), (4.0, 3.5), (5.0, 4.5)]);
        // Ekvationer.
        let s = |m: Model| m.equation().into_iter().map(|(t, sup)| if sup { format!("^{t}") } else { t }).collect::<String>();
        assert_eq!(s(Model::Exp { c: 3.0, b: 0.5 }), "y = 3e^0.5x");
        assert_eq!(s(Model::Log { a: -2.0, b: 4.0 }), "y = 4ln(x) - 2");
        assert_eq!(s(Model::Power { c: 1.5, b: 0.7 }), "y = 1.5x^0.7");
        assert_eq!(s(Model::Poly(vec![1.21428571, 0.5357142])), "y = 0.5357x + 1.2143");
    }

    #[test]
    fn felstaplarnas_belopp() {
        let mk = |kind: &str, vt: &str, val: f64| ErrBars { dir_x: false, kind: kind.into(), val_type: vt.into(), val, no_end_cap: false, plus: vec![Some(2.0)], minus: vec![Some(0.5)], sppr: None };
        let all = [2.0, 4.0, 6.0];
        assert_eq!(error_amounts(&mk("both", "fixedVal", 1.0), 0, 4.0, &all), (1.0, 1.0, None));
        assert_eq!(error_amounts(&mk("plus", "percentage", 10.0), 0, 4.0, &all), (0.0, 0.4, None));
        assert_eq!(error_amounts(&mk("minus", "cust", 0.0), 0, 4.0, &all), (0.5, 0.0, None));
        // Standardavvikelse (stickprov) = 2, kring medelvärdet 4; standardfel = 2/√3.
        assert_eq!(error_amounts(&mk("both", "stdDev", 1.0), 1, 4.0, &all), (2.0, 2.0, Some(4.0)));
        let (lo, _, c) = error_amounts(&mk("both", "stdErr", 1.0), 1, 4.0, &all);
        assert!((lo - 2.0 / 3f64.sqrt()).abs() < 1e-12 && c.is_none());
    }
}
