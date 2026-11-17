//! Tester för styckeutseende i Word: ramar, skuggning, avstånd och indrag (`docx_para`, `word::pbox`).
use super::*;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Tajt"><w:name w:val="Tajt"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="exact"/><w:ind w:left="720" w:hanging="360"/></w:pPr></w:style></w:styles>"#;

const BOX: &str = r#"<w:pBdr><w:top w:val="single" w:sz="8" w:space="4" w:color="6E4BAA"/><w:left w:val="single" w:sz="8" w:space="4" w:color="6E4BAA"/><w:bottom w:val="single" w:sz="8" w:space="4" w:color="6E4BAA"/><w:right w:val="single" w:sz="8" w:space="4" w:color="6E4BAA"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="EDE7F6"/><w:spacing w:after="0"/>"#;

fn body() -> String {
    format!(
        r#"<w:p><w:r><w:t>Vanligt stycke</w:t></w:r></w:p><w:p><w:pPr>{BOX}</w:pPr><w:r><w:t>Rutans första rad</w:t></w:r></w:p><w:p><w:pPr>{BOX}</w:pPr><w:r><w:t>Rutans andra rad</w:t></w:r></w:p><w:p><w:pPr><w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="2E7D32"/></w:pBdr><w:spacing w:before="60" w:after="160"/><w:ind w:left="215"/></w:pPr><w:r><w:t>Staplad kommentar</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Tajt"/></w:pPr><w:r><w:t>Tajt mall</w:t></w:r></w:p><w:p><w:r><w:t>Slut</w:t></w:r></w:p>"#
    )
}

fn docx_with(body: &str) -> Vec<u8> {
    let doc = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{body}</w:body></w:document>"#);
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let parts: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", types.to_owned()),
        ("_rels/.rels", root.to_owned()),
        ("word/document.xml", doc),
        ("word/_rels/document.xml.rels", rels.to_owned()),
        ("word/styles.xml", STYLES.to_owned()),
    ];
    write_zip(parts.into_iter().map(|(n, c)| (n.to_owned(), c.into_bytes())).collect()).unwrap()
}

fn part_text(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut out).unwrap();
    out
}

fn md_body(md: &str) -> &str {
    if md.starts_with("<!-- kladd:") { md.split_once("-->\n\n").map_or(md, |x| x.1) } else { md }
}

#[test]
fn read_gives_markers_for_boxes_bars_and_styles_only() {
    let l = read(&docx_with(&body())).unwrap();
    let md = md_body(&l.markdown);
    let lines: Vec<&str> = md.lines().filter(|x| !x.is_empty()).collect();
    // Vanligt stycke: docDefaults 8 pt efter, 1,08 – layoutens eget avstånd, ingen markör.
    assert_eq!(lines[0], "Vanligt stycke", "{md}");
    // Rutan: ram + skuggning + avstånd efter 0 (före 0, radavstånd som standard).
    let boxm = "<!--k:pp bdr=\"t=single:8:4:6E4BAA l=single:8:4:6E4BAA b=single:8:4:6E4BAA r=single:8:4:6E4BAA\" shd=\"EDE7F6\" sp=\"b=0 a=0\"-->";
    assert_eq!(lines[1], format!("Rutans första rad{boxm}"), "{md}");
    assert_eq!(lines[2], format!("Rutans andra rad{boxm}"), "{md}");
    // Stapeln: vänsterkant, avstånd 3/8 pt och indrag 10,75 pt.
    assert_eq!(lines[3], "Staplad kommentar<!--k:pp bdr=\"l=single:24:8:2E7D32\" sp=\"b=3 a=8\" ind=\"l=10.75 r=0 f=0\"-->", "{md}");
    // Mallen: exakt radavstånd 12 pt, hängande indrag.
    assert_eq!(lines[4], "Tajt mall<!--k:pp sp=\"b=0 a=0 l=exact:12\" ind=\"l=36 r=0 f=-18\"-->", "{md}");
    assert_eq!(lines[5], "Slut");
}

#[test]
fn untouched_document_round_trips_losslessly() {
    let bytes = docx_with(&body());
    let l = read(&bytes).unwrap();
    let w = write(&l.markdown, "t", &|_| None, l.source.as_ref()).unwrap();
    // Styckena är byte för byte desamma (Kladd lägger bara till sidmått och namnrymd i hela dokumentet).
    let paras = |x: String| x[x.find("<w:body>").unwrap()..x.find("<w:sectPr>").unwrap_or(x.len())].to_owned();
    let (a, b) = (paras(part_text(&w.bytes, "word/document.xml")), paras(part_text(&bytes, "word/document.xml")));
    assert_eq!(a, b.replace("</w:body></w:document>", ""));
    let again = read(&w.bytes).unwrap();
    assert_eq!(again.markdown, l.markdown);
}

#[test]
fn edited_box_paragraph_keeps_its_border_and_shading() {
    let bytes = docx_with(&body());
    let l = read(&bytes).unwrap();
    let md = l.markdown.replace("Rutans andra rad", "Rutans ändrade rad");
    let w = write(&md, "t", &|_| None, l.source.as_ref()).unwrap();
    let xml = part_text(&w.bytes, "word/document.xml");
    assert!(xml.contains("Rutans ändrade rad") && !xml.contains("Rutans andra rad"), "{xml}");
    assert_eq!(xml.matches("<w:pBdr><w:top").count(), 2, "{xml}");
    assert_eq!(xml.matches(r#"w:fill="EDE7F6""#).count(), 2, "{xml}");
    assert_eq!(read(&w.bytes).unwrap().markdown, md);
}

#[test]
fn new_paragraphs_with_a_marker_write_ppr_in_schema_order() {
    let pp = crate::word::pbox::PPr {
        borders: crate::word::pbox::Borders {
            left: Some(crate::word::pbox::Border { style: crate::word::pbox::LineStyle::Double, sz: 12, space: 6, color: Some([0x11, 0x22, 0x33]) }),
            ..Default::default()
        },
        shd: Some([0xFF, 0xEE, 0xDD]),
        before: Some(2.0),
        after: Some(6.0),
        line: Some(crate::word::pbox::LineRule::AtLeast(18.0)),
        ctx: Some(true),
        left: Some(36.0),
        right: Some(0.0),
        first: Some(-18.0),
    };
    let md = format!("# Rubrik{}\n\nNytt stycke{}\n", crate::word::pbox::PPr { shd: Some([1, 2, 3]), ..Default::default() }.marker(), pp.marker());
    let w = write(&md, "t", &|_| None, None).unwrap();
    let xml = part_text(&w.bytes, "word/document.xml");
    let order: Vec<usize> = ["w:pBdr", "w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"FFEEDD\"", "w:spacing w:before=\"40\" w:after=\"120\" w:line=\"360\" w:lineRule=\"atLeast\"", "w:ind w:left=\"720\" w:right=\"0\" w:hanging=\"360\"", "w:contextualSpacing"]
        .iter()
        .map(|k| xml.find(k).unwrap_or_else(|| panic!("saknar {k}: {xml}")))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{xml}");
    assert!(xml.contains(r#"<w:left w:val="double" w:sz="12" w:space="6" w:color="112233"/>"#), "{xml}");
    // Rubriken behåller sin mall och får skuggningen.
    assert!(xml.contains(r#"<w:pStyle w:val="Heading1"/><w:shd w:val="clear" w:color="auto" w:fill="010203"/>"#), "{xml}");
    // Tillbaka: samma markörer, och inga markörer som text.
    let back = read(&w.bytes).unwrap();
    assert!(!xml.contains("k:pp"));
    let lines: Vec<&str> = md_body(&back.markdown).lines().filter(|x| !x.is_empty()).collect();
    assert_eq!(lines[1], format!("Nytt stycke{}", pp.marker()), "{}", back.markdown);
}

const EMPTIES: &str = r#"<w:p><w:r><w:t>Före</w:t></w:r></w:p><w:p><w:pPr><w:spacing w:after="0"/><w:rPr><w:sz w:val="40"/></w:rPr></w:pPr></w:p><w:p/><w:p><w:r><w:t>Efter</w:t></w:r></w:p><w:p/>"#;

#[test]
fn empty_paragraphs_are_blank_lines_that_round_trip() {
    let bytes = docx_with(EMPTIES);
    let l = read(&bytes).unwrap();
    let md = md_body(&l.markdown);
    // Stycketecknets storlek och avstånd följer med; `<w:p/>` är ett tomt stycke; slutstycket efter
    // sista texten är också ett (Word visar det).
    assert_eq!(md, "Före\n\n<!--k:ep sz=\"20\" sp=\"b=0 a=0\"-->\n\n<!--k:ep-->\n\nEfter\n\n<!--k:ep-->\n", "{md}");
    let w = write(&l.markdown, "t", &|_| None, l.source.as_ref()).unwrap();
    let body = |x: String| x[x.find("<w:body>").unwrap()..x.find("<w:sectPr>").unwrap_or(x.len())].to_owned();
    assert_eq!(body(part_text(&w.bytes, "word/document.xml")), body(part_text(&bytes, "word/document.xml")).replace("</w:body></w:document>", ""));
    assert_eq!(read(&w.bytes).unwrap().markdown, l.markdown);
}

#[test]
fn deleting_and_adding_empty_paragraphs_writes_sensible_xml() {
    let l = read(&docx_with(EMPTIES)).unwrap();
    // Ta bort den första: bara den försvinner.
    let md = l.markdown.replacen("<!--k:ep sz=\"20\" sp=\"b=0 a=0\"-->\n\n", "", 1);
    let xml = part_text(&write(&md, "t", &|_| None, l.source.as_ref()).unwrap().bytes, "word/document.xml");
    assert!(!xml.contains(r#"w:sz w:val="40""#), "{xml}");
    assert_eq!(xml.matches("<w:p/>").count(), 2, "{xml}");
    // Lägg till en ny (med eget avstånd och storlek): ett riktigt tomt stycke.
    let md = l.markdown.replacen("Efter", "<!--k:ep sz=\"14\" sp=\"b=6 a=12\"-->\n\nEfter", 1);
    let xml = part_text(&write(&md, "t", &|_| None, l.source.as_ref()).unwrap().bytes, "word/document.xml");
    assert!(xml.contains(r#"<w:p><w:pPr><w:spacing w:before="120" w:after="240"/><w:rPr><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:pPr></w:p>"#), "{xml}");
    assert!(!xml.contains("k:ep"));
    // Helt nytt dokument: markören blir ett tomt stycke mellan texterna.
    let xml = part_text(&write("A\n\n<!--k:ep-->\n\nB\n", "t", &|_| None, None).unwrap().bytes, "word/document.xml");
    assert!(xml.contains("<w:p/>") && xml.find(">A</w:t>").unwrap() < xml.find("<w:p/>").unwrap() && xml.find("<w:p/>").unwrap() < xml.find(">B</w:t>").unwrap(), "{xml}");
}
