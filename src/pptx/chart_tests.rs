//! Prov på diagram och EMF/WMF i hela ritvägen: egna minimala .pptx (zip i minnet) med en
//! diagramdel eller en metafil på bilden.

use std::io::Write;

use super::chart_model::tests::BAR;
use super::color::Rgba;
use super::scene::{Item, Paint};
use super::{render, Deck};

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const RELS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Prov"><a:themeElements>
<a:clrScheme name="Prov"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4>
<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="Prov"><a:majorFont><a:latin typeface="Calibri Light"/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/></a:minorFont></a:fontScheme>
<a:fmtScheme name="Prov"><a:fillStyleLst/><a:lnStyleLst/><a:effectStyleLst/><a:bgFillStyleLst/></a:fmtScheme></a:themeElements></a:theme>"#;

/// En presentation med en bild (`inner` i formträdet) och extra delar med relationer från bilden.
fn deck(inner: &str, parts: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &[u8]| {
        z.start_file(name, opt).unwrap();
        z.write_all(data).unwrap();
    };
    put("[Content_Types].xml", br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/></Types>"#);
    put("_rels/.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#).as_bytes());
    put("ppt/_rels/presentation.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="{RELS}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rIdS1" Type="{RELS}/slide" Target="slides/slide1.xml"/></Relationships>"#).as_bytes());
    put("ppt/presentation.xml", format!(r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rIdS1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#).as_bytes());
    let tree = |s: &str| format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{s}</p:spTree></p:cSld>"#);
    put("ppt/slideMasters/slideMaster1.xml", format!(r#"<p:sldMaster {NS}>{}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:sldMaster>"#, tree("")).as_bytes());
    put("ppt/slideMasters/_rels/slideMaster1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{RELS}/theme" Target="../theme/theme1.xml"/></Relationships>"#).as_bytes());
    put("ppt/slideLayouts/slideLayout1.xml", format!(r#"<p:sldLayout {NS}>{}</p:sldLayout>"#, tree("")).as_bytes());
    put("ppt/slideLayouts/_rels/slideLayout1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#).as_bytes());
    put("ppt/theme/theme1.xml", THEME.as_bytes());
    put("ppt/slides/slide1.xml", format!(r#"<p:sld {NS}>{}</p:sld>"#, tree(inner)).as_bytes());
    let mut rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{RELS}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#);
    for (i, (name, kind, data)) in parts.iter().enumerate() {
        rels.push_str(&format!(r#"<Relationship Id="rIdX{}" Type="{RELS}/{kind}" Target="../{name}"/>"#, i + 1));
        put(&format!("ppt/{name}"), data);
    }
    rels.push_str("</Relationships>");
    put("ppt/slides/_rels/slide1.xml.rels", rels.as_bytes());
    z.finish().unwrap().into_inner()
}

/// En diagramram 400 × 300 pt på (72, 72).
fn chart_frame() -> String {
    r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="2" name="Diagram"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="914400"/><a:ext cx="5080000" cy="3810000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rIdX1"/></a:graphicData></a:graphic></p:graphicFrame>"#.to_owned()
}

fn fills(items: &[Item]) -> Vec<((f32, f32, f32, f32), Rgba)> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Fill { path, paint: Paint::Solid(c), .. } => Some((path.bounds()?, *c)),
            _ => None,
        })
        .collect()
}

fn texts(items: &[Item]) -> Vec<String> {
    items.iter().filter_map(|i| if let Item::Glyphs(g) = i { Some(g.text.trim().to_owned()) } else { None }).collect()
}

#[test]
fn stapeldiagram_ritas_ur_cachen() {
    let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", BAR.as_bytes())]);
    let d = Deck::open(&bytes).unwrap();
    let scene = render::slide_scene(&d, 0);
    let t = texts(&scene.items);
    for want in ["A", "B", "C", "Äpplen", "Päron", "0.0", "4.0"] {
        assert!(t.iter().any(|x| x == want), "saknar {want:?} i {t:?}");
    }
    assert!(!t.iter().any(|x| x == "Diagram"), "ingen platshållare");
    // Staplarna: serie 1 i accent1 (tre), serie 2 grön ur spPr (två; ett värde saknas).
    let f = fills(&scene.items);
    let blue: Vec<_> = f.iter().filter(|(_, c)| *c == Rgba::rgb8(0x44, 0x72, 0xC4)).collect();
    let green: Vec<_> = f.iter().filter(|(_, c)| *c == Rgba::rgb8(0, 255, 0)).collect();
    // Plus förklaringens nyckel för varje serie.
    assert_eq!(blue.len(), 4, "{f:?}");
    assert_eq!(green.len(), 3);
    // Staplarna står på samma baslinje och den högsta (4) är dubbelt så hög som den med 2.
    let mut bars: Vec<_> = blue.iter().map(|(b, _)| *b).filter(|b| b.3 - b.1 > 10.0).collect();
    bars.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(bars.len(), 3);
    assert!((bars[0].3 - bars[2].3).abs() < 0.01);
    let h = |b: &(f32, f32, f32, f32)| b.3 - b.1;
    assert!((h(&bars[2]) - 2.0 * h(&bars[1])).abs() < 0.1, "{bars:?}");
    // Allt inom ramen.
    for (b, _) in &f[1..] {
        assert!(b.0 >= 71.9 && b.2 <= 472.1 && b.1 >= 71.9 && b.3 <= 372.1, "{b:?}");
    }
}

#[test]
fn cirkeldiagram_med_procentetiketter() {
    let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><c:chart><c:title><c:overlay val="0"/></c:title><c:plotArea><c:layout/>
<c:pieChart><c:varyColors val="1"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>Försäljning</c:v></c:tx>
<c:dLbls><c:showLegendKey val="0"/><c:showVal val="0"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="1"/><c:showBubbleSize val="0"/></c:dLbls>
<c:cat><c:strLit><c:ptCount val="3"/><c:pt idx="0"><c:v>Ett</c:v></c:pt><c:pt idx="1"><c:v>Två</c:v></c:pt><c:pt idx="2"><c:v>Tre</c:v></c:pt></c:strLit></c:cat>
<c:val><c:numLit><c:ptCount val="3"/><c:pt idx="0"><c:v>2</c:v></c:pt><c:pt idx="1"><c:v>1</c:v></c:pt><c:pt idx="2"><c:v>1</c:v></c:pt></c:numLit></c:val></c:ser><c:firstSliceAng val="0"/></c:pieChart>
</c:plotArea><c:legend><c:legendPos val="r"/></c:legend></c:chart></c:chartSpace>"#;
    let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", xml.as_bytes())]);
    let d = Deck::open(&bytes).unwrap();
    let scene = render::slide_scene(&d, 0);
    let t = texts(&scene.items);
    // Automatisk rubrik (seriens namn), procent och förklaring per kategori.
    for want in ["Försäljning", "50%", "25%", "Ett", "Två", "Tre"] {
        assert!(t.iter().any(|x| x == want), "saknar {want:?} i {t:?}");
    }
    // Tre sektorer i accent1..3; den första (50 %) är högra halvan av cirkeln.
    let f = fills(&scene.items);
    let s1: Vec<_> = f.iter().filter(|(b, c)| *c == Rgba::rgb8(0x44, 0x72, 0xC4) && b.2 - b.0 > 20.0).collect();
    assert_eq!(s1.len(), 1, "{f:?}");
    let s2 = f.iter().find(|(b, c)| *c == Rgba::rgb8(0xED, 0x7D, 0x31) && b.2 - b.0 > 20.0).unwrap();
    assert!(s1[0].0 .0 > s2.0 .0, "första sektorn börjar klockan 12 och går medurs");
}

#[test]
fn trasigt_diagram_ger_platshallare() {
    let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", b"<c:chartSpace")]);
    let d = Deck::open(&bytes).unwrap();
    let t = texts(&render::slide_scene(&d, 0).items);
    assert!(t.iter().any(|x| x == crate::i18n::t("Diagram")), "{t:?}");
}

#[test]
fn emf_pa_bilden_med_fyllning_och_beskarning() {
    // EMF med en röd rektangel över hela ramen; bilden har ljus fyllning och `srcRect` med luft.
    let le = super::emf::tests_support::le;
    let emf = super::emf::tests_support::emf(&[(39, le(&[1, 0, 0x0000FF, 0])), (37, le(&[1])), (37, vec![8, 0, 0, 0x80]), (43, le(&[0, 0, 1000, 1000]))]);
    let pic = r#"<p:pic><p:nvPicPr><p:cNvPr id="3" name="Bild"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdX1"/><a:srcRect t="-25000" b="-25000"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="EEEEEE"/></a:solidFill></p:spPr></p:pic>"#;
    let bytes = deck(pic, &[("media/image1.emf", "image", &emf)]);
    let d = Deck::open(&bytes).unwrap();
    let f = fills(&render::slide_scene(&d, 0).items);
    // Bildens fyllning (0..100 pt) och sedan den röda rektangeln: bilden krymps till 2/3 av höjden
    // (synligt −25 %..125 %), alltså 16,7..83,3 pt.
    let gray = f.iter().position(|(_, c)| *c == Rgba::rgb8(0xEE, 0xEE, 0xEE)).expect("fyllning");
    let red = f.iter().position(|(_, c)| *c == Rgba::rgb8(255, 0, 0)).expect("emf");
    assert!(gray < red);
    let b = f[red].0;
    assert!((b.0 - 0.0).abs() < 0.1 && (b.2 - 100.0).abs() < 0.2, "{b:?}");
    assert!((b.1 - 16.67).abs() < 0.2 && (b.3 - 83.33).abs() < 0.2, "{b:?}");
}

/// Tid för en bild med diagram/EMF (visningslista, raster 1280 × 720, PDF):
/// `KLADD_PPTX_TIME=fil.pptx:83 cargo test chart_tests::tid -- --ignored --nocapture`.
#[test]
#[ignore]
fn tid() {
    let Ok(spec) = std::env::var("KLADD_PPTX_TIME") else { return };
    let (path, n) = spec.rsplit_once(':').unwrap();
    let d = Deck::open(&std::fs::read(path).unwrap()).unwrap();
    let i = n.parse::<usize>().unwrap() - 1;
    for _ in 0..3 {
        let t0 = std::time::Instant::now();
        let scene = render::slide_scene(&d, i);
        let t1 = t0.elapsed();
        let pm = super::raster::render(&scene, 1280, 720);
        // `KLADD_PPTX_PNG=bild.png` sparar bilden (för att titta på den).
        if let (Some(out), Some(pm)) = (std::env::var_os("KLADD_PPTX_PNG"), pm) {
            let (w, h, rgba) = super::raster::to_rgba(pm);
            image::RgbaImage::from_raw(w, h, rgba).unwrap().save(out).unwrap();
        }
        let t2 = t0.elapsed();
        let pdf = super::pdf::write(&d, &[i]).unwrap();
        let t3 = t0.elapsed();
        println!("{} element: scen {:?}, raster {:?}, pdf {:?} ({} kB)", scene.items.len(), t1, t2 - t1, t3 - t2, pdf.len() / 1024);
    }
}
