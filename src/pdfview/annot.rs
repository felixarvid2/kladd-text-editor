//! Anteckningar i PDF-visaren (som Okular och pdf.js): överstrykning, understruken, genomstruken,
//! anteckningslapp, textruta och penna.
//!
//! Nya anteckningar hålls i minnet per sida i *visningspunkter* (samma rum som `PageText`: origo
//! uppe till vänster i sidans beskärningsruta efter sidans `/Rotate`, före användarens extra
//! rotation i vyn). Först vid sparande skrivs de in i PDF:en med `apply`: riktiga `/Annots`-poster
//! med `/AP`-utseende, så att alla läsare (och hayro) visar dem.

use std::collections::BTreeMap;

use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};

use super::annot_edit::Edits;
use super::meta::Geo;
use super::view::PageXf;
use crate::i18n::t;
use crate::tf;

/// Markeringsfärger: gul, grön, blå, rosa.
pub const HIGHLIGHT_COLORS: [(&str, [u8; 3]); 4] =
    [("Gul", [255, 235, 59]), ("Grön", [118, 214, 114]), ("Blå", [100, 181, 246]), ("Rosa", [244, 143, 177])];
/// Färger för understruken, genomstruken, penna och text.
pub const INK_COLORS: [(&str, [u8; 3]); 5] =
    [("Svart", [20, 20, 20]), ("Röd", [213, 38, 38]), ("Blå", [30, 90, 200]), ("Grön", [30, 140, 60]), ("Orange", [235, 130, 20])];

/// Anteckningslappens storlek och textrutans bredd och teckenstorlek (pt).
pub(super) const NOTE_SIZE: f32 = 18.0;
pub(super) const FREE_WIDTH: f32 = 220.0;
pub(super) const FREE_SIZE: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnnotTool {
    Highlight,
    Underline,
    StrikeOut,
    Note,
    FreeText,
    Pen,
}

impl AnnotTool {
    /// Verktyg som skapas av en textmarkering (vid musknapp upp).
    pub fn is_markup(self) -> bool {
        matches!(self, Self::Highlight | Self::Underline | Self::StrikeOut)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NewKind {
    Highlight { rects: Vec<Rect> },
    Underline { rects: Vec<Rect> },
    StrikeOut { rects: Vec<Rect> },
    /// Lappens övre vänstra hörn.
    Note { pos: Pos2, text: String },
    FreeText { rect: Rect, text: String },
    /// Ett eller flera streck (punkter i visningspunkter) och bredd i pt.
    Ink { strokes: Vec<Vec<Pos2>>, width: f32 },
}

/// En ny anteckning på en sida (0-baserad) i visningspunkter.
#[derive(Clone, Debug, PartialEq)]
pub struct NewAnnot {
    pub page: usize,
    pub color: [u8; 3],
    pub kind: NewKind,
}

impl NewAnnot {
    /// Omslutande rektangel i visningspunkter (för träff vid radering).
    pub fn bounds(&self) -> Rect {
        let union = |rs: &[Rect]| rs.iter().fold(Rect::NOTHING, |a, r| a.union(*r));
        match &self.kind {
            NewKind::Highlight { rects } | NewKind::Underline { rects } | NewKind::StrikeOut { rects } => union(rects),
            NewKind::Note { pos, .. } => Rect::from_min_size(*pos, vec2(NOTE_SIZE, NOTE_SIZE)),
            NewKind::FreeText { rect, .. } => *rect,
            NewKind::Ink { strokes, width } => {
                let mut r = Rect::NOTHING;
                for p in strokes.iter().flatten() {
                    r.extend_with(*p);
                }
                r.expand(width / 2.0 + 2.0)
            }
        }
    }
}

struct Editing {
    tool: AnnotTool,
    page: usize,
    pt: Pos2,
    screen: Pos2,
    text: String,
    fresh: bool,
}

/// Anteckningsläget i en PDF-flik: valt verktyg, väntande anteckningar och pågående inmatning.
pub struct AnnotState {
    pub tool: Option<AnnotTool>,
    pub hl_color: usize,
    pub ink_color: usize,
    pub width: f32,
    list: Vec<NewAnnot>,
    stroke: Option<(usize, Vec<Pos2>)>,
    editing: Option<Editing>,
    editor_id: egui::Id,
    /// Anteckningar som redan finns i PDF:en: ändringar, urval och ruta (annot_edit.rs).
    pub ex: super::annot_edit::ExState,
}

impl Default for AnnotState {
    fn default() -> Self {
        Self { tool: None, hl_color: 0, ink_color: 1, width: 2.0, list: Vec::new(), stroke: None, editing: None, editor_id: egui::Id::unique("kladd-annot-editor"), ex: Default::default() }
    }
}

impl AnnotState {
    /// Finns anteckningar som inte sparats än (då är fliken "ändrad")?
    pub fn has_pending(&self) -> bool {
        !self.list.is_empty() || self.ex.dirty()
    }

    pub fn pending(&self) -> &[NewAnnot] {
        &self.list
    }

    pub fn clear(&mut self) {
        self.list.clear();
        self.stroke = None;
        self.editing = None;
        self.ex.clear();
    }

    pub fn add(&mut self, a: NewAnnot) {
        self.list.push(a);
    }

    /// Ångra: tar bort den senast tillagda eller ändrade (nya och befintliga i den ordning de gjordes).
    /// Sant om något ångrades.
    pub fn undo(&mut self) -> bool {
        if self.ex.can_undo(self.list.len()) && self.ex.undo() {
            return true;
        }
        self.list.pop().is_some()
    }

    /// Tar bort den översta väntande anteckningen under punkten.
    pub fn delete_at(&mut self, page: usize, pt: Pos2) -> bool {
        match self.list.iter().rposition(|a| a.page == page && a.bounds().expand(2.0).contains(pt)) {
            Some(i) => {
                self.list.remove(i);
                true
            }
            None => false,
        }
    }

    fn color(&self, tool: AnnotTool) -> [u8; 3] {
        match tool {
            AnnotTool::Highlight => HIGHLIGHT_COLORS[self.hl_color.min(3)].1,
            AnnotTool::Note => HIGHLIGHT_COLORS[0].1,
            _ => INK_COLORS[self.ink_color.min(4)].1,
        }
    }

    /// Lägger en markering på varje sida den täcker (rader per sida kommer från `PageText::rects`).
    pub fn add_markup(&mut self, tool: AnnotTool, per_page: Vec<(usize, Vec<Rect>)>) {
        let color = self.color(tool);
        for (page, rects) in per_page.into_iter().filter(|(_, r)| !r.is_empty()) {
            let kind = match tool {
                AnnotTool::Highlight => NewKind::Highlight { rects },
                AnnotTool::Underline => NewKind::Underline { rects },
                AnnotTool::StrikeOut => NewKind::StrikeOut { rects },
                _ => continue,
            };
            self.list.push(NewAnnot { page, color, kind });
        }
    }

    /// Hanterar musen för lapp, textruta och penna samt högerklick = radera. Sant när verktyget
    /// har hand om musen (visaren ska då inte markera text).
    /// `hit(skärmpunkt, närmaste)` ger sida och punkt i visningspunkter, `size(sida)` sidans storlek.
    pub fn pointer(
        &mut self,
        ctx: &egui::Context,
        resp: &egui::Response,
        hit: &dyn Fn(Pos2, bool) -> Option<(usize, Pos2)>,
        size: &dyn Fn(usize) -> (f32, f32),
    ) -> bool {
        let Some(tool) = self.tool else { return false };
        if resp.secondary_clicked() {
            if let Some((page, pt)) = resp.interact_pointer_pos().and_then(|p| hit(p, false)) {
                if !self.delete_at(page, pt) {
                    self.ex.delete_at(page, pt, self.list.len());
                }
            }
        }
        if tool.is_markup() {
            return false;
        }
        if self.editing.is_some() {
            return true;
        }
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        match tool {
            AnnotTool::Pen => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    self.stroke = ctx
                        .input(|i| i.pointer.press_origin())
                        .and_then(|o| hit(o, false))
                        .map(|(page, pt)| (page, vec![pt]));
                }
                if resp.dragged_by(egui::PointerButton::Primary) {
                    if let (Some((page, pts)), Some(pos)) = (self.stroke.as_mut(), ctx.pointer_latest_pos()) {
                        if let Some((p2, pt)) = hit(pos, true) {
                            if p2 == *page && pts.last().is_none_or(|l| l.distance(pt) > 0.4) {
                                pts.push(pt);
                            }
                        }
                    }
                }
                if resp.drag_stopped() {
                    if let Some((page, mut pts)) = self.stroke.take() {
                        if pts.len() == 1 {
                            pts.push(pts[0]);
                        }
                        let color = self.color(tool);
                        self.list.push(NewAnnot { page, color, kind: NewKind::Ink { strokes: vec![pts], width: self.width } });
                    }
                }
            }
            AnnotTool::Note | AnnotTool::FreeText => {
                if resp.clicked_by(egui::PointerButton::Primary) {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if let Some((page, pt)) = hit(pos, false) {
                            let (w, h) = size(page);
                            let pt = pos2(pt.x.clamp(0.0, w), pt.y.clamp(0.0, h));
                            self.editing = Some(Editing { tool, page, pt, screen: pos, text: String::new(), fresh: true });
                        }
                    }
                }
            }
            _ => {}
        }
        true
    }

    /// Inmatningsrutan för anteckningslapp och textruta.
    pub fn show_editor(&mut self, ctx: &egui::Context, size: &dyn Fn(usize) -> (f32, f32)) {
        let Some(ed) = self.editing.as_mut() else { return };
        let mut done = None;
        egui::Area::new(self.editor_id)
            .order(egui::Order::Foreground)
            .fixed_pos(ed.screen + vec2(6.0, 6.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(240.0);
                    let title = if ed.tool == AnnotTool::Note { t("Anteckning") } else { t("Textruta") };
                    ui.label(egui::RichText::new(title).strong());
                    let edit = ui.add(egui::TextEdit::multiline(&mut ed.text).desired_width(f32::INFINITY).desired_rows(3));
                    if ed.fresh {
                        edit.request_focus();
                        ed.fresh = false;
                    }
                    ui.horizontal(|ui| {
                        if ui.button(t("Lägg till")).clicked()
                            || ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
                        {
                            done = Some(true);
                        }
                        if ui.button(t("Avbryt")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            done = Some(false);
                        }
                    });
                });
            });
        let Some(ok) = done else { return };
        let ed = self.editing.take().expect("inmatning pågår");
        let text = ed.text.trim_end().to_owned();
        if !ok || text.trim().is_empty() {
            return;
        }
        let (pw, ph) = size(ed.page);
        let color = self.color(ed.tool);
        let kind = if ed.tool == AnnotTool::Note {
            let pos = pos2(ed.pt.x.min(pw - NOTE_SIZE).max(0.0), ed.pt.y.min(ph - NOTE_SIZE).max(0.0));
            NewKind::Note { pos, text }
        } else {
            NewKind::FreeText { rect: freetext_rect(ed.pt, &text, pw, ph), text }
        };
        self.list.push(NewAnnot { page: ed.page, color, kind });
    }

    /// Ritar väntande anteckningar och pågående streck på sidan `page`.
    pub fn paint(&self, painter: &egui::Painter, page: usize, xf: &PageXf) {
        let col = |c: [u8; 3], a: u8| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], a);
        let line = |painter: &egui::Painter, pts: &[Pos2], w: f32, c: Color32| {
            let pts: Vec<Pos2> = pts.iter().map(|p| xf.to_screen(*p)).collect();
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts, Stroke::new((w * xf.k).max(1.0), c)));
            }
        };
        for a in self.list.iter().filter(|a| a.page == page) {
            match &a.kind {
                NewKind::Highlight { rects } => {
                    for r in rects {
                        painter.rect_filled(xf.rect_to_screen(*r), 1.0, col(a.color, 110));
                    }
                }
                NewKind::Underline { rects } | NewKind::StrikeOut { rects } => {
                    let under = matches!(a.kind, NewKind::Underline { .. });
                    for r in rects {
                        let y = if under { r.bottom() - 1.0 } else { r.center().y };
                        line(painter, &[pos2(r.left(), y), pos2(r.right(), y)], markup_width(*r), col(a.color, 255));
                    }
                }
                NewKind::Note { pos, .. } => {
                    let r = xf.rect_to_screen(Rect::from_min_size(*pos, vec2(NOTE_SIZE, NOTE_SIZE)));
                    painter.rect_filled(r, 3.0, col([255, 235, 59], 255));
                    painter.rect_stroke(r, 3.0, Stroke::new(1.0, Color32::from_rgb(120, 100, 0)), egui::StrokeKind::Inside);
                    for i in 0..3 {
                        let y = r.top() + r.height() * (0.3 + 0.2 * i as f32);
                        painter.hline((r.left() + r.width() * 0.2)..=(r.right() - r.width() * 0.2), y, Stroke::new(1.0, Color32::from_rgb(120, 100, 0)));
                    }
                }
                NewKind::FreeText { rect, text } => {
                    let r = xf.rect_to_screen(*rect);
                    let font = egui::FontId::proportional(FREE_SIZE * xf.k);
                    for (i, l) in layout_text(text, FREE_SIZE, rect.width() - 4.0).iter().enumerate() {
                        let y = r.top() + (2.0 + i as f32 * FREE_SIZE * 1.2) * xf.k;
                        painter.text(pos2(r.left() + 2.0 * xf.k, y), egui::Align2::LEFT_TOP, l, font.clone(), col(a.color, 255));
                    }
                }
                NewKind::Ink { strokes, width } => {
                    for s in strokes {
                        line(painter, s, *width, col(a.color, 255));
                    }
                }
            }
        }
        if let (Some((p, pts)), Some(tool)) = (&self.stroke, self.tool) {
            if *p == page {
                line(painter, pts, self.width, col(self.color(tool), 255));
            }
        }
    }
}

/// Linjetjocklek för understruken/genomstruken efter radens höjd.
pub(super) fn markup_width(r: Rect) -> f32 {
    (r.height() * 0.07).clamp(0.6, 1.5)
}

/// Textrutans rektangel (visningspunkter) för text som skrivs vid `pt`.
pub fn freetext_rect(pt: Pos2, text: &str, page_w: f32, page_h: f32) -> Rect {
    let w = FREE_WIDTH.min((page_w - pt.x - 4.0).max(60.0));
    let lines = layout_text(text, FREE_SIZE, w - 4.0).len().max(1) as f32;
    let h = 4.0 + lines * FREE_SIZE * 1.2;
    let x = pt.x.min((page_w - w).max(0.0)).max(0.0);
    let y = pt.y.min((page_h - h).max(0.0)).max(0.0);
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

/// Teckenbredd i Helvetica (1/1000 em); övrigt (åäö m.m.) räknas som 556.
fn helv_width(c: char) -> f32 {
    const W: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278,
        278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611,
        722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
        556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
    ];
    match c as u32 {
        32..=126 => W[c as usize - 32] as f32,
        _ => 556.0,
    }
}

/// Radbryter `text` på ordgränser så att raderna ryms i `width` pt (Helvetica i storlek `size`).
pub fn layout_text(text: &str, size: f32, width: f32) -> Vec<String> {
    let w = |s: &str| s.chars().map(helv_width).sum::<f32>() * size / 1000.0;
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if w(&cand) <= width || line.is_empty() && word.is_empty() {
                line = cand;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            // Ett ord som är bredare än raden bryts på tecken.
            let mut cur = String::new();
            for ch in word.chars() {
                let mut t = cur.clone();
                t.push(ch);
                if w(&t) > width && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                cur.push(ch);
            }
            line = cur;
        }
        out.push(line);
    }
    out
}

// ============================================================== verktygsraden

/// Verktygsraden under PDF-verktygsraden: Markera, Understruken, Genomstruken, Anteckning, Textruta,
/// Penna och Ångra, med färgval.
pub fn tool_row(ui: &mut egui::Ui, p: &crate::theme::Palette, st: &mut AnnotState) {
    ui.spacing_mut().item_spacing.x = 4.0;
    let tools = [
        (AnnotTool::Highlight, "Markera", "Överstryk markerad text"),
        (AnnotTool::Underline, "Understruken", "Understryk markerad text"),
        (AnnotTool::StrikeOut, "Genomstruken", "Stryk över markerad text"),
        (AnnotTool::Note, "Anteckning", "Klicka på sidan och skriv en anteckning"),
        (AnnotTool::FreeText, "Textruta", "Klicka på sidan och skriv en textruta"),
        (AnnotTool::Pen, "Penna", "Rita fritt (dra); högerklick tar bort en anteckning"),
    ];
    for (tool, label, tip) in tools {
        let on = st.tool == Some(tool);
        let text = egui::RichText::new(t(label)).color(if on { p.accent } else { p.text });
        let b = egui::Button::new(text).fill(if on { p.accent.gamma_multiply(0.16) } else { Color32::TRANSPARENT }).stroke(Stroke::NONE);
        if ui.add(b).on_hover_text(t(tip)).clicked() {
            st.tool = if on { None } else { Some(tool) };
            st.editing = None;
        }
    }
    ui.separator();
    let (colors, sel): (&[(&'static str, [u8; 3])], &mut usize) = if st.tool == Some(AnnotTool::Highlight) {
        (&HIGHLIGHT_COLORS, &mut st.hl_color)
    } else {
        (&INK_COLORS, &mut st.ink_color)
    };
    for (i, (name, c)) in colors.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
        ui.painter().circle_filled(rect.center(), 7.0, Color32::from_rgb(c[0], c[1], c[2]));
        ui.painter().circle_stroke(rect.center(), 7.0, Stroke::new(1.0, p.line));
        if *sel == i {
            ui.painter().circle_stroke(rect.center(), 9.0, Stroke::new(1.5, p.accent));
        }
        if resp.on_hover_text(t(*name)).clicked() {
            *sel = i;
        }
    }
    if st.tool == Some(AnnotTool::Pen) {
        ui.separator();
        ui.label(egui::RichText::new(t("Bredd")).color(p.muted));
        ui.add(egui::DragValue::new(&mut st.width).range(0.5..=12.0).speed(0.1).suffix(" pt"));
    }
    ui.separator();
    if ui.add_enabled(st.has_pending(), egui::Button::new(t("Ångra")).fill(Color32::TRANSPARENT).stroke(Stroke::NONE)).on_hover_text(t("Ta bort den senaste anteckningen")).clicked() {
        st.undo();
    }
    if st.has_pending() {
        ui.label(egui::RichText::new(tf!("{n} osparade", n = st.list.len() + st.ex.edit_count())).color(p.muted));
    }
}

// ============================================================== skriva till PDF

fn r(v: f32) -> Object {
    Object::Real(v)
}

pub(super) fn real_array(v: &[f32]) -> Object {
    Object::Array(v.iter().map(|x| r(*x)).collect())
}

pub(super) fn color_array(c: [u8; 3]) -> Object {
    real_array(&[c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0])
}

fn rgb_ops(c: [u8; 3]) -> String {
    format!("{:.3} {:.3} {:.3}", c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0)
}

/// PDF-textsträng: ASCII som den är, annars UTF-16BE med BOM.
pub(super) fn text_string(s: &str) -> Object {
    let bytes = if s.is_ascii() {
        s.as_bytes().to_vec()
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        b
    };
    Object::String(bytes, StringFormat::Literal)
}

/// Text i innehållsström (WinAnsi ≈ Latin-1; övriga tecken blir `?`).
pub(super) fn content_string(s: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for ch in s.chars() {
        let b = match ch as u32 {
            0x20..=0x7E | 0xA0..=0xFF => ch as u8,
            _ => b'?',
        };
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
    out
}

pub(super) fn author() -> String {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "Kladd".to_owned())
}

pub(super) fn pdf_date() -> String {
    chrono::Utc::now().format("D:%Y%m%d%H%M%SZ").to_string()
}

/// Visningspunkter → användarrymd (invers av `Geo::point`).
pub(super) fn to_user(g: &Geo, p: Pos2) -> (f32, f32) {
    let (w, h) = (g.x1 - g.x0, g.y1 - g.y0);
    let (u, v) = match g.rot {
        90 => (p.y, h - p.x),
        180 => (w - p.x, h - p.y),
        270 => (w - p.y, p.x),
        _ => (p.x, p.y),
    };
    (g.x0 + u, g.y1 - v)
}

/// Rektangel i visningspunkter → (l, b, r, t) i användarrymd.
pub(super) fn user_rect(g: &Geo, rc: Rect) -> [f32; 4] {
    let a = to_user(g, rc.min);
    let b = to_user(g, rc.max);
    [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
}

fn form(bbox: [f32; 4], matrix: Option<[f32; 6]>, resources: Dictionary, content: Vec<u8>) -> Stream {
    let mut d = dictionary! {
        "Type" => "XObject", "Subtype" => "Form",
        "BBox" => real_array(&bbox),
        "Resources" => resources,
    };
    if let Some(m) = matrix {
        d.set("Matrix", real_array(&m));
    }
    Stream::new(d, content)
}

fn n(v: f32) -> String {
    format!("{v:.2}")
}

/// Bygger `/Annots`-posten (och dess utseende) för en anteckning; `geo` är sidans geometri.
fn build(doc: &mut Document, page: ObjectId, g: &Geo, a: &NewAnnot, who: &str, date: &str) -> Option<ObjectId> {
    let (mut d, ap) = parts(page, g, a, who, date)?;
    let ap_id = doc.add_object(ap);
    d.set("AP", dictionary! { "N" => ap_id });
    Some(doc.add_object(d))
}

/// Postens ordbok (utan `/AP`) och utseendeströmmen för en anteckning. Används också när en
/// befintlig anteckning ritas om efter ändrad färg eller text (`annot_edit`).
pub(super) fn parts(page: ObjectId, g: &Geo, a: &NewAnnot, who: &str, date: &str) -> Option<(Dictionary, Stream)> {
    let mut d = dictionary! {
        "Type" => "Annot",
        "P" => page,
        "F" => 4,
        "C" => color_array(a.color),
        "T" => text_string(who),
        "M" => Object::String(date.as_bytes().to_vec(), StringFormat::Literal),
        "CreationDate" => Object::String(date.as_bytes().to_vec(), StringFormat::Literal),
    };
    let none = Dictionary::new;
    let (rect, ap): ([f32; 4], Stream) = match &a.kind {
        NewKind::Highlight { rects } | NewKind::Underline { rects } | NewKind::StrikeOut { rects } => {
            if rects.is_empty() {
                return None;
            }
            let kind = match a.kind {
                NewKind::Highlight { .. } => "Highlight",
                NewKind::Underline { .. } => "Underline",
                _ => "StrikeOut",
            };
            d.set("Subtype", kind);
            let mut quads = Vec::new();
            let mut all = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            let mut content = String::new();
            let rgb = rgb_ops(a.color);
            match a.kind {
                NewKind::Highlight { .. } => content.push_str(&format!("/GS gs {rgb} rg\n")),
                _ => content.push_str(&format!("{rgb} RG 1 J\n")),
            }
            for rc in rects {
                let [l, b, rr, t] = user_rect(g, *rc);
                quads.extend_from_slice(&[l, t, rr, t, l, b, rr, b]);
                all = [all[0].min(l), all[1].min(b), all[2].max(rr), all[3].max(t)];
                match a.kind {
                    NewKind::Highlight { .. } => content.push_str(&format!("{} {} m {} {} l {} {} l {} {} l h f\n", n(l), n(t), n(rr), n(t), n(rr), n(b), n(l), n(b))),
                    _ => {
                        let y = if matches!(a.kind, NewKind::Underline { .. }) { rc.bottom() - 1.0 } else { rc.center().y };
                        let p1 = to_user(g, pos2(rc.left(), y));
                        let p2 = to_user(g, pos2(rc.right(), y));
                        content.push_str(&format!("{} w {} {} m {} {} l S\n", n(markup_width(*rc)), n(p1.0), n(p1.1), n(p2.0), n(p2.1)));
                    }
                }
            }
            d.set("QuadPoints", real_array(&quads));
            let res = if matches!(a.kind, NewKind::Highlight { .. }) {
                dictionary! { "ExtGState" => dictionary! { "GS" => dictionary! { "Type" => "ExtGState", "BM" => "Multiply" } } }
            } else {
                none()
            };
            (all, form(all, None, res, content.into_bytes()))
        }
        NewKind::Note { pos, text } => {
            let vr = Rect::from_min_size(*pos, vec2(NOTE_SIZE, NOTE_SIZE));
            d.set("Subtype", "Text");
            d.set("Name", "Comment");
            d.set("Contents", text_string(text));
            d.set("Open", false);
            d.set("F", 4 + 8); // utskrivbar, behåller storleken vid zoom
            let rect = user_rect(g, vr);
            let rgb = rgb_ops([255, 235, 59]);
            let s = NOTE_SIZE;
            let mut c = format!("{rgb} rg 0.45 0.38 0 RG 1 w 0.5 0.5 {} {} re B\n0.45 0.38 0 RG 1 w\n", n(s - 1.0), n(s - 1.0));
            for i in 0..3 {
                let y = s * (0.7 - 0.2 * i as f32);
                c.push_str(&format!("{} {} m {} {} l S\n", n(s * 0.2), n(y), n(s * 0.8), n(y)));
            }
            (rect, form([0.0, 0.0, s, s], Some(upright(g)), none(), c.into_bytes()))
        }
        NewKind::FreeText { rect: vr, text } => {
            d.set("Subtype", "FreeText");
            d.set("Contents", text_string(text));
            d.set("DA", Object::string_literal(format!("/Helv {FREE_SIZE} Tf {} rg", rgb_ops(a.color))));
            let (w, h) = (vr.width(), vr.height());
            let mut c = format!("BT /Helv {FREE_SIZE} Tf {} rg\n", rgb_ops(a.color)).into_bytes();
            for (i, l) in layout_text(text, FREE_SIZE, w - 4.0).iter().enumerate() {
                let y = h - 2.0 - FREE_SIZE * 0.85 - i as f32 * FREE_SIZE * 1.2;
                c.extend_from_slice(format!("1 0 0 1 2 {} Tm ", n(y)).as_bytes());
                c.extend_from_slice(&content_string(l));
                c.extend_from_slice(b" Tj\n");
            }
            c.extend_from_slice(b"ET\n");
            let res = dictionary! { "Font" => dictionary! { "Helv" => dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" } } };
            (user_rect(g, *vr), form([0.0, 0.0, w, h], Some(upright(g)), res, c))
        }
        NewKind::Ink { strokes, width } => {
            let strokes: Vec<&Vec<Pos2>> = strokes.iter().filter(|s| !s.is_empty()).collect();
            if strokes.is_empty() {
                return None;
            }
            d.set("Subtype", "Ink");
            d.set("BS", dictionary! { "Type" => "Border", "W" => r(*width), "S" => "S" });
            let mut list = Vec::new();
            let mut all = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            let mut c = format!("{} RG {} w 1 J 1 j\n", rgb_ops(a.color), n(*width));
            for s in strokes {
                let mut pts = Vec::new();
                for (i, p) in s.iter().enumerate() {
                    let (x, y) = to_user(g, *p);
                    all = [all[0].min(x), all[1].min(y), all[2].max(x), all[3].max(y)];
                    pts.push(r(x));
                    pts.push(r(y));
                    c.push_str(&format!("{} {} {}\n", n(x), n(y), if i == 0 { "m" } else { "l" }));
                }
                if s.len() == 1 {
                    let (x, y) = to_user(g, s[0]);
                    c.push_str(&format!("{} {} l\n", n(x), n(y)));
                }
                c.push_str("S\n");
                list.push(Object::Array(pts));
            }
            d.set("InkList", Object::Array(list));
            let e = width / 2.0 + 1.0;
            let all = [all[0] - e, all[1] - e, all[2] + e, all[3] + e];
            (all, form(all, None, none(), c.into_bytes()))
        }
    };
    d.set("Rect", real_array(&rect));
    Some((d, ap))
}

/// Matris som gör att ett utseende ritat upprätt i vyn blir upprätt på en sida med `/Rotate`.
fn upright(g: &Geo) -> [f32; 6] {
    match g.rot {
        90 => [0.0, 1.0, -1.0, 0.0, 0.0, 0.0],
        180 => [-1.0, 0.0, 0.0, -1.0, 0.0, 0.0],
        270 => [0.0, -1.0, 1.0, 0.0, 0.0, 0.0],
        _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    }
}

fn push_annots(doc: &mut Document, page: ObjectId, new: Vec<ObjectId>) {
    if new.is_empty() {
        return;
    }
    let refs = new.into_iter().map(Object::Reference);
    let existing = doc.get_dictionary(page).ok().and_then(|d| d.get(b"Annots").ok().cloned());
    if let Some(Object::Reference(id)) = existing {
        if let Ok(Object::Array(arr)) = doc.get_object_mut(id) {
            arr.extend(refs);
            return;
        }
    }
    let mut arr = match existing {
        Some(Object::Array(a)) => a,
        _ => Vec::new(),
    };
    arr.extend(refs);
    if let Ok(pd) = doc.get_dictionary_mut(page) {
        pd.set("Annots", Object::Array(arr));
    }
}

/// Läser PDF:en med lösenordet och avkrypterar den (kommer ut utan `/Encrypt`).
pub(super) fn open_doc(bytes: &[u8], password: Option<&str>) -> Result<Document, String> {
    let mut opts = lopdf::LoadOptions::default();
    opts.password = password.map(str::to_owned);
    let mut doc = Document::load_mem_with_options(bytes, opts).map_err(|e| tf!("PDF:en kunde inte läsas: {e}", e = e))?;
    if doc.is_encrypted() {
        doc.decrypt(password.unwrap_or("")).map_err(|_| t("PDF:en kunde inte avkrypteras").to_owned())?;
    }
    doc.trailer.remove(b"Encrypt");
    doc.encryption_state = None;
    Ok(doc)
}

/// Skriver `annots` in i PDF:en och ger de nya bytes. En krypterad PDF kommer ut avkrypterad
/// (lösenordet behövs för att öppna den). Fel ger ett meddelande, aldrig panik.
pub fn apply(bytes: &[u8], password: Option<&str>, annots: &[NewAnnot]) -> Result<Vec<u8>, String> {
    apply_all(bytes, password, annots, &Edits::new())
}

/// Utför ändringar och borttagningar av befintliga anteckningar och lägger till nya, i det inlästa
/// dokumentet (används både av omskrivningen och av den inkrementella sparningen).
pub fn apply_doc(doc: &mut Document, annots: &[NewAnnot], edits: &Edits) {
    super::annot_edit::edit_doc(doc, edits);
    let pages: BTreeMap<u32, ObjectId> = doc.get_pages();
    let ids: Vec<ObjectId> = pages.values().copied().collect();
    let (who, date) = (author(), pdf_date());
    let mut per_page: BTreeMap<usize, Vec<ObjectId>> = BTreeMap::new();
    for a in annots {
        let Some(&page) = ids.get(a.page) else { continue };
        let g = Geo::of(doc, page);
        if let Some(id) = build(doc, page, &g, a, &who, &date) {
            per_page.entry(a.page).or_default().push(id);
        }
    }
    for (p, new) in per_page {
        push_annots(doc, ids[p], new);
    }
}

/// Som `apply`, men utför först ändringar och borttagningar av anteckningar som redan finns i PDF:en.
/// Hela filen skrivs om; okrypterade filer sparas i stället inkrementellt, se `incremental::edit`.
pub fn apply_all(bytes: &[u8], password: Option<&str>, annots: &[NewAnnot], edits: &Edits) -> Result<Vec<u8>, String> {
    let mut doc = open_doc(bytes, password)?;
    apply_doc(&mut doc, annots, edits);
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| tf!("PDF:en kunde inte skrivas: {e}", e = e))?;
    Ok(out)
}

// ============================================================== tester

#[cfg(test)]
mod tests {
    use super::*;

    fn base_pdf(rotate: Option<i64>, crop: bool) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages = doc.new_object_id();
        let content = doc.add_object(Stream::new(dictionary! {}, b"".to_vec()));
        let mut page = dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
        };
        if let Some(r) = rotate {
            page.set("Rotate", r);
        }
        if crop {
            page.set("CropBox", vec![10.into(), 20.into(), 110.into(), 220.into()]);
        }
        let id = doc.add_object(page);
        doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(id)], "Count" => 1 }));
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", cat);
        let mut out = vec![];
        doc.save_to(&mut out).unwrap();
        out
    }

    fn all() -> Vec<NewAnnot> {
        let rc = Rect::from_min_size(pos2(20.0, 30.0), vec2(60.0, 12.0));
        vec![
            NewAnnot { page: 0, color: [255, 235, 59], kind: NewKind::Highlight { rects: vec![rc] } },
            NewAnnot { page: 0, color: [255, 0, 0], kind: NewKind::Underline { rects: vec![rc] } },
            NewAnnot { page: 0, color: [255, 0, 0], kind: NewKind::StrikeOut { rects: vec![rc] } },
            NewAnnot { page: 0, color: [255, 235, 59], kind: NewKind::Note { pos: pos2(50.0, 60.0), text: "Hej åäö (test)".into() } },
            NewAnnot { page: 0, color: [0, 0, 0], kind: NewKind::FreeText { rect: Rect::from_min_size(pos2(10.0, 80.0), vec2(80.0, 40.0)), text: "Text i ruta med flera ord".into() } },
            NewAnnot { page: 0, color: [0, 0, 255], kind: NewKind::Ink { strokes: vec![vec![pos2(5.0, 5.0), pos2(40.0, 50.0)]], width: 2.0 } },
        ]
    }

    fn annots_of(bytes: &[u8]) -> Vec<Dictionary> {
        let doc = Document::load_mem(bytes).unwrap();
        let page = *doc.get_pages().values().next().unwrap();
        let arr = doc.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
        arr.iter().map(|o| doc.get_dictionary(o.as_reference().unwrap()).unwrap().clone()).collect()
    }

    fn rect_of(d: &Dictionary) -> Vec<f32> {
        d.get(b"Rect").unwrap().as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect()
    }

    #[test]
    fn skriver_alla_typer_med_utseende() {
        let out = apply(&base_pdf(None, false), None, &all()).unwrap();
        let list = annots_of(&out);
        let subtypes: Vec<String> = list.iter().map(|d| String::from_utf8_lossy(d.get(b"Subtype").unwrap().as_name().unwrap()).into_owned()).collect();
        assert_eq!(subtypes, ["Highlight", "Underline", "StrikeOut", "Text", "FreeText", "Ink"]);
        let doc = Document::load_mem(&out).unwrap();
        for d in &list {
            assert!(d.has(b"Rect") && d.has(b"C") && d.has(b"T") && d.has(b"M") && d.has(b"P"));
            let ap = d.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
            let s = doc.get_object(ap).unwrap().as_stream().unwrap();
            assert!(s.dict.has(b"BBox"));
            assert!(!s.content.is_empty());
        }
        // Markering: QuadPoints i användarrymd (y uppåt): rad 30..42 uppifrån på en 300 hög sida.
        let q: Vec<f32> = list[0].get(b"QuadPoints").unwrap().as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect();
        assert_eq!(q, [20.0, 270.0, 80.0, 270.0, 20.0, 258.0, 80.0, 258.0]);
        assert_eq!(list[3].get(b"Contents").unwrap().as_str().unwrap().len() > 5, true);
        assert!(list[5].has(b"InkList"));
    }

    #[test]
    fn cropbox_ger_forskjutning() {
        // CropBox 10,20..110,220: raden 20..80 x 30..42 från övre vänstra hörnet.
        let out = apply(&base_pdf(None, true), None, &all()[..1]).unwrap();
        assert_eq!(rect_of(&annots_of(&out)[0]), [30.0, 178.0, 90.0, 190.0]);
    }

    #[test]
    fn rotate_ger_samma_sidpunkt_som_meta() {
        let bytes = base_pdf(Some(90), false);
        let doc = Document::load_mem(&bytes).unwrap();
        let page = *doc.get_pages().values().next().unwrap();
        let g = Geo::of(&doc, page);
        for rot in [0u16, 90, 180, 270] {
            let g = Geo { rot, ..g };
            for p in [pos2(0.0, 0.0), pos2(12.5, 33.0), pos2(100.0, 50.0)] {
                let (x, y) = to_user(&g, p);
                let back = g.point(x, y);
                assert!((back.0 - p.x).abs() < 1e-3 && (back.1 - p.y).abs() < 1e-3, "rot {rot}: {p:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn roterad_sida_ger_giltig_pdf_som_hayro_oppnar() {
        for rot in [90, 180, 270] {
            let out = apply(&base_pdf(Some(rot), false), None, &all()).unwrap();
            assert_eq!(annots_of(&out).len(), 6);
            let ctx = egui::Context::default();
            let e = crate::pdfview::engine::PdfEngine::open(std::sync::Arc::new(out), None, ctx);
            assert!(e.is_ok(), "rotate {rot}");
            assert_eq!(e.ok().unwrap().page_count(), 1);
        }
    }

    /// Renderar sida 0 med hayro (skala 1, ingen extra rotation).
    fn render(bytes: Vec<u8>) -> egui::ColorImage {
        use crate::pdfview::engine::{EngineEvent, PdfEngine, Recolor, RenderRequest};
        let e = PdfEngine::open(std::sync::Arc::new(bytes), None, egui::Context::default()).unwrap();
        e.request(vec![RenderRequest { page: 0, scale: 1.0, rotation: 0, recolor: Recolor::None }]);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < end {
            for ev in e.poll() {
                if let EngineEvent::Rendered { image, .. } = ev {
                    return image;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("ingen bild");
    }

    #[test]
    fn hayro_ritar_anteckningarna_pa_ratt_plats_oavsett_rotate() {
        let a = all();
        for (rot, crop) in [(None, false), (Some(90), false), (Some(180), false), (Some(270), false), (None, true), (Some(90), true)] {
            let img = |kinds: &[usize]| render(apply(&base_pdf(rot, crop), None, &kinds.iter().map(|i| a[*i].clone()).collect::<Vec<_>>()).unwrap());
            let w = |p: &egui::Color32| p.r() > 240 && p.g() > 240 && p.b() > 240;
            // Överstrykning: gul (blå kanal låg) mitt i rektangeln (50, 36).
            let hl = img(&[0]);
            let px = hl[(50, 36)];
            assert!(px.r() > 200 && px.b() < 150, "rotate {rot:?} crop {crop}: {px:?}");
            assert!(w(&hl[(30, 90)]), "rotate {rot:?} crop {crop}: utanför markeringen ska vara vitt");
            // Penna: blå linje från (5,5) till (40,50); mitt på (22,27).
            let ink = img(&[5]);
            let blue = (19..=25).any(|x| (24..=30).any(|y| ink[(x, y)].b() > 200 && ink[(x, y)].r() < 100));
            assert!(blue, "rotate {rot:?} crop {crop}: pennstrecket saknas");
            // Textruta och lapp syns (något ritas inom sin rektangel).
            let t = img(&[3, 4]);
            assert!((60..70).any(|y| (50..68).any(|x| !w(&t[(x, y)]))), "lapp saknas, rotate {rot:?}");
            assert!((82..120).any(|y| (10..90).any(|x| t[(x, y)].r() < 100)), "textrutans text saknas, rotate {rot:?}");
        }
    }

    #[test]
    fn befintliga_annots_bevaras() {
        let first = apply(&base_pdf(None, false), None, &all()[..2]).unwrap();
        let second = apply(&first, None, &all()[2..4]).unwrap();
        assert_eq!(annots_of(&second).len(), 4);
    }

    #[test]
    fn ogiltig_indata_ger_fel() {
        assert!(apply(b"inte en pdf", None, &[]).is_err());
        // Sida utanför dokumentet hoppas över.
        let mut a = all();
        a[0].page = 9;
        let out = apply(&base_pdf(None, false), None, &a[..1]).unwrap();
        assert!(Document::load_mem(&out).is_ok());
    }

    #[test]
    fn krypterad_pdf_skrivs_avkrypterad() {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/krypterad.pdf")).unwrap();
        assert!(apply(&bytes, Some("fel"), &[]).is_err());
        let out = apply(&bytes, Some("hemligt"), &all()[..1]).unwrap();
        assert!(!Document::load_mem(&out).unwrap().is_encrypted());
        let e = crate::pdfview::engine::PdfEngine::open(std::sync::Arc::new(out), None, egui::Context::default());
        assert!(e.is_ok());
    }

    #[test]
    fn radbrytning_och_textruta() {
        let l = layout_text("ett tvåord mycket långt ord här", 12.0, 50.0);
        assert!(l.len() >= 3);
        assert!(l.iter().all(|s| s.chars().map(helv_width).sum::<f32>() * 12.0 / 1000.0 <= 50.0 || s.chars().count() == 1));
        assert_eq!(layout_text("a\n\nb", 12.0, 100.0), ["a", "", "b"]);
        let r = freetext_rect(pos2(190.0, 290.0), "hej", 200.0, 300.0);
        assert!(r.max.x <= 200.0 && r.max.y <= 300.0 && r.min.x >= 0.0);
    }

    #[test]
    fn ta_bort_och_ångra() {
        let mut st = AnnotState::default();
        let rc = Rect::from_min_size(pos2(10.0, 10.0), vec2(30.0, 10.0));
        st.add_markup(AnnotTool::Highlight, vec![(0, vec![rc]), (1, vec![]), (2, vec![rc])]);
        assert!(st.has_pending());
        assert_eq!(st.pending().len(), 2);
        assert!(!st.delete_at(0, pos2(100.0, 100.0)));
        assert!(st.delete_at(0, pos2(15.0, 15.0)));
        assert!(st.undo());
        assert!(!st.undo() && !st.has_pending());
    }

    #[test]
    fn datum_och_text() {
        let d = pdf_date();
        assert!(d.starts_with("D:") && d.ends_with('Z') && d.len() == 17);
        assert_eq!(content_string("a(b)å€"), b"(a\\(b\\)\xE5?)".to_vec());
        assert_eq!(text_string("abc"), Object::String(b"abc".to_vec(), StringFormat::Literal));
    }
}
