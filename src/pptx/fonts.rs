//! Typsnitt för presentationer: inbäddade typsnitt, systemets typsnitt (fontdb), ersättningstabell
//! för Office-typsnitt och reserver per tecken.
//!
//! Uppslagsordning för ett familjenamn: inbäddat i filen → installerat med exakt namn → Kladds
//! inbyggda Carlito/Caladea (Calibri, Calibri Light, Cambria) → ersättare i tabellen nedan →
//! generisk sans/serif/mono → Carlito. En ersättare för ett teckenbreddsfast typsnitt (Consolas,
//! Lucida Console) får en vågrät skalning så att radbrytningen blir som med originalet.
//!
//! Symboltypsnitt (Wingdings, Symbol) görs om till Unicode-tecken (`symbol_to_unicode`) när
//! typsnittet inte finns installerat.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use resvg::usvg::fontdb;

use super::model::EmbeddedFont;
use super::package::Package;
use super::scene::Path;

/// Ett inläst typsnitt med de mått textmotorn behöver (i em).
pub struct FontFace {
    pub id: u64,
    pub family: String,
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    pub upm: f32,
    /// hhea-mått (em). Används inte av textmotorn (som räknar som PowerPoint), men finns för
    /// diagram och tabeller i nästa omgång.
    #[allow(dead_code)]
    pub ascent: f32,
    #[allow(dead_code)]
    pub descent: f32,
    #[allow(dead_code)]
    pub line_gap: f32,
    /// OS/2 winAscent/winDescent (em); PowerPoint räknar radhöjd ur dessa.
    pub win_ascent: f32,
    pub win_descent: f32,
    /// Typsnittets egen vikt/stil.
    #[allow(dead_code)]
    pub bold: bool,
    #[allow(dead_code)]
    pub italic: bool,
    /// Har en symbolteckentabell (3,0): tecknen ligger i U+F000–U+F0FF.
    pub symbol_cmap: bool,
    outlines: Mutex<HashMap<u16, Option<Arc<Path>>>>,
    #[allow(dead_code)]
    pdf_font: OnceLock<Option<krilla::text::Font>>,
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontFace({})", self.family)
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl FontFace {
    pub fn from_data(data: Arc<Vec<u8>>, index: u32) -> Option<FontFace> {
        let face = ttf_parser::Face::parse(&data, index).ok()?;
        let upm = face.units_per_em() as f32;
        if upm <= 0.0 {
            return None;
        }
        let family = face
            .names()
            .into_iter()
            .filter(|n| n.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY || n.name_id == ttf_parser::name_id::FAMILY)
            .find_map(|n| n.to_string())
            .unwrap_or_default();
        let hhea = face.tables().hhea;
        let (wa, wd) = match face.tables().os2 {
            Some(os2) => (os2.windows_ascender() as f32, os2.windows_descender() as f32),
            None => (hhea.ascender as f32, -(hhea.descender as f32)),
        };
        let symbol_cmap = face
            .tables()
            .cmap
            .is_some_and(|c| c.subtables.into_iter().any(|s| s.platform_id == ttf_parser::PlatformId::Windows && s.encoding_id == 0));
        let ff = FontFace {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            family,
            index,
            upm,
            ascent: hhea.ascender as f32 / upm,
            descent: -(hhea.descender as f32) / upm,
            line_gap: hhea.line_gap as f32 / upm,
            win_ascent: if wa > 0.0 { wa / upm } else { hhea.ascender as f32 / upm },
            win_descent: if wd.abs() > 0.0 { wd.abs() / upm } else { -(hhea.descender as f32) / upm },
            bold: face.is_bold() || face.weight().to_number() >= 600,
            italic: face.is_italic() || face.is_oblique(),
            symbol_cmap,
            outlines: Mutex::default(),
            pdf_font: OnceLock::new(),
            data: data.clone(),
        };
        Some(ff)
    }

    pub fn face(&self) -> Option<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.data, self.index).ok()
    }

    pub fn shaper(&self) -> Option<rustybuzz::Face<'_>> {
        rustybuzz::Face::from_slice(&self.data, self.index)
    }

    /// Glyfens kontur i em (y uppåt), cachad.
    pub fn outline(&self, gid: u16) -> Option<Arc<Path>> {
        if let Some(hit) = self.outlines.lock().ok()?.get(&gid) {
            return hit.clone();
        }
        let path = self.face().and_then(|f| {
            let mut b = OutlineBuilder { path: Path::default(), s: 1.0 / self.upm };
            f.outline_glyph(ttf_parser::GlyphId(gid), &mut b)?;
            Some(Arc::new(b.path))
        });
        if let Ok(mut m) = self.outlines.lock() {
            m.insert(gid, path.clone());
        }
        path
    }

    /// Typsnittet för krilla (PDF), skapat en gång.
    #[allow(dead_code)]
    pub fn pdf_font(&self) -> Option<krilla::text::Font> {
        self.pdf_font
            .get_or_init(|| krilla::text::Font::new(krilla::Data::from(self.data.clone()), self.index))
            .clone()
    }
}

struct OutlineBuilder {
    path: Path,
    s: f32,
}

impl ttf_parser::OutlineBuilder for OutlineBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(x * self.s, y * self.s);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(x * self.s, y * self.s);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path.quad_to(x1 * self.s, y1 * self.s, x * self.s, y * self.s);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path.cubic_to(x1 * self.s, y1 * self.s, x2 * self.s, y2 * self.s, x * self.s, y * self.s);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

/// Resultatet av en uppslagning: typsnittet, om fet/kursiv måste efterliknas, och vågrät skalning.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub face: Arc<FontFace>,
    pub fake_bold: bool,
    pub fake_italic: bool,
    /// Vågrät skalning för att efterlikna originalets teckenbredd (1.0 = ingen).
    pub x_scale: f32,
    /// Familjen är ett symboltypsnitt som inte finns: tecknen görs om till Unicode.
    pub symbol_map: Option<SymbolFont>,
}

/// Symboltypsnitt vi kan översätta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolFont {
    Wingdings,
    Wingdings2,
    Wingdings3,
    Symbol,
    Webdings,
}

/// Typsnitten för en presentation (inbäddade + cache).
#[derive(Default)]
pub struct FontBook {
    embedded: HashMap<(String, bool, bool), Arc<FontFace>>,
    cache: Mutex<HashMap<(String, bool, bool), Resolved>>,
    fallback: Mutex<HashMap<(char, u64), Option<Arc<FontFace>>>>,
    /// Inbäddade typsnitt som inte kunde läsas (MicroType Express-komprimerade).
    pub unreadable_embedded: Vec<String>,
}

/// Systemets typsnittsdatabas (laddas första gången den behövs; delas med SVG-tolkningen).
pub fn system_db() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        if std::env::var_os("KLADD_PPTX_NO_SYSTEM_FONTS").is_none() {
            db.load_system_fonts();
        }
        // Kladds inbyggda typsnitt finns alltid.
        for b in [crate::theme::CARLITO, crate::theme::CARLITO_BOLD, crate::theme::CARLITO_ITALIC, crate::theme::CARLITO_BOLD_ITALIC] {
            db.load_font_data(b.to_vec());
        }
        for b in [crate::word::fontmap::CALADEA, crate::word::fontmap::CALADEA_BOLD, crate::word::fontmap::CALADEA_ITALIC, crate::word::fontmap::CALADEA_BOLD_ITALIC] {
            db.load_font_data(b.to_vec());
        }
        Arc::new(db)
    })
    .clone()
}

/// Databasen för SVG-bilder i presentationer.
pub fn svg_fontdb() -> Arc<fontdb::Database> {
    system_db()
}

fn normalize(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// Inlästa typsnittsfiler (delas mellan presentationer).
fn file_cache() -> &'static Mutex<HashMap<(PathBuf, u32), Option<Arc<FontFace>>>> {
    static C: OnceLock<Mutex<HashMap<(PathBuf, u32), Option<Arc<FontFace>>>>> = OnceLock::new();
    C.get_or_init(Mutex::default)
}

fn bundled_cache() -> &'static Mutex<HashMap<(u8, bool, bool), Arc<FontFace>>> {
    static C: OnceLock<Mutex<HashMap<(u8, bool, bool), Arc<FontFace>>>> = OnceLock::new();
    C.get_or_init(Mutex::default)
}

/// Kladds inbyggda Carlito (0) eller Caladea (1).
fn bundled(which: u8, bold: bool, italic: bool) -> Arc<FontFace> {
    let mut c = bundled_cache().lock().unwrap_or_else(|e| e.into_inner());
    c.entry((which, bold, italic))
        .or_insert_with(|| {
            let b = if which == 0 { crate::word::fontmap::Bundled::Carlito } else { crate::word::fontmap::Bundled::Caladea };
            Arc::new(FontFace::from_data(Arc::new(b.data(bold, italic).to_vec()), 0).expect("inbyggt typsnitt"))
        })
        .clone()
}

/// Typsnittet för ett fontdb-id.
fn load_face(db: &fontdb::Database, id: fontdb::ID) -> Option<Arc<FontFace>> {
    let info = db.face(id)?;
    let key = match &info.source {
        fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => Some((p.clone(), info.index)),
        fontdb::Source::Binary(_) => None,
    };
    if let Some(k) = &key {
        if let Some(hit) = file_cache().lock().ok()?.get(k) {
            return hit.clone();
        }
    }
    let face = db.with_face_data(id, |data, index| FontFace::from_data(Arc::new(data.to_vec()), index)).flatten().map(Arc::new);
    if let Some(k) = key {
        if let Ok(mut m) = file_cache().lock() {
            m.insert(k, face.clone());
        }
    }
    face
}

/// Finns familjen installerad (exakt namn)?
fn has_family(db: &fontdb::Database, name: &str) -> bool {
    let n = normalize(name);
    db.faces().any(|f| f.families.iter().any(|(fam, _)| normalize(fam) == n))
}

/// Exakt familj med vikt/stil. Ger också om vikten/stilen matchade.
fn query(db: &fontdb::Database, family: &str, bold: bool, italic: bool) -> Option<(Arc<FontFace>, bool, bool)> {
    if !has_family(db, family) {
        return None;
    }
    // Familjenamnet i databasen kan ha annan skiftläge/blanksteg.
    let n = normalize(family);
    let real = db.faces().find_map(|f| f.families.iter().find(|(fam, _)| normalize(fam) == n).map(|(fam, _)| fam.clone()))?;
    let q = fontdb::Query {
        families: &[fontdb::Family::Name(&real)],
        weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
        stretch: fontdb::Stretch::Normal,
        style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
    };
    let id = db.query(&q)?;
    let info = db.face(id)?;
    let got_bold = info.weight.0 >= 600;
    let got_italic = info.style != fontdb::Style::Normal;
    let face = load_face(db, id)?;
    Some((face, bold && !got_bold, italic && !got_italic))
}

/// Ersättare för vanliga typsnitt som saknas, i prioritetsordning: (familj, vågrät skalning i
/// normal vikt, i fet vikt). Skalningen är originalets medelbredd delad med ersättarens, mätt på
/// bokstäverna i löptext (vägda efter engelsk bokstavsfrekvens) ur breddtabellerna i PowerPoints
/// PDF-exporter (se `docs/dev/pptx.md`). Halvfeta och svarta varianter ritas med fet ersättare.
fn substitutes(name: &str) -> &'static [(&'static str, f32, f32)] {
    const LS: &str = "Liberation Sans";
    const LM: &str = "Liberation Mono";
    match normalize(name).as_str() {
        "segoeui" | "segoeuihistoric" | "segoeuivariable" | "segoeuivariabletext" | "gadugi" | "helveticaneue" | "microsoftsansserif" => {
            &[("Selawik", 1.0, 1.0), (LS, 1.0, 0.988), ("Arial", 1.0, 0.988)]
        }
        "segoeuisemibold" | "segoeuivariabledisplaysemibold" | "segoeuivariablesemibold" => &[("Selawik", 1.0, 1.0), (LS, 0.953, 0.953), ("Arial", 0.953, 0.953)],
        "segoesansdisplaysemibold" | "segoesansdisplay" => &[(LS, 0.897, 0.897), ("Arial", 0.897, 0.897)],
        "segoeuilight" | "segoeuivariabledisplay" => &[("Selawik", 1.0, 1.0), (LS, 0.945, 0.945), ("Arial", 0.945, 0.945)],
        "segoeuisemilight" => &[("Selawik", 1.0, 1.0), (LS, 0.97, 0.97), ("Arial", 0.97, 0.97)],
        "segoeuibold" | "segoeuiblack" => &[("Selawik", 1.0, 1.0), (LS, 0.988, 0.988), ("Arial", 0.988, 0.988)],
        "aptos" | "aptosdisplay" => &[(LS, 0.9635, 0.949), ("Arial", 0.9635, 0.949)],
        "aptossemibold" => &[(LS, 0.922, 0.922), ("Arial", 0.922, 0.922)],
        "aptosnarrow" => &[(LS, 0.85, 0.85), ("Arial", 0.85, 0.85)],
        // Consolas: 0,55 em; Liberation Mono/Courier New: 0,60 em.
        "consolas" => &[(LM, 0.9165, 0.9165), ("Courier New", 0.9165, 0.9165), ("DejaVu Sans Mono", 0.9131, 0.9131)],
        "lucidaconsole" | "lucidasanstypewriter" => &[(LM, 1.005, 1.005), ("DejaVu Sans Mono", 1.0, 1.0)],
        "andalemono" | "menlo" | "monaco" | "sourcecodepro" | "cascadiacode" | "cascadiamono" | "firamono" => &[(LM, 1.0, 1.0), ("DejaVu Sans Mono", 1.0, 1.0)],
        "firacode" | "firacoderetina" => &[(LM, 1.025, 1.025), ("DejaVu Sans Mono", 1.02, 1.02)],
        "sarasatermcl" | "sarasamonocl" => &[(LM, 0.833, 0.833), ("DejaVu Sans Mono", 0.83, 0.83)],
        "verdana" => &[("DejaVu Sans", 1.006, 0.985), (LS, 1.15, 1.18)],
        // Century Gothic: bokstäverna 1,094 × bredare än Arial men siffrorna lika breda; kompromiss.
        "centurygothic" => &[(LS, 1.05, 1.0), ("Arial", 1.05, 1.0)],
        "lucidasansunicode" | "lucidasans" => &[(LS, 1.10, 1.05), ("Arial", 1.10, 1.05)],
        "tahoma" | "trebuchetms" | "opensans" | "sourcesanspro" | "exo2" | "gillsans" | "gillsansmt" | "franklingothicbook" | "franklingothicmedium" => {
            &[(LS, 1.0, 1.0), ("Arial", 1.0, 1.0)]
        }
        "arialblack" => &[(LS, 1.154, 1.154), ("Arial", 1.154, 1.154), ("DejaVu Sans", 1.0, 1.0)],
        "twcenmt" | "twcenmtcondensed" | "corbel" | "candara" => &[("Carlito", 0.98, 0.98)],
        "opensanscondensed" | "opensanscondensedbold" | "opensanscondensedlight" => &[("Carlito", 0.97, 0.952)],
        "garamond" | "bookantiqua" | "palatinolinotype" | "constantia" | "georgia" | "centuryschoolbook" => {
            &[("Liberation Serif", 1.0, 1.0), ("Caladea", 1.0, 1.0)]
        }
        "meiryo" | "meiryoui" | "mspgothic" | "msgothic" | "ｍｓｐゴシック" | "ｍｓゴシック" | "yugothic" | "yugothicui" | "yumincho" | "msmincho"
        | "mspmincho" | "hiraginokakugothicpro" | "gungsuh" | "malgungothic" | "batang" | "gulim" | "dengxian" | "simsun" | "microsoftyahei"
        | "mingliu" | "pmingliu" | "microsoftjhenghei" => &[("IPAPGothic", 1.0, 1.0), ("Noto Sans CJK JP", 1.0, 1.0), ("WenQuanYi Zen Hei", 1.0, 1.0)],
        _ => &[],
    }
}

/// Grov klass för typsnitt vi inte känner.
fn classify(name: &str) -> u8 {
    let n = name.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    if has(&["courier", "consolas", "mono", "menlo", "console", "code", "typewriter"]) {
        2
    } else if has(&["times", "georgia", "garamond", "cambria", "book", "serif", "palatino", "baskerville", "minion", "mincho", "roman"]) && !n.contains("sans") {
        1
    } else {
        0
    }
}

/// Symboltypsnittet för ett familjenamn.
pub fn symbol_font(name: &str) -> Option<SymbolFont> {
    Some(match normalize(name).as_str() {
        "wingdings" => SymbolFont::Wingdings,
        "wingdings2" => SymbolFont::Wingdings2,
        "wingdings3" => SymbolFont::Wingdings3,
        "symbol" => SymbolFont::Symbol,
        "webdings" => SymbolFont::Webdings,
        _ => return None,
    })
}

impl FontBook {
    /// Läser presentationens inbäddade typsnitt.
    pub fn with_embedded(pkg: &Package, list: &[EmbeddedFont]) -> FontBook {
        let mut book = FontBook::default();
        for ef in list {
            for (i, part) in ef.parts.iter().enumerate() {
                let Some(part) = part else { continue };
                let Some(bytes) = pkg.read(part) else { continue };
                match decode_embedded(bytes, part) {
                    Some(data) => {
                        if let Some(face) = FontFace::from_data(Arc::new(data), 0) {
                            let (bold, italic) = (i == 1 || i == 3, i == 2 || i == 3);
                            book.embedded.insert((normalize(&ef.typeface), bold, italic), Arc::new(face));
                        }
                    }
                    None => {
                        if !book.unreadable_embedded.contains(&ef.typeface) {
                            book.unreadable_embedded.push(ef.typeface.clone());
                        }
                    }
                }
            }
        }
        book
    }

    /// Slår upp ett familjenamn.
    pub fn resolve(&self, family: &str, bold: bool, italic: bool) -> Resolved {
        let key = (normalize(family), bold, italic);
        if let Some(hit) = self.cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
            return hit;
        }
        let r = self.resolve_uncached(family, bold, italic);
        if let Ok(mut c) = self.cache.lock() {
            c.insert(key, r.clone());
        }
        r
    }

    fn resolve_uncached(&self, family: &str, bold: bool, italic: bool) -> Resolved {
        let mk = |face: Arc<FontFace>, fb: bool, fi: bool, x_scale: f32| Resolved { face, fake_bold: fb, fake_italic: fi, x_scale, symbol_map: None };
        let n = normalize(family);
        // 1. Inbäddat.
        for (b, i) in [(bold, italic), (bold, false), (false, italic), (false, false)] {
            if let Some(f) = self.embedded.get(&(n.clone(), b, i)) {
                return mk(f.clone(), bold && !b, italic && !i, 1.0);
            }
        }
        let db = system_db();
        // 2. Installerat med exakt namn (inte Carlito/Caladea som ersättare för sig själva här).
        if !family.trim().is_empty() {
            if let Some((f, fb, fi)) = query(&db, family, bold, italic) {
                return mk(f, fb, fi, 1.0);
            }
        }
        // 3. Symboltypsnitt som saknas: översätt tecknen och använd ett vanligt typsnitt.
        if let Some(sym) = symbol_font(family) {
            let base = self.resolve("DejaVu Sans", false, false);
            return Resolved { symbol_map: Some(sym), ..base };
        }
        // 4. Inbyggda metriskt lika (Calibri, Calibri Light, Cambria).
        match crate::word::fontmap::bundled(family) {
            Some(crate::word::fontmap::Bundled::Carlito) => return mk(bundled(0, bold, italic), false, false, 1.0),
            Some(crate::word::fontmap::Bundled::Caladea) => return mk(bundled(1, bold, italic), false, false, 1.0),
            None => {}
        }
        // 5. Ersättare.
        let mut subs: Vec<(&str, f32, f32)> = substitutes(family).to_vec();
        if let Some(s) = crate::word::fontmap::substitute(family) {
            subs.push((s, 1.0, 1.0));
        }
        // Halvfeta och svarta varianter ritas feta.
        let heavy = n.contains("semibold") || n.contains("black") || n.contains("heavy") || n.contains("demibold") || n.contains("condensedbold");
        for (s, xr, xb) in subs {
            let xs = if bold || heavy { xb } else { xr };
            if s == "Carlito" {
                return mk(bundled(0, bold || heavy, italic), false, false, xs);
            }
            if let Some((f, fb, fi)) = query(&db, s, bold || heavy, italic) {
                return mk(f, fb, fi, xs);
            }
        }
        // 6. Generiskt.
        let generic: &[&str] = match classify(family) {
            2 => &["Liberation Mono", "Courier New", "DejaVu Sans Mono", "Menlo"],
            1 => &["Liberation Serif", "Times New Roman", "Times", "DejaVu Serif"],
            _ => &["Liberation Sans", "Arial", "Helvetica"],
        };
        for g in generic {
            if let Some((f, fb, fi)) = query(&db, g, bold || heavy, italic) {
                return mk(f, fb, fi, 1.0);
            }
        }
        if classify(family) == 1 {
            return mk(bundled(1, bold, italic), false, false, 1.0);
        }
        mk(bundled(0, bold || heavy, italic), false, false, 1.0)
    }

    /// Ett typsnitt som har tecknet `c`, när `primary` saknar det. Ordningen gynnar
    /// östasiatiska typsnitt för CJK och DejaVu för symboler.
    pub fn fallback(&self, c: char, primary: &FontFace) -> Option<Arc<FontFace>> {
        let key = (c, primary.id);
        if let Some(hit) = self.fallback.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return hit;
        }
        let db = system_db();
        let cjk = is_cjk(c);
        let order: &[&str] = if cjk {
            &["IPAPGothic", "Noto Sans CJK JP", "WenQuanYi Zen Hei", "Yu Gothic", "Meiryo", "MS Gothic", "Microsoft YaHei", "PingFang SC", "Hiragino Sans"]
        } else {
            &["DejaVu Sans", "Liberation Sans", "Arial", "Segoe UI Symbol", "Segoe UI", "OpenSymbol", "Noto Sans Symbols", "Noto Sans Symbols2", "Apple Symbols", "FreeSans", "IPAPGothic", "WenQuanYi Zen Hei", "Unifont"]
        };
        let mut found = None;
        for fam in order {
            if let Some((f, _, _)) = query(&db, fam, false, false) {
                if f.id != primary.id && f.face().is_some_and(|ff| ff.glyph_index(c).is_some()) {
                    found = Some(f);
                    break;
                }
            }
        }
        if found.is_none() {
            // Sista utvägen: leta igenom alla typsnitt (en gång per tecken). Täckningen provas utan att
            // typsnittet kopieras eller sparas; bara träffen läses in.
            let hit = db.faces().map(|info| info.id).find(|&id| {
                db.with_face_data(id, |data, index| ttf_parser::Face::parse(data, index).ok().is_some_and(|f| f.glyph_index(c).is_some())).unwrap_or(false)
            });
            found = hit.and_then(|id| load_face(&db, id));
        }
        if let Ok(mut m) = self.fallback.lock() {
            m.insert(key, found.clone());
        }
        found
    }
}

/// Östasiatiskt tecken (för `a:ea`-typsnittet).
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x2FDF | 0x2FF0..=0x303F | 0x3040..=0x30FF | 0x3100..=0x312F | 0x3130..=0x318F
        | 0x3190..=0x31FF | 0x3200..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)
}

/// Tecken i komplexa skrifter (`a:cs`-typsnittet): hebreiska, arabiska, thai, indiska skrifter.
pub fn is_complex(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Gör om ett tecken i ett symboltypsnitt till Unicode. Tecknen kan stå som U+0020–U+00FF eller i
/// privata området U+F020–U+F0FF.
pub fn symbol_to_unicode(font: SymbolFont, c: char) -> char {
    let code = c as u32;
    let code = if (0xF000..=0xF0FF).contains(&code) { code - 0xF000 } else { code };
    if code > 0xFF {
        return c;
    }
    let mapped = match font {
        SymbolFont::Wingdings => match code {
            0x21 => 0x270F,
            0x22 => 0x2702,
            0x23 => 0x2701,
            0x28 => 0x260E,
            0x2A => 0x2709,
            0x36 => 0x231B,
            0x3E => 0x2707,
            0x3F => 0x270D,
            0x41 => 0x270C,
            0x43 => 0x1F44D,
            0x44 => 0x1F44E,
            0x45 => 0x261C,
            0x46 => 0x261E,
            0x47 => 0x261D,
            0x48 => 0x261F,
            0x4A => 0x263A,
            0x4B => 0x1F610,
            0x4C => 0x2639,
            0x4E => 0x2620,
            0x51 => 0x2708,
            0x52 => 0x263C,
            0x54 => 0x2744,
            0x56 => 0x271E,
            0x58 => 0x2720,
            0x59 => 0x2721,
            0x5A => 0x262A,
            0x5B => 0x262F,
            0x5C => 0x0950,
            0x5D => 0x2638,
            0x5E..=0x69 => 0x2648 + (code - 0x5E),
            0x6A => 0x0026,
            0x6B => 0x0026,
            0x6C => 0x25CF,
            0x6D => 0x274D,
            0x6E => 0x25A0,
            0x6F => 0x25A1,
            0x70 => 0x25FB,
            0x71 => 0x2751,
            0x72 => 0x2752,
            0x73 => 0x2B27,
            0x74 => 0x29EB,
            0x75 => 0x25C6,
            0x76 => 0x2756,
            0x77 => 0x2B25,
            0x78 => 0x2327,
            0x79 => 0x2353,
            0x7A => 0x2318,
            0x7B => 0x2740,
            0x7C => 0x273F,
            0x7D => 0x275D,
            0x7E => 0x275E,
            0x80 => 0x24EA,
            0x81..=0x8A => 0x2460 + (code - 0x81),
            0x8B => 0x24FF,
            0x8C..=0x95 => 0x2776 + (code - 0x8C),
            0x9E => 0x00B7,
            0x9F => 0x2022,
            0xA0 => 0x25AA,
            0xA1 => 0x25CB,
            0xA2 => 0x2B55,
            0xA3 => 0x2B58,
            0xA4 => 0x25C9,
            0xA5 => 0x25CE,
            0xA7 => 0x25AA,
            0xA8 => 0x25FB,
            0xAA => 0x2726,
            0xAB => 0x2605,
            0xAC => 0x2736,
            0xAD => 0x2734,
            0xAE => 0x2739,
            0xAF => 0x2735,
            0xB1 => 0x2316,
            0xB2 => 0x27E1,
            0xB3 => 0x2311,
            0xB5 => 0x272A,
            0xB6 => 0x2730,
            0xD5 => 0x232B,
            0xD6 => 0x2326,
            0xD8 => 0x27A2,
            0xDF => 0x2190,
            0xE0 => 0x2192,
            0xE1 => 0x2191,
            0xE2 => 0x2193,
            0xE3 => 0x2196,
            0xE4 => 0x2197,
            0xE5 => 0x2199,
            0xE6 => 0x2198,
            0xE7 => 0x2B05,
            0xE8 => 0x2794,
            0xE9 => 0x2B06,
            0xEA => 0x2B07,
            0xEF => 0x21E6,
            0xF0 => 0x21E8,
            0xF1 => 0x21E7,
            0xF2 => 0x21E9,
            0xF3 => 0x2B04,
            0xF4 => 0x21F3,
            0xFB => 0x2718,
            0xFC => 0x2714,
            0xFD => 0x2612,
            0xFE => 0x2611,
            _ => 0x2022,
        },
        SymbolFont::Wingdings2 => match code {
            0x96..=0x9F => 0x25CF,
            0xA2 | 0xA3 => 0x25A0,
            0x50 => 0x2713,
            0x4F => 0x2717,
            0x54 => 0x2612,
            0x52 => 0x2611,
            0xF8 => 0x25CF,
            _ => 0x25A0,
        },
        SymbolFont::Wingdings3 => match code {
            0x5B | 0x7D => 0x25B6,
            0x5C | 0x7C => 0x25C0,
            0x5D => 0x25B2,
            0x5E => 0x25BC,
            0x70 | 0x75 => 0x25B6,
            0x71 | 0x74 => 0x25C0,
            0x72 => 0x25B2,
            0x73 => 0x25BC,
            0xC6 | 0xE0 => 0x2192,
            0xC5 | 0xDF => 0x2190,
            _ => 0x25BA,
        },
        SymbolFont::Webdings => 0x25CF,
        SymbolFont::Symbol => match code {
            0x41..=0x5A => {
                const UP: [u32; 26] = [
                    0x391, 0x392, 0x3A7, 0x394, 0x395, 0x3A6, 0x393, 0x397, 0x399, 0x3D1, 0x39A, 0x39B, 0x39C, 0x39D, 0x39F, 0x3A0, 0x398, 0x3A1,
                    0x3A3, 0x3A4, 0x3A5, 0x3C2, 0x3A9, 0x39E, 0x3A8, 0x396,
                ];
                UP[(code - 0x41) as usize]
            }
            0x61..=0x7A => {
                const LO: [u32; 26] = [
                    0x3B1, 0x3B2, 0x3C7, 0x3B4, 0x3B5, 0x3C6, 0x3B3, 0x3B7, 0x3B9, 0x3D5, 0x3BA, 0x3BB, 0x3BC, 0x3BD, 0x3BF, 0x3C0, 0x3B8, 0x3C1,
                    0x3C3, 0x3C4, 0x3C5, 0x3D6, 0x3C9, 0x3BE, 0x3C8, 0x3B6,
                ];
                LO[(code - 0x61) as usize]
            }
            0x22 => 0x2200,
            0x24 => 0x2203,
            0x27 => 0x220B,
            0x2A => 0x2217,
            0x2D => 0x2212,
            0x40 => 0x2245,
            0x5C => 0x2234,
            0x5E => 0x22A5,
            0xA3 => 0x2264,
            0xA5 => 0x221E,
            0xA7 => 0x2663,
            0xA8 => 0x2666,
            0xA9 => 0x2665,
            0xAA => 0x2660,
            0xAB => 0x2194,
            0xAC => 0x2190,
            0xAD => 0x2191,
            0xAE => 0x2192,
            0xAF => 0x2193,
            0xB0 => 0x00B0,
            0xB1 => 0x00B1,
            0xB3 => 0x2265,
            0xB4 => 0x00D7,
            0xB5 => 0x221D,
            0xB6 => 0x2202,
            0xB7 => 0x2022,
            0xB8 => 0x00F7,
            0xB9 => 0x2260,
            0xBA => 0x2261,
            0xBB => 0x2248,
            0xBC => 0x2026,
            0xC5 => 0x2295,
            0xC6 => 0x2205,
            0xC7 => 0x2229,
            0xC8 => 0x222A,
            0xCE => 0x2208,
            0xCF => 0x2209,
            0xD0 => 0x2220,
            0xD1 => 0x2207,
            0xD5 => 0x220F,
            0xD6 => 0x221A,
            0xD7 => 0x22C5,
            0xD8 => 0x00AC,
            0xD9 => 0x2227,
            0xDA => 0x2228,
            0xDB => 0x21D4,
            0xDC => 0x21D0,
            0xDD => 0x21D1,
            0xDE => 0x21D2,
            0xDF => 0x21D3,
            0xE5 => 0x2211,
            0xF2 => 0x222B,
            _ => code,
        },
    };
    char::from_u32(mapped).unwrap_or(c)
}

/// Avkodar ett inbäddat typsnitt: rå TrueType/OpenType, okomprimerad EOT (ev. XOR-krypterad)
/// eller Word-stilens GUID-obfuskering. MicroType Express-komprimerad EOT stöds inte (`None`).
pub fn decode_embedded(bytes: &[u8], part: &str) -> Option<Vec<u8>> {
    let is_sfnt = |b: &[u8]| b.len() > 12 && (b.starts_with(&[0, 1, 0, 0]) || b.starts_with(b"OTTO") || b.starts_with(b"true") || b.starts_with(b"ttcf"));
    if is_sfnt(bytes) {
        return Some(bytes.to_vec());
    }
    // EOT: "LP" vid byte 34.
    if bytes.len() > 82 && &bytes[34..36] == b"LP" {
        let u32le = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) as usize;
        let eot_size = u32le(0).min(bytes.len());
        let data_size = u32le(4);
        let flags = u32le(12);
        if data_size == 0 || data_size > eot_size {
            return None;
        }
        // TTEMBED_TTCOMPRESSED: MicroType Express.
        if flags & 0x4 != 0 {
            return None;
        }
        let mut data = bytes[eot_size - data_size..eot_size].to_vec();
        if flags & 0x1000_0000 != 0 {
            for b in &mut data {
                *b ^= 0x50;
            }
        }
        return is_sfnt(&data).then_some(data);
    }
    // GUID-obfuskering (som .odttf i Word): de första 32 byten XOR:as med GUID:en i delnamnet.
    let name = part.rsplit('/').next().unwrap_or(part);
    let hex: String = name.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() >= 32 {
        let g: Vec<u8> = (0..16).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
        if g.len() == 16 {
            let key: Vec<u8> = g.iter().rev().copied().collect();
            let mut data = bytes.to_vec();
            for i in 0..32.min(data.len()) {
                data[i] ^= key[i % 16];
            }
            if is_sfnt(&data) {
                return Some(data);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibri_blir_carlito_och_okant_far_reserv() {
        let book = FontBook::default();
        let r = book.resolve("Calibri", false, false);
        assert!(r.face.family.contains("Carlito"), "{}", r.face.family);
        let r = book.resolve("Calibri Light", true, false);
        assert!(r.face.bold);
        let r = book.resolve("Cambria", false, true);
        assert!(r.face.family.contains("Caladea") || r.face.family.contains("Cambria"));
        // Okänt namn ger alltid något typsnitt.
        let r = book.resolve("Finns Inte Alls 123", false, false);
        assert!(r.face.upm > 0.0);
    }

    #[test]
    fn symboltypsnitt() {
        assert_eq!(symbol_to_unicode(SymbolFont::Wingdings, 'Ø'), '\u{27A2}');
        assert_eq!(symbol_to_unicode(SymbolFont::Wingdings, '\u{F0A7}'), '\u{25AA}');
        assert_eq!(symbol_to_unicode(SymbolFont::Wingdings, '\u{F0FC}'), '\u{2714}');
        assert_eq!(symbol_to_unicode(SymbolFont::Symbol, '\u{F0B7}'), '•');
        assert_eq!(symbol_to_unicode(SymbolFont::Symbol, 'a'), 'α');
        assert_eq!(symbol_to_unicode(SymbolFont::Wingdings, '\u{F081}'), '①');
        assert_eq!(symbol_to_unicode(SymbolFont::Wingdings, '\u{F08C}'), '❶');
    }

    #[test]
    fn inbaddade_format() {
        let ttf = crate::theme::CARLITO;
        assert!(decode_embedded(ttf, "ppt/fonts/font1.fntdata").is_some());
        // Okomprimerad, XOR-krypterad EOT.
        let mut eot = vec![0u8; 82];
        let total = 82 + ttf.len();
        eot[0..4].copy_from_slice(&(total as u32).to_le_bytes());
        eot[4..8].copy_from_slice(&(ttf.len() as u32).to_le_bytes());
        eot[12..16].copy_from_slice(&0x1000_0000u32.to_le_bytes());
        eot[34] = b'L';
        eot[35] = b'P';
        eot.extend(ttf.iter().map(|b| b ^ 0x50));
        assert_eq!(decode_embedded(&eot, "x.fntdata").as_deref(), Some(ttf));
        // MTX-komprimerad: stöds inte.
        eot[12..16].copy_from_slice(&0x4u32.to_le_bytes());
        assert!(decode_embedded(&eot, "x.fntdata").is_none());
    }
}
