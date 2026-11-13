//! Tester för spårade ändringar och kommentarer i OpenDocument (`text:tracked-changes`, `office:annotation`).
use super::*;
use crate::review::{self, CommentRec};

/// Ett dokument som LibreOffice själv har skrivit (`soffice --convert-to odt` på ett handskrivet
/// innehåll): en infogning, en radering, en kommentar med svar över ett ord och en löst-markerad punktkommentar.
const LO_FIXTURE: &[u8] = include_bytes!("../tests/data/odt_review_lo.odt");

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn comments(md: &str) -> Vec<CommentRec> {
    review::split_records(md).2
}

#[test]
fn reads_libreoffice_changes_and_comments() {
    let l = read(LO_FIXTURE).unwrap();
    let md = &l.markdown;
    assert!(l.dropped.is_empty(), "{:?}", l.dropped);
    assert!(md.contains("Hej <ins data-k=\"1\">nya</ins> värld <del data-k=\"2\">borttagen</del>idag."), "{md}");
    let (_, revs, cs) = review::split_records(md);
    assert_eq!(revs.len(), 2);
    assert_eq!((revs[0].author.as_str(), revs[0].date.as_str()), ("Ada", "2026-05-01T10:00:00"));
    assert_eq!(revs[1].author, "Bo");
    assert_eq!(cs.len(), 3, "{md}");
    assert_eq!((cs[0].author.as_str(), cs[0].text.as_str(), cs[0].parent, cs[0].done), ("Cy", "Första kommentaren\nRad två", None, false));
    assert_eq!((cs[1].author.as_str(), cs[1].text.as_str(), cs[1].parent), ("Di", "Svar", Some(cs[0].id)));
    assert!(cs[2].done && cs[2].text == "Löst", "{:?}", cs[2]);
    // Området: kommentaren och svaret delar orden, punktkommentaren har ett tomt område.
    assert!(md.contains("Se <!--k:c:0--><!--k:c:1-->det här<!--k:/c:0--><!--k:/c:1--> ordet."), "{md}");
    assert!(md.contains("Punkt<!--k:c:2--><!--k:/c:2--> slut."), "{md}");
    let threads = review::threads(md);
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].replies.len(), 1);
    assert!(review::accept_all(md).contains("Hej nya värld idag."));
}

#[test]
fn written_changes_and_comments_are_read_back() {
    let l = read(LO_FIXTURE).unwrap();
    let w = write(&l.markdown, "T", &|_| None).unwrap();
    let c = part(&w.bytes, "content.xml");
    assert!(c.contains("<text:tracked-changes"), "{c}");
    assert!(c.contains("<text:insertion>") && c.contains("<text:deletion>"), "{c}");
    assert!(c.contains("<text:change-start text:change-id=\"ct1\"/>nya<text:change-end text:change-id=\"ct1\"/>"), "{c}");
    assert!(c.contains("<office:annotation office:name=\"kladd-0\">") && c.contains("<office:annotation-end office:name=\"kladd-0\"/>"), "{c}");
    assert!(c.contains("loext:parent-name=\"kladd-0\"") && c.contains("loext:resolved=\"true\""), "{c}");
    assert!(!c.contains('\u{200B}'), "{c}");
    let back = read(&w.bytes).unwrap();
    assert_eq!(back.markdown, l.markdown, "skrivning och omläsning ska ge samma text");
    // En tredje omgång är stabil.
    let again = write(&back.markdown, "T", &|_| None).unwrap();
    assert_eq!(read(&again.bytes).unwrap().markdown, l.markdown);
}

#[test]
fn comment_at_line_start_and_formatted_deletion_roundtrip() {
    let (e, id) = review::add_comment("Första ordet **fet** här.\n", 0..7, "Ada", "2026-06-01T09:00:00Z", "Hej & \"hå\"  <två>\nrad 2");
    let (e2, _) = review::add_reply(&e.text, id, "Bo", "2026-06-01T10:00:00Z", "Svar").unwrap();
    let t = review::track_edit(&e2.text, &e2.text.replacen("fet", "", 1), 14..14, "Cy", "2026-06-02T09:00:00Z");
    // Radering av den feta texten (om editorn gav en spårad ändring) eller vanlig text, båda ska överleva.
    let md = t.map_or(e2.text.clone(), |t| t.text);
    let w = write(&md, "T", &|_| None).unwrap();
    let back = read(&w.bytes).unwrap();
    let cs = comments(&back.markdown);
    assert_eq!(cs.len(), 2, "{}", back.markdown);
    assert_eq!(cs[0].text, "Hej & \"hå\"  <två>\nrad 2");
    assert_eq!((cs[0].author.as_str(), cs[0].date.as_str()), ("Ada", "2026-06-01T09:00:00Z"));
    assert_eq!(cs[1].parent, Some(cs[0].id));
    assert!(back.markdown.starts_with("&#8203;<!--k:c:") || back.markdown.contains("<!--k:c:"), "{}", back.markdown);
}

#[test]
fn deletion_keeps_its_formatting() {
    let md = "Före <del data-k=\"1\">**fet** och vanlig</del> efter <ins data-k=\"2\">ny</ins>.\n\n<!--k:r id=\"1\" author=\"Ada\" date=\"2026-06-01T09:00:00Z\"-->\n<!--k:r id=\"2\" author=\"Bo\" date=\"2026-06-01T10:00:00Z\"-->\n";
    let w = write(md, "T", &|_| None).unwrap();
    let c = part(&w.bytes, "content.xml");
    assert!(c.contains("<text:change text:change-id=\"ct1\"/>"), "{c}");
    let back = read(&w.bytes).unwrap();
    assert!(back.markdown.contains("<del data-k=\"1\">**fet**</del><del data-k=\"1\"> och vanlig</del>") || back.markdown.contains("**fet**"), "{}", back.markdown);
    assert!(review::accept_all(&back.markdown).contains("Före  efter ny."));
    assert!(review::reject_all(&back.markdown).contains("Före **fet** och vanlig efter ."));
}

#[test]
fn insertion_across_paragraphs_and_in_table_cell() {
    let c = r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" office:version="1.3"><office:body><office:text><text:tracked-changes><text:changed-region text:id="a"><text:insertion><office:change-info><dc:creator>Ada</dc:creator><dc:date>2026-05-01T10:00:00</dc:date></office:change-info></text:insertion></text:changed-region></text:tracked-changes><text:p>En <text:change-start text:change-id="a"/>första</text:p><text:p>andra<text:change-end text:change-id="a"/> sist</text:p></office:text></office:body></office:document-content>"##;
    let l = read(&make(c)).unwrap();
    let (body, revs, _) = review::split_records(&l.markdown);
    assert_eq!(revs.len(), 1, "{}", l.markdown);
    assert!(body.contains("En <ins data-k=\"1\">första</ins>") && body.contains("<ins data-k=\"1\">andra</ins> sist"), "{body}");
}

#[test]
fn untouched_documents_get_no_review_parts() {
    let w = write("Bara text.\n", "T", &|_| None).unwrap();
    let c = part(&w.bytes, "content.xml");
    assert!(!c.contains("tracked-changes") && !c.contains("annotation"), "{c}");
}

#[test]
fn anchorless_records_and_unknown_markers_are_dropped() {
    let md = "Text <!--k:c:7-->här<!--k:/c:7--> klar.\n\n<!--k:c id=\"9\" author=\"X\" date=\"\" text=\"utan område\"-->\n";
    let w = write(md, "T", &|_| None).unwrap();
    let c = part(&w.bytes, "content.xml");
    assert!(!c.contains("annotation") && !c.contains("utan område"), "{c}");
    assert!(c.contains("Text här klar."), "{c}");
}

#[test]
fn format_changes_are_reported() {
    let c = r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" office:version="1.3"><office:body><office:text><text:tracked-changes><text:changed-region text:id="f"><text:format-change><office:change-info><dc:creator>Ada</dc:creator></office:change-info></text:format-change></text:changed-region></text:tracked-changes><text:p>Text</text:p></office:text></office:body></office:document-content>"##;
    let l = read(&make(c)).unwrap();
    assert_eq!(l.dropped, vec!["spårade formatändringar"]);
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

/// LibreOffice öppnar det vi skriver: konvertering till docx går bra och ändringar och kommentarer finns kvar
/// när docx-läsaren läser resultatet. Hoppas över när `soffice` saknas.
#[test]
fn libreoffice_opens_our_review_output() {
    if std::process::Command::new("soffice").arg("--version").output().is_err() {
        eprintln!("soffice saknas: hoppar över");
        return;
    }
    let l = read(LO_FIXTURE).unwrap();
    let w = write(&l.markdown, "T", &|_| None).unwrap();
    let dir = std::env::temp_dir().join(format!("kladd-test-odt-review-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let odt = dir.join("granskat.odt");
    std::fs::write(&odt, &w.bytes).unwrap();
    let profile = format!("-env:UserInstallation=file://{}", dir.join("profil").display());
    for (fmt, ext) in [("docx", "docx"), ("odt:writer8", "odt")] {
        let out_dir = dir.join(ext);
        let out = std::process::Command::new("soffice")
            .arg(&profile)
            .args(["--headless", "--convert-to", fmt, "--outdir"])
            .arg(&out_dir)
            .arg(&odt)
            .output()
            .expect("soffice kunde inte köras");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let file = out_dir.join(format!("granskat.{ext}"));
        let bytes = std::fs::read(&file).expect("ingen utfil skapades");
        let back = if ext == "docx" { crate::docx::read(&bytes).unwrap().markdown } else { read(&bytes).unwrap().markdown };
        let (body, revs, cs) = review::split_records(&back);
        assert_eq!(revs.len(), 2, "{ext}: {back}");
        assert!(body.contains("nya") && body.contains("borttagen"), "{ext}: {back}");
        assert!(cs.iter().any(|c| c.text == "Första kommentaren\nRad två" && c.author == "Cy"), "{ext}: {back}");
        if ext == "odt" {
            // LibreOffice skriver bara områden för kommentarer i docx (svar och punktkommentarer saknar dem där).
            assert_eq!(cs.len(), 3, "{ext}: {back}");
            assert!(cs.iter().any(|c| c.parent.is_some() && c.text == "Svar"), "{ext}: {back}");
            assert!(cs.iter().any(|c| c.done), "{ext}: {back}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
