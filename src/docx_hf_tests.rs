//! Sidhuvud och sidfot i docx: bilder, fält, tabeller, varianter och återskrivning (påhittad XML).

use super::super::*;

const WNS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const PNG: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0xd, 0xa, 0x1a, 0xa, 0x0, 0x0, 0x0, 0xd, 0x49, 0x48, 0x44, 0x52, 0x0, 0x0, 0x0, 0x1, 0x0, 0x0, 0x0, 0x1, 0x8, 0x2, 0x0, 0x0, 0x0, 0x90, 0x77, 0x53, 0xde, 0x0,
    0x0, 0x0, 0xc, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x0, 0x0, 0x3, 0x1, 0x1, 0x0, 0xc9, 0xfe, 0x92, 0xef, 0x0, 0x0, 0x0, 0x0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
    0x60, 0x82,
];

fn ns() -> String {
    format!(
        r#"xmlns:w="{WNS}" xmlns:r="{REL_NS}" xmlns:wp="{WP_NS}" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#
    )
}

/// Ett minimalt docx: `body` (med `w:sectPr`), `doc_rels` (extra relationer) och övriga delar.
fn mk(body: &str, doc_rels: &str, parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut out);
        let o = zip::write::SimpleFileOptions::default();
        let mut add = |n: &str, d: &[u8]| {
            z.start_file(n, o).unwrap();
            z.write_all(d).unwrap();
        };
        add("[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#);
        add("_rels/.rels", format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_NS}/officeDocument" Target="word/document.xml"/></Relationships>"#).as_bytes());
        add("word/document.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {}><w:body>{body}</w:body></w:document>"#, ns()).as_bytes());
        add("word/_rels/document.xml.rels", format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{doc_rels}</Relationships>"#).as_bytes());
        for (n, d) in parts {
            add(n, d);
        }
        z.finish().unwrap();
    }
    out.into_inner()
}

fn rel(id: &str, kind: &str, target: &str) -> String {
    format!(r#"<Relationship Id="{id}" Type="{REL_NS}/{kind}" Target="{target}"/>"#)
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut s).unwrap();
    s
}

fn has_part(bytes: &[u8], name: &str) -> bool {
    zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap().file_names().any(|n| n == name)
}

fn inline_logo(rid: &str, cx: i64, cy: i64) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="1" name="logo"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="{rid}"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

fn hdr(inner: &str) -> Vec<u8> {
    format!(r#"<w:hdr {}>{inner}</w:hdr>"#, ns()).into_bytes()
}

fn ftr(inner: &str) -> Vec<u8> {
    format!(r#"<w:ftr {}>{inner}</w:ftr>"#, ns()).into_bytes()
}

const PG: &str = r#"<w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1134" w:bottom="1417" w:left="1134" w:header="708" w:footer="708" w:gutter="0"/>"#;

fn base_line(l: &Loaded) -> String {
    l.markdown.lines().find(|x| x.starts_with("<!-- kladd:")).unwrap_or("").to_owned()
}

fn defaults(l: &Loaded) -> DocDefaults {
    DocDefaults::from_comment(l.markdown.lines().next().unwrap()).expect("grundformat")
}

fn resolver(l: &Loaded) -> impl Fn(&str) -> Option<Image> + '_ {
    move |s| crate::doc::resolve_image(s, std::path::Path::new(""), &l.images)
}

fn doc_with(hdr_xml: Vec<u8>, ftr_xml: Vec<u8>) -> Vec<u8> {
    let sect = format!(r#"<w:sectPr><w:headerReference w:type="default" r:id="rId10"/><w:footerReference w:type="default" r:id="rId11"/>{PG}</w:sectPr>"#);
    mk(
        &format!("<w:p><w:r><w:t>Text</w:t></w:r></w:p>{sect}"),
        &(rel("rId10", "header", "header1.xml") + &rel("rId11", "footer", "footer1.xml")),
        &[
            ("word/header1.xml", hdr_xml),
            ("word/footer1.xml", ftr_xml),
            ("word/_rels/header1.xml.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>"#, rel("rId1", "image", "media/logo.png")).into_bytes()),
            ("word/media/logo.png", PNG.to_vec()),
        ],
    )
}

#[test]
fn logo_in_header_relation_in_header_rels_and_page_fields_with_formats() {
    let h = hdr(&format!(r#"<w:p><w:r><w:t>Skola </w:t></w:r>{}</w:p>"#, inline_logo("rId1", 508000, 254000)));
    let f = ftr(
        r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">Sida </w:t></w:r><w:fldSimple w:instr=" PAGE \* roman "><w:r><w:t>i</w:t></w:r></w:fldSimple><w:r><w:t xml:space="preserve"> av </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> NUMPAGES \* Arabic </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>9</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> (</w:t></w:r><w:fldSimple w:instr="SECTIONPAGES"><w:r><w:t>2</w:t></w:r></w:fldSimple><w:r><w:t>)</w:t></w:r></w:p>"#,
    );
    let l = read(&doc_with(h, f)).unwrap();
    let d = defaults(&l);
    assert_eq!(d.header.as_deref(), Some("Skola {bild:logo.png 40x20}"), "{}", base_line(&l));
    assert_eq!(d.footer.as_deref(), Some("Sida {sida:roman} av {sidor:1} ({avsnittssidor})"));
    assert_eq!(d.footer_align, Align::Center);
    assert_eq!(l.images.len(), 1);
    assert_eq!(l.images[0].0, "logo.png");
}

#[test]
fn anchored_logo_is_placed_from_the_page_corner() {
    let anchor = r#"<w:r><w:drawing><wp:anchor behindDoc="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>635000</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>254000</wp:posOffset></wp:positionV><wp:extent cx="762000" cy="381000"/><wp:wrapNone/><wp:docPr id="1" name="logo"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId1"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"#;
    let l = read(&doc_with(hdr(&format!("<w:p>{anchor}<w:r><w:t>Rubrik</w:t></w:r></w:p>")), ftr("<w:p/>"))).unwrap();
    assert_eq!(defaults(&l).header.as_deref(), Some("{bild:logo.png 60x30 @50,20}Rubrik"), "{}", base_line(&l));
}

#[test]
fn borderless_three_cell_table_becomes_left_middle_right_and_style_is_read() {
    let tbl = format!(
        r#"<w:tbl><w:tr><w:tc><w:p>{}</w:p></w:tc><w:tc><w:p><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="8" w:color="156082"/></w:pBdr></w:pPr><w:r><w:rPr><w:color w:val="595959"/><w:sz w:val="20"/></w:rPr><w:t>Titel</w:t></w:r></w:p></w:tc><w:tc><w:p><w:fldSimple w:instr=" DATE "><w:r><w:t>x</w:t></w:r></w:fldSimple></w:p></w:tc></w:tr></w:tbl><w:p/>"#,
        inline_logo("rId1", 457200, 457200)
    );
    let l = read(&doc_with(hdr(&tbl), ftr("<w:p/>"))).unwrap();
    let d = defaults(&l);
    assert_eq!(d.header.as_deref(), Some("{bild:logo.png 36x36}|Titel|{datum}"), "{}", base_line(&l));
    // Stilen kommer från första stycket med text.
    assert_eq!(d.header_style.size, Some(10.0));
    assert_eq!(d.header_style.color, Some([0x59, 0x59, 0x59]));
    assert_eq!(d.header_style.line, Some(([0x15, 0x60, 0x82], 1.0)));
}

#[test]
fn tab_stops_make_three_parts() {
    let h = hdr(r#"<w:p><w:pPr><w:tabs><w:tab w:val="center" w:pos="4513"/><w:tab w:val="right" w:pos="9026"/></w:tabs></w:pPr><w:r><w:t>Vänster</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>Mitt</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>Höger</w:t></w:r></w:p>"#);
    let l = read(&doc_with(h, ftr("<w:p/>"))).unwrap();
    assert_eq!(defaults(&l).header.as_deref(), Some("Vänster|Mitt|Höger"));
}

fn two_sections() -> Vec<u8> {
    let s1 = format!(r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId10"/><w:footerReference w:type="default" r:id="rId11"/>{PG}<w:pgNumType w:fmt="lowerRoman"/></w:sectPr></w:pPr><w:r><w:t>Förord</w:t></w:r></w:p>"#);
    let s2 = format!(r#"<w:p><w:r><w:t>Kapitel</w:t></w:r></w:p><w:sectPr><w:headerReference w:type="default" r:id="rId12"/>{PG}<w:pgNumType w:start="1" w:fmt="decimal"/></w:sectPr>"#);
    mk(
        &(s1 + &s2),
        &(rel("rId10", "header", "header1.xml") + &rel("rId11", "footer", "footer1.xml") + &rel("rId12", "header", "header2.xml")),
        &[
            ("word/header1.xml", hdr("<w:p><w:r><w:t>Första</w:t></w:r></w:p>")),
            ("word/footer1.xml", ftr(r#"<w:p><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>"#)),
            ("word/header2.xml", hdr("<w:p><w:r><w:t>Andra</w:t></w:r></w:p>")),
        ],
    )
}

#[test]
fn sections_have_own_headers_inherit_footer_and_restart_page_numbers() {
    let l = read(&two_sections()).unwrap();
    let rows: Vec<&str> = l.markdown.lines().filter(|x| x.starts_with("<!-- kladd: avsnitt")).collect();
    assert_eq!(rows.len(), 1, "{}", l.markdown);
    assert!(rows[0].contains("header: \"Andra\"") && rows[0].contains("page-start: 1") && rows[0].contains("page-format: 1"), "{}", rows[0]);
    assert!(!rows[0].contains("footer:"), "sidfoten ärvs: {}", rows[0]);
    let d = defaults(&l);
    assert_eq!(d.header.as_deref(), Some("Första"));
    assert_eq!(d.page_format, Some(crate::hf::NumFmt::LowerRoman));
    // Planen: avsnitt 2 har eget huvud men ärver sidfoten; numreringen börjar om.
    let secs = sections::scan(&l.markdown, &d);
    let plan = crate::hf::Plan::new(&d, &secs);
    assert_eq!(plan.secs[1].header.as_deref(), Some("Andra"));
    assert_eq!(plan.secs[1].footer.as_deref(), Some("{sida}"));
    let pp = plan.pages(&[0, 0, 1, 1]);
    assert_eq!(pp.numbers, [1, 2, 1, 2]);
}

#[test]
fn even_and_odd_headers_need_the_settings_flag() {
    let sect = format!(r#"<w:sectPr><w:headerReference w:type="even" r:id="rId12"/><w:headerReference w:type="default" r:id="rId10"/>{PG}</w:sectPr>"#);
    let rels = rel("rId10", "header", "header1.xml") + &rel("rId12", "header", "header2.xml") + &rel("rId13", "settings", "settings.xml");
    let parts = |flag: bool| {
        vec![
            ("word/header1.xml", hdr("<w:p><w:r><w:t>Udda</w:t></w:r></w:p>")),
            ("word/header2.xml", hdr("<w:p><w:r><w:t>Jämn</w:t></w:r></w:p>")),
            ("word/settings.xml", format!(r#"<w:settings xmlns:w="{WNS}">{}<w:defaultTabStop w:val="708"/></w:settings>"#, if flag { "<w:evenAndOddHeaders/>" } else { "" }).into_bytes()),
        ]
    };
    let body = format!("<w:p><w:r><w:t>T</w:t></w:r></w:p>{sect}");
    let on = defaults(&read(&mk(&body, &rels, &parts(true))).unwrap());
    assert_eq!((on.header.as_deref(), on.header_even.as_deref()), (Some("Udda"), Some("Jämn")));
    let off = defaults(&read(&mk(&body, &rels, &parts(false))).unwrap());
    assert_eq!(off.header_even, None);
}

#[test]
fn untouched_document_keeps_header_parts_and_sections_byte_for_byte() {
    let bytes = two_sections();
    let l = read(&bytes).unwrap();
    let out = write(&l.markdown, "t", &resolver(&l), l.source.as_ref()).unwrap().bytes;
    for n in ["word/header1.xml", "word/footer1.xml", "word/header2.xml"] {
        assert_eq!(part(&out, n), part(&bytes, n), "{n}");
    }
    let doc = part(&out, "word/document.xml");
    assert!(doc.contains(r#"<w:pgNumType w:start="1" w:fmt="decimal"/>"#) && doc.contains(r#"r:id="rId12""#), "{doc}");
    assert!(!doc.contains("rIdKladd"), "{doc}");
}

#[test]
fn changed_header_text_keeps_logo_and_fields() {
    let h = hdr(&format!(r#"<w:p><w:r><w:t>Skola </w:t></w:r>{}</w:p>"#, inline_logo("rId1", 508000, 254000)));
    let f = ftr(r#"<w:p><w:r><w:t xml:space="preserve">Sida </w:t></w:r><w:fldSimple w:instr=" PAGE \* ROMAN "><w:r><w:t>I</w:t></w:r></w:fldSimple></w:p>"#);
    let bytes = doc_with(h, f);
    let l = read(&bytes).unwrap();
    let md = l.markdown.replacen("header: \"Skola ", "header: \"Ny skola ", 1);
    assert_ne!(md, l.markdown);
    let out = write(&md, "t", &resolver(&l), l.source.as_ref()).unwrap().bytes;
    // Sidfoten är orörd.
    assert_eq!(part(&out, "word/footer1.xml"), part(&bytes, "word/footer1.xml"));
    let doc = part(&out, "word/document.xml");
    assert!(doc.contains("rIdKladdHdr1"), "{doc}");
    let new_hdr = part(&out, "word/header2.xml");
    assert!(new_hdr.contains("Ny skola") && new_hdr.contains("<w:drawing>") && new_hdr.contains(r#"cx="508000""#), "{new_hdr}");
    let rels = part(&out, "word/_rels/header2.xml.rels");
    assert!(rels.contains("/image") && rels.contains("media/kladd-hf-1.png"), "{rels}");
    assert!(has_part(&out, "word/media/kladd-hf-1.png"));
    // Läses tillbaka lika.
    let back = read(&out).unwrap();
    assert_eq!(defaults(&back).header.as_deref(), Some("Ny skola {bild:kladd-hf-1.png 40x20}"), "{}", base_line(&back));
}

#[test]
fn fresh_document_writes_formats_even_odd_sections_and_page_start() {
    let md = "<!-- kladd: header: \"Rapport|{bild:logo.png 40x20}|{datum}\"; footer: \"Sida {sida:roman} av {sidor} ({avsnittssidor})\"; footer-align: center; header-even: \"Jämn\"; header-style: \"9pt #666666 line=#999999/0.5pt\"; header-dist: 10mm; page-format: roman -->\n\nFörsta\n\n<!-- kladd: avsnitt; header: \"Bilaga\"; page-start: 1; page-format: 1; title-page: yes; header-first: \"\" -->\n\nAndra\n";
    let l = Loaded { source: None, markdown: md.to_owned(), images: vec![("logo.png".into(), Image { bytes: PNG.to_vec(), extent: None })], dropped: vec![] };
    let out = write(md, "t", &resolver(&l), None).unwrap().bytes;
    let h = part(&out, "word/header1.xml");
    assert!(h.contains("<w:drawing>") && h.contains("w:color w:val=\"666666\"") && h.contains("<w:pBdr><w:bottom") && h.contains("w:tab w:val=\"right\""), "{h}");
    let f = part(&out, "word/footer1.xml");
    assert!(f.contains("PAGE \\* roman") && f.contains("NUMPAGES") && f.contains("SECTIONPAGES"), "{f}");
    assert!(part(&out, "word/settings.xml").contains("<w:evenAndOddHeaders/>"));
    assert!(part(&out, "[Content_Types].xml").contains("Extension=\"png\""));
    let doc = part(&out, "word/document.xml");
    assert!(doc.contains(r#"w:type="even""#) && doc.contains("<w:pgNumType"), "{doc}");
    assert!(doc.contains(r#"w:header="567""#), "{doc}");
    // Och tillbaka.
    let back = read(&out).unwrap();
    let d = defaults(&back);
    assert_eq!(d.header.as_deref(), Some("Rapport|{bild:kladd-hf-1.png 40x20}|{datum}"), "{}", back.markdown);
    assert_eq!(d.footer.as_deref(), Some("Sida {sida:roman} av {sidor} ({avsnittssidor})"));
    assert_eq!(d.header_even.as_deref(), Some("Jämn"));
    assert_eq!(d.page_format, Some(crate::hf::NumFmt::LowerRoman));
    let row = back.markdown.lines().find(|x| x.starts_with("<!-- kladd: avsnitt")).unwrap();
    assert!(row.contains("header: \"Bilaga\"") && row.contains("page-start: 1") && row.contains("title-page: yes"), "{row}");
}

#[test]
fn changed_section_header_only_touches_that_section() {
    let bytes = two_sections();
    let l = read(&bytes).unwrap();
    let md = l.markdown.replace("header: \"Andra\"", "header: \"Tredje\"");
    let out = write(&md, "t", &resolver(&l), l.source.as_ref()).unwrap().bytes;
    assert_eq!(part(&out, "word/header1.xml"), part(&bytes, "word/header1.xml"));
    let doc = part(&out, "word/document.xml");
    // Första avsnittets referenser är orörda, det sista pekar på en ny del.
    assert!(doc.contains(r#"<w:headerReference w:type="default" r:id="rId10"/>"#), "{doc}");
    assert!(!doc.contains(r#"r:id="rId12""#) && doc.contains("rIdKladdHdr1"), "{doc}");
    let back = read(&out).unwrap();
    assert!(back.markdown.contains("header: \"Tredje\""), "{}", back.markdown);
}


