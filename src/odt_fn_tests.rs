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
fn endnotes_are_read_as_footnotes() {
    let c = r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3">
<office:body><office:text>
<text:p>Före<text:note text:id="ftn0" text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p text:style-name="Footnote">Första noten<text:s/>med blanksteg</text:p><text:p>Andra stycket</text:p></text:note-body></text:note> mitt<text:note text:id="edn0" text:note-class="endnote"><text:note-citation>i</text:note-citation><text:note-body><text:p>Slutnot</text:p></text:note-body></text:note> efter.</text:p>
</office:text></office:body></office:document-content>"##;
    let l = read(&make(c)).unwrap();
    assert!(l.dropped.is_empty());
    assert_eq!(l.markdown, "Före[^1] mitt[^2] efter.\n\n[^1]: Första noten med blanksteg<br>Andra stycket\n[^2]: Slutnot\n");
}
