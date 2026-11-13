//! Menyraden i övre raden (Arkiv · Redigera · Visa · Infoga · Verktyg · Hjälp), byggd ur kommandoregistret.
//! I ett smalt fönster ersätts raden av hamburgermenyn med samma menyer som undermenyer.

use egui::containers::menu::{MenuBar, MenuButton, SubMenuButton, menu_style};
use egui::{Color32, FontFamily, FontId, RichText, ScrollArea, Ui, vec2};

use crate::app::commands::{self, Category, Command, Env};
use crate::app::{KladdApp, menu_divider, menu_item};
use crate::theme::Palette;

const MENU_W: f32 = 300.0;
const BAR_FONT: f32 = 13.5;
const BUTTON_PAD_X: f32 = 9.0;
const BUTTON_GAP: f32 = 2.0;
/// Utrymme som flikraden minst ska få för att menyraden ska få plats bredvid den.
pub const MIN_TABS_W: f32 = 240.0;

/// Raderna i en meny: kommandona i kategorin, med avdelare och bockar. Ger det valda kommandot.
fn rows(ui: &mut Ui, p: &Palette, cat: Category, env: &Env) -> Option<&'static Command> {
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut pick = None;
    let mut first = true;
    for c in commands::menu_items(cat, env) {
        if c.sep && !first {
            menu_divider(ui, p);
        }
        first = false;
        if menu_item(ui, p, c.label(), c.shortcut, c.is_checked(env), c.available(env)) {
            pick = Some(c);
        }
    }
    pick
}

/// Menyn rullar i stället för att klippas när fönstret är lågt.
fn scrolled(ui: &mut Ui, max_h: f32, add: impl FnOnce(&mut Ui) -> Option<&'static Command>) -> Option<&'static Command> {
    ui.set_width(MENU_W);
    ScrollArea::vertical().max_height(max_h).min_scrolled_height(max_h.min(520.0)).auto_shrink([false, true]).show(ui, add).inner
}

/// Bredden raden behöver för kategorierna.
pub fn bar_width(ui: &Ui, cats: &[Category]) -> f32 {
    let font = FontId::new(BAR_FONT, FontFamily::Proportional);
    cats.iter()
        .map(|c| ui.painter().layout_no_wrap(c.label().to_owned(), font.clone(), Color32::WHITE).size().x + 2.0 * BUTTON_PAD_X + BUTTON_GAP)
        .sum()
}

impl KladdApp {
    /// Kategorierna som har något att visa i aktuellt läge (Format finns bara i dokumentläget).
    pub(crate) fn menu_categories(&self) -> Vec<Category> {
        let env = Env::of(self);
        Category::ALL.into_iter().filter(|c| commands::menu_items(*c, &env).next().is_some()).collect()
    }

    /// Menyraden: en knapp per kategori; håller man musen över en annan knapp medan en meny är öppen byts menyn.
    pub(crate) fn menu_bar(&mut self, ui: &mut Ui, ctx: &egui::Context, p: &Palette, cats: &[Category]) {
        let max_h = (ctx.content_rect().height() - 80.0).max(200.0);
        let mut pick = None;
        {
            let env = Env::of(self);
            let (hover, text) = (p.surface_hover, p.text);
            let width = bar_width(ui, cats);
            ui.allocate_ui(vec2(width + 4.0, 30.0), |ui| {
                MenuBar::new()
                    .style(move |s: &mut egui::Style| {
                        menu_style(s);
                        s.spacing.button_padding = vec2(BUTTON_PAD_X, 4.0);
                        s.spacing.item_spacing.x = BUTTON_GAP;
                        for w in [&mut s.visuals.widgets.hovered, &mut s.visuals.widgets.active, &mut s.visuals.widgets.open] {
                            w.weak_bg_fill = hover;
                            w.bg_fill = hover;
                            w.corner_radius = 6.0.into();
                        }
                        s.visuals.widgets.inactive.corner_radius = 6.0.into();
                        s.visuals.widgets.noninteractive.corner_radius = 6.0.into();
                    })
                    .ui(ui, |ui| {
                        for &cat in cats {
                            let label = RichText::new(cat.label()).size(BAR_FONT).color(text);
                            let (_, inner) = MenuButton::new(label).ui(ui, |ui| scrolled(ui, max_h, |ui| rows(ui, p, cat, &env)));
                            if let Some(c) = inner.and_then(|i| i.inner) {
                                pick = Some(c);
                            }
                        }
                    });
            });
        }
        if let Some(c) = pick {
            self.run_command(ctx, c);
        }
    }

    /// Hamburgermenyn i smala fönster: kategorierna som undermenyer.
    pub(crate) fn hamburger_menu(&mut self, ui: &mut Ui, ctx: &egui::Context, p: &Palette) {
        let cats = self.menu_categories();
        let max_h = (ctx.content_rect().height() - 80.0).max(200.0);
        let mut pick = None;
        {
            let env = Env::of(self);
            ui.spacing_mut().item_spacing.y = 2.0;
            for cat in cats {
                let label = RichText::new(cat.label()).size(BAR_FONT).color(p.text);
                let (_, inner) = SubMenuButton::new(label).ui(ui, |ui| scrolled(ui, max_h, |ui| rows(ui, p, cat, &env)));
                if let Some(c) = inner.and_then(|i| i.inner) {
                    pick = Some(c);
                }
            }
        }
        if let Some(c) = pick {
            self.run_command(ctx, c);
        }
    }
}
