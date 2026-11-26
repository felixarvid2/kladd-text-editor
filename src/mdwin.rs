//! Virtualiserad ritning av Markdown i läsläget (`mdread`) och EPUB-vyn (`epub::view`): texten delas i
//! toppnivåblock, varje blocks höjd mäts när det ritas och bara blocken i det synliga området ritas.
//! Resten får platshållarhöjd (uppskattad ur textlängden tills blocket har ritats). Därför kostar en
//! bildruta lika mycket för en 5 MB-fil som för en sida, och stora kapitel behöver inte delas.
//!
//! Hur det hänger ihop:
//! * [`split`] delar texten med samma `pulldown-cmark`-flaggor som visningen och noterar rubrik-id,
//!   kryssrutor och länkdefinitioner (som läggs efter varje block som använder dem).
//! * [`Win::show`] ritar blocken med `egui_commonmark::CommonMarkViewer::show_slice`, var och en på sin
//!   uträknade plats (överkant = summan av föregående blocks höjder). Ett block är lika högt som i en
//!   helritning (se provet `blocken_hamnar_som_i_en_helritning`), så små dokument ser ut precis som förut.
//! * Dokument under [`FULL_BYTES`] ritas helt första bildrutan så att alla höjder är exakta.
//! * När ett block ovanför det man tittar på får sin riktiga höjd flyttas rullningen lika mycket (och
//!   bildrutan ritas om) så att texten på skärmen står still. Samma sak gäller en ny uppskattning.
//! * Hopp till en rubrik (länk eller navigeringsfönstret) och till den aktiva sökträffen rullar först till
//!   blockets uppskattade plats; när blocket ritas rullar visningen exakt.
//! * `CommonMarkCache::line_points` fylls med ett punktlager för hela texten (finare för ritade block), så
//!   att dubbelklick, rullningssynk, hopp och understrykningar fungerar som när allt ritades.

use std::collections::HashMap;
use std::ops::Range;

use egui::{Align, Id, Rect, Ui, UiBuilder, pos2, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer, SliceArgs};
use pulldown_cmark::{Event, Options, Parser, Tag};

/// Dokument upp till så här många byte ritas helt första bildrutan (exakta höjder, ingen uppskattning).
pub const FULL_BYTES: usize = 64_000;
/// Så här många byte mätta block krävs innan uppskattningen för de ännu ej ritade blocken räknas om.
const CALIBRATE_AT: [usize; 3] = [4_000, 32_000, 256_000];
/// Antal byte som läggs till varje blocks längd i uppskattningen (rubriker och korta stycken tar mer plats än texten).
const EST_PAD: usize = 48;
/// Högsta antal hopp i följd som inte når fram (ett rubrik-id som inte finns, till exempel).
const MAX_JUMPS: u8 = 6;
/// Antal block utanför det ritade som får punkter i `line_points`.
const NEAR: usize = 40;

/// Ett toppnivåblock i källtexten.
#[derive(Clone, Debug)]
pub struct BlockSrc {
    /// Där blocket börjar (som i `line_points`).
    pub start: usize,
    /// Området som ritas: från radens början till blockets slut.
    pub range: Range<usize>,
    /// Blocket är ett vanligt stycke.
    pub para: bool,
    /// Kod eller HTML: länkdefinitioner läggs inte efter blocket.
    pub raw: bool,
}

/// Textens uppdelning.
#[derive(Clone, Debug, Default)]
pub struct Split {
    pub blocks: Vec<BlockSrc>,
    /// Länkdefinitioner och fotnotsdefinitioner (etikett i gemener, källtext).
    refs: Vec<(String, String)>,
    /// Rubrik-id -> blockets nummer.
    pub headings: HashMap<String, usize>,
    /// Kryssrutornas `[ ]`/`[x]` i dokumentordning.
    pub tasks: Vec<Range<usize>>,
}

fn parser_options(heading_attrs: bool) -> Options {
    let mut o = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST;
    if heading_attrs {
        o |= Options::ENABLE_HEADING_ATTRIBUTES;
    }
    o
}

/// Delar `text` i toppnivåblock. `heading_attrs`: rubrikernas `{#id}` tolkas (läsläget för Markdown).
pub fn split(text: &str, heading_attrs: bool) -> Split {
    let mut out = Split::default();
    let mut it = Parser::new_ext(text, parser_options(heading_attrs)).into_offset_iter();
    let mut depth = 0usize;
    let mut prev_end = 0usize;
    let mut push = |out: &mut Split, r: &Range<usize>, para: bool, raw: bool| {
        let line = text[..r.start.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
        let from = line.max(prev_end.min(r.start));
        out.blocks.push(BlockSrc { start: r.start, range: from..r.end.max(from), para, raw });
        prev_end = r.end;
    };
    for (e, r) in it.by_ref() {
        match &e {
            Event::Start(tag) => {
                if depth == 0 {
                    let para = matches!(tag, Tag::Paragraph);
                    let raw = matches!(tag, Tag::CodeBlock(_) | Tag::HtmlBlock);
                    push(&mut out, &r, para, raw);
                }
                if let Tag::FootnoteDefinition(label) = tag {
                    // En fotnotsreferens är bara en referens om definitionen finns i samma text: lägg definitionen efter blocken som använder den.
                    let def = text[r.clone()].trim();
                    if !def.is_empty() {
                        out.refs.push((format!("[^{}]", label.to_lowercase()), def.to_owned()));
                    }
                }
                if let Tag::Heading { id: Some(id), .. } = tag {
                    out.headings.entry(id.to_string()).or_insert(out.blocks.len().saturating_sub(1));
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::TaskListMarker(_) => out.tasks.push(r),
            Event::Rule if depth == 0 => push(&mut out, &r, false, true),
            _ if depth == 0 => push(&mut out, &r, false, true),
            _ => {}
        }
    }
    for (label, def) in it.reference_definitions().iter() {
        let mut line = format!("[{label}]: <{}>", def.dest);
        if let Some(t) = &def.title {
            line.push_str(&format!(" \"{}\"", t.replace('"', "\\\"")));
        }
        out.refs.push((label.to_lowercase(), line));
    }
    out
}

impl Split {
    /// Blocket som innehåller (eller ligger närmast före) `byte`.
    pub fn block_of(&self, byte: usize) -> usize {
        if self.blocks.is_empty() {
            return 0;
        }
        self.blocks.partition_point(|b| b.start <= byte).saturating_sub(1)
    }

    /// Länkdefinitionerna som blocket `slice` kan använda.
    fn refs_for(&self, slice: &str) -> String {
        if self.refs.is_empty() || !slice.contains('[') {
            return String::new();
        }
        let low = slice.to_lowercase();
        let mut out = String::new();
        for (label, line) in &self.refs {
            if low.contains(label.as_str()) {
                if !out.is_empty() {
                    // Tom rad emellan: en fotnotsdefinition drar annars med sig raden efter som fortsättning.
                    out.push_str("\n\n");
                }
                out.push_str(line);
            }
        }
        out
    }
}

/// Inställningar för en ritning.
pub struct Cfg<'a> {
    /// Visningens id i ritarens cache.
    pub id: Id,
    /// Textspaltens bredd.
    pub width: f32,
    /// Allt annat som påverkar blockens höjd (typsnitt, storlek, radavstånd …); ändras det mäts blocken om.
    pub key: u64,
    /// Kryssrutor går att bocka av.
    pub mutable: bool,
    /// Skapar visaren för ett block (bildbredd, färger …).
    pub make: &'a dyn Fn() -> CommonMarkViewer<'static>,
}

/// Vad en ritning gav.
pub struct WinOut {
    /// Hela textens yta (överkant och uppskattad höjd).
    pub rect: Rect,
    /// Kryssrutor som klickades (nummer i dokumentordning).
    pub toggled: Vec<usize>,
}

/// Ritarens tillstånd för en text.
pub struct Win {
    pub split: Split,
    /// Höjd (inklusive avståndet till nästa block) per block: uppskattad eller uppmätt.
    h: Vec<f32>,
    meas: Vec<bool>,
    /// Blockets punkter (källbyte, y från blockets överkant) från senaste ritningen.
    pts: Vec<Vec<(usize, f32)>>,
    /// Överkanten per block och sist totalhöjden.
    tops: Vec<f32>,
    tops_dirty: bool,
    layout_key: Option<(u32, u64)>,
    /// Pixlar per byte i uppskattningen.
    ratio: f32,
    calibrated: usize,
    /// Summa av uppmätta blocks längd och höjd, och antalet.
    meas_bytes: usize,
    meas_px: f32,
    meas_count: usize,
    /// Blocket som var överst vid förra bildrutan och dess överkant då.
    anchor: Option<(usize, f32)>,
    jumps: u8,
    /// Den aktiva sökträffen ska rullas fram.
    match_pending: bool,
    full_done: bool,
    merged: Vec<(usize, f32)>,
    view_top: f32,
    text_len: usize,
    /// Antal block som ritades senast (för mätningar).
    pub last_drawn: usize,
}

impl Win {
    pub fn new(text: &str, heading_attrs: bool) -> Self {
        let split = split(text, heading_attrs);
        let n = split.blocks.len();
        Win {
            split,
            h: vec![0.0; n],
            meas: vec![false; n],
            pts: vec![Vec::new(); n],
            tops: vec![0.0; n + 1],
            tops_dirty: true,
            layout_key: None,
            ratio: 0.2,
            calibrated: 0,
            meas_bytes: 0,
            meas_px: 0.0,
            meas_count: 0,
            anchor: None,
            jumps: 0,
            match_pending: false,
            full_done: false,
            merged: Vec::new(),
            view_top: 0.0,
            text_len: text.len(),
            last_drawn: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.split.blocks.len()
    }

    fn est(&self, i: usize) -> f32 {
        let b = &self.split.blocks[i];
        ((b.range.len() + EST_PAD) as f32 * self.ratio).max(24.0)
    }

    fn reset(&mut self, width: f32, key: (u32, u64)) {
        if let Some((w, _)) = self.layout_key {
            // Smalare spalt ger fler rader: samma text tar plats i omvänd proportion mot bredden.
            let old = f32::from_bits(w).max(1.0);
            if (old - width).abs() > 0.5 && self.calibrated > 0 {
                self.ratio *= old / width.max(1.0);
            }
        } else {
            // Ungefär 7,5 px per tecken och 24 px per rad.
            self.ratio = 24.0 * 7.5 / width.max(100.0);
        }
        self.layout_key = Some(key);
        self.anchor = None;
        self.full_done = false;
        for i in 0..self.len() {
            self.meas[i] = false;
            self.h[i] = self.est(i);
        }
        (self.meas_bytes, self.meas_px, self.meas_count) = (0, 0.0, 0);
        self.tops_dirty = true;
    }

    fn ensure_tops(&mut self) {
        if !self.tops_dirty {
            return;
        }
        let mut y = 0.0;
        self.tops.resize(self.h.len() + 1, 0.0);
        for (i, h) in self.h.iter().enumerate() {
            self.tops[i] = y;
            y += h;
        }
        let n = self.h.len();
        self.tops[n] = y;
        self.tops_dirty = false;
    }

    /// Hela textens höjd (uppskattad för block som inte ritats).
    #[cfg(test)]
    pub fn total(&mut self) -> f32 {
        self.ensure_tops();
        *self.tops.last().unwrap_or(&0.0)
    }

    /// Överkanten (från textens topp) för block `i`.
    #[cfg(test)]
    pub fn top_of(&mut self, i: usize) -> f32 {
        self.ensure_tops();
        self.tops.get(i).copied().unwrap_or(0.0)
    }

    fn block_at(&self, y: f32) -> usize {
        let n = self.len();
        if n == 0 {
            return 0;
        }
        self.tops[..n].partition_point(|t| *t <= y).saturating_sub(1)
    }

    /// Räknar om uppskattningen för block som inte ritats när tillräckligt mycket har mätts.
    fn calibrate(&mut self) -> bool {
        if self.calibrated >= CALIBRATE_AT.len() || self.meas_bytes < CALIBRATE_AT[self.calibrated] {
            return false;
        }
        while self.calibrated < CALIBRATE_AT.len() && self.meas_bytes >= CALIBRATE_AT[self.calibrated] {
            self.calibrated += 1;
        }
        self.ratio = self.meas_px / (self.meas_bytes + EST_PAD * self.meas_count) as f32;
        for i in 0..self.len() {
            if !self.meas[i] {
                self.h[i] = self.est(i);
            }
        }
        self.tops_dirty = true;
        true
    }

    /// Ritar de block som syns. Anropas med markören överst i textens yta; ritar inget under eller över
    /// det synliga (`ui.clip_rect()`) och reserverar hela textens höjd.
    pub fn show(&mut self, ui: &mut Ui, cache: &mut CommonMarkCache, text: &str, cfg: &Cfg) -> WinOut {
        let key = (cfg.width.to_bits(), cfg.key);
        if self.layout_key != Some(key) {
            self.reset(cfg.width, key);
        }
        CommonMarkViewer::begin_frame(cache, ui.ctx());
        // F3 och nya sökningar ber om att den aktiva träffen rullas fram.
        if cache.search_cache_mut(&cfg.id).take_pending_scroll_to_active_match() {
            self.match_pending = true;
        }
        let n = self.len();
        let x0 = ui.cursor().min.x;
        let origin_y = ui.cursor().min.y;
        self.ensure_tops();
        // Uppskattningen räknas om före blocken väljs, så att läget är detsamma under hela bildrutan. Allt
        // ovanför det man tittar på flyttas då (ofta långt): rulla lika mycket och rita om.
        if self.calibrate() {
            self.ensure_tops();
            if let Some((a, top_prev)) = self.anchor {
                let d = self.tops[a] - top_prev;
                if d.abs() > 0.5 {
                    ui.scroll_with_delta_animation(vec2(0.0, -d), egui::style::ScrollAnimation::none());
                    ui.ctx().request_discard("ny uppskattning av blockens höjd");
                    self.anchor = Some((a, self.tops[a]));
                    let total = self.tops[n];
                    ui.allocate_space(vec2(cfg.width, total));
                    let vt = ui.clip_rect().top() - origin_y;
                    self.view_top = vt;
                    self.set_cache(cache, cfg.id, vt, 0..0);
                    return WinOut { rect: Rect::from_min_size(pos2(x0, origin_y), vec2(cfg.width, total)), toggled: Vec::new() };
                }
            }
        }
        let clip = ui.clip_rect();
        let vis_top = clip.top() - origin_y;
        let vis_bot = clip.bottom() - origin_y;
        let render_all = !self.full_done && self.text_len <= FULL_BYTES;

        let (first, last) = if n == 0 {
            (0, 0)
        } else if render_all {
            (0, n - 1)
        } else {
            let first = self.block_at(vis_top.max(0.0));
            let mut last = first;
            while last + 1 < n && self.tops[last + 1] < vis_bot {
                last += 1;
            }
            (first, last)
        };

        // Hopp: rubriken (länk eller navigeringsfönstret) och sökträffen rullas fram om de ligger utanför det ritade.
        let mut jump: Option<(usize, Align)> = None;
        if n > 0 {
            if let Some(slug) = cache.heading_target(&cfg.id) {
                if let Some(&b) = self.split.headings.get(&slug) {
                    if !(first..=last).contains(&b) {
                        jump = Some((b, Align::TOP));
                    }
                }
            }
            if jump.is_none() && self.match_pending {
                match self.active_match_block(cache, cfg.id) {
                    Some(b) if !(first..=last).contains(&b) => jump = Some((b, Align::Center)),
                    Some(_) => {}
                    None => self.match_pending = false,
                }
            }
        }
        if let Some((b, align)) = jump {
            if self.jumps < MAX_JUMPS {
                self.jumps += 1;
                let at = pos2(x0, origin_y + self.tops[b]);
                ui.scroll_to_rect_animation(Rect::from_min_size(at, vec2(1.0, 1.0)), Some(align), egui::style::ScrollAnimation::none());
                ui.ctx().request_discard("rullar till ett block som inte är ritat");
                let total = self.tops[n];
                ui.allocate_space(vec2(cfg.width, total));
                self.set_cache(cache, cfg.id, vis_top, first..last + 1);
                return WinOut { rect: Rect::from_min_size(pos2(x0, origin_y), vec2(cfg.width, total)), toggled: Vec::new() };
            }
        }
        self.jumps = 0;

        let match_block = if self.match_pending { self.active_match_block(cache, cfg.id) } else { None };
        let mut toggled = Vec::new();
        let mut y = if n > 0 { self.tops[first] } else { 0.0 };
        let mut drawn = 0;
        // Höjden på den tomma raden som en radbrytning (`"\n"`) ger, mätt med samma stil och radhöjd som blocken.
        let row = {
            let mut probe = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, origin_y), vec2(cfg.width, 1000.0))).layout(egui::Layout::left_to_right(Align::BOTTOM).with_main_wrap(true)).invisible());
            probe.spacing_mut().item_spacing.x = 0.0;
            let h = probe.text_style_height(&egui::TextStyle::Body);
            probe.set_row_height(h);
            probe.label("\n").rect.height() / 2.0
        };
        if n > 0 {
            for i in first..=last {
                let b = &self.split.blocks[i];
                let slice = &text[b.range.clone()];
                let args = SliceArgs {
                    range: b.range.clone(),
                    ahead: self.split.blocks.get(i + 1).map_or(b.range.end, |nb| nb.range.end),
                    refs: if b.raw { String::new() } else { self.split.refs_for(slice) },
                    first: i == 0,
                    last: i + 1 == n,
                    prev_para: i > 0 && self.split.blocks[i - 1].para,
                    want_match: match_block == Some(i),
                    mutable: cfg.mutable,
                };
                // Blockets första rad delas med föregående blocks sista rad i en helritning.
                let lead = if i > 0 { row } else { 0.0 };
                let rect = Rect::from_min_size(pos2(x0, origin_y + y - lead), vec2(cfg.width, 100_000.0));
                let mut child = ui.new_child(UiBuilder::new().id_salt(("kladd-mdwin", i)).max_rect(rect));
                let out = (cfg.make)().show_slice(cfg.id, &mut child, cache, text, &args);
                let hh = out.response.rect.height() - lead;
                if out.scrolled_to_match {
                    self.match_pending = false;
                }
                for (span, _) in &out.toggled {
                    if let Some(k) = self.split.tasks.iter().position(|t| t == span) {
                        toggled.push(k);
                    }
                }
                let local = cache.slice_points(&cfg.id);
                if self.pts[i] != local {
                    self.pts[i] = local.to_vec();
                            }
                let len = self.split.blocks[i].range.len();
                if self.meas[i] {
                    self.meas_px += hh - self.h[i];
                } else {
                    self.meas_bytes += len;
                    self.meas_px += hh;
                    self.meas_count += 1;
                }
                if (self.h[i] - hh).abs() > 0.01 || !self.meas[i] {
                    self.h[i] = hh;
                    self.tops_dirty = true;
                            }
                self.meas[i] = true;
                y += hh;
                drawn += 1;
            }
        }
        self.last_drawn = drawn;
        if render_all {
            self.full_done = true;
        }
        self.ensure_tops();

        // Texten på skärmen står still när block ovanför har fått ny höjd.
        let mut delta = 0.0;
        if let Some((a, top_prev)) = self.anchor {
            if n > 0 && (first..=last).contains(&a) {
                delta = self.tops[a] - top_prev;
            }
        }
        if delta.abs() > 0.5 {
            ui.scroll_with_delta_animation(vec2(0.0, -delta), egui::style::ScrollAnimation::none());
            ui.ctx().request_discard("block ovanför det synliga fick ny höjd");
        } else {
            delta = 0.0;
        }
        if n > 0 {
            let a = self.block_at((vis_top + delta).max(0.0));
            self.anchor = Some((a, self.tops[a]));
        }
        let total = self.tops[n];
        ui.allocate_space(vec2(cfg.width, total));
        self.view_top = vis_top + delta;
        self.set_cache(cache, cfg.id, vis_top + delta, first..last + 1);
        WinOut { rect: Rect::from_min_size(pos2(x0, origin_y), vec2(cfg.width, total)), toggled }
    }

    /// Blocket med den aktiva sökträffen.
    fn active_match_block(&self, cache: &CommonMarkCache, id: Id) -> Option<usize> {
        let i = cache.active_match(&id)?;
        let r = cache.search_ranges(&id).get(i)?;
        Some(self.split.block_of(r.start))
    }

    /// Lämnar punkterna för blocken nära det synliga till cachen (`line_points`: dubbelklick, rullningssynk,
    /// understrykningar). Längre bort ger `y_of_byte` blockets överkant.
    fn set_cache(&mut self, cache: &mut CommonMarkCache, id: Id, vis_top: f32, near: Range<usize>) {
        let n = self.len();
        let (lo, hi) = (near.start.saturating_sub(NEAR), (near.end + NEAR).min(n));
        self.merged.clear();
        for i in lo..hi {
            let top = self.tops[i];
            if self.meas[i] && !self.pts[i].is_empty() {
                self.merged.extend(self.pts[i].iter().map(|&(byte, y)| (byte, top + y)));
            } else {
                self.merged.push((self.split.blocks[i].start, top));
            }
        }
        cache.set_line_points(&id, std::mem::take(&mut self.merged));
        cache.search_cache_mut(&id).last_viewport_top_y = vis_top;
    }

    /// Lämnar blockens lägen till cachen så att en ny sökning börjar vid det som syns. Anropas före
    /// `update_search_matches`.
    pub fn sync_search_layout(&mut self, cache: &mut CommonMarkCache, id: Id) {
        self.ensure_tops();
        let tops = &self.tops;
        let blocks = self.split.blocks.iter().enumerate().map(|(i, b)| (b.range.clone(), tops[i], tops[i + 1]));
        cache.set_block_layout(&id, blocks, self.view_top, 0.0);
    }

    /// Textens y (från överkanten) för källpositionen `byte`: blockets överkant plus raden i blocket om det ritats.
    /// Giltig efter en ritning.
    pub fn y_of_byte(&self, byte: usize) -> f32 {
        if self.split.blocks.is_empty() || self.tops.len() <= self.split.blocks.len() {
            return 0.0;
        }
        let i = self.split.block_of(byte);
        let top = self.tops[i];
        let inner = if self.meas[i] { self.pts[i].iter().rev().find(|(b, _)| *b <= byte).map_or(0.0, |p| p.1) } else { 0.0 };
        top + inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, RawInput};

    /// En text med alla sorters block.
    fn sample() -> String {
        let mut s = String::from("# Rubrik ett\n\nEtt vanligt stycke med *kursiv*, **fet** och `kod` samt en [länk](https://exempel.se).\n\n");
        s.push_str("- punkt ett\n- punkt två\n  - under\n  - under två\n- punkt tre\n\n1. första\n2. andra\n\n");
        s.push_str("> Ett citat\n> över två rader\n\n```rust\nfn main() {\n    println!(\"hej\");\n}\n```\n\n");
        s.push_str("| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\n---\n\n## Rubrik två\n\n- [ ] att göra\n- [x] gjort\n\n");
        s.push_str("<p align=\"center\">\n  <b>HTML</b>\n</p>\n\n[a]: https://a.example\n\nSe [a] och en fotnot[^1].\n\n[^1]: Fotnotens text.\n\n");
        for i in 0..12 {
            s.push_str(&format!("Stycke {i}. {}\n\n", "Det var en gång en lång mening som fortsätter och fortsätter för att fylla raderna. ".repeat(6)));
        }
        s
    }

    fn frame(ctx: &Context, h: f32, mut f: impl FnMut(&mut Ui)) {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, h))), ..Default::default() };
        let out = ctx.run_ui(input, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            f(ui)
        });
        out.drop_without_applying_deltas();
    }

    fn viewer() -> CommonMarkViewer<'static> {
        CommonMarkViewer::new().max_image_width(Some(700)).enable_scroll_to_heading(true)
    }

    #[test]
    fn uppdelningen_ger_toppnivåblock_rubriker_och_kryssrutor() {
        let t = sample();
        let s = split(&t, true);
        assert!(s.blocks.len() >= 15, "{}", s.blocks.len());
        assert_eq!(s.tasks.len(), 2);
        assert!(s.blocks[0].range.start == 0 && t[s.blocks[0].range.clone()].starts_with("# Rubrik ett"));
        assert!(s.blocks.windows(2).all(|w| w[0].range.end <= w[1].range.start), "blocken överlappar inte");
        assert!(s.blocks.iter().all(|b| b.range.start <= b.start && b.start < b.range.end));
        let t2 = "# A {#a}\n\ntext\n\n## B {#b}\n";
        let s2 = split(t2, true);
        assert_eq!(s2.headings.get("a"), Some(&0));
        assert_eq!(s2.headings.get("b"), Some(&2));
        assert_eq!(s2.block_of(t2.find("text").unwrap()), 1);
        assert!(split("", true).blocks.is_empty());
    }

    /// Blockens överkanter i en helritning och blockvis (för varje block som har en punkt i helritningen).
    fn tops_both(text: &str) -> Vec<(usize, f32, f32)> {
        let width = 700.0;
        let ctx = Context::default();
        let mut cache = CommonMarkCache::default();
        let id = Id::unique("hel");
        let mut full_pts = Vec::new();
        for _ in 0..3 {
            frame(&ctx, 900.0, |ui| {
                ui.set_width(width);
                viewer().show_with_id(id, ui, &mut cache, text);
                full_pts = cache.line_points(&id).to_vec();
            });
        }
        let ctx = Context::default();
        let mut cache = CommonMarkCache::default();
        let id = Id::unique("block");
        let mut win = Win::new(text, true);
        for _ in 0..3 {
            frame(&ctx, 900.0, |ui| {
                ui.set_width(width);
                let make: &dyn Fn() -> CommonMarkViewer<'static> = &viewer;
                let cfg = Cfg { id, width, key: 1, mutable: false, make };
                win.show(ui, &mut cache, text, &cfg);
            });
        }
        let blocks = win.split.blocks.clone();
        let mut out = Vec::new();
        for (i, b) in blocks.iter().enumerate() {
            // En lista får sin punkt vid första punkten (efter listans radbrytning): hoppa över, totalhöjden jämförs ändå.
            let head = &text[b.start..];
            if head.starts_with(['-', '*', '+']) && head[1..].starts_with(' ') || head.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            if let Some(want) = full_pts.iter().find(|(byte, _)| *byte == b.start).map(|p| p.1) {
                out.push((i, want, win.top_of(i)));
            }
        }
        out
    }

    #[test]
    fn avstånd_mellan_blocktyper() {
        for (name, doc) in [
            ("rubrik, stycke, stycke", "# A

text ett

text två

text tre
"),
            ("stycke, rubrik, stycke", "text ett

## B

text två

text tre
"),
            ("lista, stycke, lista, stycke", "- a
- b

text

- c
- d

text

text
"),
            ("kod, kod, regel, stycke", "```
x
```

```
y
```

---

text

text
"),
        ] {
            let t = tops_both(doc);
            eprintln!("== {name}");
            for w in t.windows(2) {
                eprintln!("  {} -> {}: hel {:5.1} blockvis {:5.1}  diff {:5.1}", w[0].0, w[1].0, w[1].1 - w[0].1, w[1].2 - w[0].2, (w[1].2 - w[0].2) - (w[1].1 - w[0].1));
            }
        }
    }

    #[test]
    fn blocken_hamnar_som_i_en_helritning() {
        let text = sample();
        let t = tops_both(&text);
        assert!(t.len() >= 10, "{}", t.len());
        for w in t.windows(2) {
            let (hel, blockvis) = (w[1].1 - w[0].1, w[1].2 - w[0].2);
            // Listor ligger emellan ibland: då är avståndet summan av flera block.
            assert!((hel - blockvis).abs() < 1.0, "block {} -> {}: hel {hel}, blockvis {blockvis}", w[0].0, w[1].0);
        }
    }

    /// Ritar `text` i ett rullfönster 700 px högt och rullar genom hela texten (`step` px per bildruta).
    /// Ger Win efter sista bildrutan och största antalet ritade block i en bildruta.
    fn sweep(text: &str, step: f32) -> (Win, usize, usize) {
        let ctx = Context::default();
        let mut cache = CommonMarkCache::default();
        let id = Id::unique("svep");
        let mut win = Win::new(text, true);
        let (mut off, mut max_drawn, mut frames) = (0.0f32, 0usize, 0usize);
        let mut total = 0.0;
        loop {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 700.0))), ..Default::default() };
            let out = ctx.run_ui(input, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                egui::ScrollArea::vertical().auto_shrink([false, false]).vertical_scroll_offset(off).show(ui, |ui| {
                    ui.set_width(700.0);
                    let make: &dyn Fn() -> CommonMarkViewer<'static> = &viewer;
                    let cfg = Cfg { id, width: ui.available_width(), key: 1, mutable: false, make };
                    win.show(ui, &mut cache, text, &cfg);
                    total = win.total();
                });
            });
            out.drop_without_applying_deltas();
            max_drawn = max_drawn.max(win.last_drawn);
            frames += 1;
            if off > total || frames > 4000 {
                break;
            }
            off += step;
        }
        (win, max_drawn, frames)
    }

    fn long_text(paragraphs: usize) -> String {
        let mut s = String::from("# Lång text\n\n");
        for i in 0..paragraphs {
            if i % 10 == 0 {
                s.push_str(&format!("## Avsnitt {i}\n\n"));
            }
            match i % 5 {
                1 => s.push_str(&format!("- punkt {i}\n- punkt två\n\n")),
                2 => s.push_str(&format!("```\nkod {i}\n```\n\n")),
                _ => s.push_str(&format!("Stycke {i}. {}\n\n", "En lång mening som fortsätter över flera rader. ".repeat(3 + i % 4))),
            }
        }
        s
    }

    #[test]
    fn stor_text_ritar_bara_det_synliga_och_hamnar_som_i_en_helritning() {
        let text = long_text(900);
        assert!(text.len() > FULL_BYTES * 2, "{}", text.len());
        let (mut win, max_drawn, frames) = sweep(&text, 300.0);
        assert!(frames > 20);
        assert!(max_drawn < 60, "bara blocken i fönstret ritas: som mest {max_drawn} av {}", win.len());
        assert!(win.meas.iter().all(|m| *m), "svepet mätte alla block");
        // Helritning av samma text.
        let want = tops_both_with(&text, &mut win);
        assert!(want.len() > 400);
        for (i, hel, vis) in &want {
            // Blockens uppmätta överkanter är desamma som i en helritning (skillnaden är en bråkdels pixel per block).
            assert!((hel - vis).abs() < 2.0 + *i as f32 * 0.05, "block {i}: hel {hel}, blockvis {vis}");
        }
    }

    /// Överkanterna i en helritning och i `win` för block som har en punkt i helritningen (utom listor).
    fn tops_both_with(text: &str, win: &mut Win) -> Vec<(usize, f32, f32)> {
        let ctx = Context::default();
        let mut cache = CommonMarkCache::default();
        let id = Id::unique("hel2");
        let mut pts = Vec::new();
        for _ in 0..2 {
            frame(&ctx, 900.0, |ui| {
                ui.set_width(700.0);
                viewer().show_with_id(id, ui, &mut cache, text);
                pts = cache.line_points(&id).to_vec();
            });
        }
        let blocks = win.split.blocks.clone();
        let mut out = Vec::new();
        for (i, b) in blocks.iter().enumerate() {
            let head = &text[b.start..];
            if head.starts_with(['-', '*', '+']) && head[1..].starts_with(' ') {
                continue;
            }
            if let Some(p) = pts.iter().find(|(byte, _)| *byte == b.start) {
                out.push((i, p.1, win.top_of(i)));
            }
        }
        out
    }

    #[test]
    fn ritade_block_ser_ut_som_i_helritningen() {
        // Samma text ritad helt och blockvis ger samma former (galleri) för varje block i fönstret.
        fn shapes(ctx: &Context, f: impl FnMut(&mut Ui)) -> Vec<String> {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 5000.0))), ..Default::default() };
            let mut f = f;
            let out = ctx.run_ui(input, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                f(ui)
            });
            let mut v = Vec::new();
            fn walk(s: &egui::Shape, v: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => v.push(format!("{:?}|{}", t.pos, t.galley.text())),
                    egui::Shape::Vec(x) => x.iter().for_each(|s| walk(s, v)),
                    _ => {}
                }
            }
            for cs in &out.shapes {
                walk(&cs.shape, &mut v);
            }
            out.drop_without_applying_deltas();
            v
        }
        let text = sample();
        let ctx = Context::default();
        let mut cache = CommonMarkCache::default();
        let id = Id::unique("a");
        let mut full = Vec::new();
        for _ in 0..2 {
            full = shapes(&ctx, |ui| {
                ui.set_width(700.0);
                viewer().show_with_id(id, ui, &mut cache, &text);
            });
        }
        let ctx = Context::default();
        let mut cache = CommonMarkCache::default();
        let id = Id::unique("b");
        let mut win = Win::new(&text, true);
        let mut by_block = Vec::new();
        for _ in 0..3 {
            by_block = shapes(&ctx, |ui| {
                ui.set_width(700.0);
                let make: &dyn Fn() -> CommonMarkViewer<'static> = &viewer;
                win.show(ui, &mut cache, &text, &Cfg { id, width: 700.0, key: 1, mutable: false, make });
            });
        }
        // Samma texter i samma ordning, och samma läge (±1 px) för alla utom listpunkter (som sitter en rad förskjutna).
        let texts = |v: &[String]| v.iter().map(|s| s.split('|').nth(1).unwrap_or("").to_owned()).collect::<Vec<_>>();
        assert_eq!(texts(&full), texts(&by_block));
        assert!(by_block.len() > 20);
    }

    #[test]
    fn fotnotsreferens_långt_från_sin_definition_ritas_som_referens() {
        let mut text = String::from("Text med en fotnot[^a] och en [länk][x].\n\n");
        for i in 0..8 {
            text.push_str(&format!("Mellanstycke {i}.\n\n"));
        }
        text.push_str("[x]: https://exempel.se\n\n[^a]: Fotnotens text.\n");
        let ctx = Context::default();
        let texts = |full: bool| {
            let mut cache = CommonMarkCache::default();
            let id = Id::unique("fn");
            let mut win = Win::new(&text, true);
            let mut v = Vec::new();
            for _ in 0..3 {
                let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 3000.0))), ..Default::default() };
                let out = ctx.run_ui(input, |ui| {
                    ui.set_width(700.0);
                    if full {
                        viewer().show_with_id(id, ui, &mut cache, &text);
                    } else {
                        let make: &dyn Fn() -> CommonMarkViewer<'static> = &viewer;
                        win.show(ui, &mut cache, &text, &Cfg { id, width: 700.0, key: 1, mutable: false, make });
                    }
                });
                v.clear();
                fn walk(s: &egui::Shape, v: &mut Vec<String>) {
                    match s {
                        egui::Shape::Text(t) => v.push(t.galley.text().to_owned()),
                        egui::Shape::Vec(x) => x.iter().for_each(|s| walk(s, v)),
                        _ => {}
                    }
                }
                for cs in &out.shapes {
                    walk(&cs.shape, &mut v);
                }
                out.drop_without_applying_deltas();
            }
            v
        };
        assert_eq!(texts(true), texts(false));
        assert!(!texts(false).iter().any(|t| t.contains("[^a]") || t.contains("[x]")), "{:?}", texts(false));
    }
}
