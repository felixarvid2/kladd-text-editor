//! Vim-tangentbindningar i textläget (inställningen "Vim-läge", av som standard).
//!
//! Som i `multicursor.rs` fångas tangenterna före egui:s `TextEdit`, men all logik är en ren
//! tillståndsmaskin här: [`Vim::key`] tar en tangent, texten och markören (`Vim::cur`) och ger
//! tillbaka vad som ändrades och vad appen ska göra ([`Effect`]: spara, stänga, dela vyn …).
//! Gränssnittsdelen (tangenthändelser, blockmarkör, statusrad) finns i `vim_ui.rs`.
//! Alla positioner är teckenindex (som egui:s `CCursor`), inte byteindex. Kolumner räknas i
//! tecken (en tabb är en kolumn). Mönster för `/`, `?` och `:s` är Rusts `regex`-syntax.

use std::collections::HashMap;

use regex::{Regex, RegexBuilder};

use crate::i18n::t;
use crate::tf;

// ------------------------------------------------------------------ gränssnitt utåt

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    VisualBlock,
    Replace,
    Command,
}

impl Mode {
    pub fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine | Mode::VisualBlock)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Char(char),
    Esc,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Ctrl(char),
}

/// Vad som syns i editorn: första synliga rad (0-baserad) och antal synliga rader. Behövs för
/// `H`, `M`, `L`, `Ctrl+d` och `Ctrl+u`.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub top: usize,
    pub lines: usize,
}

impl Default for View {
    fn default() -> Self {
        View { top: 0, lines: 40 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting {
    Number,
    Wrap,
}

/// Något appen ska göra åt Vim.
#[derive(Clone, PartialEq, Debug)]
pub enum Effect {
    Save,
    SaveAs(String),
    /// `:q`: stänger rutan eller fliken (och Kladd när det var den sista).
    Quit { force: bool },
    /// `:qa`
    QuitAll { force: bool },
    /// `:wq` och `:x`
    SaveQuit,
    Edit(String),
    Split { vertical: bool },
    /// `:only` och `:close`: tillbaka till en ruta.
    CloseSplit,
    /// `None` = växla.
    Set(Setting, Option<bool>),
    Notify(String, bool),
    Clipboard(String),
    /// Rulla vyn så här många rader (positivt = nedåt).
    Scroll(i32),
}

/// Resultatet av en tangent.
#[derive(Default, Debug)]
pub struct Out {
    pub edited: bool,
    pub effects: Vec<Effect>,
}

// ------------------------------------------------------------------ tangentkodning

/// Tangenter lagras som tecken i kommandosekvensen och i makron.
pub fn enc(k: Key) -> char {
    match k {
        Key::Char(c) => c,
        Key::Esc => '\u{1b}',
        Key::Enter => '\n',
        Key::Backspace => '\u{7f}',
        Key::Tab => '\t',
        Key::Left => '\u{e000}',
        Key::Right => '\u{e001}',
        Key::Up => '\u{e002}',
        Key::Down => '\u{e003}',
        Key::Home => '\u{e004}',
        Key::End => '\u{e005}',
        Key::Delete => '\u{e006}',
        Key::Ctrl('[') => '\u{1b}',
        Key::Ctrl(c) if c.is_ascii_alphabetic() => (c.to_ascii_lowercase() as u8 & 0x1f) as char,
        Key::Ctrl(_) => '\u{0}',
    }
}

pub fn dec(c: char) -> Key {
    match c {
        '\u{1b}' => Key::Esc,
        '\n' | '\r' => Key::Enter,
        '\u{7f}' | '\u{8}' => Key::Backspace,
        '\t' => Key::Tab,
        '\u{e000}' => Key::Left,
        '\u{e001}' => Key::Right,
        '\u{e002}' => Key::Up,
        '\u{e003}' => Key::Down,
        '\u{e004}' => Key::Home,
        '\u{e005}' => Key::End,
        '\u{e006}' => Key::Delete,
        c if (c as u32) >= 1 && (c as u32) < 0x20 => Key::Ctrl((c as u8 + 96) as char),
        c => Key::Char(c),
    }
}

// ------------------------------------------------------------------ textverktyg

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// Teckenklass för ordrörelser: 0 blank, 1 skiljetecken, 2 ordtecken (med `big` allt som inte är
/// blankt), 3 radbrytning.
fn cls(c: char, big: bool) -> u8 {
    if c == '\n' {
        3
    } else if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        2
    } else {
        1
    }
}

/// Start av raden som innehåller `p`.
fn ls(b: &[char], p: usize) -> usize {
    b[..p.min(b.len())].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1)
}

/// Slut (radbrytningens plats) av raden som innehåller `p`.
fn le(b: &[char], p: usize) -> usize {
    let p = p.min(b.len());
    b[p..].iter().position(|&c| c == '\n').map_or(b.len(), |i| p + i)
}

fn line_idx(b: &[char], p: usize) -> usize {
    b[..p.min(b.len())].iter().filter(|&&c| c == '\n').count()
}

fn nlines(b: &[char]) -> usize {
    b.iter().filter(|&&c| c == '\n').count() + 1
}

/// Startposition för rad `line` (begränsad till sista raden).
fn line_pos(b: &[char], line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    let mut l = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == '\n' {
            l += 1;
            if l == line {
                return i + 1;
            }
        }
    }
    ls(b, b.len())
}

/// Första tecknet som inte är blankt på raden som börjar vid `start`; annars radens slut.
fn first_nb(b: &[char], start: usize) -> usize {
    let mut p = start;
    while p < b.len() && is_blank(b[p]) {
        p += 1;
    }
    p
}

fn col_of(b: &[char], p: usize) -> usize {
    p - ls(b, p)
}

/// Normalläget tillåter inte markören på radbrytningen (utom på tomma rader) eller efter texten.
fn clamp_normal(b: &[char], p: usize) -> usize {
    let p = p.min(b.len());
    if p == b.len() {
        return if p > 0 && b[p - 1] != '\n' { p - 1 } else { p };
    }
    if b[p] == '\n' && p > 0 && b[p - 1] != '\n' { p - 1 } else { p }
}

/// Position på rad `line` med kolumn `col` (begränsad till radens längd).
fn at_col(b: &[char], line: usize, col: usize) -> usize {
    let s = line_pos(b, line);
    s + col.min(le(b, s) - s)
}

fn upper_char(c: char) -> char {
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

fn lower_char(c: char) -> char {
    let mut u = c.to_lowercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

fn toggle_char(c: char) -> char {
    if c.is_uppercase() { lower_char(c) } else { upper_char(c) }
}

// ------------------------------------------------------------------ ordrörelser

fn word_fwd(b: &[char], pos: usize, big: bool, stop_at_eol: bool) -> usize {
    let n = b.len();
    let mut p = pos;
    if p >= n {
        return n;
    }
    let c0 = cls(b[p], big);
    if c0 == 1 || c0 == 2 {
        while p < n && cls(b[p], big) == c0 {
            p += 1;
        }
    }
    while p < n && matches!(cls(b[p], big), 0 | 3) {
        if b[p] == '\n' {
            // En tom rad är ett ord.
            if p > pos && (p == 0 || b[p - 1] == '\n') {
                break;
            }
            if stop_at_eol {
                return p;
            }
        }
        p += 1;
    }
    p
}

fn word_end(b: &[char], pos: usize, big: bool) -> usize {
    let n = b.len();
    let mut p = pos;
    if p + 1 < n {
        p += 1;
    } else {
        return pos;
    }
    while p + 1 < n && matches!(cls(b[p], big), 0 | 3) {
        p += 1;
    }
    let c = cls(b[p], big);
    while p + 1 < n && cls(b[p + 1], big) == c && c != 0 && c != 3 {
        p += 1;
    }
    p
}

fn word_back(b: &[char], pos: usize, big: bool) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut p = pos - 1;
    while p > 0 && matches!(cls(b[p], big), 0 | 3) {
        if b[p] == '\n' && b[p - 1] == '\n' {
            return p;
        }
        p -= 1;
    }
    let c = cls(b[p], big);
    while p > 0 && cls(b[p - 1], big) == c && c != 0 && c != 3 {
        p -= 1;
    }
    p
}

fn word_end_back(b: &[char], pos: usize, big: bool) -> usize {
    if pos == 0 || b.is_empty() {
        return 0;
    }
    let mut p = pos.min(b.len() - 1);
    let c = cls(b[p], big);
    if c == 1 || c == 2 {
        while p > 0 && cls(b[p], big) == c {
            p -= 1;
        }
    } else {
        p -= 1;
    }
    while p > 0 && matches!(cls(b[p], big), 0 | 3) {
        if b[p] == '\n' && b[p - 1] == '\n' {
            return p;
        }
        p -= 1;
    }
    p
}

fn pair_of(c: char) -> Option<(char, char, bool)> {
    match c {
        '(' => Some(('(', ')', true)),
        ')' => Some(('(', ')', false)),
        '[' => Some(('[', ']', true)),
        ']' => Some(('[', ']', false)),
        '{' => Some(('{', '}', true)),
        '}' => Some(('{', '}', false)),
        _ => None,
    }
}

/// `%`: första parentesen från markören till radens slut och dess motsvarighet.
fn match_pair(b: &[char], pos: usize) -> Option<usize> {
    let e = le(b, pos);
    let start = (pos..e).find(|&i| pair_of(b[i]).is_some())?;
    let (open, close, fwd) = pair_of(b[start])?;
    let mut depth = 0i32;
    if fwd {
        for (i, &c) in b.iter().enumerate().skip(start) {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    } else {
        for i in (0..=start).rev() {
            if b[i] == close {
                depth += 1;
            } else if b[i] == open {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

fn is_empty_line(b: &[char], line_start: usize) -> bool {
    line_start >= b.len() || b[line_start] == '\n'
}

/// `}`: början av nästa tomma rad efter ett stycke, annars textens slut.
fn para_fwd(b: &[char], pos: usize) -> usize {
    let mut s = ls(b, pos);
    let next = |s: usize| -> Option<usize> { (le(b, s) < b.len()).then(|| le(b, s) + 1) };
    while is_empty_line(b, s) {
        match next(s) {
            Some(n) => s = n,
            None => return b.len(),
        }
    }
    loop {
        match next(s) {
            Some(n) => {
                s = n;
                if is_empty_line(b, s) {
                    return s;
                }
            }
            None => return b.len(),
        }
    }
}

fn para_back(b: &[char], pos: usize) -> usize {
    let mut s = ls(b, pos);
    let prev = |s: usize| -> Option<usize> { (s > 0).then(|| ls(b, s - 1)) };
    while is_empty_line(b, s) {
        match prev(s) {
            Some(p) => s = p,
            None => return 0,
        }
    }
    loop {
        match prev(s) {
            Some(p) => {
                s = p;
                if is_empty_line(b, s) {
                    return s;
                }
            }
            None => return 0,
        }
    }
}

// ------------------------------------------------------------------ textobjekt

#[derive(Clone, Copy, PartialEq, Debug)]
enum Obj {
    Word { big: bool, inner: bool },
    Para { inner: bool },
    Quote { q: char, inner: bool },
    Pair { open: char, close: char, inner: bool },
    Tag { inner: bool },
}

fn obj_of(c: char, inner: bool) -> Option<Obj> {
    Some(match c {
        'w' => Obj::Word { big: false, inner },
        'W' => Obj::Word { big: true, inner },
        'p' => Obj::Para { inner },
        '"' | '\'' | '`' => Obj::Quote { q: c, inner },
        '(' | ')' | 'b' => Obj::Pair { open: '(', close: ')', inner },
        '[' | ']' => Obj::Pair { open: '[', close: ']', inner },
        '{' | '}' | 'B' => Obj::Pair { open: '{', close: '}', inner },
        '<' | '>' => Obj::Pair { open: '<', close: '>', inner },
        't' => Obj::Tag { inner },
        _ => return None,
    })
}

fn run_end(b: &[char], p: usize, big: bool) -> usize {
    if p >= b.len() {
        return p;
    }
    let c = cls(b[p], big);
    if c == 3 {
        return p;
    }
    let mut e = p;
    while e < b.len() && cls(b[e], big) == c {
        e += 1;
    }
    e
}

fn run_start(b: &[char], p: usize, big: bool) -> usize {
    let c = cls(b[p], big);
    let mut s = p;
    while s > 0 && cls(b[s - 1], big) == c {
        s -= 1;
    }
    s
}

/// Ett textobjekt vid `pos` som (start, slut exklusive, radvis).
fn object(b: &[char], pos: usize, o: Obj, count: usize) -> Option<(usize, usize, bool)> {
    let count = count.max(1);
    match o {
        Obj::Word { big, inner } => {
            if pos >= b.len() || b[pos] == '\n' {
                return None;
            }
            let s0 = run_start(b, pos, big);
            let mut e = run_end(b, pos, big);
            let mut s = s0;
            if inner {
                for _ in 1..count {
                    if e < b.len() && b[e] != '\n' {
                        e = run_end(b, e, big);
                    }
                }
            } else {
                let on_blank = cls(b[pos], big) == 0;
                let step = |mut e: usize, blank_first: bool| -> usize {
                    if blank_first {
                        e = run_end(b, e, big);
                        if e < b.len() && b[e] != '\n' {
                            e = run_end(b, e, big);
                        }
                    } else {
                        e = run_end(b, e, big);
                        if e < b.len() && cls(b[e], big) == 0 {
                            e = run_end(b, e, big);
                        }
                    }
                    e
                };
                if on_blank {
                    if e < b.len() && b[e] != '\n' {
                        e = run_end(b, e, big);
                    }
                } else if e < b.len() && cls(b[e], big) == 0 {
                    e = run_end(b, e, big);
                } else {
                    // Inget blanksteg efter: ta med det före.
                    while s > ls(b, s) && is_blank(b[s - 1]) {
                        s -= 1;
                    }
                }
                for _ in 1..count {
                    if e < b.len() && b[e] != '\n' {
                        let blank_first = cls(b[e], big) == 0;
                        e = step(e, blank_first);
                    }
                }
            }
            Some((s, e, false))
        }
        Obj::Para { inner } => {
            let blank = |l: usize| {
                let s = line_pos(b, l);
                b[s..le(b, s)].iter().all(|c| c.is_whitespace())
            };
            let total = nlines(b);
            let l0 = line_idx(b, pos);
            let kind = blank(l0);
            let mut l1 = l0;
            while l1 > 0 && blank(l1 - 1) == kind {
                l1 -= 1;
            }
            let mut l2 = l0;
            let mut remaining = count;
            let mut cur_kind = kind;
            loop {
                while l2 + 1 < total && blank(l2 + 1) == cur_kind {
                    l2 += 1;
                }
                remaining -= 1;
                // `ap`: stycket följt av tomrader (eller tvärtom); `ip` räknar varje block för sig.
                if !inner && remaining == 0 {
                    if l2 + 1 < total {
                        l2 += 1;
                        while l2 + 1 < total && blank(l2 + 1) != cur_kind {
                            l2 += 1;
                        }
                    } else if !kind {
                        while l1 > 0 && blank(l1 - 1) {
                            l1 -= 1;
                        }
                    }
                }
                if remaining == 0 || l2 + 1 >= total {
                    break;
                }
                l2 += 1;
                cur_kind = blank(l2);
            }
            Some((l1, l2, true)).map(|(a, z, _)| (line_pos(b, a), {
                let e = le(b, line_pos(b, z));
                (e + 1).min(b.len())
            }, true))
        }
        Obj::Quote { q, inner } => {
            let s = ls(b, pos);
            let e = le(b, pos);
            let mut qs = Vec::new();
            let mut i = s;
            while i < e {
                if b[i] == '\\' {
                    i += 2;
                    continue;
                }
                if b[i] == q {
                    qs.push(i);
                }
                i += 1;
            }
            let pair = qs.chunks(2).filter(|c| c.len() == 2).find(|c| pos <= c[1]).filter(|c| {
                // Står vi före första citattecknet gäller det första paret efter markören.
                c[0] >= pos || pos <= c[1]
            })?;
            let (a, z) = (pair[0], pair[1]);
            if inner {
                return Some((a + 1, z, false));
            }
            let (mut sa, mut ez) = (a, z + 1);
            if ez < e && is_blank(b[ez]) {
                while ez < e && is_blank(b[ez]) {
                    ez += 1;
                }
            } else {
                while sa > s && is_blank(b[sa - 1]) {
                    sa -= 1;
                }
            }
            Some((sa, ez, false))
        }
        Obj::Pair { open, close, inner } => {
            if b.is_empty() {
                return None;
            }
            let pos = pos.min(b.len() - 1);
            // Den omslutande öppningsparentesen, `count` nivåer ut.
            let mut found = None;
            let mut depth = 0i32;
            let mut skip_from = pos;
            let mut need = count;
            let mut i = pos as isize;
            while i >= 0 {
                let u = i as usize;
                let c = b[u];
                if c == close && u != pos {
                    depth += 1;
                } else if c == open {
                    if depth == 0 {
                        need -= 1;
                        if need == 0 {
                            found = Some(u);
                            break;
                        }
                        skip_from = u;
                    } else {
                        depth -= 1;
                    }
                }
                i -= 1;
            }
            let _ = skip_from;
            let o = found?;
            let mut d = 0i32;
            let mut z = None;
            for (j, &c) in b.iter().enumerate().skip(o) {
                if c == open {
                    d += 1;
                } else if c == close {
                    d -= 1;
                    if d == 0 {
                        z = Some(j);
                        break;
                    }
                }
            }
            let z = z?;
            if !inner {
                return Some((o, z + 1, false));
            }
            let (mut s, mut e) = (o + 1, z);
            // Parentesen på egna rader: inre delen är raderna emellan.
            if s < e && b[s] == '\n' {
                s += 1;
                let cs = ls(b, e);
                if cs >= s && b[cs..e].iter().all(|&c| is_blank(c)) {
                    e = cs;
                }
            }
            Some((s, e, false))
        }
        Obj::Tag { inner } => {
            let tags = find_tags(b);
            let mut enclosing: Vec<&(usize, usize, usize, usize)> =
                tags.iter().filter(|t| t.0 <= pos && pos < t.3).collect();
            enclosing.sort_by_key(|t| std::cmp::Reverse(t.0));
            let t = enclosing.get(count - 1)?;
            Some(if inner { (t.1, t.2, false) } else { (t.0, t.3, false) })
        }
    }
}

/// Alla par av start- och sluttaggar som (öppning start, öppning slut, stängning start, stängning slut).
fn find_tags(b: &[char]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let mut stack: Vec<(String, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != '<' {
            i += 1;
            continue;
        }
        let Some(rel) = b[i..].iter().position(|&c| c == '>') else { break };
        let end = i + rel + 1;
        let inner: String = b[i + 1..end - 1].iter().collect();
        if inner.starts_with('!') || inner.starts_with('?') || inner.ends_with('/') || inner.is_empty() {
            i = end;
            continue;
        }
        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim().to_owned();
            if let Some(k) = stack.iter().rposition(|(n, _, _)| *n == name) {
                let (_, os, oe) = stack[k].clone();
                stack.truncate(k);
                out.push((os, oe, i, end));
            }
        } else {
            let name: String = inner.chars().take_while(|c| !c.is_whitespace()).collect();
            stack.push((name, i, end));
        }
        i = end;
    }
    out
}

// ------------------------------------------------------------------ kommandon

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Lower,
    Upper,
    Toggle,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mo {
    Left,
    Right,
    Down,
    Up,
    Word(bool),
    Back(bool),
    End(bool),
    EndBack(bool),
    LineStart,
    FirstNb,
    LineEnd,
    FirstLine,
    LastLine,
    Find { ch: char, fwd: bool, till: bool },
    RepFind(bool),
    Match,
    ParaFwd,
    ParaBack,
    Top,
    Mid,
    Bot,
    HalfDown,
    HalfUp,
    SearchNext(bool),
    Star(bool),
    Mark { ch: char, exact: bool },
    NbDown,
    NbUp,
    NbLine,
    Col,
    JumpBack,
    SearchPrompt(bool),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tgt {
    Mo(Mo),
    Obj(Obj),
    Lines,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum InsAt {
    Cursor,
    After,
    LineEnd,
    LineStartNb,
    Col0,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ac {
    Replace(char),
    Join { space: bool },
    Put { after: bool, cursor_end: bool },
    Undo,
    Redo,
    Dot,
    Open { below: bool },
    Insert(InsAt),
    Tilde,
    Visual(Mode),
    ReplaceMode,
    Record(char),
    StopRecord,
    Play(char),
    SetMark(char),
    CmdLine,
    SaveQuit,
    QuitForce,
    Reselect,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum VAc {
    /// Operator på markeringen; `true` = hela rader (`X`, `D`, `Y`, `C`, `S`, `R`).
    Op(Op, bool),
    Swap,
    SwapCorner,
    Join { space: bool },
    Replace(char),
    Put,
    BlockI,
    BlockA,
    Obj(Obj),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Move(Mo),
    Op(Op, Tgt),
    Act(Ac),
    Vis(VAc),
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Cmd {
    reg: Option<char>,
    count: Option<usize>,
    kind: Kind,
}

enum P<T> {
    Done(T),
    More,
    Bad,
}
use P::*;

fn digits(s: &[char], i: &mut usize) -> Option<usize> {
    let st = *i;
    let mut n: usize = 0;
    while let Some(c) = s.get(*i) {
        if c.is_ascii_digit() && (*i > st || *c != '0') {
            n = n.saturating_mul(10).saturating_add(c.to_digit(10).unwrap_or(0) as usize);
            *i += 1;
        } else {
            break;
        }
    }
    (*i > st).then_some(n.min(1_000_000))
}

fn valid_reg(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '"' | '+' | '*' | '_' | '-' | '.' | ':' | '/')
}

fn mul(a: Option<usize>, b: Option<usize>) -> Option<usize> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(1).saturating_mul(b.unwrap_or(1)).min(1_000_000)),
    }
}

fn parse_motion(s: &[char], i: usize) -> P<Mo> {
    let Some(&c) = s.get(i) else { return More };
    Done(match c {
        'h' | '\u{7f}' | '\u{e000}' => Mo::Left,
        'l' | ' ' | '\u{e001}' => Mo::Right,
        'j' | '\u{e003}' | '\u{e}' => Mo::Down,
        'k' | '\u{e002}' | '\u{10}' => Mo::Up,
        'w' => Mo::Word(false),
        'W' => Mo::Word(true),
        'b' => Mo::Back(false),
        'B' => Mo::Back(true),
        'e' => Mo::End(false),
        'E' => Mo::End(true),
        '0' | '\u{e004}' => Mo::LineStart,
        '^' => Mo::FirstNb,
        '$' | '\u{e005}' => Mo::LineEnd,
        'G' => Mo::LastLine,
        'f' | 'F' | 't' | 'T' => {
            let Some(&ch) = s.get(i + 1) else { return More };
            if ch == '\u{1b}' {
                return Bad;
            }
            Mo::Find { ch, fwd: c == 'f' || c == 't', till: c == 't' || c == 'T' }
        }
        ';' => Mo::RepFind(false),
        ',' => Mo::RepFind(true),
        '%' => Mo::Match,
        '}' => Mo::ParaFwd,
        '{' => Mo::ParaBack,
        'H' => Mo::Top,
        'M' => Mo::Mid,
        'L' => Mo::Bot,
        '\u{4}' => Mo::HalfDown,
        '\u{15}' => Mo::HalfUp,
        '\u{f}' => Mo::JumpBack,
        'n' => Mo::SearchNext(false),
        'N' => Mo::SearchNext(true),
        '*' => Mo::Star(true),
        '#' => Mo::Star(false),
        '\'' | '`' => {
            let Some(&m) = s.get(i + 1) else { return More };
            Mo::Mark { ch: m, exact: c == '`' }
        }
        '+' | '\n' => Mo::NbDown,
        '-' => Mo::NbUp,
        '_' => Mo::NbLine,
        '|' => Mo::Col,
        '/' => Mo::SearchPrompt(true),
        '?' => Mo::SearchPrompt(false),
        'g' => {
            let Some(&d) = s.get(i + 1) else { return More };
            match d {
                'g' => Mo::FirstLine,
                'e' => Mo::EndBack(false),
                'E' => Mo::EndBack(true),
                'j' => Mo::Down,
                'k' => Mo::Up,
                '0' => Mo::LineStart,
                '^' => Mo::FirstNb,
                '$' => Mo::LineEnd,
                _ => return Bad,
            }
        }
        _ => return Bad,
    })
}

fn op_of(c: char) -> Option<Op> {
    Some(match c {
        'd' => Op::Delete,
        'c' => Op::Change,
        'y' => Op::Yank,
        '>' => Op::Indent,
        '<' => Op::Outdent,
        _ => return None,
    })
}

fn gop_of(c: char) -> Option<Op> {
    Some(match c {
        'u' => Op::Lower,
        'U' => Op::Upper,
        '~' => Op::Toggle,
        _ => return None,
    })
}

/// Operator följd av antal och mål (`dw`, `d2w`, `dd`, `di"`, `gUU` …).
fn parse_op(s: &[char], i: usize, reg: Option<char>, count: Option<usize>, op: Op, opch: char) -> P<Cmd> {
    let mut i = i;
    let c2 = digits(s, &mut i);
    let count = mul(count, c2);
    let Some(&t) = s.get(i) else { return More };
    let mk = |tgt| Done(Cmd { reg, count, kind: Kind::Op(op, tgt) });
    if t == opch {
        return mk(Tgt::Lines);
    }
    if t == 'g' && gop_of(opch).is_some() {
        match s.get(i + 1) {
            None => return More,
            Some(&x) if x == opch => return mk(Tgt::Lines),
            _ => {}
        }
    }
    if t == 'i' || t == 'a' {
        let Some(&o) = s.get(i + 1) else { return More };
        return match obj_of(o, t == 'i') {
            Some(o) => mk(Tgt::Obj(o)),
            None => Bad,
        };
    }
    match parse_motion(s, i) {
        Done(m) => mk(Tgt::Mo(m)),
        More => More,
        Bad => Bad,
    }
}

fn parse(s: &[char], visual: bool, recording: bool) -> P<Cmd> {
    let mut i = 0;
    let mut reg = None;
    let mut count = None;
    loop {
        match s.get(i) {
            Some('"') => match s.get(i + 1) {
                None => return More,
                Some(&r) if valid_reg(r) => {
                    reg = Some(r);
                    i += 2;
                }
                Some(_) => return Bad,
            },
            Some(c) if c.is_ascii_digit() && *c != '0' => count = digits(s, &mut i),
            _ => break,
        }
    }
    let Some(&c) = s.get(i) else { return More };
    i += 1;
    let done = |kind| Done(Cmd { reg, count, kind });
    let op_mo = |op, mo| done(Kind::Op(op, Tgt::Mo(mo)));
    if visual {
        return parse_visual(s, i, c, reg, count);
    }
    if let Some(op) = op_of(c) {
        return parse_op(s, i, reg, count, op, c);
    }
    match c {
        'g' => {
            let Some(&d) = s.get(i) else { return More };
            if let Some(op) = gop_of(d) {
                return parse_op(s, i + 1, reg, count, op, d);
            }
            match d {
                'J' => done(Kind::Act(Ac::Join { space: false })),
                'v' => done(Kind::Act(Ac::Reselect)),
                'I' => done(Kind::Act(Ac::Insert(InsAt::Col0))),
                'p' => done(Kind::Act(Ac::Put { after: true, cursor_end: true })),
                'P' => done(Kind::Act(Ac::Put { after: false, cursor_end: true })),
                _ => match parse_motion(s, i - 1) {
                    Done(m) => done(Kind::Move(m)),
                    More => More,
                    Bad => Bad,
                },
            }
        }
        'x' => op_mo(Op::Delete, Mo::Right),
        'X' => op_mo(Op::Delete, Mo::Left),
        's' => op_mo(Op::Change, Mo::Right),
        'S' => done(Kind::Op(Op::Change, Tgt::Lines)),
        'D' => op_mo(Op::Delete, Mo::LineEnd),
        'C' => op_mo(Op::Change, Mo::LineEnd),
        'Y' => done(Kind::Op(Op::Yank, Tgt::Lines)),
        '\u{e006}' => op_mo(Op::Delete, Mo::Right),
        'r' => match s.get(i) {
            None => More,
            Some('\u{1b}') => Bad,
            Some(&ch) => done(Kind::Act(Ac::Replace(ch))),
        },
        'J' => done(Kind::Act(Ac::Join { space: true })),
        'p' => done(Kind::Act(Ac::Put { after: true, cursor_end: false })),
        'P' => done(Kind::Act(Ac::Put { after: false, cursor_end: false })),
        'u' => done(Kind::Act(Ac::Undo)),
        '\u{12}' => done(Kind::Act(Ac::Redo)),
        '.' => done(Kind::Act(Ac::Dot)),
        'o' => done(Kind::Act(Ac::Open { below: true })),
        'O' => done(Kind::Act(Ac::Open { below: false })),
        'i' => done(Kind::Act(Ac::Insert(InsAt::Cursor))),
        'a' => done(Kind::Act(Ac::Insert(InsAt::After))),
        'A' => done(Kind::Act(Ac::Insert(InsAt::LineEnd))),
        'I' => done(Kind::Act(Ac::Insert(InsAt::LineStartNb))),
        '~' => done(Kind::Act(Ac::Tilde)),
        'v' => done(Kind::Act(Ac::Visual(Mode::Visual))),
        'V' => done(Kind::Act(Ac::Visual(Mode::VisualLine))),
        '\u{16}' => done(Kind::Act(Ac::Visual(Mode::VisualBlock))),
        'R' => done(Kind::Act(Ac::ReplaceMode)),
        'q' if recording => done(Kind::Act(Ac::StopRecord)),
        'q' => match s.get(i) {
            None => More,
            Some(&r) if r.is_ascii_alphanumeric() || r == '"' => done(Kind::Act(Ac::Record(r))),
            Some(_) => Bad,
        },
        '@' => match s.get(i) {
            None => More,
            Some(&r) if r.is_ascii_alphanumeric() || r == '@' || r == ':' => done(Kind::Act(Ac::Play(r))),
            Some(_) => Bad,
        },
        'm' => match s.get(i) {
            None => More,
            Some(&r) if r.is_ascii_alphabetic() || r == '\'' || r == '`' => done(Kind::Act(Ac::SetMark(r))),
            Some(_) => Bad,
        },
        ':' => done(Kind::Act(Ac::CmdLine)),
        'Z' => match s.get(i) {
            None => More,
            Some('Z') => done(Kind::Act(Ac::SaveQuit)),
            Some('Q') => done(Kind::Act(Ac::QuitForce)),
            Some(_) => Bad,
        },
        _ => match parse_motion(s, i - 1) {
            Done(m) => done(Kind::Move(m)),
            More => More,
            Bad => Bad,
        },
    }
}

fn parse_visual(s: &[char], i: usize, c: char, reg: Option<char>, count: Option<usize>) -> P<Cmd> {
    let done = |kind| Done(Cmd { reg, count, kind });
    match c {
        'd' | 'x' | '\u{e006}' => done(Kind::Vis(VAc::Op(Op::Delete, false))),
        'X' | 'D' => done(Kind::Vis(VAc::Op(Op::Delete, true))),
        'y' => done(Kind::Vis(VAc::Op(Op::Yank, false))),
        'Y' => done(Kind::Vis(VAc::Op(Op::Yank, true))),
        'c' | 's' => done(Kind::Vis(VAc::Op(Op::Change, false))),
        'C' | 'S' | 'R' => done(Kind::Vis(VAc::Op(Op::Change, true))),
        '>' => done(Kind::Vis(VAc::Op(Op::Indent, true))),
        '<' => done(Kind::Vis(VAc::Op(Op::Outdent, true))),
        'u' => done(Kind::Vis(VAc::Op(Op::Lower, false))),
        'U' => done(Kind::Vis(VAc::Op(Op::Upper, false))),
        '~' => done(Kind::Vis(VAc::Op(Op::Toggle, false))),
        'J' => done(Kind::Vis(VAc::Join { space: true })),
        'r' => match s.get(i) {
            None => More,
            Some('\u{1b}') => Bad,
            Some(&ch) => done(Kind::Vis(VAc::Replace(ch))),
        },
        'p' | 'P' => done(Kind::Vis(VAc::Put)),
        'I' => done(Kind::Vis(VAc::BlockI)),
        'A' => done(Kind::Vis(VAc::BlockA)),
        'o' => done(Kind::Vis(VAc::Swap)),
        'O' => done(Kind::Vis(VAc::SwapCorner)),
        'v' => done(Kind::Act(Ac::Visual(Mode::Visual))),
        'V' => done(Kind::Act(Ac::Visual(Mode::VisualLine))),
        '\u{16}' => done(Kind::Act(Ac::Visual(Mode::VisualBlock))),
        ':' => done(Kind::Act(Ac::CmdLine)),
        'i' | 'a' => {
            let Some(&o) = s.get(i) else { return More };
            match obj_of(o, c == 'i') {
                Some(o) => done(Kind::Vis(VAc::Obj(o))),
                None => Bad,
            }
        }
        'g' => {
            let Some(&d) = s.get(i) else { return More };
            match d {
                'u' => done(Kind::Vis(VAc::Op(Op::Lower, false))),
                'U' => done(Kind::Vis(VAc::Op(Op::Upper, false))),
                '~' => done(Kind::Vis(VAc::Op(Op::Toggle, false))),
                'J' => done(Kind::Vis(VAc::Join { space: false })),
                'v' => done(Kind::Act(Ac::Reselect)),
                _ => match parse_motion(s, i - 1) {
                    Done(m) => done(Kind::Move(m)),
                    More => More,
                    Bad => Bad,
                },
            }
        }
        _ => match parse_motion(s, i - 1) {
            Done(m) => done(Kind::Move(m)),
            More => More,
            Bad => Bad,
        },
    }
}

// ------------------------------------------------------------------ register och tillstånd

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegKind {
    Char,
    Line,
    Block,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Reg {
    pub text: String,
    pub kind: RegKind,
}

/// Ett område som ett kommando verkar på.
#[derive(Clone, Debug)]
enum Region {
    Chars(usize, usize),
    /// Rader, båda inklusive.
    Lines(usize, usize),
    /// En (start, slut) per rad, vänsterkolumn och översta raden.
    Block { rows: Vec<(usize, usize)>, col: usize, top: usize },
}

struct Mr {
    pos: usize,
    lw: bool,
    incl: bool,
}

#[derive(Clone)]
struct Snap {
    text: String,
    cur: usize,
}

/// Formen på en visuell markering, för att kunna upprepa kommandot med `.`.
#[derive(Clone, Copy, Debug)]
struct VisShape {
    mode: Mode,
    lines: usize,
    width: usize,
}

#[derive(Clone)]
struct Change {
    cmd: Cmd,
    ins: String,
    replace: bool,
    vis: Option<VisShape>,
}

struct BlockIns {
    top: usize,
    bot: usize,
    col: usize,
    append: bool,
    eol: bool,
}

struct Ins {
    count: usize,
    typed: String,
    replace: bool,
    replaced: Vec<Option<char>>,
    block: Option<BlockIns>,
    open: Option<bool>,
    replaying: bool,
}

#[derive(Clone, Copy)]
struct PendingOp {
    reg: Option<char>,
    op: Op,
    count: Option<usize>,
}

pub struct Vim {
    pub mode: Mode,
    /// Markören (teckenindex). Anroparen sätter den före varje tangent och läser den efter.
    pub cur: usize,
    /// Den andra änden av den visuella markeringen.
    pub anchor: usize,
    /// `$` i blockläget: markeringen går till slutet av varje rad.
    block_dollar: bool,
    seq: Vec<char>,
    pub cmdline: String,
    cmd_kind: char,
    pending_op: Option<PendingOp>,
    regs: HashMap<char, Reg>,
    marks: HashMap<char, usize>,
    last_find: Option<(char, bool, bool)>,
    pub last_search: Option<(String, bool)>,
    last_sub: Option<(String, String, String)>,
    last_ex: Option<String>,
    last_change: Option<Change>,
    insert: Option<Ins>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    recording: Option<(char, Vec<char>)>,
    last_macro: Option<char>,
    macro_depth: usize,
    want_col: Option<usize>,
    last_visual: Option<(Mode, usize, usize)>,
    history: Vec<String>,
    hist_at: Option<usize>,
    sw: usize,
    ignorecase: bool,
    dirty: bool,
    failed: bool,
    ret_mode: Mode,
    fx: Vec<Effect>,
    /// Hämtar systemets urklipp för `"+p`. Ingen i testerna: då används registret `+`.
    pub clip_get: Option<fn() -> Option<String>>,
}

impl Default for Vim {
    fn default() -> Self {
        Vim {
            mode: Mode::Normal,
            cur: 0,
            anchor: 0,
            block_dollar: false,
            seq: Vec::new(),
            cmdline: String::new(),
            cmd_kind: ':',
            pending_op: None,
            regs: HashMap::new(),
            marks: HashMap::new(),
            last_find: None,
            last_search: None,
            last_sub: None,
            last_ex: None,
            last_change: None,
            insert: None,
            undo: Vec::new(),
            redo: Vec::new(),
            recording: None,
            last_macro: None,
            macro_depth: 0,
            want_col: None,
            last_visual: None,
            history: Vec::new(),
            hist_at: None,
            sw: 4,
            ignorecase: false,
            dirty: false,
            failed: false,
            ret_mode: Mode::Normal,
            fx: Vec::new(),
            clip_get: None,
        }
    }
}

const UNDO_MAX: usize = 300;

impl Vim {
    pub fn new() -> Self {
        Self::default()
    }

    /// Nytt dokument: läget, ångra och markeringar hör till dokumentet, register och sökning följer med.
    pub fn switch_doc(&mut self) {
        self.mode = Mode::Normal;
        self.seq.clear();
        self.insert = None;
        self.undo.clear();
        self.redo.clear();
        self.marks.clear();
        self.pending_op = None;
        self.cmdline.clear();
        self.want_col = None;
    }

    /// Markören flyttades utifrån (musen): lägg den på en giltig plats för läget.
    pub fn adopt(&mut self, text: &str) {
        if matches!(self.mode, Mode::Insert | Mode::Replace | Mode::Command) {
            return;
        }
        let b: Vec<char> = text.chars().collect();
        self.cur = clamp_normal(&b, self.cur);
        self.want_col = None;
    }

    #[cfg(test)]
    pub fn register(&self, name: char) -> Option<&Reg> {
        self.regs.get(&name)
    }

    pub fn pending(&self) -> String {
        self.seq.iter().map(|&c| display_key(c)).collect()
    }

    pub fn recording_reg(&self) -> Option<char> {
        self.recording.as_ref().map(|r| r.0)
    }

    /// Texten i statusraden för läget ("-- INFOGA --" …); tom i normalläget.
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            Mode::Normal | Mode::Command => "",
            Mode::Insert => t("-- INFOGA --"),
            Mode::Visual => t("-- MARKERA --"),
            Mode::VisualLine => t("-- MARKERA RAD --"),
            Mode::VisualBlock => t("-- MARKERA BLOCK --"),
            Mode::Replace => t("-- ERSÄTT --"),
        }
    }

    /// Områden som är markerade (teckenindex, slut exklusive), för ritning. Tom utanför visuellt läge.
    pub fn selection_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        if !self.mode.is_visual() {
            return Vec::new();
        }
        let b: Vec<char> = text.chars().collect();
        match self.region(&b) {
            Region::Chars(s, e) => vec![(s, e)],
            Region::Lines(l1, l2) => {
                let s = line_pos(&b, l1);
                vec![(s, (le(&b, line_pos(&b, l2)) + 1).min(b.len() + 1))]
            }
            Region::Block { rows, .. } => rows,
        }
    }

    // ---------------------------------------------------------------- ingång

    /// Hanterar en tangent. `text` och `self.cur` ändras på plats.
    pub fn key(&mut self, k: Key, text: &mut String, view: View) -> Out {
        let mut b: Vec<char> = text.chars().collect();
        self.dirty = false;
        self.fx.clear();
        self.cur = self.cur.min(b.len());
        if let Some((_, rec)) = &mut self.recording {
            rec.push(enc(k));
        }
        self.handle(k, &mut b, view);
        if !matches!(self.mode, Mode::Insert | Mode::Replace | Mode::Command) {
            self.cur = clamp_normal(&b, self.cur);
        }
        self.cur = self.cur.min(b.len());
        if self.dirty {
            *text = b.iter().collect();
        }
        Out { edited: self.dirty, effects: std::mem::take(&mut self.fx) }
    }

    #[cfg(test)]
    /// Hjälp för tester: skickar en hel sträng tangent för tangent.
    pub fn keys(&mut self, s: &str, text: &mut String) -> Out {
        let mut all = Out::default();
        for c in s.chars() {
            let o = self.key(dec(c), text, View::default());
            all.edited |= o.edited;
            all.effects.extend(o.effects);
        }
        all
    }

    fn handle(&mut self, k: Key, b: &mut Vec<char>, view: View) {
        match self.mode {
            Mode::Insert => self.insert_key(k, b),
            Mode::Replace => self.replace_key(k, b),
            Mode::Command => self.cmd_key(k, b, view),
            _ => self.normal_key(k, b, view),
        }
    }

    fn normal_key(&mut self, k: Key, b: &mut Vec<char>, view: View) {
        let c = enc(k);
        if c == '\u{1b}' {
            if self.seq.is_empty() && self.mode.is_visual() {
                self.exit_visual();
            }
            self.seq.clear();
            return;
        }
        self.seq.push(c);
        match parse(&self.seq, self.mode.is_visual(), self.recording.is_some()) {
            More => {}
            Bad => self.seq.clear(),
            Done(cmd) => {
                self.seq.clear();
                self.exec(b, cmd, view, false);
            }
        }
    }

    // ---------------------------------------------------------------- hjälpare för redigering

    fn splice(&mut self, b: &mut Vec<char>, s: usize, e: usize, ins: &[char]) {
        let (s, e) = (s.min(b.len()), e.min(b.len()));
        if s == e && ins.is_empty() {
            return;
        }
        b.splice(s..e, ins.iter().copied());
        self.dirty = true;
    }

    fn snap(&mut self, b: &[char]) {
        self.undo.push(Snap { text: b.iter().collect(), cur: self.cur });
        if self.undo.len() > UNDO_MAX {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn notify(&mut self, msg: String, err: bool) {
        self.fx.push(Effect::Notify(msg, err));
    }

    fn exit_visual(&mut self) {
        if self.mode.is_visual() {
            self.last_visual = Some((self.mode, self.anchor, self.cur));
            self.marks.insert('<', self.anchor.min(self.cur));
            self.marks.insert('>', self.anchor.max(self.cur));
        }
        self.mode = Mode::Normal;
        self.block_dollar = false;
    }

    // ---------------------------------------------------------------- register

    fn get_reg(&self, name: Option<char>) -> Option<Reg> {
        let c = name.unwrap_or('"');
        match c {
            '_' => None,
            '+' | '*' => self
                .clip_get
                .and_then(|f| f())
                .map(|text| {
                    let kind = match self.regs.get(&'+') {
                        Some(r) if r.text == text => r.kind,
                        _ => RegKind::Char,
                    };
                    Reg { text, kind }
                })
                .or_else(|| self.regs.get(&'+').cloned()),
            '/' => self.last_search.as_ref().map(|s| Reg { text: s.0.clone(), kind: RegKind::Char }),
            ':' => self.last_ex.as_ref().map(|s| Reg { text: s.clone(), kind: RegKind::Char }),
            c => self.regs.get(&c.to_ascii_lowercase()).cloned(),
        }
    }

    fn store(&mut self, name: Option<char>, text: String, kind: RegKind, del: bool) {
        let r = Reg { text, kind };
        match name {
            Some('_') => return,
            Some(c) if c.is_ascii_alphabetic() => {
                let lc = c.to_ascii_lowercase();
                let new = match self.regs.get(&lc) {
                    Some(old) if c.is_ascii_uppercase() => {
                        let sep = if old.kind != RegKind::Line && r.kind == RegKind::Line { "\n" } else { "" };
                        Reg {
                            text: format!("{}{sep}{}", old.text, r.text),
                            kind: if r.kind == RegKind::Line || old.kind == RegKind::Line { RegKind::Line } else { old.kind },
                        }
                    }
                    _ => r,
                };
                self.regs.insert(lc, new.clone());
                self.regs.insert('"', new);
                return;
            }
            Some('+') | Some('*') => {
                self.fx.push(Effect::Clipboard(r.text.clone()));
                self.regs.insert('+', r.clone());
                self.regs.insert('"', r);
                return;
            }
            _ => {}
        }
        self.regs.insert('"', r.clone());
        if del {
            if r.kind == RegKind::Line || r.text.contains('\n') {
                for i in (1..9).rev() {
                    let from = char::from_digit(i, 10).unwrap_or('1');
                    let to = char::from_digit(i + 1, 10).unwrap_or('9');
                    if let Some(x) = self.regs.get(&from).cloned() {
                        self.regs.insert(to, x);
                    }
                }
                self.regs.insert('1', r);
            } else {
                self.regs.insert('-', r);
            }
        } else {
            self.regs.insert('0', r);
        }
    }

    // ---------------------------------------------------------------- rörelser

    fn find_char(&self, b: &[char], pos: usize, ch: char, fwd: bool, till: bool, n: usize, repeat: bool) -> Option<usize> {
        let (s, e) = (ls(b, pos), le(b, pos));
        let mut p = pos;
        if repeat && till {
            // `;` efter `t` hoppar över tecknet som står precis intill.
            if fwd && p + 1 < e && b[p + 1] == ch {
                p += 1;
            } else if !fwd && p > s && b[p - 1] == ch {
                p -= 1;
            }
        }
        let mut left = n;
        if fwd {
            let mut i = p + 1;
            while i < e {
                if b[i] == ch {
                    left -= 1;
                    if left == 0 {
                        return Some(if till { i - 1 } else { i });
                    }
                }
                i += 1;
            }
        } else {
            let mut i = p;
            while i > s {
                i -= 1;
                if b[i] == ch {
                    left -= 1;
                    if left == 0 {
                        return Some(if till { i + 1 } else { i });
                    }
                }
            }
        }
        None
    }

    fn build_regex(&mut self, pat: &str) -> Option<Regex> {
        match RegexBuilder::new(pat).multi_line(true).case_insensitive(self.ignorecase).build() {
            Ok(r) => Some(r),
            Err(e) => {
                let msg = tf!("E383: Ogiltigt sökmönster: {e}", e = e.to_string().lines().last().unwrap_or("").trim());
                self.notify(msg, true);
                None
            }
        }
    }

    /// Nästa träff `n` steg från `from` (runt dokumentet). Ger teckenindex.
    fn search(&mut self, b: &[char], from: usize, pat: &str, fwd: bool, n: usize) -> Option<usize> {
        let re = self.build_regex(pat)?;
        let s: String = b.iter().collect();
        let offs: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
        let to_char = |byte: usize| offs.partition_point(|&o| o < byte);
        let starts: Vec<usize> = re.find_iter(&s).map(|m| to_char(m.start())).collect();
        if starts.is_empty() {
            self.notify(tf!("E486: Hittade inte mönstret: {pat}", pat = pat), true);
            return None;
        }
        let len = starts.len();
        let idx = if fwd {
            (starts.partition_point(|&x| x <= from) + n - 1) % len
        } else {
            let before = starts.partition_point(|&x| x < from);
            (before + len * n - n) % len
        };
        Some(starts[idx])
    }

    fn word_under(&self, b: &[char], pos: usize) -> Option<(usize, usize)> {
        let e = le(b, pos);
        let isw = |c: char| c.is_alphanumeric() || c == '_';
        let mut p = (pos..e).find(|&i| isw(b[i]))?;
        // Står markören på ett ord räknas det, annars nästa ord på raden.
        if pos < e && isw(b[pos]) {
            p = pos;
        }
        let mut s = p;
        while s > ls(b, p) && isw(b[s - 1]) {
            s -= 1;
        }
        let mut z = p;
        while z < e && isw(b[z]) {
            z += 1;
        }
        Some((s, z))
    }

    fn motion(&mut self, b: &[char], m: Mo, count: Option<usize>, op: bool, view: View) -> Option<Mr> {
        let n = count.unwrap_or(1).max(1);
        let cur = self.cur.min(b.len());
        let (s, e) = (ls(b, cur), le(b, cur));
        let lines = nlines(b);
        let cl = line_idx(b, cur);
        let ch = |pos| Some(Mr { pos, lw: false, incl: false });
        let chi = |pos| Some(Mr { pos, lw: false, incl: true });
        let lwm = |line: usize, b: &[char]| Some(Mr { pos: first_nb(b, line_pos(b, line)), lw: true, incl: false });
        match m {
            Mo::Left => (cur > s).then(|| Mr { pos: cur.saturating_sub(n).max(s), lw: false, incl: false }),
            Mo::Right => {
                let lim = if op { e } else { e.saturating_sub(1).max(s) };
                if cur >= lim { None } else { ch((cur + n).min(lim)) }
            }
            Mo::Down | Mo::Up => {
                let down = m == Mo::Down;
                let target = if down {
                    if cl + 1 >= lines {
                        return None;
                    }
                    (cl + n).min(lines - 1)
                } else {
                    if cl == 0 {
                        return None;
                    }
                    cl.saturating_sub(n)
                };
                let w = self.want_col.unwrap_or(cur - s);
                self.want_col = Some(w);
                let ts = line_pos(b, target);
                let te = le(b, ts);
                let pos = if w == usize::MAX { te.saturating_sub(1).max(ts) } else { ts + w.min((te - ts).saturating_sub(if op { 0 } else { 1 })) };
                Some(Mr { pos, lw: true, incl: false })
            }
            Mo::Word(big) => {
                let mut p = cur;
                for i in 0..n {
                    p = word_fwd(b, p, big, op && i + 1 == n);
                }
                (p != cur || op).then(|| Mr { pos: if op { p } else { p.min(b.len()) }, lw: false, incl: false })
            }
            Mo::Back(big) => {
                let mut p = cur;
                for _ in 0..n {
                    p = word_back(b, p, big);
                }
                (p != cur).then_some(Mr { pos: p, lw: false, incl: false })
            }
            Mo::End(big) => {
                let mut p = cur;
                for _ in 0..n {
                    p = word_end(b, p, big);
                }
                (p != cur).then_some(Mr { pos: p, lw: false, incl: true })
            }
            Mo::EndBack(big) => {
                let mut p = cur;
                for _ in 0..n {
                    p = word_end_back(b, p, big);
                }
                (p != cur).then_some(Mr { pos: p, lw: false, incl: true })
            }
            Mo::LineStart => ch(s),
            Mo::FirstNb => ch(first_nb(b, s).min(e)),
            Mo::LineEnd => {
                let mut l = cl;
                if n > 1 {
                    l = (cl + n - 1).min(lines - 1);
                    if cl + n - 1 >= lines {
                        return None;
                    }
                }
                let st = line_pos(b, l);
                let en = le(b, st);
                self.want_col = Some(usize::MAX);
                if en == st { ch(en) } else { chi(en - 1) }
            }
            Mo::FirstLine => lwm(count.map_or(0, |c| c - 1).min(lines - 1), b),
            Mo::LastLine => lwm(count.map_or(lines - 1, |c| c - 1).min(lines - 1), b),
            Mo::Find { ch: c, fwd, till } => {
                self.last_find = Some((c, fwd, till));
                self.find_char(b, cur, c, fwd, till, n, false).map(|pos| Mr { pos, lw: false, incl: fwd })
            }
            Mo::RepFind(rev) => {
                let (c, fwd, till) = self.last_find?;
                let fwd = fwd != rev;
                self.find_char(b, cur, c, fwd, till, n, true).map(|pos| Mr { pos, lw: false, incl: fwd })
            }
            Mo::Match => {
                if let Some(c) = count {
                    let l = ((c.min(100) * lines).div_ceil(100)).max(1) - 1;
                    return lwm(l.min(lines - 1), b);
                }
                match_pair(b, cur).map(|pos| Mr { pos, lw: false, incl: true })
            }
            Mo::ParaFwd => {
                let mut p = cur;
                for _ in 0..n {
                    p = para_fwd(b, p);
                }
                if p < b.len() {
                    ch(p)
                } else if !b.is_empty() && b[b.len() - 1] == '\n' {
                    ch(b.len())
                } else {
                    chi(b.len().saturating_sub(1))
                }
            }
            Mo::ParaBack => {
                let mut p = cur;
                for _ in 0..n {
                    p = para_back(b, p);
                }
                ch(p)
            }
            Mo::Top | Mo::Mid | Mo::Bot => {
                let vl = view.lines.max(1);
                let top = view.top.min(lines - 1);
                let bot = (view.top + vl - 1).min(lines - 1);
                let l = match m {
                    Mo::Top => (top + n - 1).min(bot),
                    Mo::Mid => top + (bot - top) / 2,
                    _ => bot.saturating_sub(n - 1).max(top),
                };
                lwm(l, b)
            }
            Mo::HalfDown | Mo::HalfUp => {
                let step = count.unwrap_or((view.lines / 2).max(1));
                let down = m == Mo::HalfDown;
                let target = if down { (cl + step).min(lines - 1) } else { cl.saturating_sub(step) };
                if target == cl {
                    return None;
                }
                self.fx.push(Effect::Scroll(if down { step as i32 } else { -(step as i32) }));
                let w = self.want_col.unwrap_or(cur - s);
                self.want_col = Some(w);
                Some(Mr { pos: at_col(b, target, w), lw: true, incl: false })
            }
            Mo::SearchNext(rev) => {
                let (pat, fwd) = self.last_search.clone()?;
                let fwd = fwd != rev;
                let pos = self.search(b, cur, &pat, fwd, n)?;
                ch(pos)
            }
            Mo::Star(fwd) => {
                let (ws, we) = self.word_under(b, cur)?;
                let word: String = b[ws..we].iter().collect();
                let pat = format!(r"\b{}\b", regex::escape(&word));
                self.last_search = Some((pat.clone(), fwd));
                let pos = self.search(b, if fwd { cur } else { ws }, &pat, fwd, n)?;
                ch(pos)
            }
            Mo::Mark { ch: c, exact } => {
                let &pos = self.marks.get(&if c == '`' { '\'' } else { c })?;
                let pos = pos.min(b.len());
                if exact {
                    ch(pos)
                } else {
                    lwm(line_idx(b, pos), b)
                }
            }
            Mo::NbDown => {
                if cl + n >= lines {
                    return None;
                }
                lwm(cl + n, b)
            }
            Mo::NbUp => {
                if cl < n {
                    return None;
                }
                lwm(cl - n, b)
            }
            Mo::NbLine => {
                if cl + n - 1 >= lines {
                    return None;
                }
                lwm(cl + n - 1, b)
            }
            Mo::Col => ch(s + (count.unwrap_or(1) - 1).min((e - s).saturating_sub(1))),
            Mo::JumpBack => {
                let &p = self.marks.get(&'\'')?;
                ch(p.min(b.len()))
            }
            Mo::SearchPrompt(_) => None,
        }
    }

    fn do_move(&mut self, b: &mut Vec<char>, m: Mo, count: Option<usize>, view: View) {
        if let Mo::SearchPrompt(fwd) = m {
            self.begin_cmdline(if fwd { '/' } else { '?' }, "");
            return;
        }
        let before = self.cur;
        let keep = matches!(m, Mo::Down | Mo::Up | Mo::HalfDown | Mo::HalfUp);
        if !keep {
            self.want_col = None;
        }
        let Some(mr) = self.motion(b, m, count, false, view) else {
            self.failed = true;
            return;
        };
        {
            let jump = matches!(
                m,
                Mo::FirstLine | Mo::LastLine | Mo::Match | Mo::SearchNext(_) | Mo::Star(_) | Mo::Mark { .. } | Mo::ParaFwd | Mo::ParaBack | Mo::Top | Mo::Mid | Mo::Bot
            );
            if jump {
                self.marks.insert('\'', before);
            }
            if m == Mo::JumpBack {
                self.marks.insert('\'', before);
            }
            self.cur = mr.pos;
            if m == Mo::LineEnd {
                self.want_col = Some(usize::MAX);
            }
        }
    }

    fn begin_cmdline(&mut self, kind: char, prefill: &str) {
        self.ret_mode = if self.mode.is_visual() { self.mode } else { Mode::Normal };
        self.mode = Mode::Command;
        self.cmd_kind = kind;
        self.cmdline = prefill.to_owned();
        self.hist_at = None;
    }

    // ---------------------------------------------------------------- områden

    /// Området för det som är markerat just nu.
    fn region(&self, b: &[char]) -> Region {
        let (a, c) = (self.anchor.min(b.len()), self.cur.min(b.len()));
        match self.mode {
            Mode::VisualLine => Region::Lines(line_idx(b, a.min(c)), line_idx(b, a.max(c))),
            Mode::VisualBlock => {
                let (la, lc) = (line_idx(b, a), line_idx(b, c));
                let (ca, cc) = (col_of(b, a), col_of(b, c));
                let (c1, c2) = (ca.min(cc), ca.max(cc));
                let (l1, l2) = (la.min(lc), la.max(lc));
                let mut rows = Vec::new();
                for l in l1..=l2 {
                    let st = line_pos(b, l);
                    let en = le(b, st);
                    let len = en - st;
                    let r = if self.block_dollar { (st + c1.min(len), en) } else { (st + c1.min(len), st + (c2 + 1).min(len)) };
                    rows.push(r);
                }
                Region::Block { rows, col: c1, top: l1 }
            }
            _ => {
                let s = a.min(c);
                let e = (a.max(c) + 1).min(b.len());
                Region::Chars(s, e)
            }
        }
    }

    fn lines_range(b: &[char], l1: usize, l2: usize) -> (usize, usize) {
        let s = line_pos(b, l1);
        let e = (le(b, line_pos(b, l2)) + 1).min(b.len());
        (s, e)
    }

    fn region_text(b: &[char], r: &Region) -> (String, RegKind) {
        match r {
            Region::Chars(s, e) => (b[*s..*e].iter().collect(), RegKind::Char),
            Region::Lines(l1, l2) => {
                let (s, e) = Self::lines_range(b, *l1, *l2);
                let mut t: String = b[s..e].iter().collect();
                if !t.ends_with('\n') {
                    t.push('\n');
                }
                (t, RegKind::Line)
            }
            Region::Block { rows, .. } => {
                (rows.iter().map(|&(s, e)| b[s..e].iter().collect::<String>()).collect::<Vec<_>>().join("\n"), RegKind::Block)
            }
        }
    }

    // ---------------------------------------------------------------- operatorer

    fn apply_op(&mut self, b: &mut Vec<char>, op: Op, reg: Option<char>, r: Region, amount: usize) {
        match op {
            Op::Yank => {
                let (text, kind) = Self::region_text(b, &r);
                self.store(reg, text, kind, false);
                let cl = line_idx(b, self.cur);
                self.cur = match &r {
                    Region::Chars(s, _) => *s,
                    Region::Lines(l1, _) => {
                        if *l1 < cl { at_col(b, *l1, col_of(b, self.cur)) } else { self.cur }
                    }
                    Region::Block { rows, .. } => rows.first().map_or(self.cur, |x| x.0),
                };
            }
            Op::Delete | Op::Change => {
                let change = op == Op::Change;
                let (text, kind) = Self::region_text(b, &r);
                self.store(reg, text, kind, true);
                match r {
                    Region::Chars(s, e) => {
                        self.splice(b, s, e, &[]);
                        self.cur = s;
                    }
                    Region::Lines(l1, l2) => {
                        let (s, e) = Self::lines_range(b, l1, l2);
                        if change {
                            let e2 = le(b, line_pos(b, l2));
                            self.splice(b, s, e2, &[]);
                            self.cur = s;
                        } else {
                            let s2 = if e >= b.len() && s > 0 { s - 1 } else { s };
                            self.splice(b, s2, e, &[]);
                            let l = line_idx(b, s2.min(b.len())).min(nlines(b) - 1);
                            let l = if e >= b.len() && s > 0 { line_idx(b, s2) } else { l.min(nlines(b) - 1) };
                            self.cur = first_nb(b, line_pos(b, l));
                        }
                    }
                    Region::Block { rows, col, top } => {
                        for &(s, e) in rows.iter().rev() {
                            self.splice(b, s, e, &[]);
                        }
                        self.cur = at_col(b, top, col);
                        if change {
                            let bot = top + rows.len() - 1;
                            self.insert = None;
                            self.start_insert(1, false);
                            if let Some(i) = &mut self.insert {
                                i.block = Some(BlockIns { top, bot, col, append: false, eol: false });
                            }
                        }
                    }
                }
                if change && self.insert.is_none() {
                    self.start_insert(1, false);
                }
            }
            Op::Indent | Op::Outdent => {
                let (l1, l2, first) = match &r {
                    Region::Chars(s, e) => (line_idx(b, *s), line_idx(b, (*e).max(1) - 1), *s),
                    Region::Lines(l1, l2) => (*l1, *l2, 0),
                    Region::Block { rows, top, .. } => (*top, top + rows.len() - 1, 0),
                };
                let _ = first;
                for _ in 0..amount.max(1) {
                    self.shift_lines(b, l1, l2, op == Op::Indent);
                }
                self.cur = first_nb(b, line_pos(b, l1));
            }
            Op::Lower | Op::Upper | Op::Toggle => {
                let f = match op {
                    Op::Lower => lower_char,
                    Op::Upper => upper_char,
                    _ => toggle_char,
                };
                let ranges: Vec<(usize, usize)> = match &r {
                    Region::Chars(s, e) => vec![(*s, *e)],
                    Region::Lines(l1, l2) => vec![Self::lines_range(b, *l1, *l2)],
                    Region::Block { rows, .. } => rows.clone(),
                };
                for &(s, e) in &ranges {
                    let new: Vec<char> = b[s..e].iter().map(|&c| f(c)).collect();
                    if new[..] != b[s..e] {
                        self.splice(b, s, e, &new);
                    }
                }
                self.cur = match &r {
                    Region::Lines(l1, _) => {
                        let cl = line_idx(b, self.cur);
                        if *l1 < cl { line_pos(b, *l1) } else { self.cur }
                    }
                    _ => ranges.first().map_or(self.cur, |x| x.0),
                };
            }
        }
    }

    fn shift_lines(&mut self, b: &mut Vec<char>, l1: usize, l2: usize, right: bool) {
        for l in (l1..=l2).rev() {
            let s = line_pos(b, l);
            if l >= nlines(b) {
                continue;
            }
            if right {
                if !is_empty_line(b, s) {
                    let pad = vec![' '; self.sw];
                    self.splice(b, s, s, &pad);
                }
            } else {
                let mut n = 0;
                while n < self.sw && s + n < b.len() && b[s + n] == ' ' {
                    n += 1;
                }
                if n == 0 && s < b.len() && b[s] == '\t' {
                    n = 1;
                }
                self.splice(b, s, s + n, &[]);
            }
        }
    }

    // ---------------------------------------------------------------- infogning

    fn start_insert(&mut self, count: usize, replace: bool) {
        self.mode = if replace { Mode::Replace } else { Mode::Insert };
        self.insert = Some(Ins { count: count.max(1), typed: String::new(), replace, replaced: Vec::new(), block: None, open: None, replaying: false });
    }

    fn insert_chars(&mut self, b: &mut Vec<char>, s: &str) {
        let v: Vec<char> = s.chars().collect();
        let at = self.cur.min(b.len());
        self.splice(b, at, at, &v);
        self.cur = at + v.len();
    }

    fn insert_key(&mut self, k: Key, b: &mut Vec<char>) {
        match k {
            Key::Esc | Key::Ctrl('[') => self.finish_insert(b),
            Key::Char(c) => {
                self.insert_chars(b, &c.to_string());
                if let Some(i) = &mut self.insert {
                    i.typed.push(c);
                }
            }
            Key::Enter => {
                self.insert_chars(b, "\n");
                if let Some(i) = &mut self.insert {
                    i.typed.push('\n');
                }
            }
            Key::Tab => {
                self.insert_chars(b, "\t");
                if let Some(i) = &mut self.insert {
                    i.typed.push('\t');
                }
            }
            Key::Backspace => {
                if self.cur > 0 {
                    self.splice(b, self.cur - 1, self.cur, &[]);
                    self.cur -= 1;
                    if let Some(i) = &mut self.insert {
                        i.typed.pop();
                    }
                }
            }
            Key::Delete => {
                if self.cur < b.len() {
                    self.splice(b, self.cur, self.cur + 1, &[]);
                }
            }
            Key::Left => self.cur = self.cur.saturating_sub(1).max(ls(b, self.cur)),
            Key::Right => self.cur = (self.cur + 1).min(le(b, self.cur)),
            Key::Up | Key::Down => {
                let cl = line_idx(b, self.cur);
                let t = if k == Key::Up { cl.saturating_sub(1) } else { (cl + 1).min(nlines(b) - 1) };
                self.cur = at_col(b, t, col_of(b, self.cur));
            }
            Key::Home => self.cur = ls(b, self.cur),
            Key::End => self.cur = le(b, self.cur),
            Key::Ctrl(_) => {}
        }
    }

    fn replace_key(&mut self, k: Key, b: &mut Vec<char>) {
        match k {
            Key::Esc | Key::Ctrl('[') => self.finish_insert(b),
            Key::Char(c) => {
                let at = self.cur.min(b.len());
                let old = (at < b.len() && b[at] != '\n').then(|| b[at]);
                if old.is_some() {
                    self.splice(b, at, at + 1, &[c]);
                } else {
                    self.splice(b, at, at, &[c]);
                }
                self.cur = at + 1;
                if let Some(i) = &mut self.insert {
                    i.typed.push(c);
                    i.replaced.push(old);
                }
            }
            Key::Enter => {
                self.insert_chars(b, "\n");
                if let Some(i) = &mut self.insert {
                    i.typed.push('\n');
                    i.replaced.push(None);
                }
            }
            Key::Backspace => {
                let popped = self.insert.as_mut().and_then(|i| {
                    i.typed.pop();
                    i.replaced.pop()
                });
                if let Some(old) = popped {
                    if self.cur > 0 {
                        let at = self.cur - 1;
                        match old {
                            Some(o) => self.splice(b, at, at + 1, &[o]),
                            None => self.splice(b, at, at + 1, &[]),
                        }
                        self.cur = at;
                    }
                } else {
                    self.cur = self.cur.saturating_sub(1);
                }
            }
            Key::Left => self.cur = self.cur.saturating_sub(1).max(ls(b, self.cur)),
            Key::Right => self.cur = (self.cur + 1).min(le(b, self.cur)),
            _ => {}
        }
    }

    /// Esc i infogningsläget: upprepar texten, fyller i blockets övriga rader och går tillbaka.
    fn finish_insert(&mut self, b: &mut Vec<char>) {
        let Some(ins) = self.insert.take() else {
            self.mode = Mode::Normal;
            return;
        };
        let typed: Vec<char> = ins.typed.chars().collect();
        if ins.count > 1 && !typed.is_empty() {
            let extra = ins.count - 1;
            if let Some(below) = ins.open {
                // `3ofoo`: hela raden upprepas.
                let mut chunk: Vec<char> = Vec::new();
                if below {
                    for _ in 0..extra {
                        chunk.push('\n');
                        chunk.extend(&typed);
                    }
                    let at = self.cur;
                    self.splice(b, at, at, &chunk);
                    self.cur = at + chunk.len();
                } else {
                    for _ in 0..extra {
                        chunk.extend(&typed);
                        chunk.push('\n');
                    }
                    let at = ls(b, self.cur);
                    self.splice(b, at, at, &chunk);
                    self.cur += chunk.len();
                }
            } else if ins.replace {
                for _ in 0..extra {
                    self.replace_text(b, &typed);
                }
            } else {
                let rep: Vec<char> = (0..extra).flat_map(|_| typed.iter().copied()).collect();
                let at = self.cur;
                self.splice(b, at, at, &rep);
                self.cur = at + rep.len();
            }
        }
        if let Some(bl) = &ins.block {
            if !typed.is_empty() && !typed.contains(&'\n') {
                for l in bl.top + 1..=bl.bot {
                    if l >= nlines(b) {
                        break;
                    }
                    let st = line_pos(b, l);
                    let en = le(b, st);
                    let len = en - st;
                    let at = if bl.eol {
                        en
                    } else if len < bl.col {
                        if bl.append {
                            let pad = vec![' '; bl.col - len];
                            self.splice(b, en, en, &pad);
                            st + bl.col
                        } else {
                            continue;
                        }
                    } else {
                        st + bl.col
                    };
                    self.splice(b, at, at, &typed);
                }
                let ts = line_pos(b, bl.top);
                self.cur = if bl.eol { le(b, ts).saturating_sub(1).max(ts) } else { ts + bl.col.min(le(b, ts) - ts) };
                self.mode = Mode::Normal;
                self.regs.insert('.', Reg { text: ins.typed.clone(), kind: RegKind::Char });
                if !ins.replaying {
                    if let Some(c) = &mut self.last_change {
                        c.ins = ins.typed.clone();
                    }
                }
                return;
            }
        }
        if !ins.replaying {
            if let Some(c) = &mut self.last_change {
                c.ins = ins.typed.clone();
                c.replace = ins.replace;
            }
        }
        if !ins.typed.is_empty() {
            self.regs.insert('.', Reg { text: ins.typed.clone(), kind: RegKind::Char });
        }
        self.mode = Mode::Normal;
        if self.cur > ls(b, self.cur) {
            self.cur -= 1;
        }
    }

    fn replace_text(&mut self, b: &mut Vec<char>, s: &[char]) {
        for &c in s {
            let at = self.cur.min(b.len());
            if at < b.len() && b[at] != '\n' && c != '\n' {
                self.splice(b, at, at + 1, &[c]);
            } else {
                self.splice(b, at, at, &[c]);
            }
            self.cur = at + 1;
        }
    }
}

/// Hur en tangent visas bland väntande kommandon i statusraden.
fn display_key(c: char) -> String {
    match c {
        '\u{1b}' => "^[".into(),
        '\n' => "<CR>".into(),
        '\u{7f}' => "<BS>".into(),
        '\t' => "<Tab>".into(),
        '\u{e000}'..='\u{e006}' => String::new(),
        c if (c as u32) < 0x20 => format!("^{}", ((c as u8) + 64) as char),
        c => c.to_string(),
    }
}

// ------------------------------------------------------------------ utförande

impl Vim {
    pub fn cmd_prefix(&self) -> char {
        self.cmd_kind
    }

    fn exec(&mut self, b: &mut Vec<char>, cmd: Cmd, view: View, replay: bool) {
        match cmd.kind {
            Kind::Move(m) => self.do_move(b, m, cmd.count, view),
            Kind::Op(op, tgt) => self.exec_op(b, cmd, op, tgt, view, replay),
            Kind::Act(a) => self.exec_act(b, cmd, a, view, replay),
            Kind::Vis(v) => self.exec_vis(b, cmd, v, replay),
        }
    }

    fn op_region(&mut self, b: &[char], op: Op, tgt: Tgt, count: Option<usize>, view: View) -> Option<Region> {
        let cur = self.cur.min(b.len());
        let n = count.unwrap_or(1).max(1);
        match tgt {
            Tgt::Lines => {
                let l1 = line_idx(b, cur);
                Some(Region::Lines(l1, (l1 + n - 1).min(nlines(b) - 1)))
            }
            Tgt::Obj(o) => {
                let (s, e, lw) = object(b, cur, o, n)?;
                Some(if lw { Region::Lines(line_idx(b, s), line_idx(b, e.saturating_sub(1).max(s))) } else { Region::Chars(s, e) })
            }
            Tgt::Mo(m) => {
                // `cw` på ett ord ändrar bara till ordets slut, som `ce`.
                if op == Op::Change {
                    if let Mo::Word(big) = m {
                        if cur < b.len() && matches!(cls(b[cur], big), 1 | 2) {
                            let mut p = run_end(b, cur, big) - 1;
                            for _ in 1..n {
                                p = word_end(b, p, big);
                            }
                            return Some(Region::Chars(cur, p + 1));
                        }
                    }
                }
                let mr = self.motion(b, m, count, true, view)?;
                let (a, z) = (cur.min(mr.pos), cur.max(mr.pos));
                if mr.lw {
                    return Some(Region::Lines(line_idx(b, a), line_idx(b, z)));
                }
                let mut e = (z + mr.incl as usize).min(b.len());
                if !mr.incl && z > a && b[z - 1] == '\n' && line_idx(b, a) < line_idx(b, z) {
                    // Exklusiv rörelse som slutar i kolumn 0: slutar på raden ovanför.
                    e = z - 1;
                    if a <= first_nb(b, ls(b, a)) {
                        return Some(Region::Lines(line_idx(b, a), line_idx(b, e)));
                    }
                }
                Some(Region::Chars(a, e))
            }
        }
    }

    fn exec_op(&mut self, b: &mut Vec<char>, cmd: Cmd, op: Op, tgt: Tgt, view: View, replay: bool) {
        if let Tgt::Mo(Mo::SearchPrompt(fwd)) = tgt {
            self.pending_op = Some(PendingOp { reg: cmd.reg, op, count: cmd.count });
            self.begin_cmdline(if fwd { '/' } else { '?' }, "");
            return;
        }
        let Some(region) = self.op_region(b, op, tgt, cmd.count, view) else {
            self.failed = true;
            return;
        };
        if matches!(region, Region::Chars(s, e) if s == e) && op != Op::Change {
            return;
        }
        if op != Op::Yank {
            if !replay {
                self.last_change = Some(Change { cmd, ins: String::new(), replace: false, vis: None });
            }
            self.snap(b);
        }
        self.apply_op(b, op, cmd.reg, region, 1);
    }

    fn shape(&self, b: &[char]) -> VisShape {
        let (lines, width) = match self.region(b) {
            Region::Chars(s, e) => {
                let l = line_idx(b, e.max(1) - 1) - line_idx(b, s) + 1;
                (l, if l == 1 { e - s } else { col_of(b, e.max(1) - 1) + 1 })
            }
            Region::Lines(l1, l2) => (l2 - l1 + 1, 0),
            Region::Block { rows, .. } => {
                let (ca, cc) = (col_of(b, self.anchor), col_of(b, self.cur));
                (rows.len(), ca.max(cc) - ca.min(cc) + 1)
            }
        };
        VisShape { mode: self.mode, lines, width }
    }

    fn exec_vis(&mut self, b: &mut Vec<char>, cmd: Cmd, v: VAc, replay: bool) {
        let nn = cmd.count.unwrap_or(1).max(1);
        match v {
            VAc::Swap => std::mem::swap(&mut self.anchor, &mut self.cur),
            VAc::SwapCorner => {
                if self.mode == Mode::VisualBlock {
                    let (la, lc) = (line_idx(b, self.anchor), line_idx(b, self.cur));
                    let (ca, cc) = (col_of(b, self.anchor), col_of(b, self.cur));
                    self.anchor = at_col(b, la, cc);
                    self.cur = at_col(b, lc, ca);
                } else {
                    std::mem::swap(&mut self.anchor, &mut self.cur);
                }
            }
            VAc::Obj(o) => {
                if let Some((s, e, lw)) = object(b, self.cur, o, nn) {
                    if lw {
                        self.mode = Mode::VisualLine;
                    }
                    self.anchor = s;
                    self.cur = e.saturating_sub(1).max(s);
                }
            }
            VAc::Op(op, forced) => {
                let mut region = self.region(b);
                if forced {
                    if self.mode == Mode::VisualBlock && matches!(op, Op::Delete | Op::Change) {
                        self.block_dollar = true;
                        region = self.region(b);
                    } else {
                        let (l1, l2) = (line_idx(b, self.anchor.min(self.cur)), line_idx(b, self.anchor.max(self.cur)));
                        region = Region::Lines(l1, l2);
                    }
                }
                let vis = self.shape(b);
                if op != Op::Yank {
                    if !replay {
                        self.last_change = Some(Change { cmd, ins: String::new(), replace: false, vis: Some(vis) });
                    }
                    self.snap(b);
                }
                self.exit_visual();
                self.apply_op(b, op, cmd.reg, region, nn);
            }
            VAc::Join { space } => {
                let (l1, l2) = (line_idx(b, self.anchor.min(self.cur)), line_idx(b, self.anchor.max(self.cur)));
                if !replay {
                    let vis = self.shape(b);
                    self.last_change = Some(Change { cmd, ins: String::new(), replace: false, vis: Some(vis) });
                }
                self.snap(b);
                self.exit_visual();
                self.cur = line_pos(b, l1);
                self.join(b, (l2 - l1).max(1), space);
            }
            VAc::Replace(ch) => {
                let region = self.region(b);
                if !replay {
                    let vis = self.shape(b);
                    self.last_change = Some(Change { cmd, ins: String::new(), replace: false, vis: Some(vis) });
                }
                self.snap(b);
                self.exit_visual();
                let ranges: Vec<(usize, usize)> = match &region {
                    Region::Chars(s, e) => vec![(*s, *e)],
                    Region::Lines(l1, l2) => vec![Self::lines_range(b, *l1, *l2)],
                    Region::Block { rows, .. } => rows.clone(),
                };
                for &(s, e) in &ranges {
                    let new: Vec<char> = b[s..e].iter().map(|&c| if c == '\n' { c } else { ch }).collect();
                    self.splice(b, s, e, &new);
                }
                self.cur = ranges.first().map_or(self.cur, |r| r.0);
            }
            VAc::Put => {
                let Some(reg) = self.get_reg(cmd.reg) else {
                    self.notify(t("E353: Registret är tomt").to_owned(), true);
                    return;
                };
                let region = self.region(b);
                self.snap(b);
                self.exit_visual();
                let eof = match &region {
                    Region::Lines(_, l2) => l2 + 1 >= nlines(b),
                    _ => false,
                };
                let chars_sel = matches!(region, Region::Chars(..));
                let start = match &region {
                    Region::Chars(s, _) => *s,
                    _ => 0,
                };
                self.apply_op(b, Op::Delete, None, region, 1);
                match (chars_sel, reg.kind) {
                    (true, RegKind::Line) => {
                        // Raden delas och registrets rader hamnar emellan.
                        self.splice(b, start, start, &['\n']);
                        self.cur = start;
                        self.put(b, &reg, true, nn, false);
                    }
                    (true, _) => {
                        self.cur = start;
                        self.put(b, &reg, false, nn, false);
                    }
                    (false, RegKind::Line) => self.put(b, &reg, eof, nn, false),
                    (false, _) => {
                        let as_line = Reg { text: format!("{}\n", reg.text), kind: RegKind::Line };
                        self.put(b, &as_line, eof, nn, false);
                    }
                }
            }
            VAc::BlockI | VAc::BlockA => {
                let append = v == VAc::BlockA;
                if self.mode != Mode::VisualBlock {
                    let at = if append { le(b, self.cur) } else { ls(b, self.cur) };
                    self.exit_visual();
                    self.snap(b);
                    self.cur = at;
                    self.start_insert(1, false);
                    return;
                }
                let region = self.region(b);
                let Region::Block { rows, col, top } = region else { return };
                let bot = top + rows.len() - 1;
                let (ca, cc) = (col_of(b, self.anchor), col_of(b, self.cur));
                let dollar = self.block_dollar;
                self.snap(b);
                self.exit_visual();
                let (colx, at) = if append {
                    let c2 = ca.max(cc) + 1;
                    let st = line_pos(b, top);
                    let en = le(b, st);
                    if dollar {
                        (c2, en)
                    } else {
                        if en - st < c2 {
                            let pad = vec![' '; c2 - (en - st)];
                            self.splice(b, en, en, &pad);
                        }
                        (c2, st + c2)
                    }
                } else {
                    (col, at_col(b, top, col))
                };
                self.cur = at;
                self.start_insert(1, false);
                if let Some(i) = &mut self.insert {
                    i.block = Some(BlockIns { top, bot, col: colx, append, eol: dollar && append });
                }
            }
        }
    }

    fn join(&mut self, b: &mut Vec<char>, times: usize, space: bool) {
        for _ in 0..times {
            let e = le(b, self.cur);
            if e >= b.len() {
                break;
            }
            let mut j = e + 1;
            if space {
                while j < b.len() && is_blank(b[j]) {
                    j += 1;
                }
            }
            let next_empty = j >= b.len() || b[j] == '\n';
            let cur_empty = e == ls(b, self.cur);
            let prev_ws = e > 0 && b[e - 1].is_whitespace();
            let sep: &[char] = if space && !next_empty && !cur_empty && !prev_ws && b[j] != ')' { &[' '] } else { &[] };
            self.splice(b, e, j, sep);
            self.cur = e;
        }
    }

    fn put(&mut self, b: &mut Vec<char>, reg: &Reg, after: bool, n: usize, cursor_end: bool) {
        match reg.kind {
            RegKind::Line => {
                let mut text = reg.text.clone();
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                let full: Vec<char> = text.repeat(n).chars().collect();
                let e = le(b, self.cur);
                let (at, ins, line_start): (usize, Vec<char>, usize) = if after {
                    if e >= b.len() {
                        let mut v = vec!['\n'];
                        v.extend(&full[..full.len() - 1]);
                        (b.len(), v, e + 1)
                    } else {
                        (e + 1, full.clone(), e + 1)
                    }
                } else {
                    let s = ls(b, self.cur);
                    (s, full.clone(), s)
                };
                self.splice(b, at, at, &ins);
                self.cur = if cursor_end { (line_start + full.len()).min(b.len()) } else { first_nb(b, line_start) };
            }
            RegKind::Char => {
                let chars: Vec<char> = reg.text.repeat(n).chars().collect();
                if chars.is_empty() {
                    return;
                }
                let e = le(b, self.cur);
                let at = if after && self.cur < e { self.cur + 1 } else { self.cur };
                self.splice(b, at, at, &chars);
                self.cur = if cursor_end {
                    at + chars.len()
                } else if chars.contains(&'\n') {
                    at
                } else {
                    at + chars.len() - 1
                };
            }
            RegKind::Block => {
                let rows: Vec<Vec<char>> = reg.text.split('\n').map(|r| r.chars().collect()).collect();
                let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
                let cl = line_idx(b, self.cur);
                let col0 = col_of(b, self.cur) + usize::from(after && self.cur < le(b, self.cur));
                for (i, row) in rows.iter().enumerate() {
                    let l = cl + i;
                    if l >= nlines(b) {
                        let end = b.len();
                        self.splice(b, end, end, &['\n']);
                    }
                    let st = line_pos(b, l);
                    let en = le(b, st);
                    let len = en - st;
                    let mut ins: Vec<char> = Vec::new();
                    let at = if len < col0 {
                        ins.extend(vec![' '; col0 - len]);
                        en
                    } else {
                        st + col0
                    };
                    for _ in 0..n {
                        ins.extend(row);
                    }
                    if at < en {
                        let target = width * n;
                        while ins.len() - if len < col0 { col0 - len } else { 0 } < target {
                            ins.push(' ');
                        }
                    }
                    self.splice(b, at, at, &ins);
                }
                self.cur = at_col(b, cl, col0);
            }
        }
    }

    fn exec_act(&mut self, b: &mut Vec<char>, cmd: Cmd, a: Ac, view: View, replay: bool) {
        let nn = cmd.count.unwrap_or(1).max(1);
        let changing = matches!(
            a,
            Ac::Replace(_) | Ac::Join { .. } | Ac::Put { .. } | Ac::Open { .. } | Ac::Insert(_) | Ac::Tilde | Ac::ReplaceMode
        );
        if changing {
            if !replay {
                self.last_change = Some(Change { cmd, ins: String::new(), replace: false, vis: None });
            }
            self.snap(b);
        }
        let cur = self.cur.min(b.len());
        match a {
            Ac::Visual(mode) => {
                if self.mode == mode {
                    self.exit_visual();
                } else if self.mode.is_visual() {
                    self.mode = mode;
                } else {
                    self.anchor = cur;
                    self.mode = mode;
                    self.block_dollar = false;
                }
            }
            Ac::Reselect => {
                if let Some((m, an, c)) = self.last_visual {
                    self.mode = m;
                    self.anchor = an.min(b.len());
                    self.cur = c.min(b.len());
                }
            }
            Ac::CmdLine => {
                let prefill = if self.mode.is_visual() {
                    self.exit_visual();
                    "'<,'>".to_owned()
                } else if let Some(c) = cmd.count {
                    if c > 1 { format!(".,.+{}", c - 1) } else { ".".into() }
                } else {
                    String::new()
                };
                self.begin_cmdline(':', &prefill);
            }
            Ac::Undo => self.do_undo(b),
            Ac::Redo => self.do_redo(b),
            Ac::Dot => self.dot(b, cmd.count, view),
            Ac::Record(r) => self.recording = Some((r, Vec::new())),
            Ac::StopRecord => {
                if let Some((r, mut keys)) = self.recording.take() {
                    if keys.last() == Some(&'q') {
                        keys.pop();
                    }
                    let text: String = keys.into_iter().collect();
                    let lc = r.to_ascii_lowercase();
                    let text = match self.regs.get(&lc) {
                        Some(old) if r.is_ascii_uppercase() => format!("{}{text}", old.text),
                        _ => text,
                    };
                    self.regs.insert(lc, Reg { text, kind: RegKind::Char });
                }
            }
            Ac::Play(r) => self.play(b, r, nn, view),
            Ac::SetMark(c) => {
                self.marks.insert(if c == '`' { '\'' } else { c }, cur);
            }
            Ac::SaveQuit => self.fx.push(Effect::SaveQuit),
            Ac::QuitForce => self.fx.push(Effect::Quit { force: true }),
            Ac::Insert(at) => {
                self.cur = match at {
                    InsAt::Cursor => cur,
                    InsAt::After => {
                        if cur < le(b, cur) { cur + 1 } else { cur }
                    }
                    InsAt::LineEnd => le(b, cur),
                    InsAt::LineStartNb => first_nb(b, ls(b, cur)),
                    InsAt::Col0 => ls(b, cur),
                };
                self.start_insert(nn, false);
            }
            Ac::Open { below } => {
                if below {
                    let e = le(b, cur);
                    self.splice(b, e, e, &['\n']);
                    self.cur = e + 1;
                } else {
                    let s = ls(b, cur);
                    self.splice(b, s, s, &['\n']);
                    self.cur = s;
                }
                self.start_insert(nn, false);
                if let Some(i) = &mut self.insert {
                    i.open = Some(below);
                }
            }
            Ac::ReplaceMode => self.start_insert(1, true),
            Ac::Replace(ch) => {
                let e = le(b, cur);
                if cur + nn > e {
                    self.failed = true;
                    return;
                }
                if ch == '\n' {
                    self.splice(b, cur, cur + nn, &['\n']);
                    self.cur = cur + 1;
                } else {
                    self.splice(b, cur, cur + nn, &vec![ch; nn]);
                    self.cur = cur + nn - 1;
                }
            }
            Ac::Join { space } => self.join(b, nn.max(2) - 1, space),
            Ac::Put { after, cursor_end } => match self.get_reg(cmd.reg) {
                Some(reg) => self.put(b, &reg, after, nn, cursor_end),
                None => self.notify(t("E353: Registret är tomt").to_owned(), true),
            },
            Ac::Tilde => {
                let e = le(b, cur);
                let end = (cur + nn).min(e);
                if end > cur {
                    let new: Vec<char> = b[cur..end].iter().map(|&c| toggle_char(c)).collect();
                    self.splice(b, cur, end, &new);
                    self.cur = end;
                }
            }
        }
    }

    fn do_undo(&mut self, b: &mut Vec<char>) {
        let now: String = b.iter().collect();
        while let Some(s) = self.undo.pop() {
            if s.text == now {
                continue;
            }
            self.redo.push(Snap { text: now, cur: self.cur });
            *b = s.text.chars().collect();
            self.cur = s.cur.min(b.len());
            self.dirty = true;
            return;
        }
        self.notify(t("Redan äldsta ändringen").to_owned(), false);
    }

    fn do_redo(&mut self, b: &mut Vec<char>) {
        let now: String = b.iter().collect();
        while let Some(s) = self.redo.pop() {
            if s.text == now {
                continue;
            }
            self.undo.push(Snap { text: now, cur: self.cur });
            *b = s.text.chars().collect();
            self.cur = s.cur.min(b.len());
            self.dirty = true;
            return;
        }
        self.notify(t("Redan nyaste ändringen").to_owned(), false);
    }

    /// `.`: upprepar senaste ändringen, med nytt antal om ett anges.
    fn dot(&mut self, b: &mut Vec<char>, count: Option<usize>, view: View) {
        let Some(ch) = self.last_change.clone() else { return };
        let mut cmd = ch.cmd;
        if count.is_some() {
            cmd.count = count;
        }
        if let Some(v) = ch.vis {
            let cur = self.cur.min(b.len());
            let (l, c) = (line_idx(b, cur), col_of(b, cur));
            self.anchor = cur;
            self.mode = v.mode;
            self.cur = match v.mode {
                Mode::VisualLine => at_col(b, l + v.lines - 1, c),
                Mode::VisualBlock => at_col(b, l + v.lines - 1, c + v.width.max(1) - 1),
                _ if v.lines == 1 => (cur + v.width.max(1) - 1).min(b.len()),
                _ => at_col(b, l + v.lines - 1, v.width.max(1) - 1),
            };
        }
        self.exec(b, cmd, view, true);
        if let Some(ins) = &mut self.insert {
            ins.replaying = true;
            ins.typed = ch.ins.clone();
            let typed: Vec<char> = ch.ins.chars().collect();
            let replace = ins.replace;
            if replace {
                self.replace_text(b, &typed);
            } else {
                let at = self.cur.min(b.len());
                self.splice(b, at, at, &typed);
                self.cur = at + typed.len();
            }
            self.finish_insert(b);
        }
    }

    fn play(&mut self, b: &mut Vec<char>, r: char, n: usize, view: View) {
        if r == ':' {
            if let Some(l) = self.last_ex.clone() {
                for _ in 0..n {
                    self.exec_ex(b, &l, view);
                }
            }
            return;
        }
        let reg = if r == '@' { self.last_macro } else { Some(r) };
        let Some(reg) = reg else { return };
        let Some(text) = self.regs.get(&reg.to_ascii_lowercase()).map(|r| r.text.clone()) else { return };
        if self.macro_depth >= 20 {
            return;
        }
        self.last_macro = Some(reg);
        self.macro_depth += 1;
        'outer: for _ in 0..n {
            self.failed = false;
            for c in text.chars() {
                self.handle(dec(c), b, view);
                if self.failed {
                    break 'outer;
                }
            }
        }
        self.macro_depth -= 1;
    }

    // ---------------------------------------------------------------- kommandoraden

    fn cmd_key(&mut self, k: Key, b: &mut Vec<char>, view: View) {
        let ret = if self.cmd_kind == ':' { Mode::Normal } else { self.ret_mode };
        match k {
            Key::Esc | Key::Ctrl('[') | Key::Ctrl('c') => {
                self.mode = ret;
                self.cmdline.clear();
                self.pending_op = None;
            }
            Key::Enter => {
                let line = std::mem::take(&mut self.cmdline);
                self.mode = ret;
                if !line.is_empty() && self.history.last() != Some(&line) {
                    self.history.push(line.clone());
                }
                if self.cmd_kind == ':' {
                    self.exec_ex(b, &line, view);
                } else {
                    self.commit_search(b, &line, view);
                }
            }
            Key::Backspace => {
                if self.cmdline.is_empty() {
                    self.mode = ret;
                    self.pending_op = None;
                } else {
                    self.cmdline.pop();
                }
            }
            Key::Char(c) => self.cmdline.push(c),
            Key::Tab => self.cmdline.push('\t'),
            Key::Ctrl('u') => self.cmdline.clear(),
            Key::Ctrl('w') => {
                let t = self.cmdline.trim_end().to_owned();
                let cut = t.rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + 1);
                self.cmdline = t[..cut].to_owned();
            }
            Key::Up => {
                let i = self.hist_at.map_or(self.history.len(), |i| i).saturating_sub(1);
                if let Some(h) = self.history.get(i) {
                    self.cmdline = h.clone();
                    self.hist_at = Some(i);
                }
            }
            Key::Down => {
                if let Some(i) = self.hist_at {
                    if let Some(h) = self.history.get(i + 1) {
                        self.cmdline = h.clone();
                        self.hist_at = Some(i + 1);
                    } else {
                        self.cmdline.clear();
                        self.hist_at = None;
                    }
                }
            }
            _ => {}
        }
    }

    fn commit_search(&mut self, b: &mut Vec<char>, line: &str, view: View) {
        let fwd = self.cmd_kind == '/';
        let pat = if line.is_empty() {
            match &self.last_search {
                Some((p, _)) => p.clone(),
                None => {
                    self.pending_op = None;
                    return;
                }
            }
        } else {
            line.to_owned()
        };
        self.last_search = Some((pat, fwd));
        if let Some(p) = self.pending_op.take() {
            let cmd = Cmd { reg: p.reg, count: p.count, kind: Kind::Op(p.op, Tgt::Mo(Mo::SearchNext(false))) };
            self.exec(b, cmd, view, false);
        } else {
            self.do_move(b, Mo::SearchNext(false), None, view);
        }
    }
}

// ------------------------------------------------------------------ Ex-kommandon

fn ab(name: &str, full: &str, min: usize) -> bool {
    name.len() >= min && full.starts_with(name)
}

fn err(s: String) -> String {
    s
}

/// Delar `s` vid avgränsaren `d` (`\d` är ett vanligt tecken). Ger upp till tre delar.
fn split_delim(s: &str, d: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.peek() {
                Some(&n) if n == d => {
                    parts.last_mut().map(|p| p.push(d));
                    it.next();
                }
                Some(&n) => {
                    if let Some(p) = parts.last_mut() {
                        p.push('\\');
                        p.push(n);
                    }
                    it.next();
                }
                None => {
                    parts.last_mut().map(|p| p.push('\\'));
                }
            }
        } else if c == d && parts.len() < 3 {
            parts.push(String::new());
        } else if let Some(p) = parts.last_mut() {
            p.push(c);
        }
    }
    parts
}

fn put_case(out: &mut String, s: &str, mode: Option<char>, once: &mut Option<char>) {
    for c in s.chars() {
        let c = if let Some(o) = once.take() {
            if o == 'u' { upper_char(c) } else { lower_char(c) }
        } else {
            match mode {
                Some('U') => upper_char(c),
                Some('L') => lower_char(c),
                _ => c,
            }
        };
        out.push(c);
    }
}

/// Byter ut `&`, `\1`…`\9`, `\n`, `\t` och skiftlägesräknare i ersättningstexten.
fn expand(repl: &str, caps: &regex::Captures) -> String {
    let mut out = String::new();
    let mut mode: Option<char> = None;
    let mut once: Option<char> = None;
    let mut it = repl.chars();
    while let Some(c) = it.next() {
        match c {
            '&' => put_case(&mut out, caps.get(0).map_or("", |m| m.as_str()), mode, &mut once),
            '\\' => match it.next() {
                Some(d) if d.is_ascii_digit() => {
                    let g = d.to_digit(10).unwrap_or(0) as usize;
                    put_case(&mut out, caps.get(g).map_or("", |m| m.as_str()), mode, &mut once);
                }
                Some('n') | Some('r') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(x @ ('u' | 'l')) => once = Some(x),
                Some(x @ ('U' | 'L')) => mode = Some(x),
                Some('e') | Some('E') => mode = None,
                Some(x) => put_case(&mut out, &x.to_string(), mode, &mut once),
                None => out.push('\\'),
            },
            c => put_case(&mut out, &c.to_string(), mode, &mut once),
        }
    }
    out
}

impl Vim {
    fn parse_addr<'a>(&mut self, b: &[char], s: &'a str) -> Result<(Option<usize>, &'a str), String> {
        let total = nlines(b) as i64;
        let cl = line_idx(b, self.cur) as i64 + 1;
        let mut rest = s.trim_start();
        let mut val: Option<i64> = None;
        let c = rest.chars().next();
        match c {
            Some(d) if d.is_ascii_digit() => {
                let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
                val = Some(rest[..end].parse::<i64>().unwrap_or(i64::MAX / 4));
                rest = &rest[end..];
            }
            Some('.') => {
                val = Some(cl);
                rest = &rest[1..];
            }
            Some('$') => {
                val = Some(total);
                rest = &rest[1..];
            }
            Some('\'') => {
                let m = rest[1..].chars().next().ok_or_else(|| t("E20: Markeringen är inte satt").to_owned())?;
                let &p = self.marks.get(&m).ok_or_else(|| t("E20: Markeringen är inte satt").to_owned())?;
                val = Some(line_idx(b, p.min(b.len())) as i64 + 1);
                rest = &rest[1 + m.len_utf8()..];
            }
            Some('/') => {
                let end = rest[1..].find('/').map_or(rest.len(), |i| i + 1);
                let pat = rest[1..end].to_owned();
                let re = self.build_regex(&pat).ok_or_else(|| tf!("E486: Hittade inte mönstret: {pat}", pat = pat))?;
                let text: String = b.iter().collect();
                let found = text.lines().enumerate().skip(cl as usize).find(|(_, l)| re.is_match(l)).map(|(i, _)| i as i64 + 1);
                val = Some(found.ok_or_else(|| tf!("E486: Hittade inte mönstret: {pat}", pat = pat))?);
                rest = if end < rest.len() { &rest[end + 1..] } else { "" };
            }
            _ => {}
        }
        loop {
            let Some(sign) = rest.chars().next().filter(|c| *c == '+' || *c == '-') else { break };
            rest = &rest[1..];
            let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            let n: i64 = if end == 0 { 1 } else { rest[..end].parse().unwrap_or(1) };
            rest = &rest[end..];
            let base = val.unwrap_or(cl);
            val = Some(if sign == '+' { base + n } else { base - n });
        }
        Ok((val.map(|v| (v.clamp(1, total) - 1) as usize), rest))
    }

    #[allow(clippy::type_complexity)]
    fn parse_range<'a>(&mut self, b: &[char], s: &'a str) -> Result<(Option<(usize, usize)>, &'a str), String> {
        let rest = s.trim_start();
        let last = nlines(b) - 1;
        if let Some(r) = rest.strip_prefix('%') {
            return Ok((Some((0, last)), r));
        }
        let cl = line_idx(b, self.cur);
        let (a, r1) = self.parse_addr(b, rest)?;
        let r1t = r1.trim_start();
        if let Some(r2) = r1t.strip_prefix(',').or_else(|| r1t.strip_prefix(';')) {
            let (z, r3) = self.parse_addr(b, r2)?;
            let (x, y) = (a.unwrap_or(cl), z.unwrap_or(cl));
            return Ok((Some((x.min(y), x.max(y))), r3));
        }
        Ok((a.map(|a| (a, a)), r1))
    }

    fn exec_ex(&mut self, b: &mut Vec<char>, line: &str, view: View) {
        let _ = view;
        if !line.trim().is_empty() {
            self.last_ex = Some(line.to_owned());
        }
        let line = line.trim_start_matches([':', ' ']);
        if line.is_empty() {
            return;
        }
        let (range, rest) = match self.parse_range(b, line) {
            Ok(x) => x,
            Err(e) => return self.notify(err(e), true),
        };
        let rest = rest.trim_start();
        if rest.is_empty() {
            if let Some((_, l2)) = range {
                self.marks.insert('\'', self.cur);
                self.cur = first_nb(b, line_pos(b, l2));
            }
            return;
        }
        let name_end = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        let (name, args) = if name_end == 0 {
            let l = rest.chars().next().map_or(0, char::len_utf8);
            rest.split_at(l)
        } else {
            rest.split_at(name_end)
        };
        let (force, args) = match args.strip_prefix('!') {
            Some(a) => (true, a),
            None => (false, args),
        };
        let arg = args.trim();
        let cl = line_idx(b, self.cur);
        let (l1, l2) = range.unwrap_or((cl, cl));
        match name {
            n if ab(n, "write", 1) => {
                if arg.is_empty() {
                    self.fx.push(Effect::Save);
                } else {
                    self.fx.push(Effect::SaveAs(arg.to_owned()));
                }
            }
            n if ab(n, "wq", 2) || ab(n, "xit", 1) || ab(n, "exit", 3) => self.fx.push(Effect::SaveQuit),
            n if ab(n, "quit", 1) => self.fx.push(Effect::Quit { force }),
            n if ab(n, "close", 3) || ab(n, "only", 2) => self.fx.push(Effect::CloseSplit),
            n if ab(n, "qall", 2) || ab(n, "quitall", 5) => self.fx.push(Effect::QuitAll { force }),
            n if ab(n, "edit", 1) => {
                if arg.is_empty() {
                    self.notify(t("E32: Inget filnamn").to_owned(), true);
                } else {
                    self.fx.push(Effect::Edit(arg.to_owned()));
                }
            }
            n if ab(n, "split", 2) || n == "new" => {
                self.fx.push(Effect::Split { vertical: false });
                if !arg.is_empty() {
                    self.fx.push(Effect::Edit(arg.to_owned()));
                }
            }
            n if ab(n, "vsplit", 2) || n == "vnew" => {
                self.fx.push(Effect::Split { vertical: true });
                if !arg.is_empty() {
                    self.fx.push(Effect::Edit(arg.to_owned()));
                }
            }
            n if ab(n, "set", 2) => self.ex_set(arg),
            n if ab(n, "nohlsearch", 3) => {}
            n if ab(n, "substitute", 1) && !n.starts_with("se") && !n.starts_with("sp") => {
                self.ex_sub(b, range, args, false);
            }
            "&" => self.ex_sub(b, range, "", true),
            n if ab(n, "delete", 1) => {
                self.snap(b);
                self.apply_op(b, Op::Delete, arg.chars().next(), Region::Lines(l1, l2), 1);
            }
            n if ab(n, "yank", 1) => {
                let keep = self.cur;
                self.apply_op(b, Op::Yank, arg.chars().next(), Region::Lines(l1, l2), 1);
                self.cur = keep;
            }
            n if ab(n, "join", 1) => {
                self.snap(b);
                self.cur = line_pos(b, l1);
                self.join(b, (l2 - l1).max(1), !force);
            }
            ">" | "<" => {
                self.snap(b);
                self.shift_lines(b, l1, l2, name == ">");
                self.cur = first_nb(b, line_pos(b, l2));
            }
            n if ab(n, "undo", 1) => self.do_undo(b),
            n if ab(n, "redo", 3) => self.do_redo(b),
            other => self.notify(tf!("E492: Okänt kommando: {cmd}", cmd = other), true),
        }
    }

    fn ex_set(&mut self, args: &str) {
        for tok in args.split_whitespace() {
            let (key, val) = tok.split_once('=').map_or((tok, None), |(k, v)| (k, Some(v)));
            let mut k = key;
            let mut toggle = false;
            if let Some(x) = k.strip_suffix('!') {
                k = x;
                toggle = true;
            }
            if let Some(x) = k.strip_prefix("inv") {
                k = x;
                toggle = true;
            }
            let mut on = true;
            if let Some(x) = k.strip_prefix("no") {
                k = x;
                on = false;
            }
            let value = if toggle { None } else { Some(on) };
            match k {
                "nu" | "number" => self.fx.push(Effect::Set(Setting::Number, value)),
                "wrap" => self.fx.push(Effect::Set(Setting::Wrap, value)),
                "ic" | "ignorecase" => self.ignorecase = value.unwrap_or(!self.ignorecase),
                "sw" | "shiftwidth" => match val.and_then(|v| v.parse::<usize>().ok()) {
                    Some(n) if (1..=16).contains(&n) => self.sw = n,
                    _ => self.notify(tf!("E521: Talet behövs efter =: {opt}", opt = tok), true),
                },
                "scs" | "smartcase" | "hls" | "hlsearch" | "is" | "incsearch" | "ts" | "tabstop" | "et" | "expandtab" | "ai"
                | "autoindent" | "ruler" | "list" | "cursorline" | "rnu" | "relativenumber" | "lbr" | "linebreak" => {}
                _ => self.notify(tf!("E518: Okänd inställning: {opt}", opt = tok), true),
            }
        }
    }

    fn ex_sub(&mut self, b: &mut Vec<char>, range: Option<(usize, usize)>, args: &str, repeat: bool) {
        let cl = line_idx(b, self.cur);
        let (l1, l2) = range.unwrap_or((cl, cl));
        let a = args.trim_start();
        let (pat, repl, flags) = if repeat || a.is_empty() || a.starts_with('&') {
            match self.last_sub.clone() {
                Some(x) => x,
                None => return self.notify(t("E35: Inget tidigare reguljärt uttryck").to_owned(), true),
            }
        } else {
            let d = a.chars().next().unwrap_or('/');
            if d.is_alphanumeric() || matches!(d, '\\' | '"' | '|' | ' ') {
                return self.notify(t("E146: Ogiltig avgränsare för :s").to_owned(), true);
            }
            let parts = split_delim(&a[d.len_utf8()..], d);
            let pat = parts[0].clone();
            let pat = if pat.is_empty() { self.last_search.as_ref().map(|s| s.0.clone()).unwrap_or_default() } else { pat };
            (pat, parts.get(1).cloned().unwrap_or_default(), parts.get(2).cloned().unwrap_or_default())
        };
        if pat.is_empty() {
            return self.notify(t("E35: Inget tidigare reguljärt uttryck").to_owned(), true);
        }
        let global = flags.contains('g');
        let mut ic = self.ignorecase;
        if flags.contains('i') {
            ic = true;
        }
        if flags.contains('I') {
            ic = false;
        }
        let re = match RegexBuilder::new(&pat).multi_line(true).case_insensitive(ic).build() {
            Ok(r) => r,
            Err(e) => {
                let msg = tf!("E383: Ogiltigt sökmönster: {e}", e = e.to_string().lines().last().unwrap_or("").trim());
                return self.notify(msg, true);
            }
        };
        self.last_search = Some((pat.clone(), true));
        self.last_sub = Some((pat.clone(), repl.clone(), flags));
        // Radstarter för l1..=l2 (ett varv över texten).
        let mut starts = Vec::new();
        let mut line = 0usize;
        let mut st = 0usize;
        for (i, &c) in b.iter().enumerate() {
            if c == '\n' {
                if line >= l1 && line <= l2 {
                    starts.push((line, st, i));
                }
                line += 1;
                st = i + 1;
            }
        }
        if line >= l1 && line <= l2 {
            starts.push((line, st, b.len()));
        }
        let mut edits: Vec<(usize, usize, usize, String)> = Vec::new();
        let mut subs = 0usize;
        for &(ln, s, e) in &starts {
            let text: String = b[s..e].iter().collect();
            let mut out = String::new();
            let mut last = 0usize;
            let mut n = 0usize;
            for caps in re.captures_iter(&text) {
                let Some(m) = caps.get(0) else { continue };
                out.push_str(&text[last..m.start()]);
                out.push_str(&expand(&repl, &caps));
                last = m.end();
                n += 1;
                if !global {
                    break;
                }
            }
            if n > 0 {
                out.push_str(&text[last..]);
                subs += n;
                edits.push((ln, s, e, out));
            }
        }
        if subs == 0 {
            return self.notify(tf!("E486: Hittade inte mönstret: {pat}", pat = pat), true);
        }
        self.snap(b);
        let mut out: Vec<char> = Vec::with_capacity(b.len());
        let mut prev = 0usize;
        let mut added = 0usize;
        let mut last_line = 0usize;
        for (ln, s, e, new) in &edits {
            out.extend(&b[prev..*s]);
            out.extend(new.chars());
            prev = *e;
            last_line = ln + added + new.matches('\n').count();
            added += new.matches('\n').count();
        }
        out.extend(&b[prev..]);
        *b = out;
        self.dirty = true;
        self.cur = first_nb(b, line_pos(b, last_line));
        let lines = edits.len();
        let msg = if subs == 1 {
            t("1 ersättning på 1 rad").to_owned()
        } else {
            tf!("{subs} ersättningar på {lines} rader", subs = subs, lines = lines)
        };
        self.notify(msg, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESC: &str = "\x1b";

    /// `|` i `start` är markören; resultatet har `|` där markören hamnar.
    fn run(start: &str, keys: &str) -> String {
        let mut v = Vim::new();
        run_with(&mut v, start, keys).0
    }

    fn run_with(v: &mut Vim, start: &str, keys: &str) -> (String, Out) {
        let cur = start.chars().position(|c| c == '|').expect("markör saknas");
        let mut text: String = start.chars().filter(|&c| c != '|').collect();
        v.cur = cur;
        let out = v.keys(keys, &mut text);
        let mut s: Vec<char> = text.chars().collect();
        s.insert(v.cur.min(s.len()), '|');
        (s.into_iter().collect(), out)
    }

    /// Fortsätter på samma text med samma Vim-instans.
    fn more(v: &mut Vim, text: &str, keys: &str) -> String {
        run_with(v, text, keys).0
    }

    fn reg(v: &Vim, c: char) -> String {
        v.register(c).map(|r| r.text.clone()).unwrap_or_default()
    }

    // ---------------------------------------------------------------- rörelser

    #[test]
    fn basic_hjkl() {
        assert_eq!(run("|abc\ndef", "l"), "a|bc\ndef");
        assert_eq!(run("|abc\ndef", "j"), "abc\n|def");
        assert_eq!(run("abc\nd|ef", "k"), "a|bc\ndef");
        assert_eq!(run("a|bc", "h"), "|abc");
        assert_eq!(run("|abc", "5l"), "ab|c");
        assert_eq!(run("|abc", "h"), "|abc");
    }

    #[test]
    fn vertical_keeps_column() {
        assert_eq!(run("abcd|ef\nab\nabcdef", "jj"), "abcdef\nab\nabcd|ef");
        assert_eq!(run("abcd|ef\nab\nabcdef", "j"), "abcdef\na|b\nabcdef");
        assert_eq!(run("ab|cdef\nxy\nabcdef", "$jj"), "abcdef\nxy\nabcde|f");
    }

    #[test]
    fn word_motions() {
        assert_eq!(run("|foo bar baz", "w"), "foo |bar baz");
        assert_eq!(run("|foo bar baz", "2w"), "foo bar |baz");
        assert_eq!(run("|foo.bar baz", "w"), "foo|.bar baz");
        assert_eq!(run("|foo.bar baz", "W"), "foo.bar |baz");
        assert_eq!(run("|foo bar", "e"), "fo|o bar");
        assert_eq!(run("fo|o bar", "e"), "foo ba|r");
        assert_eq!(run("foo b|ar", "b"), "foo |bar");
        assert_eq!(run("foo |bar", "b"), "|foo bar");
        assert_eq!(run("foo.ba|r", "B"), "|foo.bar");
        assert_eq!(run("foo |bar", "ge"), "fo|o bar");
        assert_eq!(run("foo\n|bar", "b"), "|foo\nbar");
        assert_eq!(run("|foo\nbar", "w"), "foo\n|bar");
        assert_eq!(run("|foo\n\nbar", "w"), "foo\n|\nbar");
        assert_eq!(run("|foo\n\nbar", "ww"), "foo\n\n|bar");
        assert_eq!(run("a|b\n  c", "e"), "ab\n  |c");
    }

    #[test]
    fn line_motions() {
        assert_eq!(run("  fo|o bar", "0"), "|  foo bar");
        assert_eq!(run("  fo|o bar", "^"), "  |foo bar");
        assert_eq!(run("|foo bar", "$"), "foo ba|r");
        assert_eq!(run("|a\nb\nc", "G"), "a\nb\n|c");
        assert_eq!(run("a\nb\n|c", "gg"), "|a\nb\nc");
        assert_eq!(run("|a\nb\nc", "2G"), "a\n|b\nc");
        assert_eq!(run("a\nb\n|c", "2gg"), "a\n|b\nc");
        assert_eq!(run("|a\n  b", "+"), "a\n  |b");
        assert_eq!(run("a\n  |b", "-"), "|a\n  b");
        assert_eq!(run("|ab\ncd\nef", "2$"), "ab\nc|d\nef");
    }

    #[test]
    fn find_motions() {
        assert_eq!(run("|a,b,c,d", "f,"), "a|,b,c,d");
        assert_eq!(run("|a,b,c,d", "2f,"), "a,b|,c,d");
        assert_eq!(run("|a,b,c,d", "t,"), "|a,b,c,d");
        assert_eq!(run("|ab,c", "t,"), "a|b,c");
        assert_eq!(run("a,b,|c", "F,"), "a,b|,c");
        assert_eq!(run("a,b,|c", "T,"), "a,b,|c");
        assert_eq!(run("a,bb,|c", "T,"), "a,bb,|c");
        assert_eq!(run("|a,b,c,d", "f,;"), "a,b|,c,d");
        assert_eq!(run("|a,b,c,d", "f,;,"), "a|,b,c,d");
        assert_eq!(run("|a,b,c,d", "t,;"), "a,|b,c,d");
        assert_eq!(run("|abc", "fz"), "|abc");
    }

    #[test]
    fn percent_and_paragraphs() {
        assert_eq!(run("|(a [b] c)", "%"), "(a [b] c|)");
        assert_eq!(run("(a [b] c|)", "%"), "|(a [b] c)");
        assert_eq!(run("x |(a)", "%"), "x (a|)");
        assert_eq!(run("|a\nb\n\nc\nd", "}"), "a\nb\n|\nc\nd");
        assert_eq!(run("a\nb\n\nc\n|d", "{"), "a\nb\n|\nc\nd");
        assert_eq!(run("|a\n\nb\n\nc", "2}"), "a\n\nb\n|\nc");
    }

    #[test]
    fn screen_motions() {
        let text = (0..20).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let mut v = Vim::new();
        let mut t = text.clone();
        v.cur = 0;
        v.key(Key::Char('L'), &mut t, View { top: 5, lines: 10 });
        assert_eq!(line_idx(&t.chars().collect::<Vec<_>>(), v.cur), 14);
        v.key(Key::Char('H'), &mut t, View { top: 5, lines: 10 });
        assert_eq!(line_idx(&t.chars().collect::<Vec<_>>(), v.cur), 5);
        v.key(Key::Char('M'), &mut t, View { top: 5, lines: 10 });
        assert_eq!(line_idx(&t.chars().collect::<Vec<_>>(), v.cur), 9);
        v.cur = 0;
        let o = v.key(Key::Ctrl('d'), &mut t, View { top: 0, lines: 10 });
        assert_eq!(line_idx(&t.chars().collect::<Vec<_>>(), v.cur), 5);
        assert!(o.effects.contains(&Effect::Scroll(5)));
        v.key(Key::Ctrl('u'), &mut t, View { top: 0, lines: 10 });
        assert_eq!(v.cur, 0);
    }

    // ---------------------------------------------------------------- operatorer

    #[test]
    fn delete_with_motions() {
        assert_eq!(run("|foo bar baz", "dw"), "|bar baz");
        assert_eq!(run("|foo bar baz", "d2w"), "|baz");
        assert_eq!(run("|foo bar baz", "2dw"), "|baz");
        assert_eq!(run("foo |bar baz", "db"), "|bar baz");
        assert_eq!(run("|foo bar", "de"), "| bar");
        assert_eq!(run("foo b|ar", "d$"), "foo |b");
        assert_eq!(run("foo b|ar", "D"), "foo |b");
        assert_eq!(run("foo b|ar", "d0"), "|ar");
        assert_eq!(run("a|bc", "x"), "a|c");
        assert_eq!(run("ab|c", "x"), "a|b");
        assert_eq!(run("a|bc", "X"), "|bc");
        assert_eq!(run("|abcdef", "3x"), "|def");
        assert_eq!(run("foo |bar\nbaz", "dw"), "foo| \nbaz");
        assert_eq!(run("|a,b,c", "df,"), "|b,c");
        assert_eq!(run("|a,b,c", "dt,"), "|,b,c");
        assert_eq!(run("a,b|,c", "dF,"), "a|,c");
    }

    #[test]
    fn dw_stops_at_end_of_line() {
        assert_eq!(run("foo |bar\n  baz", "dw"), "foo| \n  baz");
        assert_eq!(run("|foo\nbar", "dw"), "|\nbar");
    }

    #[test]
    fn linewise_delete_and_yank() {
        assert_eq!(run("a\n|b\nc", "dd"), "a\n|c");
        assert_eq!(run("a\nb\n|c", "dd"), "a\n|b");
        assert_eq!(run("|a\nb\nc", "2dd"), "|c");
        assert_eq!(run("|a\nb\nc", "dj"), "|c");
        assert_eq!(run("a\n|b\nc", "dk"), "|c");
        assert_eq!(run("|a\nb\nc", "dG"), "|".to_owned());
        assert_eq!(run("a\nb\n|c", "dgg"), "|".to_owned());
        assert_eq!(run("|only", "dd"), "|");
        assert_eq!(run("|a\nb\nc", "yyjp"), "a\nb\n|a\nc");
        assert_eq!(run("a\n|b\nc", "yyP"), "a\n|b\nb\nc");
        assert_eq!(run("|a\nb", "ddp"), "b\n|a");
    }

    #[test]
    fn exclusive_multiline_motion_becomes_linewise() {
        assert_eq!(run("|a\nb\n\nc", "d}"), "|\nc");
        assert_eq!(run("x\n|a\nb\n\nc", "d}"), "x\n|\nc");
    }

    #[test]
    fn change_commands() {
        assert_eq!(run("|foo bar", "cwxy\x1b"), "x|y bar");
        assert_eq!(run("fo|o bar", "cwxy\x1b"), "fox|y bar");
        assert_eq!(run("|foo bar baz", "c2wX\x1b"), "|X baz");
        assert_eq!(run("foo| bar", "cwX\x1b"), "foo|Xbar");
        assert_eq!(run("|foo bar", "ccX\x1b"), "|X");
        assert_eq!(run("a\n|foo\nb", "ccX\x1b"), "a\n|X\nb");
        assert_eq!(run("foo |bar", "C!\x1b"), "foo |!");
        assert_eq!(run("|abc", "sX\x1b"), "|Xbc");
        assert_eq!(run("|abc\nd", "SX\x1b"), "|X\nd");
        assert_eq!(run("foo |bar baz", "cbX\x1b"), "|Xbar baz");
        assert_eq!(run("|a b", "cWX\x1b"), "|X b");
    }

    #[test]
    fn replace_char_and_tilde() {
        assert_eq!(run("|abc", "rx"), "|xbc");
        assert_eq!(run("|abc", "3rx"), "xx|x");
        assert_eq!(run("|ab", "3rx"), "|ab");
        assert_eq!(run("|abc", "r\n"), "\n|bc");
        assert_eq!(run("|abc", "~"), "A|bc");
        assert_eq!(run("|abc", "2~"), "AB|c");
        assert_eq!(run("|abc", "5~"), "AB|C");
        assert_eq!(run("|aBc", "g~~"), "|AbC");
        assert_eq!(run("|aBc dEf", "gUw"), "|ABC dEf");
        assert_eq!(run("|aBc dEf", "guu"), "|abc def");
        assert_eq!(run("|aBc dEf", "gUU"), "|ABC DEF");
        assert_eq!(run("|ab\ncd\nef", "gUj"), "|AB\nCD\nef");
        assert_eq!(run("|ab cd", "g~w"), "|AB cd");
    }

    #[test]
    fn join_lines() {
        assert_eq!(run("|a\nb", "J"), "a| b");
        assert_eq!(run("|a\n   b", "J"), "a| b");
        assert_eq!(run("|a\nb\nc", "3J"), "a b| c");
        assert_eq!(run("|a\nb", "gJ"), "a|b");
        assert_eq!(run("|a(\n)", "J"), "a(|)");
        assert_eq!(run("|a\n\nb", "J"), "|a\nb");
        assert_eq!(run("|a", "J"), "|a");
    }

    #[test]
    fn indent_and_outdent() {
        assert_eq!(run("|a\nb", ">>"), "    |a\nb");
        assert_eq!(run("|a\nb\nc", "2>>"), "    |a\n    b\nc");
        assert_eq!(run("|a\nb\nc", ">j"), "    |a\n    b\nc");
        assert_eq!(run("    |a\n  b", "<<"), "|a\n  b");
        assert_eq!(run("\t|a", "<<"), "|a");
        assert_eq!(run("|a\n\nb", "3>>"), "    |a\n\n    b");
        assert_eq!(run("|a", ">>."), "        |a");
    }

    // ---------------------------------------------------------------- textobjekt

    #[test]
    fn word_objects() {
        assert_eq!(run("foo b|ar baz", "diw"), "foo | baz");
        assert_eq!(run("foo b|ar baz", "daw"), "foo |baz");
        assert_eq!(run("foo bar b|az", "daw"), "foo ba|r");
        assert_eq!(run("foo| bar", "diw"), "foo|bar");
        assert_eq!(run("a.b|c.d e", "diW"), "| e");
        assert_eq!(run("a.b|c.d e", "daW"), "|e");
        assert_eq!(run("foo b|ar baz qux", "d2aw"), "foo |qux");
        assert_eq!(run("foo b|ar baz", "ciwX\x1b"), "foo |X baz");
        assert_eq!(run("foo b|ar", "yiwP"), "foo ba|rbar");
    }

    #[test]
    fn paragraph_objects() {
        assert_eq!(run("a\nb|\n\nc", "dip"), "|\nc");
        assert_eq!(run("a\nb|\n\nc", "dap"), "|c");
        assert_eq!(run("a\n\nc|\nd", "dap"), "|a");
        assert_eq!(run("x\n\na|\nb\n\ny", "yipP"), "x\n\n|a\nb\na\nb\n\ny");
    }

    #[test]
    fn quote_objects() {
        assert_eq!(run("say \"he|llo\" now", "di\""), "say \"|\" now");
        assert_eq!(run("say \"he|llo\" now", "da\""), "say |now");
        assert_eq!(run("say \"hello\" |now", "da\""), "say \"hello\" |now");
        assert_eq!(run("|say \"hello\" now", "ci\"X\x1b"), "say \"|X\" now");
        assert_eq!(run("a 'b|c' d", "di'"), "a '|' d");
        assert_eq!(run("a \"b\\\"c|d\" e", "di\""), "a \"|\" e");
        assert_eq!(run("\"a\" \"b|\"", "di\""), "\"a\" \"|\"");
        assert_eq!(run("|x \"a\" y \"b\"", "di\""), "x \"|\" y \"b\"");
        assert_eq!(run("`a|b`", "di`"), "`|`");
        assert_eq!(run("\"a \" |b", "da\""), "\"a \" |b");
    }

    #[test]
    fn bracket_objects() {
        assert_eq!(run("f(a, |b)", "di("), "f(|)");
        assert_eq!(run("f(a, |b)", "da("), "|f");
        assert_eq!(run("f(a, |b)", "dib"), "f(|)");
        assert_eq!(run("f(a(b|c)d)", "di("), "f(a(|)d)");
        assert_eq!(run("f(a(b|c)d)", "d2i("), "f(|)");
        assert_eq!(run("[1, |2]", "di["), "[|]");
        assert_eq!(run("{ |a }", "di{"), "{|}");
        assert_eq!(run("{ |a }", "diB"), "{|}");
        assert_eq!(run("<a |b>", "di<"), "<|>");
        assert_eq!(run("(|a)", "ci(X\x1b"), "(|X)");
        assert_eq!(run("x |(a)", "di("), "x (|)");
        assert_eq!(run("(a|)", "di("), "(|)");
        assert_eq!(run("|(a)", "di("), "(|)");
        assert_eq!(run("fn x() {\n    a|\n    b\n}", "di{"), "fn x() {\n|}");
        assert_eq!(run("f(a[b|c]d)", "di("), "f(|)");
    }

    #[test]
    fn tag_objects() {
        assert_eq!(run("<p>he|llo</p>", "dit"), "<p>|</p>");
        assert_eq!(run("<p>he|llo</p>", "dat"), "|");
        assert_eq!(run("<a><b>x|y</b></a>", "dit"), "<a><b>|</b></a>");
        assert_eq!(run("<a><b>x|y</b></a>", "d2it"), "<a>|</a>");
        assert_eq!(run("<a href=\"x\">t|ext</a>", "cit!\x1b"), "<a href=\"x\">|!</a>");
        assert_eq!(run("<br/><i>a|</i>", "dat"), "<br/|>");
    }

    // ---------------------------------------------------------------- insättning och upprepning

    #[test]
    fn insert_commands() {
        assert_eq!(run("a|bc", "iX\x1b"), "a|Xbc");
        assert_eq!(run("a|bc", "aX\x1b"), "ab|Xc");
        assert_eq!(run("a|bc", "AX\x1b"), "abc|X");
        assert_eq!(run("  a|bc", "IX\x1b"), "  |Xabc");
        assert_eq!(run("a|bc", "gIX\x1b"), "|Xabc");
        assert_eq!(run("a|bc\nd", "oX\x1b"), "abc\n|X\nd");
        assert_eq!(run("a|bc\nd", "OX\x1b"), "|X\nabc\nd");
        assert_eq!(run("|abc", "3ix\x1b"), "xx|xabc");
        assert_eq!(run("|a", "3ox\x1b"), "a\nx\nx\n|x");
        assert_eq!(run("|a", "2Ox\x1b"), "x\n|x\na");
        assert_eq!(run("|abc", "ix\x7f\x7fy\x1b"), "|yabc");
        assert_eq!(run("|", "ia\nb\x1b"), "a\n|b");
    }

    #[test]
    fn replace_mode() {
        assert_eq!(run("|abcd", "Rxy\x1b"), "x|ycd");
        assert_eq!(run("|ab", "Rxyz\x1b"), "xy|z");
        assert_eq!(run("|abcd", "Rxy\x7f\x1b"), "|xbcd");
        assert_eq!(run("|abcd", "Rxy\x7f\x7f\x1b"), "|abcd");
    }

    #[test]
    fn dot_repeat() {
        assert_eq!(run("|a b c d", "dw."), "|c d");
        assert_eq!(run("|a b c d e", "dw2."), "|d e");
        assert_eq!(run("|a b c d e f", "d2w."), "|e f");
        assert_eq!(run("|a b c d e f", "d2w3."), "|f");
        assert_eq!(run("|abc\nabc", "xj."), "bc\n|bc");
        assert_eq!(run("|foo foo foo", "cwbar\x1bw."), "bar ba|r foo");
        assert_eq!(run("|a\nb\nc", "A!\x1bj."), "a!\nb|!\nc");
        assert_eq!(run("|a\nb", "ox\x1bj."), "a\nx\nb\n|x");
        assert_eq!(run("|ab", "3ix\x1b."), "xxxx|xxab");
        assert_eq!(run("|a b c", "ihi \x1bw."), "hi hi| a b c");
        assert_eq!(run("|ab\ncd", "rxj."), "xb\n|xd");
        assert_eq!(run("|ab\ncd", "gUiwj."), "AB\n|CD");
        assert_eq!(run("|a\nb\nc\nd", "ddj."), "b\n|d");
        assert_eq!(run("|a b c d", "dwu."), "|b c d");
    }

    #[test]
    fn dot_repeat_after_visual() {
        assert_eq!(run("|abcdef\nabcdef", "vlldj0."), "def\n|def");
        assert_eq!(run("|a\nb\nc\nd\ne", "Vjdj."), "|c");
    }

    #[test]
    fn undo_redo() {
        assert_eq!(run("|abc", "xu"), "|abc");
        assert_eq!(run("|abc", "xxu"), "|bc");
        assert_eq!(run("|abc", "xxuu\x12"), "|bc");
        assert_eq!(run("|abc", "xx\x12"), "|c");
        assert_eq!(run("|abc", "ixyz\x1bu"), "|abc");
        assert_eq!(run("|a b", "dwiX\x1bu"), "|b");
        assert_eq!(run("|a b", "dwiX\x1buu"), "|a b");
        assert_eq!(run("|a\nb", "ddpuu"), "|a\nb");
        assert_eq!(run("|a", "uu"), "|a");
    }

    #[test]
    fn escape_cancels_pending() {
        assert_eq!(run("|abc", "d\x1bx"), "|bc");
        assert_eq!(run("|abc", "2\x1bx"), "|bc");
        assert_eq!(run("|abc", "dzx"), "|bc");
    }

    // ---------------------------------------------------------------- register

    #[test]
    fn put_variants() {
        assert_eq!(run("|ab cd", "yw$p"), "ab cdab| ");
        assert_eq!(run("|ab", "ylp"), "a|ab");
        assert_eq!(run("|ab", "ylP"), "|aab");
        assert_eq!(run("|ab", "yl3p"), "aaa|ab");
        assert_eq!(run("|a b", "dwwp"), "ba| ");
        assert_eq!(run("|a\nb", "yy2p"), "a\n|a\na\nb");
        assert_eq!(run("|a\nb", "yyjp"), "a\nb\n|a");
        assert_eq!(run("a|b", "ylgpx"), "a|b");
        assert_eq!(run("|", "p"), "|");
    }

    #[test]
    fn named_registers() {
        let mut v = Vim::new();
        more(&mut v, "|foo bar", "\"ayw");
        more(&mut v, "|foo bar", "w\"byw");
        assert_eq!(reg(&v, 'a'), "foo ");
        assert_eq!(reg(&v, 'b'), "bar");
        assert_eq!(more(&mut v, "|x", "\"ap"), "xfoo| ");
        assert_eq!(more(&mut v, "|x", "\"bP"), "ba|rx");
        more(&mut v, "|X", "\"Ayl");
        assert_eq!(reg(&v, 'a'), "foo X");
        assert_eq!(more(&mut v, "|a\nb", "\"ayy"), "|a\nb");
        more(&mut v, "|a\nb", "\"Ayy");
        assert_eq!(v.register('a').unwrap().kind, RegKind::Line);
    }

    #[test]
    fn unnamed_numbered_and_small_delete_registers() {
        let mut v = Vim::new();
        more(&mut v, "|a b\nc\nd\ne", "yw");
        assert_eq!(reg(&v, '0'), "a ");
        more(&mut v, "|a b\nc\nd\ne", "dd");
        assert_eq!(reg(&v, '1'), "a b\n");
        more(&mut v, "|c\nd\ne", "dd");
        assert_eq!(reg(&v, '1'), "c\n");
        assert_eq!(reg(&v, '2'), "a b\n");
        more(&mut v, "|xy", "x");
        assert_eq!(reg(&v, '-'), "x");
        assert_eq!(reg(&v, '"'), "x");
        assert_eq!(reg(&v, '0'), "a ");
    }

    #[test]
    fn black_hole_and_clipboard_registers() {
        let mut v = Vim::new();
        more(&mut v, "|a b", "yw");
        more(&mut v, "|x y", "\"_dw");
        assert_eq!(reg(&v, '"'), "a ");
        let (_, out) = run_with(&mut v, "|clip", "\"+yw");
        assert!(out.effects.contains(&Effect::Clipboard("clip".into())));
        assert_eq!(more(&mut v, "|x", "\"+p"), "xcli|p");
        fn fake() -> Option<String> {
            Some("from system".into())
        }
        v.clip_get = Some(fake);
        assert_eq!(more(&mut v, "|x", "\"+P"), "from syste|mx");
    }

    // ---------------------------------------------------------------- visuellt läge

    #[test]
    fn visual_char_mode() {
        assert_eq!(run("|abc def", "vld"), "|c def");
        assert_eq!(run("|abc def", "vey"), "|abc def");
        assert_eq!(run("|abc def", "veyP"), "ab|cabc def");
        assert_eq!(run("|abc def", "vwd"), "|ef");
        assert_eq!(run("a|bc def", "vhd"), "|c def");
        assert_eq!(run("|abc", "v$d"), "|".to_owned());
        assert_eq!(run("|abc", "vlU"), "|ABc");
        assert_eq!(run("|ABc", "vlu"), "|abc");
        assert_eq!(run("|aBc", "v~"), "|ABc");
        assert_eq!(run("|abc", "vlcX\x1b"), "|Xc");
        assert_eq!(run("|abc", "vlrx"), "|xxc");
        assert_eq!(run("a|b\ncd", "vjd"), "|a");
        assert_eq!(run("|abc", "vlo\x1bx"), "|bc");
        assert_eq!(run("|foo bar", "viwd"), "| bar");
        assert_eq!(run("|foo bar", "vawd"), "|bar");
        assert_eq!(run("f(|a)", "vi(d"), "f(|)");
        assert_eq!(run("|ab\ncd", "vjJ"), "ab| cd");
        assert_eq!(run("|abc", "vlx"), "|c");
        assert_eq!(run("|abc", "v\x1bx"), "|bc");
    }

    #[test]
    fn visual_line_mode() {
        assert_eq!(run("a\n|b\nc", "Vd"), "a\n|c");
        assert_eq!(run("|a\nb\nc", "Vjd"), "|c");
        assert_eq!(run("|a\nb\nc", "Vjy"), "|a\nb\nc");
        assert_eq!(run("|a\nb\nc", "VjyGp"), "a\nb\nc\n|a\nb");
        assert_eq!(run("a|b\nc", "Vx"), "|c");
        assert_eq!(run("|a\nb", "V>"), "    |a\nb");
        assert_eq!(run("|a\nb\nc", "Vj>"), "    |a\n    b\nc");
        assert_eq!(run("|a\nb\nc", "Vj2>"), "        |a\n        b\nc");
        assert_eq!(run("|a\nb", "VcX\x1b"), "|X\nb");
        assert_eq!(run("|a\nb\nc", "VjJ"), "a| b\nc");
        assert_eq!(run("|ab\ncd", "VjU"), "|AB\nCD");
        assert_eq!(run("a\n|b\nc", "Vkd"), "|c");
        assert_eq!(run("|a\nb", "VGd"), "|".to_owned());
        assert_eq!(run("|a\nb\nc", "vVd"), "|b\nc");
        assert_eq!(run("|a\nb", "Vvd"), "|\nb");
    }

    #[test]
    fn visual_switch_and_reselect() {
        assert_eq!(run("|abc", "vvx"), "|bc");
        assert_eq!(run("|ab\ncd", "vjVd"), "|".to_owned());
        assert_eq!(run("|abc", "vl\x1bgvd"), "|c");
        assert_eq!(run("|abc", "vlohd"), "|c");
    }

    #[test]
    fn visual_paste_replaces_selection() {
        assert_eq!(run("|a b", "yiwwviwp"), "a |a");
        let mut v = Vim::new();
        assert_eq!(more(&mut v, "|a b", "yiwwviwp"), "a |a");
        assert_eq!(reg(&v, '"'), "b");
        assert_eq!(run("|a\nb\nc", "yyjVp"), "a\n|a\nc");
        assert_eq!(run("|a\nb\nc", "yyjvp"), "a\n\n|a\n\nc");
    }

    #[test]
    fn visual_block_mode() {
        assert_eq!(run("|abc\ndef\nghi", "\x16jld"), "|c\nf\nghi");
        assert_eq!(run("a|bc\ndef\nghi", "\x16jd"), "a|c\ndf\nghi");
        assert_eq!(run("|abc\ndef\nghi", "\x16jjly"), "|abc\ndef\nghi");
        assert_eq!(run("|abc\ndef\nghi", "\x16jjlyGp"), "abc\ndef\ng|abhi\n de\n gh");
        assert_eq!(run("|abc\ndef", "\x16jIX\x1b"), "|Xabc\nXdef");
        assert_eq!(run("|abc\ndef", "\x16jAX\x1b"), "a|Xbc\ndXef");
        assert_eq!(run("|abc\nde\nfgh", "l\x16jjIX\x1b"), "a|Xbc\ndXe\nfXgh");
        assert_eq!(run("|abc\ndef", "\x16j$AX\x1b"), "abc|X\ndefX");
        assert_eq!(run("|abc\ndef", "\x16jcX\x1b"), "|Xbc\nXef");
        assert_eq!(run("|abc\ndef", "\x16jlrx"), "|xxc\nxxf");
        assert_eq!(run("|abc\ndef", "\x16jlU"), "|ABc\nDEf");
        assert_eq!(run("|abc\ndef", "\x16jlU"), "|ABc\nDEf");
        assert_eq!(run("|abc\ndef", "\x16jOd"), "|bc\nef");
        assert_eq!(run("|abcd\nef\nghij", "ll\x16jjd"), "ab|d\nef\nghj");
    }

    #[test]
    fn visual_block_dot_and_text() {
        assert_eq!(run("|ab\ncd\nef", "\x16jlojd"), "ab\n|\nef");
    }

    // ---------------------------------------------------------------- markeringar, sökning, makron

    #[test]
    fn marks() {
        assert_eq!(run("|a\nb\n  c", "majjd'a"), "|".to_owned());
        assert_eq!(run("a\n|b\n  c", "mbggd'b"), "  |c");
        assert_eq!(run("a|bc\nd", "mxj`x"), "a|bc\nd");
        assert_eq!(run("|a\nb\nc", "Gk''"), "|a\nb\nc");
        assert_eq!(run("|a b c d", "mawwwd`a"), "|d");
        assert_eq!(run("|abc", "'z"), "|abc");
    }

    #[test]
    fn jump_back_with_ctrl_o() {
        assert_eq!(run("|a\nb\nc", "G\x0f"), "|a\nb\nc");
    }

    #[test]
    fn search_forward_and_backward() {
        let mut v = Vim::new();
        assert_eq!(more(&mut v, "|a foo b foo c foo", "/foo\n"), "a |foo b foo c foo");
        assert_eq!(more(&mut v, "a |foo b foo c foo", "n"), "a foo b |foo c foo");
        assert_eq!(more(&mut v, "a foo b |foo c foo", "nn"), "a |foo b foo c foo");
        assert_eq!(more(&mut v, "a |foo b foo c foo", "N"), "a foo b foo c |foo");
        assert_eq!(more(&mut v, "a foo b foo c |foo", "?b\n"), "a foo |b foo c foo");
        assert_eq!(more(&mut v, "a foo |b foo c foo", "n"), "a foo |b foo c foo");
        assert_eq!(run("|ab\nfoo", "/o+\n"), "ab\nf|oo");
        assert_eq!(run("|a\nb\nc", "/c\n"), "a\nb\n|c");
        assert_eq!(run("a\nb\n|c", "/a\n"), "|a\nb\nc");
        assert_eq!(run("|a bar bar", "/\\bbar\\b\nn"), "a bar |bar");
    }

    #[test]
    fn search_with_operator() {
        assert_eq!(run("|a b c", "d/c\n"), "|c");
        assert_eq!(run("|a b c", "y/b\n$p"), "a b ca| ");
        assert_eq!(run("a b |c", "d?a\n"), "|c");
    }

    #[test]
    fn star_and_hash() {
        assert_eq!(run("|foo bar foo", "*"), "foo bar |foo");
        assert_eq!(run("|foo bar foo", "**"), "|foo bar foo");
        assert_eq!(run("foo bar |foo", "#"), "|foo bar foo");
        assert_eq!(run("|foo foobar foo", "*"), "foo foobar |foo");
        assert_eq!(run("f|oo bar foo", "*"), "foo bar |foo");
        assert_eq!(run("x |foo foo", "*n"), "x |foo foo");
        assert_eq!(run("|foo x foo", "d*"), "|foo");
    }

    #[test]
    fn macros() {
        let mut v = Vim::new();
        assert_eq!(more(&mut v, "|a\nb\nc", "qaA!\x1bjq"), "a!\n|b\nc");
        assert_eq!(more(&mut v, "a!\n|b\nc", "@a"), "a!\nb!\n|c");
        assert_eq!(more(&mut v, "|a\nb\nc\nd", "qxI-\x1bjq2@x"), "-a\n-b\n-c\n|d");
        assert_eq!(more(&mut v, "|1\n2", "@@"), "-1\n|2");
        assert_eq!(reg(&v, 'x'), "I-\x1bj");
    }

    #[test]
    fn macro_stops_on_failed_motion() {
        let mut v = Vim::new();
        // Rekursivt makro: `f;` misslyckas till slut och avbryter.
        more(&mut v, "|a;b;c", "qqxf;q");
        assert_eq!(more(&mut v, ";b|;c", "5@q"), ";b|c");
    }

    // ---------------------------------------------------------------- kommandorad

    #[test]
    fn ex_write_quit_effects() {
        let mut v = Vim::new();
        assert_eq!(run_with(&mut v, "|a", ":w\n").1.effects, vec![Effect::Save]);
        assert_eq!(run_with(&mut v, "|a", ":wq\n").1.effects, vec![Effect::SaveQuit]);
        assert_eq!(run_with(&mut v, "|a", ":x\n").1.effects, vec![Effect::SaveQuit]);
        assert_eq!(run_with(&mut v, "|a", ":q\n").1.effects, vec![Effect::Quit { force: false }]);
        assert_eq!(run_with(&mut v, "|a", ":q!\n").1.effects, vec![Effect::Quit { force: true }]);
        assert_eq!(run_with(&mut v, "|a", ":qa\n").1.effects, vec![Effect::QuitAll { force: false }]);
        assert_eq!(run_with(&mut v, "|a", "ZZ").1.effects, vec![Effect::SaveQuit]);
        assert_eq!(run_with(&mut v, "|a", "ZQ").1.effects, vec![Effect::Quit { force: true }]);
        assert_eq!(run_with(&mut v, "|a", ":w x.txt\n").1.effects, vec![Effect::SaveAs("x.txt".into())]);
        assert_eq!(run_with(&mut v, "|a", ":e b.txt\n").1.effects, vec![Effect::Edit("b.txt".into())]);
        assert_eq!(run_with(&mut v, "|a", ":sp\n").1.effects, vec![Effect::Split { vertical: false }]);
        assert_eq!(run_with(&mut v, "|a", ":vs\n").1.effects, vec![Effect::Split { vertical: true }]);
        assert_eq!(run_with(&mut v, "|a", ":vsp c\n").1.effects, vec![Effect::Split { vertical: true }, Effect::Edit("c".into())]);
        assert_eq!(run_with(&mut v, "|a", ":only\n").1.effects, vec![Effect::CloseSplit]);
        assert_eq!(v.mode, Mode::Normal);
    }

    #[test]
    fn ex_set_and_noh() {
        let mut v = Vim::new();
        assert_eq!(run_with(&mut v, "|a", ":set nu\n").1.effects, vec![Effect::Set(Setting::Number, Some(true))]);
        assert_eq!(run_with(&mut v, "|a", ":set nonu\n").1.effects, vec![Effect::Set(Setting::Number, Some(false))]);
        assert_eq!(run_with(&mut v, "|a", ":set nonumber\n").1.effects, vec![Effect::Set(Setting::Number, Some(false))]);
        assert_eq!(run_with(&mut v, "|a", ":set wrap\n").1.effects, vec![Effect::Set(Setting::Wrap, Some(true))]);
        assert_eq!(run_with(&mut v, "|a", ":set nowrap\n").1.effects, vec![Effect::Set(Setting::Wrap, Some(false))]);
        assert_eq!(run_with(&mut v, "|a", ":set wrap!\n").1.effects, vec![Effect::Set(Setting::Wrap, None)]);
        assert_eq!(run_with(&mut v, "|a", ":set invnu\n").1.effects, vec![Effect::Set(Setting::Number, None)]);
        assert!(run_with(&mut v, "|a", ":noh\n").1.effects.is_empty());
        assert!(matches!(run_with(&mut v, "|a", ":set bogus\n").1.effects[0], Effect::Notify(_, true)));
        assert_eq!(run("|a", ":set sw=2\n>>"), "  |a");
        assert_eq!(run("|Foo foo", ":set ic\n/foo\n"), "Foo |foo");
    }

    #[test]
    fn ex_goto_line_and_ranges() {
        assert_eq!(run("|a\nb\nc", ":3\n"), "a\nb\n|c");
        assert_eq!(run("|a\nb\nc", ":2\n"), "a\n|b\nc");
        assert_eq!(run("|a\nb\nc", ":99\n"), "a\nb\n|c");
        assert_eq!(run("|a\nb\nc", ":$\n"), "a\nb\n|c");
        assert_eq!(run("a\nb\n|c", ":1\n"), "|a\nb\nc");
        assert_eq!(run("|a\nb\nc\nd", ":.+2\n"), "a\nb\n|c\nd");
        assert_eq!(run("|a\n  b", ":2\n"), "a\n  |b");
        assert_eq!(run("|a\nb\nc", ":2,3d\n"), "|a");
        assert_eq!(run("|a\nb\nc", ":%d\n"), "|");
        assert_eq!(run("|a\nb\nc", ":2d\n"), "a\n|c");
        assert_eq!(run("|a\nb\nc", ":1,2j\n"), "a| b\nc");
        assert_eq!(run("|a\nb\nc", ":2>\n"), "a\n    |b\nc");
        assert_eq!(run("|a\nb\nc", ":2,3y\nGp"), "a\nb\nc\n|b\nc");
        assert_eq!(run("|a\nb\nc", "jmajmb:'a,'bd\n"), "|a");
        assert_eq!(run("|a\nb\nc", ":/c/d\n"), "a\n|b");
        assert_eq!(run("|x", ":nosuchcmd\n"), "|x");
    }

    #[test]
    fn ex_substitute() {
        assert_eq!(run("|foo foo", ":s/foo/bar/\n"), "|bar foo");
        assert_eq!(run("|foo foo", ":s/foo/bar/g\n"), "|bar bar");
        assert_eq!(run("|foo\nfoo", ":s/foo/bar/\n"), "|bar\nfoo");
        assert_eq!(run("|foo\nfoo", ":%s/foo/bar/\n"), "bar\n|bar");
        assert_eq!(run("|a1 b2", ":s/(\\w)(\\d)/\\2\\1/g\n"), "|1a 2b");
        assert_eq!(run("|abc", ":s/b/[&]/\n"), "|a[b]c");
        assert_eq!(run("|abc", ":s/b/\\&/\n"), "|a&c");
        assert_eq!(run("|a,b", ":s/,/\\r/\n"), "a\n|b");
        assert_eq!(run("|a,b", ":s/,/\\t/\n"), "|a\tb");
        assert_eq!(run("|a b", ":s# #/#\n"), "|a/b");
        assert_eq!(run("|a/b", ":s/\\//-/\n"), "|a-b");
        assert_eq!(run("|Foo", ":s/foo/x/i\n"), "|x");
        assert_eq!(run("|Foo", ":s/foo/x/\n"), "|Foo");
        assert_eq!(run("|foo bar", ":s/\\w+/\\u&/g\n"), "|Foo Bar");
        assert_eq!(run("|foo bar", ":s/\\w+/\\U&/\n"), "|FOO bar");
        assert_eq!(run("|a\nb\nc", ":2,3s/$/!/\n"), "a\nb!\n|c!");
        assert_eq!(run("|a\nb", ":%s/^/> /\n"), "> a\n|> b");
        assert_eq!(run("|aaa", ":s/a*/X/\n"), "|X");
        assert_eq!(run("|foo", ":s/o//g\n"), "|f");
        assert_eq!(run("|foo\nfoo", "jma:1,2s/o/0/g\n"), "f00\n|f00");
        assert_eq!(run("|foo foo\nfoo", ":s/foo/x/\nj:&&\n"), "x foo\n|x");
        assert_eq!(run("|a b", "/b\n:s//X/\n"), "|a X");
        assert_eq!(run("|ab", ":s/(/x/\n"), "|ab");
        assert_eq!(run("|ab", ":s/z/x/\n"), "|ab");
        assert_eq!(run("|a b a", ":s/a/x/g\nu"), "|a b a");
    }

    #[test]
    fn ex_substitute_reports_counts() {
        let mut v = Vim::new();
        let (_, out) = run_with(&mut v, "|a a\na", ":%s/a/b/g\n");
        assert_eq!(out.effects, vec![Effect::Notify(tf!("{subs} ersättningar på {lines} rader", subs = 3, lines = 2), false)]);
        let (_, out) = run_with(&mut v, "|a", ":s/a/b/\n");
        assert_eq!(out.effects, vec![Effect::Notify(t("1 ersättning på 1 rad").into(), false)]);
        let (_, out) = run_with(&mut v, "|a", ":s/q/b/\n");
        assert!(matches!(&out.effects[0], Effect::Notify(m, true) if m.starts_with("E486")));
    }

    #[test]
    fn visual_colon_prefills_range() {
        let mut v = Vim::new();
        let (r, _) = run_with(&mut v, "|a\nb\nc", "Vj:");
        assert_eq!(v.mode, Mode::Command);
        assert_eq!(v.cmdline, "'<,'>");
        let _ = r;
        let mut v = Vim::new();
        assert_eq!(more(&mut v, "|a\nb\nc", "Vj:s/$/!/\n"), "a!\n|b!\nc");
        assert_eq!(run("|a\nb\nc", "3:"), "|a\nb\nc");
    }

    #[test]
    fn cmdline_editing_and_cancel() {
        let mut v = Vim::new();
        run_with(&mut v, "|a", ":abc");
        assert_eq!(v.cmdline, "abc");
        v.keys("\x7f", &mut "a".to_owned());
        assert_eq!(v.cmdline, "ab");
        v.keys("\x1b", &mut "a".to_owned());
        assert_eq!(v.mode, Mode::Normal);
        assert_eq!(v.cmdline, "");
        run_with(&mut v, "|a", ":\x7f");
        assert_eq!(v.mode, Mode::Normal);
        assert_eq!(more(&mut v, "|a\nb", "/b\x1bx"), "|\nb");
        run_with(&mut v, "|a", ":set nu\n");
        v.keys(":\u{e002}", &mut "a".to_owned());
        assert_eq!(v.cmdline, "set nu");
    }

    #[test]
    fn mode_labels_and_pending_display() {
        let mut v = Vim::new();
        let mut tx = "abc".to_owned();
        assert_eq!(v.mode_label(), "");
        v.keys("i", &mut tx);
        assert_eq!(v.mode, Mode::Insert);
        assert_eq!(v.mode_label(), t("-- INFOGA --"));
        v.keys(ESC, &mut tx);
        v.keys("v", &mut tx);
        assert_eq!(v.mode_label(), t("-- MARKERA --"));
        v.keys(ESC, &mut tx);
        v.keys("V", &mut tx);
        assert_eq!(v.mode_label(), t("-- MARKERA RAD --"));
        v.keys(ESC, &mut tx);
        v.keys("\x16", &mut tx);
        assert_eq!(v.mode_label(), t("-- MARKERA BLOCK --"));
        v.keys(ESC, &mut tx);
        v.keys("R", &mut tx);
        assert_eq!(v.mode_label(), t("-- ERSÄTT --"));
        v.keys(ESC, &mut tx);
        v.keys("\"a", &mut tx);
        assert_eq!(v.pending(), "\"a");
        v.keys("2d", &mut tx);
        assert_eq!(v.pending(), "\"a2d");
        v.keys(ESC, &mut tx);
        assert_eq!(v.pending(), "");
        v.keys("qa", &mut tx);
        assert_eq!(v.recording_reg(), Some('a'));
        v.keys("q", &mut tx);
        assert_eq!(v.recording_reg(), None);
    }

    #[test]
    fn selection_ranges_for_painting() {
        let mut v = Vim::new();
        let mut tx = "abc\ndef\nghi".to_owned();
        v.keys("vl", &mut tx);
        assert_eq!(v.selection_ranges(&tx), vec![(0, 2)]);
        v.keys(ESC, &mut tx);
        assert!(v.selection_ranges(&tx).is_empty());
        v.keys("V", &mut tx);
        assert_eq!(v.selection_ranges(&tx), vec![(0, 4)]);
        v.keys(ESC, &mut tx);
        v.cur = 0;
        v.keys("\x16jl", &mut tx);
        assert_eq!(v.selection_ranges(&tx), vec![(0, 2), (4, 6)]);
    }

    #[test]
    fn insert_does_not_break_on_unicode() {
        assert_eq!(run("|åäö", "x"), "|äö");
        assert_eq!(run("|åäö", "lylp"), "åä|äö");
        assert_eq!(run("|åäö", "gUU"), "|ÅÄÖ");
        assert_eq!(run("|åäö x", "dw"), "|x");
        assert_eq!(run("|a ö", "$ix\x1b"), "a |xö");
    }

    #[test]
    fn counts_with_registers_and_motions() {
        assert_eq!(run("|a b c d e", "3dw"), "|d e");
        assert_eq!(run("|a b c d e", "2d2w"), "|e");
        assert_eq!(run("|abcdef", "\"a3x"), "|def");
        assert_eq!(run("|a\nb\nc\nd", "2yyGp"), "a\nb\nc\nd\n|a\nb");
        assert_eq!(run("|0123456789", "5|"), "0123|456789");
    }

    #[test]
    fn no_panic_on_empty_text_and_edge_cases() {
        for keys in ["x", "dd", "p", "u", "J", "w", "b", "e", "}", "{", "G", "gg", "$", "0", "%", "dw", "cw\x1b", "ci(", "vd", "Vd", "\x16d", "yy", ".", "~", "r!", "O\x1b", "o\x1b", "*", "n", ":s/a/b/\n", ":%d\n", "viwd", "dap", "dit", "di\"", "\x04", "\x15", "H", "M", "L"] {
            let mut v = Vim::new();
            let mut t = String::new();
            v.keys(keys, &mut t);
            let mut v = Vim::new();
            let mut t = "\n".to_owned();
            v.keys(keys, &mut t);
        }
    }
}
