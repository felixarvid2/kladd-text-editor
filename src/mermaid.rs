//! ```mermaid-block som diagram: flödesscheman, sekvens-, tillstånds-, klass- och cirkeldiagram med flera
//! ritas av den rena Rust-crate:n `mermaid-svg` (MIT, enda beroendet är `thiserror`; ingen JavaScript,
//! ingen webbläsare och inget C) som SVG. SVG:n går sedan genom Kladds vanliga SVG-väg (`imgfmt`):
//! raster för visning, vektor i PDF. Kan ett block inte ritas (syntaxfel, okänd diagramtyp) står det
//! kvar som kod. Läsläget (`replace`), dokumentläget (`word::codeui`) och PDF-exporten använder det.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

use mermaid_svg::Theme;

/// Prefix för diagrammets bildkälla i PDF-exporten och för layoutens bildstorlekar (`mermaid:<nyckel>`).
pub const SRC: &str = "mermaid:";

/// Antal ritade diagram som cachas.
const CACHE_MAX: usize = 48;

/// Ett ritat diagram.
pub struct Rendered {
    /// Raster av SVG:n i den skala det bad om.
    pub png: Vec<u8>,
    /// Diagrammets naturliga storlek (CSS-pixlar).
    pub size: (f32, f32),
}

#[derive(Default)]
struct Store {
    rendered: HashMap<(u64, bool, u32), Option<Arc<Rendered>>>,
    /// PDF: bildkälla -> färdig bild.
    images: HashMap<String, crate::docx::Image>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static REGISTERED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn with_store<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Store::default))
}

/// Är språket i inhägnaden `mermaid`?
pub fn is_lang(lang: &str) -> bool {
    crate::codehl::lang_of(lang) == "mermaid"
}

fn hash_code(code: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    code.trim().hash(&mut h);
    h.finish()
}

/// Nyckeln för ett diagram (innehållet): samma kod ger samma nyckel.
pub fn key(code: &str) -> String {
    format!("{SRC}{:016x}", hash_code(code))
}

/// Mermaid-koden som SVG, med uttrycklig storlek. `None` om den inte går att rita.
pub fn svg(code: &str, dark: bool) -> Option<String> {
    if code.trim().is_empty() {
        return None;
    }
    let theme = if dark { Theme::dark() } else { Theme::default_theme() };
    // Biblioteket ska inte kunna ta ner programmet på konstig indata.
    let out = std::panic::catch_unwind(AssertUnwindSafe(|| mermaid_svg::render_with(code, &theme))).ok()?.ok()?;
    Some(with_size(&out))
}

/// `width="100%"` (bara `viewBox`) görs om till viewBox-måtten: övriga SVG-läsare vill ha en storlek.
fn with_size(svg: &str) -> String {
    let Some(i) = svg.find("viewBox=\"") else { return svg.to_owned() };
    let rest = &svg[i + 9..];
    let Some(end) = rest.find('"') else { return svg.to_owned() };
    let nums: Vec<f32> = rest[..end].split_whitespace().filter_map(|n| n.parse().ok()).collect();
    if nums.len() != 4 || nums[2] <= 0.0 || nums[3] <= 0.0 {
        return svg.to_owned();
    }
    svg.replacen("width=\"100%\"", &format!("width=\"{}\" height=\"{}\"", nums[2], nums[3]), 1)
}

/// Ritar diagrammet (cachat per innehåll, tema och skala). `scale` är rasterets skala mot SVG:ns storlek.
pub fn rendered(code: &str, dark: bool, scale: f32) -> Option<Arc<Rendered>> {
    let k = (hash_code(code), dark, scale.to_bits());
    if let Some(r) = with_store(|s| s.rendered.get(&k).cloned()) {
        return r;
    }
    let r = (|| {
        let svg = svg(code, dark)?.into_bytes();
        let tree = crate::imgfmt::parse_svg(&svg)?;
        let size = tree.size();
        if !(size.width() > 0.0 && size.height() > 0.0) {
            return None;
        }
        let scale = scale.min(2400.0 / size.width().max(size.height()));
        let (w, h) = ((size.width() * scale).ceil().max(1.0) as u32, (size.height() * scale).ceil().max(1.0) as u32);
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        Some(Arc::new(Rendered { png: pixmap.encode_png().ok()?, size: (size.width(), size.height()) }))
    })();
    with_store(|s| {
        if s.rendered.len() >= CACHE_MAX {
            s.rendered.clear();
        }
        s.rendered.insert(k, r.clone());
    });
    r
}

/// Webbadressen (egui:s minnesbilder) som rasterets bytes registreras under.
pub fn uri(code: &str, dark: bool, scale: f32) -> String {
    format!("bytes://kladd-mermaid/{:016x}-{}-{}.png", hash_code(code), u8::from(dark), scale as u32)
}

/// Ritar diagrammet och registrerar rasterbilden hos `ctx`. Ger adressen och diagrammets storlek.
pub fn include(ctx: &egui::Context, code: &str, dark: bool, scale: f32) -> Option<(String, (f32, f32))> {
    let r = rendered(code, dark, scale)?;
    let uri = uri(code, dark, scale);
    let fresh = {
        let mut g = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashSet::new).insert(uri.clone())
    };
    if fresh {
        ctx.include_bytes(uri.clone(), r.png.clone());
    }
    Some((uri, r.size))
}

/// Diagrammet som bild för exporterna (ljust tema; SVG som vektor och PNG som reserv). Bilden hämtas
/// sedan med [`resolve`] under den nyckel som ges.
pub fn image(code: &str) -> Option<String> {
    let k = key(code);
    if with_store(|s| s.images.contains_key(&k)) {
        return Some(k);
    }
    let svg = svg(code, false)?;
    let img = crate::imgfmt::prepare(svg.into_bytes())?;
    with_store(|s| s.images.insert(k.clone(), img));
    Some(k)
}

/// Bilden för en källa som [`image`] gett (`mermaid:<nyckel>`).
pub fn resolve(src: &str) -> Option<crate::docx::Image> {
    src.starts_with(SRC).then(|| with_store(|s| s.images.get(src).cloned())).flatten()
}

/// Mermaid-blocken i `md`: (blockets byteintervall, koden).
pub fn blocks(md: &str) -> Vec<(Range<usize>, String)> {
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
    if !md.contains("mermaid") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur: Option<(Range<usize>, String)> = None;
    for (ev, r) in Parser::new(md).into_offset_iter() {
        match ev {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) if is_lang(&info) => cur = Some((r, String::new())),
            Event::Text(t) => {
                if let Some((_, c)) = cur.as_mut() {
                    c.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) => out.extend(cur.take()),
            _ => {}
        }
    }
    out
}

/// Läsläget: varje ritbart ```mermaid-block byts mot en bildlänk (bilden registreras hos `ctx`); övriga
/// block och block som inte går att rita står kvar som kod.
pub fn replace(md: &str, ctx: &egui::Context, dark: bool) -> String {
    let found = blocks(md);
    if found.is_empty() {
        return md.to_owned();
    }
    let mut out = md.to_owned();
    for (range, code) in found.into_iter().rev() {
        if let Some((uri, _)) = include(ctx, &code, dark, 1.0) {
            out.replace_range(range, &format!("![diagram]({uri})\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOW: &str = "graph TD\n    A[Start] --> B{Klart?}\n    B -->|Ja| C[Slut]\n    B -->|Nej| A\n";

    #[test]
    fn flowchart_becomes_an_svg_with_a_size() {
        let svg = svg(FLOW, false).expect("flödesschema");
        assert!(svg.starts_with("<svg") && svg.contains("Start"));
        // Uttrycklig storlek: inte `width="100%"`.
        assert!(!svg.contains("width=\"100%\""), "{svg}");
        let r = rendered(FLOW, false, 1.0).unwrap();
        assert!(r.size.0 > 50.0 && r.size.1 > 50.0, "{:?}", r.size);
        assert!(r.png.starts_with(&[0x89, b'P', b'N', b'G']));
        // Dubbel skala ger ett större raster men samma storlek.
        let r2 = rendered(FLOW, false, 2.0).unwrap();
        assert_eq!(r.size, r2.size);
        assert!(r2.png.len() > r.png.len() / 2);
    }

    #[test]
    fn other_diagram_types_work_and_garbage_does_not() {
        assert!(svg("sequenceDiagram\n    A->>B: Hej\n", false).is_some());
        assert!(svg("pie\n    \"A\" : 1\n    \"B\" : 2\n", false).is_some());
        assert!(svg("det här är inget diagram", false).is_none());
        assert!(svg("", false).is_none());
    }

    #[test]
    fn dark_theme_differs_from_the_light_one() {
        assert_ne!(svg(FLOW, true), svg(FLOW, false));
    }

    #[test]
    fn blocks_are_found_in_markdown() {
        let md = format!("Före\n\n```mermaid\n{FLOW}```\n\n```rust\nfn main() {{}}\n```\n\nEfter\n");
        let b = blocks(&md);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].1, FLOW);
        assert!(md[b[0].0.clone()].starts_with("```mermaid"));
        assert!(blocks("```rust\nx\n```\n").is_empty());
    }

    #[test]
    fn export_image_has_svg_source_and_png_fallback() {
        let k = image(FLOW).expect("bild");
        assert!(k.starts_with(SRC));
        let img = resolve(&k).unwrap();
        assert!(img.svg.is_some(), "vektorkällan följer med");
        assert!(img.bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(resolve("bild:x.png").is_none());
        assert!(image("inget diagram alls").is_none());
    }
}
