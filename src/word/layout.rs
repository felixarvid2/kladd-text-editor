//! Bygger en `LayoutJob` som får Markdown-texten i en `TextEdit` att se ut som ett Word-dokument.
//!
//! Jobbets text är exakt Markdown-strängen (TextEdit-bufferten), och sektionerna täcker den
//! utan glapp. Markup (`**`, `# `, `](url)` …) döljs genom att göras transparent och 1 px
//! hög utom där markören står (`Params::reveal`), där den visas dämpad.
#![allow(dead_code)]

use std::collections::HashMap;
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

fn hidden_format(family: FontFamily) -> TextFormat {
    TextFormat {
        font_id: FontId::new(1.0, family),
        color: Color32::TRANSPARENT,
        line_height: None,
        ..Default::default()
    }
}

/// Format för en bit. `hide` = bitens markup ska döljas.
fn piece_format(pc: &Piece, hide: bool, p: &Params) -> TextFormat {
    let pal = p.palette;
    let zoom = p.zoom;
    let mono = pc.code || matches!(pc.block, Block::Code | Block::Table);
    let heading = matches!(pc.block, Block::Heading(_));
    let size = px(zoom, block_size_pt(pc.block, mono));
    let lh = if heading { size * 1.3 } else { size * 1.4 };
    let family_plain = if mono { FontFamily::Monospace } else { theme::doc_font(false, false) };

    if pc.markup {
        if hide {
            // Punktmarkören behåller brödtextens storlek så att indraget består.
            if pc.list_marker {
                let body = px(zoom, 11.0);
                return TextFormat {
                    font_id: FontId::new(body, theme::doc_font(false, false)),
                    color: Color32::TRANSPARENT,
                    line_height: Some(body * 1.4),
                    ..Default::default()
                };
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

    let bold = pc.bold || heading;
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

pub fn layout_job(text: &str, a: &Analysis, p: &Params) -> LayoutJob {
    let mut secs: Vec<(Range<usize>, TextFormat)> = Vec::new();
    let bytes = text.as_bytes();

    for pc in &a.pieces {
        let r = pc.range.clone();
        if r.is_empty() {
            continue;
        }
        let revealed = intersects(&r, &p.reveal);
        // Tabellens pipor och avgränsarrad döljs aldrig.
        let hide = !revealed && pc.block != Block::Table;

        if let Some(img) = &pc.image {
            if hide {
                if let Some(h) = (p.image_size)(&img.src).map(|s| s.y) {
                    // Dold text, men första tecknet reserverar bildens höjd.
                    let first = text[r.clone()].chars().next().map_or(1, char::len_utf8);
                    let fam = theme::doc_font(false, false);
                    let mut f1 = hidden_format(fam.clone());
                    f1.line_height = Some(h);
                    secs.push((r.start..r.start + first, f1));
                    if r.start + first < r.end {
                        secs.push((r.start + first..r.end, hidden_format(fam)));
                    }
                } else {
                    // Ingen bild att visa: visa alternativtexten.
                    let size = px(p.zoom, 11.0);
                    secs.push((
                        r,
                        TextFormat {
                            font_id: FontId::new(size, theme::doc_font(false, true)),
                            color: p.palette.page_muted,
                            line_height: Some(size * 1.4),
                            ..Default::default()
                        },
                    ));
                }
            } else {
                let mut f = piece_format(pc, false, p);
                f.color = p.palette.page_markup;
                f.line_height = Some(px(p.zoom, 11.0) * 1.4);
                secs.push((r, f));
            }
            continue;
        }

        let fmt = piece_format(pc, hide, p);
        // Tomradsbrytningar blir egna sektioner med 8 pt radhöjd.
        let mut start = r.start;
        for (off, c) in text[r.clone()].char_indices() {
            let i = r.start + off;
            if c == '\n' && (i == 0 || bytes[i - 1] == b'\n') {
                if start < i {
                    secs.push((start..i, fmt.clone()));
                }
                secs.push((i..i + 1, spacing_format(p.zoom)));
                start = i + 1;
            }
        }
        if start < r.end {
            secs.push((start..r.end, fmt));
        }
    }

    // Slå ihop angränsande sektioner med samma format.
    let mut job = LayoutJob { text: text.to_owned(), ..Default::default() };
    for (range, format) in secs {
        if let Some(last) = job.sections.last_mut() {
            if last.format == format && last.byte_range.end.0 == range.start {
                last.byte_range.end = ByteIndex(range.end);
                continue;
            }
        }
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(range.start)..ByteIndex(range.end),
            format,
        });
    }

    if !p.query.is_empty() {
        let matches = highlight::find_all(text, p.query, p.case_sensitive);
        if !matches.is_empty() {
            highlight::apply_backgrounds(&mut job, &matches, p.palette, p.current_match);
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
        assert_eq!(s.format.font_id.size, 1.0);
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
    fn bullet_keeps_body_size() {
        let job = build("- punkt\n", 0..0, "");
        let s = sec_at(&job, 0);
        assert_eq!(s.format.font_id.size, px(1.0, 11.0));
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
        };
        let job = layout_job(t, &markup::analyze(t), &p);
        assert_eq!(sec_at(&job, 0).format.color, pal.page_muted);
        // Visad: ingen reserverad höjd.
        let job = build(t, 0..t.len(), "");
        assert_eq!(sec_at(&job, 0).format.color, pal.page_markup);
        assert_ne!(sec_at(&job, 0).format.line_height, Some(50.0));
    }

    #[test]
    fn table_markup_never_hidden() {
        let t = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let job = build(t, 0..0, "");
        let s = sec_at(&job, 0);
        assert_eq!(s.format.font_id.family, FontFamily::Monospace);
        assert_ne!(s.format.color, Color32::TRANSPARENT);
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
