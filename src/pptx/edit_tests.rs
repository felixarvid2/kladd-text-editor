//! Prov för redigeringen och sparandet: tur och retur utan ändring (samma byte), ändrad text och
//! formatering, flytta/ändra storlek, ny bild med layout, ta bort/flytta/duplicera bilder (relationer,
//! `sldIdLst`, avsnitt, länkar), anteckningar, infogade bilder, ångra/gör om, signaturer och format.
//! Varje sparad fil kontrolleras med `validate` (zip, relationer, innehållstyper, unika id).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use super::edit::{self, Session};
use super::edit_ops::{self, Order, Xf};
use super::edit_text::{Pos, TextModel};
use super::edit_xml;
use super::Deck;

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

fn tree(inner: &str) -> String {
    format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#)
}

fn sp_tree(inner: &str) -> String {
    let t = tree(inner);
    t["<p:cSld>".len()..t.len() - "</p:cSld>".len()].to_owned()
}

fn ph_sp(id: u32, ph: &str, xfrm: Option<(i64, i64, i64, i64)>, body: &str) -> String {
    let x = xfrm.map(|(x, y, w, h)| format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)).unwrap_or_default();
    format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="F{id}"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{x}</p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>"#)
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([20, 120, 200, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="T"><a:themeElements><a:clrScheme name="C"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2><a:accent3><a:srgbClr val="9BBB59"/></a:accent3><a:accent4><a:srgbClr val="8064A2"/></a:accent4><a:accent5><a:srgbClr val="4BACC6"/></a:accent5><a:accent6><a:srgbClr val="F79646"/></a:accent6><a:hlink><a:srgbClr val="0000FF"/></a:hlink><a:folHlink><a:srgbClr val="800080"/></a:folHlink></a:clrScheme><a:fontScheme name="F"><a:majorFont><a:latin typeface="Liberation Sans"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Liberation Sans"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="S"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#;

/// En presentation med tema, mall (rubrik och brödtext), två layouter, tre bilder (bild 1 med
/// anteckningar, en animering och en grupp; bild 2 med ett `mc:AlternateContent`; bild 3 med en länk
/// till bild 2), avsnitt, `customXml` och en okänd del. Med `notes_master = false` saknas
/// anteckningsmallen och bild 1 har inga anteckningar.
fn fixture(notes_master: bool) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &[u8]| {
        z.start_file(name, opt).unwrap();
        z.write_all(data).unwrap();
    };
    let ov = |p: &str, t: &str| format!(r#"<Override PartName="/{p}" ContentType="application/vnd.openxmlformats-officedocument.{t}"/>"#);
    let mut ct = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    ct.push_str(r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="bin" ContentType="application/octet-stream"/>"#);
    ct.push_str(&ov("ppt/presentation.xml", "presentationml.presentation.main+xml"));
    for n in 1..=3 {
        ct.push_str(&ov(&format!("ppt/slides/slide{n}.xml"), "presentationml.slide+xml"));
    }
    ct.push_str(&ov("ppt/slideMasters/slideMaster1.xml", "presentationml.slideMaster+xml"));
    ct.push_str(&ov("ppt/slideLayouts/slideLayout1.xml", "presentationml.slideLayout+xml"));
    ct.push_str(&ov("ppt/slideLayouts/slideLayout2.xml", "presentationml.slideLayout+xml"));
    ct.push_str(&ov("ppt/theme/theme1.xml", "theme+xml"));
    ct.push_str(r#"<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#);
    if notes_master {
        ct.push_str(&ov("ppt/notesMasters/notesMaster1.xml", "presentationml.notesMaster+xml"));
        ct.push_str(&ov("ppt/notesSlides/notesSlide1.xml", "presentationml.notesSlide+xml"));
        ct.push_str(&ov("ppt/theme/theme2.xml", "theme+xml"));
    }
    ct.push_str("</Types>");
    put("[Content_Types].xml", ct.as_bytes());
    put("_rels/.rels", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{}<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="rId2" Type="{REL}/extended-properties" Target="docProps/app.xml"/></Relationships>"#, "\r\n").as_bytes());
    put("docProps/app.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>Prov</Application><Slides>3</Slides></Properties>"#);
    put("customXml/item1.xml", b"<okand xmlns=\"urn:x\">data</okand>");
    put("ppt/okand.bin", &[1, 2, 3, 0, 255]);
    let mut prels = format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rIdM" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rIdT" Type="{REL}/theme" Target="theme/theme1.xml"/>"#);
    if notes_master {
        prels.push_str(&format!(r#"<Relationship Id="rIdNM" Type="{REL}/notesMaster" Target="notesMasters/notesMaster1.xml"/>"#));
    }
    let mut ids = String::new();
    for n in 1..=3 {
        prels.push_str(&format!(r#"<Relationship Id="rIdS{n}" Type="{REL}/slide" Target="slides/slide{n}.xml"/>"#));
        ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rIdS{n}"/>"#, 255 + n));
    }
    prels.push_str("</Relationships>");
    put("ppt/_rels/presentation.xml.rels", prels.as_bytes());
    let nm = if notes_master { r#"<p:notesMasterIdLst><p:notesMasterId r:id="rIdNM"/></p:notesMasterIdLst>"# } else { "" };
    let sections = r#"<p:extLst><p:ext uri="{521415D9-36F7-43E2-AB2F-B90AF26B5E84}"><p14:sectionLst xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main"><p14:section name="A" id="{11111111-1111-1111-1111-111111111111}"><p14:sldIdLst><p14:sldId id="256"/><p14:sldId id="257"/></p14:sldIdLst></p14:section><p14:section name="B" id="{22222222-2222-2222-2222-222222222222}"><p14:sldIdLst><p14:sldId id="258"/></p14:sldIdLst></p14:section></p14:sectionLst></p:ext></p:extLst>"#;
    put(
        "ppt/presentation.xml",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<p:presentation {NS} saveSubsetFonts=\"1\"><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rIdM\"/></p:sldMasterIdLst>{nm}<p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx=\"9144000\" cy=\"6858000\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/>{sections}</p:presentation>").as_bytes(),
    );
    put("ppt/theme/theme1.xml", THEME.as_bytes());
    let title = ph_sp(2, r#"<p:ph type="title"/>"#, Some((457200, 274638, 8229600, 1143000)), "<a:p><a:r><a:rPr lang=\"sv-SE\"/><a:t>Mallrubrik</a:t></a:r></a:p>");
    let body = ph_sp(3, r#"<p:ph type="body" idx="1"/>"#, Some((457200, 1600200, 8229600, 4525963)), "<a:p><a:r><a:rPr lang=\"sv-SE\"/><a:t>Malltext</a:t></a:r></a:p>");
    put(
        "ppt/slideMasters/slideMaster1.xml",
        format!(r#"<p:sldMaster {NS}>{}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/><p:sldLayoutId id="2147483650" r:id="rId2"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="4400"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="342900" indent="-342900"><a:buChar char="•"/><a:defRPr sz="3200"/></a:lvl1pPr><a:lvl2pPr marL="742950" indent="-285750"><a:buChar char="–"/><a:defRPr sz="2800"/></a:lvl2pPr></p:bodyStyle></p:txStyles></p:sldMaster>"#, tree(&(title + &body))).as_bytes(),
    );
    put("ppt/slideMasters/_rels/slideMaster1.xml.rels", format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout2.xml"/><Relationship Id="rId3" Type="{REL}/theme" Target="../theme/theme1.xml"/></Relationships>"#).as_bytes());
    let l1 = ph_sp(2, r#"<p:ph type="title"/>"#, None, "<a:p/>") + &ph_sp(3, r#"<p:ph idx="1"/>"#, None, "<a:p/>") + &ph_sp(4, r#"<p:ph type="sldNum" sz="quarter" idx="12"/>"#, None, "<a:p/>");
    put("ppt/slideLayouts/slideLayout1.xml", format!(r#"<p:sldLayout {NS} type="obj"><p:cSld name="Rubrik och innehåll">{}</p:cSld></p:sldLayout>"#, sp_tree(&l1)).as_bytes());
    put("ppt/slideLayouts/slideLayout2.xml", format!(r#"<p:sldLayout {NS} type="blank"><p:cSld name="Tom">{}</p:cSld></p:sldLayout>"#, sp_tree("")).as_bytes());
    for l in 1..=2 {
        put(&format!("ppt/slideLayouts/_rels/slideLayout{l}.xml.rels"), format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#).as_bytes());
    }
    // Bild 1: rubrik, brödtext i två nivåer, en egen form med animering, en grupp och en bild.
    let s1 = ph_sp(2, r#"<p:ph type="title"/>"#, None, r#"<a:p><a:r><a:rPr lang="sv-SE" b="1"/><a:t>Kvartal</a:t></a:r><a:endParaRPr lang="sv-SE"/></a:p>"#)
        + &ph_sp(3, r#"<p:ph idx="1"/>"#, None, r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>Första</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:rPr lang="sv-SE"/><a:t>Andra</a:t></a:r></a:p>"#)
        + r#"<p:sp><p:nvSpPr><p:cNvPr id="4" name="Ruta"><a:extLst><a:ext uri="{FF2B5EF4-FFF2-40B4-BE49-F238E27FC236}"><a16:creationId xmlns:a16="http://schemas.microsoft.com/office/drawing/2014/main" id="{AAAA}"/></a:ext></a:extLst></p:cNvPr><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="600000"><a:off x="1000000" y="4000000"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></p:spPr></p:sp>"#
        + r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="5" name="Grupp"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="5000000" y="4000000"/><a:ext cx="1000000" cy="1000000"/><a:chOff x="0" y="0"/><a:chExt cx="1000000" cy="1000000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="6" name="Inre"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="500000" cy="500000"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom></p:spPr></p:sp></p:grpSp>"#
        + r#"<p:pic><p:nvPicPr><p:cNvPr id="7" name="Bild"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="7000000" y="4000000"/><a:ext cx="1000000" cy="750000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#;
    let timing = r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst><p:par><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="4" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:par><p:cTn id="5" presetID="1" presetClass="entr" presetSubtype="0" fill="hold" nodeType="clickEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:set><p:cBhvr><p:cTn id="6" dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="4"/></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn><p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst><p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst><p:bldLst><p:bldP spid="4" grpId="0" animBg="1"/></p:bldLst></p:timing>"#;
    // Bild 2: en form i `mc:AlternateContent` (p14-gren och reserv med samma id).
    let s2 = ph_sp(2, r#"<p:ph type="title"/>"#, None, "<a:p><a:r><a:rPr lang=\"sv-SE\"/><a:t>Andra bilden</a:t></a:r></a:p>")
        + r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" Requires="p14"><p:sp><p:nvSpPr><p:cNvPr id="3" name="Alt"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100000" y="5000000"/><a:ext cx="1000000" cy="500000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:sp></mc:Choice><mc:Fallback><p:sp><p:nvSpPr><p:cNvPr id="3" name="Alt"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100000" y="5000000"/><a:ext cx="1000000" cy="500000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:sp></mc:Fallback></mc:AlternateContent>"#;
    // Bild 3: en textruta med en länk till bild 2.
    let s3 = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Text"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="457200" y="457200"/><a:ext cx="6000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE"><a:hlinkClick r:id="rIdL" action="ppaction://hlinksldjump"/></a:rPr><a:t>Till bild 2</a:t></a:r></a:p></p:txBody></p:sp>"#.to_owned();
    let slides = [(s1, timing), (s2, ""), (s3, "")];
    for (i, (inner, extra)) in slides.iter().enumerate() {
        let n = i + 1;
        put(&format!("ppt/slides/slide{n}.xml"), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<p:sld {NS}>{}<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>{extra}</p:sld>", tree(inner)).as_bytes());
        let mut r = format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#);
        if n == 1 {
            r.push_str(&format!(r#"<Relationship Id="rIdImg" Type="{REL}/image" Target="../media/image1.png"/>"#));
            if notes_master {
                r.push_str(&format!(r#"<Relationship Id="rIdN" Type="{REL}/notesSlide" Target="../notesSlides/notesSlide1.xml"/>"#));
            }
        }
        if n == 3 {
            r.push_str(&format!(r#"<Relationship Id="rIdL" Type="{REL}/slide" Target="slide2.xml"/>"#));
        }
        r.push_str("</Relationships>");
        put(&format!("ppt/slides/_rels/slide{n}.xml.rels"), r.as_bytes());
    }
    put("ppt/media/image1.png", &png(4, 3));
    if notes_master {
        put("ppt/theme/theme2.xml", THEME.as_bytes());
        put("ppt/notesMasters/notesMaster1.xml", format!(r#"<p:notesMaster {NS}>{}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:notesMaster>"#, tree(&ph_sp(2, r#"<p:ph type="body" idx="1"/>"#, Some((685800, 4343400, 5486400, 4114800)), "<a:p/>"))).as_bytes());
        put("ppt/notesMasters/_rels/notesMaster1.xml.rels", format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/theme" Target="../theme/theme2.xml"/></Relationships>"#).as_bytes());
        put("ppt/notesSlides/notesSlide1.xml", format!(r#"<p:notes {NS}>{}</p:notes>"#, tree(&ph_sp(2, r#"<p:ph type="body" idx="1"/>"#, None, r#"<a:p><a:r><a:rPr lang="sv-SE" i="1"/><a:t>Gamla anteckningar</a:t></a:r></a:p>"#))).as_bytes());
        put("ppt/notesSlides/_rels/notesSlide1.xml.rels", format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{REL}/notesMaster" Target="../notesMasters/notesMaster1.xml"/><Relationship Id="rId2" Type="{REL}/slide" Target="../slides/slide1.xml"/></Relationships>"#).as_bytes());
    }
    z.finish().unwrap().into_inner()
}

// ------------------------------------------------------------------ kontroll av sparade filer

/// Zip-filens delar (namn → innehåll) i ordning.
fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip läsbar");
    (0..z.len())
        .filter_map(|i| {
            // Trasiga poster (provfiler för krascher) räknas som tomma.
            let mut f = z.by_index(i).ok()?;
            let mut b = Vec::new();
            let _ = f.read_to_end(&mut b);
            Some((f.name().to_owned(), b))
        })
        .collect()
}

/// Strukturkontroll av en sparad presentation. Ger felen (tom lista = giltig).
pub fn validate(bytes: &[u8]) -> Vec<String> {
    let mut errs = Vec::new();
    let Ok(mut z) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else { return vec!["zip går inte att läsa".into()] };
    let mut parts: HashMap<String, Vec<u8>> = HashMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        if f.read_to_end(&mut b).is_err() {
            errs.push(format!("{} går inte att packa upp", f.name()));
        }
        if parts.insert(f.name().to_owned(), b).is_some() {
            errs.push(format!("{} finns två gånger", f.name()));
        }
    }
    let pkg = super::package::Package::from_parts(parts.iter().map(|(k, v)| (k.clone(), std::sync::Arc::new(v.clone()))).collect());
    // Innehållstyper: varje del (utom [Content_Types].xml) har en; varje Override pekar på en del.
    for name in parts.keys() {
        if name != "[Content_Types].xml" && pkg.content_type(name).is_none() {
            errs.push(format!("{name} saknar innehållstyp"));
        }
    }
    for p in pkg.content_types.keys() {
        if !parts.contains_key(p) {
            errs.push(format!("Override för {p} som saknas"));
        }
    }
    // Relationer: ägaren finns, interna mål finns, r:id i XML:en finns bland ägarens relationer.
    for name in parts.keys().filter(|n| n.ends_with(".rels")) {
        let owner = name.replace("_rels/", "").trim_end_matches(".rels").to_owned();
        let owner = if owner.is_empty() || name == "_rels/.rels" { String::new() } else { owner };
        if !owner.is_empty() && !parts.contains_key(&owner) {
            errs.push(format!("{name}: ägaren saknas"));
        }
        for r in pkg.rels(&owner) {
            // Mål som börjar med `#` (ankare) räknas inte (vissa verktyg skriver dem utan TargetMode).
            if !r.external && !parts.contains_key(&r.target) && !r.target.contains('#') {
                errs.push(format!("{name}: {} pekar på {} som saknas", r.id, r.target));
            }
        }
    }
    for (name, b) in &parts {
        if !name.ends_with(".xml") || name == "[Content_Types].xml" {
            continue;
        }
        let Some(doc) = edit_xml::parse(b) else {
            errs.push(format!("{name}: XML går inte att tolka"));
            continue;
        };
        // Sluttaggarna och prefixen kontrolleras strikt (`edit_xml`/`xml.rs` släpper igenom fel namn
        // och odeklarerade prefix).
        let text = String::from_utf8_lossy(b);
        let mut r = quick_xml::NsReader::from_str(&text);
        loop {
            match r.read_resolved_event() {
                Ok((_, quick_xml::events::Event::Eof)) => break,
                Ok((quick_xml::name::ResolveResult::Unknown(p), _)) => {
                    errs.push(format!("{name}: prefixet {} är inte deklarerat", String::from_utf8_lossy(&p)));
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    errs.push(format!("{name}: {e}"));
                    break;
                }
            }
        }
        let ids: HashSet<String> = pkg.rels(name).into_iter().map(|r| r.id).collect();
        doc.root.walk(&mut |e| {
            for a in &e.attrs {
                if a.name.starts_with("r:") && matches!(&a.name[2..], "id" | "embed" | "link" | "pict" | "dm" | "lo" | "qs" | "cs") {
                    let v = edit_xml::unescape(&a.raw);
                    if !v.is_empty() && !ids.contains(&v) {
                        errs.push(format!("{name}: {}=\"{v}\" saknar relation", a.name));
                    }
                }
            }
        });
        // Unika form-id per bild (grenarna i ett mc:AlternateContent räknas som en form).
        if name.starts_with("ppt/slides/slide") {
            let mut seen = HashSet::new();
            if let Some(tree) = doc.root.path(&["p:cSld", "p:spTree"]) {
                for top in tree.elems() {
                    let mut mine = HashSet::new();
                    if top.name == "p:nvGrpSpPr" {
                        continue;
                    }
                    top.walk(&mut |e| {
                        if e.name == "p:cNvPr" {
                            mine.insert(e.attr("id").unwrap_or_default());
                        }
                    });
                    for id in mine {
                        if !seen.insert(id.clone()) {
                            errs.push(format!("{name}: id {id} finns två gånger"));
                        }
                    }
                }
            }
        }
    }
    // sldIdLst: id ≥ 256, unika, relationerna är bilder.
    let pres = "ppt/presentation.xml";
    if let Some(doc) = parts.get(pres).and_then(|b| edit_xml::parse(b)) {
        let rels = pkg.rels(pres);
        let mut seen = HashSet::new();
        for e in doc.root.child("p:sldIdLst").map(|l| l.elems().collect::<Vec<_>>()).unwrap_or_default() {
            let id = e.attr_i64("id").unwrap_or(0);
            if !(256..2_147_483_648).contains(&id) || !seen.insert(id) {
                errs.push(format!("sldId {id} ogiltigt eller dubbelt"));
            }
            let rid = e.attr("r:id").unwrap_or_default();
            if !rels.iter().any(|r| r.id == rid && r.kind == "slide") {
                errs.push(format!("sldId {id}: {rid} är ingen bildrelation"));
            }
        }
        // Avsnitten nämner bara bilder som finns.
        let mut sec = Vec::new();
        doc.root.walk(&mut |e| {
            if e.name == "p14:sldId" {
                sec.push(e.attr_i64("id").unwrap_or(0));
            }
        });
        for s in &sec {
            if !seen.contains(s) {
                errs.push(format!("avsnitt nämner bild-id {s} som saknas"));
            }
        }
    }
    if Deck::open(bytes).is_err() {
        errs.push("Kladd öppnar inte filen".into());
    }
    errs
}

fn assert_valid(bytes: &[u8]) {
    let e = validate(bytes);
    assert!(e.is_empty(), "ogiltig fil: {e:#?}");
}

// ------------------------------------------------------------------ hjälpare

struct Ed {
    deck: Deck,
    s: Session,
}

impl Ed {
    fn new(bytes: Vec<u8>) -> Ed {
        let deck = Deck::open(&bytes).unwrap();
        let s = Session::new(bytes, &deck);
        Ed { deck, s }
    }

    fn apply(&mut self, f: impl FnOnce(&mut edit::Txn) -> Result<(), String>) {
        let a = self.s.apply(&self.deck, None, f).unwrap().expect("ändring");
        self.deck = a.deck;
    }

    fn save(&mut self) -> Vec<u8> {
        let (b, d) = self.s.save_bytes(&self.deck, "pptx", "pptx").unwrap();
        if let Some(d) = d {
            self.deck = d;
        }
        assert_valid(&b);
        b
    }

    fn text(&self, slide: usize) -> String {
        super::render::slide_scene(&self.deck, slide).text()
    }

    fn shape(&self, slide: usize, name: &str) -> edit_ops::ShapeInfo {
        edit_ops::shapes(&self.deck, slide).into_iter().find(|s| s.name == name).unwrap_or_else(|| panic!("formen {name}"))
    }
}

fn part<'a>(e: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    &e.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name} saknas")).1
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

// ------------------------------------------------------------------ prov

#[test]
fn fixturen_ar_giltig() {
    assert_valid(&fixture(true));
    assert_valid(&fixture(false));
}

#[test]
fn tur_och_retur_utan_andring_ger_samma_delar() {
    let orig = fixture(true);
    let mut ed = Ed::new(orig.clone());
    assert!(!ed.s.dirty());
    let out = ed.save();
    assert_eq!(entries(&out), entries(&orig), "samma delar i samma ordning");
    // Korpusfilerna (python-pptx, PowerPoint-sparade) likaså, och XML-delarna tolkas förlustfritt.
    for f in std::fs::read_dir("tests/corpus/pptx").unwrap().flatten() {
        let p = f.path();
        if p.extension().is_none_or(|e| e != "pptx") {
            continue;
        }
        let bytes = std::fs::read(&p).unwrap();
        for (name, b) in entries(&bytes) {
            if name.ends_with(".xml") || name.ends_with(".rels") {
                let d = edit_xml::parse(&b).unwrap_or_else(|| panic!("{} {name}", p.display()));
                assert_eq!(d.to_bytes(), b, "{} {name} förlustfritt", p.display());
            }
        }
        let mut ed = Ed::new(bytes.clone());
        let out = ed.save();
        assert_eq!(entries(&out), entries(&bytes), "{}", p.display());
    }
}

#[test]
fn andrad_text_sparas_med_formatering_och_resten_orort() {
    let orig = fixture(true);
    let mut ed = Ed::new(orig.clone());
    let title = ed.shape(0, "F2");
    let part0 = ed.deck.slides[0].part.clone();
    ed.apply(|t| {
        let doc = t.doc(&part0)?;
        let scope = doc.root_scope();
        let sp = doc.root.path_mut(&["p:cSld", "p:spTree"]).unwrap().elems_mut().find(|e| e.name == "p:sp").unwrap();
        let mut m = TextModel::from_body(sp.child("p:txBody"), &scope, "a");
        let p = m.insert(Pos { para: 0, ch: 7 }, "srapport");
        m.split_para(p);
        m.write_into(sp, "p");
        Ok(())
    });
    let _ = title;
    assert!(ed.s.dirty());
    let out = ed.save();
    let e = entries(&out);
    let x = s(part(&e, "ppt/slides/slide1.xml"));
    assert!(x.contains(r#"<a:r><a:rPr lang="sv-SE" b="1"/><a:t>Kvartalsrapport</a:t></a:r><a:endParaRPr lang="sv-SE"/></a:p><a:p><a:endParaRPr lang="sv-SE"/></a:p>"#), "{x}");
    // Allt utom bild 1 är byte för byte detsamma, i samma ordning.
    let o = entries(&orig);
    assert_eq!(e.iter().map(|(n, _)| n).collect::<Vec<_>>(), o.iter().map(|(n, _)| n).collect::<Vec<_>>());
    for ((n, a), (_, b)) in e.iter().zip(&o) {
        if n != "ppt/slides/slide1.xml" {
            assert_eq!(a, b, "{n}");
        }
    }
    // Öppnas igen: texten finns och ritas fetstil.
    let d = Deck::open(&out).unwrap();
    let scene = super::render::slide_scene(&d, 0);
    assert!(scene.text().contains("Kvartalsrapport"));
}

#[test]
fn flytta_och_andra_storlek_aven_arvd_placering() {
    let mut ed = Ed::new(fixture(true));
    let r = ed.shape(0, "Ruta");
    assert_eq!(r.xf.rot, 600000);
    let xf = Xf { x: 2_000_000, y: 1_000_000, cx: 3_000_000, cy: 1_500_000, ..r.xf };
    ed.apply(|t| edit_ops::set_xf(t, 0, r.index, r.id, xf));
    assert_eq!(ed.shape(0, "Ruta").xf, xf);
    // Rubriken ärver läget från mallen: en egen a:xfrm skapas först i spPr.
    let title = ed.shape(0, "F2");
    assert_eq!(title.xf.x, 457200);
    let moved = Xf { x: 100, y: 200, ..title.xf };
    ed.apply(|t| edit_ops::set_xf(t, 0, title.index, title.id, moved));
    assert_eq!(ed.shape(0, "F2").xf, moved);
    // Gruppen: barnen följer med.
    let g = ed.shape(0, "Grupp");
    ed.apply(|t| edit_ops::set_xf(t, 0, g.index, g.id, Xf { x: 0, y: 0, cx: 2_000_000, cy: 2_000_000, ..g.xf }));
    let out = ed.save();
    let x = s(part(&entries(&out), "ppt/slides/slide1.xml"));
    assert!(x.contains(r#"<p:spPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="8229600" cy="1143000"/></a:xfrm></p:spPr>"#), "{x}");
    assert!(x.contains(r#"<a:xfrm rot="600000"><a:off x="2000000" y="1000000"/><a:ext cx="3000000" cy="1500000"/></a:xfrm>"#), "{x}");
    assert!(x.contains(r#"<a:chOff x="0" y="0"/><a:chExt cx="1000000" cy="1000000"/>"#));
    // Ångra tre gånger: tillbaka till originalet (inte ändrad).
    for _ in 0..3 {
        ed.deck = ed.s.undo(&ed.deck).unwrap().unwrap();
    }
    assert!(!ed.s.dirty());
    assert_eq!(ed.shape(0, "Ruta").xf, r.xf);
    ed.deck = ed.s.redo(&ed.deck).unwrap().unwrap();
    assert_eq!(ed.shape(0, "Ruta").xf, xf);
}

#[test]
fn ta_bort_form_stadar_animeringar_och_duplicera_ger_nya_id() {
    let mut ed = Ed::new(fixture(true));
    let r = ed.shape(0, "Ruta");
    ed.apply(|t| edit_ops::duplicate_shape(t, 0, r.index, r.id).map(|_| ()));
    let copies: Vec<_> = edit_ops::shapes(&ed.deck, 0).into_iter().filter(|s| s.name == "Ruta").collect();
    assert_eq!(copies.len(), 2);
    assert_ne!(copies[0].id, copies[1].id);
    assert_eq!(copies[1].xf.x, copies[0].xf.x + 228_600);
    let out = ed.save();
    assert_eq!(s(part(&entries(&out), "ppt/slides/slide1.xml")).matches("a16:creationId").count(), 1, "kopian har inget creationId");
    // Ta bort originalet: animeringen och bldP för id 4 försvinner, kopian finns kvar.
    let r = ed.shape(0, "Ruta");
    ed.apply(|t| edit_ops::delete_shape(t, 0, r.index, r.id));
    let out = ed.save();
    let x = s(part(&entries(&out), "ppt/slides/slide1.xml"));
    assert!(!x.contains("spid=\"4\""), "{x}");
    assert_eq!(edit_ops::shapes(&ed.deck, 0).iter().filter(|s| s.name == "Ruta").count(), 1);
    // Ordning: bilden längst bak hamnar först bland formerna.
    let pic = ed.shape(0, "Bild");
    ed.apply(|t| edit_ops::reorder_shape(t, 0, pic.index, pic.id, Order::Back));
    assert_eq!(edit_ops::shapes(&ed.deck, 0)[0].name, "Bild");
    let pic = ed.shape(0, "Bild");
    ed.apply(|t| edit_ops::reorder_shape(t, 0, pic.index, pic.id, Order::Forward));
    assert_eq!(edit_ops::shapes(&ed.deck, 0)[1].name, "Bild");
    ed.save();
}

#[test]
fn form_i_alternate_content_andras_i_alla_grenar() {
    let mut ed = Ed::new(fixture(true));
    let a = ed.shape(1, "Alt");
    assert!(a.alternate);
    ed.apply(|t| edit_ops::set_xf(t, 1, a.index, a.id, Xf { x: 42, ..a.xf }));
    let out = ed.save();
    let x = s(part(&entries(&out), "ppt/slides/slide2.xml"));
    assert_eq!(x.matches(r#"<a:off x="42" y="5000000"/>"#).count(), 2, "{x}");
    assert!(x.contains("<mc:AlternateContent"));
    let a = ed.shape(1, "Alt");
    ed.apply(|t| edit_ops::duplicate_shape(t, 1, a.index, a.id).map(|_| ()));
    let out = ed.save();
    let x = s(part(&entries(&out), "ppt/slides/slide2.xml"));
    assert_eq!(x.matches("<mc:AlternateContent").count(), 2);
}

#[test]
fn ny_bild_med_layout_far_tomma_platshallare() {
    let mut ed = Ed::new(fixture(true));
    ed.apply(|t| edit_ops::new_slide(t, Some(0), 0).map(|_| ()));
    assert_eq!(ed.deck.slides.len(), 4);
    let new = &ed.deck.slides[1];
    assert_eq!(new.part, "ppt/slides/slide4.xml");
    let sh = edit_ops::shapes(&ed.deck, 1);
    let phs: Vec<_> = sh.iter().filter_map(|s| s.ph.clone()).collect();
    assert_eq!(phs, vec!["title".to_owned(), "obj".to_owned()], "sidnummer följer inte med");
    assert!(sh.iter().all(|s| s.empty_ph));
    // Platshållarna ärver läget från layout/mall.
    assert_eq!(sh[0].xf.x, 457200);
    let out = ed.save();
    let e = entries(&out);
    let pres = s(part(&e, "ppt/presentation.xml"));
    assert!(pres.contains(r#"<p:sldId id="256" r:id="rIdS1"/><p:sldId id="259" r:id="rId1"/><p:sldId id="257""#), "{pres}");
    assert!(pres.contains(r#"<p14:sldId id="256"/><p14:sldId id="259"/><p14:sldId id="257"/>"#), "avsnittet: {pres}");
    assert!(s(part(&e, "docProps/app.xml")).contains("<Slides>4</Slides>"));
    assert!(s(part(&e, "[Content_Types].xml")).contains("/ppt/slides/slide4.xml"));
    // Tom layout.
    ed.apply(|t| edit_ops::new_slide(t, None, 1).map(|_| ()));
    assert_eq!(ed.deck.slides.len(), 5);
    assert!(edit_ops::shapes(&ed.deck, 0).is_empty());
    ed.save();
}

#[test]
fn ta_bort_bild_tar_bort_relationer_lankar_och_anteckningar() {
    let mut ed = Ed::new(fixture(true));
    // Bild 2 länkas från bild 3.
    ed.apply(|t| edit_ops::delete_slide(t, 1));
    assert_eq!(ed.deck.slides.len(), 2);
    let out = ed.save();
    let e = entries(&out);
    assert!(!e.iter().any(|(n, _)| n == "ppt/slides/slide2.xml" || n == "ppt/slides/_rels/slide2.xml.rels"));
    let s3 = s(part(&e, "ppt/slides/slide3.xml"));
    assert!(!s3.contains("hlinkClick") && s3.contains("Till bild 2"), "{s3}");
    assert!(!s(part(&e, "ppt/slides/_rels/slide3.xml.rels")).contains("slide2.xml"));
    let pres = s(part(&e, "ppt/presentation.xml"));
    assert!(!pres.contains("id=\"257\""), "{pres}");
    assert!(s(part(&e, "docProps/app.xml")).contains("<Slides>2</Slides>"));
    // Bild 1 med anteckningar och bild: anteckningarna tas bort; bilden (media) används inte längre.
    ed.apply(|t| edit_ops::delete_slide(t, 0));
    let out = ed.save();
    let e = entries(&out);
    for gone in ["ppt/notesSlides/notesSlide1.xml", "ppt/media/image1.png", "ppt/slides/slide1.xml"] {
        assert!(!e.iter().any(|(n, _)| n == gone), "{gone} borta");
    }
    assert!(e.iter().any(|(n, _)| n == "ppt/notesMasters/notesMaster1.xml"), "anteckningsmallen blir kvar");
    assert!(e.iter().any(|(n, _)| n == "customXml/item1.xml") && e.iter().any(|(n, _)| n == "ppt/okand.bin"), "okända delar följer med");
}

#[test]
fn flytta_och_duplicera_bild() {
    let mut ed = Ed::new(fixture(true));
    ed.apply(|t| edit_ops::move_slide(t, 0, 2));
    assert_eq!(ed.deck.slides.iter().map(|s| s.part.as_str()).collect::<Vec<_>>(), ["ppt/slides/slide2.xml", "ppt/slides/slide3.xml", "ppt/slides/slide1.xml"]);
    let out = ed.save();
    let pres = s(part(&entries(&out), "ppt/presentation.xml"));
    // Bild 1 (id 256) flyttar till avsnitt B efter bild 3.
    assert!(pres.contains(r#"<p14:sldIdLst><p14:sldId id="257"/></p14:sldIdLst>"#), "{pres}");
    assert!(pres.contains(r#"<p14:sldIdLst><p14:sldId id="258"/><p14:sldId id="256"/></p14:sldIdLst>"#), "{pres}");
    ed.apply(|t| edit_ops::move_slide(t, 2, 0));
    assert_eq!(ed.deck.slides[0].part, "ppt/slides/slide1.xml");
    // Duplicera bild 1: egen anteckningssida, delad bild (media), samma layout.
    ed.apply(|t| edit_ops::duplicate_slide(t, 0).map(|_| ()));
    assert_eq!(ed.deck.slides.len(), 4);
    let copy = ed.deck.slides[1].part.clone();
    assert_eq!(copy, "ppt/slides/slide4.xml");
    let out = ed.save();
    let e = entries(&out);
    let rels = s(part(&e, "ppt/slides/_rels/slide4.xml.rels"));
    assert!(rels.contains("../media/image1.png") && rels.contains("../notesSlides/notesSlide2.xml"), "{rels}");
    let nrels = s(part(&e, "ppt/notesSlides/_rels/notesSlide2.xml.rels"));
    assert!(nrels.contains("../slides/slide4.xml"), "{nrels}");
    assert_eq!(super::notes::notes_text(&ed.deck, 1), "Gamla anteckningar");
    assert_eq!(ed.text(1), ed.text(0));
    // Dölj kopian.
    ed.apply(|t| edit_ops::set_hidden(t, 1, true));
    assert!(ed.deck.slides[1].hidden);
    ed.save();
}

#[test]
fn anteckningar_andras_och_skapas_med_och_utan_mall() {
    let mut ed = Ed::new(fixture(true));
    ed.apply(|t| edit_ops::set_notes(t, 0, "Ny rad\nAndra raden"));
    assert_eq!(super::notes::notes_text(&ed.deck, 0), "Ny rad\nAndra raden");
    let out = ed.save();
    // Första körningens formatering behålls.
    assert!(s(part(&entries(&out), "ppt/notesSlides/notesSlide1.xml")).contains(r#"<a:rPr lang="sv-SE" i="1"/><a:t>Ny rad</a:t>"#));
    // Bild 2 saknar anteckningar: sidan skapas enligt mallen.
    ed.apply(|t| edit_ops::set_notes(t, 1, "Hej"));
    assert_eq!(super::notes::notes_text(&ed.deck, 1), "Hej");
    ed.save();
    // Utan anteckningsmall: mallen (med en kopia av temat) skapas.
    let mut ed = Ed::new(fixture(false));
    ed.apply(|t| edit_ops::set_notes(t, 2, "Första anteckningen"));
    assert_eq!(super::notes::notes_text(&ed.deck, 2), "Första anteckningen");
    let out = ed.save();
    let e = entries(&out);
    let pres = s(part(&e, "ppt/presentation.xml"));
    assert!(pres.contains("</p:sldMasterIdLst><p:notesMasterIdLst><p:notesMasterId r:id="), "{pres}");
    assert!(e.iter().any(|(n, _)| n == "ppt/theme/theme2.xml"));
}

#[test]
fn infoga_textruta_och_bild() {
    let mut ed = Ed::new(fixture(true));
    let mut idx = 0;
    ed.apply(|t| {
        idx = edit_ops::insert_textbox(t, 2, Xf { x: 0, y: 3_000_000, cx: 3_000_000, cy: 400_000, ..Default::default() })?;
        Ok(())
    });
    let sh = edit_ops::shapes(&ed.deck, 2);
    assert_eq!(sh.last().unwrap().index, idx);
    ed.apply(|t| edit_ops::insert_picture(t, 2, png(400, 200), "Diagram").map(|_| ()));
    let pic = edit_ops::shapes(&ed.deck, 2).into_iter().last().unwrap();
    assert_eq!(pic.kind, edit_ops::Kind::Picture);
    assert!((pic.xf.cx as f64 / pic.xf.cy as f64 - 2.0).abs() < 0.01, "behåller proportionerna");
    let out = ed.save();
    let e = entries(&out);
    assert!(e.iter().any(|(n, _)| n == "ppt/media/image2.png"));
    // Ritas.
    let d = Deck::open(&out).unwrap();
    assert!(super::render::slide_scene(&d, 2).items.iter().any(|i| matches!(i, super::scene::Item::Image { .. })));
}

#[test]
fn ingen_andring_ger_inget_steg_och_signaturer_tas_bort() {
    let mut ed = Ed::new(fixture(true));
    let r = ed.shape(0, "Ruta");
    assert!(ed.s.apply(&ed.deck, None, |t| edit_ops::set_xf(t, 0, r.index, r.id, r.xf)).unwrap().is_none());
    assert!(!ed.s.can_undo());
    // En signerad fil (ursprungsrelation och signaturdel).
    let mut parts = entries(&fixture(true));
    for (n, b) in parts.iter_mut() {
        if n == "_rels/.rels" {
            *b = s(b).replace("</Relationships>", &format!(r#"<Relationship Id="rIdSig" Type="{PKG}/digital-signature/origin" Target="_xmlsignatures/origin.sigs"/></Relationships>"#)).into_bytes();
        }
        if n == "[Content_Types].xml" {
            *b = s(b).replace("</Types>", r#"<Default Extension="sigs" ContentType="application/vnd.openxmlformats-package.digital-signature-origin"/><Override PartName="/_xmlsignatures/sig1.xml" ContentType="application/vnd.openxmlformats-package.digital-signature-xmlsignature+xml"/></Types>"#).into_bytes();
        }
    }
    parts.push(("_xmlsignatures/origin.sigs".into(), Vec::new()));
    parts.push(("_xmlsignatures/_rels/origin.sigs.rels".into(), format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{PKG}/digital-signature/signature" Target="sig1.xml"/></Relationships>"#).into_bytes()));
    parts.push(("_xmlsignatures/sig1.xml".into(), b"<Signature xmlns=\"http://www.w3.org/2000/09/xmldsig#\"/>".to_vec()));
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (n, b) in &parts {
        z.start_file(n.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b).unwrap();
    }
    let signed = z.finish().unwrap().into_inner();
    assert_valid(&signed);
    let mut ed = Ed::new(signed.clone());
    // Utan ändring följer signaturen med.
    assert_eq!(entries(&ed.save()), entries(&signed));
    let a = ed.s.apply(&ed.deck, None, |t| edit_ops::set_hidden(t, 2, true)).unwrap().unwrap();
    assert!(a.signature_removed);
    ed.deck = a.deck;
    let out = ed.save();
    assert!(!entries(&out).iter().any(|(n, _)| n.starts_with("_xmlsignatures")));
}

#[test]
fn format_byts_bara_nar_filandelsen_byts() {
    let mut ed = Ed::new(fixture(true));
    let (same, _) = ed.s.save_bytes(&ed.deck, "pptx", "pptx").unwrap();
    assert_eq!(entries(&same), entries(&fixture(true)));
    let (show, rebuilt) = ed.s.save_bytes(&ed.deck, "ppsx", "pptx").unwrap();
    assert!(rebuilt.is_some());
    assert_valid(&show);
    assert!(s(part(&entries(&show), "[Content_Types].xml")).contains("presentationml.slideshow.main+xml"));
    assert!(ed.s.save_bytes(&ed.deck, "docx", "pptx").is_err());
}

#[test]
fn skrivning_slas_ihop_till_ett_angrasteg() {
    let mut ed = Ed::new(fixture(true));
    let part0 = ed.deck.slides[0].part.clone();
    for (i, c) in "abc".chars().enumerate() {
        let p0 = part0.clone();
        let a = ed
            .s
            .apply(&ed.deck, Some("text".into()), move |t| {
                let doc = t.doc(&p0)?;
                let scope = doc.root_scope();
                let sp = doc.root.path_mut(&["p:cSld", "p:spTree"]).unwrap().elems_mut().find(|e| e.name == "p:sp").unwrap();
                let mut m = TextModel::from_body(sp.child("p:txBody"), &scope, "a");
                m.insert(Pos { para: 0, ch: 7 + i }, &c.to_string());
                m.write_into(sp, "p");
                Ok(())
            })
            .unwrap()
            .unwrap();
        ed.deck = a.deck;
    }
    assert!(ed.text(0).contains("Kvartalabc"));
    ed.deck = ed.s.undo(&ed.deck).unwrap().unwrap();
    assert!(ed.text(0).contains("Kvartal ") || ed.text(0).starts_with("Kvartal"));
    assert!(!ed.text(0).contains("Kvartala"));
    assert!(!ed.s.can_undo() && !ed.s.dirty());
}

#[test]
fn markorlagen_foljer_texten() {
    let ed = Ed::new(fixture(true));
    let part0 = ed.deck.slides[0].part.clone();
    let doc = edit_xml::parse(ed.deck.pkg.read(&part0).unwrap()).unwrap();
    let sp = doc.root.path(&["p:cSld", "p:spTree"]).unwrap().elems().find(|e| e.name == "p:sp").unwrap();
    let m = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), "a");
    let node = edit_ops::node(&ed.deck, 0, ed.shape(0, "F2").index).unwrap();
    let c = m.carets(&ed.deck, 0, node).unwrap();
    let (a0, _) = c.caret(Pos { para: 0, ch: 0 }).unwrap();
    let (a7, b7) = c.caret(Pos { para: 0, ch: 7 }).unwrap();
    assert!(a7.0 > a0.0 + 50.0, "markören flyttas åt höger med texten");
    // Glyferna i den ritade bilden börjar där markören står.
    let scene = super::render::slide_scene(&ed.deck, 0);
    let run = scene.items.iter().find_map(|i| if let super::scene::Item::Glyphs(g) = i { (g.text == "Kvartal").then_some(g) } else { None }).unwrap();
    let (gx, gy) = run.t.apply(run.glyphs[0].x, 0.0);
    assert!((gx - a0.0).abs() < 1.0 && gy > a0.1 && gy < b7.1, "markör {a0:?}, glyf ({gx}, {gy})");
    assert_eq!(c.hit(a7.0 + 0.5, (a7.1 + b7.1) / 2.0), Some(Pos { para: 0, ch: 7 }));
}

/// Kontrollerar en sparad fil: `KLADD_PPTX_VALIDATE=fil.pptx cargo test pptx::edit_tests::validera_fil -- --ignored --nocapture`.
#[test]
#[ignore]
fn validera_fil() {
    let Some(f) = std::env::var_os("KLADD_PPTX_VALIDATE") else { return };
    let e = validate(&std::fs::read(&f).unwrap());
    println!("{} fel", e.len());
    assert!(e.is_empty(), "{e:#?}");
}

/// Redigerar varje .pptx under `KLADD_PPTX_SWEEP` (rekursivt): oförändrad sparning ger samma delar;
/// sedan flyttas första formen, en bild läggs till, anteckningar skrivs, en bild dupliceras och den
/// sista tas bort. Varje sparad fil kontrolleras med `validate`, öppnas igen och renderas. Med
/// `KLADD_PPTX_SWEEP_OUT=mapp` skrivs de redigerade filerna dit (för LibreOffice/python-pptx).
#[test]
#[ignore]
fn redigera_alla() {
    let Some(root) = std::env::var_os("KLADD_PPTX_SWEEP") else { return };
    let out_dir = std::env::var_os("KLADD_PPTX_SWEEP_OUT").map(std::path::PathBuf::from);
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pptx")) {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(&root), &mut files);
    files.sort();
    let mut problems = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        let Ok(deck) = Deck::open(&bytes) else { continue };
        let pre = validate(&bytes);
        let name = f.display().to_string();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Vec<String>, String> {
            let mut s = Session::new(bytes.clone(), &deck);
            let (same, _) = s.save_bytes(&deck, "pptx", "pptx")?;
            let (a, b) = (entries(&same), entries(&bytes));
            if a != b {
                let diff = a.iter().zip(&b).find(|(x, y)| x != y).map(|(x, y)| format!("{} ({} byte) / {} ({} byte)", x.0, x.1.len(), y.0, y.1.len()));
                return Err(format!("oförändrad sparning skiljer sig: {} / {} poster, {diff:?}", a.len(), b.len()));
            }
            let mut deck = deck;
            let step = |s: &mut Session, deck: &mut Deck, what: &str, f: &dyn Fn(&mut edit::Txn) -> Result<(), String>| -> Result<(), String> {
                match s.apply(deck, None, f) {
                    Ok(Some(a)) => {
                        *deck = a.deck;
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err(e) => Err(format!("{what}: {e}")),
                }
            };
            let n = deck.slides.len();
            if let Some(sh) = edit_ops::shapes(&deck, 0).into_iter().next() {
                let xf = Xf { x: sh.xf.x + 12_700, ..sh.xf };
                step(&mut s, &mut deck, "flytta", &|t| edit_ops::set_xf(t, 0, sh.index, sh.id, xf))?;
            }
            // Skriv i första formen med text: tecken, backsteg, Enter, fetstil.
            if let Some(sh) = edit_ops::shapes(&deck, 0).into_iter().find(|s| s.kind == edit_ops::Kind::Shape && !s.alternate) {
                let part0 = deck.slides[0].part.clone();
                let doc = edit_xml::parse(deck.pkg.read(&part0).unwrap_or_default()).ok_or("bild 1 kan inte tolkas")?;
                if let Some(sp) = edit_ops::raw_shape(&doc, sh.index) {
                    let a = doc.prefix_for(edit::NS_A).unwrap_or_else(|| "a".into());
                    let mut m = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), &a);
                    let end = m.end();
                    let p = m.insert(end, "Kladd ");
                    let p = m.delete(m.step(p, false), p);
                    let p = m.split_para(p);
                    m.insert(p, "ny rad");
                    m.toggle(Pos { para: 0, ch: 0 }, m.end(), "b");
                    step(&mut s, &mut deck, "text", &|t| edit_ops::set_text(t, 0, sh.index, sh.id, &m))?;
                }
            }
            // Textformat: större text, färg, justering och punkt i första formen med text, och
            // autoanpassningen räknas om som när man skriver (samma transaktion).
            if let Some(sh) = edit_ops::shapes(&deck, 0).into_iter().find(|s| s.kind == edit_ops::Kind::Shape && !s.alternate) {
                let part0 = deck.slides[0].part.clone();
                let doc = edit_xml::parse(deck.pkg.read(&part0).unwrap_or_default()).ok_or("bild 1 kan inte tolkas")?;
                if let Some(sp) = edit_ops::raw_shape(&doc, sh.index) {
                    let a = doc.prefix_for(edit::NS_A).unwrap_or_else(|| "a".into());
                    let mut m = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), &a);
                    let node = edit_ops::node(&deck, 0, sh.index);
                    let c = node.and_then(|n| m.carets(&deck, 0, n));
                    let fmts = c.as_ref().map(|c| c.paras.clone()).unwrap_or_default();
                    let before = c.as_ref().map(|c| c.text_h);
                    let end = m.end();
                    m.step_size(Pos::default(), end, true, &fmts);
                    m.set_color(Pos::default(), end, Some(super::edit_format::TextColor::Scheme("accent1")));
                    m.set_align(Pos::default(), end, "ctr");
                    m.set_bullet(Pos::default(), end, true);
                    let fit = node.and_then(|n| m.autofit(&deck, 0, n, sh.xf, before));
                    step(&mut s, &mut deck, "textformat", &|t| super::edit_fit::set_text_fit(t, 0, sh.index, sh.id, &m, fit, sh.xf))?;
                }
            }
            // Text i första tabellcellen (första tabellen på bild 1) och flera former flyttas i ett steg.
            if let Some(sh) = edit_ops::shapes(&deck, 0).into_iter().find(|s| super::edit_table::is_editable_table(&deck, 0, s.index)) {
                let part0 = deck.slides[0].part.clone();
                let doc = edit_xml::parse(deck.pkg.read(&part0).unwrap_or_default()).ok_or("bild 1 kan inte tolkas")?;
                let a = doc.prefix_for(edit::NS_A).unwrap_or_else(|| "a".into());
                if let Some(mut m) = super::edit_table::cell_model(&doc, sh.index, (0, 0), &a) {
                    let end = m.end();
                    m.insert(end, " ändrad");
                    step(&mut s, &mut deck, "tabellcell", &|t| super::edit_table::set_cell_text(t, 0, sh.index, sh.id, (0, 0), &m))?;
                }
            }
            let many: Vec<_> = edit_ops::shapes(&deck, 0).into_iter().take(3).collect();
            if many.len() > 1 {
                step(&mut s, &mut deck, "flytta flera", &|t| {
                    for sh in &many {
                        edit_ops::set_xf(t, 0, sh.index, sh.id, Xf { y: sh.xf.y + 12_700, ..sh.xf })?;
                    }
                    Ok(())
                })?;
            }
            let layout = deck.slides.first().and_then(|s| s.layout).unwrap_or(0);
            if !deck.layouts.is_empty() {
                let after = (n > 0).then_some(0);
                step(&mut s, &mut deck, "ny bild", &|t| edit_ops::new_slide(t, after, layout).map(|_| ()))?;
            }
            if n > 0 {
                step(&mut s, &mut deck, "anteckningar", &|t| edit_ops::set_notes(t, 0, "Prov av anteckningar"))?;
                step(&mut s, &mut deck, "duplicera", &|t| edit_ops::duplicate_slide(t, 0).map(|_| ()))?;
                let last = deck.slides.len() - 1;
                step(&mut s, &mut deck, "ta bort", &|t| edit_ops::delete_slide(t, last))?;
            }
            let (out, _) = s.save_bytes(&deck, "pptx", "pptx")?;
            let errs: Vec<String> = validate(&out).into_iter().filter(|e| !pre.contains(e)).collect();
            let d = Deck::open(&out)?;
            for i in 0..d.slides.len() {
                let _ = super::render_slide(&d, i, 160, 90);
            }
            if let Some(dir) = &out_dir {
                let _ = std::fs::create_dir_all(dir);
                // Namnet efter mappen och filen (presentationerna heter ofta `deck.pptx`).
                let parent = f.parent().and_then(|p| p.file_name()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = std::fs::write(dir.join(format!("{parent}--{}", f.file_name().unwrap().to_string_lossy())), &out);
            }
            Ok(errs)
        }));
        match r {
            Ok(Ok(errs)) if errs.is_empty() => println!("ok   {name}"),
            Ok(Ok(errs)) => problems.push(format!("{name}: {errs:?}")),
            Ok(Err(e)) => problems.push(format!("{name}: {e}")),
            Err(_) => problems.push(format!("{name}: panik")),
        }
    }
    println!("{} filer, {} problem", files.len(), problems.len());
    for p in &problems {
        println!("FEL  {p}");
    }
    assert!(problems.is_empty());
}

// ------------------------------------------------------------------ textformat och autoanpassning

/// Bild 1:s brödtextplatshållare (F3) med `a:normAutofit` på formen och `n` rader text.
fn autofit_fixture(n: usize) -> (Ed, TextModel, edit_ops::ShapeInfo) {
    let mut ed = Ed::new(fixture(true));
    let part0 = ed.deck.slides[0].part.clone();
    let sh = ed.shape(0, "F3");
    ed.apply(|t| {
        let doc = t.doc(&part0)?;
        let frag = doc.fragment("<a:normAutofit/>").into_iter().next().unwrap();
        let sp = doc.root.path_mut(&["p:cSld", "p:spTree"]).unwrap().elems_mut().filter(|e| e.name == "p:sp").nth(1).unwrap();
        sp.path_mut(&["p:txBody", "a:bodyPr"]).unwrap().children.push(edit_xml::XNode::Elem(frag));
        Ok(())
    });
    let doc = edit_xml::parse(ed.deck.pkg.read(&part0).unwrap()).unwrap();
    let sp = edit_ops::raw_shape(&doc, sh.index).unwrap();
    let mut m = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), "a");
    let rows: String = (0..n).map(|i| format!("Rad {i}\n")).collect();
    let end = m.end();
    m.insert(end, &format!("\n{}", rows.trim_end()));
    (ed, m, sh)
}

/// Textens höjd och rutans höjd (punkter) med autoanpassningens värden.
fn heights(ed: &Ed, m: &TextModel, sh: &edit_ops::ShapeInfo, fit: (f32, f32)) -> (f32, f32) {
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    let c = m.carets_fit(&ed.deck, 0, node, Some(fit)).unwrap();
    (c.text_h, c.avail_h)
}

#[test]
fn autoanpassning_krymper_till_rutan_och_vaxer_tillbaka() {
    use super::edit_fit::{set_text_fit, Fit, LADDER};
    let (mut ed, mut m, sh) = autofit_fixture(30);
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    let fit = m.autofit(&ed.deck, 0, node, sh.xf, None).expect("texten ryms inte: ska krympas");
    let Fit::Norm { scale, red } = fit else { panic!("{fit:?}") };
    assert!(scale < 1.0 && (red - 0.2).abs() < 1e-4, "{fit:?}");
    // Texten ryms med steget och ryms inte med steget före (om det finns).
    let (h, avail) = heights(&ed, &m, &sh, (scale, red));
    assert!(h <= avail + 0.5, "ryms: {h} <= {avail}");
    let at = LADDER.iter().position(|&(s, r)| (s - scale).abs() < 1e-4 && (r - red).abs() < 1e-4).unwrap();
    if at > 0 {
        let (h, avail) = heights(&ed, &m, &sh, LADDER[at - 1]);
        assert!(h > avail + 0.5, "föregående steg ryms inte: {h} > {avail}");
    }
    // Skrivs i en transaktion; värdena hamnar på formens egen `a:normAutofit` och filen är giltig.
    ed.apply(|t| set_text_fit(t, 0, sh.index, sh.id, &m, Some(fit), sh.xf));
    let out = ed.save();
    let x = s(part(&entries(&out), "ppt/slides/slide1.xml"));
    let pct = |v: f32| (v * 100_000.0).round() as i64;
    assert!(x.contains(&format!(r#"<a:normAutofit fontScale="{}" lnSpcReduction="20000"/>"#, pct(scale))), "{x}");
    // Stegen är monotona: mer text ger aldrig ett mildare steg.
    let mut m2 = m.clone();
    let end = m2.end();
    m2.insert(end, "\nEn rad till\nOch en till");
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    if let Some(Fit::Norm { scale: s2, .. }) = m2.autofit(&ed.deck, 0, node, sh.xf, None) {
        assert!(s2 <= scale + 1e-4, "{s2} <= {scale}");
    }
    // Kortas texten ned ryms den i full storlek: värdena nollställs (`<a:normAutofit/>`).
    m.delete(Pos { para: 1, ch: 0 }, m.end());
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    let back = m.autofit(&ed.deck, 0, node, sh.xf, None);
    assert_eq!(back, Some(Fit::Norm { scale: 1.0, red: 0.0 }));
    ed.apply(|t| set_text_fit(t, 0, sh.index, sh.id, &m, back, sh.xf));
    let out = ed.save();
    let x = s(part(&entries(&out), "ppt/slides/slide1.xml"));
    assert!(x.contains("<a:normAutofit/>") && !x.contains("fontScale"), "{x}");
    // Oförändrad text ger inget nytt värde.
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    assert_eq!(m.autofit(&ed.deck, 0, node, sh.xf, None), None);
}

#[test]
fn autoanpassning_behaller_ett_varde_som_ryms() {
    use super::edit_fit::Fit;
    // 90 % ur en gammal PowerPoint-fil, och texten ryms: ingenting ändras när man skriver.
    let (mut ed, m, sh) = autofit_fixture(1);
    let part0 = ed.deck.slides[0].part.clone();
    ed.apply(|t| {
        let doc = t.doc(&part0)?;
        let sp = doc.root.path_mut(&["p:cSld", "p:spTree"]).unwrap().elems_mut().filter(|e| e.name == "p:sp").nth(1).unwrap();
        sp.path_mut(&["p:txBody", "a:bodyPr", "a:normAutofit"]).unwrap().set_attr("fontScale", "90000");
        Ok(())
    });
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    let c = m.carets(&ed.deck, 0, node).unwrap();
    assert!(matches!(c.autofit, super::edit_text::AutoFit::Norm { scale, .. } if (scale - 0.9).abs() < 1e-4));
    let r = m.autofit(&ed.deck, 0, node, sh.xf, None);
    // Mjukare steg som också ryms får användas (texten är kort), annars behålls värdet.
    assert!(matches!(r, None | Some(Fit::Norm { .. })), "{r:?}");
}

#[test]
fn spautofit_gor_formen_hogre_och_kortare_igen() {
    use super::edit_fit::{set_text_fit, Fit};
    let mut ed = Ed::new(fixture(true));
    let xf = Xf { x: 914_400, y: 914_400, cx: 3_000_000, cy: 400_000, rot: 0, flip_h: false, flip_v: false };
    ed.apply(|t| edit_ops::insert_textbox(t, 0, xf).map(|_| ()));
    let sh = edit_ops::shapes(&ed.deck, 0).into_iter().find(|s| s.name.starts_with("Textruta")).unwrap();
    let part0 = ed.deck.slides[0].part.clone();
    let doc = edit_xml::parse(ed.deck.pkg.read(&part0).unwrap()).unwrap();
    let sp = edit_ops::raw_shape(&doc, sh.index).unwrap();
    let mut m = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), "a");
    m.insert(Pos::default(), "Rad ett\nRad två\nRad tre\nRad fyra\nRad fem");
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    let before = m.carets(&ed.deck, 0, node).unwrap().text_h;
    let fit = m.autofit(&ed.deck, 0, node, sh.xf, Some(before)).expect("rutan ska växa");
    let Fit::Shape { y, cy } = fit else { panic!("{fit:?}") };
    assert!(cy > sh.xf.cy && y == sh.xf.y, "{fit:?}");
    ed.apply(|t| set_text_fit(t, 0, sh.index, sh.id, &m, Some(fit), sh.xf));
    let grown = ed.shape(0, &sh.name).xf;
    assert_eq!(grown.cy, cy);
    // Höjden är textens höjd plus insättningarna (45720 EMU över och under).
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    let c = m.carets(&ed.deck, 0, node).unwrap();
    let need = ((c.text_h + c.ins_tb.0 + c.ins_tb.1) as f64 * 12_700.0).round() as i64;
    assert!((grown.cy - need).abs() < 2, "{} / {need}", grown.cy);
    assert_valid(&ed.save());
    // Färre rader: formen krymper (men inte under det som behövs).
    let before = c.text_h;
    m.delete(Pos { para: 1, ch: 0 }, m.end());
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    let Some(Fit::Shape { cy: cy2, .. }) = m.autofit(&ed.deck, 0, node, grown, Some(before)) else { panic!("rutan ska krympa") };
    assert!(cy2 < grown.cy && cy2 >= 12_700, "{cy2} < {}", grown.cy);
    // Utan förändring av textens höjd rör sig formen inte.
    assert_eq!(m.autofit(&ed.deck, 0, node, Xf { cy: cy2, ..grown }, Some(m.carets(&ed.deck, 0, node).unwrap().text_h)).map(|_| ()), None);
}

#[test]
fn textformat_tur_och_retur_med_giltig_fil() {
    use super::edit_format::TextColor;
    let mut ed = Ed::new(fixture(true));
    let part0 = ed.deck.slides[0].part.clone();
    let sh = ed.shape(0, "F3");
    let doc = edit_xml::parse(ed.deck.pkg.read(&part0).unwrap()).unwrap();
    let sp = edit_ops::raw_shape(&doc, sh.index).unwrap();
    let mut m = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), "a");
    let end = m.end();
    // Storlek ur den ärvda (32 pt på nivå 1, 28 pt på nivå 2).
    let node = edit_ops::node(&ed.deck, 0, sh.index).unwrap();
    let fmts = m.carets(&ed.deck, 0, node).unwrap().paras;
    assert!((fmts[0].runs[0].2 - 32.0).abs() < 0.01 && (fmts[1].runs[0].2 - 28.0).abs() < 0.01, "{fmts:?}");
    assert!(fmts[0].bullet && fmts[1].bullet);
    m.step_size(Pos::default(), end, true, &fmts);
    m.set_color(Pos::default(), end, Some(TextColor::Scheme("accent2")));
    m.set_align(Pos::default(), end, "ctr");
    m.set_bullet(Pos { para: 1, ch: 0 }, Pos { para: 1, ch: 0 }, false);
    ed.apply(|t| edit_ops::set_text(t, 0, sh.index, sh.id, &m));
    let out = ed.save();
    let x = s(part(&entries(&out), "ppt/slides/slide1.xml"));
    assert!(x.contains(r#"sz="3600""#) && x.contains(r#"sz="3200""#) && x.contains(r#"<a:schemeClr val="accent2"/>"#) && x.contains(r#"algn="ctr""#) && x.contains("<a:buNone/>"), "{x}");
    // Ritas igen: första stycket utan punkt får nya storleken, andra stycket saknar punkt.
    let d = Deck::open(&out).unwrap();
    let node = edit_ops::node(&d, 0, sh.index).unwrap();
    let c = m.carets(&d, 0, node).unwrap();
    assert!((c.paras[0].runs[0].2 - 36.0).abs() < 0.01 && (c.paras[1].runs[0].2 - 32.0).abs() < 0.01, "{:?}", c.paras);
    assert_eq!(c.paras[0].align, super::text::Align::Center);
    assert!(c.paras[0].bullet && !c.paras[1].bullet);
    assert_eq!(c.paras[0].runs[0].3, [0xC0, 0x50, 0x4D]);
}

/// Räknar om autoanpassningen från grunden för varje form med `a:normAutofit` (med värden) i
/// materialet under `KLADD_PPTX_SWEEP` och jämför med de lagrade värdena från PowerPoint:
/// `KLADD_PPTX_SWEEP=mapp cargo test pptx::edit_tests::autoanpassning_mot_materialet -- --ignored --nocapture`.
#[test]
#[ignore]
fn autoanpassning_mot_materialet() {
    use super::edit_fit::LADDER;
    let Some(root) = std::env::var_os("KLADD_PPTX_SWEEP") else { return };
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pptx")) {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(&root), &mut files);
    files.sort();
    // (form, samma steg, lagrat steg ryms hos oss, vårt steg är mildare, vårt steg är hårdare)
    let (mut n, mut same, mut stored_fits, mut milder, mut harder) = (0, 0, 0, 0, 0);
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else { continue };
        let Ok(deck) = Deck::open(&bytes) else { continue };
        for slide in 0..deck.slides.len() {
            let Some(raw) = deck.pkg.read(&deck.slides[slide].part) else { continue };
            let Some(doc) = edit_xml::parse(raw) else { continue };
            for sh in edit_ops::shapes(&deck, slide) {
                if sh.kind != edit_ops::Kind::Shape || sh.alternate {
                    continue;
                }
                let Some(sp) = edit_ops::raw_shape(&doc, sh.index) else { continue };
                let Some(fit) = sp.path(&["p:txBody", "a:bodyPr", "a:normAutofit"]) else { continue };
                let stored = (fit.attr_i64("fontScale").map_or(1.0, |v| v as f32 / 100_000.0), fit.attr_i64("lnSpcReduction").map_or(0.0, |v| v as f32 / 100_000.0));
                if stored == (1.0, 0.0) {
                    continue;
                }
                let a = doc.prefix_for(edit::NS_A).unwrap_or_else(|| "a".into());
                let m = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), &a);
                let Some(node) = edit_ops::node(&deck, slide, sh.index) else { continue };
                let Some(base) = m.carets_fit(&deck, slide, node, None) else { continue };
                if !base.fit_ok {
                    continue;
                }
                n += 1;
                let fits = |s: (f32, f32)| m.carets_fit(&deck, slide, node, Some(s)).is_some_and(|c| c.text_h <= c.avail_h + 0.5);
                let ours = LADDER.iter().copied().find(|&s| fits(s)).unwrap_or(LADDER[LADDER.len() - 1]);
                let near = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 0.02 && (a.1 - b.1).abs() < 0.02;
                if near(ours, stored) {
                    same += 1;
                } else if ours.0 > stored.0 + 0.02 || (near((ours.0, 0.0), (stored.0, 0.0)) && ours.1 < stored.1) {
                    milder += 1;
                } else {
                    harder += 1;
                }
                if fits(stored) {
                    stored_fits += 1;
                }
            }
        }
    }
    println!("{n} former med lagrad autoanpassning: samma steg {same}, vårt mildare {milder}, vårt hårdare {harder}; lagrat steg ryms hos oss i {stored_fits}");
}

// ------------------------------------------------------------------ tabellceller

/// Bild 1 med en tabell (2 rader × 2 kolumner, cell (0,0) med text) längst fram.
fn table_fixture() -> (Ed, edit_ops::ShapeInfo) {
    let mut ed = Ed::new(fixture(true));
    let part0 = ed.deck.slides[0].part.clone();
    ed.apply(|t| {
        let doc = t.doc(&part0)?;
        let cell = |txt: &str| format!(r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p>{}</a:p></a:txBody><a:tcPr/></a:tc>"#, if txt.is_empty() { String::new() } else { format!(r#"<a:r><a:rPr lang="sv-SE"/><a:t>{txt}</a:t></a:r>"#) });
        let xml = format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="40" name="Tabell"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="1000000" y="1000000"/><a:ext cx="4000000" cy="800000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr/><a:tblGrid><a:gridCol w="2000000"/><a:gridCol w="2000000"/></a:tblGrid><a:tr h="400000">{}{}</a:tr><a:tr h="400000">{}{}</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
            cell("Hej"),
            cell(""),
            cell(""),
            cell("")
        );
        let el = doc.fragment(&xml).into_iter().next().unwrap();
        let tree = doc.root.path_mut(&["p:cSld", "p:spTree"]).unwrap();
        let at = tree.index_of("p:extLst").unwrap_or(tree.children.len());
        tree.children.insert(at, edit_xml::XNode::Elem(el));
        Ok(())
    });
    let sh = ed.shape(0, "Tabell");
    (ed, sh)
}

#[test]
fn text_i_tabellcell_tur_och_retur() {
    use super::edit_table::{cell_at, cell_model, next_cell, set_cell_text};
    let (mut ed, sh) = table_fixture();
    // Träff: punkterna i bildens punkter (EMU / 12700).
    let (x0, y0) = (1_000_000.0 / 12_700.0, 1_000_000.0 / 12_700.0);
    assert_eq!(cell_at(&ed.deck, 0, sh.index, x0 + 10.0, y0 + 10.0), Some((0, 0)));
    assert_eq!(cell_at(&ed.deck, 0, sh.index, x0 + 200.0, y0 + 10.0), Some((0, 1)));
    assert_eq!(cell_at(&ed.deck, 0, sh.index, x0 + 10.0, y0 + 40.0), Some((1, 0)));
    assert_eq!(cell_at(&ed.deck, 0, sh.index, x0 - 5.0, y0 + 10.0), None);
    assert_eq!(next_cell(&ed.deck, 0, sh.index, (0, 1), false), Some((1, 0)));
    assert_eq!(next_cell(&ed.deck, 0, sh.index, (1, 1), false), None);
    assert_eq!(next_cell(&ed.deck, 0, sh.index, (0, 0), true), None);
    // Cell (1,1): skriv text; markören står i cellen.
    let part0 = ed.deck.slides[0].part.clone();
    let doc = edit_xml::parse(ed.deck.pkg.read(&part0).unwrap()).unwrap();
    let mut m = cell_model(&doc, sh.index, (1, 1), "a").unwrap();
    m.insert(Pos::default(), "Cell text");
    let c = m.cell_carets(&ed.deck, 0, sh.index, (1, 1)).unwrap();
    let (a, b) = c.caret(m.end()).unwrap();
    assert!(a.0 > x0 + 157.0 && a.0 < x0 + 315.0 && a.1 > y0 + 31.5 && b.1 < y0 + 63.0, "markör {a:?} {b:?}");
    ed.apply(|t| set_cell_text(t, 0, sh.index, sh.id, (1, 1), &m));
    assert!(ed.text(0).contains("Cell text"));
    // Cell (0,0) orörd; filen giltig och oförändrad utanför tabellen.
    let out = ed.save();
    let x = s(part(&entries(&out), "ppt/slides/slide1.xml"));
    assert!(x.contains(r#"<a:t>Hej</a:t></a:r></a:p></a:txBody><a:tcPr/>"#) && x.contains("<a:t>Cell text</a:t>"), "{x}");
    // Texten ger raden mer höjd när den bryts: cellens markör följer den nya geometrin.
    let mut m2 = m.clone();
    let end = m2.end();
    m2.insert(end, " som är så lång att den måste brytas över flera rader i cellen");
    ed.apply(|t| set_cell_text(t, 0, sh.index, sh.id, (1, 1), &m2));
    let c2 = m2.cell_carets(&ed.deck, 0, sh.index, (1, 1)).unwrap();
    assert!(c2.text_h > c.text_h + 5.0, "{} > {}", c2.text_h, c.text_h);
    assert_valid(&ed.save());
}

// ------------------------------------------------------------------ flera former

#[test]
fn flera_former_flyttas_dupliceras_och_tas_bort_i_ett_steg() {
    let mut ed = Ed::new(fixture(true));
    let before = edit_ops::shapes(&ed.deck, 0);
    let pick: Vec<_> = ["Ruta", "Bild"].iter().map(|n| ed.shape(0, n)).collect();
    // Flytta båda i en transaktion.
    ed.apply(|t| {
        for s in &pick {
            edit_ops::set_xf(t, 0, s.index, s.id, Xf { x: s.xf.x + 254_000, y: s.xf.y + 127_000, ..s.xf })?;
        }
        Ok(())
    });
    for s in &pick {
        let now = ed.shape(0, &s.name);
        assert_eq!((now.xf.x, now.xf.y), (s.xf.x + 254_000, s.xf.y + 127_000));
    }
    // Duplicera bakifrån: båda får nya id, lägena före står kvar.
    let pick: Vec<_> = ["Ruta", "Bild"].iter().map(|n| ed.shape(0, n)).collect();
    ed.apply(|t| {
        for s in pick.iter().rev() {
            edit_ops::duplicate_shape(t, 0, s.index, s.id)?;
        }
        Ok(())
    });
    assert_eq!(edit_ops::shapes(&ed.deck, 0).len(), before.len() + 2);
    assert_valid(&ed.save());
    // Ta bort alla fyra (originalen och dubbletterna) bakifrån i ett steg.
    let all: Vec<_> = edit_ops::shapes(&ed.deck, 0).into_iter().filter(|s| s.name == "Ruta" || s.name == "Bild" || s.name.starts_with("Ruta ") || s.name.starts_with("Bild ")).collect();
    assert!(all.len() >= 4, "{:?}", all.iter().map(|s| &s.name).collect::<Vec<_>>());
    ed.apply(|t| {
        for s in all.iter().rev() {
            edit_ops::delete_shape(t, 0, s.index, s.id)?;
        }
        Ok(())
    });
    assert_eq!(edit_ops::shapes(&ed.deck, 0).len(), before.len() - 2);
    assert_valid(&ed.save());
}
