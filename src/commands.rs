//! Kommandoregistret: alla appens kommandon på ett ställe. Menyraden (`menubar.rs`), hamburgermenyn
//! och kommandopaletten (`palette.rs`) byggs ur `ALL`. Hur man lägger till ett kommando står i
//! `docs/dev/kommandon.md`.
//!
//! Filen är en undermodul till `app` (se `#[path]` i `app.rs`) för att kunna anropa appens privata
//! funktioner direkt.

use std::path::Path;
use std::sync::mpsc::channel;

use super::{DialogResult, KladdApp, PdfAction, PdfOrgOp, Seek, Settings, DEFAULT_FONT_SIZE};
use crate::i18n::{self, Lang, t};
use crate::panes::Orient;
use crate::tf;
use crate::word::markup::{self, Block, Inline};
use crate::word::reviewui::ReviewAct;

/// Menyn och gruppen i paletten ett kommando hör till. Ordningen är menyradens ordning.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    File,
    Edit,
    View,
    Insert,
    Format,
    Tools,
    Help,
}

impl Category {
    pub const ALL: [Category; 7] =
        [Category::File, Category::Edit, Category::View, Category::Insert, Category::Format, Category::Tools, Category::Help];

    /// Svenska namnet (nyckel för `t`).
    pub const fn name(self) -> &'static str {
        match self {
            Category::File => "Arkiv",
            Category::Edit => "Redigera",
            Category::View => "Visa",
            Category::Insert => "Infoga",
            Category::Format => "Format",
            Category::Tools => "Verktyg",
            Category::Help => "Hjälp",
        }
    }

    pub fn label(self) -> &'static str {
        t(self.name())
    }
}

/// Var ett kommando hör hemma. Menyerna döljer kommandon utanför aktuellt läge; paletten visar
/// dem nedtonade.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Any,
    /// Dokumentläget (sidvyn).
    Word,
    /// PDF-visaren.
    Pdf,
}

/// Ögonblicksbild av det som `enabled` och `checked` behöver veta om appen. Byggs en gång per bild
/// av [`Env::of`]; att den inte håller hela `KladdApp` gör att logiken kan enhetstestas.
pub struct Env<'a> {
    pub settings: &'a Settings,
    pub has_doc: bool,
    pub pdf: bool,
    pub word: bool,
    /// Markdown-baserad flik (även Word- och PDF-text).
    pub markdown: bool,
    /// Texten kan visas som dokument (Word/ODT/PDF-text).
    pub doc_capable: bool,
    pub has_path: bool,
    pub split: Option<Orient>,
    pub is_split: bool,
    pub find_open: bool,
    pub tracking: bool,
    pub changes: usize,
}

impl<'a> Env<'a> {
    pub fn of(app: &'a KladdApp) -> Self {
        let doc = app.docs.get(app.active);
        Env {
            settings: &app.settings,
            has_doc: doc.is_some(),
            pdf: app.pdf_mode(),
            word: app.word_mode(),
            markdown: doc.is_some_and(|d| d.is_markdown()),
            doc_capable: doc.is_some_and(|d| d.pdfv.is_none() && (d.docx || d.pdf)),
            has_path: doc.is_some_and(|d| d.path.is_some()),
            split: app.panes.orient,
            is_split: app.panes.is_split(),
            find_open: app.find.open,
            tracking: app.review_tracking(),
            changes: app.review_changes(),
        }
    }

    /// Ett läge utan dokument, för tester.
    #[cfg(test)]
    pub fn empty(settings: &'a Settings) -> Self {
        Env {
            settings,
            has_doc: false,
            pdf: false,
            word: false,
            markdown: false,
            doc_capable: false,
            has_path: false,
            split: None,
            is_split: false,
            find_open: false,
            tracking: false,
            changes: 0,
        }
    }
}

type EnabledFn = fn(&Env) -> bool;
type RunFn = fn(&mut KladdApp, &egui::Context);

/// Ett kommando. Registret `ALL` är en lista av dessa; se `docs/dev/kommandon.md`.
pub struct Command {
    /// Stabilt id (sparas i "senaste kommandon"): ändra aldrig ett befintligt.
    pub id: &'static str,
    /// Svenska etiketten; nyckel för `t` och i `i18n/en_menus.rs`.
    pub label: &'static str,
    pub cat: Category,
    /// Kortkommando som text ("Ctrl+Shift+S"), tomt om inget.
    pub shortcut: &'static str,
    pub scope: Scope,
    pub enabled: EnabledFn,
    /// `Some` för kommandon med bock (växlare).
    pub checked: Option<EnabledFn>,
    pub run: RunFn,
    /// Avdelare före raden i menyn.
    pub sep: bool,
    /// Visas i menyerna. Falskt = bara i paletten.
    pub menu: bool,
}

fn always(_: &Env) -> bool {
    true
}

impl Command {
    pub const fn new(id: &'static str, label: &'static str, cat: Category, shortcut: &'static str, enabled: EnabledFn, run: RunFn) -> Self {
        Command { id, label, cat, shortcut, scope: Scope::Any, enabled, checked: None, run, sep: false, menu: true }
    }

    pub const fn sep(mut self) -> Self {
        self.sep = true;
        self
    }

    pub const fn checked(mut self, f: EnabledFn) -> Self {
        self.checked = Some(f);
        self
    }

    pub const fn scope(mut self, s: Scope) -> Self {
        self.scope = s;
        self
    }

    pub const fn palette_only(mut self) -> Self {
        self.menu = false;
        self
    }

    /// Etiketten på aktuellt språk.
    pub fn label(&self) -> &'static str {
        t(self.label)
    }

    /// Etiketten på ett visst språk (paletten söker i båda).
    pub fn label_in(&self, lang: Lang) -> &'static str {
        i18n::translate(self.label, lang)
    }

    /// Hör kommandot hemma i aktuellt läge?
    pub fn in_scope(&self, env: &Env) -> bool {
        match self.scope {
            Scope::Any => true,
            Scope::Word => env.word,
            Scope::Pdf => env.pdf,
        }
    }

    /// Kan kommandot köras nu?
    pub fn available(&self, env: &Env) -> bool {
        self.in_scope(env) && (self.enabled)(env)
    }

    pub fn is_checked(&self, env: &Env) -> Option<bool> {
        self.checked.map(|f| f(env))
    }
}

/// Kommandona i en meny, i registrets ordning, utan det som inte hör till läget.
pub fn menu_items<'a>(cat: Category, env: &'a Env<'a>) -> impl Iterator<Item = &'static Command> + 'a {
    ALL.iter().filter(move |c| c.cat == cat && c.menu && c.in_scope(env))
}

#[cfg(test)]
pub fn find(id: &str) -> Option<&'static Command> {
    ALL.iter().find(|c| c.id == id)
}

/// Gränssnittets tillstånd för paletten och inställningsfönstret.
#[derive(Default)]
pub struct State {
    pub palette: Option<crate::palette::Palette>,
    pub settings: Option<crate::settings_ui::Window>,
}

impl KladdApp {
    /// Kör ett kommando om det kan köras nu, och minns det som senast använt.
    pub(crate) fn run_command(&mut self, ctx: &egui::Context, cmd: &'static Command) {
        let ok = cmd.available(&Env::of(self));
        if !ok {
            return;
        }
        let rec = &mut self.settings.recent_commands;
        rec.retain(|i| i != cmd.id);
        rec.insert(0, cmd.id.to_owned());
        rec.truncate(crate::palette::MAX_RECENT);
        (cmd.run)(self, ctx);
        ctx.request_repaint();
    }

    /// Spara-kommandot: en PDF sparas över bara när sidor ändrats, annars blir det en kopia.
    fn cmd_save(&mut self, ctx: &egui::Context) {
        if self.pdf_mode() {
            if self.pdf_unsaved() {
                self.pdf_save(ctx, self.active, false, false);
            } else {
                self.request_save_copy(ctx, self.active);
            }
        } else {
            self.save_doc(ctx, self.active, false, false);
        }
    }

    /// Exporterar till webbsida eller e-bok: som "Spara som" men med bara det filformatet.
    fn request_export(&mut self, ctx: &egui::Context, ext: &'static str) {
        if self.dialog.is_some() {
            self.notify(ctx, t("Stäng den öppna fildialogen först."), true);
            return;
        }
        let Some(doc) = self.docs.get(self.active) else { return };
        let (tx, rx) = channel();
        self.dialog = Some(rx);
        let ctx = ctx.clone();
        let doc_id = doc.id;
        let stem = doc.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned());
        let name = format!("{}.{ext}", stem.unwrap_or_else(|| doc.title()));
        let dir = doc.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(|| self.start_dir());
        std::thread::spawn(move || {
            let filter = if ext == "epub" { t("E-bok (.epub)") } else { t("Webbsida (.html)") };
            let mut dlg = rfd::FileDialog::new().set_title(t("Exportera")).set_file_name(name).add_filter(filter, &[ext]);
            if let Some(dir) = dir {
                dlg = dlg.set_directory(dir);
            }
            let _ = tx.send(DialogResult::SaveAs { doc_id, path: dlg.save_file(), then_close: false, exit: false });
            ctx.request_repaint();
        });
    }

    fn cmd_zoom(&mut self, dir: f32) {
        if self.word_mode() { self.word_zoom_step(dir) } else { self.zoom(dir) }
    }

    fn cmd_set_lang(&mut self, ctx: &egui::Context, l: i18n::LangSetting) {
        self.settings.lang = l;
        // Gäller från nästa bild utan omstart.
        i18n::set_lang(l.resolve());
        ctx.request_repaint();
    }

    fn cmd_align(&mut self, ctx: &egui::Context, a: markup::Align) {
        // En vald bild justeras som sitt stycke, oavsett var markören står.
        let img = self.wv.img.range.clone();
        self.word_edit(ctx, |t, s| markup::set_align(t, img.unwrap_or(s), a));
    }

    fn cmd_pdf_rotate(&mut self, ctx: &egui::Context, clockwise: bool) {
        let Some(tab) = self.pdf_tab_mut() else { return };
        let page = tab.view.current_page();
        self.pdf_organize(ctx, PdfOrgOp::Rotate { pages: vec![page], clockwise });
    }

    fn cmd_pdf_append(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.pdf_tab_mut() else { return };
        let n = tab.view.page_count();
        self.pdf_organize(ctx, PdfOrgOp::InsertAt(n));
    }

    fn cmd_about(&mut self, ctx: &egui::Context) {
        self.notify(ctx, tf!("Kladd {version}", version = crate::update::CURRENT), false);
    }
}

use Category::{Edit, File, Format, Help, Insert, Tools, View};

/// Alla kommandon. Menyernas ordning och avdelare följer den här listan.
pub static ALL: &[Command] = &[
    // ---------------------------------------------------------------- Arkiv
    Command::new("file.new", "Ny flik", File, "Ctrl+T", always, |a, _| a.new_tab()),
    Command::new("file.open", "Öppna…", File, "Ctrl+O", always, |a, c| a.request_open(c)),
    Command::new("file.save", "Spara", File, "Ctrl+S", |e| e.has_doc, |a, c| a.cmd_save(c)),
    Command::new("file.save_as", "Spara som…", File, "Ctrl+Shift+S", |e| e.has_doc && !e.pdf, |a, c| a.request_save_as(c, a.active, false, false)),
    Command::new("file.save_copy", "Spara kopia…", File, "Ctrl+Shift+S", |e| e.pdf, |a, c| a.request_save_copy(c, a.active)),
    Command::new("file.export_html", "Exportera som webbsida…", File, "", |e| e.markdown && !e.pdf, |a, c| a.request_export(c, "html")),
    Command::new("file.export_epub", "Exportera som e-bok…", File, "", |e| e.markdown && !e.pdf, |a, c| a.request_export(c, "epub")),
    Command::new("file.print", "Skriv ut…", File, "Ctrl+P", |e| e.has_doc, |a, c| a.open_print(c)),
    Command::new("file.history", "Versionshistorik…", File, "", |e| e.has_path && !e.pdf, |a, c| a.open_history(c)),
    Command::new("file.open_folder", "Öppna mapp…", File, "", always, |a, c| a.request_folder(c)).sep(),
    Command::new("file.close_folder", "Stäng mapp", File, "", |e| e.settings.workspace.is_some(), |a, _| a.close_workspace()),
    Command::new("file.quick_open", "Snabböppna…", File, "Ctrl+E", always, |a, c| a.open_quick(c)),
    Command::new("file.close_tab", "Stäng flik", File, "Ctrl+W", |e| e.has_doc, |a, _| a.request_close_tab(a.active)).sep(),
    Command::new("file.quit", "Avsluta", File, "Ctrl+Q", always, |_, c| c.send_viewport_cmd(egui::ViewportCommand::Close)).sep(),
    // -------------------------------------------------------------- Redigera
    Command::new("edit.undo", "Ångra", Edit, "Ctrl+Z", |e| e.has_doc, |a, c| a.word_undo(c, false)).scope(Scope::Word),
    Command::new("edit.redo", "Gör om", Edit, "Ctrl+Y", |e| e.has_doc, |a, c| a.word_undo(c, true)).scope(Scope::Word),
    Command::new("edit.pdf_undo", "Ångra sidändring", Edit, "Ctrl+Z", |e| e.pdf, |a, c| a.pdf_organize(c, PdfOrgOp::Undo)).scope(Scope::Pdf),
    Command::new("edit.pdf_redo", "Gör om sidändring", Edit, "Ctrl+Shift+Z", |e| e.pdf, |a, c| a.pdf_organize(c, PdfOrgOp::Redo)).scope(Scope::Pdf),
    Command::new("edit.find", "Sök", Edit, "Ctrl+F", |e| e.has_doc, |a, c| a.open_find(c, false)).sep(),
    Command::new("edit.replace", "Sök och ersätt", Edit, "Ctrl+H", |e| e.has_doc && !e.pdf, |a, c| a.open_find(c, true)),
    Command::new("edit.find_next", "Nästa träff", Edit, "F3", |e| e.find_open && !e.pdf, |a, c| a.seek(c, Seek::Next)),
    Command::new("edit.find_prev", "Föregående träff", Edit, "Shift+F3", |e| e.find_open && !e.pdf, |a, c| a.seek(c, Seek::Prev)),
    Command::new("edit.find_files", "Sök i filer…", Edit, "Ctrl+Shift+F", always, |a, c| a.open_find_files(c)),
    // ------------------------------------------------------------------ Visa
    Command::new("view.split_v", "Dela vyn", View, "Ctrl+\\", |e| e.has_doc, |a, _| a.toggle_split(Orient::Vertical)).checked(|e| e.split == Some(Orient::Vertical)),
    Command::new("view.split_h", "Dela vågrätt", View, "Ctrl+Shift+\\", |e| e.has_doc, |a, _| a.toggle_split(Orient::Horizontal)).checked(|e| e.split == Some(Orient::Horizontal)),
    Command::new("view.other_pane", "Växla ruta", View, "F6", |e| e.is_split, |a, _| a.focus_pane(1 - a.panes.active)),
    Command::new("view.wrap", "Radbrytning", View, "Alt+Z", always, |a, _| a.settings.wrap = !a.settings.wrap).checked(|e| e.settings.wrap).sep(),
    Command::new("view.line_numbers", "Radnummer", View, "", always, |a, _| a.settings.line_numbers = !a.settings.line_numbers).checked(|e| e.settings.line_numbers),
    Command::new("view.nav_pane", "Navigeringsfönster", View, "Ctrl+Shift+O", |e| e.markdown, |a, _| a.settings.nav_pane = !a.settings.nav_pane).checked(|e| e.settings.nav_pane),
    Command::new("view.preview", "Förhandsvisa Markdown", View, "Ctrl+Shift+M", always, |a, _| a.settings.preview = !a.settings.preview).checked(|e| e.settings.preview),
    Command::new("view.workspace", "Mappträd", View, "Ctrl+Shift+E", always, |a, c| a.toggle_workspace_panel(c)).checked(|e| e.settings.workspace_panel),
    Command::new("view.document", "Visa som dokument", View, "", |e| e.doc_capable, |a, _| {
        if let Some(d) = a.docs.get_mut(a.active) {
            d.word_view = !d.word_view;
        }
        a.focus_editor = true;
    })
    .checked(|e| e.word),
    Command::new("view.theme", "Mörkt tema", View, "Ctrl+Shift+K", always, |a, c| a.toggle_theme(c)).checked(|e| e.settings.dark).sep(),
    Command::new("view.zoom_in", "Större text", View, "Ctrl++", always, |a, _| a.cmd_zoom(1.0)).sep(),
    Command::new("view.zoom_out", "Mindre text", View, "Ctrl+−", always, |a, _| a.cmd_zoom(-1.0)),
    Command::new("view.zoom_reset", "Normal textstorlek", View, "Ctrl+0", always, |a, _| {
        if a.word_mode() { a.settings.word_zoom = 1.0 } else { a.settings.font_size = DEFAULT_FONT_SIZE }
    }),
    Command::new("view.pdf_sidebar", "Sidofält", View, "F4", |e| e.pdf, |a, _| {
        if let Some(t) = a.pdf_tab_mut() {
            t.toggle_sidebar();
        }
    })
    .scope(Scope::Pdf)
    .sep(),
    Command::new("view.pdf_present", "Presentationsläge", View, "F5", |e| e.pdf, |a, _| {
        if let Some(t) = a.pdf_tab_mut() {
            t.present();
        }
    })
    .scope(Scope::Pdf),
    Command::new("view.pdf_goto", "Gå till sida…", View, "Ctrl+G", |e| e.pdf, |a, _| {
        if let Some(t) = a.pdf_tab_mut() {
            t.focus_page_field();
        }
    })
    .scope(Scope::Pdf),
    Command::new("view.pdf_reload", "Läs om från disk", View, "", |e| e.pdf, |a, c| a.pdf_action(c, PdfAction::Reload)).scope(Scope::Pdf),
    // ---------------------------------------------------------------- Infoga
    Command::new("insert.image", "Infoga bild…", Insert, "Ctrl+Shift+I", |e| e.has_doc && !e.pdf, |a, c| a.request_insert_image(c)),
    Command::new("insert.toc", "Infoga innehållsförteckning", Insert, "", |e| e.markdown && !e.pdf, |a, c| a.insert_toc(c)),
    Command::new("insert.table", "Infoga tabell", Insert, "", |e| e.word, |a, c| a.word_edit(c, |t, s| markup::insert_table(t, s.end))).scope(Scope::Word),
    Command::new("insert.footnote", "Infoga fotnot", Insert, "Ctrl+Alt+F", |e| e.word, |a, c| a.word_footnote(c)).scope(Scope::Word),
    Command::new("insert.header", "Sidhuvud…", Insert, "", |e| e.word, |a, _| a.open_header_dialog(false, 1)).scope(Scope::Word).sep(),
    Command::new("insert.footer", "Sidfot…", Insert, "", |e| e.word, |a, _| a.open_header_dialog(true, 1)).scope(Scope::Word),
    Command::new("insert.section_break", "Avsnittsbrytning (ny sida)", Insert, "", |e| e.word, |a, c| a.section_break(c)).scope(Scope::Word),
    Command::new("insert.landscape", "Liggande sidor härifrån", Insert, "", |e| e.word, |a, c| a.section_landscape(c, true)).scope(Scope::Word),
    Command::new("insert.portrait", "Stående sidor härifrån", Insert, "", |e| e.word, |a, c| a.section_landscape(c, false)).scope(Scope::Word),
    Command::new("insert.columns", "Spalter…", Insert, "", |e| e.word, |a, c| a.open_columns_dialog(c)).scope(Scope::Word),
    Command::new("insert.column_break", "Spaltbrytning", Insert, "", |e| e.word, |a, c| a.column_break(c)).scope(Scope::Word),
    // ---------------------------------------------------------------- Format
    Command::new("format.bold", "Fetstil", Format, "Ctrl+B", |e| e.word, |a, c| a.word_toggle(c, Inline::Bold)).scope(Scope::Word),
    Command::new("format.italic", "Kursiv", Format, "Ctrl+I", |e| e.word, |a, c| a.word_toggle(c, Inline::Italic)).scope(Scope::Word),
    Command::new("format.underline", "Understruken", Format, "Ctrl+U", |e| e.word, |a, c| a.word_toggle(c, Inline::Underline)).scope(Scope::Word),
    Command::new("format.strike", "Genomstruken", Format, "", |e| e.word, |a, c| a.word_toggle(c, Inline::Strike)).scope(Scope::Word),
    Command::new("format.font_bigger", "Större tecken", Format, "Ctrl+]", |e| e.word, |a, c| a.word_font_step(c, 1)).scope(Scope::Word),
    Command::new("format.font_smaller", "Mindre tecken", Format, "Ctrl+[", |e| e.word, |a, c| a.word_font_step(c, -1)).scope(Scope::Word),
    Command::new("format.align_left", "Vänsterjustera", Format, "Ctrl+L", |e| e.word, |a, c| a.cmd_align(c, markup::Align::Left)).scope(Scope::Word).sep(),
    Command::new("format.align_center", "Centrera", Format, "Ctrl+E", |e| e.word, |a, c| a.cmd_align(c, markup::Align::Center)).scope(Scope::Word),
    Command::new("format.align_right", "Högerjustera", Format, "Ctrl+R", |e| e.word, |a, c| a.cmd_align(c, markup::Align::Right)).scope(Scope::Word),
    Command::new("format.align_justify", "Marginaljustera", Format, "Ctrl+J", |e| e.word, |a, c| a.cmd_align(c, markup::Align::Justify)).scope(Scope::Word),
    Command::new("format.normal", "Brödtext", Format, "Ctrl+Alt+0", |e| e.word, |a, c| a.word_block(c, Block::Normal)).scope(Scope::Word).sep(),
    Command::new("format.h1", "Rubrik 1", Format, "Ctrl+Alt+1", |e| e.word, |a, c| a.word_block(c, Block::Heading(1))).scope(Scope::Word),
    Command::new("format.h2", "Rubrik 2", Format, "Ctrl+Alt+2", |e| e.word, |a, c| a.word_block(c, Block::Heading(2))).scope(Scope::Word),
    Command::new("format.h3", "Rubrik 3", Format, "Ctrl+Alt+3", |e| e.word, |a, c| a.word_block(c, Block::Heading(3))).scope(Scope::Word),
    Command::new("format.bullets", "Punktlista", Format, "", |e| e.word, |a, c| a.word_block(c, Block::Bullet)).scope(Scope::Word),
    Command::new("format.numbered", "Numrerad lista", Format, "", |e| e.word, |a, c| a.word_block(c, Block::Numbered)).scope(Scope::Word),
    Command::new("format.quote", "Citat", Format, "", |e| e.word, |a, c| a.word_block(c, Block::Quote)).scope(Scope::Word),
    Command::new("format.styles", "Ändra formatmall…", Format, "", |e| e.word, |a, _| a.open_styles_dialog(None)).scope(Scope::Word).sep(),
    Command::new("format.page_setup", "Sidinställningar…", Format, "", |e| e.word, |a, _| a.open_page_dialog()).scope(Scope::Word),
    // --------------------------------------------------------------- Verktyg
    Command::new("tools.settings", "Inställningar…", Tools, "Ctrl+,", always, |a, _| a.open_settings(None)),
    Command::new("tools.palette", "Kommandopalett…", Tools, "Ctrl+Shift+P", always, |a, _| a.open_palette()),
    Command::new("tools.spell", "Stavningskontroll", Tools, "", always, |a, _| a.settings.spell = !a.settings.spell).checked(|e| e.settings.spell).sep(),
    Command::new("tools.grammar", "Grammatikkontroll", Tools, "", always, |a, _| a.grammar_toggle()).checked(|e| e.settings.grammar),
    Command::new("tools.grammar_settings", "Grammatik och stil…", Tools, "", always, |a, _| a.open_grammar_dialog(false)),
    Command::new("tools.autocorrect", "Autokorrigering…", Tools, "", always, |a, _| a.open_autocorrect_dialog()),
    Command::new("review.track", "Spåra ändringar", Tools, "Ctrl+Shift+E", |e| e.word, |a, c| a.review_act(c, ReviewAct::ToggleTrack)).scope(Scope::Word).checked(|e| e.tracking).sep(),
    Command::new("review.accept", "Godkänn ändring", Tools, "", |e| e.changes > 0, |a, c| a.review_act(c, ReviewAct::Accept)).scope(Scope::Word),
    Command::new("review.reject", "Avvisa ändring", Tools, "", |e| e.changes > 0, |a, c| a.review_act(c, ReviewAct::Reject)).scope(Scope::Word),
    Command::new("review.prev", "Föregående ändring", Tools, "", |e| e.changes > 0, |a, c| a.review_act(c, ReviewAct::Prev)).scope(Scope::Word),
    Command::new("review.next", "Nästa ändring", Tools, "", |e| e.changes > 0, |a, c| a.review_act(c, ReviewAct::Next)).scope(Scope::Word),
    Command::new("review.accept_all", "Godkänn alla", Tools, "", |e| e.changes > 0, |a, c| a.review_act(c, ReviewAct::AcceptAll)).scope(Scope::Word),
    Command::new("review.reject_all", "Avvisa alla", Tools, "", |e| e.changes > 0, |a, c| a.review_act(c, ReviewAct::RejectAll)).scope(Scope::Word),
    Command::new("review.comment", "Ny kommentar", Tools, "Ctrl+Alt+M", |e| e.word, |a, c| a.review_act(c, ReviewAct::NewComment)).scope(Scope::Word).sep(),
    Command::new("review.reply", "Svara", Tools, "", |e| e.word, |a, c| a.review_act(c, ReviewAct::Reply)).scope(Scope::Word),
    Command::new("review.delete_comment", "Ta bort kommentar", Tools, "", |e| e.word, |a, c| a.review_act(c, ReviewAct::DeleteComment)).scope(Scope::Word),
    Command::new("review.show_comments", "Visa kommentarer", Tools, "", |e| e.word, |a, c| a.review_act(c, ReviewAct::ToggleComments)).scope(Scope::Word).checked(|e| e.settings.word_comments),
    Command::new("review.your_name", "Ditt namn…", Tools, "", |e| e.word, |a, c| a.review_act(c, ReviewAct::YourName)).scope(Scope::Word),
    Command::new("pdf.ocr", "Känn igen text (OCR)", Tools, "", |e| e.pdf, |a, c| a.pdf_action(c, PdfAction::Ocr)).scope(Scope::Pdf).sep(),
    Command::new("pdf.edit", "Redigera PDF som text…", Tools, "", |e| e.pdf, |a, _| {
        if let Some(t) = a.pdf_tab_mut() {
            t.confirm_edit = true;
        }
    })
    .scope(Scope::Pdf),
    Command::new("pdf.rotate_cw", "Rotera sidan åt höger", Tools, "", |e| e.pdf, |a, c| a.cmd_pdf_rotate(c, true)).scope(Scope::Pdf),
    Command::new("pdf.rotate_ccw", "Rotera sidan åt vänster", Tools, "", |e| e.pdf, |a, c| a.cmd_pdf_rotate(c, false)).scope(Scope::Pdf),
    Command::new("pdf.append", "Lägg till PDF i slutet…", Tools, "", |e| e.pdf, |a, c| a.cmd_pdf_append(c)).scope(Scope::Pdf),
    Command::new("pdf.copy_page", "Kopiera sidan som bild", Tools, "", |e| e.pdf, |a, c| a.pdf_action(c, PdfAction::CopyImage)).scope(Scope::Pdf),
    Command::new("pdf.external", "Öppna i extern PDF-visare", Tools, "", |e| e.pdf && e.has_path, |a, c| a.pdf_action(c, PdfAction::OpenExternal)).scope(Scope::Pdf),
    // Inställningar som också finns i inställningsfönstret: bara i paletten.
    Command::new("setting.autosave", "Spara automatiskt", Tools, "", always, |a, _| a.settings.autosave = !a.settings.autosave).checked(|e| e.settings.autosave).palette_only(),
    Command::new("setting.recovery", "Säkerhetskopiera osparade ändringar", Tools, "", always, |a, _| a.settings.autosave_recovery = !a.settings.autosave_recovery).checked(|e| e.settings.autosave_recovery).palette_only(),
    Command::new("setting.auto_update", "Sök efter uppdateringar automatiskt", Tools, "", always, |a, _| a.settings.auto_update_check = !a.settings.auto_update_check).checked(|e| e.settings.auto_update_check).palette_only(),
    Command::new("setting.lang_auto", "Språk: Automatiskt", Tools, "", always, |a, c| a.cmd_set_lang(c, i18n::LangSetting::Auto)).palette_only(),
    Command::new("setting.lang_sv", "Språk: Svenska", Tools, "", always, |a, c| a.cmd_set_lang(c, i18n::LangSetting::Sv)).palette_only(),
    Command::new("setting.lang_en", "Språk: English", Tools, "", always, |a, c| a.cmd_set_lang(c, i18n::LangSetting::En)).palette_only(),
    // ------------------------------------------------------------------ Hjälp
    Command::new("help.update", "Sök efter uppdateringar", Help, "", always, |a, c| a.update.check_now(c)),
    Command::new("help.about", "Om Kladd", Help, "", always, |a, c| a.cmd_about(c)).sep(),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_labelled() {
        let mut ids = HashSet::new();
        for c in ALL {
            assert!(ids.insert(c.id), "dubblett av id: {}", c.id);
            assert!(!c.label.trim().is_empty(), "tom etikett: {}", c.id);
            assert!(c.id.contains('.'), "id ska vara grupp.namn: {}", c.id);
        }
    }

    #[test]
    fn every_command_has_both_languages() {
        for c in ALL {
            assert!(i18n::has_en(c.label), "engelsk etikett saknas för {} ({:?}): lägg den i i18n/en_menus.rs", c.id, c.label);
            assert!(!c.label_in(Lang::En).is_empty());
        }
        for cat in Category::ALL {
            assert!(i18n::has_en(cat.name()), "engelskt kategorinamn saknas: {}", cat.name());
        }
    }

    #[test]
    fn every_category_has_menu_commands() {
        for cat in Category::ALL {
            assert!(ALL.iter().any(|c| c.cat == cat && c.menu), "tom meny: {}", cat.name());
        }
    }

    #[test]
    fn covers_the_old_main_menu() {
        for id in [
            "file.new", "file.open", "file.save", "file.save_as", "file.print", "file.history", "file.open_folder", "view.workspace",
            "file.close_folder", "file.quick_open", "file.close_tab", "insert.image", "insert.toc", "edit.find", "edit.replace",
            "edit.find_files", "view.split_v", "view.split_h", "view.wrap", "view.line_numbers", "view.nav_pane", "view.preview",
            "view.theme", "view.zoom_in", "view.zoom_out", "view.zoom_reset", "setting.recovery", "setting.auto_update",
            "help.update", "setting.autosave", "file.quit",
        ] {
            assert!(find(id).is_some(), "kommando saknas: {id}");
        }
    }

    #[test]
    fn enabled_follows_the_mode() {
        let s = Settings::default();
        let none = Env::empty(&s);
        let get = |id| find(id).unwrap();
        assert!(get("file.new").available(&none));
        assert!(!get("file.save").available(&none));
        assert!(!get("edit.find").available(&none));
        assert!(get("edit.find_files").available(&none));

        let text = Env { has_doc: true, ..Env::empty(&s) };
        assert!(get("file.save").available(&text));
        assert!(get("file.save_as").available(&text));
        assert!(!get("file.save_copy").available(&text));
        assert!(!get("view.nav_pane").available(&text), "navigeringsfönstret kräver Markdown");
        assert!(!get("edit.find_next").available(&text), "nästa träff kräver ett öppet sökfält");

        let md = Env { has_doc: true, markdown: true, find_open: true, ..Env::empty(&s) };
        assert!(get("view.nav_pane").available(&md));
        assert!(get("insert.toc").available(&md));
        assert!(get("edit.find_next").available(&md));
        assert!(!get("format.bold").available(&md), "formatering hör till dokumentläget");

        let pdf = Env { has_doc: true, pdf: true, markdown: true, ..Env::empty(&s) };
        assert!(!get("file.save_as").available(&pdf));
        assert!(get("file.save_copy").available(&pdf));
        assert!(!get("insert.image").available(&pdf));
        assert!(get("pdf.ocr").available(&pdf));
        assert!(!get("pdf.ocr").available(&md));

        let word = Env { has_doc: true, word: true, markdown: true, ..Env::empty(&s) };
        assert!(get("format.bold").available(&word));
        assert!(!get("review.accept").available(&word), "inga ändringar att godkänna");
        let changes = Env { changes: 3, ..word };
        assert!(get("review.accept").available(&changes));
    }

    #[test]
    fn menus_hide_other_modes() {
        let s = Settings::default();
        let text = Env { has_doc: true, ..Env::empty(&s) };
        assert_eq!(menu_items(Category::Format, &text).count(), 0, "Format syns bara i dokumentläget");
        let word = Env { has_doc: true, word: true, ..Env::empty(&s) };
        assert!(menu_items(Category::Format, &word).count() > 5);
        assert!(menu_items(Category::File, &text).all(|c| c.menu));
        assert!(menu_items(Category::Tools, &text).all(|c| c.id != "setting.autosave"), "inställningar i paletten syns inte i menyn");
    }

    #[test]
    fn checked_reads_settings() {
        let mut s = Settings::default();
        s.wrap = true;
        let env = Env::empty(&s);
        assert_eq!(find("view.wrap").unwrap().is_checked(&env), Some(true));
        assert_eq!(find("file.new").unwrap().is_checked(&env), None);
        s.wrap = false;
        assert_eq!(find("view.wrap").unwrap().is_checked(&Env::empty(&s)), Some(false));
    }
}
