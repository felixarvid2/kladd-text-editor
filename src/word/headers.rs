//! Sidhuvud och sidfot i dokumentläget: ritning på arken (vänster, mitten och höger med fälten
//! ifyllda) och dialogen "Sidhuvud och sidfot" som öppnas med dubbelklick i huvud- eller
//! fotområdet.
//!
//! Själva formatet (`header: "Rapport|{titel}|{sida}"`, `header-first`) och fältens utfyllnad
//! finns i `style`; här finns bara gränssnittet. Dialogen skriver tillbaka till grundformatsraden
//! på samma sätt som "Sidinställningar".

use egui::{
    Align, Color32, FontFamily, FontId, Frame, Id, Layout, Margin, Painter, Rect, RichText, Sense, Stroke, Ui, pos2, vec2,
};

use super::markup::{self, DocDefaults};
use super::pages::{Geom, set_defaults_line};
use crate::app::{KladdApp, quiet_button};
use crate::i18n::t;
use crate::style::{self, Fields};
use crate::tf;
use crate::theme::{self, Palette};

/// Bilder i sidhuvud och sidfot: källa → (adress för ritning, egen storlek i px).
pub type HfImages = std::collections::HashMap<String, (String, egui::Vec2)>;

/// Ritar sidhuvud och sidfot på ett ark och ger `Some(true)` vid dubbelklick på sidfoten och
/// `Some(false)` på sidhuvudet. `hf` är det som står på arket (se `crate::hf::Plan`), `z` zoomen.
/// Texten ställs upp av `hf::layout_zone`: delar, bilder, avstånd från sidkanten och linjen mot
/// brödtexten (`header-style`).
#[allow(clippy::too_many_arguments)]
pub fn header_footer(
    ui: &mut Ui,
    p: &Palette,
    geom: &Geom,
    sheet: Rect,
    k: usize,
    hf: &crate::hf::PageHf<'_>,
    d: &DocDefaults,
    font: FontId,
    family_of: &dyn Fn(bool, bool) -> FontFamily,
    doc_id: u64,
    z: f32,
    imgs: &HfImages,
) -> Option<bool> {
    use crate::hf::{self, Placed};
    let mut open = None;
    let painter: Painter = ui.painter().clone();
    let scale = crate::word::layout::px(z, 1.0);
    let text_w = (sheet.width() - geom.left - geom.right).max(1.0);
    let zones = [
        (false, Rect::from_min_max(sheet.min, pos2(sheet.right(), sheet.top() + geom.top)), hf.header, hf.header_align, &d.header_style, d.header_dist),
        (true, Rect::from_min_max(pos2(sheet.left(), sheet.bottom() - geom.bottom), sheet.max), hf.footer, hf.footer_align, &d.footer_style, d.footer_dist),
    ];
    for (footer, zone, text, align, st, dist) in zones {
        let resp = ui.interact(zone, ui.make_persistent_id(("kladd-hf", doc_id, k, footer)), Sense::click());
        if resp.double_clicked() {
            open = Some(footer);
        }
        resp.on_hover_text(t("Dubbelklicka för att ändra sidhuvud och sidfot"));
        let Some(tpl) = text else { continue };
        let rows = hf::filled_rows(tpl, align, &hf.fields);
        let size_pt = st.size.unwrap_or(font.size / scale);
        let color = st.color.map_or(p.page_muted, |c| Color32::from_rgb(c[0], c[1], c[2]));
        // Fet, kursiv och understruken stil (`header-style`) ritas som i PDF:en och i den skrivna filen.
        let family = family_of(st.bold, st.italic);
        let measure = |s: &str, size: f32| painter.layout_no_wrap(s.to_owned(), FontId::new(size, family.clone()), color).size().x;
        let natural = |src: &str| imgs.get(src).map(|(_, s)| (s.x * 0.75, s.y * 0.75));
        let zn = hf::layout_rows(&rows, size_pt, scale, text_w, &measure, &natural);
        let edge = dist.unwrap_or(hf::DEFAULT_DIST) * scale;
        let top = if footer { sheet.bottom() - edge - zn.height } else { sheet.top() + edge };
        let left = sheet.left() + geom.left;
        for it in &zn.items {
            match it {
                Placed::Text { x, y, text, size } => {
                    let mut job = egui::text::LayoutJob::simple_singleline(text.clone(), FontId::new(*size, family.clone()), color);
                    if st.underline {
                        job.sections[0].format.underline = Stroke::new((scale).max(0.6), color);
                    }
                    let galley = painter.layout_job(job);
                    painter.galley(pos2(left + x, top + y), galley, color);
                }
                Placed::Image { x, y, w, h, src, anchored } => {
                    let Some((uri, _)) = imgs.get(src) else { continue };
                    let origin = if *anchored { sheet.min } else { pos2(left, top) };
                    let rect = Rect::from_min_size(pos2(origin.x + x, origin.y + y), vec2(*w, *h));
                    match super::images::image_shape(ui.ctx(), uri, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))) {
                        Some(sh) => {
                            painter.add(sh);
                        }
                        None => ui.ctx().request_repaint(),
                    }
                }
            }
        }
        if let Some((c, w)) = st.line {
            let gap = 3.0 * scale;
            let y = if footer { top - gap } else { top + zn.height + gap };
            let stroke = Stroke::new((w * scale).max(0.5), Color32::from_rgb(c[0], c[1], c[2]));
            painter.hline(left..=left + text_w, y, stroke);
        }
    }
    open
}

/// Delarna (vänster, mitten, höger) i en rad.
type Row = [String; 3];

/// Bilder som dialogen känner till: dokumentets egna och de som valts medan den är öppen.
const LOGO_EXTENSIONS: &[&str] = crate::imgfmt::EXTENSIONS;

/// Standardhöjd (pt) på en nyinsatt logga.
const LOGO_HEIGHT: f32 = 30.0;

/// Dialogens tillstånd medan den är öppen.
pub struct HfDialog {
    /// Visar sidfoten (annars sidhuvudet).
    footer: bool,
    /// Visar första sidans variant.
    first: bool,
    different_first: bool,
    /// `[sidhuvud, sidfot][vanliga sidor, första sidan]`: raderna (styckena), var och en med tre delar.
    rows: [[Vec<Row>; 2]; 2],
    /// Loggorna (märkena `{bild:namn 80x30}`) i samma ordning; i fälten står `[Logga n]` för den n:te.
    logos: [[Vec<String>; 2]; 2],
    /// Raden och fältet (0 vänster, 1 mitten, 2 höger) som senast hade markören; dit infogas fälten.
    focus: (usize, usize),
    /// Dokumentets övriga fält som ska bevaras.
    base: DocDefaults,
    /// Filnamnet, för förhandsvisningen.
    name: String,
    /// Fält som ska få markören vid nästa bildruta.
    want_focus: bool,
    /// Dokumentets bilder och de nyvalda (namn, data); de nyvalda läggs i dokumentet vid OK.
    images: Vec<(String, Vec<u8>)>,
    new_images: Vec<String>,
    /// Bilder som lagts in som miniatyrer i egui.
    registered: std::collections::HashSet<String>,
    /// Pågående filväljare för en logga.
    pick: Option<std::sync::mpsc::Receiver<Option<std::path::PathBuf>>>,
}

/// Märket som står för en logga i ett fält.
fn placeholder(n: usize) -> String {
    format!("[{} {n}]", t("Logga"))
}

/// Byter bildmärkena i `text` mot `[Logga n]` och lägger märkena i `logos`.
fn hide_logos(text: &str, logos: &mut Vec<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("{bild:") {
        let Some(j) = rest[i..].find('}') else { break };
        let raw = &rest[i..i + j + 1];
        if crate::hf::ImgTok::parse(&raw[1..raw.len() - 1]).is_some() {
            out.push_str(&rest[..i]);
            logos.push(raw.to_owned());
            out.push_str(&placeholder(logos.len()));
        } else {
            out.push_str(&rest[..i + j + 1]);
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

/// Tvärtom: `[Logga n]` blir märket igen.
fn show_logos(text: &str, logos: &[String]) -> String {
    let mut out = text.to_owned();
    for (k, raw) in logos.iter().enumerate() {
        out = out.replace(&placeholder(k + 1), raw);
    }
    out
}

/// Raderna i en mall som delar (vänster, mitten, höger), med loggorna dolda.
fn rows_of(template: Option<&String>, align: markup::Align, logos: &mut Vec<String>) -> Vec<Row> {
    let t = template.map_or("", String::as_str);
    crate::hf::rows(t).into_iter().map(|r| style::parts_of(r, align).map(|p| hide_logos(&p, logos))).collect()
}

fn row_empty(r: &Row) -> bool {
    r.iter().all(String::is_empty)
}

impl HfDialog {
    /// `footer`: dubbelklicket gällde sidfoten; `first_page`: det gällde första sidan. `images` är dokumentets bilder.
    pub fn from_defaults(d: &DocDefaults, footer: bool, first_page: bool, name: &str, images: &[(String, crate::docx::Image)]) -> Self {
        let mut logos: [[Vec<String>; 2]; 2] = Default::default();
        let mut row = |h: usize, v: usize, t: Option<&String>, a| rows_of(t, a, &mut logos[h][v]);
        let rows = [
            [row(0, 0, d.header.as_ref(), d.header_align), row(0, 1, d.header_first.as_ref(), d.header_align)],
            [row(1, 0, d.footer.as_ref(), d.footer_align), row(1, 1, d.footer_first.as_ref(), d.footer_align)],
        ];
        let different_first = d.different_first();
        Self {
            footer,
            first: first_page && different_first,
            different_first,
            rows,
            logos,
            focus: (0, 1),
            base: d.clone(),
            name: name.to_owned(),
            want_focus: true,
            images: images.iter().map(|(n, i)| (n.clone(), i.bytes.clone())).collect(),
            new_images: Vec::new(),
            registered: Default::default(),
            pick: None,
        }
    }

    /// Mallen och justeringen för en variant (`h` 0 sidhuvud/1 sidfot, `v` 0 vanlig/1 första sidan).
    /// Flera rader skiljs av `\n`; tomma rader sist tas bort.
    fn template(&self, h: usize, v: usize, align: markup::Align) -> (String, markup::Align) {
        let mut rows: Vec<Row> = self.rows[h][v].iter().map(|r| r.clone().map(|p| show_logos(&p, &self.logos[h][v]))).collect();
        while rows.last().is_some_and(row_empty) {
            rows.pop();
        }
        match rows.as_slice() {
            [] => (String::new(), align),
            [r] if v == 0 => style::join_parts(r),
            [r] => (join_with_align(r, align), align),
            _ => {
                // Den första radens justering gäller ett enkelt värde i de andra.
                let a = if v == 0 { style::join_parts(&rows[0]).1 } else { align };
                (rows.iter().map(|r| join_with_align(r, a)).collect::<Vec<_>>().join("\n"), a)
            }
        }
    }

    /// Dokumentets grundformat med dialogens val.
    pub fn to_defaults(&self) -> DocDefaults {
        let mut d = self.base.clone();
        let (h, h_align) = self.template(0, 0, d.header_align);
        let (f, f_align) = self.template(1, 0, d.footer_align);
        d.header = Some(h.clone()).filter(|t| !t.is_empty());
        d.footer = Some(f.clone()).filter(|t| !t.is_empty());
        if !h.is_empty() {
            d.header_align = h_align;
        }
        if !f.is_empty() {
            d.footer_align = f_align;
        }
        if self.different_first {
            d.header_first = Some(self.template(0, 1, d.header_align).0);
            d.footer_first = Some(self.template(1, 1, d.footer_align).0);
        } else {
            d.header_first = None;
            d.footer_first = None;
        }
        d
    }

    fn slot(&self) -> (usize, usize) {
        (self.footer as usize, (self.first && self.different_first) as usize)
    }

    fn rows_mut(&mut self) -> &mut Vec<Row> {
        let (h, v) = self.slot();
        &mut self.rows[h][v]
    }

    /// Lägger text i fältet som senast hade markören.
    fn insert(&mut self, text: &str) {
        let (r, z) = self.focus;
        let rows = self.rows_mut();
        if rows.is_empty() {
            rows.push(Default::default());
        }
        let r = r.min(rows.len() - 1);
        rows[r][z].push_str(text);
        self.focus = (r, z);
        self.want_focus = true;
    }

    /// Antalet loggor som syns i fälten just nu, och deras index i `logos`.
    fn visible_logos(&self) -> Vec<usize> {
        let (h, v) = self.slot();
        let all: String = self.rows[h][v].iter().flatten().cloned().collect::<Vec<_>>().join("\u{1}");
        (0..self.logos[h][v].len()).filter(|k| all.contains(&placeholder(k + 1))).collect()
    }

    fn remove_logos(&mut self) {
        let (h, v) = self.slot();
        for r in &mut self.rows[h][v] {
            for p in r.iter_mut() {
                for k in 1..=self.logos[h][v].len() {
                    *p = p.replace(&placeholder(k), "");
                }
                *p = p.trim().to_owned();
            }
        }
        self.logos[h][v].clear();
    }

    /// Byter bredden (pt) på logga `k`; höjden följer proportionerna.
    fn resize_logo(&mut self, k: usize, w: f32) {
        let (h, v) = self.slot();
        let Some(tok) = self.logos[h][v].get(k).and_then(|raw| crate::hf::ImgTok::parse(&raw[1..raw.len() - 1])) else { return };
        let ratio = if tok.w > 0.0 && tok.h > 0.0 { tok.h / tok.w } else { self.aspect(&tok.src).unwrap_or(1.0) };
        let new = crate::hf::ImgTok { w, h: (w * ratio * 10.0).round() / 10.0, ..tok };
        self.logos[h][v][k] = new.to_token();
    }

    /// Bildens höjd i förhållande till bredden.
    fn aspect(&self, src: &str) -> Option<f32> {
        let name = src.strip_prefix(crate::docx::IMAGE_SCHEME)?.replace("%20", " ");
        let (_, bytes) = self.images.iter().find(|(n, _)| *n == name)?;
        let s = imagesize::blob_size(bytes).ok()?;
        (s.width > 0).then(|| s.height as f32 / s.width as f32)
    }

    /// En vald bildfil som logga: läggs i dialogens bilder och infogas i fältet med markören.
    fn add_logo(&mut self, path: &std::path::Path) -> Result<(), ()> {
        let bytes = std::fs::read(path).map_err(|_| ())?;
        let bytes = crate::imgfmt::to_supported(bytes).ok_or(())?;
        crate::docx::image_ext(&bytes).ok_or(())?;
        let base: String = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "logga.png".into())
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
            .collect();
        let mut name = base.clone();
        let mut n = 2;
        loop {
            match self.images.iter().find(|(m, _)| *m == name) {
                Some((_, b)) if *b == bytes => break,
                Some(_) => {
                    name = format!("{n}-{base}");
                    n += 1;
                }
                None => {
                    self.images.push((name.clone(), bytes.clone()));
                    self.new_images.push(name.clone());
                    break;
                }
            }
        }
        let s = imagesize::blob_size(&bytes).map_err(|_| ())?;
        let ratio = if s.width > 0 { s.height as f32 / s.width as f32 } else { 1.0 };
        let tok = crate::hf::ImgTok { src: format!("{}{name}", crate::docx::IMAGE_SCHEME), w: (LOGO_HEIGHT / ratio.max(0.05) * 10.0).round() / 10.0, h: LOGO_HEIGHT, at: None };
        let (h, v) = self.slot();
        self.logos[h][v].push(tok.to_token());
        let ph = placeholder(self.logos[h][v].len());
        self.insert(&ph);
        Ok(())
    }

    /// Svaret från filväljaren, när det kommit.
    fn poll_pick(&mut self, ctx: &egui::Context) -> Option<std::path::PathBuf> {
        let rx = self.pick.as_ref()?;
        match rx.try_recv() {
            Ok(path) => {
                self.pick = None;
                path
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
                None
            }
            Err(_) => {
                self.pick = None;
                None
            }
        }
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

/// Talformaten i valet "Sidnummerformat".
const NUM_FORMATS: [(crate::hf::NumFmt, &str); 5] = [
    (crate::hf::NumFmt::Decimal, "1, 2, 3"),
    (crate::hf::NumFmt::LowerRoman, "i, ii, iii"),
    (crate::hf::NumFmt::UpperRoman, "I, II, III"),
    (crate::hf::NumFmt::LowerAlpha, "a, b, c"),
    (crate::hf::NumFmt::UpperAlpha, "A, B, C"),
];

impl KladdApp {
    /// Öppnar "Sidhuvud och sidfot" för det aktiva dokumentet. `page` är sidan (0-baserad) som
    /// dubbelklickades.
    pub(crate) fn open_header_dialog(&mut self, footer: bool, page: usize) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let a = self.word.analysis(doc.id, &doc.text);
        let name = doc.title();
        self.wv.hf_dlg = Some(HfDialog::from_defaults(&a.defaults, footer, page == 0, &name, &doc.images));
    }

    /// Öppnar filväljaren för en logga (i en egen tråd).
    fn pick_logo(&mut self, ctx: &egui::Context) {
        let Some(d) = self.wv.hf_dlg.as_mut() else { return };
        if d.pick.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        d.pick = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let dlg = rfd::FileDialog::new().set_title(t("Välj logga")).add_filter(t("Bilder"), LOGO_EXTENSIONS);
            let _ = tx.send(dlg.pick_file());
            ctx.request_repaint();
        });
    }

    pub(crate) fn header_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let picked = self.wv.hf_dlg.as_mut().and_then(|d| d.poll_pick(ctx));
        if let Some(path) = picked {
            let ok = self.wv.hf_dlg.as_mut().is_some_and(|d| d.add_logo(&path).is_ok());
            if !ok {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify(ctx, tf!("{name} är inte en bild som kan bäddas in (PNG, JPEG, GIF eller BMP).", name = name), true);
            }
        }
        let mut pick_wanted = false;
        let Some(d) = self.wv.hf_dlg.as_mut() else { return };
        // Miniatyrerna: bilderna läggs in i egui en gång.
        for (name, bytes) in &d.images {
            if d.registered.insert(name.clone()) {
                ctx.include_bytes(thumb_uri(name), bytes.clone());
            }
        }
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
                ui.set_width(520.0);
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
                let mut remove_row = None;
                if d.rows_mut().is_empty() {
                    d.rows_mut().push(Default::default());
                }
                let n_rows = d.rows_mut().len();
                for ri in 0..n_rows {
                    egui::Grid::new(("kladd-hf-grid", ri)).num_columns(2).spacing(vec2(14.0, 8.0)).show(ui, |ui| {
                        for (i, label) in [t("Vänster"), t("Mitten"), t("Höger")].into_iter().enumerate() {
                            ui.label(RichText::new(label).color(p.muted));
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut d.rows_mut()[ri][i]).desired_width(360.0).margin(Margin::symmetric(8, 5)),
                            );
                            if want_focus && (ri, i) == focus {
                                r.request_focus();
                            }
                            if r.has_focus() {
                                new_focus = Some((ri, i));
                            }
                            ui.end_row();
                        }
                    });
                    if n_rows > 1 {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tf!("Rad {n}", n = ri + 1)).color(p.muted).size(12.0));
                            if ui.add(quiet_button(t("Ta bort rad"), p)).clicked() {
                                remove_row = Some(ri);
                            }
                        });
                    }
                    ui.add_space(4.0);
                }
                if let Some(f) = new_focus {
                    d.focus = f;
                }
                if let Some(ri) = remove_row {
                    d.rows_mut().remove(ri);
                    d.focus = (0, 1);
                }
                if ui.add(quiet_button(t("Lägg till rad"), p)).clicked() {
                    d.rows_mut().push(Default::default());
                    d.focus = (d.rows_mut().len() - 1, 1);
                    d.want_focus = true;
                }
                ui.add_space(8.0);
                ui.label(RichText::new(t("Infoga i det markerade fältet")).color(p.muted).size(12.5));
                ui.horizontal_wrapped(|ui| {
                    if ui.add(quiet_button(t("Infoga sidnummer"), p)).clicked() {
                        d.insert("{sida}");
                    }
                    if ui.add(quiet_button(t("Antal sidor"), p)).clicked() {
                        d.insert("{sidor}");
                    }
                    for (code, label) in style::FIELD_NAMES.iter().filter(|(c, _)| !matches!(*c, "{sida}" | "{sidor}")) {
                        if ui.add(quiet_button(t(label), p)).clicked() {
                            d.insert(code);
                        }
                    }
                    if ui.add_enabled(d.pick.is_none(), quiet_button(t("Logga…"), p)).clicked() {
                        pick_wanted = true;
                    }
                    let has_logo = !d.visible_logos().is_empty();
                    if ui.add_enabled(has_logo, quiet_button(t("Ta bort logga"), p)).clicked() {
                        d.remove_logos();
                    }
                });
                // Loggorna som miniatyr med namn och bredd.
                let (lh, lv) = d.slot();
                let mut resize: Option<(usize, f32)> = None;
                for k in d.visible_logos() {
                    let Some(tok) = d.logos[lh][lv].get(k).and_then(|raw| crate::hf::ImgTok::parse(&raw[1..raw.len() - 1])) else { continue };
                    let name = tok.src.strip_prefix(crate::docx::IMAGE_SCHEME).unwrap_or(&tok.src).replace("%20", " ");
                    ui.horizontal(|ui| {
                        Frame::NONE.fill(p.surface_hover).corner_radius(6).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add(egui::Image::new(thumb_uri(&name)).fit_to_exact_size(vec2(36.0, 24.0)));
                                ui.label(RichText::new(format!("{} – {name}", placeholder(k + 1))).color(p.text).size(12.5));
                                let mut w = if tok.w > 0.0 { tok.w } else { LOGO_HEIGHT };
                                ui.label(RichText::new(t("Bredd")).color(p.muted).size(12.5));
                                if ui.add(egui::DragValue::new(&mut w).range(8.0..=400.0).suffix(" pt")).changed() {
                                    resize = Some((k, w));
                                }
                            });
                        });
                    });
                }
                if let Some((k, w)) = resize {
                    d.resize_logo(k, w);
                }
                ui.add_space(8.0);
                if ui.checkbox(&mut d.different_first, t("Annorlunda första sida")).changed() && !d.different_first {
                    d.first = false;
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("Sidnummerformat")).color(p.muted));
                    let cur = d.base.page_format.unwrap_or_default();
                    let label = NUM_FORMATS.iter().find(|(f, _)| *f == cur).map_or("1, 2, 3", |(_, l)| *l);
                    egui::ComboBox::from_id_salt("kladd-hf-numfmt").selected_text(label).show_ui(ui, |ui| {
                        for (f, l) in NUM_FORMATS {
                            if ui.selectable_label(f == cur, l).clicked() {
                                d.base.page_format = Some(f).filter(|f| *f != crate::hf::NumFmt::Decimal);
                            }
                        }
                    });
                });
                // Förhandsvisning av raderna som de ser ut på sida 1 av 3.
                let mut f = Fields::new(1, 3, &d.name);
                f.fmt = d.base.page_format.unwrap_or_default();
                ui.add_space(6.0);
                let rows = d.rows_mut().clone();
                for [l, c, r] in rows.into_iter().map(|r| r.map(|t| style::fill_fields(&t, &f))) {
                    ui.label(RichText::new(tf!("Förhandsvisning:  {l}   |   {c}   |   {r}", l = l, c = c, r = r)).color(p.muted).size(12.5));
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
        if pick_wanted {
            self.pick_logo(ctx);
        }
        let Some(act) = act else { return };
        let Some(d) = self.wv.hf_dlg.take() else { return };
        if matches!(act, Act::Cancel) {
            return;
        }
        let new = d.to_defaults();
        // Nyvalda loggor som används läggs i dokumentet.
        if let Some(doc) = self.docs.get_mut(self.active) {
            let used = [new.header.as_deref(), new.footer.as_deref(), new.header_first.as_deref(), new.footer_first.as_deref(), new.header_even.as_deref(), new.footer_even.as_deref()];
            for name in &d.new_images {
                let src = format!("{}{}", crate::docx::IMAGE_SCHEME, name);
                if !used.iter().flatten().any(|t| t.contains(&format!("{{bild:{name} "))) {
                    continue;
                }
                if let Some((_, bytes)) = d.images.iter().find(|(n, _)| n == name) {
                    if !doc.images.iter().any(|(n, _)| n == name) {
                        doc.images.push((name.clone(), crate::docx::Image { bytes: bytes.clone(), extent: None }));
                    }
                }
                self.wv.images.remove(&(doc.id, src));
            }
        }
        self.word_edit(ctx, |t, sel| {
            let (text, old_len, new_len) = set_defaults_line(t, &new);
            let map = |b: usize| if b >= old_len { b - old_len + new_len } else { b.min(new_len) };
            markup::Edit { text, sel: map(sel.start)..map(sel.end) }
        });
    }
}

fn thumb_uri(name: &str) -> String {
    format!("bytes://kladd-hf-dlg/{}", name.replace(' ', "%20"))
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
        let mut dlg = HfDialog::from_defaults(&d, true, false, "rapport.docx", &[]);
        assert_eq!(dlg.rows[0][0][0], ["Rapport", "{titel}", "{datum}"].map(String::from));
        assert_eq!(dlg.rows[1][0][0], ["", "{sida}", ""].map(String::from));
        // Oförändrad dialog ger samma inställningar tillbaka.
        assert_eq!(dlg.to_defaults(), d);
        // Mittendelen i sidfoten flyttas till höger: det gamla enkla värdet med högerjustering.
        dlg.rows[1][0] = vec![["".into(), "".into(), "{sida}".into()]];
        let out = dlg.to_defaults();
        assert_eq!((out.footer.as_deref(), out.footer_align), (Some("{sida}"), HAlign::Right));
        // Tömd rad tar bort sidhuvudet.
        dlg.rows[0][0] = Vec::new();
        assert_eq!(dlg.to_defaults().header, None);
    }

    #[test]
    fn dialog_first_page_variant() {
        let d = DocDefaults { footer: Some("{sida}".into()), footer_align: HAlign::Center, ..Default::default() };
        let mut dlg = HfDialog::from_defaults(&d, true, true, "a.md", &[]);
        assert!(!dlg.different_first && !dlg.first);
        dlg.different_first = true;
        dlg.first = true;
        // Tom första sida: båda varianterna blir tomma strängar (inte `None`).
        let out = dlg.to_defaults();
        assert_eq!((out.header_first.as_deref(), out.footer_first.as_deref()), (Some(""), Some("")));
        assert_eq!(out.footer_template(0), None);
        assert_eq!(out.footer_template(1), Some("{sida}"));
        // Första sidans sidfot får egen text med annan justering än de övriga.
        dlg.rows_mut()[0][2] = "Utkast".into();
        let out = dlg.to_defaults();
        assert_eq!(out.footer_first.as_deref(), Some("||Utkast"));
        assert_eq!(style::parts_of(out.footer_template(0).unwrap(), out.footer_align)[2], "Utkast");
        // Avbockad ruta tar bort varianterna.
        dlg.different_first = false;
        let out = dlg.to_defaults();
        assert!(!out.different_first());
    }

    const PNG: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0xd, 0xa, 0x1a, 0xa, 0x0, 0x0, 0x0, 0xd, 0x49, 0x48, 0x44, 0x52, 0x0, 0x0, 0x0, 0x1, 0x0, 0x0, 0x0, 0x1, 0x8, 0x2, 0x0, 0x0, 0x0, 0x90, 0x77, 0x53, 0xde, 0x0,
        0x0, 0x0, 0xc, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x0, 0x0, 0x3, 0x1, 0x1, 0x0, 0xc9, 0xfe, 0x92, 0xef, 0x0, 0x0, 0x0, 0x0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
        0x60, 0x82,
    ];

    #[test]
    fn dialog_keeps_several_lines() {
        let d = DocDefaults { header: Some("Skolan\n||Utkast\nA|B|C".into()), header_align: HAlign::Center, ..Default::default() };
        let mut dlg = HfDialog::from_defaults(&d, false, false, "a.docx", &[]);
        assert_eq!(dlg.rows[0][0].len(), 3);
        assert_eq!(dlg.rows[0][0][0], ["", "Skolan", ""].map(String::from));
        assert_eq!(dlg.rows[0][0][1], ["", "", "Utkast"].map(String::from));
        // Oförändrad dialog skriver tillbaka samma mall.
        assert_eq!(dlg.to_defaults(), d);
        // En ny rad och en borttagen rad; tomma rader sist försvinner.
        dlg.rows_mut().push(Default::default());
        assert_eq!(dlg.to_defaults(), d);
        dlg.rows_mut().remove(2);
        assert_eq!(dlg.to_defaults().header.as_deref(), Some("Skolan\n||Utkast"));
        // En enda kvarvarande rad blir ett vanligt värde.
        dlg.rows_mut().remove(1);
        let out = dlg.to_defaults();
        assert_eq!((out.header.as_deref(), out.header_align), (Some("Skolan"), HAlign::Center));
    }

    #[test]
    fn logos_show_as_markers_and_can_be_added_resized_and_removed() {
        let d = DocDefaults { header: Some("{bild:logo.png 80x30}|Titel|{sida}".into()), ..Default::default() };
        let img = crate::docx::Image { bytes: PNG.to_vec(), extent: None };
        let mut dlg = HfDialog::from_defaults(&d, false, false, "a.docx", &[("logo.png".into(), img)]);
        // Fältet visar en bricka i stället för bildmärket; oförändrat ger samma mall tillbaka.
        assert_eq!(dlg.rows[0][0][0][0], placeholder(1));
        assert_eq!(dlg.visible_logos(), [0]);
        assert_eq!(dlg.to_defaults(), d);
        // Bredden ändras och höjden följer proportionerna.
        dlg.resize_logo(0, 60.0);
        assert_eq!(dlg.to_defaults().header.as_deref(), Some("{bild:logo.png 60x22.5}|Titel|{sida}"));
        // En ny logga från en fil läggs i dokumentets bilder (en 1x1-bild blir 30 pt hög och 30 pt bred).
        let path = std::env::temp_dir().join(format!("kladd-hf-test-{}.png", std::process::id()));
        std::fs::write(&path, PNG).unwrap();
        let ok = dlg.add_logo(&path);
        let _ = std::fs::remove_file(&path);
        assert!(ok.is_ok());
        assert_eq!(dlg.new_images.len(), 1, "{:?}", dlg.new_images);
        let h = dlg.to_defaults().header.unwrap();
        assert!(h.starts_with("{bild:logo.png 60x22.5}|Titel{bild:kladd-hf-test-"), "{h}");
        assert_eq!(dlg.visible_logos().len(), 2);
        // Ta bort loggor tömmer märkena och texten finns kvar.
        dlg.remove_logos();
        assert!(dlg.visible_logos().is_empty());
        assert_eq!(dlg.to_defaults().header.as_deref(), Some("|Titel|{sida}"));
        // En fil som inte är en bild läggs inte till.
        let bad = std::env::temp_dir().join(format!("kladd-hf-bad-{}.png", std::process::id()));
        std::fs::write(&bad, b"inte en bild").unwrap();
        assert!(dlg.add_logo(&bad).is_err());
        let _ = std::fs::remove_file(&bad);
        // Sidnummer och antal sidor infogas i fältet med markören.
        dlg.focus = (0, 2);
        dlg.insert("{sidor}");
        assert_eq!(dlg.to_defaults().header.as_deref(), Some("|Titel|{sida}{sidor}"));
    }
}
