//! Prov för avsnitt i dokumentläget: sidstorlek per sida, spaltflöde och löpande avsnitt.

use std::sync::Arc;

use egui::epaint::text::{Galley, PlacedRow};
use egui::{Color32, FontId};

use super::layout::{Cache, Params};
use super::markup;
use super::pages::Geom;
use super::secpages::{PageBox, page_index};
use crate::theme;

fn fam(_: Option<&str>, bold: bool, italic: bool) -> egui::FontFamily {
    theme::doc_font(bold, italic)
}

struct Laid {
    g: Arc<Galley>,
    boxes: Arc<Vec<PageBox>>,
    geom: Geom,
    geoms: Arc<Vec<super::grid::TableGeom>>,
    text: String,
}

fn lay(text: &str) -> Laid {
    let pal = theme::palette(false);
    let img = |_: &str| None;
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let a = markup::analyze(text);
    let geom = Geom::for_sections(1.0, &a.defaults, &a.sections);
    let mut cache = Cache::default();
    let mut out = None;
    for _ in 0..2 {
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
            let g = cache.galley_paged(&ctx, 1, text, &p, geom.edit_w(), Some(&geom));
            let boxes = cache.page_boxes(1, text);
            let geoms = cache.table_view(1, text).map(|(_, t)| t).unwrap_or_default();
            out = Some(Laid { g, boxes, geom: geom.clone(), geoms, text: text.to_owned() });
        });
        std::mem::forget(r);
    }
    out.unwrap()
}

fn filler(n: usize) -> String {
    let p = "Brödtext som fyller raderna i avsnittet och fortsätter lite till så att stycket blir flera rader långt. ".repeat(3);
    (0..n).map(|i| format!("{p}{i}\n\n")).collect()
}

#[allow(dead_code)]
fn dump(l: &Laid) -> String {
    l.g.rows.iter().enumerate().map(|(i, r)| format!("{i}: x={:.1} y={:.1} h={:.1} w={:.1}", r.pos.x, r.pos.y, r.row.size.y, r.row.size.x)).collect::<Vec<_>>().join("\n")
}

fn pos_of(l: &Laid, needle: &str) -> egui::Rect {
    let at = l.text.find(needle).expect(needle);
    l.g.pos_from_cursor(egui::text::CCursor::new(l.text[..at].chars().count()))
}

fn page_of_text(l: &Laid, needle: &str) -> usize {
    page_index(&l.boxes, pos_of(l, needle).min.y)
}

fn col_w_at(l: &Laid, r: &PlacedRow) -> f32 {
    l.boxes[page_index(&l.boxes, r.pos.y)].region_at(r.pos.y).col_w
}

#[test]
fn plain_document_keeps_uniform_pages() {
    let l = lay(&filler(40));
    assert!(!l.geom.multi());
    assert!(l.boxes.len() >= 3);
    assert!(l.boxes.iter().all(|b| (b.w - l.geom.page_w).abs() < 0.01 && (b.h - l.geom.page_h).abs() < 0.01));
    assert!((l.boxes[1].top - l.boxes[0].top - l.geom.stride()).abs() < 0.01);
}

#[test]
fn landscape_section_gets_its_own_page_size() {
    let t = format!(
        "Första sidan\n\n<!-- kladd: avsnitt; page: A4 landscape -->\n\nLiggande sidan\n\n{}<!-- kladd: avsnitt -->\n\nTillbaka\n",
        filler(30)
    );
    let l = lay(&t);
    assert!(l.geom.multi());
    let sizes: Vec<(f32, f32)> = l.boxes.iter().map(|b| (b.w.round(), b.h.round())).collect();
    let (pw, ph) = (794.0, 1123.0);
    let (lw, lh) = (1123.0, 794.0);
    assert_eq!(sizes[0], (pw, ph), "{sizes:?}");
    assert_eq!(sizes[1], (lw, lh), "{sizes:?}");
    // Det liggande avsnittet är flera sidor; sedan åter stående.
    let n_land = sizes.iter().filter(|s| **s == (lw, lh)).count();
    assert!(n_land >= 2, "{sizes:?}");
    assert_eq!(*sizes.last().unwrap(), (pw, ph), "{sizes:?}");
    // Varje avsnitt börjar på en ny sida.
    assert_eq!(page_of_text(&l, "Första sidan"), 0);
    assert_eq!(page_of_text(&l, "Liggande sidan"), 1);
    assert_eq!(page_of_text(&l, "Tillbaka"), l.boxes.len() - 1);
    // Sidorna ligger under varandra utan att överlappa, och är centrerade på samma mittlinje.
    for w in l.boxes.windows(2) {
        assert!(w[1].top >= w[0].top + w[0].h, "ark överlappar");
        assert!(((w[0].left + w[0].w / 2.0) - (w[1].left + w[1].w / 2.0)).abs() < 0.01);
    }
    // Textraderna ryms på sina sidor, inom textytan.
    for r in &l.g.rows {
        let b = &l.boxes[page_index(&l.boxes, r.pos.y)];
        assert!(r.pos.y + r.row.size.y <= b.text_bottom() + 0.5, "raden korsar sidans nederkant");
        let tl = b.text_left() - l.geom.origin_x();
        assert!(r.pos.x >= tl - 0.5 && r.pos.x <= tl + (b.w - b.margin[1] - b.margin[3]) + 0.5, "rad utanför textytan: {}", r.pos.x);
    }
    // Hela dokumentets text finns kvar i galleriet.
    let chars: usize = l.g.rows.iter().map(|r| r.char_count_including_newline().0).sum();
    assert_eq!(chars, t.chars().count());
}

#[test]
fn two_columns_flow_down_then_across_then_next_page() {
    let t = format!("<!-- kladd: avsnitt; columns: 2; column-gap: 12mm -->\n\n{}", filler(60));
    let l = lay(&t);
    assert_eq!(l.geom.secs.len(), 1);
    assert!(l.boxes.len() >= 2, "{}", l.boxes.len());
    let b0 = &l.boxes[0];
    let r = b0.region_at(b0.text_top());
    assert_eq!(r.cols, 2);
    let right_x = b0.text_left() + r.col_w + r.gap - l.geom.origin_x();
    let first_rows: Vec<(usize, &PlacedRow)> =
        l.g.rows.iter().enumerate().filter(|(_, r)| page_index(&l.boxes, r.pos.y) == 0).collect();
    let left: Vec<_> = first_rows.iter().filter(|(_, r)| r.pos.x < right_x - 1.0).collect();
    let right: Vec<_> = first_rows.iter().filter(|(_, r)| r.pos.x >= right_x - 1.0).collect();
    assert!(!left.is_empty() && !right.is_empty(), "båda spalterna ska ha text på sida 1");
    let top = b0.text_top();
    assert!((left[0].1.pos.y - top).abs() < 1.0);
    assert!((right[0].1.pos.y - top).abs() < 1.0, "högra spalten börjar överst");
    // Vänstra spalten är full innan den högra tar vid, och raderna ligger tätt nedåt.
    let bottom_left = left.iter().map(|(_, r)| r.pos.y + r.row.size.y).fold(0.0, f32::max);
    assert!(bottom_left > b0.text_bottom() - 60.0, "spalt 1 fylls nästan till nederkanten");
    assert!(left.windows(2).all(|w| w[0].1.pos.y <= w[1].1.pos.y));
    assert!(right.windows(2).all(|w| w[0].1.pos.y <= w[1].1.pos.y));
    // Läsordningen: vänstra spalten före den högra, sedan nästa sida.
    assert!(left.last().unwrap().0 < right[0].0);
    let second = l.g.rows.iter().position(|r| page_index(&l.boxes, r.pos.y) == 1).unwrap();
    assert!(second > right.last().unwrap().0);
    // Raderna är högst en spalt breda.
    for r in &l.g.rows {
        assert!(r.row.size.x <= col_w_at(&l, r) + 1.0, "rad bredare än spalten");
    }
    // Sidnumren fortsätter: sista stycket ligger på sista sidan.
    assert_eq!(page_of_text(&l, "långt. 59"), l.boxes.len() - 1);
}

#[test]
fn column_break_moves_to_next_column() {
    let t = "<!-- kladd: avsnitt; columns: 2 -->\n\nEtt\n\nTvå\n\n<!-- kladd: spaltbrytning -->\n\nTre\n\nFyra\n";
    let l = lay(t);
    assert_eq!(l.boxes.len(), 1);
    let b = &l.boxes[0];
    let r = b.region_at(b.text_top());
    let right_x = b.text_left() + r.col_w + r.gap - l.geom.origin_x();
    assert!(pos_of(&l, "Ett").min.x < right_x - 1.0);
    assert!(pos_of(&l, "Två").min.x < right_x - 1.0);
    assert!(pos_of(&l, "Tre").min.x >= right_x - 1.0, "efter spaltbrytningen: högra spalten");
    assert!(pos_of(&l, "Fyra").min.x >= right_x - 1.0);
    // Första raden i högra spalten ligger överst.
    assert!((pos_of(&l, "Tre").min.y - b.text_top()).abs() < 2.0, "{} {}\n{}", pos_of(&l, "Tre").min.y, b.text_top(), dump(&l));
}

#[test]
fn continuous_section_changes_columns_on_the_same_page() {
    let t = "Rubrik över hela bredden\n\n<!-- kladd: avsnitt; columns: 2; type: continuous -->\n\nTvå spalter ett\n\nTvå spalter två\n\n<!-- kladd: avsnitt; type: continuous -->\n\nSlut över hela bredden\n";
    let l = lay(t);
    assert_eq!(l.boxes.len(), 1, "löpande avsnitt delar sida");
    let regs = &l.boxes[0].regions;
    assert_eq!(regs.len(), 3, "{regs:?}");
    assert_eq!((regs[0].cols, regs[1].cols, regs[2].cols), (1, 2, 1));
    assert!(regs[0].y0 < regs[1].y0 && regs[1].y0 < regs[2].y0);
    // Spalterna jämnas ut: andra stycket hamnar i högra spalten bredvid det första.
    let (a, b) = (pos_of(&l, "Två spalter ett"), pos_of(&l, "Två spalter två"));
    assert!(b.min.x > a.min.x + 50.0, "{a:?} {b:?}");
    assert!((a.min.y - b.min.y).abs() < 2.0, "bredvid varandra\n{}", dump(&l));
    // Sista avsnittet ligger under spalterna.
    assert!(pos_of(&l, "Slut över").min.y > a.min.y + 10.0);
    assert!(pos_of(&l, "Slut över").min.y >= regs[2].y0 - 0.5);
}

#[test]
fn tables_are_clamped_to_the_column() {
    let t = "<!-- kladd: avsnitt; columns: 2 -->\n\n| Rubrik ett är ganska lång | Rubrik två är också lång | Tre |\n| - | - | - |\n| a b c d e f g h i j k l m n o p | q r s t u v w x y z å ä ö | x |\n";
    let l = lay(t);
    let tg = &l.geoms[0];
    let width = tg.col_x.last().unwrap() - tg.col_x[0];
    let b = &l.boxes[0];
    let col_w = b.region_at(b.text_top()).col_w;
    assert!(width <= col_w + 1.0, "tabellen {width} bredare än spalten {col_w}");
}

#[test]
fn page_numbers_continue_over_sections() {
    let t = format!("{}<!-- kladd: avsnitt; page: A5 -->\n\n{}", filler(20), filler(20));
    let l = lay(&t);
    let n = l.boxes.len();
    assert!(n >= 4);
    // Alla sidor numreras löpande: sista raden ligger på sista sidan.
    assert_eq!(page_index(&l.boxes, l.g.rows.last().unwrap().pos.y), n - 1);
    assert!(l.boxes.iter().any(|b| (b.w - 419.53 * 96.0 / 72.0).abs() < 1.0));
}
