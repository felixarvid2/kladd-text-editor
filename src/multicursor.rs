//! Flera markörer och kolumnmarkering i textläget.
//!
//! Egui:s `TextEdit` äger den primära markören. Så länge det finns extra markörer fångar
//! [`State::pre`] tangent- och texthändelserna före `TextEdit`, tillämpar redigeringen på alla
//! markörer och skriver tillbaka den primära markören till `TextEdit`-tillståndet. Själva
//! redigeringslogiken är rena funktioner på (text, [`Cursors`]) längre ned och testas utan gränssnitt.
//! Alla positioner är teckenindex (som egui:s `CCursor`), inte byteindex.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use egui::text::CCursor;
use egui::text_edit::{TextEditOutput, TextEditState};
use egui::text_selection::CCursorRange;
use egui::{Context, Event, Galley, Id, Key, Modifiers, Painter, Pos2, Rect, Ui, Vec2, Visuals, pos2};

// ------------------------------------------------------------------ modell

/// En markering: `head` är där markören står, `anchor` där markeringen började.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sel {
    pub anchor: usize,
    pub head: usize,
}

impl Sel {
    pub fn caret(at: usize) -> Self {
        Sel { anchor: at, head: at }
    }
    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }
    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// Alla markörer, sorterade och utan överlapp. `primary` pekar ut den som `TextEdit` äger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursors {
    pub sels: Vec<Sel>,
    pub primary: usize,
}

impl Cursors {
    pub fn single(sel: Sel) -> Self {
        Cursors { sels: vec![sel], primary: 0 }
    }
    pub fn len(&self) -> usize {
        self.sels.len()
    }
    pub fn primary_sel(&self) -> Sel {
        self.sels[self.primary.min(self.sels.len() - 1)]
    }
    /// Behåller bara den primära markören (Esc).
    pub fn collapse(&mut self) {
        let p = self.primary_sel();
        self.sels = vec![p];
        self.primary = 0;
    }
    /// Sorterar och slår ihop markeringar som överlappar (eller en tom markör som rör en annan).
    pub fn normalize(&mut self) {
        let mut v: Vec<(Sel, bool)> =
            self.sels.iter().enumerate().map(|(i, s)| (*s, i == self.primary)).collect();
        v.sort_by_key(|(s, _)| (s.start(), s.end()));
        let mut out: Vec<(Sel, bool)> = Vec::with_capacity(v.len());
        for (s, p) in v {
            if let Some((last, lp)) = out.last_mut() {
                let touch = s.start() < last.end()
                    || (s.start() == last.end() && (s.is_empty() || last.is_empty()));
                if touch {
                    let (a, b) = (last.start().min(s.start()), last.end().max(s.end()));
                    // Riktningen följer den primära, annars den första.
                    let fwd = if p { s.head >= s.anchor } else { last.head >= last.anchor };
                    *last = if fwd { Sel { anchor: a, head: b } } else { Sel { anchor: b, head: a } };
                    *lp |= p;
                    continue;
                }
            }
            out.push((s, p));
        }
        self.primary = out.iter().position(|(_, p)| *p).unwrap_or(out.len() - 1);
        self.sels = out.into_iter().map(|(s, _)| s).collect();
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn chars_of(text: &str) -> Vec<char> {
    text.chars().collect()
}

/// Start (inklusive) av raden som innehåller `pos`.
fn line_start(chars: &[char], pos: usize) -> usize {
    chars[..pos.min(chars.len())].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1)
}

/// Slut (före radbrytningen) av raden som innehåller `pos`.
fn line_end(chars: &[char], pos: usize) -> usize {
    let p = pos.min(chars.len());
    chars[p..].iter().position(|&c| c == '\n').map_or(chars.len(), |i| p + i)
}

/// Ordet som rör `pos` (båda sidor räknas), som (start, slut).
fn word_range(chars: &[char], pos: usize) -> Option<(usize, usize)> {
    let pos = pos.min(chars.len());
    let on = |i: usize| chars.get(i).copied().is_some_and(is_word);
    if !on(pos) && !(pos > 0 && on(pos - 1)) {
        return None;
    }
    let mut s = pos;
    while s > 0 && on(s - 1) {
        s -= 1;
    }
    let mut e = pos;
    while on(e) {
        e += 1;
    }
    Some((s, e))
}

/// Ordgräns åt vänster: hoppar över skiljetecken och blanksteg, sedan ordet. Står markören
/// först på en rad går den till slutet av raden ovanför.
fn word_left(chars: &[char], pos: usize) -> usize {
    let mut p = pos.min(chars.len());
    if p > 0 && chars[p - 1] == '\n' {
        return p - 1;
    }
    while p > 0 && !is_word(chars[p - 1]) && chars[p - 1] != '\n' {
        p -= 1;
    }
    while p > 0 && is_word(chars[p - 1]) {
        p -= 1;
    }
    p
}

fn word_right(chars: &[char], pos: usize) -> usize {
    let n = chars.len();
    let mut p = pos.min(n);
    if p < n && chars[p] == '\n' {
        return p + 1;
    }
    while p < n && !is_word(chars[p]) && chars[p] != '\n' {
        p += 1;
    }
    while p < n && is_word(chars[p]) {
        p += 1;
    }
    p
}

// ---------------------------------------------------------------- redigering

/// En ersättning av tecknen `start..end` med `insert`.
struct Edit {
    start: usize,
    end: usize,
    insert: String,
}

/// Tillämpar en redigering per markering, i ordning så att förskjutningarna blir rätt, och
/// lämnar tomma markörer efter varje infogad text. `f` får markeringens index och värde.
fn apply_edits(text: &mut String, cur: &mut Cursors, f: impl Fn(usize, Sel) -> Edit) {
    let offs: Vec<usize> = text.char_indices().map(|(b, _)| b).chain([text.len()]).collect();
    let mut out = String::with_capacity(text.len() + 16);
    let (mut prev_end, mut delta) = (0usize, 0isize);
    let mut carets = Vec::with_capacity(cur.sels.len());
    let mut primary = cur.primary;
    for (i, s) in cur.sels.iter().enumerate() {
        let mut e = f(i, *s);
        e.start = e.start.max(prev_end);
        e.end = e.end.max(e.start);
        out.push_str(&text[offs[prev_end]..offs[e.start]]);
        out.push_str(&e.insert);
        let n = e.insert.chars().count();
        carets.push(((e.start as isize + delta) as usize) + n);
        delta += n as isize - (e.end - e.start) as isize;
        prev_end = e.end;
        if i == cur.primary {
            primary = carets.len() - 1;
        }
    }
    out.push_str(&text[offs[prev_end]..]);
    *text = out;
    cur.sels = carets.into_iter().map(Sel::caret).collect();
    cur.primary = primary;
    cur.normalize();
}

/// Skriver `s` över varje markering.
pub fn insert(text: &mut String, cur: &mut Cursors, s: &str) {
    apply_edits(text, cur, |_, sel| Edit { start: sel.start(), end: sel.end(), insert: s.to_owned() });
}

/// Backsteg: tar bort markeringen, annars tecknet (eller ordet) före markören.
pub fn backspace(text: &mut String, cur: &mut Cursors, word: bool) {
    let chars = chars_of(text);
    apply_edits(text, cur, |_, sel| {
        let start = if !sel.is_empty() {
            sel.start()
        } else if word {
            word_left(&chars, sel.head)
        } else {
            sel.head.saturating_sub(1)
        };
        Edit { start, end: sel.end(), insert: String::new() }
    });
}

/// Delete: tar bort markeringen, annars tecknet (eller ordet) efter markören.
pub fn delete(text: &mut String, cur: &mut Cursors, word: bool) {
    let chars = chars_of(text);
    apply_edits(text, cur, |_, sel| {
        let end = if !sel.is_empty() {
            sel.end()
        } else if word {
            word_right(&chars, sel.head)
        } else {
            (sel.head + 1).min(chars.len())
        };
        Edit { start: sel.start(), end, insert: String::new() }
    });
}

pub fn enter(text: &mut String, cur: &mut Cursors) {
    insert(text, cur, "\n");
}

pub fn tab(text: &mut String, cur: &mut Cursors) {
    insert(text, cur, "\t");
}

/// Markerad text, en markering per rad (tomma markeringar hoppas över). `None` om inget är markerat.
pub fn copy_text(text: &str, cur: &Cursors) -> Option<String> {
    let chars = chars_of(text);
    let parts: Vec<String> = cur
        .sels
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| chars[s.start()..s.end()].iter().collect())
        .collect();
    if parts.is_empty() { None } else { Some(parts.join("\n")) }
}

/// Klipper ut: ger den kopierade texten och tar bort markeringarna.
pub fn cut(text: &mut String, cur: &mut Cursors) -> Option<String> {
    let copied = copy_text(text, cur)?;
    delete(text, cur, false);
    Some(copied)
}

/// Klistrar in. Har urklippet lika många rader som det finns markörer får varje markör sin rad,
/// annars får alla hela texten.
pub fn paste(text: &mut String, cur: &mut Cursors, clip: &str) {
    let clip = clip.replace("\r\n", "\n");
    let lines: Vec<&str> = clip.split('\n').collect();
    if lines.len() == cur.len() && lines.len() > 1 {
        apply_edits(text, cur, |i, sel| Edit { start: sel.start(), end: sel.end(), insert: lines[i].to_owned() });
    } else {
        insert(text, cur, &clip);
    }
}

// ----------------------------------------------------------------- förflyttning

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
}

/// Flyttar alla markörer; med `extend` växer markeringen, annars kollapsar den.
pub fn move_cursors(text: &str, cur: &mut Cursors, dir: Dir, extend: bool) {
    let chars = chars_of(text);
    for s in &mut cur.sels {
        let head = match dir {
            Dir::Left if !extend && !s.is_empty() => s.start(),
            Dir::Right if !extend && !s.is_empty() => s.end(),
            Dir::Left => s.head.saturating_sub(1),
            Dir::Right => (s.head + 1).min(chars.len()),
            Dir::WordLeft => word_left(&chars, s.head),
            Dir::WordRight => word_right(&chars, s.head),
            Dir::Home => line_start(&chars, s.head),
            Dir::End => line_end(&chars, s.head),
            Dir::Up => {
                let ls = line_start(&chars, s.head);
                if ls == 0 {
                    0
                } else {
                    let col = s.head - ls;
                    let prev_start = line_start(&chars, ls - 1);
                    prev_start + col.min(ls - 1 - prev_start)
                }
            }
            Dir::Down => {
                let le = line_end(&chars, s.head);
                if le >= chars.len() {
                    chars.len()
                } else {
                    let col = s.head - line_start(&chars, s.head);
                    let next_start = le + 1;
                    next_start + col.min(line_end(&chars, next_start) - next_start)
                }
            }
        };
        s.head = head;
        if !extend {
            s.anchor = head;
        }
    }
    cur.normalize();
}

// -------------------------------------------------------------------- val

/// Ctrl+D. Utan markering: markerar ordet vid markören. Med markering: lägger till nästa
/// förekomst (efter den sista markeringen, runt dokumentet) som ny primär markering.
/// Ger den nya markeringens start, eller `None` när det inte finns något att lägga till.
pub fn add_next_occurrence(text: &str, cur: &mut Cursors) -> Option<usize> {
    let chars = chars_of(text);
    let p = cur.primary_sel();
    if p.is_empty() {
        let (s, e) = word_range(&chars, p.head)?;
        cur.sels[cur.primary] = Sel { anchor: s, head: e };
        return Some(s);
    }
    let needle = &chars[p.start()..p.end()];
    let from = cur.sels.iter().map(Sel::end).max().unwrap_or(0);
    let found = find_from(&chars, needle, from)
        .or_else(|| find_from(&chars, needle, 0))
        .filter(|&i| !cur.sels.iter().any(|s| i < s.end() && s.start() < i + needle.len()))?;
    cur.sels.push(Sel { anchor: found, head: found + needle.len() });
    cur.primary = cur.sels.len() - 1;
    cur.normalize();
    Some(found)
}

/// Ctrl+Skift+L: markerar varje förekomst av markeringen (eller ordet vid markören).
pub fn select_all_occurrences(text: &str, cur: &mut Cursors) -> bool {
    let chars = chars_of(text);
    let p = cur.primary_sel();
    let (s, e) = if p.is_empty() {
        match word_range(&chars, p.head) {
            Some(r) => r,
            None => return false,
        }
    } else {
        (p.start(), p.end())
    };
    let needle = &chars[s..e];
    let mut sels = Vec::new();
    let mut at = 0;
    while let Some(i) = find_from(&chars, needle, at) {
        sels.push(Sel { anchor: i, head: i + needle.len() });
        at = i + needle.len();
    }
    cur.primary = sels.iter().position(|m| m.start() == s).unwrap_or(0);
    cur.sels = sels;
    true
}

fn find_from(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() || from > hay.len() - needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// Alt+klick: lägger till en markör eller markering. Den nya blir primär.
pub fn add_cursor(cur: &mut Cursors, at: Sel) {
    cur.sels.push(at);
    cur.primary = cur.sels.len() - 1;
    cur.normalize();
}

/// Kolumnmarkering mellan (rad, kolumn) för ankaret och för draget (0-baserade, kolumn i tecken).
/// En markering per rad, avkortad till radens längd. Primär är raden dit draget slutar.
pub fn column_select(text: &str, anchor: (usize, usize), head: (usize, usize)) -> Cursors {
    let chars = chars_of(text);
    let mut starts = vec![0usize];
    starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
    let last = starts.len() - 1;
    let (l0, l1) = (anchor.0.min(last), head.0.min(last));
    let mut sels = Vec::new();
    for line in l0.min(l1)..=l0.max(l1) {
        let ls = starts[line];
        let len = line_end(&chars, ls) - ls;
        sels.push(Sel { anchor: ls + anchor.1.min(len), head: ls + head.1.min(len) });
    }
    let primary = if l1 >= l0 { sels.len() - 1 } else { 0 };
    Cursors { sels, primary }
}

/// (rad, kolumn), båda 0-baserade, för teckenindexet `ci`.
pub fn line_col0(text: &str, ci: usize) -> (usize, usize) {
    let (mut line, mut col) = (0, 0);
    for c in text.chars().take(ci) {
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

// ---------------------------------------------------------------- gränssnitt

/// En redigering som kommer från tangentbordet, översatt från egui-händelser.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Insert(String),
    Backspace { word: bool },
    Delete { word: bool },
    Enter,
    Tab,
    Move { dir: Dir, extend: bool },
    Copy,
    Cut,
    Paste(String),
    Escape,
}

/// Översätter en egui-händelse. `None` = den ska vidare till `TextEdit`.
/// `Some(None)` = händelsen ska bort men gör inget (t.ex. Skift+Tab).
fn translate(ev: &Event) -> Option<Option<Action>> {
    match ev {
        Event::Text(s) => {
            let clean: String = s.chars().filter(|c| !c.is_control()).collect();
            Some(if clean.is_empty() { None } else { Some(Action::Insert(clean)) })
        }
        Event::Copy => Some(Some(Action::Copy)),
        Event::Cut => Some(Some(Action::Cut)),
        Event::Paste(s) => Some(Some(Action::Paste(s.clone()))),
        Event::Key { key, pressed: true, modifiers: m, .. } => {
            let cmd = m.command;
            let plain = !m.alt && !cmd;
            let mv = |dir| Some(Some(Action::Move { dir, extend: m.shift }));
            match key {
                Key::Escape if !m.any() => Some(Some(Action::Escape)),
                Key::Backspace if !m.alt => Some(Some(Action::Backspace { word: cmd })),
                Key::Delete if !m.alt && !m.shift => Some(Some(Action::Delete { word: cmd })),
                Key::Enter if plain => Some(Some(Action::Enter)),
                Key::Tab if plain && !m.shift => Some(Some(Action::Tab)),
                Key::Tab if plain => Some(None),
                Key::ArrowLeft if !m.alt => mv(if cmd { Dir::WordLeft } else { Dir::Left }),
                Key::ArrowRight if !m.alt => mv(if cmd { Dir::WordRight } else { Dir::Right }),
                Key::ArrowUp if plain => mv(Dir::Up),
                Key::ArrowDown if plain => mv(Dir::Down),
                Key::Home if plain => mv(Dir::Home),
                Key::End if plain => mv(Dir::End),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Tangenter som flyttar markören på ett sätt vi inte stöder med flera markörer (Ctrl+A,
/// Ctrl+Home/End, sidbyte): då återgår vi till en markör och låter `TextEdit` sköta det.
fn drops_extras(ev: &Event) -> bool {
    match ev {
        Event::Key { key, pressed: true, modifiers, .. } => match key {
            Key::A | Key::Home | Key::End => modifiers.command,
            Key::PageUp | Key::PageDown => true,
            _ => false,
        },
        _ => false,
    }
}

/// Tillämpar en [`Action`]; ger text att lägga i urklipp.
pub fn apply_action(text: &mut String, cur: &mut Cursors, a: &Action) -> Option<String> {
    match a {
        Action::Insert(s) => insert(text, cur, s),
        Action::Backspace { word } => backspace(text, cur, *word),
        Action::Delete { word } => delete(text, cur, *word),
        Action::Enter => enter(text, cur),
        Action::Tab => tab(text, cur),
        Action::Move { dir, extend } => move_cursors(text, cur, *dir, *extend),
        Action::Copy => return copy_text(text, cur),
        Action::Cut => return cut(text, cur),
        Action::Paste(s) => paste(text, cur, s),
        Action::Escape => cur.collapse(),
    }
    None
}

fn hash_of(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Resultat av ett steg: om texten ändrades och vart vyn ska rulla.
#[derive(Default)]
pub struct Outcome {
    pub edited: bool,
    pub scroll_to: Option<usize>,
}

/// Flermarkörstillstånd för editorn (ett dokument åt gången).
#[derive(Default)]
pub struct State {
    doc_id: u64,
    /// `Some` bara när det finns minst två markörer.
    cur: Option<Cursors>,
    /// Textens hash efter förra bilden: avviker den har något annat (ångra, ersätt) ändrat texten.
    text_hash: u64,
    /// Markörerna före ett Alt+klick; den nya primära markören läggs till dem efter `show`.
    alt_click: Option<Cursors>,
    /// Ankare (rad, kolumn) för pågående kolumndrag.
    col_anchor: Option<(usize, usize)>,
}

fn primary_range(state: &TextEditState) -> Sel {
    state
        .cursor
        .char_range()
        .map_or(Sel::caret(0), |r| Sel { anchor: r.secondary.index.0, head: r.primary.index.0 })
}

fn set_primary(state: &mut TextEditState, s: Sel) {
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(s.anchor), CCursor::new(s.head))));
}

impl State {
    /// Finns det minst två markörer?
    pub fn is_multi(&self) -> bool {
        self.cur.is_some()
    }

    /// Kör före `TextEdit::show`. `focused`: editorn har tangentbordsfokus. `rect`: editorns yta.
    pub fn pre(&mut self, ctx: &Context, id: Id, doc_id: u64, text: &mut String, focused: bool, rect: Rect) -> Outcome {
        let mut out = Outcome::default();
        if self.doc_id != doc_id {
            *self = State { doc_id, ..State::default() };
        }
        let hash_before = self.text_hash;
        if let Some(cur) = &self.cur {
            let max = text.chars().count();
            if hash_of(text) != self.text_hash || cur.sels.iter().any(|s| s.end() > max) {
                self.cur = None;
            }
        }
        let (alt, shift, press_in) = ctx.input(|i| {
            let m = i.modifiers;
            let inside = i.pointer.interact_pos().is_some_and(|p| rect.contains(p));
            (m.alt, m.shift, i.pointer.primary_pressed() && inside)
        });
        // Vanligt klick tillbaka till en markör; Alt+klick och kolumnmarkering hanteras efter `show`.
        if press_in {
            if alt && !shift {
                self.alt_click = Some(match &self.cur {
                    Some(c) => c.clone(),
                    None => Cursors::single(primary_range(&TextEditState::load(ctx, id).unwrap_or_default())),
                });
            } else if !(alt && shift) {
                self.cur = None;
            }
        }
        if !focused {
            return out;
        }

        // Ctrl+D och Ctrl+Skift+L (Skift-varianten först: `consume_key` ignorerar extra Skift).
        let (sel_all, add_next) = ctx.input_mut(|i| {
            let a = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::L);
            let b = !a && i.consume_key(Modifiers::COMMAND, Key::D);
            (a, b)
        });
        if sel_all || add_next {
            let mut state = TextEditState::load(ctx, id).unwrap_or_default();
            let mut cur = self.cur.take().unwrap_or_else(|| Cursors::single(primary_range(&state)));
            let scroll = if sel_all {
                select_all_occurrences(text, &mut cur).then(|| cur.primary_sel().start())
            } else {
                add_next_occurrence(text, &mut cur)
            };
            set_primary(&mut state, cur.primary_sel());
            state.store(ctx, id);
            out.scroll_to = scroll;
            self.text_hash = hash_of(text);
            self.cur = (cur.len() > 1).then_some(cur);
            ctx.request_repaint();
        }

        let Some(mut cur) = self.cur.take() else {
            return out;
        };
        // Händelser vi tar hand om plockas bort så att `TextEdit` inte ser dem.
        let mut actions = Vec::new();
        ctx.input_mut(|i| {
            let mut keep = Vec::with_capacity(i.events.len());
            for ev in std::mem::take(&mut i.events) {
                if drops_extras(&ev) {
                    cur.collapse();
                    keep.push(ev);
                    continue;
                }
                match translate(&ev) {
                    Some(a) => actions.extend(a),
                    None => keep.push(ev),
                }
            }
            i.events = keep;
        });
        for a in &actions {
            if let Some(clip) = apply_action(text, &mut cur, a) {
                ctx.copy_text(clip);
            }
        }
        if !actions.is_empty() {
            let mut state = TextEditState::load(ctx, id).unwrap_or_default();
            set_primary(&mut state, cur.primary_sel());
            state.store(ctx, id);
            out.scroll_to = Some(cur.primary_sel().head);
            out.edited = hash_of(text) != hash_before;
            self.text_hash = hash_of(text);
            ctx.request_repaint();
        }
        self.cur = (cur.len() > 1).then_some(cur);
        out
    }

    /// Kör efter `TextEdit::show`: Alt+klick, kolumndrag och ritning av de extra markörerna.
    pub fn post(&mut self, ui: &Ui, id: Id, text: &str, out: &TextEditOutput, font_size: f32) {
        let ctx = ui.ctx();
        let (pos, pressed_p, pressed_m, down_p, down_m, m) = ctx.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_pressed(egui::PointerButton::Middle),
                i.pointer.button_down(egui::PointerButton::Primary),
                i.pointer.button_down(egui::PointerButton::Middle),
                i.modifiers,
            )
        });
        let galley_pos = out.galley_pos;
        let hovered = pos.is_some_and(|p| out.response.rect.contains(p));

        // Ångra/gör om i `TextEdit` ändrar texten utan att känna till de extra markörerna.
        if self.cur.is_some() && hash_of(text) != self.text_hash {
            self.cur = None;
        }
        if let Some(mut c) = self.alt_click.take() {
            add_cursor(&mut c, primary_range(&out.state));
            self.cur = (c.len() > 1).then_some(c);
        }

        // Kolumnmarkering: Alt+Skift+drag eller mittenknappen.
        let font = egui::FontId::new(font_size, egui::FontFamily::Monospace);
        let char_w = ctx.fonts_mut(|f| f.glyph_width(&font, ' ')).max(1.0);
        let at = |p: Pos2| -> (usize, usize) {
            let idx = out.galley.cursor_from_pos((p - galley_pos).max(Vec2::ZERO)).index;
            let col = ((p.x - galley_pos.x) / char_w).round().max(0.0) as usize;
            (line_col0(text, idx.0).0, col)
        };
        match pos {
            Some(p) => {
                if hovered && ((pressed_p && m.alt && m.shift) || pressed_m) {
                    self.col_anchor = Some(at(p));
                    ctx.memory_mut(|mem| mem.request_focus(id));
                }
                if let Some(a) = self.col_anchor {
                    if (down_p && m.alt && m.shift) || down_m {
                        let c = column_select(text, a, at(p));
                        let mut state = out.state.clone();
                        set_primary(&mut state, c.primary_sel());
                        state.store(ctx, id);
                        self.cur = (c.len() > 1).then_some(c);
                        ctx.request_repaint();
                    } else {
                        self.col_anchor = None;
                    }
                }
            }
            None => self.col_anchor = None,
        }

        self.text_hash = if self.cur.is_some() { hash_of(text) } else { 0 };
        if let Some(cur) = &self.cur {
            paint_extras(ui.painter(), ui.visuals(), &out.galley, galley_pos, cur);
        }
    }
}

/// Ritar de extra markörerna och deras markeringar (den primära ritas av `TextEdit`).
fn paint_extras(painter: &Painter, visuals: &Visuals, galley: &Galley, origin: Pos2, cur: &Cursors) {
    let clip = painter.clip_rect();
    let shift = origin.to_vec2();
    let at = |ci: usize| galley.pos_from_cursor(CCursor::new(ci)).translate(shift);
    // Teckenindex för varje rads början.
    let mut row_start = Vec::with_capacity(galley.rows.len());
    let mut n = 0usize;
    for row in &galley.rows {
        row_start.push(n);
        n += row.char_count_including_newline().0;
    }
    for (i, s) in cur.sels.iter().enumerate() {
        if i == cur.primary {
            continue;
        }
        if !s.is_empty() {
            for (ri, row) in galley.rows.iter().enumerate() {
                let rs = row_start[ri];
                let text_end = rs + row.char_count_excluding_newline().0;
                let (a, b) = (s.start().max(rs), s.end().min(rs + row.char_count_including_newline().0));
                if a >= b {
                    continue;
                }
                let r = row.rect().translate(shift);
                if r.bottom() < clip.top() || r.top() > clip.bottom() {
                    continue;
                }
                let x0 = at(a).min.x;
                let mut x1 = at(b.min(text_end)).min.x;
                if b > text_end {
                    x1 += 4.0;
                }
                let band = Rect::from_x_y_ranges(x0..=x1.max(x0 + 1.0), r.top()..=r.bottom());
                painter.rect_filled(band, 0.0, visuals.selection.bg_fill);
            }
        }
        let c = at(s.head);
        if c.bottom() >= clip.top() && c.top() <= clip.bottom() {
            let x = c.min.x;
            painter.line_segment([pos2(x, c.top()), pos2(x, c.bottom())], visuals.text_cursor.stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carets(v: &[usize]) -> Cursors {
        Cursors { sels: v.iter().map(|&i| Sel::caret(i)).collect(), primary: v.len() - 1 }
    }
    fn sel(a: usize, h: usize) -> Sel {
        Sel { anchor: a, head: h }
    }
    fn heads(c: &Cursors) -> Vec<usize> {
        c.sels.iter().map(|s| s.head).collect()
    }

    #[test]
    fn insert_at_three_carets() {
        let mut t = "a\nb\nc".to_owned();
        let mut c = carets(&[1, 3, 5]);
        insert(&mut t, &mut c, "X");
        assert_eq!(t, "aX\nbX\ncX");
        assert_eq!(heads(&c), vec![2, 5, 8]);
        assert_eq!(c.primary, 2);
    }

    #[test]
    fn insert_replaces_selections_and_handles_multibyte() {
        let mut t = "åäö åäö".to_owned();
        let mut c = Cursors { sels: vec![sel(0, 3), sel(4, 7)], primary: 1 };
        insert(&mut t, &mut c, "é");
        assert_eq!(t, "é é");
        assert_eq!(heads(&c), vec![1, 3]);
    }

    #[test]
    fn backspace_and_delete() {
        let mut t = "abc\ndef".to_owned();
        let mut c = carets(&[2, 6]);
        backspace(&mut t, &mut c, false);
        assert_eq!(t, "ac\ndf");
        assert_eq!(heads(&c), vec![1, 4]);
        delete(&mut t, &mut c, false);
        assert_eq!(t, "a\nd");
        // En markering tas bort som helhet.
        let mut t = "abcdef".to_owned();
        let mut c = Cursors { sels: vec![sel(0, 2), sel(4, 6)], primary: 0 };
        backspace(&mut t, &mut c, false);
        assert_eq!(t, "cd");
    }

    #[test]
    fn backspace_adjacent_carets() {
        let mut t = "ab".to_owned();
        let mut c = carets(&[0, 1, 2]);
        backspace(&mut t, &mut c, false);
        assert_eq!(t, "");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn word_delete() {
        let mut t = "foo bar\nbaz qux".to_owned();
        let mut c = carets(&[7, 15]);
        backspace(&mut t, &mut c, true);
        assert_eq!(t, "foo \nbaz ");
        let mut t = "foo bar\nbaz qux".to_owned();
        let mut c = carets(&[0, 8]);
        delete(&mut t, &mut c, true);
        assert_eq!(t, " bar\n qux");
    }

    #[test]
    fn enter_and_tab() {
        let mut t = "ab".to_owned();
        let mut c = carets(&[1, 2]);
        enter(&mut t, &mut c);
        assert_eq!(t, "a\nb\n");
        let mut t = "ab".to_owned();
        let mut c = carets(&[0, 1]);
        tab(&mut t, &mut c);
        assert_eq!(t, "\ta\tb");
    }

    #[test]
    fn horizontal_moves() {
        let t = "ab\ncd";
        let mut c = carets(&[1, 4]);
        move_cursors(t, &mut c, Dir::Left, false);
        assert_eq!(heads(&c), vec![0, 3]);
        move_cursors(t, &mut c, Dir::Right, true);
        assert_eq!(c.sels, vec![sel(0, 1), sel(3, 4)]);
        // Utan Skift kollapsar markeringen mot sin kant.
        move_cursors(t, &mut c, Dir::Left, false);
        assert_eq!(c.sels, vec![Sel::caret(0), Sel::caret(3)]);
        move_cursors(t, &mut c, Dir::End, false);
        assert_eq!(heads(&c), vec![2, 5]);
        move_cursors(t, &mut c, Dir::Home, false);
        assert_eq!(heads(&c), vec![0, 3]);
    }

    #[test]
    fn vertical_moves_clamp_and_merge() {
        let t = "abcd\nx\nabcd";
        let mut c = carets(&[3, 10]);
        move_cursors(t, &mut c, Dir::Up, false);
        assert_eq!(heads(&c), vec![0, 6]);
        let mut c = carets(&[3, 10]);
        move_cursors(t, &mut c, Dir::Down, false);
        assert_eq!(heads(&c), vec![6, 11]);
        // Två markörer som landar på samma plats blir en.
        let t = "abcd\nx";
        let mut c = carets(&[1, 3]);
        move_cursors(t, &mut c, Dir::Down, false);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn word_moves() {
        let t = "foo bar\nbaz";
        let mut c = carets(&[0, 8]);
        move_cursors(t, &mut c, Dir::WordRight, false);
        assert_eq!(heads(&c), vec![3, 11]);
        move_cursors(t, &mut c, Dir::WordLeft, false);
        assert_eq!(heads(&c), vec![0, 8]);
        let mut c = carets(&[7]);
        move_cursors(t, &mut c, Dir::WordRight, true);
        assert_eq!(c.sels, vec![sel(7, 8)]);
    }

    #[test]
    fn copy_cut_paste() {
        let mut t = "one two three".to_owned();
        let mut c = Cursors { sels: vec![sel(0, 3), sel(8, 13)], primary: 1 };
        assert_eq!(copy_text(&t, &c).as_deref(), Some("one\nthree"));
        assert_eq!(cut(&mut t, &mut c).as_deref(), Some("one\nthree"));
        assert_eq!(t, " two ");
        // Lika många rader som markörer: en rad var.
        paste(&mut t, &mut c, "A\nB");
        assert_eq!(t, "A two B");
        // Annars får alla hela texten.
        let mut c = carets(&[1, 7]);
        paste(&mut t, &mut c, "x\ny\nz");
        assert_eq!(t, "Ax\ny\nz two Bx\ny\nz");
        let c = carets(&[0, 1]);
        assert_eq!(copy_text("ab", &c), None);
    }

    #[test]
    fn paste_crlf_counts_lines() {
        let mut t = "..".to_owned();
        let mut c = carets(&[0, 1]);
        paste(&mut t, &mut c, "a\r\nb");
        assert_eq!(t, "a.b.");
    }

    #[test]
    fn next_occurrence_flow() {
        let t = "foo bar foo baz foo";
        let mut c = carets(&[1]);
        assert_eq!(add_next_occurrence(t, &mut c), Some(0));
        assert_eq!(c.sels, vec![sel(0, 3)]);
        assert_eq!(add_next_occurrence(t, &mut c), Some(8));
        assert_eq!(add_next_occurrence(t, &mut c), Some(16));
        assert_eq!(c.len(), 3);
        // Allt är redan valt.
        assert_eq!(add_next_occurrence(t, &mut c), None);
        assert_eq!(c.primary_sel(), sel(16, 19));
    }

    #[test]
    fn next_occurrence_wraps() {
        let t = "ab xx ab";
        let mut c = Cursors::single(sel(6, 8));
        assert_eq!(add_next_occurrence(t, &mut c), Some(0));
        assert_eq!(c.sels, vec![sel(0, 2), sel(6, 8)]);
        assert_eq!(c.primary, 0);
    }

    #[test]
    fn all_occurrences() {
        let t = "a_1 b a_1\na_1";
        let mut c = carets(&[1]);
        assert!(select_all_occurrences(t, &mut c));
        assert_eq!(c.sels, vec![sel(0, 3), sel(6, 9), sel(10, 13)]);
        assert_eq!(c.primary_sel(), sel(0, 3));
        let mut c = carets(&[1]);
        assert!(!select_all_occurrences(" . ", &mut c));
    }

    #[test]
    fn add_cursor_keeps_sorted_and_dedups() {
        let mut c = carets(&[5]);
        add_cursor(&mut c, Sel::caret(2));
        assert_eq!(heads(&c), vec![2, 5]);
        assert_eq!(c.primary_sel().head, 2);
        add_cursor(&mut c, Sel::caret(5));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn column_selection_clamps_to_line_length() {
        let t = "abcdef\nab\nabcdef";
        let c = column_select(t, (0, 2), (2, 5));
        assert_eq!(c.sels, vec![sel(2, 5), sel(9, 9), sel(12, 15)]);
        assert_eq!(c.primary, 2);
        // Uppåt: primär är översta raden och riktningen följer draget.
        let c = column_select(t, (2, 4), (0, 1));
        assert_eq!(c.sels[0], sel(4, 1));
        assert_eq!(c.primary, 0);
        // Noll bredd ger en markör per rad.
        let c = column_select(t, (0, 3), (2, 3));
        assert!(c.sels.iter().all(Sel::is_empty));
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn column_then_type() {
        let mut t = "aa\nbb\ncc".to_owned();
        let mut c = column_select(&t, (0, 1), (2, 1));
        insert(&mut t, &mut c, "-");
        assert_eq!(t, "a-a\nb-b\nc-c");
    }

    #[test]
    fn escape_collapses_to_primary() {
        let mut t = "a b".to_owned();
        let mut c = carets(&[0, 2]);
        apply_action(&mut t, &mut c, &Action::Escape);
        assert_eq!(c.sels, vec![Sel::caret(2)]);
    }

    #[test]
    fn translate_events() {
        let key = |k, m| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
        assert_eq!(translate(&Event::Text("x".into())), Some(Some(Action::Insert("x".into()))));
        assert_eq!(translate(&key(Key::Backspace, Modifiers::NONE)), Some(Some(Action::Backspace { word: false })));
        assert_eq!(
            translate(&key(Key::ArrowLeft, Modifiers::COMMAND | Modifiers::SHIFT)),
            Some(Some(Action::Move { dir: Dir::WordLeft, extend: true }))
        );
        assert_eq!(translate(&key(Key::Z, Modifiers::COMMAND)), None);
        assert_eq!(translate(&key(Key::Escape, Modifiers::NONE)), Some(Some(Action::Escape)));
        assert!(drops_extras(&key(Key::A, Modifiers::COMMAND)));
        assert!(!drops_extras(&key(Key::A, Modifiers::NONE)));
    }

    #[test]
    fn word_range_works() {
        let w = |t: &str, p| word_range(&chars_of(t), p);
        assert_eq!(w("foo bar", 1), Some((0, 3)));
        assert_eq!(w("foo bar", 3), Some((0, 3)));
        assert_eq!(w("foo  bar", 4), None);
    }
}
