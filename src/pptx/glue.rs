//! Presentationsfliken i appen. Undermodul till `app` (se `#[path]` i `app.rs`), som `epub/glue.rs`.

use egui::{CentralPanel, Frame, Ui};

use super::KladdApp;
use crate::theme::Palette;

impl KladdApp {
    /// Är den aktiva fliken en presentation?
    pub(crate) fn pptx_mode(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| d.pptx.is_some())
    }

    /// Bilderna i mitten.
    pub(crate) fn pptx_page(&mut self, ui: &mut Ui, p: &Palette) {
        CentralPanel::default().frame(Frame::NONE.fill(p.desk)).show(ui, |ui| {
            self.editor_rect = ui.max_rect();
            let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.pptx.as_deref_mut()) else { return };
            tab.show(ui, p);
        });
    }
}
