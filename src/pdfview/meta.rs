//! Innehållsförteckning (bokmärken), länkar, sidetiketter och dokumentinfo ur en PDF (med `lopdf`).
//!
//! Koordinater: alla rektanglar och y-värden är *visningspunkter*: origo uppe till vänster i sidans
//! beskärningsruta (`/CropBox`, annars `/MediaBox`) efter sidans `/Rotate`, y nedåt, 1 enhet = 1 pt.
//! Användarens extra vyrotation läggs på av visaren, inte här.
#![allow(dead_code)]

use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::{HashMap, HashSet};

/// Mål för ett hopp: sida (0-baserad) och ev. överkant i visningspunkter.
#[derive(Clone, Debug, PartialEq)]
pub struct Dest {
    pub page: usize,
    /// Överkant i visningspunkter. För sidor roterade 90/270 grader är PDF:ens "top" sidans
    /// x-led i bild; då räknas y fram ur målets `left` (se `Geo::dest_y`).
    pub y: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Page(Dest),
    Uri(String),
}

#[derive(Clone, Debug)]
pub struct Link {
    /// Visningspunkter.
    pub rect: egui::Rect,
    pub target: LinkTarget,
}

#[derive(Clone, Debug, Default)]
pub struct OutlineItem {
    pub title: String,
    pub dest: Option<Dest>,
    pub children: Vec<OutlineItem>,
    /// Bokmärket är utfällt som standard (`/Count` > 0).
    pub open: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DocInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    /// "ÅÅÅÅ-MM-DD TT:MM" (ingen tidszonsomräkning).
    pub created: Option<String>,
    pub modified: Option<String>,
    /// T.ex. "1.7"; tom om okänd.
    pub version: String,
}

#[derive(Clone, Debug, Default)]
pub struct PdfMeta {
    pub outline: Vec<OutlineItem>,
    /// En lista per sida.
    pub links: Vec<Vec<Link>>,
    /// En etikett per sida; tomma strängar när dokumentet saknar sidetiketter.
    pub labels: Vec<String>,
    pub info: DocInfo,
}

const MAX_LINKS_PER_PAGE: usize = 2000;
const MAX_OUTLINE_ITEMS: usize = 20_000;
const MAX_OUTLINE_DEPTH: usize = 32;
const MAX_NAMES: usize = 200_000;
const MAX_DECODED: usize = 256 * 1024 * 1024;

/// Läser metadata. Panikar aldrig; ger `PdfMeta::default()` vid fel.
pub fn read_meta(bytes: &[u8], password: Option<&str>) -> PdfMeta {
    std::panic::catch_unwind(|| read_meta_inner(bytes, password)).unwrap_or_default()
}

fn read_meta_inner(bytes: &[u8], password: Option<&str>) -> PdfMeta {
    if !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return PdfMeta::default();
    }
    let mut opts = lopdf::LoadOptions::default();
    opts.max_decompressed_size = Some(MAX_DECODED);
    if let Some(p) = password {
        opts.password = Some(p.to_owned());
    }
    let Ok(doc) = Document::load_mem_with_options(bytes, opts) else { return PdfMeta::default() };
    if doc.trailer.get(b"Encrypt").is_ok() && doc.is_encrypted() {
        let ok = doc.authenticate_password(password.unwrap_or("")).is_ok() || doc.authenticate_password("").is_ok();
        if !ok {
            return PdfMeta::default();
        }
    }
    build(&doc, bytes)
}

fn build(doc: &Document, bytes: &[u8]) -> PdfMeta {
    let pages = doc.get_pages();
    let page_ids: Vec<ObjectId> = pages.values().copied().collect();
    let cx = Cx {
        d: doc,
        page_index: page_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect(),
        geo: page_ids.iter().map(|id| Geo::of(doc, *id)).collect(),
        names: collect_names(doc),
    };
    PdfMeta {
        outline: cx.outline(),
        links: page_ids.iter().enumerate().map(|(i, id)| cx.page_links(i, *id)).collect(),
        labels: page_labels(&cx, page_ids.len()),
        info: read_info(doc, bytes),
    }
}

// ============================================================== geometri

/// Sidans beskärningsruta (användarrymd) och rotation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geo {
    pub(crate) x0: f32,
    pub(crate) y0: f32,
    pub(crate) x1: f32,
    pub(crate) y1: f32,
    /// 0, 90, 180 eller 270 (medurs).
    pub(crate) rot: u16,
}

impl Geo {
    pub(crate) fn of(doc: &Document, page: ObjectId) -> Geo {
        let rect = |key: &[u8]| inherited(doc, page, key).and_then(|o| rect_of(doc, &o));
        let (x0, y0, x1, y1) = rect(b"CropBox").or_else(|| rect(b"MediaBox")).unwrap_or((0.0, 0.0, 612.0, 792.0));
        let rot = inherited(doc, page, b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0);
        let rot = (rot.rem_euclid(360) / 90 * 90) as u16;
        Geo { x0, y0, x1, y1, rot }
    }

    /// Sidans storlek i visningspunkter (efter rotation).
    fn size(&self) -> (f32, f32) {
        let (w, h) = (self.x1 - self.x0, self.y1 - self.y0);
        if self.rot % 180 == 90 { (h, w) } else { (w, h) }
    }

    /// Användarrymd (x, y uppåt) → visningspunkter (origo uppe till vänster, y nedåt).
    pub(crate) fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let (w, h) = (self.x1 - self.x0, self.y1 - self.y0);
        let (u, v) = (x - self.x0, self.y1 - y); // oroterat, y nedåt
        match self.rot {
            90 => (h - v, u),
            180 => (w - u, h - v),
            270 => (v, w - u),
            _ => (u, v),
        }
    }

    /// Rektangel i användarrymd (x0, y0, x1, y1) → normaliserad rektangel i visningspunkter,
    /// begränsad till sidan.
    fn rect(&self, a: (f32, f32, f32, f32)) -> egui::Rect {
        let (ax, ay) = self.point(a.0, a.1);
        let (bx, by) = self.point(a.2, a.3);
        let (w, h) = self.size();
        egui::Rect::from_min_max(
            egui::pos2(ax.min(bx).clamp(0.0, w), ay.min(by).clamp(0.0, h)),
            egui::pos2(ax.max(bx).clamp(0.0, w), ay.max(by).clamp(0.0, h)),
        )
    }

    /// Målets lodräta position i visningspunkter. Vid 0/180 grader ur PDF:ens `top`; vid 90/270
    /// blir användarrymdens x-led den lodräta, så då används `left` (saknas den, rutans vänsterkant).
    /// `None` när det som behövs saknas (t.ex. /Fit utan position).
    fn dest_y(&self, left: Option<f32>, top: Option<f32>) -> Option<f32> {
        let (_, h) = self.size();
        let y = if self.rot % 180 == 90 {
            self.point(left.unwrap_or(self.x0), self.y1).1
        } else {
            self.point(self.x0, top?).1
        };
        Some(y.clamp(0.0, h))
    }
}

/// Ärvt sidattribut (Parent-kedjan), avreferenserat.
fn inherited(doc: &Document, mut id: ObjectId, key: &[u8]) -> Option<Object> {
    for _ in 0..32 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(o) = d.get(key) {
            return doc.dereference(o).ok().map(|x| x.1.clone());
        }
        id = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn num(doc: &Document, o: &Object) -> Option<f32> {
    let v = doc.dereference(o).ok()?.1.as_float().ok()?;
    v.is_finite().then_some(v)
}

/// `[x0 y0 x1 y1]` normaliserad (min/max); `None` om tom eller ogiltig.
fn rect_of(doc: &Document, o: &Object) -> Option<(f32, f32, f32, f32)> {
    let a = o.as_array().ok()?;
    let v: Vec<f32> = a.iter().filter_map(|x| num(doc, x)).collect();
    if v.len() < 4 {
        return None;
    }
    let r = (v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3]));
    (r.2 - r.0 > 0.0 && r.3 - r.1 > 0.0).then_some(r)
}

// ============================================================== text

const PDFDOC_80: [char; 33] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}', '\u{2039}', '\u{203A}', '\u{2212}',
    '\u{2030}', '\u{201E}', '\u{201C}', '\u{201D}', '\u{2018}', '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}', '\u{0141}',
    '\u{0152}', '\u{0160}', '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}', '\u{017E}', '\u{FFFD}', '\u{20AC}',
];
const PDFDOC_18: [char; 8] = ['\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}', '\u{02DC}'];

/// PDF-textsträng: UTF-16BE/LE med BOM, UTF-8 med BOM eller PDFDocEncoding.
fn decode_text(b: &[u8]) -> String {
    let s: String = if b.starts_with(&[0xFE, 0xFF]) || b.starts_with(&[0xFF, 0xFE]) {
        let le = b[0] == 0xFF;
        let u: Vec<u16> = b[2..]
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&u)
    } else if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&b[3..]).into_owned()
    } else {
        b.iter()
            .map(|&c| match c {
                0x18..=0x1F => PDFDOC_18[(c - 0x18) as usize],
                0x80..=0xA0 => PDFDOC_80[(c - 0x80) as usize],
                _ => c as char,
            })
            .collect()
    };
    s.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect()
}

fn text_of(o: &Object) -> Option<String> {
    match o {
        Object::String(b, _) => Some(decode_text(b)),
        Object::Name(n) => Some(decode_text(n)),
        _ => None,
    }
}

// ============================================================== mål, bokmärken, länkar

struct Cx<'a> {
    d: &'a Document,
    page_index: HashMap<ObjectId, usize>,
    geo: Vec<Geo>,
    /// Namngivna mål: /Dests-ordlistan och /Names /Dests-trädet.
    names: HashMap<Vec<u8>, &'a Object>,
}

/// Mål i användarrymd innan det omvandlas.
struct RawDest {
    page: usize,
    left: Option<f32>,
    top: Option<f32>,
}

fn collect_names(doc: &Document) -> HashMap<Vec<u8>, &Object> {
    let mut out = HashMap::new();
    let Ok(cat) = doc.catalog() else { return out };
    fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
        doc.dereference(o).ok()?.1.as_dict().ok()
    }
    // PDF 1.1: /Dests är en ordlista med namn som nycklar.
    if let Some(d) = cat.get(b"Dests").ok().and_then(|o| dict_of(doc, o)) {
        for (k, v) in d.iter().take(MAX_NAMES) {
            out.insert(k.clone(), v);
        }
    }
    // PDF 1.2+: namnträd under /Names /Dests.
    if let Some(root) = cat.get(b"Names").ok().and_then(|o| dict_of(doc, o)).and_then(|n| n.get(b"Dests").ok()).and_then(|o| dict_of(doc, o)) {
        let mut seen = HashSet::new();
        walk_name_tree(doc, root, 0, &mut seen, &mut out);
    }
    out
}

fn walk_name_tree<'a>(doc: &'a Document, node: &'a Dictionary, depth: usize, seen: &mut HashSet<ObjectId>, out: &mut HashMap<Vec<u8>, &'a Object>) {
    if depth > 16 || out.len() >= MAX_NAMES {
        return;
    }
    if let Ok(Object::Array(names)) = node.get(b"Names") {
        for p in names.chunks(2) {
            if let (Object::String(k, _), Some(v)) = (&p[0], p.get(1)) {
                out.entry(k.clone()).or_insert(v);
            }
        }
    }
    if let Ok(Object::Array(kids)) = node.get(b"Kids") {
        for k in kids {
            if let Object::Reference(id) = k {
                if !seen.insert(*id) {
                    continue;
                }
            }
            if let Some(kd) = doc.dereference(k).ok().and_then(|x| x.1.as_dict().ok()) {
                walk_name_tree(doc, kd, depth + 1, seen, out);
            }
        }
    }
}

impl<'a> Cx<'a> {
    fn dict(&self, o: &'a Object) -> Option<&'a Dictionary> {
        self.d.dereference(o).ok()?.1.as_dict().ok()
    }

    /// Mål ur en destinationsarray, ett namn/en sträng (namngivet mål) eller en ordlista med /D.
    fn raw_dest(&self, o: &'a Object, depth: u32) -> Option<RawDest> {
        if depth > 4 {
            return None;
        }
        match self.d.dereference(o).ok()?.1 {
            Object::Array(a) => {
                let page = match a.first()? {
                    Object::Reference(id) => *self.page_index.get(id)?,
                    Object::Integer(i) if *i >= 0 => *i as usize, // 0-baserat index
                    _ => return None,
                };
                if page >= self.geo.len() {
                    return None;
                }
                let n = |i: usize| a.get(i).and_then(|x| num(self.d, x));
                let (left, top) = match a.get(1).and_then(|x| x.as_name().ok()) {
                    Some(b"XYZ") => (n(2), n(3)),
                    Some(b"FitH") | Some(b"FitBH") => (None, n(2)),
                    Some(b"FitV") | Some(b"FitBV") => (n(2), None),
                    Some(b"FitR") => (n(2), n(5)),
                    _ => (None, None),
                };
                Some(RawDest { page, left, top })
            }
            Object::Dictionary(d) => self.raw_dest(d.get(b"D").ok()?, depth + 1),
            Object::Name(n) => self.raw_dest(self.names.get(n.as_slice())?, depth + 1),
            Object::String(s, _) => self.raw_dest(self.names.get(s.as_slice())?, depth + 1),
            _ => None,
        }
    }

    fn dest(&self, o: &'a Object) -> Option<Dest> {
        let r = self.raw_dest(o, 0)?;
        Some(Dest { page: r.page, y: self.geo[r.page].dest_y(r.left, r.top) })
    }

    /// Mål ur ett objekt med /A (GoTo/URI) eller /Dest.
    fn target(&self, d: &'a Dictionary) -> Option<LinkTarget> {
        if let Some(a) = d.get(b"A").ok().and_then(|o| self.dict(o)) {
            return match a.get(b"S").ok().and_then(|s| s.as_name().ok()) {
                Some(b"URI") => {
                    let uri = self.d.dereference(a.get(b"URI").ok()?).ok()?.1.as_str().ok().map(decode_text)?;
                    let uri = uri.trim();
                    (!uri.is_empty()).then(|| LinkTarget::Uri(uri.to_owned()))
                }
                Some(b"GoTo") => self.dest(a.get(b"D").ok()?).map(LinkTarget::Page),
                _ => None, // GoToR, Launch m.fl. hoppar vi över
            };
        }
        self.dest(d.get(b"Dest").ok()?).map(LinkTarget::Page)
    }

    fn outline(&self) -> Vec<OutlineItem> {
        let Some(root) = self.d.catalog().ok().and_then(|c| c.get(b"Outlines").ok()).and_then(|o| self.dict(o)) else { return vec![] };
        let mut budget = MAX_OUTLINE_ITEMS;
        let mut seen = HashSet::new();
        self.outline_level(root.get(b"First").ok(), 0, &mut budget, &mut seen)
    }

    fn outline_level(&self, first: Option<&'a Object>, depth: usize, budget: &mut usize, seen: &mut HashSet<ObjectId>) -> Vec<OutlineItem> {
        let mut out = vec![];
        let mut cur = first;
        while let Some(o) = cur {
            if depth > MAX_OUTLINE_DEPTH || *budget == 0 {
                break;
            }
            if let Object::Reference(id) = o {
                if !seen.insert(*id) {
                    break; // cykel
                }
            }
            let Some(d) = self.dict(o) else { break };
            *budget -= 1;
            let title = d.get(b"Title").ok().and_then(|t| self.d.dereference(t).ok()).and_then(|t| text_of(t.1)).unwrap_or_default();
            let count = d.get(b"Count").ok().and_then(|c| c.as_i64().ok()).unwrap_or(0);
            let dest = match self.target(d) {
                Some(LinkTarget::Page(p)) => Some(p),
                _ => None,
            };
            out.push(OutlineItem {
                title: title.trim().to_owned(),
                dest,
                children: self.outline_level(d.get(b"First").ok(), depth + 1, budget, seen),
                open: count > 0,
            });
            cur = d.get(b"Next").ok();
        }
        out
    }

    fn page_links(&self, index: usize, page: ObjectId) -> Vec<Link> {
        let geo = self.geo[index];
        let mut out = vec![];
        let Ok(page_dict) = self.d.get_dictionary(page) else { return out };
        let Some(annots) = page_dict.get(b"Annots").ok().and_then(|o| self.d.dereference(o).ok()).and_then(|x| x.1.as_array().ok()) else {
            return out;
        };
        for a in annots {
            if out.len() >= MAX_LINKS_PER_PAGE {
                break;
            }
            let Some(ad) = self.dict(a) else { continue };
            if ad.get(b"Subtype").ok().and_then(|s| s.as_name().ok()) != Some(b"Link") {
                continue;
            }
            let Some(target) = self.target(ad) else { continue };
            // QuadPoints (om de finns) ger en tightare ruta än /Rect.
            let from_quads = ad.get(b"QuadPoints").ok().and_then(|q| self.d.dereference(q).ok()).and_then(|q| q.1.as_array().ok()).and_then(|q| {
                let v: Vec<f32> = q.iter().filter_map(|x| num(self.d, x)).collect();
                if v.len() < 8 {
                    return None;
                }
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for p in v.chunks_exact(2) {
                    x0 = x0.min(p[0]);
                    x1 = x1.max(p[0]);
                    y0 = y0.min(p[1]);
                    y1 = y1.max(p[1]);
                }
                (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
            });
            let rect_entry = || ad.get(b"Rect").ok().and_then(|r| rect_of(self.d, self.d.dereference(r).ok()?.1));
            let Some(r) = from_quads.or_else(rect_entry) else { continue };
            let rect = geo.rect(r);
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                continue;
            }
            out.push(Link { rect, target });
        }
        out
    }
}

// ============================================================== sidetiketter

fn roman(mut n: u32) -> String {
    const T: [(u32, &str); 13] = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s += r;
            n -= v;
        }
    }
    s
}

/// PDF:ens regel: 1–26 = A–Z, 27–52 = AA–ZZ, 53–78 = AAA–ZZZ …
fn alpha(n: u32) -> String {
    let i = n.max(1) - 1;
    let l = (b'A' + (i % 26) as u8) as char;
    std::iter::repeat_n(l, (i / 26 + 1).min(64) as usize).collect()
}

/// Ett intervall i `/PageLabels`: stil, prefix och startvärde.
#[derive(Clone, Debug, Default, PartialEq)]
struct LabelRange {
    style: Option<u8>,
    prefix: String,
    start: i64,
}

impl LabelRange {
    fn label(&self, offset: u32) -> String {
        let n = self.start.saturating_add(offset as i64).clamp(0, u32::MAX as i64) as u32;
        let body = match self.style {
            Some(b'D') => n.to_string(),
            Some(b'R') => roman(n),
            Some(b'r') => roman(n).to_lowercase(),
            Some(b'A') => alpha(n),
            Some(b'a') => alpha(n).to_lowercase(),
            _ => String::new(),
        };
        format!("{}{}", self.prefix, body)
    }
}

/// Etikett per sida ur sorterade intervall (startsida 0-baserad). Utan intervall: tomma strängar.
fn labels_from_ranges(ranges: &[(u32, LabelRange)], count: usize) -> Vec<String> {
    if ranges.is_empty() {
        return vec![String::new(); count];
    }
    (0..count as u32)
        .map(|p| match ranges.iter().rev().find(|r| r.0 <= p) {
            Some((start, r)) => r.label(p - start),
            None => (p + 1).to_string(), // före första intervallet: vanligt nummer
        })
        .collect()
}

fn page_labels(cx: &Cx, count: usize) -> Vec<String> {
    let mut ranges: Vec<(u32, LabelRange)> = vec![];
    if let Some(root) = cx.d.catalog().ok().and_then(|c| c.get(b"PageLabels").ok()).and_then(|o| cx.dict(o)) {
        let mut seen = HashSet::new();
        walk_numbers(cx, root, 0, &mut seen, &mut ranges);
    }
    ranges.sort_by_key(|r| r.0);
    labels_from_ranges(&ranges, count)
}

fn walk_numbers<'a>(cx: &Cx<'a>, node: &'a Dictionary, depth: usize, seen: &mut HashSet<ObjectId>, out: &mut Vec<(u32, LabelRange)>) {
    if depth > 16 || out.len() > 100_000 {
        return;
    }
    if let Ok(Object::Array(nums)) = node.get(b"Nums") {
        for p in nums.chunks(2) {
            let (Some(i), Some(d)) = (p[0].as_i64().ok(), p.get(1).and_then(|o| cx.dict(o))) else { continue };
            out.push((
                i.max(0) as u32,
                LabelRange {
                    style: d.get(b"S").ok().and_then(|x| x.as_name().ok()).and_then(|n| n.first().copied()),
                    prefix: d.get(b"P").ok().and_then(|x| cx.d.dereference(x).ok()).and_then(|x| text_of(x.1)).unwrap_or_default(),
                    start: d.get(b"St").ok().and_then(|x| x.as_i64().ok()).unwrap_or(1),
                },
            ));
        }
    }
    if let Ok(Object::Array(kids)) = node.get(b"Kids") {
        for k in kids {
            if let Object::Reference(id) = k {
                if !seen.insert(*id) {
                    continue;
                }
            }
            if let Some(kd) = cx.dict(k) {
                walk_numbers(cx, kd, depth + 1, seen, out);
            }
        }
    }
}

// ============================================================== dokumentinfo

/// `D:20261112143000+01'00'` → "2026-11-12 14:30". Ofullständiga datum ger kortare resultat;
/// allt som inte liknar ett PDF-datum returneras som det är.
fn format_date(s: &str) -> String {
    let t = s.trim();
    let digits = t.strip_prefix("D:").unwrap_or(t);
    let d: Vec<u8> = digits.bytes().take_while(u8::is_ascii_digit).collect();
    if d.len() < 4 || (d.len() > 4 && d.len() % 2 != 0) {
        return t.to_owned();
    }
    let part = |a: usize, b: usize| std::str::from_utf8(&d[a..b]).unwrap_or("");
    let mut out = part(0, 4).to_owned();
    if d.len() >= 6 {
        out += &format!("-{}", part(4, 6));
    }
    if d.len() >= 8 {
        out += &format!("-{}", part(6, 8));
    }
    if d.len() >= 12 {
        out += &format!(" {}:{}", part(8, 10), part(10, 12));
    } else if d.len() >= 10 {
        out += &format!(" {}:00", part(8, 10));
    }
    out
}

fn parse_version(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.trim().split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Version ur `%PDF-1.7` i filhuvudet.
fn header_version(bytes: &[u8]) -> Option<(u32, u32)> {
    let i = bytes.windows(5).take(1024).position(|w| w == b"%PDF-")? + 5;
    parse_version(std::str::from_utf8(bytes.get(i..i + 3)?).ok()?)
}

fn read_info(doc: &Document, bytes: &[u8]) -> DocInfo {
    let info_dict = doc.trailer.get(b"Info").ok().and_then(|o| doc.dereference(o).ok()).and_then(|x| x.1.as_dict().ok());
    let field = |key: &str| -> Option<String> {
        let v = doc.dereference(info_dict?.get(key.as_bytes()).ok()?).ok()?.1;
        text_of(v).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
    };
    let cat_version = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Version").ok())
        .and_then(|v| v.as_name().ok())
        .and_then(|n| parse_version(&String::from_utf8_lossy(n)));
    let best = header_version(bytes).into_iter().chain(cat_version).max();
    DocInfo {
        title: field("Title"),
        author: field("Author"),
        subject: field("Subject"),
        keywords: field("Keywords"),
        creator: field("Creator"),
        producer: field("Producer"),
        created: field("CreationDate").map(|s| format_date(&s)),
        modified: field("ModDate").map(|s| format_date(&s)),
        version: best.map(|(a, b)| format!("{a}.{b}")).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream, StringFormat};

    /// Bygger en PDF med `n` sidor (alla 200x300) via lopdf. `tweak` får ändra dokumentet och
    /// katalogen innan det sparas.
    fn build_doc(n: usize, tweak: impl FnOnce(&mut Document, &[ObjectId], &mut Dictionary)) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut kids = vec![];
        let mut ids = vec![];
        for _ in 0..n {
            let content = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
            let id = doc.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => content,
                "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
            });
            kids.push(Object::Reference(id));
            ids.push(id);
        }
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 }));
        let mut cat = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
        tweak(&mut doc, &ids, &mut cat);
        let cat_id = doc.add_object(cat);
        doc.trailer.set("Root", cat_id);
        let mut out = vec![];
        doc.save_to(&mut out).unwrap();
        out
    }

    fn s(t: &str) -> Object {
        Object::String(t.as_bytes().to_vec(), StringFormat::Literal)
    }

    #[test]
    fn roman_and_alpha() {
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(roman(0), "");
        assert_eq!(alpha(1), "A");
        assert_eq!(alpha(26), "Z");
        assert_eq!(alpha(27), "AA");
        assert_eq!(alpha(28), "BB");
        assert_eq!(alpha(53), "AAA");
    }

    #[test]
    fn label_ranges() {
        let r = |style: Option<u8>, prefix: &str, start: i64| LabelRange { style, prefix: prefix.into(), start };
        let ranges = vec![
            (0, r(Some(b'r'), "", 1)),
            (3, r(Some(b'D'), "", 1)),
            (6, r(Some(b'D'), "A-", 7)),
            (8, r(None, "Bilaga", 1)),
            (9, r(Some(b'A'), "", 1)),
        ];
        let l = labels_from_ranges(&ranges, 12);
        assert_eq!(l, ["i", "ii", "iii", "1", "2", "3", "A-7", "A-8", "Bilaga", "A", "B", "C"]);
        assert_eq!(labels_from_ranges(&[], 3), ["", "", ""]);
        // Före första intervallet: vanligt nummer.
        assert_eq!(labels_from_ranges(&[(2, r(Some(b'D'), "", 1))], 3), ["1", "2", "1"]);
    }

    #[test]
    fn labels_from_number_tree() {
        let bytes = build_doc(5, |doc, _, cat| {
            let leaf = doc.add_object(dictionary! { "Nums" => vec![
                2.into(), dictionary! { "S" => "D", "St" => 10 }.into(),
                4.into(), dictionary! { "S" => "a", "P" => s("x") }.into(),
            ]});
            cat.set("PageLabels", dictionary! {
                "Kids" => vec![
                    dictionary! { "Nums" => vec![0.into(), dictionary! { "S" => "R" }.into()] }.into(),
                    leaf.into(),
                ],
            });
        });
        assert_eq!(read_meta(&bytes, None).labels, ["I", "II", "10", "11", "xa"]);
    }

    #[test]
    fn no_labels_gives_empty_strings() {
        let m = read_meta(&build_doc(3, |_, _, _| {}), None);
        assert_eq!(m.labels, ["", "", ""]);
        assert_eq!(m.links.len(), 3);
        assert!(m.outline.is_empty());
        assert_eq!(m.info.version, "1.5");
    }

    #[test]
    fn rotation_mapping() {
        // Sida 200 x 300 med origo i (10, 20): beskärningsruta (10,20)-(210,320).
        let g = |rot| Geo { x0: 10.0, y0: 20.0, x1: 210.0, y1: 320.0, rot };
        let r = (30.0, 50.0, 70.0, 80.0); // 40 x 30
        let near = |a: egui::Rect, b: [f32; 4]| {
            assert!((a.min.x - b[0]).abs() < 1e-3 && (a.min.y - b[1]).abs() < 1e-3 && (a.max.x - b[2]).abs() < 1e-3 && (a.max.y - b[3]).abs() < 1e-3, "{a:?} != {b:?}");
        };
        near(g(0).rect(r), [20.0, 240.0, 60.0, 270.0]);
        // 90 medurs: sidan blir 300 x 200; u (x) blir y, v (nedåt) blir x från höger.
        near(g(90).rect(r), [300.0 - 270.0, 20.0, 300.0 - 240.0, 60.0]);
        near(g(180).rect(r), [200.0 - 60.0, 300.0 - 270.0, 200.0 - 20.0, 300.0 - 240.0]);
        near(g(270).rect(r), [240.0, 200.0 - 60.0, 270.0, 200.0 - 20.0]);
        assert_eq!(g(90).size(), (300.0, 200.0));
        // Målets y: 0 grader ur top, 90 ur left.
        assert_eq!(g(0).dest_y(Some(0.0), Some(300.0)), Some(20.0));
        assert_eq!(g(0).dest_y(None, None), None);
        assert_eq!(g(90).dest_y(Some(50.0), None), Some(40.0));
        assert_eq!(g(270).dest_y(Some(50.0), None), Some(160.0));
        assert_eq!(g(180).dest_y(None, Some(320.0)), Some(300.0));
    }

    #[test]
    fn rotated_page_links() {
        let bytes = build_doc(2, |doc, ids, _| {
            doc.get_dictionary_mut(ids[0]).unwrap().set("Rotate", 90);
            let link = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Link",
                "Rect" => vec![30.into(), 50.into(), 70.into(), 80.into()],
                "A" => dictionary! { "S" => "URI", "URI" => s("https://example.org/") },
            });
            doc.get_dictionary_mut(ids[0]).unwrap().set("Annots", vec![Object::Reference(link)]);
        });
        let m = read_meta(&bytes, None);
        let l = &m.links[0][0];
        assert_eq!(l.target, LinkTarget::Uri("https://example.org/".into()));
        // 90 grader: sidan är 300 x 200; (30..70, 50..80) → x 50..80 (= 300 - v), y 30..70.
        assert_eq!((l.rect.min.x, l.rect.min.y, l.rect.max.x, l.rect.max.y), (50.0, 30.0, 80.0, 70.0));
        assert!(m.links[1].is_empty());
    }

    #[test]
    fn named_dests_and_goto() {
        let bytes = build_doc(3, |doc, ids, cat| {
            let (p1, p2) = (ids[1], ids[2]);
            // Gammalt /Dests (namn) och namnträd med Kids.
            cat.set("Dests", dictionary! { "old" => vec![p1.into(), "XYZ".into(), 10.into(), 250.into(), Object::Null] });
            let leaf_a = doc.add_object(dictionary! { "Names" => vec![
                s("alpha"), dictionary! { "D" => vec![p2.into(), "FitH".into(), 100.into()] }.into(),
            ]});
            let leaf_b = doc.add_object(dictionary! { "Names" => vec![
                s("beta"), vec![p1.into(), "Fit".into()].into(),
            ]});
            cat.set("Names", dictionary! { "Dests" => dictionary! { "Kids" => vec![leaf_a.into(), leaf_b.into()] } });
            let mk = |doc: &mut Document, y: i64, action: Object| {
                doc.add_object(dictionary! {
                    "Type" => "Annot", "Subtype" => "Link",
                    "Rect" => vec![10.into(), y.into(), 60.into(), (y + 10).into()],
                    "A" => action,
                })
            };
            let a1 = mk(doc, 10, dictionary! { "S" => "GoTo", "D" => s("alpha") }.into());
            let a2 = mk(doc, 30, dictionary! { "S" => "GoTo", "D" => Object::Name(b"old".to_vec()) }.into());
            let a3 = mk(doc, 50, dictionary! { "S" => "GoTo", "D" => s("beta") }.into());
            let a4 = mk(doc, 70, dictionary! { "S" => "GoTo", "D" => s("saknas") }.into()); // ogiltigt: hoppas över
            let a5 = mk(doc, 90, dictionary! { "S" => "GoToR", "F" => s("x.pdf") }.into()); // hoppas över
            // Direkt /Dest på länken.
            let a6 = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![10.into(), 110.into(), 60.into(), 120.into()],
                "Dest" => vec![ids[0].into(), "XYZ".into(), 0.into(), 300.into(), 0.into()],
            });
            doc.get_dictionary_mut(ids[0]).unwrap().set("Annots", [a1, a2, a3, a4, a5, a6].map(Object::Reference).to_vec());
        });
        let m = read_meta(&bytes, None);
        let t: Vec<_> = m.links[0].iter().map(|l| l.target.clone()).collect();
        assert_eq!(
            t,
            [
                LinkTarget::Page(Dest { page: 2, y: Some(200.0) }),
                LinkTarget::Page(Dest { page: 1, y: Some(50.0) }),
                LinkTarget::Page(Dest { page: 1, y: None }),
                LinkTarget::Page(Dest { page: 0, y: Some(0.0) }),
            ]
        );
        for l in &m.links[0] {
            assert!(l.rect.min.x >= 0.0 && l.rect.min.y >= 0.0 && l.rect.max.x <= 200.0 && l.rect.max.y <= 300.0, "{:?}", l.rect);
        }
        // Första länken: Rect (10,10)-(60,20) → y nedåt 280..290.
        assert_eq!((m.links[0][0].rect.min.y, m.links[0][0].rect.max.y), (280.0, 290.0));
    }

    #[test]
    fn quadpoints_give_bounding_rect() {
        let bytes = build_doc(1, |doc, ids, _| {
            let q: Vec<Object> = [20, 100, 80, 100, 20, 90, 80, 90].iter().map(|&v| Object::Integer(v)).collect();
            let a = doc.add_object(dictionary! {
                "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![0.into(), 0.into(), 200.into(), 300.into()],
                "QuadPoints" => q, "A" => dictionary! { "S" => "URI", "URI" => s("http://a.b") },
            });
            doc.get_dictionary_mut(ids[0]).unwrap().set("Annots", vec![Object::Reference(a)]);
        });
        let r = read_meta(&bytes, None).links[0][0].rect;
        assert_eq!((r.min.x, r.min.y, r.max.x, r.max.y), (20.0, 200.0, 80.0, 210.0));
    }

    #[test]
    fn outline_tree() {
        let bytes = build_doc(3, |doc, ids, cat| {
            let root = doc.new_object_id();
            let (a, b, a1) = (doc.new_object_id(), doc.new_object_id(), doc.new_object_id());
            let utf16: Vec<u8> = [0xFE, 0xFF].into_iter().chain("Kapitel åäö".encode_utf16().flat_map(|u| u.to_be_bytes())).collect();
            doc.objects.insert(a, dictionary! {
                "Title" => Object::String(utf16, StringFormat::Hexadecimal), "Parent" => root, "Next" => b, "First" => a1, "Count" => 1,
                "Dest" => vec![ids[0].into(), "XYZ".into(), 0.into(), 280.into(), 0.into()],
            }.into());
            doc.objects.insert(a1, dictionary! {
                "Title" => s("Avsnitt"), "Parent" => a,
                "A" => dictionary! { "S" => "GoTo", "D" => vec![ids[1].into(), "FitH".into(), 150.into()] },
            }.into());
            doc.objects.insert(b, dictionary! { "Title" => s("Slut"), "Parent" => root, "Prev" => a,
                "Dest" => vec![ids[2].into(), "Fit".into()] }.into());
            doc.objects.insert(root, dictionary! { "Type" => "Outlines", "First" => a, "Last" => b, "Count" => 3 }.into());
            cat.set("Outlines", root);
        });
        let o = read_meta(&bytes, None).outline;
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].title, "Kapitel åäö");
        assert!(o[0].open);
        assert_eq!(o[0].dest, Some(Dest { page: 0, y: Some(20.0) }));
        assert_eq!(o[0].children.len(), 1);
        assert_eq!(o[0].children[0].title, "Avsnitt");
        assert_eq!(o[0].children[0].dest, Some(Dest { page: 1, y: Some(150.0) }));
        assert!(!o[0].children[0].open);
        assert_eq!(o[1].title, "Slut");
        assert_eq!(o[1].dest, Some(Dest { page: 2, y: None }));
    }

    #[test]
    fn outline_cycle_terminates() {
        let bytes = build_doc(1, |doc, ids, cat| {
            let root = doc.new_object_id();
            let a = doc.new_object_id();
            doc.objects.insert(a, dictionary! { "Title" => s("Loop"), "Next" => a, "First" => a,
                "Dest" => vec![ids[0].into(), "Fit".into()] }.into());
            doc.objects.insert(root, dictionary! { "First" => a }.into());
            cat.set("Outlines", root);
        });
        let o = read_meta(&bytes, None).outline;
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].title, "Loop");
    }

    #[test]
    fn info_decoding() {
        let bytes = build_doc(1, |doc, _, _| {
            let utf16: Vec<u8> = [0xFE, 0xFF].into_iter().chain("Rapport – åäö".encode_utf16().flat_map(|u| u.to_be_bytes())).collect();
            let info = doc.add_object(dictionary! {
                "Title" => Object::String(utf16, StringFormat::Hexadecimal),
                "Author" => Object::String(vec![b'M', 0xE5, b'r', 0x84, b't', 0xA0], StringFormat::Literal), // PDFDoc: å, em-streck, euro
                "Subject" => s("  "),
                "Producer" => Object::String(vec![0xEF, 0xBB, 0xBF, 0xC3, 0xA5], StringFormat::Literal),
                "CreationDate" => s("D:20261112143000+01'00'"),
                "ModDate" => s("D:202611"),
            });
            doc.trailer.set("Info", info);
        });
        let i = read_meta(&bytes, None).info;
        assert_eq!(i.title.as_deref(), Some("Rapport – åäö"));
        assert_eq!(i.author.as_deref(), Some("Mår\u{2014}t\u{20AC}"));
        assert_eq!(i.subject, None);
        assert_eq!(i.producer.as_deref(), Some("å"));
        assert_eq!(i.created.as_deref(), Some("2026-11-12 14:30"));
        assert_eq!(i.modified.as_deref(), Some("2026-11"));
        assert_eq!(i.version, "1.5");
    }

    #[test]
    fn dates_and_versions() {
        assert_eq!(format_date("D:20261112143000Z"), "2026-11-12 14:30");
        assert_eq!(format_date("D:20261112"), "2026-11-12");
        assert_eq!(format_date("D:2026"), "2026");
        assert_eq!(format_date("D:2026111214"), "2026-11-12 14:00");
        assert_eq!(format_date("igår"), "igår");
        assert_eq!(header_version(b"%PDF-1.7\n..."), Some((1, 7)));
        assert_eq!(header_version(b"junk"), None);
        // Katalogens /Version vinner när den är högre än huvudet.
        let bytes = build_doc(1, |_, _, cat| cat.set("Version", Object::Name(b"2.0".to_vec())));
        assert_eq!(read_meta(&bytes, None).info.version, "2.0");
    }

    #[test]
    fn link_cap_per_page() {
        let bytes = build_doc(1, |doc, ids, _| {
            let ann: Vec<Object> = (0..MAX_LINKS_PER_PAGE + 50)
                .map(|_| {
                    Object::Reference(doc.add_object(dictionary! {
                        "Subtype" => "Link", "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
                        "A" => dictionary! { "S" => "URI", "URI" => s("http://x.y") },
                    }))
                })
                .collect();
            doc.get_dictionary_mut(ids[0]).unwrap().set("Annots", ann);
        });
        assert_eq!(read_meta(&bytes, None).links[0].len(), MAX_LINKS_PER_PAGE);
    }

    #[test]
    fn garbage_returns_default() {
        for input in [&b""[..], b"hej", b"%PDF-1.4\n", b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF", &[0xFF; 4096]] {
            let m = read_meta(input, None);
            assert!(m.outline.is_empty() && m.links.is_empty() && m.labels.is_empty());
        }
        // Avkapad riktig PDF får inte heller ge panik.
        let good = build_doc(2, |_, _, _| {});
        for cut in [10, good.len() / 2, good.len() - 5] {
            let _ = read_meta(&good[..cut], None);
        }
    }

    #[test]
    fn kladd_own_pdf_has_outline_and_links() {
        let md = "# Rubrik ett\n\nText med [en länk](https://example.org) här.\n\n## Under\n\nMer text.\n\n# Rubrik två\n\nSlut.\n";
        let pdf = crate::pdf_write::write(md, "Test", &|_| None).unwrap().bytes;
        let m = read_meta(&pdf, None);
        assert_eq!(m.outline.len(), 2, "{:?}", m.outline);
        assert_eq!(m.outline[0].title, "Rubrik ett");
        assert_eq!(m.outline[0].children.len(), 1);
        assert_eq!(m.outline[0].children[0].title, "Under");
        assert_eq!(m.outline[1].title, "Rubrik två");
        let d = m.outline[1].dest.clone().expect("mål");
        assert_eq!(d.page, 0);
        assert!(d.y.unwrap() > 0.0 && d.y.unwrap() < 842.0);
        let links: Vec<&Link> = m.links.iter().flatten().collect();
        assert!(links.iter().any(|l| l.target == LinkTarget::Uri("https://example.org".into())), "{links:?}");
        for l in links {
            assert!(l.rect.min.x >= 0.0 && l.rect.max.x <= 600.0 && l.rect.max.y <= 900.0 && l.rect.height() > 0.0);
        }
        assert!(m.info.version.starts_with("1."));
        assert_eq!(m.info.creator.as_deref(), Some("Kladd"));
        assert_eq!(m.labels.len(), m.links.len());
    }
}
