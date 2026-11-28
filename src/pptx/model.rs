//! Presentationens modell: `presentation.xml` (bildstorlek, `firstSlideNum`, `defaultTextStyle`,
//! inbäddade typsnitt, bildordning), bilder, layouter, mallar och teman.
//!
//! Formträden behålls som XML-träd (`Node`) och tolkas när de ritas (`render.rs`), med arvet i
//! `inherit.rs`. Det håller modellen liten och gör att inget i filen går förlorat.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::color::{self, ColorCtx, Rgba};
use super::fonts::FontBook;
use super::package::{self, Package, Rel};
use super::scene::ImageData;
use super::xml::Node;

/// EMU per punkt.
pub const EMU_PER_PT: f64 = 12700.0;

/// Ett tema: färgschema, typsnittsschema och stilmatris.
#[derive(Clone, Debug, Default)]
pub struct Theme {
    pub colors: HashMap<String, Rgba>,
    pub major: FontScheme,
    pub minor: FontScheme,
    /// `a:fillStyleLst`, `a:lnStyleLst`, `a:effectStyleLst`, `a:bgFillStyleLst` (barnen i ordning).
    pub fill_styles: Vec<Node>,
    pub line_styles: Vec<Node>,
    pub effect_styles: Vec<Node>,
    pub bg_fill_styles: Vec<Node>,
    /// `a:objectDefaults` (`spDef`, `lnDef`, `txDef`).
    pub object_defaults: Option<Node>,
    /// Temadelens relationer (bilder i `a:bgFillStyleLst`/`a:fillStyleLst`).
    pub rels: Vec<Rel>,
}

/// Ett typsnittsschema (`a:majorFont`/`a:minorFont`).
#[derive(Clone, Debug, Default)]
pub struct FontScheme {
    pub latin: String,
    pub ea: String,
    pub cs: String,
    /// Skriftspecifika typsnitt (`Jpan` → `Yu Gothic` …).
    pub scripts: HashMap<String, String>,
}

/// En mall (`p:sldMaster`).
pub struct Master {
    /// Delnamnet (för källbevarande redigering i nästa omgång).
    #[allow(dead_code)]
    pub part: String,
    pub root: Node,
    pub rels: Vec<Rel>,
    pub theme: Option<usize>,
    /// Mallens `otherStyle` utan teckenstorlekarna (för former utan platshållare, se `TxKind::Free`).
    pub other_free: Option<Node>,
}

/// En layout (`p:sldLayout`).
pub struct Layout {
    #[allow(dead_code)]
    pub part: String,
    pub root: Node,
    pub rels: Vec<Rel>,
    pub master: Option<usize>,
}

/// En bild (`p:sld`).
pub struct Slide {
    #[allow(dead_code)]
    pub part: String,
    pub root: Node,
    pub rels: Vec<Rel>,
    pub layout: Option<usize>,
    /// `show="0"`: dold i bildspel och PDF.
    pub hidden: bool,
    /// Anteckningsdelen, om den finns (visas av nästa omgång).
    #[allow(dead_code)]
    pub notes: Option<String>,
}

/// SmartArts sparade ritdel (`ppt/diagrams/drawingN.xml`) med sina relationer.
pub struct Drawing {
    pub root: Node,
    pub rels: Vec<Rel>,
}

/// Ett inbäddat typsnitt (`p:embeddedFont`).
#[derive(Clone, Debug)]
pub struct EmbeddedFont {
    pub typeface: String,
    /// Del per stil: vanlig, fet, kursiv, fet kursiv.
    pub parts: [Option<String>; 4],
}

/// En öppnad presentation.
pub struct Deck {
    pub pkg: Package,
    /// Bildstorlek i EMU (`p:sldSz`).
    pub size_emu: (i64, i64),
    /// Anteckningssidans storlek i EMU (`p:notesSz`).
    #[allow(dead_code)]
    pub notes_size_emu: (i64, i64),
    /// Numret på första bilden (`firstSlideNum`, standard 1).
    pub first_slide_num: i64,
    pub default_text_style: Option<Node>,
    pub slides: Vec<Slide>,
    pub layouts: Vec<Layout>,
    pub masters: Vec<Master>,
    pub themes: Vec<Theme>,
    pub embedded_fonts: Vec<EmbeddedFont>,
    /// `ppt/tableStyles.xml`.
    #[allow(dead_code)]
    pub table_styles: Option<Node>,
    /// Typsnitten (delas med en ombyggd presentation efter en redigering, se `from_package`).
    pub fonts: Arc<FontBook>,
    /// SmartArt-ritdelar efter delnamn.
    pub drawings: HashMap<String, Drawing>,
    /// Avkodade bilder och SVG; delas mellan ombyggnader (delnamn återanvänds aldrig för annat innehåll).
    images: Arc<Mutex<ImageCache>>,
    svgs: Arc<Mutex<HashMap<String, Option<Arc<resvg::usvg::Tree>>>>>,
}

/// Grovt innehåll på en bild (för jämförelseprovet och statusraden).
#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)] // används av jämförelseprovet
pub struct SlideFeatures {
    pub chart: bool,
    pub emf: bool,
    pub smartart: bool,
    pub table: bool,
}

fn rel_target<'a>(rels: &'a [Rel], kind: &str) -> Option<&'a str> {
    package::rel_of_kind(rels, kind).map(|r| r.target.as_str())
}

impl Deck {
    /// Öppnar en .pptx ur minnet. Fel ger ett meddelande för användaren, aldrig panik.
    pub fn open(bytes: &[u8]) -> Result<Deck, String> {
        Self::from_package(Package::open(bytes)?, None)
    }

    /// Bygger presentationen ur ett paket. `reuse` (presentationen före en redigering) ger typsnitten
    /// (om de inbäddade typsnitten är desamma) och bildcacharna, så att en ombyggnad går fort.
    pub fn from_package(pkg: Package, reuse: Option<&Deck>) -> Result<Deck, String> {
        let root_rels = pkg.rels("");
        let pres_part = package::rel_of_kind(&root_rels, "officeDocument")
            .map(|r| r.target.clone())
            .unwrap_or_else(|| "ppt/presentation.xml".to_owned());
        let pres = pkg
            .xml(&pres_part)
            .filter(|n| n.name == "p:presentation")
            .ok_or_else(|| crate::i18n::t("Filen saknar en PowerPoint-presentation (presentation.xml).").to_owned())?;
        let pres_rels = pkg.rels(&pres_part);

        let size_emu = pres
            .child("p:sldSz")
            .map(|s| (s.attr_i64("cx").unwrap_or(9_144_000), s.attr_i64("cy").unwrap_or(6_858_000)))
            .unwrap_or((9_144_000, 6_858_000));
        let size_emu = (size_emu.0.clamp(12_700, 51_206_400), size_emu.1.clamp(12_700, 51_206_400));
        let notes_size_emu =
            pres.child("p:notesSz").map(|s| (s.attr_i64("cx").unwrap_or(6_858_000), s.attr_i64("cy").unwrap_or(9_144_000))).unwrap_or((6_858_000, 9_144_000));
        // Som bildstorleken: orimliga mått (negativa, enorma) ger overflow i anteckningsmallens uträkningar.
        let notes_size_emu = (notes_size_emu.0.clamp(12_700, 51_206_400), notes_size_emu.1.clamp(12_700, 51_206_400));
        let first_slide_num = pres.attr_i64("firstSlideNum").unwrap_or(1);
        let default_text_style = pres.child("p:defaultTextStyle").cloned();

        let mut deck = Deck {
            size_emu,
            notes_size_emu,
            first_slide_num,
            default_text_style,
            slides: Vec::new(),
            layouts: Vec::new(),
            masters: Vec::new(),
            themes: Vec::new(),
            embedded_fonts: Vec::new(),
            table_styles: rel_target(&pres_rels, "tableStyles").and_then(|p| pkg.xml(p)),
            fonts: Arc::default(),
            drawings: HashMap::new(),
            images: reuse.map(|d| d.images.clone()).unwrap_or_default(),
            svgs: reuse.map(|d| d.svgs.clone()).unwrap_or_default(),
            pkg,
        };

        // Mallar (i presentationens ordning), deras teman och layouter.
        let mut theme_index: HashMap<String, usize> = HashMap::new();
        let mut layout_index: HashMap<String, usize> = HashMap::new();
        let master_ids: Vec<String> = pres
            .child("p:sldMasterIdLst")
            .map(|l| l.children.iter().filter_map(|c| c.attr("r:id").map(str::to_owned)).collect())
            .unwrap_or_default();
        let mut master_parts: Vec<String> =
            master_ids.iter().filter_map(|id| package::rel(&pres_rels, id).map(|r| r.target.clone())).collect();
        if master_parts.is_empty() {
            master_parts = pres_rels.iter().filter(|r| r.kind == "slideMaster").map(|r| r.target.clone()).collect();
        }
        for mp in master_parts {
            deck.load_master(&mp, &mut theme_index, &mut layout_index);
        }

        // Bilder i sldIdLst-ordning.
        let slide_ids: Vec<String> = pres
            .child("p:sldIdLst")
            .map(|l| l.children.iter().filter_map(|c| c.attr("r:id").map(str::to_owned)).collect())
            .unwrap_or_default();
        // Bildernas XML tolkas parallellt (stora presentationer); en bild som en redigering inte rört (samma
        // delar som i `reuse`) tas över ur den gamla presentationen i stället för att tolkas om.
        let parts: Vec<String> = slide_ids.iter().filter_map(|id| package::rel(&pres_rels, id)).map(|r| r.target.clone()).collect();
        let parsed: Vec<Option<(Node, Vec<Rel>)>> = {
            let pkg = &deck.pkg;
            super::par::map(&parts, |part| {
                if let Some(old) = reuse.and_then(|d| d.slides.iter().find(|s| s.part == *part)) {
                    let (o, n) = (reuse.map(|d| &d.pkg)?, pkg);
                    let rels = crate::pptx::edit::rels_name(part);
                    let same = |name: &str| match (o.parts.get(name), n.parts.get(name)) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        (None, None) => true,
                        _ => false,
                    };
                    if same(part) && same(&rels) {
                        return Some((old.root.clone(), old.rels.clone()));
                    }
                }
                let root = pkg.xml(part).filter(|n| n.name == "p:sld")?;
                Some((root, pkg.rels(part)))
            })
        };
        for (part, parsed) in parts.into_iter().zip(parsed) {
            let Some((root, rels)) = parsed else { continue };
            let layout = match rel_target(&rels, "slideLayout") {
                Some(lp) => match layout_index.get(lp) {
                    Some(&i) => Some(i),
                    // Layout som ingen mall listar: läs in den ändå.
                    None => deck.load_orphan_layout(lp, &mut theme_index, &mut layout_index),
                },
                None => None,
            };
            let hidden = root.attr("show").is_some_and(|v| v == "0" || v == "false");
            let notes = rel_target(&rels, "notesSlide").map(str::to_owned);
            deck.slides.push(Slide { part, root, rels, layout, hidden, notes });
        }

        // SmartArt-ritdelar.
        let drawing_parts: Vec<String> = deck
            .slides
            .iter()
            .flat_map(|s| s.rels.iter())
            .chain(deck.layouts.iter().flat_map(|l| l.rels.iter()))
            .chain(deck.masters.iter().flat_map(|m| m.rels.iter()))
            .filter(|r| r.kind == "diagramDrawing")
            .map(|r| r.target.clone())
            .collect();
        for part in drawing_parts {
            if deck.drawings.contains_key(&part) {
                continue;
            }
            if let Some(mut root) = deck.pkg.xml(&part) {
                super::smartart::prepare_drawing(&deck.pkg, &part, &mut root);
                let rels = deck.pkg.rels(&part);
                deck.drawings.insert(part, Drawing { root, rels });
            }
        }

        // Inbäddade typsnitt.
        if let Some(list) = pres.child("p:embeddedFontLst") {
            for ef in list.children_named("p:embeddedFont") {
                let Some(typeface) = ef.child("p:font").and_then(|f| f.attr("typeface")) else { continue };
                let part = |n: &str| ef.child(n).and_then(|c| c.attr("r:id")).and_then(|id| package::rel(&pres_rels, id)).map(|r| r.target.clone());
                deck.embedded_fonts.push(EmbeddedFont {
                    typeface: typeface.to_owned(),
                    parts: [part("p:regular"), part("p:bold"), part("p:italic"), part("p:boldItalic")],
                });
            }
        }
        deck.fonts = match reuse.filter(|d| same_fonts(d, &deck)) {
            Some(old) => old.fonts.clone(),
            None => Arc::new(FontBook::with_embedded(&deck.pkg, &deck.embedded_fonts)),
        };
        // SmartArt utan användbar ritdel läggs ut av layoutmotorn (behöver typsnitten).
        for (k, d) in super::smartart::lay_out_missing(&deck) {
            deck.drawings.insert(k, d);
        }
        Ok(deck)
    }

    fn load_theme(&mut self, part: &str, theme_index: &mut HashMap<String, usize>) -> Option<usize> {
        if let Some(&i) = theme_index.get(part) {
            return Some(i);
        }
        let root = self.pkg.xml(part)?;
        let mut theme = parse_theme(&root);
        theme.rels = self.pkg.rels(part);
        self.themes.push(theme);
        let i = self.themes.len() - 1;
        theme_index.insert(part.to_owned(), i);
        Some(i)
    }

    fn load_master(&mut self, part: &str, theme_index: &mut HashMap<String, usize>, layout_index: &mut HashMap<String, usize>) -> Option<usize> {
        if let Some(i) = self.masters.iter().position(|m| m.part == part) {
            return Some(i);
        }
        let root = self.pkg.xml(part).filter(|n| n.name == "p:sldMaster")?;
        let rels = self.pkg.rels(part);
        let theme = rel_target(&rels, "theme").map(str::to_owned).and_then(|t| self.load_theme(&t, theme_index));
        let layout_parts: Vec<String> = rels.iter().filter(|r| r.kind == "slideLayout").map(|r| r.target.clone()).collect();
        // `otherStyle` för former utan platshållare: textfärgen ignoreras som PowerPoint gör (msrc-win32k bild 5:
        // formens `fontRef`-färg vit, inte `tx1`), och en teckenstorlek som avviker mer än 10 % från
        // `defaultTextStyle` också (covid-infographic: 15,3 mot 18 pt); en liten avvikelse (msrc-win32k: 17,65
        // mot 18 pt) gäller. Det är en tumregel som passar referensbilderna, inte en känd PowerPoint-regel.
        // Övrigt (tabbstopp …) gäller (`ms-workshop-dlcv` blev sämre när hela stilen hoppades över).
        let lvl1_sz = |n: Option<&Node>| n.and_then(|n| n.path(&["a:lvl1pPr", "a:defRPr"])).and_then(|d| d.attr_i64("sz")).map(|v| v as f64);
        let other = root.path(&["p:txStyles", "p:otherStyle"]);
        let strip_sz = match (lvl1_sz(other), lvl1_sz(self.default_text_style.as_ref())) {
            (Some(o), Some(d)) => (o - d).abs() > 0.1 * d,
            _ => false,
        };
        let other_free = other.map(|o| {
            fn strip(n: &mut Node, sz: bool) {
                if n.name == "a:defRPr" {
                    if sz {
                        n.attrs.retain(|(k, _)| k != "sz");
                    }
                    n.children.retain(|c| c.name != "a:solidFill");
                }
                n.children.iter_mut().for_each(|c| strip(c, sz));
            }
            let mut o = o.clone();
            strip(&mut o, strip_sz);
            o
        });
        self.masters.push(Master { part: part.to_owned(), root, rels, theme, other_free });
        let mi = self.masters.len() - 1;
        for lp in layout_parts {
            if layout_index.contains_key(&lp) {
                continue;
            }
            let Some(root) = self.pkg.xml(&lp).filter(|n| n.name == "p:sldLayout") else { continue };
            let rels = self.pkg.rels(&lp);
            self.layouts.push(Layout { part: lp.clone(), root, rels, master: Some(mi) });
            layout_index.insert(lp, self.layouts.len() - 1);
        }
        Some(mi)
    }

    fn load_orphan_layout(&mut self, part: &str, theme_index: &mut HashMap<String, usize>, layout_index: &mut HashMap<String, usize>) -> Option<usize> {
        let root = self.pkg.xml(part).filter(|n| n.name == "p:sldLayout")?;
        let rels = self.pkg.rels(part);
        let master = rel_target(&rels, "slideMaster").map(str::to_owned).and_then(|m| self.load_master(&m, theme_index, layout_index));
        if let Some(&i) = layout_index.get(part) {
            return Some(i);
        }
        self.layouts.push(Layout { part: part.to_owned(), root, rels, master });
        layout_index.insert(part.to_owned(), self.layouts.len() - 1);
        Some(self.layouts.len() - 1)
    }

    /// Bildstorleken i punkter.
    pub fn size_pt(&self) -> (f32, f32) {
        ((self.size_emu.0 as f64 / EMU_PER_PT) as f32, (self.size_emu.1 as f64 / EMU_PER_PT) as f32)
    }

    pub fn layout_of(&self, slide: usize) -> Option<&Layout> {
        self.slides.get(slide)?.layout.and_then(|l| self.layouts.get(l))
    }

    pub fn master_of_layout(&self, layout: &Layout) -> Option<&Master> {
        layout.master.and_then(|m| self.masters.get(m))
    }

    pub fn theme_of_master(&self, master: &Master) -> Option<&Theme> {
        master.theme.and_then(|t| self.themes.get(t))
    }

    /// Färgsammanhanget för en bild: mallens tema och `clrMap`, med layoutens och bildens
    /// `clrMapOvr` (`a:overrideClrMapping`).
    pub fn color_ctx(&self, slide: Option<&Slide>, layout: Option<&Layout>, master: Option<&Master>) -> ColorCtx {
        let mut ctx = ColorCtx::default();
        if let Some(t) = master.and_then(|m| self.theme_of_master(m)) {
            ctx.scheme = t.colors.clone();
        }
        let apply = |ctx: &mut ColorCtx, n: &Node| {
            for (k, v) in &n.attrs {
                ctx.map.insert(k.clone(), v.clone());
            }
        };
        if let Some(cm) = master.and_then(|m| m.root.child("p:clrMap")) {
            apply(&mut ctx, cm);
        }
        for ovr in [layout.map(|l| &l.root), slide.map(|s| &s.root)].into_iter().flatten() {
            if let Some(o) = ovr.path(&["p:clrMapOvr", "a:overrideClrMapping"]) {
                apply(&mut ctx, o);
            }
        }
        ctx
    }

    /// Avkodad bild ur en del (cachad). SVG och EMF/WMF ger `None` här.
    pub fn image(&self, part: &str) -> Option<Arc<ImageData>> {
        if let Some(hit) = self.images.lock().ok()?.get(part) {
            return hit;
        }
        let decoded = self.pkg.read(part).and_then(decode_image).map(Arc::new);
        if let Ok(mut m) = self.images.lock() {
            m.insert(part, decoded.clone());
        }
        decoded
    }

    /// Tolkad SVG ur en del (cachad).
    pub fn svg(&self, part: &str) -> Option<Arc<resvg::usvg::Tree>> {
        if let Some(hit) = self.svgs.lock().ok()?.get(part) {
            return hit.clone();
        }
        let tree = self.pkg.read(part).and_then(|b| {
            let mut opt = resvg::usvg::Options::default();
            opt.fontdb = super::fonts::svg_fontdb();
            resvg::usvg::Tree::from_data(b, &opt).ok()
        });
        let tree = tree.map(Arc::new);
        if let Ok(mut m) = self.svgs.lock() {
            m.insert(part.to_owned(), tree.clone());
        }
        tree
    }

    /// En SmartArt-ritdel.
    pub fn drawing(&self, part: &str) -> Option<&Drawing> {
        self.drawings.get(part)
    }

    /// Bildnumret som visas för bild `i` (0-baserat index), med `firstSlideNum`.
    pub fn slide_number(&self, i: usize) -> i64 {
        self.first_slide_num + i as i64
    }

    /// Vad en bild innehåller (diagram, EMF/WMF, SmartArt, tabeller).
    #[allow(dead_code)] // används av jämförelseprovet
    pub fn features(&self, i: usize) -> SlideFeatures {
        let mut f = SlideFeatures::default();
        let Some(s) = self.slides.get(i) else { return f };
        s.root.walk(&mut |n| match n.name.as_str() {
            "a:graphicData" => match n.attr("uri").unwrap_or("") {
                u if u.ends_with("/chart") || u.contains("chartex") || u.contains("/2014/chartex") => f.chart = true,
                u if u.ends_with("/diagram") => f.smartart = true,
                u if u.ends_with("/table") => f.table = true,
                _ => {}
            },
            "a:blip" => {
                if let Some(r) = n.attr("r:embed").and_then(|id| package::rel(&s.rels, id)) {
                    let low = r.target.to_lowercase();
                    if low.ends_with(".emf") || low.ends_with(".wmf") {
                        f.emf = true;
                    }
                }
            }
            _ => {}
        });
        f
    }
}

/// Samma inbäddade typsnitt (namn, delar och delarnas innehåll) i båda presentationerna?
fn same_fonts(a: &Deck, b: &Deck) -> bool {
    a.embedded_fonts.len() == b.embedded_fonts.len()
        && a.embedded_fonts.iter().zip(&b.embedded_fonts).all(|(x, y)| {
            x.typeface == y.typeface && x.parts == y.parts && x.parts.iter().flatten().all(|p| a.pkg.read(p) == b.pkg.read(p))
        })
}

/// Avkodade bilder, högst `IMAGE_CACHE_BYTES` (de minst nyligen använda släpps; bilder som en
/// visningslista fortfarande håller lever kvar via sin `Arc`).
#[derive(Default)]
pub struct ImageCache {
    map: HashMap<String, (Option<Arc<ImageData>>, u64)>,
    bytes: usize,
    clock: u64,
}

/// Cachens tak i byte (RGBA, förmultiplicerad kopia och JPEG räknas grovt som 2,5 × RGBA).
pub const IMAGE_CACHE_BYTES: usize = 256 << 20;

fn image_cost(i: &Option<Arc<ImageData>>) -> usize {
    i.as_ref().map_or(64, |d| d.rgba.len() * 5 / 2)
}

impl ImageCache {
    fn get(&mut self, part: &str) -> Option<Option<Arc<ImageData>>> {
        self.clock += 1;
        let clock = self.clock;
        self.map.get_mut(part).map(|e| {
            e.1 = clock;
            e.0.clone()
        })
    }

    fn insert(&mut self, part: &str, img: Option<Arc<ImageData>>) {
        self.clock += 1;
        self.bytes += image_cost(&img);
        self.map.insert(part.to_owned(), (img, self.clock));
        while self.bytes > IMAGE_CACHE_BYTES && self.map.len() > 1 {
            let Some(oldest) = self.map.iter().min_by_key(|(_, e)| e.1).map(|(k, _)| k.clone()) else { break };
            if let Some((i, _)) = self.map.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(image_cost(&i));
            }
        }
    }
}

/// Längsta sida för avkodade bilder.
pub const MAX_IMAGE_SIDE: u32 = 2560;

/// Avkodar PNG/JPEG/GIF/BMP/WebP/TIFF-liknande bilder till RGBA.
pub fn decode_image(bytes: &[u8]) -> Option<ImageData> {
    let fmt = image::guess_format(bytes).ok()?;
    let mut img = image::load_from_memory_with_format(bytes, fmt).ok()?;
    // Mycket stora bilder skalas ner (minnet; en bild som fyller bilden behöver inte mer på skärmen).
    // JPEG-originalet behålls orört för PDF:en.
    if img.width().max(img.height()) > MAX_IMAGE_SIDE {
        img = img.resize(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let jpeg = (fmt == image::ImageFormat::Jpeg).then(|| bytes.to_vec());
    Some(ImageData::new(w, h, rgba.into_raw(), jpeg))
}

fn parse_font_scheme(n: Option<&Node>) -> FontScheme {
    let Some(n) = n else { return FontScheme::default() };
    let tf = |name: &str| n.child(name).and_then(|c| c.attr("typeface")).unwrap_or("").to_owned();
    let mut scripts = HashMap::new();
    for f in n.children_named("a:font") {
        if let (Some(s), Some(t)) = (f.attr("script"), f.attr("typeface")) {
            scripts.insert(s.to_owned(), t.to_owned());
        }
    }
    FontScheme { latin: tf("a:latin"), ea: tf("a:ea"), cs: tf("a:cs"), scripts }
}

/// Tolkar `a:theme`.
pub fn parse_theme(root: &Node) -> Theme {
    let mut t = Theme::default();
    let Some(el) = root.child("a:themeElements") else { return t };
    if let Some(cs) = el.child("a:clrScheme") {
        let empty = ColorCtx::default();
        for c in &cs.children {
            if let Some(col) = color::first_color(c).and_then(|n| color::resolve(n, &empty, None)) {
                t.colors.insert(c.local().to_owned(), col);
            }
        }
    }
    if let Some(fs) = el.child("a:fontScheme") {
        t.major = parse_font_scheme(fs.child("a:majorFont"));
        t.minor = parse_font_scheme(fs.child("a:minorFont"));
    }
    if let Some(fm) = el.child("a:fmtScheme") {
        let list = |name: &str| fm.child(name).map(|l| l.children.clone()).unwrap_or_default();
        t.fill_styles = list("a:fillStyleLst");
        t.line_styles = list("a:lnStyleLst");
        t.effect_styles = list("a:effectStyleLst");
        t.bg_fill_styles = list("a:bgFillStyleLst");
    }
    t.object_defaults = root.child("a:objectDefaults").cloned();
    t
}
