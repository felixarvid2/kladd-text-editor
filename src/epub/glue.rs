//! EPUB-läsläget i appen: flikens sida, sidopanel, överlägg, tangenter och lägesminne. Filen är en
//! undermodul till `app` (se `#[path]` i `app.rs`) för att nå appens privata fält och funktioner,
//! på samma sätt som `commands.rs`.

use egui::{CentralPanel, Frame, Ui};

use super::KladdApp;
use crate::epub::view::{self, Action, MAX_MEMORY};
use crate::i18n::t;
use crate::theme::Palette;

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
    }

    /// Sökfältet, fotnotsrutan och Om boken.
    pub(crate) fn epub_overlays(&mut self, ctx: &egui::Context, p: &Palette) {
        let anchor = self.editor_rect;
        let prefs = self.settings.epub.clone();
        let Some(tab) = self.epub_tab_mut() else { return };
        tab.overlays(ctx, p, &prefs, anchor);
    }

    pub(crate) fn epub_shortcuts(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.epub.as_deref_mut()) else { return };
        tab.handle_keys(ctx, &mut self.settings.epub);
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
        view::remember(&mut self.settings.epub_memory, path.to_string_lossy().into_owned(), tab.memory(), MAX_MEMORY);
    }

    /// Sätter en nyöppnad bok på senast lästa ställe.
    pub(crate) fn epub_restore(&mut self, doc: &mut crate::doc::Document) {
        let (Some(tab), Some(path)) = (doc.epub.as_deref_mut(), doc.path.as_ref()) else { return };
        let key = path.to_string_lossy();
        if let Some((_, m)) = self.settings.epub_memory.iter().find(|(k, _)| *k == key) {
            tab.restore(m);
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
