//! Delad vy: två redigeringsrutor bredvid eller över varandra.
//!
//! Här finns bara logiken (vilken flik som visas var, aktiv ruta, delningsförhållande och
//! rutornas rektanglar). Själva ritandet sker i `app.rs` med samma kod som för en enda ruta.

use std::sync::atomic::{AtomicU8, Ordering};

use egui::{Pos2, Rect, pos2};

/// Minsta och största andel av ytan som den första rutan får.
pub const MIN_RATIO: f32 = 0.15;
pub const MAX_RATIO: f32 = 0.85;
/// Skiljelinjens tjocklek.
pub const DIVIDER: f32 = 6.0;

static SALT: AtomicU8 = AtomicU8::new(0);

/// Vilken ruta som ritas just nu (och annars den aktiva). Ingår i id:n för textfält och rullningsytor
/// så att samma dokument kan visas i båda rutorna med egen markör och egen rullning.
pub fn salt() -> u8 {
    SALT.load(Ordering::Relaxed)
}

pub fn set_salt(pane: u8) {
    SALT.store(pane, Ordering::Relaxed);
}

pub fn clamp_ratio(r: f32) -> f32 {
    if r.is_finite() { r.clamp(MIN_RATIO, MAX_RATIO) } else { 0.5 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orient {
    /// Rutorna bredvid varandra.
    Vertical,
    /// Rutorna ovanpå varandra.
    Horizontal,
}

#[derive(Default, Debug)]
pub struct Panes {
    /// `None` = en enda ruta.
    pub orient: Option<Orient>,
    /// Dokument-id i varje ruta (den andra används bara när vyn är delad).
    pub docs: [u64; 2],
    /// Aktiv ruta; tangentbord, sökfält och statusrad följer den.
    pub active: usize,
}

impl Panes {
    pub fn is_split(&self) -> bool {
        self.orient.is_some()
    }

    pub fn same_doc(&self) -> bool {
        self.is_split() && self.docs[0] == self.docs[1]
    }

    /// Delar vyn (den nya rutan visar samma dokument), byter delningsriktning eller, om vyn redan
    /// är delad åt det hållet, tar bort delningen.
    pub fn toggle(&mut self, orient: Orient, active_doc: Option<u64>) {
        match (self.orient, active_doc) {
            (Some(o), _) if o == orient => self.close(),
            (Some(_), _) => self.orient = Some(orient),
            (None, Some(id)) => {
                self.orient = Some(orient);
                self.docs = [id, id];
                self.active = 0;
            }
            (None, None) => {}
        }
    }

    /// Tillbaka till en ruta (den aktiva rutans flik blir kvar).
    pub fn close(&mut self) {
        self.orient = None;
        self.active = 0;
    }

    /// Den aktiva rutan visar alltid den aktiva fliken. Den andra rutan faller tillbaka på den
    /// aktiva fliken om dess dokument inte finns längre. Utan flikar finns ingen delning.
    pub fn follow_active(&mut self, active_doc: Option<u64>, exists: impl Fn(u64) -> bool) {
        if !self.is_split() {
            return;
        }
        let Some(id) = active_doc else {
            self.close();
            return;
        };
        self.docs[self.active] = id;
        let other = 1 - self.active;
        if !exists(self.docs[other]) {
            self.docs[other] = id;
        }
    }

    /// Ett dokument stängs: rutor som visade det visar `fallback` i stället; utan någon flik kvar
    /// tas delningen bort.
    pub fn forget_doc(&mut self, closed: u64, fallback: Option<u64>) {
        let Some(fb) = fallback else {
            self.close();
            return;
        };
        for d in &mut self.docs {
            if *d == closed {
                *d = fb;
            }
        }
    }

    /// Gör `pane` aktiv. Ger dokumentet som ska bli aktiv flik.
    pub fn focus(&mut self, pane: usize) -> Option<u64> {
        if !self.is_split() || pane > 1 {
            return None;
        }
        self.active = pane;
        Some(self.docs[pane])
    }
}

/// Rektanglar för (första rutan, andra rutan, skiljelinjen) inom `area`.
pub fn split_rects(area: Rect, orient: Orient, ratio: f32) -> (Rect, Rect, Rect) {
    let ratio = clamp_ratio(ratio);
    match orient {
        Orient::Vertical => {
            let x = (area.left() + (area.width() - DIVIDER) * ratio).round();
            (
                Rect::from_min_max(area.min, pos2(x, area.bottom())),
                Rect::from_min_max(pos2(x + DIVIDER, area.top()), area.max),
                Rect::from_min_max(pos2(x, area.top()), pos2(x + DIVIDER, area.bottom())),
            )
        }
        Orient::Horizontal => {
            let y = (area.top() + (area.height() - DIVIDER) * ratio).round();
            (
                Rect::from_min_max(area.min, pos2(area.right(), y)),
                Rect::from_min_max(pos2(area.left(), y + DIVIDER), area.max),
                Rect::from_min_max(pos2(area.left(), y), pos2(area.right(), y + DIVIDER)),
            )
        }
    }
}

/// Delningsförhållandet när skiljelinjens mitt dras till `pointer`.
pub fn ratio_at(area: Rect, orient: Orient, pointer: Pos2) -> f32 {
    let (pos, start, len) = match orient {
        Orient::Vertical => (pointer.x, area.left(), area.width()),
        Orient::Horizontal => (pointer.y, area.top(), area.height()),
    };
    let room = (len - DIVIDER).max(1.0);
    clamp_ratio((pos - DIVIDER / 2.0 - start) / room)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(doc: u64) -> Panes {
        let mut p = Panes::default();
        p.toggle(Orient::Vertical, Some(doc));
        p
    }

    #[test]
    fn delning_visar_samma_dokument_i_bada() {
        let p = split(7);
        assert!(p.is_split() && p.same_doc());
        assert_eq!(p.docs, [7, 7]);
        assert_eq!(p.active, 0);
    }

    #[test]
    fn toggle_stanger_byter_riktning_och_oppnar() {
        let mut p = split(1);
        p.toggle(Orient::Horizontal, Some(1));
        assert_eq!(p.orient, Some(Orient::Horizontal));
        p.toggle(Orient::Horizontal, Some(1));
        assert!(!p.is_split());
        p.toggle(Orient::Vertical, None);
        assert!(!p.is_split(), "utan flik går det inte att dela");
    }

    #[test]
    fn aktiv_ruta_foljer_aktiv_flik() {
        let mut p = split(1);
        p.docs[1] = 2;
        assert_eq!(p.focus(1), Some(2));
        p.follow_active(Some(3), |_| true);
        assert_eq!(p.docs, [1, 3]);
        assert_eq!(p.focus(0), Some(1));
        p.follow_active(Some(4), |_| true);
        assert_eq!(p.docs, [4, 3]);
    }

    #[test]
    fn saknad_flik_i_andra_rutan_faller_tillbaka() {
        let mut p = split(1);
        p.docs[1] = 9;
        p.follow_active(Some(1), |id| id != 9);
        assert_eq!(p.docs, [1, 1]);
    }

    #[test]
    fn stangd_flik_ersatts_och_sista_stanger_delningen() {
        let mut p = split(1);
        p.docs[1] = 2;
        p.forget_doc(2, Some(1));
        assert_eq!(p.docs, [1, 1]);
        p.forget_doc(1, None);
        assert!(!p.is_split());
        assert_eq!(p.active, 0);
    }

    #[test]
    fn stang_delning_ger_en_ruta() {
        let mut p = split(1);
        p.focus(1);
        p.close();
        assert!(!p.is_split());
        assert_eq!(p.active, 0);
        assert_eq!(p.focus(1), None);
    }

    #[test]
    fn forhallandet_begransas() {
        assert_eq!(clamp_ratio(0.0), MIN_RATIO);
        assert_eq!(clamp_ratio(2.0), MAX_RATIO);
        assert_eq!(clamp_ratio(f32::NAN), 0.5);
        assert_eq!(clamp_ratio(0.4), 0.4);
    }

    #[test]
    fn rektanglar_tackar_ytan() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 500.0));
        let (a, b, d) = split_rects(area, Orient::Vertical, 0.5);
        assert_eq!(a.right(), d.left());
        assert_eq!(d.right(), b.left());
        assert_eq!(b.right(), 1000.0);
        let (a, b, d) = split_rects(area, Orient::Horizontal, 0.25);
        assert_eq!(a.bottom(), d.top());
        assert_eq!(d.bottom(), b.top());
        assert_eq!(b.bottom(), 500.0);
    }

    #[test]
    fn drag_ger_begransat_forhallande() {
        let area = Rect::from_min_max(pos2(100.0, 0.0), pos2(1100.0, 500.0));
        assert_eq!(ratio_at(area, Orient::Vertical, pos2(-50.0, 0.0)), MIN_RATIO);
        assert_eq!(ratio_at(area, Orient::Vertical, pos2(5000.0, 0.0)), MAX_RATIO);
        let r = ratio_at(area, Orient::Vertical, pos2(600.0, 0.0));
        assert!((r - 0.5).abs() < 0.01);
        // Skiljelinjen hamnar under pekaren.
        let (_, _, d) = split_rects(area, Orient::Vertical, r);
        assert!((d.center().x - 600.0).abs() < 1.5);
    }
}
