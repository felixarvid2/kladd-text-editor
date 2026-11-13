//! Bilder i dokumentläget: visningsstorlek, markering, handtag för storleksändring med musen,
//! justering och en liten flytande verktygsrad. Allt här är rena hjälpfunktioner plus ritning;
//! själva textändringarna går via `markup` och `KladdApp::word_edit` (ett ångra-steg var).

use std::ops::Range;

use egui::{Color32, CursorIcon, FontId, Id, LayerId, Order, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use super::markup::{self, Align, Float, ImageRef};
use crate::i18n::t;
use crate::icons::{self, Icon};
use crate::theme::Palette;

/// Minsta bildstorlek vid dragning (px).
pub const MIN_SIZE: f32 = 16.0;
/// Handtagens sidlängd och träffradie (skärmpx).
const HANDLE: f32 = 7.0;
const HIT: f32 = 7.0;

/// Nyckeln layouten och ritningen slår upp bildstorleken med: bara källan för `![]()`,
/// annars också bredd och höjd (samma bild kan förekomma med olika storlek).
pub fn size_key(img: &ImageRef) -> String {
    match (img.width, img.height) {
        (None, None) => img.src.clone(),
        (w, h) => format!("{}\u{1}{}\u{1}{}", img.src, w.unwrap_or(0.0), h.unwrap_or(0.0)),
    }
}

/// Visningsstorlek vid zoom `z`: `natural` är bildens egna storlek (CSS-px), `max_w` textbredden (CSS-px).
/// Bara bredd eller bara höjd behåller proportionerna; för bred bild krymps till `max_w`.
pub fn display_size(natural: Vec2, img: &ImageRef, max_w: f32, z: f32) -> Vec2 {
    let asp = if natural.y > 0.0 { natural.x / natural.y } else { 1.0 };
    let valid = |v: Option<f32>| v.filter(|v| v.is_finite() && *v >= 1.0);
    let (w, h) = match (valid(img.width), valid(img.height)) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, w / asp),
        (None, Some(h)) => (h * asp, h),
        (None, None) => (natural.x, natural.y),
    };
    let k = if w > max_w { max_w / w } else { 1.0 };
    vec2(w * k, h * k) * z
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
}

impl Handle {
    pub const ALL: [Handle; 8] =
        [Handle::Nw, Handle::N, Handle::Ne, Handle::E, Handle::Se, Handle::S, Handle::Sw, Handle::W];

    /// Riktning (-1, 0, 1) i x och y som handtaget flyttar kanten.
    fn dir(self) -> (f32, f32) {
        match self {
            Handle::Nw => (-1.0, -1.0),
            Handle::N => (0.0, -1.0),
            Handle::Ne => (1.0, -1.0),
            Handle::E => (1.0, 0.0),
            Handle::Se => (1.0, 1.0),
            Handle::S => (0.0, 1.0),
            Handle::Sw => (-1.0, 1.0),
            Handle::W => (-1.0, 0.0),
        }
    }

    pub fn cursor(self) -> CursorIcon {
        match self {
            Handle::Nw | Handle::Se => CursorIcon::ResizeNwSe,
            Handle::Ne | Handle::Sw => CursorIcon::ResizeNeSw,
            Handle::N | Handle::S => CursorIcon::ResizeVertical,
            Handle::E | Handle::W => CursorIcon::ResizeHorizontal,
        }
    }
}

/// Handtagens mittpunkter: hörn och kantmitter.
pub fn handle_points(r: Rect) -> [(Handle, egui::Pos2); 8] {
    Handle::ALL.map(|h| {
        let (dx, dy) = h.dir();
        (h, pos2(r.center().x + dx * r.width() / 2.0, r.center().y + dy * r.height() / 2.0))
    })
}

/// Handtaget under `pos` (närmast om flera ligger nära; små bilder har täta handtag).
pub fn hit_handle(r: Rect, pos: egui::Pos2) -> Option<Handle> {
    handle_points(r)
        .into_iter()
        .map(|(h, c)| (h, c.distance(pos)))
        .filter(|(_, d)| *d <= HIT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(h, _)| h)
}

/// Ny storlek när `handle` dras `delta` från en bild med storleken `orig`. `lock` bevarar
/// proportionerna (hörn: den största relativa ändringen styr; kanter: den andra sidan följer med).
/// Minst `MIN_SIZE` på båda sidor, bredd högst `max_w`.
pub fn resize(handle: Handle, orig: Vec2, delta: Vec2, lock: bool, max_w: f32) -> Vec2 {
    let (dx, dy) = handle.dir();
    let (w0, h0) = (orig.x.max(1.0), orig.y.max(1.0));
    let tw = w0 + dx * delta.x;
    let th = h0 + dy * delta.y;
    if lock {
        let kx = tw / w0;
        let ky = th / h0;
        let k = if dy == 0.0 {
            kx
        } else if dx == 0.0 {
            ky
        } else if (kx - 1.0).abs() >= (ky - 1.0).abs() {
            kx
        } else {
            ky
        };
        let lo = (MIN_SIZE / w0).max(MIN_SIZE / h0);
        let hi = (max_w / w0).max(lo);
        let k = k.clamp(lo, hi);
        vec2(w0 * k, h0 * k)
    } else {
        let w = if dx != 0.0 { tw } else { w0 };
        let h = if dy != 0.0 { th } else { h0 };
        vec2(w.clamp(MIN_SIZE, max_w.max(MIN_SIZE)), h.max(MIN_SIZE))
    }
}

/// Bildens ruta efter storleksändring: motstående hörn/kant står still (mitten i den andra leden vid kanter).
pub fn resized_rect(orig: Rect, handle: Handle, size: Vec2) -> Rect {
    let (dx, dy) = handle.dir();
    let x0 = match dx {
        d if d < 0.0 => orig.right() - size.x,
        d if d > 0.0 => orig.left(),
        _ => orig.center().x - size.x / 2.0,
    };
    let y0 = match dy {
        d if d < 0.0 => orig.bottom() - size.y,
        d if d > 0.0 => orig.top(),
        _ => orig.center().y - size.y / 2.0,
    };
    Rect::from_min_size(pos2(x0, y0), size)
}

/// Tar bort bildbiten `range` ur texten, och raden om den blir tom.
pub fn remove_image(text: &str, range: Range<usize>) -> markup::Edit {
    let ls = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let le = text[range.end..].find('\n').map_or(text.len(), |i| range.end + i);
    let rest_empty = text[ls..range.start].trim().is_empty() && text[range.end..le].trim().is_empty();
    let (a, b) = if rest_empty {
        if le < text.len() {
            (ls, le + 1)
        } else if ls > 0 {
            (ls - 1, le) // sista raden: ta radbrytningen före
        } else {
            (ls, le)
        }
    } else {
        (range.start, range.end)
    };
    let mut t = String::with_capacity(text.len());
    t.push_str(&text[..a]);
    t.push_str(&text[b..]);
    markup::Edit { text: t, sel: a..a }
}

/// Sätter bredd och höjd uttryckligen (fri storleksändring): `set_image_size` ger bredden och
/// höjden skrivs över i samma tagg.
pub fn set_free_size(text: &str, range: Range<usize>, w: f32, h: f32) -> markup::Edit {
    let e = markup::set_image_size(text, range, Some(w));
    let tag = &e.text[e.sel.clone()];
    if !tag.starts_with("<img") || !tag.ends_with('>') {
        return e;
    }
    let mut inner = tag[..tag.len() - 1].to_string();
    if let Some(i) = inner.find(" height=\"") {
        let end = inner[i + 9..].find('"').map_or(inner.len(), |j| i + 9 + j + 1);
        inner.replace_range(i..end, "");
    }
    inner.push_str(&format!(" height=\"{}\">", h.round().max(1.0)));
    let mut t = String::with_capacity(e.text.len() + 16);
    t.push_str(&e.text[..e.sel.start]);
    t.push_str(&inner);
    t.push_str(&e.text[e.sel.end..]);
    markup::Edit { sel: e.sel.start..e.sel.start + inner.len(), text: t }
}

/// En ritad bild: ruta på skärmen, bitens byteområde och löpnummer bland dokumentets bilder.
pub struct Painted {
    /// Raden/storleken bilden ritas i: för en flytande bild rutan vid textkanten.
    pub rect: Rect,
    /// Bildens `align` i Markdown-texten (för verktygsradens knappar och flyttens markering).
    pub align: Option<Float>,
    pub range: Range<usize>,
    pub index: usize,
    pub uri: String,
}

/// Något användaren bett om; utförs av `word_page` via `word_edit` efter ritningen.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Align(Align),
    Original,
    Delete,
    /// Ny storlek i CSS-px; `h` bara vid fri storleksändring (Skift).
    Resize { w: f32, h: Option<f32> },
    /// Radbrytning: texten flyter runt bilden (`Some`) eller bilden är en egen rad (`None`).
    Float(Option<Float>),
    /// Flytta bilden till före/efter raden som innehåller byteläget `target`.
    Move { target: usize, after: bool },
}

/// Var en bild som dras skulle hamna: byteläge i raden, före eller efter den, skärm-y för
/// markeringslinjen och textspaltens kanter (skärm-x).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drop {
    pub target: usize,
    pub after: bool,
    pub y: f32,
    pub x0: f32,
    pub x1: f32,
}

/// Pågående flytt av en bild med musen.
struct Moving {
    /// Pekarens avstånd från bildens hörn när dragningen började.
    grab: Vec2,
    size: Vec2,
    uri: String,
    align: Option<Float>,
}

struct Drag {
    handle: Handle,
    index: usize,
    orig: Rect,
    origin: egui::Pos2,
    /// Senaste storleken medan knappen var nere (vid släpp saknas pekarens startpunkt).
    last: Vec2,
}

/// Läget mellan bildrutor: vald bild (löpnummer), pågående dragning och senast kända område.
#[derive(Default)]
pub struct State {
    pub doc_id: u64,
    pub selected: Option<usize>,
    /// Vald bilds byteområde i senaste bildrutan (för formateringsradens justering).
    pub range: Option<Range<usize>>,
    drag: Option<Drag>,
    mv: Option<Moving>,
}

impl State {
    pub fn deselect(&mut self) {
        self.selected = None;
        self.range = None;
        self.drag = None;
        self.mv = None;
    }

    /// Pågår en dragning (storlek eller flytt)?
    pub fn busy(&self) -> bool {
        self.drag.is_some() || self.mv.is_some()
    }
}

/// Hanterar klick, handtag, tangenter och verktygsraden för alla ritade bilder.
/// `n_images` är antalet bildbitar i dokumentet; `z` zoomen; `max_w` textbredden i skärmpx.
/// Returnerar en åtgärd (högst en per bildruta) som anroparen kör.
pub fn ui(
    ui: &mut Ui,
    st: &mut State,
    doc_id: u64,
    painted: &[Painted],
    n_images: usize,
    (z, max_w): (f32, f32),
    editor_id: Id,
    p: &Palette,
    drop_at: &dyn Fn(egui::Pos2) -> Option<Drop>,
) -> Option<Act> {
    if st.doc_id != doc_id || st.selected.is_some_and(|i| i >= n_images) {
        st.deselect();
    }
    st.doc_id = doc_id;
    let mut act = None;
    let ctx = ui.ctx().clone();
    let shift = ui.input(|i| i.modifiers.shift);

    // Klick/dragning på en bild markerar den; handtagen på den valda startar storleksändring.
    for im in painted {
        let selected = st.selected == Some(im.index);
        let area = if selected { im.rect.expand(HIT) } else { im.rect };
        let r = ui.interact(area, editor_id.with(("kladd-img", im.index)), Sense::click_and_drag());
        if selected && st.drag.is_none() {
            if let Some(h) = ctx.pointer_hover_pos().filter(|q| area.contains(*q)).and_then(|q| hit_handle(im.rect, q)) {
                ctx.set_cursor_icon(h.cursor());
            }
        }
        if (r.clicked() || r.is_pointer_button_down_on() || r.drag_started()) && !selected && st.drag.is_none() {
            st.selected = Some(im.index);
            ctx.memory_mut(|m| m.surrender_focus(editor_id));
        }
        if r.drag_started() && st.selected == Some(im.index) {
            let origin = ctx.input(|i| i.pointer.press_origin());
            if let Some(h) = origin.and_then(|o| hit_handle(im.rect, o)) {
                st.drag = Some(Drag { handle: h, index: im.index, orig: im.rect, origin: origin.unwrap_or_default(), last: im.rect.size() });
            } else if let Some(o) = origin.filter(|o| im.rect.contains(*o)) {
                // Bildens mitt: flytta den.
                st.mv = Some(Moving { grab: o - im.rect.min, size: im.rect.size(), uri: im.uri.clone(), align: im.align });
            }
        }
    }

    // Pågående storleksändring: förhandsruta och mått; vid släpp en åtgärd.
    let mut released = None;
    if let Some(d) = &mut st.drag {
        let pos = ctx.pointer_latest_pos().unwrap_or(d.orig.center());
        let down = ctx.input(|i| i.pointer.primary_down());
        if down {
            d.last = resize(d.handle, d.orig.size(), pos - d.origin, !shift, max_w);
        }
        let size = d.last;
        let rect = resized_rect(d.orig, d.handle, size);
        ctx.set_cursor_icon(d.handle.cursor());
        let layer = LayerId::new(Order::Tooltip, Id::unique("kladd-img-preview"));
        let pt = ctx.layer_painter(layer);
        if let Some(im) = painted.iter().find(|i| i.index == d.index) {
            let mut child = ui.new_child(egui::UiBuilder::new().layer_id(layer).max_rect(rect));
            egui::Image::new(im.uri.clone()).tint(Color32::from_white_alpha(150)).paint_at(&mut child, rect);
        }
        pt.rect_stroke(rect, 0.0, Stroke::new(1.5, p.accent), egui::StrokeKind::Middle);
        let galley = pt.layout_no_wrap(
            format!("{} × {} px", (size.x / z).round(), (size.y / z).round()),
            FontId::proportional(12.0),
            p.on_accent,
        );
        let at = pos + vec2(14.0, 16.0);
        pt.rect_filled(Rect::from_min_size(at, galley.size()).expand(4.0), 3.0, p.accent);
        pt.galley(at, galley, p.on_accent);
        if !down {
            released = Some(Act::Resize { w: size.x / z, h: shift.then_some(size.y / z) });
        }
        ctx.request_repaint();
    }
    if released.is_some() {
        st.drag = None;
        act = released;
    }

    // Pågående flytt: spökbild, markeringslinje (och bildens ruta vid kanten för flytande bilder).
    if let Some(m) = st.mv.take() {
        let pos = ctx.pointer_latest_pos().unwrap_or_default();
        let down = ctx.input(|i| i.pointer.primary_down());
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if !esc {
            let target = drop_at(pos);
            ctx.set_cursor_icon(if target.is_some() { CursorIcon::Grabbing } else { CursorIcon::NotAllowed });
            let layer = LayerId::new(Order::Tooltip, Id::unique("kladd-img-move"));
            let pt = ctx.layer_painter(layer);
            let ghost = Rect::from_min_size(pos - m.grab, m.size);
            let mut child = ui.new_child(egui::UiBuilder::new().layer_id(layer).max_rect(ghost));
            egui::Image::new(m.uri.clone()).tint(Color32::from_white_alpha(110)).paint_at(&mut child, ghost);
            pt.rect_stroke(ghost, 0.0, Stroke::new(1.0, p.accent), egui::StrokeKind::Middle);
            if let Some(d) = target {
                pt.rect_filled(Rect::from_min_max(pos2(d.x0, d.y - 1.0), pos2(d.x1, d.y + 1.0)), 1.0, p.accent);
                for x in [d.x0, d.x1] {
                    pt.circle_filled(pos2(x, d.y), 3.0, p.accent);
                }
                if let Some(side) = m.align {
                    let x = if side == Float::Left { d.x0 } else { d.x1 - m.size.x };
                    let r = Rect::from_min_size(pos2(x, d.y), m.size);
                    pt.rect_filled(r, 0.0, p.accent.gamma_multiply(0.12));
                    pt.rect_stroke(r, 0.0, Stroke::new(1.5, p.accent), egui::StrokeKind::Inside);
                }
            }
            // Rulla när pekaren närmar sig kanten av fönstret.
            let view = ui.clip_rect();
            let edge = 36.0;
            let dy = if pos.y < view.top() + edge {
                (view.top() + edge - pos.y).min(edge) * 0.5
            } else if pos.y > view.bottom() - edge {
                -((pos.y - (view.bottom() - edge)).min(edge) * 0.5)
            } else {
                0.0
            };
            if dy != 0.0 {
                ui.scroll_with_delta(vec2(0.0, dy));
            }
            if down {
                st.mv = Some(m);
            } else if let Some(d) = target {
                act = Some(Act::Move { target: d.target, after: d.after });
            }
        }
        ctx.request_repaint();
    }

    // Ram, handtag och verktygsrad för den valda bilden.
    let mut bar_rect = None;
    st.range = None;
    if let Some(im) = st.selected.and_then(|sel| painted.iter().find(|i| i.index == sel)) {
        st.range = Some(im.range.clone());
        paint_selection(ui, im.rect, p);
        if !st.busy() && act.is_none() {
            let (a, rect) = toolbar(ui, im.rect, p, im.align);
            act = a;
            bar_rect = Some(rect);
        }
    }

    // Tryck utanför bilder och verktygsraden avmarkerar.
    if ctx.input(|i| i.pointer.primary_pressed()) && !st.busy() {
        let pos = ctx.pointer_interact_pos();
        let on_img = pos.is_some_and(|q| {
            painted.iter().any(|i| i.rect.expand(if st.selected == Some(i.index) { HIT } else { 0.0 }).contains(q))
        });
        let on_bar = pos.is_some_and(|q| bar_rect.is_some_and(|r| r.contains(q)));
        if !on_img && !on_bar {
            st.deselect();
        }
    }

    // Tangenter: bara när ingen annan ruta har fokus (markeringen tog fokus från texten).
    if st.selected.is_some() && ctx.memory(|m| m.focused().is_none()) {
        let (esc, del) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                    | i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace),
            )
        });
        if esc {
            st.deselect();
        } else if del {
            act = Some(Act::Delete);
        }
    }
    act
}

/// 1 px accentram och åtta små kvadratiska handtag.
fn paint_selection(ui: &Ui, r: Rect, p: &Palette) {
    let pt = ui.painter();
    pt.rect_stroke(r, 0.0, Stroke::new(1.0, p.accent), egui::StrokeKind::Outside);
    for (_, c) in handle_points(r) {
        let hr = Rect::from_center_size(c, Vec2::splat(HANDLE));
        pt.rect_filled(hr, 0.0, Color32::WHITE);
        pt.rect_stroke(hr, 0.0, Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
    }
}

/// Flytande verktygsrad ovanför (annars under) bilden.
fn toolbar(ui: &mut Ui, r: Rect, p: &Palette, float: Option<Float>) -> (Option<Act>, Rect) {
    let mut act = None;
    let screen = ui.ctx().content_rect();
    let pos = if r.top() - 40.0 > screen.top() { pos2(r.left(), r.top() - 38.0) } else { pos2(r.left(), r.bottom() + 10.0) };
    let area = egui::Area::new(Id::unique("kladd-img-toolbar"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .constrain(true)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).fill(p.chrome).stroke(Stroke::new(1.0, p.line)).inner_margin(4.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (label, a) in [
                        (t("Vänster"), Act::Align(Align::Left)),
                        (t("Centrera"), Act::Align(Align::Center)),
                        (t("Höger"), Act::Align(Align::Right)),
                        (t("Originalstorlek"), Act::Original),
                        (t("Ta bort"), Act::Delete),
                    ] {
                        if ui.add(egui::Button::new(label).small()).clicked() {
                            act = Some(a);
                        }
                    }
                    ui.add_space(4.0);
                    // Radbrytning: bilden på en egen rad, eller text som flyter runt den.
                    for (icon, to, tip) in [
                        (Icon::WrapInline, None, t("Radbrytning: I texten")),
                        (Icon::WrapLeft, Some(Float::Left), t("Radbrytning: Vänster (text till höger om bilden)")),
                        (Icon::WrapRight, Some(Float::Right), t("Radbrytning: Höger (text till vänster om bilden)")),
                    ] {
                        if icons::toggle_button(ui, p, icon, float == to, tip).clicked() {
                            act = Some(Act::Float(to));
                        }
                    }
                });
            });
        });
    (act, area.response.rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: Option<f32>, h: Option<f32>) -> ImageRef {
        ImageRef { alt: "a".into(), src: "x.png".into(), width: w, height: h, align: None }
    }

    #[test]
    fn visningsstorlek() {
        let nat = vec2(400.0, 200.0);
        assert_eq!(display_size(nat, &img(None, None), 1000.0, 1.0), nat);
        assert_eq!(display_size(nat, &img(None, None), 300.0, 2.0), vec2(600.0, 300.0));
        assert_eq!(display_size(nat, &img(Some(100.0), None), 1000.0, 1.0), vec2(100.0, 50.0));
        assert_eq!(display_size(nat, &img(None, Some(100.0)), 1000.0, 1.0), vec2(200.0, 100.0));
        assert_eq!(display_size(nat, &img(Some(100.0), Some(80.0)), 1000.0, 1.5), vec2(150.0, 120.0));
        assert_eq!(display_size(nat, &img(Some(800.0), None), 400.0, 1.0), vec2(400.0, 200.0));
    }

    #[test]
    fn handtag_traffas() {
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 100.0));
        assert_eq!(hit_handle(r, pos2(101.0, 99.0)), Some(Handle::Nw));
        assert_eq!(hit_handle(r, pos2(300.0, 200.0)), Some(Handle::Se));
        assert_eq!(hit_handle(r, pos2(200.0, 103.0)), Some(Handle::N));
        assert_eq!(hit_handle(r, pos2(296.0, 150.0)), Some(Handle::E));
        assert_eq!(hit_handle(r, pos2(200.0, 150.0)), None);
        let s = Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0));
        assert_eq!(hit_handle(s, pos2(1.0, 1.0)), Some(Handle::Nw));
    }

    #[test]
    fn hornet_behaller_proportioner() {
        let o = vec2(200.0, 100.0);
        assert_eq!(resize(Handle::Se, o, vec2(100.0, 0.0), true, 1000.0), vec2(300.0, 150.0));
        // Den största relativa ändringen styr.
        assert_eq!(resize(Handle::Se, o, vec2(10.0, 100.0), true, 1000.0), vec2(400.0, 200.0));
        assert_eq!(resize(Handle::Nw, o, vec2(-100.0, 0.0), true, 1000.0), vec2(300.0, 150.0));
        // Fritt: båda leder för sig.
        assert_eq!(resize(Handle::Se, o, vec2(100.0, 20.0), false, 1000.0), vec2(300.0, 120.0));
    }

    #[test]
    fn kanter() {
        let o = vec2(200.0, 100.0);
        assert_eq!(resize(Handle::E, o, vec2(100.0, 50.0), true, 1000.0), vec2(300.0, 150.0));
        assert_eq!(resize(Handle::W, o, vec2(-100.0, 50.0), false, 1000.0), vec2(300.0, 100.0));
        assert_eq!(resize(Handle::S, o, vec2(50.0, 100.0), true, 1000.0), vec2(400.0, 200.0));
        assert_eq!(resize(Handle::N, o, vec2(0.0, -50.0), false, 1000.0), vec2(200.0, 150.0));
    }

    #[test]
    fn gransvarden() {
        let o = vec2(200.0, 100.0);
        assert_eq!(resize(Handle::Se, o, vec2(-500.0, -500.0), true, 1000.0), vec2(32.0, 16.0));
        assert_eq!(resize(Handle::E, o, vec2(-500.0, 0.0), false, 1000.0), vec2(MIN_SIZE, 100.0));
        assert_eq!(resize(Handle::Se, o, vec2(5000.0, 0.0), true, 400.0), vec2(400.0, 200.0));
        assert_eq!(resize(Handle::E, o, vec2(5000.0, 0.0), false, 400.0), vec2(400.0, 100.0));
    }

    #[test]
    fn forhandsrutans_ankare() {
        let o = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 100.0));
        let r = resized_rect(o, Handle::Se, vec2(100.0, 50.0));
        assert_eq!((r.min, r.max), (pos2(100.0, 100.0), pos2(200.0, 150.0)));
        assert_eq!(resized_rect(o, Handle::Nw, vec2(100.0, 50.0)).max, pos2(300.0, 200.0));
        assert_eq!(resized_rect(o, Handle::E, vec2(100.0, 100.0)).min, pos2(100.0, 100.0));
        let r = resized_rect(o, Handle::N, vec2(100.0, 50.0));
        assert_eq!((r.center().x, r.bottom()), (200.0, 200.0));
    }

    #[test]
    fn ta_bort_bild() {
        let t = "a\n\n![x](i.png)\n\nb";
        let s = t.find("![").unwrap();
        assert_eq!(remove_image(t, s..s + 11).text, "a\n\n\nb");
        // Bild i löpande text: bara bilden försvinner.
        let t = "före ![x](i.png) efter";
        let s = t.find("![").unwrap();
        assert_eq!(remove_image(t, s..s + 11).text, "före  efter");
        // Sista raden.
        assert_eq!(remove_image("a\n![x](i.png)", 2..13).text, "a");
    }

    #[test]
    fn fri_storlek_skriver_bada_matten() {
        let e = set_free_size("![x](i.png)", 0..11, 120.4, 60.0);
        assert_eq!(e.text, "<img src=\"i.png\" alt=\"x\" width=\"120\" height=\"60\">");
        let a = markup::analyze(&e.text);
        let im = a.pieces.iter().find_map(|p| p.image.clone()).unwrap();
        assert_eq!((im.width, im.height), (Some(120.0), Some(60.0)));
        let e2 = set_free_size(&e.text, e.sel.clone(), 100.0, 90.0);
        assert!(e2.text.contains("width=\"100\" height=\"90\">") && e2.text.matches("height").count() == 1, "{}", e2.text);
    }

    #[test]
    fn justering_och_storlek_ror_inte_tabellen() {
        let tbl = "| Uppgift | Ansvarig |\n| --- | --- |\n| Design | Anna |\n";
        let t = format!("> citat\n\n![Diagram](bild:image1.png)\n\n{tbl}\nSlut\n");
        let s = t.find("![").unwrap();
        let e = markup::set_image_size(&t, s..s + 27, Some(150.0));
        let e = markup::set_align(&e.text, e.sel.clone(), Align::Center);
        assert!(e.text.contains(tbl), "{}", e.text);
        assert!(e.text.contains("<div align=\"center\">"));
    }

    #[test]
    fn storleksnyckel() {
        assert_eq!(size_key(&img(None, None)), "x.png");
        assert_ne!(size_key(&img(Some(10.0), None)), size_key(&img(Some(20.0), None)));
    }
}
