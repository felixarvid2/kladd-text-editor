//! Dokumentläget: formateringsrad, sidvy med stavningskontroll och statusrad.
//!
//! Allt här är en vy över samma Markdown-`String` som textvyn redigerar (se
//! `docs/dev/dokumentlage.md`): `TextEdit` med samma id, så markör och ångra överlever växlingen.

use std::collections::HashMap;
use std::sync::Arc;
use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Color32, CursorIcon, Id,
    Align, CentralPanel, FontFamily, FontId, Frame, KeyboardShortcut, Layout, Margin,
    Modifiers, Key, Panel, Popup, Rect, RectAlign, ScrollArea, Sense, Shape, Stroke, TextBuffer, TextEdit, Ui,
    UiBuilder, Vec2, pos2, vec2,
};

use super::grid;
use super::images;
use super::layout::{self, px};
use super::pages;
use super::markup::{self, Block, Inline, SpanProp};
use super::nav;
use super::tables::{self, TableOp};
use crate::app::{KladdApp, byte_to_char, char_to_byte, editor_id, menu_divider, menu_item, status_button};
use crate::icons::{self, GlyphStyle, Icon};
use crate::theme::Palette;

/// Storlekarna (pt) i storleksväljaren, som i Word.
const SIZES: [f32; 17] = [8.0, 9.0, 10.0, 10.5, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 26.0, 28.0, 36.0, 48.0, 72.0];
/// Words standardfärger för text (första raden) och en rad grå/temafärger.
pub(crate) const TEXT_COLORS: [[u32; 10]; 2] = [
    [0xC00000, 0xFF0000, 0xFFC000, 0xFFFF00, 0x92D050, 0x00B050, 0x00B0F0, 0x0070C0, 0x002060, 0x7030A0],
    [0x000000, 0x404040, 0x7F7F7F, 0xBFBFBF, 0xFFFFFF, 0x44546A, 0x4472C4, 0xED7D31, 0xA5A5A5, 0x70AD47],
];
const DEFAULT_TEXT_COLOR: [u8; 3] = [0xC0, 0x00, 0x00];
const DEFAULT_HIGHLIGHT: [u8; 3] = [0xFF, 0xFF, 0x00];
/// Markeringsfärgerna (Words `w:highlight`) med svenska namn.
pub(crate) const HIGHLIGHTS: [(&str, &str); 15] = [
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
    /// Dokumentrevisionen resultatet gäller.
    rev: u64,
    generation: u64,
    ranges: Arc<Vec<Range<usize>>>,
    /// Felstavade ord per textstycke (hash av stycket -> lägen relativt styckets början), så att
    /// en ändring bara kontrollerar de stycken som ändrats.
    per_range: HashMap<u64, Vec<Range<usize>>>,
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
    /// Vald bild, pågående storleksändring.
    img: images::State,
    target: Option<SpellTarget>,
    /// (aktuell sida, antal sidor) från senaste bildrutan.
    pages: (usize, usize),
    /// Öppen dialog "Sidinställningar".
    pub(crate) page_dlg: Option<pages::PageDialog>,
    /// Senast valda text- och markeringsfärg (knapparna tillämpar dem direkt).
    text_color: Option<[u8; 3]>,
    highlight: Option<[u8; 3]>,
    /// Fotnotshopp som väntar på att musknappen släpps (layouten ändras när markören flyttas vid nedtryckning).
    fn_pending: Option<usize>,
}

type Undoer = egui::util::undoer::Undoer<(CCursorRange, String)>;

enum BarAct {
    Inline(Inline),
    Block(Block),
    Image,
    Table,
    Footnote,
    Toc,
    TableOp(TableOp),
    PageSetup,
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

fn block_label(b: Block) -> String {
    match b {
        Block::Normal => "Brödtext".into(),
        Block::Heading(n) => format!("Rubrik {n}"),
        Block::Quote => "Citat".into(),
        Block::Bullet => "Punktlista".into(),
        Block::Numbered => "Numrerad lista".into(),
        Block::Code => "Kod".into(),
        Block::Table => "Tabell".into(),
        Block::Footnote => "Fotnot".into(),
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
pub(crate) fn color_popup(
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
    ui.add_space(3.0);
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 22.0), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, p.line));
    ui.add_space(3.0);
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
    pub(crate) fn word_selection(&self, ctx: &egui::Context) -> Range<usize> {
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
        let _ = (s, e);
        state.cursor.set_char_range(Some(CCursorRange::two(
            tables::cursor_at(&doc.text, edit.sel.start),
            tables::cursor_at(&doc.text, edit.sel.end),
        )));
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

    /// Infogar en fotnot vid markören och flyttar markören till den nya definitionen.
    pub(crate) fn word_footnote(&mut self, ctx: &egui::Context) {
        self.word_edit(ctx, |t, s| markup::insert_footnote(t, s.end));
        let b = self.word_selection(ctx).start;
        if let Some(doc) = self.docs.get(self.active) {
            self.scroll_to_char = Some(byte_to_char(&doc.text, b));
        }
    }

    /// Backspace direkt efter (eller Delete direkt före) en fotnotsreferens tar bort hela fotnoten,
    /// både referensen och definitionen; annars skulle en trasig `[^1` lämna en föräldralös definition.
    fn word_footnote_keys(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if !doc.text.contains("[^") {
            return;
        }
        let (back, del) = ctx.input(|i| {
            (i.key_pressed(Key::Backspace) && i.modifiers.is_none(), i.key_pressed(Key::Delete) && i.modifiers.is_none())
        });
        if !back && !del {
            return;
        }
        let sel = self.word_selection(ctx);
        if sel.start != sel.end {
            return;
        }
        let hit = markup::footnotes(&doc.text)
            .refs
            .into_iter()
            .find(|r| if back { r.full.end == sel.start } else { r.full.start == sel.start });
        let Some(r) = hit else { return };
        ctx.input_mut(|i| i.consume_key(Modifiers::NONE, if back { Key::Backspace } else { Key::Delete }));
        self.word_edit(ctx, |t, _| {
            let text = markup::delete_footnote(t, &r.label);
            markup::Edit { text, sel: r.full.start..r.full.start }
        });
    }

    /// Lazy omnumrering: efter en ändring som gör att fotnoternas etiketter inte längre är
    /// 1..n i ordning (klistrad referens, raderad referens …) numreras de om.
    fn word_renumber_footnotes(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if !doc.text.contains("[^") {
            return;
        }
        let a = self.word.analysis(doc.id, &doc.text);
        if markup::footnotes_in_order(&a) {
            return;
        }
        self.word_edit(ctx, |t, s| {
            let (text, c) = markup::renumber_footnotes_at(t, s.end);
            markup::Edit { text, sel: c..c }
        });
    }

    fn word_block(&mut self, ctx: &egui::Context, b: Block) {
        self.word_edit(ctx, |t, s| markup::set_block(t, s, b));
    }

    pub(crate) fn word_zoom_step(&mut self, dir: f32) {
        let z = (self.settings.word_zoom * 10.0).round() / 10.0 + dir * 0.1;
        self.settings.word_zoom = ((z * 100.0).round() / 100.0).clamp(MIN_ZOOM, MAX_ZOOM);
    }

    /// Ctrl+V: har urklippet HTML (Word, LibreOffice, Google Dokument, webben) görs den om till
    /// Kladds format och sätts in; annars (och vid Ctrl+Skift+V) klistrar egui in vanlig text.
    fn word_paste_html(&mut self, ctx: &egui::Context) {
        // Ctrl+V ger en `Paste`-händelse bara om urklippet också har vanlig text; knapptrycket
        // fångar även ett urklipp med enbart HTML.
        let is_paste = |e: &egui::Event| {
            matches!(e, egui::Event::Paste(_))
                || matches!(e, egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command)
        };
        let (paste, shift) = ctx.input(|i| (i.events.iter().any(is_paste), i.modifiers.shift));
        if !paste || shift {
            return;
        }
        let Some(html) = crate::paste::clipboard_html() else { return };
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let defaults = markup::analyze(&doc.text).defaults;
        let mut pasted = crate::paste::html_to_markdown(&html, Some(&defaults));
        if pasted.markdown.trim().is_empty() {
            return;
        }
        ctx.input_mut(|i| i.events.retain(|e| !is_paste(e)));
        crate::paste::make_names_unique(&mut pasted, &doc.images);
        doc.images.append(&mut pasted.images);
        let md = pasted.markdown;
        self.word_edit(ctx, |t, s| crate::paste::insert(t, s, &md));
    }

    /// Kortkommandon som bara gäller i dokumentläget (konsumeras före TextEdit).
    pub(crate) fn word_shortcuts(&mut self, ctx: &egui::Context) {
        if let Some(doc) = self.docs.get(self.active) {
            // Skriver man i t.ex. sökrutan ska Ctrl+B inte formatera dokumentet.
            if ctx.memory(|m| m.focused()).is_some_and(|f| f != editor_id(doc.id)) {
                return;
            }
        }
        self.word_table_keys(ctx);
        self.word_footnote_keys(ctx);
        self.word_paste_html(ctx);
        // egui:s `consume_shortcut` bryr sig inte om Shift. Utan kontrollen skulle Ctrl+L
        // (vänsterjustera) ta Ctrl+Shift+L (byt tema) och Ctrl+I ta Ctrl+Shift+I (infoga bild).
        let shift = ctx.input(|i| i.modifiers.shift);
        let hit = |m: Modifiers, k: Key| {
            (m.shift || !shift) && ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
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
        if hit(ctrl_alt, Key::F) {
            self.word_footnote(ctx);
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
        let in_table = markup::table_at(&doc.text, sel.start).is_some();
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
                let right_w = 218.0;
                let left_w = (full.width() - right_w - 8.0).max(200.0);
                let right = Rect::from_min_size(pos2(full.right() - right_w, full.top()), vec2(right_w, 28.0));
                ui.set_min_height(28.0);

                // Vänster del: bryts till en andra rad när fönstret är smalt.
                ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 4.0);
                    // Stil
                    egui::ComboBox::from_id_salt("kladd-word-style")
                        .width(98.0)
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
                        .width(112.0)
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
                        .width(50.0)
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
                    if icons::button(ui, p, Icon::Footnote, 28.0, false, "Infoga fotnot (Ctrl+Alt+F)", p.chrome).clicked() {
                        acts.push(BarAct::Footnote);
                    }
                    if icons::button(ui, p, Icon::Toc, 28.0, false, "Infoga innehållsförteckning", p.chrome).clicked() {
                        acts.push(BarAct::Toc);
                    }
                    if in_table {
                        divider(ui, p);
                        if let Some(op) = tables::bar_group(ui, p) {
                            acts.push(BarAct::TableOp(op));
                        }
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
                    if icons::button(ui, p, Icon::Page, 28.0, false, "Sidinställningar", p.chrome).clicked() {
                        acts.push(BarAct::PageSetup);
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
                BarAct::Toc => self.insert_toc(&ctx),
                BarAct::TableOp(op) => self.word_table_op(&ctx, op),
                BarAct::Table => self.word_edit(&ctx, |t, s| markup::insert_table(t, s.end)),
                BarAct::Footnote => self.word_footnote(&ctx),
                BarAct::PageSetup => self.open_page_dialog(),
                BarAct::Undo => self.word_undo(&ctx, false),
                BarAct::Redo => self.word_undo(&ctx, true),
                BarAct::Span(prop) => self.word_edit(&ctx, |t, s| markup::set_span_prop(t, s, prop)),
                BarAct::Align(al) => {
                    // En vald bild justeras som sitt stycke, oavsett var markören står.
                    let img = self.wv.img.range.clone();
                    self.word_edit(&ctx, |t, s| markup::set_align(t, img.unwrap_or(s), al))
                }
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

    /// Utför en åtgärd på den valda bilden (`r` är bildbitens område i texten).
    fn word_image_act(&mut self, ctx: &egui::Context, act: images::Act, r: Range<usize>) {
        use images::Act;
        match act {
            Act::Align(al) => self.word_edit(ctx, |t, _| markup::set_align(t, r, al)),
            Act::Original => self.word_edit(ctx, |t, _| markup::set_image_size(t, r, None)),
            Act::Resize { w, h: None } => self.word_edit(ctx, |t, _| markup::set_image_size(t, r, Some(w))),
            Act::Resize { w, h: Some(h) } => self.word_edit(ctx, |t, _| images::set_free_size(t, r, w, h)),
            Act::Float(f) => self.word_edit(ctx, |t, _| markup::set_image_float(t, r, f)),
            Act::Move { target, after } => {
                // Ett ångra-steg; bilden förblir vald på sin nya plats.
                let mut at = None;
                self.word_edit(ctx, |t, _| match markup::move_image(t, r.clone(), target, after) {
                    Some(e) => {
                        at = Some(e.sel.start);
                        e
                    }
                    None => markup::Edit { text: t.to_string(), sel: r.clone() },
                });
                if let (Some(at), Some(doc)) = (at, self.docs.get(self.active)) {
                    let a = markup::analyze(&doc.text);
                    let idx = a.pieces.iter().filter(|pc| pc.image.is_some() && pc.range.start < at).count();
                    self.wv.img.selected = Some(idx);
                }
            }
            Act::Delete => {
                self.word_edit(ctx, |t, _| images::remove_image(t, r));
                self.wv.img.deselect();
            }
        }
    }

    /// Sidvyn: grått skrivbord, vita ark (en per sida) med text, sidhuvud/sidfot och stavningsmarkering.
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

            let doc_id = self.docs[self.active].id;
            let id = editor_id(doc_id);
            let analysis = self.word.analysis(doc_id, &self.docs[self.active].text);

            // Sidans mått kommer från dokumentets grundformat; zoomen anpassas så att sidan ryms.
            let avail_w = ui.available_width();
            let mut z = self.settings.word_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
            let geom1 = pages::Geom::new(1.0, &analysis.defaults);
            if avail_w < geom1.page_w * z + 2.0 * GAP {
                z = ((avail_w - 2.0 * GAP) / geom1.page_w).max(0.4);
            }
            let geom = pages::Geom::new(z, &analysis.defaults);
            let page_w = geom.page_w;
            let text_w = geom.text_w();
            let area_w = avail_w.max(page_w + 2.0 * GAP);

            // Bilder: läs in och räkna ut visningsstorlek vid aktuell zoom.
            let refs = self.word.images(doc_id, &self.docs[self.active].text);
            let n_images = refs.len();
            let mut sizes: HashMap<String, Vec2> = HashMap::new();
            for img in refs.iter().cloned() {
                self.word_image(&ctx, &img.src);
                if let Some(Some(info)) = self.wv.images.get(&(doc_id, img.src.clone())) {
                    let max_w = geom1.text_w();
                    sizes.insert(images::size_key(&img), images::display_size(info.size, &img, max_w, z));
                }
            }
            let image_size = |src: &str| sizes.get(src).copied();
            // Textbredd för tabellkolumner och listindrag. Mäts före och utanför layout_job, så
            // ingen fonts-lås hålls när det anropas.
            let measure = |s: &str, f: FontId| -> f32 {
                ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x)
            };

            // Stavningskontroll: en gång per ändring, och bara för stycken som ändrats.
            let rev = self.word.rev(doc_id, &self.docs[self.active].text);
            let spell_on = self.settings.spell;
            let mut misspelled: Arc<Vec<Range<usize>>> = Arc::default();
            if spell_on {
                if let Some(sp) = self.speller.as_mut() {
                    if sp.ready() {
                        let gen_ = sp.generation();
                        let fresh = self
                            .wv
                            .spell
                            .as_ref()
                            .is_some_and(|c| c.doc_id == doc_id && c.rev == rev && c.generation == gen_);
                        if !fresh {
                            let text = &self.docs[self.active].text;
                            let mut spans: Vec<Range<usize>> = Vec::new();
                            for pc in &analysis.pieces {
                                if pc.markup
                                    || pc.code
                                    || pc.link
                                    || pc.image.is_some()
                                    || matches!(pc.block, Block::Code | Block::Table)
                                {
                                    continue;
                                }
                                match spans.last_mut() {
                                    Some(last) if last.end == pc.range.start => last.end = pc.range.end,
                                    _ => spans.push(pc.range.clone()),
                                }
                            }
                            let mut per_range = match self.wv.spell.take() {
                                Some(c) if c.doc_id == doc_id && c.generation == gen_ => c.per_range,
                                _ => HashMap::new(),
                            };
                            let found = sp.misspelled_cached(text, &spans, &mut per_range);
                            self.wv.spell =
                                Some(SpellCache { doc_id, rev, generation: gen_, ranges: Arc::new(found), per_range });
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
            let reveal = if focused { tables::reveal_range(&self.docs[self.active].text, sel_now.clone()) } else { 0..0 };
            let heads = self.word.headings(doc_id, &self.docs[self.active].text);
            let mut toc_click: Option<usize> = None;

            let matches = self.search_highlights();
            let scroll_to = self.scroll_to_char.take();
            // En vald bild tar fokus från texten (annars skulle Delete/Backspace gå dit).
            let focus = std::mem::take(&mut self.focus_editor) && self.wv.img.selected.is_none();
            let mut imgs = std::mem::take(&mut self.wv.img);
            let mut img_act: Option<images::Act> = None;
            let mut painted: Vec<images::Painted> = Vec::new();
            let mut img_index = 0usize;
            let secondary_target = self.wv.target.clone();

            // Typsnitten som dokumentet använder registreras hos egui före layouten, som sedan
            // bara slår upp dem (en okänd familj kraschar egui).
            let mut fam_map: HashMap<(String, bool, bool), FontFamily> = HashMap::new();
            let needed = self.word.fonts(doc_id, &self.docs[self.active].text);
            for (name, bold, italic) in needed.iter().cloned() {
                let fam = self.doc_fonts.family(&ctx, Some(&name), bold, italic);
                fam_map.insert((name, bold, italic), fam);
            }
            let font_family = |name: Option<&str>, bold: bool, italic: bool| -> FontFamily {
                name.and_then(|n| fam_map.get(&(n.to_owned(), bold, italic)))
                    .cloned()
                    .unwrap_or_else(|| crate::theme::doc_font(bold, italic))
            };
            let fonts_gen = self.doc_fonts.generation();

            let doc = &mut self.docs[self.active];
            let word = &mut self.word;
            let speller = &mut self.speller;
            let mut edited = false;
            let mut spell_act: Option<SpellAct> = None;
            let mut new_target: Option<Option<SpellTarget>> = None;
            let mut table_menu: Option<bool> = None;
            let mut table_act: Option<TableOp> = None;
            let mut col_resize: Option<(usize, Vec<usize>)> = None;
            let mut page_pos = (1usize, 1usize);
            let mut open_page_dlg = false;
            let mut fn_jump: Option<usize> = None;
            let mut fn_pending = self.wv.fn_pending.take();

            ScrollArea::both().id_salt(("kladd-wscroll", doc_id)).auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let origin = ui.cursor().min;
                let page_left = origin.x + (area_w - page_w) / 2.0;
                let page_top = origin.y + GAP;
                let shadow_slot = ui.painter().add(Shape::Noop);
                let page_slot = ui.painter().add(Shape::Noop);

                let text_rect = Rect::from_min_size(
                    pos2(page_left + geom.left, page_top + geom.top),
                    vec2(text_w, f32::INFINITY),
                );
                let params = layout::Params {
                    palette: p,
                    zoom: z,
                    reveal: reveal.clone(),
                    matches: &matches,
                    image_size: &image_size,
                    measure: &measure,
                    font_family: &font_family,
                    fonts_gen,
                };
                let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, wrap_width: f32| {
                    word.galley_paged(ui.ctx(), doc_id, buf.as_str(), &params, wrap_width, Some(&geom))
                };
                let min_h = geom.text_h();
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
                // Tabeller: kolumnkanter att dra i, pekarens träffyta och markören i cellens innehåll.
                let tview = if edited { None } else { word.table_view(doc_id, text) };
                let mut vtabs: Vec<VisTable> = Vec::new();
                let mut drag_preview: Option<(f32, f32, f32)> = None;
                let mut on_edge = false;
                if let Some((_, geoms)) = &tview {
                    let idx = grid::GalleyIndex::new(text, &galley);
                    let clip = ui.clip_rect();
                    let pad = grid::cell_pad(z);
                    for (ti, t) in geoms.iter().enumerate() {
                        if t.rows.is_empty() {
                            continue;
                        }
                        let first = grid::row_extent(&galley, &idx, t, 0);
                        let last = grid::row_extent(&galley, &idx, t, t.rows.len() - 1);
                        if gpos.y + last.1 < clip.top() - 4.0 || gpos.y + first.0 > clip.bottom() + 4.0 {
                            continue;
                        }
                        let exts: Vec<(f32, f32)> = (0..t.rows.len()).map(|r| grid::row_extent(&galley, &idx, t, r)).collect();
                        let (top, bottom) = (gpos.y + first.0, gpos.y + last.1);
                        for j in 1..t.cols() {
                            let x = gpos.x + t.col_x[j];
                            let strip = Rect::from_min_max(pos2(x - 3.0, top), pos2(x + 3.0, bottom));
                            let r = ui.interact(strip, Id::unique(("kladd-tblcol", doc_id, ti, j)), Sense::click_and_drag());
                            if r.hovered() || r.dragged() {
                                ui.ctx().set_cursor_icon(CursorIcon::ResizeColumn);
                                on_edge = true;
                            }
                            let drag_key = Id::unique("kladd-tblcol-drag");
                            if r.drag_started() {
                                let widths: Vec<f32> = (0..t.cols()).map(|k| t.col_x[k + 1] - t.col_x[k]).collect();
                                // Där knappen trycktes ned (draget kan ha hunnit flytta pekaren före första bildrutan).
                                let x0 = ui.ctx().input(|i| i.pointer.press_origin()).or(r.interact_pointer_pos()).map_or(x, |p| p.x);
                                ui.ctx().data_mut(|d| d.insert_temp(drag_key, (t.region.start, j, widths, x0)));
                            }
                            if r.dragged() || r.drag_stopped() {
                                let st = ui.ctx().data(|d| d.get_temp::<(usize, usize, Vec<f32>, f32)>(drag_key));
                                if let (Some((region, col, widths, x0)), Some(ptr)) = (st, r.interact_pointer_pos()) {
                                    let min_w = 2.0 * pad + 12.0;
                                    let dx = (ptr.x - x0).clamp(min_w - widths[col - 1], widths[col] - min_w);
                                    if r.dragged() {
                                        drag_preview = Some((x + dx, top, bottom));
                                    }
                                    if r.drag_stopped() {
                                        let mut w = widths.clone();
                                        w[col - 1] += dx;
                                        w[col] -= dx;
                                        let w: Vec<f64> = w.iter().map(|&v| f64::from(v)).collect();
                                        col_resize = Some((region, markup::dashes_from_widths(&w)));
                                    }
                                }
                                if r.drag_stopped() {
                                    ui.ctx().data_mut(|d| d.remove::<(usize, usize, Vec<f32>, f32)>(drag_key));
                                }
                            }
                        }
                        vtabs.push(VisTable { ti, exts });
                    }
                    tables::pointer_fix(ui.ctx(), id, text, &galley, gpos, geoms, &out.response, on_edge);
                }
                // Håller markören i tabellcellens innehåll (klick på dolda pipor och utfyllnad).
                tables::fix_caret(ui.ctx(), id, text);

                // Fotnoter: skiljelinje över definitionerna; numret är en länk mellan referens och definition.
                if text.contains("[^") {
                    if let Some(start) = markup::footnote_area_start(text, &analysis) {
                        let r = galley.pos_from_cursor(CCursor::new(byte_to_char(text, start))).translate(gpos.to_vec2());
                        let y = (r.top() - px(z, 5.0)).round() + 0.5;
                        painter_line(ui, p, gpos.x, y, text_w / 3.0, z);
                    }
                    let hover = ui.input(|i| i.pointer.hover_pos()).filter(|q| te_rect.contains(*q));
                    if let Some(q) = hover {
                        let ci = galley.cursor_from_pos(q - gpos).index.0;
                        if let Some((label, is_def, lr)) = markup::footnote_label_at(&analysis, char_to_byte(text, ci)) {
                            let ra = galley.pos_from_cursor(CCursor::new(byte_to_char(text, lr.start))).translate(gpos.to_vec2());
                            let rb = galley.pos_from_cursor(CCursor::new(byte_to_char(text, lr.end))).translate(gpos.to_vec2());
                            let hit = Rect::from_min_max(ra.min, rb.max).expand2(vec2(3.0, 0.0));
                            if hit.contains(q) {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                let tip = if is_def { "Tillbaka till texten" } else { "Gå till fotnot (Ctrl+klick)" };
                                egui::Tooltip::always_open(ui.ctx().clone(), out.response.layer_id, out.response.id.with("fotnot"), egui::PopupAnchor::Pointer)
                                    .show(|ui| ui.label(tip));
                                let (ctrl, pressed) = ui.input(|i| (i.modifiers.command, i.pointer.primary_pressed()));
                                // Ctrl+klick hoppar alltid; ett vanligt klick hoppar bara om raden inte redan visar sin markup
                                // (annars går det inte att redigera runt numret). Hoppet görs när knappen släpps.
                                if pressed && (ctrl || !overlaps(&lr, &reveal)) {
                                    fn_pending = markup::footnote_target(text, &analysis, &label, is_def);
                                }
                            }
                        }
                    }
                }
                let (released, down) = ui.input(|i| (i.pointer.primary_released(), i.pointer.primary_down()));
                if released {
                    fn_jump = fn_pending.take();
                } else if !down {
                    fn_pending = None;
                }

                // Ett ark per sida, med luft emellan.
                let total = pages::page_count(&galley, &geom);
                let clip = ui.clip_rect();
                let (shadows, sheets) = pages::sheet_shapes(p, &geom, page_left, page_top, total, clip);
                ui.painter().set(shadow_slot, shadows);
                ui.painter().set(page_slot, sheets);
                let last_bottom = pages::sheet_rect(&geom, page_left, page_top, total - 1).bottom();
                ui.allocate_rect(
                    Rect::from_min_max(origin, pos2(origin.x + area_w, last_bottom.max(te_rect.bottom()) + GAP)),
                    Sense::hover(),
                );
                // Sidhuvud och sidfot (skrivskyddade här; dubbelklick öppnar sidinställningarna).
                let hf_font = pages::header_font(
                    z,
                    &analysis.defaults,
                    font_family(Some(analysis.defaults.font()), false, false),
                );
                for k in 0..total {
                    let sheet = pages::sheet_rect(&geom, page_left, page_top, k);
                    if sheet.bottom() < clip.top() || sheet.top() > clip.bottom() {
                        continue;
                    }
                    if pages::header_footer(ui, p, &geom, sheet, k, total, &analysis.defaults, hf_font.clone(), doc_id) {
                        open_page_dlg = true;
                    }
                }
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
                // Bara det som syns (med marginal) behöver ritas om: sökväg via galleriets rader.
                let vis = {
                    let lo = galley.cursor_from_pos(vec2(0.0, clip.top() - gpos.y - 300.0)).index.0;
                    let hi = galley.cursor_from_pos(vec2(0.0, clip.bottom() - gpos.y + 300.0)).index.0;
                    char_to_byte(text, lo)..char_to_byte(text, hi)
                };
                let i0 = analysis.pieces.partition_point(|pc| pc.range.end <= vis.start);
                let i1 = i0 + analysis.pieces[i0..].partition_point(|pc| pc.range.start < vis.end);
                if !refs.is_empty() {
                    img_index = analysis.pieces[..i0].iter().filter(|pc| pc.image.is_some()).count();
                }
                for pc in &analysis.pieces[i0..i1] {
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
                    if pc.image.is_some() {
                        img_index += 1;
                    }
                    if let (Some(img), false) = (&pc.image, shown) {
                        if let Some(sz) = sizes.get(&images::size_key(img)) {
                            let ci = conv.at(pc.range.start);
                            let r = galley.pos_from_cursor(CCursor::new(ci)).translate(gpos.to_vec2());
                            // Flytande bild: vid textkanten, på radens höjd (raden själv har ingen höjd).
                            let side = word.float_side(doc_id, text, pc.range.start);
                            {
                                let uri = image_uri(&img.src, doc_id);
                                let top = side.and_then(|(_, row)| galley.rows.get(row)).map_or(r.min.y, |row| row.pos.y + gpos.y);
                                let min = match side {
                                    Some((markup::Float::Left, _)) => pos2(gpos.x, top),
                                    Some((markup::Float::Right, _)) => pos2(gpos.x + text_w - sz.x, top),
                                    None => r.min,
                                };
                                let rect = Rect::from_min_size(min, *sz);
                                if rect.bottom() >= clip.top() && rect.top() <= clip.bottom() {
                                    egui::Image::new(uri.clone()).paint_at(ui, rect);
                                    painted.push(images::Painted {
                                        rect,
                                        align: img.align,
                                        range: pc.range.clone(),
                                        index: img_index - 1,
                                        uri,
                                    });
                                }
                            }
                        }
                    }
                }
                flush_bar(&mut bar);

                // Innehållsförteckningar: raden är dold och har reserverat höjden; posterna
                // ritas här med sidnumren från galleriet (sidbrytningen är redan gjord, så en
                // enda passage räcker: förteckningens höjd beror bara på antalet rubriker).
                if text.contains("kladd: toc") {
                    let from = text[..vis.start].rfind('\n').map_or(0, |i| i + 1);
                    let to = text[vis.end..].find('\n').map_or(text.len(), |i| vis.end + i);
                    let entries = nav::toc_entries(&heads);
                    let (shown, toc_h) = nav::toc_size_for(z, &analysis.defaults, entries.len());
                    for (k, line) in nav::toc_lines(&text[from..to]).into_iter().enumerate() {
                        let line = line.start + from..line.end + from;
                        if overlaps(&line, &reveal) {
                            continue;
                        }
                        let top = galley
                            .pos_from_cursor(CCursor::new(text[..line.start].chars().count()))
                            .translate(gpos.to_vec2())
                            .min;
                        let rect = Rect::from_min_size(pos2(gpos.x, top.y), vec2(text_w, toc_h));
                        if rect.bottom() < clip.top() || rect.top() > clip.bottom() {
                            continue;
                        }
                        let mut c2 = ByteToChar::new(text);
                        let page_nums: Vec<usize> = entries
                            .iter()
                            .take(shown)
                            .map(|h| geom.page_of(galley.pos_from_cursor(CCursor::new(c2.at(h.start))).min.y) + 1)
                            .collect();
                        let fam = |bold: bool| font_family(Some(analysis.defaults.font()), bold, false);
                        if let Some(b) = super::navui::paint_toc(
                            ui,
                            p,
                            z,
                            &analysis.defaults,
                            &fam,
                            rect,
                            egui::Id::unique(("kladd-toc", doc_id, k)),
                            &entries,
                            &page_nums,
                            shown,
                        ) {
                            toc_click = Some(b);
                        }
                    }
                }

                // Markering, handtag och verktygsrad för bilder; ligger ovanpå texten.
                let max_w = text_w;
                // Var en dragen bild skulle hamna: raden under pekaren, före eller efter beroende på
                // om pekaren står över eller under radens mitt. Bara över ett ark.
                let drop_at = |pos: egui::Pos2| -> Option<images::Drop> {
                    if !(0..total).any(|k| pages::sheet_rect(&geom, page_left, page_top, k).contains(pos)) {
                        return None;
                    }
                    let b = char_to_byte(text, galley.cursor_from_pos(pos - gpos).index.0);
                    let extent = |l: &Range<usize>| {
                        let a = galley.pos_from_cursor(CCursor::new(byte_to_char(text, l.start)));
                        let e = galley.pos_from_cursor(CCursor::new(byte_to_char(text, l.end)));
                        (a.min.y + gpos.y, e.max.y + gpos.y)
                    };
                    let raw = markup::drop_line(text, b, false);
                    let (top, bottom) = extent(&raw);
                    let after = pos.y > (top + bottom) / 2.0;
                    let line = markup::drop_line(text, b, after);
                    let (top, bottom) = if line == raw { (top, bottom) } else { extent(&line) };
                    Some(images::Drop {
                        target: if after { line.end } else { line.start },
                        after,
                        y: if after { bottom } else { top },
                        x0: gpos.x,
                        x1: gpos.x + text_w,
                    })
                };
                img_act = images::ui(ui, &mut imgs, doc_id, &painted, n_images, (z, max_w), id, p, &drop_at);

                // Tabellrutnät: 1 px kantlinjer runt varje cell, och en förhandsvisning av kolumnkanten som dras.
                if let Some((_, geoms)) = &tview {
                    let stroke = Stroke::new(1.0, p.page_text.gamma_multiply(0.6));
                    let snap = |v: f32| (v - 0.5).round() + 0.5;
                    for vt in &vtabs {
                        let t = &geoms[vt.ti];
                        for (ri, row) in t.rows.iter().enumerate() {
                            let (Some(first), Some(last)) = (row.cells.first(), row.cells.last()) else { continue };
                            let (y0, y1) = (gpos.y + vt.exts[ri].0, gpos.y + vt.exts[ri].1);
                            if y1 < clip.top() || y0 > clip.bottom() {
                                continue;
                            }
                            for c in &row.cells {
                                let x = snap(gpos.x + t.cell_x(c).0);
                                painter.line_segment([pos2(x, snap(y0)), pos2(x, snap(y1))], stroke);
                            }
                            let (x0, x1) = (snap(gpos.x + t.cell_x(first).0), snap(gpos.x + t.cell_x(last).1));
                            painter.line_segment([pos2(x1, snap(y0)), pos2(x1, snap(y1))], stroke);
                            if ri == 0 {
                                painter.line_segment([pos2(x0, snap(y0)), pos2(x1, snap(y0))], stroke);
                            }
                            painter.line_segment([pos2(x0, snap(y1)), pos2(x1, snap(y1))], stroke);
                        }
                    }
                    if let Some((x, y0, y1)) = drag_preview {
                        painter.line_segment([pos2(snap(x), y0), pos2(snap(x), y1)], Stroke::new(2.0, p.accent));
                    }
                }

                let caret_ci = out.state.cursor.char_range().map_or(0, |r| r.primary.index.0);
                let caret_y = galley.pos_from_cursor(CCursor::new(caret_ci)).min.y;
                page_pos = ((geom.page_of(caret_y) + 1).min(total), total);

                // Röd våglinje under felstavade ord.
                if !misspelled.is_empty() {
                    let caret_b = char_to_byte(text, caret_ci);
                    let mut conv = ByteToChar::new(text);
                    let amp = 1.2 * z.max(0.8);
                    let half = 2.0 * z.max(0.8);
                    let m0 = misspelled.partition_point(|r| r.end < vis.start);
                    for r in misspelled[m0..].iter().take_while(|r| r.start <= vis.end) {
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
                    let geoms = tview.as_ref().map(|v| v.1.clone()).unwrap_or_default();
                    table_menu = Some(tables::on_secondary_click(ui.ctx(), id, text, ui.ctx().pointer_interact_pos().map(|pos| tables::byte_at(text, &galley, &geoms, pos - gpos))));
                }
                let target = match &new_target {
                    Some(t) => t.clone(),
                    None => secondary_target.clone(),
                };
                let in_table_menu = table_menu.unwrap_or_else(|| tables::menu_open(ui.ctx()));
                if target.is_some() || in_table_menu {
                    out.response.context_menu(|ui| {
                        ui.set_min_width(210.0);
                        ui.set_max_width(260.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if let Some(t) = &target {
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
                            if in_table_menu {
                                menu_divider(ui, p);
                            }
                        }
                        if in_table_menu {
                            table_act = tables::menu_items(ui, p);
                        }
                    });
                }

                if let Some(ci) = scroll_to {
                    let r = galley.pos_from_cursor(CCursor::new(ci)).translate(gpos.to_vec2());
                    ui.scroll_to_rect(r.expand2(vec2(80.0, 60.0)), Some(Align::Center));
                }
            });

            self.wv.pages = page_pos;
            self.wv.fn_pending = fn_pending;
            if let Some(b) = toc_click {
                self.goto_byte(&ctx, b);
            }
            let img_range = imgs.range.clone();
            self.wv.img = imgs;
            if let (Some(act), Some(r)) = (img_act, img_range) {
                self.word_image_act(&ctx, act, r);
            }
            if open_page_dlg {
                self.open_page_dialog();
            }
            if let Some(t) = new_target {
                self.wv.target = t;
            }
            if edited {
                self.find.current = None;
                self.word_renumber_footnotes(&ctx);
            }
            if let Some(b) = fn_jump {
                self.goto_byte(&ctx, b);
            }
            if let Some(op) = table_act {
                self.word_table_op(&ctx, op);
            }
            if let Some((at, dashes)) = col_resize {
                self.word_edit(&ctx, |t, s| markup::table_set_widths(t, at, s, &dashes));
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
/// En tabell som syns: index i `table_view` och överkant/nederkant för varje rad.
struct VisTable {
    ti: usize,
    exts: Vec<(f32, f32)>,
}

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

/// Fotnotsskiljelinjen: en tunn linje, en tredjedels textbredd lång.
fn painter_line(ui: &Ui, p: &Palette, x: f32, y: f32, len: f32, z: f32) {
    ui.painter().hline(x..=x + len, y, Stroke::new(z.max(1.0), p.page_text.gamma_multiply(0.7)));
}

fn image_uri(src: &str, doc_id: u64) -> String {
    format!("bytes://kladd-doc/{doc_id}/{}", src.replace(' ', "%20"))
}

#[cfg(test)]
mod tests {
    use super::step_size;

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
