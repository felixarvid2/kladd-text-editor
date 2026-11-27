//! En enkel flikvy för presentationer: bilderna under varandra, renderade med `raster.rs` i skärmens
//! skala. Bara synliga bilder renderas (i bakgrundstrådar), och texturer långt från det synliga
//! släpps. Miniatyrer, bildspel, sök och PDF-export görs av nästa omgång.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

use egui::{Align, Color32, FontId, Layout, RichText, ScrollArea, Sense, Ui, vec2};

use super::Deck;
use crate::i18n::t;
use crate::tf;
use crate::theme::Palette;

/// Högst så många bilder renderas samtidigt.
const MAX_JOBS: usize = 2;
/// Mellanrum mellan bilderna (punkter på skärmen).
const GAP: f32 = 18.0;

struct Done {
    slide: usize,
    width: u32,
    image: egui::ColorImage,
}

/// En öppen presentation i en flik.
pub struct PptxTab {
    pub deck: Arc<Deck>,
    textures: HashMap<usize, (egui::TextureHandle, u32)>,
    pending: HashSet<usize>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    /// Bilden överst i vyn (0-baserad), för statusraden.
    pub current: usize,
}

impl PptxTab {
    pub fn new(deck: Deck) -> PptxTab {
        let (tx, rx) = channel();
        PptxTab { deck: Arc::new(deck), textures: HashMap::new(), pending: HashSet::new(), tx, rx, current: 0 }
    }

    /// Meddelande om sådant som inte visas än (diagram, EMF, inbäddade typsnitt som inte kan läsas).
    pub fn open_note(&self, name: &str) -> Option<String> {
        let mut lost: Vec<&str> = Vec::new();
        let (mut charts, mut emf) = (false, false);
        for i in 0..self.deck.slides.len() {
            let f = self.deck.features(i);
            charts |= f.chart;
            emf |= f.emf;
        }
        if charts {
            lost.push(t("diagram"));
        }
        if emf {
            lost.push(t("EMF/WMF-bilder"));
        }
        if !self.deck.fonts.unreadable_embedded.is_empty() {
            lost.push(t("komprimerade inbäddade typsnitt"));
        }
        (!lost.is_empty()).then(|| tf!("{name} innehåller {lost} som ännu inte visas som i PowerPoint", name = name, lost = crate::doc::join_sv(&lost)))
    }

    fn request(&mut self, ctx: &egui::Context, slide: usize, width: u32) {
        if self.pending.contains(&slide) || self.pending.len() >= MAX_JOBS {
            return;
        }
        self.pending.insert(slide);
        let deck = self.deck.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let (w, h) = deck.size_pt();
        let height = ((width as f32) * h / w.max(1.0)).round().max(1.0) as u32;
        let _ = std::thread::Builder::new().name("pptx-bild".into()).stack_size(8 << 20).spawn(move || {
            // En trasig bild får inte fälla appen: fel ger en tom bild.
            let rgba = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::render_slide(&deck, slide, width, height)))
                .ok()
                .flatten()
                .map(|(_, _, px)| px)
                .unwrap_or_else(|| vec![255; (width * height * 4) as usize]);
            let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
            let _ = tx.send(Done { slide, width, image });
            ctx.request_repaint();
        });
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.rx.try_recv() {
            self.pending.remove(&d.slide);
            let tex = ctx.load_texture(format!("pptx-{}", d.slide), d.image, egui::TextureOptions::LINEAR);
            self.textures.insert(d.slide, (tex, d.width));
        }
    }

    /// Ritar bilderna.
    pub fn show(&mut self, ui: &mut Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        let (sw, sh) = self.deck.size_pt();
        let ppp = ctx.pixels_per_point();
        let n = self.deck.slides.len();
        if n == 0 {
            ui.centered_and_justified(|ui| ui.label(RichText::new(t("Presentationen har inga bilder.")).color(p.muted)));
            return;
        }
        let mut visible: Vec<usize> = Vec::new();
        let mut top: Option<usize> = None;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let avail = ui.available_width();
            let width = (avail - 2.0 * GAP).clamp(120.0, 1280.0);
            let height = width * sh / sw.max(1.0);
            let px_w = (width * ppp).round().max(1.0) as u32;
            ui.add_space(GAP);
            for i in 0..n {
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
                    let painter = ui.painter_at(rect.expand(4.0));
                    painter.rect_filled(rect.translate(vec2(0.0, 2.0)).expand(1.0), 2.0, Color32::from_black_alpha(40));
                    if ui.is_rect_visible(rect) {
                        visible.push(i);
                        if top.is_none() && rect.bottom() > ui.clip_rect().top() + 1.0 {
                            top = Some(i);
                        }
                        match self.textures.get(&i) {
                            Some((tex, _)) => {
                                painter.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                            }
                            None => {
                                painter.rect_filled(rect, 0.0, Color32::WHITE);
                            }
                        }
                        let stale = self.textures.get(&i).is_none_or(|(_, w)| (*w as i64 - px_w as i64).abs() > 2);
                        if stale {
                            self.request(&ctx, i, px_w);
                        }
                    }
                    let hidden = self.deck.slides[i].hidden;
                    let label = if hidden { tf!("Bild {n} (dold)", n = i + 1) } else { tf!("Bild {n}", n = i + 1) };
                    ui.add_space(4.0);
                    ui.label(RichText::new(label).font(FontId::proportional(12.0)).color(p.muted));
                });
                ui.add_space(GAP);
            }
        });
        if let Some(t) = top {
            self.current = t;
        }
        // Släpp texturer långt från det synliga (minnet).
        if let (Some(&lo), Some(&hi)) = (visible.first(), visible.last()) {
            self.textures.retain(|&k, _| k + 6 >= lo && k <= hi + 6);
        }
    }
}

/// Statusraden: bildnummer och antal.
pub fn status(ui: &mut Ui, p: &Palette, font: &FontId, tab: &PptxTab) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        let label = |ui: &mut Ui, s: String| {
            ui.label(RichText::new(s).font(font.clone()).color(p.muted));
        };
        label(ui, tf!("Bild {n} av {total}", n = tab.current + 1, total = tab.deck.slides.len()));
        label(ui, t("Bara läsning").to_owned());
    });
}
