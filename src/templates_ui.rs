//! Dialogen "Ny från mall": galleri med förhandsvisningskort och kategorier, formuläret för
//! platshållare, samt namnrutan för "Spara som mall…" och "Byt namn". Logiken ligger i `templates.rs`.

use std::path::PathBuf;

use egui::{
    Align, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Layout, Margin, Pos2, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, TextEdit, Ui, vec2,
};

use crate::i18n::{self, t};
use crate::templates::{self, Category, Sketch, Template};
use crate::theme::{self, Palette};

const CARD_W: f32 = 150.0;
const PAGE_W: f32 = 108.0;
const PAGE_MAX_H: f32 = 150.0;

/// Vad dialogen vill att appen gör.
pub enum Action {
    None,
    Close,
    /// Öppna en ny namnlös flik med den här texten.
    Create { text: String, doc: bool },
    /// Välj en .docx/.odt att importera som mall.
    Import,
    /// Visa en notis (och stäng dialogen om `close`).
    Done { message: String, error: bool, close: bool },
}

struct Field {
    key: String,
    value: String,
}

enum Stage {
    Gallery,
    Form { tpl: Template, fields: Vec<Field> },
    /// Namnruta för "Spara som mall…" (`rename: None`) eller "Byt namn" (`Some(id)`).
    Name { rename: Option<String>, name: String, error: Option<String> },
    /// Bekräfta att en befintlig egen mall ersätts.
    Replace { name: String },
}

pub struct Dialog {
    stage: Stage,
    dir: PathBuf,
    templates: Vec<Template>,
    /// Vald kategori; `None` = alla.
    category: Option<Category>,
    selected: Option<String>,
    author: String,
    /// Texten som "Spara som mall…" sparar (dokumentets text) och dess läge.
    save_text: Option<(String, bool)>,
    /// Egen mall som väntar på att bekräfta borttagning.
    confirm_delete: Option<String>,
    focus: bool,
}

impl Dialog {
    /// Galleriet.
    pub fn gallery(author: &str) -> Self {
        let mut d = Self::base(author);
        d.reload();
        d
    }

    /// "Spara som mall…" för en text; `default_name` är förslaget.
    pub fn save_as(text: String, doc: bool, default_name: &str, author: &str) -> Self {
        let mut d = Self::base(author);
        d.save_text = Some((text, doc));
        d.stage = Stage::Name { rename: None, name: default_name.to_owned(), error: None };
        d
    }

    fn base(author: &str) -> Self {
        Self {
            stage: Stage::Gallery,
            dir: templates::user_dir(),
            templates: Vec::new(),
            category: None,
            selected: None,
            author: author.to_owned(),
            save_text: None,
            confirm_delete: None,
            focus: true,
        }
    }

    /// Läser om listan (efter import, namnbyte eller borttagning). Behåller valet om mallen finns kvar.
    pub fn reload(&mut self) {
        let mut v = templates::builtins(i18n::lang());
        v.extend(templates::list_user(&self.dir));
        self.templates = v;
        if let Some(sel) = &self.selected {
            if !self.templates.iter().any(|t| key(t) == *sel) {
                self.selected = None;
            }
        }
        self.selected = self.selected.take().or_else(|| self.templates.first().map(key));
    }

    /// Markerar en egen mall (efter import) så att den syns.
    pub fn select_user(&mut self, id: &str) {
        self.reload();
        self.category = Some(Category::Mine);
        self.selected = Some(format!("u:{id}"));
    }

    fn visible(&self) -> Vec<&Template> {
        self.templates.iter().filter(|t| self.category.is_none_or(|c| t.category == c)).collect()
    }

    fn open_form(&mut self, tpl: Template) -> Action {
        let keys = templates::placeholders(&tpl.body);
        if keys.is_empty() {
            return Action::Create { text: templates::create(&tpl, &[]), doc: tpl.doc };
        }
        let values = templates::prefill(&keys, &self.author);
        let fields = keys.into_iter().zip(values).map(|(key, value)| Field { key, value }).collect();
        self.stage = Stage::Form { tpl, fields };
        self.focus = true;
        Action::None
    }

    pub fn show(&mut self, ctx: &egui::Context, p: &Palette) -> Action {
        let mut action = Action::None;
        let modal = egui::Modal::new(Id::unique("kladd-templates"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
            .frame(
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(14)
                    .inner_margin(Margin::same(24))
                    .shadow(ctx.global_style().visuals.window_shadow),
            )
            .show(ctx, |ui| {
                let w = (ctx.content_rect().width() - 120.0).clamp(420.0, 900.0);
                ui.set_width(w);
                match &self.stage {
                    Stage::Gallery => action = self.gallery_ui(ui, ctx, p),
                    Stage::Form { .. } => action = self.form_ui(ui, p),
                    Stage::Name { .. } => action = self.name_ui(ui, p),
                    Stage::Replace { .. } => action = self.replace_ui(ui, p),
                }
            });
        if modal.should_close() && matches!(action, Action::None) {
            action = match self.stage {
                Stage::Gallery => Action::Close,
                // Esc går ett steg tillbaka när man kom från galleriet.
                _ if self.save_text.is_none() => {
                    self.stage = Stage::Gallery;
                    Action::None
                }
                _ => Action::Close,
            };
        }
        action
    }

    fn title(ui: &mut Ui, p: &Palette, text: &str) {
        ui.label(RichText::new(text).font(FontId::new(17.0, theme::semibold())).color(p.text));
        ui.add_space(10.0);
    }

    // ------------------------------------------------------------ galleri

    fn gallery_ui(&mut self, ui: &mut Ui, ctx: &egui::Context, p: &Palette) -> Action {
        let mut action = Action::None;
        Self::title(ui, p, t("Ny från mall"));
        // Kategorier.
        let has_mine = self.templates.iter().any(|t| t.user);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if crate::workspace::chip(ui, p, t("Alla"), self.category.is_none(), "").clicked() {
                self.category = None;
            }
            for c in Category::BUILTIN.into_iter().chain(has_mine.then_some(Category::Mine)) {
                if crate::workspace::chip(ui, p, c.label(), self.category == Some(c), "").clicked() {
                    self.category = Some(c);
                }
            }
        });
        ui.add_space(10.0);
        let h = (ctx.content_rect().height() - 330.0).clamp(200.0, 470.0);
        let visible: Vec<Template> = self.visible().into_iter().cloned().collect();
        // Valet följer med när kategorin byts: det syns alltid en vald mall.
        if !visible.iter().any(|t| Some(key(t)) == self.selected) {
            self.selected = visible.first().map(key);
        }
        let mut pick = None;
        let mut open = None;
        ScrollArea::vertical().id_salt("tpl-gallery").max_height(h).min_scrolled_height(h).auto_shrink([false, false]).show(
            ui,
            |ui| {
                if visible.is_empty() {
                    ui.label(RichText::new(t("Inga egna mallar än. Använd Arkiv, Spara som mall… eller importera en fil.")).color(p.muted));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                    for tpl in &visible {
                        let on = self.selected.as_deref() == Some(&key(tpl));
                        let r = card(ui, p, tpl, on);
                        if r.clicked() {
                            pick = Some(key(tpl));
                        }
                        if r.double_clicked() {
                            open = Some(key(tpl));
                        }
                    }
                });
            },
        );
        if let Some(k) = open.clone().or(pick) {
            self.selected = Some(k);
            self.confirm_delete = None;
        }
        let sel = self.selected.as_ref().and_then(|k| self.templates.iter().find(|t| key(t) == *k)).cloned();
        ui.add_space(8.0);
        // Beskrivning av den valda mallen.
        let desc = sel
            .as_ref()
            .map(|s| if s.description.is_empty() { s.category.label().to_owned() } else { s.description.clone() })
            .unwrap_or_default();
        ui.add_sized(
            vec2(ui.available_width(), 34.0),
            egui::Label::new(RichText::new(desc).color(p.muted)).wrap(),
        );
        ui.add_space(6.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let use_btn = egui::Button::new(RichText::new(t("Skapa dokument")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .min_size(vec2(130.0, 32.0));
            if ui.add_enabled(sel.is_some(), use_btn).clicked() {
                open = self.selected.clone();
            }
            if ui.add(crate::app::quiet_button(t("Avbryt"), p).min_size(vec2(0.0, 32.0))).clicked() {
                action = Action::Close;
            }
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui
                    .add(crate::app::quiet_button(t("Importera .docx/.odt…"), p).min_size(vec2(0.0, 32.0)))
                    .on_hover_text(t("Läs en Word- eller OpenDocument-fil som egen mall"))
                    .clicked()
                {
                    action = Action::Import;
                }
                if let Some(s) = sel.as_ref().filter(|s| s.user) {
                    if ui.add(crate::app::quiet_button(t("Byt namn…"), p).min_size(vec2(0.0, 32.0))).clicked() {
                        self.stage = Stage::Name { rename: Some(s.id.clone()), name: s.name.clone(), error: None };
                        self.focus = true;
                    }
                    if self.confirm_delete.as_deref() == Some(&s.id) {
                        let b = egui::Button::new(RichText::new(t("Ta bort på riktigt")).color(Color32::WHITE))
                            .fill(p.error)
                            .stroke(Stroke::NONE)
                            .min_size(vec2(0.0, 32.0));
                        if ui.add(b).clicked() {
                            self.confirm_delete = None;
                            action = match templates::delete_user(&self.dir, &s.id) {
                                Ok(()) => {
                                    self.selected = None;
                                    self.reload();
                                    Action::Done { message: t("Mallen togs bort").to_owned(), error: false, close: false }
                                }
                                Err(e) => Action::Done { message: e, error: true, close: false },
                            };
                        }
                    } else if ui.add(crate::app::quiet_button(t("Ta bort"), p).min_size(vec2(0.0, 32.0))).clicked() {
                        self.confirm_delete = Some(s.id.clone());
                    }
                }
            });
        });
        if let Some(k) = open {
            if let Some(tpl) = self.templates.iter().find(|t| key(t) == k).cloned() {
                let a = self.open_form(tpl);
                if !matches!(a, Action::None) {
                    action = a;
                }
            }
        }
        action
    }

    // ------------------------------------------------------------ formulär

    fn form_ui(&mut self, ui: &mut Ui, p: &Palette) -> Action {
        let mut action = Action::None;
        let Stage::Form { tpl, fields } = &mut self.stage else { return action };
        Self::title(ui, p, &crate::tf!("Fyll i {name}", name = tpl.name));
        ui.label(
            RichText::new(if tpl.doc {
                t("Tomma fält blir gulmarkerade platshållare i dokumentet som du fyller i senare.")
            } else {
                t("Tomma fält lämnas som platshållare i texten.")
            })
            .color(p.muted),
        );
        ui.add_space(10.0);
        let mut first = true;
        let mut submit = false;
        ScrollArea::vertical().id_salt("tpl-form").max_height(380.0).auto_shrink([true, true]).show(ui, |ui| {
            egui::Grid::new("tpl-form-grid").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
                for f in fields.iter_mut() {
                    ui.label(RichText::new(templates::label(&f.key)).color(p.text));
                    let r = ui.add(TextEdit::singleline(&mut f.value).desired_width(360.0));
                    if first && self.focus {
                        r.request_focus();
                        self.focus = false;
                    }
                    first = false;
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    ui.end_row();
                }
            });
        });
        ui.add_space(16.0);
        let mut back = false;
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let b = egui::Button::new(RichText::new(t("Skapa")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .min_size(vec2(96.0, 32.0));
            if ui.add(b).clicked() {
                submit = true;
            }
            if ui.add(crate::app::quiet_button(t("Tillbaka"), p).min_size(vec2(0.0, 32.0))).clicked() {
                back = true;
            }
        });
        if submit {
            let values: Vec<(String, String)> = fields.iter().map(|f| (f.key.clone(), f.value.clone())).collect();
            action = Action::Create { text: templates::create(tpl, &values), doc: tpl.doc };
        }
        if back {
            self.stage = Stage::Gallery;
        }
        action
    }

    // ------------------------------------------------------------ namn

    fn name_ui(&mut self, ui: &mut Ui, p: &Palette) -> Action {
        let mut action = Action::None;
        let Stage::Name { rename, name, error } = &mut self.stage else { return action };
        let saving = rename.is_none();
        Self::title(ui, p, if saving { t("Spara som mall") } else { t("Byt namn på mallen") });
        if saving {
            ui.label(
                RichText::new(t("Dokumentets text sparas som en egen mall. Du hittar den under Mina mallar."))
                    .color(p.muted),
            );
            ui.add_space(8.0);
        }
        let r = ui.add(TextEdit::singleline(name).hint_text(t("Mallens namn")).desired_width(f32::INFINITY));
        if self.focus {
            r.request_focus();
            self.focus = false;
        }
        let mut go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some(e) = error {
            ui.add_space(6.0);
            ui.label(RichText::new(e.as_str()).color(p.error));
        }
        ui.add_space(16.0);
        let mut cancel = false;
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let b = egui::Button::new(RichText::new(t("Spara")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .min_size(vec2(96.0, 32.0));
            if ui.add(b).clicked() {
                go = true;
            }
            if ui.add(crate::app::quiet_button(t("Avbryt"), p).min_size(vec2(0.0, 32.0))).clicked() {
                cancel = true;
            }
        });
        if cancel {
            if saving {
                return Action::Close;
            }
            self.stage = Stage::Gallery;
            return Action::None;
        }
        if go {
            let name = name.clone();
            match rename.clone() {
                Some(old) => match templates::rename_user(&self.dir, &old, &name) {
                    Ok(id) => {
                        self.stage = Stage::Gallery;
                        self.reload();
                        self.selected = Some(format!("u:{id}"));
                        action = Action::Done { message: t("Mallen fick nytt namn").to_owned(), error: false, close: false };
                    }
                    Err(e) => {
                        if let Stage::Name { error, .. } = &mut self.stage {
                            *error = Some(e);
                        }
                    }
                },
                None => action = self.save_now(&name, false),
            }
        }
        action
    }

    /// Sparar dokumentets text som mall. Finns namnet redan frågas först.
    fn save_now(&mut self, name: &str, overwrite: bool) -> Action {
        let Some((text, doc)) = self.save_text.clone() else { return Action::Close };
        let text = templates::strip_unportable(&text);
        let exists = templates::sanitize_name(name).is_some_and(|id| self.dir.join(format!("{id}.md")).exists());
        if exists && !overwrite {
            self.stage = Stage::Replace { name: name.to_owned() };
            return Action::None;
        }
        match templates::save_user(&self.dir, name, &text, doc, overwrite) {
            Ok(id) => Action::Done { message: crate::tf!("Sparade mallen {name}", name = id), error: false, close: true },
            Err(e) => {
                self.stage = Stage::Name { rename: None, name: name.to_owned(), error: Some(e) };
                Action::None
            }
        }
    }

    fn replace_ui(&mut self, ui: &mut Ui, p: &Palette) -> Action {
        let Stage::Replace { name } = &self.stage else { return Action::None };
        let name = name.clone();
        Self::title(ui, p, t("Ersätta mallen?"));
        ui.label(RichText::new(crate::tf!("Det finns redan en mall som heter {name}. Vill du ersätta den?", name = name)).color(p.text));
        ui.add_space(16.0);
        let mut action = Action::None;
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let b = egui::Button::new(RichText::new(t("Ersätt")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .min_size(vec2(96.0, 32.0));
            if ui.add(b).clicked() {
                action = self.save_now(&name, true);
            }
            if ui.add(crate::app::quiet_button(t("Ändra namn"), p).min_size(vec2(0.0, 32.0))).clicked() {
                self.stage = Stage::Name { rename: None, name: name.clone(), error: None };
                self.focus = true;
            }
        });
        action
    }
}

/// Unik nyckel för valet i galleriet (inbyggda och egna kan ha samma id).
fn key(t: &Template) -> String {
    format!("{}:{}", if t.user { "u" } else { "b" }, t.id)
}

// ---------------------------------------------------------------- kort

fn card(ui: &mut Ui, p: &Palette, tpl: &Template, selected: bool) -> egui::Response {
    let aspect = if tpl.doc { templates::page_aspect(&tpl.body) } else { 0.75 };
    let page_h = (PAGE_W / aspect).min(PAGE_MAX_H);
    let page_w = (page_h * aspect).min(PAGE_W);
    let h = PAGE_MAX_H + 62.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(CARD_W, h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter();
    let hovered = resp.hovered();
    painter.rect_filled(rect, 10.0, if selected || hovered { p.surface_hover } else { p.surface });
    painter.rect_stroke(
        rect,
        10.0,
        if selected { Stroke::new(2.0, p.accent) } else { Stroke::new(1.0, p.line) },
        StrokeKind::Inside,
    );
    // Sidan, med skugga. Texten börjar överst och sidan är fast placerad så korten linjerar.
    let page = Rect::from_min_size(Pos2::new(rect.center().x - page_w / 2.0, rect.top() + 12.0), vec2(page_w, page_h));
    painter.rect_filled(page.translate(vec2(0.0, 2.0)).expand(0.5), 2.0, p.page_shadow);
    painter.rect_filled(page, 2.0, p.page);
    paint_sketch(painter, p, page, tpl);
    // Namn och kategori under sidan.
    let name_pos = Pos2::new(rect.left() + 10.0, rect.top() + PAGE_MAX_H + 22.0);
    let galley = painter.layout(
        tpl.name.clone(),
        FontId::new(13.0, theme::medium()),
        p.text,
        CARD_W - 20.0,
    );
    painter.galley(name_pos, galley, p.text);
    let sub = if tpl.doc { t("Dokument") } else { t("Markdown") };
    painter.text(
        Pos2::new(rect.left() + 10.0, rect.bottom() - 10.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{} · {sub}", tpl.category.label()),
        FontId::new(11.0, FontFamily::Proportional),
        p.muted,
    );
    resp
}

/// Miniatyren: grå staplar för text, färgade för rubriker och rutor för tabeller.
fn paint_sketch(painter: &egui::Painter, p: &Palette, page: Rect, tpl: &Template) {
    let inner = page.shrink2(vec2(page.width() * 0.11, page.height() * 0.07));
    let body_c = p.page_muted.gamma_multiply(0.55);
    let head_c = if tpl.doc { p.page_heading1 } else { p.page_text };
    let mut y = inner.top();
    let bar = |x0: f32, w: f32, y: f32, h: f32, c: Color32| {
        painter.rect_filled(Rect::from_min_size(Pos2::new(x0, y), vec2(w.max(2.0), h)), CornerRadius::same(1), c);
    };
    if !tpl.doc {
        // Markdown: små märken i vänsterkanten som i en kodeditor.
        painter.text(page.right_top() + vec2(-3.0, 3.0), egui::Align2::RIGHT_TOP, "MD", FontId::new(8.0, FontFamily::Monospace), p.page_muted);
    }
    for s in templates::sketch(&tpl.body, 40) {
        if y > inner.bottom() - 2.0 {
            break;
        }
        match s {
            Sketch::Heading(level, w) => {
                let h = [5.5, 4.0, 3.0][level as usize - 1];
                bar(inner.left(), inner.width() * w, y, h, head_c);
                y += h + 3.5;
            }
            Sketch::Line(w) => {
                bar(inner.left(), inner.width() * w, y, 2.2, body_c);
                y += 4.6;
            }
            Sketch::Centered(w) => {
                let bw = inner.width() * w;
                bar(inner.center().x - bw / 2.0, bw, y, 2.6, body_c);
                y += 5.0;
            }
            Sketch::Item(w) => {
                bar(inner.left() + 2.0, 2.2, y, 2.2, head_c);
                bar(inner.left() + 7.0, inner.width() * (w * 0.85).max(0.15), y, 2.2, body_c);
                y += 4.6;
            }
            Sketch::Row(cols) => {
                let r = Rect::from_min_size(Pos2::new(inner.left(), y), vec2(inner.width(), 6.0));
                painter.rect_stroke(r, 0.0, Stroke::new(0.6, p.page_muted.gamma_multiply(0.6)), StrokeKind::Inside);
                for c in 1..cols.min(6) {
                    let x = r.left() + r.width() * c as f32 / cols.min(6) as f32;
                    painter.vline(x, r.y_range(), Stroke::new(0.6, p.page_muted.gamma_multiply(0.6)));
                }
                y += 6.0;
            }
            Sketch::Gap => y += 3.5,
        }
    }
}
