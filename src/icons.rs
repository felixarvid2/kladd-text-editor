//! Små ikoner som ritas direkt med linjer, så de blir skarpa i alla storlekar.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::theme::Palette;

#[derive(Clone, Copy)]
pub enum Icon {
    Plus,
    Close,
    Menu,
    Moon,
    Sun,
    Split,
    ChevronUp,
    ChevronDown,
    CaseSensitive,
    Minimize,
    Maximize,
    Restore,
    WindowClose,
    BulletList,
    NumberedList,
    Quote,
    // Bild, tabell, fotnot och innehållsförteckning ligger i menyn Infoga, ikonerna sparas.
    #[allow(dead_code)]
    Image,
    #[allow(dead_code)]
    Table,
    /// Fotnot: ett stort A med en upphöjd etta.
    #[allow(dead_code)]
    Footnote,
    TableRowAbove,
    TableRowBelow,
    TableColLeft,
    TableColRight,
    TableDelRow,
    TableDelCol,
    TableDelete,
    TableMerge,
    TableSplit,
    /// Ett pappersark med vikt hörn.
    Page,
    Undo,
    Redo,
    Minus,
    AlignLeft,
    AlignCenter,
    AlignRight,
    AlignJustify,
    /// Bokstaven A; färgstapeln under ritas av `color_button`.
    TextColor,
    /// Markeringspenna; färgstapeln under ritas av `color_button`.
    Highlight,
    /// Radbrytning av bild: bilden på en egen rad / texten flyter höger om / vänster om den.
    WrapInline,
    WrapLeft,
    WrapRight,
    /// Rubrikträd (navigeringsfönstret).
    Outline,
    /// Innehållsförteckning: rader med sidnummer till höger.
    #[allow(dead_code)]
    Toc,
}

pub fn paint(painter: &Painter, icon: Icon, c: Pos2, color: Color32, bg: Color32) {
    let s = Stroke::new(1.5, color);
    match icon {
        Icon::Plus => {
            painter.line_segment([c + vec2(-5.5, 0.0), c + vec2(5.5, 0.0)], s);
            painter.line_segment([c + vec2(0.0, -5.5), c + vec2(0.0, 5.5)], s);
        }
        Icon::Close => {
            let d = 3.8;
            painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], s);
            painter.line_segment([c + vec2(d, -d), c + vec2(-d, d)], s);
        }
        Icon::Menu => {
            for dy in [-4.5, 0.0, 4.5] {
                painter.line_segment([c + vec2(-6.0, dy), c + vec2(6.0, dy)], s);
            }
        }
        Icon::Moon => {
            painter.circle_filled(c, 6.5, color);
            painter.circle_filled(c + vec2(3.6, -3.0), 5.6, bg);
        }
        Icon::Sun => {
            painter.circle_stroke(c, 3.4, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d = Vec2::angled(a);
                painter.line_segment([c + d * 5.8, c + d * 7.6], s);
            }
        }
        Icon::Split => {
            let r = Rect::from_center_size(c, vec2(14.0, 11.0));
            painter.rect_stroke(r, 2.5, s, egui::StrokeKind::Middle);
            painter.line_segment([pos2(c.x, r.top()), pos2(c.x, r.bottom())], s);
        }
        Icon::ChevronUp => {
            painter.line_segment([c + vec2(-4.5, 2.5), c + vec2(0.0, -2.0)], s);
            painter.line_segment([c + vec2(0.0, -2.0), c + vec2(4.5, 2.5)], s);
        }
        Icon::ChevronDown => {
            painter.line_segment([c + vec2(-4.5, -2.0), c + vec2(0.0, 2.5)], s);
            painter.line_segment([c + vec2(0.0, 2.5), c + vec2(4.5, -2.0)], s);
        }
        Icon::Minimize => {
            painter.line_segment([c + vec2(-5.0, 0.5), c + vec2(5.0, 0.5)], s);
        }
        Icon::Maximize => {
            let r = Rect::from_center_size(c, vec2(10.0, 10.0));
            painter.rect_stroke(r, 1.5, s, egui::StrokeKind::Middle);
        }
        Icon::Restore => {
            let back = Rect::from_center_size(c + vec2(1.8, -1.8), vec2(8.5, 8.5));
            let front = Rect::from_center_size(c + vec2(-1.5, 1.5), vec2(8.5, 8.5));
            painter.rect_stroke(back, 1.5, s, egui::StrokeKind::Middle);
            painter.rect_filled(front, 1.5, bg);
            painter.rect_stroke(front, 1.5, s, egui::StrokeKind::Middle);
        }
        Icon::WindowClose => {
            let d = 4.6;
            painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], s);
            painter.line_segment([c + vec2(d, -d), c + vec2(-d, d)], s);
        }
        Icon::Minus => {
            painter.line_segment([c + vec2(-5.5, 0.0), c + vec2(5.5, 0.0)], s);
        }
        Icon::AlignLeft | Icon::AlignCenter | Icon::AlignRight | Icon::AlignJustify => {
            // Fyra linjer; längden och förskjutningen skiljer sig mellan justeringarna.
            let (full, short) = (13.0, 8.0);
            for (i, dy) in [-5.25, -1.75, 1.75, 5.25].into_iter().enumerate() {
                let w = if i % 2 == 1 && !matches!(icon, Icon::AlignJustify) { short } else { full };
                let x0 = match icon {
                    Icon::AlignCenter => -w / 2.0,
                    Icon::AlignRight => full / 2.0 - w,
                    _ => -full / 2.0,
                };
                painter.line_segment([c + vec2(x0, dy), c + vec2(x0 + w, dy)], s);
            }
        }
        Icon::TextColor => {
            // A med tvärstreck.
            let apex = c + vec2(0.0, -6.0);
            painter.line_segment([c + vec2(-4.8, 4.0), apex], s);
            painter.line_segment([apex, c + vec2(4.8, 4.0)], s);
            painter.line_segment([c + vec2(-2.8, 0.6), c + vec2(2.8, 0.6)], s);
        }
        Icon::Highlight => {
            // Snedställd penna: kropp, spets och ett streck vid spetsen.
            let body = [c + vec2(2.0, -6.5), c + vec2(6.0, -2.5), c + vec2(0.5, 3.0), c + vec2(-3.5, -1.0)];
            for i in 0..4 {
                painter.line_segment([body[i], body[(i + 1) % 4]], s);
            }
            painter.line_segment([c + vec2(-3.5, -1.0), c + vec2(-5.5, 3.5)], s);
            painter.line_segment([c + vec2(-5.5, 3.5), c + vec2(0.5, 3.0)], s);
        }
        Icon::WrapInline | Icon::WrapLeft | Icon::WrapRight => {
            let boxr = |x0: f32, x1: f32, y0: f32, y1: f32| {
                painter.rect_stroke(Rect::from_min_max(c + vec2(x0, y0), c + vec2(x1, y1)), 1.0, s, egui::StrokeKind::Middle)
            };
            let line = |x0: f32, x1: f32, dy: f32| painter.line_segment([c + vec2(x0, dy), c + vec2(x1, dy)], s);
            match icon {
                Icon::WrapInline => {
                    line(-6.5, 6.5, -6.0);
                    boxr(-3.5, 3.5, -3.0, 2.5);
                    line(-6.5, 6.5, 5.5);
                }
                Icon::WrapLeft => {
                    boxr(-6.5, -1.5, -6.0, -0.5);
                    line(1.5, 6.5, -5.0);
                    line(1.5, 6.5, -1.5);
                    line(-6.5, 6.5, 2.5);
                    line(-6.5, 6.5, 6.0);
                }
                _ => {
                    boxr(1.5, 6.5, -6.0, -0.5);
                    line(-6.5, -1.5, -5.0);
                    line(-6.5, -1.5, -1.5);
                    line(-6.5, 6.5, 2.5);
                    line(-6.5, 6.5, 6.0);
                }
            }
        }
        Icon::Outline => {
            // Indragna rader: rubrik, underrubrik, underrubrik, rubrik.
            for (dx, w, dy) in [(0.0, 11.0, -5.0), (3.5, 7.5, -1.7), (3.5, 7.5, 1.7), (0.0, 11.0, 5.0)] {
                let x0 = -6.0 + dx;
                painter.line_segment([c + vec2(x0, dy), c + vec2(x0 + w, dy)], s);
            }
        }
        Icon::Toc => {
            // Rader med punktlinjer och korta sidnummer längst till höger.
            for dy in [-4.5, 0.0, 4.5] {
                painter.line_segment([c + vec2(-6.5, dy), c + vec2(0.5, dy)], s);
                painter.circle_filled(c + vec2(2.5, dy + 0.6), 0.6, color);
                painter.line_segment([c + vec2(4.5, dy), c + vec2(6.5, dy)], s);
            }
        }
        Icon::BulletList => {
            for dy in [-4.5, 0.0, 4.5] {
                painter.circle_filled(c + vec2(-5.5, dy), 1.25, color);
                painter.line_segment([c + vec2(-2.0, dy), c + vec2(6.5, dy)], s);
            }
        }
        Icon::NumberedList => {
            // Små siffror till vänster, linjer till höger.
            for (i, dy) in [-4.5, 0.0, 4.5].into_iter().enumerate() {
                painter.text(
                    c + vec2(-5.2, dy),
                    egui::Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    egui::FontId::new(7.5, crate::theme::medium()),
                    color,
                );
                painter.line_segment([c + vec2(-1.5, dy), c + vec2(6.5, dy)], s);
            }
        }
        Icon::Quote => {
            // Lodrät stapel till vänster och textrader.
            painter.line_segment(
                [c + vec2(-6.0, -5.5), c + vec2(-6.0, 5.5)],
                Stroke::new(2.2, color),
            );
            painter.line_segment([c + vec2(-2.5, -4.0), c + vec2(6.5, -4.0)], s);
            painter.line_segment([c + vec2(-2.5, 0.0), c + vec2(6.5, 0.0)], s);
            painter.line_segment([c + vec2(-2.5, 4.0), c + vec2(3.0, 4.0)], s);
        }
        Icon::Image => {
            let r = Rect::from_center_size(c, vec2(14.0, 11.0));
            painter.rect_stroke(r, 2.0, s, egui::StrokeKind::Middle);
            painter.circle_filled(c + vec2(2.8, -1.8), 1.3, color);
            let m = [
                c + vec2(-5.0, 3.2),
                c + vec2(-1.6, -1.0),
                c + vec2(0.6, 1.8),
                c + vec2(2.0, 0.6),
                c + vec2(5.0, 3.2),
            ];
            for w in m.windows(2) {
                painter.line_segment([w[0], w[1]], s);
            }
        }
        Icon::Footnote => {
            painter.text(c + vec2(-2.5, 1.5), egui::Align2::CENTER_CENTER, "A", egui::FontId::new(12.5, crate::theme::medium()), color);
            painter.text(c + vec2(5.0, -4.5), egui::Align2::CENTER_CENTER, "1", egui::FontId::new(8.5, crate::theme::medium()), color);
        }
        Icon::Table => {
            let r = Rect::from_center_size(c, vec2(13.0, 12.0));
            painter.rect_stroke(r, 1.5, s, egui::StrokeKind::Middle);
            painter.line_segment([pos2(c.x, r.top()), pos2(c.x, r.bottom())], s);
            painter.line_segment([pos2(r.left(), c.y), pos2(r.right(), c.y)], s);
        }
        Icon::TableRowAbove | Icon::TableRowBelow | Icon::TableColLeft | Icon::TableColRight => {
            // Litet rutnät med ett plustecken på den sida där det nya kommer.
            let (dx, dy, w, h, px_, py_) = match icon {
                Icon::TableRowAbove => (0.0, 2.5, 14.0, 8.0, 0.0, -5.5),
                Icon::TableRowBelow => (0.0, -2.5, 14.0, 8.0, 0.0, 5.5),
                Icon::TableColLeft => (2.5, 0.0, 8.0, 12.0, -5.5, 0.0),
                _ => (-2.5, 0.0, 8.0, 12.0, 5.5, 0.0),
            };
            let r = Rect::from_center_size(c + vec2(dx, dy), vec2(w, h));
            let s1 = Stroke::new(1.2, color);
            painter.rect_stroke(r, 1.0, s1, egui::StrokeKind::Middle);
            painter.line_segment([pos2(r.center().x, r.top()), pos2(r.center().x, r.bottom())], s1);
            painter.line_segment([pos2(r.left(), r.center().y), pos2(r.right(), r.center().y)], s1);
            let pc = c + vec2(px_, py_);
            painter.line_segment([pc + vec2(-2.5, 0.0), pc + vec2(2.5, 0.0)], s);
            painter.line_segment([pc + vec2(0.0, -2.5), pc + vec2(0.0, 2.5)], s);
        }
        Icon::TableMerge | Icon::TableSplit => {
            // Två celler med pilar mot varandra (sammanfoga) eller ifrån varandra (dela).
            let r = Rect::from_center_size(c, vec2(16.0, 11.0));
            let s1 = Stroke::new(1.2, color);
            painter.rect_stroke(r, 1.0, s1, egui::StrokeKind::Middle);
            let split = matches!(icon, Icon::TableSplit);
            if split {
                painter.line_segment([pos2(c.x, r.top()), pos2(c.x, r.bottom())], s1);
            }
            for dir in [-1.0f32, 1.0] {
                // Pilen på varje sida om mitten; spetsen vid mitten (sammanfoga) eller kanten (dela).
                let (from, to) = if split { (3.0, 6.0) } else { (6.0, 3.0) };
                let (a, b) = (c + vec2(dir * from, 0.0), c + vec2(dir * to, 0.0));
                painter.line_segment([a, b], s);
                painter.line_segment([b, b + vec2(-dir * (to - from).signum() * 2.0, -2.0)], s);
                painter.line_segment([b, b + vec2(-dir * (to - from).signum() * 2.0, 2.0)], s);
            }
        }
        Icon::TableDelRow | Icon::TableDelCol | Icon::TableDelete => {
            // Rutnät; den borttagna raden/kolumnen fylld, hela tabellen överstruken.
            let r = Rect::from_center_size(c, vec2(14.0, 12.0));
            let s1 = Stroke::new(1.2, color);
            match icon {
                Icon::TableDelRow => {
                    let band = Rect::from_min_max(pos2(r.left(), r.center().y - 2.0), pos2(r.right(), r.center().y + 2.0));
                    painter.rect_filled(band, 0.0, color.gamma_multiply(0.5));
                }
                Icon::TableDelCol => {
                    let band = Rect::from_min_max(pos2(r.center().x - 2.5, r.top()), pos2(r.center().x + 2.5, r.bottom()));
                    painter.rect_filled(band, 0.0, color.gamma_multiply(0.5));
                }
                _ => {}
            }
            painter.rect_stroke(r, 1.0, s1, egui::StrokeKind::Middle);
            painter.line_segment([pos2(c.x, r.top()), pos2(c.x, r.bottom())], s1);
            painter.line_segment([pos2(r.left(), c.y), pos2(r.right(), c.y)], s1);
            if matches!(icon, Icon::TableDelete) {
                painter.line_segment([c + vec2(-6.0, -6.0), c + vec2(6.0, 6.0)], s);
                painter.line_segment([c + vec2(6.0, -6.0), c + vec2(-6.0, 6.0)], s);
            }
        }
        Icon::Page => {
            let r = Rect::from_center_size(c, vec2(11.0, 14.0));
            let fold = 4.0;
            painter.add(egui::Shape::closed_line(
                vec![r.left_top(), pos2(r.right() - fold, r.top()), pos2(r.right(), r.top() + fold), r.right_bottom(), r.left_bottom()],
                s,
            ));
            painter.line_segment([pos2(r.right() - fold, r.top()), pos2(r.right() - fold, r.top() + fold)], s);
            painter.line_segment([pos2(r.right() - fold, r.top() + fold), pos2(r.right(), r.top() + fold)], s);
        }
        Icon::Undo | Icon::Redo => {
            // Redo är Undo speglad i x.
            let k = if matches!(icon, Icon::Undo) { 1.0 } else { -1.0 };
            let at = |x: f32, y: f32| c + vec2(x * k, y);
            let mid = vec2(0.5, 1.0);
            let r = 4.5;
            // Båge från överkanten (höger om pilspetsen) runt höger sida till nederkanten.
            let n = 10;
            let mut pts = vec![at(-5.0, -3.5)];
            for i in 0..=n {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / n as f32;
                pts.push(at(mid.x + r * a.cos(), mid.y + r * a.sin()));
            }
            pts.push(at(-3.5, 5.5));
            for w in pts.windows(2) {
                painter.line_segment([w[0], w[1]], s);
            }
            // Pilspets.
            painter.line_segment([at(-5.0, -3.5), at(-2.2, -6.0)], s);
            painter.line_segment([at(-5.0, -3.5), at(-2.2, -1.0)], s);
        }
        Icon::CaseSensitive => {
            painter.text(
                c,
                egui::Align2::CENTER_CENTER,
                "Aa",
                egui::FontId::new(12.0, crate::theme::medium()),
                color,
            );
        }
    }
}

/// En tyst ikonknapp: syns bara som yta när man hovrar över den.
pub fn button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    size: f32,
    active: bool,
    tooltip: &str,
    base: Color32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let response = response.on_hover_text(crate::keys::shortcut(tooltip));
    let hovered = response.hovered();
    let bg = if hovered { p.surface_hover } else { Color32::TRANSPARENT };
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 7.0, bg);
        let color = if active {
            p.accent
        } else if hovered {
            p.text
        } else {
            p.muted
        };
        let under = if hovered { p.surface_hover } else { base };
        paint(ui.painter(), icon, rect.center(), color, under);
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(0.5),
                7.0,
                Stroke::new(1.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }
    }
    response
}

/// Fönsterknapp i titelraden. Stäng-knappen blir röd när man hovrar.
pub fn window_button(ui: &mut Ui, p: &Palette, icon: Icon, tooltip: &str, danger: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(36.0, 30.0), Sense::click());
    let response = response.on_hover_text(crate::keys::shortcut(tooltip));
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let pressed = response.is_pointer_button_down_on();
        let (bg, fg) = match (danger, hovered) {
            (true, true) => (
                if pressed { Color32::from_rgb(0xB8, 0x2F, 0x35) } else { Color32::from_rgb(0xD9, 0x3D, 0x44) },
                Color32::WHITE,
            ),
            (false, true) => (p.surface_hover, p.text),
            _ => (Color32::TRANSPARENT, p.muted),
        };
        ui.painter().rect_filled(rect, 8.0, bg);
        let under = if hovered { bg } else { p.chrome };
        paint(ui.painter(), icon, rect.center(), fg, under);
    }
    response
}

/// Hur bokstaven i en `glyph_toggle` ritas.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GlyphStyle {
    Bold,
    Italic,
    Underline,
    Strike,
}

/// Gemensam ritning för formateringsradens växlingsknappar (28x28).
fn toggle_frame(ui: &mut Ui, p: &Palette, active: bool, tooltip: &str) -> (Rect, Response, Color32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let response = response.on_hover_text(crate::keys::shortcut(tooltip));
    let hovered = response.hovered();
    if ui.is_rect_visible(rect) {
        let fill = if active {
            p.accent.gamma_multiply(0.18)
        } else if hovered {
            p.surface_hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 4.0, fill);
        if active {
            ui.painter().rect_stroke(
                rect.shrink(0.5),
                4.0,
                Stroke::new(1.0, p.accent.gamma_multiply(0.5)),
                egui::StrokeKind::Inside,
            );
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(0.5),
                4.0,
                Stroke::new(1.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }
    }
    let color = if active { p.accent } else { p.text };
    (rect, response, color)
}

/// Knapp med en bokstav (F, K, U, S) för formateringsraden. `active` ger tonad bakgrund med accentkant.
pub fn glyph_toggle(
    ui: &mut Ui,
    p: &Palette,
    glyph: &str,
    style: GlyphStyle,
    active: bool,
    tooltip: &str,
) -> Response {
    let (rect, response, color) = toggle_frame(ui, p, active, tooltip);
    if ui.is_rect_visible(rect) {
        let family = match style {
            GlyphStyle::Bold => crate::theme::semibold(),
            _ => egui::FontFamily::Proportional,
        };
        let mut job = egui::text::LayoutJob::default();
        job.append(
            glyph,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::new(14.5, family),
                color,
                italics: style == GlyphStyle::Italic,
                ..Default::default()
            },
        );
        let galley = ui.painter().layout_job(job);
        let top_left = rect.center() - galley.size() / 2.0;
        // Pixelrunda så att texten blir skarp.
        let top_left = pos2(top_left.x.round(), top_left.y.round());
        ui.painter().galley(top_left, galley.clone(), color);
        let w = galley.size().x;
        let s = Stroke::new(1.3, color);
        match style {
            GlyphStyle::Underline => {
                let y = (top_left.y + galley.size().y - 1.5).round() + 0.5;
                ui.painter().line_segment([pos2(top_left.x, y), pos2(top_left.x + w, y)], s);
            }
            GlyphStyle::Strike => {
                let y = rect.center().y.round() + 0.5;
                ui.painter()
                    .line_segment([pos2(top_left.x - 1.0, y), pos2(top_left.x + w + 1.0, y)], s);
            }
            _ => {}
        }
    }
    response
}

/// Ikonknapp som kan vara intryckt (28x28) för formateringsraden.
pub fn toggle_button(ui: &mut Ui, p: &Palette, icon: Icon, active: bool, tooltip: &str) -> Response {
    let (rect, response, color) = toggle_frame(ui, p, active, tooltip);
    if ui.is_rect_visible(rect) {
        let under = if active {
            p.accent.gamma_multiply(0.18)
        } else if response.hovered() {
            p.surface_hover
        } else {
            p.chrome
        };
        paint(ui.painter(), icon, rect.center(), color, under);
    }
    response
}

/// Delad knapp för text- och markeringsfärg: själva knappen (ikon med färgstapel under) och en
/// liten pil som öppnar färgväljaren. Returnerar (knapp, pil).
pub fn color_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    color: Color32,
    tooltip: &str,
    arrow_tooltip: &str,
) -> (Response, Response) {
    let (main, arrow) = (vec2(26.0, 28.0), vec2(13.0, 28.0));
    let space = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 0.0;
    let (r1, main_resp) = ui.allocate_exact_size(main, Sense::click());
    let (r2, arrow_resp) = ui.allocate_exact_size(arrow, Sense::click());
    ui.spacing_mut().item_spacing.x = space;
    let main_resp = main_resp.on_hover_text(crate::keys::shortcut(tooltip));
    let arrow_resp = arrow_resp.on_hover_text(crate::keys::shortcut(arrow_tooltip));
    if ui.is_rect_visible(r1) {
        for (r, resp, round) in [
            (r1, &main_resp, CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }),
            (r2, &arrow_resp, CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 }),
        ] {
            let open = resp.is_pointer_button_down_on() || (resp.hovered() && ui.input(|i| i.pointer.any_down()));
            if resp.hovered() || open {
                ui.painter().rect_filled(r, round, p.surface_hover);
            }
        }
        let under = if main_resp.hovered() { p.surface_hover } else { p.chrome };
        let c = r1.center() + vec2(1.0, -2.0);
        paint(ui.painter(), icon, c, p.text, under);
        let bar = Rect::from_center_size(pos2(r1.center().x + 1.0, r1.bottom() - 4.5), vec2(14.0, 3.0));
        ui.painter().rect_filled(bar, 0.5, color);
        // Vita och mycket ljusa färger behöver en kant för att synas mot knappen.
        ui.painter().rect_stroke(bar, 0.5, Stroke::new(0.5, p.line), egui::StrokeKind::Outside);
        let ac = r2.center() + vec2(-1.0, 0.0);
        paint(ui.painter(), Icon::ChevronDown, ac, p.muted, under);
    }
    (main_resp, arrow_resp)
}
