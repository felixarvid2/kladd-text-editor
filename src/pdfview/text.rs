//! Textlagret för en PDF-sida: glyfer med rutor, läsordning, rader, sökning och markering.
//!
//! Alla koordinater är visningspunkter (origo uppe till vänster, y nedåt, 1 enhet = 1 pt), samma
//! rum som motorns `page_size`. Glyferna lagras i läsordning (rad för rad uppifrån och ned, inom
//! raden från vänster till höger), så glyfindex kan användas direkt som markeringsintervall.
//!
//! Begränsningar: lodrät skrift och höger-till-vänster-skrift grupperas inte särskilt (varje tecken
//! kan hamna på egen rad respektive komma i omvänd ordning); glyfrutorna använder den vanliga
//! gissningen 800/-200 av em-rutan eftersom hayro inte avslöjar teckensnittets ascent/descent.

use std::ops::Range;

use egui::{Pos2, Rect, pos2};
use hayro::hayro_interpret::font::{Glyph as PdfGlyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::kurbo::{BezPath, Rect as KRect};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache,
    InterpreterSettings, SoftMask, TransformExt, interpret_page,
};
use hayro::hayro_syntax::page::Page;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Ingen glyf: markerar tecken i texten som inte hör till någon glyf (infogade mellanrum/radbrytningar).
const NO_GLYPH: u32 = u32::MAX;
/// Luckan mellan två glyfer (i andelar av teckenhöjden) som räknas som ett ordmellanrum.
const SPACE_GAP: f32 = 0.25;

#[derive(Clone, Debug)]
pub struct Glyph {
    pub text: String,
    pub rect: Rect,
}

/// En rad: glyfintervall och omslutande ruta.
#[derive(Clone, Debug)]
struct Line {
    glyphs: Range<usize>,
    rect: Rect,
}

/// Text i en variant (skiftlägesokänslig/-känslig) med karta tillbaka till originaltextens byte.
#[derive(Default)]
struct Haystack {
    text: String,
    /// För varje byte i `text`: byteindex till motsvarande tecken i originaltexten.
    orig: Vec<u32>,
}

#[derive(Default)]
pub struct PageText {
    pub glyphs: Vec<Glyph>,
    lines: Vec<Line>,
    /// Sant om ett infogat mellanrum (ordgräns) ligger före glyfen.
    gap_before: Vec<bool>,
    text: String,
    /// Byte i `text` → glyfindex (eller `NO_GLYPH`).
    byte_glyph: Vec<u32>,
    /// Glyfindex → byteintervall i `text`.
    glyph_bytes: Vec<Range<usize>>,
    folded: Haystack,
    exact: Haystack,
}

fn is_ws(g: &Glyph) -> bool {
    g.text.chars().all(char::is_whitespace)
}

fn is_wordish(g: &Glyph) -> bool {
    g.text.chars().all(char::is_alphanumeric)
}

impl PageText {
    /// Bygger textlagret av glyfer i valfri (t.ex. ritordning). Glyferna sorteras till läsordning.
    pub fn new(raw: Vec<Glyph>) -> PageText {
        // Rensa glyfer utan text eller med ogiltig ruta. Styrtecken blir mellanslag.
        let mut input: Vec<Glyph> = raw
            .into_iter()
            .filter_map(|mut g| {
                let r = g.rect;
                if !(r.min.x.is_finite() && r.min.y.is_finite() && r.max.x.is_finite() && r.max.y.is_finite()) {
                    return None;
                }
                if g.text.is_empty() {
                    return None;
                }
                if g.text.chars().any(char::is_control) {
                    g.text = g.text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
                }
                Some(g)
            })
            .collect();

        // Gruppera till rader i strömordning: en glyf läggs på en av de senaste raderna om den
        // överlappar lodrätt och fortsätter åt höger. Dubbelritade glyfer (falsk fetstil) slängs.
        struct Work {
            glyphs: Vec<Glyph>,
            // Referensspann (den högsta glyfens) och högerkant.
            y0: f32,
            y1: f32,
            x1: f32,
        }
        let mut works: Vec<Work> = Vec::new();
        'glyph: for g in input.drain(..) {
            let h = g.rect.height().max(0.01);
            let start = works.len().saturating_sub(4);
            for w in works[start..].iter_mut().rev() {
                let overlap = g.rect.max.y.min(w.y1) - g.rect.min.y.max(w.y0);
                let min_h = h.min(w.y1 - w.y0).max(0.01);
                if overlap < 0.5 * min_h {
                    continue;
                }
                if g.rect.min.x >= w.x1 - 0.5 * h {
                    w.x1 = w.x1.max(g.rect.max.x);
                    if h > w.y1 - w.y0 {
                        w.y0 = g.rect.min.y;
                        w.y1 = g.rect.max.y;
                    }
                    w.glyphs.push(g);
                    continue 'glyph;
                }
                // Går bakåt: dubblett av en glyf som redan finns?
                let dup = w.glyphs.iter().rev().take(64).any(|o| {
                    o.text == g.text
                        && (o.rect.center().x - g.rect.center().x).abs() < 0.2 * h
                        && (o.rect.center().y - g.rect.center().y).abs() < 0.2 * h
                });
                if dup {
                    continue 'glyph;
                }
            }
            works.push(Work { y0: g.rect.min.y, y1: g.rect.max.y, x1: g.rect.max.x, glyphs: vec![g] });
        }

        // Rader uppifrån och ned (stabilt, så rader på samma höjd behåller strömordningen).
        works.sort_by(|a, b| {
            let ca = (a.y0 + a.y1) * 0.5;
            let cb = (b.y0 + b.y1) * 0.5;
            ca.total_cmp(&cb)
        });

        let mut pt = PageText::default();
        for mut w in works {
            w.glyphs.sort_by(|a, b| a.rect.min.x.total_cmp(&b.rect.min.x));
            let first = pt.glyphs.len();
            if first > 0 {
                pt.push_synthetic("\n");
            }
            let mut prev: Option<Rect> = None;
            let mut prev_ws = true;
            let mut rect = Rect::NOTHING;
            for g in w.glyphs {
                let ws = is_ws(&g);
                let gap = match prev {
                    Some(p) => g.rect.min.x - p.max.x > SPACE_GAP * g.rect.height().max(p.height()),
                    None => false,
                };
                let space = gap && !ws && !prev_ws;
                if space {
                    pt.push_synthetic(" ");
                }
                let idx = pt.glyphs.len();
                let b0 = pt.text.len();
                pt.text.push_str(&g.text);
                pt.byte_glyph.resize(pt.text.len(), idx as u32);
                pt.glyph_bytes.push(b0..pt.text.len());
                pt.gap_before.push(space);
                rect = rect.union(g.rect);
                prev = Some(g.rect);
                prev_ws = ws;
                pt.glyphs.push(g);
            }
            pt.lines.push(Line { glyphs: first..pt.glyphs.len(), rect });
        }
        pt.folded = build_haystack(&pt.text, true);
        pt.exact = build_haystack(&pt.text, false);
        pt
    }

    fn push_synthetic(&mut self, s: &str) {
        self.text.push_str(s);
        self.byte_glyph.resize(self.text.len(), NO_GLYPH);
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Hela sidans text i läsordning: " " mellan ord, "\n" mellan rader.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Söker `query`. Utan `case_sensitive` ignoreras versaler och diakritiska tecken (é = e).
    /// Radbrytningar i sidtexten matchar mellanslag i frågan. Returnerar glyfintervall.
    pub fn find(&self, query: &str, case_sensitive: bool, whole_word: bool) -> Vec<Range<usize>> {
        if query.is_empty() || self.glyphs.is_empty() {
            return Vec::new();
        }
        let hay = if case_sensitive { &self.exact } else { &self.folded };
        let q = build_haystack(query, !case_sensitive).text;
        if q.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(pos) = hay.text[from..].find(&q) {
            let s = from + pos;
            let e = s + q.len();
            // Nästa försök börjar ett tecken in i träffen (tillåter överlappande träffar inte; hoppa förbi).
            from = e;
            if whole_word && !self.word_bounds_ok(hay, s, e) {
                // Fortsätt strax efter starten så att en senare träff inte missas.
                from = s + hay.text[s..].chars().next().map_or(1, char::len_utf8);
                continue;
            }
            let o0 = hay.orig[s] as usize;
            let o1 = {
                let last = hay.orig[e - 1] as usize;
                last + self.text[last..].chars().next().map_or(1, char::len_utf8)
            };
            let g0 = self.byte_glyph[o0..o1].iter().copied().find(|&g| g != NO_GLYPH);
            let g1 = self.byte_glyph[o0..o1].iter().copied().rev().find(|&g| g != NO_GLYPH);
            if let (Some(a), Some(b)) = (g0, g1) {
                out.push(a as usize..b as usize + 1);
            }
        }
        out
    }

    fn word_bounds_ok(&self, hay: &Haystack, s: usize, e: usize) -> bool {
        let o0 = hay.orig[s] as usize;
        let o1 = {
            let last = hay.orig[e - 1] as usize;
            last + self.text[last..].chars().next().map_or(1, char::len_utf8)
        };
        let before = self.text[..o0].chars().next_back();
        let after = self.text[o1..].chars().next();
        let word = |c: char| c.is_alphanumeric() || c == '_';
        !before.is_some_and(word) && !after.is_some_and(word)
    }

    /// Rutor för ett glyfintervall, sammanslagna per rad.
    pub fn rects(&self, glyphs: Range<usize>) -> Vec<Rect> {
        let end = glyphs.end.min(self.glyphs.len());
        let mut out = Vec::new();
        for line in &self.lines {
            let lo = glyphs.start.max(line.glyphs.start);
            let hi = end.min(line.glyphs.end);
            if lo < hi {
                out.push(self.glyphs[lo..hi].iter().fold(Rect::NOTHING, |r, g| r.union(g.rect)));
            }
        }
        out
    }

    /// Närmaste glyf: först raden närmast punkten i höjdled, sedan glyfen närmast i sidled.
    pub fn glyph_at(&self, p: Pos2) -> Option<usize> {
        fn dist(a: f32, b: f32, v: f32) -> f32 {
            if v < a { a - v } else if v > b { v - b } else { 0.0 }
        }
        let line = self.lines.iter().min_by(|a, b| {
            let da = (dist(a.rect.min.y, a.rect.max.y, p.y), (a.rect.center().y - p.y).abs());
            let db = (dist(b.rect.min.y, b.rect.max.y, p.y), (b.rect.center().y - p.y).abs());
            da.0.total_cmp(&db.0).then(da.1.total_cmp(&db.1))
        })?;
        line.glyphs
            .clone()
            .min_by(|&a, &b| {
                let da = dist(self.glyphs[a].rect.min.x, self.glyphs[a].rect.max.x, p.x);
                let db = dist(self.glyphs[b].rect.min.x, self.glyphs[b].rect.max.x, p.x);
                da.total_cmp(&db)
            })
    }

    fn line_of(&self, glyph: usize) -> Option<&Line> {
        self.lines.iter().find(|l| l.glyphs.contains(&glyph))
    }

    /// Ordet runt glyfen: en följd av bokstäver/siffror utan ordmellanrum. Annat tecken: bara det.
    pub fn word_at(&self, glyph: usize) -> Range<usize> {
        let Some(line) = self.line_of(glyph) else { return glyph..glyph };
        if !is_wordish(&self.glyphs[glyph]) {
            return glyph..glyph + 1;
        }
        let mut s = glyph;
        while s > line.glyphs.start && !self.gap_before[s] && is_wordish(&self.glyphs[s - 1]) {
            s -= 1;
        }
        let mut e = glyph + 1;
        while e < line.glyphs.end && !self.gap_before[e] && is_wordish(&self.glyphs[e]) {
            e += 1;
        }
        s..e
    }

    /// Hela raden som glyfen ligger på.
    pub fn line_at(&self, glyph: usize) -> Range<usize> {
        self.line_of(glyph).map_or(glyph..glyph, |l| l.glyphs.clone())
    }

    /// Texten för ett glyfintervall, med mellanslag och radbrytningar mellan glyferna.
    pub fn text_of(&self, glyphs: Range<usize>) -> String {
        let end = glyphs.end.min(self.glyphs.len());
        if glyphs.start >= end {
            return String::new();
        }
        self.text[self.glyph_bytes[glyphs.start].start..self.glyph_bytes[end - 1].end].to_owned()
    }
}

/// Bygger söktext (med eller utan vikning till gemener utan diakritiska tecken) och byte-karta.
/// Radbrytning behandlas som mellanslag.
fn build_haystack(src: &str, fold: bool) -> Haystack {
    let mut h = Haystack::default();
    for (i, c) in src.char_indices() {
        let c = if c == '\n' { ' ' } else { c };
        let before = h.text.len();
        if fold {
            for d in std::iter::once(c).nfd().filter(|d| !is_combining_mark(*d)) {
                h.text.extend(d.to_lowercase());
            }
        } else {
            h.text.push(c);
        }
        h.orig.resize(h.text.len(), i as u32);
        debug_assert!(h.text.len() >= before);
    }
    h
}

// ---------------------------------------------------------------------------------------------
// Utdragning ur hayro

/// Enhet som struntar i all grafik och samlar glyferna med rutor i visningspunkter.
struct TextDevice {
    out: Vec<Glyph>,
}

impl<'a> Device<'a> for TextDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _mode: &DrawMode) {
        for g in run.glyphs() {
            let Some(u) = g.as_unicode() else { continue };
            let text = match u {
                BfString::Char(c) => c.to_string(),
                BfString::String(s) => s,
            };
            let adv = match &**g {
                PdfGlyph::Outline(o) => o.advance_width().unwrap_or(500.0) as f64,
                PdfGlyph::Type3(t) => t.advance_width().unwrap_or(500.0) as f64,
            };
            // Glyfrummet har 1000 enheter per em: ruta från -200 till 800 (gissad ascent/descent).
            let r = KRect::new(0.0, -200.0, adv.max(1.0), 800.0);
            let bb = (props.transform * g.transform()).transform_rect_bbox(r);
            self.out.push(Glyph {
                text,
                rect: Rect::from_min_max(pos2(bb.x0 as f32, bb.y0 as f32), pos2(bb.x1 as f32, bb.y1 as f32)),
            });
        }
    }
}

/// Läser ut textlagret för en sida. Kan panika på trasiga filer: anropa i `catch_unwind`.
pub fn extract_page<'a>(page: &Page<'a>, cache: &InterpreterCache<'a>) -> PageText {
    let (w, h) = page.render_dimensions();
    let mut ctx = Context::new(
        page.initial_transform(true).to_kurbo(),
        KRect::new(0.0, 0.0, w as f64, h as f64),
        cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    let mut dev = TextDevice { out: Vec::new() };
    interpret_page(page, &mut ctx, &mut dev);
    PageText::new(dev.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(t: &str, x: f32, y: f32, w: f32) -> Glyph {
        Glyph { text: t.into(), rect: Rect::from_min_size(pos2(x, y), egui::vec2(w, 10.0)) }
    }

    /// Lägger ut en sträng som glyfer (6 pt breda, 10 pt höga) från (x, y); mellanslag blir lucka.
    fn word_line(s: &str, x: f32, y: f32) -> Vec<Glyph> {
        let mut out = Vec::new();
        let mut cx = x;
        for c in s.chars() {
            if c != ' ' {
                out.push(g(&c.to_string(), cx, y, 6.0));
            }
            cx += 6.0;
        }
        out
    }

    fn sample() -> PageText {
        // Ritordningen är medvetet blandad: nedre raden först.
        let mut v = word_line("Andra raden", 10.0, 40.0);
        v.extend(word_line("Åsa åt äpplen", 10.0, 20.0));
        PageText::new(v)
    }

    #[test]
    fn lines_and_spaces() {
        let t = sample();
        assert_eq!(t.text(), "Åsa åt äpplen\nAndra raden");
        assert_eq!(t.line_count(), 2);
        assert_eq!(t.glyphs[0].text, "Å");
    }

    #[test]
    fn superscript_stays_on_line() {
        let mut v = word_line("x", 10.0, 20.0);
        v.push(Glyph { text: "2".into(), rect: Rect::from_min_size(pos2(16.0, 18.0), egui::vec2(4.0, 6.0)) });
        let t = PageText::new(v);
        assert_eq!(t.line_count(), 1);
        assert_eq!(t.text(), "x2");
    }

    #[test]
    fn duplicate_glyphs_dropped() {
        let mut v = word_line("Fet", 10.0, 20.0);
        v.extend(word_line("Fet", 10.5, 20.0));
        assert_eq!(PageText::new(v).text(), "Fet");
    }

    #[test]
    fn find_case_and_diacritics() {
        let t = sample();
        // "asa" matchar "Åsa" när diakritiska tecken och versaler ignoreras.
        let hits = t.find("asa", false, false);
        assert_eq!(hits, vec![0..3]);
        assert_eq!(t.text_of(hits[0].clone()), "Åsa");
        assert_eq!(t.find("ÄPPLEN", false, false), vec![5..11]);
        // Skiftlägeskänsligt: exakt tecken krävs.
        assert!(t.find("asa", true, false).is_empty());
        assert_eq!(t.find("Åsa", true, false), vec![0..3]);
        assert!(t.find("äpplen", true, false).len() == 1);
        assert!(t.find("saknas", false, false).is_empty());
        assert!(t.find("", false, false).is_empty());
    }

    #[test]
    fn find_whole_word() {
        let t = sample();
        assert_eq!(t.find("år", false, false).len(), 0);
        assert_eq!(t.find("ra", false, false).len(), 2); // "Andra", "raden"
        assert!(t.find("ra", false, true).is_empty());
        assert_eq!(t.find("åt", false, true).len(), 1);
        assert_eq!(t.find("raden", false, true).len(), 1);
    }

    #[test]
    fn find_across_line_break() {
        let t = sample();
        let hits = t.find("äpplen andra", false, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(t.text_of(hits[0].clone()), "äpplen\nAndra");
    }

    #[test]
    fn rects_merge_per_line() {
        let t = sample();
        // "äpplen" + "Andra": två rader → två rutor.
        let r = t.rects(7..15);
        assert_eq!(r.len(), 2);
        assert!(r[0].min.y < r[1].min.y);
        let one = t.rects(0..3);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].min.x, 10.0);
        assert_eq!(one[0].max.x, 28.0);
        assert!(t.rects(5..5).is_empty());
    }

    #[test]
    fn hit_testing() {
        let t = sample();
        assert_eq!(t.glyph_at(pos2(11.0, 25.0)), Some(0));
        // Långt under allt: sista raden, närmaste glyf i sidled.
        let last = t.glyph_at(pos2(1000.0, 500.0)).unwrap();
        assert_eq!(last, t.glyphs.len() - 1);
        // Mellan raderna: närmaste raden i höjdled.
        assert_eq!(t.line_at(t.glyph_at(pos2(12.0, 36.0)).unwrap()), 11..21);
        assert_eq!(t.text_of(t.word_at(1)), "Åsa");
        assert_eq!(t.text_of(t.word_at(3)), "åt");
        assert_eq!(t.text_of(t.line_at(4)), "Åsa åt äpplen");
        assert_eq!(t.text_of(0..0), "");
        assert!(PageText::default().glyph_at(pos2(0.0, 0.0)).is_none());
    }

    #[test]
    fn ligature_maps_to_one_glyph() {
        let t = PageText::new(vec![g("ﬁ", 0.0, 0.0, 8.0), g("n", 8.0, 0.0, 6.0)]);
        assert_eq!(t.find("n", false, false), vec![1..2]);
    }
}
