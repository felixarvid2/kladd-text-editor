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
    let p = layout::Params { palette: &pal, zoom: 1.0, reveal: 0..0, image_size: &imgf, measure: &measure, font_family: &fam, fonts_gen: 0 };
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
    doc_w(150, head)
}

/// Som `doc`, men med rutans bredd `w` px.
fn doc_w(w: u32, head: &str) -> String {
    format!(
        "Före\n\n<!--k:tb 1 w=\"{w}\" h=\"90\" {head}-->\n\nRuta<!--k:pp fr=\"w={} x=end pl=4 pr=4 h=67.5\"-->\n\n<!--k:/tb-->\n\n{LONG}\n\n{LONG}\n\nSlut",
        w as f32 * 0.75
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

/// Flyttar rutan som `tboxui` gör vid dragning: nytt `x`/`y` i öppningsraden och `fr=` i rutans stycken.
fn dragged(t: &str, x: f32, y: f32) -> String {
    let open = t.find("<!--k:tb").unwrap();
    let open = open..t[open..].find("-->").unwrap() + open + 3;
    let head = crate::docx_tbox::parse_head(&t[open.clone()]).unwrap().1;
    let mut a = head.anchor.unwrap_or_default();
    a.h = super::anchor::Pos::Off(x);
    a.v = super::anchor::Pos::Off(y);
    let mut head = head;
    head.anchor = Some(a);
    super::tboxui::apply(t, &super::tboxui::Change { open, head, x_pt: Some(x * 0.75) }).unwrap()
}

fn row_text(g: &Galley, i: usize) -> String {
    g.rows[i].row.glyphs.iter().map(|c| c.chr).collect()
}

/// Raderna mellan `<!--k:tb` och `<!--k:/tb-->` (rutans egna) respektive brödtextrader (minst 12 tecken) efter dem.
fn box_range(g: &Galley) -> std::ops::Range<usize> {
    let find = |p: &str| (0..g.rows.len()).find(|&i| row_text(g, i).starts_with(p));
    let (o, c) = (find("<!--k:tb").expect("rutans öppning"), find("<!--k:/tb-->").expect("rutans stängning"));
    o + 1..c
}

fn is_body_row(g: &Galley, i: usize) -> bool {
    let t = row_text(g, i);
    t.chars().count() >= 12 && !t.starts_with('<') && !box_range(g).contains(&i)
}

/// Kontrollerar att ingen brödtextrad ligger över rutan (rutans vänsterkant `pad` till vänster om dess första
/// rad, bredd `w`, höjd `h` från textens läge efter rutan). Ger antalet brödtextrader som ligger vid rutans höjd.
fn check_no_overlap(g: &Galley, pad: f32, w: f32, h: f32, what: &str) -> usize {
    let br = box_range(g);
    let open = br.start;
    let r0 = br.clone().find(|&i| g.rows[i].row.size.y > 5.0 && !row_text(g, i).is_empty()).unwrap_or_else(|| panic!("{what}: ingen rutrad"));
    // Texten efter rutan dras upp till rutans överkant: stängningsradens läge är rutans överkant i texten.
    let (bx0, y0) = (g.rows[r0].pos.x - pad, g.rows[br.end].pos.y);
    let (bx1, y1) = (bx0 + w, y0 + h);
    let mut n = 0;
    for i in (open..g.rows.len()).filter(|&i| is_body_row(g, i)) {
        let r = &g.rows[i];
        if r.pos.y + r.row.size.y <= y0 + 0.5 || r.pos.y >= y1 - 0.5 {
            continue;
        }
        n += 1;
        let (s, e) = (r.pos.x, row_end(g, i));
        assert!(e <= bx0 + 0.5 || s >= bx1 - 0.5, "{what}: rad {i} \"{}\" {s}..{e} (y {}) ligger över rutan {bx0}..{bx1} (y {y0}..{y1})", row_text(g, i), r.pos.y);
    }
    n
}

#[test]
fn text_flyter_runt_eller_hoppar_forbi_ruta_var_som_helst_i_spalten() {
    // Smal spalt (300 px): en 150 px bred ruta mitt i lämnar mindre än FLOAT_MIN_ROOM på båda sidor, så texten
    // ska hoppa förbi rutan; närmare en kant flyter den på den bredare sidan.
    for x in [0.0, 20.0, 45.0, 70.0, 100.0, 125.0, 150.0] {
        let t = dragged(&doc("x=\"right\" y=\"0\""), x, 0.0);
        let g = lay(&t, 300.0, None);
        assert_eq!(chars(&g), t.chars().count());
        check_no_overlap(&g, 5.0, 150.0, 65.0, &format!("x={x}"));
        // Texten finns kvar i dokumentet och börjar inte ovanför rutan.
        assert!((0..g.rows.len()).any(|i| is_body_row(&g, i)));
    }
}

#[test]
fn ruta_dragen_ut_over_spaltens_kant_stammer_med_ritningen() {
    // Ritningen kläms till spalten (x 0..250 för en 150 px bred ruta i 400 px); layouten gör likadant.
    for x in [-40.0, 300.0, 400.0] {
        let t = dragged(&doc("x=\"right\" y=\"0\""), x, 0.0);
        let g = lay(&t, 400.0, None);
        let boxr = &g.rows[first_row(&g, 'R')];
        let want = x.clamp(0.0, 250.0) + 5.0;
        assert!((boxr.pos.x - want).abs() < 1.5, "x={x}: ruta x {} väntade {want}", boxr.pos.x);
        check_no_overlap(&g, 5.0, 150.0, 65.0, &format!("x={x}"));
        // Till höger om rutan (x=300 kläms till 250) finns ingen plats: texten ligger vänster om den.
        let first_text = &g.rows[first_row(&g, 'L')];
        if x >= 250.0 {
            assert!(row_end(&g, first_row(&g, 'L')) <= want - 5.0 - 12.0 + 0.5 || first_text.pos.y > boxr.pos.y + 60.0, "x={x}");
        }
    }
}

#[test]
fn ruta_mitt_i_spalten_med_sidor_flyter_likadant() {
    let a = markup::analyze("");
    let geom = pages::Geom::new(1.0, &a.defaults);
    let col = geom.edit_w();
    for x in [(col - 150.0) / 2.0, col * 0.2, col * 0.35, col * 0.5, col * 0.6] {
        let t = dragged(&doc("x=\"right\" y=\"0\""), x, 0.0);
        let g = lay(&t, col, Some(&geom));
        assert_eq!(chars(&g), t.chars().count());
        check_no_overlap(&g, 5.0, 150.0, 65.0, &format!("sida x={x}"));
    }
}

fn newsletter() -> String {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus/docx/gen-nyhetsbrev.docx")).unwrap();
    crate::docx::read(&bytes).unwrap().markdown
}

fn lay_sections(text: &str) -> Arc<Galley> {
    let a = markup::analyze(text);
    let geom = pages::Geom::for_sections(1.0, &a.defaults, &a.sections);
    lay(text, geom.edit_w(), Some(&geom))
}

#[test]
fn nyhetsbrevets_ruta_dragen_mot_mitten_av_spalten() {
    // "Städdag"-rutan (157 px) i en spalt på ca 302 px: dras den in mot mitten finns inte plats på någon sida.
    let t0 = newsletter();
    for x in [None, Some(0.0), Some(20.0), Some(50.0), Some(70.0), Some(100.0), Some(145.0)] {
        let t = x.map_or_else(|| t0.clone(), |x| dragged(&t0, x, 6.3));
        let g = lay_sections(&t);
        let n = check_no_overlap(&g, 12.0, 157.5, 100.0, &format!("nyhetsbrev x={x:?}"));
        println!("x={x:?}: {n} rader bredvid rutan");
    }
}

// ---------------------------------------------------------------- rutor i marginalen

/// Brödtextraderna (text, y, slutkant) i ordning; rutans rader och markörrader räknas inte.
fn body(g: &Galley) -> Vec<(String, f32, f32)> {
    let boxed = (0..g.rows.len()).any(|i| row_text(g, i).starts_with("<!--k:tb"));
    let long = |i: usize| row_text(g, i).chars().count() >= 12 && !row_text(g, i).starts_with('<');
    (0..g.rows.len()).filter(|&i| long(i) && (!boxed || is_body_row(g, i))).map(|i| (row_text(g, i), g.rows[i].pos.y, row_end(g, i))).collect()
}

/// Pixlar per punkt-px i rutans ram: `pl=4` pt är 5,33 px.
const PL: f32 = 4.0 / 0.75;

fn page_geom() -> pages::Geom {
    pages::Geom::new(1.0, &markup::analyze("").defaults)
}

#[test]
fn ruta_helt_i_hogermarginalen_paverkar_inte_texten() {
    let geom = page_geom();
    let col = geom.edit_w();
    assert!(geom.right > 90.0, "marginal {}", geom.right);
    // 80 px bred ruta 8 px utanför spalten: ryms i marginalen (96 px).
    let t = dragged(&doc_w(80, "x=\"right\" y=\"0\""), col + 8.0, 0.0);
    let g = lay(&t, col, Some(&geom));
    let plain = lay(&format!("Före\n\n{LONG}\n\n{LONG}\n\nSlut"), col, Some(&geom));
    let (a, b) = (body(&g), body(&plain));
    // Samma rader, samma radbrytning och samma läge som utan rutan: texten ser inte rutan.
    let n = b.len();
    let a: Vec<_> = a.into_iter().filter(|r| r.0.chars().count() >= 12).collect();
    let b: Vec<_> = b.into_iter().filter(|r| r.0.chars().count() >= 12).collect();
    assert_eq!(a.len(), b.len(), "{n} rader utan ruta");
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.0, y.0);
        assert!((x.1 - y.1).abs() < 1.5 && (x.2 - y.2).abs() < 1.5, "{x:?} mot {y:?}");
    }
    // Rutan ritas där den står: ut i marginalen.
    let boxr = &g.rows[first_row(&g, 'R')];
    assert!((boxr.pos.x - (col + 8.0 + PL)).abs() < 1.5, "ruta x {} spalt {col}", boxr.pos.x);
    assert_eq!(chars(&g), t.chars().count());
}

#[test]
fn ruta_delvis_i_marginalen_flyter_texten_runt_delen_i_spalten() {
    let geom = page_geom();
    let col = geom.edit_w();
    // 150 px bred ruta, 60 px in i spalten: 90 px sticker ut i marginalen.
    let t = dragged(&doc("x=\"right\" y=\"0\"").replace("\n\nSlut", &format!("\n\n{LONG}\n\n{LONG}\n\nSlut")), col - 60.0, 0.0);
    let g = lay(&t, col, Some(&geom));
    let boxr = &g.rows[first_row(&g, 'R')];
    assert!((boxr.pos.x - (col - 60.0 + PL)).abs() < 1.5, "ruta x {} spalt {col}", boxr.pos.x);
    // Raderna bredvid rutan slutar före dess vänsterkant (och luften); under den går de ända ut.
    let first = &g.rows[first_row(&g, 'L')];
    let (mut beside, mut below) = (0, 0);
    for i in (0..g.rows.len()).filter(|&i| is_body_row(&g, i)) {
        let r = &g.rows[i];
        if r.pos.y < first.pos.y + 90.0 - 20.0 && r.pos.y > first.pos.y - 1.0 {
            beside += 1;
            assert!(row_end(&g, i) <= col - 60.0 - 12.0 + 0.5, "rad {i} slutar vid {}", row_end(&g, i));
        } else if r.pos.y > first.pos.y + 100.0 && row_end(&g, i) > col - 72.0 + 20.0 {
            below += 1;
        }
    }
    assert!(beside >= 3 && below >= 2, "{beside} bredvid, {below} under");
    check_no_overlap(&g, PL, 150.0, 65.0, "delvis i marginalen");
    assert_eq!(chars(&g), t.chars().count());
}

#[test]
fn ruta_i_vanstermarginalen_och_vid_papperets_kant() {
    let geom = page_geom();
    let col = geom.edit_w();
    // Helt i vänstermarginalen (x < 0): ritas där, texten opåverkad.
    let t = dragged(&doc_w(60, "x=\"right\" y=\"0\""), -70.0, 0.0);
    let g = lay(&t, col, Some(&geom));
    assert!((g.rows[first_row(&g, 'R')].pos.x - (-70.0 + PL)).abs() < 1.5, "x {} left {}", g.rows[first_row(&g, 'R')].pos.x, geom.left);
    let plain = lay(&format!("Före\n\n{LONG}\n\n{LONG}\n\nSlut"), col, Some(&geom));
    assert_eq!(body(&g).iter().filter(|r| r.0.chars().count() >= 12).count(), body(&plain).iter().filter(|r| r.0.chars().count() >= 12).count());
    // Förbi papperets kant klämms rutan mot kanten så att den inte försvinner.
    let (left, right) = (geom.left, geom.right);
    for (x, want) in [(-5000.0, -left), (col + 5000.0, col + right - 150.0)] {
        let t = dragged(&doc("x=\"right\" y=\"0\""), x, 0.0);
        let g = lay(&t, col, Some(&geom));
        let boxr = &g.rows[first_row(&g, 'R')];
        assert!((boxr.pos.x - (want + PL)).abs() < 1.5, "x={x}: ruta x {} väntade {}", boxr.pos.x, want + PL);
        check_no_overlap(&g, PL, 150.0, 65.0, &format!("papperskant x={x}"));
    }
}

#[test]
fn ruta_relativt_sidan_eller_marginalen_hamnar_som_relativt_spalten() {
    let geom = page_geom();
    let col = geom.edit_w();
    let head = |hrel: &str, x: f32| format!("hrel=\"{hrel}\" x=\"{x}\" y=\"0\"");
    // Samma läge angivet från spalten, marginalen (= spalten här) och sidan (marginalen till vänster längre ut).
    let by_col = lay(&dragged(&doc("x=\"right\" y=\"0\""), col - 60.0, 0.0), col, Some(&geom));
    for h in [head("margin", col - 60.0), head("page", geom.left + col - 60.0)] {
        // `dragged` skriver om ankaret till kolumn; här sätts rubriken för hand och ramens läge görs likadant.
        let t = doc(&h).replace("x=end", &format!("x={}", (col - 60.0) * 0.75));
        let g = lay(&t, col, Some(&geom));
        let (a, b) = (body(&g), body(&by_col));
        assert_eq!(a.len(), b.len(), "{h}");
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.0, y.0, "{h}");
            assert!((x.1 - y.1).abs() < 1.5 && (x.2 - y.2).abs() < 0.5, "{h}: {x:?} mot {y:?}");
        }
        assert!((g.rows[first_row(&g, 'R')].pos.x - by_col.rows[first_row(&by_col, 'R')].pos.x).abs() < 1.5, "{h}");
    }
}

// ---------------------------------------------------------------- markör och pekare

use super::grid::GalleyIndex;
use super::tboxnav::{self, Boxes, Zone};

/// Rutans skärmruta (galleri-koordinater): rutans egna rader, `w` bred.
fn box_rect(g: &Galley, w: f32) -> (usize, egui::Rect) {
    let idx = GalleyIndex::new("", g);
    let _ = idx;
    let br = box_range(g);
    let rows: Vec<_> = br.clone().filter(|&i| g.rows[i].row.size.y > 5.0 && !row_text(g, i).is_empty()).collect();
    let (top, bottom) = (g.rows[rows[0]].pos.y, g.rows[*rows.last().unwrap()].rect().max.y);
    let x0 = g.rows[rows[0]].pos.x - PL;
    (0, egui::Rect::from_min_max(egui::pos2(x0, top), egui::pos2(x0 + w, bottom.max(top + 65.0))))
}

/// Raden markören syns på: vid en mjuk radbrytning ligger slutet av en rad på radgränsen och visas sist på raden före.
fn row_at(text: &str, g: &Galley, byte: usize) -> usize {
    let idx = GalleyIndex::new(text, g);
    let c = idx.chr(byte);
    let r = idx.row_of(c);
    if r > 0 && c == idx.start(r) && !g.rows[r - 1].ends_with_newline { r - 1 } else { r }
}

fn byte_of_row_start(text: &str, g: &Galley, row: usize) -> usize {
    let idx = GalleyIndex::new(text, g);
    crate::app::char_to_byte(text, idx.start(row))
}

fn with_boxes<R>(t: &str, g: &Galley, f: impl FnOnce(&Boxes) -> R) -> R {
    let idx = GalleyIndex::new(t, g);
    f(&Boxes::new(t, g, &idx))
}

fn opens(t: &str, g: &Galley) -> Vec<usize> {
    with_boxes(t, g, |b| b.opens.clone())
}

#[test]
fn pil_upp_och_ned_hoppar_over_rutans_rader() {
    // Ruta mitt i spalten (x=60): brödtextraderna bredvid ligger i samma höjd som rutans.
    let t = dragged(&doc("x=\"right\" y=\"0\""), 60.0, 0.0);
    let g = lay(&t, 400.0, None);
    let (fore, first, second) = (first_row(&g, 'F'), first_row(&g, 'L'), first_row(&g, 'L') + 1);
    let boxr = first_row(&g, 'R');
    assert!(g.rows[first].pos.y < g.rows[boxr].pos.y + 20.0, "brödtext och ruta överlappar inte i y");
    with_boxes(&t, &g, |boxes| {
        assert_eq!(boxes.zone_of(boxr), Zone::Box(0));
        assert_eq!(boxes.zone_of(first), Zone::Body);
        // Upp från första brödtextraden efter rutan: raden före rutan, inte rutans rad.
        let from = byte_of_row_start(&t, &g, first) + 3;
        let b = tboxnav::vertical(&t, &g, boxes, from, true).expect("hoppar");
        assert_eq!(row_at(&t, &g, b), fore, "upp hamnade på rad {}", row_at(&t, &g, b));
        // Ned från raden före rutan: TextEdit tar den tomma raden under (brödtext); därifrån hoppar markören
        // över rutan till första synliga brödtextraden efter den.
        assert_eq!(tboxnav::vertical(&t, &g, boxes, byte_of_row_start(&t, &g, fore) + 1, false), None);
        let b = tboxnav::vertical(&t, &g, boxes, byte_of_row_start(&t, &g, fore + 1), false).expect("hoppar");
        assert_eq!(row_at(&t, &g, b), first, "ned hamnade på rad {}", row_at(&t, &g, b));
        // Vanliga rader (nästa rad är brödtext): TextEdit sköter det.
        assert_eq!(tboxnav::vertical(&t, &g, boxes, byte_of_row_start(&t, &g, first) + 3, false), None);
        assert_eq!(tboxnav::vertical(&t, &g, boxes, byte_of_row_start(&t, &g, second) + 3, true), None);
    });
}

#[test]
fn pil_upp_och_ned_i_rutan_ror_sig_inom_den_och_lamnar_den_vid_kanten() {
    let pp = "<!--k:pp fr=\"w=112.5 x=end pl=4 pr=4 h=67.5\"-->";
    let t = format!(
        "Före\n\n<!--k:tb 1 w=\"150\" h=\"90\" x=\"right\" y=\"0\"-->\n\nRuta ett{pp}\n\nRuta två{pp}\n\nRuta tre{pp}\n\n<!--k:/tb-->\n\n{LONG}\n\nSlut"
    );
    let g = lay(&t, 400.0, None);
    let rows: Vec<usize> = ["Ruta ett", "Ruta två", "Ruta tre"].iter().map(|p| (0..g.rows.len()).find(|&i| row_text(&g, i).starts_with(p)).unwrap()).collect();
    let (fore, first) = (first_row(&g, 'F'), first_row(&g, 'L'));
    with_boxes(&t, &g, |boxes| {
        let at = |r: usize, off: usize| byte_of_row_start(&t, &g, r) + off;
        let to = |b: Option<usize>| b.map(|b| row_at(&t, &g, b));
        assert_eq!(to(tboxnav::vertical(&t, &g, boxes, at(rows[0], 3), false)), Some(rows[1]));
        assert_eq!(to(tboxnav::vertical(&t, &g, boxes, at(rows[1], 3), false)), Some(rows[2]));
        assert_eq!(to(tboxnav::vertical(&t, &g, boxes, at(rows[2], 3), true)), Some(rows[1]));
        // Vid kanterna: upp ur första raden till raden före rutan, ned ur sista till brödtexten efter den.
        assert_eq!(to(tboxnav::vertical(&t, &g, boxes, at(rows[0], 3), true)), Some(fore));
        assert_eq!(to(tboxnav::vertical(&t, &g, boxes, at(rows[2], 3), false)), Some(first));
    });
}

#[test]
fn klick_bredvid_ruta_ger_brodtexten_och_klick_i_rutan_rutans_text() {
    let t = dragged(&doc("x=\"right\" y=\"0\""), 200.0, 0.0);
    let g = lay(&t, 400.0, None);
    let (open, rect) = box_rect(&g, 150.0);
    let _ = open;
    let rects = [(opens(&t, &g)[0], rect)];
    let idx = GalleyIndex::new(&t, &g);
    let first = first_row(&g, 'L');
    with_boxes(&t, &g, |boxes| {
        // Bredvid rutan, i höjd med dess rad: brödtexten.
        let local = egui::vec2(40.0, g.rows[first].pos.y + 4.0);
        let (b, z) = tboxnav::hit(&t, &g, &idx, boxes, &rects, local, None, (0.0, 400.0)).unwrap();
        assert_eq!(z, Zone::Body);
        assert_eq!(row_at(&t, &g, b), first);
        // I rutan: rutans text, också där en brödtextrad ligger i samma höjd.
        let rr = &g.rows[first_row(&g, 'R')];
        let local = egui::vec2(rr.pos.x + 6.0, rr.pos.y + 4.0);
        let (b, z) = tboxnav::hit(&t, &g, &idx, boxes, &rects, local, None, (0.0, 400.0)).unwrap();
        assert_eq!(z, Zone::Box(0));
        assert!(t[b..].starts_with("uta"), "{:?}", &t[b..b + 5]);
        // Markering som började i brödtexten följer brödtexten även över rutan (läsordning).
        let (b, z) = tboxnav::hit(&t, &g, &idx, boxes, &rects, local, Some(Zone::Body), (0.0, 400.0)).unwrap();
        assert_eq!(z, Zone::Body);
        assert_eq!(row_at(&t, &g, b), first, "markering över rutan ska ge brödtext");
    });
}

#[test]
fn klick_i_hogermarginalen_ger_rutans_text_och_brodtexten_ar_orord() {
    let geom = page_geom();
    let col = geom.edit_w();
    let t = dragged(&doc_w(80, "x=\"right\" y=\"0\""), col + 8.0, 0.0);
    let g = lay(&t, col, Some(&geom));
    let (_, rect) = box_rect(&g, 80.0);
    let rects = [(opens(&t, &g)[0], rect)];
    let idx = GalleyIndex::new(&t, &g);
    let first = first_row(&g, 'L');
    with_boxes(&t, &g, |boxes| {
        let rr = &g.rows[first_row(&g, 'R')];
        assert!(rr.pos.x > col, "rutans rad ligger i marginalen");
        let (b, z) = tboxnav::hit(&t, &g, &idx, boxes, &rects, egui::vec2(rr.pos.x + 6.0, rr.pos.y + 4.0), None, (0.0, col)).unwrap();
        assert_eq!(z, Zone::Box(0));
        assert!(t[b..].starts_with("uta"));
        let (b, z) = tboxnav::hit(&t, &g, &idx, boxes, &rects, egui::vec2(col - 20.0, g.rows[first].pos.y + 4.0), None, (0.0, col)).unwrap();
        assert_eq!((z, row_at(&t, &g, b)), (Zone::Body, first));
        // Pil upp förbi en ruta i marginalen går till raden ovanför.
        let up = tboxnav::vertical(&t, &g, boxes, byte_of_row_start(&t, &g, first) + 3, true).unwrap();
        assert_eq!(row_at(&t, &g, up), first_row(&g, 'F'));
    });
}

// ---------------------------------------------------------------- faktarutan i gen-skolrapport.docx

/// gen-skolrapport.docx med faktarutans vågräta läge angivet från `rel` (`off` EMU): samma plats på sidan.
fn skolrapport_with(rel: &str, off: i64) -> String {
    use std::io::{Read, Write};
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus/docx/gen-skolrapport.docx")).unwrap();
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zout = zip::ZipWriter::new(&mut buf);
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            if f.name() == "word/document.xml" {
                let x = String::from_utf8(data).unwrap();
                let old = "<wp:positionH relativeFrom=\"page\"><wp:posOffset>5900000</wp:posOffset></wp:positionH>";
                assert!(x.contains(old));
                data = x.replace(old, &format!("<wp:positionH relativeFrom=\"{rel}\"><wp:posOffset>{off}</wp:posOffset></wp:positionH>")).into_bytes();
            }
            zout.start_file(f.name().to_owned(), zip::write::SimpleFileOptions::default()).unwrap();
            zout.write_all(&data).unwrap();
        }
        zout.finish().unwrap();
    }
    crate::docx::read(&buf.into_inner()).unwrap().markdown
}

/// Faktarutan ligger i högermarginalen i alla lägen (kolumn, marginal, sida, tecken) och ritning och layout
/// är överens: rutans rader står utanför spalten och ingen brödtextrad når in i dem.
#[test]
fn faktarutan_i_skolrapporten_star_i_marginalen_for_alla_hrel() {
    // 5 900 000 EMU från sidans kant; vänstermarginalen är 1417 twips = 899 795 EMU.
    for (rel, off) in [("page", 5_900_000), ("margin", 5_000_205), ("column", 5_000_205), ("character", 5_000_205)] {
        let md = skolrapport_with(rel, off);
        let a = markup::analyze(&md);
        let geom = pages::Geom::for_sections(1.0, &a.defaults, &a.sections);
        let col = geom.edit_w();
        let g = lay(&md, col, Some(&geom));
        let find = |p: &str| (0..g.rows.len()).find(|&i| row_text(&g, i).contains(p) && !row_text(&g, i).starts_with("<!--k:tb")).unwrap_or_else(|| panic!("{rel}: ingen rad {p}"));
        let head = &g.rows[find("Fakta om skolan")];
        assert!(head.pos.x > col, "{rel}: rutans text x {} ska ligga utanför spalten ({col})", head.pos.x);
        // Rutans rektangel: från rutans vänsterkant (rubrikens x minus luften) och rutans höjd (1 650 000 EMU).
        let (bx0, y0, y1) = (head.pos.x - 12.0, head.pos.y - 5.0, head.pos.y - 5.0 + 1_650_000.0 / 9525.0);
        let br = box_range(&g);
        let mut beside = 0;
        for i in (0..g.rows.len()).filter(|&i| !br.contains(&i) && row_text(&g, i).chars().count() >= 8 && !row_text(&g, i).starts_with('<')) {
            let r = &g.rows[i];
            if r.pos.y + r.row.size.y <= y0 || r.pos.y >= y1 {
                continue;
            }
            beside += 1;
            assert!(row_end(&g, i) <= bx0 + 0.5, "{rel}: rad \"{}\" slutar vid {} över rutan från {bx0}", row_text(&g, i), row_end(&g, i));
        }
        assert!(beside >= 3, "{rel}: bara {beside} brödtextrader bredvid rutan");
    }
}
