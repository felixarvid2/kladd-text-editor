//! Kladds fönster: flikar, editor, sök, förhandsvisning och dialoger.

use std::path::{Path, PathBuf};
use std::sync::Arc;
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

use crate::panes::{self, Orient};
use crate::doc::{Document, LineEnding, LoadError, TEXT_EXTENSIONS, TextEncoding, join_sv};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp"];

fn is_image_path(p: &Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()) || e == "webp")
}
use crate::highlight::{Highlighter, Params};
use crate::pdfview::chrome::{self as pdfui, Action as PdfAction};
use crate::pdfview::engine::{OpenError, PdfEngine};
use crate::pdfview::pages::{self as pdfpages, OrgOp as PdfOrgOp};
use crate::pdfview::view::{PdfView, ViewMemory};
use crate::search::{self, Apply, DocReplace, FormatFilter, Match, Results};
use crate::word::markup;
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
const PANE_HEAD_H: f32 = 24.0;

/// Vad en ruta i delad vy rapporterar tillbaka.
#[derive(Default)]
struct PaneOut {
    clicked: bool,
    pick: Option<usize>,
    close: bool,
}

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
    /// Grammatik- och stilkontroll (LanguageTool) i dokumentläget. Av som standard.
    pub grammar: bool,
    /// "public" (api.languagetool.org) eller adressen till en egen server.
    pub grammar_server: String,
    /// Att texten skickas till publika servern är godkänt (förklaringen visad en gång).
    pub grammar_ack: bool,
    /// Säkerhetskopiera osparade flikar regelbundet så att de kan återställas efter en krasch.
    pub autosave_recovery: bool,
    /// Navigeringsfönstret (rubrikträd) för Markdown-baserade flikar.
    pub nav_pane: bool,
    /// Visningsläget (sida, zoom, rotation) för senast öppnade PDF-filer, nyast först.
    pub pdf_memory: Vec<(String, ViewMemory)>,
    /// Sök efter nya versioner av Kladd (högst en gång per dygn).
    pub auto_update_check: bool,
    /// Senaste lyckade kontroll, sekunder sedan 1970.
    pub update_last_check: u64,
    /// Version som användaren valt att hoppa över.
    pub update_skip: Option<String>,
    /// Spara ändrade flikar med sökväg automatiskt när skrivandet stannat (av som standard).
    pub autosave: bool,
    /// Första rutans andel av ytan i delad vy.
    pub split_ratio: f32,
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
            grammar: false,
            grammar_server: crate::grammar::PUBLIC_SETTING.to_owned(),
            grammar_ack: false,
            autosave_recovery: true,
            nav_pane: false,
            pdf_memory: Vec::new(),
            auto_update_check: true,
            update_last_check: 0,
            update_skip: None,
            autosave: false,
            split_ratio: 0.5,
        }
    }
}

enum DialogResult {
    Open(Vec<PathBuf>),
    InsertImage { doc_id: u64, path: Option<PathBuf> },
    SaveAs { doc_id: u64, path: Option<PathBuf>, then_close: bool, exit: bool },
    /// "Spara kopia" av en PDF som visas i PDF-visaren.
    SaveCopy { doc_id: u64, path: Option<PathBuf> },
    /// "Extrahera till ny PDF" av valda sidor.
    PdfExtract { doc_id: u64, path: Option<PathBuf>, pages: Vec<usize> },
    /// "Infoga PDF" / "Slå ihop med PDF": vald fil och plats.
    PdfInsert { doc_id: u64, path: Option<PathBuf>, at: usize },
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
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
    /// Formatfilter (bara dokumentläget).
    pub(crate) fmt: FormatFilter,
    /// Formatering som "Ersätt" lägger på träffen (bara dokumentläget).
    pub(crate) apply: Option<Apply>,
    /// Bara formatera träffarna och lämna texten orörd.
    pub(crate) keep_text: bool,
    /// Byte-start för den aktuella träffen.
    pub(crate) current: Option<usize>,
    pub(crate) focus_query: bool,
    /// Senaste sökresultatet med nyckel (flik, text, inställningar).
    cache: Option<(u64, Arc<Results>)>,
}

impl Find {
    fn spec(&self, word: bool) -> search::Spec {
        search::Spec {
            query: self.query.clone(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
            fmt: if word { self.fmt.clone() } else { FormatFilter::default() },
        }
    }
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
    recovery: crate::recovery::Recovery,
    recovery_prompt: Option<crate::recovery::Prompt>,
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
    autosave: crate::autosave::Autosave,
    history: Option<crate::history_ui::Dialog>,
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
    pub(crate) grammar: crate::grammar::Checker,
    pub(crate) grammar_dlg: Option<crate::grammar_ui::Dialog>,
    pub(crate) wv: crate::word::view::State,
    pub(crate) print: crate::print::PrintState,
    pub(crate) nav: crate::word::navui::State,
    /// Fönstret är i helskärm för presentationsläget i en PDF-flik.
    pdf_fullscreen: bool,
    update: crate::update::State,
    /// Delad vy: vilka flikar som visas i de två rutorna.
    pub(crate) panes: crate::panes::Panes,
    /// Extra markörer och kolumnmarkering i textläget.
    mc: crate::multicursor::State,
}

pub(crate) fn editor_id(doc_id: u64) -> Id {
    Id::unique(("kladd-editor", doc_id, crate::panes::salt()))
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
    crate::platform::home_dir()
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

        let recovery = crate::recovery::Recovery::new();
        let orphans = crate::recovery::find_orphans(recovery.dir(), &crate::recovery::pid_alive);
        let mut app = Self {
            recovery,
            recovery_prompt: crate::recovery::Prompt::new(orphans),
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
            autosave: Default::default(),
            history: None,
            pending_password: None,
            allow_exit: false,
            toast: None,
            cursor: (1, 1, 0),
            scroll_to_char: None,
            focus_editor: false,
            editor_rect: Rect::NOTHING,
            mc: Default::default(),
            last_title: String::new(),
            last_active_id: None,
            incoming: None,
            preview_images: std::collections::HashSet::new(),
            owns_socket: false,
            word: Default::default(),
            doc_fonts: crate::word::fonts::DocFonts::new(),
            speller: None,
            grammar: Default::default(),
            grammar_dlg: None,
            wv: Default::default(),
            print: Default::default(),
            nav: Default::default(),
            pdf_fullscreen: false,
            update: Default::default(),
            panes: Default::default(),
        };
        let (tx, rx) = channel();
        app.owns_socket = single_instance && single::listen(tx.clone(), ctx.clone());
        // macOS levererar filer från Finder som Apple-händelser till den här processen.
        #[cfg(target_os = "macos")]
        crate::macopen::install(tx, ctx.clone());
        if app.owns_socket || cfg!(target_os = "macos") {
            app.incoming = Some(rx);
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
        if looks_like_pdf(&canonical) {
            self.open_pdf_tab(ctx, &canonical, password);
            return;
        }
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
            Ok(doc) => self.install_doc(ctx, doc, &canonical),
            Err(e) => {
                self.settings.recent.retain(|p| p != &canonical);
                self.notify(ctx, e, true);
            }
        }
    }

    /// Lägger det inlästa dokumentet i en ny flik (eller i en tom, orörd namnlös flik).
    fn install_doc(&mut self, ctx: &egui::Context, doc: Document, canonical: &Path) {
        self.next_id += 1;
        // Ersätt en tom, orörd namnlös flik i stället för att lägga till en ny.
        let replace = self
            .docs
            .get(self.active)
            .is_some_and(|d| d.path.is_none() && !d.dirty && d.text.is_empty());
        if replace {
            let old = self.docs[self.active].id;
            self.hl.forget(old);
            self.hl.forget(old | (1 << 62));
            self.word.forget(old);
            self.panes.forget_doc(old, Some(doc.id));
            self.docs[self.active] = doc;
        } else {
            self.docs.push(doc);
            self.active = self.docs.len() - 1;
        }
        self.remember(canonical);
        self.autosave.record_stamp(self.docs[self.active].id, canonical);
        self.focus_editor = true;
        if let Some(note) = self.docs[self.active].open_note.take() {
            self.notify(ctx, note, false);
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
                .add_filter("OpenDocument-text", &["odt", "ott"])
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
            let ext = if doc.extension() == "ott" { "odt" } else { "docx" };
            format!("{}.{ext}", stem.unwrap_or_else(|| doc.title()))
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
                .add_filter("OpenDocument-text (.odt)", &["odt"])
                .add_filter("PDF", &["pdf"])
                .add_filter("Webbsida (.html)", &["html"])
                .add_filter("E-bok (.epub)", &["epub"]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let path = dlg.save_file();
            let _ = tx.send(DialogResult::SaveAs { doc_id, path, then_close, exit });
            ctx.request_repaint();
        });
    }

    pub(crate) fn request_insert_image(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.docs.is_empty() || self.pdf_mode() {
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
        if doc.pdfv.is_some() {
            self.pdf_save(ctx, idx, then_close, exit);
            return;
        }
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

    /// Efter varje lyckad sparning av flikens egen fil: minns filens stämpel och lagrar en version.
    fn after_save(&mut self, idx: usize, path: &Path) {
        let Some(doc) = self.docs.get(idx) else { return };
        self.autosave.record_stamp(doc.id, path);
        if let Ok(bytes) = std::fs::read(path) {
            let _ = crate::history::record(&crate::history::root(), path, &bytes, crate::history::now_ms(), &Default::default());
        }
    }

    /// Automatisk sparning (när inställningen är på): sparar ändrade flikar som reglerna tillåter.
    fn autosave_tick(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let steps = self.autosave.poll(&self.docs, self.active, Self::now(ctx), focused, self.settings.autosave);
        for step in steps {
            match step {
                crate::autosave::Step::Notice(text) => self.notify(ctx, text, true),
                crate::autosave::Step::Save(id) => {
                    let Some(idx) = self.index_of(id) else { continue };
                    let Some(path) = self.docs[idx].path.clone() else { continue };
                    match self.docs[idx].save_to(&path) {
                        Ok(warnings) => {
                            self.after_save(idx, &path);
                            if warnings.is_empty() {
                                self.notify(ctx, "Sparat automatiskt", false);
                            } else {
                                let list: Vec<&str> = warnings.iter().map(String::as_str).collect();
                                let what = if list.len() == 1 { "bilden" } else { "bilderna" };
                                self.notify(ctx, format!("Sparat automatiskt, men hittade inte {what} {}", join_sv(&list)), true);
                            }
                        }
                        Err(e) => {
                            self.autosave.mark_failed(id, &self.docs[idx].text);
                            self.notify(ctx, format!("Automatisk sparning misslyckades: {e}"), true);
                        }
                    }
                }
            }
        }
        if self.autosave.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    fn open_history(&mut self, ctx: &egui::Context) {
        let Some(doc) = self.docs.get(self.active) else { return };
        match doc.path.clone() {
            Some(path) => self.history = Some(crate::history_ui::Dialog::new(doc, &path)),
            None => self.notify(ctx, "Spara filen först, så börjar historiken.", false),
        }
    }

    fn history_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        use crate::history_ui::Action;
        let Some(mut dlg) = self.history.take() else { return };
        let action = dlg.show(ctx, p, crate::history::now_ms());
        let (ts, restore) = match action {
            Action::None => {
                self.history = Some(dlg);
                return;
            }
            Action::Close => return,
            Action::Restore(ts) => (ts, true),
            Action::OpenNew(ts) => (ts, false),
        };
        let loaded = crate::history::read(&crate::history::root(), &dlg.path, ts)
            .map_err(|e| format!("Versionen kunde inte läsas: {e}"))
            .and_then(|bytes| Document::from_version(self.next_id, &dlg.path, &bytes));
        let version = match loaded {
            Ok(v) => v,
            Err(e) => {
                self.notify(ctx, e, true);
                self.history = Some(dlg);
                return;
            }
        };
        if restore {
            let Some(idx) = self.index_of(dlg.doc_id) else { return };
            self.active = idx;
            crate::history_ui::restore_into(ctx, &mut self.docs[idx], version);
            let id = self.docs[idx].id;
            self.hl.forget(id);
            self.word.forget(id);
            self.find.current = None;
            self.focus_editor = true;
            self.notify(ctx, "Återställde versionen (ångra med Ctrl+Z)", false);
        } else {
            self.next_id += 1;
            self.docs.push(version);
            self.active = self.docs.len() - 1;
            self.focus_editor = true;
        }
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
        let export = doc.is_export_target(path);
        match doc.save_to(path) {
            Ok(warnings) if export => {
                // Export: fliken är kvar som den var (och ändrad), så den stängs inte här.
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if warnings.is_empty() {
                    self.notify(ctx, format!("Exporterade {name}"), false);
                } else {
                    let list: Vec<&str> = warnings.iter().map(String::as_str).collect();
                    let what = if list.len() == 1 { "bilden" } else { "bilderna" };
                    self.notify(ctx, format!("Exporterade {name}, men hittade inte {what} {}", join_sv(&list)), true);
                }
            }
            Ok(warnings) => {
                let title = doc.title();
                if was_md != doc.is_markdown() || doc.extension().is_empty() {
                    self.hl.forget(doc.id);
                }
                self.remember(path);
                self.after_save(idx, path);
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
        self.store_pdf_memory(idx);
        let doc = self.docs.remove(idx);
        self.autosave.forget(doc.id);
        self.recovery.forget(doc.id);
        self.hl.forget(doc.id);
        self.hl.forget(doc.id | (1 << 62));
        self.word.forget(doc.id);
        if self.docs.is_empty() {
            self.active = 0;
            self.untitled_counter = 0;
            self.find.open = false;
        } else if self.active > idx || self.active >= self.docs.len() {
            self.active = self.active.saturating_sub(1);
        }
        // Rutor som visade fliken faller tillbaka på den aktiva fliken.
        self.panes.forget_doc(doc.id, self.docs.get(self.active).map(|d| d.id));
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
        if let Some(tab) = self.pdf_tab_mut() {
            tab.open_find();
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

    /// Träffarna för den aktiva fliken (dokumentläget söker i den synliga texten). Cachas tills
    /// texten eller sökinställningarna ändras.
    pub(crate) fn search_results(&mut self) -> Arc<Results> {
        use std::hash::{Hash, Hasher};
        if !self.find.open {
            return Arc::default();
        }
        let Some(doc) = self.docs.get(self.active) else { return Arc::default() };
        let word = doc.word_view;
        let spec = self.find.spec(word);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (doc.id, word).hash(&mut h);
        crate::word::layout::fast_hash(&doc.text).hash(&mut h);
        spec.hash(&mut h);
        let key = h.finish();
        if let Some((k, r)) = &self.find.cache {
            if *k == key {
                return r.clone();
            }
        }
        let r = Arc::new(if word {
            let a = self.word.analysis(doc.id, &doc.text);
            search::search_doc(&doc.text, &a, &spec)
        } else {
            search::search_text(&doc.text, &spec)
        });
        self.find.cache = Some((key, r.clone()));
        r
    }

    /// Bakgrundsintervall för editorns layout (tomma när sökningen är stängd).
    pub(crate) fn search_highlights(&mut self) -> Vec<(usize, usize, bool)> {
        if !self.find.open {
            return Vec::new();
        }
        let r = self.search_results();
        search::highlights(&r, self.find.current)
    }

    /// Markerar träffen i editorn och rullar dit.
    fn select_match(&mut self, ctx: &egui::Context, m: &Match) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let (ca, cb) = (byte_to_char(&doc.text, m.start), byte_to_char(&doc.text, m.end));
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(ca), CCursor::new(cb))));
        state.store(ctx, id);
        self.find.current = Some(m.start);
        self.scroll_to_char = Some(ca);
    }

    fn seek(&mut self, ctx: &egui::Context, how: Seek) {
        let results = self.search_results();
        let matches = &results.matches;
        let Some(doc) = self.docs.get(self.active) else { return };
        if matches.is_empty() {
            self.find.current = None;
            return;
        }
        let id = editor_id(doc.id);
        let state = TextEditState::load(ctx, id).unwrap_or_default();
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
            Seek::FromSelectionStart => matches.iter().find(|m| m.start >= ba).or(matches.first()),
            Seek::Next => {
                let from = if self.find.current == Some(ba) { bb.max(ba + 1) } else { ba };
                matches.iter().find(|m| m.start >= from).or(matches.first())
            }
            Seek::Prev => matches.iter().rev().find(|m| m.start < ba).or(matches.last()),
        };
        let m = pick.expect("minst en träff").clone();
        self.select_match(ctx, &m);
    }

    /// Hur "Ersätt" ska formatera: bara formatering när "Behåll text" är ikryssad.
    fn doc_replace_opts<'a>(&self, only: Option<usize>, repl: &'a str) -> DocReplace<'a> {
        DocReplace { repl, keep_text: self.find.apply.is_some() && self.find.keep_text, apply: self.find.apply, only }
    }

    fn replace_one(&mut self, ctx: &egui::Context) {
        let results = self.search_results();
        let Some(cur) = self.find.current else {
            self.seek(ctx, Seek::FromSelectionStart);
            return;
        };
        let Some(idx) = results.matches.iter().position(|m| m.start == cur) else {
            self.seek(ctx, Seek::FromSelectionStart);
            return;
        };
        let repl = self.find.replacement.clone();
        if self.word_mode() {
            let o = self.doc_replace_opts(Some(cur), &repl);
            let keep = o.keep_text;
            let spec = self.find.spec(true);
            self.word_edit(ctx, |t, _| {
                search::replace_doc(t, &spec, &o).map(|r| r.0).unwrap_or(markup::Edit { text: t.to_owned(), sel: 0..0 })
            });
            // word_edit tar fokus till editorn; sökfältet ska behålla den.
            self.focus_editor = false;
            if keep {
                // Texten är kvar och matchar fortfarande: gå vidare till nästa träff.
                let after = self.search_results();
                let next = after.matches.get(idx + 1).or(after.matches.first()).cloned();
                if let Some(m) = next {
                    self.select_match(ctx, &m);
                }
                return;
            }
        } else {
            let spec = self.find.spec(false);
            let Some(doc) = self.docs.get_mut(self.active) else { return };
            if let Some(r) = search::replace_text(&doc.text, &spec, &repl, Some(cur)) {
                doc.text = r.text;
                doc.on_edited();
                let id = editor_id(doc.id);
                let after = byte_to_char(&doc.text, r.caret);
                let mut state = TextEditState::load(ctx, id).unwrap_or_default();
                state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(after))));
                state.store(ctx, id);
            }
            self.find.current = None;
        }
        self.seek(ctx, Seek::FromSelectionStart);
    }

    fn replace_all(&mut self, ctx: &egui::Context) {
        if self.search_results().matches.is_empty() {
            return;
        }
        let repl = self.find.replacement.clone();
        let n;
        let keep = self.find.apply.is_some() && self.find.keep_text && self.word_mode();
        if self.word_mode() {
            let o = self.doc_replace_opts(None, &repl);
            let spec = self.find.spec(true);
            let mut count = 0;
            // Ett enda ångra-steg för alla träffar.
            self.word_edit(ctx, |t, _| match search::replace_doc(t, &spec, &o) {
                Some((e, c)) => {
                    count = c;
                    e
                }
                None => markup::Edit { text: t.to_owned(), sel: 0..0 },
            });
            self.focus_editor = false;
            n = count;
        } else {
            let spec = self.find.spec(false);
            let Some(doc) = self.docs.get_mut(self.active) else { return };
            let Some(r) = search::replace_text(&doc.text, &spec, &repl, None) else { return };
            doc.text = r.text;
            doc.on_edited();
            n = r.count;
        }
        self.find.current = None;
        let msg = match (keep, n) {
            (true, 1) => "Formaterade 1 träff".to_owned(),
            (true, n) => format!("Formaterade {n} träffar"),
            (false, 1) => "Ersatte 1 träff".to_owned(),
            (false, n) => format!("Ersatte {n} träffar"),
        };
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
            DialogResult::SaveCopy { doc_id, path } => {
                if let Some(path) = path {
                    self.write_pdf_copy(ctx, doc_id, &path);
                }
            }
            DialogResult::PdfExtract { doc_id, path, pages } => {
                if let Some(path) = path {
                    self.write_pdf_extract(ctx, doc_id, &path, &pages);
                }
            }
            DialogResult::PdfInsert { doc_id, path, at } => {
                if let Some(path) = path {
                    self.insert_pdf_file(ctx, doc_id, &path, at);
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
            if is_image_path(&p) && !self.docs.is_empty() && !self.pdf_mode() {
                self.insert_image(ctx, &p);
            } else {
                self.open_path(ctx, &p);
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.pending_close.is_some() || self.history.is_some() || self.pending_docx.is_some() || self.pending_password.is_some() || self.print.is_open() || self.wv.page_dlg.is_some() || self.wv.hf_dlg.is_some() || self.docs.get(self.active).and_then(|d| d.pdfv.as_ref()).is_some_and(|t| t.dialog_open()) {
            return;
        }
        // COMMAND = ⌘ på Mac, Ctrl annars.
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        // Flikbyte med Ctrl+Tab är Ctrl även på Mac.
        let tab_ctrl = Modifiers::CTRL;
        let tab_ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let hit = |m: Modifiers, k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
        };
        // I presentationsläget är det bara visaren som får tangenter.
        if self.pdf_presenting() {
            self.pdf_shortcuts(ctx);
            return;
        }
        let pdf = self.pdf_mode();

        if hit(ctrl, Key::T) || hit(ctrl, Key::N) {
            self.new_tab();
        }
        // Ctrl+Skift+\ före Ctrl+\: `consume_shortcut` bryr sig inte om extra Skift.
        if hit(ctrl_shift, Key::Backslash) || hit(ctrl_shift, Key::Pipe) {
            self.toggle_split(Orient::Horizontal);
        } else if hit(ctrl, Key::Backslash) {
            self.toggle_split(Orient::Vertical);
        }
        if hit(Modifiers::NONE, Key::F6) && self.panes.is_split() {
            self.focus_pane(1 - self.panes.active);
        }
        if hit(ctrl, Key::P) {
            self.open_print(ctx);
        }
        // Ctrl+Skift+O före Ctrl+O: `consume_shortcut` bryr sig inte om extra Skift.
        if hit(ctrl_shift, Key::O) {
            self.settings.nav_pane = !self.settings.nav_pane;
        } else if hit(ctrl, Key::O) {
            self.request_open(ctx);
        }
        if hit(ctrl_shift, Key::S) {
            if pdf {
                self.request_save_copy(ctx, self.active);
            } else if !self.docs.is_empty() {
                self.request_save_as(ctx, self.active, false, false);
            }
        } else if hit(ctrl, Key::S) && !self.docs.is_empty() {
            // En PDF i visaren sparas bara över när sidor ändrats; annars ger Ctrl+S en kopia av originalet.
            if pdf {
                if self.pdf_unsaved() {
                    self.pdf_save(ctx, self.active, false, false);
                } else {
                    self.request_save_copy(ctx, self.active);
                }
            } else {
                self.save_doc(ctx, self.active, false, false);
            }
        }
        if hit(ctrl, Key::W) && !self.docs.is_empty() {
            self.request_close_tab(self.active);
        }
        if hit(tab_ctrl_shift, Key::Tab) || hit(tab_ctrl, Key::PageUp) {
            if !self.docs.is_empty() {
                self.active = (self.active + self.docs.len() - 1) % self.docs.len();
                self.focus_editor = true;
            }
        } else if (hit(tab_ctrl, Key::Tab) || hit(tab_ctrl, Key::PageDown)) && !self.docs.is_empty() {
            self.active = (self.active + 1) % self.docs.len();
            self.focus_editor = true;
        }
        if hit(ctrl_shift, Key::I) && !self.docs.is_empty() && !pdf {
            self.request_insert_image(ctx);
        }
        if hit(ctrl, Key::F) {
            self.open_find(ctx, false);
        }
        if hit(ctrl, Key::H) && !pdf {
            self.open_find(ctx, true);
        }
        if pdf {
            self.pdf_shortcuts(ctx);
        } else if self.find.open {
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
        // Zoom, radbrytning och förhandsvisning hör till redigeraren; i PDF-visaren sköter vyn zoomen.
        if pdf {
            if hit(ctrl_shift, Key::K) || hit(ctrl_shift, Key::L) {
                self.toggle_theme(ctx);
            }
            if hit(ctrl, Key::Q) {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            return;
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
        // Ctrl+Skift+L är "markera alla förekomster" i textläget; temat byts där med Ctrl+Skift+K.
        if hit(ctrl_shift, Key::K) || (word && hit(ctrl_shift, Key::L)) {
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
                let show_nav_btn = self.docs.get(self.active).is_some_and(Document::is_markdown);
                let buttons = 2 + usize::from(show_preview_btn) + usize::from(show_nav_btn);
                let right_w = buttons as f32 * 34.0 + WINDOW_CONTROLS_W;
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
                            (Icon::Sun, "Byt till ljust tema (Ctrl+Shift+K)")
                        } else {
                            (Icon::Moon, "Byt till mörkt tema (Ctrl+Shift+K)")
                        };
                        if icons::button(ui, p, icon, 30.0, false, tip, p.chrome).clicked() {
                            self.toggle_theme(&ctx);
                        }
                        if show_nav_btn {
                            let tip = if self.settings.nav_pane {
                                "Dölj navigeringsfönstret (Ctrl+Shift+O)"
                            } else {
                                "Visa navigeringsfönstret (Ctrl+Shift+O)"
                            };
                            if icons::button(ui, p, Icon::Outline, 30.0, self.settings.nav_pane, tip, p.chrome).clicked() {
                                self.settings.nav_pane = !self.settings.nav_pane;
                            }
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
        let pdf = self.pdf_mode();
        if menu_item(ui, p, "Ny flik", "Ctrl+T", None, true) {
            self.new_tab();
        }
        if menu_item(ui, p, "Öppna…", "Ctrl+O", None, true) {
            self.request_open(ctx);
        }
        let pdf_changed = self.pdf_unsaved();
        if menu_item(ui, p, if pdf && !pdf_changed { "Spara kopia…" } else { "Spara" }, "Ctrl+S", None, has_doc) {
            if pdf && pdf_changed {
                self.pdf_save(ctx, self.active, false, false);
            } else if pdf {
                self.request_save_copy(ctx, self.active);
            } else {
                self.save_doc(ctx, self.active, false, false);
            }
        }
        if pdf && pdf_changed && menu_item(ui, p, "Spara kopia…", "Ctrl+Shift+S", None, true) {
            self.request_save_copy(ctx, self.active);
        }
        if menu_item(ui, p, "Spara som…", "Ctrl+Shift+S", None, has_doc && !pdf) {
            self.request_save_as(ctx, self.active, false, false);
        }
        if menu_item(ui, p, "Skriv ut…", "Ctrl+P", None, has_doc) {
            self.open_print(ctx);
        }
        let hist = self.docs.get(self.active).is_some_and(|d| d.path.is_some() && d.pdfv.is_none());
        if menu_item(ui, p, "Versionshistorik…", "", None, hist) {
            self.open_history(ctx);
        }
        if menu_item(ui, p, "Stäng flik", "Ctrl+W", None, has_doc) {
            self.request_close_tab(self.active);
        }
        menu_divider(ui, p);
        if menu_item(ui, p, "Infoga bild…", "Ctrl+Shift+I", None, has_doc && !pdf) {
            self.request_insert_image(ctx);
        }
        if menu_item(ui, p, "Infoga innehållsförteckning", "", None, self.docs.get(self.active).is_some_and(Document::is_markdown)) {
            self.insert_toc(ctx);
        }
        if menu_item(ui, p, "Sök", "Ctrl+F", None, has_doc) {
            self.open_find(ctx, false);
        }
        if menu_item(ui, p, "Sök och ersätt", "Ctrl+H", None, has_doc && !pdf) {
            self.open_find(ctx, true);
        }
        menu_divider(ui, p);
        let split = self.panes.orient;
        if menu_item(ui, p, "Dela vyn", "Ctrl+\\", Some(split == Some(Orient::Vertical)), has_doc) {
            self.toggle_split(Orient::Vertical);
        }
        if menu_item(ui, p, "Dela vågrätt", "Ctrl+Shift+\\", Some(split == Some(Orient::Horizontal)), has_doc) {
            self.toggle_split(Orient::Horizontal);
        }
        menu_divider(ui, p);
        if menu_item(ui, p, "Radbrytning", "Alt+Z", Some(self.settings.wrap), true) {
            self.settings.wrap = !self.settings.wrap;
        }
        if menu_item(ui, p, "Radnummer", "", Some(self.settings.line_numbers), true) {
            self.settings.line_numbers = !self.settings.line_numbers;
        }
        let md = self.docs.get(self.active).is_some_and(Document::is_markdown);
        if menu_item(ui, p, "Navigeringsfönster", "Ctrl+Shift+O", Some(self.settings.nav_pane), md) {
            self.settings.nav_pane = !self.settings.nav_pane;
        }
        if menu_item(ui, p, "Förhandsvisa Markdown", "Ctrl+Shift+M", Some(self.settings.preview), true) {
            self.settings.preview = !self.settings.preview;
        }
        if menu_item(ui, p, "Mörkt tema", "Ctrl+Shift+K", Some(self.settings.dark), true) {
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
        if menu_item(ui, p, "Säkerhetskopiera osparade ändringar", "", Some(self.settings.autosave_recovery), true) {
            self.settings.autosave_recovery = !self.settings.autosave_recovery;
        }
        if menu_item(ui, p, "Sök efter uppdateringar automatiskt", "", Some(self.settings.auto_update_check), true) {
            self.settings.auto_update_check = !self.settings.auto_update_check;
        }
        if menu_item(ui, p, "Sök efter uppdateringar", "", None, true) {
            self.update.check_now(ctx);
        }
        if menu_item(ui, p, "Spara automatiskt", "", Some(self.settings.autosave), true) {
            self.settings.autosave = !self.settings.autosave;
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
                        let lang = doc.language();
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
                    if let Some(tab) = self.docs.get(self.active).and_then(|d| d.pdfv.as_deref()) {
                        pdfui::status(ui, p, &font, tab);
                        return;
                    }
                    let Some(doc) = self.docs.get_mut(self.active) else { return };
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if doc.docx || doc.pdf {
                            let (label, tip) = if doc.pdf {
                                ("PDF-dokument", "Texten visas som Markdown och sparas som en ny PDF")
                            } else if doc.language() == "OpenDocument" {
                                ("OpenDocument-text", "Visas som Markdown och sparas som .odt")
                            } else {
                                ("Word-dokument", "Visas som Markdown och sparas som .docx")
                            };
                            if doc.docx || doc.pdf {
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
        // I delad vy ryms ingen förhandsvisning bredvid två rutor.
        if self.panes.is_split() || !(self.settings.preview && doc.is_markdown()) || self.word_mode() {
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
        if self.pdf_mode() {
            self.pdf_page(ui, p);
            return;
        }
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
                let matches = self.search_highlights();
                let scroll_to = self.scroll_to_char.take();
                let focus = std::mem::take(&mut self.focus_editor);

                let doc = &mut self.docs[self.active];
                let hl = &mut self.hl;
                let mc = &mut self.mc;
                let hint = doc.syntax_hint();
                let doc_id = doc.id;
                let id = editor_id(doc_id);
                // Egen färgläggningscache per ruta, så att två vyer av samma text inte slår ut varandras.
                let hl_id = doc_id | ((crate::panes::salt() as u64) << 62);

                let mut edited = false;
                let mut scroll_to = scroll_to;
                let had_focus = ui.memory(|m| m.has_focus(id));
                let pre = mc.pre(ui.ctx(), id, doc_id, &mut doc.text, had_focus, self.editor_rect);
                if pre.edited {
                    doc.on_edited();
                    edited = true;
                }
                scroll_to = scroll_to.or(pre.scroll_to);
                let scroll = if wrap { ScrollArea::vertical() } else { ScrollArea::both() };
                let result = scroll
                    .id_salt(("kladd-scroll", doc_id, crate::panes::salt()))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let viewport_h = ui.clip_rect().height();
                        let num_font = FontId::new((font_size * 0.82).round(), FontFamily::Monospace);
                        let line_params = Params { doc_id: hl_id, syntax_hint: &hint, palette: p, font_size, matches: &matches };
                        let lines = hl.newlines(&doc.text, &line_params) + 1;
                        let digits = lines.to_string().len().max(2) as f32;
                        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&num_font, '0'));
                        let gutter_w = if line_numbers { digits * char_w + 40.0 } else { 22.0 };

                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let (gutter, _) =
                                ui.allocate_exact_size(vec2(gutter_w, viewport_h), Sense::hover());
                            let bg_idx = ui.painter().add(Shape::Noop);

                            let params = Params {
                                doc_id: hl_id,
                                syntax_hint: &hint,
                                palette: p,
                                font_size,
                                matches: &matches,
                            };
                            let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, wrap_width: f32| {
                                hl.galley(ui.ctx(), buf.as_str(), &params, if wrap { wrap_width } else { f32::INFINITY })
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
                            mc.post(ui, id, &doc.text, &out, font_size);
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

    // ---------------------------------------------------------------- delad vy

    /// Delar vyn i den givna riktningen, byter riktning eller tar bort delningen.
    fn toggle_split(&mut self, orient: Orient) {
        let doc = self.docs.get(self.active).map(|d| d.id);
        self.panes.toggle(orient, doc);
        panes::set_salt(self.panes.active as u8);
    }

    /// Håller den aktiva rutan i takt med den aktiva fliken (flikklick, öppna, stäng m.m. ändrar bara `active`).
    fn sync_panes(&mut self) {
        let active = self.docs.get(self.active).map(|d| d.id);
        let ids: Vec<u64> = self.docs.iter().map(|d| d.id).collect();
        self.panes.follow_active(active, |id| ids.contains(&id));
        panes::set_salt(self.panes.active as u8);
    }

    /// Gör en ruta aktiv: dess flik blir den aktiva fliken.
    fn focus_pane(&mut self, pane: usize) {
        if let Some(id) = self.panes.focus(pane) {
            if let Some(i) = self.index_of(id) {
                self.active = i;
            }
            self.wv.pane_switched();
            self.focus_editor = true;
        }
        panes::set_salt(self.panes.active as u8);
    }

    /// Mittytan: en ruta, eller två med en skiljelinje när vyn är delad. Varje ruta ritas av samma
    /// `editor` som en enda ruta (text, dokumentläge eller PDF) med den rutans flik som aktiv.
    fn central(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(orient) = self.panes.orient else {
            self.editor(ui, p);
            return;
        };
        CentralPanel::default().frame(Frame::NONE.fill(p.desk)).show(ui, |ui| {
            let area = ui.max_rect();
            self.settings.split_ratio = panes::clamp_ratio(self.settings.split_ratio);
            let (r0, r1, divider) = panes::split_rects(area, orient, self.settings.split_ratio);

            let resp = ui.interact(divider, Id::unique("kladd-split-divider"), Sense::click_and_drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(match orient {
                    Orient::Vertical => egui::CursorIcon::ResizeHorizontal,
                    Orient::Horizontal => egui::CursorIcon::ResizeVertical,
                });
            }
            if resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    self.settings.split_ratio = panes::ratio_at(area, orient, pos);
                }
            }
            if resp.double_clicked() {
                self.settings.split_ratio = 0.5;
            }
            let color = if resp.hovered() || resp.dragged() { p.accent } else { p.line };
            let line = divider.shrink2(match orient {
                Orient::Vertical => vec2(2.5, 0.0),
                Orient::Horizontal => vec2(0.0, 2.5),
            });
            ui.painter().rect_filled(line, 0.0, color);

            // Den inaktiva rutan först: den aktiva ritas sist och sätter markörläge, sökyta m.m. för resten av bildrutan.
            let rects = [r0, r1];
            let active = self.panes.active;
            let mut pick = None;
            let mut clicked = None;
            let mut close = false;
            for pane in [1 - active, active] {
                let out = self.pane(ui, p, pane, rects[pane], pane == active);
                if out.clicked && pane != active {
                    clicked = Some(pane);
                }
                if out.pick.is_some() {
                    pick = out.pick.map(|i| (pane, i));
                }
                close |= out.close;
            }
            if let Some((pane, i)) = pick {
                self.panes.docs[pane] = self.docs[i].id;
                self.focus_pane(pane);
            } else if let Some(pane) = clicked {
                self.focus_pane(pane);
            }
            if close {
                self.panes.close();
                panes::set_salt(0);
            }
        });
    }

    /// Ritar en ruta: liten flikväljare överst och sedan innehållet.
    fn pane(&mut self, ui: &mut Ui, p: &Palette, pane: usize, rect: Rect, active: bool) -> PaneOut {
        let mut out = PaneOut::default();
        let Some(idx) = self.index_of(self.panes.docs[pane]) else { return out };
        let saved_active = self.active;
        self.active = idx;
        panes::set_salt(pane as u8);
        // Rullning till träff och fokus gäller bara den aktiva rutan.
        let held = if active { None } else { Some((self.scroll_to_char.take(), std::mem::take(&mut self.focus_editor))) };

        let pointer_down = ui.input(|i| i.pointer.any_pressed());
        let pointer = ui.input(|i| i.pointer.interact_pos());
        // Klick i rutans flikväljare hanteras av väljaren själv.
        let below_head = Rect::from_min_max(pos2(rect.left(), rect.top() + PANE_HEAD_H), rect.max);
        out.clicked = pointer_down && pointer.is_some_and(|pos| below_head.contains(pos));

        let titles: Vec<(String, bool)> = self.docs.iter().map(|d| (d.title(), d.id == self.docs[idx].id)).collect();
        let same_pdf = pane == 1 && self.panes.same_doc() && self.docs[idx].pdfv.is_some();
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), PANE_HEAD_H));
        let body = Rect::from_min_max(pos2(rect.left(), head.bottom() + 1.0), rect.max);
        ui.scope_builder(UiBuilder::new().max_rect(head).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.set_clip_rect(head.intersect(ui.clip_rect()));
            ui.painter().rect_filled(head, 0.0, p.chrome);
            ui.painter().hline(head.x_range(), head.bottom(), Stroke::new(1.0, p.line));
            if active {
                ui.painter().rect_filled(Rect::from_min_size(head.min, vec2(head.width(), 2.0)), 0.0, p.accent);
            }
            ui.add_space(8.0);
            let title = titles.iter().find(|t| t.1).map(|t| truncate(&t.0, 32)).unwrap_or_default();
            let btn = status_button(ui, p, &format!("{title} ▾"), "Välj flik för den här rutan");
            Popup::menu(&btn).align(RectAlign::BOTTOM_START).gap(4.0).width(240.0).show(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, (t, current)) in titles.iter().enumerate() {
                    if menu_item(ui, p, &truncate(t, 36), "", Some(*current), true) {
                        out.pick = Some(i);
                    }
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                if icons::button(ui, p, Icon::Close, 22.0, false, "Stäng delad vy", p.chrome).clicked() {
                    out.close = true;
                }
            });
        });
        ui.scope_builder(UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)), |ui| {
            ui.set_clip_rect(body.intersect(ui.clip_rect()));
            if same_pdf {
                CentralPanel::default().frame(Frame::NONE.fill(p.desk)).show(ui, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("PDF:en visas redan i den andra rutan").color(p.muted));
                    });
                });
            } else {
                self.editor(ui, p);
            }
        });

        if let Some((s, f)) = held {
            self.scroll_to_char = s;
            self.focus_editor = f;
        }
        self.active = saved_active;
        panes::set_salt(self.panes.active as u8);
        out
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
        if !self.find.open || self.docs.is_empty() || self.pdf_mode() {
            return;
        }
        let word = self.word_mode();
        let width = 520.0f32.min(self.editor_rect.width() - 24.0);
        let pos = pos2(self.editor_rect.right() - width - 22.0, self.editor_rect.top() + 12.0);
        let results = self.search_results();
        let matches = &results.matches;
        let has_search = !self.find.query.is_empty() || (word && !self.find.fmt.is_empty());
        let current_idx = self.find.current.and_then(|c| matches.iter().position(|m| m.start == c));

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
                            let field_w = ui.available_width() - 26.0 * 6.0 - 84.0 - 4.0 * 8.0;
                            let q = ui.add(
                                TextEdit::singleline(&mut self.find.query)
                                    .id(Id::unique("kladd-find-query"))
                                    .hint_text("Sök")
                                    .desired_width(field_w)
                                    .margin(Margin::symmetric(8, 5)),
                            );
                            if let Some(err) = &results.error {
                                ui.painter().rect_stroke(q.rect, 4.0, Stroke::new(1.5, p.error), egui::StrokeKind::Inside);
                                q.clone().on_hover_text(format!("Ogiltigt reguljärt uttryck: {err}"));
                            }
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
                            let count = if results.error.is_some() {
                                "Ogiltigt".to_owned()
                            } else if !has_search {
                                String::new()
                            } else if matches.is_empty() {
                                "Inga träffar".to_owned()
                            } else if let Some(i) = current_idx {
                                format!("{} av {}", i + 1, matches.len())
                            } else {
                                format!("{} träffar", matches.len())
                            };
                            let (cr, _) = ui.allocate_exact_size(vec2(84.0, 26.0), Sense::hover());
                            let bad = results.error.is_some() || (matches.is_empty() && has_search);
                            let color = if bad { p.error } else { p.muted };
                            ui.painter().text(cr.center(), Align2::CENTER_CENTER, count, FontId::new(12.0, FontFamily::Proportional), color);

                            let mut toggled = false;
                            if icons::button(ui, p, Icon::CaseSensitive, 26.0, self.find.case_sensitive, "Skilj på stora och små bokstäver", p.surface).clicked() {
                                self.find.case_sensitive = !self.find.case_sensitive;
                                toggled = true;
                            }
                            if text_toggle(ui, p, "ab", self.find.whole_word, "Hela ord").clicked() {
                                self.find.whole_word = !self.find.whole_word;
                                toggled = true;
                            }
                            if text_toggle(ui, p, ".*", self.find.regex, "Reguljära uttryck (ersättning kan använda $1 och ${namn})").clicked() {
                                self.find.regex = !self.find.regex;
                                toggled = true;
                            }
                            if toggled {
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
                        if word {
                            ui.horizontal(|ui| {
                                ui.add_space(30.0);
                                let mut fmt = self.find.fmt.clone();
                                let btn = ui.add(quiet_button(&fmt.label(), p).min_size(vec2(110.0, 26.0)));
                                let btn = btn.on_hover_text("Sök bara text med vald formatering. Tom sökning hittar all text med formateringen");
                                Popup::menu(&btn).gap(4.0).width(200.0).show(|ui| format_filter_menu(ui, p, &mut fmt));
                                if fmt != self.find.fmt {
                                    self.find.fmt = fmt;
                                    self.find.current = None;
                                    seek = Some(Seek::FromSelectionStart);
                                }
                                if !self.find.fmt.is_empty() && icons::button(ui, p, Icon::Close, 22.0, false, "Ta bort formatfiltret", p.surface).clicked() {
                                    self.find.fmt = FormatFilter::default();
                                    self.find.current = None;
                                    seek = Some(Seek::FromSelectionStart);
                                }
                            });
                        }
                        let mut do_replace = false;
                        let mut do_replace_all = false;
                        if self.find.replace {
                            let keep = word && self.find.apply.is_some() && self.find.keep_text;
                            ui.horizontal(|ui| {
                                ui.add_space(30.0);
                                let field_w = (ui.available_width() - 74.0 - 104.0 - 16.0).max(80.0);
                                let r = ui.add_enabled(
                                    !keep,
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
                            if word {
                                ui.horizontal(|ui| {
                                    ui.add_space(30.0);
                                    let label = match self.find.apply {
                                        None => "Formatera".to_owned(),
                                        Some(a) => format!("Formatera: {}", apply_label(a)),
                                    };
                                    let btn = ui.add(quiet_button(&label, p).min_size(vec2(150.0, 26.0)));
                                    let btn = btn.on_hover_text("Lägg på formatering på träffen när du ersätter");
                                    let mut ap = self.find.apply;
                                    Popup::menu(&btn).gap(4.0).width(200.0).show(|ui| apply_menu(ui, p, &mut ap));
                                    self.find.apply = ap;
                                    if self.find.apply.is_some() {
                                        ui.checkbox(&mut self.find.keep_text, "Behåll text")
                                            .on_hover_text("Formatera träffarna utan att ändra texten");
                                    }
                                });
                            }
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
            Some(_) => {
                self.pending_close = None;
                self.update.cancel_launch();
            }
            None => {}
        }
    }

    /// Uppdateringsdelens meddelanden och önskemål om att avsluta.
    fn update_outs(&mut self, ctx: &egui::Context) {
        for out in self.update.take_outs() {
            match out {
                crate::update::Out::Notify(text, error) => self.notify(ctx, text, error),
                crate::update::Out::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
    }

    /// Frågar vid start om osparade ändringar från en tidigare krasch ska återställas.
    fn recovery_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(prompt) = self.recovery_prompt.as_mut() else { return };
        let Some(choice) = crate::recovery::dialog(ctx, p, prompt) else { return };
        let prompt = self.recovery_prompt.take().expect("dialogen är öppen");
        match choice {
            crate::recovery::Choice::Later => {}
            crate::recovery::Choice::Discard => prompt.items.iter().for_each(|(f, _)| f.discard()),
            crate::recovery::Choice::Restore => {
                let mut restored = 0;
                for (found, checked) in &prompt.items {
                    if !*checked {
                        continue;
                    }
                    self.untitled_counter += 1;
                    let doc = found.into_document(self.next_id, self.untitled_counter);
                    self.next_id += 1;
                    if doc.path.is_some() {
                        self.untitled_counter -= 1;
                    }
                    self.docs.push(doc);
                    self.active = self.docs.len() - 1;
                    found.discard();
                    restored += 1;
                }
                self.focus_editor = true;
                // Den återställda texten ska hamna i en ny säkerhetskopia utan att vänta.
                self.recovery.write_soon();
                self.notify(ctx, if restored == 1 { "Återställde 1 osparad flik".to_owned() } else { format!("Återställde {restored} osparade flikar") }, false);
            }
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

/// Är filen en PDF (efter filändelsen eller de första tecknen)?
fn looks_like_pdf(path: &Path) -> bool {
    use std::io::Read;
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return true;
    }
    let mut head = [0u8; 5];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"%PDF-"
}

// ---------------------------------------------------------------- PDF-visaren

impl KladdApp {
    /// Är den aktiva fliken en PDF i PDF-visaren?
    pub(crate) fn pdf_mode(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| d.pdfv.is_some())
    }

    fn pdf_presenting(&self) -> bool {
        self.docs.get(self.active).and_then(|d| d.pdfv.as_ref()).is_some_and(|t| t.view.presenting())
    }

    fn pdf_tab_mut(&mut self) -> Option<&mut pdfui::PdfTab> {
        self.docs.get_mut(self.active).and_then(|d| d.pdfv.as_deref_mut())
    }

    /// Öppnar en PDF i visaren. Lösenordsskyddade filer går via lösenordsdialogen.
    fn open_pdf_tab(&mut self, ctx: &egui::Context, path: &Path, password: Option<&str>) {
        let bytes = match std::fs::read(path) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                self.settings.recent.retain(|p| p != path);
                self.notify(ctx, crate::doc::describe_io_error(&e, path), true);
                return;
            }
        };
        let engine = match PdfEngine::open(bytes.clone(), password, ctx.clone()) {
            Ok(e) => e,
            Err(OpenError::NeedsPassword) => {
                self.pending_password = Some(PendingPassword { path: path.to_path_buf(), input: String::new(), wrong: false, focus: true });
                return;
            }
            Err(OpenError::WrongPassword) => {
                self.pending_password = Some(PendingPassword { path: path.to_path_buf(), input: String::new(), wrong: true, focus: true });
                return;
            }
            Err(OpenError::Invalid(m)) => {
                // Visaren klarar inte filen: försök med textkonverteringen som reserv.
                if let Ok(doc) = Document::from_pdf_text(self.next_id, path, &bytes, password) {
                    self.install_doc(ctx, doc, path);
                    return;
                }
                self.settings.recent.retain(|p| p != path);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify(ctx, format!("{name} kunde inte öppnas: {m}"), true);
                return;
            }
        };
        let meta = crate::pdfview::meta::read_meta(&bytes, password);
        let key = path.to_string_lossy();
        let memory = self.settings.pdf_memory.iter().find(|(k, _)| *k == key).map(|(_, m)| m.clone());
        let view = PdfView::new(engine, meta, bytes, memory);
        let tab = pdfui::PdfTab::new(view, password.map(str::to_owned), pdfui::file_stamp(path));
        let doc = Document::pdf_tab(self.next_id, path, tab);
        self.install_doc(ctx, doc, path);
    }

    /// Sparar flikens visningsläge i inställningarna (sida, zoom, rotation).
    fn store_pdf_memory(&mut self, idx: usize) {
        let Some(doc) = self.docs.get(idx) else { return };
        let (Some(tab), Some(path)) = (doc.pdfv.as_ref(), doc.path.as_ref()) else { return };
        pdfui::remember_memory(&mut self.settings.pdf_memory, path.to_string_lossy().into_owned(), tab.view.memory(), pdfui::MAX_MEMORY);
    }

    /// Redigera: gör om PDF:en till ett redigerbart dokument i samma flik (som PDF-öppning gjorde förut).
    fn pdf_edit(&mut self, ctx: &egui::Context) {
        let idx = self.active;
        let Some(doc) = self.docs.get_mut(idx) else { return };
        let (Some(tab), Some(path)) = (doc.pdfv.as_deref_mut(), doc.path.clone()) else { return };
        if tab.ocr.edit_ocr && tab.ocr.edit_offer() {
            tab.ocr.edit_ocr = false;
            return self.pdf_ocr(ctx, true);
        }
        let (bytes, password, id) = (tab.view.bytes().clone(), tab.password.clone(), doc.id);
        match Document::from_pdf_text_ocr(id, &path, &bytes, password.as_deref(), &tab.ocr.page_texts()) {
            Ok(mut new) => {
                self.store_pdf_memory(idx);
                let note = new.open_note.take();
                self.docs[idx] = new;
                self.hl.forget(id);
                self.word.forget(id);
                self.find.open = false;
                self.focus_editor = true;
                self.notify(ctx, note.unwrap_or_else(|| "PDF:en är nu ett redigerbart dokument".to_owned()), false);
            }
            Err(LoadError::Message(m)) => self.notify(ctx, m, true),
            Err(_) => self.notify(ctx, "PDF:en kunde inte göras om till text.", true),
        }
    }

    /// Läser om filen från disk i den aktiva PDF-fliken och behåller sida och zoom.
    fn pdf_reload(&mut self, ctx: &egui::Context, manual: bool) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let name = doc.title();
        let (Some(tab), Some(path)) = (doc.pdfv.as_deref_mut(), doc.path.clone()) else { return };
        tab.stamp = pdfui::file_stamp(&path);
        let result = (|| -> Result<bool, String> {
            let bytes = std::fs::read(&path).map_err(|e| crate::doc::describe_io_error(&e, &path))?;
            if !manual && bytes == **tab.view.bytes() {
                return Ok(false);
            }
            let bytes = Arc::new(bytes);
            let engine = PdfEngine::open(bytes.clone(), tab.password.as_deref(), ctx.clone()).map_err(|e| match e {
                OpenError::Invalid(m) => m,
                _ => "lösenordet stämmer inte längre".to_owned(),
            })?;
            let meta = crate::pdfview::meta::read_meta(&bytes, tab.password.as_deref());
            tab.view.reload(engine, meta, bytes);
            tab.pages_changed = false;
            tab.org.reset();
            tab.reload_forms();
            Ok(true)
        })();
        match result {
            Ok(true) => self.notify(ctx, format!("Läste om {name}"), false),
            Ok(false) if manual => self.notify(ctx, format!("{name} är oförändrad"), false),
            Ok(false) => {}
            Err(e) => self.notify(ctx, format!("{name} kunde inte läsas om: {e}"), true),
        }
    }

    /// Sekundvis kontroll av att den aktiva PDF:en ändrats på disk, samt sidbild och länkar.
    fn pdf_frame(&mut self, ctx: &egui::Context) {
        let now = Self::now(ctx);
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let path = doc.path.clone();
        // Osparade sidändringar, anteckningar och formulärvärden gör fliken "ändrad" (fråga vid stängning).
        if let Some(t) = doc.pdfv.as_deref() {
            doc.dirty = t.unsaved();
        }
        let Some(tab) = doc.pdfv.as_deref_mut() else { return };
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(pdfui::CHECK_EVERY));
        let mut reload = false;
        if now >= tab.next_check {
            tab.next_check = now + pdfui::CHECK_EVERY;
            if let Some(stamp) = path.as_deref().and_then(pdfui::file_stamp) {
                // Osparade ändringar skrivs inte över av en ändring på disk.
                reload = tab.stamp.is_some() && tab.stamp != Some(stamp) && !tab.unsaved();
            }
        }
        // Sidan som bild är färdig.
        let mut copied = None;
        if let Some(rx) = &tab.copy_rx {
            match rx.try_recv() {
                Ok(r) => copied = Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => copied = Some(Err("Sidan kunde inte renderas".to_owned())),
            }
            if copied.is_some() {
                tab.copy_rx = None;
            }
        }
        // Textigenkänning: resultat in i visaren, och notiser när den är klar.
        let pw = tab.password.clone();
        let polled = tab.ocr.poll(&mut tab.view, pw.as_deref(), ctx);
        // En klickad länk i sidan ska bekräftas innan den öppnas.
        let mut blocked = None;
        if let Some(uri) = tab.view.take_pending_uri() {
            if pdfui::safe_uri(&uri) {
                tab.confirm_uri = Some(uri);
            } else {
                blocked = Some(uri);
            }
        }
        match copied {
            Some(Ok(img)) => {
                ctx.copy_image(img);
                self.notify(ctx, "Sidan är kopierad som bild", false);
            }
            Some(Err(e)) => self.notify(ctx, format!("Kunde inte kopiera sidan: {e}"), true),
            None => {}
        }
        if let Some(uri) = blocked {
            self.notify(ctx, format!("Länken öppnas inte: {}", truncate(&uri, 60)), true);
        }
        for (m, err) in polled.notices {
            self.notify(ctx, m, err);
        }
        if polled.edit_now {
            self.pdf_edit(ctx);
        } else if reload {
            self.pdf_reload(ctx, false);
        }
    }

    /// Utför det verktygsraden eller dialogerna bad om.
    fn pdf_action(&mut self, ctx: &egui::Context, act: PdfAction) {
        match act {
            PdfAction::Edit => self.pdf_edit(ctx),
            PdfAction::Organize(op) => self.pdf_organize(ctx, op),
            PdfAction::SaveCopy => self.request_save_copy(ctx, self.active),
            PdfAction::Print => self.open_print(ctx),
            PdfAction::Reload => self.pdf_reload(ctx, true),
            PdfAction::OpenExternal => {
                if let Some(path) = self.docs.get(self.active).and_then(|d| d.path.clone()) {
                    if let Err(e) = crate::platform::open(&path) {
                        self.notify(ctx, format!("Kunde inte öppna PDF-visaren: {e}"), true);
                    }
                }
            }
            PdfAction::CopyImage => {
                let Some(tab) = self.pdf_tab_mut() else { return };
                if tab.copy_rx.is_none() {
                    let rotation = tab.view.memory().rotation;
                    tab.copy_rx = Some(pdfui::render_page_image(
                        tab.view.bytes().clone(),
                        tab.password.clone(),
                        tab.view.current_page(),
                        rotation,
                        ctx.clone(),
                    ));
                    self.notify(ctx, "Kopierar sidan…", false);
                }
            }
            PdfAction::OpenUri(uri) => {
                if let Err(e) = crate::platform::open(Path::new(&uri)) {
                    self.notify(ctx, format!("Kunde inte öppna länken: {e}"), true);
                }
            }
            PdfAction::Ocr => self.pdf_ocr(ctx, false),
        }
    }

    /// Startar textigenkänning av de skannade sidorna (`edit_after`: gör om PDF:en till text när den är klar).
    fn pdf_ocr(&mut self, ctx: &egui::Context, edit_after: bool) {
        let Some(tab) = self.pdf_tab_mut() else { return };
        let pw = tab.password.clone();
        match tab.ocr.start(&tab.view, pw.as_deref(), ctx, edit_after) {
            Ok(()) => self.notify(ctx, "Känner igen text…", false),
            Err(m) => self.notify(ctx, m, true),
        }
    }

    /// "Spara kopia": originalfilens bytes oförändrade till ett nytt namn.
    fn request_save_copy(&mut self, ctx: &egui::Context, idx: usize) {
        if self.dialog.is_some() {
            self.notify(ctx, "Stäng den öppna fildialogen först.", true);
            return;
        }
        let Some(doc) = self.docs.get(idx).filter(|d| d.pdfv.is_some()) else { return };
        let (tx, rx) = channel();
        self.dialog = Some(rx);
        let ctx = ctx.clone();
        let doc_id = doc.id;
        let stem = doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "dokument".into());
        let dir = doc.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(|| self.start_dir());
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new()
                .set_title("Spara kopia")
                .set_file_name(format!("{stem} (kopia).pdf"))
                .add_filter("PDF", &["pdf"]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let _ = tx.send(DialogResult::SaveCopy { doc_id, path: dlg.save_file() });
            ctx.request_repaint();
        });
    }

    fn write_pdf_copy(&mut self, ctx: &egui::Context, doc_id: u64, path: &Path) {
        let Some(idx) = self.index_of(doc_id) else { return };
        let Some(tab) = self.docs[idx].pdfv.as_ref() else { return };
        // Ifyllda formulärvärden följer med i kopian.
        let bytes = if tab.view.forms.dirty() {
            match crate::pdfview::forms::apply(tab.view.bytes(), tab.password.as_deref(), &tab.view.forms.changes()) {
                Ok(b) => Arc::new(b),
                Err(e) => {
                    self.notify(ctx, e, true);
                    return;
                }
            }
        } else {
            tab.view.bytes().clone()
        };
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if self.docs[idx].path.as_deref() == Some(canon.as_path()) {
            self.notify(ctx, "Välj ett annat namn än originalfilens.", true);
            return;
        }
        // Osparade ändringar (sidor, anteckningar) följer med i kopian; originalfilen och fliken är oförändrade.
        let (bytes, decrypted) = match self.docs[idx].pdfv.as_ref().filter(|t| t.unsaved()) {
            Some(t) => match t.edited_bytes() {
                Ok(out) => (Arc::new(out), t.password.is_some()),
                Err(e) => {
                    self.notify(ctx, e, true);
                    return;
                }
            },
            None => (bytes, false),
        };
        match crate::doc::write_atomic(path, &bytes) {
            Ok(()) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let note = if decrypted { " (utan lösenordsskydd)" } else { "" };
                self.notify(ctx, format!("Sparade kopian {name}{note}"), false);
            }
            Err(e) => self.notify(ctx, crate::doc::describe_io_error(&e, path), true),
        }
    }

    /// Sparar PDF-flikens ändringar (sidordning, anteckningar) över originalfilen, atomiskt.
    fn pdf_save(&mut self, ctx: &egui::Context, idx: usize, then_close: bool, exit: bool) {
        let Some(doc) = self.docs.get_mut(idx) else { return };
        let (Some(tab), Some(path)) = (doc.pdfv.as_deref_mut(), doc.path.clone()) else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !tab.unsaved() {
            if then_close {
                self.close_tab_now(idx);
            }
            if exit {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            return;
        }
        let decrypted = tab.password.is_some();
        let result = tab
            .edited_bytes()
            .and_then(|out| crate::doc::write_atomic(&path, &out).map_err(|e| crate::doc::describe_io_error(&e, &path)));
        if let Err(e) = result {
            self.notify(ctx, e, true);
            return;
        }
        tab.mark_saved();
        tab.password = None;
        tab.stamp = pdfui::file_stamp(&path);
        doc.dirty = false;
        if then_close {
            self.close_tab_now(idx);
        } else if idx == self.active {
            self.pdf_reload(ctx, true);
        }
        let note = if decrypted { " PDF:en sparades utan lösenordsskydd." } else { "" };
        self.notify(ctx, format!("Sparade {name}.{note}"), false);
        if exit {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// Verktygsraden för PDF-flikar (ersätter redigerarens).
    fn pdf_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(tab) = self.pdf_tab_mut() else { return };
        if let Some(act) = pdfui::toolbar(ui, p, tab) {
            self.pdf_action(&ui.ctx().clone(), act);
        }
        let Some(tab) = self.pdf_tab_mut() else { return };
        if let Some(crate::pdfview::forms::FormAction::Save) = crate::pdfview::forms::bar(ui, p, &mut tab.view.forms) {
            self.save_doc(&ui.ctx().clone(), self.active, false, false);
        }
    }

    fn pdf_sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        let act = self.pdf_tab_mut().and_then(|tab| pdfui::sidebar(ui, p, tab));
        if let Some(act) = act {
            self.pdf_action(&ui.ctx().clone(), act);
        }
    }

    /// Sidytan i mitten.
    fn pdf_page(&mut self, ui: &mut Ui, p: &Palette) {
        CentralPanel::default().frame(Frame::NONE.fill(p.desk)).show(ui, |ui| {
            self.editor_rect = ui.max_rect();
            if let Some(tab) = self.pdf_tab_mut() {
                tab.view.show(ui, p, p.dark);
            }
        });
    }

    fn pdf_overlays(&mut self, ctx: &egui::Context, p: &Palette) {
        let path = self.docs.get(self.active).and_then(|d| d.path.clone());
        let anchor = self.editor_rect;
        let Some(tab) = self.pdf_tab_mut() else { return };
        pdfui::find_bar(ctx, p, tab, anchor);
        let ocr_act = crate::pdfview::ocr::notice_bar(ctx, p, tab, anchor);
        let dlg_act = pdfui::dialogs(ctx, p, tab, path.as_deref());
        for act in [ocr_act, dlg_act].into_iter().flatten() {
            self.pdf_action(ctx, act);
        }
    }

    /// Helskärm för presentationsläget följer visarens läge.
    fn pdf_sync_fullscreen(&mut self, ctx: &egui::Context) {
        let on = self.pdf_presenting();
        if on != self.pdf_fullscreen {
            self.pdf_fullscreen = on;
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(on));
        }
    }

    /// PDF-specifika kortkommandon och visarens egna tangenter.
    fn pdf_shortcuts(&mut self, ctx: &egui::Context) {
        let hit = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.pdfv.as_deref_mut()) else { return };
        if tab.view.presenting() {
            tab.view.handle_keys(ctx);
            return;
        }
        if tab.find.open && hit(Modifiers::NONE, Key::Escape) {
            tab.close_find();
        }
        if hit(Modifiers::COMMAND, Key::G) {
            tab.focus_page_field();
        }
        if hit(Modifiers::NONE, Key::F4) {
            tab.toggle_sidebar();
        }
        if hit(Modifiers::NONE, Key::F5) {
            tab.present();
        }
        // Formulärfält: Tab, Esc och mellanslag hör till fältet som är öppet.
        tab.view.forms.pre_keys(ctx);
        if let Some((page, r)) = tab.view.forms.take_goto() {
            if page != tab.view.current_page() {
                tab.view.go_to(&crate::pdfview::meta::Dest { page, y: Some((r.top() - 40.0).max(0.0)) });
            }
        }
        // Skriver man i sök- eller sidfältet ska bokstäverna inte vara visarens kommandon.
        let mut org = None;
        if !ctx.egui_wants_keyboard_input() {
            // Sidändringar: Ångra/Gör om, och Delete tar bort de valda miniatyrerna.
            if hit(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || hit(Modifiers::COMMAND, Key::Y) {
                org = Some(PdfOrgOp::Redo);
            } else if hit(Modifiers::COMMAND, Key::Z) {
                org = Some(PdfOrgOp::Undo);
            } else if tab.sidebar && tab.org.sel.len() < tab.view.page_count() && !tab.org.sel.is_empty() && hit(Modifiers::NONE, Key::Delete) {
                org = Some(PdfOrgOp::Delete(tab.org.sel.iter().copied().collect()));
            }
            tab.view.handle_keys(ctx);
        }
        if let Some(op) = org {
            self.pdf_organize(ctx, op);
        }
    }

    /// Sant om den aktiva PDF-fliken har osparade ändringar (sidor, anteckningar).
    fn pdf_unsaved(&self) -> bool {
        self.docs.get(self.active).and_then(|d| d.pdfv.as_ref()).is_some_and(|t| t.unsaved())
    }

    /// Utför en sidändring i den aktiva PDF-fliken.
    fn pdf_organize(&mut self, ctx: &egui::Context, op: PdfOrgOp) {
        let idx = self.active;
        let Some(doc) = self.docs.get_mut(idx) else { return };
        let doc_id = doc.id;
        let Some(tab) = doc.pdfv.as_deref_mut() else { return };
        let (kind, pages) = match &op {
            PdfOrgOp::Extract(pages) => (0, pages.clone()),
            PdfOrgOp::InsertAt(at) => (1, vec![*at]),
            _ => {
                match pdfpages::run(ctx, tab, &op) {
                    Ok(msg) => {
                        doc.dirty = true;
                        if !msg.is_empty() {
                            self.notify(ctx, msg, false);
                        }
                    }
                    Err(e) => self.notify(ctx, e, true),
                }
                return;
            }
        };
        if self.dialog.is_some() {
            self.notify(ctx, "Stäng den öppna fildialogen först.", true);
            return;
        }
        let stem = doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "dokument".into());
        let dir = doc.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(|| self.start_dir());
        let (tx, rx) = channel();
        self.dialog = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut dlg = rfd::FileDialog::new().add_filter("PDF", &["pdf"]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let res = if kind == 0 {
                dlg = dlg.set_title("Extrahera till ny PDF").set_file_name(format!("{stem} (utdrag).pdf"));
                DialogResult::PdfExtract { doc_id, path: dlg.save_file(), pages }
            } else {
                dlg = dlg.set_title("Välj PDF att infoga");
                DialogResult::PdfInsert { doc_id, path: dlg.pick_file(), at: pages[0] }
            };
            let _ = tx.send(res);
            ctx.request_repaint();
        });
    }

    fn write_pdf_extract(&mut self, ctx: &egui::Context, doc_id: u64, path: &Path, pages: &[usize]) {
        let Some(idx) = self.index_of(doc_id) else { return };
        let Some(tab) = self.docs[idx].pdfv.as_ref() else { return };
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if self.docs[idx].path.as_deref() == Some(canon.as_path()) {
            self.notify(ctx, "Välj ett annat namn än originalfilens.", true);
            return;
        }
        match pdfpages::extract_bytes(tab, pages).and_then(|b| crate::doc::write_atomic(path, &b).map_err(|e| crate::doc::describe_io_error(&e, path))) {
            Ok(()) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify(ctx, format!("Sparade {name}"), false);
            }
            Err(e) => self.notify(ctx, e, true),
        }
    }

    fn insert_pdf_file(&mut self, ctx: &egui::Context, doc_id: u64, path: &Path, at: usize) {
        let Some(idx) = self.index_of(doc_id) else { return };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.notify(ctx, crate::doc::describe_io_error(&e, path), true);
                return;
            }
        };
        let doc = &mut self.docs[idx];
        let Some(tab) = doc.pdfv.as_deref_mut() else { return };
        match pdfpages::insert_bytes(ctx, tab, &bytes, at) {
            Ok(msg) => {
                doc.dirty = true;
                self.notify(ctx, msg, false);
            }
            Err(e) => self.notify(ctx, e, true),
        }
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

/// Liten textknapp som kan vara intryckt (t.ex. ".*" i sökfältet).
pub(crate) fn text_toggle(ui: &mut Ui, p: &Palette, label: &str, on: bool, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    let response = response.on_hover_text(crate::keys::shortcut(tooltip));
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        if hovered {
            ui.painter().rect_filled(rect, 7.0, p.surface_hover);
        }
        let color = if on {
            p.accent
        } else if hovered {
            p.text
        } else {
            p.muted
        };
        if on {
            ui.painter().rect_stroke(rect.shrink(1.5), 6.0, Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
        }
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::new(13.0, FontFamily::Monospace), color);
    }
    response
}

fn text_color_rows() -> Vec<Vec<(String, [u8; 3])>> {
    crate::word::view::TEXT_COLORS
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| {
                    let rgb = [(c >> 16) as u8, (c >> 8) as u8, *c as u8];
                    (crate::style::hex(rgb), rgb)
                })
                .collect()
        })
        .collect()
}

fn highlight_rows() -> Vec<Vec<(String, [u8; 3])>> {
    let all: Vec<(String, [u8; 3])> = crate::word::view::HIGHLIGHTS
        .iter()
        .filter_map(|(id, name)| {
            let rgb = crate::style::highlight_rgb(id)?;
            Some((format!("{name} ({})", crate::style::hex(rgb)), rgb))
        })
        .collect();
    vec![all[..8].to_vec(), all[8..].to_vec()]
}

/// Innehållet i "Format"-menyn i sökfältet: vilken formatering en träff måste ha.
fn format_filter_menu(ui: &mut Ui, p: &Palette, f: &mut FormatFilter) {
    ui.checkbox(&mut f.bold, "Fetstil");
    ui.checkbox(&mut f.italic, "Kursiv");
    ui.checkbox(&mut f.underline, "Understruken");
    ui.checkbox(&mut f.strike, "Genomstruken");
    ui.checkbox(&mut f.highlight, "Markerad");
    ui.menu_button(if f.color.is_some() { "Textfärg (vald)" } else { "Textfärg" }, |ui| {
        if let Some(c) = crate::word::view::color_popup(ui, p, "Alla färger", &text_color_rows(), f.color) {
            f.color = c;
        }
    });
    for n in 1..=3u8 {
        let mut on = f.heading == Some(n);
        if ui.checkbox(&mut on, format!("Rubrik {n}")).changed() {
            f.heading = on.then_some(n);
        }
    }
}

fn apply_label(a: Apply) -> &'static str {
    match a {
        Apply::Bold => "Fetstil",
        Apply::Italic => "Kursiv",
        Apply::Underline => "Understruken",
        Apply::Color(_) => "Textfärg",
        Apply::Highlight(_) => "Markering",
        Apply::Clear => "Ta bort formatering",
    }
}

/// Innehållet i "Formatera"-menyn i ersättningsraden.
fn apply_menu(ui: &mut Ui, p: &Palette, ap: &mut Option<Apply>) {
    if ui.selectable_label(ap.is_none(), "Ingen formatering").clicked() {
        *ap = None;
        ui.close();
    }
    for a in [Apply::Bold, Apply::Italic, Apply::Underline] {
        if ui.selectable_label(*ap == Some(a), apply_label(a)).clicked() {
            *ap = Some(a);
            ui.close();
        }
    }
    ui.menu_button("Textfärg", |ui| {
        if let Some(Some(c)) = crate::word::view::color_popup(ui, p, "Automatisk", &text_color_rows(), None) {
            *ap = Some(Apply::Color(c));
        }
    });
    ui.menu_button("Markering", |ui| {
        if let Some(Some(c)) = crate::word::view::color_popup(ui, p, "Ingen färg", &highlight_rows(), None) {
            *ap = Some(Apply::Highlight(c));
        }
    });
    if ui.selectable_label(*ap == Some(Apply::Clear), apply_label(Apply::Clear)).clicked() {
        *ap = Some(Apply::Clear);
        ui.close();
    }
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
            crate::keys::shortcut(shortcut),
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
    let resp = resp.on_hover_text(crate::keys::shortcut(tooltip));
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
    let right = if shortcut.is_empty() { std::borrow::Cow::Borrowed(detail.unwrap_or("")) } else { crate::keys::shortcut(shortcut) };
    if !right.is_empty() {
        let max_chars = ((rect.width() - label_w - 24.0) / 7.0).max(6.0) as usize;
        painter.text(
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            truncate(&right, max_chars),
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
        self.update.tick(ctx, &mut self.settings);
        self.update_outs(ctx);
        self.recovery.tick(ctx, &self.docs, self.settings.autosave_recovery);
        self.autosave_tick(ctx);
        if self.docs.get(self.active).map(|d| d.id) != self.last_active_id {
            self.last_active_id = self.docs.get(self.active).map(|d| d.id);
            self.find.current = None;
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = self.palette();

        self.sync_panes();
        self.handle_dialogs(&ctx);
        self.handle_drops(&ctx);
        self.handle_shortcuts(&ctx);
        self.pdf_sync_fullscreen(&ctx);
        self.pdf_frame(&ctx);

        // Presentationsläget: bara sidan, inga fält eller paneler.
        if self.pdf_presenting() {
            self.pdf_page(ui, &p);
            return;
        }

        self.top_bar(ui, &p);
        self.word_bar(ui, &p);
        self.pdf_bar(ui, &p);
        self.status_bar(ui, &p);
        self.update.banner(ui, &ctx, &p, &mut self.settings);
        self.pdf_sidebar(ui, &p);
        self.nav_pane(ui, &p);
        self.preview(ui, &p);
        self.central(ui, &p);
        self.find_bar(&ctx, &p);
        self.pdf_overlays(&ctx, &p);
        self.close_dialog(&ctx, &p);
        self.recovery_dialog(&ctx, &p);
        self.update.dialogs(&ctx, &p, &mut self.settings);
        self.update_outs(&ctx);
        self.docx_dialog(&ctx, &p);
        self.history_dialog(&ctx, &p);
        self.password_dialog(&ctx, &p);
        self.print_dialog(&ctx, &p);
        self.page_dialog(&ctx, &p);
        self.header_dialog(&ctx, &p);
        self.drop_hint(&ctx, &p);
        self.window_edges(&ctx, &p);
        self.update_title(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.recovery.shutdown(&self.docs);
        if self.owns_socket {
            single::cleanup();
        }
        self.update.run_on_exit();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        for i in 0..self.docs.len() {
            self.store_pdf_memory(i);
        }
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }
}
