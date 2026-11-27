//! Prov för tabeller och SmartArt på egna, minimala .pptx-filer (zip i minnet): sammanslagna
//! celler, band, inbyggt format via GUID, filens format före det inbyggda, diagonaler, dubbla
//! linjer, rader som växer med texten, formatets textfärg mot mallens, SmartArt-text med
//! `a:fontRef`-färg och reserven utan ritdel.

use std::io::Write;

use super::super::color::Rgba;
use super::super::scene::{Item, Paint};
use super::super::{Deck, raster, render};

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Prov"><a:themeElements>
<a:clrScheme name="Prov"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
<a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4>
<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="Prov"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>
<a:fmtScheme name="Prov"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
<a:lnStyleLst><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="25400"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="38100"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme>
</a:themeElements></a:theme>"#;

/// Bygger en .pptx med en bild. `parts` är extra delar (namn, innehåll) och `rels` extra
/// relationer från bilden; `table_styles` blir `ppt/tableStyles.xml`.
fn pptx(slide_inner: &str, table_styles: Option<&str>, parts: &[(&str, &str)], rels: &str) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut put = |name: &str, data: &str| {
        z.start_file(name, opt).unwrap();
        z.write_all(data.as_bytes()).unwrap();
    };
    put("[Content_Types].xml", r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/></Types>"#);
    put("_rels/.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#));
    let ts_rel = if table_styles.is_some() { format!(r#"<Relationship Id="rIdT" Type="{REL}/tableStyles" Target="tableStyles.xml"/>"#) } else { String::new() };
    put("ppt/_rels/presentation.xml.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdM" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rIdS1" Type="{REL}/slide" Target="slides/slide1.xml"/>{ts_rel}</Relationships>"#));
    put("ppt/presentation.xml", &format!(r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rIdS1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/><p:defaultTextStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:defaultTextStyle></p:presentation>"#));
    let tree = |inner: &str| format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{inner}</p:spTree></p:cSld>"#);
    // Mallens otherStyle har en egen textfärg: formatets färg ska gå före den.
    put("ppt/slideMasters/slideMaster1.xml", &format!(r#"<p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:txStyles><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#));
    put("ppt/slideMasters/_rels/slideMaster1.xml.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{REL}/theme" Target="../theme/theme1.xml"/></Relationships>"#));
    put("ppt/slideLayouts/slideLayout1.xml", &format!(r#"<p:sldLayout {NS}>{}</p:sldLayout>"#, tree("")));
    put("ppt/slideLayouts/_rels/slideLayout1.xml.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#));
    put("ppt/theme/theme1.xml", THEME);
    put("ppt/slides/slide1.xml", &format!(r#"<p:sld {NS}>{}</p:sld>"#, tree(slide_inner)));
    put("ppt/slides/_rels/slide1.xml.rels", &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>{rels}</Relationships>"#));
    if let Some(ts) = table_styles {
        put("ppt/tableStyles.xml", ts);
    }
    for (name, data) in parts {
        put(name, data);
    }
    z.finish().unwrap().into_inner()
}

/// En tabell i en ram vid (72, 72) pt: kolumner à 144 pt, `rows` som `a:tr`-XML.
fn table(tbl_pr: &str, ncols: usize, rows: &str) -> String {
    let grid: String = (0..ncols).map(|_| r#"<a:gridCol w="1828800"/>"#).collect();
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Tabell"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="914400"/><a:ext cx="{}" cy="1371600"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl>{tbl_pr}<a:tblGrid>{grid}</a:tblGrid>{rows}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
        1828800 * ncols
    )
}

/// En cell med text och `a:tcPr`-innehåll.
fn tc(attrs: &str, text: &str, tcpr: &str) -> String {
    let p = if text.is_empty() { "<a:p><a:endParaRPr sz=\"1200\"/></a:p>".to_owned() } else { text.split('\n').map(|t| format!(r#"<a:p><a:r><a:rPr lang="sv-SE" sz="1200"/><a:t>{t}</a:t></a:r></a:p>"#)).collect() };
    format!(r#"<a:tc {attrs}><a:txBody><a:bodyPr/><a:lstStyle/>{p}</a:txBody><a:tcPr>{tcpr}</a:tcPr></a:tc>"#)
}

fn tr(h: i64, cells: &[String]) -> String {
    format!(r#"<a:tr h="{h}">{}</a:tr>"#, cells.concat())
}

fn solid(hex: &str) -> String {
    format!(r#"<a:solidFill><a:srgbClr val="{hex}"/></a:solidFill>"#)
}

fn pixel(deck: &Deck, x: f32, y: f32) -> [u8; 3] {
    let (w, h) = deck.size_pt();
    let (pw, _, rgba) = raster::to_rgba(raster::render(&render::slide_scene(deck, 0), w as u32, h as u32).unwrap());
    let o = ((y as u32 * pw + x as u32) * 4) as usize;
    [rgba[o], rgba[o + 1], rgba[o + 2]]
}

/// Glyfkörningarna: (text, färg, x, y).
fn runs(deck: &Deck) -> Vec<(String, Rgba, f32, f32)> {
    render::slide_scene(deck, 0)
        .items
        .into_iter()
        .filter_map(|it| match it {
            Item::Glyphs(g) => {
                let c = match g.paint {
                    Paint::Solid(c) => c,
                    _ => Rgba::BLACK,
                };
                let (x, y) = g.t.apply(g.glyphs.first().map_or(0.0, |p| p.x), 0.0);
                Some((g.text, c, x, y))
            }
            _ => None,
        })
        .collect()
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= 6)
}

#[test]
fn sammanslagna_celler_ritas_som_en() {
    // 3×3: rad 0 kolumn 0–1 sammanslagen (gridSpan + hMerge), kolumn 0 rad 1–2 (rowSpan + vMerge).
    let rows = tr(457200, &[tc(r#"gridSpan="2""#, "AB", &solid("FF0000")), tc(r#"hMerge="1""#, "", &solid("00FF00")), tc("", "C", "")])
        + &tr(457200, &[tc(r#"rowSpan="2""#, "D", &solid("0000FF")), tc("", "E", ""), tc("", "F", "")])
        + &tr(457200, &[tc(r#"vMerge="1""#, "", &solid("00FF00")), tc("", "H", ""), tc("", "I", "")]);
    let tbl = table(r#"<a:tblPr><a:tableStyleId>{5940675A-B579-460E-94D1-54222C63F5DA}</a:tableStyleId></a:tblPr>"#, 3, &rows);
    let deck = Deck::open(&pptx(&tbl, None, &[], "")).unwrap();
    // Den sammanslagna cellen fyller båda kolumnerna; den dolda cellens fyllning syns inte.
    assert!(near(pixel(&deck, 100.0, 90.0), [255, 0, 0]));
    assert!(near(pixel(&deck, 300.0, 90.0), [255, 0, 0]));
    assert!(near(pixel(&deck, 100.0, 170.0), [0, 0, 255]));
    // Ingen rutnätslinje genom de sammanslagna cellerna (formatets kanter är 1 pt svarta), men runt dem.
    assert!(near(pixel(&deck, 216.0, 90.0), [255, 0, 0]), "{:?}", pixel(&deck, 216.0, 90.0));
    assert!(near(pixel(&deck, 100.0, 144.0), [0, 0, 255]), "{:?}", pixel(&deck, 100.0, 144.0));
    assert!(pixel(&deck, 359.0, 160.0)[1].min(pixel(&deck, 360.0, 160.0)[1]) < 160, "lodrät linje mellan kolumn 1 och 2");
    let texts: Vec<String> = runs(&deck).into_iter().map(|r| r.0).collect();
    assert_eq!(texts.iter().filter(|t| t.as_str() == "AB").count(), 1);
    assert!(texts.contains(&"I".to_owned()));
}

#[test]
fn inbyggt_format_med_band_och_rubrikrad() {
    // Medium Style 2 – Accent 1 finns inte i filen (ingen tableStyles.xml), som från python-pptx.
    let rows: String = (0..4).map(|i| tr(457200, &[tc("", &format!("R{i}"), ""), tc("", "x", "")])).collect();
    let tbl = table(r#"<a:tblPr firstRow="1" bandRow="1"><a:tableStyleId>{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}</a:tableStyleId></a:tblPr>"#, 2, &rows);
    let deck = Deck::open(&pptx(&tbl, None, &[], "")).unwrap();
    let head = pixel(&deck, 200.0, 76.0);
    let band1 = pixel(&deck, 200.0, 112.0);
    let band2 = pixel(&deck, 200.0, 148.0);
    let band3 = pixel(&deck, 200.0, 184.0);
    assert!(near(head, [0x44, 0x72, 0xC4]), "{head:?}");
    // band1H (tint 40 %) är mörkare än wholeTbl (tint 20 %), och banden växlar.
    assert!(band1[0] < band2[0] && band1[2] < band2[2], "{band1:?} {band2:?}");
    assert!(near(band1, band3), "{band1:?} {band3:?}");
    // Rubrikradens text är vit (lt1) trots att mallens otherStyle säger tx1; övriga rader svarta.
    let rs = runs(&deck);
    let c0 = rs.iter().find(|r| r.0 == "R0").unwrap().1;
    let c1 = rs.iter().find(|r| r.0 == "R1").unwrap().1;
    assert_eq!(c0, Rgba::WHITE);
    assert_eq!(c1, Rgba::BLACK);
}

#[test]
fn filens_format_gar_fore_det_inbyggda() {
    let ts = r#"<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}"><a:tblStyle styleId="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}" styleName="Eget"><a:wholeTbl><a:tcStyle><a:tcBdr/><a:fill><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></a:fill></a:tcStyle></a:wholeTbl></a:tblStyle></a:tblStyleLst>"#;
    let rows = tr(457200, &[tc("", "a", ""), tc("", "b", "")]);
    let tbl = table(r#"<a:tblPr firstRow="1"><a:tableStyleId>{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}</a:tableStyleId></a:tblPr>"#, 2, &rows);
    let deck = Deck::open(&pptx(&tbl, Some(ts), &[], "")).unwrap();
    assert!(near(pixel(&deck, 200.0, 90.0), [0, 255, 0]));
    // Okänt GUID: inget format (vit bakgrund syns).
    let tbl = table(r#"<a:tblPr firstRow="1"><a:tableStyleId>{00000000-1111-2222-3333-444444444444}</a:tableStyleId></a:tblPr>"#, 2, &rows);
    let deck = Deck::open(&pptx(&tbl, None, &[], "")).unwrap();
    assert!(near(pixel(&deck, 200.0, 90.0), [255, 255, 255]));
}

#[test]
fn diagonaler_och_egna_kanter() {
    let ln = |tag: &str, w: i64, hex: &str| format!(r#"<a:{tag} w="{w}" cmpd="sng">{}</a:{tag}>"#, solid(hex));
    let tcpr = format!("{}{}{}", ln("lnL", 25400, "000000"), r#"<a:lnR w="25400"><a:noFill/></a:lnR>"#, ln("lnTlToBr", 38100, "FF0000"));
    let rows = tr(1371600, &[tc("", "", &tcpr), tc("", "", "")]);
    let tbl = table("<a:tblPr/>", 2, &rows);
    let deck = Deck::open(&pptx(&tbl, None, &[], "")).unwrap();
    // Diagonalen från övre vänstra till nedre högra hörnet (cellen 144 × 108 pt vid 72, 72).
    assert!(near(pixel(&deck, 144.0, 126.0), [255, 0, 0]), "{:?}", pixel(&deck, 144.0, 126.0));
    assert!(near(pixel(&deck, 180.0, 100.0), [255, 255, 255]));
    // Vänsterkanten 2 pt svart; högerkanten uttryckligen ingen.
    assert!(pixel(&deck, 72.0, 150.0)[0] < 60);
    assert!(near(pixel(&deck, 216.0, 150.0), [255, 255, 255]));
}

#[test]
fn dubbel_linje_och_temats_linjestil() {
    // Light Style 3 – Accent 1: lastRow har en dubbel linje (4 pt, cmpd="dbl") överst.
    let rows: String = (0..3).map(|i| tr(457200, &[tc("", &format!("r{i}"), "")])).collect();
    let tbl = table(r#"<a:tblPr lastRow="1"><a:tableStyleId>{BC89EF96-8CEA-46FF-86C4-4CE0E7609802}</a:tableStyleId></a:tblPr>"#, 1, &rows);
    let deck = Deck::open(&pptx(&tbl, None, &[], "")).unwrap();
    // Linjen ligger vid y = 72 + 72 = 144: två streck (±1,33 pt) med luft emellan.
    let col: Vec<u8> = (138..151).map(|y| pixel(&deck, 140.0, y as f32)[2]).collect();
    let dark: Vec<bool> = col.iter().map(|v| *v < 240).collect();
    assert!(dark.iter().filter(|d| **d).count() >= 2, "{col:?}");
    assert!(!dark[6] || col[6] > col[4].min(col[8]), "luft i mitten: {col:?}");
    // Themed Style 1 – Accent 1: kanterna är temats linjestil 1 (1 pt) i accentfärgen.
    let tbl = table(r#"<a:tblPr><a:tableStyleId>{3C2FFA5D-87B4-456A-9821-1D502468CF0F}</a:tableStyleId></a:tblPr>"#, 1, &rows);
    let deck = Deck::open(&pptx(&tbl, None, &[], "")).unwrap();
    let c = pixel(&deck, 72.0, 100.0);
    assert!(c[2] > c[0] + 40, "accentfärgad kant: {c:?}");
}

#[test]
fn raden_vaxer_med_texten() {
    // Den sparade höjden (0,1 tum) rymmer inte fyra rader text: nästa rad flyttas ned.
    let rows = tr(91440, &[tc("", "a\nb\nc\nd", "")]) + &tr(91440, &[tc("", "nästa", "")]);
    let tbl = table("<a:tblPr/>", 1, &rows);
    let deck = Deck::open(&pptx(&tbl, None, &[], "")).unwrap();
    let rs = runs(&deck);
    let y = |t: &str| rs.iter().find(|r| r.0 == t).unwrap().3;
    // 12 pt text, 1,2 × 12 per rad: "nästa" ligger minst fyra rader under "a".
    assert!(y("nästa") - y("a") > 4.0 * 12.0, "{} {}", y("a"), y("nästa"));
    assert!(y("d") > y("c") && y("c") > y("b"));
    // Över två rader: rowSpan-cellens text får plats genom att den sista raden växer.
    let rows = tr(91440, &[tc(r#"rowSpan="2""#, "1\n2\n3\n4\n5\n6", ""), tc("", "x", "")]) + &tr(91440, &[tc(r#"vMerge="1""#, "", ""), tc("", "y", "")]) + &tr(91440, &[tc("", "efter", ""), tc("", "", "")]);
    let tbl = table("<a:tblPr/>", 2, &rows);
    let deck = Deck::open(&pptx(&tbl, None, &[], "")).unwrap();
    let rs = runs(&deck);
    let y = |t: &str| rs.iter().find(|r| r.0 == t).unwrap().3;
    assert!(y("efter") > y("6"), "{} {}", y("6"), y("efter"));
}

#[test]
fn formatets_text_fet_och_farg_ur_delarna() {
    use super::{style_parts, text_list, text_style, Area, Flags};
    let st = super::super::table_builtin::style("{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}");
    let fl = Flags { first_row: true, band_row: true, ..Default::default() };
    let head = style_parts(st, fl, Area { r0: 0, r1: 1, c0: 0, c1: 1 }, 3, 2);
    let names: Vec<&str> = head.iter().map(|p| p.node.name.as_str()).collect();
    assert_eq!(names, ["a:wholeTbl", "a:firstRow"]);
    let body = style_parts(st, fl, Area { r0: 1, r1: 2, c0: 0, c1: 1 }, 3, 2);
    assert_eq!(body.iter().map(|p| p.node.name.as_str()).collect::<Vec<_>>(), ["a:wholeTbl", "a:band1H"]);
    let t = text_style(&head);
    assert_eq!(t.bold, Some(true));
    assert_eq!(t.color.and_then(|c| c.attr("val")), Some("lt1"));
    let l = text_list(&t);
    let d = l.path(&["a:lvl1pPr", "a:defRPr"]).unwrap();
    assert_eq!(d.attr("b"), Some("1"));
    assert!(d.path(&["a:solidFill", "a:schemeClr"]).is_some());
    // Hörncellen: nwCell efter firstRow och firstCol.
    let st = super::super::table_builtin::style("{E8034E78-7F5D-4C2E-B375-FC64B27BC917}");
    let fl = Flags { first_row: true, first_col: true, ..Default::default() };
    let corner = style_parts(st, fl, Area { r0: 0, r1: 1, c0: 0, c1: 1 }, 3, 3);
    assert_eq!(corner.iter().map(|p| p.node.name.as_str()).collect::<Vec<_>>(), ["a:wholeTbl", "a:firstCol", "a:firstRow", "a:nwCell"]);
}

const DGM_NS: &str = r#"xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;

fn diagram_frame() -> String {
    r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="SmartArt"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="914400"/><a:ext cx="5486400" cy="3657600"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rIdD" r:lo="rIdL" r:qs="rIdQ" r:cs="rIdC"/></a:graphicData></a:graphic></p:graphicFrame>"#.to_owned()
}

#[test]
fn smartart_utan_ritdel_visar_datamodellens_text() {
    let pt = |id: &str, t: &str| format!(r#"<dgm:pt modelId="{id}"><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE"/><a:t>{t}</a:t></a:r></a:p></dgm:t></dgm:pt>"#);
    let cxn = |s: &str, d: &str, o: i32| format!(r#"<dgm:cxn modelId="c{s}{d}" srcId="{s}" destId="{d}" srcOrd="{o}" destOrd="0"/>"#);
    let data = format!(
        r#"<dgm:dataModel {DGM_NS}><dgm:ptLst><dgm:pt modelId="doc" type="doc"><dgm:t><a:bodyPr/><a:p/></dgm:t></dgm:pt>{}{}{}{}<dgm:pt modelId="p1" type="pres"/></dgm:ptLst><dgm:cxnLst>{}{}{}{}<dgm:cxn modelId="x" type="presOf" srcId="a" destId="p1"/></dgm:cxnLst></dgm:dataModel>"#,
        pt("a", "Första"),
        pt("b", "Andra"),
        pt("a1", "Under första"),
        pt("t", ""),
        cxn("doc", "b", 1),
        cxn("doc", "a", 0),
        cxn("a", "a1", 0),
        cxn("doc", "t", 2)
    );
    let rels = format!(r#"<Relationship Id="rIdD" Type="{REL}/diagramData" Target="../diagrams/data1.xml"/>"#);
    let deck = Deck::open(&pptx(&diagram_frame(), None, &[("ppt/diagrams/data1.xml", &data)], &rels)).unwrap();
    let texts: Vec<String> = runs(&deck).into_iter().map(|r| r.0).filter(|t| t != "•").collect();
    assert_eq!(texts, ["Första", "Under första", "Andra"]);
    // Utan text i datamodellen: platshållaren som förut.
    let empty = format!(r#"<dgm:dataModel {DGM_NS}><dgm:ptLst><dgm:pt modelId="doc" type="doc"/></dgm:ptLst><dgm:cxnLst/></dgm:dataModel>"#);
    let deck = Deck::open(&pptx(&diagram_frame(), None, &[("ppt/diagrams/data1.xml", &empty)], &rels)).unwrap();
    assert!(runs(&deck).iter().any(|r| r.0 == "SmartArt"));
}

#[test]
fn smartart_ritdel_med_fontref_ger_vit_text() {
    let data = format!(
        r#"<dgm:dataModel {DGM_NS}><dgm:ptLst><dgm:pt modelId="doc" type="doc"/></dgm:ptLst><dgm:cxnLst/><dgm:extLst><a:ext uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rIdW"/></a:ext></dgm:extLst></dgm:dataModel>"#
    );
    let drawing = r#"<dsp:drawing xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/><dsp:sp modelId="{1}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2743200" cy="914400"/></a:xfrm><a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></dsp:spPr><dsp:style><a:lnRef idx="2"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef><a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dsp:style><dsp:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE" sz="2000"/><a:t>Vit</a:t></a:r><a:r><a:rPr lang="sv-SE" sz="2000"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>Röd</a:t></a:r></a:p></dsp:txBody><dsp:txXfrm><a:off x="0" y="0"/><a:ext cx="2743200" cy="914400"/></dsp:txXfrm></dsp:sp></dsp:spTree></dsp:drawing>"#;
    let rels = format!(r#"<Relationship Id="rIdD" Type="{REL}/diagramData" Target="../diagrams/data1.xml"/><Relationship Id="rIdW" Type="http://schemas.microsoft.com/office/2007/relationships/diagramDrawing" Target="../diagrams/drawing1.xml"/>"#);
    let deck = Deck::open(&pptx(&diagram_frame(), None, &[("ppt/diagrams/data1.xml", &data), ("ppt/diagrams/drawing1.xml", drawing)], &rels)).unwrap();
    let rs = runs(&deck);
    // Formens fontRef-färg går före mallens otherStyle; körningens egen färg går före båda.
    assert_eq!(rs.iter().find(|r| r.0 == "Vit").map(|r| r.1), Some(Rgba::WHITE), "{rs:?}");
    assert_eq!(rs.iter().find(|r| r.0 == "Röd").map(|r| r.1), Some(Rgba::rgb8(255, 0, 0)));
    // Formen ritas (accent2) inom ramen vid (72, 72).
    assert!(near(pixel(&deck, 90.0, 80.0), [0xED, 0x7D, 0x31]), "{:?}", pixel(&deck, 90.0, 80.0));
}

#[test]
fn smartart_text_i_egen_textpunkt_far_dess_farg() {
    // Pyramidens text visas av en egen punkt (`revTx`): PowerPoint ritar den med färgdefinitionens
    // `tx1`, inte med formens `fontRef` (`lt1`) som ritdelen har.
    let data = format!(
        r#"<dgm:dataModel {DGM_NS}><dgm:ptLst><dgm:pt modelId="doc" type="doc"><dgm:prSet csTypeId="urn:prov/colors"/></dgm:pt><dgm:pt modelId="n1"><dgm:t><a:bodyPr/><a:p><a:r><a:t>Text</a:t></a:r></a:p></dgm:t></dgm:pt><dgm:pt modelId="S1" type="pres"><dgm:prSet presAssocID="n1" presName="level" presStyleLbl="node1"/></dgm:pt><dgm:pt modelId="T1" type="pres"><dgm:prSet presAssocID="n1" presName="levelTx" presStyleLbl="revTx"/></dgm:pt></dgm:ptLst><dgm:cxnLst><dgm:cxn modelId="c1" srcId="doc" destId="n1" srcOrd="0" destOrd="0"/><dgm:cxn modelId="c2" type="presOf" srcId="n1" destId="S1" srcOrd="0" destOrd="0"/><dgm:cxn modelId="c3" type="presOf" srcId="n1" destId="T1" srcOrd="1" destOrd="0"/></dgm:cxnLst></dgm:dataModel>"#
    );
    let colors = format!(r#"<dgm:colorsDef {DGM_NS} uniqueId="urn:prov/colors"><dgm:styleLbl name="node1"><dgm:txFillClrLst/></dgm:styleLbl><dgm:styleLbl name="revTx"><dgm:txFillClrLst meth="repeat"><a:schemeClr val="tx1"/></dgm:txFillClrLst></dgm:styleLbl></dgm:colorsDef>"#);
    let drawing = r#"<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/><dsp:sp modelId="S1"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2743200" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></dsp:spPr><dsp:style><a:lnRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef><a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dsp:style><dsp:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE" sz="2000"/><a:t>Text</a:t></a:r></a:p></dsp:txBody></dsp:sp></dsp:spTree></dsp:drawing>"#;
    let rels = format!(r#"<Relationship Id="rIdD" Type="{REL}/diagramData" Target="../diagrams/data1.xml"/><Relationship Id="rIdC" Type="{REL}/diagramColors" Target="../diagrams/colors1.xml"/><Relationship Id="rIdW" Type="http://schemas.microsoft.com/office/2007/relationships/diagramDrawing" Target="../diagrams/drawing1.xml"/>"#);
    let parts = [("ppt/diagrams/data1.xml", data.as_str()), ("ppt/diagrams/colors1.xml", colors.as_str()), ("ppt/diagrams/drawing1.xml", drawing)];
    let deck = Deck::open(&pptx(&diagram_frame(), None, &parts, &rels)).unwrap();
    let rs = runs(&deck);
    assert_eq!(rs.iter().find(|r| r.0 == "Text").map(|r| r.1), Some(Rgba::BLACK), "{rs:?}");
    // Utan den egna textpunkten behålls formens färg.
    let data2 = data.replace(r#"<dgm:cxn modelId="c3" type="presOf" srcId="n1" destId="T1" srcOrd="1" destOrd="0"/>"#, "");
    let parts = [("ppt/diagrams/data1.xml", data2.as_str()), ("ppt/diagrams/colors1.xml", colors.as_str()), ("ppt/diagrams/drawing1.xml", drawing)];
    let deck = Deck::open(&pptx(&diagram_frame(), None, &parts, &rels)).unwrap();
    assert_eq!(runs(&deck).iter().find(|r| r.0 == "Text").map(|r| r.1), Some(Rgba::WHITE));
}

#[test]
fn fontref_fargen_mellan_egna_och_mallens_nivastilar() {
    // Formens egen `a:lstStyle` går före `p:style/a:fontRef`, som går före mallens otherStyle.
    let sp = |id: u32, lst: &str, text: &str| {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="F{id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="{}"/><a:ext cx="2743200" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:style><a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style><p:txBody><a:bodyPr/><a:lstStyle>{lst}</a:lstStyle><a:p><a:r><a:rPr lang="sv-SE"/><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#,
            id as i64 * 1_000_000
        )
    };
    let red = r#"<a:lvl1pPr><a:defRPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:defRPr></a:lvl1pPr>"#;
    let deck = Deck::open(&pptx(&(sp(2, "", "Tema") + &sp(3, red, "Egen")), None, &[], "")).unwrap();
    let rs = runs(&deck);
    assert_eq!(rs.iter().find(|r| r.0 == "Tema").map(|r| r.1), Some(Rgba::WHITE));
    assert_eq!(rs.iter().find(|r| r.0 == "Egen").map(|r| r.1), Some(Rgba::rgb8(255, 0, 0)));
}
