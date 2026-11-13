//! Granska i dokumentläget: spårade ändringar och kommentarer (menyn, kortkommandon, kommentarsfältet
//! till höger om sidan). Logiken på texten finns i `crate::review`; här är gränssnittet och kopplingen
//! till editorn. Se `docs/dev/dokumentlage.md`.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, Galley};
use egui::text_edit::TextEditState;
use egui::{Color32, FontFamily, FontId, Frame, Key, KeyboardShortcut, Margin, Modifiers, Pos2, Rect, Sense, Stroke, TextEdit, Ui, UiBuilder, pos2, vec2};

use super::tables;
use crate::app::{KladdApp, byte_to_char, editor_id, menu_divider, menu_item};
use crate::review::{self, Thread, Tracked};
use crate::theme::Palette;

/// Kommentarsfältets bredd (px, oberoende av zoom) och luften mellan sidan och fältet.
pub(crate) const PANE_W: f32 = 250.0;
pub(crate) const PANE_GAP: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ReviewAct {
    ToggleTrack,
    Accept,
    Reject,
    AcceptAll,
    RejectAll,
    Prev,
    Next,
    NewComment,
    Reply,
    DeleteComment,
    ToggleComments,
    YourName,
}

/// Åtgärder från kommentarskorten.
pub(crate) enum PaneAct {
    /// Markera trådens område i texten.
    Select(u32),
    Reply(u32, String),
    Delete(u32),
    Done(u32, bool),
    /// Kommentarens text är skriven (eller avbruten: tom text tar bort en ny kommentar).
    SetText(u32, String),
}

struct Cache {
    doc_id: u64,
    rev: u64,
    threads: Arc<Vec<Thread>>,
    authors: Arc<Vec<String>>,
    changes: usize,
}

#[derive(Default)]
pub struct ReviewState {
    /// Dokument (id) som spårar ändringar.
    tracking: HashSet<u64>,
    cache: Option<Cache>,
    /// Vald tråd (rotkommentarens nummer).
    pub(crate) selected: Option<u32>,
    /// Kommentaren vars text skrivs just nu, och texten.
    editing: Option<u32>,
    edit_buf: String,
    reply_buf: String,
    focus_edit: bool,
    focus_reply: bool,
    name_open: bool,
}

/// Det word_page behöver veta om granskningen under en bildruta.
pub(crate) struct Prep {
    pub threads: Arc<Vec<Thread>>,
    /// Författarna i ordning (för färgerna).
    pub authors: Arc<Vec<String>>,
    /// Bredd som kommentarsfältet tar bredvid sidan (0 när det är dolt).
    pub side: f32,
    pub author: String,
    pub date: String,
    /// Text och markering före bildrutans redigering, när ändringar spåras.
    pub before: Option<(String, Range<usize>)>,
}

/// Författarens färg på sidan (ljusare i mörkt tema).
pub(crate) fn author_color(authors: &[String], author: &str, dark: bool) -> Color32 {
    let c = review::AUTHOR_COLORS[usize::from(review::author_color_index(authors, author))];
    if dark {
        let m = |v: u8| (f32::from(v) * 0.55 + 255.0 * 0.45) as u8;
        Color32::from_rgb(m(c[0]), m(c[1]), m(c[2]))
    } else {
        Color32::from_rgb(c[0], c[1], c[2])
    }
}

/// `2026-05-01T10:00:00Z` → `2026-05-01 10:00`.
pub(crate) fn short_date(d: &str) -> String {
    let d = d.trim();
    if d.len() >= 16 && d.is_char_boundary(16) {
        d[..16].replace('T', " ")
    } else {
        d.to_owned()
    }
}

/// Menyn Granska i formateringsraden.
pub(crate) fn menu(ui: &mut Ui, p: &Palette, tracking: bool, comments_shown: bool, changes: usize) -> Option<ReviewAct> {
    let mut act = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut item = |ui: &mut Ui, a: ReviewAct, label: &str, sc: &str, checked: Option<bool>, enabled: bool| {
        if menu_item(ui, p, label, sc, checked, enabled) {
            act = Some(a);
        }
    };
    item(ui, ReviewAct::ToggleTrack, "Spåra ändringar", "Ctrl+Shift+E", Some(tracking), true);
    menu_divider(ui, p);
    item(ui, ReviewAct::Accept, "Godkänn ändring", "", None, changes > 0);
    item(ui, ReviewAct::Reject, "Avvisa ändring", "", None, changes > 0);
    item(ui, ReviewAct::Prev, "Föregående ändring", "", None, changes > 0);
    item(ui, ReviewAct::Next, "Nästa ändring", "", None, changes > 0);
    item(ui, ReviewAct::AcceptAll, "Godkänn alla", "", None, changes > 0);
    item(ui, ReviewAct::RejectAll, "Avvisa alla", "", None, changes > 0);
    menu_divider(ui, p);
    item(ui, ReviewAct::NewComment, "Ny kommentar", "Ctrl+Alt+M", None, true);
    item(ui, ReviewAct::Reply, "Svara", "", None, true);
    item(ui, ReviewAct::DeleteComment, "Ta bort kommentar", "", None, true);
    item(ui, ReviewAct::ToggleComments, "Visa kommentarer", "", Some(comments_shown), true);
    menu_divider(ui, p);
    item(ui, ReviewAct::YourName, "Ditt namn…", "", None, true);
    act
}

fn truncate_to(ctx: &egui::Context, id: egui::Id, text: &str, range: Range<usize>) {
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(tables::cursor_at(text, range.start), tables::cursor_at(text, range.end))));
    state.store(ctx, id);
}

impl KladdApp {
    /// Namnet som skrivs på egna ändringar och kommentarer.
    pub(crate) fn review_author(&self) -> String {
        let n = self.settings.author_name.trim();
        if !n.is_empty() {
            return n.to_owned();
        }
        ["USER", "USERNAME"]
            .iter()
            .find_map(|k| std::env::var(k).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()))
            .unwrap_or_else(|| "Författare".to_owned())
    }

    pub(crate) fn review_tracking(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| self.wv.review.tracking.contains(&d.id))
    }

    /// Antal ändringar i aktiva dokumentet enligt senast beräknade cache.
    pub(crate) fn review_changes(&self) -> usize {
        let Some(doc) = self.docs.get(self.active) else { return 0 };
        self.wv.review.cache.as_ref().filter(|c| c.doc_id == doc.id).map_or(0, |c| c.changes)
    }

    /// Förbereder en bildruta: trådarna (cachade per revision), fältets bredd och ögonblicksbilden
    /// som behövs för att spåra en redigering.
    pub(crate) fn review_prep(&mut self, ctx: &egui::Context, doc_id: u64, sel: &Range<usize>) -> Prep {
        let author = self.review_author();
        let date = crate::docx::now_w3cdtf();
        let doc = &self.docs[self.active];
        let rev = self.word.rev(doc_id, &doc.text);
        let fresh = self.wv.review.cache.as_ref().is_some_and(|c| c.doc_id == doc_id && c.rev == rev);
        if !fresh {
            let (threads, authors, changes) = if review::has_review(&doc.text) {
                (review::threads(&doc.text), review::authors(&review::scan(&doc.text)), review::change_count(&doc.text))
            } else {
                (Vec::new(), Vec::new(), 0)
            };
            self.wv.review.cache = Some(Cache { doc_id, rev, threads: Arc::new(threads), authors: Arc::new(authors), changes });
        }
        let threads = self.wv.review.cache.as_ref().map(|c| c.threads.clone()).unwrap_or_default();
        let authors = self.wv.review.cache.as_ref().map(|c| c.authors.clone()).unwrap_or_default();
        review::set_comments_visible(self.settings.word_comments);
        let show = self.settings.word_comments && !threads.is_empty();
        let focused = ctx.memory(|m| m.has_focus(editor_id(doc_id)));
        if focused {
            self.wv.review.selected = review::thread_at(&doc.text, sel.start);
        }
        // Ögonblicksbild bara när något kan redigera texten (annars klonas 600 KB varje bildruta).
        let editing_keys = ctx.input(|i| {
            let undo = i.modifiers.command && (i.key_pressed(Key::Z) || i.key_pressed(Key::Y));
            !undo
                && i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Cut | egui::Event::Ime(_)
                    ) || matches!(e, egui::Event::Key { key: Key::Backspace | Key::Delete | Key::Enter | Key::Tab, pressed: true, .. })
                })
        });
        let tracking = self.wv.review.tracking.contains(&doc_id);
        let before = (tracking && focused && editing_keys).then(|| (doc.text.clone(), sel.clone()));
        Prep { threads, authors, side: if show { PANE_W + PANE_GAP } else { 0.0 }, author, date, before }
    }

    /// Ersätter texten med den spårade redigeringen och flyttar markören.
    pub(crate) fn review_apply_tracked(&mut self, ctx: &egui::Context, t: Tracked) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let id = editor_id(doc.id);
        doc.text = t.text;
        doc.on_edited();
        truncate_to(ctx, id, &doc.text, t.caret..t.caret);
        ctx.request_repaint();
    }

    fn review_edit(&mut self, ctx: &egui::Context, f: impl FnOnce(&str, Range<usize>) -> Option<crate::word::markup::Edit>) {
        let mut none = true;
        self.word_edit(ctx, |t, s| match f(t, s.clone()) {
            Some(e) => {
                none = false;
                e
            }
            None => crate::word::markup::Edit { text: t.to_owned(), sel: s },
        });
        if none {
            self.notify(ctx, "Ingen ändring här", false);
        }
    }

    pub(crate) fn review_act(&mut self, ctx: &egui::Context, act: ReviewAct) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let doc_id = doc.id;
        match act {
            ReviewAct::ToggleTrack => {
                let on = self.wv.review.tracking.insert(doc_id);
                if !on {
                    self.wv.review.tracking.remove(&doc_id);
                }
                self.notify(ctx, if on { "Spårar ändringar" } else { "Spårar inte ändringar" }, false);
            }
            ReviewAct::Accept | ReviewAct::Reject => {
                let accept = act == ReviewAct::Accept;
                self.review_edit(ctx, |t, s| review::resolve_range(t, s, accept));
            }
            ReviewAct::AcceptAll | ReviewAct::RejectAll => {
                let accept = act == ReviewAct::AcceptAll;
                self.word_edit(ctx, |t, s| {
                    let text = if accept { review::accept_all(t) } else { review::reject_all(t) };
                    let c = s.start.min(text.len());
                    let c = (0..=c).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
                    crate::word::markup::Edit { text, sel: c..c }
                });
            }
            ReviewAct::Next | ReviewAct::Prev => {
                let sel = self.word_selection(ctx);
                let text = &self.docs[self.active].text;
                let hit = if act == ReviewAct::Next { review::next_change(text, sel.end) } else { review::prev_change(text, sel.start) };
                match hit {
                    Some(r) => {
                        let ci = byte_to_char(text, r.start);
                        self.word_edit(ctx, |t, _| crate::word::markup::Edit { text: t.to_owned(), sel: r.clone() });
                        self.scroll_to_char = Some(ci);
                    }
                    None => self.notify(ctx, "Inga fler ändringar", false),
                }
            }
            ReviewAct::NewComment => self.review_new_comment(ctx),
            ReviewAct::Reply => {
                let sel = self.word_selection(ctx);
                let text = &self.docs[self.active].text;
                match review::thread_at(text, sel.start).or(self.wv.review.selected) {
                    Some(id) => {
                        self.settings.word_comments = true;
                        self.wv.review.selected = Some(id);
                        self.wv.review.focus_reply = true;
                    }
                    None => self.notify(ctx, "Markören står inte i en kommentar", false),
                }
            }
            ReviewAct::DeleteComment => {
                let sel = self.word_selection(ctx);
                let text = &self.docs[self.active].text;
                match review::thread_at(text, sel.start).or(self.wv.review.selected) {
                    Some(id) => self.review_pane_act(ctx, PaneAct::Delete(id)),
                    None => self.notify(ctx, "Markören står inte i en kommentar", false),
                }
            }
            ReviewAct::ToggleComments => self.settings.word_comments = !self.settings.word_comments,
            ReviewAct::YourName => self.wv.review.name_open = true,
        }
    }

    /// Ny kommentar på markeringen (eller ordet vid markören); texten skrivs direkt i kortet.
    fn review_new_comment(&mut self, ctx: &egui::Context) {
        let author = self.review_author();
        let date = crate::docx::now_w3cdtf();
        let mut new_id = 0;
        self.word_edit(ctx, |t, s| {
            let (e, id) = review::add_comment(t, s, &author, &date, "");
            new_id = id;
            e
        });
        self.settings.word_comments = true;
        let r = &mut self.wv.review;
        r.selected = Some(new_id);
        r.editing = Some(new_id);
        r.edit_buf.clear();
        r.focus_edit = true;
    }

    pub(crate) fn review_pane_act(&mut self, ctx: &egui::Context, act: PaneAct) {
        let author = self.review_author();
        let date = crate::docx::now_w3cdtf();
        match act {
            PaneAct::Select(id) => {
                let Some(doc) = self.docs.get(self.active) else { return };
                let Some(th) = review::threads(&doc.text).into_iter().find(|t| t.root.id == id) else { return };
                let r = th.range;
                let ci = byte_to_char(&doc.text, r.start);
                self.wv.review.selected = Some(id);
                self.word_edit(ctx, |t, _| crate::word::markup::Edit { text: t.to_owned(), sel: r.clone() });
                self.scroll_to_char = Some(ci);
            }
            PaneAct::Reply(id, body) => {
                let body = body.trim().to_owned();
                if body.is_empty() {
                    return;
                }
                self.word_edit(ctx, |t, s| match review::add_reply(t, id, &author, &date, &body) {
                    Some((e, _)) => crate::word::markup::Edit { text: e.text, sel: s },
                    None => crate::word::markup::Edit { text: t.to_owned(), sel: s },
                });
                self.wv.review.reply_buf.clear();
            }
            PaneAct::Delete(id) => {
                self.word_edit(ctx, |t, s| {
                    let text = review::delete_comment(t, id);
                    let c = s.start.min(text.len());
                    let c = (0..=c).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
                    crate::word::markup::Edit { text, sel: c..c }
                });
                if self.wv.review.editing == Some(id) {
                    self.wv.review.editing = None;
                }
                self.wv.review.selected = None;
            }
            PaneAct::Done(id, done) => {
                self.word_edit(ctx, |t, s| crate::word::markup::Edit { text: review::edit_comment(t, id, |c| c.done = done), sel: s });
            }
            PaneAct::SetText(id, body) => {
                self.wv.review.editing = None;
                if body.trim().is_empty() {
                    // En ny kommentar utan text avbryts; en befintlig behåller sin text.
                    let empty = self.docs.get(self.active).is_some_and(|d| review::threads(&d.text).iter().any(|t| t.root.id == id && t.root.text.is_empty()));
                    if empty {
                        self.review_pane_act(ctx, PaneAct::Delete(id));
                    }
                    return;
                }
                self.word_edit(ctx, |t, s| crate::word::markup::Edit { text: review::edit_comment(t, id, |c| c.text = body.trim().to_owned()), sel: s });
            }
        }
    }

    /// Kortkommandon i dokumentläget: Ctrl+Skift+E (spåra) och Ctrl+Alt+M (ny kommentar).
    pub(crate) fn review_keys(&mut self, ctx: &egui::Context) {
        let shift = ctx.input(|i| i.modifiers.shift);
        let hit = |m: Modifiers, k: Key| (m.shift || !shift) && ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if hit(Modifiers::COMMAND | Modifiers::SHIFT, Key::E) {
            self.review_act(ctx, ReviewAct::ToggleTrack);
        }
        if hit(Modifiers::COMMAND | Modifiers::ALT, Key::M) {
            self.review_act(ctx, ReviewAct::NewComment);
        }
    }

    /// Fönstret "Ditt namn".
    pub(crate) fn review_name_window(&mut self, ctx: &egui::Context) {
        if !self.wv.review.name_open {
            return;
        }
        let mut open = true;
        let mut done = false;
        egui::Window::new("Ditt namn").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label("Namnet som skrivs på dina ändringar och kommentarer.");
            let hint = self.review_author();
            let r = ui.add(TextEdit::singleline(&mut self.settings.author_name).hint_text(hint).desired_width(260.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                done = true;
            }
            if ui.button("Klar").clicked() {
                done = true;
            }
        });
        if done || !open {
            self.wv.review.name_open = false;
            self.focus_editor = true;
        }
    }
}

/// Ritar kommentarskorten i fältet till höger om sidan. Korten ligger vid sin kommenterade text
/// och skjuts ned så att de inte överlappar.
pub(crate) fn paint_pane(
    ui: &mut Ui,
    p: &Palette,
    rv: &Prep,
    st: &mut ReviewState,
    galley: &Galley,
    gpos: Pos2,
    text: &str,
    x: f32,
) -> Option<PaneAct> {
    if rv.side <= 0.0 {
        return None;
    }
    let mut act: Option<PaneAct> = None;
    let clip = ui.clip_rect();
    let mut next_y = f32::MIN;
    let click = ui.input(|i| i.pointer.primary_clicked()).then(|| ui.input(|i| i.pointer.interact_pos())).flatten();
    for th in rv.threads.iter() {
        let sel = st.selected == Some(th.root.id);
        let anchor = galley.pos_from_cursor(CCursor::new(byte_to_char(text, th.range.start.min(text.len())))).translate(gpos.to_vec2());
        let top = anchor.top().max(next_y);
        // Långt under det som syns: inget att rita, men kvarvarande kort behövs inte heller.
        if top > clip.bottom() + 600.0 {
            break;
        }
        let area = Rect::from_min_size(pos2(x, top), vec2(PANE_W, f32::INFINITY));
        let mut child = ui.new_child(UiBuilder::new().max_rect(area).id_salt(("kladd-cm", th.root.id)));
        let mut inner_click = false;
        let frame = Frame::NONE
            .fill(if sel { p.surface_hover } else { p.surface })
            .stroke(Stroke::new(if sel { 1.5 } else { 1.0 }, if sel { p.accent } else { p.line }))
            .corner_radius(6)
            .inner_margin(Margin::same(8))
            .show(&mut child, |ui| {
                ui.set_width(PANE_W - 16.0);
                card(ui, p, &rv.authors, th, sel, st, &mut act, &mut inner_click);
            });
        let rect = frame.response.rect;
        next_y = rect.bottom() + 8.0;
        ui.expand_to_include_rect(rect);
        if let Some(q) = click {
            if rect.contains(q) && !inner_click && act.is_none() {
                act = Some(PaneAct::Select(th.root.id));
            }
        }
        // Linje från kortet till den kommenterade texten.
        if rect.top() <= clip.bottom() && rect.bottom() >= clip.top() {
            let c = author_color(&rv.authors, &th.root.author, p.dark);
            ui.painter().line_segment([pos2(x - PANE_GAP * 0.5, anchor.center().y), pos2(x, rect.top() + 12.0)], Stroke::new(1.0, c));
        }
    }
    act
}

fn card(ui: &mut Ui, p: &Palette, authors: &[String], th: &Thread, sel: bool, st: &mut ReviewState, act: &mut Option<PaneAct>, inner_click: &mut bool) {
    let small = FontId::new(11.5, FontFamily::Proportional);
    let body = FontId::new(13.0, FontFamily::Proportional);
    let one = |ui: &mut Ui, c: &review::CommentRec, root: bool, st: &mut ReviewState, act: &mut Option<PaneAct>, inner_click: &mut bool| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(egui::RichText::new(&c.author).font(FontId::new(12.5, FontFamily::Proportional)).strong().color(author_color(authors, &c.author, p.dark)));
            ui.label(egui::RichText::new(short_date(&c.date)).font(small.clone()).color(p.muted));
        });
        if st.editing == Some(c.id) {
            let r = ui.add(TextEdit::multiline(&mut st.edit_buf).desired_rows(2).desired_width(f32::INFINITY).hint_text("Skriv en kommentar…").font(body.clone()));
            if st.focus_edit {
                r.request_focus();
                st.focus_edit = false;
            }
            if r.hovered() || r.has_focus() {
                *inner_click = true;
            }
            ui.horizontal(|ui| {
                if ui.small_button("Spara").clicked() {
                    *inner_click = true;
                    *act = Some(PaneAct::SetText(c.id, std::mem::take(&mut st.edit_buf)));
                }
                if ui.small_button("Avbryt").clicked() {
                    *inner_click = true;
                    *act = Some(PaneAct::SetText(c.id, String::new()));
                }
            });
            // Ctrl+Enter sparar.
            if r.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter)) {
                *act = Some(PaneAct::SetText(c.id, std::mem::take(&mut st.edit_buf)));
            }
        } else {
            let r = ui.add(egui::Label::new(egui::RichText::new(&c.text).font(body.clone()).color(p.text)).wrap().sense(Sense::click()));
            if root && r.double_clicked() {
                *inner_click = true;
                st.editing = Some(c.id);
                st.edit_buf = c.text.clone();
                st.focus_edit = true;
            }
        }
    };
    one(ui, &th.root, true, st, act, inner_click);
    for r in &th.replies {
        ui.add_space(4.0);
        ui.separator();
        one(ui, r, false, st, act, inner_click);
    }
    if th.root.done {
        ui.label(egui::RichText::new("Löst").font(small.clone()).color(p.muted));
    }
    if sel && st.editing != Some(th.root.id) {
        ui.add_space(4.0);
        let r = ui.add(TextEdit::singleline(&mut st.reply_buf).hint_text("Svara…").desired_width(f32::INFINITY).font(body.clone()));
        if st.focus_reply {
            r.request_focus();
            st.focus_reply = false;
        }
        if r.hovered() || r.has_focus() {
            *inner_click = true;
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            *act = Some(PaneAct::Reply(th.root.id, std::mem::take(&mut st.reply_buf)));
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui.small_button("Svara").clicked() {
                *inner_click = true;
                *act = Some(PaneAct::Reply(th.root.id, std::mem::take(&mut st.reply_buf)));
            }
            if ui.small_button(if th.root.done { "Öppna igen" } else { "Löst" }).clicked() {
                *inner_click = true;
                *act = Some(PaneAct::Done(th.root.id, !th.root.done));
            }
            if ui.small_button("Ta bort").clicked() {
                *inner_click = true;
                *act = Some(PaneAct::Delete(th.root.id));
            }
        });
    }
}
