//! Ångra-historiken för redigeringsfälten som visar ett helt dokument (dokumentläget och textläget).
//!
//! egui:s `TextEdit` sparar varje ångra-punkt som en hel kopia av texten, upp till 100 stycken: i ett
//! dokument på 2000 sidor (3 MB text) är det över 300 MB. Här sparar historiken i stället det som ändrats
//! mellan två punkter. Bara den senaste punkten är en hel text; varje äldre punkt är ändringen som tar
//! tillbaka den från nästa (gemensamt början och slut, och det som skilde i mitten).
//!
//! Beteendet är egui:s `Undoer<(CCursorRange, String)>` (samma regler för när en punkt skapas, för ångra och gör om,
//! samma gränser: 100 punkter, 1 s stabil text, 30 s i ständig förändring); `tests::som_egui` kör båda mot varandra.
//! Det enda som skiljer: texten som är i förändring jämförs med ett fingeravtryck (längd och hash) i stället för en
//! sparad kopia.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use egui::text::CCursorRange;
use egui::{Context, Id, Key, Modifiers};

/// Samma värden som egui:s `undoer::Settings::default()`.
const MAX_UNDOS: usize = 100;
const STABLE_TIME: f32 = 1.0;
const AUTO_SAVE_INTERVAL: f32 = 30.0;

/// Ett läge: markeringen och hela texten.
pub type State = (CCursorRange, String);

/// Ändringen som tar tillbaka en äldre text från nästa: `äldre = nästa[..pre] + mitten + nästa[nästa.len() - suf..]`.
struct Patch {
    range: CCursorRange,
    pre: usize,
    suf: usize,
    mid: String,
}

impl Patch {
    /// Ändringen som ger `old` ur `new`.
    fn between(old: &State, new: &str) -> Patch {
        let (a, b) = (old.1.as_bytes(), new.as_bytes());
        let max = a.len().min(b.len());
        let mut pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
        while pre > 0 && !(old.1.is_char_boundary(pre) && new.is_char_boundary(pre)) {
            pre -= 1;
        }
        let mut suf = a.iter().rev().zip(b.iter().rev()).take(max - pre).take_while(|(x, y)| x == y).count();
        while suf > 0 && !(old.1.is_char_boundary(a.len() - suf) && new.is_char_boundary(b.len() - suf)) {
            suf -= 1;
        }
        Patch { range: old.0, pre, suf, mid: old.1[pre..a.len() - suf].to_owned() }
    }

    /// Den äldre texten ur den nyare.
    fn apply(&self, new: &str) -> State {
        let mut t = String::with_capacity(self.pre + self.mid.len() + self.suf);
        t.push_str(&new[..self.pre]);
        t.push_str(&self.mid);
        t.push_str(&new[new.len() - self.suf..]);
        (self.range, t)
    }
}

/// En följd lägen, senast tillagda sist. Det senaste är lagrat helt, de andra som ändringar.
#[derive(Default)]
struct Chain {
    top: Option<State>,
    /// `older[i]` ger läget före `older[i + 1]` (och `older.back()` ger läget före `top`).
    older: VecDeque<Patch>,
}

impl Chain {
    fn len(&self) -> usize {
        self.older.len() + usize::from(self.top.is_some())
    }

    fn is_empty(&self) -> bool {
        self.top.is_none()
    }

    fn back_is(&self, range: &CCursorRange, text: &str) -> bool {
        self.top.as_ref().is_some_and(|(r, t)| r == range && t == text)
    }

    fn push(&mut self, s: State) {
        if let Some(prev) = self.top.replace(s) {
            let top = &self.top.as_ref().expect("nyss satt").1;
            self.older.push_back(Patch::between(&prev, top));
        }
    }

    fn pop(&mut self) -> Option<State> {
        let top = self.top.take()?;
        self.top = self.older.pop_back().map(|p| p.apply(&top.1));
        Some(top)
    }

    /// Tar bort det äldsta läget.
    fn pop_front(&mut self) {
        if self.older.pop_front().is_none() {
            self.top = None;
        }
    }

    fn clear(&mut self) {
        self.top = None;
        self.older.clear();
    }
}

/// Texten som håller på att ändras, utan kopia.
struct Flux {
    start_time: f64,
    latest_change_time: f64,
    latest: (CCursorRange, usize, u64),
}

fn print(range: &CCursorRange, text: &str) -> (CCursorRange, usize, u64) {
    (*range, text.len(), crate::word::layout::fast_hash(text))
}

#[derive(Default)]
pub struct History {
    undos: Chain,
    redos: Chain,
    flux: Option<Flux>,
}

impl History {
    fn has_undo(&self, range: &CCursorRange, text: &str) -> bool {
        match self.undos.len() {
            0 => false,
            1 => !self.undos.back_is(range, text),
            _ => true,
        }
    }

    /// Lägger in läget som en ångra-punkt om det skiljer sig från den senaste.
    pub fn add_undo(&mut self, range: &CCursorRange, text: &str) {
        if !self.undos.back_is(range, text) {
            self.undos.push((*range, text.to_owned()));
        }
        while self.undos.len() > MAX_UNDOS {
            self.undos.pop_front();
        }
        self.flux = None;
    }

    /// Anropas varje bildruta (före och efter att fältet behandlat indata): skapar en ångra-punkt när texten
    /// varit stabil en sekund, eller var 30:e sekund om den hela tiden ändras.
    pub fn feed_state(&mut self, time: f64, range: &CCursorRange, text: &str) {
        if self.undos.is_empty() {
            self.add_undo(range, text);
        } else if self.undos.back_is(range, text) {
            self.flux = None;
        } else {
            self.redos.clear();
            let now = print(range, text);
            match self.flux.as_mut() {
                None => self.flux = Some(Flux { start_time: time, latest_change_time: time, latest: now }),
                Some(f) => {
                    if f.latest == now {
                        if (time - f.latest_change_time) as f32 >= STABLE_TIME {
                            self.add_undo(range, text);
                        }
                    } else if (time - f.start_time) as f32 >= AUTO_SAVE_INTERVAL {
                        self.add_undo(range, text);
                    } else {
                        f.latest_change_time = time;
                        f.latest = now;
                    }
                }
            }
        }
    }

    /// Ångra: läget att gå till, om det finns ett.
    pub fn undo(&mut self, range: &CCursorRange, text: &str) -> Option<State> {
        if !self.has_undo(range, text) {
            return None;
        }
        self.flux = None;
        if self.undos.back_is(range, text) {
            let s = self.undos.pop().expect("senaste finns");
            self.redos.push(s);
        } else {
            self.redos.push((*range, text.to_owned()));
        }
        // Punkten behålls (som i egui): den är nu det aktuella läget.
        self.undos.top.clone()
    }

    /// Ett steg ångra eller gör om. Samma text men annan markör räknas som ett eget steg och hoppas över.
    pub fn step(&mut self, redo: bool, range: &CCursorRange, text: &str) -> Option<State> {
        let go = |h: &mut Self| if redo { h.redo(range, text) } else { h.undo(range, text) };
        let mut res = go(self);
        if res.as_ref().is_some_and(|(_, t)| t == text) {
            res = go(self);
        }
        res
    }

    /// Gör om: läget att gå till, om det finns ett.
    pub fn redo(&mut self, range: &CCursorRange, text: &str) -> Option<State> {
        if !self.undos.is_empty() && !self.undos.back_is(range, text) {
            // Texten har ändrats sedan sista ångra: inget att göra om.
            self.redos.clear();
            None
        } else if let Some(s) = self.redos.pop() {
            self.undos.push(s);
            self.undos.top.clone()
        } else {
            None
        }
    }
}

// ----------------------------------------------------------------------------- koppling till fältet

fn key(id: Id) -> Id {
    id.with("kladd-undo")
}

/// Historiken för redigeringsfältet `id` (en per dokument och ruta).
pub fn history(ctx: &Context, id: Id) -> Arc<Mutex<History>> {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(key(id), || Arc::new(Mutex::new(History::default()))).clone())
}

/// Läget före en ändring som ett eget ångra-steg (formatering, ersättning, autokorrigering, återställning).
pub fn add_undo(ctx: &Context, id: Id, before: &CCursorRange, text: &str) {
    if let Ok(mut h) = history(ctx, id).lock() {
        h.add_undo(before, text);
    }
}

/// Tar Ctrl+Z, Ctrl+Y och Ctrl+Skift+Z ur indata, så att `TextEdit` inte ångrar med sin egen historik: ett värde per
/// tryck i ordning, sant för gör om och falskt för ångra (flera tryck i samma bildruta är lika många steg).
pub fn take_keys(ctx: &Context) -> Vec<bool> {
    ctx.input_mut(|i| {
        let mut steps = Vec::new();
        i.events.retain(|e| {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = e else { return true };
            // Gör om först (som egui): Ctrl+Skift+Z räknas inte som ångra.
            if (*key == Key::Y && modifiers.matches_logically(Modifiers::COMMAND))
                || (*key == Key::Z && modifiers.matches_logically(Modifiers::SHIFT | Modifiers::COMMAND))
            {
                steps.push(true);
                false
            } else if *key == Key::Z && modifiers.matches_logically(Modifiers::COMMAND) {
                steps.push(false);
                false
            } else {
                true
            }
        });
        steps
    })
}

/// Matas före `TextEdit::show` (markeringen som den är nu); ångra och gör om görs här, innan fältet ritas.
/// Returnerar läget att gå till om en tangent ångrade eller gjorde om.
pub fn before_show(ctx: &Context, id: Id, focused: bool, range: &CCursorRange, text: &str) -> Option<State> {
    let h = history(ctx, id);
    let mut h = h.lock().ok()?;
    h.feed_state(ctx.input(|i| i.time), range, text);
    if !focused {
        return None;
    }
    let mut cur: Option<State> = None;
    for redo in take_keys(ctx) {
        let (r, t) = cur.as_ref().map_or((range, text), |(r, t)| (r, t.as_str()));
        // Som egui:s eget fält: ett läge med samma text men annan markör är ett eget steg (menyn hoppar över det, se `step`).
        if let Some(next) = if redo { h.redo(r, t) } else { h.undo(r, t) } {
            cur = Some(next);
        }
    }
    cur
}

/// Ångra (eller gör om) från menyn och verktygsraden: läget att gå till.
pub fn step(ctx: &Context, id: Id, redo: bool, range: &CCursorRange, text: &str) -> Option<State> {
    history(ctx, id).lock().ok()?.step(redo, range, text)
}

/// Matas efter `TextEdit::show`, och tömmer fältets egen historik (annars samlar den hela kopior av texten).
pub fn after_show(ctx: &Context, id: Id, range: &CCursorRange, text: &str) {
    if let Ok(mut h) = history(ctx, id).lock() {
        h.feed_state(ctx.input(|i| i.time), range, text);
    }
    if let Some(mut st) = egui::text_edit::TextEditState::load(ctx, id) {
        st.clear_undoer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::bench::Rng;
    use egui::text::CCursor;
    use egui::util::undoer::Undoer;

    fn range(c: usize) -> CCursorRange {
        CCursorRange::one(CCursor::new(c))
    }

    #[test]
    fn andringen_ger_tillbaka_texten() {
        let cases = [("", ""), ("abc", "abc"), ("abc", "abXc"), ("åäö", "åxö"), ("aaaa", "aaaaaa"), ("abcdef", "ab"), ("é", "è"), ("tvåa", "tvåor"), ("x", "")];
        for (old, new) in cases {
            let p = Patch::between(&(range(1), old.to_owned()), new);
            assert_eq!(p.apply(new), (range(1), old.to_owned()), "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn historiken_sparar_andringar_inte_kopior() {
        let mut h = History::default();
        let big: String = (0..20_000).map(|i| format!("rad {i}\n")).collect();
        let mut t = big.clone();
        let mut states = Vec::new();
        for k in 0..60 {
            t.insert_str(100 + k, "x");
            h.add_undo(&range(k), &t);
            states.push(t.clone());
        }
        let kept: usize = h.undos.older.iter().map(|p| p.mid.len()).sum::<usize>() + h.undos.top.as_ref().map_or(0, |s| s.1.len());
        assert!(kept < big.len() + 10_000, "{kept} byte för 60 punkter i en text på {} byte", big.len());
        // Och ångra går hela vägen tillbaka, punkt för punkt.
        let mut cur = (range(59), t.clone());
        for want in states.iter().rev().skip(1) {
            let s = h.undo(&cur.0, &cur.1).expect("en punkt till");
            assert_eq!(&s.1, want);
            cur = s;
        }
        // Den äldsta punkten är sista: sedan finns inget mer att ångra, och allt går att göra om.
        assert!(h.undo(&cur.0, &cur.1).is_none());
        for want in states.iter().skip(1) {
            let s = h.redo(&cur.0, &cur.1).expect("gör om");
            assert_eq!(&s.1, want);
            cur = s;
        }
    }

    /// Samma resultat som egui:s `Undoer` för slumpade anrop (läge, tid, ångra, gör om, tillägg).
    #[test]
    fn som_egui() {
        let seeds: u64 = std::env::var("KLADD_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        for seed in 0..seeds {
            let mut r = Rng(seed * 104_729 + 11);
            let mut theirs: Undoer<(CCursorRange, String)> = Undoer::default();
            let mut ours = History::default();
            let mut text = String::from("åäö abc");
            let mut cur = range(0);
            let mut time = 0.0f64;
            for step in 0..400 {
                match r.below(10) {
                    // Skriv eller radera ett tecken.
                    0..=3 => {
                        let at = text.char_indices().nth(r.below(text.chars().count() + 1)).map_or(text.len(), |(b, _)| b);
                        if r.below(3) == 0 && at < text.len() {
                            text.remove(at);
                        } else {
                            text.insert(at, ["a", "å", "\n", "xy", "é"][r.below(5)].chars().next().unwrap());
                        }
                        cur = range(r.below(text.chars().count() + 1));
                    }
                    // Bara markören flyttas.
                    4 => cur = range(r.below(text.chars().count() + 1)),
                    // Tiden går (ibland långt).
                    5..=6 => time += [0.05, 0.4, 1.2, 3.0, 31.0][r.below(5)],
                    7 => {
                        let a = theirs.undo(&(cur, text.clone())).cloned();
                        let b = ours.undo(&cur, &text);
                        assert_eq!(a, b, "ångra, seed {seed} steg {step}");
                        if let Some((c, t)) = a {
                            (cur, text) = (c, t);
                        }
                    }
                    8 => {
                        let a = theirs.redo(&(cur, text.clone())).cloned();
                        let b = ours.redo(&cur, &text);
                        assert_eq!(a, b, "gör om, seed {seed} steg {step}");
                        if let Some((c, t)) = a {
                            (cur, text) = (c, t);
                        }
                    }
                    _ => {
                        theirs.add_undo(&(cur, text.clone()));
                        ours.add_undo(&cur, &text);
                    }
                }
                theirs.feed_state(time, &(cur, text.clone()));
                ours.feed_state(time, &cur, &text);
                let s = (cur, text.clone());
                assert_eq!(theirs.has_undo(&s), ours.has_undo(&cur, &text), "has_undo, seed {seed} steg {step}");
                assert_eq!(theirs.has_redo(&s), !ours.redos.is_empty() && ours.undos.back_is(&cur, &text), "has_redo, seed {seed} steg {step}");
                assert_eq!(theirs.is_in_flux(), ours.flux.is_some(), "flux, seed {seed} steg {step}");
            }
        }
    }

    /// Hela vägen genom fältets tangenter: Ctrl+Z tas ur indata och ångrar med den nya historiken.
    #[test]
    fn tangenter_tas_ur_indata() {
        let ctx = Context::default();
        let mut got = Vec::new();
        let mut input = egui::RawInput::default();
        let press = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        input.events.push(press(Key::Z, Modifiers::COMMAND));
        input.events.push(press(Key::A, Modifiers::COMMAND));
        input.events.push(press(Key::Z, Modifiers::COMMAND));
        input.events.push(press(Key::Y, Modifiers::COMMAND));
        input.events.push(press(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT));
        let out = ctx.run_ui(input, |ui| {
            got = take_keys(ui.ctx());
            assert!(take_keys(ui.ctx()).is_empty(), "tangenterna är redan tagna");
        });
        std::mem::forget(out);
        // Ångra, ångra, gör om (Ctrl+Y) och gör om (Ctrl+Skift+Z) i samma bildruta blir fyra steg i ordning.
        assert_eq!(got, vec![false, false, true, true]);
    }
}
