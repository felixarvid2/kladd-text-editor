//! Navigeringsfönstret (rubrikträd till vänster) och ritningen av innehållsförteckningen.

use std::collections::HashSet;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::text_edit::TextEditState;
use egui::{
    Align2, Color32, FontFamily, FontId, Frame, Id, Margin, Panel, Rect, ScrollArea, Sense, Stroke, TextEdit, Ui,
    pos2, vec2,
};

use super::layout::px;
use super::markup::DocDefaults;
use super::nav::{self, Heading};
use crate::app::{KladdApp, byte_to_char, char_to_byte, editor_id};
use crate::i18n::t;
use crate::tf;
use crate::theme::{self, Palette};

/// Navigeringsfönstrets tillstånd.
#[derive(Default)]
pub struct State {
    /// Texten i rubrikfiltret.
    pub filter: String,
    /// Hopfällda rubriker: (flik, nivå, text).
    collapsed: HashSet<(u64, u8, String)>,
    /// Rubrikerna för textläget, nycklade på (flik, längd, hash) så att texten bara skannas
    /// när den ändrats.
    scan: Option<((u64, usize, u64), Arc<Vec<Heading>>)>,
    /// Rubriken som markören stod i förra bildrutan (för att rulla fram den i listan).
    last_current: Option<usize>,
    /// Trädets synliga rubriker, cachade (nyckel: flik, rubriklistans adress, filter, hopfällningsversion).
    shown: Option<((u64, usize, String, u64), Arc<Vec<usize>>)>,
    /// Räknas upp när en rubrik fälls ihop eller ut.
    collapse_gen: u64,
}

const ROW_H: f32 = 26.0;
const INDENT: f32 = 14.0;

impl KladdApp {
    /// Sant när navigeringsfönstret ska visas för den aktiva fliken.
    pub(crate) fn nav_visible(&self) -> bool {
        self.settings.nav_pane && self.docs.get(self.active).is_some_and(|d| d.is_markdown())
    }

    /// Rubrikerna i den aktiva fliken: ur den cachade analysen i dokumentläget, annars en
    /// radskanning som görs om bara när texten ändrats.
    pub(crate) fn nav_headings(&mut self) -> Arc<Vec<Heading>> {
        let Some(doc) = self.docs.get(self.active) else { return Arc::default() };
        if doc.word_view {
            return self.word.headings(doc.id, &doc.text);
        }
        // Stora filer: ändringsräknaren i stället för en hash över hela texten varje bildruta.
        let rev = match self.big.rev(doc.id) {
            Some(r) => r,
            None => crate::word::layout::fast_hash(&doc.text),
        };
        let key = (doc.id, doc.text.len(), rev);
        if let Some((k, h)) = &self.nav.scan {
            if *k == key {
                return h.clone();
            }
        }
        let h = Arc::new(nav::headings_scan(&doc.text));
        self.nav.scan = Some((key, h.clone()));
        h
    }

    /// Markörens byteposition i den aktiva fliken.
    fn nav_caret(&self, ctx: &egui::Context) -> usize {
        let Some(doc) = self.docs.get(self.active) else { return 0 };
        if let Some(head) = self.big.caret(doc.id) {
            return head;
        }
        TextEditState::load(ctx, editor_id(doc.id))
            .and_then(|s| s.cursor.char_range())
            .map_or(0, |r| char_to_byte(&doc.text, r.primary.index.0))
    }

    /// Flyttar markören till `byte` och rullar dit (dokumentläget: till sidpositionen, textläget: till raden).
    pub(crate) fn goto_byte(&mut self, ctx: &egui::Context, byte: usize) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if let Some(v) = self.big.view(doc.id) {
            v.select(byte, byte);
            self.focus_editor = true;
            self.find.current = None;
            ctx.request_repaint();
            return;
        }
        let id = editor_id(doc.id);
        let ci = byte_to_char(&doc.text, byte);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
        state.store(ctx, id);
        self.scroll_to_char = Some(ci);
        self.focus_editor = true;
        self.find.current = None;
        ctx.request_repaint();
    }

    /// Infogar en innehållsförteckning vid markören.
    pub(crate) fn insert_toc(&mut self, ctx: &egui::Context) {
        self.word_edit(ctx, |t, s| nav::insert_toc(t, s.end));
    }

    /// Navigeringsfönstret: vänster sidopanel med rubrikerna som ett träd.
    pub(crate) fn nav_pane(&mut self, ui: &mut Ui, p: &Palette) {
        if !self.nav_visible() {
            return;
        }
        let ctx = ui.ctx().clone();
        let doc_id = self.docs[self.active].id;
        let heads = self.nav_headings();
        let caret = self.nav_caret(&ctx);
        let cur = nav::current(&heads, caret);
        let min_level = heads.iter().map(|h| h.level).min().unwrap_or(1);
        let width = ui.available_width();
        let filtering = !self.nav.filter.trim().is_empty();
        let is_collapsed = |set: &HashSet<(u64, u8, String)>, h: &Heading| {
            !set.is_empty() && set.contains(&(doc_id, h.level, h.text.clone()))
        };
        // Med tusentals rubriker byggs listan bara om när något ändrats (inte varje bildruta).
        let shown_key = (doc_id, Arc::as_ptr(&heads) as usize, self.nav.filter.clone(), self.nav.collapse_gen);
        let shown = match &self.nav.shown {
            Some((k, s)) if *k == shown_key => s.clone(),
            _ => {
                let s = Arc::new(nav::visible(&heads, &self.nav.filter, &|h| is_collapsed(&self.nav.collapsed, h)));
                self.nav.shown = Some((shown_key, s.clone()));
                s
            }
        };

        let mut goto: Option<usize> = None;
        let mut toggle: Option<usize> = None;
        let scroll_cur = cur != self.nav.last_current;
        Panel::left("kladd-nav")
            .resizable(true)
            .default_size(240.0)
            .size_range(160.0..=(width * 0.5).max(200.0))
            .frame(Frame::NONE.fill(p.editor).inner_margin(Margin { left: 10, right: 6, top: 12, bottom: 0 }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let filter = TextEdit::singleline(&mut self.nav.filter)
                    .id(Id::unique("kladd-nav-filter"))
                    .hint_text(t("Sök i rubriker"))
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(8, 5));
                if ui.add(filter).has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.nav.filter.clear();
                }
                ui.add_space(8.0);
                ScrollArea::vertical().id_salt(("kladd-nav-scroll", doc_id)).auto_shrink([false, false]).show(ui, |ui| {
                    if heads.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(t("Inga rubriker i dokumentet")).size(12.5).color(p.muted));
                        return;
                    }
                    if shown.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(t("Inga träffar")).size(12.5).color(p.muted));
                        return;
                    }
                    // Tusentals rubriker: bara raderna som syns får en egen plats, resten blir ett tomt block.
                    let (first, last) = if shown.len() > 3000 {
                        let (clip, top) = (ui.clip_rect(), ui.next_widget_position().y);
                        let first = (((clip.top() - top) / ROW_H).floor().max(0.0) as usize).min(shown.len());
                        let last = ((((clip.bottom() - top) / ROW_H).ceil().max(0.0) as usize) + 1).min(shown.len());
                        if scroll_cur {
                            if let Some(pos) = shown.iter().position(|&i| Some(i) == cur) {
                                let at = pos2(ui.next_widget_position().x, top + pos as f32 * ROW_H);
                                ui.scroll_to_rect(Rect::from_min_size(at, vec2(ui.available_width(), ROW_H)), None);
                            }
                        }
                        (first, last)
                    } else {
                        (0, shown.len())
                    };
                    if first > 0 {
                        ui.allocate_space(vec2(ui.available_width(), first as f32 * ROW_H));
                    }
                    for &i in &shown[first..last] {
                        let h = &heads[i];
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                        let is_cur = cur == Some(i);
                        if is_cur && scroll_cur {
                            ui.scroll_to_rect(rect, None);
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let painter = ui.painter();
                        if is_cur {
                            painter.rect_filled(rect, 6.0, p.current_line);
                            painter.rect_filled(
                                Rect::from_min_size(rect.min + vec2(0.0, 5.0), vec2(2.5, ROW_H - 10.0)),
                                1.0,
                                p.accent,
                            );
                        } else if resp.hovered() {
                            painter.rect_filled(rect, 6.0, p.surface_hover);
                        }
                        let indent = if filtering { 0.0 } else { f32::from(h.level - min_level) * INDENT };
                        let x0 = rect.left() + 8.0 + indent;
                        // Hopfällningspil: bara för rubriker med underrubriker.
                        if !filtering && nav::has_children(&heads, i) {
                            let c = pos2(x0 + 5.0, rect.center().y);
                            let s = Stroke::new(1.4, p.muted);
                            if is_collapsed(&self.nav.collapsed, h) {
                                painter.line_segment([c + vec2(-1.5, -3.5), c + vec2(2.0, 0.0)], s);
                                painter.line_segment([c + vec2(2.0, 0.0), c + vec2(-1.5, 3.5)], s);
                            } else {
                                painter.line_segment([c + vec2(-3.5, -1.5), c + vec2(0.0, 2.0)], s);
                                painter.line_segment([c + vec2(0.0, 2.0), c + vec2(3.5, -1.5)], s);
                            }
                            let arrow = Rect::from_min_size(pos2(x0 - 3.0, rect.top()), vec2(18.0, ROW_H));
                            if resp.clicked() && resp.interact_pointer_pos().is_some_and(|pos| arrow.contains(pos)) {
                                toggle = Some(i);
                                continue;
                            }
                        }
                        let tx = x0 + 16.0;
                        let (label, color) = if h.text.is_empty() {
                            (t("(utan text)").to_owned(), p.faint)
                        } else {
                            (h.text.clone(), if is_cur { p.accent } else if h.level <= 1 { p.text } else { p.muted })
                        };
                        let font = if h.level <= 1 {
                            FontId::new(13.5, theme::semibold())
                        } else {
                            FontId::new(13.0, FontFamily::Proportional)
                        };
                        let mut job = LayoutJob::simple_singleline(label, font, color);
                        job.wrap.max_width = (rect.right() - tx - 6.0).max(10.0);
                        job.wrap.max_rows = 1;
                        job.wrap.break_anywhere = true;
                        let g = painter.layout_job(job);
                        painter.galley(pos2(tx, rect.center().y - g.size().y / 2.0), g, color);
                        if resp.clicked() {
                            goto = Some(h.start);
                        }
                        resp.on_hover_text(tf!("Rubrik {level}: {text}", level = h.level, text = h.text));
                    }
                    if last < shown.len() {
                        ui.allocate_space(vec2(ui.available_width(), (shown.len() - last) as f32 * ROW_H));
                    }
                    ui.add_space(16.0);
                });
            });
        self.nav.last_current = cur;
        if let Some(i) = toggle {
            let h = &heads[i];
            let key = (doc_id, h.level, h.text.clone());
            if !self.nav.collapsed.remove(&key) {
                self.nav.collapsed.insert(key);
            }
            self.nav.collapse_gen += 1;
        }
        if let Some(b) = goto {
            self.goto_byte(&ctx, b);
        }
    }
}

/// Ritar en innehållsförteckning i `rect` (översta vänstra hörnet av raden som reserverats).
/// `pages[i]` är sidnumret (från 1) för post `i`. Ger byteposition för en rubrik som klickats.
#[allow(clippy::too_many_arguments)]
pub fn paint_toc(
    ui: &mut Ui,
    p: &Palette,
    z: f32,
    d: &DocDefaults,
    family: &dyn Fn(bool) -> FontFamily,
    rect: Rect,
    key: Id,
    entries: &[&Heading],
    pages: &[usize],
    shown: usize,
) -> Option<usize> {
    let (title_h, row_h, _) = nav::toc_metrics(z, d);
    let painter = ui.painter().clone();
    let title = FontId::new(px(z, 18.0), family(true));
    painter.text(pos2(rect.left(), rect.top() + px(z, 2.0)), Align2::LEFT_TOP, "Innehåll", title, p.page_heading1);
    let size = px(z, d.size());
    let font = FontId::new(size, family(false));
    let mut y = rect.top() + title_h;
    if entries.is_empty() {
        painter.text(pos2(rect.left(), y), Align2::LEFT_TOP, t("Inga rubriker i dokumentet"), font, p.page_muted);
        return None;
    }
    let dot_w = painter.layout_no_wrap(". ".into(), font.clone(), Color32::WHITE).size().x.max(1.0);
    let mut clicked = None;
    for (i, h) in entries.iter().take(shown).enumerate() {
        let row = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), row_h));
        y += row_h;
        let resp = ui.interact(row, key.with(i), Sense::click());
        let hovered = resp.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let color: Color32 = if hovered { p.page_link } else { p.page_text };
        let x = row.left() + f32::from(h.level - 1) * px(z, 14.0);
        let num = pages.get(i).map_or(String::new(), |n| n.to_string());
        let num_g = painter.layout_no_wrap(num, font.clone(), color);
        let mut job = LayoutJob::simple_singleline(h.text.clone(), font.clone(), color);
        job.wrap.max_width = (row.right() - x - num_g.size().x - px(z, 30.0)).max(20.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        let text_g = painter.layout_job(job);
        let cy = row.center().y;
        let text_right = x + text_g.size().x;
        painter.galley(pos2(x, cy - text_g.size().y / 2.0), text_g, color);
        let num_left = row.right() - num_g.size().x;
        painter.galley(pos2(num_left, cy - num_g.size().y / 2.0), num_g, color);
        let avail = num_left - text_right - px(z, 12.0);
        if avail > dot_w {
            let n = (avail / dot_w).floor() as usize;
            let lg = painter.layout_no_wrap(". ".repeat(n), font.clone(), p.page_muted);
            painter.galley(pos2(num_left - px(z, 6.0) - lg.size().x, cy - lg.size().y / 2.0), lg, p.page_muted);
        }
        if resp.clicked() {
            clicked = Some(h.start);
        }
    }
    if entries.len() > shown {
        painter.text(pos2(rect.left(), y), Align2::LEFT_TOP, "…", font, p.page_muted);
    }
    clicked
}
