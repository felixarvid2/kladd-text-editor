//! Prov för morph: parningsreglerna, interpoleringen och ändpunkterna (egna minimala bilder).

use resvg::tiny_skia as sk;

use super::super::tests::Builder;
use super::super::transition::{Kind, MorphBy, Transition};
use super::super::{raster, render, Deck};
use super::*;

fn info(id: u32, name: &str, kind: ShapeKind, text: &str, key: &str) -> Info {
    Info { id, name: name.into(), kind, text: text.into(), key: key.into(), xfrm: None }
}

#[test]
fn par_med_samma_namn_och_slag() {
    let a = vec![info(1, "Rubrik", ShapeKind::Text, "Hej", ""), info(2, "Ruta", ShapeKind::Shape, "", "rect")];
    let b = vec![info(5, "Ruta", ShapeKind::Shape, "", "rect"), info(6, "Rubrik", ShapeKind::Text, "Hejdå", ""), info(7, "Ny", ShapeKind::Shape, "", "rect")];
    // Texten får ändras; ny form saknar par.
    assert_eq!(pair(&a, &b), vec![Some(1), Some(0), None]);
    // Samma namn men olika slag paras inte.
    let b = vec![info(5, "Ruta", ShapeKind::Picture, "", "x.png")];
    assert_eq!(pair(&a, &b), vec![None]);
}

#[test]
fn dubbletter_av_namn_paras_en_till_en_efter_bild_och_text_sedan_ordning() {
    let a = vec![info(1, "Bild", ShapeKind::Picture, "", "a.png"), info(2, "Bild", ShapeKind::Picture, "", "b.png"), info(3, "Bild", ShapeKind::Picture, "", "c.png")];
    let b = vec![info(9, "Bild", ShapeKind::Picture, "", "b.png"), info(8, "Bild", ShapeKind::Picture, "", "z.png")];
    // b.png hittar b.png; den andra tar första lediga i ordning.
    assert_eq!(pair(&a, &b), vec![Some(1), Some(0)]);
    // Tre lika namn mot en: bara en blir par, resten tonas.
    let a = vec![info(1, "T", ShapeKind::Text, "x", ""), info(2, "T", ShapeKind::Text, "x", "")];
    assert_eq!(pair(&a, &[info(4, "T", ShapeKind::Text, "x", "")]), vec![Some(0)]);
}

#[test]
fn utropstecken_paras_bara_mot_samma_namn() {
    let a = vec![info(1, "!!Logga", ShapeKind::Picture, "", "l.png"), info(2, "Bild 1", ShapeKind::Picture, "", "l.png")];
    // `!!`-formen tar inte `Bild 1` även om bilden är densamma, och en vanlig form tar inte `!!`-formen.
    let b = vec![info(5, "!!Annan", ShapeKind::Picture, "", "l.png"), info(6, "Namnlös", ShapeKind::Picture, "", "l.png")];
    assert_eq!(pair(&a, &b), vec![None, Some(1)]);
    let b = vec![info(5, "!!Logga", ShapeKind::Picture, "", "x.png")];
    assert_eq!(pair(&a, &b), vec![Some(0)]);
}

#[test]
fn omdöpta_former_paras_efter_bild_eller_text() {
    let a = vec![info(1, "A", ShapeKind::Picture, "", "foto.jpg"), info(2, "B", ShapeKind::Text, "Samma text", ""), info(3, "C", ShapeKind::Shape, "", "rect")];
    let b = vec![info(4, "X", ShapeKind::Picture, "", "foto.jpg"), info(5, "Y", ShapeKind::Text, "Samma text", ""), info(6, "Z", ShapeKind::Shape, "", "rect")];
    // Tomma, namnlösa former med samma geometri paras inte på måfå.
    assert_eq!(pair(&a, &b), vec![Some(0), Some(1), None]);
}

#[test]
fn rörelsen_är_identitet_i_ändpunkterna_och_halvvägs_i_mitten() {
    let a = Geo { cx: 100.0, cy: 100.0, w: 40.0, h: 20.0, rot: 0.0 };
    let b = Geo { cx: 300.0, cy: 200.0, w: 80.0, h: 20.0, rot: 90.0 };
    assert_eq!(morph_map(&a, &b, 0.0), Affine::IDENTITY);
    let m = morph_map(&b, &a, 0.0);
    assert_eq!(m, Affine::IDENTITY);
    // Mitten av a hamnar halvvägs.
    let (x, y) = morph_map(&a, &b, 0.5).apply(100.0, 100.0);
    assert!((x - 200.0).abs() < 1e-3 && (y - 150.0).abs() < 1e-3, "{x} {y}");
    // Dubbelt så bred på vägen till 80: vid 0.5 är bredden 60, alltså 1.5 gånger; vridningen 45 grader.
    let m = morph_map(&a, &b, 0.5);
    let (x, y) = m.apply(120.0, 100.0);
    let (dx, dy) = (x - 200.0, y - 150.0);
    let r = (dx * dx + dy * dy).sqrt();
    assert!((r - 30.0).abs() < 1e-2, "{r}");
    assert!((dy.atan2(dx).to_degrees() - 45.0).abs() < 1e-2);
    // Vridning går närmaste vägen: 350 -> 10 är 20 grader, inte -340.
    assert!((near(350.0, 10.0) - 370.0).abs() < 1e-4);
}

#[test]
fn genomskinlighet_har_exakta_ändar_och_en_täckande_sida() {
    assert_eq!(pair_alpha(0.0), (1.0, 0.0));
    assert_eq!(pair_alpha(1.0), (0.0, 1.0));
    for i in 0..=20 {
        let (a, b) = pair_alpha(i as f32 / 20.0);
        assert!(a == 1.0 || b == 1.0, "en sida ska täcka: {a} {b}");
    }
}

fn rect(id: u32, name: &str, x: i64, y: i64, w: i64, h: i64, rot: i64, rgb: &str, text: &str) -> String {
    let body = if text.is_empty() { String::new() } else { format!(r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr sz="2000"/><a:t>{text}</a:t></a:r></a:p></p:txBody>"#) };
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="{rot}"><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{rgb}"/></a:solidFill></p:spPr>{body}</p:sp>"#
    )
}

const MORPH: &str = r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:p159="http://schemas.microsoft.com/office/powerpoint/2015/09/main"><mc:Choice Requires="p159"><p:transition spd="slow"><p159:morph option="byObject"/></p:transition></mc:Choice><mc:Fallback><p:transition spd="slow"><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>"#;

/// Bild A: röd ruta uppe till vänster och en blå som försvinner. Bild B: rutan (samma namn) nere till
/// höger, större och vriden, och en grön ny.
fn deck() -> Deck {
    let a = rect(2, "Ruta", 914400, 914400, 1828800, 914400, 0, "FF0000", "") + &rect(3, "Går", 5486400, 914400, 914400, 914400, 0, "0000FF", "");
    let b = rect(2, "Ruta", 5486400, 4114800, 2743200, 1371600, 1800000, "FF0000", "") + &rect(4, "Ny", 914400, 4572000, 914400, 914400, 0, "00AA00", "");
    let bytes = Builder::new().slide("", &a).slide("", &b).tail(MORPH).build();
    Deck::open(&bytes).unwrap()
}

fn side(deck: &Deck, i: usize, w: u32, h: u32) -> Side {
    Side::build(deck, i, &[], w, h)
}

fn diff(a: &sk::Pixmap, b: &sk::Pixmap) -> i32 {
    a.data().iter().zip(b.data()).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap_or(0)
}

/// Andelen bytes som skiljer mer än 8 (kantpunkter där former möts renderas lite olika som lager; i verkligt material högst ett par procent).
fn stray(a: &sk::Pixmap, b: &sk::Pixmap) -> f64 {
    a.data().iter().zip(b.data()).filter(|(x, y)| (**x as i32 - **y as i32).abs() > 8).count() as f64 / a.data().len().max(1) as f64
}

#[test]
fn övergången_läses_som_morph_och_infon_ur_bilden() {
    let d = deck();
    assert_eq!(Transition::of_slide(&d.slides[1].root).kind, Kind::Morph(MorphBy::Object));
    assert_eq!(Transition::of_slide(&d.slides[0].root).kind, Kind::Cut { black: false });
    let i = infos(&d, 1);
    assert_eq!(i.iter().map(|x| (x.id, x.name.as_str())).collect::<Vec<_>>(), vec![(2, "Ruta"), (4, "Ny")]);
    let g = i[0].xfrm.unwrap();
    assert!((g.cx - 540.0).abs() < 0.1 && (g.w - 216.0).abs() < 0.1);
    assert!((g.rot - 30.0).abs() < 1e-3);
}

#[test]
fn ändpunkterna_är_exakt_gamla_och_nya_bilden_och_mitten_är_emellan() {
    let d = deck();
    let (w, h) = (360u32, 270u32);
    let (a, b) = (side(&d, 0, w, h), side(&d, 1, w, h));
    let plan = plan(&a, &b);
    let stat = |i| raster::render(&render::slide_scene(&d, i), w, h).unwrap();
    let p0 = compose(&plan, &a, &b, 0.0, w, h).unwrap();
    let p1 = compose(&plan, &a, &b, 1.0, w, h).unwrap();
    assert!(diff(&p0, &stat(0)) <= 2, "p=0 ska vara bild A: {}", diff(&p0, &stat(0)));
    assert!(diff(&p1, &stat(1)) <= 2, "p=1 ska vara bild B: {}", diff(&p1, &stat(1)));
    // Mitt i: röd ruta runt mitten av vägen (A:s centrum 90,67 -> B:s 450,360 punkter ger 270,214), den
    // blå som går är halvt borta och den gröna halvt inne.
    let mid = compose(&plan, &a, &b, 0.5, w, h).unwrap();
    let px = |m: &sk::Pixmap, x: f32, y: f32| {
        let c = m.pixel((x / 2.0) as u32, (y / 2.0) as u32).unwrap();
        (c.red(), c.green(), c.blue())
    };
    let (r, g, bl) = px(&mid, 270.0, 214.0);
    assert!(r > 200 && g < 60 && bl < 60, "röd ruta i mitten: {r} {g} {bl}");
    // Där A:s ruta låg är det inte längre rött (rutan har flyttat sig).
    let (r, g, _) = px(&mid, 40.0, 40.0);
    assert!(!(r > 200 && g < 60), "gamla platsen ska vara tom vid mitten");
    // Den blå fadas: vid mitten blandas den med vitt (inte helt blå, inte helt vit).
    let (r, _, bl) = px(&mid, 5486400.0 / 12700.0 + 36.0, 914400.0 / 12700.0 + 36.0);
    assert!(bl > 200 && r > 60 && r < 220, "blå halvvägs bort: {r} {bl}");
    // Alla mellanbilder är ändliga och ändras jämnt (ingen hoppar mer än en bråkdel mellan närliggande p).
    let mut last = compose(&plan, &a, &b, 0.0, w, h).unwrap();
    for k in 1..=40 {
        let f = compose(&plan, &a, &b, k as f32 / 40.0, w, h).unwrap();
        let mean = last.data().iter().zip(f.data()).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / last.data().len() as f64;
        assert!(mean < 8.0, "hoppar vid {k}: {mean}");
        last = f;
    }
}

#[test]
fn plan_utan_par_tonar_ut_och_in() {
    let d = deck();
    let (a, b) = (side(&d, 0, 72, 54), side(&d, 1, 72, 54));
    let plan = plan(&a, &b);
    let ops = plan.ops(0.5);
    let e = ease(0.5);
    // Den som går (id 3) tonar ut och den nya (id 4) tonar in.
    let gone = ops.iter().find(|o| o.which == Which::Old && o.shape == Some(3)).unwrap();
    let new = ops.iter().find(|o| o.which == Which::New && o.shape == Some(4)).unwrap();
    assert!((gone.alpha - (1.0 - e)).abs() < 1e-6 && (new.alpha - e).abs() < 1e-6);
    // Paret (id 2) ritas två gånger.
    assert_eq!(ops.iter().filter(|o| o.shape == Some(2)).count(), 2);
    // Vid 0 syns bara gamla bilden.
    assert!(plan.ops(0.0).iter().all(|o| o.which == Which::Old || o.alpha == 0.0));
}

#[test]
fn bild_utan_morph_påverkas_inte() {
    let d = Deck::open(&Builder::new().slide("", &rect(2, "R", 0, 0, 914400, 914400, 0, "FF0000", "")).slide("", "").tail(r#"<p:transition><p:fade/></p:transition>"#).build()).unwrap();
    assert_eq!(Transition::of_slide(&d.slides[1].root).kind, Kind::Fade { black: false });
}

/// Alla morph-övergångarna i materialet (`KLADD_PPTX_REFS`, mappar med `deck.pptx`): lagren renderas, planen
/// bygger och bildrutorna är ändliga, och ändpunkterna är bild A och B.
/// `KLADD_PPTX_REFS=… cargo test morph -- --ignored --nocapture`.
#[test]
#[ignore]
fn referensmaterialets_morph() {
    let Ok(dir) = std::env::var("KLADD_PPTX_REFS") else { return };
    let mut decks: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path().join("deck.pptx")).filter(|p| p.exists()).collect();
    decks.sort();
    let (mut total, mut paired, mut shapes, mut bad_end) = (0usize, 0usize, 0usize, 0usize);
    let w = 192u32;
    for p in decks {
        let Ok(deck) = Deck::open(&std::fs::read(&p).unwrap()) else { continue };
        let (sw, sh) = deck.size_pt();
        let hh = ((w as f32) * sh / sw).round() as u32;
        for i in 1..deck.slides.len() {
            if !matches!(Transition::of_slide(&deck.slides[i].root).kind, Kind::Morph(_)) {
                continue;
            }
            total += 1;
            let (a, b) = (Side::build(&deck, i - 1, &[], w, hh), Side::build(&deck, i, &[], w, hh));
            let plan = plan(&a, &b);
            let pairs = pair(&a.infos, &b.infos).iter().flatten().count();
            paired += pairs;
            shapes += b.infos.len();
            for k in 0..=4 {
                let f = compose(&plan, &a, &b, k as f32 / 4.0, w, hh).unwrap();
                assert!(f.data().len() == (w * hh * 4) as usize);
            }
            let stat = |j| raster::render(&render::slide_scene(&deck, j), w, hh).unwrap();
            let (e0, e1) = (stray(&compose(&plan, &a, &b, 0.0, w, hh).unwrap(), &stat(i - 1)), stray(&compose(&plan, &a, &b, 1.0, w, hh).unwrap(), &stat(i)));
            if e0 > 0.02 || e1 > 0.02 {
                bad_end += 1;
                println!("{} bild {}: ändpunkterna avviker {e0} / {e1}", p.display(), i + 1);
            }
        }
    }
    println!("morph-övergångar {total}; former på nya bilderna {shapes}, paren {paired}; avvikande ändpunkter {bad_end}");
    if std::env::var("KLADD_PPTX_REFS").is_ok() {
        assert_eq!(total, 65, "alla 65 morph-övergångarna i materialet ska hittas");
    }
    assert_eq!(bad_end, 0);
}
