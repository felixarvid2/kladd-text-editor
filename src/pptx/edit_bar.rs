//! Formatraden i redigeringsläget: teckenstorlek (mindre/större och ett fält), textfärg (temats
//! färger och några vanliga), justering och punktlista. Raden syns när en form med text är markerad
//! eller man skriver i den. Formatet gäller markeringen i texten (utan markering ordet vid
//! markören) eller, när bara formen är markerad, all text i den. Själva ändringarna görs i
//! `edit_format.rs`.

use egui::{Align, Color32, Frame, Id, Key, Layout, Margin, Panel, Popup, Stroke, TextEdit, Ui};

use super::edit_format::TextColor;
use super::edit_ops::Kind;
use super::edit_text::{Carets, ParaFmt, Pos, TextModel};
use super::edit_view::Sel;
use super::inherit::Sheets;
use super::text::Align as TAlign;
use super::view::PptxTab;
use crate::i18n::t;
use crate::icons::{self, Icon};
use crate::pdfview::chrome::{BAR_H, G, bar_divider, tool_btn};
use crate::theme::Palette;

/// Formatet vid markören (eller i första körningen när bara formen är markerad).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    pub size: f32,
    pub color: [u8; 3],
    pub align: TAlign,
    pub bullet: bool,
}

impl Format {
    /// Formatet vid läget: körningen som markören står i eller efter.
    pub fn at(c: &Carets, at: Pos) -> Option<Format> {
        let p = c.paras.get(at.para.min(c.paras.len().checked_sub(1)?))?;
        let r = p.runs.iter().find(|r| r.0 < at.ch && at.ch <= r.1).or_else(|| p.runs.iter().find(|r| r.0 <= at.ch && at.ch < r.1)).or(p.runs.last())?;
        Some(Format { size: r.2, color: r.3, align: p.align, bullet: p.bullet })
    }
}

/// Formatradens tillstånd.
#[derive(Default)]
pub struct FormatBar {
    /// Texten i storleksfältet.
    size_buf: String,
    /// Senast vald färg (knappen använder den igen).
    last: Option<(TextColor, [u8; 3])>,
    /// Formen som inte skrivs i: (presentationens adress, form) → format.
    cache: Option<(usize, Sel, Option<Format>)>,
}

/// Temats färger (namn, schemanamn) i PowerPoints ordning, och vanliga färger.
const THEME: &[(&str, &str)] = &[("Text 1", "tx1"), ("Bakgrund 1", "bg1"), ("Text 2", "tx2"), ("Bakgrund 2", "bg2"), ("Dekorfärg 1", "accent1"), ("Dekorfärg 2", "accent2"), ("Dekorfärg 3", "accent3"), ("Dekorfärg 4", "accent4"), ("Dekorfärg 5", "accent5"), ("Dekorfärg 6", "accent6")];
const STANDARD: &[(&str, [u8; 3])] = &[
    ("Mörkröd", [0xC0, 0x00, 0x00]),
    ("Röd", [0xFF, 0x00, 0x00]),
    ("Orange", [0xFF, 0xC0, 0x00]),
    ("Gul", [0xFF, 0xFF, 0x00]),
    ("Ljusgrön", [0x92, 0xD0, 0x50]),
    ("Grön", [0x00, 0xB0, 0x50]),
    ("Ljusblå", [0x00, 0xB0, 0xF0]),
    ("Blå", [0x00, 0x70, 0xC0]),
    ("Mörkblå", [0x00, 0x20, 0x60]),
    ("Lila", [0x70, 0x30, 0xA0]),
];

/// Formatet i storleksfältet: heltal utan decimaler, annars med decimalkomma.
fn size_text(pt: f32) -> String {
    let s = format!("{:.1}", (pt * 10.0).round() / 10.0);
    s.trim_end_matches('0').trim_end_matches('.').replace('.', ",")
}

impl PptxTab {
    /// Tillämpar `f` på texten: markeringen när man skriver, annars all text i den markerade formen.
    /// `f` får modellen, intervallet och styckenas ärvda format och ger om något ändrades.
    pub(super) fn format_text(&mut self, f: &mut dyn FnMut(&mut TextModel, Pos, Pos, &[ParaFmt]) -> bool) {
        self.edit.session.break_merge();
        if let Some(ts) = &mut self.edit.text {
            let fmts = ts.carets.as_ref().map(|c| c.paras.clone()).unwrap_or_default();
            let (a, b) = (ts.anchor, ts.caret);
            if f(&mut ts.model, a, b, &fmts) {
                self.commit_text();
            }
            self.edit.session.break_merge();
        } else {
            // Alla markerade former med text (en eller flera).
            for sel in self.selection() {
                self.begin_text(sel, None);
                let Some(ts) = &mut self.edit.text else { continue };
                let fmts = ts.carets.as_ref().map(|c| c.paras.clone()).unwrap_or_default();
                let end = ts.model.end();
                if f(&mut ts.model, Pos::default(), end, &fmts) {
                    self.commit_text();
                }
                self.end_text();
            }
        }
    }

    /// Formatet som raden visar.
    fn format_now(&mut self) -> Option<Format> {
        let ok = |me: &mut Self, sel: Sel| me.shapes_on(me.current).iter().any(|s| s.index == sel.index && s.id == sel.id && s.kind == Kind::Shape && !s.alternate);
        if let Some(ts) = &self.edit.text {
            return ts.carets.as_ref().and_then(|c| Format::at(c, ts.caret));
        }
        let sel = self.edit.sel.filter(|s| s.slide == self.current)?;
        if !ok(self, sel) || !self.edit.more.is_empty() {
            return None;
        }
        let key = std::sync::Arc::as_ptr(&self.deck) as usize;
        if let Some((k, s, f)) = &self.edit.fmt.cache {
            if *k == key && *s == sel {
                return *f;
            }
        }
        let f = self.text_of(sel).and_then(|(_, c)| c).and_then(|c| Format::at(&c, Pos::default()));
        self.edit.fmt.cache = Some((key, sel, f));
        f
    }

    /// Temafärgerna på bilden som (namn, färg, RGB).
    fn color_entries(&self) -> Vec<(String, TextColor, [u8; 3])> {
        let sheets = Sheets::for_slide(&self.deck, self.current);
        let mut out = Vec::new();
        for (name, scheme) in THEME {
            if let Some(c) = sheets.colors.scheme_color(scheme) {
                let [r, g, b, _] = c.to_u8();
                out.push((t(name).to_owned(), TextColor::Scheme(scheme), [r, g, b]));
            }
        }
        for (name, rgb) in STANDARD {
            out.push((t(name).to_owned(), TextColor::Rgb(*rgb), *rgb));
        }
        out
    }

    pub(super) fn apply_size(&mut self, pt: f32) {
        self.format_text(&mut |m, a, b, _| m.set_size(a, b, pt));
    }

    pub(super) fn apply_size_step(&mut self, up: bool) {
        self.format_text(&mut |m, a, b, fmts| m.step_size(a, b, up, fmts));
    }

    pub(super) fn apply_align(&mut self, algn: &'static str) {
        self.format_text(&mut |m, a, b, _| {
            m.set_align(a, b, algn);
            true
        });
    }

    pub(super) fn apply_bullet(&mut self, on: bool) {
        self.format_text(&mut |m, a, b, _| {
            m.set_bullet(a, b, on);
            true
        });
    }

    pub(super) fn apply_color(&mut self, c: Option<TextColor>) {
        self.format_text(&mut |m, a, b, _| m.set_color(a, b, c));
    }

    /// Formatraden under verktygsraden. Den står kvar hela redigeringsläget (så att bilden inte hoppar när
    /// markeringen ändras) och visar formatet när en form med text är markerad.
    pub(super) fn format_bar(&mut self, ui: &mut Ui, p: &Palette) {
        if !self.editing() {
            return;
        }
        let fmt = self.format_now();
        let overlay = self.edit_focus_id();
        let entries = self.color_entries();
        let mut act: Vec<Act> = Vec::new();
        Panel::top("kladd-pptx-format")
            .exact_size(BAR_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin::symmetric(12, 5)))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().hline((r.left() - 12.0)..=(r.right() + 12.0), r.bottom() + 5.0, Stroke::new(1.0, p.line));
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    // Storlek.
                    if tool_btn(ui, p, G::Std(Icon::Minus), false, true, t("Mindre text (Ctrl+Skift+<)")).clicked() {
                        act.push(Act::Step(false));
                    }
                    let id = Id::unique("kladd-pptx-size");
                    if !ui.ctx().memory(|m| m.has_focus(id)) {
                        self.edit.fmt.size_buf = fmt.map(|f| size_text(f.size)).unwrap_or_default();
                    }
                    let field = ui.add(TextEdit::singleline(&mut self.edit.fmt.size_buf).id(id).desired_width(40.0).align(egui::Align2::CENTER_CENTER).margin(Margin::symmetric(4, 4)));
                    field.clone().on_hover_text(t("Teckenstorlek (punkter)"));
                    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        if let Ok(v) = self.edit.fmt.size_buf.trim().replace(',', ".").parse::<f32>() {
                            if v >= 1.0 {
                                act.push(Act::Size(v.min(400.0)));
                            }
                        }
                        if let Some(o) = overlay {
                            ui.memory_mut(|m| m.request_focus(o));
                        }
                    }
                    if tool_btn(ui, p, G::Std(Icon::Plus), false, true, t("Större text (Ctrl+Skift+>)")).clicked() {
                        act.push(Act::Step(true));
                    }
                    bar_divider(ui, p);
                    // Färg.
                    let cur = self.edit.fmt.last.map_or_else(|| entries.iter().find(|e| e.1 == TextColor::Scheme("accent1")).map_or([0xC0, 0, 0], |e| e.2), |l| l.1);
                    let swatch = Color32::from_rgb(cur[0], cur[1], cur[2]);
                    let (main, arrow) = icons::color_button(ui, p, Icon::TextColor, swatch, t("Textfärg"), t("Välj textfärg"));
                    if main.clicked() {
                        let last = self.edit.fmt.last.map(|l| l.0).unwrap_or(TextColor::Scheme("accent1"));
                        act.push(Act::Color(Some(last)));
                    }
                    Popup::menu(&arrow).gap(4.0).width(214.0).show(|ui| {
                        let theme: Vec<(String, [u8; 3])> = entries.iter().filter(|e| matches!(e.1, TextColor::Scheme(_))).map(|e| (e.0.clone(), e.2)).collect();
                        let std: Vec<(String, [u8; 3])> = entries.iter().filter(|e| matches!(e.1, TextColor::Rgb(_))).map(|e| (e.0.clone(), e.2)).collect();
                        let rows = vec![theme, std];
                        match crate::word::view::color_popup(ui, p, t("Automatisk"), &rows, fmt.map(|f| f.color)) {
                            Some(None) => act.push(Act::Color(None)),
                            Some(Some(rgb)) => {
                                if let Some(e) = entries.iter().find(|e| e.2 == rgb) {
                                    act.push(Act::Color(Some(e.1)));
                                }
                            }
                            None => {}
                        }
                    });
                    bar_divider(ui, p);
                    // Justering.
                    for (icon, al, algn, tip) in [
                        (Icon::AlignLeft, TAlign::Left, "l", t("Vänsterjustera (Ctrl+L)")),
                        (Icon::AlignCenter, TAlign::Center, "ctr", t("Centrera (Ctrl+E)")),
                        (Icon::AlignRight, TAlign::Right, "r", t("Högerjustera (Ctrl+R)")),
                        (Icon::AlignJustify, TAlign::Justify, "just", t("Marginaljustera (Ctrl+J)")),
                    ] {
                        if icons::toggle_button(ui, p, icon, fmt.is_some_and(|f| f.align == al), tip).clicked() {
                            act.push(Act::Align(algn));
                        }
                    }
                    bar_divider(ui, p);
                    let bul = fmt.is_some_and(|f| f.bullet);
                    if icons::toggle_button(ui, p, Icon::BulletList, bul, t("Punktlista")).clicked() {
                        act.push(Act::Bullet(!bul));
                    }
                });
            });
        for a in act {
            match a {
                Act::Size(v) => self.apply_size(v),
                Act::Step(up) => self.apply_size_step(up),
                Act::Align(a) => self.apply_align(a),
                Act::Bullet(on) => self.apply_bullet(on),
                Act::Color(c) => {
                    if let Some(c) = c {
                        let rgb = entries.iter().find(|e| e.1 == c).map(|e| e.2).unwrap_or([0, 0, 0]);
                        self.edit.fmt.last = Some((c, rgb));
                    }
                    self.apply_color(c);
                }
            }
        }
    }
}

enum Act {
    Size(f32),
    Step(bool),
    Align(&'static str),
    Bullet(bool),
    Color(Option<TextColor>),
}
