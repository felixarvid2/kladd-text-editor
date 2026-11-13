//! Tabellvyn: ett rutnät över texten i en csv/tsv-flik. Raderna ritas virtuellt (bara de som syns),
//! rubrikraden och radnumren ligger kvar vid rullning. Ändringar går via `model::Patch` på texten.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Align, Align2, CentralPanel, Color32, CursorIcon, Event, EventFilter, FontFamily, FontId, Frame, Id, Key,
    KeyboardShortcut, Layout, Modifiers, Popup, Rect, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui,
    UiBuilder, Vec2, pos2, vec2,
};

use super::collate::SortMode;
use super::model::{self, Format, Patch, Table};
use super::sniff::{self, DELIMS, Sniffed};
use super::stats::{self, Stats};
use crate::app::{KladdApp, menu_item, status_button};
use crate::doc::Document;
use crate::i18n::t;
use crate::theme::{self, Palette};
use crate::tf;

const MIN_COL_W: f32 = 36.0;
const MAX_AUTO_W: f32 = 380.0;
const UNDO_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// En rektangel av celler (radnummer i tabellen, inklusive rubrikraden som rad 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRect {
    pub r0: usize,
    pub c0: usize,
    pub r1: usize,
    pub c1: usize,
}

impl CellRect {
    fn between(a: (usize, usize), b: (usize, usize)) -> Self {
        CellRect { r0: a.0.min(b.0), c0: a.1.min(b.1), r1: a.0.max(b.0), c1: a.1.max(b.1) }
    }
    fn contains(&self, r: usize, c: usize) -> bool {
        r >= self.r0 && r <= self.r1 && c >= self.c0 && c <= self.c1
    }
}

struct CellEdit {
    row: usize,
    col: usize,
    buf: String,
    /// Första bilden: sätt markören sist i texten.
    fresh: bool,
}

/// Vad som finns under pekaren.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Corner,
    /// Kolumnrubrik (visningskolumn).
    Col(usize),
    /// Kanten till höger om kolumnen.
    Border(usize),
    /// Sorteringsknappen i kolumnrubriken.
    Sort(usize),
    /// Radnummer (visningsrad).
    RowNo(usize),
    Cell(usize, usize),
    Outside,
}

#[derive(Clone, Debug)]
enum Act {
    Cut,
    Copy,
    PasteClipboard,
    Clear,
    InsertRow { below: bool },
    DeleteRows,
    InsertCol { right: bool },
    DeleteCols,
    Sort { col: usize, desc: bool },
    Mode(SortMode),
    AutoFit(usize),
    Undo,
    Redo,
}

/// Allt som vyn kommer ihåg om en flik.
pub struct Tab {
    /// Fliken visas som tabell (annars som text).
    pub grid: bool,
    /// Tabellvyn kan erbjudas (filändelse eller innehåll).
    pub available: bool,
    pub header: bool,
    /// Vald avgränsare; `None` = den gissade.
    pub delim: Option<u8>,
    sniffed: Sniffed,
    sniff_len: usize,
    fmt: Format,
    table: Table,
    key: (usize, u64),
    /// Dokumentets ändringsräknare vid senaste synkning.
    rev: u64,
    /// Kolumnbredder (0 = ännu inte beräknad).
    widths: Vec<f32>,
    cur: (usize, usize),
    anchor: (usize, usize),
    extra: Vec<CellRect>,
    filter: String,
    filter_col: Option<usize>,
    view: Option<Vec<usize>>,
    view_key: u64,
    edit: Option<CellEdit>,
    undo: Vec<Patch>,
    redo: Vec<Patch>,
    sort_mode: SortMode,
    /// Senast sorterad kolumn (och riktning) med textens nyckel då.
    sorted: Option<(usize, bool, (usize, u64))>,
    pub stats: Option<Stats>,
    stats_key: u64,
    goto: Option<usize>,
    scroll_to: bool,
    resize: Option<(usize, f32, f32)>,
    page_rows: usize,
    ctx_hit: Option<Hit>,
    toast: Option<String>,
}

impl Tab {
    fn new(doc: &Document) -> Tab {
        let ext = doc.extension();
        let sn = if sniff::may_be_table_ext(&ext) { sniff::sniff(&doc.text, &ext) } else { None };
        let available = sn.is_some();
        let sniffed = sn.unwrap_or(Sniffed { fmt: Format::COMMA, header: false, cols: 1 });
        let grid = available && sniff::is_table_ext(&ext);
        Tab {
            grid,
            available,
            header: sniffed.header,
            delim: None,
            sniffed,
            sniff_len: doc.text.len(),
            fmt: sniffed.fmt,
            table: Table::parse("", sniffed.fmt),
            key: (usize::MAX, 0),
            rev: 0,
            widths: Vec::new(),
            cur: (0, 0),
            anchor: (0, 0),
            extra: Vec::new(),
            filter: String::new(),
            filter_col: None,
            view: None,
            view_key: 0,
            edit: None,
            undo: Vec::new(),
            redo: Vec::new(),
            sort_mode: SortMode::Auto,
            sorted: None,
            stats: None,
            stats_key: 0,
            goto: None,
            scroll_to: false,
            resize: None,
            page_rows: 20,
            ctx_hit: None,
            toast: None,
        }
    }

    /// Uppdaterar om tabellvyn kan erbjudas när texten ändrats i textläget.
    fn refresh_available(&mut self, doc: &Document) {
        let ext = doc.extension();
        if self.grid || !sniff::may_be_table_ext(&ext) || doc.text.len() == self.sniff_len {
            return;
        }
        self.sniff_len = doc.text.len();
        match sniff::sniff(&doc.text, &ext) {
            Some(s) => {
                self.sniffed = s;
                self.available = true;
            }
            None => self.available = false,
        }
    }

    /// Växlar mellan tabell och text.
    pub fn set_grid(&mut self, grid: bool, doc: &Document) {
        if grid && !self.grid && self.delim.is_none() {
            // Texten kan ha ändrats i textläget.
            if let Some(s) = sniff::sniff(&doc.text, &doc.extension()) {
                self.sniffed.fmt = s.fmt;
            }
        }
        self.grid = grid;
        self.edit = None;
    }

    fn fmt_now(&self) -> Format {
        Format { delim: self.delim.unwrap_or(self.sniffed.fmt.delim), quote: self.sniffed.fmt.quote }
    }

    /// Tolkar texten på nytt när den eller avgränsaren ändrats.
    fn sync(&mut self, text: &str) {
        let key = (text.len(), self.rev);
        let fmt = self.fmt_now();
        if key == self.key && fmt == self.fmt {
            return;
        }
        let fmt_changed = fmt != self.fmt;
        let first_time = self.key.0 == usize::MAX;
        self.fmt = fmt;
        self.table = Table::parse(text, fmt);
        self.key = key;
        if first_time {
            self.cur = (self.hdr(), 0);
            self.anchor = self.cur;
        }
        self.view_key = 0;
        self.stats_key = 0;
        if fmt_changed || self.widths.len() != self.table.ncols().max(1) {
            self.widths = vec![0.0; self.table.ncols().max(1)];
        }
        let (rows, cols) = (self.table.rows().max(1), self.table.ncols().max(1));
        self.cur = (self.cur.0.min(rows - 1), self.cur.1.min(cols - 1));
        self.anchor = (self.anchor.0.min(rows - 1), self.anchor.1.min(cols - 1));
        self.extra.clear();
        if self.edit.as_ref().is_some_and(|e| e.row >= rows || e.col >= cols) {
            self.edit = None;
        }
    }

    // -------------------------------------------------- rader som visas

    fn has_hdr(&self) -> bool {
        self.header && self.table.rows() > 0
    }

    fn hdr(&self) -> usize {
        usize::from(self.has_hdr())
    }

    fn refresh_view(&mut self, text: &str) {
        if self.filter.is_empty() {
            self.view = None;
            return;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.filter, self.filter_col, self.hdr(), self.key).hash(&mut h);
        let key = h.finish() | 1;
        if key == self.view_key && self.view.is_some() {
            return;
        }
        self.view_key = key;
        let needle = self.filter.to_lowercase();
        let rows = (self.hdr()..self.table.rows())
            .filter(|&r| {
                let n = self.table.row_len(r);
                let hit = |c: usize| self.table.value(text, r, c).to_lowercase().contains(&needle);
                match self.filter_col {
                    Some(c) => hit(c),
                    None => (0..n).any(hit),
                }
            })
            .collect();
        self.view = Some(rows);
    }

    /// Antal dataraders platser i vyn (en tom fil visas som en tom rad).
    fn nvis(&self) -> usize {
        match &self.view {
            Some(v) => v.len(),
            None => self.table.rows().saturating_sub(self.hdr()).max(usize::from(self.table.rows() == 0)),
        }
    }

    fn vis_row(&self, k: usize) -> usize {
        match &self.view {
            Some(v) => v.get(k).copied().unwrap_or(0),
            None => self.hdr() + k,
        }
    }

    fn vis_pos(&self, r: usize) -> Option<usize> {
        if r < self.hdr() {
            return None;
        }
        match &self.view {
            Some(v) => v.binary_search(&r).ok(),
            None => Some(r - self.hdr()),
        }
    }

    fn disp_len(&self) -> usize {
        self.hdr() + self.nvis()
    }

    fn disp_row(&self, d: usize) -> usize {
        if self.has_hdr() {
            if d == 0 { 0 } else { self.vis_row(d - 1) }
        } else {
            self.vis_row(d)
        }
    }

    /// Visningsrad för en tabellrad; en dold rad blir närmaste synliga före den.
    fn row_disp(&self, r: usize) -> usize {
        if self.has_hdr() && r == 0 {
            return 0;
        }
        match &self.view {
            Some(v) => {
                let k = v.partition_point(|&x| x < r);
                (k.min(v.len().saturating_sub(1)) + self.hdr()).min(self.disp_len().saturating_sub(1))
            }
            None => r.min(self.disp_len().saturating_sub(1)),
        }
    }

    fn ncols(&self) -> usize {
        self.table.ncols().max(1)
    }

    // -------------------------------------------------- markering

    fn active_rect(&self) -> CellRect {
        CellRect::between(self.anchor, self.cur)
    }

    fn rects(&self) -> Vec<CellRect> {
        let mut v = self.extra.clone();
        v.push(self.active_rect());
        v
    }

    fn is_sel(&self, r: usize, c: usize) -> bool {
        self.active_rect().contains(r, c) || self.extra.iter().any(|x| x.contains(r, c))
    }

    fn rect_rows(&self, rc: &CellRect) -> Vec<usize> {
        let mut v = Vec::new();
        if self.has_hdr() && rc.r0 == 0 {
            v.push(0);
        }
        let lo = rc.r0.max(self.hdr());
        if rc.r1 >= lo {
            match &self.view {
                None => {
                    let hi = rc.r1.min(self.table.rows().max(1) - 1);
                    if hi >= lo {
                        v.extend(lo..=hi);
                    }
                }
                Some(vw) => {
                    let a = vw.partition_point(|&x| x < lo);
                    let b = vw.partition_point(|&x| x <= rc.r1);
                    v.extend_from_slice(&vw[a..b]);
                }
            }
        }
        v
    }

    fn sel_cells(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for rc in self.rects() {
            for r in self.rect_rows(&rc) {
                for c in rc.c0..=rc.c1 {
                    out.push((r, c));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    fn sel_rows(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.rects().iter().flat_map(|rc| self.rect_rows(rc)).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn sel_cols(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.rects().iter().flat_map(|rc| rc.c0..=rc.c1).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn col_selected(&self, c: usize) -> bool {
        self.active_rect().c0 <= c && c <= self.active_rect().c1 || self.extra.iter().any(|x| x.c0 <= c && c <= x.c1)
    }

    fn select_one(&mut self, r: usize, c: usize) {
        self.cur = (r, c);
        self.anchor = (r, c);
        self.extra.clear();
    }

    fn select_all(&mut self) {
        let rows = self.table.rows().max(1);
        self.extra.clear();
        self.anchor = (0, 0);
        self.cur = (rows - 1, self.ncols() - 1);
    }

    fn update_stats(&mut self, text: &str) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.rects(), self.key, self.view_key).hash(&mut h);
        let key = h.finish() | 1;
        if key == self.stats_key {
            return;
        }
        self.stats_key = key;
        let cells = self.sel_cells();
        self.stats = (cells.len() > 1).then(|| stats::compute(cells.iter().map(|&(r, c)| self.table.value(text, r, c))));
    }

    /// Flyttar markören i visningsordning.
    fn move_cur(&mut self, dr: isize, dc: isize, extend: bool) {
        let d = self.row_disp(self.cur.0) as isize;
        let nd = (d + dr).clamp(0, self.disp_len() as isize - 1).max(0) as usize;
        let nc = (self.cur.1 as isize + dc).clamp(0, self.ncols() as isize - 1) as usize;
        self.cur = (self.disp_row(nd), nc);
        if !extend {
            self.anchor = self.cur;
            self.extra.clear();
        }
        self.scroll_to = true;
    }

    fn jump_cur(&mut self, row: Option<bool>, col: Option<bool>, extend: bool) {
        // `Some(false)` = första, `Some(true)` = sista.
        let mut r = self.cur.0;
        let mut c = self.cur.1;
        if let Some(last) = row {
            r = self.disp_row(if last { self.disp_len() - 1 } else { 0 });
        }
        if let Some(last) = col {
            c = if last { self.ncols() - 1 } else { 0 };
        }
        self.cur = (r, c);
        if !extend {
            self.anchor = self.cur;
            self.extra.clear();
        }
        self.scroll_to = true;
    }

    // -------------------------------------------------- ändringar

    fn apply(&mut self, doc: &mut Document, patch: Patch) -> bool {
        if patch.is_empty() {
            return false;
        }
        let inv = patch.inverse();
        patch.apply(&mut doc.text);
        doc.on_edited();
        self.undo.push(inv);
        self.redo.clear();
        let mut total: usize = self.undo.iter().map(Patch::size).sum();
        while total > UNDO_LIMIT_BYTES && self.undo.len() > 1 {
            total -= self.undo.remove(0).size();
        }
        self.sorted = None;
        self.rev = doc.rev;
        self.sync(&doc.text);
        true
    }

    fn step_history(&mut self, doc: &mut Document, redo: bool) {
        let patch = if redo { self.redo.pop() } else { self.undo.pop() };
        let Some(p) = patch else { return };
        if !p.fits(&doc.text) {
            // Texten har ändrats utanför tabellen: historiken gäller inte längre.
            self.undo.clear();
            self.redo.clear();
            return;
        }
        let back = p.inverse();
        p.apply(&mut doc.text);
        doc.on_edited();
        if redo { self.undo.push(back) } else { self.redo.push(back) }
        self.sorted = None;
        self.edit = None;
        self.rev = doc.rev;
        self.sync(&doc.text);
    }

    fn begin_edit(&mut self, text: &str, initial: Option<String>) {
        let (r, c) = self.cur;
        let buf = initial.unwrap_or_else(|| self.table.value(text, r, c).into_owned());
        self.edit = Some(CellEdit { row: r, col: c, buf, fresh: true });
        self.scroll_to = true;
    }

    /// Avslutar redigeringen och skriver cellen. Returnerar sant om texten ändrades.
    fn commit_edit(&mut self, doc: &mut Document) -> bool {
        let Some(e) = self.edit.take() else { return false };
        if self.table.value(&doc.text, e.row, e.col) == e.buf.as_str() && e.row < self.table.rows() {
            return false;
        }
        let patch = self.table.set_values(&doc.text, &[(e.row, e.col, e.buf)]);
        self.apply(doc, patch)
    }

    fn clear_cells(&mut self, doc: &mut Document) {
        let changes: Vec<(usize, usize, String)> = self
            .sel_cells()
            .into_iter()
            .filter(|&(r, c)| r < self.table.rows() && !self.table.value(&doc.text, r, c).is_empty())
            .map(|(r, c)| (r, c, String::new()))
            .collect();
        let patch = self.table.set_values(&doc.text, &changes);
        self.apply(doc, patch);
    }

    /// Markeringen som tabbavgränsad text (aktiv rektangel).
    fn copy_text(&self, text: &str) -> String {
        let rc = self.active_rect();
        let rows = self.rect_rows(&rc);
        let m: Vec<Vec<String>> = rows
            .iter()
            .map(|&r| (rc.c0..=rc.c1).map(|c| self.table.value(text, r, c).into_owned()).collect())
            .collect();
        model::to_tsv(&m)
    }

    fn paste(&mut self, doc: &mut Document, clip: &str) {
        let m = model::from_tsv(clip);
        if m.is_empty() {
            return;
        }
        let mut changes = Vec::new();
        let single = m.len() == 1 && m[0].len() == 1;
        let cells = self.sel_cells();
        if single && cells.len() > 1 {
            // En enda cell fyller hela markeringen.
            for (r, c) in &cells {
                changes.push((*r, *c, m[0][0].clone()));
            }
        } else {
            let rc = self.active_rect();
            let (r0, c0) = (rc.r0, rc.c0);
            for (i, row) in m.iter().enumerate() {
                for (j, v) in row.iter().enumerate() {
                    changes.push((r0 + i, c0 + j, v.clone()));
                }
            }
            let (h, w) = (m.len(), m.iter().map(Vec::len).max().unwrap_or(1));
            self.extra.clear();
            self.anchor = (r0, c0);
            self.cur = (r0 + h - 1, c0 + w - 1);
        }
        let n = changes.len();
        let patch = self.table.set_values(&doc.text, &changes);
        if self.apply(doc, patch) {
            self.toast = Some(tf!("Klistrade in {n} celler", n = n));
            // Rader och kolumner kan ha tillkommit.
            let cols = self.ncols();
            self.cur.1 = self.cur.1.min(cols - 1);
            self.anchor.1 = self.anchor.1.min(cols - 1);
        }
        self.scroll_to = true;
    }

    fn insert_rows(&mut self, doc: &mut Document, below: bool) {
        let rows = self.sel_rows();
        let (Some(&first), Some(&last)) = (rows.first(), rows.last()) else { return };
        let count = rows.len().max(1);
        let at = if below { last + 1 } else { first.max(self.hdr()) };
        let col = self.cur.1;
        let patch = self.table.insert_rows(&doc.text, at, count);
        if self.apply(doc, patch) {
            // Filtret kan dölja de nya tomma raderna: nollställ det inte, men markera dem.
            self.select_one(at, col);
            self.cur = (at + count - 1, col);
            self.scroll_to = true;
        }
    }

    fn delete_rows(&mut self, doc: &mut Document) {
        let rows: Vec<usize> = self.sel_rows().into_iter().filter(|&r| r >= self.hdr()).collect();
        let Some(&first) = rows.first() else { return };
        let col = self.cur.1;
        let patch = self.table.delete_rows(&doc.text, &rows);
        if self.apply(doc, patch) {
            let last = self.table.rows().saturating_sub(1);
            self.select_one(first.min(last), col);
            self.scroll_to = true;
        }
    }

    fn insert_cols(&mut self, doc: &mut Document, right: bool) {
        let cols = self.sel_cols();
        let (Some(&first), Some(&last)) = (cols.first(), cols.last()) else { return };
        let count = cols.len().max(1);
        let at = if right { last + 1 } else { first };
        let patch = self.table.insert_cols(&doc.text, at, count);
        let old_w = self.widths.clone();
        if self.apply(doc, patch) {
            let mut w = old_w;
            for _ in 0..count {
                if at <= w.len() {
                    w.insert(at, 0.0);
                }
            }
            w.resize(self.ncols(), 0.0);
            self.widths = w;
            self.anchor = (self.cur.0, at);
            self.cur = (self.cur.0, at + count - 1);
            self.extra.clear();
            self.scroll_to = true;
        }
    }

    fn delete_cols(&mut self, doc: &mut Document) {
        let cols = self.sel_cols();
        let Some(&first) = cols.first() else { return };
        let patch = self.table.delete_cols(&doc.text, &cols);
        let old_w = self.widths.clone();
        if self.apply(doc, patch) {
            let mut w: Vec<f32> = old_w.iter().enumerate().filter(|(i, _)| !cols.contains(i)).map(|(_, w)| *w).collect();
            w.resize(self.ncols(), 0.0);
            self.widths = w;
            let c = first.min(self.ncols() - 1);
            self.select_one(self.cur.0, c);
        }
    }

    fn sort(&mut self, doc: &mut Document, col: usize, desc: bool) {
        let from = self.hdr();
        if let Some(patch) = self.table.sort_rows(&doc.text, col, desc, self.sort_mode, from) {
            if self.apply(doc, patch) {
                self.sorted = Some((col, desc, self.key));
            }
        } else {
            self.sorted = Some((col, desc, self.key));
        }
    }

    fn run(&mut self, doc: &mut Document, act: Act, clip_out: &mut Option<String>) {
        match act {
            Act::Copy => *clip_out = Some(self.copy_text(&doc.text)),
            Act::Cut => {
                *clip_out = Some(self.copy_text(&doc.text));
                self.clear_cells(doc);
            }
            Act::PasteClipboard => {
                if let Some(s) = read_clipboard() {
                    self.paste(doc, &s);
                }
            }
            Act::Clear => self.clear_cells(doc),
            Act::InsertRow { below } => self.insert_rows(doc, below),
            Act::DeleteRows => self.delete_rows(doc),
            Act::InsertCol { right } => self.insert_cols(doc, right),
            Act::DeleteCols => self.delete_cols(doc),
            Act::Sort { col, desc } => self.sort(doc, col, desc),
            Act::Mode(m) => self.sort_mode = m,
            Act::AutoFit(c) => {
                if let Some(w) = self.widths.get_mut(c) {
                    *w = 0.0;
                }
            }
            Act::Undo => self.step_history(doc, false),
            Act::Redo => self.step_history(doc, true),
        }
    }

    /// Beräknar bredder som ännu saknas utifrån de längsta värdena i de första raderna.
    fn ensure_widths(&mut self, ui: &Ui, text: &str, font_size: f32) {
        if self.widths.len() != self.ncols() {
            self.widths.resize(self.ncols(), 0.0);
        }
        if self.widths.iter().all(|w| *w > 0.0) {
            return;
        }
        let font = FontId::new(font_size, FontFamily::Proportional);
        let bold = FontId::new(font_size, theme::semibold());
        let sample = if self.ncols() > 80 { 20 } else { 120 };
        let measure = |s: &str, f: &FontId| -> f32 {
            let s: String = s.chars().take(60).collect();
            ui.fonts_mut(|fo| fo.layout_no_wrap(s, f.clone(), Color32::WHITE).size().x)
        };
        for c in 0..self.ncols() {
            if self.widths[c] > 0.0 {
                continue;
            }
            let mut best = String::new();
            let mut best_n = 0;
            for r in (self.hdr()..self.table.rows()).take(sample) {
                let v = self.table.value(text, r, c);
                let n = v.chars().count().min(60);
                if n > best_n {
                    best_n = n;
                    best = v.into_owned();
                }
            }
            let mut w = measure(&best, &font);
            let head = if self.has_hdr() { self.table.value(text, 0, c).into_owned() } else { col_letters(c) };
            w = w.max(measure(&head, &bold) + 22.0);
            self.widths[c] = (w + 18.0).clamp(MIN_COL_W + 24.0, MAX_AUTO_W);
        }
    }
}

/// A, B, … Z, AA, AB …
pub fn col_letters(mut c: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

fn read_clipboard() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// Fliknas tabellvyer i minnet.
#[derive(Default)]
pub struct State {
    tabs: HashMap<u64, Tab>,
}

impl State {
    pub fn tab_for(&mut self, doc: &Document) -> &mut Tab {
        self.tabs.entry(doc.id).or_insert_with(|| Tab::new(doc))
    }

    /// Visas fliken som tabell?
    pub fn is_grid(&mut self, doc: &Document) -> bool {
        if doc.docx || doc.pdf || doc.pdfv.is_some() {
            return false;
        }
        let tab = self.tab_for(doc);
        tab.refresh_available(doc);
        tab.grid
    }

    /// Hoppa till cellen som innehåller byteläget (sökträff).
    pub fn goto_byte(&mut self, doc_id: u64, pos: usize) {
        if let Some(t) = self.tabs.get_mut(&doc_id) {
            if t.grid {
                t.goto = Some(pos);
            }
        }
    }

    pub fn retain_docs(&mut self, ids: impl Iterator<Item = u64>) {
        let ids: std::collections::HashSet<u64> = ids.collect();
        self.tabs.retain(|id, _| ids.contains(id));
    }
}

/// Segmentväljaren "Tabell | Text" i statusraden. Returnerar den nya vyn om man klickade.
pub fn mode_switch(ui: &mut Ui, p: &Palette, grid: bool) -> Option<bool> {
    let font = FontId::new(12.0, FontFamily::Proportional);
    let mut out = None;
    // Högerjusterad layout: Text först (längst till höger), sedan Tabell.
    for (label, is_grid, tip) in [
        (t("Text"), false, t("Visa filen som text")),
        (t("Tabell"), true, t("Visa filen som tabell")),
    ] {
        let on = grid == is_grid;
        let galley = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), if on { p.accent } else { p.muted });
        let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 14.0, 20.0), Sense::click());
        let resp = resp.on_hover_text(tip);
        if on {
            ui.painter().rect_stroke(rect, 5.0, Stroke::new(1.0, p.accent), StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 5.0, p.surface_hover);
        }
        let color = if on { p.accent } else if resp.hovered() { p.text } else { p.muted };
        ui.painter().galley(pos2(rect.left() + 7.0, rect.center().y - galley.size().y / 2.0), galley, color);
        if resp.clicked() && !on {
            out = Some(is_grid);
        }
    }
    ui.add_space(4.0);
    out
}

/// Texten i statusraden i tabellvyn: position och markeringens statistik.
pub fn status_text(tab: &Tab) -> String {
    let mut s = if tab.has_hdr() && tab.cur.0 == 0 {
        tf!("Rubrikrad, kolumn {col}", col = col_letters(tab.cur.1))
    } else {
        tf!("Rad {row}, kolumn {col}", row = tab.cur.0 + 1 - tab.hdr(), col = col_letters(tab.cur.1))
    };
    if let Some(st) = &tab.stats {
        s.push_str("   ");
        s.push_str(&tf!("Antal {n}", n = st.count));
        if st.numeric > 0 {
            s.push_str("   ");
            s.push_str(&tf!("Summa {v}", v = stats::format_number(st.sum)));
            if let Some(a) = st.average() {
                s.push_str("   ");
                s.push_str(&tf!("Medel {v}", v = stats::format_number(a)));
            }
            s.push_str("   ");
            s.push_str(&tf!("Min {v}", v = stats::format_number(st.min)));
            s.push_str("   ");
            s.push_str(&tf!("Max {v}", v = stats::format_number(st.max)));
        }
    }
    s
}

struct Geometry {
    row_h: f32,
    hdr_h: f32,
    gutter: f32,
    xs: Vec<f32>,
}

fn delim_name(d: u8) -> &'static str {
    match d {
        b';' => t("Semikolon (;)"),
        b'\t' => t("Tabb"),
        b'|' => t("Lodrätt streck (|)"),
        _ => t("Komma (,)"),
    }
}

impl KladdApp {
    /// Är den aktiva fliken en tabell?
    pub(crate) fn csv_mode(&mut self) -> bool {
        match self.docs.get(self.active) {
            Some(doc) => self.csv.is_grid(doc),
            None => false,
        }
    }

    pub(crate) fn csv_page(&mut self, ui: &mut Ui, p: &Palette) {
        CentralPanel::default().frame(Frame::NONE.fill(p.editor)).show(ui, |ui| {
            self.editor_rect = ui.max_rect();
            let ctx = ui.ctx().clone();
            let font_size = self.settings.font_size;
            let matches = self.search_highlights();
            let focus = std::mem::take(&mut self.focus_editor);
            self.scroll_to_char = None;
            let find_open = self.find.open;
            let salt = crate::panes::salt();
            let Some(doc) = self.docs.get_mut(self.active) else { return };
            let tab = self.csv.tab_for(doc);
            let toast = grid_ui(ui, p, tab, doc, font_size, &matches, focus, find_open, salt);
            if let Some(msg) = toast {
                self.notify(&ctx, msg, false);
            }
        });
    }
}

/// Toolbar och rutnät. Returnerar ett meddelande att visa.
#[allow(clippy::too_many_arguments)]
fn grid_ui(
    ui: &mut Ui,
    p: &Palette,
    tab: &mut Tab,
    doc: &mut Document,
    font_size: f32,
    matches: &[(usize, usize, bool)],
    want_focus: bool,
    find_open: bool,
    salt: u8,
) -> Option<String> {
    tab.rev = doc.rev;
    tab.sync(&doc.text);
    tab.refresh_view(&doc.text);
    tab.ensure_widths(ui, &doc.text, font_size);
    toolbar(ui, p, tab, doc);
    let rect = ui.available_rect_before_wrap();
    ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.line));
    tab.refresh_view(&doc.text);
    grid(ui, p, tab, doc, font_size, matches, want_focus, find_open, salt);
    tab.update_stats(&doc.text);
    tab.toast.take()
}

fn toolbar(ui: &mut Ui, p: &Palette, tab: &mut Tab, doc: &Document) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 36.0), Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.x = 8.0;
        let q = ui.add(
            TextEdit::singleline(&mut tab.filter)
                .id(Id::unique(("csv-filter", doc.id)))
                .hint_text(t("Filtrera rader…"))
                .desired_width(200.0)
                .margin(egui::Margin::symmetric(8, 4)),
        );
        if q.changed() {
            tab.view_key = 0;
        }
        let label = match tab.filter_col {
            Some(c) if c < tab.ncols() => {
                if tab.has_hdr() {
                    let v = tab.table.value(&doc.text, 0, c).into_owned();
                    if v.is_empty() { col_letters(c) } else { v }
                } else {
                    col_letters(c)
                }
            }
            _ => t("Alla kolumner").to_owned(),
        };
        let btn = status_button(ui, p, &format!("{label} ▾"), t("Kolumnen som filtret söker i"));
        Popup::menu(&btn).gap(4.0).width(220.0).show(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if menu_item(ui, p, t("Alla kolumner"), "", Some(tab.filter_col.is_none()), true) {
                tab.filter_col = None;
                tab.view_key = 0;
            }
            for c in 0..tab.ncols().min(200) {
                let name = if tab.has_hdr() {
                    let v = tab.table.value(&doc.text, 0, c).into_owned();
                    if v.is_empty() { col_letters(c) } else { v }
                } else {
                    col_letters(c)
                };
                if menu_item(ui, p, &crate::app::truncate(&name, 28), "", Some(tab.filter_col == Some(c)), true) {
                    tab.filter_col = Some(c);
                    tab.view_key = 0;
                }
            }
        });
        if !tab.filter.is_empty() {
            let n = tab.nvis();
            let total = tab.table.rows().saturating_sub(tab.hdr());
            ui.label(egui::RichText::new(tf!("{n} av {total} rader", n = n, total = total)).size(12.0).color(p.muted));
            if ui.add(crate::app::quiet_button(t("Rensa filtret"), p)).clicked() {
                tab.filter.clear();
                tab.view_key = 0;
            }
        } else {
            let n = tab.table.rows().saturating_sub(tab.hdr());
            let s = if n == 1 { t("1 rad").to_owned() } else { tf!("{n} rader", n = n) };
            ui.label(egui::RichText::new(s).size(12.0).color(p.muted));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(10.0);
            let mut header = tab.header;
            if ui.checkbox(&mut header, t("Första raden är rubrik")).changed() {
                tab.header = header;
                tab.view_key = 0;
                tab.stats_key = 0;
                tab.widths.iter_mut().for_each(|w| *w = 0.0);
                tab.select_one(tab.cur.0.max(tab.hdr()), tab.cur.1);
            }
            let cur = tab.delim.unwrap_or(tab.sniffed.fmt.delim);
            let name = if tab.delim.is_none() { tf!("Avgränsare: {name} (automatisk)", name = delim_name(cur)) } else { tf!("Avgränsare: {name}", name = delim_name(cur)) };
            let btn = status_button(ui, p, &format!("{name} ▾"), t("Avgränsare mellan kolumnerna"));
            Popup::menu(&btn).align(egui::RectAlign::BOTTOM_END).gap(4.0).width(220.0).show(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if menu_item(ui, p, &tf!("Automatisk ({name})", name = delim_name(tab.sniffed.fmt.delim)), "", Some(tab.delim.is_none()), true) {
                    tab.delim = None;
                }
                for d in DELIMS {
                    if menu_item(ui, p, delim_name(d), "", Some(tab.delim == Some(d)), true) {
                        tab.delim = Some(d);
                    }
                }
            });
        });
    });
}

fn px_to_col(xs: &[f32], x: f32) -> Option<usize> {
    if x < 0.0 || xs.len() < 2 || x >= *xs.last()? {
        return None;
    }
    Some(xs.partition_point(|&v| v <= x) - 1)
}

#[allow(clippy::too_many_arguments)]
fn grid(
    ui: &mut Ui,
    p: &Palette,
    tab: &mut Tab,
    doc: &mut Document,
    font_size: f32,
    matches: &[(usize, usize, bool)],
    want_focus: bool,
    find_open: bool,
    salt: u8,
) {
    let ctx = ui.ctx().clone();
    let ncols = tab.ncols();
    let row_h = (font_size * 1.75).round().max(22.0);
    let hdr_h = row_h + 4.0;
    let nvis = tab.nvis();
    let digits = (tab.table.rows().max(10)).to_string().len() as f32;
    let gutter = (digits * font_size * 0.62 + 24.0).max(44.0);
    let mut xs = Vec::with_capacity(ncols + 1);
    xs.push(0.0);
    for c in 0..ncols {
        xs.push(xs[c] + tab.widths.get(c).copied().unwrap_or(100.0).max(MIN_COL_W));
    }
    let geo = Geometry { row_h, hdr_h, gutter, xs };
    let total_w = geo.gutter + geo.xs[ncols] + 40.0;
    let total_h = geo.hdr_h + nvis as f32 * geo.row_h + 40.0;

    let font = FontId::new(font_size, FontFamily::Proportional);
    let bold = FontId::new(font_size, theme::semibold());
    let small = FontId::new((font_size - 2.0).max(9.0), FontFamily::Proportional);
    let grid_line = Stroke::new(1.0, p.line.gamma_multiply(0.55));
    let mut acts: Vec<Act> = Vec::new();
    let mut clip_out: Option<String> = None;
    let mut move_after_commit: Option<(isize, isize)> = None;
    let mut start_edit: Option<Option<String>> = None;
    let mut grid_id: Option<Id> = None;
    let mut refocus = false;
    let id_salt = ("csv-grid", doc.id, salt);

    ScrollArea::both().id_salt(id_salt).auto_shrink([false, false]).show_viewport(ui, |ui, vp| {
        let size = vec2(total_w.max(vp.width()), total_h.max(vp.height()));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let o = rect.min;
        let view_left = o.x + vp.min.x;
        let view_top = o.y + vp.min.y;
        let clip = ui.clip_rect();
        let body_clip = Rect::from_min_max(pos2(view_left + geo.gutter, view_top + geo.hdr_h), clip.max);
        let painter = ui.painter().clone();
        tab.page_rows = (((vp.height() - geo.hdr_h) / geo.row_h).floor() as usize).max(1);

        // Hoppa till sökträff och rulla till markören.
        if let Some(pos) = tab.goto.take() {
            if let Some((r, c)) = tab.table.cell_at(pos) {
                tab.select_one(r, c.min(ncols - 1));
                tab.scroll_to = true;
            }
        }
        let cell_rect = |k: usize, c: usize| -> Rect {
            Rect::from_min_size(pos2(o.x + geo.gutter + geo.xs[c], o.y + geo.hdr_h + k as f32 * geo.row_h), vec2(geo.xs[c + 1] - geo.xs[c], geo.row_h))
        };
        if tab.scroll_to {
            tab.scroll_to = false;
            let r = tab.cur.0.min(tab.table.rows().max(1) - 1);
            let c = tab.cur.1.min(ncols - 1);
            let on_header = tab.has_hdr() && r == 0;
            let k = tab.vis_pos(r).unwrap_or(0);
            let mut cr = cell_rect(k, c);
            if on_header {
                cr = Rect::from_min_size(pos2(cr.min.x, view_top), cr.size());
            }
            // Ge plats åt rubrikraden och radnumren som ligger över cellerna.
            let target = Rect::from_min_max(cr.min - vec2(geo.gutter, if on_header { 0.0 } else { geo.hdr_h }), cr.max);
            ui.scroll_to_rect(target, None);
        }

        // Pekarens läge → vad som finns där.
        let (hdr_n, has_hdr_now) = (tab.hdr(), tab.has_hdr());
        let _ = has_hdr_now;
        let hit_at = |pos: egui::Pos2| -> Hit {
            let lx = pos.x - view_left;
            let ly = pos.y - view_top;
            if lx < 0.0 || ly < 0.0 {
                return Hit::Outside;
            }
            let cx = lx + vp.min.x - geo.gutter;
            if ly < geo.hdr_h {
                if lx < geo.gutter {
                    return Hit::Corner;
                }
                if let Some(c) = px_to_col(&geo.xs, cx) {
                    if (geo.xs[c + 1] - cx).abs() <= 4.0 {
                        return Hit::Border(c);
                    }
                    if cx > geo.xs[c + 1] - 20.0 {
                        return Hit::Sort(c);
                    }
                    return Hit::Col(c);
                }
                // Precis vid kanten av sista kolumnen räknas som kant.
                if let Some(last) = ncols.checked_sub(1) {
                    if (geo.xs[last + 1] - cx).abs() <= 4.0 {
                        return Hit::Border(last);
                    }
                }
                return Hit::Outside;
            }
            let cy = ly + vp.min.y - geo.hdr_h;
            let k = (cy / geo.row_h).floor() as usize;
            if k >= nvis {
                return Hit::Outside;
            }
            let d = k + hdr_n;
            if lx < geo.gutter {
                return Hit::RowNo(d);
            }
            match px_to_col(&geo.xs, cx) {
                Some(c) => Hit::Cell(d, c),
                None => Hit::Outside,
            }
        };

        // ------------------------------------------------ rita cellerna
        let first_k = ((vp.min.y / geo.row_h).floor() as isize).max(0) as usize;
        let last_k = ((((vp.max.y - geo.hdr_h) / geo.row_h).ceil()).max(0.0) as usize + 1).min(nvis);
        let c_first = (geo.xs.partition_point(|&x| x <= vp.min.x).saturating_sub(1)).min(ncols - 1);
        let c_last = geo.xs.partition_point(|&x| x < vp.max.x - geo.gutter).min(ncols);

        let match_in = |span: std::ops::Range<usize>| -> Option<bool> {
            let i = matches.partition_point(|m| m.1 <= span.start);
            let mut found = None;
            for m in &matches[i..] {
                if m.0 >= span.end && !(m.0 == span.start && m.1 == m.0) {
                    break;
                }
                if m.1 > m.0 {
                    found = Some(found.unwrap_or(false) || m.2);
                }
            }
            found
        };

        let cells_painter = painter.with_clip_rect(body_clip.intersect(clip));
        for k in first_k..last_k {
            let r = tab.vis_row(k);
            let y = o.y + geo.hdr_h + k as f32 * geo.row_h;
            let row_rect = Rect::from_min_size(pos2(o.x + geo.gutter, y), vec2(geo.xs[ncols], geo.row_h));
            let row_exists = r < tab.table.rows();
            for c in c_first..c_last {
                let cr = cell_rect(k, c);
                let selected = tab.is_sel(r, c);
                if selected {
                    cells_painter.rect_filled(cr, 0.0, p.selection);
                }
                if row_exists {
                    if let Some(span) = tab.table.cell_span(r, c) {
                        if !matches.is_empty() {
                            if let Some(cur) = match_in(span.clone()) {
                                cells_painter.rect_filled(cr.shrink(1.0), 2.0, if cur { p.match_current_bg } else { p.match_bg });
                            }
                        }
                        let v = tab.table.value(&doc.text, r, c);
                        if !v.is_empty() && tab.edit.as_ref().is_none_or(|e| !(e.row == r && e.col == c)) {
                            draw_value(&cells_painter, cr, &v, &font, p.text, ui);
                        }
                    }
                }
            }
            cells_painter.hline(row_rect.x_range(), y + geo.row_h, grid_line);
        }
        // Lodräta linjer.
        let y_end = (o.y + geo.hdr_h + nvis as f32 * geo.row_h).min(clip.bottom());
        for c in c_first..c_last {
            let x = o.x + geo.gutter + geo.xs[c + 1];
            cells_painter.vline(x, body_clip.top()..=y_end, grid_line);
        }
        // Markörcellen.
        if !(tab.has_hdr() && tab.cur.0 == 0) {
            if let Some(k) = tab.vis_pos(tab.cur.0.max(tab.hdr())) {
                if tab.cur.1 < ncols && (tab.cur.0 >= tab.hdr()) {
                    cells_painter.rect_stroke(cell_rect(k, tab.cur.1).shrink(0.5), 0.0, Stroke::new(2.0, p.accent), StrokeKind::Inside);
                }
            }
        }

        // ------------------------------------------------ rubrikraden (ligger kvar)
        let head_rect = Rect::from_min_max(pos2(view_left, view_top), pos2(clip.right().max(view_left), view_top + geo.hdr_h));
        let head_painter = painter.with_clip_rect(head_rect.intersect(clip));
        head_painter.rect_filled(head_rect, 0.0, p.chrome);
        let hover = ctx.pointer_hover_pos().map(&hit_at);
        for c in c_first..c_last {
            let x0 = o.x + geo.gutter + geo.xs[c];
            let x1 = o.x + geo.gutter + geo.xs[c + 1];
            let cr = Rect::from_min_max(pos2(x0, view_top), pos2(x1, view_top + geo.hdr_h));
            let sel = tab.col_selected(c) || (tab.has_hdr() && tab.cur.0 == 0 && tab.cur.1 == c);
            if sel {
                head_painter.rect_filled(cr, 0.0, p.accent.gamma_multiply(0.18));
            }
            let (label, color) = if tab.has_hdr() {
                (tab.table.value(&doc.text, 0, c).into_owned(), p.text)
            } else {
                (col_letters(c), p.muted)
            };
            let text_area = Rect::from_min_max(cr.min, pos2(cr.max.x - 20.0, cr.max.y));
            if tab.edit.as_ref().is_none_or(|e| !(e.row == 0 && e.col == c && tab.has_hdr())) {
                draw_value(&head_painter, text_area, &label, &bold, color, ui);
            }
            // Sorteringspil.
            let sorted_here = tab.sorted.filter(|s| s.0 == c && s.2 == tab.key);
            let hovered = matches!(hover, Some(Hit::Col(h) | Hit::Sort(h)) if h == c);
            if sorted_here.is_some() || hovered {
                let ac = pos2(cr.max.x - 11.0, cr.center().y);
                let col = if sorted_here.is_some() { p.accent } else { p.faint };
                let up = Shape::convex_polygon(vec![ac + vec2(-4.0, 1.5), ac + vec2(4.0, 1.5), ac + vec2(0.0, -3.5)], col, Stroke::NONE);
                let down = Shape::convex_polygon(vec![ac + vec2(-4.0, -1.5), ac + vec2(4.0, -1.5), ac + vec2(0.0, 3.5)], col, Stroke::NONE);
                match sorted_here {
                    Some((_, false, _)) => {
                        head_painter.add(up);
                    }
                    Some(_) => {
                        head_painter.add(down);
                    }
                    None => {
                        head_painter.add(Shape::convex_polygon(vec![ac + vec2(-3.5, -1.0), ac + vec2(3.5, -1.0), ac + vec2(0.0, -4.5)], col, Stroke::NONE));
                        head_painter.add(Shape::convex_polygon(vec![ac + vec2(-3.5, 1.0), ac + vec2(3.5, 1.0), ac + vec2(0.0, 4.5)], col, Stroke::NONE));
                    }
                }
            }
            head_painter.vline(x1, cr.y_range(), Stroke::new(1.0, p.line));
        }
        head_painter.hline(head_rect.x_range(), view_top + geo.hdr_h, Stroke::new(1.0, p.line));

        // ------------------------------------------------ radnummer (ligger kvar)
        let gut_rect = Rect::from_min_max(pos2(view_left, view_top + geo.hdr_h), pos2(view_left + geo.gutter, clip.bottom().max(view_top + geo.hdr_h)));
        let gut_painter = painter.with_clip_rect(gut_rect.intersect(clip));
        gut_painter.rect_filled(gut_rect, 0.0, p.chrome);
        let sel_rows_now: Vec<CellRect> = tab.rects();
        for k in first_k..last_k {
            let r = tab.vis_row(k);
            let y = o.y + geo.hdr_h + k as f32 * geo.row_h;
            let rr = Rect::from_min_size(pos2(view_left, y), vec2(geo.gutter, geo.row_h));
            if sel_rows_now.iter().any(|s| r >= s.r0 && r <= s.r1) {
                gut_painter.rect_filled(rr, 0.0, p.accent.gamma_multiply(0.18));
            }
            let no = r + 1 - tab.hdr();
            gut_painter.text(pos2(rr.right() - 8.0, rr.center().y), Align2::RIGHT_CENTER, no.to_string(), small.clone(), p.faint);
            gut_painter.hline(rr.x_range(), y + geo.row_h, Stroke::new(1.0, p.line.gamma_multiply(0.55)));
        }
        gut_painter.vline(view_left + geo.gutter, gut_rect.y_range(), Stroke::new(1.0, p.line));
        // Hörnet.
        let corner = Rect::from_min_size(pos2(view_left, view_top), vec2(geo.gutter, geo.hdr_h));
        painter.with_clip_rect(corner.intersect(clip)).rect_filled(corner, 0.0, p.chrome);
        painter.hline(corner.x_range(), corner.bottom(), Stroke::new(1.0, p.line));
        painter.vline(corner.right(), corner.y_range(), Stroke::new(1.0, p.line));

        // ------------------------------------------------ redigering av en cell
        let mut edit_rect = None;
        if let Some(e) = &tab.edit {
            let on_header = tab.has_hdr() && e.row == 0;
            let k = if on_header { Some(0) } else { tab.vis_pos(e.row).or(if tab.table.rows() == 0 { Some(0) } else { None }) };
            match k {
                Some(k) if e.col < ncols => {
                    let mut cr = cell_rect(k, e.col);
                    if on_header {
                        cr = Rect::from_min_size(pos2(cr.min.x, view_top), vec2(cr.width(), geo.hdr_h));
                    }
                    edit_rect = Some(cr);
                }
                _ => tab.edit = None,
            }
        }
        let editing = tab.edit.is_some();
        let (mut enter, mut tabk, mut esc) = (None, None, false);
        if let (true, Some(cr)) = (editing, edit_rect) {
            let eid = Id::unique(("csv-cell", doc.id, salt));
            ui.input_mut(|i| {
                let shift = i.modifiers.shift;
                if !i.modifiers.alt && i.consume_key(Modifiers::NONE, Key::Enter) {
                    enter = Some(if shift { -1 } else { 1 });
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) {
                    tabk = Some(if shift { -1 } else { 1 });
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    esc = true;
                }
            });
            let e = tab.edit.as_mut().expect("redigering pågår");
            if e.fresh {
                let mut st = TextEditState::load(&ctx, eid).unwrap_or_default();
                let n = e.buf.chars().count();
                st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(n))));
                st.store(&ctx, eid);
            }
            let widen = {
                let w = ui.fonts_mut(|f| f.layout_no_wrap(e.buf.lines().next().unwrap_or("").to_owned(), font.clone(), Color32::WHITE).size().x);
                (w + 24.0).max(cr.width())
            };
            let er = Rect::from_min_size(cr.min, vec2(widen, cr.height()));
            painter.rect_filled(er, 0.0, p.editor);
            painter.rect_stroke(er, 0.0, Stroke::new(2.0, p.accent), StrokeKind::Inside);
            let out = ui.scope_builder(UiBuilder::new().max_rect(er), |ui| {
                ui.add(
                    TextEdit::multiline(&mut e.buf)
                        .id(eid)
                        .font(font.clone())
                        .frame(Frame::NONE)
                        .desired_rows(1)
                        .desired_width(er.width())
                        .margin(egui::Margin::symmetric(5, 0))
                        .align(Align2::LEFT_CENTER)
                        .return_key(KeyboardShortcut::new(Modifiers::ALT, Key::Enter))
                        .clip_text(true),
                )
            });
            let te = out.inner;
            if e.fresh {
                te.request_focus();
                e.fresh = false;
            }
            if esc {
                tab.edit = None;
                refocus = true;
            } else if enter.is_some() || tabk.is_some() {
                // Åtgärden körs efter ritningen.
                refocus = true;
                move_after_commit = Some(match (enter, tabk) {
                    (Some(d), _) => (d, 0),
                    (_, Some(d)) => (0, d),
                    _ => (0, 0),
                });
            } else if te.lost_focus() {
                move_after_commit = Some((0, 0));
            }
        }

        // ------------------------------------------------ pekaren
        if resp.hovered() || resp.dragged() || tab.resize.is_some() {
            if let Some(pos) = ctx.pointer_hover_pos() {
                match hit_at(pos) {
                    Hit::Border(_) => ctx.set_cursor_icon(CursorIcon::ResizeColumn),
                    Hit::Sort(_) => ctx.set_cursor_icon(CursorIcon::PointingHand),
                    _ => {}
                }
            }
            if tab.resize.is_some() {
                ctx.set_cursor_icon(CursorIcon::ResizeColumn);
            }
        }
        let mods = ui.input(|i| i.modifiers);
        let mut commit_first = false;
        if resp.drag_started() {
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                commit_first = true;
                resp.request_focus();
                match hit_at(pos) {
                    Hit::Border(c) => tab.resize = Some((c, geo.xs[c + 1] - geo.xs[c], pos.x)),
                    Hit::Cell(d, c) => {
                        let r = tab.disp_row(d);
                        if mods.shift {
                            tab.cur = (r, c);
                        } else {
                            if mods.command {
                                let rc = tab.active_rect();
                                tab.extra.push(rc);
                            } else {
                                tab.extra.clear();
                            }
                            tab.anchor = (r, c);
                            tab.cur = (r, c);
                        }
                    }
                    Hit::Col(c) | Hit::Sort(c) => {
                        let last = tab.table.rows().max(1) - 1;
                        tab.extra.clear();
                        tab.anchor = (0, c);
                        tab.cur = (last, c);
                    }
                    Hit::RowNo(d) => {
                        let r = tab.disp_row(d);
                        tab.extra.clear();
                        tab.anchor = (r, 0);
                        tab.cur = (r, ncols - 1);
                    }
                    _ => {}
                }
            }
        }
        if resp.dragged() && !tab.edit.is_some() {
            if let Some(pos) = ctx.pointer_latest_pos() {
                if let Some((c, w0, x0)) = tab.resize {
                    let w = (w0 + pos.x - x0).max(MIN_COL_W);
                    if let Some(slot) = tab.widths.get_mut(c) {
                        *slot = w;
                    }
                } else {
                    // Dra för att markera; rulla när pekaren lämnar ytan.
                    let mut p2 = pos;
                    p2.x = p2.x.clamp(body_clip.left() + 1.0, body_clip.right() - 1.0);
                    p2.y = p2.y.clamp(body_clip.top() + 1.0, body_clip.bottom() - 1.0);
                    match hit_at(p2) {
                        Hit::Cell(d, c) => {
                            let r = tab.disp_row(d);
                            tab.cur = (r, c);
                        }
                        _ => {}
                    }
                    let edge = |a: f32, lo: f32, hi: f32| if a < lo + 18.0 { 16.0 } else if a > hi - 18.0 { -16.0 } else { 0.0 };
                    let delta = vec2(edge(pos.x, body_clip.left(), body_clip.right()), edge(pos.y, body_clip.top(), body_clip.bottom()));
                    if delta != Vec2::ZERO {
                        ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
                        ctx.request_repaint();
                    }
                }
            }
        }
        if resp.drag_stopped() {
            tab.resize = None;
        }
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                commit_first = true;
                resp.request_focus();
                match hit_at(pos) {
                    Hit::Cell(d, c) => {
                        let r = tab.disp_row(d);
                        if mods.shift {
                            tab.cur = (r, c);
                        } else if mods.command {
                            let rc = tab.active_rect();
                            tab.extra.push(rc);
                            tab.anchor = (r, c);
                            tab.cur = (r, c);
                        } else {
                            tab.select_one(r, c);
                        }
                    }
                    Hit::Col(c) => {
                        let last = tab.table.rows().max(1) - 1;
                        tab.extra.clear();
                        tab.anchor = (0, c);
                        tab.cur = (last, c);
                    }
                    Hit::Sort(c) => {
                        let desc = tab.sorted.is_some_and(|s| s.0 == c && !s.1 && s.2 == tab.key);
                        acts.push(Act::Sort { col: c, desc });
                    }
                    Hit::RowNo(d) => {
                        let r = tab.disp_row(d);
                        tab.extra.clear();
                        tab.anchor = (r, 0);
                        tab.cur = (r, ncols - 1);
                    }
                    Hit::Corner => tab.select_all(),
                    _ => {}
                }
            }
        }
        if resp.double_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                match hit_at(pos) {
                    Hit::Cell(d, c) => {
                        tab.select_one(tab.disp_row(d), c);
                        start_edit = Some(None);
                    }
                    Hit::Col(c) if tab.has_hdr() => {
                        tab.select_one(0, c);
                        start_edit = Some(None);
                    }
                    Hit::Border(c) => acts.push(Act::AutoFit(c)),
                    _ => {}
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(pos) = ctx.pointer_interact_pos() {
                resp.request_focus();
                let h = hit_at(pos);
                tab.ctx_hit = Some(h);
                match h {
                    Hit::Cell(d, c) => {
                        let r = tab.disp_row(d);
                        if !tab.is_sel(r, c) {
                            tab.select_one(r, c);
                        }
                    }
                    Hit::Col(c) | Hit::Sort(c) | Hit::Border(c) => {
                        if !tab.col_selected(c) {
                            let last = tab.table.rows().max(1) - 1;
                            tab.extra.clear();
                            tab.anchor = (0, c);
                            tab.cur = (last, c);
                        }
                    }
                    Hit::RowNo(d) => {
                        let r = tab.disp_row(d);
                        if !tab.sel_rows().contains(&r) {
                            tab.extra.clear();
                            tab.anchor = (r, 0);
                            tab.cur = (r, ncols - 1);
                        }
                    }
                    _ => {}
                }
            }
        }
        if commit_first && tab.edit.is_some() && move_after_commit.is_none() {
            move_after_commit = Some((0, 0));
        }
        Popup::context_menu(&resp).width(250.0).show(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let col = match tab.ctx_hit {
                Some(Hit::Col(c) | Hit::Sort(c) | Hit::Border(c)) => Some(c),
                _ => None,
            };
            if let Some(c) = col {
                if menu_item(ui, p, t("Sortera stigande"), "", None, true) {
                    acts.push(Act::Sort { col: c, desc: false });
                    ui.close();
                }
                if menu_item(ui, p, t("Sortera fallande"), "", None, true) {
                    acts.push(Act::Sort { col: c, desc: true });
                    ui.close();
                }
                for (label, m) in [
                    (t("Sortera som: automatiskt"), SortMode::Auto),
                    (t("Sortera som: tal"), SortMode::Number),
                    (t("Sortera som: naturlig ordning"), SortMode::Natural),
                    (t("Sortera som: text"), SortMode::Text),
                ] {
                    if menu_item(ui, p, label, "", Some(tab.sort_mode == m), true) {
                        acts.push(Act::Mode(m));
                    }
                }
                if menu_item(ui, p, t("Anpassa kolumnbredd"), "", None, true) {
                    acts.push(Act::AutoFit(c));
                    ui.close();
                }
                ui.separator();
            }
            if menu_item(ui, p, t("Klipp ut"), "Ctrl+X", None, true) {
                acts.push(Act::Cut);
                ui.close();
            }
            if menu_item(ui, p, t("Kopiera"), "Ctrl+C", None, true) {
                acts.push(Act::Copy);
                ui.close();
            }
            if menu_item(ui, p, t("Klistra in"), "Ctrl+V", None, true) {
                acts.push(Act::PasteClipboard);
                ui.close();
            }
            if menu_item(ui, p, t("Rensa celler"), "Delete", None, true) {
                acts.push(Act::Clear);
                ui.close();
            }
            ui.separator();
            let nrows = tab.sel_rows().iter().filter(|&&r| r >= tab.hdr()).count();
            if menu_item(ui, p, t("Infoga rad ovanför"), "", None, true) {
                acts.push(Act::InsertRow { below: false });
                ui.close();
            }
            if menu_item(ui, p, t("Infoga rad nedanför"), "", None, true) {
                acts.push(Act::InsertRow { below: true });
                ui.close();
            }
            let del_rows = if nrows > 1 { tf!("Ta bort {n} rader", n = nrows) } else { t("Ta bort rad").to_owned() };
            if menu_item(ui, p, &del_rows, "", None, nrows > 0) {
                acts.push(Act::DeleteRows);
                ui.close();
            }
            ui.separator();
            let ncs = tab.sel_cols().len();
            if menu_item(ui, p, t("Infoga kolumn till vänster"), "", None, true) {
                acts.push(Act::InsertCol { right: false });
                ui.close();
            }
            if menu_item(ui, p, t("Infoga kolumn till höger"), "", None, true) {
                acts.push(Act::InsertCol { right: true });
                ui.close();
            }
            let del_cols = if ncs > 1 { tf!("Ta bort {n} kolumner", n = ncs) } else { t("Ta bort kolumn").to_owned() };
            if menu_item(ui, p, &del_cols, "", None, true) {
                acts.push(Act::DeleteCols);
                ui.close();
            }
        });

        // ------------------------------------------------ tangentbord
        let id = resp.id;
        grid_id = Some(id);
        if !editing && resp.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false });
            });
            let events = ui.input(|i| i.events.clone());
            for ev in &events {
                match ev {
                    Event::Copy => acts.push(Act::Copy),
                    Event::Cut => acts.push(Act::Cut),
                    Event::Paste(s) => {
                        let s = s.clone();
                        tab.paste(doc, &s);
                    }
                    Event::Text(s) if !mods.command && !mods.alt && !s.chars().all(char::is_control) => {
                        start_edit = Some(Some(s.clone()));
                    }
                    _ => {}
                }
            }
            let page = tab.page_rows as isize;
            let shift = mods.shift;
            let key = |m: Modifiers, k: Key| ui.input_mut(|i| i.consume_key(m, k));
            // Flyttar och konsumerar alla tryckningar under bildrutan (flera kan komma samtidigt).
            let nav = |k: Key| -> (isize, bool) {
                ui.input_mut(|i| {
                    let mut n = 0;
                    let mut sh = false;
                    for e in &i.events {
                        if let Event::Key { key, pressed: true, modifiers, .. } = e {
                            if *key == k && modifiers.matches_logically(Modifiers::NONE) {
                                n += 1;
                                sh |= modifiers.shift;
                            }
                        }
                    }
                    if n > 0 {
                        i.consume_key(Modifiers::NONE, k);
                    }
                    (n, sh)
                })
            };
            let cmd = Modifiers::COMMAND;
            if key(cmd | Modifiers::SHIFT, Key::Z) || key(cmd, Key::Y) {
                acts.push(Act::Redo);
            } else if key(cmd, Key::Z) {
                acts.push(Act::Undo);
            } else if key(cmd, Key::A) {
                tab.select_all();
            } else if key(cmd, Key::ArrowUp) {
                tab.jump_cur(Some(false), None, shift);
            } else if key(cmd, Key::ArrowDown) {
                tab.jump_cur(Some(true), None, shift);
            } else if key(cmd, Key::ArrowLeft) {
                tab.jump_cur(None, Some(false), shift);
            } else if key(cmd, Key::ArrowRight) {
                tab.jump_cur(None, Some(true), shift);
            } else if key(cmd, Key::Home) {
                tab.jump_cur(Some(false), Some(false), shift);
            } else if key(cmd, Key::End) {
                tab.jump_cur(Some(true), Some(true), shift);
            } else if let (n @ 1.., shift) = nav(Key::ArrowUp) {
                tab.move_cur(-n, 0, shift);
            } else if let (n @ 1.., shift) = nav(Key::ArrowDown) {
                tab.move_cur(n, 0, shift);
            } else if let (n @ 1.., shift) = nav(Key::ArrowLeft) {
                tab.move_cur(0, -n, shift);
            } else if let (n @ 1.., shift) = nav(Key::ArrowRight) {
                tab.move_cur(0, n, shift);
            } else if key(Modifiers::NONE, Key::Home) {
                tab.jump_cur(None, Some(false), shift);
            } else if key(Modifiers::NONE, Key::End) {
                tab.jump_cur(None, Some(true), shift);
            } else if let (n @ 1.., shift) = nav(Key::PageUp) {
                tab.move_cur(-page * n, 0, shift);
            } else if let (n @ 1.., shift) = nav(Key::PageDown) {
                tab.move_cur(page * n, 0, shift);
            } else if let (n @ 1.., shift) = nav(Key::Enter) {
                tab.move_cur(if shift { -n } else { n }, 0, false);
            } else if let (n @ 1.., shift) = nav(Key::Tab) {
                tab.move_cur(0, if shift { -n } else { n }, false);
            } else if key(Modifiers::NONE, Key::F2) {
                start_edit = Some(None);
            } else if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
                acts.push(Act::Clear);
            } else if key(Modifiers::NONE, Key::Escape) {
                let c = tab.cur;
                tab.select_one(c.0, c.1);
            }
        }
        if want_focus && !find_open {
            resp.request_focus();
        }
    });

    // Ändringar efter ritningen.
    if let Some(id) = grid_id {
        if refocus || (ctx.memory(|m| m.focused()).is_none() && !find_open) {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }
    if let Some((dr, dc)) = move_after_commit {
        if tab.commit_edit(doc) {
            tab.stats_key = 0;
        }
        if dr != 0 || dc != 0 {
            tab.move_cur(dr, dc, false);
        }
    }
    for act in acts {
        tab.run(doc, act, &mut clip_out);
    }
    if let Some(text) = clip_out {
        ctx.copy_text(text);
    }
    if let Some(initial) = start_edit {
        if tab.edit.is_none() {
            tab.begin_edit(&doc.text, initial);
        }
    }
}

fn draw_value(painter: &egui::Painter, rect: Rect, v: &str, font: &FontId, color: Color32, ui: &Ui) {
    let clipped: String = v.chars().take(160).map(|c| if c == '\n' || c == '\r' { '¶' } else { c }).collect();
    let right = stats::parse_number(&clipped).is_some() && font.family != theme::semibold();
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(clipped, font.clone(), color));
    let pad = 6.0;
    let x = if right { (rect.right() - pad - galley.size().x).max(rect.left() + pad) } else { rect.left() + pad };
    let pos = pos2(x, rect.center().y - galley.size().y / 2.0);
    painter.with_clip_rect(rect.shrink2(vec2(1.0, 0.0)).intersect(painter.clip_rect())).galley(pos, galley, color);
}
