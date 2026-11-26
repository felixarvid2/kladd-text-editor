//! EPUB-läsläget i appen: flikens sida, sidopanel, överlägg, tangenter och lägesminne. Filen är en
//! undermodul till `app` (se `#[path]` i `app.rs`) för att nå appens privata fält och funktioner,
//! på samma sätt som `commands.rs`.

use egui::{Align2, CentralPanel, Color32, FontId, Frame, Rect, RichText, Sense, Ui, pos2, vec2};

use super::KladdApp;
use crate::epub::store::{self, CARD_W, SHELF_MAX};
use crate::epub::view::{self, Action, MAX_MEMORY};
use crate::i18n::t;
use crate::tf;
use crate::theme::{self, Palette};

impl KladdApp {
    /// Är den aktiva fliken en bok i EPUB-läsläget?
    pub(crate) fn epub_mode(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| d.epub.is_some())
    }

    fn epub_tab_mut(&mut self) -> Option<&mut view::EpubTab> {
        self.docs.get_mut(self.active).and_then(|d| d.epub.as_deref_mut())
    }

    /// Läsytan i mitten.
    pub(crate) fn epub_page(&mut self, ui: &mut Ui, p: &Palette) {
        let fill = view::page_fill(&self.settings.epub, p);
        let ctx = ui.ctx().clone();
        let mut action = None;
        CentralPanel::default().frame(Frame::NONE.fill(fill)).show(ui, |ui| {
            self.editor_rect = ui.max_rect();
            let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.epub.as_deref_mut()) else { return };
            action = tab.show(ui, p, &mut self.settings.epub);
        });
        if let Some(Action::EditCopy) = action {
            self.epub_edit_copy(&ctx);
        }
        self.epub_sync_notes();
    }

    /// Sparar bokmärken och understrykningar i biblioteket när de ändrats.
    pub(crate) fn epub_sync_notes(&mut self) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let (Some(tab), Some(path)) = (doc.epub.as_deref_mut(), doc.path.as_ref()) else { return };
        if tab.take_dirty() {
            let path = path.to_string_lossy().into_owned();
            self.library.set_notes(&tab.key, &path, tab.notes.clone());
            self.library.save();
        }
    }

    /// *Exportera anteckningar…*: bokens bokmärken och understrykningar som ett nytt, osparat Markdown-dokument.
    pub(crate) fn epub_export_notes(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.docs.get(self.active).and_then(|d| d.epub.as_deref()) else { return };
        let md = tab.export_markdown();
        let has_notes = !tab.notes.is_empty();
        self.new_tab();
        let d = &mut self.docs[self.active];
        d.text = md;
        d.on_edited();
        if !has_notes {
            self.notify(ctx, t("Boken har inga bokmärken eller understrykningar än."), false);
        }
    }

    /// Innehållsförteckningen i vänstra panelen (samma växling som navigeringsfönstret för dokument).
    pub(crate) fn epub_sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        if !self.settings.nav_pane {
            return;
        }
        let Some(tab) = self.epub_tab_mut() else { return };
        if let Some(d) = view::sidebar(ui, p, tab) {
            tab.follow(d);
        }
        let export = std::mem::take(&mut tab.export_requested);
        self.epub_sync_notes();
        if export {
            let ctx = ui.ctx().clone();
            self.epub_export_notes(&ctx);
        }
    }

    /// Sökfältet, fotnotsrutan och Om boken.
    pub(crate) fn epub_overlays(&mut self, ctx: &egui::Context, p: &Palette) {
        let anchor = self.editor_rect;
        let prefs = self.settings.epub.clone();
        let Some(tab) = self.epub_tab_mut() else { return };
        tab.overlays(ctx, p, &prefs, anchor);
        self.epub_sync_notes();
    }

    pub(crate) fn epub_shortcuts(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.epub.as_deref_mut()) else { return };
        tab.handle_keys(ctx, &mut self.settings.epub);
        self.epub_sync_notes();
    }

    pub(crate) fn epub_open_find(&mut self) {
        if let Some(tab) = self.epub_tab_mut() {
            tab.open_find();
        }
    }

    /// Sparar läget (del och rullning) för boken i flik `idx`.
    pub(crate) fn store_epub_memory(&mut self, idx: usize) {
        let Some(doc) = self.docs.get(idx) else { return };
        let (Some(tab), Some(path)) = (doc.epub.as_ref(), doc.path.as_ref()) else { return };
        let path = path.to_string_lossy().into_owned();
        self.library.set_progress(&tab.key, &path, tab.progress());
        self.library.save();
        view::remember(&mut self.settings.epub_memory, path, tab.memory(), MAX_MEMORY);
    }

    /// Sätter en nyöppnad bok på senast lästa ställe.
    pub(crate) fn epub_restore(&mut self, doc: &mut crate::doc::Document) {
        let (Some(tab), Some(path)) = (doc.epub.as_deref_mut(), doc.path.as_ref()) else { return };
        let key = path.to_string_lossy();
        if let Some((_, m)) = self.settings.epub_memory.iter().find(|(k, _)| *k == key) {
            tab.restore(m);
        }
        // Biblioteket: boken blir senast läst, bokmärkena läses och omslagsminiatyren sparas för startsidan.
        let book_key = store::book_key(&tab.book.meta.identifier, path);
        let path_s = path.to_string_lossy().into_owned();
        self.library.opened(&book_key, &path_s, &tab.book.title(), &tab.book.meta.authors.join(", "));
        tab.set_notes(book_key.clone(), self.library.notes_for(&book_key, &path_s));
        let cover = store::save_cover(&book_key, || tab.book.cover_image());
        if let Some(i) = self.library.index_of(&book_key, &path_s) {
            self.library.books[i].cover = cover.unwrap_or_default();
            self.library.books[i].progress = if self.library.books[i].progress > 0.0 { self.library.books[i].progress } else { tab.progress() };
        }
        self.library.save();
    }

    /// Böckerna som omslag på startsidan: titel, författare och framstegsstapel. Klick öppnar boken där man var.
    pub(crate) fn epub_shelf(&mut self, ui: &mut Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        let books: Vec<store::BookRecord> = self.library.shelf().into_iter().take(SHELF_MAX).cloned().collect();
        if books.is_empty() {
            return;
        }
        ui.label(RichText::new(t("Böcker")).font(FontId::new(12.5, theme::medium())).color(p.muted));
        ui.add_space(8.0);
        let mut open: Option<std::path::PathBuf> = None;
        let mut hide: Option<String> = None;
        let mut reveal: Option<std::path::PathBuf> = None;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for b in &books {
                let path = std::path::PathBuf::from(&b.path);
                let exists = path.exists();
                let (rect, resp) = ui.allocate_exact_size(vec2(CARD_W, 214.0), Sense::click());
                let card = rect.shrink2(vec2(6.0, 0.0));
                let painter = ui.painter();
                if resp.hovered() {
                    painter.rect_filled(rect, 10.0, p.surface);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                // Omslaget (miniatyr ur datakatalogen), centrerat i en ruta med fast höjd.
                let cover_box = Rect::from_min_size(pos2(card.left() + 6.0, card.top() + 8.0), vec2(card.width() - 12.0, 132.0));
                let file = store::cover_dir().join(&b.cover);
                if !b.cover.is_empty() && file.exists() {
                    let img = egui::Image::new(format!("file://{}", file.display()))
                        .max_size(cover_box.size())
                        .corner_radius(3)
                        .tint(if exists { Color32::WHITE } else { Color32::from_white_alpha(70) });
                    ui.put(cover_box, img);
                } else {
                    painter.rect_filled(cover_box.shrink2(vec2(10.0, 0.0)), 3.0, p.current_line);
                    let initial = b.title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                    painter.text(cover_box.center(), Align2::CENTER_CENTER, initial, FontId::new(34.0, theme::semibold()), p.faint);
                }
                // Titel (två rader), författare, framsteg.
                let tw = card.width() - 12.0;
                let title_color = if exists { p.text } else { p.faint };
                let mut job = egui::text::LayoutJob::simple(b.title.clone(), FontId::new(12.5, theme::semibold()), title_color, tw);
                job.wrap.max_rows = 2;
                let g = ui.painter().layout_job(job);
                let gh = g.size().y;
                ui.painter().galley(pos2(card.left() + 6.0, card.top() + 146.0), g, title_color);
                let line2 = if exists { b.author.clone() } else { t("Filen hittades inte").to_owned() };
                let line2_color = if exists { p.muted } else { p.error };
                let mut job = egui::text::LayoutJob::simple(line2, FontId::new(11.5, egui::FontFamily::Proportional), line2_color, tw);
                job.wrap.max_rows = 1;
                let g2 = ui.painter().layout_job(job);
                ui.painter().galley(pos2(card.left() + 6.0, card.top() + 146.0 + gh + 1.0), g2, line2_color);
                let bar = Rect::from_min_size(pos2(card.left() + 6.0, card.bottom() - 18.0), vec2(tw - 32.0, 3.0));
                ui.painter().rect_filled(bar, 1.5, p.line);
                ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * b.progress.clamp(0.0, 1.0), 3.0)), 1.5, if exists { p.accent } else { p.faint });
                ui.painter().text(
                    pos2(bar.right() + 6.0, bar.center().y),
                    Align2::LEFT_CENTER,
                    format!("{} %", (b.progress.clamp(0.0, 1.0) * 100.0).round() as i32),
                    FontId::new(11.0, egui::FontFamily::Proportional),
                    p.muted,
                );
                let tip = if exists { tf!("{title} – {pct} %", title = b.title, pct = (b.progress * 100.0).round() as i32) } else { t("Filen hittades inte").to_owned() };
                let label = if exists { b.title.clone() } else { format!("{}, {}", b.title, t("Filen hittades inte")) };
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone()));
                let resp = resp.on_hover_text(tip);
                if resp.clicked() {
                    open = Some(path.clone());
                }
                resp.context_menu(|ui| {
                    if ui.button(t("Ta bort ur listan")).clicked() {
                        hide = Some(b.path.clone());
                        ui.close();
                    }
                    if exists && ui.button(t("Visa i filhanteraren")).clicked() {
                        reveal = path.parent().map(std::path::Path::to_path_buf);
                        ui.close();
                    }
                });
            }
        });
        if let Some(path) = hide {
            self.library.hide(&path);
            self.library.save();
        }
        if let Some(dir) = reveal {
            if let Err(e) = crate::platform::open(&dir) {
                self.notify(&ctx, tf!("Kunde inte öppna filhanteraren: {e}", e = e), true);
            }
        }
        if let Some(path) = open {
            self.open_path(&ctx, &path);
        }
    }

    /// Redigera en kopia: bokens text som ett nytt, osparat dokument (boken ändras aldrig).
    fn epub_edit_copy(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.docs.get(self.active).and_then(|d| d.epub.as_deref()) else { return };
        let (md, images) = tab.book.to_markdown();
        self.new_tab();
        let d = &mut self.docs[self.active];
        d.text = md;
        d.docx = true;
        d.word_view = true;
        d.images = images;
        d.on_edited();
        self.notify(ctx, t("Kopian av boken är ett nytt dokument. Boken själv ändras inte."), false);
    }
}
