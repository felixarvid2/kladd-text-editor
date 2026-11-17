//! Ritar styckenas rutor (skuggning och ramar, `pbox`) bakom texten i dokumentläget.
//!
//! Geometrin räknas ur galleriets rader: en ruta spänner från första styckets första rad till
//! sista styckets sista rad, med ramens luft (`w:space` plus linjen) utanför texten. Layouten har
//! redan lagt luften i avståndet mellan styckena (`layout::apply_looks`), så rutan ritas bara
//! ut över den. Går rutan över en sidbrytning delas den (utan kant där den bryts), som i Word.

use egui::epaint::text::cursor::CCursor;
use egui::{Color32, Galley, Pos2, Rect, Shape, Vec2, pos2, vec2};

use super::layout::{adapt_background, adapt_text_color};
use super::pbox::{BoxRun, Border, LineStyle, PPr, ParaSpec};
use super::secpages::{self, PageBox};
use crate::theme::Palette;

/// En rad i galleriet: överkant, underkant och arket den ligger på.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowSpan {
    pub top: f32,
    pub bottom: f32,
    pub sheet: usize,
}

/// En sammanhängande del av en ruta på ett ark.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub y0: f32,
    pub y1: f32,
    /// Rutan fortsätter från föregående ark (ingen överkant).
    pub open_top: bool,
    /// Rutan fortsätter på nästa ark (ingen underkant).
    pub open_bottom: bool,
}

/// Delar en rutas rader efter ark. Första delen får överkantens luft ovanför första raden och sista
/// delen underkantens luft under sista raden (`ppx` = pixlar per punkt).
pub fn segments(rows: &[RowSpan], first: &PPr, last: &PPr, ppx: f32) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let mut cur_sheet = None;
    for r in rows {
        match out.last_mut() {
            Some(s) if cur_sheet == Some(r.sheet) => {
                s.y0 = s.y0.min(r.top);
                s.y1 = s.y1.max(r.bottom);
            }
            _ => {
                let open_top = !out.is_empty();
                out.push(Seg { y0: r.top, y1: r.bottom, open_top, open_bottom: false });
                cur_sheet = Some(r.sheet);
            }
        }
    }
    let n = out.len();
    for (i, s) in out.iter_mut().enumerate() {
        s.open_bottom = i + 1 < n;
    }
    if let Some(s) = out.first_mut() {
        if first.borders.top.is_some() {
            s.y0 -= first.pad_top() * ppx;
        }
    }
    if let Some(s) = out.last_mut() {
        if last.borders.bottom.is_some() {
            s.y1 += last.pad_bottom() * ppx;
        }
    }
    out
}

pub struct Env<'a> {
    pub galley: &'a Galley,
    pub gpos: Vec2,
    pub text: &'a str,
    pub boxes: &'a [PageBox],
    /// (x, y) i galleriet → spaltens vänsterkant och bredd (galleriets koordinater).
    pub col_at: &'a dyn Fn(f32, f32) -> (f32, f32),
    pub zoom: f32,
    pub palette: &'a Palette,
    pub clip: Rect,
    /// Det som syns, som byteintervall.
    pub vis: std::ops::Range<usize>,
}

/// Byte → teckenindex, framåt i en svep.
struct Sweep<'a> {
    text: &'a str,
    b: usize,
    c: usize,
}

impl Sweep<'_> {
    fn at(&mut self, b: usize) -> usize {
        let b = b.min(self.text.len());
        if b < self.b {
            self.b = 0;
            self.c = 0;
        }
        self.c += self.text[self.b..b].chars().count();
        self.b = b;
        self.c
    }
}

fn border_color(b: &Border, p: &Palette) -> Color32 {
    match b.color {
        Some(c) => adapt_text_color(c, p.page, p.dark),
        None => p.page_text,
    }
}

/// Linjen `r` (tunn rektangel) i stilen `b`. `horizontal` = linjen löper i x-led.
fn line(out: &mut Vec<Shape>, r: Rect, b: &Border, col: Color32, horizontal: bool, ppx: f32) {
    let w = if horizontal { r.height() } else { r.width() };
    match b.style {
        LineStyle::Double => {
            let t = w / 3.0;
            for k in [0.0, 2.0] {
                let sub = if horizontal {
                    Rect::from_min_size(pos2(r.left(), r.top() + t * k), vec2(r.width(), t))
                } else {
                    Rect::from_min_size(pos2(r.left() + t * k, r.top()), vec2(t, r.height()))
                };
                out.push(Shape::rect_filled(sub, 0.0, col));
            }
        }
        LineStyle::Dashed | LineStyle::Dotted => {
            let w1 = (f32::from(b.sz) / 8.0 * ppx).max(1.0);
            let (dash, gap) = if b.style == LineStyle::Dashed { (w1 * 4.0, w1 * 3.0) } else { (w1, w1) };
            let len = if horizontal { r.width() } else { r.height() };
            let mut at = 0.0;
            while at < len {
                let l = dash.min(len - at);
                let sub = if horizontal {
                    Rect::from_min_size(pos2(r.left() + at, r.top()), vec2(l, r.height()))
                } else {
                    Rect::from_min_size(pos2(r.left(), r.top() + at), vec2(r.width(), l))
                };
                out.push(Shape::rect_filled(sub, 0.0, col));
                at += dash + gap;
            }
        }
        _ => out.push(Shape::rect_filled(r, 0.0, col)),
    }
}

fn thick(b: &Border, ppx: f32) -> f32 {
    (b.width_pt() * ppx).max(1.0)
}

/// Rutornas former, i skärmkoordinater (galleriets plus `gpos`).
pub fn shapes(env: &Env, specs: &[ParaSpec], runs: &[BoxRun]) -> Vec<Shape> {
    let mut out = Vec::new();
    let ppx = super::layout::px(env.zoom, 1.0);
    let g = env.galley;
    let mut sweep = Sweep { text: env.text, b: 0, c: 0 };
    let first_run = runs.partition_point(|r| r.range.end <= env.vis.start);
    for run in &runs[first_run..] {
        if run.range.start >= env.vis.end {
            break;
        }
        let ps = &specs[run.specs.clone()];
        let (first, last) = (&ps[0].pp, &ps[ps.len() - 1].pp);
        let ca = sweep.at(ps[0].range.start);
        let cb = sweep.at(ps[ps.len() - 1].range.end);
        let (ra, rb) = (g.pos_from_cursor(CCursor::new(ca)), g.pos_from_cursor(CCursor::new(cb)));
        let (y0, y1) = (ra.top(), rb.bottom());
        if y1 + 200.0 < env.clip.top() - env.gpos.y || y0 - 200.0 > env.clip.bottom() - env.gpos.y {
            continue;
        }
        // Spalten rutan börjar i. I ett avsnitt med flera spalter är radernas y inte stigande genom
        // galleriet (varje spalt börjar om), så raderna väljs efter läge och spalt, inte med bisektion.
        let (cl, cw) = (env.col_at)(ra.left(), y0);
        let rows: Vec<RowSpan> = g
            .rows
            .iter()
            .filter(|r| r.rect().max.y > y0 + 0.5 && r.pos.y < y1 - 0.01 && r.pos.x >= cl - 1.0 && r.pos.x < cl + cw + 1.0)
            .map(|r| RowSpan {
                top: r.pos.y,
                bottom: r.rect().max.y,
                sheet: if env.boxes.is_empty() { 0 } else { secpages::page_index(env.boxes, r.pos.y) },
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        let segs = segments(&rows, first, last, ppx);
        let fill = first.shd.map(|c| adapt_background(c, env.palette.dark));
        let b = &first.borders;
        for s in &segs {
            let (left, right) = match first.frame_geom(cw / ppx) {
                // Ram med egen bredd: rutans egna kanter.
                Some((_, _, l, w)) => (cl + l * ppx, cl + (l + w) * ppx),
                None => {
                    let il = first.left.unwrap_or(0.0).max(0.0) * ppx;
                    let ir = first.right.unwrap_or(0.0).max(0.0) * ppx;
                    (cl + il - first.pad_left() * ppx, cl + cw - ir + first.pad_right() * ppx)
                }
            };
            let r = Rect::from_min_max(pos2(left, s.y0), pos2(right, s.y1)).translate(env.gpos);
            if r.bottom() < env.clip.top() || r.top() > env.clip.bottom() {
                continue;
            }
            if let Some(c) = fill {
                out.push(Shape::rect_filled(r, 0.0, c));
            }
            if let (Some(bd), false) = (&b.top, s.open_top) {
                let t = thick(bd, ppx);
                line(&mut out, Rect::from_min_size(r.min, vec2(r.width(), t)), bd, border_color(bd, env.palette), true, ppx);
            }
            if let (Some(bd), false) = (&b.bottom, s.open_bottom) {
                let t = thick(bd, ppx);
                line(&mut out, Rect::from_min_size(pos2(r.left(), r.bottom() - t), vec2(r.width(), t)), bd, border_color(bd, env.palette), true, ppx);
            }
            if let Some(bd) = &b.left {
                let t = thick(bd, ppx);
                line(&mut out, Rect::from_min_size(r.min, vec2(t, r.height())), bd, border_color(bd, env.palette), false, ppx);
            }
            if let Some(bd) = &b.right {
                let t = thick(bd, ppx);
                line(&mut out, Rect::from_min_size(pos2(r.right() - t, r.top()), vec2(t, r.height())), bd, border_color(bd, env.palette), false, ppx);
            }
            // Mellanlinjer mellan stycken i rutan, mitt i avståndet.
            if let Some(bd) = &b.between {
                for w in ps.windows(2) {
                    let ce = sweep.at(w[0].range.end);
                    let cs = sweep.at(w[1].range.start);
                    let (ea, sb) = (g.pos_from_cursor(CCursor::new(ce)), g.pos_from_cursor(CCursor::new(cs)));
                    let y = (ea.bottom() + sb.top()) / 2.0;
                    if y >= s.y0 && y <= s.y1 {
                        let t = thick(bd, ppx);
                        let at: Pos2 = pos2(r.left(), y + env.gpos.y - t / 2.0);
                        line(&mut out, Rect::from_min_size(at, vec2(r.width(), t)), bd, border_color(bd, env.palette), true, ppx);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::pbox::Borders;

    fn bd() -> Border {
        Border { style: LineStyle::Single, sz: 8, space: 4, color: None }
    }

    fn boxed() -> PPr {
        PPr { borders: Borders { top: Some(bd()), bottom: Some(bd()), left: Some(bd()), right: Some(bd()), between: None }, shd: Some([1, 2, 3]), ..Default::default() }
    }

    #[test]
    fn one_sheet_gets_padding_top_and_bottom() {
        let rows = [RowSpan { top: 100.0, bottom: 120.0, sheet: 0 }, RowSpan { top: 120.0, bottom: 140.0, sheet: 0 }];
        let pp = boxed();
        let s = segments(&rows, &pp, &pp, 1.0);
        let pad = bd().pad_pt();
        assert_eq!(s, vec![Seg { y0: 100.0 - pad, y1: 140.0 + pad, open_top: false, open_bottom: false }]);
    }

    #[test]
    fn page_break_splits_the_box_without_edges() {
        let rows = [
            RowSpan { top: 900.0, bottom: 920.0, sheet: 0 },
            RowSpan { top: 920.0, bottom: 940.0, sheet: 0 },
            RowSpan { top: 1200.0, bottom: 1220.0, sheet: 1 },
        ];
        let pp = boxed();
        let s = segments(&rows, &pp, &pp, 2.0);
        assert_eq!(s.len(), 2);
        let pad = bd().pad_pt() * 2.0;
        assert_eq!((s[0].y0, s[0].y1, s[0].open_top, s[0].open_bottom), (900.0 - pad, 940.0, false, true));
        assert_eq!((s[1].y0, s[1].y1, s[1].open_top, s[1].open_bottom), (1200.0, 1220.0 + pad, true, false));
    }

    #[test]
    fn shading_only_has_no_padding() {
        let rows = [RowSpan { top: 10.0, bottom: 30.0, sheet: 0 }];
        let pp = PPr { shd: Some([1, 2, 3]), ..Default::default() };
        assert_eq!(segments(&rows, &pp, &pp, 1.0), vec![Seg { y0: 10.0, y1: 30.0, open_top: false, open_bottom: false }]);
    }
}
