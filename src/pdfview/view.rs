//! PDF-visarens canvas: sidlayout, zoom, rendering till texturer, navigering, markering, sök och länkar.
//!
//! * Alla koordinater mot motorn, textlagret och länkarna är *visningspunkter* (pt): origo uppe till
//!   vänster i sidans beskärningsruta efter `/Rotate`, y nedåt. Användarens extra rotation (90° steg)
//!   är inbakad i bilden, så för text och länkar används [`PageXf`] för att komma till skärmen.
//! * Zoom 1,0 = 100 % = 96 dpi, alltså `PX` (96/72) egui-punkter per pt. Bilder renderas i
//!   `zoom * PX * pixels_per_point` bildpunkter per pt.
//! * Layouten ([`Layout`]) räknas om när zoom, rotation, uppslag eller vyns bredd ändras. Rullning
//!   mot en sida eller en träff sker genom `pending_nav`/`want_offset` som löses mot layouten i `show`.
//! * Texturer cachas per sida med den skala/rotation/omfärgning de renderades i. Är bilden inte
//!   skarp längre ritas den gamla utsträckt tills den nya kommer (zoom och omfärgning).
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::containers::scroll_area::{DragScroll, ScrollSource};
use egui::emath::GuiRounding;
use egui::style::ScrollAnimation;
use egui::{
    Align2, Color32, Context, CursorIcon, FontId, Key, Modifiers, PointerButton, Pos2, Rect, ScrollArea, Sense, Shadow,
    Spinner, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};

use super::engine::{EngineEvent, PageSize, PdfEngine, Recolor, RenderRequest, effective_scale};
use super::meta::{Dest, LinkTarget, PdfMeta};
use super::text::PageText;
use crate::i18n::t;
use crate::tf;
use crate::theme::Palette;

/// egui-punkter per pt vid zoom 1,0 (96 dpi).
const PX: f32 = 96.0 / 72.0;
/// Mellanrum mellan sidor.
const GAP: f32 = 10.0;
/// Marginal runt alla sidorna.
const MARGIN: f32 = 12.0;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 25.0;
/// Tak för texturcachen (bytes).
const CACHE_LIMIT: usize = 256 * 1024 * 1024;
const MAX_THUMBS: usize = 240;
/// Tak för antalet sidors textlager i minnet (stora dokument: sökningen läser en sida i taget och
/// släpper den). Sidor nära vyn och sidor med OCR-text rörs aldrig.
const MAX_TEXT_PAGES: usize = 160;
const SPINNER_AFTER: Duration = Duration::from_millis(400);
/// Så länge efter en zoomändring väntar vi med att begära skarpa bilder för sidor som redan har en.
const ZOOM_SETTLE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ZoomMode {
    #[default]
    Auto,
    ActualSize,
    PageFit,
    PageWidth,
    Custom,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Spread {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tool {
    #[default]
    Select,
    Hand,
}

/// Sparas per fil i inställningarna (kom ihåg läget). `page` är översta synliga sidan och
/// `y_frac` hur långt (0..1) vyns överkant ligger ner i den sidan.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ViewMemory {
    pub page: usize,
    pub y_frac: f32,
    pub zoom_mode: ZoomMode,
    pub zoom: f32,
    pub rotation: u16,
    pub spread: Spread,
}

/// Sökningens läge: `current` är 0-baserat index bland alla träffar i dokumentordning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindStatus {
    pub searching: bool,
    pub current: Option<usize>,
    pub total: usize,
    pub done_pages: usize,
}

// ---------------------------------------------------------------------------------------------
// Rena delar: rotation, uppslag, layout, zoom, historik, markering.
// ---------------------------------------------------------------------------------------------

/// Sidans storlek i pt efter användarens rotation.
pub fn rotated_size(s: PageSize, rot: u16) -> (f32, f32) {
    if rot % 180 == 90 { (s.height, s.width) } else { (s.width, s.height) }
}

/// Omvandling mellan en sidas visningspunkter (före användarens rotation) och skärmen.
#[derive(Clone, Copy, Debug)]
pub struct PageXf {
    /// Skärmposition för sidans övre vänstra hörn (efter rotation).
    pub origin: Pos2,
    /// egui-punkter per pt.
    pub k: f32,
    /// Sidans storlek i pt före användarens rotation.
    pub w: f32,
    pub h: f32,
    pub rot: u16,
}

impl PageXf {
    /// pt före rotation → pt i den roterade sidan (medurs).
    pub fn to_view(&self, p: Pos2) -> Pos2 {
        match self.rot % 360 {
            90 => Pos2::new(self.h - p.y, p.x),
            180 => Pos2::new(self.w - p.x, self.h - p.y),
            270 => Pos2::new(p.y, self.w - p.x),
            _ => p,
        }
    }

    pub fn from_view(&self, q: Pos2) -> Pos2 {
        match self.rot % 360 {
            90 => Pos2::new(q.y, self.h - q.x),
            180 => Pos2::new(self.w - q.x, self.h - q.y),
            270 => Pos2::new(self.w - q.y, q.x),
            _ => q,
        }
    }

    pub fn to_screen(&self, p: Pos2) -> Pos2 {
        self.origin + self.to_view(p).to_vec2() * self.k
    }

    pub fn from_screen(&self, s: Pos2) -> Pos2 {
        self.from_view(Pos2::ZERO + (s - self.origin) / self.k)
    }

    pub fn rect_to_screen(&self, r: Rect) -> Rect {
        Rect::from_two_pos(self.to_screen(r.min), self.to_screen(r.max))
    }
}

/// Rad och kolumn för en sida i uppslaget.
pub fn page_cell(page: usize, spread: Spread) -> (usize, usize) {
    match spread {
        Spread::None => (page, 0),
        Spread::Odd => (page / 2, page % 2),
        Spread::Even if page == 0 => (0, 0),
        Spread::Even => (1 + (page - 1) / 2, (page - 1) % 2),
    }
}

/// Sidintervall per rad.
pub fn spread_rows(n: usize, spread: Spread) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut i = match spread {
        Spread::Even if n > 0 => {
            rows.push(0..1);
            1
        }
        _ => 0,
    };
    let step = if spread == Spread::None { 1 } else { 2 };
    while i < n {
        rows.push(i..(i + step).min(n));
        i += step;
    }
    rows
}

#[derive(Clone, Debug)]
pub struct Row {
    pub top: f32,
    pub bottom: f32,
    pub pages: Range<usize>,
}

/// Sidornas rutor i innehållskoordinater (egui-punkter, origo = innehållets övre vänstra hörn).
#[derive(Clone, Debug)]
pub struct Layout {
    pub rects: Vec<Rect>,
    pub rows: Vec<Row>,
    pub size: Vec2,
    pub zoom: f32,
    pub rotation: u16,
    pub spread: Spread,
    pub vp_w: f32,
}

pub fn build_layout(sizes: &[PageSize], rotation: u16, spread: Spread, zoom: f32, vp_w: f32) -> Layout {
    let k = zoom * PX;
    let rows_def = spread_rows(sizes.len(), spread);
    let dims: Vec<Vec2> = sizes
        .iter()
        .map(|s| {
            let (w, h) = rotated_size(*s, rotation);
            vec2(w * k, h * k)
        })
        .collect();
    let row_w = |r: &Range<usize>| dims[r.clone()].iter().map(|d| d.x).sum::<f32>() + GAP * (r.len().saturating_sub(1)) as f32;
    let widest = rows_def.iter().map(row_w).fold(0.0, f32::max);
    let width = vp_w.max(widest + 2.0 * MARGIN);
    let mut rects = vec![Rect::NOTHING; sizes.len()];
    let mut rows = Vec::with_capacity(rows_def.len());
    let mut y = MARGIN;
    for r in rows_def {
        let mut x = (width - row_w(&r)) / 2.0;
        let h = dims[r.clone()].iter().map(|d| d.y).fold(0.0, f32::max);
        for p in r.clone() {
            rects[p] = Rect::from_min_size(Pos2::new(x, y), dims[p]);
            x += dims[p].x + GAP;
        }
        rows.push(Row { top: y, bottom: y + h, pages: r });
        y += h + GAP;
    }
    let height = if rows.is_empty() { 2.0 * MARGIN } else { y - GAP + MARGIN };
    Layout { rects, rows, size: vec2(width, height), zoom, rotation, spread, vp_w }
}

impl Layout {
    /// Radintervallet som skär y-intervallet.
    pub fn rows_in(&self, y0: f32, y1: f32) -> Range<usize> {
        let a = self.rows.partition_point(|r| r.bottom < y0);
        let b = self.rows.partition_point(|r| r.top <= y1);
        a..b.max(a)
    }

    pub fn row_of(&self, page: usize) -> usize {
        self.rows.partition_point(|r| r.pages.end <= page).min(self.rows.len().saturating_sub(1))
    }

    /// Närmaste sida till punkten (alltid en sida om dokumentet har några).
    pub fn nearest_page(&self, p: Pos2) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        let ri = self.rows.partition_point(|r| r.bottom < p.y);
        // Mellan två rader: välj den närmaste.
        let ri = if ri >= self.rows.len() {
            self.rows.len() - 1
        } else if ri > 0 && (p.y - self.rows[ri - 1].bottom) < (self.rows[ri].top - p.y) {
            ri - 1
        } else {
            ri
        };
        self.rows[ri]
            .pages
            .clone()
            .min_by(|&a, &b| {
                let da = (self.rects[a].center().x - p.x).abs();
                let db = (self.rects[b].center().x - p.x).abs();
                da.total_cmp(&db)
            })
            .unwrap_or(0)
    }

    /// Sidan som punkten ligger på, om någon.
    pub fn page_at(&self, p: Pos2) -> Option<usize> {
        if self.rects.is_empty() {
            return None;
        }
        let page = self.nearest_page(p);
        self.rects[page].contains(p).then_some(page)
    }
}

/// Ankare i en sida (andelar av sidans ruta) för att hålla en punkt still vid zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub page: usize,
    pub fx: f32,
    pub fy: f32,
}

pub fn anchor_at(layout: &Layout, content_pos: Pos2) -> Option<Anchor> {
    if layout.rects.is_empty() {
        return None;
    }
    let page = layout.nearest_page(content_pos);
    let r = layout.rects[page];
    Some(Anchor { page, fx: (content_pos.x - r.min.x) / r.width().max(1.0), fy: (content_pos.y - r.min.y) / r.height().max(1.0) })
}

/// Rullningsläget som lägger ankaret på `vp_pos` (relativt vyns hörn) i den nya layouten.
pub fn offset_for(layout: &Layout, a: &Anchor, vp_pos: Vec2) -> Vec2 {
    let r = layout.rects[a.page.min(layout.rects.len() - 1)];
    vec2(r.min.x + a.fx * r.width(), r.min.y + a.fy * r.height()) - vp_pos
}

/// Håller en punkt still vid enkel proportionell zoom (utan sidlayout): ny rullning.
pub fn zoom_offset(offset: Vec2, anchor_in_viewport: Vec2, old_zoom: f32, new_zoom: f32) -> Vec2 {
    (offset + anchor_in_viewport) * (new_zoom / old_zoom) - anchor_in_viewport
}

/// Zoom för lägena som följer fönstret. `None` för Custom. Alla sidor räknas, så zoomen ändras
/// inte medan man rullar genom ett dokument med blandade storlekar.
pub fn fit_zoom(mode: ZoomMode, vp: Vec2, sizes: &[PageSize], rotation: u16, spread: Spread) -> Option<f32> {
    let z = match mode {
        ZoomMode::Custom => return None,
        ZoomMode::ActualSize => 1.0,
        ZoomMode::Auto | ZoomMode::PageFit | ZoomMode::PageWidth => {
            let rows = spread_rows(sizes.len(), spread);
            let dim = |p: usize| rotated_size(sizes[p], rotation);
            let mut max_w = 1.0f32;
            let mut max_cols = 1usize;
            for r in &rows {
                let w: f32 = r.clone().map(|p| dim(p).0).sum();
                if w > max_w {
                    max_w = w;
                    max_cols = r.len();
                }
            }
            let max_h = (0..sizes.len()).map(|p| dim(p).1).fold(1.0, f32::max);
            let avail_w = (vp.x - 2.0 * MARGIN - GAP * (max_cols - 1) as f32).max(20.0);
            let avail_h = (vp.y - 2.0 * MARGIN).max(20.0);
            let fit_w = avail_w / (max_w * PX);
            let fit_h = avail_h / (max_h * PX);
            match mode {
                ZoomMode::PageWidth => fit_w,
                ZoomMode::PageFit => fit_w.min(fit_h),
                _ => {
                    let first = if sizes.is_empty() { (1.0, 1.0) } else { dim(0) };
                    if first.1 >= first.0 { fit_w.min(1.25) } else { fit_w.min(fit_h) }
                }
            }
        }
    };
    Some(z.clamp(MIN_ZOOM, MAX_ZOOM))
}

/// Historik: `stack[pos]` är där man står. Poster är (sida, y i visningspunkter från sidans topp).
#[derive(Default, Debug)]
pub struct History {
    stack: Vec<(usize, f32)>,
    pos: usize,
}

impl History {
    /// Hoppar från `cur` till `target`; det som låg framåt i historiken stryks.
    pub fn push(&mut self, cur: (usize, f32), target: (usize, f32)) {
        if self.stack.is_empty() {
            self.stack.push(cur);
            self.pos = 0;
        } else {
            self.stack[self.pos] = cur;
            self.stack.truncate(self.pos + 1);
        }
        self.stack.push(target);
        self.pos = self.stack.len() - 1;
    }

    pub fn back(&mut self, cur: (usize, f32)) -> Option<(usize, f32)> {
        if !self.can_back() {
            return None;
        }
        self.stack[self.pos] = cur;
        self.pos -= 1;
        Some(self.stack[self.pos])
    }

    pub fn forward(&mut self, cur: (usize, f32)) -> Option<(usize, f32)> {
        if !self.can_forward() {
            return None;
        }
        self.stack[self.pos] = cur;
        self.pos += 1;
        Some(self.stack[self.pos])
    }

    pub fn can_back(&self) -> bool {
        !self.stack.is_empty() && self.pos > 0
    }

    pub fn can_forward(&self) -> bool {
        self.pos + 1 < self.stack.len()
    }
}

/// (sida, glyfindex)
type GPos = (usize, usize);
/// Ett markeringssteg (tecken, ord eller rad): [start, slut) i dokumentordning.
type Unit = (GPos, GPos);

/// Markering över sidor: [start, end) i dokumentordning. `usize::MAX` som glyfindex = sidans slut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sel {
    start: GPos,
    end: GPos,
}

/// Förenar två steg till en markering (ordnad oavsett dragriktning).
fn union_units(a: Unit, b: Unit) -> Sel {
    Sel { start: a.0.min(b.0), end: a.1.max(b.1) }
}

/// Glyfintervallet av markeringen på `page` (med `len` glyfer), om något.
fn sel_range_on_page(sel: &Sel, page: usize, len: usize) -> Option<Range<usize>> {
    if page < sel.start.0 || page > sel.end.0 {
        return None;
    }
    let s = if page == sel.start.0 { sel.start.1 } else { 0 };
    let e = if page == sel.end.0 { sel.end.1 } else { usize::MAX };
    let (s, e) = (s.min(len), e.min(len));
    (s < e).then_some(s..e)
}

// ---------------------------------------------------------------------------------------------
// Tillstånd
// ---------------------------------------------------------------------------------------------

struct Tex {
    handle: TextureHandle,
    /// Den skala bilden faktiskt renderades i.
    scale: f32,
    rotation: u16,
    recolor: Recolor,
    bytes: usize,
    used: u64,
}

struct Thumb {
    handle: TextureHandle,
    scale: f32,
    rotation: u16,
}

#[derive(Clone, Copy, Debug)]
enum Nav {
    /// Sida och y (visningspunkter, efter rotation) från sidans topp.
    At { page: usize, y: f32 },
    /// Sida och andel av sidans höjd.
    Frac { page: usize, frac: f32 },
}

struct Find {
    query: String,
    case_sensitive: bool,
    whole_word: bool,
    /// Träffar per sida (glyfintervall).
    matches: Vec<Vec<Range<usize>>>,
    searched: Vec<bool>,
    done: usize,
    total: usize,
    /// (sida, index bland sidans träffar)
    current: Option<(usize, usize)>,
    /// Välj och visa första träffen från nuvarande sida så fort den finns.
    jump_pending: bool,
    scroll_to_current: bool,
}

pub struct PdfView {
    engine: PdfEngine,
    meta: PdfMeta,
    bytes: Arc<Vec<u8>>,
    sizes: Vec<PageSize>,

    zoom_mode: ZoomMode,
    zoom: f32,
    rotation: u16,
    spread: Spread,
    recolor: Recolor,
    tool: Tool,
    presenting: bool,

    layout: Option<Arc<Layout>>,
    /// Rullning (innehållskoordinater för vyns övre vänstra hörn) och vyns storlek, från senaste bilden.
    scroll: Vec2,
    vp: Vec2,
    cur_page: usize,
    pending_nav: Option<Nav>,
    want_offset: Option<Vec2>,
    pending_scroll: Vec2,
    /// Där zoomen ska hålla fast (relativt vyns hörn); annars vyns mitt.
    zoom_anchor: Option<Vec2>,
    last_zoom_change: Instant,
    /// egui-punkter per pt i den här bilden (zoom × PX, eller anpassat i presentationsläget).
    k: f32,
    ppp_cache: f32,
    /// Rullning efter ett hopp: sidan behåller sitt nummer tills användaren rullar.
    nav_lock: Option<f32>,
    history: History,

    frame: u64,
    tex: HashMap<usize, Tex>,
    tex_bytes: usize,
    failed: HashMap<usize, String>,
    waiting: HashMap<usize, Instant>,
    visible: Vec<usize>,
    thumbs: HashMap<usize, Thumb>,
    thumb_wants: HashMap<usize, (f32, u64)>,

    texts: HashMap<usize, Arc<PageText>>,
    /// Sidor vars textlager kommer från OCR (motorns text får inte skriva över det).
    ocr_pages: HashSet<usize>,
    want_all_text: bool,
    sel: Option<Sel>,
    drag_anchor: Option<Unit>,
    /// Dragsteg: tecken, eller ord/rad när draget började med dubbel-/trippelklick.
    drag_mode: UnitMode,
    /// Senaste knapptryckningen (tid, plats, antal i följd) för att känna igen dubbel- och trippelklick vid drag.
    press_hist: Option<(f64, Pos2, u8)>,
    /// Blanksteg hålls nere; `true` när det använts för att dra (då ska det inte bläddra).
    space: Option<SpaceHold>,
    find: Option<Find>,
    pending_uri: Option<String>,
    /// Nya anteckningar som inte sparats än (annot.rs).
    pub annot: super::annot::AnnotState,
    /// Formulärfält (AcroForm): värden och redigering; se `forms.rs`.
    pub forms: super::forms::FormState,
}

impl PdfView {
    pub fn new(engine: PdfEngine, meta: PdfMeta, bytes: Arc<Vec<u8>>, memory: Option<ViewMemory>) -> Self {
        let n = engine.page_count();
        let sizes = (0..n).map(|p| engine.page_size(p)).collect();
        let mut v = PdfView {
            engine,
            meta,
            bytes,
            sizes,
            zoom_mode: ZoomMode::Auto,
            zoom: 1.0,
            rotation: 0,
            spread: Spread::None,
            recolor: Recolor::None,
            tool: Tool::Select,
            presenting: false,
            layout: None,
            scroll: Vec2::ZERO,
            vp: vec2(800.0, 600.0),
            cur_page: 0,
            pending_nav: None,
            want_offset: None,
            pending_scroll: Vec2::ZERO,
            zoom_anchor: None,
            last_zoom_change: Instant::now() - Duration::from_secs(10),
            k: PX,
            ppp_cache: 1.0,
            nav_lock: None,
            history: History::default(),
            frame: 0,
            tex: HashMap::new(),
            tex_bytes: 0,
            failed: HashMap::new(),
            waiting: HashMap::new(),
            visible: Vec::new(),
            thumbs: HashMap::new(),
            thumb_wants: HashMap::new(),
            texts: HashMap::new(),
            ocr_pages: HashSet::new(),
            want_all_text: false,
            sel: None,
            annot: Default::default(),
            drag_anchor: None,
            drag_mode: UnitMode::Char,
            press_hist: None,
            space: None,
            find: None,
            pending_uri: None,
            forms: Default::default(),
        };
        if let Some(m) = memory {
            v.zoom_mode = m.zoom_mode;
            if m.zoom.is_finite() && m.zoom > 0.0 {
                v.zoom = m.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
            }
            v.rotation = norm_rot(m.rotation);
            v.spread = m.spread;
            let page = m.page.min(n.saturating_sub(1));
            v.cur_page = page;
            v.pending_nav = Some(Nav::Frac { page, frac: if m.y_frac.is_finite() { m.y_frac.clamp(0.0, 1.0) } else { 0.0 } });
        }
        v
    }

    // -- Enkla läsare och inställningar ------------------------------------------------------

    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    /// 0-baserad sida i mitten av vyn.
    pub fn current_page(&self) -> usize {
        self.cur_page
    }

    /// Sidetiketten ("iv") om PDF:en har sådana, annars sidnumret ("4").
    pub fn page_label(&self, page: usize) -> String {
        match self.meta.labels.get(page) {
            Some(l) if !l.is_empty() => l.clone(),
            _ => (page + 1).to_string(),
        }
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn zoom_mode(&self) -> ZoomMode {
        self.zoom_mode
    }

    pub fn set_zoom_mode(&mut self, m: ZoomMode) {
        self.zoom_mode = m;
        if m == ZoomMode::ActualSize {
            self.zoom = 1.0;
        }
    }

    /// Fast zoom (Custom).
    pub fn set_zoom(&mut self, z: f32) {
        if z.is_finite() {
            self.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
            self.zoom_mode = ZoomMode::Custom;
        }
    }

    pub fn zoom_in(&mut self) {
        self.set_zoom(self.zoom * 1.1);
    }

    pub fn zoom_out(&mut self) {
        self.set_zoom(self.zoom / 1.1);
    }

    pub fn rotation(&self) -> u16 {
        self.rotation
    }

    /// Roterar vyn 90° (medurs eller moturs). Behåller översta sidan.
    pub fn rotate(&mut self, clockwise: bool) {
        let r = if clockwise { self.rotation + 90 } else { self.rotation + 270 };
        self.keep_top_page();
        self.rotation = norm_rot(r);
        self.clear_textures();
    }

    pub fn spread(&self) -> Spread {
        self.spread
    }

    pub fn set_spread(&mut self, s: Spread) {
        if s != self.spread {
            self.keep_top_page();
            self.spread = s;
        }
    }

    pub fn recolor(&self) -> Recolor {
        self.recolor
    }

    /// Byter omfärgning. Gamla bilder visas utsträckta tills de nya renderats.
    pub fn set_recolor(&mut self, r: Recolor) {
        self.recolor = r;
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn set_tool(&mut self, t: Tool) {
        self.tool = t;
    }

    pub fn presenting(&self) -> bool {
        self.presenting
    }

    pub fn set_presenting(&mut self, on: bool) {
        if on != self.presenting {
            self.presenting = on;
            // Tillbaka i rullvyn: stå kvar på aktuell sida.
            self.pending_nav = Some(Nav::At { page: self.cur_page, y: 0.0 });
        }
    }

    pub fn meta(&self) -> &PdfMeta {
        &self.meta
    }

    pub fn engine(&self) -> &PdfEngine {
        &self.engine
    }

    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }

    /// Lägger textigenkänd text som sidans textlager (sök, markering och kopiering).
    pub fn set_ocr_text(&mut self, page: usize, text: Arc<PageText>) {
        if page < self.sizes.len() {
            self.texts.insert(page, text);
            self.ocr_pages.insert(page);
            self.sel = None;
            // En pågående sökning ska också titta i den nya texten.
            if let Some(f) = &self.find {
                let (q, c, w) = (f.query.clone(), f.case_sensitive, f.whole_word);
                self.find_set(&q, c, w);
            }
        }
    }

    /// Uri som användaren klickat på; chrome frågar om den ska öppnas. Tas bara en gång.
    pub fn take_pending_uri(&mut self) -> Option<String> {
        self.pending_uri.take()
    }

    pub fn memory(&self) -> ViewMemory {
        let (page, y_frac) = self.top_page_frac();
        ViewMemory { page, y_frac, zoom_mode: self.zoom_mode, zoom: self.zoom, rotation: self.rotation, spread: self.spread }
    }

    /// Ersätter dokumentet efter att filen ändrats på disk; behåller sida, zoom och rotation.
    pub fn reload(&mut self, engine: PdfEngine, meta: PdfMeta, bytes: Arc<Vec<u8>>) {
        let (page, frac) = self.top_page_frac();
        self.engine = engine;
        self.meta = meta;
        self.bytes = bytes;
        self.sizes = (0..self.engine.page_count()).map(|p| self.engine.page_size(p)).collect();
        let n = self.sizes.len();
        self.clear_textures();
        self.thumbs.clear();
        self.thumb_wants.clear();
        self.texts.clear();
        self.ocr_pages.clear();
        self.sel = None;
        self.drag_anchor = None;
        self.space = None;
        self.layout = None;
        self.history = History::default();
        let page = page.min(n.saturating_sub(1));
        self.cur_page = page;
        self.pending_nav = Some(Nav::Frac { page, frac });
        if let Some(f) = self.find.take() {
            self.find_set(&f.query, f.case_sensitive, f.whole_word);
        }
    }

    fn clear_textures(&mut self) {
        self.tex.clear();
        self.tex_bytes = 0;
        self.failed.clear();
        self.waiting.clear();
        self.thumbs.clear();
    }

    /// Översta synliga sida och andel ner i den.
    fn top_page_frac(&self) -> (usize, f32) {
        if let Some(Nav::Frac { page, frac }) = self.pending_nav {
            return (page, frac);
        }
        if let Some(Nav::At { page, y }) = self.pending_nav {
            let h = self.sizes.get(page).map_or(1.0, |s| rotated_size(*s, self.rotation).1);
            return (page, (y / h).clamp(0.0, 1.0));
        }
        let Some(l) = &self.layout else { return (self.cur_page, 0.0) };
        let (page, frac) = top_of(l, self.scroll.y);
        (page, frac)
    }

    /// Plats för historiken: (sida, y i visningspunkter från sidans topp).
    fn cur_loc(&self) -> (usize, f32) {
        let Some(l) = &self.layout else { return (self.cur_page, 0.0) };
        let (page, _) = top_of(l, self.scroll.y);
        let r = l.rects[page];
        (page, ((self.scroll.y - r.top()) / (l.zoom * PX)).max(0.0))
    }

    fn keep_top_page(&mut self) {
        if self.pending_nav.is_none() {
            let (page, frac) = self.top_page_frac();
            self.pending_nav = Some(Nav::Frac { page, frac });
        }
    }

    // -- Navigering -----------------------------------------------------------------------------

    pub fn go_to_page(&mut self, page: usize) {
        let page = page.min(self.sizes.len().saturating_sub(1));
        self.cur_page = page;
        self.pending_nav = Some(Nav::At { page, y: 0.0 });
    }

    /// Hopp till ett mål (länk, innehållsförteckning); läggs i historiken.
    pub fn go_to(&mut self, dest: &Dest) {
        let page = dest.page.min(self.sizes.len().saturating_sub(1));
        let y = dest.y.map_or(0.0, |y| {
            let (w, h) = (self.sizes[page].width, self.sizes[page].height);
            let xf = PageXf { origin: Pos2::ZERO, k: 1.0, w, h, rot: self.rotation };
            xf.to_view(Pos2::new(0.0, y)).y.max(0.0)
        });
        let cur = self.cur_loc();
        self.history.push(cur, (page, y));
        self.cur_page = page;
        self.pending_nav = Some(Nav::At { page, y });
    }

    pub fn back(&mut self) {
        let cur = self.cur_loc();
        if let Some((page, y)) = self.history.back(cur) {
            self.cur_page = page;
            self.pending_nav = Some(Nav::At { page, y });
        }
    }

    pub fn forward(&mut self) {
        let cur = self.cur_loc();
        if let Some((page, y)) = self.history.forward(cur) {
            self.cur_page = page;
            self.pending_nav = Some(Nav::At { page, y });
        }
    }

    pub fn can_back(&self) -> bool {
        self.history.can_back()
    }

    pub fn can_forward(&self) -> bool {
        self.history.can_forward()
    }

    /// Hoppar en rad (ett uppslag) framåt/bakåt.
    fn step_row(&mut self, dir: i32) {
        let rows = spread_rows(self.sizes.len(), self.spread);
        if rows.is_empty() {
            return;
        }
        let (row, _) = page_cell(self.cur_page, self.spread);
        let target = (row as i64 + dir as i64).clamp(0, rows.len() as i64 - 1) as usize;
        self.go_to_page(rows[target].start);
    }

    fn scroll_by(&mut self, d: Vec2) {
        self.pending_scroll += d;
    }

    /// Ryms hela raden i bredd (ingen vågrät rullning)?
    fn fits_width(&self) -> bool {
        self.layout.as_ref().is_none_or(|l| l.size.x <= self.vp.x + 1.0)
    }

    /// Ryms hela sidan i höjd?
    fn fits_height(&self) -> bool {
        self.layout.as_ref().is_none_or(|l| l.rects.get(self.cur_page).is_none_or(|r| r.height() + 2.0 * MARGIN <= self.vp.y + 1.0))
    }

    // -- Tangenter ------------------------------------------------------------------------------

    pub fn handle_keys(&mut self, ctx: &Context) {
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let cmd = Modifiers::COMMAND;
        let typing = ctx.egui_wants_keyboard_input();
        if self.presenting {
            if typing {
                return;
            }
            self.present_keys(ctx);
            return;
        }
        // Zoom (item 15, 49: COMMAND är ⌘ på Mac).
        let plus = [(cmd, Key::Plus), (cmd | Modifiers::SHIFT, Key::Plus), (cmd, Key::Equals), (cmd | Modifiers::SHIFT, Key::Equals)];
        let mut zoomed_in = false;
        for (m, k) in plus {
            zoomed_in |= key(m, k);
        }
        if zoomed_in {
            self.zoom_in();
        }
        if key(cmd, Key::Minus) {
            self.zoom_out();
        }
        if key(cmd, Key::Num0) {
            self.set_zoom(1.0);
        }
        if key(cmd, Key::Num1) {
            self.set_zoom_mode(ZoomMode::ActualSize);
        }
        if key(cmd, Key::Num2) {
            self.set_zoom_mode(ZoomMode::PageWidth);
        }
        if key(cmd, Key::Num3) {
            self.set_zoom_mode(ZoomMode::PageFit);
        }
        if typing {
            self.space = None;
            return;
        }
        // Historik (23).
        if key(Modifiers::ALT, Key::ArrowLeft) {
            self.back();
        }
        if key(Modifiers::ALT, Key::ArrowRight) {
            self.forward();
        }
        // Markering (30).
        if key(cmd, Key::A) {
            self.select_all();
        }
        if self.sel.is_some() && key(cmd, Key::C) {
            self.copy_selection(ctx);
        }
        // Sök (33).
        if key(Modifiers::SHIFT, Key::F3) {
            self.find_next(true);
        }
        if key(Modifiers::NONE, Key::F3) {
            self.find_next(false);
        }
        // Första/sista sidan (21).
        if key(Modifiers::NONE, Key::Home) || key(cmd, Key::ArrowUp) {
            self.go_to_page(0);
        }
        if key(Modifiers::NONE, Key::End) || key(cmd, Key::ArrowDown) {
            self.go_to_page(self.sizes.len().saturating_sub(1));
        }
        // Sid- och skärmsteg (20).
        let screen = (self.vp.y * 0.9).max(100.0);
        // Blanksteg: dra med musen panorerar (som handverktyget); bara ett tryck utan drag bläddrar,
        // och då först när tangenten släpps.
        let (space_down, space_up) = (key(Modifiers::NONE, Key::Space), key(Modifiers::SHIFT, Key::Space));
        if (space_down || space_up) && self.space.is_none() {
            self.space = Some(SpaceHold { back: space_up, dragged: false });
        }
        let mut space_page = None;
        if let Some(hold) = self.space {
            if !ctx.input(|i| i.key_down(Key::Space)) {
                self.space = None;
                if !hold.dragged {
                    space_page = Some(hold.back);
                }
            }
        }
        let page_down = key(Modifiers::NONE, Key::PageDown) || space_page == Some(false);
        let page_up = key(Modifiers::NONE, Key::PageUp) || space_page == Some(true);
        if page_down {
            if self.fits_height() { self.step_row(1) } else { self.scroll_by(vec2(0.0, screen)) }
        }
        if page_up {
            if self.fits_height() { self.step_row(-1) } else { self.scroll_by(vec2(0.0, -screen)) }
        }
        if key(Modifiers::NONE, Key::N) || key(Modifiers::NONE, Key::J) {
            self.step_row(1);
        }
        if key(Modifiers::NONE, Key::P) || key(Modifiers::NONE, Key::K) {
            self.step_row(-1);
        }
        // Pilar (22).
        const STEP: f32 = 56.0;
        if key(Modifiers::NONE, Key::ArrowDown) {
            self.scroll_by(vec2(0.0, STEP));
        }
        if key(Modifiers::NONE, Key::ArrowUp) {
            self.scroll_by(vec2(0.0, -STEP));
        }
        if key(Modifiers::NONE, Key::ArrowRight) {
            if self.fits_width() { self.step_row(1) } else { self.scroll_by(vec2(STEP, 0.0)) }
        }
        if key(Modifiers::NONE, Key::ArrowLeft) {
            if self.fits_width() { self.step_row(-1) } else { self.scroll_by(vec2(-STEP, 0.0)) }
        }
        // Rotation (11) och verktyg.
        if key(Modifiers::SHIFT, Key::R) {
            self.rotate(false);
        }
        if key(Modifiers::NONE, Key::R) {
            self.rotate(true);
        }
        if key(Modifiers::NONE, Key::H) {
            self.tool = if self.tool == Tool::Hand { Tool::Select } else { Tool::Hand };
        }
    }

    fn present_keys(&mut self, ctx: &Context) {
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if key(Modifiers::NONE, Key::Escape) {
            self.set_presenting(false);
            return;
        }
        let last = self.sizes.len().saturating_sub(1);
        let next = key(Modifiers::NONE, Key::ArrowRight)
            | key(Modifiers::NONE, Key::ArrowDown)
            | key(Modifiers::NONE, Key::PageDown)
            | key(Modifiers::NONE, Key::Space)
            | key(Modifiers::NONE, Key::Enter);
        let prev = key(Modifiers::NONE, Key::ArrowLeft)
            | key(Modifiers::NONE, Key::ArrowUp)
            | key(Modifiers::NONE, Key::PageUp)
            | key(Modifiers::SHIFT, Key::Space)
            | key(Modifiers::NONE, Key::Backspace);
        if next {
            self.cur_page = (self.cur_page + 1).min(last);
        }
        if prev {
            self.cur_page = self.cur_page.saturating_sub(1);
        }
        if key(Modifiers::NONE, Key::Home) {
            self.cur_page = 0;
        }
        if key(Modifiers::NONE, Key::End) {
            self.cur_page = last;
        }
    }

    // -- Text, markering ----------------------------------------------------------------------

    pub fn select_all(&mut self) {
        if self.sizes.is_empty() {
            return;
        }
        self.sel = Some(Sel { start: (0, 0), end: (self.sizes.len() - 1, usize::MAX) });
        self.want_all_text = true;
    }

    /// Markerad text (sidor skarvas med radbrytning). Bara sidor vars text hunnit läsas ingår.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.sel?;
        let mut parts = Vec::new();
        for p in sel.start.0..=sel.end.0.min(self.sizes.len().saturating_sub(1)) {
            let Some(t) = self.texts.get(&p) else { continue };
            if let Some(r) = sel_range_on_page(&sel, p, t.glyphs.len()) {
                let s = t.text_of(r);
                if !s.is_empty() {
                    parts.push(s);
                }
            }
        }
        let s = parts.join("\n");
        (!s.is_empty()).then_some(s)
    }

    pub fn copy_selection(&self, ctx: &Context) {
        if let Some(t) = self.selected_text() {
            ctx.copy_text(t);
        }
    }

    /// Markeringssteget under en punkt på en sida.
    fn unit_at(&self, page: usize, pt: Pos2, mode: UnitMode) -> Option<Unit> {
        let t = self.texts.get(&page)?;
        let g = t.glyph_at(pt)?;
        let r = match mode {
            UnitMode::Char => g..g + 1,
            UnitMode::Word => t.word_at(g),
            UnitMode::Line => t.line_at(g),
        };
        (r.start < r.end).then_some(((page, r.start), (page, r.end)))
    }

    /// Sida och punkt (visningspunkter) under en skärmposition; `nearest` ger närmaste sida även
    /// när punkten ligger utanför alla sidor (vid drag).
    fn hit(&self, screen: Pos2, origin: Pos2, nearest: bool) -> Option<(usize, Pos2)> {
        let l = self.layout.as_ref()?;
        let content = Pos2::ZERO + (screen - origin);
        let page = if nearest { l.nearest_page(content) } else { l.page_at(content)? };
        let xf = self.xf(l, page, origin);
        let mut pt = xf.from_screen(screen);
        if nearest {
            let s = self.sizes[page];
            pt = Pos2::new(pt.x.clamp(0.0, s.width), pt.y.clamp(0.0, s.height));
        }
        Some((page, pt))
    }

    fn xf(&self, l: &Layout, page: usize, origin: Pos2) -> PageXf {
        let s = self.sizes[page];
        PageXf { origin: origin + l.rects[page].min.to_vec2(), k: l.zoom * PX, w: s.width, h: s.height, rot: self.rotation }
    }

    // -- Sök ------------------------------------------------------------------------------------

    /// Startar/uppdaterar sökningen. Tom fråga rensar. Träffar samlas i bakgrunden sida för sida.
    pub fn find_set(&mut self, query: &str, case_sensitive: bool, whole_word: bool) {
        if query.is_empty() {
            self.find = None;
            return;
        }
        let n = self.sizes.len();
        self.find = Some(Find {
            query: query.to_owned(),
            case_sensitive,
            whole_word,
            matches: vec![Vec::new(); n],
            searched: vec![false; n],
            done: 0,
            total: 0,
            current: None,
            jump_pending: true,
            scroll_to_current: false,
        });
    }

    /// Alla träffar i dokumentordning.
    fn find_list(&self) -> Vec<(usize, usize)> {
        let Some(f) = &self.find else { return Vec::new() };
        f.matches.iter().enumerate().flat_map(|(p, m)| (0..m.len()).map(move |i| (p, i))).collect()
    }

    pub fn find_next(&mut self, backwards: bool) {
        let list = self.find_list();
        let cur_page = self.cur_page;
        let Some(f) = &mut self.find else { return };
        f.jump_pending = false;
        if list.is_empty() {
            return;
        }
        let idx = match f.current.and_then(|c| list.iter().position(|x| *x == c)) {
            Some(i) if backwards => (i + list.len() - 1) % list.len(),
            Some(i) => (i + 1) % list.len(),
            // Ingen aktuell träff: första efter/före nuvarande sida.
            None if backwards => list.iter().rposition(|x| x.0 <= cur_page).unwrap_or(list.len() - 1),
            None => list.iter().position(|x| x.0 >= cur_page).unwrap_or(0),
        };
        f.current = Some(list[idx]);
        f.scroll_to_current = true;
    }

    pub fn find_status(&self) -> FindStatus {
        let Some(f) = &self.find else { return FindStatus::default() };
        // Träffens nummer räknas fram utan att bygga hela träfflistan (den kan ha hundratusentals poster).
        let current = f.current.and_then(|(p, i)| {
            (p < f.matches.len() && i < f.matches[p].len()).then(|| f.matches[..p].iter().map(Vec::len).sum::<usize>() + i)
        });
        FindStatus { searching: f.done < f.matches.len(), current, total: f.total, done_pages: f.done }
    }

    pub fn find_clear(&mut self) {
        self.find = None;
    }

    /// Söker igenom sidor vars text kommit (högst ett knippe per bild).
    fn step_find(&mut self) {
        let cur_page = self.cur_page;
        let Some(f) = &mut self.find else { return };
        if f.done < f.matches.len() {
            let mut budget = 24;
            for p in 0..f.matches.len() {
                if budget == 0 {
                    break;
                }
                if f.searched[p] {
                    continue;
                }
                if let Some(t) = self.texts.get(&p) {
                    f.matches[p] = t.find(&f.query, f.case_sensitive, f.whole_word);
                    f.total += f.matches[p].len();
                    f.searched[p] = true;
                    f.done += 1;
                    budget -= 1;
                }
            }
        }
        if f.jump_pending && f.current.is_none() {
            let all_done = f.done >= f.matches.len();
            let first_after = (cur_page..f.matches.len()).find(|&p| !f.matches[p].is_empty());
            let pick = first_after.or_else(|| if all_done { f.matches.iter().position(|m| !m.is_empty()) } else { None });
            if let Some(p) = pick {
                f.current = Some((p, 0));
                f.scroll_to_current = true;
                f.jump_pending = false;
            } else if all_done {
                f.jump_pending = false;
            }
        }
    }

    // -- Miniatyrer -------------------------------------------------------------------------------

    /// Bild av en sida för miniatyrer. Begär rendering i liten skala; `None` tills den finns.
    pub fn thumbnail(&mut self, ctx: &Context, page: usize, width_px: f32) -> Option<TextureHandle> {
        if page >= self.sizes.len() {
            return None;
        }
        self.pump_events(ctx);
        let (w, _) = rotated_size(self.sizes[page], self.rotation);
        let scale = (width_px / w.max(1.0)).max(0.01);
        self.thumb_wants.insert(page, (scale, self.frame));
        self.thumbs.get(&page).map(|t| t.handle.clone())
    }

    // -- Motorhändelser och cache ---------------------------------------------------------------

    fn pump_events(&mut self, ctx: &Context) {
        for ev in self.engine.poll() {
            match ev {
                EngineEvent::Rendered { req, image } => self.on_rendered(ctx, req, image),
                EngineEvent::Text { page, text } => {
                    if !self.ocr_pages.contains(&page) {
                        self.texts.insert(page, text);
                    }
                }
                EngineEvent::Failed { page, error } => {
                    if super::engine::is_text_error(&error) {
                        // Sidan saknar användbar text; ett tomt lager hindrar nya försök.
                        self.texts.insert(page, Arc::new(PageText::default()));
                    } else {
                        self.failed.insert(page, error);
                    }
                }
            }
        }
    }

    fn on_rendered(&mut self, ctx: &Context, req: RenderRequest, image: egui::ColorImage) {
        if req.rotation != self.rotation || req.page >= self.sizes.len() {
            return;
        }
        let page = req.page;
        let bytes = image.size[0] * image.size[1] * 4;
        let want = self.wanted_scale(page);
        let is_thumb = self.thumb_wants.get(&page).is_some_and(|(s, _)| {
            req.recolor == Recolor::None && close(req.scale, effective_scale(self.sizes[page], self.rotation, *s))
        });
        let is_page = !is_thumb || close(req.scale, want);
        if is_thumb {
            let handle = ctx.load_texture(format!("pdf-thumb-{page}"), image.clone(), TextureOptions::LINEAR);
            self.thumbs.insert(page, Thumb { handle, scale: req.scale, rotation: req.rotation });
            self.evict_thumbs();
        }
        if is_page && req.recolor == self.recolor {
            let handle = ctx.load_texture(format!("pdf-page-{page}"), image, TextureOptions::LINEAR);
            if let Some(old) = self.tex.insert(
                page,
                Tex { handle, scale: req.scale, rotation: req.rotation, recolor: req.recolor, bytes, used: self.frame },
            ) {
                self.tex_bytes = self.tex_bytes.saturating_sub(old.bytes);
            }
            self.tex_bytes += bytes;
            self.waiting.remove(&page);
            self.failed.remove(&page);
        }
    }

    fn evict_thumbs(&mut self) {
        if self.thumbs.len() <= MAX_THUMBS {
            return;
        }
        let cur = self.cur_page as i64;
        let mut keys: Vec<usize> = self.thumbs.keys().copied().collect();
        keys.sort_by_key(|p| -(*p as i64 - cur).abs());
        for p in keys.into_iter().take(self.thumbs.len() - MAX_THUMBS) {
            self.thumbs.remove(&p);
        }
    }

    /// Tar bort bilder långt från vyn när cachen blir för stor.
    /// Släpper textlager långt från vyn när det blivit för många (1000+ sidor), så att minnet inte
    /// växer med dokumentets storlek. Allt behålls vid "markera allt".
    fn evict_texts(&mut self) {
        if self.texts.len() <= MAX_TEXT_PAGES || self.want_all_text {
            return;
        }
        let cur = self.cur_page as i64;
        let keep = |p: &usize| {
            self.visible.contains(p)
                || self.ocr_pages.contains(p)
                || (*p as i64 - cur).abs() <= 3
                || self.find.as_ref().is_some_and(|f| f.current.is_some_and(|c| c.0 == *p) || f.searched.get(*p) == Some(&false))
        };
        let mut far: Vec<usize> = self.texts.keys().copied().filter(|p| !keep(p)).collect();
        far.sort_by_key(|p| -(*p as i64 - cur).abs());
        let drop = self.texts.len() - MAX_TEXT_PAGES * 3 / 4;
        for p in far.into_iter().take(drop) {
            self.texts.remove(&p);
        }
    }

    fn evict_textures(&mut self) {
        if self.tex_bytes <= CACHE_LIMIT {
            return;
        }
        let cur = self.cur_page as i64;
        let mut keys: Vec<usize> = self.tex.keys().copied().filter(|p| !self.visible.contains(p)).collect();
        keys.sort_by_key(|p| -(*p as i64 - cur).abs());
        for p in keys {
            if self.tex_bytes <= CACHE_LIMIT * 3 / 4 {
                break;
            }
            if let Some(t) = self.tex.remove(&p) {
                self.tex_bytes = self.tex_bytes.saturating_sub(t.bytes);
            }
        }
    }

    /// Skalan (bildpunkter per pt) en sida bör renderas i, begränsad som motorn gör.
    fn wanted_scale(&self, page: usize) -> f32 {
        effective_scale(self.sizes[page], self.rotation, self.k * self.ppp())
    }

    fn ppp(&self) -> f32 {
        self.ppp_cache
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UnitMode {
    Char,
    Word,
    Line,
}

/// Blanksteg som hålls nere.
#[derive(Clone, Copy, Debug)]
struct SpaceHold {
    /// Trycktes med Skift (bläddra bakåt).
    back: bool,
    dragged: bool,
}

/// Antal tryck i följd: ett nytt tryck nära förra i tid och rum räknas vidare, annars börjar det om på 1.
fn next_press_count(prev: Option<(f64, Pos2, u8)>, now: f64, pos: Pos2) -> u8 {
    match prev {
        Some((t, p, n)) if now - t <= 0.5 && p.distance(pos) <= 6.0 => (n % 3) + 1,
        _ => 1,
    }
}

fn close(a: f32, b: f32) -> bool {
    (a / b - 1.0).abs() < 0.02
}

fn norm_rot(r: u16) -> u16 {
    r % 360 / 90 * 90
}

/// Översta sidan vid rullning `y` och andel ner i den.
fn top_of(l: &Layout, y: f32) -> (usize, f32) {
    if l.rects.is_empty() {
        return (0, 0.0);
    }
    let ri = l.rows.partition_point(|r| r.bottom <= y).min(l.rows.len() - 1);
    let page = l.rows[ri].pages.start;
    let r = l.rects[page];
    (page, ((y - r.top()) / r.height().max(1.0)).clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------------------------
// Ritning
// ---------------------------------------------------------------------------------------------

impl PdfView {
    /// Ritar sidorna i hela `ui`-ytan och hanterar mus. Tangenter tas av `handle_keys`.
    pub fn show(&mut self, ui: &mut Ui, palette: &Palette, dark: bool) {
        let ctx = ui.ctx().clone();
        self.frame += 1;
        self.ppp_cache = ctx.pixels_per_point();
        self.pump_events(&ctx);
        self.step_find();
        if self.sizes.is_empty() {
            return;
        }
        if self.presenting {
            self.show_presenting(ui, &ctx, palette);
        } else {
            self.show_scroll(ui, &ctx, palette, dark);
        }
        let sizes = &self.sizes;
        self.annot.show_editor(&ctx, &|p| sizes.get(p).map_or((0.0, 0.0), |s| (s.width, s.height)));
        let new_len = self.annot.pending().len();
        self.annot.ex.show_popover(&ctx, new_len);
        if self.annot.ex.take_stale() {
            self.rerender_annots(&ctx);
        }
        self.after_frame(&ctx);
    }

    fn show_scroll(&mut self, ui: &mut Ui, ctx: &Context, pal: &Palette, dark: bool) {
        let avail = ui.available_rect_before_wrap();
        let desk = if dark { Color32::from_rgb(0x2A, 0x2A, 0x2E) } else { Color32::from_rgb(0xD4, 0xD4, 0xD7) };
        ui.painter().rect_filled(avail, 0.0, desk);
        let vp_size = vec2((avail.width() - ui.spacing().scroll.allocated_width()).max(50.0), avail.height().max(50.0));

        // Ctrl+hjul och nypning zoomar kring pekaren (16).
        let zd = ctx.input(|i| i.zoom_delta());
        if (zd - 1.0).abs() > 1e-4 {
            if let Some(p) = ctx.pointer_hover_pos().filter(|p| avail.contains(*p)) {
                self.set_zoom(self.zoom * zd);
                self.zoom_anchor = Some(p - avail.min);
            }
        }
        // Lägena som följer fönstret räknas om varje bild (17).
        if let Some(z) = fit_zoom(self.zoom_mode, vp_size, &self.sizes, self.rotation, self.spread) {
            self.zoom = z;
        }
        self.zoom = self.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.k = self.zoom * PX;

        let old = self.layout.clone();
        let rebuild = old.as_ref().is_none_or(|l| {
            l.zoom != self.zoom || l.rotation != self.rotation || l.spread != self.spread || (l.vp_w - vp_size.x).abs() > 0.5
        });
        if rebuild {
            self.layout = Some(Arc::new(build_layout(&self.sizes, self.rotation, self.spread, self.zoom, vp_size.x)));
        }
        let layout = self.layout.clone().expect("layout");
        let zoom_changed = old.as_ref().is_some_and(|l| l.zoom != layout.zoom);
        if zoom_changed {
            self.last_zoom_change = Instant::now();
        }

        // Var ska vyn stå? Hopp först, sedan zoomankare, sedan sökträff.
        let mut want = self.want_offset.take();
        let mut navigated = false;
        if let Some(nav) = self.pending_nav.take() {
            want = Some(self.nav_offset(&layout, nav, vp_size));
            navigated = matches!(nav, Nav::At { .. });
        } else if zoom_changed {
            if let Some(old) = &old {
                let vp_pos = self.zoom_anchor.unwrap_or(vp_size / 2.0);
                if let Some(a) = anchor_at(old, (self.scroll + vp_pos).to_pos2()) {
                    want = Some(offset_for(&layout, &a, vp_pos));
                }
            }
        }
        self.zoom_anchor = None;
        if let Some(o) = self.find_offset(&layout, vp_size) {
            want = Some(o);
            navigated = false;
        }
        if let Some(o) = want.as_mut() {
            *o = clamp_offset(*o, layout.size, vp_size);
        }
        let scroll_delta = std::mem::take(&mut self.pending_scroll);

        let mut area = ScrollArea::both()
            .id_salt("pdf-scroll")
            .auto_shrink([false, false])
            .content_margin(0.0)
            .scroll_source(ScrollSource { drag: DragScroll::Never, ..ScrollSource::default() });
        if let Some(o) = want {
            area = area.scroll_offset(o);
        }
        let out = area.show_viewport(ui, |ui, viewport| {
            if scroll_delta != Vec2::ZERO {
                ui.scroll_with_delta(-scroll_delta);
            }
            self.draw_pages(ui, ctx, viewport, &layout, pal, dark);
        });
        self.scroll = out.state.clamped_offset();
        self.vp = out.inner_rect.size();
        if navigated {
            self.nav_lock = want.map(|o| o.y);
        }
        match self.nav_lock {
            Some(y) if (self.scroll.y - y).abs() < 2.0 => {}
            _ => {
                self.nav_lock = None;
                self.cur_page = most_visible(&layout, Rect::from_min_size(self.scroll.to_pos2(), self.vp));
            }
        }
    }

    fn nav_offset(&self, layout: &Layout, nav: Nav, vp: Vec2) -> Vec2 {
        let y = match nav {
            Nav::At { page, y } => {
                let r = layout.rects[page.min(layout.rects.len() - 1)];
                r.top() + y * layout.zoom * PX - if y <= 0.0 { 6.0 } else { 10.0 }
            }
            Nav::Frac { page, frac } => {
                let r = layout.rects[page.min(layout.rects.len() - 1)];
                // Överst på sidan: visa lite av mellanrummet ovanför, som vid hopp.
                r.top() + frac * r.height() - if frac <= 0.0 { 6.0 } else { 0.0 }
            }
        };
        clamp_offset(vec2(self.scroll.x, y), layout.size, vp)
    }

    /// Rullar sökträffen i bild om den inte redan syns.
    fn find_offset(&mut self, layout: &Layout, vp: Vec2) -> Option<Vec2> {
        let f = self.find.as_mut()?;
        if !f.scroll_to_current {
            return None;
        }
        let (p, i) = f.current?;
        let t = self.texts.get(&p)?;
        f.scroll_to_current = false;
        let first = *t.rects(f.matches[p][i].clone()).first()?;
        let s = self.sizes[p];
        let xf = PageXf { origin: Pos2::ZERO + layout.rects[p].min.to_vec2(), k: layout.zoom * PX, w: s.width, h: s.height, rot: self.rotation };
        let r = xf.rect_to_screen(first);
        let view = Rect::from_min_size(self.scroll.to_pos2(), vp);
        if view.shrink2(vec2(20.0, 50.0)).contains_rect(r) {
            return None;
        }
        let x = if r.left() < view.left() || r.right() > view.right() { r.center().x - vp.x / 2.0 } else { self.scroll.x };
        Some(vec2(x, r.center().y - vp.y / 3.0))
    }

    fn draw_pages(&mut self, ui: &mut Ui, ctx: &Context, viewport: Rect, layout: &Layout, pal: &Palette, dark: bool) {
        let (rect, resp) = ui.allocate_exact_size(layout.size, Sense::click_and_drag());
        let origin = rect.min;
        let ppp = ctx.pixels_per_point();
        self.visible.clear();
        for ri in layout.rows_in(viewport.min.y, viewport.max.y) {
            for p in layout.rows[ri].pages.clone() {
                if !layout.rects[p].intersects(viewport) {
                    continue;
                }
                self.visible.push(p);
                let r = layout.rects[p].translate(origin.to_vec2()).round_to_pixels(ppp);
                self.paint_page(ui, ctx, p, r, pal, dark, true);
                let xf = self.xf(layout, p, origin);
                self.paint_overlays(ui, p, &xf);
                self.forms.page_ui(ui, p, &xf);
            }
        }
        self.interact(ui, ctx, &resp, origin);
    }

    fn paint_page(&mut self, ui: &Ui, ctx: &Context, p: usize, rect: Rect, pal: &Palette, dark: bool, shadow: bool) {
        let painter = ui.painter();
        if shadow {
            let color = Color32::from_black_alpha(if dark { 130 } else { 60 });
            painter.add(Shadow { offset: [0, 2], blur: 12, spread: 0, color }.as_shape(rect, 0.0));
        }
        let base = match self.recolor {
            Recolor::None => Color32::WHITE,
            Recolor::Invert => Color32::from_rgb(0x1C, 0x1C, 0x1E),
            Recolor::Sepia => Color32::from_rgb(0xF4, 0xEC, 0xD8),
        };
        painter.rect_filled(rect, 0.0, base);
        if let Some(t) = self.tex.get_mut(&p) {
            t.used = self.frame;
            painter.image(t.handle.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        } else if let Some(err) = self.failed.get(&p) {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("{}\n{err}", t("Sidan kunde inte visas")),
                FontId::proportional(14.0),
                pal.error,
            );
        } else {
            let since = *self.waiting.entry(p).or_insert_with(Instant::now);
            if since.elapsed() >= SPINNER_AFTER {
                let sz = 28.0f32.min(rect.width()).min(rect.height());
                Spinner::new().size(sz).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(sz)));
            } else {
                ctx.request_repaint_after(SPINNER_AFTER - since.elapsed());
            }
        }
    }

    /// Sökträffar (gult, aktuell orange) och markering (blått) ovanpå sidan.
    fn paint_overlays(&self, ui: &Ui, p: usize, xf: &PageXf) {
        self.annot.paint(ui.painter(), p, xf);
        self.annot.ex.paint(ui.painter(), p, xf);
        let Some(t) = self.texts.get(&p) else { return };
        let painter = ui.painter();
        if let Some(f) = &self.find {
            for (i, r) in f.matches[p].iter().enumerate() {
                let col = if f.current == Some((p, i)) {
                    Color32::from_rgba_unmultiplied(255, 140, 0, 150)
                } else {
                    Color32::from_rgba_unmultiplied(255, 221, 0, 105)
                };
                for rr in t.rects(r.clone()) {
                    painter.rect_filled(xf.rect_to_screen(rr), 1.0, col);
                }
            }
        }
        if let Some(sel) = &self.sel {
            if let Some(r) = sel_range_on_page(sel, p, t.glyphs.len()) {
                for rr in t.rects(r) {
                    painter.rect_filled(xf.rect_to_screen(rr), 1.0, Color32::from_rgba_unmultiplied(50, 110, 230, 90));
                }
            }
        }
    }

    fn link_at(&self, screen: Pos2, origin: Pos2) -> Option<LinkTarget> {
        let (page, pt) = self.hit(screen, origin, false)?;
        self.meta.links.get(page)?.iter().find(|l| l.rect.contains(pt)).map(|l| l.target.clone())
    }

    fn interact(&mut self, ui: &Ui, ctx: &Context, resp: &egui::Response, origin: Pos2) {
        // Panorera: handverktyget eller mittenknappen (26).
        let middle = resp.dragged_by(PointerButton::Middle);
        let space_pan = self.space.is_some() && resp.dragged_by(PointerButton::Primary);
        if space_pan {
            if let Some(hold) = &mut self.space {
                hold.dragged = true;
            }
        }
        let hand = (self.tool == Tool::Hand || space_pan) && resp.dragged_by(PointerButton::Primary);
        if middle || hand {
            ui.scroll_with_delta_animation(resp.drag_delta(), ScrollAnimation::none());
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            return;
        }

        // Anteckningsverktygen (lapp, textruta, penna) tar hand om musen själva.
        if self.annot.tool.is_some() && self.annot_pointer(ctx, resp, origin) {
            return;
        }

        // Befintliga anteckningar: välj med klick i Anteckna-läget, verktygstips med författare och text.
        if self.annot_existing(ctx, resp, origin) {
            return;
        }

        // Länkar (35–36): handpekare och verktygstips.
        let mut over_link = false;
        if let Some(h) = resp.hover_pos() {
            if let Some(target) = self.link_at(h, origin) {
                over_link = true;
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                let tip = match &target {
                    LinkTarget::Uri(u) => u.clone(),
                    LinkTarget::Page(d) => tf!("Gå till sidan {n}", n = d.page + 1),
                };
                resp.clone().on_hover_text(tip);
            } else if self.tool == Tool::Hand || self.space.is_some() {
                ctx.set_cursor_icon(CursorIcon::Grab);
            } else if let Some((page, pt)) = self.hit(h, origin, false) {
                let on_text = self
                    .texts
                    .get(&page)
                    .is_some_and(|t| t.glyph_at(pt).is_some_and(|g| t.glyphs[g].rect.expand(2.0).contains(pt)));
                if on_text {
                    ctx.set_cursor_icon(CursorIcon::Text);
                }
            }
        }
        if resp.clicked_by(PointerButton::Primary) && over_link {
            if let Some(pos) = resp.interact_pointer_pos() {
                match self.link_at(pos, origin) {
                    Some(LinkTarget::Page(d)) => self.go_to(&d),
                    Some(LinkTarget::Uri(u)) => self.pending_uri = Some(u),
                    None => {}
                }
            }
            return;
        }

        // Markering (30): dra, dubbelklick = ord, trippelklick = rad.
        if self.tool != Tool::Select {
            return;
        }
        let click_unit = |this: &Self, mode| {
            let pos = resp.interact_pointer_pos()?;
            let (page, pt) = this.hit(pos, origin, false)?;
            this.unit_at(page, pt, mode)
        };
        if resp.triple_clicked_by(PointerButton::Primary) {
            if let Some((a, b)) = click_unit(self, UnitMode::Line) {
                self.sel = Some(Sel { start: a, end: b });
            }
        } else if resp.double_clicked_by(PointerButton::Primary) {
            if let Some((a, b)) = click_unit(self, UnitMode::Word) {
                self.sel = Some(Sel { start: a, end: b });
            }
        } else if resp.clicked_by(PointerButton::Primary) {
            self.sel = None;
        }
        // Antal tryck i följd, räknat vid trycket så att ett drag efter dubbel-/trippelklick känns igen.
        if resp.is_pointer_button_down_on() && ctx.input(|i| i.pointer.primary_pressed()) {
            if let Some(pos) = ctx.input(|i| i.pointer.press_origin()) {
                let now = ctx.input(|i| i.time);
                let n = next_press_count(self.press_hist, now, pos);
                self.press_hist = Some((now, pos, n));
            }
        }
        if resp.drag_started_by(PointerButton::Primary) {
            self.drag_mode = match self.press_hist.map_or(1, |h| h.2) {
                2 => UnitMode::Word,
                n if n >= 3 => UnitMode::Line,
                _ => UnitMode::Char,
            };
            self.drag_anchor = ctx
                .input(|i| i.pointer.press_origin())
                .and_then(|o| self.hit(o, origin, false))
                .and_then(|(page, pt)| self.unit_at(page, pt, self.drag_mode));
            self.sel = self.drag_anchor.filter(|_| self.drag_mode != UnitMode::Char).map(|(a, b)| Sel { start: a, end: b });
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let (Some(anchor), Some(pos)) = (self.drag_anchor, ctx.pointer_latest_pos()) {
                if let Some((page, pt)) = self.hit(pos, origin, true) {
                    if let Some(head) = self.unit_at(page, pt, self.drag_mode) {
                        self.sel = Some(union_units(anchor, head));
                    }
                }
                // Rulla när pekaren dras utanför vyn.
                let clip = ui.clip_rect();
                let d = if pos.y < clip.top() {
                    (clip.top() - pos.y) * 0.3
                } else if pos.y > clip.bottom() {
                    -(pos.y - clip.bottom()) * 0.3
                } else {
                    0.0
                };
                if d != 0.0 {
                    ui.scroll_with_delta_animation(vec2(0.0, d.clamp(-40.0, 40.0)), ScrollAnimation::none());
                    ctx.request_repaint();
                }
            }
        }
        if resp.drag_stopped() {
            self.drag_anchor = None;
            self.drag_mode = UnitMode::Char;
        }
        self.annot_from_selection(resp);
    }

    /// Befintliga anteckningar under pekaren (annot_edit.rs). Sant när ett klick valde en anteckning.
    fn annot_existing(&mut self, ctx: &Context, resp: &egui::Response, origin: Pos2) -> bool {
        let at = |this: &Self, s: Pos2| this.hit(s, origin, false).and_then(|(p, pt)| this.annot.ex.hit(p, pt));
        if let Some(h) = resp.hover_pos() {
            if let Some(id) = at(self, h).filter(|_| self.link_at(h, origin).is_none()) {
                if self.annot.ex.is_active() {
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }
                if self.annot.ex.selected != Some(id) {
                    if let Some(tip) = self.annot.ex.tooltip(id) {
                        resp.clone().on_hover_ui(|ui| super::annot_edit::show_tooltip(ui, &tip));
                    }
                }
            }
        }
        if resp.clicked_by(PointerButton::Primary) {
            match resp.interact_pointer_pos().and_then(|p| at(self, p)) {
                Some(id) if self.annot.ex.is_active() => {
                    self.annot.ex.select(id);
                    return true;
                }
                _ => self.annot.ex.deselect(),
            }
        }
        false
    }

    /// Ritar om sidorna med väntande ändringar av befintliga anteckningar (borttagna syns inte längre).
    /// Visarens `bytes` förblir originalet; bara motorn får de ändrade.
    fn rerender_annots(&mut self, ctx: &Context) {
        let edits = self.annot.ex.edits().clone();
        let pw = self.annot.ex.password().map(str::to_owned);
        let bytes = if edits.is_empty() {
            self.bytes.clone()
        } else {
            match super::annot::apply_all(&self.bytes, pw.as_deref(), &[], &edits) {
                Ok(b) => Arc::new(b),
                Err(_) => return,
            }
        };
        let Ok(engine) = PdfEngine::open(bytes, pw.as_deref(), ctx.clone()) else { return };
        let (meta, hist, orig) = (self.meta.clone(), std::mem::take(&mut self.history), self.bytes.clone());
        self.reload(engine, meta, orig);
        self.history = hist;
    }

    fn annot_pointer(&mut self, ctx: &Context, resp: &egui::Response, origin: Pos2) -> bool {
        let mut a = std::mem::take(&mut self.annot);
        let hit = |s: Pos2, nearest: bool| self.hit(s, origin, nearest);
        let size = |p: usize| (self.sizes[p].width, self.sizes[p].height);
        let took = a.pointer(ctx, resp, &hit, &size);
        self.annot = a;
        took
    }

    /// Markera/understruken/genomstruken: musen släpps efter en markering → ny anteckning på texten.
    fn annot_from_selection(&mut self, resp: &egui::Response) {
        let Some(tool) = self.annot.tool.filter(|t| t.is_markup()) else { return };
        let done = resp.drag_stopped_by(PointerButton::Primary) || resp.double_clicked() || resp.triple_clicked();
        let Some(sel) = self.sel.filter(|_| done) else { return };
        let mut per_page = Vec::new();
        for p in sel.start.0..=sel.end.0.min(self.sizes.len().saturating_sub(1)) {
            let Some(t) = self.texts.get(&p) else { continue };
            if let Some(r) = sel_range_on_page(&sel, p, t.glyphs.len()) {
                per_page.push((p, t.rects(r)));
            }
        }
        self.annot.add_markup(tool, per_page);
        self.sel = None;
    }

    /// Presentationsläge (43): svart bakgrund, en sida anpassad till skärmen.
    fn show_presenting(&mut self, ui: &mut Ui, ctx: &Context, pal: &Palette) {
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        let last = self.sizes.len() - 1;
        self.cur_page = self.cur_page.min(last);
        let p = self.cur_page;
        let (w, h) = rotated_size(self.sizes[p], self.rotation);
        let k = ((rect.width() - 8.0) / w).min((rect.height() - 8.0) / h).max(0.01);
        self.k = k;
        self.visible = vec![p];
        let page_rect = Rect::from_center_size(rect.center(), vec2(w * k, h * k)).round_to_pixels(ctx.pixels_per_point());
        self.paint_page(ui, ctx, p, page_rect, pal, true, false);
        let resp = ui.allocate_rect(rect, Sense::click());
        if resp.clicked_by(PointerButton::Primary) {
            self.cur_page = (p + 1).min(last);
        } else if resp.clicked_by(PointerButton::Secondary) {
            self.cur_page = p.saturating_sub(1);
        }
    }

    /// Begär rendering och text för det som syns, grannarna och miniatyrerna; städar cachen.
    fn after_frame(&mut self, ctx: &Context) {
        let n = self.sizes.len();
        let cur = self.cur_page as i64;
        let mut vis = self.visible.clone();
        vis.sort_by_key(|p| (*p as i64 - cur).abs());
        let mut near = Vec::new();
        if let (Some(&lo), Some(&hi)) = (self.visible.iter().min(), self.visible.iter().max()) {
            let reach = if self.presenting { 1 } else { 2 };
            for d in 1..=reach {
                if hi + d < n {
                    near.push(hi + d);
                }
                if lo >= d {
                    near.push(lo - d);
                }
            }
        }
        let settling = self.last_zoom_change.elapsed() < ZOOM_SETTLE;
        let mut reqs = Vec::new();
        let mut waiting = false;
        for &p in vis.iter().chain(near.iter()) {
            if self.failed.contains_key(&p) {
                continue;
            }
            let want = self.wanted_scale(p);
            let fresh = self.tex.get(&p).is_some_and(|t| t.rotation == self.rotation && t.recolor == self.recolor && close(t.scale, want));
            if fresh {
                continue;
            }
            if settling && self.tex.contains_key(&p) {
                ctx.request_repaint_after(ZOOM_SETTLE);
                continue;
            }
            waiting |= self.visible.contains(&p);
            reqs.push(RenderRequest { page: p, scale: self.k * self.ppp(), rotation: self.rotation, recolor: self.recolor });
        }
        // Miniatyrer efter sidorna; de som inte efterfrågats på ett tag glöms.
        let frame = self.frame;
        self.thumb_wants.retain(|_, (_, f)| frame.saturating_sub(*f) <= 3);
        let mut wants: Vec<(usize, f32)> = self.thumb_wants.iter().map(|(p, (s, _))| (*p, *s)).collect();
        wants.sort_by_key(|(p, _)| (*p as i64 - cur).abs());
        for (p, scale) in wants.into_iter().take(48) {
            let eff = effective_scale(self.sizes[p], self.rotation, scale);
            let ok = self.thumbs.get(&p).is_some_and(|t| t.rotation == self.rotation && (t.scale / eff - 1.0).abs() < 0.1);
            if !ok {
                reqs.push(RenderRequest { page: p, scale, rotation: self.rotation, recolor: Recolor::None });
            }
        }
        self.engine.request(reqs);
        if waiting {
            ctx.request_repaint_after(Duration::from_millis(60));
        }

        // Text: synliga sidor och grannar för markering, sedan alla vid sökning/markera allt.
        let mut tw: Vec<usize> = vis.iter().chain(near.iter()).copied().filter(|p| !self.texts.contains_key(p)).collect();
        if self.find.is_some() || self.want_all_text {
            // Redan sökta sidor hämtas inte om (deras text kan ha släppts), bara vid "markera allt".
            let searched = |p: &usize| !self.want_all_text && self.find.as_ref().is_some_and(|f| f.searched.get(*p) == Some(&true));
            let mut rest: Vec<usize> =
                (0..n).filter(|p| !self.texts.contains_key(p) && !tw.contains(p) && !searched(p)).collect();
            rest.sort_by_key(|p| (*p as i64 - cur).abs());
            tw.extend(rest.into_iter().take(32));
        }
        // Sidan med den aktuella träffen behöver sin text för att kunna rullas fram och markeras.
        if let Some((p, _)) = self.find.as_ref().and_then(|f| f.current) {
            if !self.texts.contains_key(&p) && !tw.contains(&p) {
                tw.insert(0, p);
            }
        }
        if !tw.is_empty() {
            self.engine.request_text(tw);
        }
        self.evict_texts();

        let visible = self.visible.clone();
        self.waiting.retain(|p, _| visible.contains(p));
        self.evict_textures();
    }
}

fn clamp_offset(o: Vec2, content: Vec2, vp: Vec2) -> Vec2 {
    vec2(o.x.clamp(0.0, (content.x - vp.x).max(0.0)), o.y.clamp(0.0, (content.y - vp.y).max(0.0)))
}

/// Sidan med störst synlig yta (vid lika: den första).
fn most_visible(l: &Layout, view: Rect) -> usize {
    let mut best = (0usize, -1.0f32);
    for ri in l.rows_in(view.min.y, view.max.y) {
        for p in l.rows[ri].pages.clone() {
            let i = l.rects[p].intersect(view);
            let a = if i.is_positive() { i.width() * i.height() } else { 0.0 };
            if a > best.1 + 0.5 {
                best = (p, a);
            }
        }
    }
    if best.1 < 0.0 { l.nearest_page(view.center()) } else { best.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use std::time::Duration;

    const A4: PageSize = PageSize { width: 595.0, height: 842.0 };
    const LANDSCAPE: PageSize = PageSize { width: 842.0, height: 595.0 };

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn auto_stående_sida_passar_bredden_högst_125_procent() {
        let sizes = [A4; 3];
        let z = fit_zoom(ZoomMode::Auto, vec2(1000.0, 800.0), &sizes, 0, Spread::None).unwrap();
        assert!(approx(z, (1000.0 - 2.0 * MARGIN) / (595.0 * PX)), "{z}");
        let z = fit_zoom(ZoomMode::Auto, vec2(3000.0, 800.0), &sizes, 0, Spread::None).unwrap();
        assert!(approx(z, 1.25), "{z}");
    }

    #[test]
    fn auto_liggande_sida_passar_hela_sidan() {
        let sizes = [LANDSCAPE];
        let z = fit_zoom(ZoomMode::Auto, vec2(1600.0, 700.0), &sizes, 0, Spread::None).unwrap();
        assert!(approx(z, (700.0 - 2.0 * MARGIN) / (595.0 * PX)), "{z}");
    }

    #[test]
    fn sidbredd_sidan_och_övriga_lägen() {
        let sizes = [A4];
        let vp = vec2(1000.0, 800.0);
        let w = fit_zoom(ZoomMode::PageWidth, vp, &sizes, 0, Spread::None).unwrap();
        let f = fit_zoom(ZoomMode::PageFit, vp, &sizes, 0, Spread::None).unwrap();
        assert!(approx(w, (1000.0 - 2.0 * MARGIN) / (595.0 * PX)));
        assert!(approx(f, (800.0 - 2.0 * MARGIN) / (842.0 * PX)));
        assert!(f < w);
        assert_eq!(fit_zoom(ZoomMode::ActualSize, vp, &sizes, 0, Spread::None), Some(1.0));
        assert_eq!(fit_zoom(ZoomMode::Custom, vp, &sizes, 0, Spread::None), None);
        // Rotation byter bredd och höjd.
        let w90 = fit_zoom(ZoomMode::PageWidth, vp, &sizes, 90, Spread::None).unwrap();
        assert!(approx(w90, (1000.0 - 2.0 * MARGIN) / (842.0 * PX)));
    }

    #[test]
    fn sidbredd_med_uppslag_räknar_två_sidor() {
        let sizes = [A4; 4];
        let z = fit_zoom(ZoomMode::PageWidth, vec2(1400.0, 800.0), &sizes, 0, Spread::Odd).unwrap();
        assert!(approx(z, (1400.0 - 2.0 * MARGIN - GAP) / (2.0 * 595.0 * PX)), "{z}");
    }

    #[test]
    fn zoom_hålls_inom_gränserna() {
        let z = fit_zoom(ZoomMode::PageWidth, vec2(1.0e6, 800.0), &[A4], 0, Spread::None).unwrap();
        assert_eq!(z, MAX_ZOOM);
        let z = fit_zoom(ZoomMode::PageWidth, vec2(30.0, 800.0), &[PageSize { width: 1.0e5, height: 1.0e5 }], 0, Spread::None).unwrap();
        assert_eq!(z, MIN_ZOOM);
    }

    #[test]
    fn uppslag_sida_till_rad_och_kolumn() {
        let odd: Vec<_> = (0..5).map(|p| page_cell(p, Spread::Odd)).collect();
        assert_eq!(odd, [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0)]);
        let even: Vec<_> = (0..5).map(|p| page_cell(p, Spread::Even)).collect();
        assert_eq!(even, [(0, 0), (1, 0), (1, 1), (2, 0), (2, 1)]);
        let none: Vec<_> = (0..3).map(|p| page_cell(p, Spread::None)).collect();
        assert_eq!(none, [(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn uppslag_rader() {
        assert_eq!(spread_rows(5, Spread::Odd), vec![0..2, 2..4, 4..5]);
        assert_eq!(spread_rows(5, Spread::Even), vec![0..1, 1..3, 3..5]);
        assert_eq!(spread_rows(3, Spread::None), vec![0..1, 1..2, 2..3]);
        assert!(spread_rows(0, Spread::Even).is_empty());
        // page_cell och spread_rows är konsekventa.
        for spread in [Spread::None, Spread::Odd, Spread::Even] {
            let rows = spread_rows(9, spread);
            for p in 0..9 {
                let (r, c) = page_cell(p, spread);
                assert_eq!(rows[r].start + c, p);
            }
        }
    }

    #[test]
    fn layout_centrerar_sidor_med_mellanrum() {
        let l = build_layout(&[A4, A4], 0, Spread::None, 1.0, 1000.0);
        let w = 595.0 * PX;
        assert_eq!(l.size.x, 1000.0);
        assert!(approx(l.rects[0].center().x, 500.0));
        assert!(approx(l.rects[1].top() - l.rects[0].bottom(), GAP));
        assert!(approx(l.rects[0].width(), w));
        assert!(approx(l.size.y, 2.0 * MARGIN + GAP + 2.0 * 842.0 * PX));
    }

    #[test]
    fn layout_uppslag_ligger_sida_vid_sida() {
        let l = build_layout(&[A4; 3], 0, Spread::Odd, 1.0, 2000.0);
        assert_eq!(l.rows.len(), 2);
        assert!(approx(l.rects[1].left() - l.rects[0].right(), GAP));
        assert!(approx(l.rects[0].top(), l.rects[1].top()));
        assert!(l.rects[2].top() > l.rects[0].bottom());
        // Raden är centrerad som helhet.
        assert!(approx((l.rects[0].left() + l.rects[1].right()) / 2.0, 1000.0));
    }

    #[test]
    fn layout_bredare_än_vyn_ger_vågrät_rullning() {
        let l = build_layout(&[A4], 0, Spread::None, 4.0, 500.0);
        assert!(l.size.x > 500.0);
        assert!(approx(l.rects[0].left(), MARGIN));
    }

    #[test]
    fn layout_sökning_av_sidor() {
        let l = build_layout(&[A4; 3], 0, Spread::None, 1.0, 1000.0);
        let mid = l.rects[1].center();
        assert_eq!(l.page_at(mid), Some(1));
        assert_eq!(l.nearest_page(mid), 1);
        // Mellanrummet och marginalen är ingen sida men har en närmaste.
        let gap = Pos2::new(500.0, (l.rects[0].bottom() + l.rects[1].top()) / 2.0);
        assert_eq!(l.page_at(gap), None);
        assert!(l.nearest_page(Pos2::new(500.0, 1.0e6)) == 2);
        assert!(l.nearest_page(Pos2::new(500.0, -50.0)) == 0);
        let rows = l.rows_in(l.rects[1].top() + 1.0, l.rects[1].bottom() - 1.0);
        assert_eq!(rows, 1..2);
    }

    #[test]
    fn rotation_av_punkter_och_tillbaka() {
        let xf = |rot| PageXf { origin: Pos2::new(100.0, 50.0), k: 2.0, w: 200.0, h: 300.0, rot };
        // Medurs 90°: övre vänstra hörnet hamnar uppe till höger i den roterade sidan (300 × 200).
        let p = xf(90).to_view(Pos2::new(0.0, 0.0));
        assert_eq!((p.x, p.y), (300.0, 0.0));
        let p = xf(180).to_view(Pos2::new(0.0, 0.0));
        assert_eq!((p.x, p.y), (200.0, 300.0));
        let p = xf(270).to_view(Pos2::new(0.0, 0.0));
        assert_eq!((p.x, p.y), (0.0, 200.0));
        for rot in [0, 90, 180, 270] {
            let x = xf(rot);
            let pt = Pos2::new(37.0, 211.0);
            let back = x.from_screen(x.to_screen(pt));
            assert!(approx(back.x, pt.x) && approx(back.y, pt.y), "rot {rot}: {back:?}");
        }
        // rect_to_screen ger en normaliserad ruta.
        let r = xf(90).rect_to_screen(Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(30.0, 40.0)));
        assert!(r.width() > 0.0 && r.height() > 0.0);
    }

    #[test]
    fn historik_framåt_och_bakåt() {
        let mut h = History::default();
        assert!(!h.can_back() && !h.can_forward());
        h.push((0, 0.0), (5, 10.0));
        assert!(h.can_back() && !h.can_forward());
        h.push((5, 12.0), (9, 0.0));
        assert_eq!(h.back((9, 3.0)), Some((5, 12.0)));
        assert_eq!(h.back((5, 14.0)), Some((0, 0.0)));
        assert_eq!(h.back((0, 0.0)), None);
        assert!(h.can_forward());
        assert_eq!(h.forward((0, 1.0)), Some((5, 14.0)));
        assert_eq!(h.forward((5, 14.0)), Some((9, 3.0)));
        assert_eq!(h.forward((9, 3.0)), None);
        // Nytt hopp mitt i historiken stryker det som låg framåt.
        h.back((9, 3.0));
        h.push((5, 14.0), (2, 0.0));
        assert!(!h.can_forward());
        assert_eq!(h.back((2, 0.0)), Some((5, 14.0)));
        assert_eq!(h.back((5, 14.0)), Some((0, 1.0)));
    }

    #[test]
    fn zoom_kring_pekaren_håller_punkten_still() {
        let off = vec2(120.0, 340.0);
        let anchor = vec2(300.0, 200.0);
        let new = zoom_offset(off, anchor, 1.0, 1.5);
        // Innehållspunkten under ankaret före zoom, skalad, ska ligga under ankaret efter zoom.
        let before = off + anchor;
        let after = new + anchor;
        assert!(approx(after.x, before.x * 1.5) && approx(after.y, before.y * 1.5));
        // Ingen zoomändring ger samma rullning.
        assert_eq!(zoom_offset(off, anchor, 1.0, 1.0), off);
    }

    #[test]
    fn zoomankare_i_layouten_håller_sidpunkten_still() {
        let sizes = [A4; 20];
        let old = build_layout(&sizes, 0, Spread::None, 1.0, 900.0);
        let offset = vec2(0.0, 5000.0);
        let vp_pos = vec2(450.0, 300.0);
        let a = anchor_at(&old, (offset + vp_pos).to_pos2()).unwrap();
        let new = build_layout(&sizes, 0, Spread::None, 1.7, 900.0);
        let off2 = offset_for(&new, &a, vp_pos);
        // Samma sida och samma andel under pekaren som förut.
        let b = anchor_at(&new, (off2 + vp_pos).to_pos2()).unwrap();
        assert_eq!(a.page, b.page);
        assert!(approx(a.fx, b.fx) && approx(a.fy, b.fy), "{a:?} {b:?}");
    }

    #[test]
    fn tryck_i_följd_räknas() {
        let p = Pos2::new(10.0, 10.0);
        let one = next_press_count(None, 1.0, p);
        assert_eq!(one, 1);
        let two = next_press_count(Some((1.0, p, one)), 1.3, Pos2::new(12.0, 10.0));
        assert_eq!(two, 2);
        let three = next_press_count(Some((1.3, p, two)), 1.6, p);
        assert_eq!(three, 3);
        // Fjärde trycket börjar om; för långsamt eller för långt bort också.
        assert_eq!(next_press_count(Some((1.6, p, three)), 1.8, p), 1);
        assert_eq!(next_press_count(Some((1.0, p, 1)), 2.0, p), 1);
        assert_eq!(next_press_count(Some((1.0, p, 1)), 1.2, Pos2::new(40.0, 10.0)), 1);
    }

    #[test]
    fn markering_ordnas_över_sidor() {
        let a: Unit = ((2, 5), (2, 6));
        let b: Unit = ((0, 7), (0, 9));
        let fwd = union_units(b, a);
        let rev = union_units(a, b);
        assert_eq!(fwd, rev);
        assert_eq!(fwd, Sel { start: (0, 7), end: (2, 6) });
        // Första sidan från glyf 7 till slutet, mellansidor helt, sista sidan till glyf 6.
        assert_eq!(sel_range_on_page(&fwd, 0, 20), Some(7..20));
        assert_eq!(sel_range_on_page(&fwd, 1, 11), Some(0..11));
        assert_eq!(sel_range_on_page(&fwd, 2, 30), Some(0..6));
        assert_eq!(sel_range_on_page(&fwd, 3, 30), None);
        // En sida: bara intervallet; tom sida ger inget.
        let one = union_units(((1, 3), (1, 4)), ((1, 1), (1, 2)));
        assert_eq!(sel_range_on_page(&one, 1, 10), Some(1..4));
        assert_eq!(sel_range_on_page(&one, 1, 0), None);
        // Ordsteg utvidgas åt båda håll.
        let w = union_units(((0, 4), (0, 9)), ((0, 12), (0, 15)));
        assert_eq!(w, Sel { start: (0, 4), end: (0, 15) });
    }

    // -- Hela visaren mot motorn ----------------------------------------------------------------

    fn make_pdf(pages: usize) -> Arc<Vec<u8>> {
        let mut md = String::from("# Rubrik\n\nÅsa åt äpplen\n\n");
        for i in 0..pages * 45 {
            md.push_str(&format!("Stycke nummer {i} med lite text som fyller raden.\n\n"));
        }
        Arc::new(crate::pdf_write::write(&md, "Test", &|_| None).expect("pdf").bytes)
    }

    fn open_view(ctx: &Context, pages: usize) -> PdfView {
        let bytes = make_pdf(pages);
        let engine = PdfEngine::open(bytes.clone(), None, ctx.clone()).expect("öppna");
        let meta = super::super::meta::read_meta(&bytes, None);
        PdfView::new(engine, meta, bytes, None)
    }

    fn frame(ctx: &Context, view: &mut PdfView) {
        let pal = theme::palette(false);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| view.show(ui, &pal, false));
        out.textures_delta.clear();
    }

    fn run_until(ctx: &Context, view: &mut PdfView, mut done: impl FnMut(&mut PdfView) -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            frame(ctx, view);
            if done(view) {
                return;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        panic!("tidsgräns");
    }

    #[test]
    fn ocr_text_ersatter_textlagret_och_sparas_over_motorn() {
        let ctx = Context::default();
        let mut v = open_view(&ctx, 1);
        let t = Arc::new(PageText::new(vec![super::super::text::Glyph { text: "Å".into(), rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(8.0, 12.0)) }]));
        v.set_ocr_text(0, t);
        v.set_ocr_text(99, Arc::new(PageText::default()));
        v.find_set("å", false, false);
        run_until(&ctx, &mut v, |v| v.find_status().done_pages > 0);
        // Motorns text kommer efter och får inte skriva över.
        for _ in 0..10 {
            frame(&ctx, &mut v);
        }
        assert_eq!(v.texts.get(&0).map(|t| t.text().to_owned()).as_deref(), Some("Å"));
        // Bara sida 0 räknas: testdokumentet kan ha en sida till vars text (med "a", som matchar
        // "å" när diakritiska tecken ignoreras) hinner komma eller inte.
        assert_eq!(v.find.as_ref().map(|f| f.matches[0].len()), Some(1));
        assert!(!v.texts.contains_key(&99));
    }

    #[test]
    fn rökprov_sidor_text_sök_och_navigering() {
        let ctx = Context::default();
        let mut v = open_view(&ctx, 3);
        assert!(v.page_count() >= 3, "sidor: {}", v.page_count());
        run_until(&ctx, &mut v, |v| !v.tex.is_empty());
        // Texturen har rätt rotation/omfärgning och skala nära begärd.
        let t = v.tex.values().next().unwrap();
        assert_eq!((t.rotation, t.recolor), (0, Recolor::None));
        assert_eq!(v.current_page(), 0);
        assert!(v.zoom() > 0.1);

        // Sök: hittar rubriken, första träffen väljs.
        v.find_set("åsa", false, false);
        run_until(&ctx, &mut v, |v| !v.find_status().searching);
        let st = v.find_status();
        assert!(st.total >= 1 && st.done_pages == v.page_count(), "{st:?}");
        assert_eq!(st.current, Some(0));
        v.find_next(false);
        assert_eq!(v.find_status().current, Some(0), "en träff: runt hela dokumentet");
        v.find_set("Stycke nummer", true, false);
        run_until(&ctx, &mut v, |v| !v.find_status().searching);
        assert!(v.find_status().total > 10);
        v.find_next(true);
        v.find_clear();
        assert_eq!(v.find_status(), FindStatus::default());

        // Markera allt och hämta texten.
        v.select_all();
        run_until(&ctx, &mut v, |v| v.selected_text().is_some_and(|t| t.contains("Åsa") && t.contains("Stycke nummer")));

        // Navigering och historik.
        let last = v.page_count() - 1;
        v.go_to(&Dest { page: last, y: Some(100.0) });
        run_until(&ctx, &mut v, |v| v.current_page() == last);
        assert!(v.can_back());
        v.back();
        run_until(&ctx, &mut v, |v| v.current_page() == 0);
        assert!(v.can_forward());
        assert_eq!(v.page_label(1), "2");

        // Zoom, rotation, uppslag och omfärgning ger nya bilder utan att krascha.
        v.set_zoom(1.5);
        v.rotate(true);
        v.set_spread(Spread::Odd);
        v.set_recolor(Recolor::Invert);
        run_until(&ctx, &mut v, |v| v.tex.values().any(|t| t.rotation == 90 && t.recolor == Recolor::Invert));
        assert_eq!(v.memory().rotation, 90);
        assert_eq!(v.memory().spread, Spread::Odd);

        // Miniatyr.
        let ctx2 = ctx.clone();
        run_until(&ctx, &mut v, |v| v.thumbnail(&ctx2, 1, 80.0).is_some());

        // Presentationsläge visar en sida.
        v.set_presenting(true);
        run_until(&ctx, &mut v, |v| v.visible == [v.current_page()] && v.tex.contains_key(&v.current_page()));
        v.set_presenting(false);
        frame(&ctx, &mut v);
    }

    #[test]
    fn minne_och_omladdning_behåller_läget() {
        let ctx = Context::default();
        let mut v = open_view(&ctx, 3);
        v.set_zoom(0.8);
        v.rotate(false);
        v.go_to_page(2);
        run_until(&ctx, &mut v, |v| v.current_page() == 2);
        let m = v.memory();
        assert_eq!((m.page, m.rotation, m.zoom_mode), (2, 270, ZoomMode::Custom));
        assert!(approx(m.zoom, 0.8));
        let json = serde_json::to_string(&m).unwrap();
        let m2: ViewMemory = serde_json::from_str(&json).unwrap();

        // Ny visare från minnet hamnar på samma sida.
        let mut v2 = {
            let bytes = v.bytes().clone();
            let engine = PdfEngine::open(bytes.clone(), None, ctx.clone()).unwrap();
            PdfView::new(engine, super::super::meta::read_meta(&bytes, None), bytes, Some(m2))
        };
        run_until(&ctx, &mut v2, |v| !v.tex.is_empty() && v.current_page() == 2);
        assert_eq!(v2.rotation(), 270);

        // Omladdning: samma sida och zoom, texturer börjar om.
        let bytes = v2.bytes().clone();
        let engine = PdfEngine::open(bytes.clone(), None, ctx.clone()).unwrap();
        let meta = super::super::meta::read_meta(&bytes, None);
        v2.reload(engine, meta, bytes);
        assert!(v2.tex.is_empty() && v2.texts.is_empty());
        run_until(&ctx, &mut v2, |v| !v.tex.is_empty());
        assert_eq!(v2.current_page(), 2);
        assert!(approx(v2.zoom(), 0.8));
    }

    #[test]
    fn länkmål_uri_sparas_för_bekräftelse() {
        let ctx = Context::default();
        let mut v = open_view(&ctx, 1);
        assert_eq!(v.take_pending_uri(), None);
        v.pending_uri = Some("https://example.org".into());
        assert_eq!(v.take_pending_uri().as_deref(), Some("https://example.org"));
        assert_eq!(v.take_pending_uri(), None);
    }

    #[test]
    fn zoomknappar_och_gränser() {
        let ctx = Context::default();
        let mut v = open_view(&ctx, 1);
        v.set_zoom(1.0);
        v.zoom_in();
        assert!(approx(v.zoom(), 1.1) && v.zoom_mode() == ZoomMode::Custom);
        v.zoom_out();
        v.zoom_out();
        assert!(approx(v.zoom(), 1.0 / 1.1));
        v.set_zoom(1.0e9);
        assert_eq!(v.zoom(), MAX_ZOOM);
        v.set_zoom(0.0);
        assert_eq!(v.zoom(), MIN_ZOOM);
        v.set_zoom_mode(ZoomMode::ActualSize);
        assert_eq!(v.zoom(), 1.0);
    }

    #[test]
    fn textlager_i_minnet_har_tak_och_behaller_det_som_behovs() {
        let ctx = Context::default();
        let mut v = open_view(&ctx, 3);
        // Många sidors textlager (som vid sökning i ett dokument med tusentals sidor).
        for p in 100..100 + MAX_TEXT_PAGES * 2 {
            v.texts.insert(p, Arc::new(PageText::default()));
        }
        v.texts.insert(0, Arc::new(PageText::default()));
        v.ocr_pages.insert(100);
        v.evict_texts();
        assert!(v.texts.len() <= MAX_TEXT_PAGES, "{}", v.texts.len());
        assert!(v.texts.contains_key(&0), "sidan vid vyn behålls");
        assert!(v.texts.contains_key(&100), "OCR-text släpps aldrig");
        // "Markera allt" behåller allt.
        for p in 1000..1400 {
            v.texts.insert(p, Arc::new(PageText::default()));
        }
        v.want_all_text = true;
        let n = v.texts.len();
        v.evict_texts();
        assert_eq!(v.texts.len(), n);
    }

    #[test]
    fn traffnumret_raknas_utan_traffista() {
        let ctx = Context::default();
        let mut v = open_view(&ctx, 3);
        v.find_set("Stycke nummer", true, false);
        run_until(&ctx, &mut v, |v| !v.find_status().searching);
        v.find_next(false);
        v.find_next(false);
        let list = v.find_list();
        let cur = v.find.as_ref().unwrap().current.unwrap();
        assert_eq!(v.find_status().current, list.iter().position(|x| *x == cur));
        assert_eq!(v.find_status().current, Some(2));
    }

    /// Mätning: PDF med många sidor (`KLADD_BENCH_PDF_PAGES`, standard 1200).
    #[test]
    #[ignore]
    fn bench_pdf_many_pages() {
        let pages: usize = std::env::var("KLADD_BENCH_PDF_PAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(1200);
        let rss = || {
            let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
            let f = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / 1024.0;
            (f("VmRSS:"), f("VmHWM:"))
        };
        let t = Instant::now();
        let bytes = make_pdf(pages);
        println!("PDF: skapa {} sidor, {} MB: {:.0} ms", pages, bytes.len() / 1_000_000, t.elapsed().as_secs_f64() * 1000.0);
        let r0 = rss().0;
        let ctx = Context::default();
        let t = Instant::now();
        let engine = PdfEngine::open(bytes.clone(), None, ctx.clone()).expect("öppna");
        let meta = super::super::meta::read_meta(&bytes, None);
        let mut v = PdfView::new(engine, meta, bytes, None);
        println!("PDF: öppna {:.0} ms ({} sidor)", t.elapsed().as_secs_f64() * 1000.0, v.page_count());
        run_until(&ctx, &mut v, |v| !v.tex.is_empty());
        let mut worst = 0.0f64;
        for _ in 0..30 {
            let t = Instant::now();
            frame(&ctx, &mut v);
            worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("PDF: bildruta (rullat till början) värsta {worst:.1} ms");
        let t = Instant::now();
        v.find_set("Stycke nummer 40000", true, false);
        let mut peak_texts = 0;
        let end = Instant::now() + Duration::from_secs(300);
        while v.find_status().searching && Instant::now() < end {
            frame(&ctx, &mut v);
            peak_texts = peak_texts.max(v.texts.len());
            std::thread::sleep(Duration::from_millis(2));
        }
        println!("PDF: sök i alla sidor {:.0} ms, träffar {}, mest textlager i minnet {}", t.elapsed().as_secs_f64() * 1000.0, v.find_status().total, peak_texts);
        let (r1, hwm) = rss();
        println!("PDF: minne +{:.0} MB (topp {hwm:.0})", r1 - r0);
        v.go_to_page(pages - 1);
        run_until(&ctx, &mut v, |v| v.current_page() == pages - 1);
    }
}
