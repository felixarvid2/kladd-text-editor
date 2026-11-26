//! Återanvändning mellan omläggningar i stora dokument: styckena (`paragraphs`), fotnotsplanen, grupperna och
//! cachenycklarnas statiska del tar tid i proportion till dokumentet, men efter en liten ändring (inkrementell
//! analys, se `incr`) ändras bara stycken nära ändringen och nära markören. `prepare` bygger om bara dessa
//! fönster (`paragraphs_in`) och behåller resten från förra omläggningen; resultatet ska vara detsamma som en
//! full omräkning (tester i `memo_tests`).
//!
//! Fönstren görs större än det som ändrats: ett par innehållsstycken på var sida är kontrollstycken som räknas
//! om och jämförs med de gamla (får de annan luft, justering eller radbrytning har ändringen påverkat något längre
//! bort och allt räknas om), och ett ytterligare stycke längst ut räknas om bara för att kanterna ska bli rätt.

use super::*;

/// Det en omläggning behöver före utläggningen av grupperna.
pub(super) struct Prep {
    pub paras: Vec<Para>,
    pub defs: Vec<notes::Def>,
    pub groups: Vec<Group>,
    /// Statisk del av cachenyckeln (`unit_key_static`) för varje grupp.
    pub skeys: Vec<u64>,
    /// Antal poster i innehållsförteckningen om texten har en (styckenas höjd beror på det).
    pub toc_n: Option<usize>,
    /// Fotnotsplanen gjordes (sidindelat dokument), och det `notes::plan` lämnade efter sig utöver definitionerna:
    /// etiketterna med referens, stycket som fick luft över sig och om styckena efter sista definitionen krympts.
    pub planned: bool,
    pub refs: BTreeSet<String>,
    pub gap: Option<usize>,
    pub trailing: bool,
    /// För varje grupp den grupp i förra omläggningen som den är oförändrad från (`u32::MAX` = ändrad), om
    /// förberedelsen återanvändes från förra omläggningen (`from_rev`); annars tom. Se `resume`.
    pub from: Vec<u32>,
    pub from_rev: u64,
    /// Första gruppen `i` där `from[i] != i` (eller antalet grupper), så att den inte behöver sökas.
    pub first_new: usize,
    /// Styckena som öppnar eller stänger en textruta (index, stigande): fönstren vidgas till hela rutor.
    pub marks: Vec<usize>,
}

fn marks_of(paras: &[Para]) -> Vec<usize> {
    (0..paras.len()).filter(|&i| paras[i].tbox.is_some() || paras[i].tbox_end).collect()
}

pub(super) type Group = (Range<usize>, Range<usize>, Option<usize>);

/// Det förra omläggningen lämnade efter sig, för texten som fanns då (`DocState::memo_text`).
pub(super) struct Memo {
    /// Allt utom texten som resultatet gäller för (parametrar, typsnitt, sidgeometri, bildstorlekar).
    pub sig: u64,
    /// `DocState::rev` då det byggdes.
    pub rev: u64,
    pub reveal: Range<usize>,
    pub prep: Prep,
}

// Varför `reuse` avstod (nummer i källan; bara för mätning och prov).
#[cfg(test)]
thread_local! {
    pub static WHY: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn no<T>(_why: usize) -> Option<T> {
    #[cfg(test)]
    WHY.with(|w| w.borrow_mut().push(_why));
    None
}

/// Hur många innehållsstycken (inte tomma eller dolda rader) på var sida om det ändrade som räknas om och jämförs.
const CHECK: usize = 3;
/// Fönstret får inte bli större än så här många stycken: då lönar det sig inte.
const MAX_PARAS: usize = 600;
/// Så många fönster (ändring, markörens gamla och nya rader) hanteras; fler ger full omräkning.
const MAX_REGIONS: usize = 3;

/// Slår ihop det ändrade området (i nuvarande text) med fönstret (`ws..we_new`, `we_old` i texten före) från
/// en inkrementell analys: ett område som täcker alla ändringar sedan förra omläggningen.
pub(super) fn compose_hull(hull: Option<(usize, usize)>, ws: usize, we_old: usize, we_new: usize) -> (usize, usize) {
    let Some((hs, he)) = hull else { return (ws, we_new) };
    let delta = we_new as isize - we_old as isize;
    let m = |x: usize| if x >= we_old { (x as isize + delta) as usize } else { x };
    (m(hs).min(ws), m(he).max(we_new))
}

/// Kan intervallet röra något stycke? (`0..0` är "ingen markör"; ett tomt intervall mitt i en rad räknas ändå, se `intersects`.)
fn shows(r: &Range<usize>) -> bool {
    r.start != 0 || r.end != 0
}

fn content(q: &Para) -> bool {
    !q.collapse && !q.range.is_empty()
}

/// Fotnotsetiketterna i `[^etikett]` i `w`: ändras de har definitionsplanen (`notes::plan`) ändrats.
fn note_refs(w: &str) -> Vec<&str> {
    w.match_indices("[^").map(|(i, _)| w[i + 2..].split(']').next().unwrap_or("")).collect()
}

/// Finns en kommentar eller markör (`<!--…`) som inte avslutas på samma rad? `pbox::scan` hoppar då över allt fram till
/// nästa `-->`, och en oavslutad HTML-kommentar sväljer text långt efter den (bitarna ändras utanför fönstret), vilket
/// ett fönster inte ser. (I appen ger det redan full analys, `incr::Facts`; här gäller det direkta användningar.)
fn broken_marker(w: &str) -> bool {
    w.match_indices("<!--").any(|(i, _)| !w[i..].split('\n').next().unwrap_or("").contains("-->"))
}

/// Är styckena exakt lika (talen jämförs bit för bit, som om de skrivits ut)? Används för kontrollstyckena.
fn same_para(a: &Para, b: &Para) -> bool {
    let f = |x: f32, y: f32| x.to_bits() == y.to_bits();
    let of = |x: Option<f32>, y: Option<f32>| x.map(f32::to_bits) == y.map(f32::to_bits);
    a.range == b.range
        && a.align == b.align
        && match (a.list, b.list) {
            (Some(x), Some(y)) => f(x.text_x, y.text_x) && x.marker_chars == y.marker_chars,
            (None, None) => true,
            _ => false,
        }
        && f(a.image_w, b.image_w)
        && a.collapse == b.collapse
        && match (a.float, b.float) {
            (Some(x), Some(y)) => x.side == y.side && x.anchor == y.anchor && f(x.size.x, y.size.x) && f(x.size.y, y.size.y) && x.alone == y.alone,
            (None, None) => true,
            _ => false,
        }
        && of(a.reserve, b.reserve)
        && f(a.toc_h, b.toc_h)
        && f(a.space_before, b.space_before)
        && f(a.first_indent, b.first_indent)
        && a.col_break == b.col_break
        && a.page_break == b.page_break
        && match (&a.look, &b.look) {
            (Some(x), Some(y)) => Arc::ptr_eq(x, y) || x == y,
            (None, None) => true,
            _ => false,
        }
        && of(a.gap, b.gap)
        && a.line == b.line
        && f(a.ind_left, b.ind_left)
        && f(a.ind_right, b.ind_right)
        && f(a.hang, b.hang)
        && f(a.ppx, b.ppx)
        && f(a.room.0, b.room.0)
        && f(a.room.1, b.room.1)
        && of(a.empty_h, b.empty_h)
        && a.tbox == b.tbox
        && a.tbox_end == b.tbox_end
}

fn toc_count(text: &str, a: &Analysis) -> usize {
    crate::word::nav::toc_total(text, &crate::word::nav::headings(text, a))
}

/// Bygger allt från grunden.
pub(super) fn fresh(text: &str, a: &Analysis, p: &Params, planned: bool, grid: &[Table], cb: &[codeblock::Block]) -> Prep {
    let mut paras = paragraphs_in(text, a, p, None, cb);
    let plan = if planned { Some(notes::plan(text, a, &mut paras, p.zoom)) } else { None };
    let groups = group_paras(text, &paras, grid);
    let skeys = groups.iter().map(|g| unit_key_static(text, a, p, &g.1)).collect();
    let toc_n = paras.iter().any(|q| q.toc_h > 0.0).then(|| toc_count(text, a));
    let (defs, refs, gap, trailing) = match plan {
        Some(pl) => (pl.defs, pl.refs, pl.gap, pl.trailing),
        None => (Vec::new(), BTreeSet::new(), None, false),
    };
    let marks = marks_of(&paras);
    Prep { paras, defs, groups, skeys, toc_n, planned, refs, gap, trailing, from: Vec::new(), from_rev: 0, first_new: 0, marks }
}

/// Styckena efter en omräkning av fönstret `lo..=hi` (gamla index) som gav `w`: stycken före fönstret, fönstret och
/// (flyttade) stycken efter det, utan att något ändras. Nya index.
struct Virt<'a> {
    old: &'a [Para],
    w: &'a [Para],
    lo: usize,
    hi: usize,
}

impl Virt<'_> {
    fn len(&self) -> usize {
        self.old.len() - (self.hi - self.lo + 1) + self.w.len()
    }
    fn get(&self, k: usize) -> Option<&Para> {
        if k < self.lo {
            self.old.get(k)
        } else if k < self.lo + self.w.len() {
            self.w.get(k - self.lo)
        } else {
            self.old.get(k - self.w.len() + self.hi - self.lo + 1)
        }
    }
    /// Nytt index för gammalt stycke `i` till och med `lo` eller från och med `hi` (fönstrets kanter räknas om men
    /// motsvarar samma stycken).
    fn map(&self, i: usize) -> usize {
        if i <= self.lo { i } else { i + self.w.len() - (self.hi - self.lo + 1) }
    }
}

/// Fotnotsplanen (`notes::plan`) i ett omräknat fönster: definitionerna i `w` och det planen gör med styckena runt dem
/// (tomma stycken före en definition krymps, luften över första slutnoten, styckena efter sista definitionen). Planen i
/// övrigt gäller oförändrad: definitionerna i fönstret ska vara samma (etiketter i samma ordning) som förut, annars
/// `None` (full omräkning). `w` (stycken `lo..=hi` i gamla index, `used` = de som sätts in) får planens ändringar och
/// resultatet är de nya definitionerna, luftstycket och om slutet är krympt.
#[allow(clippy::too_many_arguments)]
fn plan_window(
    prep: &Prep,
    w: &mut [Para],
    lo: usize,
    hi: usize,
    used: Range<usize>,
    text: &str,
    a: &Analysis,
    zoom: f32,
    old_line: &dyn Fn(usize) -> usize,
) -> Option<(Vec<notes::Def>, Option<usize>, bool)> {
    let np_old = prep.paras.len();
    // Definitionerna i fönstret (utom sista stycket, vars granne efter ligger utanför och är oförändrad) jämförs med de gamla.
    let mut cand: Vec<notes::Def> = Vec::new();
    for j in 0..w.len().saturating_sub(1) {
        for (l, body) in notes::defs_of(text, a, &w[j], w.get(j + 1), &prep.refs) {
            cand.push(notes::Def { label: l.to_owned(), para: lo + j, line: w[j].range.clone(), body });
        }
    }
    let span_end = if hi + 1 == np_old { hi + 1 } else { hi };
    if hi + 1 == np_old {
        // Fönstret når dokumentets slut: sista stycket har ingen granne.
        if let Some(q) = w.last() {
            for (l, body) in notes::defs_of(text, a, q, None, &prep.refs) {
                cand.push(notes::Def { label: l.to_owned(), para: lo + w.len() - 1, line: q.range.clone(), body });
            }
        }
    }
    let i0 = prep.defs.partition_point(|d| d.para < lo);
    let i1 = prep.defs.partition_point(|d| d.para < span_end);
    if prep.defs[i0..i1].iter().map(|d| d.label.as_str()).ne(cand.iter().map(|d| d.label.as_str())) {
        return no(20);
    }
    let v = Virt { old: &prep.paras, w, lo, hi };
    let dn = v.len() as isize - np_old as isize;
    let mut defs: Vec<notes::Def> = prep.defs[..i0].to_vec();
    defs.extend(cand);
    for d in &prep.defs[i1..] {
        let mut d = d.clone();
        d.para = v.map(d.para);
        d.body = old_line(d.body);
        d.line = old_line(d.line.start)..old_line(d.line.end);
        defs.push(d);
    }
    let is_def = |k: usize| defs.binary_search_by(|d| d.para.cmp(&k)).is_ok();
    let last_old = prep.defs.last().map(|d| d.para);
    let last = defs.last().map(|d| d.para);
    // Tomma stycken vars nästa icke-tomma stycke är en definition krymps.
    let mut collapse: Vec<usize> = Vec::new();
    for j in 0..v.w.len() {
        if !v.w[j].range.is_empty() {
            continue;
        }
        let mut k = lo + j + 1;
        while v.get(k).is_some_and(|q| q.range.is_empty()) {
            k += 1;
            if k - (lo + j) > 4096 {
                return no(21);
            }
        }
        if v.get(k).is_some() && is_def(k) {
            collapse.push(j);
        }
    }
    // Luften över första slutnoten efter sista definitionen, och de krympta styckena i slutet: räknas om när fönstret
    // ligger efter sista definitionen (fram till luftstycket).
    let (mut gap, mut trailing) = (prep.gap.map(|g| v.map(g)), prep.trailing);
    let after_last = last_old.is_some_and(|l| hi > l) && prep.gap.is_none_or(|g| lo <= g);
    if after_last {
        let Some(last) = last else { return no(22) };
        let mut k = last + 1;
        let mut found = None;
        while let Some(q) = v.get(k) {
            if notes::gap_candidate(text, q) {
                found = Some(k);
                break;
            }
            k += 1;
            if k - last > 20_000 {
                return no(23);
            }
        }
        // Utanför de insatta styckena ska luftstycket vara detsamma (det har redan luften, eller saknar den).
        let inside = |k: usize| k >= lo + used.start && k < lo + used.end;
        let lo_t = lo + used.start;
        let hi_t = hi - (v.w.len() - used.end);
        let old_out = prep.gap.filter(|&g| g < lo_t || g > hi_t).map(|g| if g < lo_t { g } else { (g as isize + dn) as usize });
        let new_out = found.filter(|&k| !inside(k));
        if old_out != new_out {
            return no(24);
        }
        gap = found;
        let mut all_empty = true;
        let mut k = last + 1;
        while let Some(q) = v.get(k) {
            if !q.range.is_empty() {
                all_empty = false;
                break;
            }
            k += 1;
        }
        if all_empty != prep.trailing {
            return no(25);
        }
        trailing = all_empty;
    }
    let w_len = w.len();
    for j in collapse {
        w[j].collapse = true;
    }
    if after_last {
        if let Some(g) = gap.filter(|&g| g >= lo && g < lo + w_len) {
            w[g - lo].space_before += notes::gap_space(zoom);
        }
        if let (true, Some(last)) = (trailing, last) {
            for (j, q) in w.iter_mut().enumerate() {
                if lo + j > last {
                    q.collapse = true;
                }
            }
        }
    }
    Some((defs, gap, trailing))
}

/// Grupperna efter att fönstren `spliced` (gamla index: första stycket, antal gamla och nya stycken, i stigande ordning)
/// satts in: grupperna utanför fönstren flyttas bara (index och byte efter ändringen `edit` = gammalt slut, förskjutning)
/// och behåller sina nycklar; grupperna som rör fönstren görs om. `None` (och grupperna orörda) om en tabell sträcker sig
/// över en gräns, då görs alla grupper om.
#[allow(clippy::too_many_arguments)]
fn regroup(prep: &mut Prep, spliced: &[(usize, usize, usize)], text: &str, a: &Analysis, p: &Params, grid: &[Table], edit: Option<(usize, isize)>) -> Option<()> {
    let paras = &prep.paras;
    // Ingen tabell får börja före stycket `i` och fortsätta in i det (då hör det till gruppen före).
    let open_table = |i: usize| -> bool {
        let Some(q) = paras.get(i) else { return false };
        let x = q.range.start;
        let k = grid.partition_point(|t| t.region.end <= x);
        grid.get(k).is_some_and(|t| t.region.start < x)
    };
    let old = &prep.groups;
    // Gamla grupper som rör varje fönster, och de nya styckena de motsvarar.
    let mut segs: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut shift: isize = 0;
    for &(lo, old_n, new_n) in spliced {
        let g0 = old.partition_point(|g| g.0.end <= lo);
        let g1 = old.partition_point(|g| g.0.start < lo + old_n);
        if g0 >= g1 {
            return None;
        }
        let p0 = (old[g0].0.start as isize + shift) as usize;
        shift += new_n as isize - old_n as isize;
        let p1 = (old[g1 - 1].0.end as isize + shift) as usize;
        if segs.last().is_some_and(|s| s.1 > g0) || open_table(p0) || open_table(p1) {
            return None;
        }
        segs.push((g0, g1, p0, p1));
    }
    let mv_b = |x: usize| match edit {
        Some((he_old, d)) if x >= he_old => (x as isize + d) as usize,
        _ => x,
    };
    // Grupperna efter varje fönster flyttas på plats (styckeindex, byte och tabellindex), sedan sätts de omgjorda
    // grupperna in bakifrån.
    let mut groups = std::mem::take(&mut prep.groups);
    let mut skeys = std::mem::take(&mut prep.skeys);
    let mut from = std::mem::take(&mut prep.from);
    from.clear();
    from.extend(0..groups.len() as u32);
    let mut shift: isize = 0;
    for (k, &(_, g1, _, _)) in segs.iter().enumerate() {
        shift += spliced[k].2 as isize - spliced[k].1 as isize;
        let end = segs.get(k + 1).map_or(groups.len(), |s| s.0);
        for (pr, w, t) in &mut groups[g1..end] {
            *pr = (pr.start as isize + shift) as usize..(pr.end as isize + shift) as usize;
            *w = mv_b(w.start)..mv_b(w.end);
            // Tabellens index kan ha ändrats om en tabell tillkommit eller försvunnit i ett fönster.
            if t.is_some() {
                *t = Some(grid.partition_point(|x| x.region.end <= w.start));
            }
        }
    }
    for &(g0, g1, p0, p1) in segs.iter().rev() {
        let made: Vec<Group> = group_paras(text, &prep.paras[p0..p1], grid).into_iter().map(|(pr, w, t)| (pr.start + p0..pr.end + p0, w, t)).collect();
        let keys: Vec<u64> = made.iter().map(|g| unit_key_static(text, a, p, &g.1)).collect();
        from.splice(g0..g1, std::iter::repeat_n(u32::MAX, made.len()));
        skeys.splice(g0..g1, keys);
        groups.splice(g0..g1, made);
    }
    prep.first_new = segs.first().map_or(groups.len(), |s| s.0);
    prep.groups = groups;
    prep.skeys = skeys;
    prep.from = from;
    Some(())
}

/// Ett fönster av stycken (index i de gamla styckena) som ska räknas om.
struct Region {
    /// Ändrade stycken, och fönstret med kontrollstycken (`lo..=hi`; `lo` och `hi` räknas om men används inte).
    a: usize,
    b: usize,
    lo: usize,
    hi: usize,
    edit: bool,
}

/// Återanvänder `m` för den nya texten, eller `None` om det inte går att göra säkert (då byggs allt om).
/// `old_text` är texten som `m` gäller för och `hull` området som ändrats sedan dess (nuvarande text).
#[allow(clippy::too_many_arguments)]
pub(super) fn reuse(
    m: Memo,
    old_text: &str,
    hull: Option<(usize, usize)>,
    rev: u64,
    text: &str,
    a: &Analysis,
    p: &Params,
    sig: u64,
    grid: &[Table],
    cb: &[codeblock::Block],
    toc_delta: Option<isize>,
    composed: bool,
) -> Option<Prep> {
    if m.sig != sig {
        return no(1);
    }
    let mut prep = m.prep;
    let changed = m.rev != rev;
    prep.from_rev = m.rev;
    if !changed && p.reveal == m.reveal {
        prep.from = (0..prep.groups.len() as u32).collect();
        prep.first_new = prep.groups.len();
        return Some(prep);
    }
    let delta = text.len() as isize - old_text.len() as isize;
    // Ändringen i den gamla (`hs..he_old`) och nya (`hs..he_new`) texten.
    let edit: Option<(usize, usize, usize)> = if changed {
        let (hs, he_new) = hull.or_else(|| { no::<()>(90); None })?;
        let he_old = (he_new as isize - delta).try_into().ok()?;
        if hs > he_new || he_new > text.len() || hs > he_old || he_old > old_text.len() {
            return no(2);
        }
        // Kontroll: utanför området är texten verkligen densamma. Ett område från en enda inkrementell analys (som själv
        // jämför texterna) kontrolleras bara i debugbyggen, och där är en avvikelse ett fel; ett område som satts samman av
        // flera ändringar sedan förra omläggningen (ovanligt) kontrolleras alltid.
        #[cfg(debug_assertions)]
        let checks = CHECKS.load(std::sync::atomic::Ordering::Relaxed);
        #[cfg(not(debug_assertions))]
        let checks = false;
        if (composed || checks) && (old_text.as_bytes()[..hs] != text.as_bytes()[..hs] || old_text.as_bytes()[he_old..] != text.as_bytes()[he_new..]) {
            assert!(!checks, "texten utanför det ändrade området {hs}..{he_new} har ändrats");
            return no(3);
        }
        Some((hs, he_old, he_new))
    } else {
        None
    };
    if prep.paras.is_empty() {
        return no(4);
    }
    // Områden (i gamla texten) som ska räknas om: ändringen, markörens gamla och nya rader. En bit som är med i
    // markörens rader (`intersects(pc.range, reveal)` ingår i nyckeln för hela biten) kan sträcka sig över flera
    // stycken: områdena vidgas till de bitarna.
    let to_new = |x: usize, end: bool| -> usize {
        match edit {
            Some((hs, he_old, he_new)) => {
                if x <= hs {
                    x
                } else if x >= he_old {
                    (x as isize + delta) as usize
                } else if end {
                    he_new
                } else {
                    hs
                }
            }
            None => x,
        }
    };
    let to_old = |x: usize, end: bool| -> usize {
        match edit {
            Some((hs, he_old, he_new)) => {
                if x <= hs {
                    x
                } else if x >= he_new {
                    (x as isize - delta) as usize
                } else if end {
                    he_old
                } else {
                    hs
                }
            }
            None => x,
        }
    };
    let by_pieces = |s: usize, e: usize| -> (usize, usize) {
        let i0 = a.pieces.partition_point(|pc| pc.range.end < s);
        let i1 = a.pieces.partition_point(|pc| pc.range.start <= e);
        match (a.pieces.get(i0), i1.checked_sub(1).and_then(|i| a.pieces.get(i))) {
            (Some(f), Some(l)) if i0 < i1 => (f.range.start.min(s), l.range.end.max(e)),
            _ => (s, e),
        }
    };
    let mut ivs: Vec<(usize, usize, bool)> = Vec::new();
    if let Some((hs, he_old, _)) = edit {
        ivs.push((hs, he_old, true));
    }
    for (r, old_coords) in [(&m.reveal, true), (&p.reveal, false)] {
        if !shows(r) || (!old_coords && *r == m.reveal) {
            continue;
        }
        let (s, e) = if old_coords { (to_new(r.start, false), to_new(r.end, true)) } else { (r.start, r.end) };
        let (s, e) = by_pieces(s.min(text.len()), e.min(text.len()));
        ivs.push((to_old(s, false).min(old_text.len()), to_old(e, true).min(old_text.len()), false));
    }
    ivs.sort_unstable();
    let mut regions: Vec<Region> = Vec::new();
    let np = prep.paras.len();
    // `n` innehållsstycken bakåt/framåt från `i` (och hela vägen till dokumentets kant om det inte finns så många).
    let back = |mut i: usize| {
        let mut n = 0;
        while i > 0 && n <= CHECK {
            i -= 1;
            n += usize::from(content(&prep.paras[i]));
        }
        i
    };
    let fwd = |mut i: usize| {
        let mut n = 0;
        while i + 1 < np && n <= CHECK {
            i += 1;
            n += usize::from(content(&prep.paras[i]));
        }
        i
    };
    let marks = &prep.marks;
    for (s, e, is_edit) in ivs {
        let ai = prep.paras.partition_point(|q| q.range.end < s).min(np - 1);
        let bi = prep.paras.partition_point(|q| q.range.start <= e).saturating_sub(1).max(ai);
        // Fönstret ska innehålla hela textrutor (öppning till stängning) som det rör vid.
        let (mut a_ext, mut b_ext) = (ai, bi);
        let (mut lo, mut hi);
        loop {
            lo = back(a_ext);
            hi = fwd(b_ext);
            let (mut nlo, mut nhi) = (lo, hi);
            if let Some(&m) = marks.partition_point(|&m| m <= lo).checked_sub(1).and_then(|k| marks.get(k)) {
                if prep.paras[m].tbox.is_some() {
                    nlo = m;
                }
            }
            for i in lo..=hi {
                if prep.paras[i].tbox.is_some() {
                    if let Some(&end) = marks.get(marks.partition_point(|&m| m <= i)) {
                        nhi = nhi.max(end);
                    }
                }
            }
            if nlo == lo && nhi == hi || hi - lo + 1 > MAX_PARAS {
                break;
            }
            a_ext = a_ext.min(nlo);
            b_ext = b_ext.max(nhi);
        }
        match regions.last_mut() {
            Some(r) if lo <= r.hi + 1 => {
                r.hi = r.hi.max(hi);
                r.b = r.b.max(bi);
                r.edit |= is_edit;
            }
            _ => regions.push(Region { a: ai, b: bi, lo, hi, edit: is_edit }),
        }
    }
    if regions.len() > MAX_REGIONS || regions.iter().any(|r| r.hi - r.lo + 1 > MAX_PARAS) {
        return no(5);
    }
    // Fönstren i gamla och nya texten (byte).
    let after_edit = |r: &Region| edit.is_some() && !r.edit && prep.paras[r.lo].range.start >= edit.map_or(0, |e| e.1);
    let mut wins: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for r in &regions {
        let old = prep.paras[r.lo].range.start..prep.paras[r.hi].range.end;
        let shift_start = if after_edit(r) { delta } else { 0 };
        let shift_end = if after_edit(r) || r.edit { delta } else { 0 };
        let new = (old.start as isize + shift_start) as usize..(old.end as isize + shift_end) as usize;
        if new.end > text.len() || new.start > new.end {
            return no(8);
        }
        wins.push((old, new));
    }
    // Fotnotsreferenserna i ett ändrat fönster ska vara desamma (annars har definitionsplanen ändrats).
    for (r, (old, new)) in regions.iter().zip(&wins) {
        if r.edit && note_refs(&old_text[old.clone()]) != note_refs(&text[new.clone()]) {
            return no(9);
        }
        if broken_marker(&old_text[old.clone()]) || broken_marker(&text[new.clone()]) {
            return no(10);
        }
    }
    // Innehållsförteckningens höjd beror på antalet rubriker (ändringen i antalet kommer från den inkrementella analysen).
    if let Some(n) = prep.toc_n {
        if toc_delta.map_or_else(|| toc_count(text, a) != n, |d| d != 0) {
            return no(11);
        }
    }
    // Räkna om fönstren. Är rubrikerna som kan komma med i en förteckning desamma (`toc_delta`) har en förteckning i
    // fönstret samma höjd som förut, om raden är densamma.
    let mut toc_known: Vec<(String, f32)> = Vec::new();
    if toc_delta == Some(0) {
        for r in &regions {
            for q in prep.paras[r.lo..=r.hi].iter().filter(|q| q.toc_h > 0.0) {
                if let Some(t) = old_text.get(q.range.clone()) {
                    toc_known.push((t.trim().to_owned(), q.toc_h));
                }
            }
        }
    }
    let mut news: Vec<Vec<Para>> = Vec::new();
    for (_, new) in &wins {
        news.push(paragraphs_toc(text, a, p, Some(new.clone()), cb, &toc_known));
    }
    // Fotnotsplanen har sammanhang som fönstret inte ser: ett fönster med eller nära definitioner (även tomma stycken
    // fram till en definition) eller efter den sista räknar om planen i fönstret (`plan_window`). Det görs bara för ett
    // fönster i taget.
    let last_def = prep.defs.last().map(|d| d.para);
    let near_notes: Vec<bool> = regions
        .iter()
        .zip(&news)
        .map(|(r, w)| {
            if !prep.planned {
                return false;
            }
            let mut e = r.hi + 1;
            while e < np && prep.paras[e].range.is_empty() && e - r.hi < 64 {
                e += 1;
            }
            let i = prep.defs.partition_point(|d| d.para < r.lo);
            w.iter().any(|q| text[q.range.clone()].starts_with("[^"))
                || prep.defs.get(i).is_some_and(|d| d.para <= e)
                || e - r.hi >= 64
                || last_def.is_some_and(|l| r.hi > l)
        })
        .collect();
    // Övriga fönster får inte ändra antalet stycken (de är markörens rader utan ändring), så att styckenas index i
    // fönstret nära fotnoterna gäller när planen räknas om där.
    let n_near = near_notes.iter().filter(|&&n| n).count();
    if n_near > 1 || (n_near == 1 && regions.iter().zip(&near_notes).any(|(r, &n)| !n && r.edit)) {
        return no(7);
    }
    // Flytta styckena efter ändringen (före jämförelsen och inklistringen: de nya fönstren ligger i nya koordinater).
    let b_edit = regions.iter().find(|r| r.edit).map(|r| r.b);
    if let (Some(b), true) = (b_edit, delta != 0) {
        for q in &mut prep.paras[b + 1..] {
            q.range = (q.range.start as isize + delta) as usize..(q.range.end as isize + delta) as usize;
        }
    }
    // Kontrollera kontrollstyckena och sätt in de nya styckena (bakifrån så att index i gamla listan gäller).
    let mut dn: Vec<(usize, isize)> = Vec::new();
    // De insatta styckena per fönster (gamla index): första, antal gamla och antal nya.
    let mut spliced: Vec<(usize, usize, usize)> = Vec::new();
    // Innehållsförteckningens rad i de gamla och de nya styckena i fönstren.
    let (mut toc_old, mut toc_new) = (Vec::new(), Vec::new());
    let he_old_shift = |x: usize| match edit {
        Some((_, he_old, _)) if x >= he_old => (x as isize + delta) as usize,
        _ => x,
    };
    let mut planned_win: Option<(Vec<notes::Def>, Option<usize>, bool)> = None;
    for (((r, w), (_, new)), near) in regions.iter().zip(news.iter_mut()).zip(&wins).zip(&near_notes).rev() {
        if w.is_empty() || w[0].range.start != new.start || w.last()?.range.end != new.end {
            return no(12);
        }
        let lo_t = if r.lo == 0 { 0 } else { r.lo + 1 };
        let hi_t = if r.hi + 1 == np { r.hi } else { r.hi - 1 };
        if lo_t > r.a || hi_t < r.b || hi_t < lo_t {
            return no(13);
        }
        if *near {
            let used = (lo_t - r.lo)..(w.len().checked_sub(r.hi - hi_t)?);
            planned_win = Some(plan_window(&prep, w, r.lo, r.hi, used, text, a, p.zoom, &he_old_shift)?);
        }
        // Före det ändrade.
        for i in lo_t..r.a {
            if !same_para(&w[i - r.lo], &prep.paras[i]) {
                return no(14);
            }
        }
        // Efter det ändrade: stycke `i` (gamla index) ligger `r.hi - i` från slutet av fönstret.
        for i in r.b + 1..=hi_t {
            let k = (r.hi - i) + 1;
            if w.len() < k || !same_para(&w[w.len() - k], &prep.paras[i]) {
                return no(15);
            }
        }
        let from = lo_t - r.lo;
        let to = w.len() - (r.hi - hi_t);
        if to < from {
            return no(16);
        }
        let old_n = hi_t - lo_t + 1;
        let new_n = to - from;
        // Utan ändring (markören flyttades) har fönstret samma antal stycken.
        if !r.edit && new_n != old_n {
            return no(17);
        }
        // Styckena efter de ändrade har redan flyttats (nya lägen), de övriga står i den gamla texten.
        for (k, q) in prep.paras[lo_t..=hi_t].iter().enumerate().filter(|(_, q)| q.toc_h > 0.0) {
            let src = if b_edit.is_some_and(|b| lo_t + k > b) && delta != 0 { text } else { old_text };
            toc_old.push(src.get(q.range.clone()).unwrap_or("").trim().to_owned());
        }
        toc_new.extend(w[from..to].iter().filter(|q| q.toc_h > 0.0).map(|q| text[q.range.clone()].trim().to_owned()));
        prep.paras.splice(lo_t..=hi_t, w[from..to].iter().cloned());
        dn.push((hi_t, new_n as isize - old_n as isize));
        spliced.push((lo_t, old_n, new_n));
    }
    if let Some((defs, gap, trailing)) = planned_win {
        prep.defs = defs;
        prep.gap = gap;
        prep.trailing = trailing;
    } else {
        // Fotnotsdefinitionerna (och luftstycket) efter ändringen flyttas (inga ligger i eller nära fönstren).
        let shift = |i: usize| -> usize { (i as isize + dn.iter().filter(|(hi_t, _)| *hi_t < i).map(|(_, n)| *n).sum::<isize>()) as usize };
        if let Some((_, he_old, _)) = edit {
            for d in &mut prep.defs {
                let after = d.line.start >= he_old;
                d.para = shift(d.para);
                if after && delta != 0 {
                    let mv = |x: usize| (x as isize + delta) as usize;
                    d.line = mv(d.line.start)..mv(d.line.end);
                    d.body = mv(d.body);
                }
            }
        }
        prep.gap = prep.gap.map(shift);
    }
    // Innehållsförteckningen kan ha tillkommit eller försvunnit i ett fönster (annars är antalet rubriker oförändrat).
    if toc_old != toc_new || toc_delta.is_none() {
        prep.toc_n = prep.paras.iter().any(|q| q.toc_h > 0.0).then(|| toc_count(text, a));
    }
    spliced.reverse();
    // Textrutornas markörer: de i fönstren tas från de nya styckena, de efter flyttas.
    {
        let old = std::mem::take(&mut prep.marks);
        let mut marks = Vec::with_capacity(old.len() + 2);
        let (mut k, mut shift) = (0usize, 0isize);
        for &(lo, old_n, new_n) in &spliced {
            while k < old.len() && old[k] < lo {
                marks.push((old[k] as isize + shift) as usize);
                k += 1;
            }
            while k < old.len() && old[k] < lo + old_n {
                k += 1;
            }
            let s0 = (lo as isize + shift) as usize;
            marks.extend((s0..s0 + new_n).filter(|&i| prep.paras[i].tbox.is_some() || prep.paras[i].tbox_end));
            shift += new_n as isize - old_n as isize;
        }
        marks.extend(old[k..].iter().map(|&m| (m as isize + shift) as usize));
        prep.marks = marks;
    }
    if let Some(()) = regroup(&mut prep, &spliced, text, a, p, grid, edit.map(|e| (e.1, delta))) {
        return Some(prep);
    }
    // Grupper och statiska nycklar: oförändrade grupper utanför fönstren behåller sin nyckel.
    let old_groups = std::mem::take(&mut prep.groups);
    let old_keys = std::mem::take(&mut prep.skeys);
    let groups = group_paras(text, &prep.paras, grid);
    let dirty: Vec<Range<usize>> = wins.iter().map(|(_, n)| n.clone()).collect();
    let he_new = edit.map_or(usize::MAX, |e| e.2);
    let mut skeys: Vec<u64> = Vec::with_capacity(groups.len());
    let mut from: Vec<u32> = Vec::with_capacity(groups.len());
    let mut j = 0;
    for g in &groups {
        let w = &g.1;
        let i = dirty.partition_point(|r| r.end <= w.start);
        let clean = !dirty.get(i).is_some_and(|r| r.start < w.end);
        let old_start = if edit.is_some() && w.start >= he_new { (w.start as isize - delta) as usize } else { w.start };
        let mut key = None;
        if clean {
            while j < old_groups.len() && old_groups[j].1.start < old_start {
                j += 1;
            }
            if let Some(og) = old_groups.get(j) {
                if og.1.start == old_start && og.1.len() == w.len() && og.2.is_some() == g.2.is_some() && og.0.len() == g.0.len() {
                    key = Some(old_keys[j]);
                }
            }
        }
        from.push(if key.is_some() { j as u32 } else { u32::MAX });
        skeys.push(key.unwrap_or_else(|| unit_key_static(text, a, p, w)));
    }
    prep.first_new = from.iter().enumerate().position(|(i, &j)| j as usize != i).unwrap_or(from.len());
    prep.groups = groups;
    prep.skeys = skeys;
    prep.from = from;
    Some(prep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::bench::Rng;
    use crate::word::{incr, pages};

    fn fam(_: Option<&str>, bold: bool, italic: bool) -> FontFamily {
        theme::doc_font(bold, italic)
    }

    /// Dokument i Word-dialekten: stycken med `<!--k:pp-->`-markörer, tomma stycken, rubriker, listor, tabeller,
    /// `<div align>`, sidbrytningar, innehållsförteckning, fotnoter (definitioner sist) och ibland en textruta.
    fn doc(r: &mut Rng, paras: usize, boxes: bool) -> String {
        doc_with(r, paras, boxes, false)
    }

    /// `word`: fotnotsdefinitionerna står som ett block utan tomrader (som Word-läsaren skriver dem), ibland med
    /// fortsättningsrader, slutnoter och tomma rader efter.
    fn doc_with(r: &mut Rng, paras: usize, boxes: bool, word: bool) -> String {
        let words = ["alfa", "beta", "gamma", "delta", "åäö", "text", "rad", "stycke", "dokument", "sida"];
        let mut s = String::from("<!-- kladd: sida=a4; h1: \"Calibri\" 20pt -->\n\n");
        let mut notes = 0;
        for i in 0..paras {
            let kind = r.below(24);
            let mut line = String::new();
            for _ in 0..(3 + r.below(40)) {
                let w = words[r.below(words.len())];
                match r.below(14) {
                    0 => line.push_str(&format!("**{w}** ")),
                    1 => line.push_str(&format!("*{w}* ")),
                    _ => line.push_str(&format!("{w} ")),
                }
            }
            match kind {
                0 => s.push_str(&format!("## Rubrik {i}")),
                1 => s.push_str("- punkt ett\n- punkt två\n- punkt tre"),
                2 => s.push_str("| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |"),
                3 => s.push_str("<!--k:ep-->"),
                13 => s.push_str("```\nkod rad ett\n\nkod rad två\n```"),
                4 => s.push_str("<!--k:ep sp=\"b=0 a=12\"-->"),
                5 => s.push_str("<!-- kladd: sidbrytning -->"),
                6 => s.push_str(&format!("<div align=\"justify\">\n\n{line}\n\n</div>")),
                7 | 8 => s.push_str(&format!("{line}<!--k:pp sp=\"b=0 a={}\"-->", r.below(12))),
                9 => s.push_str(&format!("{line}<!--k:pp bdr=\"t=single:8:4:156082 b=single:8:4:156082\" shd=\"E8F1F5\"-->")),
                10 if boxes => s.push_str(&format!("<!--k:tb 1 w=\"157.5\" h=\"90\" hrel=\"page\" x=\"400\" y=\"0\"-->\n\n{line}\n\n<!--k:/tb-->")),
                11 | 12 => {
                    notes += 1;
                    s.push_str(&format!("{line}[^{notes}]"));
                }
                _ => s.push_str(&line),
            }
            s.push_str("\n\n");
            if i == paras / 5 {
                s.push_str("<!-- kladd: toc -->\n\n");
            }
        }
        if word {
            for n in 1..=notes {
                s.push_str(&format!("[^{n}]: Fotnot {n} med *text* och mer text"));
                match r.below(12) {
                    0 => s.push_str("\nfortsättningsrad"),
                    1 => s.push_str(&" lång".repeat(30)),
                    _ => {}
                }
                s.push('\n');
            }
            match r.below(3) {
                0 => s.push_str("\n[^ei]: Slutnot.\n[^eii]: Slutnot två.\n"),
                1 => s.push_str("\n\n"),
                _ => {}
            }
            return s;
        }
        for n in 1..=notes {
            s.push_str(&format!("[^{n}]: Fotnot {n} med text.\n\n"));
        }
        s
    }

    /// Ett ställe för en ändring: i två fall av tre i eller nära fotnotsdefinitionerna (om `near`).
    fn spot(r: &mut Rng, text: &str, near: bool) -> usize {
        let from = match text.find("\n[^1]:").filter(|_| near && r.below(3) != 0) {
            Some(f) => f.saturating_sub(400),
            None => 0,
        };
        let mut at = from + r.below(text.len() - from);
        while !text.is_char_boundary(at) {
            at += 1;
        }
        at
    }

    fn edit(r: &mut Rng, text: &mut String) {
        edit_near(r, text, false)
    }

    fn edit_near(r: &mut Rng, text: &mut String, near: bool) {
        let at = spot(r, text, near);
        match r.below(12) {
            0..=3 => text.insert(at, 'x'),
            4 => text.insert_str(at, " ord "),
            5 => text.insert_str(at, "\n\n"),
            6 => text.insert(at, '\n'),
            7 => text.insert_str(at, "<!--k:pp sp=\"b=3 a=3\"-->"),
            8 => text.insert_str(at, "# "),
            9 => {
                if let Some(c) = text[at..].chars().next() {
                    text.replace_range(at..at + c.len_utf8(), "");
                }
            }
            10 => {
                let mut e = (at + 1 + r.below(40)).min(text.len());
                while !text.is_char_boundary(e) {
                    e += 1;
                }
                text.replace_range(at..e, "");
            }
            _ => text.insert_str(at, "**"),
        }
    }

    fn defs_of(prep: &Prep) -> Vec<(String, usize, Range<usize>, usize)> {
        prep.defs.iter().map(|d| (d.label.clone(), d.para, d.line.clone(), d.body)).collect()
    }

    fn same(inc: &Prep, full: &Prep, step: &str) {
        assert_eq!(inc.paras.len(), full.paras.len(), "steg {step}: antal stycken");
        for (i, (x, y)) in inc.paras.iter().zip(&full.paras).enumerate() {
            assert_eq!(format!("{x:?}"), format!("{y:?}"), "steg {step}: stycke {i}");
        }
        assert_eq!(defs_of(inc), defs_of(full), "steg {step}: fotnotsdefinitioner");
        assert_eq!(inc.groups.len(), full.groups.len(), "steg {step}: antal grupper");
        assert!(inc.groups == full.groups, "steg {step}: grupper");
        let bad: Vec<usize> = (0..inc.skeys.len()).filter(|&i| inc.skeys[i] != full.skeys[i]).collect();
        assert!(bad.is_empty(), "steg {step}: nycklar skiljer i grupper {bad:?} (av {}), t.ex. {:?}", inc.skeys.len(), bad.first().map(|&i| &inc.groups[i]));
        assert_eq!(inc.toc_n, full.toc_n, "steg {step}: innehållsförteckning");
        assert_eq!(inc.marks, full.marks, "steg {step}: textrutornas markörer");
    }

    /// Återanvänd förberedelse (`reuse`) ska ge exakt det som en full omräkning ger.
    fn reuse_equals_fresh(seed: u64, paras: usize, boxes: bool, steps: usize) -> (usize, usize) {
        reuse_equals_fresh_with(seed, paras, boxes, steps, false)
    }

    /// `word`: fotnoterna som ett block (se `doc_with`) och ändringarna oftast i eller nära det.
    fn reuse_equals_fresh_with(seed: u64, paras: usize, boxes: bool, steps: usize, word: bool) -> (usize, usize) {
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let measure = |_: &str, _: FontId| 8.0;
        let mut r = Rng(seed);
        let mut text = doc_with(&mut r, paras, boxes, word);
        let (mut reused, mut fell_back) = (0, 0);
        for step in 0..steps {
            let old = text.clone();
            for _ in 0..1 + r.below(2) {
                edit_near(&mut r, &mut text, word);
            }
            let rv = |r: &mut Rng, t: &str| -> Range<usize> {
                if r.below(3) == 0 {
                    return 0..0;
                }
                let a = spot(r, t, word);
                crate::word::tables::reveal_range(t, a..a)
            };
            let (old_rv, new_rv) = (rv(&mut r, &old), rv(&mut r, &text));
            let mk = |reveal: Range<usize>| Params {
                palette: &pal,
                zoom: 1.0,
                reveal,
                image_size: &img,
                measure: &measure,
                font_family: &fam,
                fonts_gen: 0,
            };
            let (po, pn) = (mk(old_rv.clone()), mk(new_rv));
            let (ao, an) = (markup::analyze(&old), markup::analyze(&text));
            let (go, gn) = (tables(&old, &ao), tables(&text, &an));
            let (co, cn) = (codeblock::blocks(&old, &ao), codeblock::blocks(&text, &an));
            let memo = Memo { sig: 7, rev: 1, reveal: old_rv, prep: fresh(&old, &ao, &po, true, &go, &co) };
            let full = fresh(&text, &an, &pn, true, &gn, &cn);
            let ch = incr::changed_range(&old, &text);
            // Ett litet område (som `analyze_incremental` skulle ge), ibland utvidgat till hela styckena runt.
            let hull = if r.below(2) == 0 {
                (ch.start, ch.end)
            } else {
                let mut c0 = ch.start.min(old.len());
                while !old.is_char_boundary(c0) {
                    c0 -= 1;
                }
                let mut c1 = ch.end.min(text.len());
                while !text.is_char_boundary(c1) {
                    c1 += 1;
                }
                let s = old[..c0].rfind("\n\n").map_or(0, |i| i + 2);
                (s.min(ch.start), text[c1..].find("\n\n").map_or(text.len(), |i| c1 + i).max(ch.end))
            };
            let rev = if old == text { 1 } else { 2 };
            // `analyze_incremental` anger om fotnotsreferenserna ändrats (`Delta::notes_changed`); här jämförs hela analyserna.
            let notes = |a: &Analysis| -> Vec<(bool, String)> {
                let mut v: Vec<(bool, String)> = a.pieces.iter().flat_map(|p| p.footnote_ref.clone().map(|x| (true, x)).into_iter().chain(p.footnote_def.clone().map(|x| (false, x)))).collect();
                v.sort();
                v.dedup();
                v
            };
            if notes(&ao) != notes(&an) {
                WHY.with(|w| w.borrow_mut().push(99));
                fell_back += 1;
                continue;
            }
            // Ändringen i det som kan komma med i en innehållsförteckning (som den inkrementella analysen ger), i hälften av
            // stegen: med 0 återanvänds förteckningens höjd och antal (`toc_known`).
            let toc_delta = (step % 2 == 0).then(|| {
                use crate::word::nav::{toc_sig_diff, toc_sig_in};
                toc_sig_diff(&toc_sig_in(&text, &an.pieces), &toc_sig_in(&old, &ao.pieces))
            });
            match reuse(memo, &old, Some(hull), rev, &text, &an, &pn, 7, &gn, &cn, toc_delta, step % 3 == 0) {
                Some(inc) => {
                    same(&inc, &full, &format!("seed {seed} steg {step} ch {ch:?} hull {hull:?} rv {:?}->{:?} | grupp ~ ", po.reveal, pn.reveal));
                    reused += 1;
                }
                None => fell_back += 1,
            }
        }
        (reused, fell_back)
    }

    #[test]
    fn reused_preparation_equals_full() {
        let (mut reused, mut back) = (0, 0);
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(6).max(6);
        for seed in 1..=seeds {
            let (a, b) = reuse_equals_fresh(seed * 7919, 260, false, 40);
            reused += a;
            back += b;
        }
        // De flesta ändringar ligger långt från fotnotsdefinitioner och rutor och ska gå att återanvända.
        assert!(reused > 2 * back, "återanvändes {reused}, full omräkning {back}");
    }

    /// Ändringar i och nära fotnotsdefinitionerna (Word-dialekten, ett block utan tomrader): planen räknas om i
    /// fönstret (`plan_window`) och ska ge samma stycken, definitioner och grupper som en full omräkning.
    #[test]
    fn reused_preparation_near_footnote_definitions() {
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).max(6);
        let (mut reused, mut back) = (0, 0);
        for seed in 1..=seeds {
            let (a, b) = reuse_equals_fresh_with(seed * 6151, 160, seed % 3 == 0, 40, true);
            reused += a;
            back += b;
        }
        let why = WHY.with(|w| {
            let mut c = std::collections::BTreeMap::new();
            for x in w.borrow_mut().drain(..) {
                *c.entry(x).or_insert(0) += 1;
            }
            c
        });
        // Markörens gamla och nya rader slumpas (ofta två fönster nära fotnoterna, som ger full omräkning).
        assert!(reused * 3 > reused + back, "återanvändes {reused}, full omräkning {back} (skäl {why:?})");
    }

    #[test]
    fn reused_one_seed() {
        if let Some(seed) = std::env::var("MEMO_SEED").ok().and_then(|v| v.parse::<u64>().ok()) {
            reuse_equals_fresh(seed, 260, false, 40);
        }
    }

    #[test]
    fn reused_preparation_with_text_boxes() {
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).max(4);
        let (mut reused, mut back) = (0, 0);
        for seed in 1..=seeds {
            let (a, b) = reuse_equals_fresh(seed * 104_729, 200, true, 40);
            reused += a;
            back += b;
        }
        assert!(reused > back, "återanvändes {reused}, full omräkning {back}");
    }

    /// Hela vägen: galleriet efter inkrementell analys och återanvänd förberedelse är detsamma som i en ny cache.
    #[test]
    fn galley_after_edits_equals_fresh_cache() {
        let reused = galley_equals_fresh(4242, false, 30);
        assert!(reused > 8, "återanvändes bara {reused} gånger");
    }

    /// Som ovan, med fotnoterna som ett block och ändringarna oftast i eller nära det: fotnoterna längst ned på sidorna
    /// (utläggning och sidor) ska också bli desamma.
    #[test]
    fn galley_after_edits_near_footnotes_equals_fresh_cache() {
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).map_or(2, |n| (n / 10).max(2));
        let mut reused = 0;
        for seed in 0..seeds {
            reused += galley_equals_fresh(777 + seed * 31, true, 30);
        }
        // Enstaka dokument kan ge full analys vid de flesta ändringar (t.ex. en tomrad i definitionsblocket).
        assert!(reused > seeds as usize * 6, "återanvändes bara {reused} gånger");
    }

    /// Styckenas utseende (`<!--k:pp …-->`) och rubrikerna efter inkrementella ändringar (läses om bara i fönstret,
    /// `pspecs_patch` och `headings_patch`) ska vara desamma som när hela texten läses.
    #[test]
    fn paragraph_looks_follow_incremental_edits() {
        incr::MIN_OVERRIDE.with(|m| m.set(Some(1000)));
        let seeds = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse::<u64>().ok()).map_or(3, |n| (n / 10).max(3));
        let mut patched = 0;
        for seed in 0..seeds {
            let mut r = Rng(3001 + seed * 17);
            let mut text = doc_with(&mut r, 160, seed % 2 == 0, seed % 3 == 0);
            let mut cache = Cache::default();
            let _ = (cache.para_boxes(1, &text), cache.headings(1, &text));
            for step in 0..60 {
                edit_near(&mut r, &mut text, seed % 3 == 0);
                if r.below(3) == 0 {
                    text.insert_str(spot(&mut r, &text, false), "<!--k:pp bdr=\"t=single:8:4:156082\" shd=\"E8F1F5\"-->");
                }
                let got = cache.para_boxes(1, &text);
                patched += usize::from(cache.docs.get(&1).is_some_and(|d| d.facts.is_some()));
                let specs = if text.contains(pbox::MARK) { pbox::scan(&text) } else { Vec::new() };
                let runs = pbox::box_runs(&text, &specs);
                let f = |v: &[pbox::ParaSpec]| v.iter().map(|s| (s.range.clone(), format!("{:?}", s.pp))).collect::<Vec<_>>();
                assert_eq!(f(&got.0), f(&specs), "seed {seed} steg {step}: styckenas utseende");
                let g = |v: &[pbox::BoxRun]| v.iter().map(|b| (b.specs.clone(), b.range.clone())).collect::<Vec<_>>();
                assert_eq!(g(&got.1), g(&runs), "seed {seed} steg {step}: rutorna");
                // Rubrikerna flyttas och räknas om i fönstret på samma sätt.
                let heads = cache.headings(1, &text);
                let want = crate::word::nav::headings(&text, &markup::analyze(&text));
                let h = |v: &[crate::word::nav::Heading]| v.iter().map(|x| (x.level, x.text.clone(), x.start)).collect::<Vec<_>>();
                assert_eq!(h(&heads), h(&want), "seed {seed} steg {step}: rubriker");
            }
        }
        incr::MIN_OVERRIDE.with(|m| m.set(None));
        assert!(patched > 0, "ingen inkrementell ändring");
    }

    /// En förteckning med egna nivåer eller över bildtexter: en ny rubrik på en nivå utanför 1–3 eller en ny bildtext
    /// ändrar förteckningens höjd, så förberedelsen får inte återanvändas med den gamla höjden.
    #[test]
    fn toc_with_own_levels_or_captions_follows_new_entries() {
        incr::MIN_OVERRIDE.with(|m| m.set(Some(1000)));
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let body: String = (0..160).map(|i| format!("Stycke {i} med lite text som fyller ut raden ordentligt.\n\n")).collect();
        for (spec, entry) in [("<!-- kladd: toc; levels: 1-5 -->", "#### Ny rubrik"), ("<!-- kladd: toc; kind: figures -->", "Figur 9: Ny bildtext")] {
            let mut text = format!("{spec}\n\n# Början\n\n{body}");
            let a0 = markup::analyze(&text);
            let geom = pages::Geom::for_sections(1.0, &a0.defaults, &a0.sections);
            let mut cache = Cache::default();
            let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, egui::Color32::WHITE).size().x);
            let p = Params { palette: &pal, zoom: 1.0, reveal: 0..0, image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
            for step in 0..3 {
                if step > 0 {
                    // Ersätt ett stycke mitt i dokumentet med en ny post (styckegränserna är kvar).
                    let at = text.find(&format!("Stycke {}", 40 * step)).unwrap();
                    let end = at + text[at..].find('\n').unwrap();
                    text.replace_range(at..end, entry);
                }
                let out = ctx.run_ui(Default::default(), |_| {
                    let g = cache.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                    let mut fresh_cache = Cache::default();
                    let f = fresh_cache.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                    assert_eq!(g.rect, f.rect, "{spec} steg {step}");
                    assert_eq!(g.rows.len(), f.rows.len(), "{spec} steg {step}");
                    for (k, (x, y)) in g.rows.iter().zip(f.rows.iter()).enumerate() {
                        assert_eq!(x.pos, y.pos, "{spec} steg {step} rad {k}");
                    }
                });
                std::mem::forget(out);
            }
        }
        incr::MIN_OVERRIDE.with(|m| m.set(None));
    }

    /// Ger hur många gånger förberedelsen återanvändes.
    fn galley_equals_fresh(seed: u64, word: bool, steps: usize) -> usize {
        incr::MIN_OVERRIDE.with(|m| m.set(Some(1000)));
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut r = Rng(seed);
        let mut text = doc_with(&mut r, 120, false, word);
        let mut cache = Cache::default();
        let a0 = markup::analyze(&text);
        let geom = pages::Geom::for_sections(1.0, &a0.defaults, &a0.sections);
        let shape = |g: &Galley| -> Vec<(i32, i32, usize, bool)> {
            g.rows.iter().map(|r| ((r.pos.x * 100.0) as i32, (r.pos.y * 100.0) as i32, r.char_count_including_newline().0, r.ends_with_newline)).collect()
        };
        let mut reused = 0;
        for step in 0..steps {
            if step > 0 {
                edit_near(&mut r, &mut text, word);
            }
            // Ibland två ändringar före en omläggning: analysen följer den första (området sätts samman av två).
            if step % 3 == 2 {
                let _ = cache.analysis(1, &text);
                edit_near(&mut r, &mut text, word);
            }
            let reveal = if step % 4 == 1 { crate::word::tables::reveal_range(&text, text.len() / 2..text.len() / 2) } else { 0..0 };
            let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, egui::Color32::WHITE).size().x);
            let p = Params { palette: &pal, zoom: 1.0, reveal, image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
            let out = ctx.run_ui(Default::default(), |_| {
                let g = cache.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                reused += usize::from(cache.stats(1).reused);
                let mut fresh_cache = Cache::default();
                let f = fresh_cache.galley_paged(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom));
                assert_eq!(shape(&g), shape(&f), "steg {step}");
                assert_eq!(g.rect, f.rect, "steg {step}");
                let (pb, fb) = (cache.page_boxes(1, &text), fresh_cache.page_boxes(1, &text));
                assert_eq!(pb.len(), fb.len(), "steg {step}: sidor");
                for (k, (x, y)) in pb.iter().zip(fb.iter()).enumerate() {
                    assert_eq!((x.top, x.h, &x.notes, x.reserved), (y.top, y.h, &y.notes, y.reserved), "steg {step}: sida {k}");
                }
                let (nx, ny) = (cache.notes(1, &text), fresh_cache.notes(1, &text));
                let notes = |v: &[crate::word::fnotes::NoteLayout]| -> Vec<(String, Range<usize>, Range<usize>, i32, Vec<(i32, i32, usize)>)> {
                    v.iter().map(|n| (n.label.clone(), n.line.clone(), n.body.clone(), (n.indent * 100.0) as i32, shape(&n.galley).into_iter().map(|r| (r.0, r.1, r.2)).collect())).collect()
                };
                assert_eq!(notes(&nx), notes(&ny), "steg {step}: fotnoter");
            });
            std::mem::forget(out);
        }
        incr::MIN_OVERRIDE.with(|m| m.set(None));
        reused
    }
}
