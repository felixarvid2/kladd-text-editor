//! Läsläget för EPUB: flikens tillstånd och ritning. Ett kapitel (en del) ritas åt gången med
//! läsläget för Markdown (`egui_commonmark`), med en tunn rad överst, en framstegslinje nederst,
//! innehållsförteckning i sidopanelen, sök i hela boken, fotnoter i en liten ruta och en
//! typografimeny (Aa). Designen står i `docs/dev/epub.md`.

use std::collections::HashSet;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Align, Align2, Color32, FontFamily, FontId, Frame, Id, Key, KeyboardShortcut, Layout, Margin, Modal, Modifiers, Order, Panel, Popup,
    PopupCloseBehavior, Rect, RectAlign, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, UiBuilder, pos2, vec2,
};
use crate::mdwin::{Cfg, Win};
use egui_commonmark::{BlockAlign, BlockStyle, CommonMarkCache, CommonMarkViewer, SearchOptions, ViewExtras, set_extras, take_mark_hit};
use serde::{Deserialize, Serialize};

#[path = "annot.rs"]
pub mod annot;

use super::anchor::Note;
use super::html::image_uri;
use super::{Book, Dest};
use annot::{AnnMenu, SideTab};
use crate::app::quiet_button;
use crate::i18n::t;
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};
use crate::tf;

/// Samma nyckel som `egui_commonmark_backend::misc::LINE_FACTOR_ID` i den lappade kopian i `vendor/`.
const LINE_FACTOR_ID: &str = "egui_commonmark_line_factor";
/// Samma nyckel som `BOLD_FAMILY_ID` i `mdread.rs`.
const BOLD_FAMILY_ID: &str = "egui_commonmark_bold_family";

const TOP_H: f32 = 32.0;
const BOTTOM_H: f32 = 56.0;
/// Läshastighet för "tid kvar" (ord per minut).
const WORDS_PER_MINUTE: f32 = 230.0;
/// Högsta antal träffar som samlas vid sökning i boken.
const MAX_HITS: usize = 2000;
/// Högsta antal böcker vars läge sparas.
pub const MAX_MEMORY: usize = 100;

// ------------------------------------------------------------------ inställningar

/// Typografin i läsläget. Gäller alla böcker och sparas med inställningarna.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Textstorlek (punkter).
    pub size: f32,
    /// Sans-serif i stället för serif.
    pub sans: bool,
    /// Radavstånd: 0 kompakt, 1 normal, 2 luftig.
    pub spacing: u8,
    /// Spaltbredd: 0 smal, 1 normal, 2 bred.
    pub width: u8,
    /// Sidfärg: 0 automatisk (följer temat), 1 ljus, 2 sepia, 3 mörk.
    pub page: u8,
    /// Justerad text (rak högerkant).
    pub justify: bool,
    /// Förlagets formatering (centrerade rubriker, indrag, små versaler …).
    pub publisher: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { size: 17.0, sans: false, spacing: 1, width: 1, page: 0, justify: false, publisher: true }
    }
}

pub const SIZE_MIN: f32 = 12.0;
pub const SIZE_MAX: f32 = 34.0;

impl Prefs {
    pub fn line_factor(&self) -> f32 {
        [1.3, 1.5, 1.75][usize::from(self.spacing.min(2))]
    }

    pub fn column(&self) -> f32 {
        [560.0, 700.0, 880.0][usize::from(self.width.min(2))]
    }

    pub fn step_size(&mut self, delta: f32) {
        self.size = (self.size + delta).clamp(SIZE_MIN, SIZE_MAX);
    }

    fn sanitized(&self) -> Prefs {
        Prefs { size: self.size.clamp(SIZE_MIN, SIZE_MAX), ..self.clone() }
    }
}

/// Var i boken man var (per fil).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub section: usize,
    /// Andel (0..1) av delens rullning.
    pub frac: f32,
    /// Delningsgränsen (`SECTION_LIMIT`) som delnumret gäller för; 0 i gamla filer (då 24 000). Ändras gränsen
    /// räknas stället om till rätt del.
    #[serde(default)]
    pub limit: usize,
    /// Källpositionen överst i delen, om den är känd (säkrare än andelen i stora delar).
    #[serde(default)]
    pub byte: Option<usize>,
}

/// Lägger läget först i listan (och tar bort äldre poster för samma fil).
pub fn remember(list: &mut Vec<(String, Memory)>, key: String, m: Memory, max: usize) {
    list.retain(|(k, _)| *k != key);
    list.insert(0, (key, m));
    list.truncate(max);
}

// ------------------------------------------------------------------ färger

/// Läsytans färger efter sidfärgen (Auto följer Kladds tema).
#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub link: Color32,
    pub line: Color32,
    pub dark: bool,
}

pub fn colors(prefs: &Prefs, p: &Palette) -> Colors {
    let rgb = |r, g, b| Color32::from_rgb(r, g, b);
    match prefs.page {
        1 => Colors { bg: rgb(251, 251, 248), text: rgb(34, 34, 34), muted: rgb(116, 116, 110), link: rgb(28, 98, 168), line: rgb(226, 226, 219), dark: false },
        2 => Colors { bg: rgb(244, 236, 216), text: rgb(91, 70, 54), muted: rgb(146, 122, 98), link: rgb(139, 80, 30), line: rgb(222, 208, 178), dark: false },
        3 => Colors { bg: rgb(25, 26, 29), text: rgb(210, 210, 205), muted: rgb(138, 140, 146), link: rgb(124, 172, 232), line: rgb(52, 54, 60), dark: true },
        _ => Colors { bg: p.editor, text: p.text, muted: p.muted, link: p.syntax.function, line: p.line, dark: p.dark },
    }
}

/// Bakgrunden som läsytan fylls med (för panelen runt).
pub fn page_fill(prefs: &Prefs, p: &Palette) -> Color32 {
    colors(prefs, p).bg
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

// ------------------------------------------------------------------ tillstånd

/// Det läsvyn ber appen göra.
pub enum Action {
    /// Öppna bokens text som ett nytt, osparat dokument.
    EditCopy,
}

/// Ett hopp till ett ställe i den visade delen.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Jump {
    Top,
    End,
    Frac(f32),
    Offset(f32),
    /// Byteposition i delens Markdown.
    Byte(usize),
}

#[derive(Clone, Copy, Debug)]
struct Pos {
    section: usize,
    offset: f32,
}

/// En träff i boken.
#[derive(Clone, Debug)]
pub struct Hit {
    pub section: usize,
    /// Träffens ordning bland delens träffar.
    pub nth: usize,
    pub snippet: String,
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub query: String,
    pub case: bool,
    pub hits: Vec<Hit>,
    pub cur: Option<usize>,
    /// Träffarna i delen som visas (för räknaren när listan är kapad).
    capped: bool,
    built_for: Option<(String, bool)>,
    plain: Option<Vec<Vec<String>>>,
    focus: bool,
    /// Delen där sökningen öppnades: första träffen är den första härifrån.
    origin: usize,
}

struct NotePopup {
    md: String,
    dest: Dest,
    /// Öppnades i den här bildrutan: klicket som öppnade den får inte stänga den igen.
    fresh: bool,
}

pub struct EpubTab {
    pub book: Arc<Book>,
    pub section: usize,
    pending: Option<Jump>,
    /// Rullningsläget som ritas nästa bildruta.
    offset_next: Option<f32>,
    /// Delen har ritats minst en gång (innehållets höjd är känd).
    laid_out: bool,
    layout_tries: u8,
    scroll: f32,
    max_scroll: f32,
    view_h: f32,
    /// Där Markdown-texten börjar i rullytans innehåll.
    md_top: f32,
    back: Vec<Pos>,
    fwd: Vec<Pos>,
    cache: CommonMarkCache,
    /// Blockindelningen och höjderna för delen som visas (delens nummer hör till).
    win: Option<(usize, Win)>,
    registered: HashSet<String>,
    hooks_for: Option<usize>,
    applied: Option<(usize, String, bool)>,
    want_nth: Option<usize>,
    pub find: Find,
    note: Option<NotePopup>,
    pub about: bool,
    toc_cur: Option<usize>,
    toc_scroll: bool,
    /// Rullar innehållsförteckningen så att aktuell post syns.
    last_toc_cur: Option<usize>,
    /// Bokens nyckel i biblioteket och dess bokmärken och understrykningar.
    pub key: String,
    pub notes: Vec<Note>,
    notes_gen: u64,
    dirty: bool,
    /// Var posterna står i kapitlens Markdown just nu (post-id och område), giltigt för `resolved_gen`.
    resolved: Vec<(u64, Option<std::ops::Range<usize>>)>,
    resolved_gen: Option<u64>,
    /// Vilken post varje markering i den visade delen hör till.
    mark_ids: Vec<u64>,
    menu: Option<AnnMenu>,
    pub side: SideTab,
    /// Användaren bad om *Exportera anteckningar…* i sidopanelen (appen tar det).
    pub export_requested: bool,
    /// Var musknappen trycktes ned senast och när senaste dubbelklicket var (för att känna igen en markering).
    press_at: Option<egui::Pos2>,
    multi_click_at: Option<f64>,
}

impl EpubTab {
    pub fn new(book: Book) -> Self {
        EpubTab {
            book: Arc::new(book),
            section: 0,
            pending: Some(Jump::Top),
            offset_next: None,
            laid_out: false,
            layout_tries: 0,
            scroll: 0.0,
            max_scroll: 0.0,
            view_h: 600.0,
            md_top: 0.0,
            back: Vec::new(),
            fwd: Vec::new(),
            cache: CommonMarkCache::default(),
            win: None,
            registered: HashSet::new(),
            hooks_for: None,
            applied: None,
            want_nth: None,
            find: Find::default(),
            note: None,
            about: false,
            toc_cur: None,
            toc_scroll: true,
            last_toc_cur: None,
            key: String::new(),
            notes: Vec::new(),
            notes_gen: 0,
            dirty: false,
            resolved: Vec::new(),
            resolved_gen: None,
            mark_ids: Vec::new(),
            menu: None,
            side: SideTab::default(),
            export_requested: false,
            press_at: None,
            multi_click_at: None,
        }
    }

    /// Titeln som fliken visar.
    pub fn title(&self) -> String {
        self.book.title()
    }

    /// Visas menyn vid en markering eller understrykning?
    #[cfg(test)]
    pub fn has_menu(&self) -> bool {
        self.menu.is_some()
    }

    /// Gäller menyn en befintlig understrykning (inte en ny markering)?
    #[cfg(test)]
    pub fn menu_is_existing(&self) -> bool {
        matches!(&self.menu, Some(AnnMenu { target: annot::Target::Existing(_), .. }))
    }

    // ---------------------------------------------------------------- läge

    pub fn memory(&self) -> Memory {
        let frac = if self.max_scroll > 1.0 { (self.scroll / self.max_scroll).clamp(0.0, 1.0) } else { 0.0 };
        let byte = (self.book.sections.get(self.section).is_some_and(|s| s.cover.is_none())).then(|| self.top_byte());
        Memory { section: self.section, frac, limit: super::SECTION_LIMIT, byte }
    }

    pub fn restore(&mut self, m: &Memory) {
        let limit = if m.limit == 0 { super::LEGACY_SECTION_LIMIT } else { m.limit };
        if limit != super::SECTION_LIMIT {
            // Boken delades annorlunda när läget sparades: räkna om stället till rätt del.
            if let Some((s, byte)) = self.book.remap_position(m.section, m.frac, limit) {
                self.section = s;
                self.pending = Some(Jump::Byte(byte));
                self.laid_out = false;
                self.layout_tries = 0;
            }
            return;
        }
        if m.section < self.book.sections.len() {
            self.section = m.section;
            self.pending = Some(match m.byte {
                Some(b) if b > 0 => Jump::Byte(b),
                _ => Jump::Frac(m.frac.clamp(0.0, 1.0)),
            });
            self.laid_out = false;
            self.layout_tries = 0;
        }
    }

    fn view_id(&self, section: usize) -> Id {
        Id::unique(("kladd-epub-md", self.book.uid, section))
    }

    fn here(&self) -> Pos {
        Pos { section: self.section, offset: self.offset_next.unwrap_or(self.scroll) }
    }

    /// Byter del; hoppet görs när delen har ritats.
    fn go_section(&mut self, section: usize, jump: Jump) {
        let section = section.min(self.book.sections.len().saturating_sub(1));
        if section != self.section {
            let old = self.view_id(self.section);
            self.cache.clear_viewer(&old);
            self.section = section;
            self.laid_out = false;
            self.layout_tries = 0;
            self.hooks_for = None;
            self.applied = None;
            self.scroll = 0.0;
            self.offset_next = None;
            self.toc_scroll = true;
        }
        self.pending = Some(jump);
    }

    fn go_dest(&mut self, d: Dest) {
        let jump = if d.byte == 0 { Jump::Top } else { Jump::Byte(d.byte) };
        self.go_section(d.section, jump);
    }

    /// Följer en länk eller ett val i förteckningen: minns var man var, så att Tillbaka går dit.
    pub fn follow(&mut self, d: Dest) {
        let here = self.here();
        self.back.push(here);
        self.fwd.clear();
        self.go_dest(d);
        self.note = None;
    }

    pub fn go_back(&mut self) {
        if let Some(p) = self.back.pop() {
            let here = self.here();
            self.fwd.push(here);
            self.go_section(p.section, Jump::Offset(p.offset));
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(p) = self.fwd.pop() {
            let here = self.here();
            self.back.push(here);
            self.go_section(p.section, Jump::Offset(p.offset));
        }
    }

    /// Delen som följer i läsordningen (nästa kapitels första del när kapitlet är slut).
    fn next_section(&self, s: usize) -> Option<usize> {
        let ch = &self.book.chapters[self.book.sections[s].chapter];
        if s + 1 < ch.first_section + ch.section_count {
            return Some(s + 1);
        }
        let next = self.book.step_chapter(self.book.sections[s].chapter, 1)?;
        Some(self.book.chapters[next].first_section)
    }

    fn prev_section(&self, s: usize) -> Option<usize> {
        let ch = &self.book.chapters[self.book.sections[s].chapter];
        if s > ch.first_section {
            return Some(s - 1);
        }
        let prev = self.book.step_chapter(self.book.sections[s].chapter, -1)?;
        let c = &self.book.chapters[prev];
        Some(c.first_section + c.section_count - 1)
    }

    pub fn next_chapter(&mut self) {
        let ch = self.book.sections[self.section].chapter;
        if let Some(n) = self.book.step_chapter(ch, 1) {
            self.go_section(self.book.chapters[n].first_section, Jump::Top);
        }
    }

    pub fn prev_chapter(&mut self) {
        let ch = self.book.sections[self.section].chapter;
        if let Some(n) = self.book.step_chapter(ch, -1) {
            self.go_section(self.book.chapters[n].first_section, Jump::Top);
        }
    }

    fn scroll_base(&self) -> f32 {
        self.offset_next.unwrap_or(self.scroll)
    }

    fn scroll_by(&mut self, dy: f32) {
        let to = (self.scroll_base() + dy).clamp(0.0, self.max_scroll.max(0.0));
        self.offset_next = Some(to);
    }

    /// En skärm framåt (minus en rad), eller nästa del när delen är slut.
    pub fn page_down(&mut self, line: f32) {
        if self.scroll_base() >= self.max_scroll - 2.0 {
            if let Some(n) = self.next_section(self.section) {
                self.go_section(n, Jump::Top);
            }
        } else {
            self.scroll_by((self.view_h - line * 1.5).max(40.0));
        }
    }

    pub fn page_up(&mut self, line: f32) {
        if self.scroll_base() <= 2.0 {
            if let Some(p) = self.prev_section(self.section) {
                self.go_section(p, Jump::End);
            }
        } else {
            self.scroll_by(-(self.view_h - line * 1.5).max(40.0));
        }
    }

    pub fn book_start(&mut self) {
        self.go_section(0, Jump::Top);
    }

    pub fn book_end(&mut self) {
        let last = self.book.sections.len().saturating_sub(1);
        // Sista delen i läsordningen (extra filer ligger sist i listan).
        let lin = self.book.linear_chapters();
        let s = lin.last().map_or(last, |c| self.book.chapters[*c].first_section + self.book.chapters[*c].section_count - 1);
        self.go_section(s, Jump::End);
    }

    // ---------------------------------------------------------------- framsteg

    fn frac_here(&self) -> f32 {
        if self.max_scroll > 1.0 { (self.scroll / self.max_scroll).clamp(0.0, 1.0) } else { 0.0 }
    }

    /// Ord som räknas i framstegen för en del (bara läsordningens kapitel).
    fn weight(&self, s: usize) -> usize {
        let sec = &self.book.sections[s];
        if self.book.chapters[sec.chapter].linear { sec.words } else { 0 }
    }

    /// Andel (0..1) av boken som är läst.
    pub fn progress(&self) -> f32 {
        let total = self.book.total_words.max(1) as f32;
        let s = &self.book.sections[self.section];
        ((s.words_before as f32 + self.frac_here() * self.weight(self.section) as f32) / total).clamp(0.0, 1.0)
    }

    /// Del och rullningsandel vid en andel av boken.
    fn at_fraction(&self, f: f32) -> (usize, f32) {
        let target = f.clamp(0.0, 1.0) * self.book.total_words as f32;
        let mut last = 0;
        for (i, s) in self.book.sections.iter().enumerate() {
            let w = self.weight(i) as f32;
            if w == 0.0 {
                continue;
            }
            last = i;
            if target <= s.words_before as f32 + w {
                return (i, ((target - s.words_before as f32) / w).clamp(0.0, 1.0));
            }
        }
        (last, 1.0)
    }

    /// Minuter kvar i kapitlet.
    pub fn minutes_left(&self) -> usize {
        let ch = &self.book.chapters[self.book.sections[self.section].chapter];
        let end = ch.first_section + ch.section_count;
        let mut words: f32 = self.book.sections[self.section + 1..end].iter().map(|s| s.words as f32).sum();
        words += (1.0 - self.frac_here()) * self.book.sections[self.section].words as f32;
        (words / WORDS_PER_MINUTE).ceil() as usize
    }

    /// (kapitelnummer, antal kapitel) i läsordningen och kapitlets titel.
    pub fn chapter_label(&self) -> (usize, usize, String) {
        let ci = self.book.sections[self.section].chapter;
        let lin = self.book.linear_chapters();
        let n = lin.iter().position(|c| *c == ci).map_or(0, |p| p + 1);
        (n, lin.len(), self.book.chapters[ci].title.clone())
    }

    // ---------------------------------------------------------------- tangenter

    /// Tangenterna i läsläget.
    pub fn handle_keys(&mut self, ctx: &egui::Context, prefs: &mut Prefs) {
        let hit = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let ctrl = Modifiers::COMMAND;
        let typing = ctx.egui_wants_keyboard_input();
        // Esc stänger det som hör till läsläget; en öppen meny (Aa) får tangenten själv.
        if (self.note.is_some() || self.about || self.find.open) && self.menu.is_none() && hit(Modifiers::NONE, Key::Escape) {
            if self.note.is_some() {
                self.note = None;
            } else if self.about {
                self.about = false;
            } else {
                self.find.open = false;
            }
        }
        if hit(ctrl, Key::Plus) || hit(ctrl, Key::Equals) {
            prefs.step_size(1.0);
        }
        if hit(ctrl, Key::Minus) {
            prefs.step_size(-1.0);
        }
        if hit(ctrl, Key::Num0) {
            prefs.size = Prefs::default().size;
        }
        if hit(ctrl, Key::I) {
            self.about = !self.about;
        }
        // Bokmärke på stället man läser (Ctrl+D, som i Foliate): finns det ett tas det bort.
        if !typing && hit(ctrl, Key::D) {
            self.toggle_bookmark();
        }
        if self.find.open && hit(Modifiers::SHIFT, Key::F3) {
            self.step_hit(true);
        } else if self.find.open && hit(Modifiers::NONE, Key::F3) {
            self.step_hit(false);
        }
        if typing || self.about {
            return;
        }
        let line = prefs.size * prefs.line_factor();
        // Alt först: `consume_shortcut` bryr sig inte om extra Alt/Skift.
        if hit(Modifiers::ALT, Key::ArrowLeft) {
            self.go_back();
        } else if hit(Modifiers::ALT, Key::ArrowRight) {
            self.go_forward();
        } else if hit(Modifiers::ALT, Key::ArrowUp) {
            self.prev_chapter();
        } else if hit(Modifiers::ALT, Key::ArrowDown) {
            self.next_chapter();
        } else if hit(Modifiers::NONE, Key::OpenBracket) {
            self.prev_chapter();
        } else if hit(Modifiers::NONE, Key::CloseBracket) {
            self.next_chapter();
        } else if hit(ctrl, Key::Home) {
            self.book_start();
        } else if hit(ctrl, Key::End) {
            self.book_end();
        } else if hit(Modifiers::SHIFT, Key::Space) || hit(Modifiers::NONE, Key::PageUp) {
            self.page_up(line);
        } else if hit(Modifiers::NONE, Key::Space) || hit(Modifiers::NONE, Key::PageDown) {
            self.page_down(line);
        } else if hit(Modifiers::NONE, Key::ArrowDown) {
            self.scroll_by(line * 2.0);
        } else if hit(Modifiers::NONE, Key::ArrowUp) {
            self.scroll_by(-line * 2.0);
        } else if hit(Modifiers::NONE, Key::Home) {
            self.offset_next = Some(0.0);
        } else if hit(Modifiers::NONE, Key::End) {
            self.offset_next = Some(self.max_scroll.max(0.0));
        }
    }

    // ---------------------------------------------------------------- sök

    pub fn open_find(&mut self) {
        if !self.find.open {
            self.find.origin = self.section;
        }
        self.find.open = true;
        self.find.focus = true;
    }

    /// Bygger träfflistan när frågan ändrats.
    fn update_hits(&mut self) {
        if !self.find.open || self.find.built_for.as_ref() == Some(&(self.find.query.clone(), self.find.case)) {
            return;
        }
        self.find.built_for = Some((self.find.query.clone(), self.find.case));
        self.find.hits.clear();
        self.find.cur = None;
        self.find.capped = false;
        let q = self.find.query.trim().to_owned();
        if q.is_empty() {
            return;
        }
        let book = self.book.clone();
        let plain = self.find.plain.get_or_insert_with(|| book.sections.iter().map(|s| super::plain_lines(&s.md)).collect());
        let needle = if self.find.case { q.clone() } else { q.to_lowercase() };
        'all: for (si, lines) in plain.iter().enumerate() {
            let mut nth = 0;
            for line in lines {
                let hay = if self.find.case { line.clone() } else { line.to_lowercase() };
                let mut from = 0;
                while let Some(p) = hay[from..].find(&needle) {
                    let at = from + p;
                    let snippet = snippet_of(&hay, line, at, needle.len());
                    self.find.hits.push(Hit { section: si, nth, snippet });
                    nth += 1;
                    from = at + needle.len().max(1);
                    if self.find.hits.len() >= MAX_HITS {
                        self.find.capped = true;
                        break 'all;
                    }
                }
            }
        }
        // Börja vid första träffen från där man är.
        let here = self.find.origin;
        let first = self.find.hits.iter().position(|h| h.section >= here).or(if self.find.hits.is_empty() { None } else { Some(0) });
        match first {
            Some(i) => self.goto_hit(i),
            None => self.find.cur = None,
        }
    }

    fn goto_hit(&mut self, i: usize) {
        let Some(h) = self.find.hits.get(i).cloned() else { return };
        self.find.cur = Some(i);
        if h.section != self.section {
            self.go_section(h.section, Jump::Top);
        }
        self.want_nth = Some(h.nth);
        // Träffen i samma del hittas av ritaren (aktiv träff); återställ markeringen.
        self.applied = None;
    }

    pub fn step_hit(&mut self, back: bool) {
        let n = self.find.hits.len();
        if n == 0 {
            return;
        }
        let cur = self.find.cur.unwrap_or(if back { 0 } else { n - 1 });
        let next = if back { (cur + n - 1) % n } else { (cur + 1) % n };
        self.goto_hit(next);
    }

    // ---------------------------------------------------------------- hopp

    /// Räknar ut rullningsläget för ett hopp med det senast ritade (innehållets storlek och blockens lägen).
    fn resolve(&self, j: Jump) -> f32 {
        let max = self.max_scroll.max(0.0);
        match j {
            Jump::Top => 0.0,
            Jump::End => max,
            Jump::Frac(f) => (f * max).clamp(0.0, max),
            Jump::Offset(o) => o.clamp(0.0, max),
            Jump::Byte(b) => match self.win.as_ref().filter(|(s, _)| *s == self.section) {
                Some((_, w)) => (self.md_top + w.y_of_byte(b) - 28.0).clamp(0.0, max),
                None => 0.0,
            },
        }
    }

    // ---------------------------------------------------------------- ritning

    /// Ritar läsytan. `full` är hela ytan (panelen är redan fylld med sidfärgen).
    pub fn show(&mut self, ui: &mut Ui, p: &Palette, prefs: &mut Prefs) -> Option<Action> {
        let ctx = ui.ctx().clone();
        let prefs_now = prefs.sanitized();
        let c = colors(&prefs_now, p);
        let full = ui.max_rect();
        let top = Rect::from_min_max(full.min, pos2(full.right(), full.top() + TOP_H));
        let bottom = Rect::from_min_max(pos2(full.left(), full.bottom() - BOTTOM_H), full.max);
        let mid = Rect::from_min_max(pos2(full.left(), top.bottom()), pos2(full.right(), bottom.top()));
        let mut action = None;
        let book = self.book.clone();
        self.update_hits();

        // Stil för läsytan: typsnitt, storlek, färger.
        let size = prefs_now.size;
        ctx.data_mut(|d| {
            d.insert_temp(Id::unique(LINE_FACTOR_ID), prefs_now.line_factor());
            d.insert_temp(Id::unique(BOLD_FAMILY_ID), if prefs_now.sans { theme::semibold() } else { theme::serif_bold() });
        });

        // Rullytan.
        let mut clicked: Option<String> = None;
        let mut next_click = false;
        let section = self.section;
        let sec = &book.sections[section];
        let avail = mid.width();
        let col = prefs_now.column().min(avail - 40.0).max(240.0);
        let side = ((avail - col) / 2.0).max(20.0);
        self.view_h = mid.height();
        // Hoppar till ett ställe längre ner ritas först osynligt (innehållets höjd är då okänd); början syns direkt.
        let hide = !self.laid_out && !matches!(self.pending, Some(Jump::Top) | None);
        let mut md_rect_top = None;
        let res = ui
            .scope_builder(UiBuilder::new().max_rect(mid), |ui| {
                ui.set_clip_rect(mid.intersect(ui.clip_rect()));
                if hide {
                    ui.set_opacity(0.0);
                }
                let mut area = ScrollArea::vertical().id_salt(("kladd-epub-scroll", book.uid, section)).auto_shrink([false, false]);
                if let Some(o) = self.offset_next.take() {
                    area = area.vertical_scroll_offset(o);
                }
                area.show(ui, |ui| {
                    apply_style(ui, &c, &prefs_now);
                    ui.horizontal_top(|ui| {
                        ui.add_space(side);
                        ui.vertical(|ui| {
                            ui.set_width(col);
                            ui.add_space(if sec.cover.is_some() { 16.0 } else { 36.0 });
                            if let Some(cover) = &sec.cover {
                                self.cover_page(ui, &book, cover, col, mid.height());
                            } else {
                                let (top_y, link) = self.render_md(ui, p, section, col, &prefs_now, &c);
                                md_rect_top = Some(top_y);
                                clicked = link;
                            }
                            ui.add_space(if sec.cover.is_some() { 8.0 } else { 24.0 });
                            next_click = self.next_footer(ui, &c, &prefs_now, col);
                            ui.add_space(if sec.cover.is_some() { 4.0 } else { 70.0 });
                        });
                    });
                })
            })
            .inner;
        // Mått från den här bildrutan.
        let content_origin = res.inner_rect.top() - res.state.clamped_offset().y;
        self.scroll = res.state.clamped_offset().y;
        self.max_scroll = (res.content_size.y - res.inner_rect.height()).max(0.0);
        if let Some(t) = md_rect_top {
            self.md_top = t - content_origin;
        }
        // Hopp: efter första ritningen av en ny del (då innehållets höjd och blockens lägen är kända).
        if let Some(j) = self.pending {
            if self.laid_out {
                self.pending = None;
                self.offset_next = Some(self.resolve(j));
                ctx.request_repaint();
            }
        }
        if !self.laid_out {
            // Innehållets höjd är känd efter första ritningen (en tom del får ett par bilder på sig).
            self.layout_tries += 1;
            if res.content_size.y > 0.0 || self.layout_tries >= 3 {
                self.laid_out = true;
            }
            ctx.request_repaint();
        }

        // En avslutad textmarkering eller ett klick på en understrykning: menyn för understrykning.
        // egui rapporterar också ett vanligt klick som en markering; bara drag och dubbel- eller trippelklick räknas.
        let (pressed, at, now, multi) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.interact_pos(),
                i.time,
                i.pointer.button_double_clicked(egui::PointerButton::Primary) || i.pointer.button_triple_clicked(egui::PointerButton::Primary),
            )
        });
        if pressed {
            self.press_at = at;
        }
        if multi {
            self.multi_click_at = Some(now);
        }
        if let Some((text, pos, trusted)) = super::selection::take() {
            if let Some(pos) = pos {
                let moved = self.press_at.is_some_and(|p| p.distance(pos) > 4.0);
                let multi = self.multi_click_at.is_some_and(|t| now - t < 2.0);
                if trusted || moved || multi {
                    self.on_selection(text, pos, mid);
                }
            }
        }
        if let Some((mi, pos)) = take_mark_hit(&ctx) {
            self.on_mark_click(mi, pos);
        }

        // Klick i marginalerna: en skärm bakåt eller framåt.
        let col_rect = Rect::from_min_max(pos2(mid.left() + side, mid.top()), pos2(mid.left() + side + col, mid.bottom()));
        if ctx.input(|i| i.pointer.primary_clicked()) && !Popup::is_any_open(&ctx) && self.note.is_none() && !self.about && self.menu.is_none() {
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()).filter(|p| mid.contains(*p)) {
                let line = size * prefs_now.line_factor();
                if pos.x < col_rect.left() - 4.0 {
                    self.page_up(line);
                } else if pos.x > col_rect.right() + 4.0 && pos.x < mid.right() - 12.0 {
                    self.page_down(line);
                }
            }
        }

        // Länkar och "nästa kapitel".
        if let Some(d) = clicked {
            self.click_link(&d);
        }
        if next_click {
            if let Some(n) = self.next_section(section) {
                self.follow_flow(n);
            }
        }

        // Överst och nederst.
        if let Some(a) = self.top_row(ui, p, &c, &mut *prefs, top) {
            action = Some(a);
        }
        self.bottom_row(ui, p, &c, bottom);
        action
    }

    /// Fortsätter läsningen i nästa del (ingen historik: det är ordinarie läsning).
    fn follow_flow(&mut self, s: usize) {
        self.go_section(s, Jump::Top);
    }

    fn click_link(&mut self, dest: &str) {
        let book = self.book.clone();
        let Some((d, noteref)) = book.link(dest) else { return };
        if noteref {
            if let Some(md) = book.link_note(dest) {
                self.note = Some(NotePopup { md, dest: d, fresh: true });
                return;
            }
        }
        self.follow(d);
    }

    fn cover_page(&mut self, ui: &mut Ui, book: &Book, path: &str, col: f32, view_h: f32) {
        let uri = image_uri(book.uid, path);
        if self.registered.insert(uri.clone()) {
            if let Some(b) = book.image(path) {
                ui.ctx().include_bytes(uri.clone(), b);
            }
        }
        ui.vertical_centered(|ui| {
            ui.add(egui::Image::new(uri).max_size(vec2(col.max(240.0) * 1.3, (view_h - 120.0).max(120.0))));
        });
    }

    /// Ritar delens Markdown. Ger var texten börjar (skärmens y) och en eventuell klickad länk.
    fn render_md(&mut self, ui: &mut Ui, p: &Palette, si: usize, col: f32, prefs: &Prefs, c: &Colors) -> (f32, Option<String>) {
        let book = self.book.clone();
        let sec = &book.sections[si];
        // Justering, förlagets blockstil och understrykningar (se `egui_commonmark_backend::misc::ViewExtras`).
        let (marks, ids) = self.marks_for_section(si, c.dark);
        self.mark_ids = ids;
        let blocks = if prefs.publisher {
            sec.blocks
                .iter()
                .map(|(b, l)| {
                    use crate::epub::css::Align as A;
                    let align = l.align.map(|a| match a {
                        A::Left => BlockAlign::Left,
                        A::Center => BlockAlign::Center,
                        A::Right => BlockAlign::Right,
                        A::Justify => BlockAlign::Justify,
                    });
                    (*b, BlockStyle { align, indent: l.indent, margin_left: l.margin_left, small_caps: l.small_caps })
                })
                .collect()
        } else {
            Default::default()
        };
        set_extras(ui.ctx(), Some(Arc::new(ViewExtras { justify: prefs.justify, blocks, marks, note_color: c.link })));
        for path in &sec.images {
            let uri = image_uri(book.uid, path);
            if self.registered.insert(uri.clone()) {
                if let Some(b) = book.image(path) {
                    ui.ctx().include_bytes(uri, b);
                }
            }
        }
        let id = self.view_id(si);
        if self.hooks_for != Some(si) {
            self.cache.link_hooks_clear();
            for d in &sec.link_dests {
                self.cache.add_link_hook(d.clone());
            }
            self.hooks_for = Some(si);
        }
        if self.win.as_ref().is_none_or(|(s, _)| *s != si) {
            self.win = Some((si, Win::new(&sec.md, false)));
        }
        // Sökningen (markeringar i delen).
        let want = if self.find.open { self.find.query.trim().to_owned() } else { String::new() };
        let key = (si, want.clone(), self.find.case);
        if self.applied.as_ref() != Some(&key) {
            *self.cache.search_query_mut(&id) = want.clone();
            let mut f = SearchOptions::empty();
            f.set(SearchOptions::CASE_SENSITIVE, self.find.case);
            *self.cache.search_options_mut(&id) = f;
            if let Some((_, w)) = self.win.as_mut() {
                w.sync_search_layout(&mut self.cache, id);
            }
            self.cache.update_search_matches(&id, &sec.md);
            self.applied = Some(key);
            if let Some(n) = self.want_nth.take() {
                let count = self.cache.search_ranges(&id).len();
                if count > 0 {
                    let cur = self.cache.active_match(&id).unwrap_or(0);
                    self.cache.go_to_match(&id, n.min(count - 1) as isize - cur as isize);
                }
            }
        }
        let (match_bg, match_cur, max_w) = (p.match_bg, p.match_current_bg, col as usize);
        let make = move || CommonMarkViewer::new().max_image_width(Some(max_w)).search_match_color(match_bg).search_active_match_color(match_cur);
        // Allt som påverkar blockens höjd; ändras något mäts blocken om.
        let mut kh = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(prefs.size.to_bits(), prefs.line_factor().to_bits(), prefs.sans, prefs.justify, prefs.publisher, c.dark), &mut kh);
        let cfg = Cfg { id, width: ui.available_width(), key: std::hash::Hasher::finish(&kh), mutable: false, make: &make };
        let win = &mut self.win.as_mut().expect("win").1;
        let wo = win.show(ui, &mut self.cache, &sec.md, &cfg);
        set_extras(ui.ctx(), None);
        let top = wo.rect.top();
        let link = sec.link_dests.iter().find(|d| self.cache.get_link_hook(d) == Some(true)).cloned();
        (top, link)
    }

    /// "Nästa kapitel: …"-raden i slutet av en del. Ger sant när den klickats.
    fn next_footer(&mut self, ui: &mut Ui, c: &Colors, prefs: &Prefs, col: f32) -> bool {
        let sec = &self.book.sections[self.section];
        let next = self.next_section(self.section);
        let (rect, resp) = ui.allocate_exact_size(vec2(col, 46.0), if next.is_some() { Sense::click() } else { Sense::hover() });
        ui.painter().hline(rect.x_range().shrink(col * 0.3), rect.top(), Stroke::new(1.0, c.line));
        let label = match next {
            Some(n) => {
                let nc = self.book.sections[n].chapter;
                if nc == sec.chapter {
                    t("Fortsätt ›").to_owned()
                } else {
                    tf!("Nästa kapitel: {title} ›", title = self.book.chapters[nc].title)
                }
            }
            None => t("Slut på boken").to_owned(),
        };
        let color = if next.is_none() {
            c.muted
        } else if resp.hovered() {
            c.link
        } else {
            c.muted
        };
        let font = FontId::new((prefs.size * 0.85).max(12.0), if prefs.sans { FontFamily::Proportional } else { theme::serif() });
        let mut job = egui::text::LayoutJob::simple_singleline(label, font, color);
        job.wrap.max_width = rect.width();
        job.wrap.max_rows = 1;
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(rect.center().x - g.size().x / 2.0, rect.center().y - g.size().y / 2.0 + 4.0), g, color);
        if next.is_some() && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        next.is_some() && resp.clicked()
    }

    // ---------------------------------------------------------------- överrad

    fn top_row(&mut self, ui: &mut Ui, p: &Palette, c: &Colors, prefs: &mut Prefs, rect: Rect) -> Option<Action> {
        let mut action = None;
        let font = FontId::new(12.5, FontFamily::Proportional);
        let (_, _, chapter) = self.chapter_label();
        let title = self.book.title();
        let label = if title.is_empty() { chapter } else if chapter.is_empty() || chapter == title { title } else { format!("{title}  ·  {chapter}") };
        ui.scope_builder(UiBuilder::new().max_rect(rect.shrink2(vec2(18.0, 0.0))).layout(Layout::right_to_left(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let edit = ui.add(
                egui::Button::new(RichText::new(t("Redigera en kopia")).font(font.clone()).color(c.muted))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, c.line))
                    .corner_radius(7),
            );
            if edit.on_hover_text(t("Öppna bokens text som ett nytt dokument som går att redigera (boken ändras inte)")).clicked() {
                action = Some(Action::EditCopy);
            }
            let aa = ui.add(
                egui::Button::new(RichText::new("Aa").font(FontId::new(13.5, theme::medium())).color(c.muted))
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.0, c.line))
                    .corner_radius(7),
            );
            let aa = aa.on_hover_text(t("Typografi: textstorlek, typsnitt, radavstånd, spaltbredd och sidfärg"));
            Popup::menu(&aa)
                .align(RectAlign::BOTTOM_END)
                .gap(6.0)
                .width(300.0)
                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| prefs_menu(ui, p, prefs));
            // Bokmärke: fylld när stället har ett, ett klick till tar bort det.
            let here = !self.bookmarks_here().is_empty();
            let (br, bresp) = ui.allocate_exact_size(vec2(28.0, 26.0), Sense::click());
            paint_bookmark(ui, br, here, if bresp.hovered() { c.text } else { c.muted }, c.link);
            let tip = if here { t("Ta bort bokmärket (Ctrl+D)") } else { t("Bokmärke på det här stället (Ctrl+D)") };
            if bresp.on_hover_text(tip).clicked() {
                self.toggle_bookmark();
            }
            // Titeln till vänster: ett klick visar Om boken.
            let w = ui.available_width().max(40.0);
            let (r, resp) = ui.allocate_exact_size(vec2(w, rect.height()), Sense::click());
            let mut job = egui::text::LayoutJob::simple_singleline(label, font, c.muted);
            job.wrap.max_width = r.width();
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let g = ui.painter().layout_job(job);
            let col = if resp.hovered() { c.text } else { c.muted };
            ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, col);
            if resp.on_hover_text(t("Om boken (Ctrl+I)")).clicked() {
                self.about = true;
            }
        });
        action
    }

    // ---------------------------------------------------------------- nederrad

    fn bottom_row(&mut self, ui: &mut Ui, _p: &Palette, c: &Colors, rect: Rect) {
        let inset = 24.0;
        let bar_y = rect.top() + 14.0;
        let x0 = rect.left() + inset;
        let x1 = rect.right() - inset;
        let width = (x1 - x0).max(10.0);
        let hit = Rect::from_min_max(pos2(x0, bar_y - 8.0), pos2(x1, bar_y + 10.0));
        let resp = ui.interact(hit, Id::unique(("kladd-epub-progress", self.book.uid)), Sense::click());
        let hovered = resp.hovered();
        let h = if hovered { 5.0 } else { 3.0 };
        let track = Rect::from_min_max(pos2(x0, bar_y - h / 2.0), pos2(x1, bar_y + h / 2.0));
        let painter = ui.painter();
        painter.rect_filled(track, h / 2.0, mix(c.bg, c.text, 0.14));
        let frac = self.progress();
        painter.rect_filled(Rect::from_min_max(track.min, pos2(x0 + width * frac, track.max.y)), h / 2.0, c.link);
        // Streck vid kapitelgränserna.
        let total = self.book.total_words.max(1) as f32;
        let lin = self.book.linear_chapters();
        let ticks: Vec<(f32, usize)> = lin
            .iter()
            .skip(1)
            .map(|&ch| {
                let s = self.book.chapters[ch].first_section;
                (x0 + width * (self.book.sections[s].words_before as f32 / total), ch)
            })
            .collect();
        for (x, _) in &ticks {
            painter.vline(*x, (bar_y - 5.0)..=(bar_y + 5.0), Stroke::new(1.0, mix(c.bg, c.text, 0.35)));
        }
        if let Some(pos) = resp.hover_pos() {
            let near = ticks.iter().find(|(x, _)| (x - pos.x).abs() <= 4.0);
            let tip = match near {
                Some((_, ch)) => self.book.chapters[*ch].title.clone(),
                None => tf!("{pct} % av boken", pct = (((pos.x - x0) / width).clamp(0.0, 1.0) * 100.0).round() as i32),
            };
            resp.clone().on_hover_text(tip);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                if let Some((_, ch)) = ticks.iter().find(|(x, _)| (x - pos.x).abs() <= 4.0) {
                    let here = self.here();
                    self.back.push(here);
                    self.fwd.clear();
                    self.go_section(self.book.chapters[*ch].first_section, Jump::Top);
                } else {
                    let (s, f) = self.at_fraction((pos.x - x0) / width);
                    let here = self.here();
                    self.back.push(here);
                    self.fwd.clear();
                    self.go_section(s, Jump::Frac(f));
                }
            }
        }
        // Texterna under linjen.
        let font = FontId::new(12.0, FontFamily::Proportional);
        let (n, count, _) = self.chapter_label();
        let pct = (self.progress() * 100.0).floor() as i32;
        let left = if n > 0 {
            tf!("Kapitel {n} av {count} · {pct} %", n = n, count = count, pct = pct)
        } else {
            tf!("{pct} %", pct = pct)
        };
        let mins = self.minutes_left();
        let right = if mins == 0 {
            t("mindre än en minut kvar i kapitlet").to_owned()
        } else {
            tf!("ca {min} min kvar i kapitlet", min = mins)
        };
        let y = bar_y + 20.0;
        ui.painter().text(pos2(x0, y), Align2::LEFT_CENTER, left, font.clone(), c.muted);
        ui.painter().text(pos2(x1, y), Align2::RIGHT_CENTER, right, font, c.muted);
    }

    // ---------------------------------------------------------------- överlägg

    /// Sökfältet, fotnotsrutan och Om boken.
    pub fn overlays(&mut self, ctx: &egui::Context, p: &Palette, prefs: &Prefs, anchor: Rect) {
        let c = colors(prefs, p);
        self.find_bar(ctx, p, anchor);
        self.note_popup(ctx, &c, prefs, anchor);
        self.ann_menu(ctx, p, anchor);
        self.about_dialog(ctx, p);
    }

    fn find_bar(&mut self, ctx: &egui::Context, p: &Palette, anchor: Rect) {
        if !self.find.open {
            return;
        }
        let width = 520.0f32.min(anchor.width() - 24.0);
        let pos = pos2(anchor.right() - width - 22.0, anchor.top() + TOP_H + 8.0);
        let mut goto: Option<usize> = None;
        let mut step: Option<bool> = None;
        egui::Area::new(Id::unique("kladd-epub-find")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            Frame::NONE
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(11)
                .inner_margin(Margin::same(8))
                .shadow(ui.visuals().popup_shadow)
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    ui.spacing_mut().item_spacing = vec2(4.0, 6.0);
                    ui.horizontal(|ui| {
                        let qid = Id::unique("kladd-epub-find-query");
                        let field_w = (ui.available_width() - 26.0 * 4.0 - 118.0 - 4.0 * 5.0).max(60.0);
                        let q = ui.add(
                            TextEdit::singleline(&mut self.find.query)
                                .id(qid)
                                .hint_text(t("Sök i boken"))
                                .desired_width(field_w)
                                .margin(Margin::symmetric(8, 5)),
                        );
                        if self.find.focus {
                            q.request_focus();
                            let mut st = TextEditState::load(ctx, qid).unwrap_or_default();
                            st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(self.find.query.chars().count()))));
                            st.store(ctx, qid);
                            self.find.focus = false;
                        }
                        if q.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            step = Some(ui.input(|i| i.modifiers.shift));
                            q.request_focus();
                        }
                        let n = self.find.hits.len();
                        let text = if self.find.query.trim().is_empty() {
                            String::new()
                        } else if n == 0 {
                            t("Inga träffar").to_owned()
                        } else {
                            let more = if self.find.capped { "+" } else { "" };
                            tf!("{cur} av {n}{more}", cur = self.find.cur.map_or(0, |c| c + 1), n = n, more = more)
                        };
                        let (cr, _) = ui.allocate_exact_size(vec2(118.0, 26.0), Sense::hover());
                        let bad = !self.find.query.trim().is_empty() && n == 0;
                        ui.painter().text(cr.center(), Align2::CENTER_CENTER, text, FontId::new(12.0, FontFamily::Proportional), if bad { p.error } else { p.muted });
                        if icons::button(ui, p, Icon::CaseSensitive, 26.0, self.find.case, t("Skilj på stora och små bokstäver"), p.surface).clicked() {
                            self.find.case = !self.find.case;
                        }
                        if icons::button(ui, p, Icon::ChevronUp, 26.0, false, t("Föregående (Shift+Enter)"), p.surface).clicked() {
                            step = Some(true);
                        }
                        if icons::button(ui, p, Icon::ChevronDown, 26.0, false, t("Nästa (Enter)"), p.surface).clicked() {
                            step = Some(false);
                        }
                        if icons::button(ui, p, Icon::Close, 26.0, false, t("Stäng (Esc)"), p.surface).clicked() {
                            self.find.open = false;
                        }
                    });
                    // Träfflistan: kapitel och sammanhang, ett klick hoppar dit.
                    if !self.find.hits.is_empty() {
                        ScrollArea::vertical()
                            .id_salt("kladd-epub-hits")
                            .min_scrolled_height(self.find.hits.len().min(6) as f32 * 38.0)
                            .max_height(240.0)
                            .auto_shrink([false, true]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let book = self.book.clone();
                            let cur = self.find.cur;
                            for (i, h) in self.find.hits.iter().enumerate().take(400) {
                                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                                if !ui.is_rect_visible(rect) {
                                    continue;
                                }
                                if cur == Some(i) {
                                    ui.painter().rect_filled(rect, 6.0, p.current_line);
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, 6.0, p.surface_hover);
                                }
                                let chapter = &book.chapters[book.sections[h.section].chapter].title;
                                ui.painter().text(pos2(rect.left() + 8.0, rect.top() + 11.0), Align2::LEFT_CENTER, truncate(chapter, 60), FontId::new(11.5, theme::medium()), p.muted);
                                ui.painter().text(pos2(rect.left() + 8.0, rect.top() + 27.0), Align2::LEFT_CENTER, truncate(&h.snippet, 80), FontId::new(12.5, FontFamily::Proportional), p.text);
                                if resp.clicked() {
                                    goto = Some(i);
                                }
                            }
                        });
                    }
                });
        });
        if let Some(back) = step {
            self.step_hit(back);
        }
        if let Some(i) = goto {
            self.goto_hit(i);
        }
    }

    fn note_popup(&mut self, ctx: &egui::Context, c: &Colors, prefs: &Prefs, anchor: Rect) {
        let Some(note) = &self.note else { return };
        let width = 560.0f32.min(anchor.width() - 32.0);
        let pos = pos2(anchor.center().x - width / 2.0, anchor.bottom() - BOTTOM_H - 12.0);
        let (md, dest, fresh) = (note.md.clone(), note.dest, note.fresh);
        if let Some(n) = &mut self.note {
            n.fresh = false;
        }
        let mut close = false;
        let mut go = false;
        let id = Id::unique(("kladd-epub-note-view", self.book.uid));
        // Rutan har läsytans färger (som i andra läsare), inte appens.
        let fill = mix(c.bg, c.text, if c.dark { 0.10 } else { 0.05 });
        let area = egui::Area::new(Id::unique("kladd-epub-note")).order(Order::Foreground).pivot(Align2::LEFT_BOTTOM).fixed_pos(pos).show(ctx, |ui| {
            Frame::NONE
                .fill(fill)
                .stroke(Stroke::new(1.0, c.line))
                .corner_radius(12)
                .inner_margin(Margin::same(14))
                .shadow(ui.visuals().popup_shadow)
                .show(ui, |ui| {
                    ui.set_width(width - 28.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(t("Not")).font(FontId::new(12.0, theme::medium())).color(c.muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let btn = |text: &str| egui::Button::new(RichText::new(text.to_owned()).color(c.text)).fill(mix(fill, c.text, 0.10)).stroke(Stroke::NONE);
                            if ui.add(btn("×").min_size(vec2(26.0, 24.0))).on_hover_text(t("Stäng (Esc)")).clicked() {
                                close = true;
                            }
                            if ui.add(btn(t("Gå till noten"))).clicked() {
                                go = true;
                            }
                        });
                    });
                    ScrollArea::vertical().id_salt("kladd-epub-note-scroll").max_height(200.0).auto_shrink([false, true]).show(ui, |ui| {
                        apply_style(ui, c, &Prefs { size: (prefs.size * 0.9).max(12.0), ..prefs.clone() });
                        CommonMarkViewer::new().max_image_width(Some((width - 40.0) as usize)).show_with_id(id, ui, &mut self.cache, &md);
                    });
                });
        });
        // Ett klick utanför rutan stänger den.
        if !fresh && ctx.input(|i| i.pointer.primary_clicked()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| !area.response.rect.contains(pos)) {
            close = true;
        }
        if go {
            self.follow(dest);
        } else if close {
            self.note = None;
        }
    }

    fn about_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.about {
            return;
        }
        let book = self.book.clone();
        let cover_uri = format!("bytes://kladd-epub-about/{}", book.uid);
        if book.cover.is_some() && self.registered.insert(cover_uri.clone()) {
            if let Some(b) = book.cover_image() {
                ctx.include_bytes(cover_uri.clone(), b);
            }
        }
        let m = Modal::new(Id::unique("kladd-epub-about"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
            .frame(Frame::NONE.fill(p.surface).stroke(Stroke::new(1.0, p.line)).corner_radius(14).inner_margin(Margin::same(24)).shadow(ctx.global_style().visuals.window_shadow))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.horizontal_top(|ui| {
                    if book.cover.is_some() {
                        ui.add(egui::Image::new(cover_uri.clone()).max_size(vec2(150.0, 220.0)).corner_radius(4));
                        ui.add_space(14.0);
                    }
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        let title = book.title();
                        ui.label(RichText::new(if title.is_empty() { t("Utan titel").to_owned() } else { title }).font(FontId::new(19.0, theme::semibold())).color(p.text));
                        if !book.meta.authors.is_empty() {
                            ui.label(RichText::new(book.meta.authors.join(", ")).font(FontId::new(14.0, FontFamily::Proportional)).color(p.muted));
                        }
                        ui.add_space(10.0);
                        let row = |ui: &mut Ui, k: &str, v: &str| {
                            if !v.trim().is_empty() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(format!("{k}:")).color(p.muted));
                                    ui.label(RichText::new(v).color(p.text));
                                });
                            }
                        };
                        row(ui, t("Förlag"), &book.meta.publisher);
                        row(ui, t("Språk"), &book.meta.language);
                        row(ui, t("Utgiven"), &date_only(&book.meta.date));
                        let (_, chapters, _) = self.chapter_label();
                        let hours = book.total_words as f32 / WORDS_PER_MINUTE / 60.0;
                        let size = if hours >= 1.0 { tf!("ca {h} timmar", h = crate::i18n::num(hours as f64, 1)) } else { tf!("ca {m} minuter", m = (hours * 60.0).ceil() as usize) };
                        row(ui, t("Omfång"), &tf!("{chapters} kapitel · {words} ord · {size} läsning", chapters = chapters, words = book.total_words, size = size));
                    });
                });
                if !book.meta.description.trim().is_empty() {
                    ui.add_space(12.0);
                    ScrollArea::vertical().id_salt("kladd-epub-about-desc").max_height(180.0).show(ui, |ui| {
                        ui.label(RichText::new(&book.meta.description).color(p.text));
                    });
                }
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(quiet_button(t("Stäng"), p).min_size(vec2(80.0, 30.0))).clicked() {
                        self.about = false;
                    }
                });
            });
        if m.should_close() {
            self.about = false;
        }
    }
}

// ------------------------------------------------------------------ hjälpare för ritning

/// Textsnuttens omgivning kring en träff (texten är ren, en rad).
fn snippet_of(hay: &str, orig: &str, at: usize, len: usize) -> String {
    let text = if hay.len() == orig.len() { orig } else { hay };
    let mut start = at.saturating_sub(30);
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (at + len + 50).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    let pre = if start > 0 { "…" } else { "" };
    let post = if end < text.len() { "…" } else { "" };
    format!("{pre}{}{post}", &text[start..end])
}

/// `2014-05-25T00:00:00Z` blir `2014-05-25`; andra datumformat lämnas.
fn date_only(d: &str) -> String {
    let d = d.trim();
    match d.split_once('T') {
        Some((day, _)) if day.len() >= 4 && day.chars().all(|c| c.is_ascii_digit() || c == '-') => day.to_owned(),
        _ => d.to_owned(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_owned() } else { format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

/// Typsnitt, storlekar och färger i läsytan.
fn apply_style(ui: &mut Ui, c: &Colors, prefs: &Prefs) {
    let size = prefs.size;
    let body = if prefs.sans { FontFamily::Proportional } else { theme::serif() };
    let st = ui.style_mut();
    st.text_styles.insert(egui::TextStyle::Body, FontId::new(size, body.clone()));
    st.text_styles.insert(egui::TextStyle::Monospace, FontId::new(size * 0.86, FontFamily::Monospace));
    st.text_styles.insert(egui::TextStyle::Heading, FontId::new(size * 2.2, if prefs.sans { theme::semibold() } else { theme::serif_bold() }));
    st.spacing.item_spacing.y = size * 0.75;
    st.visuals.dark_mode = c.dark;
    st.visuals.widgets.noninteractive.fg_stroke.color = c.text;
    st.visuals.widgets.active.fg_stroke.color = c.text;
    st.visuals.hyperlink_color = c.link;
    st.visuals.faint_bg_color = mix(c.bg, c.text, 0.05);
    st.visuals.code_bg_color = mix(c.bg, c.text, 0.08);
    st.visuals.extreme_bg_color = mix(c.bg, c.text, 0.04);
    st.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, c.line);
    st.visuals.selection.bg_fill = mix(c.bg, c.link, 0.35);
}

/// En rad med alternativ där ett är valt. Ger det valda numret om något klickades.
fn segmented(ui: &mut Ui, p: &Palette, options: &[&str], selected: usize) -> Option<usize> {
    let mut out = None;
    let w = (ui.available_width() - 4.0 * (options.len() as f32 - 1.0)) / options.len() as f32;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, o) in options.iter().enumerate() {
            let on = i == selected;
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
            let fill = if on { p.accent } else if resp.hovered() { p.surface_hover } else { p.current_line };
            ui.painter().rect_filled(rect, 7.0, fill);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, *o, FontId::new(12.5, theme::medium()), if on { p.on_accent } else { p.text });
            if resp.clicked() {
                out = Some(i);
            }
        }
    });
    out
}

/// Ett bokmärkesband (fyllt med färg när `on`).
fn paint_bookmark(ui: &Ui, r: Rect, on: bool, color: Color32, fill: Color32) {
    let c = r.center();
    let (x0, x1, y0, y1) = (c.x - 5.5, c.x + 5.5, c.y - 8.0, c.y + 8.0);
    let ym = y1 - 5.0;
    let xm = c.x;
    let painter = ui.painter();
    if on {
        painter.rect_filled(Rect::from_min_max(pos2(x0, y0), pos2(x1, ym)), 0.0, fill);
        painter.add(egui::Shape::convex_polygon(vec![pos2(x0, ym), pos2(xm, ym), pos2(x0, y1)], fill, Stroke::NONE));
        painter.add(egui::Shape::convex_polygon(vec![pos2(xm, ym), pos2(x1, ym), pos2(x1, y1)], fill, Stroke::NONE));
    } else {
        let st = Stroke::new(1.5, color);
        painter.add(egui::Shape::line(vec![pos2(x0, y1), pos2(x0, y0), pos2(x1, y0), pos2(x1, y1), pos2(xm, ym), pos2(x0, y1)], st));
    }
}

fn prefs_menu(ui: &mut Ui, p: &Palette, prefs: &mut Prefs) {
    ui.set_width(300.0);
    ui.spacing_mut().item_spacing.y = 6.0;
    let head = |ui: &mut Ui, s: &str| {
        ui.add_space(4.0);
        ui.label(RichText::new(s).font(FontId::new(11.5, theme::medium())).color(p.muted));
    };
    head(ui, t("Textstorlek"));
    ui.horizontal(|ui| {
        if ui.add(quiet_button("A−", p).min_size(vec2(44.0, 28.0))).clicked() {
            prefs.step_size(-1.0);
        }
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width() - 48.0, 28.0), Sense::hover());
        ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{}", prefs.size.round() as i32), FontId::new(13.5, theme::medium()), p.text);
        if ui.add(quiet_button("A+", p).min_size(vec2(44.0, 28.0))).clicked() {
            prefs.step_size(1.0);
        }
    });
    head(ui, t("Typsnitt"));
    if let Some(i) = segmented(ui, p, &[t("Serif"), t("Sans")], usize::from(prefs.sans)) {
        prefs.sans = i == 1;
    }
    head(ui, t("Radavstånd"));
    if let Some(i) = segmented(ui, p, &[t("Kompakt"), t("Normal"), t("Luftig")], usize::from(prefs.spacing)) {
        prefs.spacing = i as u8;
    }
    head(ui, t("Spaltbredd"));
    if let Some(i) = segmented(ui, p, &[t("Smal"), t("Normal"), t("Bred")], usize::from(prefs.width)) {
        prefs.width = i as u8;
    }
    head(ui, t("Justerad text"));
    if let Some(i) = segmented(ui, p, &[t("Av"), t("På")], usize::from(prefs.justify)) {
        prefs.justify = i == 1;
    }
    head(ui, t("Förlagets formatering"));
    if let Some(i) = segmented(ui, p, &[t("Av"), t("På")], usize::from(prefs.publisher)) {
        prefs.publisher = i == 1;
    }
    head(ui, t("Sidfärg"));
    if let Some(i) = segmented(ui, p, &[t("Auto"), t("Ljus"), t("Sepia"), t("Mörk")], usize::from(prefs.page)) {
        prefs.page = i as u8;
    }
    ui.add_space(4.0);
}

// ------------------------------------------------------------------ sidopanel och statusrad

/// Innehållsförteckningen i vänstra panelen. Ger det ställe som valts.
pub fn sidebar(ui: &mut Ui, p: &Palette, tab: &mut EpubTab) -> Option<Dest> {
    let at = Dest { section: tab.section, byte: tab.top_byte() };
    let cur = tab.book.toc_current(at);
    if cur != tab.last_toc_cur {
        tab.toc_scroll = true;
        tab.last_toc_cur = cur;
    }
    tab.toc_cur = cur;
    let book = tab.book.clone();
    let scroll_cur = std::mem::take(&mut tab.toc_scroll);
    let mut goto = None;
    let width = ui.available_width();
    Panel::left("kladd-epub-toc")
        .resizable(true)
        .default_size(260.0)
        .size_range(170.0..=(width * 0.5).max(220.0))
        .frame(Frame::NONE.fill(p.editor).inner_margin(Margin { left: 10, right: 6, top: 12, bottom: 0 }))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            // Flikar: Innehåll | Bokmärken | Anteckningar.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for which in [SideTab::Toc, SideTab::Bookmarks, SideTab::Notes] {
                    let label = annot::tab_label(which, &tab.notes);
                    let on = tab.side == which;
                    let galley = ui.painter().layout_no_wrap(label, FontId::new(12.0, theme::medium()), if on { p.accent } else { p.muted });
                    let (r, resp) = ui.allocate_exact_size(vec2(galley.size().x + 14.0, 26.0), Sense::click());
                    if on {
                        ui.painter().rect_filled(Rect::from_min_size(pos2(r.left() + 7.0, r.bottom() - 2.0), vec2(galley.size().x, 2.0)), 1.0, p.accent);
                    }
                    let color = if on { p.accent } else if resp.hovered() { p.text } else { p.muted };
                    ui.painter().galley(pos2(r.left() + 7.0, r.center().y - galley.size().y / 2.0 - 1.0), galley, color);
                    if resp.clicked() {
                        tab.side = which;
                        if which == SideTab::Toc {
                            tab.toc_scroll = true;
                        }
                    }
                }
            });
            ui.add_space(8.0);
            if tab.side != SideTab::Toc {
                let which = tab.side;
                let salt = if which == SideTab::Bookmarks { "kladd-epub-bm-scroll" } else { "kladd-epub-hl-scroll" };
                let mut picked = None;
                annot::list_scroll(salt).show(ui, |ui| {
                    picked = tab.note_list(ui, p, which == SideTab::Bookmarks);
                    if which == SideTab::Notes && !tab.notes.is_empty() {
                        ui.add_space(10.0);
                        if ui.add(quiet_button(t("Exportera anteckningar…"), p).min_size(vec2(0.0, 28.0))).clicked() {
                            tab.export_requested = true;
                        }
                        ui.add_space(16.0);
                    }
                });
                if let Some(id) = picked {
                    tab.goto_note(id);
                }
                return;
            }
            ScrollArea::vertical().id_salt(("kladd-epub-toc-scroll", book.uid)).auto_shrink([false, false]).show(ui, |ui| {
                if book.toc.is_empty() {
                    ui.label(RichText::new(t("Ingen innehållsförteckning")).size(12.5).color(p.muted));
                    return;
                }
                let min_depth = book.toc.iter().map(|e| e.depth).min().unwrap_or(0);
                for (i, e) in book.toc.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), if e.dest.is_some() { Sense::click() } else { Sense::hover() });
                    let is_cur = cur == Some(i);
                    if is_cur && scroll_cur {
                        ui.scroll_to_rect(rect, None);
                    }
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let painter = ui.painter();
                    if is_cur {
                        painter.rect_filled(rect, 6.0, p.current_line);
                        painter.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 5.0), vec2(2.5, 18.0)), 1.0, p.accent);
                    } else if resp.hovered() && e.dest.is_some() {
                        painter.rect_filled(rect, 6.0, p.surface_hover);
                    }
                    let x = rect.left() + 10.0 + (e.depth.saturating_sub(min_depth)) as f32 * 14.0;
                    let color = if is_cur {
                        p.accent
                    } else if e.dest.is_none() {
                        p.faint
                    } else if e.depth <= min_depth {
                        p.text
                    } else {
                        p.muted
                    };
                    let font = if e.depth <= min_depth { FontId::new(13.5, theme::semibold()) } else { FontId::new(13.0, FontFamily::Proportional) };
                    let mut job = egui::text::LayoutJob::simple_singleline(e.title.clone(), font, color);
                    job.wrap.max_width = (rect.right() - x - 6.0).max(10.0);
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    let g = painter.layout_job(job);
                    painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
                    if resp.clicked() {
                        goto = e.dest;
                    }
                    resp.on_hover_text(e.title.clone());
                }
                ui.add_space(16.0);
            });
        });
    goto
}

impl EpubTab {
    /// Byteposition överst i delen som visas (för att markera aktuell post i förteckningen).
    fn top_byte(&self) -> usize {
        let pts = self.cache.line_points(&self.view_id(self.section));
        let y = self.scroll - self.md_top + 4.0;
        let mut best: Option<(usize, f32)> = None;
        for &(b, py) in pts {
            if py <= y + 0.5 && best.is_none_or(|(_, by)| py >= by) {
                best = Some((b, py));
            }
        }
        best.map_or(0, |b| b.0)
    }
}

/// Statusraden: läge, kapitel, procent och textstorlek.
pub fn status(ui: &mut Ui, p: &Palette, font: &FontId, tab: &EpubTab, prefs: &Prefs) {
    let label = |ui: &mut Ui, s: String| {
        ui.label(RichText::new(s).font(font.clone()).color(p.muted));
    };
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        label(ui, tf!("Textstorlek {size}", size = prefs.size.round() as i32));
        label(ui, format!("{} %", (tab.progress() * 100.0).floor() as i32));
        let (_, _, ch) = tab.chapter_label();
        if !ch.is_empty() {
            label(ui, truncate(&ch, 60));
        }
        label(ui, t("Bara läsning").to_owned());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epub::tests::epub3;

    fn tab() -> EpubTab {
        EpubTab::new(Book::load(3, epub3()).unwrap())
    }

    #[test]
    fn läget_per_bok_minns_högst_hundra_och_flyttar_fram_senast_använda() {
        let mut list: Vec<(String, Memory)> = Vec::new();
        for i in 0..105 {
            remember(&mut list, format!("/bok{i}.epub"), Memory { section: i, frac: 0.5, ..Default::default() }, MAX_MEMORY);
        }
        assert_eq!(list.len(), MAX_MEMORY);
        assert_eq!(list[0].0, "/bok104.epub");
        remember(&mut list, "/bok50.epub".into(), Memory { section: 7, frac: 0.1, ..Default::default() }, MAX_MEMORY);
        assert_eq!(list[0], ("/bok50.epub".into(), Memory { section: 7, frac: 0.1, ..Default::default() }));
        assert_eq!(list.iter().filter(|(k, _)| k == "/bok50.epub").count(), 1);
        assert_eq!(list.len(), MAX_MEMORY);
    }

    #[test]
    fn inställningar_från_en_gammal_fil_får_standardvärden() {
        let p: Prefs = serde_json::from_str("{\"size\":21.0,\"page\":2}").unwrap();
        assert_eq!((p.size, p.page, p.sans, p.spacing, p.width), (21.0, 2, false, 1, 1));
        assert!(!p.justify && p.publisher, "justerad text av och förlagets formatering på som standard");
        let p: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, Prefs::default());
        // Hela appens inställningar utan EPUB-fälten.
        let s: crate::app::Settings = serde_json::from_str("{\"dark\":true}").unwrap();
        assert_eq!(s.epub, Prefs::default());
        assert!(s.epub_memory.is_empty());
        // Orimliga värden hålls inom gränserna vid ritning.
        let mut p = Prefs { size: 400.0, ..Prefs::default() };
        assert_eq!(p.sanitized().size, SIZE_MAX);
        p.size = SIZE_MIN;
        p.step_size(-5.0);
        assert_eq!(p.size, SIZE_MIN);
        p.step_size(100.0);
        assert_eq!(p.size, SIZE_MAX);
        assert!(Prefs { spacing: 9, width: 9, ..Prefs::default() }.line_factor() > 1.7, "okänt värde faller tillbaka på största");
    }

    #[test]
    fn sidfärgerna_sepia_ljus_mörk_och_auto() {
        let p = theme::palette(true);
        let auto = colors(&Prefs::default(), &p);
        assert_eq!((auto.bg, auto.text, auto.dark), (p.editor, p.text, true), "Auto följer temat");
        let sepia = colors(&Prefs { page: 2, ..Prefs::default() }, &p);
        assert!(!sepia.dark && sepia.bg.r() > sepia.bg.b() + 15, "varm beige bakgrund");
        assert!(sepia.text.r() > sepia.text.b(), "brun text");
        assert!(colors(&Prefs { page: 3, ..Prefs::default() }, &theme::palette(false)).dark, "Mörk gäller även i ljust tema");
        let light = colors(&Prefs { page: 1, ..Prefs::default() }, &p);
        assert!(!light.dark && light.bg.r() > 240);
        assert_eq!(page_fill(&Prefs { page: 1, ..Prefs::default() }, &p), light.bg);
    }

    #[test]
    fn framsteg_tid_kvar_och_kapitelnummer() {
        let mut t = tab();
        assert_eq!(t.progress(), 0.0);
        let (n, count, title) = t.chapter_label();
        assert_eq!((n, count, title.as_str()), (1, 4, "Omslag"));
        // Mitt i boken: delen och andelen ger ungefär samma framsteg tillbaka.
        let (s, f) = t.at_fraction(0.5);
        t.section = s;
        t.max_scroll = 100.0;
        t.scroll = f * 100.0;
        assert!((t.progress() - 0.5).abs() < 0.02, "{}", t.progress());
        let (s, f) = t.at_fraction(1.0);
        assert_eq!(s, t.book.sections.len() - 1);
        assert!((f - 1.0).abs() < 1e-6);
        // Tid kvar räknas med 230 ord/minut och aldrig för mycket.
        t.section = t.book.chapters[1].first_section;
        t.max_scroll = 0.0;
        let words = t.book.sections[t.section].words as f32;
        assert_eq!(t.minutes_left(), (words / 230.0).ceil() as usize);
        t.max_scroll = 100.0;
        t.scroll = 100.0;
        assert_eq!(t.minutes_left(), 0, "längst ner: inget kvar");
    }

    #[test]
    fn minnet_återställer_del_och_rullning() {
        let mut t = tab();
        t.restore(&Memory { section: 2, frac: 0.25, limit: super::super::SECTION_LIMIT, byte: None });
        assert_eq!(t.section, 2);
        assert_eq!(t.pending, Some(Jump::Frac(0.25)));
        // En del som inte finns (boken har ändrats) ignoreras.
        t.restore(&Memory { section: 999, frac: 0.9, limit: super::super::SECTION_LIMIT, byte: None });
        assert_eq!(t.section, 2);
        t.scroll = 30.0;
        t.max_scroll = 120.0;
        let m = t.memory();
        assert_eq!((m.section, m.limit), (2, super::super::SECTION_LIMIT));
        assert!((m.frac - 0.25).abs() < 1e-6);
    }

    #[test]
    fn tillbaka_och_framåt_minns_ställena() {
        let mut t = tab();
        t.scroll = 12.0;
        t.follow(Dest { section: 3, byte: 0 });
        assert_eq!(t.section, 3);
        t.go_back();
        assert_eq!((t.section, t.pending), (0, Some(Jump::Offset(12.0))));
        t.go_forward();
        assert_eq!(t.section, 3);
        // Ett nytt hopp efter Tillbaka tömmer Framåt.
        t.go_back();
        t.follow(Dest { section: 2, byte: 0 });
        t.go_forward();
        assert_eq!(t.section, 2, "inget att gå framåt till");
    }

    #[test]
    fn datum_visas_utan_klockslag() {
        assert_eq!(date_only("2014-05-25T00:00:00Z"), "2014-05-25");
        assert_eq!(date_only(" 2020 "), "2020");
        assert_eq!(date_only("May 5, 2020"), "May 5, 2020");
        assert_eq!(date_only("Tuesday"), "Tuesday");
    }

    #[test]
    fn träfflistan_hoppar_mellan_delar_och_snuttar_blir_hanterliga() {
        let mut t = tab();
        t.open_find();
        t.find.query = "noten".into();
        t.update_hits();
        assert_eq!(t.find.hits.len(), 1);
        assert_eq!(t.find.cur, Some(0));
        let chapter = t.book.sections[t.section].chapter;
        assert_eq!(t.book.chapters[chapter].title, "Noter");
        // Stor/liten bokstav.
        t.find.query = "NOTEN".into();
        t.update_hits();
        assert_eq!(t.find.hits.len(), 1);
        t.find.case = true;
        t.update_hits();
        assert!(t.find.hits.is_empty() && t.find.cur.is_none());
        // Snutten klipps vid teckengränser (åäö).
        let long = format!("{}åäö träffen här{}", "å".repeat(60), "ö".repeat(80));
        let s = snippet_of(&long.to_lowercase(), &long, long.find("träffen").unwrap(), "träffen".len());
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains("träffen"), "{s}");
    }

    // ------------------------------------------------------------ galleriet (justering, förlagets stil, markeringar)

    /// Ritar `md` med tilläggen och ger alla textgalleri som målades.
    fn galleys(md: &str, extras: ViewExtras) -> Vec<Arc<egui::Galley>> {
        fn walk(shape: &egui::Shape, out: &mut Vec<Arc<egui::Galley>>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.clone()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        let mut cache = CommonMarkCache::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(700.0, 900.0))), ..Default::default() };
        let mut out = Vec::new();
        let extras = Arc::new(extras);
        for _ in 0..2 {
            out.clear();
            let full = ctx.run_ui(input.clone(), |ui| {
                set_extras(ui.ctx(), Some(extras.clone()));
                CommonMarkViewer::new().show_with_id(Id::unique("t"), ui, &mut cache, md);
                set_extras(ui.ctx(), None);
            });
            for cs in &full.shapes {
                walk(&cs.shape, &mut out);
            }
            full.drop_without_applying_deltas();
        }
        out
    }

    const LONG: &str = "Det var en gång en lång berättelse om en räv och en hare som bodde i en stor skog långt borta från alla andra djur, \
        och varje morgon gick de ut för att leta efter mat, medan solen steg över träden och dimman lättade över sjön där svanarna simmade.";

    #[test]
    fn justerad_text_ger_rak_högerkant_utom_på_sista_raden() {
        let md = format!("{LONG}\n");
        let plain = galleys(&md, ViewExtras::default());
        let g = plain.iter().find(|g| g.text().starts_with("Det var")).expect("stycket ritas");
        assert!(!g.job.justify);
        let widths: Vec<f32> = g.rows.iter().map(|r| r.rect().width()).collect();
        assert!(widths.len() >= 3, "stycket bryts över flera rader: {widths:?}");
        let ragged = widths[..widths.len() - 1].windows(2).any(|w| (w[0] - w[1]).abs() > 1.0);
        assert!(ragged, "utan justering är raderna ojämna: {widths:?}");

        let just = galleys(&md, ViewExtras { justify: true, ..ViewExtras::default() });
        let g = just.iter().find(|g| g.text().starts_with("Det var")).expect("stycket ritas justerat");
        let widths: Vec<f32> = g.rows.iter().map(|r| r.rect().width()).collect();
        let body = &widths[..widths.len() - 1];
        assert!(body.windows(2).all(|w| (w[0] - w[1]).abs() < 1.5), "alla rader utom den sista är lika breda: {widths:?}");
        assert!(widths[widths.len() - 1] < body[0] - 5.0, "sista raden är vänsterställd och kortare: {widths:?}");
        assert_eq!(g.text(), plain.iter().find(|g| g.text().starts_with("Det var")).unwrap().text(), "justeringen ändrar inte texten");
    }

    #[test]
    fn justerad_text_behåller_första_radens_indrag() {
        let md = format!("{LONG}\n");
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(0, BlockStyle { indent: 2.0, ..BlockStyle::default() });
        let gs = galleys(&md, ViewExtras { justify: true, blocks, ..ViewExtras::default() });
        let g = gs.iter().find(|g| g.text().starts_with("Det var")).expect("stycket ritas");
        let first_x = g.rows[0].row.glyphs[0].pos.x + g.rows[0].pos.x;
        let second_x = g.rows[1].row.glyphs[0].pos.x + g.rows[1].pos.x;
        assert!(first_x > second_x + 20.0, "första raden är indragen även när texten är justerad: {first_x} mot {second_x}");
        let widths: Vec<f32> = g.rows.iter().map(|r| r.rect().right()).collect();
        let n = widths.len();
        assert!(widths[..n - 1].iter().all(|w| (w - widths[0]).abs() < 1.5), "alla rader utom den sista slutar vid samma kant: {widths:?}");
    }

    #[test]
    fn förlagets_blockstil_centrerar_drar_in_och_ger_små_versaler() {
        let md = "# Kapitel ett\n\nFörsta stycket här.\n\nAndra stycket.\n";
        let mut blocks = std::collections::HashMap::new();
        blocks.insert(0, BlockStyle { align: Some(BlockAlign::Center), small_caps: true, ..BlockStyle::default() });
        blocks.insert(md.find("Första").unwrap(), BlockStyle { indent: 2.0, ..BlockStyle::default() });
        let gs = galleys(md, ViewExtras { blocks, ..ViewExtras::default() });
        let head = gs.iter().find(|g| g.text().to_lowercase().starts_with("kapitel")).expect("rubriken ritas");
        assert_eq!(head.job.halign, egui::Align::Center, "centrerad rubrik");
        assert_eq!(head.text(), "KAPITEL ETT", "små versaler: gemener ritas som versaler");
        let sizes: std::collections::BTreeSet<u32> = head.job.sections.iter().map(|s| (s.format.font_id.size * 10.0) as u32).collect();
        assert!(sizes.len() == 2, "gemenerna är mindre än versalerna: {sizes:?}");
        let p = gs.iter().find(|g| g.text().starts_with("Första")).expect("stycket ritas");
        assert!(p.job.sections[0].leading_space > 20.0, "första raden är indragen: {}", p.job.sections[0].leading_space);
        assert_eq!(p.job.halign, egui::Align::LEFT);
        // Ett stycke utan blockstil ritas som vanligt (ingen galleri-väg: samma text men utan indrag).
        let q = gs.iter().find(|g| g.text().starts_with("Andra")).expect("andra stycket ritas");
        assert!(q.job.sections[0].leading_space < 5.0);
    }

    #[test]
    fn markeringar_ritas_som_bakgrund_och_länkar_med_färg() {
        let md = "Hej [en länk](kl:0:0) och mer text här.\n";
        let color = Color32::from_rgba_unmultiplied(247, 200, 28, 110);
        let start = md.find("mer").unwrap();
        let marks = vec![egui_commonmark::Mark { range: start..start + 3, color, note: false }];
        let gs = galleys(md, ViewExtras { justify: true, marks, ..ViewExtras::default() });
        let g = gs.iter().find(|g| g.text().starts_with("Hej")).expect("stycket ritas");
        let marked: String = g.job.sections.iter().filter(|s| s.format.background == color).map(|s| &g.job.text[s.byte_range.start.0..s.byte_range.end.0]).collect();
        assert_eq!(marked, "mer", "bara den markerade texten får bakgrund");
        let parts: Vec<&str> = g.job.sections.iter().map(|s| &g.job.text[s.byte_range.start.0..s.byte_range.end.0]).collect();
        let link = g.job.sections.iter().find(|s| &g.job.text[s.byte_range.start.0..s.byte_range.end.0] == "en länk").unwrap_or_else(|| panic!("länken: {parts:?}"));
        assert_ne!(link.format.color, g.job.sections[0].format.color, "länkar har länkfärg");
        // Bild i stycket: ritas på vanligt sätt, utan att tappa texten omkring.
        let with_img = galleys("Före ![x](bytes://finns-inte.png) efter\n", ViewExtras { justify: true, ..ViewExtras::default() });
        assert!(with_img.iter().any(|g| g.text().contains("Före")), "stycket med bild finns kvar");
    }
}

