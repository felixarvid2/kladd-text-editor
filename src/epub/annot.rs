//! Bokmärken, understrykningar och anteckningar i läsvyn: menyn vid en markering, markeringarna som
//! färgade bakgrunder, listorna i sidopanelen och exporten. Förankring och export som ren logik finns i
//! `anchor.rs`. Modulen är en undermodul till `view` för att nå flikens privata tillstånd.

use std::ops::Range;

use egui::{Align2, Color32, Context, FontFamily, FontId, Frame, Id, Key, Margin, Order, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, pos2, vec2};
use egui_commonmark::Mark;

use super::{EpubTab, truncate};
use crate::epub::anchor::{self, COLORS, Note};
use crate::i18n::t;
use crate::theme::{self, Palette};
use crate::tf;

/// Vad sidopanelen visar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SideTab {
    #[default]
    Toc,
    Bookmarks,
    Notes,
}

/// Det menyn gäller: en ny markering (ännu inte sparad) eller en befintlig understrykning.
#[derive(Clone, Debug)]
pub enum Target {
    New { chapter: usize, range: Range<usize>, text: String },
    Existing(u64),
}

/// Menyn vid en markering eller en understrykning.
pub struct AnnMenu {
    pub target: Target,
    pub pos: egui::Pos2,
    /// Anteckningen skrivs just nu.
    pub editing: bool,
    pub buf: String,
    /// Öppnades i den här bildrutan: klicket som öppnade den får inte stänga den.
    pub fresh: bool,
    pub focus: bool,
}

/// Understrykningens färg (genomskinlig, så att texten syns) för färgnummer `idx`.
pub fn mark_color(idx: u8, dark: bool) -> Color32 {
    let (r, g, b) = swatch_rgb(idx);
    Color32::from_rgba_unmultiplied(r, g, b, if dark { 92 } else { 110 })
}

fn swatch_rgb(idx: u8) -> (u8, u8, u8) {
    [(247, 200, 28), (86, 196, 108), (84, 162, 245), (244, 120, 176)][usize::from(idx) % COLORS]
}

pub fn swatch_color(idx: u8) -> Color32 {
    let (r, g, b) = swatch_rgb(idx);
    Color32::from_rgb(r, g, b)
}

pub fn color_name(idx: u8) -> &'static str {
    [t("Gul"), t("Grön"), t("Blå"), t("Rosa")][usize::from(idx) % COLORS]
}

impl EpubTab {
    /// Sätter bokens nyckel och sparade poster (när boken öppnats).
    pub fn set_notes(&mut self, key: String, notes: Vec<Note>) {
        self.key = key;
        self.notes = notes;
        self.notes_gen += 1;
        self.dirty = false;
    }

    /// Har posterna ändrats sedan sist? (Appen sparar dem då.)
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    fn changed(&mut self) {
        self.notes_gen += 1;
        self.dirty = true;
    }

    fn next_id(&self) -> u64 {
        self.notes.iter().map(|n| n.id).max().map_or(1, |m| m + 1)
    }

    /// Räknar om var posterna står i kapitlens Markdown när något ändrats.
    fn ensure_resolved(&mut self) {
        if self.resolved_gen == Some(self.notes_gen) {
            return;
        }
        let mut cache: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
        let mut out = Vec::with_capacity(self.notes.len());
        for n in &self.notes {
            let md = cache.entry(n.chapter).or_insert_with(|| self.book.chapter_md(n.chapter));
            out.push((n.id, anchor::resolve(md, n)));
        }
        self.resolved = out;
        self.resolved_gen = Some(self.notes_gen);
    }

    fn range_of(&self, id: u64) -> Option<Range<usize>> {
        self.resolved.iter().find(|(i, _)| *i == id).and_then(|(_, r)| r.clone())
    }

    pub fn note(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    pub fn bookmarks(&self) -> Vec<&Note> {
        let mut v: Vec<&Note> = self.notes.iter().filter(|n| n.bookmark).collect();
        v.sort_by_key(|n| (n.chapter, n.start));
        v
    }

    pub fn highlights(&self) -> Vec<&Note> {
        let mut v: Vec<&Note> = self.notes.iter().filter(|n| !n.bookmark).collect();
        v.sort_by_key(|n| (n.chapter, n.start));
        v
    }

    /// Markeringarna i del `si` som färgade intervall (och vilken post varje hör till).
    pub(super) fn marks_for_section(&mut self, si: usize, dark: bool) -> (Vec<Mark>, Vec<u64>) {
        self.ensure_resolved();
        let sec = &self.book.sections[si];
        let (lo, hi) = (sec.offset, sec.offset + sec.md.len());
        let mut marks = Vec::new();
        let mut ids = Vec::new();
        for n in self.notes.iter().filter(|n| !n.bookmark && n.chapter == sec.chapter) {
            let Some(r) = self.range_of(n.id) else { continue };
            let (a, b) = (r.start.max(lo), r.end.min(hi));
            if a < b {
                marks.push(Mark { range: a - lo..b - lo, color: mark_color(n.color, dark), note: !n.note.trim().is_empty() });
                ids.push(n.id);
            }
        }
        (marks, ids)
    }

    /// Området (byte i delens Markdown) som syns i rullytan.
    fn visible_bytes(&self) -> Range<usize> {
        let id = self.view_id(self.section);
        let pts = self.cache.line_points(&id);
        let top = self.scroll - self.md_top + 4.0;
        let bottom = self.scroll - self.md_top + self.view_h;
        let len = self.book.sections[self.section].md.len();
        let lo = crate::mdread::line_at(pts, top).unwrap_or(0);
        let hi = pts.iter().filter(|(_, y)| *y > bottom).map(|(b, _)| *b).min().unwrap_or(len);
        lo..hi.max(lo)
    }

    /// Bokmärken som ligger på skärmen nu (de som en tryckning på knappen tar bort).
    pub(super) fn bookmarks_here(&mut self) -> Vec<u64> {
        self.ensure_resolved();
        let sec = &self.book.sections[self.section];
        let vis = self.visible_bytes();
        let (lo, hi) = (sec.offset + vis.start, sec.offset + vis.end);
        let ch = sec.chapter;
        self.notes
            .iter()
            .filter(|n| n.bookmark && n.chapter == ch)
            .filter(|n| self.range_of(n.id).is_some_and(|r| r.start >= lo && r.start < hi.max(lo + 1)))
            .map(|n| n.id)
            .collect()
    }

    /// Bokmärke på stället som visas, eller bort med det som redan finns där.
    pub fn toggle_bookmark(&mut self) {
        let here = self.bookmarks_here();
        if !here.is_empty() {
            self.notes.retain(|n| !here.contains(&n.id));
            self.changed();
            return;
        }
        let sec = &self.book.sections[self.section];
        let (chapter, at) = (sec.chapter, sec.offset + self.visible_bytes().start);
        let md = self.book.chapter_md(chapter);
        let id = self.next_id();
        self.notes.push(anchor::bookmark_at(id, chapter, &md, at));
        self.changed();
    }

    pub fn remove_note(&mut self, id: u64) {
        let before = self.notes.len();
        self.notes.retain(|n| n.id != id);
        if self.notes.len() != before {
            self.changed();
        }
        if matches!(&self.menu, Some(AnnMenu { target: Target::Existing(i), .. }) if *i == id) {
            self.menu = None;
        }
    }

    /// Hoppar till en post (med Tillbaka).
    pub fn goto_note(&mut self, id: u64) {
        self.ensure_resolved();
        let Some(n) = self.note(id) else { return };
        let ch = n.chapter;
        let Some(r) = self.range_of(id) else { return };
        if let Some(d) = self.book.dest_in_chapter(ch, r.start) {
            self.follow(d);
        }
    }

    /// Posternas utdrag som Markdown (se `anchor::export_markdown`).
    pub fn export_markdown(&self) -> String {
        anchor::export_markdown(&self.book.title(), &self.book.meta.authors, &self.book.chapter_titles(), &self.notes)
    }

    // ---------------------------------------------------------------- markering → meny

    /// En markering är klar: öppnar menyn om den går att placera i boken.
    pub(super) fn on_selection(&mut self, text: String, pos: egui::Pos2, mid: Rect) {
        if text.trim().is_empty() || !mid.contains(pos) || self.note.is_some() || self.about {
            return;
        }
        let sec = &self.book.sections[self.section];
        if sec.cover.is_some() {
            return;
        }
        // Var i texten pekaren är (för att välja rätt träff när samma text står på flera ställen).
        let rel = pos.y - (mid.top() - self.scroll + self.md_top);
        let near = crate::mdread::line_at(self.cache.line_points(&self.view_id(self.section)), rel).unwrap_or(0);
        let Some(r) = anchor::locate(&sec.md, &text, near) else { return };
        let target = Target::New { chapter: sec.chapter, range: r.start + sec.offset..r.end + sec.offset, text };
        self.menu = Some(AnnMenu { target, pos, editing: false, buf: String::new(), fresh: true, focus: false });
    }

    /// Klick på en understrykning: menyn för den.
    pub(super) fn on_mark_click(&mut self, mark_index: usize, pos: egui::Pos2) {
        let Some(id) = self.mark_ids.get(mark_index).copied() else { return };
        self.menu = Some(AnnMenu { target: Target::Existing(id), pos, editing: false, buf: String::new(), fresh: true, focus: false });
    }

    /// Skapar understrykningen för en ny markering och ger dess id.
    fn commit_new(&mut self, color: u8) -> Option<u64> {
        let Some(AnnMenu { target: Target::New { chapter, range, .. }, .. }) = &self.menu else { return None };
        let (chapter, range) = (*chapter, range.clone());
        let md = self.book.chapter_md(chapter);
        let id = self.next_id();
        let mut n = anchor::make(id, chapter, &md, range, false);
        n.color = color;
        self.notes.push(n);
        self.changed();
        Some(id)
    }

    /// Ritar menyn vid markeringen.
    pub(super) fn ann_menu(&mut self, ctx: &Context, p: &Palette, anchor_rect: Rect) {
        let Some(menu) = &mut self.menu else { return };
        let fresh = std::mem::take(&mut menu.fresh);
        let width = if menu.editing { 320.0 } else { 262.0 };
        let target = menu.target.clone();
        let editing = menu.editing;
        let existing = match &target {
            Target::Existing(id) => self.notes.iter().find(|n| n.id == *id).map(|n| (n.color, n.note.clone(), n.quote.clone())),
            Target::New { .. } => None,
        };
        if matches!(target, Target::Existing(_)) && existing.is_none() {
            self.menu = None;
            return;
        }
        let at = self.menu.as_ref().map_or(pos2(0.0, 0.0), |m| m.pos);
        let x = (at.x - width / 2.0).clamp(anchor_rect.left() + 8.0, (anchor_rect.right() - width - 8.0).max(anchor_rect.left() + 8.0));
        let y = (at.y + 14.0).min(anchor_rect.bottom() - 70.0).max(anchor_rect.top() + 40.0);
        let mut pick: Option<u8> = None;
        let mut want_note = false;
        let mut copy = false;
        let mut delete = false;
        let mut save = false;
        let mut cancel = false;
        let area = egui::Area::new(Id::unique("kladd-epub-annmenu")).order(Order::Foreground).fixed_pos(pos2(x, y)).show(ctx, |ui| {
            Frame::NONE
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(11)
                .inner_margin(Margin::same(10))
                .shadow(ui.visuals().popup_shadow)
                .show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    ui.spacing_mut().item_spacing = vec2(6.0, 8.0);
                    if editing {
                        let id = Id::unique("kladd-epub-annmenu-edit");
                        let focus = self.menu.as_ref().is_some_and(|m| !m.focus);
                        if let Some(m) = &mut self.menu {
                            let te = ui.add(
                                TextEdit::multiline(&mut m.buf)
                                    .id(id)
                                    .hint_text(t("Skriv en anteckning"))
                                    .desired_rows(4)
                                    .desired_width(f32::INFINITY)
                                    .font(FontId::new(13.5, FontFamily::Proportional)),
                            );
                            if focus {
                                te.request_focus();
                                m.focus = true;
                            }
                        }
                        ui.horizontal(|ui| {
                            if ui.add(crate::app::quiet_button(t("Spara"), p).min_size(vec2(72.0, 28.0))).clicked() {
                                save = true;
                            }
                            if ui.add(crate::app::quiet_button(t("Avbryt"), p).min_size(vec2(72.0, 28.0))).clicked() {
                                cancel = true;
                            }
                        });
                        return;
                    }
                    ui.horizontal(|ui| {
                        let cur = existing.as_ref().map(|e| e.0);
                        for i in 0..COLORS as u8 {
                            let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                            let col = swatch_color(i);
                            ui.painter().circle_filled(r.center(), 10.0, col);
                            if cur == Some(i) {
                                ui.painter().circle_stroke(r.center(), 12.5, Stroke::new(2.0, p.text));
                            } else if resp.hovered() {
                                ui.painter().circle_stroke(r.center(), 12.5, Stroke::new(1.5, p.muted));
                            }
                            if resp.on_hover_text(color_name(i)).clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        let btn = |ui: &mut Ui, s: &str| ui.add(crate::app::quiet_button(s, p).min_size(vec2(0.0, 28.0))).clicked();
                        let label = match &existing {
                            Some((_, note, _)) if !note.trim().is_empty() => t("Redigera anteckning…"),
                            _ => t("Anteckning…"),
                        };
                        if btn(ui, label) {
                            want_note = true;
                        }
                        if btn(ui, t("Kopiera")) {
                            copy = true;
                        }
                        if existing.is_some() && btn(ui, t("Ta bort")) {
                            delete = true;
                        }
                    });
                    if let Some((_, note, _)) = &existing {
                        if !note.trim().is_empty() {
                            ui.label(RichText::new(truncate(note, 160)).size(12.5).italics().color(p.muted));
                        }
                    }
                });
        });
        // Esc och klick utanför stänger menyn.
        let outside = !fresh
            && ctx.input(|i| i.pointer.primary_clicked())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| !area.response.rect.contains(pos));
        if outside || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            self.menu = None;
            return;
        }
        if cancel {
            self.menu = None;
            return;
        }
        // Färgval: en ny markering sparas med färgen, en befintlig byter färg.
        if let Some(i) = pick {
            match &target {
                Target::New { .. } => {
                    self.commit_new(i);
                }
                Target::Existing(id) => {
                    if let Some(n) = self.notes.iter_mut().find(|n| n.id == *id) {
                        n.color = i;
                        self.changed();
                    }
                }
            }
            self.menu = None;
            clear_text_selection(ctx);
            return;
        }
        if want_note {
            let id = match &target {
                Target::New { .. } => self.commit_new(0),
                Target::Existing(id) => Some(*id),
            };
            if let (Some(id), Some(m)) = (id, &mut self.menu) {
                m.buf = self.notes.iter().find(|n| n.id == id).map(|n| n.note.clone()).unwrap_or_default();
                m.target = Target::Existing(id);
                m.editing = true;
                m.focus = false;
            }
            return;
        }
        if copy {
            let text = match &target {
                Target::New { text, .. } => text.clone(),
                Target::Existing(_) => existing.as_ref().map(|e| e.2.clone()).unwrap_or_default(),
            };
            ctx.copy_text(text);
            self.menu = None;
            clear_text_selection(ctx);
            return;
        }
        if delete {
            if let Target::Existing(id) = target {
                self.remove_note(id);
            }
            self.menu = None;
            return;
        }
        if save {
            if let Some(AnnMenu { target: Target::Existing(id), buf, .. }) = &self.menu {
                let (id, text) = (*id, buf.trim().to_owned());
                if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
                    n.note = text;
                    self.changed();
                }
            }
            self.menu = None;
            clear_text_selection(ctx);
        }
    }

    // ---------------------------------------------------------------- sidopanelens listor

    /// Listan över bokmärken (`bookmarks`) eller understrykningar. Ger stället som valts.
    pub(super) fn note_list(&mut self, ui: &mut Ui, p: &Palette, bookmarks: bool) -> Option<u64> {
        let book = self.book.clone();
        let items: Vec<(u64, usize, String, u8, String)> = if bookmarks { self.bookmarks() } else { self.highlights() }
            .into_iter()
            .map(|n| (n.id, n.chapter, n.quote.clone(), n.color, n.note.clone()))
            .collect();
        if items.is_empty() {
            let text = if bookmarks {
                t("Inga bokmärken. Tryck Ctrl+D för att sätta ett på stället du läser.")
            } else {
                t("Inga understrykningar. Markera text i boken för att understryka den eller skriva en anteckning.")
            };
            ui.add_space(6.0);
            ui.label(RichText::new(text).size(12.5).color(p.muted));
            return None;
        }
        let mut goto = None;
        let mut delete = None;
        for (id, ch, quote, color, note) in &items {
            let chapter = book.chapters.get(*ch).map_or(String::new(), |c| c.title.clone());
            let has_note = !note.trim().is_empty();
            let h = if bookmarks { 48.0 } else if has_note { 88.0 } else { 66.0 };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let painter = ui.painter();
            if resp.hovered() {
                painter.rect_filled(rect, 6.0, p.surface_hover);
            }
            let x = rect.left() + if bookmarks { 8.0 } else { 14.0 };
            if !bookmarks {
                painter.rect_filled(Rect::from_min_size(rect.min + vec2(3.0, 6.0), vec2(3.0, h - 12.0)), 1.5, swatch_color(*color));
            }
            painter.text(pos2(x, rect.top() + 12.0), Align2::LEFT_CENTER, truncate(&chapter, 40), FontId::new(11.5, theme::medium()), p.muted);
            let w = rect.right() - x - 26.0;
            let lines = if bookmarks { 1 } else { 2 };
            let mut job = egui::text::LayoutJob::simple(quote.clone(), FontId::new(12.5, FontFamily::Proportional), p.text, w.max(40.0));
            job.wrap.max_rows = lines;
            let g = painter.layout_job(job);
            painter.galley(pos2(x, rect.top() + 24.0), g.clone(), p.text);
            if has_note && !bookmarks {
                let mut job = egui::text::LayoutJob::simple(note.clone(), FontId::new(12.0, FontFamily::Proportional), p.muted, w.max(40.0));
                job.wrap.max_rows = 2;
                let gn = painter.layout_job(job);
                painter.galley(pos2(x, rect.top() + 24.0 + g.size().y + 4.0), gn, p.muted);
            }
            // Ta bort: ett kryss när raden är under pekaren.
            let xr = Rect::from_center_size(pos2(rect.right() - 13.0, rect.top() + 13.0), vec2(20.0, 20.0));
            if resp.hovered() {
                let over = ui.input(|i| i.pointer.hover_pos()).is_some_and(|pp| xr.contains(pp));
                if over {
                    ui.painter().rect_filled(xr, 5.0, p.current_line);
                }
                ui.painter().text(xr.center(), Align2::CENTER_CENTER, "×", FontId::new(14.0, FontFamily::Proportional), if over { p.text } else { p.muted });
                if resp.clicked() && over {
                    delete = Some(*id);
                    continue;
                }
            }
            if resp.clicked() {
                goto = Some(*id);
            }
        }
        if let Some(id) = delete {
            self.remove_note(id);
        }
        goto
    }
}

fn clear_text_selection(ctx: &Context) {
    if let Some(h) = ctx.plugin_opt::<egui::text_selection::LabelSelectionState>() {
        h.lock().clear_selection();
    }
}

/// Texten på fliken i sidopanelen, med antal.
pub fn tab_label(tab: SideTab, notes: &[Note]) -> String {
    match tab {
        SideTab::Toc => t("Innehåll").to_owned(),
        SideTab::Bookmarks => {
            let n = notes.iter().filter(|n| n.bookmark).count();
            if n == 0 { t("Bokmärken").to_owned() } else { tf!("Bokmärken ({n})", n = n) }
        }
        SideTab::Notes => {
            let n = notes.iter().filter(|n| !n.bookmark).count();
            if n == 0 { t("Anteckningar").to_owned() } else { tf!("Anteckningar ({n})", n = n) }
        }
    }
}

/// Anpassar `ScrollArea` för listorna.
pub fn list_scroll(salt: &str) -> ScrollArea {
    ScrollArea::vertical().id_salt(salt.to_owned()).auto_shrink([false, false])
}
