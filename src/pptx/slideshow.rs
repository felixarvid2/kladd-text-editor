//! Bildspel i helskärm: ordningen (dolda bilder hoppas över), klick-steg för animeringar, svart
//! slutskärm, svart/vit skärm (`B`/`W`), uppspelningen i tiden (övergångar, animeringar, automatisk
//! övergång efter `advTm`) och föredragsvyns klocka. Tillståndet är rent och provas här; vyn
//! (`presenter.rs`) ritar.

use std::ops::Range;

use super::transition::{Kind, Transition};

/// Tom skärm i stället för bilden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blank {
    Black,
    White,
}

/// Vad som visas just nu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Slide(usize),
    Blank(Blank),
    /// Svart skärm efter sista bilden ("Slut på bildspelet").
    End,
}

/// Ett pågående bildspel.
#[derive(Clone, Debug)]
pub struct Show {
    /// Bilderna som visas (index i presentationen), i ordning.
    order: Vec<usize>,
    pos: usize,
    end: bool,
    blank: Option<Blank>,
    /// Antal klick-steg per bild (index i presentationen); saknas = inga.
    steps: Vec<usize>,
    /// Klick-steg på aktuell bild (0 = inget klick än).
    step: usize,
}

impl Show {
    /// Startar på `start` (eller närmaste följande synliga bild; finns ingen, den sista synliga).
    /// En dold bild visas om bildspelet startar på den (som Skift+F5 i PowerPoint).
    /// `None` om ingen bild kan visas.
    pub fn new(hidden: &[bool], start: usize) -> Option<Show> {
        let mut order: Vec<usize> = (0..hidden.len()).filter(|&i| !hidden[i]).collect();
        if start < hidden.len() && hidden[start] {
            order.push(start);
            order.sort_unstable();
        }
        if order.is_empty() {
            return None;
        }
        let pos = order.iter().position(|&i| i >= start).unwrap_or(order.len() - 1);
        Some(Show { order, pos, end: false, blank: None, steps: Vec::new(), step: 0 })
    }

    /// F5: från första synliga bilden (en dold första bild visas inte).
    pub fn from_beginning(hidden: &[bool]) -> Option<Show> {
        let first = hidden.iter().position(|h| !h)?;
        Show::new(hidden, first)
    }

    /// Klick-stegen per bild (animeringarna).
    pub fn with_steps(mut self, steps: Vec<usize>) -> Show {
        self.steps = steps;
        self
    }

    fn steps_of(&self, slide: usize) -> usize {
        self.steps.get(slide).copied().unwrap_or(0)
    }

    /// Klick-steget på aktuell bild.
    pub fn step(&self) -> usize {
        self.step
    }

    pub fn screen(&self) -> Screen {
        if let Some(b) = self.blank {
            Screen::Blank(b)
        } else if self.end {
            Screen::End
        } else {
            Screen::Slide(self.order[self.pos])
        }
    }

    /// Bilden som visas eller senast visades (för att stå kvar där när bildspelet slutar).
    pub fn slide(&self) -> usize {
        self.order[self.pos]
    }

    /// Nästa bild att förrendera (bildspelet använder `peek_next`, som räknar med klick-stegen).
    #[allow(dead_code)]
    pub fn upcoming(&self) -> Option<usize> {
        if self.end { None } else { self.order.get(self.pos + 1).copied() }
    }

    /// Nästa klick-steg på bilden, annars nästa bild. Falskt när bildspelet ska avslutas (framåt på
    /// slutskärmen).
    pub fn next(&mut self) -> bool {
        if self.blank.take().is_some() {
            return true;
        }
        if self.end {
            return false;
        }
        if self.step < self.steps_of(self.slide()) {
            self.step += 1;
        } else if self.pos + 1 < self.order.len() {
            self.pos += 1;
            self.step = 0;
        } else {
            self.end = true;
        }
        true
    }

    /// Föregående klick-steg; från bildens början till föregående bild i sitt slutläge (som PowerPoint).
    pub fn prev(&mut self) {
        if self.blank.take().is_some() {
            return;
        }
        if self.end {
            self.end = false;
        } else if self.step > 0 {
            self.step -= 1;
        } else if self.pos > 0 {
            self.pos -= 1;
            self.step = self.steps_of(self.slide());
        }
    }

    pub fn first(&mut self) {
        self.blank = None;
        self.end = false;
        self.pos = 0;
        self.step = 0;
    }

    pub fn last(&mut self) {
        self.blank = None;
        self.end = false;
        self.pos = self.order.len() - 1;
        self.step = 0;
    }

    /// Bildspelets plats i ordningen (för föredragsvyns "bild 3 av 12").
    pub fn position(&self) -> (usize, usize) {
        (self.pos, self.order.len())
    }

    /// Bilden och steget som kommer efter nästa klick (för föredragsvyns förhandsvisning): samma bild
    /// med ett steg till, eller nästa bild från början. `None` på sista bildens sista steg.
    pub fn peek_next(&self) -> Option<(usize, usize)> {
        if self.end {
            return None;
        }
        let s = self.slide();
        if self.step < self.steps_of(s) {
            Some((s, self.step + 1))
        } else {
            self.order.get(self.pos + 1).map(|&n| (n, 0))
        }
    }

    /// `B`/`W`: växlar svart eller vit skärm.
    pub fn toggle_blank(&mut self, b: Blank) {
        self.blank = if self.blank == Some(b) { None } else { Some(b) };
    }
}

// ------------------------------------------------------------ uppspelning i tiden

/// En bilds tider för uppspelningen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlideTiming {
    /// Animeringsgrupperna som spelas för varje steg (0 = när bilden visas, 1 = första klicket …) och
    /// hur länge de tar (s). Tom = inga animeringar.
    pub chains: Vec<(Range<usize>, f32)>,
    pub transition: Transition,
}

impl SlideTiming {
    pub fn clicks(&self) -> usize {
        self.chains.len().saturating_sub(1)
    }

    fn chain(&self, step: usize) -> (Range<usize>, f32) {
        self.chains.get(step).cloned().unwrap_or((0..0, 0.0))
    }
}

/// Vad som pågår.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Övergång till aktuell bild från (bild, steg) eller från svart; startade vid `start`.
    Transition { from: Option<(usize, usize)>, start: f64 },
    /// Stegets animeringar spelas sedan `start`.
    Anim { start: f64 },
    /// Vila.
    Rest,
}

/// En bild i ett visst läge: animeringsgrupperna i `chain` har spelats i `t` sekunder (grupperna före
/// är klara). `t = f32::INFINITY` = vila.
#[derive(Clone, Debug, PartialEq)]
pub struct SlideState {
    pub slide: usize,
    pub step: usize,
    pub chain: Range<usize>,
    pub t: f32,
}

/// Bildrutan att rita.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub screen: Screen,
    /// Aktuell bild (när `screen` är en bild).
    pub cur: Option<SlideState>,
    /// Pågående övergång: förra bilden (`None` = svart), slaget och förloppet 0..1.
    pub transition: Option<(Option<SlideState>, Kind, f32)>,
}

/// Bildspelet i tiden. Tider i sekunder från en valfri nollpunkt (vyn använder en `Instant`).
#[derive(Clone, Debug)]
pub struct Playback {
    pub show: Show,
    timing: Vec<SlideTiming>,
    phase: Phase,
    /// När aktuellt steg började (för `advTm`).
    step_start: f64,
}

impl Playback {
    /// `timing` per bild i presentationen. Den första bilden visas med sin övergång från svart.
    pub fn new(show: Show, timing: Vec<SlideTiming>, now: f64) -> Playback {
        let steps = timing.iter().map(SlideTiming::clicks).collect();
        let show = show.with_steps(steps);
        let mut p = Playback { show, timing, phase: Phase::Rest, step_start: now };
        p.enter_slide(None, now);
        p
    }

    fn timing_of(&self, slide: usize) -> SlideTiming {
        self.timing.get(slide).cloned().unwrap_or_default()
    }

    /// Bilden har just blivit aktuell (framåt): övergång, sedan stegets självstartande animeringar.
    fn enter_slide(&mut self, from: Option<(usize, usize)>, now: f64) {
        let t = self.timing_of(self.show.slide());
        self.step_start = now;
        self.phase = if t.transition.animated() { Phase::Transition { from, start: now } } else { Phase::Anim { start: now } };
    }

    /// Framåt (`click` = musklick; `advClick="0"` gör att klick inte byter bild). Falskt = avsluta.
    pub fn next(&mut self, now: f64, click: bool) -> bool {
        let before = (self.show.screen(), self.show.step());
        if click && self.show.blank.is_none() && !self.show.end {
            let s = self.show.slide();
            if self.show.step() >= self.timing_of(s).clicks() && !self.timing_of(s).transition.adv_click {
                return true;
            }
        }
        // En pågående övergång eller animering hoppar till sitt slut först? Nej: som PowerPoint går
        // nästa steg direkt (den pågående visas i sitt slutläge).
        if !self.show.next() {
            return false;
        }
        let after = (self.show.screen(), self.show.step());
        match (before, after) {
            ((Screen::Slide(a), sa), (Screen::Slide(b), sb)) if a == b && sb == sa + 1 => {
                self.phase = Phase::Anim { start: now };
                self.step_start = now;
            }
            ((Screen::Slide(a), sa), (Screen::Slide(b), _)) if a != b => self.enter_slide(Some((a, sa)), now),
            _ => {
                self.phase = Phase::Rest;
                self.step_start = now;
            }
        }
        true
    }

    /// Bakåt: föregående steg i vila (ingen animering eller övergång).
    pub fn prev(&mut self, now: f64) {
        self.show.prev();
        self.phase = Phase::Rest;
        self.step_start = now;
    }

    /// Hoppar (Home/End, svart/vit skärm …) utan övergång.
    pub fn jump(&mut self, now: f64, f: impl FnOnce(&mut Show)) {
        f(&mut self.show);
        self.phase = Phase::Rest;
        self.step_start = now;
    }

    /// Flyttar fram tiden: övergång → självstartande animeringar → vila, och automatisk övergång efter
    /// `advTm` (räknat från stegets början, tidigast när stegets animeringar är klara). Falskt = avsluta.
    pub fn tick(&mut self, now: f64) -> bool {
        if !matches!(self.show.screen(), Screen::Slide(_)) {
            return true;
        }
        let s = self.show.slide();
        let t = self.timing_of(s);
        if let Phase::Transition { start, .. } = self.phase {
            if now - start >= t.transition.dur as f64 {
                self.phase = Phase::Anim { start: start + t.transition.dur as f64 };
                self.step_start = start + t.transition.dur as f64;
            }
        }
        if let Phase::Anim { start } = self.phase {
            if now - start >= t.chain(self.show.step()).1 as f64 {
                self.phase = Phase::Rest;
            }
        }
        if let (Phase::Rest, Some(after)) = (self.phase, t.transition.adv_after) {
            let due = self.step_start + (after as f64).max(t.chain(self.show.step()).1 as f64);
            if now >= due {
                return self.next(now, false);
            }
        }
        true
    }

    /// Pågår något som behöver nya bildrutor?
    pub fn animating(&self) -> bool {
        !matches!(self.phase, Phase::Rest)
    }

    /// Tid kvar till nästa automatiska övergång (för omritning), om någon.
    pub fn auto_due(&self, now: f64) -> Option<f64> {
        let Screen::Slide(s) = self.show.screen() else { return None };
        let t = self.timing_of(s);
        let after = t.transition.adv_after?;
        Some((self.step_start + (after as f64).max(t.chain(self.show.step()).1 as f64) - now).max(0.0))
    }

    fn rest_state(&self, slide: usize, step: usize) -> SlideState {
        let end = self.timing_of(slide).chain(step).0.end;
        SlideState { slide, step, chain: end..end, t: f32::INFINITY }
    }

    /// Bildrutan vid `now`.
    pub fn frame(&self, now: f64) -> Frame {
        let screen = self.show.screen();
        let Screen::Slide(s) = screen else { return Frame { screen, cur: None, transition: None } };
        let step = self.show.step();
        let t = self.timing_of(s);
        match self.phase {
            Phase::Transition { from, start } => {
                let p = if t.transition.dur > 0.0 { ((now - start) / t.transition.dur as f64).clamp(0.0, 1.0) as f32 } else { 1.0 };
                let (chain, _) = t.chain(step);
                // Bildens egna animeringar börjar först när övergången är klar.
                let cur = SlideState { slide: s, step, chain, t: -1.0 };
                let old = from.map(|(fs, fstep)| self.rest_state(fs, fstep));
                Frame { screen, cur: Some(cur), transition: Some((old, t.transition.kind, p)) }
            }
            Phase::Anim { start } => {
                let (chain, _) = t.chain(step);
                Frame { screen, cur: Some(SlideState { slide: s, step, chain, t: (now - start).max(0.0) as f32 }), transition: None }
            }
            Phase::Rest => Frame { screen, cur: Some(self.rest_state(s, step)), transition: None },
        }
    }
}

/// Föredragsvyns klocka: förfluten tid med paus och nollställning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timer {
    start: f64,
    paused_at: Option<f64>,
}

impl Timer {
    pub fn new(now: f64) -> Timer {
        Timer { start: now, paused_at: None }
    }

    pub fn elapsed(&self, now: f64) -> f64 {
        (self.paused_at.unwrap_or(now) - self.start).max(0.0)
    }

    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn toggle_pause(&mut self, now: f64) {
        match self.paused_at.take() {
            Some(p) => self.start += now - p,
            None => self.paused_at = Some(now),
        }
    }

    pub fn reset(&mut self, now: f64) {
        self.start = now;
        if self.paused_at.is_some() {
            self.paused_at = Some(now);
        }
    }
}

/// "1:05:09" eller "05:09".
pub fn clock_text(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;


    fn timing(clicks: usize, dur: f32, kind: Kind, tdur: f32, adv_after: Option<f32>) -> SlideTiming {
        let chains = (0..=clicks).map(|k| (k.saturating_sub(1)..k, if k > 0 { dur } else { 0.0 })).collect();
        SlideTiming { chains, transition: Transition { kind, dur: tdur, adv_click: true, adv_after } }
    }

    #[test]
    fn klick_steg_framåt_och_bakåt() {
        let mut s = Show::new(&[false, false, false], 0).unwrap().with_steps(vec![2, 0, 1]);
        assert_eq!((s.slide(), s.step()), (0, 0));
        assert_eq!(s.peek_next(), Some((0, 1)));
        s.next();
        s.next();
        assert_eq!((s.slide(), s.step()), (0, 2));
        assert_eq!(s.peek_next(), Some((1, 0)));
        s.next();
        assert_eq!((s.slide(), s.step()), (1, 0));
        // Bakåt från bildens början: föregående bild i sitt slutläge.
        s.prev();
        assert_eq!((s.slide(), s.step()), (0, 2));
        s.prev();
        assert_eq!((s.slide(), s.step()), (0, 1));
        s.last();
        assert_eq!((s.slide(), s.step()), (2, 0));
        s.next();
        assert_eq!(s.step(), 1);
        assert_eq!(s.peek_next(), None);
        assert!(s.next());
        assert_eq!(s.screen(), Screen::End);
        s.prev();
        assert_eq!((s.slide(), s.step()), (2, 1), "från slutskärmen tillbaka till sista steget");
        assert_eq!(s.position(), (2, 3));
    }

    #[test]
    fn uppspelning_övergång_animering_och_vila() {
        let t = vec![timing(1, 2.0, Kind::Fade { black: false }, 0.0, None), timing(0, 0.0, Kind::Fade { black: false }, 1.0, None)];
        let mut p = Playback::new(Show::new(&[false, false], 0).unwrap(), t, 0.0);
        // Första bilden har ingen övergång: vila direkt.
        assert!(p.tick(0.0));
        assert!(!p.animating());
        assert_eq!(p.frame(0.0).cur.unwrap().t, f32::INFINITY);
        // Klick: steg 1 spelas i 2 s.
        assert!(p.next(10.0, true));
        let f = p.frame(11.0);
        let cur = f.cur.unwrap();
        assert_eq!((cur.step, cur.chain.clone(), cur.t), (1, 0..1, 1.0));
        assert!(p.tick(11.0) && p.animating());
        assert!(p.tick(12.5));
        assert!(!p.animating());
        // Nästa bild: toning på 1 s från bild 0 i sitt slutläge.
        assert!(p.next(20.0, true));
        let f = p.frame(20.5);
        let (old, kind, prog) = f.transition.clone().unwrap();
        assert_eq!(kind, Kind::Fade { black: false });
        assert!((prog - 0.5).abs() < 1e-6);
        let old = old.unwrap();
        assert_eq!((old.slide, old.step, old.chain.end), (0, 1, 1));
        assert!(f.cur.unwrap().t < 0.0, "nya bilden i läget före sina animeringar");
        assert!(p.tick(21.1));
        assert!(p.frame(21.1).transition.is_none());
        // Bakåt: förra bilden i slutläget, utan övergång.
        p.prev(30.0);
        let f = p.frame(30.0);
        assert!(f.transition.is_none());
        assert_eq!(f.cur.clone().unwrap().slide, 0);
        assert_eq!(f.cur.unwrap().chain, 1..1);
        p.prev(31.0);
        assert_eq!(p.frame(31.0).cur.unwrap().chain, 0..0, "ett klick-steg tillbaka");
        // Framåt till slutskärmen och ut.
        assert!(p.next(32.0, false));
        assert!(p.next(33.0, false));
        assert!(p.next(34.0, false));
        assert_eq!(p.frame(34.0).screen, Screen::End);
        assert!(!p.next(35.0, false));
    }

    #[test]
    fn automatisk_övergång_efter_tid_och_klick_avstängt() {
        let mut t0 = timing(1, 4.0, Kind::Cut { black: false }, 0.0, Some(3.0));
        t0.transition.adv_click = false;
        let t = vec![t0, timing(0, 0.0, Kind::Cut { black: false }, 0.0, None)];
        let mut p = Playback::new(Show::new(&[false, false], 0).unwrap(), t, 0.0);
        assert!(p.tick(2.9));
        assert_eq!(p.show.step(), 0);
        assert_eq!(p.auto_due(2.0), Some(1.0));
        // Efter 3 s: nästa steg (animeringen på 4 s), sedan väntar övergången tills den är klar.
        assert!(p.tick(3.0));
        assert_eq!(p.show.step(), 1);
        assert!(p.tick(6.5));
        assert_eq!(p.show.slide(), 0, "animeringen är inte klar");
        assert!(p.tick(7.0));
        assert_eq!(p.show.slide(), 1);
        // advClick="0": klick på sista steget byter inte bild, men tangenter gör det.
        let mut t0 = timing(0, 0.0, Kind::Cut { black: false }, 0.0, None);
        t0.transition.adv_click = false;
        let mut p = Playback::new(Show::new(&[false, false], 0).unwrap(), vec![t0, SlideTiming::default()], 0.0);
        assert!(p.next(1.0, true));
        assert_eq!(p.show.slide(), 0);
        assert!(p.next(1.0, false));
        assert_eq!(p.show.slide(), 1);
    }

    #[test]
    fn klocka_med_paus() {
        let mut t = Timer::new(100.0);
        assert_eq!(t.elapsed(165.0), 65.0);
        t.toggle_pause(165.0);
        assert!(t.paused());
        assert_eq!(t.elapsed(200.0), 65.0);
        t.toggle_pause(200.0);
        assert_eq!(t.elapsed(210.0), 75.0);
        t.reset(300.0);
        assert_eq!(t.elapsed(301.0), 1.0);
        assert_eq!(clock_text(65.0), "01:05");
        assert_eq!(clock_text(3725.0), "1:02:05");
    }

    #[test]
    fn dolda_bilder_hoppas_över_och_slutskärm() {
        let hidden = [false, true, false];
        let mut s = Show::new(&hidden, 0).unwrap();
        assert_eq!(s.screen(), Screen::Slide(0));
        assert_eq!(s.upcoming(), Some(2));
        assert!(s.next());
        assert_eq!(s.screen(), Screen::Slide(2), "den dolda bilden hoppas över");
        assert!(s.next());
        assert_eq!(s.screen(), Screen::End);
        assert_eq!(s.upcoming(), None);
        s.prev();
        assert_eq!(s.screen(), Screen::Slide(2));
        assert!(s.next());
        assert!(!s.next(), "framåt på slutskärmen avslutar");
    }

    #[test]
    fn start_på_aktuell_och_på_dold_bild() {
        let hidden = [false, true, false, false];
        let s = Show::new(&hidden, 2).unwrap();
        assert_eq!(s.screen(), Screen::Slide(2));
        let mut s = Show::new(&hidden, 1).unwrap();
        assert_eq!(s.screen(), Screen::Slide(1), "startar man på en dold bild visas den");
        s.next();
        assert_eq!(s.screen(), Screen::Slide(2));
        s.first();
        assert_eq!(s.screen(), Screen::Slide(0));
        s.last();
        assert_eq!(s.screen(), Screen::Slide(3));
        // Utanför: sista bilden.
        assert_eq!(Show::new(&hidden, 99).unwrap().screen(), Screen::Slide(3));
    }

    #[test]
    fn från_början_hoppar_över_en_dold_första_bild() {
        let s = Show::from_beginning(&[true, false, false]).unwrap();
        assert_eq!(s.screen(), Screen::Slide(1));
        assert!(Show::from_beginning(&[true, true]).is_none());
    }

    #[test]
    fn inga_synliga_bilder() {
        assert!(Show::new(&[], 0).is_none());
        assert!(Show::new(&[true, true], 5).is_none());
        // Startar man på en dold bild i en helt dold presentation visas just den.
        let s = Show::new(&[true, true], 1).unwrap();
        assert_eq!(s.screen(), Screen::Slide(1));
    }

    #[test]
    fn svart_och_vit_skärm() {
        let mut s = Show::new(&[false, false], 0).unwrap();
        s.toggle_blank(Blank::Black);
        assert_eq!(s.screen(), Screen::Blank(Blank::Black));
        s.toggle_blank(Blank::White);
        assert_eq!(s.screen(), Screen::Blank(Blank::White));
        // Framåt från tom skärm visar bilden igen utan att byta bild.
        assert!(s.next());
        assert_eq!(s.screen(), Screen::Slide(0));
        s.toggle_blank(Blank::Black);
        s.toggle_blank(Blank::Black);
        assert_eq!(s.screen(), Screen::Slide(0));
    }
}
