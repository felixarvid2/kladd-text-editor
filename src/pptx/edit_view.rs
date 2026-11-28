//! Redigeringsläget i presentationsvyn: markera, flytta, ändra storlek och vrida former med
//! handtag, ta bort, duplicera och ordna; skriva i former (markör och markering ur textmotorn);
//! bilder i sidopanelen (ny med layout, duplicera, ta bort, dölja, flytta); anteckningar; infoga
//! textruta och bild; ångra/gör om och sparande. Ändringarna görs i `edit.rs`/`edit_ops.rs`.
//!
//! Medan en form dras ritas bara dess kontur; ändringen görs när musen släpps. Text ändras direkt
//! (en ombyggnad per tangent) och bilden renderas om i bakgrunden.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::Arc;
use std::time::Duration;

use egui::{Align2, Color32, CursorIcon, Event, FontFamily, FontId, Id, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

use super::edit::Session;
use super::edit_ops::{self, Kind, Order, ShapeInfo, Xf};
use super::edit_text::{Carets, Pos, TextModel};
use super::edit_xml;
use super::view::PptxTab;
use super::Deck;
use crate::i18n::t;
use crate::theme::Palette;

/// En markerad form.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sel {
    pub slide: usize,
    pub index: usize,
    pub id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grip {
    Move,
    /// Handtag 0–7 medurs från övre vänstra hörnet.
    Handle(u8),
    Rotate,
}

struct Drag {
    grip: Grip,
    shape: ShapeInfo,
    /// De andra markerade formerna som flyttas med (bara vid `Grip::Move`).
    others: Vec<ShapeInfo>,
    start: (f32, f32),
    cur: Xf,
}

/// Text under redigering.
pub struct TextState {
    pub sel: Sel,
    pub model: TextModel,
    pub caret: Pos,
    pub anchor: Pos,
    pub(super) carets: Option<Carets>,
    /// Cellen i en tabell när texten är en cells.
    pub cell: Option<super::edit_table::CellAddr>,
    /// Formen i gruppen (`p:cNvPr/@id`) när texten är en gruppforms; `sel` är då gruppen.
    pub child: Option<u32>,
    /// Dras en markering med musen?
    selecting: bool,
}

/// Redigeringstillståndet i en presentationsflik.
pub struct Editor {
    pub session: Session,
    /// Redigeringsläget är på (annars visning).
    pub on: bool,
    pub sel: Option<Sel>,
    drag: Option<Drag>,
    pub text: Option<TextState>,
    /// Fler markerade former förutom `sel` (flervalsmarkering).
    pub more: Vec<Sel>,
    /// Formen som ritas med nästa drag (`Infoga → Former`) och dragrutan under dragningen.
    pub tool: Option<super::edit_new::ShapeKind>,
    pub new_drag: Option<((f32, f32), (f32, f32))>,
    /// Dragrutan som markerar former: (start, nu) i bildens punkter.
    band: Option<((f32, f32), (f32, f32))>,
    /// Anteckningarna under redigering (bild, text).
    notes: Option<(usize, String)>,
    /// Meddelanden till appen (text, fel?).
    pub notices: Vec<(String, bool)>,
    picture: Option<Receiver<Option<PathBuf>>>,
    /// Miniatyr som dras i sidopanelen (bild, pekarens y).
    thumb_drag: Option<(usize, f32)>,
    /// Formerna på bilden (cache: presentationens adress, bild).
    shapes: Option<(usize, usize, Vec<ShapeInfo>)>,
    /// Redigeringslagrets id: det har tangentbordsfokus medan en form är markerad, så att Tab och
    /// pilarna går till redigeringen och inte flyttar fokus mellan knapparna.
    overlay: Option<Id>,
    /// Formatraden (storleksfält, senast använd färg, formatet för markerad form).
    pub(super) fmt: super::edit_bar::FormatBar,
    /// Fönstret för diagramdata (`edit_chart_ui.rs`).
    pub chart: Option<super::edit_chart_ui::ChartDialog>,
}

impl Editor {
    pub fn new(source: Vec<u8>, deck: &Deck) -> Editor {
        Editor { session: Session::new(source, deck), on: false, sel: None, drag: None, text: None, more: Vec::new(), tool: None, new_drag: None, band: None, notes: None, notices: Vec::new(), picture: None, thumb_drag: None, shapes: None, overlay: None, fmt: Default::default(), chart: None }
    }
}

/// Handtagens storlek (punkter på skärmen).
const HANDLE: f32 = 7.0;
/// Rotationshandtagets avstånd ovanför formen.
const ROT_GAP: f32 = 22.0;
/// Pilstegen i EMU (1 pt, med Skift 10 pt).
const NUDGE: i64 = 12_700;

/// Ledtexten för en tom platshållare.
fn prompt(ph: &str) -> &'static str {
    match ph {
        "title" | "ctrTitle" => t("Klicka för att lägga till rubrik"),
        "subTitle" => t("Klicka för att lägga till underrubrik"),
        _ => t("Klicka för att lägga till text"),
    }
}

impl PptxTab {
    /// Är redigeringsläget på?
    pub fn editing(&self) -> bool {
        self.edit.on
    }

    /// Finns osparade ändringar?
    pub fn unsaved(&self) -> bool {
        self.edit.session.dirty()
    }

    /// Slår på/av redigeringsläget.
    pub fn toggle_edit(&mut self) {
        self.edit.on = !self.edit.on;
        if !self.edit.on {
            self.end_text();
            self.edit.sel = None;
            self.edit.more.clear();
            self.edit.drag = None;
            self.edit.chart = None;
        }
    }

    /// Alla markerade former på aktuell bild (den första markerade först).
    pub fn selection(&self) -> Vec<Sel> {
        self.edit.sel.into_iter().chain(self.edit.more.iter().copied()).filter(|s| s.slide == self.current).collect()
    }

    /// Redigeringslagrets id (för att skilja dess fokus från textfältens).
    pub(super) fn edit_focus_id(&self) -> Option<Id> {
        self.edit.overlay
    }

    /// Formerna på bild `slide` (cachade per presentation).
    pub(super) fn shapes_on(&mut self, slide: usize) -> Vec<ShapeInfo> {
        let key = Arc::as_ptr(&self.deck) as usize;
        if let Some((k, s, v)) = &self.edit.shapes {
            if *k == key && *s == slide {
                return v.clone();
            }
        }
        let v = edit_ops::shapes(&self.deck, slide);
        self.edit.shapes = Some((key, slide, v.clone()));
        v
    }

    fn shape_of(&mut self, sel: Sel) -> Option<ShapeInfo> {
        self.shapes_on(sel.slide).into_iter().find(|s| s.index == sel.index && s.id == sel.id)
    }

    /// Gör en ändring och byter till den nya presentationen. Falskt om inget ändrades eller det
    /// inte gick (felet blir en notis).
    pub fn apply(&mut self, merge: Option<String>, f: impl FnOnce(&mut super::edit::Txn) -> Result<(), String>) -> bool {
        let deck = self.deck.clone();
        match self.edit.session.apply(&deck, merge, f) {
            Ok(Some(a)) => {
                if a.signature_removed {
                    self.edit.notices.push((t("Presentationen var digitalt signerad. Signaturen togs bort eftersom filen ändrades.").to_owned(), false));
                }
                self.set_deck(a.deck);
                true
            }
            Ok(None) => false,
            Err(e) => {
                self.edit.notices.push((e, true));
                false
            }
        }
    }

    /// Byter presentation efter en ändring: bilder som ändrats renderas om, registret och
    /// anteckningarna läses om.
    pub(super) fn set_deck(&mut self, deck: Deck) {
        let old = self.deck.clone();
        let same_layout = old.slides.len() == deck.slides.len() && old.slides.iter().zip(&deck.slides).all(|(a, b)| a.part == b.part);
        let mut changed: Vec<usize> = if same_layout {
            (0..deck.slides.len())
                .filter(|&i| {
                    let p = &deck.slides[i].part;
                    match (old.pkg.parts.get(p), deck.pkg.parts.get(p)) {
                        (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                        _ => true,
                    }
                })
                .collect()
        } else {
            (0..deck.slides.len()).collect()
        };
        // Bilder vars diagram ändrats (diagramdelen är en egen del).
        if same_layout {
            changed.extend(super::edit_chart::slides_with_changed_parts(&old, &deck).into_iter().filter(|i| !changed.contains(i)).collect::<Vec<_>>());
        }
        self.deck = Arc::new(deck);
        self.invalidate(&changed, !same_layout);
        self.notes_changed();
        let n = self.count();
        if n > 0 && self.current >= n {
            self.current = n - 1;
        }
        if let Some(s) = self.edit.sel {
            if s.slide >= n {
                self.edit.sel = None;
            }
        }
        // Markörlägena gäller den nya presentationen.
        if let Some(ts) = &mut self.edit.text {
            ts.carets = match (ts.cell, ts.child) {
                (_, Some(child)) => Self::child_carets(&self.deck, ts.sel, child, &ts.model),
                (Some(addr), _) => ts.model.cell_carets(&self.deck, ts.sel.slide, ts.sel.index, addr),
                _ => edit_ops::node(&self.deck, ts.sel.slide, ts.sel.index).and_then(|node| ts.model.carets(&self.deck, ts.sel.slide, node)),
            };
        }
    }

    pub fn undo(&mut self, redo: bool) {
        self.end_text();
        self.edit.drag = None;
        let deck = self.deck.clone();
        let r = if redo { self.edit.session.redo(&deck) } else { self.edit.session.undo(&deck) };
        match r {
            Ok(Some(d)) => {
                self.set_deck(d);
                // Markeringen kan ha försvunnit.
                if let Some(s) = self.edit.sel {
                    if self.shape_of(s).is_none() {
                        self.edit.sel = None;
                    }
                }
                self.edit.notes = None;
            }
            Ok(None) => {}
            Err(e) => self.edit.notices.push((e, true)),
        }
    }

    /// Filens innehåll för att spara som `ext` (filen hade `from_ext`).
    pub fn save_bytes(&mut self, ext: &str, from_ext: &str) -> Result<Vec<u8>, String> {
        // Man kan fortsätta skriva efter sparningen (också den automatiska); nästa tangent blir ett nytt ångra-steg.
        self.edit.session.break_merge();
        let deck = self.deck.clone();
        let (bytes, rebuilt) = self.edit.session.save_bytes(&deck, ext, from_ext)?;
        if let Some(d) = rebuilt {
            self.set_deck(d);
        }
        Ok(bytes)
    }

    pub fn mark_saved(&mut self, bytes: Vec<u8>) {
        self.edit.session.mark_saved(bytes);
    }

    // ---------------------------------------------------------------- text

    /// Formens text som modell, och dess markörlägen. `None` om formen inte kan få text.
    pub(super) fn text_of(&mut self, sel: Sel) -> Option<(TextModel, Option<Carets>)> {
        let info = self.shape_of(sel)?;
        if info.kind != Kind::Shape || info.alternate {
            return None;
        }
        let bytes = self.deck.pkg.read(&self.deck.slides[sel.slide].part).map(<[u8]>::to_vec)?;
        let Some(doc) = edit_xml::parse(&bytes) else {
            self.edit.notices.push((t("Bilden kan inte ändras (XML som inte är UTF-8).").to_owned(), true));
            return None;
        };
        let sp = edit_ops::raw_shape(&doc, sel.index)?;
        let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
        let model = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), &a);
        let carets = edit_ops::node(&self.deck, sel.slide, sel.index).and_then(|n| model.carets(&self.deck, sel.slide, n));
        Some((model, carets))
    }

    /// Börjar skriva i formen (markören sist, eller vid punkten).
    pub fn begin_text(&mut self, sel: Sel, at: Option<(f32, f32)>) {
        let Some((model, carets)) = self.text_of(sel) else { return };
        let mut caret = model.end();
        if let (Some(c), Some((x, y))) = (&carets, at) {
            if let Some(p) = c.hit(x, y) {
                caret = p;
            }
        }
        self.edit.sel = Some(sel);
        self.edit.text = Some(TextState { sel, model, caret, anchor: caret, carets, selecting: false, cell: None, child: None });
    }

    /// Börjar skriva i en form i en grupp (`child` = formens `p:cNvPr/@id`).
    pub fn begin_child(&mut self, sel: Sel, child: u32, at: Option<(f32, f32)>) {
        let Some(bytes) = self.deck.pkg.read(&self.deck.slides[sel.slide].part).map(<[u8]>::to_vec) else { return };
        let Some(doc) = edit_xml::parse(&bytes) else { return };
        let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
        let Some(model) = super::edit_group::child_model(&self.deck, &doc, sel.slide, sel.index, child, &a) else { return };
        let carets = Self::child_carets(&self.deck, sel, child, &model);
        self.end_text();
        let mut caret = model.end();
        if let (Some(c), Some((x, y))) = (&carets, at) {
            if let Some(p) = c.hit(x, y) {
                caret = p;
            }
        }
        self.edit.sel = Some(sel);
        self.edit.text = Some(TextState { sel, model, caret, anchor: caret, carets, selecting: false, cell: None, child: Some(child) });
    }

    fn child_carets(deck: &Deck, sel: Sel, child: u32, model: &TextModel) -> Option<Carets> {
        let (node, parent) = super::edit_group::find_child(deck, sel.slide, sel.index, child)?;
        model.clone().in_parent(parent).carets(deck, sel.slide, node)
    }

    /// Börjar skriva i en tabellcell (markören vid punkten, annars sist).
    pub fn begin_cell(&mut self, sel: Sel, addr: super::edit_table::CellAddr, at: Option<(f32, f32)>) {
        if !super::edit_table::is_editable_table(&self.deck, sel.slide, sel.index) {
            return;
        }
        let Some(bytes) = self.deck.pkg.read(&self.deck.slides[sel.slide].part).map(<[u8]>::to_vec) else { return };
        let Some(doc) = edit_xml::parse(&bytes) else { return };
        let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
        let Some(model) = super::edit_table::cell_model(&doc, sel.index, addr, &a) else { return };
        let carets = model.cell_carets(&self.deck, sel.slide, sel.index, addr);
        // Bara en cell åt gången: det som skrevs i den förra är redan skrivet.
        self.end_text();
        let mut caret = model.end();
        if let (Some(c), Some((x, y))) = (&carets, at) {
            if let Some(p) = c.hit(x, y) {
                caret = p;
            }
        }
        self.edit.sel = Some(sel);
        self.edit.text = Some(TextState { sel, model, caret, anchor: caret, carets, selecting: false, cell: Some(addr), child: None });
    }

    /// Räknar om `normAutofit`/`spAutoFit` för formen efter att den fått ny storlek (`xf`); skrivs med
    /// samma sammanslagningsnyckel som storleksändringen så att det blir ett ångra-steg.
    fn refit_after_resize(&mut self, slide: usize, index: usize, id: u32, xf: Xf, key: String) {
        let Some((model, _)) = self.text_of(Sel { slide, index, id }) else { return };
        let Some(node) = edit_ops::node(&self.deck, slide, index) else { return };
        // `f32::MAX` som tidigare höjd: en `spAutoFit`-form får exakt textens höjd (också när den blev för hög).
        let Some(fit) = model.autofit(&self.deck, slide, node, xf, Some(f32::MAX)) else { return };
        self.apply(Some(key), |txn| super::edit_fit::set_fit_only(txn, slide, index, id, fit, xf));
        self.edit.session.break_merge();
    }

    /// Står man i en tabellcell och klickar i en annan i samma tabell börjar den cellen redigeras.
    fn switch_cell(&mut self, x: f32, y: f32) -> bool {
        let Some(ts) = &self.edit.text else { return false };
        let sel = ts.sel;
        if let Some(cur) = ts.child {
            return match super::edit_group::child_at(&self.deck, sel.slide, sel.index, x, y) {
                Some(c) if c != cur => {
                    self.begin_child(sel, c, Some((x, y)));
                    true
                }
                _ => false,
            };
        }
        let Some(cur) = ts.cell else { return false };
        match super::edit_table::cell_at(&self.deck, sel.slide, sel.index, x, y) {
            Some(a) if a != cur => {
                self.begin_cell(sel, a, Some((x, y)));
                true
            }
            _ => false,
        }
    }

    /// Slutar skriva (formen förblir markerad).
    pub fn end_text(&mut self) {
        if self.edit.text.take().is_some() {
            self.edit.session.break_merge();
        }
    }

    /// Skriver modellen till bilden (ett ångra-steg per textruta och skrivstund).
    pub(super) fn commit_text(&mut self) {
        let Some(ts) = &self.edit.text else { return };
        let (sel, model, before) = (ts.sel, ts.model.clone(), ts.carets.as_ref().map(|c| c.text_h));
        if let Some(addr) = ts.cell {
            self.apply(Some(format!("text:{}:{}:{}:{}", sel.slide, sel.id, addr.0, addr.1)), |txn| super::edit_table::set_cell_text(txn, sel.slide, sel.index, sel.id, addr, &model));
            return;
        }
        if let Some(child) = ts.child {
            // Autoanpassning (bara `a:normAutofit`) räknas om som för vanliga former.
            let fit = super::edit_group::find_child(&self.deck, sel.slide, sel.index, child).and_then(|(n, parent)| {
                let xf = edit_ops::shapes(&self.deck, sel.slide).into_iter().find(|s| s.index == sel.index).map(|s| s.xf)?;
                match model.clone().in_parent(parent).autofit(&self.deck, sel.slide, n, xf, before) {
                    Some(f @ super::edit_fit::Fit::Norm { .. }) => Some(f),
                    _ => None,
                }
            });
            self.apply(Some(format!("text:{}:{}:g{child}", sel.slide, sel.id)), |txn| super::edit_group::set_child_text(txn, sel.slide, sel.index, sel.id, child, &model, fit));
            return;
        }
        let Some(info) = self.shape_of(sel) else { return };
        // Autoanpassningen räknas om här och bara här: när texten ändras (inte när filen öppnas eller sparas).
        let fit = edit_ops::node(&self.deck, sel.slide, sel.index).and_then(|n| model.autofit(&self.deck, sel.slide, n, info.xf, before));
        self.apply(Some(format!("text:{}:{}", sel.slide, sel.id)), |txn| super::edit_fit::set_text_fit(txn, sel.slide, sel.index, sel.id, &model, fit, info.xf));
    }

    /// Tangenter och text medan man skriver i en form. Händelserna som hanteras tas bort ur kön.
    fn text_keys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let mut handled = Vec::new();
        let mut tab_cell = None;
        for (k, ev) in events.iter().enumerate() {
            let Some(ts) = &mut self.edit.text else { break };
            let mut changed = false;
            let sel_range = (ts.anchor.min(ts.caret), ts.anchor.max(ts.caret));
            let has_sel = ts.anchor != ts.caret;
            match ev {
                Event::Text(s) | Event::Paste(s) if !s.is_empty() => {
                    let s = s.replace("\r\n", "\n");
                    let at = if has_sel { ts.model.delete(sel_range.0, sel_range.1) } else { ts.caret };
                    ts.caret = ts.model.insert(at, &s);
                    ts.anchor = ts.caret;
                    changed = true;
                    handled.push(k);
                }
                Event::Ime(egui::ImeEvent::Commit(s)) if !s.is_empty() => {
                    let at = if has_sel { ts.model.delete(sel_range.0, sel_range.1) } else { ts.caret };
                    ts.caret = ts.model.insert(at, s);
                    ts.anchor = ts.caret;
                    changed = true;
                    handled.push(k);
                }
                Event::Copy | Event::Cut => {
                    if has_sel {
                        ctx.copy_text(ts.model.slice(sel_range.0, sel_range.1));
                        if matches!(ev, Event::Cut) {
                            ts.caret = ts.model.delete(sel_range.0, sel_range.1);
                            ts.anchor = ts.caret;
                            changed = true;
                        }
                    }
                    handled.push(k);
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    let m = *modifiers;
                    let shift = m.shift;
                    let word = m.command || m.alt && cfg!(target_os = "macos");
                    let mv = |ts: &mut TextState, p: Pos| {
                        ts.caret = p;
                        if !shift {
                            ts.anchor = p;
                        }
                    };
                    match key {
                        Key::ArrowLeft | Key::ArrowRight => {
                            let fwd = *key == Key::ArrowRight;
                            let p = if has_sel && !shift {
                                if fwd { sel_range.1 } else { sel_range.0 }
                            } else if word {
                                ts.model.word(ts.caret, fwd)
                            } else {
                                ts.model.step(ts.caret, fwd)
                            };
                            mv(ts, p);
                            handled.push(k);
                        }
                        Key::ArrowUp | Key::ArrowDown => {
                            let dir = if *key == Key::ArrowUp { -1 } else { 1 };
                            let p = ts.carets.as_ref().and_then(|c| c.vertical(ts.caret, dir)).unwrap_or(if dir < 0 { Pos::default() } else { ts.model.end() });
                            mv(ts, p);
                            handled.push(k);
                        }
                        Key::Home | Key::End => {
                            let end = *key == Key::End;
                            let p = if m.command {
                                if end { ts.model.end() } else { Pos::default() }
                            } else {
                                ts.carets.as_ref().and_then(|c| c.line_edge(ts.caret, end)).unwrap_or(ts.caret)
                            };
                            mv(ts, p);
                            handled.push(k);
                        }
                        Key::Backspace | Key::Delete => {
                            let fwd = *key == Key::Delete;
                            let (a, b) = if has_sel {
                                sel_range
                            } else {
                                let o = if word { ts.model.word(ts.caret, fwd) } else { ts.model.step(ts.caret, fwd) };
                                (ts.caret.min(o), ts.caret.max(o))
                            };
                            ts.caret = ts.model.delete(a, b);
                            ts.anchor = ts.caret;
                            changed = true;
                            handled.push(k);
                        }
                        Key::Enter => {
                            let at = if has_sel { ts.model.delete(sel_range.0, sel_range.1) } else { ts.caret };
                            ts.caret = if shift { ts.model.insert_break(at) } else { ts.model.split_para(at) };
                            ts.anchor = ts.caret;
                            changed = true;
                            handled.push(k);
                        }
                        Key::Tab if !m.command && ts.cell.is_some() => {
                            // I en tabell går Tab till nästa cell och Skift+Tab till föregående.
                            tab_cell = Some((ts.sel, ts.cell.unwrap_or_default(), shift));
                            handled.push(k);
                        }
                        Key::Tab if !m.command => {
                            // Nivå i punktlistor när markören står först i stycket eller flera stycken är markerade.
                            if shift || has_sel && sel_range.0.para != sel_range.1.para || ts.caret.ch == 0 {
                                ts.model.indent(sel_range.0, sel_range.1, if shift { -1 } else { 1 });
                            } else {
                                ts.caret = ts.model.insert(ts.caret, "\t");
                                ts.anchor = ts.caret;
                            }
                            changed = true;
                            handled.push(k);
                        }
                        Key::L | Key::E | Key::R | Key::J if m.command && !shift && !m.alt => {
                            let algn = match key {
                                Key::L => "l",
                                Key::E => "ctr",
                                Key::R => "r",
                                _ => "just",
                            };
                            ts.model.set_align(ts.anchor, ts.caret, algn);
                            changed = true;
                            handled.push(k);
                        }
                        // Större/mindre text: Ctrl+Skift+> och Ctrl+Skift+< (och Ctrl+] och Ctrl+[, som
                        // också finns i PowerPoint och inte beror på tangentbordslayouten).
                        Key::Period | Key::Comma | Key::IntlBackslash | Key::CloseBracket | Key::OpenBracket if m.command && (shift || matches!(key, Key::CloseBracket | Key::OpenBracket | Key::IntlBackslash)) => {
                            // `<>`-tangenten på nordiska tangentbord är IntlBackslash: `>` med Skift, `<` utan.
                            let up = matches!(key, Key::Period | Key::CloseBracket) || *key == Key::IntlBackslash && shift;
                            let fmts = ts.carets.as_ref().map(|c| c.paras.clone()).unwrap_or_default();
                            ts.model.step_size(ts.anchor, ts.caret, up, &fmts);
                            changed = true;
                            handled.push(k);
                        }
                        Key::A if m.command => {
                            ts.anchor = Pos::default();
                            ts.caret = ts.model.end();
                            handled.push(k);
                        }
                        Key::B | Key::I | Key::U if m.command && !shift => {
                            let attr = match key {
                                Key::B => "b",
                                Key::I => "i",
                                _ => "u",
                            };
                            ts.model.toggle(ts.anchor, ts.caret, attr);
                            changed = true;
                            handled.push(k);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            if changed {
                self.commit_text();
            }
        }
        if !handled.is_empty() {
            ctx.input_mut(|i| {
                let mut k = 0;
                i.events.retain(|_| {
                    let keep = !handled.contains(&k);
                    k += 1;
                    keep
                });
            });
        }
        if let Some((sel, from, back)) = tab_cell {
            if let Some(next) = super::edit_table::next_cell(&self.deck, sel.slide, sel.index, from, back) {
                self.begin_cell(sel, next, None);
            } else if !back && super::edit_table::is_last_cell(&self.deck, sel.slide, sel.index, from) {
                // Tab i sista cellen lägger till en rad (som i PowerPoint) och går till dess första cell.
                self.end_text();
                let mut row = None;
                if self.apply(None, |txn| {
                    row = Some(super::edit_table::add_row(txn, sel.slide, sel.index, sel.id)?);
                    Ok(())
                }) {
                    if let Some(r) = row {
                        self.begin_cell(sel, (r, 0), None);
                    }
                }
            }
        }
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if key(Modifiers::NONE, Key::Escape) {
            self.end_text();
        }
    }

    // ---------------------------------------------------------------- tangenter

    /// Tangenterna i redigeringsläget. Körs före vyns egna; tangenter som används tas bort.
    pub(super) fn edit_keys(&mut self, ctx: &egui::Context) {
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let cmd = Modifiers::COMMAND;
        // Ctrl+E centrerar när man skriver i en form.
        if self.edit.text.is_none() && key(cmd, Key::E) {
            self.toggle_edit();
            return;
        }
        let overlay = self.edit.overlay;
        if !self.edit.on || self.edit.chart.is_some() || ctx.memory(|m| m.focused().is_some_and(|f| Some(f) != overlay)) {
            return;
        }
        // Ctrl+Skift+Z före Ctrl+Z: `consume_key` bryr sig inte om extra Skift.
        if key(cmd | Modifiers::SHIFT, Key::Z) || key(cmd, Key::Y) {
            self.undo(true);
            return;
        }
        if key(cmd, Key::Z) {
            self.undo(false);
            return;
        }
        if key(cmd, Key::M) {
            let layout = self.deck.slides.get(self.current).and_then(|s| s.layout).unwrap_or(0);
            self.new_slide(layout);
            return;
        }
        if self.edit.text.is_some() {
            self.text_keys(ctx);
            return;
        }
        let shapes = self.shapes_on(self.current);
        let sel = self.edit.sel.filter(|s| s.slide == self.current);
        if key(Modifiers::SHIFT, Key::Tab) || key(Modifiers::NONE, Key::Tab) {
            let back = ctx.input(|i| i.modifiers.shift);
            if !shapes.is_empty() {
                let pos = sel.and_then(|s| shapes.iter().position(|x| x.index == s.index));
                let n = shapes.len();
                let next = match pos {
                    Some(p) if back => (p + n - 1) % n,
                    Some(p) => (p + 1) % n,
                    None if back => n - 1,
                    None => 0,
                };
                let s = &shapes[next];
                self.edit.sel = Some(Sel { slide: self.current, index: s.index, id: s.id });
                self.edit.more.clear();
            }
            return;
        }
        if key(cmd, Key::A) {
            // Markera alla former på bilden.
            let all: Vec<Sel> = shapes.iter().map(|s| Sel { slide: self.current, index: s.index, id: s.id }).collect();
            self.edit.sel = all.first().copied();
            self.edit.more = all.iter().skip(1).copied().collect();
            return;
        }
        let Some(sel) = sel else { return };
        if key(Modifiers::NONE, Key::Escape) {
            self.edit.sel = None;
            self.edit.more.clear();
            return;
        }
        if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
            self.delete_selected();
            return;
        }
        if key(cmd, Key::D) {
            self.duplicate_selected();
            return;
        }
        if key(Modifiers::NONE, Key::Enter) || key(Modifiers::NONE, Key::F2) {
            if super::edit_table::is_editable_table(&self.deck, sel.slide, sel.index) {
                self.begin_cell(sel, (0, 0), None);
                return;
            }
            self.begin_text(sel, None);
            return;
        }
        let mut d = (0i64, 0i64);
        for (k, v) in [(Key::ArrowLeft, (-1, 0)), (Key::ArrowRight, (1, 0)), (Key::ArrowUp, (0, -1)), (Key::ArrowDown, (0, 1))] {
            if key(Modifiers::SHIFT, k) {
                d = (v.0 * 10, v.1 * 10);
            } else if key(Modifiers::NONE, k) {
                d = v;
            }
        }
        if d != (0, 0) {
            let infos = self.selected_infos();
            if !infos.is_empty() {
                self.apply(Some(format!("nudge:{}:{}", sel.slide, sel.id)), |txn| {
                    for i in &infos {
                        let xf = Xf { x: i.xf.x + d.0 * NUDGE, y: i.xf.y + d.1 * NUDGE, ..i.xf };
                        edit_ops::set_xf(txn, sel.slide, i.index, i.id, xf)?;
                    }
                    Ok(())
                });
            }
        }
    }

    // ---------------------------------------------------------------- åtgärder

    /// De markerade formernas nuvarande läge (index kan ha ändrats): (form, index) i bildordning.
    fn selected_infos(&mut self) -> Vec<ShapeInfo> {
        let sel = self.selection();
        let Some(slide) = sel.first().map(|s| s.slide) else { return Vec::new() };
        self.shapes_on(slide).into_iter().filter(|i| sel.iter().any(|s| s.id == i.id && s.index == i.index)).collect()
    }

    pub fn delete_selected(&mut self) {
        let infos = self.selected_infos();
        let Some(slide) = self.selection().first().map(|s| s.slide) else { return };
        self.end_text();
        // Bakifrån: ett borttaget index flyttar bara formerna efter det.
        if self.apply(None, |txn| {
            for i in infos.iter().rev() {
                edit_ops::delete_shape(txn, slide, i.index, i.id)?;
            }
            Ok(())
        }) {
            self.edit.sel = None;
            self.edit.more.clear();
        }
    }

    pub fn duplicate_selected(&mut self) {
        let infos = self.selected_infos();
        let Some(slide) = self.selection().first().map(|s| s.slide) else { return };
        self.end_text();
        let mut new = Vec::new();
        let before_ids: std::collections::HashSet<u32> = self.shapes_on(slide).iter().map(|s| s.id).collect();
        if self.apply(None, |txn| {
            // Bakifrån, så att lägena före de som dupliceras står kvar.
            for i in infos.iter().rev() {
                new.push(edit_ops::duplicate_shape(txn, slide, i.index, i.id)?);
            }
            Ok(())
        }) {
            // Dubbletterna (de nya id:n) blir markerade; lägena räknas om ur ritmodellen.
            let shapes = self.shapes_on(slide);
            let fresh: Vec<Sel> = shapes.iter().filter(|s| !before_ids.contains(&s.id)).map(|s| Sel { slide, index: s.index, id: s.id }).collect();
            self.edit.sel = fresh.first().copied();
            self.edit.more = fresh.iter().skip(1).copied().collect();
        }
    }

    pub fn reorder_selected(&mut self, order: Order) {
        let mut infos = self.selected_infos();
        let Some(slide) = self.selection().first().map(|s| s.slide) else { return };
        self.end_text();
        // Längst fram och bakåt: nedersta först; framåt och längst bak: översta först. Då behåller
        // formerna sin ordning sinsemellan.
        if matches!(order, Order::Forward | Order::Back) {
            infos.reverse();
        }
        let ids: Vec<u32> = infos.iter().map(|i| i.id).collect();
        for id in &ids {
            let Some(cur) = self.shapes_on(slide).into_iter().find(|s| s.id == *id) else { continue };
            self.apply(Some(format!("order:{slide}")), |txn| edit_ops::reorder_shape(txn, slide, cur.index, cur.id, order));
        }
        let shapes = self.shapes_on(slide);
        let now: Vec<Sel> = ids.iter().filter_map(|id| shapes.iter().find(|s| s.id == *id)).map(|s| Sel { slide, index: s.index, id: s.id }).collect();
        self.edit.sel = now.first().copied();
        self.edit.more = now.iter().skip(1).copied().collect();
        self.edit.session.break_merge();
    }

    /// Ny bild efter aktuell med layouten.
    pub fn new_slide(&mut self, layout: usize) {
        self.end_text();
        let after = (self.count() > 0).then_some(self.current);
        let mut at = 0;
        if self.apply(None, |txn| {
            at = edit_ops::new_slide(txn, after, layout)?;
            Ok(())
        }) {
            self.edit.sel = None;
            self.go_to(at);
        }
    }

    pub fn duplicate_slide(&mut self, i: usize) {
        self.end_text();
        if self.apply(None, |txn| edit_ops::duplicate_slide(txn, i).map(|_| ())) {
            self.go_to(i + 1);
        }
    }

    pub fn delete_slide(&mut self, i: usize) {
        self.end_text();
        self.edit.sel = None;
        if self.apply(None, |txn| edit_ops::delete_slide(txn, i)) {
            self.go_to(i.min(self.count().saturating_sub(1)));
        }
    }

    pub fn move_slide(&mut self, from: usize, to: usize) {
        self.end_text();
        self.edit.sel = None;
        if self.apply(None, |txn| edit_ops::move_slide(txn, from, to)) {
            self.go_to(to);
        }
    }

    pub fn toggle_hidden(&mut self, i: usize) {
        let hidden = self.deck.slides.get(i).is_some_and(|s| s.hidden);
        self.apply(None, |txn| edit_ops::set_hidden(txn, i, !hidden));
    }

    /// Ny textruta mitt på bilden; man skriver direkt i den.
    pub fn insert_textbox(&mut self) {
        self.end_text();
        let slide = self.current;
        let (w, h) = self.deck.size_emu;
        let xf = Xf { x: w / 4, y: h * 2 / 5, cx: w / 2, cy: 457_200, ..Default::default() };
        let mut idx = 0;
        if self.apply(None, |txn| {
            idx = edit_ops::insert_textbox(txn, slide, xf)?;
            Ok(())
        }) {
            self.edit.on = true;
            if let Some(s) = self.shapes_on(slide).into_iter().find(|s| s.index == idx) {
                self.begin_text(Sel { slide, index: s.index, id: s.id }, None);
            }
        }
    }

    /// Frågar efter en bild (PNG/JPEG/GIF) att infoga på aktuell bild.
    pub fn begin_insert_picture(&mut self, ctx: &egui::Context, dir: Option<PathBuf>) {
        if self.edit.picture.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.edit.picture = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new().set_title(t("Infoga bild")).add_filter(t("Bilder"), &["png", "jpg", "jpeg", "gif"]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let _ = tx.send(dlg.pick_file());
            ctx.request_repaint();
        });
    }

    /// Infogar en bild ur en fil.
    pub fn insert_picture_file(&mut self, path: &std::path::Path) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.edit.notices.push((crate::doc::describe_io_error(&e, path), true));
                return;
            }
        };
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let slide = self.current;
        let mut idx = 0;
        if self.apply(None, |txn| {
            idx = edit_ops::insert_picture(txn, slide, bytes, &name)?;
            Ok(())
        }) {
            self.edit.on = true;
            self.end_text();
            if let Some(s) = self.shapes_on(slide).into_iter().find(|s| s.index == idx) {
                self.edit.sel = Some(Sel { slide, index: s.index, id: s.id });
            }
        }
    }

    /// Tar emot bilddialogens svar och appens notiser.
    pub fn poll_edit(&mut self) -> Vec<(String, bool)> {
        if let Some(rx) = &self.edit.picture {
            match rx.try_recv() {
                Ok(Some(p)) => {
                    self.edit.picture = None;
                    self.insert_picture_file(&p);
                }
                Ok(None) | Err(TryRecvError::Disconnected) => self.edit.picture = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        std::mem::take(&mut self.edit.notices)
    }

    // ---------------------------------------------------------------- ritning och mus

    /// Redigeringslagret över bilden `slide` i `rect` (skärmkoordinater).
    pub(super) fn edit_overlay(&mut self, ui: &mut Ui, rect: Rect, slide: usize, p: &Palette) {
        self.chart_dialog(ui.ctx());
        if !self.edit.on {
            // Dubbelklick på en form startar redigeringen.
            let resp = ui.interact(rect, Id::unique(("kladd-pptx-view", slide)), Sense::click());
            if resp.double_clicked() {
                if let Some(pt) = resp.interact_pointer_pos() {
                    let (sw, _) = self.deck.size_pt();
                    let k = rect.width() / sw.max(1.0);
                    let (x, y) = ((pt.x - rect.left()) / k, (pt.y - rect.top()) / k);
                    self.edit.on = true;
                    if let Some(s) = self.shapes_on(slide).into_iter().rev().find(|s| s.hit(x, y, 2.0)) {
                        let sel = Sel { slide, index: s.index, id: s.id };
                        self.edit.sel = Some(sel);
                        if s.kind == Kind::Frame && super::edit_chart::is_chart(&self.deck, slide, s.index) {
                            self.open_chart(sel);
                        } else if s.kind == Kind::Shape && !s.alternate {
                            self.begin_text(sel, Some((x, y)));
                        } else if let Some(addr) = super::edit_table::cell_at(&self.deck, slide, s.index, x, y) {
                            self.begin_cell(sel, addr, Some((x, y)));
                        } else if let Some(c) = super::edit_group::child_at(&self.deck, slide, s.index, x, y) {
                            self.begin_child(sel, c, Some((x, y)));
                        }
                    }
                }
            }
            return;
        }
        let (sw, _) = self.deck.size_pt();
        let k = rect.width() / sw.max(1.0);
        let to_screen = |x: f32, y: f32| pos2(rect.left() + x * k, rect.top() + y * k);
        let to_slide = |pt: Pos2| ((pt.x - rect.left()) / k, (pt.y - rect.top()) / k);
        let shapes = self.shapes_on(slide);
        let id = Id::unique(("kladd-pptx-edit", slide));
        self.edit.overlay = Some(id);
        let resp = ui.interact(rect.expand(ROT_GAP + HANDLE), id, Sense::click_and_drag());
        if resp.is_pointer_button_down_on() {
            ui.memory_mut(|m| m.request_focus(id));
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let sel = self.edit.sel.filter(|s| s.slide == slide);
        let sel_info = sel.and_then(|s| shapes.iter().find(|x| x.index == s.index && x.id == s.id).cloned());
        let text_sel = self.edit.text.as_ref().map(|t| t.sel).filter(|s| s.slide == slide);

        // Vad ligger under pekaren?
        let grip_at = |pt: Pos2, info: &ShapeInfo| -> Option<Grip> {
            let corners = handle_points(info);
            for (i, (x, y)) in corners.iter().enumerate() {
                if to_screen(*x, *y).distance(pt) <= HANDLE + 2.0 && info.kind != Kind::Connector || to_screen(*x, *y).distance(pt) <= HANDLE + 2.0 && i % 2 == 0 {
                    return Some(Grip::Handle(i as u8));
                }
            }
            let (rx, ry) = rot_point(info, ROT_GAP / k);
            if info.kind != Kind::Frame && to_screen(rx, ry).distance(pt) <= HANDLE + 2.0 {
                return Some(Grip::Rotate);
            }
            None
        };
        let hit_shape = |pt: Pos2| -> Option<ShapeInfo> {
            let (x, y) = to_slide(pt);
            shapes.iter().rev().find(|s| s.hit(x, y, 3.0 / k)).cloned()
        };

        // Pekaren.
        if let Some(pt) = pointer.filter(|pt| rect.expand(ROT_GAP + HANDLE).contains(*pt)) {
            let cursor = match sel_info.as_ref().filter(|_| self.edit.more.is_empty()).and_then(|i| grip_at(pt, i)) {
                Some(Grip::Rotate) => Some(CursorIcon::Alias),
                Some(Grip::Handle(h)) => Some(handle_cursor(h, sel_info.as_ref().map_or(0.0, |i| i.xf.rot_deg()))),
                _ => match hit_shape(pt) {
                    Some(s) if Some(Sel { slide, index: s.index, id: s.id }) == text_sel => Some(CursorIcon::Text),
                    Some(_) => Some(CursorIcon::Move),
                    None => None,
                },
            };
            if let Some(c) = cursor {
                ui.ctx().set_cursor_icon(c);
            }
        }

        // Formverktyget (Infoga → Former) tar musen medan en form ska ritas.
        let tool_on = self.shape_tool_active();
        if tool_on {
            self.shape_tool(ui, &resp, rect, slide, p.accent);
        }
        // Musen.
        if !tool_on && resp.drag_started_by(PointerButton::Primary) {
            if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
                let start = to_slide(origin);
                let in_text = text_sel.and_then(|ts| shapes.iter().find(|s| s.index == ts.index)).is_some_and(|s| s.hit(start.0, start.1, 0.0));
                if in_text && self.switch_cell(start.0, start.1) {
                    // Ett klick i en annan cell: den börjar redigeras.
                } else if in_text {
                    if let Some(ts) = &mut self.edit.text {
                        if let Some(p) = ts.carets.as_ref().and_then(|c| c.hit(start.0, start.1)) {
                            ts.anchor = p;
                            ts.caret = p;
                        }
                        ts.selecting = true;
                    }
                } else {
                    let grip = sel_info.as_ref().filter(|_| self.edit.more.is_empty()).and_then(|i| grip_at(origin, i).map(|g| (g, i.clone())));
                    let target = match grip {
                        Some(g) => Some(g),
                        None => hit_shape(origin).map(|s| (Grip::Move, s)),
                    };
                    match target {
                        Some((grip, shape)) => {
                            if grip == Grip::Move {
                                self.end_text();
                            }
                            let me = Sel { slide, index: shape.index, id: shape.id };
                            let group: Vec<Sel> = self.selection();
                            // Dras en av flera markerade former flyttas alla; annars gäller bara den.
                            let mut others = Vec::new();
                            if grip == Grip::Move && group.len() > 1 && group.contains(&me) {
                                others = shapes.iter().filter(|i| i.id != shape.id && group.iter().any(|g| g.id == i.id && g.index == i.index)).cloned().collect();
                            } else {
                                self.edit.sel = Some(me);
                                self.edit.more.clear();
                            }
                            self.edit.drag = Some(Drag { grip, cur: shape.xf, shape, others, start });
                        }
                        None => {
                            self.end_text();
                            if !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
                                self.edit.sel = None;
                                self.edit.more.clear();
                            }
                            self.edit.band = Some((start, start));
                        }
                    }
                }
            }
        }
        if !tool_on && resp.dragged_by(PointerButton::Primary) {
            if let Some(pt) = ui.input(|i| i.pointer.interact_pos()) {
                let (x, y) = to_slide(pt);
                let shift = ui.input(|i| i.modifiers.shift);
                if let Some(ts) = self.edit.text.as_mut().filter(|t| t.selecting) {
                    if let Some(p) = ts.carets.as_ref().and_then(|c| c.hit(x, y)) {
                        ts.caret = p;
                    }
                } else if let Some(d) = &mut self.edit.drag {
                    d.cur = drag_xf(d, (x, y), shift);
                } else if let Some((_, end)) = &mut self.edit.band {
                    *end = (x, y);
                }
            }
        }
        if !tool_on && resp.drag_stopped() {
            if let Some(ts) = &mut self.edit.text {
                ts.selecting = false;
            }
            if let Some(d) = self.edit.drag.take() {
                if d.cur != d.shape.xf {
                    let (index, id, xf) = (d.shape.index, d.shape.id, d.cur);
                    let (dx, dy) = (xf.x - d.shape.xf.x, xf.y - d.shape.xf.y);
                    let others = d.others;
                    let resize = matches!(d.grip, Grip::Handle(_)) && d.shape.kind == Kind::Shape && !d.shape.alternate && d.shape.text;
                    let key = resize.then(|| format!("resize:{slide}:{id}"));
                    self.apply(key.clone(), |txn| {
                        edit_ops::set_xf(txn, slide, index, id, xf)?;
                        for o in &others {
                            edit_ops::set_xf(txn, slide, o.index, o.id, Xf { x: o.xf.x + dx, y: o.xf.y + dy, ..o.xf })?;
                        }
                        Ok(())
                    });
                    // Textens autoanpassning räknas om för den nya storleken, i samma ångra-steg.
                    if let Some(key) = key {
                        self.refit_after_resize(slide, index, id, xf, key);
                    }
                }
            }
            if let Some(((ax, ay), (bx, by))) = self.edit.band.take() {
                let (l, r, t, b) = (ax.min(bx), ax.max(bx), ay.min(by), ay.max(by));
                if (r - l) > 2.0 || (b - t) > 2.0 {
                    let hit: Vec<Sel> = shapes
                        .iter()
                        .filter(|s| {
                            let [x, y, w, h] = s.xf.rect_pt();
                            x < r && x + w > l && y < b && y + h > t
                        })
                        .map(|s| Sel { slide, index: s.index, id: s.id })
                        .collect();
                    let keep = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                    let mut all: Vec<Sel> = if keep { self.selection() } else { Vec::new() };
                    for h in hit {
                        if !all.contains(&h) {
                            all.push(h);
                        }
                    }
                    self.edit.sel = all.first().copied();
                    self.edit.more = all.iter().skip(1).copied().collect();
                }
            }
        }
        if !tool_on && resp.clicked_by(PointerButton::Primary) {
            if let Some(pt) = resp.interact_pointer_pos() {
                let (x, y) = to_slide(pt);
                let shift = ui.input(|i| i.modifiers.shift);
                match hit_shape(pt) {
                    Some(s) if Some(Sel { slide, index: s.index, id: s.id }) == text_sel => {
                        if self.switch_cell(x, y) {
                            // Ny cell.
                        } else if let Some(ts) = &mut self.edit.text {
                            if let Some(p) = ts.carets.as_ref().and_then(|c| c.hit(x, y)) {
                                ts.caret = p;
                                if !shift {
                                    ts.anchor = p;
                                }
                            }
                        }
                    }
                    Some(s) if ui.input(|i| i.modifiers.shift || i.modifiers.command) => {
                        // Skift/Ctrl+klick lägger till eller tar bort formen i markeringen.
                        let me = Sel { slide, index: s.index, id: s.id };
                        let mut all = self.selection();
                        match all.iter().position(|a| *a == me) {
                            Some(i) => {
                                all.remove(i);
                            }
                            None => all.push(me),
                        }
                        self.end_text();
                        self.edit.sel = all.first().copied();
                        self.edit.more = all.iter().skip(1).copied().collect();
                    }
                    Some(s) => {
                        let new = Sel { slide, index: s.index, id: s.id };
                        let again = self.edit.sel == Some(new) && self.edit.more.is_empty();
                        self.end_text();
                        self.edit.sel = Some(new);
                        self.edit.more.clear();
                        // Ett andra klick på en markerad form med text börjar skriva.
                        if again && s.kind == Kind::Shape && !s.alternate && (s.text || s.empty_ph) {
                            self.begin_text(new, Some((x, y)));
                        } else if again && s.kind == Kind::Frame {
                            if let Some(addr) = super::edit_table::cell_at(&self.deck, slide, s.index, x, y) {
                                self.begin_cell(new, addr, Some((x, y)));
                            }
                        }
                    }
                    None => {
                        self.end_text();
                        if !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
                            self.edit.sel = None;
                            self.edit.more.clear();
                        }
                    }
                }
            }
        }
        if !tool_on && resp.double_clicked() {
            if let Some(pt) = resp.interact_pointer_pos() {
                let (x, y) = to_slide(pt);
                let table = |me: &Self, s: &ShapeInfo| s.kind == Kind::Frame && super::edit_table::is_editable_table(&me.deck, slide, s.index) || s.kind == Kind::Group && !s.alternate && super::edit_group::is_group(&me.deck, slide, s.index);
                if let Some(s) = hit_shape(pt).filter(|s| s.kind == Kind::Frame && super::edit_chart::is_chart(&self.deck, slide, s.index)) {
                    self.end_text();
                    self.open_chart(Sel { slide, index: s.index, id: s.id });
                } else if let Some(s) = hit_shape(pt).filter(|s| s.kind == Kind::Shape && !s.alternate || table(self, s)) {
                    let sel = Sel { slide, index: s.index, id: s.id };
                    if text_sel != Some(sel) {
                        if s.kind == Kind::Frame {
                            if let Some(addr) = super::edit_table::cell_at(&self.deck, slide, s.index, x, y) {
                                self.begin_cell(sel, addr, Some((x, y)));
                            }
                        } else if s.kind == Kind::Group {
                            if let Some(c) = super::edit_group::child_at(&self.deck, slide, s.index, x, y) {
                                self.begin_child(sel, c, Some((x, y)));
                            }
                        } else {
                            self.begin_text(sel, Some((x, y)));
                        }
                    } else if self.switch_cell(x, y) {
                        // Ny cell.
                    } else if let Some(ts) = &mut self.edit.text {
                        // Dubbelklick i texten markerar ordet.
                        let a = ts.model.word(ts.caret, false);
                        let b = ts.model.word(ts.caret, true);
                        ts.anchor = Pos { para: ts.caret.para, ch: a.ch.min(ts.caret.ch) };
                        ts.caret = Pos { para: ts.caret.para, ch: if b.para == ts.caret.para { b.ch } else { ts.model.paras[ts.caret.para].len() } };
                    }
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(pt) = resp.interact_pointer_pos() {
                if let Some(s) = hit_shape(pt) {
                    let new = Sel { slide, index: s.index, id: s.id };
                    if text_sel != Some(new) {
                        self.end_text();
                    }
                    self.edit.sel = Some(new);
                }
            }
        }
        let has_sel = self.edit.sel.is_some_and(|s| s.slide == slide);
        resp.context_menu(|ui| {
            ui.set_min_width(220.0);
            ui.set_max_width(240.0);
            if !has_sel {
                if crate::app::menu_item(ui, p, t("Infoga textruta"), "", None, true) {
                    self.insert_textbox();
                    ui.close();
                }
                return;
            }
            if crate::app::menu_item(ui, p, t("Redigera text"), "Enter", None, true) {
                if let Some(sel) = self.edit.sel {
                    self.begin_text(sel, None);
                }
                ui.close();
            }
            crate::app::menu_divider(ui, p);
            for (label, order) in [(t("Längst fram"), Order::Front), (t("Framåt"), Order::Forward), (t("Bakåt"), Order::Backward), (t("Längst bak"), Order::Back)] {
                if crate::app::menu_item(ui, p, label, "", None, true) {
                    self.reorder_selected(order);
                    ui.close();
                }
            }
            crate::app::menu_divider(ui, p);
            if crate::app::menu_item(ui, p, t("Duplicera"), &crate::keys::shortcut("Ctrl+D"), None, true) {
                self.duplicate_selected();
                ui.close();
            }
            if crate::app::menu_item(ui, p, t("Ta bort"), "Delete", None, true) {
                self.delete_selected();
                ui.close();
            }
        });

        self.keep_focus(ui, id, slide);
        // Ritning.
        let painter = ui.painter_at(rect.expand(ROT_GAP + HANDLE + 2.0));
        let accent = p.accent;
        let outline = |painter: &egui::Painter, info: &ShapeInfo, xf: Xf, stroke: Stroke| {
            let pts: Vec<Pos2> = corners(info, xf).iter().map(|&(x, y)| to_screen(x, y)).collect();
            painter.add(Shape::closed_line(pts, stroke));
        };
        // Tomma platshållare: streckad ram och ledtext.
        for s in shapes.iter().filter(|s| s.empty_ph) {
            if text_sel.is_some_and(|t| t.index == s.index) {
                continue;
            }
            let pts: Vec<Pos2> = corners(s, s.xf).iter().map(|&(x, y)| to_screen(x, y)).collect();
            for i in 0..4 {
                painter.add(Shape::dashed_line(&[pts[i], pts[(i + 1) % 4]], Stroke::new(1.0, Color32::from_gray(150)), 4.0, 3.0));
            }
            let c = to_screen(s.xf.rect_pt()[0] + s.xf.rect_pt()[2] / 2.0, s.xf.rect_pt()[1] + s.xf.rect_pt()[3] / 2.0);
            let size = (s.xf.rect_pt()[3] * k * 0.3).clamp(9.0, 22.0);
            painter.text(c, Align2::CENTER_CENTER, prompt(s.ph.as_deref().unwrap_or("")), FontId::new(size, FontFamily::Proportional), Color32::from_gray(120));
        }
        // Formen under pekaren.
        if let Some(h) = pointer.filter(|_| self.edit.drag.is_none()).and_then(hit_shape) {
            if sel_info.as_ref().is_none_or(|s| s.index != h.index) {
                outline(&painter, &h, h.xf, Stroke::new(1.0, accent.gamma_multiply(0.6)));
            }
        }
        // De andra markerade formerna (flervalsmarkering): bara kontur, och de flyttas med den som dras.
        let moved = self.edit.drag.as_ref().filter(|d| d.grip == Grip::Move).map(|d| (d.cur.x - d.shape.xf.x, d.cur.y - d.shape.xf.y));
        for m in self.edit.more.iter().filter(|m| m.slide == slide) {
            if let Some(i) = shapes.iter().find(|i| i.id == m.id && i.index == m.index) {
                let xf = moved.map_or(i.xf, |(dx, dy)| Xf { x: i.xf.x + dx, y: i.xf.y + dy, ..i.xf });
                outline(&painter, i, xf, Stroke::new(1.5, accent));
            }
        }
        // Markeringen med handtag (eller konturen som dras).
        let show = self.edit.drag.as_ref().map(|d| (d.shape.clone(), d.cur)).or(sel_info.clone().map(|s| (s.clone(), s.xf)));
        if let Some((info, xf)) = show {
            let in_text = text_sel.is_some_and(|t| t.index == info.index);
            let shown = ShapeInfo { xf, ..info.clone() };
            outline(&painter, &shown, xf, Stroke::new(if in_text { 1.0 } else { 1.5 }, accent));
            if !in_text && self.edit.more.is_empty() {
                for (i, (x, y)) in handle_points(&shown).into_iter().enumerate() {
                    if info.kind == Kind::Connector && i % 2 == 1 {
                        continue;
                    }
                    let r = Rect::from_center_size(to_screen(x, y), vec2(HANDLE, HANDLE));
                    painter.rect_filled(r, 1.5, Color32::WHITE);
                    painter.rect_stroke(r, 1.5, Stroke::new(1.2, accent), egui::StrokeKind::Middle);
                }
                if info.kind != Kind::Frame {
                    let (rx, ry) = rot_point(&shown, ROT_GAP / k);
                    let top = handle_points(&shown)[1];
                    painter.line_segment([to_screen(top.0, top.1), to_screen(rx, ry)], Stroke::new(1.0, accent));
                    painter.circle(to_screen(rx, ry), HANDLE / 2.0 + 1.0, Color32::WHITE, Stroke::new(1.2, accent));
                }
            }
        }
        if let Some(((ax, ay), (bx, by))) = self.edit.band {
            let r = Rect::from_two_pos(to_screen(ax, ay), to_screen(bx, by));
            painter.rect_filled(r, 0.0, accent.gamma_multiply(0.12));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
        }
        // Texten: markering och markör.
        if let Some(ts) = self.edit.text.as_ref().filter(|t| t.sel.slide == slide) {
            if let Some(c) = &ts.carets {
                if ts.anchor != ts.caret {
                    for q in c.selection(ts.anchor, ts.caret) {
                        let pts: Vec<Pos2> = q.iter().map(|&(x, y)| to_screen(x, y)).collect();
                        painter.add(Shape::convex_polygon(pts, accent.gamma_multiply(0.3), Stroke::NONE));
                    }
                }
                if let Some((a, b)) = c.caret(ts.caret) {
                    let blink = ui.input(|i| i.time) % 1.0 < 0.6;
                    if blink {
                        painter.line_segment([to_screen(a.0, a.1), to_screen(b.0, b.1)], Stroke::new(1.5, p.text));
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(300));
                }
            }
        }
    }

    /// Håller tangentbordsfokus på redigeringslagret medan något är markerat (se `Editor::overlay`).
    fn keep_focus(&self, ui: &Ui, id: Id, slide: usize) {
        let active = self.edit.text.is_some() || self.edit.sel.is_some_and(|s| s.slide == slide);
        ui.memory_mut(|m| {
            if active {
                if m.focused().is_none() {
                    m.request_focus(id);
                }
                if m.has_focus(id) {
                    m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true });
                }
            } else if m.has_focus(id) {
                m.surrender_focus(id);
            }
        });
    }

    // ---------------------------------------------------------------- anteckningar

    /// Anteckningarna som ett textfält (redigeringsläget). Ändringar sparas direkt i presentationen.
    pub(super) fn notes_editor(&mut self, ui: &mut Ui, p: &Palette) {
        let i = self.current;
        let fresh = !matches!(&self.edit.notes, Some((s, _)) if *s == i);
        let id = Id::unique(("kladd-pptx-notes-edit", i));
        let focused = ui.ctx().memory(|m| m.has_focus(id));
        if fresh || !focused {
            let cur = super::notes::notes_text(&self.deck, i);
            if fresh || self.edit.notes.as_ref().is_some_and(|(_, t)| *t != cur) {
                self.edit.notes = Some((i, cur));
            }
        }
        let Some((_, text)) = &mut self.edit.notes else { return };
        let before = text.clone();
        let resp = ui.add(
            egui::TextEdit::multiline(text)
                .id(id)
                .hint_text(t("Klicka för att lägga till anteckningar"))
                .desired_width(f32::INFINITY)
                .desired_rows(4)
                .frame(egui::Frame::NONE)
                .font(FontId::new(14.0, FontFamily::Proportional))
                .text_color(p.text),
        );
        if resp.changed() && *text != before {
            let text = text.clone();
            self.apply(Some(format!("notes:{i}")), |txn| edit_ops::set_notes(txn, i, &text));
        }
    }

    // ---------------------------------------------------------------- sidopanelen

    /// Högerklicksmenyn för en miniatyr.
    pub(super) fn slide_menu(&mut self, ui: &mut Ui, p: &Palette, i: usize) {
        ui.set_min_width(240.0);
        ui.set_max_width(260.0);
        ui.menu_button(t("Ny bild"), |ui| {
            self.layout_items(ui, p, Some(i));
        });
        if crate::app::menu_item(ui, p, t("Duplicera bilden"), "", None, true) {
            self.duplicate_slide(i);
            ui.close();
        }
        if crate::app::menu_item(ui, p, t("Ta bort bilden"), "", None, self.count() > 1) {
            self.delete_slide(i);
            ui.close();
        }
        let hidden = self.deck.slides.get(i).is_some_and(|s| s.hidden);
        if crate::app::menu_item(ui, p, if hidden { t("Visa bilden") } else { t("Dölj bilden") }, "", None, true) {
            self.toggle_hidden(i);
            ui.close();
        }
        crate::app::menu_divider(ui, p);
        if crate::app::menu_item(ui, p, t("Flytta upp"), "", None, i > 0) {
            self.move_slide(i, i - 1);
            ui.close();
        }
        if crate::app::menu_item(ui, p, t("Flytta ned"), "", None, i + 1 < self.count()) {
            self.move_slide(i, i + 1);
            ui.close();
        }
    }

    /// Mallens layouter som menyval ("Ny bild").
    pub(super) fn layout_items(&mut self, ui: &mut Ui, p: &Palette, after: Option<usize>) {
        ui.set_max_width(260.0);
        let names: Vec<String> = self.deck.layouts.iter().enumerate().map(|(k, l)| l.root.child("p:cSld").and_then(|c| c.attr("name")).filter(|n| !n.is_empty()).map(str::to_owned).unwrap_or_else(|| crate::tf!("Layout {n}", n = k + 1))).collect();
        for (k, name) in names.iter().enumerate() {
            if crate::app::menu_item(ui, p, name, "", None, true) {
                if let Some(a) = after {
                    self.current = a;
                }
                self.new_slide(k);
                ui.close();
            }
        }
        if names.is_empty() {
            ui.label(t("Presentationen har inga layouter."));
        }
    }

    /// Dra miniatyrer för att ändra ordning: `resp` är radens svar, `row` dess rektangel.
    pub(super) fn thumb_drag(&mut self, ui: &Ui, i: usize, resp: &egui::Response, row: Rect, p: &Palette) {
        if !self.edit.on {
            return;
        }
        if resp.drag_started() {
            self.edit.thumb_drag = Some((i, row.center().y));
        }
        if let Some((from, _)) = self.edit.thumb_drag {
            if let Some(pt) = ui.input(|inp| inp.pointer.interact_pos()) {
                self.edit.thumb_drag = Some((from, pt.y));
                // Infogningslinjen över eller under raden pekaren står på.
                if row.y_range().contains(pt.y) && from != i {
                    let y = if pt.y < row.center().y { row.top() } else { row.bottom() };
                    ui.painter().hline(row.x_range(), y, Stroke::new(2.5, p.accent));
                }
            }
            if resp.drag_stopped() || (from == i && ui.input(|inp| inp.pointer.any_released())) {
                let y = self.edit.thumb_drag.map_or(0.0, |d| d.1);
                self.edit.thumb_drag = None;
                ui.ctx().data_mut(|d| d.insert_temp(Id::unique("kladd-pptx-thumb-drop"), (from, y)));
            }
        }
    }

    /// Avslutar en dragning av miniatyrer: `rows` är radernas rektanglar (bild, rektangel).
    pub(super) fn thumb_drop(&mut self, ctx: &egui::Context, rows: &[(usize, Rect)]) {
        let Some((from, y)) = ctx.data_mut(|d| d.remove_temp::<(usize, f32)>(Id::unique("kladd-pptx-thumb-drop"))) else { return };
        let Some(&(i, r)) = rows.iter().find(|(_, r)| r.y_range().contains(y)) else { return };
        let mut to = if y < r.center().y { i } else { i + 1 };
        if to > from {
            to -= 1;
        }
        if to != from {
            self.move_slide(from, to.min(self.count().saturating_sub(1)));
        }
    }
}

/// Formens hörn (bildens punkter) för placeringen `xf`.
fn corners(info: &ShapeInfo, xf: Xf) -> [(f32, f32); 4] {
    let s = ShapeInfo { xf, ..info.clone() };
    let t = s.affine();
    let [_, _, w, h] = xf.rect_pt();
    [t.apply(0.0, 0.0), t.apply(w, 0.0), t.apply(w, h), t.apply(0.0, h)]
}

/// Handtagen 0–7 (medurs från övre vänstra hörnet).
fn handle_points(info: &ShapeInfo) -> [(f32, f32); 8] {
    let t = info.affine();
    let [_, _, w, h] = info.xf.rect_pt();
    let l = [(0.0, 0.0), (w / 2.0, 0.0), (w, 0.0), (w, h / 2.0), (w, h), (w / 2.0, h), (0.0, h), (0.0, h / 2.0)];
    l.map(|(x, y)| t.apply(x, y))
}

fn rot_point(info: &ShapeInfo, gap: f32) -> (f32, f32) {
    let [_, _, w, _] = info.xf.rect_pt();
    info.affine().apply(w / 2.0, -gap)
}

fn handle_cursor(h: u8, rot: f32) -> CursorIcon {
    let step = ((rot / 45.0).round() as i32).rem_euclid(8) as u8;
    match (h + step) % 4 {
        0 => CursorIcon::ResizeNwSe,
        1 => CursorIcon::ResizeVertical,
        2 => CursorIcon::ResizeNeSw,
        _ => CursorIcon::ResizeHorizontal,
    }
}

/// Den nya placeringen när ett handtag dras till punkten `pt` (bildens punkter). Storleksändringen
/// görs i formens egna (vridna) koordinater med motsatt kant fast; Skift behåller proportionerna
/// (hörn) eller vrider i steg om 15°.
fn drag_xf(d: &Drag, pt: (f32, f32), shift: bool) -> Xf {
    let o = d.shape.xf;
    let emu = |v: f32| (v as f64 * super::model::EMU_PER_PT).round() as i64;
    let [x, y, w, h] = o.rect_pt();
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    match d.grip {
        Grip::Move => {
            let (mut dx, mut dy) = (pt.0 - d.start.0, pt.1 - d.start.1);
            if shift {
                // Skift: bara vågrätt eller lodrätt.
                if dx.abs() > dy.abs() { dy = 0.0 } else { dx = 0.0 }
            }
            Xf { x: o.x + emu(dx), y: o.y + emu(dy), ..o }
        }
        Grip::Rotate => {
            let mut a = (pt.1 - cy).atan2(pt.0 - cx).to_degrees() + 90.0;
            if shift {
                a = (a / 15.0).round() * 15.0;
            }
            let a = a.rem_euclid(360.0);
            Xf { rot: ((a * 60000.0).round() as i64) % 21_600_000, ..o }
        }
        Grip::Handle(hd) => {
            // Punkten i formens ursprungliga, ovridna koordinater.
            let th = -o.rot_deg().to_radians();
            let (px, py) = (pt.0 - cx, pt.1 - cy);
            let (u, v) = (px * th.cos() - py * th.sin() + w / 2.0, px * th.sin() + py * th.cos() + h / 2.0);
            let (mut u0, mut v0, mut u1, mut v1) = (0.0f32, 0.0f32, w, h);
            match hd {
                0 => (u0, v0) = (u, v),
                1 => v0 = v,
                2 => (u1, v0) = (u, v),
                3 => u1 = u,
                4 => (u1, v1) = (u, v),
                5 => v1 = v,
                6 => (u0, v1) = (u, v),
                _ => u0 = u,
            }
            const MIN: f32 = 2.0;
            if u1 - u0 < MIN {
                if matches!(hd, 0 | 6 | 7) { u0 = u1 - MIN } else { u1 = u0 + MIN }
            }
            if v1 - v0 < MIN {
                if matches!(hd, 0 | 1 | 2) { v0 = v1 - MIN } else { v1 = v0 + MIN }
            }
            if shift && hd % 2 == 0 && w > 0.0 && h > 0.0 {
                let s = ((u1 - u0) / w).max((v1 - v0) / h);
                let (nw, nh) = (w * s, h * s);
                if matches!(hd, 0 | 6) { u0 = u1 - nw } else { u1 = u0 + nw }
                if matches!(hd, 0 | 2) { v0 = v1 - nh } else { v1 = v0 + nh }
            }
            let (nw, nh) = (u1 - u0, v1 - v0);
            // Den nya mittpunkten tillbaka till bilden.
            let (lx, ly) = ((u0 + u1) / 2.0 - w / 2.0, (v0 + v1) / 2.0 - h / 2.0);
            let r = o.rot_deg().to_radians();
            let (ncx, ncy) = (cx + lx * r.cos() - ly * r.sin(), cy + lx * r.sin() + ly * r.cos());
            Xf { x: emu(ncx - nw / 2.0), y: emu(ncy - nh / 2.0), cx: emu(nw), cy: emu(nh), ..o }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(grip: Grip, xf: Xf) -> Drag {
        let shape = ShapeInfo { index: 0, id: 1, name: String::new(), kind: Kind::Shape, xf, ph: None, text: true, empty_ph: false, alternate: false };
        Drag { grip, shape, others: Vec::new(), start: (0.0, 0.0), cur: xf }
    }

    const PT: i64 = 12_700;

    #[test]
    fn storleksandring_med_fast_motsatt_horn() {
        let xf = Xf { x: 100 * PT, y: 100 * PT, cx: 200 * PT, cy: 100 * PT, ..Default::default() };
        // Nedre högra hörnet till (400, 300): övre vänstra står kvar.
        let n = drag_xf(&drag(Grip::Handle(4), xf), (400.0, 300.0), false);
        assert_eq!((n.x, n.y, n.cx, n.cy), (100 * PT, 100 * PT, 300 * PT, 200 * PT));
        // Med Skift: proportionerna 2:1 behålls.
        let n = drag_xf(&drag(Grip::Handle(4), xf), (400.0, 300.0), true);
        assert_eq!((n.cx, n.cy), (400 * PT, 200 * PT));
        // Vänsterkanten förbi högerkanten: minsta bredd, högerkanten står kvar.
        let n = drag_xf(&drag(Grip::Handle(7), xf), (500.0, 150.0), false);
        assert_eq!(n.x + n.cx, 300 * PT);
    }

    #[test]
    fn storleksandring_av_vriden_form_och_vridning() {
        // 90° vriden: formens högerkant pekar nedåt på bilden.
        let xf = Xf { x: 0, y: 0, cx: 200 * PT, cy: 100 * PT, rot: 90 * 60000, ..Default::default() };
        // Mitt (100, 50); högerkantens handtag ligger vid (100, 150). Dra det 50 pt nedåt.
        let n = drag_xf(&drag(Grip::Handle(3), xf), (100.0, 200.0), false);
        assert_eq!(n.cx, 250 * PT);
        assert_eq!(n.cy, 100 * PT);
        // Mitten flyttas 25 pt nedåt: (100, 75).
        assert_eq!((n.x + n.cx / 2, n.y + n.cy / 2), (100 * PT, 75 * PT));
        let r = drag_xf(&drag(Grip::Rotate, Xf { rot: 0, ..xf }), (200.0, 50.0), false);
        assert_eq!(r.rot, 90 * 60000);
        let r = drag_xf(&drag(Grip::Rotate, Xf { rot: 0, ..xf }), (190.0, 20.0), true);
        assert_eq!(r.rot % (15 * 60000), 0, "Skift: steg om 15°");
    }
}
