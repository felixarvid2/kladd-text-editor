//! Små ikoner som ritas direkt med linjer, så de blir skarpa i alla storlekar.

use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};

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
    let response = response.on_hover_text(tooltip);
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
    let response = response.on_hover_text(tooltip);
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
