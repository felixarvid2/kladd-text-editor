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
    Align, Align2, CentralPanel, FontFamily, FontId, Frame, KeyboardShortcut, Layout, Margin,
    Modifiers, Key, Panel, Popup, Rect, RectAlign, ScrollArea, Sense, Shape, Stroke, TextBuffer, TextEdit, Ui,
    UiBuilder, Vec2, pos2, vec2,
};

use super::layout::{self, MARGIN_PT, PAGE_H_PT, PAGE_W_PT, px};
use super::markup::{self, Block, Inline};
use crate::app::{KladdApp, byte_to_char, char_to_byte, editor_id, menu_divider, menu_item, status_button};
use crate::icons::{self, GlyphStyle, Icon};
use crate::theme::{self, Palette};

const BAR_H: f32 = 40.0;
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

fn divider(ui: &mut Ui, p: &Palette) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 22.0), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, p.line));
    ui.add_space(4.0);
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
        Panel::top("kladd-wordbar")
            .exact_size(BAR_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin::symmetric(12, 0)))
            .show(ui, |ui| {
                let full = ui.max_rect();
                ui.painter().hline(
                    (full.left() - 12.0)..=(full.right() + 12.0),
                    full.bottom() - 0.5,
                    Stroke::new(1.0, p.line),
                );
                let right_w = 250.0;
                let left = Rect::from_min_max(full.min, pos2((full.right() - right_w - 8.0).max(full.left()), full.bottom()));
                let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);

                ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)), |ui| {
                    ui.set_clip_rect(left.expand2(vec2(0.0, 2.0)));
                    ui.spacing_mut().item_spacing.x = 4.0;
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
                    divider(ui, p);
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

        for a in acts {
            match a {
                BarAct::Inline(m) => self.word_toggle(&ctx, m),
                BarAct::Block(b) => self.word_block(&ctx, b),
                BarAct::Image => self.request_insert_image(&ctx),
                BarAct::Table => self.word_edit(&ctx, |t, s| markup::insert_table(t, s.end)),
                BarAct::Undo => self.word_undo(&ctx, false),
                BarAct::Redo => self.word_undo(&ctx, true),
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
                let body = FontId::new(px(z, 11.0), theme::doc_font(false, false));

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
                    if pc.list_marker && pc.markup && !shown {
                        let ci = conv.at(pc.range.start);
                        let r = galley.pos_from_cursor(CCursor::new(ci)).translate(gpos.to_vec2());
                        if r.bottom() >= clip.top() && r.top() <= clip.bottom() {
                            painter.text(r.min, Align2::LEFT_TOP, "•", body.clone(), p.page_text);
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

                // Sidbrytningar (ungefärliga).
                let top_of_text = gpos.y;
                let bottom = gpos.y + galley.size().y;
                let mut total = 1usize;
                let right_x = page_left + page_w - margin + 8.0;
                let mut k = 1usize;
                while top_of_text + k as f32 * text_h_page < bottom {
                    let y = top_of_text + k as f32 * text_h_page;
                    total += 1;
                    if y >= clip.top() - 20.0 && y <= clip.bottom() + 20.0 {
                        painter.extend(Shape::dashed_line(
                            &[pos2(page_left, y), pos2(page_left + page_w, y)],
                            Stroke::new(1.0, p.page_markup),
                            5.0,
                            4.0,
                        ));
                        painter.text(
                            pos2(right_x, y - 2.0),
                            Align2::LEFT_BOTTOM,
                            format!("Sida {}", k + 1),
                            FontId::new(10.0, FontFamily::Proportional),
                            p.page_muted,
                        );
                    }
                    k += 1;
                }
                let caret_ci = out.state.cursor.char_range().map_or(0, |r| r.primary.index.0);
                let caret_y = galley.pos_from_cursor(CCursor::new(caret_ci)).min.y + gpos.y - top_of_text;
                let cur = ((caret_y / text_h_page).floor().max(0.0) as usize + 1).min(total);
                pages = (cur, total);

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
