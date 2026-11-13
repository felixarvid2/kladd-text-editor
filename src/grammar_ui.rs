//! Gränssnittet för grammatikkontrollen: inställningsdialogen med integritetsförklaringen och
//! kopplingen mellan inställningarna och `grammar::Checker`.

use crate::app::KladdApp;
use crate::i18n::t;
use crate::grammar::{self, Server};

/// Öppen dialog "Grammatik och stil" (redigerar en kopia tills man klickar OK).
pub struct Dialog {
    pub enabled: bool,
    pub public: bool,
    pub url: String,
}

impl KladdApp {
    /// Menyvalet "Grammatik": slår av direkt, men slår på först efter förklaringen när den
    /// publika servern används och inte godkänts än.
    pub(crate) fn grammar_toggle(&mut self) {
        if self.settings.grammar {
            self.settings.grammar = false;
            self.grammar.idle();
        } else if Server::from_setting(&self.settings.grammar_server).is_public() && !self.settings.grammar_ack {
            self.open_grammar_dialog(true);
        } else {
            self.settings.grammar = true;
        }
    }

    pub(crate) fn open_grammar_dialog(&mut self, enable: bool) {
        let server = Server::from_setting(&self.settings.grammar_server);
        self.grammar_dlg = Some(Dialog {
            enabled: enable || self.settings.grammar,
            public: server.is_public(),
            url: match server {
                Server::Custom(u) => u,
                Server::Public => "http://localhost:8081".to_owned(),
            },
        });
    }

    /// Inställningar som `Checker::update` ska använda, `None` när kontrollen är av.
    pub(crate) fn grammar_config(&self) -> Option<grammar::Config> {
        if !self.settings.grammar {
            return None;
        }
        let server = Server::from_setting(&self.settings.grammar_server);
        let lang = grammar::language_for(self.speller.as_ref().and_then(|s| s.lang()).map(|l| l.code.as_str()));
        Some(grammar::Config { server, lang })
    }

    pub(crate) fn grammar_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.grammar_dlg.take() else { return };
        let mut done: Option<bool> = None;
        let mut open = true;
        egui::Window::new(t("Grammatik och stil"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.checkbox(&mut dlg.enabled, t("Kontrollera grammatik och stil med LanguageTool"));
                ui.add_space(6.0);
                ui.label(t("Server:"));
                ui.radio_value(&mut dlg.public, true, t(grammar::PUBLIC_LABEL));
                ui.radio_value(&mut dlg.public, false, t("Egen server (till exempel en lokal LanguageTool)"));
                ui.add_enabled_ui(!dlg.public, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut dlg.url)
                            .hint_text("http://localhost:8081")
                            .desired_width(f32::INFINITY),
                    );
                });
                ui.add_space(6.0);
                if dlg.public {
                    ui.label(egui::RichText::new(t(grammar::PRIVACY_NOTE)).strong());
                } else {
                    ui.label(t("Texten skickas till adressen du anger. Bara synlig text skickas, utan Markdown-syntax."));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ok = if dlg.enabled && dlg.public && !self.settings.grammar_ack {
                        t("Godkänn och spara")
                    } else {
                        "OK"
                    };
                    if ui.button(ok).clicked() {
                        done = Some(true);
                    }
                    if ui.button(t("Avbryt")).clicked() {
                        done = Some(false);
                    }
                });
            });
        if !open && done.is_none() {
            done = Some(false);
        }
        match done {
            None => self.grammar_dlg = Some(dlg),
            Some(false) => {}
            Some(true) => {
                if dlg.public {
                    self.settings.grammar_server = grammar::PUBLIC_SETTING.to_owned();
                    if dlg.enabled {
                        self.settings.grammar_ack = true;
                    }
                } else if !dlg.url.trim().is_empty() {
                    self.settings.grammar_server = dlg.url.trim().to_owned();
                }
                self.settings.grammar = dlg.enabled;
                if !dlg.enabled {
                    self.grammar.idle();
                }
            }
        }
    }
}
