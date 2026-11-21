//! Fotnoter längst ned på sin sida i dokumentläget.
//!
//! Definitionerna (`[^n]: text`) ligger kvar sist i texten, men deras rader i galleriet är hopkrympta
//! (se `layout::relayout`): galleriets rader ligger då fortfarande i stigande y-ordning, så markör,
//! klick, markering och piltangenter fungerar som förut. Sidbrytningen (`pages::paginate_notes`,
//! `secpages::paginate_sections`) reserverar i stället plats längst ned på sidan för fotnoterna vars
//! referenser står på sidan (samma regler som PDF-exporten: `page_notes`/`reserved` i `pdf_write.rs`),
//! och vyn ritar noterna där (`NoteLayout`, `paint`). En not redigeras i en liten textruta ovanpå
//! (`view`), vars text skrivs tillbaka till definitionen.

use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::Galley;

/// Det sidbrytningen behöver veta om fotnoterna.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteSpec {
    /// (radnummer i galleriet, notindex) för varje referens, stigande efter rad.
    pub refs: Vec<(usize, usize)>,
    /// Varje nots höjd (px) med luften efter den.
    pub heights: Vec<f32>,
    /// Skiljelinjens höjd (px), med den första noten på sidan.
    pub sep: f32,
    /// Mest plats (px) fotnoterna får ta på en sida; högre noter klipps.
    pub cap: f32,
}

/// Fotnoterna som hör till en sida och höjden som reserverats för dem längst ned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageNotes {
    /// Notindex i den ordning referenserna kommer.
    pub notes: Vec<usize>,
    pub reserved: f32,
}

/// Håller reda på sidans fotnoter medan raderna placeras en efter en.
pub struct NoteFlow<'a> {
    spec: &'a NoteSpec,
    placed: Vec<bool>,
    page: usize,
    cur: Vec<usize>,
    reserved: f32,
    out: Vec<PageNotes>,
}

impl<'a> NoteFlow<'a> {
    pub fn new(spec: &'a NoteSpec) -> Self {
        Self { spec, placed: vec![false; spec.heights.len()], page: 0, cur: Vec::new(), reserved: 0.0, out: Vec::new() }
    }

    /// Raderna placeras nu på sida `page`; en ny sida börjar utan fotnoter.
    pub fn enter(&mut self, page: usize) {
        if page != self.page {
            self.page = page;
            self.cur.clear();
            self.reserved = 0.0;
        }
    }

    /// Höjden som redan är reserverad på sidan.
    pub fn reserved(&self) -> f32 {
        self.reserved
    }

    /// De fotnoter som raderna `rows` lägger till på sidan och hur mycket plats de tar (utan tak).
    pub fn need(&self, rows: Range<usize>) -> (Vec<usize>, f32) {
        if self.spec.refs.is_empty() {
            return (Vec::new(), 0.0);
        }
        let lo = self.spec.refs.partition_point(|r| r.0 < rows.start);
        let mut new: Vec<usize> = Vec::new();
        for &(row, n) in &self.spec.refs[lo..] {
            if row >= rows.end {
                break;
            }
            if !self.placed.get(n).copied().unwrap_or(true) && !new.contains(&n) {
                new.push(n);
            }
        }
        let mut h: f32 = new.iter().map(|&n| self.spec.heights[n]).sum();
        if self.cur.is_empty() && !new.is_empty() {
            h += self.spec.sep;
        }
        (new, h)
    }

    /// Tar noterna `new` med på sidan. Reservationen stannar vid taket.
    pub fn commit(&mut self, new: Vec<usize>, extra: f32) {
        if new.is_empty() {
            return;
        }
        for &n in &new {
            self.placed[n] = true;
        }
        self.cur.extend(new);
        self.reserved = (self.reserved + extra).min(self.spec.cap.max(0.0));
        if self.out.len() <= self.page {
            self.out.resize(self.page + 1, PageNotes::default());
        }
        self.out[self.page] = PageNotes { notes: self.cur.clone(), reserved: self.reserved };
    }

    /// Sidorna med fotnoter (index = sidnummer; sidor utan fotnoter är tomma och sista kan saknas).
    pub fn finish(self) -> Vec<PageNotes> {
        self.out
    }
}

/// En fotnot färdigutlagd för ritning längst ned på sidan.
#[derive(Clone)]
pub struct NoteLayout {
    pub label: String,
    /// Definitionens text som den ser ut utan markup (nummer, mellanslag och texten).
    pub galley: Arc<Galley>,
    /// Definitionsraden (byte i dokumentet) och där texten efter `[^n]: ` börjar.
    pub line: Range<usize>,
    pub body: Range<usize>,
    /// Nummerts bredd (px) i galleriet: där texten börjar på första raden.
    pub indent: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(refs: &[(usize, usize)], heights: &[f32]) -> NoteSpec {
        NoteSpec { refs: refs.to_vec(), heights: heights.to_vec(), sep: 10.0, cap: 1000.0 }
    }

    #[test]
    fn need_counts_separator_once_and_skips_placed() {
        let s = spec(&[(1, 0), (1, 1), (4, 1), (5, 2)], &[20.0, 30.0, 40.0]);
        let mut f = NoteFlow::new(&s);
        let (new, h) = f.need(0..2);
        assert_eq!(new, vec![0, 1]);
        assert_eq!(h, 60.0, "två noter och skiljelinjen");
        f.commit(new, h);
        assert_eq!(f.reserved(), 60.0);
        // Not 1 är redan placerad: bara not 2 är ny, utan ny skiljelinje.
        let (new, h) = f.need(4..6);
        assert_eq!(new, vec![2]);
        assert_eq!(h, 40.0);
        f.commit(new, h);
        // Ny sida: tomt igen.
        f.enter(1);
        assert_eq!(f.reserved(), 0.0);
        let out = f.finish();
        assert_eq!(out[0].notes, vec![0, 1, 2]);
        assert_eq!(out[0].reserved, 100.0);
    }

    #[test]
    fn reservation_stops_at_cap() {
        let mut s = spec(&[(0, 0)], &[500.0]);
        s.cap = 120.0;
        let mut f = NoteFlow::new(&s);
        let (new, h) = f.need(0..1);
        assert_eq!(h, 510.0);
        f.commit(new, h);
        assert_eq!(f.reserved(), 120.0);
    }
}
