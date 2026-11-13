//! PDF-motorn: öppnar dokumentet med hayro och renderar sidor och läser ut text i en trådpool.
//!
//! * `Pdf` är `Send + Sync` och delas mellan trådarna; `RenderCache`/`InterpreterCache` är inte
//!   `Send`, så varje arbetstråd har egna.
//! * Jobb hämtas ur en gemensam prioritetskö. `request` ersätter kön med sidor som ännu inte
//!   påbörjats (listans ordning = prioritet). Renderingsjobb går före textjobb.
//! * Varje jobb körs i `catch_unwind`; efter en panik byter arbetaren ut sina cacher.
//! * Resultat skickas på en kanal, därefter `ctx.request_repaint()`; `poll` tömmer kanalen.
//!
//! Skalan i `Rendered.req.scale` är den *faktiskt använda* skalan: den begränsas så att bilden
//! högst blir 32 megapixlar och högst 8192 px på någon sida (se `effective_scale`). Vyn ska
//! därför räkna med bildens riktiga storlek (`image.size`), inte den begärda skalan.

use std::collections::{HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use egui::ColorImage;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::hayro_syntax::{DecryptionError, LoadPdfError};
use hayro::hayro_interpret::TransformExt;
use hayro::hayro_interpret::kurbo::Affine;
use hayro::hayro_interpret::{InterpreterCache, InterpreterSettings};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};

use crate::i18n::t;

/// Felet när en sidas text inte kunde läsas (sidan saknar då användbar text, vilket inte är ett visningsfel).
pub fn text_error() -> &'static str {
    t("Texten kunde inte läsas")
}

/// Om `error` är [`text_error`] (på aktuellt språk).
pub fn is_text_error(error: &str) -> bool {
    error == text_error()
}

use super::text::{PageText, extract_page};

/// Största antal bildpunkter per renderad sida.
pub const MAX_PIXELS: f64 = 32_000_000.0;
/// Största antal bildpunkter på en sida av bilden.
pub const MAX_SIDE: f64 = 8192.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    NeedsPassword,
    WrongPassword,
    Invalid(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Recolor {
    #[default]
    None,
    /// Inverterar ljusheten men behåller nyans och mättnad (HSL: L → 1 − L), så bilder och färger
    /// förblir någorlunda naturliga: vitt blir svart, svart blir vitt.
    Invert,
    /// Papperston: svart blir mörkbrunt (#3B2A1A), vitt blir #F4ECD8, däremellan mappas ljusheten.
    Sepia,
}

#[derive(Clone, Copy, Debug)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub page: usize,
    pub scale: f32,
    pub rotation: u16,
    pub recolor: Recolor,
}

pub enum EngineEvent {
    Rendered { req: RenderRequest, image: ColorImage },
    Text { page: usize, text: Arc<PageText> },
    Failed { page: usize, error: String },
}

/// Nyckel för att känna igen identiska renderingsjobb.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    page: usize,
    scale_bits: u32,
    rotation: u16,
    recolor: Recolor,
}

impl Key {
    fn of(r: &RenderRequest) -> Key {
        Key { page: r.page, scale_bits: r.scale.to_bits(), rotation: norm_rotation(r.rotation), recolor: r.recolor }
    }
}

fn norm_rotation(r: u16) -> u16 {
    (r / 90 % 4) * 90
}

#[derive(Default)]
struct Queues {
    renders: VecDeque<RenderRequest>,
    texts: VecDeque<usize>,
    active_renders: HashSet<Key>,
    active_texts: HashSet<usize>,
    /// Sidor vars text redan har levererats (levereras bara en gång).
    text_done: HashSet<usize>,
    quit: bool,
}

struct Shared {
    q: Mutex<Queues>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queues> {
        self.q.lock().unwrap_or_else(|e| e.into_inner())
    }
}

enum Job {
    Render(RenderRequest),
    Text(usize),
}

pub struct PdfEngine {
    shared: Arc<Shared>,
    rx: Mutex<Receiver<EngineEvent>>,
    sizes: Vec<PageSize>,
}

impl PdfEngine {
    pub fn open(bytes: Arc<Vec<u8>>, password: Option<&str>, ctx: egui::Context) -> Result<Self, OpenError> {
        let pw = password.unwrap_or("");
        let parsed = catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes, pw)))
            .map_err(|_| OpenError::Invalid(t("Filen kunde inte läsas").into()))?;
        let pdf = parsed.map_err(|e| match e {
            LoadPdfError::Decryption(d) => {
                use DecryptionError as D;
                match d {
                    D::PasswordProtected if pw.is_empty() => OpenError::NeedsPassword,
                    D::PasswordProtected => OpenError::WrongPassword,
                    D::UnsupportedAlgorithm => OpenError::Invalid(t("Krypteringen stöds inte").into()),
                    _ => OpenError::Invalid(t("Ogiltig kryptering").into()),
                }
            }
            LoadPdfError::Invalid => OpenError::Invalid(t("Filen är ingen giltig PDF").into()),
        })?;
        let sizes = catch_unwind(AssertUnwindSafe(|| {
            pdf.pages()
                .iter()
                .map(|p| {
                    let (w, h) = p.render_dimensions();
                    PageSize { width: w.max(1.0), height: h.max(1.0) }
                })
                .collect::<Vec<_>>()
        }))
        .map_err(|_| OpenError::Invalid(t("Sidorna kunde inte läsas").into()))?;
        if sizes.is_empty() {
            return Err(OpenError::Invalid(t("Dokumentet har inga sidor").into()));
        }

        let pdf = Arc::new(pdf);
        let shared = Arc::new(Shared { q: Mutex::new(Queues::default()), cv: Condvar::new() });
        let (tx, rx) = channel();
        let n = std::thread::available_parallelism().map_or(2, |n| n.get());
        let workers = n.saturating_sub(1).min(4).max(1);
        for i in 0..workers {
            let (pdf, shared, tx, ctx, sizes) = (pdf.clone(), shared.clone(), tx.clone(), ctx.clone(), sizes.clone());
            // Lösgjord tråd: den avslutas av sig själv när motorn släpps (`quit`).
            let spawned = std::thread::Builder::new()
                .name(format!("pdf-worker-{i}"))
                .stack_size(16 * 1024 * 1024)
                .spawn(move || worker(pdf, shared, tx, ctx, sizes));
            if spawned.is_err() && i == 0 {
                return Err(OpenError::Invalid(t("Kunde inte starta arbetstråd").into()));
            }
        }
        Ok(PdfEngine { shared, rx: Mutex::new(rx), sizes })
    }

    pub fn page_count(&self) -> usize {
        self.sizes.len()
    }

    /// Sidans storlek i visningspunkter (efter sidans `/Rotate`). Ogiltigt index ger A4.
    pub fn page_size(&self, page: usize) -> PageSize {
        self.sizes.get(page).copied().unwrap_or(PageSize { width: 595.0, height: 842.0 })
    }

    /// Ersätter kön: listan är i prioritetsordning (först = viktigast). Gamla, ännu ej påbörjade
    /// jobb stryks. Identiska jobb som redan pågår läggs inte till igen. Skalan begränsas direkt
    /// med `effective_scale`, så resultatets `req.scale` är den begränsade.
    pub fn request(&self, reqs: Vec<RenderRequest>) {
        let mut q = self.shared.lock();
        let mut seen = HashSet::new();
        q.renders.clear();
        for mut r in reqs {
            if r.page >= self.sizes.len() {
                continue;
            }
            r.rotation = norm_rotation(r.rotation);
            r.scale = effective_scale(self.sizes[r.page], r.rotation, r.scale);
            let k = Key::of(&r);
            if q.active_renders.contains(&k) || !seen.insert(k) {
                continue;
            }
            q.renders.push_back(r);
        }
        drop(q);
        self.shared.cv.notify_all();
    }

    /// Textlager för sidorna (i prioritetsordning); levereras en gång per sida. Sidor som redan
    /// levererats eller pågår hoppas över.
    pub fn request_text(&self, pages: Vec<usize>) {
        let mut q = self.shared.lock();
        q.texts.clear();
        let mut seen = HashSet::new();
        for p in pages {
            if p < self.sizes.len() && !q.text_done.contains(&p) && !q.active_texts.contains(&p) && seen.insert(p) {
                q.texts.push_back(p);
            }
        }
        drop(q);
        self.shared.cv.notify_all();
    }

    /// Hämtar färdiga resultat utan att blockera.
    pub fn poll(&self) -> Vec<EngineEvent> {
        let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
        rx.try_iter().collect()
    }
}

impl Drop for PdfEngine {
    fn drop(&mut self) {
        let mut q = self.shared.lock();
        q.quit = true;
        q.renders.clear();
        q.texts.clear();
        drop(q);
        self.shared.cv.notify_all();
    }
}

/// Skalan som faktiskt används: begärd skala, men så att bilden högst blir 32 MP och 8192 px per
/// sida (efter användarens rotation). Aldrig under 0,01.
pub fn effective_scale(size: PageSize, rotation: u16, scale: f32) -> f32 {
    let (w, h) = view_size(size, rotation);
    let (w, h) = (w as f64, h as f64);
    let limit = (MAX_SIDE / w).min(MAX_SIDE / h).min((MAX_PIXELS / (w * h)).sqrt()) * 0.999;
    let s = if scale.is_finite() { scale as f64 } else { 1.0 };
    s.min(limit).max(0.01) as f32
}

/// Sidans storlek i pt efter användarens rotation.
fn view_size(size: PageSize, rotation: u16) -> (f32, f32) {
    if norm_rotation(rotation) % 180 == 90 { (size.height, size.width) } else { (size.width, size.height) }
}

/// Bildens storlek i bildpunkter för en (redan begränsad) skala.
pub fn pixel_size(size: PageSize, rotation: u16, scale: f32) -> (u16, u16) {
    let (w, h) = view_size(size, rotation);
    (((w * scale).round() as u32).clamp(1, 8192) as u16, ((h * scale).round() as u32).clamp(1, 8192) as u16)
}

fn worker(pdf: Arc<Pdf>, shared: Arc<Shared>, tx: Sender<EngineEvent>, ctx: egui::Context, sizes: Vec<PageSize>) {
    let pages = pdf.pages();
    'caches: loop {
        // Cacherna är inte `Send` och byts ut efter en panik.
        let render_cache = RenderCache::new();
        let interp_cache = InterpreterCache::new();
        loop {
            let job = {
                let mut q = shared.lock();
                loop {
                    if q.quit {
                        return;
                    }
                    if let Some(r) = q.renders.pop_front() {
                        q.active_renders.insert(Key::of(&r));
                        break Job::Render(r);
                    }
                    if let Some(p) = q.texts.pop_front() {
                        q.active_texts.insert(p);
                        break Job::Text(p);
                    }
                    q = shared.cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            let (event, panicked) = match &job {
                Job::Render(req) => {
                    let res = catch_unwind(AssertUnwindSafe(|| {
                        let page = &pages[req.page];
                        render_page(page, sizes[req.page], &render_cache, req)
                    }));
                    match res {
                        Ok(image) => (EngineEvent::Rendered { req: req.clone(), image }, false),
                        Err(_) => (EngineEvent::Failed { page: req.page, error: t("Sidan kunde inte renderas").into() }, true),
                    }
                }
                Job::Text(p) => {
                    let res = catch_unwind(AssertUnwindSafe(|| extract_page(&pages[*p], &interp_cache)));
                    match res {
                        Ok(t) => (EngineEvent::Text { page: *p, text: Arc::new(t) }, false),
                        Err(_) => (EngineEvent::Failed { page: *p, error: text_error().into() }, true),
                    }
                }
            };
            {
                let mut q = shared.lock();
                match &job {
                    Job::Render(r) => {
                        q.active_renders.remove(&Key::of(r));
                    }
                    Job::Text(p) => {
                        q.active_texts.remove(p);
                        q.text_done.insert(*p);
                    }
                }
            }
            if tx.send(event).is_err() {
                return;
            }
            ctx.request_repaint();
            if panicked {
                continue 'caches;
            }
        }
    }
}

/// Renderar en sida med användarens rotation inbakad i transformen (inga pixlar roteras om).
fn render_page<'a>(
    page: &'a hayro::hayro_interpret::hayro_syntax::page::Page<'a>,
    size: PageSize,
    cache: &RenderCache<'a>,
    req: &RenderRequest,
) -> ColorImage {
    let rot = norm_rotation(req.rotation);
    let s = req.scale as f64;
    let (pw, ph) = (size.width as f64, size.height as f64);
    let (w, h) = pixel_size(size, rot, req.scale);
    // Rotation medurs i sidans y-ned-rum: (x, y) → ...
    let rotate = match rot {
        90 => Affine::new([0.0, 1.0, -1.0, 0.0, ph, 0.0]),
        180 => Affine::new([-1.0, 0.0, 0.0, -1.0, pw, ph]),
        270 => Affine::new([0.0, -1.0, 1.0, 0.0, 0.0, pw]),
        _ => Affine::IDENTITY,
    };
    let transform = Affine::scale(s) * rotate * page.initial_transform(true).to_kurbo();
    let mut rc = RenderContext::new(w, h);
    render_into(page, cache, &InterpreterSettings::default(), &RenderSettings::default(), &mut rc, transform);
    rc.flush();
    let mut pixmap = Pixmap::new(w, h);
    rc.render_with(
        &mut pixmap,
        &mut Resources::default(),
        RasterizerSettings { target_init: TargetInit::Clear(WHITE), ..Default::default() },
    );
    let mut image =
        ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data_as_u8_slice());
    recolor(&mut image, req.recolor);
    image
}

/// Omfärgning på färdiga (ogenomskinliga) pixlar.
fn recolor(img: &mut ColorImage, mode: Recolor) {
    match mode {
        Recolor::None => {}
        Recolor::Invert => {
            for p in &mut img.pixels {
                let [r, g, b, a] = p.to_array();
                // HSL-ljusheten vänds (L → 1 − L) med oförändrad nyans och mättnad: varje kanal
                // förskjuts med 255 − (max + min).
                let shift = 255 - (r.max(g).max(b) as i32 + r.min(g).min(b) as i32);
                let f = |c: u8| (c as i32 + shift).clamp(0, 255) as u8;
                *p = egui::Color32::from_rgba_premultiplied(f(r), f(g), f(b), a);
            }
        }
        Recolor::Sepia => {
            const INK: [f32; 3] = [59.0, 42.0, 26.0];
            const PAPER: [f32; 3] = [244.0, 236.0, 216.0];
            for p in &mut img.pixels {
                let [r, g, b, a] = p.to_array();
                let y = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0;
                let f = |i: usize| (INK[i] + (PAPER[i] - INK[i]) * y).round() as u8;
                *p = egui::Color32::from_rgba_premultiplied(f(0), f(1), f(2), a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const ENC: &[u8] = include_bytes!("../../tests/data/krypterad.pdf");

    /// Genererar en PDF med Kladds egen skrivare.
    fn make_pdf(md: &str) -> Arc<Vec<u8>> {
        Arc::new(crate::pdf_write::write(md, "Test", &|_| None).expect("pdf").bytes)
    }

    fn sample() -> Arc<Vec<u8>> {
        make_pdf("# Rubrik\n\nÅsa åt äpplen\n\nAndra stycket med text.\n")
    }

    fn open(bytes: Arc<Vec<u8>>) -> PdfEngine {
        PdfEngine::open(bytes, None, egui::Context::default()).expect("öppna")
    }

    fn req(page: usize, scale: f32, rotation: u16) -> RenderRequest {
        RenderRequest { page, scale, rotation, recolor: Recolor::None }
    }

    fn wait_for(e: &PdfEngine, mut want: impl FnMut(&EngineEvent) -> bool) -> Vec<EngineEvent> {
        let end = Instant::now() + Duration::from_secs(30);
        let mut all = Vec::new();
        while Instant::now() < end {
            for ev in e.poll() {
                let done = want(&ev);
                all.push(ev);
                if done {
                    return all;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timeout");
    }

    fn render_one(e: &PdfEngine, r: RenderRequest) -> (RenderRequest, ColorImage) {
        e.request(vec![r]);
        let evs = wait_for(e, |ev| matches!(ev, EngineEvent::Rendered { .. } | EngineEvent::Failed { .. }));
        match evs.into_iter().last().unwrap() {
            EngineEvent::Rendered { req, image } => (req, image),
            EngineEvent::Failed { error, .. } => panic!("misslyckades: {error}"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn pages_and_sizes() {
        let e = open(sample());
        assert_eq!(e.page_count(), 1);
        let s = e.page_size(0);
        assert!(s.width > 500.0 && s.width < 700.0 && s.height > 700.0, "{s:?}");
        // Ogiltigt index ger A4 i stället för panik.
        assert_eq!(e.page_size(99).width, 595.0);
    }

    #[test]
    fn renders_page_with_ink() {
        let e = open(sample());
        let s = e.page_size(0);
        let (req, img) = render_one(&e, req(0, 1.0, 0));
        assert_eq!(img.size, [s.width.round() as usize, s.height.round() as usize]);
        assert_eq!(req.scale, 1.0);
        assert!(img.pixels.iter().any(|p| p.r() < 128), "sidan är tom");
        assert!(img.pixels.iter().filter(|p| p.r() > 250).count() > img.pixels.len() / 2, "bakgrunden är inte vit");
        assert!(img.pixels.iter().all(|p| p.a() == 255));
    }

    #[test]
    fn scale_changes_dimensions() {
        let e = open(sample());
        let s = e.page_size(0);
        let (_, img) = render_one(&e, req(0, 2.0, 0));
        assert_eq!(img.size, [(s.width * 2.0).round() as usize, (s.height * 2.0).round() as usize]);
    }

    #[test]
    fn rotation_swaps_dimensions_and_moves_content() {
        let e = open(sample());
        let (_, a) = render_one(&e, req(0, 1.0, 0));
        let (_, b) = render_one(&e, req(0, 1.0, 90));
        let (_, c) = render_one(&e, req(0, 1.0, 180));
        assert_eq!(b.size, [a.size[1], a.size[0]]);
        assert_eq!(c.size, a.size);
        // Första sidans text ligger upptill: efter 180° ligger den nedtill.
        let first_ink_row = |img: &ColorImage| (0..img.size[1]).find(|&y| (0..img.size[0]).any(|x| img[(x, y)].r() < 128)).unwrap();
        let last_ink_row = |img: &ColorImage| (0..img.size[1]).rev().find(|&y| (0..img.size[0]).any(|x| img[(x, y)].r() < 128)).unwrap();
        assert!(first_ink_row(&a) < a.size[1] / 3);
        assert!(last_ink_row(&c) > c.size[1] * 2 / 3);
        // 90° medurs: texten som låg upptill hamnar till höger.
        let right_ink = (b.size[0] * 2 / 3..b.size[0]).any(|x| (0..b.size[1]).any(|y| b[(x, y)].r() < 128));
        assert!(right_ink);
    }

    #[test]
    fn scale_is_clamped_to_32_megapixels() {
        let size = PageSize { width: 595.0, height: 842.0 };
        let s = effective_scale(size, 0, 50.0);
        let (w, h) = pixel_size(size, 0, s);
        assert!(w as f64 * h as f64 <= MAX_PIXELS && w <= 8192 && h <= 8192, "{w}x{h}");
        assert!(w as f64 * h as f64 > MAX_PIXELS * 0.5 || h >= 8000);
        assert_eq!(effective_scale(size, 0, 1.0), 1.0);
        // En mycket lång sida begränsas av kantlängden.
        let long = PageSize { width: 200.0, height: 20000.0 };
        let (w, h) = pixel_size(long, 0, effective_scale(long, 0, 4.0));
        assert!(h <= 8192 && w >= 1);
    }

    #[test]
    fn clamped_render_reports_actual_scale() {
        let e = open(sample());
        let s = e.page_size(0);
        e.request(vec![req(0, 30.0, 0)]);
        let evs = wait_for(&e, |ev| matches!(ev, EngineEvent::Rendered { .. } | EngineEvent::Failed { .. }));
        let Some(EngineEvent::Rendered { req, image }) = evs.into_iter().last() else { panic!("ingen bild") };
        assert!(req.scale < 30.0);
        assert!(image.size[0] * image.size[1] <= MAX_PIXELS as usize);
        assert_eq!(image.size[0], (s.width * req.scale).round() as usize);
    }

    #[test]
    fn recolor_modes() {
        let e = open(sample());
        let (_, plain) = render_one(&e, req(0, 0.5, 0));
        let (_, inv) = render_one(&e, RenderRequest { recolor: Recolor::Invert, ..req(0, 0.5, 0) });
        let (_, sep) = render_one(&e, RenderRequest { recolor: Recolor::Sepia, ..req(0, 0.5, 0) });
        assert_eq!(plain.pixels[0].r(), 255);
        assert!(inv.pixels[0].r() < 10, "vit bakgrund ska bli mörk");
        assert_eq!(sep.pixels[0].to_array()[..3], [244, 236, 216]);
        assert!(sep.pixels.iter().any(|p| p.r() < 100));
    }

    #[test]
    fn invert_keeps_hue() {
        let mut img = ColorImage::new([2, 1], vec![egui::Color32::from_rgb(200, 50, 50), egui::Color32::WHITE]);
        recolor(&mut img, Recolor::Invert);
        // Rött förblir rött men byter ljushet: (200,50,50) → (55,-95→0..)
        let p = img.pixels[0];
        assert!(p.r() > p.g() && p.r() > p.b());
        assert_eq!(img.pixels[1], egui::Color32::BLACK);
    }

    #[test]
    fn extracts_text() {
        let e = open(sample());
        e.request_text(vec![0, 0]);
        let evs = wait_for(&e, |ev| matches!(ev, EngineEvent::Text { .. } | EngineEvent::Failed { .. }));
        let Some(EngineEvent::Text { page, text }) = evs.into_iter().last() else { panic!("ingen text") };
        assert_eq!(page, 0);
        assert!(text.text().contains("Åsa åt äpplen"), "{:?}", text.text());
        assert!(text.text().contains("Rubrik"));
        assert_eq!(text.find("asa at applen", false, false).len(), 1);
        assert!(text.text().lines().count() >= 3);
        // Levereras bara en gång: en ny begäran ger inget mer.
        e.request_text(vec![0]);
        std::thread::sleep(Duration::from_millis(100));
        assert!(e.poll().iter().all(|ev| !matches!(ev, EngineEvent::Text { .. })));
    }

    #[test]
    fn multi_page_and_replacement_drops_stale_jobs() {
        let md = (1..=6).map(|i| format!("# Sida {i}\n\nText {i}\n")).collect::<Vec<_>>().join("\n<!-- sidbrytning -->\n");
        let e = open(make_pdf(&md));
        let n = e.page_count();
        // Fyll kön med stora jobb och ersätt den direkt: det mesta ska aldrig renderas.
        let many: Vec<_> = (0..200).map(|i| req(i % n, 3.0 + (i / n) as f32 * 0.01, 0)).collect();
        e.request(many);
        e.request(vec![req(0, 0.25, 0)]);
        let evs = wait_for(&e, |ev| matches!(ev, EngineEvent::Rendered { req, .. } if req.scale == 0.25));
        let rendered = evs.iter().filter(|ev| matches!(ev, EngineEvent::Rendered { .. })).count();
        assert!(rendered < 100, "{rendered} renderade: kön ersattes inte");
    }

    #[test]
    fn identical_in_flight_requests_are_deduplicated() {
        let e = open(sample());
        e.request(vec![req(0, 1.0, 0), req(0, 1.0, 0), req(0, 1.0, 360)]);
        wait_for(&e, |ev| matches!(ev, EngineEvent::Rendered { .. }));
        std::thread::sleep(Duration::from_millis(200));
        assert!(e.poll().is_empty(), "dubblettjobb renderades");
    }

    #[test]
    fn password_errors() {
        let bytes = Arc::new(ENC.to_vec());
        let ctx = egui::Context::default();
        assert!(matches!(PdfEngine::open(bytes.clone(), None, ctx.clone()), Err(OpenError::NeedsPassword)));
        assert!(matches!(PdfEngine::open(bytes.clone(), Some("fel"), ctx.clone()), Err(OpenError::WrongPassword)));
        let e = PdfEngine::open(bytes, Some("hemligt"), ctx).expect("rätt lösenord");
        assert_eq!(e.page_count(), 1);
        e.request_text(vec![0]);
        let evs = wait_for(&e, |ev| matches!(ev, EngineEvent::Text { .. } | EngineEvent::Failed { .. }));
        let Some(EngineEvent::Text { text, .. }) = evs.into_iter().last() else { panic!() };
        assert!(text.text().contains("Hemligt dokument"), "{:?}", text.text());
    }

    #[test]
    fn corrupt_files_do_not_panic() {
        let ctx = egui::Context::default();
        let good = sample();
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            b"%PDF-1.7\n".to_vec(),
            b"inte en pdf alls".to_vec(),
            good[..good.len() / 2].to_vec(),
            good[..good.len() / 3].to_vec(),
            good.iter().map(|b| b.wrapping_add(7)).collect(),
        ];
        for c in cases {
            // Öppningen får misslyckas men aldrig panika; lyckas den ska rendering också vara säker.
            if let Ok(e) = PdfEngine::open(Arc::new(c), None, ctx.clone()) {
                e.request(vec![req(0, 1.0, 0)]);
                e.request_text(vec![0]);
                std::thread::sleep(Duration::from_millis(100));
                let _ = e.poll();
            }
        }
        // Skadad mitt i innehållet (flera tecken ändras).
        let mut bad = good.to_vec();
        for i in (bad.len() / 2..bad.len() / 2 + 400).step_by(3) {
            bad[i] = 0xFF;
        }
        if let Ok(e) = PdfEngine::open(Arc::new(bad), None, ctx) {
            e.request(vec![req(0, 1.0, 0)]);
            e.request_text(vec![0]);
            std::thread::sleep(Duration::from_millis(100));
            let _ = e.poll();
        }
    }
}
