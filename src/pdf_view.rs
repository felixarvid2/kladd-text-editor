//! Visar en PDF som sidor (renderade med hayro) med innehållslista.
//!
//! Upplägget följer PdfCraft (github.com/storytold/pdfcraft, MIT/Apache-2.0):
//! - sidornas storlek läses en gång, så att layouten är klar innan något renderats,
//! - bara synliga sidor (plus en före och efter) renderas, i en egen tråd med egen cache,
//! - skalan är exakt skärmens (zoom × 96/72 × pixlar per punkt) så att texten blir skarp,
//! - vid zoom ritas den gamla bilden utsträckt tills en skarp är klar (0,3 s fördröjning),
//! - "anpassa till bredd" räknas på den bredaste sidan så att zoomen inte hoppar.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, ColorImage, Rect, TextureHandle, TextureOptions, Ui, pos2, vec2};

use crate::theme::Palette;

/// Punkter → logiska pixlar vid 100 %.
const PT: f32 = 96.0 / 72.0;
const GAP: f32 = 18.0;
const MARGIN_TOP: f32 = 24.0;
const SIDE: f32 = 40.0;
/// Högst så här många renderade sidor sparas.
const MAX_CACHED: usize = 14;
pub const ZOOM_STEPS: &[f32] = &[0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0];

struct Request {
    page: usize,
    scale: f32,
    tag: u64,
}

struct Done {
    page: usize,
    tag: u64,
    image: Option<ColorImage>,
}

pub struct PdfView {
    sizes: Vec<(f32, f32)>,
    textures: HashMap<usize, (u64, TextureHandle)>,
    requested: HashMap<usize, u64>,
    tx: Sender<Request>,
    rx: Receiver<Done>,
    /// 1.0 = sidan fyller bredden.
    pub zoom: f32,
    pub current_page: usize,
    /// Läsposition: sida + andel uppifrån (för att markera aktuell rubrik).
    pub reading: (usize, f32),
    pending_goto: Option<(usize, f32)>,
    last_zoom_change: f64,
    page_tops: Vec<f32>,
    page_heights: Vec<f32>,
    doc_id: u64,
}

/// Öppnar PDF:en för visning. Fel ger ett meddelande som kan visas.
pub fn open(ctx: &egui::Context, doc_id: u64, bytes: Arc<Vec<u8>>, password: Option<String>) -> Result<PdfView, String> {
    let pw = password.clone().unwrap_or_default();
    let pdf = hayro::hayro_syntax::Pdf::new_with_password(bytes.clone(), &pw)
        .map_err(|e| format!("PDF:en kan inte visas: {e:?}"))?;
    let sizes: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
    if sizes.is_empty() {
        return Err("PDF:en har inga sidor.".into());
    }
    drop(pdf);

    let (tx, req_rx) = channel::<Request>();
    let (done_tx, rx) = channel::<Done>();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("kladd-pdf".into())
        .spawn(move || worker(bytes, pw, req_rx, done_tx, ctx))
        .map_err(|e| e.to_string())?;

    Ok(PdfView {
        sizes,
        textures: HashMap::new(),
        requested: HashMap::new(),
        tx,
        rx,
        zoom: 1.0,
        current_page: 0,
        reading: (0, 0.0),
        pending_goto: None,
        last_zoom_change: f64::NEG_INFINITY,
        page_tops: Vec::new(),
        page_heights: Vec::new(),
        doc_id,
    })
}

fn worker(bytes: Arc<Vec<u8>>, pw: String, rx: Receiver<Request>, tx: Sender<Done>, ctx: egui::Context) {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{PixmapSettings, RenderCache, RenderSettings};

    let Ok(pdf) = hayro::hayro_syntax::Pdf::new_with_password(bytes, &pw) else { return };
    let cache = RenderCache::new();
    let interp = InterpreterSettings::default();
    let settings = RenderSettings::default();
    while let Ok(first) = rx.recv() {
        // Ta det senaste önskemålet per sida, nyaste först.
        let mut batch = vec![first];
        while let Ok(r) = rx.try_recv() {
            batch.push(r);
        }
        let mut seen = std::collections::HashSet::new();
        for req in batch.into_iter().rev() {
            if !seen.insert(req.page) {
                continue;
            }
            let pages = pdf.pages();
            let image = pages.get(req.page).and_then(|page| {
                // Skydda appen om en trasig sida får renderaren att krascha.
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let pm = hayro::render(
                        page,
                        &cache,
                        &interp,
                        &settings,
                        &PixmapSettings { x_scale: req.scale, y_scale: req.scale, bg_color: WHITE },
                    );
                    let size = [pm.width() as usize, pm.height() as usize];
                    ColorImage::from_rgba_premultiplied(size, pm.data_as_u8_slice())
                }))
                .ok()
            });
            if tx.send(Done { page: req.page, tag: req.tag, image }).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}

impl PdfView {
    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    pub fn zoom_in(&mut self, now: f64) {
        let z = ZOOM_STEPS.iter().copied().find(|s| *s > self.zoom * 1.01).unwrap_or(*ZOOM_STEPS.last().unwrap());
        self.set_zoom(z, now);
    }

    pub fn zoom_out(&mut self, now: f64) {
        let z = ZOOM_STEPS.iter().rev().copied().find(|s| *s < self.zoom * 0.99).unwrap_or(ZOOM_STEPS[0]);
        self.set_zoom(z, now);
    }

    pub fn set_zoom(&mut self, z: f32, now: f64) {
        if (z - self.zoom).abs() > f32::EPSILON {
            // Behåll läspositionen.
            let (p, f) = self.reading;
            self.zoom = z;
            self.last_zoom_change = now;
            self.pending_goto = Some((p, f));
        }
    }

    /// Hoppa till en sida och ett läge (andel uppifrån) på den.
    pub fn go_to(&mut self, page: usize, frac: f32) {
        self.pending_goto = Some((page.min(self.sizes.len().saturating_sub(1)), frac.clamp(0.0, 1.0)));
    }

    /// Ritar sidorna i en rullbar yta.
    pub fn show(&mut self, ui: &mut Ui, p: &Palette) {
        let now = ui.input(|i| i.time);
        let ppp = ui.ctx().pixels_per_point();
        let max_side = ui.ctx().input(|i| i.max_texture_side).min(8192) as f32;

        // Ta emot färdiga sidor (högst 6 per bildruta).
        for _ in 0..6 {
            let Ok(done) = self.rx.try_recv() else { break };
            if let Some(img) = done.image {
                let tex = ui.ctx().load_texture(format!("kladd-pdf-{}-{}", self.doc_id, done.page), img, TextureOptions::LINEAR);
                self.textures.insert(done.page, (done.tag, tex));
            }
        }

        let avail_w = ui.available_width();
        let widest = self.sizes.iter().map(|s| s.0).fold(1.0, f32::max);
        // Anpassa till bredd, men inte större än ungefär A4 i 150 %.
        let fit = ((avail_w - 2.0 * SIDE) / (widest * PT)).clamp(0.2, 1.5);
        let k = fit * self.zoom * PT; // logiska pixlar per punkt

        // Layout
        self.page_tops.clear();
        self.page_heights.clear();
        let mut y = MARGIN_TOP;
        let mut content_w: f32 = 0.0;
        for (w, h) in &self.sizes {
            self.page_tops.push(y);
            self.page_heights.push(h * k);
            y += h * k + GAP;
            content_w = content_w.max(w * k + 2.0 * SIDE);
        }
        let total_h = y + MARGIN_TOP;

        // Ingen toning i kanterna: den ser ut som en skugga över sidorna.
        ui.spacing_mut().scroll.fade.strength = 0.0;
        let mut area = egui::ScrollArea::both().id_salt(("kladd-pdf-scroll", self.doc_id)).auto_shrink([false, false]);
        if let Some((page, frac)) = self.pending_goto.take() {
            let off = self.page_tops.get(page).copied().unwrap_or(0.0) + frac * self.page_heights.get(page).copied().unwrap_or(0.0) - 12.0;
            area = area.vertical_scroll_offset(off.max(0.0));
        }

        let zooming = now - self.last_zoom_change < 0.3;
        if zooming {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(320));
        }
        let scale = (k * ppp).max(0.05);
        let tag = (scale * 64.0).round() as u64;

        area.show_viewport(ui, |ui, viewport| {
            ui.set_min_size(vec2(content_w.max(avail_w), total_h));
            let origin = ui.min_rect().min;
            let full_w = content_w.max(avail_w);
            let painter = ui.painter();
            let mut best = (0usize, 0.0f32);
            let wanted = viewport.expand2(vec2(0.0, viewport.height()));
            for (i, (w, h)) in self.sizes.iter().enumerate() {
                let (pw, ph) = (w * k, h * k);
                let x = (full_w - pw) / 2.0;
                let local = Rect::from_min_size(pos2(x, self.page_tops[i]), vec2(pw, ph));
                // Sidan som syns mest är "aktuell".
                let overlap = local.intersect(viewport);
                if overlap.is_positive() && overlap.height() > best.1 + 0.5 {
                    best = (i, overlap.height());
                }
                if !local.intersects(wanted) {
                    continue;
                }
                let screen = local.translate(origin.to_vec2());
                // Skugga + papper
                painter.add(egui::epaint::Shadow { offset: [0, 3], blur: 14, spread: 0, color: Color32::from_black_alpha(if p.dark { 120 } else { 45 }) }.as_shape(screen, 2.0));
                painter.rect_filled(screen, 2.0, Color32::WHITE);
                match self.textures.get(&i) {
                    Some((_, tex)) => {
                        painter.image(tex.id(), screen, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        painter.text(screen.center(), egui::Align2::CENTER_CENTER, format!("Sida {}", i + 1), egui::FontId::proportional(13.0), Color32::from_gray(170));
                    }
                }
                // Be om en skarp rendering om den saknas eller har fel skala.
                let have = self.textures.get(&i).map(|(t, _)| *t);
                if have != Some(tag) && self.requested.get(&i) != Some(&tag) && (!zooming || have.is_none()) {
                    let s = scale.min(max_side / (w.max(*h))).max(0.05);
                    let _ = self.tx.send(Request { page: i, scale: s, tag });
                    self.requested.insert(i, tag);
                }
            }
            self.current_page = best.0;
            // Läsposition: en fjärdedel ner i fönstret.
            let probe = viewport.top() + viewport.height() * 0.25;
            let page = self.page_tops.iter().rposition(|t| *t <= probe).unwrap_or(0);
            let frac = ((probe - self.page_tops[page]) / self.page_heights[page].max(1.0)).clamp(0.0, 1.0);
            self.reading = (page, frac);
        });

        // Släpp sidor långt från den aktuella.
        if self.textures.len() > MAX_CACHED {
            let cur = self.current_page as i64;
            let mut keys: Vec<usize> = self.textures.keys().copied().collect();
            keys.sort_by_key(|k| std::cmp::Reverse((*k as i64 - cur).abs()));
            for k in keys.into_iter().take(self.textures.len() - MAX_CACHED) {
                self.textures.remove(&k);
                self.requested.remove(&k);
            }
        }
    }
}
