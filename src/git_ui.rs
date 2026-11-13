//! Git i gränssnittet: statusfärger i filträdet, gren i statusraden, ändringsmarkörer och blame i
//! marginalen, jämförelse med HEAD och källkontrollpanelen. Allt som körs mot `git` sker i
//! bakgrundstrådar (se `git.rs`); här tas svaren emot och ritas. Saknas `git` visas inget av detta.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::{
    Align, Align2, Color32, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Order, Pos2, Rect, RectAlign,
    RichText, ScrollArea, Sense, Shape, Stroke, TextEdit, Ui, pos2, vec2,
};

use crate::app::{KladdApp, quiet_button, status_button};
use crate::doc::Document;
use crate::git::{self, BlameLine, Change, FileStatus, Hunk, HunkKind, Snapshot, Step};
use crate::history::{self, DiffRow};
use crate::i18n::t;
use crate::tf;
use crate::theme::{self, Palette};

/// Så ofta (sekunder) status läses om av sig själv när fönstret har fokus.
const REFRESH_SECS: f64 = 15.0;
/// Bredden på blame-kolumnen i marginalen.
const BLAME_W: f32 = 250.0;
/// Högst så lång tid mellan två blame-körningar för en text som ändras.
const BLAME_EVERY: f64 = 1.0;
const ROW_H: f32 = 22.0;

// ============================================================== färger

pub fn change_color(c: Change, p: &Palette) -> Color32 {
    let (dark, light) = match c {
        Change::Modified => ((226, 192, 141), (150, 105, 0)),
        Change::Added | Change::Untracked => ((115, 201, 145), (30, 125, 65)),
        Change::Deleted => return p.error,
        Change::Renamed => ((120, 170, 235), (30, 95, 175)),
        Change::Conflicted => ((255, 140, 85), (200, 75, 15)),
    };
    let (r, g, b) = if p.dark { dark } else { light };
    Color32::from_rgb(r, g, b)
}

fn hunk_color(k: HunkKind, p: &Palette) -> Color32 {
    match k {
        HunkKind::Added => change_color(Change::Added, p),
        HunkKind::Modified => Color32::from_rgb(80, 150, 230),
        HunkKind::Deleted => p.error,
    }
}

// ============================================================== tillstånd

/// Slår upp ändringar per sökväg för filträdet.
#[derive(Default)]
struct Index {
    files: HashMap<String, Change>,
    /// Mappar som innehåller ändringar.
    dirs: HashSet<String>,
    /// Hela mappar som är ospårade (git listar bara mappen, inte filerna i den).
    untracked_dirs: HashSet<String>,
}

impl Index {
    fn build(snap: &Snapshot) -> Self {
        let mut ix = Index::default();
        for f in &snap.status.files {
            let is_dir = f.path.ends_with('/');
            let path = f.path.trim_end_matches('/').to_owned();
            if is_dir {
                ix.untracked_dirs.insert(path.clone());
                ix.dirs.insert(path.clone());
            }
            let mut rest = path.as_str();
            while let Some(i) = rest.rfind('/') {
                rest = &rest[..i];
                ix.dirs.insert(rest.to_owned());
            }
            ix.files.insert(path, f.shown());
        }
        ix
    }

    fn file(&self, key: &str) -> Option<Change> {
        if let Some(c) = self.files.get(key) {
            return Some(*c);
        }
        let mut rest = key;
        while let Some(i) = rest.rfind('/') {
            rest = &rest[..i];
            if self.untracked_dirs.contains(rest) {
                return Some(Change::Untracked);
            }
        }
        None
    }
}

/// Basen (HEAD-versionen) för en flik.
#[derive(Default)]
struct Base {
    path: Option<PathBuf>,
    epoch: u64,
    loaded: bool,
    text: Option<Arc<String>>,
    rx: Option<Receiver<Option<String>>>,
}

#[derive(Default)]
struct DiffCache {
    hash: u64,
    base: Option<Arc<String>>,
    hunks: Arc<Vec<Hunk>>,
}

#[derive(Default)]
struct BlameCache {
    path: Option<PathBuf>,
    /// Hash av texten som blame gäller (eller är beställd för).
    hash: u64,
    epoch: u64,
    asked_at: f64,
    lines: Option<Arc<Vec<BlameLine>>>,
    rx: Option<Receiver<Option<Vec<BlameLine>>>>,
}

/// En åtgärd som körs i bakgrunden (en eller flera git-kommandon efter varandra).
struct PendingOp {
    ok: String,
    fail: String,
    steps: Vec<Step>,
    clear_message: bool,
    then: Option<Box<PendingOp>>,
}

struct Op {
    job: PendingOp,
    rx: Receiver<Result<String, String>>,
}

#[derive(Clone)]
struct HunkPopup {
    doc_id: u64,
    idx: usize,
    text_hash: u64,
    anchor: Pos2,
}

struct Compare {
    title: String,
    rx: Option<Receiver<Result<Vec<DiffRow>, String>>>,
    result: Option<Result<Vec<DiffRow>, String>>,
}

struct BranchPrompt {
    input: String,
    focus: bool,
    error: Option<&'static str>,
}

#[derive(Default)]
pub struct State {
    avail: Option<bool>,
    probe_rx: Option<Receiver<bool>>,
    /// Arbetsmappen som `snap` hör till.
    snap_for: Option<PathBuf>,
    snap: Option<Arc<Snapshot>>,
    index: Index,
    refresh: Option<(PathBuf, Receiver<Option<Snapshot>>)>,
    want_refresh: bool,
    last_refresh: f64,
    was_focused: bool,
    /// Räknas upp när HEAD eller filerna kan ha ändrats (spara, fokus, åtgärd): beställer nya baser.
    epoch: u64,
    op: Option<Op>,
    /// Källkontrollpanelen visas i stället för filträdet.
    pub scm: bool,
    message: String,
    /// Senaste utdata från git (fel i rött).
    output: Option<(bool, String)>,
    commits_closed: bool,
    discard: Option<Vec<FileStatus>>,
    branch_prompt: Option<BranchPrompt>,
    compare: Option<Compare>,
    bases: HashMap<u64, Base>,
    diffs: HashMap<u64, DiffCache>,
    pub blame_on: bool,
    blames: HashMap<u64, BlameCache>,
    popup: Option<HunkPopup>,
}

/// Något panelen ber om (utförs när ritningen är klar).
pub(crate) enum ScmAction {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Discard(Vec<FileStatus>),
    Open(String),
    Compare(String),
    Commit { push: bool },
    Fetch,
    Pull,
    Push,
    Switch(String),
    NewBranch,
    CopyHash(String),
}

fn hash_text(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Mappen och filnamnet att köra git med för en fil.
fn target_of(path: &Path) -> Option<(PathBuf, String)> {
    Some((path.parent()?.to_path_buf(), path.file_name()?.to_string_lossy().into_owned()))
}

/// De första raderna av ett (ofta flerradigt) felmeddelande på en rad, för en notis.
fn one_line(s: &str) -> String {
    let joined = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · ");
    if joined.chars().count() > 300 { format!("{}…", joined.chars().take(300).collect::<String>()) } else { joined }
}

impl State {
    /// Finns git på datorn (och är kontrollen klar)?
    pub fn available(&self) -> bool {
        self.avail == Some(true)
    }

    /// Arkivet för den öppna mappen, när git finns och mappen ligger i ett arkiv.
    pub fn repo(&self) -> Option<&Snapshot> {
        if self.available() { self.snap.as_deref() } else { None }
    }

    /// Beställer ny status och nya jämförelsetexter (efter sparning, fokus eller en åtgärd).
    pub fn bump(&mut self) {
        self.want_refresh = true;
        self.epoch += 1;
    }

    /// Bokstav och färg för en post i filträdet (mappar får bara färg).
    pub fn tree_mark(&self, root: &Path, path: &Path, is_dir: bool, p: &Palette) -> Option<(Option<char>, Color32)> {
        let snap = self.repo()?;
        if self.snap_for.as_deref() != Some(root) {
            return None;
        }
        let rel = path.strip_prefix(root).ok()?;
        let mut key = snap.prefix.clone();
        let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        key.push_str(&parts.join("/"));
        if is_dir {
            if self.index.untracked_dirs.contains(&key) {
                Some((None, change_color(Change::Untracked, p)))
            } else if self.index.dirs.contains(&key) {
                Some((None, change_color(Change::Modified, p)))
            } else {
                None
            }
        } else {
            self.index.file(&key).map(|c| (Some(c.letter()), change_color(c, p)))
        }
    }

    fn start_refresh(&mut self, ctx: &egui::Context, ws: PathBuf, now: f64) {
        let (tx, rx) = channel();
        let dir = ws.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(git::snapshot(&dir));
            ctx.request_repaint();
        });
        self.refresh = Some((ws, rx));
        self.want_refresh = false;
        self.last_refresh = now;
    }

    fn start_op(&mut self, ctx: &egui::Context, job: PendingOp) {
        let Some(root) = self.snap.as_ref().map(|s| s.root.clone()) else { return };
        if self.op.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let steps = job.steps.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(git::run_steps(&root, &steps));
            ctx.request_repaint();
        });
        self.op = Some(Op { job, rx });
    }
}

// ============================================================== marginalen

/// Det marginalen i editorn behöver för den aktiva fliken.
#[derive(Clone)]
pub struct GutterView {
    doc_id: u64,
    text_hash: u64,
    hunks: Arc<Vec<Hunk>>,
    blame: Option<Arc<Vec<BlameLine>>>,
    blame_on: bool,
    now_ms: u64,
    /// Sista radens nummer (från 0), för borttagna rader sist i filen.
    last_line: usize,
}

impl GutterView {
    /// Extra bredd i marginalen för blame-kolumnen.
    pub fn extra_width(&self) -> f32 {
        if self.blame_on { BLAME_W } else { 0.0 }
    }

    fn hunk_for_line(&self, line: usize) -> Option<usize> {
        git::hunk_at(&self.hunks, line).or_else(|| {
            // Rader borttagna sist i filen hör till sista raden.
            let last = self.hunks.len().checked_sub(1)?;
            let h = &self.hunks[last];
            (line == self.last_line && h.kind == HunkKind::Deleted && h.new_start > self.last_line).then_some(last)
        })
    }

    /// Ritar markör och blame för en rad (`r` är radens rektangel, `line` räknas från 0).
    /// Ger den ändring (index, ankarpunkt) som klickades.
    pub fn row(&self, ui: &Ui, p: &Palette, gutter: Rect, r: Rect, line: usize, is_start: bool) -> Option<(usize, Pos2)> {
        let painter = ui.painter();
        if self.blame_on && is_start {
            let area = Rect::from_min_max(pos2(gutter.left() + 6.0, r.top()), pos2(gutter.left() + BLAME_W - 6.0, r.bottom()));
            if let Some(b) = self.blame.as_ref().and_then(|b| b.get(line)) {
                let repeat = line > 0 && self.blame.as_ref().and_then(|bl| bl.get(line - 1)).is_some_and(|prev| prev.sha == b.sha);
                let text = if b.uncommitted() {
                    t("Inte incheckad").to_owned()
                } else {
                    let age = crate::history_ui::format_age(self.now_ms, (b.time.max(0) as u64) * 1000);
                    format!("{}  ·  {}  ·  {}", b.author, age, b.summary)
                };
                let color = if repeat { p.faint.gamma_multiply(0.55) } else { p.muted };
                painter.with_clip_rect(area).text(
                    pos2(area.left(), r.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    FontId::new(11.0, FontFamily::Proportional),
                    color,
                );
            }
        }
        let idx = self.hunk_for_line(line)?;
        let h = &self.hunks[idx];
        let color = hunk_color(h.kind, p);
        let x = gutter.right() - 12.0;
        let hit = match h.kind {
            HunkKind::Added | HunkKind::Modified => {
                painter.rect_filled(Rect::from_min_max(pos2(x, r.top()), pos2(x + 3.0, r.bottom())), 1.0, color);
                Rect::from_min_max(pos2(x - 4.0, r.top()), pos2(x + 8.0, r.bottom()))
            }
            HunkKind::Deleted => {
                if !is_start && line != self.last_line {
                    return None;
                }
                let y = if line == self.last_line && h.new_start > self.last_line { r.bottom() } else { r.top() };
                painter.add(Shape::convex_polygon(
                    vec![pos2(x - 1.0, y - 4.0), pos2(x - 1.0, y + 4.0), pos2(x + 5.0, y)],
                    color,
                    Stroke::NONE,
                ));
                Rect::from_center_size(pos2(x + 2.0, y), vec2(14.0, 12.0))
            }
        };
        if !is_start {
            return None;
        }
        let resp = ui.interact(hit, Id::unique(("kladd-git-hunk", self.doc_id, line, crate::panes::salt())), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        resp.clicked().then(|| (idx, pos2(gutter.right() + 4.0, r.bottom())))
    }
}

impl State {
    /// Förbereder marginalen för `doc`: hämtar HEAD-versionen i bakgrunden, diffar texten och
    /// beställer blame. `None` när inget ska visas.
    pub fn gutter(&mut self, ctx: &egui::Context, doc: &Document, now: f64) -> Option<GutterView> {
        if !self.available() || doc.docx || doc.pdf || doc.pdfv.is_some() || doc.word_view || doc.text.len() > git::MAX_TEXT_BYTES {
            return None;
        }
        let path = doc.path.as_ref()?;
        let (dir, name) = target_of(path)?;
        let epoch = self.epoch;

        let base = self.bases.entry(doc.id).or_default();
        if base.path.as_ref() != Some(path) {
            *base = Base { path: Some(path.clone()), ..Default::default() };
        }
        if let Some(rx) = &base.rx {
            match rx.try_recv() {
                Ok(text) => {
                    base.text = text.map(Arc::new);
                    base.loaded = true;
                    base.rx = None;
                }
                Err(TryRecvError::Disconnected) => {
                    base.loaded = true;
                    base.rx = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if base.rx.is_none() && (!base.loaded || base.epoch != epoch) {
            let (tx, rx) = channel();
            let (dir, name, ctx) = (dir.clone(), name.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(git::head_text(&dir, &name));
                ctx.request_repaint();
            });
            base.rx = Some(rx);
            base.epoch = epoch;
        }
        let base_text = base.text.clone()?;

        let hash = hash_text(&doc.text);
        let d = self.diffs.entry(doc.id).or_default();
        if d.hash != hash || d.base.as_ref().is_none_or(|b| !Arc::ptr_eq(b, &base_text)) {
            d.hunks = Arc::new(git::hunks(&base_text, &doc.text));
            d.hash = hash;
            d.base = Some(base_text);
        }
        let hunks = d.hunks.clone();

        let blame = if self.blame_on { self.blame_lines(ctx, doc, path, &dir, &name, hash, now) } else { None };
        Some(GutterView {
            doc_id: doc.id,
            text_hash: hash,
            hunks,
            blame,
            blame_on: self.blame_on,
            now_ms: history::now_ms(),
            last_line: doc.text.bytes().filter(|b| *b == b'\n').count(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn blame_lines(&mut self, ctx: &egui::Context, doc: &Document, path: &Path, dir: &Path, name: &str, hash: u64, now: f64) -> Option<Arc<Vec<BlameLine>>> {
        let epoch = self.epoch;
        let b = self.blames.entry(doc.id).or_default();
        if b.path.as_deref() != Some(path) {
            *b = BlameCache { path: Some(path.to_path_buf()), ..Default::default() };
            b.asked_at = f64::NEG_INFINITY;
        }
        if let Some(rx) = &b.rx {
            match rx.try_recv() {
                Ok(lines) => {
                    b.lines = lines.map(Arc::new);
                    b.rx = None;
                }
                Err(TryRecvError::Disconnected) => b.rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if b.rx.is_none() && (b.hash != hash || b.epoch != epoch || b.lines.is_none() && b.asked_at < 0.0) {
            if now - b.asked_at >= BLAME_EVERY {
                let (tx, rx) = channel();
                let (dir, name, text, ctx2) = (dir.to_path_buf(), name.to_owned(), doc.text.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(git::blame(&dir, &name, &text));
                    ctx2.request_repaint();
                });
                b.rx = Some(rx);
                b.hash = hash;
                b.epoch = epoch;
                b.asked_at = now;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(BLAME_EVERY));
            }
        }
        b.lines.clone()
    }
}

// ============================================================== källkontrollpanelen

fn section_header(ui: &mut Ui, p: &Palette, title: &str, count: usize, all: Option<(&str, &str)>) -> bool {
    let mut clicked = false;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{title}  {count}")).font(FontId::new(11.0, theme::semibold())).color(p.muted));
        if let Some((label, tip)) = all {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                clicked = crate::workspace::chip(ui, p, label, false, tip).clicked();
            });
        }
    });
    clicked
}

/// Kolumnens knappar i en filrad: (tecken, tips). Ritas längst till höger när raden hålls över.
fn file_row(ui: &mut Ui, p: &Palette, f: &FileStatus, staged: bool, actions: &mut Vec<ScmAction>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
    if hovered {
        ui.painter().rect_filled(rect, 5.0, p.surface_hover);
    }
    let change = if staged { f.index.unwrap_or(Change::Modified) } else if f.conflicted() { Change::Conflicted } else { f.work.unwrap_or(Change::Modified) };
    let color = change_color(change, p);
    let painter = ui.painter().with_clip_rect(rect);
    painter.text(pos2(rect.left() + 12.0, rect.center().y), Align2::CENTER_CENTER, change.letter().to_string(), FontId::new(12.0, theme::semibold()), color);

    // Knapparna först, så att namnet kan kortas efter dem.
    let mut buttons: Vec<(&str, &str, u8)> = Vec::new();
    if staged {
        buttons.push(("−", t("Ta ur kön"), 1));
        buttons.push(("≠", t("Jämför med HEAD"), 0));
    } else if f.conflicted() {
        buttons.push(("+", t("Markera som löst (köa)"), 2));
    } else {
        buttons.push(("+", t("Köa"), 2));
        buttons.push(("×", t("Förkasta ändringarna"), 3));
        buttons.push(("≠", t("Jämför med HEAD"), 0));
    }
    let mut right = rect.right() - 2.0;
    let mut left_limit = rect.right() - 6.0;
    let mut hit_button = false;
    for (glyph, tip, kind) in &buttons {
        let b = Rect::from_min_size(pos2(right - 20.0, rect.top() + 1.0), vec2(20.0, ROW_H - 2.0));
        right -= 20.0;
        left_limit = b.left() - 4.0;
        if !hovered {
            left_limit = rect.right() - 6.0;
            continue;
        }
        let r = ui.interact(b, Id::unique(("kladd-git-file-btn", &f.path, *kind, staged)), Sense::click());
        if r.hovered() {
            ui.painter().rect_filled(b, 4.0, p.line);
            hit_button = true;
        }
        painter.text(b.center(), Align2::CENTER_CENTER, *glyph, FontId::new(14.0, FontFamily::Proportional), if r.hovered() { p.text } else { p.muted });
        let r = r.on_hover_text(*tip);
        if r.clicked() {
            let path = vec![f.path.clone()];
            actions.push(match kind {
                0 => ScmAction::Compare(f.path.clone()),
                1 => ScmAction::Unstage(path),
                2 => ScmAction::Stage(path),
                _ => ScmAction::Discard(vec![f.clone()]),
            });
        }
    }
    let (dir, name) = match f.path.trim_end_matches('/').rfind('/') {
        Some(i) => (&f.path[..i], f.path[i + 1..].to_owned()),
        None => ("", f.path.clone()),
    };
    let clip = ui.painter().with_clip_rect(Rect::from_min_max(pos2(rect.left() + 24.0, rect.top()), pos2(left_limit, rect.bottom())));
    let galley = clip.layout_no_wrap(name, FontId::new(13.0, FontFamily::Proportional), p.text);
    let nw = galley.size().x;
    clip.galley(pos2(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0), galley, p.text);
    if !dir.is_empty() {
        clip.text(pos2(rect.left() + 26.0 + nw + 8.0, rect.center().y), Align2::LEFT_CENTER, dir, FontId::new(11.5, FontFamily::Proportional), p.faint);
    }
    if resp.clicked() && !hit_button {
        actions.push(if change == Change::Deleted { ScmAction::Compare(f.path.clone()) } else { ScmAction::Open(f.path.clone()) });
    }
}

impl State {
    /// Källkontrollpanelens innehåll (i den vänstra panelen i stället för filträdet).
    pub(crate) fn scm_ui(&mut self, ui: &mut Ui, p: &Palette) -> Vec<ScmAction> {
        let mut actions = Vec::new();
        let Some(snap) = self.snap.clone() else { return actions };
        let busy = self.op.is_some();
        let st = &snap.status;
        let staged: Vec<&FileStatus> = st.files.iter().filter(|f| f.staged()).collect();
        let unstaged: Vec<&FileStatus> = st.files.iter().filter(|f| f.unstaged()).collect();
        let conflicts: Vec<&FileStatus> = st.files.iter().filter(|f| f.conflicted()).collect();

        // Gren och synkstatus.
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let name = match (&st.branch, &st.oid) {
                (Some(b), _) => b.clone(),
                (None, Some(o)) => format!("({o})"),
                _ => "HEAD".to_owned(),
            };
            let btn = ui.add(quiet_button(&name, p).min_size(vec2(0.0, 24.0))).on_hover_text(t("Byt gren eller skapa en ny"));
            egui::Popup::menu(&btn).align(RectAlign::BOTTOM_START).width(220.0).show(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for b in &snap.branches {
                    let current = st.branch.as_ref() == Some(b);
                    if ui.selectable_label(current, b).clicked() && !current {
                        actions.push(ScmAction::Switch(b.clone()));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(t("Ny gren…")).clicked() {
                    actions.push(ScmAction::NewBranch);
                    ui.close();
                }
            });
            let sync = match (st.upstream.is_some(), st.ahead, st.behind) {
                (false, ..) => t("ingen fjärrgren").to_owned(),
                (true, 0, 0) => t("uppdaterad").to_owned(),
                (true, a, b) => format!("↑{a} ↓{b}"),
            };
            ui.label(RichText::new(sync).size(12.0).color(p.muted)).on_hover_text(st.upstream.clone().unwrap_or_else(|| t("Push skapar grenen på origin").to_owned()));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            for (label, tip, a) in [
                (t("Hämta"), t("Hämta ändringar från fjärrarkivet (fetch)"), ScmAction::Fetch),
                (t("Pulla"), t("Hämta och slå ihop med fjärrgrenen (pull)"), ScmAction::Pull),
                (t("Pusha"), t("Skicka incheckningarna till fjärrarkivet (push)"), ScmAction::Push),
            ] {
                if ui.add_enabled(!busy, quiet_button(label, p).min_size(vec2(0.0, 24.0))).on_hover_text(tip).clicked() {
                    actions.push(a);
                }
            }
            if busy {
                ui.add(egui::Spinner::new().size(14.0));
            }
        });
        ui.add_space(6.0);

        // Meddelande och knappar.
        let msg_id = Id::unique("kladd-git-message");
        if ui.memory(|m| m.has_focus(msg_id)) && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) && !busy && !staged.is_empty() && !self.message.trim().is_empty() {
            actions.push(ScmAction::Commit { push: false });
        }
        ui.add(
            TextEdit::multiline(&mut self.message)
                .id(msg_id)
                .hint_text(t("Meddelande (Ctrl+Enter för att checka in)"))
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(8, 6)),
        );
        let can = !busy && !staged.is_empty() && !self.message.trim().is_empty();
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            let go = egui::Button::new(RichText::new(t("Checka in")).font(FontId::new(13.0, theme::medium())).color(p.on_accent))
                .fill(if can { p.accent } else { p.accent.gamma_multiply(0.4) })
                .stroke(Stroke::NONE)
                .min_size(vec2(0.0, 26.0));
            let tip = if staged.is_empty() { t("Köa ändringar först") } else { t("Checka in de köade ändringarna (Ctrl+Enter)") };
            if ui.add_enabled(can, go).on_hover_text(tip).clicked() {
                actions.push(ScmAction::Commit { push: false });
            }
            if ui.add_enabled(can, quiet_button(t("Checka in och pusha"), p).min_size(vec2(0.0, 26.0))).clicked() {
                actions.push(ScmAction::Commit { push: true });
            }
        });
        if let Some((err, text)) = &self.output {
            ui.add_space(4.0);
            let color = if *err { p.error } else { p.muted };
            ScrollArea::vertical().id_salt("kladd-git-output").max_height(90.0).auto_shrink([true, true]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).selectable(true));
            });
        }

        ScrollArea::vertical().id_salt("kladd-git-scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if !conflicts.is_empty() {
                section_header(ui, p, t("KONFLIKTER"), conflicts.len(), None);
                for f in &conflicts {
                    file_row(ui, p, f, false, &mut actions);
                }
            }
            if !staged.is_empty() {
                let all = Some((t("Ta ur alla"), t("Ta alla ur kön")));
                if section_header(ui, p, t("KÖADE ÄNDRINGAR"), staged.len(), all) {
                    actions.push(ScmAction::Unstage(staged.iter().map(|f| f.path.clone()).collect()));
                }
                for f in &staged {
                    file_row(ui, p, f, true, &mut actions);
                }
            }
            let plain: Vec<&FileStatus> = unstaged.iter().copied().collect();
            let all = Some((t("Köa alla"), t("Köa alla ändringar")));
            if section_header(ui, p, t("ÄNDRINGAR"), plain.len(), if plain.is_empty() { None } else { all }) {
                actions.push(ScmAction::Stage(plain.iter().map(|f| f.path.clone()).collect()));
            }
            if plain.is_empty() && staged.is_empty() && conflicts.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(t("Inga ändringar")).size(12.5).color(p.muted));
            }
            for f in &plain {
                if !f.conflicted() {
                    file_row(ui, p, f, false, &mut actions);
                }
            }

            ui.add_space(12.0);
            let head = ui.horizontal(|ui| {
                ui.add_space(4.0);
                let arrow = if self.commits_closed { "▸" } else { "▾" };
                ui.add(egui::Label::new(RichText::new(format!("{arrow} {}  {}", t("SENASTE INCHECKNINGAR"), snap.log.len())).font(FontId::new(11.0, theme::semibold())).color(p.muted)).sense(Sense::click()))
            });
            if head.inner.clicked() {
                self.commits_closed = !self.commits_closed;
            }
            if !self.commits_closed {
                if snap.log.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(t("Inga incheckningar än")).size(12.5).color(p.muted));
                }
                let now_ms = history::now_ms();
                for c in &snap.log {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 5.0, p.surface_hover);
                    }
                    let painter = ui.painter().with_clip_rect(rect.shrink2(vec2(4.0, 0.0)));
                    painter.text(rect.left_top() + vec2(8.0, 7.0), Align2::LEFT_TOP, &c.subject, FontId::new(13.0, FontFamily::Proportional), p.text);
                    let short: String = c.hash.chars().take(7).collect();
                    let age = crate::history_ui::format_age(now_ms, (c.time.max(0) as u64) * 1000);
                    painter.text(rect.left_top() + vec2(8.0, 23.0), Align2::LEFT_TOP, format!("{short}  ·  {}  ·  {age}", c.author), FontId::new(11.5, FontFamily::Proportional), p.faint);
                    if resp.on_hover_text(tf!("{subject}\n{hash}\nKlicka för att kopiera incheckningens id", subject = c.subject, hash = c.hash)).clicked() {
                        actions.push(ScmAction::CopyHash(c.hash.clone()));
                    }
                }
            }
        });
        actions
    }
}

// ============================================================== kopplingen till fönstret

/// Ersätter flikens text som ett eget ångra-steg (som `history_ui::restore_into`).
fn set_text_undoable(ctx: &egui::Context, doc: &mut Document, text: String) {
    use egui::text::{CCursor, CCursorRange};
    use egui::text_edit::TextEditState;
    let id = crate::app::editor_id(doc.id);
    let mut state = TextEditState::load(ctx, id).unwrap_or_default();
    let end = doc.text.chars().count();
    let before = state.cursor.char_range().unwrap_or(CCursorRange::two(CCursor::new(end), CCursor::new(end)));
    let mut undoer = state.undoer();
    undoer.add_undo(&(before, doc.text.clone()));
    state.set_undoer(undoer);
    doc.text = text;
    doc.on_edited();
    let end = doc.text.chars().count();
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(end.min(before.primary.index.0)))));
    state.store(ctx, id);
}

impl KladdApp {
    /// Varje bildruta: kontrollerar att git finns, tar emot svar från bakgrundstrådar och beställer ny
    /// status när fönstret får fokus, efter sparning och åtgärder och med jämna mellanrum.
    pub(crate) fn git_tick(&mut self, ctx: &egui::Context) {
        let now = Self::now(ctx);
        let ws = self.settings.workspace.clone();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let mut done: Option<(PendingOp, Result<String, String>)> = None;
        {
            let g = &mut self.git;
            if g.avail.is_none() && g.probe_rx.is_none() {
                let (tx, rx) = channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(git::probe());
                    ctx.request_repaint();
                });
                g.probe_rx = Some(rx);
            }
            if let Some(rx) = &g.probe_rx {
                match rx.try_recv() {
                    Ok(v) => {
                        g.avail = Some(v);
                        g.probe_rx = None;
                    }
                    Err(TryRecvError::Disconnected) => {
                        g.avail = Some(false);
                        g.probe_rx = None;
                    }
                    Err(TryRecvError::Empty) => {}
                }
            }
            if !g.available() {
                return;
            }
            let regained = focused && !g.was_focused;
            g.was_focused = focused;
            if regained {
                g.bump();
            }
            // Ny arbetsmapp: börja om.
            if g.snap_for != ws {
                g.snap_for = ws.clone();
                g.snap = None;
                g.index = Index::default();
                g.refresh = None;
                g.want_refresh = true;
                g.scm = false;
                g.output = None;
                g.message.clear();
            }
            let ids: HashSet<u64> = self.docs.iter().map(|d| d.id).collect();
            g.bases.retain(|id, _| ids.contains(id));
            g.diffs.retain(|id, _| ids.contains(id));
            g.blames.retain(|id, _| ids.contains(id));

            if let Some(op) = &g.op {
                match op.rx.try_recv() {
                    Ok(r) => {
                        if let Some(op) = g.op.take() {
                            done = Some((op.job, r));
                        }
                    }
                    Err(TryRecvError::Disconnected) => g.op = None,
                    Err(TryRecvError::Empty) => {}
                }
            }
            if let Some((dir, rx)) = &g.refresh {
                match rx.try_recv() {
                    Ok(snap) => {
                        if Some(dir) == ws.as_ref() {
                            let old = g.snap.as_ref().and_then(|s| s.status.oid.clone());
                            let new = snap.as_ref().and_then(|s| s.status.oid.clone());
                            if old != new {
                                g.epoch += 1;
                            }
                            g.index = snap.as_ref().map(Index::build).unwrap_or_default();
                            if snap.is_none() {
                                g.scm = false;
                            }
                            g.snap = snap.map(Arc::new);
                        }
                        g.refresh = None;
                    }
                    Err(TryRecvError::Disconnected) => g.refresh = None,
                    Err(TryRecvError::Empty) => {}
                }
            }
            if let Some(ws) = ws {
                if g.refresh.is_none() && (g.want_refresh || (focused && now - g.last_refresh >= REFRESH_SECS)) {
                    g.start_refresh(ctx, ws, now);
                }
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(REFRESH_SECS));
            }
        }
        if let Some((job, r)) = done {
            match r {
                Ok(out) => {
                    self.git.output = if out.is_empty() { None } else { Some((false, out)) };
                    if job.clear_message {
                        self.git.message.clear();
                    }
                    self.notify(ctx, job.ok.clone(), false);
                    if let Some(next) = job.then {
                        self.git.start_op(ctx, *next);
                    }
                }
                Err(e) => {
                    self.notify(ctx, format!("{}: {}", job.fail, one_line(&e)), true);
                    self.git.output = Some((true, e));
                }
            }
            self.git.bump();
        }
    }

    /// Gren och ändringar i statusraden när den öppna mappen är ett git-arkiv.
    pub(crate) fn git_status_chip(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(snap) = self.git.repo() else { return };
        let st = &snap.status;
        let mut text = match (&st.branch, &st.oid) {
            (Some(b), _) => b.clone(),
            (None, Some(o)) => format!("({o})"),
            _ => "HEAD".to_owned(),
        };
        if !st.files.is_empty() {
            text.push('*');
        }
        if st.ahead > 0 {
            text.push_str(&format!("  ↑{}", st.ahead));
        }
        if st.behind > 0 {
            text.push_str(&format!("  ↓{}", st.behind));
        }
        let n = st.files.len();
        let tip = if n == 0 { t("Inga ändringar. Öppna källkontroll (Ctrl+Shift+G)").to_owned() } else { tf!("{n} ändrade filer. Öppna källkontroll (Ctrl+Shift+G)", n = n) };
        if status_button(ui, p, &text, &tip).clicked() {
            self.git_toggle_scm_open();
        }
    }

    /// Visar källkontrollpanelen (eller filträdet om den redan visas).
    pub(crate) fn git_toggle_scm_open(&mut self) {
        if self.git.repo().is_none() {
            return;
        }
        if self.settings.workspace_panel && self.git.scm {
            self.git.scm = false;
        } else {
            self.settings.workspace_panel = true;
            self.git.scm = true;
            self.git.bump();
        }
    }

    /// Ctrl+Skift+G.
    pub(crate) fn git_shortcut(&mut self, ctx: &egui::Context) {
        if !self.git.available() {
            return;
        }
        if self.settings.workspace.is_none() {
            self.notify(ctx, t("Öppna en mapp först (Arkiv, Öppna mapp…)."), true);
        } else if self.git.repo().is_none() {
            self.notify(ctx, t("Mappen är inte ett Git-arkiv."), true);
        } else {
            self.git_toggle_scm_open();
        }
    }

    /// Marginalen för den aktiva fliken.
    pub(crate) fn git_gutter(&mut self, ctx: &egui::Context) -> Option<GutterView> {
        let doc = self.docs.get(self.active)?;
        self.git.gutter(ctx, doc, Self::now(ctx))
    }

    /// Klick på en markör i marginalen: visa de gamla raderna.
    pub(crate) fn git_open_hunk(&mut self, g: &GutterView, idx: usize, anchor: Pos2) {
        self.git.popup = Some(HunkPopup { doc_id: g.doc_id, idx, text_hash: g.text_hash, anchor });
    }

    /// Jämför den aktiva filen med HEAD.
    pub(crate) fn git_compare_active(&mut self, ctx: &egui::Context) {
        let Some(path) = self.docs.get(self.active).and_then(|d| d.path.clone()) else { return };
        self.git_compare_path(ctx, &path);
    }

    fn git_compare_path(&mut self, ctx: &egui::Context, path: &Path) {
        if !self.git.available() {
            return;
        }
        let Some((dir, name)) = target_of(path) else { return };
        let open_text = self.docs.iter().find(|d| d.path.as_deref() == Some(path)).map(|d| d.text.clone());
        let path = path.to_path_buf();
        let title = name_of(&path);
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let r = (|| {
                let head = git::head_text(&dir, &name).ok_or_else(|| t("Filen ligger inte i ett Git-arkiv, är ignorerad eller är inte text.").to_owned())?;
                let new = match open_text {
                    Some(t) => t,
                    None => match std::fs::read(&path) {
                        Ok(b) if b.len() <= git::MAX_TEXT_BYTES && !b.contains(&0) => String::from_utf8_lossy(&b).replace("\r\n", "\n"),
                        Ok(_) => return Err(t("Filen är för stor eller inte text.").to_owned()),
                        Err(e) => return Err(e.to_string()),
                    },
                };
                if new.len() > git::MAX_TEXT_BYTES {
                    return Err(t("Texten är för stor för att jämföras här.").to_owned());
                }
                Ok(history::collapse(history::diff_lines(&head, &new), 3))
            })();
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        self.git.compare = Some(Compare { title, rx: Some(rx), result: None });
    }

    /// Utför det källkontrollpanelen bad om.
    pub(crate) fn git_apply(&mut self, ctx: &egui::Context, actions: Vec<ScmAction>) {
        for a in actions {
            let Some(snap) = self.git.snap.clone() else { return };
            let job = |ok: &str, fail: &str, steps: Vec<Step>| PendingOp { ok: ok.to_owned(), fail: fail.to_owned(), steps, clear_message: false, then: None };
            match a {
                ScmAction::Stage(paths) => self.git.start_op(ctx, job(t("Köade"), t("Kunde inte köa"), vec![git::stage_step(&paths)])),
                ScmAction::Unstage(paths) => self.git.start_op(ctx, job(t("Tog ur kön"), t("Kunde inte ta ur kön"), vec![git::unstage_step(&paths)])),
                ScmAction::Discard(files) => self.git.discard = Some(files),
                ScmAction::Open(rel) => {
                    let path = snap.root.join(&rel);
                    if path.is_file() {
                        self.open_path(ctx, &path);
                    }
                }
                ScmAction::Compare(rel) => {
                    let path = snap.root.join(rel.trim_end_matches('/'));
                    if path.is_file() || !path.exists() {
                        self.git_compare_path(ctx, &path);
                    }
                }
                ScmAction::Commit { push } => {
                    let msg = self.git.message.trim().to_owned();
                    if msg.is_empty() {
                        continue;
                    }
                    let mut j = job(t("Incheckat"), t("Incheckningen misslyckades"), vec![git::commit_step(&msg)]);
                    j.clear_message = true;
                    if push {
                        let st = &snap.status;
                        j.then = Some(Box::new(job(t("Pushat"), t("Push misslyckades"), vec![git::push_step(st.branch.as_deref(), st.upstream.is_some())])));
                    }
                    self.git.start_op(ctx, j);
                }
                ScmAction::Fetch => self.git.start_op(ctx, job(t("Hämtat"), t("Hämtningen misslyckades"), vec![git::step(&["fetch"])])),
                ScmAction::Pull => self.git.start_op(ctx, job(t("Pullat"), t("Pull misslyckades"), vec![git::step(&["pull", "--no-edit"])])),
                ScmAction::Push => {
                    let st = &snap.status;
                    self.git.start_op(ctx, job(t("Pushat"), t("Push misslyckades"), vec![git::push_step(st.branch.as_deref(), st.upstream.is_some())]));
                }
                ScmAction::Switch(name) => {
                    self.git.start_op(ctx, job(&tf!("Bytte till {name}", name = name), t("Kunde inte byta gren"), vec![git::switch_step(&name)]));
                }
                ScmAction::NewBranch => self.git.branch_prompt = Some(BranchPrompt { input: String::new(), focus: true, error: None }),
                ScmAction::CopyHash(h) => {
                    ctx.copy_text(h);
                    self.notify(ctx, t("Kopierade incheckningens id"), false);
                }
            }
        }
    }

    /// Dialogerna: förkasta, ny gren, jämförelse med HEAD och den gamla texten för en ändring.
    pub(crate) fn git_dialogs(&mut self, ctx: &egui::Context, p: &Palette) {
        self.git_hunk_popup(ctx, p);
        self.git_discard_dialog(ctx, p);
        self.git_branch_dialog(ctx, p);
        self.git_compare_dialog(ctx, p);
    }

    fn git_modal(ctx: &egui::Context, p: &Palette, id: &'static str, add: impl FnOnce(&mut Ui)) -> bool {
        egui::Modal::new(Id::unique(id))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
            .frame(
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(14)
                    .inner_margin(Margin::same(24))
                    .shadow(ctx.global_style().visuals.window_shadow),
            )
            .show(ctx, add)
            .should_close()
    }

    fn git_discard_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(files) = self.git.discard.clone() else { return };
        let mut choice: Option<bool> = None;
        let untracked = files.iter().any(|f| f.index.is_none() && f.work == Some(Change::Untracked));
        let close = Self::git_modal(ctx, p, "kladd-git-discard", |ui| {
            ui.set_width(400.0);
            let title = if files.len() == 1 { tf!("Förkasta ändringarna i {name}?", name = name_of(Path::new(files[0].path.trim_end_matches('/')))) } else { tf!("Förkasta ändringarna i {n} filer?", n = files.len()) };
            ui.label(RichText::new(title).font(FontId::new(17.0, theme::semibold())).color(p.text));
            ui.add_space(8.0);
            let body = if untracked { t("Ändringarna går inte att ångra. Nya filer som inte är köade tas bort.") } else { t("Filen återställs till det köade eller incheckade innehållet. Det går inte att ångra.") };
            ui.label(RichText::new(body).color(p.muted));
            ui.add_space(20.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let go = egui::Button::new(RichText::new(t("Förkasta")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                    .fill(p.error)
                    .stroke(Stroke::NONE)
                    .min_size(vec2(96.0, 32.0));
                if ui.add(go).clicked() {
                    choice = Some(true);
                }
                if ui.add(quiet_button(t("Avbryt"), p).min_size(vec2(0.0, 32.0))).clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                self.git.discard = None;
                let steps = git::discard_steps(&files);
                self.git.start_op(ctx, PendingOp { ok: t("Förkastade ändringarna").to_owned(), fail: t("Kunde inte förkasta").to_owned(), steps, clear_message: false, then: None });
            }
            Some(false) => self.git.discard = None,
            None if close => self.git.discard = None,
            None => {}
        }
    }

    fn git_branch_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(bp) = self.git.branch_prompt.as_mut() else { return };
        let mut choice: Option<bool> = None;
        let close = Self::git_modal(ctx, p, "kladd-git-branch", |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(t("Ny gren")).font(FontId::new(17.0, theme::semibold())).color(p.text));
            ui.add_space(8.0);
            let field = ui.add(TextEdit::singleline(&mut bp.input).hint_text(t("Namn på grenen")).desired_width(f32::INFINITY).margin(Margin::symmetric(10, 7)));
            if bp.focus {
                field.request_focus();
                bp.focus = false;
            }
            if field.changed() {
                bp.error = None;
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                choice = Some(true);
            }
            if let Some(e) = bp.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(p.error));
            }
            ui.add_space(20.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let go = egui::Button::new(RichText::new(t("Skapa")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                    .fill(p.accent)
                    .stroke(Stroke::NONE)
                    .min_size(vec2(84.0, 32.0));
                if ui.add(go).clicked() {
                    choice = Some(true);
                }
                if ui.add(quiet_button(t("Avbryt"), p).min_size(vec2(0.0, 32.0))).clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => match git::valid_branch_name(&bp.input) {
                Ok(name) => {
                    let name = name.to_owned();
                    self.git.branch_prompt = None;
                    let job = PendingOp { ok: tf!("Skapade grenen {name}", name = name), fail: t("Kunde inte skapa grenen").to_owned(), steps: vec![git::create_branch_step(&name)], clear_message: false, then: None };
                    self.git.start_op(ctx, job);
                }
                Err(e) => bp.error = Some(e),
            },
            Some(false) => self.git.branch_prompt = None,
            None if close => self.git.branch_prompt = None,
            None => {}
        }
    }

    fn git_compare_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(c) = self.git.compare.as_mut() else { return };
        if let Some(rx) = &c.rx {
            match rx.try_recv() {
                Ok(r) => {
                    c.result = Some(r);
                    c.rx = None;
                }
                Err(TryRecvError::Disconnected) => {
                    c.result = Some(Err(t("Jämförelsen avbröts.").to_owned()));
                    c.rx = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut close_btn = false;
        let c = &*c;
        let close = Self::git_modal(ctx, p, "kladd-git-compare", |ui| {
            let w = (ctx.content_rect().width() - 120.0).clamp(420.0, 920.0);
            ui.set_width(w);
            ui.label(RichText::new(tf!("{name} jämfört med HEAD", name = c.title)).font(FontId::new(17.0, theme::semibold())).color(p.text));
            ui.add_space(10.0);
            let h = (ctx.content_rect().height() - 240.0).clamp(160.0, 560.0);
            ui.set_min_height(h);
            match &c.result {
                None => {
                    ui.label(RichText::new(t("Läser…")).color(p.muted));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(p.muted));
                }
                Some(Ok(rows)) => crate::history_ui::diff_view(
                    ui,
                    p,
                    rows,
                    h,
                    "git-compare",
                    t("Röd: finns i HEAD men inte nu. Grön: finns nu men inte i HEAD."),
                    t("Filen är identisk med HEAD."),
                ),
            }
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(quiet_button(t("Stäng"), p).min_size(vec2(0.0, 32.0))).clicked() {
                    close_btn = true;
                }
            });
        });
        if close || close_btn {
            self.git.compare = None;
        }
    }

    /// Rutan som visar de gamla raderna för en ändring, med "Återställ ändringen".
    fn git_hunk_popup(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(pop) = self.git.popup.clone() else { return };
        let hunk = self.git.diffs.get(&pop.doc_id).filter(|d| d.hash == pop.text_hash).and_then(|d| d.hunks.get(pop.idx).cloned());
        let Some(h) = hunk else {
            self.git.popup = None;
            return;
        };
        let mut revert = false;
        let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let width = 420.0f32.min(ctx.content_rect().width() - 32.0);
        let area = egui::Area::new(Id::unique("kladd-git-hunk-popup"))
            .order(Order::Foreground)
            .fixed_pos(pop.anchor)
            .constrain(true)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(10)
                    .inner_margin(Margin::same(10))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(width);
                        let title = match h.kind {
                            HunkKind::Added => t("Nya rader: finns inte i HEAD"),
                            HunkKind::Modified => t("Ändrade rader. HEAD hade:"),
                            HunkKind::Deleted => t("Borttagna rader. HEAD hade:"),
                        };
                        ui.label(RichText::new(title).font(FontId::new(12.5, theme::semibold())).color(p.muted));
                        if !h.old.is_empty() {
                            ui.add_space(4.0);
                            let bg = Color32::from_rgba_unmultiplied(210, 80, 80, if p.dark { 60 } else { 55 });
                            Frame::NONE.fill(bg).corner_radius(5).inner_margin(Margin::same(6)).show(ui, |ui| {
                                ScrollArea::both().id_salt("kladd-git-hunk-old").max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                                    let text = h.old.concat().replace('\t', "    ");
                                    let text = text.trim_end_matches('\n');
                                    ui.add(egui::Label::new(RichText::new(text).font(FontId::new(12.5, FontFamily::Monospace)).color(p.text)).wrap_mode(egui::TextWrapMode::Extend));
                                });
                            });
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let go = egui::Button::new(RichText::new(t("Återställ ändringen")).font(FontId::new(13.0, theme::medium())).color(p.on_accent))
                                .fill(p.accent)
                                .stroke(Stroke::NONE)
                                .min_size(vec2(0.0, 28.0));
                            if ui.add(go).clicked() {
                                revert = true;
                            }
                            if ui.add(quiet_button(t("Stäng"), p).min_size(vec2(0.0, 28.0))).clicked() {
                                close = true;
                            }
                        });
                    });
            });
        let outside = ctx.input(|i| i.pointer.any_pressed()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| !area.response.rect.contains(pos));
        if revert {
            if let Some(i) = self.docs.iter().position(|d| d.id == pop.doc_id) {
                let text = git::revert_hunk(&self.docs[i].text, &h);
                set_text_undoable(ctx, &mut self.docs[i], text);
                self.focus_editor = true;
            }
            self.git.popup = None;
        } else if close || outside {
            self.git.popup = None;
        }
    }
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::parse_status;

    fn snap(out: &str, prefix: &str) -> Snapshot {
        Snapshot { prefix: prefix.to_owned(), status: parse_status(out), ..Default::default() }
    }

    #[test]
    fn index_marks_files_and_parent_dirs() {
        let s = snap("# branch.head main\0\
                      1 .M N... 100644 100644 100644 a b src/deep/a.rs\0\
                      1 A. N... 000000 100644 100644 0 c docs/new.md\0\
                      ? ny-mapp/\0", "");
        let ix = Index::build(&s);
        assert_eq!(ix.file("src/deep/a.rs"), Some(Change::Modified));
        assert_eq!(ix.file("docs/new.md"), Some(Change::Added));
        assert_eq!(ix.file("src/other.rs"), None);
        assert!(ix.dirs.contains("src") && ix.dirs.contains("src/deep") && ix.dirs.contains("docs"));
        assert!(!ix.dirs.contains("tests"));
        // Filer i en ospårad mapp är ospårade, även om git bara listar mappen.
        assert_eq!(ix.file("ny-mapp/under/x.txt"), Some(Change::Untracked));
        assert!(ix.untracked_dirs.contains("ny-mapp"));
    }

    #[test]
    fn tree_mark_maps_workspace_paths_through_prefix() {
        let mut st = State { avail: Some(true), ..Default::default() };
        let s = snap("1 .M N... 100644 100644 100644 a b sub/x.txt\0", "sub/");
        st.index = Index::build(&s);
        st.snap = Some(Arc::new(s));
        st.snap_for = Some(PathBuf::from("/repo/sub"));
        let p = theme::palette(false);
        let root = Path::new("/repo/sub");
        let m = st.tree_mark(root, Path::new("/repo/sub/x.txt"), false, &p).unwrap();
        assert_eq!(m.0, Some('M'));
        assert!(st.tree_mark(root, Path::new("/repo/sub/y.txt"), false, &p).is_none());
        // Arbetsmappen själv ligger i `sub/`; en mapp under den utan ändringar markeras inte.
        assert!(st.tree_mark(root, Path::new("/repo/sub/mapp"), true, &p).is_none());
        // Fel arbetsmapp ger ingen markering.
        assert!(st.tree_mark(Path::new("/annan"), Path::new("/annan/x.txt"), false, &p).is_none());
        st.avail = Some(false);
        assert!(st.tree_mark(root, Path::new("/repo/sub/x.txt"), false, &p).is_none());
    }

    #[test]
    fn notice_text_is_one_line() {
        assert_eq!(one_line("fatal: något\n\n  hint: gör så\n"), "fatal: något · hint: gör så");
        assert!(one_line(&"x".repeat(1000)).chars().count() <= 301);
    }
}
