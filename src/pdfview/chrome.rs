//! PDF-visarens gränssnitt runt själva sidvyn: verktygsrad, sidopanel (miniatyrer och innehåll),
//! sökfält, dialoger (Redigera, egenskaper, länkar) och statusrad. `PdfTab` är flikens tillstånd;
//! `app.rs` äger fliken i `Document::pdfv` och utför de `Action` som raden och dialogerna ger.
//! Visaren själv (`PdfView`) används bara via API-kontraktet i `docs/dev/pdfvisare.md`.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use egui::style::ScrollAnimation;
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Align, Align2, Color32, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Modal, Order, Panel, Popup, Rect,
    RectAlign, Response, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui, Vec2, pos2, vec2,
};

use super::engine::{OpenError, PdfEngine, Recolor, RenderRequest};
use super::meta::{Dest, OutlineItem};
use super::pages::{self, OrgOp, OrgState};
use super::view::{FindStatus, PdfView, Spread, Tool, ZoomMode};
use crate::app::{menu_divider, menu_item, quiet_button, text_toggle};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

/// Filens ändringstid och storlek, för att märka när den ändrats på disk.
pub type FileStamp = (SystemTime, u64);

/// Hur ofta filen på disk kontrolleras (sekunder).
pub const CHECK_EVERY: f64 = 1.0;
/// Högsta antal filer vars visningsläge sparas.
pub const MAX_MEMORY: usize = 100;

const BAR_H: f32 = 40.0;
const BTN: f32 = 30.0;

/// Det raden och dialogerna ber appen göra.
pub enum Action {
    /// Gör om PDF:en till ett redigerbart dokument (efter bekräftelse).
    Edit,
    SaveCopy,
    Print,
    OpenExternal,
    Reload,
    CopyImage,
    /// Öppna en bekräftad extern länk.
    OpenUri(String),
    /// Ändra sidor (flytta, ta bort, rotera, extrahera, infoga, ångra).
    Organize(OrgOp),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SideTab {
    Thumbs,
    Outline,
}

/// Sökfältets tillstånd.
#[derive(Default)]
pub struct FindUi {
    pub open: bool,
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    focus: bool,
    /// Senast skickade (fråga, versaler, hela ord), så att `find_set` bara anropas vid ändring.
    sent: Option<(String, bool, bool)>,
}

/// En öppen PDF-flik: visaren och allt gränssnittstillstånd runt den.
pub struct PdfTab {
    pub view: PdfView,
    /// Lösenordet som användes vid öppning (behövs för Redigera, omläsning och sidbild).
    pub password: Option<String>,
    pub stamp: Option<FileStamp>,
    pub next_check: f64,
    pub sidebar: bool,
    side_tab: SideTab,
    outline_open: HashMap<Vec<usize>, bool>,
    last_page: Option<usize>,
    last_section: Option<Vec<usize>>,
    pub find: FindUi,
    page_text: String,
    page_focus: bool,
    pub props: bool,
    pub confirm_edit: bool,
    pub confirm_uri: Option<String>,
    pub copy_rx: Option<Receiver<Result<egui::ColorImage, String>>>,
    /// Sidorna har ändrats (flyttats, tagits bort, roterats, infogats) och är inte sparade.
    pub pages_changed: bool,
    pub org: OrgState,
}

impl PdfTab {
    pub fn new(view: PdfView, password: Option<String>, stamp: Option<FileStamp>) -> Self {
        let has_outline = !view.meta().outline.is_empty();
        Self {
            view,
            password,
            stamp,
            next_check: 0.0,
            sidebar: has_outline,
            side_tab: if has_outline { SideTab::Outline } else { SideTab::Thumbs },
            outline_open: HashMap::new(),
            last_page: None,
            last_section: None,
            find: FindUi::default(),
            page_text: String::new(),
            page_focus: false,
            props: false,
            confirm_edit: false,
            confirm_uri: None,
            copy_rx: None,
            pages_changed: false,
            org: OrgState::default(),
        }
    }

    /// Är någon dialog öppen (då ska kortkommandon vänta)?
    pub fn dialog_open(&self) -> bool {
        self.props || self.confirm_edit || self.confirm_uri.is_some()
    }

    pub fn open_find(&mut self) {
        if !self.find.open {
            // Förifyll med markerad text om den är kort och på en rad.
            if let Some(sel) = self.view.selected_text() {
                let sel = sel.trim();
                if !sel.is_empty() && !sel.contains('\n') && sel.chars().count() <= 80 {
                    self.find.query = sel.to_owned();
                }
            }
        }
        self.find.open = true;
        self.find.focus = true;
    }

    pub fn close_find(&mut self) {
        self.find.open = false;
        self.find.sent = None;
        self.view.find_clear();
    }

    /// Ctrl+G: sätter markören i sidfältet.
    pub fn focus_page_field(&mut self) {
        self.page_focus = true;
    }

    pub fn toggle_sidebar(&mut self) {
        self.sidebar = !self.sidebar;
    }

    /// F5: presentationsläge.
    pub fn present(&mut self) {
        self.view.set_presenting(true);
    }
}

// ------------------------------------------------------------ rena hjälpfunktioner

/// Ändringstid och storlek för en fil.
pub fn file_stamp(path: &Path) -> Option<FileStamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Läser en sidangivelse: en sidetikett ("iv") eller ett sidnummer (1-baserat). Ger 0-baserat index.
pub fn parse_page_input(input: &str, labels: &[String], count: usize) -> Option<usize> {
    let s = input.trim();
    if s.is_empty() || count == 0 {
        return None;
    }
    if let Some(i) = labels.iter().position(|l| !l.is_empty() && l.eq_ignore_ascii_case(s)) {
        return (i < count).then_some(i);
    }
    let n: usize = s.parse().ok()?;
    (1..=count).contains(&n).then(|| n - 1)
}

/// Texten i statusraden: "iv (4 av 12)" när sidan har en etikett som skiljer sig från numret, annars "Sida 4 av 12".
pub fn page_status(label: &str, page: usize, total: usize) -> String {
    let n = page + 1;
    if !label.is_empty() && label != n.to_string() {
        format!("{label} ({n} av {total})")
    } else {
        format!("Sida {n} av {total}")
    }
}

/// Filstorlek på svenska: "532 B", "12,5 kB", "3,4 MB".
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "kB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    let s = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{} {}", s.replace('.', ","), UNITS[u])
}

/// Punkter (1/72 tum) till millimeter.
pub fn pt_to_mm(pt: f32) -> f32 {
    pt * 25.4 / 72.0
}

/// Pappersformatets namn om sidan (i mm, valfri riktning) stämmer med A3/A4/A5/Letter/Legal inom 2 mm.
pub fn paper_name(w_mm: f32, h_mm: f32) -> Option<&'static str> {
    const SIZES: [(&str, f32, f32); 5] =
        [("A3", 297.0, 420.0), ("A4", 210.0, 297.0), ("A5", 148.0, 210.0), ("Letter", 215.9, 279.4), ("Legal", 215.9, 355.6)];
    let (short, long) = if w_mm <= h_mm { (w_mm, h_mm) } else { (h_mm, w_mm) };
    SIZES.iter().find(|(_, s, l)| (short - s).abs() <= 2.0 && (long - l).abs() <= 2.0).map(|(n, _, _)| *n)
}

/// "210 × 297 mm (A4, stående)".
pub fn page_size_text(w_pt: f32, h_pt: f32) -> String {
    let (w, h) = (pt_to_mm(w_pt), pt_to_mm(h_pt));
    let orient = if w > h + 0.5 { "liggande" } else { "stående" };
    match paper_name(w, h) {
        Some(name) => format!("{w:.0} × {h:.0} mm ({name}, {orient})"),
        None => format!("{w:.0} × {h:.0} mm ({orient})"),
    }
}

/// Ett val i zoomlistan.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ZoomChoice {
    Mode(ZoomMode),
    Percent(u32),
}

pub const ZOOM_CHOICES: [(&str, ZoomChoice); 12] = [
    ("Automatisk", ZoomChoice::Mode(ZoomMode::Auto)),
    ("Verklig storlek", ZoomChoice::Mode(ZoomMode::ActualSize)),
    ("Hel sida", ZoomChoice::Mode(ZoomMode::PageFit)),
    ("Sidbredd", ZoomChoice::Mode(ZoomMode::PageWidth)),
    ("50 %", ZoomChoice::Percent(50)),
    ("75 %", ZoomChoice::Percent(75)),
    ("100 %", ZoomChoice::Percent(100)),
    ("125 %", ZoomChoice::Percent(125)),
    ("150 %", ZoomChoice::Percent(150)),
    ("200 %", ZoomChoice::Percent(200)),
    ("300 %", ZoomChoice::Percent(300)),
    ("400 %", ZoomChoice::Percent(400)),
];

/// Vilket zoomläge (och ev. zoomfaktor, 1.0 = 100 %) ett val ger.
pub fn zoom_choice_target(c: ZoomChoice) -> (ZoomMode, Option<f32>) {
    match c {
        ZoomChoice::Mode(m) => (m, None),
        ZoomChoice::Percent(p) => (ZoomMode::Custom, Some(p as f32 / 100.0)),
    }
}

/// Vilket val som motsvarar visarens nuvarande läge (None för en egen zoomnivå).
pub fn current_zoom_choice(mode: ZoomMode, zoom: f32) -> Option<ZoomChoice> {
    if mode != ZoomMode::Custom {
        return Some(ZoomChoice::Mode(mode));
    }
    let pct = zoom * 100.0;
    ZOOM_CHOICES.iter().map(|(_, c)| *c).find(|c| matches!(c, ZoomChoice::Percent(p) if (pct - *p as f32).abs() < 0.5))
}

/// Texten i zoomlistan: lägets namn, eller procent vid egen zoom.
pub fn zoom_label(mode: ZoomMode, zoom: f32) -> String {
    match current_zoom_choice(mode, zoom) {
        Some(c) => ZOOM_CHOICES.iter().find(|(_, x)| *x == c).map(|(l, _)| (*l).to_owned()).unwrap_or_default(),
        None => format!("{} %", (zoom * 100.0).round() as i32),
    }
}

/// Lägger in ett visningsläge sist-använd-först i listan (högst `cap` poster).
pub fn remember_memory<T>(list: &mut Vec<(String, T)>, key: String, mem: T, cap: usize) {
    list.retain(|(k, _)| *k != key);
    list.insert(0, (key, mem));
    list.truncate(cap);
}

/// Söktexten i sökfältet.
pub fn find_status_text(s: &FindStatus, page_count: usize, has_query: bool) -> String {
    if !has_query {
        return String::new();
    }
    if s.searching && s.done_pages < page_count && s.total == 0 {
        return format!("Söker… ({}/{} sidor)", s.done_pages, page_count);
    }
    if s.total == 0 {
        return "Hittades inte".to_owned();
    }
    let suffix = if s.searching { "+" } else { "" };
    match s.current {
        Some(i) => format!("{} av {}{suffix}", i + 1, s.total),
        None => format!("{}{suffix} träffar", s.total),
    }
}

/// Länkar som får öppnas i webbläsaren efter bekräftelse.
pub fn safe_uri(uri: &str) -> bool {
    let u = uri.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:", "ftp://"].iter().any(|s| u.starts_with(s))
}

/// Sökvägen (index i varje nivå) till det avsnitt som sidan `page` tillhör: det sista
/// bokmärket i dokumentordning vars mål ligger på eller före sidan.
pub fn current_section(outline: &[OutlineItem], page: usize) -> Option<Vec<usize>> {
    fn walk(items: &[OutlineItem], page: usize, path: &mut Vec<usize>, best: &mut Option<Vec<usize>>) {
        for (i, it) in items.iter().enumerate() {
            path.push(i);
            if it.dest.as_ref().is_some_and(|d| d.page <= page) {
                *best = Some(path.clone());
            }
            walk(&it.children, page, path, best);
            path.pop();
        }
    }
    let mut best = None;
    walk(outline, page, &mut Vec::new(), &mut best);
    best
}

// ------------------------------------------------------------ ikoner

#[derive(Clone, Copy)]
enum G {
    Std(Icon),
    Sidebar,
    Left,
    Right,
    RotL,
    RotR,
    Search,
    Print,
}

fn paint_glyph(ui: &Ui, g: G, c: egui::Pos2, color: Color32, under: Color32) {
    let s = Stroke::new(1.5, color);
    let p = ui.painter();
    match g {
        G::Std(i) => icons::paint(p, i, c, color, under),
        G::Sidebar => {
            let r = Rect::from_center_size(c, vec2(16.0, 13.0));
            p.rect_stroke(r, 2.5, s, StrokeKind::Inside);
            p.vline(r.left() + 5.5, r.y_range(), s);
        }
        G::Left => {
            p.line_segment([c + vec2(2.5, -5.5), c + vec2(-2.5, 0.0)], s);
            p.line_segment([c + vec2(-2.5, 0.0), c + vec2(2.5, 5.5)], s);
        }
        G::Right => {
            p.line_segment([c + vec2(-2.5, -5.5), c + vec2(2.5, 0.0)], s);
            p.line_segment([c + vec2(2.5, 0.0), c + vec2(-2.5, 5.5)], s);
        }
        G::RotL | G::RotR => {
            // Cirkelbåge (ca 270°) med pilspets.
            let dir = if matches!(g, G::RotR) { 1.0 } else { -1.0 };
            let r = 6.0;
            let start = -std::f32::consts::FRAC_PI_2 * 1.0;
            let steps = 14;
            let mut prev = None;
            let mut last = c;
            for k in 0..=steps {
                let a = start + dir * (k as f32 / steps as f32) * std::f32::consts::PI * 1.5;
                let pt = c + vec2(a.cos(), a.sin()) * r;
                if let Some(pp) = prev {
                    p.line_segment([pp, pt], s);
                }
                prev = Some(pt);
                last = pt;
            }
            // Pilspets vid slutet, tangent i rörelseriktningen.
            let a_end = start + dir * std::f32::consts::PI * 1.5;
            let tangent = vec2(-a_end.sin(), a_end.cos()) * dir;
            let normal = vec2(-tangent.y, tangent.x);
            let tip = last + tangent * 2.0;
            p.line_segment([tip, last - tangent * 2.0 + normal * 3.0], s);
            p.line_segment([tip, last - tangent * 2.0 - normal * 3.0], s);
        }
        G::Search => {
            p.circle_stroke(c + vec2(-1.5, -1.5), 5.0, s);
            p.line_segment([c + vec2(2.2, 2.2), c + vec2(6.5, 6.5)], s);
        }
        G::Print => {
            p.rect_stroke(Rect::from_center_size(c + vec2(0.0, 1.0), vec2(16.0, 7.0)), 2.0, s, StrokeKind::Inside);
            p.rect_stroke(Rect::from_min_max(c + vec2(-4.0, -6.5), c + vec2(4.0, -1.0)), 1.0, s, StrokeKind::Inside);
            p.rect_stroke(Rect::from_min_max(c + vec2(-4.0, 2.5), c + vec2(4.0, 7.0)), 1.0, s, StrokeKind::Inside);
        }
    }
}

fn tool_btn(ui: &mut Ui, p: &Palette, g: G, active: bool, enabled: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(BTN), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(crate::keys::shortcut(tip));
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered() && enabled;
        if hovered {
            ui.painter().rect_filled(rect, 7.0, p.surface_hover);
        } else if active {
            ui.painter().rect_filled(rect, 7.0, p.accent.gamma_multiply(0.16));
        }
        let color = if !enabled {
            p.faint
        } else if active {
            p.accent
        } else if hovered {
            p.text
        } else {
            p.muted
        };
        let under = if hovered { p.surface_hover } else { p.chrome };
        paint_glyph(ui, g, rect.center(), color, under);
        if resp.has_focus() {
            ui.painter().rect_stroke(rect.shrink(0.5), 7.0, Stroke::new(1.0, p.accent), StrokeKind::Inside);
        }
    }
    resp
}

fn bar_divider(ui: &mut Ui, p: &Palette) {
    ui.add_space(3.0);
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 22.0), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, p.line));
    ui.add_space(3.0);
}

/// Knapp med text och liten pil som öppnar en meny.
fn menu_trigger(ui: &mut Ui, p: &Palette, label: &str) -> Response {
    let font = FontId::new(13.5, FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, p.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, BTN), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 7.0, p.surface_hover);
    }
    let color = if resp.hovered() { p.text } else { p.muted };
    ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, color);
    icons::paint(ui.painter(), Icon::ChevronDown, pos2(rect.right() - 12.0, rect.center().y), color, p.chrome);
    resp
}

fn menu_heading(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(text).font(FontId::new(11.5, theme::medium())).color(p.muted));
    });
    ui.add_space(2.0);
}

// ------------------------------------------------------------ verktygsrad

/// Verktygsraden under flikraden. Ger en åtgärd som appen ska utföra.
pub fn toolbar(ui: &mut Ui, p: &Palette, tab: &mut PdfTab) -> Option<Action> {
    let ctx = ui.ctx().clone();
    let mut act = None;
    Panel::top("kladd-pdf-bar")
        .exact_size(BAR_H)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin::symmetric(12, 5)))
        .show(ui, |ui| {
            let r = ui.max_rect();
            ui.painter().hline((r.left() - 12.0)..=(r.right() + 12.0), r.bottom() + 5.0, Stroke::new(1.0, p.line));
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let total = tab.view.page_count();
                let cur = tab.view.current_page();

                if tool_btn(ui, p, G::Sidebar, tab.sidebar, true, "Sidopanel (F4)").clicked() {
                    tab.toggle_sidebar();
                }
                bar_divider(ui, p);
                // Historik
                if tool_btn(ui, p, G::Left, false, tab.view.can_back(), "Bakåt (Alt+Vänster)").clicked() {
                    tab.view.back();
                }
                if tool_btn(ui, p, G::Right, false, tab.view.can_forward(), "Framåt (Alt+Höger)").clicked() {
                    tab.view.forward();
                }
                bar_divider(ui, p);

                // Sidor
                if tool_btn(ui, p, G::Std(Icon::ChevronUp), false, cur > 0, "Föregående sida (PgUp)").clicked() {
                    tab.view.go_to_page(cur - 1);
                }
                let field_id = Id::unique("kladd-pdf-page");
                let focused = ctx.memory(|m| m.has_focus(field_id));
                if !focused && !tab.page_focus {
                    tab.page_text = tab.view.page_label(cur);
                }
                let field = ui.add(
                    TextEdit::singleline(&mut tab.page_text)
                        .id(field_id)
                        .desired_width(46.0)
                        .align(Align2::CENTER_CENTER)
                        .margin(Margin::symmetric(4, 4)),
                );
                if tab.page_focus {
                    field.request_focus();
                    let mut st = TextEditState::load(&ctx, field_id).unwrap_or_default();
                    st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(tab.page_text.chars().count()))));
                    st.store(&ctx, field_id);
                    tab.page_focus = false;
                }
                field.clone().on_hover_text(crate::keys::shortcut("Gå till sida (Ctrl+G)"));
                if field.lost_focus() {
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        let labels = &tab.view.meta().labels;
                        if let Some(i) = parse_page_input(&tab.page_text, labels, total) {
                            tab.view.go_to_page(i);
                        }
                    }
                    // Fältet visar sidan igen när det släpper fokus.
                    tab.page_text = tab.view.page_label(tab.view.current_page());
                }
                ui.label(RichText::new(format!("/ {total}")).color(p.muted));
                if tool_btn(ui, p, G::Std(Icon::ChevronDown), false, cur + 1 < total, "Nästa sida (PgDn)").clicked() {
                    tab.view.go_to_page(cur + 1);
                }
                bar_divider(ui, p);

                // Zoom
                if tool_btn(ui, p, G::Std(Icon::Minus), false, true, "Zooma ut (Ctrl+−)").clicked() {
                    tab.view.zoom_out();
                }
                let (mode, zoom) = (tab.view.zoom_mode(), tab.view.zoom());
                egui::ComboBox::from_id_salt("kladd-pdf-zoom")
                    .width(108.0)
                    .selected_text(zoom_label(mode, zoom))
                    .show_ui(ui, |ui| {
                        let now = current_zoom_choice(mode, zoom);
                        for (label, choice) in ZOOM_CHOICES {
                            if ui.selectable_label(now == Some(choice), label).clicked() {
                                match zoom_choice_target(choice) {
                                    (m, None) => tab.view.set_zoom_mode(m),
                                    (_, Some(z)) => tab.view.set_zoom(z),
                                }
                            }
                        }
                    });
                if tool_btn(ui, p, G::Std(Icon::Plus), false, true, "Zooma in (Ctrl+plus)").clicked() {
                    tab.view.zoom_in();
                }
                bar_divider(ui, p);

                if tool_btn(ui, p, G::RotL, false, true, "Rotera åt vänster (Shift+R)").clicked() {
                    tab.view.rotate(false);
                }
                if tool_btn(ui, p, G::RotR, false, true, "Rotera åt höger (R)").clicked() {
                    tab.view.rotate(true);
                }
                let view_btn = menu_trigger(ui, p, "Visa");
                Popup::menu(&view_btn).width(270.0).align(RectAlign::BOTTOM_START).gap(6.0).show(|ui| {
                    view_menu(ui, p, tab, &mut act);
                });
                bar_divider(ui, p);
                if tool_btn(ui, p, G::Search, tab.find.open, true, "Sök i dokumentet (Ctrl+F)").clicked() {
                    if tab.find.open {
                        tab.close_find();
                    } else {
                        tab.open_find();
                    }
                }
                if tool_btn(ui, p, G::Print, false, true, "Skriv ut (Ctrl+P)").clicked() {
                    act = Some(Action::Print);
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let edit = egui::Button::new(
                        RichText::new("Redigera").font(FontId::new(13.5, theme::medium())).color(p.on_accent),
                    )
                    .fill(p.accent)
                    .stroke(Stroke::NONE)
                    .corner_radius(8)
                    .min_size(vec2(92.0, 28.0));
                    if ui.add(edit).on_hover_text("Gör om PDF:en till ett redigerbart dokument").clicked() {
                        tab.confirm_edit = true;
                    }
                });
            });
        });
    act
}

/// Innehållet i "Visa"-menyn.
fn view_menu(ui: &mut Ui, p: &Palette, tab: &mut PdfTab, act: &mut Option<Action>) {
    ui.spacing_mut().item_spacing.y = 0.0;
    menu_heading(ui, p, "Uppslag");
    let sp = tab.view.spread();
    for (label, s) in [("En sida", Spread::None), ("Två sidor, udda till vänster", Spread::Odd), ("Två sidor, jämna till vänster", Spread::Even)] {
        if menu_item(ui, p, label, "", Some(sp == s), true) {
            tab.view.set_spread(s);
        }
    }
    menu_divider(ui, p);
    menu_heading(ui, p, "Färger");
    let rc = tab.view.recolor();
    for (label, r) in [("Normala färger", Recolor::None), ("Inverterade färger", Recolor::Invert), ("Sepia", Recolor::Sepia)] {
        if menu_item(ui, p, label, "", Some(rc == r), true) {
            tab.view.set_recolor(r);
        }
    }
    menu_divider(ui, p);
    let hand = tab.view.tool() == Tool::Hand;
    if menu_item(ui, p, "Handverktyg", "H", Some(hand), true) {
        tab.view.set_tool(if hand { Tool::Select } else { Tool::Hand });
    }
    if menu_item(ui, p, "Presentationsläge", "F5", None, true) {
        tab.present();
    }
    menu_divider(ui, p);
    menu_heading(ui, p, "Sidor");
    if menu_item(ui, p, "Slå ihop med PDF…", "", None, true) {
        *act = Some(Action::Organize(OrgOp::InsertAt(tab.view.page_count())));
    }
    if menu_item(ui, p, "Ångra sidändring", "Ctrl+Z", None, tab.org.history.can_undo()) {
        *act = Some(Action::Organize(OrgOp::Undo));
    }
    if menu_item(ui, p, "Gör om sidändring", "Ctrl+Shift+Z", None, tab.org.history.can_redo()) {
        *act = Some(Action::Organize(OrgOp::Redo));
    }
    menu_divider(ui, p);
    if menu_item(ui, p, "Läs om", "", None, true) {
        *act = Some(Action::Reload);
    }
    if menu_item(ui, p, "Spara kopia…", if tab.pages_changed { "" } else { "Ctrl+S" }, None, true) {
        *act = Some(Action::SaveCopy);
    }
    if menu_item(ui, p, "Kopiera sidan som bild", "", None, true) {
        *act = Some(Action::CopyImage);
    }
    if menu_item(ui, p, "Dokumentegenskaper…", "", None, true) {
        tab.props = true;
    }
    if menu_item(ui, p, "Öppna i systemets PDF-visare", "", None, true) {
        *act = Some(Action::OpenExternal);
    }
}

// ------------------------------------------------------------ sidopanel

const LABEL_H: f32 = 22.0;
const THUMB_GAP: f32 = 10.0;
const OUTLINE_ROW_H: f32 = 26.0;

/// Panelen till vänster med miniatyrer och innehållsförteckning (F4).
pub fn sidebar(ui: &mut Ui, p: &Palette, tab: &mut PdfTab) -> Option<Action> {
    let mut act = None;
    if !tab.sidebar {
        return act;
    }
    let ctx = ui.ctx().clone();
    let has_outline = !tab.view.meta().outline.is_empty();
    if !has_outline {
        tab.side_tab = SideTab::Thumbs;
    }
    let width = ui.available_width();
    Panel::left("kladd-pdf-sidebar")
        .resizable(true)
        .default_size(210.0)
        .size_range(150.0..=(width * 0.5).max(200.0))
        .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin { left: 10, right: 6, top: 8, bottom: 0 }))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
            ui.horizontal(|ui| {
                if side_tab_button(ui, p, "Miniatyrer", tab.side_tab == SideTab::Thumbs).clicked() {
                    tab.side_tab = SideTab::Thumbs;
                }
                if has_outline && side_tab_button(ui, p, "Innehåll", tab.side_tab == SideTab::Outline).clicked() {
                    tab.side_tab = SideTab::Outline;
                }
            });
            ui.add_space(8.0);
            match tab.side_tab {
                SideTab::Thumbs => act = thumbnails(ui, p, tab, &ctx).map(Action::Organize),
                SideTab::Outline => outline(ui, p, tab),
            }
        });
    act
}

fn side_tab_button(ui: &mut Ui, p: &Palette, text: &str, active: bool) -> Response {
    let font = FontId::new(13.0, if active { theme::medium() } else { FontFamily::Proportional });
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, if active { p.text } else { p.muted });
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 20.0, 26.0), Sense::click());
    if resp.hovered() && !active {
        ui.painter().rect_filled(rect, 7.0, p.surface_hover);
    }
    if active {
        ui.painter().rect_filled(rect, 7.0, p.surface);
    }
    let color = if active { p.text } else { p.muted };
    ui.painter().galley(pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, color);
    resp
}

fn thumbnails(ui: &mut Ui, p: &Palette, tab: &mut PdfTab, ctx: &egui::Context) -> Option<OrgOp> {
    let n = tab.view.page_count();
    let cur = tab.view.current_page();
    let w = ui.available_width();
    let tw = (w - 30.0).clamp(70.0, 240.0);
    let swap = tab.view.memory().rotation % 180 == 90;
    // Höjd per miniatyr utifrån sidans proportioner.
    let mut tops = Vec::with_capacity(n + 1);
    let mut heights = Vec::with_capacity(n);
    let mut y = 4.0;
    for i in 0..n {
        let s = tab.view.engine().page_size(i);
        let (pw, ph) = if swap { (s.height, s.width) } else { (s.width, s.height) };
        let th = (tw * ph / pw.max(1.0)).clamp(tw * 0.3, tw * 3.0);
        tops.push(y);
        heights.push(th);
        y += th + LABEL_H + THUMB_GAP;
    }
    tops.push(y);
    let item_h = |i: usize| heights[i] + LABEL_H + THUMB_GAP;
    let scroll_to_cur = tab.last_page != Some(cur);
    tab.last_page = Some(cur);
    let ppp = ctx.pixels_per_point();
    let mut jump = None;
    let mut op = None;
    let panel = ui.max_rect();
    // Ett tryck utanför panelen (inte i en meny) släpper urvalet, så att Delete inte träffar sidor man glömt.
    if !tab.org.sel.is_empty() && !Popup::is_any_open(ctx) {
        let outside = ctx.input(|i| i.pointer.primary_pressed() && i.pointer.press_origin().is_some_and(|o| !panel.contains(o)));
        if outside {
            tab.org.sel.clear();
        }
    }
    ScrollArea::vertical().id_salt("kladd-pdf-thumbs").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        ui.allocate_space(vec2(w, y));
        let origin = ui.min_rect().min;
        if scroll_to_cur && cur < n {
            let r = Rect::from_min_size(origin + vec2(0.0, tops[cur]), vec2(w, item_h(cur)));
            ui.scroll_to_rect(r, Some(Align::Center));
        }
        let lo = tops.partition_point(|t| *t < viewport.top() - 300.0).saturating_sub(1);
        let hi = tops.partition_point(|t| *t < viewport.bottom() + 300.0).min(n);
        for i in lo..hi {
            let item = Rect::from_min_size(origin + vec2(0.0, tops[i]), vec2(w, item_h(i)));
            let resp = ui.interact(item, Id::unique(("kladd-pdf-thumb", i)), Sense::click_and_drag());
            let is_cur = i == cur;
            let chosen = tab.org.sel.contains(&i);
            if chosen {
                ui.painter().rect_filled(item.shrink2(vec2(2.0, 0.0)), 8.0, p.accent.gamma_multiply(if is_cur { 0.30 } else { 0.22 }));
            } else if is_cur {
                ui.painter().rect_filled(item.shrink2(vec2(2.0, 0.0)), 8.0, p.accent.gamma_multiply(0.16));
            } else if resp.hovered() {
                ui.painter().rect_filled(item.shrink2(vec2(2.0, 0.0)), 8.0, p.surface_hover);
            }
            let slot = Rect::from_min_size(pos2(item.center().x - tw / 2.0, item.top() + 4.0), vec2(tw, heights[i]));
            let tex = tab.view.thumbnail(ctx, i, tw * ppp);
            let img_rect = match &tex {
                Some(t) => {
                    let sz = t.size_vec2();
                    let k = (slot.width() / sz.x.max(1.0)).min(slot.height() / sz.y.max(1.0));
                    Rect::from_center_size(slot.center(), sz * k)
                }
                None => slot,
            };
            ui.painter().rect_filled(img_rect.translate(vec2(0.0, 1.5)), 2.0, p.page_shadow);
            match &tex {
                Some(t) => {
                    ui.painter().image(t.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    ui.painter().rect_filled(img_rect, 2.0, Color32::from_gray(if p.dark { 60 } else { 245 }));
                }
            }
            ui.painter().rect_stroke(img_rect, 2.0, Stroke::new(if is_cur { 2.0 } else { 1.0 }, if is_cur { p.accent } else { p.line }), StrokeKind::Outside);
            ui.painter().text(
                pos2(item.center().x, img_rect.bottom() + 4.0 + LABEL_H / 2.0 - 2.0),
                Align2::CENTER_CENTER,
                tab.view.page_label(i),
                FontId::new(12.0, if is_cur { theme::medium() } else { FontFamily::Proportional }),
                if is_cur { p.accent } else { p.muted },
            );
            if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                let (sel, anchor) = pages::click_selection(&tab.org.sel, tab.org.anchor, i, m.shift, m.command, cur);
                tab.org.sel = sel;
                tab.org.anchor = anchor;
                if !m.shift && !m.command {
                    jump = Some(i);
                }
            }
            if resp.secondary_clicked() && !tab.org.sel.contains(&i) {
                tab.org.sel = BTreeSet::from([i]);
                tab.org.anchor = Some(i);
            }
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if !tab.org.sel.contains(&i) {
                    tab.org.sel = BTreeSet::from([i]);
                    tab.org.anchor = Some(i);
                }
                tab.org.drag_active = true;
            }
            Popup::context_menu(&resp).width(230.0).show(|ui| {
                if let Some(o) = page_menu(ui, p, tab, n, i) {
                    op = Some(o);
                }
            });
        }
        // Drag av miniatyrer: markera var de hamnar (före vilken sida) och flytta när musen släpps.
        if tab.org.drag_active {
            let pointer = ctx.pointer_latest_pos();
            let clip = ui.clip_rect();
            tab.org.drop = pointer.filter(|pos| clip.contains(*pos)).map(|pos| {
                let y = pos.y - origin.y;
                (0..n).find(|&k| y < tops[k] + item_h(k) / 2.0).unwrap_or(n)
            });
            if let (Some(before), true) = (tab.org.drop, n > 0) {
                let ly = origin.y + tops[before] - THUMB_GAP / 2.0 + 2.0;
                ui.painter().line_segment([pos2(origin.x + 6.0, ly), pos2(origin.x + w - 6.0, ly)], Stroke::new(3.0, p.accent));
            }
            if let Some(pos) = pointer {
                let edge = if pos.y < clip.top() + 24.0 { 14.0 } else if pos.y > clip.bottom() - 24.0 { -14.0 } else { 0.0 };
                if edge != 0.0 && pos.x >= clip.left() && pos.x <= clip.right() {
                    ui.scroll_with_delta_animation(vec2(0.0, edge), ScrollAnimation::none());
                    ctx.request_repaint();
                }
                let label = if tab.org.sel.len() == 1 { "1 sida".to_owned() } else { format!("{} sidor", tab.org.sel.len()) };
                let font = FontId::new(12.0, theme::medium());
                let galley = ui.painter().layout_no_wrap(label, font, p.on_accent);
                let r = Rect::from_min_size(pos + vec2(12.0, 8.0), galley.size() + vec2(14.0, 8.0));
                ui.painter().rect_filled(r, 6.0, p.accent);
                ui.painter().galley(r.min + vec2(7.0, 4.0), galley, p.on_accent);
            }
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            if !ctx.input(|i| i.pointer.primary_down()) {
                tab.org.drag_active = false;
                if let Some(before) = tab.org.drop.take() {
                    op = Some(OrgOp::Move { pages: tab.org.sel.clone(), before });
                }
            }
        }
    });
    if op.is_some() {
        return op;
    }
    if let Some(i) = jump {
        tab.view.go_to_page(i);
        // Miniaturen man klickade på syns redan; panelen ska inte hoppa.
        tab.last_page = Some(i);
    }
    None
}

/// Högerklicksmenyn på en miniatyr. `page` är den klickade sidan, som alltid ingår i urvalet.
fn page_menu(ui: &mut Ui, p: &Palette, tab: &PdfTab, count: usize, page: usize) -> Option<OrgOp> {
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut pages: Vec<usize> = tab.org.sel.iter().copied().collect();
    if pages.is_empty() {
        pages.push(page);
    }
    let all = pages.len() >= count;
    let first = pages[0];
    let last = pages[pages.len() - 1];
    let mut op = None;
    if menu_item(ui, p, "Rotera åt höger", "", None, true) {
        op = Some(OrgOp::Rotate { pages: pages.clone(), clockwise: true });
    }
    if menu_item(ui, p, "Rotera åt vänster", "", None, true) {
        op = Some(OrgOp::Rotate { pages: pages.clone(), clockwise: false });
    }
    menu_divider(ui, p);
    if menu_item(ui, p, "Ta bort", "Delete", None, !all) {
        op = Some(OrgOp::Delete(pages.clone()));
    }
    if menu_item(ui, p, "Extrahera till ny PDF…", "", None, true) {
        op = Some(OrgOp::Extract(pages.clone()));
    }
    menu_divider(ui, p);
    if menu_item(ui, p, "Infoga PDF före…", "", None, true) {
        op = Some(OrgOp::InsertAt(first));
    }
    if menu_item(ui, p, "Infoga PDF efter…", "", None, true) {
        op = Some(OrgOp::InsertAt(last + 1));
    }
    op
}

struct OutRow {
    path: Vec<usize>,
    depth: usize,
    title: String,
    dest: Option<Dest>,
    has_kids: bool,
    open: bool,
}

fn flatten_outline(items: &[OutlineItem], open_map: &HashMap<Vec<usize>, bool>, path: &mut Vec<usize>, depth: usize, out: &mut Vec<OutRow>) {
    for (i, it) in items.iter().enumerate() {
        path.push(i);
        let has_kids = !it.children.is_empty();
        let open = has_kids && open_map.get(path.as_slice()).copied().unwrap_or(it.open);
        out.push(OutRow { path: path.clone(), depth, title: it.title.clone(), dest: it.dest.clone(), has_kids, open });
        if open {
            flatten_outline(&it.children, open_map, path, depth + 1, out);
        }
        path.pop();
    }
}

fn outline(ui: &mut Ui, p: &Palette, tab: &mut PdfTab) {
    let cur_page = tab.view.current_page();
    let section = current_section(&tab.view.meta().outline, cur_page);
    let mut rows = Vec::new();
    flatten_outline(&tab.view.meta().outline, &tab.outline_open, &mut Vec::new(), 0, &mut rows);
    let section_changed = tab.last_section != section;
    let scroll_off = section_changed
        .then(|| section.as_ref().and_then(|s| rows.iter().position(|r| &r.path == s)))
        .flatten()
        .map(|i| (i as f32 * OUTLINE_ROW_H - 90.0).max(0.0));
    tab.last_section = section.clone();
    let mut goto: Option<Dest> = None;
    let mut toggle: Option<(Vec<usize>, bool)> = None;
    let mut area = ScrollArea::vertical().id_salt("kladd-pdf-outline").auto_shrink([false, false]);
    if let Some(off) = scroll_off {
        area = area.vertical_scroll_offset(off);
    }
    area.show_rows(ui, OUTLINE_ROW_H, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for row in &rows[range] {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), OUTLINE_ROW_H), Sense::click());
            let is_cur = section.as_ref() == Some(&row.path);
            if is_cur {
                ui.painter().rect_filled(rect, 6.0, p.accent.gamma_multiply(0.16));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 6.0, p.surface_hover);
            }
            let x0 = rect.left() + 4.0 + row.depth as f32 * 14.0;
            if row.has_kids {
                let arrow = Rect::from_center_size(pos2(x0 + 7.0, rect.center().y), vec2(18.0, OUTLINE_ROW_H));
                let ar = ui.interact(arrow, resp.id.with("arrow"), Sense::click());
                // Triangel: nedåt när raden är utfälld, åt höger när den är hopfälld.
                let c = arrow.center();
                let col = if ar.hovered() { p.text } else { p.muted };
                let pts = if row.open {
                    vec![c + vec2(-3.5, -2.0), c + vec2(3.5, -2.0), c + vec2(0.0, 3.0)]
                } else {
                    vec![c + vec2(-2.0, -3.5), c + vec2(-2.0, 3.5), c + vec2(3.0, 0.0)]
                };
                ui.painter().add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
                if ar.clicked() {
                    toggle = Some((row.path.clone(), !row.open));
                }
            }
            let tx = x0 + 18.0;
            let clip = Rect::from_min_max(pos2(tx, rect.top()), pos2(rect.right() - 4.0, rect.bottom()));
            let color = if is_cur { p.accent } else if row.dest.is_some() { p.text } else { p.muted };
            let font = FontId::new(13.0, if is_cur { theme::medium() } else { FontFamily::Proportional });
            ui.painter().with_clip_rect(clip).text(pos2(tx, rect.center().y), Align2::LEFT_CENTER, &row.title, font, color);
            if resp.clicked() {
                if let Some(d) = &row.dest {
                    goto = Some(d.clone());
                } else if row.has_kids {
                    toggle = Some((row.path.clone(), !row.open));
                }
            }
            if row.title.chars().count() > 22 {
                resp.on_hover_text(&row.title);
            }
        }
    });
    if let Some((path, open)) = toggle {
        tab.outline_open.insert(path, open);
    }
    if let Some(d) = goto {
        tab.view.go_to(&d);
    }
}

// ------------------------------------------------------------ sökfält

/// Sökfältet längst upp till höger i vyn. `anchor` är vyns yta.
pub fn find_bar(ctx: &egui::Context, p: &Palette, tab: &mut PdfTab, anchor: Rect) {
    if !tab.find.open {
        return;
    }
    let width = 500.0f32.min(anchor.width() - 24.0);
    let pos = pos2(anchor.right() - width - 22.0, anchor.top() + 12.0);
    let total_pages = tab.view.page_count();
    egui::Area::new(Id::unique("kladd-pdf-find")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        Frame::NONE
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.line))
            .corner_radius(11)
            .inner_margin(Margin::same(8))
            .shadow(ui.visuals().popup_shadow)
            .show(ui, |ui| {
                ui.set_width(width - 16.0);
                ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                ui.horizontal(|ui| {
                    let qid = Id::unique("kladd-pdf-find-query");
                    let field_w = (ui.available_width() - 26.0 * 5.0 - 128.0 - 4.0 * 7.0).max(60.0);
                    let q = ui.add(
                        TextEdit::singleline(&mut tab.find.query)
                            .id(qid)
                            .hint_text("Sök i dokumentet")
                            .desired_width(field_w)
                            .margin(Margin::symmetric(8, 5)),
                    );
                    if tab.find.focus {
                        q.request_focus();
                        let mut st = TextEditState::load(ctx, qid).unwrap_or_default();
                        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(tab.find.query.chars().count()))));
                        st.store(ctx, qid);
                        tab.find.focus = false;
                    }
                    let mut step: Option<bool> = None;
                    if q.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        step = Some(ui.input(|i| i.modifiers.shift));
                        q.request_focus();
                    }
                    let status = tab.view.find_status();
                    let text = find_status_text(&status, total_pages, !tab.find.query.is_empty());
                    let (cr, _) = ui.allocate_exact_size(vec2(128.0, 26.0), Sense::hover());
                    let bad = !tab.find.query.is_empty() && !status.searching && status.total == 0;
                    ui.painter().text(
                        cr.center(),
                        Align2::CENTER_CENTER,
                        text,
                        FontId::new(12.0, FontFamily::Proportional),
                        if bad { p.error } else { p.muted },
                    );
                    if icons::button(ui, p, Icon::CaseSensitive, 26.0, tab.find.case_sensitive, "Skilj på stora och små bokstäver", p.surface).clicked() {
                        tab.find.case_sensitive = !tab.find.case_sensitive;
                    }
                    if text_toggle(ui, p, "ab", tab.find.whole_word, "Hela ord").clicked() {
                        tab.find.whole_word = !tab.find.whole_word;
                    }
                    if icons::button(ui, p, Icon::ChevronUp, 26.0, false, "Föregående (Shift+Enter)", p.surface).clicked() {
                        step = Some(true);
                    }
                    if icons::button(ui, p, Icon::ChevronDown, 26.0, false, "Nästa (Enter)", p.surface).clicked() {
                        step = Some(false);
                    }
                    if icons::button(ui, p, Icon::Close, 26.0, false, "Stäng (Esc)", p.surface).clicked() {
                        tab.find.open = false;
                    }
                    // Skicka sökningen till visaren när något ändrats.
                    let key = (tab.find.query.clone(), tab.find.case_sensitive, tab.find.whole_word);
                    if tab.find.sent.as_ref() != Some(&key) {
                        if key.0.is_empty() {
                            tab.view.find_clear();
                        } else {
                            tab.view.find_set(&key.0, key.1, key.2);
                        }
                        tab.find.sent = Some(key);
                    }
                    if let Some(back) = step {
                        if !tab.find.query.is_empty() {
                            tab.view.find_next(back);
                        }
                    }
                });
            });
    });
    if !tab.find.open {
        tab.close_find();
    }
}

// ------------------------------------------------------------ dialoger

fn modal(ctx: &egui::Context, p: &Palette, id: &'static str) -> Modal {
    Modal::new(Id::unique(id))
        .backdrop_color(Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
        .frame(
            Frame::NONE
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(14)
                .inner_margin(Margin::same(24))
                .shadow(ctx.global_style().visuals.window_shadow),
        )
}

fn accent_button(p: &Palette, text: &str, min_w: f32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
        .fill(p.accent)
        .stroke(Stroke::NONE)
        .min_size(vec2(min_w, 32.0))
}

/// Dialogerna (Redigera, egenskaper, extern länk). Ger `Edit` eller `OpenUri` när användaren bekräftat.
pub fn dialogs(ctx: &egui::Context, p: &Palette, tab: &mut PdfTab, path: Option<&Path>) -> Option<Action> {
    let mut out = None;
    if tab.confirm_edit {
        let mut choice = None;
        let m = modal(ctx, p, "kladd-pdf-edit").show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Redigera PDF:en?").font(FontId::new(17.0, theme::semibold())).color(p.text));
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "PDF:en görs om till ett redigerbart dokument. Text, rubriker, listor, tabeller och bilder följer med men layouten kan ändras. Originalfilen ändras inte förrän du sparar.",
                )
                .color(p.muted),
            );
            ui.add_space(22.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.add(accent_button(p, "Redigera", 92.0)).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    choice = Some(true);
                }
                if ui.add(quiet_button("Avbryt", p).min_size(vec2(0.0, 32.0))).clicked() {
                    choice = Some(false);
                }
            });
        });
        if m.should_close() && choice.is_none() {
            choice = Some(false);
        }
        match choice {
            Some(true) => {
                tab.confirm_edit = false;
                out = Some(Action::Edit);
            }
            Some(false) => tab.confirm_edit = false,
            None => {}
        }
    }
    if let Some(uri) = tab.confirm_uri.clone() {
        let mut choice = None;
        let m = modal(ctx, p, "kladd-pdf-uri").show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Öppna länken i webbläsaren?").font(FontId::new(17.0, theme::semibold())).color(p.text));
            ui.add_space(8.0);
            let shown = if uri.chars().count() > 160 { format!("{}…", uri.chars().take(159).collect::<String>()) } else { uri.clone() };
            ui.add(egui::Label::new(RichText::new(shown).color(p.accent)).wrap());
            ui.add_space(22.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.add(accent_button(p, "Öppna", 84.0)).clicked() {
                    choice = Some(true);
                }
                if ui.add(quiet_button("Avbryt", p).min_size(vec2(0.0, 32.0))).clicked() {
                    choice = Some(false);
                }
            });
        });
        if m.should_close() && choice.is_none() {
            choice = Some(false);
        }
        if let Some(open) = choice {
            tab.confirm_uri = None;
            if open {
                out = Some(Action::OpenUri(uri));
            }
        }
    }
    if tab.props {
        let mut close = false;
        let m = modal(ctx, p, "kladd-pdf-props").show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new("Dokumentegenskaper").font(FontId::new(17.0, theme::semibold())).color(p.text));
            ui.add_space(12.0);
            let rows = property_rows(tab, path);
            for (k, v) in rows {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    let (r, _) = ui.allocate_exact_size(vec2(104.0, 18.0), Sense::hover());
                    ui.painter().text(r.left_top(), Align2::LEFT_TOP, k, FontId::new(13.5, FontFamily::Proportional), p.muted);
                    ui.add(egui::Label::new(RichText::new(v).color(p.text)).wrap());
                });
                ui.add_space(5.0);
            }
            ui.add_space(20.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(accent_button(p, "Stäng", 84.0)).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    close = true;
                }
            });
        });
        if close || m.should_close() {
            tab.props = false;
        }
    }
    out
}

/// Raderna i egenskapsdialogen (etikett, värde). Okända fält utelämnas.
fn property_rows(tab: &PdfTab, path: Option<&Path>) -> Vec<(&'static str, String)> {
    let info = &tab.view.meta().info;
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    let mut opt = |label: &'static str, v: &Option<String>| {
        if let Some(s) = v.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            rows.push((label, s.to_owned()));
        }
    };
    opt("Titel", &info.title);
    opt("Författare", &info.author);
    opt("Ämne", &info.subject);
    opt("Nyckelord", &info.keywords);
    opt("Skapad", &info.created);
    opt("Ändrad", &info.modified);
    opt("Skapad med", &info.creator);
    opt("PDF-skapare", &info.producer);
    if !info.version.is_empty() {
        rows.push(("PDF-version", info.version.clone()));
    }
    let size = tab.view.engine().page_size(tab.view.current_page());
    rows.push(("Sidstorlek", page_size_text(size.width, size.height)));
    rows.push(("Antal sidor", tab.view.page_count().to_string()));
    rows.push(("Filstorlek", format_size(tab.view.bytes().len() as u64)));
    if let Some(p) = path {
        rows.push(("Plats", p.display().to_string()));
    }
    rows
}

// ------------------------------------------------------------ statusrad

/// Höger del av statusraden: sida, PDF-version och zoom.
pub fn status(ui: &mut Ui, p: &Palette, font: &FontId, tab: &PdfTab) {
    let cur = tab.view.current_page();
    let label = |ui: &mut Ui, s: String| {
        ui.label(RichText::new(s).font(font.clone()).color(p.muted));
    };
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        label(ui, format!("{} %", (tab.view.zoom() * 100.0).round() as i32));
        let v = &tab.view.meta().info.version;
        if !v.is_empty() {
            label(ui, format!("PDF {v}"));
        }
        label(ui, page_status(&tab.view.page_label(cur), cur, tab.view.page_count()));
    });
}

// ------------------------------------------------------------ sida som bild

/// Renderar en sida i bakgrunden med en egen motor (visarens motor och dess kö rörs inte).
/// Resultatet kommer som en färdig bild att lägga på urklipp.
pub fn render_page_image(
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    page: usize,
    rotation: u16,
    ctx: egui::Context,
) -> Receiver<Result<egui::ColorImage, String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let res = (|| {
            let engine = PdfEngine::open(bytes, password.as_deref(), ctx.clone()).map_err(|e| match e {
                OpenError::Invalid(m) => m,
                _ => "Kunde inte öppna PDF:en".to_owned(),
            })?;
            // Dubbel upplösning mot 96 dpi.
            let req = RenderRequest { page, scale: 2.0 * 96.0 / 72.0, rotation, recolor: Recolor::None };
            engine.request(vec![req]);
            let start = Instant::now();
            loop {
                for ev in engine.poll() {
                    match ev {
                        super::engine::EngineEvent::Rendered { req, image } if req.page == page => return Ok(image),
                        super::engine::EngineEvent::Failed { page: pg, error } if pg == page => return Err(error),
                        _ => {}
                    }
                }
                if start.elapsed() > Duration::from_secs(20) {
                    return Err("Renderingen tog för lång tid".to_owned());
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        })();
        let _ = tx.send(res);
        ctx.request_repaint();
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn paper_names() {
        assert_eq!(paper_name(210.0, 297.0), Some("A4"));
        assert_eq!(paper_name(297.0, 210.0), Some("A4"));
        assert_eq!(paper_name(148.5, 210.0), Some("A5"));
        assert_eq!(paper_name(297.0, 420.0), Some("A3"));
        assert_eq!(paper_name(215.9, 279.4), Some("Letter"));
        assert_eq!(paper_name(215.9, 355.6), Some("Legal"));
        assert_eq!(paper_name(200.0, 300.0), None);
        // A4 i punkter är 595 × 842.
        assert_eq!(page_size_text(595.0, 842.0), "210 × 297 mm (A4, stående)");
        assert_eq!(page_size_text(842.0, 595.0), "297 × 210 mm (A4, liggande)");
        assert!(page_size_text(300.0, 300.0).ends_with("(stående)"));
    }

    #[test]
    fn page_input() {
        let none: Vec<String> = Vec::new();
        assert_eq!(parse_page_input("4", &none, 12), Some(3));
        assert_eq!(parse_page_input(" 1 ", &none, 12), Some(0));
        assert_eq!(parse_page_input("12", &none, 12), Some(11));
        assert_eq!(parse_page_input("0", &none, 12), None);
        assert_eq!(parse_page_input("13", &none, 12), None);
        assert_eq!(parse_page_input("", &none, 12), None);
        assert_eq!(parse_page_input("abc", &none, 12), None);
        assert_eq!(parse_page_input("-1", &none, 12), None);
        let l = labels(&["i", "ii", "iii", "1", "2"]);
        assert_eq!(parse_page_input("iv", &l, 5), None);
        assert_eq!(parse_page_input("II", &l, 5), Some(1));
        // Etiketten går före sidnumret: "1" är sida 4 här.
        assert_eq!(parse_page_input("1", &l, 5), Some(3));
        // Ett nummer utan etikett räknas som sidnummer.
        assert_eq!(parse_page_input("5", &l, 5), Some(4));
    }

    #[test]
    fn page_status_texts() {
        assert_eq!(page_status("iv", 3, 12), "iv (4 av 12)");
        assert_eq!(page_status("4", 3, 12), "Sida 4 av 12");
        assert_eq!(page_status("", 3, 12), "Sida 4 av 12");
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(532), "532 B");
        assert_eq!(format_size(1024), "1,0 kB");
        assert_eq!(format_size(12_800), "12,5 kB");
        assert_eq!(format_size(3_500_000), "3,3 MB");
        assert_eq!(format_size(250 * 1024 * 1024), "250 MB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5,0 GB");
    }

    #[test]
    fn memory_is_capped_and_ordered() {
        let mut list: Vec<(String, u32)> = Vec::new();
        for i in 0..150 {
            remember_memory(&mut list, format!("/f{i}.pdf"), i, MAX_MEMORY);
        }
        assert_eq!(list.len(), MAX_MEMORY);
        assert_eq!(list[0].0, "/f149.pdf");
        assert_eq!(list.last().unwrap().0, "/f50.pdf");
        // Samma fil igen: ersätts och hamnar först, utan dubbletter.
        remember_memory(&mut list, "/f100.pdf".into(), 999, MAX_MEMORY);
        assert_eq!(list.len(), MAX_MEMORY);
        assert_eq!(list[0], ("/f100.pdf".to_owned(), 999));
        assert_eq!(list.iter().filter(|(k, _)| k == "/f100.pdf").count(), 1);
    }

    #[test]
    fn zoom_choices() {
        assert_eq!(zoom_choice_target(ZoomChoice::Mode(ZoomMode::PageFit)), (ZoomMode::PageFit, None));
        assert_eq!(zoom_choice_target(ZoomChoice::Percent(150)), (ZoomMode::Custom, Some(1.5)));
        assert_eq!(current_zoom_choice(ZoomMode::Auto, 1.2), Some(ZoomChoice::Mode(ZoomMode::Auto)));
        assert_eq!(current_zoom_choice(ZoomMode::Custom, 1.25), Some(ZoomChoice::Percent(125)));
        assert_eq!(current_zoom_choice(ZoomMode::Custom, 1.37), None);
        assert_eq!(zoom_label(ZoomMode::Auto, 1.0), "Automatisk");
        assert_eq!(zoom_label(ZoomMode::ActualSize, 1.0), "Verklig storlek");
        assert_eq!(zoom_label(ZoomMode::PageWidth, 0.8), "Sidbredd");
        assert_eq!(zoom_label(ZoomMode::Custom, 2.0), "200 %");
        assert_eq!(zoom_label(ZoomMode::Custom, 1.37), "137 %");
        assert_eq!(ZOOM_CHOICES.len(), 12);
    }

    #[test]
    fn find_status_texts() {
        let s = |searching, current, total, done_pages| FindStatus { searching, current, total, done_pages };
        assert_eq!(find_status_text(&s(false, None, 0, 0), 40, false), "");
        assert_eq!(find_status_text(&s(true, None, 0, 12), 40, true), "Söker… (12/40 sidor)");
        assert_eq!(find_status_text(&s(false, None, 0, 40), 40, true), "Hittades inte");
        assert_eq!(find_status_text(&s(false, Some(2), 17, 40), 40, true), "3 av 17");
        assert_eq!(find_status_text(&s(true, Some(0), 3, 10), 40, true), "1 av 3+");
        assert_eq!(find_status_text(&s(false, None, 5, 40), 40, true), "5 träffar");
    }

    #[test]
    fn uris() {
        assert!(safe_uri("https://example.com/a?b=1"));
        assert!(safe_uri(" HTTP://example.com"));
        assert!(safe_uri("mailto:a@b.se"));
        assert!(!safe_uri("file:///etc/passwd"));
        assert!(!safe_uri("javascript:alert(1)"));
        assert!(!safe_uri("-rf"));
    }

    #[test]
    fn sections() {
        let item = |page: Option<usize>, children: Vec<OutlineItem>| OutlineItem {
            title: "x".into(),
            dest: page.map(|p| Dest { page: p, y: None }),
            children,
            open: false,
        };
        let outline = vec![item(Some(0), vec![item(Some(1), vec![]), item(Some(4), vec![])]), item(Some(6), vec![item(None, vec![])])];
        assert_eq!(current_section(&outline, 0), Some(vec![0]));
        assert_eq!(current_section(&outline, 2), Some(vec![0, 0]));
        assert_eq!(current_section(&outline, 5), Some(vec![0, 1]));
        assert_eq!(current_section(&outline, 9), Some(vec![1]));
        assert_eq!(current_section(&[], 3), None);
    }
}
