//! Presentationsvyn: miniatyrlist i sidopanelen, en bild i taget eller alla bilder i en rullande
//! lista, zoom (hel bild, bildbredd, procent), anteckningspanel under bilden, sök i bildtext och
//! anteckningar med träffarna markerade på bilden, bildspel i helskärm och PDF-export. Bilderna
//! renderas med `raster.rs` i bakgrundstrådar; bara det som syns (och nästa bild i bildspelet)
//! renderas, och texturer långt från det synliga släpps. Förebilder: PDF-visaren och EPUB-vyn.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::Arc;
use std::time::{Duration};

use egui::emath::GuiRounding;
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Order, Panel,
    Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui, pos2, vec2,
};
use serde::{Deserialize, Serialize};

use super::find::{self, Hit, Place, SlideText};
use super::slideshow::Show;
use super::Deck;
use crate::i18n::t;
use crate::icons::{self, Icon};
use crate::pdfview::chrome::{BAR_H, G, bar_divider, menu_heading, menu_trigger, tool_btn};
use crate::tf;
use crate::theme::{self, Palette};

/// Högst så många bilder renderas samtidigt.
const MAX_JOBS: usize = 2;
/// Mellanrum mellan bilderna (punkter på skärmen).
const GAP: f32 = 18.0;
/// Läget sparas för så många presentationer.
pub const MAX_MEMORY: usize = 100;
/// 100 % = verklig storlek på en skärm med 96 dpi (en punkt är 4/3 bildpunkt).
const PT_TO_PX: f32 = 96.0 / 72.0;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 4.0;

/// Zoomläge.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum Zoom {
    /// Hela bilden syns i fönstret.
    Window,
    /// Bilden fyller fönstrets bredd.
    Width,
    /// Fast skala (1.0 = verklig storlek).
    Pct(f32),
}

/// Visningsval som gäller alla presentationer (sparas i inställningarna).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Prefs {
    /// Miniatyrlisten i sidopanelen.
    pub thumbs: bool,
    /// Anteckningspanelen under bilden (när presentationen har anteckningar).
    pub notes: bool,
    /// Alla bilder i en rullande lista i stället för en bild i taget.
    pub scroll: bool,
    pub zoom: Zoom,
    /// Bildspelet spelar upp övergångar och animeringar.
    pub animate: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { thumbs: true, notes: true, scroll: false, zoom: Zoom::Window, animate: true }
    }
}

/// Läget i en presentation (sparas per fil).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Memory {
    pub slide: usize,
}

/// Lägger läget först i listan (en post per fil, högst `cap`).
pub fn remember(list: &mut Vec<(String, Memory)>, key: String, mem: Memory, cap: usize) {
    crate::pdfview::chrome::remember_memory(list, key, mem, cap);
}

/// Skalan (egui-punkter per bildpunkt i `pt`) för en bild `sw`×`sh` pt i ytan `aw`×`ah`.
pub fn scale_for(zoom: Zoom, aw: f32, ah: f32, sw: f32, sh: f32) -> f32 {
    let (sw, sh) = (sw.max(1.0), sh.max(1.0));
    let k = match zoom {
        Zoom::Window => (aw / sw).min(ah / sh),
        Zoom::Width => aw / sw,
        Zoom::Pct(z) => z.clamp(ZOOM_MIN, ZOOM_MAX) * PT_TO_PX,
    };
    // Minst 40 punkter bred, så att en bild aldrig försvinner i ett litet fönster.
    k.max(40.0 / sw)
}

/// Nästa zoomsteg (×1,25 eller ÷1,25) från skalan `k`, avrundat till hela procent.
pub fn zoom_step(k: f32, dir: f32) -> Zoom {
    let pct = k / PT_TO_PX;
    let next = if dir > 0.0 { pct * 1.25 } else { pct / 1.25 };
    Zoom::Pct(((next * 100.0).round() / 100.0).clamp(ZOOM_MIN, ZOOM_MAX))
}

/// Texten i zoomväljaren.
pub fn zoom_label(z: Zoom) -> String {
    match z {
        Zoom::Window => t("Hel bild").to_owned(),
        Zoom::Width => t("Bildbredd").to_owned(),
        Zoom::Pct(p) => format!("{} %", (p * 100.0).round() as i32),
    }
}

/// Bildnummer ur fältet i verktygsraden (1-baserat) till index.
pub fn parse_slide_input(s: &str, count: usize) -> Option<usize> {
    let n: usize = s.trim().parse().ok()?;
    (1..=count).contains(&n).then(|| n - 1)
}

/// Vad appen ska göra efter en knapp eller ett kortkommando i vyn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    EditCopy,
    ExportPdf,
    Print,
    /// Bildspel från början (`false`) eller från aktuell bild (`true`).
    Show(bool),
    /// Infoga en bild ur en fil (redigeringsläget).
    InsertPicture,
}

// ------------------------------------------------------------ rendering

/// Vilken sorts textur.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
enum Kind {
    /// Bilden i vyn.
    Page,
    /// Miniatyr i sidopanelen.
    Thumb,
}

struct Done {
    kind: Kind,
    slide: usize,
    width: u32,
    /// Bildens version när renderingen startades (se `Pool::gens`).
    ver: u64,
    image: egui::ColorImage,
}

/// Texturer och renderingar i bakgrunden.
struct Pool {
    tex: HashMap<(Kind, usize), (egui::TextureHandle, u32, u64)>,
    /// Version per bild: räknas upp när bilden ändras (redigering), så att texturen renderas om.
    gens: HashMap<usize, u64>,
    next_gen: u64,
    pending: HashSet<(Kind, usize)>,
    wants: Vec<(Kind, usize, u32)>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
}

impl Pool {
    fn new() -> Pool {
        let (tx, rx) = channel();
        Pool { tex: HashMap::new(), gens: HashMap::new(), next_gen: 1, pending: HashSet::new(), wants: Vec::new(), tx, rx }
    }

    fn get(&self, kind: Kind, slide: usize) -> Option<&egui::TextureHandle> {
        self.tex.get(&(kind, slide)).map(|(t, _, _)| t)
    }

    fn ver(&self, slide: usize) -> u64 {
        self.gens.get(&slide).copied().unwrap_or(0)
    }

    /// Bilderna har ändrats: texturerna behålls tills nya renderingar är klara. `all` = bildernas
    /// ordning eller antal ändrades (texturerna gäller inte längre sina index).
    fn bump(&mut self, slides: &[usize], all: bool) {
        if all {
            self.tex.clear();
            self.gens.clear();
        }
        for &s in slides {
            self.gens.insert(s, self.next_gen);
        }
        self.next_gen += 1;
    }

    /// Önskar en rendering om texturen saknas eller har fel bredd.
    fn want(&mut self, kind: Kind, slide: usize, width: u32) {
        let ver = self.ver(slide);
        let stale = self.tex.get(&(kind, slide)).is_none_or(|(_, w, g)| (*w as i64 - width as i64).abs() > 2 || *g != ver);
        if stale {
            self.wants.push((kind, slide, width));
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.rx.try_recv() {
            self.pending.remove(&(d.kind, d.slide));
            let tex = ctx.load_texture(format!("pptx-{:?}-{}", d.kind, d.slide), d.image, egui::TextureOptions::LINEAR);
            // En rendering av en äldre version ersätter inte en nyare textur.
            if self.tex.get(&(d.kind, d.slide)).is_some_and(|(_, _, g)| *g > d.ver) {
                continue;
            }
            self.tex.insert((d.kind, d.slide), (tex, d.width, d.ver));
        }
    }

    /// Startar de viktigaste önskningarna (vyn före miniatyrer).
    fn dispatch(&mut self, ctx: &egui::Context, deck: &Arc<Deck>) {
        let mut wants = std::mem::take(&mut self.wants);
        wants.sort_by_key(|w| w.0);
        for (kind, slide, width) in wants {
            if self.pending.len() >= MAX_JOBS {
                break;
            }
            if self.pending.contains(&(kind, slide)) {
                continue;
            }
            self.pending.insert((kind, slide));
            let ver = self.ver(slide);
            let deck = deck.clone();
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            let (w, h) = deck.size_pt();
            let width = width.clamp(16, 8192);
            let height = ((width as f32) * h / w.max(1.0)).round().clamp(1.0, 8192.0) as u32;
            let _ = std::thread::Builder::new().name("pptx-bild".into()).stack_size(8 << 20).spawn(move || {
                // En trasig bild får inte fälla appen: fel ger en tom bild.
                let rgba = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::render_slide(&deck, slide, width, height)))
                    .ok()
                    .flatten()
                    .map(|(_, _, px)| px)
                    .unwrap_or_else(|| vec![255; (width * height * 4) as usize]);
                let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
                let _ = tx.send(Done { kind, slide, width, ver, image });
                ctx.request_repaint();
            });
        }
    }

    /// Släpper texturer av ett slag utanför `keep`.
    fn retain(&mut self, kind: Kind, keep: impl Fn(usize) -> bool) {
        self.tex.retain(|&(k, s), _| k != kind || keep(s));
    }
}

// ------------------------------------------------------------ sök

type IndexMsg = (usize, SlideText, String);

/// Sökfältet och registret över bildernas text.
#[derive(Default)]
pub struct FindState {
    pub open: bool,
    focus: bool,
    pub query: String,
    pub case: bool,
    index: Vec<Option<(SlideText, String)>>,
    index_rx: Option<Receiver<IndexMsg>>,
    hits: Vec<Hit>,
    cur: Option<usize>,
    /// Sökningen som `hits` gäller (fråga, skiftläge, antal indexerade bilder).
    done_for: Option<(String, bool, usize)>,
    /// Aktuell träff står i anteckningarna: panelen ska öppnas (en gång per hopp).
    reveal_notes: bool,
    /// Träffen som anteckningspanelen senast rullade fram till.
    notes_scrolled: Option<Hit>,
}

impl FindState {
    fn indexed(&self) -> usize {
        self.index.iter().filter(|e| e.is_some()).count()
    }

    fn searching(&self) -> bool {
        self.index_rx.is_some()
    }

    /// Texten bredvid fältet: "3 av 12", "Inga träffar" eller "Söker…".
    fn status(&self) -> String {
        if self.query.is_empty() {
            return String::new();
        }
        match (self.hits.len(), self.cur) {
            (0, _) if self.searching() => t("Söker…").to_owned(),
            (0, _) => t("Inga träffar").to_owned(),
            (n, Some(c)) => tf!("{n} av {total}", n = c + 1, total = n),
            (n, None) => tf!("{total} träffar", total = n),
        }
    }
}

// ------------------------------------------------------------ export

#[derive(Default)]
struct Export {
    dialog: Option<Receiver<Option<PathBuf>>>,
    writing: Option<Receiver<Result<PathBuf, String>>>,
}

// ------------------------------------------------------------ fliken

/// En öppen presentation i en flik.
pub struct PptxTab {
    pub deck: Arc<Deck>,
    /// Aktuell bild (0-baserad).
    pub current: usize,
    pool: Pool,
    /// Rullande lista: rulla fram till bilden.
    scroll_to: Option<usize>,
    /// Miniatyrlisten ska rulla fram till aktuell bild.
    thumb_follow: Option<usize>,
    /// Anteckningarnas text per bild (läses när den behövs).
    notes: HashMap<usize, String>,
    has_notes: bool,
    pub find: FindState,
    show: Option<super::presenter::ShowView>,
    /// Helskärm har begärts för bildspelet (appen återställer när det slutar).
    pub fullscreen_sent: bool,
    /// Nästa bildspel startar med föredragsvyn (Alt+F5).
    presenter_on_start: bool,
    wheel: f32,
    /// Skalan i senaste bilden (för zoomstegen).
    last_k: f32,
    page_text: String,
    export: Export,
    /// Redigeringen (`edit_view.rs`).
    pub(super) edit: super::edit_view::Editor,
}

impl PptxTab {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(deck: Deck) -> PptxTab {
        Self::with_source(deck, Vec::new())
    }

    /// En flik för en presentation som öppnats ur `source` (filens byte; orörda delar sparas
    /// därifrån oförändrade).
    pub fn with_source(deck: Deck, source: Vec<u8>) -> PptxTab {
        let edit = super::edit_view::Editor::new(source, &deck);
        let has_notes = (0..deck.slides.len()).any(|i| deck.slides[i].notes.is_some() && !super::notes::notes(&deck, i).is_empty());
        PptxTab {
            deck: Arc::new(deck),
            current: 0,
            pool: Pool::new(),
            scroll_to: None,
            thumb_follow: Some(0),
            notes: HashMap::new(),
            has_notes,
            find: FindState::default(),
            show: None,
            fullscreen_sent: false,
            presenter_on_start: false,
            wheel: 0.0,
            last_k: 1.0,
            page_text: String::new(),
            export: Export::default(),
            edit,
        }
    }

    /// Bilder har ändrats (redigering): de renderas om, och sökregistret byggs om.
    pub(super) fn invalidate(&mut self, slides: &[usize], all: bool) {
        self.pool.bump(slides, all);
        if !self.find.index.is_empty() || self.find.index_rx.is_some() {
            self.find.index.clear();
            self.find.index_rx = None;
            self.find.done_for = None;
            if self.find.open {
                self.start_index();
            }
        }
    }

    /// Anteckningarna kan ha ändrats.
    pub(super) fn notes_changed(&mut self) {
        self.notes.clear();
        let deck = self.deck.clone();
        self.has_notes = (0..deck.slides.len()).any(|i| deck.slides[i].notes.is_some() && !super::notes::notes(&deck, i).is_empty());
    }

    /// Meddelande om sådant som inte visas än (diagram, EMF, inbäddade typsnitt som inte kan läsas).
    pub fn open_note(&self, name: &str) -> Option<String> {
        let mut lost: Vec<&str> = Vec::new();
        if !self.deck.fonts.unreadable_embedded.is_empty() {
            lost.push(t("inbäddade typsnitt som inte kan läsas"));
        }
        (!lost.is_empty()).then(|| tf!("{name} innehåller {lost} som ännu inte visas som i PowerPoint", name = name, lost = crate::doc::join_sv(&lost)))
    }

    pub fn count(&self) -> usize {
        self.deck.slides.len()
    }

    /// Bilderna som syns i bildspel och PDF (dolda bilder hoppas över).
    pub fn visible_slides(&self) -> Vec<usize> {
        (0..self.count()).filter(|&i| !self.deck.slides[i].hidden).collect()
    }

    /// Presentationen som PDF (dolda bilder utelämnas, som i PowerPoints export).
    pub fn pdf_bytes(&self) -> Result<Vec<u8>, String> {
        pdf_of(&self.deck)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_notes(&self) -> bool {
        self.has_notes
    }

    pub fn go_to(&mut self, i: usize) {
        let n = self.count();
        if n == 0 {
            return;
        }
        self.current = i.min(n - 1);
        self.scroll_to = Some(self.current);
        self.thumb_follow = Some(self.current);
    }

    pub fn memory(&self) -> Memory {
        Memory { slide: self.current }
    }

    pub fn restore(&mut self, m: &Memory) {
        self.go_to(m.slide);
    }

    fn notes_of(&mut self, i: usize) -> &str {
        let deck = self.deck.clone();
        self.notes.entry(i).or_insert_with(|| super::notes::notes_text(&deck, i))
    }

    // -------------------------------------------------------- bildspel

    pub fn presenting(&self) -> bool {
        self.show.is_some()
    }

    /// Startar bildspelet (`animate`: övergångar och animeringar). Falskt om ingen bild kan visas.
    pub fn start_show(&mut self, from_current: bool, animate: bool) -> bool {
        let hidden: Vec<bool> = self.deck.slides.iter().map(|s| s.hidden).collect();
        let show = if from_current { Show::new(&hidden, self.current) } else { Show::from_beginning(&hidden) };
        let presenter = std::mem::take(&mut self.presenter_on_start);
        self.show = show.map(|s| super::presenter::ShowView::new(self.deck.clone(), s, animate).with_presenter(presenter));
        self.show.is_some()
    }

    /// Avslutar bildspelet och står kvar på bilden som visades.
    pub fn stop_show(&mut self) {
        if let Some(s) = self.show.take() {
            self.go_to(s.slide());
        }
    }

    /// Tangenterna i bildspelet (appen skickar alla tangenter hit medan det pågår).
    pub fn show_keys(&mut self, ctx: &egui::Context) {
        if self.show.as_mut().is_some_and(|s| !s.keys(ctx)) {
            self.stop_show();
        }
    }

    /// Bildspelet (och föredragsvyn), se `presenter.rs`.
    pub fn show_page(&mut self, ui: &mut Ui) {
        if self.show.as_mut().is_some_and(|s| !s.ui(ui)) {
            self.stop_show();
        }
    }

    // -------------------------------------------------------- sök

    pub fn open_find(&mut self) {
        self.find.open = true;
        self.find.focus = true;
        if self.find.index.is_empty() && self.find.index_rx.is_none() && self.count() > 0 {
            self.start_index();
        }
    }

    pub fn close_find(&mut self) {
        self.find.open = false;
        self.find.cur = None;
    }

    /// Bygger registret i en bakgrundstråd: varje bilds visningslista (texten där den ritas) och
    /// anteckningar. Träffarna visas efter hand.
    fn start_index(&mut self) {
        let n = self.count();
        self.find.index = (0..n).map(|_| None).collect();
        let (tx, rx) = channel::<IndexMsg>();
        self.find.index_rx = Some(rx);
        let deck = self.deck.clone();
        let _ = std::thread::Builder::new().name("pptx-sok".into()).stack_size(8 << 20).spawn(move || {
            for i in 0..n {
                let st = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| find::slide_text(&super::render::slide_scene(&deck, i)))).unwrap_or_default();
                let notes = super::notes::notes_text(&deck, i);
                if tx.send((i, st, notes)).is_err() {
                    return;
                }
            }
        });
    }

    /// Tar emot registret och räknar om träffarna när frågan eller registret ändrats.
    fn update_find(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.find.index_rx {
            loop {
                match rx.try_recv() {
                    Ok((i, st, notes)) => {
                        if let Some(e) = self.find.index.get_mut(i) {
                            *e = Some((st, notes));
                        }
                    }
                    Err(TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(80));
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        self.find.index_rx = None;
                        break;
                    }
                }
            }
        }
        let key = (self.find.query.clone(), self.find.case, self.find.indexed());
        if self.find.done_for.as_ref() == Some(&key) {
            return;
        }
        let query_changed = self.find.done_for.as_ref().is_none_or(|(q, c, _)| *q != key.0 || *c != key.1);
        let old = self.find.cur.and_then(|c| self.find.hits.get(c).copied());
        let mut hits = Vec::new();
        if !key.0.is_empty() {
            for (i, e) in self.find.index.iter().enumerate() {
                if let Some((st, notes)) = e {
                    hits.extend(find::hits_for(i, st, notes, &key.0, key.1));
                }
            }
        }
        self.find.hits = hits;
        self.find.done_for = Some(key);
        if query_changed {
            // Ny fråga: första träffen från aktuell bild.
            self.find.cur = self.find.hits.iter().position(|h| h.slide >= self.current).or((!self.find.hits.is_empty()).then_some(0));
            if let Some(h) = self.find.cur.map(|c| self.find.hits[c]) {
                self.find.reveal_notes = h.place == Place::Notes;
                self.go_to(h.slide);
            }
        } else {
            // Registret växte: behåll samma träff.
            self.find.cur = old.and_then(|o| self.find.hits.iter().position(|h| *h == o)).or(self.find.cur.filter(|&c| c < self.find.hits.len()));
        }
    }

    /// Nästa (eller föregående) träff; öppnar anteckningspanelen om träffen står där.
    pub fn find_step(&mut self, back: bool, prefs: &mut Prefs) {
        let n = self.find.hits.len();
        if n == 0 {
            return;
        }
        let c = match self.find.cur {
            Some(c) if back => (c + n - 1) % n,
            Some(c) => (c + 1) % n,
            None => 0,
        };
        self.find.cur = Some(c);
        let h = self.find.hits[c];
        if h.place == Place::Notes {
            prefs.notes = true;
        }
        self.find.notes_scrolled = None;
        self.go_to(h.slide);
    }

    /// Sökfältet uppe till höger i vyn.
    pub fn find_bar(&mut self, ctx: &egui::Context, p: &Palette, prefs: &mut Prefs, anchor: Rect) {
        if !self.find.open {
            return;
        }
        self.update_find(ctx);
        let width = 420.0f32.min(anchor.width() - 24.0).max(200.0);
        let pos = pos2(anchor.right() - width - 22.0, anchor.top() + 12.0);
        let mut step: Option<bool> = None;
        let mut close = false;
        egui::Area::new(Id::unique("kladd-pptx-find")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            Frame::NONE
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(11)
                .inner_margin(Margin::same(8))
                .shadow(ui.visuals().popup_shadow)
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                    ui.horizontal(|ui| {
                        let qid = Id::unique("kladd-pptx-find-query");
                        let field_w = (ui.available_width() - 26.0 * 4.0 - 96.0 - 4.0 * 6.0).max(60.0);
                        let q = ui.add(
                            TextEdit::singleline(&mut self.find.query)
                                .id(qid)
                                .hint_text(t("Sök i bilder och anteckningar"))
                                .desired_width(field_w)
                                .margin(Margin::symmetric(8, 5)),
                        );
                        if self.find.focus {
                            q.request_focus();
                            self.find.focus = false;
                        }
                        if q.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            step = Some(ui.input(|i| i.modifiers.shift));
                            q.request_focus();
                        }
                        let (cr, _) = ui.allocate_exact_size(vec2(96.0, 26.0), Sense::hover());
                        let bad = !self.find.query.is_empty() && !self.find.searching() && self.find.hits.is_empty();
                        ui.painter().text(cr.center(), Align2::CENTER_CENTER, self.find.status(), FontId::new(12.0, FontFamily::Proportional), if bad { p.error } else { p.muted });
                        if icons::button(ui, p, Icon::CaseSensitive, 26.0, self.find.case, t("Skilj på stora och små bokstäver"), p.surface).clicked() {
                            self.find.case = !self.find.case;
                        }
                        if icons::button(ui, p, Icon::ChevronUp, 26.0, false, t("Föregående (Shift+Enter)"), p.surface).clicked() {
                            step = Some(true);
                        }
                        if icons::button(ui, p, Icon::ChevronDown, 26.0, false, t("Nästa (Enter)"), p.surface).clicked() {
                            step = Some(false);
                        }
                        if icons::button(ui, p, Icon::Close, 26.0, false, t("Stäng (Esc)"), p.surface).clicked() {
                            close = true;
                        }
                    });
                });
        });
        self.update_find(ctx);
        if let Some(back) = step {
            self.find_step(back, prefs);
        }
        if std::mem::take(&mut self.find.reveal_notes) {
            prefs.notes = true;
        }
        if close {
            self.close_find();
        }
    }

    /// Träffrektanglarna på bild `i` i bildens punkter; den aktuella träffen markeras starkare.
    fn slide_hits(&self, i: usize) -> Vec<([f32; 4], bool)> {
        if !self.find.open || self.find.query.is_empty() {
            return Vec::new();
        }
        let Some(Some((st, _))) = self.find.index.get(i) else { return Vec::new() };
        let cur = self.find.cur.and_then(|c| self.find.hits.get(c)).copied();
        let mut out = Vec::new();
        for h in self.find.hits.iter().filter(|h| h.slide == i && h.place == Place::Slide) {
            for r in find::hit_rects(st, h.start, h.end) {
                out.push((r, Some(*h) == cur));
            }
        }
        out
    }

    // -------------------------------------------------------- vyn

    /// Ritar en bild i `rect` (skugga, textur eller vit yta, sökträffar) och önskar rendering.
    fn paint_slide(&mut self, ui: &Ui, rect: Rect, i: usize, p: &Palette) {
        let ppp = ui.ctx().pixels_per_point();
        let painter = ui.painter_at(rect.expand(4.0));
        painter.rect_filled(rect.translate(vec2(0.0, 2.0)).expand(1.0), 2.0, Color32::from_black_alpha(40));
        match self.pool.get(Kind::Page, i) {
            Some(tex) => {
                painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                painter.rect_filled(rect, 0.0, Color32::WHITE);
            }
        }
        let (sw, _) = self.deck.size_pt();
        let k = rect.width() / sw.max(1.0);
        for (r, cur) in self.slide_hits(i) {
            let hr = Rect::from_min_max(rect.min + vec2(r[0], r[1]) * k, rect.min + vec2(r[2], r[3]) * k).expand(1.0);
            let fill = if cur { Color32::from_rgba_unmultiplied(255, 150, 0, 110) } else { Color32::from_rgba_unmultiplied(255, 220, 0, 90) };
            painter.rect_filled(hr, 2.0, fill);
            if cur {
                painter.rect_stroke(hr, 2.0, Stroke::new(1.5, Color32::from_rgb(230, 120, 0)), StrokeKind::Outside);
            }
        }
        if self.deck.slides[i].hidden {
            // Dolda bilder: tonade och märkta (de visas inte i bildspelet).
            painter.rect_filled(rect, 0.0, Color32::from_white_alpha(70));
        }
        let _ = p;
        self.pool.want(Kind::Page, i, (rect.width() * ppp).round().max(1.0) as u32);
    }

    /// Bilderna i mitten (och anteckningspanelen under dem).
    pub fn show(&mut self, ui: &mut Ui, p: &Palette, prefs: &mut Prefs) {
        let ctx = ui.ctx().clone();
        self.pool.receive(&ctx);
        let n = self.count();
        if n == 0 {
            ui.centered_and_justified(|ui| ui.label(RichText::new(t("Presentationen har inga bilder.")).color(p.muted)));
            return;
        }
        self.current = self.current.min(n - 1);
        if prefs.notes && (self.has_notes || self.editing()) {
            self.notes_panel(ui, p);
        }
        // Redigeringen sker bild för bild.
        if prefs.scroll && !self.editing() {
            self.show_scroll(ui, p, prefs);
        } else {
            self.show_single(ui, p, prefs);
        }
        self.pool.dispatch(&ctx, &self.deck);
    }

    fn show_single(&mut self, ui: &mut Ui, p: &Palette, prefs: &Prefs) {
        let (sw, sh) = self.deck.size_pt();
        let avail = ui.available_rect_before_wrap();
        let k = scale_for(prefs.zoom, avail.width() - 2.0 * GAP, avail.height() - 2.0 * GAP, sw, sh);
        self.last_k = k;
        let size = vec2(sw * k, sh * k);
        let i = self.current;
        let fits = size.x <= avail.width() - 2.0 * GAP + 0.5 && size.y <= avail.height() - 2.0 * GAP + 0.5;
        if fits {
            let resp = ui.allocate_rect(avail, Sense::hover());
            let rect = Rect::from_center_size(avail.center(), size).round_to_pixels(ui.ctx().pixels_per_point());
            self.paint_slide(ui, rect, i, p);
            self.edit_overlay(ui, rect, i, p);
            // Hjulet bläddrar mellan bilderna när hela bilden syns.
            if resp.hovered() && !self.editing() {
                let dy = ui.input(|inp| if inp.modifiers.command { 0.0 } else { inp.smooth_scroll_delta.y });
                self.wheel += dy;
                if self.wheel <= -60.0 {
                    self.wheel = 0.0;
                    self.go_to(i + 1);
                } else if self.wheel >= 60.0 {
                    self.wheel = 0.0;
                    self.go_to(i.saturating_sub(1));
                }
            }
        } else {
            ScrollArea::both().id_salt(("kladd-pptx-single", i)).auto_shrink([false, false]).show(ui, |ui| {
                let w = ui.available_width().max(size.x + 2.0 * GAP);
                let (area, _) = ui.allocate_exact_size(vec2(w, size.y + 2.0 * GAP), Sense::hover());
                let rect = Rect::from_min_size(pos2(area.center().x - size.x / 2.0, area.top() + GAP), size);
                self.paint_slide(ui, rect, i, p);
                self.edit_overlay(ui, rect, i, p);
            });
        }
        self.pool.retain(Kind::Page, |s| s + 1 >= i && s <= i + 1);
    }

    fn show_scroll(&mut self, ui: &mut Ui, p: &Palette, prefs: &Prefs) {
        let (sw, sh) = self.deck.size_pt();
        let n = self.count();
        let mut visible: Vec<usize> = Vec::new();
        let mut center: Option<usize> = None;
        let jump = self.scroll_to.take();
        let avail = ui.available_rect_before_wrap();
        let k = scale_for(prefs.zoom, avail.width() - 2.0 * GAP - 12.0, avail.height() - 2.0 * GAP - 22.0, sw, sh);
        self.last_k = k;
        let size = vec2(sw * k, sh * k);
        ScrollArea::both().id_salt("kladd-pptx-scroll").auto_shrink([false, false]).show(ui, |ui| {
            let clip = ui.clip_rect();
            let w = ui.available_width().max(size.x + 2.0 * GAP);
            ui.add_space(GAP);
            for i in 0..n {
                let (area, _) = ui.allocate_exact_size(vec2(w, size.y), Sense::hover());
                let rect = Rect::from_min_size(pos2(area.center().x - size.x / 2.0, area.top()), size);
                if jump == Some(i) {
                    ui.scroll_to_rect(rect.expand(GAP / 2.0), Some(Align::TOP));
                }
                if ui.is_rect_visible(rect) {
                    visible.push(i);
                    if center.is_none() && rect.bottom() > clip.center().y.min(clip.top() + size.y / 2.0) {
                        center = Some(i);
                    }
                    self.paint_slide(ui, rect, i, p);
                }
                let label = if self.deck.slides[i].hidden { tf!("Bild {n} (dold)", n = i + 1) } else { tf!("Bild {n}", n = i + 1) };
                ui.add_space(4.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(label).font(FontId::proportional(12.0)).color(p.muted)));
                ui.add_space(GAP);
            }
        });
        // Aktuell bild följer rullningen (utom bildrutan där ett hopp just gjordes).
        if jump.is_none() {
            if let Some(c) = center.filter(|&c| c != self.current) {
                self.current = c;
                self.thumb_follow = Some(c);
            }
        }
        if let (Some(&lo), Some(&hi)) = (visible.first(), visible.last()) {
            self.pool.retain(Kind::Page, |s| s + 6 >= lo && s <= hi + 6);
        }
    }

    /// Anteckningarna till aktuell bild i en panel under bilden, med sökträffar markerade.
    fn notes_panel(&mut self, ui: &mut Ui, p: &Palette) {
        let i = self.current;
        let text = self.notes_of(i).to_owned();
        let cur = self.find.cur.and_then(|c| self.find.hits.get(c)).copied();
        let hits: Vec<Hit> = if self.find.open { self.find.hits.iter().filter(|h| h.slide == i && h.place == Place::Notes).copied().collect() } else { Vec::new() };
        let max = (ui.available_height() * 0.6).max(80.0);
        Panel::bottom("kladd-pptx-notes")
            .resizable(true)
            .default_size(120.0)
            .size_range(60.0..=max)
            .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin { left: 16, right: 12, top: 8, bottom: 6 }))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().hline((r.left() - 16.0)..=(r.right() + 12.0), r.top() - 8.0, Stroke::new(1.0, p.line));
                ui.label(RichText::new(t("Anteckningar")).font(FontId::new(11.5, theme::medium())).color(p.muted));
                ui.add_space(4.0);
                if self.editing() {
                    ScrollArea::vertical().id_salt(("kladd-pptx-notes-edit", i)).auto_shrink([false, false]).show(ui, |ui| self.notes_editor(ui, p));
                    return;
                }
                ScrollArea::vertical().id_salt(("kladd-pptx-notes-text", i)).auto_shrink([false, false]).show(ui, |ui| {
                    if text.trim().is_empty() {
                        ui.label(RichText::new(t("Inga anteckningar till den här bilden.")).color(p.faint));
                        return;
                    }
                    let font = FontId::new(14.0, FontFamily::Proportional);
                    let mut job = LayoutJob::default();
                    job.wrap.max_width = ui.available_width();
                    let mut at = 0;
                    for h in &hits {
                        if h.start < at || h.end > text.len() {
                            continue;
                        }
                        job.append(&text[at..h.start], 0.0, TextFormat::simple(font.clone(), p.text));
                        let bg = if Some(*h) == cur { Color32::from_rgba_unmultiplied(255, 150, 0, 140) } else { Color32::from_rgba_unmultiplied(255, 220, 0, 110) };
                        job.append(&text[h.start..h.end], 0.0, TextFormat { background: bg, ..TextFormat::simple(font.clone(), p.text) });
                        at = h.end;
                    }
                    job.append(&text[at..], 0.0, TextFormat::simple(font.clone(), p.text));
                    let galley = ui.painter().layout_job(job);
                    let resp = ui.add(egui::Label::new(galley.clone()).selectable(true));
                    // Rulla fram till den aktuella träffen (en gång per träff).
                    if let Some(h) = cur.filter(|h| h.slide == i && h.place == Place::Notes && h.start <= text.len()) {
                        if self.find.notes_scrolled != Some(h) {
                            self.find.notes_scrolled = Some(h);
                            let at = egui::text::CCursor::new(text[..h.start].chars().count());
                            let r = galley.pos_from_cursor(at).translate(resp.rect.min.to_vec2());
                            ui.scroll_to_rect(r.expand(8.0), Some(Align::Center));
                        }
                    }
                });
            });
    }

    /// Miniatyrlisten i sidopanelen: klick hoppar, aktuell bild är markerad, dolda bilder tonade.
    pub fn sidebar(&mut self, ui: &mut Ui, p: &Palette, prefs: &Prefs) {
        if !prefs.thumbs || self.count() == 0 {
            return;
        }
        let ctx = ui.ctx().clone();
        self.pool.receive(&ctx);
        let ppp = ctx.pixels_per_point();
        let (sw, sh) = self.deck.size_pt();
        let width = ui.available_width();
        let follow = self.thumb_follow.take();
        let mut clicked = None;
        let mut visible: Vec<usize> = Vec::new();
        let mut rows: Vec<(usize, Rect)> = Vec::new();
        let editing = self.editing();
        Panel::left("kladd-pptx-sidebar")
            .resizable(true)
            .default_size(200.0)
            .size_range(120.0..=(width * 0.4).max(160.0))
            .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin { left: 6, right: 8, top: 10, bottom: 0 }))
            .show(ui, |ui| {
                ScrollArea::vertical().id_salt("kladd-pptx-thumbs").auto_shrink([false, false]).show(ui, |ui| {
                    let tw = (ui.available_width() - 30.0).max(60.0);
                    let th = tw * sh / sw.max(1.0);
                    // Bredden avrundas uppåt så att små storleksändringar inte ger nya renderingar.
                    let px = ((tw * ppp / 32.0).ceil() * 32.0) as u32;
                    for i in 0..self.count() {
                        // En åtgärd i högerklicksmenyn kan ha ändrat antalet bilder.
                        if i >= self.count() {
                            break;
                        }
                        let sense = if editing { Sense::click_and_drag() } else { Sense::click() };
                        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), th + 12.0), sense);
                        rows.push((i, row));
                        if editing {
                            self.thumb_drag(ui, i, &resp, row, p);
                            resp.context_menu(|ui| self.slide_menu(ui, p, i));
                        }
                        let rect = Rect::from_min_size(pos2(row.left() + 26.0, row.top() + 4.0), vec2(tw, th));
                        if follow == Some(i) {
                            ui.scroll_to_rect(row, None);
                        }
                        if !ui.is_rect_visible(row) {
                            continue;
                        }
                        visible.push(i);
                        let hidden = self.deck.slides[i].hidden;
                        let cur = i == self.current;
                        let painter = ui.painter();
                        if resp.hovered() && !cur {
                            painter.rect_filled(row, 6.0, p.surface_hover);
                        }
                        // Numret till vänster (överstruket för dolda bilder, som i PowerPoint).
                        let ncol = if cur { p.accent } else if hidden { p.faint } else { p.muted };
                        let g = painter.layout_no_wrap((i + 1).to_string(), FontId::new(11.5, FontFamily::Proportional), ncol);
                        let npos = pos2(row.left() + 20.0 - g.size().x, rect.top());
                        let nrect = Rect::from_min_size(npos, g.size());
                        painter.galley(npos, g, ncol);
                        if hidden {
                            painter.line_segment([nrect.left_center(), nrect.right_center()], Stroke::new(1.0, ncol));
                        }
                        match self.pool.get(Kind::Thumb, i).or_else(|| self.pool.get(Kind::Page, i)) {
                            Some(tex) => {
                                painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                            }
                            None => {
                                painter.rect_filled(rect, 0.0, Color32::WHITE);
                            }
                        }
                        if hidden {
                            painter.rect_filled(rect, 0.0, Color32::from_white_alpha(110));
                        }
                        let stroke = if cur { Stroke::new(2.5, p.accent) } else { Stroke::new(1.0, p.line) };
                        painter.rect_stroke(rect, 1.0, stroke, StrokeKind::Outside);
                        let label = if hidden { tf!("Bild {n} (dold)", n = i + 1) } else { tf!("Bild {n}", n = i + 1) };
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone()));
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        self.pool.want(Kind::Thumb, i, px);
                    }
                    if editing {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            ui.menu_button(t("+ Ny bild"), |ui| {
                                let last = self.count().checked_sub(1);
                                self.layout_items(ui, p, last.map(|_| self.current));
                            });
                        });
                        ui.add_space(10.0);
                    }
                });
            });
        self.thumb_drop(&ctx, &rows);
        if let (Some(&lo), Some(&hi)) = (visible.first(), visible.last()) {
            self.pool.retain(Kind::Thumb, |s| s + 40 >= lo && s <= hi + 40);
        }
        if let Some(i) = clicked {
            self.go_to(i);
        }
    }

    /// Verktygsraden under flikraden.
    pub fn toolbar(&mut self, ui: &mut Ui, p: &Palette, prefs: &mut Prefs) -> Option<Action> {
        let ctx = ui.ctx().clone();
        let mut act = None;
        let n = self.count();
        Panel::top("kladd-pptx-bar")
            .exact_size(BAR_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin::symmetric(12, 5)))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().hline((r.left() - 12.0)..=(r.right() + 12.0), r.bottom() + 5.0, Stroke::new(1.0, p.line));
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let cur = self.current;
                    if tool_btn(ui, p, G::Sidebar, prefs.thumbs, true, t("Miniatyrer (F4)")).clicked() {
                        prefs.thumbs = !prefs.thumbs;
                    }
                    bar_divider(ui, p);
                    if tool_btn(ui, p, G::Std(Icon::ChevronUp), false, cur > 0, t("Föregående bild (PgUp)")).clicked() {
                        self.go_to(cur - 1);
                    }
                    let field_id = Id::unique("kladd-pptx-slide");
                    if !ctx.memory(|m| m.has_focus(field_id)) {
                        self.page_text = (cur + 1).to_string();
                    }
                    let field = ui.add(TextEdit::singleline(&mut self.page_text).id(field_id).desired_width(40.0).align(Align2::CENTER_CENTER).margin(Margin::symmetric(4, 4)));
                    field.clone().on_hover_text(t("Gå till bild"));
                    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        if let Some(i) = parse_slide_input(&self.page_text, n) {
                            self.go_to(i);
                        }
                    }
                    ui.label(RichText::new(format!("/ {n}")).color(p.muted));
                    if tool_btn(ui, p, G::Std(Icon::ChevronDown), false, cur + 1 < n, t("Nästa bild (PgDn)")).clicked() {
                        self.go_to(cur + 1);
                    }
                    bar_divider(ui, p);
                    if tool_btn(ui, p, G::Std(Icon::Minus), false, true, t("Zooma ut (Ctrl+−)")).clicked() {
                        prefs.zoom = zoom_step(self.last_k, -1.0);
                    }
                    egui::ComboBox::from_id_salt("kladd-pptx-zoom").width(96.0).selected_text(zoom_label(prefs.zoom)).show_ui(ui, |ui| {
                        for z in [Zoom::Window, Zoom::Width, Zoom::Pct(0.5), Zoom::Pct(0.75), Zoom::Pct(1.0), Zoom::Pct(1.5), Zoom::Pct(2.0)] {
                            if ui.selectable_label(prefs.zoom == z, zoom_label(z)).clicked() {
                                prefs.zoom = z;
                            }
                        }
                    });
                    if tool_btn(ui, p, G::Std(Icon::Plus), false, true, t("Zooma in (Ctrl+plus)")).clicked() {
                        prefs.zoom = zoom_step(self.last_k, 1.0);
                    }
                    let view_btn = menu_trigger(ui, p, t("Visa"));
                    egui::Popup::menu(&view_btn).width(320.0).align(egui::RectAlign::BOTTOM_START).gap(6.0).show(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        menu_heading(ui, p, t("Visning"));
                        if crate::app::menu_item(ui, p, t("Bild för bild"), "", Some(!prefs.scroll), true) {
                            prefs.scroll = false;
                        }
                        if crate::app::menu_item(ui, p, t("Rullande lista"), "", Some(prefs.scroll), true) {
                            prefs.scroll = true;
                        }
                        crate::app::menu_divider(ui, p);
                        if crate::app::menu_item(ui, p, t("Miniatyrer"), "F4", Some(prefs.thumbs), true) {
                            prefs.thumbs = !prefs.thumbs;
                        }
                        if crate::app::menu_item(ui, p, t("Anteckningar"), "", Some(prefs.notes && self.has_notes), self.has_notes) {
                            prefs.notes = !prefs.notes;
                        }
                        crate::app::menu_divider(ui, p);
                        if crate::app::menu_item(ui, p, t("Bildspel från början"), "F5", None, true) {
                            act = Some(Action::Show(false));
                        }
                        if crate::app::menu_item(ui, p, t("Bildspel från aktuell bild"), "Shift+F5", None, true) {
                            act = Some(Action::Show(true));
                        }
                        if crate::app::menu_item(ui, p, t("Spela upp övergångar och animeringar"), "", Some(prefs.animate), true) {
                            prefs.animate = !prefs.animate;
                        }
                        crate::app::menu_divider(ui, p);
                        if crate::app::menu_item(ui, p, t("Exportera som PDF…"), "", None, true) {
                            act = Some(Action::ExportPdf);
                        }
                    });
                    bar_divider(ui, p);
                    if tool_btn(ui, p, G::Search, self.find.open, true, t("Sök i presentationen (Ctrl+F)")).clicked() {
                        if self.find.open {
                            self.close_find();
                        } else {
                            self.open_find();
                        }
                    }
                    if tool_btn(ui, p, G::Print, false, true, t("Skriv ut (Ctrl+P)")).clicked() {
                        act = Some(Action::Print);
                    }
                    bar_divider(ui, p);
                    let show = egui::Button::new(RichText::new(t("Bildspel")).color(p.text)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE);
                    if ui.add(show).on_hover_text(crate::keys::shortcut(t("Bildspel från början (F5), från aktuell bild (Shift+F5)"))).clicked() {
                        act = Some(Action::Show(false));
                    }
                    if self.editing() {
                        bar_divider(ui, p);
                        if tool_btn(ui, p, G::Std(Icon::Undo), false, self.edit.session.can_undo(), t("Ångra (Ctrl+Z)")).clicked() {
                            self.undo(false);
                        }
                        if tool_btn(ui, p, G::Std(Icon::Redo), false, self.edit.session.can_redo(), t("Gör om (Ctrl+Y)")).clicked() {
                            self.undo(true);
                        }
                        let new_btn = menu_trigger(ui, p, t("Ny bild"));
                        egui::Popup::menu(&new_btn).width(260.0).align(egui::RectAlign::BOTTOM_START).gap(6.0).show(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            menu_heading(ui, p, t("Layout (Ctrl+M: samma som aktuell bild)"));
                            let cur = (n > 0).then_some(self.current);
                            self.layout_items(ui, p, cur);
                        });
                        let ins = menu_trigger(ui, p, t("Infoga"));
                        egui::Popup::menu(&ins).width(220.0).align(egui::RectAlign::BOTTOM_START).gap(6.0).show(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            if crate::app::menu_item(ui, p, t("Textruta"), "", None, n > 0) {
                                self.insert_textbox();
                            }
                            if crate::app::menu_item(ui, p, t("Bild…"), "Ctrl+Shift+I", None, n > 0) {
                                act = Some(Action::InsertPicture);
                            }
                        });
                        let has_sel = self.edit.sel.is_some();
                        if tool_btn(ui, p, G::Std(Icon::Close), false, has_sel, t("Ta bort formen (Delete)")).clicked() {
                            self.delete_selected();
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let edit = egui::Button::new(RichText::new(t("Redigera en kopia")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                            .fill(p.accent)
                            .stroke(Stroke::NONE)
                            .corner_radius(8)
                            .min_size(vec2(92.0, 28.0));
                        if ui.add(edit).on_hover_text(t("Presentationens text som ett nytt dokument (presentationen ändras inte)")).clicked() {
                            act = Some(Action::EditCopy);
                        }
                        let on = self.editing();
                        let label = if on { t("Klar") } else { t("Redigera") };
                        let toggle = egui::Button::new(RichText::new(label).font(FontId::new(13.5, theme::medium())).color(p.text))
                            .fill(if on { p.accent.gamma_multiply(0.16) } else { Color32::TRANSPARENT })
                            .stroke(Stroke::new(1.0, p.line))
                            .corner_radius(8)
                            .min_size(vec2(72.0, 28.0));
                        let tip = if on { t("Tillbaka till visning (Ctrl+E)") } else { t("Ändra presentationen: flytta och ändra former, skriv text, lägg till bilder (Ctrl+E)") };
                        if ui.add(toggle).on_hover_text(crate::keys::shortcut(tip)).clicked() {
                            self.toggle_edit();
                        }
                    });
                });
            });
        act
    }

    /// Kortkommandon i vyn (utanför bildspelet).
    pub fn handle_keys(&mut self, ctx: &egui::Context, prefs: &mut Prefs) -> Option<Action> {
        // Redigeringens tangenter först (de som används tas bort ur kön).
        self.edit_keys(ctx);
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let mut act = None;
        // Alt+F5 (bildspel med föredragsvyn) och Skift+F5 före F5: `consume_key` bryr sig inte om extra Alt/Skift.
        if key(Modifiers::ALT, Key::F5) {
            self.presenter_on_start = true;
            act = Some(Action::Show(false));
        } else if key(Modifiers::SHIFT, Key::F5) {
            act = Some(Action::Show(true));
        } else if key(Modifiers::NONE, Key::F5) {
            act = Some(Action::Show(false));
        }
        if key(Modifiers::NONE, Key::F4) {
            prefs.thumbs = !prefs.thumbs;
        }
        if self.find.open {
            if key(Modifiers::SHIFT, Key::F3) {
                self.find_step(true, prefs);
            } else if key(Modifiers::NONE, Key::F3) {
                self.find_step(false, prefs);
            }
            if key(Modifiers::NONE, Key::Escape) {
                self.close_find();
            }
        }
        let cmd = Modifiers::COMMAND;
        if key(cmd, Key::Plus) || key(cmd, Key::Equals) {
            prefs.zoom = zoom_step(self.last_k, 1.0);
        }
        if key(cmd, Key::Minus) {
            prefs.zoom = zoom_step(self.last_k, -1.0);
        }
        if key(cmd, Key::Num0) || key(cmd, Key::Num3) {
            prefs.zoom = Zoom::Window;
        }
        if key(cmd, Key::Num1) {
            prefs.zoom = Zoom::Pct(1.0);
        }
        if key(cmd, Key::Num2) {
            prefs.zoom = Zoom::Width;
        }
        // Bläddring: inte när ett textfält (sök, bildnummer) har fokus eller man skriver i en form.
        let overlay = self.edit_focus_id();
        if ctx.memory(|m| m.focused().is_some_and(|f| Some(f) != overlay)) || self.edit.text.is_some() {
            return act;
        }
        let single = !prefs.scroll;
        let cur = self.current;
        let prev = key(Modifiers::NONE, Key::PageUp)
            | key(Modifiers::NONE, Key::ArrowLeft)
            | (single && (key(Modifiers::SHIFT, Key::Space) | key(Modifiers::NONE, Key::ArrowUp) | key(Modifiers::NONE, Key::Backspace)));
        let next = key(Modifiers::NONE, Key::PageDown)
            | key(Modifiers::NONE, Key::ArrowRight)
            | (single && (key(Modifiers::NONE, Key::Space) | key(Modifiers::NONE, Key::ArrowDown)));
        if prev && cur > 0 {
            self.go_to(cur - 1);
        }
        if next {
            self.go_to(cur + 1);
        }
        if key(Modifiers::NONE, Key::Home) {
            self.go_to(0);
        }
        if key(Modifiers::NONE, Key::End) {
            self.go_to(self.count().saturating_sub(1));
        }
        act
    }

    /// Zoomsteg från menyn (`Ctrl++`, `Ctrl+−`, normal storlek = hel bild).
    pub fn zoom_cmd(&self, prefs: &mut Prefs, dir: f32) {
        prefs.zoom = if dir == 0.0 { Zoom::Window } else { zoom_step(self.last_k, dir) };
    }

    // -------------------------------------------------------- PDF-export

    pub fn exporting(&self) -> bool {
        self.export.dialog.is_some() || self.export.writing.is_some()
    }

    /// Frågar efter filnamn i en egen tråd (fildialogen blockerar).
    pub fn begin_export(&mut self, ctx: &egui::Context, name: String, dir: Option<PathBuf>) {
        let (tx, rx) = channel();
        self.export.dialog = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new().set_title(t("Exportera som PDF")).set_file_name(name).add_filter("PDF", &["pdf"]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let _ = tx.send(dlg.save_file());
            ctx.request_repaint();
        });
    }

    /// Skriver PDF:en till `path` i en bakgrundstråd.
    pub fn write_pdf(&mut self, ctx: &egui::Context, path: PathBuf) {
        let (tx, rx) = channel();
        self.export.writing = Some(rx);
        let deck = self.deck.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("pptx-pdf".into()).stack_size(8 << 20).spawn(move || {
            let res = pdf_of(&deck).and_then(|bytes| crate::doc::write_atomic(&path, &bytes).map_err(|e| crate::doc::describe_io_error(&e, &path))).map(|_| path);
            let _ = tx.send(res);
            ctx.request_repaint();
        });
    }

    /// Följer exporten: `Some(Ok(sökväg))`/`Some(Err(fel))` när den är klar.
    pub fn poll_export(&mut self, ctx: &egui::Context) -> Option<Result<PathBuf, String>> {
        if let Some(rx) = &self.export.dialog {
            match rx.try_recv() {
                Ok(Some(path)) => {
                    self.export.dialog = None;
                    let path = if path.extension().is_none() { path.with_extension("pdf") } else { path };
                    self.write_pdf(ctx, path);
                }
                Ok(None) | Err(TryRecvError::Disconnected) => self.export.dialog = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.export.writing {
            match rx.try_recv() {
                Ok(r) => {
                    self.export.writing = None;
                    return Some(r);
                }
                Err(TryRecvError::Disconnected) => {
                    self.export.writing = None;
                    return Some(Err(t("PDF-exporten avbröts.").to_owned()));
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        }
        None
    }
}

/// Presentationens synliga bilder som PDF. Fel (och panik i ritmotorn) blir ett meddelande.
pub fn pdf_of(deck: &Deck) -> Result<Vec<u8>, String> {
    let slides: Vec<usize> = (0..deck.slides.len()).filter(|&i| !deck.slides[i].hidden).collect();
    if slides.is_empty() {
        return Err(t("Presentationen har inga synliga bilder.").to_owned());
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::pdf::write(deck, &slides))).unwrap_or_else(|_| Err(t("PDF:en kunde inte skapas.").to_owned()))
}

/// Statusraden: bildnummer och antal.
pub fn status(ui: &mut Ui, p: &Palette, font: &FontId, tab: &PptxTab) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        let label = |ui: &mut Ui, s: String| {
            ui.label(RichText::new(s).font(font.clone()).color(p.muted));
        };
        let n = tab.count();
        let cur = tab.current.min(n.saturating_sub(1));
        let hidden = tab.deck.slides.get(cur).is_some_and(|s| s.hidden);
        let text = if hidden { tf!("Bild {n} av {total} (dold)", n = cur + 1, total = n) } else { tf!("Bild {n} av {total}", n = cur + 1, total = n) };
        label(ui, text);
        let mode = if tab.editing() {
            t("Redigering")
        } else if tab.unsaved() {
            t("Osparade ändringar")
        } else {
            t("Bara läsning")
        };
        label(ui, mode.to_owned());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoomlägen() {
        // 960×540 pt i en yta på 1000×1000: hel bild begränsas av bredden, bildbredd likaså.
        let k = scale_for(Zoom::Window, 1000.0, 1000.0, 960.0, 540.0);
        assert!((k - 1000.0 / 960.0).abs() < 1e-4);
        let k = scale_for(Zoom::Window, 2000.0, 540.0, 960.0, 540.0);
        assert!((k - 1.0).abs() < 1e-4, "höjden begränsar");
        assert!((scale_for(Zoom::Width, 2000.0, 540.0, 960.0, 540.0) - 2000.0 / 960.0).abs() < 1e-4);
        assert!((scale_for(Zoom::Pct(1.0), 10.0, 10.0, 960.0, 540.0) - PT_TO_PX).abs() < 1e-4);
        // Ett litet fönster ger aldrig en osynlig bild.
        assert!(scale_for(Zoom::Window, 0.0, -5.0, 960.0, 540.0) * 960.0 >= 40.0 - 1e-3);
    }

    #[test]
    fn zoomsteg_och_etiketter() {
        assert_eq!(zoom_step(PT_TO_PX, 1.0), Zoom::Pct(1.25));
        assert_eq!(zoom_step(PT_TO_PX, -1.0), Zoom::Pct(0.8));
        assert_eq!(zoom_step(PT_TO_PX * 100.0, 1.0), Zoom::Pct(ZOOM_MAX));
        assert_eq!(zoom_step(0.0, -1.0), Zoom::Pct(ZOOM_MIN));
        assert_eq!(zoom_label(Zoom::Pct(1.5)), "150 %");
    }

    #[test]
    fn bildnummer_i_fältet() {
        assert_eq!(parse_slide_input(" 3 ", 5), Some(2));
        assert_eq!(parse_slide_input("0", 5), None);
        assert_eq!(parse_slide_input("6", 5), None);
        assert_eq!(parse_slide_input("x", 5), None);
    }

    #[test]
    fn lägesminnet_har_tak_och_senaste_först() {
        let mut list = Vec::new();
        for i in 0..5 {
            remember(&mut list, format!("f{i}"), Memory { slide: i }, 3);
        }
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], ("f4".to_owned(), Memory { slide: 4 }));
        remember(&mut list, "f2".into(), Memory { slide: 9 }, 3);
        assert_eq!(list[0].0, "f2");
        assert_eq!(list.iter().filter(|(k, _)| k == "f2").count(), 1);
    }

    #[test]
    fn inställningar_tål_gamla_och_nya_filer() {
        let p: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, Prefs::default());
        let p: Prefs = serde_json::from_str(r#"{"scroll":true,"zoom":{"Pct":1.5}}"#).unwrap();
        assert!(p.scroll && p.thumbs && p.zoom == Zoom::Pct(1.5));
    }
}
