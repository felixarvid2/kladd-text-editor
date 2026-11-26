//! Sidbrytning med avsnitt (`sections`): varje avsnitt kan ha egen sidstorlek, egna marginaler
//! och flera spalter. Galleriet läggs ut stycke för stycke med spaltens bredd (se
//! `layout::relayout`); `paginate_sections` flyttar sedan raderna: en spalt fylls uppifrån och
//! ned, därefter nästa spalt, därefter nästa sida. Raderna flyttas i själva galleriet (också i x),
//! så markör, markering och träffytor följer med av sig själva.
//!
//! Ark med olika bredd centreras på skrivbordet. Galleriets x = 0 ligger vid vänstra kanten av
//! den vänstraste textytan (`Geom::origin_x`), så att alla rader har x ≥ 0 och TextEdits yta täcker
//! allt.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::Galley;
use egui::Rect;

use super::fnotes::{NoteFlow, NoteSpec};
use super::layout::px;
use super::pages::{Geom, SHEET_GAP};
use super::sections::Section;

/// Ett avsnitts sidgeometri i px vid en given zoom.
#[derive(Clone, Debug, PartialEq)]
pub struct SecGeom {
    pub page_w: f32,
    pub page_h: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub cols: usize,
    /// Spaltens bredd (första spalten när de är olika breda).
    pub col_w: f32,
    pub gap: f32,
    /// Spaltbredderna när de är olika breda (annars tom).
    pub col_ws: Vec<f32>,
    /// Skiljelinje mellan spalterna.
    pub rule: bool,
    /// Börjar på samma sida som föregående avsnitt (om sidorna är lika stora).
    pub continuous: bool,
    /// Spalterna jämnas inte ut på avsnittets sista ark.
    pub no_balance: bool,
}

impl SecGeom {
    pub fn new(zoom: f32, s: &Section) -> SecGeom {
        let (w, h) = s.page;
        let [t, r, b, l] = s.margin;
        // Marginalerna får aldrig äta upp hela sidan (som i `Geom::new`).
        let cap = |m: f32, dim: f32| px(zoom, m.clamp(0.0, dim * 0.4));
        let (left, right) = (cap(l, w), cap(r, w));
        let page_w = px(zoom, w);
        let text_w = (page_w - left - right).max(1.0);
        let gap = px(zoom, s.gap);
        let n = s.columns.max(1) as f32;
        // Spalterna får inte bli smalare än 20 px.
        let gap = if n > 1.0 { gap.min(((text_w - 20.0 * n) / (n - 1.0)).max(0.0)) } else { gap };
        // Olika breda spalter: bredderna skalas så att de tillsammans med mellanrummen fyller textytan.
        let avail = (text_w - gap * (n - 1.0)).max(1.0);
        let sum: f32 = s.col_widths.iter().sum();
        let col_ws: Vec<f32> = if s.col_widths.len() == s.columns && sum > 0.0 {
            s.col_widths.iter().map(|w| (w / sum * avail).max(1.0)).collect()
        } else {
            Vec::new()
        };
        SecGeom {
            page_w,
            page_h: px(zoom, h),
            top: cap(t, h),
            right,
            bottom: cap(b, h),
            left,
            cols: s.columns.max(1),
            col_w: col_ws.first().copied().unwrap_or(((text_w - gap * (n - 1.0)) / n).max(1.0)),
            gap,
            col_ws,
            rule: s.rule,
            continuous: s.continuous,
            no_balance: s.no_balance,
        }
    }

    /// Samma sidstorlek och marginaler (så att ett löpande avsnitt kan dela sida).
    fn same_sheet(&self, o: &SecGeom) -> bool {
        let c = |a: f32, b: f32| (a - b).abs() < 0.5;
        c(self.page_w, o.page_w)
            && c(self.page_h, o.page_h)
            && c(self.top, o.top)
            && c(self.bottom, o.bottom)
            && c(self.left, o.left)
            && c(self.right, o.right)
    }

    fn hash_into<H: Hasher>(&self, h: &mut H) {
        for v in [self.page_w, self.page_h, self.top, self.right, self.bottom, self.left, self.col_w, self.gap] {
            v.to_bits().hash(h);
        }
        self.cols.hash(h);
        self.rule.hash(h);
        for w in &self.col_ws {
            w.to_bits().hash(h);
        }
        self.continuous.hash(h);
        self.no_balance.hash(h);
    }
}

impl Geom {
    /// Sidgeometri för dokumentets avsnitt. Ett vanligt dokument (ett avsnitt som följer
    /// grundformatet, en spalt) ger samma `Geom` som `Geom::new`.
    pub fn for_sections(zoom: f32, d: &super::markup::DocDefaults, sections: &[Section]) -> Geom {
        let base = Geom::new(zoom, d);
        let plain = match sections {
            [] => true,
            [s] => {
                let (pw, ph) = d.page();
                let c = |a: f32, b: f32| (a - b).abs() < 0.01;
                s.columns == 1 && c(s.page.0, pw) && c(s.page.1, ph) && s.margin.iter().zip(d.margins()).all(|(a, b)| c(*a, b))
            }
            _ => false,
        };
        if plain {
            return base;
        }
        let plan = crate::hf::Plan::new(d, sections);
        let secs: Vec<SecGeom> = sections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut g = SecGeom::new(zoom, s);
                // Sidhuvud och sidfot skjuter ned texten om marginalen inte räcker.
                let r = plan.reserve(d, i);
                g.top = g.top.max(px(zoom, r[0]).min(g.page_h * 0.4));
                g.bottom = g.bottom.max(px(zoom, r[1]).min(g.page_h * 0.4));
                g
            })
            .collect();
        let f = &secs[0];
        Geom {
            page_w: f.page_w,
            page_h: f.page_h,
            top: f.top,
            right: f.right,
            bottom: f.bottom,
            left: f.left,
            gap: SHEET_GAP,
            secs: Arc::new(secs),
        }
    }

    /// Dokumentet har avsnitt som kräver egen sidbrytning (annars gäller `Geom`s egna mått).
    pub fn multi(&self) -> bool {
        !self.secs.is_empty()
    }

    /// Avsnitt `i` (utan avsnitt: grundgeometrin).
    pub fn sec(&self, i: usize) -> SecGeom {
        match self.secs.get(i.min(self.secs.len().saturating_sub(1))) {
            Some(s) => s.clone(),
            None => SecGeom {
                page_w: self.page_w,
                page_h: self.page_h,
                top: self.top,
                right: self.right,
                bottom: self.bottom,
                left: self.left,
                cols: 1,
                col_w: self.text_w(),
                gap: 0.0,
                col_ws: Vec::new(),
                rule: false,
                continuous: false,
                no_balance: false,
            },
        }
    }

    /// Bredd för utläggning av stycken i avsnitt `i`: spaltens (första spaltens när de är olika breda).
    pub fn wrap_for(&self, i: usize) -> f32 {
        if self.multi() { self.sec(i).col_w } else { self.text_w() }
    }

    /// Något avsnitt har olika breda spalter (då läggs stycken ut med bredden för spalten de hamnar i).
    pub fn unequal_cols(&self) -> bool {
        self.secs.iter().any(|s| !s.col_ws.is_empty())
    }

    /// Avsnitt `i` har olika breda spalter och följs av ett löpande avsnitt på samma ark: då jämnas
    /// spalterna ut på avsnittets sista ark (se `layout::relayout`).
    pub fn balance_candidate(&self, i: usize) -> bool {
        let (a, b) = (self.sec(i), self.secs.get(i + 1));
        !a.no_balance && !a.col_ws.is_empty() && b.is_some_and(|b| b.continuous && b.same_sheet(&a))
    }

    /// Bredaste arket.
    pub fn max_page_w(&self) -> f32 {
        self.secs.iter().map(|s| s.page_w).fold(self.page_w, f32::max)
    }

    /// Arkets vänsterkant för ett ark med bredden `w`, från skrivbordsytans vänsterkant (arken centreras).
    pub fn sheet_left(&self, w: f32) -> f32 {
        (self.max_page_w() - w) / 2.0
    }

    /// Galleriets x = 0 i samma koordinater som `sheet_left`: den vänstraste textytans kant.
    pub fn origin_x(&self) -> f32 {
        if !self.multi() {
            return self.left;
        }
        self.secs.iter().map(|s| self.sheet_left(s.page_w) + s.left).fold(f32::INFINITY, f32::min)
    }

    /// Bredden på ytan som TextEdit täcker: från `origin_x` till den högraste textkanten.
    pub fn edit_w(&self) -> f32 {
        if !self.multi() {
            return self.text_w();
        }
        let right = self.secs.iter().map(|s| self.sheet_left(s.page_w) + s.page_w - s.right).fold(0.0, f32::max);
        (right - self.origin_x()).max(1.0)
    }

    pub fn hash_secs<H: Hasher>(&self, h: &mut H) {
        self.secs.len().hash(h);
        for s in self.secs.iter() {
            s.hash_into(h);
        }
    }

    /// Arken för ett vanligt dokument: `count` lika stora ark under varandra.
    pub fn uniform_boxes(&self, count: usize) -> Vec<PageBox> {
        (0..count)
            .map(|k| PageBox {
                top: -self.top + k as f32 * self.stride(),
                left: 0.0,
                w: self.page_w,
                h: self.page_h,
                margin: [self.top, self.right, self.bottom, self.left],
                regions: vec![Region { y0: k as f32 * self.stride(), cols: 1, col_w: self.text_w(), gap: 0.0, col_ws: Vec::new(), rule: false, used: 0.0 }],
                sec: 0,
                notes: Vec::new(),
                reserved: 0.0,
            })
            .collect()
    }
}

/// En del av ett ark med en bestämd spaltindelning: hela arket, eller (efter ett löpande
/// avsnittsbyte) delen under bytet.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// Galleriets y där delen börjar.
    pub y0: f32,
    pub cols: usize,
    pub col_w: f32,
    pub gap: f32,
    /// Spaltbredderna när de är olika breda (annars tom).
    pub col_ws: Vec<f32>,
    /// Skiljelinje mellan spalterna.
    pub rule: bool,
    /// Nedre kanten av det som ligger i delen (galleriets y); skiljelinjerna slutar här.
    pub used: f32,
}

impl Region {
    /// Spaltens bredd.
    pub fn width_of(&self, c: usize) -> f32 {
        self.col_ws.get(c).copied().unwrap_or(self.col_w)
    }

    /// Spaltens vänsterkant från textytans vänsterkant.
    pub fn left_of(&self, c: usize) -> f32 {
        (0..c).map(|k| self.width_of(k) + self.gap).sum()
    }

    /// Spalten som en x-position (från textytans vänsterkant) hör till.
    pub fn col_at(&self, rel: f32) -> usize {
        (0..self.cols.max(1) - 1).find(|c| rel < self.left_of(*c) + self.width_of(*c) + self.gap / 2.0).unwrap_or(self.cols.max(1) - 1)
    }

    /// Skiljelinjernas x (från textytans vänsterkant): mitt i varje mellanrum.
    pub fn rule_xs(&self) -> Vec<f32> {
        if !self.rule || self.cols < 2 {
            return Vec::new();
        }
        (0..self.cols - 1).map(|c| self.left_of(c) + self.width_of(c) + self.gap / 2.0).collect()
    }
}

/// Ett ark: läge och mått i px. `top` räknas från galleriets y = 0, `left` från skrivbordsytans
/// vänsterkant (se `Geom::sheet_left`).
#[derive(Clone, Debug, PartialEq)]
pub struct PageBox {
    pub top: f32,
    pub left: f32,
    pub w: f32,
    pub h: f32,
    /// Marginaler: topp, höger, botten, vänster.
    pub margin: [f32; 4],
    pub regions: Vec<Region>,
    /// Avsnittet arket börjar i.
    pub sec: usize,
    /// Fotnoterna (index i `NoteLayout`-listan) vars referenser står på arket, och höjden som
    /// reserverats för dem längst ned i textytan (se `fnotes`).
    pub notes: Vec<usize>,
    pub reserved: f32,
}

impl PageBox {
    pub fn text_top(&self) -> f32 {
        self.top + self.margin[0]
    }

    pub fn text_bottom(&self) -> f32 {
        self.top + self.h - self.margin[2]
    }

    /// Där fotnoternas område börjar (textytans nederkant om arket saknar fotnoter).
    pub fn notes_top(&self) -> f32 {
        self.text_bottom() - self.reserved
    }

    /// Textytans vänsterkant (samma koordinater som `left`).
    pub fn text_left(&self) -> f32 {
        self.left + self.margin[3]
    }

    /// Spaltindelningen vid galleriets y.
    pub fn region_at(&self, y: f32) -> &Region {
        let i = self.regions.partition_point(|r| r.y0 <= y + 0.5).saturating_sub(1);
        &self.regions[i]
    }

    /// Arkets rektangel; `base` = skrivbordsytans vänsterkant, `origin_y` = skärm-y för galleriets y = 0.
    pub fn rect(&self, base: f32, origin_y: f32) -> Rect {
        Rect::from_min_size(egui::pos2(base + self.left, origin_y + self.top), egui::vec2(self.w, self.h))
    }
}

/// Arket (index) som galleriets y ligger på: det sista vars textyta börjar vid eller före `y`.
pub fn page_index(boxes: &[PageBox], y: f32) -> usize {
    boxes.partition_point(|b| b.text_top() <= y + 0.5).saturating_sub(1)
}

/// Spalten (vänsterkant och bredd, i skrivbordskoordinater från `left`) som galleriets (x, y) ligger i.
/// `origin_x` = galleriets x = 0.
pub fn column_at(boxes: &[PageBox], origin_x: f32, x: f32, y: f32) -> (f32, f32) {
    let Some(b) = boxes.get(page_index(boxes, y)) else { return (origin_x, 0.0) };
    let r = b.region_at(y);
    let rel = x + origin_x - b.text_left();
    let c = r.col_at(rel);
    (b.text_left() + r.left_of(c) - origin_x, r.width_of(c))
}

/// Uppskattad spaltfyllnad under utläggningen, för avsnitt med olika breda spalter: ett stycke måste
/// läggas ut med bredden för spalten det hamnar i, men spalten avgörs först av sidbrytningen
/// (`paginate_sections`). Utläggningen följer därför samma regler i förväg, rad för rad: spalten
/// fylls ned till sidans botten, sedan nästa spalt, sedan nästa ark. Flytande bilder och balansering
/// räknas inte; en rad som hamnar fel får nöja sig med den smalare spaltens bredd.
#[derive(Clone, Debug, Default)]
pub struct ColSim {
    sec: Option<usize>,
    col: usize,
    /// Använd höjd i spalten (från delens överkant).
    y: f32,
    /// Delens överkant på arket (efter ett löpande avsnittsbyte), och längsta använda höjd på arket.
    region_y: f32,
    max_y: f32,
    /// Textytans höjd.
    cap: f32,
    /// Antal gånger spalterna i avsnittet gått över till ett nytt ark, och hur högt sista arkets del är använd.
    sheets: usize,
    /// Avsnitt med utjämnade spalter: (ark där utjämningen gäller, spalthöjd); se `Balance`.
    balance: Balance,
    /// Resultat per avsnitt (arkens antal och sista arkets höjd) när utläggningen är klar.
    pub runs: HashMap<usize, SecRun>,
}

/// Avsnitt vars spalter ska jämnas ut på sista arket: avsnitt → (sista arkets nummer i avsnittet,
/// högsta spalthöjd i px från delens överkant).
pub type Balance = HashMap<usize, (usize, f32)>;

/// Hur ett avsnitt med olika breda spalter fyllde sina ark.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SecRun {
    /// Antal gånger spalterna gått över till nästa ark (0 = allt på första arket).
    pub sheets: usize,
    /// Höjden (från delens överkant) som sista arkets spalter använder, den högsta.
    pub used: f32,
}

impl ColSim {
    pub fn with_balance(balance: Balance) -> ColSim {
        ColSim { balance, ..Default::default() }
    }

    /// Nyckel till styckecachen: spalten och fyllnaden där ett stycke börjar, och spalthöjden som gäller
    /// avsnittet om dess spalter jämnas ut.
    pub fn state_key(&self) -> [u32; 7] {
        let (sheet, cap) = self.sec.and_then(|s| self.balance.get(&s)).map_or((u32::MAX, 0), |(sh, h)| (*sh as u32, h.to_bits()));
        [self.col as u32, self.y.to_bits(), self.region_y.to_bits(), self.max_y.to_bits(), self.sheets as u32, sheet, cap]
    }

    fn width(sg: &SecGeom, col: usize) -> f32 {
        sg.col_ws.get(col).copied().unwrap_or(sg.col_w)
    }

    fn next_col(&mut self, sg: &SecGeom) {
        if self.col + 1 < sg.cols {
            self.col += 1;
        } else {
            self.col = 0;
            self.region_y = 0.0;
            self.max_y = 0.0;
            self.sheets += 1;
        }
        self.y = 0.0;
    }

    /// Högsta spalthöjd just nu: textytans, eller den utjämnade på avsnittets sista ark.
    fn limit(&self) -> f32 {
        match self.sec.and_then(|s| self.balance.get(&s)) {
            Some((sheet, h)) if *sheet == self.sheets => self.cap.min(self.region_y + h),
            _ => self.cap,
        }
    }

    fn overflows(&self, h: f32) -> bool {
        self.y > 0.0 && self.region_y + self.y + h > self.limit() + 0.5
    }

    fn note(&mut self) {
        if let Some(s) = self.sec {
            self.runs.insert(s, SecRun { sheets: self.sheets, used: (self.max_y - self.region_y).max(0.0) });
        }
    }

    /// Börjar ett stycke i avsnitt `sec` (`brk` = spaltbrytning före det): bredden där det börjar.
    pub fn start(&mut self, geom: &Geom, sec: usize, brk: bool) -> f32 {
        let sg = geom.sec(sec);
        if self.sec != Some(sec) {
            let same_sheet = self.sec.is_some() && sg.continuous;
            self.region_y = if same_sheet { self.max_y.max(self.region_y + self.y) } else { 0.0 };
            if !same_sheet {
                self.max_y = 0.0;
            }
            self.col = 0;
            self.y = 0.0;
            self.sheets = 0;
            self.cap = (sg.page_h - sg.top - sg.bottom).max(1.0);
            self.sec = Some(sec);
        }
        if brk {
            self.next_col(&sg);
        }
        Self::width(&sg, self.col)
    }

    /// Placerar enheter med höjderna `hs`; ger den smalaste spaltbredd de når.
    pub fn place(&mut self, geom: &Geom, sec: usize, hs: impl IntoIterator<Item = f32>) -> f32 {
        let sg = geom.sec(sec);
        let mut min_w = Self::width(&sg, self.col);
        for h in hs {
            if self.overflows(h) {
                self.next_col(&sg);
                min_w = min_w.min(Self::width(&sg, self.col));
            }
            self.y += h;
            self.max_y = self.max_y.max(self.region_y + self.y);
        }
        self.note();
        min_w
    }

    /// Var en rad av höjderna `hs` skulle byta spalt om de placerades nu: radens index och bredden på
    /// spalten den hamnar i (ändrar inget).
    pub fn probe(&self, geom: &Geom, sec: usize, hs: &[f32]) -> Option<(usize, f32)> {
        let sg = geom.sec(sec);
        let mut t = self.clone();
        for (i, h) in hs.iter().enumerate() {
            if t.overflows(*h) {
                t.next_col(&sg);
                return Some((i, Self::width(&sg, t.col)));
            }
            t.y += h;
            t.max_y = t.max_y.max(t.region_y + t.y);
        }
        None
    }

    /// Går till nästa spalt (eller ark); ger dess bredd.
    pub fn advance(&mut self, geom: &Geom, sec: usize) -> f32 {
        let sg = geom.sec(sec);
        self.next_col(&sg);
        Self::width(&sg, self.col)
    }
}

/// Resultatet av `paginate_sections`.
pub struct Paged {
    pub boxes: Vec<PageBox>,
    /// Hur långt (x) varje rad flyttades.
    pub dx: Vec<f32>,
}

/// En placerad enhet i pågående avsnittsdel (för att kunna balansera spalterna).
struct Unit {
    rows: Range<usize>,
    top: f32,
    col: usize,
    h: f32,
}

/// Spaltens vänsterkant från textytans vänsterkant i ett avsnitt.
fn region_left(sg: &SecGeom, col: usize) -> f32 {
    (0..col).map(|k| sg.col_ws.get(k).copied().unwrap_or(sg.col_w) + sg.gap).sum()
}

/// Minsta spalthöjd som rymmer enheterna `hs` i `cols` spalter (girig fyllning).
fn balanced_height(hs: &[f32], cols: usize) -> f32 {
    let lo0 = hs.iter().copied().fold(0.0, f32::max);
    let total: f32 = hs.iter().sum();
    let fits = |cap: f32| {
        let (mut n, mut y) = (1usize, 0.0f32);
        for &h in hs {
            if y > 0.0 && y + h > cap + 0.01 {
                n += 1;
                y = 0.0;
            }
            y += h;
        }
        n <= cols
    };
    let (mut lo, mut hi) = (lo0, total.max(lo0));
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// Sidbryter galleriet för dokument med avsnitt. `row_sec[i]` = avsnitt för rad `i`, `col_breaks` =
/// rader som är spaltbrytningar, `floats` och `groups` som i `paginate`.
#[allow(clippy::too_many_arguments)]
pub fn paginate_sections(
    g: &mut Galley,
    geom: &Geom,
    ppp: f32,
    floats: &[(usize, f32)],
    groups: &[Range<usize>],
    row_sec: &[u32],
    col_breaks: &[usize],
    page_breaks: &[usize],
    notes: &NoteSpec,
    balance: &Balance,
    overlay: &[Range<usize>],
) -> Paged {
    let n_rows = g.rows.len();
    let mut boxes: Vec<PageBox> = Vec::new();
    let mut dx = vec![0.0f32; n_rows];
    if n_rows == 0 {
        boxes.extend(geom.uniform_boxes(1));
        return Paged { boxes, dx };
    }
    let eps = 0.5 / ppp;
    let round = |v: f32| (v * ppp).round() / ppp;
    let origin = geom.origin_x();

    // Läge i pågående avsnitt.
    let mut cur_sec = usize::MAX;
    let mut sg = geom.sec(0);
    let mut region_top = 0.0f32;
    let mut region_page = 0usize;
    let mut col = 0usize;
    let mut col_y = 0.0f32;
    let mut page_bottom = 0.0f32; // lägsta y som använts på sidan
    let mut prev_bottom = f32::NEG_INFINITY;
    let mut units: Vec<Unit> = Vec::new();
    let mut broke_page = false; // avsnittsdelen har gått över en sidgräns
    let mut flow = NoteFlow::new(notes);
    // Antal ark avsnittet gått över till; med utjämnade spalter (`balance`) fylls sista arket bara till spalthöjden.
    let mut sec_sheets = 0usize;

    fn new_sheet(boxes: &mut Vec<PageBox>, geom: &Geom, sec: usize, sg: &SecGeom) {
        let top = match boxes.last() {
            Some(b) => b.top + b.h + SHEET_GAP,
            None => -sg.top,
        };
        boxes.push(PageBox {
            top,
            left: geom.sheet_left(sg.page_w),
            w: sg.page_w,
            h: sg.page_h,
            margin: [sg.top, sg.right, sg.bottom, sg.left],
            regions: vec![Region { y0: top + sg.top, cols: sg.cols, col_w: sg.col_w, gap: sg.gap, col_ws: sg.col_ws.clone(), rule: sg.rule, used: 0.0 }],
            sec,
            notes: Vec::new(),
            reserved: 0.0,
        });
    }
    // Ark som en tidigare hög rad sträcker sig över hoppas över (de blir tomma ark).
    fn skip_covered(boxes: &mut Vec<PageBox>, geom: &Geom, sec: usize, sg: &SecGeom, until: f32) {
        let mut guard = 0;
        while boxes.last().is_some_and(|b| b.text_top() < until) && guard < 10_000 {
            new_sheet(boxes, geom, sec, sg);
            guard += 1;
        }
    }
    let col_x = |boxes: &Vec<PageBox>, sg: &SecGeom, col: usize| -> f32 {
        let b = boxes.last().expect("minst ett ark");
        b.text_left() + region_left(sg, col) - origin
    };

    let mut next_group = 0;
    let mut next_ov = 0;
    // Rutor med omflöde (`overlay`): raderna ligger ovanpå texten och tar ingen plats; de placeras när raden
    // efter dem (`pending.end`) har fått sitt läge, med samma avstånd till den som de hade i layouten.
    let mut pending: Option<Range<usize>> = None;
    // Underkanten (i layoutens egna koordinater) på raden/gruppen som placerades senast: ett avstånd som layouten
    // lagt in mellan två rader (en ruta eller bild över hela bredden som texten hoppar förbi) bevaras.
    let mut prev_raw = f32::NEG_INFINITY;
    let mut float_at = super::pages::FloatAt::new(floats);
    let mut i = 0;
    while i < n_rows {
        while next_ov < overlay.len() && overlay[next_ov].start < i {
            next_ov += 1;
        }
        if let Some(ov) = overlay.get(next_ov).filter(|o| o.start == i && o.end > i && o.end < n_rows) {
            pending = Some(ov.clone());
            i = ov.end;
            next_ov += 1;
            continue;
        }
        while next_group < groups.len() && groups[next_group].start < i {
            next_group += 1;
        }
        let end = match groups.get(next_group) {
            Some(r) if r.start == i && r.end > i => r.end.min(n_rows),
            _ => i + 1,
        };
        let (mut y0, mut y1) = (f32::INFINITY, f32::NEG_INFINITY);
        for pr in &g.rows[i..end] {
            y0 = y0.min(pr.pos.y);
            y1 = y1.max(pr.pos.y + pr.row.size.y);
        }
        let h = y1 - y0;
        let skip = |col_y: f32, region_top: f32| if col_y > region_top + eps && prev_raw.is_finite() { (y0 - prev_raw).max(0.0) } else { 0.0 };
        let fh = float_at.at(i);
        let sec = row_sec.get(i).map_or(0, |s| *s as usize);

        // Nytt avsnitt: ny sida, eller (löpande) vidare på samma sida.
        if sec != cur_sec {
            let ng = geom.sec(sec);
            let same = boxes.last().is_some_and(|b| {
                let cmp = SecGeom { cols: 1, col_w: 0.0, gap: 0.0, col_ws: Vec::new(), rule: false, continuous: false, no_balance: false, page_w: b.w, page_h: b.h, top: b.margin[0], right: b.margin[1], bottom: b.margin[2], left: b.margin[3] };
                ng.same_sheet(&cmp)
            });
            if ng.continuous && same && cur_sec != usize::MAX {
                // Föregående avsnitts spalter jämnas ut på sista sidan (som Word och LibreOffice gör vid ett
                // löpande avsnittsbyte); `units` innehåller bara den sidans stycken.
                if sg.cols > 1 && sg.col_ws.is_empty() && !sg.no_balance && !units.is_empty() {
                    let hs: Vec<f32> = units.iter().map(|u| u.h).collect();
                    let cap = balanced_height(&hs, sg.cols);
                    let (mut c, mut y) = (0usize, region_top);
                    let mut bottom = region_top;
                    for u in units.iter_mut() {
                        if y > region_top && y + u.h > region_top + cap + 0.01 && c + 1 < sg.cols {
                            c += 1;
                            y = region_top;
                        }
                        let (ny, nx) = (round(y), col_x(&boxes, &sg, c) - col_x(&boxes, &sg, u.col));
                        let dy = ny - u.top;
                        for r in u.rows.clone() {
                            g.rows[r].pos.y += dy;
                            g.rows[r].pos.x += nx;
                            dx[r] += nx;
                        }
                        u.top = ny;
                        u.col = c;
                        y += u.h;
                        bottom = bottom.max(y);
                    }
                    page_bottom = page_bottom.max(bottom);
                    prev_bottom = prev_bottom.max(bottom);
                }
                let b = boxes.last_mut().expect("minst ett ark");
                let top = round(page_bottom.max(region_top));
                b.regions.push(Region { y0: top, cols: ng.cols, col_w: ng.col_w, gap: ng.gap, col_ws: ng.col_ws.clone(), rule: ng.rule, used: 0.0 });
                region_top = top;
                region_page = boxes.len() - 1;
            } else {
                new_sheet(&mut boxes, geom, sec, &ng);
                skip_covered(&mut boxes, geom, sec, &ng, prev_bottom - eps);
                let b = boxes.last().expect("minst ett ark");
                region_top = b.text_top();
                region_page = boxes.len() - 1;
            }
            sg = ng;
            cur_sec = sec;
            sec_sheets = 0;
            col = 0;
            col_y = region_top;
            page_bottom = region_top;
            units.clear();
            broke_page = false;
        }

        // Spaltbrytning: nästa spalt (eller sida) innan raden placeras.
        let is_break = col_breaks.binary_search(&i).is_ok();
        let is_page_break = page_breaks.binary_search(&i).is_ok();
        let advance = |boxes: &mut Vec<PageBox>, col: &mut usize, col_y: &mut f32, region_top: &mut f32, region_page: &mut usize, page_bottom: &mut f32, broke: &mut bool, units: &mut Vec<Unit>, sheets: &mut usize, force_page: bool| {
            if !force_page && *col + 1 < sg.cols {
                *col += 1;
                *col_y = *region_top;
            } else {
                // Ny sida i samma avsnitt.
                new_sheet(boxes, geom, cur_sec, &sg);
                skip_covered(boxes, geom, cur_sec, &sg, prev_bottom - eps);
                let b = boxes.last().expect("minst ett ark");
                *region_top = b.text_top();
                *region_page = boxes.len() - 1;
                *col = 0;
                *col_y = *region_top;
                *page_bottom = *region_top;
                *broke = true;
                *sheets += 1;
                units.clear();
            }
        };
        if is_break {
            advance(&mut boxes, &mut col, &mut col_y, &mut region_top, &mut region_page, &mut page_bottom, &mut broke_page, &mut units, &mut sec_sheets, false);
        } else if is_page_break {
            // Sidbrytning: ny sida om den nuvarande har innehåll (i någon spalt).
            if col > 0 || col_y > region_top + eps || page_bottom > region_top + eps {
                advance(&mut boxes, &mut col, &mut col_y, &mut region_top, &mut region_page, &mut page_bottom, &mut broke_page, &mut units, &mut sec_sheets, true);
            }
        } else {
            // Fotnoterna på raderna ska få plats längst ned på samma ark (över alla spalter), och med
            // utjämnade spalter (`balance`) fylls avsnittets sista ark bara till spalthöjden.
            let mut guard = 0;
            loop {
                let bi = boxes.len() - 1;
                flow.enter(bi);
                let extra = flow.need(i..end).1;
                let mut text_bottom = boxes[bi].text_bottom();
                if let Some((sheet, cap)) = balance.get(&cur_sec) {
                    if *sheet == sec_sheets {
                        text_bottom = text_bottom.min(region_top + cap);
                    }
                }
                let limit = text_bottom - flow.reserved() - extra;
                let no_room = col_y + skip(col_y, region_top) + h.max(fh) > limit + eps && col_y > region_top + eps;
                let overlaps = extra > 0.0 && page_bottom > limit + eps && page_bottom > region_top + eps;
                if !(no_room || overlaps) || guard >= 8 {
                    break;
                }
                advance(&mut boxes, &mut col, &mut col_y, &mut region_top, &mut region_page, &mut page_bottom, &mut broke_page, &mut units, &mut sec_sheets, false);
                guard += 1;
            }
        }
        flow.enter(boxes.len() - 1);
        let (new, extra) = flow.need(i..end);
        flow.commit(new, extra);
        let _ = region_page;

        let top = round(col_y + skip(col_y, region_top));
        let dy = top - y0;
        let cx = col_x(&boxes, &sg, col);
        if let Some(ov) = pending.take().filter(|o| o.end == i) {
            for r in ov {
                let pr = &mut g.rows[r];
                pr.pos.y = round(top + (pr.pos.y - y0));
                pr.pos.x += cx;
                dx[r] = cx;
            }
        }
        for (k, pr) in g.rows[i..end].iter_mut().enumerate() {
            pr.pos.y += dy;
            pr.pos.x += cx;
            dx[i + k] = cx;
        }
        units.push(Unit { rows: i..end, top, col, h });
        prev_raw = y1;
        col_y = top + h;
        prev_bottom = prev_bottom.max(col_y).max(if fh > 0.0 { top + fh } else { f32::NEG_INFINITY });
        page_bottom = page_bottom.max(col_y);
        i = end;
    }

    // Hur långt ned varje del av ett ark är använd (för skiljelinjerna mellan spalter).
    if boxes.iter().any(|b| b.regions.iter().any(|r| r.rule)) {
        for pr in &g.rows {
            let bi = page_index(&boxes, pr.pos.y);
            let b = &mut boxes[bi];
            let ri = b.regions.partition_point(|r| r.y0 <= pr.pos.y + 0.5).saturating_sub(1);
            let r = &mut b.regions[ri];
            r.used = r.used.max(pr.pos.y + pr.row.size.y);
        }
    }

    // Galleriets mått: alla rader, och bredden upp till den högraste raden.
    let mut max_x = g.rect.max.x;
    let mut max_y = g.rect.max.y;
    g.mesh_bounds = Rect::NOTHING;
    for pr in &g.rows {
        max_x = max_x.max(pr.pos.x + pr.row.size.x);
        max_y = max_y.max(pr.pos.y + pr.row.size.y);
        g.mesh_bounds |= pr.row.visuals.mesh_bounds.translate(pr.pos.to_vec2());
    }
    g.rect.max.x = max_x.max(geom.edit_w());
    g.rect.max.y = max_y.max(prev_bottom);
    for (b, n) in boxes.iter_mut().zip(flow.finish()) {
        b.notes = n.notes;
        b.reserved = n.reserved;
    }
    Paged { boxes, dx }
}

// ------------------------------------------------------------------ pekaren i spalter

/// Byteläget under pekaren (`local` i galleri-koordinater) om den står i en flerspaltig del av ett
/// ark. `Galley::cursor_from_pos` antar att raderna ligger under varandra; med spalter ligger de
/// bredvid varandra, så raden söks bland spaltens egna rader.
pub fn column_hit(text: &str, g: &Galley, boxes: &[PageBox], origin_x: f32, local: egui::Vec2) -> Option<usize> {
    let b = boxes.get(page_index(boxes, local.y))?;
    let r = b.region_at(local.y);
    if r.cols <= 1 {
        return None;
    }
    let (left, w) = column_at(boxes, origin_x, local.x, local.y);
    let (lo, hi) = (left - 0.5, left + w + r.gap / 2.0);
    let (top, bottom) = (b.top - 1.0, b.top + b.h + 1.0);
    let mut best: Option<(f32, usize)> = None;
    for (i, row) in g.rows.iter().enumerate() {
        if row.pos.x < lo || row.pos.x >= hi || row.pos.y < top || row.pos.y > bottom {
            continue;
        }
        let d = if local.y < row.pos.y {
            row.pos.y - local.y
        } else if local.y > row.pos.y + row.row.size.y {
            local.y - (row.pos.y + row.row.size.y)
        } else {
            0.0
        };
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    let (_, i) = best?;
    let idx = super::grid::GalleyIndex::new(text, g);
    let row = &g.rows[i];
    let col = row.char_at(local.x - row.pos.x).0.min(row.glyphs.len());
    Some(crate::app::char_to_byte(text, idx.start(i) + col))
}

/// Efter TextEdit: musklick och drag i flerspaltiga delar. Markören sätts om härifrån (TextEdit
/// väljer första raden med rätt y, som ligger i första spalten).
pub fn pointer_fix(
    ctx: &egui::Context,
    id: egui::Id,
    text: &str,
    g: &Galley,
    gpos: egui::Pos2,
    boxes: &[PageBox],
    origin_x: f32,
    resp: &egui::Response,
) {
    if !boxes.iter().any(|b| b.regions.iter().any(|r| r.cols > 1)) {
        return;
    }
    let key = egui::Id::unique("kladd-column-drag");
    let (pressed, down, shift, ptr) =
        ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.modifiers.shift, i.pointer.interact_pos()));
    if !down {
        ctx.data_mut(|d| d.remove::<usize>(key));
    }
    let Some(ptr) = ptr else { return };
    let hit = || column_hit(text, g, boxes, origin_x, ptr - gpos).filter(|b| super::markup::table_at(text, *b).is_none());
    let anchor = ctx.data(|d| d.get_temp::<usize>(key));
    if pressed && resp.contains_pointer() {
        match hit() {
            Some(b) => {
                let secondary = egui::text_edit::TextEditState::load(ctx, id)
                    .and_then(|s| s.cursor.char_range())
                    .map(|r| crate::app::char_to_byte(text, r.secondary.index.0));
                let from = if shift { secondary.unwrap_or(b) } else { b };
                super::tables::set_caret(ctx, id, text, from, b);
                ctx.data_mut(|d| d.insert_temp(key, from));
                ctx.request_repaint();
            }
            None => ctx.data_mut(|d| d.remove::<usize>(key)),
        }
    } else if down {
        if let (Some(from), Some(b)) = (anchor, hit()) {
            super::tables::set_caret(ctx, id, text, from, b);
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::markup::DocDefaults;
    use crate::word::sections::{Section, SectionProps};

    fn geom(md_props: &str) -> Geom {
        let d = DocDefaults::default();
        let p = SectionProps::from_comment(md_props).unwrap();
        Geom::for_sections(1.0, &d, &[Section::resolve(&p, &d)])
    }

    #[test]
    fn col_sim_follows_each_columns_width() {
        let g = geom("<!-- kladd: avsnitt; columns: 2; column-gap: 10mm; column-widths: 40mm 120mm; column-rule: yes -->");
        assert!(g.unequal_cols());
        let (w0, w1) = (g.sec(0).col_w, g.sec(0).col_ws[1]);
        assert!((w1 / w0 - 3.0).abs() < 0.01);
        let mut sim = ColSim::default();
        assert_eq!(sim.start(&g, 0, false), w0);
        let cap = g.sec(0).page_h - g.sec(0).top - g.sec(0).bottom;
        // Fyller första spalten nästan helt; ett stycke på flera rader hamnar sedan delvis i den breda.
        assert_eq!(sim.place(&g, 0, [cap - 30.0]), w0);
        assert_eq!(sim.start(&g, 0, false), w0);
        assert_eq!(sim.place(&g, 0, [20.0, 20.0, 20.0]), w0, "första raden ryms i den smala");
        // Nästa stycke börjar i den breda spalten.
        assert_eq!(sim.start(&g, 0, false), w1);
        assert_eq!(sim.place(&g, 0, [20.0, 20.0]), w1);
        assert_eq!(sim.col, 1);
        // Spaltbrytning går till nästa spalt, och efter sista spalten till ett nytt ark (första spalten).
        let mut s2 = ColSim::default();
        s2.start(&g, 0, false);
        assert_eq!(s2.start(&g, 0, true), w1);
        assert_eq!(s2.start(&g, 0, true), w0);
        // Lika breda spalter ger ingen ojämn geometri.
        assert!(!geom("<!-- kladd: avsnitt; columns: 2 -->").unequal_cols());
    }
}
