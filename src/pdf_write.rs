//! Sparar Markdown som PDF.
//!
//! Upplägget följer WordCraft (github.com/storytold/wordcraft, MIT/Apache-2.0), som
//! också skriver PDF med `krilla`: typsnitten bäddas in som delmängder, texten går att
//! markera och söka i, rubriker blir bokmärken och länkar blir klickbara.
//!
//! Kladd har en egen enkel sidlayout: sidstorlek och marginaler från grundformatraden,
//! IBM Plex Sans för brödtext och rubriker,
//! IBM Plex Mono för kod, radbrytning mellan ord (mätt med rustybuzz, samma formning
//! som krilla använder när texten ritas), sidhuvud och sidfot med sidnummer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, LineCap, LineJoin, Stroke};
use krilla::text::Font;
use krilla::Document;
use krilla::image::Image;
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::docx::Image as DocImage;
use crate::style::{self, Align, DocDefaults, SpanStyle};
use crate::theme;
use crate::word::anchor::{Anchor, PageCtx, Reserve, VAt, Wrap};
use crate::word::markup::Float;
use crate::word::sections::{self, Section, SectionProps};

// --- mått i punkter (1/72 tum); sidstorlek och marginaler kommer från dokumentet ---
const BODY_SIZE: f32 = 10.5;
const LINE: f32 = 1.5;
const PARA_GAP: f32 = 7.5;
const INDENT: f32 = 16.0;
/// Radhöjd för enkelt radavstånd i formatmallarnas `line-spacing`, gånger teckenstorleken.
const LINE_SINGLE: f32 = 1.3;

// --- färger --------------------------------------------------------------------
const INK: (u8, u8, u8) = (0x1E, 0x28, 0x36);
const MUTED: (u8, u8, u8) = (0x66, 0x74, 0x8A);
const RULE: (u8, u8, u8) = (0xD0, 0xD8, 0xE3);
const LINK: (u8, u8, u8) = (0x1F, 0x63, 0xA8);
const CODE_BG: (u8, u8, u8) = (0xF1, 0xF4, 0xF8);
const ACCENT: (u8, u8, u8) = (0xB8, 0x65, 0x0F);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Face {
    Sans,
    SansMedium,
    SansSemi,
    Mono,
    MonoMedium,
    /// Carlito (Calibri) och Caladea (Cambria): används när dokumentet ber om dem, se `word::fontmap`.
    Carlito,
    CarlitoBold,
    Serif,
    SerifBold,
}

const FACES: [Face; 9] = [
    Face::Sans,
    Face::SansMedium,
    Face::SansSemi,
    Face::Mono,
    Face::MonoMedium,
    Face::Carlito,
    Face::CarlitoBold,
    Face::Serif,
    Face::SerifBold,
];

impl Face {
    /// Fet variant av en löptextsnitt (mono har ingen: den behåller sin).
    fn strong(self) -> Face {
        match self {
            Face::Sans => Face::SansSemi,
            Face::Carlito => Face::CarlitoBold,
            Face::Serif => Face::SerifBold,
            f => f,
        }
    }

    /// Normal vikt av en fet variant.
    fn plain(self) -> Face {
        match self {
            Face::SansSemi => Face::Sans,
            Face::CarlitoBold => Face::Carlito,
            Face::SerifBold => Face::Serif,
            f => f,
        }
    }

    fn is_mono(self) -> bool {
        matches!(self, Face::Mono | Face::MonoMedium)
    }

    /// Löptextsnittet för ett typsnittsnamn: Calibri blir Carlito, serif (Cambria, Times,
    /// Georgia...) Caladea, mono Plex Mono och allt annat Plex Sans.
    fn for_family(name: &str) -> Face {
        match crate::word::fontmap::bundled(name) {
            Some(crate::word::fontmap::Bundled::Carlito) => Face::Carlito,
            Some(crate::word::fontmap::Bundled::Caladea) => Face::Serif,
            None => match classify_family(name) {
                Family::Mono => Face::Mono,
                Family::Serif => Face::Serif,
                Family::Sans => Face::Sans,
            },
        }
    }

    fn data(self) -> &'static [u8] {
        match self {
            Face::Carlito => theme::CARLITO,
            Face::CarlitoBold => theme::CARLITO_BOLD,
            Face::Serif => crate::word::fontmap::CALADEA,
            Face::SerifBold => crate::word::fontmap::CALADEA_BOLD,
            Face::Sans => theme::PLEX_SANS,
            Face::SansMedium => theme::PLEX_SANS_MEDIUM,
            Face::SansSemi => theme::PLEX_SANS_SEMIBOLD,
            Face::Mono => theme::PLEX_MONO,
            Face::MonoMedium => theme::PLEX_MONO_MEDIUM,
        }
    }
}

/// Mäter textbredd med samma formning (rustybuzz) som krilla använder.
/// Typsnittsytorna och mätcachen delas (`Rc`) mellan utläggningspassen och ögonblicksbilderna, så
/// att varje ord bara formas en gång per export.
#[derive(Clone)]
struct Measurer {
    shaper: Rc<Shaper>,
    cache: Rc<RefCell<HashMap<Face, HashMap<String, f32>>>>,
}

/// Typsnittsytorna (rustybuzz) och färdiga formningsplaner. `rustybuzz::shape` bygger en plan för
/// typsnittets GSUB/GPOS vid varje anrop (~30 µs); här görs det en gång per typsnitt, riktning och skrift.
struct Shaper {
    faces: HashMap<Face, rustybuzz::Face<'static>>,
    plans: RefCell<HashMap<(Face, rustybuzz::Direction, rustybuzz::Script), rustybuzz::ShapePlan>>,
}

impl Shaper {
    fn new() -> Self {
        let faces = FACES.into_iter().map(|f| (f, rustybuzz::Face::from_slice(f.data(), 0).expect("inbyggt typsnitt"))).collect();
        Self { faces, plans: RefCell::default() }
    }

    /// Samma resultat som `rustybuzz::shape(face, &[], buf)`.
    fn shape(&self, face: Face, mut buf: rustybuzz::UnicodeBuffer) -> rustybuzz::GlyphBuffer {
        buf.guess_segment_properties();
        let f = &self.faces[&face];
        let script = buf.script();
        // Text utan skrift (bara siffror och blanksteg) har ingen plan att dela.
        if script == rustybuzz::script::UNKNOWN {
            return rustybuzz::shape(f, &[], buf);
        }
        let key = (face, buf.direction(), script);
        let mut plans = self.plans.borrow_mut();
        let plan = plans.entry(key).or_insert_with(|| rustybuzz::ShapePlan::new(f, key.1, Some(script), None, &[]));
        rustybuzz::shape_with_plan(f, plan, buf)
    }
}

impl Measurer {
    fn new() -> Self {
        Self { shaper: Rc::new(Shaper::new()), cache: Rc::default() }
    }

    /// Bredd i punkter för `text` i storlek `size`.
    fn width(&mut self, face: Face, size: f32, text: &str) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let hit = self.cache.borrow().get(&face).and_then(|m| m.get(text).copied());
        let units = if let Some(w) = hit {
            w
        } else {
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(text);
            let out = self.shaper.shape(face, buf);
            let w: i32 = out.glyph_positions().iter().map(|g| g.x_advance).sum();
            let w = w as f32 / self.shaper.faces[&face].units_per_em() as f32;
            let mut cache = self.cache.borrow_mut();
            let per_face = cache.entry(face).or_default();
            if per_face.len() < 50_000 {
                per_face.insert(text.to_owned(), w);
            }
            w
        };
        units * size
    }
}

/// Formar text som `krilla::Surface::draw_text` gör (`naive_shape`, riktning automatisk), men med en
/// rustybuzz-yta som återanvänds: `draw_text` tolkar om hela typsnittets tabeller vid varje anrop, och
/// det var det mesta av tiden för ritningen av sidorna.
fn shape_glyphs(shaper: &Shaper, face: Face, text: &str) -> Vec<krilla::text::KrillaGlyph> {
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let dir = buffer.direction();
    let output = shaper.shape(face, buffer);
    let (positions, infos) = (output.glyph_positions(), output.glyph_infos());
    let upm = shaper.faces[&face].units_per_em() as f32;
    let ltr = dir == rustybuzz::Direction::LeftToRight || dir == rustybuzz::Direction::TopToBottom;
    (0..output.len())
        .map(|i| {
            let (pos, info) = (positions[i], infos[i]);
            let start = info.cluster as usize;
            // Slutet av klustret: nästa glyf med annat kluster (framåt vid vänster-till-höger, annars bakåt).
            let end = if ltr {
                (i + 1..infos.len()).find(|&j| infos[j].cluster != info.cluster).map(|j| infos[j].cluster as usize)
            } else {
                (0..i).rev().find(|&j| infos[j].cluster != info.cluster).map(|j| infos[j].cluster as usize)
            }
            .unwrap_or(text.len());
            krilla::text::KrillaGlyph::new(
                krilla::text::GlyphId::new(info.glyph_id),
                pos.x_advance as f32 / upm,
                pos.x_offset as f32 / upm,
                pos.y_offset as f32 / upm,
                pos.y_advance as f32 / upm,
                start..end,
                None,
            )
        })
        .collect()
}

/// Antal sidor vars texter formas åt gången.
const SHAPE_BATCH: usize = 32;

/// Formar texterna i sidorna `jobs` (en lista per sida: typsnitt och text) på flera trådar. Formningen är
/// det mesta av tiden för att rita sidorna och beror inte på något annat. Svaret har samma form som `jobs`.
fn shape_pages(jobs: &[Vec<(Face, &str)>], shaper: &Shaper) -> Vec<Vec<Vec<krilla::text::KrillaGlyph>>> {
    let flat: Vec<(Face, &str)> = jobs.iter().flatten().copied().collect();
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8).min(flat.len() / 200 + 1);
    let shaped: Vec<Vec<krilla::text::KrillaGlyph>> = if workers <= 1 {
        flat.iter().map(|(f, t)| shape_glyphs(shaper, *f, t)).collect()
    } else {
        let per = flat.len().div_ceil(workers);
        std::thread::scope(|sc| {
            let handles: Vec<_> = flat
                .chunks(per)
                .map(|chunk| {
                    sc.spawn(move || {
                        let shaper = Shaper::new();
                        chunk.iter().map(|(f, t)| shape_glyphs(&shaper, *f, t)).collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().expect("formningstråd")).collect()
        })
    };
    let mut it = shaped.into_iter();
    jobs.iter().map(|j| it.by_ref().take(j.len()).collect()).collect()
}

// ============================================================== blockmodell

#[derive(Clone, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    underline: bool,
    link: Option<String>,
    /// Färg, bakgrund, storlek och typsnittsklass från omgivande `<span style="…">`.
    span: SpanStyle,
    /// Fotnotsnummer (1-baserat): texten ritas som litet upphöjt nummer.
    fnref: Option<usize>,
    /// Slutnotsnummer (romerskt tal): ritas upphöjt men hör inte till någon sida.
    endref: bool,
    /// Formel (LaTeX-kod, egen rad): ritas som glyfer; `Span::text` är koden.
    math: Option<(String, bool)>,
}

#[derive(Clone)]
struct Span {
    text: String,
    style: Style,
}

enum Block {
    Heading(u8, Vec<Span>, Align),
    /// `look`: ram, fyllning och bredd ur `<!--k:pp …-->` (`word::pbox`).
    Para { spans: Vec<Span>, indent: f32, quote: u8, align: Align, look: Option<crate::word::pbox::PPr> },
    Item { spans: Vec<Span>, indent: f32, marker: String, quote: u8, align: Align },
    Code { text: String, indent: f32 },
    /// `spans`: 1 vanlig cell, n sammanfogad, 0 täckt; `weights`: relativ kolumnbredd (streck).
    Table { rows: Vec<Vec<Vec<Span>>>, spans: Vec<Vec<usize>>, weights: Option<Vec<usize>>, fmt: Option<crate::word::tablefmt::TableFmt> },
    Rule,
    Image { src: String, alt: String, indent: f32, size: ImgSize, align: Align, float: Option<Float>, place: ImgPlace },
    /// Innehållsförteckning (`<!-- kladd: toc -->`).
    Toc,
    /// Nytt avsnitt (`<!-- kladd: avsnitt; … -->`): egen sidstorlek, marginaler och spalter.
    SectionBreak(SectionProps),
    /// Spaltbrytning (`<!-- kladd: spaltbrytning -->`).
    ColumnBreak,
    PageBreak,
    /// Förankrad textruta med omflöde (`<!--k:tb N …-->`) och dess slut: stycken däremellan ritas på rutans läge.
    TBoxOpen(crate::docx_tbox::Head),
    TBoxClose,
}

/// Tar bort `<!--k:N-->` (ogenomskinliga Word-objekt) som aldrig ska skrivas ut.
fn strip_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<!--k:") {
        let after = &rest[i + 6..];
        match after.find("-->") {
            Some(j) if after[..j].chars().all(|c| c.is_ascii_digit()) || crate::word::pbox::is_marker(&rest[i..i + 6 + j + 3]) || crate::docx_tbox::is_mark(&rest[i..i + 6 + j + 3]) => {
                out.push_str(&rest[..i]);
                rest = &after[j + 3..];
            }
            _ => {
                out.push_str(&rest[..i + 6]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Värdet av attributet `name` i en tagg (med citattecken).
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[at + name.len()..].trim_start();
        if before_ok {
            if let Some(r) = rest.strip_prefix('=') {
                let r = r.trim_start();
                let q = r.chars().next()?;
                if q == '"' || q == '\'' {
                    let end = r[1..].find(q)?;
                    return Some(r[1..1 + end].to_owned());
                }
            }
        }
        from = at + name.len();
    }
    None
}

/// `<img src="…" alt="…" width="W" [height="H"]>` med storlek i CSS-px (1/96 tum) omräknad till pt.
/// Förankring och beskärning av en `<img>` (se `word::anchor`).
#[derive(Clone, Copy, Default)]
struct ImgPlace {
    anchor: Option<Anchor>,
    /// Beskärning i procent: vänster, ovan, höger, under.
    crop: Option<[f32; 4]>,
}

fn parse_img(tag: &str) -> Option<(String, String, ImgSize, Option<Float>, ImgPlace)> {
    if !tag.get(..4)?.eq_ignore_ascii_case("<img") || !tag.ends_with('>') {
        return None;
    }
    let src = attr(tag, "src")?;
    let alt = attr(tag, "alt").unwrap_or_default();
    let px = |n: &str| attr(tag, n).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| *v > 0.0).map(|v| v * 0.75);
    let float = match attr(tag, "align").map(|a| a.trim().to_ascii_lowercase()).as_deref() {
        Some("left") => Some(Float::Left),
        Some("right") => Some(Float::Right),
        _ => None,
    };
    let anchor = Anchor::from_attrs(&|k| attr(tag, k));
    let crop = attr(tag, "crop").and_then(|v| {
        let n: Vec<f32> = v.split_whitespace().filter_map(|s| s.parse::<f32>().ok()).filter(|x| x.is_finite() && (0.0..100.0).contains(x)).collect();
        (n.len() == 4 && n[0] + n[2] < 100.0 && n[1] + n[3] < 100.0 && n.iter().any(|x| *x > 0.0)).then(|| [n[0], n[1], n[2], n[3]])
    });
    // Förankrad bild: `align` är bara en antydan (kortformen gäller bara utan ankare).
    let float = float.filter(|_| anchor.is_none());
    Some((src, alt, ImgSize { w: px("width"), h: px("height") }, float, ImgPlace { anchor, crop }))
}

/// Rubriknivåerna som Titel och Underrubrik lagras med (över Markdowns 1-6).
const TITLE_LEVEL: u8 = 100;
const SUBTITLE_LEVEL: u8 = 101;

fn heading_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
fn parse_blocks(md: &str) -> Vec<Block> {
    parse_blocks_notes(md).0
}

/// Blocken och fotnoterna (text per fotnot, i den ordning referenserna kommer).
fn parse_blocks_notes(md: &str) -> (Vec<Block>, Vec<Vec<Span>>) {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_MATH);
    opts.insert(crate::style::titles::OPTS);
    // Fotnoter: etiketter i ordning (nummer = plats + 1), definitionernas text och pågående definition.
    let nf = doc_defaults(md).notes;
    let mut note_labels: Vec<String> = Vec::new();
    let mut end_labels: Vec<String> = Vec::new();
    let mut note_defs: Vec<(String, Vec<Span>)> = Vec::new();
    let mut note_cur: Option<(String, Vec<Block>)> = None;

    let mut blocks = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut bold = 0u32;
    let mut italic = 0u32;
    let mut strike = 0u32;
    let mut underline = 0u32;
    // Öppna `<span style>` (inre ovanpå yttre) och `<div align>` (innersta gäller).
    let mut span_stack: Vec<SpanStyle> = Vec::new();
    let mut aligns: Vec<Align> = Vec::new();
    let mut heading: Option<u8> = None;
    let mut quote = 0u8;
    // Listor: nästa nummer (None = punktlista)
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut pending_marker: Option<String> = None;
    let mut code: Option<String> = None;
    let mut image: Option<(String, String)> = None;
    let mut table: Option<Vec<Vec<Vec<Span>>>> = None;
    let mut row: Vec<Vec<Span>> = Vec::new();

    // Styckeutseendet (`k:pp`) som hör till stycket som läses; `flush` lägger det på stycket.
    let look_cell: std::cell::RefCell<Option<crate::word::pbox::PPr>> = std::cell::RefCell::new(None);
    let indent_now = |lists: &Vec<Option<u64>>, quote: u8| lists.len() as f32 * INDENT + quote as f32 * 14.0;

    let flush = |spans: &mut Vec<Span>,
                 blocks: &mut Vec<Block>,
                 heading: Option<u8>,
                 pending_marker: &mut Option<String>,
                 lists: &Vec<Option<u64>>,
                 quote: u8,
                 align: Align| {
        let look = look_cell.borrow_mut().take();
        if spans.iter().all(|s| s.text.trim().is_empty()) && pending_marker.is_none() {
            spans.clear();
            return;
        }
        let taken = std::mem::take(spans);
        // En rad som bara är en `$$…$$`-formel centreras.
        let lone_display = taken.iter().filter(|s| !s.text.trim().is_empty()).count() == 1
            && taken.iter().any(|s| s.style.math.as_ref().is_some_and(|m| m.1));
        let align = if lone_display && align == Align::Left { Align::Center } else { align };
        let indent = indent_now(lists, quote);
        if let Some(h) = heading {
            blocks.push(Block::Heading(h, taken, align));
        } else if let Some(m) = pending_marker.take() {
            blocks.push(Block::Item { spans: taken, indent, marker: m, quote, align });
        } else {
            blocks.push(Block::Para { spans: taken, indent, quote, align, look: look.filter(|l| l.has_box() || l.frame.is_some()) });
        }
    };

    let push_text = |spans: &mut Vec<Span>, text: &str, style: &Style| {
        if let Some(last) = spans.last_mut() {
            if last.style == *style {
                last.text.push_str(text);
                return;
            }
        }
        spans.push(Span { text: text.to_owned(), style: style.clone() });
    };

    let mut table_src: Option<crate::word::markup::TableSrc> = None;
    // Tabellformatet (`<!-- kladd-tabell: … -->`) som väntar på tabellen efter det.
    let mut pending_tfmt: Option<crate::word::tablefmt::TableFmt> = None;
    for (ev, range) in Parser::new_ext(md, opts).into_offset_iter() {
        let is_display = matches!(ev, MdEvent::DisplayMath(_));
        let align = aligns.last().copied().unwrap_or_default();
        style.span = span_stack.iter().fold(SpanStyle::default(), |acc, s| acc.merged(s));
        style.bold = bold > 0;
        style.italic = italic > 0;
        style.strike = strike > 0;
        style.underline = underline > 0;
        match ev {
            MdEvent::Start(Tag::FootnoteDefinition(label)) => {
                flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                note_cur = Some((label.to_string(), std::mem::take(&mut blocks)));
            }
            MdEvent::End(TagEnd::FootnoteDefinition) => {
                flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                if let Some((label, saved)) = note_cur.take() {
                    // Styckena i definitionen läggs efter varandra med ett mellanslag emellan.
                    let mut text: Vec<Span> = Vec::new();
                    for b in std::mem::replace(&mut blocks, saved) {
                        if let Block::Para { spans, .. } | Block::Item { spans, .. } = b {
                            if !text.is_empty() {
                                text.push(Span { text: " ".into(), style: Style::default() });
                            }
                            text.extend(spans);
                        }
                    }
                    note_defs.push((label, text));
                }
            }
            MdEvent::Start(tag) => match tag {
                Tag::Heading { level, classes, .. } => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    heading = Some(match crate::style::titles::kind(&classes) {
                        Some(crate::style::titles::TitleKind::Title) => TITLE_LEVEL,
                        Some(crate::style::titles::TitleKind::Subtitle) => SUBTITLE_LEVEL,
                        None => heading_num(level),
                    });
                }
                Tag::Paragraph => {
                    if pending_marker.is_none() {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    }
                }
                Tag::BlockQuote(_) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    quote += 1;
                }
                Tag::CodeBlock(_) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    code = Some(String::new());
                }
                Tag::List(start) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    lists.push(start);
                }
                Tag::Item => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    let depth = lists.len().saturating_sub(1);
                    pending_marker = Some(match lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}.");
                            *n += 1;
                            m
                        }
                        _ => ["•", "–", "·"][depth % 3].to_owned(),
                    });
                }
                Tag::Table(_) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    table_src = Some(crate::word::markup::table_src(md.get(range.clone()).unwrap_or("")));
                    table = Some(Vec::new());
                }
                Tag::TableHead | Tag::TableRow => row = Vec::new(),
                Tag::TableCell => spans.clear(),
                Tag::Emphasis => italic += 1,
                Tag::Strong => bold += 1,
                Tag::Strikethrough => strike += 1,
                Tag::Link { dest_url, .. } => {
                    let url = dest_url.to_string();
                    // Bara säkra länktyper blir klickbara (som i WordCraft).
                    let lower = url.to_ascii_lowercase();
                    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
                        style.link = Some(url);
                    }
                }
                Tag::Image { dest_url, .. } => image = Some((dest_url.to_string(), String::new())),
                _ => {}
            },
            MdEvent::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    flush(&mut spans, &mut blocks, heading, &mut pending_marker, &lists, quote, align);
                    heading = None;
                }
                TagEnd::Paragraph => flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align),
                TagEnd::BlockQuote(_) => {
                    flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                    quote = quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    if let Some(c) = code.take() {
                        blocks.push(Block::Code {
                            text: c.trim_end_matches('\n').to_owned(),
                            indent: indent_now(&lists, quote),
                        });
                    }
                }
                TagEnd::Item => flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align),
                TagEnd::List(_) => {
                    lists.pop();
                }
                TagEnd::TableCell => row.push(std::mem::take(&mut spans)),
                TagEnd::TableHead | TagEnd::TableRow => {
                    if let Some(t) = table.as_mut() {
                        t.push(std::mem::take(&mut row));
                    }
                }
                TagEnd::Table => {
                    if let Some(rows) = table.take() {
                        if !rows.is_empty() {
                            let src = table_src.take().unwrap_or(crate::word::markup::TableSrc { cols: 0, spans: Vec::new(), weights: None });
                            blocks.push(Block::Table { rows, spans: src.spans, weights: src.weights, fmt: pending_tfmt.take() });
                        }
                    }
                }
                TagEnd::Emphasis => italic = italic.saturating_sub(1),
                TagEnd::Strong => bold = bold.saturating_sub(1),
                TagEnd::Strikethrough => strike = strike.saturating_sub(1),
                TagEnd::Link => style.link = None,
                TagEnd::Image => {
                    if let Some((src, alt)) = image.take() {
                        // En bild i en tabellcell eller mitt i text ritas som eget block efter stycket.
                        flush(&mut spans, &mut blocks, heading, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::Image { src, alt, indent: indent_now(&lists, quote), size: ImgSize::default(), align: Align::Left, float: None, place: ImgPlace::default() });
                    }
                }
                _ => {}
            },
            MdEvent::Text(t) => {
                if let Some(c) = code.as_mut() {
                    c.push_str(&t);
                } else if let Some((_, alt)) = image.as_mut() {
                    alt.push_str(&t);
                } else {
                    push_text(&mut spans, &t, &style);
                }
            }
            MdEvent::Code(t) => {
                let mut s = style.clone();
                s.code = true;
                push_text(&mut spans, &t, &s);
            }
            MdEvent::InlineMath(t) | MdEvent::DisplayMath(t) => {
                let display = is_display;
                let src = t.trim().to_owned();
                if crate::math::formula(&src, display).is_some() && image.is_none() {
                    let mut s = style.clone();
                    s.math = Some((src.clone(), display));
                    spans.push(Span { text: src, style: s });
                } else {
                    let d = if display { "$$" } else { "$" };
                    push_text(&mut spans, &format!("{d}{t}{d}"), &style);
                }
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => push_text(&mut spans, "\n", &style),
            MdEvent::Html(t) | MdEvent::InlineHtml(t) => {
                // Ett HTML-block kan ha flera rader; varje rad som är en känd tagg tolkas för sig.
                let mut first = true;
                for line in t.split('\n') {
                    if let Some(pp) = crate::word::pbox::PPr::parse_marker(line.trim()) {
                        *look_cell.borrow_mut() = Some(pp);
                    } else if let Some((_, head)) = crate::docx_tbox::parse_head(line.trim()).filter(|(_, h)| h.anchor.is_some_and(|a| a.wrap != Wrap::TopBottom)) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::TBoxOpen(head));
                    } else if line.trim() == crate::docx_tbox::CLOSE {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::TBoxClose);
                    }
                }
                let t = strip_markers(&t);
                for line in t.split('\n') {
                    let tag = line.trim();
                    if tag.is_empty() {
                        continue;
                    }
                    if tag.eq_ignore_ascii_case("<br>") || tag.eq_ignore_ascii_case("<br/>") {
                        push_text(&mut spans, "\n", &style);
                    } else if tag.eq_ignore_ascii_case("<u>") {
                        underline += 1;
                    } else if tag.eq_ignore_ascii_case("</u>") {
                        underline = underline.saturating_sub(1);
                    } else if let Some(st) = style::parse_span_open(tag) {
                        span_stack.push(st);
                        style.span = span_stack.iter().fold(SpanStyle::default(), |acc, s| acc.merged(s));
                    } else if style::is_span_close(tag) {
                        span_stack.pop();
                        style.span = span_stack.iter().fold(SpanStyle::default(), |acc, s| acc.merged(s));
                    } else if let Some(a) = style::parse_div_open(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        aligns.push(a);
                    } else if style::is_div_close(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        aligns.pop();
                    } else if let Some((src, alt, size, float, place)) = parse_img(tag) {
                        // `<img>` med storlek blir ett eget bildblock, justerat av omgivande `<div align>`.
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::Image { src, alt, indent: indent_now(&lists, quote), size, align, float, place });
                    } else if tag == crate::word::nav::TOC_LINE {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::Toc);
                    } else if let Some(props) = SectionProps::from_comment(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::SectionBreak(props));
                    } else if sections::is_page_break(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::PageBreak);
                    } else if sections::is_column_break(tag) {
                        flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                        blocks.push(Block::ColumnBreak);
                    } else if crate::word::tablefmt::is_comment(tag) {
                        pending_tfmt = crate::word::tablefmt::TableFmt::from_comment(tag);
                    } else if tag.starts_with("<!--") && tag.ends_with("-->") {
                        // Kommentarer (också grundformatraden) skrivs inte ut.
                    } else {
                        if !first {
                            push_text(&mut spans, "\n", &style);
                        }
                        push_text(&mut spans, line.trim_end_matches('\r'), &style);
                        first = false;
                    }
                }
            }
            MdEvent::Rule => {
                flush(&mut spans, &mut blocks, None, &mut pending_marker, &lists, quote, align);
                blocks.push(Block::Rule);
            }
            MdEvent::TaskListMarker(done) => push_text(&mut spans, if done { "☑ " } else { "☐ " }, &style),
            MdEvent::FootnoteReference(t) if note_cur.is_some() => push_text(&mut spans, &format!("[{t}]"), &style),
            // Slutnoter: romersk siffra upphöjd i texten, noten kommer efter sista texten.
            MdEvent::FootnoteReference(t) if crate::word::markup::is_endnote_label(&t) => {
                let n = match end_labels.iter().position(|l| l.as_str() == &*t) {
                    Some(i) => i + 1,
                    None => {
                        end_labels.push(t.to_string());
                        end_labels.len()
                    }
                };
                let st = Style { endref: true, ..style.clone() };
                spans.push(Span { text: nf.number(true, n), style: st });
            }
            MdEvent::FootnoteReference(t) => {
                // Samma etikett flera gånger är samma fotnot (som i Markdown).
                let n = match note_labels.iter().position(|l| l.as_str() == &*t) {
                    Some(i) => i + 1,
                    None => {
                        note_labels.push(t.to_string());
                        note_labels.len()
                    }
                };
                let st = Style { fnref: Some(n), ..style.clone() };
                spans.push(Span { text: nf.number(false, n), style: st });
            }
        }
    }
    flush(&mut spans, &mut blocks, heading, &mut pending_marker, &lists, quote, aligns.last().copied().unwrap_or_default());
    let notes = note_labels
        .iter()
        .map(|l| note_defs.iter().find(|(d, _)| d == l).map(|(_, t)| t.clone()).unwrap_or_default())
        .collect();
    // Slutnoterna efter sista texten, under en linje (hela dokumentet, inte per sida).
    if !end_labels.is_empty() {
        blocks.push(Block::Rule);
        for (i, l) in end_labels.iter().enumerate() {
            let mut spans = vec![
                Span { text: nf.number(true, i + 1), style: Style { endref: true, ..Style::default() } },
                Span { text: " ".into(), style: Style::default() },
            ];
            spans.extend(note_defs.iter().find(|(d, _)| d == l).map(|(_, t)| t.clone()).unwrap_or_default());
            blocks.push(Block::Para { spans, indent: 0.0, quote: 0, align: Align::Left, look: None });
        }
    }
    (blocks, notes)
}

// ============================================================== sidlayout

/// En ritinstruktion på en sida.
#[derive(Clone)]
enum Op {
    Text { x: f32, y: f32, face: Face, size: f32, text: String, color: (u8, u8, u8), italic: bool },
    Rect { x: f32, y: f32, w: f32, h: f32, color: (u8, u8, u8) },
    /// Formel med origo på baslinjen (`y`), `em` pt per em.
    Math { x: f32, y: f32, src: String, display: bool, em: f32, color: (u8, u8, u8) },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: (u8, u8, u8) },
    Image { x: f32, y: f32, w: f32, h: f32, index: usize, crop: Option<[f32; 4]> },
    Link { x: f32, y: f32, w: f32, h: f32, url: String },
    /// Intern länk till en plats i dokumentet (sida från 0, y).
    Goto { x: f32, y: f32, w: f32, h: f32, page: usize, to_y: f32 },
}

impl Op {
    /// Flyttar operationen `dy` pt nedåt (en textruta som läggs på sitt läge).
    fn shift_y(&mut self, dy: f32) {
        match self {
            Op::Text { y, .. } | Op::Rect { y, .. } | Op::Math { y, .. } | Op::Image { y, .. } | Op::Link { y, .. } | Op::Goto { y, .. } => *y += dy,
            Op::Line { y1, y2, .. } => {
                *y1 += dy;
                *y2 += dy;
            }
        }
    }
}

/// Sidans mått (pt): bredd, höjd, marginaler och textytans vänsterkant och bredd (alla spalter).
#[derive(Clone, Copy, Debug, PartialEq)]
struct PageGeom {
    pw: f32,
    ph: f32,
    mt: f32,
    mb: f32,
    ml: f32,
    cw: f32,
}

#[derive(Clone)]
struct PageOps {
    ops: Vec<Op>,
    geom: PageGeom,
    /// Avsnittet sidan börjar i (för sidhuvud, sidfot och sidnummer).
    sec: usize,
}

#[derive(Clone)]
struct Heading {
    level: u8,
    text: String,
    page: usize,
    y: f32,
}

#[derive(Clone)]
struct Layout<'a> {
    m: Measurer,
    pages: Vec<PageOps>,
    y: f32,
    headings: Vec<Heading>,
    images: Vec<Pic>,
    resolve: &'a dyn Fn(&str) -> Option<DocImage>,
    missing: Vec<String>,
    /// Brödtextens storlek (dokumentets grundstorlek) och faktorn mot `BODY_SIZE`
    /// som rubriker, tabeller och kod skalas med.
    body: f32,
    scale: f32,
    /// Dokumentets formatmallar (`h1: …`, `body: …`, `quote: …`).
    styles: style::StyleSet,
    /// Radhöjd som faktor av teckenstorleken (`LINE` om formatmallen inte anger något).
    line: f32,
    /// Indrag av nästa blocks första rad (formatmallens `indent`).
    first_indent: f32,
    /// Grundtypsnitt för brödtext (Mono om dokumentets typsnitt är ett monospace).
    base: Face,
    /// Justering för blocket som läggs ut just nu.
    align: Align,
    /// Sidans mått och marginaler i pt (från grundformatraden): bredd, höjd, topp, botten, vänster
    /// och textbredd.
    pw: f32,
    ph: f32,
    mt: f32,
    mb: f32,
    ml: f32,
    cw: f32,
    /// Avsnittet: dokumentets grundformat, antal spalter, aktuell spalt (0-baserad), mellanrum, textytans
    /// vänsterkant och bredd, spaltens bredd och var spalterna börjar (y) på sidan.
    defaults: DocDefaults,
    cols: usize,
    col: usize,
    gap: f32,
    text_l: f32,
    text_w: f32,
    col_w: f32,
    /// Spaltbredderna när de är olika breda (annars tom), och skiljelinje mellan spalterna.
    col_ws: Vec<f32>,
    rule: bool,
    rt: f32,
    /// Längsta y som använts i spalter som lämnats på sidan (för löpande avsnitt).
    page_ymax: f32,
    /// Utjämning av spalterna på avsnittets sista sida före ett löpande byte: spalthöjd (pt från
    /// spalternas överkant) per (avsnitt, sida), och uppgifter om varje sådant byte (`SecEnd`).
    balance: Vec<(usize, usize, f32)>,
    sec_ends: Vec<SecEnd>,
    /// Summan av (höjd × bredd) i spalter som lämnats på sidan, och om en manuell spaltbrytning setts.
    col_area: f32,
    manual_col: bool,
    /// Avsnittet jämnar inte ut sina spalter (`column-balance: no`).
    no_balance: bool,
    /// Räknas upp vid varje spalt- eller sidbyte.
    breaks: usize,
    /// Avsnittet vi står i och om en avsnittsrad redan setts (en rad före all text gäller första avsnittet).
    cur_sec: usize,
    sec_row_seen: bool,
    /// Minsta marginal uppifrån och nedifrån per avsnitt för sidhuvud och sidfot (`hf::Plan::reserve`).
    hf_reserve: Rc<Vec<[f32; 2]>>,
    /// Fotnoternas text (nummer − 1), och de som hör till sidan som läggs ut just nu.
    notes: Rc<Vec<Vec<Span>>>,
    page_notes: Vec<usize>,
    /// Höjd längst ned på sidan som är reserverad för fotnoter (avskiljare och text).
    reserved: f32,
    /// Sant medan fotnoterna ritas (deras nummer är inga nya referenser).
    in_notes: bool,
    /// Flytande bilder som texten flyter runt just nu.
    floats: Vec<FloatBox>,
    /// Rutans fasta underkant (pt) medan en textruta med fast höjd ritas.
    box_min: Option<f32>,
    /// Rubrikerna (nivå 1–3) som innehållsförteckningen listar, och var de hamnade i förra
    /// passet (sida från 0, y). Antalet rader är detsamma i båda passen.
    toc_entries: Rc<Vec<(u8, String)>>,
    toc_pos: Vec<(usize, f32)>,
    /// Avkodade bilder per källa, delade mellan utläggningspassen (varje bild avkodas en gång per export).
    pics: Rc<RefCell<HashMap<String, PicLoad>>>,
    /// Sidhuvudets och sidfotens bilder: index i `images` och storlek (pt) per källa.
    hf_pics: HashMap<String, Option<(usize, f32, f32)>>,
}

/// En bildkälla som lästs och avkodats (eller inte gick att läsa respektive avkoda).
#[derive(Clone)]
enum PicLoad {
    Missing,
    Broken,
    Ok(Pic, Option<(i64, i64)>),
}

/// Ett löpande avsnittsbyte efter ett flerspaltigt avsnitt: avsnittet, dess sista sida och vad som
/// behövs för att jämna ut spalterna där (`balance_columns`).
#[derive(Clone)]
struct SecEnd {
    sec: usize,
    page: usize,
    /// Spalternas överkant och längsta använda y på sidan.
    top: f32,
    bottom: f32,
    /// Innehållets yta (pt²) och spaltbredden i summa: lägsta möjliga spalthöjd.
    area: f32,
    width: f32,
    /// Manuell spaltbrytning (inget utjämnas, som i Word) eller byte som inte ska jämna ut.
    skip: bool,
    /// Radhöjd (pt) att stega med.
    step: f32,
}

/// En flytande bild: sida, upptagen bredd (bild + luft), sidan (0-baserad) och understa kanten (pt).
#[derive(Clone)]
struct FloatBox {
    side: Float,
    /// Bilden tar hela radens bredd (texten hoppar under den).
    full: bool,
    w: f32,
    page: usize,
    /// Övre kanten (pt): texten påverkas först från den.
    top: f32,
    bottom: f32,
}

/// Luft mellan en flytande bild och texten (12 px).
const FLOAT_GAP: f32 = 9.0;
/// Minsta textbredd bredvid en flytande bild; smalare och bilden blir ett vanligt block.
const FLOAT_MIN_ROOM: f32 = 72.0;

/// En färdigbruten rad. `hard` = raden slutar med radbrytning (ska inte marginaljusteras).
struct Line {
    pieces: Vec<Piece>,
    hard: bool,
}

/// Typsnittsklass av ett typsnittsnamn från Word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Family {
    Sans,
    Serif,
    Mono,
}

/// Grov klassning på namnet (Plex Sans/Mono och Caladea är inbäddade, se `Face::for_family`).
fn classify_family(name: &str) -> Family {
    let n = name.to_ascii_lowercase();
    const MONO: [&str; 8] = ["courier", "consolas", "mono", "menlo", "monaco", "lucida console", "typewriter", "fixed"];
    const SERIF: [&str; 14] = [
        "times", "georgia", "garamond", "cambria", "serif", "book", "palatino", "minion", "baskerville", "century",
        "didot", "constantia", "caslon", "roman",
    ];
    if MONO.iter().any(|m| n.contains(m)) {
        Family::Mono
    } else if n.contains("sans") {
        // "sans-serif" ska inte bli serif.
        Family::Sans
    } else if SERIF.iter().any(|m| n.contains(m)) {
        Family::Serif
    } else {
        Family::Sans
    }
}

/// Ett ord eller mellanslag med sin stil, klart att placera på en rad.
struct Piece {
    text: String,
    face: Face,
    size: f32,
    width: f32,
    style: Style,
    space: bool,
    newline: bool,
}

fn face_for(style: &Style, base: Face) -> Face {
    // Spannens eget typsnitt, annars styckets.
    let own = style.span.font_family.as_deref().map(Face::for_family);
    let face = if style.code { Face::Mono } else { own.unwrap_or(base) };
    if face.is_mono() {
        return if style.bold { Face::MonoMedium } else { Face::Mono };
    }
    if style.bold { face.strong() } else { face }
}

/// Fördelar överskottet till `width` på mellanrummen i raden (marginaljustering).
fn justify(pieces: &mut [Piece], width: f32) {
    let total: f32 = pieces.iter().map(|p| p.width).sum();
    let gaps = pieces.iter().filter(|p| p.space).count();
    let extra = width - total;
    if gaps == 0 || extra <= 0.0 {
        return;
    }
    let add = extra / gaps as f32;
    for p in pieces.iter_mut().filter(|p| p.space) {
        p.width += add;
    }
}

/// Radens höjdgivande storlek: den största spannen på raden.
fn line_size(l: &Line, size: f32) -> f32 {
    let m = l.pieces.iter().map(|p| p.size).fold(0.0, f32::max);
    if m > 0.0 { m } else { size }
}

/// Bildens storlek enligt `width`/`height` i `<img>`, i pt.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ImgSize {
    w: Option<f32>,
    h: Option<f32>,
}

impl<'a> Layout<'a> {
    /// Rubrikernas typsnitt (normal vikt): dokumentets eget, utom mono som får Plex Sans.
    fn head_face(&self) -> Face {
        if self.base.is_mono() { Face::Sans } else { self.base.plain() }
    }

    fn new(resolve: &'a dyn Fn(&str) -> Option<DocImage>, defaults: &DocDefaults) -> Self {
        let body = defaults.size();
        let base = defaults.font_family.as_deref().map_or(Face::Sans, Face::for_family);
        let (pw, ph) = defaults.page();
        let [mt, mr, mb, ml] = defaults.layout_margins();
        // Orimliga marginaler (större än sidan) ger ändå en användbar textyta.
        let cw = (pw - ml - mr).max(36.0);
        Layout {
            m: Measurer::new(),
            pages: Vec::new(),
            y: mt,
            headings: Vec::new(),
            images: Vec::new(),
            resolve,
            missing: Vec::new(),
            body,
            scale: body / BODY_SIZE,
            styles: defaults.styles.clone(),
            line: LINE,
            first_indent: 0.0,
            base,
            align: Align::Left,
            pw,
            ph,
            mt,
            mb,
            ml,
            cw,
            defaults: defaults.clone(),
            cols: 1,
            col: 0,
            gap: 0.0,
            text_l: ml,
            text_w: cw,
            col_w: cw,
            col_ws: Vec::new(),
            balance: Vec::new(),
            sec_ends: Vec::new(),
            col_area: 0.0,
            manual_col: false,
            no_balance: false,
            rule: false,
            rt: mt,
            page_ymax: 0.0,
            breaks: 0,
            cur_sec: 0,
            sec_row_seen: false,
            hf_reserve: Rc::default(),
            notes: Rc::default(),
            page_notes: Vec::new(),
            reserved: 0.0,
            in_notes: false,
            floats: Vec::new(),
            box_min: None,
            toc_entries: Rc::default(),
            toc_pos: Vec::new(),
            pics: Rc::default(),
            hf_pics: HashMap::new(),
        }
    }

    /// Ritar innehållsförteckningen: rubriken "Innehåll" och en rad per rubrik med prickad
    /// ledlinje, sidnummer till höger och en intern länk till rubriken.
    fn toc(&mut self) {
        let size = 20.0 * self.scale;
        self.ensure(size * LINE + self.body * LINE * 3.0);
        let (ml, cw, body) = (self.ml, self.cw, self.body);
        let title = vec![Span { text: "Innehåll".to_owned(), style: Style::default() }];
        self.text_block(&title, ml, cw, self.head_face().strong(), size, INK, false, None);
        self.y += 6.0;
        let face = self.base;
        let line_h = body * LINE;
        let entries = self.toc_entries.clone();
        if entries.is_empty() {
            let spans = vec![Span { text: "Inga rubriker i dokumentet".to_owned(), style: Style::default() }];
            self.text_block(&spans, ml, cw, face, body, MUTED, true, None);
            self.y += PARA_GAP;
            return;
        }
        for (i, (level, text)) in entries.iter().enumerate() {
            self.ensure(line_h);
            let indent = f32::from(level.saturating_sub(1)) * 14.0;
            let baseline = self.y + body * 1.08;
            let num = self.toc_pos.get(i).map(|(p, _)| (p + 1).to_string()).unwrap_or_default();
            let num_w = self.m.width(face, body, &num);
            let x = ml + indent;
            // Titeln kortas med "…" om den inte ryms bredvid sidnumret.
            let avail = (cw - indent - num_w - 16.0).max(20.0);
            let mut title = text.clone();
            if self.m.width(face, body, &title) > avail {
                while !title.is_empty() && self.m.width(face, body, &format!("{title}…")) > avail {
                    title.pop();
                }
                title.push('…');
            }
            let title_w = self.m.width(face, body, &title);
            let dot = self.m.width(face, body, ". ").max(1.0);
            let gap = cw - indent - title_w - num_w - 12.0;
            let dots = if gap > dot { ". ".repeat((gap / dot) as usize) } else { String::new() };
            let dots_w = self.m.width(face, body, &dots);
            let page = self.pages.len() - 1;
            let ops = &mut self.page().ops;
            ops.push(Op::Text { x, y: baseline, face, size: body, text: title, color: INK, italic: false });
            if !dots.is_empty() {
                ops.push(Op::Text { x: ml + cw - num_w - 6.0 - dots_w, y: baseline, face, size: body, text: dots, color: MUTED, italic: false });
            }
            if !num.is_empty() {
                ops.push(Op::Text { x: ml + cw - num_w, y: baseline, face, size: body, text: num, color: INK, italic: false });
            }
            if let Some((p, y)) = self.toc_pos.get(i).copied() {
                let top = self.y;
                self.pages[page].ops.push(Op::Goto { x, y: top, w: cw - indent, h: line_h, page: p, to_y: y });
            }
            self.y += line_h + 2.0;
        }
        self.y += PARA_GAP;
    }

    /// Sidhuvud och sidfot för sidan `f.page` (1-baserad) av `f.pages` i ett dokument utan avsnitt.
    #[cfg(test)]
    fn header_footer(&mut self, d: &DocDefaults, f: &style::Fields) -> Vec<Op> {
        let plan = crate::hf::Plan::new(d, &[]);
        let pp = plan.pages(&vec![0; f.pages]);
        self.header_footer_page(d, &plan, &pp, f, f.page.wrapping_sub(1))
    }

    /// Sidhuvud och sidfot för ark `k` (0-baserat) som ritinstruktioner. Raden ställs upp av
    /// `hf::layout_zone` på sidans egen storlek (avsnitt kan ha olika): avstånd från sidkanten,
    /// delar vänster|mitten|höger, bilder och linjen mot brödtexten. `base` har sidantalet.
    fn header_footer_page(&mut self, d: &DocDefaults, plan: &crate::hf::Plan, pp: &crate::hf::PagePlan, base: &style::Fields, k: usize) -> Vec<Op> {
        use crate::hf::{self, Placed};
        let g = self.pages.get(k).map_or(
            PageGeom { pw: self.pw, ph: self.ph, mt: self.mt, mb: self.mb, ml: self.text_l, cw: self.text_w },
            |p| p.geom,
        );
        let hf_page = plan.page(pp, k, base);
        let f = &hf_page.fields;
        // Utan eget sidfotstecken visas ändå "N av M" till höger när dokumentet har flera sidor.
        let first = d.different_first() && k == 0;
        let auto_footer = (d.footer.is_none() && d.footer_even.is_none() && plan.secs.len() == 1 && !first && f.pages > 1).then(|| "{sida} av {sidor}".to_owned());
        let items = [
            (false, hf_page.header.map(str::to_owned), hf_page.header_align, &d.header_style, d.header_dist),
            (
                true,
                hf_page.footer.map(str::to_owned).or(auto_footer.clone()),
                if auto_footer.is_some() { Align::Right } else { hf_page.footer_align },
                &d.footer_style,
                d.footer_dist,
            ),
        ];
        let mut out = Vec::new();
        for (footer, tpl, align, st, dist) in items {
            let Some(tpl) = tpl else { continue };
            let rows = hf::filled_rows(&tpl, align, f, st);
            let size = st.size.unwrap_or(self.body * 0.9);
            let color = st.color.map_or(MUTED, |c| (c[0], c[1], c[2]));
            // Bilderna först: de behöver sin egen storlek för uppställningen.
            // En bild läggs bara in en gång (inte per sida).
            let mut loaded: HashMap<String, (usize, f32, f32)> = HashMap::new();
            for src in hf::row_image_srcs(&rows) {
                if loaded.contains_key(&src) {
                    continue;
                }
                if !self.hf_pics.contains_key(&src) {
                    let entry = match self.load_pic(&src) {
                        PicLoad::Ok(img, _) => {
                            let (pw, ph) = img.size();
                            self.images.push(img);
                            Some((self.images.len() - 1, pw * 0.75, ph * 0.75))
                        }
                        _ => None,
                    };
                    self.hf_pics.insert(src.clone(), entry);
                }
                if let Some(Some(e)) = self.hf_pics.get(&src) {
                    loaded.insert(src, *e);
                }
            }
            let (plain, strong) = (self.base, self.base.strong());
            let face_of = |fmt: &hf::RunFmt| if fmt.bold { strong } else { plain };
            let m = std::cell::RefCell::new(&mut self.m);
            let measure = |t: &str, sz: f32, fmt: &hf::RunFmt| m.borrow_mut().width(face_of(fmt), sz, t);
            let natural = |src: &str| loaded.get(src).map(|(_, w, h)| (*w, *h));
            let zn = hf::layout_rows(&rows, size, 1.0, g.cw, &measure, &natural);
            let edge = dist.unwrap_or(hf::DEFAULT_DIST);
            let top = if footer { g.ph - edge - zn.height } else { edge };
            for it in &zn.items {
                match it {
                    Placed::Text { x, y, text, size, fmt } => {
                        let base_y = top + y + size * 0.95;
                        let face = face_of(fmt);
                        let color = fmt.color.map_or(color, |c| (c[0], c[1], c[2]));
                        out.push(Op::Text { x: g.ml + x, y: base_y, face, size: *size, text: text.clone(), color, italic: fmt.italic });
                        if fmt.underline {
                            let w = m.borrow_mut().width(face, *size, text);
                            let uy = base_y + size * 0.12;
                            out.push(Op::Line { x1: g.ml + x, y1: uy, x2: g.ml + x + w, y2: uy, width: (size * 0.06).max(0.4), color });
                        }
                    }
                    Placed::Image { x, y, w, h, src, anchored } => {
                        let Some((index, ..)) = loaded.get(src) else { continue };
                        let (ox, oy) = if *anchored { (0.0, 0.0) } else { (g.ml, top) };
                        out.push(Op::Image { x: ox + x, y: oy + y, w: *w, h: *h, index: *index, crop: None });
                    }
                }
            }
            if let Some((c, w)) = st.line {
                let y = if footer { top - 3.0 } else { top + zn.height + 3.0 };
                out.push(Op::Line { x1: g.ml, y1: y, x2: g.ml + g.cw, y2: y, width: w, color: (c[0], c[1], c[2]) });
            }
        }
        out
    }

    /// Flyttar ner till under alla flytande bilder (block som inte flyter runt dem) och glömmer dem.
    fn clear_floats(&mut self) {
        self.prune_floats();
        if let Some(b) = self.floats.iter().map(|f| f.bottom).reduce(f32::max) {
            self.y = self.y.max(b);
        }
        self.floats.clear();
    }

    /// Tar bort flytande bilder som är passerade (annan sida, eller texten har kommit under dem).
    fn prune_floats(&mut self) {
        let (page, y) = (self.pages.len().wrapping_sub(1), self.y);
        self.floats.retain(|f| f.page == page && y < f.bottom - 0.5);
    }

    /// Utrymmet vid aktuell rad: (förskjutning från vänster, bredd som tas av flytande bilder).
    fn float_room(&mut self) -> (f32, f32) {
        self.prune_floats();
        // Bild över hela bredden: texten hoppar till under den.
        while let Some(b) = self.floats.iter().filter(|f| f.full && f.top <= self.y + 0.5).map(|f| f.bottom).reduce(f32::max) {
            self.y = self.y.max(b);
            self.prune_floats();
        }
        let y = self.y;
        let wide = |side: Float| self.floats.iter().filter(|f| !f.full && f.side == side && f.top <= y + 0.5).map(|f| f.w).fold(0.0, f32::max);
        (wide(Float::Left), wide(Float::Left) + wide(Float::Right))
    }

    /// Läser och avkodar en bild; resultatet sparas så att samma bild inte avkodas igen i nästa pass,
    /// på nästa sida eller i sidhuvudet.
    fn load_pic(&self, src: &str) -> PicLoad {
        if let Some(p) = self.pics.borrow().get(src) {
            return p.clone();
        }
        let loaded = match (self.resolve)(src) {
            None => PicLoad::Missing,
            Some(img) => make_pic(&img).map_or(PicLoad::Broken, |p| PicLoad::Ok(p, img.extent)),
        };
        self.pics.borrow_mut().insert(src.to_owned(), loaded.clone());
        loaded
    }

    fn page(&mut self) -> &mut PageOps {
        self.pages.last_mut().expect("minst en sida")
    }

    /// Nästa spalt i avsnittet, annars en ny sida.
    fn new_page(&mut self) {
        if self.cols > 1 && self.col + 1 < self.cols && !self.pages.is_empty() {
            self.page_ymax = self.page_ymax.max(self.y);
            self.col_area += (self.y - self.rt) * self.col_width(self.col);
            self.col += 1;
            self.ml = self.text_l + self.col_left(self.col);
            self.cw = self.col_width(self.col);
            self.y = self.rt;
            self.floats.clear();
            self.breaks += 1;
        } else {
            self.real_page();
        }
    }

    /// Nedre gräns (y) för text i spalten: sidans botten minus fotnoterna, och vid utjämning av
    /// avsnittets sista sida spalternas överkant plus vald höjd.
    fn room_end(&self) -> f32 {
        let end = self.ph - self.mb - self.reserved;
        let page = self.pages.len().wrapping_sub(1);
        match self.balance.iter().find(|b| b.0 == self.cur_sec && b.1 == page) {
            Some(b) if self.cols > 1 => end.min(self.rt + b.2),
            _ => end,
        }
    }

    fn page_geom(&self) -> PageGeom {
        PageGeom { pw: self.pw, ph: self.ph, mt: self.mt, mb: self.mb, ml: self.text_l, cw: self.text_w }
    }

    /// Spaltens bredd.
    fn col_width(&self, c: usize) -> f32 {
        self.col_ws.get(c).copied().unwrap_or(self.col_w)
    }

    /// Spaltens vänsterkant från textytans vänsterkant.
    fn col_left(&self, c: usize) -> f32 {
        (0..c).map(|k| self.col_width(k) + self.gap).sum()
    }

    /// Skiljelinjerna mellan spalterna för delen av sidan som just lämnas (från spalternas överkant
    /// till längsta använda höjd), ritade med sidans innehåll.
    fn flush_rules(&mut self) {
        let end = self.y.max(self.page_ymax);
        if !self.rule || self.cols < 2 || self.pages.is_empty() || end <= self.rt + 1.0 {
            return;
        }
        let (rt, text_l) = (self.rt, self.text_l);
        let xs: Vec<f32> = (0..self.cols - 1).map(|c| text_l + self.col_left(c) + self.col_width(c) + self.gap / 2.0).collect();
        for x in xs {
            self.page().ops.push(Op::Line { x1: x, y1: rt, x2: x, y2: end, width: 0.5, color: MUTED });
        }
    }

    fn real_page(&mut self) {
        self.flush_rules();
        self.start_page();
    }

    fn start_page(&mut self) {
        self.flush_notes();
        let geom = self.page_geom();
        let sec = self.cur_sec;
        self.pages.push(PageOps { ops: Vec::new(), geom, sec });
        self.y = self.mt;
        self.rt = self.mt;
        self.page_ymax = 0.0;
        self.col_area = 0.0;
        self.manual_col = false;
        self.col = 0;
        self.ml = self.text_l;
        self.cw = self.col_width(0);
        self.breaks += 1;
    }

    /// Tar avsnittets sidstorlek, marginaler och spalter i bruk (sidan eller spalten byts av anroparen).
    fn set_section(&mut self, sec: &Section) {
        let [mt, mr, mb, ml] = sec.margin;
        let res = self.hf_reserve.get(self.cur_sec).copied().unwrap_or([0.0; 2]);
        let (mt, mb) = (mt.max(res[0]), mb.max(res[1]));
        self.pw = sec.page.0;
        self.ph = sec.page.1;
        self.mt = mt;
        self.mb = mb;
        self.text_l = ml;
        self.text_w = (sec.page.0 - ml - mr).max(36.0);
        self.cols = sec.columns.max(1);
        let n = self.cols as f32;
        self.gap = if self.cols > 1 { sec.gap.min(((self.text_w - 18.0 * n) / (n - 1.0)).max(0.0)) } else { 0.0 };
        self.col_w = ((self.text_w - self.gap * (n - 1.0)) / n).max(18.0);
        // Olika breda spalter skalas så att de med mellanrummen fyller textytan.
        let sum: f32 = sec.col_widths.iter().sum();
        self.col_ws = if self.cols > 1 && sec.col_widths.len() == self.cols && sum > 0.0 {
            sec.col_widths.iter().map(|w| (w / sum * (self.text_w - self.gap * (n - 1.0))).max(18.0)).collect()
        } else {
            Vec::new()
        };
        self.rule = sec.rule && self.cols > 1;
        self.no_balance = sec.no_balance;
        self.col = 0;
        self.ml = ml;
        self.cw = self.col_width(0);
    }

    /// Avsnittsbrytning: ny sida med avsnittets mått, eller (löpande, samma sidstorlek) vidare på samma sida.
    fn section_break(&mut self, props: &SectionProps) {
        let sec = Section::resolve(props, &self.defaults);
        let page_empty = self.pages.last().is_some_and(|p| p.ops.is_empty()) && self.col == 0 && (self.y - self.rt).abs() < 0.5;
        let same_sheet = [(self.pw, sec.page.0), (self.ph, sec.page.1), (self.mt, sec.margin[0]), (self.mb, sec.margin[2]), (self.text_l, sec.margin[3])]
            .iter()
            .all(|(a, b)| (a - b).abs() < 0.5);
        if !page_empty {
            if self.cols > 1 && sec.continuous && same_sheet {
                let bottom = self.y.max(self.page_ymax);
                let area = self.col_area + (self.y - self.rt) * self.col_width(self.col);
                let width: f32 = (0..self.cols).map(|c| self.col_width(c)).sum();
                let (page, sec_i) = (self.pages.len() - 1, self.cur_sec);
                self.sec_ends.push(SecEnd { sec: sec_i, page, top: self.rt, bottom, area, width, skip: self.manual_col || self.no_balance, step: self.body * LINE });
            }
            self.flush_rules();
        }
        self.manual_col = false;
        self.col_area = 0.0;
        if !(self.pages.len() <= 1 && page_empty && !self.sec_row_seen) {
            self.cur_sec += 1;
        }
        self.sec_row_seen = true;
        if page_empty {
            // Början av dokumentet (eller två rader i rad): sidan görs om i stället för att lämnas tom.
            self.set_section(&sec);
            let geom = self.page_geom();
            let cur = self.cur_sec;
            if let Some(p) = self.pages.last_mut() {
                p.geom = geom;
                p.sec = cur;
            }
            self.y = self.mt;
            self.rt = self.mt;
            self.page_ymax = 0.0;
        } else if sec.continuous && same_sheet && !self.pages.is_empty() {
            let y = self.y.max(self.page_ymax);
            self.set_section(&sec);
            self.y = y;
            self.rt = y;
            self.page_ymax = 0.0;
            self.col_area = 0.0;
            self.floats.clear();
            self.breaks += 1;
        } else {
            self.flush_notes();
            self.set_section(&sec);
            self.start_page();
        }
    }

    fn ensure(&mut self, h: f32) {
        if self.y + h > self.room_end() && self.y > self.rt + 0.5 {
            self.new_page();
        }
    }

    /// Fotnotens rader: litet nummer, mellanslag och texten.
    fn note_spans(&self, n: usize) -> Vec<Span> {
        let mut v = vec![
            Span { text: self.defaults.notes.number(false, n), style: Style { fnref: Some(n), ..Style::default() } },
            Span { text: " ".into(), style: Style::default() },
        ];
        v.extend(self.notes.get(n - 1).cloned().unwrap_or_default());
        v
    }

    /// Hur mycket plats fotnoterna `refs` tar längst ned på sidan (med avskiljaren om det är de första).
    fn note_cost(&mut self, refs: &[usize]) -> f32 {
        let size = self.body * 0.85;
        let mut h = if self.page_notes.is_empty() && !refs.is_empty() { 9.0 } else { 0.0 };
        for &n in refs {
            let spans = self.note_spans(n);
            let width = self.cw;
            let lines = self.break_lines(&spans, self.base, size, width);
            h += lines.iter().map(|l| line_size(l, size) * LINE).sum::<f32>() + 2.0;
        }
        h
    }

    /// Ritar sidans fotnoter längst ned: en kort linje och sedan noterna i liten stil.
    fn flush_notes(&mut self) {
        if self.page_notes.is_empty() || self.pages.is_empty() {
            return;
        }
        let notes = std::mem::take(&mut self.page_notes);
        let top = self.ph - self.mb - self.reserved;
        self.reserved = 0.0;
        let (ml, cw) = (self.text_l, self.text_w);
        let (saved_y, saved_align) = (self.y, self.align);
        self.page().ops.push(Op::Line { x1: ml, y1: top + 3.0, x2: ml + cw / 3.0, y2: top + 3.0, width: 0.6, color: MUTED });
        self.y = top + 9.0;
        self.align = Align::Left;
        self.in_notes = true;
        let size = self.body * 0.85;
        for n in notes {
            let spans = self.note_spans(n);
            self.text_block(&spans, ml, cw, self.base, size, INK, false, None);
            self.y += 2.0;
        }
        self.in_notes = false;
        self.y = saved_y;
        self.align = saved_align;
    }

    /// Delar upp textstyckena i ord som kan radbrytas.
    fn pieces(&mut self, spans: &[Span], base: Face, size: f32) -> Vec<Piece> {
        let mut out = Vec::new();
        for s in spans {
            let face = face_for(&s.style, base);
            let sz = s.style.span.font_size.unwrap_or(size);
            let sz = if s.style.code { sz * 0.9 } else { sz };
            // Notnummer: 0,65 av texten (0,8 i notlistan), minst 7,5 pt.
            let sz = if s.style.fnref.is_some() || s.style.endref { (sz * if self.in_notes { 0.8 } else { 0.65 }).max(7.5) } else { sz };
            if let Some((src, disp)) = &s.style.math {
                if let Some(f) = crate::math::formula(src, *disp) {
                    // Raden ska rymma formeln: storleken höjs så att över- och underdelen får plats.
                    let eff = sz.max(f.layout.asc * sz / 1.08).max(f.layout.desc * sz / (self.line - 1.08).max(0.1));
                    out.push(Piece { text: String::new(), face, size: eff, width: f.layout.w * sz, style: s.style.clone(), space: false, newline: false });
                    continue;
                }
            }
            for (li, line) in s.text.split('\n').enumerate() {
                if li > 0 {
                    out.push(Piece { text: String::new(), face, size: sz, width: 0.0, style: s.style.clone(), space: false, newline: true });
                }
                let mut word = String::new();
                for c in line.chars() {
                    if c == ' ' || c == '\t' {
                        if !word.is_empty() {
                            let w = self.m.width(face, sz, &word);
                            out.push(Piece { text: std::mem::take(&mut word), face, size: sz, width: w, style: s.style.clone(), space: false, newline: false });
                        }
                        let sp = if c == '\t' { "    " } else { " " };
                        let w = self.m.width(face, sz, sp);
                        out.push(Piece { text: sp.to_owned(), face, size: sz, width: w, style: s.style.clone(), space: true, newline: false });
                    } else {
                        word.push(c);
                    }
                }
                if !word.is_empty() {
                    let w = self.m.width(face, sz, &word);
                    out.push(Piece { text: word, face, size: sz, width: w, style: s.style.clone(), space: false, newline: false });
                }
            }
        }
        out
    }

    /// Bryter ord för långa för raden i tecken.
    fn split_long(&mut self, p: Piece, max: f32) -> Vec<Piece> {
        if p.width <= max || p.space {
            return vec![p];
        }
        let mut out = Vec::new();
        let mut cur = String::new();
        for c in p.text.chars() {
            let mut next = cur.clone();
            next.push(c);
            if !cur.is_empty() && self.m.width(p.face, p.size, &next) > max {
                let w = self.m.width(p.face, p.size, &cur);
                out.push(Piece { text: std::mem::take(&mut cur), width: w, style: p.style.clone(), ..p_clone(&p) });
                cur.push(c);
            } else {
                cur = next;
            }
        }
        if !cur.is_empty() {
            let w = self.m.width(p.face, p.size, &cur);
            out.push(Piece { text: cur, width: w, style: p.style.clone(), ..p_clone(&p) });
        }
        out
    }

    /// Bryter stycket i rader som ryms i `width`. Efterföljande mellanslag tas bort.
    fn break_lines(&mut self, spans: &[Span], base: Face, size: f32, width: f32) -> Vec<Line> {
        let pieces = self.pieces(spans, base, size);
        let mut lines = vec![Line { pieces: Vec::new(), hard: false }];
        let mut line_w = 0.0;
        for p in pieces {
            if p.newline {
                lines.last_mut().unwrap().hard = true;
                lines.push(Line { pieces: Vec::new(), hard: false });
                line_w = 0.0;
                continue;
            }
            for p in self.split_long(p, width) {
                if !p.space && line_w + p.width > width && lines.last().is_some_and(|l| !l.pieces.is_empty()) {
                    lines.push(Line { pieces: Vec::new(), hard: false });
                    line_w = 0.0;
                }
                if p.space && lines.last().is_some_and(|l| l.pieces.is_empty()) {
                    continue; // inget mellanslag först på en rad
                }
                line_w += p.width;
                lines.last_mut().unwrap().pieces.push(p);
            }
        }
        for l in &mut lines {
            while l.pieces.last().is_some_and(|p| p.space) {
                l.pieces.pop();
            }
        }
        lines
    }

    /// Nästa rad ur `pieces` från `from`, bruten i `width`: raden och index för första ordet efter den.
    /// Samma girighet som `break_lines`, men en rad i taget så att bredden kan variera med flytande bilder.
    fn take_line(pieces: &[Piece], from: usize, width: f32) -> (Line, usize) {
        let mut line = Line { pieces: Vec::new(), hard: false };
        let (mut w, mut i) = (0.0, from);
        while i < pieces.len() {
            let p = &pieces[i];
            if p.newline {
                line.hard = true;
                i += 1;
                break;
            }
            if !p.space && w + p.width > width && !line.pieces.is_empty() {
                break;
            }
            i += 1;
            if p.space && line.pieces.is_empty() {
                continue; // inget mellanslag först på en rad
            }
            w += p.width;
            line.pieces.push(Piece { text: p.text.clone(), width: p.width, ..p_clone(p) });
        }
        while line.pieces.last().is_some_and(|p| p.space) {
            line.pieces.pop();
        }
        (line, i)
    }

    /// Lägger ut text i en ruta. Returnerar höjden som användes.
    #[allow(clippy::too_many_arguments)]
    fn text_block(
        &mut self,
        spans: &[Span],
        x: f32,
        width: f32,
        base: Face,
        size: f32,
        color: (u8, u8, u8),
        italic_all: bool,
        first_line_extra: Option<(String, Face)>,
    ) {
        let pieces = self.pieces(spans, base, size);
        let pieces: Vec<Piece> = pieces.into_iter().flat_map(|p| self.split_long(p, width)).collect();
        let (mut next, mut i) = (0usize, 0usize);
        let (ml_entry, cw_entry) = (self.ml, self.cw);
        let width0 = width;
        loop {
            // Texten följer med till nästa spalt och bryts i dess bredd (spalterna kan vara olika breda).
            let mut width = width0 + (self.cw - cw_entry);
            // Flytande bilder gör raden smalare (och förskjuter den vid vänsterbilder).
            let (mut fx, mut fw) = self.float_room();
            let ind0 = if i == 0 { self.first_indent } else { 0.0 };
            let (mut line, mut after) = Self::take_line(&pieces, next, (width - fw.min(width - 24.0) - ind0).max(1.0));
            let page_before = self.breaks;
            // Fotnoterna på raden ska ha plats längst ned på samma sida som raden.
            let refs_of = |this: &Self, line: &Line| -> Vec<usize> {
                if this.in_notes {
                    Vec::new()
                } else {
                    line.pieces.iter().filter_map(|p| p.style.fnref).filter(|n| !this.page_notes.contains(n)).collect()
                }
            };
            let mut refs = refs_of(self, &line);
            let line_h0 = line_size(&line, size) * self.line;
            let need0 = self.note_cost(&refs);
            if need0 > 0.0 && self.y + line_h0 + need0 > self.room_end() && self.y > self.rt + 0.5 {
                self.new_page();
            }
            self.ensure(line_h0);
            // Texten följer med till nästa spalt.
            let x = x + (self.ml - ml_entry);
            if self.breaks != page_before {
                width = width0 + (self.cw - cw_entry);
                (fx, fw) = self.float_room();
                (line, after) = Self::take_line(&pieces, next, (width - fw.min(width - 24.0) - ind0).max(1.0));
                refs = refs_of(self, &line);
            }
            let need = self.note_cost(&refs);
            self.reserved += need;
            self.page_notes.extend(refs);
            let (x, width) = (x + fx + ind0, width - fw.min(width - 24.0) - ind0);
            let last = after >= pieces.len() && !line.hard;
            let msz = line_size(&line, size);
            let line_h = msz * self.line;
            let baseline = self.y + msz * 1.08;
            if i == 0 {
                if let Some((marker, mface)) = &first_line_extra {
                    let mw = self.m.width(*mface, size, marker);
                    self.page().ops.push(Op::Text {
                        x: x - mw - 6.0,
                        y: baseline,
                        face: *mface,
                        size,
                        text: marker.clone(),
                        color: MUTED,
                        italic: false,
                    });
                }
            }
            let lw: f32 = line.pieces.iter().map(|p| p.width).sum();
            let mut dx = 0.0;
            let mut stretched = false;
            match self.align {
                Align::Center => dx = ((width - lw) / 2.0).max(0.0),
                Align::Right => dx = (width - lw).max(0.0),
                Align::Justify if !last && !line.hard => {
                    justify(&mut line.pieces, width);
                    stretched = true;
                }
                _ => {}
            }
            // Slå ihop ord med samma stil till en körning.
            let mut cx = x + dx;
            let mut run: Option<(f32, String, Face, f32, Style, f32)> = None;
            let mut runs = Vec::new();
            for p in line.pieces {
                match run.as_mut() {
                    Some((_, text, f, sz, st, w)) if !stretched && st.math.is_none() && *f == p.face && *sz == p.size && *st == p.style => {
                        text.push_str(&p.text);
                        *w += p.width;
                    }
                    _ => {
                        if let Some(r) = run.take() {
                            runs.push(r);
                        }
                        run = Some((cx, p.text.clone(), p.face, p.size, p.style.clone(), p.width));
                    }
                }
                cx += p.width;
            }
            if let Some(r) = run.take() {
                runs.push(r);
            }
            for (rx, text, face, sz, st, w) in runs {
                if let Some((src, disp)) = &st.math {
                    if let Some(f) = crate::math::formula(src, *disp) {
                        let em = if f.layout.w > 0.0 { w / f.layout.w } else { size };
                        let c = st.span.color.map_or(color, |sc| (sc[0], sc[1], sc[2]));
                        self.page().ops.push(Op::Math { x: rx, y: baseline, src: src.clone(), display: *disp, em, color: c });
                    }
                    continue;
                }
                if st.code {
                    self.page().ops.push(Op::Rect { x: rx - 1.5, y: baseline - sz * 0.95, w: w + 3.0, h: sz * 1.3, color: CODE_BG });
                }
                if let Some(bg) = st.span.background {
                    self.page().ops.push(Op::Rect { x: rx, y: baseline - sz * 0.9, w, h: sz * 1.2, color: (bg[0], bg[1], bg[2]) });
                }
                let c = match (st.link.is_some(), st.span.color) {
                    (true, _) => LINK,
                    (false, Some(sc)) => (sc[0], sc[1], sc[2]),
                    (false, None) => color,
                };
                // Fotnotsnummer ritas upphöjda.
                let ty = if st.fnref.is_some() || st.endref { baseline - size * 0.38 } else { baseline };
                self.page().ops.push(Op::Text { x: rx, y: ty, face, size: sz, text, color: c, italic: st.italic || italic_all });
                if st.link.is_some() {
                    self.page().ops.push(Op::Line { x1: rx, y1: baseline + 1.6, x2: rx + w, y2: baseline + 1.6, width: 0.6, color: LINK });
                }
                if st.underline && st.link.is_none() {
                    let uy = baseline + 1.6;
                    self.page().ops.push(Op::Line { x1: rx, y1: uy, x2: rx + w, y2: uy, width: 0.6, color: c });
                }
                if st.strike {
                    let sy = baseline - sz * 0.3;
                    self.page().ops.push(Op::Line { x1: rx, y1: sy, x2: rx + w, y2: sy, width: 0.7, color: c });
                }
                if let Some(url) = &st.link {
                    let y0 = self.y;
                    self.page().ops.push(Op::Link { x: rx, y: y0, w, h: line_h, url: url.clone() });
                }
            }
            self.y += line_h;
            next = after;
            i += 1;
            if last {
                break;
            }
        }
    }

    fn image(&mut self, src: &str, alt: &str, indent: f32, size: ImgSize, align: Align, float: Option<Float>, place: ImgPlace) {
        // Bilder som inte flyter, och texten om bilden saknas, kommer under flytande bilder.
        let float = float.filter(|_| indent == 0.0);
        let anchor = place.anchor.filter(|_| indent == 0.0);
        if float.is_none() && anchor.is_none() {
            self.clear_floats();
        }
        let (image, extent) = match self.load_pic(src) {
            PicLoad::Ok(p, e) => (p, e),
            PicLoad::Broken => {
                self.missing.push(src.to_owned());
                return;
            }
            PicLoad::Missing => {
                self.missing.push(src.to_owned());
                let label = if alt.is_empty() { format!("[bild: {src}]") } else { format!("[bild: {alt}]") };
                let spans = vec![Span { text: label, style: Style::default() }];
                self.text_block(&spans, self.ml + indent, self.cw - indent, self.base, self.body, MUTED, false, None);
                return;
            }
        };
        let (pw, ph) = image.size();
        // Storlek: originalets storlek i Word om den finns, annars 96 dpi, högst sidbredden.
        let (mut w, mut h) = match extent {
            Some((cx, cy)) => (cx as f32 / 12_700.0, cy as f32 / 12_700.0),
            None => (pw * 0.75, ph * 0.75),
        };
        // Angiven storlek (`width`/`height` i CSS-px, redan omräknat till pt); proportionerna bevaras
        // när bara en sida anges.
        let mut aspect = if pw > 0.0 { ph / pw } else { 1.0 };
        // Beskuren bild: den synliga delen avgör proportionerna.
        if let Some(c) = place.crop {
            aspect *= (1.0 - (c[1] + c[3]) / 100.0) / (1.0 - (c[0] + c[2]) / 100.0);
            if size.w.is_none() && size.h.is_none() && extent.is_none() {
                w *= 1.0 - (c[0] + c[2]) / 100.0;
                h = w * aspect;
            }
        }
        match (size.w, size.h) {
            (Some(sw), Some(sh)) => (w, h) = (sw, sh),
            (Some(sw), None) => (w, h) = (sw, sw * aspect),
            (None, Some(sh)) => (w, h) = (sh / aspect.max(0.0001), sh),
            _ => {}
        }
        let max_w = if anchor.is_some() { self.pw } else { self.cw - indent };
        if w > max_w {
            h *= max_w / w;
            w = max_w;
        }
        let max_h = if anchor.is_some() { self.ph } else { self.ph - self.mt - self.mb };
        if h > max_h {
            w *= max_h / h;
            h = max_h;
        }
        if let Some(an) = anchor {
            let index = self.images.len();
            self.images.push(image);
            self.anchored_image(an, w, h, index, place.crop);
            return;
        }
        // Flytande bild: vid vänster/höger textkant, text runt; ryms den inte flyttas den till nästa sida.
        let mut float = float;
        if float.is_some() && (w + FLOAT_GAP > max_w - FLOAT_MIN_ROOM || !self.floats.is_empty() && self.floats.iter().any(|f| Some(f.side) == float)) {
            self.clear_floats();
            float = None;
        }
        self.ensure(h);
        let index = self.images.len();
        self.images.push(image);
        if let Some(side) = float {
            let (x, y) = (if side == Float::Left { self.ml } else { self.ml + self.cw - w }, self.y);
            self.page().ops.push(Op::Image { x, y, w, h, index, crop: place.crop });
            let page = self.pages.len() - 1;
            self.floats.push(FloatBox { side, full: false, w: w + FLOAT_GAP, page, top: y, bottom: y + h });
            return;
        }
        let free = (self.cw - indent - w).max(0.0);
        let dx = match align {
            Align::Center => free / 2.0,
            Align::Right => free,
            _ => 0.0,
        };
        let (x, y) = (self.ml + indent + dx, self.y);
        self.page().ops.push(Op::Image { x, y, w, h, index, crop: place.crop });
        self.y += h + PARA_GAP;
    }

    /// Förankrad bild: läget räknas mot sidan, marginalen, spalten eller stycket (som i
    /// dokumentläget); text flyter runt den på bredare sidan, hoppar förbi den (över/under), eller
    /// påverkas inte alls (bakom/framför). Sidan är den där bilden förankras.
    fn anchored_image(&mut self, an: Anchor, w: f32, h: f32, index: usize, crop: Option<[f32; 4]>) {
        // CSS-px till pt.
        let a = an.scaled(0.75);
        let ctx = PageCtx {
            page_w: self.pw,
            page_h: self.ph,
            margin: [self.mt, (self.pw - self.text_l - self.text_w).max(0.0), self.mb, self.text_l],
            col_left: self.ml,
            col_w: self.cw,
        };
        let affects = a.wrap.affects_text();
        let para = matches!(a.y_at(h, &ctx), VAt::Para(_));
        // En bild vid stycket som inte ryms på sidan flyttas till nästa sida med texten.
        if affects && para {
            self.ensure(h);
        }
        let x = self.ml + a.x_in_column(w, &ctx);
        let y = match a.y_at(h, &ctx) {
            VAt::Para(o) => self.y + o,
            VAt::Page(py) => py,
        };
        let op = Op::Image { x, y, w, h, index, crop };
        if a.wrap == Wrap::Behind {
            self.page().ops.insert(0, op);
        } else {
            self.page().ops.push(op);
        }
        if !affects {
            return;
        }
        let page = self.pages.len() - 1;
        let (top, bottom) = (y - a.dist[0], y + h + a.dist[2]);
        let (side, rw, full) = match a.wrap {
            Wrap::TopBottom => (Float::Left, 0.0, true),
            _ => match crate::word::anchor::reserve(&a, x - self.ml, w, self.cw) {
                Reserve::Left(r) => (Float::Left, r, false),
                Reserve::Right(r) => (Float::Right, r, false),
                Reserve::Full => (Float::Left, 0.0, true),
            },
        };
        // Utan plats vid sidan hoppar texten förbi bilden.
        let full = full || self.cw - rw < FLOAT_MIN_ROOM;
        // En rad som börjar strax ovanför en bild över hela bredden skulle överlappa den.
        let top = if full { top - self.body * 1.2 } else { top };
        self.floats.push(FloatBox { side, full, w: rw, page, top, bottom });
    }

    /// Förankrad textruta: stycknen ritas som vanliga ramstycken, flyttas till rutans läge (som en förankrad bild)
    /// och texten efter dem flyter runt rutan. `inner` är blocken mellan rutans öppnings- och slutrad.
    fn tbox(&mut self, head: &crate::docx_tbox::Head, inner: &[Block]) {
        let Some(an) = head.anchor else { return self.run(inner) };
        let a = an.scaled(0.75);
        let (w, h) = (head.w * 0.75, head.h * 0.75);
        let ctx = PageCtx {
            page_w: self.pw,
            page_h: self.ph,
            margin: [self.mt, (self.pw - self.text_l - self.text_w).max(0.0), self.mb, self.text_l],
            col_left: self.ml,
            col_w: self.cw,
        };
        if a.wrap.affects_text() && matches!(a.y_at(h, &ctx), VAt::Para(_)) {
            self.ensure(h.min(self.ph * 0.5).max(self.body * 2.0));
        }
        let (y0, page, at) = (self.y, self.pages.len() - 1, self.page().ops.len());
        self.box_min = (h > 1.0).then_some(y0 + h);
        self.run(inner);
        self.box_min = None;
        let same_page = self.pages.len() - 1 == page;
        let used = if same_page { (self.y - y0).max(0.0) } else { h };
        let full_h = h.max(used);
        let top = match a.y_at(full_h, &ctx) {
            VAt::Para(o) => y0 + o.max(0.0),
            VAt::Page(py) => py,
        };
        if same_page {
            let behind = a.wrap == Wrap::Behind;
            let ops = &mut self.pages[page].ops;
            let mut moved: Vec<Op> = ops.drain(at.min(ops.len())..).collect();
            moved.iter_mut().for_each(|o| o.shift_y(top - y0));
            if behind {
                ops.splice(0..0, moved);
            } else {
                ops.extend(moved);
            }
        }
        self.y = y0;
        if !a.wrap.affects_text() {
            return;
        }
        let x = self.ml + a.x_in_column(w, &ctx);
        let (side, rw, full) = match crate::word::anchor::reserve(&a, x - self.ml, w, self.cw) {
            Reserve::Left(r) => (Float::Left, r, false),
            Reserve::Right(r) => (Float::Right, r, false),
            Reserve::Full => (Float::Left, 0.0, true),
        };
        let full = full || self.cw - rw < FLOAT_MIN_ROOM;
        // Raden som delvis ligger över rutans överkant flyter också runt den.
        let ftop = top - a.dist[0] - self.body * 1.1;
        self.floats.push(FloatBox { side, full, w: rw, page, top: ftop, bottom: top + full_h + a.dist[2] });
    }

    fn table(&mut self, rows: &[Vec<Vec<Span>>], spans: &[Vec<usize>], weights: Option<&[usize]>, fmt: Option<&crate::word::tablefmt::TableFmt>) {
        use crate::word::tablefmt::{BOTTOM, LEFT, RIGHT, TAlign, TOP, VAlign, Width};
        self.clear_floats();
        let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let size = self.body * 0.95;
        let pad = fmt.map_or(5.0, |f| (f.pad[0] + f.pad[1]) / 2.0);
        let pad_v = fmt.map_or(pad, |f| (f.pad[2] + f.pad[3]) / 2.0);
        // Tabellens egen bredd (`w:tblW`).
        let target: Option<f32> = fmt.and_then(|f| match f.width {
            Width::Pct(p) => Some(self.cw * f32::from(p) / 100.0),
            Width::Pt(p) => Some(f32::from(p)),
            Width::Auto => None,
        });
        let span_of = |ri: usize, ci: usize| spans.get(ri).and_then(|r| r.get(ci)).copied().unwrap_or(1);
        // Cellens text rad för rad (`<br>` är ett radslut).
        let widest = |this: &mut Self, cell: &[Span]| -> f32 {
            let text: String = cell.iter().map(|s| s.text.as_str()).collect();
            text.split('\n').map(|l| this.m.width(this.base, size, l)).fold(0.0, f32::max)
        };
        // Kolumnbredd: antalet streck i avgränsarraden, annars innehållets naturliga bredd inom sidbredden.
        let widths: Vec<f32> = match weights.filter(|w| w.len() == cols) {
            Some(w) => {
                let sum: usize = w.iter().sum();
                let total = target.unwrap_or(self.cw).min(self.cw);
                w.iter().map(|&n| total * n as f32 / sum as f32).collect()
            }
            None if target.is_some() => vec![target.unwrap_or(self.cw).min(self.cw) / cols as f32; cols],
            None => {
                let mut natural = vec![24.0f32; cols];
                let mut wide: Vec<(usize, usize, f32)> = Vec::new();
                for (ri, r) in rows.iter().enumerate() {
                    for (i, cell) in r.iter().enumerate() {
                        let w = widest(self, cell) + 2.0 * pad + 2.0;
                        match span_of(ri, i) {
                            0 => {}
                            1 => natural[i] = natural[i].max(w.min(self.cw)),
                            n => wide.push((i, n, w)),
                        }
                    }
                }
                for (i, n, w) in wide {
                    let have: f32 = natural.iter().skip(i).take(n).sum();
                    if have < w {
                        let add = (w.min(self.cw) - have) / n as f32;
                        for c in natural.iter_mut().skip(i).take(n) {
                            *c += add.max(0.0);
                        }
                    }
                }
                let total: f32 = natural.iter().sum();
                if total <= self.cw {
                    natural
                } else {
                    // Proportionellt, men ingen kolumn smalare än 30 pt; resten delar på det som blir över.
                    let mut w: Vec<f32> = natural.iter().map(|w| w / total * self.cw).collect();
                    let small: f32 = w.iter().filter(|x| **x < 30.0).count() as f32 * 30.0;
                    let big: f32 = w.iter().filter(|x| **x >= 30.0).sum();
                    let scale = if big > 0.0 { ((self.cw - small) / big).min(1.0) } else { 1.0 };
                    for x in &mut w {
                        *x = if *x < 30.0 { 30.0 } else { *x * scale };
                    }
                    w
                }
            }
        };
        let line_h = size * LINE;
        let total_w: f32 = widths.iter().sum();
        // Cellernas x och bredd (täckta celler utelämnas, ägaren får hela bredden).
        let cells_of = |ri: usize, len: usize| -> Vec<(usize, f32, f32)> {
            let mut out = Vec::new();
            let mut x = 0.0;
            for ci in 0..len.min(cols) {
                let n = span_of(ri, ci);
                let w: f32 = widths.iter().skip(ci).take(n.max(1)).sum();
                if n > 0 {
                    out.push((ci, x, w));
                }
                x += widths[ci];
            }
            out
        };
        for (ri, r) in rows.iter().enumerate() {
            let face = if ri == 0 && fmt.is_none_or(|f| f.header_bold) { self.head_face().strong() } else { self.base };
            let layout = cells_of(ri, r.len());
            // Mät radens höjd genom att lägga ut cellerna på en provsida.
            let mut heights = Vec::new();
            for &(ci, _, w) in &layout {
                heights.push(self.cell_height(&r[ci], w - 2.0 * pad, size, face));
            }
            let min_h = fmt.and_then(|f| f.rows.get(&ri)).map_or(0.0, |r| f32::from(r.h));
            let text_h = heights.iter().copied().fold(line_h, f32::max);
            let row_h = (text_h + 2.0 * pad_v).max(min_h);
            self.ensure(row_h);
            let top = self.y;
            let x_off = match fmt.map(|f| f.align) {
                Some(TAlign::Center) => (self.cw - total_w).max(0.0) / 2.0,
                Some(TAlign::Right) => (self.cw - total_w).max(0.0),
                _ => fmt.map_or(0.0, |f| f.indent),
            };
            if ri == 0 && fmt.is_none() {
                let ml = self.ml;
                self.page().ops.push(Op::Rect { x: ml, y: top, w: total_w, h: row_h, color: CODE_BG });
            }
            if let Some(f) = fmt {
                for &(ci, x, w) in &layout {
                    if let Some(c) = f.fill(ri, ci) {
                        let ml = self.ml + x_off;
                        self.page().ops.push(Op::Rect { x: ml + x, y: top, w, h: row_h, color: (c[0], c[1], c[2]) });
                    }
                }
            }
            for (k, &(ci, x, w)) in layout.iter().enumerate() {
                let dy = match fmt.map_or(VAlign::Top, |f| f.valign(ri, ci)) {
                    VAlign::Top => 0.0,
                    VAlign::Center => (row_h - 2.0 * pad_v - heights[k]).max(0.0) / 2.0,
                    VAlign::Bottom => (row_h - 2.0 * pad_v - heights[k]).max(0.0),
                };
                self.y = top + pad_v + dy;
                let ml = self.ml + x_off;
                self.text_block(&r[ci], ml + x + pad, w - 2.0 * pad, face, size, INK, false, None);
            }
            if let Some(f) = fmt {
                // Ramarna efter Words regler; en cells egna kanter går före tabellens.
                let nrows = rows.len();
                let ml = self.ml + x_off;
                let last = layout.len().saturating_sub(1);
                let color = |b: crate::word::tablefmt::Border| b.color.map_or(INK, |c| (c[0], c[1], c[2]));
                let mut lines: Vec<(f32, f32, f32, f32, crate::word::tablefmt::Border)> = Vec::new();
                for (k, &(ci, x, w)) in layout.iter().enumerate() {
                    let own = |side: usize| f.own_border(ri, ci, side);
                    lines.push((x, 0.0, x + w, 0.0, f.edge(TOP, None, own(TOP), ri > 0)));
                    lines.push((x, 0.0, x, row_h, f.edge(LEFT, None, own(LEFT), k > 0)));
                    if k == last {
                        lines.push((x + w, 0.0, x + w, row_h, f.edge(RIGHT, own(RIGHT), None, false)));
                    }
                    if ri + 1 == nrows {
                        lines.push((x, row_h, x + w, row_h, f.edge(BOTTOM, own(BOTTOM), None, false)));
                    }
                }
                let page = self.page();
                for (x1, y1, x2, y2, b) in lines {
                    if b.visible() {
                        page.ops.push(Op::Line { x1: ml + x1, y1: top + y1, x2: ml + x2, y2: top + y2, width: b.pt().max(0.4), color: color(b) });
                    }
                }
                self.y = top + row_h;
                continue;
            }
            // Rutnät: kanter runt varje cell (en sammanfogad cell har inga streck inuti).
            let ml = self.ml;
            let right = layout.last().map_or(total_w, |&(_, x, w)| x + w);
            let page = self.page();
            page.ops.push(Op::Line { x1: ml, y1: top, x2: ml + right, y2: top, width: 0.6, color: RULE });
            page.ops.push(Op::Line { x1: ml, y1: top + row_h, x2: ml + right, y2: top + row_h, width: 0.6, color: RULE });
            for &(_, x, _) in &layout {
                page.ops.push(Op::Line { x1: ml + x, y1: top, x2: ml + x, y2: top + row_h, width: 0.6, color: RULE });
            }
            page.ops.push(Op::Line { x1: ml + right, y1: top, x2: ml + right, y2: top + row_h, width: 0.6, color: RULE });
            self.y = top + row_h;
        }
        self.y += PARA_GAP;
    }

    /// Höjden en cell får när texten bryts i `width`.
    fn cell_height(&mut self, spans: &[Span], width: f32, size: f32, base: Face) -> f32 {
        self.break_lines(spans, base, size, width).iter().map(|l| line_size(l, size) * LINE).sum()
    }

    fn code(&mut self, text: &str, indent: f32) {
        self.clear_floats();
        let size = self.body * 0.86;
        let line_h = size * 1.45;
        let pad = 8.0;
        let w = self.cw - indent;
        let mut lines = Vec::new();
        for raw in text.split('\n') {
            let raw = raw.replace('\t', "    ");
            // Långa kodrader bryts på tecken.
            let mut cur = String::new();
            for c in raw.chars() {
                cur.push(c);
                if self.m.width(Face::Mono, size, &cur) > w - 2.0 * pad {
                    let last = cur.pop().unwrap_or(' ');
                    lines.push(std::mem::take(&mut cur));
                    cur.push(last);
                }
            }
            lines.push(cur);
        }
        let mut i = 0;
        while i < lines.len() {
            self.ensure(line_h + 2.0 * pad);
            let x = self.ml + indent;
            // Så många rader som får plats på sidan.
            let room = ((self.room_end() - self.y - 2.0 * pad) / line_h).floor().max(1.0) as usize;
            let chunk = &lines[i..(i + room).min(lines.len())];
            let h = chunk.len() as f32 * line_h + 2.0 * pad;
            let top = self.y;
            self.page().ops.push(Op::Rect { x, y: top, w, h, color: CODE_BG });
            for (j, l) in chunk.iter().enumerate() {
                if !l.is_empty() {
                    let y = top + pad + j as f32 * line_h + size * 1.05;
                    self.page().ops.push(Op::Text { x: x + pad, y, face: Face::Mono, size, text: l.clone(), color: INK, italic: false });
                }
            }
            i += chunk.len();
            self.y = top + h;
        }
        self.y += PARA_GAP;
    }

    /// Ritar fyllning (bakom texten, på `ops_at`) och kanter för ett stycke i en ram. Bara delen på
    /// sidan där stycket slutar ritas när det bryts över en sida.
    fn paint_box(&mut self, l: &crate::word::pbox::PPr, x: f32, w: f32, ops_at: Option<usize>, top: f32, first: bool, last: bool, same_page: bool) {
        // Luften under sista stycket, och avståndet fram till nästa stycke i samma ruta fylls också.
        let bottom = self.y + if last { l.pad_bottom() } else { PARA_GAP };
        let bottom = if last { bottom.max(self.box_min.unwrap_or(0.0)) } else { bottom };
        let start = if same_page { top - if first { l.pad_top() } else { 0.0 } } else { self.rt };
        let h = (bottom - start).max(1.0);
        let col = |c: Option<[u8; 3]>| c.map_or(INK, |c| (c[0], c[1], c[2]));
        let mut ops: Vec<Op> = Vec::new();
        let bd = &l.borders;
        if let Some(c) = l.shd {
            ops.push(Op::Rect { x, y: start, w, h, color: (c[0], c[1], c[2]) });
        }
        let draw_top = first && same_page;
        if let (Some(b), true) = (bd.top, draw_top) {
            ops.push(Op::Rect { x, y: start, w, h: b.width_pt(), color: col(b.color) });
        }
        if let (Some(b), true) = (bd.bottom, last) {
            ops.push(Op::Rect { x, y: bottom - b.width_pt(), w, h: b.width_pt(), color: col(b.color) });
        }
        if let Some(b) = bd.left {
            ops.push(Op::Rect { x, y: start, w: b.width_pt(), h, color: col(b.color) });
        }
        if let Some(b) = bd.right {
            ops.push(Op::Rect { x: x + w - b.width_pt(), y: start, w: b.width_pt(), h, color: col(b.color) });
        }
        // Fyllningen först (bakom texten), kanterna ovanpå.
        let mut at = ops_at.unwrap_or(0);
        let fill = l.shd.is_some();
        for (i, op) in ops.into_iter().enumerate() {
            if i == 0 && fill {
                let pg = self.page();
                let n = pg.ops.len();
                pg.ops.insert(at.min(n), op);
                at += 1;
            } else {
                self.page().ops.push(op);
            }
        }
    }

    fn run(&mut self, blocks: &[Block]) {
        self.run_range(blocks, 0, blocks.len());
    }

    /// Lägger ut `blocks[from..to]`. Blocken före och efter räknas ändå som grannar (`bi` är index i
    /// hela `blocks`), så att resultatet blir detsamma som när allt läggs ut i ett svep.
    fn run_range(&mut self, blocks: &[Block], from: usize, to: usize) {
        let mut skip_to = 0;
        for (bi, b) in blocks.iter().enumerate().take(to).skip(from) {
            if bi < skip_to {
                continue;
            }
            self.align = Align::Left;
            match b {
                Block::TBoxOpen(head) => {
                    let end = blocks[bi + 1..].iter().position(|b| matches!(b, Block::TBoxClose)).map_or(blocks.len(), |i| bi + 1 + i);
                    self.tbox(head, &blocks[bi + 1..end]);
                    skip_to = end + 1;
                }
                Block::TBoxClose => {}
                Block::Heading(level, spans, align) => {
                    self.align = *align;
                    let hb = self.head_face();
                    let (size, face, before) = match *level {
                        TITLE_LEVEL => (26.0, hb, 0.0),
                        SUBTITLE_LEVEL => (14.0, hb, 0.0),
                        1 => (22.0, hb.strong(), 14.0),
                        2 => (16.5, hb.strong(), 14.0),
                        3 => (13.5, hb.strong(), 10.0),
                        _ => (11.5, hb.strong(), 8.0),
                    };
                    let size = size * self.scale;
                    // Formatmallens utseende (typsnittet är alltid det inbäddade).
                    let id = match *level {
                        TITLE_LEVEL => style::StyleId::Title,
                        SUBTITLE_LEVEL => style::StyleId::Subtitle,
                        n => style::StyleId::Heading(n.clamp(1, 6)),
                    };
                    let st = self.styles.get(id).clone();
                    let size = st.size.unwrap_or(size);
                    let face = match (st.bold, *level >= TITLE_LEVEL) {
                        (Some(false), _) | (None, true) => hb,
                        (Some(true), true) => hb.strong(),
                        _ => face,
                    };
                    let ink = st.color.map_or(if *level == SUBTITLE_LEVEL { MUTED } else { INK }, |c| (c[0], c[1], c[2]));
                    if *align == Align::Left {
                        self.align = st.align.unwrap_or(Align::Left);
                    }
                    self.line = st.line_spacing.map_or(LINE, |m| m * LINE_SINGLE);
                    self.first_indent = st.first_indent.unwrap_or(0.0);
                    let before = before + st.space_before.unwrap_or(0.0);
                    if bi > 0 && self.y > self.rt + 0.5 {
                        self.y += before;
                    }
                    // Håll ihop rubriken med minst två rader text efter.
                    self.ensure(size * LINE + self.body * LINE * 2.0);
                    let text: String = spans.iter().map(|s| s.text.as_str()).collect::<String>().replace('\n', " ");
                    self.headings.push(Heading { level: *level, text: text.trim().to_owned(), page: self.pages.len() - 1, y: self.y });
                    let spans: Vec<Span> = spans.iter().map(|s| Span { text: s.text.replace('\n', " "), style: s.style.clone() }).collect();
                    self.text_block(&spans, self.ml, self.cw, face, size, ink, st.italic == Some(true), None);
                    self.line = LINE;
                    self.first_indent = 0.0;
                    self.y += st.space_after.unwrap_or(0.0);
                    // Linje under stycket (Words Titel har en).
                    if let Some(b) = st.border_bottom {
                        let (y, ml, cw) = (self.y + b.space * 0.5, self.ml, self.cw);
                        self.page().ops.push(Op::Rect { x: ml, y, w: cw, h: b.width, color: (b.color[0], b.color[1], b.color[2]) });
                        self.y += b.space * 0.5 + b.width + 4.0;
                    }
                    if *level == 1 {
                        let y = self.y + 2.0;
                        let ml = self.ml;
                        self.page().ops.push(Op::Rect { x: ml, y, w: 36.0, h: 2.2, color: ACCENT });
                        self.y += 9.0;
                    } else {
                        self.y += 3.0;
                    }
                }
                Block::Para { spans, indent, quote, align, look } => {
                    self.align = *align;
                    let page_before = self.breaks;
                    let (color, italic) = if *quote > 0 { (MUTED, true) } else { (INK, false) };
                    let st = self.styles.get(if *quote > 0 { style::StyleId::Quote } else { style::StyleId::Body }).clone();
                    let (color, italic) = (st.color.map_or(color, |c| (c[0], c[1], c[2])), st.italic.unwrap_or(italic));
                    let size = st.size.unwrap_or(self.body);
                    let face = if st.bold == Some(true) { self.base.strong() } else { self.base };
                    if *align == Align::Left {
                        self.align = st.align.unwrap_or(Align::Left);
                    }
                    self.line = st.line_spacing.map_or(LINE, |m| m * LINE_SINGLE);
                    self.first_indent = st.first_indent.unwrap_or(0.0);
                    self.y += st.space_before.unwrap_or(0.0);
                    // Ram och fyllning (textruta eller stycke med ram): text inne i rutan, ramen ritas under och runt.
                    let look_of = |b: Option<&Block>| match b {
                        Some(Block::Para { look: Some(l), .. }) => Some(l.clone()),
                        _ => None,
                    };
                    let boxed = look.as_ref().filter(|l| l.has_box());
                    let first_in_box = boxed.is_some_and(|l| bi == 0 || !look_of(blocks.get(bi.wrapping_sub(1)).filter(|_| bi > 0)).is_some_and(|p| p.same_box(l)));
                    let last_in_box = boxed.is_some_and(|l| !look_of(blocks.get(bi + 1)).is_some_and(|n| n.same_box(l)));
                    let (mut tx, mut tw) = (self.ml + indent, self.cw - indent);
                    let mut frame_x: Option<(f32, f32)> = None;
                    if let Some(l) = boxed {
                        if let Some((tl, tr, fl, fw)) = l.frame_geom(self.cw - indent, (self.ml, (self.pw - self.ml - self.cw).max(0.0))) {
                            tx += tl;
                            tw -= tl + tr;
                            frame_x = Some((self.ml + indent + fl, fw));
                        } else {
                            let (il, ir) = (l.left.unwrap_or(0.0).max(0.0), l.right.unwrap_or(0.0).max(0.0));
                            tx += il;
                            tw -= il + ir;
                            frame_x = Some((tx - l.pad_left(), tw + l.pad_left() + l.pad_right()));
                        }
                        if first_in_box {
                            self.y += l.pad_top();
                        }
                    }
                    let top = self.y;
                    let ops_at = self.page().ops.len();
                    let page_at = self.breaks;
                    self.text_block(spans, tx, tw.max(24.0), face, size, color, italic, None);
                    self.line = LINE;
                    self.first_indent = 0.0;
                    self.y += st.space_after.unwrap_or(0.0);
                    if let (Some(l), Some((fx, fw))) = (boxed, frame_x) {
                        self.paint_box(l, fx, fw, if page_at == self.breaks { Some(ops_at) } else { None }, top, first_in_box, last_in_box, page_at == self.breaks);
                    }
                    if *quote > 0 {
                        let x = self.ml + indent - 9.0;
                        let start = if self.breaks == page_before { top } else { self.rt };
                        let end = self.y;
                        self.page().ops.push(Op::Rect { x, y: start + 2.0, w: 2.0, h: (end - start - 2.0).max(1.0), color: RULE });
                    }
                    self.y += PARA_GAP;
                }
                Block::Item { spans, indent, marker, quote, align } => {
                    self.align = *align;
                    let mface = if marker.ends_with('.') { Face::Sans } else { Face::SansMedium };
                    let color = if *quote > 0 { MUTED } else { INK };
                    self.text_block(spans, self.ml + indent + 4.0, self.cw - indent - 4.0, self.base, self.body, color, *quote > 0, Some((marker.clone(), mface)));
                    let next_is_item = matches!(blocks.get(bi + 1), Some(Block::Item { .. }));
                    self.y += if next_is_item { 2.0 } else { PARA_GAP };
                }
                Block::Code { text, indent } => self.code(text, *indent),
                Block::Table { rows, spans, weights, fmt } => self.table(rows, spans, weights.as_deref(), fmt.as_ref()),
                Block::Rule => {
                    self.clear_floats();
                    self.ensure(16.0);
                    let y = self.y + 6.0;
                    let (ml, cw) = (self.ml, self.cw);
                    self.page().ops.push(Op::Line { x1: ml, y1: y, x2: ml + cw, y2: y, width: 0.8, color: RULE });
                    self.y += 16.0;
                }
                Block::Image { src, alt, indent, size, align, float, place } => self.image(src, alt, *indent, *size, *align, *float, *place),
                Block::Toc => self.toc(),
                Block::SectionBreak(props) => self.section_break(props),
                Block::ColumnBreak => {
                    self.manual_col = true;
                    self.new_page();
                }
                Block::PageBreak => {
                    // Ny sida (i alla spalter) om den nuvarande har innehåll.
                    let empty = self.pages.last().is_some_and(|p| p.ops.is_empty()) && self.col == 0 && (self.y - self.rt).abs() < 0.5;
                    if !empty && !self.pages.is_empty() {
                        self.real_page();
                    }
                }
            }
        }
    }
}

fn p_clone(p: &Piece) -> Piece {
    Piece { text: String::new(), face: p.face, size: p.size, width: 0.0, style: p.style.clone(), space: p.space, newline: p.newline }
}

/// Ritar en formel som fyllda konturer (glyfer) och rektanglar. `(x, y)` = vänster kant på baslinjen.
fn draw_math(s: &mut krilla::surface::Surface<'_>, src: &str, display: bool, x: f32, y: f32, em: f32, color: (u8, u8, u8)) {
    use crate::math::layout::Item;
    let (Some(f), Some(font)) = (crate::math::formula(src, display), crate::math::font::font()) else { return };
    s.set_stroke(None);
    s.set_fill(Some(fill(color)));
    for it in &f.layout.items {
        let mut pb = PathBuilder::new();
        match *it {
            Item::Glyph { gid, x: gx, y: gy, s: gs } => {
                struct B<'a> {
                    pb: &'a mut PathBuilder,
                    ox: f32,
                    oy: f32,
                    k: f32,
                }
                impl ttf_parser::OutlineBuilder for B<'_> {
                    fn move_to(&mut self, x: f32, y: f32) {
                        self.pb.move_to(self.ox + x * self.k, self.oy - y * self.k);
                    }
                    fn line_to(&mut self, x: f32, y: f32) {
                        self.pb.line_to(self.ox + x * self.k, self.oy - y * self.k);
                    }
                    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
                        self.pb.quad_to(self.ox + x1 * self.k, self.oy - y1 * self.k, self.ox + x * self.k, self.oy - y * self.k);
                    }
                    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
                        self.pb.cubic_to(
                            self.ox + x1 * self.k,
                            self.oy - y1 * self.k,
                            self.ox + x2 * self.k,
                            self.oy - y2 * self.k,
                            self.ox + x * self.k,
                            self.oy - y * self.k,
                        );
                    }
                    fn close(&mut self) {
                        self.pb.close();
                    }
                }
                let mut b = B { pb: &mut pb, ox: x + gx * em, oy: y - gy * em, k: gs * em / font.upem };
                font.outline(gid, &mut b);
            }
            Item::Rule { x: rx, y: ry, w, h } => {
                let h = h.max(0.012);
                if let Some(r) = Rect::from_xywh(x + rx * em, y - (ry + h) * em, w * em, h * em) {
                    pb.push_rect(r);
                }
            }
        }
        if let Some(path) = pb.finish() {
            s.draw_path(&path);
        }
    }
}

/// En bild på en sida: raster, eller en SVG som ritas som vektor (text och former, inga bildpunkter).
#[derive(Clone)]
enum Pic {
    Raster(Image),
    Svg(std::sync::Arc<resvg::usvg::Tree>),
}

impl Pic {
    /// Storleken i bildpunkter (96 dpi).
    fn size(&self) -> (f32, f32) {
        match self {
            Pic::Raster(i) => {
                let (w, h) = i.size();
                (w as f32, h as f32)
            }
            Pic::Svg(t) => (t.size().width(), t.size().height()),
        }
    }

    /// Ritar bilden med övre vänstra hörnet i origo i storleken `size`.
    fn draw(&self, s: &mut krilla::surface::Surface, size: Size) {
        match self {
            Pic::Raster(i) => s.draw_image(i.clone(), size),
            Pic::Svg(t) => {
                use krilla_svg::SurfaceExt;
                s.draw_svg(t, size, krilla_svg::SvgSettings::default());
            }
        }
    }
}

fn make_pic(img: &DocImage) -> Option<Pic> {
    if let Some(svg) = &img.svg {
        if let Some(tree) = crate::imgfmt::parse_svg(&svg.data) {
            return Some(Pic::Svg(std::sync::Arc::new(tree)));
        }
    }
    make_image(&img.bytes).map(Pic::Raster)
}

fn make_image(bytes: &[u8]) -> Option<Image> {
    let data = || krilla::Data::from(bytes.to_vec());
    match crate::docx::image_ext(bytes) {
        Some("png") => Image::from_png(data(), true).ok(),
        Some("jpeg") => Image::from_jpeg(data(), true).ok(),
        Some("gif") => Image::from_gif(data(), true).ok(),
        _ => {
            // BMP och annat: avkoda till RGBA.
            let img = image::load_from_memory(bytes).ok()?.to_rgba8();
            let (w, h) = img.dimensions();
            Some(Image::from_rgba8(img.into_raw(), w, h))
        }
    }
}

fn fill(c: (u8, u8, u8)) -> Fill {
    Fill { paint: rgb::Color::new(c.0, c.1, c.2).into(), opacity: NormalizedF32::ONE, rule: FillRule::NonZero }
}

/// Resultatet av att spara: filens innehåll och bilder som inte kunde bäddas in.
pub struct Written {
    pub bytes: Vec<u8>,
    pub missing_images: Vec<String>,
}

/// Dokumentets grundformat från första raden (`<!-- kladd: … -->`).
fn doc_defaults(md: &str) -> DocDefaults {
    let first = md.trim_start_matches('\u{feff}').lines().next().unwrap_or("");
    DocDefaults::from_comment(first).unwrap_or_default()
}

/// Lägger ut alla block. Med en innehållsförteckning görs två passager: förteckningens höjd beror
/// bara på antalet rubriker, så sidnumren från första passet stämmer i det andra.
/// Sidhuvudets och sidfotens utrymme per avsnitt (se `hf::Plan::reserve`); avsnittsraderna räknas som i
/// `Layout::section_break` (en rad före all text gäller första avsnittet).
fn hf_reserve(blocks: &[Block], defaults: &DocDefaults) -> Vec<[f32; 2]> {
    let mut secs: Vec<Section> = Vec::new();
    for b in blocks {
        match b {
            Block::SectionBreak(p) => secs.push(Section::resolve(p, defaults)),
            _ if secs.is_empty() => secs.push(Section::resolve(&SectionProps::default(), defaults)),
            _ => {}
        }
    }
    let plan = crate::hf::Plan::new(defaults, &secs);
    (0..secs.len().max(1)).map(|i| plan.reserve(defaults, i)).collect()
}

fn lay_out<'a>(blocks: &[Block], notes: &[Vec<Span>], defaults: &DocDefaults, resolve: &'a dyn Fn(&str) -> Option<DocImage>) -> Layout<'a> {
    // Rubrikerna H1–H3 till en eventuell innehållsförteckning (samma urval som bokmärkena).
    let toc_entries: Vec<(u8, String)> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Heading(l, spans, _) if *l <= 3 => {
                let t: String = spans.iter().map(|s| s.text.as_str()).collect::<String>().replace('\n', " ");
                (!t.trim().is_empty()).then(|| (*l, t.trim().to_owned()))
            }
            _ => None,
        })
        .collect();
    let reserve = Rc::new(hf_reserve(blocks, defaults));
    let toc_entries = Rc::new(toc_entries);
    let notes = Rc::new(notes.to_vec());
    // Mätcachen och de avkodade bilderna delas mellan passen.
    let (m, pics) = (Measurer::new(), Rc::<RefCell<HashMap<String, PicLoad>>>::default());
    let pass = |toc_pos: Vec<(usize, f32)>, balance: &[(usize, usize, f32)], search: bool| -> Layout<'a> {
        let mut lay = Layout::new(resolve, defaults);
        lay.m = m.clone();
        lay.pics = pics.clone();
        lay.hf_reserve = reserve.clone();
        lay.toc_entries = toc_entries.clone();
        lay.toc_pos = toc_pos;
        lay.notes = notes.clone();
        lay.balance = balance.to_vec();
        lay.new_page();
        if search {
            lay.run_balanced(blocks);
        } else {
            lay.run(blocks);
        }
        lay.flush_rules();
        lay.flush_notes();
        lay
    };
    let mut lay = pass(Vec::new(), &[], true);
    crate::timing::lap("pdf: utläggning (inkl. utjämning av spalter)");
    if blocks.iter().any(|b| matches!(b, Block::Toc)) {
        // Andra passet använder samma spalthöjder och söker inga nya.
        let balance = lay.balance.clone();
        let pos: Vec<(usize, f32)> =
            lay.headings.iter().filter(|h| h.level <= 3 && !h.text.is_empty()).map(|h| (h.page, h.y)).collect();
        lay = pass(pos, &balance, false);
        crate::timing::lap("pdf: utläggning, pass för innehållsförteckning");
    }
    lay
}

/// Layoutens tillstånd vid ett avsnitts början. `Layout::rollback` återställer det, så att avsnittet kan
/// läggas ut igen med en annan spalthöjd utan att allt före det läggs ut om.
struct Snapshot<'a> {
    /// Allt utom de växande listorna (sidor, rubriker, bilder, saknade bilder, avsnittsslut).
    state: Layout<'a>,
    pages: usize,
    /// Sidan som lades ut när bilden togs (den enda tidigare sida som kan ändras efteråt).
    last: Option<PageOps>,
    headings: usize,
    images: usize,
    missing: usize,
    sec_ends: usize,
}

impl<'a> Layout<'a> {
    fn snapshot(&mut self) -> Snapshot<'a> {
        let (pages, headings, images) = (std::mem::take(&mut self.pages), std::mem::take(&mut self.headings), std::mem::take(&mut self.images));
        let (missing, sec_ends) = (std::mem::take(&mut self.missing), std::mem::take(&mut self.sec_ends));
        let snap = Snapshot {
            state: self.clone(),
            pages: pages.len(),
            last: pages.last().cloned(),
            headings: headings.len(),
            images: images.len(),
            missing: missing.len(),
            sec_ends: sec_ends.len(),
        };
        (self.pages, self.headings, self.images, self.missing, self.sec_ends) = (pages, headings, images, missing, sec_ends);
        snap
    }

    fn rollback(&mut self, snap: &Snapshot<'a>) {
        let (mut pages, mut headings, mut images) = (std::mem::take(&mut self.pages), std::mem::take(&mut self.headings), std::mem::take(&mut self.images));
        let (mut missing, mut sec_ends) = (std::mem::take(&mut self.missing), std::mem::take(&mut self.sec_ends));
        pages.truncate(snap.pages);
        if let (Some(last), Some(slot)) = (&snap.last, pages.last_mut()) {
            *slot = last.clone();
        }
        headings.truncate(snap.headings);
        images.truncate(snap.images);
        missing.truncate(snap.missing);
        sec_ends.truncate(snap.sec_ends);
        *self = snap.state.clone();
        (self.pages, self.headings, self.images, self.missing, self.sec_ends) = (pages, headings, images, missing, sec_ends);
    }

    /// Lägger ut alla block och jämnar ut spalterna på avsnittets sista sida före varje löpande
    /// avsnittsbyte (som Word och LibreOffice; inte sist i dokumentet och inte när avsnittet har en
    /// manuell spaltbrytning på sidan). Avsnitten läggs ut ett i taget, framåt: vid ett byte med
    /// spalter söks lägsta spalthöjd där allt ryms kvar på sidan genom att bara det avsnittet läggs ut
    /// igen från sitt början (först ur ytan delad på spaltbredden, sedan med växande steg och halvering).
    /// Dokumentet före avsnittet påverkas inte av höjden, så resultatet är detsamma som när hela
    /// dokumentet läggs ut om för varje försök, men tiden växer inte med antalet avsnitt.
    fn run_balanced(&mut self, blocks: &[Block]) {
        // Avsnittsbyten på översta nivån (de i textrutor läggs ut av rutan).
        let mut breaks = Vec::new();
        let mut i = 0;
        while i < blocks.len() {
            match &blocks[i] {
                Block::TBoxOpen(_) => {
                    i = blocks[i + 1..].iter().position(|b| matches!(b, Block::TBoxClose)).map_or(blocks.len(), |k| i + 1 + k) + 1;
                    continue;
                }
                Block::SectionBreak(_) => breaks.push(i),
                _ => {}
            }
            i += 1;
        }
        let mut start = 0;
        let mut snap = self.snapshot();
        for j in breaks {
            let before = self.sec_ends.len();
            self.run_range(blocks, start, j + 1);
            if self.sec_ends.len() > before {
                self.balance_section(blocks, start, j + 1, &snap);
            }
            start = j + 1;
            snap = self.snapshot();
        }
        self.run_range(blocks, start, blocks.len());
    }

    /// Avsnittet `blocks[from..to]` (till och med bytet) har just lagts ut med ett avsnittsslut sist i
    /// `sec_ends`: väljer spalthöjd för dess sista sida och lägger ut avsnittet med den.
    fn balance_section(&mut self, blocks: &[Block], from: usize, to: usize, snap: &Snapshot<'a>) {
        let Some(e) = self.sec_ends.last() else { return };
        let (sec, page, top, step) = (e.sec, e.page, e.top, e.step.max(1.0));
        let full = e.bottom - top;
        let low = (e.area / e.width.max(1.0)).max(step);
        if e.skip || low >= full - 0.5 {
            return;
        }
        // Ryms allt på sidan med spalthöjden `h`? Då slutar avsnittet på samma sida.
        let fits = |lay: &mut Self, h: f32| -> bool {
            lay.rollback(snap);
            lay.balance.push((sec, page, h));
            lay.run_range(blocks, from, to);
            lay.sec_ends.iter().find(|e| e.sec == sec).is_some_and(|e| e.page == page)
        };
        let (mut lo, mut hi, mut best) = (low - step * 0.5, full, None);
        // Växande steg uppåt från den lägsta möjliga höjden, sedan halvering.
        let mut grow = step;
        let mut h = low;
        while h < full - 0.5 {
            if fits(self, h) {
                best = Some(h);
                hi = h;
                break;
            }
            lo = h;
            h = (h + grow).min(full);
            grow *= 2.0;
        }
        while best.is_some() && hi - lo > 2.0 {
            let mid = (lo + hi) / 2.0;
            if fits(self, mid) {
                best = Some(mid);
                hi = mid;
            } else {
                lo = mid;
            }
        }
        // Sista utläggningen med den valda höjden (eller utan utjämning om ingen höjd räckte).
        self.rollback(snap);
        if let Some(h) = best {
            self.balance.push((sec, page, h));
        }
        self.run_range(blocks, from, to);
    }
}

/// Bygger en PDF av Markdown-text.
pub fn write(markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<DocImage>) -> Result<Written, String> {
    // Ändringar godkänns och kommentarer utelämnas i PDF.
    crate::timing::lap_start();
    let cleaned = crate::review::clean_for_export(markdown);
    let markdown = cleaned.as_str();
    let (blocks, notes) = parse_blocks_notes(markdown);
    let defaults = doc_defaults(markdown);
    crate::timing::lap("pdf: förberedelse (rensning, tolkning)");
    let mut lay = lay_out(&blocks, &notes, &defaults, resolve);

    let fonts: HashMap<Face, Font> = FACES
        .into_iter()
        .map(|f| (f, Font::new(krilla::Data::from(f.data()), 0).expect("inbyggt typsnitt")))
        .collect();

    let shapers = lay.m.shaper.clone();
    crate::timing::lap("pdf: typsnitt");
    let mut doc = Document::new();
    let total = lay.pages.len();
    // Sidhuvud och sidfot ritas sist, när sidantalet är känt.
    let fields = style::Fields::for_export(title, "pdf").at(1, total);
    let secs = sections::scan(markdown, &defaults);
    let hf_plan = crate::hf::Plan::new(&defaults, &secs);
    let hf_pages = hf_plan.pages(&lay.pages.iter().map(|p| p.sec).collect::<Vec<_>>());
    let extras: Vec<Vec<Op>> = (0..total).map(|pi| lay.header_footer_page(&defaults, &hf_plan, &hf_pages, &fields, pi)).collect();
    let images = std::mem::take(&mut lay.images);
    crate::timing::lap("pdf: sidhuvud/sidfot");
    // Texterna formas på flera trådar, några sidor i taget (formen är densamma som `Surface::draw_text`).
    let mut shaped: std::collections::VecDeque<std::vec::IntoIter<Vec<krilla::text::KrillaGlyph>>> = std::collections::VecDeque::new();
    for (pi, page_ops) in lay.pages.iter().enumerate() {
        if shaped.is_empty() {
            let end = (pi + SHAPE_BATCH).min(total);
            let jobs: Vec<Vec<(Face, &str)>> = (pi..end)
                .map(|k| {
                    lay.pages[k]
                        .ops
                        .iter()
                        .chain(extras[k].iter())
                        .filter_map(|o| match o {
                            Op::Text { face, text, .. } => Some((*face, text.as_str())),
                            _ => None,
                        })
                        .collect()
                })
                .collect();
            shaped = shape_pages(&jobs, &shapers).into_iter().map(Vec::into_iter).collect();
        }
        let mut page_glyphs = shaped.pop_front().expect("formade sidor");
        let mut page = doc.start_page_with(PageSettings::from_wh(page_ops.geom.pw, page_ops.geom.ph).ok_or("ogiltig sidstorlek")?);
        let mut links = Vec::new();
        let mut gotos = Vec::new();
        {
            let mut s = page.surface();
            for op in page_ops.ops.iter().chain(extras[pi].iter()) {
                match op {
                    Op::Rect { x, y, w, h, color } => {
                        if let Some(r) = Rect::from_xywh(*x, *y, *w, *h) {
                            let mut pb = PathBuilder::new();
                            pb.push_rect(r);
                            if let Some(path) = pb.finish() {
                                s.set_fill(Some(fill(*color)));
                                s.set_stroke(None);
                                s.draw_path(&path);
                            }
                        }
                    }
                    Op::Math { x, y, src, display, em, color } => draw_math(&mut s, src, *display, *x, *y, *em, *color),
                    Op::Line { x1, y1, x2, y2, width, color } => {
                        let mut pb = PathBuilder::new();
                        pb.move_to(*x1, *y1);
                        pb.line_to(*x2, *y2);
                        if let Some(path) = pb.finish() {
                            s.set_fill(None);
                            s.set_stroke(Some(Stroke {
                                paint: rgb::Color::new(color.0, color.1, color.2).into(),
                                width: *width,
                                miter_limit: 4.0,
                                line_cap: LineCap::Butt,
                                line_join: LineJoin::Miter,
                                opacity: NormalizedF32::ONE,
                                dash: None,
                            }));
                            s.draw_path(&path);
                        }
                    }
                    Op::Text { x, y, face, size, text, color, italic } => {
                        s.set_stroke(None);
                        s.set_fill(Some(fill(*color)));
                        if *italic {
                            // Plex finns inte i kursiv här: luta texten 12° (falsk kursiv).
                            s.push_transform(&Transform::from_row(1.0, 0.0, -0.21, 1.0, 0.21 * y, 0.0));
                        }
                        let glyphs = page_glyphs.next().expect("formad text");
                        s.draw_glyphs(Point::from_xy(*x, *y), &glyphs, fonts[face].clone(), text, *size, false);
                        if *italic {
                            s.pop();
                        }
                    }
                    Op::Image { x, y, w, h, index, crop } => {
                        if let Some(size) = Size::from_wh(*w, *h) {
                            match crop {
                                // Beskärning: hela bilden ritas större och klipps till rutan.
                                Some(c) => {
                                    let (fw, fh) = (*w / (1.0 - (c[0] + c[2]) / 100.0), *h / (1.0 - (c[1] + c[3]) / 100.0));
                                    if let (Some(full), Some(r)) = (Size::from_wh(fw, fh), Rect::from_xywh(*x, *y, *w, *h)) {
                                        let mut pb = PathBuilder::new();
                                        pb.push_rect(r);
                                        if let Some(path) = pb.finish() {
                                            s.push_clip_path(&path, &FillRule::NonZero);
                                            s.push_transform(&Transform::from_translate(*x - fw * c[0] / 100.0, *y - fh * c[1] / 100.0));
                                            images[*index].draw(&mut s, full);
                                            s.pop();
                                            s.pop();
                                        }
                                    }
                                }
                                None => {
                                    s.push_transform(&Transform::from_translate(*x, *y));
                                    images[*index].draw(&mut s, size);
                                    s.pop();
                                }
                            }
                        }
                    }
                    Op::Link { x, y, w, h, url } => links.push((*x, *y, *w, *h, url.clone())),
                    Op::Goto { x, y, w, h, page, to_y } => gotos.push((*x, *y, *w, *h, *page, *to_y)),
                }
            }
            s.finish();
        }
        for (x, y, w, h, url) in links {
            if let Some(rect) = Rect::from_xywh(x, y, w.max(1.0), h) {
                let target = krilla::annotation::Target::Action(krilla::action::Action::Link(krilla::action::LinkAction::new(url)));
                page.add_annotation(krilla::annotation::Annotation::new_link(krilla::annotation::LinkAnnotation::new(rect, target), None));
            }
        }
        for (x, y, w, h, to_page, to_y) in gotos {
            if let Some(rect) = Rect::from_xywh(x, y, w.max(1.0), h) {
                let dest = krilla::destination::XyzDestination::new(to_page, Point::from_xy(lay.ml, to_y));
                let target = krilla::annotation::Target::Destination(krilla::destination::Destination::Xyz(dest));
                page.add_annotation(krilla::annotation::Annotation::new_link(krilla::annotation::LinkAnnotation::new(rect, target), None));
            }
        }
        page.finish();
    }

    crate::timing::lap("pdf: ritning av sidor");
    // Bokmärken från rubrikerna (nivå 1–3), nästlade efter nivå.
    let mut outline = krilla::outline::Outline::new();
    let mut stack: Vec<(u8, krilla::outline::OutlineNode)> = Vec::new();
    for h in lay.headings.iter().filter(|h| h.level <= 3 && !h.text.is_empty()) {
        let dest = krilla::destination::XyzDestination::new(h.page, Point::from_xy(lay.ml, h.y));
        let node = krilla::outline::OutlineNode::new(h.text.clone(), dest);
        while stack.last().is_some_and(|(l, _)| *l >= h.level) {
            let (_, done) = stack.pop().unwrap();
            match stack.last_mut() {
                Some((_, parent)) => parent.push_child(done),
                None => outline.push_child(done),
            }
        }
        stack.push((h.level, node));
    }
    while let Some((_, done)) = stack.pop() {
        match stack.last_mut() {
            Some((_, parent)) => parent.push_child(done),
            None => outline.push_child(done),
        }
    }
    doc.set_outline(outline);

    let heading_title = lay.headings.first().map(|h| h.text.clone());
    doc.set_metadata(
        krilla::metadata::Metadata::new()
            .title(heading_title.unwrap_or_else(|| title.to_owned()))
            .creator("Kladd".to_owned())
            .producer(format!("Kladd {}", env!("CARGO_PKG_VERSION")))
            .language("sv".to_owned()),
    );

    crate::timing::lap("pdf: bokmärken, metadata");
    let bytes = doc.finish().map_err(|e| crate::tf!("PDF:en kunde inte skapas: {e}", e = format!("{e:?}")))?;
    crate::timing::lap("pdf: skrivning (typsnittsdelmängder, bilder, komprimering)");
    Ok(Written { bytes, missing_images: lay.missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lägger ut Markdown och ger sidans ritinstruktioner (för kontroll av tabellernas geometri).
    fn first_page_ops(md: &str) -> (Vec<(f32, f32, f32, f32)>, Vec<(f32, f32, String)>, f32, f32) {
        let blocks = parse_blocks(md);
        let defaults = doc_defaults(md);
        let resolve = |_: &str| None;
        let mut lay = Layout::new(&resolve, &defaults);
        lay.new_page();
        lay.run(&blocks);
        let mut vlines = Vec::new();
        let mut texts = Vec::new();
        for op in &lay.pages[0].ops {
            match op {
                Op::Line { x1, y1, x2, y2, .. } if (x1 - x2).abs() < 0.01 => vlines.push((*x1, *y1, *x2, *y2)),
                Op::Text { x, y, text, .. } => texts.push((*x, *y, text.clone())),
                _ => {}
            }
        }
        (vlines, texts, lay.ml, lay.cw)
    }

    fn page_ops(md: &str) -> Vec<Op> {
        let blocks = parse_blocks(md);
        let defaults = doc_defaults(md);
        let resolve = |_: &str| None;
        let mut lay = Layout::new(&resolve, &defaults);
        lay.new_page();
        lay.run(&blocks);
        lay.pages.swap_remove(0).ops
    }

    #[test]
    fn framed_paragraphs_and_text_boxes_are_painted() {
        let pp = |frame: &str| format!("<!--k:pp bdr=\"t=single:8:4:112233 l=single:8:4:112233 b=single:8:4:112233 r=single:8:4:112233\" shd=\"FFF2CC\"{frame}-->");
        // En ram med egen bredd (textruta): två stycken ger en fyllning bakom texten och fyra kanter.
        let md = format!(
            "<!--k:tb 1-->\n\nEtt{m}\n\nTvå{m}\n\n<!--k:/tb-->\n\nEfter\n",
            m = pp(" fr=\"w=200 x=end pl=8 pr=8\"")
        );
        let ops = page_ops(&md);
        let rects: Vec<(f32, f32, (u8, u8, u8))> = ops.iter().filter_map(|o| if let Op::Rect { x, w, color, .. } = o { Some((*x, *w, *color)) } else { None }).collect();
        let fills: Vec<_> = rects.iter().filter(|r| r.2 == (0xFF, 0xF2, 0xCC)).collect();
        assert_eq!(fills.len(), 2, "{rects:?}");
        // Fyllningen ligger före texten i ritordningen.
        let first_text = ops.iter().position(|o| matches!(o, Op::Text { .. })).unwrap();
        let first_fill = ops.iter().position(|o| matches!(o, Op::Rect { color: (0xFF, 0xF2, 0xCC), .. })).unwrap();
        assert!(first_fill < first_text);
        // Rutan är 200 pt bred och ligger vid högerkanten; kantlinjerna har kantens färg; inga markörer som text.
        assert!((fills[0].1 - 200.0).abs() < 0.5);
        assert!(rects.iter().filter(|r| r.2 == (0x11, 0x22, 0x33)).count() >= 6, "{rects:?}");
        assert!(ops.iter().all(|o| !matches!(o, Op::Text { text, .. } if text.contains("k:tb") || text.contains("k:pp"))));
    }

    #[test]
    fn anchored_text_box_floats_and_text_wraps_around_it() {
        let pp = "<!--k:pp bdr=\"t=single:8:4:112233 l=single:8:4:112233 b=single:8:4:112233 r=single:8:4:112233\" shd=\"FFF2CC\" fr=\"w=120 x=end pl=8 pr=8 h=100\"-->";
        let lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam.";
        let md = format!("Före\n\n<!--k:tb 1 w=\"160\" h=\"133.3\" x=\"right\" y=\"0\"-->\n\nRutan{pp}\n\n<!--k:/tb-->\n\n{lorem}\n\n{lorem}\n");
        let (lay, _) = laid_out(&md, &|_| None);
        let ops = &lay.pages[0].ops;
        let t = texts(ops);
        let (bx, by, _) = t.iter().find(|(_, _, s)| s.contains("Rutan")).unwrap().clone();
        let (_, ly, _) = t.iter().find(|(_, _, s)| s.contains("Lorem")).unwrap().clone();
        // Rutans text ligger vid högerkanten, på höjd med första textraden (inte under den).
        assert!(bx > lay.ml + lay.cw - 130.0, "{bx}");
        assert!((by - ly).abs() < 30.0, "{by} {ly}");
        // Texten bredvid slutar före rutan (120 pt + luft); fyllningen har den fasta höjden 100 pt.
        let near: Vec<_> = t.iter().filter(|(x, y, _)| *y < by + 60.0 && *y > ly - 1.0 && *x < lay.ml + 5.0).collect();
        assert!(!near.is_empty());
        let fill = ops.iter().find_map(|o| if let Op::Rect { h, color: (0xFF, 0xF2, 0xCC), .. } = o { Some(*h) } else { None }).unwrap();
        assert!(fill >= 99.0, "{fill}");
    }

    #[test]
    fn table_format_gives_fills_borders_and_no_comment_text() {
        use crate::word::tablefmt::{TableFmt, Width};
        let mut f = TableFmt::word_base();
        f.cell_mut(0, 0).fill = Some([0x5B, 0x3A, 0x9E]);
        f.width = Width::Pct(50);
        let md = format!("{}\n\n| A | B |\n| - | - |\n| c | d |\n", f.to_comment());
        let ops = page_ops(&md);
        // Skuggningen i cellens färg, halva textbredden, och ingen linje (inga ramar).
        let fills: Vec<_> = ops.iter().filter_map(|o| if let Op::Rect { w, color, .. } = o { Some((*w, *color)) } else { None }).collect();
        assert_eq!(fills.len(), 1, "{fills:?}");
        assert_eq!(fills[0].1, (0x5B, 0x3A, 0x9E));
        assert!(ops.iter().all(|o| !matches!(o, Op::Line { .. })));
        assert!(ops.iter().all(|o| !matches!(o, Op::Text { text, .. } if text.contains("kladd-tabell"))));
        // Med rutnät ritas linjer och rubrikraden är inte fet av sig själv.
        let mut g = TableFmt::word_base();
        g.borders = [crate::word::tablefmt::Border::GRID; 6];
        let md = format!("{}\n\n| A | B |\n| - | - |\n| c | d |\n", g.to_comment());
        assert!(page_ops(&md).iter().any(|o| matches!(o, Op::Line { .. })));
        assert!(write(&md, "x", &|_| None).is_ok());
    }

    #[test]
    fn table_columns_follow_dash_counts() {
        let (v, _, ml, cw) = first_page_ops("| A | B | C |\n| ---- | -- | -- |\n| 1 | 2 | 3 |\n");
        let mut xs: Vec<f32> = v.iter().map(|l| l.0).collect();
        xs.sort_by(|a, b| a.total_cmp(b));
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert_eq!(xs.len(), 4, "{xs:?}");
        assert!((xs[0] - ml).abs() < 0.01 && (xs[3] - (ml + cw)).abs() < 0.01, "{xs:?} {ml} {cw}");
        let w: Vec<f32> = xs.windows(2).map(|p| p[1] - p[0]).collect();
        assert!((w[0] / w[1] - 2.0).abs() < 0.01 && (w[1] - w[2]).abs() < 0.01, "{w:?}");
    }

    #[test]
    fn table_merged_cells_have_no_inner_border() {
        let (v, texts, ml, _) = first_page_ops("| Två || x |\n| --- | --- | --- |\n| a | b | c |\n");
        // Rubrikradens lodräta streck: vänster, mellan sammanfogad cell och x, höger. Brödraden har fyra.
        let tops: Vec<f32> = {
            let mut t: Vec<f32> = v.iter().map(|l| l.1).collect();
            t.sort_by(|a, b| a.total_cmp(b));
            t.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            t
        };
        assert_eq!(tops.len(), 2, "{tops:?}");
        let head = v.iter().filter(|l| (l.1 - tops[0]).abs() < 0.01).count();
        let body = v.iter().filter(|l| (l.1 - tops[1]).abs() < 0.01).count();
        assert_eq!((head, body), (3, 4), "{v:?}");
        // Texten i den sammanfogade cellen börjar i första kolumnen och "x" efter den.
        let tva = texts.iter().find(|t| t.2 == "Två").unwrap();
        let x = texts.iter().find(|t| t.2 == "x").unwrap();
        assert!(tva.0 < ml + 10.0 && x.0 > tva.0 + 30.0, "{tva:?} {x:?}");
    }

    #[test]
    fn table_cells_wrap_and_break_lines() {
        let one = first_page_ops("| A | B |\n| --- | --- |\n| kort | kort |\n").0;
        let height = |v: &[(f32, f32, f32, f32)], row: usize| {
            let mut t: Vec<&(f32, f32, f32, f32)> = v.iter().collect();
            t.sort_by(|a, b| a.1.total_cmp(&b.1));
            let l = t[row * 3];
            l.3 - l.1
        };
        let h1 = height(&one, 1);
        // `<br>` ger tre rader i cellen.
        let (v, texts, _, _) = first_page_ops("| A | B |\n| --- | --- |\n| a<br>b<br>c | kort |\n");
        assert!(height(&v, 1) > 2.0 * h1, "{} vs {h1}", height(&v, 1));
        let ys: Vec<f32> = ["a", "b", "c"].iter().map(|n| texts.iter().find(|t| t.2 == *n).unwrap().1).collect();
        assert!(ys[0] < ys[1] && ys[1] < ys[2], "{ys:?}");
        // Lång text bryts inom sin kolumn i stället för att göra tabellen bredare.
        let long = "ord ".repeat(60);
        let (v, _, ml, cw) = first_page_ops(&format!("| A | B |\n| --- | --- |\n| {long} | kort |\n"));
        assert!(height(&v, 1) > 2.0 * h1, "{}", height(&v, 1));
        assert!(v.iter().all(|l| l.0 <= ml + cw + 0.01), "tabellen är bredare än texten: {v:?} {ml} {cw}");
    }

    #[test]
    fn writes_a_pdf() {
        let md = "# Rubrik\n\nEn **fet** och *kursiv* rad med [länk](https://example.com) och `kod`.\n\n- ett\n- två\n    1. nästlad\n\n> citat\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\n```\nkod\n```\n\n---\n\n![saknas](finns-inte.png)\n";
        let w = write(md, "Test", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF-"));
        assert_eq!(w.missing_images, vec!["finns-inte.png".to_owned()]);
    }

    #[test]
    fn toc_has_title_entries_and_real_page_numbers() {
        let mut md = String::from("<!-- kladd: toc -->\n\n");
        for i in 1..=4 {
            md.push_str(&format!("# Kapitel {i}\n\n"));
            for _ in 0..22 {
                md.push_str("Brödtext som fyller ut sidan så att kapitlen hamnar på olika sidor i dokumentet. Och lite till så att raden bryts.\n\n");
            }
            md.push_str(&format!("## Avsnitt {i}\n\nKort.\n\n"));
        }
        let w = write(&md, "T", &|_| None).unwrap();
        // Rubrikernas riktiga sidor, ur layouten.
        let blocks = parse_blocks(&md);
        assert!(blocks.iter().any(|b| matches!(b, Block::Toc)));
        let lay = lay_out(&blocks, &[], &doc_defaults(&md), &|_| None);
        let pages: Vec<usize> = lay.headings.iter().map(|h| h.page + 1).collect();
        assert_eq!(pages.len(), 8);
        assert!(pages.last().unwrap() > &2, "{pages:?}");
        // PDF:en innehåller förteckningen med de sidnummer rubrikerna faktiskt fick.
        let back = crate::pdf_read::read(&w.bytes, None).ok().expect("läsbar PDF");
        assert!(back.markdown.contains("Innehåll"), "{}", back.markdown);
        for (name, page) in ["Kapitel 1", "Avsnitt 1", "Kapitel 2", "Avsnitt 2", "Kapitel 3", "Avsnitt 3", "Kapitel 4", "Avsnitt 4"].iter().zip(&pages) {
            let at = back.markdown.find(&format!("{name} .")).unwrap_or_else(|| panic!("{name} saknas i förteckningen:\n{}", back.markdown));
            let rest = back.markdown[at + name.len()..].trim_start_matches([' ', '.']);
            let num: String = rest.chars().take_while(char::is_ascii_digit).collect();
            assert_eq!(num, page.to_string(), "{name}:\n{}", back.markdown);
        }
        assert!(back.page_count >= 3);
    }

    #[test]
    fn underline_tags_are_not_literal() {
        let w = write("En <u>understruken</u> text.\n", "Test", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF-"));
        let blocks = parse_blocks("En <u>understruken</u> text.\n");
        let Some(Block::Para { spans, .. }) = blocks.first() else { panic!("inget stycke") };
        assert!(spans.iter().all(|s| !s.text.contains("<u>") && !s.text.contains("</u>")));
        let under: String = spans.iter().filter(|s| s.style.underline).map(|s| s.text.as_str()).collect();
        assert_eq!(under, "understruken");
    }

    fn all_spans(blocks: &[Block]) -> Vec<&Span> {
        let mut out = Vec::new();
        for b in blocks {
            match b {
                Block::Heading(_, s, _) | Block::Para { spans: s, .. } | Block::Item { spans: s, .. } => out.extend(s.iter()),
                _ => {}
            }
        }
        out
    }

    const FULL: &str = "<!-- kladd: font-family: \"Courier New\"; font-size: 12pt -->\n\n# Rubrik\n\n<div align=\"center\">\n\nMitten med <span style=\"color: #C00000; font-size: 20pt\">röd stor</span> text.\n\n</div>\n\n<div align=\"justify\">\n\nEtt <span style=\"background-color: #FFFF00\">gult <span style=\"font-family: Times New Roman\">inre</span></span> stycke som är långt nog att bryta över flera rader så att marginaljusteringen får något att göra med mellanrummen.\n\n- punkt i <span style=\"font-family: Consolas\">mono</span>\n\n</div>\n\n<div align=\"right\">\n\nHöger\n\n</div>\n\n<!-- annan kommentar -->\n\nSlut.\n";

    #[test]
    fn span_colour_size_background_are_carried() {
        let blocks = parse_blocks("A <span style=\"color: #C00000; font-size: 20pt; background-color: #FFFF00\">röd <b>x</b></span> B.\n");
        let spans = all_spans(&blocks);
        let red: Vec<_> = spans.iter().filter(|s| s.style.span.color.is_some()).collect();
        assert!(!red.is_empty());
        for s in &red {
            assert_eq!(s.style.span.color, Some([0xC0, 0, 0]));
            assert_eq!(s.style.span.font_size, Some(20.0));
            assert_eq!(s.style.span.background, Some([0xFF, 0xFF, 0]));
        }
        assert!(spans.iter().any(|s| s.style.span.is_empty() && s.text.contains('B')));
    }

    #[test]
    fn nested_spans_merge_and_pop() {
        let blocks = parse_blocks("<span style=\"color: red\">a <span style=\"font-size: 9pt\">b</span> c</span> d\n");
        let spans = all_spans(&blocks);
        let b = spans.iter().find(|s| s.text.contains('b')).unwrap();
        assert_eq!(b.style.span.color, Some([255, 0, 0]));
        assert_eq!(b.style.span.font_size, Some(9.0));
        let c = spans.iter().find(|s| s.text.contains('c')).unwrap();
        assert_eq!(c.style.span.font_size, None);
        let d = spans.iter().find(|s| s.text.contains('d')).unwrap();
        assert!(d.style.span.is_empty());
    }

    #[test]
    fn div_alignment_is_carried_and_tags_hidden() {
        let blocks = parse_blocks(FULL);
        let mut aligns = Vec::new();
        for b in &blocks {
            match b {
                Block::Heading(_, _, a) | Block::Para { align: a, .. } | Block::Item { align: a, .. } => aligns.push(*a),
                _ => {}
            }
        }
        // rubrik, mitten, justerat stycke, punkt, höger, slut
        assert_eq!(aligns, vec![Align::Left, Align::Center, Align::Justify, Align::Justify, Align::Right, Align::Left]);
        for s in all_spans(&blocks) {
            for bad in ["<span", "</span", "<div", "</div", "<!--", "-->"] {
                assert!(!s.text.contains(bad), "{bad} i {:?}", s.text);
            }
        }
    }

    #[test]
    fn family_classification() {
        assert_eq!(classify_family("Times New Roman"), Family::Serif);
        assert_eq!(classify_family("Georgia"), Family::Serif);
        assert_eq!(classify_family("Courier New"), Family::Mono);
        assert_eq!(classify_family("Consolas"), Family::Mono);
        assert_eq!(classify_family("Arial"), Family::Sans);
        assert_eq!(classify_family("Noto Sans"), Family::Sans);
        let mut st = Style::default();
        st.span.font_family = Some("Courier New".into());
        assert_eq!(face_for(&st, Face::Sans), Face::Mono);
        // Serif blir inbäddade Caladea, Calibri Carlito; fet ger fet variant av samma typsnitt.
        st.span.font_family = Some("Georgia".into());
        assert_eq!(face_for(&st, Face::Sans), Face::Serif);
        st.span.font_family = Some("Cambria".into());
        st.bold = true;
        assert_eq!(face_for(&st, Face::Sans), Face::SerifBold);
        st.span.font_family = Some("Calibri Light".into());
        assert_eq!(face_for(&st, Face::Sans), Face::CarlitoBold);
        st.span.font_family = None;
        st.bold = false;
        assert_eq!(face_for(&st, Face::Carlito), Face::Carlito);
        assert_eq!(Face::for_family("Arial"), Face::Sans);
        assert_eq!(Face::for_family("Calibri"), Face::Carlito);
        assert_eq!(Face::for_family("Times New Roman"), Face::Serif);
    }

    /// PDF med Cambria och Calibri bäddar in Caladea respektive Carlito.
    #[test]
    fn pdf_med_cambria_och_calibri() {
        for fam in ["Cambria", "Calibri"] {
            let md = format!("<!-- kladd: font-family: \"{fam}\"; font-size: 11pt -->\n\n# Rubrik åäö\n\nBrödtext med **fet** och *kursiv*.\n");
            let w = write(&md, "T", &|_| None).unwrap();
            assert!(w.bytes.starts_with(b"%PDF"));
            let hay = String::from_utf8_lossy(&w.bytes);
            let want = if fam == "Cambria" { "Caladea" } else { "Carlito" };
            assert!(hay.contains(want), "{fam}: {want} saknas i PDF:en");
        }
    }

    #[test]
    fn doc_defaults_from_first_line() {
        let d = doc_defaults(FULL);
        assert_eq!(d.font_size, Some(12.0));
        assert_eq!(d.font_family.as_deref(), Some("Courier New"));
        assert!(doc_defaults("# Hej\n").is_empty());
    }

    fn test_layout(resolve: &dyn Fn(&str) -> Option<DocImage>) -> Layout<'_> {
        Layout::new(resolve, &DocDefaults { font_size: Some(BODY_SIZE), ..Default::default() })
    }

    #[test]
    fn justify_fills_every_line_but_the_last() {
        let resolve = |_: &str| None;
        let mut lay = test_layout(&resolve);
        let text = "Det här är en ganska lång mening som måste brytas på flera rader för att vi ska kunna mäta hur marginaljusteringen fördelar utrymmet.";
        let spans = vec![Span { text: text.into(), style: Style::default() }];
        let width = 200.0;
        let lines = lay.break_lines(&spans, Face::Sans, BODY_SIZE, width);
        assert!(lines.len() >= 3);
        let n = lines.len();
        for (i, mut l) in lines.into_iter().enumerate() {
            let before: f32 = l.pieces.iter().map(|p| p.width).sum();
            assert!(before <= width + 0.01);
            if i + 1 < n && !l.hard {
                justify(&mut l.pieces, width);
                let after: f32 = l.pieces.iter().map(|p| p.width).sum();
                assert!((after - width).abs() < 0.01, "rad {i}: {after}");
            }
        }
    }

    #[test]
    fn bigger_span_raises_the_line() {
        let resolve = |_: &str| None;
        let mut lay = test_layout(&resolve);
        let mut big = Style::default();
        big.span.font_size = Some(24.0);
        let spans = vec![Span { text: "liten ".into(), style: Style::default() }, Span { text: "stor".into(), style: big }];
        let lines = lay.break_lines(&spans, Face::Sans, BODY_SIZE, 400.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_size(&lines[0], BODY_SIZE), 24.0);
    }

    #[test]
    fn style_overrides_change_pdf_layout() {
        let resolve = |_: &str| None;
        let ys = |md: &str| {
            let (lay, _) = laid_out(md, &resolve);
            (lay.headings[0].y, lay.y)
        };
        let doc = "Före\n\n# Rubrik\n\nEfter\n";
        let (h0, end0) = ys(doc);
        // Luft före rubriken flyttar den, luft efter flyttar texten under.
        let (h1, end1) = ys(&format!("<!-- kladd: h1: space-before 20pt space-after 10pt -->\n\n{doc}"));
        assert!((h1 - h0 - 20.0).abs() < 0.01, "{h0} {h1}");
        assert!((end1 - end0 - 30.0).abs() < 0.01, "{end0} {end1}");
        // Radavstånd i brödtexten och större rubrikstorlek ger en högre sida.
        let (_, end2) = ys(&format!("<!-- kladd: body: line-spacing 2 -->\n\n{doc}"));
        assert!(end2 > end0, "{end0} {end2}");
        let big = ys(&format!("<!-- kladd: h1: 40pt -->\n\n{doc}")).1;
        assert!(big > end0);
        // Hela dokumentet blir ändå en PDF.
        let w = write("<!-- kladd: h1: 30pt bold #112233 center indent 10pt; body: italic justify indent 12pt; quote: 9pt -->\n\n# R\n\nText\n\n> citat\n", "x", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn document_with_all_features_makes_a_pdf() {
        let w = write(FULL, "Allt", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF"));
        let w = write("<div align=\"justify\">\n\nEn <span style=\"background-color: #FF0\">gul</span> rad.\n\n</div>\n", "x", &|_| None).unwrap();
        assert!(w.bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn long_documents_get_several_pages() {
        let md: String = (0..400).map(|i| format!("Stycke {i} med lite text som fyller sidan.\n\n")).collect();
        let w = write(&md, "Lång", &|_| None).unwrap();
        let s = String::from_utf8_lossy(&w.bytes);
        assert!(s.matches("/Type /Page").count() > 3 || s.matches("/Type/Page").count() > 3);
    }

    // --- sidformat, sidhuvud/sidfot, bildstorlekar ---------------------------------

    fn laid_out<'a>(md: &str, resolve: &'a dyn Fn(&str) -> Option<DocImage>) -> (Layout<'a>, DocDefaults) {
        let d = doc_defaults(md);
        let mut lay = Layout::new(resolve, &d);
        lay.new_page();
        lay.run(&parse_blocks(md));
        (lay, d)
    }

    fn texts(ops: &[Op]) -> Vec<(f32, f32, String)> {
        ops.iter()
            .filter_map(|o| match o {
                Op::Text { x, y, text, .. } => Some((*x, *y, text.clone())),
                _ => None,
            })
            .collect()
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(w, h))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn images_of(lay: &Layout) -> Vec<(f32, f32, f32, f32)> {
        lay.pages
            .iter()
            .flat_map(|p| p.ops.iter())
            .filter_map(|o| match o {
                Op::Image { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn page_size_and_margins_follow_the_comment() {
        let md = "<!-- kladd: page-size: letter landscape; page-margin: 36pt 50pt 40pt 60pt -->\n\nHej\n";
        let (lay, _) = laid_out(md, &|_| None);
        assert!((lay.pw - 792.0).abs() < 0.5 && (lay.ph - 612.0).abs() < 0.5, "{} x {}", lay.pw, lay.ph);
        assert_eq!((lay.mt, lay.mb, lay.ml), (36.0, 40.0, 60.0));
        assert!((lay.cw - (792.0 - 110.0)).abs() < 0.5);
        let t = texts(&lay.pages[0].ops);
        assert!((t[0].0 - 60.0).abs() < 0.01, "texten börjar i vänstermarginalen");
        assert!(write(md, "x", &|_| None).unwrap().bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn header_and_footer_on_every_page_with_page_numbers() {
        let md = "<!-- kladd: header: \"Rapport\"; header-align: right; footer: \"Sida {sida} av {sidor}\"; footer-align: center -->\n\n".to_owned()
            + &(0..150).map(|i| format!("Stycke {i} med lite text som fyller sidan.\n\n")).collect::<String>();
        let (mut lay, d) = laid_out(&md, &|_| None);
        let total = lay.pages.len();
        assert!(total >= 3, "{total} sidor");
        for p in 1..=total {
            let ops = lay.header_footer(&d, &style::Fields::new(p, total, "x"));
            let t = texts(&ops);
            assert_eq!(t.len(), 2);
            assert_eq!(t[0].2, "Rapport");
            assert_eq!(t[1].2, format!("Sida {p} av {total}"));
            // Sidhuvudet börjar `DEFAULT_DIST` från överkanten och sidfoten slutar lika långt från nederkanten.
            let size = lay.body * 0.9;
            assert!((t[0].1 - (crate::hf::DEFAULT_DIST + size * 0.95)).abs() < 0.01);
            assert!(t[1].1 < lay.ph - crate::hf::DEFAULT_DIST + 0.01 && t[1].1 > lay.ph - lay.mb);
            // Sidfoten är centrerad, sidhuvudet högerställt.
            let fw = lay.m.width(lay.base, lay.body * 0.9, &t[1].2);
            assert!((t[1].0 + fw / 2.0 - (lay.ml + lay.cw / 2.0)).abs() < 0.01);
            let hw = lay.m.width(lay.base, lay.body * 0.9, "Rapport");
            assert!((t[0].0 + hw - (lay.ml + lay.cw)).abs() < 0.01);
        }
        assert!(write(&md, "x", &|_| None).unwrap().bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn header_parts_fields_and_first_page_in_pdf() {
        let md = "<!-- kladd: header: \"Rapport|{titel}|{datum}\"; footer: \"{sida}/{sidor}\"; footer-align: right; header-first: \"\"; footer-first: \"Omslag\" -->\n\n".to_owned()
            + &(0..150).map(|i| format!("Stycke {i} med lite text som fyller sidan.\n\n")).collect::<String>();
        let (mut lay, d) = laid_out(&md, &|_| None);
        let total = lay.pages.len();
        assert!(total >= 3);
        let f = |p: usize| style::Fields { page: p, pages: total, title: "Min rapport".into(), file: "Min rapport.pdf".into(), date: "2026-11-13".into(), sec_pages: total, fmt: Default::default() };
        // Första sidan: tomt sidhuvud och egen sidfot.
        let t1 = texts(&lay.header_footer(&d, &f(1)));
        assert_eq!(t1.iter().map(|t| t.2.as_str()).collect::<Vec<_>>(), ["Omslag"]);
        // Övriga sidor: tre delar i sidhuvudet (vänster, mitten, höger) och sidfot med fält.
        let t2 = texts(&lay.header_footer(&d, &f(2)));
        assert_eq!(t2.iter().map(|t| t.2.as_str()).collect::<Vec<_>>(), ["Rapport", "Min rapport", "2026-11-13", format!("2/{total}").as_str()]);
        let (l, c, r) = (&t2[0], &t2[1], &t2[2]);
        assert!((l.0 - lay.ml).abs() < 0.01);
        let cw = lay.m.width(lay.base, lay.body * 0.9, "Min rapport");
        assert!((c.0 + cw / 2.0 - (lay.ml + lay.cw / 2.0)).abs() < 0.01);
        let rw = lay.m.width(lay.base, lay.body * 0.9, "2026-11-13");
        assert!((r.0 + rw - (lay.ml + lay.cw)).abs() < 0.01);
        assert!(write(&md, "Min rapport", &|_| None).unwrap().bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn no_header_means_no_header_text() {
        let (mut lay, d) = laid_out("Hej\n", &|_| None);
        assert!(texts(&lay.header_footer(&d, &style::Fields::new(1, 1, "x"))).is_empty());
    }

    #[test]
    fn img_width_sets_size_keeping_aspect_and_clamps() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None, svg: None });
        let (lay, _) = laid_out("<img src=\"bild:a.png\" alt=\"A\" width=\"192\">\n", &resolve);
        let im = images_of(&lay);
        assert_eq!(im.len(), 1);
        assert!((im[0].2 - 144.0).abs() < 0.01 && (im[0].3 - 72.0).abs() < 0.01, "{im:?}");
        // Både bredd och höjd.
        let (lay, _) = laid_out("<img src=\"a\" width=\"96\" height=\"96\">\n", &resolve);
        let im = images_of(&lay);
        assert!((im[0].2 - 72.0).abs() < 0.01 && (im[0].3 - 72.0).abs() < 0.01);
        // För bred bild krymps till textbredden.
        let (lay, _) = laid_out("<img src=\"a\" width=\"5000\">\n", &resolve);
        let im = images_of(&lay);
        assert!((im[0].2 - lay.cw).abs() < 0.01 && (im[0].3 - lay.cw / 2.0).abs() < 0.01);
        // Utan storlek: som förut (96 dpi).
        let (lay, _) = laid_out("![A](a)\n", &resolve);
        let im = images_of(&lay);
        assert!((im[0].2 - 75.0).abs() < 0.01);
    }

    #[test]
    fn div_align_places_an_image() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None, svg: None });
        let md = "<div align=\"center\">\n\n<img src=\"a\" width=\"96\">\n\n</div>\n\n<div align=\"right\">\n\n<img src=\"a\" width=\"96\">\n\n</div>\n";
        let (lay, _) = laid_out(md, &resolve);
        let im = images_of(&lay);
        assert_eq!(im.len(), 2);
        assert!((im[0].0 - (lay.ml + (lay.cw - 72.0) / 2.0)).abs() < 0.01, "{im:?}");
        assert!((im[1].0 - (lay.ml + lay.cw - 72.0)).abs() < 0.01);
    }

    const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.";

    #[test]
    fn floats_wrap_text_line_by_line() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None, svg: None });
        for (side, tag) in [(Float::Left, "left"), (Float::Right, "right")] {
            let md = format!("<img src=\"a\" width=\"192\" align=\"{tag}\">\n\n{LOREM}\n\n{LOREM}\n\n{LOREM}\n\n{LOREM}\n");
            let (mut lay, _) = laid_out(&md, &resolve);
            let im = images_of(&lay);
            assert_eq!(im.len(), 1);
            let (ix, iy, iw, ih) = im[0];
            let want_x = if side == Float::Left { lay.ml } else { lay.ml + lay.cw - iw };
            assert!((ix - want_x).abs() < 0.01, "{side:?}: bild vid {ix}");
            let body = lay.body;
            let (mut beside, mut below) = (0, 0);
            for (x, y, t) in texts(&lay.pages[0].ops) {
                let w = lay.m.width(Face::Sans, body, &t);
                if y < iy + ih {
                    beside += 1;
                    let (lo, hi) = if side == Float::Left { (lay.ml + iw + FLOAT_GAP, lay.ml + lay.cw) } else { (lay.ml, lay.ml + lay.cw - iw - FLOAT_GAP) };
                    assert!(x >= lo - 0.01 && x + w <= hi + 0.5, "{side:?}: rad vid {x}..{} (y {y}) utanför {lo}..{hi}", x + w);
                } else {
                    below += 1;
                    assert!((x - lay.ml).abs() < 0.01, "{side:?}: rad under bilden vid {x}");
                }
            }
            assert!(beside >= 3 && below >= 3, "{side:?}: {beside} bredvid, {below} under");
            // Bilden tar ingen egen höjd: första textraden ligger överst.
            let first = texts(&lay.pages[0].ops).first().map(|t| t.1).unwrap();
            assert!(first < lay.mt + 20.0, "första raden y = {first}");
        }
    }

    #[test]
    fn float_that_does_not_fit_moves_to_the_next_page() {
        let data = png(100, 100);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None, svg: None });
        // Fyll sidan nästan helt, sedan en bild på 150 pt som inte ryms.
        let filler: Vec<String> = (0..12).map(|_| LOREM.to_owned()).collect();
        let md = format!("{}\n\n<img src=\"a\" width=\"200\" align=\"left\">\n\n{LOREM}\n\n{LOREM}\n", filler.join("\n\n"));
        let (lay, _) = laid_out(&md, &resolve);
        assert!(lay.pages.len() >= 2);
        let on_page = |k: usize| lay.pages[k].ops.iter().filter(|o| matches!(o, Op::Image { .. })).count();
        assert_eq!((on_page(0), on_page(1)), (0, 1));
        // Texten efter bilden står bredvid den på sida 2.
        let t = texts(&lay.pages[1].ops);
        assert!(t.iter().any(|(x, _, _)| *x > lay.ml + 100.0), "{t:?}");
    }

    #[test]
    fn anchored_images_are_placed_like_in_the_document_mode() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None, svg: None });
        // Mot sidan: absolut plats (96 px = 72 pt från sidans kant), text påverkas inte (bakom).
        let md = format!("<img src=\"a\" width=\"200\" hrel=\"page\" x=\"96\" vrel=\"page\" y=\"bottom\" wrap=\"behind\">\n\n{LOREM}\n");
        let (lay, _) = laid_out(&md, &resolve);
        let (ix, iy, iw, ih) = images_of(&lay)[0];
        assert!((ix - 72.0).abs() < 0.01 && (iy + ih - lay.ph).abs() < 0.01 && (iw - 150.0).abs() < 0.01, "{ix} {iy} {iw} {ih}");
        assert!(texts(&lay.pages[0].ops).iter().all(|(x, _, _)| (*x - lay.ml).abs() < 0.01));
        // Bakom texten: bilden ritas först.
        assert!(matches!(lay.pages[0].ops[0], Op::Image { .. }));
        // Över och under: texten börjar under bilden.
        let md = format!("<img src=\"a\" width=\"200\" x=\"center\" y=\"0\" wrap=\"top\">\n\n{LOREM}\n");
        let (lay, _) = laid_out(&md, &resolve);
        let (ix, iy, iw, ih) = images_of(&lay)[0];
        assert!((ix - (lay.ml + (lay.cw - iw) / 2.0)).abs() < 0.01);
        assert!(texts(&lay.pages[0].ops).iter().all(|(_, y, _)| *y > iy + ih - 1.0));
        // Kvadratiskt vid högerkanten: raderna slutar före bilden.
        let md = format!("<img src=\"a\" width=\"200\" x=\"right\" y=\"0\" dist=\"0 0 0 12\">\n\n{LOREM}\n");
        let (lay, _) = laid_out(&md, &resolve);
        let (ix, _, _, _) = images_of(&lay)[0];
        assert!((ix - (lay.ml + lay.cw - 150.0)).abs() < 0.01);
    }

    #[test]
    fn cropped_image_keeps_the_visible_proportions() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None, svg: None });
        let md = "<img src=\"a\" width=\"100\" crop=\"0 0 50 0\">\n";
        let (lay, _) = laid_out(md, &resolve);
        let (_, _, w, h) = images_of(&lay)[0];
        // Halva bredden synlig: visningsrutan 75 x 37,5 pt blir 75 x 75 vid samma höjdförhållande.
        assert!((w - 75.0).abs() < 0.01 && (h - 75.0 * 50.0 / 50.0).abs() < 0.01, "{w} x {h}");
    }

    #[test]
    fn float_without_room_is_a_plain_block() {
        let data = png(100, 50);
        let resolve = |_: &str| Some(DocImage { bytes: data.clone(), extent: None, svg: None });
        let md = format!("<img src=\"a\" width=\"700\" align=\"left\">\n\n{LOREM}\n");
        let (lay, _) = laid_out(&md, &resolve);
        let (_, iy, _, ih) = images_of(&lay)[0];
        assert!(texts(&lay.pages[0].ops).iter().all(|(x, y, _)| (*x - lay.ml).abs() < 0.01 && *y > iy + ih - 1.0));
    }

    #[test]
    fn k_markers_are_never_printed() {
        for md in ["Före <!--k:1--> efter\n", "<!--k:12-->\n\nText\n", "<!--k:3--> rad\n"] {
            let (lay, _) = laid_out(md, &|_| None);
            let all: String = texts(&lay.pages[0].ops).iter().map(|t| t.2.clone()).collect::<Vec<_>>().join(" ");
            assert!(!all.contains("<!--") && !all.contains("k:"), "{md:?} -> {all:?}");
        }
        assert_eq!(strip_markers("a<!--k:1-->b<!-- x -->"), "ab<!-- x -->");
    }
}

/// `KLADD_MD_IN=a.md KLADD_PDF_OUT=b.pdf cargo test pdf_manual -- --ignored --nocapture`
#[cfg(test)]
mod manual {
    #[test]
    #[ignore]
    fn pdf_manual() {
        let (Ok(i), Ok(o)) = (std::env::var("KLADD_MD_IN"), std::env::var("KLADD_PDF_OUT")) else { return };
        let md = std::fs::read_to_string(&i).unwrap();
        let dir = std::path::Path::new(&i).parent().unwrap().to_path_buf();
        let w = super::write(&md, "Prov", &|src| {
            std::fs::read(dir.join(src)).ok().map(|bytes| crate::docx::Image { bytes, extent: None, svg: None })
        })
        .unwrap();
        println!("saknade bilder: {:?}", w.missing_images);
        std::fs::write(o, w.bytes).unwrap();
    }
}

#[cfg(test)]
#[path = "pdf_fn_tests.rs"]
mod fn_tests;

#[cfg(test)]
#[path = "pdf_section_tests.rs"]
mod section_tests;

#[cfg(test)]
#[path = "pdf_hf_tests.rs"]
mod hf_tests;
