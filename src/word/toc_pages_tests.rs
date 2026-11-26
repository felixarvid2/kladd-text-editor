//! Innehållsförteckningens sidnummer: dokumentläget och PDF-utläggningen (`pdf_write::toc_laid_pages`, som
//! också ger sidnumren i .docx och .odt) ska ge samma sida för varje rubrik.

use egui::text::CCursor;
use egui::{Color32, FontId};

use crate::word::layout::{Cache, Params};
use crate::word::{markup, nav, pages, secpages};
use crate::theme;

fn fam(_: Option<&str>, bold: bool, italic: bool) -> egui::FontFamily {
    theme::doc_font(bold, italic)
}

/// Rubrikernas (nivå 1–3) sidor (från 1) i dokumentläget.
fn view_pages(text: &str) -> Vec<(String, usize)> {
    let pal = theme::palette(false);
    let img = |_: &str| None;
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let a = markup::analyze(text);
    let geom = pages::Geom::for_sections(1.0, &a.defaults, &a.sections);
    let mut cache = Cache::default();
    let mut out = Vec::new();
    for _ in 0..2 {
        let r = ctx.run_ui(Default::default(), |_| {
            let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
            let p = Params { palette: &pal, zoom: 1.0, reveal: 0..0, matches: &[], image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
            let g = cache.galley_paged(&ctx, 1, text, &p, geom.edit_w(), Some(&geom));
            let boxes = cache.page_boxes(1, text);
            out = nav::headings(text, &a)
                .iter()
                .filter(|h| h.level <= 3)
                .map(|h| (h.text.clone(), secpages::page_index(&boxes, g.pos_from_cursor(CCursor::new(text[..h.start].chars().count())).min.y) + 1))
                .collect();
        });
        std::mem::forget(r);
    }
    out
}

fn pdf_pages(text: &str) -> Vec<(String, usize)> {
    crate::pdf_write::toc_laid_pages(text, &|_| None).into_iter().filter(|(l, ..)| *l <= 3).map(|(_, t, p)| (t, p)).collect()
}

fn same(label: &str, text: &str) {
    let (v, p) = (view_pages(text), pdf_pages(text));
    let v: Vec<usize> = v.iter().map(|x| x.1).collect();
    let p: Vec<usize> = p.iter().map(|x| x.1).collect();
    assert_eq!(v, p, "{label}: dokumentläget (vänster) och PDF-utläggningen (höger) ger olika sidor");
}

#[test]
fn several_tocs_and_page_breaks_give_the_same_pages_in_view_and_pdf() {
    let pb = crate::word::sections::PAGE_BREAK_LINE;
    let mut md = String::from("# Årsrapport\n\n<!-- kladd: toc -->\n\n<!-- kladd: toc; levels: 2-2 -->\n\n<!-- kladd: toc; kind: figures -->\n\n");
    for i in 1..=4 {
        md.push_str(&format!("# Kapitel {i}\n\nBrödtext i kapitlet. Här står lite text som fyller sidan med innehåll.\n\n## Avsnitt {i}.1\n\nMer text.\n\n### Detalj {i}\n\nFigur {i}: En bildtext\n\n{pb}\n\n"));
    }
    same("md", &md);
}

#[test]
fn long_report_with_leading_toc_gives_the_same_pages() {
    let bytes = std::fs::read("tests/corpus/docx/gen-skolrapport.docx").unwrap();
    let md = crate::docx::read(&bytes).unwrap().markdown;
    same("skolrapport", &format!("<!-- kladd: toc -->\n\n{md}"));
}

#[test]
fn long_toc_first_and_plain_text_give_the_same_pages() {
    let para = "Brödtext som fyller raderna på sidan och fortsätter lite till så att stycket blir flera rader långt. ".repeat(3);
    // Förteckningen först, många rubriker (längre än en sida) och bara rubriker och brödtext.
    let mut md = String::from("<!-- kladd: toc -->\n\n");
    for i in 0..70 {
        md.push_str(&format!("## Avsnitt {i}\n\n{para}\n\n### Under {i}\n\nKort.\n\n"));
    }
    same("med förteckning", &md);
    // Ingen förteckning, och en annan teckenstorlek (typsnitt som saknas i programmet kan inte bli desamma i de två utläggningarna).
    same("utan förteckning", &md.replace("<!-- kladd: toc -->\n\n", ""));
}

#[test]
fn page_edges_agree_for_several_sizes_and_paragraph_lengths() {
    for size in [11, 12, 13] {
        for body in ["Kort.".to_owned(), "ord ".repeat(25), "ord ".repeat(60)] {
            let mut t = format!("<!-- kladd: font-family: \"Calibri\"; font-size: {size}pt -->\n\n");
            for i in 0..70 {
                t.push_str(&format!("## A{i}\n\n{body}\n\n"));
            }
            same(&format!("{size} pt, {} tecken", body.len()), &t);
        }
    }
}
