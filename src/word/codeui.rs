//! Ritar kodblockens ruta i dokumentläget: svag bakgrund bakom texten, språknamnet i hörnet och en
//! kopieringsknapp som visas när musen är över blocket. Själva koden (färgad per token) läggs ut av
//! `layout`; rutan följer radernas lägen i galleriet och delas där en sidbrytning går igenom blocket.

use std::ops::Range;

use egui::epaint::text::Galley;
use egui::{Color32, CursorIcon, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::codeblock::{self, Block};
use super::grid::GalleyIndex;
use super::layout::{CODE_PAD_PT, px};
use crate::i18n::t;
use crate::theme::Palette;

/// Hur länge bocken visas efter en kopiering (sekunder).
const CHECK_SECS: f64 = 1.4;

#[derive(Default)]
pub struct Out {
    /// Rutorna, att ritas bakom texten.
    pub fills: Vec<Shape>,
    /// Koden som användaren bad om att få kopierad.
    pub copy: Option<String>,
}

/// En sammanhängande del av ett block (ett block som går över en sidbrytning har flera): överkant och nederkant.
pub fn spans(rows: &[(f32, f32)], gap: f32) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::new();
    for &(top, bottom) in rows {
        match out.last_mut() {
            Some(last) if top - last.1 <= gap => last.1 = last.1.max(bottom),
            _ => out.push((top, bottom)),
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub fn paint(
    ui: &Ui,
    p: &Palette,
    zoom: f32,
    doc_id: u64,
    text: &str,
    galley: &Galley,
    gpos: Pos2,
    cb: &[Block],
    vis: &Range<usize>,
    clip: Rect,
    col_at: &dyn Fn(f32, f32) -> (f32, f32),
    diagram: &dyn Fn(&Block) -> Option<(String, Vec2)>,
) -> Out {
    let mut out = Out::default();
    let blocks: Vec<&Block> = codeblock::in_range(cb, vis).collect();
    if blocks.is_empty() {
        return out;
    }
    let idx = GalleyIndex::new(text, galley);
    let pad = px(zoom, CODE_PAD_PT);
    let padx = px(zoom, 6.0);
    let hover = ui.input(|i| i.pointer.hover_pos());
    let now = ui.input(|i| i.time);
    let stroke = Stroke::new(1.0, p.page_markup.gamma_multiply(0.45));
    for b in blocks {
        let w = b.whole();
        let rows = idx.rows(idx.chr(w.start), idx.chr(w.end));
        let ext: Vec<(f32, f32)> = galley.rows[rows.clone()].iter().map(|r| (r.pos.y, r.pos.y + r.row.size.y)).collect();
        if ext.is_empty() {
            continue;
        }
        let first_x = galley.rows[rows.start].pos.x;
        for (si, (top, bottom)) in spans(&ext, pad * 3.0 + 3.0).into_iter().enumerate() {
            let (cl, cw) = col_at(first_x, top);
            let mut rect = Rect::from_min_max(pos2(gpos.x + cl - padx, gpos.y + top), pos2(gpos.x + cl + cw + padx, gpos.y + bottom));
            if rect.bottom() < clip.top() || rect.top() > clip.bottom() {
                continue;
            }
            // Ett diagram ritas i stället för rutan: mitt i spalten, på den första radens plats.
            if let Some((uri, size)) = diagram(b).filter(|_| si == 0) {
                let x = gpos.x + cl + (cw - size.x) / 2.0;
                let img = Rect::from_min_size(pos2(x, gpos.y + top + pad), size);
                match crate::word::images::image_shape(ui.ctx(), &uri, img, crate::word::images::crop_uv(None)) {
                    Some(sh) => out.fills.push(sh),
                    None => ui.ctx().request_repaint_after(std::time::Duration::from_millis(50)),
                }
                rect = img.expand(px(zoom, 4.0));
            } else {
                out.fills.push(Shape::rect_filled(rect, px(zoom, 3.0), p.page_code_bg));
                out.fills.push(Shape::rect_stroke(rect, px(zoom, 3.0), stroke, StrokeKind::Inside));
            }
            if si != 0 {
                continue;
            }
            // Språknamnet och kopieringsknappen i det övre högra hörnet.
            let hovered = hover.is_some_and(|q| rect.contains(q));
            let label_font = FontId::proportional(px(zoom, 8.0).max(8.0));
            let mut right = rect.right() - px(zoom, 6.0);
            let y = rect.top() + px(zoom, 2.0);
            if !b.lang.is_empty() {
                let g = ui.painter().layout_no_wrap(b.lang.clone(), label_font.clone(), p.page_muted);
                let pos = pos2(right - g.size().x, y);
                ui.painter().galley(pos, g.clone(), p.page_muted);
                right = pos.x - px(zoom, 6.0);
            }
            if !hovered {
                continue;
            }
            let size = px(zoom, 14.0).max(14.0);
            let btn = Rect::from_min_size(pos2(right - size, y - px(zoom, 1.0)), vec2(size, size));
            let id = Id::unique(("kladd-codecopy", doc_id, b.open.start));
            let resp = ui.interact(btn, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(t("Kopiera kod"));
            let done_key = Id::unique(("kladd-codecopied", doc_id, b.open.start));
            if resp.clicked() {
                out.copy = Some(b.code(text).to_owned());
                ui.ctx().data_mut(|d| d.insert_temp(done_key, now));
            }
            let done = ui.ctx().data(|d| d.get_temp::<f64>(done_key)).is_some_and(|t0| now - t0 < CHECK_SECS);
            if done {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
            let fill = if resp.hovered() { p.page_markup.gamma_multiply(0.35) } else { Color32::TRANSPARENT };
            let painter = ui.painter();
            painter.rect_filled(btn, px(zoom, 3.0), fill);
            copy_icon(painter, btn, p.page_muted, p.page_code_bg, done);
        }
    }
    out
}

/// Kopieringsikonen (två ark) eller, när koden just kopierats, en bock.
fn copy_icon(painter: &egui::Painter, r: Rect, color: Color32, bg: Color32, done: bool) {
    let c = r.center();
    let s = Stroke::new(1.2, color);
    if done {
        let k = r.width() * 0.28;
        painter.line_segment([pos2(c.x - k, c.y), pos2(c.x - k * 0.3, c.y + k * 0.8)], s);
        painter.line_segment([pos2(c.x - k * 0.3, c.y + k * 0.8), pos2(c.x + k, c.y - k * 0.8)], s);
        return;
    }
    let k = r.width() * 0.22;
    let back = Rect::from_min_size(pos2(c.x - k * 1.5, c.y - k * 1.5), vec2(k * 2.2, k * 2.4));
    let front = Rect::from_min_size(pos2(c.x - k * 0.7, c.y - k * 0.9), vec2(k * 2.2, k * 2.4));
    painter.rect_stroke(back, 1.5, s, StrokeKind::Inside);
    painter.rect_filled(front, 1.5, bg);
    painter.rect_stroke(front, 1.5, s, StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_join_rows_and_split_at_page_gaps() {
        let rows = [(0.0, 1.0), (5.0, 20.0), (20.0, 35.0), (35.0, 36.0), (200.0, 215.0), (215.0, 216.0)];
        let s = spans(&rows, 12.0);
        assert_eq!(s, vec![(0.0, 36.0), (200.0, 216.0)]);
        assert!(spans(&[], 5.0).is_empty());
    }
}
