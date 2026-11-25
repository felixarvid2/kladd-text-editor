//! Läsläget för Markdown: en öppnad `.md`-fil visas färdigritad i hela fönstret, med en
//! Redigera-knapp uppe till höger. Redigera (eller Ctrl+Skift+M) byter till textredigeraren;
//! samma kortkommando eller knappen i namnlisten går tillbaka till läsläget. Ritningen delas
//! med förhandsvisningen bredvid texten (`Settings::side_preview`).
//!
//! Läsläget har egen sökning (träffarna markeras i den ritade texten), rullar till rubrikerna i
//! navigeringsfönstret, låter kryssrutor bockas av, öppnar redigeraren vid texten man
//! dubbelklickar och håller rullningsläget ungefär kvar när man växlar. Texten som ritas är inte
//! exakt dokumentets (formler, HTML och bilder görs om först), så källpositioner översätts
//! tillbaka med [`to_doc_offset`].

use std::collections::HashSet;
use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Align2, CentralPanel, FontFamily, FontId, Frame, Id, Rect, RichText, ScrollArea, Stroke, Ui, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer, SearchOptions};

use crate::app::{KladdApp, byte_to_char, editor_id};
use crate::doc::Document;
use crate::i18n::t;
use crate::theme::{self, Palette};
use crate::word::nav;

/// Samma nyckel som `egui_commonmark_backend::misc::BOLD_FAMILY_ID` i den lappade kopian i `vendor/`.
const BOLD_FAMILY_ID: &str = "egui_commonmark_bold_family";

/// Bredden på textspalten i läsläget, som i en vanlig läsvy.
const COLUMN_W: f32 = 760.0;

/// Ska fliken öppnas i läsläget? Vanliga Markdown-filer från disk; inte Word, ODT eller PDF
/// (de har dokumentläget och PDF-visaren), inte namnlösa flikar och inte stora filer.
pub fn opens_in_reading(doc: &Document) -> bool {
    doc.path.is_some()
        && !doc.docx
        && !doc.pdf
        && doc.pdfv.is_none()
        && !doc.word_view
        && matches!(doc.extension().as_str(), "md" | "markdown" | "mdown" | "mkd")
        && !crate::bigtext::is_big(&doc.text)
}

/// Förenklade tecken för rubrikernas id i den ritade texten (`# Rubrik {#kladd-h3}`).
const HEADING_ID: &str = "kladd-h";

/// Sökningen i läsläget: samma fält som sökfältet.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct Search {
    pub query: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
}

impl Search {
    fn flags(&self) -> SearchOptions {
        let mut f = SearchOptions::empty();
        f.set(SearchOptions::CASE_SENSITIVE, self.case);
        f.set(SearchOptions::WHOLE_WORD, self.word);
        f.set(SearchOptions::REGEX, self.regex);
        f
    }
}

/// Läsvyns tillstånd mellan bildrutor.
#[derive(Default)]
pub struct MdState {
    /// Sökningen som senast lades in i ritaren (vy, sökning, textversion och längd).
    applied: Option<(Id, Search, u64, usize)>,
    /// Antal träffar, aktuell träff (0-baserad) och felet i ett ogiltigt uttryck, för sökfältet.
    pub count: usize,
    pub current: Option<usize>,
    pub error: Option<String>,
    /// Väntande F3-steg (+1 nästa, -1 föregående).
    step: isize,
    /// Dokumentbyte som läsvyn ska rulla till (en rubrik i navigeringsfönstret).
    pub scroll_to: Option<usize>,
    /// Väntande rullningsandel (0..1) från redigeraren.
    frac_in: Option<f32>,
    /// Dokumentbyte högst upp i läsvyn, och rullningsandelen och största rullningen.
    pub top_byte: usize,
    read_frac: f32,
    read_max: f32,
    /// Redigerarens rullningsandel i senaste bildrutan.
    pub editor_frac: f32,
}

impl MdState {
    /// Redigerarens rullning, för att läsvyn ska kunna hamna ungefär på samma ställe.
    pub fn note_editor_scroll(&mut self, offset_y: f32, content_h: f32, viewport_h: f32) {
        let max = content_h - viewport_h;
        self.editor_frac = if max > 1.0 { (offset_y / max).clamp(0.0, 1.0) } else { 0.0 };
    }
}

/// Hur en ritning ska bete sig.
pub struct Opts {
    /// Vyns id i ritarens cache (sök och rullning hör till det).
    pub id: Id,
    pub search: Option<Search>,
    /// Kryssrutor går att bocka av.
    pub boxes: bool,
    /// Läsvyn: håll reda på översta raden, rulla till rubriker och ta dubbelklick.
    pub reading: bool,
}

/// Vad användaren gjorde i ritningen.
#[derive(Default)]
pub struct Out {
    /// Kryssrutan (i ordning bland dokumentets kryssrutor) som klickades.
    pub toggled: Option<usize>,
    /// Dokumentbyte vid ett dubbelklick.
    pub dbl: Option<usize>,
}

/// Ritar dokumentets Markdown som i förhandsvisningen: bilder, formler och Kladds HTML-format.
pub fn render(
    ui: &mut Ui,
    p: &Palette,
    doc: &Document,
    images: &mut HashSet<String>,
    cache: &mut CommonMarkCache,
    st: &mut MdState,
    o: &Opts,
) -> Out {
    let st_ui = ui.style_mut();
    st_ui.text_styles.insert(egui::TextStyle::Body, FontId::new(15.0, FontFamily::Proportional));
    st_ui.text_styles.insert(egui::TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace));
    st_ui.text_styles.insert(egui::TextStyle::Heading, FontId::new(26.0, theme::semibold()));
    st_ui.spacing.item_spacing.y = 8.0;
    // Brödtexten får textfärgen via det vanliga ytans färg (inte `override_text_color`, som också
    // skulle färga länkarna i textfärgen). Fet text (`RichText::strong`) får samma färg; fetstilen
    // kommer från typsnittet nedan.
    st_ui.visuals.widgets.noninteractive.fg_stroke.color = p.text;
    st_ui.visuals.widgets.active.fg_stroke.color = p.text;
    st_ui.visuals.hyperlink_color = link_color(p);
    // Den lappade egui_commonmark_backend (vendor/) ritar fet text med den här familjen.
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique(BOLD_FAMILY_ID), theme::semibold()));
    // Bilder: inbäddade (bild:) registreras som minnesdata, övriga läses från disk.
    // HTML-formatet (<u>, <span>, <div>, <img> …) görs först om till vanlig Markdown.
    let with_math = crate::math::preview::replace(&doc.text, ui.ctx(), 15.0, p.text, ui.available_width());
    // ```mermaid-block som diagram (bilder i minnet); block som inte går att rita står kvar som kod.
    let with_math = crate::mermaid::replace(&with_math, ui.ctx(), p.dark);
    let mut text = crate::preview::clean(&with_math);
    for (name, img) in &doc.images {
        let enc = name.replace(' ', "%20");
        let uri = format!("bytes://kladd/{}/{enc}", doc.id);
        if images.insert(uri.clone()) {
            ui.ctx().include_bytes(uri.clone(), img.bytes.clone());
        }
        let from = format!("]({}{enc})", crate::docx::IMAGE_SCHEME);
        if text.contains(&from) {
            text = text.replace(&from, &format!("]({uri})"));
        }
    }
    let home = crate::platform::home_dir();
    if let Some(home) = &home {
        if text.contains("](~/") {
            text = text.replace("](~/", &format!("](file://{}/", home.display()));
        }
    }
    // Rubrikerna får id (för rullning från navigeringsfönstret).
    if o.reading {
        text = with_heading_ids(&text);
    }

    // Sökningen: görs om när något ändrats, och F3 hoppar mellan träffarna.
    let want = o.search.clone().unwrap_or_default();
    let key = (o.id, want.clone(), doc.rev, text.len());
    if st.applied.as_ref() != Some(&key) {
        *cache.search_query_mut(&o.id) = want.query.clone();
        *cache.search_options_mut(&o.id) = want.flags();
        cache.update_search_matches(&o.id, &text);
        st.applied = Some(key);
    }
    if st.step != 0 && o.search.is_some() {
        cache.go_to_match(&o.id, std::mem::take(&mut st.step));
    }
    if o.reading {
        if let Some(byte) = st.scroll_to.take() {
            if let Some(slug) = heading_slug(&doc.text, byte) {
                cache.scroll_to_heading(&o.id, Some(slug));
            }
        }
    }

    let base = doc.base_dir().or(home).unwrap_or_default();
    let max_w = ui.available_width() as usize;
    let viewer = CommonMarkViewer::new()
        .default_implicit_uri_scheme(format!("file://{}/", base.display()))
        .max_image_width(Some(max_w))
        .enable_scroll_to_heading(true)
        .search_match_color(p.match_bg)
        .search_active_match_color(p.match_current_bg);
    let before = o.boxes.then(|| text.clone());
    let resp = if o.boxes {
        viewer.show_with_id_mut(o.id, ui, cache, &mut text)
    } else {
        viewer.show_with_id(o.id, ui, cache, &text)
    };
    let rect = resp.response.rect;
    let mut out = Out::default();
    if let Some(before) = before {
        if resp.response.changed() {
            out.toggled = toggled_task(&before, &text);
        }
    }
    if o.search.is_some() {
        st.count = cache.search_ranges(&o.id).len();
        st.current = cache.active_match(&o.id);
        st.error = cache.search_regex_error(&o.id);
    }
    if o.reading {
        let points = cache.line_points(&o.id);
        let top = ui.clip_rect().top() - rect.top();
        st.top_byte = to_doc_offset(&text, &doc.text, line_at(points, top).unwrap_or(0));
        // Dubbelklick på ett stycke (inte på en länk eller kryssruta).
        let hand = ui.ctx().output(|o| o.cursor_icon) == egui::CursorIcon::PointingHand;
        if !hand && out.toggled.is_none() && ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()).filter(|p| ui.clip_rect().contains(*p)) {
                if let Some(at) = line_at(points, pos.y - rect.top()) {
                    out.dbl = Some(to_doc_offset(&text, &doc.text, at));
                }
            }
        }
    }
    out
}

/// Länkfärgen i läsvyn.
fn link_color(p: &Palette) -> egui::Color32 {
    p.syntax.function
}

// ------------------------------------------------------------------ rena hjälpfunktioner

/// Lägger `{#kladd-hN}` sist på varje rubrikrad (N = rubrikens ordning, som i
/// `nav::headings_scan`) så att visningen kan rulla till den. Rubriker som redan har ett id behålls.
pub fn with_heading_ids(text: &str) -> String {
    let heads = nav::headings_scan(text);
    if heads.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + heads.len() * 14);
    let mut last = 0;
    for (i, h) in heads.iter().enumerate() {
        let end = text[h.start..].find('\n').map_or(text.len(), |e| h.start + e);
        let line = text[h.start..end].trim_end();
        if line.contains("{#") {
            continue;
        }
        // Före ett avslutande `##`.
        let core = line.trim_end_matches('#');
        let at = if core.len() < line.len() && core.ends_with([' ', '\t']) && !core.trim().is_empty() {
            h.start + core.trim_end().len()
        } else {
            h.start + line.len()
        };
        out.push_str(&text[last..at]);
        out.push_str(&format!(" {{#{HEADING_ID}{i}}}"));
        last = at;
    }
    out.push_str(&text[last..]);
    out
}

/// Rubrik-id:t för rubriken som börjar vid eller före `byte`.
fn heading_slug(doc_text: &str, byte: usize) -> Option<String> {
    let heads = nav::headings_scan(doc_text);
    let i = heads.iter().rposition(|h| h.start <= byte)?;
    Some(format!("{HEADING_ID}{i}"))
}

/// Källbyten i den ritade texten för den ruta som ligger vid eller strax före höjden `y`.
pub fn line_at(points: &[(usize, f32)], y: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for &(b, py) in points {
        if py <= y + 0.5 && best.is_none_or(|(_, by)| py >= by) {
            best = Some((b, py));
        }
    }
    best.map(|b| b.0)
}

/// Översätter en byteposition i den ritade texten till dokumentets text. Den ritade texten är
/// dokumentets med formler, HTML och rubrik-id omgjorda, men oftast med samma rader: samma radnummer
/// om antalet rader stämmer, annars den lika lydande raden närmast, annars samma andel in.
pub fn to_doc_offset(rendered: &str, doc: &str, off: usize) -> usize {
    let off = off.min(rendered.len());
    let line_idx = rendered.as_bytes()[..off].iter().filter(|b| **b == b'\n').count();
    let line_start = rendered[..off].rfind('\n').map_or(0, |i| i + 1);
    let col = off - line_start;
    let doc_lines: Vec<&str> = doc.split('\n').collect();
    let rendered_lines = rendered.split('\n').count();
    let idx = if doc_lines.len() == rendered_lines {
        line_idx
    } else {
        let want = rendered.split('\n').nth(line_idx).unwrap_or("").trim();
        let same = (!want.is_empty())
            .then(|| {
                doc_lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.trim() == want)
                    .map(|(i, _)| i)
                    .min_by_key(|i| i.abs_diff(line_idx))
            })
            .flatten();
        same.unwrap_or_else(|| (line_idx * doc_lines.len() / rendered_lines.max(1)).min(doc_lines.len() - 1))
    };
    let start: usize = doc_lines[..idx].iter().map(|l| l.len() + 1).sum();
    let line = doc_lines[idx];
    if doc_lines.len() == rendered_lines {
        // Behåll kolumnen på en tecken-gräns.
        let mut c = col.min(line.len());
        while !line.is_char_boundary(c) {
            c -= 1;
        }
        start + c
    } else {
        start
    }
}

/// `[ ]`/`[x]` i listpunkter (byteområden), i samma ordning som visningen ritar kryssrutorna.
pub fn task_markers(text: &str) -> Vec<Range<usize>> {
    use pulldown_cmark::{Event, Parser};
    // Samma tillägg som visningen tolkar med.
    use pulldown_cmark::Options as O;
    let opts = O::ENABLE_TABLES | O::ENABLE_TASKLISTS | O::ENABLE_STRIKETHROUGH | O::ENABLE_FOOTNOTES | O::ENABLE_DEFINITION_LIST;
    Parser::new_ext(text, opts)
        .into_offset_iter()
        .filter_map(|(e, r)| matches!(e, Event::TaskListMarker(_)).then_some(r))
        .collect()
}

/// Vilken kryssruta som skiljer `before` från `after` (visningen byter tecknet i hakparentesen).
fn toggled_task(before: &str, after: &str) -> Option<usize> {
    let pos = before.bytes().zip(after.bytes()).position(|(a, b)| a != b)?;
    task_markers(after).iter().position(|r| r.contains(&pos))
}

/// Texten med kryssrutan `n` omvänd (`[ ]` blir `[x]` och tvärtom). Bara tecknet i hakparentesen ändras.
pub fn flip_task(text: &str, n: usize) -> Option<String> {
    let r = task_markers(text).get(n)?.clone();
    let mid = r.start + 1;
    let new = if text.as_bytes().get(mid) == Some(&b' ') { "x" } else { " " };
    let mut out = text.to_owned();
    out.replace_range(mid..mid + 1, new);
    Some(out)
}

/// Byten `n` rader efter raden som innehåller `byte`.
fn advance_lines(text: &str, byte: usize, n: usize) -> usize {
    let mut at = byte.min(text.len());
    for _ in 0..n {
        match text[at..].find('\n') {
            Some(i) => at += i + 1,
            None => return text.len(),
        }
    }
    at
}

// ------------------------------------------------------------------ appens sida

impl KladdApp {
    /// Sant när den aktiva fliken visas i läsläget.
    pub(crate) fn md_read_mode(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| {
            self.md_reading.contains(&d.id)
                && d.is_markdown()
                && !d.word_view
                && d.pdfv.is_none()
                && !self.big.has_view(d.id)
                && !self.panes.is_split()
        })
    }

    /// Byter mellan läsläget och redigeraren för den aktiva Markdown-fliken.
    pub(crate) fn toggle_md_reading(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        if !d.is_markdown() || d.word_view || d.pdfv.is_some() {
            return;
        }
        let id = d.id;
        if self.md_reading.contains(&id) {
            self.md_leave_reading();
        } else {
            self.md_reading.insert(id);
            self.md.frac_in = Some(self.md.editor_frac);
        }
    }

    /// Lämnar läsläget för redigeraren, som rullas dit läsvyn stod.
    pub(crate) fn md_leave_reading(&mut self) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if self.md_reading.remove(&doc.id) {
            // Redigeraren centrerar raden; räkna in ungefär en halv skärm så att toppen hamnar rätt.
            let at = if self.md.read_frac > 0.01 { advance_lines(&doc.text, self.md.top_byte, 10) } else { 0 };
            self.scroll_to_char = Some(byte_to_char(&doc.text, at));
        }
        self.focus_editor = true;
    }

    /// Öppnar redigeraren med markören vid `byte` (dubbelklick i läsvyn).
    fn md_open_at(&mut self, ctx: &egui::Context, byte: usize) {
        let Some(doc) = self.docs.get(self.active) else { return };
        let id = editor_id(doc.id);
        let ci = byte_to_char(&doc.text, byte);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
        state.store(ctx, id);
        self.md_reading.remove(&doc.id);
        self.scroll_to_char = Some(ci);
        self.focus_editor = true;
        self.find.current = None;
    }

    /// Bockar av eller i kryssrutan `n` i texten, som ett eget ångra-steg.
    pub(crate) fn md_toggle_task(&mut self, ctx: &egui::Context, n: usize) {
        let Some(doc) = self.docs.get_mut(self.active) else { return };
        let Some(new) = flip_task(&doc.text, n) else { return };
        let id = editor_id(doc.id);
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let end = doc.text.chars().count();
        let before = state.cursor.char_range().unwrap_or(CCursorRange::one(CCursor::new(end)));
        // Läget före ändringen som ångra-steg (editorn har kanske aldrig visats för fliken).
        let mut undoer = state.undoer();
        undoer.add_undo(&(before, doc.text.clone()));
        state.set_undoer(undoer);
        state.store(ctx, id);
        doc.text = new;
        doc.on_edited();
        ctx.request_repaint();
    }

    /// F3 och Skift+F3 i läsläget.
    pub(crate) fn md_seek(&mut self, delta: isize) {
        self.md.step = delta;
    }

    /// Ångra och gör om i läsläget (editorn, som annars tar tangenterna, visas inte).
    pub(crate) fn md_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        // Ett textfält (sökfältet) har fokus: det ångrar sin egen text.
        if ctx.memory(|m| m.focused()).is_some() {
            return;
        }
        let hit = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let ctrl = Modifiers::COMMAND;
        if hit(ctrl | Modifiers::SHIFT, Key::Z) || hit(ctrl, Key::Y) {
            self.word_undo(ctx, true);
            self.focus_editor = false;
        } else if hit(ctrl, Key::Z) {
            self.word_undo(ctx, false);
            self.focus_editor = false;
        }
    }

    /// Läsläget: hela fönstret är den färdigritade texten, med Redigera uppe till höger.
    pub(crate) fn md_read_page(&mut self, ui: &mut Ui, p: &Palette) {
        // Hopp till en rad (gå till rad, sökträffar från andra flikar) visas i redigeraren.
        if self.scroll_to_char.is_some() {
            if let Some(d) = self.docs.get(self.active) {
                self.md_reading.remove(&d.id);
            }
            self.focus_editor = true;
            ui.ctx().request_repaint();
            return;
        }
        let ctx = ui.ctx().clone();
        let search = self.find.open.then(|| Search {
            query: self.find.query.clone(),
            case: self.find.case_sensitive,
            word: self.find.whole_word,
            regex: self.find.regex,
        });
        let mut edit = false;
        let mut out = Out::default();
        CentralPanel::default().frame(Frame::NONE.fill(p.editor)).show(ui, |ui| {
            self.editor_rect = ui.max_rect();
            let full = ui.max_rect();
            let Some(doc) = self.docs.get(self.active) else { return };
            let opts = Opts { id: Id::unique(("kladd-md-read", doc.id)), search, boxes: true, reading: true };
            let mut area = ScrollArea::vertical().id_salt(("kladd-mdread", doc.id)).auto_shrink([false, false]);
            if let Some(f) = self.md.frac_in.take() {
                if self.md.read_max > 0.0 {
                    area = area.vertical_scroll_offset(f * self.md.read_max);
                }
            }
            let res = area.show(ui, |ui| {
                let avail = ui.available_width();
                let col = COLUMN_W.min(avail - 48.0).max(200.0);
                let side = ((avail - col) / 2.0).max(24.0);
                let mut out = Out::default();
                ui.horizontal_top(|ui| {
                    ui.add_space(side);
                    ui.vertical(|ui| {
                        ui.set_width(col);
                        ui.add_space(36.0);
                        out = render(ui, p, doc, &mut self.preview_images, &mut self.md_cache, &mut self.md, &opts);
                        ui.add_space(60.0);
                    });
                });
                out
            });
            out = res.inner;
            let max = (res.content_size.y - res.inner_rect.height()).max(0.0);
            self.md.read_max = max;
            self.md.read_frac = if max > 1.0 { (res.state.clamped_offset().y / max).clamp(0.0, 1.0) } else { 0.0 };
            let btn = egui::Button::new(RichText::new(t("Redigera")).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
                .fill(p.accent)
                .stroke(Stroke::NONE)
                .corner_radius(8)
                .min_size(vec2(92.0, 28.0));
            let at = Align2::RIGHT_TOP.align_size_within_rect(vec2(92.0, 28.0), full.shrink2(vec2(22.0, 12.0)));
            let r = ui.put(Rect::from_min_size(at.min, at.size()), btn);
            if r.on_hover_text(t("Redigera texten (Ctrl+Shift+M)")).clicked() {
                edit = true;
            }
        });
        if let Some(n) = out.toggled {
            self.md_toggle_task(&ctx, n);
        }
        if let Some(b) = out.dbl {
            self.md_open_at(&ctx, b);
        } else if edit {
            self.toggle_md_reading();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn bara_markdownfiler_från_disk_öppnas_i_läsläget() {
        let mut d = Document::new_untitled(1, 1);
        assert!(!opens_in_reading(&d), "namnlös flik redigeras direkt");
        d.path = Some(PathBuf::from("/tmp/anteckning.md"));
        assert!(opens_in_reading(&d));
        d.path = Some(PathBuf::from("/tmp/README.markdown"));
        assert!(opens_in_reading(&d));
        d.path = Some(PathBuf::from("/tmp/kod.rs"));
        assert!(!opens_in_reading(&d));
        d.path = Some(PathBuf::from("/tmp/brev.docx"));
        d.docx = true;
        d.word_view = true;
        assert!(!opens_in_reading(&d), "Word har dokumentläget");
    }

    #[test]
    fn rubrikerna_får_id_i_samma_ordning_som_navigeringsfönstret() {
        let doc = "# A\n\ntext\n\n```\n# inte en rubrik\n```\n\n## B ##\n\n### C {#eget}\n";
        let out = with_heading_ids(doc);
        assert!(out.contains("# A {#kladd-h0}\n"), "{out}");
        assert!(out.contains("## B {#kladd-h1} ##"), "{out}");
        assert!(out.contains("### C {#eget}\n") && !out.contains("kladd-h2"), "eget id behålls");
        assert!(out.contains("```\n# inte en rubrik\n```"), "kodstaket rörs inte");
        assert_eq!(out.lines().count(), doc.lines().count(), "bara rubrikraderna ändras");
        // Rubrik 1 börjar vid "## B".
        let at = doc.find("## B").unwrap();
        assert_eq!(heading_slug(doc, at).as_deref(), Some("kladd-h1"));
        assert_eq!(heading_slug(doc, 0).as_deref(), Some("kladd-h0"));
        assert_eq!(heading_slug("text utan rubrik", 3), None);
    }

    #[test]
    fn kryssrutor_vänds_och_bara_tecknet_ändras() {
        let doc = "- [ ] ett\n- [x] två\n\n```\n- [ ] kod\n```\n\n1. [ ] tre\n";
        assert_eq!(task_markers(doc).len(), 3, "kryssrutor i kodstaket räknas inte");
        assert_eq!(flip_task(doc, 0).unwrap(), doc.replacen("[ ] ett", "[x] ett", 1));
        assert_eq!(flip_task(doc, 1).unwrap(), doc.replacen("[x] två", "[ ] två", 1));
        assert_eq!(flip_task(doc, 2).unwrap(), doc.replacen("1. [ ] tre", "1. [x] tre", 1));
        assert!(flip_task(doc, 3).is_none());
        // Visningen byter tecknet; ordningen hittas igen ur skillnaden.
        let after = flip_task(doc, 2).unwrap();
        assert_eq!(toggled_task(doc, &after), Some(2));
    }

    #[test]
    fn källpositioner_översätts_till_dokumentets_text() {
        let doc = "# Rubrik\n\nförsta\n\nandra stycket\n";
        // Samma rader: samma rad och kolumn.
        let rendered = with_heading_ids(doc);
        let at = rendered.find("andra").unwrap();
        assert_eq!(to_doc_offset(&rendered, doc, at), doc.find("andra").unwrap());
        // Annat radantal (en formel blev flera rader): den lika lydande raden hittas.
        let rendered = "# Rubrik\n\n![formel](x)\n![formel](y)\n\nförsta\n\nandra stycket\n";
        let at = rendered.find("andra").unwrap();
        assert_eq!(to_doc_offset(rendered, doc, at), doc.find("andra").unwrap());
        // Förbi slutet hamnar inte utanför dokumentet.
        assert!(to_doc_offset(doc, doc, 10_000) <= doc.len());
        assert_eq!(to_doc_offset("", "", 0), 0);
    }

    #[test]
    fn raden_vid_en_höjd_och_rullningen_räknas_om() {
        let pts = [(0, 0.0), (10, 40.0), (25, 40.0), (60, 120.0)];
        assert_eq!(line_at(&pts, -5.0), None);
        assert_eq!(line_at(&pts, 10.0), Some(0));
        assert_eq!(line_at(&pts, 41.0), Some(25), "den djupaste vid samma höjd");
        assert_eq!(line_at(&pts, 500.0), Some(60));
        assert_eq!(advance_lines("a\nb\nc\nd", 0, 2), 4);
        assert_eq!(advance_lines("a\nb", 0, 9), 3);
        let mut st = MdState::default();
        st.note_editor_scroll(150.0, 1000.0, 500.0);
        assert!((st.editor_frac - 0.3).abs() < 1e-6);
        st.note_editor_scroll(0.0, 300.0, 500.0);
        assert_eq!(st.editor_frac, 0.0, "innehåll som ryms har ingen rullning");
    }
}
