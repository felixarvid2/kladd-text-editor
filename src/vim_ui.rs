//! Gränssnittsdelen av Vim-läget: tangenthändelser från egui till [`crate::vim::Vim`], blockmarkör,
//! markering och texten i statusraden. All logik finns i `vim.rs`; här sitter bara limmet.
//!
//! Som i `multicursor.rs` plockas de händelser Vim använder bort ur `egui::InputState` före
//! `TextEdit`. Kladds egna Ctrl-genvägar rörs inte utom de Vim själv använder i normalläget
//! (Ctrl+d, u, r, v, o). Ctrl+v kommer från plattformen som en inklistringshändelse och tolkas
//! som blockmarkering i normalläget.

use egui::text::CCursor;
use egui::text_edit::TextEditState;
use egui::text_selection::CCursorRange;
use egui::{Context, Event, FontFamily, FontId, Galley, Id, Key as EKey, Modifiers, Painter, Pos2, Rect, Ui, pos2};

use crate::theme::Palette;
use crate::tf;
use crate::vim::{Effect, Key, Mode, View, Vim};

/// Vad ett steg gav: ändrad text, vart vyn ska rulla och vad appen ska göra.
#[derive(Default)]
pub struct Step {
    pub edited: bool,
    pub scroll_to: Option<usize>,
    pub scroll_lines: i32,
    pub effects: Vec<Effect>,
}

pub struct State {
    pub vim: Vim,
    doc_id: u64,
    /// Markören som Vim sist skrev till `TextEdit`; avviker den har musen flyttat den.
    last_set: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        let mut vim = Vim::new();
        vim.clip_get = Some(read_clipboard);
        State { vim, doc_id: 0, last_set: None }
    }
}

fn read_clipboard() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

enum Verdict {
    Keys(Vec<Key>),
    Drop,
    Pass,
}

/// Översätter en egui-händelse beroende på läget. `Pass` = vidare till `TextEdit`.
fn classify(ev: &Event, mode: Mode) -> Verdict {
    let insert = matches!(mode, Mode::Insert | Mode::Replace);
    let cmdline = mode == Mode::Command;
    match ev {
        Event::Text(s) => {
            let keys: Vec<Key> = s.chars().filter(|c| !c.is_control()).map(Key::Char).collect();
            if keys.is_empty() { Verdict::Drop } else { Verdict::Keys(keys) }
        }
        Event::Paste(s) => {
            if insert {
                Verdict::Pass
            } else if cmdline {
                Verdict::Keys(s.lines().next().unwrap_or("").chars().map(Key::Char).collect())
            } else {
                // Ctrl+v kommer som en inklistring: blockmarkering.
                Verdict::Keys(vec![Key::Ctrl('v')])
            }
        }
        Event::Cut if !insert => Verdict::Drop,
        Event::Key { key, pressed: true, modifiers: m, .. } => key_verdict(*key, *m, mode),
        _ => Verdict::Pass,
    }
}

fn key_verdict(key: EKey, m: Modifiers, mode: Mode) -> Verdict {
    let insert = matches!(mode, Mode::Insert | Mode::Replace);
    let cmdline = mode == Mode::Command;
    if m.ctrl && !m.alt && !m.shift {
        return match key {
            EKey::OpenBracket => Verdict::Keys(vec![Key::Esc]),
            EKey::D | EKey::U | EKey::R | EKey::O if !insert && !cmdline => {
                let c = match key {
                    EKey::D => 'd',
                    EKey::U => 'u',
                    EKey::R => 'r',
                    _ => 'o',
                };
                Verdict::Keys(vec![Key::Ctrl(c)])
            }
            EKey::U | EKey::W | EKey::C if cmdline => {
                let c = match key {
                    EKey::U => 'u',
                    EKey::W => 'w',
                    _ => 'c',
                };
                Verdict::Keys(vec![Key::Ctrl(c)])
            }
            _ => Verdict::Pass,
        };
    }
    if m.command || m.alt || m.ctrl {
        return Verdict::Pass;
    }
    let plain = !m.shift;
    let k = match key {
        EKey::Escape if plain => Some(Key::Esc),
        EKey::Enter => Some(Key::Enter),
        EKey::Backspace => Some(Key::Backspace),
        EKey::Tab if plain => Some(Key::Tab),
        EKey::Delete if plain => Some(Key::Delete),
        EKey::ArrowLeft if plain => Some(Key::Left),
        EKey::ArrowRight if plain => Some(Key::Right),
        EKey::ArrowUp if plain => Some(Key::Up),
        EKey::ArrowDown if plain => Some(Key::Down),
        EKey::Home if plain => Some(Key::Home),
        EKey::End if plain => Some(Key::End),
        _ => None,
    };
    match k {
        Some(k) => Verdict::Keys(vec![k]),
        // Skift+Tab och Skift+piltangenter: i infogningsläget gör `TextEdit` det, annars inget.
        None if matches!(key, EKey::Tab | EKey::ArrowLeft | EKey::ArrowRight | EKey::ArrowUp | EKey::ArrowDown) && !insert => {
            Verdict::Drop
        }
        None => Verdict::Pass,
    }
}

impl State {
    /// Kör före allt annat som läser tangenter. `focused`: editorn har tangentbordsfokus.
    pub fn pre(&mut self, ctx: &Context, id: Id, doc_id: u64, text: &mut String, view: View) -> Step {
        let mut step = Step::default();
        if self.doc_id != doc_id {
            self.vim.switch_doc();
            self.doc_id = doc_id;
            self.last_set = None;
        }
        // Ingen annan ruta (sökfältet, dialoger) har tangentbordsfokus.
        if ctx.memory(|m| m.focused().is_some_and(|f| f != id)) {
            return step;
        }
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let head = state.cursor.char_range().map_or(0, |r| r.primary.index.0);
        // Musen har flyttat markören (eller texten ändrades utifrån): lita på `TextEdit`.
        let moved = self.last_set != Some(head);
        if moved {
            self.vim.cur = head;
            self.vim.adopt(text);
        }
        let vim = &mut self.vim;
        let mut any = false;
        ctx.input_mut(|i| {
            let events = std::mem::take(&mut i.events);
            let mut keep = Vec::with_capacity(events.len());
            for ev in events {
                match classify(&ev, vim.mode) {
                    Verdict::Pass => keep.push(ev),
                    Verdict::Drop => any = true,
                    Verdict::Keys(keys) => {
                        any = true;
                        for k in keys {
                            let out = vim.key(k, text, view);
                            step.edited |= out.edited;
                            for e in out.effects {
                                match e {
                                    Effect::Scroll(n) => step.scroll_lines += n,
                                    e => step.effects.push(e),
                                }
                            }
                        }
                    }
                }
            }
            i.events = keep;
        });
        if any || moved {
            let n = text.chars().count();
            self.vim.cur = self.vim.cur.min(n);
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(self.vim.cur))));
            state.store(ctx, id);
            self.last_set = Some(self.vim.cur);
            step.scroll_to = Some(self.vim.cur);
            ctx.request_repaint();
        }
        step
    }

    /// Statusradens text: (läge eller kommandorad, väntande kommando, inspelning).
    pub fn status(&self) -> (String, String) {
        let v = &self.vim;
        let left = if v.mode == Mode::Command { format!("{}{}", v.cmd_prefix(), v.cmdline) } else { v.mode_label().to_owned() };
        let mut right = v.pending();
        if let Some(r) = v.recording_reg() {
            let rec = tf!("registrerar @{reg}", reg = r);
            right = if right.is_empty() { rec } else { format!("{rec}  {right}") };
        }
        (left, right)
    }

    pub fn hides_caret(&self) -> bool {
        self.vim.mode != Mode::Insert
    }

    /// Ritar markeringen och blockmarkören (utom i infogningsläget där `TextEdit` ritar strecket).
    pub fn paint(&self, ui: &Ui, p: &Palette, galley: &Galley, origin: Pos2, text: &str, font_size: f32) {
        let painter = ui.painter();
        let shift = origin.to_vec2();
        let clip = painter.clip_rect();
        let at = |ci: usize| galley.pos_from_cursor(CCursor::new(ci)).translate(shift);
        let mut row_start = Vec::with_capacity(galley.rows.len());
        let mut n = 0usize;
        for row in &galley.rows {
            row_start.push(n);
            n += row.char_count_including_newline().0;
        }
        for (s, e) in self.vim.selection_ranges(text) {
            for (ri, row) in galley.rows.iter().enumerate() {
                let rs = row_start[ri];
                let text_end = rs + row.char_count_excluding_newline().0;
                let (a, b) = (s.max(rs), e.min(rs + row.char_count_including_newline().0));
                if a >= b && !(s == e && a == rs) {
                    continue;
                }
                let r = row.rect().translate(shift);
                if r.bottom() < clip.top() || r.top() > clip.bottom() {
                    continue;
                }
                let x0 = at(a.min(text_end)).min.x;
                let mut x1 = at(b.min(text_end)).min.x;
                if b > text_end || a >= text_end {
                    x1 += 6.0;
                }
                let band = Rect::from_x_y_ranges(x0..=x1.max(x0 + 2.0), r.top()..=r.bottom());
                painter.rect_filled(band, 0.0, ui.visuals().selection.bg_fill);
            }
        }
        if self.vim.mode == Mode::Insert {
            return;
        }
        let c = at(self.vim.cur);
        if c.bottom() < clip.top() || c.top() > clip.bottom() {
            return;
        }
        let font = FontId::new(font_size, FontFamily::Monospace);
        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font, ' ')).max(1.0);
        let next = at(self.vim.cur + 1);
        let w = if (next.min.y - c.min.y).abs() < 1.0 && next.min.x > c.min.x + 1.0 { next.min.x - c.min.x } else { char_w };
        let rect = if self.vim.mode == Mode::Replace {
            Rect::from_min_max(pos2(c.min.x, c.bottom() - 2.5), pos2(c.min.x + w, c.bottom()))
        } else {
            Rect::from_min_max(c.min, pos2(c.min.x + w, c.bottom()))
        };
        paint_block(painter, rect, p);
    }
}

fn paint_block(painter: &Painter, rect: Rect, p: &Palette) {
    painter.rect_filled(rect, 1.0, p.accent.gamma_multiply(0.62));
}
