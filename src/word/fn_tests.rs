//! Tester för fotnoter i Markdown-modellen (`markup::insert_footnote` m.fl.).
use super::markup::*;

const DOC: &str = "Första[^1] och andra[^2].\n\n[^1]: Ett.\n[^2]: Två *kursiv*.\n";

fn covers(d: &str) {
    let a = analyze(d);
    let mut at = 0;
    for p in &a.pieces {
        assert_eq!(p.range.start, at);
        at = p.range.end;
    }
    assert_eq!(at, d.len());
}

fn piece_of<'a>(a: &'a Analysis, text: &str, needle: &str) -> &'a Piece {
    let at = text.find(needle).unwrap();
    a.pieces.iter().find(|p| p.range.start <= at && at < p.range.end).unwrap()
}

#[test]
fn analys_av_referenser_och_definitioner() {
    let a = analyze(DOC);
    let r = a.pieces.iter().find(|p| p.footnote_ref.as_deref() == Some("1")).unwrap();
    assert_eq!(&DOC[r.range.clone()], "1");
    assert!(!r.markup);
    let open = a.pieces.iter().find(|p| p.range.end == r.range.start).unwrap();
    assert!(open.markup && &DOC[open.range.clone()] == "[^");
    let close = a.pieces.iter().find(|p| p.range.start == r.range.end).unwrap();
    assert!(close.markup && DOC[close.range.clone()].starts_with(']'));
    let d = a.pieces.iter().find(|p| p.footnote_def.as_deref() == Some("2")).unwrap();
    assert_eq!(d.block, Block::Footnote);
    let at = DOC.find("[^2]:").unwrap();
    assert!(a.pieces.iter().any(|p| p.markup && p.range.start == at && p.block == Block::Footnote));
    let t = piece_of(&a, DOC, "kursiv");
    assert!(t.italic && t.block == Block::Footnote);
    assert_eq!(piece_of(&a, DOC, "Första").block, Block::Normal);
    covers(DOC);
    assert!(footnotes_in_order(&a));
    let at = DOC.find("[^1]").unwrap() + 2;
    assert_eq!(footnote_label_at(&a, at), Some(("1".into(), false, at..at + 1)));
    let at = DOC.find("[^2]:").unwrap() + 2;
    assert_eq!(footnote_label_at(&a, at).map(|x| (x.0, x.1)), Some(("2".into(), true)));
    // Hopp mellan referens och definition, och var definitionerna börjar.
    assert_eq!(footnote_target(DOC, &a, "2", false), Some(DOC.find("Två").unwrap()));
    assert_eq!(footnote_target(DOC, &a, "2", true), Some(DOC.find("[^2]").unwrap() + 4));
    assert_eq!(footnote_area_start(DOC, &a), Some(DOC.find("[^1]:").unwrap()));
    assert_eq!(footnote_area_start("Bara text\n", &analyze("Bara text\n")), None);
}

#[test]
fn räkning_ignorerar_etiketter() {
    let a = analyze(DOC);
    let (words, _) = count(DOC, &a);
    // "Första och andra." + "Ett." + "Två kursiv."
    assert_eq!(words, 3 + 1 + 2);
}

#[test]
fn infoga_fotnot_i_text_utan_fotnoter() {
    let e = insert_footnote("Hej", 3);
    assert_eq!(e.text, "Hej[^1]\n\n[^1]: \n");
    let at = e.text.find("[^1]: ").unwrap() + 6;
    assert_eq!(e.sel, at..at);
    let a = analyze(&e.text);
    assert!(a.pieces.iter().any(|p| p.footnote_def.as_deref() == Some("1")));
    assert!(a.pieces.iter().any(|p| p.footnote_ref.as_deref() == Some("1")));
}

#[test]
fn infoga_fotnot_numrerar_senare() {
    let pos = DOC.find(" och").unwrap();
    let e = insert_footnote(DOC, pos);
    assert_eq!(e.text, "Första[^1][^2] och andra[^3].\n\n[^1]: Ett.\n[^2]: \n[^3]: Två *kursiv*.\n");
    assert_eq!(&e.text[..e.sel.start], "Första[^1][^2] och andra[^3].\n\n[^1]: Ett.\n[^2]: ");
    let e = insert_footnote(DOC, 0);
    assert!(e.text.starts_with("[^1]Första[^2] och andra[^3]."));
    assert!(e.text.contains("[^1]: \n[^2]: Ett.\n[^3]: Två *kursiv*.\n"));
    let e = insert_footnote(DOC, DOC.find('\n').unwrap());
    assert!(e.text.ends_with("[^2]: Två *kursiv*.\n[^3]: \n"));
    assert!(e.text.starts_with("Första[^1] och andra[^2].[^3]\n"));
    // Resultatet är i ordning.
    assert!(footnotes_in_order(&analyze(&e.text)));
}

#[test]
fn infoga_fotnot_i_definition_gör_inget() {
    let pos = DOC.find("Ett").unwrap();
    assert_eq!(insert_footnote(DOC, pos).text, DOC);
}

#[test]
fn numrera_om_ordnar_etiketter_och_definitioner() {
    let t = "A[^7] B[^3] C[^7].\n\n[^3]: tre\n[^7]: sju\n";
    let r = renumber_footnotes(t);
    assert_eq!(r, "A[^1] B[^2] C[^1].\n\n[^1]: sju\n[^2]: tre\n");
    assert_eq!(renumber_footnotes(DOC), DOC);
    assert_eq!(renumber_footnotes(&r), r);
    assert!(footnotes_in_order(&analyze(&r)));
    assert!(!footnotes_in_order(&analyze(t)));
}

#[test]
fn numrera_om_behåller_föräldralösa_sist() {
    let t = "A[^2].\n\n[^9]: övrig\n[^2]: med\n";
    assert_eq!(renumber_footnotes(t), "A[^1].\n\n[^1]: med\n[^9]: övrig\n");
    let t = "A[^2].\n\n[^1]: övrig\n[^2]: med\n";
    assert_eq!(renumber_footnotes(t), "A[^1].\n\n[^1]: med\n[^2]: övrig\n");
}

#[test]
fn numrera_om_flyttar_definitioner_sist_och_markören() {
    let t = "A[^2] B[^1].\n\n[^2]: b\n[^1]: a\n\nSlut.\n";
    let (r, c) = renumber_footnotes_at(t, t.find("a\n").unwrap());
    assert_eq!(r, "A[^1] B[^2].\n\nSlut.\n\n[^1]: b\n[^2]: a\n");
    assert_eq!(&r[c..c + 1], "a");
}

#[test]
fn ta_bort_fotnot() {
    assert_eq!(delete_footnote(DOC, "1"), "Första och andra[^1].\n\n[^1]: Två *kursiv*.\n");
    assert_eq!(delete_footnote("Hej[^1].\n\n[^1]: x\n", "1"), "Hej.\n");
    assert_eq!(delete_footnote(DOC, "9"), DOC);
}

#[test]
fn referenser_i_kod_räknas_inte() {
    let t = "```\n[^1]\n```\n\nText[^1].\n\n[^1]: x\n";
    let n = footnotes(t);
    assert_eq!(n.refs.len(), 1);
    assert_eq!(n.defs.len(), 1);
}
