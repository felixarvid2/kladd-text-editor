//! Tester för slutnoter i Markdown-modellen (`[^ei]`, `[^eii]` …).
use super::markup::*;

const DOC: &str = "Fot[^1] slut[^ei] mer[^eii].\n\n[^1]: Ett.\n[^ei]: Slut ett.\n[^eii]: Slut två.\n";

#[test]
fn etiketter_och_romerska_tal() {
    assert!(is_endnote_label("ei") && is_endnote_label("eiv") && !is_endnote_label("e") && !is_endnote_label("1") && !is_endnote_label("ex1"));
    assert_eq!(endnote_label(4), "eiv");
    assert_eq!(roman(1994), "mcmxciv");
    assert_eq!(roman(0), "0");
}

#[test]
fn fotnoter_och_slutnoter_skiljs_åt() {
    assert_eq!(footnotes(DOC).refs.len(), 1);
    assert_eq!(footnotes(DOC).defs.len(), 1);
    assert_eq!(endnotes(DOC).refs.len(), 2);
    assert_eq!(endnotes(DOC).defs.len(), 2);
    assert!(footnotes_in_order(&analyze(DOC)));
}

#[test]
fn analys_döljer_e_och_visar_romersk_siffra() {
    let a = analyze(DOC);
    let r = a.pieces.iter().find(|p| p.footnote_ref.as_deref() == Some("eii")).unwrap();
    assert_eq!(&DOC[r.range.clone()], "ii");
    let d = a.pieces.iter().find(|p| p.footnote_def.as_deref() == Some("ei")).unwrap();
    assert_eq!(&DOC[d.range.clone()], "i");
}

#[test]
fn omnumrering_av_slutnoter() {
    let t = "A[^eiii] b[^ei] c[^1].\n\n[^1]: Fot.\n[^ei]: B.\n[^eiii]: A.\n";
    let r = renumber_footnotes(t);
    assert_eq!(r, "A[^ei] b[^eii] c[^1].\n\n[^1]: Fot.\n[^ei]: A.\n[^eii]: B.\n");
    assert!(footnotes_in_order(&analyze(&r)));
    assert!(!footnotes_in_order(&analyze(t)));
    assert_eq!(renumber_footnotes(&r), r);
    // Fotnotsdefinitionerna ligger före slutnotsdefinitionerna även om de blandats.
    let t = "A[^ei] c[^1].\n\n[^ei]: A.\n[^1]: Fot.\n";
    assert_eq!(renumber_footnotes(t), "A[^ei] c[^1].\n\n[^1]: Fot.\n[^ei]: A.\n");
}

#[test]
fn infoga_slutnot() {
    let e = insert_endnote("Hej och hå", 3);
    assert_eq!(e.text, "Hej[^ei] och hå\n\n[^ei]: \n");
    assert_eq!(e.sel.start, e.text.len() - 1);
    // En andra, före den första i texten: numreras om.
    let t = "Först[^ei] sist.\n\n[^ei]: Ettan.\n";
    let e = insert_endnote(t, 0);
    assert_eq!(e.text, "[^ei]Först[^eii] sist.\n\n[^ei]: \n[^eii]: Ettan.\n");
    assert!(footnotes_in_order(&analyze(&e.text)));
    // Fotnoterna rörs inte.
    let t = "A[^1] b.\n\n[^1]: Fot.\n";
    let e = insert_endnote(t, 8);
    assert_eq!(e.text, "A[^1] b.[^ei]\n\n[^1]: Fot.\n[^ei]: \n");
}

#[test]
fn radera_slutnot() {
    assert_eq!(delete_endnote(DOC, "ei"), "Fot[^1] slut mer[^ei].\n\n[^1]: Ett.\n[^ei]: Slut två.\n");
    assert_eq!(delete_footnote(DOC, "1"), "Fot slut[^ei] mer[^eii].\n\n[^ei]: Slut ett.\n[^eii]: Slut två.\n");
}
