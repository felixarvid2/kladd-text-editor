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

// ---------------------------------------------------------------- talformat (grundformatsraden)

const FMT_DOC: &str = "<!-- kladd: endnote-format: decimal; footnote-format: lowerRoman -->\n\nA[^1] b[^ei] c[^eii].\n\n[^1]: Fot.\n[^ei]: Ett.\n[^eii]: Två.\n";

#[test]
fn visad_text_följer_formatet_men_etiketten_är_stabil() {
    let a = analyze(FMT_DOC);
    let n = a.defaults.notes;
    assert_eq!(n.display("1").as_deref(), Some("i"));
    assert_eq!(n.display("eii").as_deref(), Some("2"));
    // Utan inställning: som förut (fotnoter 1, slutnoter i).
    let a0 = analyze("A[^1] b[^ei].\n\n[^1]: Fot.\n[^ei]: Ett.\n");
    assert_eq!((a0.defaults.notes.display("1"), a0.defaults.notes.display("ei")), (None, None));
    // Bitarna bär fortfarande etiketterna.
    assert!(a.pieces.iter().any(|p| p.footnote_ref.as_deref() == Some("eii")));
}

#[test]
fn numrering_efter_insättning_och_borttagning_med_format() {
    // Infoga en slutnot före de andra: etiketterna numreras om, raden med formatet består.
    let e = insert_endnote(FMT_DOC, FMT_DOC.find("A[^1]").unwrap() + 1);
    assert!(e.text.starts_with("<!-- kladd: endnote-format: decimal; footnote-format: lowerRoman -->\n\n"));
    assert_eq!(endnotes(&e.text).refs.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["ei", "eii", "eiii"]);
    let n = analyze(&e.text).defaults.notes;
    assert_eq!(n.display("eiii").as_deref(), Some("3"));
    // Ta bort den första: de andra rycker upp.
    let d = delete_endnote(&e.text, "ei");
    assert_eq!(endnotes(&d).refs.len(), 2);
    assert!(d.contains("[^eii]") && !d.contains("[^eiii]"));
    // Fotnot: infoga och ta bort.
    let f = insert_footnote(FMT_DOC, FMT_DOC.find("A[^1]").unwrap() + 1);
    assert_eq!(footnotes(&f.text).refs.len(), 2);
    assert_eq!(analyze(&f.text).defaults.notes.display("2").as_deref(), Some("ii"));
    let g = delete_footnote(&f.text, "1");
    assert_eq!(footnotes(&g).refs.len(), 1);
    assert!(g.starts_with("<!-- kladd: endnote-format: decimal; footnote-format: lowerRoman -->"));
}

#[test]
fn startnummer_och_chicago() {
    let t = "<!-- kladd: footnote-format: chicago; footnote-start: 2 -->\n\nA[^1] b[^2].\n\n[^1]: x\n[^2]: y\n";
    let n = analyze(t).defaults.notes;
    assert_eq!(n.display("1").as_deref(), Some("†"));
    assert_eq!(n.display("2").as_deref(), Some("‡"));
}

#[test]
fn notnummer_har_minsta_storlek_och_etikettens_bredd_är_textens() {
    use super::layout::{Params, layout_job, note_label_size};
    // 11 pt-text i flödet: 0,65 ger under 7,5 pt, så minstanivån gäller; i notlistan 0,8.
    let px = |pt: f32| pt * 96.0 / 72.0;
    assert!((note_label_size(px(11.0), false, 1.0) - px(7.5)).abs() < 0.01);
    assert!((note_label_size(px(20.0), false, 1.0) - px(13.0)).abs() < 0.01);
    assert!((note_label_size(px(10.0), true, 1.0) - px(8.0)).abs() < 0.01);
    assert!((note_label_size(px(11.0), false, 2.0) - 2.0 * px(7.5)).abs() < 0.01);
    // Den reserverade bredden (`leading_space` på etikettens första tecken) är textens uppmätta bredd.
    let t = "<!-- kladd: footnote-format: upperRoman; endnote-format: chicago; footnote-start: 8 -->\n\nA[^1] B[^ei] C[^eii].\n\n[^1]: x\n[^ei]: y\n[^eii]: z\n";
    let a = analyze(t);
    let pal = crate::theme::palette(false);
    let measure = |s: &str, f: egui::FontId| s.chars().count() as f32 * f.size * 0.5;
    let fam = |_: Option<&str>, _: bool, _: bool| egui::FontFamily::Proportional;
    let p = Params {
        palette: &pal,
        zoom: 1.0,
        reveal: 0..0,
        matches: &[],
        image_size: &|_| None,
        measure: &measure,
        font_family: &fam,
        fonts_gen: 0,
    };
    let job = layout_job(t, &a, &p);
    let lead_at = |byte: usize| job.sections.iter().find(|s| s.byte_range.start.0 == byte).map(|s| s.leading_space).unwrap();
    let r1 = a.pieces.iter().find(|p| p.footnote_ref.as_deref() == Some("1")).unwrap();
    let r2 = a.pieces.iter().find(|p| p.footnote_ref.as_deref() == Some("ei")).unwrap();
    let size = note_label_size(px(11.0), false, 1.0);
    assert_eq!(a.defaults.notes.display("1").as_deref(), Some("VIII"));
    assert!((lead_at(r1.range.start) - 4.0 * size * 0.5).abs() < 0.01, "VIII");
    assert_eq!(a.defaults.notes.display("ei").as_deref(), Some("*"));
    assert!((lead_at(r2.range.start) - size * 0.5).abs() < 0.01, "stjärna");
    let r3 = a.pieces.iter().find(|p| p.footnote_ref.as_deref() == Some("eii")).unwrap();
    assert_eq!(a.defaults.notes.display("eii").as_deref(), Some("†"));
    assert!((lead_at(r3.range.start) - size * 0.5).abs() < 0.01, "kors");
}
