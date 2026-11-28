//! Ritar ett diagram (`chart_model::Chart`) till visningslistan: diagramytan, titeln, förklaringen,
//! ritytan med rutnät och axlar, serierna (stapel, linje, yta, cirkel/ring, punkt) och dataetiketter.
//!
//! Den automatiska layouten är uppmätt mot PowerPoints egna PDF-exporter av docx4j:s `p4-*`-diagram
//! (Apache-2.0; 432 × 252 pt, 12 pt text): yttre marginal 7 pt, värdeetiketter 0,92 em från ritytan,
//! kategorietiketternas baslinje 1,57 em under axeln, förklaringens baslinje 0,48 em ovanför den nedre
//! marginalen, förklaringsnyckeln 0,55 em. Talen i `docs/dev/pptx.md`. Axelskalan väljs som i Office:
//! största värdet + 5 %, steg 1/2/5·10ⁿ så att etiketterna får plats.
//!
//! All text går genom den gemensamma textmotorn (`text::layout`) och formateringen (`c:spPr`,
//! `c:txPr`) genom formernas fyllnings- och textkod (`Builder::paint`, `TextEnv::run_style`).

use super::chart_fmt;
use super::chart_model::{Axis, Chart, Grouping, Kind, Labels, Plot, Series};
use super::color::{self, Rgba};
use super::inherit::{self, Owner};
use super::render::{emu, Builder, Frame};
use super::richtext::TextEnv;
use super::scene::{Affine, Cap, Item, Join, Paint, Path, Stroke};
use super::text::{self, Align, Anchor, Para, Run, RunStyle};
use super::xml::Node;

#[path = "chart_3d_draw.rs"]
mod three_d;
#[path = "chart_analysis.rs"]
mod analysis;
#[path = "chart_radar.rs"]
mod radar;
#[path = "chart_table.rs"]
mod table;
#[path = "chart_dates.rs"]
mod dates;

/// Yttre marginal (punkter).
const PAD: f32 = 7.0;

/// En rektangel i diagrammets koordinater (punkter, origo uppe till vänster).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    fn right(&self) -> f32 {
        self.x + self.w
    }
    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Värdeaxelns skala.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl Scale {
    pub fn ticks(&self) -> Vec<f64> {
        let n = ((self.max - self.min) / self.step).round().clamp(0.0, 1000.0) as usize;
        (0..=n).map(|i| self.min + self.step * i as f64).collect()
    }
}

/// Automatisk skala: 0 med om alla värden är positiva, största + 5 %, steg 1/2/5·10ⁿ minst `min_steps`
/// av axellängden isär (`fits(steg)` avgör om etiketterna får plats).
pub fn auto_scale(lo: f64, hi: f64, fixed_min: Option<f64>, fixed_max: Option<f64>, fixed_step: Option<f64>, fits: &dyn Fn(f64, f64, f64) -> bool) -> Scale {
    let (mut lo, mut hi) = (lo.min(hi), lo.max(hi));
    if !lo.is_finite() || !hi.is_finite() {
        (lo, hi) = (0.0, 1.0);
    }
    if lo > 0.0 && fixed_min.is_none() {
        // Excel tar med 0 när det minsta värdet är nära (mindre än 5/6 av det största).
        if lo < hi * 5.0 / 6.0 || hi == lo {
            lo = 0.0;
        }
    }
    if hi < 0.0 && fixed_max.is_none() {
        hi = 0.0;
    }
    if (hi - lo).abs() < 1e-12 {
        hi = lo + 1.0;
    }
    let span = hi - lo;
    let hi_p = if hi > 0.0 { hi + 0.05 * span } else { hi };
    let lo_p = if lo < 0.0 { lo - 0.05 * span } else { lo };
    let mut candidates = Vec::new();
    let mag = 10f64.powf((span.log10() - 2.0).floor());
    for k in 0..12 {
        for m in [1.0, 2.0, 5.0] {
            candidates.push(m * mag * 10f64.powi(k));
        }
    }
    let mut pick = None;
    for &step in candidates.iter().filter(|s| fixed_step.is_none() && **s > 0.0) {
        let mn = fixed_min.unwrap_or((lo_p / step).floor() * step);
        let mx = fixed_max.unwrap_or((hi_p / step).ceil() * step);
        if fits(mn, mx, step) {
            pick = Some(Scale { min: mn, max: mx, step });
            break;
        }
    }
    let s = pick.unwrap_or_else(|| {
        let step = fixed_step.unwrap_or_else(|| candidates.last().copied().unwrap_or(1.0));
        Scale { min: fixed_min.unwrap_or((lo_p / step).floor() * step), max: fixed_max.unwrap_or((hi_p / step).ceil() * step), step }
    });
    if s.max <= s.min { Scale { max: s.min + s.step.max(1e-9), ..s } } else { s }
}

/// Ritar diagrammet i ramen `f` och ger elementen (bildkoordinater).
pub fn draw(b: &Builder<'_>, c: &Chart<'_>, f: &Frame, owner: Owner) -> Vec<Item> {
    let mut d = Draw { b, f: *f, owner, items: Vec::new(), base: 10.0, default_ppr: default_ppr(1000) };
    d.base = d.style(&[c.space_txpr], 1000).size;
    d.chart(c);
    d.items
}

/// Standardstil för diagramtext: 10 pt, temats textfärg med 65 % (som Office 2013+).
fn default_ppr(sz: i64) -> Node {
    let clr = Node {
        name: "a:schemeClr".into(),
        attrs: vec![("val".into(), "tx1".into())],
        children: vec![
            Node { name: "a:lumMod".into(), attrs: vec![("val".into(), "65000".into())], ..Default::default() },
            Node { name: "a:lumOff".into(), attrs: vec![("val".into(), "35000".into())], ..Default::default() },
        ],
        ..Default::default()
    };
    let fill = Node { name: "a:solidFill".into(), children: vec![clr], ..Default::default() };
    let latin = Node { name: "a:latin".into(), attrs: vec![("typeface".into(), "+mn-lt".into())], ..Default::default() };
    let def = Node { name: "a:defRPr".into(), attrs: vec![("sz".into(), sz.to_string())], children: vec![fill, latin], ..Default::default() };
    Node { name: "a:pPr".into(), children: vec![def], ..Default::default() }
}

struct Draw<'b, 'a> {
    b: &'b Builder<'a>,
    f: Frame,
    owner: Owner,
    items: Vec<Item>,
    /// Diagrammets grundstorlek för text (punkter).
    base: f32,
    default_ppr: Node,
}

/// En utlagd etikett: elementen i rutans koordinater och storleken.
struct Label {
    items: Vec<Item>,
    w: f32,
    h: f32,
}

impl<'b, 'a> Draw<'b, 'a> {
    // ---- Koordinater, fyllningar och linjer ----

    fn frame_of(&self, r: R) -> Frame {
        Frame { t: Affine::translate(r.x, r.y).then(&self.f.t), w: r.w.max(0.01), h: r.h.max(0.01), rot: 0.0, flip_h: false, flip_v: false, parent: self.f.t, x: r.x, y: r.y }
    }

    fn to_slide(&self, p: Path) -> Path {
        p.transformed(&self.f.t)
    }

    /// Fyllningen i ett `spPr`: `Some(None)` = uttryckligen ingen, `None` = inte angiven.
    fn fill_of(&self, sppr: Option<&Node>, r: R) -> Option<Option<Paint>> {
        let n = sppr?.children.iter().find(|c| inherit::FILLS.contains(&c.name.as_str()))?;
        // Bildfyllningar pekar på diagramdelens relationer, inte bildens: standardfärgen i stället.
        if n.name == "a:blipFill" {
            return None;
        }
        Some(self.b.paint(n, None, &self.frame_of(r), self.owner))
    }

    /// Linjen i ett `spPr` (`a:ln`): `Some(None)` = ingen linje.
    fn line_of(&self, sppr: Option<&Node>, r: R) -> Option<Option<Stroke>> {
        let ln = sppr?.child("a:ln")?;
        let fill = ln.children.iter().find(|c| inherit::FILLS.contains(&c.name.as_str()) && c.name != "a:blipFill");
        let width = ln.attr_i64("w").map_or(0.75, emu);
        match fill {
            Some(n) => Some(self.b.paint(n, None, &self.frame_of(r), self.owner).map(|p| stroke(p, width, ln))),
            // Bara bredd/streck: färgen kommer från standarden.
            None => None,
        }
    }

    fn fill(&mut self, p: Path, paint: Paint) {
        if p.is_empty() {
            return;
        }
        let p = self.to_slide(p);
        if let Paint::Image { image, t, alpha, .. } = &paint {
            self.items.push(Item::PushClip { path: p });
            self.items.push(Item::Image { image: image.clone(), t: *t, crop: [0.0; 4], alpha: *alpha });
            self.items.push(Item::PopClip);
            return;
        }
        self.items.push(Item::Fill { path: p, paint, even_odd: false });
    }

    fn stroke(&mut self, p: Path, s: Stroke) {
        if p.is_empty() || s.width <= 0.0 {
            return;
        }
        let p = self.to_slide(p);
        self.items.push(Item::Stroke { path: p, stroke: s });
    }

    /// Fyller och ramar in en bana enligt `spPr`, med standardfärger när inget anges.
    fn shape(&mut self, p: Path, r: R, sppr: Option<&Node>, def_fill: Option<Paint>, def_line: Option<Stroke>) {
        let fill = self.fill_of(sppr, r).unwrap_or(def_fill);
        let line = self.line_of(sppr, r).unwrap_or_else(|| {
            // `a:ln` med bara bredd: standardfärgen med den bredden.
            let w = sppr.and_then(|s| s.child("a:ln")).and_then(|l| l.attr_i64("w")).map(emu);
            def_line.map(|mut s| {
                if let Some(w) = w {
                    s.width = w;
                }
                s
            })
        });
        if let Some(f) = fill {
            self.fill(p.clone(), f);
        }
        if let Some(s) = line {
            self.stroke(p, s);
        }
    }

    // ---- Färger ----

    fn scheme(&self, name: &str) -> Rgba {
        self.b.sheets.colors.scheme_color(name).unwrap_or(Rgba::BLACK)
    }

    /// Seriefärgen för index `i` av `n` enligt diagramformatet (`c:style`).
    fn auto_color(&self, style: i64, i: usize, n: usize) -> Rgba {
        let s = ((style - 1).rem_euclid(8) + 1) as usize;
        match s {
            1 => {
                // Gråskala: från mörk till ljus.
                let t = (i as f32 + 1.0) / (n as f32 + 1.0);
                lum(self.scheme("tx1"), 1.0 - t * 0.8, t * 0.75)
            }
            3..=8 => {
                let base = self.scheme(&format!("accent{}", s - 2));
                let t = (i as f32 + 1.0) / (n as f32 + 1.0);
                if t < 0.5 { lum(base, 0.5 + t, 0.0) } else { lum(base, 1.0 - (t - 0.5) * 1.2, (t - 0.5) * 1.2) }
            }
            _ => {
                let base = self.scheme(&format!("accent{}", i % 6 + 1));
                match (i / 6) % 6 {
                    0 => base,
                    1 => lum(base, 0.6, 0.0),
                    2 => lum(base, 0.8, 0.2),
                    3 => lum(base, 0.8, 0.0),
                    4 => lum(base, 0.6, 0.4),
                    _ => lum(base, 0.5, 0.0),
                }
            }
        }
    }

    // ---- Text ----

    /// Körningsstilen ur en kedja av `c:txPr`/`c:rich` (närmast först) och standarden `sz`.
    fn style(&self, chain: &[Option<&Node>], sz: i64) -> RunStyle {
        let env = TextEnv { sheets: &self.b.sheets, lists: Vec::new(), font_ref: None, font_color: None, font_scale: 1.0, slide_no: self.b.slide_no };
        let def = if sz == 1000 { None } else { Some(default_ppr(sz)) };
        let mut levels: Vec<&Node> = chain.iter().flatten().filter_map(|n| n.child("a:p").and_then(|p| p.child("a:pPr"))).collect();
        levels.push(def.as_ref().unwrap_or(&self.default_ppr));
        env.run_style(None, &levels).0
    }

    /// Stilen för en körning i en `c:rich`-text.
    fn run_style(&self, rpr: Option<&Node>, ppr: Option<&Node>, chain: &[Option<&Node>], sz: i64) -> RunStyle {
        let env = TextEnv { sheets: &self.b.sheets, lists: Vec::new(), font_ref: None, font_color: None, font_scale: 1.0, slide_no: self.b.slide_no };
        let def = default_ppr(sz);
        let mut levels: Vec<&Node> = ppr.into_iter().collect();
        levels.extend(chain.iter().flatten().filter_map(|n| n.child("a:p").and_then(|p| p.child("a:pPr"))));
        levels.push(&def);
        env.run_style(rpr, &levels).0
    }

    /// Stycken ur en `c:rich` (fält ersätts med `fields`).
    fn rich_paras(&self, rich: &Node, chain: &[Option<&Node>], sz: i64, align: Align, fields: &dyn Fn(&str) -> Option<String>) -> Vec<Para> {
        let mut out = Vec::new();
        for p in rich.children_named("a:p") {
            let ppr = p.child("a:pPr");
            let mut runs = Vec::new();
            for r in &p.children {
                match r.name.as_str() {
                    "a:r" | "a:fld" => {
                        let st = self.run_style(r.child("a:rPr"), ppr, chain, sz);
                        let mut t = r.child("a:t").map(|t| t.text.clone()).unwrap_or_default();
                        if r.name == "a:fld" {
                            if let Some(v) = r.attr("type").and_then(fields) {
                                t = v;
                            }
                        }
                        // Radbrytningar i texten blir egna stycken nedan.
                        for (k, part) in t.split('\n').enumerate() {
                            if k > 0 {
                                runs.push(Run { text: String::new(), style: st.clone(), line_break: true });
                            }
                            if !part.is_empty() {
                                runs.push(Run { text: part.to_owned(), style: st.clone(), line_break: false });
                            }
                        }
                    }
                    "a:br" => runs.push(Run { text: String::new(), style: self.run_style(r.child("a:rPr"), ppr, chain, sz), line_break: true }),
                    _ => {}
                }
            }
            let end_style = self.run_style(p.child("a:endParaRPr"), ppr, chain, sz);
            let algn = match ppr.and_then(|p| p.attr("algn")) {
                Some("l") => Align::Left,
                Some("r") => Align::Right,
                Some("ctr") => Align::Center,
                _ => align,
            };
            out.push(Para { runs, align: algn, end_style, ..Para::default() });
        }
        out
    }

    fn plain_paras(text: &str, style: &RunStyle, align: Align) -> Vec<Para> {
        text.split('\n')
            .map(|line| Para { runs: vec![Run { text: line.to_owned(), style: style.clone(), line_break: false }], align, end_style: style.clone(), ..Para::default() })
            .collect()
    }

    /// Lägger ut stycken; `max_w` är största bredd (radbrytning) eller oändligt.
    fn lay(&self, paras: Vec<Para>, max_w: f32) -> Label {
        let fonts = &self.b.sheets.deck.fonts;
        let wrap = max_w.is_finite();
        let probe = text::Frame { paras: paras.iter().map(|p| Para { align: Align::Left, ..p.clone() }).collect(), wrap, ..Default::default() };
        let laid = text::layout(&probe, if wrap { max_w.max(1.0) } else { 100_000.0 }, 10_000.0, fonts);
        let w = text_width(&laid.items);
        let h = laid.height;
        // Lägg ut igen i den uppmätta bredden så att justeringen per rad blir rätt.
        let fr = text::Frame { paras, wrap: true, anchor: Anchor::Top, ..Default::default() };
        let laid = text::layout(&fr, w + 0.5, h.max(1.0), fonts);
        Label { items: laid.items, w, h }
    }

    /// Placerar en etikett med punkten (x, y) som ankare; `ax`/`ay` 0 = vänster/topp, 0,5 = mitt, 1 = höger/botten.
    fn place(&mut self, l: Label, x: f32, y: f32, ax: f32, ay: f32, rot: f32) {
        let t = Affine::translate(-l.w * ax, -l.h * ay).then(&Affine::rotate(rot)).then(&Affine::translate(x, y)).then(&self.f.t);
        self.items.extend(text::transform_items(l.items, &t));
    }

    // ---- Diagrammet ----

    fn chart(&mut self, c: &Chart<'_>) {
        let (cw, ch) = (self.f.w, self.f.h);
        let full = R { x: 0.0, y: 0.0, w: cw, h: ch };
        // Diagramytan.
        let path = Path::rect(0.0, 0.0, cw, ch);
        self.shape(path, full, c.space_sppr, None, None);
        let mut area = R { x: PAD, y: PAD, w: cw - 2.0 * PAD, h: ch - 2.0 * PAD };
        let s = self.base;

        // Titeln.
        if let Some(t) = &c.title {
            let single = c.plots.iter().map(|p| p.series.len()).sum::<usize>() == 1;
            let auto = || -> String {
                if single { c.plots.iter().flat_map(|p| p.series.first()).map(|s| s.name.clone()).next().unwrap_or_default() } else { crate::i18n::t("Diagramrubrik").to_owned() }
            };
            let chain = [t.txpr, c.space_txpr];
            let tsz = ((s * 1.2 * 100.0).round() as i64).max(100);
            let paras = match (t.rich, &t.text) {
                (Some(rich), _) => self.rich_paras(rich, &chain, tsz, Align::Center, &|_| None),
                (None, Some(txt)) => Self::plain_paras(txt, &self.style(&chain, tsz), Align::Center),
                (None, None) => Self::plain_paras(&auto(), &self.style(&chain, tsz), Align::Center),
            };
            let l = self.lay(paras, cw * 0.8);
            let (tx, ty) = match t.layout.and_then(|m| Some((m.x?, m.y?))) {
                Some((x, y)) => (x * cw, y * ch),
                None => ((cw - l.w) / 2.0, PAD),
            };
            let r = R { x: tx, y: ty, w: l.w, h: l.h };
            self.shape(Path::rect(r.x, r.y, r.w, r.h), r, t.sppr, None, None);
            let lh = l.h;
            self.place(l, tx, ty, 0.0, 0.0, 0.0);
            if !t.overlay {
                let bottom = ty + lh + PAD * 0.5;
                if bottom > area.y {
                    area.h -= bottom - area.y;
                    area.y = bottom;
                }
            }
        }

        // Förklaringen.
        if let Some(lg) = &c.legend {
            self.legend(c, lg, &mut area);
        }

        // Ritytan.
        let manual = c.plot_layout.filter(|m| m.w.is_some() && m.h.is_some() && !m.x_factor && !m.y_factor);
        let (outer, inner_fixed) = match manual {
            Some(m) => {
                let r = R { x: m.x.unwrap_or(0.0) * cw, y: m.y.unwrap_or(0.0) * ch, w: m.w.unwrap_or(1.0) * cw, h: m.h.unwrap_or(1.0) * ch };
                if m.inner { (r, Some(r)) } else { (r, None) }
            }
            None => (area, None),
        };
        let pies: Vec<&Plot<'_>> = c.plots.iter().filter(|p| matches!(p.kind, Kind::Pie { .. })).collect();
        if !pies.is_empty() {
            let inner = inner_fixed.unwrap_or(outer);
            self.shape(Path::rect(inner.x, inner.y, inner.w, inner.h), inner, c.plot_sppr, None, None);
            for p in pies {
                self.pie(c, p, inner);
            }
            return;
        }
        if c.plots.iter().any(|p| matches!(p.kind, Kind::Radar { .. })) {
            let inner = inner_fixed.unwrap_or(outer);
            self.shape(Path::rect(inner.x, inner.y, inner.w, inner.h), inner, c.plot_sppr, None, None);
            self.radar(c, inner);
            return;
        }
        if c.plots.iter().any(|p| p.three_d) {
            self.axes_plot_3d(c, outer, inner_fixed);
        } else {
            self.axes_plot(c, outer, inner_fixed);
        }
    }

    // ---- Förklaring ----

    fn legend(&mut self, c: &Chart<'_>, lg: &super::chart_model::Legend<'_>, area: &mut R) {
        let s_size = self.style(&[lg.txpr, c.space_txpr], 1000);
        let s = s_size.size;
        // Poster: (text, fyllning/linje-spPr, standardfärg, linjediagram?)
        struct Entry<'n> {
            text: String,
            sppr: Option<&'n Node>,
            color: Rgba,
            line: bool,
        }
        let mut entries: Vec<Entry<'_>> = Vec::new();
        let total: usize = c.plots.iter().map(|p| p.series.len()).sum();
        for p in &c.plots {
            let by_point = p.vary_colors && p.series.len() == 1;
            for ser in &p.series {
                if by_point {
                    let n = ser.cats.len().max(ser.vals.len());
                    for i in 0..n {
                        let sp = ser.point(i).and_then(|d| d.sppr).or(ser.sppr);
                        entries.push(Entry { text: ser.cats.get(i).cloned().unwrap_or_else(|| (i + 1).to_string()), sppr: sp, color: self.auto_color(c.style, i, n), line: false });
                    }
                } else {
                    // 3D-linjer är band: nyckeln är en ruta i bandets färg.
                    let line = (matches!(p.kind, Kind::Line | Kind::Scatter) && !p.three_d) || p.kind == (Kind::Radar { filled: false });
                    entries.push(Entry { text: ser.name.clone(), sppr: ser.sppr, color: self.auto_color(c.style, ser.idx, total), line });
                }
            }
        }
        let mut entries: Vec<Entry<'_>> = entries.into_iter().enumerate().filter(|(i, _)| !lg.deleted.contains(i)).map(|(_, e)| e).collect();
        // Liggande grupperade staplar: förklaringen i samma ordning som staplarna (nerifrån och upp).
        if c.plots.first().is_some_and(|p| p.kind == Kind::Bar { horizontal: true } && p.grouping == Grouping::Clustered) && lg.pos != "r" && lg.pos != "l" {
            entries.reverse();
        }
        if entries.is_empty() {
            return;
        }
        let key = 0.55 * s;
        let key_w = |e: &Entry<'_>| if e.line { 1.59 * s } else { key };
        let gap_key = 0.245 * s;
        let gap_entry = 0.98 * s;
        let labels: Vec<Label> = entries.iter().map(|e| self.lay(Self::plain_paras(&e.text, &s_size, Align::Left), f32::INFINITY)).collect();
        let line_h = labels.iter().map(|l| l.h).fold(0.0, f32::max);
        let widths: Vec<f32> = entries.iter().zip(&labels).map(|(e, l)| key_w(e) + gap_key + l.w).collect();
        let vertical = matches!(lg.pos.as_str(), "r" | "l" | "tr");
        let (lw, lh) = if vertical {
            (widths.iter().fold(0.0f32, |a, &b| a.max(b)), line_h * entries.len() as f32)
        } else {
            (widths.iter().sum::<f32>() + gap_entry * (entries.len() as f32 - 1.0), line_h)
        };
        let (cw, ch) = (self.f.w, self.f.h);
        let (x0, y0) = match lg.layout.and_then(|m| Some((m.x?, m.y?))).filter(|_| lg.layout.is_some_and(|m| !m.x_factor && !m.y_factor)) {
            Some((x, y)) => (x * cw, y * ch),
            None => match lg.pos.as_str() {
                "r" | "tr" => (cw - PAD - lw, if lg.pos == "tr" { PAD } else { area.y + (area.h - lh) / 2.0 }),
                "l" => (PAD, area.y + (area.h - lh) / 2.0),
                "t" => ((cw - lw) / 2.0, area.y),
                _ => ((cw - lw) / 2.0, ch - PAD - 0.25 * s - lh),
            },
        };
        let box_r = R { x: x0 - 2.0, y: y0 - 2.0, w: lw + 4.0, h: lh + 4.0 };
        self.shape(Path::rect(box_r.x, box_r.y, box_r.w, box_r.h), box_r, lg.sppr, None, None);
        if !lg.overlay && lg.layout.is_none() {
            match lg.pos.as_str() {
                "r" | "tr" => area.w = (x0 - 0.75 * s - area.x).max(1.0),
                "l" => {
                    let nx = x0 + lw + 0.75 * s;
                    area.w -= nx - area.x;
                    area.x = nx;
                }
                "t" => {
                    let ny = y0 + lh + 0.5 * s;
                    area.h -= ny - area.y;
                    area.y = ny;
                }
                _ => area.h = (y0 - 0.75 * s - area.y).max(1.0),
            }
        }
        let (mut x, mut y) = (x0, y0);
        for ((e, l), w) in entries.iter().zip(labels).zip(widths) {
            let kw = key_w(e);
            let cy = y + line_h * 0.55;
            if e.line {
                let r = R { x, y: cy - 1.0, w: kw, h: 2.0 };
                let mut p = Path::default();
                p.move_to(x, cy);
                p.line_to(x + kw, cy);
                let def = Stroke { paint: Paint::Solid(e.color), width: 2.25, dash: Vec::new(), cap: Cap::Round, join: Join::Round };
                let st = self.line_of(e.sppr, r).unwrap_or(Some(def));
                if let Some(st) = st {
                    self.stroke(p, st);
                }
            } else {
                let r = R { x, y: cy - key / 2.0, w: key, h: key };
                self.shape(Path::rect(r.x, r.y, r.w, r.h), r, e.sppr, Some(Paint::Solid(e.color)), None);
            }
            self.place(l, x + kw + gap_key, y, 0.0, 0.0, 0.0);
            if vertical {
                y += line_h;
            } else {
                x += w + gap_entry;
            }
        }
    }

    // ---- Cirkel och ring ----

    fn pie(&mut self, c: &Chart<'_>, p: &Plot<'_>, area: R) {
        let Kind::Pie { hole, three_d } = p.kind else { return };
        if three_d {
            self.pie3d(c, p, area);
            return;
        }
        let Some(ser) = p.series.first() else { return };
        let vals: Vec<f64> = ser.vals.iter().map(|v| v.unwrap_or(0.0).abs()).collect();
        let total: f64 = vals.iter().sum();
        if total <= 0.0 {
            return;
        }
        let n = vals.len();
        let expl = |i: usize| ser.point(i).and_then(|d| d.explosion).unwrap_or(ser.explosion).max(0.0) / 100.0;
        let max_expl = (0..n).map(expl).fold(0.0f32, f32::max);
        let tilt = 1.0;
        // Diameter: 92,5 % av den minsta sidan (uppmätt i PowerPoints PDF för p4-pie). 3D: `chart_3d_draw.rs`.
        let d = area.w.min(area.h) * 0.925 / (1.0 + max_expl);
        let r = d / 2.0;
        let th = 0.0;
        let (cx, cy) = (area.x + area.w / 2.0, area.y + area.h / 2.0);
        let first = p.first_angle;
        // Vinklar (grader medurs från klockan 12).
        let mut angles = Vec::with_capacity(n);
        let mut a = first;
        for v in &vals {
            let sweep = (*v / total) as f32 * 360.0;
            angles.push((a, sweep));
            a += sweep;
        }
        let pt = |ang: f32, rr: f32, ox: f32, oy: f32| -> (f32, f32) {
            let t = ang.to_radians();
            (cx + ox + rr * t.sin(), cy + oy - rr * t.cos() * tilt)
        };
        let offs: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let (a0, sw) = angles[i];
                let mid = (a0 + sw / 2.0).to_radians();
                let e = expl(i) * r;
                (e * mid.sin(), -e * mid.cos() * tilt)
            })
            .collect();
        let colors: Vec<Rgba> = (0..n).map(|i| if p.vary_colors { self.auto_color(c.style, i, n) } else { self.auto_color(c.style, ser.idx, 1) }).collect();
        let color = |i: usize| colors[i];
        let slice_path = |i: usize, oy_extra: f32| -> Path {
            let (a0, sw) = angles[i];
            let (ox, oy) = (offs[i].0, offs[i].1 + oy_extra);
            let steps = ((sw / 3.0).ceil() as usize).clamp(2, 180);
            let mut path = Path::default();
            if hole > 0.0 {
                let (x, y) = pt(a0, r * hole, ox, oy);
                path.move_to(x, y);
            } else {
                path.move_to(cx + ox, cy + oy);
            }
            for k in 0..=steps {
                let (x, y) = pt(a0 + sw * k as f32 / steps as f32, r, ox, oy);
                path.line_to(x, y);
            }
            if hole > 0.0 {
                for k in (0..=steps).rev() {
                    let (x, y) = pt(a0 + sw * k as f32 / steps as f32, r * hole, ox, oy);
                    path.line_to(x, y);
                }
            }
            path.close();
            path
        };
        let def_line = |_: usize| -> Option<Stroke> { None };
        for i in 0..n {
            let path = slice_path(i, 0.0);
            let sp = ser.point(i).and_then(|x| x.sppr).or(ser.sppr);
            let bounds = R { x: cx - r, y: cy - r * tilt, w: 2.0 * r, h: 2.0 * r * tilt };
            self.shape(path, bounds, sp, Some(Paint::Solid(color(i))), def_line(i));
        }
        // Dataetiketter.
        for i in 0..n {
            let l = ser.labels_for(i);
            if !l.any() {
                continue;
            }
            let (a0, sw) = angles[i];
            let mid = a0 + sw / 2.0;
            let pct = vals[i] / total;
            let lbl = self.data_label(c, p, ser, i, l, Some(pct), true);
            let Some(lbl) = lbl else { continue };
            let pos = l.pos.as_deref().unwrap_or("bestFit");
            let (ox, oy) = offs[i];
            let (lw, lh) = (lbl.w, lbl.h);
            let (x, y, ax, ay) = match pos {
                "outEnd" => {
                    let t = mid.to_radians();
                    let (px, py) = pt(mid, r * 1.05 + 2.0, ox, oy + if t.cos() < 0.0 { th } else { 0.0 });
                    (px, py, 0.5 - 0.5 * t.sin(), 0.5 + 0.5 * t.cos())
                }
                "inEnd" => {
                    let (px, py) = pt(mid, r * 0.8, ox, oy);
                    (px, py, 0.5, 0.5)
                }
                "ctr" => {
                    let (px, py) = pt(mid, r * if hole > 0.0 { (1.0 + hole) / 2.0 } else { 0.5 }, ox, oy);
                    (px, py, 0.5, 0.5)
                }
                _ => {
                    // Bästa läge: inne i sektorn om etiketten får plats, annars utanför.
                    let chord = 2.0 * r * 0.65 * (sw.to_radians() / 2.0).sin().abs();
                    if lw < chord.max(r * 0.6) && lh < r * 0.5 && sw > 25.0 {
                        let (px, py) = pt(mid, r * 0.65, ox, oy);
                        (px, py, 0.5, 0.5)
                    } else {
                        let t = mid.to_radians();
                        let (px, py) = pt(mid, r * 1.05 + 2.0, ox, oy);
                        (px, py, 0.5 - 0.5 * t.sin(), 0.5 + 0.5 * t.cos())
                    }
                }
            };
            self.label_box(lbl, l, x, y, ax, ay);
        }
    }

    // ---- Axeldiagram ----

    fn axes_plot<'n>(&mut self, c: &Chart<'n>, outer: R, inner_fixed: Option<R>) {
        // Datumaxel: en kategori per tidsenhet (`chart_dates.rs`).
        let dated = dates::remap(c);
        let date_step = dated.as_ref().and_then(|d| d.1);
        let c: &Chart<'n> = dated.as_ref().map_or(c, |d| &d.0);
        let first = &c.plots[0];
        let horizontal = matches!(first.kind, Kind::Bar { horizontal: true });
        let scatter = matches!(first.kind, Kind::Scatter);
        let find = |id: &str| c.axes.iter().find(|a| a.id == id);
        let cat_ax: Option<&Axis<'_>> = first.ax_ids.iter().filter_map(|id| find(id)).find(|a| a.kind != "valAx" || scatter && matches!(a.pos.as_str(), "b" | "t"));
        let val_ax: Option<&Axis<'_>> = first.ax_ids.iter().filter_map(|id| find(id)).find(|a| a.kind == "valAx" && cat_ax.is_none_or(|cx| cx.id != a.id));
        let ncat = c.plots.iter().flat_map(|p| p.series.iter()).map(|s| s.vals.len().max(s.cats.len())).max().unwrap_or(0).max(1);
        let cats: Vec<String> = c.plots.iter().flat_map(|p| p.series.iter()).map(|s| s.cats.clone()).find(|v| !v.is_empty()).unwrap_or_else(|| (1..=ncat).map(|i| i.to_string()).collect());

        // Sekundär värdeaxel: en senare plotts värdeaxel som inte är den första plottens (punktdiagram har
        // två värdeaxlar och ingen sekundär här).
        let sec_ax: Option<&Axis<'_>> = if scatter {
            None
        } else {
            c.plots.iter().skip(1).flat_map(|p| p.ax_ids.iter().filter_map(|id| find(id))).find(|a| a.kind == "valAx" && val_ax.is_none_or(|v| v.id != a.id) && cat_ax.is_none_or(|cx| cx.id != a.id))
        };
        let on_sec = |p: &Plot<'_>| sec_ax.is_some_and(|s| p.ax_ids.contains(&s.id));
        let (lo2, hi2, percent2) = value_range(c.plots.iter().filter(|p| on_sec(p)), ncat);

        // Värdeområdet.
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut percent = false;
        for p in c.plots.iter().filter(|p| !on_sec(p)) {
            let stacked = p.grouping != Grouping::Clustered && !matches!(p.kind, Kind::Scatter);
            if p.grouping == Grouping::Percent {
                percent = true;
                lo = lo.min(0.0);
                hi = hi.max(1.0);
                continue;
            }
            if stacked {
                for i in 0..ncat {
                    let (mut pos, mut neg) = (0.0, 0.0);
                    for s in &p.series {
                        let v = s.vals.get(i).copied().flatten().unwrap_or(0.0);
                        if v >= 0.0 { pos += v } else { neg += v }
                    }
                    lo = lo.min(neg);
                    hi = hi.max(pos);
                }
            } else {
                for v in p.series.iter().flat_map(|s| s.vals.iter().flatten()) {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
        }
        if lo > hi {
            (lo, hi) = (0.0, 1.0);
        }
        // Felstaplarna ska få plats (Excel räknar in dem i skalan).
        let (lo, hi) = error_extent(c.plots.iter().filter(|p| !on_sec(p)), (lo, hi));
        let (lo2, hi2) = error_extent(c.plots.iter().filter(|p| on_sec(p)), (lo2, hi2));
        let x_range = scatter.then(|| {
            let xs: Vec<f64> = first.series.iter().flat_map(|s| s.xvals.iter().flatten().copied()).collect();
            if xs.is_empty() { (1.0, ncat as f64) } else { (xs.iter().cloned().fold(f64::INFINITY, f64::min), xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max)) }
        });

        let val_style = self.style(&[val_ax.and_then(|a| a.txpr), c.space_txpr], 1000);
        let cat_style = self.style(&[cat_ax.and_then(|a| a.txpr), c.space_txpr], 1000);
        let val_fmt = |v: f64| -> String {
            let code = match val_ax {
                Some(a) if !a.source_linked => a.num_fmt.clone().unwrap_or_else(|| "General".into()),
                _ if percent => "0%".into(),
                _ => first.series.first().map(|s| s.format.clone()).unwrap_or_else(|| "General".into()),
            };
            chart_fmt::format(v, &code)
        };
        let (vs, cs) = (val_style.size, cat_style.size);
        let (left_s, bottom_s) = if horizontal { (cs, vs) } else { (vs, cs) };
        let show_val_lbl = val_ax.is_some_and(|a| !a.deleted && a.tick_lbl_pos != "none");
        let show_cat_lbl = cat_ax.is_some_and(|a| !a.deleted && a.tick_lbl_pos != "none");

        // Mått på etiketterna (för ritytans läge).
        let measure = |d: &Self, t: &str, st: &RunStyle| -> (f32, f32) {
            let l = d.lay(Self::plain_paras(t, st, Align::Left), f32::INFINITY);
            (l.w, l.h)
        };
        let cat_line_h = measure(self, "0", &cat_style).1;
        let cat_w = if show_cat_lbl && !scatter { cats.iter().map(|t| measure(self, t, &cat_style).0).fold(0.0, f32::max) } else { 0.0 };

        let fits_vertical = |len: f32| move |mn: f64, mx: f64, step: f64| -> bool { (len as f64) / ((mx - mn) / step) >= 1.5 * vs as f64 * 0.98 || (mx - mn) / step <= 2.0 };
        // Första gissning av ritytan för skalan.
        let guess = inner_fixed.unwrap_or(R { x: outer.x + 3.0 * vs, y: outer.y + 0.45 * vs, w: outer.w - 3.5 * vs, h: outer.h - 2.3 * vs });
        let scale_for = |d: &Self, inner: R| -> Scale {
            let (fmin, fmax, fstep) = val_ax.map_or((None, None, None), |a| (a.min, a.max, a.major));
            let (fmin, fmax) = if percent { (fmin.or(Some(0.0)), fmax.or(Some(1.0))) } else { (fmin, fmax) };
            if horizontal {
                let fits = |mn: f64, mx: f64, step: f64| -> bool {
                    let n = ((mx - mn) / step).round().max(1.0);
                    let wmax = [mn, mx, mx - step].iter().map(|v| measure(d, &val_fmt(*v), &val_style).0).fold(0.0, f32::max);
                    // Liggande värdeaxel: minst två em luft mellan etiketterna (uppmätt i PowerPoint).
                    (inner.w as f64) / n >= (wmax + 2.0 * vs) as f64
                };
                auto_scale(lo, hi, fmin, fmax, fstep, &fits)
            } else {
                auto_scale(lo, hi, fmin, fmax, fstep, &fits_vertical(inner.h))
            }
        };
        let mut scale = scale_for(self, guess);
        // Den sekundära axelns skala och etiketter (till höger, eller överst för liggande staplar).
        let sec_style = self.style(&[sec_ax.and_then(|a| a.txpr), c.space_txpr], 1000);
        let sec_fmt = |v: f64| -> String {
            let code = match sec_ax {
                Some(a) if !a.source_linked => a.num_fmt.clone().unwrap_or_else(|| "General".into()),
                _ if percent2 => "0%".into(),
                _ => c.plots.iter().filter(|p| on_sec(p)).flat_map(|p| p.series.first()).map(|s| s.format.clone()).next().unwrap_or_else(|| "General".into()),
            };
            chart_fmt::format(v, &code)
        };
        let show_sec_lbl = sec_ax.is_some_and(|a| !a.deleted && a.tick_lbl_pos != "none");
        let sec_scale_for = |inner: R| -> Scale {
            let a = sec_ax;
            let (fmin, fmax, fstep) = a.map_or((None, None, None), |a| (a.min, a.max, a.major));
            let (fmin, fmax) = if percent2 { (fmin.or(Some(0.0)), fmax.or(Some(1.0))) } else { (fmin, fmax) };
            auto_scale(lo2, hi2, fmin, fmax, fstep, &fits_vertical(if horizontal { inner.w } else { inner.h }))
        };
        let mut scale2 = sec_scale_for(guess);
        let x_scale = x_range.map(|(a, b2)| {
            let fits = |mn: f64, mx: f64, step: f64| -> bool {
                let n = ((mx - mn) / step).round().max(1.0);
                let wmax = [mn, mx].iter().map(|v| measure(self, &chart_fmt::general(*v), &cat_style).0).fold(0.0, f32::max);
                (guess.w as f64) / n >= (wmax + 2.0 * cs) as f64
            };
            let ax = cat_ax.filter(|a| a.kind == "valAx");
            auto_scale(a, b2, ax.and_then(|a| a.min), ax.and_then(|a| a.max), ax.and_then(|a| a.major), &fits)
        });

        let between = val_ax.and_then(|a| a.cross_between.as_deref()).map_or(!matches!(first.kind, Kind::Area), |b| b == "between");
        // Datatabell (stående kategoridiagram med kategorierna i band).
        let dtable = c.dtable.filter(|_| !horizontal && !scatter && between).map(|t| table::Table { t, cats: cats.clone(), style: self.style(&[t.txpr, c.space_txpr], 1000) });
        let show_cat_lbl = show_cat_lbl && dtable.is_none();
        let cat_rot = cat_ax.and_then(|a| a.rot);
        // Ritytans inre rektangel.
        let inner = inner_fixed.unwrap_or_else(|| {
            let val_w = if show_val_lbl { scale.ticks().iter().map(|v| measure(self, &val_fmt(*v), &val_style).0).fold(0.0, f32::max) } else { 0.0 };
            let (mut left_w, mut bottom_h) = if horizontal {
                (if show_cat_lbl { cat_w + 0.92 * cs } else { 0.0 }, if show_val_lbl { 1.81 * vs } else { 0.0 })
            } else {
                (if show_val_lbl { val_w + 0.92 * vs } else { 0.0 }, if show_cat_lbl || scatter { 1.81 * cs } else { 0.0 })
            };
            // Vridna kategorietiketter (`bodyPr rot`): höjden är den vridna etikettens.
            if let (Some(rot), false, true) = (cat_rot, horizontal, show_cat_lbl && !scatter) {
                let (sn, cn) = rot.to_radians().sin_cos();
                bottom_h = cat_w * sn.abs() + cat_line_h * cn.abs() + 0.6 * cs;
            }
            // Datatabellen tar kategorietiketternas plats under ritytan och har seriernas namn till vänster.
            if let Some((name_w, table_h)) = dtable.as_ref().map(|t| t.size(self, c)) {
                bottom_h = table_h + 0.3 * cs;
                left_w = left_w.max(name_w);
            }
            let mut x = outer.x + left_w;
            let mut y = outer.y + 0.45 * left_s;
            let mut right = outer.right() - 0.33 * bottom_s;
            if show_sec_lbl {
                if horizontal {
                    y = outer.y + 1.81 * sec_style.size;
                } else {
                    let w2 = scale2.ticks().iter().map(|v| measure(self, &sec_fmt(*v), &sec_style).0).fold(0.0, f32::max);
                    right = outer.right() - w2 - 0.92 * sec_style.size;
                }
            }
            // Kategorier på strecken (`midCat`): första och sista etiketten ska få plats i diagrammet.
            if !between && !horizontal && show_cat_lbl && !scatter {
                if let (Some(f0), Some(fl)) = (cats.first(), cats.last()) {
                    x = x.max(outer.x + 0.33 * cs + measure(self, f0, &cat_style).0 / 2.0);
                    right = right.min(outer.right() - 0.33 * cs - measure(self, fl, &cat_style).0 / 2.0);
                }
            }
            R { x, y, w: (right - x).max(1.0), h: (outer.bottom() - bottom_h - y).max(1.0) }
        });
        if inner_fixed.is_none() {
            scale = scale_for(self, inner);
        }
        scale2 = if sec_ax.is_some() { sec_scale_for(inner) } else { scale2 };
        let sec_rev = sec_ax.is_some_and(|a| a.reversed);
        let vpos2 = move |v: f64| -> f32 {
            let t = ((v - scale2.min) / (scale2.max - scale2.min)) as f32;
            let t = if sec_rev { 1.0 - t } else { t };
            if horizontal { inner.x + t * inner.w } else { inner.bottom() - t * inner.h }
        };
        // Ritytans bakgrund.
        self.shape(Path::rect(inner.x, inner.y, inner.w, inner.h), inner, c.plot_sppr, None, None);

        let val_rev = val_ax.is_some_and(|a| a.reversed);
        let cat_rev = cat_ax.is_some_and(|a| a.reversed);
        // Värde → koordinat längs värdeaxeln.
        let vpos = move |v: f64| -> f32 {
            let t = ((v - scale.min) / (scale.max - scale.min)) as f32;
            let t = if val_rev { 1.0 - t } else { t };
            if horizontal { inner.x + t * inner.w } else { inner.bottom() - t * inner.h }
        };
        let band = if horizontal { inner.h } else { inner.w } / if between { ncat as f32 } else { (ncat as f32 - 1.0).max(1.0) };
        // Kategori i → mitten (eller strecket) längs kategoriaxeln.
        let cpos = move |i: f32| -> f32 {
            let k = if between { i + 0.5 } else { i };
            if horizontal {
                // Liggande staplar: första kategorin längst ner.
                let y = inner.bottom() - k * band;
                if cat_rev { inner.y + k * band } else { y }
            } else if cat_rev {
                inner.right() - k * band
            } else {
                inner.x + k * band
            }
        };
        let xpos = move |v: f64| -> f32 {
            match x_scale {
                Some(xs) => inner.x + ((v - xs.min) / (xs.max - xs.min)) as f32 * inner.w,
                None => cpos(v as f32 - 1.0),
            }
        };

        // Rutnät.
        let grid_def = Stroke { paint: Paint::Solid(lum(self.scheme("tx1"), 0.15, 0.85)), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Round };
        if let Some(ax) = val_ax {
            for (grid, minor) in [(ax.minor_grid, true), (ax.major_grid, false)] {
                let Some(sp) = grid else { continue };
                let step = if minor { scale.step / 5.0 } else { scale.step };
                let n = ((scale.max - scale.min) / step).round().clamp(0.0, 2000.0) as usize;
                let st = self.line_of(sp, inner).unwrap_or(Some(grid_def.clone()));
                let Some(st) = st else { continue };
                let mut path = Path::default();
                for k in 0..=n {
                    let p = vpos(scale.min + step * k as f64);
                    if horizontal {
                        path.move_to(p, inner.y);
                        path.line_to(p, inner.bottom());
                    } else {
                        path.move_to(inner.x, p);
                        path.line_to(inner.right(), p);
                    }
                }
                self.stroke(path, st);
            }
        }
        if let Some(ax) = cat_ax {
            if let Some(sp) = ax.major_grid {
                if let Some(st) = self.line_of(sp, inner).unwrap_or(Some(grid_def.clone())) {
                    let mut path = Path::default();
                    if let Some(xs) = x_scale {
                        for v in xs.ticks() {
                            let x = xpos(v);
                            path.move_to(x, inner.y);
                            path.line_to(x, inner.bottom());
                        }
                    } else {
                        let n = if between { ncat } else { ncat.saturating_sub(1) };
                        for k in 0..=n {
                            let p = if between { cpos(k as f32 - 0.5) } else { cpos(k as f32) };
                            if horizontal {
                                path.move_to(inner.x, p);
                                path.line_to(inner.right(), p);
                            } else {
                                path.move_to(p, inner.y);
                                path.line_to(p, inner.bottom());
                            }
                        }
                    }
                    self.stroke(path, st);
                }
            }
        }

        // Serierna.
        let total_series: usize = c.plots.iter().map(|p| p.series.len()).sum();
        let mut labels: Vec<(Label, (f32, f32, f32, f32), Labels<'n>)> = Vec::new();
        for p in &c.plots {
            let vp: &dyn Fn(f64) -> f32 = if on_sec(p) { &vpos2 } else { &vpos };
            match p.kind {
                Kind::Bar { .. } => self.bars(c, p, ncat, total_series, vp, &cpos, band, horizontal, inner, &mut labels),
                Kind::Line | Kind::Area => self.lines(c, p, ncat, total_series, vp, &cpos, inner, &mut labels),
                Kind::Scatter => self.scatter(c, p, total_series, vp, &xpos, inner, &mut labels),
                Kind::Pie { .. } | Kind::Radar { .. } => {}
            }
            // Trendlinjer och felstaplar.
            if p.series.iter().any(|s| !s.trendlines.is_empty() || !s.err_bars.is_empty()) {
                for (k, ser) in p.series.iter().enumerate() {
                    if ser.trendlines.is_empty() && ser.err_bars.is_empty() {
                        continue;
                    }
                    let sp = series_points(p, ser, k, ncat, band, horizontal, &cpos, &xpos, scatter);
                    let col = self.series_paint(c, p, ser, None, total_series, ncat);
                    let xp: Option<&dyn Fn(f64) -> f32> = if scatter { Some(&xpos) } else { None };
                    self.analysis(ser, &sp, vp, horizontal, inner, col, xp);
                }
            }
        }

        // Axellinjer och etiketter.
        let axis_def = Stroke { paint: Paint::Solid(lum(self.scheme("tx1"), 0.25, 0.75)), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Round };
        let zero = vpos(0.0f64.clamp(scale.min, scale.max));
        if let Some(ax) = cat_ax.filter(|a| !a.deleted) {
            let st = self.line_of(ax.sppr, inner).unwrap_or(Some(axis_def.clone()));
            let cross = if ax.crosses == "max" { vpos(scale.max) } else if ax.crosses == "min" { vpos(scale.min) } else { zero };
            if let Some(st) = st {
                let mut path = Path::default();
                if horizontal {
                    path.move_to(cross, inner.y);
                    path.line_to(cross, inner.bottom());
                } else {
                    path.move_to(inner.x, cross);
                    path.line_to(inner.right(), cross);
                }
                self.stroke(path, st);
            }
            if show_cat_lbl {
                if let Some(xs) = x_scale {
                    for v in xs.ticks() {
                        let l = self.lay(Self::plain_paras(&chart_fmt::general(v), &cat_style, Align::Center), f32::INFINITY);
                        let x = xpos(v);
                        self.place(l, x, inner.bottom() + 0.61 * cs, 0.5, 0.0, 0.0);
                    }
                } else {
                    // Etiketter som inte får plats hoppas över (var k:te); datumaxelns `majorUnit` och
                    // `tickLblSkip` anger steget själva.
                    let max_w = band * 0.98;
                    let step = if let Some(k) = date_step.or(cat_ax.and_then(|a| a.tick_skip)) {
                        k.max(1)
                    } else if horizontal {
                        ((cat_line_h / band).ceil() as usize).max(1)
                    } else if let Some(rot) = cat_rot {
                        // Vridna etiketter tar radhöjden / sin(vinkeln) i bredd.
                        let need = cat_line_h / rot.to_radians().sin().abs().max(0.2);
                        ((need / band).ceil() as usize).max(1)
                    } else {
                        let w = cats.iter().map(|t| measure(self, t, &cat_style).0).fold(0.0, f32::max);
                        if w <= max_w { 1 } else { ((w / band).ceil() as usize).max(1) }
                    };
                    for (i, t) in cats.iter().enumerate().step_by(step) {
                        let p = cpos(i as f32);
                        if let (Some(rot), false) = (cat_rot, horizontal) {
                            // Vriden: etikettens slut (moturs) eller början (medurs) vid strecket, texten nedåt.
                            let l = self.lay(Self::plain_paras(t, &cat_style, Align::Right), f32::INFINITY);
                            self.place(l, p, inner.bottom() + 0.3 * cs, if rot < 0.0 { 1.0 } else { 0.0 }, 0.5, rot);
                        } else if horizontal {
                            let l = self.lay(Self::plain_paras(t, &cat_style, Align::Right), f32::INFINITY);
                            self.place(l, inner.x - 0.92 * cs, p - 0.1 * cs, 1.0, 0.5, 0.0);
                        } else {
                            let l = self.lay(Self::plain_paras(t, &cat_style, Align::Center), if step == 1 { band.max(cat_w.min(band)) } else { f32::INFINITY });
                            self.place(l, p, inner.bottom() + 0.61 * cs, 0.5, 0.0, 0.0);
                        }
                    }
                }
            }
        }
        if let Some(ax) = val_ax.filter(|a| !a.deleted) {
            let st = self.line_of(ax.sppr, inner).unwrap_or(Some(axis_def.clone()));
            if let Some(st) = st {
                let mut path = Path::default();
                if horizontal {
                    path.move_to(inner.x, inner.bottom());
                    path.line_to(inner.right(), inner.bottom());
                } else {
                    path.move_to(inner.x, inner.y);
                    path.line_to(inner.x, inner.bottom());
                }
                self.stroke(path, st);
            }
            if show_val_lbl {
                for v in scale.ticks() {
                    let t = val_fmt(v);
                    let p = vpos(v);
                    if horizontal {
                        let l = self.lay(Self::plain_paras(&t, &val_style, Align::Center), f32::INFINITY);
                        self.place(l, p, inner.bottom() + 0.61 * vs, 0.5, 0.0, 0.0);
                    } else {
                        let l = self.lay(Self::plain_paras(&t, &val_style, Align::Right), f32::INFINITY);
                        // Etiketten centreras på strecket (baslinjen 0,28 em under det, som i PowerPoint).
                        self.place(l, inner.x - 0.92 * vs, p - 0.1 * vs, 1.0, 0.5, 0.0);
                    }
                }
            }
        }
        if let Some(t) = &dtable {
            self.data_table(c, t, inner, band, &cpos);
        }
        // Den sekundära värdeaxeln.
        if let Some(ax) = sec_ax.filter(|a| !a.deleted) {
            if let Some(st) = self.line_of(ax.sppr, inner).unwrap_or(Some(axis_def.clone())) {
                let mut path = Path::default();
                if horizontal {
                    path.move_to(inner.x, inner.y);
                    path.line_to(inner.right(), inner.y);
                } else {
                    path.move_to(inner.right(), inner.y);
                    path.line_to(inner.right(), inner.bottom());
                }
                self.stroke(path, st);
            }
            if show_sec_lbl {
                let ss = sec_style.size;
                for v in scale2.ticks() {
                    let t = sec_fmt(v);
                    let p = vpos2(v);
                    if horizontal {
                        let l = self.lay(Self::plain_paras(&t, &sec_style, Align::Center), f32::INFINITY);
                        self.place(l, p, inner.y - 0.61 * ss, 0.5, 1.0, 0.0);
                    } else {
                        let l = self.lay(Self::plain_paras(&t, &sec_style, Align::Left), f32::INFINITY);
                        self.place(l, inner.right() + 0.92 * ss, p - 0.1 * ss, 0.0, 0.5, 0.0);
                    }
                }
            }
            if let Some(t) = &ax.title {
                let chain = [t.txpr, c.space_txpr];
                let paras = match (t.rich, &t.text) {
                    (Some(rich), _) => Some(self.rich_paras(rich, &chain, 1000, Align::Center, &|_| None)),
                    (None, Some(txt)) => Some(Self::plain_paras(txt, &self.style(&chain, 1000), Align::Center)),
                    _ => None,
                };
                if let Some(paras) = paras {
                    let l = self.lay(paras, f32::INFINITY);
                    let w2 = scale2.ticks().iter().map(|v| measure(self, &sec_fmt(*v), &sec_style).0).fold(0.0, f32::max);
                    let x = (inner.right() + 0.92 * sec_style.size + w2 + 0.3 * sec_style.size).min(self.f.w - l.h);
                    self.place(l, x, inner.y + inner.h / 2.0, 0.5, 0.0, -90.0);
                }
            }
        }
        // Axelrubriker.
        for ax in [cat_ax, val_ax].into_iter().flatten().filter(|a| !a.deleted) {
            let Some(t) = &ax.title else { continue };
            let chain = [t.txpr, c.space_txpr];
            let paras = match (t.rich, &t.text) {
                (Some(rich), _) => self.rich_paras(rich, &chain, 1000, Align::Center, &|_| None),
                (None, Some(txt)) => Self::plain_paras(txt, &self.style(&chain, 1000), Align::Center),
                _ => continue,
            };
            let l = self.lay(paras, f32::INFINITY);
            let vertical_axis = matches!(ax.pos.as_str(), "l" | "r");
            if vertical_axis {
                let lbl_w = if ax.kind == "valAx" { scale.ticks().iter().map(|v| measure(self, &val_fmt(*v), &val_style).0).fold(0.0, f32::max) } else { cat_w };
                let x = (inner.x - 0.92 * left_s - lbl_w - 0.3 * left_s).max(l.h);
                // Lodrät rubrik (vriden −90°), med underkanten mot etiketterna.
                self.place(l, x, inner.y + inner.h / 2.0, 0.5, 1.0, -90.0);
            } else {
                self.place(l, inner.x + inner.w / 2.0, inner.bottom() + 1.9 * bottom_s, 0.5, 0.0, 0.0);
            }
        }
        // Dataetiketterna överst.
        for (l, (x, y, ax, ay), lbls) in labels {
            self.label_box(l, &lbls, x, y, ax, ay);
        }
    }

    /// Seriens fyllning och linje för stapel/yta/punkt (`dPt` för enskilda punkter).
    fn series_paint(&self, c: &Chart<'_>, p: &Plot<'_>, ser: &Series<'_>, i: Option<usize>, total: usize, n_points: usize) -> Rgba {
        match i {
            Some(i) if p.vary_colors && p.series.len() == 1 => self.auto_color(c.style, i, n_points),
            _ => self.auto_color(c.style, ser.idx, total),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn bars<'n>(
        &mut self,
        c: &Chart<'n>,
        p: &Plot<'n>,
        ncat: usize,
        total: usize,
        vpos: &dyn Fn(f64) -> f32,
        cpos: &dyn Fn(f32) -> f32,
        band: f32,
        horizontal: bool,
        _inner: R,
        labels: &mut Vec<(Label, (f32, f32, f32, f32), Labels<'n>)>,
    ) {
        let m = p.series.len().max(1) as f32;
        let gap = p.gap / 100.0;
        let ov = p.overlap / 100.0;
        let clustered = p.grouping == Grouping::Clustered;
        let w = if clustered { band / (m - (m - 1.0) * ov + gap) } else { band / (1.0 + gap) };
        let group_w = if clustered { m * w - (m - 1.0) * ov * w } else { w };
        let mut pos_acc = vec![0.0f64; ncat];
        let mut neg_acc = vec![0.0f64; ncat];
        let sums: Vec<f64> = (0..ncat).map(|i| p.series.iter().map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0).abs()).sum()).collect();
        for (k, ser) in p.series.iter().enumerate() {
            for i in 0..ncat {
                let Some(v) = ser.vals.get(i).copied().flatten() else { continue };
                let v = if p.grouping == Grouping::Percent { if sums[i] > 0.0 { v / sums[i] } else { 0.0 } } else { v };
                let (a, bv) = if clustered {
                    (0.0, v)
                } else if v >= 0.0 {
                    let a = pos_acc[i];
                    pos_acc[i] += v;
                    (a, a + v)
                } else {
                    let a = neg_acc[i];
                    neg_acc[i] += v;
                    (a, a + v)
                };
                let center = cpos(i as f32);
                // Läge längs kategoriaxeln: gruppen centrerad i bandet.
                let start = if horizontal { center + group_w / 2.0 } else { center - group_w / 2.0 };
                let off = if clustered { k as f32 * w * (1.0 - ov) } else { 0.0 };
                let (p0, p1) = (vpos(a), vpos(bv));
                let r = if horizontal {
                    let y = start - off - w;
                    R { x: p0.min(p1), y, w: (p1 - p0).abs(), h: w }
                } else {
                    R { x: start + off, y: p0.min(p1), w, h: (p1 - p0).abs() }
                };
                let sp = ser.point(i).and_then(|d| d.sppr).or(ser.sppr);
                let col = self.series_paint(c, p, ser, Some(i), total, ncat);
                self.shape(Path::rect(r.x, r.y, r.w, r.h), r, sp, Some(Paint::Solid(col)), None);
                let l = ser.labels_for(i);
                if l.any() {
                    let pct = if sums[i] > 0.0 { Some(v.abs() / sums[i]) } else { None };
                    if let Some(lbl) = self.data_label(c, p, ser, i, l, pct, false) {
                        let pos = l.pos.as_deref().unwrap_or(if clustered { "outEnd" } else { "ctr" });
                        let neg = bv < a;
                        let anchor = match (pos, horizontal) {
                            ("ctr", _) => (r.x + r.w / 2.0, r.y + r.h / 2.0, 0.5, 0.5),
                            ("inBase", false) => (r.x + r.w / 2.0, if neg { r.y + 2.0 } else { r.bottom() - 2.0 }, 0.5, if neg { 0.0 } else { 1.0 }),
                            ("inEnd", false) => (r.x + r.w / 2.0, if neg { r.bottom() - 2.0 } else { r.y + 2.0 }, 0.5, if neg { 1.0 } else { 0.0 }),
                            (_, false) => (r.x + r.w / 2.0, if neg { r.bottom() + 2.0 } else { r.y - 2.0 }, 0.5, if neg { 0.0 } else { 1.0 }),
                            ("inBase", true) => (if neg { r.right() - 2.0 } else { r.x + 2.0 }, r.y + r.h / 2.0, if neg { 1.0 } else { 0.0 }, 0.5),
                            ("inEnd", true) => (if neg { r.x + 2.0 } else { r.right() - 2.0 }, r.y + r.h / 2.0, if neg { 0.0 } else { 1.0 }, 0.5),
                            (_, true) => (if neg { r.x - 2.0 } else { r.right() + 2.0 }, r.y + r.h / 2.0, if neg { 1.0 } else { 0.0 }, 0.5),
                        };
                        labels.push((lbl, anchor, l.clone()));
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lines<'n>(
        &mut self,
        c: &Chart<'n>,
        p: &Plot<'n>,
        ncat: usize,
        total: usize,
        vpos: &dyn Fn(f64) -> f32,
        cpos: &dyn Fn(f32) -> f32,
        inner: R,
        labels: &mut Vec<(Label, (f32, f32, f32, f32), Labels<'n>)>,
    ) {
        let area = p.kind == Kind::Area;
        let sums: Vec<f64> = (0..ncat).map(|i| p.series.iter().map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0)).sum()).collect();
        let mut acc = vec![0.0f64; ncat];
        let mut drawn: Vec<(Vec<Option<(f32, f32)>>, Vec<(f32, f32)>)> = Vec::new();
        for ser in &p.series {
            let base: Vec<f64> = acc.clone();
            let mut pts: Vec<Option<(f32, f32)>> = Vec::with_capacity(ncat);
            let mut tops = Vec::with_capacity(ncat);
            for i in 0..ncat {
                let raw = ser.vals.get(i).copied().flatten();
                let v = raw.map(|v| if p.grouping == Grouping::Percent { if sums[i] != 0.0 { v / sums[i] } else { 0.0 } } else { v });
                let top = match p.grouping {
                    Grouping::Clustered => v.unwrap_or(0.0),
                    _ => base[i] + v.unwrap_or(0.0),
                };
                if p.grouping != Grouping::Clustered {
                    acc[i] = top;
                }
                tops.push(top);
                pts.push(raw.map(|_| (cpos(i as f32), vpos(top))));
            }
            if area {
                // Ytan mellan föregående seriens kurva (staplat) eller nollan och den här.
                let mut path = Path::default();
                for i in 0..ncat {
                    let (x, y) = (cpos(i as f32), vpos(tops[i]));
                    if i == 0 { path.move_to(x, y) } else { path.line_to(x, y) }
                }
                for i in (0..ncat).rev() {
                    let b0 = if p.grouping == Grouping::Clustered { 0.0 } else { base[i] };
                    path.line_to(cpos(i as f32), vpos(b0));
                }
                path.close();
                let col = self.auto_color(c.style, ser.idx, total);
                self.shape(path, inner, ser.sppr, Some(Paint::Solid(col)), None);
            } else {
                let col = self.auto_color(c.style, ser.idx, total);
                let def = Stroke { paint: Paint::Solid(col), width: 2.25, dash: Vec::new(), cap: Cap::Round, join: Join::Round };
                let st = self.line_of(ser.sppr, inner).unwrap_or_else(|| {
                    let w = ser.sppr.and_then(|s| s.child("a:ln")).and_then(|l| l.attr_i64("w")).map(emu);
                    Some(Stroke { width: w.unwrap_or(2.25), ..def })
                });
                if let Some(st) = st {
                    // `dispBlanksAs span` (och datumaxelns dagar utan värde): linjen går över luckorna.
                    let path = if c.blanks == "span" { polyline(&pts.iter().filter(|p| p.is_some()).copied().collect::<Vec<_>>(), ser.smooth) } else { polyline(&pts, ser.smooth) };
                    self.stroke(path, st);
                }
            }
            let mids: Vec<(f32, f32)> = (0..ncat)
                .map(|i| {
                    let b0 = if p.grouping == Grouping::Clustered || !area { tops[i] } else { (tops[i] + base[i]) / 2.0 };
                    (cpos(i as f32), vpos(b0))
                })
                .collect();
            drawn.push((pts, mids));
        }
        // Markörer och etiketter ovanpå alla linjer.
        for (ser, (pts, mids)) in p.series.iter().zip(drawn) {
            if !area {
                let col = self.auto_color(c.style, ser.idx, total);
                self.markers(p, ser, &pts, col, total);
            }
            for i in 0..ncat {
                let l = ser.labels_for(i);
                if !l.any() || pts[i].is_none() {
                    continue;
                }
                let pct = if sums[i] != 0.0 { ser.vals.get(i).copied().flatten().map(|v| v / sums[i]) } else { None };
                let Some(lbl) = self.data_label(c, p, ser, i, l, pct, false) else { continue };
                let (x, y) = if area { mids[i] } else { pts[i].unwrap_or(mids[i]) };
                let anchor = match l.pos.as_deref().unwrap_or(if area { "ctr" } else { "r" }) {
                    "t" => (x, y - 4.0, 0.5, 1.0),
                    "b" => (x, y + 4.0, 0.5, 0.0),
                    "l" => (x - 4.0, y, 1.0, 0.5),
                    "ctr" => (x, y, 0.5, 0.5),
                    _ => (x + 4.0, y, 0.0, 0.5),
                };
                labels.push((lbl, anchor, l.clone()));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn scatter<'n>(
        &mut self,
        c: &Chart<'n>,
        p: &Plot<'n>,
        total: usize,
        vpos: &dyn Fn(f64) -> f32,
        xpos: &dyn Fn(f64) -> f32,
        inner: R,
        labels: &mut Vec<(Label, (f32, f32, f32, f32), Labels<'n>)>,
    ) {
        for ser in &p.series {
            let pts: Vec<Option<(f32, f32)>> = ser
                .vals
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let x = ser.xvals.get(i).copied().flatten().unwrap_or(i as f64 + 1.0);
                    v.map(|v| (xpos(x), vpos(v)))
                })
                .collect();
            let col = self.auto_color(c.style, ser.idx, total);
            let def = Stroke { paint: Paint::Solid(col), width: 2.25, dash: Vec::new(), cap: Cap::Round, join: Join::Round };
            if let Some(st) = self.line_of(ser.sppr, inner).unwrap_or(Some(def)) {
                self.stroke(polyline(&pts, ser.smooth), st);
            }
            self.markers(p, ser, &pts, col, total);
            for (i, pt) in pts.iter().enumerate() {
                let l = ser.labels_for(i);
                let Some((x, y)) = pt else { continue };
                if !l.any() {
                    continue;
                }
                if let Some(lbl) = self.data_label(c, p, ser, i, l, None, false) {
                    labels.push((lbl, (x + 4.0, *y, 0.0, 0.5), l.clone()));
                }
            }
        }
    }

    fn markers(&mut self, p: &Plot<'_>, ser: &Series<'_>, pts: &[Option<(f32, f32)>], col: Rgba, _total: usize) {
        let sym = ser.marker.as_ref().map_or("auto", |m| m.symbol.as_str());
        if sym == "none" || (!p.markers && ser.marker.is_none()) {
            return;
        }
        let sym = if sym == "auto" { ["diamond", "square", "triangle", "x", "star", "circle", "plus", "dash", "dot"][ser.idx % 9] } else { sym };
        let size = ser.marker.as_ref().map_or(5.0, |m| m.size);
        let msp = ser.marker.as_ref().and_then(|m| m.sppr);
        for &(x, y) in pts.iter().flatten() {
            let h = size / 2.0;
            let r = R { x: x - h, y: y - h, w: size, h: size };
            let path = marker_path(sym, x, y, h);
            let open = matches!(sym, "x" | "plus" | "star" | "dash");
            let def_line = Some(Stroke { paint: Paint::Solid(col), width: 0.75, dash: Vec::new(), cap: Cap::Butt, join: Join::Round });
            self.shape(path, r, msp, if open { None } else { Some(Paint::Solid(col)) }, def_line);
        }
    }

    /// Texten i en dataetikett (värde, kategori, serie, procent) utlagd, eller `None`.
    #[allow(clippy::too_many_arguments)]
    fn data_label(&self, c: &Chart<'_>, _p: &Plot<'_>, ser: &Series<'_>, i: usize, l: &Labels<'_>, pct: Option<f64>, pie: bool) -> Option<Label> {
        let chain = [l.txpr, c.space_txpr];
        let v = ser.vals.get(i).copied().flatten();
        let val_text = || v.map(|v| chart_fmt::format(v, l.num_fmt.as_deref().unwrap_or(&ser.format))).unwrap_or_default();
        let pct_text = || pct.map(|p| chart_fmt::format(p, l.num_fmt.as_deref().filter(|f| f.contains('%')).unwrap_or("0%"))).unwrap_or_default();
        let cat_text = || ser.cats.get(i).cloned().unwrap_or_default();
        if let Some(rich) = l.rich {
            let fields = |t: &str| -> Option<String> {
                Some(match t {
                    "VALUE" | "YVALUE" => val_text(),
                    "CATEGORYNAME" | "XVALUE" => cat_text(),
                    "SERIESNAME" => ser.name.clone(),
                    "PERCENTAGE" => pct_text(),
                    _ => return None,
                })
            };
            let paras = self.rich_paras(rich, &chain, 1000, Align::Center, &fields);
            return Some(self.lay(paras, self.f.w * 0.3));
        }
        let mut parts = Vec::new();
        if l.show_ser {
            parts.push(ser.name.clone());
        }
        if l.show_cat {
            parts.push(cat_text());
        }
        if l.show_val {
            parts.push(val_text());
        }
        if l.show_pct && pct.is_some() {
            parts.push(pct_text());
        }
        if parts.is_empty() || v.is_none() {
            return None;
        }
        let sep = l.separator.clone().unwrap_or_else(|| if pie { "\n".into() } else { ", ".into() });
        let st = self.style(&chain, 1000);
        Some(self.lay(Self::plain_paras(&parts.join(&sep), &st, Align::Center), f32::INFINITY))
    }

    /// Ritar en etikett med sin ruta (`spPr` på etiketterna) runt.
    fn label_box(&mut self, l: Label, lb: &Labels<'_>, x: f32, y: f32, ax: f32, ay: f32) {
        let (pad_x, pad_y) = if lb.sppr.is_some() { (3.0, 1.5) } else { (0.0, 0.0) };
        let r = R { x: x - (l.w + 2.0 * pad_x) * ax, y: y - (l.h + 2.0 * pad_y) * ay, w: l.w + 2.0 * pad_x, h: l.h + 2.0 * pad_y };
        if lb.sppr.is_some() {
            self.shape(Path::rect(r.x, r.y, r.w, r.h), r, lb.sppr, None, None);
        }
        self.place(l, r.x + pad_x, r.y + pad_y, 0.0, 0.0, 0.0);
    }
}

/// Linje med fyllning ur en `a:ln`-nod (bredd i punkter).
/// Värdeområdet (minsta, största, 100 %) för plottarna, staplade summerade.
fn value_range<'a, 'n: 'a>(plots: impl Iterator<Item = &'a Plot<'n>>, ncat: usize) -> (f64, f64, bool) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut percent = false;
    for p in plots {
        let stacked = p.grouping != Grouping::Clustered && !matches!(p.kind, Kind::Scatter);
        if p.grouping == Grouping::Percent {
            percent = true;
            lo = lo.min(0.0);
            hi = hi.max(1.0);
        } else if stacked {
            for i in 0..ncat {
                let (mut pos, mut neg) = (0.0, 0.0);
                for s in &p.series {
                    let v = s.vals.get(i).copied().flatten().unwrap_or(0.0);
                    if v >= 0.0 { pos += v } else { neg += v }
                }
                lo = lo.min(neg);
                hi = hi.max(pos);
            }
        } else {
            for v in p.series.iter().flat_map(|s| s.vals.iter().flatten()) {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
        }
    }
    if lo > hi {
        (lo, hi) = (0.0, 1.0);
    }
    (lo, hi, percent)
}

/// Värdeområdet utvidgat med felstaplarna i värdeled (ogrupperade serier).
fn error_extent<'a, 'n: 'a>(plots: impl Iterator<Item = &'a Plot<'n>>, (mut lo, mut hi): (f64, f64)) -> (f64, f64) {
    for p in plots.filter(|p| p.grouping == Grouping::Clustered) {
        for ser in &p.series {
            let all: Vec<f64> = ser.vals.iter().flatten().copied().collect();
            for e in ser.err_bars.iter().filter(|e| !e.dir_x) {
                for (i, v) in ser.vals.iter().enumerate() {
                    let Some(v) = *v else { continue };
                    let (a, b, center) = analysis::error_amounts(e, i, v, &all);
                    let c = center.unwrap_or(v);
                    lo = lo.min(c - a);
                    hi = hi.max(c + b);
                }
            }
        }
    }
    (lo, hi)
}

/// En series punkter för trendlinjer och felstaplar (samma lägen som när serien ritas: stapelns mitt,
/// staplade värden ovanpå varandra).
#[allow(clippy::too_many_arguments)]
fn series_points(p: &Plot<'_>, ser: &Series<'_>, k: usize, ncat: usize, band: f32, horizontal: bool, cpos: &dyn Fn(f32) -> f32, xpos: &dyn Fn(f64) -> f32, scatter: bool) -> analysis::SeriesPoints {
    if scatter {
        let pts = ser.vals.iter().enumerate().filter_map(|(i, v)| {
            let x = ser.xvals.get(i).copied().flatten().unwrap_or(i as f64 + 1.0);
            Some((x, (*v)?, xpos(x)))
        });
        let x0 = xpos(0.0);
        return analysis::SeriesPoints { pts: pts.collect(), x0, dx: xpos(1.0) - x0 };
    }
    // Stapelns förskjutning från kategorins mitt.
    let slot = match p.kind {
        Kind::Bar { .. } if p.grouping == Grouping::Clustered => {
            let m = p.series.len().max(1) as f32;
            let (gap, ov) = (p.gap / 100.0, p.overlap / 100.0);
            let w = band / (m - (m - 1.0) * ov + gap);
            let group_w = m * w - (m - 1.0) * ov * w;
            let off = k as f32 * w * (1.0 - ov) + w / 2.0 - group_w / 2.0;
            if horizontal { -off } else { off }
        }
        _ => 0.0,
    };
    let sums: Vec<f64> = (0..ncat).map(|i| p.series.iter().map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0)).sum()).collect();
    let pts = (0..ncat).filter_map(|i| {
        let v = ser.vals.get(i).copied().flatten()?;
        let v = match p.grouping {
            Grouping::Clustered => v,
            // Staplat: värdet ovanpå de föregående seriernas.
            g => {
                let below: f64 = p.series[..k].iter().map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0)).sum();
                let t = below + v;
                if g == Grouping::Percent && sums[i] != 0.0 { t / sums[i] } else { t }
            }
        };
        Some(((i + 1) as f64, v, cpos(i as f32) + slot))
    });
    let x0 = cpos(-1.0) + slot;
    analysis::SeriesPoints { pts: pts.collect(), x0, dx: cpos(0.0) - cpos(-1.0) }
}

fn stroke(paint: Paint, width: f32, ln: &Node) -> Stroke {
    let dash: Vec<f32> = match ln.child("a:prstDash").and_then(|d| d.attr("val")).unwrap_or("solid") {
        "dot" | "sysDot" => vec![1.0, 1.0],
        "dash" | "sysDash" => vec![3.0, 1.0],
        "lgDash" => vec![8.0, 3.0],
        "dashDot" | "sysDashDot" => vec![4.0, 3.0, 1.0, 3.0],
        "lgDashDot" => vec![8.0, 3.0, 1.0, 3.0],
        _ => Vec::new(),
    }
    .into_iter()
    .map(|v| v * width.max(0.5))
    .collect();
    let cap = match ln.attr("cap") {
        Some("rnd") => Cap::Round,
        Some("sq") => Cap::Square,
        _ => Cap::Butt,
    };
    let join = if ln.child("a:miter").is_some() {
        Join::Miter
    } else if ln.child("a:bevel").is_some() {
        Join::Bevel
    } else {
        Join::Round
    };
    Stroke { paint, width: if width <= 0.0 { 0.5 } else { width }, dash, cap, join }
}

/// Ljusheten i HSL: l · `m` + `off` (som `lumMod`/`lumOff`).
fn lum(c: Rgba, m: f32, off: f32) -> Rgba {
    let (h, s, l) = color::rgb_to_hsl(c.r as f64, c.g as f64, c.b as f64);
    let l = (l * m as f64 + off as f64).clamp(0.0, 1.0);
    let (r, g, b) = color::hsl_to_rgb(h, s, l);
    Rgba { r: r as f32, g: g as f32, b: b as f32, a: c.a }
}

/// Bredden av utlagd text (vänsterjusterad, utan insättningar).
fn text_width(items: &[Item]) -> f32 {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Glyphs(g) => g.glyphs.iter().map(|p| g.t.e + p.x + p.advance).fold(None, |a: Option<f32>, b| Some(a.map_or(b, |a| a.max(b)))),
            _ => None,
        })
        .fold(0.0, f32::max)
}

/// Polylinje genom punkterna (hål där värden saknas); `smooth` ger en Catmull-Rom-kurva.
fn polyline(pts: &[Option<(f32, f32)>], smooth: bool) -> Path {
    let mut path = Path::default();
    let mut run: Vec<(f32, f32)> = Vec::new();
    let flush = |run: &mut Vec<(f32, f32)>, path: &mut Path| {
        if run.len() >= 2 {
            path.move_to(run[0].0, run[0].1);
            for k in 1..run.len() {
                if smooth {
                    let p0 = run[k.saturating_sub(2).min(k - 1)];
                    let p0 = if k >= 2 { p0 } else { run[k - 1] };
                    let p1 = run[k - 1];
                    let p2 = run[k];
                    let p3 = if k + 1 < run.len() { run[k + 1] } else { p2 };
                    let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
                    let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
                    path.cubic_to(c1.0, c1.1, c2.0, c2.1, p2.0, p2.1);
                } else {
                    path.line_to(run[k].0, run[k].1);
                }
            }
        }
        run.clear();
    };
    for p in pts {
        match p {
            Some(q) => run.push(*q),
            None => flush(&mut run, &mut path),
        }
    }
    flush(&mut run, &mut path);
    path
}

/// En markörs bana med mitten (x, y) och halva storleken `h`.
fn marker_path(sym: &str, x: f32, y: f32, h: f32) -> Path {
    let mut p = Path::default();
    let poly = |p: &mut Path, pts: &[(f32, f32)]| {
        for (i, &(a, b)) in pts.iter().enumerate() {
            if i == 0 { p.move_to(x + a, y + b) } else { p.line_to(x + a, y + b) }
        }
        p.close();
    };
    match sym {
        "square" => poly(&mut p, &[(-h, -h), (h, -h), (h, h), (-h, h)]),
        "diamond" => poly(&mut p, &[(0.0, -h), (h, 0.0), (0.0, h), (-h, 0.0)]),
        "triangle" => poly(&mut p, &[(0.0, -h), (h, h), (-h, h)]),
        "x" => {
            p.move_to(x - h, y - h);
            p.line_to(x + h, y + h);
            p.move_to(x + h, y - h);
            p.line_to(x - h, y + h);
        }
        "plus" => {
            p.move_to(x - h, y);
            p.line_to(x + h, y);
            p.move_to(x, y - h);
            p.line_to(x, y + h);
        }
        "star" => {
            p.move_to(x - h, y - h);
            p.line_to(x + h, y + h);
            p.move_to(x + h, y - h);
            p.line_to(x - h, y + h);
            p.move_to(x, y - h);
            p.line_to(x, y + h);
        }
        "dash" => {
            p.move_to(x - h, y);
            p.line_to(x + h, y);
        }
        "dot" => p = super::render::ellipse(x, y, h * 0.5, h * 0.5),
        _ => p = super::render::ellipse(x, y, h, h),
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertical(len: f32, s: f32) -> impl Fn(f64, f64, f64) -> bool {
        move |mn, mx, step| (len as f64) / ((mx - mn) / step) >= 1.5 * s as f64 * 0.98
    }

    #[test]
    fn automatisk_skala_som_powerpoint() {
        // Uppmätt i PowerPoints PDF för docx4j:s p4-diagram (rityta ≈ 185 pt hög, 12 pt text).
        assert_eq!(auto_scale(1.8, 5.0, None, None, None, &vertical(184.6, 12.0)), Scale { min: 0.0, max: 6.0, step: 1.0 });
        assert_eq!(auto_scale(0.0, 12.3, None, None, None, &vertical(184.6, 12.0)), Scale { min: 0.0, max: 14.0, step: 2.0 });
        let s = auto_scale(0.0, 1.0, Some(0.0), Some(1.0), None, &vertical(184.6, 12.0));
        assert_eq!((s.min, s.max), (0.0, 1.0));
        assert!((s.step - 0.1).abs() < 1e-9);
        // Negativa värden: nollan och marginal nedåt.
        let s = auto_scale(-3.0, 7.0, None, None, None, &vertical(200.0, 10.0));
        assert!(s.min <= -3.0 && s.max >= 7.0 && s.ticks().contains(&0.0));
    }

    #[test]
    fn talformat_och_polylinje_med_hal() {
        let p = polyline(&[Some((0.0, 0.0)), Some((1.0, 1.0)), None, Some((3.0, 3.0)), Some((4.0, 4.0))], false);
        let moves = p.cmds.iter().filter(|c| matches!(c, super::super::scene::Cmd::Move(..))).count();
        assert_eq!(moves, 2);
        let c = lum(Rgba::rgb8(0, 0, 0), 0.15, 0.85);
        assert!((c.r - 0.85).abs() < 0.01);
    }

    /// Jämför textens lägen med PowerPoints PDF-export av docx4j:s `p4-*`-diagram (Apache-2.0):
    /// `KLADD_CHART_REFS=…/charts/render/office cargo test chart_draw::tests::docx4j_referens -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn docx4j_referens() {
        let Some(dir) = std::env::var_os("KLADD_CHART_REFS").map(std::path::PathBuf::from) else { return };
        let mut all = Vec::new();
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pptx")).collect();
        names.sort();
        for path in names {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let Ok(js) = std::fs::read_to_string(dir.join("powerpoint-pdf").join(format!("{stem}.json"))) else { continue };
            let v: serde_json::Value = serde_json::from_str(&js).unwrap();
            let bx = v["box"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect::<Vec<_>>();
            let deck = super::super::Deck::open(&std::fs::read(&path).unwrap()).unwrap();
            let scene = super::super::render::slide_scene(&deck, 0);
            let runs: Vec<(String, f32, f32, f32)> = scene
                .items
                .iter()
                .filter_map(|i| match i {
                    Item::Glyphs(g) => {
                        let (x, y) = g.t.apply(g.glyphs[0].x, 0.0);
                        Some((g.text.trim().to_owned(), x - bx[0], y - bx[1], g.size))
                    }
                    _ => None,
                })
                .collect();
            println!("== {stem}");
            for t in v["texts"].as_array().unwrap() {
                let text = t["text"].as_str().unwrap().trim();
                let (rx, ry) = (t["x"].as_f64().unwrap() as f32, t["y"].as_f64().unwrap() as f32);
                match runs.iter().filter(|r| r.0 == text).min_by(|a, b| ((a.1 - rx).abs() + (a.2 - ry).abs()).total_cmp(&((b.1 - rx).abs() + (b.2 - ry).abs()))) {
                    Some(r) => {
                        println!("  {:<14} pp ({:6.1},{:6.1}) kladd ({:6.1},{:6.1})  dx {:+5.1} dy {:+5.1}", text, rx, ry, r.1, r.2, r.1 - rx, r.2 - ry);
                        all.push(((r.1 - rx).abs(), (r.2 - ry).abs()));
                    }
                    None => println!("  {:<14} pp ({:6.1},{:6.1}) SAKNAS", text, rx, ry),
                }
            }
        }
        let n = all.len().max(1) as f32;
        println!("medel |dx| {:.2} |dy| {:.2} över {} texter", all.iter().map(|a| a.0).sum::<f32>() / n, all.iter().map(|a| a.1).sum::<f32>() / n, all.len());
    }
}
