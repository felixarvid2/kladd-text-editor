//! Prov för fotnoter längst ned på sin sida: sidbrytning med reserverad plats, flera noter,
//! sidbrytning, spalter och markörmappningen när definitionsraderna är dolda i flödet.

use std::sync::Arc;

use egui::epaint::text::Galley;
use egui::text::CCursor;
use egui::{Color32, FontId};

use super::fnotes::NoteLayout;
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
    notes: Arc<Vec<NoteLayout>>,
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
            let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
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
            out = Some(Laid { g, boxes: cache.page_boxes(1, text), notes: cache.notes(1, text), text: text.to_owned() });
        });
        std::mem::forget(r);
    }
    out.unwrap()
}

fn para(i: usize) -> String {
    format!("{} {i}\n\n", "Brödtext som fyller raderna på sidan och fortsätter lite till så att stycket blir flera rader långt.".repeat(3))
}

fn filler(n: usize) -> String {
    (0..n).map(para).collect()
}

fn at(l: &Laid, needle: &str) -> egui::Rect {
    let b = l.text.find(needle).expect(needle);
    l.g.pos_from_cursor(CCursor::new(l.text[..b].chars().count()))
}

fn page_of(l: &Laid, needle: &str) -> usize {
    page_index(&l.boxes, at(l, needle).min.y)
}

/// Raderna är i stigande ordning och ingen synlig rad korsar notytan eller sidans nederkant.
fn check_rows(l: &Laid) {
    check_rows_cols(l, false)
}

fn check_rows_cols(l: &Laid, columns: bool) {
    assert!(columns || l.g.rows.windows(2).all(|w| w[0].pos.y <= w[1].pos.y + 0.01), "raderna är inte i stigande y-ordning");
    for r in &l.g.rows {
        if r.row.size.y < 1.0 {
            continue; // dolda rader (definitioner, taggar)
        }
        let b = &l.boxes[page_index(&l.boxes, r.pos.y)];
        assert!(r.pos.y + r.row.size.y <= b.notes_top() + 0.6, "raden på y={} når in i notytan ({})", r.pos.y, b.notes_top());
    }
}

#[test]
fn fotnoten_ligger_pa_referensens_sida_och_plats_reserveras() {
    // Referensen står sist i ett stycke på andra sidan.
    let t = format!("{}Referens här[^1].\n\n{}\n[^1]: Fotnotens text.\n", filler(16), filler(16));
    let l = lay(&t);
    assert!(l.boxes.len() >= 3, "{}", l.boxes.len());
    let p = page_of(&l, "Referens här");
    assert!(p >= 1, "referensen ska ligga på en senare sida");
    assert_eq!(l.notes.len(), 1);
    for (k, b) in l.boxes.iter().enumerate() {
        if k == p {
            assert_eq!(b.notes, vec![0]);
            assert!(b.reserved > l.notes[0].galley.rect.height(), "plats för noten och skiljelinjen");
            assert!(b.notes_top() < b.text_bottom());
        } else {
            assert!(b.notes.is_empty() && b.reserved == 0.0, "sida {k}");
        }
    }
    check_rows(&l);
    // Definitionsraden är hopkrympt: hela dokumentet har lika många sidor som utan definitionen.
    let utan = lay(&format!("{}Referens här.\n\n{}", filler(16), filler(16)));
    assert_eq!(l.boxes.len(), utan.boxes.len());
}

#[test]
fn flera_fotnoter_pa_samma_sida_delar_utrymmet() {
    let t = format!("Första[^1] och andra[^2] och tredje[^3].\n\n{}[^1]: Ett.\n[^2]: Två.\n[^3]: Tre.\n", filler(2));
    let l = lay(&t);
    assert_eq!(l.boxes[0].notes, vec![0, 1, 2]);
    let want: f32 = l.notes.iter().map(|n| n.galley.rect.height() + 2.0 * 96.0 / 72.0).sum::<f32>() + 9.0 * 96.0 / 72.0;
    assert!((l.boxes[0].reserved - want).abs() < 1.0, "{} != {want}", l.boxes[0].reserved);
    check_rows(&l);
}

#[test]
fn sidbrytning_ger_noterna_var_sin_sida() {
    let t = format!(
        "Före[^1].\n\n{}\nEfter[^2].\n\n{}\n[^1]: Ett.\n[^2]: Två.\n",
        markup_break(),
        "Mer text.\n"
    );
    let l = lay(&t);
    assert!(l.boxes.len() >= 2);
    assert_eq!(l.boxes[0].notes, vec![0]);
    assert_eq!(l.boxes[1].notes, vec![1]);
    check_rows(&l);
}

fn markup_break() -> &'static str {
    "<!-- kladd: sidbrytning -->\n"
}

#[test]
fn markormappningen_foljer_de_dolda_definitionsraderna() {
    let t = format!("{}Ref[^1] och[^2].\n\n{}\n[^1]: Första noten.\n[^2]: Andra noten.\n", filler(4), filler(8));
    let l = lay(&t);
    check_rows(&l);
    let n = l.g.rows.len();
    // Varje teckenposition ger en rektangel som träffar tillbaka på samma rad (nära nog), och y växer med positionen.
    let mut last_y = f32::NEG_INFINITY;
    for (ci, _) in t.char_indices().step_by(7) {
        let c = CCursor::new(t[..ci].chars().count());
        let r = l.g.pos_from_cursor(c);
        assert!(r.top() >= last_y - 0.01, "markören hoppar bakåt vid {ci}");
        last_y = r.top();
        if r.height() < 1.0 {
            continue;
        }
        let back = l.g.pos_from_cursor(l.g.cursor_from_pos(r.center().to_vec2() + egui::vec2(1.0, 0.0)));
        assert!((back.top() - r.top()).abs() < 0.01, "klick vid {ci}");
    }
    // Definitionsraderna sist är kollapsade till (nästan) ingen höjd och ligger efter all synlig text.
    let def = at(&l, "Första noten");
    assert!(def.height() < 1.0, "{}", def.height());
    let last_visible = l.g.rows.iter().filter(|r| r.row.size.y >= 1.0).map(|r| r.pos.y + r.row.size.y).fold(0.0, f32::max);
    assert!(def.top() >= last_visible - 2.0, "{} < {last_visible}", def.top());
    assert!(n > 10);
}

#[test]
fn slutnoter_och_foraldralosa_definitioner_star_kvar_sist() {
    let t = format!("Text[^1] med slutnot[^ei].\n\n{}[^1]: Fotnot.\n\n[^ei]: Slutnoten.\n\n[^9]: Föräldralös.\n", filler(1));
    let l = lay(&t);
    assert_eq!(l.notes.len(), 1, "bara fotnoten med referens flyttas");
    assert_eq!(l.notes[0].label, "1");
    // Slutnoten och den föräldralösa definitionen har riktig radhöjd.
    assert!(at(&l, "Slutnoten").height() > 8.0);
    assert!(at(&l, "Föräldralös").height() > 8.0);
    assert!(at(&l, "Fotnot.").height() < 1.0);
}

#[test]
fn spalter_reserverar_plats_over_hela_sidans_bredd() {
    let t = format!("<!-- kladd: avsnitt; columns: 2 -->\n\nNot här[^1].\n\n{}[^1]: Spaltnot.\n", filler(30));
    let l = lay(&t);
    assert!(l.boxes[0].reserved > 0.0);
    assert_eq!(l.boxes[0].notes, vec![0]);
    // Ingen synlig rad (i någon spalt) når in i notytan.
    check_rows_cols(&l, true);
    // Spalterna fylls fortfarande: minst två sidor och text i båda spalterna på första sidan.
    let xs: std::collections::BTreeSet<i32> = l.g.rows.iter().filter(|r| r.row.size.y >= 1.0 && r.pos.y < 400.0).map(|r| r.pos.x as i32 / 50).collect();
    assert!(xs.len() >= 2, "{xs:?}");
}

#[test]
fn sidantal_paverkas_inte_av_definitionerna() {
    let med = lay(&format!("{}Ref[^1].\n\n[^1]: Not.\n", filler(6)));
    let utan = lay(&format!("{}Ref.\n\n", filler(6)));
    assert_eq!(med.boxes.len(), utan.boxes.len());
    // Sista sidan är inte en tom sida med bara dolda rader.
    let last = med.boxes.len() - 1;
    assert!(med.g.rows.iter().any(|r| r.row.size.y >= 1.0 && page_index(&med.boxes, r.pos.y) == last));
}
