//! Tabellredigering i dokumentläget: tabellen är ett rutnät också med markören i den, och
//! markören hålls i cellens innehåll. Den rena logiken (vad en tangent ska göra givet text och
//! markör) ligger överst och är enhetstestad; längre ner kopplas den till egui och KladdApp.
//!
//! Pipor och cellutfyllnad är dolda (nollbreda) tecken i texten. Därför spärras Backspace/Delete
//! vid cellens kanter, Vänster/Höger hoppar över dem, `|` skrivs som `\|` och Enter ignoreras
//! (en Markdown-cell kan inte innehålla ett stycke).

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Context, Event, Id, Key, Ui};

use super::markup::{self, Edit, TableInfo};
use crate::app::{KladdApp, byte_to_char, char_to_byte, editor_id, menu_divider, menu_item};
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
}

impl TableOp {
    pub const ALL: [TableOp; 7] = [
        TableOp::RowAbove,
        TableOp::RowBelow,
        TableOp::ColLeft,
        TableOp::ColRight,
        TableOp::DelRow,
        TableOp::DelCol,
        TableOp::DelTable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TableOp::RowAbove => "Rad ovanför",
            TableOp::RowBelow => "Rad nedanför",
            TableOp::ColLeft => "Kolumn till vänster",
            TableOp::ColRight => "Kolumn till höger",
            TableOp::DelRow => "Ta bort rad",
            TableOp::DelCol => "Ta bort kolumn",
            TableOp::DelTable => "Ta bort tabell",
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
        }
    }

    pub fn apply(self, text: &str, pos: usize) -> Edit {
        match self {
            TableOp::RowAbove => markup::table_insert_row(text, pos, false),
            TableOp::RowBelow => markup::table_insert_row(text, pos, true),
            TableOp::ColLeft => markup::table_insert_col(text, pos, false),
            TableOp::ColRight => markup::table_insert_col(text, pos, true),
            TableOp::DelRow => markup::table_delete_row(text, pos),
            TableOp::DelCol => markup::table_delete_col(text, pos),
            TableOp::DelTable => markup::table_delete(text, pos),
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

/// Text som skrivs eller klistras in i en cell: `|` blir `\|`, radbrytning blir blanksteg och
/// tabbtecken och vagnreturer försvinner.
pub fn sanitize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '|' => out.push_str("\\|"),
            '\n' => out.push(' '),
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
        TableKey::Enter => return Action::Block,
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
                Action::Pass
            } else if shift {
                Action::Block
            } else {
                neighbour(&t, cell, true).map_or(Action::Block, Action::Move)
            }
        }
        TableKey::Right => {
            if pos < bounds.end {
                Action::Pass
            } else if shift {
                Action::Block
            } else {
                neighbour(&t, cell, false).map_or(Action::Block, Action::Move)
            }
        }
        TableKey::Tab | TableKey::ShiftTab | TableKey::Enter => Action::Pass,
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
                None
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

fn set_caret(ctx: &Context, id: Id, text: &str, from: usize, to: usize) {
    let mut st = TextEditState::load(ctx, id).unwrap_or_default();
    let a = CCursor::new(byte_to_char(text, from));
    let b = CCursor::new(byte_to_char(text, to));
    st.cursor.set_char_range(Some(CCursorRange::two(a, b)));
    st.store(ctx, id);
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
        if matches!(op, TableOp::DelRow) {
            menu_divider(ui, p);
        }
        if menu_item(ui, p, op.label(), "", None, true) {
            hit = Some(op);
        }
    }
    hit
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
                    match key_action(text, sel, k, modifiers.shift, modifiers.command || modifiers.ctrl) {
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
        self.word_edit(ctx, |t, s| op.apply(t, s.start));
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
    fn enter_is_ignored_in_table_only() {
        assert_eq!(act(at("Namn"), TableKey::Enter), Action::Block);
        let t = "text\n";
        assert_eq!(key_action(t, 1..1, TableKey::Enter, false, false), Action::Pass);
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
    fn pipe_is_escaped_and_newline_dropped() {
        assert_eq!(sanitize_text("a|b"), "a\\|b");
        assert_eq!(sanitize_text("a\nb\t"), "a b");
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
        let e = TableOp::RowBelow.apply(T, at("Namn"));
        assert_eq!(markup::table_at(&e.text, 0).unwrap().rows.len(), 3);
        let e = TableOp::ColRight.apply(T, at("Namn"));
        assert_eq!(markup::table_at(&e.text, 0).unwrap().cols, 3);
        let e = TableOp::DelTable.apply(T, at("Namn"));
        assert!(markup::table_at(&e.text, 0).is_none());
    }
}
