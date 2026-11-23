//! Bildformat som Kladd kan infoga men som Word, OpenDocument och PDF inte alla kan bädda in:
//! WebP görs om till PNG när bilden läses för ritning eller export. SVG får en PNG som raster
//! (visning, utskrift, reserv) men behåller sin källa (`docx::Image::svg`), som skrivs som vektor i
//! .docx, .odt, PDF och HTML/EPUB. De andra formaten (png, jpeg, gif, bmp) går orörda igenom.

use std::io::Cursor;

/// Filändelser i "Infoga bild", dra och släpp och loggaväljaren.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "svg"];

/// Längsta sida (bildpunkter) när en SVG rasteriseras.
const SVG_MAX_SIDE: f32 = 2400.0;
/// SVG ritas i dubbel upplösning (skarp på hög-DPI och i PDF) men aldrig över `SVG_MAX_SIDE`.
const SVG_SCALE: f32 = 2.0;

pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// Är det en SVG-fil? Ser efter `<svg` i början (efter BOM och XML-deklaration).
pub fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    let head = String::from_utf8_lossy(head);
    let t = head.trim_start_matches('\u{feff}').trim_start();
    t.starts_with('<') && t.contains("<svg")
}

/// Den naturliga storleken (CSS-pixlar) hos en SVG, eller `None` om den inte går att tolka.
pub fn svg_size(bytes: &[u8]) -> Option<(f32, f32)> {
    let tree = parse_svg(bytes)?;
    let s = tree.size();
    (s.width() > 0.0 && s.height() > 0.0).then(|| (s.width(), s.height()))
}

pub(crate) fn parse_svg(bytes: &[u8]) -> Option<resvg::usvg::Tree> {
    let mut opt = resvg::usvg::Options::default();
    // Systemets typsnitt läses bara in när SVG:n har text (det tar en stund).
    if bytes.windows(5).any(|w| w == b"<text") {
        opt.fontdb_mut().load_system_fonts();
    }
    resvg::usvg::Tree::from_data(bytes, &opt).ok()
}

/// Ritar en SVG som PNG. Externa filer som SVG:n pekar på läses inte.
pub fn svg_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let tree = parse_svg(bytes)?;
    let size = tree.size();
    if !(size.width() > 0.0 && size.height() > 0.0) {
        return None;
    }
    let scale = SVG_SCALE.min(SVG_MAX_SIDE / size.width().max(size.height()));
    let (w, h) = ((size.width() * scale).ceil().max(1.0) as u32, (size.height() * scale).ceil().max(1.0) as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}

/// WebP (första bildrutan) som PNG.
pub fn webp_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP).ok()?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Gör om WebP och SVG till PNG; övriga bytes lämnas som de är. `None` om en WebP/SVG inte kan avkodas.
pub fn to_supported(bytes: Vec<u8>) -> Option<Vec<u8>> {
    if is_webp(&bytes) {
        webp_to_png(&bytes)
    } else if is_svg(&bytes) {
        svg_to_png(&bytes)
    } else {
        Some(bytes)
    }
}

/// Läser en bildfil till en [`Image`]: SVG behåller sin källa (för vektorskrivning) och får en PNG som
/// raster för visning och reserv; övriga format går som i [`to_supported`]. `None` om bilden inte kan avkodas.
pub fn prepare(bytes: Vec<u8>) -> Option<crate::docx::Image> {
    if is_svg(&bytes) {
        let size = svg_size(&bytes)?;
        let png = svg_to_png(&bytes)?;
        return Some(crate::docx::Image { bytes: png, extent: None, svg: Some(crate::docx::Svg { data: bytes, size }) });
    }
    Some(crate::docx::Image::raster(to_supported(bytes)?, None))
}

/// En SVG-del ur ett paket (`.docx`/`.odt`) med PNG-reserven som raster; saknas reserven (eller går
/// den inte att avkoda) ritas SVG:n.
pub fn from_package(svg: Vec<u8>, fallback: Option<Vec<u8>>) -> Option<crate::docx::Image> {
    if !is_svg(&svg) {
        return None;
    }
    let size = svg_size(&svg)?;
    let png = match fallback.filter(|b| crate::docx::is_supported_image(b)) {
        Some(b) => b,
        None => svg_to_png(&svg)?,
    };
    Some(crate::docx::Image { bytes: png, extent: None, svg: Some(crate::docx::Svg { data: svg, size }) })
}

/// Är filen en bild som Kladd kan infoga (efter omvandling)?
pub fn insertable(bytes: &[u8]) -> bool {
    crate::docx::image_ext(bytes).is_some() || is_webp(bytes) || is_svg(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SVG: &str = r##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#ff0000"/><circle cx="10" cy="10" r="5" fill="#0000ff"/></svg>"##;

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(w, h, image::Rgba([10, 200, 30, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    pub(crate) fn webp_bytes() -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(12, 8, image::Rgba([10, 200, 30, 255])).write_to(&mut out, image::ImageFormat::WebP).unwrap();
        out.into_inner()
    }

    #[test]
    fn sniffs_formats() {
        assert!(is_svg(SVG.as_bytes()));
        assert!(is_svg(b"\xef\xbb\xbf  <svg width=\"1\" height=\"1\"/>"));
        assert!(!is_svg(b"<html><body>hej</body></html>"));
        assert!(!is_svg(&png_of(2, 2)));
        assert!(is_webp(&webp_bytes()));
        assert!(!is_webp(&png_of(2, 2)));
        assert!(insertable(SVG.as_bytes()) && insertable(&webp_bytes()) && insertable(&png_of(2, 2)));
        assert!(!insertable(b"vanlig text"));
    }

    #[test]
    fn webp_becomes_png_with_same_size() {
        let png = to_supported(webp_bytes()).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (12, 8));
        assert_eq!(img.get_pixel(3, 3).0, [10, 200, 30, 255]);
    }

    #[test]
    fn svg_is_rasterized_at_double_size() {
        assert_eq!(svg_size(SVG.as_bytes()), Some((40.0, 20.0)));
        let png = to_supported(SVG.as_bytes().to_vec()).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (80, 40));
        assert_eq!(img.get_pixel(60, 30).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(20, 20).0, [0, 0, 255, 255]);
    }

    #[test]
    fn huge_svg_is_capped() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="100"/>"#;
        let img = image::load_from_memory(&svg_to_png(svg.as_bytes()).unwrap()).unwrap();
        assert!(img.width() <= 2401);
    }

    #[test]
    fn broken_images_give_none_and_plain_formats_pass() {
        assert!(to_supported(b"RIFF\0\0\0\0WEBPgarbage".to_vec()).is_none());
        assert!(to_supported(b"<svg><oops".to_vec()).is_none());
        let png = png_of(3, 3);
        assert_eq!(to_supported(png.clone()), Some(png));
    }

    fn dir_with_images(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("foto.webp"), webp_bytes()).unwrap();
        std::fs::write(dir.join("logo.svg"), SVG).unwrap();
        dir
    }

    fn zip_has_png(bytes: &[u8], prefix: &str) -> usize {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_owned()).collect();
        names.iter().filter(|n| n.starts_with(prefix) && n.ends_with(".png")).count()
    }

    #[test]
    fn webp_and_svg_export_to_docx_odt_and_pdf_as_png() {
        let dir = dir_with_images("kladd-test-imgfmt-export");
        let mut d = crate::doc::Document::new_untitled(1, 1);
        d.text = "Foto: ![Ett foto](foto.webp)\n\n<img src=\"logo.svg\" alt=\"Logga\" width=\"40\">\n".into();
        for (file, prefix) in [("ut.docx", "word/media/"), ("ut.odt", "Pictures/")] {
            let out = dir.join(file);
            let warnings = d.save_to(&out).unwrap();
            assert!(warnings.is_empty(), "{file}: {warnings:?}");
            assert_eq!(zip_has_png(&std::fs::read(&out).unwrap(), prefix), 2, "{file}");
            let back = crate::doc::Document::load(2, &out).unwrap();
            assert_eq!(back.images.len(), 2, "{file}");
        }
        let pdf = crate::pdf_write::write(&d.text, "Prov", &|src| crate::doc::resolve_image(src, &dir, &[])).unwrap();
        assert!(pdf.bytes.starts_with(b"%PDF") && pdf.missing_images.is_empty(), "{:?}", pdf.missing_images);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_image_converts_local_files() {
        let dir = dir_with_images("kladd-test-imgfmt-resolve");
        for f in ["foto.webp", "logo.svg", "saknas.svg"] {
            let r = crate::doc::resolve_image(f, &dir, &[]);
            assert_eq!(r.is_some(), f != "saknas.svg", "{f}");
            if let Some(i) = r {
                assert!(i.bytes.starts_with(&[0x89, b'P']), "{f}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
