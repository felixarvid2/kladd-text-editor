//! Stegvis utläggning av stora dokument: textformningen av ett stycke är det dyra i `relayout`, och i
//! ett dokument på hundratals sidor tar den sekunder när styckecachen är tom (första öppningen,
//! zoomändring). I stället för att frysa fönstret lägger `relayout_pass` ut stycken tills tidsbudgeten
//! (`Cache::galley_paged_lazy`) är slut och ersätter resten med platshållare: stycken med rätt antal
//! tecken men inga glyfer, en uppskattad höjd och inget att rita. De första sidorna syns direkt.
//! Nästa bildruta fortsätter där den förra slutade (de färdiga styckena kommer ur cachen), och
//! till slut är allt utlagt på exakt samma sätt som i en enda omgång (samma kod, samma nycklar).

use std::time::Duration;

use egui::epaint::text::{PlacedRow, Row, RowVisuals};
use egui::pos2;

use super::*;

/// Dokument under så här många byte läggs alltid ut i en enda omgång (små dokument beter sig som förut).
pub const MIN_BYTES: usize = 60_000;
/// Tidsbudget (ms) för utläggning av stycken per bildruta: växer med antalet bildrutor så att första
/// sidan syns direkt och resten går snabbare, men aldrig över taket (fönstret ska hinna svara).
const BASE_MS: u64 = 25;
const MAX_MS: u64 = 120;
/// Budget när sidan som visas nått gränsen för det utlagda (platshållare syns): fyll i den först.
const FILL_MS: u64 = 250;

/// Budgeten för den här bildrutan, eller `None` (allt läggs ut direkt) för små dokument och när
/// något kräver ett komplett galleri (tangent, hopp, att platshållare är det enda som syns).
/// `frames` = antalet bildrutor i rad som varit ofullständiga, `boost` = platshållare syns.
pub fn budget(text_len: usize, frames: u32, boost: bool, complete_now: bool) -> Option<Duration> {
    if text_len < MIN_BYTES || complete_now {
        return None;
    }
    let ms = if boost { FILL_MS } else { (BASE_MS << frames.min(3)).min(MAX_MS) };
    Some(Duration::from_millis(ms))
}

/// Platshållare för stycken som inte lagts ut än, kalibrerade mot det som redan är utlagt.
#[derive(Default)]
pub(super) struct Stubs {
    /// En riktig rad att kopiera (fältet `section_index_at_start` går inte att sätta utifrån).
    template: Option<Arc<Row>>,
    /// Rader utan glyfer, delade per (bredd, höjd).
    rows: HashMap<(u32, u32), Arc<Row>>,
    /// Utlagda brödtextstycken: tecken, rader och höjd (px), för uppskattningen.
    chars: usize,
    nrows: usize,
    height: f32,
    /// Tomma rader mellan stycken: antal och sammanlagd höjd.
    empties: usize,
    empty_height: f32,
}

impl Stubs {
    /// Kan platshållare göras (finns det en utlagd rad att utgå från)?
    pub fn ready(&self) -> bool {
        self.template.is_some() && self.nrows > 0
    }

    /// Räknar med ett utlagt stycke i uppskattningen.
    pub fn note(&mut self, para: &Para, text: &str, galleys: &[Arc<Galley>]) {
        for lg in galleys {
            if self.template.is_none() {
                self.template = lg.rows.first().map(|r| r.row.clone());
            }
        }
        if para.collapse || para.reserve.is_some() || para.float.is_some() || galleys.len() != 1 {
            return;
        }
        let g = &galleys[0];
        let chars = text[para.range.clone()].chars().count();
        if chars == 0 {
            self.empties += 1;
            self.empty_height += g.rect.height();
            return;
        }
        // Bara flerradiga stycken: rubriker och korta rader skulle snedvrida medelhöjden.
        if chars < 60 || g.rows.len() < 2 {
            return;
        }
        self.chars += chars;
        self.nrows += g.rows.len();
        self.height += g.rect.height() - para.space_before;
    }

    fn row(&mut self, w: f32, h: f32) -> Option<Arc<Row>> {
        let t = self.template.clone()?;
        Some(
            self.rows
                .entry((w.to_bits(), h.to_bits()))
                .or_insert_with(|| {
                    let mut r = (*t).clone();
                    r.glyphs = Vec::new();
                    r.size = egui::vec2(w, h);
                    r.visuals = RowVisuals::default();
                    Arc::new(r)
                })
                .clone(),
        )
    }

    /// Platshållare för ett stycke: uppskattat antal rader med uppskattad höjd.
    pub fn para(&mut self, q: &Para, text: &str, wrap_width: f32, a: &Analysis, p: &Params, ppp: f32) -> Option<Arc<Galley>> {
        let nchars = text[q.range.clone()].chars().count();
        let (n, row_h, first_extra) = if q.collapse {
            let h = if q.toc_h > 0.0 {
                q.toc_h
            } else if q.range.is_empty() {
                1.0
            } else {
                HIDDEN_PX
            };
            (1, h, 0.0)
        } else if let Some(h) = q.reserve.or(q.empty_h).filter(|_| nchars <= 1) {
            (1, h, q.space_before)
        } else if nchars == 0 && self.empties > 0 {
            (1, self.empty_height / self.empties as f32, 0.0)
        } else {
            let font = px(p.zoom, a.defaults.size());
            let (cpr, rh) = if self.nrows > 0 && self.chars > 0 {
                (self.chars as f32 / self.nrows as f32, self.height / self.nrows as f32)
            } else {
                (wrap_width / (font * 0.5), font * LINE_SINGLE)
            };
            // Indragna stycken får färre tecken per rad.
            let avail = (wrap_width - q.ind_left - q.ind_right - q.list.map_or(0.0, |l| l.text_x)).max(24.0);
            let cpr = (cpr * avail / wrap_width.max(24.0)).max(4.0);
            (((nchars as f32 / cpr).ceil() as usize).max(1), rh, q.space_before)
        };
        let width = if wrap_width.is_finite() { wrap_width } else { 0.0 };
        let mut g = Galley::concat(Arc::new(LayoutJob::default()), &[], ppp);
        let mut y = 0.0;
        // Tecknen fördelas inte på raderna: stycket har inga glyfer, så markören hamnar vid styckets slut.
        for i in 0..n {
            let h = row_h + if i == 0 { first_extra } else { 0.0 };
            g.rows.push(PlacedRow { pos: pos2(0.0, y), row: self.row(width, h)?, ends_with_newline: false });
            y += h;
        }
        g.rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(width, y));
        Some(Arc::new(g))
    }
}
