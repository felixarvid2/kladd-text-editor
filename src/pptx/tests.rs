//! Prov på egna, minimala .pptx-filer som byggs i provet (zip i minnet): arv, färger, geometri,
//! text, grupper, dolda bilder, `mc:AlternateContent`, trasiga filer och PDF.

use std::io::Write;

use super::color::Rgba;
use super::scene::{Item, Paint};
use super::{Deck, raster, render};

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Prov"><a:themeElements>
<a:clrScheme name="Prov"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4>
<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="Prov"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>
<a:fmtScheme name="Prov"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
<a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme>
</a:themeElements></a:theme>"#;

/// Bygger en .pptx med en mall, en layout och bilderna (`p:sld`-innehåll utan rotelement).
struct Builder {
    master_tree: String,
    master_extra: String,
    layout_tree: String,
    slides: Vec<(String, String)>,
    pres_attrs: String,
}

fn sp_ph(id: u32, name: &str, ph: &str, xfrm: Option<(i64, i64, i64, i64)>, body: &str) -> String {
    let x = xfrm.map_or(String::new(), |(x, y, w, h)| format!(r#"<a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#));
    format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr>{x}</p:spPr>{body}</p:sp>"#)
}

fn tree(inner: &str) -> String {
    format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#)
}

impl Builder {
    fn new() -> Builder {
        // Mallen: rubrik och brödtext med egna nivåstilar.
        let master_tree = sp_ph(2, "Rubrik", r#"<p:ph type="title"/>"#, Some((457200, 274638, 8229600, 1143000)), r#"<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r><a:t>Rubrik</a:t></a:r></a:p></p:txBody>"#)
            + &sp_ph(3, "Text", r#"<p:ph type="body" idx="1"/>"#, Some((457200, 1600200, 8229600, 4525963)), r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Text</a:t></a:r></a:p></p:txBody>"#)
            + &sp_ph(4, "Nummer", r#"<p:ph type="sldNum" sz="quarter" idx="12"/>"#, Some((6553200, 6356350, 2133600, 365125)), r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:fld id="{1}" type="slidenum"><a:t>‹#›</a:t></a:fld></a:p></p:txBody>"#);
        let master_extra = r#"<p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="4400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:lvl1pPr></p:titleStyle>
<p:bodyStyle><a:lvl1pPr marL="342900" indent="-342900"><a:buFont typeface="Arial"/><a:buChar char="•"/><a:defRPr sz="3200"><a:solidFill><a:schemeClr val="accent2"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr>
<a:lvl2pPr marL="742950" indent="-285750"><a:buChar char="–"/><a:defRPr sz="2800"><a:solidFill><a:schemeClr val="tx2"/></a:solidFill></a:defRPr></a:lvl2pPr></p:bodyStyle>
<p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles>"#
            .to_owned();
        // Layouten flyttar brödtexten och ger rubriken en egen storlek.
        let layout_tree = sp_ph(2, "Rubrik", r#"<p:ph type="title"/>"#, None, r#"<p:txBody><a:bodyPr/><a:lstStyle><a:lvl1pPr><a:defRPr sz="3600"/></a:lvl1pPr></a:lstStyle><a:p><a:endParaRPr/></a:p></p:txBody>"#)
            + &sp_ph(3, "Innehåll", r#"<p:ph idx="1"/>"#, Some((914400, 1828800, 7315200, 3657600)), r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr/></a:p></p:txBody>"#)
            + &sp_ph(4, "Nummer", r#"<p:ph type="sldNum" sz="quarter" idx="12"/>"#, None, r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:fld id="{1}" type="slidenum"><a:t>‹#›</a:t></a:fld></a:p></p:txBody>"#);
        Builder { master_tree, master_extra, layout_tree, slides: Vec::new(), pres_attrs: String::new() }
    }

    fn slide(mut self, attrs: &str, inner: &str) -> Builder {
        self.slides.push((attrs.to_owned(), inner.to_owned()));
        self
    }

    fn build(&self) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opt = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, data: &str| {
            z.start_file(name, opt).unwrap();
            z.write_all(data.as_bytes()).unwrap();
        };
        let mut ct = String::from(r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#);
        ct.push_str(r#"<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/></Types>"#);
        put("[Content_Types].xml", &ct);
        put("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#);
        let mut ids = String::new();
        let mut rels = String::from(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#);
        for (i, (attrs, inner)) in self.slides.iter().enumerate() {
            let n = i + 1;
            ids.push_str(&format!(r#"<p:sldId id="{}" r:id="rIdS{n}"/>"#, 255 + n));
            rels.push_str(&format!(r#"<Relationship Id="rIdS{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{n}.xml"/>"#));
            put(&format!("ppt/slides/slide{n}.xml"), &format!(r#"<p:sld {NS} {attrs}>{}</p:sld>"#, tree(inner)));
            put(
                &format!("ppt/slides/_rels/slide{n}.xml.rels"),
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#,
            );
        }
        rels.push_str("</Relationships>");
        put("ppt/_rels/presentation.xml.rels", &rels);
        put(
            "ppt/presentation.xml",
            &format!(r#"<p:presentation {NS} {}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/><p:defaultTextStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:defaultTextStyle></p:presentation>"#, self.pres_attrs),
        );
        put(
            "ppt/slideMasters/slideMaster1.xml",
            &format!(r#"<p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>{}</p:sldMaster>"#, self.master_tree, self.master_extra),
        );
        put(
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/></Relationships>"#,
        );
        put("ppt/slideLayouts/slideLayout1.xml", &format!(r#"<p:sldLayout {NS}>{}<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#, tree(&self.layout_tree)));
        put(
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#,
        );
        put("ppt/theme/theme1.xml", THEME);
        z.finish().unwrap().into_inner()
    }
}

fn text_body(paras: &str) -> String {
    format!(r#"<p:txBody><a:bodyPr/><a:lstStyle/>{paras}</p:txBody>"#)
}

/// Glyfkörningarna i bilden: (text, storlek, färg, x, y).
fn runs(deck: &Deck, i: usize) -> Vec<(String, f32, Rgba, f32, f32)> {
    render::slide_scene(deck, i)
        .items
        .into_iter()
        .filter_map(|it| match it {
            Item::Glyphs(g) => {
                let c = match g.paint {
                    Paint::Solid(c) => c,
                    _ => Rgba::BLACK,
                };
                let (x, y) = g.t.apply(g.glyphs.first().map_or(0.0, |p| p.x), 0.0);
                Some((g.text, g.size, c, x, y))
            }
            _ => None,
        })
        .collect()
}

fn hex(c: Rgba) -> String {
    let [r, g, b, _] = c.to_u8();
    format!("{r:02X}{g:02X}{b:02X}")
}

fn pixel(deck: &Deck, i: usize, x: f32, y: f32) -> [u8; 4] {
    let (w, h) = deck.size_pt();
    let (pw, ph, rgba) = raster::to_rgba(raster::render(&render::slide_scene(deck, i), w as u32, h as u32).unwrap());
    let (px, py) = ((x as u32).min(pw - 1), (y as u32).min(ph - 1));
    let o = ((py * pw + px) * 4) as usize;
    [rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]
}

#[test]
fn platshallare_arver_lage_storlek_och_farg() {
    let body = text_body(r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>Första</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:t>Andra</a:t></a:r></a:p>"#);
    let title = text_body(r#"<a:p><a:r><a:t>Rubriken</a:t></a:r></a:p>"#);
    let bytes = Builder::new().slide("", &(sp_ph(2, "R", r#"<p:ph type="title"/>"#, None, &title) + &sp_ph(3, "I", r#"<p:ph idx="1"/>"#, None, &body))).build();
    let deck = Deck::open(&bytes).unwrap();
    let r = runs(&deck, 0);
    let find = |t: &str| r.iter().find(|x| x.0 == t).cloned().unwrap_or_else(|| panic!("{t} saknas i {r:?}"));
    // Rubriken: layoutens lstStyle (36 pt) går före mallens titleStyle (44 pt); läget från mallen.
    let t = find("Rubriken");
    assert_eq!(t.1, 36.0);
    assert!((t.3 - (36.0 + 7.2)).abs() < 1.0, "rubrikens x {}", t.3);
    // Brödtexten: läget från layouten (x = 72 pt + insättning 7,2 + marL 27), storlek och färg från mallens bodyStyle.
    let f = find("Första");
    assert_eq!(f.1, 32.0);
    assert_eq!(hex(f.2), "ED7D31");
    assert!((f.3 - (72.0 + 7.2 + 27.0)).abs() < 1.0, "x {}", f.3);
    assert!(f.4 > 144.0 && f.4 < 144.0 + 60.0, "y {}", f.4);
    // Nivå 2: tx2 via clrMap.
    let a = find("Andra");
    assert_eq!((a.1, hex(a.2)), (28.0, "1F497D".to_owned()));
    // Punkterna ritas med punktens typsnitt framför texten.
    assert!(r.iter().any(|x| x.0 == "•" && x.3 < f.3));
    assert!(r.iter().any(|x| x.0 == "–"));
}

#[test]
fn clrmapovr_och_tomma_platshallare() {
    let body = text_body(r#"<a:p><a:r><a:rPr><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:rPr><a:t>Ljus</a:t></a:r></a:p>"#);
    let shapes = sp_ph(3, "I", r#"<p:ph idx="1"/>"#, None, &body) + &sp_ph(2, "R", r#"<p:ph type="title"/>"#, None, &text_body("<a:p><a:endParaRPr/></a:p>"));
    let bytes = Builder::new()
        .slide("", &format!("{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:overrideClrMapping bg1=\"dk1\" tx1=\"lt1\" bg2=\"dk2\" tx2=\"lt2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/></p:clrMapOvr><p:cSld><p:spTree>"))
        .build();
    let deck = Deck::open(&bytes).unwrap();
    let r = runs(&deck, 0);
    // tx1 → lt1 (vit) med bildens clrMapOvr; bakgrunden bgRef bg1 → dk1 (svart).
    let l = r.iter().find(|x| x.0 == "Ljus").unwrap();
    assert_eq!(hex(l.2), "FFFFFF");
    assert!(pixel(&deck, 0, 5.0, 5.0)[0] < 10, "svart bakgrund");
    // Den tomma rubriken ritas inte.
    assert_eq!(r.iter().filter(|x| x.0 != "•").count(), 1, "{r:?}");
}

#[test]
fn bildnummer_med_firstslidenum_och_dolda_bilder() {
    let num = sp_ph(4, "N", r#"<p:ph type="sldNum" sz="quarter" idx="12"/>"#, None, &text_body(r#"<a:p><a:fld id="{1}" type="slidenum"><a:t>9</a:t></a:fld></a:p>"#));
    let mut b = Builder::new().slide("", &num).slide(r#"show="0""#, &num);
    b.pres_attrs = r#"firstSlideNum="0""#.into();
    let deck = Deck::open(&b.build()).unwrap();
    assert_eq!(deck.slides.len(), 2);
    assert!(!deck.slides[0].hidden && deck.slides[1].hidden);
    assert_eq!(runs(&deck, 0)[0].0, "0");
    assert_eq!(runs(&deck, 1)[0].0, "1");
}

#[test]
fn former_fylls_och_stilmatrisen_anvands() {
    // En ellips med temats fyllning 2 (phClr = accent1 med tint 50 %) och en rektangel med egen färg.
    let ell = r#"<p:sp><p:nvSpPr><p:cNvPr id="5" name="E"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom></p:spPr><p:style><a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style></p:sp>"#;
    let rect = r#"<p:sp><p:nvSpPr><p:cNvPr id="6" name="R"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="5400000"><a:off x="2540000" y="0"/><a:ext cx="2540000" cy="1270000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></p:spPr></p:sp>"#;
    let deck = Deck::open(&Builder::new().slide("", &format!("{ell}{rect}")).build()).unwrap();
    // Ellipsens mitt (50, 50) är accent1, hörnet (2, 2) är vit bakgrund.
    let c = pixel(&deck, 0, 50.0, 50.0);
    assert!(c[2] > 180 && c[0] < 100, "{c:?}");
    let w = pixel(&deck, 0, 2.0, 2.0);
    assert!(w[0] > 250 && w[1] > 250, "{w:?}");
    // Rektangeln 200×100 pt med mitten i (300, 50), vriden 90°: täcker x 250..350, y −50..150.
    assert_eq!(pixel(&deck, 0, 300.0, 140.0)[..3], [255, 0, 0]);
    assert!(pixel(&deck, 0, 220.0, 50.0)[1] > 250, "utanför den roterade rektangeln");
}

#[test]
fn grupp_med_barnkoordinater() {
    let child = r#"<p:sp><p:nvSpPr><p:cNvPr id="8" name="B"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000" y="1000"/><a:ext cx="1000" cy="1000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></p:spPr></p:sp>"#;
    let grp = format!(r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="7" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="1270000" y="1270000"/><a:ext cx="2540000" cy="2540000"/><a:chOff x="0" y="0"/><a:chExt cx="2000" cy="2000"/></a:xfrm></p:grpSpPr>{child}</p:grpSp>"#);
    let deck = Deck::open(&Builder::new().slide("", &grp).build()).unwrap();
    // Barnet täcker gruppens nedre högra fjärdedel: 200..300 pt.
    assert_eq!(pixel(&deck, 0, 250.0, 250.0)[..3], [0, 255, 0]);
    assert!(pixel(&deck, 0, 150.0, 150.0)[0] > 250);
}

#[test]
fn autofit_och_alternate_content() {
    let shape = r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main"><mc:Choice Requires="a14"><p:sp><p:nvSpPr><p:cNvPr id="9" name="X"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Val</a:t></a:r></a:p></p:txBody></p:sp></mc:Choice><mc:Fallback><p:sp><p:nvSpPr><p:cNvPr id="9" name="X"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="5080000" cy="1270000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr><a:normAutofit fontScale="50000" lnSpcReduction="20000"/></a:bodyPr><a:lstStyle/><a:p><a:r><a:rPr sz="4000"/><a:t>Reserv</a:t></a:r></a:p></p:txBody></p:sp></mc:Fallback></mc:AlternateContent>"#;
    let deck = Deck::open(&Builder::new().slide("", shape).build()).unwrap();
    let r = runs(&deck, 0);
    assert_eq!(r.len(), 1, "{r:?}");
    // Reserven valdes (a14 stöds inte) och fontScale 50 % gäller exakt.
    assert_eq!((r[0].0.as_str(), r[0].1), ("Reserv", 20.0));
}

#[test]
fn radbrytning_och_radavstand() {
    let long = "ord ".repeat(60);
    let body = format!(r#"<p:txBody><a:bodyPr wrap="square"/><a:lstStyle/><a:p><a:pPr><a:lnSpc><a:spcPct val="100000"/></a:lnSpc></a:pPr><a:r><a:rPr sz="2000"/><a:t>{long}</a:t></a:r></a:p></p:txBody>"#);
    let sp = format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="T"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2540000" cy="2540000"/></a:xfrm></p:spPr>{body}</p:sp>"#);
    let deck = Deck::open(&Builder::new().slide("", &sp).build()).unwrap();
    let r = runs(&deck, 0);
    let mut ys: Vec<f32> = r.iter().map(|x| x.4).collect();
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    assert!(ys.len() >= 5, "texten bryts i flera rader: {ys:?}");
    // 100 % radavstånd = 1,2 · 20 pt.
    assert!((ys[1] - ys[0] - 24.0).abs() < 0.05, "{ys:?}");
    // Ingen rad är bredare än rutan (200 pt minus insättningar).
    let (w, h) = deck.size_pt();
    let pm = raster::render(&render::slide_scene(&deck, 0), w as u32, h as u32).unwrap();
    let (pw, _, px) = raster::to_rgba(pm);
    let ink_right = (0..pw as usize).rev().find(|&x| (0..200).any(|y| px[(y * pw as usize + x) * 4] < 128)).unwrap_or(0);
    assert!(ink_right <= 200, "texten går utanför rutan: {ink_right}");
}

#[test]
fn trasiga_filer_ger_fel_inte_panik() {
    assert!(Deck::open(b"inte en zip").is_err());
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    z.start_file("ppt/presentation.xml", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(b"<p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:sldIdLst><p:sldId r:id=\"x\"/>").unwrap();
    let bytes = z.finish().unwrap().into_inner();
    let _ = Deck::open(&bytes);
    // En bild med trasig XML hoppas över; resten fungerar.
    let mut b = Builder::new().slide("", "<p:sp>");
    b = b.slide("", "");
    let deck = Deck::open(&b.build()).unwrap();
    for i in 0..deck.slides.len() {
        let _ = render::slide_scene(&deck, i);
    }
}

#[test]
fn pdf_har_en_sida_per_bild() {
    let body = text_body(r#"<a:p><a:r><a:t>Hej PDF</a:t></a:r></a:p>"#);
    let deck = Deck::open(&Builder::new().slide("", &sp_ph(3, "I", r#"<p:ph idx="1"/>"#, None, &body)).slide("", "").build()).unwrap();
    let pdf = super::pdf::write(&deck, &[0, 1]).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let doc = lopdf::Document::load_mem(&pdf).unwrap();
    assert_eq!(doc.get_pages().len(), 2);
    let text = doc.extract_text(&[1]).unwrap_or_default();
    assert!(text.contains("Hej PDF"), "{text:?}");
}

/// PowerPoints miniatyr (`docProps/thumbnail.jpeg`) mot Kladds rendering av bild 1, för filerna i
/// `tests/corpus/pptx/manifest.toml`. Ett sämre mått än det sparade (minus marginal) fäller provet.
#[test]
fn corpus_pptx_miniatyrer() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/pptx");
    let manifest = std::fs::read_to_string(dir.join("manifest.toml")).expect("manifest.toml");
    let mut checked = 0;
    let mut report = String::new();
    let mut current: Option<String> = None;
    let mut entries: Vec<(String, f64)> = Vec::new();
    for line in manifest.lines() {
        let l = line.trim();
        if let Some(name) = l.strip_prefix("[\"").and_then(|s| s.strip_suffix("\"]")) {
            current = Some(name.to_owned());
        } else if let (Some(n), Some(v)) = (&current, l.strip_prefix("thumbnail_ssim = ")) {
            entries.push((n.clone(), v.trim().parse().expect("tal")));
        }
    }
    for (name, stored) in entries {
        let bytes = std::fs::read(dir.join(&name)).unwrap_or_else(|_| panic!("{name}"));
        let got = thumbnail_score(&bytes).unwrap_or_else(|| panic!("{name}: ingen miniatyr"));
        report.push_str(&format!("[\"{name}\"]\nthumbnail_ssim = {got:.3}\n\n"));
        assert!(got >= stored - 0.03, "{name}: likheten med PowerPoints miniatyr sjönk från {stored:.3} till {got:.3}");
        checked += 1;
    }
    if std::env::var_os("KLADD_CORPUS_BLESS").is_some() {
        println!("{report}");
    }
    assert!(checked >= 5, "för få filer i manifestet");
}

/// Likheten mellan PowerPoints miniatyr och Kladds bild 1 i samma storlek.
pub fn thumbnail_score(bytes: &[u8]) -> Option<f64> {
    let deck = Deck::open(bytes).ok()?;
    let thumb = image::load_from_memory(deck.pkg.read("docProps/thumbnail.jpeg")?).ok()?.to_rgba8();
    let (w, h) = thumb.dimensions();
    let first = (0..deck.slides.len()).find(|&i| !deck.slides[i].hidden)?;
    let pm = raster::render(&render::slide_scene(&deck, first), w, h)?;
    let (_, _, rgba) = raster::to_rgba(pm);
    use super::compare::{Gray, similarity};
    Some(similarity(&Gray::from_rgba(w, h, thumb.as_raw()), &Gray::from_rgba(w, h, &rgba)))
}

/// Mäter miniatyrerna i en mapp (för att välja korpusfiler): `KLADD_PPTX_THUMBS=mapp`.
#[test]
#[ignore]
fn miniatyrer_i_mapp() {
    let Some(dir) = std::env::var_os("KLADD_PPTX_THUMBS") else { return };
    let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pptx")).collect();
    v.sort();
    for p in v {
        let s = std::fs::read(&p).ok().and_then(|b| thumbnail_score(&b));
        println!("{:.3} {}", s.unwrap_or(-1.0), p.display());
        // Sida vid sida bredvid filen.
        if let Some(bytes) = std::fs::read(&p).ok() {
            if let (Ok(deck), Some(t)) = (Deck::open(&bytes), Deck::open(&bytes).ok().and_then(|d| d.pkg.read("docProps/thumbnail.jpeg").map(|b| b.to_vec()))) {
                if let Ok(thumb) = image::load_from_memory(&t).map(|i| i.to_rgba8()) {
                    let (w, h) = thumb.dimensions();
                    if let Some(pm) = raster::render(&render::slide_scene(&deck, 0), w, h) {
                        let (_, _, rgba) = raster::to_rgba(pm);
                        if let Some(ours) = image::RgbaImage::from_raw(w, h, rgba) {
                            super::compare::side_by_side(&thumb, &ours, &p.with_extension("cmp.png"));
                        }
                    }
                }
            }
        }
    }
}

/// Felsökning: `KLADD_PPTX_DUMP=fil.pptx:7` skriver visningslistan för bild 7.
#[test]
#[ignore]
fn dump_scene() {
    let Ok(spec) = std::env::var("KLADD_PPTX_DUMP") else { return };
    let (path, n) = spec.rsplit_once(':').unwrap();
    let deck = Deck::open(&std::fs::read(path).unwrap()).unwrap();
    let scene = render::slide_scene(&deck, n.parse::<usize>().unwrap() - 1);
    for it in &scene.items {
        match it {
            Item::Fill { path, paint, .. } => println!("fill {:?} {:?}", path.bounds(), paint_desc(paint)),
            Item::Stroke { path, stroke } => println!("stroke {:?} w={} {:?}", path.bounds(), stroke.width, paint_desc(&stroke.paint)),
            Item::Glyphs(g) => println!("text {:?} {} {} {:?} at {:?}", g.text, g.font.family, g.size, paint_desc(&g.paint), g.t.apply(0.0, 0.0)),
            Item::Image { image, t, crop, .. } => println!("image {}x{} t={:?} crop={:?}", image.width, image.height, t, crop),
            Item::Svg { t, alpha, .. } => println!("svg t={t:?} alpha={alpha}"),
            Item::PushClip { path } => println!("clip {:?}", path.bounds()),
            Item::PopClip => println!("pop"),
            Item::BeginEffect(e) => println!("effect {e:?}"),
            Item::EndEffect => println!("end effect"),
        }
    }
}

fn paint_desc(p: &Paint) -> String {
    match p {
        Paint::Solid(c) => format!("#{} a={:.2}", hex(*c), c.a),
        Paint::Linear { stops, .. } => format!("linear {} stopp", stops.len()),
        Paint::Radial { stops, .. } => format!("radial {} stopp", stops.len()),
        Paint::Image { image, .. } => format!("bild {}x{}", image.width, image.height),
    }
}

#[test]
fn presentationsflik_skriver_aldrig_over_filen() {
    let dir = std::env::temp_dir().join(format!("kladd-pptx-flik-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("prov.pptx");
    let bytes = Builder::new().slide("", &sp_ph(3, "I", r#"<p:ph idx="1"/>"#, None, &text_body("<a:p><a:r><a:t>Hej</a:t></a:r></a:p>"))).build();
    std::fs::write(&path, &bytes).unwrap();
    let mut doc = crate::doc::Document::load(1, &path).unwrap();
    assert!(doc.pptx.is_some() && !doc.dirty);
    assert_eq!(doc.language(), "PowerPoint");
    // Ett kommando som ändrar texten gör inte fliken ändrad, och sparning vägras.
    doc.text.push_str("skräp");
    doc.on_edited();
    assert!(!doc.dirty && doc.text.is_empty());
    assert!(doc.save_to(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "filen är orörd");
    // En trasig .pptx ger ett felmeddelande, ingen panik.
    let bad = dir.join("trasig.pptx");
    std::fs::write(&bad, b"inte en zip").unwrap();
    assert!(crate::doc::Document::load(2, &bad).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Öppnar och renderar alla .pptx under `KLADD_PPTX_SWEEP` (rekursivt) i en tråd med visningens
/// stackstorlek: inga paniker, ingen stack som tar slut.
#[test]
#[ignore]
fn oppna_alla_utan_krasch() {
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
    let mut slides = 0;
    let mut errors = 0;
    for f in &files {
        let f2 = f.clone();
        let r = std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(move || {
                let bytes = std::fs::read(&f2).unwrap();
                match Deck::open(&bytes) {
                    Ok(deck) => {
                        for i in 0..deck.slides.len() {
                            let _ = super::render_slide(&deck, i, 320, 180);
                        }
                        Ok(deck.slides.len())
                    }
                    Err(e) => Err(e),
                }
            })
            .unwrap()
            .join();
        match r {
            Ok(Ok(n)) => slides += n,
            Ok(Err(e)) => {
                errors += 1;
                println!("fel (väntat för trasiga filer): {}: {e}", f.display());
            }
            Err(_) => panic!("panik i {}", f.display()),
        }
    }
    println!("{} filer, {slides} bilder, {errors} öppnades inte", files.len());
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        println!("{}", s.lines().filter(|l| l.starts_with("VmHWM")).collect::<Vec<_>>().join(" "));
    }
}

/// PDF mot raster: skriver bilderna i `KLADD_PPTX_PDF=fil.pptx` till PDF, rastrerar med `pdftoppm -r 72` och
/// jämför med Kladds egen rastrering (samma visningslista ska ge samma bild).
#[test]
#[ignore]
fn pdf_mot_raster() {
    let Some(path) = std::env::var_os("KLADD_PPTX_PDF") else { return };
    let deck = Deck::open(&std::fs::read(&path).unwrap()).unwrap();
    let n = deck.slides.len().min(12);
    let pdf = super::pdf::write(&deck, &(0..n).collect::<Vec<_>>()).unwrap();
    let dir = std::env::temp_dir().join(format!("kladd-pptx-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ut.pdf"), &pdf).unwrap();
    let ok = std::process::Command::new("pdftoppm").args(["-r", "72", "-png"]).arg(dir.join("ut.pdf")).arg(dir.join("s")).status().is_ok_and(|s| s.success());
    assert!(ok, "pdftoppm saknas");
    let mut pages: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "png")).collect();
    pages.sort();
    for (i, p) in pages.iter().enumerate() {
        let img = image::open(p).unwrap().to_rgba8();
        let (w, h) = img.dimensions();
        let (_, _, ours) = super::render_slide(&deck, i, w, h).unwrap();
        use super::compare::{Gray, similarity};
        let s = similarity(&Gray::from_rgba(w, h, img.as_raw()), &Gray::from_rgba(w, h, &ours));
        println!("bild {}: {s:.4}", i + 1);
    }
    println!("PDF: {}", dir.display());
}

/// Reserv för ett tecken som inget typsnitt har: sista utvägen går igenom alla typsnitt utan att
/// läsa in dem (tid och minne skrivs ut).
#[test]
#[ignore]
fn reserv_for_tecken_som_saknas() {
    let book = super::fonts::FontBook::default();
    let base = book.resolve("Calibri", false, false);
    let t = std::time::Instant::now();
    let r = book.fallback('\u{10FFFD}', &base.face);
    println!("{:?} på {:?}", r.map(|f| f.family.clone()), t.elapsed());
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        println!("{}", s.lines().filter(|l| l.starts_with("VmRSS") || l.starts_with("VmHWM")).collect::<Vec<_>>().join(" "));
    }
}

/// Felsökning av minnet: `KLADD_PPTX_MEM=fil.pptx` skriver RSS efter öppning och efter varje bild.
#[test]
#[ignore]
fn minne_per_bild() {
    let Some(p) = std::env::var_os("KLADD_PPTX_MEM") else { return };
    let rss = || std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS")).map(|l| l.to_owned())).unwrap_or_default();
    let deck = Deck::open(&std::fs::read(p).unwrap()).unwrap();
    println!("öppnad: {}", rss());
    for i in 0..deck.slides.len() {
        let s = render::slide_scene(&deck, i);
        println!("bild {} scen: {}", i + 1, rss());
        let _ = raster::render(&s, 320, 180);
        println!("bild {} raster: {}", i + 1, rss());
    }
}
