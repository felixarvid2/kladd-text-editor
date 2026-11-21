//! Tester för textrutor i Word: läsning, orörd sparning och ändrad text (`docx_tbox`).
use super::*;
use crate::word::anchor::Pos;

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
    assert!(md.contains("<!--k:tb 0 ") && md.contains("<!--k:/tb-->"), "{md}");
    let block = md.split("<!--k:tb 0 ").nth(1).unwrap().split("<!--k:/tb-->").next().unwrap();
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

#[test]
fn inserted_box_is_written_as_a_real_text_box() {
    let block = docx_tbox::new_block();
    let md = format!("Före\n\n{}\n\nEfter\n", block.replace("Text<!--k:pp", "Ny **ruta**<!--k:pp"));
    // Utan original.
    let out = write(&md, "x", &|_| None, None).unwrap().bytes;
    let x = part_text(&out, "word/document.xml");
    assert!(x.contains("<wps:txbx><w:txbxContent>") && x.contains("Ny ") && x.contains("<wp:wrapTopAndBottom/>"), "{x}");
    assert!(x.contains("w:pBdr") == false && !x.contains("k:tb"), "{x}");
    let back = read(&out).unwrap();
    assert!(back.markdown.contains("<!--k:tb ") && back.markdown.contains("**ruta**"), "{}", back.markdown);
    // Med original: den nya rutan hamnar mellan styckena och den gamla rutan är orörd.
    let l = read(&doc()).unwrap();
    let md = l.markdown.replace("Slut", &format!("Slut\n\n{}", block));
    let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let x = part_text(&out, "word/document.xml");
    assert!(x.contains(BOX_RUN), "den gamla rutan ska vara orörd");
    assert_eq!(x.matches("<wps:txbx>").count(), 2, "{x}"); // gamla (Choice) + nya
    let back = read(&out).unwrap();
    assert_eq!(back.markdown.matches("<!--k:tb ").count(), 2, "{}", back.markdown);
    assert!(back.markdown.find("Slut").unwrap() < back.markdown.rfind("k:tb").unwrap());
}

#[test]
fn anchored_box_has_geometry_in_the_open_tag() {
    let l = read(&doc()).unwrap();
    let open = l.markdown.lines().find(|l| l.starts_with("<!--k:tb ")).unwrap();
    let (n, h) = docx_tbox::parse_head(open).unwrap();
    assert_eq!(n, 0);
    // 2540000 x 1270000 EMU = 266,7 x 133,3 px; centrerad i kolumnen, framför texten (`wrapNone`).
    assert!((h.w - 266.7).abs() < 0.2 && (h.h - 133.3).abs() < 0.2, "{open}");
    let a = h.anchor.unwrap();
    assert_eq!((a.h, a.wrap), (Pos::Center, Wrap::Front), "{open}");
    // Ramen har samma bredd och den fasta höjden (pt).
    assert!(l.markdown.contains("fr=\"w=200 x=center") && l.markdown.contains("h=100"), "{}", l.markdown);
}

#[test]
fn moved_and_resized_box_is_written_back() {
    let l = read(&doc()).unwrap();
    let open = l.markdown.lines().find(|l| l.starts_with("<!--k:tb ")).unwrap().to_owned();
    let (_, mut head) = docx_tbox::parse_head(&open).unwrap();
    head.w = 300.0;
    head.h = 150.0;
    let mut a = head.anchor.unwrap();
    a.hrel = HRel::Column;
    a.h = Pos::Off(40.0);
    a.vrel = VRel::Paragraph;
    a.v = Pos::Off(12.0);
    head.anchor = Some(a);
    let md = l.markdown.replace(&open, &docx_tbox::open_tag_head(0, &head));
    let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let x = part_text(&out, "word/document.xml");
    assert!(x.contains("<wp:positionH relativeFrom=\"column\"><wp:posOffset>381000</wp:posOffset></wp:positionH>"), "{x}");
    assert!(x.contains("<wp:positionV relativeFrom=\"paragraph\"><wp:posOffset>114300</wp:posOffset></wp:positionV>"), "{x}");
    assert!(x.contains("<wp:extent cx=\"2857500\" cy=\"1428750\"/>") && x.contains("<a:ext cx=\"2857500\" cy=\"1428750\"/>"), "{x}");
    // Fyllning och text bevaras, och läget läses tillbaka.
    assert!(x.contains("FFF2CC") && x.contains("Text i rutan"));
    let back = read(&out).unwrap();
    let open2 = back.markdown.lines().find(|l| l.starts_with("<!--k:tb ")).unwrap();
    let (_, h2) = docx_tbox::parse_head(open2).unwrap();
    assert!((h2.w - 300.0).abs() < 0.2 && (h2.h - 150.0).abs() < 0.2, "{open2}");
    assert_eq!((h2.anchor.unwrap().h, h2.anchor.unwrap().v), (Pos::Off(40.0), Pos::Off(12.0)), "{open2}");
}

#[test]
fn box_in_heading_list_and_table_cell_is_shown() {
    let inner = BOX_RUN.replace("Faktaruta", "Rubrikruta");
    let tbl = format!("<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"4000\"/></w:tblGrid><w:tr><w:tc><w:p>{BOX_RUN}<w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>");
    let body = format!(
        "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>{inner}<w:r><w:t>Rubrik</w:t></w:r></w:p><w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr>{BOX_RUN}<w:r><w:t>Punkt</w:t></w:r></w:p>{tbl}<w:p/>"
    );
    let l = read(&docx_with(&body)).unwrap();
    let md = &l.markdown;
    assert_eq!(md.matches("<!--k:tb ").count(), 3, "{md}");
    assert!(md.contains("Rubrikruta") && md.contains("Faktaruta"), "{md}");
    // Rutan i cellen ligger ovanför tabellen (i flödet), före tabellraden.
    let tb = md.rfind("<!--k:tb ").unwrap();
    assert!(tb < md.find("| Cell").unwrap_or(usize::MAX) || !md.contains("Cell"), "{md}");
    let last_open = md.lines().filter(|l| l.starts_with("<!--k:tb ")).last().unwrap();
    assert!(docx_tbox::parse_head(last_open).unwrap().1.anchor.is_none(), "{last_open}");
    // Orörda rutor sparas som förut.
    let out = write(md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    assert_eq!(part_text(&out, "word/document.xml").matches("<wps:txbx>").count(), 3);
}

#[test]
fn box_with_footnote_reference_shows_its_text() {
    let run = BOX_RUN.replace("<w:r><w:t>Text i rutan</w:t></w:r>", "<w:r><w:t>Text i rutan</w:t></w:r><w:r><w:footnoteReference w:id=\"2\"/></w:r>");
    let l = read(&docx_with(&format!("<w:p>{run}<w:r><w:t>Efter</w:t></w:r></w:p>"))).unwrap();
    assert!(l.markdown.contains("Text i rutan") && !l.markdown.contains("[^"), "{}", l.markdown);
}

#[test]
fn set_geometry_updates_vml_style() {
    let x = r#"<w:pict><v:rect style="position:absolute;margin-left:10pt;margin-top:5pt;width:100pt;height:50pt;mso-position-horizontal-relative:page"/></w:pict>"#;
    let old = docx_tbox::Head { w: 133.3, h: 66.7, anchor: Some(Anchor { hrel: HRel::Page, h: Pos::Off(13.3), vrel: VRel::Paragraph, v: Pos::Off(6.7), ..Anchor::default() }) };
    let mut new = old;
    new.w = 200.0;
    new.anchor = Some(Anchor { hrel: HRel::Column, h: Pos::Off(40.0), vrel: VRel::Paragraph, v: Pos::Off(0.0), ..Anchor::default() });
    let y = docx_tbox::set_geometry(x, &old, &new);
    assert!(y.contains("width:150pt") && y.contains("margin-left:30pt") && y.contains("mso-position-horizontal-relative:text"), "{y}");
    // Oförändrat: orört.
    assert_eq!(docx_tbox::set_geometry(x, &old, &old), x);
}

/// Flyttar rutan i texten som dragning i dokumentläget gör (`tboxui`): nytt `x`/`y` från spaltens vänsterkant och styckets överkant.
fn drag_box(md: &str, x: f32, y: f32) -> String {
    use crate::word::{anchor::Pos, tboxui};
    let o = md.find("<!--k:tb").unwrap();
    let open = o..md[o..].find("-->").unwrap() + o + 3;
    let mut head = docx_tbox::parse_head(&md[open.clone()]).unwrap().1;
    let mut a = head.anchor.unwrap_or_default();
    (a.h, a.v) = (Pos::Off(x), Pos::Off(y));
    head.anchor = Some(a);
    tboxui::apply(md, &tboxui::Change { open, head, x_pt: Some(x * 0.75) }).unwrap()
}

fn box_pos(md: &str) -> (Pos, Pos, f32, f32) {
    let o = md.find("<!--k:tb").unwrap();
    let h = docx_tbox::parse_head(&md[o..o + md[o..].find("-->").unwrap() + 3]).unwrap().1;
    let a = h.anchor.unwrap();
    (a.h, a.v, h.w, h.h)
}

#[test]
fn dragged_box_is_saved_and_read_back_at_the_same_place() {
    let l = read(&doc()).unwrap();
    let md = drag_box(&l.markdown, 73.5, 21.0);
    // Både med originalpaketet (läget skrivs in i originalets XML) och som nytt dokument.
    for source in [l.source.as_ref(), None] {
        let out = write(&md, "x", &|_| None, source).unwrap().bytes;
        let back = read(&out).unwrap();
        let (h, v, w, hh) = box_pos(&back.markdown);
        let (_, _, w0, h0h) = box_pos(&md);
        assert_eq!((h, v), (Pos::Off(73.5), Pos::Off(21.0)), "{}", back.markdown);
        assert!((w - w0).abs() < 0.6 && (hh - h0h).abs() < 0.6, "{w}x{hh} mot {w0}x{h0h}");
        assert!(back.markdown.contains("**Faktaruta**") && back.markdown.contains("Text i rutan"));
    }
}
