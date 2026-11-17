//! Tabellrutnät i dokumentläget: kolumnbredder, cellernas rader och pekarens träffyta.
//!
//! En tabellrad ligger i texten som en enda Markdown-rad, men läggs ut som ett rutnät: varje
//! cell blir ett eget galleri med radbredd = kolumnens bredd − cellmarginalen, och cellernas
//! rader placeras bredvid varandra (`PlacedRow::pos`). Raderna ligger kvar i källordning (celler
//! från vänster till höger, rad för rad) så att teckenräkningen stämmer med texten. Eftersom
//! cellernas rader överlappar i y duger inte `Galley::cursor_from_pos` och `cursor_up_one_row`
//! i tabeller; träffar och Upp/Ned räknas här i stället från samma geometri.
//!
//! `<br>` i en cell bryter raden: cellen läggs ut som ett galleri per del (delen slutar med den
//! dolda taggen) som staplas lodrätt.

use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::FontsView;
use egui::text::LayoutJob;
use egui::{FontId, Galley, Rect, Vec2, pos2};

use super::layout::{CELL_PAD_PT, EMPTY_COL_EM, Params, clip_job, is_hidden_format, px};
use super::markup::{self, Analysis};
use super::tablefmt::{self, TAlign, TableFmt, VAlign, Width};
use crate::app::char_to_byte;

/// En cell: kolumnen den börjar i, hur många kolumner den spänner över, hela texten den äger
/// (`chunk`, från pipan före cellen till pipan före nästa cell) och innehållet mellan pipor.
#[derive(Debug, Clone, PartialEq)]
pub struct CellGeom {
    pub col: usize,
    pub span: usize,
    pub chunk: Range<usize>,
    pub seg: Range<usize>,
}

/// En tabellrad (utan avgränsarraden).
#[derive(Debug, Clone, PartialEq)]
pub struct RowGeom {
    pub line: Range<usize>,
    pub cells: Vec<CellGeom>,
}

/// Tabellens geometri. Byteintervallen är relativa till fönstret tabellen lades ut i när den
/// ligger i cachen och absoluta efter `abs`.
#[derive(Debug, Clone, PartialEq)]
pub struct TableGeom {
    pub region: Range<usize>,
    /// Kolumngränsernas x, relativt galleriets vänsterkant (`kolumner + 1` värden).
    pub col_x: Vec<f32>,
    /// Rader i tabellens ordning: rubrikraden först, avgränsarraden utelämnad.
    pub rows: Vec<RowGeom>,
    pub delim: Option<Range<usize>>,
    /// Formatet ur kommentarsraden före tabellen (skuggning, ramar …), om den har någon.
    pub fmt: Option<Arc<TableFmt>>,
    /// Cellmarginal vänster, höger, överkant, nederkant (px).
    pub pad: [f32; 4],
    /// Lägsta radhöjd (px) per rad i `rows` (0 = ingen).
    pub min_h: Vec<f32>,
}

impl TableGeom {
    fn shift(&self, f: impl Fn(usize) -> usize) -> TableGeom {
        let r = |x: &Range<usize>| f(x.start)..f(x.end);
        TableGeom {
            region: r(&self.region),
            col_x: self.col_x.clone(),
            rows: self
                .rows
                .iter()
                .map(|row| RowGeom {
                    line: r(&row.line),
                    cells: row
                        .cells
                        .iter()
                        .map(|c| CellGeom { col: c.col, span: c.span, chunk: r(&c.chunk), seg: r(&c.seg) })
                        .collect(),
                })
                .collect(),
            delim: self.delim.as_ref().map(r),
            fmt: self.fmt.clone(),
            pad: self.pad,
            min_h: self.min_h.clone(),
        }
    }

    /// Radens nederkant när texten slutar vid `bottom`: minst radens egen höjd (`w:trHeight`).
    pub fn row_bottom(&self, row: usize, top: f32, bottom: f32) -> f32 {
        bottom.max(top + self.min_h.get(row).copied().unwrap_or(0.0))
    }

    /// Relativ till fönstret som börjar på `off`.
    pub fn rel(&self, off: usize) -> TableGeom {
        self.shift(|x| x - off)
    }

    /// Absolut, för ett fönster som börjar på `off`.
    pub fn abs(&self, off: usize) -> TableGeom {
        self.shift(|x| x + off)
    }

    pub fn cols(&self) -> usize {
        self.col_x.len().saturating_sub(1)
    }

    /// Cellens x-intervall (kolumngränserna).
    pub fn cell_x(&self, c: &CellGeom) -> (f32, f32) {
        let last = self.col_x.len() - 1;
        (self.col_x[c.col.min(last)], self.col_x[(c.col + c.span).min(last)])
    }

    /// Kolumnen (0-baserad) som x ligger i; utanför tabellen närmaste kolumn.
    pub fn col_at(&self, x: f32) -> usize {
        let n = self.cols().max(1);
        (1..n).find(|&j| x < self.col_x[j]).map_or(n - 1, |j| j - 1)
    }
}

/// Cellmarginal (px).
pub fn cell_pad(zoom: f32) -> f32 {
    px(zoom, CELL_PAD_PT)
}

// ------------------------------------------------------------------ plan

/// Delarna av cellen `seg` som `<br>` skiljer åt (utan taggarna), för breddmätning.
fn br_parts(text: &str, seg: &Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut at = 0;
    for t in markup::br_tags(&text[seg.clone()]) {
        out.push(seg.start + at..seg.start + t.start);
        at = t.end;
    }
    out.push(seg.start + at..seg.end);
    out
}

/// Visuell text för mätning: bitar utan markup, trimmade.
fn visible(text: &str, a: &Analysis, r: &Range<usize>) -> String {
    let i = a.pieces.partition_point(|p| p.range.end <= r.start);
    let mut s = String::new();
    for pc in &a.pieces[i..] {
        if pc.range.start >= r.end {
            break;
        }
        if !pc.markup {
            s.push_str(&text[pc.range.start.max(r.start)..pc.range.end.min(r.end)]);
        }
    }
    s.trim().to_owned()
}

/// Fördelar `avail` på kolumnerna: de som ryms på sin naturliga bredd får den, resten delar på
/// det som blir över.
fn fit(nat: &[f32], avail: f32, min: &[f32]) -> Vec<f32> {
    if nat.iter().sum::<f32>() <= avail {
        return nat.to_vec();
    }
    let n = nat.len();
    let mut w = nat.to_vec();
    let mut fixed = vec![false; n];
    let mut rem = avail;
    loop {
        let free: Vec<usize> = (0..n).filter(|&j| !fixed[j]).collect();
        if free.is_empty() {
            break;
        }
        let share = rem / free.len() as f32;
        if let Some(&j) = free.iter().find(|&&j| nat[j] <= share) {
            fixed[j] = true;
            w[j] = nat[j];
            rem -= nat[j];
            continue;
        }
        for &j in &free {
            w[j] = share.max(min[j]);
        }
        break;
    }
    w
}

/// Tabellens geometri (absoluta byteintervall). `lines` är tabellens rader i texten
/// (rubrik, avgränsare, brödrader). Kolumnbredderna kommer från avgränsarradens streck när de
/// skiljer sig åt (hela textbredden fördelas), annars från innehållet; blir tabellen bredare än
/// `wrap_width` bryts långa celler.
pub fn plan(text: &str, a: &Analysis, lines: &[Range<usize>], p: &Params, wrap_width: f32) -> TableGeom {
    let b = text.as_bytes();
    let z = p.zoom;
    let fmt = lines.first().and_then(|l| tablefmt::before(text, l.start)).map(|(_, f)| Arc::new(f));
    let pads: [f32; 4] = match fmt.as_ref() {
        Some(f) => [px(z, f.pad[0]), px(z, f.pad[1]), px(z, f.pad[2]), px(z, f.pad[3])],
        None => [cell_pad(z), cell_pad(z), 0.0, 0.0],
    };
    let pad = (pads[0] + pads[1]) / 2.0;
    let header_bold = fmt.as_ref().is_none_or(|f| f.header_bold);
    let size = px(z, a.defaults.size());
    let mut rows: Vec<RowGeom> = Vec::new();
    let mut delim = None;
    let mut ncols = 1;
    for (i, line) in lines.iter().enumerate() {
        let mut s = line.start;
        while s < line.end && matches!(b[s], b' ' | b'\t' | b'>') {
            s += 1;
        }
        let (segs, _, _) = markup::table_line_cells(text, s, line.end);
        ncols = ncols.max(segs.len());
        if i == 1 {
            delim = Some(line.clone());
            continue;
        }
        let mut cells = Vec::new();
        if segs.is_empty() {
            cells.push(CellGeom { col: 0, span: 1, chunk: line.clone(), seg: line.clone() });
        } else {
            let spans = markup::cell_spans(&segs);
            let starts: Vec<usize> = (0..segs.len()).map(|k| if k == 0 { line.start } else { segs[k].start - 1 }).collect();
            let owners: Vec<usize> = (0..segs.len()).filter(|&k| spans[k] > 0).collect();
            for (n, &k) in owners.iter().enumerate() {
                let end = owners.get(n + 1).map_or(line.end, |&k2| starts[k2]);
                cells.push(CellGeom { col: k, span: spans[k], chunk: starts[k]..end, seg: segs[k].clone() });
            }
        }
        rows.push(RowGeom { line: line.clone(), cells });
    }

    let avail = if wrap_width.is_finite() && wrap_width > 0.0 { wrap_width } else { f32::INFINITY };
    let weights = delim.as_ref().and_then(|d| markup::parse_weights(&text[d.clone()])).filter(|w| w.len() == ncols);
    // Tabellens egen bredd (`w:tblW`): procent av textbredden eller fast, aldrig bredare än spalten.
    let target = fmt.as_ref().and_then(|f| match f.width {
        Width::Pct(pc) if avail.is_finite() => Some(avail * f32::from(pc) / 100.0),
        Width::Pt(w) => Some(if avail.is_finite() { px(z, f32::from(w)).min(avail) } else { px(z, f32::from(w)) }),
        _ => None,
    });
    let widths: Vec<f32> = match (&weights, target) {
        (Some(w), t) if t.is_some() || avail.is_finite() => {
            let total = t.unwrap_or(avail);
            let sum: usize = w.iter().sum();
            w.iter().map(|&x| (total * x as f32 / sum as f32).max(2.0 * pad + 4.0)).collect()
        }
        (None, Some(total)) => vec![(total / ncols as f32).max(2.0 * pad + 4.0); ncols],
        _ => {
            // Naturlig bredd: det bredaste cellinnehållet (en rad i taget) per kolumn.
            let mut content = vec![0.0f32; ncols];
            let mut spanned: Vec<(usize, usize, f32)> = Vec::new();
            for (ri, row) in rows.iter().enumerate() {
                let font = FontId::new(size, (p.font_family)(Some(a.defaults.font()), ri == 0 && header_bold, false));
                for c in &row.cells {
                    let w = br_parts(text, &c.seg)
                        .iter()
                        .map(|r| {
                            let v = visible(text, a, r);
                            if v.is_empty() { 0.0 } else { (p.measure)(&v, font.clone()) }
                        })
                        .fold(0.0, f32::max);
                    if c.span == 1 {
                        content[c.col] = content[c.col].max(w);
                    } else {
                        spanned.push((c.col, c.span, w));
                    }
                }
            }
            for (col, span, w) in spanned {
                // Cellens bredd är innehållet plus marginal i alla kolumner den täcker.
                let have: f32 = (col..(col + span).min(ncols)).map(|j| content[j]).sum::<f32>() + (span - 1) as f32 * 2.0 * pad;
                if have < w {
                    let add = (w - have) / span as f32;
                    for c in content.iter_mut().skip(col).take(span) {
                        *c += add;
                    }
                }
            }
            let nat: Vec<f32> = content.iter().map(|&w| (if w == 0.0 { EMPTY_COL_EM * size } else { w }) + 2.0 * pad).collect();
            let min: Vec<f32> = nat.iter().map(|&w| w.min(3.0 * size + 2.0 * pad)).collect();
            fit(&nat, avail, &min)
        }
    };
    // Tabellens läge i sidled: justering (`w:jc`) och indrag (`w:tblInd`).
    let total: f32 = widths.iter().sum();
    let x0 = match fmt.as_ref().map(|f| f.align) {
        Some(TAlign::Center) if avail.is_finite() => ((avail - total) / 2.0).max(0.0),
        Some(TAlign::Right) if avail.is_finite() => (avail - total).max(0.0),
        _ => fmt.as_ref().map_or(0.0, |f| px(z, f.indent)),
    };
    let mut col_x = vec![x0; ncols + 1];
    for j in 0..ncols {
        col_x[j + 1] = col_x[j] + widths[j];
    }
    let min_h = (0..rows.len())
        .map(|r| fmt.as_ref().and_then(|f| f.rows.get(&r)).map_or(0.0, |rf| px(z, f32::from(rf.h))))
        .collect();
    TableGeom {
        region: lines.first().map_or(0..0, |l| l.start..lines.last().map_or(l.end, |m| m.end)),
        col_x,
        rows,
        delim,
        fmt,
        pad: pads,
        min_h,
    }
}

// ------------------------------------------------------------------ layout

/// Lägger ut tabellraden `row` (index i `geom.rows`) som ett galleri. `full` är fönstrets jobb,
/// `off` fönstrets början i `geom`s koordinater och `line` radens intervall relativt fönstret.
/// `min_h` är radhöjden för en rad utan synlig text.
#[allow(clippy::too_many_arguments)]
pub fn layout_line(
    fonts: &mut FontsView<'_>,
    full: &LayoutJob,
    off: usize,
    geom: &TableGeom,
    row: usize,
    line: Range<usize>,
    ppp: f32,
    _p: &Params,
    min_h: f32,
) -> Arc<Galley> {
    let (padl, padr, padt, padb) = (geom.pad[0], geom.pad[1], geom.pad[2], geom.pad[3]);
    let rg = &geom.rows[row];
    let round = |v: f32| (v * ppp).round() / ppp;
    let mut cell_galleys: Vec<Arc<Galley>> = Vec::new();
    let mut xs: Vec<f32> = Vec::new();
    for c in &rg.cells {
        let (x0, x1) = geom.cell_x(c);
        // En pixel marginal: bredden mättes utan radbrytning och får inte brytas av avrundning.
        let wrap = (x1 - x0 - padl - padr + 1.0).max(8.0);
        let (chunk_s, chunk_e) = (c.chunk.start - off, c.chunk.end - off);
        let seg_text = &full.text[c.seg.start - off..c.seg.end - off];
        // Delar: varje del slutar med sin `<br>`.
        let mut cuts = vec![chunk_s];
        for t in markup::br_tags(seg_text) {
            cuts.push(c.seg.start - off + t.end);
        }
        cuts.push(chunk_e);
        cuts.dedup();
        let mut parts: Vec<Arc<Galley>> = Vec::new();
        for w in cuts.windows(2) {
            let mut j = clip_job(full, w[0]..w[1]);
            j.wrap.max_width = wrap;
            if j.sections.iter().all(|s| is_hidden_format(&s.format)) {
                for s in &mut j.sections {
                    s.format.line_height = Some(min_h);
                }
            }
            parts.push(fonts.layout_job(j));
        }
        let g = if parts.len() == 1 {
            parts.pop().unwrap_or_else(|| unreachable!())
        } else {
            Arc::new(Galley::concat(Arc::new(clip_job(full, chunk_s..chunk_e)), &parts, ppp))
        };
        cell_galleys.push(g);
        xs.push(x0 + padl);
    }
    let mut g = Galley::concat(Arc::new(clip_job(full, line)), &cell_galleys, ppp);
    // Radens höjd: den högsta cellen, minst `w:trHeight`; lodrät justering flyttar cellens rader.
    let tallest = cell_galleys.iter().map(|c| c.rect.height()).fold(0.0f32, f32::max) + padt + padb;
    let row_h = tallest.max(geom.min_h.get(row).copied().unwrap_or(0.0));
    let valign = |ci: usize| geom.fmt.as_ref().map_or(VAlign::Top, |f| f.valign(row, rg.cells[ci].col));
    // Samla cellernas rader på rätt plats: x efter kolumnen, y från cellens egen överkant.
    let mut i = 0;
    for (ci, (cg, x)) in cell_galleys.iter().zip(&xs).enumerate() {
        let inner = row_h - padt - padb;
        let dy = padt
            + match valign(ci) {
                VAlign::Top => 0.0,
                VAlign::Center => ((inner - cg.rect.height()) / 2.0).max(0.0),
                VAlign::Bottom => (inner - cg.rect.height()).max(0.0),
            };
        let avail_w = geom.cell_x(&rg.cells[ci]).1 - geom.cell_x(&rg.cells[ci]).0 - padl - padr;
        let ha = geom.fmt.as_ref().map_or(TAlign::Left, |f| f.cell(row, rg.cells[ci].col).map_or(TAlign::Left, |c| c.halign));
        for r in &cg.rows {
            let pr = &mut g.rows[i];
            // Cellens justering: raden flyttas i sidled efter sin synliga bredd.
            let dx = match ha {
                TAlign::Left => 0.0,
                TAlign::Center => ((avail_w - r.size.x) / 2.0).max(0.0),
                TAlign::Right => (avail_w - r.size.x).max(0.0),
            };
            pr.pos = pos2(round(x + dx + r.pos.x), round(r.pos.y + dy));
            pr.ends_with_newline = false;
            i += 1;
        }
    }
    g.rect = Rect::ZERO;
    g.mesh_bounds = Rect::NOTHING;
    for pr in &g.rows {
        g.rect |= pr.rect();
        g.mesh_bounds |= pr.row.visuals.mesh_bounds.translate(pr.pos.to_vec2());
    }
    // Nederkantsmarginal och lägsta radhöjd: concat räknar bara radernas mått, så den sista raden
    // (den som når längst ned) görs så hög att raden fyller `row_h`.
    let bottom = g.rows.iter().map(|r| r.pos.y + r.row.size.y).fold(0.0f32, f32::max);
    if row_h > bottom + 0.01 {
        if let Some(pr) = g.rows.iter_mut().rev().find(|r| (r.pos.y + r.row.size.y - bottom).abs() < 0.01) {
            Arc::make_mut(&mut pr.row).size.y += row_h - bottom;
        }
        for pr in &g.rows {
            g.rect |= pr.rect();
        }
    }
    Arc::new(g)
}

// ------------------------------------------------------------------ geometri i det färdiga galleriet

/// Tecknet varje rad i galleriet börjar på, och byte -> tecken för texten (räknar från förra
/// frågan så att frågor nära varandra är billiga).
pub struct GalleyIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
    last: std::cell::Cell<(usize, usize)>,
}

impl<'a> GalleyIndex<'a> {
    pub fn new(text: &'a str, g: &Galley) -> Self {
        let mut starts = Vec::with_capacity(g.rows.len());
        let mut c = 0;
        for r in &g.rows {
            starts.push(c);
            c += r.char_count_including_newline().0;
        }
        Self { text, starts, last: std::cell::Cell::new((0, 0)) }
    }

    /// Teckenindex för byteläget `b`.
    pub fn chr(&self, b: usize) -> usize {
        let b = b.min(self.text.len());
        let (lb, lc) = self.last.get();
        let c = if b >= lb {
            lc + self.text[lb..b].chars().count()
        } else {
            lc - self.text[b..lb].chars().count()
        };
        self.last.set((b, c));
        c
    }

    /// Raden som tecknet `c` börjar på (vid en radgräns den senare raden).
    pub fn row_of(&self, c: usize) -> usize {
        self.starts.partition_point(|&s| s <= c).saturating_sub(1)
    }

    /// Raderna som täcker tecknen `c0..c1`.
    pub fn rows(&self, c0: usize, c1: usize) -> Range<usize> {
        let a = self.row_of(c0);
        let b = self.row_of(c1.saturating_sub(1).max(c0));
        a..b + 1
    }

    pub fn start(&self, row: usize) -> usize {
        self.starts[row]
    }
}

/// Överkant och nederkant för raderna `rows` i galleriet.
pub fn extent(g: &Galley, rows: Range<usize>) -> (f32, f32) {
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for r in &g.rows[rows] {
        top = top.min(r.pos.y);
        bottom = bottom.max(r.pos.y + r.row.size.y);
    }
    (top, bottom)
}

/// Rad (överkant, nederkant) för tabellens rad `row`.
pub fn row_extent(g: &Galley, idx: &GalleyIndex, t: &TableGeom, row: usize) -> (f32, f32) {
    let l = &t.rows[row].line;
    extent(g, idx.rows(idx.chr(l.start), idx.chr(l.end)))
}

/// Cellens rader i galleriet.
pub fn cell_rows(idx: &GalleyIndex, c: &CellGeom) -> Range<usize> {
    idx.rows(idx.chr(c.chunk.start), idx.chr(c.chunk.end))
}

/// Cellen på raden `r` som täcker kolumn `j` (eller den sista om raden är kortare).
fn cell_in_col(t: &TableGeom, r: usize, j: usize) -> Option<&CellGeom> {
    let row = &t.rows[r];
    row.cells.iter().find(|c| c.col <= j && j < c.col + c.span).or(row.cells.last())
}

/// Byteläget i raden `row` (galleriets radindex) närmast x (galleri-koordinater). Mitt i en cell
/// med radbrytningar hålls läget före den dolda `<br>`-taggen på radens slut.
fn place(g: &Galley, idx: &GalleyIndex, row: usize, x: f32, cell: &CellGeom, cr: &Range<usize>) -> usize {
    let r = &g.rows[row];
    let col = r.char_at(x - r.pos.x).0.min(r.glyphs.len());
    let mut b = char_to_byte(idx.text, idx.start(row) + col);
    if row + 1 < cr.end && col == r.glyphs.len() {
        let seg = &idx.text[cell.seg.clone()];
        if let Some(t) = markup::br_tags(seg).into_iter().find(|t| cell.seg.start + t.end == b) {
            b = cell.seg.start + t.start;
        }
    }
    b.clamp(cell.seg.start, cell.seg.end)
}

/// Byteläget under pekaren `pos` (galleri-koordinater) om den är inom tabellen `t`:
/// cellen under pekaren, sedan tecknet närmast i cellens egna rader.
pub fn hit(g: &Galley, idx: &GalleyIndex, t: &TableGeom, pos: Vec2) -> Option<usize> {
    if t.rows.is_empty() {
        return None;
    }
    let exts: Vec<(f32, f32)> = (0..t.rows.len()).map(|r| row_extent(g, idx, t, r)).collect();
    if pos.y < exts[0].0 || pos.y > exts[exts.len() - 1].1 {
        return None;
    }
    let r = exts.iter().rposition(|e| e.0 <= pos.y).unwrap_or(0);
    let cell = cell_in_col(t, r, t.col_at(pos.x))?;
    let cr = cell_rows(idx, cell);
    let row = cr.clone().rev().find(|&i| g.rows[i].pos.y <= pos.y).unwrap_or(cr.start);
    Some(place(g, idx, row, pos.x, cell, &cr))
}

/// Tabellen (index i `geoms`), raden och cellen som innehåller byteläget.
fn locate(geoms: &[TableGeom], byte: usize) -> Option<(usize, usize, usize)> {
    let ti = geoms.iter().position(|t| t.region.start <= byte && byte <= t.region.end)?;
    let t = &geoms[ti];
    let r = t.rows.iter().position(|row| row.line.start <= byte && byte <= row.line.end)?;
    let cells = &t.rows[r].cells;
    let k = cells.iter().position(|c| byte < c.chunk.end).unwrap_or(cells.len().checked_sub(1)?);
    Some((ti, r, k))
}

/// Upp/Ned i en tabell: raden över (under) i samma cell, annars cellen över (under) i samma
/// kolumn och sist raden utanför tabellen. `None` om markören inte står i en tabellcell;
/// `Some(byte)` är nya läget (oförändrat om det inte finns något att flytta till).
pub fn vertical(text: &str, g: &Galley, geoms: &[TableGeom], byte: usize, up: bool) -> Option<usize> {
    let (ti, r, k) = locate(geoms, byte)?;
    let t = &geoms[ti];
    let cell = &t.rows[r].cells[k];
    let idx = GalleyIndex::new(text, g);
    let cr = cell_rows(&idx, cell);
    let c = idx.chr(byte);
    let cur = idx.row_of(c).clamp(cr.start, cr.end - 1);
    let row = &g.rows[cur];
    let x = row.pos.x + row.row.x_offset(egui::text::CharIndex(c.saturating_sub(idx.start(cur))));
    if up && cur > cr.start {
        return Some(place(g, &idx, cur - 1, x, cell, &cr));
    }
    if !up && cur + 1 < cr.end {
        return Some(place(g, &idx, cur + 1, x, cell, &cr));
    }
    let target = if up { r.checked_sub(1) } else { (r + 1 < t.rows.len()).then_some(r + 1) };
    if let Some(r2) = target {
        let cell2 = cell_in_col(t, r2, t.col_at(x))?;
        let cr2 = cell_rows(&idx, cell2);
        let row2 = if up { cr2.end - 1 } else { cr2.start };
        return Some(place(g, &idx, row2, x, cell2, &cr2));
    }
    // Ut ur tabellen: raden över första/under sista raden, om det finns någon.
    let first = idx.rows(idx.chr(t.rows[0].line.start), idx.chr(t.rows[0].line.end)).start;
    let last = idx.rows(idx.chr(t.rows[r].line.start), idx.chr(t.rows[r].line.end)).end;
    if (up && first == 0) || (!up && last >= g.rows.len()) {
        return Some(byte);
    }
    let (y0, y1) = row_extent(g, &idx, t, r);
    let y = if up { y0 - 2.0 } else { y1 + 2.0 };
    let ci = g.cursor_from_pos(Vec2::new(x, y)).index.0;
    Some(char_to_byte(text, ci))
}

