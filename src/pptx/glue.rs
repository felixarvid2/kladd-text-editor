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
        let pref = self.settings.pptx.audience_screen;
        let dir = self.docs.get(self.active).and_then(|d| d.path.as_ref()).and_then(|p| p.parent()).map(std::path::Path::to_path_buf);
        let Some(tab) = self.pptx_tab_mut() else { return };
        tab.media_dir = dir;
        tab.audience_pref = pref;
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
            if want && !tab.fullscreen_sent {
                // Bildspelet startar: skärmen Kladd-fönstret ligger på blir föredragsvyns.
                let outer = ctx.input(|i| i.viewport().outer_rect);
                tab.restore_pos = outer.map(|r| r.min);
                if let (Some(r), Some(show)) = (outer, tab.show_mut()) {
                    let ppp = crate::pptx::show_screens::units_per_point(ctx.pixels_per_point());
                    show.set_home_at(r.center() * ppp);
                }
            }
            // Med flera skärmar går publikens helskärm till sin skärm (`show_screens.rs`), annars aktuell.
            let monitor = if want { tab.show_mut().and_then(|s| s.audience_screen()) } else { None };
            if want != tab.fullscreen_sent || (want && monitor != tab.monitor_sent) {
                tab.fullscreen_sent = want;
                tab.monitor_sent = monitor;
                match (want, monitor) {
                    (true, Some(i)) => ctx.send_viewport_cmd(ViewportCommand::SetMonitor(i)),
                    (w, _) => ctx.send_viewport_cmd(ViewportCommand::Fullscreen(w)),
                }
                if !want {
                    if let Some(p) = tab.restore_pos.take() {
                        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(p));
                    }
                }
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

// ---------------------------------------------------------------- ny presentation

impl KladdApp {
    /// *Arkiv → Ny presentation…*: öppnar dialogen med färgvarianter.
    pub(crate) fn pptx_open_new_dialog(&mut self) {
        self.new_pres = Some(crate::pptx::new_deck::Variant::Kladd);
    }

    /// Lägger en ny, namnlös presentation i en ny flik (ingen sökväg förrän man sparar).
    fn pptx_install_new(&mut self, ctx: &egui::Context, bytes: Vec<u8>) {
        match crate::pptx::Deck::open(&bytes) {
            Ok(deck) => {
                self.untitled_counter += 1;
                let tab = PptxTab::with_source(deck, bytes);
                let doc = crate::doc::Document::pptx_untitled(self.next_id, self.untitled_counter, tab);
                self.next_id += 1;
                self.docs.push(doc);
                self.active = self.docs.len() - 1;
                // Börja i redigeringsläget: en ny presentation är till för att fyllas.
                if let Some(tab) = self.pptx_tab_mut() {
                    tab.toggle_edit();
                }
            }
            Err(e) => self.notify(ctx, e, true),
        }
    }

    /// Dialogen *Ny presentation*: välj färgvariant.
    pub(crate) fn pptx_new_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        use crate::pptx::new_deck::Variant;
        let Some(mut sel) = self.new_pres else { return };
        let mut create = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("kladd-pptx-new"))
            .backdrop_color(egui::Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
            .frame(Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0, p.line)).corner_radius(14).inner_margin(egui::Margin::same(24)).shadow(ctx.global_style().visuals.window_shadow))
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(egui::RichText::new(t("Ny presentation")).font(egui::FontId::new(17.0, crate::theme::semibold())).color(p.text));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(t("Välj färgtema. Bilden får Kladds egna bildmall med sju layouter.")).color(p.muted));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    for v in Variant::ALL {
                        let name = match v {
                            Variant::Kladd => t("Kladd"),
                            Variant::Skog => t("Skog"),
                            Variant::Skymning => t("Skymning"),
                        };
                        let (r, resp) = ui.allocate_exact_size(egui::vec2(126.0, 84.0), egui::Sense::click());
                        let on = sel == v;
                        ui.painter().rect_filled(r, 8.0, p.chrome);
                        ui.painter().rect_stroke(r, 8.0, egui::Stroke::new(if on { 2.0 } else { 1.0 }, if on { p.accent } else { p.line }), egui::StrokeKind::Inside);
                        for (i, c) in v.swatches().iter().enumerate() {
                            let sw = egui::Rect::from_min_size(r.min + egui::vec2(12.0 + i as f32 * 36.0, 14.0), egui::vec2(30.0, 30.0));
                            ui.painter().rect_filled(sw, 5.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
                        }
                        ui.painter().text(r.min + egui::vec2(12.0, 62.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(13.5), p.text);
                        if resp.clicked() {
                            sel = v;
                        }
                    }
                });
                ui.add_space(20.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let ok = egui::Button::new(egui::RichText::new(t("Skapa")).font(egui::FontId::new(13.5, crate::theme::medium())).color(p.on_accent)).fill(p.accent).stroke(egui::Stroke::NONE).corner_radius(8).min_size(egui::vec2(88.0, 30.0));
                    if ui.add(ok).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        create = true;
                    }
                    if ui.add(crate::app::quiet_button(t("Avbryt"), p)).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                });
            });
        if create {
            self.new_pres = None;
            self.pptx_install_new(ctx, crate::pptx::new_deck::build(sel));
        } else if cancel {
            self.new_pres = None;
        } else {
            self.new_pres = Some(sel);
        }
    }

    /// *Ny presentation från mall…*: välj en `.potx` (eller en annan presentation).
    pub(crate) fn pptx_request_new_from_file(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            self.notify(ctx, t("Stäng den öppna fildialogen först."), true);
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.dialog = Some(rx);
        let ctx = ctx.clone();
        let dir = self.start_dir();
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new().set_title(t("Ny presentation från mall")).add_filter(t("PowerPoint-mallar och presentationer"), &["potx", "potm", "pptx", "pptm", "ppsx"]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let _ = tx.send(super::DialogResult::NewFromPptx(dlg.pick_file()));
            ctx.request_repaint();
        });
    }

    /// Öppnar filen som en ny, namnlös presentation: ingen sökväg, innehållstypen blir en presentation
    /// när den sparas.
    pub(crate) fn pptx_new_from_file(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(bytes) => self.pptx_install_new(ctx, bytes),
            Err(e) => self.notify(ctx, tf!("Kunde inte läsa {name}: {e}", name = path.display(), e = e), true),
        }
    }
}
