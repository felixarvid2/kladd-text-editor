//! Prov för nya presentationer: filen går genom `validate`, öppnas, ritas, kan redigeras och sparas,
//! och (om python-pptx respektive LibreOffice finns) öppnas av dem.

use super::edit::Session;
use super::edit_ops;
use super::edit_tests::validate;
use super::new_deck::{build, Variant};
use super::Deck;

#[test]
fn ny_presentation_ar_giltig_och_kan_oppnas_i_alla_varianter() {
    for v in Variant::ALL {
        let bytes = build(v);
        let errs = validate(&bytes);
        assert!(errs.is_empty(), "{v:?}: {errs:#?}");
        let deck = Deck::open(&bytes).expect("öppnas");
        assert_eq!(deck.slides.len(), 1);
        assert_eq!(deck.layouts.len(), 7, "{v:?}");
        // Ritas utan panik (bildens text är tom, men layouten och temat läses).
        let img = super::render_slide(&deck, 0, 320, 180);
        assert!(img.is_some_and(|(w, h, px)| w == 320 && h == 180 && px.len() == 320 * 180 * 4));
        assert_eq!(deck.size_emu, (12_192_000, 6_858_000));
    }
}

#[test]
fn ny_presentation_redigeras_och_sparas_med_alla_layouter() {
    let bytes = build(Variant::Kladd);
    let mut deck = Deck::open(&bytes).unwrap();
    let mut s = Session::new(bytes.clone(), &deck);
    // Oförändrad sparning ger samma delar.
    let (same, _) = s.save_bytes(&deck, "pptx", "").unwrap();
    assert!(validate(&same).is_empty());
    // En ny bild per layout, och text i första bildens rubrik.
    for l in 0..deck.layouts.len() {
        let a = s.apply(&deck, None, |t| edit_ops::new_slide(t, Some(0), l).map(|_| ())).unwrap().unwrap();
        deck = a.deck;
    }
    assert_eq!(deck.slides.len(), 8);
    let sh = edit_ops::shapes(&deck, 0);
    let title = sh.iter().find(|x| x.ph.as_deref() == Some("ctrTitle")).expect("rubrikplatshållare");
    let part0 = deck.slides[0].part.clone();
    let doc = super::edit_xml::parse(deck.pkg.read(&part0).unwrap()).unwrap();
    let el = edit_ops::raw_shape(&doc, title.index).unwrap();
    let mut m = super::edit_text::TextModel::from_body(el.child("p:txBody"), &doc.root_scope(), "a");
    m.insert(super::edit_text::Pos::default(), "Min presentation");
    let a = s.apply(&deck, None, |t| edit_ops::set_text(t, 0, title.index, title.id, &m)).unwrap().unwrap();
    deck = a.deck;
    let (out, _) = s.save_bytes(&deck, "pptx", "").unwrap();
    let errs = validate(&out);
    assert!(errs.is_empty(), "{errs:#?}");
    let d2 = Deck::open(&out).unwrap();
    assert_eq!(d2.slides.len(), 8);
    assert!(super::render::slide_scene(&d2, 0).text().contains("Min presentation"));
    for i in 0..d2.slides.len() {
        let _ = super::render_slide(&d2, i, 160, 90);
    }
}

#[test]
fn ny_presentation_egen_inte_microsofts() {
    // Temat och mallen är skrivna av oss: namnet är Kladds, inte Office.
    let bytes = build(Variant::Skog);
    let deck = Deck::open(&bytes).unwrap();
    let theme = String::from_utf8_lossy(deck.pkg.read("ppt/theme/theme1.xml").unwrap()).into_owned();
    assert!(theme.contains(r#"name="Kladd""#) && !theme.contains("Office Theme"));
}

/// Öppnas av python-pptx (om det finns) och konverteras av LibreOffice (om det finns).
#[test]
fn ny_presentation_oppnas_av_andra_verktyg() {
    let dir = std::env::temp_dir().join(format!("kladd-newdeck-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let f = dir.join("ny.pptx");
    std::fs::write(&f, build(Variant::Skymning)).unwrap();
    if let Ok(o) = std::process::Command::new("python3").args(["-c", "import sys; from pptx import Presentation; p = Presentation(sys.argv[1]); assert len(p.slides) == 1 and len(p.slide_layouts) == 7, (len(p.slides), len(p.slide_layouts))", f.to_str().unwrap()]).output() {
        let err = String::from_utf8_lossy(&o.stderr);
        if !err.contains("No module named") {
            assert!(o.status.success(), "python-pptx: {err}");
        }
    }
    if let Ok(o) = std::process::Command::new("soffice").args(["--headless", "--convert-to", "pdf", "--outdir", dir.to_str().unwrap(), f.to_str().unwrap()]).output() {
        assert!(o.status.success() && dir.join("ny.pdf").exists(), "soffice: {}", String::from_utf8_lossy(&o.stderr));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn former_infogas_med_temats_stil_och_ritas() {
    use super::edit_new::{insert_shape, ShapeKind};
    use super::edit_ops::Xf;
    let bytes = build(Variant::Kladd);
    let mut deck = Deck::open(&bytes).unwrap();
    let mut s = Session::new(bytes, &deck);
    let blank = render_px(&deck);
    for (i, kind) in ShapeKind::ALL.into_iter().enumerate() {
        let xf = Xf { x: 1_000_000 + 900_000 * i as i64, y: 3_000_000, cx: 800_000, cy: 600_000, rot: 0, flip_h: kind == ShapeKind::Line, flip_v: kind == ShapeKind::Line };
        let a = s.apply(&deck, None, |t| insert_shape(t, 0, kind, xf).map(|_| ())).unwrap().unwrap();
        deck = a.deck;
    }
    let before = 2; // rubrik och underrubrik
    assert_eq!(edit_ops::shapes(&deck, 0).len(), before + ShapeKind::ALL.len());
    let (out, _) = s.save_bytes(&deck, "pptx", "").unwrap();
    let errs = validate(&out);
    assert!(errs.is_empty(), "{errs:#?}");
    let x = String::from_utf8_lossy(Deck::open(&out).unwrap().pkg.read("ppt/slides/slide1.xml").unwrap()).into_owned();
    // Temats stil på formerna (inte på textrutan): fyllning, linje, effekt och typsnitt.
    assert_eq!(x.matches("<p:style>").count(), 6, "{x}");
    assert!(x.contains(r#"<a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>"#) && x.contains(r#"prst="roundRect""#) && x.contains(r#"prst="rightArrow""#) && x.contains("<p:cxnSp>") && x.contains(r#"flipH="1""#));
    // Schemaordning i p:sp: nvSpPr, spPr, style, txBody.
    let sp = x.find("<p:style>").unwrap();
    assert!(x[..sp].rfind("</p:spPr>").unwrap() + "</p:spPr>".len() == sp);
    let d2 = Deck::open(&out).unwrap();
    assert_ne!(render_px(&d2), blank, "formerna syns");
    // Unika id.
    let ids: Vec<u32> = edit_ops::shapes(&d2, 0).iter().map(|s| s.id).collect();
    assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), ids.len());
}

fn render_px(deck: &Deck) -> Vec<u8> {
    super::render_slide(deck, 0, 480, 270).map(|(_, _, p)| p).unwrap_or_default()
}
