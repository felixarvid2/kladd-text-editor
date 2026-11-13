//! Dialogen "Versionshistorik": lista över sparade versioner, diff mot aktuell text och knappar
//! för att återställa eller öppna en version i en ny flik.

use std::path::{Path, PathBuf};

use egui::{Align, Color32, FontFamily, FontId, Frame, Id, Layout, Margin, RichText, ScrollArea, Sense, Stroke, Ui, vec2};

use crate::doc::Document;
use crate::history::{self, DiffRow, LineKind, Version};
use crate::i18n::t;
use crate::tf;
use crate::theme::{self, Palette};

/// Större texter diffas inte (diffen körs i gränssnittstråden).
const MAX_DIFF_BYTES: usize = 2 << 20;
const ROW_H: f32 = 18.0;

pub enum Action {
    None,
    Close,
    /// Ersätt flikens innehåll med versionen.
    Restore(u64),
    /// Öppna versionen i en ny flik.
    OpenNew(u64),
}

enum View {
    Diff(Vec<DiffRow>),
    Message(String),
}

pub struct Dialog {
    pub doc_id: u64,
    pub path: PathBuf,
    binary: bool,
    root: PathBuf,
    versions: Vec<Version>,
    selected: Option<usize>,
    view: Option<View>,
    /// Flikens text när dialogen öppnades (dialogen är modal, så den ändras inte under tiden).
    current: String,
}

impl Dialog {
    pub fn new(doc: &Document, path: &Path) -> Self {
        let root = history::root();
        let versions = history::list(&root, path);
        let mut d = Self {
            doc_id: doc.id,
            path: path.to_path_buf(),
            binary: history::is_binary_path(path),
            root,
            versions,
            selected: None,
            view: None,
            current: doc.text.clone(),
        };
        d.select(0);
        d
    }

    fn select(&mut self, i: usize) {
        let Some(v) = self.versions.get(i) else { return };
        self.selected = Some(i);
        self.view = if self.binary {
            None
        } else {
            Some(self.make_diff(v.ts_ms))
        };
    }

    fn make_diff(&self, ts: u64) -> View {
        let bytes = match history::read(&self.root, &self.path, ts) {
            Ok(b) => b,
            Err(_) => return View::Message(t("Versionen kunde inte läsas.").into()),
        };
        if bytes.len() > MAX_DIFF_BYTES || self.current.len() > MAX_DIFF_BYTES {
            return View::Message(t("Texten är för stor för att jämföras här. Återställ eller öppna versionen i en ny flik.").into());
        }
        match Document::from_version(0, &self.path, &bytes) {
            Ok(old) => {
                let rows = history::collapse(history::diff_lines(&old.text, &self.current), 3);
                View::Diff(rows)
            }
            Err(e) => View::Message(e),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, p: &Palette, now_ms: u64) -> Action {
        let mut action = Action::None;
        let mut pick = None;
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let modal = egui::Modal::new(Id::unique("kladd-history"))
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
                let w = (ctx.content_rect().width() - 120.0).clamp(420.0, 920.0);
                ui.set_width(w);
                ui.label(
                    RichText::new(tf!("Versionshistorik för {name}", name = name))
                        .font(FontId::new(17.0, theme::semibold()))
                        .color(p.text),
                );
                ui.add_space(10.0);
                if self.versions.is_empty() {
                    ui.label(
                        RichText::new(t("Inga tidigare versioner än. En version sparas varje gång du sparar filen."))
                            .color(p.muted),
                    );
                } else {
                    let h = (ctx.content_rect().height() - 260.0).clamp(160.0, 480.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(250.0);
                            ScrollArea::vertical().id_salt("hist-list").max_height(h).min_scrolled_height(h).show(ui, |ui| {
                                for (i, v) in self.versions.iter().enumerate() {
                                    if version_row(ui, p, v, now_ms, self.selected == Some(i)) {
                                        pick = Some(i);
                                    }
                                }
                            });
                        });
                        ui.add_space(8.0);
                        ui.vertical(|ui| self.right_pane(ui, p, h));
                    });
                }
                ui.add_space(16.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if ui.add(crate::app::quiet_button(t("Stäng"), p).min_size(vec2(0.0, 32.0))).clicked() {
                        action = Action::Close;
                    }
                    if let Some(v) = self.selected.and_then(|i| self.versions.get(i)) {
                        if ui.add(crate::app::quiet_button(t("Öppna som ny flik"), p).min_size(vec2(0.0, 32.0))).clicked() {
                            action = Action::OpenNew(v.ts_ms);
                        }
                        let restore = egui::Button::new(
                            RichText::new(t("Återställ")).font(FontId::new(13.5, theme::medium())).color(p.on_accent),
                        )
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .min_size(vec2(96.0, 32.0));
                        if ui.add(restore).on_hover_text(t("Ersätter flikens innehåll med versionen. Går att ångra.")).clicked() {
                            action = Action::Restore(v.ts_ms);
                        }
                    }
                });
            });
        if let Some(i) = pick {
            self.select(i);
        }
        if modal.should_close() && matches!(action, Action::None) {
            action = Action::Close;
        }
        action
    }

    fn right_pane(&self, ui: &mut Ui, p: &Palette, h: f32) {
        ui.set_min_height(h);
        match &self.view {
            None => {
                ui.label(
                    RichText::new(t("Det här formatet kan inte jämföras som text. Återställ versionen eller öppna den i en ny flik."))
                        .color(p.muted),
                );
            }
            Some(View::Message(m)) => {
                ui.label(RichText::new(m).color(p.muted));
            }
            Some(View::Diff(rows)) => {
                ui.label(
                    RichText::new(t("Röd: finns i versionen men inte nu. Grön: finns nu men inte i versionen."))
                        .font(FontId::new(12.0, FontFamily::Proportional))
                        .color(p.muted),
                );
                ui.add_space(4.0);
                if rows.iter().all(|r| matches!(r, DiffRow::Gap(_))) {
                    ui.label(RichText::new(t("Versionen är identisk med den aktuella texten.")).color(p.muted));
                    return;
                }
                let (add_bg, del_bg) = if p.dark {
                    (Color32::from_rgba_unmultiplied(70, 170, 90, 55), Color32::from_rgba_unmultiplied(210, 80, 80, 60))
                } else {
                    (Color32::from_rgba_unmultiplied(70, 170, 90, 60), Color32::from_rgba_unmultiplied(210, 80, 80, 55))
                };
                let font = FontId::new(12.5, FontFamily::Monospace);
                ScrollArea::both().id_salt("hist-diff").max_height(h - 26.0).auto_shrink([false, false]).show_rows(
                    ui,
                    ROW_H,
                    rows.len(),
                    |ui, range| {
                        for row in &rows[range] {
                            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), ROW_H), Sense::hover());
                            let (bg, mark, text, color) = match row {
                                DiffRow::Gap(n) => (None, " ", tf!("… {n} oförändrade rader", n = n), p.faint),
                                DiffRow::Line(l) => match l.kind {
                                    LineKind::Same => (None, " ", l.text.clone(), p.text),
                                    LineKind::Added => (Some(add_bg), "+", l.text.clone(), p.text),
                                    LineKind::Removed => (Some(del_bg), "−", l.text.clone(), p.text),
                                },
                            };
                            if let Some(bg) = bg {
                                ui.painter().rect_filled(rect, 2, bg);
                            }
                            let shown = text.replace('\t', "    ");
                            ui.painter().with_clip_rect(rect).text(
                                rect.left_center() + vec2(6.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                format!("{mark} {shown}"),
                                font.clone(),
                                color,
                            );
                        }
                    },
                );
            }
        }
    }
}

fn version_row(ui: &mut Ui, p: &Palette, v: &Version, now_ms: u64, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 6, p.selection);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 6, p.surface_hover);
    }
    ui.painter().text(
        rect.left_top() + vec2(10.0, 8.0),
        egui::Align2::LEFT_TOP,
        format_time(v.ts_ms),
        FontId::new(13.5, theme::medium()),
        p.text,
    );
    ui.painter().text(
        rect.left_top() + vec2(10.0, 24.0),
        egui::Align2::LEFT_TOP,
        format!("{}  ·  {}", format_age(now_ms, v.ts_ms), format_size(v.size)),
        FontId::new(12.0, FontFamily::Proportional),
        p.muted,
    );
    resp.clicked()
}

/// Ersätter flikens innehåll med en inläst version som ett eget ångra-steg. Fliken behåller sin sökväg
/// och blir ändrad.
pub fn restore_into(ctx: &egui::Context, doc: &mut Document, version: Document) {
    use egui::text::{CCursor, CCursorRange};
    use egui::text_edit::TextEditState;
    let id = crate::app::editor_id(doc.id);
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    let end = doc.text.chars().count();
    let before = state.cursor.char_range().unwrap_or(CCursorRange::two(CCursor::new(end), CCursor::new(end)));
    let mut undoer = state.undoer();
    undoer.add_undo(&(before, doc.text.clone()));
    state.set_undoer(undoer);
    doc.text = version.text;
    doc.encoding = version.encoding;
    doc.line_ending = version.line_ending;
    doc.images = version.images;
    doc.docx_source = version.docx_source;
    doc.docx_dropped = version.docx_dropped;
    doc.on_edited();
    doc.dirty = true;
    let end = doc.text.chars().count();
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(end.min(before.primary.index.0)))));
    state.store(ctx, id);
}

/// Lokal tid, t.ex. "2026-11-13 14:32:05".
pub fn format_time(ts_ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ts_ms as i64)
        .single()
        .map(|dt| {
            use chrono::{Datelike, Timelike};
            match crate::i18n::lang() {
                crate::i18n::Lang::Sv => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
                crate::i18n::Lang::En => {
                    let date = crate::i18n::date(dt.year(), dt.month(), dt.day());
                    let hour = if dt.hour() % 12 == 0 { 12 } else { dt.hour() % 12 };
                    format!("{date} {hour}:{:02}:{:02} {}", dt.minute(), dt.second(), if dt.hour() < 12 { "AM" } else { "PM" })
                }
            }
        })
        .unwrap_or_default()
}

/// "nu", "5 min sedan", "3 tim sedan", "2 dagar sedan".
pub fn format_age(now_ms: u64, ts_ms: u64) -> String {
    let s = now_ms.saturating_sub(ts_ms) / 1000;
    match s {
        0..=59 => t("nu").to_owned(),
        60..=3599 => tf!("{n} min sedan", n = s / 60),
        3600..=86_399 => tf!("{n} tim sedan", n = s / 3600),
        _ => {
            let d = s / 86_400;
            if d == 1 { t("1 dag sedan").to_owned() } else { tf!("{n} dagar sedan", n = d) }
        }
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} kB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_labels() {
        assert_eq!(format_age(100_000, 100_000), "nu");
        assert_eq!(format_age(100_000 + 59_000, 100_000), "nu");
        assert_eq!(format_age(100_000 + 5 * 60_000, 100_000), "5 min sedan");
        assert_eq!(format_age(100_000 + 3 * 3_600_000, 100_000), "3 tim sedan");
        assert_eq!(format_age(100_000 + 86_400_000, 100_000), "1 dag sedan");
        assert_eq!(format_age(100_000 + 3 * 86_400_000, 100_000), "3 dagar sedan");
        assert_eq!(format_age(0, 100_000), "nu");
    }

    #[test]
    fn size_labels() {
        assert_eq!(format_size(12), "12 B");
        assert_eq!(format_size(2048), "2.0 kB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
    }
}
