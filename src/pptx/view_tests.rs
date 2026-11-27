//! Prov för visningen: anteckningar, sök med träffrektanglar, *Redigera en kopia*, PDF-export utan
//! dolda bilder, bildspelet i fliken och `.ppsx`/`.potx`/`.pptm`/`.ppt`. Presentationen byggs i
//! provet (zip i minnet) med bilder, tabell, bild och anteckningar.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::find::{self, Place};
use super::view::PptxTab;
use super::{copy_md, notes, render, Deck};

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn tree(inner: &str) -> String {
    format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#)
}

fn sp(id: u32, ph: &str, xfrm: (i64, i64, i64, i64), paras: &str) -> String {
    let (x, y, w, h) = xfrm;
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="F{id}"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{paras}</p:txBody></p:sp>"#
    )
}

fn p(text: &str) -> String {
    format!(r#"<a:p><a:r><a:rPr lang="sv-SE" sz="2400"/><a:t>{text}</a:t></a:r></a:p>"#)
}

fn p_lvl(lvl: u32, text: &str) -> String {
    format!(r#"<a:p><a:pPr lvl="{lvl}"/><a:r><a:rPr lang="sv-SE" sz="2000"/><a:t>{text}</a:t></a:r></a:p>"#)
}

fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(4, 3, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// Tre bilder: (1) rubrik, punkter i två nivåer, en bild och anteckningar; (2) dold, rubrik och en
/// tabell; (3) bara en textruta. `main_ct` är huvuddelens innehållstyp (presentation, bildspel …).
pub fn fixture(main_ct: &str) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &[u8]| {
        z.start_file(name, opt).unwrap();
        z.write_all(data).unwrap();
    };
    put(
        "[Content_Types].xml",
        format!(r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="{main_ct}"/></Types>"#).as_bytes(),
    );
    put("_rels/.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#).as_bytes());
    let title = r#"<p:ph type="title"/>"#;
    let body = r#"<p:ph idx="1"/>"#;
    let s1 = sp(2, title, (457200, 274638, 8229600, 1143000), &p("Kvartalsrapport"))
        + &sp(3, body, (457200, 1600200, 5000000, 4525963), &(p_lvl(0, "Försäljningen ökade") + &p_lvl(1, "mest i norr") + &p_lvl(0, "Nya kunder")))
        + r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="Bild" descr="Stapel"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="6000000" y="2000000"/><a:ext cx="2000000" cy="1500000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#;
    let cell = |t: &str| format!(r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr/></a:tc>"#, p(t));
    let s2 = sp(2, title, (457200, 274638, 8229600, 1143000), &p("Siffror"))
        + &format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="Tabell"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="457200" y="1600200"/><a:ext cx="6000000" cy="800000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr/><a:tblGrid><a:gridCol w="3000000"/><a:gridCol w="3000000"/></a:tblGrid><a:tr h="400000">{}{}</a:tr><a:tr h="400000">{}{}</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
            cell("Region"),
            cell("Belopp"),
            cell("Norr"),
            cell("12|3")
        );
    let s3 = sp(2, "", (457200, 457200, 6000000, 1000000), &p("Tack för idag"));
    let slides = [(s1, ""), (s2, r#"show="0""#), (s3, "")];
    let mut ids = String::new();
    let mut rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#);
    for (i, (inner, attrs)) in slides.iter().enumerate() {
        let n = i + 1;
        ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rIdS{n}"/>"#, 255 + n));
        rels.push_str(&format!(r#"<Relationship Id="rIdS{n}" Type="{REL}/slide" Target="slides/slide{n}.xml"/>"#));
        put(&format!("ppt/slides/slide{n}.xml"), format!(r#"<p:sld {NS} {attrs}>{}</p:sld>"#, tree(inner)).as_bytes());
        let mut srels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#);
        if n == 1 {
            srels.push_str(&format!(r#"<Relationship Id="rIdImg" Type="{REL}/image" Target="../media/image1.png"/><Relationship Id="rIdN" Type="{REL}/notesSlide" Target="../notesSlides/notesSlide1.xml"/>"#));
        }
        srels.push_str("</Relationships>");
        put(&format!("ppt/slides/_rels/slide{n}.xml.rels"), srels.as_bytes());
    }
    rels.push_str("</Relationships>");
    put("ppt/_rels/presentation.xml.rels", rels.as_bytes());
    put(
        "ppt/presentation.xml",
        format!(r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#).as_bytes(),
    );
    put(
        "ppt/slideMasters/slideMaster1.xml",
        format!(r#"<p:sldMaster {NS}>{}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:sldMaster>"#, tree("")).as_bytes(),
    );
    put("ppt/slideMasters/_rels/slideMaster1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#).as_bytes());
    put("ppt/slideLayouts/slideLayout1.xml", format!(r#"<p:sldLayout {NS}>{}</p:sldLayout>"#, tree("")).as_bytes());
    put("ppt/slideLayouts/_rels/slideLayout1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#).as_bytes());
    put("ppt/media/image1.png", &png());
    // Anteckningssidan: bildplatshållaren (ingen text) och brödtexten med två stycken.
    put(
        "ppt/notesSlides/notesSlide1.xml",
        format!(
            r#"<p:notes {NS}>{}</p:notes>"#,
            tree(&(sp(2, r#"<p:ph type="sldImg"/>"#, (0, 0, 100, 100), "<a:p/>") + &sp(3, r#"<p:ph type="body" idx="1"/>"#, (0, 0, 100, 100), &(p("Nämn siffrorna för norr.") + &p("Fråga efter frågor.") + "<a:p/>"))))
        )
        .as_bytes(),
    );
    z.finish().unwrap().into_inner()
}

const PPTX_CT: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";

fn deck() -> Deck {
    Deck::open(&fixture(PPTX_CT)).expect("presentationen öppnas")
}

#[test]
fn anteckningar_ur_anteckningssidan() {
    let d = deck();
    assert_eq!(notes::notes_text(&d, 0), "Nämn siffrorna för norr.\nFråga efter frågor.");
    assert_eq!(notes::notes_text(&d, 1), "", "bild utan anteckningar");
    assert_eq!(notes::notes_text(&d, 99), "");
    assert!(PptxTab::new(d).has_notes());
}

#[test]
fn sök_i_bildtext_och_anteckningar_med_rektanglar() {
    let d = deck();
    let (sw, sh) = d.size_pt();
    let st = find::slide_text(&render::slide_scene(&d, 0));
    assert!(st.text.contains("Kvartalsrapport") && st.text.contains("mest i norr"), "{}", st.text);
    let hits = find::hits_for(0, &st, &notes::notes_text(&d, 0), "NORR", false);
    let on_slide: Vec<_> = hits.iter().filter(|h| h.place == Place::Slide).collect();
    let in_notes: Vec<_> = hits.iter().filter(|h| h.place == Place::Notes).collect();
    assert_eq!(on_slide.len(), 1, "{hits:?}");
    assert_eq!(in_notes.len(), 1);
    // Rektangeln ligger på bilden, inom brödtextens ruta och under rubrikens.
    let rects = find::hit_rects(&st, on_slide[0].start, on_slide[0].end);
    assert_eq!(rects.len(), 1);
    let r = rects[0];
    assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= sw && r[3] <= sh, "{r:?}");
    let body = (457200.0 / 12700.0, 1600200.0 / 12700.0, (457200.0 + 5000000.0) / 12700.0);
    assert!(r[0] >= body.0 && r[2] <= body.2 && r[1] >= body.1 - 5.0, "{r:?}");
    // Ordet "norr" är smalare än hela raden "mest i norr".
    let line = find::find_all(&st.text, "mest i norr", false)[0];
    let full = find::hit_rects(&st, line.0, line.1)[0];
    assert!(r[2] - r[0] < full[2] - full[0] - 5.0 && (r[2] - full[2]).abs() < 2.0, "{r:?} {full:?}");
    let title = find::find_all(&st.text, "Kvartalsrapport", false)[0];
    assert!(find::hit_rects(&st, title.0, title.1)[0][3] <= r[1], "rubriken står ovanför");
}

#[test]
fn redigera_en_kopia_ger_markdown() {
    let d = deck();
    let (md, images) = copy_md::deck_to_markdown(&d);
    assert!(md.starts_with("# Kvartalsrapport\n"), "{md}");
    assert!(md.contains("- Försäljningen ökade\n    - mest i norr\n- Nya kunder"), "{md}");
    assert!(md.contains("![Stapel](bild:image1.png)"), "{md}");
    assert!(md.contains("> **Anteckningar**\n>\n> Nämn siffrorna för norr.\n>\n> Fråga efter frågor."), "{md}");
    assert!(md.contains("# Siffror\n\n| Region | Belopp |\n| --- | --- |\n| Norr | 12\\|3 |"), "{md}");
    assert!(md.contains("# Bild 3\n\nTack för idag"), "bild utan rubrik får sitt nummer: {md}");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].0, "image1.png");
    // Markdown-tolken ser rubriker, nästlad lista, tabell, bild och citat.
    use pulldown_cmark::{Event, Options, Parser, Tag};
    let mut heads = 0;
    let (mut lists, mut table, mut img, mut quote) = (0, false, false, false);
    for ev in Parser::new_ext(&md, Options::ENABLE_TABLES) {
        if let Event::Start(tag) = ev {
            match tag {
                Tag::Heading { .. } => heads += 1,
                Tag::List(_) => lists += 1,
                Tag::Table(_) => table = true,
                Tag::Image { .. } => img = true,
                Tag::BlockQuote(_) => quote = true,
                _ => {}
            }
        }
    }
    assert_eq!(heads, 3);
    assert!(lists >= 2 && table && img && quote);
}

#[test]
fn kopian_kan_sparas_som_docx_och_läsas_igen() {
    let (md, images) = copy_md::deck_to_markdown(&deck());
    let dir = tmp_dir("docx");
    let path = dir.join("kopia.docx");
    let mut doc = crate::doc::Document::new_untitled(1, 1);
    doc.text = md;
    doc.docx = true;
    doc.images = images;
    doc.save_to(&path).expect("sparas");
    let back = crate::doc::Document::load(2, &path).expect("läses");
    assert!(back.text.contains("Kvartalsrapport") && back.text.contains("mest i norr") && back.text.contains("Region"), "{}", back.text);
    assert!(!back.images.is_empty(), "bilden följer med");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pdf_utan_dolda_bilder() {
    let tab = PptxTab::new(deck());
    assert_eq!(tab.visible_slides(), vec![0, 2]);
    let bytes = tab.pdf_bytes().expect("pdf");
    let pdf = lopdf::Document::load_mem(&bytes).expect("giltig PDF");
    assert_eq!(pdf.get_pages().len(), 2, "den dolda bilden är inte med");
}

#[test]
fn pdf_exporten_skriver_filen_i_bakgrunden() {
    let mut tab = PptxTab::new(deck());
    let dir = tmp_dir("export");
    let path = dir.join("ut.pdf");
    let ctx = egui::Context::default();
    tab.write_pdf(&ctx, path.clone());
    assert!(tab.exporting());
    let t0 = std::time::Instant::now();
    let res = loop {
        if let Some(r) = tab.poll_export(&ctx) {
            break r;
        }
        assert!(t0.elapsed() < std::time::Duration::from_secs(60), "exporten blev aldrig klar");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(res.expect("exporten lyckas"), path);
    assert!(!tab.exporting());
    let pdf = lopdf::Document::load(&path).expect("giltig PDF");
    assert_eq!(pdf.get_pages().len(), 2);
    // Fel blir ett meddelande, inte panik.
    tab.write_pdf(&ctx, dir.join("saknas").join("ut.pdf"));
    let t0 = std::time::Instant::now();
    let res = loop {
        if let Some(r) = tab.poll_export(&ctx) {
            break r;
        }
        assert!(t0.elapsed() < std::time::Duration::from_secs(60));
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(res.is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bildspel_i_fliken_hoppar_över_dolda_och_står_kvar() {
    let mut tab = PptxTab::new(deck());
    tab.go_to(2);
    assert!(tab.start_show(false, true) && tab.presenting());
    tab.stop_show();
    assert_eq!(tab.current, 0, "slutar där bildspelet stod");
    tab.go_to(1);
    assert!(tab.start_show(true, true));
    tab.stop_show();
    assert_eq!(tab.current, 1, "Skift+F5 på en dold bild visar den");
    tab.go_to(50);
    assert_eq!(tab.current, 2, "utanför presentationen: sista bilden");
}

fn tmp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("kladd-pptx-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn bildspel_mallar_och_makrofiler_öppnas_i_vyn() {
    let dir = tmp_dir("ext");
    for (ext, ct) in [
        ("ppsx", "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml"),
        ("potx", "application/vnd.openxmlformats-officedocument.presentationml.template.main+xml"),
        ("pptm", "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml"),
        ("PPTX", PPTX_CT),
    ] {
        let path = dir.join(format!("prov.{ext}"));
        std::fs::write(&path, fixture(ct)).unwrap();
        let doc = crate::doc::Document::load(1, &path).unwrap_or_else(|e| panic!("{ext}: {e}"));
        let tab = doc.pptx.as_deref().unwrap_or_else(|| panic!("{ext} öppnas i presentationsvyn"));
        assert_eq!(tab.count(), 3);
        assert!(doc.text.is_empty() && !doc.dirty);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn gammal_ppt_utan_libreoffice_ger_tydligt_fel() {
    let dir = tmp_dir("ppt");
    let path = dir.join("gammal.ppt");
    std::fs::write(&path, [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]).unwrap();
    assert!(crate::legacy::needs_conversion(&path));
    assert_eq!(crate::legacy::target_ext(&path), "pptx");
    let e = crate::doc::Document::load(1, &path).err().unwrap();
    assert!(e.contains("LibreOffice") && e.contains(".pptx") && e.contains("gammal.ppt"), "{e}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn konverterad_ppt_öppnas_i_presentationsvyn() {
    let path = Path::new("/h/gammal.ppt");
    let doc = crate::doc::Document::from_converted(1, path, fixture(PPTX_CT)).expect("öppnas");
    assert!(doc.pptx.is_some());
    assert_eq!(doc.path.as_deref(), Some(path));
    let note = doc.open_note.as_deref().unwrap_or_default();
    assert!(note.contains("LibreOffice") && !note.contains(".docx"), "{note}");
}

#[cfg(unix)]
#[test]
fn ppt_konverteras_till_pptx_med_soffice() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp_dir("soffice");
    let input = dir.join("x.ppt");
    std::fs::write(&input, [0xD0, 0xCF, 0x11, 0xE0]).unwrap();
    // Ett låtsasprogram som skriver resultatet med den begärda filändelsen.
    let fake = dir.join("fake.sh");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprev=\"\"\nfor a; do [ \"$prev\" = \"--outdir\" ] && out=\"$a\"; [ \"$prev\" = \"--convert-to\" ] && fmt=\"$a\"; prev=\"$a\"; done\nprintf 'PPTX' > \"$out/x.$fmt\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(crate::legacy::convert(&fake, &input, std::time::Duration::from_secs(5)).unwrap(), b"PPTX");
    let _ = std::fs::remove_dir_all(dir);
}

/// Gör en .ppt av korpusfilen med LibreOffice och öppnar den (kör med `cargo test -- --ignored`).
#[test]
#[ignore = "kräver LibreOffice"]
fn soffice_konverterar_ppt() {
    let Some(soffice) = crate::legacy::find_soffice() else {
        eprintln!("hoppar över: LibreOffice saknas");
        return;
    };
    let dir = tmp_dir("lo");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/pptx/pythonpptx-txt-text-frame.pptx");
    let pptx = dir.join("prov.pptx");
    std::fs::copy(&src, &pptx).unwrap();
    let st = std::process::Command::new(&soffice)
        .args(["--headless", "--convert-to", "ppt", "--outdir"])
        .arg(&dir)
        .arg(&pptx)
        .env("HOME", &dir)
        .status()
        .unwrap();
    assert!(st.success());
    let ppt = dir.join("prov.ppt");
    assert!(crate::legacy::needs_conversion(&ppt));
    let bytes = crate::legacy::convert(&soffice, &ppt, crate::legacy::TIMEOUT).expect("konverteras");
    let doc = crate::doc::Document::from_converted(1, &ppt, bytes).expect("öppnas");
    assert!(doc.pptx.as_deref().is_some_and(|t| t.count() > 0));
    let _ = std::fs::remove_dir_all(dir);
}
