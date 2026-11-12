//! Tester för fotnoter i PDF-utskriften: noterna ritas längst ned på sidan där de refereras.
use super::*;

fn laid_out(md: &str) -> (Layout<'static>, DocDefaults) {
    let d = doc_defaults(md);
    let mut lay = Layout::new(&|_| None, &d);
    let (blocks, notes) = parse_blocks_notes(md);
    lay.notes = notes;
    lay.new_page();
    lay.run(&blocks);
    lay.flush_notes();
    (lay, d)
}

fn texts(lay: &Layout, page: usize) -> Vec<(f32, f32, String)> {
    lay.pages[page]
        .ops
        .iter()
        .filter_map(|o| match o {
            Op::Text { x, y, text, .. } => Some((*x, *y, text.clone())),
            _ => None,
        })
        .collect()
}

fn find(lay: &Layout, page: usize, needle: &str) -> Option<(f32, f32)> {
    texts(lay, page).into_iter().find(|t| t.2.contains(needle)).map(|t| (t.0, t.1))
}

#[test]
fn notes_are_parsed_in_reference_order() {
    let (blocks, notes) = parse_blocks_notes("A[^b] och B[^a].\n\n[^a]: Andra noten.\n[^b]: Första *noten*.\n");
    // Definitionerna blir inga stycken i brödtexten.
    assert_eq!(blocks.len(), 1);
    assert_eq!(notes.len(), 2);
    let flat = |n: &Vec<Span>| n.iter().map(|s| s.text.as_str()).collect::<String>();
    assert_eq!(flat(&notes[0]), "Första noten.");
    assert_eq!(flat(&notes[1]), "Andra noten.");
    let Some(Block::Para { spans, .. }) = blocks.first() else { panic!("inget stycke") };
    let refs: Vec<(usize, &str)> = spans.iter().filter_map(|s| s.style.fnref.map(|n| (n, s.text.as_str()))).collect();
    assert_eq!(refs, [(1, "1"), (2, "2")]);
}

#[test]
fn footnotes_are_drawn_at_the_bottom_of_their_page() {
    let (lay, d) = laid_out("Brödtext med en fotnot[^1] mitt i.\n\n[^1]: Fotnotens text står längst ned.\n");
    assert_eq!(lay.pages.len(), 1);
    let (_, ry) = find(&lay, 0, "Brödtext").unwrap();
    let (_, ny) = find(&lay, 0, "Fotnotens").unwrap();
    assert!(ny > ry + 300.0, "noten ska ligga långt under texten: {ry} {ny}");
    assert!(ny < d.page().1 - d.margins()[2] + 2.0 && ny > d.page().1 - d.margins()[2] - 40.0, "noten ska ligga strax ovanför nedre marginalen: {ny}");
    // Avskiljaren: en kort linje ovanför noten.
    let line = lay.pages[0].ops.iter().find_map(|o| match o {
        Op::Line { x1, y1, x2, .. } if *y1 < ny && *y1 > ny - 25.0 => Some(x2 - x1),
        _ => None,
    });
    assert!(line.is_some_and(|w| w > 20.0 && w < d.page().0 / 2.0), "{line:?}");
    // Referensen är ett litet upphöjt nummer.
    let big = lay.pages[0].ops.iter().find_map(|o| match o {
        Op::Text { size, text, y, .. } if text.contains("Brödtext") => Some((*size, *y)),
        _ => None,
    });
    let small = lay.pages[0].ops.iter().find_map(|o| match o {
        Op::Text { size, text, y, .. } if text == "1" && *y < ny - 100.0 => Some((*size, *y)),
        _ => None,
    });
    let (big, small) = (big.unwrap(), small.unwrap());
    assert!(small.0 < big.0 * 0.8 && small.1 < big.1, "{big:?} {small:?}");
}

#[test]
fn a_footnote_follows_its_reference_to_the_next_page() {
    let mut md: String = (0..45).map(|i| format!("Stycke {i} med lite text som fyller sidan.\n\n")).collect();
    md.push_str("Sista stycket med fotnot[^1].\n\nOch en sak till[^2].\n\n[^1]: Första noten.\n[^2]: Andra noten.\n");
    let (lay, _) = laid_out(&md);
    assert!(lay.pages.len() >= 2);
    let page_of = |needle: &str| (0..lay.pages.len()).find(|&p| find(&lay, p, needle).is_some());
    let p = page_of("Sista stycket").unwrap();
    assert_eq!(page_of("Första noten"), Some(p));
    assert_eq!(page_of("Andra noten"), page_of("Och en sak"));
    // Ingen sida har text under nedre marginalen (utom sidfoten).
    let (ph, mb) = (lay.ph, lay.mb);
    for pi in 0..lay.pages.len() {
        for (_, y, t) in texts(&lay, pi) {
            assert!(y <= ph - mb + 3.0, "{t:?} på y={y}");
        }
    }
}

#[test]
fn pdf_with_footnotes_is_written() {
    let w = write("Text[^1] och mer[^2].\n\n[^1]: Ett.\n[^2]: Två.\n", "T", &|_| None).unwrap();
    assert!(w.bytes.starts_with(b"%PDF-"));
    // Utan fotnoter ritas ingen avskiljare.
    let (lay, _) = laid_out("Bara text.\n");
    assert!(lay.pages[0].ops.iter().all(|o| !matches!(o, Op::Line { .. })));
}
