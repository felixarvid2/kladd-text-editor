//! SVG som vektorgrafik: skrivs som SVG-del (med PNG-reserv) i .docx och .odt, som vektor i PDF och
//! som SVG-fil i HTML/EPUB; tur och retur bevarar SVG-bytes.

use crate::docx::{Image, IMAGE_SCHEME};
use crate::imgfmt::tests::SVG;
use std::io::{Cursor, Read};

fn img() -> Image {
    crate::imgfmt::prepare(SVG.as_bytes().to_vec()).unwrap()
}

fn resolve(src: &str) -> Option<Image> {
    (src == "logo.svg").then(img)
}

fn zip_parts(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut d = Vec::new();
            f.read_to_end(&mut d).unwrap();
            (f.name().to_owned(), d)
        })
        .collect()
}

fn text_of(parts: &[(String, Vec<u8>)], name: &str) -> String {
    String::from_utf8_lossy(&parts.iter().find(|(n, _)| n == name).unwrap().1).into_owned()
}

const MD: &str = "# Logga\n\nFöre ![Logotyp](logo.svg) efter.\n";

#[test]
fn prepared_svg_keeps_source_and_natural_size() {
    let i = img();
    assert_eq!(i.svg.as_ref().unwrap().data, SVG.as_bytes());
    assert_eq!(i.natural_px(), Some((40.0, 20.0)));
    // Rastret är dubbelt så stort men får inte styra storleken.
    assert_eq!(imagesize::blob_size(&i.bytes).map(|s| (s.width, s.height)).unwrap(), (80, 40));
}

#[test]
fn svg_without_width_uses_viewbox_size() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 60 30"><rect width="60" height="30" fill="red"/></svg>"#;
    let i = crate::imgfmt::prepare(svg.to_vec()).unwrap();
    assert_eq!(i.natural_px(), Some((60.0, 30.0)));
    let w = crate::docx::write("![](a.svg)\n", "x", &|_| Some(i.clone()), None).unwrap();
    let doc = text_of(&zip_parts(&w.bytes), "word/document.xml");
    // 60 px = 571500 EMU (inte 120 px).
    assert!(doc.contains(r#"<wp:extent cx="571500" cy="285750"/>"#), "{doc}");
}

#[test]
fn docx_writes_svg_part_with_png_fallback() {
    let w = crate::docx::write(MD, "x", &resolve, None).unwrap();
    assert!(w.missing_images.is_empty());
    let parts = zip_parts(&w.bytes);
    let svg = parts.iter().find(|(n, _)| n.starts_with("word/media/") && n.ends_with(".svg")).expect("svg-del");
    assert_eq!(svg.1, SVG.as_bytes());
    assert!(parts.iter().any(|(n, d)| n.starts_with("word/media/") && n.ends_with(".png") && d.starts_with(&[0x89, b'P'])), "PNG-reserv");
    let doc = text_of(&parts, "word/document.xml");
    assert!(doc.contains("{96DAC541-7B7A-43D3-8B79-37D633B846F1}"));
    assert!(doc.contains("asvg:svgBlip"));
    assert!(doc.contains("http://schemas.microsoft.com/office/drawing/2016/SVG/main"));
    let rels = text_of(&parts, "word/_rels/document.xml.rels");
    assert_eq!(rels.matches("relationships/image").count(), 2);
    assert!(text_of(&parts, "[Content_Types].xml").contains(r#"Extension="svg" ContentType="image/svg+xml""#));
    // a:blip pekar på PNG:n, svgBlip på SVG:n.
    let blip_id = doc.split("<a:blip r:embed=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
    let svg_id = doc.split("<asvg:svgBlip").nth(1).unwrap().split("r:embed=\"").nth(1).unwrap().split('"').next().unwrap().to_owned();
    assert_ne!(blip_id, svg_id);
    assert!(rels.contains(&format!("Id=\"{svg_id}\"")) && rels.contains(&format!("Id=\"{blip_id}\"")));
}

#[test]
fn docx_roundtrip_keeps_svg_bytes() {
    let w = crate::docx::write(MD, "x", &resolve, None).unwrap();
    let back = crate::docx::read(&w.bytes).unwrap();
    assert!(back.dropped.is_empty(), "{:?}", back.dropped);
    assert_eq!(back.images.len(), 1);
    let (name, im) = &back.images[0];
    assert_eq!(im.svg.as_ref().unwrap().data, SVG.as_bytes());
    assert_eq!(im.natural_px(), Some((40.0, 20.0)));
    assert_eq!(back.markdown, format!("# Logga\n\nFöre ![Logotyp]({IMAGE_SCHEME}{name}) efter.\n"));
    // Skrivs igen (nu med originalpaketet och utan): SVG-delen finns kvar, en gång.
    let imgs = back.images.clone();
    let res = move |s: &str| imgs.iter().find(|(n, _)| Some(n.as_str()) == s.strip_prefix(IMAGE_SCHEME)).map(|(_, i)| i.clone());
    for source in [None, back.source.as_ref()] {
        let w2 = crate::docx::write(&back.markdown, "x", &res, source).unwrap();
        let parts = zip_parts(&w2.bytes);
        let svgs: Vec<_> = parts.iter().filter(|(n, _)| n.ends_with(".svg")).collect();
        assert_eq!(svgs.len(), 1);
        assert_eq!(svgs[0].1, SVG.as_bytes());
        let again = crate::docx::read(&w2.bytes).unwrap();
        assert_eq!(again.images[0].1.svg.as_ref().unwrap().data, SVG.as_bytes());
    }
}

#[test]
fn docx_edit_reuses_existing_svg_and_png_parts() {
    let w = crate::docx::write(MD, "x", &resolve, None).unwrap();
    let back = crate::docx::read(&w.bytes).unwrap();
    let imgs = back.images.clone();
    let res = move |s: &str| imgs.iter().find(|(n, _)| Some(n.as_str()) == s.strip_prefix(IMAGE_SCHEME)).map(|(_, i)| i.clone());
    // Texten ändras: stycket skrivs om, bilddelarna i originalet återanvänds.
    let md = back.markdown.replace("Före", "Efter ändring, före");
    let w2 = crate::docx::write(&md, "x", &res, back.source.as_ref()).unwrap();
    let parts = zip_parts(&w2.bytes);
    assert_eq!(parts.iter().filter(|(n, _)| n.starts_with("word/media/")).count(), 2);
    assert!(text_of(&parts, "word/document.xml").contains("asvg:svgBlip"));
}

#[test]
fn odt_writes_svg_and_png_fallback_and_roundtrips() {
    let w = crate::odt::write(MD, "x", &resolve).unwrap();
    let parts = zip_parts(&w.bytes);
    let svg = parts.iter().find(|(n, _)| n.starts_with("Pictures/") && n.ends_with(".svg")).expect("svg-del");
    assert_eq!(svg.1, SVG.as_bytes());
    assert!(parts.iter().any(|(n, _)| n.starts_with("Pictures/") && n.ends_with(".png")));
    let content = text_of(&parts, "content.xml");
    assert_eq!(content.matches("<draw:image ").count(), 2);
    let (svg_at, png_at) = (content.find(".svg\"").unwrap(), content.find(".png\"").unwrap());
    assert!(svg_at < png_at, "SVG först, PNG-reserv efter");
    let manifest = text_of(&parts, "META-INF/manifest.xml");
    assert!(manifest.contains("image/svg+xml") && manifest.contains(".svg\""));
    let back = crate::odt::read(&w.bytes).unwrap();
    assert!(back.dropped.is_empty(), "{:?}", back.dropped);
    assert_eq!(back.images.len(), 1);
    assert_eq!(back.images[0].1.svg.as_ref().unwrap().data, SVG.as_bytes());
    assert_eq!(back.images[0].1.natural_px(), Some((40.0, 20.0)));
    assert!(back.markdown.contains("![Logotyp](bild:"), "{}", back.markdown);
}

#[test]
fn pdf_draws_svg_as_vector() {
    let w = crate::pdf_write::write(MD, "x", &resolve).unwrap();
    assert!(w.missing_images.is_empty());
    let doc = lopdf::Document::load_mem(&w.bytes).unwrap();
    let mut path_ops = 0;
    for (_, obj) in doc.objects.iter() {
        if let lopdf::Object::Stream(s) = obj {
            assert_ne!(s.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()), Some(&b"Image"[..]), "SVG ska inte bli ett bildobjekt");
            if let Ok(data) = s.decompressed_content() {
                if let Ok(c) = lopdf::content::Content::decode(&data) {
                    path_ops += c.operations.iter().filter(|o| matches!(o.operator.as_str(), "re" | "c" | "l" | "f" | "f*" | "h")).count();
                }
            }
        }
    }
    assert!(path_ops >= 4, "förväntade banoperatorer, fick {path_ops}");
    // Ett raster ger däremot ett bildobjekt.
    let raster = crate::pdf_write::write(MD, "x", &|_| Some(Image::raster(img().bytes, None))).unwrap();
    let doc = lopdf::Document::load_mem(&raster.bytes).unwrap();
    assert!(doc.objects.values().any(|o| matches!(o, lopdf::Object::Stream(s) if s.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) == Some(&b"Image"[..]))));
}

#[test]
fn html_and_epub_keep_the_svg_file() {
    let h = String::from_utf8(crate::export::html(MD, "T", &resolve).unwrap().bytes).unwrap();
    assert!(h.contains("data:image/svg+xml;base64,"), "{h}");
    assert!(!h.contains("data:image/png"));
    let e = crate::export::epub(MD, "T", &resolve).unwrap();
    let parts = zip_parts(&e.bytes);
    let svg = parts.iter().find(|(n, _)| n.starts_with("OEBPS/images/") && n.ends_with(".svg")).expect("svg i e-boken");
    assert_eq!(svg.1, SVG.as_bytes());
    assert!(text_of(&parts, "OEBPS/content.opf").contains("image/svg+xml"));
    assert!(parts.iter().any(|(n, d)| n.starts_with("OEBPS/") && n.ends_with(".xhtml") && String::from_utf8_lossy(d).contains("images/img1.svg")));
}

#[test]
fn resolve_image_keeps_svg_for_files() {
    let dir = std::env::temp_dir().join("kladd-svg-tests");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("logo.svg"), SVG).unwrap();
    let i = crate::doc::resolve_image("logo.svg", &dir, &[]).unwrap();
    assert!(i.bytes.starts_with(&[0x89, b'P']));
    assert_eq!(i.svg.unwrap().data, SVG.as_bytes());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `KLADD_SVG_OUT=mapp cargo test dump_svg_files -- --ignored` skriver prov.docx, prov.odt, prov.pdf och
/// prov.html med en SVG (former och text) för att öppna dem i Word/LibreOffice.
#[test]
#[ignore]
fn dump_svg_files() {
    let Some(dir) = std::env::var_os("KLADD_SVG_OUT") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 120"><defs><linearGradient id="g"><stop offset="0" stop-color="#2a6fdb"/><stop offset="1" stop-color="#e8b100"/></linearGradient></defs><rect width="240" height="120" rx="14" fill="url(#g)"/><circle cx="52" cy="60" r="30" fill="#fff" fill-opacity="0.85"/><path d="M110 85 L150 25 L190 85 Z" fill="none" stroke="#102030" stroke-width="5"/><text x="12" y="112" font-size="16" font-family="sans-serif" fill="#102030">Vektor SVG</text></svg>"##;
    let res = move |_: &str| crate::imgfmt::prepare(svg.as_bytes().to_vec());
    let md = "# SVG som vektor\n\nFöre bilden.\n\n![Logotyp](x.svg)\n\nEfter bilden.\n";
    std::fs::write(dir.join("prov.docx"), crate::docx::write(md, "Prov", &res, None).unwrap().bytes).unwrap();
    std::fs::write(dir.join("prov.odt"), crate::odt::write(md, "Prov", &res).unwrap().bytes).unwrap();
    std::fs::write(dir.join("prov.pdf"), crate::pdf_write::write(md, "Prov", &res).unwrap().bytes).unwrap();
    std::fs::write(dir.join("prov.html"), crate::export::html(md, "Prov", &res).unwrap().bytes).unwrap();
}
