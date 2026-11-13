//! Tester för avsnitt i PDF-utskriften: sidstorlek per sida, spalter och löpande avsnitt.
use super::*;

fn lay(md: &str) -> Layout<'static> {
    let blocks = parse_blocks(md);
    let defaults = doc_defaults(md);
    let resolve: &'static dyn Fn(&str) -> Option<DocImage> = &|_| None;
    let mut lay = Layout::new(resolve, &defaults);
    lay.new_page();
    lay.run(&blocks);
    lay.flush_notes();
    lay
}

fn texts(ops: &[Op]) -> Vec<(f32, f32, String)> {
    ops.iter()
        .filter_map(|o| match o {
            Op::Text { x, y, text, .. } => Some((*x, *y, text.clone())),
            _ => None,
        })
        .collect()
}

fn filler(n: usize) -> String {
    (0..n).map(|i| format!("Stycke {i} med så pass mycket text att det blir flera rader i en smal spalt och inte bara en. Mer text för att fylla ut.\n\n")).collect()
}

#[test]
fn each_section_has_its_own_page_size() {
    let l = lay("Före\n\n<!-- kladd: avsnitt; page: A4 landscape -->\n\nLiggande\n\n<!-- kladd: avsnitt -->\n\nTillbaka\n");
    assert_eq!(l.pages.len(), 3);
    let sizes: Vec<(f32, f32)> = l.pages.iter().map(|p| (p.geom.pw.round(), p.geom.ph.round())).collect();
    assert_eq!(sizes, vec![(595.0, 842.0), (842.0, 595.0), (595.0, 842.0)]);
    // Texten på den liggande sidan ligger vid vänster marginal men textytan är bredare.
    assert!(l.pages[1].geom.cw > l.pages[0].geom.cw + 200.0);
    assert!(texts(&l.pages[1].ops).iter().any(|t| t.2.contains("Liggande")));
    assert!(texts(&l.pages[2].ops).iter().any(|t| t.2.contains("Tillbaka")));
}

#[test]
fn leading_marker_applies_to_the_first_page() {
    let l = lay("<!-- kladd: avsnitt; page: A5 -->\n\nEtt\n");
    assert_eq!(l.pages.len(), 1);
    assert_eq!(l.pages[0].geom.pw.round(), 420.0);
}

#[test]
fn page_numbers_in_footer_continue_across_sections() {
    let l_md = "<!-- kladd: footer: \"{sida} av {sidor}\" -->\n\nEtt\n\n<!-- kladd: avsnitt; page: A5 -->\n\nTvå\n";
    let mut l = lay(l_md);
    let d = doc_defaults(l_md);
    let total = l.pages.len();
    assert_eq!(total, 2);
    let f2 = texts(&l.header_footer(&d, &crate::style::Fields::new(2, total, "x")));
    assert_eq!(f2[0].2, "2 av 2");
    // Sidfoten centreras på den egna (mindre) sidan.
    let g = l.pages[1].geom;
    assert!(f2[0].0 >= g.ml && f2[0].0 < g.ml + g.cw);
    assert!(f2[0].1 > g.ph - g.mb);
}

#[test]
fn columns_fill_down_then_across_then_next_page() {
    let l = lay(&format!("<!-- kladd: avsnitt; columns: 2; column-gap: 10mm -->\n\n{}", filler(40)));
    assert!(l.pages.len() >= 2, "{}", l.pages.len());
    let g = l.pages[0].geom;
    let col_w = (g.cw - 10.0 * 72.0 / 25.4) / 2.0;
    let right_x = g.ml + col_w + 10.0 * 72.0 / 25.4;
    let t = texts(&l.pages[0].ops);
    let left: Vec<_> = t.iter().filter(|t| (t.0 - g.ml).abs() < 1.0).collect();
    let right: Vec<_> = t.iter().filter(|t| (t.0 - right_x).abs() < 1.0).collect();
    assert!(!left.is_empty() && !right.is_empty(), "text i båda spalterna");
    // Högra spalten börjar överst, och vänstra fylls nästan till nederkanten innan den tar vid.
    let top = left.iter().map(|t| t.1).fold(f32::INFINITY, f32::min);
    let top_r = right.iter().map(|t| t.1).fold(f32::INFINITY, f32::min);
    assert!((top - top_r).abs() < 1.0, "{top} {top_r}");
    let bottom_l = left.iter().map(|t| t.1).fold(0.0, f32::max);
    assert!(bottom_l > g.ph - g.mb - 40.0, "{bottom_l}");
    // Ingen text bredare än sin spalt (radbrytningen sker i spaltbredden).
    for (x, _, _) in &t {
        assert!(*x >= g.ml - 0.1 && *x <= right_x + col_w, "{x}");
    }
}

#[test]
fn column_break_and_continuous_sections() {
    let l = lay("<!-- kladd: avsnitt; columns: 2 -->\n\nEtt\n\n<!-- kladd: spaltbrytning -->\n\nTvå\n");
    assert_eq!(l.pages.len(), 1);
    let t = texts(&l.pages[0].ops);
    let (a, b) = (t.iter().find(|t| t.2 == "Ett").unwrap(), t.iter().find(|t| t.2 == "Två").unwrap());
    assert!(b.0 > a.0 + 100.0 && (a.1 - b.1).abs() < 1.0, "{a:?} {b:?}");
    // Löpande avsnitt: samma sida, ny spaltindelning under den förra.
    let l = lay("Rubrik\n\n<!-- kladd: avsnitt; columns: 2; type: continuous -->\n\nVänster\n\n<!-- kladd: spaltbrytning -->\n\nHöger\n\n<!-- kladd: avsnitt; type: continuous -->\n\nSlut\n");
    assert_eq!(l.pages.len(), 1);
    let t = texts(&l.pages[0].ops);
    let at = |s: &str| t.iter().find(|t| t.2 == s).unwrap().clone();
    let (rub, v, h, slut) = (at("Rubrik"), at("Vänster"), at("Höger"), at("Slut"));
    assert!(v.1 > rub.1 && (v.1 - h.1).abs() < 1.0 && h.0 > v.0 + 100.0);
    assert!(slut.1 > v.1 && (slut.0 - rub.0).abs() < 0.1, "sista avsnittet över hela bredden under spalterna");
}

#[test]
fn write_makes_a_pdf_with_sections() {
    let md = format!("Före\n\n<!-- kladd: avsnitt; page: A4 landscape; columns: 2 -->\n\n{}<!-- kladd: avsnitt -->\n\nSlut\n", filler(30));
    let w = write(&md, "x", &|_| None).unwrap();
    assert!(w.bytes.starts_with(b"%PDF"));
    let back = crate::pdf_read::read(&w.bytes, None).ok().expect("pdf kan läsas");
    assert!(back.page_count >= 3);
    // Avsnittsraderna skrivs inte ut som text.
    assert!(!back.markdown.contains("avsnitt"), "{}", back.markdown);
}

#[test]
fn three_part_header_and_first_page_variant_fit_each_sections_page() {
    let md = "<!-- kladd: header: \"V|M|{sida}\"; header-first: \"Förstasida\"; footer: \"{sida}\" -->\n\nEtt\n\n<!-- kladd: avsnitt; page: A4 landscape -->\n\nTvå\n";
    let mut l = lay(md);
    let d = doc_defaults(md);
    assert_eq!(l.pages.len(), 2);
    let f1 = texts(&l.header_footer(&d, &crate::style::Fields::new(1, 2, "x")));
    assert!(f1.iter().any(|t| t.2 == "Förstasida"), "{f1:?}");
    let f2 = texts(&l.header_footer(&d, &crate::style::Fields::new(2, 2, "x")));
    let g = l.pages[1].geom;
    let right = f2.iter().find(|t| t.2 == "2" && t.1 < g.mt).expect("högerdelen i sidhuvudet");
    // Delen ligger vid den liggande sidans högra marginal, inte vid den stående sidans.
    assert!(right.0 > 595.0 && right.0 < g.ml + g.cw, "{right:?}");
    assert!(f2.iter().any(|t| t.2 == "M" && t.0 > g.ml + g.cw / 3.0 && t.0 < g.ml + 2.0 * g.cw / 3.0));
}

#[test]
fn style_overrides_apply_inside_columns() {
    let l = lay("<!-- kladd: h1: 30pt color #FF0000 -->\n\n<!-- kladd: avsnitt; columns: 2 -->\n\n# Rubrik\n\nText\n");
    let size = l.pages[0].ops.iter().find_map(|o| match o {
        Op::Text { size, text, .. } if text.contains("Rubrik") => Some(*size),
        _ => None,
    });
    assert_eq!(size, Some(30.0));
}
