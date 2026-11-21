//! Fotnoterna längst ned på sidorna: ritning, skiljelinje och redigering på plats.
//!
//! Noterna (`fnotes::NoteLayout`) är färdigutlagda av `layout`; här ritas de på arken enligt
//! `PageBox::notes`/`reserved`. Ett klick på en not öppnar en liten textruta ovanpå den med
//! definitionens text; ändringarna skrivs tillbaka till definitionsraden i dokumentet.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::text_edit::TextEditState;
use egui::{FontFamily, FontId, Frame, Id, Key, Margin, Modifiers, Rect, Sense, Stroke, TextEdit, Ui, UiBuilder, pos2, vec2};

use super::fnotes::NoteLayout;
use super::layout::px;
use super::markup::DocDefaults;
use super::secpages::PageBox;
use crate::theme::Palette;

/// Den not som redigeras just nu.
#[derive(Clone, Debug)]
pub struct NoteEdit {
    pub label: String,
    /// Textrutan har haft fokus (förlorar den det stängs redigeringen).
    pub focused: bool,
    /// Bildrutor sedan redigeringen öppnades, för att ge upp om fokus aldrig kommer.
    pub age: u32,
    /// Ändringen har lagts som ett ångra-steg (resten av redigeringen slås ihop med det).
    pub undo_pushed: bool,
    /// Textrutans markör ska läggas sist när den öppnas.
    pub caret_end: bool,
}

impl NoteEdit {
    pub fn new(label: &str) -> Self {
        Self { label: label.to_owned(), focused: false, age: 0, undo_pushed: false, caret_end: true }
    }
}

/// Vad vyn ska göra efter ritningen.
#[derive(Default)]
pub struct Out {
    /// Noten (etikett) som klickats.
    pub open: Option<String>,
    /// Ny text för definitionen (byteintervallet i dokumentet, ny text).
    pub write: Option<(Range<usize>, String)>,
    /// Redigeringen ska stängas; sant om markören ska tillbaka till referensen.
    pub close: Option<bool>,
}

/// Allt som behövs för att rita fotnoterna.
pub struct Env<'a> {
    pub palette: &'a Palette,
    pub zoom: f32,
    pub doc_id: u64,
    pub text: &'a str,
    pub boxes: &'a [PageBox],
    pub notes: &'a [NoteLayout],
    /// Skrivbordsytans vänsterkant och skärm-y för galleriets y = 0.
    pub page_left: f32,
    pub origin_y: f32,
    pub clip: Rect,
    pub defaults: &'a DocDefaults,
    pub family: FontFamily,
}

/// Textytans läge på skärmen för noterna på arket: (vänsterkant, bredd).
fn text_x(b: &PageBox, page_left: f32) -> (f32, f32) {
    (page_left + b.text_left(), (b.w - b.margin[1] - b.margin[3]).max(1.0))
}

/// Var på skärmen not nummer `k` på arket ligger (överkant och höjd), räknat från arkets notyta.
pub fn note_tops(b: &PageBox, notes: &[NoteLayout], zoom: f32, origin_y: f32) -> Vec<(usize, f32, f32)> {
    let mut y = origin_y + b.notes_top() + px(zoom, 9.0);
    let mut out = Vec::new();
    for &n in &b.notes {
        let Some(nl) = notes.get(n) else { continue };
        let h = nl.galley.rect.height();
        out.push((n, y, h));
        y += h + px(zoom, 2.0);
    }
    out
}

/// Ritar fotnoterna på alla ark som syns och sköter redigeringen av en not.
pub fn show(ui: &mut Ui, env: &Env, edit: &mut Option<NoteEdit>) -> Out {
    let mut out = Out::default();
    let p = env.palette;
    for (bi, b) in env.boxes.iter().enumerate() {
        if b.notes.is_empty() || b.reserved <= 0.0 {
            continue;
        }
        let sheet = b.rect(env.page_left, env.origin_y);
        if sheet.bottom() < env.clip.top() || sheet.top() > env.clip.bottom() {
            continue;
        }
        let (x, w) = text_x(b, env.page_left);
        let area_top = env.origin_y + b.notes_top();
        let area_bottom = env.origin_y + b.text_bottom();
        // Skiljelinjen: en tredjedels textbredd.
        let line_y = (area_top + px(env.zoom, 3.0)).round() + 0.5;
        ui.painter().hline(x..=x + w / 3.0, line_y, Stroke::new(env.zoom.max(1.0), p.page_text.gamma_multiply(0.7)));
        let clipped = ui.painter().with_clip_rect(Rect::from_min_max(pos2(x - 4.0, area_top), pos2(x + w + 4.0, area_bottom)).intersect(env.clip));
        for (n, y, h) in note_tops(b, env.notes, env.zoom, env.origin_y) {
            let nl = &env.notes[n];
            let rect = Rect::from_min_size(pos2(x, y), vec2(w, h));
            if rect.bottom() < env.clip.top() || rect.top() > env.clip.bottom() {
                continue;
            }
            let editing = edit.as_ref().is_some_and(|e| e.label == nl.label);
            if editing {
                // Numret ritas som förut; resten ersätts av textrutan.
                let row0 = nl.galley.rows.first().map_or(h, |r| r.row.size.y);
                let num = ui.painter().with_clip_rect(Rect::from_min_size(rect.min, vec2(nl.indent.max(1.0), row0)).intersect(env.clip));
                num.galley(rect.min, nl.galley.clone(), p.page_text);
                edit_box(ui, env, nl, rect, edit, &mut out);
            } else {
                clipped.galley(rect.min, nl.galley.clone(), p.page_text);
                let id = Id::unique(("kladd-note", env.doc_id, bi, n));
                let r = ui.interact(rect.intersect(Rect::from_min_max(pos2(x - 4.0, area_top), pos2(x + w + 4.0, area_bottom))), id, Sense::click());
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                }
                if r.clicked() {
                    out.open = Some(nl.label.clone());
                }
            }
        }
    }
    // Redigeringen hör till en not som inte syns (arket ligger utanför bilden eller noten är borta):
    // ge upp efter en stund så att den inte blir hängande.
    if let Some(e) = edit.as_mut() {
        if env.notes.iter().all(|n| n.label != e.label) {
            out.close = Some(false);
        }
    }
    out
}

/// Textrutan som ersätter noten medan den redigeras.
fn edit_box(ui: &mut Ui, env: &Env, nl: &NoteLayout, rect: Rect, edit: &mut Option<NoteEdit>, out: &mut Out) {
    let Some(e) = edit.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let id = Id::unique(("kladd-note-edit", env.doc_id));
    let body = env.text.get(nl.body.clone()).unwrap_or("").to_owned();
    if e.caret_end {
        let mut st = TextEditState::default();
        let end = CCursor::new(body.chars().count());
        st.cursor.set_char_range(Some(CCursorRange::one(end)));
        st.store(&ctx, id);
        e.caret_end = false;
    }
    if !e.focused {
        ctx.memory_mut(|m| m.request_focus(id));
        ctx.request_repaint();
    }
    let has_focus = ctx.memory(|m| m.has_focus(id));
    // Enter och Escape avslutar (en not är ett enda stycke); Enter konsumeras så att ingen radbrytning skrivs.
    // Escape kan redan ha tagit fokus från textrutan (appen släpper fokus på Escape), så det avläses oavsett fokus.
    let enter = has_focus && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::SHIFT, Key::Enter));
    let esc = e.focused && ctx.input(|i| i.key_pressed(Key::Escape));
    let size = px(env.zoom, env.defaults.size() * 0.85);
    let (family, color, indent) = (env.family.clone(), env.palette.page_text, nl.indent);
    let mut layouter = |ui: &Ui, s: &dyn egui::TextBuffer, wrap: f32| {
        let mut job = LayoutJob::default();
        job.wrap.max_width = wrap;
        job.append(
            s.as_str(),
            indent,
            TextFormat { font_id: FontId::new(size, family.clone()), color, line_height: Some(size * 1.4), ..Default::default() },
        );
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut buf = body.clone();
    let r = ui
        .scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            TextEdit::multiline(&mut buf)
                .id(id)
                .frame(Frame::NONE)
                .margin(Margin::ZERO)
                .desired_width(rect.width())
                .desired_rows(1)
                .min_size(vec2(rect.width(), 1.0))
                .layouter(&mut layouter)
                .show(ui)
        })
        .inner;
    if r.response.has_focus() {
        e.focused = true;
    } else {
        e.age += 1;
    }
    if buf != body {
        out.write = Some((nl.body.clone(), buf.replace(['\r', '\n'], " ")));
    }
    if enter || esc {
        out.close = Some(true);
    } else if (e.focused && !r.response.has_focus()) || e.age > 5 {
        out.close = Some(false);
    }
}

// ---------------------------------------------------------------------------------------------
// Vyn: öppna, skriva tillbaka och stänga

use super::markup;
use super::tables;
use crate::app::{KladdApp, editor_id};

impl KladdApp {
    /// Efter bildrutan: öppnar och stänger notredigeringen, skriver tillbaka ändrad text och håller
    /// markören ur de dolda definitionsraderna (där den inte syns).
    pub(crate) fn word_notes_after(&mut self, ctx: &egui::Context, open: Option<String>, out: Out) {
        if let Some(label) = out.open.or(open) {
            self.wv.fn_edit = Some(NoteEdit::new(&label));
            ctx.request_repaint();
        }
        if let Some((range, new)) = out.write {
            self.word_note_write(ctx, range, &new);
        }
        if let Some(to_ref) = out.close {
            let label = self.wv.fn_edit.take().map(|e| e.label);
            if let (true, Some(label)) = (to_ref, label) {
                self.word_note_back(ctx, &label);
                self.focus_editor = true;
            }
            ctx.request_repaint();
        }
        self.word_note_caret(ctx);
    }

    /// Byter ut definitionens text. Hela redigeringen av en not blir ett ångra-steg.
    fn word_note_write(&mut self, ctx: &egui::Context, range: Range<usize>, new: &str) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if range.end > doc.text.len() || !doc.text.is_char_boundary(range.start) || !doc.text.is_char_boundary(range.end) {
            return;
        }
        if !self.wv.fn_edit.as_ref().is_some_and(|e| e.undo_pushed) {
            let id = editor_id(doc.id);
            let mut state = TextEditState::load(ctx, id).unwrap_or_default();
            let end = doc.text.chars().count();
            let before = state.cursor.char_range().unwrap_or(CCursorRange::two(CCursor::new(end), CCursor::new(end)));
            let mut undoer = state.undoer();
            undoer.add_undo(&(before, doc.text.clone()));
            state.set_undoer(undoer);
            state.store(ctx, id);
            if let Some(e) = self.wv.fn_edit.as_mut() {
                e.undo_pushed = true;
            }
        }
        doc.text.replace_range(range, new);
        doc.on_edited();
        self.find.current = None;
        ctx.request_repaint();
    }

    /// Flyttar markören till efter referensen till `label`, utan att rulla.
    fn word_note_back(&mut self, ctx: &egui::Context, label: &str) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let a = self.word.analysis(doc.id, &doc.text);
        if let Some(b) = markup::footnote_target(&doc.text, &a, label, true) {
            self.word_put_caret(ctx, b);
        }
    }

    fn word_put_caret(&mut self, ctx: &egui::Context, byte: usize) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let c = tables::cursor_at(&doc.text, byte);
        state.cursor.set_char_range(Some(CCursorRange::one(c)));
        state.store(ctx, id);
    }

    /// Markören i en dold definition (infogad fotnot, sökträff, Ctrl+End): noten öppnas för redigering på sin
    /// sida och markören går till referensen. Piltangenter som bara glider in bland definitionerna backas.
    fn word_note_caret(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if !doc.text.contains("[^") {
            return;
        }
        let sel = self.word_selection(ctx);
        if sel.start != sel.end {
            return;
        }
        let notes = self.word.notes(doc.id, &doc.text);
        let Some(n) = notes.iter().find(|n| n.line.start <= sel.start && sel.start <= n.line.end) else { return };
        let editing = self.wv.fn_edit.as_ref().map(|e| e.label.as_str());
        let sliding = ctx.input(|i| [Key::ArrowDown, Key::ArrowRight, Key::PageDown, Key::End].iter().any(|k| i.key_pressed(*k)));
        if editing.is_none() && sliding {
            let first = notes.iter().map(|n| n.line.start).min().unwrap_or(0);
            let to = doc.text[..first].trim_end().len();
            self.word_put_caret(ctx, to);
            return;
        }
        let label = n.label.clone();
        if editing != Some(label.as_str()) {
            self.wv.fn_edit = Some(NoteEdit::new(&label));
        }
        self.word_note_back(ctx, &label);
        ctx.request_repaint();
    }
}
