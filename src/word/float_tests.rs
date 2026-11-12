//! Tester för flytande bilder: `<img align>`, `set_image_float`, flytt av bilder och (längre ned)
//! textomflöde i layouten.

use std::ops::Range;

use super::markup::*;

fn img_range(t: &str) -> Range<usize> {
    analyze(t).pieces.iter().find(|p| p.image.is_some()).unwrap().range.clone()
}

#[test]
fn align_lases() {
    let t = "<img src=\"a.png\" alt=\"x\" width=\"200\" align=\"left\">";
    let im = analyze(t).pieces.iter().find_map(|p| p.image.clone()).unwrap();
    assert_eq!((im.width, im.align), (Some(200.0), Some(Float::Left)));
    let t = "<img align=RIGHT src=a.png>";
    assert_eq!(analyze(t).pieces.iter().find_map(|p| p.image.clone()).unwrap().align, Some(Float::Right));
    // Okänt värde eller center: ingen flytande bild.
    for v in ["center", "top", ""] {
        let t = format!("<img src=\"a.png\" align=\"{v}\">");
        assert_eq!(analyze(&t).pieces.iter().find_map(|p| p.image.clone()).unwrap().align, None, "{v}");
    }
}

#[test]
fn set_image_float_skriver_align() {
    let t = "före\n\n![Sol](bild:a.png)\n\nefter";
    let e = set_image_float(t, img_range(t), Some(Float::Left));
    assert!(e.text.contains("<img src=\"bild:a.png\" alt=\"Sol\" align=\"left\">"), "{}", e.text);
    assert_eq!(analyze(&e.text).pieces.iter().find_map(|p| p.image.clone()).unwrap().align, Some(Float::Left));
    let e2 = set_image_float(&e.text, e.sel.clone(), Some(Float::Right));
    assert!(e2.text.contains("align=\"right\"") && !e2.text.contains("left"));
    // Samma värde: orört. Tillbaka till "i texten": Markdown igen när inget annat finns.
    assert_eq!(set_image_float(&e2.text, e2.sel.clone(), Some(Float::Right)).text, e2.text);
    let e3 = set_image_float(&e2.text, e2.sel.clone(), None);
    assert_eq!(e3.text, t);
    // Med bredd behålls <img>.
    let w = set_image_size(t, img_range(t), Some(200.0));
    let f = set_image_float(&w.text, w.sel.clone(), Some(Float::Left));
    assert!(f.text.contains("width=\"200\" align=\"left\""));
    let back = set_image_float(&f.text, f.sel.clone(), None);
    assert_eq!(back.text, w.text);
    // Ingen bild på platsen.
    assert_eq!(set_image_float(t, 0..3, Some(Float::Left)).text, t);
}

#[test]
fn storlek_behaller_align() {
    let t = "<img src=\"a.png\" alt=\"\" width=\"200\" align=\"right\">";
    let e = set_image_size(t, 0..t.len(), Some(100.0));
    assert_eq!(e.text, "<img src=\"a.png\" alt=\"\" width=\"100\" align=\"right\">");
    let e = set_image_size(&e.text, e.sel, None);
    assert_eq!(e.text, "<img src=\"a.png\" alt=\"\" align=\"right\">");
}

#[test]
fn flytta_fore_och_efter() {
    let t = "A\n\n![x](i.png)\n\nB\n\nC";
    let r = img_range(t);
    let c = t.find('C').unwrap();
    let e = move_image(t, r.clone(), c, true).unwrap();
    assert_eq!(e.text, "A\n\nB\n\nC\n\n![x](i.png)");
    assert_eq!(&e.text[e.sel.clone()], "![x](i.png)");
    let e = move_image(t, r.clone(), c, false).unwrap();
    assert_eq!(e.text, "A\n\nB\n\n![x](i.png)\n\nC");
    let e = move_image(t, r.clone(), 0, false).unwrap();
    assert_eq!(e.text, "![x](i.png)\n\nA\n\nB\n\nC");
    assert_eq!(&e.text[e.sel.clone()], "![x](i.png)");
    // På sig själv ändrar inget.
    assert!(move_image(t, r.clone(), r.start, true).is_none());
    assert!(move_image(t, r.clone(), r.start, false).is_none());
    // En tom rad går till närmaste stycke.
    let blank = t.find("\n\nC").unwrap() + 1;
    let e = move_image(t, r, blank, true).unwrap();
    assert_eq!(e.text, "A\n\nB\n\n![x](i.png)\n\nC");
}

#[test]
fn flytta_tar_med_div() {
    let t = "A\n\n<div align=\"center\">\n\n<img src=\"i.png\" width=\"20\">\n\n</div>\n\nB\n\nC\n";
    let r = img_range(t);
    let blk = image_block(t, r.clone());
    assert_eq!(&t[blk], "<div align=\"center\">\n\n<img src=\"i.png\" width=\"20\">\n\n</div>");
    let e = move_image(t, r.clone(), t.find('C').unwrap(), true).unwrap();
    assert_eq!(e.text, "A\n\nB\n\nC\n\n<div align=\"center\">\n\n<img src=\"i.png\" width=\"20\">\n\n</div>\n");
    assert_eq!(&e.text[e.sel.clone()], "<img src=\"i.png\" width=\"20\">");
    let e = move_image(t, r, 0, false).unwrap();
    assert!(e.text.starts_with("<div align=\"center\">\n\n<img") && e.text.ends_with("B\n\nC\n"), "{}", e.text);
    assert_eq!(analyze(&e.text).pieces.iter().find(|p| p.image.is_some()).unwrap().align, Align::Center);
}

#[test]
fn flytta_inline_bild_tar_bara_taggen() {
    let t = "ett <img src=\"a.png\" width=\"4\"> två\n\nslut";
    let r = img_range(t);
    assert_eq!(image_block(t, r.clone()), r);
    let e = move_image(t, r, t.find("slut").unwrap(), true).unwrap();
    assert!(e.text.ends_with("slut\n\n<img src=\"a.png\" width=\"4\">"), "{}", e.text);
    assert!(e.text.starts_with("ett  två"));
}

#[test]
fn flytta_hoppar_over_tabeller() {
    let t = "<img src=\"a.png\">\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nslut";
    let r = img_range(t);
    let mid = t.find("| 1").unwrap();
    let e = move_image(t, r, mid, true).unwrap();
    assert_eq!(e.text, "| a | b |\n| - | - |\n| 1 | 2 |\n\n<img src=\"a.png\">\n\nslut");
}
