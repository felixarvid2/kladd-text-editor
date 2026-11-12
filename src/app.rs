//! Kladds fönster: flikar, editor, sök, förhandsvisning och dialoger.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Align, Align2, CentralPanel, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Key,
    KeyboardShortcut, Layout, Margin, Modifiers, Order, Panel, Popup, Rect, RectAlign, Response,
    RichText, ScrollArea, Sense, Shape, Stroke, TextBuffer, TextEdit, Ui, UiBuilder,
    ViewportCommand, pos2, vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde::{Deserialize, Serialize};

use crate::doc::{Document, LineEnding, LoadError, TEXT_EXTENSIONS, TextEncoding, join_sv};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp"];

fn is_image_path(p: &Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()) || e == "webp")
}
use crate::highlight::{Highlighter, Params, find_all};
use crate::icons::{self, Icon};
use crate::single::{self, Incoming};
use crate::theme::{self, Palette};

const SETTINGS_KEY: &str = "kladd-settings";
const TOPBAR_H: f32 = 46.0;
const TOPBAR_PAD_TOP: f32 = 8.0;
const STATUS_H: f32 = 28.0;
const DEFAULT_FONT_SIZE: f32 = 15.0;
const MAX_RECENT: usize = 8;
const WINDOW_CONTROLS_W: f32 = 3.0 * 38.0 + 14.0;
const RESIZE_EDGE: f32 = 6.0;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    pub font_size: f32,
    pub wrap: bool,
    pub preview: bool,
    pub line_numbers: bool,
    pub recent: Vec<PathBuf>,
    /// Zoom för sidan i dokumentläget (1.0 = 100 %).
    pub word_zoom: f32,
    /// Stavningskontroll i dokumentläget.
    pub spell: bool,
    /// Vald ordlista (t.ex. "sv_SE"); `None` = automatiskt.
    pub spell_lang: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: true,
            font_size: DEFAULT_FONT_SIZE,
            wrap: true,
            preview: true,
            line_numbers: true,
            recent: Vec::new(),
            word_zoom: 1.0,
            spell: true,
            spell_lang: None,
        }
    }
}

enum DialogResult {
    Open(Vec<PathBuf>),
    InsertImage { doc_id: u64, path: Option<PathBuf> },
    SaveAs { doc_id: u64, path: Option<PathBuf>, then_close: bool, exit: bool },
}

struct PendingPassword {
    path: PathBuf,
    input: String,
    wrong: bool,
    focus: bool,
}

#[derive(Clone, Copy)]
struct PendingDocx {
    doc_id: u64,
    then_close: bool,
    exit: bool,
}

#[derive(Clone, Copy)]
struct PendingClose {
    doc_id: u64,
    exit: bool,
}

#[derive(Default)]
pub(crate) struct Find {
    pub(crate) open: bool,
    pub(crate) replace: bool,
    pub(crate) query: String,
    pub(crate) replacement: String,
    pub(crate) case_sensitive: bool,
    /// Byte-start för den aktuella träffen.
    pub(crate) current: Option<usize>,
    pub(crate) focus_query: bool,
}

struct Toast {
    text: String,
    until: f64,
    error: bool,
}

enum Seek {
    FromSelectionStart,
    Next,
    Prev,
}

pub struct KladdApp {
    pub(crate) docs: Vec<Document>,
    pub(crate) active: usize,
    next_id: u64,
    untitled_counter: usize,
    pub(crate) settings: Settings,
    hl: Highlighter,
    md_cache: CommonMarkCache,
    pub(crate) find: Find,
    dialog: Option<Receiver<DialogResult>>,
    pending_close: Option<PendingClose>,
    pending_docx: Option<PendingDocx>,
    pending_password: Option<PendingPassword>,
    allow_exit: bool,
    toast: Option<Toast>,
    cursor: (usize, usize, usize),
    pub(crate) scroll_to_char: Option<usize>,
    pub(crate) focus_editor: bool,
    pub(crate) editor_rect: Rect,
    last_title: String,
    last_active_id: Option<u64>,
    incoming: Option<Receiver<Incoming>>,
    preview_images: std::collections::HashSet<String>,
    owns_socket: bool,
    pub(crate) word: crate::word::layout::Cache,
    /// Typsnitt som Word-dokumenten använder, hämtade från systemet.
    pub(crate) doc_fonts: crate::word::fonts::DocFonts,
    pub(crate) speller: Option<crate::spell::Speller>,
    pub(crate) wv: crate::word::view::State,
}

pub(crate) fn editor_id(doc_id: u64) -> Id {
    Id::unique(("kladd-editor", doc_id))
}

pub(crate) fn char_to_byte(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b)
}

pub(crate) fn byte_to_char(text: &str, b: usize) -> usize {
    text[..b.min(text.len())].chars().count()
}

fn line_col(text: &str, ci: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, c) in text.chars().enumerate() {
        if i >= ci {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn pretty_dir(path: &Path) -> String {
    let dir = path.parent().unwrap_or(path);
    let s = dir.display().to_string();
    if let Some(home) = home_dir() {
        let h = home.display().to_string();
        if let Some(rest) = s.strip_prefix(&h) {
            return format!("~{rest}");
        }
    }
    s
}

impl KladdApp {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>, single_instance: bool) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install_fonts(ctx);
        theme::install_styles(ctx);
        egui_extras::install_image_loaders(ctx);
        ctx.options_mut(|o| o.zoom_with_keyboard = false);

        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        theme::set_dark(ctx, settings.dark);

        let mut app = Self {
            docs: Vec::new(),
            active: 0,
            next_id: 1,
            untitled_counter: 0,
            settings,
            hl: Highlighter::new(),
            md_cache: CommonMarkCache::default(),
            find: Find::default(),
            dialog: None,
            pending_close: None,
            pending_docx: None,
            pending_password: None,
            allow_exit: false,
            toast: None,
            cursor: (1, 1, 0),
            scroll_to_char: None,
            focus_editor: false,
            editor_rect: Rect::NOTHING,
            last_title: String::new(),
            last_active_id: None,
            incoming: None,
            preview_images: std::collections::HashSet::new(),
            owns_socket: false,
            word: Default::default(),
            doc_fonts: crate::word::fonts::DocFonts::new(),
            speller: None,
            wv: Default::default(),
        };
        if single_instance {
            let (tx, rx) = channel();
            if single::listen(tx, ctx.clone()) {
                app.incoming = Some(rx);
                app.owns_socket = true;
            }
        }
        for f in files {
            app.open_path(ctx, &f);
        }
        app
    }

    pub(crate) fn palette(&self) -> Palette {
        theme::palette(self.settings.dark)
    }

    pub(crate) fn now(ctx: &egui::Context) -> f64 {
        ctx.input(|i| i.time)
    }

    pub(crate) fn notify(&mut self, ctx: &egui::Context, text: impl Into<String>, error: bool) {
        let secs = if error { 6.0 } else { 2.5 };
        self.toast = Some(Toast { text: text.into(), until: Self::now(ctx) + secs, error });
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(secs + 0.05));
    }

    // ---------------------------------------------------------------- dokument

    fn new_tab(&mut self) {
        self.untitled_counter += 1;
        let doc = Document::new_untitled(self.next_id, self.untitled_counter);
        self.next_id += 1;
        self.docs.push(doc);
        self.active = self.docs.len() - 1;
        self.focus_editor = true;
    }

    fn remember(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.settings.recent.retain(|p| p != &path);
        self.settings.recent.insert(0, path);
        self.settings.recent.truncate(MAX_RECENT);
    }

    fn open_path(&mut self, ctx: &egui::Context, path: &Path) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(&canonical)) {
            self.active = i;
            self.focus_editor = true;
            return;
        }
        self.load_into_tab(ctx, &canonical, None);
    }

    fn load_into_tab(&mut self, ctx: &egui::Context, canonical: &Path, password: Option<&str>) {
        let canonical = canonical.to_path_buf();
        let loaded = Document::load_with_password(self.next_id, &canonical, password);
        let loaded = match loaded {
            Err(LoadError::NeedsPassword) => {
                self.pending_password = Some(PendingPassword { path: canonical, input: String::new(), wrong: false, focus: true });
                return;
            }
            Err(LoadError::WrongPassword) => {
                self.pending_password = Some(PendingPassword { path: canonical, input: String::new(), wrong: true, focus: true });
                return;
            }
            Err(LoadError::Message(m)) => Err(m),
            Ok(d) => Ok(d),
        };
        match loaded {
            Ok(doc) => {
                self.next_id += 1;
                // Ersätt en tom, orörd namnlös flik i stället för att lägga till en ny.
                let replace = self
                    .docs
                    .get(self.active)
                    .is_some_and(|d| d.path.is_none() && !d.dirty && d.text.is_empty());
                if replace {
                    let old = self.docs[self.active].id;
                    self.hl.forget(old);
                    self.word.forget(old);
                    self.docs[self.active] = doc;
                } else {
                    self.docs.push(doc);
                    self.active = self.docs.len() - 1;
                }
                self.remember(&canonical);
                self.focus_editor = true;
                if let Some(note) = self.docs[self.active].open_note.take() {
                    self.notify(ctx, note, false);
                }
            }
            Err(e) => {
                self.settings.recent.retain(|p| p != &canonical);
                self.notify(ctx, e, true);
            }
        }
    }

    fn start_dir(&self) -> Option<PathBuf> {
        self.docs
            .get(self.active)
            .and_then(|d| d.path.as_ref())
            .or(self.settings.recent.first())
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(home_dir)
    }

    fn request_open(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.dialog = Some(rx);
        let ctx = ctx.clone();
        let dir = self.start_dir();
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new()
                .set_title("Öppna fil")
                .add_filter("Alla filer", &["*"])
                .add_filter("Text och Word", TEXT_EXTENSIONS)
                .add_filter("Word-dokument", &["docx", "docm", "dotx"])
                .add_filter("PDF", &["pdf"]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let files = dlg.pick_files().unwrap_or_default();
            let _ = tx.send(DialogResult::Open(files));
            ctx.request_repaint();
        });
    }

    fn request_save_as(&mut self, ctx: &egui::Context, idx: usize, then_close: bool, exit: bool) {
        if self.dialog.is_some() {
            self.notify(ctx, "Stäng den öppna fildialogen först.", true);
            return;
        }
        let Some(doc) = self.docs.get(idx) else { return };
        let (tx, rx) = channel();
        self.dialog = Some(rx);
        let ctx = ctx.clone();
        let doc_id = doc.id;
        let name = if doc.needs_save_as() {
            let stem = doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned());
            format!("{}.docx", stem.unwrap_or_else(|| doc.title()))
        } else if doc.path.is_some() {
            doc.title()
        } else {
            format!("{}.txt", doc.title())
        };
        let dir = doc
            .path
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(|| self.start_dir());
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new()
                .set_title("Spara som")
                .set_file_name(name)
                .add_filter("Alla filer", &["*"])
                .add_filter("Text", &["txt"])
                .add_filter("Markdown", &["md"])
                .add_filter("Word-dokument", &["docx"])
                .add_filter("PDF", &["pdf"]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let path = dlg.save_file();
            let _ = tx.send(DialogResult::SaveAs { doc_id, path, then_close, exit });
            ctx.request_repaint();
        });
    }

    pub(crate) fn request_insert_image(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.docs.is_empty() {
            return;
        }
        let doc_id = self.docs[self.active].id;
        let (tx, rx) = channel();
        self.dialog = Some(rx);
        let ctx = ctx.clone();
        let dir = self.start_dir();
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new()
                .set_title("Infoga bild")
                .add_filter("Bilder", IMAGE_EXTENSIONS);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let path = dlg.pick_file();
            let _ = tx.send(DialogResult::InsertImage { doc_id, path });
            ctx.request_repaint();
        });
    }

    /// Lägger in `![namn](sökväg)` vid markören i den aktiva fliken.
    fn insert_image(&mut self, ctx: &egui::Context, path: &Path) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        if std::fs::read(path).ok().and_then(|b| crate::docx::image_ext(&b)).is_none() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.notify(ctx, format!("{name} är inte en bild som kan bäddas in (PNG, JPEG, GIF eller BMP)."), true);
            return;
        }
        // Relativ sökväg om bilden ligger i dokumentets mapp, annars absolut.
        let src = match doc.base_dir().and_then(|b| path.strip_prefix(&b).ok().map(Path::to_path_buf)) {
            Some(rel) => rel,
            None => path.to_path_buf(),
        };
        let src = src.to_string_lossy().replace(' ', "%20");
        let alt = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let ci = state.cursor.char_range().map_or(doc.text.chars().count(), |r| r.primary.index.0);
        let at = char_to_byte(&doc.text, ci);
        let before = &doc.text[..at];
        let lead = if before.is_empty() || before.ends_with("\n\n") {
            ""
        } else if before.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let snippet = format!("{lead}![{}]({src})\n", alt.replace(['[', ']'], ""));
        doc.text.insert_str(at, &snippet);
        doc.on_edited();
        let after = byte_to_char(&doc.text, at + snippet.len());
        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(after))));
        state.store(ctx, id);
        self.focus_editor = true;
        self.scroll_to_char = Some(after);
    }

    /// Sparar dokumentet. Öppnar "Spara som" om det saknar sökväg.
    fn save_doc(&mut self, ctx: &egui::Context, idx: usize, then_close: bool, exit: bool) {
        let Some(doc) = self.docs.get(idx) else { return };
        let Some(path) = doc.path.clone().filter(|_| !doc.needs_save_as()) else {
            self.request_save_as(ctx, idx, then_close, exit);
            return;
        };
        // Att spara över ett Word-dokument med bilder m.m. kräver ett aktivt val.
        if (doc.docx || doc.pdf) && !doc.docx_dropped.is_empty() {
            self.pending_docx = Some(PendingDocx { doc_id: doc.id, then_close, exit });
            return;
        }
        self.write_doc(ctx, idx, &path, then_close, exit);
    }

    fn write_doc(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        path: &Path,
        then_close: bool,
        exit: bool,
    ) {
        let doc = &mut self.docs[idx];
        let was_md = doc.is_markdown();
        match doc.save_to(path) {
            Ok(warnings) => {
                let title = doc.title();
                if was_md != doc.is_markdown() || doc.extension().is_empty() {
                    self.hl.forget(doc.id);
                }
                self.remember(path);
                if warnings.is_empty() {
                    self.notify(ctx, format!("Sparade {title}"), false);
                } else {
                    let list: Vec<&str> = warnings.iter().map(String::as_str).collect();
                    let what = if list.len() == 1 { "bilden" } else { "bilderna" };
                    self.notify(ctx, format!("Sparade {title}, men hittade inte {what} {}", join_sv(&list)), true);
                }
                if then_close {
                    self.close_tab_now(idx);
                }
                if exit {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Err(e) => self.notify(ctx, e, true),
        }
    }

    fn request_close_tab(&mut self, idx: usize) {
        let Some(doc) = self.docs.get(idx) else { return };
        if doc.dirty {
            self.active = idx;
            self.pending_close = Some(PendingClose { doc_id: doc.id, exit: false });
        } else {
            self.close_tab_now(idx);
        }
    }

    fn close_tab_now(&mut self, idx: usize) {
        if idx >= self.docs.len() {
            return;
        }
        let doc = self.docs.remove(idx);
        self.hl.forget(doc.id);
        self.word.forget(doc.id);
        if self.docs.is_empty() {
            self.active = 0;
            self.untitled_counter = 0;
            self.find.open = false;
        } else if self.active > idx || self.active >= self.docs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.focus_editor = true;
    }

    fn index_of(&self, doc_id: u64) -> Option<usize> {
        self.docs.iter().position(|d| d.id == doc_id)
    }

    // ---------------------------------------------------------------- sök

    fn open_find(&mut self, ctx: &egui::Context, replace: bool) {
        if self.docs.is_empty() {
            return;
        }
        // Förifyll med markerad text om den är kort och på en rad.
        let doc = &self.docs[self.active];
        if let Some(state) = TextEditState::load(ctx, editor_id(doc.id)) {
            if let Some(r) = state.cursor.char_range() {
                let [a, b] = r.sorted_cursors();
                if a.index != b.index {
                    let ba = char_to_byte(&doc.text, a.index.0);
                    let bb = char_to_byte(&doc.text, b.index.0);
                    let sel = &doc.text[ba..bb];
                    if !sel.contains('\n') && sel.chars().count() <= 80 {
                        self.find.query = sel.to_owned();
                        self.find.current = Some(ba);
                    }
                }
            }
        }
        if replace {
            self.find.replace = true;
        } else if !self.find.open {
            self.find.replace = false;
        }
        self.find.open = true;
        self.find.focus_query = true;
    }

    fn close_find(&mut self) {
        self.find.open = false;
        self.find.current = None;
        self.focus_editor = true;
    }

    fn seek(&mut self, ctx: &egui::Context, how: Seek) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let matches = find_all(&doc.text, &self.find.query, self.find.case_sensitive);
        if matches.is_empty() {
            self.find.current = None;
            return;
        }
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let (sa, sb) = state
            .cursor
            .char_range()
            .map(|r| {
                let [a, b] = r.sorted_cursors();
                (a.index.0, b.index.0)
            })
            .unwrap_or((0, 0));
        let (ba, bb) = (char_to_byte(&doc.text, sa), char_to_byte(&doc.text, sb));
        let pick = match how {
            Seek::FromSelectionStart => matches.iter().find(|m| m.0 >= ba).or(matches.first()),
            Seek::Next => {
                let from = if self.find.current == Some(ba) { bb.max(ba + 1) } else { ba };
                matches.iter().find(|m| m.0 >= from).or(matches.first())
            }
            Seek::Prev => matches.iter().rev().find(|m| m.0 < ba).or(matches.last()),
        };
        let &(ma, mb) = pick.expect("minst en träff");
        let (ca, cb) = (byte_to_char(&doc.text, ma), byte_to_char(&doc.text, mb));
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(ca), CCursor::new(cb))));
        state.store(ctx, id);
        self.find.current = Some(ma);
        self.scroll_to_char = Some(ca);
    }

    fn replace_one(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let matches = find_all(&doc.text, &self.find.query, self.find.case_sensitive);
        let current = self
            .find
            .current
            .and_then(|c| matches.iter().copied().find(|m| m.0 == c));
        if let Some((a, b)) = current {
            doc.text.replace_range(a..b, &self.find.replacement);
            doc.on_edited();
            let after = byte_to_char(&doc.text, a + self.find.replacement.len());
            let id = editor_id(doc.id);
            let mut state = TextEditState::load(ctx, id).unwrap_or_default();
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(after))));
            state.store(ctx, id);
            self.find.current = None;
        }
        self.seek(ctx, Seek::FromSelectionStart);
    }

    fn replace_all(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let matches = find_all(&doc.text, &self.find.query, self.find.case_sensitive);
        if matches.is_empty() {
            return;
        }
        let mut out = String::with_capacity(doc.text.len());
        let mut last = 0;
        for &(a, b) in &matches {
            out.push_str(&doc.text[last..a]);
            out.push_str(&self.find.replacement);
            last = b;
        }
        out.push_str(&doc.text[last..]);
        doc.text = out;
        doc.on_edited();
        self.find.current = None;
        let n = matches.len();
        let msg = if n == 1 { "Ersatte 1 träff".to_owned() } else { format!("Ersatte {n} träffar") };
        self.notify(ctx, msg, false);
    }

    // ---------------------------------------------------------------- händelser

    fn handle_dialogs(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.dialog else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.dialog = None;
                self.notify(ctx, "Fildialogen kunde inte öppnas. Installera paketet zenity och försök igen.", true);
                return;
            }
        };
        self.dialog = None;
        match result {
            DialogResult::Open(files) => {
                for f in files {
                    self.open_path(ctx, &f);
                }
            }
            DialogResult::InsertImage { doc_id, path } => {
                if let (Some(path), Some(idx)) = (path, self.index_of(doc_id)) {
                    self.active = idx;
                    self.insert_image(ctx, &path);
                }
            }
            DialogResult::SaveAs { doc_id, path, then_close, exit } => {
                if let (Some(path), Some(idx)) = (path, self.index_of(doc_id)) {
                    let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
                    let clash = self
                        .docs
                        .iter()
                        .any(|d| d.id != doc_id && d.path.as_deref() == Some(canon.as_path()));
                    if clash {
                        self.notify(ctx, "Filen är redan öppen i en annan flik. Stäng den först eller välj ett annat namn.", true);
                    } else {
                        self.write_doc(ctx, idx, &path, then_close, exit);
                    }
                }
            }
        }
    }

    /// Filer som en annan `kladd`-process har skickat hit (t.ex. från filhanteraren).
    fn handle_incoming(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.incoming else { return };
        let batches: Vec<Incoming> = rx.try_iter().collect();
        if batches.is_empty() {
            return;
        }
        for Incoming::Open(files) in batches {
            for f in files {
                self.open_path(ctx, &f);
            }
        }
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let p = f.path().to_path_buf();
            if p.as_os_str().is_empty() {
                continue;
            }
            if is_image_path(&p) && !self.docs.is_empty() {
                self.insert_image(ctx, &p);
            } else {
                self.open_path(ctx, &p);
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.pending_close.is_some() || self.pending_docx.is_some() || self.pending_password.is_some() {
            return;
        }
        let ctrl = Modifiers::CTRL;
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let hit = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };

        if hit(ctrl, Key::T) || hit(ctrl, Key::N) {
            self.new_tab();
        }
        if hit(ctrl, Key::O) {
            self.request_open(ctx);
        }
        if hit(ctrl_shift, Key::S) {
            if !self.docs.is_empty() {
                self.request_save_as(ctx, self.active, false, false);
            }
        } else if hit(ctrl, Key::S) && !self.docs.is_empty() {
            self.save_doc(ctx, self.active, false, false);
        }
        if hit(ctrl, Key::W) && !self.docs.is_empty() {
            self.request_close_tab(self.active);
        }
        if hit(ctrl_shift, Key::Tab) || hit(ctrl, Key::PageUp) {
            if !self.docs.is_empty() {
                self.active = (self.active + self.docs.len() - 1) % self.docs.len();
                self.focus_editor = true;
            }
        } else if (hit(ctrl, Key::Tab) || hit(ctrl, Key::PageDown)) && !self.docs.is_empty() {
            self.active = (self.active + 1) % self.docs.len();
            self.focus_editor = true;
        }
        if hit(ctrl_shift, Key::I) && !self.docs.is_empty() {
            self.request_insert_image(ctx);
        }
        if hit(ctrl, Key::F) {
            self.open_find(ctx, false);
        }
        if hit(ctrl, Key::H) {
            self.open_find(ctx, true);
        }
        if self.find.open {
            if hit(Modifiers::NONE, Key::Escape) {
                self.close_find();
            }
            if hit(Modifiers::SHIFT, Key::F3) {
                self.seek(ctx, Seek::Prev);
            } else if hit(Modifiers::NONE, Key::F3) {
                self.seek(ctx, Seek::Next);
            }
        }
        let word = self.word_mode();
        if word {
            self.word_shortcuts(ctx);
        }
        if hit(ctrl, Key::Plus) || hit(ctrl, Key::Equals) {
            if word { self.word_zoom_step(1.0) } else { self.zoom(1.0) }
        }
        if hit(ctrl, Key::Minus) {
            if word { self.word_zoom_step(-1.0) } else { self.zoom(-1.0) }
        }
        if hit(ctrl, Key::Num0) {
            if word { self.settings.word_zoom = 1.0 } else { self.settings.font_size = DEFAULT_FONT_SIZE }
        }
        if hit(Modifiers::ALT, Key::Z) {
            self.settings.wrap = !self.settings.wrap;
        }
        if hit(ctrl_shift, Key::M) {
            if !word { self.settings.preview = !self.settings.preview; }
        }
        if hit(ctrl_shift, Key::L) {
            self.toggle_theme(ctx);
        }
        if hit(ctrl, Key::Q) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn zoom(&mut self, delta: f32) {
        self.settings.font_size = (self.settings.font_size + delta).clamp(9.0, 32.0);
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.settings.dark = !self.settings.dark;
        theme::set_dark(ctx, self.settings.dark);
    }

    fn handle_exit(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.allow_exit {
            return;
        }
        if let Some(idx) = self.docs.iter().position(|d| d.dirty) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.active = idx;
            self.pending_close = Some(PendingClose { doc_id: self.docs[idx].id, exit: true });
        } else {
            self.allow_exit = true;
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.docs.get(self.active) {
            Some(d) => format!("{}{} – Kladd", if d.dirty { "• " } else { "" }, d.title()),
            None => "Kladd".to_owned(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    // ---------------------------------------------------------------- UI

    fn top_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        Panel::top("kladd-topbar")
            .exact_size(TOPBAR_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin {
                left: 14,
                right: 10,
                top: TOPBAR_PAD_TOP as i8,
                bottom: 0,
            }))
            .show(ui, |ui| {
                let full = ui.max_rect();
                // Den tomma ytan i titelraden flyttar fönstret; dubbelklick maximerar.
                let bar_rect = full.expand2(vec2(14.0, 0.0)).with_min_y(full.top() - TOPBAR_PAD_TOP);
                let bar = ui.interact(bar_rect, Id::unique("kladd-drag"), Sense::click_and_drag());
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                if bar.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
                if bar.double_clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }
                let show_preview_btn = self.docs.get(self.active).is_some_and(Document::is_markdown) && !self.word_mode();
                let right_w = if show_preview_btn { 3.0 * 34.0 } else { 2.0 * 34.0 } + WINDOW_CONTROLS_W;
                let left = Rect::from_min_max(full.min, pos2(full.right() - right_w - 8.0, full.bottom()));
                let right = Rect::from_min_max(pos2(full.right() - right_w, full.top()), full.max);

                ui.scope_builder(
                    UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        self.wordmark(ui, p);
                        ui.add_space(14.0);
                        ScrollArea::horizontal()
                            .id_salt("kladd-tabs")
                            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                            .auto_shrink([true, true])
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    self.tabs(ui, p);
                                    ui.add_space(4.0);
                                    if icons::button(ui, p, Icon::Plus, 28.0, false, "Ny flik (Ctrl+T)", p.chrome).clicked() {
                                        self.new_tab();
                                    }
                                });
                            });
                    },
                );

                ui.scope_builder(
                    UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if icons::window_button(ui, p, Icon::WindowClose, "Stäng", true).clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                        let (icon, tip) = if maximized {
                            (Icon::Restore, "Återställ")
                        } else {
                            (Icon::Maximize, "Maximera")
                        };
                        if icons::window_button(ui, p, icon, tip, false).clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                        }
                        if icons::window_button(ui, p, Icon::Minimize, "Minimera", false).clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                        }
                        // diskret avdelare mellan appens knappar och fönsterknapparna
                        let (sep, _) = ui.allocate_exact_size(vec2(14.0, 30.0), Sense::hover());
                        ui.painter().vline(sep.center().x, sep.y_range().shrink(7.0), Stroke::new(1.0, p.line));
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let menu = icons::button(ui, p, Icon::Menu, 30.0, false, "Meny", p.chrome);
                        Popup::menu(&menu)
                            .width(312.0)
                            .align(RectAlign::BOTTOM_END)
                            .gap(6.0)
                            .show(|ui| self.menu(ui, &ctx, p));

                        let (icon, tip) = if self.settings.dark {
                            (Icon::Sun, "Byt till ljust tema (Ctrl+Shift+L)")
                        } else {
                            (Icon::Moon, "Byt till mörkt tema (Ctrl+Shift+L)")
                        };
                        if icons::button(ui, p, icon, 30.0, false, tip, p.chrome).clicked() {
                            self.toggle_theme(&ctx);
                        }
                        if show_preview_btn {
                            let tip = if self.settings.preview {
                                "Dölj förhandsvisning (Ctrl+Shift+M)"
                            } else {
                                "Visa förhandsvisning (Ctrl+Shift+M)"
                            };
                            if icons::button(ui, p, Icon::Split, 30.0, self.settings.preview, tip, p.chrome)
                                .clicked()
                            {
                                self.settings.preview = !self.settings.preview;
                            }
                        }
                    },
                );
            });
    }

    fn wordmark(&self, ui: &mut Ui, p: &Palette) {
        let font = FontId::new(15.0, theme::semibold());
        let galley = ui.painter().layout_no_wrap("Kladd".into(), font, p.text);
        let w = galley.size().x + 10.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w, ui.available_height()), Sense::hover());
        let c = rect.left_center();
        let text_pos = pos2(c.x, c.y - galley.size().y / 2.0);
        let caret_x = text_pos.x + galley.size().x + 3.0;
        ui.painter().galley(text_pos, galley, p.text);
        ui.painter().rect_filled(
            Rect::from_center_size(pos2(caret_x + 1.0, c.y), vec2(2.5, 16.0)),
            1.0,
            p.accent,
        );
    }

    fn tabs(&mut self, ui: &mut Ui, p: &Palette) {
        let mut activate = None;
        let mut close = None;
        let height = ui.available_height().max(TOPBAR_H - TOPBAR_PAD_TOP);
        for (i, doc) in self.docs.iter().enumerate() {
            let active = i == self.active;
            let family = if active { theme::medium() } else { FontFamily::Proportional };
            let color = if active { p.text } else { p.muted };
            let title = truncate(&doc.title(), 26);
            let galley =
                ui.painter().layout_no_wrap(title, FontId::new(13.5, family), color);
            let w = (galley.size().x + 14.0 + 34.0).max(96.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
            let resp = resp.on_hover_text(
                doc.path.as_ref().map_or_else(|| "Inte sparad än".to_owned(), |p| p.display().to_string()),
            );
            let close_rect = Rect::from_center_size(pos2(rect.right() - 17.0, rect.center().y), vec2(20.0, 20.0));
            let close_resp = ui.interact(close_rect, resp.id.with("close"), Sense::click());

            let hovered = resp.hovered() || close_resp.hovered();
            let radius = CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 };
            let painter = ui.painter();
            if active {
                painter.rect_filled(rect, radius, p.editor);
                painter.rect_filled(
                    Rect::from_min_size(rect.left_top() + vec2(12.0, 0.0), vec2(w - 24.0, 2.0)),
                    1.0,
                    p.accent,
                );
            } else if hovered {
                painter.rect_filled(rect.shrink2(vec2(0.0, 3.0)).translate(vec2(0.0, -1.0)), 7.0, p.surface);
            }
            let text_y = rect.center().y - galley.size().y / 2.0;
            painter.galley(pos2(rect.left() + 14.0, text_y), galley, color);

            if close_resp.hovered() {
                painter.circle_filled(close_rect.center(), 9.0, p.surface_hover);
                icons::paint(painter, Icon::Close, close_rect.center(), p.text, p.surface_hover);
            } else if doc.dirty && !hovered {
                painter.circle_filled(close_rect.center(), 3.5, if active { p.accent } else { p.muted });
            } else if active || hovered {
                icons::paint(painter, Icon::Close, close_rect.center(), p.muted, p.editor);
            }

            if close_resp.clicked() || resp.middle_clicked() {
                close = Some(i);
            } else if resp.clicked() {
                activate = Some(i);
            }
        }
        if let Some(i) = activate {
            self.active = i;
            self.focus_editor = true;
        }
        if let Some(i) = close {
            self.request_close_tab(i);
        }
    }

    fn menu(&mut self, ui: &mut Ui, ctx: &egui::Context, p: &Palette) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let has_doc = !self.docs.is_empty();
        if menu_item(ui, p, "Ny flik", "Ctrl+T", None, true) {
            self.new_tab();
        }
        if menu_item(ui, p, "Öppna…", "Ctrl+O", None, true) {
            self.request_open(ctx);
        }
        if menu_item(ui, p, "Spara", "Ctrl+S", None, has_doc) {
            self.save_doc(ctx, self.active, false, false);
        }
        if menu_item(ui, p, "Spara som…", "Ctrl+Shift+S", None, has_doc) {
            self.request_save_as(ctx, self.active, false, false);
        }
        if menu_item(ui, p, "Stäng flik", "Ctrl+W", None, has_doc) {
            self.request_close_tab(self.active);
        }
        menu_divider(ui, p);
        if menu_item(ui, p, "Infoga bild…", "Ctrl+Shift+I", None, has_doc) {
            self.request_insert_image(ctx);
        }
        if menu_item(ui, p, "Sök", "Ctrl+F", None, has_doc) {
            self.open_find(ctx, false);
        }
        if menu_item(ui, p, "Sök och ersätt", "Ctrl+H", None, has_doc) {
            self.open_find(ctx, true);
        }
        menu_divider(ui, p);
        if menu_item(ui, p, "Radbrytning", "Alt+Z", Some(self.settings.wrap), true) {
            self.settings.wrap = !self.settings.wrap;
        }
        if menu_item(ui, p, "Radnummer", "", Some(self.settings.line_numbers), true) {
            self.settings.line_numbers = !self.settings.line_numbers;
        }
        if menu_item(ui, p, "Förhandsvisa Markdown", "Ctrl+Shift+M", Some(self.settings.preview), true) {
            self.settings.preview = !self.settings.preview;
        }
        if menu_item(ui, p, "Mörkt tema", "Ctrl+Shift+L", Some(self.settings.dark), true) {
            self.toggle_theme(ctx);
        }
        menu_divider(ui, p);
        if menu_item(ui, p, "Större text", "Ctrl++", None, true) {
            self.zoom(1.0);
        }
        if menu_item(ui, p, "Mindre text", "Ctrl+−", None, true) {
            self.zoom(-1.0);
        }
        if menu_item(ui, p, "Normal textstorlek", "Ctrl+0", None, true) {
            self.settings.font_size = DEFAULT_FONT_SIZE;
        }
        menu_divider(ui, p);
        if menu_item(ui, p, "Avsluta", "Ctrl+Q", None, true) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn status_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        Panel::bottom("kladd-status")
            .exact_size(STATUS_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin::symmetric(14, 0)))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().hline(
                    (r.left() - 14.0)..=(r.right() + 14.0),
                    r.top(),
                    Stroke::new(1.0, p.line),
                );
                let font = FontId::new(12.0, FontFamily::Proportional);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    if let Some(doc) = self.docs.get(self.active) {
                        let lang = if doc.word_view { "Word".to_owned() } else { doc.language() };
                        ui.label(RichText::new(lang).font(font.clone()).color(p.muted));
                    }
                    let now = Self::now(&ctx);
                    if let Some(t) = &self.toast {
                        if t.until > now {
                            let color = if t.error { p.error } else { p.accent };
                            ui.label(RichText::new(&t.text).font(font.clone()).color(color));
                        } else {
                            self.toast = None;
                        }
                    }

                    if self.word_mode() {
                        self.word_status(ui, p, &font);
                        return;
                    }
                    let Some(doc) = self.docs.get_mut(self.active) else { return };
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if doc.docx || doc.pdf {
                            let (label, tip) = if doc.pdf {
                                ("PDF-dokument", "Texten visas som Markdown och sparas som en ny PDF")
                            } else {
                                ("Word-dokument", "Visas som Markdown och sparas som .docx")
                            };
                            if doc.docx && !doc.pdf {
                                if status_button(ui, p, "Visa som dokument", "Visa som Word-dokument med sidor och formateringsrad").clicked() {
                                    doc.word_view = true;
                                }
                            } else {
                            ui.label(RichText::new(label).font(font.clone()).color(p.muted)).on_hover_text(tip);
                            }
                        } else {
                        let le = status_button(ui, p, doc.line_ending.label(), "Radslut när filen sparas");
                        if le.clicked() {
                            doc.line_ending = match doc.line_ending {
                                LineEnding::Lf => LineEnding::CrLf,
                                LineEnding::CrLf => LineEnding::Lf,
                            };
                            doc.on_edited();
                        }
                        let enc = status_button(ui, p, doc.encoding.label(), "Teckenkodning när filen sparas");
                        Popup::menu(&enc).align(RectAlign::TOP_END).gap(6.0).width(200.0).show(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for e in [
                                TextEncoding::Utf8,
                                TextEncoding::Utf8Bom,
                                TextEncoding::Utf16Le,
                                TextEncoding::Utf16Be,
                                TextEncoding::Windows1252,
                            ] {
                                if menu_item(ui, p, e.label(), "", Some(doc.encoding == e), true) && doc.encoding != e {
                                    doc.encoding = e;
                                    doc.on_edited();
                                }
                            }
                        });
                        }
                        ui.add_space(10.0);
                        let words = doc.text.split_whitespace().count();
                        let words = if words == 1 { "1 ord".to_owned() } else { format!("{words} ord") };
                        ui.label(RichText::new(words).font(font.clone()).color(p.muted));
                        ui.add_space(10.0);
                        let (line, col, sel) = self.cursor;
                        let pos = if sel > 0 {
                            format!("Rad {line}, kolumn {col}  ({sel} markerade)")
                        } else {
                            format!("Rad {line}, kolumn {col}")
                        };
                        ui.label(RichText::new(pos).font(font.clone()).color(p.muted));
                    });
                });
            });
    }

    fn preview(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if !(self.settings.preview && doc.is_markdown()) || self.word_mode() {
            return;
        }
        let width = ui.available_width();
        Panel::right("kladd-preview")
            .resizable(true)
            .default_size(width * 0.46)
            .size_range(260.0..=width * 0.75)
            .frame(Frame::NONE.fill(p.editor).inner_margin(Margin { left: 32, right: 24, top: 0, bottom: 0 }))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt(("kladd-preview", doc.id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(22.0);
                        let st = ui.style_mut();
                        st.text_styles.insert(egui::TextStyle::Body, FontId::new(15.0, FontFamily::Proportional));
                        st.text_styles.insert(egui::TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace));
                        st.text_styles.insert(egui::TextStyle::Heading, FontId::new(26.0, theme::semibold()));
                        st.spacing.item_spacing.y = 8.0;
                        st.visuals.override_text_color = Some(p.text);
                        ui.set_max_width(680.0f32.min(ui.available_width()));
                        // Bilder: inbäddade (bild:) registreras som minnesdata, övriga läses från disk.
                        // HTML-formatet (<u>, <span>, <div>, <img> …) görs först om till vanlig Markdown.
                        let mut text: std::borrow::Cow<str> = std::borrow::Cow::Owned(crate::preview::clean(&doc.text));
                        for (name, img) in &doc.images {
                            let enc = name.replace(' ', "%20");
                            let uri = format!("bytes://kladd/{}/{enc}", doc.id);
                            if self.preview_images.insert(uri.clone()) {
                                ui.ctx().include_bytes(uri.clone(), img.bytes.clone());
                            }
                            let from = format!("]({}{enc})", crate::docx::IMAGE_SCHEME);
                            if text.contains(&from) {
                                text = std::borrow::Cow::Owned(text.replace(&from, &format!("]({uri})")));
                            }
                        }
                        if let Some(home) = home_dir() {
                            if text.contains("](~/") {
                                text = std::borrow::Cow::Owned(text.replace("](~/", &format!("](file://{}/", home.display())));
                            }
                        }
                        let base = doc.base_dir().or_else(home_dir).unwrap_or_default();
                        let max_w = ui.available_width() as usize;
                        CommonMarkViewer::new()
                            .default_implicit_uri_scheme(format!("file://{}/", base.display()))
                            .max_image_width(Some(max_w))
                            .show(ui, &mut self.md_cache, &text);
                        ui.add_space(40.0);
                    });
            });
    }

    fn editor(&mut self, ui: &mut Ui, p: &Palette) {
        if self.word_mode() {
            self.word_page(ui, p);
            return;
        }
        CentralPanel::default()
            .frame(Frame::NONE.fill(p.editor))
            .show(ui, |ui| {
                self.editor_rect = ui.max_rect();
                if self.docs.is_empty() {
                    self.welcome(ui, p);
                    return;
                }
                let font_size = self.settings.font_size;
                let wrap = self.settings.wrap;
                let line_numbers = self.settings.line_numbers;
                let (query, case_sensitive) = if self.find.open {
                    (self.find.query.clone(), self.find.case_sensitive)
                } else {
                    (String::new(), false)
                };
                let current_match = self.find.current;
                let scroll_to = self.scroll_to_char.take();
                let focus = std::mem::take(&mut self.focus_editor);

                let doc = &mut self.docs[self.active];
                let hl = &mut self.hl;
                let hint = doc.syntax_hint();
                let doc_id = doc.id;
                let id = editor_id(doc_id);

                let mut edited = false;
                let scroll = if wrap { ScrollArea::vertical() } else { ScrollArea::both() };
                let result = scroll
                    .id_salt(("kladd-scroll", doc_id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let viewport_h = ui.clip_rect().height();
                        let num_font = FontId::new((font_size * 0.82).round(), FontFamily::Monospace);
                        let lines = doc.text.bytes().filter(|&b| b == b'\n').count() + 1;
                        let digits = lines.to_string().len().max(2) as f32;
                        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&num_font, '0'));
                        let gutter_w = if line_numbers { digits * char_w + 40.0 } else { 22.0 };

                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let (gutter, _) =
                                ui.allocate_exact_size(vec2(gutter_w, viewport_h), Sense::hover());
                            let bg_idx = ui.painter().add(Shape::Noop);

                            let params = Params {
                                doc_id,
                                syntax_hint: &hint,
                                palette: p,
                                font_size,
                                query: &query,
                                case_sensitive,
                                current_match,
                            };
                            let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, wrap_width: f32| {
                                let mut job = hl.layout(buf.as_str(), &params);
                                job.wrap.max_width = if wrap { wrap_width } else { f32::INFINITY };
                                ui.ctx().fonts_mut(|f| f.layout_job(job))
                            };
                            let avail_w = ui.available_width();
                            let out = TextEdit::multiline(&mut doc.text)
                                .id(id)
                                .frame(Frame::NONE)
                                .margin(Margin { left: 2, right: 32, top: 18, bottom: 18 })
                                .desired_width(if wrap { avail_w } else { f32::INFINITY })
                                .min_size(vec2(avail_w, viewport_h))
                                .lock_focus(true)
                                .cursor_at_end(false)
                                .layouter(&mut layouter)
                                .show(ui);

                            if out.response.changed() {
                                doc.on_edited();
                                edited = true;
                            }
                            if focus {
                                ui.memory_mut(|m| m.request_focus(id));
                            }

                            let range = out.cursor_range.or_else(|| out.state.cursor.char_range());
                            let (ci, sel) = range.map_or((0, 0), |r| {
                                let cr = r.as_sorted_char_range();
                                (r.primary.index.0, cr.end.0 - cr.start.0)
                            });
                            let (line, col) = line_col(&doc.text, ci);

                            // Radnummer och markering av aktuell rad.
                            let galley = &out.galley;
                            let origin = out.galley_pos.to_vec2();
                            let clip = ui.clip_rect();
                            let painter = ui.painter();
                            let mut logical = 1usize;
                            let mut starts_line = true;
                            let mut cur: Option<Rect> = None;
                            for row in &galley.rows {
                                let r = row.rect().translate(origin);
                                let this = logical;
                                let is_start = starts_line;
                                starts_line = row.ends_with_newline;
                                if row.ends_with_newline {
                                    logical += 1;
                                }
                                if this == line {
                                    cur = Some(cur.map_or(r, |c| c.union(r)));
                                }
                                if r.bottom() < clip.top() {
                                    continue;
                                }
                                if r.top() > clip.bottom() {
                                    if this > line {
                                        break;
                                    }
                                    continue;
                                }
                                if line_numbers && is_start {
                                    let color = if this == line { p.accent } else { p.faint };
                                    painter.text(
                                        pos2(gutter.right() - 18.0, r.center().y),
                                        Align2::RIGHT_CENTER,
                                        this.to_string(),
                                        num_font.clone(),
                                        color,
                                    );
                                }
                            }
                            if let Some(c) = cur {
                                if sel == 0 {
                                    let band = Rect::from_x_y_ranges(clip.left()..=clip.right(), c.top()..=c.bottom());
                                    painter.set(bg_idx, Shape::rect_filled(band, 0.0, p.current_line));
                                }
                            }
                            if let Some(ci) = scroll_to {
                                let r = galley.pos_from_cursor(CCursor::new(ci)).translate(origin);
                                ui.scroll_to_rect(r.expand2(vec2(80.0, 60.0)), Some(Align::Center));
                            }
                            (line, col, sel)
                        })
                        .inner
                    });
                self.cursor = result.inner;
                if edited {
                    self.find.current = None;
                }
            });
    }

    fn welcome(&mut self, ui: &mut Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        let col_w = 400.0f32.min(rect.width() - 48.0);
        let top = rect.top() + (rect.height() * 0.24).max(36.0);
        let col = Rect::from_min_size(pos2(rect.center().x - col_w / 2.0, top), vec2(col_w, rect.bottom() - top));

        ui.scope_builder(UiBuilder::new().max_rect(col).layout(Layout::top_down(Align::Min)), |ui| {
            // Ordbild med blinkande markör – appens enda rörelse.
            let font = FontId::new(52.0, theme::semibold());
            let galley = ui.painter().layout_no_wrap("Kladd".into(), font, p.text);
            let (r, _) = ui.allocate_exact_size(vec2(col_w, galley.size().y + 4.0), Sense::hover());
            let gsize = galley.size();
            ui.painter().galley(r.left_top(), galley, p.text);
            let t = ctx.input(|i| i.time);
            let on = (t % 1.1) < 0.65;
            if on {
                ui.painter().rect_filled(
                    Rect::from_min_size(pos2(r.left() + gsize.x + 6.0, r.top() + 10.0), vec2(5.0, gsize.y - 16.0)),
                    2.0,
                    p.accent,
                );
            }
            let wait = if on { 0.65 - t % 1.1 } else { 1.1 - t % 1.1 };
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.01)));

            ui.add_space(6.0);
            ui.label(
                RichText::new("Skriv något nytt eller fortsätt där du var.")
                    .font(FontId::new(15.0, FontFamily::Proportional))
                    .color(p.muted),
            );
            ui.add_space(30.0);

            ui.spacing_mut().item_spacing.y = 2.0;
            if welcome_row(ui, p, "Ny flik", "Ctrl+T", None) {
                self.new_tab();
            }
            if welcome_row(ui, p, "Öppna fil…", "Ctrl+O", None) {
                self.request_open(&ctx);
            }

            if !self.settings.recent.is_empty() {
                ui.add_space(28.0);
                ui.label(RichText::new("Senast öppnade").font(FontId::new(12.5, theme::medium())).color(p.muted));
                ui.add_space(6.0);
                let mut open = None;
                for path in self.settings.recent.iter().take(6) {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if welcome_row(ui, p, &name, "", Some(&pretty_dir(path))) {
                        open = Some(path.clone());
                    }
                }
                if let Some(path) = open {
                    self.open_path(&ctx, &path);
                }
            }
        });
    }

    fn find_bar(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.find.open || self.docs.is_empty() {
            return;
        }
        let width = 430.0f32.min(self.editor_rect.width() - 24.0);
        let pos = pos2(self.editor_rect.right() - width - 22.0, self.editor_rect.top() + 12.0);
        let matches = {
            let doc = &self.docs[self.active];
            find_all(&doc.text, &self.find.query, self.find.case_sensitive)
        };
        let current_idx = self.find.current.and_then(|c| matches.iter().position(|m| m.0 == c));

        egui::Area::new(Id::unique("kladd-find"))
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
                        ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                        let mut seek = None;
                        ui.horizontal(|ui| {
                            let chevron = if self.find.replace { Icon::ChevronUp } else { Icon::ChevronDown };
                            if icons::button(ui, p, chevron, 26.0, false, "Visa ersätt", p.surface).clicked() {
                                self.find.replace = !self.find.replace;
                            }
                            let field_w = ui.available_width() - 26.0 * 4.0 - 84.0 - 4.0 * 6.0;
                            let q = ui.add(
                                TextEdit::singleline(&mut self.find.query)
                                    .id(Id::unique("kladd-find-query"))
                                    .hint_text("Sök")
                                    .desired_width(field_w)
                                    .margin(Margin::symmetric(8, 5)),
                            );
                            if self.find.focus_query {
                                q.request_focus();
                                self.find.focus_query = false;
                            }
                            if q.changed() {
                                self.find.current = None;
                                seek = Some(Seek::FromSelectionStart);
                            }
                            if q.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                let back = ui.input(|i| i.modifiers.shift);
                                seek = Some(if back { Seek::Prev } else { Seek::Next });
                                self.find.focus_query = true;
                            }
                            let count = if self.find.query.is_empty() {
                                String::new()
                            } else if matches.is_empty() {
                                "Inga träffar".to_owned()
                            } else if let Some(i) = current_idx {
                                format!("{} av {}", i + 1, matches.len())
                            } else {
                                format!("{} träffar", matches.len())
                            };
                            let (cr, _) = ui.allocate_exact_size(vec2(84.0, 26.0), Sense::hover());
                            let color = if matches.is_empty() && !self.find.query.is_empty() { p.error } else { p.muted };
                            ui.painter().text(cr.center(), Align2::CENTER_CENTER, count, FontId::new(12.0, FontFamily::Proportional), color);

                            if icons::button(ui, p, Icon::CaseSensitive, 26.0, self.find.case_sensitive, "Skilj på stora och små bokstäver", p.surface).clicked() {
                                self.find.case_sensitive = !self.find.case_sensitive;
                                self.find.current = None;
                                seek = Some(Seek::FromSelectionStart);
                            }
                            if icons::button(ui, p, Icon::ChevronUp, 26.0, false, "Föregående (Shift+F3)", p.surface).clicked() {
                                seek = Some(Seek::Prev);
                            }
                            if icons::button(ui, p, Icon::ChevronDown, 26.0, false, "Nästa (Enter eller F3)", p.surface).clicked() {
                                seek = Some(Seek::Next);
                            }
                            if icons::button(ui, p, Icon::Close, 26.0, false, "Stäng (Esc)", p.surface).clicked() {
                                self.close_find();
                            }
                        });
                        let mut do_replace = false;
                        let mut do_replace_all = false;
                        if self.find.replace {
                            ui.horizontal(|ui| {
                                ui.add_space(30.0);
                                let field_w = (ui.available_width() - 74.0 - 104.0 - 16.0).max(80.0);
                                let r = ui.add(
                                    TextEdit::singleline(&mut self.find.replacement)
                                        .id(Id::unique("kladd-find-replace"))
                                        .hint_text("Ersätt med")
                                        .desired_width(field_w)
                                        .margin(Margin::symmetric(8, 5)),
                                );
                                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                                    do_replace = true;
                                    r.request_focus();
                                }
                                ui.add_space(4.0);
                                let enabled = !matches.is_empty();
                                if ui.add_enabled(enabled, quiet_button("Ersätt", p).min_size(vec2(74.0, 28.0))).clicked() {
                                    do_replace = true;
                                }
                                if ui.add_enabled(enabled, quiet_button("Ersätt alla", p).min_size(vec2(104.0, 28.0))).clicked() {
                                    do_replace_all = true;
                                }
                            });
                        }
                        if let Some(s) = seek {
                            self.seek(ctx, s);
                        }
                        if do_replace {
                            self.replace_one(ctx);
                        }
                        if do_replace_all {
                            self.replace_all(ctx);
                        }
                    });
            });
    }

    /// Frågar innan ett Word-dokument med bilder, kommentarer m.m. skrivs över.
    fn docx_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(pending) = self.pending_docx else { return };
        let Some(idx) = self.index_of(pending.doc_id) else {
            self.pending_docx = None;
            return;
        };
        let title = self.docs[idx].title();
        let lost = join_sv(&self.docs[idx].docx_dropped);
        let mut choice = None;
        let modal = egui::Modal::new(Id::unique("kladd-docx-overwrite"))
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
                ui.set_width(400.0);
                ui.label(
                    RichText::new(format!("Spara över {title}?"))
                        .font(FontId::new(17.0, theme::semibold()))
                        .color(p.text),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(if self.docs[idx].pdf {
                        format!("Kladd skapar en ny PDF av texten, med rubriker, listor, tabeller, länkar och bilder. {} i originalet ersätts.", capitalize(&lost))
                    } else {
                        format!("Kladd sparar text, rubriker, listor, tabeller, länkar, fetstil och kursiv. {} i originalet försvinner.", capitalize(&lost))
                    })
                    .color(p.muted),
                );
                ui.add_space(22.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let over = egui::Button::new(
                        RichText::new("Spara över").font(FontId::new(13.5, theme::medium())).color(p.on_accent),
                    )
                    .fill(p.accent)
                    .stroke(Stroke::NONE)
                    .min_size(vec2(96.0, 32.0));
                    if ui.add(over).clicked() {
                        choice = Some(0);
                    }
                    if ui.add(quiet_button("Spara som ny fil…", p).min_size(vec2(0.0, 32.0))).clicked() {
                        choice = Some(1);
                    }
                    if ui.add(quiet_button("Avbryt", p).min_size(vec2(0.0, 32.0))).clicked() {
                        choice = Some(2);
                    }
                });
            });
        if modal.should_close() && choice.is_none() {
            choice = Some(2);
        }
        match choice {
            Some(0) => {
                self.pending_docx = None;
                if let Some(path) = self.docs[idx].path.clone() {
                    self.write_doc(ctx, idx, &path, pending.then_close, pending.exit);
                }
            }
            Some(1) => {
                self.pending_docx = None;
                self.request_save_as(ctx, idx, pending.then_close, pending.exit);
            }
            Some(_) => self.pending_docx = None,
            None => {}
        }
    }

    /// Frågar efter lösenord till en skyddad PDF.
    fn password_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(pending) = self.pending_password.as_mut() else { return };
        let name = pending.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut choice = None;
        let modal = egui::Modal::new(Id::unique("kladd-pdf-password"))
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
                ui.label(
                    RichText::new(format!("{name} är lösenordsskyddad"))
                        .font(FontId::new(17.0, theme::semibold()))
                        .color(p.text),
                );
                ui.add_space(6.0);
                let hint = if pending.wrong { "Lösenordet stämde inte. Försök igen." } else { "Skriv lösenordet för att öppna dokumentet." };
                ui.label(RichText::new(hint).color(if pending.wrong { p.error } else { p.muted }));
                ui.add_space(14.0);
                let field = ui.add(
                    TextEdit::singleline(&mut pending.input)
                        .password(true)
                        .hint_text("Lösenord")
                        .desired_width(f32::INFINITY)
                        .margin(Margin::symmetric(10, 7)),
                );
                if pending.focus {
                    field.request_focus();
                    pending.focus = false;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    choice = Some(0);
                }
                ui.add_space(20.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let open = egui::Button::new(
                        RichText::new("Öppna").font(FontId::new(13.5, theme::medium())).color(p.on_accent),
                    )
                    .fill(p.accent)
                    .stroke(Stroke::NONE)
                    .min_size(vec2(84.0, 32.0));
                    if ui.add(open).clicked() {
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
            Some(0) => {
                if let Some(pending) = self.pending_password.take() {
                    self.load_into_tab(ctx, &pending.path, Some(&pending.input));
                }
            }
            Some(_) => self.pending_password = None,
            None => {}
        }
    }

    fn close_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(pending) = self.pending_close else { return };
        let Some(idx) = self.index_of(pending.doc_id) else {
            self.pending_close = None;
            return;
        };
        let title = self.docs[idx].title();
        let mut choice = None;
        let modal = egui::Modal::new(Id::unique("kladd-close"))
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
                ui.label(
                    RichText::new(format!("Spara ändringarna i {title}?"))
                        .font(FontId::new(17.0, theme::semibold()))
                        .color(p.text),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Om du inte sparar försvinner ändringarna.")
                        .color(p.muted),
                );
                ui.add_space(22.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let save = egui::Button::new(
                        RichText::new("Spara").font(FontId::new(13.5, theme::medium())).color(p.on_accent),
                    )
                    .fill(p.accent)
                    .stroke(Stroke::NONE)
                    .min_size(vec2(84.0, 32.0));
                    if ui.add(save).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                        choice = Some(0);
                    }
                    if ui.add(quiet_button("Släng ändringar", p).min_size(vec2(0.0, 32.0))).clicked() {
                        choice = Some(1);
                    }
                    if ui.add(quiet_button("Avbryt", p).min_size(vec2(0.0, 32.0))).clicked() {
                        choice = Some(2);
                    }
                });
            });
        if modal.should_close() && choice.is_none() {
            choice = Some(2);
        }
        match choice {
            Some(0) => {
                self.pending_close = None;
                self.save_doc(ctx, idx, true, pending.exit);
            }
            Some(1) => {
                self.pending_close = None;
                self.close_tab_now(idx);
                if pending.exit {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Some(_) => self.pending_close = None,
            None => {}
        }
    }

    /// Utan systemets fönsterram sköter Kladd själv storleksändring från kanterna.
    fn window_edges(&self, ctx: &egui::Context, p: &Palette) {
        let (maximized, fullscreen) =
            ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
        let r = ctx.content_rect();
        if !maximized && !fullscreen {
            let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::unique("kladd-border")));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
        }
        if maximized || fullscreen || self.pending_close.is_some() {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let e = RESIZE_EDGE;
        let corner = 14.0;
        let (left, right) = (pos.x - r.left() < e, r.right() - pos.x < e);
        let (top, bottom) = (pos.y - r.top() < e, r.bottom() - pos.y < e);
        let (near_l, near_r) = (pos.x - r.left() < corner, r.right() - pos.x < corner);
        let (near_t, near_b) = (pos.y - r.top() < corner, r.bottom() - pos.y < corner);
        use egui::{CursorIcon as C, viewport::ResizeDirection as D};
        let dir = if (top && near_l) || (left && near_t) {
            Some((D::NorthWest, C::ResizeNorthWest))
        } else if (top && near_r) || (right && near_t) {
            Some((D::NorthEast, C::ResizeNorthEast))
        } else if (bottom && near_l) || (left && near_b) {
            Some((D::SouthWest, C::ResizeSouthWest))
        } else if (bottom && near_r) || (right && near_b) {
            Some((D::SouthEast, C::ResizeSouthEast))
        } else if top {
            Some((D::North, C::ResizeNorth))
        } else if bottom {
            Some((D::South, C::ResizeSouth))
        } else if left {
            Some((D::West, C::ResizeWest))
        } else if right {
            Some((D::East, C::ResizeEast))
        } else {
            None
        };
        if let Some((d, cursor)) = dir {
            ctx.set_cursor_icon(cursor);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
            }
        }
    }

    fn drop_hint(&self, ctx: &egui::Context, p: &Palette) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::unique("kladd-drop")));
        let r = ctx.content_rect().shrink(14.0);
        painter.rect_filled(r, 16.0, p.chrome.gamma_multiply(0.92));
        painter.rect_stroke(r, 16.0, Stroke::new(2.0, p.accent), egui::StrokeKind::Inside);
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            "Släpp för att öppna",
            FontId::new(20.0, theme::medium()),
            p.text,
        );
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

pub(crate) fn quiet_button(text: &str, p: &Palette) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(p.text))
        .fill(p.surface_hover)
        .stroke(Stroke::NONE)
}

pub(crate) fn menu_divider(ui: &mut Ui, p: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 11.0), Sense::hover());
    ui.painter().hline(r.x_range().shrink(6.0), r.center().y, Stroke::new(1.0, p.line));
}

/// En menyrad: valfri bock, etikett och kortkommando.
pub(crate) fn menu_item(ui: &mut Ui, p: &Palette, label: &str, shortcut: &str, checked: Option<bool>, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), if enabled { Sense::click() } else { Sense::hover() });
    if !ui.is_rect_visible(rect) {
        return false;
    }
    let painter = ui.painter();
    if resp.hovered() && enabled {
        painter.rect_filled(rect, 6.0, p.surface_hover);
    }
    let text_color = if enabled { p.text } else { p.faint };
    let label_x = rect.left() + 30.0;
    if checked == Some(true) {
        let c = pos2(rect.left() + 15.0, rect.center().y);
        let s = Stroke::new(1.6, p.accent);
        painter.line_segment([c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.2)], s);
        painter.line_segment([c + vec2(-1.0, 3.2), c + vec2(4.5, -3.5)], s);
    }
    painter.text(pos2(label_x, rect.center().y), Align2::LEFT_CENTER, label, FontId::new(13.5, FontFamily::Proportional), text_color);
    if !shortcut.is_empty() {
        painter.text(
            pos2(rect.right() - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            shortcut,
            FontId::new(12.0, FontFamily::Proportional),
            if enabled { p.muted } else { p.faint },
        );
    }
    let clicked = enabled && resp.clicked();
    if clicked {
        ui.close();
    }
    clicked
}

pub(crate) fn status_button(ui: &mut Ui, p: &Palette, text: &str, tooltip: &str) -> Response {
    let font = FontId::new(12.0, FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, p.muted);
    let size = vec2(galley.size().x + 14.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_text(tooltip);
    let color = if resp.hovered() { p.text } else { p.muted };
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, p.surface_hover);
    }
    ui.painter().galley(
        pos2(rect.left() + 7.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    resp
}

/// Rad på startskärmen: etikett till vänster, kortkommando eller mapp till höger.
fn welcome_row(ui: &mut Ui, p: &Palette, label: &str, shortcut: &str, detail: Option<&str>) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    let painter = ui.painter();
    let hovered = resp.hovered();
    if hovered {
        painter.rect_filled(rect.expand2(vec2(10.0, 0.0)), 8.0, p.surface);
    }
    if resp.has_focus() {
        painter.rect_stroke(rect.expand2(vec2(10.0, 0.0)), 8.0, Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
    }
    let label_color = if hovered { p.accent } else { p.text };
    let galley = painter.layout_no_wrap(truncate(label, 34), FontId::new(14.5, theme::medium()), label_color);
    let label_w = galley.size().x;
    painter.galley(pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, label_color);
    let right = if shortcut.is_empty() { detail.unwrap_or("") } else { shortcut };
    if !right.is_empty() {
        let max_chars = ((rect.width() - label_w - 24.0) / 7.0).max(6.0) as usize;
        painter.text(
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            truncate(right, max_chars),
            FontId::new(12.5, FontFamily::Proportional),
            p.muted,
        );
    }
    resp.clicked()
}

impl eframe::App for KladdApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Körs även när fönstret är minimerat, så att osparade ändringar aldrig tappas.
        self.handle_exit(ctx);
        self.handle_incoming(ctx);
        if self.docs.get(self.active).map(|d| d.id) != self.last_active_id {
            self.last_active_id = self.docs.get(self.active).map(|d| d.id);
            self.find.current = None;
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = self.palette();

        self.handle_dialogs(&ctx);
        self.handle_drops(&ctx);
        self.handle_shortcuts(&ctx);

        self.top_bar(ui, &p);
        self.word_bar(ui, &p);
        self.status_bar(ui, &p);
        self.preview(ui, &p);
        self.editor(ui, &p);
        self.find_bar(&ctx, &p);
        self.close_dialog(&ctx, &p);
        self.docx_dialog(&ctx, &p);
        self.password_dialog(&ctx, &p);
        self.drop_hint(&ctx, &p);
        self.window_edges(&ctx, &p);
        self.update_title(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.owns_socket {
            single::cleanup();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }
}
