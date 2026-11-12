//! Automatisk säkerhetskopia av osparade flikar och återställning efter en krasch.
//!
//! Var 30:e sekund (och när fönstret tappar fokus) skrivs en ögonblicksbild av varje osparad flik till
//! `$XDG_DATA_HOME/kladd/återställning/`: en JSON-fil per flik (`<pid>-<flik>.json`) och, om fliken har
//! inbäddade bilder, en mapp `<pid>-<flik>.bilder/` med bara de bilder som texten använder.
//! Serialiseringen sker i gränssnittstråden; själva skrivandet i en bakgrundstråd.
//! Ögonblicksbilden tas bort när fliken sparas, stängs eller blir ren igen. Vid nästa start erbjuds
//! bilder vars process inte längre lever (`/proc/<pid>`) som återställning.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use egui::{Align, Color32, FontId, Frame, Id, Key, Layout, Margin, RichText, Stroke, vec2};
use serde::{Deserialize, Serialize};

use crate::app::quiet_button;
use crate::doc::{Document, LineEnding, TextEncoding};
use crate::theme::{self, Palette};

const INTERVAL: Duration = Duration::from_secs(30);
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ImageRef {
    pub name: String,
    /// Filnamn i sidovägsmappen.
    pub file: String,
    pub extent: Option<(i64, i64)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub version: u32,
    pub pid: u32,
    pub id: u64,
    pub path: Option<PathBuf>,
    pub title: String,
    pub text: String,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    pub docx: bool,
    pub odt: bool,
    pub pdf: bool,
    pub word_view: bool,
    /// Sekunder sedan 1970.
    pub saved_at: u64,
    pub images: Vec<ImageRef>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Snapshot {
    /// Ögonblicksbilden av en flik, plus bilddata (i samma ordning som `images`).
    pub fn from_doc(doc: &Document, pid: u32, saved_at: u64) -> (Self, Vec<Vec<u8>>) {
        let mut images = Vec::new();
        let mut data = Vec::new();
        for (name, img) in &doc.images {
            // Samma skrivsätt som i texten: mellanslag blir %20.
            let marker = format!("{}{}", crate::docx::IMAGE_SCHEME, name.replace(' ', "%20"));
            if !doc.text.contains(&marker) {
                continue;
            }
            images.push(ImageRef { name: name.clone(), file: format!("{}.bin", data.len()), extent: img.extent });
            data.push(img.bytes.clone());
        }
        let snap = Self {
            version: VERSION,
            pid,
            id: doc.id,
            path: doc.path.clone(),
            title: doc.title(),
            text: doc.text.clone(),
            encoding: doc.encoding,
            line_ending: doc.line_ending,
            docx: doc.docx,
            odt: doc.docx && doc.extension() == "odt",
            pdf: doc.pdf,
            word_view: doc.word_view,
            saved_at,
            images,
        };
        (snap, data)
    }
}

fn content_hash(doc: &Document) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    doc.text.hash(&mut h);
    (doc.encoding as u8).hash(&mut h);
    (doc.line_ending as u8).hash(&mut h);
    doc.images.len().hash(&mut h);
    h.finish()
}

/// Mappen där ögonblicksbilderna ligger.
pub fn default_dir() -> PathBuf {
    crate::platform::data_dir().join("återställning")
}

fn json_path(dir: &Path, pid: u32, id: u64) -> PathBuf {
    dir.join(format!("{pid}-{id}.json"))
}

fn side_dir(json: &Path) -> PathBuf {
    json.with_extension("bilder")
}

/// Skriver via en temporär fil och döper om, så att en krasch mitt i aldrig lämnar en halv fil.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_snapshot(dir: &Path, snap: &Snapshot, images: &[Vec<u8>]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = json_path(dir, snap.pid, snap.id);
    let side = side_dir(&json);
    if images.is_empty() {
        let _ = std::fs::remove_dir_all(&side);
    } else {
        std::fs::create_dir_all(&side)?;
        for (r, bytes) in snap.images.iter().zip(images) {
            write_atomic(&side.join(&r.file), bytes)?;
        }
        // Bilder som inte längre används.
        if let Ok(rd) = std::fs::read_dir(&side) {
            for e in rd.flatten() {
                if !snap.images.iter().any(|r| e.file_name().to_string_lossy() == r.file) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    let bytes = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
    write_atomic(&json, &bytes)
}

fn delete_files(json: &Path) {
    let _ = std::fs::remove_file(json);
    let _ = std::fs::remove_dir_all(side_dir(json));
}

enum Job {
    Write(Box<Snapshot>, Vec<Vec<u8>>),
    Delete(u64),
}

pub struct Recovery {
    dir: PathBuf,
    pid: u32,
    tx: Option<Sender<Job>>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Antal skrivjobb som ännu inte är klara.
    busy: Arc<AtomicUsize>,
    /// Flik-id → hash för det som senast skickades till skrivtråden.
    written: HashMap<u64, u64>,
    next_at: Instant,
    was_focused: bool,
}

impl Recovery {
    pub fn new() -> Self {
        Self::with_dir(default_dir())
    }

    pub fn with_dir(dir: PathBuf) -> Self {
        let pid = std::process::id();
        let (tx, rx) = channel::<Job>();
        let busy = Arc::new(AtomicUsize::new(0));
        let handle = {
            let (dir, busy) = (dir.clone(), busy.clone());
            std::thread::spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Write(snap, images) => {
                            // Ett misslyckat försök är inte värt att störa användaren med; nästa varv försöker igen.
                            let _ = write_snapshot(&dir, &snap, &images);
                            busy.fetch_sub(1, Ordering::SeqCst);
                        }
                        Job::Delete(id) => delete_files(&json_path(&dir, pid, id)),
                    }
                }
            })
        };
        Self {
            dir,
            pid,
            tx: Some(tx),
            handle: Some(handle),
            busy,
            written: HashMap::new(),
            next_at: Instant::now() + INTERVAL,
            was_focused: true,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Anropas varje bildruta. Städar direkt, skriver när det är dags.
    pub fn tick(&mut self, ctx: &egui::Context, docs: &[Document], enabled: bool) {
        if !enabled {
            self.forget_all();
            return;
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let lost_focus = self.was_focused && !focused;
        self.was_focused = focused;
        let now = Instant::now();
        let due = lost_focus || now >= self.next_at;
        if self.sync(docs, due) {
            self.next_at = now + INTERVAL;
        } else if due {
            // Förra skrivningen pågår fortfarande: försök igen strax.
            self.next_at = now + Duration::from_secs(1);
        }
        ctx.request_repaint_after(self.next_at.saturating_duration_since(Instant::now()));
    }

    /// Tar bort ögonblicksbilder för stängda och rena flikar och, om `write`, skriver ändrade osparade flikar.
    /// Returnerar sant om en skrivomgång gjordes.
    pub fn sync(&mut self, docs: &[Document], write: bool) -> bool {
        let stale: Vec<u64> = self
            .written
            .keys()
            .copied()
            .filter(|id| !docs.iter().any(|d| d.id == *id && d.dirty))
            .collect();
        for id in stale {
            self.forget(id);
        }
        if !write || self.busy.load(Ordering::SeqCst) > 0 {
            return false;
        }
        let saved_at = unix_now();
        for doc in docs.iter().filter(|d| d.dirty) {
            let hash = content_hash(doc);
            if self.written.get(&doc.id) == Some(&hash) {
                continue;
            }
            let (snap, images) = Snapshot::from_doc(doc, self.pid, saved_at);
            self.busy.fetch_add(1, Ordering::SeqCst);
            if let Some(tx) = &self.tx {
                if tx.send(Job::Write(Box::new(snap), images)).is_err() {
                    self.busy.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
            }
            self.written.insert(doc.id, hash);
        }
        true
    }

    /// Nästa varv skriver direkt (t.ex. efter en återställning).
    pub fn write_soon(&mut self) {
        self.next_at = Instant::now();
    }

    /// Tar bort fliken id:s ögonblicksbild (sparad, stängd eller släng).
    pub fn forget(&mut self, id: u64) {
        self.written.remove(&id);
        if let Some(tx) = &self.tx {
            let _ = tx.send(Job::Delete(id));
        }
    }

    fn forget_all(&mut self) {
        let ids: Vec<u64> = self.written.keys().copied().collect();
        for id in ids {
            self.forget(id);
        }
    }

    /// Vid avslut: städa och vänta in skrivtråden.
    pub fn shutdown(&mut self, docs: &[Document]) {
        self.sync(docs, false);
        self.tx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

pub use crate::platform::pid_alive;

pub struct Found {
    pub json: PathBuf,
    pub snap: Snapshot,
}

/// Ögonblicksbilder i `dir` som hör till processer som inte längre lever, äldst först.
pub fn find_orphans(dir: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<Found> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(snap) = serde_json::from_slice::<Snapshot>(&bytes) else { continue };
        if alive(snap.pid) {
            continue;
        }
        out.push(Found { json: path, snap });
    }
    out.sort_by(|a, b| a.snap.saved_at.cmp(&b.snap.saved_at).then_with(|| a.json.cmp(&b.json)));
    out
}

impl Found {
    pub fn images(&self) -> Vec<(String, crate::docx::Image)> {
        let side = side_dir(&self.json);
        self.snap
            .images
            .iter()
            .filter_map(|r| {
                let bytes = std::fs::read(side.join(&r.file)).ok()?;
                Some((r.name.clone(), crate::docx::Image { bytes, extent: r.extent }))
            })
            .collect()
    }

    pub fn discard(&self) {
        delete_files(&self.json);
    }

    /// Bygger en osparad flik av ögonblicksbilden. Word/ODT läses om från originalfilen (om den finns)
    /// så att det förlustfria paketet och bilderna finns kvar; därefter ersätts texten.
    pub fn into_document(&self, id: u64, untitled_index: usize) -> Document {
        let s = &self.snap;
        let loaded = s
            .path
            .as_ref()
            .filter(|p| p.is_file())
            .and_then(|p| Document::load_with_password(id, p, None).ok());
        let mut doc = match loaded {
            Some(d) => d,
            None => {
                let mut d = Document::new_untitled(id, untitled_index);
                d.path = s.path.clone();
                d.docx = s.docx;
                d.word_view = s.word_view;
                // En PDF utan original kan inte sparas som PDF-läsning; den blir vanlig text.
                d.pdf = false;
                d
            }
        };
        doc.set_recovered(s.text.clone(), s.encoding, s.line_ending);
        for (name, img) in self.images() {
            match doc.images.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = img,
                None => doc.images.push((name, img)),
            }
        }
        doc
    }
}

// ------------------------------------------------------------------ dialog

pub struct Prompt {
    pub items: Vec<(Found, bool)>,
}

impl Prompt {
    pub fn new(found: Vec<Found>) -> Option<Self> {
        (!found.is_empty()).then(|| Self { items: found.into_iter().map(|f| (f, true)).collect() })
    }
}

pub enum Choice {
    Restore,
    Discard,
    Later,
}

/// Lokal tid (år, dag på året, timme, minut, dag i månaden, månad 0-11) för en unix-tid.
type Tm = (i32, u32, u32, u32, u32, usize);

fn local_tm(t: u64) -> Tm {
    use chrono::{DateTime, Datelike, Local, Timelike};
    let dt = DateTime::from_timestamp(t as i64, 0).unwrap_or_default().with_timezone(&Local);
    (dt.year(), dt.ordinal(), dt.hour(), dt.minute(), dt.day(), dt.month0() as usize)
}

/// "i dag 14:32", "i går 09:05" eller "3 nov 14:32".
pub fn when(t: u64, now: u64) -> String {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "maj", "jun", "jul", "aug", "sep", "okt", "nov", "dec"];
    let (a, n) = (local_tm(t), local_tm(now));
    let clock = format!("{:02}:{:02}", a.2, a.3);
    let day = |tm: &Tm| (tm.0, tm.1);
    let yesterday = local_tm(now.saturating_sub(86_400));
    if day(&a) == day(&n) {
        format!("i dag {clock}")
    } else if day(&a) == day(&yesterday) {
        format!("i går {clock}")
    } else {
        format!("{} {} {clock}", a.4, MONTHS[a.5.min(11)])
    }
}

/// Modalen "Återställ osparade ändringar?". Returnerar valet när användaren har gjort ett.
pub fn dialog(ctx: &egui::Context, p: &Palette, prompt: &mut Prompt) -> Option<Choice> {
    let mut choice = None;
    let now = unix_now();
    let modal = egui::Modal::new(Id::unique("kladd-recovery"))
        .backdrop_color(Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
        .frame(
            Frame::NONE
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(14)
                .inner_margin(Margin::same(24))
                .shadow(ctx.global_style().visuals.window_shadow),
        )
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(
                RichText::new("Återställ osparade ändringar?")
                    .font(FontId::new(17.0, theme::semibold()))
                    .color(p.text),
            );
            ui.add_space(6.0);
            ui.label(RichText::new("Kladd avslutades innan det här sparades.").color(p.muted));
            ui.add_space(14.0);
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (found, checked) in &mut prompt.items {
                    let s = &found.snap;
                    let folder = s
                        .path
                        .as_ref()
                        .and_then(|p| p.parent())
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "Ej sparad".into());
                    ui.horizontal(|ui| {
                        ui.checkbox(checked, "");
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(RichText::new(&s.title).font(FontId::new(14.0, theme::medium())).color(p.text));
                            ui.label(RichText::new(format!("{folder} · {}", when(s.saved_at, now))).size(12.0).color(p.muted));
                        });
                    });
                    ui.add_space(6.0);
                }
            });
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let any = prompt.items.iter().any(|(_, c)| *c);
                let restore = egui::Button::new(
                    RichText::new("Återställ markerade").font(FontId::new(13.5, theme::medium())).color(p.on_accent),
                )
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .min_size(vec2(84.0, 32.0));
                if ui.add_enabled(any, restore).clicked() {
                    choice = Some(Choice::Restore);
                }
                if ui.add(quiet_button("Släng", p).min_size(vec2(0.0, 32.0))).clicked() {
                    choice = Some(Choice::Discard);
                }
                if ui.add(quiet_button("Senare", p).min_size(vec2(0.0, 32.0))).clicked() {
                    choice = Some(Choice::Later);
                }
            });
        });
    if modal.should_close() && choice.is_none() {
        choice = Some(Choice::Later);
    }
    if ctx.input(|i| i.key_pressed(Key::Escape)) && choice.is_none() {
        choice = Some(Choice::Later);
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kladd-rec-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn dirty_doc(id: u64, text: &str) -> Document {
        let mut d = Document::new_untitled(id, 1);
        d.text = text.into();
        d.on_edited();
        assert!(d.dirty);
        d
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    fn wait_idle(r: &Recovery) {
        for _ in 0..500 {
            if r.busy.load(Ordering::SeqCst) == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("skrivtråden blev aldrig klar");
    }

    #[test]
    fn snapshot_round_trip() {
        let mut d = dirty_doc(7, "Hej ![x](bild:a%20b.png) världen åäö");
        d.path = Some(std::env::temp_dir().join("x.docx"));
        d.docx = true;
        d.word_view = true;
        d.encoding = TextEncoding::Utf16Le;
        d.line_ending = LineEnding::CrLf;
        d.images.push(("a b.png".into(), crate::docx::Image { bytes: vec![1, 2, 3], extent: Some((10, 20)) }));
        d.images.push(("oanvand.png".into(), crate::docx::Image { bytes: vec![9], extent: None }));
        let (snap, data) = Snapshot::from_doc(&d, 42, 1000);
        assert_eq!(snap.images.len(), 1, "bara bilder som används sparas");
        assert_eq!(data, vec![vec![1, 2, 3]]);
        let json = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snap);
        assert_eq!(back.encoding, TextEncoding::Utf16Le);
        assert_eq!(back.line_ending, LineEnding::CrLf);
        assert!(back.docx && back.word_view && !back.pdf && !back.odt);
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let dir = tmp("atomic");
        let f = dir.join("a.json");
        write_atomic(&f, b"ett").unwrap();
        write_atomic(&f, b"tva").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"tva");
        assert_eq!(files(&dir), vec!["a.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_only_changed_dirty_tabs_and_cleans_up() {
        let dir = tmp("cleanup");
        let mut r = Recovery::with_dir(dir.clone());
        let pid = std::process::id();
        let mut docs = vec![dirty_doc(1, "ett"), dirty_doc(2, "tva"), Document::new_untitled(3, 2)];
        assert!(r.sync(&docs, true));
        wait_idle(&r);
        assert_eq!(files(&dir), vec![format!("{pid}-1.json"), format!("{pid}-2.json")]);

        // Oförändrad text skrivs inte om.
        let m = std::fs::metadata(dir.join(format!("{pid}-1.json"))).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        r.sync(&docs, true);
        wait_idle(&r);
        assert_eq!(std::fs::metadata(dir.join(format!("{pid}-1.json"))).unwrap().modified().unwrap(), m);

        // Ändrad text skrivs om.
        docs[0].text = "ett och mer".into();
        docs[0].on_edited();
        r.sync(&docs, true);
        wait_idle(&r);
        let found = find_orphans(&dir, &|_| false);
        assert!(found.iter().any(|f| f.snap.text == "ett och mer"));

        // Sparad (ren) flik: bilden försvinner.
        docs[0].dirty = false;
        // Stängd flik: bilden försvinner.
        docs.remove(1);
        r.sync(&docs, false);
        r.shutdown(&docs);
        assert!(files(&dir).is_empty(), "{:?}", files(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn forget_removes_snapshot_and_images() {
        let dir = tmp("images");
        let mut r = Recovery::with_dir(dir.clone());
        let mut d = dirty_doc(5, "![a](bild:p.png)");
        d.images.push(("p.png".into(), crate::docx::Image { bytes: vec![7; 100], extent: Some((1, 2)) }));
        let docs = vec![d];
        r.sync(&docs, true);
        wait_idle(&r);
        let pid = std::process::id();
        let side = dir.join(format!("{pid}-5.bilder"));
        assert_eq!(std::fs::read(side.join("0.bin")).unwrap(), vec![7; 100]);

        let found = find_orphans(&dir, &|_| false);
        assert_eq!(found.len(), 1);
        let imgs = found[0].images();
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].0, "p.png");
        assert_eq!(imgs[0].1.extent, Some((1, 2)));
        let doc = found[0].into_document(9, 1);
        assert!(doc.dirty);
        assert_eq!(doc.text, "![a](bild:p.png)");
        assert_eq!(doc.images.len(), 1);

        r.forget(5);
        r.shutdown(&[]);
        assert!(files(&dir).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn orphans_skip_live_processes() {
        let dir = tmp("pid");
        let mine = std::process::id();
        for (pid, id) in [(mine, 1u64), (999_999, 2)] {
            let (mut snap, _) = Snapshot::from_doc(&dirty_doc(id, "x"), pid, 5);
            snap.pid = pid;
            write_snapshot(&dir, &snap, &[]).unwrap();
        }
        assert!(pid_alive(mine));
        assert!(!pid_alive(999_999));
        let found = find_orphans(&dir, &pid_alive);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].snap.pid, 999_999);
        found[0].discard();
        assert_eq!(find_orphans(&dir, &pid_alive).len(), 0);
        assert_eq!(files(&dir), vec![format!("{mine}-1.json")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn when_formats_today() {
        let now = unix_now();
        assert!(when(now, now).starts_with("i dag "));
        assert!(when(now - 86_400, now).starts_with("i går "));
    }
}
