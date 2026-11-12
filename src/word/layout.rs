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

use egui::epaint::text::FontsView;
use egui::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat};
use egui::{Color32, FontFamily, FontId, Galley, Stroke, Vec2};

use super::markup::{self, Align, Analysis, Block, DocDefaults, Piece};
use crate::highlight;
use crate::theme::{self, Palette};

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
    /// Egui-familj för (typsnittsnamn, fet, kursiv). Ren uppslagning: typsnitten är redan
    /// registrerade (se `needed_fonts`); okända namn ger Carlito.
    pub font_family: &'a dyn Fn(Option<&str>, bool, bool) -> FontFamily,
    /// `DocFonts::generation()`: ändras när ett typsnitt blivit klart, så att layouten görs om.
    pub fonts_gen: u64,
}

#[derive(Default)]
pub struct Cache {
    analyses: HashMap<u64, (u64, Arc<Analysis>)>,
    jobs: HashMap<u64, (u64, LayoutJob)>,
    galleys: HashMap<u64, (u64, Arc<Galley>)>,
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

    /// Nyckeln: texthash, reveal, zoom, dark, query, case_sensitive, current_match, typsnittsgeneration.
    fn job_key(text: &str, p: &Params) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        p.reveal.start.hash(&mut h);
        p.reveal.end.hash(&mut h);
        p.zoom.to_bits().hash(&mut h);
        p.palette.dark.hash(&mut h);
        p.query.hash(&mut h);
        p.case_sensitive.hash(&mut h);
        p.current_match.hash(&mut h);
        p.fonts_gen.hash(&mut h);
        h.finish()
    }

    /// Hela dokumentets galleri, utlagt stycke för stycke (justering, hängande indrag). Jobbet
    /// byggs först (`p.measure` tar fonts-låset, så det måste ske utanför `fonts_mut`).
    pub fn galley(&mut self, ctx: &egui::Context, doc_id: u64, text: &str, p: &Params, wrap_width: f32) -> Arc<Galley> {
        self.galley_paged(ctx, doc_id, text, p, wrap_width, None)
    }

    /// Som `galley`, men med raderna uppdelade på sidor enligt `pages` (se `pages::paginate`).
    pub fn galley_paged(
        &mut self,
        ctx: &egui::Context,
        doc_id: u64,
        text: &str,
        p: &Params,
        wrap_width: f32,
        pages: Option<&super::pages::Geom>,
    ) -> Arc<Galley> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        Self::job_key(text, p).hash(&mut h);
        if let Some(g) = pages {
            g.hash_into(&mut h);
        }
        wrap_width.to_bits().hash(&mut h);
        ctx.pixels_per_point().to_bits().hash(&mut h);
        // En nyregistrerad familj bygger om typsnittsatlasen: gamla galleri kan bli ogiltiga.
        ctx.fonts(|f| f.families().len()).hash(&mut h);
        let key = h.finish();
        if let Some((k, g)) = self.galleys.get(&doc_id) {
            if *k == key {
                return g.clone();
            }
        }
        let job = self.job(doc_id, text, p);
        let a = self.analysis(doc_id, text);
        let ppp = ctx.pixels_per_point();
        let mut g = ctx.fonts_mut(|f| layout_galley(f, job, &a, p, wrap_width, ppp));
        if let Some(geom) = pages {
            super::pages::paginate(&mut g, geom, ppp);
        }
        let g = Arc::new(g);
        self.galleys.insert(doc_id, (key, g.clone()));
        g
    }

    pub fn job(&mut self, doc_id: u64, text: &str, p: &Params) -> LayoutJob {
        let key = Self::job_key(text, p);

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
        self.galleys.remove(&doc_id);
    }
}

/// Storlek (pt) för blockets text; `body` = dokumentets brödtextstorlek.
fn block_size_pt(b: Block, mono: bool, body: f32) -> f32 {
    if mono {
        return 10.0;
    }
    match b {
        Block::Heading(1) => 18.0,
        Block::Heading(2) => 15.0,
        Block::Heading(3) => 13.0,
        Block::Heading(4) => 12.0,
        Block::Heading(_) => 11.0,
        _ => body,
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
/// (`Some(true)` = rubrikraden). Brödtextens typsnitt och storlek kommer från `d`, och en
/// `<span>` på biten (`pc.span`) går före både dem och rubrikens värden.
fn piece_format(pc: &Piece, hide: bool, grid: Option<bool>, p: &Params, d: &DocDefaults) -> TextFormat {
    let pal = p.palette;
    let zoom = p.zoom;
    let table_raw = pc.block == Block::Table && grid.is_none();
    let mono = pc.code || matches!(pc.block, Block::Code) || table_raw;
    let heading = matches!(pc.block, Block::Heading(_));
    let body_family = |bold: bool, italic: bool| (p.font_family)(Some(d.font()), bold, italic);
    let block_pt = block_size_pt(pc.block, mono, d.size());
    let lh_for = |size: f32| if heading { size * 1.3 } else { size * 1.4 };
    let family_plain = if mono { FontFamily::Monospace } else { body_family(false, false) };

    if pc.markup {
        let size = px(zoom, block_pt);
        if hide {
            // Punktmarkören behåller brödtextens radhöjd så att en tom punkt inte kollapsar.
            if pc.list_marker {
                let mut f = hidden_format(body_family(false, false));
                f.line_height = Some(px(zoom, d.size()) * 1.4);
                return f;
            }
            return hidden_format(family_plain);
        }
        return TextFormat {
            font_id: FontId::new(size, family_plain),
            color: pal.page_markup,
            line_height: Some(lh_for(size)),
            ..Default::default()
        };
    }

    let span = pc.span.as_deref();
    let size = px(zoom, span.and_then(|s| s.font_size).unwrap_or(block_pt));
    let header = grid == Some(true);
    let bold = pc.bold || heading || header;
    let italic = pc.italic || matches!(pc.block, Block::Quote);
    let family = if mono {
        FontFamily::Monospace
    } else {
        match span.and_then(|s| s.font_family.as_deref()) {
            Some(name) => (p.font_family)(Some(name), bold, italic),
            // Rubriker behåller Kladds eget typsnitt (Carlito) om inte texten har ett eget.
            None if heading => theme::doc_font(bold, italic),
            None => body_family(bold, italic),
        }
    };
    let mut color = match pc.block {
        Block::Heading(1) => pal.page_heading1,
        Block::Heading(6) => pal.page_muted,
        Block::Heading(_) => pal.page_heading,
        Block::Quote => pal.page_muted,
        _ => pal.page_text,
    };
    if let Some(c) = span.and_then(|s| s.color) {
        color = adapt_text_color(c, pal.page, pal.dark);
    }
    if pc.link {
        color = pal.page_link;
    }
    let mut f = TextFormat {
        font_id: FontId::new(size, family),
        line_height: Some(lh_for(size)),
        color,
        ..Default::default()
    };
    if pc.code && pc.block != Block::Table {
        f.background = pal.page_code_bg;
    }
    if let Some(b) = span.and_then(|s| s.background) {
        f.background = adapt_background(b, pal.dark);
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

// ------------------------------------------------------------------ färger

/// Relativ luminans (WCAG).
fn luminance(c: [u8; 3]) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// Kontrastförhållande (1..21) mellan två färger.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// RGB -> (nyans 0..360, mättnad 0..1, ljushet 0..1).
fn rgb_to_hsl(c: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let q = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [q(r), q(g), q(b)]
}

/// Textfärg ur dokumentet. I mörkt tema får färger med för låg kontrast mot sidan (< 3:1)
/// omvänd HSL-ljushet (nyans och mättnad behålls): svart blir vitt, mörkblå ljusblå.
pub fn adapt_text_color(c: [u8; 3], page: Color32, dark: bool) -> Color32 {
    let mut out = c;
    if dark && contrast(c, [page.r(), page.g(), page.b()]) < 3.0 {
        let (h, s, l) = rgb_to_hsl(c);
        let mut nl = 1.0 - l;
        out = hsl_to_rgb(h, s, nl);
        // Mellanljusa färger (ren blå) ändras knappt av inversionen: ljusa upp tills de syns.
        while contrast(out, [page.r(), page.g(), page.b()]) < 3.0 && nl < 0.9 {
            nl += 0.05;
            out = hsl_to_rgb(h, s, nl);
        }
    }
    Color32::from_rgb(out[0], out[1], out[2])
}

/// Markerings-/skuggningsfärg. I mörkt tema mörknas och avmättas ljusa färger så att den
/// ljusa texten går att läsa (gult blir mörkt olivgrönt); mörka färger lämnas som de är.
pub fn adapt_background(c: [u8; 3], dark: bool) -> Color32 {
    let mut out = c;
    if dark && luminance(c) > 0.2 {
        let (h, s, l) = rgb_to_hsl(c);
        out = hsl_to_rgb(h, s * 0.7, (l * 0.4).min(0.35));
    }
    Color32::from_rgb(out[0], out[1], out[2])
}

/// De typsnitt (namn, fet, kursiv) som layouten kommer att be om, så att `word_page` kan
/// registrera dem hos egui innan layouten körs.
pub fn needed_fonts(a: &Analysis) -> BTreeSet<(String, bool, bool)> {
    let d = &a.defaults;
    let mut set = BTreeSet::new();
    set.insert((d.font().to_owned(), false, false));
    for pc in &a.pieces {
        if pc.markup || pc.code || matches!(pc.block, Block::Code) {
            continue;
        }
        let heading = matches!(pc.block, Block::Heading(_));
        let span_font = pc.span.as_deref().and_then(|s| s.font_family.as_deref());
        if heading && span_font.is_none() {
            continue; // Carlito
        }
        let name = span_font.unwrap_or(d.font()).to_owned();
        let italic = pc.italic || matches!(pc.block, Block::Quote);
        set.insert((name.clone(), pc.bold || heading, italic));
        if pc.block == Block::Table {
            set.insert((name.clone(), true, false));
            set.insert((name, false, false));
        }
    }
    set
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
    let size = px(z, a.defaults.size());
    let mut rows: Vec<(&TableRow, Vec<Cell>)> = Vec::new();
    let mut ncols = 0;
    for (ri, row) in t.rows.iter().enumerate() {
        if row.delimiter {
            continue;
        }
        let font = FontId::new(size, (p.font_family)(Some(a.defaults.font()), ri == 0, false));
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

    // Listor: indraget före markören döljs. Själva hängande indraget (texten börjar 0,5" in,
    // +0,25" per nivå) görs i `layout_galley` när stycket läggs ut.
    for pc in &a.pieces {
        if !pc.list_marker || pc.range.is_empty() || intersects(&pc.range, &p.reveal) {
            continue;
        }
        let ls = text[..pc.range.start].rfind('\n').map_or(0, |i| i + 1);
        if pc.range.start > ls && text[ls..pc.range.start].chars().all(|c| c == ' ' || c == '\t') {
            bld.marks.push((ls..pc.range.start, hidden_format(theme::doc_font(false, false))));
        }
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
                    let fam = (p.font_family)(Some(a.defaults.font()), false, false);
                    let mut f1 = hidden_format(fam.clone());
                    f1.line_height = Some(h);
                    bld.push(r.start..r.start + first, &f1);
                    if r.start + first < r.end {
                        bld.push(r.start + first..r.end, &hidden_format(fam));
                    }
                } else {
                    // Ingen bild att visa: visa alternativtexten.
                    let size = px(p.zoom, a.defaults.size());
                    let f = TextFormat {
                        font_id: FontId::new(size, (p.font_family)(Some(a.defaults.font()), false, true)),
                        color: p.palette.page_muted,
                        line_height: Some(size * 1.4),
                        ..Default::default()
                    };
                    bld.push(r, &f);
                }
            } else {
                let mut f = piece_format(pc, false, None, p, &a.defaults);
                f.color = p.palette.page_markup;
                f.line_height = Some(px(p.zoom, a.defaults.size()) * 1.4);
                bld.push(r, &f);
            }
            continue;
        }

        let fmt = piece_format(pc, hide, grid_hdr, p, &a.defaults);
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


// ------------------------------------------------------------------ stycken

/// Hängande indrag för ett listobjekt.
#[derive(Debug, Clone, Copy)]
struct ListGeom {
    /// Textens startposition (px från marginalen).
    text_x: f32,
    /// Antal tecken från styckets början till och med markören (`- `, `1. `).
    marker_chars: usize,
}

/// Ett stycke (texten mellan två `\n`) och det som behövs för att lägga ut det.
#[derive(Debug, Clone)]
struct Para {
    /// Byteintervall utan avslutande `\n`.
    range: Range<usize>,
    align: Align,
    list: Option<ListGeom>,
    /// Bredd (px) på en bild som ritas på stycket, för centrering och högerjustering.
    image_w: f32,
    /// Dold taggrad (div, standardkommentaren) eller tom rad intill en: ~0 höjd.
    collapse: bool,
}

/// Styckena i texten. En avslutande `\n` ger ett tomt sista stycke.
fn paragraphs(text: &str, a: &Analysis, p: &Params) -> Vec<Para> {
    // 0 = vanligt stycke, 1 = dold öppningstagg/kommentar, 2 = dold stängningstagg.
    let mut tag: Vec<u8> = Vec::new();
    let mut out: Vec<Para> = Vec::new();
    let mut s = 0;
    loop {
        let e = text[s..].find('\n').map_or(text.len(), |i| s + i);
        let range = s..e;
        let first = a.pieces.partition_point(|pc| pc.range.end <= s);
        let ps: Vec<&Piece> = if e > s {
            a.pieces[first..].iter().take_while(|pc| pc.range.start < e).collect()
        } else {
            Vec::new()
        };
        let body = |pc: &Piece| &text[pc.range.start.max(s)..pc.range.end.min(e)];

        let t = text[range.clone()].trim();
        let is_tag = (t.starts_with("<div") || t.starts_with("</div") || t.starts_with("<!--"))
            && ps.iter().all(|pc| pc.markup || body(pc).trim().is_empty());
        tag.push(if !is_tag || intersects(&range, &p.reveal) {
            0
        } else if t.starts_with("</div") {
            2
        } else {
            1
        });

        let align = ps
            .iter()
            .find(|pc| !pc.markup && !body(pc).trim().is_empty())
            .or(ps.first())
            .map_or(Align::Left, |pc| pc.align);
        let list = ps.iter().find(|pc| pc.list_marker && pc.range.start >= s && !pc.range.is_empty()).and_then(|pc| {
            if !text[s..pc.range.start].chars().all(|c| c == ' ' || c == '\t') {
                return None; // t.ex. ett objekt i ett citat: ingen indragning
            }
            let level = list_level(text, pc.range.start);
            Some(ListGeom { text_x: list_text_x(p.zoom, level), marker_chars: text[s..pc.range.end].chars().count() })
        });
        let image_w = ps
            .iter()
            .filter(|pc| !intersects(&pc.range, &p.reveal))
            .filter_map(|pc| pc.image.as_ref())
            .filter_map(|img| (p.image_size)(&img.src))
            .map(|sz| sz.x)
            .sum();
        out.push(Para { range, align, list, image_w, collapse: tag.last() != Some(&0) });
        if e >= text.len() {
            break;
        }
        s = e + 1;
    }
    // Tomma rader närmast en dold taggrad (efter en öppning, före en stängning) kollapsar också,
    // så att en centrerad div inte får fyra gånger så stort radavstånd som vanliga stycken.
    for i in 0..out.len() {
        if !out[i].range.is_empty() {
            continue;
        }
        let after_open = i > 0 && tag[i - 1] == 1;
        let before_close = tag.get(i + 1) == Some(&2);
        out[i].collapse = after_open || before_close;
    }
    out
}

/// Utsnitt av `full` för byteintervallet `r`, med sektionerna omräknade till 0. Ett tomt
/// intervall får en tom sektion med formatet där stycket börjar (det ger radhöjden).
fn clip_job(full: &LayoutJob, r: Range<usize>) -> LayoutJob {
    let mut j = LayoutJob {
        text: full.text[r.clone()].to_owned(),
        wrap: full.wrap.clone(),
        round_output_to_gui: full.round_output_to_gui,
        break_on_newline: full.break_on_newline,
        keep_trailing_whitespace: full.keep_trailing_whitespace,
        ..Default::default()
    };
    let i = full.sections.partition_point(|s| s.byte_range.end.0 <= r.start);
    if r.is_empty() {
        if let Some(sec) = full.sections.get(i).or(full.sections.last()) {
            j.sections.push(LayoutSection {
                leading_space: 0.0,
                byte_range: ByteIndex(0)..ByteIndex(0),
                format: sec.format.clone(),
            });
        }
        return j;
    }
    for sec in &full.sections[i..] {
        let (s, e) = (sec.byte_range.start.0, sec.byte_range.end.0);
        if s >= r.end {
            break;
        }
        j.sections.push(LayoutSection {
            // Ledigt utrymme hör till sektionens början: hamnar den före utsnittet försvinner det.
            leading_space: if s >= r.start { sec.leading_space } else { 0.0 },
            byte_range: ByteIndex(s.max(r.start) - r.start)..ByteIndex(e.min(r.end) - r.start),
            format: sec.format.clone(),
        });
    }
    j
}

/// Högsta x för radens sista synliga (icke-blanka) tecken; 0 för en blank rad.
fn content_end(row: &egui::epaint::text::Row) -> f32 {
    row.glyphs.iter().rposition(|g| !g.chr.is_whitespace()).map_or(0.0, |i| row.glyphs[i].max_x())
}

fn is_hidden_format(f: &TextFormat) -> bool {
    f.font_id.size <= HIDDEN_PX * 2.0
}

/// Marginaljustering av en rad: lägger ut raden igen utan radbrytning, med extra
/// `leading_space` efter varje synligt blanksteg inne i raden (så att dold markup inte räknas
/// som mellanrum). `row_chars` = radens tecken i `pj`, `target` = önskad bredd från radens
/// vänsterkant. `None` om raden inte kan eller behöver justeras.
fn justify_row(
    fonts: &mut FontsView<'_>,
    pj: &LayoutJob,
    char_off: usize,
    row: &egui::epaint::text::Row,
    target: f32,
) -> Option<Arc<egui::epaint::text::Row>> {
    let n = row.glyphs.len();
    if n < 2 || !target.is_finite() {
        return None;
    }
    let bstart = pj.text.char_indices().nth(char_off).map_or(pj.text.len(), |(b, _)| b);
    let chars: Vec<(usize, char)> = pj.text[bstart..].char_indices().take(n).collect();
    if chars.len() != n {
        return None;
    }
    let visible = |b: usize| {
        let i = pj.sections.partition_point(|s| s.byte_range.end.0 <= bstart + b);
        pj.sections.get(i).is_some_and(|s| !is_hidden_format(&s.format))
    };
    let vis_chars: Vec<usize> = (0..n).filter(|&i| !chars[i].1.is_whitespace() && visible(chars[i].0)).collect();
    let (&first, &last) = (vis_chars.first()?, vis_chars.last()?);
    let cuts: Vec<usize> = (first + 1..last).filter(|&i| chars[i].1 == ' ' && visible(chars[i].0)).map(|i| chars[i].0 + 1).collect();
    if cuts.is_empty() {
        return None;
    }
    let extra = (target - row.glyphs[last].max_x()) / cuts.len() as f32;
    if extra < 0.25 {
        return None;
    }
    let bend = chars.last().map_or(bstart, |&(b, c)| bstart + b + c.len_utf8());
    let base = clip_job(pj, bstart..bend);
    let layout_with = |fonts: &mut FontsView<'_>, extra: f32| {
        let mut sub = LayoutJob { sections: Vec::new(), ..base.clone() };
        sub.wrap.max_width = f32::INFINITY;
        for sec in &base.sections {
            let (s0, e0) = (sec.byte_range.start.0, sec.byte_range.end.0);
            let mut bounds = vec![s0];
            bounds.extend(cuts.iter().copied().filter(|&c| c > s0 && c < e0));
            bounds.push(e0);
            for w in bounds.windows(2) {
                let mut lead = if w[0] == s0 { sec.leading_space } else { 0.0 };
                if cuts.contains(&w[0]) {
                    lead += extra;
                }
                sub.sections.push(LayoutSection {
                    leading_space: lead,
                    byte_range: ByteIndex(w[0])..ByteIndex(w[1]),
                    format: sec.format.clone(),
                });
            }
        }
        let g = fonts.layout_job(sub);
        (g.rows.len() == 1 && g.rows[0].glyphs.len() == n).then(|| g.rows[0].row.clone())
    };
    let mut best = layout_with(fonts, extra)?;
    // Glyferna avrundas till hela pixlar: räkna om en gång med det som blev över.
    let miss = target - content_end(&best);
    if miss.abs() > 0.4 {
        if let Some(r) = layout_with(fonts, extra + miss / cuts.len() as f32) {
            if (target - content_end(&r)).abs() < miss.abs() {
                best = r;
            }
        }
    }
    Some(best)
}

/// Lägger ut ett stycke och placerar dess rader: hängande indrag, centrering, högerjustering
/// och marginaljustering.
fn layout_para(fonts: &mut FontsView<'_>, full: &LayoutJob, para: &Para, avail: f32, ppp: f32) -> Arc<Galley> {
    let mut pj = clip_job(full, para.range.clone());
    if para.collapse {
        let h = if para.range.is_empty() { 1.0 } else { HIDDEN_PX };
        for s in &mut pj.sections {
            s.format.line_height = Some(h);
        }
    }
    let list = para.list;
    let wrap_w = match list {
        Some(l) => (avail - l.text_x).max(24.0),
        None => avail,
    };
    pj.wrap.max_width = wrap_w;
    let mut g = fonts.layout_job(pj.clone());

    let finite = avail.is_finite();
    let shifts = finite && (list.is_some() || para.align != Align::Left);
    if !shifts {
        return g;
    }
    let round = |x: f32| (x * ppp).round() / ppp;
    let gm = Arc::make_mut(&mut g);
    let n = gm.rows.len();
    let mut char_off = 0;
    for i in 0..n {
        let row = gm.rows[i].row.clone();
        let end = content_end(&row);
        // Radens vänsterkant och bredd som raden ska fylla.
        let left = match list {
            Some(l) if i == 0 => {
                let prefix = row.glyphs.get(l.marker_chars).map_or(row.size.x, |g| g.pos.x);
                (l.text_x - prefix).max(0.0)
            }
            Some(l) => l.text_x,
            None => 0.0,
        };
        let target = (avail - left).max(1.0);
        let mut x = left;
        match para.align {
            Align::Center if list.is_none() => x += ((target - end.max(para.image_w)) / 2.0).max(0.0),
            Align::Right if list.is_none() => x += (target - end.max(para.image_w)).max(0.0),
            Align::Justify if i + 1 < n => {
                if let Some(r) = justify_row(fonts, &pj, char_off, &row, target) {
                    gm.rows[i].row = r;
                }
            }
            _ => {}
        }
        gm.rows[i].pos.x = round(x);
        char_off += row.glyphs.len();
    }
    g
}

/// Lägger ut hela jobbet stycke för stycke och fogar ihop styckena till ett galleri som
/// TextEdit kan använda som vilket annat: tecken och markörlägen stämmer med texten.
pub fn layout_galley(fonts: &mut FontsView<'_>, mut job: LayoutJob, a: &Analysis, p: &Params, wrap_width: f32, ppp: f32) -> Galley {
    job.wrap.max_width = wrap_width;
    let full = Arc::new(job);
    let paras = paragraphs(&full.text, a, p);
    let galleys: Vec<Arc<Galley>> = paras.iter().map(|para| layout_para(fonts, &full, para, wrap_width, ppp)).collect();
    Galley::concat(full, &galleys, ppp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_img(_: &str) -> Option<Vec2> {
        None
    }

    fn fam(_: Option<&str>, bold: bool, italic: bool) -> FontFamily {
        theme::doc_font(bold, italic)
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
            font_family: &fam,
            fonts_gen: 0,
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
            font_family: &fam,
            fonts_gen: 0,
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
            font_family: &fam,
            fonts_gen: 0,
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
    fn list_levels_and_hidden_indent() {
        let t = "- ett\n    - två\n1. tre\n";
        let job = build(t, 0..0, "");
        // Punktmarkören och radens indrag är dolda; ingen sektion bär längre leading_space.
        assert_eq!(sec_at(&job, 0).format.color, Color32::TRANSPARENT);
        let l1 = t.find("    - två").unwrap();
        let s1 = sec_at(&job, l1);
        assert_eq!(s1.byte_range.start.0, l1);
        assert_eq!(s1.format.color, Color32::TRANSPARENT);
        assert!(job.sections.iter().all(|s| s.leading_space == 0.0));
        assert_eq!(list_level(t, l1 + 4), 1);
        assert_eq!(list_level("\t\t- x", 2), 2);
        // Numrerad: markören syns.
        assert_ne!(sec_at(&job, t.find("1. tre").unwrap()).format.color, Color32::TRANSPARENT);
        // Markörens rad visar rå markup.
        let rev = build(t, l1..l1 + 9, "");
        assert_ne!(sec_at(&rev, l1).format.color, Color32::TRANSPARENT);
    }

    /// Lägger ut `text` med `layout_galley` (riktiga typsnitt, fast `measure`) och ger galleriet till `f`.
    fn with_doc_galley<R>(
        text: &str,
        reveal: Range<usize>,
        dark: bool,
        wrap: f32,
        font_family: &dyn Fn(Option<&str>, bool, bool) -> FontFamily,
        f: impl FnOnce(&Galley, &LayoutJob) -> R,
    ) -> R {
        let pal = theme::palette(dark);
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let img = |_: &str| Some(Vec2::new(100.0, 50.0));
        let p = Params {
            palette: &pal,
            zoom: 1.0,
            reveal,
            query: "",
            case_sensitive: false,
            current_match: None,
            image_size: &img,
            measure: &measure,
            font_family,
            fonts_gen: 0,
        };
        let a = markup::analyze(text);
        let job = layout_job(text, &a, &p);
        let mut f = Some(f);
        let mut res = None;
        for frame in 0..2 {
            let out = ctx.run_ui(Default::default(), |ui| {
                let g = ui.ctx().fonts_mut(|fo| layout_galley(fo, job.clone(), &a, &p, wrap, 1.0));
                if frame == 1 {
                    res = Some((f.take().unwrap())(&g, &job));
                }
            });
            std::mem::forget(out); // TexturesDelta får inte släppas utan att tas om hand
        }
        res.unwrap()
    }

    /// (förstatecken, antal tecken) för varje rad.
    fn row_chars(g: &Galley) -> Vec<(usize, usize)> {
        let mut at = 0;
        g.rows
            .iter()
            .map(|r| {
                let n = r.char_count_including_newline().0;
                at += n;
                (at - n, n)
            })
            .collect()
    }

    fn ci(t: &str, needle: &str) -> usize {
        t[..t.find(needle).unwrap()].chars().count()
    }

    const LONG: &str = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua Ut enim ad minim veniam quis nostrud exercitation";

    fn doc_text() -> String {
        format!(
            "{LONG}\n\n- punkt {LONG}\n\n1. nummer {LONG}\n\n<div align=\"center\">\n\nMitten\n\n</div>\n\n<div align=\"justify\">\n\n{LONG}\n\n{LONG}\n\n</div>\n\n<div align=\"right\">\n\nHöger\n\n</div>\n\nSlut"
        )
    }

    #[test]
    fn hanging_indent_on_wrapped_list_rows() {
        let t = doc_text();
        with_doc_galley(&t, 0..0, false, 400.0, &fam, |g, _| {
            let lst = list_text_x(1.0, 0);
            for needle in ["punkt Lorem", "nummer Lorem"] {
                let start = ci(&t, needle);
                // Första radens text (efter markören) och alla följande rader börjar på textpositionen.
                let x = g.pos_from_cursor(egui::text::CCursor::new(start)).left();
                assert!((x - lst).abs() < 1.5, "{needle}: text vid {x}, väntade {lst}");
                let row0 = g.layout_from_cursor(egui::text::CCursor::new(start)).row;
                let mut last = row0;
                while !g.rows[last].ends_with_newline {
                    last += 1;
                }
                assert!(last > row0 + 1, "stycket ska brytas över flera rader");
                for k in row0 + 1..=last {
                    let left = g.rows[k].pos.x + g.rows[k].glyphs[0].pos.x;
                    assert!((left - lst).abs() < 1.5, "{needle}: rad {k} börjar på {left}, väntade {lst}");
                }
            }
        });
    }

    #[test]
    fn revealed_list_keeps_text_position() {
        let t = "- ett\n    - två lång rad\n1. tre\n";
        let l1 = t.find("    - två").unwrap();
        for reveal in [0..0, l1..l1 + 9] {
            with_doc_galley(t, reveal, false, 400.0, &fam, |g, _| {
                let x = g.pos_from_cursor(egui::text::CCursor::new(ci(t, "två"))).left();
                assert!((x - list_text_x(1.0, 1)).abs() < 1.5, "två vid {x}");
            });
        }
    }

    #[test]
    fn alignment_and_cursor_mapping() {
        let t = doc_text();
        let wrap = 400.0;
        with_doc_galley(&t, 0..0, false, wrap, &fam, |g, _| {
            let total = t.chars().count();
            // Tecknen i raderna summerar till texten (TextEdit förutsätter det).
            let rows = row_chars(g);
            let sum: usize = rows.iter().map(|r| r.1).sum();
            assert_eq!(sum, total, "tecken i raderna");
            assert_eq!(g.job.text, t);
            // Radnummer och kolumn för varje tecken hänger ihop.
            for i in 0..=total {
                let lc = g.layout_from_cursor(egui::text::CCursor::new(i));
                let pos = g.pos_from_cursor(egui::text::CCursor::new(i));
                let r = &g.rows[lc.row];
                assert!(pos.top() >= r.min_y() - 0.01 && pos.top() <= r.max_y() + 0.01);
                assert!(rows[lc.row].0 + lc.column.0 == i, "tecken {i}");
            }
            // Klick på första tecknet i varje ord (osynlig markup hoppas över) ger tillbaka tecknet.
            let mut checked = 0;
            for (i, c) in t.chars().enumerate() {
                let line_start = t.chars().take(i).collect::<String>().rfind('\n').map_or(0, |k| k + 1);
                let in_tag = t[line_start..].starts_with('<');
                if !in_tag && c.is_alphabetic() && (i == 0 || t.chars().nth(i - 1).is_some_and(|p| p == ' ')) {
                    let r = g.pos_from_cursor(egui::text::CCursor::new(i));
                    let back = g.cursor_from_pos((r.left_center() + egui::vec2(1.0, 0.0)).to_vec2()).index.0;
                    assert_eq!(back, i, "tecken {i}");
                    checked += 1;
                }
            }
            assert!(checked > 20);

            // Centrerat, högerjusterat och marginaljusterat.
            let row_of = |needle: &str| g.layout_from_cursor(egui::text::CCursor::new(ci(&t, needle))).row;
            let mid = &g.rows[row_of("Mitten")];
            let (l, r) = (mid.pos.x, mid.pos.x + content_end(&mid.row));
            assert!(((l + r) / 2.0 - wrap / 2.0).abs() < 1.5, "mitt {l}..{r}");
            let hog = &g.rows[row_of("Höger")];
            assert!((hog.pos.x + content_end(&hog.row) - wrap).abs() < 1.5);
            // Vänsterstycket börjar vid 0.
            assert_eq!(g.rows[0].pos.x, 0.0);
        });
    }

    #[test]
    fn justified_rows_fill_the_width() {
        let t = doc_text();
        let wrap = 400.0;
        with_doc_galley(&t, 0..0, false, wrap, &fam, |g, _| {
            let rows = row_chars(g);
            // De två justerade styckena: första raden i "Lorem" efter justify-diven.
            let start = t.find("<div align=\"justify\">").unwrap();
            let s_ci = t[..start].chars().count();
            let r0 = g.layout_from_cursor(egui::text::CCursor::new(s_ci)).row;
            let end_ci = ci(&t, "</div>\n\n<div align=\"right\">");
            let r1 = g.layout_from_cursor(egui::text::CCursor::new(end_ci)).row;
            let mut justified = 0;
            for i in r0..r1 {
                let row = &g.rows[i];
                if row.glyphs.is_empty() || row.ends_with_newline {
                    continue; // tomrad eller styckets sista rad
                }
                let edge = row.pos.x + content_end(&row.row);
                assert!((edge - wrap).abs() <= 1.0, "rad {i} slutar vid {edge} (bredd {wrap})");
                assert_eq!(rows[i].1, row.glyphs.len());
                justified += 1;
            }
            assert!(justified >= 3, "{justified}");
        });
        // Samma antal tecken som utan justering.
        let plain = t.replace("align=\"justify\"", "align=\"left\"");
        let n = |s: &str| with_doc_galley(s, 0..0, false, wrap, &fam, |g, _| g.rows.iter().map(|r| r.char_count_including_newline().0).sum::<usize>());
        assert_eq!(n(&t), t.chars().count());
        assert_eq!(n(&plain), plain.chars().count());
    }

    #[test]
    fn hidden_tag_lines_collapse() {
        let t = "<!-- kladd: font-size: 12pt -->\n\n<div align=\"center\">\n\nMitten\n\n</div>\n\nSlut";
        let heights = |reveal: Range<usize>| {
            with_doc_galley(t, reveal, false, 400.0, &fam, |g, _| {
                let h = |needle: &str| g.rows[g.layout_from_cursor(egui::text::CCursor::new(ci(t, needle))).row].height();
                (h("<!--"), h("<div"), h("Mitten"), h("</div>"), g.rect.height())
            })
        };
        let (c, d, m, e, total) = heights(0..0);
        assert!(c < 1.0 && d < 1.0 && e < 1.0, "{c} {d} {e}");
        assert!(m > 14.0);
        // Raderna däremellan är ihopkrympta, så hela blocket är lågt: text + ett mellanrum före "Slut".
        assert!(total < 60.0, "total {total}");
        // Med markören på taggraden visas den.
        let at = t.find("<div").unwrap();
        let (_, d2, ..) = heights(at..at + 20);
        assert!(d2 > 10.0, "{d2}");
    }

    #[test]
    fn span_font_size_color_and_background() {
        let t = "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt -->\n\nStandard <span style=\"font-family: Arial; font-size: 16pt; color: #FF0000; background-color: #FFFF00\">röd</span> slut\n\n# Rubrik <span style=\"color: #0000FF\">blå</span>\n";
        let times = FontFamily::Name("times".into());
        let arial = FontFamily::Name("arial".into());
        let ff = |n: Option<&str>, b: bool, i: bool| match n {
            Some("Times New Roman") => times.clone(),
            Some("Arial") => arial.clone(),
            _ => theme::doc_font(b, i),
        };
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let p = Params {
            palette: &pal,
            zoom: 1.0,
            reveal: 0..0,
            query: "",
            case_sensitive: false,
            current_match: None,
            image_size: &img,
            measure: &measure,
            font_family: &ff,
            fonts_gen: 0,
        };
        let job = layout_job(t, &markup::analyze(t), &p);
        let std = sec_at(&job, t.find("Standard").unwrap());
        assert_eq!(std.format.font_id.family, times);
        assert_eq!(std.format.font_id.size, px(1.0, 12.0));
        let red = sec_at(&job, t.find("röd").unwrap());
        assert_eq!(red.format.font_id.family, arial);
        assert_eq!(red.format.font_id.size, px(1.0, 16.0));
        assert_eq!(red.format.color, Color32::from_rgb(255, 0, 0));
        assert_eq!(red.format.background, Color32::from_rgb(255, 255, 0));
        // Rubrik: Kladds stil, men spanens färg vinner.
        let h = sec_at(&job, t.find("Rubrik").unwrap());
        assert_eq!(h.format.font_id.size, px(1.0, 18.0));
        assert_eq!(h.format.font_id.family, theme::doc_font(true, false));
        assert_eq!(h.format.color, pal.page_heading1);
        let b = sec_at(&job, t.find("blå").unwrap());
        assert_eq!(b.format.color, Color32::from_rgb(0, 0, 255));
        assert_eq!(b.format.font_id.size, px(1.0, 18.0));
        // Sökträffar går före markeringsfärgen.
        let mut q = Params { query: "röd", ..p };
        q.palette = &pal;
        let job = Cache::default().job(1, t, &q);
        assert_eq!(sec_at(&job, t.find("röd").unwrap()).format.background, pal.match_bg);
        // De efterfrågade typsnitten: grundtypsnittet och spanens, i rätt variant.
        let need = needed_fonts(&markup::analyze(t));
        assert!(need.contains(&("Times New Roman".to_owned(), false, false)));
        assert!(need.contains(&("Arial".to_owned(), false, false)));
        assert!(need.contains(&("Times New Roman".to_owned(), true, false)) == false);
    }

    #[test]
    fn dark_mode_color_adaptation() {
        let page = theme::palette(true).page;
        let rgb = |c: Color32| [c.r(), c.g(), c.b()];
        let black = adapt_text_color([0, 0, 0], page, true);
        assert!(contrast(rgb(black), rgb(page)) > 7.0, "{black:?}");
        let navy = adapt_text_color([0, 0, 128], page, true);
        assert!(navy.b() > 200 && navy.r() > 100, "{navy:?}");
        assert!(contrast(rgb(navy), rgb(page)) >= 3.0);
        let blue = adapt_text_color([0, 0, 255], page, true);
        assert!(contrast(rgb(blue), rgb(page)) >= 3.0 && blue.b() > 200, "{blue:?}");
        // Redan synliga färger och ljust tema lämnas.
        assert_eq!(adapt_text_color([255, 128, 0], page, true), Color32::from_rgb(255, 128, 0));
        assert_eq!(adapt_text_color([0, 0, 0], theme::palette(false).page, false), Color32::BLACK);
        // Gult markeringsfärg blir mörk olivgrön i mörkt tema, orörd i ljust.
        let y = adapt_background([255, 255, 0], true);
        assert!(y.g() < 120 && y.r() < 120 && y.b() < 60 && y.g() > 40, "{y:?}");
        assert_eq!(adapt_background([255, 255, 0], false), Color32::from_rgb(255, 255, 0));
        // Mörka skuggningar lämnas.
        assert_eq!(adapt_background([0, 0, 128], true), Color32::from_rgb(0, 0, 128));
        // HSL-rundresa.
        for c in [[200u8, 30, 90], [10, 200, 120], [128, 128, 128]] {
            let (h, s, l) = rgb_to_hsl(c);
            let back = hsl_to_rgb(h, s, l);
            for k in 0..3 {
                assert!((back[k] as i32 - c[k] as i32).abs() <= 1, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn galley_cache_hit_and_miss() {
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let p = Params {
            palette: &pal,
            zoom: 1.0,
            reveal: 0..0,
            query: "",
            case_sensitive: false,
            current_match: None,
            image_size: &img,
            measure: &measure,
            font_family: &fam,
            fonts_gen: 0,
        };
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut c = Cache::default();
        let t = "Hej **du**\n\nandra";
        let mut res = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(Default::default(), |ui| {
                let ctx = ui.ctx().clone();
                let a = c.galley(&ctx, 1, t, &p, 300.0);
                let b = c.galley(&ctx, 1, t, &p, 300.0);
                let d = c.galley(&ctx, 1, t, &p, 200.0);
                res = vec![Arc::ptr_eq(&a, &b), Arc::ptr_eq(&a, &d)];
            });
            std::mem::forget(out);
        }
        assert_eq!(res, vec![true, false]);
        c.forget(1);
        assert!(c.galleys.is_empty());
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
            font_family: &fam,
            fonts_gen: 0,
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
            font_family: &fam,
            fonts_gen: 0,
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
