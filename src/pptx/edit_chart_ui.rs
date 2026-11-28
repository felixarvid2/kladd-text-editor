//! Fönstret för diagramdata i redigeringsläget: en tabell med kategorier (rader) × serier (kolumner),
//! seriernas namn överst, knappar för att lägga till och ta bort kategorier och serier, och diagrammets
//! rubrik och axelrubriker som textfält. *OK* skriver allt i ett ångringssteg (`edit_chart::write`).
//! Öppnas med dubbelklick på ett diagram (`PptxTab::open_chart`, kroken i `edit_view.rs`).

use egui::{Context, Id, RichText, TextEdit};

use super::edit_chart::{self, Cat, ChartData, SeriesData, TitleData};
use super::edit_view::Sel;
use super::view::PptxTab;
use crate::i18n::t;

/// Tabellen under redigering (text i alla fält).
pub struct ChartDialog {
    pub sel: Sel,
    pub cats: Vec<String>,
    /// Kategoriernas ursprungliga text och tal (ett tal behålls om texten inte ändrats).
    orig_cats: Vec<Cat>,
    pub names: Vec<String>,
    pub vals: Vec<Vec<String>>,
    /// Seriernas ursprung (`SeriesData::orig`).
    orig: Vec<Option<usize>>,
    pub titles: Vec<(TitleData, String)>,
    numeric_cats: bool,
    /// Senast ritade bildruta (fönstret ritas en gång per bildruta).
    drawn: u64,
}

impl ChartDialog {
    pub fn new(sel: Sel, d: ChartData) -> ChartDialog {
        ChartDialog {
            sel,
            cats: d.cats.iter().map(|c| c.text.clone()).collect(),
            orig_cats: d.cats.clone(),
            names: d.series.iter().map(|s| s.name.clone()).collect(),
            vals: d.series.iter().map(|s| s.vals.iter().map(|v| v.map(edit_chart::num_text).unwrap_or_default()).collect()).collect(),
            orig: d.series.iter().map(|s| s.orig).collect(),
            titles: d.titles.iter().map(|t| (t.clone(), t.text.clone().unwrap_or_default())).collect(),
            numeric_cats: d.numeric_cats,
            drawn: u64::MAX,
        }
    }

    /// Tabellen som `ChartData` (oparsbara värden blir tomma).
    pub fn data(&self) -> ChartData {
        let cats = self
            .cats
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let num = self.orig_cats.get(i).filter(|o| o.text == *text).and_then(|o| o.num);
                Cat { text: text.clone(), num }
            })
            .collect();
        let series = self
            .names
            .iter()
            .enumerate()
            .map(|(k, name)| SeriesData {
                name: name.clone(),
                vals: self.vals[k].iter().map(|v| {
                    let t = v.trim().replace(',', ".").replace([' ', '\u{a0}'], "");
                    t.parse::<f64>().ok().filter(|x| x.is_finite())
                }).collect(),
                orig: self.orig[k],
            })
            .collect();
        let titles = self.titles.iter().map(|(t, s)| TitleData { text: (!s.trim().is_empty()).then(|| s.clone()), ..t.clone() }).collect();
        ChartData { cats, series, titles, numeric_cats: self.numeric_cats }
    }

    pub fn add_cat(&mut self) {
        self.cats.push(format!("{} {}", t("Kategori"), self.cats.len() + 1));
        for v in &mut self.vals {
            v.push(String::new());
        }
    }

    pub fn remove_cat(&mut self, i: usize) {
        if self.cats.len() <= 1 || i >= self.cats.len() {
            return;
        }
        self.cats.remove(i);
        if i < self.orig_cats.len() {
            self.orig_cats.remove(i);
        }
        for v in &mut self.vals {
            if i < v.len() {
                v.remove(i);
            }
        }
    }

    pub fn add_series(&mut self) {
        self.names.push(format!("{} {}", t("Serie"), self.names.len() + 1));
        self.vals.push(vec![String::new(); self.cats.len()]);
        self.orig.push(None);
    }

    pub fn remove_series(&mut self, k: usize) {
        if self.names.len() <= 1 || k >= self.names.len() {
            return;
        }
        self.names.remove(k);
        self.vals.remove(k);
        self.orig.remove(k);
    }
}

/// Rubrikens etikett i fönstret.
fn title_label(td: &TitleData) -> String {
    let Some(_) = &td.axis else { return t("Diagramrubrik").to_owned() };
    let (kind, pos) = td.label.split_once(':').unwrap_or((td.label.as_str(), ""));
    let what = match kind {
        "valAx" => t("värden"),
        "serAx" => t("serier"),
        _ => t("kategorier"),
    };
    let side = match pos {
        "l" => t("vänster"),
        "r" => t("höger"),
        "t" => t("överst"),
        _ => t("nederst"),
    };
    format!("{} ({what}, {side})", t("Axelrubrik"))
}

impl PptxTab {
    /// Öppnar diagramdata för formen (dubbelklick på ett diagram). `false` om formen inte är ett diagram.
    pub(super) fn open_chart(&mut self, sel: Sel) -> bool {
        match edit_chart::read_shape(&self.deck, sel.slide, sel.index) {
            Some((_, data)) => {
                self.edit.sel = Some(sel);
                self.edit.chart = Some(ChartDialog::new(sel, data));
                true
            }
            None => false,
        }
    }

    /// Ritar fönstret för diagramdata (om det är öppet).
    pub(super) fn chart_dialog(&mut self, ctx: &Context) {
        let Some(dlg) = &mut self.edit.chart else { return };
        let frame = ctx.cumulative_pass_nr();
        if dlg.drawn == frame {
            return;
        }
        dlg.drawn = frame;
        let mut open = true;
        let mut action: Option<bool> = None;
        egui::Window::new(t("Diagramdata"))
            .id(Id::unique("kladd-pptx-chart-data"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .default_pos(egui::pos2((ctx.content_rect().right() - 600.0).max(0.0), 110.0))
            .show(ctx, |ui| {
                // Rubriker.
                egui::Grid::new("kladd-chart-titles").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    for (td, text) in &mut dlg.titles {
                        ui.label(title_label(td));
                        ui.add_sized([280.0, 20.0], TextEdit::singleline(text));
                        ui.end_row();
                    }
                });
                ui.separator();
                let (mut del_cat, mut del_ser) = (None, None);
                egui::ScrollArea::both().max_height(360.0).show(ui, |ui| {
                    egui::Grid::new("kladd-chart-data").striped(true).spacing([4.0, 3.0]).show(ui, |ui| {
                        ui.label("");
                        for (k, name) in dlg.names.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add_sized([96.0, 20.0], TextEdit::singleline(name));
                                if ui.small_button("✕").on_hover_text(t("Ta bort serien")).clicked() {
                                    del_ser = Some(k);
                                }
                            });
                        }
                        ui.end_row();
                        for i in 0..dlg.cats.len() {
                            ui.horizontal(|ui| {
                                if ui.small_button("✕").on_hover_text(t("Ta bort kategorin")).clicked() {
                                    del_cat = Some(i);
                                }
                                ui.add_sized([110.0, 20.0], TextEdit::singleline(&mut dlg.cats[i]));
                            });
                            for v in dlg.vals.iter_mut() {
                                if let Some(cell) = v.get_mut(i) {
                                    let bad = !cell.trim().is_empty() && cell.trim().replace(',', ".").replace([' ', '\u{a0}'], "").parse::<f64>().is_err();
                                    let mut te = TextEdit::singleline(cell).align(egui::Align2::RIGHT_CENTER);
                                    if bad {
                                        te = te.text_color(egui::Color32::from_rgb(200, 40, 40));
                                    }
                                    ui.add_sized([120.0, 20.0], te);
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
                if let Some(i) = del_cat {
                    dlg.remove_cat(i);
                }
                if let Some(k) = del_ser {
                    dlg.remove_series(k);
                }
                ui.horizontal(|ui| {
                    if ui.button(t("+ Kategori")).clicked() {
                        dlg.add_cat();
                    }
                    if ui.button(t("+ Serie")).clicked() {
                        dlg.add_series();
                    }
                });
                ui.label(RichText::new(t("Värden som inte är tal (röda) blir tomma.")).small().weak());
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(t("OK")).clicked() {
                        action = Some(true);
                    }
                    if ui.button(t("Avbryt")).clicked() {
                        action = Some(false);
                    }
                });
            });
        if !open {
            action = Some(false);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && action.is_none() {
            action = Some(false);
        }
        match action {
            Some(true) => {
                let Some(dlg) = self.edit.chart.take() else { return };
                let data = dlg.data();
                let sel = dlg.sel;
                let mut written = None;
                let ok = self.apply(None, |txn| {
                    written = Some(edit_chart::write(txn, sel.slide, sel.index, &data)?);
                    Ok(())
                });
                if ok {
                    if written.is_some_and(|w| w.workbook == Some(false)) {
                        self.edit.notices.push((t("Diagrammet är länkat till en extern arbetsbok: bara diagrammets egna värden ändrades.").to_owned(), false));
                    }
                }
            }
            Some(false) => self.edit.chart = None,
            None => {}
        }
    }
}
