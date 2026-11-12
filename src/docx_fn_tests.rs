//! Tester för fotnoter i Word-läsning och -skrivning (`w:footnoteReference` ↔ `[^N]`).
use super::*;
use crate::word::markup;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn patch_part(bytes: &[u8], part: &str, f: impl Fn(&str) -> String) -> Vec<u8> {
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..src.len() {
            let mut file = src.by_index(i).unwrap();
            let name = file.name().to_owned();
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut data).unwrap();
            if name == part {
                data = f(&String::from_utf8(data).unwrap()).into_bytes();
            }
            zw.start_file(name, opts).unwrap();
            std::io::Write::write_all(&mut zw, &data).unwrap();
        }
        zw.finish().unwrap();
    }
    out.into_inner()
}

fn part_text(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut out).unwrap();
    out
}

fn has_part(bytes: &[u8], name: &str) -> bool {
    zip::ZipArchive::new(Cursor::new(bytes)).unwrap().file_names().any(|n| n == name)
}

fn fn_ref(id: u32) -> String {
    format!(r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="{id}"/></w:r>"#)
}

const P_PLAIN: &str = r#"<w:p w:rsidR="00B1"><w:r><w:t>Ett orört stycke.</w:t></w:r></w:p>"#;

fn p_first() -> String {
    format!(r#"<w:p w:rsidR="00B2"><w:r><w:t xml:space="preserve">Sju kommer först</w:t></w:r>{}<w:r><w:t xml:space="preserve"> och sedan tre</w:t></w:r>{}<w:r><w:t>.</w:t></w:r></w:p>"#, fn_ref(7), fn_ref(3))
}

const SEPARATORS: &str = r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#;
const FN7: &str = r#"<w:footnote w:id="7"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> Sju</w:t></w:r></w:p></w:footnote>"#;
const FN3: &str = r#"<w:footnote w:id="3"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> Se </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>källan</w:t></w:r></w:p><w:p><w:r><w:t>Andra stycket.</w:t></w:r></w:p></w:footnote>"#;

fn footnotes_xml() -> String {
    format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes {NS}>{SEPARATORS}{FN3}{FN7}</w:footnotes>"#)
}

/// Ett paket med två fotnoter (id 7 och 3, i den ordningen i texten), skiljelinjer i footnotes.xml.
fn fixture() -> Vec<u8> {
    let base = write("Text[^1]\n\n[^1]: x\n", "x", &|_| None, None).unwrap().bytes;
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{P_PLAIN}{}<w:p><w:r><w:t>Slutstycke</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1134" w:bottom="1417" w:left="1134" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#,
        p_first()
    );
    let b = patch_part(&base, "word/document.xml", |_| doc.clone());
    patch_part(&b, "word/footnotes.xml", |_| footnotes_xml())
}

fn ref_ids(xml: &str) -> Vec<String> {
    footnote_ids_in(xml)
}

fn note_ids(xml: &str) -> Vec<String> {
    let (_, items, _) = split_footnotes(xml).unwrap();
    items.into_iter().filter(|i| i.1.is_none()).map(|i| i.0).collect()
}

#[test]
fn reads_footnotes_as_markdown_definitions() {
    let l = read(&fixture()).unwrap();
    // Etiketterna räknas i dokumentordning; idn 7 och 3 blir [^1] och [^2].
    assert!(l.markdown.contains("Sju kommer först[^1] och sedan tre[^2]."), "{}", l.markdown);
    assert!(l.markdown.ends_with("\n[^1]: Sju\n[^2]: Se **källan**<br>Andra stycket.\n"), "{}", l.markdown);
    assert!(!l.markdown.contains("<!--k:"), "{}", l.markdown);
    assert!(l.dropped.is_empty());
    // Skiljelinjerna är inte fotnoter.
    assert_eq!(l.markdown.matches("[^").count(), 4);
    // Modellen ser dem som referenser och definitioner.
    let n = markup::footnotes(&l.markdown);
    assert_eq!((n.refs.len(), n.defs.len()), (2, 2));
}

#[test]
fn fresh_write_then_read_roundtrip() {
    let md = "Hej[^1] och [länk](https://exempel.se)[^2].\n\n[^1]: Ett *kursiv* och `kod`.\n[^2]: Två **fet** <u>under</u>.\n";
    let out = write(md, "x", &|_| None, None).unwrap().bytes;
    let doc = part_text(&out, "word/document.xml");
    assert_eq!(ref_ids(&doc), ["1", "2"]);
    assert!(doc.contains(r#"<w:rStyle w:val="FootnoteReference"/>"#));
    let f = part_text(&out, "word/footnotes.xml");
    // Skiljelinjer, två fotnoter med Words nummerrun och FootnoteText.
    assert!(f.contains(r#"w:type="separator" w:id="-1""#) && f.contains(r#"w:type="continuationSeparator" w:id="0""#));
    assert_eq!(f.matches("<w:footnoteRef/>").count(), 2);
    assert_eq!(note_ids(&f), ["1", "2"]);
    assert!(part_text(&out, "word/_rels/document.xml.rels").contains("/footnotes\" Target=\"footnotes.xml\""));
    assert!(part_text(&out, "[Content_Types].xml").contains("/word/footnotes.xml"));
    assert!(part_text(&out, "word/styles.xml").contains("w:styleId=\"FootnoteReference\""));
    assert!(part_text(&out, "word/styles.xml").contains("w:styleId=\"FootnoteText\""));
    assert!(part_text(&out, "word/settings.xml").contains("<w:footnotePr>"));
    let back = read(&out).unwrap();
    // Länken i brödtexten består; fotnotstexten bevaras (länkar i fotnoter skrivs som text).
    assert_eq!(back.markdown, md);
}

#[test]
fn document_without_footnotes_has_no_footnote_part() {
    let out = write("Bara text\n", "x", &|_| None, None).unwrap().bytes;
    assert!(!has_part(&out, "word/footnotes.xml"));
    assert!(!part_text(&out, "word/settings.xml").contains("footnotePr"));
}

#[test]
fn unrelated_edit_keeps_footnotes_byte_for_byte() {
    let docx = fixture();
    let l = read(&docx).unwrap();
    let edited = l.markdown.replace("Slutstycke", "Ändrat slutstycke");
    let out = write(&edited, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    assert_eq!(part_text(&out, "word/footnotes.xml"), footnotes_xml());
    let doc = part_text(&out, "word/document.xml");
    // Stycket med referenserna och det orörda stycket är kvar som de var, med originalets id.
    assert!(doc.contains(&p_first()) && doc.contains(P_PLAIN), "{doc}");
    assert!(doc.contains("Ändrat slutstycke"));
    assert_eq!(ref_ids(&doc), ["7", "3"]);
    assert_eq!(read(&out).unwrap().markdown, edited);
}

#[test]
fn unchanged_document_is_identical() {
    let docx = fixture();
    let l = read(&docx).unwrap();
    let out = write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    assert_eq!(part_text(&out, "word/footnotes.xml"), footnotes_xml());
    let strip = |x: String| x.replace(r#" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#, "");
    assert_eq!(strip(part_text(&out, "word/document.xml")), strip(part_text(&docx, "word/document.xml")));
}

#[test]
fn editing_the_paragraph_with_references_keeps_ids_and_notes() {
    let docx = fixture();
    let l = read(&docx).unwrap();
    let edited = l.markdown.replace("kommer först", "står först");
    let out = write(&edited, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let doc = part_text(&out, "word/document.xml");
    assert!(doc.contains("står först") && !doc.contains("kommer först"));
    // Referenserna får samma id som förut eftersom fotnotstexterna är oförändrade.
    assert_eq!(ref_ids(&doc), ["7", "3"]);
    let f = part_text(&out, "word/footnotes.xml");
    assert!(f.contains(FN7) && f.contains(FN3), "{f}");
}

#[test]
fn inserting_a_footnote_renumbers_without_touching_old_paragraphs() {
    let docx = fixture();
    let l = read(&docx).unwrap();
    // Ny fotnot i början av det orörda stycket: de gamla blir [^2] och [^3].
    let at = l.markdown.find("Ett orört").unwrap();
    let e = markup::insert_footnote(&l.markdown, at + 3);
    assert!(e.text.contains("Ett[^1] orört") && e.text.contains("först[^2]") && e.text.contains("tre[^3]"), "{}", e.text);
    let md = e.text.replace("[^1]: \n", "[^1]: Ny fotnot.\n");
    let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let doc = part_text(&out, "word/document.xml");
    // Stycket med de gamla referenserna är orört (även om numren ändrats).
    assert!(doc.contains(&p_first()), "{doc}");
    let f = part_text(&out, "word/footnotes.xml");
    assert!(f.contains(FN7) && f.contains(FN3) && f.contains(SEPARATORS), "{f}");
    // Den nya fotnoten har ett eget id; referenser och fotnoter stämmer.
    let mut refs = ref_ids(&doc);
    refs.sort();
    let mut notes = note_ids(&f);
    notes.sort();
    assert_eq!(refs, notes);
    assert_eq!(refs.len(), 3);
    assert!(f.contains("Ny fotnot."));
    // Och det går att läsa tillbaka.
    assert_eq!(read(&out).unwrap().markdown, md);
}

#[test]
fn edited_footnote_text_is_rewritten_with_the_same_id() {
    let docx = fixture();
    let l = read(&docx).unwrap();
    let md = l.markdown.replace("[^1]: Sju", "[^1]: Sju och ändrad");
    let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let f = part_text(&out, "word/footnotes.xml");
    assert!(f.contains(FN3) && !f.contains(FN7));
    assert!(f.contains(r#"<w:footnote w:id="7">"#) && f.contains("Sju och ändrad") && f.contains("<w:footnoteRef/>"));
    assert_eq!(part_text(&out, "word/document.xml").matches(r#"w:footnoteReference w:id="7""#).count(), 1);
    assert_eq!(read(&out).unwrap().markdown, md);
}

#[test]
fn deleted_footnote_disappears_but_separators_stay() {
    let docx = fixture();
    let l = read(&docx).unwrap();
    let md = markup::delete_footnote(&l.markdown, "1");
    assert!(md.contains("Sju kommer först och sedan tre[^1]."), "{md}");
    let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    let doc = part_text(&out, "word/document.xml");
    assert_eq!(ref_ids(&doc), ["3"]);
    let f = part_text(&out, "word/footnotes.xml");
    assert!(f.contains(SEPARATORS) && f.contains(FN3) && !f.contains("w:id=\"7\""), "{f}");
}

#[test]
fn new_footnote_in_a_document_without_footnote_part() {
    let base = write("Text\n", "x", &|_| None, None).unwrap().bytes;
    let l = read(&base).unwrap();
    let md = "Text[^1]\n\n[^1]: Ny.\n";
    let out = write(md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    assert!(has_part(&out, "word/footnotes.xml"));
    assert!(part_text(&out, "[Content_Types].xml").contains("/word/footnotes.xml"));
    assert!(part_text(&out, "word/_rels/document.xml.rels").contains("footnotes.xml"));
    assert!(part_text(&out, "word/settings.xml").contains("<w:footnotePr>"));
    assert_eq!(read(&out).unwrap().markdown, md);
}

#[test]
fn endnotes_and_custom_marks_stay_opaque() {
    let base = fixture();
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body><w:p><w:r><w:t>A</w:t></w:r><w:r><w:endnoteReference w:id="1"/></w:r><w:r><w:footnoteReference w:id="3" w:customMarkFollows="1"/></w:r><w:r><w:t>*</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#
    );
    let b = patch_part(&base, "word/document.xml", |_| doc.clone());
    let l = read(&b).unwrap();
    assert!(l.markdown.contains("<!--k:"), "{}", l.markdown);
    assert!(!l.markdown.contains("[^"), "{}", l.markdown);
    // Sparas den orörd består allt.
    let out = write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
    // Fotnoten som den anpassade referensen pekar på behålls; den som ingen längre refererar till faller bort.
    let f = part_text(&out, "word/footnotes.xml");
    assert!(f.contains(FN3) && f.contains(SEPARATORS) && !f.contains(r#"w:id="7""#), "{f}");
    assert!(part_text(&out, "word/document.xml").contains(r#"w:customMarkFollows="1""#));
}
