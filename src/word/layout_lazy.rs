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

/// Styckeslag som platshållarnas storlek kalibreras per: rubrik, brödtext, lista, citat, kod, tabellrad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Body,
    Heading,
    List,
    Quote,
    Code,
    Table,
}

const KINDS: usize = 6;

impl Kind {
    fn index(self) -> usize {
        self as usize
    }

    /// Slaget på stycket `para` (efter första biten med text).
    pub fn of(para: &Para, a: &Analysis, table: bool) -> Kind {
        if table {
            return Kind::Table;
        }
        let first = a.pieces.partition_point(|pc| pc.range.end <= para.range.start);
        let pc = a.pieces[first..].iter().take_while(|pc| pc.range.start < para.range.end).find(|pc| !pc.markup).or_else(|| a.pieces.get(first));
        match pc.map(|pc| pc.block) {
            Some(Block::Heading(_) | Block::Title | Block::Subtitle) => Kind::Heading,
            Some(Block::Bullet | Block::Numbered) => Kind::List,
            Some(Block::Quote) => Kind::Quote,
            Some(Block::Code) => Kind::Code,
            Some(Block::Table) => Kind::Table,
            _ => Kind::Body,
        }
    }
}

/// Det som är utlagt av ett styckeslag: stycken, tecken, rader (viktade med radbredden i px, så att stycken i
/// smalare spalter och med indrag räknas rätt) och höjd (px, utan luften ovanför).
#[derive(Default, Clone, Copy)]
struct Calib {
    paras: usize,
    /// Tecken och rader (bredd i px × rader) i stycken med minst två rader (korta stycken säger inget om radlängden).
    chars: f64,
    row_px: f64,
    /// Rader och höjd i alla stycken av slaget.
    rows: usize,
    height: f64,
}

/// Platshållare för stycken som inte lagts ut än, kalibrerade mot det som redan är utlagt.
#[derive(Default)]
pub(super) struct Stubs {
    /// En riktig rad att kopiera (fältet `section_index_at_start` går inte att sätta utifrån).
    template: Option<Arc<Row>>,
    /// Rader utan glyfer, delade per (bredd, höjd).
    rows: HashMap<(u32, u32), Arc<Row>>,
    /// Utlagda stycken per slag.
    cal: [Calib; KINDS],
    /// Tomma rader mellan stycken: antal och sammanlagd höjd.
    empties: usize,
    empty_height: f32,
}

/// Antal synliga tecken i stycket (markup som dolda taggar och `**` ger inga tecken på raden).
fn visible_chars(q: &Para, text: &str, a: &Analysis) -> usize {
    let first = a.pieces.partition_point(|pc| pc.range.end <= q.range.start);
    a.pieces[first..]
        .iter()
        .take_while(|pc| pc.range.start < q.range.end)
        .filter(|pc| !pc.markup)
        .map(|pc| text[pc.range.start.max(q.range.start)..pc.range.end.min(q.range.end)].chars().count())
        .sum()
}

/// Radbredd (px) som texten i stycket har att fylla.
fn avail_width(q: &Para, wrap_width: f32) -> f32 {
    (wrap_width - q.ind_left - q.ind_right - q.list.map_or(0.0, |l| l.text_x)).max(24.0)
}

impl Stubs {
    /// Kan platshållare göras (finns det en utlagd rad att utgå från)?
    pub fn ready(&self) -> bool {
        // Några brödtextstycken med flera rader behövs för att uppskatta radlängd och höjd.
        let body = &self.cal[Kind::Body.index()];
        self.template.is_some() && body.paras >= 6 && body.row_px > 0.0
    }

    /// Räknar med ett utlagt stycke i uppskattningen. `table`: stycket är en rad i en tabell.
    #[allow(clippy::too_many_arguments)]
    pub fn note(&mut self, para: &Para, text: &str, a: &Analysis, wrap_width: f32, galleys: &[Arc<Galley>], table: bool, idx: usize) {
        for lg in galleys {
            if self.template.is_none() {
                self.template = lg.rows.first().map(|r| r.row.clone());
            }
        }
        if para.collapse || para.reserve.is_some() || para.float.is_some() || galleys.len() != 1 {
            return;
        }
        let g = &galleys[0];
        let chars = visible_chars(para, text, a);
        if chars == 0 && !table {
            self.empties += 1;
            self.empty_height += g.rect.height();
            return;
        }
        // Avgränsarraden i en tabell är en dold rad som inte säger något om radhöjden.
        if table && idx == 1 {
            return;
        }
        let kind = Kind::of(para, a, table);
        let c = &mut self.cal[kind.index()];
        c.paras += 1;
        // En tabellrad är ett galleri (cellernas rader ligger i den): höjd per rad i tabellen.
        c.rows += if table { 1 } else { g.rows.len() };
        c.height += f64::from(g.rect.height() - para.space_before);
        // Bara flerradiga stycken säger något om hur många tecken en rad rymmer.
        if g.rows.len() >= 2 && chars >= 20 {
            c.chars += chars as f64;
            // Sista raden är i snitt halvfull: radens fulla längd är tecknen på de övriga raderna plus en halv.
            c.row_px += (g.rows.len() as f64 - 0.5) * f64::from(avail_width(para, wrap_width));
        }
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

    /// (tecken per px radbredd, radhöjd) för ett slag: egna siffror när det finns tillräckligt många stycken, annars
    /// brödtextens, omräknade med teckenstorlekarna (rubriker är större, kod mindre).
    fn per_kind(&self, kind: Kind, a: &Analysis) -> Option<(f32, f32)> {
        let body = &self.cal[Kind::Body.index()];
        if body.rows == 0 || body.row_px <= 0.0 {
            return None;
        }
        let (body_cpx, body_rh) = ((body.chars / body.row_px) as f32, (body.height / body.rows as f64) as f32);
        let c = &self.cal[kind.index()];
        let own_rh = (c.paras >= 3 && c.rows > 0).then(|| (c.height / c.rows as f64) as f32);
        let own_cpx = (c.row_px > 0.0 && c.chars >= 400.0).then(|| (c.chars / c.row_px) as f32);
        if kind == Kind::Body {
            return Some((body_cpx, body_rh));
        }
        let base = a.defaults.size();
        let ratio = match kind {
            Kind::Heading => 1.4,
            Kind::Code => block_size_pt(Block::Code, true, base) / base,
            _ => 1.0,
        };
        Some((own_cpx.unwrap_or(body_cpx / ratio), own_rh.unwrap_or(body_rh * ratio)))
    }

    /// Platshållare för ett stycke: uppskattat antal rader med uppskattad höjd. `table`/`idx`: raden i en tabell
    /// (rad 1 är avgränsarraden).
    #[allow(clippy::too_many_arguments)]
    pub fn para(&mut self, q: &Para, text: &str, wrap_width: f32, a: &Analysis, p: &Params, ppp: f32, table: bool, idx: usize) -> Option<Arc<Galley>> {
        let nchars = visible_chars(q, text, a);
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
        } else if table && idx == 1 {
            (1, 1.0, 0.0)
        } else if nchars == 0 && self.empties > 0 && !table {
            (1, self.empty_height / self.empties as f32, 0.0)
        } else {
            let kind = Kind::of(q, a, table);
            let font = px(p.zoom, a.defaults.size());
            let (cpx, rh) = self.per_kind(kind, a).unwrap_or((1.0 / (font * 0.5), font * LINE_SINGLE));
            let avail = avail_width(q, wrap_width);
            let cpr = (cpx * avail).max(4.0);
            // En tabellrad är en rad i galleriet (cellernas rader ligger i samma rad), annars räknas raderna.
            let rows = if table { 1 } else { ((nchars as f32 / cpr).ceil() as usize).max(1) };
            (rows, rh, q.space_before)
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
