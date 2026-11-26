//! Återupptagen utläggning för mycket stora dokument (tusentals sidor). Efter en liten ändring är nästan alla grupper
//! (stycken och tabeller) oförändrade, men utläggningsvarvet (`relayout_pass`), sammanfogningen av galleriet och
//! sidindelningen gick ändå igenom hela dokumentet. Förra varvet sparas därför (`Trace`): tillståndet före varje grupp
//! (läge, flytande bilder, öppen textruta, sidsimulering) och vad varje grupp lade till (gallerier, ankare, tabeller,
//! fotnoter …). Nästa varv börjar vid den första ändrade gruppen med det sparade tillståndet, och när tillståndet före en
//! oförändrad grupp är exakt detsamma som före motsvarande grupp förra gången tas resten av den sträckan (flyttad i rader
//! och byte) från förra varvet.
//!
//! Sidindelningen (ett avsnitt, `paginate`) återupptas på samma sätt: från början av sidan där den första ändrade raden
//! står, med tillståndet som sparades där, tills en ny sida börjar med exakt samma tillstånd som förut; raderna efter
//! behåller sina lägen från förra galleriet. Sammanfogningen (`concat`) räknar på samma sätt som `Galley::concat` och
//! fortsätter bara över de ändrade grupperna.
//!
//! Resultatet är detsamma som en full omläggning (provas mot en ny cache i `resume_tests` och `layout_memo`). Går det inte
//! (en ändrad höjd flyttar allt efter, spaltutjämning, stegvis utläggning, ingen återanvänd förberedelse) görs allt.

use egui::NumExt;
use egui::emath::GuiRounding;
use egui::epaint::text::PlacedRow;
use egui::pos2;

use super::*;
use crate::word::fnotes::{NoteLayout, NoteSpec, PageNotes};

/// En flytande bild eller ruta som texten flyter runt (absoluta y i galleriet före sidindelning).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FloatBox {
    pub side: Side,
    pub w: f32,
    pub top: f32,
    pub bottom: f32,
}

/// En förankrad textruta som är öppen (dess stycken läggs ut): se `relayout_pass`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BoxSt {
    pub tb: TBox,
    pub content_h: f32,
    pub first_row: usize,
}

/// Det ett utläggningsvarv lägger till, grupp för grupp.
#[derive(Default)]
pub(super) struct Out {
    pub all: Vec<Arc<Galley>>,
    pub row_sec: Vec<u32>,
    pub col_breaks: Vec<usize>,
    pub page_breaks: Vec<usize>,
    pub anchors: Vec<(usize, f32)>,
    pub starts: Vec<(usize, Float, usize)>,
    pub geoms: Vec<TableGeom>,
    pub geom_rows: Vec<usize>,
    pub row_groups: Vec<Range<usize>>,
    pub pulls: Vec<(usize, f32)>,
    pub shifts: Vec<(Range<usize>, f32)>,
    pub box_rows: Vec<Range<usize>>,
    pub notes: Vec<NoteLayout>,
}

/// Längderna på `Out`:s listor (utom `row_sec`, som har en post per rad).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Counts {
    pub all: usize,
    col_breaks: usize,
    page_breaks: usize,
    anchors: usize,
    starts: usize,
    geoms: usize,
    row_groups: usize,
    pulls: usize,
    shifts: usize,
    box_rows: usize,
    notes: usize,
}

impl Out {
    pub fn counts(&self) -> Counts {
        Counts {
            all: self.all.len(),
            col_breaks: self.col_breaks.len(),
            page_breaks: self.page_breaks.len(),
            anchors: self.anchors.len(),
            starts: self.starts.len(),
            geoms: self.geoms.len(),
            row_groups: self.row_groups.len(),
            pulls: self.pulls.len(),
            shifts: self.shifts.len(),
            box_rows: self.box_rows.len(),
            notes: self.notes.len(),
        }
    }
}

/// Flytande bilder som fortfarande påverkar texten och en öppen textruta (med sin första rad räknad bakåt från gruppens
/// första rad): en del av tillståndet före en grupp, oftast tomt.
pub(super) type Extra = (Vec<FloatBox>, Option<BoxSt>);

fn extra_of(active: &[FloatBox], boxst: Option<&BoxSt>, row_no: usize) -> Option<Extra> {
    if active.is_empty() && boxst.is_none() {
        return None;
    }
    let b = boxst.map(|b| BoxSt { first_row: if b.first_row == usize::MAX { usize::MAX } else { row_no - b.first_row }, ..*b });
    Some((active.to_vec(), b))
}

/// Sätter tillbaka ett sparat tillstånd i utläggningens variabler (`row_no` = gruppens första rad i det nya varvet).
#[allow(clippy::too_many_arguments)]
pub(super) fn restore(y0: f32, sim0: Option<(u32, u32)>, extra: Option<&Extra>, y: &mut f32, sim: &mut Option<PageSim>, active: &mut Vec<FloatBox>, boxst: &mut Option<BoxSt>, row_no: usize) {
    *y = y0;
    if let (Some(s), Some((page, shift))) = (sim.as_mut(), sim0) {
        s.page = page as usize;
        s.shift = f32::from_bits(shift);
    }
    active.clear();
    *boxst = None;
    if let Some(x) = extra {
        active.extend_from_slice(&x.0);
        *boxst = x.1.map(|b| BoxSt { first_row: if b.first_row == usize::MAX { usize::MAX } else { row_no - b.first_row }, ..b });
    }
}

/// Glesa poster (grupp, värde) i stigande gruppordning: värdet för grupp `g` är det i den sista posten till och med `g`
/// (`at`), eller bara poster för exakt `g` (`exact`).
fn sparse_at<T>(v: &[(u32, T)], g: usize) -> Option<&T> {
    let i = v.partition_point(|e| e.0 as usize <= g).checked_sub(1)?;
    Some(&v[i].1)
}

fn sparse_exact<T>(v: &[(u32, T)], g: usize) -> Option<&T> {
    sparse_at(v, g).filter(|_| v[v.partition_point(|e| e.0 as usize <= g) - 1].0 as usize == g)
}

/// Inspelning av ett varv, grupp för grupp (blir nästa `Trace`). Listorna med en post per grupp har en extra post för
/// tillståndet efter sista gruppen; `extra` och `cnt` är glesa.
#[derive(Default)]
pub(super) struct Rec {
    pub y: Vec<f32>,
    pub sim: Vec<(u32, u32)>,
    pub has_sim: bool,
    pub extra: Vec<(u32, Box<Extra>)>,
    pub row: Vec<usize>,
    pub gall: Vec<usize>,
    pub wstart: Vec<usize>,
    pub key: Vec<u64>,
    /// Längderna på listorna före gruppen, där de ändrats.
    pub cnt: Vec<(u32, Counts)>,
    /// Återupptaget varv: listorna får bara de ändrade grupperna (sätts ihop i `Fast::finish_rec`).
    pub fast: bool,
    /// Gruppen som senast fick ett tillstånd (`state`), för att slå ihop lika längder i följd även i ett återupptaget varv.
    last_state: Option<usize>,
}

impl Rec {
    pub fn with_capacity(n: usize, has_sim: bool) -> Rec {
        Rec {
            y: Vec::with_capacity(n + 1),
            sim: Vec::with_capacity(if has_sim { n + 1 } else { 0 }),
            has_sim,
            extra: Vec::new(),
            row: Vec::with_capacity(n + 1),
            gall: Vec::with_capacity(n + 1),
            wstart: Vec::with_capacity(n),
            key: Vec::with_capacity(n),
            cnt: Vec::new(),
            fast: false,
            last_state: None,
        }
    }

    /// Längderna (utom galleriernas, som finns i `gall`) före grupp `g`, om de ändrats.
    fn push_cnt(&mut self, g: usize, c: Counts) {
        let c = Counts { all: 0, ..c };
        // I ett återupptaget varv kan gamla poster hamna emellan (sträckorna): slå bara ihop med föregående grupp.
        let same_prev = self.cnt.last().is_some_and(|e| e.1 == c && (!self.fast || (g > 0 && self.last_state == Some(g - 1))));
        if !same_prev {
            self.cnt.push((g as u32, c));
        }
    }

    /// Tillståndet före grupp `g` (eller efter sista gruppen, utan `wstart`).
    #[allow(clippy::too_many_arguments)]
    pub fn state(&mut self, g: usize, y: f32, sim: Option<&PageSim>, active: &[FloatBox], boxst: Option<&BoxSt>, row_no: usize, c: Counts) {
        self.y.push(y);
        if self.has_sim {
            self.sim.push(sim.map_or((0, 0), |s| (s.page as u32, s.shift.to_bits())));
        }
        if let Some(x) = extra_of(active, boxst, row_no) {
            self.extra.push((g as u32, Box::new(x)));
        }
        self.row.push(row_no);
        self.gall.push(c.all);
        self.push_cnt(g, c);
        self.last_state = Some(g);
    }

    pub fn begin(&mut self, wstart: usize) {
        self.wstart.push(wstart);
        self.key.push(0);
    }
}

/// Det förra varvet lämnade efter sig (se modulens beskrivning): `Rec` och listorna.
pub(super) struct Trace {
    /// Allt utom grupperna som avgör utläggningen (`pass_sig`).
    pub sig: u64,
    /// `DocState::rev` för texten varvet gällde.
    pub rev: u64,
    pub rec: Rec,
    pub out: Out,
    /// Sammanfogning och sidindelning (bara för dokument med ett avsnitt och sidor).
    pub page: Option<PageTrace>,
}

impl Trace {
    fn n(&self) -> usize {
        self.rec.key.len()
    }

    fn counts_at(&self, g: usize) -> Counts {
        sparse_at(&self.rec.cnt, g).copied().unwrap_or_default()
    }

    /// Är tillståndet före gammal grupp `gj` detsamma som `(y, sim, extra)`?
    fn same_state(&self, gj: usize, y: f32, sim: Option<&PageSim>, extra: Option<&Extra>) -> bool {
        let r = &self.rec;
        r.y[gj].to_bits() == y.to_bits()
            && (!r.has_sim || sim.map_or((0, 0), |s| (s.page as u32, s.shift.to_bits())) == r.sim[gj])
            && sparse_exact(&r.extra, gj).map(|b| &**b) == extra
    }
}

/// Sammanfogningen och sidindelningen i förra varvet, per grupp (listorna med en extra post för slutet) och per sida.
pub(super) struct PageTrace {
    /// Galleriets rektangel före gruppen (som i `Galley::concat`) och tecknen före gruppen.
    pub rect: Vec<Rect>,
    pub chars: Vec<usize>,
    /// Gruppens rader efter textrutornas omflöde (bara om det finns drag) och deras ritytor efter sidindelningen.
    pub urect: Vec<Rect>,
    pub fmesh: Vec<Rect>,
    pub nv: Vec<u32>,
    pub ni: Vec<u32>,
    pub elided: Vec<bool>,
    /// Tillståndet före den första raden på varje sida.
    pub snaps: Vec<PSnap>,
    pub pout: Vec<PageNotes>,
    /// Sidindelningens sista `prev_bottom` och `float_bottom`.
    pub end: (f32, f32),
    pub spec: NoteSpec,
    pub pulls: Vec<(usize, f32)>,
    /// Summorna för `CHUNK` grupper i taget (så att hela listorna inte behöver gås igenom efter en liten ändring).
    pub sums: Vec<Sum>,
}

/// Antal grupper per post i `PageTrace::sums`.
pub(super) const CHUNK: usize = 256;

/// Summorna för en följd av grupper: hörn, index, `elided`, ritytor och rektangel (efter textrutornas omflöde).
#[derive(Clone, Copy, Debug)]
pub(super) struct Sum {
    pub nv: usize,
    pub ni: usize,
    pub el: bool,
    pub mesh: Rect,
    pub urect: Rect,
}

/// Summan för grupperna `r`.
pub(super) fn sum_of(nv: &[u32], ni: &[u32], el: &[bool], fmesh: &[Rect], urect: &[Rect], r: Range<usize>) -> Sum {
    let mut s = Sum { nv: 0, ni: 0, el: false, mesh: Rect::NOTHING, urect: Rect::NOTHING };
    for g in r {
        s.nv += nv[g] as usize;
        s.ni += ni[g] as usize;
        s.el |= el[g];
        s.mesh |= fmesh.get(g).copied().unwrap_or(Rect::NOTHING);
        if let Some(u) = urect.get(g) {
            s.urect |= *u;
        }
    }
    s
}

/// Summorna för alla grupper, `CHUNK` i taget.
pub(super) fn sums_of(nv: &[u32], ni: &[u32], el: &[bool], fmesh: &[Rect], urect: &[Rect]) -> Vec<Sum> {
    (0..nv.len().div_ceil(CHUNK)).map(|k| sum_of(nv, ni, el, fmesh, urect, k * CHUNK..((k + 1) * CHUNK).min(nv.len()))).collect()
}

/// Sidindelningens tillstånd före den första raden (gruppen av rader) på en sida.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PSnap {
    pub row: usize,
    pub page: usize,
    pub shift: f32,
    pub prev_bottom: f32,
    pub float_bottom: f32,
    pub cur: Vec<usize>,
    pub reserved: f32,
}

/// Nyckel för allt utom grupperna som avgör varvet.
pub(super) fn pass_sig(memo_sig: u64, wrap_width: f32, ppp: f32, multi: bool) -> u64 {
    let mut h = Fx(memo_sig);
    wrap_width.to_bits().hash(&mut h);
    ppp.to_bits().hash(&mut h);
    multi.hash(&mut h);
    h.finish()
}

/// Flyttar element ur en gammal lista i ordning (de som hoppas över släpps).
struct Mover<T> {
    it: std::vec::IntoIter<T>,
    pos: usize,
}

impl<T> Mover<T> {
    fn new(v: Vec<T>, base: usize) -> Self {
        Mover { it: v.into_iter(), pos: base }
    }

    /// Elementen med gamla index `from..to` (`from` får inte vara mindre än tidigare `to`).
    fn take(&mut self, from: usize, to: usize) -> impl Iterator<Item = T> + '_ {
        if from > self.pos {
            let skip = from - self.pos;
            if skip > 0 {
                self.it.nth(skip - 1);
            }
            self.pos = from;
        }
        let n = to.saturating_sub(self.pos);
        self.pos += n;
        self.it.by_ref().take(n)
    }
}

fn split<T>(v: &mut Vec<T>, at: usize) -> Vec<T> {
    v.split_off(at.min(v.len()))
}

/// De gamla listorna från den första ändrade gruppen och framåt.
struct Tail {
    all: Mover<Arc<Galley>>,
    row_sec: Mover<u32>,
    col_breaks: Mover<usize>,
    page_breaks: Mover<usize>,
    anchors: Mover<(usize, f32)>,
    starts: Mover<(usize, Float, usize)>,
    geoms: Mover<TableGeom>,
    geom_rows: Mover<usize>,
    row_groups: Mover<Range<usize>>,
    pulls: Mover<(usize, f32)>,
    shifts: Mover<(Range<usize>, f32)>,
    box_rows: Mover<Range<usize>>,
    notes: Mover<NoteLayout>,
}

fn mv(x: usize, d: isize) -> usize {
    (x as isize + d) as usize
}

fn mvr(r: &Range<usize>, d: isize) -> Range<usize> {
    mv(r.start, d)..mv(r.end, d)
}

fn shift_geom(g: &mut TableGeom, d: isize) {
    if d == 0 {
        return;
    }
    g.region = mvr(&g.region, d);
    for row in &mut g.rows {
        row.line = mvr(&row.line, d);
        for c in &mut row.cells {
            c.chunk = mvr(&c.chunk, d);
            c.seg = mvr(&c.seg, d);
        }
    }
    g.delim = g.delim.as_ref().map(|r| mvr(r, d));
}

fn add(a: Counts, b: Counts, c: Counts) -> Counts {
    Counts {
        all: a.all + b.all - c.all,
        col_breaks: a.col_breaks + b.col_breaks - c.col_breaks,
        page_breaks: a.page_breaks + b.page_breaks - c.page_breaks,
        anchors: a.anchors + b.anchors - c.anchors,
        starts: a.starts + b.starts - c.starts,
        geoms: a.geoms + b.geoms - c.geoms,
        row_groups: a.row_groups + b.row_groups - c.row_groups,
        pulls: a.pulls + b.pulls - c.pulls,
        shifts: a.shifts + b.shifts - c.shifts,
        box_rows: a.box_rows + b.box_rows - c.box_rows,
        notes: a.notes + b.notes - c.notes,
    }
}

/// En sträcka oförändrade grupper som togs från förra varvet: nya grupper `gi..gi+len` = gamla `gj..gj+len`, och de nya
/// raderna `rows` motsvarar gamla rader från `old_row`.
#[derive(Clone, Debug)]
pub(super) struct Run {
    pub gi: usize,
    pub gj: usize,
    pub len: usize,
    pub rows: Range<usize>,
    pub old_row: usize,
    /// Förskjutningen i gallerier och byte.
    pub da: isize,
    pub db: isize,
    /// Listornas längder före sträckan, nu och förut.
    pub cbase: Counts,
    pub c0: Counts,
}

/// Som `patch_dense` för glesa listor (grupp, värde) i gruppordning: posterna i sträckorna flyttas (`adj` och nytt
/// gruppindex), de i ersatta grupper försvinner och `patch` (nya poster i gruppordning, med de ändrade gruppernas och
/// en post där varje sträcka börjar) sätts in.
fn patch_sparse<T>(v: &mut Vec<(u32, T)>, runs: &[Run], n_old: usize, n_new: usize, end: usize, mut patch: Vec<(u32, T)>, adj: impl Fn(&mut T, &Run) -> bool) {
    let pp = |v: &[(u32, T)], g: usize| v.partition_point(|e| (e.0 as usize) < g);
    let mut gaps: Vec<(Range<usize>, Range<usize>)> = Vec::with_capacity(runs.len() + 1);
    let (mut po, mut pn) = (0, 0);
    for r in runs {
        let len = r.len + if r.gj + r.len == n_old && r.gi + r.len == n_new { end } else { 0 };
        let (a, b) = (pp(v, r.gj), pp(v, r.gj + len));
        let dg = r.gi as isize - r.gj as isize;
        for e in &mut v[a..b] {
            if !adj(&mut e.1, r) && dg == 0 {
                break;
            }
            e.0 = mv(e.0 as usize, dg) as u32;
        }
        gaps.push((po..r.gj, pn..r.gi + 1));
        po = r.gj + len;
        pn = r.gi + len;
    }
    gaps.push((po..n_old + end, pn..n_new + end + 1));
    for (old, new) in gaps.into_iter().rev() {
        let k0 = patch.partition_point(|e| (e.0 as usize) < new.start);
        let tail = patch.split_off(k0);
        let (keep, rest): (Vec<_>, Vec<_>) = tail.into_iter().partition(|e| (e.0 as usize) < new.end);
        patch.extend(rest);
        let (a, b) = (pp(v, old.start), pp(v, old.end));
        v.splice(a..b, keep);
    }
}

/// Lägger in nya poster (`patch`: grupperna mellan sträckorna, i ny ordning) i en lista med en post per gammal grupp
/// (`v`; `end` = 1 om listan har en sista post för tillståndet efter sista gruppen) och justerar sträckornas poster med
/// `adj`, på plats (utan att kopiera hela listan).
fn patch_dense<T: Clone>(v: &mut Vec<T>, runs: &[Run], n_old: usize, n_new: usize, end: usize, mut patch: Vec<T>, adj: impl Fn(&mut T, isize), delta: impl Fn(&Run) -> isize) {
    let mut gaps: Vec<(Range<usize>, usize)> = Vec::with_capacity(runs.len() + 1);
    let (mut po, mut pn) = (0, 0);
    for r in runs {
        let len = r.len + if r.gj + r.len == n_old && r.gi + r.len == n_new { end } else { 0 };
        let d = delta(r);
        if d != 0 {
            for x in &mut v[r.gj..r.gj + len] {
                adj(x, d);
            }
        }
        gaps.push((po..r.gj, r.gi - pn));
        po = r.gj + len;
        pn = r.gi + len;
    }
    gaps.push((po..n_old + end, n_new + end - pn));
    debug_assert_eq!(gaps.iter().map(|g| g.1).sum::<usize>(), patch.len());
    for (old, n) in gaps.into_iter().rev() {
        let tail = patch.split_off(patch.len() - n);
        if !old.is_empty() || !tail.is_empty() {
            v.splice(old, tail);
        }
    }
}

// Prov: slår av återupptagningen (för en jämförelsecache som alltid gör hela varvet).
#[cfg(test)]
thread_local! {
    pub static OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Ett återupptaget varv.
pub(super) struct Fast {
    pub old: Trace,
    /// Antal grupper och rader i förra varvet.
    pub n_old: usize,
    pub n_old_rows: usize,
    tail: Tail,
    /// Första gruppen som inte är densamma som förut.
    pub d0: usize,
    pub runs: Vec<Run>,
    /// Gamla grupper som ersatts (deras cacheposter släpps om de inte används igen).
    pub replaced: Vec<u64>,
}

impl Fast {
    /// Börjar ett återupptaget varv om förra varvet passar (samma parametrar, förberedelsen återanvänd från just det).
    /// `out` och `rec` får förra varvets listor och tillstånd fram till den första ändrade gruppen.
    pub fn start(st: &mut DocState, prep: &memo::Prep, sig: u64, out: &mut Out, rec: &mut Rec) -> Option<Fast> {
        let Some(mut old) = st.trace.take() else {
            st.stats.resume_why = 1;
            return None;
        };
        #[cfg(test)]
        if OFF.with(|o| o.get()) {
            return None;
        }
        let n_old = old.n();
        if old.sig != sig || prep.from.is_empty() || old.rev != prep.from_rev || old.rec.y.len() != n_old + 1 || old.rec.has_sim != rec.has_sim {
            st.stats.resume_why = if old.sig != sig {
                2
            } else if prep.from.is_empty() {
                3
            } else if old.rev != prep.from_rev {
                4
            } else {
                5
            };
            return None;
        }
        let d0 = prep.from.iter().enumerate().position(|(i, &j)| j as usize != i).unwrap_or(prep.from.len()).min(n_old);
        // Listorna som lämnats till vyn (tabellernas geometri, fotnoterna, bildernas lägen) tas tillbaka.
        old.out.geoms = Arc::try_unwrap(std::mem::take(&mut st.geoms)).unwrap_or_else(|a| (*a).clone());
        old.out.notes = Arc::try_unwrap(std::mem::take(&mut st.notes)).unwrap_or_else(|a| (*a).clone());
        old.out.starts = std::mem::take(&mut st.float_starts);
        let end = old.counts_at(n_old);
        if old.out.geoms.len() != end.geoms || old.out.notes.len() != end.notes || old.out.starts.len() != end.starts {
            st.stats.resume_why = 6;
            return None;
        }
        let c = old.counts_at(d0);
        let r0 = old.rec.row[d0];
        let o = &mut old.out;
        let tail = Tail {
            all: Mover::new(o.all.split_off(old.rec.gall[d0]), old.rec.gall[d0]),
            row_sec: Mover::new(split(&mut o.row_sec, r0), r0),
            col_breaks: Mover::new(split(&mut o.col_breaks, c.col_breaks), c.col_breaks),
            page_breaks: Mover::new(split(&mut o.page_breaks, c.page_breaks), c.page_breaks),
            anchors: Mover::new(split(&mut o.anchors, c.anchors), c.anchors),
            starts: Mover::new(split(&mut o.starts, c.starts), c.starts),
            geoms: Mover::new(split(&mut o.geoms, c.geoms), c.geoms),
            geom_rows: Mover::new(split(&mut o.geom_rows, c.geoms), c.geoms),
            row_groups: Mover::new(split(&mut o.row_groups, c.row_groups), c.row_groups),
            pulls: Mover::new(split(&mut o.pulls, c.pulls), c.pulls),
            shifts: Mover::new(split(&mut o.shifts, c.shifts), c.shifts),
            box_rows: Mover::new(split(&mut o.box_rows, c.box_rows), c.box_rows),
            notes: Mover::new(split(&mut o.notes, c.notes), c.notes),
        };
        *out = std::mem::take(&mut old.out);
        // Inspelningen fram till den första ändrade gruppen är densamma som förut. De täta listorna (en post per grupp) får
        // bara de ändrade grupperna under varvet och sätts ihop med de gamla efteråt (`finish_rec`); de glesa byggs i ordning.
        rec.fast = true;
        let runs = if d0 > 0 { vec![Run { gi: 0, gj: 0, len: d0, rows: 0..r0, old_row: 0, da: 0, db: 0, cbase: c, c0: c }] } else { Vec::new() };
        let n_old_rows = old.rec.row[n_old];
        Some(Fast { old, n_old, n_old_rows, tail, d0, runs, replaced: Vec::new() })
    }

    /// Den gamla grupp som ny grupp `gi` motsvarar, om den är oförändrad.
    pub fn old_of(&self, prep: &memo::Prep, gi: usize) -> Option<usize> {
        let j = *prep.from.get(gi)? as usize;
        (j < self.old.n()).then_some(j)
    }

    /// Är tillståndet nu detsamma som före gammal grupp `gj`?
    pub fn matches(&self, gj: usize, y: f32, sim: Option<&PageSim>, active: &[FloatBox], boxst: Option<&BoxSt>, row_no: usize) -> bool {
        let r = &self.old.rec;
        if r.y[gj].to_bits() != y.to_bits() {
            return false;
        }
        self.old.same_state(gj, y, sim, extra_of(active, boxst, row_no).as_ref())
    }

    /// Ger upp det återupptagna varvet före ny grupp `upto` (höjden har ändrats och tillståndet blir inte detsamma som förut
    /// igen): inspelningen görs fullständig för grupperna före (de gamla för sträckorna, de nya däremellan) och varvet
    /// fortsätter som ett vanligt. Styckecachens poster för sträckorna markeras som använda (`tick`).
    pub fn abandon(self, rec: &mut Rec, upto: usize, units: &mut HashMap<u64, Unit, IdBuild>, tick: u64) {
        let old = &self.old.rec;
        let mut n = Rec::with_capacity(upto, rec.has_sim);
        let (mut g, mut pk) = (0usize, 0usize);
        let take_patch = |n: &mut Rec, pk: usize| {
            n.y.push(rec.y[pk]);
            if rec.has_sim {
                n.sim.push(rec.sim[pk]);
            }
            n.row.push(rec.row[pk]);
            n.gall.push(rec.gall[pk]);
            n.wstart.push(rec.wstart[pk]);
            n.key.push(rec.key[pk]);
        };
        let mut sparse_cnt: Vec<(u32, Counts)> = Vec::new();
        let mut sparse_extra: Vec<(u32, Box<Extra>)> = Vec::new();
        for r in &self.runs {
            while g < r.gi {
                take_patch(&mut n, pk);
                pk += 1;
                g += 1;
            }
            let (dr, dg) = (r.rows.start as isize - r.old_row as isize, r.gi as isize - r.gj as isize);
            let s = r.gj..r.gj + r.len;
            n.y.extend_from_slice(&old.y[s.clone()]);
            if old.has_sim {
                n.sim.extend_from_slice(&old.sim[s.clone()]);
            }
            n.row.extend(old.row[s.clone()].iter().map(|&x| mv(x, dr)));
            n.gall.extend(old.gall[s.clone()].iter().map(|&x| mv(x, r.da)));
            n.wstart.extend(old.wstart[s.clone()].iter().map(|&x| mv(x, r.db)));
            n.key.extend_from_slice(&old.key[s.clone()]);
            for k in &old.key[s.clone()] {
                if let Some(u) = units.get_mut(k) {
                    u.used = tick;
                }
            }
            let (a, b) = (old.cnt.partition_point(|e| (e.0 as usize) < s.start), old.cnt.partition_point(|e| (e.0 as usize) < s.end));
            sparse_cnt.extend(old.cnt[a..b].iter().map(|e| (mv(e.0 as usize, dg) as u32, add(r.cbase, e.1, r.c0))));
            let (a, b) = (old.extra.partition_point(|e| (e.0 as usize) < s.start), old.extra.partition_point(|e| (e.0 as usize) < s.end));
            sparse_extra.extend(old.extra[a..b].iter().map(|e| (mv(e.0 as usize, dg) as u32, e.1.clone())));
            g += r.len;
        }
        while g < upto {
            take_patch(&mut n, pk);
            pk += 1;
            g += 1;
        }
        sparse_cnt.extend(std::mem::take(&mut rec.cnt));
        sparse_cnt.sort_by_key(|e| e.0);
        sparse_extra.extend(std::mem::take(&mut rec.extra));
        sparse_extra.sort_by_key(|e| e.0);
        n.cnt = sparse_cnt;
        n.extra = sparse_extra;
        *rec = n;
    }

    /// Hur många grupper från `gi` (= gamla `gj`) som är oförändrade i följd.
    pub fn run_len(&self, prep: &memo::Prep, gi: usize, gj: usize) -> usize {
        let mut n = 0;
        while gi + n < prep.groups.len() && gj + n < self.old.n() && prep.from[gi + n] as usize == gj + n {
            n += 1;
        }
        n
    }

    /// Tar gamla grupper `gj..gj+len` till de nya (från ny grupp `gi`, rad `row_no`, byte `db` från de gamla) och spelar in
    /// dem i `rec`. Ger raden efter sträckan; tillståndet efter den sätts med `restore_after`.
    #[allow(clippy::too_many_arguments)]
    pub fn splice(&mut self, out: &mut Out, rec: &mut Rec, gi: usize, gj: usize, len: usize, row_no: usize, db: isize, multi: bool) -> usize {
        let o = &self.old;
        let (c0, c1) = (o.counts_at(gj), o.counts_at(gj + len));
        let r = &o.rec;
        let dr = row_no as isize - r.row[gj] as isize;
        let base = out.counts();
        let da = base.all as isize - r.gall[gj] as isize;
        rec.push_cnt(gi, base);
        let t = &mut self.tail;
        out.all.extend(t.all.take(r.gall[gj], r.gall[gj + len]));
        if multi {
            out.row_sec.extend(t.row_sec.take(r.row[gj], r.row[gj + len]));
        }
        out.col_breaks.extend(t.col_breaks.take(c0.col_breaks, c1.col_breaks).map(|x| mv(x, dr)));
        out.page_breaks.extend(t.page_breaks.take(c0.page_breaks, c1.page_breaks).map(|x| mv(x, dr)));
        out.anchors.extend(t.anchors.take(c0.anchors, c1.anchors).map(|(x, h)| (mv(x, dr), h)));
        out.starts.extend(t.starts.take(c0.starts, c1.starts).map(|(b, s, x)| (mv(b, db), s, mv(x, dr))));
        out.geoms.extend(t.geoms.take(c0.geoms, c1.geoms).map(|mut g| {
            shift_geom(&mut g, db);
            g
        }));
        out.geom_rows.extend(t.geom_rows.take(c0.geoms, c1.geoms).map(|x| mv(x, dr)));
        out.row_groups.extend(t.row_groups.take(c0.row_groups, c1.row_groups).map(|x| mvr(&x, dr)));
        out.pulls.extend(t.pulls.take(c0.pulls, c1.pulls).map(|(x, h)| (mv(x, dr), h)));
        out.shifts.extend(t.shifts.take(c0.shifts, c1.shifts).map(|(x, y)| (mvr(&x, dr), y)));
        out.box_rows.extend(t.box_rows.take(c0.box_rows, c1.box_rows).map(|x| mvr(&x, dr)));
        out.notes.extend(t.notes.take(c0.notes, c1.notes).map(|mut n| {
            n.line = mvr(&n.line, db);
            n.body = mvr(&n.body, db);
            n
        }));
        let row_end = mv(r.row[gj + len], dr);
        self.runs.push(Run { gi, gj, len, rows: row_no..row_end, old_row: r.row[gj], da, db, cbase: Counts { all: 0, ..base }, c0: Counts { all: 0, ..c0 } });
        row_end
    }

    /// Tillståndet efter en sträcka som slutar före gammal grupp `gj` (ny första rad `row_no`).
    pub fn restore_at(&self, gj: usize, y: &mut f32, sim: &mut Option<PageSim>, active: &mut Vec<FloatBox>, boxst: &mut Option<BoxSt>, row_no: usize) {
        let r = &self.old.rec;
        let s = r.has_sim.then(|| r.sim[gj]);
        restore(r.y[gj], s, sparse_exact(&r.extra, gj).map(|b| &**b), y, sim, active, boxst, row_no);
    }

    /// Sätter ihop inspelningens täta listor: de gamla (flyttade) för sträckorna och de nya för grupperna däremellan (som
    /// `rec` har fått under varvet, med en sista post för slutet).
    pub fn finish_rec(&mut self, rec: &mut Rec, n_new: usize) {
        let n_old = self.old.n();
        let last_covers = self.runs.last().is_some_and(|r| r.gj + r.len == n_old && r.gi + r.len == n_new);
        let old = &mut self.old.rec;
        let runs = &self.runs;
        let (mut y, mut sim, mut row, mut gall) = (std::mem::take(&mut rec.y), std::mem::take(&mut rec.sim), std::mem::take(&mut rec.row), std::mem::take(&mut rec.gall));
        if last_covers {
            y.pop();
            sim.pop();
            row.pop();
            gall.pop();
        }
        let mut v = std::mem::take(&mut old.y);
        patch_dense(&mut v, runs, n_old, n_new, 1, y, |_, _| {}, |_| 0);
        rec.y = v;
        if rec.has_sim {
            let mut v = std::mem::take(&mut old.sim);
            patch_dense(&mut v, runs, n_old, n_new, 1, sim, |_, _| {}, |_| 0);
            rec.sim = v;
        }
        let mut v = std::mem::take(&mut old.row);
        patch_dense(&mut v, runs, n_old, n_new, 1, row, |x, d| *x = mv(*x, d), |r| r.rows.start as isize - r.old_row as isize);
        rec.row = v;
        let mut v = std::mem::take(&mut old.gall);
        patch_dense(&mut v, runs, n_old, n_new, 1, gall, |x, d| *x = mv(*x, d), |r| r.da);
        rec.gall = v;
        let mut v = std::mem::take(&mut old.wstart);
        patch_dense(&mut v, runs, n_old, n_new, 0, std::mem::take(&mut rec.wstart), |x, d| *x = mv(*x, d), |r| r.db);
        rec.wstart = v;
        let mut v = std::mem::take(&mut old.key);
        patch_dense(&mut v, runs, n_old, n_new, 0, std::mem::take(&mut rec.key), |_, _| {}, |_| 0);
        rec.key = v;
        let mut v = std::mem::take(&mut old.cnt);
        patch_sparse(&mut v, runs, n_old, n_new, 1, std::mem::take(&mut rec.cnt), |c, r| {
            if r.cbase == r.c0 {
                return false;
            }
            *c = add(r.cbase, *c, r.c0);
            true
        });
        rec.cnt = v;
        let mut v = std::mem::take(&mut old.extra);
        patch_sparse(&mut v, runs, n_old, n_new, 1, std::mem::take(&mut rec.extra), |_, _| false);
        rec.extra = v;
        rec.fast = false;
    }

    /// Noterar att gamla grupper före `upto_old` som inte tagits med ersattes (deras cacheposter kan släppas).
    pub fn replaced_upto(&mut self, upto_old: usize, done: &mut usize) {
        while *done < upto_old.min(self.old.n()) {
            self.replaced.push(self.old.rec.key[*done]);
            *done += 1;
        }
    }
}

// ------------------------------------------------------------------ sammanfogning

/// `Galley::concat` för en grupps gallerier, från rektangeln `rect` (allt före gruppen): radernas lägen läggs till
/// `rows`. `last`: gruppen är dokumentets sista. Ger rektangeln efter gruppen och gruppens hörn, index och `elided`.
pub(super) fn concat_group(galleys: &[Arc<Galley>], mut rect: Rect, ppp: f32, last: bool, rows: &mut Vec<PlacedRow>) -> (Rect, usize, usize, bool) {
    let (mut nv, mut ni, mut el) = (0, 0, false);
    for (i, galley) in galleys.iter().enumerate() {
        let offset = rect.height();
        let is_last_galley = last && i + 1 == galleys.len();
        for (row_idx, placed_row) in galley.rows.iter().enumerate() {
            let new_pos = placed_row.pos + offset * egui::Vec2::Y;
            let new_pos = new_pos.round_to_pixels(ppp);
            rect |= Rect::from_min_size(new_pos, placed_row.size);
            let is_last_row_in_galley = row_idx + 1 == galley.rows.len();
            let mut row = Arc::clone(&placed_row.row);
            // Som `Galley::round_output_to_gui` (görs efter rektangeln).
            let rounded = row.size.round_ui();
            if row.size != rounded {
                Arc::make_mut(&mut row).size = rounded;
            }
            rows.push(PlacedRow { pos: new_pos, row, ends_with_newline: placed_row.ends_with_newline || (!is_last_galley && is_last_row_in_galley) });
        }
        nv += galley.num_vertices;
        ni += galley.num_indices;
        el |= galley.elided;
    }
    (rect, nv, ni, el)
}

/// Rektangeln som `Galley::round_output_to_gui` ger.
pub(super) fn round_rect(rect: Rect, max_width: f32) -> Rect {
    let did_exceed_wrap_width_by_a_lot = rect.width() > max_width + 1.0;
    let mut rect = rect.round_ui();
    if !did_exceed_wrap_width_by_a_lot {
        rect.max.x = rect.max.x.at_most(rect.min.x + max_width).floor_ui();
    }
    rect
}

/// Textrutornas omflöde för en rad (som `apply_pulls`): `cum` = summan av dragen till och med raden.
pub(super) struct Pulls<'a> {
    pulls: &'a [(usize, f32)],
    shifts: &'a [(Range<usize>, f32)],
    ordered: bool,
    next: usize,
    cum: f32,
    sh: usize,
    row: usize,
}

impl<'a> Pulls<'a> {
    pub fn new(pulls: &'a [(usize, f32)], shifts: &'a [(Range<usize>, f32)]) -> Self {
        Pulls { pulls, shifts, ordered: shifts.windows(2).all(|w| w[0].0.end <= w[1].0.start), next: 0, cum: 0.0, sh: 0, row: 0 }
    }

    /// Radens y efter omflödet; raderna ska komma i stigande ordning (`seek` för att hoppa).
    pub fn y(&mut self, i: usize, y: f32, ppp: f32) -> f32 {
        debug_assert!(i >= self.row);
        self.row = i;
        while self.next < self.pulls.len() && self.pulls[self.next].0 <= i {
            self.cum += self.pulls[self.next].1;
            self.next += 1;
        }
        let dy = if self.ordered {
            while self.sh < self.shifts.len() && self.shifts[self.sh].0.end <= i {
                self.sh += 1;
            }
            self.shifts.get(self.sh).filter(|(rg, _)| rg.contains(&i)).map_or(0.0, |(_, d)| *d)
        } else {
            self.shifts.iter().find(|(rg, _)| rg.contains(&i)).map_or(0.0, |(_, d)| *d)
        };
        ((y - self.cum + dy) * ppp).round() / ppp
    }
}

// ------------------------------------------------------------------ fotnoternas rader

/// Underlaget till sidindelningen (som `notes::spec`), men raden för varje referens söks bland raderna i gruppen som
/// referensen står i, i stället för att räkna tecknen i hela texten. `gstart(gi)` = tecken före grupp `gi` och dess första
/// rad, `row_counts(gi, k)` = tecken utan och med radbrytning på gruppens rad `k` (`None` efter sista).
#[allow(clippy::too_many_arguments)]
pub(super) fn spec(
    notes: &[NoteLayout],
    groups: &[memo::Group],
    gstart: &dyn Fn(usize) -> (usize, usize),
    row_counts: &dyn Fn(usize, usize) -> Option<(usize, usize)>,
    text: &str,
    a: &Analysis,
    zoom: f32,
    cap: f32,
    n_rows: usize,
) -> NoteSpec {
    let mut spec = NoteSpec { refs: Vec::new(), heights: notes.iter().map(|n| n.galley.rect.height() + px(zoom, 2.0)).collect(), sep: px(zoom, 9.0), cap };
    let mut index: HashMap<&str, usize, std::hash::BuildHasherDefault<Fx>> = HashMap::default();
    for (i, n) in notes.iter().enumerate() {
        index.entry(n.label.as_str()).or_insert(i);
    }
    if index.is_empty() || n_rows == 0 || groups.is_empty() {
        return spec;
    }
    for pc in &a.pieces {
        let Some(l) = pc.footnote_ref.as_deref() else { continue };
        let Some(&n) = index.get(l) else { continue };
        // Gruppen som tecknet står i.
        let gi = groups.partition_point(|g| g.1.start <= pc.range.start).saturating_sub(1);
        let (mut cs, mut r) = gstart(gi);
        let w0 = groups[gi].1.start.min(pc.range.start);
        let chars = cs + text[w0..pc.range.start].chars().count();
        // Första raden (i gruppen och framåt) som slutar vid eller efter tecknet (som `Galley::layout_from_cursor`).
        let (mut g, mut k) = (gi, 0);
        let row = loop {
            match row_counts(g, k) {
                Some((excl, incl)) => {
                    if cs + excl >= chars {
                        break r;
                    }
                    cs += incl;
                    r += 1;
                    k += 1;
                }
                None => {
                    g += 1;
                    k = 0;
                    if g >= groups.len() {
                        break n_rows - 1;
                    }
                }
            }
        };
        spec.refs.push((row.min(n_rows - 1), n));
    }
    spec.refs.sort_by_key(|r| r.0);
    spec
}

/// Tecknen utan och med radbrytning på rad `k` bland galleriets rader för en grupp (`galleys`), med radbrytningen som
/// `Galley::concat` lägger till på sista raden i varje galleri utom dokumentets sista (`last`).
pub(super) fn row_counts(galleys: &[Arc<Galley>], last: bool, mut k: usize) -> Option<(usize, usize)> {
    for (i, g) in galleys.iter().enumerate() {
        if k < g.rows.len() {
            let pr = &g.rows[k];
            let nl = pr.ends_with_newline || (!(last && i + 1 == galleys.len()) && k + 1 == g.rows.len());
            let n = pr.row.char_count_excluding_newline().0;
            return Some((n, n + usize::from(nl)));
        }
        k -= g.rows.len();
    }
    None
}

// ------------------------------------------------------------------ sidindelning

/// Fotnoternas plats på sidan medan raderna placeras (samma regler som `fnotes::NoteFlow`).
struct Flow<'a> {
    spec: &'a NoteSpec,
    placed: Vec<bool>,
    page: usize,
    cur: Vec<usize>,
    reserved: f32,
    /// Sidorna från `base` och framåt (sidor utan fotnoter är tomma och sista kan saknas).
    out: Vec<PageNotes>,
    base: usize,
    /// Första referensen som kan ligga på eller efter raden som frågas (raderna kommer i stigande ordning).
    lo: usize,
}

impl<'a> Flow<'a> {
    fn new(spec: &'a NoteSpec) -> Self {
        Flow { spec, placed: vec![false; spec.heights.len()], page: 0, cur: Vec::new(), reserved: 0.0, out: Vec::new(), base: 0, lo: 0 }
    }

    fn enter(&mut self, page: usize) {
        if page != self.page {
            self.page = page;
            self.cur.clear();
            self.reserved = 0.0;
        }
    }

    fn need(&mut self, rows: Range<usize>) -> (Vec<usize>, f32) {
        if self.spec.refs.is_empty() {
            return (Vec::new(), 0.0);
        }
        while self.lo < self.spec.refs.len() && self.spec.refs[self.lo].0 < rows.start {
            self.lo += 1;
        }
        let mut new: Vec<usize> = Vec::new();
        for &(row, n) in &self.spec.refs[self.lo..] {
            if row >= rows.end {
                break;
            }
            if !self.placed.get(n).copied().unwrap_or(true) && !new.contains(&n) {
                new.push(n);
            }
        }
        let mut h: f32 = new.iter().map(|&n| self.spec.heights[n]).sum();
        if self.cur.is_empty() && !new.is_empty() {
            h += self.spec.sep;
        }
        (new, h)
    }

    fn commit(&mut self, new: Vec<usize>, extra: f32) {
        if new.is_empty() {
            return;
        }
        for &n in &new {
            self.placed[n] = true;
        }
        self.cur.extend(new);
        self.reserved = (self.reserved + extra).min(self.spec.cap.max(0.0));
        let k = self.page - self.base;
        if self.out.len() <= k {
            self.out.resize(k + 1, PageNotes::default());
        }
        self.out[k] = PageNotes { notes: self.cur.clone(), reserved: self.reserved };
    }
}

/// Sidindelningens tillstånd och fasta parametrar (samma regler som `pages::paginate_notes`).
pub(super) struct Pager<'a> {
    text_h: f32,
    stride: f32,
    eps: f32,
    ppp: f32,
    floats: crate::word::pages::FloatAt,
    groups: &'a [Range<usize>],
    breaks: &'a [usize],
    flow: Flow<'a>,
    page: usize,
    shift: f32,
    prev_bottom: f32,
    float_bottom: f32,
    next_group: usize,
    /// Tillståndet före första raden på varje ny sida.
    pub snaps: Vec<PSnap>,
}

impl<'a> Pager<'a> {
    pub fn new(geom: &crate::word::pages::Geom, ppp: f32, floats: &[(usize, f32)], groups: &'a [Range<usize>], breaks: &'a [usize], spec: &'a NoteSpec) -> Self {
        Pager {
            text_h: geom.text_h(),
            stride: geom.stride(),
            eps: 0.5 / ppp,
            ppp,
            floats: crate::word::pages::FloatAt::new(floats),
            groups,
            breaks,
            flow: Flow::new(spec),
            page: 0,
            shift: 0.0,
            prev_bottom: f32::NEG_INFINITY,
            float_bottom: f32::NEG_INFINITY,
            next_group: 0,
            snaps: Vec::new(),
        }
    }

    /// Fortsätter från ett sparat tillstånd före rad `s.row`: fotnoterna med referenser före raden är redan placerade.
    /// Sidorna från `s.page` och framåt hamnar i `flow.out` (från `flow.base`).
    pub fn resume(&mut self, s: &PSnap) {
        self.page = s.page;
        self.shift = s.shift;
        self.prev_bottom = s.prev_bottom;
        self.float_bottom = s.float_bottom;
        self.flow.page = s.page;
        self.flow.cur = s.cur.clone();
        self.flow.reserved = s.reserved;
        self.flow.out.clear();
        self.flow.base = s.page;
        if !s.cur.is_empty() {
            self.flow.out.push(PageNotes { notes: s.cur.clone(), reserved: s.reserved });
        }
        self.flow.placed.iter_mut().for_each(|p| *p = false);
        for &(row, n) in &self.flow.spec.refs {
            if row >= s.row {
                break;
            }
            if let Some(p) = self.flow.placed.get_mut(n) {
                *p = true;
            }
        }
        self.flow.lo = self.flow.spec.refs.partition_point(|r| r.0 < s.row);
        self.next_group = self.groups.partition_point(|g| g.start < s.row);
        self.floats.seek(s.row);
    }

    /// Tillståndet före rad `i` (som det sparas i `snaps`).
    pub fn state(&self, i: usize) -> PSnap {
        PSnap { row: i, page: self.page, shift: self.shift, prev_bottom: self.prev_bottom, float_bottom: self.float_bottom, cur: self.flow.cur.clone(), reserved: self.flow.reserved }
    }

    /// Är tillståndet före rad `i` detsamma som `s` (utan att kopiera)?
    pub fn same(&self, s: &PSnap) -> bool {
        self.page == s.page
            && self.shift.to_bits() == s.shift.to_bits()
            && self.prev_bottom.to_bits() == s.prev_bottom.to_bits()
            && self.float_bottom.to_bits() == s.float_bottom.to_bits()
            && self.flow.cur == s.cur
            && self.flow.reserved.to_bits() == s.reserved.to_bits()
    }

    /// Slutet på gruppen av rader som börjar vid `i` (`n` = antal rader).
    pub fn unit_end(&mut self, i: usize, n: usize) -> usize {
        while self.next_group < self.groups.len() && self.groups[self.next_group].start < i {
            self.next_group += 1;
        }
        match self.groups.get(self.next_group) {
            Some(r) if r.start == i && r.end > i => r.end.min(n),
            _ => i + 1,
        }
    }

    /// Placerar raderna `i..end` (lägen före sidindelning `ys`, höjder `hs`): ger förskjutningen i y. Börjar raderna en
    /// ny sida sparas tillståndet före dem.
    pub fn place(&mut self, i: usize, end: usize, ys: &[f32], hs: &[f32]) -> f32 {
        let (mut y0, mut y1) = (f32::INFINITY, f32::NEG_INFINITY);
        for (y, h) in ys.iter().zip(hs) {
            y0 = y0.min(*y);
            y1 = y1.max(*y + *h);
        }
        let h = y1 - y0;
        let fh = self.floats.at(i);
        let top = y0 + self.shift;
        let page_top = self.page as f32 * self.stride;
        let forced = self.breaks.binary_search(&i).is_ok();
        self.flow.enter(self.page);
        let need = self.flow.need(i..end).1;
        let limit = page_top + self.text_h - self.flow.reserved - need;
        if i == 0 {
            self.snaps.push(self.state(0));
        }
        if forced && top > page_top + self.eps || top + h.max(fh) > limit + self.eps && top > page_top + self.eps {
            if self.snaps.last().is_none_or(|s| s.row != i) {
                self.snaps.push(self.state(i));
            }
            self.page += 1;
            while (self.page as f32 * self.stride) < self.prev_bottom - self.eps {
                self.page += 1;
            }
            let ppp = self.ppp;
            self.shift = ((self.page as f32 * self.stride - y0) * ppp).round() / ppp;
            self.flow.enter(self.page);
        }
        let (new, extra) = self.flow.need(i..end);
        self.flow.commit(new, extra);
        self.prev_bottom = y0 + self.shift + h;
        if fh > 0.0 {
            self.float_bottom = self.float_bottom.max(y0 + self.shift + fh);
        }
        self.shift
    }

    pub fn finish(self) -> (Vec<PageNotes>, Vec<PSnap>, f32, f32) {
        (self.flow.out, self.snaps, self.prev_bottom, self.float_bottom)
    }
}

/// Sidindelar hela galleriet `g` (som `pages::paginate_notes`) och sparar tillstånden.
pub(super) fn paginate_all(g: &mut Galley, geom: &crate::word::pages::Geom, ppp: f32, floats: &[(usize, f32)], groups: &[Range<usize>], breaks: &[usize], spec: &NoteSpec) -> (Vec<PageNotes>, Vec<PSnap>, f32, f32) {
    let mut pg = Pager::new(geom, ppp, floats, groups, breaks, spec);
    let n = g.rows.len();
    let (mut ys, mut hs) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < n {
        let end = pg.unit_end(i, n);
        ys.clear();
        hs.clear();
        for pr in &g.rows[i..end] {
            ys.push(pr.pos.y);
            hs.push(pr.row.size.y);
        }
        let shift = pg.place(i, end, &ys, &hs);
        for pr in &mut g.rows[i..end] {
            pr.pos.y += shift;
        }
        i = end;
    }
    pg.finish()
}

/// Ritytan för raderna `rows` (efter sidindelning).
pub(super) fn mesh_of(rows: &[PlacedRow]) -> Rect {
    let mut m = Rect::NOTHING;
    for pr in rows {
        m |= pr.row.visuals.mesh_bounds.translate(pr.pos.to_vec2());
    }
    m
}

/// Rektangeln för raderna `rows` (efter textrutornas omflöde).
pub(super) fn rect_of(rows: &[PlacedRow]) -> Rect {
    let mut r = Rect::NOTHING;
    for pr in rows {
        r |= Rect::from_min_size(pr.pos, pr.row.size);
    }
    r
}

/// Radernas lägen före sidindelning för de grupper som sidindelas om, räknade som i den fullständiga vägen
/// (sammanfogning och textrutornas omflöde).
pub(super) struct FlatRows<'a> {
    pub all: &'a [Arc<Galley>],
    pub gall: &'a [usize],
    pub row: &'a [usize],
    pub rect: &'a [Rect],
    pub ppp: f32,
    pub pulls: Pulls<'a>,
    pub pulls_empty: bool,
    pub n_groups: usize,
    /// Gruppen vars rader ligger i `rows` och dess första rad.
    pub cur: usize,
    pub first: usize,
    pub rows: Vec<PlacedRow>,
}

impl FlatRows<'_> {
    /// Rad `r` (raderna ska komma i stigande ordning).
    pub fn at(&mut self, r: usize) -> &PlacedRow {
        if self.cur == usize::MAX || r >= self.first + self.rows.len() || r < self.first {
            let mut gi = self.row.partition_point(|&x| x <= r).saturating_sub(1).min(self.n_groups - 1);
            while gi + 1 < self.n_groups && self.row[gi + 1] <= r {
                gi += 1;
            }
            self.rows.clear();
            concat_group(&self.all[self.gall[gi]..self.gall[gi + 1]], self.rect[gi], self.ppp, gi + 1 == self.n_groups, &mut self.rows);
            if !self.pulls_empty {
                let first = self.row[gi];
                for (k, pr) in self.rows.iter_mut().enumerate() {
                    pr.pos.y = self.pulls.y(first + k, pr.pos.y, self.ppp);
                }
            }
            self.cur = gi;
            self.first = self.row[gi];
        }
        &self.rows[r - self.first]
    }
}

/// Summan av textrutornas drag till och med rad `row` (i samma ordning som `apply_pulls`).
fn cum_at(pulls: &[(usize, f32)], row: usize) -> f32 {
    let mut cum = 0.0f32;
    for &(r, h) in pulls {
        if r > row {
            break;
        }
        cum += h;
    }
    cum
}

/// Slår ihop intervall (sorterade efter början).
fn merge(mut v: Vec<Range<usize>>) -> Vec<Range<usize>> {
    v.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in v {
        match out.last_mut() {
            Some(l) if r.start <= l.end => l.end = l.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// Sammanfogning och sidindelning för ett återupptaget varv (ett avsnitt, sidor): grupperna som inte ändrats behåller
/// sina rader från förra galleriet (`old_frame`), och sidindelningen görs om bara från sidan med den första ändrade raden
/// tills tillståndet åter är detsamma som förut. `Err(job)` om det inte går (höjden ändrades, textrutornas drag ändrades,
/// fotnoternas referenser ändrades …; med ett nummer för `LayoutStats::resume_why`); då görs allt. Ger också sidorna som
/// räknats om.
#[allow(clippy::too_many_arguments)]
pub(super) fn finish_fast(
    f: &mut Fast,
    out: &Out,
    rec: &Rec,
    groups: &[memo::Group],
    text: &str,
    a: &Analysis,
    zoom: f32,
    geom: &crate::word::pages::Geom,
    ppp: f32,
    wrap_width: f32,
    mut job: LayoutJob,
    old_frame: Option<(u64, Arc<Galley>)>,
) -> Result<(Galley, PageTrace, Range<usize>), (LayoutJob, usize)> {
    let n = groups.len();
    let Some(mut op) = f.old.page.take() else { return Err((job, 20)) };
    let Some((_, old_g)) = old_frame else { return Err((job, 21)) };
    let (n_old, n_old_rows) = (f.n_old, f.n_old_rows);
    let n_rows = rec.row[n];
    let pulls_empty = out.pulls.is_empty();
    if n == 0 || n_rows == 0 || old_g.rows.len() != n_old_rows || op.rect.len() != n_old + 1 || op.pulls.is_empty() != pulls_empty {
        return Err((job, 22));
    }
    // Sammanfogningen grupp för grupp: oförändrade sträckor ska börja med samma rektangel (och samma drag från
    // textrutorna) som förut, annars har allt efter flyttats. De ändrade gruppernas poster samlas i ordning (`p_*`) och
    // sätts in i de gamla listorna efteråt.
    let mut p_rect: Vec<Rect> = Vec::new();
    let mut p_chars: Vec<usize> = Vec::new();
    let mut p_urect: Vec<Rect> = Vec::new();
    let mut p_nv: Vec<u32> = Vec::new();
    let mut p_ni: Vec<u32> = Vec::new();
    let mut p_el: Vec<bool> = Vec::new();
    let mut dcs: Vec<isize> = Vec::with_capacity(f.runs.len());
    let mut rect = Rect::ZERO;
    let mut chars = 0usize;
    let mut dirty: Vec<Range<usize>> = Vec::new();
    let mut dirty_groups: Vec<usize> = Vec::new();
    let mut pulls = Pulls::new(&out.pulls, &out.shifts);
    let mut tmp: Vec<PlacedRow> = Vec::new();
    let mut runs = f.runs.iter().peekable();
    let mut gi = 0;
    while gi < n {
        if let Some(run) = runs.next_if(|r| r.gi == gi) {
            let j = run.gj;
            if op.rect[j] != rect || cum_at(&out.pulls, run.rows.start) != cum_at(&op.pulls, run.old_row) {
                return Err((job, 23));
            }
            let dc = chars as isize - op.chars[j] as isize;
            dcs.push(dc);
            rect = op.rect[j + run.len];
            chars = mv(op.chars[j + run.len], dc);
            gi += run.len;
            continue;
        }
        p_rect.push(rect);
        p_chars.push(chars);
        tmp.clear();
        let (r, v, i, e) = concat_group(&out.all[rec.gall[gi]..rec.gall[gi + 1]], rect, ppp, gi + 1 == n, &mut tmp);
        rect = r;
        p_nv.push(v as u32);
        p_ni.push(i as u32);
        p_el.push(e);
        let first = rec.row[gi];
        let mut u = Rect::NOTHING;
        for (k, pr) in tmp.iter_mut().enumerate() {
            chars += pr.char_count_including_newline().0;
            if !pulls_empty {
                pr.pos.y = pulls.y(first + k, pr.pos.y, ppp);
                u |= Rect::from_min_size(pr.pos, pr.row.size);
            }
        }
        if !pulls_empty {
            p_urect.push(u);
        }
        dirty_groups.push(gi);
        if rec.row[gi + 1] > first {
            dirty.push(first..rec.row[gi + 1]);
        }
        gi += 1;
    }
    let last_covers = f.runs.last().is_some_and(|r| r.gj + r.len == n_old && r.gi + r.len == n);
    if !last_covers {
        p_rect.push(rect);
        p_chars.push(chars);
    }
    let runs_v = &f.runs;
    let dc_of = |r: &Run| runs_v.iter().position(|x| x.gi == r.gi).map_or(0, |k| dcs[k]);
    let mut rect_v = std::mem::take(&mut op.rect);
    patch_dense(&mut rect_v, runs_v, n_old, n, 1, p_rect, |_, _| {}, |_| 0);
    let mut chars_v = std::mem::take(&mut op.chars);
    patch_dense(&mut chars_v, runs_v, n_old, n, 1, p_chars, |x, d| *x = mv(*x, d), dc_of);
    let mut urect_v = std::mem::take(&mut op.urect);
    if !pulls_empty {
        patch_dense(&mut urect_v, runs_v, n_old, n, 0, p_urect, |_, _| {}, |_| 0);
    }
    let mut nv_v = std::mem::take(&mut op.nv);
    patch_dense(&mut nv_v, runs_v, n_old, n, 0, p_nv, |_, _| {}, |_| 0);
    let mut ni_v = std::mem::take(&mut op.ni);
    patch_dense(&mut ni_v, runs_v, n_old, n, 0, p_ni, |_, _| {}, |_| 0);
    let mut el_v = std::mem::take(&mut op.elided);
    patch_dense(&mut el_v, runs_v, n_old, n, 0, p_el, |_, _| {}, |_| 0);
    let n_dirty = dirty_groups.len();
    let mut fmesh_v = std::mem::take(&mut op.fmesh);
    patch_dense(&mut fmesh_v, runs_v, n_old, n, 0, vec![Rect::NOTHING; n_dirty], |_, _| {}, |_| 0);
    // Fotnoterna: referenserna i oförändrade sträckor flyttas bara; de i ändrade grupper söks upp. Samma antal noter;
    // referenserna i de ändrade raderna ska vara desamma (annars ändras vilka noter som redan placerats längre fram),
    // och rader med referenser till noter som fått annan höjd sidindelas om.
    // Höjderna: bara noterna från de ändrade grupperna är nya (de andra flyttades med sina gallerier).
    let mut heights = std::mem::take(&mut op.spec.heights);
    if heights.len() != out.notes.len() {
        return Err((job, 24));
    }
    let mut changed_notes: Vec<usize> = Vec::new();
    for &g in &dirty_groups {
        let (n0, n1) = (sparse_at(&rec.cnt, g).map_or(0, |c| c.notes), sparse_at(&rec.cnt, g + 1).map_or(0, |c| c.notes));
        for i in n0..n1.min(out.notes.len()) {
            let h = out.notes[i].galley.rect.height() + px(zoom, 2.0);
            if h.to_bits() != heights[i].to_bits() {
                heights[i] = h;
                changed_notes.push(i);
            }
        }
    }
    let mut spec = NoteSpec { refs: std::mem::take(&mut op.spec.refs), heights, sep: px(zoom, 9.0), cap: geom.text_h() * 0.5 };
    if spec.sep.to_bits() != op.spec.sep.to_bits() || spec.cap.to_bits() != op.spec.cap.to_bits() {
        return Err((job, 25));
    }
    // De gamla referensernas rader räknas om till nya (de i ersatta grupper tas bort och noteras), på plats.
    let mut old_dirty_notes: Vec<usize> = Vec::new();
    {
        let mut k = 0;
        spec.refs.retain_mut(|x| {
            while k < f.runs.len() && f.runs[k].old_row + f.runs[k].rows.len() <= x.0 {
                k += 1;
            }
            match f.runs.get(k).filter(|r| r.old_row <= x.0) {
                Some(r) => {
                    x.0 = r.rows.start + (x.0 - r.old_row);
                    true
                }
                None => {
                    old_dirty_notes.push(x.1);
                    false
                }
            }
        });
    }
    // Referenserna i de ändrade grupperna söks upp.
    let mut new_refs: Vec<(usize, usize)> = Vec::new();
    if !out.notes.is_empty() {
        let mut index: Option<HashMap<&str, usize, std::hash::BuildHasherDefault<Fx>>> = None;
        let all = &out.all;
        let gall = &rec.gall;
        for &g in &dirty_groups {
            let w = &groups[g].1;
            let i0 = a.pieces.partition_point(|pc| pc.range.end <= w.start);
            for pc in a.pieces[i0..].iter().take_while(|pc| pc.range.start < w.end) {
                let Some(l) = pc.footnote_ref.as_deref() else { continue };
                if pc.range.start < w.start {
                    continue;
                }
                let index = index.get_or_insert_with(|| {
                    let mut m: HashMap<&str, usize, std::hash::BuildHasherDefault<Fx>> = HashMap::default();
                    for (i, x) in out.notes.iter().enumerate() {
                        m.entry(x.label.as_str()).or_insert(i);
                    }
                    m
                });
                let Some(&k) = index.get(l) else { continue };
                let chars0 = chars_v[g] + text[w.start..pc.range.start].chars().count();
                let (mut cs, mut r, mut gg, mut kk) = (chars_v[g], rec.row[g], g, 0);
                let row = loop {
                    match row_counts(&all[gall[gg]..gall[gg + 1]], gg + 1 == n, kk) {
                        Some((excl, incl)) => {
                            if cs + excl >= chars0 {
                                break r;
                            }
                            cs += incl;
                            r += 1;
                            kk += 1;
                        }
                        None => {
                            gg += 1;
                            kk = 0;
                            if gg >= n {
                                break n_rows - 1;
                            }
                        }
                    }
                };
                new_refs.push((row.min(n_rows - 1), k));
            }
        }
    }
    // Referenserna i de ändrade raderna ska vara desamma som förut.
    new_refs.sort_by_key(|x| x.0);
    if new_refs.iter().map(|x| x.1).ne(old_dirty_notes.iter().copied()) {
        return Err((job, 26));
    }
    if !new_refs.is_empty() {
        for x in new_refs {
            let i = spec.refs.partition_point(|y| y.0 <= x.0);
            spec.refs.insert(i, x);
        }
    }
    // Nya rader -> gamla rader för de oförändrade sträckorna.
    let to_old = |r: usize| -> Option<usize> {
        let k = f.runs.partition_point(|x| x.rows.start <= r).checked_sub(1)?;
        let x = &f.runs[k];
        (r < x.rows.end).then(|| x.old_row + (r - x.rows.start))
    };
    let mut changes = dirty.clone();
    for &k in &changed_notes {
        changes.extend(spec.refs.iter().filter(|x| x.1 == k).map(|x| x.0..x.0 + 1));
    }
    let changes = merge(changes);
    // Sidindelningen: från ett sparat tillstånd före varje ändring tills en ny sida börjar med samma tillstånd som förut.
    // Raderna däremellan ersätter de gamla (`windows`: gamla rader, nya rader).
    let mut windows: Vec<(Range<usize>, Range<usize>, Vec<PlacedRow>, Vec<PSnap>)> = Vec::new();
    let mut pout: Vec<PageNotes> = op.pout;
    let mut touched: Range<usize> = usize::MAX..0;
    let mut pager = Pager::new(geom, ppp, &out.anchors, &out.row_groups, &out.page_breaks, &spec);
    let mut flat = FlatRows {
        all: &out.all,
        gall: &rec.gall,
        row: &rec.row,
        rect: &rect_v,
        ppp,
        pulls: Pulls::new(&out.pulls, &out.shifts),
        pulls_empty,
        n_groups: n,
        cur: usize::MAX,
        first: 0,
        rows: Vec::new(),
    };
    let mut pos = 0usize;
    let mut end = op.end;
    let mut k = 0usize;
    let (mut ys, mut hs) = (Vec::new(), Vec::new());
    let mut unit: Vec<PlacedRow> = Vec::new();
    while k < changes.len() {
        let c0 = changes[k].start.max(pos);
        // Senaste sparade tillståndet i den oförändrade sträckan mellan `pos` och `c0` (före rad 0: början).
        let mut best: Option<PSnap> = None;
        if let Some(o_hi) = last_mapped(&f.runs, c0) {
            let si = op.snaps.partition_point(|x| x.row <= o_hi);
            for x in op.snaps[..si].iter().rev() {
                match to_new(&f.runs, x.row) {
                    Some(rn) if rn >= pos && rn <= c0 => {
                        best = Some(PSnap { row: rn, ..x.clone() });
                        break;
                    }
                    Some(rn) if rn < pos => break,
                    _ => {}
                }
            }
        }
        if best.is_none() && pos == 0 {
            best = Some(PSnap { row: 0, page: 0, shift: 0.0, prev_bottom: f32::NEG_INFINITY, float_bottom: f32::NEG_INFINITY, cur: Vec::new(), reserved: 0.0 });
        }
        let Some(s_new) = best else { return Err((job, 27)) };
        let rn = s_new.row;
        let Some(o_start) = (if rn == 0 { Some(0) } else { to_old(rn) }) else { return Err((job, 28)) };
        pager.snaps.clear();
        pager.resume(&s_new);
        // Tillståndet jämförs med det gamla först efter den ändring som sidindelningen återupptogs för.
        let first_end = changes[k].end;
        let mut i = rn;
        let mut conv = None;
        let mut new_rows: Vec<PlacedRow> = Vec::new();
        while i < n_rows {
            while k < changes.len() && changes[k].end <= i {
                k += 1;
            }
            let in_change = k < changes.len() && changes[k].start <= i;
            if !in_change && i >= first_end {
                if let Some(o) = to_old(i) {
                    if let Ok(j) = op.snaps.binary_search_by_key(&o, |x| x.row) {
                        if pager.same(&op.snaps[j]) {
                            conv = Some(o);
                            break;
                        }
                    }
                }
            }
            let e = pager.unit_end(i, n_rows);
            ys.clear();
            hs.clear();
            unit.clear();
            for r in i..e {
                let pr = flat.at(r);
                ys.push(pr.pos.y);
                hs.push(pr.row.size.y);
                unit.push(pr.clone());
            }
            let shift = pager.place(i, e, &ys, &hs);
            for pr in unit.drain(..) {
                new_rows.push(PlacedRow { pos: pos2(pr.pos.x, pr.pos.y + shift), ..pr });
            }
            i = e;
        }
        // Fotnoterna på de omräknade sidorna.
        let base = pager.flow.base;
        let last_page = pager.page;
        touched = touched.start.min(base)..touched.end.max(last_page + 1);
        let np = std::mem::take(&mut pager.flow.out);
        if conv.is_some() {
            if pout.len() <= last_page {
                pout.resize(last_page + 1, PageNotes::default());
            }
            for p in base..=last_page {
                pout[p] = np.get(p - base).cloned().unwrap_or_default();
            }
        } else {
            pout.truncate(base);
            pout.resize(base, PageNotes::default());
            pout.extend(np);
            end = (pager.prev_bottom, pager.float_bottom);
            touched.end = usize::MAX;
        }
        let o_end = conv.unwrap_or(n_old_rows);
        windows.push((o_start..o_end, rn..i, new_rows, std::mem::take(&mut pager.snaps)));
        match conv {
            Some(_) => pos = i,
            None => break,
        }
    }
    // De nya raderna in i det gamla galleriets rader (bakifrån, så att de gamla indexen gäller), och de nya tillstånden i
    // stället för de gamla (vars rader först räknas om till nya; de i ersatta grupper försvinner).
    // Galleriets text (`job.text` är tom): det gamla galleriets buffert återanvänds om den inte delas.
    let mut rows = match Arc::try_unwrap(old_g) {
        Ok(g) => {
            if let Ok(oj) = Arc::try_unwrap(g.job) {
                let mut t = oj.text;
                t.clear();
                t.push_str(text);
                job.text = t;
            }
            g.rows
        }
        Err(g) => g.rows.clone(),
    };
    if job.text.len() != text.len() {
        job.text = text.to_owned();
    }
    for (old, _, new_rows, _) in windows.iter_mut().rev() {
        rows.splice(old.clone(), std::mem::take(new_rows));
    }
    if rows.len() != n_rows {
        return Err((job, 29));
    }
    let mut snaps: Vec<PSnap> = op.snaps;
    {
        let mut k = 0;
        snaps.retain_mut(|x| {
            while k < f.runs.len() && f.runs[k].old_row + f.runs[k].rows.len() <= x.row {
                k += 1;
            }
            match f.runs.get(k).filter(|r| r.old_row <= x.row) {
                Some(r) => {
                    x.row = r.rows.start + (x.row - r.old_row);
                    true
                }
                None => false,
            }
        });
    }
    for (_, new, _, new_snaps) in windows.iter_mut().rev() {
        let (s0, s1) = (snaps.partition_point(|x| x.row < new.start), snaps.partition_point(|x| x.row < new.end));
        snaps.splice(s0..s1, std::mem::take(new_snaps));
    }
    // Ritytor för grupper med omräknade rader.
    for (_, new, _, _) in &windows {
        let mut g = rec.row.partition_point(|&x| x <= new.start).saturating_sub(1);
        while g < n && rec.row[g] < new.end {
            fmesh_v[g] = mesh_of(&rows[rec.row[g]..rec.row[g + 1]]);
            g += 1;
        }
    }
    for &g in &dirty_groups {
        fmesh_v[g] = mesh_of(&rows[rec.row[g]..rec.row[g + 1]]);
    }
    // Summorna: om för de block av grupper som ändrats (alla efter den första ändringen om antalet grupper ändrats).
    let mut sums = std::mem::take(&mut op.sums);
    let first_changed = dirty_groups.first().copied().unwrap_or(n).min(windows.iter().map(|w| rec.row.partition_point(|&x| x <= w.1.start).saturating_sub(1)).min().unwrap_or(n));
    let n_chunks = n.div_ceil(CHUNK);
    let mut redo: Vec<usize> = Vec::new();
    if n != n_old || sums.len() != n_old.div_ceil(CHUNK) {
        sums.truncate((first_changed / CHUNK).min(sums.len()));
        redo.extend(sums.len()..n_chunks);
    } else {
        redo.extend(dirty_groups.iter().map(|g| g / CHUNK));
        for (_, new, _, _) in &windows {
            let g0 = rec.row.partition_point(|&x| x <= new.start).saturating_sub(1);
            let g1 = rec.row.partition_point(|&x| x < new.end).min(n);
            redo.extend(g0 / CHUNK..=(g1.saturating_sub(1) / CHUNK).max(g0 / CHUNK));
        }
        redo.sort_unstable();
        redo.dedup();
    }
    sums.resize(n_chunks, Sum { nv: 0, ni: 0, el: false, mesh: Rect::NOTHING, urect: Rect::NOTHING });
    for k in redo {
        if k < n_chunks {
            sums[k] = sum_of(&nv_v, &ni_v, &el_v, &fmesh_v, &urect_v, k * CHUNK..((k + 1) * CHUNK).min(n));
        }
    }
    let (mut nv, mut ni, mut el) = (0usize, 0usize, false);
    let mut mesh = Rect::NOTHING;
    let mut u = Rect::ZERO;
    for x in &sums {
        nv += x.nv;
        ni += x.ni;
        el |= x.el;
        mesh |= x.mesh;
        u |= x.urect;
    }
    let mut r = if pulls_empty { round_rect(rect_v[n], wrap_width) } else { u };
    r.max.y = r.max.y.max(end.0).max(end.1);
    let g = galley_from(job, rows, r, mesh, nv, ni, el, ppp);
    let pt = PageTrace { rect: rect_v, chars: chars_v, urect: urect_v, fmesh: fmesh_v, nv: nv_v, ni: ni_v, elided: el_v, snaps, pout, end, spec, pulls: out.pulls.clone(), sums };
    Ok((g, pt, touched))
}

/// Gammal rad för den sista nya raden till och med `r` som ligger i en oförändrad sträcka.
fn last_mapped(runs: &[Run], r: usize) -> Option<usize> {
    let k = runs.partition_point(|x| x.rows.start <= r).checked_sub(1)?;
    let x = &runs[k];
    if r < x.rows.end {
        Some(x.old_row + (r - x.rows.start))
    } else if x.rows.end > x.rows.start {
        Some(x.old_row + x.rows.len() - 1)
    } else {
        None
    }
}

/// Ny rad för gammal rad `o` i en oförändrad sträcka.
fn to_new(runs: &[Run], o: usize) -> Option<usize> {
    runs.iter().find(|x| o >= x.old_row && o < x.old_row + x.rows.len()).map(|x| x.rows.start + (o - x.old_row))
}

/// Bygger ett galleri av färdiga rader och delarna som `Galley::concat` annars räknar fram.
pub(super) fn galley_from(job: LayoutJob, rows: Vec<PlacedRow>, rect: Rect, mesh: Rect, nv: usize, ni: usize, elided: bool, ppp: f32) -> Galley {
    let mut g = Galley::concat(Arc::new(job), &[], ppp);
    g.rows = rows;
    g.rect = rect;
    g.mesh_bounds = mesh;
    g.num_vertices = nv;
    g.num_indices = ni;
    g.elided = elided;
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::bench::Rng;
    use crate::word::{incr, pages};

    fn fam(_: Option<&str>, bold: bool, italic: bool) -> FontFamily {
        theme::doc_font(bold, italic)
    }

    /// Ett dokument med det som påverkar utläggningens tillstånd: flytande bilder, textrutor, fotnoter (ett block sist,
    /// som Word-läsaren skriver dem), tabeller, sidbrytningar, tomma stycken, rubriker och ibland avsnitt med spalter.
    fn doc(r: &mut Rng, paras: usize, sections: bool) -> String {
        let words = ["alfa", "beta", "gamma", "delta", "åäö", "text", "rad", "stycke", "dokument", "sida", "ord"];
        let mut s = String::from("<!-- kladd: sida=a5 -->\n\n");
        let mut notes = 0;
        for i in 0..paras {
            let mut line = String::new();
            for _ in 0..(3 + r.below(60)) {
                let w = words[r.below(words.len())];
                match r.below(16) {
                    0 => line.push_str(&format!("**{w}** ")),
                    1 => line.push_str(&format!("*{w}* ")),
                    _ => line.push_str(&format!("{w} ")),
                }
            }
            match r.below(22) {
                0 => s.push_str(&format!("## Rubrik {i}")),
                1 => s.push_str("- punkt ett\n- punkt två med lite mer text\n- punkt tre"),
                2 => s.push_str("| a | b |\n| --- | --- |\n| 1 | 2 och mer text i cellen |\n| 3 | 4 |"),
                3 => s.push_str("<!-- kladd: sidbrytning -->"),
                4 => s.push_str(&format!("<img src=\"a.png\" alt=\"\" width=\"100\" align=\"left\">{line}")),
                5 => s.push_str(&format!("{line}<img src=\"b.png\" alt=\"\" width=\"80\" align=\"right\">")),
                6 => s.push_str(&format!("<!--k:tb 1 w=\"157.5\" h=\"90\" hrel=\"page\" x=\"400\" y=\"0\"-->\n\n{line}\n\n<!--k:/tb-->")),
                7 | 8 | 9 => {
                    notes += 1;
                    s.push_str(&format!("{line}[^{notes}] och mer"));
                }
                10 if sections && r.below(4) == 0 => s.push_str("<!-- kladd: avsnitt; columns: 2; type: continuous -->"),
                11 => s.push_str("<!--k:ep-->"),
                _ => s.push_str(&line),
            }
            s.push_str("\n\n");
        }
        for n in 1..=notes {
            s.push_str(&format!("[^{n}]: Fotnot {n} med *text* och mer text"));
            if r.below(5) == 0 {
                s.push_str(&" lång".repeat(20));
            }
            s.push('\n');
        }
        s
    }

    /// En ändring som när man skriver: oftast ett tecken i ett ord, ibland en radbrytning, ett nytt stycke eller en
    /// borttagning, i två fall av fem i fotnotsdefinitionerna. Ger var markören står efteråt.
    fn edit(r: &mut Rng, text: &mut String) -> usize {
        let from = match text.find("\n[^1]:").filter(|_| r.below(5) < 2) {
            Some(f) => f,
            None => 0,
        };
        let mut at = from + r.below(text.len() - from);
        while !text.is_char_boundary(at) {
            at += 1;
        }
        match r.below(20) {
            0 => text.insert_str(at, "\n\n"),
            1 => text.insert(at, '\n'),
            2 | 3 => {
                if let Some(c) = text[at..].chars().next() {
                    text.replace_range(at..at + c.len_utf8(), "");
                }
            }
            4 => text.insert_str(at, " ett par ord till som gör raden längre "),
            5 => text.insert_str(at, "**"),
            _ => text.insert(at, 'x'),
        }
        at.min(text.len())
    }

    #[derive(Debug, PartialEq)]
    struct Shape {
        rows: Vec<(u32, u32, usize, bool, u32)>,
        rect: [u32; 4],
        mesh: [u32; 4],
        nv: usize,
        ni: usize,
        boxes: Vec<(u32, Vec<usize>, u32)>,
        notes: Vec<(String, Range<usize>, Range<usize>)>,
        geoms: Vec<Range<usize>>,
    }

    fn rbits(r: Rect) -> [u32; 4] {
        [r.min.x.to_bits(), r.min.y.to_bits(), r.max.x.to_bits(), r.max.y.to_bits()]
    }

    fn shape(c: &mut Cache, g: &Galley, text: &str) -> Shape {
        Shape {
            rows: g.rows.iter().map(|r| (r.pos.x.to_bits(), r.pos.y.to_bits(), r.char_count_including_newline().0, r.ends_with_newline, r.row.size.y.to_bits())).collect(),
            rect: rbits(g.rect),
            mesh: rbits(g.mesh_bounds),
            nv: g.num_vertices,
            ni: g.num_indices,
            boxes: c.page_boxes(1, text).iter().map(|b| (b.top.to_bits(), b.notes.clone(), b.reserved.to_bits())).collect(),
            notes: c.notes(1, text).iter().map(|n| (n.label.clone(), n.line.clone(), n.body.clone())).collect(),
            geoms: c.docs.get(&1).map(|d| d.geoms.iter().map(|t| t.region.clone()).collect()).unwrap_or_default(),
        }
    }

    /// Slumpade ändringar: galleriet, sidorna och fotnoterna ska bli exakt desamma som när hela varvet görs (en cache
    /// utan återupptagning), och ibland som i en helt ny cache. Ger hur många gånger sidindelningen återupptogs.
    fn resumed_equals_full(seed: u64, paras: usize, steps: usize, sections: bool) -> (usize, usize) {
        incr::MIN_OVERRIDE.with(|m| m.set(Some(1000)));
        let pal = theme::palette(false);
        let img = |k: &str| Some(if k.contains("b.png") { egui::vec2(80.0, 60.0) } else { egui::vec2(100.0, 120.0) });
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut r = Rng(seed);
        let mut text = doc(&mut r, paras, sections);
        let (mut c, mut control) = (Cache::default(), Cache::default());
        let (mut resumed, mut gave_up) = (0, 0);
        let mut at = 0;
        for step in 0..steps {
            if step > 0 {
                at = edit(&mut r, &mut text);
            }
            let reveal = match r.below(4) {
                0 => 0..0,
                _ => crate::word::tables::reveal_range(&text, at..at),
            };
            let a0 = markup::analyze(&text);
            let geom = pages::Geom::for_sections(1.0, &a0.defaults, &a0.sections);
            let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, egui::Color32::WHITE).size().x);
            let p = Params { palette: &pal, zoom: 1.0, reveal, image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
            let out = ctx.run_ui(Default::default(), |_| {
                let g = c.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                resumed += usize::from(c.stats(1).resumed);
                gave_up += usize::from(c.stats(1).resume_why == 30);
                let got = shape(&mut c, &g, &text);
                OFF.with(|o| o.set(true));
                let f = control.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                OFF.with(|o| o.set(false));
                let want = shape(&mut control, &f, &text);
                assert_eq!(got.rows.len(), want.rows.len(), "seed {seed} steg {step}: antal rader");
                if let Some(i) = (0..got.rows.len()).find(|&i| got.rows[i] != want.rows[i]) {
                    panic!("seed {seed} steg {step}: rad {i} av {}: {:?} != {:?}", got.rows.len(), got.rows[i], want.rows[i]);
                }
                assert_eq!(got, want, "seed {seed} steg {step}");
                if step % 8 == 7 {
                    let mut fresh = Cache::default();
                    let h = fresh.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                    assert_eq!(got, shape(&mut fresh, &h, &text), "seed {seed} steg {step}: ny cache");
                }
            });
            std::mem::forget(out);
        }
        incr::MIN_OVERRIDE.with(|m| m.set(None));
        (resumed, gave_up)
    }

    #[test]
    fn resumed_layout_equals_full_layout() {
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).map_or(3, |n| (n / 10).max(3));
        let (mut resumed, mut gave_up) = (0, 0);
        for seed in 0..seeds {
            let (r, g) = resumed_equals_full(9001 + seed * 7, 140, 30, false);
            resumed += r;
            gave_up += g;
        }
        assert!(resumed > seeds as usize * 5, "återupptogs bara {resumed} gånger");
        // Ändringar som ändrar höjden ger upp det återupptagna varvet (`Fast::abandon`): den vägen provas också.
        assert!(gave_up > 0, "inget varv gavs upp");
    }

    #[test]
    fn resumed_layout_with_sections_equals_full_layout() {
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).map_or(2, |n| (n / 15).max(2));
        for seed in 0..seeds {
            resumed_equals_full(31337 + seed * 13, 120, 24, true);
        }
    }
}
