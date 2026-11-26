//! Prov för stegvis utläggning (`layout_lazy`): resultatet ska bli exakt detsamma som när allt läggs ut
//! på en gång, och en redigering ska bara lägga om stycket som ändrats.

use std::sync::Arc;
use std::time::Duration;

use egui::epaint::text::Galley;
use egui::{Color32, FontId};

use super::layout::{Cache, Params};
use super::markup;
use super::pages::Geom;
use crate::theme;

fn fam(_: Option<&str>, bold: bool, italic: bool) -> egui::FontFamily {
    theme::doc_font(bold, italic)
}

struct Rig {
    ctx: egui::Context,
    cache: Cache,
}

impl Rig {
    fn new() -> Self {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        Self { ctx, cache: Cache::default() }
    }

    /// En bildruta; ger galleriet och de sidor/fotnoter som hör till det.
    fn frame(&mut self, text: &str, budget: Option<Duration>) -> Arc<Galley> {
        let pal = theme::palette(false);
        let img = |_: &str| None;
        let a = markup::analyze(text);
        let geom = Geom::for_sections(1.0, &a.defaults, &a.sections);
        let (ctx, cache) = (&self.ctx, &mut self.cache);
        let mut out = None;
        let r = ctx.run_ui(Default::default(), |_| {
            let measure =
                |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
            let p = Params {
                palette: &pal,
                zoom: 1.0,
                reveal: 0..0,
                matches: &[],
                image_size: &img,
                measure: &measure,
                font_family: &fam,
                fonts_gen: 0,
            };
            out = Some(cache.galley_paged_lazy(ctx, 1, text, &p, geom.edit_w(), Some(&geom), budget));
        });
        std::mem::forget(r);
        out.unwrap()
    }
}

/// Dokument med rubriker, brödtext, fotnoter, en lista, en tabell och ett tvåspaltsavsnitt.
fn document(paras: usize) -> String {
    let mut t = String::from("# Rapport\n\n");
    for i in 0..paras {
        if i % 25 == 0 {
            t.push_str(&format!("## Avsnitt {i}\n\n"));
        }
        let note = if i % 10 == 3 { format!("[^{}]", i / 10 + 1) } else { String::new() };
        t.push_str(&format!(
            "Stycke {i}{note} med **fet** och *kursiv* text som fyller raderna på sidan och fortsätter lite till så att stycket blir flera rader långt, och lite till.\n\n"
        ));
        if i == paras / 3 {
            t.push_str("- punkt ett\n- punkt två\n- punkt tre\n\n");
            t.push_str("| Namn | Värde |\n| - | - |\n| a | 1 |\n| b | 2 |\n\n");
        }
        if i == paras / 2 {
            t.push_str("<!-- kladd: avsnitt; columns: 2; type: continuous -->\n\n");
        }
        if i == paras / 2 + 20 {
            t.push_str("<!-- kladd: avsnitt; type: continuous -->\n\n");
        }
    }
    for n in 1..=paras / 10 {
        t.push_str(&format!("[^{n}]: Fotnot nummer {n} med lite text.\n\n"));
    }
    t
}

/// Anropar tills utläggningen är klar; ger det sista galleriet och antalet bildrutor.
fn until_done(rig: &mut Rig, text: &str, budget: Duration) -> (Arc<Galley>, usize) {
    for n in 1..2000 {
        let g = rig.frame(text, Some(budget));
        if !rig.cache.partial(1) {
            return (g, n);
        }
    }
    panic!("utläggningen blev aldrig klar");
}

#[test]
fn stegvis_utlaggning_ger_samma_galleri_som_allt_pa_en_gang() {
    let text = document(260);
    let all = Rig::new().frame(&text, None);
    let mut rig = Rig::new();
    let (g, frames) = until_done(&mut rig, &text, Duration::ZERO);
    assert!(frames > 2, "budgeten ska ha delat upp utläggningen ({frames} bildrutor)");
    assert_eq!(g.rows.len(), all.rows.len());
    assert!(*g == *all, "galleriet efter stegvis utläggning skiljer sig från det som lades ut direkt");
    // Sidorna och fotnoterna också.
    assert_eq!(rig.cache.page_boxes(1, &text).len(), Rig::new().cache_boxes(&text));
}

impl Rig {
    fn cache_boxes(&mut self, text: &str) -> usize {
        self.frame(text, None);
        self.cache.page_boxes(1, text).len()
    }
}

#[test]
fn ofullstandigt_galleri_har_ratt_prefix_och_en_rimlig_hojd() {
    let text = document(260);
    let all = Rig::new().frame(&text, None);
    let mut rig = Rig::new();
    let part = rig.frame(&text, Some(Duration::ZERO));
    assert!(rig.cache.partial(1), "dokumentet är för stort för en bildruta med budget noll");
    let row = rig.cache.lazy_row(1).expect("gräns för utlagda rader");
    assert!(row > 0 && row < all.rows.len());
    // Det utlagda är identiskt, platshållarna ger ungefär rätt höjd.
    for i in 0..row {
        assert_eq!(part.rows[i].pos, all.rows[i].pos, "rad {i}");
        assert_eq!(part.rows[i].row.glyphs.len(), all.rows[i].row.glyphs.len(), "rad {i}");
    }
    let (h, h_all) = (part.rect.height(), all.rect.height());
    assert!((h / h_all - 1.0).abs() < 0.4, "uppskattad höjd {h} mot verklig {h_all}");
    assert!(rig.cache.progress(1) < 1.0);
    // Ett galleri utan budget är alltid komplett, även mitt i en stegvis utläggning.
    let g = rig.frame(&text, None);
    assert!(!rig.cache.partial(1));
    assert!(*g == *all);
}

#[test]
fn utan_budget_och_med_stor_budget_blir_allt_utlagt_direkt() {
    let text = document(60);
    let mut rig = Rig::new();
    rig.frame(&text, Some(Duration::from_secs(60)));
    assert!(!rig.cache.partial(1));
    assert!(rig.cache.stats(1).stubs == 0);
    // Små dokument får aldrig någon budget.
    assert_eq!(super::layout::lazy_budget(10_000, 0, false, false), None);
    assert_eq!(super::layout::lazy_budget(500_000, 0, false, true), None);
    assert!(super::layout::lazy_budget(500_000, 0, false, false).is_some());
    // Budgeten växer men har tak.
    let b = |f| super::layout::lazy_budget(500_000, f, false, false).unwrap();
    assert!(b(0) < b(2) && b(3) == b(9));
}

#[test]
fn redigering_lagger_bara_om_stycket_som_andrats() {
    let text = document(260);
    let mut rig = Rig::new();
    rig.frame(&text, None);
    let cold = rig.cache.stats(1);
    assert!(cold.misses > 200, "kall utläggning: {} missar", cold.misses);
    // Ett tecken i mitten.
    let mid = text[..text.len() / 2].rfind("Stycke").unwrap();
    let mut edited = text.clone();
    edited.insert(mid + 8, 'x');
    rig.frame(&edited, None);
    let s = rig.cache.stats(1);
    assert!(s.misses <= 2, "ett tecken ska bara ge ett omlagt stycke, fick {}", s.misses);
    assert!(s.hits + s.misses >= s.groups, "alla grupper ska ha gåtts igenom");
    // Och i början (efter rubriken).
    let mut edited2 = edited.clone();
    edited2.insert(edited2.find("Stycke 0").unwrap() + 3, 'y');
    rig.frame(&edited2, None);
    let s = rig.cache.stats(1);
    assert!(s.misses <= 2, "fick {}", s.misses);
    // Oförändrat: ingenting läggs om.
    rig.frame(&edited2, None);
    assert_eq!(rig.cache.stats(1).misses, s.misses);
}

#[test]
fn redigering_under_pagaende_utlaggning_ger_ratt_galleri() {
    let text = document(260);
    let mut edited = text.clone();
    let at = edited.find("Stycke 5").unwrap() + 3;
    edited.insert(at, 'z');
    let all = Rig::new().frame(&edited, None);
    let mut rig = Rig::new();
    rig.frame(&text, Some(Duration::ZERO));
    assert!(rig.cache.partial(1));
    let (g, _) = until_done(&mut rig, &edited, Duration::ZERO);
    // Glyfernas strukturkoordinater (`uv_rect`) beror på i vilken ordning tecken lagts in i typsnittsatlasen: jämför
    // utläggningen (rader, lägen och glyfer med läge, tecken och bredd).
    assert!(same_layout(&g, &all), "galleriet efter ändring under utläggning skiljer sig");
}

/// Gen-skolrapporten (rubriker, tabeller, bilder, fotnoter, tvåspaltsavsnitt, textrutor) upprepad `n` gånger,
/// med fotnotsetiketterna numrerade om.
fn skolrapport(n: usize) -> String {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus/docx/gen-skolrapport.docx")).unwrap();
    let md = crate::docx::read(&bytes).unwrap().markdown;
    let head_end = md.find('\n').unwrap() + 1;
    let (head, body) = md.split_at(head_end);
    let mut t = head.to_owned();
    for k in 0..n {
        t.push_str(&body.replace("[^", &format!("[^r{k}x")));
        t.push('\n');
    }
    t
}

/// Det uppskattade sidantalet under den stegvisa utläggningen (`Cache::estimated_pages`: utlagda sidor delat med den
/// del av texten de utgör) ska ligga nära det slutliga och inte hoppa mellan bildrutorna.
#[test]
fn uppskattat_sidantal_ligger_nara_det_slutliga() {
    let text = skolrapport(16);
    let mut rig = Rig::new();
    let all = rig.frame(&text, None);
    let a = markup::analyze(&text);
    let geom = Geom::for_sections(1.0, &a.defaults, &a.sections);
    let final_pages = match rig.cache.page_boxes(1, &text).len() {
        0 => super::pages::page_count(&all, &geom),
        n => n,
    };
    let mut rig = Rig::new();
    let mut est = Vec::new();
    let mut frames = 0;
    for _ in 0..400 {
        let g = rig.frame(&text, Some(Duration::ZERO));
        frames += 1;
        let boxes = rig.cache.page_boxes(1, &text);
        if let Some(n) = rig.cache.estimated_pages(1, text.len(), &g, &boxes) {
            est.push(n);
        }
        if !rig.cache.partial(1) {
            break;
        }
    }
    assert!(frames > 5 && est.len() > 5, "utläggningen ska ta flera bildrutor ({frames}) med uppskattning ({})", est.len());
    let off = |n: usize| (n as f32 / final_pages as f32 - 1.0).abs();
    assert!(est.iter().all(|n| off(*n) < 0.2), "uppskattningar {est:?} mot {final_pages} sidor");
    assert!(off(est[0]) < 0.2);
    assert!(est.windows(2).all(|w| (w[1] as f32 / w[0] as f32 - 1.0).abs() < 0.2), "hoppar mellan bildrutor: {est:?}");
}

/// Samma utläggning (rader, lägen, glyfer): `Galley ==` jämför också atlaskoordinater som beror på inläggningsordningen.
fn same_layout(a: &Galley, b: &Galley) -> bool {
    a.rows.len() == b.rows.len()
        && a.rect == b.rect
        && a.rows.iter().zip(&b.rows).all(|(x, y)| {
            x.pos == y.pos
                && x.row.size == y.row.size
                && x.ends_with_newline == y.ends_with_newline
                && x.row.glyphs.len() == y.row.glyphs.len()
                && x.row.glyphs.iter().zip(&y.row.glyphs).all(|(g, h)| g.pos == h.pos && g.chr == h.chr && g.advance_width == h.advance_width)
        })
}

/// Första bildrutan efter öppning: när bilderna blir kända behålls analysen (den tolkas inte om); efter en
/// utläggning kasseras allt som förr.
#[test]
fn image_sizes_keep_the_analysis_until_something_is_laid_out() {
    let text = document(30);
    let mut rig = Rig::new();
    let before = rig.cache.analysis(1, &text);
    rig.cache.images_sized(1);
    assert!(Arc::ptr_eq(&before, &rig.cache.analysis(1, &text)), "analysen tolkades om före första utläggningen");
    rig.frame(&text, None);
    let laid = rig.cache.analysis(1, &text);
    rig.cache.images_sized(1);
    assert!(!Arc::ptr_eq(&laid, &rig.cache.analysis(1, &text)), "utläggningen kasserades inte");
    // Okänt dokument: inget att göra.
    rig.cache.images_sized(99);
}
