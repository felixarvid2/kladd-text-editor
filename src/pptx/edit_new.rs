//! Infoga former genom att dra i redigeringsläget: rektangel, rundad rektangel, ellips, triangel,
//! pil, linje och textruta. Formerna får temats stil som i PowerPoint (`p:style` med `lnRef`,
//! `fillRef`, `effectRef` och `fontRef`), så fyllning, linje och textfärg följer temats färger.

use egui::{Color32, CursorIcon, PointerButton, Rect, Shape, Stroke, Ui, pos2};

use super::edit::Txn;
use super::edit_ops::{self, Xf};
use super::edit_view::Sel;
use super::view::PptxTab;
use crate::i18n::t;

/// En form som kan ritas med musen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Rect,
    RoundRect,
    Ellipse,
    Triangle,
    Arrow,
    Line,
    TextBox,
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 7] = [ShapeKind::Rect, ShapeKind::RoundRect, ShapeKind::Ellipse, ShapeKind::Triangle, ShapeKind::Arrow, ShapeKind::Line, ShapeKind::TextBox];

    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Rect => t("Rektangel"),
            ShapeKind::RoundRect => t("Rundad rektangel"),
            ShapeKind::Ellipse => t("Ellips"),
            ShapeKind::Triangle => t("Triangel"),
            ShapeKind::Arrow => t("Pil"),
            ShapeKind::Line => t("Linje"),
            ShapeKind::TextBox => t("Textruta"),
        }
    }

    fn prst(self) -> &'static str {
        match self {
            ShapeKind::Rect | ShapeKind::TextBox => "rect",
            ShapeKind::RoundRect => "roundRect",
            ShapeKind::Ellipse => "ellipse",
            ShapeKind::Triangle => "triangle",
            ShapeKind::Arrow => "rightArrow",
            ShapeKind::Line => "line",
        }
    }
}

/// Temats stil för en form: fyllning `accent1`, linje `accent1` (mörkare), text i vitt.
const SHAPE_STYLE: &str = r#"<p:style><a:lnRef idx="2"><a:schemeClr val="accent1"><a:shade val="50000"/></a:schemeClr></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style>"#;
/// Temats stil för en linje: linjen i `accent1`, ingen fyllning.
const LINE_STYLE: &str = r#"<p:style><a:lnRef idx="1"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="0"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="tx1"/></a:fontRef></p:style>"#;

/// Lägger formen sist på bilden med läget `xf` (för en linje: `flip_h`/`flip_v` i `xf` ger riktningen).
/// Ger formens läge i ritmodellen.
pub fn insert_shape(txn: &mut Txn, slide: usize, kind: ShapeKind, xf: Xf) -> Result<usize, String> {
    if kind == ShapeKind::TextBox {
        return edit_ops::insert_textbox(txn, slide, xf);
    }
    let part = edit_ops::slide_part(txn.deck, slide)?;
    let doc = txn.doc(&part)?;
    let (p, a) = (edit_ops::pfx(doc, super::edit::NS_P, "p"), edit_ops::pfx(doc, super::edit::NS_A, "a"));
    let q = |pre: &str, l: &str| if pre.is_empty() { l.to_owned() } else { format!("{pre}:{l}") };
    let id = edit_ops::max_id(&doc.root) + 1;
    let name = crate::tf!("{form} {n}", form = kind.label(), n = id - 1);
    let name = super::edit_xml::escape(&name);
    let flips = format!("{}{}", if xf.flip_h { " flipH=\"1\"" } else { "" }, if xf.flip_v { " flipV=\"1\"" } else { "" });
    let spr = format!(
        r#"<{p}:spPr><{a}:xfrm{flips}><{a}:off x="{}" y="{}"/><{a}:ext cx="{}" cy="{}"/></{a}:xfrm><{a}:prstGeom prst="{}"><{a}:avLst/></{a}:prstGeom></{p}:spPr>"#,
        xf.x,
        xf.y,
        xf.cx,
        xf.cy,
        kind.prst()
    );
    let xml = if kind == ShapeKind::Line {
        format!(r#"<{p}:cxnSp><{p}:nvCxnSpPr><{p}:cNvPr id="{id}" name="{name}"/><{p}:cNvCxnSpPr/><{p}:nvPr/></{p}:nvCxnSpPr>{spr}{}</{p}:cxnSp>"#, with_prefix(LINE_STYLE, &p, &a))
    } else {
        format!(
            r#"<{p}:sp><{p}:nvSpPr><{p}:cNvPr id="{id}" name="{name}"/><{p}:cNvSpPr/><{p}:nvPr/></{p}:nvSpPr>{spr}{}<{p}:txBody><{a}:bodyPr rtlCol="0" anchor="ctr"/><{a}:lstStyle/><{a}:p><{a}:pPr algn="ctr"/><{a}:endParaRPr lang="sv-SE"/></{a}:p></{p}:txBody></{p}:sp>"#,
            with_prefix(SHAPE_STYLE, &p, &a)
        )
    };
    let _ = q;
    edit_ops::append_shape(doc, &xml)
}

/// Byter prefixen `p:`/`a:` i stilmallen mot dokumentets egna.
fn with_prefix(xml: &str, p: &str, a: &str) -> String {
    let fix = |s: String, from: &str, to: &str| {
        if from == to {
            return s;
        }
        let (open, close) = (format!("<{from}:"), format!("</{from}:"));
        let to_open = if to.is_empty() { "<".to_owned() } else { format!("<{to}:") };
        let to_close = if to.is_empty() { "</".to_owned() } else { format!("</{to}:") };
        s.replace(&open, &to_open).replace(&close, &to_close)
    };
    fix(fix(xml.to_owned(), "p", p), "a", a)
}

impl PptxTab {
    /// Väljer formen som ritas med nästa drag (`None` avbryter).
    pub fn set_shape_tool(&mut self, tool: Option<ShapeKind>) {
        self.edit.tool = tool;
        self.edit.new_drag = None;
        if tool.is_some() {
            self.end_text();
        }
    }

    pub(super) fn shape_tool_active(&self) -> bool {
        self.edit.tool.is_some()
    }

    /// Musen när en formverktyg är valt: dra en ruta (ett klick ger standardstorlek), `Esc` avbryter.
    pub(super) fn shape_tool(&mut self, ui: &mut Ui, resp: &egui::Response, rect: Rect, slide: usize, accent: Color32) {
        let Some(kind) = self.edit.tool else { return };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.set_shape_tool(None);
            return;
        }
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        let (sw, _) = self.deck.size_pt();
        let k = rect.width() / sw.max(1.0);
        let to_slide = |pt: egui::Pos2| ((pt.x - rect.left()) / k, (pt.y - rect.top()) / k);
        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
                let s = to_slide(o);
                self.edit.new_drag = Some((s, s));
            }
        }
        if resp.dragged_by(PointerButton::Primary) {
            if let (Some(pt), Some((_, end))) = (ui.input(|i| i.pointer.interact_pos()), &mut self.edit.new_drag) {
                *end = to_slide(pt);
            }
        }
        let mut finish: Option<((f32, f32), (f32, f32))> = None;
        if resp.drag_stopped() {
            finish = self.edit.new_drag.take();
        } else if resp.clicked_by(PointerButton::Primary) {
            // Ett klick: standardstorlek med klickpunkten som mitt.
            if let Some(pt) = resp.interact_pointer_pos() {
                let (x, y) = to_slide(pt);
                let (w, h) = if kind == ShapeKind::Line { (144.0, 0.0) } else { (144.0, 86.4) };
                finish = Some(((x - w / 2.0, y - h / 2.0), (x + w / 2.0, y + h / 2.0)));
            }
        }
        if let Some(((ax, ay), (bx, by))) = finish {
            self.create_shape(slide, kind, (ax, ay), (bx, by));
        }
        if let Some(((ax, ay), (bx, by))) = self.edit.new_drag {
            let (a, b) = (pos2(rect.left() + ax * k, rect.top() + ay * k), pos2(rect.left() + bx * k, rect.top() + by * k));
            let painter = ui.painter_at(rect.expand(4.0));
            if kind == ShapeKind::Line {
                painter.add(Shape::line_segment([a, b], Stroke::new(2.0, accent)));
            } else {
                let r = Rect::from_two_pos(a, b);
                painter.rect_filled(r, 0.0, accent.gamma_multiply(0.15));
                painter.rect_stroke(r, 0.0, Stroke::new(1.5, accent), egui::StrokeKind::Middle);
            }
        }
    }

    /// Skapar formen mellan två punkter (bildens punkter) och markerar den.
    fn create_shape(&mut self, slide: usize, kind: ShapeKind, a: (f32, f32), b: (f32, f32)) {
        const EMU: f32 = 12_700.0;
        let (sw, sh) = self.deck.size_pt();
        let (l, r) = (a.0.min(b.0).clamp(0.0, sw), a.0.max(b.0).clamp(0.0, sw));
        let (t0, bt) = (a.1.min(b.1).clamp(0.0, sh), a.1.max(b.1).clamp(0.0, sh));
        let (mut w, mut h) = (r - l, bt - t0);
        let (mut x, mut y) = (l, t0);
        // En för liten ruta (ett klick som råkade röra sig) ger standardstorlek.
        if w < 4.0 && h < 4.0 {
            (w, h) = if kind == ShapeKind::Line { (144.0, 0.0) } else { (144.0, 86.4) };
            (x, y) = ((a.0 - w / 2.0).clamp(0.0, (sw - w).max(0.0)), (a.1 - h / 2.0).clamp(0.0, (sh - h).max(0.0)));
        }
        let xf = Xf {
            x: (x * EMU) as i64,
            y: (y * EMU) as i64,
            cx: (w * EMU) as i64,
            cy: (h * EMU) as i64,
            rot: 0,
            flip_h: kind == ShapeKind::Line && b.0 < a.0,
            flip_v: kind == ShapeKind::Line && b.1 < a.1,
        };
        self.set_shape_tool(None);
        let mut index = None;
        if self.apply(None, |txn| {
            index = Some(insert_shape(txn, slide, kind, xf)?);
            Ok(())
        }) {
            if let Some(i) = index {
                if let Some(s) = self.shapes_on(slide).into_iter().find(|s| s.index == i) {
                    self.edit.sel = Some(Sel { slide, index: s.index, id: s.id });
                    self.edit.more.clear();
                    if kind == ShapeKind::TextBox {
                        self.begin_text(Sel { slide, index: s.index, id: s.id }, None);
                    }
                }
            }
        }
    }
}
