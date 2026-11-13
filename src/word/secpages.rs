//! Sidbrytning med avsnitt (`sections`): varje avsnitt kan ha egen sidstorlek, egna marginaler
//! och flera spalter. Galleriet läggs ut stycke för stycke med spaltens bredd (se
//! `layout::relayout`); `paginate_sections` flyttar sedan raderna: en spalt fylls uppifrån och
//! ned, därefter nästa spalt, därefter nästa sida. Raderna flyttas i själva galleriet (också i x),
//! så markör, markering och träffytor följer med av sig själva.
//!
//! Ark med olika bredd centreras på skrivbordet. Galleriets x = 0 ligger vid vänstra kanten av
//! den vänstraste textytan (`Geom::origin_x`), så att alla rader har x ≥ 0 och TextEdits yta täcker
//! allt.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::Galley;
use egui::Rect;

use super::layout::px;
use super::pages::{Geom, SHEET_GAP};
use super::sections::Section;

/// Ett avsnitts sidgeometri i px vid en given zoom.
#[derive(Clone, Debug, PartialEq)]
pub struct SecGeom {
    pub page_w: f32,
    pub page_h: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub cols: usize,
    /// Spaltens bredd.
    pub col_w: f32,
    pub gap: f32,
    /// Börjar på samma sida som föregående avsnitt (om sidorna är lika stora).
    pub continuous: bool,
}

impl SecGeom {
    pub fn new(zoom: f32, s: &Section) -> SecGeom {
        let (w, h) = s.page;
        let [t, r, b, l] = s.margin;
        // Marginalerna får aldrig äta upp hela sidan (som i `Geom::new`).
        let cap = |m: f32, dim: f32| px(zoom, m.clamp(0.0, dim * 0.4));
        let (left, right) = (cap(l, w), cap(r, w));
        let page_w = px(zoom, w);
        let text_w = (page_w - left - right).max(1.0);
        let gap = px(zoom, s.gap);
        let n = s.columns.max(1) as f32;
        // Spalterna får inte bli smalare än 20 px.
        let gap = if n > 1.0 { gap.min(((text_w - 20.0 * n) / (n - 1.0)).max(0.0)) } else { gap };
        SecGeom {
            page_w,
            page_h: px(zoom, h),
            top: cap(t, h),
            right,
            bottom: cap(b, h),
            left,
            cols: s.columns.max(1),
            col_w: ((text_w - gap * (n - 1.0)) / n).max(1.0),
            gap,
            continuous: s.continuous,
        }
    }

    /// Samma sidstorlek och marginaler (så att ett löpande avsnitt kan dela sida).
    fn same_sheet(&self, o: &SecGeom) -> bool {
        let c = |a: f32, b: f32| (a - b).abs() < 0.5;
        c(self.page_w, o.page_w)
            && c(self.page_h, o.page_h)
            && c(self.top, o.top)
            && c(self.bottom, o.bottom)
            && c(self.left, o.left)
            && c(self.right, o.right)
    }

    fn hash_into<H: Hasher>(&self, h: &mut H) {
        for v in [self.page_w, self.page_h, self.top, self.right, self.bottom, self.left, self.col_w, self.gap] {
            v.to_bits().hash(h);
        }
        self.cols.hash(h);
        self.continuous.hash(h);
    }
}

impl Geom {
    /// Sidgeometri för dokumentets avsnitt. Ett vanligt dokument (ett avsnitt som följer
    /// grundformatet, en spalt) ger samma `Geom` som `Geom::new`.
    pub fn for_sections(zoom: f32, d: &super::markup::DocDefaults, sections: &[Section]) -> Geom {
        let base = Geom::new(zoom, d);
        let plain = match sections {
            [] => true,
            [s] => {
                let (pw, ph) = d.page();
                let c = |a: f32, b: f32| (a - b).abs() < 0.01;
                s.columns == 1 && c(s.page.0, pw) && c(s.page.1, ph) && s.margin.iter().zip(d.margins()).all(|(a, b)| c(*a, b))
            }
            _ => false,
        };
        if plain {
            return base;
        }
        let secs: Vec<SecGeom> = sections.iter().map(|s| SecGeom::new(zoom, s)).collect();
        let f = &secs[0];
        Geom {
            page_w: f.page_w,
            page_h: f.page_h,
            top: f.top,
            right: f.right,
            bottom: f.bottom,
            left: f.left,
            gap: SHEET_GAP,
            secs: Arc::new(secs),
        }
    }

    /// Dokumentet har avsnitt som kräver egen sidbrytning (annars gäller `Geom`s egna mått).
    pub fn multi(&self) -> bool {
        !self.secs.is_empty()
    }

    /// Avsnitt `i` (utan avsnitt: grundgeometrin).
    pub fn sec(&self, i: usize) -> SecGeom {
        match self.secs.get(i.min(self.secs.len().saturating_sub(1))) {
            Some(s) => s.clone(),
            None => SecGeom {
                page_w: self.page_w,
                page_h: self.page_h,
                top: self.top,
                right: self.right,
                bottom: self.bottom,
                left: self.left,
                cols: 1,
                col_w: self.text_w(),
                gap: 0.0,
                continuous: false,
            },
        }
    }

    /// Bredd för utläggning av stycken i avsnitt `i`: spaltens.
    pub fn wrap_for(&self, i: usize) -> f32 {
        if self.multi() { self.sec(i).col_w } else { self.text_w() }
    }

    /// Bredaste arket.
    pub fn max_page_w(&self) -> f32 {
        self.secs.iter().map(|s| s.page_w).fold(self.page_w, f32::max)
    }

    /// Arkets vänsterkant för ett ark med bredden `w`, från skrivbordsytans vänsterkant (arken centreras).
    pub fn sheet_left(&self, w: f32) -> f32 {
        (self.max_page_w() - w) / 2.0
    }

    /// Galleriets x = 0 i samma koordinater som `sheet_left`: den vänstraste textytans kant.
    pub fn origin_x(&self) -> f32 {
        if !self.multi() {
            return self.left;
        }
        self.secs.iter().map(|s| self.sheet_left(s.page_w) + s.left).fold(f32::INFINITY, f32::min)
    }

    /// Bredden på ytan som TextEdit täcker: från `origin_x` till den högraste textkanten.
    pub fn edit_w(&self) -> f32 {
        if !self.multi() {
            return self.text_w();
        }
        let right = self.secs.iter().map(|s| self.sheet_left(s.page_w) + s.page_w - s.right).fold(0.0, f32::max);
        (right - self.origin_x()).max(1.0)
    }

    pub fn hash_secs<H: Hasher>(&self, h: &mut H) {
        self.secs.len().hash(h);
        for s in self.secs.iter() {
            s.hash_into(h);
        }
    }

    /// Arken för ett vanligt dokument: `count` lika stora ark under varandra.
    pub fn uniform_boxes(&self, count: usize) -> Vec<PageBox> {
        (0..count)
            .map(|k| PageBox {
                top: -self.top + k as f32 * self.stride(),
                left: 0.0,
                w: self.page_w,
                h: self.page_h,
                margin: [self.top, self.right, self.bottom, self.left],
                regions: vec![Region { y0: k as f32 * self.stride(), cols: 1, col_w: self.text_w(), gap: 0.0 }],
                sec: 0,
            })
            .collect()
    }
}

/// En del av ett ark med en bestämd spaltindelning: hela arket, eller (efter ett löpande
/// avsnittsbyte) delen under bytet.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// Galleriets y där delen börjar.
    pub y0: f32,
    pub cols: usize,
    pub col_w: f32,
    pub gap: f32,
}

/// Ett ark: läge och mått i px. `top` räknas från galleriets y = 0, `left` från skrivbordsytans
/// vänsterkant (se `Geom::sheet_left`).
#[derive(Clone, Debug, PartialEq)]
pub struct PageBox {
    pub top: f32,
    pub left: f32,
    pub w: f32,
    pub h: f32,
    /// Marginaler: topp, höger, botten, vänster.
    pub margin: [f32; 4],
    pub regions: Vec<Region>,
    /// Avsnittet arket börjar i.
    pub sec: usize,
}

impl PageBox {
    pub fn text_top(&self) -> f32 {
        self.top + self.margin[0]
    }

    pub fn text_bottom(&self) -> f32 {
        self.top + self.h - self.margin[2]
    }

    /// Textytans vänsterkant (samma koordinater som `left`).
    pub fn text_left(&self) -> f32 {
        self.left + self.margin[3]
    }

    /// Spaltindelningen vid galleriets y.
    pub fn region_at(&self, y: f32) -> &Region {
        let i = self.regions.partition_point(|r| r.y0 <= y + 0.5).saturating_sub(1);
        &self.regions[i]
    }

    /// Arkets rektangel; `base` = skrivbordsytans vänsterkant, `origin_y` = skärm-y för galleriets y = 0.
    pub fn rect(&self, base: f32, origin_y: f32) -> Rect {
        Rect::from_min_size(egui::pos2(base + self.left, origin_y + self.top), egui::vec2(self.w, self.h))
    }
}

/// Arket (index) som galleriets y ligger på: det sista vars textyta börjar vid eller före `y`.
pub fn page_index(boxes: &[PageBox], y: f32) -> usize {
    boxes.partition_point(|b| b.text_top() <= y + 0.5).saturating_sub(1)
}

/// Spalten (vänsterkant och bredd, i skrivbordskoordinater från `left`) som galleriets (x, y) ligger i.
/// `origin_x` = galleriets x = 0.
pub fn column_at(boxes: &[PageBox], origin_x: f32, x: f32, y: f32) -> (f32, f32) {
    let Some(b) = boxes.get(page_index(boxes, y)) else { return (origin_x, 0.0) };
    let r = b.region_at(y);
    let rel = x + origin_x - b.text_left();
    let c = ((rel / (r.col_w + r.gap).max(1.0)).floor().max(0.0) as usize).min(r.cols - 1);
    (b.text_left() + c as f32 * (r.col_w + r.gap) - origin_x, r.col_w)
}

/// Resultatet av `paginate_sections`.
pub struct Paged {
    pub boxes: Vec<PageBox>,
    /// Hur långt (x) varje rad flyttades.
    pub dx: Vec<f32>,
}

/// En placerad enhet i pågående avsnittsdel (för att kunna balansera spalterna).
struct Unit {
    rows: Range<usize>,
    top: f32,
    col: usize,
    h: f32,
}

/// Minsta spalthöjd som rymmer enheterna `hs` i `cols` spalter (girig fyllning).
fn balanced_height(hs: &[f32], cols: usize) -> f32 {
    let lo0 = hs.iter().copied().fold(0.0, f32::max);
    let total: f32 = hs.iter().sum();
    let fits = |cap: f32| {
        let (mut n, mut y) = (1usize, 0.0f32);
        for &h in hs {
            if y > 0.0 && y + h > cap + 0.01 {
                n += 1;
                y = 0.0;
            }
            y += h;
        }
        n <= cols
    };
    let (mut lo, mut hi) = (lo0, total.max(lo0));
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// Sidbryter galleriet för dokument med avsnitt. `row_sec[i]` = avsnitt för rad `i`, `col_breaks` =
/// rader som är spaltbrytningar, `floats` och `groups` som i `paginate`.
#[allow(clippy::too_many_arguments)]
pub fn paginate_sections(
    g: &mut Galley,
    geom: &Geom,
    ppp: f32,
    floats: &[(usize, f32)],
    groups: &[Range<usize>],
    row_sec: &[u32],
    col_breaks: &[usize],
) -> Paged {
    let n_rows = g.rows.len();
    let mut boxes: Vec<PageBox> = Vec::new();
    let mut dx = vec![0.0f32; n_rows];
    if n_rows == 0 {
        boxes.extend(geom.uniform_boxes(1));
        return Paged { boxes, dx };
    }
    let eps = 0.5 / ppp;
    let round = |v: f32| (v * ppp).round() / ppp;
    let origin = geom.origin_x();

    // Läge i pågående avsnitt.
    let mut cur_sec = usize::MAX;
    let mut sg = geom.sec(0);
    let mut region_top = 0.0f32;
    let mut region_page = 0usize;
    let mut col = 0usize;
    let mut col_y = 0.0f32;
    let mut page_bottom = 0.0f32; // lägsta y som använts på sidan
    let mut prev_bottom = f32::NEG_INFINITY;
    let mut units: Vec<Unit> = Vec::new();
    let mut broke_page = false; // avsnittsdelen har gått över en sidgräns

    fn new_sheet(boxes: &mut Vec<PageBox>, geom: &Geom, sec: usize, sg: &SecGeom) {
        let top = match boxes.last() {
            Some(b) => b.top + b.h + SHEET_GAP,
            None => -sg.top,
        };
        boxes.push(PageBox {
            top,
            left: geom.sheet_left(sg.page_w),
            w: sg.page_w,
            h: sg.page_h,
            margin: [sg.top, sg.right, sg.bottom, sg.left],
            regions: vec![Region { y0: top + sg.top, cols: sg.cols, col_w: sg.col_w, gap: sg.gap }],
            sec,
        });
    }
    // Ark som en tidigare hög rad sträcker sig över hoppas över (de blir tomma ark).
    fn skip_covered(boxes: &mut Vec<PageBox>, geom: &Geom, sec: usize, sg: &SecGeom, until: f32) {
        let mut guard = 0;
        while boxes.last().is_some_and(|b| b.text_top() < until) && guard < 10_000 {
            new_sheet(boxes, geom, sec, sg);
            guard += 1;
        }
    }
    let col_x = |boxes: &Vec<PageBox>, sg: &SecGeom, col: usize| -> f32 {
        let b = boxes.last().expect("minst ett ark");
        b.text_left() + col as f32 * (sg.col_w + sg.gap) - origin
    };

    let mut next_group = 0;
    let mut i = 0;
    while i < n_rows {
        while next_group < groups.len() && groups[next_group].start < i {
            next_group += 1;
        }
        let end = match groups.get(next_group) {
            Some(r) if r.start == i && r.end > i => r.end.min(n_rows),
            _ => i + 1,
        };
        let (mut y0, mut y1) = (f32::INFINITY, f32::NEG_INFINITY);
        for pr in &g.rows[i..end] {
            y0 = y0.min(pr.pos.y);
            y1 = y1.max(pr.pos.y + pr.row.size.y);
        }
        let h = y1 - y0;
        let fh = floats.iter().find(|f| f.0 == i).map_or(0.0, |f| f.1);
        let sec = row_sec.get(i).map_or(0, |s| *s as usize);

        // Nytt avsnitt: ny sida, eller (löpande) vidare på samma sida.
        if sec != cur_sec {
            let ng = geom.sec(sec);
            let same = boxes.last().is_some_and(|b| {
                let cmp = SecGeom { cols: 1, col_w: 0.0, gap: 0.0, continuous: false, page_w: b.w, page_h: b.h, top: b.margin[0], right: b.margin[1], bottom: b.margin[2], left: b.margin[3] };
                ng.same_sheet(&cmp)
            });
            if ng.continuous && same && cur_sec != usize::MAX {
                // Föregående avsnitts spalter jämnas ut när det ryms på sidan.
                if sg.cols > 1 && !broke_page && !units.is_empty() {
                    let hs: Vec<f32> = units.iter().map(|u| u.h).collect();
                    let cap = balanced_height(&hs, sg.cols);
                    let (mut c, mut y) = (0usize, region_top);
                    let mut bottom = region_top;
                    for u in units.iter_mut() {
                        if y > region_top && y + u.h > region_top + cap + 0.01 && c + 1 < sg.cols {
                            c += 1;
                            y = region_top;
                        }
                        let (ny, nx) = (round(y), col_x(&boxes, &sg, c) - col_x(&boxes, &sg, u.col));
                        let dy = ny - u.top;
                        for r in u.rows.clone() {
                            g.rows[r].pos.y += dy;
                            g.rows[r].pos.x += nx;
                            dx[r] += nx;
                        }
                        u.top = ny;
                        u.col = c;
                        y += u.h;
                        bottom = bottom.max(y);
                    }
                    page_bottom = page_bottom.max(bottom);
                    prev_bottom = prev_bottom.max(bottom);
                }
                let b = boxes.last_mut().expect("minst ett ark");
                let top = round(page_bottom.max(region_top));
                b.regions.push(Region { y0: top, cols: ng.cols, col_w: ng.col_w, gap: ng.gap });
                region_top = top;
                region_page = boxes.len() - 1;
            } else {
                new_sheet(&mut boxes, geom, sec, &ng);
                skip_covered(&mut boxes, geom, sec, &ng, prev_bottom - eps);
                let b = boxes.last().expect("minst ett ark");
                region_top = b.text_top();
                region_page = boxes.len() - 1;
            }
            sg = ng;
            cur_sec = sec;
            col = 0;
            col_y = region_top;
            page_bottom = region_top;
            units.clear();
            broke_page = false;
        }

        // Spaltbrytning: nästa spalt (eller sida) innan raden placeras.
        let is_break = col_breaks.binary_search(&i).is_ok();
        let advance = |boxes: &mut Vec<PageBox>, col: &mut usize, col_y: &mut f32, region_top: &mut f32, region_page: &mut usize, page_bottom: &mut f32, broke: &mut bool, units: &mut Vec<Unit>| {
            if *col + 1 < sg.cols {
                *col += 1;
                *col_y = *region_top;
            } else {
                // Ny sida i samma avsnitt.
                new_sheet(boxes, geom, cur_sec, &sg);
                skip_covered(boxes, geom, cur_sec, &sg, prev_bottom - eps);
                let b = boxes.last().expect("minst ett ark");
                *region_top = b.text_top();
                *region_page = boxes.len() - 1;
                *col = 0;
                *col_y = *region_top;
                *page_bottom = *region_top;
                *broke = true;
                units.clear();
            }
        };
        if is_break {
            advance(&mut boxes, &mut col, &mut col_y, &mut region_top, &mut region_page, &mut page_bottom, &mut broke_page, &mut units);
        } else {
            let text_bottom = boxes[boxes.len() - 1].text_bottom();
            if col_y + h.max(fh) > text_bottom + eps && col_y > region_top + eps {
                advance(&mut boxes, &mut col, &mut col_y, &mut region_top, &mut region_page, &mut page_bottom, &mut broke_page, &mut units);
            }
        }
        let _ = region_page;

        let top = round(col_y);
        let dy = top - y0;
        let cx = col_x(&boxes, &sg, col);
        for (k, pr) in g.rows[i..end].iter_mut().enumerate() {
            pr.pos.y += dy;
            pr.pos.x += cx;
            dx[i + k] = cx;
        }
        units.push(Unit { rows: i..end, top, col, h });
        col_y = top + h;
        prev_bottom = prev_bottom.max(col_y).max(if fh > 0.0 { top + fh } else { f32::NEG_INFINITY });
        page_bottom = page_bottom.max(col_y);
        i = end;
    }

    // Galleriets mått: alla rader, och bredden upp till den högraste raden.
    let mut max_x = g.rect.max.x;
    let mut max_y = g.rect.max.y;
    g.mesh_bounds = Rect::NOTHING;
    for pr in &g.rows {
        max_x = max_x.max(pr.pos.x + pr.row.size.x);
        max_y = max_y.max(pr.pos.y + pr.row.size.y);
        g.mesh_bounds |= pr.row.visuals.mesh_bounds.translate(pr.pos.to_vec2());
    }
    g.rect.max.x = max_x.max(geom.edit_w());
    g.rect.max.y = max_y.max(prev_bottom);
    Paged { boxes, dx }
}

// ------------------------------------------------------------------ pekaren i spalter

/// Byteläget under pekaren (`local` i galleri-koordinater) om den står i en flerspaltig del av ett
/// ark. `Galley::cursor_from_pos` antar att raderna ligger under varandra; med spalter ligger de
/// bredvid varandra, så raden söks bland spaltens egna rader.
pub fn column_hit(text: &str, g: &Galley, boxes: &[PageBox], origin_x: f32, local: egui::Vec2) -> Option<usize> {
    let b = boxes.get(page_index(boxes, local.y))?;
    let r = b.region_at(local.y);
    if r.cols <= 1 {
        return None;
    }
    let (left, w) = column_at(boxes, origin_x, local.x, local.y);
    let (lo, hi) = (left - 0.5, left + w + r.gap / 2.0);
    let (top, bottom) = (b.top - 1.0, b.top + b.h + 1.0);
    let mut best: Option<(f32, usize)> = None;
    for (i, row) in g.rows.iter().enumerate() {
        if row.pos.x < lo || row.pos.x >= hi || row.pos.y < top || row.pos.y > bottom {
            continue;
        }
        let d = if local.y < row.pos.y {
            row.pos.y - local.y
        } else if local.y > row.pos.y + row.row.size.y {
            local.y - (row.pos.y + row.row.size.y)
        } else {
            0.0
        };
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    let (_, i) = best?;
    let idx = super::grid::GalleyIndex::new(text, g);
    let row = &g.rows[i];
    let col = row.char_at(local.x - row.pos.x).0.min(row.glyphs.len());
    Some(crate::app::char_to_byte(text, idx.start(i) + col))
}

/// Efter TextEdit: musklick och drag i flerspaltiga delar. Markören sätts om härifrån (TextEdit
/// väljer första raden med rätt y, som ligger i första spalten).
pub fn pointer_fix(
    ctx: &egui::Context,
    id: egui::Id,
    text: &str,
    g: &Galley,
    gpos: egui::Pos2,
    boxes: &[PageBox],
    origin_x: f32,
    resp: &egui::Response,
) {
    if !boxes.iter().any(|b| b.regions.iter().any(|r| r.cols > 1)) {
        return;
    }
    let key = egui::Id::unique("kladd-column-drag");
    let (pressed, down, shift, ptr) =
        ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.modifiers.shift, i.pointer.interact_pos()));
    if !down {
        ctx.data_mut(|d| d.remove::<usize>(key));
    }
    let Some(ptr) = ptr else { return };
    let hit = || column_hit(text, g, boxes, origin_x, ptr - gpos).filter(|b| super::markup::table_at(text, *b).is_none());
    let anchor = ctx.data(|d| d.get_temp::<usize>(key));
    if pressed && resp.contains_pointer() {
        match hit() {
            Some(b) => {
                let secondary = egui::text_edit::TextEditState::load(ctx, id)
                    .and_then(|s| s.cursor.char_range())
                    .map(|r| crate::app::char_to_byte(text, r.secondary.index.0));
                let from = if shift { secondary.unwrap_or(b) } else { b };
                super::tables::set_caret(ctx, id, text, from, b);
                ctx.data_mut(|d| d.insert_temp(key, from));
                ctx.request_repaint();
            }
            None => ctx.data_mut(|d| d.remove::<usize>(key)),
        }
    } else if down {
        if let (Some(from), Some(b)) = (anchor, hit()) {
            super::tables::set_caret(ctx, id, text, from, b);
            ctx.request_repaint();
        }
    }
}
