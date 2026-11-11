//! Bygger en `LayoutJob` som får Markdown-texten i en `TextEdit` att se ut som ett Word-dokument.
//!
//! Jobbets text är exakt Markdown-strängen (TextEdit-bufferten), och sektionerna täcker den
//! utan glapp. Markup (`**`, `# `, `](url)` …) döljs genom att göras transparent och 1 px
//! hög utom där markören står (`Params::reveal`), där den visas dämpad.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat};
use egui::{Color32, FontFamily, FontId, Stroke, Vec2};

use super::markup::{self, Analysis, Block, Piece};
use crate::highlight;
use crate::theme::{self, Palette};

pub const PAGE_W_PT: f32 = 595.0; // A4
pub const PAGE_H_PT: f32 = 842.0;
pub const MARGIN_PT: f32 = 72.0; // 1 tum

/// Punkter till pixlar vid zoom (96 dpi).
pub fn px(zoom: f32, pt: f32) -> f32 {
    pt * 96.0 / 72.0 * zoom
}

pub struct Params<'a> {
    pub palette: &'a Palette,
    pub zoom: f32,
    /// Byteintervall där markup visas (markörens rader); tomt = dölj all markup.
    pub reveal: Range<usize>,
    pub query: &'a str,
    pub case_sensitive: bool,
    pub current_match: Option<usize>,
    /// Bildkälla -> visningsstorlek i px (redan zoomad).
    pub image_size: &'a dyn Fn(&str) -> Option<Vec2>,
    /// Textbredd i px för en sträng i ett typsnitt (för tabellkolumner och listindrag).
    pub measure: &'a dyn Fn(&str, FontId) -> f32,
}

#[derive(Default)]
pub struct Cache {
    analyses: HashMap<u64, (u64, Arc<Analysis>)>,
    jobs: HashMap<u64, (u64, LayoutJob)>,
}

fn hash_of<T: Hash + ?Sized>(v: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

impl Cache {
    pub fn analysis(&mut self, doc_id: u64, text: &str) -> Arc<Analysis> {
        let th = hash_of(text);
        if let Some((k, a)) = self.analyses.get(&doc_id) {
            if *k == th {
                return a.clone();
            }
        }
        let a = Arc::new(markup::analyze(text));
        self.analyses.insert(doc_id, (th, a.clone()));
        a
    }

    /// Nyckeln: texthash, reveal, zoom, dark, query, case_sensitive, current_match.
    pub fn job(&mut self, doc_id: u64, text: &str, p: &Params) -> LayoutJob {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        p.reveal.start.hash(&mut h);
        p.reveal.end.hash(&mut h);
        p.zoom.to_bits().hash(&mut h);
        p.palette.dark.hash(&mut h);
        p.query.hash(&mut h);
        p.case_sensitive.hash(&mut h);
        p.current_match.hash(&mut h);
        let key = h.finish();

        if let Some((k, job)) = self.jobs.get(&doc_id) {
            if *k == key {
                return job.clone();
            }
        }
        let a = self.analysis(doc_id, text);
        let job = layout_job(text, &a, p);
        self.jobs.insert(doc_id, (key, job.clone()));
        job
    }

    pub fn forget(&mut self, doc_id: u64) {
        self.analyses.remove(&doc_id);
        self.jobs.remove(&doc_id);
    }
}

/// Storlek (pt) för blockets text.
fn block_size_pt(b: Block, mono: bool) -> f32 {
    if mono {
        return 10.0;
    }
    match b {
        Block::Heading(1) => 18.0,
        Block::Heading(2) => 15.0,
        Block::Heading(3) => 13.0,
        Block::Heading(4) => 12.0,
        _ => 11.0,
    }
}

fn intersects(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// Dold markup får den minsta teckenstorlek som typsnittet klarar: tecknen får då i praktiken
/// bredd noll (mätt: 25 tecken < 1 px) men finns kvar för markören.
pub const HIDDEN_PX: f32 = 0.01;

/// Cellmarginal i tabeller (pt).
const CELL_PAD_PT: f32 = 5.4;

fn hidden_format(family: FontFamily) -> TextFormat {
    TextFormat {
        font_id: FontId::new(HIDDEN_PX, family),
        color: Color32::TRANSPARENT,
        line_height: None,
        ..Default::default()
    }
}

/// Format för en bit. `hide` = bitens markup ska döljas. `grid` = bitens tabell visas som rutnät
/// (`Some(true)` = rubrikraden).
fn piece_format(pc: &Piece, hide: bool, grid: Option<bool>, p: &Params) -> TextFormat {
    let pal = p.palette;
    let zoom = p.zoom;
    let table_raw = pc.block == Block::Table && grid.is_none();
    let mono = pc.code || matches!(pc.block, Block::Code) || table_raw;
    let heading = matches!(pc.block, Block::Heading(_));
    let size = px(zoom, block_size_pt(pc.block, mono));
    let lh = if heading { size * 1.3 } else { size * 1.4 };
    let family_plain = if mono { FontFamily::Monospace } else { theme::doc_font(false, false) };

    if pc.markup {
        if hide {
            // Punktmarkören behåller brödtextens radhöjd så att en tom punkt inte kollapsar.
            if pc.list_marker {
                let mut f = hidden_format(theme::doc_font(false, false));
                f.line_height = Some(px(zoom, 11.0) * 1.4);
                return f;
            }
            return hidden_format(family_plain);
        }
        return TextFormat {
            font_id: FontId::new(size, family_plain),
            color: pal.page_markup,
            line_height: Some(lh),
            ..Default::default()
        };
    }

    let header = grid == Some(true);
    let bold = pc.bold || heading || header;
    let italic = pc.italic || matches!(pc.block, Block::Quote);
    let family = if mono { FontFamily::Monospace } else { theme::doc_font(bold, italic) };
    let mut color = match pc.block {
        Block::Heading(1) => pal.page_heading1,
        Block::Heading(6) => pal.page_muted,
        Block::Heading(_) => pal.page_heading,
        Block::Quote => pal.page_muted,
        _ => pal.page_text,
    };
    if pc.link {
        color = pal.page_link;
    }
    let mut f = TextFormat {
        font_id: FontId::new(size, family),
        line_height: Some(lh),
        color,
        ..Default::default()
    };
    if pc.code && pc.block != Block::Table {
        f.background = pal.page_code_bg;
    }
    let stroke = Stroke::new(zoom.max(1.0), color);
    if pc.underline || pc.link {
        f.underline = stroke;
    }
    if pc.strike {
        f.strikethrough = stroke;
    }
    f
}

/// Radbrytning som är hela innehållet på en tom rad: ska bli Words "space after" på 8 pt.
fn spacing_format(zoom: f32) -> TextFormat {
    TextFormat {
        line_height: Some(px(zoom, 8.0)),
        ..hidden_format(theme::doc_font(false, false))
    }
}

// ------------------------------------------------------------------ listor

/// Nivå för ett listobjekt: ett tabbtecken eller fyra blanksteg per nivå i radens indrag.
pub fn list_level(text: &str, marker_start: usize) -> usize {
    let ls = text[..marker_start].rfind('\n').map_or(0, |i| i + 1);
    let mut spaces = 0;
    let mut level = 0;
    for c in text[ls..marker_start].chars() {
        match c {
            '\t' => {
                level += 1;
                spaces = 0;
            }
            ' ' => {
                spaces += 1;
                if spaces == 4 {
                    level += 1;
                    spaces = 0;
                }
            }
            _ => break,
        }
    }
    level
}

/// Textens startposition (px från marginalen) för ett listobjekt på nivå `level`: Words
/// punktlista har texten 0,5" in och sedan 0,25" till per nivå.
pub fn list_text_x(zoom: f32, level: usize) -> f32 {
    px(zoom, 18.0 * (level as f32 + 2.0))
}

// ------------------------------------------------------------------ tabeller

/// En tabellrad: radens byteintervall (utan radbrytning) och byteläget för varje `|`.
#[derive(Debug, Clone)]
pub struct TableRow {
    pub line: Range<usize>,
    pub pipes: Vec<usize>,
    /// Avgränsarraden (`| --- | --- |`), som döljs helt.
    pub delimiter: bool,
}

#[derive(Debug, Clone)]
pub struct Table {
    /// Från första till sista tabellbiten.
    pub region: Range<usize>,
    pub rows: Vec<TableRow>,
}

fn table_pipes(text: &str, line: &Range<usize>) -> Vec<usize> {
    let b = text.as_bytes();
    let mut p = line.start;
    while p < line.end && matches!(b[p], b' ' | b'\t' | b'>') {
        p += 1;
    }
    let mut in_code = false;
    let mut pipes = Vec::new();
    for i in p..line.end {
        match b[i] {
            b'`' => in_code = !in_code,
            b'|' if !in_code && (i == 0 || b[i - 1] != b'\\') => pipes.push(i),
            _ => {}
        }
    }
    pipes
}

/// Alla tabeller i texten (sammanhängande tabellbitar), rad för rad.
pub fn tables(text: &str, a: &Analysis) -> Vec<Table> {
    let mut regions: Vec<Range<usize>> = Vec::new();
    let mut cur: Option<Range<usize>> = None;
    for pc in &a.pieces {
        if pc.block == Block::Table {
            match &mut cur {
                Some(r) if r.end == pc.range.start => r.end = pc.range.end,
                _ => {
                    regions.extend(cur.take());
                    cur = Some(pc.range.clone());
                }
            }
        } else {
            regions.extend(cur.take());
        }
    }
    regions.extend(cur);
    let mut out = Vec::new();
    for region in regions {
        let mut rows = Vec::new();
        let mut pos = region.start;
        let mut idx = 0;
        while pos < region.end {
            let le = text[pos..region.end].find('\n').map_or(region.end, |i| pos + i);
            if le > pos {
                let line = pos..le;
                let pipes = table_pipes(text, &line);
                rows.push(TableRow { line, pipes, delimiter: idx == 1 });
            }
            idx += 1;
            pos = le + 1;
        }
        out.push(Table { region, rows });
    }
    out
}

/// Utvidgar `reveal` så att en tabell som markören står i visas helt (rå Markdown).
pub fn reveal_whole_tables(tables: &[Table], reveal: Range<usize>) -> Range<usize> {
    if reveal.is_empty() {
        return reveal;
    }
    let mut r = reveal;
    for t in tables {
        if intersects(&t.region, &r) {
            r.start = r.start.min(t.region.start);
            r.end = r.end.max(t.region.end);
        }
    }
    r
}

/// Sektionsbyggare: delar bitar vid `cuts` och lägger över `marks` (egna format för intervall).
struct Builder {
    secs: Vec<(Range<usize>, TextFormat)>,
    cuts: BTreeSet<usize>,
    marks: Vec<(Range<usize>, TextFormat)>,
}

impl Builder {
    fn push(&mut self, r: Range<usize>, f: &TextFormat) {
        let mut start = r.start;
        let cuts: Vec<usize> = self.cuts.range(r.start + 1..r.end).copied().collect();
        for end in cuts.into_iter().chain([r.end]) {
            self.push_marked(start..end, f);
            start = end;
        }
    }

    fn push_marked(&mut self, r: Range<usize>, f: &TextFormat) {
        let mut pos = r.start;
        let i = self.marks.partition_point(|m| m.0.end <= r.start);
        for (mr, mf) in &self.marks[i..] {
            if mr.start >= r.end {
                break;
            }
            let (s, e) = (mr.start.max(pos), mr.end.min(r.end));
            if pos < s {
                self.secs.push((pos..s, f.clone()));
            }
            self.secs.push((s..e, mf.clone()));
            pos = e;
        }
        if pos < r.end {
            self.secs.push((pos..r.end, f.clone()));
        }
    }
}

/// Cellens synliga text (utan markup), för breddmätning.
fn visible_text(text: &str, a: &Analysis, r: &Range<usize>) -> String {
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

struct Cell {
    start: usize,
    end: usize,
    w: f32,
}

/// Rutnätslayout för en tabell som inte visas rått: kolumnbredder mäts med `p.measure`, och
/// pipor och celltext får `leading_space` så att kolumnerna linjerar. Pipor, blanksteg i
/// cellerna och avgränsarraden döljs (avgränsarraden kollapsar till 1 px höjd).
fn grid_table(
    text: &str,
    a: &Analysis,
    t: &Table,
    p: &Params,
    bld: &mut Builder,
    leads: &mut BTreeMap<usize, f32>,
) {
    let z = p.zoom;
    let pad = px(z, CELL_PAD_PT);
    let size = px(z, 11.0);
    let mut rows: Vec<(&TableRow, Vec<Cell>)> = Vec::new();
    let mut ncols = 0;
    for (ri, row) in t.rows.iter().enumerate() {
        if row.delimiter {
            continue;
        }
        let font = FontId::new(size, theme::doc_font(ri == 0, false));
        let mut cells = Vec::new();
        for (j, &pp) in row.pipes.iter().enumerate() {
            let start = pp + 1;
            let end = row.pipes.get(j + 1).copied().unwrap_or(row.line.end);
            if j + 1 == row.pipes.len() && text[start..end].trim().is_empty() {
                break; // avslutande pipan
            }
            let vis = visible_text(text, a, &(start..end));
            let w = if vis.is_empty() { 0.0 } else { (p.measure)(&vis, font.clone()) };
            cells.push(Cell { start, end, w });
        }
        ncols = ncols.max(cells.len());
        rows.push((row, cells));
    }
    let mut col_x = vec![0.0f32; ncols + 1];
    for j in 0..ncols {
        let w = rows.iter().filter_map(|(_, c)| c.get(j)).map(|c| c.w).fold(0.0, f32::max);
        col_x[j + 1] = col_x[j] + w + 2.0 * pad;
    }
    let hid = hidden_format(theme::doc_font(false, false));
    for (row, cells) in &rows {
        for (j, c) in cells.iter().enumerate() {
            let s = &text[c.start..c.end];
            let lead_ws = s.len() - s.trim_start().len();
            let trail_ws = s.len() - s.trim_end().len();
            if lead_ws == s.len() {
                if !s.is_empty() {
                    bld.marks.push((c.start..c.end, hid.clone()));
                }
            } else {
                if lead_ws > 0 {
                    bld.marks.push((c.start..c.start + lead_ws, hid.clone()));
                }
                if trail_ws > 0 {
                    bld.marks.push((c.end - trail_ws..c.end, hid.clone()));
                }
            }
            *leads.entry(c.start).or_insert(0.0) += pad;
            // Nästa pipa hamnar på kolumnens högra kant.
            if row.pipes.get(j + 1).is_some() {
                *leads.entry(c.end).or_insert(0.0) += (col_x[j + 1] - (col_x[j] + pad + c.w)).max(0.0);
            }
        }
    }
    for row in t.rows.iter().filter(|r| r.delimiter) {
        let nl = usize::from(text.as_bytes().get(row.line.end) == Some(&b'\n'));
        let mut f = hid.clone();
        f.line_height = Some(px(z, 1.0));
        bld.marks.push((row.line.start..row.line.end + nl, f));
    }
}

pub fn layout_job(text: &str, a: &Analysis, p: &Params) -> LayoutJob {
    let bytes = text.as_bytes();
    let mut bld = Builder { secs: Vec::new(), cuts: BTreeSet::new(), marks: Vec::new() };
    let mut leads: BTreeMap<usize, f32> = BTreeMap::new();

    // Tabeller som inte visas rått blir rutnät.
    let tabs = tables(text, a);
    let grid: Vec<&Table> = tabs.iter().filter(|t| !intersects(&t.region, &p.reveal)).collect();
    for t in &grid {
        grid_table(text, a, t, p, &mut bld, &mut leads);
    }

    // Listor: dolt indrag, och texten börjar 0,5" in (+ 0,25" per nivå) via leading_space.
    let body = FontId::new(px(p.zoom, 11.0), theme::doc_font(false, false));
    for pc in &a.pieces {
        if !pc.list_marker || pc.range.is_empty() {
            continue;
        }
        let ls = text[..pc.range.start].rfind('\n').map_or(0, |i| i + 1);
        if !text[ls..pc.range.start].chars().all(|c| c == ' ' || c == '\t') {
            continue; // t.ex. ett objekt i ett citat: ingen indragning
        }
        let level = list_level(text, pc.range.start);
        // Det som syns före texten: rå markup på markörens rad, annars bara ett sifferstreck.
        let shown = if intersects(&pc.range, &p.reveal) {
            text[ls..pc.range.end].replace('\t', "    ")
        } else {
            if pc.range.start > ls {
                bld.marks.push((ls..pc.range.start, hidden_format(theme::doc_font(false, false))));
            }
            if pc.markup { String::new() } else { text[pc.range.clone()].to_owned() }
        };
        let w = if shown.is_empty() { 0.0 } else { (p.measure)(&shown, body.clone()) };
        leads.insert(ls, (list_text_x(p.zoom, level) - w).max(0.0));
    }
    bld.marks.sort_by_key(|m| m.0.start);
    bld.cuts.extend(leads.keys().copied());

    for pc in &a.pieces {
        let r = pc.range.clone();
        if r.is_empty() {
            continue;
        }
        let revealed = intersects(&r, &p.reveal);
        let hide = !revealed;
        let grid_hdr = if pc.block == Block::Table && !revealed {
            grid.iter()
                .find(|t| t.region.start <= r.start && r.start < t.region.end)
                .map(|t| t.rows.first().is_some_and(|row| r.start <= row.line.end))
        } else {
            None
        };

        if let Some(img) = &pc.image {
            if hide {
                if let Some(h) = (p.image_size)(&img.src).map(|s| s.y) {
                    // Dold text, men första tecknet reserverar bildens höjd.
                    let first = text[r.clone()].chars().next().map_or(1, char::len_utf8);
                    let fam = theme::doc_font(false, false);
                    let mut f1 = hidden_format(fam.clone());
                    f1.line_height = Some(h);
                    bld.push(r.start..r.start + first, &f1);
                    if r.start + first < r.end {
                        bld.push(r.start + first..r.end, &hidden_format(fam));
                    }
                } else {
                    // Ingen bild att visa: visa alternativtexten.
                    let size = px(p.zoom, 11.0);
                    let f = TextFormat {
                        font_id: FontId::new(size, theme::doc_font(false, true)),
                        color: p.palette.page_muted,
                        line_height: Some(size * 1.4),
                        ..Default::default()
                    };
                    bld.push(r, &f);
                }
            } else {
                let mut f = piece_format(pc, false, None, p);
                f.color = p.palette.page_markup;
                f.line_height = Some(px(p.zoom, 11.0) * 1.4);
                bld.push(r, &f);
            }
            continue;
        }

        let fmt = piece_format(pc, hide, grid_hdr, p);
        // Tomradsbrytningar blir egna sektioner med 8 pt radhöjd.
        let mut start = r.start;
        for (off, c) in text[r.clone()].char_indices() {
            let i = r.start + off;
            if c == '\n' && (i == 0 || bytes[i - 1] == b'\n') {
                if start < i {
                    bld.push(start..i, &fmt);
                }
                bld.push(i..i + 1, &spacing_format(p.zoom));
                start = i + 1;
            }
        }
        if start < r.end {
            bld.push(start..r.end, &fmt);
        }
    }

    // Slå ihop angränsande sektioner med samma format (inte där en sektion har leading_space).
    let mut job = LayoutJob { text: text.to_owned(), ..Default::default() };
    for (range, format) in bld.secs {
        let lead = leads.get(&range.start).copied().unwrap_or(0.0);
        if lead == 0.0 {
            if let Some(last) = job.sections.last_mut() {
                if last.format == format && last.byte_range.end.0 == range.start {
                    last.byte_range.end = ByteIndex(range.end);
                    continue;
                }
            }
        }
        job.sections.push(LayoutSection {
            leading_space: lead,
            byte_range: ByteIndex(range.start)..ByteIndex(range.end),
            format,
        });
    }

    if !p.query.is_empty() {
        let matches = highlight::find_all(text, p.query, p.case_sensitive);
        if !matches.is_empty() {
            highlight::apply_backgrounds(&mut job, &matches, p.palette, p.current_match);
            // apply_backgrounds nollar leading_space: sätt tillbaka den.
            for s in &mut job.sections {
                s.leading_space = leads.get(&s.byte_range.start.0).copied().unwrap_or(0.0);
            }
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_img(_: &str) -> Option<Vec2> {
        None
    }

    /// Fast teckenbredd (7 px) så att testerna inte behöver typsnitt.
    fn measure(s: &str, _: FontId) -> f32 {
        s.chars().count() as f32 * 7.0
    }

    fn build(text: &str, reveal: Range<usize>, query: &str) -> LayoutJob {
        let pal = theme::palette(false);
        let img = |_: &str| Some(Vec2::new(100.0, 50.0));
        let p = Params {
            palette: &pal,
            zoom: 1.0,
            reveal,
            query,
            case_sensitive: false,
            current_match: None,
            image_size: &img,
            measure: &measure,
        };
        layout_job(text, &markup::analyze(text), &p)
    }

    fn sec_at(job: &LayoutJob, byte: usize) -> &LayoutSection {
        job.sections
            .iter()
            .find(|s| s.byte_range.start.0 <= byte && byte < s.byte_range.end.0)
            .expect("sektion")
    }

    const DOC: &str = "# Rubrik\n\nText med **fet** och *kursiv*.\n\n- punkt\n\n![alt](bild.png)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

    #[test]
    fn text_and_contiguity() {
        for reveal in [0..0, 0..DOC.len()] {
            let job = build(DOC, reveal, "");
            assert_eq!(job.text, DOC);
            let mut pos = 0;
            for s in &job.sections {
                assert_eq!(s.byte_range.start.0, pos);
                assert!(s.byte_range.end.0 > pos);
                pos = s.byte_range.end.0;
            }
            assert_eq!(pos, DOC.len());
        }
    }

    #[test]
    fn markup_hidden_and_revealed() {
        let t = "Hej **fet** x";
        let hidden = build(t, 0..0, "");
        let s = sec_at(&hidden, 4);
        assert_eq!(s.format.color, Color32::TRANSPARENT);
        assert_eq!(s.format.font_id.size, HIDDEN_PX);
        assert_eq!(s.format.line_height, None);
        let shown = build(t, 0..t.len(), "");
        let s = sec_at(&shown, 4);
        assert_eq!(s.format.color, theme::palette(false).page_markup);
        assert_eq!(s.format.font_id.size, px(1.0, 11.0));
    }

    #[test]
    fn heading_and_bold() {
        let pal = theme::palette(false);
        let t = "# Rubrik\n\nett **fet** ord";
        let job = build(t, 0..0, "");
        let h = sec_at(&job, 3);
        assert_eq!(h.format.font_id.size, px(1.0, 18.0));
        assert_eq!(h.format.color, pal.page_heading1);
        assert_eq!(h.format.font_id.family, theme::doc_font(true, false));
        let at_fet = t.find("fet").unwrap();
        let b = sec_at(&job, at_fet);
        assert_eq!(b.format.font_id.family, theme::doc_font(true, false));
        assert_eq!(b.format.color, pal.page_text);
        let n = sec_at(&job, t.find("ett").unwrap());
        assert_eq!(n.format.font_id.family, theme::doc_font(false, false));
    }

    #[test]
    fn bullet_keeps_line_height() {
        let job = build("- punkt\n", 0..0, "");
        let s = sec_at(&job, 0);
        assert_eq!(s.format.font_id.size, HIDDEN_PX);
        assert_eq!(s.format.line_height, Some(px(1.0, 11.0) * 1.4));
        assert_eq!(s.format.color, Color32::TRANSPARENT);
    }

    #[test]
    fn image_reserves_height() {
        let t = "![alt](bild.png)\n";
        let job = build(t, 0..0, "");
        assert_eq!(sec_at(&job, 0).format.line_height, Some(50.0));
        assert_eq!(sec_at(&job, 0).format.color, Color32::TRANSPARENT);
        assert_eq!(sec_at(&job, 3).format.line_height, None);
        // Utan bild visas alternativtexten.
        let pal = theme::palette(false);
        let p = Params {
            palette: &pal,
            zoom: 1.0,
            reveal: 0..0,
            query: "",
            case_sensitive: false,
            current_match: None,
            image_size: &no_img,
            measure: &measure,
        };
        let job = layout_job(t, &markup::analyze(t), &p);
        assert_eq!(sec_at(&job, 0).format.color, pal.page_muted);
        // Visad: ingen reserverad höjd.
        let job = build(t, 0..t.len(), "");
        assert_eq!(sec_at(&job, 0).format.color, pal.page_markup);
        assert_ne!(sec_at(&job, 0).format.line_height, Some(50.0));
    }

    #[test]
    fn table_raw_when_revealed() {
        let t = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let job = build(t, 0..t.len(), "");
        let s = sec_at(&job, 0);
        assert_eq!(s.format.font_id.family, FontFamily::Monospace);
        assert_ne!(s.format.color, Color32::TRANSPARENT);
        assert!(job.sections.iter().all(|s| s.leading_space == 0.0));
    }

    #[test]
    fn find_background() {
        let pal = theme::palette(false);
        let job = build("hej och hopp", 0..0, "och");
        assert_eq!(sec_at(&job, 5).format.background, pal.match_bg);
        assert_eq!(job.text, "hej och hopp");
    }

    #[test]
    fn blank_line_row_height() {
        let t = "a\n\nb";
        let job = build(t, 0..0, "");
        let s = sec_at(&job, 2);
        assert_eq!(s.format.line_height, Some(px(1.0, 8.0)));
        assert_eq!(s.byte_range, ByteIndex(2)..ByteIndex(3));

        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut heights = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(Default::default(), |ui| {
                let ctx = ui.ctx().clone();
                let g = ctx.fonts_mut(|f| f.layout_job(job.clone()));
                heights = g.rows.iter().map(|r| r.height()).collect();
            });
            std::mem::forget(out); // TexturesDelta får inte släppas utan att tas om hand
        }
        eprintln!("rader: {heights:?}");
        assert_eq!(heights.len(), 3);
        assert!((heights[1] - px(1.0, 8.0)).abs() < 0.5, "{heights:?}");
        assert!(heights[0] > 14.0);
    }

    /// Lägger ut `job` med riktiga typsnitt och ger galleriet till `f`.
    fn with_galley<R>(job: LayoutJob, f: impl FnOnce(&egui::Galley) -> R) -> R {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut f = Some(f);
        let mut res = None;
        for _ in 0..2 {
            let out = ctx.run_ui(Default::default(), |ui| {
                let g = ui.ctx().fonts_mut(|fo| fo.layout_job(job.clone()));
                if let Some(f) = f.take() {
                    res = Some(f(&g));
                }
            });
            std::mem::forget(out); // TexturesDelta får inte släppas utan att tas om hand
        }
        res.unwrap()
    }

    #[test]
    fn hidden_markup_has_no_width() {
        let hidden = "](https://example.com)x__y";
        let mut job = LayoutJob::default();
        let f = hidden_format(theme::doc_font(false, false));
        assert_eq!(f.underline, Stroke::NONE);
        assert_eq!(f.strikethrough, Stroke::NONE);
        assert_eq!(f.background, Color32::TRANSPARENT);
        job.append(&hidden[..25], 0.0, f);
        assert_eq!(job.text.chars().count(), 25);
        let (w, n) = with_galley(job, |g| {
            (g.size().x, g.rows.iter().map(|r| r.char_count_including_newline().0).sum::<usize>())
        });
        assert!(w < 1.0, "bredd {w}");
        assert_eq!(n, 25, "markören ska kunna placeras på varje tecken");
    }

    #[test]
    fn link_has_no_gap() {
        let pal = theme::palette(false);
        let t = "[länk](https://example.com).";
        let p = Params {
            palette: &pal,
            zoom: 1.0,
            reveal: 0..0,
            query: "",
            case_sensitive: false,
            current_match: None,
            image_size: &no_img,
            measure: &measure,
        };
        let job = layout_job(t, &markup::analyze(t), &p);
        let gap = with_galley(job, |g| {
            let cc = |b: usize| egui::text::CCursor::new(t[..b].chars().count());
            let x_dot = g.pos_from_cursor(cc(t.len() - 1)).left();
            let x_end = g.pos_from_cursor(cc(t.find(']').unwrap())).left();
            x_dot - x_end
        });
        assert!(gap < 1.0, "glapp {gap} före punkten");
    }

    #[test]
    fn list_indent_and_hidden_marker() {
        let t = "- ett\n    - två\n1. tre\n";
        let job = build(t, 0..0, "");
        let s0 = sec_at(&job, 0);
        assert_eq!(s0.leading_space, list_text_x(1.0, 0));
        assert_eq!(s0.format.color, Color32::TRANSPARENT);
        // Nästlad rad: det dolda indraget bär leading_space, nivå 1.
        let l1 = t.find("    - två").unwrap();
        let s1 = sec_at(&job, l1);
        assert_eq!(s1.byte_range.start.0, l1);
        assert_eq!(s1.leading_space, list_text_x(1.0, 1));
        assert_eq!(s1.format.color, Color32::TRANSPARENT);
        assert_eq!(list_level(t, l1 + 4), 1);
        assert_eq!(list_level("\t\t- x", 2), 2);
        // Numrerad: markören syns, texten ska hamna på samma x som en punkts.
        let l2 = t.find("1. tre").unwrap();
        let s2 = sec_at(&job, l2);
        assert_ne!(s2.format.color, Color32::TRANSPARENT);
        assert!((s2.leading_space + measure("1. ", FontId::default()) - list_text_x(1.0, 0)).abs() < 0.01);
        // Markörens rad visar rå markup, indraget justeras så texten inte hoppar.
        let rev = build(t, l1..l1 + 9, "");
        let r1 = sec_at(&rev, l1);
        assert_ne!(r1.format.color, Color32::TRANSPARENT);
        assert!((r1.leading_space + measure("    - ", FontId::default()) - list_text_x(1.0, 1)).abs() < 0.01);
    }

    #[test]
    fn table_grid_layout() {
        let pal = theme::palette(false);
        let t = "| Namn | Id |\n| --- | --- |\n| Alexandra | 7 |\n\nefter";
        let job = build(t, 0..0, "");
        // Pipor och cellblanksteg är dolda.
        assert_eq!(sec_at(&job, 0).format.color, Color32::TRANSPARENT);
        assert_eq!(sec_at(&job, 1).format.color, Color32::TRANSPARENT);
        let namn = t.find("Namn").unwrap();
        assert_eq!(sec_at(&job, namn).format.font_id.family, theme::doc_font(true, false));
        assert_eq!(sec_at(&job, namn).format.color, pal.page_text);
        let alex = t.find("Alexandra").unwrap();
        assert_eq!(sec_at(&job, alex).format.font_id.family, theme::doc_font(false, false));
        // Avgränsarraden kollapsar, även radbrytningen.
        let d = t.find("| ---").unwrap();
        assert_eq!(sec_at(&job, d).format.line_height, Some(px(1.0, 1.0)));
        // Cellmarginal före texten; pipan efter den smalare cellen skjuts ut.
        let pad = px(1.0, CELL_PAD_PT);
        assert_eq!(sec_at(&job, namn - 1).leading_space, pad);
        let pipe2 = t.find("| Id").unwrap();
        let w_alex = measure("Alexandra", FontId::default());
        let w_namn = measure("Namn", FontId::default());
        assert!((sec_at(&job, pipe2).leading_space - (w_alex - w_namn + pad)).abs() < 0.01);
        // Texten efter tabellen är orörd.
        assert_eq!(sec_at(&job, t.find("efter").unwrap()).leading_space, 0.0);
    }

    #[test]
    fn table_columns_align_in_galley() {
        let pal = theme::palette(false);
        let t = "| A | Bee |\n| --- | --- |\n| Mycket långt | d |\n";
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let meas = |s: &str, f: FontId| -> f32 {
            let out = ctx.run_ui(Default::default(), |_| {});
            std::mem::forget(out);
            ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x)
        };
        let p = Params {
            palette: &pal,
            zoom: 1.0,
            reveal: 0..0,
            query: "",
            case_sensitive: false,
            current_match: None,
            image_size: &no_img,
            measure: &meas,
        };
        let job = layout_job(t, &markup::analyze(t), &p);
        let ci = |needle: &str| t[..t.find(needle).unwrap()].chars().count();
        let (xb, xd) = with_galley(job, |g| {
            let x = |needle: &str| g.pos_from_cursor(egui::text::CCursor::new(ci(needle))).left();
            (x("Bee"), x("d |"))
        });
        assert!((xb - xd).abs() < 1.5, "kolumn 2 linjerar inte: {xb} vs {xd}");
    }

    #[test]
    fn cache_hit_and_miss() {
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let mk = |reveal: Range<usize>| Params {
            palette: &pal,
            zoom: 1.0,
            reveal,
            query: "",
            case_sensitive: false,
            current_match: None,
            image_size: &img,
            measure: &measure,
        };
        let mut c = Cache::default();
        let t = "Hej **du**";
        let a = c.job(1, t, &mk(0..0));
        let b = c.job(1, t, &mk(0..0));
        assert_eq!(a.sections.len(), b.sections.len());
        assert!(a.sections.iter().zip(&b.sections).all(|(x, y)| x.format == y.format));
        assert!(Arc::ptr_eq(&c.analysis(1, t), &c.analysis(1, t)));
        let d = c.job(1, t, &mk(0..t.len()));
        assert_ne!(a.sections[1].format.color, d.sections[1].format.color);
        c.forget(1);
        assert!(c.jobs.is_empty());
    }
}
