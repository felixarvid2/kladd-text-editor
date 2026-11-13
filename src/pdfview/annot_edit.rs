//! Redigera och ta bort anteckningar som redan finns i PDF:en (annot.rs hanterar de nya).
//!
//! `read` läser sidornas `/Annots` med lopdf (markering, understruken, genomstruken, vågig, lapp,
//! textruta, penna, rektangel, ellips, linje; länkar, formulärfält och popup-rutor hoppas över) till
//! `ExAnnot` med geometri i visningspunkter. Ändringar hålls som `Edits` per objektnummer och
//! läggs på PDF:en först vid sparande (`edit_doc`, anropad av `annot::apply_all`): borttagning
//! tar bort posten, dess popup och svar; ändring skriver `/Contents`, `/C`, `/M` och ritar om `/AP`
//! för de typer Kladd kan rita (de andra behåller sitt utseende och får bara ny text).

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use lopdf::{Document, Object, ObjectId};

use super::annot::{self, AnnotState, HIGHLIGHT_COLORS, INK_COLORS, NewAnnot, NewKind};
use super::meta::{Geo, num, rect_of};
use crate::i18n::t;
use crate::tf;

const MAX_ANNOTS: usize = 20_000;
const MAX_UNDO: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Text,
    FreeText,
    Ink,
    Square,
    Circle,
    Line,
}

impl Kind {
    fn parse(name: &[u8]) -> Option<Kind> {
        Some(match name {
            b"Highlight" => Kind::Highlight,
            b"Underline" => Kind::Underline,
            b"StrikeOut" => Kind::StrikeOut,
            b"Squiggly" => Kind::Squiggly,
            b"Text" => Kind::Text,
            b"FreeText" => Kind::FreeText,
            b"Ink" => Kind::Ink,
            b"Square" => Kind::Square,
            b"Circle" => Kind::Circle,
            b"Line" => Kind::Line,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Highlight => t("Markering"),
            Kind::Underline => t("Understruken"),
            Kind::StrikeOut => t("Genomstruken"),
            Kind::Squiggly => t("Vågig understrykning"),
            Kind::Text => t("Anteckning"),
            Kind::FreeText => t("Textruta"),
            Kind::Ink => t("Ritning"),
            Kind::Square => t("Rektangel"),
            Kind::Circle => t("Ellips"),
            Kind::Line => t("Linje"),
        }
    }

    /// Typer vars utseende Kladd kan rita om (annot.rs); övriga behåller sitt `/AP`.
    pub fn redrawable(self) -> bool {
        matches!(self, Kind::Highlight | Kind::Underline | Kind::StrikeOut | Kind::Text | Kind::FreeText | Kind::Ink)
    }
}

/// En anteckning som redan finns i PDF:en. Geometri i visningspunkter (se `Geo`).
#[derive(Clone, Debug, PartialEq)]
pub struct ExAnnot {
    pub id: ObjectId,
    pub page: usize,
    pub kind: Kind,
    /// Omslutande rektangel.
    pub rect: Rect,
    /// Rader (QuadPoints) för markeringstyperna.
    pub quads: Vec<Rect>,
    /// Streck för Ink.
    pub strokes: Vec<Vec<Pos2>>,
    /// Linjens ändpunkter.
    pub line: Option<(Pos2, Pos2)>,
    pub width: f32,
    pub color: Option<[u8; 3]>,
    pub author: String,
    /// Datum som i PDF:en (`D:ÅÅÅÅMMDD…`), se `show_date`.
    pub date: String,
    pub contents: String,
    pub popup: Option<ObjectId>,
}

impl ExAnnot {
    /// Träffar punkten (visningspunkter) anteckningen?
    pub fn hit(&self, pt: Pos2) -> bool {
        let near = |a: Pos2, b: Pos2, r: f32| {
            let ab = b - a;
            let len2 = ab.length_sq();
            let t = if len2 <= f32::EPSILON { 0.0 } else { ((pt - a).dot(ab) / len2).clamp(0.0, 1.0) };
            (a + ab * t).distance(pt) <= r
        };
        match self.kind {
            Kind::Highlight | Kind::Underline | Kind::StrikeOut | Kind::Squiggly => {
                if self.quads.is_empty() {
                    self.rect.expand(1.5).contains(pt)
                } else {
                    self.quads.iter().any(|q| q.expand(1.5).contains(pt))
                }
            }
            Kind::Ink => {
                let r = self.width / 2.0 + 3.0;
                self.strokes.iter().any(|s| match s.len() {
                    0 => false,
                    1 => s[0].distance(pt) <= r,
                    _ => s.windows(2).any(|w| near(w[0], w[1], r)),
                })
            }
            Kind::Line => match self.line {
                Some((a, b)) => near(a, b, self.width / 2.0 + 3.0),
                None => self.rect.expand(2.0).contains(pt),
            },
            // Stora ramar träffas bara på kanten, så att texten inuti går att markera.
            Kind::Square | Kind::Circle => {
                let r = self.rect;
                if r.width() > 40.0 && r.height() > 40.0 {
                    r.expand(3.0).contains(pt) && !r.shrink(6.0).contains(pt)
                } else {
                    r.expand(3.0).contains(pt)
                }
            }
            Kind::Text | Kind::FreeText => self.rect.expand(2.0).contains(pt),
        }
    }

    /// Anteckningen som ny (för att rita om utseendet), med given färg och text.
    fn redraw_as(&self, color: [u8; 3], text: &str, page_size: (f32, f32)) -> Option<NewAnnot> {
        let kind = match self.kind {
            Kind::Highlight | Kind::Underline | Kind::StrikeOut => {
                let rects = if self.quads.is_empty() { vec![self.rect] } else { self.quads.clone() };
                match self.kind {
                    Kind::Highlight => NewKind::Highlight { rects },
                    Kind::Underline => NewKind::Underline { rects },
                    _ => NewKind::StrikeOut { rects },
                }
            }
            Kind::Text => NewKind::Note { pos: self.rect.min, text: text.to_owned() },
            Kind::FreeText => {
                // Samma bredd; höjden växer så att den nya texten ryms (inom sidan).
                let w = self.rect.width().max(40.0);
                let need = 4.0 + annot::layout_text(text, annot::FREE_SIZE, w - 4.0).len().max(1) as f32 * annot::FREE_SIZE * 1.2;
                let h = self.rect.height().max(need).min(page_size.1.max(10.0));
                let y = self.rect.min.y.min((page_size.1 - h).max(0.0));
                NewKind::FreeText { rect: Rect::from_min_size(pos2(self.rect.min.x, y), vec2(w, h)), text: text.to_owned() }
            }
            Kind::Ink => NewKind::Ink { strokes: self.strokes.clone(), width: self.width },
            _ => return None,
        };
        Some(NewAnnot { page: self.page, color, kind })
    }
}

/// Väntande ändring av en befintlig anteckning.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edit {
    pub contents: Option<String>,
    pub color: Option<[u8; 3]>,
    pub deleted: bool,
}

pub type Edits = BTreeMap<ObjectId, Edit>;

// ============================================================== läsa

fn nums(doc: &Document, o: Option<&Object>) -> Vec<f32> {
    let Some(arr) = o.and_then(|o| doc.dereference(o).ok()).and_then(|x| x.1.as_array().ok()) else { return vec![] };
    arr.iter().filter_map(|x| num(doc, x)).collect()
}

/// PDF-textsträng (UTF-16 med BOM, UTF-8 med BOM eller Latin-1 som approximation av PDFDocEncoding).
pub fn decode_text(b: &[u8]) -> String {
    if let Some(r) = b.strip_prefix(&[0xFE, 0xFF]) {
        let u: Vec<u16> = r.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else if let Some(r) = b.strip_prefix(&[0xFF, 0xFE]) {
        let u: Vec<u16> = r.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else if let Some(r) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(r).into_owned()
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

fn text_of(doc: &Document, d: &lopdf::Dictionary, key: &[u8]) -> String {
    match d.get(key).ok().and_then(|o| doc.dereference(o).ok()).map(|x| x.1) {
        Some(Object::String(b, _)) => decode_text(b),
        _ => String::new(),
    }
}

fn color_of(c: &[f32]) -> Option<[u8; 3]> {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    match c.len() {
        1 => Some([b(c[0]); 3]),
        3 => Some([b(c[0]), b(c[1]), b(c[2])]),
        4 => Some([b((1.0 - c[0]) * (1.0 - c[3])), b((1.0 - c[1]) * (1.0 - c[3])), b((1.0 - c[2]) * (1.0 - c[3]))]),
        _ => None,
    }
}

/// Textfärgen ur en `/DA`-sträng ("/Helv 12 Tf 1 0 0 rg").
fn da_color(da: &str) -> Option<[u8; 3]> {
    let tok: Vec<&str> = da.split_whitespace().collect();
    let i = tok.iter().rposition(|t| matches!(*t, "rg" | "g"))?;
    let n = if tok[i] == "rg" { 3 } else { 1 };
    let v: Vec<f32> = tok.get(i.checked_sub(n)?..i)?.iter().filter_map(|t| t.parse().ok()).collect();
    if v.len() == n { color_of(&v) } else { None }
}

/// `D:20240131123456+01'00'` → `2024-01-31 12:34`. Okänt format ges oförändrat.
pub fn show_date(d: &str) -> String {
    let digits: String = d.strip_prefix("D:").unwrap_or(d).chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 8 {
        return d.to_owned();
    }
    let mut s = format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..8]);
    if digits.len() >= 12 {
        s.push_str(&format!(" {}:{}", &digits[8..10], &digits[10..12]));
    }
    s
}

fn page_refs(doc: &Document, page: ObjectId) -> Vec<Object> {
    let Ok(pd) = doc.get_dictionary(page) else { return vec![] };
    pd.get(b"Annots").ok().and_then(|o| doc.dereference(o).ok()).and_then(|x| x.1.as_array().ok()).cloned().unwrap_or_default()
}

/// Alla redigerbara anteckningar i dokumentet, sida för sida.
pub fn read_doc(doc: &Document) -> Vec<ExAnnot> {
    let mut out = Vec::new();
    for (i, (_, page)) in doc.get_pages().into_iter().enumerate() {
        let g = Geo::of(doc, page);
        for o in page_refs(doc, page) {
            if out.len() >= MAX_ANNOTS {
                return out;
            }
            let Object::Reference(id) = o else { continue };
            let Ok(d) = doc.get_dictionary(id) else { continue };
            let Some(kind) = d.get(b"Subtype").ok().and_then(|s| s.as_name().ok()).and_then(Kind::parse) else { continue };
            // Dolda (2) och inte visade (32) anteckningar syns inte och går inte att klicka på.
            let flags = d.get(b"F").ok().and_then(|f| num(doc, f)).unwrap_or(0.0) as i64;
            if flags & (2 | 32) != 0 {
                continue;
            }
            let pt = |x: f32, y: f32| {
                let (a, b) = g.point(x, y);
                pos2(a, b)
            };
            let rect = d.get(b"Rect").ok().and_then(|r| rect_of(doc, doc.dereference(r).ok()?.1)).map(|r| g.rect(r));
            let mut quads = Vec::new();
            for q in nums(doc, d.get(b"QuadPoints").ok()).chunks_exact(8).take(5000) {
                let (mut r, mut first) = (Rect::NOTHING, true);
                for p in q.chunks_exact(2) {
                    if first {
                        r = Rect::from_min_max(pt(p[0], p[1]), pt(p[0], p[1]));
                        first = false;
                    } else {
                        r.extend_with(pt(p[0], p[1]));
                    }
                }
                quads.push(r);
            }
            let mut strokes = Vec::new();
            if kind == Kind::Ink {
                if let Some(list) = d.get(b"InkList").ok().and_then(|o| doc.dereference(o).ok()).and_then(|x| x.1.as_array().ok()) {
                    for s in list.iter().take(5000) {
                        let v = nums(doc, Some(s));
                        strokes.push(v.chunks_exact(2).map(|p| pt(p[0], p[1])).collect::<Vec<_>>());
                    }
                }
            }
            let l = nums(doc, d.get(b"L").ok());
            let line = (kind == Kind::Line && l.len() >= 4).then(|| (pt(l[0], l[1]), pt(l[2], l[3])));
            let width = d
                .get(b"BS")
                .ok()
                .and_then(|o| doc.dereference(o).ok())
                .and_then(|x| x.1.as_dict().ok())
                .and_then(|b| b.get(b"W").ok())
                .and_then(|w| num(doc, w))
                .or_else(|| nums(doc, d.get(b"Border").ok()).get(2).copied())
                .unwrap_or(1.0);
            let bounds = || {
                let mut r = Rect::NOTHING;
                for p in strokes.iter().flatten().chain(line.iter().flat_map(|(a, b)| [a, b])) {
                    r.extend_with(*p);
                }
                for q in &quads {
                    r = r.union(*q);
                }
                r.expand(width / 2.0 + 1.0)
            };
            let rect = match rect {
                Some(r) if r.width() > 0.0 || r.height() > 0.0 => r,
                _ => {
                    let b = bounds();
                    if !b.is_positive() {
                        continue;
                    }
                    b
                }
            };
            let mut color = color_of(&nums(doc, d.get(b"C").ok()));
            if kind == Kind::FreeText {
                color = da_color(&text_of(doc, d, b"DA")).or(color);
            }
            let popup = match d.get(b"Popup") {
                Ok(Object::Reference(p)) => Some(*p),
                _ => None,
            };
            out.push(ExAnnot {
                id,
                page: i,
                kind,
                rect,
                quads,
                strokes,
                line,
                width,
                color,
                author: text_of(doc, d, b"T"),
                date: text_of(doc, d, b"M"),
                contents: text_of(doc, d, b"Contents"),
                popup,
            });
        }
    }
    out
}

/// Läser anteckningarna ur PDF-bytes. Ger tom lista vid fel och panikar aldrig.
pub fn read(bytes: &[u8], password: Option<&str>) -> Vec<ExAnnot> {
    std::panic::catch_unwind(|| annot::open_doc(bytes, password).map(|d| read_doc(&d)).unwrap_or_default()).unwrap_or_default()
}

// ============================================================== skriva

/// Tar bort `gone` ur alla sidors `/Annots` och ur dokumentet.
fn remove_annots(doc: &mut Document, gone: &mut BTreeSet<ObjectId>) {
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    // Popup-rutor och svar (/IRT) hör till den borttagna anteckningen.
    let mut extra: Vec<ObjectId> = gone.iter().filter_map(|id| doc.get_dictionary(*id).ok()).filter_map(|d| d.get(b"Popup").ok()?.as_reference().ok()).collect();
    loop {
        gone.extend(extra.drain(..));
        for p in &pages {
            for o in page_refs(doc, *p) {
                let Object::Reference(id) = o else { continue };
                let Ok(d) = doc.get_dictionary(id) else { continue };
                let owner = |k: &[u8]| d.get(k).ok().and_then(|o| o.as_reference().ok());
                if !gone.contains(&id) && [b"IRT".as_slice(), b"Parent"].iter().any(|k| owner(k).is_some_and(|r| gone.contains(&r))) {
                    extra.push(id);
                }
            }
        }
        if extra.is_empty() {
            break;
        }
    }
    for p in pages {
        let keep = |arr: &mut Vec<Object>| arr.retain(|o| !matches!(o, Object::Reference(id) if gone.contains(id)));
        let loc = doc.get_dictionary(p).ok().and_then(|d| d.get(b"Annots").ok().cloned());
        match loc {
            Some(Object::Reference(r)) => {
                if let Ok(Object::Array(a)) = doc.get_object_mut(r) {
                    keep(a);
                }
            }
            Some(Object::Array(_)) => {
                if let Ok(Object::Array(a)) = doc.get_dictionary_mut(p).and_then(|d| d.get_mut(b"Annots")) {
                    keep(a);
                }
            }
            _ => {}
        }
    }
    for id in gone.iter() {
        doc.objects.remove(id);
    }
}

/// Utför ändringarna på dokumentet (se modulens översikt). Okända objekt hoppas över.
pub(super) fn edit_doc(doc: &mut Document, edits: &Edits) {
    if edits.is_empty() {
        return;
    }
    let all = read_doc(doc);
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let date = annot::pdf_date();
    for (id, e) in edits.iter().filter(|(_, e)| !e.deleted) {
        let Some(a) = all.iter().find(|a| a.id == *id) else { continue };
        let Some(&page) = pages.get(a.page) else { continue };
        let g = Geo::of(doc, page);
        let text = e.contents.clone().unwrap_or_else(|| a.contents.clone());
        let color = e.color.or(a.color).unwrap_or([0, 0, 0]);
        let (pw, ph) = {
            let (w, h) = (g.x1 - g.x0, g.y1 - g.y0);
            if g.rot % 180 == 90 { (h, w) } else { (w, h) }
        };
        let redraw = match a.kind {
            Kind::FreeText => e.color.is_some() || e.contents.is_some(),
            k => k.redrawable() && e.color.is_some(),
        };
        let fresh = if redraw { a.redraw_as(color, &text, (pw, ph)).and_then(|n| annot::parts(page, &g, &n, "", "")) } else { None };
        let ap_id = fresh.as_ref().map(|(_, ap)| ap.clone()).map(|ap| doc.add_object(ap));
        let Ok(d) = doc.get_dictionary_mut(*id) else { continue };
        if let Some(c) = e.color {
            d.set("C", annot::color_array(c));
        }
        if e.contents.is_some() {
            d.set("Contents", annot::text_string(&text));
            d.remove(b"RC");
        }
        d.set("M", Object::String(date.as_bytes().to_vec(), lopdf::StringFormat::Literal));
        if let (Some((nd, _)), Some(ap)) = (fresh, ap_id) {
            for k in ["Rect", "DA"] {
                if let Ok(v) = nd.get(k.as_bytes()) {
                    d.set(k, v.clone());
                }
            }
            d.set("AP", lopdf::dictionary! { "N" => ap });
        }
    }
    let mut gone: BTreeSet<ObjectId> = edits.iter().filter(|(_, e)| e.deleted).map(|(i, _)| *i).collect();
    if !gone.is_empty() {
        remove_annots(doc, &mut gone);
    }
}

// ============================================================== tillstånd och gränssnitt

/// Befintliga anteckningar i en flik: lästa ur PDF:en, väntande ändringar, urval och Ångra.
#[derive(Default)]
pub struct ExState {
    list: Vec<ExAnnot>,
    edits: Edits,
    /// Tidigare lägen av `edits` med antalet nya anteckningar då (för gemensam Ångra).
    undo: Vec<(Edits, usize)>,
    pub selected: Option<ObjectId>,
    /// Anteckna-läget är på (klick väljer anteckning).
    active: bool,
    password: Option<String>,
    /// Antal nya anteckningar just nu (hålls i takt av `AnnotState`).
    new_len: usize,
    buf: String,
    buf_for: Option<ObjectId>,
    typing: bool,
    stale: bool,
    /// Valda anteckningens skärmrektangel i senaste ritningen (för ruta intill).
    anchor: Cell<Option<Rect>>,
    pop_id: Option<egui::Id>,
}

impl ExState {
    /// Läser anteckningarna på nytt och släpper alla ändringar (efter öppning, sparning eller sidändring).
    pub fn load(&mut self, bytes: &[u8], password: Option<&str>) {
        *self = ExState { list: read(bytes, password), password: password.map(str::to_owned), active: self.active, ..Default::default() };
    }

    /// Släpper väntande ändringar utan att läsa om.
    pub fn clear(&mut self) {
        self.edits.clear();
        self.undo.clear();
        self.selected = None;
        self.buf_for = None;
        self.typing = false;
        self.stale = false;
    }

    pub fn set_active(&mut self, on: bool) {
        self.active = on;
        if !on {
            self.selected = None;
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn dirty(&self) -> bool {
        !self.edits.is_empty()
    }

    pub fn edits(&self) -> &Edits {
        &self.edits
    }

    pub fn edit_count(&self) -> usize {
        self.edits.len()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub fn all(&self) -> &[ExAnnot] {
        &self.list
    }

    fn is_deleted(&self, id: ObjectId) -> bool {
        self.edits.get(&id).is_some_and(|e| e.deleted)
    }

    /// Anteckningar som inte tagits bort.
    pub fn visible(&self) -> impl Iterator<Item = &ExAnnot> {
        self.list.iter().filter(|a| !self.is_deleted(a.id))
    }

    pub fn contents(&self, a: &ExAnnot) -> String {
        self.edits.get(&a.id).and_then(|e| e.contents.clone()).unwrap_or_else(|| a.contents.clone())
    }

    pub fn color(&self, a: &ExAnnot) -> Option<[u8; 3]> {
        self.edits.get(&a.id).and_then(|e| e.color).or(a.color)
    }

    /// Översta (minsta) anteckning under punkten på sidan.
    pub fn hit(&self, page: usize, pt: Pos2) -> Option<ObjectId> {
        // Bakifrån: vid lika stora träffar vinner den som ligger överst.
        self.list
            .iter()
            .rev()
            .filter(|a| !self.is_deleted(a.id) && a.page == page && a.hit(pt))
            .min_by(|a, b| (a.rect.area()).total_cmp(&b.rect.area()))
            .map(|a| a.id)
    }

    pub fn select(&mut self, id: ObjectId) {
        self.selected = Some(id);
        self.buf_for = None;
        self.typing = false;
    }

    pub fn deselect(&mut self) {
        self.selected = None;
        self.typing = false;
    }

    fn push_undo(&mut self) {
        self.undo.push((self.edits.clone(), self.new_len));
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
    }

    /// Tar bort en anteckning (Ångra-bart).
    pub fn delete(&mut self, id: ObjectId) {
        if !self.list.iter().any(|a| a.id == id) {
            return;
        }
        self.push_undo();
        self.edits.entry(id).or_default().deleted = true;
        if self.selected == Some(id) {
            self.deselect();
        }
        self.stale = true;
    }

    /// Högerklick med ett verktyg: tar bort befintlig anteckning under punkten.
    pub fn delete_at(&mut self, page: usize, pt: Pos2, new_len: usize) -> bool {
        self.new_len = new_len;
        match self.hit(page, pt) {
            Some(id) => {
                self.delete(id);
                true
            }
            None => false,
        }
    }

    pub fn set_color(&mut self, id: ObjectId, c: [u8; 3]) {
        let Some(a) = self.list.iter().find(|a| a.id == id) else { return };
        if self.color(a) == Some(c) {
            return;
        }
        let orig = a.color;
        self.push_undo();
        let e = self.edits.entry(id).or_default();
        e.color = (orig != Some(c)).then_some(c);
        self.tidy(id);
        self.stale = true;
    }

    /// Ny kommentartext. Ritas om (textrutor) först när skrivandet är klart.
    pub fn set_contents(&mut self, id: ObjectId, text: &str) {
        let Some(a) = self.list.iter().find(|a| a.id == id) else { return };
        let (same, kind) = (a.contents == text, a.kind);
        let e = self.edits.entry(id).or_default();
        e.contents = (!same).then(|| text.to_owned());
        self.tidy(id);
        if kind == Kind::FreeText {
            self.stale = true;
        }
    }

    fn tidy(&mut self, id: ObjectId) {
        if self.edits.get(&id).is_some_and(|e| *e == Edit::default()) {
            self.edits.remove(&id);
        }
    }

    /// Kan nästa Ångra göra ett steg här (inga nya anteckningar har lagts till efter det)?
    pub fn can_undo(&self, new_len: usize) -> bool {
        self.undo.last().is_some_and(|(_, n)| new_len <= *n)
    }

    pub fn undo(&mut self) -> bool {
        let Some((edits, _)) = self.undo.pop() else { return false };
        self.edits = edits;
        self.buf_for = None;
        self.typing = false;
        self.stale = true;
        if self.selected.is_some_and(|id| self.is_deleted(id)) {
            self.selected = None;
        }
        true
    }

    /// Behöver sidbilden ritas om (och skrivs inget just nu)? Nollställer.
    pub fn take_stale(&mut self) -> bool {
        let go = self.stale && !self.typing;
        if go {
            self.stale = false;
        }
        go
    }

    /// Verktygstips: (författare, datum, text) om det finns något att visa.
    pub fn tooltip(&self, id: ObjectId) -> Option<(String, String, String)> {
        let a = self.list.iter().find(|a| a.id == id)?;
        let text = self.contents(a);
        if a.author.is_empty() && text.trim().is_empty() {
            return None;
        }
        Some((a.author.clone(), show_date(&a.date), text))
    }

    /// Markeringsram runt vald anteckning (och minns var, så att rutan kan läggas intill).
    pub fn paint(&self, painter: &egui::Painter, page: usize, xf: &super::view::PageXf) {
        let Some(a) = self.selected.and_then(|id| self.visible().find(|a| a.id == id)).filter(|a| a.page == page) else { return };
        let r = xf.rect_to_screen(a.rect);
        painter.rect_stroke(r.expand(2.0), 2.0, Stroke::new(1.5, Color32::from_rgb(50, 110, 230)), egui::StrokeKind::Outside);
        self.anchor.set(Some(r));
    }

    /// Rutan intill vald anteckning: författare och datum, kommentar, färg, Ta bort.
    pub fn show_popover(&mut self, ctx: &egui::Context, new_len: usize) {
        self.new_len = new_len;
        let anchor = self.anchor.take();
        let Some(id) = self.selected else { return };
        let Some(a) = self.visible().find(|a| a.id == id).cloned() else {
            self.selected = None;
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.deselect();
            return;
        }
        let Some(anchor) = anchor else { return };
        if self.buf_for != Some(id) {
            self.buf = self.contents(&a);
            self.buf_for = Some(id);
            self.typing = false;
        }
        let pop_id = *self.pop_id.get_or_insert_with(|| egui::Id::unique("kladd-annot-popover"));
        let screen = ctx.content_rect();
        let (width, height) = (270.0, 215.0);
        let x = anchor.left().clamp(screen.left() + 4.0, (screen.right() - width - 8.0).max(screen.left() + 4.0));
        let y = if anchor.bottom() + 8.0 + height > screen.bottom() { (anchor.top() - 8.0 - height).max(screen.top() + 4.0) } else { anchor.bottom() + 8.0 };
        let (mut changed, mut lost, mut recolor, mut delete) = (false, false, None, false);
        let cur = self.color(&a);
        let mut buf = std::mem::take(&mut self.buf);
        egui::Area::new(pop_id).order(egui::Order::Foreground).fixed_pos(pos2(x, y)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(width - 20.0);
                ui.label(egui::RichText::new(a.kind.label()).strong());
                let who = if a.author.is_empty() { t("Okänd författare").to_owned() } else { a.author.clone() };
                let meta = if a.date.is_empty() { who } else { format!("{who} · {}", show_date(&a.date)) };
                ui.label(egui::RichText::new(meta).small().weak());
                ui.add_space(4.0);
                let edit = ui.add(egui::TextEdit::multiline(&mut buf).desired_width(f32::INFINITY).desired_rows(3).hint_text(t("Kommentar")));
                changed = edit.changed();
                lost = edit.lost_focus();
                if a.kind.redrawable() {
                    ui.add_space(4.0);
                    recolor = swatches(ui, a.kind, cur);
                }
                ui.add_space(4.0);
                let red = ui.visuals().error_fg_color;
                if ui.button(egui::RichText::new(t("Ta bort")).color(red)).clicked() {
                    delete = true;
                }
            });
        });
        self.buf = buf;
        if changed {
            if !self.typing {
                self.push_undo();
                self.typing = true;
            }
            let text = self.buf.clone();
            self.set_contents(id, &text);
        }
        if lost {
            self.typing = false;
        }
        if let Some(c) = recolor {
            self.set_color(id, c);
        }
        if delete {
            self.delete(id);
        }
    }
}

/// Färgrutor för en typ; ger vald färg när man klickar på en.
fn swatches(ui: &mut egui::Ui, kind: Kind, cur: Option<[u8; 3]>) -> Option<[u8; 3]> {
    let colors: &[(&str, [u8; 3])] = if matches!(kind, Kind::Highlight | Kind::Text) { &HIGHLIGHT_COLORS } else { &INK_COLORS };
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (name, c) in colors {
            let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::click());
            ui.painter().circle_filled(rect.center(), 7.0, Color32::from_rgb(c[0], c[1], c[2]));
            ui.painter().circle_stroke(rect.center(), 7.0, Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color));
            if cur == Some(*c) {
                ui.painter().circle_stroke(rect.center(), 9.0, Stroke::new(1.5, Color32::from_rgb(50, 110, 230)));
            }
            if resp.on_hover_text(t(name)).clicked() {
                pick = Some(*c);
            }
        }
    });
    pick
}

/// Innehållet i verktygstipset.
pub fn show_tooltip(ui: &mut egui::Ui, tip: &(String, String, String)) {
    ui.set_max_width(320.0);
    if !tip.0.is_empty() {
        let head = if tip.1.is_empty() { tip.0.clone() } else { format!("{} · {}", tip.0, tip.1) };
        ui.label(egui::RichText::new(head).strong());
    }
    if !tip.2.trim().is_empty() {
        let mut s: String = tip.2.chars().take(400).collect();
        if tip.2.chars().count() > 400 {
            s.push('…');
        }
        ui.label(s);
    }
}

// ============================================================== kommentarlistan

/// En rad i sidopanelens kommentarlista.
pub struct Row {
    pub id: Option<ObjectId>,
    pub page: usize,
    pub y: f32,
    pub author: String,
    pub kind: &'static str,
    pub text: String,
}

/// Alla anteckningar (befintliga och nya, ej borttagna) sorterade på sida och läge.
pub fn rows(st: &AnnotState) -> Vec<Row> {
    let mut v: Vec<Row> = st
        .ex
        .visible()
        .map(|a| Row { id: Some(a.id), page: a.page, y: a.rect.top(), author: a.author.clone(), kind: a.kind.label(), text: st.ex.contents(a) })
        .collect();
    let me = annot::author();
    for n in st.pending() {
        let (kind, text) = match &n.kind {
            NewKind::Highlight { .. } => (Kind::Highlight, String::new()),
            NewKind::Underline { .. } => (Kind::Underline, String::new()),
            NewKind::StrikeOut { .. } => (Kind::StrikeOut, String::new()),
            NewKind::Note { text, .. } => (Kind::Text, text.clone()),
            NewKind::FreeText { text, .. } => (Kind::FreeText, text.clone()),
            NewKind::Ink { .. } => (Kind::Ink, String::new()),
        };
        v.push(Row { id: None, page: n.page, y: n.bounds().top(), author: me.clone(), kind: kind.label(), text });
    }
    v.sort_by(|a, b| a.page.cmp(&b.page).then(a.y.total_cmp(&b.y)));
    v
}

/// "Kommentarer" i sidopanelen. Ger (sida, y i visningspunkter) att hoppa till vid klick.
pub fn comments_ui(ui: &mut egui::Ui, p: &crate::theme::Palette, st: &mut AnnotState) -> Option<(usize, f32)> {
    use egui::{FontId, Sense, text::LayoutJob};
    let rows = rows(st);
    if rows.is_empty() {
        ui.label(egui::RichText::new(t("Inga kommentarer")).color(p.muted));
        return None;
    }
    let mut jump = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let w = ui.available_width();
        for row in &rows {
            let head = if row.author.is_empty() {
                tf!("Sida {n} · {kind}", n = row.page + 1, kind = row.kind)
            } else {
                tf!("Sida {n} · {who}", n = row.page + 1, who = row.author)
            };
            let hg = ui.painter().layout_no_wrap(head, FontId::proportional(11.5), p.muted);
            let body = if row.text.trim().is_empty() { row.kind.to_owned() } else { row.text.clone() };
            let mut job = LayoutJob::single_section(body, egui::TextFormat::simple(FontId::proportional(13.0), if row.text.trim().is_empty() { p.muted } else { p.text }));
            job.wrap.max_width = w - 16.0;
            job.wrap.max_rows = 3;
            let bg = ui.painter().layout_job(job);
            let h = 8.0 + hg.size().y + 2.0 + bg.size().y + 8.0;
            let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
            let sel = row.id.is_some() && row.id == st.ex.selected;
            if sel {
                ui.painter().rect_filled(rect, 6.0, p.accent.gamma_multiply(0.16));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, p.surface_hover);
            }
            ui.painter().galley(rect.min + vec2(8.0, 8.0), hg.clone(), p.muted);
            ui.painter().galley(rect.min + vec2(8.0, 10.0 + hg.size().y), bg, p.text);
            if resp.clicked() {
                if let Some(id) = row.id {
                    st.ex.select(id);
                }
                jump = Some((row.page, row.y));
            }
        }
    });
    jump
}

// ============================================================== tester

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    fn base_pdf(rotate: Option<i64>, pages: usize) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        for _ in 0..pages {
            let content = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
            let mut page = dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => content,
                "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
            };
            if let Some(r) = rotate {
                page.set("Rotate", r);
            }
            kids.push(Object::Reference(doc.add_object(page)));
        }
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => pages as i64 }));
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", cat);
        let mut out = vec![];
        doc.save_to(&mut out).unwrap();
        out
    }

    fn all_kinds() -> Vec<NewAnnot> {
        let rc = Rect::from_min_size(pos2(20.0, 30.0), vec2(60.0, 12.0));
        vec![
            NewAnnot { page: 0, color: [255, 235, 59], kind: NewKind::Highlight { rects: vec![rc] } },
            NewAnnot { page: 0, color: [255, 0, 0], kind: NewKind::Underline { rects: vec![rc] } },
            NewAnnot { page: 0, color: [255, 0, 0], kind: NewKind::StrikeOut { rects: vec![rc] } },
            NewAnnot { page: 0, color: [255, 235, 59], kind: NewKind::Note { pos: pos2(50.0, 60.0), text: "Hej åäö".into() } },
            NewAnnot { page: 0, color: [0, 0, 0], kind: NewKind::FreeText { rect: Rect::from_min_size(pos2(10.0, 80.0), vec2(80.0, 40.0)), text: "Text i ruta".into() } },
            NewAnnot { page: 0, color: [0, 0, 255], kind: NewKind::Ink { strokes: vec![vec![pos2(5.0, 150.0), pos2(40.0, 190.0)]], width: 2.0 } },
        ]
    }

    fn with_annots(rotate: Option<i64>) -> Vec<u8> {
        annot::apply(&base_pdf(rotate, 2), None, &all_kinds()).unwrap()
    }

    #[test]
    fn laser_alla_typer() {
        for rot in [None, Some(90), Some(180), Some(270)] {
            let list = read(&with_annots(rot), None);
            let kinds: Vec<Kind> = list.iter().map(|a| a.kind).collect();
            assert_eq!(kinds, [Kind::Highlight, Kind::Underline, Kind::StrikeOut, Kind::Text, Kind::FreeText, Kind::Ink], "rot {rot:?}");
            assert!(list.iter().all(|a| a.page == 0 && !a.author.is_empty() && a.date.starts_with("D:")));
            let (hl, note, ink) = (&list[0], &list[3], &list[5]);
            assert_eq!(hl.color, Some([255, 235, 59]));
            assert_eq!(note.contents, "Hej åäö");
            // Samma visningsrum som de nya anteckningarna skrevs i.
            let want = Rect::from_min_size(pos2(20.0, 30.0), vec2(60.0, 12.0));
            assert!((hl.rect.min - want.min).length() < 0.1 && (hl.rect.max - want.max).length() < 0.1, "rot {rot:?}: {:?}", hl.rect);
            assert_eq!(hl.quads.len(), 1);
            assert!((note.rect.min - pos2(50.0, 60.0)).length() < 0.1);
            assert!((ink.strokes[0][1] - pos2(40.0, 190.0)).length() < 0.1 && (ink.width - 2.0).abs() < 1e-3);
            assert_eq!(list[4].color, Some([0, 0, 0]));
        }
    }

    #[test]
    fn traffytor() {
        let list = read(&with_annots(Some(90)), None);
        let mut st = ExState::default();
        st.list = list;
        assert_eq!(st.hit(0, pos2(50.0, 36.0)).map(|i| st.list.iter().position(|a| a.id == i).unwrap()), Some(2)); // översta raden = genomstruken
        assert!(st.hit(0, pos2(150.0, 250.0)).is_none());
        assert!(st.hit(1, pos2(50.0, 36.0)).is_none());
        // Pennan: nära strecket men inte långt ifrån.
        let ink = st.list[5].id;
        assert_eq!(st.hit(0, pos2(22.0, 170.0)), Some(ink));
        assert!(st.hit(0, pos2(60.0, 170.0)).is_none());
    }

    /// Öppnas resultatet av visarens motor?
    fn opens(bytes: Vec<u8>) -> bool {
        crate::pdfview::engine::PdfEngine::open(std::sync::Arc::new(bytes), None, egui::Context::default()).is_ok()
    }

    fn id_of(list: &[ExAnnot], k: Kind) -> ObjectId {
        list.iter().find(|a| a.kind == k).unwrap().id
    }

    #[test]
    fn ta_bort_tar_med_popup_och_svar() {
        let bytes = with_annots(None);
        let list = read(&bytes, None);
        // Lägg en popup och ett svar på anteckningen.
        let mut doc = Document::load_mem(&bytes).unwrap();
        let note = id_of(&list, Kind::Text);
        let popup = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Popup", "Parent" => note, "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()] });
        let reply = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Text", "IRT" => note, "Contents" => Object::string_literal("svar"), "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()] });
        doc.get_dictionary_mut(note).unwrap().set("Popup", popup);
        let page = *doc.get_pages().values().next().unwrap();
        if let Ok(Object::Array(a)) = doc.get_dictionary_mut(page).unwrap().get_mut(b"Annots") {
            a.push(popup.into());
            a.push(reply.into());
        }
        let mut b = vec![];
        doc.save_to(&mut b).unwrap();
        assert_eq!(read(&b, None).len(), 7); // popup hoppas över
        let edits: Edits = [(note, Edit { deleted: true, ..Default::default() })].into();
        let out = annot::apply_all(&b, None, &[], &edits).unwrap();
        let after = read(&out, None);
        assert_eq!(after.len(), 5);
        assert!(after.iter().all(|a| a.id != note && a.id != reply));
        let d = Document::load_mem(&out).unwrap();
        assert!(d.get_object(popup).is_err() && d.get_object(note).is_err());
        // Öppnas av motorn.
        assert!(opens(out));
    }

    #[test]
    fn andra_text_och_farg() {
        let bytes = with_annots(None);
        let list = read(&bytes, None);
        let (hl, note, ft, ink) = (id_of(&list, Kind::Highlight), id_of(&list, Kind::Text), id_of(&list, Kind::FreeText), id_of(&list, Kind::Ink));
        let mut edits = Edits::new();
        edits.insert(hl, Edit { color: Some([118, 214, 114]), ..Default::default() });
        edits.insert(note, Edit { contents: Some("Ändrad (text)".into()), ..Default::default() });
        edits.insert(ft, Edit { contents: Some("Mycket längre text i rutan som måste radbrytas flera gånger".into()), color: Some([213, 38, 38]), ..Default::default() });
        edits.insert(ink, Edit { color: Some([20, 20, 20]), ..Default::default() });
        let out = annot::apply_all(&bytes, None, &[], &edits).unwrap();
        let after = read(&out, None);
        assert_eq!(after.len(), 6);
        let get = |k| after.iter().find(|a| a.kind == k).unwrap();
        assert_eq!(get(Kind::Highlight).color, Some([118, 214, 114]));
        assert_eq!(get(Kind::Text).contents, "Ändrad (text)");
        assert_eq!(get(Kind::FreeText).color, Some([213, 38, 38]));
        assert!(get(Kind::FreeText).rect.height() > list.iter().find(|a| a.kind == Kind::FreeText).unwrap().rect.height());
        assert_eq!(get(Kind::Ink).color, Some([20, 20, 20]));
        // Utseendet är nytt (innehåller nya färgen) och datum satt.
        let doc = Document::load_mem(&out).unwrap();
        let ap = |id: ObjectId| {
            let ap = doc.get_dictionary(id).unwrap().get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
            String::from_utf8_lossy(&doc.get_object(ap).unwrap().as_stream().unwrap().content).into_owned()
        };
        assert!(ap(hl).contains("0.463 0.839 0.447 rg"), "{}", ap(hl));
        assert!(ap(ft).contains("0.835 0.149 0.149 rg"));
        assert!(ap(ft).contains("radbrytas"));
        // Anteckningens utseende rörs inte av ny text.
        let before = Document::load_mem(&bytes).unwrap();
        let ap_before = before.get_dictionary(note).unwrap().get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
        assert_eq!(ap(note), String::from_utf8_lossy(&before.get_object(ap_before).unwrap().as_stream().unwrap().content));
        let m = |k| after.iter().find(|a| a.kind == k).unwrap().date.clone();
        assert_ne!(m(Kind::Text), "");
        assert!(opens(out));
    }

    #[test]
    fn okanda_typer_behaller_utseendet() {
        let bytes = base_pdf(None, 1);
        let mut doc = Document::load_mem(&bytes).unwrap();
        let ap = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 50.into(), 20.into()] }, b"1 0 0 rg 0 0 50 20 re f".to_vec()));
        let sq = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Square", "Rect" => vec![10.into(), 10.into(), 60.into(), 30.into()],
            "C" => vec![Object::Real(1.0), Object::Real(0.0), Object::Real(0.0)], "T" => Object::string_literal("Anna"),
            "Contents" => Object::string_literal("gammal"), "AP" => dictionary! { "N" => ap },
        });
        let link = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![0.into(), 0.into(), 5.into(), 5.into()] });
        let page = *doc.get_pages().values().next().unwrap();
        doc.get_dictionary_mut(page).unwrap().set("Annots", vec![Object::Reference(sq), Object::Reference(link)]);
        let mut b = vec![];
        doc.save_to(&mut b).unwrap();
        let list = read(&b, None);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].kind, list[0].author.as_str(), list[0].contents.as_str()), (Kind::Square, "Anna", "gammal"));
        assert_eq!(list[0].color, Some([255, 0, 0]));
        let edits: Edits = [(sq, Edit { contents: Some("ny".into()), color: Some([0, 0, 255]), ..Default::default() })].into();
        let out = annot::apply_all(&b, None, &[], &edits).unwrap();
        let d = Document::load_mem(&out).unwrap();
        let dict = d.get_dictionary(sq).unwrap();
        assert_eq!(dict.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap(), ap);
        let after = read(&out, None);
        assert_eq!((after[0].contents.as_str(), after[0].color), ("ny", Some([0, 0, 255])));
        // Länken finns kvar.
        assert_eq!(page_refs(&d, page).len(), 2);
    }

    #[test]
    fn tillstand_ingen_andring_och_angra() {
        let bytes = with_annots(None);
        let mut st = ExState::default();
        st.load(&bytes, None);
        assert!(!st.dirty());
        let (hl, note) = (id_of(st.all(), Kind::Highlight), id_of(st.all(), Kind::Text));
        st.set_contents(note, "ny");
        assert!(st.dirty());
        st.set_contents(note, "Hej åäö"); // tillbaka till originalet
        assert!(!st.dirty());
        st.set_color(hl, [1, 2, 3]);
        st.delete(note);
        assert_eq!(st.edit_count(), 2);
        assert_eq!(st.visible().count(), 5);
        assert!(st.can_undo(0) && st.can_undo(0));
        assert!(st.undo());
        assert_eq!(st.visible().count(), 6);
        assert!(st.undo());
        assert!(!st.dirty() && !st.undo());
        // Nya anteckningar efter en ändring ångras först.
        st.delete(hl);
        assert!(st.can_undo(0) && !st.can_undo(1));
    }

    #[test]
    fn annotstate_gemensam_angra_och_osparat() {
        let bytes = with_annots(None);
        let mut a = AnnotState::default();
        a.ex.load(&bytes, None);
        assert!(!a.has_pending());
        let note = id_of(a.ex.all(), Kind::Text);
        a.ex.delete(note);
        assert!(a.has_pending());
        a.add(NewAnnot { page: 0, color: [0; 3], kind: NewKind::Note { pos: pos2(1.0, 1.0), text: "x".into() } });
        assert!(a.undo()); // den nya först
        assert!(a.has_pending() && a.pending().is_empty());
        assert!(a.undo());
        assert!(!a.has_pending());
        a.ex.delete(note);
        a.clear();
        assert!(!a.has_pending());
    }

    #[test]
    fn datum_text_och_farger() {
        assert_eq!(show_date("D:20240131123456+01'00'"), "2024-01-31 12:34");
        assert_eq!(show_date("D:20240131"), "2024-01-31");
        assert_eq!(show_date("igår"), "igår");
        assert_eq!(decode_text(&[0xFE, 0xFF, 0x00, 0x48, 0x00, 0xE5]), "Hå");
        assert_eq!(decode_text(&[0x48, 0xE5]), "Hå");
        assert_eq!(da_color("/Helv 12 Tf 1 0 0 rg"), Some([255, 0, 0]));
        assert_eq!(da_color("0.5 g /Helv 9 Tf"), Some([128, 128, 128]));
        assert_eq!(da_color("/Helv 12 Tf"), None);
        assert_eq!(color_of(&[0.0, 0.0, 0.0, 1.0]), Some([0, 0, 0]));
        assert_eq!(color_of(&[]), None);
    }

    #[test]
    fn rader_sorteras_och_inkluderar_nya() {
        let mut a = AnnotState::default();
        a.ex.load(&with_annots(None), None);
        a.add(NewAnnot { page: 0, color: [0; 3], kind: NewKind::Note { pos: pos2(1.0, 1.0), text: "ny".into() } });
        let r = rows(&a);
        assert_eq!(r.len(), 7);
        assert!(r.windows(2).all(|w| (w[0].page, w[0].y) <= (w[1].page, w[1].y)));
        assert!(r.iter().any(|x| x.id.is_none() && x.text == "ny"));
    }
}
