//! Formulärfält (AcroForm): läsning med `lopdf`, ifyllnad i visaren och sparning.
//!
//! * `read_fields` går igenom `/AcroForm /Fields` (med ärvda `/FT`, `/Ff`, `/V`, `/DA`, `/Opt`,
//!   `/MaxLen`, `/Q` genom `/Kids`) och ger varje fält med sina widgetar. Rektanglarna är
//!   *visningspunkter* (origo uppe till vänster efter sidans `/Rotate`), som länkarna i `meta`.
//! * `FormState` hör till visaren: aktuella värden, markering av fält, redigeringsrutor i egui
//!   ovanpå sidan och Tab-ordning. `page_ui` är den enda kroken visaren behöver.
//! * `apply` skriver värdena till PDF:en: `/V`, `/AS` och nya utseendeströmmar (`/AP /N`) för
//!   text- och valfält med typsnitt och storlek ur `/DA`; `/NeedAppearances` sätts också.
//!
//! Krypterade PDF:er stöds inte (de visas utan fält): lopdf kan inte skriva dem krypterade igen.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use egui::{
    Align, Color32, ComboBox, FontId, Frame, Key, Layout, Margin, Modifiers, Panel, Popup, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, TextEdit, Ui, UiBuilder, pos2, vec2,
};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use super::meta::{Geo, decode_text, num, rect_of};
use super::view::PageXf;
use crate::i18n::t;
use crate::tf;
use crate::theme::{self, Palette};

const MAX_FIELDS: usize = 5000;
const MAX_DECODED: usize = 256 * 1024 * 1024;

// ============================================================== modell

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// `max_len` 0 = obegränsad. `comb`: ett tecken per ruta (kräver `max_len`).
    Text { multiline: bool, password: bool, comb: bool, max_len: usize },
    Check,
    Radio,
    Combo { editable: bool },
    List { multi: bool },
}

/// Fältets värde. Text och kombinationsrutor: `Text` (exportvärde). Kryss- och alternativknappar:
/// `State` (namnet på det valda läget, "Off" när inget är valt). Listrutor: `Multi` (exportvärden).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    State(String),
    Multi(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct Widget {
    pub id: ObjectId,
    /// 0-baserad sida.
    pub page: usize,
    /// Visningspunkter.
    pub rect: Rect,
    /// Namnet på "på"-läget (`/AP /N`) för kryss- och alternativknappar, annars tomt.
    pub on: String,
}

#[derive(Clone, Debug)]
pub struct Field {
    /// Objektet som bär `/V` (fältet, inte alltid widgeten).
    pub id: ObjectId,
    pub name: String,
    pub kind: Kind,
    pub read_only: bool,
    pub required: bool,
    pub widgets: Vec<Widget>,
    /// (exportvärde, visningstext)
    pub options: Vec<(String, String)>,
    pub value: Value,
    /// Typsnittsstorlek ur `/DA`; 0 = automatisk.
    pub font_size: f32,
    /// 0 vänster, 1 mitten, 2 höger.
    pub quad: u8,
}

impl Field {
    fn display_of(&self, export: &str) -> String {
        self.options.iter().find(|(e, _)| e == export).map(|(_, d)| d.clone()).unwrap_or_else(|| export.to_owned())
    }
}

// ============================================================== läsning

fn load_doc(bytes: &[u8], password: Option<&str>) -> Option<Document> {
    if !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return None;
    }
    let mut opts = lopdf::LoadOptions::default();
    opts.max_decompressed_size = Some(MAX_DECODED);
    if let Some(p) = password {
        opts.password = Some(p.to_owned());
    }
    Document::load_mem_with_options(bytes, opts).ok()
}

/// Läser alla ifyllbara fält. Panikar aldrig; tomt vid fel, krypterad PDF eller utan formulär.
pub fn read_fields(bytes: &[u8], password: Option<&str>) -> Vec<Field> {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(doc) = load_doc(bytes, password) else { return Vec::new() };
        if doc.trailer.get(b"Encrypt").is_ok() {
            return Vec::new();
        }
        read_doc(&doc)
    }))
    .unwrap_or_default()
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    doc.dereference(o).ok()?.1.as_dict().ok()
}

fn deref<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    doc.dereference(d.get(key).ok()?).ok().map(|x| x.1)
}

fn str_of(o: &Object) -> Option<String> {
    match o {
        Object::String(b, _) => Some(decode_text(b)),
        Object::Name(n) => Some(decode_text(n)),
        _ => None,
    }
}

/// Typsnittsnamn, storlek och färgoperatorer ur en `/DA`-sträng ("/Helv 12 Tf 0 g").
fn parse_da(da: &str) -> (String, f32, String) {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut font = "Helv".to_owned();
    let mut size = 0.0;
    let mut rest: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i] == "Tf" && i >= 2 {
            rest.truncate(rest.len().saturating_sub(2));
            font = toks[i - 2].trim_start_matches('/').to_owned();
            size = toks[i - 1].parse().unwrap_or(0.0);
        } else {
            rest.push(toks[i]);
        }
        i += 1;
    }
    let color = if rest.is_empty() { "0 g".to_owned() } else { rest.join(" ") };
    (font, size, color)
}

#[derive(Clone, Default)]
struct Inh {
    ft: Option<Vec<u8>>,
    ff: Option<u32>,
    v: Option<Object>,
    da: Option<String>,
    opt: Option<Object>,
    max_len: Option<usize>,
    q: Option<i64>,
}

struct Reader<'a> {
    doc: &'a Document,
    /// Widgetannoteringar på sidorna: sida och rektangel i visningspunkter.
    widgets: HashMap<ObjectId, (usize, Rect)>,
    seen: HashSet<ObjectId>,
    out: Vec<Field>,
}

fn read_doc(doc: &Document) -> Vec<Field> {
    let Ok(cat) = doc.catalog() else { return Vec::new() };
    let Some(acro) = deref(doc, cat, b"AcroForm").and_then(|o| o.as_dict().ok()) else { return Vec::new() };
    let Some(roots) = deref(doc, acro, b"Fields").and_then(|o| o.as_array().ok()) else { return Vec::new() };
    let mut widgets = HashMap::new();
    for (pi, (_, pid)) in doc.get_pages().iter().enumerate() {
        let geo = Geo::of(doc, *pid);
        let Ok(page) = doc.get_dictionary(*pid) else { continue };
        let Some(annots) = deref(doc, page, b"Annots").and_then(|o| o.as_array().ok()) else { continue };
        for a in annots {
            let Ok(id) = a.as_reference() else { continue };
            let Ok(d) = doc.get_dictionary(id) else { continue };
            if d.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) != Some(b"Widget") {
                continue;
            }
            // Dold (2) eller ej synlig (32): inget att fylla i.
            let flags = deref(doc, d, b"F").and_then(|o| o.as_i64().ok()).unwrap_or(0);
            if flags & (2 | 32) != 0 {
                continue;
            }
            let Some(r) = deref(doc, d, b"Rect").and_then(|o| rect_of(doc, o)) else { continue };
            widgets.insert(id, (pi, geo.rect(r)));
        }
    }
    let mut rd = Reader { doc, widgets, seen: HashSet::new(), out: Vec::new() };
    let base = Inh {
        da: deref(doc, acro, b"DA").and_then(str_of),
        ff: None,
        q: deref(doc, acro, b"Q").and_then(|o| o.as_i64().ok()),
        ..Inh::default()
    };
    for r in roots {
        if let Ok(id) = r.as_reference() {
            rd.walk(id, &base, "", 0);
        }
    }
    rd.out
}

impl Reader<'_> {
    fn walk(&mut self, id: ObjectId, inh: &Inh, prefix: &str, depth: usize) {
        if depth > 32 || self.out.len() >= MAX_FIELDS || !self.seen.insert(id) {
            return;
        }
        let doc = self.doc;
        let Ok(d) = doc.get_dictionary(id) else { return };
        let mut cur = inh.clone();
        if let Some(o) = deref(doc, d, b"FT").and_then(|o| o.as_name().ok()) {
            cur.ft = Some(o.to_vec());
        }
        if let Some(v) = deref(doc, d, b"Ff").and_then(|o| o.as_i64().ok()) {
            cur.ff = Some(v as u32);
        }
        if let Some(v) = deref(doc, d, b"V") {
            cur.v = Some(v.clone());
        }
        if let Some(v) = deref(doc, d, b"DA").and_then(str_of) {
            cur.da = Some(v);
        }
        if let Some(v) = deref(doc, d, b"Opt") {
            cur.opt = Some(v.clone());
        }
        if let Some(v) = deref(doc, d, b"MaxLen").and_then(|o| o.as_i64().ok()) {
            cur.max_len = Some(v.max(0) as usize);
        }
        if let Some(v) = deref(doc, d, b"Q").and_then(|o| o.as_i64().ok()) {
            cur.q = Some(v);
        }
        let name = deref(doc, d, b"T").and_then(str_of).unwrap_or_default();
        let full = match (prefix.is_empty(), name.is_empty()) {
            (_, true) => prefix.to_owned(),
            (true, false) => name,
            (false, false) => format!("{prefix}.{name}"),
        };
        let kids: Vec<ObjectId> = deref(doc, d, b"Kids")
            .and_then(|o| o.as_array().ok())
            .map(|a| a.iter().filter_map(|k| k.as_reference().ok()).collect())
            .unwrap_or_default();
        let mut wids = Vec::new();
        for k in &kids {
            if doc.get_dictionary(*k).is_ok_and(|kd| kd.has(b"T")) {
                self.walk(*k, &cur, &full, depth + 1);
            } else {
                wids.push(*k);
            }
        }
        if kids.is_empty() {
            wids.push(id);
        }
        wids.retain(|w| self.widgets.contains_key(w));
        if wids.is_empty() {
            return;
        }
        if let Some(f) = self.build(id, &cur, full, &wids) {
            self.out.push(f);
        }
    }

    fn build(&self, id: ObjectId, cur: &Inh, name: String, wids: &[ObjectId]) -> Option<Field> {
        let doc = self.doc;
        let ff = cur.ff.unwrap_or(0);
        let max_len = cur.max_len.unwrap_or(0);
        let kind = match cur.ft.as_deref()? {
            b"Tx" => Kind::Text {
                multiline: ff & (1 << 12) != 0,
                password: ff & (1 << 13) != 0,
                comb: ff & (1 << 24) != 0 && max_len > 0,
                max_len,
            },
            b"Btn" if ff & (1 << 16) != 0 => return None,
            b"Btn" if ff & (1 << 15) != 0 => Kind::Radio,
            b"Btn" => Kind::Check,
            b"Ch" if ff & (1 << 17) != 0 => Kind::Combo { editable: ff & (1 << 18) != 0 },
            b"Ch" => Kind::List { multi: ff & (1 << 21) != 0 },
            _ => return None,
        };
        let mut widgets: Vec<Widget> = wids
            .iter()
            .map(|w| {
                let (page, rect) = self.widgets[w];
                let on = if matches!(kind, Kind::Check | Kind::Radio) { on_state(doc, *w) } else { String::new() };
                Widget { id: *w, page, rect, on }
            })
            .collect();
        widgets.sort_by_key(|w| w.page);
        let options = cur.opt.as_ref().and_then(|o| o.as_array().ok()).map(|a| options_of(doc, a)).unwrap_or_default();
        let value = match &kind {
            Kind::Text { .. } | Kind::Combo { .. } => Value::Text(match &cur.v {
                Some(Object::Array(a)) => a.first().and_then(|o| doc.dereference(o).ok()).and_then(|o| str_of(o.1)).unwrap_or_default(),
                Some(o) => str_of(o).unwrap_or_default(),
                None => String::new(),
            }),
            Kind::List { .. } => Value::Multi(match &cur.v {
                Some(Object::Array(a)) => a.iter().filter_map(|o| doc.dereference(o).ok().and_then(|o| str_of(o.1))).collect(),
                Some(o) => str_of(o).into_iter().collect(),
                None => Vec::new(),
            }),
            Kind::Check | Kind::Radio => {
                let from_v = cur.v.as_ref().and_then(|o| o.as_name().ok()).map(|n| decode_text(n));
                let from_as = wids.iter().find_map(|w| {
                    let wd = doc.get_dictionary(*w).ok()?;
                    let s = deref(doc, wd, b"AS")?.as_name().ok()?;
                    (s != b"Off").then(|| decode_text(s))
                });
                Value::State(from_v.or(from_as).unwrap_or_else(|| "Off".to_owned()))
            }
        };
        let (_, font_size, _) = parse_da(cur.da.as_deref().unwrap_or(""));
        Some(Field {
            id,
            name,
            kind,
            read_only: ff & 1 != 0,
            required: ff & 2 != 0,
            widgets,
            options,
            value,
            font_size,
            quad: cur.q.unwrap_or(0).clamp(0, 2) as u8,
        })
    }
}

fn options_of(doc: &Document, a: &[Object]) -> Vec<(String, String)> {
    a.iter()
        .filter_map(|o| {
            let o = doc.dereference(o).ok()?.1;
            match o {
                Object::Array(p) => {
                    let e = str_of(doc.dereference(p.first()?).ok()?.1)?;
                    let d = p.get(1).and_then(|x| doc.dereference(x).ok()).and_then(|x| str_of(x.1)).unwrap_or_else(|| e.clone());
                    Some((e, d))
                }
                o => str_of(o).map(|s| (s.clone(), s)),
            }
        })
        .collect()
}

/// Namnet på widgetens "på"-läge: nyckeln i `/AP /N` som inte är `Off`. "Yes" om utseende saknas.
fn on_state(doc: &Document, w: ObjectId) -> String {
    let Ok(d) = doc.get_dictionary(w) else { return "Yes".into() };
    let n = deref(doc, d, b"AP").and_then(|o| o.as_dict().ok()).and_then(|ap| deref(doc, ap, b"N")).and_then(|o| o.as_dict().ok());
    n.and_then(|n| n.iter().map(|(k, _)| k.as_slice()).find(|k| *k != b"Off").map(decode_text)).unwrap_or_else(|| "Yes".into())
}

// ============================================================== skrivning

/// Skriver värdena (fält-id → värde) till PDF:en och ger de nya bytena.
pub fn apply(bytes: &[u8], password: Option<&str>, values: &[(ObjectId, Value)]) -> Result<Vec<u8>, String> {
    catch_unwind(AssertUnwindSafe(|| apply_inner(bytes, password, values)))
        .unwrap_or_else(|_| Err(t("Formulärvärdena kunde inte skrivas").to_owned()))
}

fn apply_inner(bytes: &[u8], password: Option<&str>, values: &[(ObjectId, Value)]) -> Result<Vec<u8>, String> {
    let mut doc = load_doc(bytes, password).ok_or(t("PDF:en kunde inte läsas"))?;
    if doc.trailer.get(b"Encrypt").is_ok() {
        return Err(t("Krypterade PDF:er kan inte fyllas i ännu").to_owned());
    }
    apply_doc(&mut doc, values)?;
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| tf!("PDF:en kunde inte sparas: {e}", e = e))?;
    Ok(out)
}

/// Skriver värdena till det inlästa dokumentet (fältens `/V`, `/AS`, `/AP` och `/NeedAppearances`).
pub fn apply_doc(doc: &mut Document, values: &[(ObjectId, Value)]) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(|| apply_doc_inner(doc, values))).unwrap_or_else(|_| Err(t("Formulärvärdena kunde inte skrivas").to_owned()))
}

fn apply_doc_inner(doc: &mut Document, values: &[(ObjectId, Value)]) -> Result<(), String> {
    let fields = read_doc(doc);
    let mut helv = None;
    for (id, val) in values {
        let Some(f) = fields.iter().find(|f| f.id == *id) else { continue };
        write_field(doc, f, val, &mut helv)?;
    }
    if let Some(acro) = acroform_mut(doc) {
        acro.set("NeedAppearances", Object::Boolean(true));
    }
    Ok(())
}

fn acroform_mut(doc: &mut Document) -> Option<&mut Dictionary> {
    let acro = doc.catalog().ok()?.get(b"AcroForm").ok()?.clone();
    match acro {
        Object::Reference(id) => doc.get_dictionary_mut(id).ok(),
        _ => doc.catalog_mut().ok()?.get_mut(b"AcroForm").ok()?.as_dict_mut().ok(),
    }
}

/// PDF-textsträng: PDFDocEncoding (= Latin-1 här) när det går, annars UTF-16BE med BOM.
fn pdf_text(s: &str) -> Object {
    if s.chars().all(|c| (c as u32) < 0x80 || ((c as u32) >= 0xA0 && (c as u32) < 0x100)) {
        Object::String(s.chars().map(|c| c as u8).collect(), StringFormat::Literal)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend(u.to_be_bytes());
        }
        Object::String(b, StringFormat::Hexadecimal)
    }
}

fn write_field(doc: &mut Document, f: &Field, val: &Value, helv: &mut Option<ObjectId>) -> Result<(), String> {
    match (&f.kind, val) {
        (Kind::Text { .. } | Kind::Combo { .. }, Value::Text(t)) => {
            set_v(doc, f.id, pdf_text(t))?;
            for w in &f.widgets {
                text_appearance(doc, f, w, val, helv);
            }
        }
        (Kind::List { .. }, Value::Multi(v)) => {
            let o = match v.as_slice() {
                [one] => pdf_text(one),
                many => Object::Array(many.iter().map(|s| pdf_text(s)).collect()),
            };
            set_v(doc, f.id, o)?;
            if let Ok(d) = doc.get_dictionary_mut(f.id) {
                d.remove(b"I");
            }
            for w in &f.widgets {
                text_appearance(doc, f, w, val, helv);
            }
        }
        (Kind::Check | Kind::Radio, Value::State(s)) => {
            set_v(doc, f.id, Object::Name(s.clone().into_bytes()))?;
            for w in &f.widgets {
                let on = if w.on.is_empty() { "Yes" } else { w.on.as_str() };
                ensure_check_ap(doc, w, on, f.kind == Kind::Radio);
                let state = if s == on { on } else { "Off" };
                if let Ok(d) = doc.get_dictionary_mut(w.id) {
                    d.set("AS", Object::Name(state.as_bytes().to_vec()));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn set_v(doc: &mut Document, id: ObjectId, v: Object) -> Result<(), String> {
    doc.get_dictionary_mut(id).map_err(|e| e.to_string())?.set("V", v);
    Ok(())
}

/// Widgetens storlek i användarrymd och utseendeinformation ur `/MK`.
struct WGeo {
    w: f32,
    h: f32,
    /// `/MK /R`: 0, 90, 180, 270.
    rot: u16,
    bg: Option<String>,
    border: Option<String>,
    bw: f32,
}

fn color_ops(doc: &Document, o: Option<&Object>, stroke: bool) -> Option<String> {
    let a = doc.dereference(o?).ok()?.1.as_array().ok()?;
    let v: Vec<f32> = a.iter().filter_map(|x| num(doc, x)).collect();
    let (ops, n) = match (v.len(), stroke) {
        (1, false) => ("g", 1),
        (1, true) => ("G", 1),
        (3, false) => ("rg", 3),
        (3, true) => ("RG", 3),
        (4, false) => ("k", 4),
        (4, true) => ("K", 4),
        _ => return None,
    };
    Some(format!("{} {ops}", v[..n].iter().map(|x| fmt(*x)).collect::<Vec<_>>().join(" ")))
}

fn widget_geo(doc: &Document, w: ObjectId) -> Option<WGeo> {
    let d = doc.get_dictionary(w).ok()?;
    let (x0, y0, x1, y1) = rect_of(doc, deref(doc, d, b"Rect")?)?;
    let mk = deref(doc, d, b"MK").and_then(|o| o.as_dict().ok());
    let rot = mk.and_then(|m| deref(doc, m, b"R")).and_then(|o| o.as_i64().ok()).unwrap_or(0).rem_euclid(360) as u16 / 90 * 90;
    let bs = deref(doc, d, b"BS").and_then(|o| o.as_dict().ok()).and_then(|b| deref(doc, b, b"W")).and_then(|o| num(doc, o));
    Some(WGeo {
        w: x1 - x0,
        h: y1 - y0,
        rot,
        bg: mk.and_then(|m| color_ops(doc, m.get(b"BG").ok(), false)),
        border: mk.and_then(|m| color_ops(doc, m.get(b"BC").ok(), true)),
        bw: bs.unwrap_or(1.0).max(0.0),
    })
}

fn fmt(x: f32) -> String {
    let s = format!("{:.3}", x);
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Bakgrund och ram som i `/MK`.
fn frame_ops(g: &WGeo, w: f32, h: f32) -> String {
    let mut s = String::new();
    if let Some(bg) = &g.bg {
        s += &format!("q {bg} 0 0 {} {} re f Q\n", fmt(w), fmt(h));
    }
    if let (Some(bc), true) = (&g.border, g.bw > 0.0) {
        let b = g.bw;
        s += &format!("q {bc} {} w {} {} {} {} re S Q\n", fmt(b), fmt(b / 2.0), fmt(b / 2.0), fmt(w - b), fmt(h - b));
    }
    s
}

/// Streamen för ett utseende med BBox/Matrix som tar hänsyn till `/MK /R`.
fn make_stream(g: &WGeo, content: String, fonts: Option<(&str, ObjectId)>) -> Stream {
    let (w, h) = (g.w, g.h);
    let (bbox, matrix): ([f32; 4], Option<[f32; 6]>) = match g.rot {
        90 => ([0.0, 0.0, h, w], Some([0.0, 1.0, -1.0, 0.0, w, 0.0])),
        180 => ([0.0, 0.0, w, h], Some([-1.0, 0.0, 0.0, -1.0, w, h])),
        270 => ([0.0, 0.0, h, w], Some([0.0, -1.0, 1.0, 0.0, 0.0, h])),
        _ => ([0.0, 0.0, w, h], None),
    };
    let mut d = Dictionary::new();
    d.set("Type", "XObject");
    d.set("Subtype", "Form");
    d.set("BBox", Object::Array(bbox.iter().map(|x| Object::Real(*x)).collect()));
    if let Some(m) = matrix {
        d.set("Matrix", Object::Array(m.iter().map(|x| Object::Real(*x)).collect()));
    }
    if let Some((name, id)) = fonts {
        let mut fd = Dictionary::new();
        fd.set(name.as_bytes().to_vec(), Object::Reference(id));
        let mut res = Dictionary::new();
        res.set("Font", Object::Dictionary(fd));
        d.set("Resources", Object::Dictionary(res));
    }
    Stream::new(d, content.into_bytes())
}

/// Typsnittet för `name`: AcroFormens `/DR` om det är Helvetica/Arial med kodning, annars ett eget
/// Helvetica med WinAnsi (så att å, ä och ö blir rätt).
fn font_ref(doc: &mut Document, name: &str, helv: &mut Option<ObjectId>) -> ObjectId {
    let found = (|| {
        let cat = doc.catalog().ok()?;
        let acro = deref(doc, cat, b"AcroForm")?.as_dict().ok()?;
        let dr = deref(doc, acro, b"DR")?.as_dict().ok()?;
        let fonts = deref(doc, dr, b"Font")?.as_dict().ok()?;
        let r = fonts.get(name.as_bytes()).ok()?.as_reference().ok()?;
        let fd = doc.get_dictionary(r).ok()?;
        let base = fd.get(b"BaseFont").ok()?.as_name().ok()?;
        let base = String::from_utf8_lossy(base).to_lowercase();
        ((base.contains("helvetica") || base.contains("arial")) && fd.has(b"Encoding")).then_some(r)
    })();
    if let Some(r) = found {
        return r;
    }
    *helv.get_or_insert_with(|| {
        let mut d = Dictionary::new();
        d.set("Type", "Font");
        d.set("Subtype", "Type1");
        d.set("BaseFont", "Helvetica");
        d.set("Encoding", "WinAnsiEncoding");
        doc.add_object(Object::Dictionary(d))
    })
}

/// Bygger och kopplar nytt utseende (`/AP /N`) för ett text- eller valfält.
fn text_appearance(doc: &mut Document, f: &Field, w: &Widget, val: &Value, helv: &mut Option<ObjectId>) {
    let Some(g) = widget_geo(doc, w.id) else { return };
    let da = (|| {
        let wd = doc.get_dictionary(w.id).ok()?;
        deref(doc, wd, b"DA").and_then(str_of)
    })();
    // DA ärvs: widgetens egen först, annars fältets (`font_size`) och till sist AcroFormens.
    let da = da.or_else(|| field_da(doc, f.id)).unwrap_or_else(|| "/Helv 0 Tf 0 g".to_owned());
    let (font, da_size, color) = parse_da(&da);
    let (cw, ch) = if g.rot % 180 == 90 { (g.h, g.w) } else { (g.w, g.h) };
    let size = if da_size > 0.0 { da_size } else { auto_size(f, ch) };
    let content = format!(
        "{}/Tx BMC\nq\n{} {} {} {} re W n\nBT\n{color}\n{}ET\nQ\nEMC\n",
        frame_ops(&g, cw, ch),
        fmt(1.0),
        fmt(1.0),
        fmt((cw - 2.0).max(0.0)),
        fmt((ch - 2.0).max(0.0)),
        text_ops(f, val, cw, ch, &font, size),
    );
    let font_id = font_ref(doc, &font, helv);
    let sid = doc.add_object(make_stream(&g, content, Some((&font, font_id))));
    set_normal_ap(doc, w.id, Object::Reference(sid));
}

fn field_da(doc: &Document, id: ObjectId) -> Option<String> {
    let mut cur = id;
    for _ in 0..32 {
        let d = doc.get_dictionary(cur).ok()?;
        if let Some(s) = deref(doc, d, b"DA").and_then(str_of) {
            return Some(s);
        }
        match d.get(b"Parent").ok().and_then(|p| p.as_reference().ok()) {
            Some(p) => cur = p,
            None => break,
        }
    }
    let acro = deref(doc, doc.catalog().ok()?, b"AcroForm")?.as_dict().ok()?;
    deref(doc, acro, b"DA").and_then(str_of)
}

fn set_normal_ap(doc: &mut Document, w: ObjectId, n: Object) {
    if let Ok(d) = doc.get_dictionary_mut(w) {
        let mut ap = Dictionary::new();
        ap.set("N", n);
        d.set("AP", Object::Dictionary(ap));
    }
}

/// Gör att kryss-/alternativknappens "på"-läge har ett utseende (annars ritas det av oss).
fn ensure_check_ap(doc: &mut Document, w: &Widget, on: &str, radio: bool) {
    let Some(g) = widget_geo(doc, w.id) else { return };
    let has = (|| {
        let d = doc.get_dictionary(w.id).ok()?;
        let ap = deref(doc, d, b"AP")?.as_dict().ok()?;
        let n = deref(doc, ap, b"N")?.as_dict().ok()?;
        n.has(on.as_bytes()).then_some(())
    })()
    .is_some();
    if has {
        return;
    }
    let (cw, ch) = (g.w, g.h);
    let m = cw.min(ch);
    let content = if radio {
        let (cx, cy, r) = (cw / 2.0, ch / 2.0, m * 0.28);
        let k = r * 0.5523;
        let p = |x: f32, y: f32| format!("{} {}", fmt(x), fmt(y));
        format!(
            "q 0 g {} m {} {} {} c {} {} {} c {} {} {} c {} {} {} c f Q\n",
            p(cx + r, cy),
            p(cx + r, cy + k),
            p(cx + k, cy + r),
            p(cx, cy + r),
            p(cx - k, cy + r),
            p(cx - r, cy + k),
            p(cx - r, cy),
            p(cx - r, cy - k),
            p(cx - k, cy - r),
            p(cx, cy - r),
            p(cx + k, cy - r),
            p(cx + r, cy - k),
            p(cx + r, cy),
        )
    } else {
        format!(
            "q 0 G {} w 1 J 1 j {} {} m {} {} l {} {} l S Q\n",
            fmt(m * 0.12),
            fmt(cw * 0.22),
            fmt(ch * 0.5),
            fmt(cw * 0.42),
            fmt(ch * 0.26),
            fmt(cw * 0.78),
            fmt(ch * 0.76),
        )
    };
    let on_id = doc.add_object(make_stream(&g, format!("{}{content}", frame_ops(&g, cw, ch)), None));
    let off_id = doc.add_object(make_stream(&g, frame_ops(&g, cw, ch), None));
    let mut n = Dictionary::new();
    n.set(on.as_bytes().to_vec(), Object::Reference(on_id));
    n.set("Off", Object::Reference(off_id));
    set_normal_ap(doc, w.id, Object::Dictionary(n));
}

// ------------------------------------------------------------ text i utseenden

/// Bredder för Helvetica, tecken 32..=126, i 1/1000 em.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556,
    278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667,
    611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833,
    556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

pub fn text_width(s: &str, size: f32) -> f32 {
    s.chars().map(|c| if (' '..='~').contains(&c) { HELV[c as usize - 32] } else { 556 } as f32).sum::<f32>() / 1000.0 * size
}

/// Tecknet som en WinAnsi-byte (`?` om det saknas).
fn win_ansi(c: char) -> u8 {
    const HIGH: [(char, u8); 27] = [
        ('€', 0x80), ('‚', 0x82), ('ƒ', 0x83), ('„', 0x84), ('…', 0x85), ('†', 0x86), ('‡', 0x87), ('ˆ', 0x88), ('‰', 0x89),
        ('Š', 0x8A), ('‹', 0x8B), ('Œ', 0x8C), ('Ž', 0x8E), ('‘', 0x91), ('’', 0x92), ('“', 0x93), ('”', 0x94), ('•', 0x95),
        ('–', 0x96), ('—', 0x97), ('˜', 0x98), ('™', 0x99), ('š', 0x9A), ('›', 0x9B), ('œ', 0x9C), ('ž', 0x9E), ('Ÿ', 0x9F),
    ];
    let u = c as u32;
    if (0x20..0x7F).contains(&u) || (0xA0..0x100).contains(&u) {
        u as u8
    } else {
        HIGH.iter().find(|(h, _)| *h == c).map(|(_, b)| *b).unwrap_or(b'?')
    }
}

/// `(text)` som PDF-literal i ren ASCII (oktala tecken över 126).
fn literal(s: &str) -> String {
    let mut o = String::from("(");
    for c in s.chars() {
        match win_ansi(c) {
            b'(' => o += "\\(",
            b')' => o += "\\)",
            b'\\' => o += "\\\\",
            b if (0x20..0x7F).contains(&b) => o.push(b as char),
            b => o += &format!("\\{b:03o}"),
        }
    }
    o.push(')');
    o
}

/// Automatisk typsnittsstorlek (`/DA` med storlek 0).
pub fn auto_size(f: &Field, height: f32) -> f32 {
    if f.font_size > 0.0 {
        return f.font_size;
    }
    match f.kind {
        Kind::Text { multiline: true, .. } | Kind::List { .. } => 12.0,
        _ => ((height - 2.0) * 0.75).clamp(4.0, 14.0),
    }
}

/// Radbryter `text` till bredden `width` (ord för ord, långa ord bryts på tecken).
pub fn wrap_lines(text: &str, width: f32, size: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if text_width(&cand, size) <= width || line.is_empty() && word.is_empty() {
                line = cand;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            // Ordet ensamt: bryt på tecken om det ändå är för brett.
            let mut part = String::new();
            for c in word.chars() {
                let mut t = part.clone();
                t.push(c);
                if text_width(&t, size) > width && !part.is_empty() {
                    out.push(std::mem::take(&mut part));
                }
                part.push(c);
            }
            line = part;
        }
        out.push(line);
    }
    out
}

/// Textoperatorer (mellan BT och ET) för värdet, placerade i en ruta `w` × `h`.
fn text_ops(f: &Field, val: &Value, w: f32, h: f32, font: &str, size: f32) -> String {
    let mut s = format!("/{font} {} Tf\n", fmt(size));
    let pad = 2.0;
    let x_for = |tw: f32| match f.quad {
        1 => ((w - tw) / 2.0).max(pad),
        2 => (w - pad - tw).max(pad),
        _ => pad + 1.0,
    };
    let place = |s: &mut String, x: f32, y: f32, t: &str| {
        // Td är relativt föregående rad: håll koll på absolut position i stället.
        *s += &format!("1 0 0 1 {} {} Tm\n{} Tj\n", fmt(x), fmt(y), literal(t));
    };
    match (&f.kind, val) {
        (Kind::Text { multiline: true, password: false, .. }, Value::Text(t)) => {
            let lead = size * 1.15;
            for (i, line) in wrap_lines(t, w - 2.0 * pad - 2.0, size).iter().enumerate() {
                let y = h - pad - size * 0.8 - i as f32 * lead;
                if y < -size {
                    break;
                }
                place(&mut s, x_for(text_width(line, size)), y, line);
            }
        }
        (Kind::Text { comb: true, max_len, .. }, Value::Text(t)) => {
            let n = (*max_len).max(1);
            let cell = w / n as f32;
            let y = (h - size * 0.925) / 2.0 + size * 0.207;
            for (i, c) in t.chars().take(n).enumerate() {
                let cs = c.to_string();
                place(&mut s, i as f32 * cell + (cell - text_width(&cs, size)) / 2.0, y, &cs);
            }
        }
        (Kind::Text { password, .. }, Value::Text(t)) => {
            let line: String = if *password { "*".repeat(t.chars().count()) } else { t.replace(['\n', '\r'], " ") };
            let y = (h - size * 0.925) / 2.0 + size * 0.207;
            place(&mut s, x_for(text_width(&line, size)), y, &line);
        }
        (Kind::Combo { .. }, Value::Text(t)) => {
            let line = f.display_of(t);
            let y = (h - size * 0.925) / 2.0 + size * 0.207;
            place(&mut s, x_for(text_width(&line, size)), y, &line);
        }
        (Kind::List { .. }, Value::Multi(sel)) => {
            let lead = size * 1.15;
            let mut y = h - pad - size * 0.8;
            let mut hl = String::new();
            let mut body = String::new();
            for (e, d) in &f.options {
                if y < -size {
                    break;
                }
                if sel.contains(e) {
                    hl += &format!("{} {} {} {} re f\n", fmt(1.0), fmt(y - size * 0.25), fmt(w - 2.0), fmt(lead));
                }
                place(&mut body, pad + 1.0, y, d);
                y -= lead;
            }
            // Markeringen ritas före texten, utanför BT/ET.
            return format!("ET\nq 0.6 0.76 0.86 rg\n{hl}Q\nBT\n{s}{body}");
        }
        _ => {}
    }
    s
}

// ============================================================== tillstånd och gränssnitt

pub enum FormAction {
    Save,
}

#[derive(Default)]
pub struct FormState {
    fields: Vec<Field>,
    values: Vec<Value>,
    initial: Vec<Value>,
    /// Tab-ordning: (fält, widget) sorterat efter sida, rad och kolumn.
    order: Vec<(usize, usize)>,
    /// Markera fälten (ljusblått som pdf.js).
    pub highlight: bool,
    active: Option<(usize, usize)>,
    grab_focus: bool,
    scroll_to: bool,
    goto: Option<(usize, Rect)>,
}

impl FormState {
    pub fn load(bytes: &[u8], password: Option<&str>) -> Self {
        Self::from_fields(read_fields(bytes, password))
    }

    pub fn from_fields(fields: Vec<Field>) -> Self {
        let values: Vec<Value> = fields.iter().map(|f| f.value.clone()).collect();
        let mut order: Vec<(usize, usize)> = Vec::new();
        for (fi, f) in fields.iter().enumerate() {
            if f.read_only {
                continue;
            }
            for wi in 0..f.widgets.len() {
                order.push((fi, wi));
            }
        }
        order.sort_by_key(|&(fi, wi)| {
            let w = &fields[fi].widgets[wi];
            (w.page, (w.rect.top() / 4.0).floor() as i32, w.rect.left().round() as i32)
        });
        FormState { initial: values.clone(), values, fields, order, highlight: true, ..Default::default() }
    }

    pub fn has_fields(&self) -> bool {
        !self.fields.is_empty()
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }

    pub fn dirty(&self) -> bool {
        self.values != self.initial
    }

    /// Ett fält är öppet för redigering.
    pub fn editing(&self) -> bool {
        self.active.is_some()
    }

    /// De ändrade fälten, klara för `apply`.
    pub fn changes(&self) -> Vec<(ObjectId, Value)> {
        (0..self.fields.len()).filter(|&i| self.values[i] != self.initial[i]).map(|i| (self.fields[i].id, self.values[i].clone())).collect()
    }

    pub fn set_value(&mut self, field: usize, v: Value) {
        if let Some(slot) = self.values.get_mut(field) {
            *slot = v;
        }
    }

    pub fn activate(&mut self, field: usize, widget: usize) {
        if self.fields.get(field).is_some_and(|f| f.widgets.get(widget).is_some() && !f.read_only) {
            self.active = Some((field, widget));
            self.grab_focus = true;
        }
    }

    pub fn deactivate(&mut self) {
        self.active = None;
    }

    /// Flyttar till nästa (eller föregående) fält i Tab-ordningen.
    pub fn step(&mut self, back: bool) {
        if self.order.is_empty() {
            return;
        }
        let n = self.order.len();
        let cur = self.active.and_then(|a| self.order.iter().position(|o| *o == a));
        let next = match (cur, back) {
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
            (None, false) => 0,
            (None, true) => n - 1,
        };
        let (fi, wi) = self.order[next];
        self.active = Some((fi, wi));
        self.grab_focus = true;
        self.scroll_to = true;
        self.goto = Some((self.fields[fi].widgets[wi].page, self.fields[fi].widgets[wi].rect));
    }

    /// Sidan och rektangeln visaren ska rulla till efter Tab (om en sådan väntar).
    pub fn take_goto(&mut self) -> Option<(usize, Rect)> {
        self.goto.take()
    }

    /// Tangenter som hör till formuläret; anropas före visarens egna. Tab/Skift+Tab byter fält,
    /// Esc lämnar fältet och mellanslag växlar ett markerat kryss.
    pub fn pre_keys(&mut self, ctx: &egui::Context) {
        let Some((fi, wi)) = self.active else { return };
        let (back, fwd, esc) = ctx.input_mut(|i| {
            let back = i.consume_key(Modifiers::SHIFT, Key::Tab);
            (back, i.consume_key(Modifiers::NONE, Key::Tab), i.consume_key(Modifiers::NONE, Key::Escape))
        });
        if back || fwd {
            self.step(back);
        } else if esc {
            self.active = None;
        } else if matches!(self.fields[fi].kind, Kind::Check | Kind::Radio) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Space)) {
            self.click_state(fi, wi);
        }
    }

    fn click_state(&mut self, fi: usize, wi: usize) {
        let f = &self.fields[fi];
        let on = f.widgets[wi].on.clone();
        let cur_on = matches!(&self.values[fi], Value::State(s) if *s == on);
        let v = match (&f.kind, cur_on) {
            (Kind::Radio, _) => on,
            (_, true) => "Off".to_owned(),
            (_, false) => on,
        };
        self.values[fi] = Value::State(v);
    }

    /// Kroken för visaren: markerar fälten på sidan och sköter klick och redigering. `xf` är sidans
    /// omvandling till skärmen.
    pub fn page_ui(&mut self, ui: &mut Ui, page: usize, xf: &PageXf) {
        if self.fields.is_empty() {
            return;
        }
        let clip = ui.clip_rect();
        let k = xf.k;
        for fi in 0..self.fields.len() {
            for wi in 0..self.fields[fi].widgets.len() {
                let w = &self.fields[fi].widgets[wi];
                if w.page != page {
                    continue;
                }
                let screen = xf.rect_to_screen(w.rect);
                if !clip.intersects(screen) {
                    continue;
                }
                if self.active == Some((fi, wi)) {
                    self.edit(ui, fi, wi, screen, k);
                } else {
                    self.show_idle(ui, fi, wi, screen, k);
                }
            }
        }
    }

    fn show_idle(&mut self, ui: &mut Ui, fi: usize, wi: usize, screen: Rect, k: f32) {
        let f = &self.fields[fi];
        let changed = self.values[fi] != self.initial[fi];
        let painter = ui.painter_at(screen);
        if changed {
            // Sidans bild visar fortfarande det sparade utseendet: rita över med det nya.
            painter.rect_filled(screen, 0.0, Color32::WHITE);
            painter.rect_stroke(screen, 0.0, Stroke::new(1.0, Color32::from_gray(150)), StrokeKind::Inside);
            self.paint_value(&painter, fi, wi, screen, k);
        }
        if self.highlight && !f.read_only {
            painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(0, 54, 255, 34));
            if f.required {
                painter.rect_stroke(screen, 0.0, Stroke::new(1.0, Color32::from_rgb(220, 40, 40)), StrokeKind::Inside);
            }
        }
        if f.read_only {
            return;
        }
        let resp = ui.interact(screen, ui.make_persistent_id(("pdf-form", fi, wi)), Sense::click_and_drag());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(match f.kind {
                Kind::Text { .. } | Kind::Combo { editable: true } => egui::CursorIcon::Text,
                _ => egui::CursorIcon::PointingHand,
            });
        }
        if resp.clicked() {
            match f.kind {
                Kind::Check | Kind::Radio => self.click_state(fi, wi),
                _ => {}
            }
            self.active = Some((fi, wi));
            self.grab_focus = true;
        }
    }

    fn paint_value(&self, painter: &egui::Painter, fi: usize, wi: usize, screen: Rect, k: f32) {
        let f = &self.fields[fi];
        let size = auto_size(f, f.widgets[wi].rect.height()) * k;
        let font = FontId::proportional(size);
        let text_at = |t: &str| {
            let galley = painter.layout_no_wrap(t.to_owned(), font.clone(), Color32::BLACK);
            let x = match f.quad {
                1 => screen.center().x - galley.size().x / 2.0,
                2 => screen.right() - 2.0 * k - galley.size().x,
                _ => screen.left() + 2.0 * k,
            };
            painter.galley(pos2(x, screen.center().y - galley.size().y / 2.0), galley, Color32::BLACK);
        };
        match (&f.kind, &self.values[fi]) {
            (Kind::Text { password, multiline: false, .. }, Value::Text(t)) => {
                text_at(&if *password { "*".repeat(t.chars().count()) } else { t.clone() })
            }
            (Kind::Text { multiline: true, .. }, Value::Text(t)) => {
                let galley = painter.layout(t.clone(), font, Color32::BLACK, (screen.width() - 4.0 * k).max(1.0));
                painter.galley(screen.min + vec2(2.0 * k, 2.0 * k), galley, Color32::BLACK);
            }
            (Kind::Combo { .. }, Value::Text(t)) => text_at(&f.display_of(t)),
            (Kind::List { .. }, Value::Multi(v)) => {
                if let Some(first) = v.first() {
                    text_at(&f.display_of(first));
                }
            }
            (Kind::Check, Value::State(s)) if *s == f.widgets[wi].on => {
                let p = |x: f32, y: f32| screen.min + vec2(screen.width() * x, screen.height() * y);
                painter.add(egui::Shape::line(vec![p(0.22, 0.5), p(0.42, 0.74), p(0.78, 0.24)], Stroke::new(screen.height().min(screen.width()) * 0.12, Color32::BLACK)));
            }
            (Kind::Radio, Value::State(s)) if *s == f.widgets[wi].on => {
                painter.circle_filled(screen.center(), screen.width().min(screen.height()) * 0.28, Color32::BLACK);
            }
            _ => {}
        }
    }

    fn edit(&mut self, ui: &mut Ui, fi: usize, wi: usize, screen: Rect, k: f32) {
        let ctx = ui.ctx().clone();
        let f = self.fields[fi].clone();
        let size = auto_size(&f, f.widgets[wi].rect.height()) * k;
        let accent = ui.visuals().selection.stroke.color;
        let grab = std::mem::take(&mut self.grab_focus);
        if std::mem::take(&mut self.scroll_to) {
            ui.scroll_to_rect(screen.expand(40.0), None);
        }
        ui.painter().rect_filled(screen, 0.0, Color32::WHITE);
        let outside = ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_latest_pos().is_some_and(|p| !screen.contains(p));
        let mut child = ui.new_child(
            UiBuilder::new().id_salt(("pdf-form-edit", fi, wi)).max_rect(screen.shrink(1.0)).layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(screen.intersect(ui.clip_rect()));
        child.visuals_mut().override_text_color = Some(Color32::BLACK);
        match &f.kind {
            Kind::Text { multiline, password, max_len, .. } => {
                let mut s = match &self.values[fi] {
                    Value::Text(t) => t.clone(),
                    _ => String::new(),
                };
                let font = FontId::proportional(size);
                let mut te = if *multiline {
                    let rows = ((screen.height() / (size * 1.25)).floor() as usize).max(1);
                    TextEdit::multiline(&mut s).desired_rows(rows).lock_focus(false)
                } else {
                    TextEdit::singleline(&mut s)
                };
                te = te
                    .id(child.make_persistent_id(("pdf-form-te", fi, wi)))
                    .font(font)
                    .text_color(Color32::BLACK)
                    .frame(Frame::NONE)
                    .margin(Margin::symmetric(2, 1))
                    .desired_width((screen.width() - 2.0).max(10.0))
                    .password(*password);
                if *max_len > 0 {
                    te = te.char_limit(*max_len);
                }
                let r = child.add(te);
                if grab {
                    r.request_focus();
                }
                if r.changed() {
                    self.values[fi] = Value::Text(s);
                }
                if r.lost_focus() && self.active == Some((fi, wi)) {
                    self.active = None;
                }
            }
            Kind::Combo { editable: true } => {
                let mut s = match &self.values[fi] {
                    Value::Text(t) => t.clone(),
                    _ => String::new(),
                };
                let r = child.add(
                    TextEdit::singleline(&mut s)
                        .id(child.make_persistent_id(("pdf-form-te", fi, wi)))
                        .font(FontId::proportional(size))
                        .text_color(Color32::BLACK)
                        .frame(Frame::NONE)
                        .desired_width((screen.width() - 2.0).max(10.0)),
                );
                if grab {
                    r.request_focus();
                }
                if r.changed() {
                    self.values[fi] = Value::Text(s);
                }
                if r.lost_focus() && self.active == Some((fi, wi)) {
                    self.active = None;
                }
            }
            Kind::Combo { editable: false } => {
                let salt = ("pdf-form-combo", fi, wi);
                let cid = child.make_persistent_id(salt);
                if grab {
                    Popup::open_id(&ctx, cid.with("popup"));
                }
                let cur = match &self.values[fi] {
                    Value::Text(t) => t.clone(),
                    _ => String::new(),
                };
                let mut pick = None;
                ComboBox::from_id_salt(salt)
                    .width((screen.width() - 6.0).max(10.0))
                    .selected_text(RichText::new(f.display_of(&cur)).size(size.clamp(8.0, 40.0)))
                    .show_ui(&mut child, |ui| {
                        for (e, d) in &f.options {
                            if ui.selectable_label(*e == cur, d).clicked() {
                                pick = Some(e.clone());
                            }
                        }
                    });
                if let Some(e) = pick {
                    self.values[fi] = Value::Text(e);
                }
                if outside && !grab && !ComboBox::is_open(&ctx, cid) && self.active == Some((fi, wi)) {
                    self.active = None;
                }
            }
            Kind::List { multi } => {
                let mut sel = match &self.values[fi] {
                    Value::Multi(v) => v.clone(),
                    _ => Vec::new(),
                };
                let before = sel.clone();
                ScrollArea::vertical().max_height(screen.height() - 2.0).auto_shrink([false, false]).show(&mut child, |ui| {
                    for (e, d) in &f.options {
                        let on = sel.contains(e);
                        if ui.selectable_label(on, RichText::new(d).size(size.clamp(8.0, 40.0))).clicked() {
                            if *multi {
                                if on {
                                    sel.retain(|x| x != e);
                                } else {
                                    sel.push(e.clone());
                                }
                            } else {
                                sel = vec![e.clone()];
                            }
                        }
                    }
                });
                if sel != before {
                    self.values[fi] = Value::Multi(sel);
                }
                if outside && !grab && self.active == Some((fi, wi)) {
                    self.active = None;
                }
            }
            Kind::Check | Kind::Radio => {
                self.paint_value(&ui.painter_at(screen), fi, wi, screen, k);
                let resp = ui.interact(screen, ui.make_persistent_id(("pdf-form", fi, wi)), Sense::click());
                if resp.clicked() {
                    self.click_state(fi, wi);
                }
                if outside && !resp.hovered() && self.active == Some((fi, wi)) {
                    self.active = None;
                }
            }
        }
        if self.active == Some((fi, wi)) {
            ui.painter().rect_stroke(screen, 0.0, Stroke::new(2.0, accent), StrokeKind::Outside);
        }
    }
}

// ------------------------------------------------------------ fältet under verktygsraden

const BAR_H: f32 = 34.0;

/// Raden "Det här dokumentet har formulärfält" under verktygsraden. Syns bara om PDF:en har fält.
pub fn bar(ui: &mut Ui, p: &Palette, forms: &mut FormState) -> Option<FormAction> {
    if !forms.has_fields() {
        return None;
    }
    let mut act = None;
    Panel::top("kladd-pdf-forms")
        .exact_size(BAR_H)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(p.surface).inner_margin(Margin::symmetric(12, 4)))
        .show(ui, |ui| {
            let r = ui.max_rect();
            ui.painter().hline((r.left() - 12.0)..=(r.right() + 12.0), r.bottom() + 4.0, Stroke::new(1.0, p.line));
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(t("Det här dokumentet har formulärfält")).font(FontId::new(13.0, theme::medium())).color(p.text));
                let txt = RichText::new(t("Markera fält")).color(if forms.highlight { p.on_accent } else { p.text });
                let b = egui::Button::new(txt).fill(if forms.highlight { p.accent } else { p.surface_hover }).stroke(Stroke::NONE);
                if ui.add(b).on_hover_text(t("Markera ifyllbara fält med ljusblått")).clicked() {
                    forms.highlight = !forms.highlight;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if forms.dirty() {
                        let save = egui::Button::new(RichText::new(t("Spara")).color(p.on_accent)).fill(p.accent).stroke(Stroke::NONE);
                        if ui.add(save).on_hover_text(crate::keys::shortcut(t("Spara ändringarna i PDF:en (Ctrl+S)"))).clicked() {
                            act = Some(FormAction::Save);
                        }
                        ui.label(RichText::new(t("Osparade ändringar")).color(p.muted));
                    }
                });
            });
        });
    act
}

// ============================================================== tester

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfview::engine::PdfEngine;
    use lopdf::dictionary;
    use std::sync::Arc;

    fn rect(a: [i64; 4]) -> Object {
        Object::Array(a.iter().map(|x| Object::Integer(*x)).collect())
    }

    fn stream(doc: &mut Document, content: &str) -> ObjectId {
        doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => rect([0, 0, 14, 14]) }, content.as_bytes().to_vec()))
    }

    /// Sidan 200x300 med text, kryss, alternativgrupp, kombinationsruta, listruta och en knapp.
    fn build(rotate: i64) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.new_object_id();
        let helv = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
        });
        let yes = stream(&mut doc, "0 0 14 14 re f");
        let off = stream(&mut doc, "");
        let ra = stream(&mut doc, "0 0 7 7 re f");
        let rb = stream(&mut doc, "0 0 9 9 re f");
        let roff = stream(&mut doc, "");
        let text = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("namn"),
            "V" => Object::string_literal("Anna"), "Rect" => rect([20, 250, 120, 270]), "P" => page_id,
            "DA" => Object::string_literal("/Helv 12 Tf 0 g"), "MaxLen" => 40,
        });
        let multi = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "Ff" => 1 << 12, "T" => Object::string_literal("anm"),
            "Rect" => rect([20, 150, 120, 220]), "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        });
        let check = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "T" => Object::string_literal("ok"),
            "Rect" => rect([20, 120, 34, 134]), "V" => "Off", "AS" => "Off",
            "AP" => dictionary! { "N" => dictionary! { "Yes" => yes, "Off" => off } },
        });
        let r1 = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Rect" => rect([20, 90, 29, 99]), "AS" => "Off",
            "AP" => dictionary! { "N" => dictionary! { "A" => ra, "Off" => roff } },
        });
        let r2 = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Rect" => rect([60, 90, 69, 99]), "AS" => "Off",
            "AP" => dictionary! { "N" => dictionary! { "B" => rb, "Off" => roff } },
        });
        let radio = doc.add_object(dictionary! {
            "FT" => "Btn", "Ff" => 1 << 15, "T" => Object::string_literal("val"), "Kids" => vec![r1.into(), r2.into()],
        });
        for r in [r1, r2] {
            doc.get_dictionary_mut(r).unwrap().set("Parent", radio);
        }
        let combo = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "Ff" => 1 << 17, "T" => Object::string_literal("stad"),
            "Rect" => rect([20, 50, 120, 70]), "DA" => Object::string_literal("/Helv 11 Tf 0 g"),
            "Opt" => vec![
                Object::Array(vec![Object::string_literal("sto"), Object::string_literal("Stockholm")]),
                Object::Array(vec![Object::string_literal("got"), pdf_text("Göteborg")]),
            ],
        });
        let list = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "T" => Object::string_literal("lista"),
            "Rect" => rect([130, 50, 190, 100]), "Opt" => vec![Object::string_literal("a"), Object::string_literal("b")],
        });
        let button = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "Ff" => 1 << 16, "T" => Object::string_literal("skicka"),
            "Rect" => rect([130, 10, 190, 30]),
        });
        let hidden = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("dold"), "F" => 2,
            "Rect" => rect([130, 200, 190, 220]),
        });
        let all = [text, multi, check, r1, r2, combo, list, button, hidden];
        let annots: Vec<Object> = all.iter().map(|i| Object::Reference(*i)).collect();
        let content = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        doc.objects.insert(page_id, Object::Dictionary(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content, "Rotate" => rotate,
            "MediaBox" => rect([0, 0, 200, 300]), "Annots" => annots,
        }));
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }));
        let acro = dictionary! {
            "Fields" => vec![text.into(), multi.into(), check.into(), radio.into(), combo.into(), list.into(), button.into(), hidden.into()],
            "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
            "DR" => dictionary! { "Font" => dictionary! { "Helv" => helv } },
        };
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "AcroForm" => acro });
        doc.trailer.set("Root", cat);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    fn by_name<'a>(fs: &'a [Field], n: &str) -> &'a Field {
        fs.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("fält {n} saknas"))
    }

    fn to_text(o: &Object) -> String {
        match o {
            Object::String(b, _) => decode_text(b),
            Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
            _ => panic!("inte text: {o:?}"),
        }
    }

    #[test]
    fn reads_all_field_kinds() {
        let fs = read_fields(&build(0), None);
        // Knappen och det dolda fältet hoppas över.
        assert_eq!(fs.len(), 6, "{:?}", fs.iter().map(|f| &f.name).collect::<Vec<_>>());
        let t = by_name(&fs, "namn");
        assert_eq!(t.value, Value::Text("Anna".into()));
        assert!(matches!(t.kind, Kind::Text { multiline: false, max_len: 40, .. }));
        assert_eq!(t.font_size, 12.0);
        // Visningspunkter: y vänds (300 - 270 = 30).
        assert_eq!(t.widgets[0].rect, Rect::from_min_max(pos2(20.0, 30.0), pos2(120.0, 50.0)));
        assert!(matches!(by_name(&fs, "anm").kind, Kind::Text { multiline: true, .. }));
        let c = by_name(&fs, "ok");
        assert_eq!((&c.kind, &c.value, c.widgets[0].on.as_str()), (&Kind::Check, &Value::State("Off".into()), "Yes"));
        let r = by_name(&fs, "val");
        assert_eq!(r.kind, Kind::Radio);
        assert_eq!(r.widgets.iter().map(|w| w.on.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        let s = by_name(&fs, "stad");
        assert_eq!(s.kind, Kind::Combo { editable: false });
        assert_eq!(s.options[1], ("got".to_owned(), "Göteborg".to_owned()));
        assert!(matches!(by_name(&fs, "lista").kind, Kind::List { multi: false }));
    }

    #[test]
    fn rect_follows_page_rotation() {
        let fs = read_fields(&build(90), None);
        let t = by_name(&fs, "namn");
        // Sidan roteras 90 grader medurs: bredd 300, höjd 200; PDF-rutans nedre vänstra hörn hamnar uppe till vänster.
        let r = t.widgets[0].rect;
        assert!((r.width() - 20.0).abs() < 0.01 && (r.height() - 100.0).abs() < 0.01, "{r:?}");
        assert!((r.left() - 250.0).abs() < 0.01 && (r.top() - 20.0).abs() < 0.01, "{r:?}");
    }

    #[test]
    fn apply_sets_values_and_states() {
        let bytes = build(0);
        let fs = read_fields(&bytes, None);
        let vals = vec![
            (by_name(&fs, "namn").id, Value::Text("Åsa Öberg (testad)".into())),
            (by_name(&fs, "anm").id, Value::Text("En lång rad som behöver brytas över flera rader i rutan\nNy rad".into())),
            (by_name(&fs, "ok").id, Value::State("Yes".into())),
            (by_name(&fs, "val").id, Value::State("B".into())),
            (by_name(&fs, "stad").id, Value::Text("got".into())),
            (by_name(&fs, "lista").id, Value::Multi(vec!["b".into()])),
        ];
        let out = apply(&bytes, None, &vals).expect("apply");
        let back = read_fields(&out, None);
        assert_eq!(by_name(&back, "namn").value, Value::Text("Åsa Öberg (testad)".into()));
        assert_eq!(by_name(&back, "ok").value, Value::State("Yes".into()));
        assert_eq!(by_name(&back, "val").value, Value::State("B".into()));
        assert_eq!(by_name(&back, "stad").value, Value::Text("got".into()));
        assert_eq!(by_name(&back, "lista").value, Value::Multi(vec!["b".into()]));

        // Rå kontroll av /V, /AS och utseendeströmmen.
        let doc = Document::load_mem(&out).unwrap();
        let get = |n: &str| doc.get_dictionary(by_name(&back, n).id).unwrap();
        assert_eq!(to_text(get("namn").get(b"V").unwrap()), "Åsa Öberg (testad)");
        assert_eq!(to_text(get("ok").get(b"V").unwrap()), "Yes");
        assert_eq!(to_text(get("ok").get(b"AS").unwrap()), "Yes");
        let radio = by_name(&back, "val");
        let ass: Vec<String> = radio.widgets.iter().map(|w| to_text(doc.get_dictionary(w.id).unwrap().get(b"AS").unwrap())).collect();
        assert_eq!(ass, ["Off", "B"]);
        let ap = get("namn").get(b"AP").unwrap().as_dict().unwrap();
        let sid = ap.get(b"N").unwrap().as_reference().unwrap();
        let st = doc.get_object(sid).unwrap().as_stream().unwrap();
        let c = String::from_utf8_lossy(&st.content).into_owned();
        assert!(c.contains("/Helv 12 Tf") && c.contains("\\305sa \\326berg \\(testad\\)"), "{c}");
        assert!(st.dict.get(b"Resources").is_ok());
        // Flerradig text bryts.
        let multi = get("anm").get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
        let c = String::from_utf8_lossy(&doc.get_object(multi).unwrap().as_stream().unwrap().content).into_owned();
        assert!(c.matches(" Tj").count() >= 4, "{c}");
        // Kombinationsrutan visar visningstexten.
        let combo = get("stad").get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
        let c = String::from_utf8_lossy(&doc.get_object(combo).unwrap().as_stream().unwrap().content).into_owned();
        assert!(c.contains("(G\\366teborg)"), "{c}");
        // /NeedAppearances
        let acro = doc.catalog().unwrap().get(b"AcroForm").unwrap().as_dict().unwrap();
        assert_eq!(acro.get(b"NeedAppearances").unwrap(), &Object::Boolean(true));
    }

    #[test]
    fn engine_opens_the_result() {
        let bytes = build(0);
        let fs = read_fields(&bytes, None);
        let out = apply(&bytes, None, &[(by_name(&fs, "namn").id, Value::Text("Kalle".into())), (by_name(&fs, "ok").id, Value::State("Yes".into()))]).unwrap();
        let engine = PdfEngine::open(Arc::new(out), None, egui::Context::default()).expect("öppnas");
        assert_eq!(engine.page_count(), 1);
        assert!(PdfEngine::open(Arc::new(bytes), None, egui::Context::default()).is_ok());
    }

    #[test]
    fn unicode_text_is_written_as_utf16() {
        let bytes = build(0);
        let fs = read_fields(&bytes, None);
        let out = apply(&bytes, None, &[(by_name(&fs, "namn").id, Value::Text("Привет ✓".into()))]).unwrap();
        assert_eq!(by_name(&read_fields(&out, None), "namn").value, Value::Text("Привет ✓".into()));
    }

    #[test]
    fn radio_selects_the_first_state() {
        let bytes = build(0);
        let fs = read_fields(&bytes, None);
        let out = apply(&bytes, None, &[(by_name(&fs, "val").id, Value::State("A".into()))]).unwrap();
        let back = read_fields(&out, None);
        assert_eq!(by_name(&back, "val").value, Value::State("A".into()));
    }

    #[test]
    fn wrapping_and_widths() {
        assert!((text_width("iii", 10.0) - 6.66).abs() < 0.01);
        let lines = wrap_lines("aaa bbb ccc", text_width("aaa bbb", 10.0) + 0.1, 10.0);
        assert_eq!(lines, ["aaa bbb", "ccc"]);
        assert_eq!(wrap_lines("a\n\nb", 100.0, 10.0), ["a", "", "b"]);
        let long = wrap_lines("mmmmmmmmmmmmmmmmmmmm", 30.0, 10.0);
        assert!(long.len() > 1 && long.iter().all(|l| text_width(l, 10.0) <= 30.0 + 9.0));
        assert_eq!(literal("a(b)\\"), "(a\\(b\\)\\\\)");
        assert_eq!(literal("å"), "(\\345)");
        assert_eq!(parse_da("0 0 1 rg /Helv 9 Tf"), ("Helv".to_owned(), 9.0, "0 0 1 rg".to_owned()));
    }

    #[test]
    fn state_dirty_changes_and_tab_order() {
        let bytes = build(0);
        let mut st = FormState::load(&bytes, None);
        assert!(st.has_fields() && !st.dirty() && st.changes().is_empty());
        // Ordning uppifrån och ned: namn, anm, ok, val(A), val(B), stad/lista på samma rad.
        let names: Vec<&str> = st.order.iter().map(|&(f, _)| st.fields[f].name.as_str()).collect();
        assert_eq!(&names[..5], ["namn", "anm", "ok", "val", "val"], "{names:?}");
        st.step(false);
        assert_eq!(st.active, Some(st.order[0]));
        st.step(true);
        assert_eq!(st.active, Some(*st.order.last().unwrap()));
        st.step(false);
        assert_eq!(st.active, Some(st.order[0]));
        let fi = st.fields.iter().position(|f| f.name == "ok").unwrap();
        st.click_state(fi, 0);
        assert_eq!(st.values[fi], Value::State("Yes".into()));
        assert!(st.dirty());
        assert_eq!(st.changes().len(), 1);
        st.click_state(fi, 0);
        assert!(!st.dirty());
        let out = apply(&bytes, None, &[(st.fields[fi].id, Value::State("Yes".into()))]).unwrap();
        assert!(!FormState::load(&out, None).dirty());
    }

    #[test]
    fn garbage_and_plain_pdfs_have_no_fields() {
        assert!(read_fields(b"inte en pdf", None).is_empty());
        assert!(apply(b"inte en pdf", None, &[]).is_err());
        let fs = read_fields(include_bytes!("../../tests/data/krypterad.pdf"), Some("hemligt"));
        assert!(fs.is_empty());
    }
}
