//! Prov på redigering av diagramdata: egen presentation med ett stapeldiagram och en inbäddad
//! arbetsbok (byggda i provet), ändringar av värden, namn, kategorier, serier och rubriker, sparning
//! som går igenom `validate`, arbetsbokens celler, tabell och beräkningskedja, och att diagrammet ritas
//! med de nya värdena.

use std::io::{Read, Write as _};

use super::edit::Session;
use super::edit_chart::{self, Range};
use super::edit_chart_xlsx::{self as xlsx, CellVal};
use super::edit_tests::validate;
use super::Deck;

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const RELS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    for (n, d) in files {
        z.start_file(*n, opt).unwrap();
        z.write_all(d).unwrap();
    }
    z.finish().unwrap().into_inner()
}

/// Arbetsboken: Kategori/Äpplen/Päron i A1:C1, Q1–Q3 i A2:A4, värden i B2:C4 (C4 en formel), en tabell
/// över A1:C4 och en beräkningskedja.
pub fn workbook() -> Vec<u8> {
    let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/xl/calcChain.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.calcChain+xml"/><Override PartName="/xl/tables/table1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#;
    let rels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#);
    let wb = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{RELS}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#);
    let wb_rels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{RELS}/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId3" Type="{RELS}/calcChain" Target="calcChain.xml"/><Relationship Id="rId4" Type="{RELS}/styles" Target="styles.xml"/></Relationships>"#);
    let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="0.0"/></numFmts><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#;
    let sheet = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><dimension ref="A1:C4"/><sheetData><row r="1" spans="1:3"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c></row><row r="2" spans="1:3"><c r="A2" t="s"><v>3</v></c><c r="B2"><v>1</v></c><c r="C2"><v>3</v></c></row><row r="3" spans="1:3"><c r="A3" t="s"><v>4</v></c><c r="B3"><v>2</v></c><c r="C3"><v>4</v></c></row><row r="4" spans="1:3"><c r="A4" t="s"><v>5</v></c><c r="B4" s="1"><v>4</v></c><c r="C4"><f>C3+1</f><v>5</v></c></row></sheetData><tableParts count="1"><tablePart r:id="rId1"/></tableParts></worksheet>"#;
    let sheet_rels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/table" Target="../tables/table1.xml"/></Relationships>"#);
    let sst = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="6" uniqueCount="6"><si><t>Kategori</t></si><si><t>Äpplen</t></si><si><t>Päron</t></si><si><t>Q1</t></si><si><t>Q2</t></si><si><t>Q3</t></si></sst>"#;
    let calc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><calcChain xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><c r="C4" i="1"/></calcChain>"#;
    let table = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="1" name="Tabell1" displayName="Tabell1" ref="A1:C4" totalsRowShown="0"><autoFilter ref="A1:C4"/><tableColumns count="3"><tableColumn id="1" name="Kategori"/><tableColumn id="2" name="Äpplen"/><tableColumn id="3" name="Päron"/></tableColumns><tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/></table>"#;
    zip(&[
        ("[Content_Types].xml", ct.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("xl/workbook.xml", wb.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels.as_bytes()),
        ("xl/sharedStrings.xml", sst.as_bytes()),
        ("xl/calcChain.xml", calc.as_bytes()),
        ("xl/tables/table1.xml", table.as_bytes()),
        ("xl/styles.xml", styles.as_bytes()),
    ])
}

fn ser(idx: usize, col: &str, name: &str, vals: &[f64]) -> String {
    let pts: String = vals.iter().enumerate().map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#)).collect();
    format!(
        r#"<c:ser><c:idx val="{idx}"/><c:order val="{idx}"/><c:tx><c:strRef><c:f>Sheet1!${col}$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>{name}</c:v></c:pt></c:strCache></c:strRef></c:tx><c:spPr><a:solidFill><a:schemeClr val="accent{}"/></a:solidFill></c:spPr><c:invertIfNegative val="0"/><c:cat><c:strRef><c:f>Sheet1!$A$2:$A$4</c:f><c:strCache><c:ptCount val="3"/><c:pt idx="0"><c:v>Q1</c:v></c:pt><c:pt idx="1"><c:v>Q2</c:v></c:pt><c:pt idx="2"><c:v>Q3</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>Sheet1!${col}$2:${col}$4</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="3"/>{pts}</c:numCache></c:numRef></c:val><c:extLst><c:ext uri="{{C3380CC4-5D6E-409C-BE32-E72D297353CC}}" xmlns:c16="http://schemas.microsoft.com/office/drawing/2014/chart"><c16:uniqueId val="{{0000000{idx}-0000-0000-0000-000000000000}}"/></c:ext></c:extLst></c:ser>"#,
        idx + 1
    )
}

pub fn chart_xml(external: bool) -> String {
    let _ = external;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="{RELS}"><c:date1904 val="0"/><c:lang val="sv-SE"/><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1400"/></a:pPr><a:r><a:rPr lang="sv-SE" sz="1800" b="1"/><a:t>Frukt</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:autoTitleDeleted val="0"/><c:plotArea><c:layout/><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>{}{}<c:gapWidth val="150"/><c:axId val="1"/><c:axId val="2"/></c:barChart><c:catAx><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/><c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="2"/><c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx><c:valAx><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="l"/><c:majorGridlines/><c:title><c:tx><c:rich><a:bodyPr rot="-5400000"/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE"/><a:t>Antal</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:numFmt formatCode="General" sourceLinked="1"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="1"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx></c:plotArea><c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend><c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/></c:chart><c:externalData r:id="rId1"><c:autoUpdate val="0"/></c:externalData></c:chartSpace>"#,
        ser(0, "B", "Äpplen", &[1.0, 2.0, 4.0]),
        ser(1, "C", "Päron", &[3.0, 4.0, 5.0])
    )
}

/// Presentationen: en bild med diagrammet (inbäddad arbetsbok, eller länkad om `external`).
pub fn deck_bytes(external: bool) -> Vec<u8> {
    let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="xlsx" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/ppt/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/></Types>"#;
    let rels = format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#);
    let pres = format!(r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rIdM"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rIdS1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#);
    let pres_rels = format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rIdM" Type="{RELS}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rIdS1" Type="{RELS}/slide" Target="slides/slide1.xml"/></Relationships>"#);
    let tree = |s: &str| format!(r#"<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{s}</p:spTree></p:cSld>"#);
    let master = format!(r#"<p:sldMaster {NS}>{}<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>"#, tree(""));
    let master_rels = format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{RELS}/theme" Target="../theme/theme1.xml"/></Relationships>"#);
    let layout = format!(r#"<p:sldLayout {NS}>{}</p:sldLayout>"#, tree(""));
    let layout_rels = format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#);
    let theme = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Prov"><a:themeElements><a:clrScheme name="Prov"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Prov"><a:majorFont><a:latin typeface="Arial"/></a:majorFont><a:minorFont><a:latin typeface="Arial"/></a:minorFont></a:fontScheme><a:fmtScheme name="Prov"><a:fillStyleLst/><a:lnStyleLst/><a:effectStyleLst/><a:bgFillStyleLst/></a:fmtScheme></a:themeElements></a:theme>"#;
    let frame = r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Diagram 3"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="914400"/><a:ext cx="5080000" cy="3810000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId2"/></a:graphicData></a:graphic></p:graphicFrame>"#;
    let slide = format!(r#"<p:sld {NS}>{}</p:sld>"#, tree(frame));
    let slide_rels = format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{RELS}/chart" Target="../charts/chart1.xml"/></Relationships>"#);
    let chart_rels = if external {
        format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/oleObject" Target="file:///C:/Data/Frukt.xlsx" TargetMode="External"/></Relationships>"#)
    } else {
        format!(r#"<Relationships xmlns="{PKG}"><Relationship Id="rId1" Type="{RELS}/package" Target="../embeddings/Microsoft_Excel_Worksheet.xlsx"/></Relationships>"#)
    };
    let wb = workbook();
    let chart = chart_xml(external);
    let mut files: Vec<(&str, &[u8])> = vec![
        ("[Content_Types].xml", ct.as_bytes()),
        ("_rels/.rels", rels.as_bytes()),
        ("ppt/presentation.xml", pres.as_bytes()),
        ("ppt/_rels/presentation.xml.rels", pres_rels.as_bytes()),
        ("ppt/slideMasters/slideMaster1.xml", master.as_bytes()),
        ("ppt/slideMasters/_rels/slideMaster1.xml.rels", master_rels.as_bytes()),
        ("ppt/slideLayouts/slideLayout1.xml", layout.as_bytes()),
        ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", layout_rels.as_bytes()),
        ("ppt/theme/theme1.xml", theme.as_bytes()),
        ("ppt/slides/slide1.xml", slide.as_bytes()),
        ("ppt/slides/_rels/slide1.xml.rels", slide_rels.as_bytes()),
        ("ppt/charts/chart1.xml", chart.as_bytes()),
        ("ppt/charts/_rels/chart1.xml.rels", chart_rels.as_bytes()),
    ];
    if !external {
        files.push(("ppt/embeddings/Microsoft_Excel_Worksheet.xlsx", &wb));
    }
    zip(&files)
}

fn entries(bytes: &[u8]) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut m = std::collections::BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        m.insert(f.name().to_owned(), b);
    }
    m
}

fn texts(scene: &super::scene::Scene) -> Vec<String> {
    scene.items.iter().filter_map(|i| if let super::scene::Item::Glyphs(g) = i { Some(g.text.trim().to_owned()) } else { None }).collect()
}

fn frame_index(deck: &Deck) -> (usize, u32) {
    let s = super::edit_ops::shapes(deck, 0).into_iter().find(|s| s.kind == super::edit_ops::Kind::Frame).expect("diagramramen");
    (s.index, s.id)
}

#[test]
fn referenser_tolkas_och_andras() {
    let r = Range::parse("Sheet1!$B$2:$B$4").unwrap();
    assert_eq!((r.c0, r.r0, r.c1, r.r1), (2, 2, 2, 4));
    assert!(r.vertical());
    assert_eq!(r.resized(5, true).format(), "Sheet1!$B$2:$B$6");
    assert_eq!(r.shifted(1, true).format(), "Sheet1!$C$2:$C$4");
    let h = Range::parse("'Mitt blad'!$B$1:$E$1").unwrap();
    assert!(!h.vertical());
    assert_eq!(h.sheet_name(), "Mitt blad");
    assert_eq!(h.resized(2, false).format(), "'Mitt blad'!$B$1:$C$1");
    assert_eq!(edit_chart::col_name(28), "AB");
    assert_eq!(edit_chart::cell("AB$12"), Some((28, 12)));
    assert_eq!(Range::parse("Sheet1!$C$1").unwrap().shifted(1, true).format(), "Sheet1!$D$1");
}

#[test]
fn diagrammets_data_lases() {
    let deck = Deck::open(&deck_bytes(false)).unwrap();
    let (index, _) = frame_index(&deck);
    assert!(edit_chart::is_chart(&deck, 0, index));
    let (part, d) = edit_chart::read_shape(&deck, 0, index).unwrap();
    assert_eq!(part, "ppt/charts/chart1.xml");
    assert_eq!(d.cats.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Q1", "Q2", "Q3"]);
    assert_eq!(d.series[1].name, "Päron");
    assert_eq!(d.series[0].vals, [Some(1.0), Some(2.0), Some(4.0)]);
    assert_eq!(d.titles[0].text.as_deref(), Some("Frukt"));
    assert!(d.titles.iter().any(|t| t.axis.as_deref() == Some("2") && t.text.as_deref() == Some("Antal")));
    assert!(d.titles.iter().any(|t| t.axis.as_deref() == Some("1") && t.text.is_none()));
}

/// Ändringarna som provas: värde, namn, kategori, ny kategori, ny serie, borttagen serie, rubriker.
fn edited(d: &edit_chart::ChartData) -> edit_chart::ChartData {
    let mut d = d.clone();
    d.series[0].vals[1] = Some(7.5);
    d.series[0].name = "Äpplen (kg)".into();
    d.cats[2].text = "Q3 rev".into();
    d.cats.push(edit_chart::Cat { text: "Q4".into(), num: None });
    for s in &mut d.series {
        s.vals.push(Some(6.0));
    }
    d.series.push(edit_chart::SeriesData { name: "Plommon".into(), vals: vec![Some(2.0), Some(3.0), None, Some(9.0)], orig: None });
    // Päron tas bort.
    d.series.remove(1);
    d.titles[0].text = Some("Frukt 2024".into());
    for t in &mut d.titles {
        if t.axis.as_deref() == Some("1") {
            t.text = Some("Kvartal".into());
        }
    }
    d
}

#[test]
fn diagramdata_tur_och_retur_med_arbetsbok() {
    let bytes = deck_bytes(false);
    let deck = Deck::open(&bytes).unwrap();
    let (index, _) = frame_index(&deck);
    let (_, d) = edit_chart::read_shape(&deck, 0, index).unwrap();
    let mut s = Session::new(bytes.clone(), &deck);
    // Oförändrad tabell: inget ångringssteg.
    assert!(s.apply(&deck, None, |t| edit_chart::write(t, 0, index, &d).map(|_| ())).unwrap().is_none());
    let new = edited(&d);
    let mut written = None;
    let a = s.apply(&deck, None, |t| {
        written = Some(edit_chart::write(t, 0, index, &new)?);
        Ok(())
    });
    let deck2 = a.unwrap().expect("ändring").deck;
    assert_eq!(written.unwrap().workbook, Some(true));
    let (out, _) = s.save_bytes(&deck2, "pptx", "pptx").unwrap();
    let errs = validate(&out);
    assert!(errs.is_empty(), "{errs:?}");

    // Diagramdelen: läses tillbaka som tabellen.
    let d3 = Deck::open(&out).unwrap();
    let (_, back) = edit_chart::read_shape(&d3, 0, index).unwrap();
    assert_eq!(back.cats.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Q1", "Q2", "Q3 rev", "Q4"]);
    assert_eq!(back.series.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Äpplen (kg)", "Plommon"]);
    assert_eq!(back.series[0].vals, [Some(1.0), Some(7.5), Some(4.0), Some(6.0)]);
    assert_eq!(back.series[1].vals, [Some(2.0), Some(3.0), None, Some(9.0)]);
    assert_eq!(back.titles[0].text.as_deref(), Some("Frukt 2024"));
    assert!(back.titles.iter().any(|t| t.axis.as_deref() == Some("1") && t.text.as_deref() == Some("Kvartal")));
    let e = entries(&out);
    let chart = String::from_utf8(e["ppt/charts/chart1.xml"].clone()).unwrap();
    // Referenserna: fyra kategorier, den nya serien en kolumn till höger om den sista (C → D).
    assert!(chart.contains("<c:f>Sheet1!$A$2:$A$5</c:f>") && chart.contains("<c:f>Sheet1!$B$2:$B$5</c:f>"), "{chart}");
    assert!(chart.contains("<c:f>Sheet1!$D$1</c:f>") && chart.contains("<c:f>Sheet1!$D$2:$D$5</c:f>"));
    // Den nya serien: nytt idx, ingen egen färg; källans extLst och rubrikens formatering står kvar.
    assert!(chart.contains(r#"<c:idx val="2"/>"#));
    assert_eq!(chart.matches("<c:spPr>").count(), 1, "bara Äpplens färg");
    assert!(chart.contains(r#"<a:rPr lang="sv-SE" sz="1800" b="1"/><a:t>Frukt 2024</a:t>"#));
    assert!(chart.contains("c16:uniqueId") && chart.contains("<c:date1904 val=\"0\"/>"));
    assert!(chart.contains(r#"<c:ptCount val="4"/>"#));

    // Arbetsboken: cellerna som referenserna pekar på.
    let wb = &e["ppt/embeddings/Microsoft_Excel_Worksheet.xlsx"];
    let cell = |c: u32, r: u32| xlsx::read_cell(wb, "Sheet1", c, r);
    assert_eq!(cell(2, 1), Some(CellVal::Str("Äpplen (kg)".into())));
    assert_eq!(cell(2, 3), Some(CellVal::Num(7.5)));
    assert_eq!(cell(1, 4), Some(CellVal::Str("Q3 rev".into())));
    assert_eq!(cell(1, 5), Some(CellVal::Str("Q4".into())));
    assert_eq!(cell(4, 1), Some(CellVal::Str("Plommon".into())));
    assert_eq!(cell(4, 5), Some(CellVal::Num(9.0)));
    assert!(matches!(cell(4, 4), None | Some(CellVal::Empty)), "tomt värde");
    let we = entries(wb);
    // Varje XML-del i arbetsboken går att tolka; formatet på B4 står kvar; tabellen växte till D5 med en
    // ny kolumn och rubriken som namn; calcChain fanns för C4 (orörd) och står kvar.
    for (n, b) in &we {
        if n.ends_with(".xml") || n.ends_with(".rels") {
            assert!(super::edit_xml::parse(b).is_some(), "{n} är inte giltig XML");
        }
    }
    let sheet = String::from_utf8(we["xl/worksheets/sheet1.xml"].clone()).unwrap();
    assert!(sheet.contains(r#"<c r="B4" s="1"><v>4</v></c>"#), "{sheet}");
    assert!(sheet.contains(r#"<dimension ref="A1:D5"/>"#));
    let table = String::from_utf8(we["xl/tables/table1.xml"].clone()).unwrap();
    assert!(table.contains(r#"ref="A1:D5""#) && table.contains(r#"name="Plommon""#) && table.contains(r#"name="Äpplen (kg)""#) && table.contains(r#"count="4""#), "{table}");
    let sst = String::from_utf8(we["xl/sharedStrings.xml"].clone()).unwrap();
    assert!(sst.contains("<t>Plommon</t>") && sst.contains(r#"uniqueCount="10""#), "{sst}");
    assert!(we.contains_key("xl/calcChain.xml"));
    openpyxl(wb, "ws=wb['Sheet1']; assert ws['D5'].value==9 and ws['B1'].value=='Äpplen (kg)' and ws.tables['Tabell1'].ref=='A1:D5', ws.tables['Tabell1'].ref");

    // Diagrammet ritas med de nya värdena (kategorin Q4, serien Plommon, rubriken).
    let t = texts(&super::render::slide_scene(&d3, 0));
    for want in ["Q4", "Plommon", "Kvartal"] {
        assert!(t.iter().any(|x| x == want), "saknar {want:?} i {t:?}");
    }
    assert!(t.iter().any(|x| x.contains("2024")));
    assert!(!t.iter().any(|x| x == "Päron"));

    // Ångra ger tillbaka originalet.
    let back = s.undo(&deck2).unwrap().unwrap();
    assert_eq!(back.pkg.read("ppt/charts/chart1.xml"), deck.pkg.read("ppt/charts/chart1.xml"));
}

#[test]
fn formel_som_skrivs_over_tar_bort_berakningskedjan() {
    let wb = workbook();
    let w = |c, r, v| xlsx::Write { sheet: "sheet1".into(), col: c, row: r, val: v };
    let out = xlsx::edit(&wb, &[w(3, 4, CellVal::Num(10.0)), w(2, 2, CellVal::Empty), w(5, 7, CellVal::Str("Ny".into()))]).unwrap();
    let e = entries(&out);
    assert!(!e.contains_key("xl/calcChain.xml"));
    let rels = String::from_utf8(e["xl/_rels/workbook.xml.rels"].clone()).unwrap();
    assert!(!rels.contains("calcChain"));
    assert!(!String::from_utf8(e["[Content_Types].xml"].clone()).unwrap().contains("calcChain"));
    assert_eq!(xlsx::read_cell(&out, "Sheet1", 3, 4), Some(CellVal::Num(10.0)));
    assert_eq!(xlsx::read_cell(&out, "Sheet1", 2, 2), Some(CellVal::Empty));
    // En ny rad (7) och cell (E7) i ordning, utan att tabellen växer (inte intill den).
    assert_eq!(xlsx::read_cell(&out, "Sheet1", 5, 7), Some(CellVal::Str("Ny".into())));
    let sheet = String::from_utf8(e["xl/worksheets/sheet1.xml"].clone()).unwrap();
    assert!(sheet.find(r#"<row r="4""#).unwrap() < sheet.find(r#"<row r="7""#).unwrap());
    assert!(String::from_utf8(e["xl/tables/table1.xml"].clone()).unwrap().contains(r#"ref="A1:C4""#));
    // Oförändrade poster kopieras byte för byte.
    assert_eq!(e["xl/workbook.xml"], entries(&wb)["xl/workbook.xml"]);
    // Med openpyxl (om det finns) går arbetsboken att öppna.
    openpyxl(&out, "ws=wb['Sheet1']; assert ws['C4'].value==10 and ws['E7'].value=='Ny', (ws['C4'].value, ws['E7'].value)");
}

/// Öppnar arbetsboken med openpyxl (om det finns) och kör `check` (Python, `wb` är arbetsboken).
fn openpyxl(xlsx: &[u8], check: &str) {
    if !std::process::Command::new("python3").args(["-c", "import openpyxl"]).status().is_ok_and(|s| s.success()) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("kladd-chart-xlsx-{}-{}", std::process::id(), check.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("prov.xlsx");
    std::fs::write(&p, xlsx).unwrap();
    let ok = std::process::Command::new("python3").args(["-c", &format!("import sys,openpyxl; wb=openpyxl.load_workbook(sys.argv[1]); {check}"), p.to_str().unwrap()]).status().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok.success(), "openpyxl kunde inte läsa arbetsboken");
}

#[test]
fn lankad_arbetsbok_ger_bara_cachen() {
    let bytes = deck_bytes(true);
    let deck = Deck::open(&bytes).unwrap();
    let (index, _) = frame_index(&deck);
    let (_, d) = edit_chart::read_shape(&deck, 0, index).unwrap();
    let mut d2 = d.clone();
    d2.series[0].vals[0] = Some(42.0);
    let mut s = Session::new(bytes, &deck);
    let mut written = None;
    let a = s.apply(&deck, None, |t| {
        written = Some(edit_chart::write(t, 0, index, &d2)?);
        Ok(())
    });
    let deck2 = a.unwrap().unwrap().deck;
    assert_eq!(written.unwrap().workbook, Some(false));
    let (out, _) = s.save_bytes(&deck2, "pptx", "pptx").unwrap();
    assert!(validate(&out).is_empty());
    let (_, back) = edit_chart::read_shape(&Deck::open(&out).unwrap(), 0, index).unwrap();
    assert_eq!(back.series[0].vals[0], Some(42.0));
}

#[test]
fn rubrik_tas_bort_och_laggs_till() {
    let bytes = deck_bytes(false);
    let deck = Deck::open(&bytes).unwrap();
    let (index, _) = frame_index(&deck);
    let (_, d) = edit_chart::read_shape(&deck, 0, index).unwrap();
    let mut d2 = d.clone();
    d2.titles[0].text = None;
    let mut s = Session::new(bytes, &deck);
    let deck2 = s.apply(&deck, None, |t| edit_chart::write(t, 0, index, &d2).map(|_| ())).unwrap().unwrap().deck;
    let chart = String::from_utf8(deck2.pkg.read("ppt/charts/chart1.xml").unwrap().to_vec()).unwrap();
    assert!(!chart.contains("<c:title><c:tx><c:rich><a:bodyPr/>") && chart.contains(r#"<c:autoTitleDeleted val="1"/>"#));
    let (_, back) = edit_chart::read_shape(&deck2, 0, index).unwrap();
    assert!(back.titles[0].text.is_none());
    // Och tillbaka: en ny rubrik.
    let mut d3 = back.clone();
    d3.titles[0].text = Some("Ny rubrik".into());
    let deck3 = s.apply(&deck2, None, |t| edit_chart::write(t, 0, index, &d3).map(|_| ())).unwrap().unwrap().deck;
    let (_, back) = edit_chart::read_shape(&deck3, 0, index).unwrap();
    assert_eq!(back.titles[0].text.as_deref(), Some("Ny rubrik"));
    let (out, _) = s.save_bytes(&deck3, "pptx", "pptx").unwrap();
    assert!(validate(&out).is_empty());
}

/// Diagrammen i materialet: varje diagram läses, får ett värde ändrat, en kategori och en serie till,
/// sparas (giltigt) och ritas. `KLADD_PPTX_SWEEP=mapp cargo test redigera_alla_diagram -- --ignored`.
#[test]
#[ignore]
fn redigera_alla_diagram() {
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
    let (mut charts, mut workbooks, mut problems) = (0, 0, Vec::new());
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else { continue };
        let Ok(deck) = Deck::open(&bytes) else { continue };
        let pre = validate(&bytes);
        for slide in 0..deck.slides.len() {
            for sh in super::edit_ops::shapes(&deck, slide).into_iter().filter(|s| s.kind == super::edit_ops::Kind::Frame) {
                let Some((_, d)) = edit_chart::read_shape(&deck, slide, sh.index) else { continue };
                charts += 1;
                let mut d2 = d.clone();
                if let Some(v) = d2.series.first_mut().and_then(|s| s.vals.first_mut()) {
                    *v = Some(v.unwrap_or(0.0) + 1.0);
                }
                d2.cats.push(edit_chart::Cat { text: "Ny".into(), num: None });
                for s in &mut d2.series {
                    s.vals.push(Some(1.0));
                }
                d2.series.push(edit_chart::SeriesData { name: "Ny serie".into(), vals: vec![Some(2.0); d2.cats.len()], orig: None });
                let name = format!("{} bild {}", f.display(), slide + 1);
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<bool, String> {
                    let mut s = Session::new(bytes.clone(), &deck);
                    let mut w = None;
                    let a = s.apply(&deck, None, |t| {
                        w = Some(edit_chart::write(t, slide, sh.index, &d2)?);
                        Ok(())
                    })?;
                    let deck2 = a.ok_or("ingen ändring")?.deck;
                    let (out, _) = s.save_bytes(&deck2, "pptx", "pptx")?;
                    let errs: Vec<String> = validate(&out).into_iter().filter(|e| !pre.contains(e)).collect();
                    if !errs.is_empty() {
                        return Err(format!("{errs:?}"));
                    }
                    let d3 = Deck::open(&out)?;
                    let (_, back) = edit_chart::read_shape(&d3, slide, sh.index).ok_or("diagrammet kan inte läsas igen")?;
                    if back.series.len() != d2.series.len() || back.cats.len() != d2.cats.len() {
                        return Err(format!("{} serier / {} kategorier efter sparning", back.series.len(), back.cats.len()));
                    }
                    let _ = super::render_slide(&d3, slide, 160, 90);
                    // `KLADD_CHART_XLSX_OUT=mapp` sparar de ändrade arbetsböckerna (för openpyxl/Excel).
                    if let (Some(dir), true) = (std::env::var_os("KLADD_CHART_XLSX_OUT"), w.as_ref().is_some_and(|w| w.workbook == Some(true))) {
                        let part = edit_chart::chart_part(&d3, slide, sh.index).unwrap_or_default();
                        if let Some(r) = d3.pkg.rels(&part).into_iter().find(|r| r.target.ends_with(".xlsx")) {
                            let _ = std::fs::create_dir_all(&dir);
                            let n = format!("{}-{}-{}.xlsx", f.parent().and_then(|p| p.file_name()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), slide + 1, sh.index);
                            let _ = std::fs::write(std::path::Path::new(&dir).join(n), d3.pkg.read(&r.target).unwrap_or_default());
                        }
                    }
                    Ok(w.is_some_and(|w| w.workbook == Some(true)))
                }));
                match r {
                    Ok(Ok(wb)) => workbooks += usize::from(wb),
                    Ok(Err(e)) => problems.push(format!("{name}: {e}")),
                    Err(_) => problems.push(format!("{name}: panik")),
                }
            }
        }
    }
    println!("{charts} diagram, {workbooks} med inbäddad arbetsbok uppdaterad, {} problem", problems.len());
    for p in &problems {
        println!("  {p}");
    }
    assert!(problems.is_empty());
}
