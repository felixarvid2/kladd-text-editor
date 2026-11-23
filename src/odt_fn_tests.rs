//! Tester för fotnoter i OpenDocument (`text:note` ↔ `[^N]`).
use super::*;

fn content_of(bytes: &[u8]) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name("content.xml").unwrap().read_to_string(&mut s).unwrap();
    s
}

fn make(content: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        z.start_file("mimetype", o).unwrap();
        z.write_all(MIME.as_bytes()).unwrap();
        z.start_file("content.xml", o).unwrap();
        z.write_all(content.as_bytes()).unwrap();
        z.finish().unwrap();
    }
    buf.into_inner()
}

#[test]
fn writes_text_note_with_citation_and_body() {
    let md = "Första[^1] och andra[^2].\n\n[^1]: Ett *kursiv*.\n[^2]: Två **fet** och [länk](https://exempel.se).\n";
    let w = write(md, "T", &|_| None).unwrap();
    let c = content_of(&w.bytes);
    assert_eq!(c.matches("<text:note ").count(), 2, "{c}");
    assert!(c.contains(r#"text:note-class="footnote""#) && c.contains("<text:note-citation>1</text:note-citation>") && c.contains("<text:note-citation>2</text:note-citation>"));
    assert!(c.contains("<text:note-body><text:p text:style-name=\"Footnote\">"), "{c}");
    // Definitionerna ligger inte kvar som stycken efter brödtexten.
    assert!(!c.contains("[^"), "{c}");
}

#[test]
fn footnotes_roundtrip() {
    let md = "Första[^1] och andra[^2].\n\n[^1]: Ett *kursiv*.\n[^2]: Två **fet** och [länk](https://exempel.se)<br>ny rad.\n";
    let w = write(md, "T", &|_| None).unwrap();
    let back = read(&w.bytes).unwrap();
    assert!(back.dropped.is_empty(), "{:?}", back.dropped);
    assert_eq!(back.markdown, md, "\n--- fick ---\n{}", back.markdown);
    // Och en gång till.
    let again = write(&back.markdown, "T", &|_| None).unwrap();
    assert_eq!(read(&again.bytes).unwrap().markdown, md);
}

#[test]
fn footnotes_roundtrip_with_other_base_font() {
    // Fotnotsmallen är mindre än brödtexten och ska inte ge span-format.
    let md = "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt -->\n\nText[^1]\n\n[^1]: Fotnot utan spans.\n";
    let w = write(md, "T", &|_| None).unwrap();
    assert_eq!(read(&w.bytes).unwrap().markdown, md);
}

#[test]
fn endnotes_are_read_as_endnotes() {
    let c = r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3">
<office:body><office:text>
<text:p>Före<text:note text:id="ftn0" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p text:style-name="Footnote">Första noten<text:s/>med blanksteg</text:p><text:p>Andra stycket</text:p></text:note-body></text:note> mitt<text:note text:id="edn0" text:note-class="endnote"><text:note-citation>i</text:note-citation><text:note-body><text:p>Slutnot</text:p></text:note-body></text:note> efter.</text:p>
</office:text></office:body></office:document-content>"##;
    let l = read(&make(c)).unwrap();
    assert!(l.dropped.is_empty());
    assert_eq!(l.markdown, "Före[^1] mitt[^ei] efter.\n\n[^1]: Första noten med blanksteg<br>Andra stycket\n[^ei]: Slutnot\n");
}

#[test]
fn endnotes_roundtrip() {
    let md = "Fot[^1] och slut[^ei] och mer[^eii].\n\n[^1]: Fotnot.\n[^ei]: Slut ett.\n[^eii]: Slut *två*.\n";
    let w = write(md, "T", &|_| None).unwrap();
    let c = content_of(&w.bytes);
    assert_eq!(c.matches(r#"text:note-class="endnote""#).count(), 2, "{c}");
    assert!(c.contains("<text:note-citation>ii</text:note-citation>") && c.contains("<text:note-citation>1</text:note-citation>"), "{c}");
    let back = read(&w.bytes).unwrap();
    assert!(back.dropped.is_empty(), "{:?}", back.dropped);
    assert_eq!(back.markdown, md, "\n--- fick ---\n{}", back.markdown);
}

// ---------------------------------------------------------------- talformat (`text:notes-configuration`)

fn make_styled(content: &str, styles: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        z.start_file("mimetype", o).unwrap();
        z.write_all(MIME.as_bytes()).unwrap();
        z.start_file("content.xml", o).unwrap();
        z.write_all(content.as_bytes()).unwrap();
        z.start_file("styles.xml", o).unwrap();
        z.write_all(styles.as_bytes()).unwrap();
        z.finish().unwrap();
    }
    buf.into_inner()
}

const NOTE_NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3""#;

fn notes_doc(config: &str) -> Vec<u8> {
    let c = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {NOTE_NS}><office:body><office:text><text:p>Före<text:note text:id="ftn0" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>Fot</text:p></text:note-body></text:note> mitt<text:note text:id="edn0" text:note-class="endnote"><text:note-citation>i</text:note-citation><text:note-body><text:p>Slut</text:p></text:note-body></text:note>.</text:p></office:text></office:body></office:document-content>"#
    );
    let s = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {NOTE_NS}><office:styles>{config}</office:styles></office:document-styles>"#);
    make_styled(&c, &s)
}

#[test]
fn reads_notes_configuration() {
    let cfg = r#"<text:notes-configuration text:note-class="footnote" style:num-format="A" text:start-value="2"/><text:notes-configuration text:note-class="endnote" style:num-format="1" text:start-value="0"/>"#;
    let l = read(&notes_doc(cfg)).unwrap();
    assert!(l.markdown.starts_with("<!-- kladd: endnote-format: decimal; footnote-format: upperLetter; footnote-start: 3 -->\n\n"), "{}", l.markdown);
    let a = crate::word::markup::analyze(&l.markdown);
    assert_eq!(a.defaults.notes.display("1").as_deref(), Some("C"));
    assert_eq!(a.defaults.notes.display("ei").as_deref(), Some("1"));
}

#[test]
fn default_configuration_leaves_no_line() {
    let cfg = r#"<text:notes-configuration text:note-class="footnote" style:num-format="1" text:start-value="0"/><text:notes-configuration text:note-class="endnote" style:num-format="i" text:start-value="0"/>"#;
    let l = read(&notes_doc(cfg)).unwrap();
    assert!(!l.markdown.contains("kladd:"), "{}", l.markdown);
}

#[test]
fn note_formats_roundtrip_through_odt() {
    let md = "<!-- kladd: endnote-format: decimal; endnote-start: 2; footnote-format: chicago -->\n\nFot[^1] slut[^ei] mer[^eii].\n\n[^1]: Fotnot.\n[^ei]: Ett.\n[^eii]: Två.\n";
    let w = write(md, "T", &|_| None).unwrap();
    let mut z = zip::ZipArchive::new(Cursor::new(&w.bytes)).unwrap();
    let mut styles = String::new();
    z.by_name("styles.xml").unwrap().read_to_string(&mut styles).unwrap();
    assert!(styles.contains(r#"text:note-class="footnote" style:num-format="*" text:start-value="0""#), "{styles}");
    assert!(styles.contains(r#"text:note-class="endnote" style:num-format="1" text:start-value="1""#), "{styles}");
    let c = content_of(&w.bytes);
    assert!(c.contains("<text:note-citation>*</text:note-citation>") && c.contains("<text:note-citation>2</text:note-citation>") && c.contains("<text:note-citation>3</text:note-citation>"), "{c}");
    let back = read(&w.bytes).unwrap();
    assert_eq!(back.markdown, md, "\n--- fick ---\n{}", back.markdown);
}
