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

#[test]
fn unequal_columns_get_own_x_and_width_and_a_rule() {
    let md = "<!-- kladd: avsnitt; columns: 2; column-gap: 10mm; column-widths: 40mm 120mm; column-rule: yes -->\n\nVänster\n\n<!-- kladd: spaltbrytning -->\n\nHöger\n";
    let mut l = lay(md);
    let (w0, w1) = (l.col_width(0), l.col_width(1));
    assert!((w1 / w0 - 3.0).abs() < 0.01, "{w0} {w1}");
    assert!((l.col_left(1) - (w0 + l.gap)).abs() < 0.01);
    // Efter spaltbrytningen står texten i den breda spalten.
    assert_eq!(l.col, 1);
    assert!((l.cw - w1).abs() < 0.01);
    l.flush_rules();
    let x = l.text_l + w0 + l.gap / 2.0;
    let rules = l.pages[0].ops.iter().filter(|o| matches!(o, Op::Line { x1, x2, .. } if (x1 - x2).abs() < 0.01 && (x1 - x).abs() < 0.01)).count();
    assert_eq!(rules, 1);
    let right = texts(&l.pages[0].ops).into_iter().find(|t| t.2.contains("Höger")).map(|t| t.0);
    assert!((right.expect("text") - (l.text_l + w0 + l.gap)).abs() < 0.5);
}

#[test]
fn paragraph_flowing_into_a_narrower_column_wraps_at_that_columns_width() {
    let long = "Ord som fyller raderna med flera ord på varje rad och fortsätter. ".repeat(160);
    let md = format!("<!-- kladd: avsnitt; columns: 2; column-gap: 10mm; column-widths: 120mm 40mm -->\n\n{long}\n");
    let l = lay(&md);
    let (w0, w1) = (l.col_width(0), l.col_width(1));
    let right_x = l.text_l + w0 + l.gap - 1.0;
    let mut chars = (0usize, 0usize);
    for t in texts(&l.pages[0].ops) {
        let c = t.2.chars().count();
        if t.0 < right_x {
            chars.0 = chars.0.max(c);
        } else {
            chars.1 = chars.1.max(c);
        }
    }
    // Raderna i den smala spalten är kortare i förhållande till spaltens bredd: ingen rad går utanför sidan.
    assert!(chars.0 > 0 && chars.1 > 0, "{chars:?}");
    assert!((chars.1 as f32) < chars.0 as f32 * (w1 / w0) * 1.4, "{chars:?} {w0} {w1}");
    assert!(chars.0 as f32 > (w1 / 0.5) * 0.0 + 60.0, "{chars:?}");
}

fn lay_bal(md: &str) -> Layout<'static> {
    let (blocks, notes) = parse_blocks_notes(md);
    let defaults = doc_defaults(md);
    let resolve: &'static dyn Fn(&str) -> Option<DocImage> = &|_| None;
    lay_out(&blocks, &notes, &defaults, resolve)
}

/// Understa textraden (y) per spalt på sidan, för text som börjar efter `after_y`, med spaltens x som gräns.
fn col_bottoms(l: &Layout, page: usize, xs: &[f32], after: &str) -> Vec<f32> {
    let t = texts(&l.pages[page].ops);
    let top = t.iter().find(|t| t.2.contains(after)).map_or(0.0, |t| t.1);
    let mut out = vec![0.0f32; xs.len()];
    for (x, y, s) in &t {
        if *y <= top || s.contains("Slut") {
            continue;
        }
        let c = xs.iter().rposition(|cx| *x >= *cx - 1.0).unwrap_or(0);
        out[c] = out[c].max(*y);
    }
    out
}

fn col_xs(l: &Layout) -> Vec<f32> {
    (0..l.cols).map(|c| l.text_l + l.col_left(c)).collect()
}

fn balanced_doc(cols: &str, n: usize) -> String {
    format!("Rubrik\n\n<!-- kladd: avsnitt; columns: {cols}; type: continuous -->\n\n{}<!-- kladd: avsnitt; type: continuous -->\n\nSlut\n", filler(n))
}

#[test]
fn two_equal_columns_are_balanced_before_a_continuous_section() {
    let l = lay_bal(&balanced_doc("2", 6));
    assert_eq!(l.pages.len(), 1);
    let mut probe = lay_bal("<!-- kladd: avsnitt; columns: 2 -->\n\nx\n");
    probe.cols = 2;
    let xs = col_xs(&probe);
    let b = col_bottoms(&l, 0, &xs, "Rubrik");
    let line = l.body * LINE;
    assert!(b[0] > 0.0 && b[1] > 0.0, "{b:?}");
    assert!((b[0] - b[1]).abs() <= line * 1.01, "{b:?}");
    // Slutavsnittet börjar under den högsta spalten.
    let t = texts(&l.pages[0].ops);
    let slut = t.iter().find(|t| t.2 == "Slut").unwrap();
    assert!(slut.1 > b[0].max(b[1]), "{slut:?} {b:?}");
    // Utan balansering fylls bara första spalten: spalt 2 är tom.
    let raw = lay(&balanced_doc("2", 6));
    let br = col_bottoms(&raw, 0, &xs, "Rubrik");
    assert!(br[1] == 0.0 || (br[0] - br[1]).abs() > line * 3.0, "{br:?}");
}

#[test]
fn three_equal_columns_are_balanced() {
    let l = lay_bal(&balanced_doc("3", 9));
    assert_eq!(l.pages.len(), 1);
    let mut probe = lay_bal("<!-- kladd: avsnitt; columns: 3 -->\n\nx\n");
    probe.cols = 3;
    let xs = col_xs(&probe);
    let b = col_bottoms(&l, 0, &xs, "Rubrik");
    let line = l.body * LINE;
    let (lo, hi) = (b.iter().cloned().fold(f32::MAX, f32::min), b.iter().cloned().fold(0.0, f32::max));
    assert!(lo > 0.0 && hi - lo <= line * 1.01, "{b:?}");
}

#[test]
fn unequal_columns_are_balanced_in_their_own_widths() {
    let md = format!(
        "Rubrik\n\n<!-- kladd: avsnitt; columns: 2; column-gap: 10mm; column-widths: 50mm 110mm; column-rule: yes; type: continuous -->\n\n{}<!-- kladd: avsnitt; type: continuous -->\n\nSlut\n",
        filler(6)
    );
    let l = lay_bal(&md);
    assert_eq!(l.pages.len(), 1);
    let gap = 10.0 * 72.0 / 25.4;
    let w0 = (l.text_w - gap) * 50.0 / 160.0;
    let xs = vec![l.text_l, l.text_l + w0 + gap];
    let b = col_bottoms(&l, 0, &xs, "Rubrik");
    assert!(b[0] > 0.0 && b[1] > 0.0, "{b:?}");
    assert!((b[0] - b[1]).abs() <= l.body * LINE * 1.01 * 2.0, "{b:?}");
    // Skiljelinjen går lika långt ned som den högsta spalten.
    let rule_end = l.pages[0].ops.iter().filter_map(|o| match o { Op::Line { x1, x2, y2, .. } if (x1 - x2).abs() < 0.01 => Some(*y2), _ => None }).fold(0.0, f32::max);
    assert!(rule_end >= b[0].max(b[1]) && rule_end < b[0].max(b[1]) + l.body * LINE * 1.5, "{rule_end} {b:?}");
    let slut = texts(&l.pages[0].ops).into_iter().find(|t| t.2 == "Slut").unwrap();
    assert!(slut.1 > rule_end - 1.0, "{slut:?} {rule_end}");
}

#[test]
fn a_manual_column_break_stops_the_balancing() {
    let md = "Rubrik\n\n<!-- kladd: avsnitt; columns: 2; type: continuous -->\n\nEtt\n\nTvå\n\nTre\n\n<!-- kladd: spaltbrytning -->\n\nFyra\n\n<!-- kladd: avsnitt; type: continuous -->\n\nSlut\n";
    let l = lay_bal(md);
    let t = texts(&l.pages[0].ops);
    let at = |s: &str| t.iter().find(|t| t.2 == s).unwrap().clone();
    let (ett, tre, fyra) = (at("Ett"), at("Tre"), at("Fyra"));
    // Spalt 1 har tre stycken, spalt 2 ett: oförändrat.
    assert!(tre.0 < fyra.0 && (ett.1 - fyra.1).abs() < 1.0 && tre.1 > ett.1 + 20.0, "{ett:?} {tre:?} {fyra:?}");
}

#[test]
fn only_the_last_page_of_a_multi_page_section_is_balanced() {
    let l = lay_bal(&balanced_doc("2", 60));
    assert!(l.pages.len() >= 2);
    let last = l.pages.len() - 1;
    // Första sidan är full i båda spalterna.
    let g = l.pages[0].geom;
    let t = texts(&l.pages[0].ops);
    let max_y = t.iter().map(|t| t.1).fold(0.0, f32::max);
    assert!(max_y > g.ph - g.mb - 40.0, "{max_y}");
    let mut probe = lay_bal("<!-- kladd: avsnitt; columns: 2 -->\n\nx\n");
    probe.cols = 2;
    let xs = col_xs(&probe);
    let first = col_bottoms(&l, 0, &xs, "Rubrik");
    assert!(first[0] > g.ph - g.mb - 40.0 && first[1] > g.ph - g.mb - 40.0, "{first:?}");
    // Sista sidan: jämn och med slutavsnittet under.
    let b = col_bottoms(&l, last, &xs, "zzz");
    assert!(b[0] > 0.0 && b[1] > 0.0 && (b[0] - b[1]).abs() <= l.body * LINE * 1.01, "{b:?}");
    assert!(texts(&l.pages[last].ops).iter().any(|t| t.2 == "Slut"));
}

#[test]
fn no_balancing_at_the_end_of_the_document() {
    let md = format!("<!-- kladd: avsnitt; columns: 2 -->\n\n{}", filler(3));
    let l = lay_bal(&md);
    let t = texts(&l.pages[0].ops);
    let right = l.text_l + l.col_left(1) - 1.0;
    assert!(t.iter().all(|t| t.0 < right), "allt står i första spalten");
}

#[test]
fn balanced_pdf_with_footnotes_and_rules_is_written() {
    let md = format!(
        "Rubrik\n\n<!-- kladd: avsnitt; columns: 2; column-rule: yes; type: continuous -->\n\n{}Text med not[^1].\n\n<!-- kladd: avsnitt; type: continuous -->\n\nSlut\n\n[^1]: Noten.\n",
        filler(5)
    );
    let l = lay_bal(&md);
    assert_eq!(l.pages.len(), 1);
    let t = texts(&l.pages[0].ops);
    assert!(t.iter().any(|t| t.2.contains("Noten")), "fotnoten ritas");
    let w = write(&md, "x", &|_| None).unwrap();
    assert!(w.bytes.starts_with(b"%PDF"));
}

#[test]
fn trailing_continuous_row_balances_but_the_last_section_without_one_does_not() {
    // `.odt` från LibreOffice: ett `text:section` med spalter ger en löpande rad efter sig, också sist.
    let sect = format!("Rubrik\n\n<!-- kladd: avsnitt; columns: 2; type: continuous -->\n\n{}", filler(5));
    let with_row = lay_bal(&format!("{sect}<!-- kladd: avsnitt; type: continuous -->\n"));
    let right = with_row.text_l + with_row.text_w / 2.0;
    assert!(texts(&with_row.pages[0].ops).iter().any(|t| t.0 > right), "spalt 2 används");
    let without = lay_bal(&sect);
    assert!(texts(&without.pages[0].ops).iter().all(|t| t.0 < right), "utan rad fylls bara spalt 1");
}

#[test]
fn column_balance_no_keeps_the_columns_unbalanced_in_the_pdf() {
    let md = format!("Rubrik\n\n<!-- kladd: avsnitt; columns: 2; type: continuous; column-balance: no -->\n\n{}<!-- kladd: avsnitt; type: continuous -->\n\nSlut\n", filler(5));
    let l = lay_bal(&md);
    let right = l.text_l + l.text_w / 2.0;
    assert!(texts(&l.pages[0].ops).iter().filter(|t| t.2 != "Slut").all(|t| t.0 < right), "bara spalt 1 fylls");
}
