//! Tester för avsnitt i Word: `w:sectPr` i stycken och brödtextens `w:sectPr` ↔ avsnittsrader.
use super::*;
use crate::word::sections::{self, SectionProps};

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

const P_ONE: &str = r#"<w:p><w:r><w:t>Första avsnittet</w:t></w:r></w:p>"#;
/// Slut på avsnitt 1 (stående, med sidhuvud): ett tomt stycke.
const END_ONE: &str = r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId10"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1134" w:bottom="1417" w:left="1134" w:header="708" w:footer="708" w:gutter="0"/><w:cols w:space="708"/></w:sectPr></w:pPr></w:p>"#;
const P_TWO: &str = r#"<w:p><w:r><w:t>Liggande med spalter</w:t></w:r></w:p>"#;
/// Slut på avsnitt 2: liggande, två spalter, i ett stycke med text.
const END_TWO: &str = r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/><w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134" w:header="708" w:footer="708" w:gutter="0"/><w:cols w:num="2" w:space="567"/></w:sectPr></w:pPr><w:r><w:t>Slut på avsnitt två</w:t></w:r></w:p>"#;
const P_THREE: &str = r#"<w:p><w:r><w:t>Sista avsnittet</w:t></w:r></w:p>"#;
/// Brödtextens sectPr: avsnitt 3, stående, löpande.
const BODY_SECT: &str = r#"<w:sectPr><w:type w:val="continuous"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1134" w:bottom="1417" w:left="1134" w:header="708" w:footer="708" w:gutter="0"/><w:cols w:space="708"/></w:sectPr>"#;
const HEADER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t>Rapport</w:t></w:r></w:p></w:hdr>"#;

fn sectioned_docx() -> Vec<u8> {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{P_ONE}{END_ONE}{P_TWO}{END_TWO}{P_THREE}{BODY_SECT}</w:body></w:document>"#
    );
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/></Relationships>"#;
    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let parts: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", types.to_owned()),
        ("_rels/.rels", root.to_owned()),
        ("word/document.xml", doc),
        ("word/_rels/document.xml.rels", rels.to_owned()),
        ("word/header1.xml", HEADER_XML.to_owned()),
    ];
    write_zip(parts.into_iter().map(|(n, c)| (n.to_owned(), c.into_bytes())).collect()).unwrap()
}

fn part_text(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut out).unwrap();
    out
}

#[test]
fn sections_are_read_as_marker_lines() {
    let l = read(&sectioned_docx()).unwrap();
    assert!(l.dropped.is_empty());
    let md = &l.markdown;
    let marks: Vec<&str> = md.lines().filter(|l| l.starts_with("<!-- kladd: avsnitt")).collect();
    assert_eq!(marks.len(), 2, "{md}");
    // Avsnitt 2: liggande A4, två spalter med 10 mm mellanrum (567 twips); avsnitt 3: löpande.
    let two = SectionProps::from_comment(marks[0]).unwrap();
    assert_eq!(two.page_size.map(|(w, h)| (w.round(), h.round())), Some((842.0, 595.0)));
    assert_eq!(two.columns, Some(2));
    assert!((two.column_gap.unwrap() - 28.35).abs() < 0.01);
    assert!(two.page_margin.is_some());
    assert!(!two.continuous);
    let three = SectionProps::from_comment(marks[1]).unwrap();
    assert!(three.continuous);
    assert_eq!(three.page_size, None);
    assert_eq!(three.columns, None);
    // Ordningen: raden för avsnitt 2 kommer efter första avsnittets text, före avsnitt 2:s.
    let at = |s: &str| md.find(s).unwrap_or_else(|| panic!("{s} saknas i\n{md}"));
    assert!(at("Första avsnittet") < at(marks[0]) && at(marks[0]) < at("Liggande med spalter"));
    assert!(at("Slut på avsnitt två") < at(marks[1]) && at(marks[1]) < at("Sista avsnittet"));
    // Grundformatet kommer från första avsnittet (sidhuvudet), inte från det sista.
    let d = DocDefaults::from_comment(md.lines().next().unwrap()).expect("grundformat");
    assert_eq!(d.header.as_deref(), Some("Rapport"));
    // Avsnitten i analysen stämmer.
    let a = markup::analyze(md);
    assert_eq!(a.sections.len(), 3);
    assert!(a.sections[1].landscape() && a.sections[1].columns == 2);
}

#[test]
fn untouched_sections_roundtrip_losslessly() {
    let l = read(&sectioned_docx()).unwrap();
    let out = write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let doc = part_text(&out, "word/document.xml");
    for p in [P_ONE, END_ONE, P_TWO, END_TWO, P_THREE, BODY_SECT] {
        assert!(doc.contains(p), "saknas: {p}\n{doc}");
    }
    assert_eq!(part_text(&out, "word/header1.xml"), HEADER_XML);
    assert_eq!(read(&out).unwrap().markdown, l.markdown);
}

#[test]
fn editing_a_marker_rewrites_only_that_section() {
    let l = read(&sectioned_docx()).unwrap();
    let md = l.markdown.replace("columns: 2", "columns: 3").replace("Sista avsnittet", "Sista stycket");
    assert_ne!(md, l.markdown);
    let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let doc = part_text(&out, "word/document.xml");
    // Avsnitt 2 (slutet av stycket med text) fick tre spalter men behöll sin sida och sitt stycke.
    assert!(doc.contains(r#"w:num="3""#) && !doc.contains(r#"w:num="2""#), "{doc}");
    assert!(doc.contains(r#"w:orient="landscape""#));
    assert!(doc.contains("Slut på avsnitt två"));
    // Övriga avsnitt är orörda.
    assert!(doc.contains(END_ONE) && doc.contains(BODY_SECT), "{doc}");
    assert!(doc.contains("Sista stycket"));
    let again = read(&out).unwrap();
    assert_eq!(again.markdown, md);
}

#[test]
fn removing_and_adding_sections() {
    let l = read(&sectioned_docx()).unwrap();
    // Tar bort avsnittsraden för avsnitt 2: bara en sectPr i ett stycke finns kvar.
    let first_mark = l.markdown.lines().find(|l| l.starts_with("<!-- kladd: avsnitt; page")).unwrap().to_owned();
    let md = l.markdown.replace(&format!("{first_mark}\n\n"), "");
    let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let doc = part_text(&out, "word/document.xml");
    assert_eq!(doc.matches("<w:sectPr").count(), 2, "{doc}");
    assert!(!doc.contains(r#"w:orient="landscape""#));
    assert_eq!(read(&out).unwrap().markdown, md);
    // Lägger till en brytning i ett dokument utan avsnitt: första avsnittet behåller sidhuvudet.
    let plain = read(&write("<!-- kladd: header: \"Rubrik\" -->\n\nEtt\n\nTvå\n", "x", &|_| None, None).unwrap().bytes).unwrap();
    let md = "<!-- kladd: header: \"Rubrik\" -->\n\nEtt\n\n<!-- kladd: avsnitt; page: A4 landscape; columns: 2 -->\n\nTvå\n";
    let w = write(md, "x", &|_| None, plain.source.as_ref()).unwrap();
    let doc = part_text(&w.bytes, "word/document.xml");
    assert_eq!(doc.matches("<w:sectPr").count(), 2, "{doc}");
    let first = &doc[doc.find("<w:sectPr").unwrap()..doc.find("</w:sectPr>").unwrap()];
    assert!(first.contains("headerReference") && first.contains(r#"w:w="11906""#), "{first}");
    let last = &doc[doc.rfind("<w:sectPr").unwrap()..];
    assert!(last.contains(r#"w:w="16838""#) && last.contains(r#"w:orient="landscape""#) && last.contains(r#"w:num="2""#), "{last}");
}

#[test]
fn fresh_write_makes_sections_that_read_back() {
    let md = "Ett\n\n<!-- kladd: avsnitt; page: A5; page-margin: 15mm; columns: 3; column-gap: 8mm -->\n\nTvå\n\n<!-- kladd: spaltbrytning -->\n\nTre\n\n<!-- kladd: avsnitt; type: continuous -->\n\nFyra\n";
    let w = write(md, "x", &|_| None, None).unwrap();
    let doc = part_text(&w.bytes, "word/document.xml");
    assert_eq!(doc.matches("<w:sectPr").count(), 3, "{doc}");
    assert!(doc.contains(r#"<w:br w:type="column"/>"#));
    assert!(doc.contains(r#"w:num="3""#) && doc.contains(r#"<w:type w:val="continuous"/>"#), "{doc}");
    let back = read(&w.bytes).unwrap();
    let lines: Vec<&str> = back.markdown.lines().filter(|l| l.starts_with("<!-- kladd:")).collect();
    assert_eq!(lines.len(), 3, "{}", back.markdown);
    let p = SectionProps::from_comment(lines[0]).unwrap();
    assert_eq!(p.columns, Some(3));
    assert_eq!(p.page_size.map(|(w, _)| w.round()), Some(420.0));
    assert!(sections::is_column_break(lines[1]));
    assert!(SectionProps::from_comment(lines[2]).unwrap().continuous);
}

#[test]
fn leading_marker_sets_the_first_section_without_a_break() {
    let md = "<!-- kladd: avsnitt; columns: 2 -->\n\nEtt\n\nTvå\n";
    let w = write(md, "x", &|_| None, None).unwrap();
    let doc = part_text(&w.bytes, "word/document.xml");
    assert_eq!(doc.matches("<w:sectPr").count(), 1, "{doc}");
    assert!(doc.contains(r#"<w:cols w:num="2""#), "{doc}");
}

#[test]
fn first_page_header_refs_follow_the_first_section() {
    let md = "<!-- kladd: header: \"Rapport|{titel}|{sida}\"; header-first: \"Omslag\" -->\n\nEtt\n\n<!-- kladd: avsnitt; page: A4 landscape; columns: 2 -->\n\nTvå\n";
    let w = write(md, "t.docx", &|_| None, None).unwrap();
    let doc = part_text(&w.bytes, "word/document.xml");
    // Första avsnittet bär referenserna och titlePg; brödtextens avsnitt ärver (ingen titlePg).
    assert_eq!(doc.matches("<w:titlePg/>").count(), 1, "{doc}");
    assert!(doc.find("w:type=\"first\"").unwrap() < doc.find("w:orient=\"landscape\"").unwrap(), "{doc}");
    let back = read(&w.bytes).unwrap();
    assert!(back.markdown.contains("header-first"), "{}", back.markdown);
    assert!(back.markdown.contains("columns: 2"), "{}", back.markdown);
    // Ett oförändrat omläst dokument sparas likadant i dokumentdelen.
    let again = write(&back.markdown, "t.docx", &|_| None, back.source.as_ref()).unwrap();
    assert_eq!(part_text(&again.bytes, "word/document.xml"), doc);
}
