//! Läsläget för Markdown: en öppnad `.md`-fil visas färdigritad i hela fönstret, med en
//! Redigera-knapp uppe till höger. Redigera (eller Ctrl+Skift+M) byter till textredigeraren;
//! samma kortkommando eller knappen i namnlisten går tillbaka till läsläget. Ritningen delas
//! med förhandsvisningen bredvid texten (`Settings::side_preview`).

use std::borrow::Cow;
use std::collections::HashSet;

use egui::{Align2, CentralPanel, FontFamily, FontId, Frame, Rect, RichText, ScrollArea, Stroke, Ui, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::app::KladdApp;
use crate::doc::Document;
use crate::i18n::t;
use crate::theme::{self, Palette};

/// Samma nyckel som `egui_commonmark_backend::misc::BOLD_FAMILY_ID` i den lappade kopian i `vendor/`.
const BOLD_FAMILY_ID: &str = "egui_commonmark_bold_family";

/// Bredden på textspalten i läsläget, som i en vanlig läsvy.
const COLUMN_W: f32 = 760.0;

/// Ska fliken öppnas i läsläget? Vanliga Markdown-filer från disk; inte Word, ODT eller PDF
/// (de har dokumentläget och PDF-visaren), inte namnlösa flikar och inte stora filer.
pub fn opens_in_reading(doc: &Document) -> bool {
    doc.path.is_some()
        && !doc.docx
        && !doc.pdf
        && doc.pdfv.is_none()
        && !doc.word_view
        && matches!(doc.extension().as_str(), "md" | "markdown" | "mdown" | "mkd")
        && !crate::bigtext::is_big(&doc.text)
}

/// Ritar dokumentets Markdown som i förhandsvisningen: bilder, formler och Kladds HTML-format.
pub fn render(ui: &mut Ui, p: &Palette, doc: &Document, images: &mut HashSet<String>, cache: &mut CommonMarkCache) {
    let st = ui.style_mut();
    st.text_styles.insert(egui::TextStyle::Body, FontId::new(15.0, FontFamily::Proportional));
    st.text_styles.insert(egui::TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace));
    st.text_styles.insert(egui::TextStyle::Heading, FontId::new(26.0, theme::semibold()));
    st.spacing.item_spacing.y = 8.0;
    st.visuals.override_text_color = Some(p.text);
    // Fet text (`RichText::strong`) får textens färg; fetstilen kommer från typsnittet nedan.
    st.visuals.widgets.active.fg_stroke.color = p.text;
    // Den lappade egui_commonmark_backend (vendor/) ritar fet text med den här familjen.
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(BOLD_FAMILY_ID), theme::semibold()));
    // Bilder: inbäddade (bild:) registreras som minnesdata, övriga läses från disk.
    // HTML-formatet (<u>, <span>, <div>, <img> …) görs först om till vanlig Markdown.
    let with_math = crate::math::preview::replace(&doc.text, ui.ctx(), 15.0, p.text, ui.available_width());
    let mut text: Cow<str> = Cow::Owned(crate::preview::clean(&with_math));
    for (name, img) in &doc.images {
        let enc = name.replace(' ', "%20");
        let uri = format!("bytes://kladd/{}/{enc}", doc.id);
        if images.insert(uri.clone()) {
            ui.ctx().include_bytes(uri.clone(), img.bytes.clone());
        }
        let from = format!("]({}{enc})", crate::docx::IMAGE_SCHEME);
        if text.contains(&from) {
            text = Cow::Owned(text.replace(&from, &format!("]({uri})")));
        }
    }
    let home = crate::platform::home_dir();
    if let Some(home) = &home {
        if text.contains("](~/") {
            text = Cow::Owned(text.replace("](~/", &format!("](file://{}/", home.display())));
        }
    }
    let base = doc.base_dir().or(home).unwrap_or_default();
    let max_w = ui.available_width() as usize;
    CommonMarkViewer::new()
        .default_implicit_uri_scheme(format!("file://{}/", base.display()))
        .max_image_width(Some(max_w))
        .show(ui, cache, &text);
}

impl KladdApp {
    /// Sant när den aktiva fliken visas i läsläget.
    pub(crate) fn md_read_mode(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| {
            self.md_reading.contains(&d.id)
                && d.is_markdown()
                && !d.word_view
                && d.pdfv.is_none()
                && !self.big.has_view(d.id)
                && !self.panes.is_split()
        })
    }

    /// Byter mellan läsläget och redigeraren för den aktiva Markdown-fliken.
    pub(crate) fn toggle_md_reading(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        if !d.is_markdown() || d.word_view || d.pdfv.is_some() {
            return;
        }
        let id = d.id;
        if !self.md_reading.remove(&id) {
            self.md_reading.insert(id);
        } else {
            self.focus_editor = true;
        }
    }

    /// Läsläget: hela fönstret är den färdigritade texten, med Redigera uppe till höger.
    pub(crate) fn md_read_page(&mut self, ui: &mut Ui, p: &Palette) {
        // Sök och hopp (sökfältet, navigeringsfönstret, gå till rad) visar träffen i redigeraren.
        if self.find.open || self.scroll_to_char.is_some() {
            if let Some(d) = self.docs.get(self.active) {
                self.md_reading.remove(&d.id);
            }
            self.focus_editor = true;
            ui.ctx().request_repaint();
            return;
        }
        let mut edit = false;
        CentralPanel::default().frame(Frame::NONE.fill(p.editor)).show(ui, |ui| {
            self.editor_rect = ui.max_rect();
            let full = ui.max_rect();
            let Some(doc) = self.docs.get(self.active) else { return };
            ScrollArea::vertical()
                .id_salt(("kladd-mdread", doc.id))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let avail = ui.available_width();
                    let col = COLUMN_W.min(avail - 48.0).max(200.0);
                    let side = ((avail - col) / 2.0).max(24.0);
                    ui.horizontal_top(|ui| {
                        ui.add_space(side);
                        ui.vertical(|ui| {
                            ui.set_width(col);
                            ui.add_space(36.0);
                            render(ui, p, doc, &mut self.preview_images, &mut self.md_cache);
                            ui.add_space(60.0);
                        });
                    });
                });
            let btn = egui::Button::new(RichText::new(t("Redigera")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .corner_radius(8)
                .min_size(vec2(92.0, 28.0));
            let at = Align2::RIGHT_TOP.align_size_within_rect(vec2(92.0, 28.0), full.shrink2(vec2(22.0, 12.0)));
            let r = ui.put(Rect::from_min_size(at.min, at.size()), btn);
            if r.on_hover_text(t("Redigera texten (Ctrl+Shift+M)")).clicked() {
                edit = true;
            }
        });
        if edit {
            self.toggle_md_reading();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn bara_markdownfiler_från_disk_öppnas_i_läsläget() {
        let mut d = Document::new_untitled(1, 1);
        assert!(!opens_in_reading(&d), "namnlös flik redigeras direkt");
        d.path = Some(PathBuf::from("/tmp/anteckning.md"));
        assert!(opens_in_reading(&d));
        d.path = Some(PathBuf::from("/tmp/README.markdown"));
        assert!(opens_in_reading(&d));
        d.path = Some(PathBuf::from("/tmp/kod.rs"));
        assert!(!opens_in_reading(&d));
        d.path = Some(PathBuf::from("/tmp/brev.docx"));
        d.docx = true;
        d.word_view = true;
        assert!(!opens_in_reading(&d), "Word har dokumentläget");
    }
}
