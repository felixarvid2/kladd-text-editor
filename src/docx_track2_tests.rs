//! Tester för spårade stycketecken och formateringsändringar i .docx (`w:rPr/w:ins|w:del` i `w:pPr`,
//! `w:rPrChange`, `w:pPrChange`).
use super::*;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
const MARK_INS: &str = r#"<w:pPr><w:rPr><w:ins w:id="7" w:author="Ada" w:date="2026-05-01T10:00:00Z"/></w:rPr></w:pPr>"#;
const MARK_DEL: &str = r#"<w:pPr><w:rPr><w:del w:id="8" w:author="Bo" w:date="2026-05-02T10:00:00Z"/></w:rPr></w:pPr>"#;
const RUN_CHANGE: &str = r#"<w:r><w:rPr><w:b/><w:rPrChange w:id="3" w:author="Ada" w:date="2026-05-01T10:00:00Z"><w:rPr/></w:rPrChange></w:rPr><w:t>Fet</w:t></w:r>"#;
const PARA_CHANGE: &str = r#"<w:pPr><w:pStyle w:val="Heading2"/><w:jc w:val="center"/><w:pPrChange w:id="4" w:author="Ada" w:date="2026-05-01T10:00:00Z"><w:pPr><w:pStyle w:val="Normal"/></w:pPr></w:pPrChange></w:pPr>"#;

fn package(body: &str) -> Vec<u8> {
    let doc = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{body}</w:body></w:document>"#);
    let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    write_zip(vec![
        ("[Content_Types].xml".into(), types.as_bytes().to_vec()),
        ("_rels/.rels".into(), root.as_bytes().to_vec()),
        ("word/document.xml".into(), doc.into_bytes()),
    ])
    .unwrap()
}

fn document(bytes: &[u8]) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut out).unwrap();
    out
}

#[test]
fn paragraph_marks_are_read_as_markers_and_round_trip() {
    let body = format!("<w:p>{MARK_INS}<w:r><w:t>Första</w:t></w:r></w:p><w:p>{MARK_DEL}<w:r><w:t>Andra</w:t></w:r></w:p><w:p><w:r><w:t>Tredje</w:t></w:r></w:p>");
    let bytes = package(&body);
    let l = read(&bytes).unwrap();
    let md = &l.markdown;
    assert!(md.contains("Första<!--k:pm ins 1-->\n\nAndra<!--k:pm del 2-->\n\nTredje"), "{md}");
    assert!(md.contains("<!--k:r id=\"1\" author=\"Ada\" date=\"2026-05-01T10:00:00Z\"-->"), "{md}");
    assert!(md.contains("<!--k:r id=\"2\" author=\"Bo\""), "{md}");
    assert!(!l.dropped.iter().any(|d| d.contains("spårade")), "{:?}", l.dropped);
    // Orörd: alla stycken följer med byte för byte.
    let w = write(md, "t", &|_| None, l.source.as_ref()).unwrap();
    let doc = document(&w.bytes);
    assert!(doc.contains(&format!("<w:p>{MARK_INS}<w:r><w:t>Första</w:t></w:r></w:p>")), "{doc}");
    assert!(doc.contains(&format!("<w:p>{MARK_DEL}<w:r><w:t>Andra</w:t></w:r></w:p>")), "{doc}");
}

#[test]
fn rewritten_paragraph_writes_the_marks_again() {
    let body = format!("<w:p>{MARK_INS}<w:r><w:t>Första</w:t></w:r></w:p><w:p>{MARK_DEL}<w:r><w:t>Andra</w:t></w:r></w:p>");
    let l = read(&package(&body)).unwrap();
    let md = l.markdown.replace("Första", "Förste").replace("Andra", "Andre");
    let w = write(&md, "t", &|_| None, l.source.as_ref()).unwrap();
    let doc = document(&w.bytes);
    assert!(doc.contains("<w:pPr><w:rPr><w:ins w:id=") && doc.contains("w:author=\"Ada\"") && doc.contains("w:date=\"2026-05-01T10:00:00Z\""), "{doc}");
    assert!(doc.contains("<w:rPr><w:del w:id=") && doc.contains("w:author=\"Bo\""), "{doc}");
    let back = read(&w.bytes).unwrap();
    assert!(back.markdown.contains("Förste<!--k:pm ins 1-->\n\nAndre<!--k:pm del 2-->"), "{}", back.markdown);
}

#[test]
fn accepting_and_rejecting_marks_from_word() {
    let body = format!("<w:p>{MARK_INS}<w:r><w:t>Första</w:t></w:r></w:p><w:p><w:r><w:t>Andra</w:t></w:r></w:p>");
    let l = read(&package(&body)).unwrap();
    assert_eq!(crate::review::reject_all(&l.markdown), "FörstaAndra\n");
    assert_eq!(crate::review::accept_all(&l.markdown), "Första\n\nAndra\n");
    let body = format!("<w:p>{MARK_DEL}<w:r><w:t>Första</w:t></w:r></w:p><w:p><w:r><w:t>Andra</w:t></w:r></w:p>");
    let l = read(&package(&body)).unwrap();
    assert_eq!(crate::review::accept_all(&l.markdown), "FörstaAndra\n");
    assert_eq!(crate::review::reject_all(&l.markdown), "Första\n\nAndra\n");
}

#[test]
fn tracked_line_breaks_are_markers_and_written_back() {
    let ins = r#"<w:ins w:id="5" w:author="Ada" w:date="2026-05-01T10:00:00Z"><w:r><w:br/></w:r></w:ins>"#;
    let body = format!("<w:p><w:r><w:t>Hej</w:t></w:r>{ins}<w:r><w:t>där</w:t></w:r></w:p>");
    let l = read(&package(&body)).unwrap();
    assert!(l.markdown.contains("Hej<!--k:pm ins 1-->\ndär"), "{}", l.markdown);
    assert_eq!(crate::review::reject_all(&l.markdown), "Hejdär\n");
    let md = l.markdown.replace("där", "där borta");
    let w = write(&md, "t", &|_| None, l.source.as_ref()).unwrap();
    let doc = document(&w.bytes);
    assert!(doc.contains("<w:ins w:id=") && doc.contains("<w:r><w:br/></w:r></w:ins>") && doc.contains("w:author=\"Ada\""), "{doc}");
    assert!(read(&w.bytes).unwrap().markdown.contains("Hej<!--k:pm ins 1-->\ndär borta"));
}

#[test]
fn enter_tracked_in_kladd_becomes_a_word_paragraph_mark() {
    let md = "Hej<!--k:pm ins 1 n=1-->\n\nvärlden\n\n<!--k:r id=\"1\" author=\"Cy\" date=\"2026-06-01T09:00:00Z\"-->\n";
    let w = write(md, "t", &|_| None, None).unwrap();
    let doc = document(&w.bytes);
    assert!(doc.contains("<w:p><w:pPr><w:rPr><w:ins w:id=") && doc.contains("w:author=\"Cy\"") && doc.contains("<w:t xml:space=\"preserve\">Hej</w:t>"), "{doc}");
    let back = read(&w.bytes).unwrap();
    assert!(back.markdown.contains("Hej<!--k:pm ins 1-->\n\nvärlden"), "{}", back.markdown);
}

#[test]
fn character_format_change_is_read_as_fmt_and_written_back() {
    let body = format!("<w:p><w:r><w:t xml:space=\"preserve\">Text </w:t></w:r>{RUN_CHANGE}</w:p>");
    let bytes = package(&body);
    let l = read(&bytes).unwrap();
    let md = &l.markdown;
    assert!(md.contains("Text <fmt data-k=\"1\">**Fet**</fmt>"), "{md}");
    assert!(md.contains("<!--k:r id=\"1\" author=\"Ada\" date=\"2026-05-01T10:00:00Z\" old=\";\"-->"), "{md}");
    assert!(!l.dropped.iter().any(|d| d.contains("spårade")), "{:?}", l.dropped);
    // Godkänn behåller fetstilen, avvisa tar bort den.
    assert!(crate::review::accept_all(md).starts_with("Text **Fet**"), "{}", crate::review::accept_all(md));
    assert!(crate::review::reject_all(md).starts_with("Text Fet"), "{}", crate::review::reject_all(md));
    // Orört stycke: samma XML.
    let w = write(md, "t", &|_| None, l.source.as_ref()).unwrap();
    assert!(document(&w.bytes).contains(RUN_CHANGE));
    // Omskrivet stycke: `w:rPrChange` skrivs igen med det gamla formatet.
    let md2 = md.replace("Text ", "Mer text ");
    let w = write(&md2, "t", &|_| None, l.source.as_ref()).unwrap();
    let doc = document(&w.bytes);
    assert!(doc.contains("<w:rPrChange w:id=") && doc.contains("w:author=\"Ada\"") && doc.contains("<w:rPr></w:rPr></w:rPrChange>"), "{doc}");
    let back = read(&w.bytes).unwrap();
    assert!(back.markdown.contains("Mer text <fmt data-k=\"1\">**Fet**</fmt>") && back.markdown.contains("old=\";\""), "{}", back.markdown);
}

#[test]
fn old_run_properties_are_kept_in_the_record() {
    let run = r#"<w:r><w:rPr><w:i/><w:rPrChange w:id="3" w:author="Ada" w:date="2026-05-01T10:00:00Z"><w:rPr><w:b/><w:color w:val="C00000"/><w:sz w:val="28"/></w:rPr></w:rPrChange></w:rPr><w:t>Kurs</w:t></w:r>"#;
    let l = read(&package(&format!("<w:p>{run}</w:p>"))).unwrap();
    assert!(l.markdown.contains("<fmt data-k=\"1\">*Kurs*</fmt>"), "{}", l.markdown);
    let old = crate::review::scan(&l.markdown).rev_rec(1).and_then(|r| r.old.clone()).unwrap();
    let f = crate::review::fmt::CharFmt::decode(&old);
    assert!(f.bold && !f.italic, "{old}");
    assert_eq!(f.span.color, Some([0xC0, 0, 0]));
    assert_eq!(f.span.font_size, Some(14.0));
    // Avvisa sätter tillbaka fetstil, färg och storlek och tar bort kursiven.
    let rejected = crate::review::reject_all(&l.markdown);
    assert!(rejected.contains("**Kurs**") && !rejected.contains("*Kurs*\n") || rejected.contains("Kurs"), "{rejected}");
    assert!(rejected.contains("color: #C00000") || rejected.contains("color:#C00000"), "{rejected}");
    // Skrivs igen med det gamla formatet i `w:rPrChange`.
    let w = write(&l.markdown.replace("Kurs", "Kursiv"), "t", &|_| None, l.source.as_ref()).unwrap();
    let doc = document(&w.bytes);
    assert!(doc.contains("<w:rPrChange") && doc.contains("<w:b/>") && doc.contains("w:val=\"C00000\"") && doc.contains("<w:sz w:val=\"28\"/>"), "{doc}");
}

#[test]
fn paragraph_format_change_is_read_as_a_marker_and_written_back() {
    let body = format!("<w:p>{PARA_CHANGE}<w:r><w:t>Titel</w:t></w:r></w:p><w:p><w:r><w:t>Brödtext</w:t></w:r></w:p>");
    let bytes = package(&body);
    let l = read(&bytes).unwrap();
    let md = &l.markdown;
    assert!(md.contains("Titel<!--k:pm fmt 1-->"), "{md}");
    assert!(md.contains("old=\"normal|left\""), "{md}");
    assert!(!l.dropped.iter().any(|d| d.contains("spårade")), "{:?}", l.dropped);
    // Avvisa gör stycket till vanlig text.
    let rejected = crate::review::reject_all(md);
    assert!(rejected.starts_with("Titel") && !rejected.contains('#') && !rejected.contains("<div"), "{rejected}");
    assert!(crate::review::accept_all(md).contains("Titel") && !crate::review::accept_all(md).contains("pm fmt"));
    // Orört stycke: samma XML; omskrivet: `w:pPrChange` med det gamla formatet.
    let w = write(md, "t", &|_| None, l.source.as_ref()).unwrap();
    assert!(document(&w.bytes).contains(PARA_CHANGE));
    let w = write(&md.replace("Titel", "Rubriken"), "t", &|_| None, l.source.as_ref()).unwrap();
    let doc = document(&w.bytes);
    assert!(doc.contains("<w:pPrChange w:id=") && doc.contains("w:author=\"Ada\"") && doc.contains("<w:pPr><w:pStyle w:val=\"Normal\"/>") || doc.contains("<w:pPr></w:pPr></w:pPrChange>"), "{doc}");
    assert!(read(&w.bytes).unwrap().markdown.contains("Rubriken<!--k:pm fmt 1-->"));
}

#[test]
fn tracked_formatting_from_kladd_is_written_as_word_changes() {
    let md = "ett **<fmt data-k=\"1\">ord</fmt>** här\n\n## Titel<!--k:pm fmt 2-->\n\n<!--k:r id=\"1\" author=\"Cy\" date=\"2026-06-01T09:00:00Z\" old=\";\"-->\n<!--k:r id=\"2\" author=\"Cy\" date=\"2026-06-01T09:00:00Z\" old=\"normal|center\"-->\n";
    let w = write(md, "t", &|_| None, None).unwrap();
    let doc = document(&w.bytes);
    assert!(doc.contains("<w:rPrChange w:id=") && doc.contains("w:author=\"Cy\""), "{doc}");
    assert!(doc.contains("<w:pPrChange w:id=") && doc.contains("<w:jc w:val=\"center\"/></w:pPr></w:pPrChange>"), "{doc}");
    let back = read(&w.bytes).unwrap();
    assert!(back.markdown.contains("<fmt data-k=\"1\">**ord**</fmt>") || back.markdown.contains("**<fmt"), "{}", back.markdown);
    assert!(back.markdown.contains("<!--k:pm fmt "), "{}", back.markdown);
}
