//! Dokumentläget: formateringsrad, sidvy med stavningskontroll och statusrad.
//!
//! Allt här är en vy över samma Markdown-`String` som textvyn redigerar (se
//! `docs/dev/dokumentlage.md`): `TextEdit` med samma id, så markör och ångra överlever växlingen.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Color32,
    Align, Align2, CentralPanel, FontFamily, FontId, Frame, KeyboardShortcut, Layout, Margin,
    Modifiers, Key, Panel, Popup, Rect, RectAlign, ScrollArea, Sense, Shape, Stroke, TextBuffer, TextEdit, Ui,
    UiBuilder, Vec2, pos2, vec2,
};

use super::layout::{self, MARGIN_PT, PAGE_H_PT, PAGE_W_PT, px};
use super::markup::{self, Block, Inline, SpanProp};
use crate::app::{KladdApp, byte_to_char, char_to_byte, editor_id, menu_divider, menu_item, status_button};
use crate::icons::{self, GlyphStyle, Icon};
use crate::theme::Palette;

/// Storlekarna (pt) i storleksväljaren, som i Word.
const SIZES: [f32; 17] = [8.0, 9.0, 10.0, 10.5, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 36.0, 48.0, 72.0];
/// Words standardfärger för text (första raden) och en rad grå/temafärger.
const TEXT_COLORS: [[u32; 10]; 2] = [
    [0xC00000, 0xFF0000, 0xFFC000, 0xFFFF00, 0x92D050, 0x00B050, 0x00B0F0, 0x0070C0, 0x002060, 0x7030A0],
    [0x000000, 0x404040, 0x7F7F7F, 0xBFBFBF, 0xFFFFFF, 0x44546A, 0x4472C4, 0xED7D31, 0xA5A5A5, 0x70AD47],
];
const DEFAULT_TEXT_COLOR: [u8; 3] = [0xC0, 0x00, 0x00];
const DEFAULT_HIGHLIGHT: [u8; 3] = [0xFF, 0xFF, 0x00];
/// Markeringsfärgerna (Words `w:highlight`) med svenska namn.
const HIGHLIGHTS: [(&str, &str); 15] = [
    ("yellow", "Gul"),
    ("green", "Grön"),
    ("cyan", "Turkos"),
    ("magenta", "Rosa"),
    ("blue", "Blå"),
    ("red", "Röd"),
    ("darkBlue", "Mörkblå"),
    ("darkCyan", "Mörk turkos"),
    ("darkGreen", "Mörkgrön"),
    ("darkMagenta", "Violett"),
    ("darkRed", "Mörkröd"),
    ("darkYellow", "Mörkgul"),
    ("darkGray", "Mörkgrå"),
    ("lightGray", "Ljusgrå"),
    ("black", "Svart"),
];
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 3.0;
/// Luft mellan skrivbordets kant och sidan, och ovanför/under den.
const GAP: f32 = 24.0;

/// En bild som lästs in för dokumentläget.
struct ImgInfo {
    /// Naturlig storlek i px (utan zoom).
    size: Vec2,
}

struct SpellCache {
    doc_id: u64,
    text_hash: u64,
    generation: u64,
    ranges: Vec<Range<usize>>,
}

/// Ordet som högerklickades, med förslag (beräknade en gång när menyn öppnas).
#[derive(Clone)]
struct SpellTarget {
    range: Range<usize>,
    word: String,
    suggestions: Vec<String>,
}

enum SpellAct {
    Replace(Range<usize>, String),
    Ignore(String),
    Add(String),
}

#[derive(Default)]
pub struct State {
    images: HashMap<(u64, String), Option<ImgInfo>>,
    spell: Option<SpellCache>,
    target: Option<SpellTarget>,
    /// (aktuell sida, antal sidor) från senaste bildrutan.
    pages: (usize, usize),
    /// Senast valda text- och markeringsfärg (knapparna tillämpar dem direkt).
    text_color: Option<[u8; 3]>,
    highlight: Option<[u8; 3]>,
}

type Undoer = egui::util::undoer::Undoer<(CCursorRange, String)>;

enum BarAct {
    Inline(Inline),
    Block(Block),
    Image,
    Table,
    Undo,
    Redo,
    Zoom(f32),
    ZoomReset,
    ToMarkdown,
    Span(SpanProp),
    Align(markup::Align),
    TextColor(Option<[u8; 3]>),
    Highlight(Option<[u8; 3]>),
}

fn hash_text(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

fn block_label(b: Block) -> String {
    match b {
        Block::Normal => "Brödtext".into(),
        Block::Heading(n) => format!("Rubrik {n}"),
        Block::Quote => "Citat".into(),
        Block::Bullet => "Punktlista".into(),
        Block::Numbered => "Numrerad lista".into(),
        Block::Code => "Kod".into(),
        Block::Table => "Tabell".into(),
    }
}

fn fmt_size(pt: f32) -> String {
    if pt.fract() == 0.0 { format!("{pt}") } else { format!("{pt}").replace('.', ",") }
}

/// Nästa storlek i listan (`dir` = 1 större, -1 mindre) som i Words Ctrl+Shift+>/<.
fn step_size(cur: f32, dir: i32) -> f32 {
    if dir > 0 {
        SIZES.iter().copied().find(|s| *s > cur + 0.01).unwrap_or(cur + 10.0)
    } else {
        SIZES.iter().rev().copied().find(|s| *s < cur - 0.01).unwrap_or(SIZES[0])
    }
}

fn rgb_color(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Färgväljare i en meny: "ingen färg"-rad och rader med 16 px stora rutor. `Some(val)` när
/// något valts (`val` = `None` för den första raden).
fn color_popup(
    ui: &mut Ui,
    p: &Palette,
    none_label: &str,
    rows: &[Vec<(String, [u8; 3])>],
    current: Option<[u8; 3]>,
) -> Option<Option<[u8; 3]>> {
    let mut pick = None;
    ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
    if menu_item(ui, p, none_label, "", Some(current.is_none()), true) {
        pick = Some(None);
    }
    for row in rows {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            for (name, c) in row {
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
                let resp = resp.on_hover_text(name);
                ui.painter().rect_filled(r, 2.0, rgb_color(*c));
                ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
                if current == Some(*c) {
                    ui.painter().rect_stroke(r.expand(2.0), 3.0, Stroke::new(1.5, p.accent), egui::StrokeKind::Outside);
                } else if resp.hovered() {
                    ui.painter().rect_stroke(r.expand(1.0), 2.0, Stroke::new(1.0, p.muted), egui::StrokeKind::Outside);
                }
                if resp.clicked() {
                    pick = Some(Some(*c));
                    ui.close();
                }
            }
        });
    }
    ui.add_space(3.0);
    pick
}

fn divider(ui: &mut Ui, p: &Palette) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 22.0), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, p.line));
    ui.add_space(4.0);
}

/// Radgränserna (y i px från galleriets överkant) där en ny sida börjar. `rows` är (topp, botten)
/// för varje rad. Varje sida rymmer så många hela rader som får plats på `page_h`, minst en, och
/// brytlinjen läggs mellan sista raden och nästa, så den aldrig skär en rad.
fn page_breaks(rows: &[(f32, f32)], page_h: f32) -> Vec<f32> {
    let mut out = Vec::new();
    if page_h <= 0.0 || rows.is_empty() {
        return out;
    }
    let mut i = 0;
    while i < rows.len() {
        let limit = rows[i].0 + page_h + 0.01;
        let mut j = i;
        while j + 1 < rows.len() && rows[j + 1].1 <= limit {
            j += 1;
        }
        if j + 1 >= rows.len() {
            break;
        }
        out.push((rows[j].1 + rows[j + 1].0) / 2.0);
        i = j + 1;
    }
    out
}

fn overlaps_strict(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end || (a.start == a.end && b.start <= a.start && a.start < b.end)
}

impl KladdApp {
    /// Sant när den aktiva fliken är ett Word-dokument i dokumentläget.
    pub(crate) fn word_mode(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| d.word_view)
    }

    /// Markeringen i den aktiva flikens editor, som byteintervall.
    fn word_selection(&self, ctx: &egui::Context) -> Range<usize> {
        let Some(doc) = self.docs.get(self.active) else { return 0..0 };
        let state = TextEditState::load(ctx, editor_id(doc.id)).unwrap_or_default();
        match state.cursor.char_range() {
            Some(r) => {
                let r = r.as_sorted_char_range();
                char_to_byte(&doc.text, r.start.0)..char_to_byte(&doc.text, r.end.0)
            }
            None => doc.text.len()..doc.text.len(),
        }
    }

    /// Gemensam väg för all formatering: kör en markup-operation på markeringen och flyttar markören.
    pub(crate) fn word_edit(&mut self, ctx: &egui::Context, f: impl FnOnce(&str, Range<usize>) -> markup::Edit) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let end = doc.text.chars().count();
        let before = state.cursor.char_range().unwrap_or(CCursorRange::two(CCursor::new(end), CCursor::new(end)));
        let r = before.as_sorted_char_range();
        let sel = char_to_byte(&doc.text, r.start.0)..char_to_byte(&doc.text, r.end.0);
        let edit = f(&doc.text, sel);
        if edit.text != doc.text {
            // Spara läget före ändringen som ett eget ångra-steg.
            let mut undoer = state.undoer();
            undoer.add_undo(&(before, doc.text.clone()));
            state.set_undoer(undoer);
            doc.text = edit.text;
            doc.on_edited();
        }
        let s = byte_to_char(&doc.text, edit.sel.start);
        let e = byte_to_char(&doc.text, edit.sel.end);
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(s), CCursor::new(e))));
        state.store(ctx, id);
        self.focus_editor = true;
        self.find.current = None;
    }

    fn word_undo(&mut self, ctx: &egui::Context, redo: bool) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let cur_range = state.cursor.char_range().unwrap_or(CCursorRange::one(CCursor::new(0)));
        let mut undoer = state.undoer();
        let cur = (cur_range, doc.text.clone());
        let step = |u: &mut Undoer, c: &_| if redo { u.redo(c) } else { u.undo(c) }.cloned();
        let mut res = step(&mut undoer, &cur);
        // Samma text men annan markör räknas som ett eget steg; hoppa över det.
        if res.as_ref().is_some_and(|(_, t)| *t == cur.1) {
            res = step(&mut undoer, &cur);
        }
        state.set_undoer(undoer);
        if let Some((range, text)) = res {
            doc.text = text;
            doc.on_edited();
            state.cursor.set_char_range(Some(range));
            self.find.current = None;
        }
        state.store(ctx, id);
        self.focus_editor = true;
    }

    fn word_toggle(&mut self, ctx: &egui::Context, m: Inline) {
        self.word_edit(ctx, |t, s| markup::toggle_inline(t, s, m));
    }

    /// Större/mindre text ett steg i storlekslistan.
    fn word_font_step(&mut self, ctx: &egui::Context, dir: i32) {
        self.word_edit(ctx, |t, s| {
            let a = markup::analyze(t);
            let act = markup::active(t, &a, s.clone());
            let cur = act.span.font_size.unwrap_or(a.defaults.size());
            let next = step_size(cur, dir);
            let prop = if (next - a.defaults.size()).abs() < 0.01 { None } else { Some(next) };
            markup::set_span_prop(t, s, SpanProp::FontSize(prop))
        });
    }

    fn word_block(&mut self, ctx: &egui::Context, b: Block) {
        self.word_edit(ctx, |t, s| markup::set_block(t, s, b));
    }

    pub(crate) fn word_zoom_step(&mut self, dir: f32) {
        let z = (self.settings.word_zoom * 10.0).round() / 10.0 + dir * 0.1;
        self.settings.word_zoom = ((z * 100.0).round() / 100.0).clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// Kortkommandon som bara gäller i dokumentläget (konsumeras före TextEdit).
    pub(crate) fn word_shortcuts(&mut self, ctx: &egui::Context) {
        if let Some(doc) = self.docs.get(self.active) {
            // Skriver man i t.ex. sökrutan ska Ctrl+B inte formatera dokumentet.
            if ctx.memory(|m| m.focused()).is_some_and(|f| f != editor_id(doc.id)) {
                return;
            }
        }
        let hit = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let ctrl = Modifiers::CTRL;
        let ctrl_alt = Modifiers::CTRL | Modifiers::ALT;
        if hit(ctrl, Key::B) {
            self.word_toggle(ctx, Inline::Bold);
        }
        if hit(ctrl, Key::I) {
            self.word_toggle(ctx, Inline::Italic);
        }
        if hit(ctrl, Key::U) {
            self.word_toggle(ctx, Inline::Underline);
        }
        for (k, a) in [
            (Key::L, markup::Align::Left),
            (Key::E, markup::Align::Center),
            (Key::R, markup::Align::Right),
            (Key::J, markup::Align::Justify),
        ] {
            if hit(ctrl, k) {
                self.word_edit(ctx, |t, s| markup::set_align(t, s, a));
            }
        }
        // Teckenstorlek ett steg: Words Ctrl+Shift+> / < (på svenskt tangentbord Ctrl+Shift+<
        // som ger `Comma`/`Period` i egui) samt Ctrl+] / Ctrl+[.
        if hit(ctrl | Modifiers::SHIFT, Key::Period) || hit(ctrl, Key::CloseBracket) {
            self.word_font_step(ctx, 1);
        }
        if hit(ctrl | Modifiers::SHIFT, Key::Comma) || hit(ctrl, Key::OpenBracket) {
            self.word_font_step(ctx, -1);
        }
        for (k, b) in [
            (Key::Num1, Block::Heading(1)),
            (Key::Num2, Block::Heading(2)),
            (Key::Num3, Block::Heading(3)),
            (Key::Num0, Block::Normal),
        ] {
            if hit(ctrl_alt, k) {
                self.word_block(ctx, b);
            }
        }
    }

    /// Formateringsraden under flikraden.
    pub(crate) fn word_bar(&mut self, ui: &mut Ui, p: &Palette) {
        if !self.word_mode() {
            return;
        }
        let ctx = ui.ctx().clone();
        if self.speller.is_none() {
            self.speller = Some(crate::spell::Speller::new(self.settings.spell_lang.as_deref()));
        }
        let sel = self.word_selection(&ctx);
        let doc = &self.docs[self.active];
        let analysis = self.word.analysis(doc.id, &doc.text);
        let act = markup::active(&doc.text, &analysis, sel);
        let zoom_pct = (self.settings.word_zoom * 100.0).round() as i32;

        let mut acts: Vec<BarAct> = Vec::new();
        // Typsnitt i dokumentet (grundtypsnittet först), och de som är installerade.
        let default_font = analysis.defaults.font().to_owned();
        let default_size = analysis.defaults.size();
        let mut used_fonts = vec![default_font.clone()];
        for pc in &analysis.pieces {
            if let Some(f) = pc.span.as_ref().and_then(|s| s.font_family.as_ref()) {
                if !used_fonts.iter().any(|u| u.eq_ignore_ascii_case(f)) {
                    used_fonts.push(f.clone());
                }
            }
        }
        used_fonts[1..].sort_by_key(|f| f.to_lowercase());
        let cur_font = act.span.font_family.clone().unwrap_or_else(|| default_font.clone());
        let cur_size = act.span.font_size.unwrap_or(default_size);
        let text_color = self.wv.text_color.unwrap_or(DEFAULT_TEXT_COLOR);
        let highlight = self.wv.highlight.unwrap_or(DEFAULT_HIGHLIGHT);
        let panel = Panel::top("kladd-wordbar")
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin::symmetric(12, 6)))
            .show(ui, |ui| {
                let full = ui.max_rect();
                let right_w = 200.0;
                let left_w = (full.width() - right_w - 8.0).max(200.0);
                let right = Rect::from_min_size(pos2(full.right() - right_w, full.top()), vec2(right_w, 28.0));
                ui.set_min_height(28.0);

                // Vänster del: bryts till en andra rad när fönstret är smalt.
                ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    // Stil
                    egui::ComboBox::from_id_salt("kladd-word-style")
                        .width(130.0)
                        .selected_text(block_label(act.block))
                        .show_ui(ui, |ui| {
                            for b in [
                                Block::Normal,
                                Block::Heading(1),
                                Block::Heading(2),
                                Block::Heading(3),
                                Block::Quote,
                            ] {
                                if ui.selectable_label(act.block == b, block_label(b)).clicked() {
                                    acts.push(BarAct::Block(b));
                                }
                            }
                        });
                    // Typsnitt och storlek
                    egui::ComboBox::from_id_salt("kladd-word-font")
                        .width(150.0)
                        .selected_text(cur_font.clone())
                        .show_ui(ui, |ui| {
                            let pick = |f: &str| {
                                BarAct::Span(SpanProp::FontFamily(
                                    (!f.eq_ignore_ascii_case(&default_font)).then(|| f.to_owned()),
                                ))
                            };
                            for f in &used_fonts {
                                if ui.selectable_label(f.eq_ignore_ascii_case(&cur_font), f).clicked() {
                                    acts.push(pick(f));
                                }
                            }
                            ui.separator();
                            for f in self.doc_fonts.installed() {
                                if used_fonts.iter().any(|u| u.eq_ignore_ascii_case(f)) {
                                    continue;
                                }
                                if ui.selectable_label(false, f).clicked() {
                                    acts.push(pick(f));
                                }
                            }
                        });
                    egui::ComboBox::from_id_salt("kladd-word-size")
                        .width(56.0)
                        .selected_text(fmt_size(cur_size))
                        .show_ui(ui, |ui| {
                            for sz in SIZES {
                                if ui.selectable_label((sz - cur_size).abs() < 0.01, fmt_size(sz)).clicked() {
                                    let v = ((sz - default_size).abs() > 0.01).then_some(sz);
                                    acts.push(BarAct::Span(SpanProp::FontSize(v)));
                                }
                            }
                        });
                    // Tecken
                    for (glyph, style, on, tip, m) in [
                        ("F", GlyphStyle::Bold, act.bold, "Fetstil (Ctrl+B)", Inline::Bold),
                        ("K", GlyphStyle::Italic, act.italic, "Kursiv (Ctrl+I)", Inline::Italic),
                        ("U", GlyphStyle::Underline, act.underline, "Understruken (Ctrl+U)", Inline::Underline),
                        ("S", GlyphStyle::Strike, act.strike, "Genomstruken", Inline::Strike),
                    ] {
                        if icons::glyph_toggle(ui, p, glyph, style, on, tip).clicked() {
                            acts.push(BarAct::Inline(m));
                        }
                    }
                    divider(ui, p);
                    // Färger
                    let (main, arrow) = icons::color_button(
                        ui,
                        p,
                        Icon::TextColor,
                        rgb_color(text_color),
                        "Textfärg",
                        "Välj textfärg",
                    );
                    if main.clicked() {
                        acts.push(BarAct::Span(SpanProp::Color(Some(text_color))));
                    }
                    Popup::menu(&arrow).gap(4.0).width(214.0).show(|ui| {
                        let rows: Vec<Vec<(String, [u8; 3])>> = TEXT_COLORS
                            .iter()
                            .map(|r| {
                                r.iter()
                                    .map(|c| {
                                        let rgb = [(c >> 16) as u8, (c >> 8) as u8, *c as u8];
                                        (crate::style::hex(rgb), rgb)
                                    })
                                    .collect()
                            })
                            .collect();
                        if let Some(c) = color_popup(ui, p, "Automatisk", &rows, act.span.color) {
                            acts.push(BarAct::Span(SpanProp::Color(c)));
                            acts.push(BarAct::TextColor(c));
                        }
                    });
                    let (main, arrow) = icons::color_button(
                        ui,
                        p,
                        Icon::Highlight,
                        rgb_color(highlight),
                        "Markera text",
                        "Välj markeringsfärg",
                    );
                    if main.clicked() {
                        acts.push(BarAct::Span(SpanProp::Background(Some(highlight))));
                    }
                    Popup::menu(&arrow).gap(4.0).width(214.0).show(|ui| {
                        let all: Vec<(String, [u8; 3])> = HIGHLIGHTS
                            .iter()
                            .filter_map(|(id, name)| {
                                let rgb = crate::style::highlight_rgb(id)?;
                                Some((format!("{name} ({})", crate::style::hex(rgb)), rgb))
                            })
                            .collect();
                        let rows = vec![all[..8].to_vec(), all[8..].to_vec()];
                        if let Some(c) = color_popup(ui, p, "Ingen färg", &rows, act.span.background) {
                            acts.push(BarAct::Span(SpanProp::Background(c)));
                            acts.push(BarAct::Highlight(c));
                        }
                    });
                    divider(ui, p);
                    // Justering
                    for (icon, al, tip) in [
                        (Icon::AlignLeft, markup::Align::Left, "Vänsterjustera (Ctrl+L)"),
                        (Icon::AlignCenter, markup::Align::Center, "Centrera (Ctrl+E)"),
                        (Icon::AlignRight, markup::Align::Right, "Högerjustera (Ctrl+R)"),
                        (Icon::AlignJustify, markup::Align::Justify, "Marginaljustera (Ctrl+J)"),
                    ] {
                        if icons::toggle_button(ui, p, icon, act.align == al, tip).clicked() {
                            acts.push(BarAct::Align(al));
                        }
                    }
                    divider(ui, p);
                    // Listor och citat
                    for (icon, b, tip) in [
                        (Icon::BulletList, Block::Bullet, "Punktlista"),
                        (Icon::NumberedList, Block::Numbered, "Numrerad lista"),
                        (Icon::Quote, Block::Quote, "Citat"),
                    ] {
                        let on = act.block == b;
                        if icons::toggle_button(ui, p, icon, on, tip).clicked() {
                            acts.push(BarAct::Block(if on { Block::Normal } else { b }));
                        }
                    }
                    divider(ui, p);
                    if icons::button(ui, p, Icon::Image, 28.0, false, "Infoga bild", p.chrome).clicked() {
                        acts.push(BarAct::Image);
                    }
                    if icons::button(ui, p, Icon::Table, 28.0, false, "Infoga tabell", p.chrome).clicked() {
                        acts.push(BarAct::Table);
                    }
                    divider(ui, p);
                    if icons::button(ui, p, Icon::Undo, 28.0, false, "Ångra (Ctrl+Z)", p.chrome).clicked() {
                        acts.push(BarAct::Undo);
                    }
                    if icons::button(ui, p, Icon::Redo, 28.0, false, "Gör om (Ctrl+Y)", p.chrome).clicked() {
                        acts.push(BarAct::Redo);
                    }
                });

                ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if status_button(ui, p, "Markdown", "Visa dokumentet som Markdown-text").clicked() {
                        acts.push(BarAct::ToMarkdown);
                    }
                    divider(ui, p);
                    if icons::button(ui, p, Icon::Plus, 28.0, false, "Zooma in (Ctrl+plus)", p.chrome).clicked() {
                        acts.push(BarAct::Zoom(1.0));
                    }
                    if status_button(ui, p, &format!("{zoom_pct} %"), "Klicka för 100 %").clicked() {
                        acts.push(BarAct::ZoomReset);
                    }
                    if icons::button(ui, p, Icon::Minus, 28.0, false, "Zooma ut (Ctrl+minus)", p.chrome).clicked() {
                        acts.push(BarAct::Zoom(-1.0));
                    }
                });
            });
        let bar = panel.response.rect;
        ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, p.line));

        for a in acts {
            match a {
                BarAct::Inline(m) => self.word_toggle(&ctx, m),
                BarAct::Block(b) => self.word_block(&ctx, b),
                BarAct::Image => self.request_insert_image(&ctx),
                BarAct::Table => self.word_edit(&ctx, |t, s| markup::insert_table(t, s.end)),
                BarAct::Undo => self.word_undo(&ctx, false),
                BarAct::Redo => self.word_undo(&ctx, true),
                BarAct::Span(prop) => self.word_edit(&ctx, |t, s| markup::set_span_prop(t, s, prop)),
                BarAct::Align(al) => self.word_edit(&ctx, |t, s| markup::set_align(t, s, al)),
                BarAct::TextColor(c) => self.wv.text_color = c.or(self.wv.text_color),
                BarAct::Highlight(c) => self.wv.highlight = c.or(self.wv.highlight),
                BarAct::Zoom(d) => self.word_zoom_step(d),
                BarAct::ZoomReset => self.settings.word_zoom = 1.0,
                BarAct::ToMarkdown => {
                    if let Some(d) = self.docs.get_mut(self.active) {
                        d.word_view = false;
                    }
                    self.focus_editor = true;
                }
            }
        }
    }

    /// Läser in en bild (en gång per dokument och källa) och registrerar bytes för ritning.
    fn word_image(&mut self, ctx: &egui::Context, src: &str) {
        let doc = &self.docs[self.active];
        let key = (doc.id, src.to_owned());
        if self.wv.images.contains_key(&key) {
            return;
        }
        let base = doc.base_dir().unwrap_or_default();
        let info = crate::doc::resolve_image(src, &base, &doc.images).and_then(|img| {
            let size = match img.extent {
                Some((cx, cy)) if cx > 0 && cy > 0 => vec2(cx as f32 / 9525.0, cy as f32 / 9525.0),
                _ => {
                    let s = imagesize::blob_size(&img.bytes).ok()?;
                    vec2(s.width as f32, s.height as f32)
                }
            };
            ctx.include_bytes(image_uri(src, doc.id), img.bytes);
            Some(ImgInfo { size })
        });
        let known = info.is_some();
        self.wv.images.insert(key, info);
        if known {
            // Layoutcachen nycklar inte på bildstorlekar.
            self.word.forget(self.docs[self.active].id);
        }
    }

    /// Sidvyn: grått skrivbord, en vit A4-sida med text, sidbrytningar och stavningsmarkering.
    pub(crate) fn word_page(&mut self, ui: &mut Ui, p: &Palette) {
        CentralPanel::default().frame(Frame::NONE.fill(p.desk)).show(ui, |ui| {
            self.editor_rect = ui.max_rect();
            let ctx = ui.ctx().clone();
            if self.speller.is_none() {
                self.speller = Some(crate::spell::Speller::new(self.settings.spell_lang.as_deref()));
            }

            // Ctrl + rullhjul zoomar sidan.
            if ctx.input(|i| i.zoom_delta()) != 1.0 && ui.rect_contains_pointer(ui.max_rect()) {
                let zd = ctx.input(|i| i.zoom_delta());
                self.settings.word_zoom = (self.settings.word_zoom * zd).clamp(MIN_ZOOM, MAX_ZOOM);
            }

            let avail_w = ui.available_width();
            let mut z = self.settings.word_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
            if avail_w < px(z, PAGE_W_PT) + 2.0 * GAP {
                z = ((avail_w - 2.0 * GAP) / px(1.0, PAGE_W_PT)).max(0.4);
            }
            let page_w = px(z, PAGE_W_PT);
            let margin = px(z, MARGIN_PT);
            let text_w = page_w - 2.0 * margin;
            let text_h_page = px(z, PAGE_H_PT - 2.0 * MARGIN_PT);
            let area_w = avail_w.max(page_w + 2.0 * GAP);

            let doc_id = self.docs[self.active].id;
            let id = editor_id(doc_id);
            let analysis = self.word.analysis(doc_id, &self.docs[self.active].text);

            // Bilder: läs in och räkna ut visningsstorlek vid aktuell zoom.
            let srcs: Vec<String> = analysis
                .pieces
                .iter()
                .filter_map(|pc| pc.image.as_ref().map(|i| i.src.clone()))
                .collect();
            let mut sizes: HashMap<String, Vec2> = HashMap::new();
            for src in srcs {
                self.word_image(&ctx, &src);
                if let Some(Some(info)) = self.wv.images.get(&(doc_id, src.clone())) {
                    let max_w = px(1.0, PAGE_W_PT - 2.0 * MARGIN_PT);
                    let k = (max_w / info.size.x).min(1.0);
                    sizes.insert(src, info.size * k * z);
                }
            }
            let image_size = |src: &str| sizes.get(src).copied();
            // Textbredd för tabellkolumner och listindrag. Mäts före och utanför layout_job, så
            // ingen fonts-lås hålls när det anropas.
            let measure = |s: &str, f: FontId| -> f32 {
                ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x)
            };

            // Stavningskontroll.
            let text_hash = hash_text(&self.docs[self.active].text);
            let spell_on = self.settings.spell;
            let mut misspelled: Vec<Range<usize>> = Vec::new();
            if spell_on {
                if let Some(sp) = self.speller.as_mut() {
                    if sp.ready() {
                        let gen_ = sp.generation();
                        let fresh = self
                            .wv
                            .spell
                            .as_ref()
                            .is_some_and(|c| c.doc_id == doc_id && c.text_hash == text_hash && c.generation == gen_);
                        if !fresh {
                            let text = &self.docs[self.active].text;
                            let mut ranges: Vec<Range<usize>> = Vec::new();
                            for pc in &analysis.pieces {
                                if pc.markup
                                    || pc.code
                                    || pc.link
                                    || pc.image.is_some()
                                    || matches!(pc.block, Block::Code | Block::Table)
                                {
                                    continue;
                                }
                                match ranges.last_mut() {
                                    Some(last) if last.end == pc.range.start => last.end = pc.range.end,
                                    _ => ranges.push(pc.range.clone()),
                                }
                            }
                            let found = sp.misspelled(text, &ranges);
                            self.wv.spell = Some(SpellCache { doc_id, text_hash, generation: gen_, ranges: found });
                        }
                        misspelled = self.wv.spell.as_ref().map(|c| c.ranges.clone()).unwrap_or_default();
                    } else {
                        ctx.request_repaint_after(std::time::Duration::from_millis(120));
                    }
                }
            }

            // Markupen visas bara på markörens rader när editorn har fokus.
            let focused = ctx.memory(|m| m.has_focus(id));
            let sel_now = self.word_selection(&ctx);
            let reveal = if focused { markup::reveal_range(&self.docs[self.active].text, sel_now.clone()) } else { 0..0 };
            // Står markören i en tabell visas hela tabellen som rå Markdown.
            let tabs = layout::tables(&self.docs[self.active].text, &analysis);
            let reveal = layout::reveal_whole_tables(&tabs, reveal);

            let (query, case_sensitive) = if self.find.open {
                (self.find.query.clone(), self.find.case_sensitive)
            } else {
                (String::new(), false)
            };
            let current_match = self.find.current;
            let scroll_to = self.scroll_to_char.take();
            let focus = std::mem::take(&mut self.focus_editor);
            let secondary_target = self.wv.target.clone();

            let doc = &mut self.docs[self.active];
            let word = &mut self.word;
            let speller = &mut self.speller;
            let mut edited = false;
            let mut spell_act: Option<SpellAct> = None;
            let mut new_target: Option<Option<SpellTarget>> = None;
            let mut pages = (1usize, 1usize);

            ScrollArea::both().id_salt(("kladd-wscroll", doc_id)).auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let origin = ui.cursor().min;
                let page_left = origin.x + (area_w - page_w) / 2.0;
                let page_top = origin.y + GAP;
                let shadow_slot = ui.painter().add(Shape::Noop);
                let page_slot = ui.painter().add(Shape::Noop);

                let text_rect = Rect::from_min_size(
                    pos2(page_left + margin, page_top + margin),
                    vec2(text_w, f32::INFINITY),
                );
                let params = layout::Params {
                    palette: p,
                    zoom: z,
                    reveal: reveal.clone(),
                    query: &query,
                    case_sensitive,
                    current_match,
                    image_size: &image_size,
                    measure: &measure,
                };
                let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, wrap_width: f32| {
                    let mut job = word.job(doc_id, buf.as_str(), &params);
                    job.wrap.max_width = wrap_width;
                    ui.ctx().fonts_mut(|f| f.layout_job(job))
                };
                let min_h = (px(z, PAGE_H_PT) - 2.0 * margin).max(0.0);
                let out = ui
                    .scope_builder(UiBuilder::new().max_rect(text_rect), |ui| {
                        TextEdit::multiline(&mut doc.text)
                            .id(id)
                            .frame(Frame::NONE)
                            .margin(Margin::ZERO)
                            .desired_width(text_w)
                            .min_size(vec2(text_w, min_h))
                            .lock_focus(true)
                            .cursor_at_end(false)
                            .layouter(&mut layouter)
                            .show(ui)
                    })
                    .inner;

                if out.response.changed() {
                    doc.on_edited();
                    edited = true;
                }
                if focus {
                    ui.memory_mut(|m| m.request_focus(id));
                }

                let galley = out.galley.clone();
                let gpos = out.galley_pos;
                let te_rect = out.response.rect;
                let text = doc.text.as_str();

                // Sidan och skuggan, i höjd med texten.
                let page_h = px(z, PAGE_H_PT).max(2.0 * margin + te_rect.height());
                let page_rect = Rect::from_min_size(pos2(page_left, page_top), vec2(page_w, page_h));
                ui.painter().set(
                    shadow_slot,
                    Shape::rect_filled(page_rect.translate(vec2(0.0, 2.0)).expand(1.5), 2.0, p.page_shadow),
                );
                ui.painter().set(page_slot, Shape::rect_filled(page_rect, 1.0, p.page));
                ui.allocate_rect(
                    Rect::from_min_max(origin, pos2(origin.x + area_w, page_rect.bottom() + GAP)),
                    Sense::hover(),
                );

                let clip = ui.clip_rect();
                let painter = ui.painter().clone();

                // Byte -> teckenindex, framåt i en svep (bitarna ligger i ordning).
                let mut conv = ByteToChar::new(text);

                // Punkter för dolda listmarkörer, citatstreck och bilder.
                let mut bar: Option<(f32, f32)> = None;
                let flush_bar = |bar: &mut Option<(f32, f32)>| {
                    if let Some((y0, y1)) = bar.take() {
                        let x = gpos.x - px(z, 5.0);
                        painter.rect_filled(
                            Rect::from_min_max(pos2(x - 3.0, y0), pos2(x, y1)),
                            0.0,
                            p.page_markup,
                        );
                    }
                };
                for pc in &analysis.pieces {
                    let shown = overlaps(&pc.range, &reveal);
                    if pc.list_marker && pc.markup && !shown && !edited {
                        // Punkten ritas 12 pt före textens början (markören efter dold markör).
                        let ci = conv.at(pc.range.end);
                        let r = galley.pos_from_cursor(CCursor::new(ci)).translate(gpos.to_vec2());
                        if r.bottom() >= clip.top() && r.top() <= clip.bottom() {
                            let level = layout::list_level(text, pc.range.start);
                            let c = pos2(r.left() - px(z, 12.0), r.center().y + 0.5);
                            let rad = (2.6 * z).max(1.8);
                            match level {
                                0 => {
                                    painter.circle_filled(c, rad, p.page_text);
                                }
                                1 => {
                                    painter.circle_stroke(c, rad - 0.4, Stroke::new(1.2, p.page_text));
                                }
                                _ => {
                                    painter.rect_filled(Rect::from_center_size(c, Vec2::splat(rad * 1.6)), 0.0, p.page_text);
                                }
                            }
                        }
                    }
                    if pc.block == Block::Quote && !shown && pc.range.end > pc.range.start {
                        let a = conv.at(pc.range.start);
                        let last = if text[..pc.range.end].ends_with('\n') && pc.range.end - 1 > pc.range.start {
                            pc.range.end - 1
                        } else {
                            pc.range.end
                        };
                        let b = conv.at(last);
                        let ra = galley.pos_from_cursor(CCursor::new(a)).translate(gpos.to_vec2());
                        let rb = galley.pos_from_cursor(CCursor::new(b)).translate(gpos.to_vec2());
                        let (y0, y1) = (ra.top(), rb.bottom());
                        bar = match bar {
                            Some((c0, c1)) if y0 <= c1 + 1.0 => Some((c0, c1.max(y1))),
                            other => {
                                flush_bar(&mut { other });
                                Some((y0, y1))
                            }
                        };
                    }
                    if let (Some(img), false) = (&pc.image, shown) {
                        if let Some(sz) = sizes.get(&img.src) {
                            let ci = conv.at(pc.range.start);
                            let r = galley.pos_from_cursor(CCursor::new(ci)).translate(gpos.to_vec2());
                            {
                                let uri = image_uri(&img.src, doc_id);
                                let rect = Rect::from_min_size(r.min, *sz);
                                if rect.bottom() >= clip.top() && rect.top() <= clip.bottom() {
                                    egui::Image::new(uri).paint_at(ui, rect);
                                }
                            }
                        }
                    }
                }
                flush_bar(&mut bar);

                // Tabellrutnät: 1 px kantlinjer vid de dolda pipornas lägen och radgränserna.
                if !edited {
                    let stroke = Stroke::new(1.0, p.page_text.gamma_multiply(0.6));
                    let snap = |v: f32| (v - 0.5).round() + 0.5;
                    let mut conv = ByteToChar::new(text);
                    for t in tabs.iter().filter(|t| !overlaps_strict(&t.region, &reveal)) {
                        for (ri, row) in t.rows.iter().enumerate() {
                            let (Some(&first), Some(&last)) = (row.pipes.first(), row.pipes.last()) else { continue };
                            if row.delimiter || first == last {
                                continue;
                            }
                            let at = |conv: &mut ByteToChar, b: usize| {
                                galley.pos_from_cursor(CCursor::new(conv.at(b))).translate(gpos.to_vec2())
                            };
                            let rs = at(&mut conv, row.line.start);
                            let re = at(&mut conv, row.line.end);
                            let (y0, y1) = (rs.top(), re.bottom());
                            if y1 < clip.top() || y0 > clip.bottom() {
                                continue;
                            }
                            let xs: Vec<f32> = row.pipes.iter().map(|&b| snap(at(&mut conv, b).left())).collect();
                            for &x in &xs {
                                painter.line_segment([pos2(x, snap(y0)), pos2(x, snap(y1))], stroke);
                            }
                            let (x0, x1) = (xs[0], xs[xs.len() - 1]);
                            if ri == 0 {
                                painter.line_segment([pos2(x0, snap(y0)), pos2(x1, snap(y0))], stroke);
                            }
                            painter.line_segment([pos2(x0, snap(y1)), pos2(x1, snap(y1))], stroke);
                        }
                    }
                }

                // Sidbrytningar (ungefärliga): vid radgränser, aldrig genom en rad.
                let top_of_text = gpos.y;
                let rows: Vec<(f32, f32)> = galley.rows.iter().map(|r| (r.min_y(), r.max_y())).collect();
                let breaks = page_breaks(&rows, text_h_page);
                let total = breaks.len() + 1;
                let right_x = page_left + page_w - margin + 8.0;
                for (k, by) in breaks.iter().enumerate() {
                    let y = top_of_text + by;
                    if y >= clip.top() - 20.0 && y <= clip.bottom() + 20.0 {
                        painter.extend(Shape::dashed_line(
                            &[pos2(page_left, y), pos2(page_left + page_w, y)],
                            Stroke::new(1.0, p.page_markup),
                            5.0,
                            4.0,
                        ));
                        painter.text(
                            pos2(right_x, y),
                            Align2::LEFT_CENTER,
                            format!("Sida {}", k + 2),
                            FontId::new(10.0, FontFamily::Proportional),
                            p.page_muted,
                        );
                    }
                }
                let caret_ci = out.state.cursor.char_range().map_or(0, |r| r.primary.index.0);
                let caret_y = galley.pos_from_cursor(CCursor::new(caret_ci)).min.y;
                let cur = breaks.iter().filter(|b| **b <= caret_y + 0.5).count() + 1;
                pages = (cur.min(total), total);

                // Röd våglinje under felstavade ord.
                if !misspelled.is_empty() {
                    let caret_b = char_to_byte(text, caret_ci);
                    let mut conv = ByteToChar::new(text);
                    let amp = 1.2 * z.max(0.8);
                    let half = 2.0 * z.max(0.8);
                    for r in &misspelled {
                        if focused && r.start <= caret_b && caret_b <= r.end {
                            continue;
                        }
                        let a = conv.at(r.start);
                        let b = conv.at(r.end);
                        let ra = galley.pos_from_cursor(CCursor::new(a)).translate(gpos.to_vec2());
                        if ra.bottom() < clip.top() || ra.top() > clip.bottom() {
                            continue;
                        }
                        let rb = galley.pos_from_cursor(CCursor::new(b)).translate(gpos.to_vec2());
                        if (ra.top() - rb.top()).abs() > 1.0 {
                            continue; // ordet bryts över två rader
                        }
                        let y = ra.bottom() - 1.0 + amp;
                        let mut pts = Vec::new();
                        let mut x = ra.left();
                        let mut up = false;
                        while x < rb.left() {
                            pts.push(pos2(x, if up { y - amp } else { y + amp }));
                            x += half;
                            up = !up;
                        }
                        pts.push(pos2(rb.left(), if up { y - amp } else { y + amp }));
                        if pts.len() >= 2 {
                            painter.add(Shape::line(pts, Stroke::new(1.0, p.error)));
                        }
                    }
                }

                // Högerklick på ett felstavat ord.
                if out.response.secondary_clicked() {
                    let mut found = None;
                    if let Some(pos) = ui.ctx().pointer_interact_pos() {
                        let ci = galley.cursor_from_pos(pos - gpos).index.0;
                        let b = char_to_byte(text, ci);
                        if let Some(r) = misspelled.iter().find(|r| r.start <= b && b < r.end) {
                            let word = text[r.clone()].to_owned();
                            let suggestions = speller.as_ref().map(|s| s.suggest(&word)).unwrap_or_default();
                            found = Some(SpellTarget { range: r.clone(), word, suggestions });
                        }
                    }
                    new_target = Some(found);
                }
                let target = match &new_target {
                    Some(t) => t.clone(),
                    None => secondary_target.clone(),
                };
                if let Some(t) = target {
                    out.response.context_menu(|ui| {
                        ui.set_min_width(210.0);
                        ui.set_max_width(260.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if t.suggestions.is_empty() {
                            ui.add_enabled(false, egui::Label::new("Inga förslag"));
                        }
                        for s in t.suggestions.iter().take(6) {
                            if menu_item(ui, p, s, "", None, true) {
                                spell_act = Some(SpellAct::Replace(t.range.clone(), s.clone()));
                            }
                        }
                        menu_divider(ui, p);
                        if menu_item(ui, p, "Ignorera alla", "", None, true) {
                            spell_act = Some(SpellAct::Ignore(t.word.clone()));
                        }
                        if menu_item(ui, p, "Lägg till i ordlistan", "", None, true) {
                            spell_act = Some(SpellAct::Add(t.word.clone()));
                        }
                    });
                }

                if let Some(ci) = scroll_to {
                    let r = galley.pos_from_cursor(CCursor::new(ci)).translate(gpos.to_vec2());
                    ui.scroll_to_rect(r.expand2(vec2(80.0, 60.0)), Some(Align::Center));
                }
            });

            self.wv.pages = pages;
            if let Some(t) = new_target {
                self.wv.target = t;
            }
            if edited {
                self.find.current = None;
            }
            match spell_act {
                Some(SpellAct::Replace(r, s)) => {
                    self.word_edit(&ctx, |t, _| {
                        let mut text = String::with_capacity(t.len() + s.len());
                        text.push_str(&t[..r.start]);
                        text.push_str(&s);
                        text.push_str(&t[r.end..]);
                        let end = r.start + s.len();
                        markup::Edit { text, sel: end..end }
                    });
                }
                Some(SpellAct::Ignore(w)) => {
                    if let Some(sp) = self.speller.as_mut() {
                        sp.ignore_all(&w);
                    }
                }
                Some(SpellAct::Add(w)) => {
                    if let Some(sp) = self.speller.as_mut() {
                        sp.add_to_dictionary(&w);
                    }
                }
                None => {}
            }
        });
    }

    /// Statusradens högra del i dokumentläget (läggs ut från höger till vänster).
    pub(crate) fn word_status(&mut self, ui: &mut Ui, p: &Palette, font: &FontId) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let analysis = self.word.analysis(doc.id, &doc.text);
        let (words, _) = markup::count(&doc.text, &analysis);
        let (cur, total) = self.wv.pages;
        let zoom_pct = (self.settings.word_zoom * 100.0).round() as i32;
        let label = |ui: &mut Ui, s: String| {
            ui.label(egui::RichText::new(s).font(font.clone()).color(p.muted));
        };
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            label(ui, format!("{zoom_pct} %"));
            if let Some(sp) = self.speller.as_mut() {
                if sp.languages().is_empty() {
                    ui.label(egui::RichText::new("Ingen ordlista").font(font.clone()).color(p.muted))
                        .on_hover_text("Installera paketet hunspell-sv för svensk stavningskontroll");
                } else {
                    let name = sp.lang().map_or_else(|| "Ingen ordlista".to_owned(), |l| l.name.clone());
                    let btn = status_button(ui, p, &name, "Språk för stavningskontroll");
                    let langs: Vec<(String, String)> =
                        sp.languages().iter().map(|l| (l.code.clone(), l.name.clone())).collect();
                    let current = sp.lang().map(|l| l.code.clone());
                    let mut pick: Option<String> = None;
                    let mut spell = self.settings.spell;
                    Popup::menu(&btn).align(RectAlign::TOP_END).gap(6.0).width(220.0).show(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if menu_item(ui, p, "Stavningskontroll", "", Some(spell), true) {
                            spell = !spell;
                        }
                        menu_divider(ui, p);
                        for (code, name) in &langs {
                            if menu_item(ui, p, name, "", Some(current.as_deref() == Some(code.as_str())), true) {
                                pick = Some(code.clone());
                            }
                        }
                    });
                    self.settings.spell = spell;
                    if let Some(code) = pick {
                        sp.set_lang(&code);
                        self.settings.spell_lang = Some(code);
                    }
                }
            }
            label(ui, if words == 1 { "1 ord".to_owned() } else { format!("{words} ord") });
            label(ui, format!("Sida {cur} av {total}"));
        });
    }
}

/// Byteoffset till teckenindex, effektivt för stigande offset.
struct ByteToChar<'a> {
    text: &'a str,
    b: usize,
    c: usize,
}

impl<'a> ByteToChar<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, b: 0, c: 0 }
    }

    fn at(&mut self, b: usize) -> usize {
        let b = b.min(self.text.len());
        if b < self.b {
            self.b = 0;
            self.c = 0;
        }
        self.c += self.text[self.b..b].chars().count();
        self.b = b;
        self.c
    }
}

fn image_uri(src: &str, doc_id: u64) -> String {
    format!("bytes://kladd-doc/{doc_id}/{}", src.replace(' ', "%20"))
}

#[cfg(test)]
mod tests {
    use super::{page_breaks, step_size};

    #[test]
    fn sidbrytning_mellan_rader() {
        // Rader på 20 px; sidan rymmer 100 px = 5 rader.
        let rows: Vec<(f32, f32)> = (0..12).map(|i| (i as f32 * 20.0, i as f32 * 20.0 + 20.0)).collect();
        let b = page_breaks(&rows, 100.0);
        assert_eq!(b, vec![100.0, 200.0]);
        // Aldrig genom en rad.
        for y in &b {
            assert!(rows.iter().all(|r| !(r.0 < *y && *y < r.1)), "{y}");
        }
        // Sidan rymmer inte en hel rad till: brytningen hamnar före den.
        let b = page_breaks(&rows, 110.0);
        assert_eq!(b[0], 100.0);
        // En rad högre än sidan får en egen sida.
        let tall = [(0.0, 20.0), (20.0, 400.0), (400.0, 420.0)];
        assert_eq!(page_breaks(&tall, 100.0).len(), 2);
        assert!(page_breaks(&rows[..5], 100.0).is_empty());
        assert!(page_breaks(&[], 100.0).is_empty());
    }

    #[test]
    fn size_steps_follow_the_list() {
        assert_eq!(step_size(11.0, 1), 12.0);
        assert_eq!(step_size(10.0, 1), 10.5);
        assert_eq!(step_size(11.0, -1), 10.5);
        assert_eq!(step_size(8.0, -1), 8.0);
        assert_eq!(step_size(72.0, 1), 82.0);
        assert_eq!(step_size(13.0, 1), 14.0);
    }
}
