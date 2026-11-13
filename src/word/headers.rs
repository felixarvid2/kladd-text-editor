//! Sidhuvud och sidfot i dokumentläget: ritning på arken (vänster, mitten och höger med fälten
//! ifyllda) och dialogen "Sidhuvud och sidfot" som öppnas med dubbelklick i huvud- eller
//! fotområdet.
//!
//! Själva formatet (`header: "Rapport|{titel}|{sida}"`, `header-first`) och fältens utfyllnad
//! finns i `style`; här finns bara gränssnittet. Dialogen skriver tillbaka till grundformatsraden
//! på samma sätt som "Sidinställningar".

use egui::{
    Align, Align2, Color32, FontId, Frame, Id, Layout, Margin, Painter, Rect, RichText, Sense, Stroke, Ui, pos2, vec2,
};

use super::markup::{self, DocDefaults};
use super::pages::{Geom, set_defaults_line};
use crate::app::{KladdApp, quiet_button};
use crate::i18n::t;
use crate::style::{self, Fields};
use crate::tf;
use crate::theme::{self, Palette};

/// Ritar sidhuvud och sidfot på ett ark och ger `Some(true)` vid dubbelklick på sidfoten och
/// `Some(false)` på sidhuvudet. `fields` har sidnummer 1 och är mallen för alla sidor.
#[allow(clippy::too_many_arguments)]
pub fn header_footer(
    ui: &mut Ui,
    p: &Palette,
    geom: &Geom,
    sheet: Rect,
    k: usize,
    total: usize,
    d: &DocDefaults,
    fields: &Fields,
    font: FontId,
    doc_id: u64,
) -> Option<bool> {
    let mut open = None;
    let painter: Painter = ui.painter().clone();
    let f = fields.at(k + 1, total);
    let zones = [
        (false, Rect::from_min_max(sheet.min, pos2(sheet.right(), sheet.top() + geom.top)), d.header_template(k), d.header_align),
        (true, Rect::from_min_max(pos2(sheet.left(), sheet.bottom() - geom.bottom), sheet.max), d.footer_template(k), d.footer_align),
    ];
    for (footer, zone, text, align) in zones {
        let resp = ui.interact(zone, ui.make_persistent_id(("kladd-hf", doc_id, k, footer)), Sense::click());
        if resp.double_clicked() {
            open = Some(footer);
        }
        resp.on_hover_text(t("Dubbelklicka för att ändra sidhuvud och sidfot"));
        let Some(t) = text else { continue };
        let y = zone.center().y;
        for (i, part) in style::parts_of(t, align).iter().enumerate() {
            let part = style::fill_fields(part, &f).replace('\n', " ");
            if part.is_empty() {
                continue;
            }
            let (x, anchor) = match i {
                1 => (sheet.center().x, Align2::CENTER_CENTER),
                2 => (sheet.right() - geom.right, Align2::RIGHT_CENTER),
                _ => (sheet.left() + geom.left, Align2::LEFT_CENTER),
            };
            painter.text(pos2(x, y), anchor, part, font.clone(), p.page_muted);
        }
    }
    open
}

/// Delarna (vänster, mitten, höger) i en rad.
type Row = [String; 3];

/// Dialogens tillstånd medan den är öppen.
pub struct HfDialog {
    /// Visar sidfoten (annars sidhuvudet).
    footer: bool,
    /// Visar första sidans variant.
    first: bool,
    different_first: bool,
    /// `[sidhuvud, sidfot][vanliga sidor, första sidan]`.
    rows: [[Row; 2]; 2],
    /// Fältet (0 vänster, 1 mitten, 2 höger) som senast hade markören; dit infogas fälten.
    focus: usize,
    /// Dokumentets övriga fält som ska bevaras.
    base: DocDefaults,
    /// Filnamnet, för förhandsvisningen.
    name: String,
    /// Fält som ska få markören vid nästa bildruta.
    want_focus: bool,
}

impl HfDialog {
    /// `footer`: dubbelklicket gällde sidfoten; `first_page`: det gällde första sidan.
    pub fn from_defaults(d: &DocDefaults, footer: bool, first_page: bool, name: &str) -> Self {
        let row = |t: Option<&String>, a| style::parts_of(t.map_or("", String::as_str), a);
        let rows = [
            [row(d.header.as_ref(), d.header_align), row(d.header_first.as_ref(), d.header_align)],
            [row(d.footer.as_ref(), d.footer_align), row(d.footer_first.as_ref(), d.footer_align)],
        ];
        let different_first = d.different_first();
        Self {
            footer,
            first: first_page && different_first,
            different_first,
            rows,
            focus: 1,
            base: d.clone(),
            name: name.to_owned(),
            want_focus: true,
        }
    }

    /// Dokumentets grundformat med dialogens val.
    pub fn to_defaults(&self) -> DocDefaults {
        let mut d = self.base.clone();
        let (h, h_align) = style::join_parts(&self.rows[0][0]);
        let (f, f_align) = style::join_parts(&self.rows[1][0]);
        d.header = Some(h.clone()).filter(|t| !t.is_empty());
        d.footer = Some(f.clone()).filter(|t| !t.is_empty());
        if !h.is_empty() {
            d.header_align = h_align;
        }
        if !f.is_empty() {
            d.footer_align = f_align;
        }
        if self.different_first {
            d.header_first = Some(join_with_align(&self.rows[0][1], d.header_align));
            d.footer_first = Some(join_with_align(&self.rows[1][1], d.footer_align));
        } else {
            d.header_first = None;
            d.footer_first = None;
        }
        d
    }

    fn row_mut(&mut self) -> &mut Row {
        &mut self.rows[self.footer as usize][(self.first && self.different_first) as usize]
    }
}

/// Mallen för en rad, med en justering som bevaras även för ett enda värde (första sidan delar
/// justering med de andra sidorna).
fn join_with_align(p: &Row, align: markup::Align) -> String {
    let (t, a) = style::join_parts(p);
    if !t.is_empty() && a != align && style::split_parts(&t).is_none() {
        let esc = |s: &str| s.replace('|', "\\|");
        return format!("{}|{}|{}", esc(&p[0]), esc(&p[1]), esc(&p[2]));
    }
    t
}

enum Act {
    Ok,
    Cancel,
}

impl KladdApp {
    /// Öppnar "Sidhuvud och sidfot" för det aktiva dokumentet. `page` är sidan (0-baserad) som
    /// dubbelklickades.
    pub(crate) fn open_header_dialog(&mut self, footer: bool, page: usize) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let a = self.word.analysis(doc.id, &doc.text);
        let name = doc.title();
        self.wv.hf_dlg = Some(HfDialog::from_defaults(&a.defaults, footer, page == 0, &name));
    }

    pub(crate) fn header_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(d) = self.wv.hf_dlg.as_mut() else { return };
        let mut act = None;
        let modal = egui::Modal::new(Id::unique("kladd-hf-dialog"))
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
                ui.set_width(480.0);
                ui.label(RichText::new(t("Sidhuvud och sidfot")).font(FontId::new(17.0, theme::semibold())).color(p.text));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut d.footer, false, t("Sidhuvud"));
                    ui.selectable_value(&mut d.footer, true, t("Sidfot"));
                    if d.different_first {
                        ui.add_space(14.0);
                        ui.selectable_value(&mut d.first, false, t("Övriga sidor"));
                        ui.selectable_value(&mut d.first, true, t("Första sidan"));
                    }
                });
                ui.add_space(10.0);
                let focus = d.focus;
                let want_focus = std::mem::take(&mut d.want_focus);
                let mut new_focus = None;
                egui::Grid::new("kladd-hf-grid").num_columns(2).spacing(vec2(14.0, 8.0)).show(ui, |ui| {
                    for (i, label) in [t("Vänster"), t("Mitten"), t("Höger")].into_iter().enumerate() {
                        ui.label(RichText::new(label).color(p.muted));
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut d.row_mut()[i]).desired_width(340.0).margin(Margin::symmetric(8, 5)),
                        );
                        if want_focus && i == focus {
                            r.request_focus();
                        }
                        if r.has_focus() {
                            new_focus = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if let Some(i) = new_focus {
                    d.focus = i;
                }
                ui.add_space(8.0);
                ui.label(RichText::new(t("Infoga i det markerade fältet")).color(p.muted).size(12.5));
                ui.horizontal_wrapped(|ui| {
                    for (code, label) in style::FIELD_NAMES {
                        if ui.add(quiet_button(t(label), p)).clicked() {
                            let f = d.focus;
                            d.row_mut()[f].push_str(code);
                            d.want_focus = true;
                        }
                    }
                });
                ui.add_space(8.0);
                if ui.checkbox(&mut d.different_first, t("Annorlunda första sida")).changed() && !d.different_first {
                    d.first = false;
                }
                // Förhandsvisning av raden som den ser ut på sida 1 av 3.
                let f = Fields::new(1, 3, &d.name);
                let [l, c, r] = d.row_mut().clone().map(|t| style::fill_fields(&t, &f));
                ui.add_space(6.0);
                ui.label(RichText::new(tf!("Förhandsvisning:  {l}   |   {c}   |   {r}", l = l, c = c, r = r)).color(p.muted).size(12.5));
                ui.add_space(20.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let ok = egui::Button::new(RichText::new(t("OK")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                        .fill(p.accent)
                        .stroke(Stroke::NONE)
                        .min_size(vec2(84.0, 32.0));
                    if ui.add(ok).clicked() {
                        act = Some(Act::Ok);
                    }
                    if ui.add(quiet_button(t("Avbryt"), p).min_size(vec2(0.0, 32.0))).clicked() {
                        act = Some(Act::Cancel);
                    }
                });
            });
        if modal.should_close() && act.is_none() {
            act = Some(Act::Cancel);
        }
        let Some(act) = act else { return };
        let Some(d) = self.wv.hf_dlg.take() else { return };
        if matches!(act, Act::Cancel) {
            return;
        }
        let new = d.to_defaults();
        self.word_edit(ctx, |t, sel| {
            let (text, old_len, new_len) = set_defaults_line(t, &new);
            let map = |b: usize| if b >= old_len { b - old_len + new_len } else { b.min(new_len) };
            markup::Edit { text, sel: map(sel.start)..map(sel.end) }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use markup::Align as HAlign;

    #[test]
    fn dialog_reads_and_writes_single_and_three_part_forms() {
        let d = DocDefaults {
            header: Some("Rapport|{titel}|{datum}".into()),
            footer: Some("{sida}".into()),
            footer_align: HAlign::Center,
            ..Default::default()
        };
        let mut dlg = HfDialog::from_defaults(&d, true, false, "rapport.docx");
        assert_eq!(dlg.rows[0][0], ["Rapport", "{titel}", "{datum}"].map(String::from));
        assert_eq!(dlg.rows[1][0], ["", "{sida}", ""].map(String::from));
        // Oförändrad dialog ger samma inställningar tillbaka.
        assert_eq!(dlg.to_defaults(), d);
        // Mittendelen i sidfoten flyttas till höger: det gamla enkla värdet med högerjustering.
        dlg.rows[1][0] = ["".into(), "".into(), "{sida}".into()];
        let out = dlg.to_defaults();
        assert_eq!((out.footer.as_deref(), out.footer_align), (Some("{sida}"), HAlign::Right));
        // Tömd rad tar bort sidhuvudet.
        dlg.rows[0][0] = Default::default();
        assert_eq!(dlg.to_defaults().header, None);
    }

    #[test]
    fn dialog_first_page_variant() {
        let d = DocDefaults { footer: Some("{sida}".into()), footer_align: HAlign::Center, ..Default::default() };
        let mut dlg = HfDialog::from_defaults(&d, true, true, "a.md");
        assert!(!dlg.different_first && !dlg.first);
        dlg.different_first = true;
        dlg.first = true;
        // Tom första sida: båda varianterna blir tomma strängar (inte `None`).
        let out = dlg.to_defaults();
        assert_eq!((out.header_first.as_deref(), out.footer_first.as_deref()), (Some(""), Some("")));
        assert_eq!(out.footer_template(0), None);
        assert_eq!(out.footer_template(1), Some("{sida}"));
        // Första sidans sidfot får egen text med annan justering än de övriga.
        dlg.row_mut()[2] = "Utkast".into();
        let out = dlg.to_defaults();
        assert_eq!(out.footer_first.as_deref(), Some("||Utkast"));
        assert_eq!(style::parts_of(out.footer_template(0).unwrap(), out.footer_align)[2], "Utkast");
        // Avbockad ruta tar bort varianterna.
        dlg.different_first = false;
        let out = dlg.to_defaults();
        assert!(!out.different_first());
    }
}
