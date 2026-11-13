//! Stora filer i appen: vilka flikar som visas i den virtualiserade vyn (`bigtext`), inläsning i
//! bakgrunden med förlopp och sökning i bitar utan att gränssnittet fryser.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::{Align2, Color32, Context, FontFamily, FontId, Rect, Stroke, pos2, vec2};

use crate::bigtext::{self, BigView};
use crate::doc::{Document, LoadError};
use crate::search::{Match, Results, Spec, TextSearcher};
use crate::theme::Palette;
use crate::tf;

/// Så många träffar samlas som mest i en stor fil (sedan visas "N+ träffar").
pub const MAX_MATCHES: usize = 50_000;
/// Hur mycket text som genomsöks per steg, och hur länge en bildruta får gå åt till sökning.
const CHUNK: usize = 256 * 1024;
const BUDGET: Duration = Duration::from_millis(6);

pub type Loaded = Result<(Document, Option<BigView>), LoadError>;

/// En fil som läses in i bakgrunden.
pub struct Loading {
    pub path: PathBuf,
    pub total: u64,
    progress: Arc<AtomicU64>,
    rx: Receiver<Loaded>,
}

impl Loading {
    pub fn fraction(&self) -> f32 {
        (self.progress.load(Ordering::Relaxed) as f32 / self.total.max(1) as f32).clamp(0.0, 1.0)
    }
}

/// Sökning som pågår (eller är klar) i den aktiva stora filen.
struct Scan {
    key: u64,
    searcher: Option<TextSearcher>,
    pos: usize,
    matches: Vec<Match>,
    error: Option<String>,
    done: bool,
    capped: bool,
    snapshot: Arc<Results>,
}

#[derive(Default)]
pub struct State {
    views: HashMap<u64, BigView>,
    /// Flikar som avgjorts vara vanliga (avgörandet ändras inte medan fliken lever).
    small: HashSet<u64>,
    pub loading: Vec<Loading>,
    scan: Option<Scan>,
}

/// Filändelser som aldrig läses i den virtualiserade textvyn.
fn is_container(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "docx" | "dotx" | "doc" | "odt" | "pdf")
}

/// Ska filen läsas in i en bakgrundstråd?
pub fn wants_background(path: &Path) -> bool {
    !is_container(path) && std::fs::metadata(path).is_ok_and(|m| m.len() > bigtext::BACKGROUND_LOAD_BYTES)
}

impl State {
    /// Visas fliken i den virtualiserade vyn? Avgörs en gång per flik (O(n) första gången).
    pub fn is_big(&mut self, doc: &Document) -> bool {
        if self.views.contains_key(&doc.id) {
            return true;
        }
        if self.small.contains(&doc.id) {
            return false;
        }
        if doc.docx || doc.pdf || doc.pdfv.is_some() || !bigtext::is_big(&doc.text) {
            self.small.insert(doc.id);
            return false;
        }
        self.views.insert(doc.id, BigView::new(&doc.text));
        true
    }

    pub fn has_view(&self, id: u64) -> bool {
        self.views.contains_key(&id)
    }

    pub fn view(&mut self, id: u64) -> Option<&mut BigView> {
        self.views.get_mut(&id)
    }

    pub fn forget(&mut self, id: u64) {
        self.views.remove(&id);
        self.small.remove(&id);
    }

    /// Texten ändrades utanför vyn (ersätt alla, återställning): bygg om indexet.
    pub fn invalidate(&mut self, id: u64) {
        if let Some(v) = self.views.get_mut(&id) {
            v.invalidate();
        }
    }

    /// Ändringsräknaren för en stor fil (cachenyckel i stället för en hash över hela texten).
    pub fn rev(&self, id: u64) -> Option<u64> {
        self.views.get(&id).map(|v| v.rev)
    }

    /// Markörens byteposition i en stor fil.
    pub fn caret(&self, id: u64) -> Option<usize> {
        self.views.get(&id).map(|v| v.head)
    }

    /// Antal ord i en stor fil (hålls à jour vid varje ändring).
    pub fn words(&self, id: u64) -> Option<usize> {
        self.views.get(&id).map(BigView::words)
    }

    // ------------------------------------------------------------------ inläsning

    pub fn is_loading(&self, path: &Path) -> bool {
        self.loading.iter().any(|l| l.path == path)
    }

    /// Startar inläsningen av `path` som dokument `id` i en bakgrundstråd.
    pub fn spawn_load(&mut self, ctx: &Context, id: u64, path: &Path, password: Option<String>) {
        let total = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let progress = Arc::new(AtomicU64::new(0));
        let (tx, rx) = channel();
        let (p, pw, prog, ctx2) = (path.to_path_buf(), password, progress.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = Document::load_with_progress(id, &p, pw.as_deref(), Some(&prog)).map(|doc| {
                // Radindex och ordantal byggs här så att första bildrutan inte väntar på dem.
                let big = (!doc.docx && !doc.pdf && bigtext::is_big(&doc.text)).then(|| BigView::new(&doc.text));
                (doc, big)
            });
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        self.loading.push(Loading { path: path.to_path_buf(), total, progress, rx });
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    /// Färdiginlästa filer (och fel). Registrerar den stora vyn för varje lyckad inläsning.
    pub fn poll_loads(&mut self, ctx: &Context) -> Vec<(PathBuf, Loaded)> {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.loading.len() {
            match self.loading[i].rx.try_recv() {
                Ok(r) => {
                    let l = self.loading.remove(i);
                    if let Ok((doc, view)) = &r {
                        match view {
                            // Skickas vidare i resultatet; registreras när fliken installeras.
                            Some(_) => {}
                            None => {
                                self.small.insert(doc.id);
                            }
                        }
                    }
                    done.push((l.path, r));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => i += 1,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let l = self.loading.remove(i);
                    done.push((l.path, Err(LoadError::Message(crate::i18n::t("Inläsningen avbröts.").to_owned()))));
                }
            }
        }
        if !self.loading.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        done
    }

    /// Lägger en förberedd vy för ett installerat dokument.
    pub fn adopt(&mut self, id: u64, view: Option<BigView>) {
        if let Some(v) = view {
            self.views.insert(id, v);
        }
    }

    /// Förloppsruta nere till vänster medan en stor fil läses in.
    pub fn show_progress(&self, ctx: &Context, p: &Palette) {
        let Some(l) = self.loading.first() else { return };
        let name = l.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let pct = (l.fraction() * 100.0) as u32;
        egui::Area::new(egui::Id::unique("kladd-big-progress"))
            .anchor(Align2::LEFT_BOTTOM, vec2(16.0, -44.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let font = FontId::new(13.0, FontFamily::Proportional);
                let msg = tf!("Läser in {name} … {pct} %", name = name, pct = pct);
                let galley = ui.painter().layout_no_wrap(msg, font, p.text);
                let size = vec2(galley.size().x.max(220.0) + 24.0, galley.size().y + 30.0);
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 8.0, p.surface);
                ui.painter().rect_stroke(rect, 8.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
                ui.painter().galley(pos2(rect.left() + 12.0, rect.top() + 8.0), galley, p.text);
                let bar = Rect::from_min_size(pos2(rect.left() + 12.0, rect.bottom() - 12.0), vec2(size.x - 24.0, 4.0));
                ui.painter().rect_filled(bar, 2.0, Color32::from_rgba_unmultiplied(128, 128, 128, 60));
                let fill = Rect::from_min_size(bar.min, vec2(bar.width() * l.fraction(), 4.0));
                ui.painter().rect_filled(fill, 2.0, p.accent);
            });
    }

    // ------------------------------------------------------------------ sökning

    /// Pågår en sökning (då ritas nästa bildruta direkt så att den fortsätter)?
    pub fn scanning(&self) -> bool {
        self.scan.as_ref().is_some_and(|s| !s.done)
    }

    /// Träffarna i den stora filen `doc`. Söker i bitar med en tidsbudget per bildruta och slutar
    /// vid `MAX_MATCHES`; `partial` är sant tills sökningen är klar (eller avbruten vid taket).
    pub fn search(&mut self, doc: &Document, spec: &Spec) -> Arc<Results> {
        let rev = self.views.get(&doc.id).map_or(0, |v| v.rev);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (doc.id, rev, doc.text.len()).hash(&mut h);
        spec.hash(&mut h);
        let key = h.finish();
        if self.scan.as_ref().is_none_or(|s| s.key != key) {
            let (searcher, error) = match TextSearcher::new(spec) {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(e)),
            };
            self.scan = Some(Scan {
                key,
                searcher,
                pos: 0,
                matches: Vec::new(),
                done: error.is_some() || spec.query.is_empty(),
                capped: false,
                snapshot: Arc::new(Results { error: error.clone(), ..Default::default() }),
                error,
            });
        }
        let Some(s) = self.scan.as_mut() else { return Arc::default() };
        if !s.done {
            let t0 = Instant::now();
            while !s.done && t0.elapsed() < BUDGET {
                s.step(&doc.text);
            }
            s.snapshot = Arc::new(Results { matches: s.matches.clone(), error: s.error.clone(), partial: !s.done || s.capped });
        }
        s.snapshot.clone()
    }
}

impl Scan {
    /// Söker ett stycke text (hela rader, plus en rad till så att träffar över radbrytningen hittas).
    fn step(&mut self, text: &str) {
        let Some(searcher) = &self.searcher else {
            self.done = true;
            return;
        };
        let n = text.len();
        if self.pos >= n {
            self.done = true;
            return;
        }
        let to = line_end_after(text, self.pos + CHUNK);
        let scan_to = line_end_after(text, to);
        let hits = searcher.find_chunk(text, self.pos, to, scan_to);
        let room = MAX_MATCHES - self.matches.len();
        if hits.len() >= room {
            self.matches.extend(hits.into_iter().take(room));
            self.capped = true;
            self.done = true;
            return;
        }
        self.matches.extend(hits);
        self.pos = to;
        if to >= n {
            self.done = true;
        }
    }
}

/// Ersätter träffen som börjar vid `cur` (bara på träffens rader, med ångra). Sant om något ändrades.
pub fn replace_one(view: &mut BigView, text: &mut String, spec: &Spec, repl: &str, cur: usize) -> bool {
    let (ws, we) = view.line_window(text, cur);
    let Some(r) = crate::search::replace_text(&text[ws..we], spec, repl, Some(cur - ws)) else { return false };
    let new = r.text;
    view.edit(text, ws, we, &new);
    view.select(ws + r.caret, ws + r.caret);
    true
}

/// Första byten efter radbrytningen vid eller efter `pos` (eller textens slut).
pub fn line_end_after(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    match text.as_bytes()[pos..].iter().position(|&b| b == b'\n') {
        Some(i) => pos + i + 1,
        None => text.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(q: &str) -> Spec {
        Spec { query: q.into(), ..Default::default() }
    }

    fn scan_all(text: &str, sp: &Spec) -> (Vec<Match>, bool) {
        let mut s = Scan {
            key: 0,
            searcher: TextSearcher::new(sp).ok(),
            pos: 0,
            matches: Vec::new(),
            error: None,
            done: false,
            capped: false,
            snapshot: Arc::default(),
        };
        while !s.done {
            s.step(text);
        }
        (s.matches, s.capped)
    }

    #[test]
    fn chunked_search_equals_whole_search() {
        // Text större än flera bitar, med träffar nära gränserna.
        let mut text = String::new();
        for i in 0..40_000 {
            text.push_str(&format!("rad {i} nål och lite fyllnad här\n"));
        }
        for q in ["nål", "rad 39999", "fyllnad här\nrad"] {
            let (chunked, capped) = scan_all(&text, &spec(q));
            let whole = crate::search::search_text(&text, &spec(q));
            assert!(!capped);
            assert_eq!(chunked.len(), whole.matches.len(), "{q}");
            assert!(chunked.iter().zip(&whole.matches).all(|(a, b)| a.start == b.start && a.end == b.end));
        }
    }

    #[test]
    fn search_is_capped() {
        let text = "a\n".repeat(MAX_MATCHES + 5000);
        let (m, capped) = scan_all(&text, &spec("a"));
        assert_eq!(m.len(), MAX_MATCHES);
        assert!(capped);
    }

    #[test]
    fn line_end_after_finds_next_break() {
        let t = "ab\ncd\nef";
        assert_eq!(line_end_after(t, 0), 3);
        assert_eq!(line_end_after(t, 3), 6);
        assert_eq!(line_end_after(t, 6), 8);
        assert_eq!(line_end_after(t, 99), 8);
    }

    #[test]
    fn decides_once_per_doc() {
        let mut st = State::default();
        let mut d = Document::new_untitled(1, 1);
        d.text = "x".repeat(bigtext::BIG_BYTES);
        assert!(st.is_big(&d));
        d.text.truncate(10);
        assert!(st.is_big(&d), "avgörandet ändras inte vid ändring");
        let mut e = Document::new_untitled(2, 1);
        e.text = "kort".into();
        assert!(!st.is_big(&e));
        e.text = "x".repeat(bigtext::BIG_BYTES);
        assert!(!st.is_big(&e));
        st.forget(1);
        assert!(!st.has_view(1));
    }

    #[test]
    fn large_files_load_in_background_except_containers() {
        let dir = std::env::temp_dir().join(format!("kladd-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("stor.log");
        std::fs::write(&big, vec![b'a'; bigtext::BACKGROUND_LOAD_BYTES as usize + 1]).unwrap();
        assert!(wants_background(&big));
        let small = dir.join("liten.txt");
        std::fs::write(&small, "hej").unwrap();
        assert!(!wants_background(&small));
        let docx = dir.join("stor.docx");
        std::fs::write(&docx, vec![b'a'; bigtext::BACKGROUND_LOAD_BYTES as usize + 1]).unwrap();
        assert!(!wants_background(&docx));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
