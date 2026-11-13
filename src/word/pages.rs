//! Riktiga sidor i dokumentläget: sidgeometri från dokumentets grundformat, sidbrytning av
//! galleriet, sidhuvud/sidfot och dialogen "Sidinställningar".
//!
//! Galleriet läggs ut som en enda lång spalt med textbredden. `paginate` flyttar sedan ner de
//! rader som inte får plats på sin sida, så att varje sida börjar överst i textytan. Eftersom
//! raderna flyttas i själva galleriet följer markör, markering och träffytor med av sig själva.

use std::hash::{Hash, Hasher};

use egui::epaint::text::Galley;
use egui::{
    Align, Color32, FontFamily, FontId, Frame, Id, Layout, Margin, Rect, RichText, Shape, Stroke,
    Ui, vec2,
};

use super::layout::px;
use super::markup::{self, Align as HAlign, DocDefaults};
use super::secpages::PageBox;
use crate::app::{KladdApp, quiet_button};
use crate::i18n::t;
use crate::tf;
use crate::theme::{self, Palette};

/// Luft (px, ozoomad) mellan två ark på skrivbordet.
pub const SHEET_GAP: f32 = 24.0;

/// Sidans mått i px vid en given zoom.
#[derive(Clone, Debug, PartialEq)]
pub struct Geom {
    pub page_w: f32,
    pub page_h: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub gap: f32,
    /// Avsnittens sidgeometri när dokumentet har avsnitt med egen sida eller spalter
    /// (`Geom::for_sections`); tom för ett vanligt dokument.
    pub secs: std::sync::Arc<Vec<super::secpages::SecGeom>>,
}

impl Geom {
    pub fn new(zoom: f32, d: &DocDefaults) -> Self {
        let (w, h) = d.page();
        let [t, r, b, l] = d.margins();
        // Marginalerna får aldrig äta upp hela sidan.
        let cap = |m: f32, dim: f32| m.clamp(0.0, dim * 0.4);
        Self {
            page_w: px(zoom, w),
            page_h: px(zoom, h),
            top: px(zoom, cap(t, h)),
            right: px(zoom, cap(r, w)),
            bottom: px(zoom, cap(b, h)),
            left: px(zoom, cap(l, w)),
            gap: SHEET_GAP,
            secs: Default::default(),
        }
    }

    pub fn text_w(&self) -> f32 {
        (self.page_w - self.left - self.right).max(1.0)
    }

    /// Måtten för ett ark (marginalerna till sidhuvud och sidfot).
    pub fn for_page(&self, b: &PageBox) -> Geom {
        Geom {
            page_w: b.w,
            page_h: b.h,
            top: b.margin[0],
            right: b.margin[1],
            bottom: b.margin[2],
            left: b.margin[3],
            gap: self.gap,
            secs: Default::default(),
        }
    }

    pub fn text_h(&self) -> f32 {
        (self.page_h - self.top - self.bottom).max(1.0)
    }

    /// Avståndet mellan två sidors textytor (och mellan två arks överkanter).
    pub fn stride(&self) -> f32 {
        self.page_h + self.gap
    }

    pub fn hash_into<H: Hasher>(&self, h: &mut H) {
        for v in [self.page_w, self.page_h, self.top, self.right, self.bottom, self.left, self.gap] {
            v.to_bits().hash(h);
        }
        self.hash_secs(h);
    }

    /// Sidan (från 0) som y-värdet (px från galleriets överkant) ligger på.
    pub fn page_of(&self, y: f32) -> usize {
        (((y + 0.5) / self.stride()).floor().max(0.0)) as usize
    }
}

/// Flyttar ner rader som inte ryms på sin sida så att de börjar överst på nästa sida. Rader
/// delas aldrig; en rad som är högre än en sida står ensam på sin. Galleriets y = 0 är textytans
/// överkant på första sidan. `groups` är radintervall (stigande, utan överlapp) som hör ihop och
/// flyttas tillsammans, till exempel cellernas rader i en tabellrad.
///
/// `floats` = (radnummer, höjd) för raden som en flytande bild förankras på: bilden räknas som en
/// rad av sin höjd, så att en bild som inte ryms flyttas till nästa sida tillsammans med texten
/// efter den.
pub fn paginate(g: &mut Galley, geom: &Geom, ppp: f32, floats: &[(usize, f32)], groups: &[std::ops::Range<usize>]) {
    if g.rows.is_empty() {
        return;
    }
    let text_h = geom.text_h();
    let stride = geom.stride();
    let eps = 0.5 / ppp;
    let round = |v: f32| (v * ppp).round() / ppp;
    let mut page = 0usize;
    let mut shift = 0.0f32;
    let mut prev_bottom = f32::NEG_INFINITY;
    let mut float_bottom = f32::NEG_INFINITY;
    let mut next_group = 0;
    let mut i = 0;
    while i < g.rows.len() {
        while next_group < groups.len() && groups[next_group].start < i {
            next_group += 1;
        }
        let end = match groups.get(next_group) {
            Some(r) if r.start == i && r.end > i => r.end.min(g.rows.len()),
            _ => i + 1,
        };
        let (mut y0, mut y1) = (f32::INFINITY, f32::NEG_INFINITY);
        for pr in &g.rows[i..end] {
            y0 = y0.min(pr.pos.y);
            y1 = y1.max(pr.pos.y + pr.row.size.y);
        }
        let h = y1 - y0;
        let fh = floats.iter().find(|f| f.0 == i).map_or(0.0, |f| f.1);
        let top = y0 + shift;
        let page_top = page as f32 * stride;
        if top + h.max(fh) > page_top + text_h + eps && top > page_top + eps {
            // Nästa sida, men inte över en rad som sträcker sig förbi arket.
            page += 1;
            while (page as f32 * stride) < prev_bottom - eps {
                page += 1;
            }
            shift = round(page as f32 * stride - y0);
        }
        for pr in &mut g.rows[i..end] {
            pr.pos.y += shift;
        }
        prev_bottom = y0 + shift + h;
        if fh > 0.0 {
            float_bottom = float_bottom.max(y0 + shift + fh);
        }
        i = end;
    }
    g.rect.max.y = g.rect.max.y.max(prev_bottom).max(float_bottom);
    g.mesh_bounds = Rect::NOTHING;
    for pr in &g.rows {
        g.mesh_bounds |= pr.row.visuals.mesh_bounds.translate(pr.pos.to_vec2());
    }
}

/// Antal sidor i ett paginerat galleri (minst 1).
pub fn page_count(g: &Galley, geom: &Geom) -> usize {
    g.rows.last().map_or(1, |r| geom.page_of(r.pos.y) + 1)
}

/// Ritar arken (skugga och vitt papper). `base` är skrivbordsytans vänsterkant och `origin_y` y för
/// galleriets y = 0 (textytans överkant på första sidan). Bara ark som syns inom `clip` ritas.
pub fn sheet_shapes(p: &Palette, boxes: &[PageBox], base: f32, origin_y: f32, clip: Rect) -> (Shape, Shape) {
    let mut shadows = Vec::new();
    let mut sheets = Vec::new();
    for b in boxes {
        let r = b.rect(base, origin_y);
        if r.bottom() + 8.0 < clip.top() || r.top() - 8.0 > clip.bottom() {
            continue;
        }
        shadows.push(Shape::rect_filled(r.translate(vec2(0.0, 2.0)).expand(1.5), 2.0, p.page_shadow));
        sheets.push(Shape::rect_filled(r, 1.0, p.page));
    }
    (Shape::Vec(shadows), Shape::Vec(sheets))
}

/// Sidhuvudets/sidfotens typsnitt: brödtextens, 0,9 gånger så stort.
pub fn header_font(zoom: f32, d: &DocDefaults, family: FontFamily) -> FontId {
    FontId::new(px(zoom, d.size() * 0.9), family)
}

// ---------------------------------------------------------------------------------------------
// Skrivning av grundformatsraden

/// Ersätter (eller lägger till/tar bort) första radens `<!-- kladd: … -->` med `new`. Ger den nya
/// texten och längden (byte) på den gamla och nya inledningen, så att markeringen kan flyttas.
pub fn set_defaults_line(text: &str, new: &DocDefaults) -> (String, usize, usize) {
    let comment = new.to_comment();
    let first_end = text.find('\n').unwrap_or(text.len());
    let has_line = DocDefaults::from_comment(&text[..first_end]).is_some();
    if has_line {
        let mut cut = first_end;
        if comment.is_empty() {
            // Raden och den tomma raden efter den försvinner.
            cut = (first_end + 1).min(text.len());
            if text[cut..].starts_with('\n') {
                cut += 1;
            } else if text[cut..].starts_with("\r\n") {
                cut += 2;
            }
        }
        let rest = &text[cut..];
        let out = if comment.is_empty() { rest.to_owned() } else { format!("{comment}{rest}") };
        (out, cut, comment.len())
    } else if comment.is_empty() {
        (text.to_owned(), 0, 0)
    } else {
        (format!("{comment}\n\n{text}"), 0, comment.len() + 2)
    }
}

// ---------------------------------------------------------------------------------------------
// Dialogen

/// Pappersformaten i listan: namn och mått (pt, stående), samma som `style` känner igen.
const PAPERS: [(&str, (f32, f32)); 5] = [
    ("A4", (595.28, 841.89)),
    ("A5", (419.53, 595.28)),
    ("A3", (841.89, 1190.55)),
    ("Letter", (612.0, 792.0)),
    ("Legal", (612.0, 1008.0)),
];
const MM: f32 = 72.0 / 25.4;
const CUSTOM: usize = PAPERS.len();

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.5
}

/// Dialogens tillstånd medan den är öppen.
pub struct PageDialog {
    /// Index i `PAPERS`, eller `CUSTOM` för ett eget mått.
    paper: usize,
    /// Stående mått (pt) för ett eget format.
    custom: (f32, f32),
    landscape: bool,
    /// Marginaler i mm: topp, höger, botten, vänster.
    margins: [f32; 4],
    header: String,
    header_align: HAlign,
    footer: String,
    footer_align: HAlign,
    /// Dokumentets övriga fält (typsnitt, storlek) som ska bevaras.
    base: DocDefaults,
}

impl PageDialog {
    pub fn from_defaults(d: &DocDefaults) -> Self {
        let (w, h) = d.page();
        let (pw, ph) = (w.min(h), w.max(h));
        let paper = PAPERS.iter().position(|(_, (a, b))| close(pw, *a) && close(ph, *b)).unwrap_or(CUSTOM);
        Self {
            paper,
            custom: (pw, ph),
            landscape: w > h + 0.5,
            margins: d.margins().map(|m| (m / MM * 10.0).round() / 10.0),
            header: d.header.clone().unwrap_or_default(),
            header_align: d.header_align,
            footer: d.footer.clone().unwrap_or_default(),
            footer_align: d.footer_align,
            base: d.clone(),
        }
    }

    /// Dokumentets grundformat med dialogens val; typsnitt och storlek behålls.
    pub fn to_defaults(&self) -> DocDefaults {
        let (w, h) = if self.paper == CUSTOM { self.custom } else { PAPERS[self.paper].1 };
        let size = if self.landscape { (h, w) } else { (w, h) };
        let mut d = self.base.clone();
        d.page_size = Some(size);
        d.page_margin = Some(self.margins.map(|m| m * MM));
        d.header = Some(self.header.clone()).filter(|t| !t.is_empty());
        d.header_align = self.header_align;
        d.footer = Some(self.footer.clone()).filter(|t| !t.is_empty());
        d.footer_align = self.footer_align;
        d
    }

    fn paper_label(&self) -> String {
        if self.paper == CUSTOM {
            let (w, h) = self.custom;
            tf!("Eget ({w} × {h} mm)", w = format!("{:.0}", w / MM), h = format!("{:.0}", h / MM))
        } else {
            PAPERS[self.paper].0.to_owned()
        }
    }
}

fn align_row(ui: &mut Ui, id: &str, value: &mut HAlign) {
    ui.horizontal(|ui| {
        let _ = id;
        for (a, name) in [(HAlign::Left, t("Vänster")), (HAlign::Center, t("Centrerad")), (HAlign::Right, t("Höger"))] {
            ui.selectable_value(value, a, name);
        }
    });
}

enum Act {
    Ok,
    Cancel,
}

impl KladdApp {
    /// Öppnar "Sidinställningar" för det aktiva dokumentet.
    pub(crate) fn open_page_dialog(&mut self) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let a = self.word.analysis(doc.id, &doc.text);
        self.wv.page_dlg = Some(PageDialog::from_defaults(&a.defaults));
    }

    pub(crate) fn page_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(d) = self.wv.page_dlg.as_mut() else { return };
        let mut act = None;
        let modal = egui::Modal::new(Id::unique("kladd-page-setup"))
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
                ui.set_width(460.0);
                ui.label(RichText::new(t("Sidinställningar")).font(FontId::new(17.0, theme::semibold())).color(p.text));
                ui.add_space(14.0);
                egui::Grid::new("kladd-page-grid").num_columns(2).spacing(vec2(14.0, 10.0)).show(ui, |ui| {
                    ui.label(RichText::new(t("Pappersstorlek")).color(p.muted));
                    egui::ComboBox::from_id_salt("kladd-page-paper").selected_text(d.paper_label()).width(220.0).show_ui(
                        ui,
                        |ui| {
                            for (i, (name, _)) in PAPERS.iter().enumerate() {
                                ui.selectable_value(&mut d.paper, i, *name);
                            }
                            if d.paper == CUSTOM {
                                let l = d.paper_label();
                                ui.selectable_value(&mut d.paper, CUSTOM, l);
                            }
                        },
                    );
                    ui.end_row();

                    ui.label(RichText::new(t("Orientering")).color(p.muted));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut d.landscape, false, t("Stående"));
                        ui.selectable_value(&mut d.landscape, true, t("Liggande"));
                    });
                    ui.end_row();

                    ui.label(RichText::new(t("Marginaler")).color(p.muted));
                    ui.horizontal(|ui| {
                        for (label, m) in [("Normala (25,4 mm)", 25.4), ("Smala (12,7 mm)", 12.7), ("Breda (50,8 mm)", 50.8)] {
                            if ui.add(quiet_button(t(label), p)).clicked() {
                                d.margins = [m; 4];
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("");
                    ui.horizontal(|ui| {
                        for (label, i) in [("Överkant", 0), ("Höger", 1), ("Nederkant", 2), ("Vänster", 3)] {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(t(label)).color(p.muted).size(12.0));
                                ui.add(
                                    egui::DragValue::new(&mut d.margins[i]).range(0.0..=120.0).speed(0.5).suffix(" mm").max_decimals(1),
                                );
                            });
                        }
                    });
                    ui.end_row();

                    ui.label(RichText::new(t("Sidhuvud")).color(p.muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut d.header).desired_width(300.0).margin(Margin::symmetric(8, 5)),
                    );
                    ui.end_row();
                    ui.label("");
                    align_row(ui, "kladd-page-halign", &mut d.header_align);
                    ui.end_row();

                    ui.label(RichText::new(t("Sidfot")).color(p.muted));
                    ui.add(
                        egui::TextEdit::singleline(&mut d.footer).desired_width(300.0).margin(Margin::symmetric(8, 5)),
                    );
                    ui.end_row();
                    ui.label("");
                    align_row(ui, "kladd-page-falign", &mut d.footer_align);
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(t("Fält: {sida} {sidor} {datum} {titel} {fil}. Med | delas texten i vänster, mitten och höger. Dubbelklicka på sidhuvudet i dokumentet för fler val."))
                        .color(p.muted)
                        .size(12.5),
                );
                ui.add_space(6.0);
                if ui.add(quiet_button(t("Sidnummer i sidfoten"), p)).clicked() {
                    d.footer = "{sida}".to_owned();
                    d.footer_align = HAlign::Center;
                }
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
        let Some(d) = self.wv.page_dlg.take() else { return };
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
    use egui::text::{CCursor, LayoutJob, TextFormat};
    use std::sync::Arc;

    fn geom() -> Geom {
        Geom { page_w: 300.0, page_h: 400.0, top: 40.0, right: 30.0, bottom: 40.0, left: 30.0, gap: 24.0, secs: Default::default() }
    }

    /// Ett galleri med många rader (en per stycke) som `paginate` sedan bryter.
    fn paged(n: usize, tall: Option<(usize, f32)>) -> Arc<Galley> {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut out = None;
        for _ in 0..2 {
            let r = ctx.run_ui(Default::default(), |ui| {
                let mut job = LayoutJob::default();
                for i in 0..n {
                    let h = match tall {
                        Some((at, h)) if at == i => h,
                        _ => 17.0,
                    };
                    job.append(
                        &format!("rad {i}\n"),
                        0.0,
                        TextFormat { font_id: FontId::proportional(14.0), line_height: Some(h), ..Default::default() },
                    );
                }
                job.wrap.max_width = 200.0;
                let mut g = ui.ctx().fonts_mut(|f| f.layout_job(job));
                paginate(Arc::make_mut(&mut g), &geom(), 1.0, &[], &[]);
                out = Some(g);
            });
            std::mem::forget(r);
        }
        out.unwrap()
    }

    #[test]
    fn rader_korsar_aldrig_sidgranser() {
        let geom = geom();
        let g = paged(80, None);
        let pages = page_count(&g, &geom);
        assert!(pages >= 3, "{pages}");
        for r in &g.rows {
            let page = geom.page_of(r.pos.y);
            let top = page as f32 * geom.stride();
            assert!(r.pos.y >= top - 0.01, "raden ovanför sin sida");
            assert!(r.pos.y + r.row.size.y <= top + geom.text_h() + 0.01, "raden korsar sidans nederkant");
        }
        // Första raden på varje sida ligger överst i textytan.
        for k in 1..pages {
            let first = g.rows.iter().find(|r| geom.page_of(r.pos.y) == k).unwrap();
            assert!((first.pos.y - k as f32 * geom.stride()).abs() < 0.01);
        }
        // Ordningen bevaras och galleriet omsluter alla rader.
        assert!(g.rows.windows(2).all(|w| w[0].pos.y < w[1].pos.y));
        assert!(g.rect.bottom() >= g.rows.last().unwrap().max_y());
    }

    #[test]
    fn markor_och_position_foljer_de_flyttade_raderna() {
        let geom = geom();
        let g = paged(80, None);
        let text = g.text().to_owned();
        for (ci, _) in text.char_indices().step_by(5) {
            let c = CCursor::new(text[..ci].chars().count());
            let r = g.pos_from_cursor(c);
            // Rektangeln ligger på en rad, och en träff på dess mitt ger tillbaka samma rad.
            let back = g.cursor_from_pos(r.center().to_vec2() + vec2(1.0, 0.0));
            let rb = g.pos_from_cursor(back);
            assert!((rb.top() - r.top()).abs() < 0.01, "markör {ci}: {r:?} -> {rb:?}");
            let page = geom.page_of(r.top());
            assert!(r.bottom() <= page as f32 * geom.stride() + geom.text_h() + 0.01);
        }
        // Teckenräkningen är intakt.
        let chars: usize = g.rows.iter().map(|r| r.char_count_including_newline().0).sum();
        assert_eq!(chars, text.chars().count());
    }

    #[test]
    fn rad_hogre_an_en_sida_star_ensam() {
        let geom = geom();
        let g = paged(10, Some((4, 500.0)));
        let tall = &g.rows[4];
        assert!(tall.row.size.y >= 500.0);
        let page = geom.page_of(tall.pos.y);
        assert!((tall.pos.y - page as f32 * geom.stride()).abs() < 0.01);
        // Raden efter ligger på en senare sida.
        assert!(geom.page_of(g.rows[5].pos.y) > page);
        assert!(g.rows[5].pos.y >= tall.pos.y + 500.0);
    }

    #[test]
    fn grundformatsraden_skrivs_och_tas_bort() {
        let base = DocDefaults { header: Some("Hej".into()), ..Default::default() };
        // Läggs till i ett dokument utan rad.
        let (t, old, new) = set_defaults_line("text\n", &base);
        assert_eq!(t, "<!-- kladd: header: \"Hej\" -->\n\ntext\n");
        assert_eq!((old, new), (0, "<!-- kladd: header: \"Hej\" -->".len() + 2));
        // Ersätter en befintlig rad och behåller typsnittet.
        let t0 = "<!-- kladd: font-size: 12pt -->\n\ntext\n";
        let d = DocDefaults::from_comment("<!-- kladd: font-size: 12pt -->").unwrap();
        let d2 = DocDefaults { header: Some("Hej".into()), footer: Some("{sida}".into()), footer_align: HAlign::Center, ..d };
        let (t1, ..) = set_defaults_line(t0, &d2);
        assert_eq!(t1, "<!-- kladd: font-size: 12pt; header: \"Hej\"; footer: \"{sida}\"; footer-align: center -->\n\ntext\n");
        // Blir den tom försvinner raden och den tomma raden efter.
        let (t2, ..) = set_defaults_line(t0, &DocDefaults::default());
        assert_eq!(t2, "text\n");
        // Ingen rad och inget att skriva: oförändrat.
        assert_eq!(set_defaults_line("text\n", &DocDefaults::default()).0, "text\n");
    }

    #[test]
    fn dialogen_ger_tillbaka_samma_format() {
        let d = DocDefaults::from_comment(
            "<!-- kladd: page-size: A5 landscape; page-margin: 20mm; header: \"Rapport\"; header-align: right -->",
        )
        .unwrap();
        let dlg = PageDialog::from_defaults(&d);
        assert_eq!(dlg.paper, 1);
        assert!(dlg.landscape);
        let back = dlg.to_defaults();
        assert_eq!(back.to_comment(), d.to_comment());
        let a4 = PageDialog::from_defaults(&DocDefaults::default());
        assert_eq!(a4.paper, 0);
        assert!(a4.to_defaults().is_empty());
    }
}
