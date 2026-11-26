//! Sökträffar som ett lager ovanpå galleriet i dokumentläget.
//!
//! Träffarna är inte en del av utläggningen: då lades hela dokumentet om vid varje tangent med sökningen
//! öppen, och att bara byta aktuell träff gjorde det också. I stället ritar vyn rektanglar från galleriets
//! glyfer, på samma ställe och i samma färg som bakgrunden som tidigare lades i texten (epaints
//! `add_row_backgrounds`: en rektangel per följd glyfer med samma övre och nedre kant, en pixel vidgad).
//! Rektanglarna ritas bakom texten; där texten har en egen bakgrund (markerad text, inline-kod, kommentarer)
//! skulle den täcka dem, så där ritas de ovanpå.

use egui::{Galley, Rect, Shape, Vec2, emath::GuiRounding, epaint::text::PlacedRow, pos2};

use crate::theme::Palette;

/// Träffarnas rektanglar i skärmkoordinater.
#[derive(Default)]
pub struct Layer {
    /// Bakom texten (efter tabellernas och kodblockens fyllning).
    pub behind: Vec<Shape>,
    /// Ovanpå texten, där texten har en egen bakgrund.
    pub over: Vec<Shape>,
}

/// Rektanglarna för träffarna `matches` (`(start, slut, är_aktuell)` i byte i `text`, sorterade på start) på
/// raderna vars y ligger mellan `y0` och `y1` (galleriets koordinater). `at` flyttar dem till skärmen.
pub fn build(galley: &Galley, text: &str, matches: &[(usize, usize, bool)], at: Vec2, y0: f32, y1: f32, pal: &Palette) -> Layer {
    let mut out = Layer::default();
    let rows = &galley.rows;
    let first = rows.partition_point(|r| r.max_y() < y0);
    if matches.is_empty() || first >= rows.len() {
        return out;
    }
    // Tecknen före första synliga rad (galleriets tecken är textens tecken, se `ByteToChar` i vyn).
    let mut row_start: usize = rows[..first].iter().map(|r| r.char_count_including_newline().0).sum();
    let b0 = text.char_indices().nth(row_start).map_or(text.len(), |(b, _)| b);
    let mut conv = Conv { text, b: b0, c: row_start };
    let ppp = galley.pixels_per_point;
    let mut next = matches.partition_point(|m| m.1 <= b0);
    // Träffarna (i tecken) som kan nå aktuell rad.
    let mut live: Vec<(usize, usize, bool)> = Vec::new();
    for row in &rows[first..] {
        if row.min_y() > y1 {
            break;
        }
        let end = row_start + row.glyphs.len();
        let after = end + usize::from(row.ends_with_newline);
        while let Some(m) = matches.get(next) {
            let a = conv.at(m.0);
            if a >= after && a > row_start {
                break;
            }
            live.push((a, conv.at(m.1), m.2));
            next += 1;
        }
        live.retain(|m| m.1 > row_start);
        // Delarna av träffarna på raden; angränsande delar med samma färg blir en rektangel, som i epaint.
        let mut parts: Vec<(usize, usize, bool)> = Vec::new();
        for &(a, b, cur) in &live {
            let (s, e) = (a.max(row_start), b.min(end));
            if s >= e {
                continue;
            }
            match parts.last_mut() {
                Some(l) if l.1 >= s && l.2 == cur => l.1 = l.1.max(e),
                _ => parts.push((s, e, cur)),
            }
        }
        if !parts.is_empty() {
            let own = own_backgrounds(row);
            for (s, e, cur) in parts {
                let color = if cur { pal.match_current_bg } else { pal.match_bg };
                let g = &row.glyphs[s - row_start..e - row_start];
                let mut push = |a: Rect, b: Rect| {
                    let r = Rect::from_min_max(a.left_top(), pos2(b.right(), a.bottom()));
                    let over = own.iter().any(|&(x0, x1)| x0 < r.right() && r.left() < x1);
                    let r = r.expand(1.0).round_to_pixels(ppp).translate(row.pos.to_vec2() + at);
                    let list = if over { &mut out.over } else { &mut out.behind };
                    list.push(Shape::rect_filled(r, 0.0, color));
                };
                let mut start = g[0].logical_rect();
                let mut last = start;
                for gl in &g[1..] {
                    let r = gl.logical_rect();
                    if r.top() != start.top() || r.bottom() != start.bottom() {
                        push(start, last);
                        start = r;
                    }
                    last = r;
                }
                push(start, last);
            }
        }
        row_start = after;
    }
    out
}

/// Vågräta utsträckningar (radens koordinater) av radens egna bakgrunder: rektanglarna i radens nät före glyferna.
fn own_backgrounds(row: &PlacedRow) -> Vec<(f32, f32)> {
    let v = &row.row.visuals;
    let n = v.glyph_vertex_range.start.min(v.mesh.vertices.len());
    v.mesh.vertices[..n]
        .chunks(4)
        .filter(|q| q.iter().any(|p| p.color.a() > 0))
        .map(|q| {
            let xs = q.iter().map(|p| p.pos.x);
            (xs.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max))
        })
        .collect()
}

/// Byte till tecken, framåt (träffarna är sorterade) men med steg bakåt när en träff börjar före förra slutet.
struct Conv<'a> {
    text: &'a str,
    b: usize,
    c: usize,
}

impl Conv<'_> {
    fn at(&mut self, b: usize) -> usize {
        let b = b.min(self.text.len());
        if b < self.b {
            return self.c - self.text[b..self.b].chars().count();
        }
        self.c += self.text[self.b..b].chars().count();
        self.b = b;
        self.c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::bench::Rng;
    use crate::word::{layout, markup};
    use crate::{highlight, theme};
    use egui::{Color32, FontFamily, FontId};

    fn fam(_: Option<&str>, bold: bool, italic: bool) -> FontFamily {
        theme::doc_font(bold, italic)
    }

    /// Bakgrundsrektanglarna i raderna (galleriets koordinater): nätets första hörn före glyferna, fyra hörn per rektangel.
    fn quads(g: &Galley) -> Vec<(Rect, Color32)> {
        let mut v = Vec::new();
        for r in &g.rows {
            let vis = &r.row.visuals;
            for q in vis.mesh.vertices[..vis.glyph_vertex_range.start].chunks(4) {
                let mut rect = Rect::NOTHING;
                for p in q {
                    rect.extend_with(p.pos);
                }
                v.push((rect.translate(r.pos.to_vec2()), q[0].color));
            }
        }
        v
    }

    fn shape_rects(shapes: &[Shape]) -> Vec<(Rect, Color32)> {
        shapes
            .iter()
            .map(|s| match s {
                Shape::Rect(r) => (r.rect, r.fill),
                _ => panic!("oväntad form"),
            })
            .collect()
    }

    /// Lagret ritar exakt de rektanglar som bakgrunden i utläggningen gav: samma ställen, samma färger.
    /// Jämför mot den gamla vägen (träffarna som bakgrund i `LayoutJob`, `highlight::apply_backgrounds`).
    fn same_as_backgrounds(text: &str, matches: &[(usize, usize, bool)], width: f32) -> bool {
        let pal = theme::palette(false);
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let img = |_: &str| Some(egui::vec2(100.0, 50.0));
        let measure = |s: &str, _: FontId| s.chars().count() as f32 * 7.0;
        let p = layout::Params {
            palette: &pal,
            zoom: 1.0,
            reveal: 0..0,
            image_size: &img,
            measure: &measure,
            font_family: &fam,
            fonts_gen: 0,
        };
        let a = markup::analyze(text);
        let mut plain = layout::layout_job(text, &a, &p);
        plain.wrap.max_width = width;
        let mut old = plain.clone();
        highlight::apply_backgrounds(&mut old, matches, &pal);
        let mut res = None;
        let out = ctx.run_ui(Default::default(), |_| {
            let (g_old, g_plain) = ctx.fonts_mut(|f| (f.layout_job(old.clone()), f.layout_job(plain.clone())));
            res = Some((g_old, g_plain));
        });
        std::mem::forget(out);
        let (g_old, g_plain) = res.unwrap();
        // Delningen av sektioner kan flytta ett radbrott när en rad är precis full: jämför då inte (det är inte lagrets fel).
        let shape = |g: &Galley| g.rows.iter().map(|r| r.glyphs.len()).collect::<Vec<_>>();
        if shape(&g_old) != shape(&g_plain) {
            return false;
        }
        let layer = build(&g_plain, text, matches, Vec2::ZERO, -1000.0, 1.0e6, &pal);
        // Utan egen bakgrund i texten är allt bakom texten.
        assert!(layer.over.is_empty());
        let mut want = quads(&g_old);
        let mut got = shape_rects(&layer.behind);
        let key = |a: &(Rect, Color32)| (a.0.min.y.to_bits(), a.0.min.x.to_bits());
        want.sort_by_key(key);
        got.sort_by_key(key);
        assert_eq!(want.len(), got.len(), "{text:?} {matches:?}\n{want:?}\n{got:?}");
        for (w, g) in want.iter().zip(&got) {
            // Lodrätt exakt. Vågrätt några pixlar: att träffarna delade sektionerna i den gamla vägen flyttade
            // glyferna efter varje delningspunkt en delpixel (kerning över sektionsgränsen), så rektanglarna där
            // låg lite vid sidan av texten i förra utläggningen; lagret följer det nya galleriets glyfer exakt.
            let dy = (w.0.min.y - g.0.min.y).abs().max((w.0.max.y - g.0.max.y).abs());
            let dx = (w.0.min.x - g.0.min.x).abs().max((w.0.max.x - g.0.max.x).abs());
            assert!(dy < 0.01 && dx <= 4.0, "{w:?} != {g:?}");
            assert_eq!(w.1, g.1);
        }
        true
    }

    #[test]
    fn lagret_ritar_som_bakgrunden() {
        let t = "Ett kort stycke med ord och mer ord.\n\nEtt andra stycke med **fet ord** och *kursiv ord* som radbryts över flera rader när bredden är liten nog.\n\nSlut ord";
        let ms: Vec<(usize, usize, bool)> = highlight::find_all(t, "ord", false).into_iter().enumerate().map(|(i, m)| (m.0, m.1, i == 2)).collect();
        assert!(ms.len() >= 5);
        assert!(same_as_backgrounds(t, &ms, 160.0));
        assert!(same_as_backgrounds(t, &ms, 2000.0));
        // Angränsande träffar blir en rektangel, en träff över radbrytningen två.
        let adj = [(4, 7, false), (7, 10, false), (60, 140, true)];
        assert!(same_as_backgrounds(t, &adj, 160.0));
        assert!(same_as_backgrounds(t, &[], 160.0));
    }

    #[test]
    fn lagret_i_tabeller_och_listor() {
        let t = "| namn | antal |\n| --- | --- |\n| ord ett | 3 |\n| ord två | 4 |\n\n- punkt ord\n- ord punkt\n\n> citat ord\n";
        let ms: Vec<(usize, usize, bool)> = highlight::find_all(t, "ord", false).into_iter().map(|m| (m.0, m.1, false)).collect();
        assert!(ms.len() >= 5);
        assert!(same_as_backgrounds(t, &ms, 300.0));
    }

    #[test]
    fn lagret_slumpat() {
        let seeds: u64 = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
        let mut checked = 0;
        for seed in 0..seeds {
            let mut r = Rng(seed * 7919 + 3);
            let words = ["alfa", "beta", "gamma", "åäö", "dokument", "**fet**", "*kursiv*", "x", "ord"];
            let mut t = String::new();
            for _ in 0..(5 + r.below(30)) {
                for _ in 0..(1 + r.below(25)) {
                    t.push_str(words[r.below(words.len())]);
                    t.push(' ');
                }
                t.push_str(if r.below(3) == 0 { "\n" } else { "\n\n" });
            }
            let q = ["alfa", "ord", "a", "dokument"][r.below(4)];
            let mut ms: Vec<(usize, usize, bool)> = highlight::find_all(&t, q, false).into_iter().map(|m| (m.0, m.1, false)).collect();
            if !ms.is_empty() {
                let k = r.below(ms.len());
                ms[k].2 = true;
            }
            checked += usize::from(same_as_backgrounds(&t, &ms, 120.0 + r.below(400) as f32));
        }
        // De flesta fallen har samma radbrott i båda vägarna.
        assert!(checked * 4 >= seeds as usize * 3, "{checked} av {seeds}");
    }

    /// Träffar i text med egen bakgrund (markerad text) ritas ovanpå texten, de andra bakom.
    #[test]
    fn egen_bakgrund_ritas_under_lagret() {
        let pal = theme::palette(false);
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let t = "ord före <span style=\"background-color: #FFFF00\">ord gul</span> ord efter\n";
        let img = |_: &str| None;
        let measure = |s: &str, _: FontId| s.chars().count() as f32 * 7.0;
        let p = layout::Params { palette: &pal, zoom: 1.0, reveal: 0..0, image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
        let mut job = layout::layout_job(t, &markup::analyze(t), &p);
        job.wrap.max_width = 2000.0;
        let mut g = None;
        let out = ctx.run_ui(Default::default(), |_| g = Some(ctx.fonts_mut(|f| f.layout_job(job.clone()))));
        std::mem::forget(out);
        let ms: Vec<(usize, usize, bool)> = highlight::find_all(t, "ord", false).into_iter().map(|m| (m.0, m.1, false)).collect();
        assert_eq!(ms.len(), 3);
        let layer = build(&g.unwrap(), t, &ms, Vec2::ZERO, -10.0, 1000.0, &pal);
        // Bara träffen i den gula texten ritas ovanpå; "ord före" och "ord efter" bakom.
        assert_eq!((layer.behind.len(), layer.over.len()), (ms.len() - 1, 1), "{} bakom, {} ovanpå", layer.behind.len(), layer.over.len());
    }

    /// Bara raderna i fönstret får rektanglar, och träffar utanför fönstret kostar ingenting.
    #[test]
    fn lagret_begransas_till_fonstret() {
        let pal = theme::palette(false);
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let t: String = (0..400).map(|i| format!("rad {i} ord\n\n")).collect();
        let img = |_: &str| None;
        let measure = |s: &str, _: FontId| s.chars().count() as f32 * 7.0;
        let p = layout::Params { palette: &pal, zoom: 1.0, reveal: 0..0, image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
        let mut job = layout::layout_job(&t, &markup::analyze(&t), &p);
        job.wrap.max_width = 300.0;
        let mut g = None;
        let out = ctx.run_ui(Default::default(), |_| g = Some(ctx.fonts_mut(|f| f.layout_job(job.clone()))));
        std::mem::forget(out);
        let g = g.unwrap();
        let ms: Vec<(usize, usize, bool)> = highlight::find_all(&t, "ord", false).into_iter().map(|m| (m.0, m.1, false)).collect();
        let all = build(&g, &t, &ms, Vec2::ZERO, 0.0, g.rect.max.y, &pal);
        assert_eq!(all.behind.len(), 400);
        let mid = g.rect.max.y / 2.0;
        let some = build(&g, &t, &ms, egui::vec2(10.0, 5.0), mid, mid + 200.0, &pal);
        assert!(!some.behind.is_empty() && some.behind.len() < 40, "{}", some.behind.len());
        let r = shape_rects(&some.behind)[0].0;
        assert!(r.min.y >= mid - 40.0 && r.max.y <= mid + 250.0, "{r:?}");
    }
}
