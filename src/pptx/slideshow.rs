//! Bildspel i helskärm: ordningen (dolda bilder hoppas över), svart slutskärm, svart/vit skärm
//! (`B`/`W`) och tangenterna. Tillståndet är rent och provas här; vyn (`view.rs`) ritar.

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
        Some(Show { order, pos, end: false, blank: None })
    }

    /// F5: från första synliga bilden (en dold första bild visas inte).
    pub fn from_beginning(hidden: &[bool]) -> Option<Show> {
        let first = hidden.iter().position(|h| !h)?;
        Show::new(hidden, first)
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

    /// Nästa bild att förrendera.
    pub fn upcoming(&self) -> Option<usize> {
        if self.end { None } else { self.order.get(self.pos + 1).copied() }
    }

    /// Framåt. Falskt när bildspelet ska avslutas (framåt på slutskärmen).
    pub fn next(&mut self) -> bool {
        if self.blank.take().is_some() {
            return true;
        }
        if self.end {
            return false;
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
        } else {
            self.end = true;
        }
        true
    }

    pub fn prev(&mut self) {
        if self.blank.take().is_some() {
            return;
        }
        if self.end {
            self.end = false;
        } else {
            self.pos = self.pos.saturating_sub(1);
        }
    }

    pub fn first(&mut self) {
        self.blank = None;
        self.end = false;
        self.pos = 0;
    }

    pub fn last(&mut self) {
        self.blank = None;
        self.end = false;
        self.pos = self.order.len() - 1;
    }

    /// `B`/`W`: växlar svart eller vit skärm.
    pub fn toggle_blank(&mut self, b: Blank) {
        self.blank = if self.blank == Some(b) { None } else { Some(b) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
