//! Bildspelets vy: bilderna i helskärm med övergångar och animeringar, och föredragsvyn.
//!
//! Varje bild renderas i bakgrunden som lager (`anim_layers.rs`): fasta lager och ett lager per
//! animerat mål. En bildruta ritas genom att lagren läggs ut som egui-nät med tillståndet ur
//! tidslinjen (`anim.rs`: synlighet, opacitet, förskjutning, skala, vridning, avslöjningsform), och
//! övergångar (`transition.rs`) klipper och flyttar hela bilder på samma sätt. Inget renderas om medan
//! något rör sig; GPU:n blandar texturerna.
//!
//! Föredragsvyn (`Alt+F5`) visar aktuell bild, nästa klick-steg, anteckningar, klocka och förfluten tid.
//! Den öppnas i ett eget fönster när miljön har flera fönster (egui-viewport), annars växlar `Alt+F5`
//! mellan publikens vy och talarens vy i samma fönster.
//!
//! Tiden och stegningen är rena och provas i `slideshow.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::emath::GuiRounding;
use egui::{Align2, Color32, CursorIcon, FontFamily, FontId, Key, Modifiers, Painter, PointerButton, Pos2, Rect, RichText, Sense, Ui, pos2, vec2};

use super::anim::{State, Target, Timeline};
use super::anim_layers::{self, LayerKind};
use super::scene::Affine;
use super::slideshow::{Blank, Frame, Playback, Screen, Show, SlideState, SlideTiming, Timer, clock_text};
use super::transition::{self, Layer, Transition};
use super::Deck;
use crate::i18n::t;
use crate::tf;

/// Högst så många bilder renderas samtidigt.
const MAX_JOBS: usize = 2;
/// Pekaren göms när musen stått still så här länge.
const HIDE_POINTER: Duration = Duration::from_secs(2);

/// Ett renderat lager som textur.
struct Tex {
    kind: LayerKind,
    tex: egui::TextureHandle,
    /// Bildpunktsrektangeln i hela bildens raster.
    px: (u32, u32, u32, u32),
    geom: super::anim::Geom,
}

/// En bild som lager, renderad i bredden `width` (höjden `height`).
struct Prepared {
    width: u32,
    height: u32,
    layers: Vec<Tex>,
}

type Done = (usize, u32, u32, Option<Vec<anim_layers::Rendered>>);

/// Pågående bildspel.
pub struct ShowView {
    deck: Arc<Deck>,
    play: Playback,
    timelines: Vec<Arc<Timeline>>,
    origin: Instant,
    prepared: HashMap<usize, Prepared>,
    pending: HashSet<usize>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// Bredd i bildpunkter som bilderna renderas i (den största ytan de ritas på).
    width: u32,
    /// Största ytan bilden ritats på i den här bildrutan (publikens vy och föredragsvyn).
    frame_px: u32,
    /// Föredragsvyn är på.
    presenter: bool,
    timer: Timer,
    notes: HashMap<usize, String>,
    notes_size: f32,
    pointer_moved: Instant,
    wheel: f32,
    /// Tid (s) som uppspelningen stått still i väntan på att aktuell bild ska renderas, så att en
    /// övergång eller självstartande animering inte hinner gå medan bilden är svart.
    stalled: f64,
    last_real: f64,
}

impl ShowView {
    /// `animate` = falskt: inga övergångar eller animeringar (bilderna visas i sitt vanliga utseende).
    pub fn new(deck: Arc<Deck>, show: Show, animate: bool) -> ShowView {
        let timelines: Vec<Arc<Timeline>> =
            deck.slides.iter().map(|s| Arc::new(if animate { Timeline::of_slide(&s.root) } else { Timeline::default() })).collect();
        let timing = timings(&deck, &timelines, animate);
        let origin = Instant::now();
        let (tx, rx) = channel();
        ShowView {
            play: Playback::new(show, timing, 0.0),
            deck,
            timelines,
            origin,
            prepared: HashMap::new(),
            pending: HashSet::new(),
            tx,
            rx,
            width: 0,
            frame_px: 0,
            presenter: false,
            timer: Timer::new(0.0),
            notes: HashMap::new(),
            notes_size: 18.0,
            pointer_moved: Instant::now(),
            wheel: 0.0,
            stalled: 0.0,
            last_real: 0.0,
        }
    }

    /// Startar med föredragsvyn (Alt+F5 i vyn, som i PowerPoint).
    pub fn with_presenter(mut self, on: bool) -> ShowView {
        self.presenter = on;
        self
    }

    /// Uppspelningens klocka (står still medan aktuell bild renderas).
    fn now(&self) -> f64 {
        self.real() - self.stalled
    }

    /// Väggklockan sedan bildspelet startade (föredragsvyns förflutna tid).
    fn real(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }

    /// Bilden som visas eller senast visades.
    pub fn slide(&self) -> usize {
        self.play.show.slide()
    }

    /// Tangenterna (från publikens fönster och föredragsvyns). Falskt = avsluta.
    pub fn keys(&mut self, ctx: &egui::Context) -> bool {
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let now = self.now();
        if key(Modifiers::NONE, Key::Escape) {
            return false;
        }
        if key(Modifiers::ALT, Key::F5) {
            self.presenter = !self.presenter;
        }
        // Skift-varianterna först: `consume_key` bryr sig inte om extra Skift.
        let prev = key(Modifiers::SHIFT, Key::Space)
            | key(Modifiers::NONE, Key::ArrowLeft)
            | key(Modifiers::NONE, Key::ArrowUp)
            | key(Modifiers::NONE, Key::PageUp)
            | key(Modifiers::NONE, Key::Backspace)
            | key(Modifiers::NONE, Key::P);
        let next = key(Modifiers::NONE, Key::ArrowRight)
            | key(Modifiers::NONE, Key::ArrowDown)
            | key(Modifiers::NONE, Key::PageDown)
            | key(Modifiers::NONE, Key::Space)
            | key(Modifiers::NONE, Key::Enter)
            | key(Modifiers::NONE, Key::N);
        if prev {
            self.play.prev(now);
        }
        if next && !self.play.next(now, false) {
            return false;
        }
        if key(Modifiers::NONE, Key::Home) {
            self.play.jump(now, Show::first);
        }
        if key(Modifiers::NONE, Key::End) {
            self.play.jump(now, Show::last);
        }
        if key(Modifiers::NONE, Key::B) || key(Modifiers::NONE, Key::Period) {
            self.play.jump(now, |s| s.toggle_blank(Blank::Black));
        }
        if key(Modifiers::NONE, Key::W) || key(Modifiers::NONE, Key::Comma) {
            self.play.jump(now, |s| s.toggle_blank(Blank::White));
        }
        true
    }

    /// Ritar bildspelet i `ui` (hela fönstret) och föredragsvyn. Falskt = avsluta.
    pub fn ui(&mut self, ui: &mut Ui) -> bool {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        let real = self.real();
        if let Screen::Slide(i) = self.play.show.screen() {
            if !self.prepared.contains_key(&i) {
                self.stalled += real - self.last_real;
            }
        }
        self.last_real = real;
        let now = self.now();
        if !self.play.tick(now) {
            return false;
        }
        let rect = ui.max_rect();
        let mut keep = true;
        let embedded = ctx.embed_viewports();
        if self.presenter && embedded {
            keep &= self.presenter_ui(ui, rect);
        } else {
            keep &= self.audience(ui, rect);
        }
        if self.presenter && !embedded {
            let id = egui::ViewportId::from_hash_of("kladd-pptx-presenter");
            let builder = egui::ViewportBuilder::default().with_title(t("Föredragsvy")).with_inner_size([1100.0, 720.0]);
            ctx.show_viewport_immediate(id, builder, |ui, _| {
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    // Att stänga talarens fönster stänger bara föredragsvyn.
                    self.presenter = false;
                    return;
                }
                let r = ui.max_rect();
                ui.painter().rect_filled(r, 0.0, Color32::from_gray(24));
                keep &= self.presenter_ui(ui, r);
                keep &= self.keys(ui.ctx());
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            });
        }
        // Omritning: varje bildruta när något rör sig, annars till nästa automatiska övergång.
        if self.play.animating() {
            ctx.request_repaint();
        } else if let Some(d) = self.play.auto_due(self.now()) {
            ctx.request_repaint_after(Duration::from_secs_f64(d.min(3600.0)));
        }
        if self.presenter {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        self.settle_width();
        self.retain_and_dispatch(&ctx);
        keep
    }

    /// Publikens vy: hela fönstret svart med bilden anpassad till skärmen.
    fn audience(&mut self, ui: &mut Ui, rect: Rect) -> bool {
        let ctx = ui.ctx().clone();
        let resp = ui.allocate_rect(rect, Sense::click());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        let slide_rect = self.fit(rect).round_to_pixels(ctx.pixels_per_point());
        self.want_width((slide_rect.width() * ctx.pixels_per_point()).round() as u32);
        let frame = self.play.frame(self.now());
        self.paint_frame(&painter, slide_rect, rect, &frame);
        // Mus: klick framåt, högerklick bakåt, hjulet bläddrar.
        let now = self.now();
        let mut keep = true;
        if resp.clicked_by(PointerButton::Primary) {
            keep = self.play.next(now, true);
        } else if resp.clicked_by(PointerButton::Secondary) {
            self.play.prev(now);
        }
        self.wheel += ui.input(|i| i.smooth_scroll_delta.y);
        if self.wheel <= -60.0 {
            self.wheel = 0.0;
            keep &= self.play.next(now, false);
        } else if self.wheel >= 60.0 {
            self.wheel = 0.0;
            self.play.prev(now);
        }
        // Pekaren göms när musen står still.
        if ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_pressed()) {
            self.pointer_moved = Instant::now();
        }
        if self.pointer_moved.elapsed() >= HIDE_POINTER {
            ctx.set_cursor_icon(CursorIcon::None);
        } else {
            ctx.request_repaint_after(HIDE_POINTER);
        }
        keep
    }

    /// Bildens rektangel anpassad i `rect`.
    fn fit(&self, rect: Rect) -> Rect {
        let (sw, sh) = self.deck.size_pt();
        let k = (rect.width() / sw.max(1.0)).min(rect.height() / sh.max(1.0)).max(0.0);
        Rect::from_center_size(rect.center(), vec2(sw * k, sh * k))
    }

    fn want_width(&mut self, px: u32) {
        self.frame_px = self.frame_px.max(px.clamp(16, 8192));
    }

    /// Bildrutans största yta blir renderingsbredden; om först när storleken ändrats märkbart.
    fn settle_width(&mut self) {
        let px = std::mem::take(&mut self.frame_px);
        if px > 0 && (self.width == 0 || (px as f32 - self.width as f32).abs() > self.width as f32 * 0.02 + 2.0) {
            self.width = px;
        }
    }

    /// Ritar en bildruta: skärmen (bild, svart/vit, slut) med pågående övergång.
    fn paint_frame(&self, painter: &Painter, slide_rect: Rect, full: Rect, frame: &Frame) {
        match frame.screen {
            Screen::Slide(_) => {
                let Some(cur) = &frame.cur else { return };
                match &frame.transition {
                    Some((old, kind, p)) => {
                        let (sw, sh) = self.deck.size_pt();
                        for piece in transition::pieces(*kind, *p, sw / sh.max(1.0)) {
                            match (piece.layer, old) {
                                (Layer::New, _) => self.paint_slide(painter, slide_rect, cur, Some(&piece.poly), &piece.map, piece.alpha),
                                (Layer::Old, Some(o)) => self.paint_slide(painter, slide_rect, o, Some(&piece.poly), &piece.map, piece.alpha),
                                (Layer::Old, None) | (Layer::Black, _) => {
                                    let pts: Vec<Pos2> = piece.poly.iter().map(|&q| unit_to_screen(slide_rect, transition::apply(&piece.map, q))).collect();
                                    painter.add(egui::Shape::convex_polygon(pts, Color32::from_black_alpha((piece.alpha * 255.0) as u8), egui::Stroke::NONE));
                                }
                            }
                        }
                    }
                    None => self.paint_slide(painter, slide_rect, cur, None, &transition::IDENTITY, 1.0),
                }
            }
            Screen::Blank(Blank::Black) => {}
            Screen::Blank(Blank::White) => {
                painter.rect_filled(full, 0.0, Color32::WHITE);
            }
            Screen::End => {
                painter.text(
                    pos2(full.center().x, full.top() + 40.0),
                    Align2::CENTER_TOP,
                    t("Slut på bildspelet. Klicka för att avsluta."),
                    FontId::new(16.0, FontFamily::Proportional),
                    Color32::from_gray(170),
                );
            }
        }
    }

    /// Lagrets tillstånd i bildens läge.
    fn layer_state(&self, st: &SlideState, kind: LayerKind, geom: super::anim::Geom) -> State {
        let LayerKind::Target(tg) = kind else { return State::SHOWN };
        let Some(tl) = self.timelines.get(st.slide) else { return State::SHOWN };
        let own = tl.state(tg, geom, st.chain.clone(), st.t);
        match tg.para {
            // Ett stycke följer också formens egna effekter.
            Some(_) if tl.targets().contains(&Target { shape: tg.shape, para: None }) => {
                tl.state(Target { shape: tg.shape, para: None }, geom, st.chain.clone(), st.t).compose(&own)
            }
            _ => own,
        }
    }

    /// Ritar bilden i läget `st` i `rect`, klippt till `clip` (bildens enhetskoordinater) och flyttad med
    /// `map` (enhet → enhet), med genomskinligheten `alpha`. Inget ritas förrän lagren finns (så att
    /// former med in-effekter aldrig syns för tidigt).
    fn paint_slide(&self, painter: &Painter, rect: Rect, st: &SlideState, clip: Option<&[[f32; 2]]>, map: &transition::Map, alpha: f32) {
        let Some(prep) = self.prepared.get(&st.slide) else { return };
        let (sw, sh) = self.deck.size_pt();
        let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        // Bildpunkter i rastret → punkter på bilden.
        let (kx, ky) = (sw / prep.width.max(1) as f32, sh / prep.height.max(1) as f32);
        let s = Affine::scale(sw, sh);
        let m = Affine { a: map[0], b: map[1], c: map[2], d: map[3], e: map[4], f: map[5] };
        let to_dst = s.invert().unwrap_or(Affine::IDENTITY).then(&m).then(&s);
        let clip_pts: Option<Vec<[f32; 2]>> = clip.map(|c| c.iter().map(|q| [q[0] * sw, q[1] * sh]).collect());
        let scr = rect.width() / sw.max(1.0);
        for l in &prep.layers {
            let state = self.layer_state(st, l.kind, l.geom);
            if !state.drawn() {
                continue;
            }
            let (x0, y0, w, h) = (l.px.0 as f32 * kx, l.px.1 as f32 * ky, l.px.2 as f32 * kx, l.px.3 as f32 * ky);
            let (cx, cy) = (x0 + w / 2.0, y0 + h / 2.0);
            let base = Affine::scale(w, h).then(&Affine::translate(x0, y0));
            let anim = Affine::translate(-cx, -cy)
                .then(&Affine::scale(state.sx, state.sy))
                .then(&Affine::rotate(state.rot))
                .then(&Affine::translate(cx + state.dx * sw, cy + state.dy * sh));
            let a = base.then(&anim);
            let Some(inv) = a.invert() else { continue };
            let polys = match state.reveal {
                Some((r, p)) => transition::reveal(r, p, w / h.max(0.01)),
                None => vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]],
            };
            let color = Color32::from_white_alpha((state.opacity * alpha * 255.0).round().clamp(0.0, 255.0) as u8);
            let mut mesh = egui::Mesh::with_texture(l.tex.id());
            for poly in polys {
                let mut pts: Vec<[f32; 2]> = poly.iter().map(|q| { let (x, y) = a.apply(q[0], q[1]); [x, y] }).collect();
                if let Some(c) = &clip_pts {
                    pts = clip_convex(&pts, c);
                }
                if pts.len() < 3 {
                    continue;
                }
                let first = mesh.vertices.len() as u32;
                for p in &pts {
                    let (u, v) = inv.apply(p[0], p[1]);
                    let (dx, dy) = to_dst.apply(p[0], p[1]);
                    mesh.vertices.push(egui::epaint::Vertex { pos: pos2(rect.left() + dx * scr, rect.top() + dy * scr), uv: pos2(u, v), color });
                }
                for k in 1..pts.len() as u32 - 1 {
                    mesh.add_triangle(first, first + k, first + k + 1);
                }
            }
            if !mesh.is_empty() {
                painter.add(egui::Shape::mesh(mesh));
            }
        }
    }

    // ------------------------------------------------------------ föredragsvyn

    fn notes_of(&mut self, i: usize) -> String {
        let deck = self.deck.clone();
        self.notes.entry(i).or_insert_with(|| super::notes::notes_text(&deck, i)).clone()
    }

    /// Talarens vy i `rect`. Falskt = avsluta.
    fn presenter_ui(&mut self, ui: &mut Ui, rect: Rect) -> bool {
        let ctx = ui.ctx().clone();
        let now = self.now();
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 0.0, Color32::from_gray(24));
        let pad = 16.0;
        let text = Color32::from_gray(225);
        let dim = Color32::from_gray(150);
        let mut keep = true;
        // Överst: förfluten tid med paus/nollställ, klockan till höger.
        let top = Rect::from_min_size(rect.min + vec2(pad, pad / 2.0), vec2(rect.width() - 2.0 * pad, 40.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(top), |ui| {
            ui.horizontal_centered(|ui| {
                let real = self.real();
                let el = clock_text(self.timer.elapsed(real));
                ui.label(RichText::new(el).size(26.0).color(if self.timer.paused() { dim } else { text }).monospace());
                let pause = if self.timer.paused() { t("Fortsätt") } else { t("Pausa") };
                if ui.button(pause).clicked() {
                    self.timer.toggle_pause(real);
                }
                if ui.button(t("Nollställ")).clicked() {
                    self.timer.reset(real);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    use chrono::Timelike;
                    let n = chrono::Local::now();
                    ui.label(RichText::new(crate::i18n::clock(n.hour(), n.minute())).size(22.0).color(text));
                });
            });
        });
        // Nederst: bildnummer och knappar.
        let bottom = Rect::from_min_size(pos2(rect.left() + pad, rect.bottom() - 48.0), vec2(rect.width() - 2.0 * pad, 40.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(bottom), |ui| {
            ui.horizontal_centered(|ui| {
                let (pos, total) = self.play.show.position();
                let label = match self.play.show.screen() {
                    Screen::End => t("Slut på bildspelet").to_owned(),
                    _ => tf!("Bild {n} av {total}", n = pos + 1, total = total),
                };
                ui.label(RichText::new(label).size(16.0).color(text));
                ui.add_space(12.0);
                if ui.button(t("◀ Föregående")).clicked() {
                    self.play.prev(now);
                }
                if ui.button(t("Nästa ▶")).clicked() {
                    keep &= self.play.next(now, false);
                }
                let black = matches!(self.play.show.screen(), Screen::Blank(Blank::Black));
                if ui.selectable_label(black, t("Svart skärm")).clicked() {
                    self.play.jump(now, |s| s.toggle_blank(Blank::Black));
                }
                if ui.button(t("Avsluta bildspelet")).clicked() {
                    keep = false;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("A+").on_hover_text(t("Större text i anteckningarna")).clicked() {
                        self.notes_size = (self.notes_size + 2.0).min(48.0);
                    }
                    if ui.button("A−").on_hover_text(t("Mindre text i anteckningarna")).clicked() {
                        self.notes_size = (self.notes_size - 2.0).max(10.0);
                    }
                    if !ctx.embed_viewports() {
                        ui.label(RichText::new(t("Alt+F5 stänger föredragsvyn")).color(dim));
                    } else {
                        ui.label(RichText::new(t("Alt+F5 visar publikens vy")).color(dim));
                    }
                });
            });
        });
        // Mitten: aktuell bild till vänster, nästa och anteckningarna till höger.
        let body = Rect::from_min_max(pos2(rect.left() + pad, top.bottom() + pad / 2.0), pos2(rect.right() - pad, bottom.top() - pad / 2.0));
        if body.width() < 50.0 || body.height() < 50.0 {
            return keep;
        }
        let left = Rect::from_min_max(body.min, pos2(body.left() + body.width() * 0.6, body.bottom()));
        let right = Rect::from_min_max(pos2(left.right() + pad, body.top()), body.max);
        let cur_rect = self.fit(Rect::from_min_max(left.min + vec2(0.0, 22.0), left.max)).round_to_pixels(ctx.pixels_per_point());
        painter.text(left.min, Align2::LEFT_TOP, t("Aktuell bild"), FontId::proportional(14.0), dim);
        painter.rect_filled(cur_rect, 0.0, Color32::BLACK);
        self.want_width((cur_rect.width() * ctx.pixels_per_point()).round() as u32);
        let frame = self.play.frame(now);
        self.paint_frame(&painter, cur_rect, cur_rect, &frame);
        let next_area = Rect::from_min_size(right.min + vec2(0.0, 22.0), vec2(right.width(), (right.height() * 0.45).max(40.0)));
        painter.text(right.min, Align2::LEFT_TOP, t("Nästa"), FontId::proportional(14.0), dim);
        let next_rect = self.fit(next_area);
        let next_rect = Rect::from_min_size(next_area.min, next_rect.size()).round_to_pixels(ctx.pixels_per_point());
        match self.play.show.peek_next() {
            Some((s, step)) => {
                painter.rect_filled(next_rect, 0.0, Color32::BLACK);
                let end = self.play_chain_end(s, step);
                let st = SlideState { slide: s, step, chain: end..end, t: f32::INFINITY };
                self.paint_slide(&painter, next_rect, &st, None, &transition::IDENTITY, 1.0);
            }
            None => {
                painter.rect_filled(next_rect, 0.0, Color32::BLACK);
                painter.text(next_rect.center(), Align2::CENTER_CENTER, t("Slut på bildspelet"), FontId::proportional(14.0), dim);
            }
        }
        let notes_rect = Rect::from_min_max(pos2(right.left(), next_rect.bottom() + pad), right.max);
        if notes_rect.height() > 30.0 {
            let notes = match self.play.show.screen() {
                Screen::Slide(i) => self.notes_of(i),
                _ => String::new(),
            };
            let size = self.notes_size;
            ui.scope_builder(egui::UiBuilder::new().max_rect(notes_rect), |ui| {
                ui.label(RichText::new(t("Anteckningar")).size(14.0).color(dim));
                egui::ScrollArea::vertical().id_salt("kladd-pptx-presenter-notes").auto_shrink([false, false]).show(ui, |ui| {
                    if notes.is_empty() {
                        ui.label(RichText::new(t("Inga anteckningar")).size(size).color(dim));
                    } else {
                        ui.label(RichText::new(notes).size(size).color(text));
                    }
                });
            });
        }
        // Klick på aktuell bild går framåt.
        let resp = ui.interact(cur_rect, ui.scope_id().with("kladd-pptx-presenter-cur"), Sense::click());
        if resp.clicked() {
            keep &= self.play.next(now, true);
        }
        keep
    }

    /// Antal klara animeringsgrupper efter `step` klick på bild `slide`.
    fn play_chain_end(&self, slide: usize, step: usize) -> usize {
        self.timelines.get(slide).map_or(0, |t| t.groups_at_step(step))
    }

    // ------------------------------------------------------------ rendering

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok((slide, width, height, layers)) = self.rx.try_recv() {
            self.pending.remove(&slide);
            let layers = layers.unwrap_or_else(|| {
                // En trasig bild får inte fälla appen: vit bild.
                vec![anim_layers::Rendered { kind: LayerKind::Static, px: (0, 0, 1, 1), rgba: vec![255; 4], geom: super::anim::Geom { cx: 0.5, cy: 0.5, w: 1.0, h: 1.0 } }]
            });
            let (w, h) = if layers.len() == 1 && layers[0].px == (0, 0, 1, 1) { (1, 1) } else { (width, height) };
            let layers = layers
                .into_iter()
                .enumerate()
                .map(|(n, r)| {
                    let image = egui::ColorImage::from_rgba_premultiplied([r.px.2 as usize, r.px.3 as usize], &r.rgba);
                    let tex = ctx.load_texture(format!("pptx-show-{slide}-{n}"), image, egui::TextureOptions::LINEAR);
                    Tex { kind: r.kind, tex, px: r.px, geom: r.geom }
                })
                .collect();
            self.prepared.insert(slide, Prepared { width: w, height: h, layers });
        }
    }

    /// Släpper bilder som inte behövs och startar renderingar av de som saknas (aktuell bild först).
    fn retain_and_dispatch(&mut self, ctx: &egui::Context) {
        let frame = self.play.frame(self.now());
        let mut need: Vec<usize> = Vec::new();
        if let Screen::Slide(i) = frame.screen {
            need.push(i);
        }
        if let Some((Some(o), _, _)) = &frame.transition {
            need.push(o.slide);
        }
        if let Some((n, _)) = self.play.show.peek_next() {
            need.push(n);
        }
        need.push(self.play.show.slide());
        self.prepared.retain(|k, _| need.contains(k));
        if self.width == 0 {
            return;
        }
        for i in need {
            if self.pending.len() >= MAX_JOBS {
                break;
            }
            let fresh = self.prepared.get(&i).is_some_and(|p| p.width == self.width || p.width == 1);
            if fresh || self.pending.contains(&i) {
                continue;
            }
            self.pending.insert(i);
            let deck = self.deck.clone();
            let tl = self.timelines.get(i).cloned().unwrap_or_default();
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            let (sw, sh) = deck.size_pt();
            let width = self.width;
            let height = ((width as f32) * sh / sw.max(1.0)).round().clamp(1.0, 8192.0) as u32;
            let _ = std::thread::Builder::new().name("pptx-bildspel".into()).stack_size(8 << 20).spawn(move || {
                let layers = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let scene = super::render::slide_scene(&deck, i);
                    let split = anim_layers::split(&scene, &tl.targets());
                    anim_layers::render(&scene, &split, width, height)
                }))
                .ok();
                let _ = tx.send((i, width, height, layers));
                ctx.request_repaint();
            });
        }
    }
}

/// Uppspelningens tider per bild ur tidslinjerna och övergångarna. Utan animeringar (`animate` falskt)
/// byts bilderna direkt, men automatisk övergång efter tid gäller ändå (som PowerPoints "visa utan
/// animering").
pub fn timings(deck: &Deck, timelines: &[Arc<Timeline>], animate: bool) -> Vec<SlideTiming> {
    deck.slides
        .iter()
        .zip(timelines)
        .map(|(s, tl)| {
            let parsed = Transition::of_slide(&s.root);
            let transition = if animate { parsed } else { Transition { adv_after: parsed.adv_after, ..Transition::default() } };
            let chains = (0..=tl.clicks()).map(|k| (tl.chain(k), tl.chain_dur(k))).collect();
            SlideTiming { chains, transition }
        })
        .collect()
}

fn unit_to_screen(rect: Rect, q: [f32; 2]) -> Pos2 {
    pos2(rect.left() + q[0] * rect.width(), rect.top() + q[1] * rect.height())
}

/// Klipper den konvexa polygonen `subject` med den konvexa polygonen `clip` (Sutherland–Hodgman).
pub fn clip_convex(subject: &[[f32; 2]], clip: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if clip.len() < 3 {
        return Vec::new();
    }
    // Orientering: positiv yta = medurs i y nedåt.
    let area: f32 = (0..clip.len()).map(|i| { let (a, b) = (clip[i], clip[(i + 1) % clip.len()]); a[0] * b[1] - b[0] * a[1] }).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut out = subject.to_vec();
    for i in 0..clip.len() {
        if out.is_empty() {
            break;
        }
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let side = |p: &[f32; 2]| sign * ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]));
        let input = std::mem::take(&mut out);
        for k in 0..input.len() {
            let cur = input[k];
            let prev = input[(k + input.len() - 1) % input.len()];
            let (sc, sp) = (side(&cur), side(&prev));
            if (sc >= 0.0) != (sp >= 0.0) {
                let t = sp / (sp - sc);
                out.push([prev[0] + (cur[0] - prev[0]) * t, prev[1] + (cur[1] - prev[1]) * t]);
            }
            if sc >= 0.0 {
                out.push(cur);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn konvex_klippning_åt_båda_hållen() {
        let sq = vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        let tri = vec![[1.0, -1.0], [3.0, 1.0], [1.0, 3.0]];
        let a = |p: &[[f32; 2]]| (0..p.len()).map(|i| { let (x, y) = (p[i], p[(i + 1) % p.len()]); x[0] * y[1] - y[0] * x[1] }).sum::<f32>().abs() / 2.0;
        let c1 = clip_convex(&sq, &tri);
        let rev: Vec<[f32; 2]> = tri.iter().rev().copied().collect();
        let c2 = clip_convex(&sq, &rev);
        assert!((a(&c1) - a(&c2)).abs() < 1e-5);
        assert!((a(&c1) - 2.0).abs() < 1e-5, "{}", a(&c1));
        assert!(clip_convex(&sq, &[[5.0, 5.0], [6.0, 5.0], [6.0, 6.0]]).is_empty());
    }
}
