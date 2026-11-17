//! Tester för sidhuvud och sidfot i PDF-utskriften: sidnummer per avsnitt, varianter och bilder.
use super::*;

const PNG: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0xd, 0xa, 0x1a, 0xa, 0x0, 0x0, 0x0, 0xd, 0x49, 0x48, 0x44, 0x52, 0x0, 0x0, 0x0, 0x1, 0x0, 0x0, 0x0, 0x1, 0x8, 0x2, 0x0, 0x0, 0x0, 0x90, 0x77, 0x53, 0xde, 0x0,
    0x0, 0x0, 0xc, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x0, 0x0, 0x3, 0x1, 0x1, 0x0, 0xc9, 0xfe, 0x92, 0xef, 0x0, 0x0, 0x0, 0x0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
    0x60, 0x82,
];

fn png(src: &str) -> Option<DocImage> {
    (src == "bild:logo.png").then(|| DocImage { bytes: PNG.to_vec(), extent: None })
}

/// Sidhuvud/sidfot per sida som `write` räknar dem: (huvudets text, sidfotens text, antal bilder).
fn per_page(md: &str) -> Vec<(String, String, usize)> {
    let resolve: &dyn Fn(&str) -> Option<DocImage> = &png;
    let (blocks, notes) = parse_blocks_notes(md);
    let defaults = doc_defaults(md);
    let mut lay = lay_out(&blocks, &notes, &defaults, resolve);
    let total = lay.pages.len();
    let secs = sections::scan(md, &defaults);
    let plan = crate::hf::Plan::new(&defaults, &secs);
    let pp = plan.pages(&lay.pages.iter().map(|p| p.sec).collect::<Vec<_>>());
    let base = style::Fields::for_export("rapport", "pdf").at(1, total);
    (0..total)
        .map(|k| {
            let ops = lay.header_footer_page(&defaults, &plan, &pp, &base, k);
            let mid = lay.pages[k].geom.ph / 2.0;
            let join = |top: bool| ops.iter().filter_map(|o| match o {
                Op::Text { y, text, .. } if (*y < mid) == top => Some(text.clone()),
                _ => None,
            }).collect::<Vec<_>>().join(" ");
            let imgs = ops.iter().filter(|o| matches!(o, Op::Image { .. })).count();
            (join(true), join(false), imgs)
        })
        .collect()
}

fn filler(n: usize) -> String {
    (0..n).map(|i| format!("Stycke {i} med så pass mycket text att det blir flera rader och sidan fylls långsamt men säkert.\n\n")).collect()
}

#[test]
fn page_x_of_y_restarts_per_section_with_roman_numbers() {
    let md = format!(
        "<!-- kladd: header: \"Rapport\"; footer: \"Sida {{sida}} av {{sidor}}\"; page-format: roman -->\n\n{}<!-- kladd: avsnitt; header: \"Bilaga\"; page-start: 1; page-format: 1 -->\n\n{}",
        filler(70),
        filler(70)
    );
    let pages = per_page(&md);
    let n = pages.len();
    assert!(n >= 4, "{n} sidor");
    assert_eq!(pages[0].1, format!("Sida i av {n}"));
    assert_eq!(pages[1].1, format!("Sida ii av {n}"));
    assert_eq!(pages[0].0, "Rapport");
    // Andra avsnittet börjar om på 1, med eget sidhuvud.
    let first2 = pages.iter().position(|p| p.0 == "Bilaga").expect("avsnitt 2");
    assert_eq!(pages[first2].1, format!("Sida 1 av {n}"));
    assert_eq!(pages[first2 + 1].1, format!("Sida 2 av {n}"));
}

#[test]
fn first_page_and_even_odd_variants() {
    let md = format!(
        "<!-- kladd: header: \"Udda\"; header-first: \"Omslag\"; header-even: \"Jämn\"; footer: \"{{sida}}\" -->\n\n{}",
        filler(100)
    );
    let pages = per_page(&md);
    assert!(pages.len() >= 4);
    // Första sidan: titlePg; sedan jämna (2, 4) och udda (3) sidor.
    assert_eq!(pages[0].0, "Omslag");
    assert_eq!(pages[1].0, "Jämn");
    assert_eq!(pages[2].0, "Udda");
    assert_eq!(pages[3].0, "Jämn");
    // Utan egen sidfot på jämna/första sidor (Some saknas = tom) syns bara udda sidors nummer.
    assert_eq!(pages[2].1, "3");
}

#[test]
fn logo_is_drawn_and_pushes_the_body_down() {
    let md = "<!-- kladd: page-margin: 20mm; header: \"{bild:logo.png 40x60}|Titel|\"; header-dist: 10mm -->\n\nText\n";
    let pages = per_page(md);
    assert_eq!(pages[0].2, 1, "en bild i sidhuvudet");
    // 10 mm + 60 pt ligger under 20 mm marginal: texten börjar under loggan.
    let blocks = parse_blocks(md);
    let defaults = doc_defaults(md);
    let resolve: &dyn Fn(&str) -> Option<DocImage> = &png;
    let mut lay = Layout::new(resolve, &defaults);
    lay.hf_reserve = hf_reserve(&blocks, &defaults);
    assert!(lay.hf_reserve[0][0] >= 28.35 + 60.0 - 0.1, "{:?}", lay.hf_reserve);
}

#[test]
fn full_pdf_with_header_image_is_written() {
    let md = "<!-- kladd: header: \"{bild:logo.png 40x20}|Titel|{datum}\"; footer: \"Sida {sida} av {sidor}\"; footer-align: center; header-style: \"9pt #666666 line=#999999/0.5pt\" -->\n\nText\n";
    let out = write(md, "rapport", &png).unwrap();
    assert!(out.bytes.starts_with(b"%PDF"));
}
