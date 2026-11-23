//! Markör och pekare vid förankrade textrutor (`<!--k:tb N …-->` … `<!--k:/tb-->`).
//!
//! Rutans rader ligger i galleriet mitt i texten men ritas bredvid brödtexten, så brödtextrader och
//! rutans rader överlappar i y. Eguis radsökning (`Galley::cursor_from_pos`, pil upp/ned) antar att
//! raderna ligger under varandra och kan då välja fel rad. Här görs sökningen i stället på raderna
//! som hör till rätt del: rutan eller brödtexten (samma mönster som `secpages::pointer_fix` för spalter).
//! Allt är rena funktioner över galleriet; `pointer_fix` och `KladdApp::word_tbox_keys` kopplar dem
//! till pekaren och tangenterna.

use std::ops::Range;

use egui::text::CharIndex;
use egui::{Context, Galley, Id, Rect, Vec2};

use super::grid::GalleyIndex;
use super::markup;
use crate::app::{KladdApp, char_to_byte, editor_id};
use crate::docx_tbox::{CLOSE, OPEN};

/// Rader som är dolda (markörrader, tomma rader): för låga för att vara ett mål för markören.
const MIN_ROW_H: f32 = 1.0;

/// Var en markör eller pekare står.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Body,
    /// Rutan med detta nummer (index i `Boxes`).
    Box(usize),
}

/// Textrutorna i dokumentet som radintervall i galleriet (öppningsraden till och med stängningsraden).
#[derive(Clone, Debug, Default)]
pub struct Boxes {
    pub rows: Vec<Range<usize>>,
    /// Öppningsradens byteläge, i samma ordning (för att para ihop med ritade rutor).
    pub opens: Vec<usize>,
}

impl Boxes {
    pub fn new(text: &str, g: &Galley, idx: &GalleyIndex) -> Boxes {
        let mut out = Boxes::default();
        let mut from = 0;
        while let Some(i) = text[from..].find(OPEN).map(|i| i + from) {
            from = i + OPEN.len();
            if i > 0 && !text[..i].ends_with('\n') {
                continue;
            }
            let Some(c) = text[from..].find(CLOSE).map(|c| c + from) else { break };
            let end = c + CLOSE.len();
            let r = idx.row_of(idx.chr(i))..(idx.row_of(idx.chr(end.saturating_sub(1))) + 1).min(g.rows.len());
            if !r.is_empty() {
                out.rows.push(r);
                out.opens.push(i);
            }
            from = end;
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Rutan som raden `row` hör till.
    pub fn zone_of(&self, row: usize) -> Zone {
        let k = self.rows.partition_point(|r| r.end <= row);
        match self.rows.get(k) {
            Some(r) if r.contains(&row) => Zone::Box(k),
            _ => Zone::Body,
        }
    }
}

fn visible(g: &Galley, i: usize) -> bool {
    g.rows[i].row.size.y > MIN_ROW_H
}

/// Synlig rad med text (inte avståndsraden mellan två stycken, som ligger i spaltens x även i en ruta).
fn content(g: &Galley, i: usize) -> bool {
    visible(g, i) && !g.rows[i].row.glyphs.is_empty()
}

/// Byteläget på raden `row` närmast x (galleri-koordinater).
fn place(text: &str, g: &Galley, idx: &GalleyIndex, row: usize, x: f32) -> usize {
    let r = &g.rows[row];
    let col = r.char_at(x - r.pos.x).0.min(r.glyphs.len());
    char_to_byte(text, idx.start(row) + col)
}

/// Raden i `rows` (galleriets radindex) vars höjd ligger närmast `y`; vid lika avstånd den första.
fn nearest(g: &Galley, rows: impl Iterator<Item = usize>, y: f32, ok: fn(&Galley, usize) -> bool) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for i in rows.filter(|&i| ok(g, i)) {
        let r = &g.rows[i];
        let d = if y < r.pos.y {
            r.pos.y - y
        } else if y > r.pos.y + r.row.size.y {
            y - (r.pos.y + r.row.size.y)
        } else {
            0.0
        };
        if best.is_none_or(|(bd, _)| d < bd - 0.01) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Byteläget under pekaren (`local` i galleri-koordinater). `rects` är rutornas skärmrutor (galleri-
/// koordinater, ihopparade med `Boxes::opens`). Utan `zone` avgör pekaren: ligger den i en ruta hör den till
/// den, annars till brödtexten; en pågående markering behåller sin del så att den inte hoppar mellan
/// rutans rader och brödtexten bredvid. `col` är spaltens (vänsterkant, bredd) för brödtext. `None` när
/// delen saknar rader.
pub fn hit(text: &str, g: &Galley, idx: &GalleyIndex, boxes: &Boxes, rects: &[(usize, Rect)], local: Vec2, zone: Option<Zone>, col: (f32, f32)) -> Option<(usize, Zone)> {
    let zone = zone.unwrap_or_else(|| {
        let k = boxes.opens.iter().position(|o| rects.iter().any(|(ro, r)| ro == o && r.contains(local.to_pos2())));
        k.map_or(Zone::Body, Zone::Box)
    });
    let row = match zone {
        Zone::Box(k) => nearest(g, boxes.rows.get(k)?.clone(), local.y, content)?,
        Zone::Body => {
            let (lo, hi) = (col.0 - 0.5, col.0 + col.1 + 0.5);
            let rows = (0..g.rows.len()).filter(|&i| boxes.zone_of(i) == Zone::Body && g.rows[i].pos.x >= lo && g.rows[i].pos.x < hi);
            nearest(g, rows, local.y, visible)?
        }
    };
    Some((place(text, g, idx, row, local.x), zone))
}

/// Byteläget efter pil upp/ned från `byte`. `None` = lämna åt TextEdit (inte vid en ruta).
/// I brödtexten hoppar markören över rutans rader till föregående/nästa synliga brödtextrad; i rutan rör
/// den sig mellan rutans rader och lämnar den vid kanterna. Läget i sidled (x) behålls.
pub fn vertical(text: &str, g: &Galley, boxes: &Boxes, byte: usize, up: bool) -> Option<usize> {
    if boxes.is_empty() || g.rows.is_empty() {
        return None;
    }
    let idx = GalleyIndex::new(text, g);
    let c = idx.chr(byte);
    let cur = idx.row_of(c).min(g.rows.len() - 1);
    let row = &g.rows[cur];
    let x = row.pos.x + row.row.x_offset(CharIndex(c.saturating_sub(idx.start(cur))));
    let zone = boxes.zone_of(cur);
    // Raden i samma del närmast före/efter `from` (rader i rutan och dolda rader räknas inte som brödtext).
    let step = |from: usize, zone: Zone| -> Option<usize> {
        let ok = |i: &usize| content(g, *i) && boxes.zone_of(*i) == zone;
        if up { (0..from).rev().find(ok) } else { (from + 1..g.rows.len()).find(ok) }
    };
    let target = match zone {
        Zone::Body => {
            // Bara om nästa rad åt det hållet (dolda rader oräknade) hör till en ruta: annars gör TextEdit rätt.
            let mut near = if up { (0..cur).rev().find(|&i| visible(g, i) || boxes.zone_of(i) != Zone::Body) } else { (cur + 1..g.rows.len()).find(|&i| visible(g, i) || boxes.zone_of(i) != Zone::Body) };
            if !near.take().is_some_and(|n| boxes.zone_of(n) != Zone::Body) {
                return None;
            }
            step(cur, Zone::Body)
        }
        Zone::Box(k) => {
            step(cur, zone).or_else(|| {
                let r = boxes.rows.get(k)?;
                step(if up { r.start } else { r.end - 1 }, Zone::Body)
            })
        }
    };
    // Ingen rad åt det hållet: stanna (TextEdit skulle söka bland rader som ligger bredvid).
    Some(target.map_or(byte, |t| place(text, g, &idx, t, x)))
}

/// Efter TextEdit: musklick och drag vid textrutor. TextEdit väljer rad efter y och kan då ta en
/// brödtextrad när pekaren står i rutan eller tvärtom; här sätts markören om från rätt del. Bara när
/// pekaren står i höjd med en ruta (annars är TextEdits träff rätt).
#[allow(clippy::too_many_arguments)]
pub fn pointer_fix(
    ctx: &Context,
    id: Id,
    text: &str,
    g: &Galley,
    gpos: egui::Pos2,
    rects: &[(usize, Rect)],
    col_at: &dyn Fn(f32, f32) -> (f32, f32),
    resp: &egui::Response,
    edge: f32,
) {
    if rects.is_empty() {
        return;
    }
    let key = Id::unique("kladd-tbox-drag");
    let (pressed, down, shift, ptr) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.modifiers.shift, i.pointer.interact_pos()));
    if !down {
        ctx.data_mut(|d| d.remove::<(usize, Option<Zone>)>(key));
    }
    let Some(ptr) = ptr else { return };
    let local = ptr - gpos;
    let anchor = ctx.data(|d| d.get_temp::<(usize, Option<Zone>)>(key));
    // Kanten av rutan är en träffyta för rutan själv (flytt och storlek).
    if rects.iter().any(|(_, r)| r.expand(edge).contains(ptr) && !r.shrink(edge).contains(ptr)) {
        return;
    }
    // Bara i höjd med en ruta.
    let local_rects: Vec<(usize, Rect)> = rects.iter().map(|(o, r)| (*o, r.translate(-gpos.to_vec2()))).collect();
    if !local_rects.iter().any(|(_, r)| local.y >= r.top() - 1.0 && local.y <= r.bottom() + 1.0) && anchor.is_none_or(|(_, z)| z.is_none()) {
        return;
    }
    let idx = GalleyIndex::new(text, g);
    let boxes = Boxes::new(text, g, &idx);
    let col = col_at(local.x, local.y);
    let find = |zone: Option<Zone>| hit(text, g, &idx, &boxes, &local_rects, local, zone, col).filter(|(b, _)| markup::table_at(text, *b).is_none());
    if pressed && resp.contains_pointer() {
        let Some((b, zone)) = find(None) else { return };
        let secondary = egui::text_edit::TextEditState::load(ctx, id).and_then(|s| s.cursor.char_range()).map(|r| char_to_byte(text, r.secondary.index.0));
        let from = if shift { secondary.unwrap_or(b) } else { b };
        super::tables::set_caret(ctx, id, text, from, b);
        ctx.data_mut(|d| d.insert_temp(key, (from, Some(zone))));
        ctx.request_repaint();
    } else if down {
        if let Some((from, zone)) = anchor {
            if let Some((b, _)) = find(zone) {
                super::tables::set_caret(ctx, id, text, from, b);
                ctx.request_repaint();
            }
        }
    }
}

impl KladdApp {
    /// Före TextEdit: pil upp/ned vid en textruta (se `vertical`). Skift utökar markeringen.
    pub(crate) fn word_tbox_keys(&mut self, ctx: &Context) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let id = editor_id(doc.id);
        if !ctx.memory(|m| m.has_focus(id)) || !doc.text.contains(OPEN) {
            return;
        }
        let doc_id = doc.id;
        let Some((g, _)) = self.word.table_view(doc_id, &doc.text) else { return };
        let events = ctx.input(|i| i.events.clone());
        let mut keep = Vec::with_capacity(events.len());
        let mut changed = false;
        for ev in events {
            let up = match &ev {
                egui::Event::Key { key: egui::Key::ArrowUp, pressed: true, modifiers, .. } if !modifiers.alt && !modifiers.command && !modifiers.ctrl => Some(true),
                egui::Event::Key { key: egui::Key::ArrowDown, pressed: true, modifiers, .. } if !modifiers.alt && !modifiers.command && !modifiers.ctrl => Some(false),
                _ => None,
            };
            let shift = matches!(&ev, egui::Event::Key { modifiers, .. } if modifiers.shift);
            let Some(up) = up else {
                keep.push(ev);
                continue;
            };
            let text = &self.docs[self.active].text;
            let state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
            let Some(range) = state.cursor.char_range() else {
                keep.push(ev);
                continue;
            };
            let idx = GalleyIndex::new(text, &g);
            let boxes = Boxes::new(text, &g, &idx);
            let (prim, sec) = (char_to_byte(text, range.primary.index.0), char_to_byte(text, range.secondary.index.0));
            // Med en markering går den utan skift till sin övre/nedre kant, som i TextEdit.
            let from = if !shift && prim != sec { if up { prim.min(sec) } else { prim.max(sec) } } else { prim };
            match vertical(text, &g, &boxes, from, up) {
                Some(b) => {
                    changed = true;
                    super::tables::set_caret(ctx, id, text, if shift { sec } else { b }, b);
                }
                None => keep.push(ev),
            }
        }
        if changed {
            ctx.input_mut(|i| i.events = keep);
            self.focus_editor = true;
        }
    }
}
