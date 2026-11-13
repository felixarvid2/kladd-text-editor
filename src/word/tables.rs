//! Tabellredigering i dokumentläget: tabellen är ett rutnät också med markören i den, och
//! markören hålls i cellens innehåll. Den rena logiken (vad en tangent ska göra givet text och
//! markör) ligger överst och är enhetstestad; längre ner kopplas den till egui och KladdApp.
//!
//! Pipor och cellutfyllnad är dolda (nollbreda) tecken i texten. Därför spärras Backspace/Delete
//! vid cellens kanter, Vänster/Höger hoppar över dem och `|` skrivs som `\|`. Enter (också med
//! Skift) lägger in `<br>`, en dold radbrytning som Backspace/Delete/Vänster/Höger hanterar som
//! ett enda tecken. Upp/Ned räknas av `grid` ur cellernas geometri (cellernas rader överlappar
//! i höjdled och galleriets egen Upp/Ned duger inte), och musklick i tabeller mappas av
//! `pointer_fix` på samma sätt.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Context, Event, Galley, Id, Key, Pos2, Response, Ui};

use super::grid::{self, TableGeom};
use super::markup::{self, Edit, TableInfo};
use crate::app::{KladdApp, byte_to_char, char_to_byte, editor_id, menu_divider, menu_item};
use crate::i18n::t;
use crate::icons::{self, Icon};
use crate::theme::Palette;

// ------------------------------------------------------------------ operationer

/// Tabelloperationerna i verktygsraden och högerklicksmenyn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableOp {
    RowAbove,
    RowBelow,
    ColLeft,
    ColRight,
    DelRow,
    DelCol,
    DelTable,
    Merge,
    Split,
}

impl TableOp {
    pub const ALL: [TableOp; 9] = [
        TableOp::RowAbove,
        TableOp::RowBelow,
        TableOp::ColLeft,
        TableOp::ColRight,
        TableOp::Merge,
        TableOp::Split,
        TableOp::DelRow,
        TableOp::DelCol,
        TableOp::DelTable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TableOp::RowAbove => t("Rad ovanför"),
            TableOp::RowBelow => t("Rad nedanför"),
            TableOp::ColLeft => t("Kolumn till vänster"),
            TableOp::ColRight => t("Kolumn till höger"),
            TableOp::DelRow => t("Ta bort rad"),
            TableOp::DelCol => t("Ta bort kolumn"),
            TableOp::DelTable => t("Ta bort tabell"),
            TableOp::Merge => t("Sammanfoga celler"),
            TableOp::Split => t("Dela cell"),
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            TableOp::RowAbove => Icon::TableRowAbove,
            TableOp::RowBelow => Icon::TableRowBelow,
            TableOp::ColLeft => Icon::TableColLeft,
            TableOp::ColRight => Icon::TableColRight,
            TableOp::DelRow => Icon::TableDelRow,
            TableOp::DelCol => Icon::TableDelCol,
            TableOp::DelTable => Icon::TableDelete,
            TableOp::Merge => Icon::TableMerge,
            TableOp::Split => Icon::TableSplit,
        }
    }

    /// Kör operationen på markeringen `sel` (sammanfoga använder hela markeringen, övriga
    /// markörens läge).
    pub fn apply(self, text: &str, sel: Range<usize>) -> Edit {
        let pos = sel.start;
        match self {
            TableOp::RowAbove => markup::table_insert_row(text, pos, false),
            TableOp::RowBelow => markup::table_insert_row(text, pos, true),
            TableOp::ColLeft => markup::table_insert_col(text, pos, false),
            TableOp::ColRight => markup::table_insert_col(text, pos, true),
            TableOp::DelRow => markup::table_delete_row(text, pos),
            TableOp::DelCol => markup::table_delete_col(text, pos),
            TableOp::DelTable => markup::table_delete(text, pos),
            TableOp::Merge => markup::table_merge_cells(text, sel),
            TableOp::Split => markup::table_split_cell(text, pos),
        }
    }
}

// ------------------------------------------------------------------ ren logik

/// Tangenterna som tabellen bryr sig om.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKey {
    Backspace,
    Delete,
    Left,
    Right,
    Tab,
    ShiftTab,
    Enter,
    Up,
    Down,
}

/// Vad en tangent ska göra när markören står i en tabell.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Låt TextEdit hantera tangenten.
    Pass,
    /// Förbruka tangenten utan effekt.
    Block,
    /// Flytta (hoppa) markören till byteläget; texten är oförändrad.
    Move(usize),
    /// Ersätt text och markering.
    Edit(Edit),
}

/// Reveal-intervall: med markören i en tabell bara cellen (eller cellerna) markeringen berör, så
/// att inline-markup visas dämpad utan att pipor eller avgränsarrad avslöjas. Annars hela rader.
pub fn reveal_range(text: &str, sel: Range<usize>) -> Range<usize> {
    if let (Some(a), Some(b)) = (markup::table_at(text, sel.start), markup::table_at(text, sel.end)) {
        if a.range == b.range {
            if let (Some(x), Some(y)) =
                (markup::table_cell_bounds(text, sel.start), markup::table_cell_bounds(text, sel.end))
            {
                return x.start..y.end.max(x.start);
            }
        }
    }
    markup::reveal_range(text, sel)
}

/// Text som skrivs eller klistras in i en cell: `|` blir `\|`, radbrytning blir `<br>` (en
/// avslutande hoppas över) och tabbtecken och vagnreturer försvinner.
pub fn sanitize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim_end_matches(['\n', '\r']).chars() {
        match c {
            '|' => out.push_str("\\|"),
            '\n' => out.push_str("<br>"),
            '\r' | '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// Får text skrivas vid markeringen? Bara när den ligger inom en enda cells innehåll.
pub fn text_allowed(text: &str, sel: Range<usize>) -> bool {
    let (Some(a), Some(b)) = (markup::table_cell_bounds(text, sel.start), markup::table_cell_bounds(text, sel.end))
    else {
        return false;
    };
    a == b && sel.start >= a.start && sel.end <= a.end
}

/// Cellen före (efter) cell (rad, kolumn): föregående cellens slut / nästa cells början, med
/// radbyte. `None` vid tabellens första (sista) cell.
fn neighbour(t: &TableInfo, (r, c): (usize, usize), back: bool) -> Option<usize> {
    if back {
        if c > 0 {
            return t.rows[r].get(c - 1).map(|x| x.end);
        }
        let pr = r.checked_sub(1)?;
        t.rows[pr].last().map(|x| x.end)
    } else {
        if let Some(x) = t.rows[r].get(c + 1) {
            return Some(x.start);
        }
        t.rows.get(r + 1)?.first().map(|x| x.start)
    }
}

/// Beslut för en tangent. `shift`/`ctrl` är modifierare på tangenthändelsen.
pub fn key_action(text: &str, sel: Range<usize>, key: TableKey, shift: bool, ctrl: bool) -> Action {
    let Some(t) = markup::table_at(text, sel.start) else { return Action::Pass };
    let b = text.as_bytes();
    match key {
        TableKey::Enter => {
            // En radbrytning i cellen. Utanför cellens innehåll (avgränsarraden) hände inget.
            if !text_allowed(text, sel.clone()) {
                return Action::Block;
            }
            let mut s = text.to_string();
            s.replace_range(sel.clone(), "<br>");
            let p = sel.start + 4;
            return Action::Edit(Edit { text: s, sel: p..p });
        }
        TableKey::Tab | TableKey::ShiftTab => {
            let back = key == TableKey::ShiftTab;
            let pos = if back { sel.start } else { sel.end };
            return markup::table_next_cell(text, pos, back).map_or(Action::Block, Action::Edit);
        }
        _ => {}
    }
    let Some(cell) = markup::table_cell_at(&t, sel.start) else {
        // Avgränsarraden: inget att radera där, markören flyttas av `clamp_caret`.
        return if matches!(key, TableKey::Backspace | TableKey::Delete) { Action::Block } else { Action::Pass };
    };
    let Some(bounds) = markup::table_cell_bounds(text, sel.start) else { return Action::Pass };
    if !sel.is_empty() {
        return match key {
            TableKey::Backspace | TableKey::Delete if !text_allowed(text, sel.clone()) => Action::Block,
            _ => Action::Pass,
        };
    }
    let pos = sel.start;
    match key {
        TableKey::Backspace => {
            if pos <= bounds.start {
                Action::Block
            } else if ctrl && text[bounds.start..pos].trim().is_empty() {
                Action::Block
            } else if let Some(i) = markup::br_ending_at(text, pos).filter(|&i| !ctrl && i >= bounds.start) {
                // Tar bort `<br>` som ett tecken.
                let mut s = text.to_string();
                s.replace_range(i..pos, "");
                Action::Edit(Edit { text: s, sel: i..i })
            } else if !ctrl && pos >= bounds.start + 2 && b[pos - 2] == b'\\' && b[pos - 1] == b'|' {
                // Tar bort `\|` som ett tecken.
                let mut s = text.to_string();
                s.replace_range(pos - 2..pos, "");
                Action::Edit(Edit { text: s, sel: pos - 2..pos - 2 })
            } else {
                Action::Pass
            }
        }
        TableKey::Delete => {
            if pos >= bounds.end {
                Action::Block
            } else if ctrl && text[pos..bounds.end].trim().is_empty() {
                Action::Block
            } else if let Some(e) = markup::br_starting_at(text, pos).filter(|&e| !ctrl && e <= bounds.end) {
                let mut s = text.to_string();
                s.replace_range(pos..e, "");
                Action::Edit(Edit { text: s, sel: pos..pos })
            } else if !ctrl && pos + 2 <= bounds.end && b[pos] == b'\\' && b[pos + 1] == b'|' {
                let mut s = text.to_string();
                s.replace_range(pos..pos + 2, "");
                Action::Edit(Edit { text: s, sel: pos..pos })
            } else {
                Action::Pass
            }
        }
        TableKey::Left => {
            if pos > bounds.start {
                match markup::br_ending_at(text, pos) {
                    Some(i) if !shift && !ctrl && i >= bounds.start => Action::Move(i),
                    _ => Action::Pass,
                }
            } else if shift {
                Action::Block
            } else {
                neighbour(&t, cell, true).map_or(Action::Block, Action::Move)
            }
        }
        TableKey::Right => {
            if pos < bounds.end {
                match markup::br_starting_at(text, pos) {
                    Some(e) if !shift && !ctrl && e <= bounds.end => Action::Move(e),
                    _ => Action::Pass,
                }
            } else if shift {
                Action::Block
            } else {
                neighbour(&t, cell, false).map_or(Action::Block, Action::Move)
            }
        }
        TableKey::Tab | TableKey::ShiftTab | TableKey::Enter | TableKey::Up | TableKey::Down => Action::Pass,
    }
}

/// Rättar en markör som hamnat utanför cellens innehåll (klick på dolda pipor eller utfyllnad,
/// Upp/Ned, Home/End): närmaste kant i cellen. På avgränsarraden hoppar markören till raden
/// under (om den kom ovanifrån, `prev`) eller över, i samma kolumn. `None` = inget att rätta.
pub fn clamp_caret(text: &str, pos: usize, prev: Option<usize>) -> Option<usize> {
    let t = markup::table_at(text, pos)?;
    match markup::table_cell_at(&t, pos) {
        Some(_) => {
            let b = markup::table_cell_bounds(text, pos)?;
            if pos < b.start {
                Some(b.start)
            } else if pos > b.end {
                Some(b.end)
            } else {
                // Inne i en `<br>`-tagg: efter den.
                let i = text[b.start..pos].rfind('<')? + b.start;
                markup::br_starting_at(text, i).filter(|&e| i < pos && pos < e)
            }
        }
        None => {
            let col = prev.and_then(|p| markup::table_cell_at(&t, p)).map_or(0, |x| x.1);
            let down = prev.is_none_or(|p| p < t.delim_row.start);
            let row = if down && t.rows.len() > 1 { 1 } else { 0 };
            let cells = &t.rows[row];
            cells.get(col.min(cells.len().saturating_sub(1))).map(|c| c.start)
        }
    }
}

// ------------------------------------------------------------------ egui

fn prev_id() -> Id {
    Id::unique("kladd-table-prev-caret")
}

fn menu_flag_id() -> Id {
    Id::unique("kladd-table-menu")
}

pub(crate) fn set_caret(ctx: &Context, id: Id, text: &str, from: usize, to: usize) {
    let mut st = TextEditState::load(ctx, id).unwrap_or_default();
    let a = cursor_at(text, from);
    let b = cursor_at(text, to);
    st.cursor.set_char_range(Some(CCursorRange::two(a, b)));
    st.store(ctx, id);
}

/// Markör vid byteläget. Direkt efter en `<br>` ligger läget på gränsen mellan två rader och
/// ska visas först på nästa.
pub(crate) fn cursor_at(text: &str, b: usize) -> CCursor {
    let mut c = CCursor::new(byte_to_char(text, b));
    c.prefer_next_row = markup::br_ending_at(text, b).is_some();
    c
}

/// Efter TextEdit: håll en hoppande markör i cellen och minns var den stod.
pub fn fix_caret(ctx: &Context, id: Id, text: &str) {
    if !ctx.memory(|m| m.has_focus(id)) {
        return;
    }
    let st = TextEditState::load(ctx, id).unwrap_or_default();
    let Some(r) = st.cursor.char_range() else { return };
    if r.primary != r.secondary {
        ctx.data_mut(|d| d.remove::<usize>(prev_id()));
        return;
    }
    let mut pos = char_to_byte(text, r.primary.index.0);
    let prev = ctx.data(|d| d.get_temp::<usize>(prev_id()));
    // En markör efter `<br>` som inte vill till nästa rad (Slut, Backspace) står på radens slut.
    if !r.primary.prefer_next_row {
        if let Some(i) = markup::br_ending_at(text, pos).filter(|_| markup::table_at(text, pos).is_some()) {
            set_caret(ctx, id, text, i, i);
            ctx.request_repaint();
            ctx.data_mut(|d| d.insert_temp(prev_id(), i));
            return;
        }
    }
    if let Some(t) = clamp_caret(text, pos, prev) {
        set_caret(ctx, id, text, t, t);
        ctx.request_repaint();
        pos = t;
    }
    ctx.data_mut(|d| d.insert_temp(prev_id(), pos));
}

/// Högerklick på byteläget `click`: flytta markören dit om det är i en tabell (och utanför
/// markeringen) och kom ihåg att menyn ska ha tabellvalen. Sant om klicket var i en tabell.
pub fn on_secondary_click(ctx: &Context, id: Id, text: &str, click: Option<usize>) -> bool {
    let in_table = click.is_some_and(|b| markup::table_at(text, b).is_some());
    if let (true, Some(b)) = (in_table, click) {
        let st = TextEditState::load(ctx, id).unwrap_or_default();
        let inside = st.cursor.char_range().is_some_and(|r| {
            let s = r.as_sorted_char_range();
            let (s0, s1) = (char_to_byte(text, s.start.0), char_to_byte(text, s.end.0));
            s0 != s1 && s0 <= b && b <= s1
        });
        if !inside {
            set_caret(ctx, id, text, b, b);
            fix_caret(ctx, id, text);
        }
    }
    ctx.data_mut(|d| d.insert_temp(menu_flag_id(), in_table));
    in_table
}

/// Ska högerklicksmenyn ha tabellvalen (klicket gjordes i en tabell)?
pub fn menu_open(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(menu_flag_id())).unwrap_or(false)
}

/// Verktygsradens tabellgrupp. Returnerar den klickade operationen.
pub fn bar_group(ui: &mut Ui, p: &Palette) -> Option<TableOp> {
    let mut hit = None;
    for op in TableOp::ALL {
        if icons::button(ui, p, op.icon(), 28.0, false, op.label(), p.chrome).clicked() {
            hit = Some(op);
        }
    }
    hit
}

/// Tabellvalen i högerklicksmenyn.
pub fn menu_items(ui: &mut Ui, p: &Palette) -> Option<TableOp> {
    let mut hit = None;
    for op in TableOp::ALL {
        if matches!(op, TableOp::DelRow | TableOp::Merge) {
            menu_divider(ui, p);
        }
        if menu_item(ui, p, op.label(), "", None, true) {
            hit = Some(op);
        }
    }
    hit
}

/// Upp/Ned i tabellen, räknat ur cellernas geometri. Skift+Upp/Ned (markering över celler) görs
/// inte i tabeller; Ctrl/Alt lämnas åt TextEdit.
pub fn vertical_action(text: &str, sel: Range<usize>, g: &Galley, geoms: &[TableGeom], up: bool, shift: bool) -> Action {
    if markup::table_at(text, sel.start).is_none() {
        return Action::Pass;
    }
    if shift {
        return Action::Block;
    }
    let from = if up { sel.start } else { sel.end };
    match grid::vertical(text, g, geoms, from, up) {
        Some(b) => {
            let b = clamp_caret(text, b, None).unwrap_or(b);
            Action::Move(b)
        }
        None => Action::Pass,
    }
}

/// Byteläget under pekaren (galleri-koordinater): tabellens egen träffyta i tabeller, annars
/// galleriets.
pub fn byte_at(text: &str, g: &Galley, geoms: &[TableGeom], local: egui::Vec2) -> usize {
    if !geoms.is_empty() {
        let idx = grid::GalleyIndex::new(text, g);
        if let Some(b) = geoms.iter().find_map(|t| grid::hit(g, &idx, t, local)) {
            return b;
        }
    }
    char_to_byte(text, g.cursor_from_pos(local).index.0)
}

fn drag_id() -> Id {
    Id::unique("kladd-table-drag")
}

/// Ordet (bokstäver och siffror) kring `pos` i cellen `bounds`; tomt om `pos` inte står vid ett.
fn word_range(text: &str, pos: usize, bounds: &Range<usize>) -> Range<usize> {
    let cell = &text[bounds.clone()];
    let at = pos - bounds.start;
    let is_w = |c: char| c.is_alphanumeric() || c == '_';
    let start = cell[..at].char_indices().rev().take_while(|&(_, c)| is_w(c)).last().map_or(at, |(i, _)| i);
    let end = at + cell[at..].char_indices().take_while(|&(_, c)| is_w(c)).last().map_or(0, |(i, c)| i + c.len_utf8());
    bounds.start + start..bounds.start + end
}

/// Efter TextEdit: musklick, drag och dubbelklick i tabeller. TextEdit mappar pekaren med
/// `Galley::cursor_from_pos`, som antar att raderna ligger under varandra; i en tabell
/// ligger cellernas rader bredvid varandra, så markören sätts om här. `blocked` = pekaren är på
/// en kolumnkant (som tar emot draget själv).
#[allow(clippy::too_many_arguments)]
pub fn pointer_fix(
    ctx: &Context,
    id: Id,
    text: &str,
    g: &Galley,
    gpos: Pos2,
    geoms: &[TableGeom],
    resp: &Response,
    blocked: bool,
) {
    if geoms.is_empty() || blocked {
        return;
    }
    let (pressed, down, shift, ptr) =
        ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.modifiers.shift, i.pointer.interact_pos()));
    if !down {
        ctx.data_mut(|d| d.remove::<usize>(drag_id()));
    }
    let Some(ptr) = ptr else { return };
    let hit = || {
        let idx = grid::GalleyIndex::new(text, g);
        geoms.iter().find_map(|t| grid::hit(g, &idx, t, ptr - gpos))
    };
    let anchor = ctx.data(|d| d.get_temp::<usize>(drag_id()));
    if pressed && resp.contains_pointer() {
        match hit() {
            Some(b) => {
                let b = clamp_caret(text, b, None).unwrap_or(b);
                let secondary = TextEditState::load(ctx, id)
                    .and_then(|s| s.cursor.char_range())
                    .map(|r| char_to_byte(text, r.secondary.index.0));
                let from = if shift { secondary.unwrap_or(b) } else { b };
                set_caret(ctx, id, text, from, b);
                ctx.data_mut(|d| d.insert_temp(drag_id(), from));
                ctx.request_repaint();
            }
            None => ctx.data_mut(|d| d.remove::<usize>(drag_id())),
        }
    } else if down {
        if let (Some(from), Some(b)) = (anchor, hit()) {
            set_caret(ctx, id, text, from, b);
            ctx.request_repaint();
        }
    }
    if resp.double_clicked() {
        if let Some(b) = hit() {
            if let Some(bounds) = markup::table_cell_bounds(text, b) {
                let w = word_range(text, b.clamp(bounds.start, bounds.end), &bounds);
                if !w.is_empty() {
                    set_caret(ctx, id, text, w.start, w.end);
                    ctx.request_repaint();
                }
            }
        }
    }
}

fn table_key(key: Key, shift: bool) -> Option<TableKey> {
    Some(match key {
        Key::Backspace => TableKey::Backspace,
        Key::Delete => TableKey::Delete,
        Key::ArrowLeft => TableKey::Left,
        Key::ArrowRight => TableKey::Right,
        Key::Tab if shift => TableKey::ShiftTab,
        Key::Tab => TableKey::Tab,
        Key::Enter => TableKey::Enter,
        Key::ArrowUp => TableKey::Up,
        Key::ArrowDown => TableKey::Down,
        _ => return None,
    })
}

impl KladdApp {
    /// Före TextEdit: tangenter och text som skrivs i en tabell hålls inom cellen.
    pub(crate) fn word_table_keys(&mut self, ctx: &Context) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if !ctx.memory(|m| m.has_focus(editor_id(doc.id))) {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        let mut keep = Vec::with_capacity(events.len());
        let mut changed = false;
        for ev in events {
            let sel = self.word_selection(ctx);
            let text = &self.docs[self.active].text;
            if markup::table_at(text, sel.start).is_none() {
                keep.push(ev);
                continue;
            }
            match &ev {
                Event::Key { key, pressed: true, modifiers, .. } => {
                    let Some(k) = table_key(*key, modifiers.shift) else {
                        keep.push(ev);
                        continue;
                    };
                    let ctrl = modifiers.command || modifiers.ctrl;
                    let act = match k {
                        TableKey::Up | TableKey::Down if modifiers.alt || ctrl => Action::Pass,
                        TableKey::Up | TableKey::Down => {
                            let doc_id = self.docs[self.active].id;
                            match self.word.table_view(doc_id, text) {
                                Some((g, geoms)) => {
                                    vertical_action(text, sel, &g, &geoms, k == TableKey::Up, modifiers.shift)
                                }
                                None => Action::Block,
                            }
                        }
                        _ => key_action(text, sel, k, modifiers.shift, ctrl),
                    };
                    match act {
                        Action::Pass => keep.push(ev),
                        Action::Block => changed = true,
                        Action::Move(p) => {
                            changed = true;
                            self.word_edit(ctx, |t, _| Edit { text: t.to_string(), sel: p..p });
                        }
                        Action::Edit(e) => {
                            changed = true;
                            self.word_edit(ctx, |_, _| e);
                        }
                    }
                }
                Event::Text(s) | Event::Paste(s) => {
                    if !text_allowed(text, sel) {
                        changed = true;
                        continue;
                    }
                    let n = sanitize_text(s);
                    if n == *s {
                        keep.push(ev);
                    } else {
                        changed = true;
                        if !n.is_empty() {
                            keep.push(if matches!(ev, Event::Text(_)) { Event::Text(n) } else { Event::Paste(n) });
                        }
                    }
                }
                _ => keep.push(ev),
            }
        }
        if changed {
            ctx.input_mut(|i| i.events = keep);
        }
    }

    /// Kör en tabelloperation på cellen vid markören.
    pub(crate) fn word_table_op(&mut self, ctx: &Context, op: TableOp) {
        self.word_edit(ctx, |t, s| op.apply(t, s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "| Namn | Id |\n| --- | --- |\n| Alex | 7 |\n";

    fn at(needle: &str) -> usize {
        T.find(needle).unwrap()
    }

    fn act(pos: usize, key: TableKey) -> Action {
        key_action(T, pos..pos, key, false, false)
    }

    #[test]
    fn backspace_at_cell_start_is_blocked() {
        assert_eq!(act(at("Namn"), TableKey::Backspace), Action::Block);
        assert_eq!(act(at("Namn") + 1, TableKey::Backspace), Action::Pass);
        assert_eq!(act(at("Alex"), TableKey::Backspace), Action::Block);
    }

    #[test]
    fn delete_at_cell_end_is_blocked() {
        assert_eq!(act(at("Namn") + 4, TableKey::Delete), Action::Block);
        assert_eq!(act(at("Namn"), TableKey::Delete), Action::Pass);
        // Markören på de dolda tecknen efter cellen (före pipan) räknas också som slutet.
        assert_eq!(act(at(" | Id"), TableKey::Delete), Action::Block);
    }

    #[test]
    fn selection_across_cells_cannot_be_deleted() {
        let r = at("Namn")..at("Id");
        assert_eq!(key_action(T, r, TableKey::Backspace, false, false), Action::Block);
        let r = at("Namn")..at("Namn") + 2;
        assert_eq!(key_action(T, r, TableKey::Backspace, false, false), Action::Pass);
    }

    #[test]
    fn left_skips_hidden_pipe() {
        // Från början av "Id" till slutet av "Namn".
        assert_eq!(act(at("Id"), TableKey::Left), Action::Move(at("Namn") + 4));
        // Inne i cellen går Vänster som vanligt.
        assert_eq!(act(at("Namn") + 2, TableKey::Left), Action::Pass);
        // Första cellen: ingenting före.
        assert_eq!(act(at("Namn"), TableKey::Left), Action::Block);
        // Första cellen på andra raden: slutet av sista cellen på raden över.
        assert_eq!(act(at("Alex"), TableKey::Left), Action::Move(at("Id") + 2));
    }

    #[test]
    fn right_skips_hidden_pipe() {
        assert_eq!(act(at("Namn") + 4, TableKey::Right), Action::Move(at("Id")));
        assert_eq!(act(at("Namn"), TableKey::Right), Action::Pass);
        assert_eq!(act(at("Id") + 2, TableKey::Right), Action::Move(at("Alex")));
        // Sista cellen: stannar kvar.
        assert_eq!(act(at("7") + 1, TableKey::Right), Action::Block);
    }

    #[test]
    fn shift_arrows_stop_at_cell_edge() {
        assert_eq!(key_action(T, at("Id")..at("Id"), TableKey::Left, true, false), Action::Block);
        assert_eq!(key_action(T, at("Id")..at("Id"), TableKey::Right, true, false), Action::Pass);
    }

    #[test]
    fn enter_inserts_a_line_break_in_a_cell() {
        match act(at("Namn") + 2, TableKey::Enter) {
            Action::Edit(e) => {
                assert!(e.text.starts_with("| Na<br>mn |"), "{}", e.text);
                assert_eq!(e.sel, at("Namn") + 6..at("Namn") + 6);
            }
            a => panic!("{a:?}"),
        }
        // En markering ersätts.
        let r = at("Namn")..at("Namn") + 2;
        match key_action(T, r, TableKey::Enter, true, false) {
            Action::Edit(e) => assert!(e.text.starts_with("| <br>mn |"), "{}", e.text),
            a => panic!("{a:?}"),
        }
        // Avgränsarraden och markeringar över flera celler: ingenting.
        assert_eq!(act(at("---"), TableKey::Enter), Action::Block);
        assert_eq!(key_action(T, at("Namn")..at("Id"), TableKey::Enter, false, false), Action::Block);
        let t = "text\n";
        assert_eq!(key_action(t, 1..1, TableKey::Enter, false, false), Action::Pass);
    }

    const BR: &str = "| a<br>b | c |\n| --- | --- |\n| 1 | 2 |\n";

    #[test]
    fn backspace_and_delete_remove_br_as_one() {
        let after = BR.find("<br>").unwrap() + 4;
        match key_action(BR, after..after, TableKey::Backspace, false, false) {
            Action::Edit(e) => {
                assert!(e.text.starts_with("| ab |"), "{}", e.text);
                assert_eq!(e.sel, after - 4..after - 4);
            }
            a => panic!("{a:?}"),
        }
        let before = after - 4;
        match key_action(BR, before..before, TableKey::Delete, false, false) {
            Action::Edit(e) => assert!(e.text.starts_with("| ab |"), "{}", e.text),
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn arrows_jump_over_br() {
        let after = BR.find("<br>").unwrap() + 4;
        assert_eq!(key_action(BR, after..after, TableKey::Left, false, false), Action::Move(after - 4));
        assert_eq!(key_action(BR, after - 4..after - 4, TableKey::Right, false, false), Action::Move(after));
        // Inne i taggen rättas markören till efter den.
        assert_eq!(clamp_caret(BR, after - 2, None), Some(after));
        assert_eq!(clamp_caret(BR, after, None), None);
    }

    #[test]
    fn merge_and_split_ops() {
        let t = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let e = TableOp::Merge.apply(t, t.find('A').unwrap()..t.find('B').unwrap() + 1);
        assert!(e.text.starts_with("| A B ||"), "{}", e.text);
        let e2 = TableOp::Split.apply(&e.text, e.sel.start..e.sel.start);
        assert!(e2.text.starts_with("| A B |  |"), "{}", e2.text);
        assert_eq!(TableOp::ALL.len(), 9);
        assert_eq!(TableOp::Merge.label(), "Sammanfoga celler");
        assert_eq!(TableOp::Split.label(), "Dela cell");
    }

    #[test]
    fn words_in_cells_for_double_click() {
        let t = "| ab cd |";
        let b = 2..t.len() - 2;
        assert_eq!(&t[word_range(t, 3, &b)], "ab");
        assert_eq!(&t[word_range(t, 6, &b)], "cd");
        assert_eq!(&t[word_range(t, 4, &b)], "ab");
    }

    #[test]
    fn tab_moves_and_selects_cell() {
        match act(at("Namn"), TableKey::Tab) {
            Action::Edit(e) => {
                assert_eq!(e.text, T);
                assert_eq!(&e.text[e.sel], "Id");
            }
            a => panic!("{a:?}"),
        }
        match act(at("Id"), TableKey::ShiftTab) {
            Action::Edit(e) => assert_eq!(&e.text[e.sel], "Namn"),
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn tab_from_last_cell_appends_row() {
        match act(at("7"), TableKey::Tab) {
            Action::Edit(e) => {
                let t = markup::table_at(&e.text, 0).unwrap();
                assert_eq!(t.rows.len(), 3);
                assert!(e.sel.is_empty());
                assert_eq!(markup::table_cell_at(&t, e.sel.start), Some((2, 0)));
            }
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn pipe_is_escaped_and_newline_becomes_br() {
        assert_eq!(sanitize_text("a|b"), "a\\|b");
        assert_eq!(sanitize_text("a\nb\t"), "a<br>b");
        assert_eq!(sanitize_text("a\r\nb\n"), "a<br>b");
        assert_eq!(sanitize_text("abc"), "abc");
    }

    #[test]
    fn typing_only_inside_a_cell() {
        assert!(text_allowed(T, at("Namn")..at("Namn") + 2));
        assert!(text_allowed(T, at("Namn") + 4..at("Namn") + 4));
        assert!(!text_allowed(T, at("Namn")..at("Id")));
        // Avgränsarraden.
        assert!(!text_allowed(T, at("---")..at("---")));
    }

    #[test]
    fn backspace_removes_escaped_pipe_whole() {
        let t = "| a\\|b | c |\n| --- | --- |\n| 1 | 2 |\n";
        let pos = t.find('b').unwrap();
        match key_action(t, pos..pos, TableKey::Backspace, false, false) {
            Action::Edit(e) => assert!(e.text.starts_with("| ab |")),
            a => panic!("{a:?}"),
        }
        let pos = t.find('\\').unwrap();
        match key_action(t, pos..pos, TableKey::Delete, false, false) {
            Action::Edit(e) => assert!(e.text.starts_with("| ab |")),
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn clicks_on_hidden_text_snap_to_the_cell() {
        // På pipan efter "Namn" och på utfyllnaden före "Id".
        assert_eq!(clamp_caret(T, at("| Id"), None), Some(at("Namn") + 4));
        assert_eq!(clamp_caret(T, at("| Id") + 1, None), Some(at("Id")));
        assert_eq!(clamp_caret(T, at("Namn") + 2, None), None);
        assert_eq!(clamp_caret(T, 0, None), Some(at("Namn")));
        // Avgränsarraden: neråt från rubriken och uppåt från brödraden, i samma kolumn.
        let d = at("---");
        assert_eq!(clamp_caret(T, d, Some(at("Id"))), Some(at("7")));
        assert_eq!(clamp_caret(T, d, Some(at("7"))), Some(at("Id")));
        // Utanför tabellen.
        assert_eq!(clamp_caret("hej", 1, None), None);
    }

    #[test]
    fn reveal_is_only_the_cell() {
        let t = "| **fet** | b |\n| --- | --- |\n| 1 | 2 |\n";
        let pos = t.find("fet").unwrap();
        let r = reveal_range(t, pos..pos);
        assert_eq!(&t[r], "**fet**");
        // Utanför tabellen: hela raden som förut.
        let u = "hej\n\n| a | b |\n| - | - |\n";
        assert_eq!(reveal_range(u, 1..1), markup::reveal_range(u, 1..1));
    }

    #[test]
    fn ops_edit_the_table() {
        let e = TableOp::RowBelow.apply(T, at("Namn")..at("Namn"));
        assert_eq!(markup::table_at(&e.text, 0).unwrap().rows.len(), 3);
        let e = TableOp::ColRight.apply(T, at("Namn")..at("Namn"));
        assert_eq!(markup::table_at(&e.text, 0).unwrap().cols, 3);
        let e = TableOp::DelTable.apply(T, at("Namn")..at("Namn"));
        assert!(markup::table_at(&e.text, 0).is_none());
    }
}
