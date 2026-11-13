//! Stora textfiler (loggar, 100 MB-filer, kodfiler med en miljon rader): en egen virtualiserad vy
//! i stället för egui:s `TextEdit`, som lägger ut hela texten varje bildruta.
//!
//! Vyn ritar bara raderna som syns, håller ett radindex (`LineIndex`) som uppdateras inkrementellt
//! vid varje ändring, har en egen rullningslist (egui:s `ScrollArea` tappar precision över ~16 miljoner
//! pixlar) och sin egen ångra-historik. Radbrytning och färgning är avstängda (låga trösklar i
//! `docs/dev/prestanda.md`). Vanliga filer under tröskeln använder `TextEdit` precis som förut.

use egui::{Align2, Color32, Event, EventFilter, FontFamily, FontId, Key, Modifiers, Rect, Sense, Ui, pos2, vec2};

use crate::theme::Palette;

/// Filer från så här många byte öppnas i den virtualiserade vyn.
pub const BIG_BYTES: usize = 5_000_000;
/// ...eller med så här många rader (och minst en megabyte text).
pub const BIG_LINES: usize = 200_000;
/// Filer större än så här läses in i en bakgrundstråd med förloppsvisning.
pub const BACKGROUND_LOAD_BYTES: u64 = 20_000_000;
/// Största fil som går att öppna (radindexet använder 32-bitars förskjutningar).
pub const MAX_BYTES: u64 = 2_000_000_000;
/// Minnestak för ångra-historiken.
const UNDO_BYTES: usize = 64_000_000;
/// Antal byte av en rad som läggs ut åt gången (resten ligger utanför fönstret).
const MIN_SLICE: usize = 160;

/// Ska den här texten visas i den virtualiserade vyn?
pub fn is_big(text: &str) -> bool {
    if text.len() >= BIG_BYTES {
        return true;
    }
    text.len() >= 1_000_000 && text.bytes().filter(|&b| b == b'\n').count() >= BIG_LINES
}

/// Byteförskjutning för varje rads början. `starts[0] == 0`; en text som slutar med radbrytning
/// har en avslutande tom rad (som `newlines + 1` i det vanliga läget).
#[derive(Clone, Default, Debug, PartialEq)]
pub struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    pub fn build(text: &str) -> Self {
        let mut starts = Vec::with_capacity(text.len() / 40 + 2);
        starts.push(0u32);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push((i + 1) as u32);
            }
        }
        Self { starts }
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn line_of(&self, byte: usize) -> usize {
        self.starts.partition_point(|&s| s as usize <= byte).saturating_sub(1)
    }

    pub fn start(&self, line: usize) -> usize {
        self.starts.get(line).map_or(0, |&s| s as usize)
    }

    /// Slutet av raden utan radbrytning.
    pub fn end(&self, text: &str, line: usize) -> usize {
        match self.starts.get(line + 1) {
            Some(&s) => s as usize - 1,
            None => text.len(),
        }
    }

    /// Slutet inklusive radbrytningen.
    pub fn full_end(&self, text: &str, line: usize) -> usize {
        self.starts.get(line + 1).map_or(text.len(), |&s| s as usize)
    }

    /// Byteintervallet `a..b` ersätts av en text som är `ins_len` byte lång och innehåller
    /// radbrytningar på de givna (absoluta, nya) förskjutningarna.
    fn splice(&mut self, a: usize, b: usize, ins: &str) {
        let (la, lb) = (self.line_of(a), self.line_of(b));
        let new_starts: Vec<u32> =
            ins.bytes().enumerate().filter(|&(_, c)| c == b'\n').map(|(i, _)| (a + i + 1) as u32).collect();
        let delta = ins.len() as i64 - (b - a) as i64;
        if delta != 0 {
            for s in &mut self.starts[lb + 1..] {
                *s = (*s as i64 + delta) as u32;
            }
        }
        self.starts.splice(la + 1..=lb, new_starts);
    }
}

fn words_in(s: &str) -> usize {
    s.split_whitespace().count()
}

/// En ändring i ångra-historiken.
#[derive(Clone, Debug, PartialEq)]
struct Edit {
    pos: usize,
    removed: String,
    inserted: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    PageUp,
    PageDown,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

/// Allt som hör till ett stort dokument i textläget (utom själva texten, som ligger i `Document`).
pub struct BigView {
    index: LineIndex,
    /// Fingeravtryck av texten som indexet byggdes för (längd + prov), för att upptäcka ändringar
    /// utanför vyn (sök och ersätt, återställning).
    fp: u64,
    /// Antal ord, hålls à jour vid varje ändring.
    words: usize,
    /// Räknas upp vid varje ändring av texten (cachenyckel för sökning m.m.).
    pub rev: u64,
    pub head: usize,
    pub anchor: usize,
    want_col: Option<usize>,
    /// Första synliga rad (bråkdel = delvis rullad).
    top: f64,
    /// Första synliga byte på varje rad (vågrät rullning).
    hcol: usize,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    undo_bytes: usize,
    /// Nästa tillägg får inte slås ihop med det förra.
    seal: bool,
    dragging: bool,
    /// Rulla så att den här byteförskjutningen syns.
    pub scroll_to: Option<usize>,
    /// Antal rader som får plats (från senaste ritningen), för PageUp/PageDown.
    page_rows: usize,
}

/// Billigt fingeravtryck: längd och några korta prov av texten.
fn fingerprint(text: &str) -> u64 {
    let b = text.as_bytes();
    let mut h = 0xcbf29ce484222325u64 ^ b.len() as u64;
    let n = b.len();
    for off in [0, n / 4, n / 2, n * 3 / 4, n.saturating_sub(64)] {
        for &c in &b[off.min(n)..(off + 64).min(n)] {
            h = (h ^ c as u64).wrapping_mul(0x100000001b3);
        }
    }
    h
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl BigView {
    pub fn new(text: &str) -> Self {
        Self {
            index: LineIndex::build(text),
            fp: fingerprint(text),
            words: words_in(text),
            rev: 0,
            head: 0,
            anchor: 0,
            want_col: None,
            top: 0.0,
            hcol: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_bytes: 0,
            seal: false,
            dragging: false,
            scroll_to: None,
            page_rows: 30,
        }
    }

    /// Bygger om indexet om texten ändrats utanför vyn.
    pub fn sync(&mut self, text: &str) {
        if fingerprint(text) != self.fp {
            self.index = LineIndex::build(text);
            self.words = words_in(text);
            self.fp = fingerprint(text);
            self.rev += 1;
            self.undo.clear();
            self.redo.clear();
            self.undo_bytes = 0;
            self.head = self.head.min(text.len());
            self.anchor = self.anchor.min(text.len());
            self.snap(text);
        }
    }

    /// Tvinga ombyggnad av indexet (efter en ändring av texten utanför vyn).
    pub fn invalidate(&mut self) {
        self.fp = !self.fp;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn lines(&self) -> usize {
        self.index.len()
    }

    pub fn words(&self) -> usize {
        self.words
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.head.min(self.anchor), self.head.max(self.anchor))
    }

    pub fn select(&mut self, a: usize, b: usize) {
        self.anchor = a;
        self.head = b;
        self.want_col = None;
        self.scroll_to = Some(b);
    }

    fn snap(&mut self, text: &str) {
        for p in [&mut self.head, &mut self.anchor] {
            *p = (*p).min(text.len());
            while !text.is_char_boundary(*p) {
                *p -= 1;
            }
        }
    }

    /// Rad (0-baserad) och kolumn (tecken, 0-baserad) för markören.
    pub fn line_col(&self, text: &str) -> (usize, usize) {
        let line = self.index.line_of(self.head);
        let s = self.index.start(line);
        let off = self.head.saturating_sub(s);
        // En rad på flera megabyte får inte räknas om varje bildruta.
        let col = if off > 100_000 { off } else { text.get(s..self.head).map_or(off, |t| t.chars().count()) };
        (line, col)
    }

    // ------------------------------------------------------------------ ändringar

    fn raw_splice(&mut self, text: &mut String, a: usize, b: usize, ins: &str) {
        let la = self.index.line_of(a);
        let lb = self.index.line_of(b);
        let (rs, re) = (self.index.start(la), self.index.full_end(text, lb));
        let before = words_in(&text[rs..re]);
        text.replace_range(a..b, ins);
        self.index.splice(a, b, ins);
        let re2 = (re as i64 + ins.len() as i64 - (b - a) as i64) as usize;
        self.words = (self.words + words_in(&text[rs..re2])).saturating_sub(before);
        self.fp = fingerprint(text);
        self.rev += 1;
    }

    /// Ersätter `a..b` med `ins` och lägger ändringen i ångra-historiken.
    pub fn edit(&mut self, text: &mut String, a: usize, b: usize, ins: &str) {
        if a > b || b > text.len() || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            return;
        }
        if text.len() + ins.len() - (b - a) > u32::MAX as usize - 1 || (a == b && ins.is_empty()) {
            return;
        }
        let removed = text[a..b].to_owned();
        self.raw_splice(text, a, b, ins);
        self.redo.clear();
        let merge = !self.seal
            && removed.is_empty()
            && !ins.contains('\n')
            && self.undo.last().is_some_and(|l| l.removed.is_empty() && l.pos + l.inserted.len() == a);
        if merge {
            if let Some(l) = self.undo.last_mut() {
                l.inserted.push_str(ins);
            }
        } else {
            self.undo.push(Edit { pos: a, removed: removed.clone(), inserted: ins.to_owned() });
        }
        self.seal = false;
        self.undo_bytes += removed.len() + ins.len();
        while self.undo_bytes > UNDO_BYTES && self.undo.len() > 1 {
            let e = self.undo.remove(0);
            self.undo_bytes = self.undo_bytes.saturating_sub(e.removed.len() + e.inserted.len());
        }
        self.head = a + ins.len();
        self.anchor = self.head;
        self.want_col = None;
        self.scroll_to = Some(self.head);
    }

    /// Ersätter markeringen (eller infogar vid markören).
    pub fn insert(&mut self, text: &mut String, s: &str) {
        let (a, b) = self.selection();
        self.edit(text, a, b, s);
    }

    pub fn undo(&mut self, text: &mut String) -> bool {
        let Some(e) = self.undo.pop() else { return false };
        let end = e.pos + e.inserted.len();
        self.raw_splice(text, e.pos, end, &e.removed);
        self.undo_bytes = self.undo_bytes.saturating_sub(e.removed.len() + e.inserted.len());
        self.head = e.pos + e.removed.len();
        self.anchor = if e.removed.is_empty() { self.head } else { e.pos };
        self.scroll_to = Some(self.head);
        self.redo.push(e);
        self.seal = true;
        true
    }

    pub fn redo(&mut self, text: &mut String) -> bool {
        let Some(e) = self.redo.pop() else { return false };
        self.raw_splice(text, e.pos, e.pos + e.removed.len(), &e.inserted);
        self.undo_bytes += e.removed.len() + e.inserted.len();
        self.head = e.pos + e.inserted.len();
        self.anchor = self.head;
        self.scroll_to = Some(self.head);
        self.undo.push(e);
        self.seal = true;
        true
    }

    /// Backsteg/Delete: tar bort markeringen, annars ett tecken (eller ett ord) åt `forward`-hållet.
    pub fn delete(&mut self, text: &mut String, forward: bool, word: bool) {
        let (a, b) = self.selection();
        if a != b {
            self.edit(text, a, b, "");
            return;
        }
        let (a, b) = if forward {
            (self.head, self.motion_target(text, if word { Motion::WordRight } else { Motion::Right }))
        } else {
            (self.motion_target(text, if word { Motion::WordLeft } else { Motion::Left }), self.head)
        };
        self.edit(text, a, b, "");
    }

    // ------------------------------------------------------------------ förflyttning

    fn prev_boundary(text: &str, mut i: usize) -> usize {
        if i == 0 {
            return 0;
        }
        i -= 1;
        while i > 0 && !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    fn next_boundary(text: &str, mut i: usize) -> usize {
        if i >= text.len() {
            return text.len();
        }
        i += 1;
        while i < text.len() && !text.is_char_boundary(i) {
            i += 1;
        }
        i
    }

    /// Byte på `col` tecken in på raden `line` (eller radens slut).
    fn byte_at_col(&self, text: &str, line: usize, col: usize) -> usize {
        let (s, e) = (self.index.start(line), self.index.end(text, line));
        match text.get(s..e) {
            Some(l) => l.char_indices().nth(col).map_or(e, |(b, _)| s + b),
            None => e,
        }
    }

    /// Var markören hamnar vid förflyttningen `m` (ändrar inte markören).
    pub fn motion_target(&self, text: &str, m: Motion) -> usize {
        let head = self.head.min(text.len());
        let line = self.index.line_of(head);
        match m {
            Motion::Left => Self::prev_boundary(text, head),
            Motion::Right => Self::next_boundary(text, head),
            Motion::WordLeft => {
                let mut i = head;
                // Hoppa över blanka tecken bakåt, sedan över ett ord (eller skiljetecken).
                while i > 0 {
                    let p = Self::prev_boundary(text, i);
                    if text[p..i].chars().next().is_some_and(char::is_whitespace) {
                        i = p;
                    } else {
                        break;
                    }
                }
                let kind = (i > 0).then(|| text[Self::prev_boundary(text, i)..i].chars().next().map(is_word)).flatten();
                while i > 0 {
                    let p = Self::prev_boundary(text, i);
                    let c = text[p..i].chars().next().unwrap_or(' ');
                    if c.is_whitespace() || Some(is_word(c)) != kind {
                        break;
                    }
                    i = p;
                }
                i
            }
            Motion::WordRight => {
                let mut i = head;
                let kind = text[i..].chars().next().map(is_word);
                while i < text.len() {
                    let n = Self::next_boundary(text, i);
                    let c = text[i..n].chars().next().unwrap_or(' ');
                    if c.is_whitespace() || Some(is_word(c)) != kind {
                        break;
                    }
                    i = n;
                }
                while i < text.len() {
                    let n = Self::next_boundary(text, i);
                    if text[i..n].chars().next().is_some_and(char::is_whitespace) {
                        i = n;
                    } else {
                        break;
                    }
                }
                i
            }
            Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => {
                let n = if matches!(m, Motion::PageUp | Motion::PageDown) { self.page_rows.max(1) } else { 1 };
                let target = if matches!(m, Motion::Up | Motion::PageUp) {
                    if line == 0 {
                        return 0;
                    }
                    line.saturating_sub(n)
                } else {
                    if line + 1 >= self.index.len() {
                        return text.len();
                    }
                    (line + n).min(self.index.len() - 1)
                };
                let col = self.want_col.unwrap_or_else(|| self.line_col(text).1);
                self.byte_at_col(text, target, col)
            }
            Motion::LineStart => self.index.start(line),
            Motion::LineEnd => self.index.end(text, line),
            Motion::DocStart => 0,
            Motion::DocEnd => text.len(),
        }
    }

    /// Flyttar markören (`extend`: behåll ankaret, alltså markera).
    pub fn go(&mut self, text: &str, m: Motion, extend: bool) {
        let vertical = matches!(m, Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown);
        if vertical && self.want_col.is_none() {
            self.want_col = Some(self.line_col(text).1);
        }
        let (a, b) = self.selection();
        // Utan Skift kollapsar vänster/höger en markering mot rätt kant.
        let t = if !extend && a != b && matches!(m, Motion::Left | Motion::Right) {
            if m == Motion::Left { a } else { b }
        } else {
            self.motion_target(text, m)
        };
        self.head = t;
        if !extend {
            self.anchor = t;
        }
        if !vertical {
            self.want_col = None;
        }
        self.scroll_to = Some(t);
    }

    /// Byteintervall med raden som innehåller `byte` och raden efter (för ersättning av en träff).
    pub fn line_window(&self, text: &str, byte: usize) -> (usize, usize) {
        let l = self.index.line_of(byte.min(text.len()));
        (self.index.start(l), self.index.full_end(text, (l + 1).min(self.index.len() - 1)))
    }

    /// Ord vid byte `b` (för dubbelklick).
    pub fn word_at(&self, text: &str, b: usize) -> (usize, usize) {
        let (s, e) = (self.index.start(self.index.line_of(b)), self.index.end(text, self.index.line_of(b)));
        let Some(l) = text.get(s..e) else { return (b, b) };
        let rel = b - s;
        let kind = l[rel..].chars().next().or_else(|| l[..rel].chars().next_back()).map(is_word);
        let (mut a, mut z) = (rel, rel);
        while a > 0 {
            let p = Self::prev_boundary(l, a);
            let c = l[p..a].chars().next().unwrap_or(' ');
            if c.is_whitespace() || Some(is_word(c)) != kind {
                break;
            }
            a = p;
        }
        while z < l.len() {
            let n = Self::next_boundary(l, z);
            let c = l[z..n].chars().next().unwrap_or(' ');
            if c.is_whitespace() || Some(is_word(c)) != kind {
                break;
            }
            z = n;
        }
        (s + a, s + z)
    }

    // ------------------------------------------------------------------ gränssnitt

    /// Hanterar tangentbordet och musen och ritar de synliga raderna. Returnerar om texten ändrats.
    pub fn show(&mut self, ui: &mut Ui, text: &mut String, o: &Opts<'_>) -> bool {
        self.sync(text);
        let p = o.palette;
        let mut changed = false;
        let font = FontId::new(o.font_size, FontFamily::Monospace);
        let num_font = FontId::new((o.font_size * 0.82).round(), FontFamily::Monospace);
        let row_h = (o.font_size * 1.6).round();
        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font, '0')).max(1.0);
        let digits = self.index.len().to_string().len().max(2) as f32;
        let num_w = ui.ctx().fonts_mut(|f| f.glyph_width(&num_font, '0'));
        let gutter_w = if o.line_numbers { digits * num_w + 40.0 } else { 22.0 };

        let rect = ui.available_rect_before_wrap();
        let resp = ui.interact(rect, o.id, Sense::click_and_drag());
        ui.advance_cursor_after_rect(rect);
        if resp.clicked() || resp.drag_started() || o.focus {
            ui.memory_mut(|m| m.request_focus(o.id));
        }
        let focused = ui.memory(|m| m.has_focus(o.id));
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    o.id,
                    EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false },
                )
            });
        }
        let page_rows = ((rect.height() / row_h) as usize).max(2) - 1;
        self.page_rows = page_rows;
        let text_left = rect.left() + gutter_w;
        let n_lines = self.index.len();
        let max_top = (n_lines as f64 - 1.0).max(0.0);

        // Tangentbord.
        if focused {
            let events = ui.input(|i| i.events.clone());
            for ev in events {
                match ev {
                    Event::Text(s) => {
                        let s: String = s.chars().filter(|c| !c.is_control() || *c == '\t').collect();
                        if !s.is_empty() {
                            self.insert(text, &s);
                            changed = true;
                        }
                    }
                    Event::Paste(s) => {
                        if !s.is_empty() {
                            self.insert(text, &s.replace("\r\n", "\n"));
                            changed = true;
                        }
                    }
                    Event::Copy | Event::Cut => {
                        let (a, b) = self.selection();
                        if a != b {
                            ui.ctx().copy_text(text[a..b].to_owned());
                            if matches!(ev, Event::Cut) {
                                self.edit(text, a, b, "");
                                changed = true;
                            }
                        }
                    }
                    Event::Key { key, pressed: true, modifiers, .. } => {
                        changed |= self.key(text, key, modifiers);
                    }
                    _ => {}
                }
            }
        }

        // Rullning: hjul, rullningslist och "rulla till".
        if resp.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            if d != vec2(0.0, 0.0) {
                self.top = (self.top - d.y as f64 / row_h as f64).clamp(0.0, max_top);
                let dx = (-d.x / char_w) as i64;
                self.hcol = (self.hcol as i64 + dx).max(0) as usize;
            }
        }
        let bar = Rect::from_min_max(pos2(rect.right() - 14.0, rect.top()), rect.right_bottom());
        let bar_resp = ui.interact(bar, o.id.with("bar"), Sense::click_and_drag());
        let thumb_h = (rect.height() * (page_rows as f32 / n_lines.max(1) as f32)).clamp(28.0, rect.height());
        if bar_resp.dragged() || bar_resp.clicked() {
            if let Some(pt) = bar_resp.interact_pointer_pos() {
                let f = ((pt.y - rect.top() - thumb_h / 2.0) / (rect.height() - thumb_h).max(1.0)).clamp(0.0, 1.0);
                self.top = f as f64 * max_top;
            }
        }
        if let Some(b) = self.scroll_to.take() {
            let b = b.min(text.len());
            let line = self.index.line_of(b) as f64;
            let rows = page_rows as f64;
            if line < self.top || line > self.top + rows - 1.0 {
                self.top = (line - rows / 2.0).clamp(0.0, max_top);
            }
            let col = b.saturating_sub(self.index.start(self.index.line_of(b)));
            let cols = ((rect.width() - gutter_w - 40.0) / char_w) as usize;
            if col < self.hcol {
                self.hcol = col.saturating_sub(4);
            } else if col > self.hcol + cols.saturating_sub(2) {
                self.hcol = col + 4 - cols.saturating_sub(2);
            }
        }
        self.top = self.top.clamp(0.0, max_top);

        // Mus: placera markör, dra för att markera, dubbelklick väljer ett ord.
        let hit = |pt: egui::Pos2, this: &Self, ui: &Ui, text: &str| -> usize {
            let line = (this.top + ((pt.y - rect.top()) / row_h) as f64).floor().clamp(0.0, max_top) as usize;
            let (s, e) = (this.index.start(line), this.index.end(text, line));
            let (a, z) = this.slice(text, s, e, ((rect.width() / char_w) as usize) * 3 + MIN_SLICE);
            let seg = text.get(a..z).unwrap_or("");
            let g = ui.painter().layout_no_wrap(seg.to_owned(), font.clone(), p.text);
            let idx = g.cursor_from_pos(vec2(pt.x - text_left - 2.0, 0.0)).index.0;
            a + seg.char_indices().nth(idx).map_or(seg.len(), |(b, _)| b)
        };
        if resp.double_clicked() {
            if let Some(pt) = resp.interact_pointer_pos() {
                let b = hit(pt, self, ui, text);
                let (a, z) = self.word_at(text, b);
                self.anchor = a;
                self.head = z;
                self.dragging = false;
            }
        } else if resp.is_pointer_button_down_on() && !bar_resp.hovered() && !bar_resp.dragged() {
            if let Some(pt) = resp.interact_pointer_pos() {
                if pt.x < rect.right() - 14.0 {
                    let b = hit(pt, self, ui, text);
                    let shift = ui.input(|i| i.modifiers.shift);
                    if resp.drag_started() || resp.clicked() || !self.dragging {
                        self.head = b;
                        if !shift {
                            self.anchor = b;
                        }
                    } else {
                        self.head = b;
                    }
                    self.dragging = true;
                    self.want_col = None;
                    // Dra utanför kanten: rulla.
                    if pt.y < rect.top() {
                        self.top = (self.top - 0.4).max(0.0);
                    } else if pt.y > rect.bottom() {
                        self.top = (self.top + 0.4).min(max_top);
                    }
                    ui.ctx().request_repaint();
                }
            }
        } else {
            self.dragging = false;
        }

        // Ritning.
        let painter = ui.painter_at(rect);
        let (cur_line, _) = self.line_col(text);
        let first = self.top.floor() as usize;
        let off = -(self.top.fract() as f32) * row_h;
        let (sa, sb) = self.selection();
        let rows = (rect.height() / row_h).ceil() as usize + 2;
        let cols = (rect.width() / char_w) as usize;
        let slice_len = cols * 3 + MIN_SLICE;
        let clip_text = Rect::from_min_max(pos2(text_left, rect.top()), pos2(rect.right() - 14.0, rect.bottom()));
        let text_painter = ui.painter_at(clip_text);
        let mut caret: Option<Rect> = None;
        for r in 0..rows {
            let line = first + r;
            if line >= n_lines {
                break;
            }
            let y = rect.top() + off + r as f32 * row_h;
            let (s, e) = (self.index.start(line), self.index.end(text, line));
            if line == cur_line && sa == sb {
                let band = Rect::from_min_max(pos2(rect.left(), y), pos2(rect.right() - 14.0, y + row_h));
                painter.rect_filled(band, 0.0, p.current_line);
            }
            if o.line_numbers {
                let color = if line == cur_line { p.accent } else { p.faint };
                painter.text(
                    pos2(rect.left() + gutter_w - 18.0, y + row_h / 2.0),
                    Align2::RIGHT_CENTER,
                    (line + 1).to_string(),
                    num_font.clone(),
                    color,
                );
            }
            let (a, z) = self.slice(text, s, e, slice_len);
            let seg = text.get(a..z).unwrap_or("");
            let g = painter.layout_no_wrap(seg.to_owned(), font.clone(), p.text);
            let tx = text_left + 2.0;
            let char_x = |g: &egui::Galley, byte: usize| -> f32 {
                let ci = seg.get(..byte.saturating_sub(a).min(seg.len())).map_or(0, |t| t.chars().count());
                g.pos_from_cursor(egui::text::CCursor::new(ci)).left()
            };
            // Sökträffar och markering som bakgrund.
            let paint_bg = |from: usize, to: usize, color: Color32| {
                let (f, t) = (from.max(a), to.min(z));
                // Täcker markeringen radbrytningen? Då ritas ett litet extra streck.
                let nl = to > e && from <= e && z == e;
                if f < t || nl {
                    let f = f.min(z);
                    let x0 = tx + char_x(&g, f);
                    let x1 = tx + char_x(&g, t.max(f)) + if nl { char_w * 0.6 } else { 0.0 };
                    text_painter.rect_filled(Rect::from_min_max(pos2(x0, y), pos2(x1.max(x0 + 1.0), y + row_h)), 0.0, color);
                }
            };
            if !o.matches.is_empty() {
                let first_m = o.matches.partition_point(|m| m.1 <= s);
                for m in o.matches[first_m..].iter().take_while(|m| m.0 <= e) {
                    paint_bg(m.0, m.1, if m.2 { p.match_current_bg } else { p.match_bg });
                }
            }
            if sa != sb && sb > s && sa <= e {
                paint_bg(sa.max(s), sb.min(e + 1), p.selection);
            }
            text_painter.galley(pos2(tx, y + (row_h - g.size().y) / 2.0), g.clone(), p.text);
            if line == cur_line {
                let x = tx + char_x(&g, self.head);
                caret = Some(Rect::from_min_size(pos2(x, y + 2.0), vec2(2.0, row_h - 4.0)));
            }
        }
        if let (Some(c), true) = (caret, focused) {
            text_painter.rect_filled(c, 1.0, p.accent);
        }
        // Rullningslist.
        let track_h = rect.height() - thumb_h;
        let f = if max_top > 0.0 { (self.top / max_top) as f32 } else { 0.0 };
        let thumb = Rect::from_min_size(pos2(bar.left() + 3.0, rect.top() + track_h * f), vec2(8.0, thumb_h));
        let alpha = if bar_resp.hovered() || bar_resp.dragged() { 150 } else { 70 };
        painter.rect_filled(thumb, 4.0, Color32::from_rgba_unmultiplied(p.muted.r(), p.muted.g(), p.muted.b(), alpha));
        changed
    }

    /// Byteintervallet av raden som läggs ut: börjar vid `hcol` och är högst `len` byte.
    fn slice(&self, text: &str, s: usize, e: usize, len: usize) -> (usize, usize) {
        let mut a = (s + self.hcol).min(e);
        while a < e && !text.is_char_boundary(a) {
            a += 1;
        }
        let mut z = (a + len).min(e);
        while z < e && !text.is_char_boundary(z) {
            z += 1;
        }
        (a, z)
    }

    fn key(&mut self, text: &mut String, key: Key, m: Modifiers) -> bool {
        let sh = m.shift;
        let cmd = m.command;
        let word = m.alt || m.ctrl;
        match key {
            Key::ArrowLeft => self.go(text, if word { Motion::WordLeft } else { Motion::Left }, sh),
            Key::ArrowRight => self.go(text, if word { Motion::WordRight } else { Motion::Right }, sh),
            Key::ArrowUp => self.go(text, Motion::Up, sh),
            Key::ArrowDown => self.go(text, Motion::Down, sh),
            Key::PageUp => self.go(text, Motion::PageUp, sh),
            Key::PageDown => self.go(text, Motion::PageDown, sh),
            Key::Home => self.go(text, if cmd { Motion::DocStart } else { Motion::LineStart }, sh),
            Key::End => self.go(text, if cmd { Motion::DocEnd } else { Motion::LineEnd }, sh),
            Key::A if cmd => {
                self.anchor = 0;
                self.head = text.len();
            }
            Key::Z if cmd && !sh => return self.undo(text),
            Key::Z if cmd && sh => return self.redo(text),
            Key::Y if cmd => return self.redo(text),
            Key::Enter if !cmd => {
                self.seal = true;
                self.insert(text, "\n");
                return true;
            }
            Key::Tab if !cmd && !sh => {
                self.insert(text, "\t");
                return true;
            }
            Key::Backspace => {
                self.delete(text, false, word);
                return true;
            }
            Key::Delete if !sh => {
                self.delete(text, true, word);
                return true;
            }
            _ => {}
        }
        false
    }
}

/// Vad vyn behöver veta för att rita.
pub struct Opts<'a> {
    pub id: egui::Id,
    pub palette: &'a Palette,
    pub font_size: f32,
    pub line_numbers: bool,
    /// Sökträffar (start, slut, aktuell), sorterade på start.
    pub matches: &'a [(usize, usize, bool)],
    /// Ge editorn tangentbordsfokus.
    pub focus: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % n.max(1)
        }
    }

    fn sample() -> String {
        "första raden\nandra raden med åäö\n\ntredje raden\nsista".to_owned()
    }

    #[test]
    fn thresholds() {
        assert!(!is_big(&"x".repeat(BIG_BYTES - 1)));
        assert!(is_big(&"x".repeat(BIG_BYTES)));
        // Många korta rader räknas också, men bara från en megabyte.
        assert!(!is_big(&"a\n".repeat(BIG_LINES)));
        assert!(is_big(&"abcdefgh\n".repeat(BIG_LINES + 1)));
        assert!(!is_big(&sample()));
    }

    #[test]
    fn index_lines() {
        let t = sample();
        let ix = LineIndex::build(&t);
        assert_eq!(ix.len(), 5);
        assert_eq!(&t[ix.start(1)..ix.end(&t, 1)], "andra raden med åäö");
        assert_eq!(ix.start(2), ix.end(&t, 2));
        assert_eq!(&t[ix.start(4)..ix.end(&t, 4)], "sista");
        assert_eq!(ix.line_of(0), 0);
        assert_eq!(ix.line_of(t.len()), 4);
        // Avslutande radbrytning ger en tom sista rad.
        assert_eq!(LineIndex::build("a\n").len(), 2);
        assert_eq!(LineIndex::build("").len(), 1);
    }

    #[test]
    fn incremental_index_matches_rebuild() {
        let mut t = sample();
        let mut v = BigView::new(&t);
        let mut r = Lcg(7);
        let pieces = ["x", "\n", "ab\ncd\n", "å", "", "\n\n", "lång text utan radbrytning"];
        for _ in 0..400 {
            let boundaries: Vec<usize> = (0..=t.len()).filter(|&i| t.is_char_boundary(i)).collect();
            let a = boundaries[r.next(boundaries.len())];
            let span = boundaries.iter().filter(|&&b| b >= a).nth(r.next(4)).copied().unwrap_or(a);
            let ins = pieces[r.next(pieces.len())];
            v.edit(&mut t, a, span, ins);
            assert_eq!(v.index, LineIndex::build(&t), "index efter ändring");
            assert_eq!(v.words, words_in(&t), "ordantal efter ändring");
        }
        // Ångra allt ger ursprungstexten.
        while v.undo(&mut t) {
            assert_eq!(v.index, LineIndex::build(&t));
            assert_eq!(v.words, words_in(&t));
        }
        assert_eq!(t, sample());
        while v.redo(&mut t) {}
        assert_eq!(v.index, LineIndex::build(&t));
    }

    #[test]
    fn typing_is_coalesced_and_undone_in_one_step() {
        let mut t = String::new();
        let mut v = BigView::new(&t);
        for c in "hej världen".chars() {
            v.insert(&mut t, &c.to_string());
        }
        assert_eq!(t, "hej världen");
        assert_eq!(v.undo.len(), 1);
        v.undo(&mut t);
        assert_eq!(t, "");
        v.redo(&mut t);
        assert_eq!(t, "hej världen");
        assert_eq!(v.head, t.len());
    }

    #[test]
    fn delete_and_selection() {
        let mut t = "abc åäö def".to_owned();
        let mut v = BigView::new(&t);
        v.head = 4;
        v.anchor = 4;
        v.delete(&mut t, true, false);
        assert_eq!(t, "abc äö def");
        v.delete(&mut t, false, false);
        assert_eq!(t, "abcäö def");
        v.select(0, 3);
        v.insert(&mut t, "X");
        assert_eq!(t, "Xäö def");
        v.go(&t, Motion::DocEnd, false);
        v.delete(&mut t, false, true);
        assert_eq!(t, "Xäö ");
    }

    #[test]
    fn motions() {
        let t = sample();
        let mut v = BigView::new(&t);
        v.go(&t, Motion::Down, false);
        assert_eq!(v.line_col(&t), (1, 0));
        v.go(&t, Motion::LineEnd, false);
        assert_eq!(v.line_col(&t).1, "andra raden med åäö".chars().count());
        // Lodrät rörelse minns kolumnen över en kortare rad.
        v.go(&t, Motion::Down, false);
        assert_eq!(v.line_col(&t), (2, 0));
        v.go(&t, Motion::Down, false);
        assert_eq!(v.line_col(&t), (3, "tredje raden".chars().count()));
        v.go(&t, Motion::DocStart, true);
        assert_eq!(v.selection(), (0, v.anchor));
        v.go(&t, Motion::DocEnd, false);
        assert_eq!(v.head, t.len());
        v.go(&t, Motion::Down, false);
        assert_eq!(v.head, t.len());
        v.go(&t, Motion::WordLeft, false);
        assert_eq!(&t[v.head..], "sista");
        v.go(&t, Motion::Left, false);
        v.go(&t, Motion::Left, false);
        assert!(t.is_char_boundary(v.head));
        // Flerbytetecken hoppas över hela.
        let u = "åäö";
        let mut w = BigView::new(u);
        w.go(u, Motion::Right, false);
        assert_eq!(w.head, 2);
    }

    #[test]
    fn word_at_picks_word() {
        let t = "foo bar_baz  qux";
        let v = BigView::new(t);
        assert_eq!(v.word_at(t, 5), (4, 11));
        assert_eq!(v.word_at(t, 0), (0, 3));
    }

    #[test]
    fn sync_detects_external_change() {
        let mut t = sample();
        let mut v = BigView::new(&t);
        v.head = t.len();
        t = "kort".into();
        v.sync(&t);
        assert_eq!(v.lines(), 1);
        assert_eq!(v.words(), 1);
        assert!(v.head <= t.len());
        // Ersättning av samma längd upptäcks när anroparen ogiltigförklarar.
        let mut t2 = "a\nb".to_owned();
        let mut v2 = BigView::new(&t2);
        t2 = "ab\n".into();
        v2.invalidate();
        v2.sync(&t2);
        assert_eq!(v2.index, LineIndex::build(&t2));
    }

    #[test]
    fn huge_single_line_is_cheap_to_index() {
        let t = "x".repeat(3_000_000);
        let v = BigView::new(&t);
        assert_eq!(v.lines(), 1);
        let (a, z) = v.slice(&t, 0, t.len(), 500);
        assert_eq!((a, z), (0, 500));
    }
}
