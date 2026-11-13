//! Sök i filer: söker i arbetsmappen eller i alla öppna flikar i en bakgrundstråd (med avbryt),
//! visar träffarna grupperade per fil i en panel längst ned och kan ersätta över flera filer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::text_edit::TextEditState;
use egui::{Align, Align2, Color32, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Panel, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, pos2, vec2};

use crate::app::{KladdApp, byte_to_char, editor_id, quiet_button, text_toggle};
use crate::i18n::t;
use crate::tf;
use crate::search::{self, Spec};
use crate::theme::{self, Palette};

/// Filer större än så här hoppas över.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Sökningen stoppas efter så här många träffar.
const MAX_HITS: usize = 5_000;
/// Längsta förhandsvisningsrad (tecken, ungefär).
const PREVIEW_CHARS: usize = 220;
const ROW_H: f32 = 22.0;

// ============================================================== logik

/// En träff i en text. `start`/`end` är byteintervallet i hela texten; `pstart`/`pend` intervallet i `preview`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Radnummer från 1.
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub preview: String,
    pub pstart: usize,
    pub pend: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    /// En öppen flik (dokument-id), för träffar i "alla öppna flikar".
    Tab(u64),
}

#[derive(Clone, Debug)]
pub struct FileHits {
    pub target: Target,
    /// Visningsnamn: sökväg relativt arbetsmappen, eller flikens titel.
    pub label: String,
    pub hits: Vec<Hit>,
}

/// Var sökningen letar.
pub enum Source {
    /// Alla filer under `root`. `overrides` är texten i öppna flikar (sökvägen -> text) som används
    /// i stället för filen på disk, så att träffarna stämmer med det som syns i fliken.
    Dir { root: PathBuf, overrides: HashMap<PathBuf, String> },
    /// Alla öppna flikar: (dokument-id, titel, text).
    Tabs(Vec<(u64, String, String)>),
}

pub enum Msg {
    Found(FileHits),
    /// Klar (eller avbruten): `skipped` = binära, för stora eller icke-UTF-8-filer, `truncated` = träffgränsen nåddes.
    Done { files: usize, skipped: usize, truncated: bool },
}

/// Träffarna i `text` med rad och förhandsvisning.
pub fn hits_in_text(text: &str, spec: &Spec) -> Result<Vec<Hit>, String> {
    let r = search::search_text(text, spec);
    if let Some(e) = r.error {
        return Err(e);
    }
    let mut out = Vec::with_capacity(r.matches.len());
    let (mut line, mut at) = (1usize, 0usize);
    for m in &r.matches {
        line += text.as_bytes()[at..m.start].iter().filter(|&&b| b == b'\n').count();
        at = m.start;
        out.push(make_hit(text, line, m.start, m.end));
    }
    Ok(out)
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn make_hit(text: &str, line: usize, start: usize, end: usize) -> Hit {
    let ls = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let le = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let full = text[ls..le].trim_end_matches('\r');
    let ms = start - ls;
    let me = (end - ls).min(full.len());
    // Hoppa över indrag, men aldrig förbi träffen.
    let lead = (full.len() - full.trim_start().len()).min(ms);
    let wstart = floor_boundary(full, lead.max(ms.saturating_sub(40)));
    let wend = ceil_boundary(full, (wstart + PREVIEW_CHARS * 2).min(full.len()));
    let prefix = if wstart > lead { "…" } else { "" };
    let suffix = if wend < full.len() { "…" } else { "" };
    let preview = format!("{prefix}{}{suffix}", &full[wstart..wend]);
    let pstart = ms - wstart + prefix.len();
    let pend = (me.min(wend) - wstart + prefix.len()).max(pstart);
    Hit { line, start, end, preview, pstart, pend }
}

/// Läser en fil som text om den är UTF-8, inte binär och inte för stor. `None` = hoppa över.
pub fn read_text_file(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Kör en sökning och skickar resultat till `emit`. Avbryts av `cancel`.
pub fn run_search(source: Source, spec: &Spec, cancel: &AtomicBool, mut emit: impl FnMut(Msg)) {
    let (mut files, mut skipped, mut total) = (0usize, 0usize, 0usize);
    let mut truncated = false;
    match source {
        Source::Tabs(tabs) => {
            for (id, label, text) in tabs {
                if cancel.load(Ordering::Relaxed) || truncated {
                    break;
                }
                files += 1;
                if let Ok(hits) = hits_in_text(&text, spec) {
                    if !hits.is_empty() {
                        total += hits.len();
                        emit(Msg::Found(FileHits { target: Target::Tab(id), label, hits }));
                    }
                }
                truncated = total >= MAX_HITS;
            }
        }
        Source::Dir { root, overrides } => {
            crate::workspace::walk_files(&root, cancel, |path| {
                files += 1;
                let text = match overrides.get(&path) {
                    Some(t) => Some(t.clone()),
                    None => read_text_file(&path),
                };
                let Some(text) = text else {
                    skipped += 1;
                    return true;
                };
                if let Ok(hits) = hits_in_text(&text, spec) {
                    if !hits.is_empty() {
                        total += hits.len();
                        let label = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                        emit(Msg::Found(FileHits { target: Target::File(path), label, hits }));
                    }
                } else {
                    // Ogiltigt reguljärt uttryck: ingen mening att gå vidare.
                    return false;
                }
                truncated = total >= MAX_HITS;
                !truncated
            });
        }
    }
    emit(Msg::Done { files, skipped, truncated });
}

/// Ersätter i filer på disk (som UTF-8-text). Ger per fil antal ersättningar eller ett fel.
/// Filer utan träff lämnas orörda.
pub fn replace_in_files(paths: &[PathBuf], spec: &Spec, repl: &str) -> Vec<(PathBuf, Result<usize, String>)> {
    paths
        .iter()
        .map(|p| {
            let r = (|| {
                let text = read_text_file(p).ok_or_else(|| t("kunde inte läsas som text").to_owned())?;
                match search::replace_text(&text, spec, repl, None) {
                    None => Ok(0),
                    Some(done) => {
                        std::fs::write(p, done.text.as_bytes()).map_err(|e| e.to_string())?;
                        Ok(done.count)
                    }
                }
            })();
            (p.clone(), r)
        })
        .collect()
}

// ============================================================== tillstånd

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    Folder,
    Tabs,
}

struct Job {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct State {
    pub open: bool,
    scope: Scope,
    query: String,
    replacement: String,
    show_replace: bool,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    focus: bool,
    job: Option<Job>,
    results: Vec<FileHits>,
    /// Inställningarna som resultatet hör till (används för "Ersätt alla").
    result_spec: Option<Spec>,
    error: Option<String>,
    status: String,
    collapsed: HashSet<usize>,
    confirm_replace: bool,
}

impl State {
    fn spec(&self) -> Spec {
        Spec {
            query: self.query.clone(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
            fmt: Default::default(),
        }
    }

    fn total(&self) -> usize {
        self.results.iter().map(|f| f.hits.len()).sum()
    }
}

enum Row {
    File(usize),
    Hit(usize, usize),
}

impl KladdApp {
    pub(crate) fn open_find_files(&mut self, ctx: &egui::Context) {
        let _ = ctx;
        self.ff.open = true;
        self.ff.focus = true;
        if self.settings.workspace.is_none() {
            self.ff.scope = Scope::Tabs;
        }
        // Förifyll med markerad kort text.
        if let Some(doc) = self.docs.get(self.active) {
            if !doc.word_view && doc.pdfv.is_none() {
                if let Some(r) = TextEditState::load(ctx, editor_id(doc.id)).and_then(|s| s.cursor.char_range()) {
                    let [a, b] = r.sorted_cursors();
                    if a.index != b.index {
                        let (ba, bb) = (crate::app::char_to_byte(&doc.text, a.index.0), crate::app::char_to_byte(&doc.text, b.index.0));
                        let sel = &doc.text[ba..bb];
                        if !sel.contains('\n') && sel.chars().count() <= 80 {
                            self.ff.query = sel.to_owned();
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn cancel_find_files(&mut self) {
        if let Some(job) = self.ff.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn close_find_files(&mut self) {
        self.cancel_find_files();
        self.ff.open = false;
        self.ff.confirm_replace = false;
        self.focus_editor = true;
    }

    /// Texten i de flikar som kan sökas som vanlig text.
    fn plain_tabs(&self) -> Vec<(u64, String, String)> {
        self.docs
            .iter()
            .filter(|d| !d.docx && !d.pdf && !d.word_view && d.pdfv.is_none())
            .map(|d| (d.id, d.title(), d.text.clone()))
            .collect()
    }

    fn start_find_files(&mut self, ctx: &egui::Context) {
        self.cancel_find_files();
        self.ff.results.clear();
        self.ff.collapsed.clear();
        self.ff.error = None;
        self.ff.result_spec = None;
        self.ff.confirm_replace = false;
        let spec = self.ff.spec();
        if spec.query.is_empty() {
            self.ff.status.clear();
            return;
        }
        let source = match self.ff.scope {
            Scope::Tabs => Source::Tabs(self.plain_tabs()),
            Scope::Folder => {
                let Some(root) = self.settings.workspace.clone() else {
                    self.ff.status = t("Öppna en mapp först (Arkiv, Öppna mapp…).").into();
                    return;
                };
                let overrides = self
                    .docs
                    .iter()
                    .filter(|d| !d.docx && !d.pdf && !d.word_view && d.pdfv.is_none())
                    .filter_map(|d| d.path.clone().map(|p| (p, d.text.clone())))
                    .collect();
                Source::Dir { root, overrides }
            }
        };
        // Ogiltigt reguljärt uttryck visas direkt i stället för i tråden.
        if let Err(e) = hits_in_text("", &spec) {
            self.ff.error = Some(e);
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.ff.job = Some(Job { rx, cancel: cancel.clone() });
        self.ff.result_spec = Some(spec.clone());
        self.ff.status = t("Söker…").into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last = std::time::Instant::now();
            run_search(source, &spec, &cancel, |m| {
                let done = matches!(m, Msg::Done { .. });
                let _ = tx.send(m);
                if done || last.elapsed().as_millis() > 80 {
                    last = std::time::Instant::now();
                    ctx.request_repaint();
                }
            });
            ctx.request_repaint();
        });
    }

    /// Tar emot resultat från bakgrundstråden.
    fn poll_find_files(&mut self) {
        let Some(job) = &self.ff.job else { return };
        let mut finished = false;
        while let Ok(m) = job.rx.try_recv() {
            match m {
                Msg::Found(f) => self.ff.results.push(f),
                Msg::Done { files, skipped, truncated } => {
                    finished = true;
                    let hits = self.ff.total();
                    let mut s = if hits == 0 {
                        tf!("Inga träffar i {files}", files = count_files(files))
                    } else {
                        tf!("{hits} i {found} (av {files})", hits = count_hits(hits), found = count_files(self.ff.results.len()), files = files)
                    };
                    if skipped > 0 {
                        s.push_str(&tf!(", {skipped} hoppades över", skipped = skipped));
                    }
                    if truncated {
                        s.push_str(t(". Gränsen för antal träffar nåddes, förfina sökningen"));
                    }
                    self.ff.status = s;
                }
            }
        }
        if finished {
            self.ff.job = None;
        } else if !self.ff.results.is_empty() {
            let hits = self.ff.total();
            self.ff.status = tf!("Söker… {hits} hittills", hits = count_hits(hits));
        }
    }

    /// Öppnar träffens fil och markerar träffen.
    fn goto_hit(&mut self, ctx: &egui::Context, target: &Target, start: usize, end: usize) {
        match target {
            Target::Tab(id) => {
                let Some(i) = self.index_of(*id) else {
                    self.notify(ctx, t("Fliken är stängd."), true);
                    return;
                };
                self.active = i;
            }
            Target::File(path) => {
                self.open_path(ctx, path);
                let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
                match self.docs.iter().position(|d| d.path.as_deref() == Some(canon.as_path())) {
                    Some(i) if self.docs[i].pdfv.is_none() => self.active = i,
                    _ => return,
                }
            }
        }
        let doc = &self.docs[self.active];
        if end > doc.text.len() || !doc.text.is_char_boundary(start) || !doc.text.is_char_boundary(end) {
            self.notify(ctx, t("Filen har ändrats sedan sökningen. Sök igen."), false);
            return;
        }
        let id = editor_id(doc.id);
        let (ca, cb) = (byte_to_char(&doc.text, start), byte_to_char(&doc.text, end));
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(ca), CCursor::new(cb))));
        state.store(ctx, id);
        self.scroll_to_char = Some(ca);
        self.focus_editor = true;
        self.find.current = None;
    }

    /// Ersätter alla träffar i resultatet. Öppna flikar ändras i minnet (och sparas om de var
    /// ostörda), övriga filer skrivs på disk.
    fn replace_in_all(&mut self, ctx: &egui::Context) {
        let Some(spec) = self.ff.result_spec.clone() else { return };
        let repl = self.ff.replacement.clone();
        let (mut files, mut count) = (0usize, 0usize);
        let mut errors: Vec<String> = Vec::new();
        let mut disk: Vec<PathBuf> = Vec::new();
        let targets: Vec<Target> = self.ff.results.iter().map(|f| f.target.clone()).collect();
        for tg in targets {
            let idx = match &tg {
                Target::Tab(id) => self.index_of(*id),
                Target::File(p) => {
                    let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
                    self.docs.iter().position(|d| d.path.as_deref() == Some(canon.as_path()) && !d.docx && !d.pdf && !d.word_view && d.pdfv.is_none())
                }
            };
            match (idx, &tg) {
                (Some(i), _) => {
                    let doc = &mut self.docs[i];
                    let Some(done) = search::replace_text(&doc.text, &spec, &repl, None) else { continue };
                    let was_clean = !doc.dirty;
                    doc.text = done.text;
                    doc.on_edited();
                    if was_clean {
                        if let Some(path) = doc.path.clone() {
                            if let Err(e) = doc.save_to(&path) {
                                errors.push(format!("{}: {e}", doc.title()));
                            }
                        }
                    }
                    files += 1;
                    count += done.count;
                }
                (None, Target::File(p)) => disk.push(p.clone()),
                (None, Target::Tab(_)) => {}
            }
        }
        for (p, r) in replace_in_files(&disk, &spec, &repl) {
            match r {
                Ok(0) => {}
                Ok(n) => {
                    files += 1;
                    count += n;
                }
                Err(e) => errors.push(format!("{}: {e}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
            }
        }
        self.find.current = None;
        let mut msg = tf!("Ersatte {hits} i {files}", hits = count_hits(count), files = count_files(files));
        if !errors.is_empty() {
            msg.push_str(&tf!(". Fel: {errors}", errors = errors.join("; ")));
        }
        self.notify(ctx, msg, !errors.is_empty());
        self.start_find_files(ctx);
    }

    /// Resultatpanelen längst ned.
    pub(crate) fn find_files_panel(&mut self, ui: &mut Ui, p: &Palette) {
        self.poll_find_files();
        if !self.ff.open {
            return;
        }
        let ctx = ui.ctx().clone();
        let mut goto: Option<(Target, usize, usize)> = None;
        let mut run = false;
        let mut cancel = false;
        let mut close = false;
        let mut toggle_file: Option<usize> = None;
        let running = self.ff.job.is_some();
        let has_root = self.settings.workspace.is_some();

        Panel::bottom("kladd-find-files")
            .resizable(true)
            .default_size(260.0)
            .size_range(120.0..=600.0)
            .frame(Frame::NONE.fill(p.editor).stroke(Stroke::new(1.0, p.line)).inner_margin(Margin { left: 12, right: 12, top: 8, bottom: 6 }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("SÖK I FILER")).font(FontId::new(11.5, theme::semibold())).color(p.muted));
                    ui.add_space(8.0);
                    if crate::workspace::chip(ui, p, t("Mapp"), self.ff.scope == Scope::Folder, if has_root { t("Sök i hela arbetsmappen") } else { t("Öppna en mapp först") }).clicked() {
                        self.ff.scope = Scope::Folder;
                    }
                    if crate::workspace::chip(ui, p, t("Flikar"), self.ff.scope == Scope::Tabs, t("Sök i alla öppna flikar")).clicked() {
                        self.ff.scope = Scope::Tabs;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if crate::icons::button(ui, p, crate::icons::Icon::Close, 24.0, false, t("Stäng (Esc)"), p.editor).clicked() {
                            close = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let field_w = (ui.available_width() - 26.0 * 3.0 - 100.0 - 24.0).clamp(120.0, 420.0);
                    let q = ui.add(
                        TextEdit::singleline(&mut self.ff.query)
                            .id(Id::unique("kladd-ff-query"))
                            .hint_text(t("Sök"))
                            .desired_width(field_w)
                            .margin(Margin::symmetric(8, 5)),
                    );
                    if let Some(e) = &self.ff.error {
                        ui.painter().rect_stroke(q.rect, 4.0, Stroke::new(1.5, p.error), egui::StrokeKind::Inside);
                        q.clone().on_hover_text(tf!("Ogiltigt reguljärt uttryck: {e}", e = e));
                    }
                    if self.ff.focus {
                        q.request_focus();
                        self.ff.focus = false;
                    }
                    if q.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        run = true;
                        q.request_focus();
                    }
                    if crate::icons::button(ui, p, crate::icons::Icon::CaseSensitive, 26.0, self.ff.case_sensitive, t("Skilj på stora och små bokstäver"), p.editor).clicked() {
                        self.ff.case_sensitive = !self.ff.case_sensitive;
                    }
                    if text_toggle(ui, p, "ab", self.ff.whole_word, t("Hela ord")).clicked() {
                        self.ff.whole_word = !self.ff.whole_word;
                    }
                    if text_toggle(ui, p, ".*", self.ff.regex, t("Reguljära uttryck (ersättning kan använda $1 och ${namn})")).clicked() {
                        self.ff.regex = !self.ff.regex;
                    }
                    if running {
                        if ui.add(quiet_button(t("Avbryt"), p).min_size(vec2(84.0, 28.0))).clicked() {
                            cancel = true;
                        }
                    } else if ui.add(quiet_button(t("Sök"), p).min_size(vec2(84.0, 28.0))).clicked() {
                        run = true;
                    }
                    let label = if self.ff.show_replace { t("Dölj ersätt") } else { t("Ersätt…") };
                    if ui.add(quiet_button(label, p).min_size(vec2(0.0, 28.0))).clicked() {
                        self.ff.show_replace = !self.ff.show_replace;
                    }
                });
                if self.ff.show_replace {
                    ui.horizontal(|ui| {
                        ui.add(
                            TextEdit::singleline(&mut self.ff.replacement)
                                .id(Id::unique("kladd-ff-replace"))
                                .hint_text(t("Ersätt med"))
                                .desired_width(ui.available_width().clamp(120.0, 420.0) - 120.0)
                                .margin(Margin::symmetric(8, 5)),
                        );
                        let ok = self.ff.total() > 0 && !running && self.ff.result_spec.is_some();
                        if ui.add_enabled(ok, quiet_button(t("Ersätt alla"), p).min_size(vec2(104.0, 28.0))).clicked() {
                            self.ff.confirm_replace = true;
                        }
                    });
                }
                if !self.ff.status.is_empty() {
                    let color = if self.ff.error.is_some() { p.error } else { p.muted };
                    ui.label(RichText::new(&self.ff.status).size(12.0).color(color));
                }
                let mut rows: Vec<Row> = Vec::new();
                for (fi, f) in self.ff.results.iter().enumerate() {
                    rows.push(Row::File(fi));
                    if !self.ff.collapsed.contains(&fi) {
                        rows.extend((0..f.hits.len()).map(|hi| Row::Hit(fi, hi)));
                    }
                }
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::vertical().id_salt("kladd-ff-scroll").auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
                    for row in &rows[range] {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 4.0, p.surface_hover);
                        }
                        let painter = ui.painter().with_clip_rect(rect);
                        match *row {
                            Row::File(fi) => {
                                let f = &self.ff.results[fi];
                                let arrow = if self.ff.collapsed.contains(&fi) { "▸" } else { "▾" };
                                painter.text(pos2(rect.left() + 4.0, rect.center().y), Align2::LEFT_CENTER, arrow, FontId::new(11.0, FontFamily::Proportional), p.muted);
                                let name_job = file_label(&f.label, p);
                                let galley = painter.layout_job(name_job);
                                let w = galley.size().x;
                                painter.galley(pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0), galley, p.text);
                                painter.text(
                                    pos2(rect.left() + 20.0 + w + 8.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    f.hits.len().to_string(),
                                    FontId::new(11.5, FontFamily::Proportional),
                                    p.muted,
                                );
                                if resp.clicked() {
                                    toggle_file = Some(fi);
                                }
                            }
                            Row::Hit(fi, hi) => {
                                let f = &self.ff.results[fi];
                                let h = &f.hits[hi];
                                painter.text(
                                    pos2(rect.left() + 56.0, rect.center().y),
                                    Align2::RIGHT_CENTER,
                                    h.line.to_string(),
                                    FontId::new(12.0, FontFamily::Monospace),
                                    p.faint,
                                );
                                let galley = painter.layout_job(preview_job(h, p));
                                painter.galley(pos2(rect.left() + 66.0, rect.center().y - galley.size().y / 2.0), galley, p.muted);
                                if resp.clicked() {
                                    goto = Some((f.target.clone(), h.start, h.end));
                                }
                            }
                        }
                    }
                });
            });

        if ui.input(|i| i.key_pressed(Key::Escape)) && self.ff.job.is_none() && !self.ff.confirm_replace && ctx.memory(|m| m.focused()).is_some_and(|id| id == Id::unique("kladd-ff-query") || id == Id::unique("kladd-ff-replace")) {
            close = true;
        }
        if let Some(fi) = toggle_file {
            if !self.ff.collapsed.remove(&fi) {
                self.ff.collapsed.insert(fi);
            }
        }
        if cancel {
            self.cancel_find_files();
            self.ff.status = tf!("Avbröts. {hits} hittills", hits = count_hits(self.ff.total()));
        }
        if run {
            self.start_find_files(&ctx);
        }
        if close {
            self.close_find_files();
        }
        if let Some((t, s, e)) = goto {
            self.goto_hit(&ctx, &t, s, e);
        }
        self.replace_confirm_dialog(&ctx, p);
    }

    fn replace_confirm_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.ff.confirm_replace {
            return;
        }
        let (hits, files) = (self.ff.total(), self.ff.results.len());
        let to = self.ff.replacement.clone();
        let mut choice: Option<bool> = None;
        let modal = egui::Modal::new(Id::unique("kladd-ff-confirm"))
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
                ui.label(RichText::new(tf!("Ersätt {hits} i {files}?", hits = count_hits(hits), files = count_files(files))).font(FontId::new(17.0, theme::semibold())).color(p.text));
                ui.add_space(6.0);
                let what = if to.is_empty() { t("Träffarna tas bort").to_owned() } else { tf!("Träffarna ersätts med \"{text}\"", text = crate::app::truncate(&to, 40)) };
                let how = match self.ff.scope {
                    Scope::Folder => t("Filer på disk skrivs över och kan inte ångras här. Öppna flikar ändras i stället i redigeraren (och sparas om de inte hade osparade ändringar)."),
                    Scope::Tabs => t("Flikarna ändras i redigeraren och kan ångras där; spara dem själv."),
                };
                ui.label(RichText::new(what).color(p.muted));
                ui.add_space(4.0);
                ui.label(RichText::new(how).size(12.5).color(p.muted));
                ui.add_space(20.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let go = egui::Button::new(RichText::new(t("Ersätt alla")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .min_size(vec2(104.0, 32.0));
                    if ui.add(go).clicked() {
                        choice = Some(true);
                    }
                    if ui.add(quiet_button(t("Avbryt"), p).min_size(vec2(0.0, 32.0))).clicked() {
                        choice = Some(false);
                    }
                });
            });
        if modal.should_close() && choice.is_none() {
            choice = Some(false);
        }
        match choice {
            Some(true) => {
                self.ff.confirm_replace = false;
                self.replace_in_all(ctx);
            }
            Some(false) => self.ff.confirm_replace = false,
            None => {}
        }
    }
}

/// "1 träff" / "{n} träffar" (och engelska motsvarigheter).
fn count_hits(n: usize) -> String {
    if n == 1 { t("1 träff").to_owned() } else { tf!("{count} träffar", count = n) }
}

fn count_files(n: usize) -> String {
    if n == 1 { t("1 fil").to_owned() } else { tf!("{count} filer", count = n) }
}

fn file_label(label: &str, p: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let (dir, name) = match label.rfind('/') {
        Some(i) => (&label[..=i], &label[i + 1..]),
        None => ("", label),
    };
    job.append(dir, 0.0, TextFormat { font_id: FontId::new(12.5, FontFamily::Proportional), color: p.muted, ..Default::default() });
    job.append(name, 0.0, TextFormat { font_id: FontId::new(13.0, theme::semibold()), color: p.text, ..Default::default() });
    job
}

fn preview_job(h: &Hit, p: &Palette) -> LayoutJob {
    let font = FontId::new(12.5, FontFamily::Monospace);
    let plain = TextFormat { font_id: font.clone(), color: p.muted, ..Default::default() };
    let hit = TextFormat { font_id: font, color: p.text, background: p.match_bg, ..Default::default() };
    let mut job = LayoutJob::default();
    let (a, b) = (h.pstart.min(h.preview.len()), h.pend.min(h.preview.len()));
    job.append(&h.preview[..a], 0.0, plain.clone());
    job.append(&h.preview[a..b], 0.0, hit);
    job.append(&h.preview[b..], 0.0, plain);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(q: &str) -> Spec {
        Spec { query: q.into(), ..Default::default() }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kladd-ff-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn collect(source: Source, s: &Spec) -> (Vec<FileHits>, usize, usize) {
        let mut found = Vec::new();
        let (mut files, mut skipped) = (0, 0);
        run_search(source, s, &AtomicBool::new(false), |m| match m {
            Msg::Found(f) => found.push(f),
            Msg::Done { files: f, skipped: s, .. } => {
                files = f;
                skipped = s;
            }
        });
        (found, files, skipped)
    }

    #[test]
    fn hits_have_lines_and_previews() {
        let text = "först\n    andra rad med nål\r\ntredje\nnål nål";
        let hits = hits_in_text(text, &spec("nål")).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!((hits[0].line, hits[1].line, hits[2].line), (2, 4, 4));
        // Indraget klipps bort, CR också, och träffen pekar rätt i förhandsvisningen.
        assert_eq!(hits[0].preview, "andra rad med nål");
        assert_eq!(&hits[0].preview[hits[0].pstart..hits[0].pend], "nål");
        assert_eq!(&text[hits[0].start..hits[0].end], "nål");
        assert_eq!(&hits[2].preview[hits[2].pstart..hits[2].pend], "nål");
    }

    #[test]
    fn long_lines_are_windowed_on_char_boundaries() {
        let text = format!("{}ÅÄÖ-hit-{}", "å".repeat(300), "ö".repeat(500));
        let hits = hits_in_text(&text, &spec("hit")).unwrap();
        let h = &hits[0];
        assert!(h.preview.starts_with('…') && h.preview.ends_with('…'));
        assert_eq!(&h.preview[h.pstart..h.pend], "hit");
        assert!(h.preview.chars().count() < PREVIEW_CHARS * 2 + 5);
    }

    #[test]
    fn options_and_invalid_regex() {
        let text = "Katt katten KATT";
        let n = |s: Spec| hits_in_text(text, &s).unwrap().len();
        assert_eq!(n(spec("katt")), 3);
        assert_eq!(n(Spec { case_sensitive: true, ..spec("katt") }), 1);
        assert_eq!(n(Spec { whole_word: true, ..spec("katt") }), 2);
        assert_eq!(n(Spec { regex: true, ..spec("k+att") }), 3);
        assert!(hits_in_text(text, &Spec { regex: true, ..spec("(") }).is_err());
    }

    #[test]
    fn dir_search_skips_binary_huge_ignored_and_non_utf8() {
        let d = tmp("search");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("node_modules")).unwrap();
        std::fs::write(d.join("src/a.txt"), "hej nål\nigen").unwrap();
        std::fs::write(d.join("b.txt"), "ingenting").unwrap();
        std::fs::write(d.join("bin.dat"), "nål\0\0nål".as_bytes()).unwrap();
        std::fs::write(d.join("latin1.txt"), b"n\xe5l").unwrap();
        std::fs::write(d.join("huge.txt"), "nål".repeat(1_000_000)).unwrap();
        std::fs::write(d.join("node_modules/x.js"), "nål").unwrap();
        std::fs::write(d.join("ign.txt"), "nål").unwrap();
        std::fs::write(d.join(".gitignore"), "ign.txt\n").unwrap();
        let (found, files, skipped) = collect(Source::Dir { root: d.clone(), overrides: HashMap::new() }, &spec("nål"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "src/a.txt");
        assert_eq!(found[0].hits[0].line, 1);
        // a.txt, b.txt, bin, latin1 och huge lästes eller hoppades över (.gitignore är dold).
        assert_eq!(files, 5);
        assert_eq!(skipped, 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn overrides_replace_disk_content() {
        let d = tmp("override");
        let f = d.join("a.txt");
        std::fs::write(&f, "gammal").unwrap();
        let mut ov = HashMap::new();
        ov.insert(f.clone(), "ny text med nål".to_owned());
        let (found, _, _) = collect(Source::Dir { root: d.clone(), overrides: ov }, &spec("nål"));
        assert_eq!(found.len(), 1);
        let (none, _, _) = collect(Source::Dir { root: d.clone(), overrides: HashMap::new() }, &spec("nål"));
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tabs_search() {
        let tabs = vec![(1, "a".to_owned(), "x nål".to_owned()), (2, "b".to_owned(), "inget".to_owned())];
        let (found, files, _) = collect(Source::Tabs(tabs), &spec("nål"));
        assert_eq!(files, 2);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].target, Target::Tab(1));
    }

    #[test]
    fn cancel_stops_search() {
        let d = tmp("cancel");
        std::fs::write(d.join("a.txt"), "nål").unwrap();
        let mut got = 0;
        run_search(Source::Dir { root: d.clone(), overrides: HashMap::new() }, &spec("nål"), &AtomicBool::new(true), |m| {
            if matches!(m, Msg::Found(_)) {
                got += 1;
            }
        });
        assert_eq!(got, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn replace_across_files() {
        let d = tmp("replace");
        let (a, b, c) = (d.join("a.txt"), d.join("b.txt"), d.join("c.txt"));
        std::fs::write(&a, "foo och foo\r\nfoo").unwrap();
        std::fs::write(&b, "bar").unwrap();
        std::fs::write(&c, b"foo\0bin").unwrap();
        let r = replace_in_files(&[a.clone(), b.clone(), c.clone()], &spec("foo"), "baz");
        assert_eq!(r[0].1, Ok(3));
        assert_eq!(r[1].1, Ok(0));
        assert!(r[2].1.is_err());
        // Radslut bevaras och orörda filer lämnas som de var.
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "baz och baz\r\nbaz");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "bar");
        assert_eq!(std::fs::read(&c).unwrap(), b"foo\0bin");
        // Reguljära uttryck med fångstgrupper.
        let re = Spec { regex: true, ..spec("(b)(a)(z)") };
        let r = replace_in_files(&[a.clone()], &re, "$3$2$1");
        assert_eq!(r[0].1, Ok(3));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "zab och zab\r\nzab");
        let _ = std::fs::remove_dir_all(&d);
    }
}
