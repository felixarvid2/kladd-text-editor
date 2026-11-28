//! Tester för EPUB-inläsningen: handskrivna små böcker (zip byggd i testet) och tur och retur via `export::epub`.

use std::io::Write;

use super::*;

/// En zip med filerna (första filen skrivs okomprimerad som `mimetype`).
pub(crate) fn zip_of(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in files {
        z.start_file(*name, opts).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

pub(crate) fn png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 3, image::Rgb([200, 30, 30]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

pub(crate) const CONTAINER: &str = r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

pub(crate) fn xhtml(body: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><!DOCTYPE html><html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\
         <head><title>t</title><style>p {{ color: red }}</style></head><body>{body}</body></html>"
    )
    .into_bytes()
}

/// EPUB 3 med nav, omslagsbild, tre kapitel och en notfil.
pub(crate) fn epub3() -> Vec<u8> {
    let opf = r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Testboken</dc:title><dc:creator>A. Författare</dc:creator><dc:creator>B. Medförfattare</dc:creator>
<dc:publisher>Förlaget</dc:publisher><dc:language>sv</dc:language><dc:date>2020-05-01</dc:date><dc:description>&lt;p&gt;En &lt;b&gt;kort&lt;/b&gt; beskrivning.&lt;/p&gt;</dc:description></metadata>
<manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
<item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
<item id="c3" href="text/notes.xhtml" media-type="application/xhtml+xml"/><item id="cov" href="images/cover%20bild.png" media-type="image/png" properties="cover-image"/>
<item id="im" href="images/fig.png" media-type="image/png"/></manifest>
<spine><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c3"/></spine></package>"#;
    let nav = xhtml(
        r#"<nav epub:type="toc"><ol><li><a href="text/ch1.xhtml">Första kapitlet</a><ol><li><a href="text/ch1.xhtml#sek">Ett avsnitt</a></li></ol></li>
<li><a href="text/ch2.xhtml">Andra kapitlet</a></li><li><a href="text/notes.xhtml">Noter</a></li></ol></nav>"#,
    );
    let ch1 = xhtml(
        r#"<h1>Första kapitlet</h1><p>Börjar här<a epub:type="noteref" href="notes.xhtml#n1">1</a>. Se <a href="ch2.xhtml#mal">andra kapitlet</a>.</p>
<h2 id="sek">Ett avsnitt</h2><p><img src="../images/fig.png" alt="Figur"/></p>
<ul><li>ett<ul><li>ett-ett</li></ul></li><li>två</li></ul>
<table><tr><th>Namn</th><th>Värde</th></tr><tr><td>a</td><td>1</td></tr></table>"#,
    );
    let ch2 = xhtml(r#"<h1>Andra kapitlet</h1><p>Före.</p><p id="mal">Målet för länken.</p>"#);
    let notes = xhtml(r#"<h1>Noter</h1><ol><li id="n1">Den första noten.<a href="ch1.xhtml">↩</a></li></ol>"#);
    zip_of(&[
        ("mimetype", b"application/epub+zip".to_vec()),
        ("META-INF/container.xml", CONTAINER.as_bytes().to_vec()),
        ("OEBPS/content.opf", opf.as_bytes().to_vec()),
        ("OEBPS/nav.xhtml", nav),
        ("OEBPS/text/ch1.xhtml", ch1),
        ("OEBPS/text/ch2.xhtml", ch2),
        ("OEBPS/text/notes.xhtml", notes),
        ("OEBPS/images/cover bild.png", png()),
        ("OEBPS/images/fig.png", png()),
    ])
}

#[test]
fn epub3_metadata_kapitel_och_förteckning() {
    let b = Book::load(5, epub3()).unwrap();
    assert_eq!(b.meta.title, "Testboken");
    assert_eq!(b.meta.authors, ["A. Författare", "B. Medförfattare"]);
    assert_eq!((b.meta.publisher.as_str(), b.meta.language.as_str(), b.meta.date.as_str()), ("Förlaget", "sv", "2020-05-01"));
    assert_eq!(b.meta.description, "En kort beskrivning.", "HTML i beskrivningen blir text");
    assert_eq!(b.title(), "Testboken");
    // Omslaget får en egen första sida (spinen börjar inte med det).
    assert_eq!(b.cover.as_deref(), Some("OEBPS/images/cover bild.png"));
    assert_eq!(b.chapters.len(), 4);
    assert_eq!(b.chapters[0].title, "Omslag");
    assert_eq!(b.sections[0].cover.as_deref(), Some("OEBPS/images/cover bild.png"));
    assert!(b.cover_image().is_some());
    let titles: Vec<&str> = b.chapters.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["Omslag", "Första kapitlet", "Andra kapitlet", "Noter"]);
    let toc: Vec<(&str, usize)> = b.toc.iter().map(|t| (t.title.as_str(), t.depth)).collect();
    assert_eq!(toc, [("Första kapitlet", 0), ("Ett avsnitt", 1), ("Andra kapitlet", 0), ("Noter", 0)]);
    assert!(b.toc.iter().all(|t| t.dest.is_some()));
    // Avsnittets mål ligger efter kapitelstarten i samma kapitel.
    let sek = b.toc[1].dest.unwrap();
    assert_eq!(sek.section, b.toc[0].dest.unwrap().section);
    assert!(sek.byte > 0 && b.sections[sek.section].md[sek.byte..].starts_with("## Ett avsnitt"), "{:?}", &b.sections[sek.section].md[sek.byte..]);
}

#[test]
fn länkar_bilder_tabeller_och_noter_i_epub3() {
    let b = Book::load(5, epub3()).unwrap();
    let s1 = &b.sections[b.chapters[1].first_section];
    assert!(s1.md.contains("| Namn | Värde |\n| --- | --- |\n| a | 1 |"), "{}", s1.md);
    assert!(s1.md.contains("- ett\n  - ett-ett\n- två"), "{}", s1.md);
    assert!(s1.md.contains("![Figur](bytes://kladd-epub/5/OEBPS/images/fig.png)"), "{}", s1.md);
    assert_eq!(s1.images, ["OEBPS/images/fig.png"]);
    assert!(b.image("OEBPS/images/fig.png").is_some());
    // Länken till andra kapitlets mål.
    assert_eq!(s1.link_dests.len(), 2);
    let mut seen = Vec::new();
    for d in &s1.link_dests {
        let (dest, noteref) = b.link(d).unwrap_or_else(|| panic!("länk {d}"));
        seen.push((b.sections[dest.section].chapter, noteref));
        if noteref {
            assert_eq!(b.link_note(d).as_deref(), Some("Den första noten."), "noten utan bakåtlänk");
        } else {
            let ch2 = &b.sections[dest.section];
            assert!(ch2.md[dest.byte..].starts_with("Målet för länken."), "{:?}", &ch2.md[dest.byte..]);
        }
    }
    seen.sort();
    assert_eq!(seen, [(2, false), (3, true)]);
}

#[test]
fn läsordning_framsteg_och_nuvarande_förteckningspost() {
    let b = Book::load(1, epub3()).unwrap();
    assert_eq!(b.linear_chapters(), [0, 1, 2, 3]);
    assert_eq!(b.step_chapter(1, 1), Some(2));
    assert_eq!(b.step_chapter(1, -1), Some(0));
    assert_eq!(b.step_chapter(0, -1), None);
    assert_eq!(b.step_chapter(3, 1), None);
    let s2 = b.chapters[2].first_section;
    assert_eq!(b.toc_current(Dest { section: s2, byte: 5 }), Some(2));
    assert_eq!(b.toc_current(Dest { section: 0, byte: 0 }), None, "omslaget ligger före förteckningen");
    let words: usize = b.sections.iter().map(|s| s.words).sum();
    assert_eq!(b.total_words, words);
    assert!(b.sections.windows(2).all(|w| w[1].words_before == w[0].words_before + w[0].words));
}

/// EPUB 2: bara NCX, omslag via `<meta name="cover">`, ett kapitel med linear="no" och SVG-omslag som första sida.
#[test]
fn epub2_ncx_svg_omslag_och_icke_linjär_fil() {
    let opf = r#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="x" xmlns:opf="y"><dc:title>Gammal bok</dc:title><meta name="cover" content="img"/></metadata>
<manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="cv" href="cover.xhtml" media-type="application/xhtml+xml"/>
<item id="a" href="a.html" media-type="application/xhtml+xml"/><item id="s" href="side.html" media-type="application/xhtml+xml"/><item id="img" href="cover.jpg" media-type="image/jpeg"/></manifest>
<spine toc="ncx"><itemref idref="cv"/><itemref idref="a"/><itemref idref="s" linear="no"/></spine></package>"#;
    let ncx = r#"<ncx><navMap><navPoint><navLabel><text>Kapitel A</text></navLabel><content src="a.html"/></navPoint>
<navPoint><navLabel><text>Sidofil</text></navLabel><content src="side.html#x"/></navPoint></navMap></ncx>"#;
    let cover = xhtml(r#"<div><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image width="4" height="3" xlink:href="cover.jpg"/></svg></div>"#);
    let a = xhtml(r#"<p>Text i A med en <a href="side.html#x">sidolänk</a>.</p>"#);
    let side = xhtml(r#"<p id="x">Sidan.</p>"#);
    let epub = zip_of(&[
        ("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "content.opf").into_bytes()),
        ("content.opf", opf.as_bytes().to_vec()),
        ("toc.ncx", ncx.as_bytes().to_vec()),
        ("cover.xhtml", cover),
        ("a.html", a),
        ("side.html", side),
        ("cover.jpg", png()),
    ]);
    let b = Book::load(1, epub).unwrap();
    // Omslagssidan finns redan i läsordningen: ingen extra sida.
    assert_eq!(b.chapters.len(), 3);
    assert_eq!(b.sections[0].cover.as_deref(), Some("cover.jpg"));
    assert_eq!(b.linear_chapters(), [0, 1]);
    assert_eq!(b.step_chapter(1, 1), None, "den icke-linjära filen hoppas över i flödet");
    assert_eq!(b.step_chapter(2, -1), Some(1), "men är nåbar och går tillbaka i läsordningen");
    let toc: Vec<&str> = b.toc.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(toc, ["Kapitel A", "Sidofil"]);
    let d = b.toc[1].dest.unwrap();
    assert_eq!(b.sections[d.section].chapter, 2);
    // Länken i kapitel A hoppar till den icke-linjära filen.
    let a_sec = &b.sections[b.chapters[1].first_section];
    let (dest, noteref) = b.link(&a_sec.link_dests[0]).unwrap();
    assert!(!noteref);
    assert_eq!(b.sections[dest.section].chapter, 2);
}

#[test]
fn bok_utan_förteckning_använder_kapitlens_rubriker() {
    let opf = r#"<package><metadata><title>Utan nav</title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/><item id="b" href="b.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="a"/><itemref idref="b"/></spine></package>"#;
    let epub = zip_of(&[
        ("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "o.opf").into_bytes()),
        ("o.opf", opf.as_bytes().to_vec()),
        ("a.xhtml", xhtml("<h2>Början</h2><p>x</p>")),
        ("b.xhtml", xhtml("<p>Utan rubrik</p>")),
    ]);
    let b = Book::load(1, epub).unwrap();
    let toc: Vec<&str> = b.toc.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(toc, ["Början", "Del 2"]);
    assert_eq!(b.chapters[1].title, "Del 2");
    assert!(b.cover.is_none() && b.sections.iter().all(|s| s.cover.is_none()));
}

#[test]
fn drm_tom_läsordning_och_trasiga_filer_ger_fel() {
    // DRM: krypterat innehåll.
    let enc = r#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/><enc:CipherData><enc:CipherReference URI="OEBPS/ch1.xhtml"/></enc:CipherData></enc:EncryptedData></encryption>"#;
    let mut files: Vec<(&str, Vec<u8>)> = vec![("META-INF/container.xml", CONTAINER.as_bytes().to_vec()), ("META-INF/encryption.xml", enc.as_bytes().to_vec())];
    let e = Book::load(1, zip_of(&files)).unwrap_err();
    assert!(e.contains("DRM"), "{e}");
    // Typsnittsobfuskering är inget skydd.
    let font = enc.replace("http://www.w3.org/2001/04/xmlenc#aes128-cbc", "http://www.idpf.org/2008/embedding");
    files[1] = ("META-INF/encryption.xml", font.into_bytes());
    let e = Book::load(1, zip_of(&files)).unwrap_err();
    assert!(!e.contains("DRM"), "obfuskering är ingen DRM: {e}");
    // Tom läsordning.
    let opf = r#"<package><metadata><title>T</title></metadata><manifest/><spine/></package>"#;
    let e = Book::load(1, zip_of(&[("META-INF/container.xml", CONTAINER.as_bytes().to_vec()), ("OEBPS/content.opf", opf.as_bytes().to_vec())])).unwrap_err();
    assert!(e.contains("läsordning"), "{e}");
    // Ingen zip, tom fil, saknad container.
    assert!(Book::load(1, b"inte en zip".to_vec()).is_err());
    assert!(Book::load(1, Vec::new()).is_err());
    assert!(Book::load(1, zip_of(&[("x.txt", b"hej".to_vec())])).is_err());
}

#[test]
fn trasig_xhtml_och_saknade_filer_tolereras() {
    let opf = r#"<package><metadata><title>Slarv</title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/><item id="m" href="saknas.xhtml" media-type="application/xhtml+xml"/>
<item id="b" href="B.XHTML" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/><itemref idref="m"/><itemref idref="b"/></spine></package>"#;
    let epub = zip_of(&[
        ("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "o.opf").into_bytes()),
        ("o.opf", opf.as_bytes().to_vec()),
        ("a.xhtml", b"<html><body><p>oavslutat <b>fet <i>kursiv<p>nytt <table><tr><td>x".to_vec()),
        ("b.xhtml", b"\xEF\xBB\xBF<p>Gemener i namnet, h\xC3\xA5ll</p>".to_vec()),
    ]);
    let b = Book::load(1, epub).unwrap();
    assert_eq!(b.chapters.len(), 2, "filen som saknas hoppas över");
    assert!(b.sections[0].md.contains("oavslutat"));
    assert!(b.sections[1].md.contains("håll"));
}

#[test]
fn stora_kapitel_delas_vid_styckegränser_och_länkmålen_följer_med() {
    let mut body = String::new();
    for i in 0..6000 {
        body.push_str(&format!("<p id=\"p{i}\">Stycke nummer {i} med lite text som fyller ut raden så att kapitlet blir stort nog att delas i flera delar. {}</p>", "ord ".repeat(30)));
    }
    body.push_str("<pre>kod\n\nmed tom rad\n</pre>");
    let opf = r#"<package><metadata><title>Stor</title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#;
    let epub = zip_of(&[
        ("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "o.opf").into_bytes()),
        ("o.opf", opf.as_bytes().to_vec()),
        ("a.xhtml", xhtml(&body)),
    ]);
    let b = Book::load(1, epub).unwrap();
    assert!(b.sections.len() >= 3, "{} delar", b.sections.len());
    assert!(b.sections.iter().all(|s| s.md.len() <= SECTION_LIMIT + 2_000), "delarna är hanterliga");
    assert_eq!(b.chapters.len(), 1);
    assert_eq!(b.chapters[0].section_count, b.sections.len());
    // Ett mål långt in hamnar i rätt del, med byteposition i den delen.
    let d = b.anchors["a.xhtml#p5999"];
    assert!(d.section > 0);
    assert!(b.sections[d.section].md[d.byte..].starts_with("Stycke nummer 5999"), "{:?}", &b.sections[d.section].md[d.byte..d.byte + 30]);
    // Kodblocket med tom rad delades inte mitt i.
    let all: String = b.sections.iter().map(|s| s.md.as_str()).collect::<Vec<_>>().join("");
    assert!(all.contains("```\nkod\n\nmed tom rad\n```"), "{}", &all[all.len() - 80..]);
    for s in &b.sections {
        assert_eq!(s.md.matches("```").count() % 2, 0, "kodstaket är helt i varje del");
    }
}

#[test]
fn delning_vid_tomma_rader_utanför_kodblock() {
    let md = format!("{}\n\n```\nx\n\ny\n```\n\n{}\n", "a".repeat(50), "b".repeat(50));
    #[allow(clippy::single_range_in_vec_init)] // ett enda intervall är väntat resultat
    {
        assert_eq!(split_ranges(&md, 10_000), [0..md.len()]);
    }
    let r = split_ranges(&md, 60);
    let joined: String = r.iter().map(|r| &md[r.clone()]).collect();
    assert_eq!(joined, md);
    assert!(r.len() >= 2);
    for r in &r {
        assert_eq!(md[r.clone()].matches("```").count() % 2, 0, "{:?}", &md[r.clone()]);
    }
    // Ett enda jättestycke delas inte.
    let big = "x".repeat(500);
    assert_eq!(split_ranges(&big, 100).len(), 1);
}

#[test]
fn länkar_tas_bort_ur_noter_och_kopior() {
    assert_eq!(unlink("Se [kapitel](kl:1:2) och [en **fet** länk](kl:3:4)."), "Se kapitel och en **fet** länk.");
    assert_eq!(unlink("[![bild](bytes://x)](kl:0:0) och [web](http://x.se)"), "![bild](bytes://x) och [web](http://x.se)");
    assert_eq!(unlink("[a \\] b](kl:1:1)"), "a \\] b");
    assert_eq!(unlink("ingen ](kl:x) länk"), "ingen ](kl:x) länk");
    assert_eq!(unlink("](kl:1:1) utan öppning"), "](kl:1:1) utan öppning");
}

#[test]
fn ren_text_för_sökning() {
    let l = plain_lines("# Rubrik\n\nEtt **fett** stycke med [länk](http://x) och `kod`.\n\n- punkt ett\n- punkt två\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n");
    assert_eq!(l[0], "Rubrik");
    assert_eq!(l[1], "Ett fett stycke med länk och kod.");
    assert!(l.contains(&"punkt ett".to_owned()) && l.contains(&"punkt två".to_owned()));
    assert!(l.contains(&"A".to_owned()) && l.contains(&"2".to_owned()), "tabellens celler");
    assert!(!l.iter().any(|s| s.contains("http")));
}

#[test]
fn tur_och_retur_med_exporten() {
    let md = "# Min bok\n\nFörsta stycket med *kursiv* och **fet** text.\n\n## Avsnitt ett\n\nEn [länk](https://example.org) och en lista:\n\n- ett\n- två\n\n# Andra kapitlet\n\nSlutet.\n";
    let out = crate::export::epub(md, "Min bok", &|_| None).unwrap();
    let b = Book::load(9, out.bytes).unwrap();
    assert_eq!(b.title(), "Min bok");
    let all: String = b.sections.iter().map(|s| s.md.as_str()).collect::<Vec<_>>().join("\n");
    assert!(all.contains("Första stycket med *kursiv* och **fet** text."), "{all}");
    assert!(all.contains("[länk](https://example.org)"), "{all}");
    assert!(all.contains("- ett\n- två"), "{all}");
    assert!(all.contains("## Avsnitt ett") && all.contains("Slutet."), "{all}");
    assert!(b.toc.len() >= 2, "{:?}", b.toc);
    assert!(b.chapters.iter().filter(|c| c.linear).count() >= 2, "ett kapitel per H1");
}

#[test]
fn kopia_som_markdown_tar_med_bilder_och_tar_bort_länkar() {
    let b = Book::load(5, epub3()).unwrap();
    let (md, images) = b.to_markdown();
    assert!(md.starts_with("# Testboken\n\n*A. Författare, B. Medförfattare*\n\n"), "{md}");
    assert!(md.contains("Börjar här1. Se andra kapitlet.") || md.contains("andra kapitlet"), "{md}");
    assert!(!md.contains("kl:") && !md.contains("bytes://"), "{md}");
    assert_eq!(images.len(), 1, "bara bilden i läsordningen, inte omslaget");
    assert!(md.contains(&format!("![Figur]({}{})", crate::docx::IMAGE_SCHEME, images[0].0)), "{md}");
}

/// Läser en riktig bok (`KLADD_EPUB=sökväg`) och skriver ut hur den tolkades. Körs för hand.
#[test]
#[ignore]
fn riktig_bok_för_hand() {
    let Ok(path) = std::env::var("KLADD_EPUB") else { return };
    let bytes = std::fs::read(&path).unwrap();
    let t = std::time::Instant::now();
    let b = Book::load(1, bytes).unwrap();
    println!("inläst på {:?}: {b:?}, {} ord", t.elapsed(), b.total_words);
    println!("titel {:?} författare {:?} omslag {:?}", b.title(), b.meta.authors, b.cover);
    for c in &b.chapters {
        println!("kapitel {:?} linear={} delar={}", c.title, c.linear, c.section_count);
    }
    for e in b.toc.iter().take(12) {
        println!("toc {}{} -> {:?}", "  ".repeat(e.depth), e.title, e.dest);
    }
    let biggest = b.sections.iter().map(|s| s.md.len()).max().unwrap_or(0);
    println!("största delen {biggest} byte, noter: {}", b.notes.len());
    if let Some(i) = std::env::var("KLADD_SECTION").ok().and_then(|s| s.parse::<usize>().ok()) {
        println!("---- del {i} ----\n{}", b.sections[i].md);
    }
}

/// Skriver provböcker för visuell kontroll (`KLADD_EPUB_OUT=katalog`). Körs för hand.
#[test]
#[ignore]
fn skriv_provböcker() {
    let Ok(dir) = std::env::var("KLADD_EPUB_OUT") else { return };
    std::fs::write(format!("{dir}/prov.epub"), epub3()).unwrap();
    // En bok med ett mycket stort kapitel (över 150 kB text) för att mäta rullningen.
    let mut body = String::from("<h1>Det stora kapitlet</h1>");
    for i in 0..900 {
        body.push_str(&format!(
            "<p>Stycke {i}. Det var en gång en mycket lång berättelse som aldrig tycktes ta slut, med <i>kursiv</i> och <b>fet</b> text samt en <a href=\"#s{i}\">länk</a>. \
             Vi fortsätter att skriva för att fylla ut raderna ordentligt så att varje stycke blir flera rader långt och kapitlet växer över hundrafemtio kilobyte text.</p>"
        ));
        if i % 100 == 0 {
            body.push_str(&format!("<h2 id=\"s{i}\">Avsnitt {i}</h2>"));
        }
    }
    let opf = r#"<package><metadata><title>Stor bok</title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#;
    let epub = zip_of(&[
        ("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "o.opf").into_bytes()),
        ("o.opf", opf.as_bytes().to_vec()),
        ("a.xhtml", xhtml(&body)),
    ]);
    std::fs::write(format!("{dir}/stor.epub"), epub).unwrap();
}

#[test]
fn bok_med_bara_icke_linjära_filer_och_bild_i_läsordningen_läses_ändå() {
    let opf = r#"<package><metadata><title>Märklig</title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/><item id="i" href="b.png" media-type="image/png"/></manifest>
<spine><itemref idref="a" linear="no"/><itemref idref="i" linear="no"/></spine></package>"#;
    let epub = zip_of(&[
        ("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "o.opf").into_bytes()),
        ("o.opf", opf.as_bytes().to_vec()),
        ("a.xhtml", xhtml("<p>Text</p>")),
        ("b.png", png()),
    ]);
    let b = Book::load(2, epub).unwrap();
    assert_eq!(b.linear_chapters(), [0, 1], "inget linjärt: alla läses i ordning");
    assert_eq!(b.step_chapter(0, 1), Some(1));
    assert!(b.sections[1].md.contains("![](bytes://kladd-epub/2/b.png)"), "{}", b.sections[1].md);
    assert!(b.sections[1].cover.is_some(), "en sida med bara en bild ritas som omslag");
    assert!(b.image("b.png").is_some());
    assert!(b.total_words > 0);
}

// ---------------------------------------------------------------- bokmärken, förlagets stil och bibliotek (2.9.0)

/// En liten bok med id, en stilmall (centrerade titlar, indrag) och ett långt kapitel.
fn styled_book(body: &str) -> Vec<u8> {
    let opf = r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="other">urn:isbn:111</dc:identifier><dc:identifier id="bookid">urn:uuid:abc-123</dc:identifier>
<dc:title>Stilboken</dc:title></metadata>
<manifest><item id="css" href="style.css" media-type="text/css"/><item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c1"/></spine></package>"#;
    let page = format!(
        "<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>t</title><link rel=\"stylesheet\" href=\"style.css\"/>\
         <style>.extra {{ text-align: right }}</style></head><body>{body}</body></html>"
    );
    zip_of(&[
        ("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "content.opf").into_bytes()),
        ("content.opf", opf.as_bytes().to_vec()),
        ("style.css", b"h1 { text-align: center; font-variant: small-caps }\np { text-indent: 1.5em; margin: 0 }\np.first { text-indent: 0 }".to_vec()),
        ("ch1.xhtml", page.into_bytes()),
    ])
}

#[test]
fn bokens_id_följer_paketets_unique_identifier_och_stilen_når_delarna() {
    let b = Book::load(1, styled_book("<h1>Titel</h1><p class=\"first\">Första.</p><p>Andra.</p><p class=\"extra\">Höger.</p>")).unwrap();
    assert_eq!(b.meta.identifier, "urn:uuid:abc-123", "id:t som unique-identifier pekar på, inte det första");
    let sec = &b.sections[b.chapters[0].first_section];
    let look = |needle: &str| sec.blocks.iter().find(|(p, _)| *p == sec.md.find(needle).unwrap()).map(|(_, l)| l.clone());
    let h = look("# Titel").expect("rubrikens utseende från den länkade stilmallen");
    assert_eq!((h.align, h.small_caps), (Some(css::Align::Center), true));
    assert!(look("Första").is_none(), "p.first (text-indent: 0) vinner över p och ger inget särskilt utseende");
    assert_eq!(look("Andra").unwrap().indent, 1.5);
    assert_eq!(look("Höger").unwrap().align, Some(css::Align::Right), "<style> i sidan läses också");
}

#[test]
fn bokmärkenas_plats_hittas_igen_oavsett_hur_kapitlet_delas_i_delar() {
    use super::anchor::{bookmark_at, locate, make, resolve};
    let mut body = String::from("<h1>Långt</h1>");
    // Så långt att kapitlet delas vid `SECTION_LIMIT`.
    for i in 0..9000 {
        body.push_str(&format!("<p>Stycke nummer {i} med en del text som fyller ut raden och gör kapitlet långt nog att delas.</p>"));
    }
    let b = Book::load(2, styled_book(&body)).unwrap();
    let ch = 0;
    let md = b.chapter_md(ch);
    assert!(b.chapters[ch].section_count >= 2, "kapitlet delas");
    assert_eq!(md.len(), b.sections.iter().map(|s| s.md.len()).sum::<usize>());
    // Markera en mening långt in och sätt ett bokmärke; båda går att slå upp i delen de ligger i.
    let r = locate(&md, "Stycke nummer 333 med en del text", 0).unwrap();
    let note = make(1, ch, &md, r.clone(), false);
    let bm = bookmark_at(2, ch, &md, md.find("Stycke nummer 250").unwrap());
    for (n, want) in [(&note, "Stycke nummer 333"), (&bm, "Stycke nummer 250")] {
        let at = resolve(&md, n).unwrap();
        let d = b.dest_in_chapter(ch, at.start).unwrap();
        assert!(b.sections[d.section].md[d.byte..].starts_with(want), "{want}: {:?}", &b.sections[d.section].md[d.byte..d.byte + 30]);
    }
    // Delas kapitlet på ett annat sätt (annan gräns) hittas samma ställe: kapitlets text är densamma.
    for limit in [3_000, 9_000, 50_000] {
        let ranges = split_ranges(&md, limit);
        let (si, rg) = ranges.iter().enumerate().find(|(_, rg)| rg.contains(&r.start)).unwrap();
        assert!(md[rg.start..][r.start - rg.start..].starts_with("Stycke nummer 333"), "gräns {limit}, del {si}");
        assert_eq!(rg.start + (r.start - rg.start), r.start);
    }
}

#[test]
fn sparat_läge_från_gamla_delningen_räknas_om_till_rätt_ställe() {
    let mut body = String::new();
    for i in 0..400 {
        body.push_str(&format!("<p>Stycke nummer {i} med lite text som fyller ut raden. {}</p>", "ord ".repeat(30)));
    }
    let opf = r#"<package><metadata><title>Stor</title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#;
    let epub = zip_of(&[
        ("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "o.opf").into_bytes()),
        ("o.opf", opf.as_bytes().to_vec()),
        ("a.xhtml", xhtml(&body)),
    ]);
    let b = Book::load(1, epub).unwrap();
    assert_eq!(b.sections.len(), 1, "ett kapitel på ungefär 80 kB delas inte längre");
    let md = b.chapter_md(0);
    // Gamla delningen vid 24 000 byte: del 2 (nr 2), hälften in.
    let old = split_ranges(&md, 24_000);
    assert!(old.len() >= 3);
    let (s, byte) = b.remap_position(2, 0.5, 24_000).expect("läget finns");
    assert_eq!(s, 0);
    let want = old[2].start + old[2].len() / 2;
    assert_eq!(byte, want);
    assert!(b.remap_position(99, 0.0, 24_000).is_none());
}
