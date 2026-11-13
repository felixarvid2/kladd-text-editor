//! Dialogen "Ändra formatmall": Brödtext, Rubrik 1-6 och Citat med förhandsvisning.
//!
//! Dialogen är ett vanligt fönster (inte modal) så att dokumentet syns bakom: varje ändring
//! skrivs direkt in i `<!-- kladd: … -->`-raden och gäller hela dokumentet. "Uppdatera från
//! markering" tar utseendet på texten vid markören, "Återställ" tar bort mallens ändringar.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, FontId, Frame, Id, Layout, Margin, RichText, Stroke, Ui, vec2};

use super::layout::{LINE_SINGLE, px};
use super::markup::{self, Align as HAlign, DocDefaults};
use super::pages::set_defaults_line;
use crate::app::{KladdApp, quiet_button};
use crate::i18n::t;
use crate::style::{STYLE_IDS, StyleId, StyleOverride, StyleSet};
use crate::tf;
use crate::theme::Palette;

/// Dialogens tillstånd medan den är öppen.
pub struct StylesDialog {
    /// Dokumentet som dialogen ändrar.
    doc_id: u64,
    /// Vald formatmall i listan.
    sel: StyleId,
    /// Det dialogen visar och redigerar.
    set: StyleSet,
    /// Det som senast skrevs till (eller lästes från) dokumentet.
    applied: StyleSet,
}

/// Ett val i dialogen: en egenskap som antingen ärvs (`None`) eller har ett värde.
fn toggled<T: Clone>(ui: &mut Ui, slot: &mut Option<T>, init: T, add: impl FnOnce(&mut Ui, &mut T)) {
    ui.horizontal(|ui| {
        let mut on = slot.is_some();
        if ui.checkbox(&mut on, "").changed() {
            *slot = on.then(|| init.clone());
        }
        match slot {
            Some(v) => add(ui, v),
            None => {
                ui.label(RichText::new(t("Standard")).weak());
            }
        }
    });
}

/// Radhöjd (px) för förhandsvisningen, samma tolkning som layouten.
fn preview_line_height(size_px: f32, heading: bool, o: &StyleOverride) -> f32 {
    match o.line_spacing {
        Some(m) => size_px * LINE_SINGLE * m,
        None if heading => size_px * 1.3,
        None => size_px * 1.4,
    }
}

impl KladdApp {
    /// Öppnar "Ändra formatmall" för det aktiva dokumentet, med `sel` vald.
    pub(crate) fn open_styles_dialog(&mut self, sel: Option<StyleId>) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let id = doc.id;
        let a = self.word.analysis(id, &doc.text);
        let set = a.defaults.styles.clone();
        let sel = sel.unwrap_or(StyleId::Body);
        self.wv.styles_dlg = Some(StylesDialog { doc_id: id, sel, applied: set.clone(), set });
    }

    /// Skriver `set` som formatmallar i dokumentets grundformatsrad.
    fn apply_style_set(&mut self, ctx: &egui::Context, set: StyleSet) {
        self.word_edit(ctx, |t, sel| {
            let mut d = DocDefaults::from_comment(t.lines().next().unwrap_or("")).unwrap_or_default();
            d.styles = set;
            let (text, old_len, new_len) = set_defaults_line(t, &d);
            let map = |b: usize| if b >= old_len { b - old_len + new_len } else { b.min(new_len) };
            markup::Edit { text, sel: map(sel.start)..map(sel.end) }
        });
    }

    /// Utseendet på texten vid markören: (typsnitt, storlek, färg, fet, kursiv, justering).
    fn caret_look(&mut self, ctx: &egui::Context) -> Option<markup::Active> {
        let mut found = None;
        self.word_edit(ctx, |t, sel| {
            let a = markup::analyze(t);
            found = Some(markup::active(t, &a, sel.clone()));
            markup::Edit { text: t.to_owned(), sel }
        });
        found
    }

    pub(crate) fn styles_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(dlg) = self.wv.styles_dlg.as_ref() else { return };
        let Some(doc) = self.docs.get(self.active) else {
            self.wv.styles_dlg = None;
            return;
        };
        if doc.id != dlg.doc_id {
            self.wv.styles_dlg = None;
            return;
        }
        // Ändrades dokumentet utifrån (ångra, redigering i textläge) följer dialogen med.
        let doc_id = doc.id;
        let cur = self.word.analysis(doc_id, &doc.text).defaults.clone();
        let Some(mut dlg) = self.wv.styles_dlg.take() else { return };
        if cur.styles != dlg.applied {
            dlg.set = cur.styles.clone();
            dlg.applied = cur.styles.clone();
        }
        let body_size = cur.size();
        let doc_font = cur.font().to_owned();
        let mut fonts: Vec<String> = self.doc_fonts.installed().to_vec();
        fonts.sort_by_key(|f| f.to_lowercase());
        fonts.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        let cur_font = dlg.set.get(dlg.sel).font_family.clone();
        if let Some(f) = cur_font.as_ref().filter(|f| !fonts.iter().any(|n| n.eq_ignore_ascii_case(f))) {
            fonts.insert(0, f.clone());
        }
        let preview_family = {
            let o = dlg.set.get(dlg.sel);
            let heading = matches!(dlg.sel, StyleId::Heading(_));
            let bold = o.bold.unwrap_or(heading);
            let italic = o.italic.unwrap_or(dlg.sel == StyleId::Quote);
            let name = o.font_family.clone().or_else(|| (!heading).then(|| doc_font.clone()));
            self.doc_fonts.family(ctx, name.as_deref(), bold, italic)
        };

        let mut open = true;
        let mut update_from_caret = false;
        let mut done = false;
        egui::Window::new(t("Ändra formatmall"))
            .id(Id::unique("kladd-styles-dialog"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(egui::pos2((ctx.content_rect().right() - 620.0).max(8.0), 96.0))
            .frame(
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(12)
                    .inner_margin(Margin::same(16))
                    .shadow(ctx.global_style().visuals.window_shadow),
            )
            .show(ctx, |ui| {
                ui.set_width(580.0);
                ui.horizontal_top(|ui| {
                    // Listan med formatmallar
                    ui.vertical(|ui| {
                        ui.set_width(120.0);
                        for id in STYLE_IDS {
                            let changed = !dlg.set.get(id).is_empty();
                            let label = if changed { format!("{} •", id.label()) } else { id.label() };
                            if ui.selectable_label(dlg.sel == id, label).clicked() {
                                dlg.sel = id;
                            }
                        }
                    });
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        let sel = dlg.sel;
                        let heading = matches!(sel, StyleId::Heading(_));
                        let mut o = dlg.set.get(sel).clone();
                        egui::Grid::new("kladd-styles-grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                            ui.label(RichText::new(t("Typsnitt")).color(p.muted));
                            toggled(ui, &mut o.font_family, doc_font.clone(), |ui, v| {
                                egui::ComboBox::from_id_salt("kladd-styles-font").selected_text(v.clone()).width(170.0).show_ui(
                                    ui,
                                    |ui| {
                                        for f in &fonts {
                                            ui.selectable_value(v, f.clone(), f);
                                        }
                                    },
                                );
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Storlek")).color(p.muted));
                            toggled(ui, &mut o.size, sel.default_size(body_size), |ui, v| {
                                ui.add(egui::DragValue::new(v).range(4.0..=96.0).speed(0.25).suffix(" pt").max_decimals(1));
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Fet")).color(p.muted));
                            toggled(ui, &mut o.bold, heading, |ui, v| {
                                ui.checkbox(v, "");
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Kursiv")).color(p.muted));
                            toggled(ui, &mut o.italic, sel == StyleId::Quote, |ui, v| {
                                ui.checkbox(v, "");
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Färg")).color(p.muted));
                            toggled(ui, &mut o.color, [0, 0, 0], |ui, v| {
                                ui.color_edit_button_srgb(v);
                                ui.label(crate::style::hex(*v));
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Justering")).color(p.muted));
                            toggled(ui, &mut o.align, HAlign::Left, |ui, v| {
                                for (a, name) in [
                                    (HAlign::Left, t("Vänster")),
                                    (HAlign::Center, t("Mitten")),
                                    (HAlign::Right, t("Höger")),
                                    (HAlign::Justify, t("Block")),
                                ] {
                                    ui.selectable_value(v, a, name);
                                }
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Luft före")).color(p.muted));
                            toggled(ui, &mut o.space_before, 0.0, |ui, v| {
                                ui.add(egui::DragValue::new(v).range(0.0..=200.0).speed(0.5).suffix(" pt").max_decimals(1));
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Luft efter")).color(p.muted));
                            toggled(ui, &mut o.space_after, 0.0, |ui, v| {
                                ui.add(egui::DragValue::new(v).range(0.0..=200.0).speed(0.5).suffix(" pt").max_decimals(1));
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Radavstånd")).color(p.muted));
                            toggled(ui, &mut o.line_spacing, 1.15, |ui, v| {
                                ui.add(egui::DragValue::new(v).range(0.5..=5.0).speed(0.01).max_decimals(2).suffix(" ×"));
                            });
                            ui.end_row();
                            ui.label(RichText::new(t("Indrag, första raden")).color(p.muted));
                            toggled(ui, &mut o.first_indent, 18.0, |ui, v| {
                                ui.add(egui::DragValue::new(v).range(0.0..=200.0).speed(0.5).suffix(" pt").max_decimals(1));
                            });
                            ui.end_row();
                        });
                        dlg.set.set(sel, o);
                    });
                });
                ui.add_space(10.0);
                // Förhandsvisning av vald formatmall.
                let sel = dlg.sel;
                let o = dlg.set.get(sel).clone();
                let heading = matches!(sel, StyleId::Heading(_));
                let size = px(1.0, o.size.unwrap_or_else(|| sel.default_size(body_size)));
                let color = match (o.color, sel) {
                    (Some(c), _) => Color32::from_rgb(c[0], c[1], c[2]),
                    (None, StyleId::Heading(1)) => p.page_heading1,
                    (None, StyleId::Heading(6)) | (None, StyleId::Quote) => p.page_muted,
                    (None, StyleId::Heading(_)) => p.page_heading,
                    (None, StyleId::Body) => p.page_text,
                };
                let sample = if heading {
                    tf!("{style} – exempeltext", style = sel.label())
                } else {
                    t("Så här ser stycket ut. Ändringarna gäller hela dokumentet direkt.").to_owned()
                };
                Frame::NONE.fill(p.page).stroke(Stroke::new(1.0, p.line)).inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(70.0);
                    ui.add_space(px(1.0, o.space_before.unwrap_or(0.0)).min(40.0));
                    let mut job = LayoutJob::default();
                    job.wrap.max_width = ui.available_width();
                    job.halign = match o.align.unwrap_or(HAlign::Left) {
                        HAlign::Center => Align::Center,
                        HAlign::Right => Align::RIGHT,
                        _ => Align::LEFT,
                    };
                    job.justify = o.align == Some(HAlign::Justify);
                    job.append(
                        &sample,
                        px(1.0, o.first_indent.unwrap_or(0.0)),
                        TextFormat {
                            font_id: FontId::new(size, preview_family.clone()),
                            color,
                            line_height: Some(preview_line_height(size, heading, &o)),
                            ..Default::default()
                        },
                    );
                    ui.add(egui::Label::new(job));
                    ui.add_space(px(1.0, o.space_after.unwrap_or(0.0)).min(40.0));
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.add(quiet_button(t("Uppdatera från markering"), p)).on_hover_text(t("Ta utseendet på texten vid markören")).clicked() {
                        update_from_caret = true;
                    }
                    if ui.add(quiet_button(t("Återställ"), p)).on_hover_text(t("Ta bort ändringarna i den här formatmallen")).clicked() {
                        dlg.set.set(dlg.sel, StyleOverride::default());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(quiet_button(t("Klar"), p)).clicked() {
                            done = true;
                        }
                    });
                });
            });
        if update_from_caret {
            if let Some(a) = self.caret_look(ctx) {
                let mut o = dlg.set.get(dlg.sel).clone();
                if let Some(f) = a.span.font_family.clone() {
                    o.font_family = Some(f);
                }
                if let Some(s) = a.span.font_size {
                    o.size = Some(s);
                }
                if let Some(c) = a.span.color {
                    o.color = Some(c);
                }
                if a.bold {
                    o.bold = Some(true);
                }
                if a.italic {
                    o.italic = Some(true);
                }
                if a.align != HAlign::Left {
                    o.align = Some(a.align);
                }
                dlg.set.set(dlg.sel, o);
            }
        }
        if dlg.set != dlg.applied {
            let set = dlg.set.clone();
            self.apply_style_set(ctx, set.clone());
            dlg.applied = set;
        }
        if open && !done {
            self.wv.styles_dlg = Some(dlg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_line_height_follows_layout() {
        let none = StyleOverride::default();
        assert_eq!(preview_line_height(10.0, false, &none), 14.0);
        assert_eq!(preview_line_height(10.0, true, &none), 13.0);
        let o = StyleOverride { line_spacing: Some(2.0), ..Default::default() };
        assert_eq!(preview_line_height(10.0, false, &o), 10.0 * LINE_SINGLE * 2.0);
    }
}
