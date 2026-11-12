//! Textomflöde runt flytande bilder i layouten (styckecache, per-rad-delning, sidbrytning).

use std::sync::Arc;

use egui::{Color32, FontFamily, FontId, Galley, Vec2};

use super::{layout, markup, pages};
use crate::theme;

const LONG: &str = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua Ut enim ad minim veniam quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat";

fn fam(_: Option<&str>, b: bool, i: bool) -> FontFamily {
    theme::doc_font(b, i)
}

/// Lägger ut `text` som `word_page` (styckecache + sidbrytning) med bilder på `img` px.
fn lay(text: &str, wrap: f32, img: Vec2, geom: Option<&pages::Geom>) -> (Arc<Galley>, Vec<Option<markup::Float>>) {
    let pal = theme::palette(false);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let imgf = move |_: &str| Some(img);
    let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
    let p = layout::Params {
        palette: &pal,
        zoom: 1.0,
        reveal: 0..0,
        matches: &[],
        image_size: &imgf,
        measure: &measure,
        font_family: &fam,
        fonts_gen: 0,
    };
    let mut cache = layout::Cache::default();
    let a = markup::analyze(text);
    let starts: Vec<usize> = a.pieces.iter().filter(|pc| pc.image.is_some()).map(|pc| pc.range.start).collect();
    let mut res = None;
    for _ in 0..2 {
        let out = ctx.run_ui(Default::default(), |_| {
            let g = cache.galley_paged(&ctx, 1, text, &p, wrap, geom);
            let sides = starts.iter().map(|&s| cache.float_side(1, text, s).map(|f| f.0)).collect();
            res = Some((g, sides));
        });
        std::mem::forget(out);
    }
    res.unwrap()
}

fn row_end(g: &Galley, i: usize) -> f32 {
    let r = &g.rows[i];
    r.pos.x + r.row.glyphs.iter().rposition(|c| !c.chr.is_whitespace()).map_or(0.0, |k| r.row.glyphs[k].max_x())
}

fn chars(g: &Galley) -> usize {
    g.rows.iter().map(|r| r.char_count_including_newline().0).sum()
}

const IMG_L: &str = "<img src=\"a.png\" alt=\"\" width=\"100\" align=\"left\">";
const IMG_R: &str = "<img src=\"a.png\" alt=\"\" width=\"100\" align=\"right\">";

fn text_rows(g: &Galley) -> Vec<usize> {
    (0..g.rows.len()).filter(|&i| g.rows[i].row.glyphs.len() > 10 && g.rows[i].row.size.y > 3.0).collect()
}

#[test]
fn rader_bredvid_vansterbild_borjar_till_hoger_om_den() {
    let t = format!("{IMG_L}\n\n{LONG}\n\n{LONG}\n\n{LONG}\n\n{LONG}\n\nSlut");
    let (g, sides) = lay(&t, 400.0, Vec2::new(100.0, 120.0), None);
    assert_eq!(sides, vec![Some(markup::Float::Left)]);
    // Inga tecken försvinner eller dubbleras.
    assert_eq!(chars(&g), t.chars().count());
    let rows = text_rows(&g);
    // Bilden har ingen egen höjd: första textraden ligger överst.
    assert!(g.rows[rows[0]].pos.y < 12.0, "första textraden y = {}", g.rows[rows[0]].pos.y);
    let (mut beside, mut below) = (0, 0);
    for &i in &rows {
        let (y, x) = (g.rows[i].pos.y, g.rows[i].pos.x);
        if y < 120.0 - 0.5 {
            beside += 1;
            assert!(x >= 112.0 - 0.6, "rad {i} (y {y}) börjar vid {x}");
            assert!(row_end(&g, i) <= 400.5, "rad {i} slutar vid {}", row_end(&g, i));
        } else {
            below += 1;
            assert!(x < 1.0, "rad {i} (y {y}) under bilden börjar vid {x}");
        }
    }
    assert!(beside >= 4 && below >= 2, "{beside} bredvid, {below} under");
    // Raderna under bilden använder hela bredden: någon går längre än den smala kolumnen.
    assert!(rows.iter().any(|&i| g.rows[i].pos.y >= 120.0 && row_end(&g, i) > 400.0 - 112.0 + 20.0));
}

#[test]
fn hogerbild_smalnar_men_forskjuter_inte() {
    let t = format!("{IMG_R}\n\n{LONG}\n\n{LONG}\n\n{LONG}\n\nSlut");
    let (g, sides) = lay(&t, 400.0, Vec2::new(100.0, 100.0), None);
    assert_eq!(sides, vec![Some(markup::Float::Right)]);
    assert_eq!(chars(&g), t.chars().count());
    for &i in &text_rows(&g) {
        if g.rows[i].pos.y < 99.0 {
            assert!(g.rows[i].pos.x < 1.0);
            assert!(row_end(&g, i) <= 400.0 - 112.0 + 0.5, "rad {i} slutar vid {}", row_end(&g, i));
        }
    }
}

#[test]
fn stycke_delas_per_rad_och_markor_stammer() {
    // Ett långt stycke som passerar bildens underkant: raderna under får full bredd.
    let long = std::iter::repeat_n(LONG, 4).collect::<Vec<_>>().join(" ");
    let t = format!("{IMG_L}\n\n{long}\n\nSlut");
    let (g, _) = lay(&t, 400.0, Vec2::new(100.0, 60.0), None);
    assert_eq!(chars(&g), t.chars().count());
    let rows = text_rows(&g);
    let narrow = rows.iter().filter(|&&i| g.rows[i].pos.x > 100.0).count();
    let wide = rows.iter().filter(|&&i| g.rows[i].pos.x < 1.0).count();
    assert!(narrow >= 2 && wide >= 2, "{narrow} smala, {wide} breda");
    // Markörer: tecken -> läge -> tecken på varje rad i det delade stycket.
    for ci in (IMG_L.len() + 2..t.chars().count()).step_by(37) {
        let r = g.pos_from_cursor(egui::text::CCursor::new(ci));
        assert_eq!(g.cursor_from_pos(r.center().to_vec2()).index.0, ci, "tecken {ci}");
    }
    // Delningen inne i stycket är ingen radbrytning i texten: bara en `\n` mellan styckena.
    let nl = g.rows.iter().filter(|r| r.ends_with_newline).count();
    assert_eq!(nl, t.matches('\n').count());
}

#[test]
fn bild_utan_plats_far_en_egen_rad() {
    // Bilden är bredare än spalten minus minsta textbredd: ingen flytning.
    let t = format!("{IMG_L}\n\n{LONG}\n\nSlut");
    let (g, sides) = lay(&t, 400.0, Vec2::new(380.0, 90.0), None);
    assert_eq!(sides, vec![None]);
    assert_eq!(chars(&g), t.chars().count());
    let row = g.rows.iter().find(|r| r.row.glyphs.first().is_some_and(|c| c.chr == 'L')).unwrap();
    assert!(row.pos.y >= 89.0 && row.pos.x < 1.0, "y {} x {}", row.pos.y, row.pos.x);
}

#[test]
fn andra_bilden_pa_samma_sida_blir_en_rad() {
    let t = format!("{IMG_L}\n\n{LONG}\n\n{IMG_L}\n\n{LONG}\n\nSlut");
    let (_, sides) = lay(&t, 400.0, Vec2::new(100.0, 400.0), None);
    assert_eq!(sides, vec![Some(markup::Float::Left), None]);
    let t = format!("{IMG_L}\n\n{LONG}\n\n{IMG_R}\n\n{LONG}\n\nSlut");
    let (_, sides) = lay(&t, 600.0, Vec2::new(100.0, 400.0), None);
    assert_eq!(sides, vec![Some(markup::Float::Left), Some(markup::Float::Right)]);
}

#[test]
fn bild_som_inte_ryms_flyttas_till_nasta_sida() {
    let geom = pages::Geom::new(1.0, &markup::analyze("").defaults);
    let h = geom.text_h();
    // Fyll nästan hela första sidan med text; bilden (40 % av sidan) ryms inte kvar.
    let filler: Vec<String> = (0..16).map(|_| LONG.to_string()).collect();
    let t = format!("{}\n\n{IMG_L}\n\n{LONG}\n\nSlut", filler.join("\n\n"));
    let (g, _) = lay(&t, geom.text_w(), Vec2::new(150.0, h * 0.4), Some(&geom));
    assert_eq!(chars(&g), t.chars().count());
    // Texten efter bilden ligger på sida 2, i den smala kolumnen.
    let key = t.find(IMG_L).unwrap() + IMG_L.len();
    let ci = t[..key].chars().count() + 2;
    let r = g.pos_from_cursor(egui::text::CCursor::new(ci));
    assert!(geom.page_of(r.min.y) >= 1, "y = {}", r.min.y);
    assert!(r.min.x >= 150.0, "x = {}", r.min.x);
    assert!(pages::page_count(&g, &geom) >= 2);
}

#[test]
fn bild_i_stycket_far_stycket_sjalvt_att_flyta() {
    // Bilden står först i ett stycke med text (så läser Word in en förankrad bild).
    let t = format!("<img src=\"a.png\" alt=\"\" width=\"100\" align=\"left\">{LONG} {LONG} {LONG}\n\nSlut");
    let (g, sides) = lay(&t, 400.0, Vec2::new(100.0, 70.0), None);
    assert_eq!(sides, vec![Some(markup::Float::Left)]);
    assert_eq!(chars(&g), t.chars().count());
    let rows = text_rows(&g);
    // Första raden ligger överst och är indragen; raderna under bilden är det inte.
    assert!(g.rows[rows[0]].pos.y < 1.0 && g.rows[rows[0]].pos.x >= 111.0);
    assert!(rows.iter().any(|&i| g.rows[i].pos.y >= 70.0 && g.rows[i].pos.x < 1.0));
}

#[test]
fn andrad_align_andrar_layouten() {
    let l = format!("{IMG_L}\n\n{LONG}\n\nSlut");
    let (a, _) = lay(&l, 400.0, Vec2::new(100.0, 70.0), None);
    let n = l.replace("align=\"left\"", "");
    let (b, sides) = lay(&n, 400.0, Vec2::new(100.0, 70.0), None);
    assert_eq!(sides, vec![None]);
    assert!(a.rect.height() < b.rect.height() + 0.1);
    assert!(b.rows.iter().all(|r| r.pos.x < 1.0));
}
