//! Omflöde runt förankrade textrutor (`<!--k:tb N …-->`): rutans rader ligger ovanpå texten bredvid.

use std::sync::Arc;

use egui::{Color32, FontFamily, FontId, Galley};

use super::{layout, markup, pages};
use crate::theme;

const LONG: &str = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua Ut enim ad minim veniam quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat";

fn fam(_: Option<&str>, b: bool, i: bool) -> FontFamily {
    theme::doc_font(b, i)
}

fn lay(text: &str, wrap: f32, geom: Option<&pages::Geom>) -> Arc<Galley> {
    let pal = theme::palette(false);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let imgf = |_: &str| None;
    let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
    let p = layout::Params { palette: &pal, zoom: 1.0, reveal: 0..0, matches: &[], image_size: &imgf, measure: &measure, font_family: &fam, fonts_gen: 0 };
    let mut cache = layout::Cache::default();
    let mut res = None;
    for _ in 0..2 {
        let out = ctx.run_ui(Default::default(), |_| {
            res = Some(cache.galley_paged(&ctx, 1, text, &p, wrap, geom));
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

fn first_row(g: &Galley, ch: char) -> usize {
    g.rows.iter().position(|r| r.row.glyphs.first().is_some_and(|c| c.chr == ch)).unwrap_or_else(|| panic!("ingen rad börjar med {ch}"))
}

/// Dokument: en rad före, rutan (150 px bred, 90 px hög, `x`/`y`/övrigt i `head`), därefter lång text.
fn doc(head: &str) -> String {
    format!(
        "Före\n\n<!--k:tb 1 w=\"150\" h=\"90\" {head}-->\n\nRuta<!--k:pp fr=\"w=112.5 x=end pl=4 pr=4 h=67.5\"-->\n\n<!--k:/tb-->\n\n{LONG}\n\n{LONG}\n\nSlut"
    )
}

#[test]
fn text_flyter_vid_sidan_av_en_hogerruta() {
    let t = doc("x=\"right\" y=\"0\"");
    let g = lay(&t, 400.0, None);
    assert_eq!(chars(&g), t.chars().count());
    let before = &g.rows[first_row(&g, 'F')];
    let boxr = &g.rows[first_row(&g, 'R')];
    let first_text = &g.rows[first_row(&g, 'L')];
    // Rutans text står till höger, text startar vid rutans överkant (inte under den).
    assert!(boxr.pos.x > 400.0 - 150.0 - 1.0, "ruta x {}", boxr.pos.x);
    assert!(first_text.pos.y < boxr.pos.y + 20.0 && first_text.pos.y > before.pos.y, "text y {} ruta y {}", first_text.pos.y, boxr.pos.y);
    // Raderna bredvid rutan är smala (rutan 150 + luft 12); under rutan använder de hela bredden.
    let (mut beside, mut below) = (0, 0);
    for (i, r) in g.rows.iter().enumerate() {
        if r.row.glyphs.len() < 12 || r.pos.y < first_text.pos.y - 1.0 || r.pos.x > 100.0 {
            continue;
        }
        if r.pos.y < first_text.pos.y + 90.0 - 20.0 {
            beside += 1;
            assert!(row_end(&g, i) <= 400.0 - 150.0 - 12.0 + 0.5, "rad {i} (y {}) slutar vid {}", r.pos.y, row_end(&g, i));
        } else if r.pos.y > first_text.pos.y + 90.0 + 5.0 {
            below += 1;
        }
    }
    assert!(beside >= 3 && below >= 2, "{beside} bredvid, {below} under");
    // Allt under rutan är längre än den smala kolumnen någonstans.
    assert!((0..g.rows.len()).any(|i| g.rows[i].pos.y > first_text.pos.y + 100.0 && row_end(&g, i) > 400.0 - 162.0 + 20.0));
}

#[test]
fn ruta_bakom_texten_paverkar_inte_radbrytningen() {
    let t = doc("x=\"right\" y=\"0\" wrap=\"behind\"");
    let g = lay(&t, 400.0, None);
    let first_text = first_row(&g, 'L');
    assert!(g.rows[first_text].pos.y < g.rows[first_row(&g, 'R')].pos.y + 20.0);
    // Full bredd: en av första raderna går bortom rutans vänsterkant.
    assert!((first_text..first_text + 3).any(|i| row_end(&g, i) > 400.0 - 150.0 + 10.0));
}

#[test]
fn ruta_i_floden_med_over_och_under_tar_plats() {
    // `wrap="top"`: rutan ligger i flödet (ingen överlappning, texten börjar under den).
    let t = doc("x=\"right\" y=\"0\" wrap=\"top\"");
    let g = lay(&t, 400.0, None);
    let boxr = &g.rows[first_row(&g, 'R')];
    let text = &g.rows[first_row(&g, 'L')];
    assert!(text.pos.y > boxr.pos.y + boxr.row.size.y - 1.0, "text y {} ruta y {}", text.pos.y, boxr.pos.y);
}

#[test]
fn ruta_med_forskjutning_nedat_flyttar_bara_rutan() {
    let (g0, g1) = (lay(&doc("x=\"right\" y=\"0\""), 400.0, None), lay(&doc("x=\"right\" y=\"40\""), 400.0, None));
    let y = |g: &Galley, c: char| g.rows[first_row(g, c)].pos.y;
    assert!((y(&g1, 'R') - y(&g0, 'R') - 40.0).abs() < 1.5, "ruta {} -> {}", y(&g0, 'R'), y(&g1, 'R'));
    assert!((y(&g1, 'L') - y(&g0, 'L')).abs() < 1.5, "text {} -> {}", y(&g0, 'L'), y(&g1, 'L'));
}

#[test]
fn omflode_overlever_sidbrytning() {
    let a = markup::analyze("");
    let geom = pages::Geom::new(1.0, &a.defaults);
    let t = doc("x=\"right\" y=\"0\"");
    let g = lay(&t, geom.edit_w(), Some(&geom));
    let boxr = &g.rows[first_row(&g, 'R')];
    let text = &g.rows[first_row(&g, 'L')];
    assert!(text.pos.y < boxr.pos.y + 20.0, "text y {} ruta y {}", text.pos.y, boxr.pos.y);
    assert!(boxr.pos.x > text.pos.x + 150.0, "ruta x {} text x {}", boxr.pos.x, text.pos.x);
    assert_eq!(chars(&g), t.chars().count());
}
