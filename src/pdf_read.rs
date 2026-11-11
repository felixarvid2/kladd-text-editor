//! Läser text (och bilder) ur PDF-filer och gör om dem till Markdown.
//!
//! Metoderna följer PdfCraft (github.com/storytold/pdfcraft, MIT/Apache-2.0):
//! - tecken avkodas via `/ToUnicode` före `/Encoding` (här med `lopdf`),
//! - glyfernas positioner räknas fram ur text- och grafikmatriserna med
//!   `Tc`, `Tw`, `Tz`, `TL`, `Ts` och `TJ`-förskjutningar,
//! - mellanslag läggs in när avståndet är större än 0,2 em efter avdrag för
//!   "spärrning", så att spärrad text inte blir "H e j",
//! - rader byggs av körningar på samma baslinje, stycken av rader med normalt radavstånd,
//! - läsordningen följer spalter: läs nedåt i samma spalt så länge nästa block ligger
//!   inom 3,5 radhöjder, hoppa sedan till nästa spalt,
//! - rubriker känns igen på storlek jämfört med brödtextens vanligaste storlek
//!   (1,6× → H1, 1,25× → H2, fet kort rad → H3) och listor på sina markörer.
//!
//! Gränser skyddar mot skadliga filer: hur mycket som packas upp, hur djupt
//! formulär-XObjekt får nästlas och hur mycket text en sida får ge.

use std::collections::{HashMap, HashSet};

use lopdf::content::Content;
use lopdf::Encoding;
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::docx::{Image as DocImage, IMAGE_SCHEME, escape_inline, escape_line_start, wrap};

const MAX_FILE_BYTES: usize = 512 * 1024 * 1024;
const MAX_DECODED: usize = 256 * 1024 * 1024;
const MAX_FORM_DEPTH: usize = 12;
const MAX_FORM_VISITS: usize = 5_000;
const MAX_PAGE_TEXT: usize = 16 * 1024 * 1024;
const MAX_IMAGES: usize = 200;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

pub enum ReadError {
    /// Dokumentet är krypterat och behöver ett lösenord.
    NeedsPassword,
    /// Lösenordet stämde inte.
    WrongPassword,
    Other(String),
}

pub struct Loaded {
    pub markdown: String,
    pub images: Vec<(String, DocImage)>,
    /// Saker i originalet som inte kommer med, i läsbar form.
    pub dropped: Vec<&'static str>,
    /// Sidor utan läsbar text (t.ex. skannade sidor).
    pub pages_without_text: usize,
    pub page_count: usize,
}

// ============================================================== geometri

#[derive(Clone, Copy, Debug)]
struct M([f32; 6]);

impl M {
    const ID: M = M([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    /// self × other (PDF-ordning: punkt × self × other)
    fn mul(self, o: M) -> M {
        let a = self.0;
        let b = o.0;
        M([
            a[0] * b[0] + a[1] * b[2],
            a[0] * b[1] + a[1] * b[3],
            a[2] * b[0] + a[3] * b[2],
            a[2] * b[1] + a[3] * b[3],
            a[4] * b[0] + a[5] * b[2] + b[4],
            a[4] * b[1] + a[5] * b[3] + b[5],
        ])
    }
    fn translate(x: f32, y: f32) -> M {
        M([1.0, 0.0, 0.0, 1.0, x, y])
    }
    fn apply(self, x: f32, y: f32) -> (f32, f32) {
        let a = self.0;
        (a[0] * x + a[2] * y + a[4], a[1] * x + a[3] * y + a[5])
    }
    /// Skalfaktor i y-led (för textstorlek på sidan).
    fn scale_y(self) -> f32 {
        (self.0[2] * self.0[2] + self.0[3] * self.0[3]).sqrt()
    }
    /// Lutning i grader för textens riktning.
    fn angle(self) -> f32 {
        self.0[1].atan2(self.0[0]).to_degrees()
    }
}

fn num(o: &Object) -> f32 {
    match o {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        _ => 0.0,
    }
}

fn matrix(ops: &[Object]) -> Option<M> {
    if ops.len() < 6 {
        return None;
    }
    Some(M([num(&ops[0]), num(&ops[1]), num(&ops[2]), num(&ops[3]), num(&ops[4]), num(&ops[5])]))
}

// ============================================================== typsnitt

struct FontInfo<'a> {
    encoding: Option<Encoding<'a>>,
    /// Typ0/CID: koder är två byte.
    two_byte: bool,
    widths: HashMap<u32, f32>,
    default_width: f32,
    bold: bool,
    italic: bool,
    mono: bool,
    /// Kan inte avkodas (CID-typsnitt utan /ToUnicode).
    undecodable: bool,
}

impl FontInfo<'_> {
    fn width(&self, code: u32) -> f32 {
        self.widths.get(&code).copied().unwrap_or(self.default_width) / 1000.0
    }

    fn codes(&self, bytes: &[u8]) -> Vec<u32> {
        if self.two_byte {
            bytes.chunks(2).map(|c| if c.len() == 2 { u32::from(c[0]) << 8 | u32::from(c[1]) } else { u32::from(c[0]) }).collect()
        } else {
            bytes.iter().map(|&b| u32::from(b)).collect()
        }
    }

    fn decode(&self, bytes: &[u8]) -> String {
        if self.undecodable {
            return String::new();
        }
        match &self.encoding {
            Some(enc) => enc.bytes_to_string(bytes).unwrap_or_default(),
            None => bytes.iter().map(|&b| b as char).collect(),
        }
    }
}

fn resolve<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match resolve(doc, o) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn name_str(o: &Object) -> String {
    match o {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => String::new(),
    }
}

/// Förbereder typsnittets ordlista så att `/ToUnicode` används före `/Encoding`
/// (lopdf tar annars `/Encoding` först, PdfCraft gör tvärtom).
fn prepared_font_dict(doc: &Document, font: &Dictionary) -> Dictionary {
    let mut d = font.clone();
    if d.has(b"ToUnicode") && d.has(b"Encoding") {
        let enc_name = d.get(b"Encoding").map(|o| name_str(resolve(doc, o))).unwrap_or_default();
        // Identity-H går via ToUnicode i lopdf ändå; övriga byts mot ToUnicode.
        if enc_name != "Identity-H" && enc_name != "Identity-V" {
            d.remove(b"Encoding");
        }
    }
    d
}

fn font_info<'a>(doc: &'a Document, font: &'a Dictionary) -> FontInfo<'a> {
    let subtype = font.get(b"Subtype").map(name_str).unwrap_or_default();
    let base = font.get(b"BaseFont").map(|o| name_str(resolve(doc, o))).unwrap_or_default();
    let lower = base.to_lowercase();
    let bold = ["bold", "black", "heavy", "semibold", "demi"].iter().any(|k| lower.contains(k));
    let italic = lower.contains("italic") || lower.contains("oblique");
    let mono = ["mono", "courier", "consol", "code"].iter().any(|k| lower.contains(k));
    let two_byte = subtype == "Type0";
    let has_to_unicode = font.has(b"ToUnicode");

    let mut widths = HashMap::new();
    let mut default_width = if mono { 600.0 } else { 500.0 };
    if two_byte {
        // Bredder finns i det underliggande CID-typsnittet: /W och /DW.
        let desc = font
            .get(b"DescendantFonts")
            .ok()
            .map(|o| resolve(doc, o))
            .and_then(|o| if let Object::Array(a) = o { a.first() } else { None })
            .and_then(|o| dict_of(doc, o));
        if let Some(desc) = desc {
            default_width = desc.get(b"DW").map(|o| num(resolve(doc, o))).unwrap_or(1000.0);
            if let Ok(Object::Array(w)) = desc.get(b"W").map(|o| resolve(doc, o)) {
                let mut i = 0;
                while i < w.len() {
                    let first = num(resolve(doc, &w[i])) as u32;
                    match w.get(i + 1).map(|o| resolve(doc, o)) {
                        Some(Object::Array(list)) => {
                            for (k, v) in list.iter().enumerate() {
                                widths.insert(first + k as u32, num(resolve(doc, v)));
                            }
                            i += 2;
                        }
                        Some(last) => {
                            let last = num(last) as u32;
                            let v = w.get(i + 2).map(|o| num(resolve(doc, o))).unwrap_or(default_width);
                            for c in first..=last.min(first + 65_535) {
                                widths.insert(c, v);
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
            }
        }
    } else {
        let first = font.get(b"FirstChar").map(|o| num(resolve(doc, o))).unwrap_or(0.0) as u32;
        if let Ok(Object::Array(w)) = font.get(b"Widths").map(|o| resolve(doc, o)) {
            for (k, v) in w.iter().enumerate() {
                widths.insert(first + k as u32, num(resolve(doc, v)));
            }
        }
        if let Some(fd) = font.get(b"FontDescriptor").ok().and_then(|o| dict_of(doc, o)) {
            if let Ok(mw) = fd.get(b"MissingWidth") {
                default_width = num(resolve(doc, mw));
            }
        }
    }
    let encoding = font.get_font_encoding_with_limit(doc, MAX_DECODED).ok();
    // CID-typsnitt utan ToUnicode: koderna är glyf-id och går inte att översätta säkert.
    let undecodable = two_byte && !has_to_unicode;
    FontInfo { encoding, two_byte, widths, default_width, bold, italic, mono, undecodable }
}

// ============================================================== textinsamling

#[derive(Clone, Debug)]
struct Run {
    x: f32,
    y: f32,
    x_end: f32,
    size: f32,
    text: String,
    bold: bool,
    italic: bool,
    mono: bool,
    invisible: bool,
}

#[derive(Clone, Debug)]
struct ImageHit {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    name: String,
}

#[derive(Clone)]
struct TextState {
    tc: f32,
    tw: f32,
    tz: f32,
    tl: f32,
    ts: f32,
    font: Option<Vec<u8>>,
    size: f32,
    render: i64,
}

impl Default for TextState {
    fn default() -> Self {
        Self { tc: 0.0, tw: 0.0, tz: 1.0, tl: 0.0, ts: 0.0, font: None, size: 0.0, render: 0 }
    }
}

struct Collector<'a> {
    doc: &'a Document,
    runs: Vec<Run>,
    images: Vec<ImageHit>,
    found_images: &'a mut Vec<(String, DocImage)>,
    image_by_id: &'a mut HashMap<ObjectId, String>,
    text_bytes: usize,
    visits: usize,
    undecodable: bool,
    has_annotations_or_forms: bool,
}

impl<'a> Collector<'a> {
    fn run_content(&mut self, content: &[u8], resources: Option<&'a Dictionary>, base: M, depth: usize) {
        let Ok(content) = Content::decode(content) else { return };
        // Typsnitt: ägda kopior först, sedan avkodning som lånar från dem.
        let font_dicts: HashMap<Vec<u8>, Dictionary> = resources
            .and_then(|r| r.get(b"Font").ok())
            .and_then(|f| dict_of(self.doc, f))
            .map(|fonts| {
                fonts
                    .iter()
                    .filter_map(|(k, v)| dict_of(self.doc, v).map(|d| (k.clone(), prepared_font_dict(self.doc, d))))
                    .collect()
            })
            .unwrap_or_default();
        let fonts: HashMap<Vec<u8>, FontInfo<'_>> =
            font_dicts.iter().map(|(k, d)| (k.clone(), font_info(self.doc, d))).collect();
        let xobjects = resources.and_then(|r| r.get(b"XObject").ok()).and_then(|x| dict_of(self.doc, x));

        let mut ctm = base;
        let mut stack: Vec<(M, TextState)> = Vec::new();
        let mut ts = TextState::default();
        let mut tm = M::ID;
        let mut tlm = M::ID;

        for op in &content.operations {
            let o = &op.operands;
            match op.operator.as_str() {
                "q" => {
                    if stack.len() < 64 {
                        stack.push((ctm, ts.clone()));
                    }
                }
                "Q" => {
                    if let Some((c, t)) = stack.pop() {
                        ctm = c;
                        ts = t;
                    }
                }
                "cm" => {
                    if let Some(m) = matrix(o) {
                        ctm = m.mul(ctm);
                    }
                }
                "BT" => {
                    tm = M::ID;
                    tlm = M::ID;
                }
                "Tc" => ts.tc = o.first().map(num).unwrap_or(0.0),
                "Tw" => ts.tw = o.first().map(num).unwrap_or(0.0),
                "Tz" => ts.tz = o.first().map(num).unwrap_or(100.0) / 100.0,
                "TL" => ts.tl = o.first().map(num).unwrap_or(0.0),
                "Ts" => ts.ts = o.first().map(num).unwrap_or(0.0),
                "Tr" => {
                    ts.render = match o.first() {
                        Some(Object::Integer(i)) => *i,
                        _ => 0,
                    }
                }
                "Tf" => {
                    if let (Some(Object::Name(n)), Some(s)) = (o.first(), o.get(1)) {
                        ts.font = Some(n.clone());
                        ts.size = num(s);
                    }
                }
                "Td" => {
                    if o.len() >= 2 {
                        tlm = M::translate(num(&o[0]), num(&o[1])).mul(tlm);
                        tm = tlm;
                    }
                }
                "TD" => {
                    if o.len() >= 2 {
                        ts.tl = -num(&o[1]);
                        tlm = M::translate(num(&o[0]), num(&o[1])).mul(tlm);
                        tm = tlm;
                    }
                }
                "Tm" => {
                    if let Some(m) = matrix(o) {
                        tm = m;
                        tlm = m;
                    }
                }
                "T*" => {
                    tlm = M::translate(0.0, -ts.tl).mul(tlm);
                    tm = tlm;
                }
                "Tj" | "'" | "\"" | "TJ" => {
                    if op.operator == "'" || op.operator == "\"" {
                        if op.operator == "\"" && o.len() >= 3 {
                            ts.tw = num(&o[0]);
                            ts.tc = num(&o[1]);
                        }
                        tlm = M::translate(0.0, -ts.tl).mul(tlm);
                        tm = tlm;
                    }
                    let Some(font) = ts.font.as_ref().and_then(|f| fonts.get(f)) else { continue };
                    let items: Vec<&Object> = match op.operator.as_str() {
                        "TJ" => match o.first() {
                            Some(Object::Array(a)) => a.iter().collect(),
                            _ => Vec::new(),
                        },
                        _ => o.last().into_iter().collect(),
                    };
                    self.show_text(&items, font, &mut ts, &mut tm, ctm);
                }
                "Do" => {
                    let (Some(Object::Name(name)), Some(xo)) = (o.first(), xobjects) else { continue };
                    let Ok(obj_ref) = xo.get(name) else { continue };
                    let id = if let Object::Reference(id) = obj_ref { Some(*id) } else { None };
                    let Object::Stream(stream) = resolve(self.doc, obj_ref) else { continue };
                    let sub = stream.dict.get(b"Subtype").map(name_str).unwrap_or_default();
                    self.visits += 1;
                    if sub == "Form" && depth < MAX_FORM_DEPTH && self.visits < MAX_FORM_VISITS {
                        let fm = stream.dict.get(b"Matrix").ok().and_then(|m| match resolve(self.doc, m) {
                            Object::Array(a) => matrix(a),
                            _ => None,
                        });
                        let fm = fm.unwrap_or(M::ID).mul(ctm);
                        let res = stream.dict.get(b"Resources").ok().and_then(|r| dict_of(self.doc, r)).or(resources);
                        if let Ok(data) = stream.decompressed_content_with_limit(MAX_DECODED).or_else(|_| Ok::<_, ()>(stream.content.clone())) {
                            self.run_content(&data, res, fm, depth + 1);
                        }
                    } else if sub == "Image" {
                        self.image(stream, id, ctm);
                    }
                }
                _ => {}
            }
        }
    }

    fn show_text(&mut self, items: &[&Object], font: &FontInfo<'_>, ts: &mut TextState, tm: &mut M, ctm: M) {
        if self.text_bytes > MAX_PAGE_TEXT {
            return;
        }
        if font.undecodable {
            self.undecodable = true;
        }
        let size = ts.size;
        let trm_scale = |tm: &M| M([size * ts.tz, 0.0, 0.0, size, 0.0, ts.ts]).mul(*tm).mul(ctm);
        // Uppskatta spärrning för TJ-strängen: nedre tredjedelen av mellanrummen (max 0,5 em).
        let gaps: Vec<f32> = items
            .iter()
            .filter_map(|o| if let Object::Integer(_) | Object::Real(_) = o { Some(-num(o) / 1000.0) } else { None })
            .filter(|g| *g > 0.0)
            .collect();
        let tracking = if gaps.len() >= 3 {
            let mut g = gaps.clone();
            g.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            g[g.len() / 3].min(0.5)
        } else {
            0.0
        };

        let mut text = String::new();
        let mut start: Option<(f32, f32)> = None;
        let mut eff_size = 0.0;
        for item in items {
            match item {
                Object::String(bytes, _) => {
                    let decoded = font.decode(bytes);
                    let codes = font.codes(bytes);
                    let trm = trm_scale(tm);
                    if start.is_none() {
                        start = Some(trm.apply(0.0, 0.0));
                        eff_size = trm.scale_y();
                    }
                    if !(text.ends_with(' ') && decoded.starts_with(' ')) {
                        text.push_str(&decoded);
                    } else {
                        text.push_str(decoded.trim_start_matches(' '));
                    }
                    // Flytta textmatrisen med glyfernas bredd.
                    let mut adv = 0.0;
                    for c in &codes {
                        let w = font.width(*c) * size + ts.tc + if *c == 32 && !font.two_byte { ts.tw } else { 0.0 };
                        adv += w * ts.tz;
                    }
                    *tm = M::translate(adv, 0.0).mul(*tm);
                }
                Object::Integer(_) | Object::Real(_) => {
                    let n = num(item);
                    let dx = -n / 1000.0 * size * ts.tz;
                    *tm = M::translate(dx, 0.0).mul(*tm);
                    // Stort mellanrum i TJ = mellanslag.
                    if -n / 1000.0 - tracking > 0.2 && !text.is_empty() && !text.ends_with(' ') {
                        text.push(' ');
                    }
                }
                _ => {}
            }
        }
        let Some((x, y)) = start else { return };
        if text.is_empty() {
            return;
        }
        let (x_end, _) = trm_scale(tm).apply(0.0, 0.0);
        // Bara vågrät text (±3°) tas med i flödet.
        let angle = trm_scale(tm).angle();
        if angle.abs() > 3.0 {
            return;
        }
        self.text_bytes += text.len();
        self.runs.push(Run {
            x,
            y,
            x_end: x_end.max(x),
            size: eff_size.max(0.1),
            text,
            bold: font.bold,
            italic: font.italic,
            mono: font.mono,
            invisible: ts.render == 3,
        });
    }

    fn image(&mut self, stream: &lopdf::Stream, id: Option<ObjectId>, ctm: M) {
        let (x0, y0) = ctm.apply(0.0, 0.0);
        let (x1, y1) = ctm.apply(1.0, 1.0);
        let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
        // Pyttesmå bilder (linjer, prickar) hoppas över.
        if w < 24.0 || h < 24.0 {
            return;
        }
        let name = if let Some(name) = id.and_then(|i| self.image_by_id.get(&i)) {
            name.clone()
        } else {
            if self.found_images.len() >= MAX_IMAGES {
                return;
            }
            let Some((bytes, ext)) = extract_image(self.doc, stream) else { return };
            let name = format!("pdf-bild{}.{ext}", self.found_images.len() + 1);
            // Storleken på sidan i EMU (1 pt = 12 700 EMU).
            let extent = Some(((w * 12_700.0) as i64, (h * 12_700.0) as i64));
            self.found_images.push((name.clone(), DocImage { bytes, extent }));
            if let Some(i) = id {
                self.image_by_id.insert(i, name.clone());
            }
            name
        };
        self.images.push(ImageHit { x: x0.min(x1), y: y0.max(y1), w, h, name });
    }
}

/// Plockar ut en bild: JPEG som den är, annars avkodade pixlar som PNG.
fn extract_image(doc: &Document, s: &lopdf::Stream) -> Option<(Vec<u8>, &'static str)> {
    let filters: Vec<String> = match s.dict.get(b"Filter").map(|o| resolve(doc, o)) {
        Ok(Object::Name(n)) => vec![String::from_utf8_lossy(n).into_owned()],
        Ok(Object::Array(a)) => a.iter().map(name_str).collect(),
        _ => Vec::new(),
    };
    if filters.last().is_some_and(|f| f == "DCTDecode") && filters.len() == 1 {
        if s.content.len() > MAX_IMAGE_BYTES {
            return None;
        }
        return Some((s.content.clone(), "jpeg"));
    }
    if filters.iter().any(|f| f == "JPXDecode" || f == "JBIG2Decode" || f == "CCITTFaxDecode" || f == "DCTDecode") {
        return None;
    }
    let get = |k: &[u8]| s.dict.get(k).map(|o| num(resolve(doc, o)) as u32).unwrap_or(0);
    let (w, h, bpc) = (get(b"Width"), get(b"Height"), get(b"BitsPerComponent").max(1));
    if w == 0 || h == 0 || bpc != 8 || (w as u64 * h as u64) > 40_000_000 {
        return None;
    }
    let cs = s.dict.get(b"ColorSpace").map(|o| resolve(doc, o)).map(|o| match o {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::Array(a) => a.first().map(name_str).unwrap_or_default(),
        _ => String::new(),
    });
    let channels = match cs.as_deref() {
        Ok("DeviceRGB") | Ok("CalRGB") => 3,
        Ok("DeviceGray") | Ok("CalGray") => 1,
        Ok("ICCBased") => {
            // Antalet kanaler står i ICC-profilens /N.
            let n = s
                .dict
                .get(b"ColorSpace")
                .ok()
                .map(|o| resolve(doc, o))
                .and_then(|o| if let Object::Array(a) = o { a.get(1) } else { None })
                .and_then(|o| dict_of(doc, o))
                .and_then(|d| d.get(b"N").ok().map(num))
                .unwrap_or(3.0) as u32;
            if n == 1 || n == 3 { n } else { return None }
        }
        _ => return None,
    };
    let raw = s.decompressed_content_with_limit(MAX_DECODED).ok()?;
    if raw.len() < (w * h * channels) as usize {
        return None;
    }
    let raw = &raw[..(w * h * channels) as usize];
    let img: image::DynamicImage = if channels == 3 {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, raw.to_vec())?)
    } else {
        image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, raw.to_vec())?)
    };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some((out.into_inner(), "png"))
}

// ============================================================== rader och block

#[derive(Clone, Debug)]
struct Seg {
    text: String,
    bold: bool,
    italic: bool,
    mono: bool,
}

#[derive(Clone, Debug)]
struct Line {
    x0: f32,
    x1: f32,
    y: f32,
    size: f32,
    segs: Vec<Seg>,
}

impl Line {
    fn text(&self) -> String {
        self.segs.iter().map(|s| s.text.as_str()).collect()
    }
}

#[derive(Clone, Debug)]
enum Item {
    Text { lines: Vec<Line> },
    Image(ImageHit),
    Table { rows: Vec<Vec<String>>, x0: f32, x1: f32, top: f32, bottom: f32 },
}

impl Item {
    fn top(&self) -> f32 {
        match self {
            Item::Text { lines } => lines[0].y + lines[0].size,
            Item::Image(i) => i.y,
            Item::Table { top, .. } => *top,
        }
    }
    fn bottom(&self) -> f32 {
        match self {
            Item::Text { lines } => lines.last().map_or(0.0, |l| l.y - l.size * 0.3),
            Item::Image(i) => i.y - i.h,
            Item::Table { bottom, .. } => *bottom,
        }
    }
    fn x0(&self) -> f32 {
        match self {
            Item::Text { lines } => lines.iter().map(|l| l.x0).fold(f32::MAX, f32::min),
            Item::Image(i) => i.x,
            Item::Table { x0, .. } => *x0,
        }
    }
    fn x1(&self) -> f32 {
        match self {
            Item::Text { lines } => lines.iter().map(|l| l.x1).fold(f32::MIN, f32::max),
            Item::Image(i) => i.x + i.w,
            Item::Table { x1, .. } => *x1,
        }
    }
    fn line_h(&self) -> f32 {
        match self {
            Item::Text { lines } => lines[0].size * 1.2,
            Item::Image(_) | Item::Table { .. } => 14.0,
        }
    }
}

fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{FB00}' => out.push_str("ff"),
            '\u{FB01}' => out.push_str("fi"),
            '\u{FB02}' => out.push_str("fl"),
            '\u{FB03}' => out.push_str("ffi"),
            '\u{FB04}' => out.push_str("ffl"),
            '\u{FB05}' | '\u{FB06}' => out.push_str("st"),
            '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => {}
            '\u{00A0}' => out.push(' '),
            // Symbol/Wingdings-punkter i privata området.
            '\u{F0B7}' | '\u{F0A7}' | '\u{F0D8}' | '\u{F076}' => out.push('•'),
            c if c.is_control() && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// Bygger rader av körningar på samma baslinje.
fn build_lines(mut runs: Vec<Run>) -> Vec<Line> {
    // Hoppa över överskrivna dubbletter ("falsk fetstil").
    runs.sort_by(|a, b| b.y.partial_cmp(&a.y).unwrap_or(std::cmp::Ordering::Equal).then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal)));
    let mut dedup: Vec<Run> = Vec::with_capacity(runs.len());
    for r in runs {
        if dedup.iter().rev().take(4).any(|d| d.text == r.text && (d.x - r.x).abs() < r.size * 0.1 && (d.y - r.y).abs() < r.size * 0.1) {
            continue;
        }
        dedup.push(r);
    }

    let mut lines: Vec<Line> = Vec::new();
    // Gruppera på baslinje, sortera sedan i x.
    let mut groups: Vec<Vec<Run>> = Vec::new();
    for r in dedup {
        match groups.last_mut() {
            Some(g) if (g[0].y - r.y).abs() <= g[0].size.max(r.size) * 0.3 => g.push(r),
            _ => groups.push(vec![r]),
        }
    }
    for mut g in groups {
        g.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let mut cur: Option<Line> = None;
        for r in g {
            let text = clean(&r.text);
            if text.is_empty() {
                continue;
            }
            let seg = Seg { text: text.clone(), bold: r.bold, italic: r.italic, mono: r.mono };
            match cur.as_mut() {
                // Långt glapp (mer än ~1 em) = ny spalt eller tabellcell: egen rad.
                Some(l) if r.x - l.x1 < r.size * 1.0 => {
                    let gap = r.x - l.x1;
                    let ends_space = l.segs.last().is_some_and(|s| s.text.ends_with(' '));
                    if gap > r.size * 0.2 && !ends_space && !text.starts_with(' ') {
                        l.segs.push(Seg { text: " ".into(), bold: false, italic: false, mono: false });
                    }
                    l.segs.push(seg);
                    l.x1 = l.x1.max(r.x_end);
                    l.size = l.size.max(r.size);
                }
                _ => {
                    if let Some(l) = cur.take() {
                        lines.push(l);
                    }
                    cur = Some(Line { x0: r.x, x1: r.x_end, y: r.y, size: r.size, segs: vec![seg] });
                }
            }
        }
        if let Some(l) = cur.take() {
            lines.push(l);
        }
    }
    lines.retain(|l| !l.text().trim().is_empty());
    merge_detached_markers(lines)
}

/// En punkt eller ett nummer som står för sig självt fogas ihop med texten
/// till höger på samma baslinje (PdfCraft: "detached bullets").
fn merge_detached_markers(lines: Vec<Line>) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    for l in lines {
        if let Some(prev) = out.last_mut() {
            let t = prev.text();
            let t = t.trim();
            let is_marker = is_list_marker(&format!("{t} x")).is_some() && t.chars().count() <= 4;
            let same_line = (prev.y - l.y).abs() <= prev.size * 0.3;
            if is_marker && same_line && l.x0 > prev.x1 && l.x0 - prev.x1 < prev.size * 6.0 {
                let mut segs = vec![Seg { text: format!("{t} "), bold: false, italic: false, mono: false }];
                segs.extend(l.segs);
                prev.segs = segs;
                prev.x1 = l.x1;
                continue;
            }
        }
        out.push(l);
    }
    out
}

fn is_list_marker(text: &str) -> Option<(String, String)> {
    let t = text.trim_start();
    for b in ['•', '◦', '▪', '‣', '–', '-', '*', '·', '●', '○', '■', '□', '➢', '►', '✓', '\u{F0B7}'] {
        if let Some(rest) = t.strip_prefix(b) {
            if rest.starts_with(' ') || rest.starts_with('\t') {
                return Some(("- ".into(), rest.trim_start().to_owned()));
            }
        }
    }
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    if (1..=3).contains(&digits.len()) {
        let rest = &t[digits.len()..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((format!("{digits}. "), r.trim_start().to_owned()));
        }
    }
    None
}

/// Hittar tabeller: flera rader i följd där cellerna börjar på samma x-positioner.
/// Spalter med löptext skiljs ut genom att tabellceller är korta.
fn find_tables(lines: Vec<Line>) -> (Vec<Line>, Vec<Item>) {
    // Rader på samma baslinje (lines är sorterade uppifrån, sedan i x).
    let mut rows: Vec<Vec<Line>> = Vec::new();
    for l in lines {
        match rows.last_mut() {
            Some(r) if (r[0].y - l.y).abs() <= r[0].size * 0.3 => r.push(l),
            _ => rows.push(vec![l]),
        }
    }
    let near = |a: f32, b: f32| (a - b).abs() <= 4.0;
    let mut rest = Vec::new();
    let mut tables = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let cols: Vec<f32> = rows[i].iter().map(|l| l.x0).collect();
        let mut j = i + 1;
        if cols.len() >= 2 {
            while j < rows.len() {
                let gap = rows[j - 1][0].y - rows[j][0].y;
                let fits = rows[j].len() >= 2 && rows[j].iter().all(|l| cols.iter().any(|c| near(*c, l.x0)));
                if !fits || gap > rows[j - 1][0].size * 3.5 {
                    break;
                }
                j += 1;
            }
        }
        let block = &rows[i..j];
        let cells: Vec<&Line> = block.iter().flatten().collect();
        let avg_len = cells.iter().map(|l| l.text().chars().count()).sum::<usize>() as f32 / cells.len().max(1) as f32;
        // Löptext i spalter: raderna fyller spalten och fortsätter ofta med gemen.
        let prose = {
            let ncols = cols.len();
            let mut full = 0usize;
            let mut total = 0usize;
            for c in 0..ncols {
                let col_cells: Vec<&Line> = block.iter().filter_map(|r| r.iter().find(|l| near(cols[c], l.x0))).collect();
                let maxw = col_cells.iter().map(|l| l.x1 - l.x0).fold(0.0, f32::max);
                total += col_cells.len();
                full += col_cells.iter().filter(|l| l.x1 - l.x0 >= maxw * 0.8).count();
            }
            let continues = block
                .iter()
                .skip(1)
                .filter(|r| r.iter().any(|l| l.text().trim_start().chars().next().is_some_and(char::is_lowercase)))
                .count();
            block.len() >= 3 && avg_len > 25.0 && (full as f32 >= total as f32 * 0.6 || continues * 3 >= block.len())
        };
        if block.len() >= 2 && avg_len <= 40.0 && !prose {
            let table_rows: Vec<Vec<String>> = block
                .iter()
                .map(|r| {
                    cols.iter()
                        .map(|c| {
                            r.iter()
                                .find(|l| near(*c, l.x0))
                                .map(|l| segs_md(&l.segs, true).trim().replace('|', "\\|"))
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .collect();
            let x0 = cells.iter().map(|l| l.x0).fold(f32::MAX, f32::min);
            let x1 = cells.iter().map(|l| l.x1).fold(f32::MIN, f32::max);
            let top = block[0][0].y + block[0][0].size;
            let bottom = block[block.len() - 1][0].y - block[block.len() - 1][0].size * 0.3;
            tables.push(Item::Table { rows: table_rows, x0, x1, top, bottom });
            i = j;
        } else {
            rest.extend(rows[i].iter().cloned());
            i += 1;
        }
    }
    (rest, tables)
}

/// Delar upp rader i block (stycken) och placerar bilderna.
fn build_items(lines: Vec<Line>, images: Vec<ImageHit>) -> Vec<Item> {
    let (lines, tables) = find_tables(lines);
    let mut items: Vec<Item> = tables;
    // Rader sorterade uppifrån; ett block växer med rader som ligger under,
    // överlappar i x och har normalt radavstånd.
    let mut open: Vec<Vec<Line>> = Vec::new();
    for l in lines {
        let fit = open.iter_mut().rev().find(|b| {
            let last = b.last().unwrap();
            let gap = last.y - l.y;
            let overlap = l.x0 < last.x1.max(b[0].x1) + 2.0 && l.x1 > b[0].x0 - 2.0;
            let same_size = (last.size - l.size).abs() <= last.size * 0.15;
            let starts_item = is_list_marker(&l.text()).is_some();
            gap > 0.0 && gap <= last.size * 1.75 && overlap && same_size && !starts_item && (l.x0 - b[0].x0).abs() < last.size * 4.0
        });
        match fit {
            Some(b) => b.push(l),
            None => open.push(vec![l]),
        }
    }
    items.extend(open.into_iter().map(|lines| Item::Text { lines }));
    items.extend(images.into_iter().map(Item::Image));
    items
}

/// Läsordning: uppifrån, och nedåt i samma spalt innan nästa spalt (PdfCraft).
fn reading_order(items: Vec<Item>) -> Vec<Item> {
    if items.len() > 2000 {
        let mut items = items;
        items.sort_by(|a, b| b.top().partial_cmp(&a.top()).unwrap_or(std::cmp::Ordering::Equal));
        return items;
    }
    let mut unread: Vec<Option<Item>> = items.into_iter().map(Some).collect();
    let mut out = Vec::new();
    while unread.iter().any(Option::is_some) {
        // Start: översta blocket; vid ungefär samma höjd vinner det vänstra.
        let max_top = unread.iter().flatten().map(Item::top).fold(f32::MIN, f32::max);
        let start = unread
            .iter()
            .enumerate()
            .filter_map(|(i, it)| it.as_ref().map(|it| (i, it)))
            .filter(|(_, it)| it.top() >= max_top - it.line_h() * 1.5)
            .min_by(|a, b| a.1.x0().partial_cmp(&b.1.x0()).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap();
        let mut cur = unread[start].take().unwrap();
        loop {
            let (cx0, cx1, cb, lh) = (cur.x0(), cur.x1(), cur.bottom(), cur.line_h());
            // Nästa block nedåt i samma spalt: överlappar i x, ligger nära under.
            let candidates: Vec<(usize, &Item)> = unread
                .iter()
                .enumerate()
                .filter_map(|(i, it)| it.as_ref().map(|it| (i, it)))
                .filter(|(_, it)| it.x0() < cx1 && it.x1() > cx0 && it.top() <= cb + lh * 0.5 && cb - it.top() <= lh * 3.5)
                .collect();
            // Högst upp först; block på ungefär samma höjd (spalter) – det vänstra.
            let best_top = candidates.iter().map(|(_, it)| it.top()).fold(f32::MIN, f32::max);
            let next = candidates
                .iter()
                .filter(|(_, it)| it.top() >= best_top - it.line_h())
                .min_by(|a, b| a.1.x0().partial_cmp(&b.1.x0()).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| *i);
            // Inget oläst block får ligga ovanför i samma spalt (det skulle läsas först).
            let blocked = next.is_some_and(|n| {
                let nt = unread[n].as_ref().unwrap().top();
                unread.iter().flatten().any(|it| it.top() > nt + 1.0 && it.top() > cb + lh && it.x0() < cx1 && it.x1() > cx0)
            });
            out.push(cur);
            match next {
                Some(n) if !blocked => cur = unread[n].take().unwrap(),
                _ => break,
            }
        }
    }
    out
}

// ============================================================== Markdown

fn segs_md(segs: &[Seg], allow_style: bool) -> String {
    // Slå ihop intilliggande segment med samma stil.
    let mut merged: Vec<Seg> = Vec::new();
    for s in segs {
        match merged.last_mut() {
            Some(m) if (m.bold == s.bold && m.italic == s.italic && m.mono == s.mono) || s.text.trim().is_empty() => m.text.push_str(&s.text),
            _ => merged.push(s.clone()),
        }
    }
    let all_bold = merged.iter().all(|s| s.bold || s.text.trim().is_empty());
    let all_italic = merged.iter().all(|s| s.italic || s.text.trim().is_empty());
    let all_mono = merged.iter().all(|s| s.mono || s.text.trim().is_empty());
    merged
        .iter()
        .map(|s| {
            if s.mono && !all_mono && allow_style && !s.text.contains('`') && !s.text.trim().is_empty() {
                return wrap(&s.text, "`");
            }
            let mut t = escape_inline(&s.text);
            if allow_style && s.italic && !all_italic {
                t = wrap(&t, "*");
            }
            if allow_style && s.bold && !all_bold {
                t = wrap(&t, "**");
            }
            t
        })
        .collect()
}

/// Fogar ihop raderna i ett stycke; avstavning i radslut tas bort före gemen.
fn join_lines(lines: &[Line]) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            let prev_ends_hyphen = segs.last().is_some_and(|s| s.text.ends_with('-') && !s.text.ends_with(" -"));
            let next_lower = l.text().trim_start().chars().next().is_some_and(char::is_lowercase);
            if prev_ends_hyphen && next_lower {
                if let Some(s) = segs.last_mut() {
                    s.text.pop();
                }
            } else if !segs.last().is_some_and(|s| s.text.ends_with(' ')) {
                segs.push(Seg { text: " ".into(), bold: false, italic: false, mono: false });
            }
        }
        let mut ls = l.segs.clone();
        if let Some(first) = ls.first_mut() {
            if i > 0 {
                first.text = first.text.trim_start().to_owned();
            }
        }
        segs.extend(ls);
    }
    if let Some(last) = segs.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }
    segs
}

fn is_page_furniture(item: &Item, page_h: f32) -> bool {
    let Item::Text { lines } = item else { return false };
    if lines.len() != 1 {
        return false;
    }
    let l = &lines[0];
    let t = l.text().trim().to_lowercase();
    let near_edge = l.y < page_h * 0.08 || l.y > page_h * 0.94;
    if !near_edge || t.len() > 24 {
        return false;
    }
    // Sidnummer: "3", "- 3 -", "3 av 10", "sida 3", "page 3 of 10"
    let words: Vec<&str> = t.split(|c: char| c.is_whitespace() || c == '/' || c == '-' || c == '–').filter(|w| !w.is_empty()).collect();
    words.iter().any(|w| w.chars().all(|c| c.is_ascii_digit()))
        && words.iter().all(|w| w.chars().all(|c| c.is_ascii_digit()) || matches!(*w, "av" | "of" | "sida" | "page" | "s." | "p."))
}

struct PageOut {
    items: Vec<Item>,
}

fn to_markdown(pages: &[PageOut], body: f32) -> String {
    let mut out: Vec<String> = Vec::new();
    // Ett stycke som fortsätter på nästa sida fogas ihop.
    let mut prev_open_para = false;
    for page in pages {
        for (ii, it) in page.items.iter().enumerate() {
            match it {
                Item::Image(img) => {
                    out.push(format!("![]({IMAGE_SCHEME}{})", img.name));
                    prev_open_para = false;
                }
                Item::Table { rows, .. } => {
                    let cols = rows.iter().map(Vec::len).max().unwrap_or(1);
                    let row_md = |r: &Vec<String>| {
                        let mut cells: Vec<String> = r.iter().map(|c| if c.is_empty() { " ".into() } else { c.clone() }).collect();
                        cells.resize(cols, " ".into());
                        format!("| {} |", cells.join(" | "))
                    };
                    let mut t = row_md(&rows[0]);
                    t.push_str(&format!("\n|{}", " --- |".repeat(cols)));
                    for r in &rows[1..] {
                        t.push('\n');
                        t.push_str(&row_md(r));
                    }
                    out.push(t);
                    prev_open_para = false;
                }
                Item::Text { lines } => {
                    let first_text = lines[0].text();
                    let size = lines.iter().map(|l| l.size).fold(0.0, f32::max);
                    let segs = join_lines(lines);
                    let plain: String = segs.iter().map(|s| s.text.as_str()).collect();
                    let chars = plain.chars().count();
                    let all_bold = segs.iter().all(|s| s.bold || s.text.trim().is_empty());
                    let level = if chars < 200 && size >= body * 1.6 {
                        Some(1)
                    } else if chars < 200 && size >= body * 1.25 {
                        Some(2)
                    } else if all_bold && chars < 80 && size >= body * 0.95 && !plain.trim_end().ends_with(['.', ',', ':', ';']) && lines.len() <= 2 {
                        Some(3)
                    } else {
                        None
                    };
                    if let Some(h) = level {
                        out.push(format!("{} {}", "#".repeat(h), segs_md(&segs, false).replace('\n', " ").trim()));
                        prev_open_para = false;
                        continue;
                    }
                    if let Some((marker, _)) = is_list_marker(&first_text) {
                        // Ta bort markören ur första segmentet.
                        let mut segs = segs.clone();
                        if let Some(first) = segs.iter_mut().find(|s| !s.text.trim().is_empty()) {
                            if let Some((_, rest)) = is_list_marker(&first.text) {
                                first.text = rest;
                            } else {
                                // Markören låg i ett eget segment.
                                first.text.clear();
                            }
                        }
                        while segs.first().is_some_and(|s| s.text.trim().is_empty()) {
                            segs.remove(0);
                        }
                        let text = segs_md(&segs, true);
                        let item = format!("{marker}{}", text.trim());
                        // Listpunkter i följd (av samma sort) skrivs utan tom rad emellan.
                        let numbered = marker.starts_with(|c: char| c.is_ascii_digit());
                        let same_kind = out.last().is_some_and(|l: &String| {
                            let last_line = l.lines().last().unwrap_or("");
                            is_list_marker(last_line).is_some_and(|(m, _)| m.starts_with(|c: char| c.is_ascii_digit()) == numbered)
                        });
                        if same_kind {
                            let last = out.pop().unwrap();
                            out.push(format!("{last}\n{item}"));
                        } else {
                            out.push(item);
                        }
                        prev_open_para = false;
                        continue;
                    }
                    let md = segs_md(&segs, true);
                    let md = escape_line_start(md.trim());
                    let starts_lower = plain.trim_start().chars().next().is_some_and(char::is_lowercase);
                    if ii == 0 && prev_open_para && starts_lower {
                        if let Some(last) = out.last_mut() {
                            if last.ends_with('-') {
                                last.pop();
                            } else {
                                last.push(' ');
                            }
                            last.push_str(&md);
                            prev_open_para = !plain.trim_end().ends_with(['.', '!', '?', ':']);
                            continue;
                        }
                    }
                    prev_open_para = !plain.trim_end().ends_with(['.', '!', '?', ':', '"', '”']);
                    out.push(md);
                }
            }
        }
    }
    let mut md = out.join("\n\n");
    md.push('\n');
    md
}

// ============================================================== in

pub fn read(bytes: &[u8], password: Option<&str>) -> Result<Loaded, ReadError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(ReadError::Other("PDF-filen är för stor för att öppnas.".into()));
    }
    if !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err(ReadError::Other("Filen är inte en PDF.".into()));
    }
    let mut opts = lopdf::LoadOptions::default();
    opts.max_decompressed_size = Some(MAX_DECODED);
    if let Some(p) = password {
        opts.password = Some(p.to_owned());
    }
    let doc = match Document::load_mem_with_options(bytes, opts) {
        Ok(d) => d,
        Err(lopdf::Error::InvalidPassword) => return Err(ReadError::WrongPassword),
        Err(e) => return Err(ReadError::Other(format!("PDF-filen kunde inte läsas: {e}"))),
    };
    // Krypterad utan att lösenordet godtogs: be om lösenord.
    if doc.trailer.get(b"Encrypt").is_ok() && doc.is_encrypted() {
        let ok = doc.authenticate_password(password.unwrap_or("")).is_ok() || doc.authenticate_password("").is_ok();
        if !ok {
            return Err(if password.is_some() { ReadError::WrongPassword } else { ReadError::NeedsPassword });
        }
    }

    let pages = doc.get_pages();
    let mut found_images: Vec<(String, DocImage)> = Vec::new();
    let mut image_by_id: HashMap<ObjectId, String> = HashMap::new();
    let mut page_out = Vec::new();
    let mut size_hist: HashMap<i32, usize> = HashMap::new();
    let mut pages_without_text = 0;
    let mut undecodable = false;
    let mut has_annots = false;
    let mut has_forms = false;
    let mut seen_pages: HashSet<ObjectId> = HashSet::new();

    for (_, page_id) in pages.iter() {
        if !seen_pages.insert(*page_id) {
            continue;
        }
        let Ok(page) = doc.get_dictionary(*page_id) else { continue };
        if let Ok(a) = page.get(b"Annots").map(|o| resolve(&doc, o)) {
            if let Object::Array(arr) = a {
                for an in arr {
                    if let Some(d) = dict_of(&doc, an) {
                        let st = d.get(b"Subtype").map(name_str).unwrap_or_default();
                        if st == "Widget" {
                            has_forms = true;
                        } else if st != "Link" && st != "Popup" {
                            has_annots = true;
                        }
                    }
                }
            }
        }
        // Sidans höjd (för sidhuvud/sidfot).
        let page_h = page
            .get(b"MediaBox")
            .ok()
            .map(|o| resolve(&doc, o))
            .and_then(|o| if let Object::Array(a) = o { Some(a.iter().map(num).collect::<Vec<_>>()) } else { None })
            .filter(|v| v.len() == 4)
            .map(|v| (v[3] - v[1]).abs())
            .unwrap_or(842.0);
        let resources = doc.get_page_resources(*page_id).ok().and_then(|(d, ids)| {
            d.or_else(|| ids.first().and_then(|id| doc.get_dictionary(*id).ok()))
        });
        let content = doc.get_page_content(*page_id);
        let mut c = Collector {
            doc: &doc,
            runs: Vec::new(),
            images: Vec::new(),
            found_images: &mut found_images,
            image_by_id: &mut image_by_id,
            text_bytes: 0,
            visits: 0,
            undecodable: false,
            has_annotations_or_forms: false,
        };
        c.run_content(&content, resources, M::ID, 0);
        let _ = c.has_annotations_or_forms;
        undecodable |= c.undecodable;
        let images = std::mem::take(&mut c.images);
        let mut runs = std::mem::take(&mut c.runs);
        // Osynlig text (OCR-lager) används bara om sidan saknar synlig text.
        if runs.iter().any(|r| !r.invisible && !r.text.trim().is_empty()) {
            runs.retain(|r| !r.invisible);
        }
        if runs.iter().all(|r| r.text.trim().is_empty()) {
            pages_without_text += 1;
        }
        for r in &runs {
            let bucket = (r.size * 2.0).round() as i32;
            *size_hist.entry(bucket).or_default() += r.text.chars().filter(|c| !c.is_whitespace()).count();
        }
        let lines = build_lines(runs);
        let items = build_items(lines, images);
        let mut items = reading_order(items);
        items.retain(|it| !is_page_furniture(it, page_h));
        page_out.push(PageOut { items });
    }

    // Brödtextens storlek: den storlek som flest tecken har.
    let body = size_hist.iter().max_by_key(|(_, n)| **n).map(|(b, _)| *b as f32 / 2.0).unwrap_or(11.0);
    let markdown = to_markdown(&page_out, body);

    // En PDF som Kladd själv har skapat förlorar inget när den sparas om.
    let producer = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| dict_of(&doc, o))
        .and_then(|d| d.get(b"Producer").ok())
        .map(|o| match resolve(&doc, o) {
            Object::String(b, _) => lopdf::decode_text_string(&Object::String(b.clone(), lopdf::StringFormat::Literal)).unwrap_or_default(),
            _ => String::new(),
        })
        .unwrap_or_default();
    let made_by_kladd = producer.starts_with("Kladd");
    let mut dropped = if made_by_kladd { Vec::new() } else { vec!["sidlayouten", "typsnitten"] };
    // Kladd sparar utan lösenord: säg till om originalet krävde ett.
    if password.is_some_and(|p| !p.is_empty()) {
        dropped.push("lösenordsskyddet");
    }
    if has_forms {
        dropped.push("formulärfält");
    }
    if has_annots {
        dropped.push("anteckningar och markeringar");
    }
    if undecodable {
        dropped.push("text i typsnitt som inte kan avkodas");
    }
    Ok(Loaded { markdown, images: found_images, dropped, pages_without_text, page_count: page_out.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_through_our_own_pdf() {
        let md = "# Planering för fritids\n\nVi ska baka bullar och måla höstlöv i dag, och sedan gå ut på gården en stund om vädret tillåter.\n\n## Att ta med\n\n- Förkläde\n- Mjöl\n\n1. Värm ugnen\n2. Blanda\n\nSista stycket med åäö.\n";
        let pdf = crate::pdf_write::write(md, "Test", &|_| None).unwrap().bytes;
        let back = match read(&pdf, None) {
            Ok(b) => b,
            Err(ReadError::Other(e)) => panic!("{e}"),
            Err(_) => panic!("lösenord"),
        };
        let m = &back.markdown;
        assert!(m.starts_with("# Planering för fritids\n"), "{m}");
        assert!(m.contains("\n## Att ta med\n"), "{m}");
        assert!(m.contains("Vi ska baka bullar och måla höstlöv i dag, och sedan gå ut på gården en stund om vädret tillåter."), "{m}");
        assert!(m.contains("- Förkläde\n- Mjöl"), "{m}");
        assert!(m.contains("1. Värm ugnen\n2. Blanda"), "{m}");
        assert!(m.contains("Sista stycket med åäö."), "{m}");
        assert_eq!(back.page_count, 1);
    }

    #[test]
    fn rejects_non_pdf() {
        assert!(matches!(read(b"hej", None), Err(ReadError::Other(_))));
    }

    #[test]
    fn list_markers() {
        assert_eq!(is_list_marker("• Punkt"), Some(("- ".into(), "Punkt".into())));
        assert_eq!(is_list_marker("12) Tolv"), Some(("12. ".into(), "Tolv".into())));
        assert_eq!(is_list_marker("2024 var ett år"), None);
        assert_eq!(clean("ﬁka\u{00AD}rast"), "fikarast");
    }
}

/// `KLADD_PDF_IN=a.pdf[:b.pdf] [KLADD_PDF_PASSWORD=x] cargo test pdf_read_manual -- --ignored --nocapture`
#[cfg(test)]
mod manual {
    #[test]
    #[ignore]
    fn pdf_read_manual() {
        let Ok(files) = std::env::var("KLADD_PDF_IN") else { return };
        let pw = std::env::var("KLADD_PDF_PASSWORD").ok();
        for f in files.split(':') {
            let bytes = std::fs::read(f).unwrap();
            match super::read(&bytes, pw.as_deref()) {
                Ok(l) => println!(
                    "===== {f} (sidor {}, utan text {}, tappat {:?}, bilder {:?})\n{}",
                    l.page_count,
                    l.pages_without_text,
                    l.dropped,
                    l.images.iter().map(|(n, i)| (n.clone(), i.bytes.len())).collect::<Vec<_>>(),
                    l.markdown
                ),
                Err(super::ReadError::NeedsPassword) => println!("===== {f}: BEHÖVER LÖSENORD"),
                Err(super::ReadError::WrongPassword) => println!("===== {f}: FEL LÖSENORD"),
                Err(super::ReadError::Other(e)) => println!("===== {f}: FEL {e}"),
            }
        }
    }
}
