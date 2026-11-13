//! Textigenkänning för skannade PDF-sidor: letar skannade sidor, renderar dem i ~300 dpi med en egen
//! motor i bakgrundstråd, kör Tesseract (`crate::ocr`) och gör om resultatet till `PageText` så att
//! sök, markering och kopiering fungerar. `OcrState` hör till `PdfTab`; appen anropar `poll` varje bildruta.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use egui::{FontId, Frame, Id, Margin, Order, Rect, RichText, Stroke, pos2, vec2};

use super::chrome::{Action, PdfTab};
use super::engine::{EngineEvent, PdfEngine, Recolor, RenderRequest};
use super::text::{Glyph, PageText};
use super::view::PdfView;
use crate::i18n::t;
use crate::ocr::{self, Tesseract, Word};
use crate::tf;
use crate::theme::{self, Palette};

/// Renderingsskala för igenkänning: 300 dpi mot PDF:ens 72 punkter per tum.
const SCALE: f32 = 300.0 / 72.0;
/// Så många sidor undersöks efter skannade sidor (text hämtas för varje sida).
const MAX_SCAN: usize = 400;
/// En sida med färre tecken än så räknas som utan text.
const NEAR_EMPTY: usize = 3;

/// Sidor utan text som ser ut att vara bilder, och om Tesseract finns.
struct Probe {
    tess: Option<Tesseract>,
    scanned: Vec<usize>,
}

enum Msg {
    Page { page: usize, text: Arc<PageText>, plain: String },
    Progress { done: usize, total: usize },
    Done(Result<(), String>),
}

struct Job {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
}

/// Vad `poll` vill att appen ska göra.
#[derive(Default)]
pub struct Polled {
    /// Notiser (text, fel).
    pub notices: Vec<(String, bool)>,
    /// Igenkänningen inför Redigera är klar: gör om PDF:en nu.
    pub edit_now: bool,
}

#[derive(Default)]
pub struct OcrState {
    tess: Option<Tesseract>,
    probe: Option<Receiver<Probe>>,
    probed: bool,
    scanned: Vec<usize>,
    /// Engångsnotisen om skannade sidor visas.
    pub notice: bool,
    notice_done: bool,
    job: Option<Job>,
    /// (klara, totalt) medan igenkänning pågår.
    pub progress: Option<(usize, usize)>,
    /// Sidtext per sida (stycken åtskilda av tom rad), för Redigera.
    texts: HashMap<usize, String>,
    /// Bocken i Redigera-dialogen: känn igen texten först.
    pub edit_ocr: bool,
    edit_after: bool,
}

impl Drop for OcrState {
    fn drop(&mut self) {
        if let Some(j) = &self.job {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl OcrState {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// Sidor som Redigera ska fylla på med text (sida, text).
    pub fn page_texts(&self) -> HashMap<usize, String> {
        self.texts.clone()
    }

    /// Ska Redigera-dialogen erbjuda igenkänning? (Tesseract finns, skannade sidor saknar resultat.)
    pub fn edit_offer(&self) -> bool {
        self.tess.is_some() && !self.running() && self.scanned.iter().any(|p| !self.texts.contains_key(p))
    }

    /// Startar igenkänning av de skannade sidor som saknar resultat. Fel är läsbara meddelanden.
    pub fn start(&mut self, view: &PdfView, password: Option<&str>, ctx: &egui::Context, edit_after: bool) -> Result<(), String> {
        if self.running() {
            return Err(t("Textigenkänning pågår redan").to_owned());
        }
        if !self.probed {
            if self.probe.is_none() && ocr::find_exe().is_none() {
                return Err(ocr::install_hint().to_owned());
            }
            return Err(t("Letar efter skannade sidor, försök igen om en stund").to_owned());
        }
        let Some(tess) = self.tess.clone() else { return Err(ocr::install_hint().to_owned()) };
        if tess.lang_arg().is_none() {
            return Err(tf!("Tesseract har inga språkpaket. {hint}", hint = ocr::install_hint()));
        }
        let pages: Vec<usize> = self.scanned.iter().copied().filter(|p| !self.texts.contains_key(p)).collect();
        if pages.is_empty() {
            return Err(if self.scanned.is_empty() { t("Inga skannade sidor hittades") } else { t("Texten är redan igenkänd") }.to_owned());
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (bytes, pw, ctx2, c2) = (view.bytes().clone(), password.map(str::to_owned), ctx.clone(), cancel.clone());
        self.progress = Some((0, pages.len()));
        self.edit_after = edit_after;
        self.job = Some(Job { rx, cancel });
        std::thread::spawn(move || {
            let r = run_job(bytes, pw, pages, tess, ctx2.clone(), c2, &tx);
            let _ = tx.send(Msg::Done(r));
            ctx2.request_repaint();
        });
        Ok(())
    }

    /// Tar emot resultat från trådarna och lägger texten i visaren. Anropas varje bildruta.
    pub fn poll(&mut self, view: &mut PdfView, password: Option<&str>, ctx: &egui::Context) -> Polled {
        let mut out = Polled::default();
        if !self.probed && self.probe.is_none() {
            let (tx, rx) = channel();
            self.probe = Some(rx);
            let (bytes, pw, ctx2) = (view.bytes().clone(), password.map(str::to_owned), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(probe(bytes, pw, ctx2.clone()));
                ctx2.request_repaint();
            });
        }
        if let Some(rx) = &self.probe {
            match rx.try_recv() {
                Ok(p) => {
                    self.probe = None;
                    self.probed = true;
                    self.notice = p.tess.is_some() && !p.scanned.is_empty() && !self.notice_done;
                    self.notice_done |= self.notice;
                    self.tess = p.tess;
                    self.scanned = p.scanned;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.probe = None;
                    self.probed = true;
                }
            }
        }
        let mut finished = None;
        if let Some(job) = &self.job {
            loop {
                match job.rx.try_recv() {
                    Ok(Msg::Page { page, text, plain }) => {
                        view.set_ocr_text(page, text);
                        self.texts.insert(page, plain);
                    }
                    Ok(Msg::Progress { done, total }) => self.progress = Some((done, total)),
                    Ok(Msg::Done(r)) => finished = Some(r),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished.get_or_insert(Err(t("Textigenkänningen avbröts").to_owned()));
                        break;
                    }
                }
            }
        }
        if let Some(r) = finished {
            self.job = None;
            self.progress = None;
            let edit = std::mem::take(&mut self.edit_after);
            match r {
                Ok(()) => {
                    let n = self.texts.len();
                    let msg = if n == 1 { t("Textigenkänningen är klar (1 sida)").to_owned() } else { tf!("Textigenkänningen är klar ({n} sidor)", n = n) };
                    out.notices.push((msg, false));
                    out.edit_now = edit;
                }
                Err(e) => {
                    out.notices.push((tf!("Textigenkänningen misslyckades: {e}", e = e), true));
                    // Redigera fortsätter ändå med de sidor som hann bli klara.
                    out.edit_now = edit && !self.texts.is_empty();
                }
            }
        }
        if self.running() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        out
    }
}

// ------------------------------------------------------------ trådar

fn probe(bytes: Arc<Vec<u8>>, password: Option<String>, ctx: egui::Context) -> Probe {
    let Some(tess) = Tesseract::detect() else { return Probe { tess: None, scanned: Vec::new() } };
    let scanned = find_scanned(&bytes, password.as_deref(), &ctx).unwrap_or_default();
    Probe { tess: Some(tess), scanned }
}

/// Sidor utan (nästan) någon text som innehåller en bild. Egen motor, text för högst `MAX_SCAN` sidor.
fn find_scanned(bytes: &Arc<Vec<u8>>, password: Option<&str>, ctx: &egui::Context) -> Option<Vec<usize>> {
    let engine = PdfEngine::open(bytes.clone(), password, ctx.clone()).ok()?;
    let n = engine.page_count().min(MAX_SCAN);
    engine.request_text((0..n).collect());
    let mut chars: HashMap<usize, usize> = HashMap::new();
    let start = Instant::now();
    while chars.len() < n && start.elapsed() < Duration::from_secs(90) {
        for ev in engine.poll() {
            match ev {
                EngineEvent::Text { page, text } => {
                    chars.insert(page, text.text().chars().filter(|c| !c.is_whitespace()).count());
                }
                EngineEvent::Failed { page, .. } => {
                    chars.insert(page, 0);
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let images = pages_with_images(bytes, password);
    Some((0..n).filter(|p| chars.get(p).is_some_and(|&c| c < NEAR_EMPTY) && images.get(*p).copied().unwrap_or(true)).collect())
}

/// För varje sida: finns en bild (eller ett formulär-objekt som kan innehålla en) bland resurserna?
/// Kan filen inte läsas här antas det, så att en sida utan text ändå erbjuds igenkänning.
fn pages_with_images(bytes: &[u8], password: Option<&str>) -> Vec<bool> {
    use lopdf::{Document, Object};
    let mut opts = lopdf::LoadOptions::default();
    if let Some(p) = password {
        opts.password = Some(p.to_owned());
    }
    let Ok(doc) = Document::load_mem_with_options(bytes, opts) else { return Vec::new() };
    let resolve = |o: &Object| -> Option<Object> {
        match o {
            Object::Reference(id) => doc.get_object(*id).ok().cloned(),
            o => Some(o.clone()),
        }
    };
    doc.get_pages()
        .values()
        .map(|id| {
            let Ok((res, ids)) = doc.get_page_resources(*id) else { return true };
            let res = res.or_else(|| ids.first().and_then(|i| doc.get_dictionary(*i).ok()));
            let Some(xo) = res.and_then(|r| r.get(b"XObject").ok()).and_then(resolve) else { return false };
            let Object::Dictionary(xo) = xo else { return true };
            xo.iter().any(|(_, v)| match resolve(v) {
                Some(Object::Stream(s)) => matches!(s.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()), Some(b"Image") | Some(b"Form")),
                _ => true,
            })
        })
        .collect()
}

fn run_job(bytes: Arc<Vec<u8>>, password: Option<String>, pages: Vec<usize>, tess: Tesseract, ctx: egui::Context, cancel: Arc<AtomicBool>, tx: &Sender<Msg>) -> Result<(), String> {
    let engine = PdfEngine::open(bytes, password.as_deref(), ctx.clone()).map_err(|_| t("kunde inte öppna PDF:en").to_owned())?;
    let total = pages.len();
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    for (i, &page) in pages.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(t("avbröts").to_owned());
        }
        let _ = tx.send(Msg::Progress { done: i, total });
        ctx.request_repaint();
        engine.request(vec![RenderRequest { page, scale: SCALE, rotation: 0, recolor: Recolor::None }]);
        let start = Instant::now();
        let image = loop {
            let mut got = None;
            for ev in engine.poll() {
                match ev {
                    EngineEvent::Rendered { req, image } if req.page == page => got = Some(Ok(image)),
                    EngineEvent::Failed { page: pg, error } if pg == page && !super::engine::is_text_error(&error) => got = Some(Err(error)),
                    _ => {}
                }
            }
            match got {
                Some(r) => break r?,
                None if start.elapsed() > Duration::from_secs(60) => return Err(t("renderingen tog för lång tid").to_owned()),
                None if cancel.load(Ordering::Relaxed) => return Err(t("avbröts").to_owned()),
                None => std::thread::sleep(Duration::from_millis(15)),
            }
        };
        let [w, h] = image.size;
        let raw: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        let png = std::env::temp_dir().join(format!("kladd-ocr-{}-{}.png", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
        let img = image::RgbaImage::from_raw(w as u32, h as u32, raw).ok_or(t("bilden kunde inte skrivas"))?;
        img.save_with_format(&png, image::ImageFormat::Png).map_err(|e| tf!("bilden kunde inte skrivas: {e}", e = e))?;
        let words = tess.recognize(&png);
        let _ = std::fs::remove_file(&png);
        let words = words?;
        // Bildpunkter tillbaka till visningspunkter: bildens bredd mot sidans bredd (skalan kan ha begränsats).
        let factor = w as f32 / engine.page_size(page).width;
        let text = Arc::new(words_to_page_text(&words, factor));
        let _ = tx.send(Msg::Page { page, text, plain: ocr::plain_text(&words) });
    }
    Ok(())
}

// ------------------------------------------------------------ textlager

/// Gör orden (i bildpunkter) till glyfer i visningspunkter: ett tecken per glyf, utspritt över ordets ruta.
pub fn words_to_glyphs(words: &[Word], factor: f32) -> Vec<Glyph> {
    let f = if factor.is_finite() && factor > 0.0 { factor } else { 1.0 };
    let mut out = Vec::new();
    for w in words {
        let n = w.text.chars().count();
        if n == 0 {
            continue;
        }
        let cw = w.width / f / n as f32;
        let (y0, y1) = (w.top / f, (w.top + w.height) / f);
        for (i, c) in w.text.chars().enumerate() {
            let x0 = w.left / f + cw * i as f32;
            out.push(Glyph { text: c.to_string(), rect: Rect::from_min_max(pos2(x0, y0), pos2(x0 + cw, y1)) });
        }
    }
    out
}

pub fn words_to_page_text(words: &[Word], factor: f32) -> PageText {
    PageText::new(words_to_glyphs(words, factor))
}

// ------------------------------------------------------------ gränssnitt

/// Engångsnotisen längst upp i sidytan när dokumentet har skannade sidor.
pub fn notice_bar(ctx: &egui::Context, p: &Palette, tab: &mut PdfTab, anchor: Rect) -> Option<Action> {
    if !tab.ocr.notice || tab.ocr.running() {
        return None;
    }
    let mut act = None;
    let mut close = false;
    let width = 560.0f32.min(anchor.width() - 24.0);
    let pos = pos2(anchor.center().x - width / 2.0, anchor.top() + 12.0);
    egui::Area::new(Id::unique("kladd-pdf-ocr-notice")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        Frame::NONE
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.line))
            .corner_radius(11)
            .inner_margin(Margin::symmetric(14, 10))
            .shadow(ui.visuals().popup_shadow)
            .show(ui, |ui| {
                ui.set_width(width - 28.0);
                ui.label(RichText::new(t("Sidorna är skannade. Känn igen texten för att kunna söka och markera.")).color(p.text));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let b = egui::Button::new(RichText::new(t("Känn igen text")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .min_size(vec2(0.0, 30.0));
                    if ui.add(b).clicked() {
                        act = Some(Action::Ocr);
                        close = true;
                    }
                    if ui.add(egui::Button::new(RichText::new(t("Inte nu")).color(p.muted)).frame(false).min_size(vec2(0.0, 30.0))).clicked() {
                        close = true;
                    }
                });
            });
    });
    if close {
        tab.ocr.notice = false;
    }
    act
}

/// Texten i statusraden medan igenkänning pågår.
pub fn status_text(s: &OcrState) -> Option<String> {
    s.progress.map(|(d, total)| tf!("Känner igen text: sida {n} av {total}", n = (d + 1).min(total), total = total))
}

/// En PDF med en enda sida som bara är en RGB-bild (som en skanning), för tester.
#[cfg(test)]
pub fn test_image_pdf(w: usize, h: usize, rgb: &[u8], page_w: f32, page_h: f32) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offs = Vec::new();
    let content = format!("q {page_w} 0 0 {page_h} 0 0 cm /Im0 Do Q");
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_w} {page_h}] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>").into_bytes(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
        [format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n", rgb.len()).as_bytes(), rgb, b"\nendstream"].concat(),
    ];
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renderar en text-PDF till en bild och gör en skannad PDF av den. Ger (skannad PDF, orden på sidan).
    fn scanned_pdf() -> Vec<u8> {
        let md = "# Rapport\n\nÅsa åt äpplen vid sjön.\n\nSkannade sidor kan nu sökas.\n";
        let src = Arc::new(crate::pdf_write::write(md, "Test", &|_| None).expect("pdf").bytes);
        let engine = PdfEngine::open(src, None, egui::Context::default()).expect("öppna");
        engine.request(vec![RenderRequest { page: 0, scale: 2.5, rotation: 0, recolor: Recolor::None }]);
        let start = Instant::now();
        let img = loop {
            let found = engine.poll().into_iter().find_map(|e| if let EngineEvent::Rendered { image, .. } = e { Some(image) } else { None });
            if let Some(i) = found {
                break i;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "tidsgräns");
            std::thread::sleep(Duration::from_millis(15));
        };
        let size = engine.page_size(0);
        let rgb: Vec<u8> = img.pixels.iter().flat_map(|c| [c.r(), c.g(), c.b()]).collect();
        test_image_pdf(img.size[0], img.size[1], &rgb, size.width, size.height)
    }

    #[test]
    fn bildsida_utan_text_hittas_som_skannad() {
        let bytes = Arc::new(scanned_pdf());
        let found = find_scanned(&bytes, None, &egui::Context::default()).expect("skanning");
        assert_eq!(found, vec![0]);
        assert_eq!(pages_with_images(&bytes, None), vec![true]);
        // En vanlig textsida är inte skannad.
        let text = Arc::new(crate::pdf_write::write("Hej\n", "T", &|_| None).unwrap().bytes);
        assert_eq!(find_scanned(&text, None, &egui::Context::default()), Some(vec![]));
    }

    #[test]
    fn redigera_fyller_pa_sidor_utan_text() {
        let bytes = scanned_pdf();
        let plain = crate::pdf_read::read(&bytes, None).ok().unwrap();
        assert_eq!(plain.pages_without_text, 1);
        let mut ocr = HashMap::new();
        ocr.insert(0usize, "Första stycket.\n\n# inte rubrik".to_owned());
        let with = crate::pdf_read::read_with_ocr(&bytes, None, &ocr).ok().unwrap();
        assert_eq!(with.pages_without_text, 0);
        assert!(with.markdown.contains("Första stycket.\n\n\\# inte rubrik"), "{}", with.markdown);
        // Sidor med egen text påverkas inte.
        let text = crate::pdf_write::write("Egen text\n", "T", &|_| None).unwrap().bytes;
        let r = crate::pdf_read::read_with_ocr(&text, None, &ocr).ok().unwrap();
        assert!(!r.markdown.contains("Första"));
    }

    #[test]
    #[ignore = "kräver Tesseract (apt install tesseract-ocr tesseract-ocr-swe)"]
    fn tesseract_kanner_igen_en_skannad_sida() {
    let Some(tess) = Tesseract::detect() else {
            eprintln!("Tesseract saknas, hoppar över");
            return;
        };
        let bytes = Arc::new(scanned_pdf());
        let (tx, rx) = channel();
        let ctx = egui::Context::default();
        run_job(bytes, None, vec![0], tess, ctx, Arc::new(AtomicBool::new(false)), &tx).expect("igenkänning");
        let mut got = None;
        while let Ok(m) = rx.try_recv() {
            if let Msg::Page { text, plain, .. } = m {
                got = Some((text, plain));
            }
        }
        let (text, plain) = got.expect("en sida");
        assert!(plain.contains("äpplen"), "{plain}");
        assert!(!text.find("sjön", false, false).is_empty(), "{}", text.text());
        assert!(!text.find("sökas", false, false).is_empty(), "{}", text.text());
        // Rutorna ligger inom sidan (visningspunkter).
        assert!(text.glyphs.iter().all(|g| g.rect.min.x >= 0.0 && g.rect.max.x < 600.0 && g.rect.max.y < 850.0));
    }

    fn word(text: &str, left: f32, top: f32, width: f32, height: f32) -> Word {
        Word { text: text.into(), left, top, width, height, conf: 90.0, block: 1, par: 1, line: 1 }
    }

    #[test]
    fn glyfer_sprids_over_ordets_ruta() {
        // 300 dpi: faktor 300/72. Ordet "Hej" är 125 px brett → 30 pt, tre tecken à 10 pt.
        let f = 300.0 / 72.0;
        let g = words_to_glyphs(&[word("Hej", 125.0, 83.0, 125.0, 41.5)], f);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].text, "H");
        assert!((g[0].rect.min.x - 30.0).abs() < 0.01);
        assert!((g[0].rect.width() - 10.0).abs() < 0.01);
        assert!((g[2].rect.max.x - 60.0).abs() < 0.01);
        assert!((g[1].rect.min.y - 19.92).abs() < 0.01 && (g[1].rect.height() - 9.96).abs() < 0.01);
    }

    #[test]
    fn tecken_utan_ruta_hoppas_over_och_dalig_faktor_tolereras() {
        let g = words_to_glyphs(&[word("åäö", 0.0, 0.0, 30.0, 10.0)], 0.0);
        assert_eq!(g.len(), 3);
        assert_eq!(g[2].text, "ö");
        assert!((g[2].rect.max.x - 30.0).abs() < 0.01);
        assert!(words_to_glyphs(&[], 1.0).is_empty());
    }

    #[test]
    fn textlager_av_ord_ger_sokbar_text_med_mellanrum() {
        let f = 1.0;
        let words = vec![word("Hej", 10.0, 10.0, 30.0, 12.0), word("världen", 50.0, 10.0, 70.0, 12.0), word("Nästa", 10.0, 40.0, 50.0, 12.0)];
        let t = words_to_page_text(&words, f);
        assert_eq!(t.text(), "Hej världen\nNästa");
        let hit = t.find("värld", false, false);
        assert_eq!(hit.len(), 1);
        assert_eq!(t.text_of(hit[0].clone()), "värld");
        assert_eq!(t.rects(hit[0].clone()).len(), 1);
        // Klick mitt i ordet "Nästa" ger ordet.
        let g = t.glyph_at(pos2(30.0, 46.0)).unwrap();
        assert_eq!(t.text_of(t.word_at(g)), "Nästa");
    }

    #[test]
    fn statusen_raknar_sidor() {
        let mut s = OcrState::default();
        assert_eq!(status_text(&s), None);
        s.progress = Some((0, 3));
        assert_eq!(status_text(&s).unwrap(), "Känner igen text: sida 1 av 3");
        s.progress = Some((3, 3));
        assert_eq!(status_text(&s).unwrap(), "Känner igen text: sida 3 av 3");
    }
}
