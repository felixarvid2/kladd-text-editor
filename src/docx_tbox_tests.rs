//! Tester för textrutor i Word: läsning, orörd sparning och ändrad text (`docx_tbox`).
use super::*;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:v="urn:schemas-microsoft-com:vml""#;

const BOX_RUN: &str = r#"<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:align>center</wp:align></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="2540000" cy="1270000"/><wp:wrapNone/><wp:docPr id="3" name="Textruta 3"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2540000" cy="1270000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FFF2CC"/></a:solidFill><a:ln w="9525"><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Faktaruta</w:t></w:r></w:p><w:p><w:r><w:t>Text i rutan</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr rot="0" lIns="91440" tIns="45720" rIns="91440" bIns="45720"/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict><v:rect><v:textbox><w:txbxContent><w:p><w:r><w:t>Faktaruta</w:t></w:r></w:p><w:p><w:r><w:t>Text i rutan</w:t></w:r></w:p></w:txbxContent></v:textbox></v:rect></w:pict></mc:Fallback></mc:AlternateContent></w:r>"#;

fn docx_with(body: &str) -> Vec<u8> {
    let doc = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{body}</w:body></w:document>"#);
    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let parts: Vec<(&str, String)> = vec![("[Content_Types].xml", types.to_owned()), ("_rels/.rels", root.to_owned()), ("word/document.xml", doc)];
    write_zip(parts.into_iter().map(|(n, c)| (n.to_owned(), c.into_bytes())).collect()).unwrap()
}

fn part_text(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut out).unwrap();
    out
}

fn doc() -> Vec<u8> {
    docx_with(&format!("<w:p><w:r><w:t>Före</w:t></w:r></w:p><w:p>{BOX_RUN}<w:r><w:t>Efter rutan</w:t></w:r></w:p><w:p><w:r><w:t>Slut</w:t></w:r></w:p>"))
}

#[test]
fn box_text_is_shown_as_a_framed_block() {
    let l = read(&doc()).unwrap();
    let md = &l.markdown;
    assert!(md.contains("<!--k:tb 0-->") && md.contains("<!--k:/tb-->"), "{md}");
    let block = md.split("<!--k:tb 0-->").nth(1).unwrap().split("<!--k:/tb-->").next().unwrap();
    assert!(block.contains("**Faktaruta**") && block.contains("Text i rutan"), "{md}");
    // Fyllning och ram som styckemarkörer, och rutan ligger före stycket den sitter i.
    assert!(block.contains("shd=\"FFF2CC\"") && block.contains("bdr="), "{md}");
    assert!(md.find("k:tb 0").unwrap() < md.find("Efter rutan").unwrap());
    // Enheterna är oförändrade: rutans stycken blir inga egna.
    assert_eq!(l.source.as_ref().unwrap().elems.iter().map(|e| e.units.len()).sum::<usize>(), 3);
}

#[test]
fn untouched_box_is_saved_byte_for_byte() {
    let bytes = doc();
    let l = read(&bytes).unwrap();
    let out = write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    assert_eq!(part_text(&out, "word/document.xml").contains(BOX_RUN), true);
    assert_eq!(part_text(&out, "word/document.xml").matches("Faktaruta").count(), 2);
}

#[test]
fn edited_box_text_replaces_content_in_both_branches() {
    let l = read(&doc()).unwrap();
    let md = l.markdown.replace("Text i rutan", "Ny text i rutan");
    let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let x = part_text(&out, "word/document.xml");
    assert_eq!(x.matches("Ny text i rutan").count(), 2, "{x}");
    assert!(!x.contains(">Text i rutan<"));
    // Form, fyllning och läge bevaras, och ramens markör hamnar inte i rutans stycken.
    assert!(x.contains("FFF2CC") && x.contains("wp:anchor") && !x.contains("w:pBdr"), "{x}");
    // Texten utanför rutan är orörd, och det går att läsa tillbaka.
    assert!(x.contains("Efter rutan") && x.contains("Slut"));
    let back = read(&out).unwrap();
    assert!(back.markdown.contains("Ny text i rutan") && back.markdown.contains("**Faktaruta**"), "{}", back.markdown);
}

#[test]
fn box_without_source_becomes_plain_paragraphs() {
    let l = read(&doc()).unwrap();
    let out = write(&l.markdown, "x", &|_| None, None).unwrap().bytes;
    let x = part_text(&out, "word/document.xml");
    assert!(x.contains("Faktaruta") && x.contains("Text i rutan") && !x.contains("k:tb"), "{x}");
}
