//! Presentationsfliken i appen: verktygsrad, sidopanel, sökfält, tangenter, bildspel i helskärm,
//! PDF-export, lägesminne och *Redigera en kopia*. Undermodul till `app` (se `#[path]` i `app.rs`),
//! som `epub/glue.rs`.

use egui::{CentralPanel, Frame, Ui, ViewportCommand};

use super::KladdApp;
use crate::i18n::t;
use crate::pptx::view::{self, Action, PptxTab, MAX_MEMORY};
use crate::tf;
use crate::theme::Palette;

impl KladdApp {
    /// Är den aktiva fliken en presentation?
    pub(crate) fn pptx_mode(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| d.pptx.is_some())
    }

    fn pptx_tab_mut(&mut self) -> Option<&mut PptxTab> {
        self.docs.get_mut(self.active).and_then(|d| d.pptx.as_deref_mut())
    }

    /// Pågår ett bildspel i den aktiva fliken?
    pub(crate) fn pptx_presenting(&self) -> bool {
        self.docs.get(self.active).and_then(|d| d.pptx.as_deref()).is_some_and(PptxTab::presenting)
    }

    /// Bilderna i mitten.
    pub(crate) fn pptx_page(&mut self, ui: &mut Ui, p: &Palette) {
        let prefs = &mut self.settings.pptx;
        let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.pptx.as_deref_mut()) else { return };
        let mut editor_rect = self.editor_rect;
        CentralPanel::default().frame(Frame::NONE.fill(p.desk)).show(ui, |ui| {
            editor_rect = ui.max_rect();
            tab.show(ui, p, prefs);
        });
        self.editor_rect = editor_rect;
    }

    /// Bildspelet: bara bilden, i helskärm.
    pub(crate) fn pptx_show_page(&mut self, ui: &mut Ui) {
        CentralPanel::default().frame(Frame::NONE.fill(egui::Color32::BLACK)).show(ui, |ui| {
            if let Some(tab) = self.pptx_tab_mut() {
                tab.show_page(ui);
            }
        });
    }

    /// Verktygsraden under flikraden.
    pub(crate) fn pptx_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let prefs = &mut self.settings.pptx;
        let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.pptx.as_deref_mut()) else { return };
        if let Some(act) = tab.toolbar(ui, p, prefs) {
            self.pptx_action(&ui.ctx().clone(), act);
        }
    }

    /// Miniatyrlisten.
    pub(crate) fn pptx_sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        let prefs = &self.settings.pptx;
        if let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.pptx.as_deref_mut()) {
            tab.sidebar(ui, p, prefs);
        }
    }

    /// Sökfältet och PDF-exportens förlopp.
    pub(crate) fn pptx_overlays(&mut self, ctx: &egui::Context, p: &Palette) {
        let anchor = self.editor_rect;
        let prefs = &mut self.settings.pptx;
        let mut done = None;
        if let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.pptx.as_deref_mut()) {
            tab.find_bar(ctx, p, prefs, anchor);
        }
        // Redigeringen: bilddialogen, notiser och ändrat-läget (fråga vid stängning, • i fliken).
        let mut notices = Vec::new();
        for d in &mut self.docs {
            if let Some(tab) = d.pptx.as_deref_mut() {
                notices.extend(tab.poll_edit());
                d.dirty = tab.unsaved();
            }
        }
        for (text, err) in notices {
            self.notify(ctx, text, err);
        }
        // Exporten fortsätter även om man byter flik.
        for d in &mut self.docs {
            if let Some(tab) = d.pptx.as_deref_mut()
                && let Some(r) = tab.poll_export(ctx)
            {
                done = Some(r);
            }
        }
        match done {
            Some(Ok(path)) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify(ctx, tf!("Exporterade {name}", name = name), false);
            }
            Some(Err(e)) => self.notify(ctx, tf!("Kunde inte exportera PDF: {e}", e = e), true),
            None => {}
        }
    }

    /// Kortkommandon i presentationsvyn.
    pub(crate) fn pptx_shortcuts(&mut self, ctx: &egui::Context) {
        let prefs = &mut self.settings.pptx;
        let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.pptx.as_deref_mut()) else { return };
        if let Some(act) = tab.handle_keys(ctx, prefs) {
            self.pptx_action(ctx, act);
        }
    }

    /// Tangenterna i bildspelet.
    pub(crate) fn pptx_show_keys(&mut self, ctx: &egui::Context) {
        if let Some(tab) = self.pptx_tab_mut() {
            tab.show_keys(ctx);
        }
    }

    pub(crate) fn pptx_open_find(&mut self) {
        if let Some(tab) = self.pptx_tab_mut() {
            tab.open_find();
        }
    }

    /// Zoom från menyn och paletten (`dir` 0 = hel bild).
    pub(crate) fn pptx_zoom(&mut self, dir: f32) {
        let prefs = &mut self.settings.pptx;
        if let Some(tab) = self.docs.get(self.active).and_then(|d| d.pptx.as_deref()) {
            tab.zoom_cmd(prefs, dir);
        }
    }

    pub(crate) fn pptx_action(&mut self, ctx: &egui::Context, act: Action) {
        match act {
            Action::EditCopy => self.pptx_edit_copy(ctx),
            Action::ExportPdf => self.pptx_export_pdf(ctx),
            Action::Print => self.open_print(ctx),
            Action::Show(from_current) => self.pptx_start_show(ctx, from_current),
            Action::InsertPicture => self.pptx_insert_picture(ctx),
        }
    }

    /// Infoga bild (redigeringsläget slås på).
    pub(crate) fn pptx_insert_picture(&mut self, ctx: &egui::Context) {
        let dir = self.docs.get(self.active).and_then(|d| d.path.as_ref()).and_then(|p| p.parent().map(std::path::Path::to_path_buf)).or_else(|| self.start_dir());
        if let Some(tab) = self.pptx_tab_mut() {
            tab.begin_insert_picture(ctx, dir);
        }
    }

    /// *Spara som* för en presentation: .pptx, .ppsx (bildspel) eller .potx (mall), och varianterna
    /// med makron om filen har makron.
    pub(super) fn request_save_as_pptx(&mut self, ctx: &egui::Context, idx: usize, then_close: bool, exit: bool) {
        let Some(doc) = self.docs.get(idx) else { return };
        let ext = doc.extension();
        let ext = if crate::pptx::is_ext(&ext) { ext } else { "pptx".to_owned() };
        let stem = doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| doc.title());
        let name = format!("{stem}.{ext}");
        let dir = doc.path.as_ref().and_then(|p| p.parent().map(std::path::Path::to_path_buf)).or_else(|| self.start_dir());
        let doc_id = doc.id;
        let macros = ext.ends_with('m');
        let (tx, rx) = std::sync::mpsc::channel();
        self.dialog = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new()
                .set_title(t("Spara som"))
                .set_file_name(name)
                .add_filter(t("PowerPoint-presentation (.pptx)"), &["pptx"])
                .add_filter(t("PowerPoint-bildspel (.ppsx)"), &["ppsx"])
                .add_filter(t("PowerPoint-mall (.potx)"), &["potx"]);
            if macros {
                dlg = dlg.add_filter(t("Med makron (.pptm, .ppsm, .potm)"), &["pptm", "ppsm", "potm"]);
            }
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let path = dlg.save_file();
            let _ = tx.send(super::DialogResult::SaveAs { doc_id, path, then_close, exit });
            ctx.request_repaint();
        });
    }

    /// Slår på/av redigeringsläget (kommandot och menyn).
    pub(crate) fn pptx_toggle_edit(&mut self) {
        if let Some(tab) = self.pptx_tab_mut() {
            tab.toggle_edit();
        }
    }

    pub(crate) fn pptx_start_show(&mut self, ctx: &egui::Context, from_current: bool) {
        let animate = self.settings.pptx.animate;
        let Some(tab) = self.pptx_tab_mut() else { return };
        if !tab.start_show(from_current, animate) {
            self.notify(ctx, t("Presentationen har inga bilder att visa (alla är dolda)."), false);
        }
    }

    /// Helskärm följer bildspelet. Ett bildspel i en flik som inte längre är aktiv (en fil som
    /// öppnades under tiden) avslutas, så att fönstret aldrig fastnar i helskärm.
    pub(crate) fn pptx_sync_fullscreen(&mut self, ctx: &egui::Context) {
        let active = self.active;
        for (i, d) in self.docs.iter_mut().enumerate() {
            let Some(tab) = d.pptx.as_deref_mut() else { continue };
            if i != active && tab.presenting() {
                tab.stop_show();
            }
            let want = tab.presenting();
            if want != tab.fullscreen_sent {
                tab.fullscreen_sent = want;
                ctx.send_viewport_cmd(ViewportCommand::Fullscreen(want));
            }
        }
    }

    /// *Exportera som PDF…*: alla synliga bilder, med riktig text.
    pub(crate) fn pptx_export_pdf(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            self.notify(ctx, t("Stäng den öppna fildialogen först."), true);
            return;
        }
        let dir = self.docs.get(self.active).and_then(|d| d.path.as_ref()).and_then(|p| p.parent().map(std::path::Path::to_path_buf)).or_else(|| self.start_dir());
        let Some(doc) = self.docs.get(self.active) else { return };
        let stem = doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "presentation".to_owned());
        let Some(tab) = doc.pptx.as_deref() else { return };
        if tab.exporting() {
            return;
        }
        if tab.visible_slides().is_empty() {
            self.notify(ctx, t("Presentationen har inga synliga bilder."), false);
            return;
        }
        let Some(tab) = self.pptx_tab_mut() else { return };
        tab.begin_export(ctx, format!("{stem}.pdf"), dir);
    }

    /// Sparar aktuell bild för presentationen i flik `idx`.
    pub(crate) fn store_pptx_memory(&mut self, idx: usize) {
        let Some(doc) = self.docs.get(idx) else { return };
        let (Some(tab), Some(path)) = (doc.pptx.as_deref(), doc.path.as_ref()) else { return };
        view::remember(&mut self.settings.pptx_memory, path.to_string_lossy().into_owned(), tab.memory(), MAX_MEMORY);
    }

    /// Ställer en nyöppnad presentation på bilden man senast var på.
    pub(crate) fn pptx_restore(&mut self, doc: &mut crate::doc::Document) {
        let (Some(tab), Some(path)) = (doc.pptx.as_deref_mut(), doc.path.as_ref()) else { return };
        let key = path.to_string_lossy();
        if let Some((_, m)) = self.settings.pptx_memory.iter().find(|(k, _)| *k == key) {
            tab.restore(m);
        }
    }

    /// Redigera en kopia: presentationens text som ett nytt, osparat dokument (presentationen ändras aldrig).
    fn pptx_edit_copy(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.docs.get(self.active).and_then(|d| d.pptx.as_deref()) else { return };
        let (md, images) = crate::pptx::copy_md::deck_to_markdown(&tab.deck);
        self.new_tab();
        let d = &mut self.docs[self.active];
        d.text = md;
        d.docx = true;
        d.word_view = true;
        d.images = images;
        d.on_edited();
        self.notify(ctx, t("Kopian av presentationen är ett nytt dokument. Presentationen själv ändras inte."), false);
    }
}
