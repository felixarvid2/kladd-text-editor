//! Fotnoter i layouten: vilka definitioner som lämnar flödet, deras utläggning för ritning längst ned
//! på sidan och underlaget till sidbrytningen (se `fnotes`).

use egui::text::CCursor;

use super::*;
use crate::word::fnotes::{NoteLayout, NoteSpec};

/// En fotnotsdefinition som ritas längst ned på referenssidan i stället för sist i dokumentet.
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

/// Fotnotsdefinitionerna (inte slutnoterna) som har en referens. Tomma stycken närmast dem hoppas
/// också ur flödet, så att de inte tar en sida i anspråk.
pub(super) fn plan(text: &str, a: &Analysis, paras: &mut [Para], zoom: f32) -> Vec<Def> {
    let mut defs: Vec<Def> = Vec::new();
    if !text.contains("[^") {
        return defs;
    }
    let refs: BTreeSet<&str> = a
        .pieces
        .iter()
        .filter_map(|pc| pc.footnote_ref.as_deref())
        .filter(|l| !markup::is_endnote_label(l))
        .collect();
    for pc in &a.pieces {
        let Some(l) = pc.footnote_def.as_deref() else { continue };
        if markup::is_endnote_label(l) || !refs.contains(l) || defs.iter().any(|d| d.label == l) {
            continue;
        }
        let pi = paras.partition_point(|q| q.range.end < pc.range.start);
        let Some(q) = paras.get(pi) else { continue };
        if !(q.range.start <= pc.range.start && pc.range.end <= q.range.end) || !text[q.range.clone()].starts_with("[^") {
            continue;
        }
        if paras.get(pi + 1).is_some_and(|n| !n.range.is_empty() && is_continuation(text, a, n)) {
            continue;
        }
        defs.push(Def { label: l.to_owned(), para: pi, line: q.range.clone(), body: (pc.range.end + 3).min(q.range.end) });
    }
    // Följer slutnoter (synliga definitioner) efter fotnoterna får den första luft över sig (skiljelinjen
    // ritas där), eftersom tomraden före fotnoterna är hopkrympt.
    let last = defs.iter().map(|d| d.para).max();
    if let Some(last) = last {
        let next = (last + 1..paras.len()).find(|&i| {
            let q = &paras[i];
            !q.range.is_empty() && text[q.range.clone()].starts_with("[^") && !q.collapse
        });
        if let Some(i) = next {
            paras[i].space_before += px(zoom, 12.0);
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
            for q in &mut paras[last + 1..] {
                q.collapse = true;
            }
        }
    }
    defs
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
) -> NoteLayout {
    let pv = Params {
        palette: p.palette,
        zoom: p.zoom,
        reveal: 0..0,
        matches: &[],
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
        let mut h = Fx(unit_key(env, text, a, &pv, w, std::slice::from_ref(&q)));
        (wrap_width.to_bits(), 0xF007u32).hash(&mut h);
        h.finish()
    };
    let tick = st.tick;
    let galley = match st.units.get_mut(&key) {
        Some(u) => {
            u.used = tick;
            u.galleys[0].clone()
        }
        None => {
            let mut job = LayoutJob { text: text[w.clone()].to_owned(), ..Default::default() };
            job.sections = build_sections(text, a, &pv, grid, w.clone());
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
    let label_ov = a
        .pieces
        .iter()
        .find(|pc| pc.footnote_def.as_deref() == Some(d.label.as_str()) && d.line.contains(&pc.range.start))
        .and_then(|pc| {
            let ov = note_label_ov(pc, a, &pv)?;
            let at = galley.pos_from_cursor(CCursor::new(text[w.start..pc.range.start].chars().count())).min;
            let wd = (pv.measure)(&ov.text, ov.font.clone());
            Some((ov, egui::pos2(at.x - wd, at.y)))
        });
    NoteLayout { label: d.label.clone(), galley, line: d.line.clone(), body: d.body..d.line.end, indent, label_ov }
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
    for pc in &a.pieces {
        let Some(l) = pc.footnote_ref.as_deref() else { continue };
        let Some(n) = notes.iter().position(|n| n.label == l) else { continue };
        chars += text[byte..pc.range.start].chars().count();
        byte = pc.range.start;
        spec.refs.push((g.layout_from_cursor(CCursor::new(chars)).row, n));
    }
    spec.refs.sort_by_key(|r| r.0);
    spec
}

/// Alla tecken dolda (för definitionsraden i flödet; texten ritas längst ned på sidan i stället).
pub(super) fn hide_all(job: &mut LayoutJob, family: FontFamily) {
    let len = job.text.len();
    job.sections = vec![LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(len), format: hidden_format(family) }];
}
