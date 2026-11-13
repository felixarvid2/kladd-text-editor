//! Gränssnittet för avsnitt: menyåtgärderna i "Infoga" och dialogen "Spalter". Själva
//! redigeringen av Markdown-texten ligger i `sections`.

use egui::{Align, Color32, FontId, Frame, Id, Layout, Margin, RichText, Stroke, vec2};

use super::sections::{self, Section};
use crate::app::{KladdApp, quiet_button};
use crate::theme::{self, Palette};

/// 1 mm i pt.
const MM: f32 = 72.0 / 25.4;

/// Dialogens tillstånd medan den är öppen.
pub struct ColumnsDialog {
    cols: usize,
    gap_mm: f32,
}

impl ColumnsDialog {
    pub fn from_section(s: &Section) -> Self {
        Self { cols: s.columns.min(3), gap_mm: (s.gap / MM * 10.0).round() / 10.0 }
    }
}

impl KladdApp {
    /// Avsnittet (med dokumentets grundformat) som markören står i.
    fn current_section(&mut self, ctx: &egui::Context) -> Option<(super::markup::DocDefaults, Section)> {
        let sel = self.word_selection(ctx);
        let doc = self.docs.get(self.active)?;
        let a = self.word.analysis(doc.id, &doc.text);
        let i = sections::index_at(&a.sections, sel.start);
        Some((a.defaults.clone(), a.sections[i].clone()))
    }

    /// Avsnittsbrytning med ny sida; det nya avsnittet börjar som det nuvarande.
    pub(crate) fn section_break(&mut self, ctx: &egui::Context) {
        let Some((_, sec)) = self.current_section(ctx) else { return };
        let mut props = sec.props;
        props.continuous = false;
        self.word_edit(ctx, |t, s| sections::insert_break(t, s.end, &props));
    }

    /// Liggande (eller stående) sidor från markören.
    pub(crate) fn section_landscape(&mut self, ctx: &egui::Context, landscape: bool) {
        let Some((d, _)) = self.current_section(ctx) else { return };
        self.word_edit(ctx, |t, s| sections::set_landscape_from(t, s.start, &d, landscape));
    }

    /// Spaltbrytning vid markören.
    pub(crate) fn column_break(&mut self, ctx: &egui::Context) {
        self.word_edit(ctx, |t, s| sections::insert_column_break(t, s.end));
    }

    /// Öppnar "Spalter" för avsnittet som markören står i.
    pub(crate) fn open_columns_dialog(&mut self, ctx: &egui::Context) {
        if let Some((_, s)) = self.current_section(ctx) {
            self.wv.cols_dlg = Some(ColumnsDialog::from_section(&s));
        }
    }

    pub(crate) fn columns_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(d) = self.wv.cols_dlg.as_mut() else { return };
        let mut ok = None;
        let modal = egui::Modal::new(Id::unique("kladd-columns"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
            .frame(
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(14)
                    .inner_margin(Margin::same(24))
                    .shadow(ctx.global_style().visuals.window_shadow),
            )
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(RichText::new("Spalter").font(FontId::new(17.0, theme::semibold())).color(p.text));
                ui.label(RichText::new("Gäller avsnittet som markören står i").color(p.muted).size(12.5));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    for (n, name) in [(1, "En"), (2, "Två"), (3, "Tre")] {
                        ui.selectable_value(&mut d.cols, n, name);
                    }
                });
                ui.add_space(8.0);
                ui.add_enabled_ui(d.cols > 1, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Mellanrum").color(p.muted));
                        ui.add(egui::DragValue::new(&mut d.gap_mm).range(2.0..=60.0).speed(0.5).suffix(" mm").max_decimals(1));
                    });
                });
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let b = egui::Button::new(RichText::new("OK").font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .min_size(vec2(84.0, 32.0));
                    if ui.add(b).clicked() {
                        ok = Some(true);
                    }
                    if ui.add(quiet_button("Avbryt", p).min_size(vec2(0.0, 32.0))).clicked() {
                        ok = Some(false);
                    }
                });
            });
        if modal.should_close() && ok.is_none() {
            ok = Some(false);
        }
        let Some(ok) = ok else { return };
        let Some(d) = self.wv.cols_dlg.take() else { return };
        if !ok {
            return;
        }
        let Some((defaults, _)) = self.current_section(ctx) else { return };
        let (cols, gap) = (d.cols, d.gap_mm * MM);
        self.word_edit(ctx, |t, s| {
            sections::update_current(t, s.start, &defaults, |p| {
                p.columns = (cols > 1).then_some(cols);
                p.column_gap = (cols > 1).then_some(gap);
            })
        });
    }
}
