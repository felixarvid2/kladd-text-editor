//! Fotnoter i layouten: vilka definitioner som lämnar flödet, deras utläggning för ritning längst ned
//! på sidan och underlaget till sidbrytningen (se `fnotes`).

use egui::text::CCursor;

use super::*;
use crate::word::fnotes::{NoteLayout, NoteSpec};

/// En fotnotsdefinition som ritas längst ned på referenssidan i stället för sist i dokumentet.
#[derive(Clone)]
pub(super) struct Def {
    pub label: String,
    /// Styckets index.
    pub para: usize,
    /// Raden och där texten efter `[^n]: ` börjar (byte).
    pub line: Range<usize>,
    pub body: usize,
}

/// Fortsättningsrad i en definition (en rad i fotnotsblocket som inte börjar med `[^`).
fn is_continuation(text: &str, a: &Analysis, q: &Para) -> bool {
    let first = a.pieces.partition_point(|pc| pc.range.end <= q.range.start);
    a.pieces.get(first).is_some_and(|pc| pc.block == Block::Footnote) && !text[q.range.clone()].starts_with("[^")
}

/// Etiketterna som har en referens (utom slutnoter): bara deras definitioner lämnar flödet.
pub(super) fn ref_labels(a: &Analysis) -> BTreeSet<String> {
    a.pieces.iter().filter_map(|pc| pc.footnote_ref.as_deref()).filter(|l| !markup::is_endnote_label(l)).map(str::to_owned).collect()
}

/// Fotnotsdefinitionerna som stycket `q` (följt av `next`) är, i textordning: (etikett, där texten efter `[^n]: ` börjar).
/// Allt i `plan` utom regeln att bara den första definitionen för en etikett räknas.
pub(super) fn defs_of<'a>(text: &str, a: &'a Analysis, q: &Para, next: Option<&Para>, refs: &BTreeSet<String>) -> Vec<(&'a str, usize)> {
    let mut out = Vec::new();
    if q.range.is_empty() || !text[q.range.clone()].starts_with("[^") {
        return out;
    }
    for pc in pieces_in(a, &q.range) {
        let Some(l) = pc.footnote_def.as_deref() else { continue };
        if markup::is_endnote_label(l) || !refs.contains(l) || !(q.range.start <= pc.range.start && pc.range.end <= q.range.end) {
            continue;
        }
        if next.is_some_and(|n| !n.range.is_empty() && is_continuation(text, a, n)) {
            continue;
        }
        out.push((l, (pc.range.end + 3).min(q.range.end)));
    }
    out
}

/// Resultatet av `plan`: definitionerna och det som `layout_memo` behöver för att kunna räkna om planen i ett fönster.
pub(super) struct Plan {
    pub defs: Vec<Def>,
    /// Etiketterna med referens (`ref_labels`).
    pub refs: BTreeSet<String>,
    /// Stycket (första synliga slutnoten efter fotnoterna) som fick luft över sig.
    pub gap: Option<usize>,
    /// Alla stycken efter den sista definitionen är tomma (och hopkrympta).
    pub trailing: bool,
}

/// Fotnotsdefinitionerna (inte slutnoterna) som har en referens. Tomma stycken närmast dem hoppas
/// också ur flödet, så att de inte tar en sida i anspråk.
pub(super) fn plan(text: &str, a: &Analysis, paras: &mut [Para], zoom: f32) -> Plan {
    let mut out = Plan { defs: Vec::new(), refs: BTreeSet::new(), gap: None, trailing: false };
    if !text.contains("[^") {
        return out;
    }
    let refs = ref_labels(a);
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut defs: Vec<Def> = Vec::new();
    for pi in 0..paras.len() {
        let q = &paras[pi];
        for (l, body) in defs_of(text, a, q, paras.get(pi + 1), &refs) {
            if seen.insert(l) {
                defs.push(Def { label: l.to_owned(), para: pi, line: q.range.clone(), body });
            }
        }
    }
    // Följer slutnoter (synliga definitioner) efter fotnoterna får den första luft över sig (skiljelinjen
    // ritas där), eftersom tomraden före fotnoterna är hopkrympt.
    let last = defs.iter().map(|d| d.para).max();
    if let Some(last) = last {
        out.gap = (last + 1..paras.len()).find(|&i| gap_candidate(text, &paras[i]));
        if let Some(i) = out.gap {
            paras[i].space_before += gap_space(zoom);
        }
    }
    for d in &defs {
        let mut k = d.para;
        while k > 0 && paras[k - 1].range.is_empty() {
            k -= 1;
            paras[k].collapse = true;
        }
    }
    if let Some(last) = defs.iter().map(|d| d.para).max() {
        if paras[last + 1..].iter().all(|q| q.range.is_empty()) {
            out.trailing = true;
            for q in &mut paras[last + 1..] {
                q.collapse = true;
            }
        }
    }
    out.defs = defs;
    out.refs = refs;
    out
}

/// Kan stycket vara den första synliga slutnoten efter fotnoterna (som får luft över sig, se `plan`)?
pub(super) fn gap_candidate(text: &str, q: &Para) -> bool {
    !q.range.is_empty() && text[q.range.clone()].starts_with("[^") && !q.collapse
}

/// Luften över den första slutnoten efter fotnoterna (se `plan`).
pub(super) fn gap_space(zoom: f32) -> f32 {
    px(zoom, 12.0)
}

/// Definitionens text som den ser ut längst ned på sidan: nummer och text, utan markup.
#[allow(clippy::too_many_arguments)]
pub(super) fn layout_def(
    st: &mut DocState,
    ctx: &egui::Context,
    text: &str,
    a: &Analysis,
    p: &Params,
    grid: &[Table],
    w: &Range<usize>,
    para: &Para,
    wrap_width: f32,
    ppp: f32,
    env: u64,
    d: &Def,
    lazy: bool,
    skey: u64,
) -> Option<NoteLayout> {
    let pv = Params {
        palette: p.palette,
        zoom: p.zoom,
        reveal: 0..0,
        image_size: p.image_size,
        measure: p.measure,
        font_family: p.font_family,
        fonts_gen: p.fonts_gen,
    };
    let mut q = para.clone();
    q.collapse = false;
    q.space_before = 0.0;
    q.first_indent = 0.0;
    q.float = None;
    q.page_break = false;
    q.col_break = false;
    let key = {
        // Den statiska nyckeln gäller så länge markören inte står i raden (här visas aldrig markup).
        let skey = if !intersects(&p.reveal, w) { skey } else { unit_key_static(text, a, &pv, w) };
        let mut h = Fx(unit_key_with(env, skey, w, std::slice::from_ref(&q)));
        (wrap_width.to_bits(), 0xF007u32).hash(&mut h);
        h.finish()
    };
    let tick = st.tick;
    let galley = match st.units.get_mut(&key) {
        Some(u) => {
            u.used = tick;
            u.galleys[0].clone()
        }
        // Stegvis utläggning: noter som inte är utlagda ännu hoppas över (de saknas på sidorna som ännu är platshållare).
        None if lazy => return None,
        None => {
            let mut job = LayoutJob { text: text[w.clone()].to_owned(), ..Default::default() };
            job.sections = build_sections(text, a, &pv, grid, w.clone(), &[]);
            for s in &mut job.sections {
                s.byte_range = ByteIndex(s.byte_range.start.0 - w.start)..ByteIndex(s.byte_range.end.0 - w.start);
            }
            let rel = Para { range: q.range.start - w.start..q.range.end - w.start, ..q.clone() };
            let g = ctx.fonts_mut(|f| layout_para_floats(f, &job, &rel, wrap_width, ppp, &[]));
            st.units.insert(key, Unit { galleys: Arc::new(vec![g.clone()]), geom: None, used: tick });
            g
        }
    };
    let rel_chars = text[w.start..d.body.max(w.start)].chars().count();
    let indent = galley.pos_from_cursor(CCursor::new(rel_chars)).min.x;
    // Numret i dokumentets talformat: etiketten är dold i galleriet och numret ritas ovanpå.
    let first = a.pieces.partition_point(|pc| pc.range.end <= d.line.start);
    let label_ov = a.pieces[first..]
        .iter()
        .take_while(|pc| pc.range.start < d.line.end)
        .find(|pc| pc.footnote_def.as_deref() == Some(d.label.as_str()) && d.line.contains(&pc.range.start))
        .and_then(|pc| {
            let ov = note_label_ov(pc, a, &pv)?;
            let at = galley.pos_from_cursor(CCursor::new(text[w.start..pc.range.start].chars().count())).min;
            let wd = (pv.measure)(&ov.text, ov.font.clone());
            Some((ov, egui::pos2(at.x - wd, at.y)))
        });
    Some(NoteLayout { label: d.label.clone(), galley, line: d.line.clone(), body: d.body..d.line.end, indent, label_ov })
}

/// Underlaget till sidbrytningen: höjderna och raderna (i galleriet `g`, före sidbrytningen) som
/// referenserna ligger på.
pub(super) fn spec(notes: &[NoteLayout], g: &Galley, text: &str, a: &Analysis, zoom: f32, cap: f32) -> NoteSpec {
    let mut spec = NoteSpec {
        refs: Vec::new(),
        heights: notes.iter().map(|n| n.galley.rect.height() + px(zoom, 2.0)).collect(),
        sep: px(zoom, 9.0),
        cap,
    };
    let (mut byte, mut chars) = (0usize, 0usize);
    let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, n) in notes.iter().enumerate() {
        index.entry(n.label.as_str()).or_insert(i);
    }
    // Raden som ett tecken ligger på: som `Galley::layout_from_cursor` (första raden som slutar vid eller efter
    // tecknet), men med binärsökning (egui går igenom alla rader för varje anrop).
    let mut row_ends: Vec<usize> = Vec::with_capacity(g.rows.len());
    let mut start = 0usize;
    for r in &g.rows {
        row_ends.push(start + r.char_count_excluding_newline().0);
        start += r.char_count_including_newline().0;
    }
    for pc in &a.pieces {
        let Some(l) = pc.footnote_ref.as_deref() else { continue };
        let Some(&n) = index.get(l) else { continue };
        chars += text[byte..pc.range.start].chars().count();
        byte = pc.range.start;
        let row = row_ends.partition_point(|e| *e < chars).min(row_ends.len().saturating_sub(1));
        spec.refs.push((row, n));
    }
    spec.refs.sort_by_key(|r| r.0);
    spec
}

/// Alla tecken dolda (för definitionsraden i flödet; texten ritas längst ned på sidan i stället).
pub(super) fn hide_all(job: &mut LayoutJob, family: FontFamily) {
    let len = job.text.len();
    job.sections = vec![LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(len), format: hidden_format(family) }];
}
