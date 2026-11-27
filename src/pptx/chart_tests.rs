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

// ---- 3D-diagram (w-p3d) ----

/// Stapeldiagrammet `BAR` som 3D-typ (`c:bar3DChart` m.fl.) med en vy och golv/väggar.
fn bar_3d(kind: &str, extra: &str, view: &str) -> String {
    BAR.replace("<c:barChart>", &format!("<c:{kind}>{extra}"))
        .replace("</c:barChart>", &format!("<c:shape val=\"box\"/></c:{kind}>"))
        .replace("<c:autoTitleDeleted val=\"1\"/>", &format!("<c:autoTitleDeleted val=\"1\"/>{view}<c:backWall><c:spPr><a:solidFill><a:srgbClr val=\"F0F0F0\"/></a:solidFill></c:spPr></c:backWall>"))
}

fn render_png(scene: &super::scene::Scene) -> Vec<u8> {
    let pm = super::raster::render(scene, 400, 300).unwrap();
    super::raster::to_rgba(pm).2
}

#[test]
fn stapeldiagram_i_3d_har_skuggade_sidor() {
    let accent1 = Rgba::rgb8(0x44, 0x72, 0xC4);
    let view = r#"<c:view3D><c:rotX val="15"/><c:rotY val="20"/><c:depthPercent val="100"/><c:rAngAx val="1"/></c:view3D>"#;
    let xml = bar_3d("bar3DChart", "", view);
    let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", xml.as_bytes())]);
    let d = Deck::open(&bytes).unwrap();
    let scene = render::slide_scene(&d, 0);
    let t = texts(&scene.items);
    assert!(!t.iter().any(|x| x == "Diagram"), "ingen platshållare: {t:?}");
    for want in ["A", "B", "C", "4.0"] {
        assert!(t.iter().any(|x| x == want), "saknar {want:?} i {t:?}");
    }
    let f = fills(&scene.items);
    // Framsidorna i seriens färg, toppen ljusare och sidan mörkare (samma kulör).
    let fronts = f.iter().filter(|(_, c)| *c == accent1).count();
    assert_eq!(fronts, 4, "tre framsidor och förklaringens nyckel i accent1");
    let lighter = f.iter().any(|(_, c)| c.b > accent1.b + 0.01 && c.r > accent1.r && (c.r - c.b).abs() < (accent1.b - accent1.r) + 0.01);
    let darker = f.iter().any(|(_, c)| c.b < accent1.b - 0.05 && c.r < accent1.r && c.b > c.r);
    assert!(lighter && darker, "{f:?}");
    // Bakväggen fylls.
    assert!(f.iter().any(|(_, c)| *c == Rgba::rgb8(0xF0, 0xF0, 0xF0)));
    // Rastret och PDF:en klarar diagrammet.
    assert!(render_png(&scene).len() == 400 * 300 * 4);
    assert!(super::pdf::write(&d, &[0]).is_ok());
}

#[test]
fn diagram_i_3d_alla_typer_och_former() {
    let persp = r#"<c:view3D><c:rotX val="20"/><c:rotY val="30"/><c:depthPercent val="150"/><c:rAngAx val="0"/><c:perspective val="30"/></c:view3D>"#;
    let cases = [
        bar_3d("bar3DChart", "", persp).replace("<c:shape val=\"box\"/>", "<c:shape val=\"cylinder\"/>"),
        bar_3d("bar3DChart", "", persp).replace("<c:shape val=\"box\"/>", "<c:shape val=\"coneToMax\"/>").replace("<c:barDir val=\"col\"/>", "<c:barDir val=\"bar\"/>"),
        bar_3d("bar3DChart", "", persp).replace("<c:shape val=\"box\"/>", "<c:shape val=\"pyramid\"/>").replace("clustered", "stacked"),
        bar_3d("line3DChart", "", persp).replace("<c:barDir val=\"col\"/>", ""),
        bar_3d("area3DChart", "", persp).replace("<c:barDir val=\"col\"/>", "").replace("clustered", "standard"),
    ];
    for (n, xml) in cases.iter().enumerate() {
        if let Some(dir) = std::env::var_os("KLADD_3D_PNG") {
            let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", xml.as_bytes())]);
            let scene = render::slide_scene(&Deck::open(&bytes).unwrap(), 0);
            let pm = super::raster::render(&scene, 720, 540).unwrap();
            let (w, h, rgba) = super::raster::to_rgba(pm);
            image::RgbaImage::from_raw(w, h, rgba).unwrap().save(std::path::Path::new(&dir).join(format!("3d-{n}.png"))).unwrap();
        }
        let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", xml.as_bytes())]);
        let d = Deck::open(&bytes).unwrap();
        let scene = render::slide_scene(&d, 0);
        let t = texts(&scene.items);
        assert!(!t.iter().any(|x| x == "Diagram"), "ingen platshållare");
        assert!(t.iter().any(|x| x == "B"), "kategorier: {t:?}");
        // Allt hamnar inom diagramramen (72..472 × 72..372 pt).
        for (b, _) in fills(&scene.items).into_iter().filter(|(b, _)| b.2 - b.0 < 700.0) {
            assert!(b.0 >= 71.0 && b.2 <= 473.0 && b.1 >= 71.0 && b.3 <= 373.0, "{b:?} utanför ramen");
        }
        assert!(fills(&scene.items).len() > 4, "fall {n}: {}", fills(&scene.items).len());
        render_png(&scene);
    }
}

#[test]
fn cirkel_i_3d_har_sidor_med_overtoning() {
    let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<c:chart><c:autoTitleDeleted val="1"/><c:view3D><c:rotX val="30"/><c:rotY val="0"/><c:rAngAx val="0"/></c:view3D><c:plotArea><c:layout/>
<c:pie3DChart><c:varyColors val="1"/><c:ser><c:idx val="0"/><c:order val="0"/><c:explosion val="10"/>
<c:val><c:numLit><c:ptCount val="2"/><c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="1"><c:v>3</c:v></c:pt></c:numLit></c:val></c:ser></c:pie3DChart>
</c:plotArea></c:chart></c:chartSpace>"#;
    let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", xml.as_bytes())]);
    let d = Deck::open(&bytes).unwrap();
    let scene = render::slide_scene(&d, 0);
    // Topparna i accent1/accent2 ritas sist; sidorna är vågräta övertoningar, mörkare än toppen.
    let f = fills(&scene.items);
    let tops: Vec<usize> = f.iter().enumerate().filter(|(_, (_, c))| *c == Rgba::rgb8(0x44, 0x72, 0xC4) || *c == Rgba::rgb8(0xED, 0x7D, 0x31)).map(|(i, _)| i).collect();
    assert_eq!(tops.len(), 2);
    let grads: Vec<&Item> = scene.items.iter().filter(|i| matches!(i, Item::Fill { paint: Paint::Linear { .. }, .. })).collect();
    assert!(!grads.is_empty(), "sidor med övertoning");
    let last_grad = scene.items.iter().rposition(|i| matches!(i, Item::Fill { paint: Paint::Linear { .. }, .. })).unwrap();
    let first_top = scene.items.iter().position(|i| matches!(i, Item::Fill { paint: Paint::Solid(c), .. } if *c == Rgba::rgb8(0x44, 0x72, 0xC4) || *c == Rgba::rgb8(0xED, 0x7D, 0x31))).unwrap();
    assert!(last_grad < first_top, "sidorna före topparna");
    for g in grads {
        if let Item::Fill { paint: Paint::Linear { stops, .. }, .. } = g {
            assert!(stops.iter().all(|s| s.color.r < 0.95 && s.color.b < 0.8));
        }
    }
}

/// En fylld rektangel med `extra` i `spPr` (efter fyllningen).
fn shape_3d(extra: &str) -> String {
    format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Form"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="1270000" y="1270000"/><a:ext cx="2540000" cy="1270000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="4472C4"/></a:solidFill>{extra}</p:spPr></p:sp>"#)
}

#[test]
fn rakt_framifran_utan_fasning_ger_samma_bild() {
    let plain = Deck::open(&deck(&shape_3d(""), &[])).unwrap();
    let front = Deck::open(&deck(&shape_3d(r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d/>"#), &[])).unwrap();
    let a = super::raster::render(&render::slide_scene(&plain, 0), 480, 360).unwrap();
    let b = super::raster::render(&render::slide_scene(&front, 0), 480, 360).unwrap();
    assert!(a.data() == b.data(), "samma bildpunkter");
    // Med en fasning blir det skillnad.
    let bev = Deck::open(&deck(&shape_3d(r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d><a:bevelT w="127000" h="76200"/></a:sp3d>"#), &[])).unwrap();
    let c = super::raster::render(&render::slide_scene(&bev, 0), 480, 360).unwrap();
    assert!(a.data() != c.data());
}

#[test]
fn pdf_med_3d_effekter() {
    let plain = super::pdf::write(&Deck::open(&deck(&shape_3d(""), &[])).unwrap(), &[0]).unwrap();
    for extra in [
        r#"<a:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d><a:bevelT w="127000" h="76200" prst="coolSlant"/></a:sp3d>"#,
        r#"<a:sp3d contourW="25400"><a:contourClr><a:srgbClr val="FF0000"/></a:contourClr></a:sp3d>"#,
        r#"<a:scene3d><a:camera prst="isometricOffAxis1Right"/><a:lightRig rig="threePt" dir="t"/></a:scene3d><a:sp3d extrusionH="127000"><a:bevelT/></a:sp3d>"#,
    ] {
        let d = Deck::open(&deck(&shape_3d(extra), &[])).unwrap();
        let pdf = super::pdf::write(&d, &[0]).expect("PDF");
        assert!(pdf.len() > plain.len() + 200, "3D-effekten som rasterbild i PDF:en: {} mot {}", pdf.len(), plain.len());
    }
}

/// Fyllningar i en serie färgs kulör (färgen skalad mot svart eller blandad mot vitt).
fn of_hue(f: &[((f32, f32, f32, f32), Rgba)], base: Rgba) -> usize {
    let n = |c: Rgba| {
        let m = c.r.max(c.g).max(c.b).max(1e-3);
        (c.r / m, c.g / m, c.b / m)
    };
    let (br, bg, bb) = n(base);
    f.iter()
        .filter(|(b, c)| b.2 - b.0 < 700.0 && c.r.max(c.g).max(c.b) > 0.05)
        .filter(|(_, c)| {
            // Mörkare: samma förhållanden. Ljusare: avståndet till vitt i samma förhållanden.
            let (r, g, b) = n(*c);
            let dark = (r - br).abs() < 0.08 && (g - bg).abs() < 0.08 && (b - bb).abs() < 0.08;
            let w = |x: f32| 1.0 - x;
            let wm = w(c.r).max(w(c.g)).max(w(c.b)).max(1e-3);
            let bw = w(base.r).max(w(base.g)).max(w(base.b)).max(1e-3);
            let light = (w(c.r) / wm - w(base.r) / bw).abs() < 0.08 && (w(c.g) / wm - w(base.g) / bw).abs() < 0.08 && (w(c.b) / wm - w(base.b) / bw).abs() < 0.08;
            dark || light
        })
        .count()
}

#[test]
fn linjediagram_i_3d_ritar_varje_serie() {
    // Serie 2 (grön i spPr) saknar mittvärdet. Utan `dispBlanksAs` (standard `zero`) och med `span`
    // ritas båda serierna som band; med `gap` har serie 2 inga två grannar men serie 1 ritas.
    let accent1 = Rgba::rgb8(0x44, 0x72, 0xC4);
    let green = Rgba::rgb8(0, 0xFF, 0);
    let view = r#"<c:view3D><c:rotX val="15"/><c:rotY val="20"/><c:rAngAx val="0"/></c:view3D>"#;
    for (blanks, both) in [("", true), (r#"<c:dispBlanksAs val="span"/>"#, true), (r#"<c:dispBlanksAs val="gap"/>"#, false)] {
        let xml = bar_3d("line3DChart", "", view).replace("<c:barDir val=\"col\"/>", "").replace("</c:plotArea>", &format!("</c:plotArea>{blanks}"));
        let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", xml.as_bytes())]);
        let scene = render::slide_scene(&Deck::open(&bytes).unwrap(), 0);
        let f = fills(&scene.items);
        // Bandet: ovansida, framsida, undersida och ändar per sträcka (förklaringens nyckel är en till).
        assert!(of_hue(&f, accent1) >= 4, "serie 1 ({blanks}): {f:?}");
        let g = of_hue(&f, green);
        if both {
            assert!(g >= 4, "serie 2 ({blanks}): {g}");
        } else {
            assert_eq!(g, 1, "bara förklaringens nyckel ({blanks})");
        }
        // Bandets ovansida är ljusare än framsidan: tydligt djup.
        assert!(f.iter().any(|(_, c)| c.b > accent1.b + 0.02 && c.r > accent1.r), "ljus ovansida");
    }
}

// ---- Sekundäraxlar, trendlinjer, felstaplar och radar (w-pchart2) ----

const C_NS: &str = r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;

fn num_lit(vals: &[f64]) -> String {
    let pts: String = vals.iter().enumerate().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
    format!(r#"<c:numLit><c:ptCount val="{}"/>{pts}</c:numLit>"#, vals.len())
}

fn str_lit(vals: &[&str]) -> String {
    let pts: String = vals.iter().enumerate().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
    format!(r#"<c:strLit><c:ptCount val="{}"/>{pts}</c:strLit>"#, vals.len())
}

fn ser(idx: usize, name: &str, cats: &[&str], vals: &[f64], extra: &str) -> String {
    format!(r#"<c:ser><c:idx val="{idx}"/><c:order val="{idx}"/><c:tx><c:v>{name}</c:v></c:tx>{extra}<c:cat>{}</c:cat><c:val>{}</c:val></c:ser>"#, str_lit(cats), num_lit(vals))
}

fn axes(cat: &str, val: &str, val_pos: &str, crosses: &str, deleted_cat: bool) -> String {
    format!(
        r#"<c:catAx><c:axId val="{cat}"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="{}"/><c:axPos val="b"/><c:crossAx val="{val}"/></c:catAx><c:valAx><c:axId val="{val}"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="{val_pos}"/>{}<c:numFmt formatCode="General" sourceLinked="0"/><c:crossAx val="{cat}"/><c:crosses val="{crosses}"/></c:valAx>"#,
        u8::from(deleted_cat),
        if val_pos == "l" { "<c:majorGridlines/>" } else { "" }
    )
}

fn chart_xml(plot: &str) -> String {
    format!(r#"<c:chartSpace {C_NS}><c:chart><c:autoTitleDeleted val="1"/><c:plotArea><c:layout/>{plot}</c:plotArea><c:legend><c:legendPos val="b"/></c:legend></c:chart></c:chartSpace>"#)
}

/// Ritar ett diagram och sparar bilden i `KLADD_CHART_PNG` (för att titta på den).
fn render_chart(xml: &str, name: &str) -> super::scene::Scene {
    let bytes = deck(&chart_frame(), &[("charts/chart1.xml", "chart", xml.as_bytes())]);
    let d = Deck::open(&bytes).unwrap();
    let scene = render::slide_scene(&d, 0);
    if let Some(dir) = std::env::var_os("KLADD_CHART_PNG") {
        let pm = super::raster::render(&scene, 720, 540).unwrap();
        let (w, h, rgba) = super::raster::to_rgba(pm);
        image::RgbaImage::from_raw(w, h, rgba).unwrap().save(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
    }
    assert!(super::pdf::write(&d, &[0]).is_ok());
    scene
}

const CATS: [&str; 4] = ["Q1", "Q2", "Q3", "Q4"];

#[test]
fn sekundar_axel_har_egen_skala_till_hoger() {
    // Staplar 10–40 på den primära axeln, linje 0,1–0,4 på den sekundära (till höger, `crosses max`).
    let plot = format!(
        r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>{}<c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:lineChart><c:grouping val="standard"/>{}<c:marker val="1"/><c:axId val="3"/><c:axId val="4"/></c:lineChart>{}{}"#,
        ser(0, "Intäkt", &CATS, &[10.0, 20.0, 30.0, 40.0], ""),
        ser(1, "Andel", &CATS, &[0.1, 0.4, 0.2, 0.3], ""),
        axes("1", "2", "l", "autoZero", false),
        axes("3", "4", "r", "max", true),
    );
    let scene = render_chart(&chart_xml(&plot), "sekundaraxel");
    let t = texts(&scene.items);
    // Båda skalorna: 0–45 till vänster, 0–0,45 till höger.
    for want in ["45", "0.45", "0.1", "Q1"] {
        assert!(t.iter().any(|x| x == want), "saknar {want:?} i {t:?}");
    }
    // Den sekundära axelns etiketter står till höger om de primära.
    let x_of = |s: &str| scene.items.iter().find_map(|i| if let Item::Glyphs(g) = i { (g.text.trim() == s).then(|| g.t.apply(0.0, 0.0).0) } else { None }).unwrap();
    assert!(x_of("0.45") > x_of("45") + 200.0);
    // Linjens högsta punkt (0,4) ligger nära ritytans överkant, inte nere vid staplarnas 0,4.
    let line_top = scene.items.iter().filter_map(|i| if let Item::Stroke { path, stroke } = i { (stroke.width > 2.0).then(|| path.bounds()).flatten() } else { None }).map(|b| b.1).fold(f32::INFINITY, f32::min);
    let bar_top = fills(&scene.items).iter().filter(|(_, c)| *c == Rgba::rgb8(0x44, 0x72, 0xC4)).map(|(b, _)| b.1).fold(f32::INFINITY, f32::min);
    assert!(line_top < bar_top + 40.0, "linjen {line_top} staplarna {bar_top}");
}

#[test]
fn trendlinjer_med_ekvation_och_r2() {
    let trend = |t: &str, extra: &str| format!(r#"<c:trendline><c:spPr><a:ln w="19050"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:prstDash val="sysDash"/></a:ln></c:spPr><c:trendlineType val="{t}"/>{extra}<c:dispRSqr val="1"/><c:dispEq val="1"/></c:trendline>"#);
    let plot = format!(
        r#"<c:scatterChart><c:scatterStyle val="lineMarker"/>{}<c:axId val="1"/><c:axId val="2"/></c:scatterChart><c:valAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:crossAx val="2"/></c:valAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:crossAx val="1"/></c:valAx>"#,
        format!(
            r#"<c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>Mätning</c:v></c:tx><c:spPr><a:ln><a:noFill/></a:ln></c:spPr>{}<c:xVal>{}</c:xVal><c:yVal>{}</c:yVal></c:ser>"#,
            trend("linear", r#"<c:forward val="1"/>"#),
            num_lit(&[1.0, 2.0, 3.0, 4.0, 5.0]),
            num_lit(&[1.0, 3.0, 2.0, 5.0, 4.0])
        )
    );
    let scene = render_chart(&chart_xml(&plot), "trendlinje");
    let t: String = texts(&scene.items).join(" ");
    // y = 0,8x + 0,6 och R² = 0,64 (som Excel).
    assert!(t.contains("y = 0.8x + 0.6"), "{t}");
    assert!(t.contains("R² = 0.64"), "{t}");
    // Den röda streckade linjen ritas, klippt till ritytan.
    assert!(scene.items.iter().any(|i| matches!(i, Item::Stroke { stroke, .. } if matches!(stroke.paint, Paint::Solid(c) if c == Rgba::rgb8(255, 0, 0)) && !stroke.dash.is_empty())));
    assert!(scene.items.iter().any(|i| matches!(i, Item::PushClip { .. })));
    // Polynom och glidande medel i ett linjediagram.
    let plot = format!(
        r#"<c:lineChart><c:grouping val="standard"/>{}<c:marker val="1"/><c:axId val="1"/><c:axId val="2"/></c:lineChart>{}"#,
        ser(0, "A", &CATS, &[3.0, 1.0, 2.0, 6.0], &format!("{}{}", trend("poly", r#"<c:order val="2"/>"#).replace("<c:dispRSqr val=\"1\"/><c:dispEq val=\"1\"/>", ""), trend("movingAvg", r#"<c:period val="2"/>"#).replace("<c:dispRSqr val=\"1\"/><c:dispEq val=\"1\"/>", ""))),
        axes("1", "2", "l", "autoZero", false)
    );
    let scene = render_chart(&chart_xml(&plot), "trendlinje-polynom");
    let reds = scene.items.iter().filter(|i| matches!(i, Item::Stroke { stroke, .. } if matches!(stroke.paint, Paint::Solid(c) if c == Rgba::rgb8(255, 0, 0)))).count();
    assert_eq!(reds, 2, "polynom och glidande medel");
}

#[test]
fn felstaplar_pa_staplar_och_punkter() {
    let err = |extra: &str| format!(r#"<c:errBars>{extra}<c:spPr><a:ln w="12700"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln></c:spPr></c:errBars>"#);
    let plot = format!(
        r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>{}{}<c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart>{}"#,
        ser(0, "A", &CATS, &[10.0, 20.0, 30.0, 25.0], &err(r#"<c:errBarType val="both"/><c:errValType val="fixedVal"/><c:noEndCap val="0"/><c:val val="5"/>"#)),
        ser(1, "B", &CATS, &[15.0, 10.0, 20.0, 30.0], &err(&format!(r#"<c:errBarType val="plus"/><c:errValType val="cust"/><c:noEndCap val="1"/><c:plus>{}</c:plus>"#, num_lit(&[2.0, 4.0, 6.0, 8.0])))),
        axes("1", "2", "l", "autoZero", false)
    );
    let scene = render_chart(&chart_xml(&plot), "felstaplar");
    let black: Vec<&Item> = scene.items.iter().filter(|i| matches!(i, Item::Stroke { stroke, .. } if matches!(stroke.paint, Paint::Solid(c) if c == Rgba::BLACK) && (stroke.width - 1.0).abs() < 1e-3)).collect();
    assert_eq!(black.len(), 2, "en bana per serie");
    // Serie A: ±5 kring 30 vid Q3 → från 25 till 35; serie B: bara plus.
    let Item::Stroke { path, .. } = black[0] else { unreachable!() };
    let (_, y0, _, y1) = path.bounds().unwrap();
    // Höjden motsvarar 10 + 5 + 5 enheter i skalan 0–35 (5 ovanför 30): större än noll och mindre än ritytan.
    assert!(y1 - y0 > 20.0 && y1 - y0 < 300.0, "{y0} {y1}");
}

#[test]
fn radardiagram_med_rutnat_och_serier() {
    let plot = format!(
        r#"<c:radarChart><c:radarStyle val="marker"/><c:varyColors val="0"/>{}{}<c:axId val="1"/><c:axId val="2"/></c:radarChart>{}"#,
        ser(0, "Lag 1", &["Fart", "Kraft", "Teknik", "Taktik", "Ork"], &[4.0, 3.0, 5.0, 2.0, 4.0], ""),
        ser(1, "Lag 2", &["Fart", "Kraft", "Teknik", "Taktik", "Ork"], &[2.0, 5.0, 3.0, 4.0, 3.0], ""),
        axes("1", "2", "l", "autoZero", false).replace(r#"<c:axPos val="b"/>"#, r#"<c:axPos val="b"/><c:majorGridlines/>"#)
    );
    let scene = render_chart(&chart_xml(&plot), "radar");
    let t = texts(&scene.items);
    for want in ["Fart", "Kraft", "Teknik", "Taktik", "Ork", "Lag 1", "Lag 2", "6"] {
        assert!(t.iter().any(|x| x == want), "saknar {want:?} i {t:?}");
    }
    assert!(!t.iter().any(|x| x == "Diagram"), "ingen platshållare");
    // "Fart" står överst (första ekern klockan tolv), "Teknik" nedanför mitten till höger.
    let at = |s: &str| scene.items.iter().find_map(|i| if let Item::Glyphs(g) = i { (g.text.trim() == s).then(|| g.t.apply(0.0, 0.0)) } else { None }).unwrap();
    let (fart, kraft, teknik) = (at("Fart"), at("Kraft"), at("Teknik"));
    assert!(fart.1 < kraft.1 && kraft.1 < teknik.1 && kraft.0 > fart.0);
    // Fyllt radardiagram.
    let filled = chart_xml(&plot.replace(r#"<c:radarStyle val="marker"/>"#, r#"<c:radarStyle val="filled"/>"#));
    let scene = render_chart(&filled, "radar-fylld");
    let f = fills(&scene.items);
    assert!(f.iter().filter(|(b, c)| *c == Rgba::rgb8(0x44, 0x72, 0xC4) && b.2 - b.0 > 50.0).count() >= 1);
}

#[test]
fn datatabell_under_ritytan() {
    let plot = format!(
        r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>{}{}<c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart>{}<c:dTable><c:showHorzBorder val="1"/><c:showVertBorder val="1"/><c:showOutline val="1"/><c:showKeys val="1"/></c:dTable>"#,
        ser(0, "Norr", &CATS, &[12.0, 20.0, 31.0, 25.0], ""),
        ser(1, "Söder", &CATS, &[15.0, 11.5, 20.0, 30.0], ""),
        axes("1", "2", "l", "autoZero", false)
    );
    let scene = render_chart(&chart_xml(&plot), "datatabell");
    let t = texts(&scene.items);
    for want in ["Q1", "Q4", "Norr", "Söder", "31", "11.5"] {
        assert!(t.iter().any(|x| x == want), "saknar {want:?} i {t:?}");
    }
    // Värdena står i rader under kategorierna: "31" (Norr, Q3) ovanför "20" (Söder, Q3) och under "Q3".
    let at = |s: &str| scene.items.iter().filter_map(|i| if let Item::Glyphs(g) = i { (g.text.trim() == s).then(|| g.t.apply(0.0, 0.0)) } else { None }).collect::<Vec<_>>();
    let q3 = at("Q3")[0];
    let v31 = at("31")[0];
    assert!(v31.1 > q3.1 && (v31.0 - q3.0).abs() < 15.0, "{q3:?} {v31:?}");
    assert!(at("Söder").last().unwrap().1 > at("Norr").last().unwrap().1);
    // Förklaringsnycklarna i tabellen (två extra rutor i seriefärgerna).
    let keys = fills(&scene.items).iter().filter(|(b, c)| *c == Rgba::rgb8(0x44, 0x72, 0xC4) && (b.2 - b.0) < 8.0).count();
    assert!(keys >= 2, "{keys}");
}
