//! Fönstret "Inställningar" (Ctrl+,): inställningarna som förut låg i huvudmenyn, i sektioner.
//! Fälten är de i `Settings`; fönstret redigerar dem direkt, så ändringar gäller med en gång.

use egui::{Align, Color32, ComboBox, DragValue, Key, Layout, RichText, ScrollArea, TextEdit, Ui, vec2};

use crate::app::{KladdApp, Settings};
use crate::grammar::{self, Server};
use crate::i18n::{self, LangSetting, t};
use crate::tf;
use crate::theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    General,
    Editor,
    Document,
    Pdf,
    Advanced,
}

impl Section {
    pub const ALL: [Section; 5] = [Section::General, Section::Editor, Section::Document, Section::Pdf, Section::Advanced];

    /// Svenska namnet (nyckel för `t`).
    pub const fn name(self) -> &'static str {
        match self {
            Section::General => "Allmänt",
            Section::Editor => "Redigerare",
            Section::Document => "Dokument",
            Section::Pdf => "PDF",
            Section::Advanced => "Avancerat",
        }
    }
}

/// Öppet fönster.
pub struct Window {
    pub section: Section,
    /// Adressen i fältet "Egen server" (behålls när man växlar till den publika servern och tillbaka).
    custom_url: String,
    confirm_reset: bool,
}

impl KladdApp {
    /// Öppnar fönstret (på `section` om den anges), eller stänger det om det redan är öppet utan ny sektion.
    pub(crate) fn open_settings(&mut self, section: Option<Section>) {
        self.cmd.settings = match (self.cmd.settings.take(), section) {
            (Some(_), None) => None,
            (Some(mut w), Some(s)) => {
                w.section = s;
                Some(w)
            }
            (None, s) => {
                let custom_url = match Server::from_setting(&self.settings.grammar_server) {
                    Server::Custom(u) => u,
                    Server::Public => "http://localhost:8081".to_owned(),
                };
                Some(Window { section: s.unwrap_or(Section::General), custom_url, confirm_reset: false })
            }
        };
    }

    pub(crate) fn settings_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut win) = self.cmd.settings.take() else { return };
        let mut open = true;
        let screen = ctx.content_rect();
        let narrow = screen.width() < 700.0;
        let size = vec2((screen.width() - 32.0).min(700.0), (screen.height() - 90.0).clamp(240.0, 540.0));
        egui::Window::new(t("Inställningar"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size(size)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 10.0))
            .show(ctx, |ui| {
                if narrow {
                    ui.horizontal_wrapped(|ui| {
                        for s in Section::ALL {
                            ui.selectable_value(&mut win.section, s, t(s.name()));
                        }
                    });
                    ui.separator();
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.settings_page(ui, ctx, p, &mut win));
                } else {
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(vec2(130.0, size.y - 8.0), Layout::top_down(Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for s in Section::ALL {
                                let on = win.section == s;
                                let text = RichText::new(t(s.name())).size(14.0).color(if on { p.text } else { p.muted });
                                if ui.add_sized(vec2(124.0, 28.0), egui::Button::selectable(on, text)).clicked() {
                                    win.section = s;
                                }
                            }
                        });
                        ui.separator();
                        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            // Rullytan ärver radens riktning: lägg ut sidan uppifrån och ned.
                            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                                ui.set_min_width(size.x - 190.0);
                                self.settings_page(ui, ctx, p, &mut win)
                            });
                        });
                    });
                }
            });
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            open = false;
        }
        if open {
            self.cmd.settings = Some(win);
        } else {
            self.focus_editor = true;
        }
    }

    fn settings_page(&mut self, ui: &mut Ui, ctx: &egui::Context, p: &Palette, win: &mut Window) {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.add_space(2.0);
        match win.section {
            Section::General => self.page_general(ui, ctx),
            Section::Editor => self.page_editor(ui),
            Section::Document => self.page_document(ui, p, win),
            Section::Pdf => self.page_pdf(ui, p),
            Section::Advanced => self.page_advanced(ui, ctx, p, win),
        }
    }

    fn page_general(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        heading(ui, t("Språk"));
        let cur = self.settings.lang;
        let mut lang = cur;
        ComboBox::from_id_salt("kladd-set-lang").selected_text(cur.label()).width(180.0).show_ui(ui, |ui| {
            for l in LangSetting::ALL {
                ui.selectable_value(&mut lang, l, l.label());
            }
        });
        if lang != cur {
            self.settings.lang = lang;
            // Gäller från nästa bild utan omstart.
            i18n::set_lang(lang.resolve());
        }
        heading(ui, t("Tema"));
        let mut dark = self.settings.dark;
        ui.horizontal(|ui| {
            ui.radio_value(&mut dark, true, t("Mörkt"));
            ui.radio_value(&mut dark, false, t("Ljust"));
        });
        if dark != self.settings.dark {
            self.toggle_theme(ctx);
        }
        heading(ui, t("Textstorlek"));
        ui.horizontal(|ui| {
            if ui.button("\u{2212}").clicked() {
                self.zoom(-1.0);
            }
            ui.add(DragValue::new(&mut self.settings.font_size).range(9.0..=32.0).speed(0.2).fixed_decimals(0).suffix(" pt"));
            if ui.button("+").clicked() {
                self.zoom(1.0);
            }
            if ui.button(t("Normal")).clicked() {
                self.settings.font_size = crate::app::DEFAULT_FONT_SIZE;
            }
        });
        heading(ui, t("Uppdateringar"));
        ui.checkbox(&mut self.settings.auto_update_check, t("Sök efter uppdateringar automatiskt"));
        if ui.button(t("Sök efter uppdateringar nu")).clicked() {
            self.update.check_now(ctx);
        }
        heading(ui, t("Spara"));
        ui.checkbox(&mut self.settings.autosave, t("Spara automatiskt"))
            .on_hover_text(t("Sparar ändrade flikar som har en sökväg när skrivandet stannat"));
        ui.checkbox(&mut self.settings.autosave_recovery, t("Säkerhetskopiera osparade ändringar"))
            .on_hover_text(t("Osparade flikar kan återställas efter en krasch"));
    }

    fn page_editor(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        ui.checkbox(&mut s.wrap, t("Radbrytning"));
        ui.checkbox(&mut s.line_numbers, t("Radnummer"));
        ui.checkbox(&mut s.preview, t("Förhandsvisa Markdown"));
        ui.checkbox(&mut s.nav_pane, t("Navigeringsfönster"))
            .on_hover_text(t("Rubrikträd för Markdown-baserade flikar"));
        ui.checkbox(&mut s.workspace_panel, t("Mappträd"));
    }

    fn page_document(&mut self, ui: &mut Ui, p: &Palette, win: &mut Window) {
        heading(ui, t("Författare"));
        ui.label(RichText::new(t("Namnet som skrivs på dina ändringar och kommentarer.")).color(p.muted));
        let hint = self.review_author();
        ui.add(TextEdit::singleline(&mut self.settings.author_name).hint_text(hint).desired_width(260.0));
        ui.checkbox(&mut self.settings.word_comments, t("Visa kommentarer"));

        heading(ui, t("Stavning"));
        ui.checkbox(&mut self.settings.spell, t("Stavningskontroll"));
        if self.speller.is_none() {
            self.speller = Some(crate::spell::Speller::new(self.settings.spell_lang.as_deref()));
        }
        if let Some(sp) = self.speller.as_mut() {
            let langs: Vec<(String, String)> = sp.languages().iter().map(|l| (l.code.clone(), l.name.clone())).collect();
            let current = sp.lang().map(|l| l.code.clone());
            if langs.is_empty() {
                ui.label(RichText::new(t("Ingen ordlista för stavningskontroll är installerad – klicka för att hämta en")).color(p.muted));
            } else {
                let shown = sp.lang().map_or_else(|| t("Ingen ordlista").to_owned(), |l| l.name.clone());
                let mut pick = None;
                ComboBox::from_id_salt("kladd-set-spell").selected_text(shown).width(220.0).show_ui(ui, |ui| {
                    for (code, name) in &langs {
                        if ui.selectable_label(current.as_deref() == Some(code.as_str()), name).clicked() {
                            pick = Some(code.clone());
                        }
                    }
                });
                if let Some(code) = pick {
                    sp.set_lang(&code);
                    self.settings.spell_lang = Some(code);
                }
            }
            ui.label(RichText::new(t(crate::spell::LICENSE_NOTE)).size(12.0).color(p.muted));
        }

        heading(ui, t("Grammatik och stil"));
        let mut on = self.settings.grammar;
        if ui.checkbox(&mut on, t("Kontrollera grammatik och stil med LanguageTool")).changed() {
            // Slår på först efter förklaringen när den publika servern inte godkänts än.
            self.grammar_toggle();
        }
        let public = Server::from_setting(&self.settings.grammar_server).is_public();
        let mut want_public = public;
        ui.label(t("Server:"));
        ui.radio_value(&mut want_public, true, t(grammar::PUBLIC_LABEL));
        ui.radio_value(&mut want_public, false, t("Egen server (till exempel en lokal LanguageTool)"));
        if want_public != public {
            if want_public {
                self.settings.grammar_server = grammar::PUBLIC_SETTING.to_owned();
            } else if !win.custom_url.trim().is_empty() {
                self.settings.grammar_server = win.custom_url.trim().to_owned();
            }
        }
        if !want_public {
            let r = ui.add(TextEdit::singleline(&mut win.custom_url).hint_text("http://localhost:8081").desired_width(300.0));
            if r.changed() && !win.custom_url.trim().is_empty() {
                self.settings.grammar_server = win.custom_url.trim().to_owned();
            }
            ui.label(RichText::new(t("Texten skickas till adressen du anger. Bara synlig text skickas, utan Markdown-syntax.")).size(12.0).color(p.muted));
        } else {
            ui.label(RichText::new(t(grammar::PRIVACY_NOTE)).size(12.0).color(p.muted));
        }
    }

    fn page_pdf(&mut self, ui: &mut Ui, p: &Palette) {
        let n = self.settings.pdf_memory.len();
        ui.label(t("Kladd minns sida, zoom och rotation för de senast öppnade PDF-filerna."));
        let text = if n == 1 { t("1 PDF-fil har en sparad vy").to_owned() } else { tf!("{n} PDF-filer har en sparad vy", n = n) };
        ui.label(RichText::new(text).color(p.muted));
        ui.add_enabled_ui(n > 0, |ui| {
            if ui.button(t("Glöm sparade vyer")).clicked() {
                self.settings.pdf_memory.clear();
            }
        });
    }

    fn page_advanced(&mut self, ui: &mut Ui, ctx: &egui::Context, p: &Palette, win: &mut Window) {
        ui.checkbox(&mut self.settings.workspace_hidden, t("Visa dolda filer och mappar i mappträdet"))
            .on_hover_text(t("Som .git, target och node_modules"));
        ui.add_space(6.0);
        let n = self.settings.recent.len();
        ui.add_enabled_ui(n > 0, |ui| {
            if ui.button(t("Rensa listan över senaste filer")).clicked() {
                self.settings.recent.clear();
            }
        });
        ui.add_space(10.0);
        heading(ui, t("Återställ"));
        ui.label(RichText::new(t("Ställer tillbaka alla inställningar till grundvärdena. Öppna filer, senaste filer och arbetsmappen behålls.")).size(12.0).color(p.muted));
        if win.confirm_reset {
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(t("Ja, återställ")).color(Color32::WHITE)).fill(p.error)).clicked() {
                    self.reset_settings(ctx);
                    win.confirm_reset = false;
                }
                if ui.button(t("Avbryt")).clicked() {
                    win.confirm_reset = false;
                }
            });
        } else if ui.button(t("Återställ alla inställningar…")).clicked() {
            win.confirm_reset = true;
        }
    }

    /// Alla inställningar till grundvärdena, utom det som är historik och inte val.
    fn reset_settings(&mut self, ctx: &egui::Context) {
        let old = std::mem::take(&mut self.settings);
        self.settings = Settings {
            recent: old.recent,
            pdf_memory: old.pdf_memory,
            workspace: old.workspace,
            update_last_check: old.update_last_check,
            recent_commands: old.recent_commands,
            ..Settings::default()
        };
        crate::theme::set_dark(ctx, self.settings.dark);
        i18n::set_lang(self.settings.lang.resolve());
        self.notify(ctx, t("Inställningarna är återställda"), false);
    }
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).strong().size(14.5));
}
