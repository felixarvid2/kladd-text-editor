//! Gränssnittet för språkservern: krumelurer och ikoner för fel, komplettering, hover, signaturhjälp,
//! Problem-panelen, namnrutan och inställningsrutan, samt kopplingen till `KladdApp`.
//!
//! Editorn anropar bara två krokar: [`Lsp::pre`] före `TextEdit::show` (tangenter) och [`Lsp::post`]
//! efter (ritning, hover, utlösare). Resten är `impl KladdApp`-block längst ned.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::text_edit::{TextEditOutput, TextEditState};
use egui::{
    Align, Align2, Color32, Context, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Order, Panel, Rect,
    RichText, ScrollArea, Sense, Shape, Stroke, TextEdit, Ui, Vec2, pos2, vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::convert::{self, CompItem, Severity, Signature};
use super::pos::{self, CEdit, Edit, Remap};
use super::{Cfg, Jump, Lsp, Trigger, config};
use crate::app::{KladdApp, editor_id, quiet_button, status_button};
use crate::doc::Document;
use crate::i18n::t;
use crate::theme::{self, Palette};
use crate::tf;

/// Så länge pekaren ska vila på ett ord innan hover visas.
const HOVER_DELAY: Duration = Duration::from_millis(450);
const ROW_H: f32 = 22.0;
const COMP_ROWS: usize = 10;
const COMP_W: f32 = 420.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Problems,
    Refs,
}

pub(super) struct Comp {
    doc_id: u64,
    base: Arc<String>,
    /// Där ordet före markören börjar (i texten vid anropet; ändringar sker bara efter den).
    anchor: usize,
    pub(super) items: Vec<CompItem>,
    incomplete: bool,
    /// Index i `items`, bäst först, för det som skrivits sedan `anchor`.
    view: Vec<usize>,
    prefix: Option<String>,
    sel: usize,
    scroll: bool,
    /// Listans ruta senast den ritades (klick utanför stänger den).
    rect: Rect,
}

pub(super) struct Hover {
    doc_id: u64,
    base: Arc<String>,
    range: (usize, usize),
    md: String,
    rect: Rect,
}

struct Sig {
    doc_id: u64,
    sig: Signature,
    /// Markörens plats när hjälpen öppnades: bakom den (före den) stänger den.
    open_at: usize,
}

struct RenameDlg {
    doc_id: u64,
    at: usize,
    name: String,
    focus: bool,
}

pub(super) struct RefItem {
    path: PathBuf,
    start: (u32, u32),
    end: (u32, u32),
    preview: String,
}

#[derive(Default)]
pub struct State {
    pub(super) comp: Option<Comp>,
    pub(super) hover: Option<Hover>,
    /// Ordet pekaren vilar på (början, sedan när) och om hover redan frågats efter.
    hover_wait: Option<(usize, Instant, bool)>,
    sig: Option<Sig>,
    /// Markörens plats när signaturhjälpen öppnades.
    sig_at: usize,
    rename: Option<RenameDlg>,
    refs_title: String,
    refs: Vec<RefItem>,
    pub(crate) panel: bool,
    tab: Tab,
    pub(crate) settings_open: bool,
    /// Text som skrevs den här bilden (för utlösartecken).
    typed: String,
    accept: Option<usize>,
    md: CommonMarkCache,
}

/// Vad `pre` ändrade, på samma sätt som flermarkörsläget rapporterar.
#[derive(Default)]
pub struct PreOut {
    pub edited: bool,
    pub scroll_to: Option<usize>,
}

pub(super) fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Där ordet som slutar vid `caret` börjar.
pub(super) fn word_start(text: &str, caret: usize) -> usize {
    let before: Vec<char> = text.chars().take(caret).collect();
    before.len() - before.iter().rev().take_while(|c| is_word(**c)).count()
}

fn sev_color(p: &Palette, s: Severity) -> Color32 {
    match s {
        Severity::Error => p.error,
        Severity::Warning => Color32::from_rgb(226, 168, 50),
        Severity::Info => Color32::from_rgb(86, 156, 226),
        Severity::Hint => p.muted,
    }
}

fn sev_name(s: Severity) -> &'static str {
    match s {
        Severity::Error => t("Fel"),
        Severity::Warning => t("Varning"),
        Severity::Info => t("Information"),
        Severity::Hint => t("Tips"),
    }
}

/// Bokstav och färg för objektets sort i kompletteringslistan.
fn kind_badge(p: &Palette, kind: u8) -> (&'static str, Color32) {
    match kind {
        2..=4 => ("f", p.accent),
        5 | 10 => ("p", Color32::from_rgb(86, 156, 226)),
        6 => ("v", Color32::from_rgb(86, 156, 226)),
        7 | 8 | 13 | 22 | 25 => ("T", Color32::from_rgb(78, 190, 150)),
        9 => ("M", Color32::from_rgb(190, 130, 220)),
        14 => ("k", p.muted),
        15 => ("s", Color32::from_rgb(226, 168, 50)),
        12 | 20 | 21 => ("c", Color32::from_rgb(226, 120, 100)),
        _ => ("·", p.muted),
    }
}

// ------------------------------------------------------------------ tillstånd

impl State {
    #[cfg(test)]
    pub(super) fn has_rename(&self) -> bool {
        self.rename.is_some()
    }

    #[cfg(test)]
    pub(super) fn rename_name(&self) -> Option<&str> {
        self.rename.as_ref().map(|r| r.name.as_str())
    }

    pub(super) fn open_rename(&mut self, doc_id: u64, at: usize, name: String) {
        self.rename = Some(RenameDlg { doc_id, at, name, focus: true });
    }

    pub(super) fn set_completion(&mut self, doc_id: u64, base: Arc<String>, anchor: usize, items: Vec<CompItem>, incomplete: bool) {
        self.comp = Some(Comp { doc_id, base, anchor, items, incomplete, view: Vec::new(), prefix: None, sel: 0, scroll: true, rect: Rect::NOTHING });
    }

    pub(super) fn hover_reply(&mut self, doc_id: u64, base: Arc<String>, at: usize, reply: Option<(String, Option<(usize, usize)>)>) {
        let word = convert::word_at(&base, at);
        let (md, range) = match reply {
            Some((md, r)) => (md, r.filter(|(a, b)| a < b).or(word)),
            None => (String::new(), word),
        };
        let Some(range) = range else { return };
        self.hover = Some(Hover { doc_id, base, range, md, rect: Rect::NOTHING });
    }

    pub(super) fn set_signature(&mut self, doc_id: u64, base: Arc<String>, sig: Option<Signature>) {
        let _ = base;
        match sig {
            Some(sig) if !sig.label.is_empty() => {
                self.sig = Some(Sig { doc_id, sig, open_at: self.sig_at });
            }
            _ => self.sig = None,
        }
    }

    /// Lista med platser (definitioner, referenser) i panelen, med radtext som förhandsvisning.
    pub(super) fn show_locations(&mut self, title: &str, locs: &[convert::Location], cur: Option<(PathBuf, Arc<String>)>) {
        let mut files: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut items = Vec::new();
        for l in locs.iter().take(1000) {
            let lines = files.entry(l.path.clone()).or_insert_with(|| {
                let text = match &cur {
                    Some((p, t)) if *p == l.path => Some(t.to_string()),
                    _ => std::fs::read_to_string(&l.path).ok(),
                };
                text.map(|t| t.lines().map(str::to_owned).collect()).unwrap_or_default()
            });
            let preview = lines.get(l.start.0 as usize).map(|s| s.trim().to_owned()).unwrap_or_default();
            items.push(RefItem { path: l.path.clone(), start: l.start, end: l.end, preview });
        }
        self.refs_title = title.to_owned();
        self.refs = items;
        self.tab = Tab::Refs;
        self.panel = true;
    }
}

// ------------------------------------------------------------------ editorkrokar

impl Lsp {
    /// Före `TextEdit::show`: kortkommandon och tangenter för komplettering. `multi`: flermarkörsläget är aktivt.
    pub fn pre(&mut self, ctx: &Context, id: Id, doc: &mut Document, focused: bool, multi: bool) -> PreOut {
        let mut out = PreOut::default();
        self.ui.typed.clear();
        if self.caps(doc.id).is_none() {
            self.ui.comp = None;
            self.ui.sig = None;
            return out;
        }
        if !focused || multi {
            return out;
        }
        ctx.input(|i| {
            for e in &i.events {
                if let egui::Event::Text(s) = e {
                    self.ui.typed.push_str(s);
                }
            }
        });
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let caret = state.cursor.char_range().map_or(0, |r| r.primary.index.0);

        // Kortkommandon. Skift-varianten först: `consume_key` ignorerar extra Skift.
        let (refs, def, rename, format, sig, comp) = ctx.input_mut(|i| {
            let refs = i.consume_key(Modifiers::SHIFT, Key::F12);
            let def = !refs && i.consume_key(Modifiers::NONE, Key::F12);
            let rename = i.consume_key(Modifiers::NONE, Key::F2);
            let format = i.consume_key(Modifiers::SHIFT | Modifiers::ALT, Key::F);
            let sig = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Space);
            let comp = !sig && i.consume_key(Modifiers::COMMAND, Key::Space);
            (refs, def, rename, format, sig, comp)
        });
        if def {
            self.definition(doc, caret);
        }
        if refs {
            self.references(doc, caret);
        }
        if rename {
            self.begin_rename(doc, caret);
        }
        if format {
            self.format(doc);
        }
        if comp {
            self.complete(doc, caret, Trigger::Manual);
        }
        if sig {
            self.ui.sig = None;
            self.ui.sig_at = caret;
            self.signature(doc, caret);
        }

        // Komplettering: piltangenter, Enter, Tab och Esc hör till listan så länge den visas.
        let mut accept = self.ui.accept.take();
        if let Some(c) = &mut self.ui.comp
            && c.doc_id == doc.id
            && !c.view.is_empty()
        {
            let n = c.view.len();
            let (down, up, pgdn, pgup, enter, tab) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::PageDown),
                    i.consume_key(Modifiers::NONE, Key::PageUp),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::NONE, Key::Tab),
                )
            });
            if down {
                c.sel = (c.sel + 1) % n;
            }
            if up {
                c.sel = (c.sel + n - 1) % n;
            }
            if pgdn {
                c.sel = (c.sel + COMP_ROWS).min(n - 1);
            }
            if pgup {
                c.sel = c.sel.saturating_sub(COMP_ROWS);
            }
            if down || up || pgdn || pgup {
                c.scroll = true;
            }
            if enter || tab {
                accept = Some(c.sel);
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            let open = self.ui.comp.is_some() || self.ui.sig.is_some() || self.ui.hover.is_some();
            if open {
                ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
                self.ui.comp = None;
                self.ui.sig = None;
                self.ui.hover = None;
            }
        }

        if let Some(k) = accept
            && let Some(c) = self.ui.comp.take()
            && c.doc_id == doc.id
            && let Some(&ix) = c.view.get(k)
        {
            let new_caret = accept_item(doc, &c, &c.items[ix], caret);
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(new_caret))));
            state.store(ctx, id);
            doc.on_edited();
            out.edited = true;
            out.scroll_to = Some(new_caret);
            ctx.request_repaint();
        }
        out
    }

    /// Efter `TextEdit::show`: utlösare, ritning (krumelurer, ikoner, popuper) och hover.
    pub fn post(&mut self, ui: &Ui, p: &Palette, id: Id, doc: &Document, out: &TextEditOutput, gutter: Rect) {
        let ctx = ui.ctx();
        let Some(tracked) = self.docs.get(&doc.id).filter(|t| t.opened) else {
            self.ui.comp = None;
            self.ui.sig = None;
            self.ui.hover = None;
            return;
        };
        let path = tracked.path.clone();
        let caps = self.caps(doc.id).cloned();
        let caret = out.cursor_range.map_or(0, |r| r.primary.index.0);
        let changed = out.response.changed();
        let focused = ui.memory(|m| m.has_focus(id));

        // Utlösare: tecknet som skrevs, och att texten ändrats med en öppen lista eller signatur.
        if let Some(caps) = &caps {
            let typed = std::mem::take(&mut self.ui.typed);
            if changed && focused {
                if let Some(c) = typed.chars().last().filter(|_| typed.chars().count() == 1) {
                    if caps.completion_triggers.contains(&c) {
                        self.complete(doc, caret, Trigger::Char(c));
                    }
                    if caps.signature_triggers.contains(&c) {
                        if self.ui.sig.is_none() {
                            self.ui.sig_at = caret;
                        }
                        self.signature(doc, caret);
                    }
                    if c == ')' {
                        self.ui.sig = None;
                    }
                }
                if self.ui.sig.is_some() && !typed.is_empty() {
                    self.signature(doc, caret);
                }
            }
        }

        // Listan följer det som skrivs efter ordets början och stängs när den inte längre stämmer.
        let mut refetch = false;
        if let Some(c) = &mut self.ui.comp {
            let text = &doc.text;
            let valid = c.doc_id == doc.id && caret >= c.anchor && {
                let b0 = crate::app::char_to_byte(text, c.anchor);
                let b1 = crate::app::char_to_byte(text, caret);
                b0 <= b1 && text[b0..b1].chars().all(is_word)
            };
            if valid {
                let b0 = crate::app::char_to_byte(text, c.anchor);
                let b1 = crate::app::char_to_byte(text, caret);
                let prefix = text[b0..b1].to_owned();
                if c.prefix.as_deref() != Some(prefix.as_str()) {
                    refetch = c.incomplete && c.prefix.is_some();
                    let mut scored: Vec<(i32, usize)> = c
                        .items
                        .iter()
                        .enumerate()
                        .filter_map(|(i, it)| convert::match_score(&prefix, &it.filter).map(|s| (s, i)))
                        .collect();
                    // Stabil: lika poäng behåller serverns ordning (sortText).
                    scored.sort_by(|a, b| b.0.cmp(&a.0));
                    c.view = scored.into_iter().take(300).map(|(_, i)| i).collect();
                    c.prefix = Some(prefix);
                    c.sel = 0;
                    c.scroll = true;
                }
                if c.view.is_empty() {
                    self.ui.comp = None;
                }
            } else {
                self.ui.comp = None;
            }
        }
        if refetch {
            self.complete(doc, caret, Trigger::Incomplete);
        }
        if out.response.clicked() || out.response.dragged() {
            self.ui.comp = None;
            self.ui.sig = None;
        }
        // Klick någon annanstans än i listan stänger den.
        if let Some(c) = &self.ui.comp
            && ctx.input(|i| i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|pt| !c.rect.contains(pt)))
        {
            self.ui.comp = None;
        }
        // Signaturhjälpen stängs när markören lämnat anropet.
        if let Some(s) = &self.ui.sig
            && (s.doc_id != doc.id || (s.open_at != usize::MAX && caret < s.open_at))
        {
            self.ui.sig = None;
        }

        self.paint_diags(ui, p, &path, doc, out, gutter);
        self.hover_and_links(ui, p, doc, out, &caps);
        self.draw_completion(ui, p, id, doc, out, caret);
        self.draw_signature(ui, p, out, caret);
    }

    fn paint_diags(&self, ui: &Ui, p: &Palette, path: &std::path::Path, doc: &Document, out: &TextEditOutput, gutter: Rect) {
        let diags = self.diags_for(path);
        if diags.is_empty() {
            return;
        }
        let _ = doc;
        let painter = ui.painter();
        let clip = ui.clip_rect();
        let origin = out.galley_pos;
        let rows = &out.galley.rows;
        let mut start = 0usize;
        for (ri, row) in rows.iter().enumerate() {
            let r = row.rect().translate(origin.to_vec2());
            let rs = start;
            let n = row.char_count_excluding_newline().0;
            let next = rs + row.char_count_including_newline().0;
            start = next;
            if r.bottom() < clip.top() {
                continue;
            }
            if r.top() > clip.bottom() {
                break;
            }
            let last = ri + 1 == rows.len();
            let x_at = |col: usize| -> f32 {
                match row.glyphs.get(col) {
                    Some(g) => origin.x + row.pos.x + g.pos.x,
                    None => origin.x + row.pos.x + row.size.x,
                }
            };
            let mut best: Option<Severity> = None;
            for d in diags {
                let Some((a0, b0)) = d.range else { continue };
                if a0 >= rs && (a0 < next || last) {
                    best = Some(best.map_or(d.severity, |b| b.min(d.severity)));
                }
                // Utan längd: understryk tecknet efter (eller före, vid radens slut).
                let (mut a, mut b) = (a0, b0);
                if a == b {
                    if a < rs + n {
                        b = a + 1;
                    } else if a > rs {
                        a -= 1;
                    }
                }
                let (sa, sb) = (a.max(rs), b.min(rs + n));
                if sa >= sb {
                    continue;
                }
                let (x0, x1) = (x_at(sa - rs), x_at(sb - rs));
                let color = sev_color(p, d.severity);
                let y = r.bottom() - 1.5;
                let mut pts = Vec::new();
                let mut x = x0;
                let mut up = false;
                while x < x1 {
                    pts.push(pos2(x, if up { y - 1.6 } else { y }));
                    x += 2.0;
                    up = !up;
                }
                pts.push(pos2(x1, if up { y - 1.6 } else { y }));
                painter.add(Shape::line(pts, Stroke::new(1.2, color)));
            }
            if let Some(sev) = best {
                let c = pos2(gutter.left() + 10.0, r.center().y);
                let color = sev_color(p, sev);
                match sev {
                    Severity::Error => {
                        painter.circle_filled(c, 4.0, color);
                    }
                    Severity::Warning => {
                        painter.add(Shape::convex_polygon(
                            vec![c + vec2(0.0, -4.5), c + vec2(4.5, 3.5), c + vec2(-4.5, 3.5)],
                            color,
                            Stroke::NONE,
                        ));
                    }
                    _ => {
                        painter.circle_stroke(c, 3.5, Stroke::new(1.4, color));
                    }
                }
            }
        }
    }

    /// Hover (pekaren vilar på ett ord) och Ctrl+klick till definition.
    fn hover_and_links(&mut self, ui: &Ui, p: &Palette, doc: &Document, out: &TextEditOutput, caps: &Option<convert::Caps>) {
        let ctx = ui.ctx();
        let origin = out.galley_pos;
        let (ptr, ctrl, clicked, scrolled) = ctx.input(|i| {
            (i.pointer.hover_pos(), i.modifiers.command, i.pointer.primary_clicked(), i.smooth_scroll_delta != Vec2::ZERO)
        });
        let over = ptr.filter(|pt| out.response.hovered() && ui.clip_rect().contains(*pt));
        let under = over.and_then(|pt| char_at_pos(&out.galley, pt - origin));
        let word = under.filter(|(_, c)| is_word(*c)).and_then(|(i, _)| convert::word_at(&doc.text, i));
        let has_def = caps.as_ref().is_some_and(|c| c.definition);

        // Ctrl hållen över ett ord: länk. Klick går till definitionen.
        if ctrl
            && has_def
            && let Some((a, b)) = word
        {
            let r = rect_of_range(&out.galley, origin.to_vec2(), a, b);
            ui.painter().hline(r.x_range(), r.bottom() - 1.0, Stroke::new(1.2, p.accent));
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            if clicked {
                self.definition(doc, a);
                self.ui.hover = None;
            }
            return;
        }

        // En hover som visas försvinner när pekaren lämnat ordet och rutan, eller texten ändrats.
        if let Some(h) = &self.ui.hover {
            let in_word = under.is_some_and(|(i, _)| i >= h.range.0 && i < h.range.1);
            let in_box = ptr.is_some_and(|pt| h.rect.expand(4.0).contains(pt));
            if h.doc_id != doc.id || *h.base != doc.text || scrolled || !(in_word || in_box) {
                self.ui.hover = None;
            }
        }
        if self.ui.hover.is_some() {
            return;
        }
        let Some((a, _)) = word else {
            self.ui.hover_wait = None;
            return;
        };
        let has_diag = doc.path.as_deref().is_some_and(|pth| {
            self.diags_for(pth).iter().any(|d| d.range.is_some_and(|(s, e)| a >= s && a < e.max(s + 1)))
        });
        let can_hover = caps.as_ref().is_some_and(|c| c.hover);
        if !can_hover && !has_diag {
            self.ui.hover_wait = None;
            return;
        }
        match &mut self.ui.hover_wait {
            Some((w, _, _)) if *w == a => {}
            slot => *slot = Some((a, Instant::now(), false)),
        }
        let Some((_, since, asked)) = self.ui.hover_wait else { return };
        let left = HOVER_DELAY.saturating_sub(since.elapsed());
        if !left.is_zero() {
            ctx.request_repaint_after(left);
        } else if !asked {
            if let Some(w) = &mut self.ui.hover_wait {
                w.2 = true;
            }
            if can_hover {
                self.hover(doc, a);
            } else {
                self.ui.hover_reply(doc.id, Arc::new(doc.text.clone()), a, None);
            }
        }
        // Ritas av `draw_hover` när svaret kommit.
    }

    fn draw_hover(&mut self, ui: &Ui, p: &Palette, doc: &Document, out: &TextEditOutput) {
        let ctx = ui.ctx();
        let Some(h) = &mut self.ui.hover else { return };
        let diags: Vec<convert::Diag> = doc
            .path
            .as_deref()
            .map(|pth| {
                self.diags
                    .get(pth)
                    .into_iter()
                    .flatten()
                    .filter(|d| d.range.is_some_and(|(s, e)| h.range.0 >= s && h.range.0 < e.max(s + 1)))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if h.md.trim().is_empty() && diags.is_empty() {
            self.ui.hover = None;
            return;
        }
        let r = rect_of_range(&out.galley, out.galley_pos.to_vec2(), h.range.0, h.range.1);
        let md = h.md.clone();
        let cache = &mut self.ui.md;
        let area = egui::Area::new(Id::unique("kladd-lsp-hover"))
            .order(Order::Tooltip)
            .fixed_pos(pos2(r.left(), r.bottom() + 6.0))
            .constrain(true)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(8)
                    .inner_margin(Margin::same(10))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_max_width(540.0);
                        for d in &diags {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(sev_name(d.severity)).color(sev_color(p, d.severity)).strong());
                                ui.label(RichText::new(&d.message).color(p.text));
                            });
                            let meta = [d.source.clone(), d.code.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                            if !meta.is_empty() {
                                ui.label(RichText::new(meta).small().color(p.muted));
                            }
                        }
                        if !diags.is_empty() && !md.trim().is_empty() {
                            ui.add_space(4.0);
                            ui.separator();
                        }
                        if !md.trim().is_empty() {
                            ScrollArea::vertical().max_height(300.0).auto_shrink([true, true]).show(ui, |ui| {
                                let st = ui.style_mut();
                                st.visuals.override_text_color = Some(p.text);
                                CommonMarkViewer::new().show(ui, cache, &md);
                            });
                        }
                    });
            });
        if let Some(h) = &mut self.ui.hover {
            h.rect = area.response.rect;
        }
    }

    fn draw_completion(&mut self, ui: &Ui, p: &Palette, id: Id, doc: &Document, out: &TextEditOutput, caret: usize) {
        let _ = caret;
        self.draw_hover(ui, p, doc, out);
        let Some(c) = &mut self.ui.comp else { return };
        if c.view.is_empty() {
            return;
        }
        let ctx = ui.ctx();
        let origin = out.galley_pos.to_vec2();
        let a = out.galley.pos_from_cursor(CCursor::new(c.anchor)).translate(origin);
        let rows = c.view.len().min(COMP_ROWS);
        let height = rows as f32 * ROW_H + 10.0;
        let screen = ctx.content_rect();
        let below = a.bottom() + 3.0;
        let top = if below + height > screen.bottom() - 4.0 && a.top() - height - 3.0 > screen.top() { a.top() - height - 3.0 } else { below };
        let mut clicked = None;
        let area = egui::Area::new(Id::unique("kladd-lsp-comp"))
            .order(Order::Foreground)
            .fixed_pos(pos2(a.left().min(screen.right() - COMP_W - 8.0).max(screen.left() + 4.0), top))
            .constrain(true)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(8)
                    .inner_margin(Margin::same(4))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(COMP_W);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ScrollArea::vertical().max_height(rows as f32 * ROW_H).auto_shrink([false, true]).show(ui, |ui| {
                            for (k, &ix) in c.view.iter().enumerate() {
                                let it = &c.items[ix];
                                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                                let sel = k == c.sel;
                                if sel || resp.hovered() {
                                    ui.painter().rect_filled(rect, 5.0, if sel { p.selection } else { p.surface_hover });
                                }
                                let (letter, col) = kind_badge(p, it.kind);
                                let font = FontId::new(12.5, FontFamily::Monospace);
                                ui.painter().text(pos2(rect.left() + 12.0, rect.center().y), Align2::CENTER_CENTER, letter, font.clone(), col);
                                let label = ui.painter().layout_no_wrap(it.label.clone(), font, p.text);
                                let lw = label.size().x;
                                ui.painter().galley(pos2(rect.left() + 26.0, rect.center().y - label.size().y / 2.0), label, p.text);
                                if !it.detail.is_empty() {
                                    let room = (rect.width() - 40.0 - lw).max(0.0);
                                    if room > 30.0 {
                                        let d = crate::app::truncate(it.detail.lines().next().unwrap_or(""), (room / 6.5) as usize);
                                        ui.painter().text(
                                            pos2(rect.right() - 8.0, rect.center().y),
                                            Align2::RIGHT_CENTER,
                                            d,
                                            FontId::new(11.5, FontFamily::Proportional),
                                            p.muted,
                                        );
                                    }
                                }
                                if sel && c.scroll {
                                    ui.scroll_to_rect(rect, None);
                                }
                                if resp.clicked() {
                                    clicked = Some(k);
                                }
                            }
                        });
                        c.scroll = false;
                    });
            });
        c.rect = area.response.rect;
        if let Some(k) = clicked {
            self.ui.accept = Some(k);
            ctx.memory_mut(|m| m.request_focus(id));
            ctx.request_repaint();
        }
    }

    fn draw_signature(&mut self, ui: &Ui, p: &Palette, out: &TextEditOutput, caret: usize) {
        let Some(s) = &self.ui.sig else { return };
        let _ = caret;
        let ctx = ui.ctx();
        let a = out.galley.pos_from_cursor(CCursor::new(caret)).translate(out.galley_pos.to_vec2());
        let font = FontId::new(13.0, FontFamily::Monospace);
        let chars: Vec<char> = s.sig.label.chars().collect();
        let act = s.sig.active.and_then(|i| s.sig.params.get(i)).copied();
        let mut job = LayoutJob::default();
        job.wrap.max_width = 560.0;
        let fmt = |color: Color32, underline: bool| TextFormat {
            font_id: font.clone(),
            color,
            underline: if underline { Stroke::new(1.5, p.accent) } else { Stroke::NONE },
            ..Default::default()
        };
        match act {
            Some((x, y)) if x <= y && y <= chars.len() => {
                let part = |r: std::ops::Range<usize>| chars[r].iter().collect::<String>();
                job.append(&part(0..x), 0.0, fmt(p.muted, false));
                job.append(&part(x..y), 0.0, fmt(p.accent, true));
                job.append(&part(y..chars.len()), 0.0, fmt(p.muted, false));
            }
            _ => job.append(&s.sig.label, 0.0, fmt(p.text, false)),
        }
        let doc_line = s.sig.doc.lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_owned();
        egui::Area::new(Id::unique("kladd-lsp-sig"))
            .order(Order::Foreground)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(pos2(a.left(), a.top() - 6.0))
            .constrain(true)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(10, 7))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.label(job);
                        if !doc_line.is_empty() {
                            ui.label(RichText::new(crate::app::truncate(&doc_line, 140)).small().color(p.muted));
                        }
                    });
            });
    }
}

/// Tar emot valet av ett förslag: ersätter ordet (eller serverns intervall), plattar ut snippets och
/// lägger till extra ändringar (t.ex. `use`). Ger markörens nya plats.
fn accept_item(doc: &mut Document, c: &Comp, item: &CompItem, caret: usize) -> usize {
    let remap = Remap::new(&c.base, &doc.text);
    let conv = |e: &Edit| -> (usize, usize) {
        let ce = pos::to_cedits(&c.base, std::slice::from_ref(e));
        (remap.map(ce[0].start), remap.map(ce[0].end))
    };
    let (mut s, mut e) = match item.range {
        Some((a, b)) => conv(&Edit { start: a, end: b, new_text: String::new() }),
        None => (c.anchor, caret),
    };
    // Det som skrivits sedan anropet hör till ordet som ersätts.
    s = s.min(caret);
    e = e.max(caret).max(s);
    let (text, off) = if item.snippet {
        pos::flatten_snippet(&item.new_text)
    } else {
        let n = item.new_text.chars().count();
        (item.new_text.clone(), n)
    };
    let others: Vec<CEdit> = item
        .additional
        .iter()
        .map(|a| {
            let (x, y) = conv(a);
            CEdit { start: x, end: y.max(x), text: a.new_text.clone() }
        })
        .collect();
    let new_caret = pos::map_index(&others, s) + off;
    let mut edits = others;
    edits.push(CEdit { start: s, end: e, text });
    pos::apply_cedits(&mut doc.text, &edits);
    new_caret
}

/// Tecknet under pekaren (teckenindex och tecken), om pekaren är över text.
fn char_at_pos(galley: &egui::Galley, local: Vec2) -> Option<(usize, char)> {
    let rows = &galley.rows;
    let ri = rows.partition_point(|r| r.rect().bottom() <= local.y);
    let row = rows.get(ri)?;
    if local.y < row.rect().top() {
        return None;
    }
    let start: usize = rows[..ri].iter().map(|r| r.char_count_including_newline().0).sum();
    row.glyphs.iter().enumerate().find_map(|(i, g)| {
        let x0 = row.pos.x + g.pos.x;
        (local.x >= x0 && local.x < x0 + g.advance_width).then_some((start + i, g.chr))
    })
}

/// Rektangel för tecknen `a..b` (första raden om intervallet bryts).
fn rect_of_range(galley: &egui::Galley, shift: Vec2, a: usize, b: usize) -> Rect {
    let r0 = galley.pos_from_cursor(CCursor::new(a)).translate(shift);
    let r1 = galley.pos_from_cursor(CCursor::new(b)).translate(shift);
    if (r0.top() - r1.top()).abs() < 1.0 {
        Rect::from_min_max(r0.min, pos2(r1.max.x.max(r0.min.x + 4.0), r0.max.y))
    } else {
        Rect::from_min_size(r0.min, vec2(60.0, r0.height()))
    }
}

// ------------------------------------------------------------------ fönstret

impl KladdApp {
    /// Varje bild: synka dokument med servrarna och utför det de svarat (hopp, ändringar, meddelanden).
    pub(crate) fn lsp_tick(&mut self, ctx: &Context) {
        let active = self.docs.get(self.active).map(|d| d.id);
        let cfg = Cfg { enabled: self.settings.lsp, commands: &self.settings.lsp_commands, workspace: self.settings.workspace.as_deref() };
        self.lsp.tick(ctx, &self.docs, active, &cfg);
        for (text, error) in std::mem::take(&mut self.lsp.out.notes) {
            self.notify(ctx, text, error);
        }
        for fe in std::mem::take(&mut self.lsp.out.edits) {
            self.lsp_apply_edits(ctx, fe);
        }
        if let Some(j) = self.lsp.out.jump.take() {
            self.lsp_jump(ctx, &j);
        }
    }

    /// Index för en flik med den filen (öppnar den om den inte redan är öppen).
    fn lsp_doc_for(&mut self, ctx: &Context, path: &std::path::Path) -> Option<usize> {
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let find = |s: &Self| s.docs.iter().position(|d| d.path.as_deref() == Some(canon.as_path()) && d.pdfv.is_none());
        if let Some(i) = find(self) {
            return Some(i);
        }
        self.open_path(ctx, &canon);
        find(self)
    }

    fn lsp_apply_edits(&mut self, ctx: &Context, fe: super::FileEdits) {
        let keep = self.docs.get(self.active).map(|d| d.id);
        let Some(i) = self.lsp_doc_for(ctx, &fe.path) else {
            self.notify(ctx, tf!("Kunde inte öppna {file}.", file = fe.path.display()), true);
            return;
        };
        let doc = &mut self.docs[i];
        if let Some(expect) = &fe.expect
            && doc.text != **expect
        {
            self.notify(ctx, t("Texten ändrades medan servern arbetade. Försök igen."), true);
            return;
        }
        let edits = pos::to_cedits(&doc.text, &fe.edits);
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let caret = state.cursor.char_range().map_or(0, |r| r.primary.index.0);
        pos::apply_cedits(&mut doc.text, &edits);
        doc.on_edited();
        let at = pos::map_index(&edits, caret);
        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(at))));
        state.store(ctx, id);
        // Andra filer än den man står i öppnas i bakgrunden: stanna kvar där man är.
        if let Some(k) = keep.and_then(|k| self.index_of(k)) {
            self.active = k;
        }
    }

    fn lsp_jump(&mut self, ctx: &Context, j: &Jump) {
        let Some(i) = self.lsp_doc_for(ctx, &j.path) else {
            self.notify(ctx, tf!("Kunde inte öppna {file}.", file = j.path.display()), true);
            return;
        };
        self.active = i;
        let doc = &self.docs[i];
        let idx = pos::LineIndex::new(&doc.text);
        let a = idx.pos_to_char(&doc.text, j.start.0, j.start.1);
        let b = idx.pos_to_char(&doc.text, j.end.0, j.end.1).max(a);
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
        state.store(ctx, id);
        self.scroll_to_char = Some(a);
        self.focus_editor = true;
    }

    /// Serverns läge och problemräknaren i statusraden.
    pub(crate) fn lsp_status(&mut self, ui: &mut Ui, p: &Palette, font: &FontId) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let Some((text, error)) = self.lsp.status(doc.id) else { return };
        let color = if error { p.error } else { p.muted };
        if error {
            if status_button(ui, p, &text, t("Klicka för att försöka starta språkservern igen")).clicked() {
                self.lsp.restart_failed();
            }
        } else {
            ui.label(RichText::new(text).font(font.clone()).color(color));
        }
        let (e, w) = self.lsp.counts();
        if e + w > 0 || self.lsp.ui.panel {
            let label = tf!("{errors} fel, {warnings} varningar", errors = e, warnings = w);
            if status_button(ui, p, &label, t("Visa eller dölj Problem-panelen")).clicked() {
                self.lsp.ui.panel = !self.lsp.ui.panel;
                self.lsp.ui.tab = Tab::Problems;
            }
        }
    }

    /// Panelen längst ned: Problem och Referenser.
    pub(crate) fn lsp_panel(&mut self, ui: &mut Ui, p: &Palette) {
        if !self.lsp.ui.panel || !self.settings.lsp || self.pdf_mode() || self.word_mode() {
            return;
        }
        let active_path = self.docs.get(self.active).and_then(|d| d.path.clone());
        let mut jump = None;
        let mut close = false;
        let st = &mut self.lsp.ui;
        let diags = &self.lsp.diags;
        Panel::bottom("kladd-lsp-panel")
            .resizable(true)
            .default_size(190.0)
            .size_range(80.0..=600.0)
            .frame(Frame::NONE.fill(p.editor).stroke(Stroke::new(1.0, p.line)).inner_margin(Margin { left: 14, right: 14, top: 6, bottom: 6 }))
            .show(ui, |ui| {
                let mut all: Vec<(&PathBuf, &convert::Diag)> = diags.iter().flat_map(|(path, v)| v.iter().map(move |d| (path, d))).collect();
                let cur = active_path.as_ref();
                all.sort_by(|a, b| {
                    (Some(a.0) != cur, a.1.severity, a.0, a.1.start).cmp(&(Some(b.0) != cur, b.1.severity, b.0, b.1.start))
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    for (tab, label) in [
                        (Tab::Problems, tf!("Problem ({n})", n = all.len())),
                        (Tab::Refs, if st.refs_title.is_empty() { t("Referenser").to_owned() } else { format!("{} ({})", st.refs_title, st.refs.len()) }),
                    ] {
                        let on = st.tab == tab;
                        let rt = RichText::new(label).color(if on { p.text } else { p.muted });
                        let rt = if on { rt.font(FontId::new(13.0, theme::semibold())) } else { rt };
                        if ui.add(egui::Label::new(rt).sense(Sense::click())).clicked() {
                            st.tab = tab;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new("×").color(p.muted)).frame(false)).on_hover_text(t("Stäng")).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(4.0);
                let row = |ui: &mut Ui, color: Option<Color32>, main: &str, side: &str| -> bool {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 4.0, p.surface_hover);
                    }
                    let mut x = rect.left() + 6.0;
                    if let Some(c) = color {
                        ui.painter().circle_filled(pos2(x + 4.0, rect.center().y), 3.5, c);
                        x += 16.0;
                    }
                    let side_g = ui.painter().layout_no_wrap(side.to_owned(), FontId::new(11.5, FontFamily::Proportional), p.muted);
                    let room = (rect.right() - side_g.size().x - 20.0 - x).max(40.0);
                    let main = crate::app::truncate(main, (room / 7.0) as usize);
                    ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, main, FontId::new(13.0, FontFamily::Monospace), p.text);
                    ui.painter().galley(pos2(rect.right() - side_g.size().x - 8.0, rect.center().y - side_g.size().y / 2.0), side_g, p.muted);
                    resp.clicked()
                };
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match st.tab {
                    Tab::Problems => {
                        if all.is_empty() {
                            ui.label(RichText::new(t("Inga problem.")).color(p.muted));
                        }
                        for (path, d) in all.iter().take(1000) {
                            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            let side = format!("{name}:{}:{}", d.start.0 + 1, d.start.1 + 1);
                            let msg = d.message.lines().next().unwrap_or("");
                            if row(ui, Some(sev_color(p, d.severity)), msg, &side) {
                                jump = Some(Jump { path: (*path).clone(), start: d.start, end: d.end });
                            }
                        }
                    }
                    Tab::Refs => {
                        if st.refs.is_empty() {
                            ui.label(RichText::new(t("Inga platser att visa. Prova Skift+F12 på ett namn.")).color(p.muted));
                        }
                        for r in &st.refs {
                            let name = r.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            let side = format!("{name}:{}:{}", r.start.0 + 1, r.start.1 + 1);
                            if row(ui, None, &r.preview, &side) {
                                jump = Some(Jump { path: r.path.clone(), start: r.start, end: r.end });
                            }
                        }
                    }
                });
            });
        if close {
            self.lsp.ui.panel = false;
        }
        if jump.is_some() {
            self.lsp.out.jump = jump;
            ui.ctx().request_repaint();
        }
    }

    /// Namnrutan (F2) och inställningsrutan för språkservrar.
    pub(crate) fn lsp_dialogs(&mut self, ctx: &Context, p: &Palette) {
        self.lsp_rename_dialog(ctx, p);
        self.lsp_settings_dialog(ctx, p);
    }

    fn lsp_rename_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(r) = self.lsp.ui.rename.as_mut() else { return };
        let mut choice = None;
        let modal = egui::Modal::new(Id::unique("kladd-lsp-rename"))
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
                ui.set_width(380.0);
                ui.label(RichText::new(t("Byt namn")).font(FontId::new(17.0, theme::semibold())).color(p.text));
                ui.add_space(6.0);
                ui.label(RichText::new(t("Nytt namn på symbolen (ändras överallt servern hittar den).")).color(p.muted));
                ui.add_space(12.0);
                let field = ui.add(TextEdit::singleline(&mut r.name).desired_width(f32::INFINITY).margin(Margin::symmetric(10, 7)));
                if r.focus {
                    field.request_focus();
                    r.focus = false;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    choice = Some(true);
                }
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let ok = egui::Button::new(RichText::new(t("Byt namn")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .min_size(vec2(96.0, 32.0));
                    if ui.add(ok).clicked() {
                        choice = Some(true);
                    }
                    if ui.add(quiet_button(t("Avbryt"), p).min_size(vec2(0.0, 32.0))).clicked() {
                        choice = Some(false);
                    }
                });
            });
        if modal.should_close() && choice.is_none() {
            choice = Some(false);
        }
        let Some(ok) = choice else { return };
        let Some(r) = self.lsp.ui.rename.take() else { return };
        let name = r.name.trim().to_owned();
        if ok
            && !name.is_empty()
            && let Some(i) = self.index_of(r.doc_id)
        {
            self.lsp.rename(&self.docs[i], r.at, &name);
        }
    }

    fn lsp_settings_dialog(&mut self, ctx: &Context, p: &Palette) {
        if !self.lsp.ui.settings_open {
            return;
        }
        let mut close = false;
        let mut restart = false;
        let modal = egui::Modal::new(Id::unique("kladd-lsp-settings"))
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
                ui.set_width(560.0);
                ui.label(RichText::new(t("Språkservrar")).font(FontId::new(17.0, theme::semibold())).color(p.text));
                ui.add_space(6.0);
                ui.label(
                    RichText::new(t("Kladd startar en språkserver (LSP) när en kodfil öppnas och visar fel, förslag och mer. Lämna kommandot tomt för standardvalet."))
                        .color(p.muted),
                );
                ui.add_space(10.0);
                ui.checkbox(&mut self.settings.lsp, t("Använd språkservrar"));
                ui.add_space(10.0);
                for lang in config::LANGS {
                    ui.horizontal(|ui| {
                        ui.add_sized([150.0, 24.0], egui::Label::new(RichText::new(config::lang_name(lang)).color(p.text)));
                        let mut cmd = self.settings.lsp_commands.iter().find(|(k, _)| k == lang.key).map(|(_, c)| c.clone()).unwrap_or_default();
                        let r = ui.add(
                            TextEdit::singleline(&mut cmd)
                                .hint_text(config::default_text(lang))
                                .font(FontId::new(13.0, FontFamily::Monospace))
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 5)),
                        );
                        if r.changed() {
                            self.settings.lsp_commands.retain(|(k, _)| k != lang.key);
                            if !cmd.trim().is_empty() {
                                self.settings.lsp_commands.push((lang.key.to_owned(), cmd));
                            }
                        }
                    });
                    let found = config::command_for(lang, &self.settings.lsp_commands);
                    let (txt, col) = match found {
                        Some(c) => (tf!("Hittad: {path}", path = c[0]), p.muted),
                        None => (t("Hittades inte på PATH").to_owned(), p.error),
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(150.0 + 8.0);
                        ui.label(RichText::new(txt).small().color(col));
                    });
                    ui.add_space(4.0);
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let ok = egui::Button::new(RichText::new(t("Klar")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .min_size(vec2(84.0, 32.0));
                    if ui.add(ok).clicked() {
                        close = true;
                    }
                    if ui.add(quiet_button(t("Starta om servrar"), p).min_size(vec2(0.0, 32.0))).clicked() {
                        restart = true;
                    }
                });
            });
        if modal.should_close() {
            close = true;
        }
        if restart {
            self.lsp.restart_all();
        }
        if close {
            self.lsp.ui.settings_open = false;
        }
    }
}
