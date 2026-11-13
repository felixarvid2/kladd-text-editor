//! Gränssnittet för autokorrigering: dialogen "Autokorrigering…", kopplingen till editorn
//! (upptäcka det som skrevs, göra ersättningen som ett eget ångra-steg) och språkvalet.

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Context, Id};

use crate::app::{KladdApp, byte_to_char, char_to_byte, editor_id};
use crate::autocorrect::{self, Config, Lang};
use crate::i18n::t;

/// Öppen dialog (redigerar en kopia tills man klickar OK).
pub struct Dialog {
    cfg: Config,
    new_from: String,
    new_to: String,
}

/// Tecknet som skrevs den här bilden: ett enda tecken ur en texthändelse, eller radbrytning för Enter.
/// Anropas före `TextEdit::show`; klistrad text och flertecken (IME) räknas inte.
pub(crate) fn typed_char(ctx: &Context) -> Option<char> {
    ctx.input(|i| {
        let mut typed = None;
        for e in &i.events {
            match e {
                egui::Event::Text(s) => {
                    let mut it = s.chars();
                    typed = match (it.next(), it.next()) {
                        (Some(c), None) => Some(c),
                        _ => None,
                    };
                }
                egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. }
                    if !modifiers.command && !modifiers.alt =>
                {
                    typed = Some('\n');
                }
                _ => {}
            }
        }
        typed
    })
}

/// Gör autokorrigeringen efter att `typed` skrivits. Ersättningen blir ett eget ångra-steg, så att
/// Ctrl+Z direkt efteråt bara tar tillbaka den (texten med det du skrev kommer tillbaka).
/// Returnerar om texten ändrades.
pub(crate) fn run_in_editor(ctx: &Context, id: Id, text: &mut String, cfg: &Config, lang: Lang, typed: char) -> bool {
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    let Some(range) = state.cursor.char_range() else { return false };
    // Bara en vanlig markör direkt efter det skrivna tecknet.
    if range.primary != range.secondary {
        return false;
    }
    let caret = char_to_byte(text, range.primary.index.0);
    if !text[..caret].ends_with(typed) {
        return false;
    }
    let Some(fix) = autocorrect::on_typed(text, caret, cfg, lang) else { return false };
    let before = (CCursorRange::one(CCursor::new(range.primary.index.0)), text.clone());
    let mut undoer = state.undoer();
    undoer.add_undo(&before);
    state.set_undoer(undoer);
    let (new_text, new_caret) = fix.apply(text, caret);
    *text = new_text;
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(byte_to_char(text, new_caret)))));
    state.store(ctx, id);
    ctx.request_repaint();
    true
}

impl KladdApp {
    /// Språket reglerna följer: valt i dialogen, annars stavningsspråket, annars gränssnittets.
    pub(crate) fn autocorrect_lang(&self) -> Lang {
        match self.settings.autocorrect.lang.as_deref() {
            Some("sv") => Lang::Sv,
            Some("en") => Lang::En,
            _ => match self.speller.as_ref().and_then(|s| s.lang()) {
                Some(l) => Lang::from_code(&l.code),
                None => match crate::i18n::lang() {
                    crate::i18n::Lang::Sv => Lang::Sv,
                    crate::i18n::Lang::En => Lang::En,
                },
            },
        }
    }

    /// Gäller autokorrigering för den aktiva fliken? Dokumentläget (om på), eller Markdown och vanlig
    /// text när det är valt; aldrig i kodfiler, PDF och bilder.
    pub(crate) fn autocorrect_active(&self, word_mode: bool) -> bool {
        let c = &self.settings.autocorrect;
        let Some(doc) = self.docs.get(self.active) else { return false };
        if word_mode {
            return c.document;
        }
        c.text && doc.pdfv.is_none() && !doc.pdf && !doc.docx && matches!(doc.language().as_str(), "Text" | "Markdown")
    }

    /// Efter en redigering i dokumentläget (där texten ändrats av `typed`).
    pub(crate) fn autocorrect_after_edit(&mut self, ctx: &Context, typed: char) {
        let lang = self.autocorrect_lang();
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let id = editor_id(doc.id);
        if run_in_editor(ctx, id, &mut doc.text, &self.settings.autocorrect, lang, typed) {
            doc.on_edited();
            self.find.current = None;
        }
    }

    pub(crate) fn open_autocorrect_dialog(&mut self) {
        self.autocorrect_dlg = Some(Dialog {
            cfg: self.settings.autocorrect.clone(),
            new_from: String::new(),
            new_to: String::new(),
        });
    }

    pub(crate) fn autocorrect_dialog(&mut self, ctx: &Context) {
        let Some(mut dlg) = self.autocorrect_dlg.take() else { return };
        let mut done: Option<bool> = None;
        let mut open = true;
        egui::Window::new(t("Autokorrigering"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                let c = &mut dlg.cfg;
                ui.label(egui::RichText::new(t("Rätta medan du skriver")).strong());
                ui.checkbox(&mut c.document, t("I dokumentläget"));
                ui.checkbox(&mut c.text, t("I Markdown och vanlig text (aldrig i kodfiler)"));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(t("Regler")).strong());
                ui.checkbox(&mut c.quotes, t("Typografiska citattecken (”…” på svenska, “…” på engelska)"));
                ui.checkbox(&mut c.dashes, t("-- blir – och --- blir —"));
                ui.checkbox(&mut c.ellipsis, t("... blir …"));
                ui.checkbox(&mut c.symbols, t("(c) blir ©, (r) blir ® och (tm) blir ™"));
                ui.checkbox(&mut c.capitalize, t("Stor bokstav i början av meningar"));
                ui.checkbox(&mut c.two_capitals, t("Rätta två inledande versaler (SVerige blir Sverige)"));
                ui.checkbox(&mut c.typos, t("Vanliga stavfel (inbyggd lista)"));
                ui.checkbox(&mut c.replacements, t("Egna ersättningar"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(t("Språk:"));
                    ui.radio_value(&mut c.lang, None, t("Följ stavningskontrollen"));
                    ui.radio_value(&mut c.lang, Some("sv".to_owned()), "Svenska");
                    ui.radio_value(&mut c.lang, Some("en".to_owned()), "English");
                });
                ui.add_space(6.0);
                ui.label(egui::RichText::new(t("Egna ersättningar")).strong());
                ui.label(t("Ordet till vänster byts mot texten till höger när du skriver det och trycker mellanslag."));
                let mut remove = None;
                egui::ScrollArea::vertical().max_height(150.0).auto_shrink([true, true]).show(ui, |ui| {
                    for (i, (from, to)) in c.list.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(from).desired_width(100.0).id_salt(("ac-from", i)));
                            ui.label("→");
                            ui.add(egui::TextEdit::singleline(to).desired_width(250.0).id_salt(("ac-to", i)));
                            if ui.small_button("✕").on_hover_text(t("Ta bort")).clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                });
                if let Some(i) = remove {
                    c.list.remove(i);
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut dlg.new_from).desired_width(100.0).hint_text("mvh"));
                    ui.label("→");
                    ui.add(
                        egui::TextEdit::singleline(&mut dlg.new_to)
                            .desired_width(250.0)
                            .hint_text(t("Med vänliga hälsningar")),
                    );
                    let can = !dlg.new_from.trim().is_empty()
                        && !dlg.new_from.trim().contains(char::is_whitespace)
                        && !dlg.new_to.is_empty();
                    if ui.add_enabled(can, egui::Button::new(t("Lägg till"))).clicked() {
                        let from = dlg.new_from.trim().to_owned();
                        let to = std::mem::take(&mut dlg.new_to);
                        match c.list.iter_mut().find(|(f, _)| *f == from) {
                            Some(e) => e.1 = to,
                            None => c.list.push((from, to)),
                        }
                        dlg.new_from.clear();
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
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
            None => self.autocorrect_dlg = Some(dlg),
            Some(false) => {}
            Some(true) => {
                let mut cfg = dlg.cfg;
                // Tomma och ogiltiga rader sparas inte.
                cfg.list.retain(|(f, to)| {
                    let f = f.trim();
                    !f.is_empty() && !f.contains(char::is_whitespace) && !to.is_empty()
                });
                for e in &mut cfg.list {
                    e.0 = e.0.trim().to_owned();
                }
                self.settings.autocorrect = cfg;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skriver ett tecken i en editor som `ctx` håller tillstånd för och kör autokorrigeringen.
    struct Ed {
        ctx: Context,
        id: Id,
        text: String,
    }

    impl Ed {
        fn new(text: &str, caret_char: usize) -> Ed {
            let ctx = Context::default();
            let id = Id::unique("ac-test");
            let mut s = TextEditState::default();
            s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(caret_char))));
            s.store(&ctx, id);
            Ed { ctx, id, text: text.to_owned() }
        }

        /// Det TextEdit gör: lägg in tecknet vid markören, flytta markören, mata ångra-historiken.
        fn type_char(&mut self, c: char, cfg: &Config) -> bool {
            let mut s = TextEditState::load(&self.ctx, self.id).unwrap_or_default();
            let ci = s.cursor.char_range().unwrap().primary.index.0;
            let b = char_to_byte(&self.text, ci);
            let mut undoer = s.undoer();
            undoer.feed_state(0.0, &(s.cursor.char_range().unwrap(), self.text.clone()));
            self.text.insert(b, c);
            let after = CCursorRange::one(CCursor::new(ci + 1));
            s.cursor.set_char_range(Some(after));
            undoer.feed_state(0.1, &(after, self.text.clone()));
            s.set_undoer(undoer);
            s.store(&self.ctx, self.id);
            run_in_editor(&self.ctx, self.id, &mut self.text, cfg, Lang::Sv, c)
        }

        fn undo(&mut self) {
            let mut s = TextEditState::load(&self.ctx, self.id).unwrap();
            let cur = (s.cursor.char_range().unwrap(), self.text.clone());
            let mut undoer = s.undoer();
            // Som TextEdit: matas med aktuell bild först (korrigeringen), sedan ångra.
            undoer.feed_state(0.3, &cur);
            if let Some((range, text)) = undoer.undo(&cur).cloned() {
                self.text = text;
                s.cursor.set_char_range(Some(range));
            }
            s.set_undoer(undoer);
            s.store(&self.ctx, self.id);
        }
    }

    #[test]
    fn correction_moves_caret_and_text() {
        let mut ed = Ed::new("Vi ocskå", "Vi ocskå".chars().count());
        assert!(ed.type_char(' ', &Config::default()));
        assert_eq!(ed.text, "Vi också ");
        let s = TextEditState::load(&ed.ctx, ed.id).unwrap();
        assert_eq!(s.cursor.char_range().unwrap().primary.index.0, 9);
    }

    #[test]
    fn undo_reverts_only_the_correction() {
        let mut ed = Ed::new("Vi ocskå", 8);
        assert!(ed.type_char(' ', &Config::default()));
        assert_eq!(ed.text, "Vi också ");
        ed.undo();
        // Det du skrev (inklusive mellanslaget) är kvar, bara rättelsen är borta.
        assert_eq!(ed.text, "Vi ocskå ");
        let s = TextEditState::load(&ed.ctx, ed.id).unwrap();
        assert_eq!(s.cursor.char_range().unwrap().primary.index.0, 9);
    }

    #[test]
    fn no_fix_no_undo_point_and_selection_ignored() {
        let mut ed = Ed::new("Vi gör", 6);
        assert!(!ed.type_char(' ', &Config::default()));
        assert_eq!(ed.text, "Vi gör ");
        // Markering: ingenting rättas.
        let mut ed = Ed::new("ocskå ", 6);
        let mut s = TextEditState::load(&ed.ctx, ed.id).unwrap();
        s.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(6))));
        s.store(&ed.ctx, ed.id);
        assert!(!run_in_editor(&ed.ctx, ed.id, &mut ed.text, &Config::default(), Lang::Sv, ' '));
    }

    #[test]
    fn typed_char_must_precede_caret() {
        let ed = Ed::new("ocskå x", 7);
        let mut t = ed.text.clone();
        assert!(!run_in_editor(&ed.ctx, ed.id, &mut t, &Config::default(), Lang::Sv, ' '));
    }
}
