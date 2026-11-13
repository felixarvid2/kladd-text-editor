//! Arbetsmapp: filträd i sidopanelen, snabböppna (Ctrl+E) och filhantering (ny, byt namn, ta bort).
//! Logiken (listning, ignorerade mappar, suddig matchning, namnkontroll) är fristående och testad;
//! ritningen och kopplingen till fönstret ligger längst ned.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use egui::{
    Align, Align2, Color32, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Order, Panel,
    Painter, Rect, RichText, ScrollArea, Sense, Shape, Stroke, TextEdit, Ui, pos2, vec2,
};

use crate::app::{KladdApp, quiet_button};
use crate::theme::{self, Palette};

/// Mappar som döljs i trädet (och hoppas över vid sökning) om inte "Dolda filer" är på.
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules"];
/// Högst så många filer i snabböppna.
const MAX_QUICK_FILES: usize = 50_000;
/// Så ofta (sekunder) trädet läses om i bakgrunden av fönstret.
const REFRESH_SECS: f64 = 3.0;
const ROW_H: f32 = 24.0;
const INDENT: f32 = 14.0;

// ============================================================== logik

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Döljs posten som standard? Punktfiler och mapparna `.git`, `target` och `node_modules`.
pub fn is_hidden(name: &str, is_dir: bool) -> bool {
    name.starts_with('.') || (is_dir && SKIP_DIRS.contains(&name))
}

/// Innehållet i en mapp: mappar först, sedan filer, båda i bokstavsordning utan hänsyn till versaler.
pub fn list_dir(dir: &Path, show_hidden: bool) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Entry> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = path.is_dir();
            if !show_hidden && is_hidden(&name, is_dir) {
                return None;
            }
            Some(Entry { name, path, is_dir })
        })
        .collect();
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Går igenom alla filer under `root` utom dolda, ignorerade (`.gitignore`) och `target`/`node_modules`.
/// Återkallet returnerar `false` för att avbryta. Avbryts också av `cancel`.
pub fn walk_files(root: &Path, cancel: &AtomicBool, mut f: impl FnMut(PathBuf) -> bool) {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        // `.gitignore` gäller även i mappar som inte är git-arkiv.
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            !(is_dir && e.depth() > 0 && SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        })
        .build();
    for entry in walker.filter_map(|e| e.ok()) {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if entry.file_type().is_some_and(|t| t.is_file()) && !f(entry.into_path()) {
            return;
        }
    }
}

/// Suddig matchning av `query` mot `cand` (sökvägen med `/`). `None` om inte alla tecken finns i ordning.
/// Högre poäng = bättre: sammanhängande tecken, ordgränser (efter `/ _ - .` eller versal) och träffar i
/// filnamnet belönas, långa sökvägar straffas lätt.
pub fn fuzzy_score(query: &str, cand: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let orig: Vec<char> = cand.chars().collect();
    let c: Vec<char> = cand.chars().flat_map(|c| c.to_lowercase()).collect();
    // Gemener som ändrar antal tecken (sällsynt) bryter indexeringen mot originalet: använd då gemenerna.
    let orig = if orig.len() == c.len() { orig } else { c.clone() };
    let n = c.len();
    if q.len() > n || n > 1024 {
        return None;
    }
    let name_start = c.iter().rposition(|&ch| ch == '/' || ch == '\\').map_or(0, |i| i + 1);
    let boundary = |i: usize| -> bool {
        if i == 0 {
            return true;
        }
        let prev = orig[i - 1];
        matches!(prev, '/' | '\\' | '_' | '-' | '.' | ' ') || (prev.is_lowercase() && orig[i].is_uppercase())
    };
    const MATCH: i32 = 16;
    const CONSECUTIVE: i32 = 24;
    const BOUNDARY: i32 = 18;
    const IN_NAME: i32 = 6;
    let mut prev: Vec<Option<i32>> = vec![None; n];
    for j in 0..q.len() {
        let mut cur: Vec<Option<i32>> = vec![None; n];
        // Bästa `poäng + k` över tidigare positioner k som inte är direkt före (för avståndsstraffet).
        let mut run: Option<i32> = None;
        for i in 0..n {
            // k = i - 2 blir tillgänglig för icke-sammanhängande övergångar först nu.
            if j > 0 && i >= 2 {
                if let Some(s) = prev[i - 2] {
                    let v = s + (i as i32 - 2);
                    run = Some(run.map_or(v, |r| r.max(v)));
                }
            }
            if c[i] != q[j] {
                continue;
            }
            let mut gain = MATCH;
            if boundary(i) {
                gain += BOUNDARY;
            }
            if i >= name_start {
                gain += IN_NAME;
            }
            let base = if j == 0 {
                // Tecken före första träffen kostar lite, men mindre än mellan träffar.
                Some(-(i as i32) / 4)
            } else {
                let consecutive = if i >= 1 { prev[i - 1].map(|s| s + CONSECUTIVE) } else { None };
                let gap = run.map(|r| r - (i as i32 - 1));
                match (consecutive, gap) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                }
            };
            cur[i] = base.map(|b| b + gain);
        }
        prev = cur;
    }
    let best = prev.iter().flatten().copied().max()?;
    let name: String = c[name_start..].iter().collect();
    let qs: String = q.iter().collect();
    let mut bonus = 0;
    if name == qs {
        bonus += 120;
    } else if name.starts_with(&qs) {
        bonus += 60;
    } else if name.contains(&qs) {
        bonus += 30;
    }
    Some(best + bonus - (n as i32) / 8)
}

/// De bästa träffarna för `query` bland `files` (sökvägar med `/`), bäst först. Tom fråga ger
/// `files` i given ordning.
pub fn rank(query: &str, files: &[String], limit: usize) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..files.len().min(limit)).collect();
    }
    let mut scored: Vec<(i32, usize)> =
        files.iter().enumerate().filter_map(|(i, f)| fuzzy_score(query, f).map(|s| (s, i))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| files[a.1].len().cmp(&files[b.1].len())).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, i)| i).collect()
}

/// Kontrollerar ett nytt fil- eller mappnamn.
pub fn valid_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Skriv ett namn.".into());
    }
    if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err("Namnet får inte innehålla snedstreck.".into());
    }
    if cfg!(windows) && name.contains([':', '*', '?', '"', '<', '>', '|']) {
        return Err("Namnet innehåller tecken som inte tillåts.".into());
    }
    Ok(name)
}

/// Skapar en tom fil i `dir`.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = valid_name(name)?;
    let path = dir.join(name);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| io_message(&e, name))?;
    Ok(path)
}

/// Skapar en mapp i `dir`.
pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = valid_name(name)?;
    let path = dir.join(name);
    std::fs::create_dir(&path).map_err(|e| io_message(&e, name))?;
    Ok(path)
}

/// Byter namn på en fil eller mapp (stannar i samma mapp). Skriver aldrig över något.
pub fn rename_path(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let name = valid_name(new_name)?;
    let parent = path.parent().ok_or("Sökvägen saknar överordnad mapp.")?;
    let target = parent.join(name);
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        return Err(format!("{name} finns redan."));
    }
    std::fs::rename(path, &target).map_err(|e| io_message(&e, name))?;
    Ok(target)
}

fn io_message(e: &std::io::Error, name: &str) -> String {
    match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{name} finns redan."),
        std::io::ErrorKind::PermissionDenied => format!("Åtkomst nekad för {name}."),
        _ => format!("Det gick inte: {e}"),
    }
}

/// Ersätter `old` med `new` som början på `path` (för flikar som ligger i en omdöpt mapp).
pub fn moved(path: &Path, old: &Path, new: &Path) -> Option<PathBuf> {
    path.strip_prefix(old).ok().map(|rest| if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) })
}

// ============================================================== tillstånd

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameKind {
    NewFile,
    NewDir,
    Rename,
}

enum Prompt {
    Name { kind: NameKind, base: PathBuf, input: String, focus: bool, error: Option<String> },
    Delete(PathBuf),
}

struct Quick {
    query: String,
    selected: usize,
    /// Relativa sökvägar (med `/`) när en mapp är öppen, annars fullständiga sökvägar ur "senaste".
    files: Vec<String>,
    root: Option<PathBuf>,
    rx: Option<Receiver<Vec<String>>>,
    cancel: std::sync::Arc<AtomicBool>,
    focus: bool,
}

/// Avbryter filsökningen i bakgrunden när popupen stängs.
impl Drop for Quick {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct State {
    expanded: HashSet<PathBuf>,
    children: HashMap<PathBuf, Vec<Entry>>,
    /// Mappen trädet är byggt för.
    loaded_root: Option<PathBuf>,
    loaded_hidden: bool,
    last_refresh: f64,
    was_focused: bool,
    prompt: Option<Prompt>,
    quick: Option<Quick>,
    folder_dialog: Option<Receiver<Option<PathBuf>>>,
}

/// En åtgärd som trädet ber om (utförs när ritningen är klar).
enum Action {
    Toggle(PathBuf),
    Open(PathBuf),
    New(NameKind, PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
}

impl State {
    fn reload_all(&mut self, show_hidden: bool) {
        let dirs: Vec<PathBuf> = self.expanded.iter().cloned().collect();
        for d in dirs {
            if d.is_dir() {
                let list = list_dir(&d, show_hidden);
                self.children.insert(d, list);
            } else {
                self.expanded.remove(&d);
                self.children.remove(&d);
            }
        }
    }

    fn expand(&mut self, dir: &Path, show_hidden: bool) {
        self.expanded.insert(dir.to_path_buf());
        self.children.insert(dir.to_path_buf(), list_dir(dir, show_hidden));
    }
}

fn flatten<'a>(
    children: &'a HashMap<PathBuf, Vec<Entry>>,
    expanded: &HashSet<PathBuf>,
    dir: &Path,
    depth: usize,
    out: &mut Vec<(usize, &'a Entry)>,
) {
    let Some(list) = children.get(dir) else { return };
    for e in list {
        out.push((depth, e));
        if e.is_dir && expanded.contains(&e.path) {
            flatten(children, expanded, &e.path, depth + 1, out);
        }
    }
}

/// En liten textknapp som kan vara intryckt (för val som "Dolda" och sökomfång).
pub(crate) fn chip(ui: &mut Ui, p: &Palette, label: &str, on: bool, tooltip: &str) -> egui::Response {
    let color = if on { p.accent } else { p.muted };
    let btn = egui::Button::new(RichText::new(label).size(12.5).color(color))
        .fill(Color32::TRANSPARENT)
        .stroke(if on { Stroke::new(1.0, p.accent) } else { Stroke::NONE })
        .min_size(vec2(0.0, 24.0));
    ui.add(btn).on_hover_text(tooltip)
}

// ============================================================== ikoner

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Folder,
    Code,
    Markdown,
    Text,
    Data,
    Image,
    Document,
    Other,
}

fn kind_of(e: &Entry) -> Kind {
    if e.is_dir {
        return Kind::Folder;
    }
    let ext = e.path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" | "py" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "cc" | "java" | "kt" | "go"
        | "rb" | "php" | "lua" | "sql" | "sh" | "bash" | "zsh" | "html" | "htm" | "css" | "scss" => Kind::Code,
        "md" | "markdown" => Kind::Markdown,
        "txt" | "log" | "rst" | "org" | "tex" | "srt" => Kind::Text,
        "json" | "toml" | "yaml" | "yml" | "xml" | "ini" | "conf" | "cfg" | "env" | "csv" | "tsv" | "lock" => Kind::Data,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" => Kind::Image,
        "docx" | "odt" | "pdf" | "dotx" | "docm" | "ott" => Kind::Document,
        _ => Kind::Other,
    }
}

fn kind_color(k: Kind, p: &Palette) -> Color32 {
    match k {
        Kind::Folder => Color32::from_rgb(0xD9, 0xA4, 0x41),
        Kind::Code => Color32::from_rgb(0x5B, 0x9B, 0xD5),
        Kind::Markdown => Color32::from_rgb(0x6F, 0xB7, 0x7A),
        Kind::Data => Color32::from_rgb(0xC5, 0x86, 0xC0),
        Kind::Image => Color32::from_rgb(0xE0, 0x7A, 0x5F),
        Kind::Document => Color32::from_rgb(0xD9, 0x5F, 0x5F),
        Kind::Text | Kind::Other => p.muted,
    }
}

/// Ritar en liten ikon (mapp eller fil med färgad kant efter typ) i `r`.
fn paint_icon(painter: &Painter, r: Rect, kind: Kind, open: bool, p: &Palette) {
    let col = kind_color(kind, p);
    let c = r.center();
    if kind == Kind::Folder {
        let body = Rect::from_center_size(c + vec2(0.0, 1.0), vec2(14.0, 10.0));
        let tab = Rect::from_min_size(body.left_top() - vec2(0.0, 2.0), vec2(6.0, 3.0));
        painter.rect_filled(tab, 1.0, col);
        if open {
            painter.rect_stroke(body, 1.5, Stroke::new(1.4, col), egui::StrokeKind::Inside);
        } else {
            painter.rect_filled(body, 1.5, col);
        }
        return;
    }
    let page = Rect::from_center_size(c, vec2(11.0, 14.0));
    let fold = 4.0;
    let pts = vec![
        page.left_top(),
        pos2(page.right() - fold, page.top()),
        pos2(page.right(), page.top() + fold),
        page.right_bottom(),
        page.left_bottom(),
    ];
    painter.add(Shape::convex_polygon(pts, Color32::TRANSPARENT, Stroke::new(1.3, col)));
    painter.line_segment(
        [pos2(page.right() - fold, page.top()), pos2(page.right() - fold, page.top() + fold)],
        Stroke::new(1.0, col),
    );
    painter.line_segment([pos2(page.right() - fold, page.top() + fold), pos2(page.right(), page.top() + fold)], Stroke::new(1.0, col));
    // Typmarkering: en kort rad(er) i filens färg.
    let y = page.top() + 7.0;
    let (x0, x1) = (page.left() + 2.5, page.right() - 2.5);
    match kind {
        Kind::Code => {
            painter.line_segment([pos2(x0, y), pos2(x0 + 2.0, y + 1.5)], Stroke::new(1.2, col));
            painter.line_segment([pos2(x0 + 2.0, y + 1.5), pos2(x0, y + 3.0)], Stroke::new(1.2, col));
            painter.line_segment([pos2(x1 - 1.5, y + 3.0), pos2(x1, y + 3.0)], Stroke::new(1.2, col));
        }
        Kind::Image => {
            painter.circle_filled(pos2(c.x, y + 2.0), 2.0, col);
        }
        _ => {
            painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.2, col));
            painter.line_segment([pos2(x0, y + 3.0), pos2(x1, y + 3.0)], Stroke::new(1.2, col));
        }
    }
}

// ============================================================== fönstret

impl KladdApp {
    /// Hela panelens och dialogernas ritning och hantering av tangentbordet hör hit.
    /// Kortkommandon: Ctrl+Skift+E (panelen), Ctrl+E (snabböppna). I dokumentläget är båda
    /// upptagna (spåra ändringar respektive centrera, som i Word); där nås de via menyn.
    /// Anropas före Ctrl+F i `handle_shortcuts` eftersom Skift-varianterna måste tas först.
    pub(crate) fn workspace_shortcuts(&mut self, ctx: &egui::Context) {
        let ctrl = Modifiers::COMMAND;
        let hit = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(m, k)));
        if !self.word_mode() && hit(ctrl | Modifiers::SHIFT, Key::E) {
            self.toggle_workspace_panel(ctx);
        } else if !self.word_mode() && hit(ctrl, Key::E) {
            // I dokumentläget är Ctrl+E "centrera"; där finns snabböppna bara i menyn.
            self.open_quick(ctx);
        }
        if hit(ctrl | Modifiers::SHIFT, Key::F) {
            self.open_find_files(ctx);
        }
    }

    pub(crate) fn toggle_workspace_panel(&mut self, ctx: &egui::Context) {
        if self.settings.workspace.is_none() {
            self.request_folder(ctx);
        } else {
            self.settings.workspace_panel = !self.settings.workspace_panel;
        }
    }

    /// Öppnar mappväljaren (i en tråd); resultatet tas emot i `workspace_poll`.
    pub(crate) fn request_folder(&mut self, ctx: &egui::Context) {
        if self.ws.folder_dialog.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.ws.folder_dialog = Some(rx);
        let ctx = ctx.clone();
        let start = self
            .settings
            .workspace
            .clone()
            .or_else(|| self.docs.get(self.active).and_then(|d| d.path.as_ref()).and_then(|p| p.parent().map(Path::to_path_buf)))
            .or_else(crate::platform::home_dir);
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new().set_title("Öppna mapp");
            if let Some(d) = start {
                dlg = dlg.set_directory(d);
            }
            let _ = tx.send(dlg.pick_folder());
            ctx.request_repaint();
        });
    }

    /// Gör `dir` till arbetsmapp och visar panelen.
    pub(crate) fn set_workspace(&mut self, ctx: &egui::Context, dir: &Path) {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !canonical.is_dir() {
            self.notify(ctx, "Mappen finns inte.", true);
            return;
        }
        self.settings.workspace = Some(canonical);
        self.settings.workspace_panel = true;
        self.ws.loaded_root = None;
        self.cancel_find_files();
    }

    pub(crate) fn close_workspace(&mut self) {
        self.settings.workspace = None;
        self.settings.workspace_panel = false;
        self.ws.loaded_root = None;
        self.ws.expanded.clear();
        self.ws.children.clear();
        self.ws.quick = None;
    }

    /// Tar emot mappväljarens svar och bygger om trädet när mappen eller "dolda filer" ändrats.
    fn workspace_poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.ws.folder_dialog {
            match rx.try_recv() {
                Ok(picked) => {
                    self.ws.folder_dialog = None;
                    if let Some(dir) = picked {
                        self.set_workspace(ctx, &dir);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ws.folder_dialog = None;
                    self.notify(ctx, "Fildialogen kunde inte öppnas. Installera paketet zenity och försök igen.", true);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(root) = self.settings.workspace.clone() else { return };
        let hidden = self.settings.workspace_hidden;
        if self.ws.loaded_root.as_ref() != Some(&root) {
            if !root.is_dir() {
                self.close_workspace();
                self.notify(ctx, "Arbetsmappen finns inte längre.", true);
                return;
            }
            self.ws.expanded.clear();
            self.ws.children.clear();
            self.ws.expand(&root, hidden);
            self.ws.loaded_root = Some(root);
            self.ws.loaded_hidden = hidden;
            self.ws.last_refresh = Self::now(ctx);
        } else if self.ws.loaded_hidden != hidden {
            self.ws.loaded_hidden = hidden;
            self.ws.reload_all(hidden);
        }
    }

    /// Läser om öppna mappar när fönstret får fokus och annars var tredje sekund (bara de utfällda mapparna).
    fn workspace_refresh(&mut self, ctx: &egui::Context) {
        let now = Self::now(ctx);
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let regained = focused && !self.ws.was_focused;
        self.ws.was_focused = focused;
        if focused && (regained || now - self.ws.last_refresh >= REFRESH_SECS) {
            self.ws.last_refresh = now;
            self.ws.reload_all(self.settings.workspace_hidden);
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(REFRESH_SECS));
    }

    /// Filträdet i en sidopanel till vänster.
    pub(crate) fn workspace_panel(&mut self, ui: &mut Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        self.workspace_poll(&ctx);
        if !self.settings.workspace_panel || self.settings.workspace.is_none() {
            return;
        }
        self.workspace_refresh(&ctx);
        let Some(root) = self.settings.workspace.clone() else { return };
        let width = ui.available_width();
        let active_path = self.docs.get(self.active).and_then(|d| d.path.clone());
        let mut actions: Vec<Action> = Vec::new();
        let mut close = false;
        let mut pick = false;
        let mut hidden = self.settings.workspace_hidden;

        Panel::left("kladd-workspace")
            .resizable(true)
            .default_size(250.0)
            .size_range(160.0..=(width * 0.5).max(200.0))
            .frame(Frame::NONE.fill(p.editor).inner_margin(Margin { left: 8, right: 6, top: 10, bottom: 0 }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(name.to_uppercase()).font(FontId::new(11.5, theme::semibold())).color(p.muted))
                        .on_hover_text(root.display().to_string());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if crate::icons::button(ui, p, crate::icons::Icon::Close, 24.0, false, "Dölj mappträdet (Ctrl+Shift+E)", p.editor).clicked() {
                            close = true;
                        }
                        if chip(ui, p, "Dolda", hidden, "Visa dolda filer och mappar som .git, target och node_modules").clicked() {
                            hidden = !hidden;
                        }
                        if ui.add(quiet_button("Byt…", p).min_size(vec2(0.0, 24.0))).on_hover_text("Öppna en annan mapp").clicked() {
                            pick = true;
                        }
                    });
                });
                ui.add_space(6.0);
                let mut rows: Vec<(usize, &Entry)> = Vec::new();
                flatten(&self.ws.children, &self.ws.expanded, &root, 0, &mut rows);
                // Bakgrunden läggs först så att raderna hamnar ovanpå och tar sina egna klick.
                let rest = ui.interact(ui.available_rect_before_wrap(), Id::unique("kladd-workspace-bg"), Sense::click());
                let area = ScrollArea::vertical().id_salt("kladd-workspace-scroll").auto_shrink([false, false]);
                let out = area.show_rows(ui, ROW_H, rows.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for &(depth, e) in &rows[range] {
                        tree_row(ui, p, depth, e, self.ws.expanded.contains(&e.path), active_path.as_deref(), &mut actions);
                    }
                });
                if rows.is_empty() {
                    ui.painter().text(
                        out.inner_rect.left_top() + vec2(8.0, 8.0),
                        Align2::LEFT_TOP,
                        "Mappen är tom",
                        FontId::new(12.5, FontFamily::Proportional),
                        p.muted,
                    );
                }
                // Högerklick på tom yta: ny fil/mapp i rotmappen.
                rest.context_menu(|ui| {
                    if ui.button("Ny fil").clicked() {
                        actions.push(Action::New(NameKind::NewFile, root.clone()));
                        ui.close();
                    }
                    if ui.button("Ny mapp").clicked() {
                        actions.push(Action::New(NameKind::NewDir, root.clone()));
                        ui.close();
                    }
                    if ui.button("Visa i filhanteraren").clicked() {
                        actions.push(Action::Reveal(root.clone()));
                        ui.close();
                    }
                });
            });

        self.settings.workspace_hidden = hidden;
        if pick {
            self.request_folder(&ctx);
        }
        if close {
            self.settings.workspace_panel = false;
        }
        for a in actions {
            self.workspace_action(&ctx, a);
        }
    }

    fn workspace_action(&mut self, ctx: &egui::Context, a: Action) {
        let hidden = self.settings.workspace_hidden;
        match a {
            Action::Toggle(dir) => {
                if self.ws.expanded.contains(&dir) {
                    self.ws.expanded.remove(&dir);
                } else {
                    self.ws.expand(&dir, hidden);
                }
            }
            Action::Open(path) => self.open_path(ctx, &path),
            Action::New(kind, dir) => {
                self.ws.prompt = Some(Prompt::Name { kind, base: dir, input: String::new(), focus: true, error: None });
            }
            Action::Rename(path) => {
                let input = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.ws.prompt = Some(Prompt::Name { kind: NameKind::Rename, base: path, input, focus: true, error: None });
            }
            Action::Delete(path) => self.ws.prompt = Some(Prompt::Delete(path)),
            Action::Reveal(dir) => {
                if let Err(e) = crate::platform::open(&dir) {
                    self.notify(ctx, format!("Kunde inte öppna filhanteraren: {e}"), true);
                }
            }
        }
    }

    /// Dialogerna för nytt namn och borttagning.
    pub(crate) fn workspace_dialogs(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(prompt) = self.ws.prompt.as_mut() else { return };
        // 0 = bekräfta, 1 = avbryt
        let mut choice: Option<u8> = None;
        let modal = egui::Modal::new(Id::unique("kladd-workspace-dialog"))
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
                ui.set_width(380.0);
                let (title, action) = match prompt {
                    Prompt::Name { kind: NameKind::NewFile, .. } => ("Ny fil".to_owned(), "Skapa"),
                    Prompt::Name { kind: NameKind::NewDir, .. } => ("Ny mapp".to_owned(), "Skapa"),
                    Prompt::Name { kind: NameKind::Rename, .. } => ("Byt namn".to_owned(), "Byt namn"),
                    Prompt::Delete(path) => {
                        let n = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        (format!("Ta bort {n}?"), "Ta bort")
                    }
                };
                ui.label(RichText::new(title).font(FontId::new(17.0, theme::semibold())).color(p.text));
                ui.add_space(8.0);
                match prompt {
                    Prompt::Name { input, focus, error, base, kind } => {
                        if *kind != NameKind::Rename {
                            ui.label(RichText::new(format!("I {}", base.display())).size(12.5).color(p.muted));
                            ui.add_space(6.0);
                        }
                        let field = ui.add(
                            TextEdit::singleline(input)
                                .hint_text("Namn")
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(10, 7)),
                        );
                        if *focus {
                            field.request_focus();
                            *focus = false;
                        }
                        if field.changed() {
                            *error = None;
                        }
                        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            choice = Some(0);
                        }
                        if let Some(e) = error {
                            ui.add_space(6.0);
                            ui.label(RichText::new(e.as_str()).color(p.error));
                        }
                    }
                    Prompt::Delete(path) => {
                        let what = if path.is_dir() { "Mappen och allt i den" } else { "Filen" };
                        ui.label(RichText::new(format!("{what} flyttas till papperskorgen.")).color(p.muted));
                    }
                }
                ui.add_space(20.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let fill = if matches!(prompt, Prompt::Delete(_)) { p.error } else { p.accent };
                    let go = egui::Button::new(RichText::new(action).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                        .fill(fill)
                        .stroke(Stroke::NONE)
                        .min_size(vec2(84.0, 32.0));
                    if ui.add(go).clicked() {
                        choice = Some(0);
                    }
                    if ui.add(quiet_button("Avbryt", p).min_size(vec2(0.0, 32.0))).clicked() {
                        choice = Some(1);
                    }
                });
            });
        if modal.should_close() && choice.is_none() {
            choice = Some(1);
        }
        match choice {
            Some(0) => self.workspace_confirm(ctx),
            Some(_) => self.ws.prompt = None,
            None => {}
        }
    }

    fn workspace_confirm(&mut self, ctx: &egui::Context) {
        let hidden = self.settings.workspace_hidden;
        let Some(prompt) = self.ws.prompt.take() else { return };
        match prompt {
            Prompt::Name { kind, base, input, error: _, focus: _ } => {
                let result = match kind {
                    NameKind::NewFile => create_file(&base, &input),
                    NameKind::NewDir => create_dir(&base, &input),
                    NameKind::Rename => rename_path(&base, &input),
                };
                match result {
                    Ok(path) => {
                        let parent = if kind == NameKind::Rename { base.parent().map(Path::to_path_buf) } else { Some(base.clone()) };
                        if let Some(parent) = &parent {
                            self.ws.expand(parent, hidden);
                        }
                        match kind {
                            NameKind::NewFile => self.open_path(ctx, &path),
                            NameKind::NewDir => self.ws.expand(&path, hidden),
                            NameKind::Rename => {
                                self.workspace_renamed(&base, &path);
                                self.ws.reload_all(hidden);
                            }
                        }
                    }
                    Err(e) => {
                        // Behåll dialogen med felet.
                        self.ws.prompt = Some(Prompt::Name { kind, base, input, focus: true, error: Some(e) });
                    }
                }
            }
            Prompt::Delete(path) => match trash::delete(&path) {
                Ok(()) => {
                    self.ws.expanded.retain(|d| !d.starts_with(&path));
                    self.ws.reload_all(hidden);
                    let open = self.docs.iter().any(|d| d.path.as_ref().is_some_and(|p| p.starts_with(&path)));
                    let msg = if open { "Flyttade till papperskorgen. En öppen flik pekar på den borttagna filen." } else { "Flyttade till papperskorgen" };
                    self.notify(ctx, msg, false);
                }
                Err(e) => self.notify(ctx, format!("Kunde inte flytta till papperskorgen: {e}"), true),
            },
        }
    }

    /// Flikar vars fil ligger under det omdöpta får den nya sökvägen.
    fn workspace_renamed(&mut self, old: &Path, new: &Path) {
        for d in &mut self.docs {
            if let Some(np) = d.path.as_deref().and_then(|p| moved(p, old, new)) {
                d.path = Some(np);
            }
        }
        let was: Vec<PathBuf> = self.ws.expanded.iter().filter(|d| d.starts_with(old)).cloned().collect();
        for d in was {
            self.ws.expanded.remove(&d);
            if let Some(nd) = moved(&d, old, new) {
                self.ws.expanded.insert(nd);
            }
        }
        for r in &mut self.settings.recent {
            if let Some(np) = moved(r, old, new) {
                *r = np;
            }
        }
    }

    // ---------------------------------------------------------------- snabböppna

    pub(crate) fn open_quick(&mut self, ctx: &egui::Context) {
        if self.ws.quick.is_some() {
            self.ws.quick = None;
            return;
        }
        let root = self.settings.workspace.clone().filter(|r| r.is_dir());
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let mut quick = Quick { query: String::new(), selected: 0, files: Vec::new(), root: root.clone(), rx: None, cancel: cancel.clone(), focus: true };
        match root {
            Some(root) => {
                let (tx, rx) = channel();
                quick.rx = Some(rx);
                let ctx = ctx.clone();
                let recent: Vec<PathBuf> = self.docs.iter().filter_map(|d| d.path.clone()).chain(self.settings.recent.iter().cloned()).collect();
                std::thread::spawn(move || {
                    let mut files = Vec::new();
                    walk_files(&root, &cancel, |p| {
                        if let Ok(rel) = p.strip_prefix(&root) {
                            files.push(rel.to_string_lossy().replace('\\', "/"));
                        }
                        files.len() < MAX_QUICK_FILES
                    });
                    // Senast använda filer först, resten i bokstavsordning.
                    let rank_of = |f: &String| recent.iter().position(|r| r == &root.join(f)).unwrap_or(usize::MAX);
                    files.sort_by_key(|f| rank_of(f));
                    let _ = tx.send(files);
                    ctx.request_repaint();
                });
            }
            None => {
                quick.files = self.settings.recent.iter().filter(|p| p.is_file()).map(|p| p.to_string_lossy().replace('\\', "/")).collect();
            }
        }
        self.ws.quick = Some(quick);
    }

    /// Popupen för snabböppna: sökfält och lista, pilar + Enter.
    pub(crate) fn quick_open_popup(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(q) = self.ws.quick.as_mut() else { return };
        if let Some(rx) = &q.rx {
            if let Ok(files) = rx.try_recv() {
                q.files = files;
                q.rx = None;
            }
        }
        let ranked = rank(&q.query, &q.files, 12);
        if q.selected >= ranked.len() {
            q.selected = ranked.len().saturating_sub(1);
        }
        let (down, up, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if down && !ranked.is_empty() {
            q.selected = (q.selected + 1) % ranked.len();
        }
        if up && !ranked.is_empty() {
            q.selected = (q.selected + ranked.len() - 1) % ranked.len();
        }
        let mut chosen: Option<usize> = if enter { ranked.get(q.selected).copied() } else { None };
        let mut close = esc;
        let screen = ctx.content_rect();
        let width = 560.0f32.min(screen.width() - 32.0);
        let pos = pos2(screen.center().x - width / 2.0, screen.top() + 70.0);
        let scanning = q.rx.is_some();
        let has_root = q.root.is_some();
        let area = egui::Area::new(Id::unique("kladd-quick-open"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(11)
                    .inner_margin(Margin::same(8))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(width - 16.0);
                        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
                        let hint = if has_root { "Sök fil i mappen" } else { "Sök bland senaste filer" };
                        let field = ui.add(
                            TextEdit::singleline(&mut q.query)
                                .hint_text(hint)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(10, 7)),
                        );
                        if q.focus {
                            field.request_focus();
                            q.focus = false;
                        }
                        if field.changed() {
                            q.selected = 0;
                        }
                        ui.add_space(4.0);
                        if ranked.is_empty() {
                            let msg = if scanning { "Läser mappen…" } else if q.files.is_empty() && !has_root { "Inga senaste filer" } else { "Inga träffar" };
                            ui.label(RichText::new(msg).size(12.5).color(p.muted));
                        }
                        for (row, &i) in ranked.iter().enumerate() {
                            let full = &q.files[i];
                            let (dir, name) = match full.rfind('/') {
                                Some(k) => (&full[..k], &full[k + 1..]),
                                None => ("", full.as_str()),
                            };
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                            if row == q.selected {
                                ui.painter().rect_filled(rect, 6.0, p.selection);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, 6.0, p.surface_hover);
                            }
                            let font = FontId::new(13.5, FontFamily::Proportional);
                            let galley = ui.painter().layout_no_wrap(name.to_owned(), font.clone(), p.text);
                            let nw = galley.size().x;
                            let clip = ui.painter().with_clip_rect(rect.shrink2(vec2(8.0, 0.0)));
                            clip.galley(pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, p.text);
                            clip.text(
                                pos2(rect.left() + 8.0 + nw + 10.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                dir,
                                FontId::new(12.0, FontFamily::Proportional),
                                p.muted,
                            );
                            if resp.clicked() {
                                chosen = Some(i);
                            }
                            if row == q.selected && (down || up) {
                                ui.scroll_to_rect(rect, None);
                            }
                        }
                    });
            });
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| !area.response.rect.contains(pos));
        if clicked_outside {
            close = true;
        }
        if let Some(i) = chosen {
            let q = self.ws.quick.take().expect("popupen är öppen");
            let path = match &q.root {
                Some(root) => root.join(&q.files[i]),
                None => PathBuf::from(&q.files[i]),
            };
            self.open_path(ctx, &path);
        } else if close {
            self.ws.quick = None;
            self.focus_editor = true;
        }
    }
}

fn tree_row(ui: &mut Ui, p: &Palette, depth: usize, e: &Entry, open: bool, active: Option<&Path>, actions: &mut Vec<Action>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let is_active = !e.is_dir && active == Some(e.path.as_path());
    if is_active {
        ui.painter().rect_filled(rect, 5.0, p.selection);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, p.surface_hover);
    }
    let x0 = rect.left() + 4.0 + depth as f32 * INDENT;
    let painter = ui.painter().with_clip_rect(rect);
    if e.is_dir {
        // Liten pil framför mappen.
        let c = pos2(x0 + 5.0, rect.center().y);
        let pts = if open {
            vec![c + vec2(-3.5, -2.0), c + vec2(3.5, -2.0), c + vec2(0.0, 2.5)]
        } else {
            vec![c + vec2(-2.0, -3.5), c + vec2(-2.0, 3.5), c + vec2(2.5, 0.0)]
        };
        painter.add(Shape::convex_polygon(pts, p.muted, Stroke::NONE));
    }
    let icon = Rect::from_center_size(pos2(x0 + 22.0, rect.center().y), vec2(16.0, 16.0));
    paint_icon(&painter, icon, kind_of(e), open, p);
    painter.text(
        pos2(x0 + 36.0, rect.center().y),
        Align2::LEFT_CENTER,
        &e.name,
        FontId::new(13.0, FontFamily::Proportional),
        if is_active { p.text } else if e.is_dir { p.text } else { p.muted.gamma_multiply(1.0) },
    );
    if resp.clicked() {
        actions.push(if e.is_dir { Action::Toggle(e.path.clone()) } else { Action::Open(e.path.clone()) });
    }
    resp.context_menu(|ui| {
        let dir = if e.is_dir { e.path.clone() } else { e.path.parent().map(Path::to_path_buf).unwrap_or_default() };
        if ui.button("Ny fil").clicked() {
            actions.push(Action::New(NameKind::NewFile, dir.clone()));
            ui.close();
        }
        if ui.button("Ny mapp").clicked() {
            actions.push(Action::New(NameKind::NewDir, dir.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Byt namn").clicked() {
            actions.push(Action::Rename(e.path.clone()));
            ui.close();
        }
        if ui.button("Ta bort").clicked() {
            actions.push(Action::Delete(e.path.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Visa i filhanteraren").clicked() {
            actions.push(Action::Reveal(dir));
            ui.close();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kladd-ws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fuzzy_requires_all_chars_in_order() {
        assert!(fuzzy_score("abc", "a_b_c").is_some());
        assert!(fuzzy_score("acb", "abc").is_none());
        assert!(fuzzy_score("xyz", "src/main.rs").is_none());
        assert_eq!(fuzzy_score("", "whatever"), Some(0));
    }

    #[test]
    fn fuzzy_prefers_filename_and_boundaries() {
        let files: Vec<String> = ["src/app.rs", "src/apple_pie/readme.md", "docs/dev/application.md", "src/main.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let r = rank("app.rs", &files, 10);
        assert_eq!(files[r[0]], "src/app.rs");
        let r = rank("main", &files, 10);
        assert_eq!(files[r[0]], "src/main.rs");
        // Versaler spelar ingen roll.
        assert!(fuzzy_score("MAIN", "src/main.rs").is_some());
    }

    #[test]
    fn fuzzy_consecutive_beats_scattered() {
        let a = fuzzy_score("find", "src/find_files.rs").unwrap();
        let b = fuzzy_score("find", "src/fonts/image_nd.rs").unwrap();
        assert!(a > b, "{a} > {b}");
        // Camel case och ordgränser räknas som gränser.
        let c = fuzzy_score("ff", "findFiles.rs").unwrap();
        let d = fuzzy_score("ff", "xxfxxfxx.rs").unwrap();
        assert!(c > d);
    }

    #[test]
    fn rank_empty_query_keeps_order_and_limits() {
        let files: Vec<String> = (0..20).map(|i| format!("f{i}")).collect();
        assert_eq!(rank("", &files, 5), vec![0, 1, 2, 3, 4]);
        assert!(rank("zzz", &files, 5).is_empty());
    }

    #[test]
    fn list_dir_sorts_dirs_first_and_hides() {
        let d = tmp("list");
        std::fs::create_dir(d.join("zeta")).unwrap();
        std::fs::create_dir(d.join("Alfa")).unwrap();
        std::fs::create_dir(d.join(".git")).unwrap();
        std::fs::create_dir(d.join("target")).unwrap();
        std::fs::create_dir(d.join("node_modules")).unwrap();
        std::fs::write(d.join("b.txt"), "").unwrap();
        std::fs::write(d.join("A.txt"), "").unwrap();
        std::fs::write(d.join(".env"), "").unwrap();
        let names = |show| list_dir(&d, show).into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names(false), ["Alfa", "zeta", "A.txt", "b.txt"]);
        let all = names(true);
        assert!(all.contains(&".git".to_string()) && all.contains(&".env".to_string()) && all.contains(&"target".to_string()));
        assert_eq!(&all[..5], [".git", "Alfa", "node_modules", "target", "zeta"]);
        assert!(list_dir(&d.join("finns-inte"), false).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn walk_respects_gitignore_hidden_and_skip_dirs() {
        let d = tmp("walk");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("target/debug")).unwrap();
        std::fs::create_dir_all(d.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::create_dir_all(d.join("out")).unwrap();
        std::fs::write(d.join("src/a.rs"), "").unwrap();
        std::fs::write(d.join("target/debug/b"), "").unwrap();
        std::fs::write(d.join("node_modules/x/c.js"), "").unwrap();
        std::fs::write(d.join(".git/config"), "").unwrap();
        std::fs::write(d.join(".hidden"), "").unwrap();
        std::fs::write(d.join("out/gen.txt"), "").unwrap();
        std::fs::write(d.join("secret.log"), "").unwrap();
        std::fs::write(d.join("keep.txt"), "").unwrap();
        // Ingen .git-katalog behövs för att .gitignore ska gälla.
        std::fs::remove_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".gitignore"), "out/\n*.log\n").unwrap();
        let mut found = Vec::new();
        walk_files(&d, &AtomicBool::new(false), |p| {
            found.push(p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/"));
            true
        });
        assert_eq!(found, ["keep.txt", "src/a.rs"]);
        // Avbrott stoppar genast.
        let mut n = 0;
        walk_files(&d, &AtomicBool::new(true), |_| {
            n += 1;
            true
        });
        assert_eq!(n, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn name_validation() {
        assert_eq!(valid_name("  a.txt "), Ok("a.txt"));
        assert!(valid_name("").is_err());
        assert!(valid_name("..").is_err());
        assert!(valid_name("a/b").is_err());
        assert!(valid_name("a\\b").is_err());
    }

    #[test]
    fn create_rename_and_clash() {
        let d = tmp("fsops");
        let f = create_file(&d, "ny.txt").unwrap();
        assert!(f.is_file());
        assert!(create_file(&d, "ny.txt").is_err());
        let sub = create_dir(&d, "mapp").unwrap();
        assert!(sub.is_dir());
        assert!(create_dir(&d, "mapp").is_err());
        let r = rename_path(&f, "nytt.txt").unwrap();
        assert!(r.is_file() && !f.exists());
        // Skriver aldrig över en befintlig fil.
        std::fs::write(d.join("annan.txt"), "x").unwrap();
        assert!(rename_path(&r, "annan.txt").is_err());
        assert_eq!(std::fs::read_to_string(d.join("annan.txt")).unwrap(), "x");
        assert!(rename_path(&r, "../ut.txt").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn moved_rewrites_prefix() {
        let (old, new) = (Path::new("/a/b"), Path::new("/a/c"));
        assert_eq!(moved(Path::new("/a/b"), old, new), Some(PathBuf::from("/a/c")));
        assert_eq!(moved(Path::new("/a/b/x/y.txt"), old, new), Some(PathBuf::from("/a/c/x/y.txt")));
        assert_eq!(moved(Path::new("/a/bb/x"), old, new), None);
    }
}
