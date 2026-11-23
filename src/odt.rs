//! Läser och skriver OpenDocument-text (.odt, LibreOffice och Apache OpenOffice) som Markdown-text.
//!
//! Samma Markdown-dialekt som `docx.rs` (rubriker `#`, `**`, `*`, `~~`, `<u>`, `<span style>`,
//! `<div align>`, listor, tabeller, `![alt](bild:namn)` och `<img … width>`), samma grundformat
//! i första raden (`DocDefaults`) och samma bildschema (`bild:`).
//!
//! Läsning: paketet är en zip med `content.xml`, `styles.xml`, `meta.xml` och `Pictures/`. XML
//! läses till ett litet träd. Formatmallar (`office:styles` och `office:automatic-styles` i båda
//! filerna) löses upp längs `style:parent-style-name`, och bara det som skiljer sig från
//! dokumentets grundformat (standardstycket + Standard) skrivs som `<span style>`.
//!
//! Beslut om saker Kladd inte kan visa:
//! - Fotnoter (`text:note`) blir `[^N]` och en definition `[^N]: text` sist i dokumentet; slutnoter
//!   (`text:note-class="endnote"`) blir `[^ei]`, `[^eii]` … (egen numrering, definitionerna efter fotnoternas).
//! - Spårade ändringar och kommentarer läses som i .docx (`crate::review`): `text:insertion` (mellan
//!   `text:change-start` och `text:change-end`) blir `<ins>`, `text:deletion` (platsen markeras av
//!   `text:change`, den raderade texten ligger i `text:changed-region`) blir `<del>`, och
//!   `office:annotation` blir ett kommentarsområde (`office:annotation-end` ger slutet, `loext:parent-name`
//!   ett svar, `loext:resolved` löst). Författare och datum hamnar i postraderna. Formatändringar
//!   (`text:format-change`) rapporteras som borttagna.
//! - Diagram, former och inbäddade objekt (`draw:*` utom bilder) rapporteras som borttagna.
//! - Länkade bilder (utanför paketet) och bildformat som Kladd inte kan visa (svg, wmf …) likaså.
//! - Fält i sidhuvud/sidfot blir `{sida}`, `{sidor}`, `{datum}`, `{titel}` och `{fil}`, flikar delar i
//!   vänster|mitten|höger och `style:header-first`/`footer-first` blir första sidans variant; bara standardsidans
//!   (första huvudsidans) vanliga sidhuvud och sidfot läses.
//!
//! Skrivning: ett nytt, giltigt paket med `mimetype` först och okomprimerad, manifest, innehåll,
//! formatmallar (Heading_20_1–6, Quotations, Preformatted_20_Text, listformat, sidformat och
//! huvudsida med sidhuvud/sidfot), metadata och bilder i `Pictures/`.

use std::collections::{BTreeSet, HashMap};
use std::io::{Cursor, Read, Write};

use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::docx::{escape_inline, escape_line_start, image_ext, is_supported_image, wrap, Image, Loaded, Written, IMAGE_SCHEME};
use crate::review::{self, CommentRec, RevKind, RevRec};
use crate::word::sections;
use crate::style::titles::{self, TitleKind};
use crate::style::{self, Align, DocDefaults, SpanStyle};
use crate::word::anchor::{Anchor, HRel, Pos, VRel, Wrap};
use crate::word::markup::{self, Float};

const MIME: &str = "application/vnd.oasis.opendocument.text";
const MAX_ENTRIES: usize = 20_000;
const MAX_PART_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 512 * 1024 * 1024;
const MAX_DEPTH: usize = 256;

// ============================================================== XML-träd

#[derive(Clone)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Kid>,
}

#[derive(Clone)]
enum Kid {
    El(Node),
    Text(String),
}

impl Node {
    fn new(name: String) -> Node {
        Node { name, attrs: Vec::new(), kids: Vec::new() }
    }

    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn els(&self) -> impl Iterator<Item = &Node> {
        self.kids.iter().filter_map(|k| if let Kid::El(n) = k { Some(n) } else { None })
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.els().find(|n| n.name == name)
    }

    /// All text under noden, utan hänsyn till formatering.
    fn deep_text(&self) -> String {
        let mut s = String::new();
        for k in &self.kids {
            match k {
                Kid::Text(t) => s.push_str(t),
                Kid::El(n) => s.push_str(&n.deep_text()),
            }
        }
        s
    }
}

fn text_of(ev: &Event<'_>) -> String {
    match ev {
        Event::Text(t) => t.xml10_content().map(|c| c.into_owned()).unwrap_or_default(),
        Event::CData(t) => String::from_utf8_lossy(t).into_owned(),
        Event::GeneralRef(r) => {
            if let Ok(Some(c)) = r.resolve_char_ref() {
                return c.to_string();
            }
            match r.decode().as_deref() {
                Ok("amp") => "&".into(),
                Ok("lt") => "<".into(),
                Ok("gt") => ">".into(),
                Ok("quot") => "\"".into(),
                Ok("apos") => "'".into(),
                _ => String::new(),
            }
        }
        _ => String::new(),
    }
}

fn node_of(e: &BytesStart<'_>) -> Node {
    let mut n = Node::new(String::from_utf8_lossy(e.name().as_ref()).into_owned());
    for a in e.attributes().flatten() {
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let val = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
        n.attrs.push((key, val));
    }
    n
}

fn parse_xml(xml: &str) -> Result<Node, String> {
    let mut r = Reader::from_str(xml);
    let mut stack: Vec<Node> = vec![Node::new("#rot".into())];
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                if stack.len() > MAX_DEPTH {
                    return Err(crate::i18n::t("OpenDocument-filen är för djupt nästlad för att öppnas.").into());
                }
                stack.push(node_of(&e));
            }
            Ok(Event::Empty(e)) => {
                let n = node_of(&e);
                stack.last_mut().unwrap().kids.push(Kid::El(n));
            }
            Ok(Event::End(_)) => {
                if stack.len() > 1 {
                    let n = stack.pop().unwrap();
                    stack.last_mut().unwrap().kids.push(Kid::El(n));
                }
            }
            Ok(ev @ (Event::Text(_) | Event::CData(_) | Event::GeneralRef(_))) => {
                let t = text_of(&ev);
                if t.is_empty() {
                    continue;
                }
                let top = stack.last_mut().unwrap();
                match top.kids.last_mut() {
                    Some(Kid::Text(prev)) => prev.push_str(&t),
                    _ => top.kids.push(Kid::Text(t)),
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(crate::tf!("OpenDocument-filen är skadad: {e}", e = e)),
            _ => {}
        }
    }
    while stack.len() > 1 {
        let n = stack.pop().unwrap();
        stack.last_mut().unwrap().kids.push(Kid::El(n));
    }
    let root = stack.pop().unwrap();
    root.kids
        .into_iter()
        .find_map(|k| if let Kid::El(n) = k { Some(n) } else { None })
        .ok_or_else(|| "OpenDocument-filen saknar innehåll.".to_owned())
}

// ============================================================== zip

struct Pkg<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    total: u64,
}

impl Pkg<'_> {
    fn bytes(&mut self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let name = name.trim_start_matches("./").trim_start_matches('/');
        let real = if self.zip.index_for_name(name).is_some() {
            name.to_owned()
        } else {
            match self.zip.file_names().find(|n| n.eq_ignore_ascii_case(name)) {
                Some(n) => n.to_owned(),
                None => return Ok(None),
            }
        };
        let room = MAX_PART_BYTES.min(MAX_TOTAL_BYTES.saturating_sub(self.total));
        let Ok(mut file) = self.zip.by_name(&real) else { return Ok(None) };
        if file.size() > room {
            return Err(crate::i18n::t("En del av dokumentet är för stor för att öppnas.").into());
        }
        let mut buf = Vec::with_capacity(file.size() as usize);
        let res = file.by_ref().take(room + 1).read_to_end(&mut buf);
        self.total += buf.len() as u64;
        res.map_err(|e| crate::tf!("OpenDocument-filen är skadad: {e}", e = e))?;
        if buf.len() as u64 > room {
            return Err(crate::i18n::t("En del av dokumentet är för stor för att öppnas.").into());
        }
        Ok(Some(buf))
    }

    fn text(&mut self, name: &str) -> Result<Option<String>, String> {
        Ok(self.bytes(name)?.map(|b| {
            let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b);
            String::from_utf8_lossy(b).into_owned()
        }))
    }
}

// ============================================================== formatmallar

#[derive(Clone, Default, Debug)]
struct TProps {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    font: Option<String>,
    size: Option<f32>,
    /// Relativ storlek (`130%`) som ännu inte har något att räknas mot.
    size_pct: Option<f32>,
    color: Option<[u8; 3]>,
    /// `Some(None)` = uttryckligen genomskinlig.
    bg: Option<Option<[u8; 3]>>,
}

impl TProps {
    /// `inner` ovanpå `self`.
    fn over(&self, inner: &TProps) -> TProps {
        let (size, size_pct) = if inner.size.is_some() {
            (inner.size, None)
        } else if let Some(p) = inner.size_pct {
            match self.size {
                Some(s) => (Some(s * p / 100.0), None),
                None => (None, Some(p)),
            }
        } else {
            (self.size, self.size_pct)
        };
        TProps {
            bold: inner.bold.or(self.bold),
            italic: inner.italic.or(self.italic),
            underline: inner.underline.or(self.underline),
            strike: inner.strike.or(self.strike),
            font: inner.font.clone().or_else(|| self.font.clone()),
            size,
            size_pct,
            color: inner.color.or(self.color),
            bg: inner.bg.or(self.bg),
        }
    }

    fn bg(&self) -> Option<[u8; 3]> {
        self.bg.flatten()
    }
}

/// Styckeegenskaper som formatmallarna (`style::StyleSet`) använder.
#[derive(Clone, Default, Debug)]
struct PProps {
    before: Option<f32>,
    after: Option<f32>,
    /// `fo:line-height="115%"` som 1,15.
    line: Option<f32>,
    indent: Option<f32>,
}

#[derive(Clone, Default)]
struct GfxPlace {
    hrel: Option<String>,
    vpos: Option<String>,
    vrel: Option<String>,
    run_through: Option<String>,
    clip: Option<String>,
    margin: [Option<f32>; 4],
    /// Fyllning (`draw:fill-color` när `draw:fill="solid"`), kant (`fo:border`) och luft (`fo:padding`) för textrutor.
    fill: Option<String>,
    border: Option<String>,
    pad: Option<f32>,
}

/// Längd med tecken (`svg:x` kan vara negativ) i pt.
fn signed_length(v: &str) -> Option<f32> {
    let t = v.trim();
    match t.strip_prefix('-') {
        Some(r) => style::parse_length(r).map(|x| -x),
        None => style::parse_length(t),
    }
}

#[derive(Clone, Default)]
struct StyleDef {
    name: String,
    display: String,
    parent: Option<String>,
    auto: bool,
    tp: TProps,
    align: Option<Align>,
    /// Luft, radavstånd och indrag i `style:paragraph-properties` (för formatmallarna).
    pp: PProps,
    /// `style:wrap` och `style:horizontal-pos` i `style:graphic-properties`.
    wrap: Option<String>,
    hpos: Option<String>,
    /// Resten av ramens placering: `style:horizontal-rel`, `style:vertical-pos`, `style:vertical-rel`,
    /// `style:run-through`, `fo:clip` och marginalerna (ovan, höger, under, vänster; pt).
    gfx: GfxPlace,
    /// `style:master-page-name`: stycket börjar en ny sida med den sidmallen (avsnitt).
    master: Option<String>,
    /// Spalter i ett avsnittsformat (`style:columns`).
    cols: Option<ColsDef>,
    /// `fo:break-before` och `fo:break-after` (`page` = true, `auto` = false) i styckeformatet.
    break_before: Option<bool>,
    break_after: Option<bool>,
    /// `style:page-number`: sidnumret börjar om på detta tal (`auto` = fortsätter).
    page_number: Option<u32>,
    /// Tabbstoppens typer (`left`, `center`, `right`) och ramlinjerna över/under stycket (för sidhuvud och sidfot).
    tabs: Option<Vec<String>>,
    border_top: Option<String>,
    border_bottom: Option<String>,
}

#[derive(Default)]
struct Styles {
    /// (familj, namn) → formatmall.
    defs: HashMap<(String, String), StyleDef>,
    /// Familj → standardformat (`style:default-style`).
    defaults: HashMap<String, StyleDef>,
    fonts: HashMap<String, String>,
    /// Listformat → (ordnad, startvärde) per nivå.
    lists: HashMap<String, Vec<(bool, u64)>>,
    /// Kolumnformat (`style:family="table-column"`) → bredd i pt (eller relativ bredd).
    col_widths: HashMap<String, f64>,
}

/// Spalter ur ett `style:columns`: antal (minst två), mellanrum, spaltandelar och skiljelinje.
#[derive(Clone, Debug, Default, PartialEq)]
struct ColsDef {
    n: usize,
    /// Mellanrum i pt.
    gap: f32,
    /// Per spalt `style:rel-width` (en andel av textytan, inklusive utfyllnaden) och utfyllnad
    /// (start + slut) i pt; tom när spalterna saknar egna mått.
    shares: Vec<(f32, f32)>,
    rule: bool,
    /// `text:dont-balance-text-columns="true"` på sektionsstilen.
    no_balance: bool,
}

impl ColsDef {
    /// Spaltbredderna i pt på en textyta `text_w` bred; tom när spalterna är lika breda.
    fn widths(&self, text_w: f32) -> Vec<f32> {
        let sum: f32 = self.shares.iter().map(|s| s.0).sum();
        if self.shares.len() != self.n || sum <= 0.0 {
            return Vec::new();
        }
        // Lika andelar är lika breda spalter (LibreOffice skriver så även om utfyllnaden skiljer mellan
        // kant- och mittspalter).
        if self.shares.iter().all(|(v, _)| (v / sum * self.n as f32 - 1.0).abs() < 0.01) {
            return Vec::new();
        }
        let w: Vec<f32> = self.shares.iter().map(|(v, pad)| v / sum * text_w - pad).collect();
        if w.iter().all(|w| *w > 1.0) && w.iter().any(|x| (x - w[0]).abs() > 0.5) { w } else { Vec::new() }
    }

    /// Egenskaper för ett avsnitt (de som skiljer sig från grundvärdena) på en textyta `text_w` bred.
    fn apply(&self, p: &mut sections::SectionProps, text_w: f32) {
        p.columns = Some(self.n);
        p.column_gap = Some(self.gap).filter(|g| (g - sections::DEFAULT_GAP).abs() > 0.5);
        p.column_widths = Some(self.widths(text_w)).filter(|w| w.len() > 1);
        p.column_rule = self.rule;
        p.no_balance = self.no_balance;
    }
}

/// Spalterna i ett `style:columns`. Utfyllnaden står som `fo:start-padding`/`fo:end-padding` eller
/// (LibreOffice) `fo:start-indent`/`fo:end-indent`; mellanrummet är utfyllnaden mellan första och
/// andra spalten, annars `fo:column-gap`.
fn columns_of(c: &Node) -> Option<ColsDef> {
    let n = c.attr("fo:column-count")?.trim().parse::<usize>().ok()?;
    if n < 2 {
        return None;
    }
    let n = n.min(sections::MAX_COLUMNS);
    let mut gap = c.attr("fo:column-gap").and_then(style::parse_length).unwrap_or(sections::DEFAULT_GAP);
    let cols: Vec<&Node> = c.els().filter(|k| k.name == "style:column").collect();
    let pad = |k: &Node, a: &str| k.attr(a).or_else(|| k.attr(&a.replace("padding", "indent"))).and_then(style::parse_length).unwrap_or(0.0);
    let mut shares = Vec::new();
    if cols.len() == n {
        if let [a, b, ..] = cols[..] {
            let g = pad(a, "fo:end-padding") + pad(b, "fo:start-padding");
            if g > 0.0 {
                gap = g;
            }
        }
        shares = cols
            .iter()
            .map(|k| {
                let v = k.attr("style:rel-width").and_then(|v| v.trim().trim_end_matches('*').parse::<f32>().ok()).unwrap_or(0.0);
                (v, pad(k, "fo:start-padding") + pad(k, "fo:end-padding"))
            })
            .collect();
    }
    let rule = c.child("style:column-sep").is_some_and(|s| s.attr("style:style") != Some("none") && s.attr("style:width").and_then(style::parse_length).is_none_or(|w| w > 0.0));
    Some(ColsDef { n, gap, shares, rule, no_balance: false })
}

fn unquote_font(f: &str) -> String {
    f.trim().trim_matches(|c| c == '\'' || c == '"').trim().to_owned()
}

fn parse_align(v: &str) -> Align {
    match v.trim() {
        "center" => Align::Center,
        "end" | "right" => Align::Right,
        "justify" => Align::Justify,
        _ => Align::Left,
    }
}

impl Styles {
    fn read_fonts(&mut self, root: &Node) {
        if let Some(decls) = root.child("office:font-face-decls") {
            for f in decls.els().filter(|n| n.name == "style:font-face") {
                if let (Some(name), Some(fam)) = (f.attr("style:name"), f.attr("svg:font-family")) {
                    self.fonts.insert(name.to_owned(), unquote_font(fam));
                }
            }
        }
    }

    fn text_props(&self, n: &Node) -> TProps {
        let mut t = TProps::default();
        for (k, v) in &n.attrs {
            let v = v.trim();
            match k.as_str() {
                "fo:font-weight" => {
                    t.bold = Some(v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600));
                }
                "fo:font-style" => t.italic = Some(matches!(v, "italic" | "oblique")),
                "style:text-underline-style" => t.underline = Some(v != "none"),
                "style:text-line-through-style" => t.strike = Some(v != "none"),
                "fo:font-size" => {
                    if let Some(p) = v.strip_suffix('%') {
                        t.size_pct = p.trim().parse().ok();
                    } else {
                        t.size = style::parse_size(v);
                    }
                }
                "fo:color" => t.color = style::parse_color(v),
                "fo:background-color" => {
                    t.bg = if v == "transparent" { Some(None) } else { style::parse_color(v).map(Some) };
                }
                _ => {}
            }
        }
        // `fo:font-family` vinner över `style:font-name` (som i LibreOffice).
        if let Some(f) = n.attr("fo:font-family") {
            let f = unquote_font(f.split(',').next().unwrap_or(f));
            if !f.is_empty() {
                t.font = Some(f);
            }
        } else if let Some(name) = n.attr("style:font-name") {
            t.font = Some(self.fonts.get(name).cloned().unwrap_or_else(|| name.to_owned()));
        }
        t
    }

    fn read_style(&self, s: &Node, auto: bool) -> StyleDef {
        let mut d = StyleDef {
            name: s.attr("style:name").unwrap_or("").to_owned(),
            display: s.attr("style:display-name").unwrap_or("").to_owned(),
            parent: s.attr("style:parent-style-name").map(str::to_owned),
            auto,
            master: s.attr("style:master-page-name").filter(|m| !m.is_empty()).map(str::to_owned),
            ..Default::default()
        };
        for c in s.els() {
            match c.name.as_str() {
                "style:section-properties" => d.cols = c.child("style:columns").and_then(columns_of).map(|mut cols| {
                        cols.no_balance = c.attr("text:dont-balance-text-columns").is_some_and(|v| v.trim() == "true");
                        cols
                    }),
                "style:text-properties" => d.tp = self.text_props(c),
                "style:paragraph-properties" => {
                    if let Some(a) = c.attr("fo:text-align") {
                        d.align = Some(parse_align(a));
                    }
                    let brk = |v: &str| match v {
                        "page" => Some(true),
                        "auto" => Some(false),
                        _ => None,
                    };
                    d.break_before = c.attr("fo:break-before").and_then(brk);
                    d.break_after = c.attr("fo:break-after").and_then(brk);
                    d.page_number = c.attr("style:page-number").and_then(|v| v.trim().parse().ok());
                    d.tabs = c
                        .child("style:tab-stops")
                        .map(|t| t.els().filter(|k| k.name == "style:tab-stop").map(|k| k.attr("style:type").unwrap_or("left").to_owned()).collect());
                    d.border_top = c.attr("fo:border-top").map(str::to_owned);
                    d.border_bottom = c.attr("fo:border-bottom").map(str::to_owned);
                    d.pp.before = c.attr("fo:margin-top").and_then(style::parse_length);
                    d.pp.after = c.attr("fo:margin-bottom").and_then(style::parse_length);
                    d.pp.indent = c.attr("fo:text-indent").and_then(style::parse_length);
                    d.pp.line = c
                        .attr("fo:line-height")
                        .and_then(|v| v.trim().strip_suffix('%')?.trim().parse::<f32>().ok())
                        .filter(|p| *p > 0.0)
                        .map(|p| p / 100.0);
                }
                "style:graphic-properties" => {
                    d.wrap = c.attr("style:wrap").map(str::to_owned);
                    d.hpos = c.attr("style:horizontal-pos").map(str::to_owned);
                    let g = &mut d.gfx;
                    g.hrel = c.attr("style:horizontal-rel").map(str::to_owned);
                    g.vpos = c.attr("style:vertical-pos").map(str::to_owned);
                    g.vrel = c.attr("style:vertical-rel").map(str::to_owned);
                    g.run_through = c.attr("style:run-through").map(str::to_owned);
                    g.clip = c.attr("fo:clip").map(str::to_owned);
                    for (i, k) in ["fo:margin-top", "fo:margin-right", "fo:margin-bottom", "fo:margin-left"].iter().enumerate() {
                        g.margin[i] = c.attr(k).and_then(style::parse_length);
                    }
                    if let Some(m) = c.attr("fo:margin").and_then(style::parse_length) {
                        g.margin = [Some(m); 4];
                    }
                    g.fill = c.attr("draw:fill-color").filter(|_| c.attr("draw:fill") == Some("solid")).map(str::to_owned);
                    g.border = c.attr("fo:border").map(str::to_owned);
                    g.pad = c.attr("fo:padding").and_then(style::parse_length);
                }
                _ => {}
            }
        }
        d
    }

    /// Ramens placering ur grafikformatet `name` och ramens egna `svg:x`/`svg:y`. `None` för ramar
    /// som ligger i texten (`as-char`) eller i en annan ram. `wrap="right"` betyder att texten
    /// står till höger om bilden, `left` tvärtom; `none` att texten står över och under; `run-through`
    /// att bilden ligger framför eller (`style:run-through="background"`) bakom texten.
    fn frame_place(&self, e: &Node, name: Option<&str>) -> Option<Anchor> {
        let kind = e.attr("text:anchor-type")?;
        if !matches!(kind, "paragraph" | "page" | "char") {
            return None;
        }
        let mut wrap: Option<&String> = None;
        let mut hpos: Option<&String> = None;
        let mut g = GfxPlace::default();
        if let Some(n) = name {
            for d in self.chain("graphic", n) {
                wrap = d.wrap.as_ref().or(wrap);
                hpos = d.hpos.as_ref().or(hpos);
                g.hrel = d.gfx.hrel.clone().or(g.hrel);
                g.vpos = d.gfx.vpos.clone().or(g.vpos);
                g.vrel = d.gfx.vrel.clone().or(g.vrel);
                g.run_through = d.gfx.run_through.clone().or(g.run_through);
                for i in 0..4 {
                    g.margin[i] = d.gfx.margin[i].or(g.margin[i]);
                }
            }
        }
        let px = |pt: f32| (pt * 96.0 / 72.0 * 10.0).round() / 10.0;
        let on_page = kind == "page";
        let hrel = match g.hrel.as_deref() {
            Some("page" | "page-start-margin" | "page-end-margin") => HRel::Page,
            Some("page-content") => HRel::Margin,
            Some(_) => HRel::Column,
            None if on_page => HRel::Page,
            None => HRel::Column,
        };
        let vrel = match g.vrel.as_deref() {
            Some("page") => VRel::Page,
            Some("page-content") => VRel::Margin,
            Some(_) => VRel::Paragraph,
            None if on_page => VRel::Page,
            None => VRel::Paragraph,
        };
        let wrap_s = wrap.map(String::as_str).unwrap_or(if on_page { "none" } else { "parallel" });
        let wrap = match wrap_s {
            "none" => Wrap::TopBottom,
            "run-through" => {
                if g.run_through.as_deref() == Some("background") { Wrap::Behind } else { Wrap::Front }
            }
            _ => Wrap::Square,
        };
        // Vågrätt: `from-left` använder `svg:x`; utan `horizontal-pos` avgör omflödet sidan.
        let x = e.attr("svg:x").and_then(signed_length).map(px);
        let h = match hpos.map(String::as_str) {
            Some("left" | "inside" | "from-inside") => Pos::Start,
            Some("center") => Pos::Center,
            Some("right" | "outside") => Pos::End,
            Some(_) => Pos::Off(x.unwrap_or(0.0)),
            None => match wrap_s {
                "right" => Pos::Start,
                "left" => Pos::End,
                _ => Pos::Off(x.unwrap_or(0.0)),
            },
        };
        let y = e.attr("svg:y").and_then(signed_length).map(px);
        let v = match g.vpos.as_deref() {
            Some("top") => Pos::Start,
            Some("middle") => Pos::Center,
            Some("bottom") => Pos::End,
            _ => Pos::Off(y.unwrap_or(0.0)),
        };
        let m = |i: usize| g.margin[i].map_or(0.0, px);
        let a = Anchor { hrel, h, vrel, v, wrap, dist: [m(0), m(1), m(2), m(3)] };
        // Förankring vid stycket utan eget läge och utan omflöde (`wrap="none"` utan placering) är
        // en vanlig bild på egen rad, som förut.
        let plain = wrap_s == "none" && hpos.is_none() && x.is_none() && y.is_none() && !on_page;
        (!plain).then_some(a)
    }

    /// Beskärningen (`fo:clip="rect(ovan, höger, under, vänster)"`) som procent av bilden
    /// (vänster, ovan, höger, under). Bildens eget mått antas vara 96 dpi.
    fn clip_pct(&self, name: &str, nw: f32, nh: f32) -> Option<[f32; 4]> {
        let clip = self.chain("graphic", name).iter().rev().find_map(|d| d.gfx.clip.clone())?;
        let inner = clip.trim().strip_prefix("rect(")?.strip_suffix(')')?;
        let parts: Vec<&str> = inner.split([',', ' ']).filter(|p| !p.is_empty()).collect();
        let l: Vec<f32> = parts.iter().filter_map(|p| signed_length(p)).map(|pt| pt * 96.0 / 72.0).collect();
        if l.len() != 4 || nw < 1.0 || nh < 1.0 {
            return None;
        }
        let c = [l[3] / nw * 100.0, l[0] / nh * 100.0, l[1] / nw * 100.0, l[2] / nh * 100.0];
        (c.iter().all(|v| (0.0..100.0).contains(v)) && c[0] + c[2] < 100.0 && c[1] + c[3] < 100.0 && c.iter().any(|v| *v > 0.1)).then_some(c)
    }

    fn list_style(&mut self, s: &Node) {
        let Some(name) = s.attr("style:name") else { return };
        let mut levels: Vec<(bool, u64)> = Vec::new();
        for l in s.els() {
            let ordered = match l.name.as_str() {
                "text:list-level-style-number" => true,
                "text:list-level-style-bullet" | "text:list-level-style-image" => false,
                _ => continue,
            };
            let level = l.attr("text:level").and_then(|v| v.parse::<usize>().ok()).unwrap_or(levels.len() + 1).clamp(1, 20);
            let start = l.attr("text:start-value").and_then(|v| v.parse().ok()).unwrap_or(1);
            while levels.len() < level {
                levels.push((false, 1));
            }
            levels[level - 1] = (ordered, start);
        }
        self.lists.insert(name.to_owned(), levels);
    }

    fn read_styles(&mut self, root: &Node) {
        for (section, auto) in [("office:styles", false), ("office:automatic-styles", true)] {
            let Some(sec) = root.child(section) else { continue };
            for s in sec.els() {
                match s.name.as_str() {
                    "style:style" => {
                        let d = self.read_style(s, auto);
                        let fam = s.attr("style:family").unwrap_or("").to_owned();
                        if fam == "table-column" {
                            if let Some(p) = s.child("style:table-column-properties") {
                                let w = p
                                    .attr("style:column-width")
                                    .and_then(style::parse_length)
                                    .map(f64::from)
                                    .or_else(|| p.attr("style:rel-column-width").and_then(|v| v.trim_end_matches('*').parse().ok()));
                                if let Some(w) = w {
                                    self.col_widths.insert(d.name.clone(), w);
                                }
                            }
                        }
                        self.defs.insert((fam, d.name.clone()), d);
                    }
                    "style:default-style" => {
                        let d = self.read_style(s, false);
                        self.defaults.insert(s.attr("style:family").unwrap_or("").to_owned(), d);
                    }
                    "text:list-style" => self.list_style(s),
                    _ => {}
                }
            }
        }
    }

    /// Sidmallen som ett stycke byter till (`style:master-page-name`), direkt eller ärvd.
    fn master_of(&self, style: &str) -> Option<&str> {
        self.chain("paragraph", style).iter().rev().find_map(|d| d.master.as_deref())
    }

    /// Sidnumret ett stycke börjar om på (`style:page-number`), direkt eller ärvt; `auto` ger `None`.
    fn page_number_of(&self, style: &str) -> Option<u32> {
        self.chain("paragraph", style).iter().rev().find_map(|d| d.page_number)
    }

    /// Sidbrytning före och efter ett stycke (`fo:break-before`/`fo:break-after`), direkt eller ärvd.
    fn page_breaks(&self, style: &str) -> (bool, bool) {
        let chain = self.chain("paragraph", style);
        let before = chain.iter().rev().find_map(|d| d.break_before).unwrap_or(false);
        let after = chain.iter().rev().find_map(|d| d.break_after).unwrap_or(false);
        (before, after)
    }

    /// Spalterna i ett avsnittsformat.
    fn section_columns(&self, style: &str) -> Option<ColsDef> {
        self.chain("section", style).iter().rev().find_map(|d| d.cols.clone())
    }

    /// Formatmallen och dess föräldrar, rot först.
    fn chain(&self, family: &str, name: &str) -> Vec<&StyleDef> {
        let mut out = Vec::new();
        let mut cur = Some(name.to_owned());
        while let Some(n) = cur {
            if out.len() >= 32 {
                break;
            }
            let Some(d) = self.defs.get(&(family.to_owned(), n)) else { break };
            out.push(d);
            cur = d.parent.clone();
        }
        out.reverse();
        out
    }

    fn default_props(&self, family: &str) -> TProps {
        self.defaults.get(family).map(|d| d.tp.clone()).unwrap_or_default()
    }

    /// Standardformat följt av hela kedjan.
    fn tprops(&self, family: &str, name: &str) -> TProps {
        self.chain(family, name).iter().fold(self.default_props(family), |acc, d| acc.over(&d.tp))
    }

    /// Som `tprops`, men utan stycken/tecken som bara är automatiska (direkt formatering).
    fn named_props(&self, family: &str, name: Option<&str>) -> TProps {
        let Some(name) = name else { return self.default_props(family) };
        let mut chain = self.chain(family, name);
        while chain.last().is_some_and(|d| d.auto) {
            chain.pop();
        }
        chain.iter().fold(self.default_props(family), |acc, d| acc.over(&d.tp))
    }

    /// Brödtext, Rubrik 1-6 och Citat så som formatmallarna själva anger dem (utan standardformat).
    fn style_set(&self) -> style::StyleSet {
        use style::{STYLE_IDS, StyleId};
        let mut set = style::StyleSet::default();
        for id in STYLE_IDS {
            let name = match id {
                StyleId::Body => "Text_20_body".to_owned(),
                StyleId::Heading(n) => format!("Heading_20_{n}"),
                StyleId::Quote => "Quotations".to_owned(),
                StyleId::Title => "Title".to_owned(),
                StyleId::Subtitle => "Subtitle".to_owned(),
            };
            if !self.defs.contains_key(&("paragraph".to_owned(), name.clone())) {
                continue;
            }
            let chain = self.chain("paragraph", &name);
            let tp = chain.iter().fold(TProps::default(), |acc, d| acc.over(&d.tp));
            let mut o = style::StyleOverride {
                font_family: tp.font,
                size: tp.size,
                bold: tp.bold,
                italic: tp.italic,
                color: tp.color,
                ..Default::default()
            };
            for d in &chain {
                o.align = d.align.or(o.align);
                o.space_before = d.pp.before.or(o.space_before);
                o.space_after = d.pp.after.or(o.space_after);
                o.line_spacing = d.pp.line.or(o.line_spacing);
                o.first_indent = d.pp.indent.or(o.first_indent);
            }
            if id == StyleId::Body {
                // Brödtextens typsnitt och storlek är dokumentets grundformat.
                o.font_family = None;
                o.size = None;
            }
            set.set(id, o);
        }
        set
    }

    fn align(&self, name: Option<&str>) -> Align {
        let mut a = self.defaults.get("paragraph").and_then(|d| d.align);
        if let Some(n) = name {
            for d in self.chain("paragraph", n) {
                if d.align.is_some() {
                    a = d.align;
                }
            }
        }
        a.unwrap_or(Align::Left)
    }
}

/// `Heading_20_1` → `heading 1`.
fn norm(s: &str) -> String {
    s.replace("_20_", " ").replace('_', " ").to_lowercase()
}

// ============================================================== läsning

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Normal,
    Heading(u8),
    /// Wordstilarna Title och Subtitle (`# Text {.title}` / `## Text {.subtitle}`).
    Title,
    Subtitle,
    Quote,
    Code,
    Rule,
}

#[derive(Clone, PartialEq, Default)]
struct Fmt {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    code: bool,
    span: SpanStyle,
    /// Spårad ändring som texten ligger i: slag och ändringens nummer.
    rev: Option<(RevKind, u32)>,
}

#[derive(Clone)]
struct Run {
    text: String,
    fmt: Fmt,
    link: Option<String>,
    /// Färdig Markdown (bild) som inte ska skyddas.
    raw: bool,
}

#[derive(Clone)]
struct ListInfo {
    id: usize,
    level: usize,
    /// `None` för fortsättningsstycken i samma listobjekt.
    marker: Option<String>,
}

struct Para {
    kind: Kind,
    runs: Vec<Run>,
    align: Align,
    list: Option<ListInfo>,
}

/// En tabell som läses: celler per rutnätsrad, spann (0 = täckt av cellen före) och kolumnbredder.
#[derive(Clone, Default)]
struct Tbl {
    rows: Vec<Vec<String>>,
    spans: Vec<Vec<usize>>,
    widths: Vec<f64>,
}

enum Block {
    Para(Para),
    Table(Tbl),
    Rule,
    /// Färdig Markdown på egna rader (textrutans markörer).
    Raw(String),
}

#[derive(Default)]
struct Dropped {
    shapes: bool,
    linked: bool,
    textboxes: bool,
    comments: bool,
    /// Spårade formatändringar (`text:format-change`) som inte kan visas.
    format_changes: bool,
}

impl Dropped {
    fn list(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.shapes {
            v.push(crate::i18n::t("diagram och former"));
        }
        if self.linked {
            v.push(crate::i18n::t("länkade bilder"));
        }
        if self.textboxes {
            v.push(crate::i18n::t("textrutor"));
        }
        if self.comments {
            v.push(crate::i18n::t("kommentarer"));
        }
        if self.format_changes {
            v.push(crate::i18n::t("spårade formatändringar"));
        }
        v
    }
}

/// Läget i ett stycke medan det läses.
struct Pc {
    /// Format som stycket självt ger (fet rubrik osv.): skrivs inte som fetstil.
    named: TProps,
    /// Det span-format jämförs mot.
    span_ref: TProps,
    /// Förra tecknet var ett mellanslag (för mellanslagskollaps).
    prev_space: bool,
}

#[derive(Clone)]
struct St {
    eff: TProps,
    code: bool,
    link: Option<String>,
}

struct Rd<'s, 'b> {
    st: &'s Styles,
    base: TProps,
    pkg: Pkg<'b>,
    dropped: Dropped,
    images: Vec<(String, Image)>,
    by_target: HashMap<String, String>,
    blocks: Vec<Block>,
    list_id: usize,
    /// Fotnotstexterna (Markdown på en rad) i dokumentordning; fotnot N är `[^N]`.
    /// Noternas Markdown, med sant för slutnoter.
    notes: Vec<(bool, String)>,
    /// Sant medan en fotnots stycken läses.
    in_note: bool,
    /// Textrutor som lästs i stycket som byggs: läggs före stycket (`docx_tbox`), och sant medan en ruta läses.
    pending_boxes: Vec<(Vec<Block>, crate::docx_tbox::Head)>,
    in_box: bool,
    /// Sidmallarnas sida och spalter, sidmallen som gäller just nu och dess egenskaper (avsnitt).
    masters: HashMap<String, sections::SectionProps>,
    cur_master: Option<String>,
    cur_page: sections::SectionProps,
    /// Dokumentets grundsida (för spaltbredder i förhållande till textytan).
    page_defaults: DocDefaults,
    review: ReviewRd,
}

/// En ändringsregion ur `text:tracked-changes`: slag, författare, datum och det raderade innehållet.
struct Region {
    kind: RevKind,
    author: String,
    date: String,
    paras: Vec<Node>,
}

/// Granskningsläget vid läsning: regionerna, posterna och pågående infogning.
#[derive(Default)]
struct ReviewRd {
    regions: HashMap<String, Region>,
    revs: Vec<RevRec>,
    comments: Vec<CommentRec>,
    /// `office:name` per kommentar (index = kommentarens nummer).
    names: Vec<String>,
    /// Namn som har en `office:annotation-end` i dokumentet.
    ends: std::collections::HashSet<String>,
    /// Kommentarer som väntar på sin slutmarkör.
    pending_end: Vec<u32>,
    /// Ändringen som gäller för texten just nu (infogning mellan start och slut, eller raderad text).
    cur: Option<(RevKind, u32)>,
    /// Den öppna infogningen (kan sträcka sig över flera stycken).
    open_ins: Option<u32>,
}

fn kind_of(st: &Styles, style: Option<&str>) -> Kind {
    let Some(s) = style else { return Kind::Normal };
    for d in st.chain("paragraph", s).iter().rev() {
        for x in [norm(&d.name), norm(&d.display)] {
            if let Some(num) = x.strip_prefix("heading ") {
                if let Ok(k) = num.trim().parse::<u8>() {
                    return Kind::Heading(k.clamp(1, 6));
                }
            }
            match x.as_str() {
                "title" => return Kind::Title,
                "subtitle" => return Kind::Subtitle,
                "quotations" | "quote" | "block quote" => return Kind::Quote,
                "preformatted text" => return Kind::Code,
                "horizontal line" => return Kind::Rule,
                _ => {}
            }
        }
    }
    Kind::Normal
}

fn is_code_char_style(st: &Styles, style: &str) -> bool {
    st.chain("text", style).iter().any(|d| norm(&d.name) == "source text" || norm(&d.display) == "source text")
}

fn escape_alt(s: &str) -> String {
    s.replace(['\n', '\r'], " ").replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]")
}

fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;").replace(['\n', '\r'], " ")
}

fn push_run(runs: &mut Vec<Run>, run: Run) {
    if run.text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if !run.raw && !last.raw && last.fmt == run.fmt && last.link == run.link {
            last.text.push_str(&run.text);
            return;
        }
    }
    runs.push(run);
}

impl<'s, 'b> Rd<'s, 'b> {
    fn fmt_for(&self, st: &St, pc: &Pc) -> Fmt {
        let e = &st.eff;
        let r = &pc.named;
        let sr = &pc.span_ref;
        let ref_font = sr.font.as_deref().unwrap_or(style::DEFAULT_FONT);
        let ref_size = sr.size.unwrap_or(style::DEFAULT_SIZE);
        Fmt {
            bold: e.bold == Some(true) && r.bold != Some(true),
            italic: e.italic == Some(true) && r.italic != Some(true),
            strike: e.strike == Some(true) && r.strike != Some(true),
            underline: e.underline == Some(true) && r.underline != Some(true),
            code: st.code,
            span: SpanStyle {
                font_family: e.font.clone().filter(|f| !st.code && !f.eq_ignore_ascii_case(ref_font)),
                font_size: e.size.filter(|s| (s - ref_size).abs() > 0.01),
                color: e.color.filter(|c| Some(*c) != sr.color),
                background: e.bg().filter(|c| Some(*c) != sr.bg()),
            },
            rev: self.review.cur,
        }
    }

    fn add_text(&self, runs: &mut Vec<Run>, text: String, st: &St, pc: &Pc) {
        push_run(runs, Run { text, fmt: self.fmt_for(st, pc), link: st.link.clone(), raw: false });
    }

    fn inline(&mut self, n: &Node, st: &St, pc: &mut Pc, runs: &mut Vec<Run>) {
        for k in &n.kids {
            match k {
                Kid::Text(t) => {
                    let mut out = String::new();
                    for c in t.chars() {
                        if matches!(c, ' ' | '\t' | '\n' | '\r') {
                            if !pc.prev_space {
                                out.push(' ');
                                pc.prev_space = true;
                            }
                        } else {
                            out.push(c);
                            pc.prev_space = false;
                        }
                    }
                    self.add_text(runs, out, st, pc);
                }
                Kid::El(e) => self.inline_el(e, st, pc, runs),
            }
        }
    }

    fn inline_el(&mut self, e: &Node, st: &St, pc: &mut Pc, runs: &mut Vec<Run>) {
        match e.name.as_str() {
            "text:span" => {
                let mut s2 = st.clone();
                if let Some(name) = e.attr("text:style-name") {
                    s2.eff = s2.eff.over(&self.st.tprops("text", name));
                    // Teckenmallen ärver inte styckets standardformat: bara det den själv anger.
                    if is_code_char_style(self.st, name) {
                        s2.code = true;
                    }
                }
                self.inline(e, &s2, pc, runs);
            }
            "text:a" => {
                let mut s2 = st.clone();
                // Länkens egen teckenmall (blå, understruken) ska inte bli formatering.
                if let Some(href) = e.attr("xlink:href").filter(|h| !h.is_empty() && !h.starts_with('#')) {
                    s2.link = Some(href.to_owned());
                }
                self.inline(e, &s2, pc, runs);
            }
            "text:s" => {
                let c = e.attr("text:c").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(10_000);
                self.add_text(runs, " ".repeat(c), st, pc);
                pc.prev_space = false;
            }
            "text:tab" => {
                self.add_text(runs, "\t".into(), st, pc);
                pc.prev_space = true;
            }
            "text:line-break" => {
                self.add_text(runs, "\n".into(), st, pc);
                pc.prev_space = true;
            }
            "draw:frame" => self.frame(e, st, pc, runs),
            "text:note" => self.note(e, runs),
            "office:annotation" => self.annotation(e, runs),
            "office:annotation-end" => self.annotation_end(e, runs),
            "text:change-start" | "text:change" => self.change_mark(e, st, pc, runs),
            "text:change-end" => {
                if e.attr("text:change-id").and_then(|id| self.review.regions.get(id)).is_some_and(|r| r.kind == RevKind::Ins) {
                    self.review.cur = None;
                    self.review.open_ins = None;
                }
            }
            "text:soft-page-break" | "text:bookmark" | "text:bookmark-start" | "text:bookmark-end" | "text:reference-mark"
            | "text:reference-mark-start" | "text:reference-mark-end" | "text:index-mark"
            | "text:index-mark-start" | "text:index-mark-end" | "text:changed-region" | "text:toc-mark"
            | "text:toc-mark-start" | "text:toc-mark-end" => {}
            n if n.starts_with("draw:") => self.dropped.shapes = true,
            _ => self.inline(e, st, pc, runs),
        }
    }

    fn marker_run(text: String) -> Run {
        Run { text, fmt: Fmt::default(), link: None, raw: true }
    }

    /// `text:change-start` (infogning: text som följer är infogad, borttagning: den raderade texten
    /// läggs här) och `text:change` (raderad text på platsen).
    fn change_mark(&mut self, e: &Node, st: &St, pc: &mut Pc, runs: &mut Vec<Run>) {
        let Some(id) = e.attr("text:change-id") else { return };
        let Some(reg) = self.review.regions.get(id) else { return };
        let (kind, author, date) = (reg.kind, reg.author.clone(), reg.date.clone());
        let rev_id = self.review.revs.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        match kind {
            RevKind::Ins if e.name == "text:change-start" => {
                self.review.revs.push(RevRec { id: rev_id, author, date, old: None });
                self.review.cur = Some((RevKind::Ins, rev_id));
                self.review.open_ins = Some(rev_id);
            }
            RevKind::Ins => {}
            RevKind::Del if e.name == "text:change-start" || e.name == "text:change" => {
                let paras = self.review.regions.get(id).map(|r| r.paras.clone()).unwrap_or_default();
                if paras.is_empty() || self.in_note {
                    return;
                }
                self.review.revs.push(RevRec { id: rev_id, author, date, old: None });
                let saved = self.review.cur;
                self.review.cur = Some((RevKind::Del, rev_id));
                for (i, p) in paras.iter().enumerate() {
                    if i > 0 {
                        self.add_text(runs, " ".into(), st, pc);
                    }
                    self.inline(p, st, pc, runs);
                }
                self.review.cur = saved;
            }
            RevKind::Del => {}
            // ODT-läsaren ger inga formateringsändringar (de rapporteras som tappade).
            RevKind::Fmt => {}
        }
    }

    /// Texten i en kommentars stycken (`text:p`), ett stycke per rad.
    fn annotation_text(n: &Node) -> String {
        fn plain(n: &Node, out: &mut String) {
            for k in &n.kids {
                match k {
                    Kid::Text(t) => out.push_str(t),
                    Kid::El(e) => match e.name.as_str() {
                        "text:s" => out.push_str(&" ".repeat(e.attr("text:c").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(10_000))),
                        "text:tab" => out.push('\t'),
                        "text:line-break" => out.push('\n'),
                        _ => plain(e, out),
                    },
                }
            }
        }
        let mut paras: Vec<String> = Vec::new();
        for c in n.els().filter(|c| matches!(c.name.as_str(), "text:p" | "text:h")) {
            let mut s = String::new();
            plain(c, &mut s);
            paras.push(s);
        }
        paras.join("\n")
    }

    /// `office:annotation`: startmarkören i texten och en post. Saknar kommentaren slutmarkör
    /// (`office:annotation-end`) blir området tomt.
    fn annotation(&mut self, e: &Node, runs: &mut Vec<Run>) {
        if self.in_note {
            self.dropped.comments = true;
            return;
        }
        let name = e.attr("office:name").unwrap_or("").to_owned();
        let id = self.review.comments.len() as u32;
        // Svar: `loext:parent-name` pekar på kommentaren (trådens rot) de svarar på.
        let parent = e.attr("loext:parent-name").and_then(|p| self.review.names.iter().position(|n| !n.is_empty() && n == p)).map(|i| {
            let i = i as u32;
            self.review.comments[i as usize].parent.unwrap_or(i)
        });
        let child_text = |k: &str| e.child(k).map(|c| c.deep_text().trim().to_owned()).unwrap_or_default();
        self.review.comments.push(CommentRec {
            id,
            parent,
            author: child_text("dc:creator"),
            date: child_text("dc:date"),
            done: e.attr("loext:resolved") == Some("true"),
            text: Self::annotation_text(e),
        });
        self.review.names.push(name.clone());
        // Ett svar delar trådens område: dess markörer sätts intill förälderns efter läsningen (`attach_replies`),
        // eftersom LibreOffice skriver svaret efter områdets slut.
        if parent.is_some() {
            return;
        }
        runs.push(Self::marker_run(review::marker_start(id)));
        if !name.is_empty() && self.review.ends.contains(&name) {
            self.review.pending_end.push(id);
        } else {
            runs.push(Self::marker_run(review::marker_end(id)));
        }
    }

    /// Slutet på ett kommentarsområde.
    fn annotation_end(&mut self, e: &Node, runs: &mut Vec<Run>) {
        let Some(name) = e.attr("office:name") else { return };
        let Some(id) = self.review.names.iter().position(|n| n == name) else { return };
        let id = id as u32;
        if self.review.pending_end.contains(&id) {
            self.review.pending_end.retain(|&c| c != id);
            runs.push(Self::marker_run(review::marker_end(id)));
        }
    }

    /// En fotnot eller slutnot: `[^N]` i texten, och texten sparas för definitionen sist i dokumentet.
    fn note(&mut self, e: &Node, runs: &mut Vec<Run>) {
        if self.in_note {
            return;
        }
        self.in_note = true;
        let mut parts: Vec<String> = Vec::new();
        if let Some(body) = e.child("text:note-body") {
            self.note_paras(body, &mut parts);
        }
        self.in_note = false;
        let end = e.attr("text:note-class") == Some("endnote");
        self.notes.push((end, escape_line_start(&parts.join("<br>"))));
        let n = self.notes.iter().filter(|x| x.0 == end).count();
        let label = if end { format!("[^{}]", crate::word::markup::endnote_label(n)) } else { format!("[^{n}]") };
        push_run(runs, Run { text: label, fmt: Fmt::default(), link: None, raw: true });
    }

    /// Fotnotens stycken som Markdown, med radbrytningar som `<br>`.
    fn note_paras(&mut self, n: &Node, out: &mut Vec<String>) {
        for c in n.els() {
            match c.name.as_str() {
                "text:p" | "text:h" => {
                    let p = self.build_para(c, None);
                    let s = para_md(&p).replace('\n', "<br>");
                    if !s.trim().is_empty() {
                        out.push(s.trim().to_owned());
                    }
                }
                "text:list" | "text:list-item" | "text:section" => self.note_paras(c, out),
                _ => {}
            }
        }
    }

    fn frame(&mut self, e: &Node, st: &St, pc: &mut Pc, runs: &mut Vec<Run>) {
        if let Some(tb) = e.child("draw:text-box") {
            self.text_box(e, tb);
            return;
        }
        // LibreOffice skriver en SVG som två `draw:image`: SVG-filen och en PNG-reserv. Rastret är nyckeln;
        // SVG:n följer med som vektorkälla.
        let hrefs: Vec<String> = e
            .els()
            .filter(|c| c.name == "draw:image")
            .filter_map(|c| c.attr("xlink:href").map(|h| h.trim().to_owned()))
            .filter(|h| !h.is_empty())
            .collect();
        if hrefs.is_empty() {
            self.dropped.shapes = true;
            return;
        }
        if hrefs.iter().all(|h| h.contains("://") || h.starts_with("..") || h.starts_with('/')) {
            self.dropped.linked = true;
            return;
        }
        let local: Vec<String> = hrefs.iter().filter(|h| !(h.contains("://") || h.starts_with("..") || h.starts_with('/'))).map(|h| h.trim_start_matches("./").to_owned()).collect();
        let is_svg_href = |h: &str| h.to_lowercase().ends_with(".svg");
        let target = local.iter().find(|h| !is_svg_href(h)).or(local.first()).cloned().unwrap_or_default();
        let svg_target = local.iter().find(|h| is_svg_href(h) && **h != target).cloned();
        let name = if let Some(n) = self.by_target.get(&target) {
            n.clone()
        } else {
            let Some(bytes) = self.pkg.bytes(&target).ok().flatten() else {
                self.dropped.shapes = true;
                return;
            };
            let svg_bytes = svg_target.as_ref().and_then(|t| self.pkg.bytes(t).ok().flatten());
            let (bytes, svg) = if crate::imgfmt::is_svg(&bytes) {
                // Bara SVG (utan reserv): rastret ritas ur SVG:n.
                match crate::imgfmt::from_package(bytes, None) {
                    Some(i) => (i.bytes, i.svg),
                    None => {
                        self.dropped.shapes = true;
                        return;
                    }
                }
            } else if !is_supported_image(&bytes) {
                self.dropped.shapes = true;
                return;
            } else {
                match svg_bytes.and_then(|sv| crate::imgfmt::from_package(sv, Some(bytes.clone()))) {
                    Some(i) => (i.bytes, i.svg),
                    None => (bytes, None),
                }
            };
            let base = target.rsplit('/').next().unwrap_or("bild").to_owned();
            let mut name = base.clone();
            let mut k = 2;
            while self.images.iter().any(|(m, _)| *m == name) {
                name = format!("{k}-{base}");
                k += 1;
            }
            let w = e.attr("svg:width").and_then(style::parse_length);
            let h = e.attr("svg:height").and_then(style::parse_length);
            let extent = w.zip(h).map(|(w, h)| ((w * 12700.0) as i64, (h * 12700.0) as i64));
            self.images.push((name.clone(), Image { bytes, extent, svg }));
            self.by_target.insert(target.clone(), name.clone());
            name
        };
        let alt = ["svg:desc", "svg:title"]
            .iter()
            .find_map(|k| e.child(k).map(|n| n.deep_text()).filter(|t| !t.trim().is_empty()))
            .unwrap_or_default();
        let (_, image) = self.images.iter().find(|(m, _)| *m == name).unwrap();
        let natural = image.natural_px();
        let fw = e.attr("svg:width").and_then(style::parse_length).map(|pt| pt * 96.0 / 72.0);
        let fh = e.attr("svg:height").and_then(style::parse_length).map(|pt| pt * 96.0 / 72.0);
        let src = format!("{IMAGE_SCHEME}{}", name.replace(' ', "%20"));
        // Ramar som ligger i texten (`as-char`) flyter aldrig.
        let place = self.st.frame_place(e, e.attr("draw:style-name"));
        let float = place.as_ref().and_then(Anchor::legacy_side);
        let anchor = place.filter(|_| float.is_none());
        let crop = natural.zip(e.attr("draw:style-name")).and_then(|((nw, nh), n)| self.st.clip_pct(n, nw, nh));
        // Beskärning och sträckning ger en egen höjd.
        let cropped_h = |w: f32| match (crop, natural) {
            (Some(c), Some((nw, nh))) => nh * (1.0 - (c[1] + c[3]) / 100.0) / (nw * (1.0 - (c[0] + c[2]) / 100.0)) * w,
            (None, Some((nw, nh))) => w * nh / nw,
            _ => w,
        };
        let md = match (natural, fw) {
            (Some((nw, _)), Some(w))
                if float.is_some() || anchor.is_some() || crop.is_some() || (w - nw).abs() > 1.5 || fh.is_some_and(|h| (h - cropped_h(w)).abs() > 1.5) =>
            {
                let mut tag = format!("<img src=\"{}\"", html_escape_attr(&src));
                if !alt.is_empty() {
                    tag.push_str(&format!(" alt=\"{}\"", html_escape_attr(&alt)));
                }
                tag.push_str(&format!(" width=\"{}\"", w.round() as i64));
                if let Some(h) = fh.filter(|h| (h - cropped_h(w)).abs() > 1.5 || crop.is_some()) {
                    tag.push_str(&format!(" height=\"{}\"", h.round() as i64));
                }
                match (anchor, float) {
                    (Some(a), _) => a.write_attrs(&mut tag),
                    (None, Some(Float::Left)) => tag.push_str(" align=\"left\""),
                    (None, Some(Float::Right)) => tag.push_str(" align=\"right\""),
                    (None, None) => {}
                }
                if let Some(c) = crop {
                    let f = |v: f32| format!("{}", (v * 100.0).round() / 100.0);
                    tag.push_str(&format!(" crop=\"{} {} {} {}\"", f(c[0]), f(c[1]), f(c[2]), f(c[3])));
                }
                tag.push('>');
                tag
            }
            _ => format!("![{}]({src})", escape_alt(&alt)),
        };
        let _ = pc;
        push_run(runs, Run { text: md, fmt: Fmt::default(), link: st.link.clone(), raw: true });
    }

    /// Bygger ett stycke ur `text:p` eller `text:h`.
    fn build_para(&mut self, n: &Node, list: Option<ListInfo>) -> Para {
        let style = n.attr("text:style-name");
        let mut kind = kind_of(self.st, style);
        if n.name == "text:h" && !matches!(kind, Kind::Heading(_)) {
            let lvl = n.attr("text:outline-level").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1).clamp(1, 6);
            kind = Kind::Heading(lvl);
        }
        let named = self.st.named_props("paragraph", style);
        // I en fotnot jämförs texten med fotnotsmallen (inte brödtexten), så att den inte fylls med spans.
        let span_ref = if self.in_note {
            self.base.over(&named)
        } else if kind == Kind::Normal {
            self.base.clone()
        } else {
            named.clone()
        };
        let eff = self.st.tprops("paragraph", style.unwrap_or("Standard"));
        let mut pc = Pc { named, span_ref, prev_space: true };
        let st = St { eff, code: false, link: None };
        let mut runs = Vec::new();
        self.inline(n, &st, &mut pc, &mut runs);
        let list = if matches!(kind, Kind::Heading(_)) { None } else { list };
        Para { kind, runs, align: self.st.align(style), list }
    }

    /// Lägger de lästa textrutorna som block (`<!--k:tb 0-->` … `<!--k:/tb-->`) på brödtexten.
    fn flush_boxes(&mut self) {
        if self.in_box {
            return;
        }
        for (inner, head) in std::mem::take(&mut self.pending_boxes) {
            self.blocks.push(Block::Raw(crate::docx_tbox::open_tag_head(0, &head)));
            self.blocks.extend(inner);
            self.blocks.push(Block::Raw(crate::docx_tbox::CLOSE.to_owned()));
        }
    }

    /// Fyllning, kant och luft för ramen med grafikformatet `name`.
    fn box_shape(&self, name: Option<&str>) -> crate::docx_tbox::Shape {
        let mut g = GfxPlace::default();
        for d in name.map(|n| self.st.chain("graphic", n)).unwrap_or_default() {
            g.fill = d.gfx.fill.clone().or(g.fill);
            g.border = d.gfx.border.clone().or(g.border);
            g.pad = d.gfx.pad.or(g.pad);
        }
        let hex = |c: &str| {
            let c = c.trim().trim_start_matches('#');
            (c.len() == 6 && c.is_ascii()).then(|| u32::from_str_radix(c, 16).ok()).flatten().map(|v| [(v >> 16) as u8, (v >> 8) as u8, v as u8])
        };
        let line = g.border.as_deref().and_then(|b| {
            let parts: Vec<&str> = b.split_whitespace().collect();
            if parts.len() < 3 || parts[1] == "none" || parts[1] == "hidden" {
                return None;
            }
            Some((style::parse_length(parts[0])?.max(0.25), hex(parts[2])?))
        });
        let pad = g.pad.unwrap_or(5.7);
        crate::docx_tbox::Shape { fill: g.fill.as_deref().and_then(hex), line, ins: Some([pad; 4]) }
    }

    /// En textruta (`draw:text-box`): innehållet läses som stycken och läggs före stycket ramen sitter i.
    fn text_box(&mut self, e: &Node, tb: &Node) {
        use crate::word::pbox::FrameX;
        if self.in_box {
            self.dropped.textboxes = true;
            return;
        }
        let name = e.attr("draw:style-name");
        let shape = self.box_shape(name);
        let width = e.attr("svg:width").and_then(style::parse_length).unwrap_or(0.0);
        let place = self.st.frame_place(e, name);
        // `fo:min-height` på 0,5 cm skriver Kladd själv när rutan saknar höjd: ingen fast höjd.
        let height = e
            .attr("svg:height")
            .and_then(style::parse_length)
            .or_else(|| e.attr("fo:min-height").and_then(style::parse_length).filter(|h| (*h - 14.173).abs() > 0.01))
            .unwrap_or(0.0);
        let head = crate::docx_tbox::Head { w: width * 96.0 / 72.0, h: height * 96.0 / 72.0, anchor: place };
        // Läget från sidans kant räknas om till spaltens vänsterkant.
        let from_page = place.as_ref().is_some_and(|a| a.hrel == HRel::Page);
        let x = match place.map(|a| a.h) {
            Some(Pos::Center) => FrameX::Center,
            Some(Pos::End) => FrameX::End,
            Some(Pos::Off(px)) => {
                // Rutan får stå ut i marginalen (som i LibreOffice) men klämms mot papperets kant.
                let (ml, pw) = (self.page_defaults.margins()[3].round(), self.page_defaults.page().0);
                let pt = ((px * 0.75 - if from_page { ml } else { 0.0 }) * 2.0).round() / 2.0;
                let pt = pt.clamp(-ml, (pw - ml - width).max(-ml));
                if pt == 0.0 { FrameX::Start } else { FrameX::Off(pt) }
            }
            _ => FrameX::Start,
        };
        let saved = std::mem::take(&mut self.blocks);
        self.in_box = true;
        for c in tb.els() {
            self.walk(c);
        }
        self.in_box = false;
        let mut inner = std::mem::replace(&mut self.blocks, saved);
        // En ruta utan text (bara tomma stycken) ger inget block.
        let has_text = inner.iter().any(|b| match b {
            Block::Para(p) => !para_md(p).trim().is_empty(),
            Block::Table(_) | Block::Raw(_) => true,
            Block::Rule => false,
        });
        let pp = crate::docx_tbox::frame(&shape, x, width, height);
        for b in inner.iter_mut() {
            if let Block::Para(p) = b {
                if p.list.is_none() && p.kind != Kind::Code && !pp.is_empty() {
                    p.runs.push(Rd::marker_run(pp.marker()));
                }
            }
        }
        if has_text {
            self.pending_boxes.push((inner, head));
        }
    }

    fn emit(&mut self, p: Para) {
        self.flush_boxes();
        if p.kind == Kind::Rule {
            self.blocks.push(Block::Rule);
            return;
        }
        if p.kind == Kind::Code && p.list.is_none() {
            // Kommentarsmarkörer och raderad text hör inte hemma i ett kodblock.
            let code_text = |p: &Para| -> String {
                p.runs
                    .iter()
                    .filter(|r| !(r.raw && review::parse_marker(&r.text).is_some()) && !matches!(r.fmt.rev, Some((RevKind::Del, _))))
                    .map(|r| r.text.as_str())
                    .collect()
            };
            if let Some(Block::Para(last)) = self.blocks.last_mut() {
                if last.kind == Kind::Code && last.list.is_none() {
                    let raw = code_text(&p);
                    let first = &mut last.runs[0];
                    first.text.push('\n');
                    first.text.push_str(&raw);
                    return;
                }
            }
            // Ett kodblock samlas i en enda körning.
            let raw = code_text(&p);
            self.blocks.push(Block::Para(Para {
                kind: Kind::Code,
                runs: vec![Run { text: raw, fmt: Fmt::default(), link: None, raw: true }],
                align: Align::Left,
                list: None,
            }));
            return;
        }
        self.blocks.push(Block::Para(p));
    }

    /// En rad `<!-- kladd: avsnitt; … -->`.
    fn section_row(&mut self, props: &sections::SectionProps) {
        let run = Run { text: props.to_comment(), fmt: Fmt::default(), link: None, raw: true };
        self.blocks.push(Block::Para(Para { kind: Kind::Normal, runs: vec![run], align: Align::Left, list: None }));
    }

    /// En rad `<!-- kladd: sidbrytning -->` (inte först i dokumentet och inte två i rad).
    /// `allow_first`: ett tomt stycke som bär brytningen (så skriver Kladd en avslutande brytning).
    fn page_break_row(&mut self, allow_first: bool) {
        let last_is_break = matches!(self.blocks.last(), Some(Block::Para(p)) if matches!(p.runs.as_slice(), [r] if r.raw && r.text == sections::PAGE_BREAK_LINE));
        if (self.blocks.is_empty() && !allow_first) || last_is_break {
            return;
        }
        let run = Run { text: sections::PAGE_BREAK_LINE.to_owned(), fmt: Fmt::default(), link: None, raw: true };
        self.blocks.push(Block::Para(Para { kind: Kind::Normal, runs: vec![run], align: Align::Left, list: None }));
    }

    /// Byter stycket sidmall börjar en ny sida med nya mått: en avsnittsrad.
    fn check_master(&mut self, n: &Node) {
        let Some(m) = n.attr("text:style-name").and_then(|s| self.st.master_of(s)) else { return };
        // Sidnumret kan börja om också med samma sidmall (`style:page-number` på stycket).
        let restart = n.attr("text:style-name").and_then(|s| self.st.page_number_of(s));
        if self.cur_master.as_deref() == Some(m) && (restart.is_none() || self.blocks.is_empty()) {
            return;
        }
        let Some(mut props) = self.masters.get(m).cloned() else { return };
        if restart.is_some() {
            props.hf.page_start = restart;
        }
        self.cur_master = Some(m.to_owned());
        self.cur_page = props.clone();
        self.section_row(&props);
    }

    fn walk(&mut self, n: &Node) {
        match n.name.as_str() {
            "text:p" | "text:h" => {
                let before_master = self.blocks.len();
                self.check_master(n);
                let (brk_before, brk_after) = n.attr("text:style-name").map_or((false, false), |s| self.st.page_breaks(s));
                if brk_before && self.blocks.len() == before_master {
                    self.page_break_row(n.deep_text().trim().is_empty());
                }
                let p = self.build_para(n, None);
                self.emit(p);
                if brk_after {
                    self.page_break_row(false);
                }
            }
            // Avsnitt med spalter: en löpande avsnittsrad före och en som återställer efter.
            "text:section" if n.attr("text:style-name").and_then(|s| self.st.section_columns(s)).is_some() => {
                let def = n.attr("text:style-name").and_then(|s| self.st.section_columns(s)).unwrap_or(ColsDef { n: 2, gap: sections::DEFAULT_GAP, ..Default::default() });
                let mut open = sections::SectionProps { continuous: true, ..self.cur_page.clone() };
                let cur = sections::Section::resolve(&self.cur_page, &self.page_defaults);
                def.apply(&mut open, (cur.page.0 - cur.margin[1] - cur.margin[3]).max(1.0));
                self.section_row(&open);
                for c in n.els() {
                    self.walk(c);
                }
                let close = sections::SectionProps { columns: None, column_gap: None, column_widths: None, column_rule: false, continuous: true, ..self.cur_page.clone() };
                self.section_row(&close);
            }
            // Innehållsförteckningen blir raden `<!-- kladd: toc -->`; innehållet byggs om av Kladd.
            "text:table-of-content" => {
                let run = Run { text: crate::word::nav::TOC_LINE.to_owned(), fmt: Fmt::default(), link: None, raw: true };
                self.blocks.push(Block::Para(Para { kind: Kind::Normal, runs: vec![run], align: Align::Left, list: None }));
            }
            "text:list" => {
                self.list_id += 1;
                let id = self.list_id;
                self.list(n, 0, None, id);
            }
            "table:table" => self.table(n),
            "text:section" | "text:alphabetical-index" | "text:illustration-index"
            | "text:table-index" | "text:object-index" | "text:user-index" | "text:bibliography" | "text:index-body"
            | "text:index-title" => {
                for c in n.els() {
                    self.walk(c);
                }
            }
            "draw:frame" => {
                let mut runs = Vec::new();
                let st = St { eff: TProps::default(), code: false, link: None };
                let mut pc = Pc { named: TProps::default(), span_ref: self.base.clone(), prev_space: true };
                self.frame(n, &st, &mut pc, &mut runs);
                if !runs.is_empty() {
                    self.blocks.push(Block::Para(Para { kind: Kind::Normal, runs, align: Align::Left, list: None }));
                }
            }
            n if n.starts_with("draw:") => self.dropped.shapes = true,
            _ => {}
        }
    }

    fn list(&mut self, n: &Node, depth: usize, inherited: Option<&str>, id: usize) {
        let style = n.attr("text:style-name").or(inherited);
        let (ordered, start) = style
            .and_then(|s| self.st.lists.get(s))
            .and_then(|l| l.get(depth).or(l.last()))
            .copied()
            .unwrap_or((false, 1));
        let mut counter = start;
        for item in n.els() {
            let header = item.name == "text:list-header";
            if item.name != "text:list-item" && !header {
                continue;
            }
            if let Some(v) = item.attr("text:start-value").and_then(|v| v.parse::<u64>().ok()) {
                counter = v;
            }
            let marker = if ordered { format!("{counter}. ") } else { "- ".to_owned() };
            let mut first = !header;
            for c in item.els() {
                match c.name.as_str() {
                    "text:p" | "text:h" => {
                        let m = if first { Some(marker.clone()) } else { None };
                        let info = ListInfo { id, level: depth, marker: m };
                        let mut p = self.build_para(c, Some(info));
                        if p.list.is_none() {
                            p.list = None;
                        }
                        // Ett tomt första stycke ger ändå objektet sitt märke.
                        if p.runs.is_empty() && first && p.kind == Kind::Normal {
                            p.runs.push(Run { text: String::new(), fmt: Fmt::default(), link: None, raw: false });
                        }
                        first = false;
                        self.emit(p);
                    }
                    "text:list" => self.list(c, depth + 1, style, id),
                    _ => {}
                }
            }
            if !header {
                counter += 1;
            }
        }
    }

    /// Cellens text som Markdown på en rad.
    fn cell_text(&mut self, cell: &Node) -> String {
        let mut parts = Vec::new();
        self.cell_paras(cell, &mut parts);
        parts.join("<br>").replace('|', "\\|")
    }

    /// Kolumnernas bredder ur `table:table-column` (också i grupper).
    fn column_widths(&self, n: &Node, out: &mut Vec<f64>) {
        for c in n.els() {
            match c.name.as_str() {
                "table:table-columns" | "table:table-header-columns" | "table:table-column-group" => self.column_widths(c, out),
                "table:table-column" => {
                    let rep = c.attr("table:number-columns-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
                    let w = c.attr("table:style-name").and_then(|s| self.st.col_widths.get(s)).copied().unwrap_or(0.0);
                    for _ in 0..rep.clamp(1, 256) {
                        out.push(w);
                    }
                }
                _ => {}
            }
        }
    }

    fn cell_paras(&mut self, n: &Node, out: &mut Vec<String>) {
        for c in n.els() {
            match c.name.as_str() {
                "text:p" | "text:h" => {
                    let p = self.build_para(c, None);
                    let s = para_md(&p).replace('\n', "<br>");
                    if !s.trim().is_empty() {
                        out.push(s.trim().to_owned());
                    }
                }
                "text:list" | "text:list-item" | "text:section" => self.cell_paras(c, out),
                "table:table" => {}
                _ => {}
            }
        }
    }

    fn table(&mut self, t: &Node) {
        let mut tbl = Tbl::default();
        self.table_rows(t, &mut tbl);
        if tbl.rows.is_empty() {
            return;
        }
        self.column_widths(t, &mut tbl.widths);
        self.blocks.push(Block::Table(tbl));
    }

    fn table_rows(&mut self, n: &Node, tbl: &mut Tbl) {
        for c in n.els() {
            match c.name.as_str() {
                "table:table-header-rows" | "table:table-row-group" | "table:table-rows" => self.table_rows(c, tbl),
                "table:table-row" => {
                    let mut cells = Vec::new();
                    let mut spans = Vec::new();
                    // Kolumner kvar i den vågräta sammanfogningen från cellen före.
                    let mut pending = 0usize;
                    for cell in c.els() {
                        let rep = cell.attr("table:number-columns-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
                        match cell.name.as_str() {
                            "table:table-cell" => {
                                let text = self.cell_text(cell);
                                let rep = if text.is_empty() { rep.min(1) } else { rep.min(64) };
                                for _ in 0..rep.max(1) {
                                    cells.push(text.clone());
                                    spans.push(1);
                                }
                                pending = cell
                                    .attr("table:number-columns-spanned")
                                    .and_then(|v| v.parse::<usize>().ok())
                                    .map_or(0, |n| n.clamp(1, 64) - 1);
                            }
                            // Täckt av en vågrät sammanfogning, eller (lodrätt) av en cell ovanför:
                            // då en tom vanlig cell.
                            "table:covered-table-cell" => {
                                for _ in 0..rep.clamp(1, 64) {
                                    cells.push(String::new());
                                    if pending > 0 {
                                        pending -= 1;
                                        spans.push(0);
                                    } else {
                                        spans.push(1);
                                    }
                                }
                            }
                            _ => {}
                        }
                        if cells.len() > 256 {
                            break;
                        }
                    }
                    let rep = c.attr("table:number-rows-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
                    if cells.iter().all(String::is_empty) && rep > 1 {
                        continue;
                    }
                    for _ in 0..rep.clamp(1, 64) {
                        tbl.rows.push(cells.clone());
                        tbl.spans.push(spans.clone());
                    }
                }
                _ => {}
            }
        }
    }
}

fn wrap_tag(text: &str, open: &str, close: &str) -> String {
    let start = text.len() - text.trim_start().len();
    let end = text.trim_end().len();
    if start >= end {
        return text.to_owned();
    }
    format!("{}{open}{}{close}{}", &text[..start], &text[start..end], &text[end..])
}

fn inline_md(run: &Run) -> String {
    if run.raw {
        return run.text.clone();
    }
    let mut parts = Vec::new();
    for (i, piece) in run.text.split('\n').enumerate() {
        if i > 0 {
            parts.push("\n".to_owned());
        }
        if piece.is_empty() {
            continue;
        }
        let mut s = if run.fmt.code {
            let ticks = if piece.contains('`') { "``" } else { "`" };
            let pad = if piece.starts_with('`') || piece.ends_with('`') { " " } else { "" };
            format!("{ticks}{pad}{piece}{pad}{ticks}")
        } else {
            escape_inline(piece)
        };
        if run.fmt.strike {
            s = wrap(&s, "~~");
        }
        if run.fmt.italic {
            s = wrap(&s, "*");
        }
        if run.fmt.bold {
            s = wrap(&s, "**");
        }
        if run.fmt.underline {
            s = wrap_tag(&s, "<u>", "</u>");
        }
        if !run.fmt.span.is_empty() {
            s = wrap_tag(&s, &run.fmt.span.open_tag(), "</span>");
        }
        if let Some((kind, id)) = run.fmt.rev {
            s = format!("{}{s}{}", review::open_tag(kind, id), review::close_tag(kind));
        }
        parts.push(s);
    }
    parts.concat()
}

fn link_url(url: &str) -> String {
    url.replace(' ', "%20").replace(')', "%29")
}

fn para_md(p: &Para) -> String {
    let mut s = String::new();
    let mut i = 0;
    while i < p.runs.len() {
        let link = &p.runs[i].link;
        if link.is_some() {
            // Intilliggande körningar med samma länk delar en länk.
            let mut inner = String::new();
            let mut j = i;
            while j < p.runs.len() && p.runs[j].link == *link {
                inner.push_str(&inline_md(&p.runs[j]));
                j += 1;
            }
            s.push_str(&format!("[{inner}]({})", link_url(link.as_deref().unwrap_or(""))));
            i = j;
        } else {
            s.push_str(&inline_md(&p.runs[i]));
            i += 1;
        }
    }
    let lines: Vec<String> = s
        .lines()
        .map(|l| {
            let l = l.trim_end();
            // En tab först på raden blir annars ett kodblock.
            let t = l.trim_start_matches('\t');
            if t.len() != l.len() {
                format!("{}{t}", "    ".repeat(l.len() - t.len()))
            } else if l.starts_with("<!--k:") {
                // En rad som börjar med en markör vore annars ett HTML-block; `write` tar bort tecknet igen.
                format!("&#8203;{l}")
            } else {
                l.to_owned()
            }
        })
        .collect();
    lines.join("\n").trim_matches('\n').to_owned()
}

fn set_align(out: &mut String, cur: &mut Align, new: Align) {
    if *cur == new {
        return;
    }
    if *cur != Align::Left {
        out.push_str("\n\n</div>");
    }
    if new != Align::Left {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&style::div_open(new));
        out.push_str("\n\n");
    } else if !out.is_empty() {
        out.push_str("\n\n");
    }
    *cur = new;
}

/// `text_w` är textytans bredd i pt: tabeller som är klart smalare får automatisk bredd.
fn render(blocks: &[Block], text_w: Option<f64>) -> String {
    let mut out = String::new();
    let mut prev_list: Option<usize> = None;
    let mut cur_align = Align::Left;
    for b in blocks {
        match b {
            Block::Para(p) => {
                let text = if p.kind == Kind::Code { p.runs.first().map(|r| r.text.clone()).unwrap_or_default() } else { para_md(p) };
                let list_marker = p.list.as_ref().and_then(|l| l.marker.clone());
                // Ett tomt listobjekt behåller sitt märke.
                if p.kind != Kind::Code && text.trim().is_empty() && list_marker.is_none() {
                    continue;
                }
                let align = if p.kind == Kind::Code { Align::Left } else { p.align };
                let this_list = p.list.as_ref().map(|l| l.id);
                if align != cur_align {
                    set_align(&mut out, &mut cur_align, align);
                } else if !out.is_empty() {
                    let tight = this_list.is_some() && this_list == prev_list && list_marker.is_some();
                    out.push_str(if tight { "\n" } else { "\n\n" });
                }
                prev_list = this_list;
                match (&p.kind, &p.list) {
                    (Kind::Heading(n), _) => {
                        let one_line = text.replace('\n', " ");
                        out.push_str(&format!("{} {}", "#".repeat(*n as usize), one_line.trim()));
                    }
                    (k @ (Kind::Title | Kind::Subtitle), _) => {
                        let tk = if *k == Kind::Title { TitleKind::Title } else { TitleKind::Subtitle };
                        let one_line = text.replace('\n', " ");
                        out.push_str(&format!("{} {} {}", "#".repeat(tk.level() as usize), one_line.trim(), tk.marker()));
                    }
                    (_, Some(l)) => {
                        let indent = "    ".repeat(l.level);
                        let (marker, mlen) = match &l.marker {
                            Some(m) => (m.clone(), m.len()),
                            None => (String::new(), 2),
                        };
                        let cont = format!("{indent}{}", " ".repeat(mlen));
                        let mut lines = text.lines();
                        let first = lines.next().unwrap_or("");
                        if l.marker.is_some() {
                            out.push_str(&format!("{indent}{marker}{}", escape_line_start(first)));
                        } else {
                            out.push_str(&format!("{cont}{}", escape_line_start(first)));
                        }
                        for line in lines {
                            out.push('\n');
                            out.push_str(&cont);
                            out.push_str(line);
                        }
                    }
                    (Kind::Quote, None) => {
                        let q: Vec<String> = text.lines().map(|l| format!("> {}", escape_line_start(l))).collect();
                        out.push_str(&q.join("\n"));
                    }
                    (Kind::Code, None) => {
                        let fence = if text.contains("```") { "~~~~" } else { "```" };
                        out.push_str(&format!("{fence}\n{}\n{fence}", text.trim_matches('\n')));
                    }
                    _ => {
                        let lines: Vec<String> = text.lines().map(escape_line_start).collect();
                        out.push_str(&lines.join("\n"));
                    }
                }
            }
            Block::Raw(r) => {
                prev_list = None;
                if cur_align != Align::Left {
                    set_align(&mut out, &mut cur_align, Align::Left);
                } else if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(r);
            }
            Block::Rule => {
                prev_list = None;
                if cur_align != Align::Left {
                    set_align(&mut out, &mut cur_align, Align::Left);
                } else if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str("---");
            }
            Block::Table(tbl) => {
                prev_list = None;
                let mut rows = tbl.rows.clone();
                if rows[0].iter().all(|c| c.is_empty() || (c.len() > 4 && c.starts_with("**") && c.ends_with("**"))) {
                    for c in rows[0].iter_mut() {
                        if c.len() > 4 {
                            *c = c[2..c.len() - 2].to_owned();
                        }
                    }
                }
                if cur_align != Align::Left {
                    set_align(&mut out, &mut cur_align, Align::Left);
                } else if !out.is_empty() {
                    out.push_str("\n\n");
                }
                let narrow = text_w.is_some_and(|tw| tw > 0.0 && tbl.widths.iter().sum::<f64>() < tw * 0.9);
                let widths: &[f64] = if narrow { &[] } else { &tbl.widths };
                out.push_str(&markup::table_markdown(&rows, &tbl.spans, widths));
            }
        }
    }
    if cur_align != Align::Left {
        out.push_str("\n\n</div>");
    }
    out.push('\n');
    out
}

/// Sidmallarnas sida och spalter som avsnittsegenskaper (mot dokumentets grundformat `d`), per namn.
fn master_props(st: &Styles, styles_root: &Node, d: &DocDefaults, ctx: &mut hf::HfCtx<'_, '_>) -> (HashMap<String, sections::SectionProps>, Option<String>) {
    let mut out = HashMap::new();
    let Some(masters) = styles_root.child("office:master-styles") else { return (out, None) };
    let all: Vec<&Node> = masters.els().filter(|m| m.name == "style:master-page").collect();
    let start = all.iter().find(|m| m.attr("style:name") == Some("Standard")).or(all.first()).and_then(|m| m.attr("style:name")).map(str::to_owned);
    let Some(auto) = styles_root.child("office:automatic-styles") else { return (out, start) };
    for m_node in all {
        let m = m_node;
        let (Some(name), Some(layout)) = (m.attr("style:name"), m.attr("style:page-layout-name")) else { continue };
        let Some(layout_node) = auto.els().find(|n| n.name == "style:page-layout" && n.attr("style:name") == Some(layout)) else { continue };
        let Some(pp) = layout_node.child("style:page-layout-properties") else { continue };
        let len = |k: &str| pp.attr(k).and_then(style::parse_length);
        let mut p = sections::SectionProps::default();
        if let (Some(w), Some(h)) = (len("fo:page-width"), len("fo:page-height")) {
            p.page_size = Some((w, h)).filter(|s| !style::same_size(*s, d.page()));
        }
        let all_m = len("fo:margin");
        let def = style::DEFAULT_MARGIN;
        let mm = [
            len("fo:margin-top").or(all_m),
            len("fo:margin-right").or(all_m),
            len("fo:margin-bottom").or(all_m),
            len("fo:margin-left").or(all_m),
        ];
        if mm.iter().any(Option::is_some) {
            let mut m = [mm[0].unwrap_or(def), mm[1].unwrap_or(def), mm[2].unwrap_or(def), mm[3].unwrap_or(def)];
            // Sidhuvud och sidfot skjuter ned brödtexten (som i grundformatet, se `hf::read_master_hf`).
            hf::add_hf_extent(st, m_node, layout_node, &mut m, ctx);
            p.page_margin = Some(m).filter(|m| m.iter().zip(d.margins()).any(|(a, b)| (a - b).abs() > 0.6));
        }
        if let Some(def) = pp.child("style:columns").and_then(columns_of) {
            let (pw, _) = p.page_size.unwrap_or_else(|| d.page());
            let m = p.page_margin.unwrap_or_else(|| d.margins());
            def.apply(&mut p, (pw - m[1] - m[3]).max(1.0));
        }
        if start.as_deref() != Some(name) {
            p.hf = hf::master_hf(st, m, d, ctx);
        }
        out.insert(name.to_owned(), p);
    }
    (out, start)
}

/// `text:notes-configuration`: talformat och första nummer för fotnoter och slutnoter (`start-value` är 0-baserat).
fn read_notes_config(styles_root: &Node, d: &mut DocDefaults) {
    let Some(os) = styles_root.child("office:styles") else { return };
    for n in os.els().filter(|n| n.name == "text:notes-configuration") {
        let st = match n.attr("text:note-class") {
            Some("footnote") => &mut d.notes.foot,
            Some("endnote") => &mut d.notes.end,
            _ => continue,
        };
        if let Some(f) = n.attr("style:num-format").and_then(crate::word::notefmt::NoteFmt::from_odf) {
            st.fmt = f;
        }
        if let Some(v) = n.attr("text:start-value").and_then(|v| v.trim().parse::<u32>().ok()) {
            st.start = v.saturating_add(1);
        }
    }
}

fn read_page(st: &Styles, styles_root: &Node, d: &mut DocDefaults, ctx: &mut hf::HfCtx<'_, '_>) {
    let Some(masters) = styles_root.child("office:master-styles") else { return };
    let mut it = masters.els().filter(|m| m.name == "style:master-page");
    let all: Vec<&Node> = it.by_ref().collect();
    let Some(master) = all.iter().find(|m| m.attr("style:name") == Some("Standard")).or(all.first()).copied() else { return };
    if let (Some(layout), Some(auto)) = (master.attr("style:page-layout-name"), styles_root.child("office:automatic-styles")) {
        if let Some(pl) = auto.els().find(|n| n.name == "style:page-layout" && n.attr("style:name") == Some(layout)) {
            if let Some(pp) = pl.child("style:page-layout-properties") {
                let len = |k: &str| pp.attr(k).and_then(style::parse_length);
                if let (Some(w), Some(h)) = (len("fo:page-width"), len("fo:page-height")) {
                    d.page_size = Some((w, h));
                }
                d.page_start = pp.attr("style:first-page-number").and_then(|v| v.trim().parse().ok());
                d.page_format = pp.attr("style:num-format").and_then(crate::hf::NumFmt::from_name).filter(|f| *f != crate::hf::NumFmt::Decimal);
                let all_m = len("fo:margin");
                let m = [
                    len("fo:margin-top").or(all_m),
                    len("fo:margin-right").or(all_m),
                    len("fo:margin-bottom").or(all_m),
                    len("fo:margin-left").or(all_m),
                ];
                if m.iter().any(Option::is_some) {
                    let def = style::DEFAULT_MARGIN;
                    d.page_margin = Some([m[0].unwrap_or(def), m[1].unwrap_or(def), m[2].unwrap_or(def), m[3].unwrap_or(def)]);
                }
            }
        }
    }
    let layout = master.attr("style:page-layout-name").zip(styles_root.child("office:automatic-styles")).and_then(|(l, auto)| auto.els().find(|n| n.name == "style:page-layout" && n.attr("style:name") == Some(l)));
    hf::read_master_hf(st, master, layout, d, ctx);
}

/// Läser ett OpenDocument-textdokument som Markdown.
/// Sätter svarens markörer intill trådens egna (`<!--k:c:0--><!--k:c:1-->…<!--k:/c:0--><!--k:/c:1-->`).
fn attach_replies(md: &str, comments: &[CommentRec]) -> String {
    let mut out = md.to_owned();
    for root in comments.iter().filter(|c| c.parent.is_none()) {
        let replies: Vec<u32> = comments.iter().filter(|c| c.parent == Some(root.id)).map(|c| c.id).collect();
        if replies.is_empty() {
            continue;
        }
        let (start, end) = (review::marker_start(root.id), review::marker_end(root.id));
        let starts: String = replies.iter().map(|&r| review::marker_start(r)).collect();
        let ends: String = replies.iter().map(|&r| review::marker_end(r)).collect();
        out = out.replacen(&start, &format!("{start}{starts}"), 1).replacen(&end, &format!("{end}{ends}"), 1);
    }
    out
}

/// Förarbete: ändringsregionerna (`text:tracked-changes`) och namnen på kommentarsområdenas slut.
fn read_regions(body: &Node, rv: &mut ReviewRd, dropped: &mut Dropped) {
    fn ends(n: &Node, out: &mut std::collections::HashSet<String>) {
        for c in n.els() {
            if c.name == "office:annotation-end" {
                if let Some(name) = c.attr("office:name") {
                    out.insert(name.to_owned());
                }
            }
            ends(c, out);
        }
    }
    ends(body, &mut rv.ends);
    for tc in body.els().filter(|c| c.name == "text:tracked-changes") {
        for reg in tc.els().filter(|c| c.name == "text:changed-region") {
            let Some(id) = reg.attr("text:id").or_else(|| reg.attr("xml:id")) else { continue };
            for k in reg.els() {
                let kind = match k.name.as_str() {
                    "text:insertion" => RevKind::Ins,
                    "text:deletion" => RevKind::Del,
                    "text:format-change" => {
                        dropped.format_changes = true;
                        continue;
                    }
                    _ => continue,
                };
                let info = k.child("office:change-info");
                let field = |n: &str| info.and_then(|i| i.child(n)).map(|c| c.deep_text().trim().to_owned()).unwrap_or_default();
                let paras = if kind == RevKind::Del {
                    k.els().filter(|c| matches!(c.name.as_str(), "text:p" | "text:h")).cloned().collect()
                } else {
                    Vec::new()
                };
                rv.regions.insert(id.to_owned(), Region { kind, author: field("dc:creator"), date: field("dc:date"), paras });
            }
        }
    }
}

pub fn read(bytes: &[u8]) -> Result<Loaded, String> {
    if !bytes.starts_with(b"PK") {
        return Err(crate::i18n::t("Filen är inte ett OpenDocument-textdokument (.odt).").into());
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(crate::i18n::t("Dokumentet är för stort för att öppnas.").into());
    }
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| crate::tf!("OpenDocument-filen är skadad: {e}", e = e))?;
    if zip.len() > MAX_ENTRIES {
        return Err(crate::i18n::t("Dokumentet innehåller orimligt många delar och öppnas inte.").into());
    }
    let mut pkg = Pkg { zip, total: 0 };
    if let Some(m) = pkg.text("mimetype")? {
        if !m.trim().starts_with("application/vnd.oasis.opendocument.text") {
            return Err(crate::i18n::t("Filen är inte ett OpenDocument-textdokument (.odt).").into());
        }
    }
    if pkg.text("META-INF/manifest.xml")?.is_some_and(|m| m.contains("encryption-data")) {
        return Err(crate::i18n::t("Dokumentet är lösenordsskyddat. Öppna det i LibreOffice och spara det utan lösenord.").into());
    }
    let content = pkg
        .text("content.xml")?
        .ok_or_else(|| "OpenDocument-filen saknar sitt innehåll (content.xml).".to_owned())?;
    let croot = parse_xml(&content)?;
    drop(content);
    let sroot = match pkg.text("styles.xml")? {
        Some(x) => Some(parse_xml(&x)?),
        None => None,
    };

    let mut st = Styles::default();
    if let Some(s) = &sroot {
        st.read_fonts(s);
    }
    st.read_fonts(&croot);
    if let Some(s) = &sroot {
        st.read_styles(s);
    }
    st.read_styles(&croot);

    let base = st.default_props("paragraph").over(&st.tprops("paragraph", "Standard"));
    let body = croot
        .child("office:body")
        .and_then(|b| b.child("office:text"))
        .ok_or_else(|| crate::i18n::t("Filen är inte ett OpenDocument-textdokument (.odt).").to_owned())?;

    let mut defaults = DocDefaults { font_family: base.font.clone(), font_size: base.size, ..Default::default() };
    // Formatmallar som avviker från Kladds egna (de Kladd själv skriver ger inga ändringar).
    defaults.styles = st.style_set().minus(&kladd_style_set());
    let mut hf_images: Vec<(String, Image)> = Vec::new();
    let mut hf_by_target: HashMap<String, String> = HashMap::new();
    let (masters, start_master) = {
        let mut ctx = hf::HfCtx { pkg: &mut pkg, images: &mut hf_images, by_target: &mut hf_by_target };
        if let Some(s) = &sroot {
            read_page(&st, s, &mut defaults, &mut ctx);
            read_notes_config(s, &mut defaults);
        }
        sroot.as_ref().map(|s| master_props(&st, s, &defaults, &mut ctx)).unwrap_or_default()
    };
    let mut rd = Rd {
        st: &st,
        base: base.clone(),
        pkg,
        dropped: Dropped::default(),
        images: hf_images,
        by_target: hf_by_target,
        blocks: Vec::new(),
        list_id: 0,
        notes: Vec::new(),
        in_note: false,
        pending_boxes: Vec::new(),
        in_box: false,
        cur_page: start_master.as_ref().and_then(|m| masters.get(m)).cloned().unwrap_or_default(),
        cur_master: start_master,
        page_defaults: defaults.clone(),
        masters,
        review: ReviewRd::default(),
    };
    read_regions(body, &mut rd.review, &mut rd.dropped);
    for c in body.els() {
        rd.walk(c);
    }
    rd.flush_boxes();
    let text_w = {
        let (w, _) = defaults.page();
        let m = defaults.margins();
        f64::from(w - m[1] - m[3])
    };
    let mut markdown = render(&rd.blocks, Some(text_w));
    if !rd.notes.is_empty() {
        markdown.push('\n');
        // Fotnoterna först (1..n), sedan slutnoterna (`ei`, `eii` …).
        for end in [false, true] {
            for (i, (_, n)) in rd.notes.iter().filter(|x| x.0 == end).enumerate() {
                let label = if end { crate::word::markup::endnote_label(i + 1) } else { (i + 1).to_string() };
                markdown.push_str(&format!("[^{label}]: {n}\n"));
            }
        }
    }
    if !rd.review.revs.is_empty() || !rd.review.comments.is_empty() {
        markdown = attach_replies(&markdown, &rd.review.comments);
        markdown = review::renumber(&review::prune(&review::merge_adjacent(&review::put_records(&markdown, &rd.review.revs, &rd.review.comments))));
    }
    let comment = defaults.to_comment();
    if !comment.is_empty() {
        markdown = format!("{comment}\n\n{markdown}");
    }
    Ok(Loaded { source: None, markdown, images: rd.images, dropped: rd.dropped.list() })
}

// ============================================================== skrivning

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

fn fmt_num(v: f32, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

/// Ett cellformat med skuggning, ramar, marginaler och lodrät justering ur tabellformatet.
fn odt_cell_style(f: &crate::word::tablefmt::TableFmt, name: &str, r: usize, c: usize, nrows: usize, last_col: bool) -> String {
    use crate::word::tablefmt::{BOTTOM, BStyle, LEFT, RIGHT, TOP, VAlign};
    let own = |side: usize| f.own_border(r, c, side);
    let b = |b: crate::word::tablefmt::Border| {
        if !b.visible() {
            return "none".to_owned();
        }
        let style = match b.style {
            BStyle::Double => "double",
            BStyle::Dashed => "dashed",
            BStyle::Dotted => "dotted",
            _ => "solid",
        };
        let col = b.color.map_or("#000000".to_owned(), |c| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]));
        format!("{:.2}pt {style} {col}", b.pt())
    };
    let mut a = format!(
        "fo:border-top=\"{}\" fo:border-left=\"{}\" fo:border-bottom=\"{}\" fo:border-right=\"{}\" fo:padding-left=\"{}\" fo:padding-right=\"{}\" fo:padding-top=\"{}\" fo:padding-bottom=\"{}\"",
        b(f.edge(TOP, None, own(TOP), r > 0)),
        b(f.edge(LEFT, None, own(LEFT), c > 0)),
        b(f.edge(BOTTOM, own(BOTTOM), None, r + 1 < nrows)),
        b(f.edge(RIGHT, own(RIGHT), None, !last_col)),
        cm(f.pad[0]),
        cm(f.pad[1]),
        cm(f.pad[2]),
        cm(f.pad[3])
    );
    if let Some(x) = f.fill(r, c) {
        a.push_str(&format!(" fo:background-color=\"#{:02X}{:02X}{:02X}\"", x[0], x[1], x[2]));
    }
    match f.valign(r, c) {
        VAlign::Center => a.push_str(" style:vertical-align=\"middle\""),
        VAlign::Bottom => a.push_str(" style:vertical-align=\"bottom\""),
        VAlign::Top => {}
    }
    format!("<style:style style:name=\"{name}\" style:family=\"table-cell\"><style:table-cell-properties {a}/></style:style>")
}

fn cm(pt: f32) -> String {
    format!("{}cm", fmt_num(pt * 2.54 / 72.0, 4))
}

/// `style:columns` för ett avsnitt. Olika breda spalter skrivs som `style:column` med `style:rel-width`
/// i twips (spaltens bredd plus utfyllnad) och halva mellanrummet på var sida, skiljelinjen först.
fn columns_xml(sec: &sections::Section) -> String {
    let mut x = format!("<style:columns fo:column-count=\"{}\" fo:column-gap=\"{}\">", sec.columns, cm(sec.gap));
    if sec.rule {
        x.push_str("<style:column-sep style:width=\"0.0176cm\" style:color=\"#000000\" style:style=\"solid\" style:height=\"100%\" style:vertical-align=\"top\"/>");
    }
    if sec.col_widths.len() == sec.columns {
        let n = sec.columns as f32;
        let text_w = (sec.page.0 - sec.margin[1] - sec.margin[3]).max(1.0);
        let avail = (text_w - sec.gap * (n - 1.0)).max(1.0);
        let sum: f32 = sec.col_widths.iter().sum();
        let half = cm(sec.gap / 2.0);
        for (i, cw) in sec.col_widths.iter().enumerate() {
            let (first, last) = (i == 0, i + 1 == sec.columns);
            let w = cw / sum * avail;
            // Ytterkanterna har ingen utfyllnad, så att bredderna summerar till textytans bredd.
            let pads = sec.gap / 2.0 * (if first { 0.0 } else { 1.0 } + if last { 0.0 } else { 1.0 });
            let rel = ((w + pads) * 20.0).round() as i64;
            let (sp, ep) = (if first { "0cm".to_owned() } else { half.clone() }, if last { "0cm".to_owned() } else { half.clone() });
            x.push_str(&format!("<style:column style:rel-width=\"{rel}*\" fo:start-padding=\"{sp}\" fo:end-padding=\"{ep}\"/>"));
        }
    }
    x.push_str("</style:columns>");
    x
}

fn pt_str(s: f32) -> String {
    let r = (s * 2.0).round() / 2.0;
    format!("{}pt", fmt_num(r, 1))
}

fn font_family_attr(name: &str) -> String {
    if name.contains(char::is_whitespace) { format!("'{name}'") } else { name.to_owned() }
}

fn font_decls(fonts: &BTreeSet<String>) -> String {
    let mut s = String::from("<office:font-face-decls>");
    for f in fonts {
        s.push_str(&format!(
            "<style:font-face style:name=\"{}\" svg:font-family=\"{}\"/>",
            esc(f),
            esc(&font_family_attr(f))
        ));
    }
    s.push_str("</office:font-face-decls>");
    s
}

fn align_attr(a: Align) -> &'static str {
    match a {
        Align::Left => "start",
        Align::Center => "center",
        Align::Right => "end",
        Align::Justify => "justify",
    }
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    // `s` är det som står efter `<img`.
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        while i < b.len() && (b[i].is_whitespace() || b[i] == '/') {
            i += 1;
        }
        if i >= b.len() || b[i] == '>' {
            break;
        }
        let start = i;
        while i < b.len() && !b[i].is_whitespace() && !matches!(b[i], '=' | '>' | '/') {
            i += 1;
        }
        let name: String = b[start..i].iter().collect::<String>().to_ascii_lowercase();
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == '=' {
            i += 1;
            while i < b.len() && b[i].is_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == '"' || b[i] == '\'') {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    val.push(b[i]);
                    i += 1;
                }
                i += 1;
            } else {
                while i < b.len() && !b[i].is_whitespace() && b[i] != '>' {
                    val.push(b[i]);
                    i += 1;
                }
            }
        }
        if !name.is_empty() {
            out.push((name, val.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")));
        }
    }
    out
}

struct WTable {
    in_head: bool,
    /// Spann per rad och cell ur tabellens källa (0 = täckt cell).
    spans: Vec<Vec<usize>>,
    /// Antal påbörjade rader och cellen i raden som skrivs.
    rows: usize,
    cell: usize,
    covered: bool,
    /// Tabellens format (skuggning, ramar …) och tabellnumret för cellformaten.
    fmt: Option<crate::word::tablefmt::TableFmt>,
    n: usize,
}

struct W<'a> {
    resolve: &'a dyn Fn(&str) -> Option<Image>,
    body: String,
    auto: String,
    fonts: BTreeSet<String>,
    text_styles: HashMap<String, String>,
    para_styles: HashMap<(String, &'static str), String>,
    n_text: usize,
    n_para: usize,
    n_list: usize,
    n_table: usize,
    n_frame: usize,
    /// Sluttaggen för det öppna stycket.
    open: Option<&'static str>,
    /// Förra tecknet var ett vanligt mellanslag (nästa mellanslag måste då skrivas med `text:s`).
    after_space: bool,
    bold: u32,
    italic: u32,
    strike: u32,
    underline: u32,
    link: Option<String>,
    link_open: bool,
    spans: Vec<SpanStyle>,
    aligns: Vec<Align>,
    heading: Option<u8>,
    title: Option<TitleKind>,
    quote: u32,
    code: bool,
    table: Option<WTable>,
    cell_style: bool,
    /// Tabellformatet (`<!-- kladd-tabell: … -->`) som väntar på tabellen efter det.
    pending_tfmt: Option<crate::word::tablefmt::TableFmt>,
    frame_style: bool,
    /// Grafikformat för förankrade/beskurna bilder: egenskaperna och formatets namn.
    frame_styles: Vec<(String, String)>,
    image: Option<(String, String)>,
    media: Vec<(String, Vec<u8>, &'static str)>,
    missing: Vec<String>,
    text_w: f32,
    /// Fotnotsdefinitionerna (etikett, stycken som XML) i textens ordning.
    notes: Vec<(String, String)>,
    /// Etiketten för varje fotnotsreferens i dokumentordning; platshållaren i texten är dess index.
    note_order: Vec<String>,
    in_note: bool,
    /// Rubrikerna H1–H3 (nivå, text, uppskattad sida) till innehållsförteckningen.
    toc: Vec<(u8, String, usize)>,
    /// Avsnitt: dokumentets grundformat, första arkets mått, arket som gäller just nu, sidmallarna
    /// utöver "Standard" (M1, M2 …), ett öppet `text:section` och en brytning som väntar på nästa stycke
    /// (sidmallens namn, "column" eller "page").
    defaults: DocDefaults,
    first: sections::Section,
    cur: sections::Section,
    masters: Vec<sections::Section>,
    section_open: bool,
    n_sect: usize,
    pending_break: Option<String>,
    /// Sidnumret som avsnittet efter brytningen börjar om på (`style:page-number`).
    pending_start: Option<u32>,
    /// Öppna textrutor (`<!--k:tb N-->`): var rutans innehåll börjar i brödtexten och ramen (första `k:pp`).
    boxes: Vec<(usize, Option<crate::word::pbox::PPr>, crate::docx_tbox::Head)>,
    review: ReviewWr,
}

/// En ändringsregion som skrivs: slag, post och (för raderingar) det raderade innehållet som XML.
struct WRegion {
    id: String,
    kind: RevKind,
    rec: RevRec,
    content: String,
}

/// Granskningsläget vid skrivning: posterna, ändringen som gäller just nu, regionerna och kommentarerna.
#[derive(Default)]
struct ReviewWr {
    revs: HashMap<u32, RevRec>,
    comments: HashMap<u32, CommentRec>,
    /// Ändringen som texten ligger i (`<ins>`/`<del>`).
    cur: Option<(RevKind, u32)>,
    regions: Vec<WRegion>,
    /// Den öppna infogningen (ändringens nummer, regionens plats) och den senaste raderingen.
    ins_open: Option<(u32, usize)>,
    del_last: Option<(u32, usize)>,
    started: std::collections::HashSet<u32>,
    ended: std::collections::HashSet<u32>,
    /// Svar (delar förälderns område och får ingen egen slutmarkör).
    replies: std::collections::HashSet<u32>,
}

fn heading_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn list_style_xml(name: &str, ordered: bool, start: u64) -> String {
    let mut s = format!("<text:list-style style:name=\"{name}\">");
    for lvl in 1..=10 {
        let margin = 1.27 * lvl as f32;
        let props = format!(
            "<style:list-level-properties text:list-level-position-and-space-mode=\"label-alignment\"><style:list-level-label-alignment text:label-followed-by=\"listtab\" text:list-tab-stop-position=\"{m}cm\" fo:text-indent=\"-0.635cm\" fo:margin-left=\"{m}cm\"/></style:list-level-properties>",
            m = fmt_num(margin, 3)
        );
        if ordered {
            s.push_str(&format!(
                "<text:list-level-style-number text:level=\"{lvl}\" style:num-suffix=\".\" style:num-format=\"1\" text:start-value=\"{start}\">{props}</text:list-level-style-number>"
            ));
        } else {
            s.push_str(&format!(
                "<text:list-level-style-bullet text:level=\"{lvl}\" text:bullet-char=\"•\">{props}</text:list-level-style-bullet>"
            ));
        }
    }
    s.push_str("</text:list-style>");
    s
}

impl<'a> W<'a> {
    fn new(resolve: &'a dyn Fn(&str) -> Option<Image>, text_w: f32) -> Self {
        W {
            resolve,
            body: String::new(),
            auto: String::new(),
            fonts: BTreeSet::new(),
            text_styles: HashMap::new(),
            para_styles: HashMap::new(),
            n_text: 0,
            n_para: 0,
            n_list: 0,
            n_table: 0,
            n_frame: 0,
            open: None,
            after_space: true,
            bold: 0,
            italic: 0,
            strike: 0,
            underline: 0,
            link: None,
            link_open: false,
            spans: Vec::new(),
            aligns: Vec::new(),
            heading: None,
            title: None,
            quote: 0,
            code: false,
            table: None,
            cell_style: false,
            pending_tfmt: None,
            frame_style: false,
            frame_styles: Vec::new(),
            image: None,
            media: Vec::new(),
            missing: Vec::new(),
            text_w,
            notes: Vec::new(),
            note_order: Vec::new(),
            in_note: false,
            toc: Vec::new(),
            defaults: DocDefaults::default(),
            first: sections::Section::resolve(&sections::SectionProps::default(), &DocDefaults::default()),
            cur: sections::Section::resolve(&sections::SectionProps::default(), &DocDefaults::default()),
            masters: Vec::new(),
            section_open: false,
            n_sect: 0,
            pending_break: None,
            pending_start: None,
            boxes: Vec::new(),
            review: ReviewWr::default(),
        }
    }

    fn region(&mut self, kind: RevKind, id: u32) -> usize {
        let rec = self.review.revs.get(&id).cloned().unwrap_or(RevRec { id, ..Default::default() });
        let n = self.review.regions.len() + 1;
        self.review.regions.push(WRegion { id: format!("ct{n}"), kind, rec, content: String::new() });
        self.review.regions.len() - 1
    }

    /// Öppnar en infogning för ändring `want` (och stänger en annan som är öppen); `None` stänger.
    fn sync_ins(&mut self, want: Option<u32>) {
        if self.review.ins_open.map(|(id, _)| id) == want {
            return;
        }
        self.end_ins();
        if let Some(id) = want {
            let idx = self.region(RevKind::Ins, id);
            self.body.push_str(&format!("<text:change-start text:change-id=\"{}\"/>", self.review.regions[idx].id));
            self.review.ins_open = Some((id, idx));
        }
    }

    fn end_ins(&mut self) {
        if let Some((_, idx)) = self.review.ins_open.take() {
            self.body.push_str(&format!("<text:change-end text:change-id=\"{}\"/>", self.review.regions[idx].id));
        }
    }

    /// Raderad text: den ligger i ändringsregionen, och `text:change` markerar platsen.
    fn run_deleted(&mut self, id: u32, text: &str, code: bool) {
        let style = if self.code { None } else { self.text_style(code) };
        let t = self.odt_text(text);
        let x = match style {
            Some(s) => format!("<text:span text:style-name=\"{s}\">{t}</text:span>"),
            None => t,
        };
        // Länken hör till den raderade texten (annars tappas den när regionen skrivs).
        let x = match &self.link {
            Some(url) => format!(
                "<text:a xlink:type=\"simple\" xlink:href=\"{}\" text:style-name=\"Internet_20_link\" text:visited-style-name=\"Visited_20_Internet_20_Link\">{x}</text:a>",
                esc(url)
            ),
            None => x,
        };
        match self.review.del_last {
            Some((last, idx)) if last == id => self.review.regions[idx].content.push_str(&x),
            _ => {
                self.end_ins();
                let idx = self.region(RevKind::Del, id);
                self.review.regions[idx].content = x;
                self.body.push_str(&format!("<text:change text:change-id=\"{}\"/>", self.review.regions[idx].id));
                self.review.del_last = Some((id, idx));
            }
        }
    }

    /// Kommentarsområdets början (`office:annotation`) eller slut (`office:annotation-end`).
    fn comment_marker(&mut self, start: bool, id: u32) {
        let Some(rec) = self.review.comments.get(&id).cloned() else { return };
        if self.in_note {
            return;
        }
        if start {
            if !self.review.started.insert(id) {
                return;
            }
            self.open_para();
            self.review.del_last = None;
            let mut attrs = format!(" office:name=\"kladd-{id}\"");
            if let Some(p) = rec.parent.filter(|p| self.review.started.contains(p)) {
                self.review.replies.insert(id);
                attrs.push_str(&format!(" loext:parent-name=\"kladd-{p}\""));
            }
            if rec.done {
                attrs.push_str(" loext:resolved=\"true\"");
            }
            let mut x = format!("<office:annotation{attrs}>");
            if !rec.author.is_empty() {
                x.push_str(&format!("<dc:creator>{}</dc:creator>", esc(&rec.author)));
            }
            if !rec.date.is_empty() {
                x.push_str(&format!("<dc:date>{}</dc:date>", esc(&rec.date)));
            }
            for line in rec.text.split('\n') {
                x.push_str(&format!("<text:p>{}</text:p>", annotation_line(line)));
            }
            x.push_str("</office:annotation>");
            self.body.push_str(&x);
        } else if self.review.started.contains(&id) && !self.review.replies.contains(&id) && self.review.ended.insert(id) {
            self.open_para();
            self.body.push_str(&format!("<office:annotation-end office:name=\"kladd-{id}\"/>"));
        }
    }

    /// `text:tracked-changes` med alla regioner (tom sträng utan ändringar).
    fn tracked_changes(&self) -> String {
        if self.review.regions.is_empty() {
            return String::new();
        }
        let mut s = String::from("<text:tracked-changes text:track-changes=\"false\">");
        for r in &self.review.regions {
            s.push_str(&format!("<text:changed-region xml:id=\"{0}\" text:id=\"{0}\">", r.id));
            let tag = if r.kind == RevKind::Ins { "text:insertion" } else { "text:deletion" };
            s.push_str(&format!("<{tag}><office:change-info>"));
            if !r.rec.author.is_empty() {
                s.push_str(&format!("<dc:creator>{}</dc:creator>", esc(&r.rec.author)));
            }
            if !r.rec.date.is_empty() {
                s.push_str(&format!("<dc:date>{}</dc:date>", esc(&r.rec.date)));
            }
            s.push_str("</office:change-info>");
            if r.kind == RevKind::Del {
                s.push_str(&format!("<text:p text:style-name=\"Standard\">{}</text:p>", r.content));
            }
            s.push_str(&format!("</{tag}></text:changed-region>"));
        }
        s.push_str("</text:tracked-changes>");
        s
    }

    /// Ett automatiskt styckeformat som börjar en ny sida med sidmallen `brk`, eller en ny spalt (`column`)
    /// eller sida (`page`).
    fn break_style(&mut self, base: &str, brk: &str) -> String {
        self.n_para += 1;
        let name = format!("P{}", self.n_para);
        let (attrs, props) = match brk {
            "column" | "page" => (String::new(), format!("<style:paragraph-properties fo:break-before=\"{brk}\"/>")),
            m => {
                let start = self.pending_start.take().map_or(String::new(), |n| format!("<style:paragraph-properties style:page-number=\"{n}\"/>"));
                (format!(" style:master-page-name=\"{m}\""), start)
            }
        };
        self.auto.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"paragraph\" style:parent-style-name=\"{base}\"{attrs}>{props}</style:style>"
        ));
        name
    }

    /// Ett tomt stycke som bär en väntande brytning (för block som inte är stycken).
    fn flush_break(&mut self) {
        if let Some(b) = self.pending_break.take() {
            let style = self.break_style("Text_20_body", &b);
            self.body.push_str(&format!("<text:p text:style-name=\"{style}\"/>"));
        }
    }

    fn open_columns(&mut self, sec: &sections::Section) {
        if sec.columns > 1 {
            self.n_sect += 1;
            let name = format!("Sect{}", self.n_sect);
            self.auto.push_str(&format!(
                "<style:style style:name=\"{name}\" style:family=\"section\"><style:section-properties text:dont-balance-text-columns=\"{}\">{}</style:section-properties></style:style>",
                sec.no_balance,
                columns_xml(sec)
            ));
            self.body.push_str(&format!("<text:section text:style-name=\"{name}\" text:name=\"Avsnitt{}\">", self.n_sect));
            self.section_open = true;
        }
    }

    fn close_text_section(&mut self) {
        if self.section_open {
            self.close_para();
            self.body.push_str("</text:section>");
            self.section_open = false;
        }
    }

    /// En avsnittsrad: första avsnittets egenskaper före all text, annars en ny sidmall (ny sida) eller,
    /// löpande på samma ark, ett nytt `text:section` med spalter.
    fn section_marker(&mut self, props: &sections::SectionProps) {
        self.close_para();
        let sec = sections::Section::resolve(props, &self.defaults);
        if self.body.is_empty() && self.masters.is_empty() {
            self.first = sec.clone();
            self.cur = sec.clone();
            self.open_columns(&sec);
            return;
        }
        self.close_text_section();
        let c = &self.cur;
        let same = (c.page.0 - sec.page.0).abs() < 0.5
            && (c.page.1 - sec.page.1).abs() < 0.5
            && c.margin.iter().zip(sec.margin).all(|(a, b)| (a - b).abs() < 0.5);
        if sec.continuous && same {
            self.cur = sec.clone();
            self.open_columns(&sec);
            return;
        }
        self.masters.push(sec.clone());
        let name = format!("M{}", self.masters.len());
        self.cur = sec.clone();
        self.pending_start = sec.props.hf.page_start;
        if sec.columns > 1 {
            // Spalter ligger i ett avsnitt, som inte får börja med sidbrytningen.
            self.pending_break = Some(name);
            self.flush_break();
            self.open_columns(&sec);
        } else {
            self.pending_break = Some(name);
        }
    }

    fn base_para_style(&self) -> String {
        if self.in_note {
            "Footnote".into()
        } else if let Some(t) = self.title {
            match t {
                TitleKind::Title => "Title".into(),
                TitleKind::Subtitle => "Subtitle".into(),
            }
        } else if let Some(h) = self.heading {
            format!("Heading_20_{h}")
        } else if self.code {
            "Preformatted_20_Text".into()
        } else if let Some(t) = &self.table {
            if t.in_head { "Table_20_Heading".into() } else { "Table_20_Contents".into() }
        } else if self.quote > 0 {
            "Quotations".into()
        } else {
            "Text_20_body".into()
        }
    }

    fn para_style(&mut self) -> String {
        let base = self.base_para_style();
        let align = self.aligns.last().copied().unwrap_or(Align::Left);
        if self.code {
            return base;
        }
        // Står formatmallen justerad (`body: justify`) måste ett vänsterställt stycke säga det själv.
        let base_align = style::STYLE_IDS
            .iter()
            .find(|&&id| odt_style_name(id) == base)
            .and_then(|&id| self.defaults.styles.get(id).align)
            .unwrap_or(Align::Left);
        if align == Align::Left && base_align == Align::Left {
            return base;
        }
        let key = (base.clone(), align.as_attr());
        if let Some(n) = self.para_styles.get(&key) {
            return n.clone();
        }
        self.n_para += 1;
        let name = format!("P{}", self.n_para);
        self.auto.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"paragraph\" style:parent-style-name=\"{base}\"><style:paragraph-properties fo:text-align=\"{}\" style:justify-single-word=\"false\"/></style:style>",
            align_attr(align)
        ));
        self.para_styles.insert(key, name.clone());
        name
    }

    fn open_para(&mut self) {
        if self.open.is_some() {
            return;
        }
        let mut style = self.para_style();
        if let Some(b) = self.pending_break.take() {
            style = self.break_style(&style, &b);
        }
        if let Some(h) = self.heading {
            self.body.push_str(&format!("<text:h text:style-name=\"{style}\" text:outline-level=\"{h}\">"));
            self.open = Some("</text:h>");
        } else {
            self.body.push_str(&format!("<text:p text:style-name=\"{style}\">"));
            self.open = Some("</text:p>");
        }
        self.after_space = true;
    }

    /// En formel som inbäddat formelobjekt (MathML i `Object N/content.xml`). Går den inte att
    /// tolka skrivs källtexten som text.
    fn math(&mut self, src: &str, display: bool) {
        let src = src.trim();
        let (Ok(nodes), Some(f)) = (crate::math::parse::parse(src), crate::math::formula(src, display)) else {
            let d = if display { "$$" } else { "$" };
            return self.run(&format!("{d}{src}{d}"), false);
        };
        if nodes.is_empty() {
            return;
        }
        let n = self.media.iter().filter(|m| m.0.ends_with('/')).count() + 1;
        let dir = format!("Object {n}");
        let ns = crate::math::mathml::NS;
        let content = format!(
            "{XML_DECL}<math:math xmlns:math=\"{ns}\"><math:semantics><math:mrow>{}</math:mrow></math:semantics></math:math>",
            crate::math::mathml::body(&nodes, "math:")
        );
        self.media.push((format!("{dir}/"), Vec::new(), "application/vnd.oasis.opendocument.formula"));
        self.media.push((format!("{dir}/content.xml"), content.into_bytes(), "text/xml"));
        let cm = |em: f32| format!("{:.3}cm", em * 11.0 * 0.0352778);
        // Ersättningsbild för program utan formelmodul (visas om objektet inte kan läsas).
        let mut replacement = String::new();
        if let Some(png) = crate::math::preview::png_exact(src, display, 48.0) {
            let path = format!("ObjectReplacements/{dir}");
            replacement = format!("<draw:image xlink:href=\"./{path}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/>");
            self.media.push((path, png, "image/png"));
        }
        let frame = format!(
            "<draw:frame draw:name=\"Formel{n}\" text:anchor-type=\"as-char\" svg:width=\"{}\" svg:height=\"{}\"><draw:object xlink:href=\"./{dir}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/>{replacement}</draw:frame>",
            cm(f.layout.w.max(0.2)),
            cm((f.layout.asc + f.layout.desc).max(0.5))
        );
        if display {
            self.close_para();
            self.aligns.push(Align::Center);
            self.open_para();
            self.body.push_str(&frame);
            self.close_para();
            self.aligns.pop();
        } else {
            self.open_para();
            self.body.push_str(&frame);
            self.after_space = false;
        }
    }

    fn close_link(&mut self) {
        // `text:change-start`/`-end` är tomma punktmarkörer och behöver inte omsluta länken: en ändring
        // som sträcker sig över en länk förblir en region.
        if self.link_open {
            self.body.push_str("</text:a>");
            self.link_open = false;
        }
    }

    fn close_para(&mut self) {
        if let Some(t) = self.open.take() {
            self.end_ins();
            self.review.del_last = None;
            self.close_link();
            self.body.push_str(t);
        }
    }

    fn text_style(&mut self, code: bool) -> Option<String> {
        let span = self.spans.last().cloned().unwrap_or_default();
        if self.bold + self.italic + self.strike + self.underline == 0 && span.is_empty() && !code {
            return None;
        }
        let key = format!(
            "{}{}{}{}|{code}|{}",
            self.bold > 0,
            self.italic > 0,
            self.strike > 0,
            self.underline > 0,
            span.to_css()
        );
        if let Some(n) = self.text_styles.get(&key) {
            return Some(n.clone());
        }
        self.n_text += 1;
        let name = format!("T{}", self.n_text);
        let mut p = String::new();
        if self.bold > 0 {
            p.push_str(" fo:font-weight=\"bold\" style:font-weight-asian=\"bold\" style:font-weight-complex=\"bold\"");
        }
        if self.italic > 0 {
            p.push_str(" fo:font-style=\"italic\" style:font-style-asian=\"italic\" style:font-style-complex=\"italic\"");
        }
        if self.underline > 0 {
            p.push_str(" style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"");
        }
        if self.strike > 0 {
            p.push_str(" style:text-line-through-style=\"solid\" style:text-line-through-type=\"single\"");
        }
        if let Some(f) = &span.font_family {
            self.fonts.insert(f.clone());
            p.push_str(&format!(" style:font-name=\"{}\"", esc(f)));
        }
        if let Some(s) = span.font_size {
            let s = pt_str(s);
            p.push_str(&format!(" fo:font-size=\"{s}\" style:font-size-asian=\"{s}\" style:font-size-complex=\"{s}\""));
        }
        if let Some(c) = span.color {
            p.push_str(&format!(" fo:color=\"{}\"", style::hex(c)));
        }
        if let Some(c) = span.background {
            p.push_str(&format!(" fo:background-color=\"{}\"", style::hex(c)));
        }
        let parent = if code { " style:parent-style-name=\"Source_20_Text\"" } else { "" };
        self.auto.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"text\"{parent}><style:text-properties{p}/></style:style>"
        ));
        self.text_styles.insert(key, name.clone());
        Some(name)
    }

    /// Text med mellanslag, tabbar och radbrytningar som ODF vill ha dem.
    fn odt_text(&mut self, t: &str) -> String {
        let mut out = String::new();
        let mut pending = 0usize;
        let flush = |out: &mut String, pending: &mut usize| {
            if *pending > 0 {
                out.push_str(&format!("<text:s text:c=\"{pending}\"/>"));
                *pending = 0;
            }
        };
        for c in t.chars() {
            match c {
                ' ' => {
                    if self.after_space {
                        pending += 1;
                    } else {
                        out.push(' ');
                        self.after_space = true;
                    }
                }
                '\t' => {
                    flush(&mut out, &mut pending);
                    out.push_str("<text:tab/>");
                    self.after_space = true;
                }
                '\n' => {
                    flush(&mut out, &mut pending);
                    out.push_str("<text:line-break/>");
                    self.after_space = true;
                }
                '\r' => {}
                c => {
                    flush(&mut out, &mut pending);
                    out.push_str(&esc(&c.to_string()));
                    self.after_space = false;
                }
            }
        }
        flush(&mut out, &mut pending);
        out
    }

    fn open_link_if_needed(&mut self) {
        if let Some(url) = self.link.clone() {
            if !self.link_open {
                self.body.push_str(&format!(
                    "<text:a xlink:type=\"simple\" xlink:href=\"{}\" text:style-name=\"Internet_20_link\" text:visited-style-name=\"Visited_20_Internet_20_Link\">",
                    esc(&url)
                ));
                self.link_open = true;
            }
        }
    }

    fn run(&mut self, text: &str, code: bool) {
        // Den osynliga tecknet-referensen framför en markör i radens början (se `read`).
        let cleaned;
        let text = if text.contains('\u{200B}') {
            cleaned = text.replace('\u{200B}', "");
            cleaned.as_str()
        } else {
            text
        };
        if text.is_empty() {
            return;
        }
        self.open_para();
        let cur = self.review.cur.filter(|_| !self.in_note);
        if let Some((RevKind::Del, id)) = cur {
            self.run_deleted(id, text, code);
            return;
        }
        self.review.del_last = None;
        self.sync_ins(cur.map(|(_, id)| id));
        self.open_link_if_needed();
        let style = if self.code { None } else { self.text_style(code) };
        let t = self.odt_text(text);
        match style {
            Some(s) => self.body.push_str(&format!("<text:span text:style-name=\"{s}\">{t}</text:span>")),
            None => self.body.push_str(&t),
        }
    }

    fn line_break(&mut self) {
        self.open_para();
        self.body.push_str("<text:line-break/>");
        self.after_space = true;
    }

    /// Sökvägen i paketet för en bildfil (en gång per innehåll).
    fn media_path(&mut self, bytes: &[u8], ext: &str, mime: &'static str) -> String {
        match self.media.iter().find(|(_, b, _)| b == bytes) {
            Some((p, _, _)) => p.clone(),
            None => {
                let p = format!("Pictures/image{}.{ext}", self.media.len() + 1);
                self.media.push((p.clone(), bytes.to_vec(), mime));
                p
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn image(&mut self, src: &str, alt: &str, width: Option<f32>, height: Option<f32>, float: Option<Float>, anchor: Option<Anchor>, crop: Option<[f32; 4]>) {
        let Some(img) = (self.resolve)(src).filter(|i| is_supported_image(&i.bytes)) else {
            self.missing.push(src.to_owned());
            let label = if alt.is_empty() { format!("[bild: {src}]") } else { format!("[bild: {alt}]") };
            self.run(&label, false);
            return;
        };
        let ext = image_ext(&img.bytes).unwrap_or("png");
        let mime = match ext {
            "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            _ => "image/png",
        };
        let path = self.media_path(&img.bytes, ext, mime);
        // SVG: filen som vektor och PNG-reserven efter (LibreOffices mönster).
        let svg_path = img.svg.as_ref().map(|s| self.media_path(&s.data, "svg", "image/svg+xml"));
        let natural = img.natural_px();
        let ratio = natural.map(|(w, h)| h / w.max(1.0)).unwrap_or(0.66);
        // Beskuren bild: de synliga proportionerna styr höjden när bara bredden är angiven.
        let ratio = match (natural, crop) {
            (Some((nw, nh)), Some(c)) => nh * (1.0 - (c[1] + c[3]) / 100.0) / (nw * (1.0 - (c[0] + c[2]) / 100.0)).max(1.0),
            _ => ratio,
        };
        let mut w_pt = width
            .map(|w| w * 0.75)
            .or_else(|| img.extent.map(|(cx, _)| cx as f32 / 12700.0))
            .or_else(|| natural.map(|(w, _)| w * 0.75))
            .unwrap_or(self.text_w / 2.0)
            .max(0.5);
        let mut h_pt = match (height, width) {
            (Some(h), Some(_)) => h * 0.75,
            _ => match (width, img.extent) {
                (None, Some((_, cy))) => cy as f32 / 12700.0,
                _ => w_pt * ratio,
            },
        }
        .max(0.5);
        if w_pt > self.text_w {
            h_pt = h_pt * self.text_w / w_pt;
            w_pt = self.text_w;
        }
        if !self.frame_style {
            self.frame_style = true;
            self.auto.push_str("<style:style style:name=\"fr1\" style:family=\"graphic\" style:parent-style-name=\"Graphics\"><style:graphic-properties style:vertical-pos=\"top\" style:vertical-rel=\"baseline\" style:wrap=\"none\" fo:margin-left=\"0cm\" fo:margin-right=\"0cm\"/></style:style>");
        }
        // Förankrad bild och/eller beskärning: eget grafikformat (namnet följer innehållet) och `svg:x`/`svg:y`.
        let mut frame_pos = String::new();
        let custom = if anchor.is_some() || crop.is_some() {
            let mut props = String::from("style:vertical-pos=\"top\" style:vertical-rel=\"baseline\" style:wrap=\"none\"");
            if let Some(a) = anchor {
                let h_rel = match a.hrel {
                    HRel::Column => "paragraph",
                    HRel::Margin => "page-content",
                    HRel::Page => "page",
                };
                let v_rel = match a.vrel {
                    VRel::Paragraph => "paragraph",
                    VRel::Margin => "page-content",
                    VRel::Page => "page",
                };
                let pt = |px: f32| px * 0.75;
                let (hp, x) = match a.h {
                    Pos::Off(o) => ("from-left", Some(o)),
                    Pos::Start => ("left", None),
                    Pos::Center => ("center", None),
                    Pos::End => ("right", None),
                };
                let (vp, y) = match a.v {
                    Pos::Off(o) => ("from-top", Some(o)),
                    Pos::Start => ("top", None),
                    Pos::Center => ("middle", None),
                    Pos::End => ("bottom", None),
                };
                let (wrap, run) = match a.wrap {
                    Wrap::Square => ("parallel", "foreground"),
                    Wrap::TopBottom => ("none", "foreground"),
                    Wrap::Behind => ("run-through", "background"),
                    Wrap::Front => ("run-through", "foreground"),
                };
                let cmv = |px: f32| format!("{:.3}cm", pt(px) * 2.54 / 72.0);
                props = format!(
                    "style:vertical-pos=\"{vp}\" style:vertical-rel=\"{v_rel}\" style:horizontal-pos=\"{hp}\" style:horizontal-rel=\"{h_rel}\" style:wrap=\"{wrap}\" style:number-wrapped-paragraphs=\"no-limit\" style:run-through=\"{run}\" fo:margin-top=\"{}\" fo:margin-right=\"{}\" fo:margin-bottom=\"{}\" fo:margin-left=\"{}\"",
                    cmv(a.dist[0]),
                    cmv(a.dist[1]),
                    cmv(a.dist[2]),
                    cmv(a.dist[3])
                );
                if let Some(x) = x {
                    frame_pos.push_str(&format!(" svg:x=\"{}\"", cmv(x)));
                }
                if let Some(y) = y {
                    frame_pos.push_str(&format!(" svg:y=\"{}\"", cmv(y)));
                }
            }
            if let (Some(c), Some((nw, nh))) = (crop, natural) {
                // `fo:clip` är i bildens egna mått (96 dpi): ovan, höger, under, vänster.
                let cm = |pct: f32, full: f32| format!("{:.3}cm", pct / 100.0 * full * 2.54 / 96.0);
                props.push_str(&format!(" fo:clip=\"rect({}, {}, {}, {})\"", cm(c[1], nh), cm(c[2], nw), cm(c[3], nh), cm(c[0], nw)));
            }
            let idx = match self.frame_styles.iter().position(|(p, _)| *p == props) {
                Some(i) => i,
                None => {
                    self.frame_styles.push((props.clone(), format!("frA{}", self.frame_styles.len() + 1)));
                    let (props, name) = self.frame_styles.last().cloned().unwrap_or_default();
                    self.auto.push_str(&format!("<style:style style:name=\"{name}\" style:family=\"graphic\" style:parent-style-name=\"Graphics\"><style:graphic-properties {props}/></style:style>"));
                    self.frame_styles.len() - 1
                }
            };
            Some((self.frame_styles[idx].1.clone(), if anchor.is_some() { "paragraph" } else { "as-char" }))
        } else {
            None
        };
        // Flytande bild: förankrad vid stycket, vid kanten, text runt med 12 px (0,318 cm) luft.
        let (style_name, anchor) = match float {
            _ if custom.is_some() => custom.unwrap_or(("fr1".into(), "as-char")),
            Some(side) => {
                let (name, pos, wrap, ml, mr) = match side {
                    Float::Left => ("frL", "left", "right", "0cm", "0.318cm"),
                    Float::Right => ("frR", "right", "left", "0.318cm", "0cm"),
                };
                if !self.auto.contains(&format!("style:name=\"{name}\"")) {
                    self.auto.push_str(&format!("<style:style style:name=\"{name}\" style:family=\"graphic\" style:parent-style-name=\"Graphics\"><style:graphic-properties style:vertical-pos=\"top\" style:vertical-rel=\"paragraph\" style:horizontal-pos=\"{pos}\" style:horizontal-rel=\"paragraph\" style:wrap=\"{wrap}\" style:number-wrapped-paragraphs=\"no-limit\" style:run-through=\"foreground\" fo:margin-left=\"{ml}\" fo:margin-right=\"{mr}\" fo:margin-top=\"0cm\" fo:margin-bottom=\"0cm\"/></style:style>"));
                }
                (name.to_owned(), "paragraph")
            }
            None => ("fr1".to_owned(), "as-char"),
        };
        self.open_para();
        self.open_link_if_needed();
        self.n_frame += 1;
        let n = self.n_frame;
        let svg_frame = svg_path
            .map(|p| format!("<draw:image xlink:href=\"{p}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\" draw:mime-type=\"image/svg+xml\"/>"))
            .unwrap_or_default();
        let desc = if alt.is_empty() { String::new() } else { format!("<svg:desc>{}</svg:desc>", esc(alt)) };
        self.body.push_str(&format!(
            "<draw:frame draw:style-name=\"{style_name}\" draw:name=\"Bild{n}\" text:anchor-type=\"{anchor}\"{frame_pos} svg:width=\"{}\" svg:height=\"{}\" draw:z-index=\"{}\">{svg_frame}<draw:image xlink:href=\"{path}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\" draw:mime-type=\"{mime}\"/>{desc}</draw:frame>",
            cm(w_pt),
            cm(h_pt),
            n - 1
        ));
        self.after_space = false;
    }

    /// Skriver innehållsförteckningen: `text:table-of-content` med rubriknivåer 1–3 och en
    /// färdig förteckning (LibreOffice uppdaterar den via Verktyg > Uppdatera).
    fn toc(&mut self) {
        self.flush_break();
        let mut x = String::from("<text:table-of-content text:protected=\"true\" text:name=\"Innehåll1\"><text:table-of-content-source text:outline-level=\"3\"><text:index-title-template text:style-name=\"Contents_20_Heading\">Innehåll</text:index-title-template>");
        for n in 1..=3 {
            x.push_str(&format!(
                "<text:table-of-content-entry-template text:outline-level=\"{n}\" text:style-name=\"Contents_20_{n}\"><text:index-entry-link-start text:style-name=\"Internet_20_link\"/><text:index-entry-chapter/><text:index-entry-text/><text:index-entry-tab-stop style:type=\"right\" style:leader-char=\".\"/><text:index-entry-page-number/><text:index-entry-link-end/></text:table-of-content-entry-template>"
            ));
        }
        x.push_str("</text:table-of-content-source><text:index-body><text:index-title text:name=\"Innehåll1_Head\"><text:p text:style-name=\"Contents_20_Heading\">Innehåll</text:p></text:index-title>");
        for (level, text, page) in &self.toc {
            x.push_str(&format!(
                "<text:p text:style-name=\"Contents_20_{level}\">{}<text:tab/>{page}</text:p>",
                esc(text)
            ));
        }
        x.push_str("</text:index-body></text:table-of-content>");
        self.body.push_str(&x);
    }

    /// Stänger en textruta: stycknen sedan öppningen läggs i en `draw:text-box` i en ram som är förankrad
    /// i ett eget stycke, med rutans fyllning, kant, bredd och läge ur ramens markör.
    fn finish_box(&mut self) {
        use crate::word::pbox::FrameX;
        let Some((start, pp, head)) = self.boxes.pop() else { return };
        let inner = self.body.split_off(start.min(self.body.len()));
        if inner.trim().is_empty() {
            return;
        }
        let pp = pp.unwrap_or_default();
        let frame = pp.frame;
        let bd = &pp.borders;
        let pt = |v: f32| format!("{:.3}pt", v);
        let hex = |c: [u8; 3]| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
        // Vågrätt läge: ankaret i öppningsraden (relativt kolumn, marginal eller sida), annars ramens läge i spalten.
        let (hpos, x, hrel) = match head.anchor {
            Some(a) => {
                let rel = match a.hrel {
                    HRel::Column => "paragraph",
                    HRel::Margin => "page-content",
                    HRel::Page => "page",
                };
                match a.h {
                    Pos::Off(px) => ("from-left", Some(px * 0.75), rel),
                    Pos::Start => ("left", None, rel),
                    Pos::Center => ("center", None, rel),
                    Pos::End => ("right", None, rel),
                }
            }
            None => match frame.map(|f| f.x) {
                Some(FrameX::Center) => ("center", None, "paragraph"),
                Some(FrameX::End) => ("right", None, "paragraph"),
                Some(FrameX::Off(v)) if v != 0.0 => ("from-left", Some(v), "paragraph"),
                _ => ("left", None, "paragraph"),
            },
        };
        let width = frame.map(|f| f.w).filter(|w| *w < 5000.0).unwrap_or(self.text_w).min(self.text_w.max(36.0));
        let border = match (bd.left, bd.left.and_then(|b| b.color)) {
            (Some(b), c) => format!("{} solid {}", pt(f32::from(b.sz) / 8.0), c.map_or("#000000".to_owned(), hex)),
            _ => "none".to_owned(),
        };
        // Luften är avståndet från kanten (inklusive linjen) till texten, så att läsningen ger samma tal.
        let pad = (frame.map_or(0.0, |f| f.pl) - bd.left.map_or(0.0, |b| f32::from(b.sz) / 8.0)).max(0.0);
        let fill = pp.shd.map_or("draw:fill=\"none\"".to_owned(), |c| format!("draw:fill=\"solid\" draw:fill-color=\"{}\"", hex(c)));
        self.n_frame += 1;
        let name = format!("tb{}", self.n_frame);
        // Omflöde, lodrätt läge och avstånd till texten ur öppningsradens ankare (`docx_tbox::Head`); en ruta i
        // flödet ligger vid stycket utan omflöde.
        let anchor = head.anchor;
        let wrap = match anchor.map(|a| a.wrap) {
            Some(Wrap::Square) => "parallel",
            Some(Wrap::Behind | Wrap::Front) => "run-through",
            _ => "none",
        };
        let run_through = if anchor.is_some_and(|a| a.wrap == Wrap::Behind) { "background" } else { "foreground" };
        let (vpos, vrel, svg_y) = match anchor {
            Some(a) => {
                let rel = match a.vrel {
                    VRel::Paragraph => "paragraph",
                    VRel::Margin => "page-content",
                    VRel::Page => "page",
                };
                match a.v {
                    Pos::Off(y) => ("from-top", rel, Some(y * 0.75)),
                    Pos::Start => ("top", rel, None),
                    Pos::Center => ("middle", rel, None),
                    Pos::End => ("bottom", rel, None),
                }
            }
            None => ("top", "paragraph", None),
        };
        let [mt, mr, mb, ml] = anchor.map_or([0.0, 0.0, 5.67, 0.0], |a| [a.dist[0] * 0.75, a.dist[1] * 0.75, a.dist[2] * 0.75, a.dist[3] * 0.75]);
        self.auto.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"graphic\" style:parent-style-name=\"Graphics\"><style:graphic-properties style:wrap=\"{wrap}\" style:vertical-pos=\"{vpos}\" style:vertical-rel=\"{vrel}\" style:horizontal-pos=\"{hpos}\" style:horizontal-rel=\"{hrel}\" style:run-through=\"{run_through}\" fo:margin-top=\"{}\" fo:margin-bottom=\"{}\" fo:margin-left=\"{}\" fo:margin-right=\"{}\" fo:padding=\"{}\" fo:border=\"{border}\" {fill} draw:auto-grow-height=\"true\" draw:textarea-vertical-align=\"top\"/></style:style>",
            pt(mt),
            pt(mb),
            pt(ml),
            pt(mr),
            pt(pad)
        ));
        let svg_x = x.map_or(String::new(), |v| format!(" svg:x=\"{:.5}pt\"", v));
        let svg_y = svg_y.map_or(String::new(), |v| format!(" svg:y=\"{}\"", pt(v)));
        let fh = frame.map_or(0.0, |f| f.h);
        let min_h = if fh > 0.0 { pt(fh) } else if head.h > 0.0 { pt(head.h * 0.75) } else { "0.5cm".to_owned() };
        self.open_para();
        self.body.push_str(&format!(
            "<draw:frame draw:style-name=\"{name}\" draw:name=\"Textruta{}\" text:anchor-type=\"paragraph\"{svg_x}{svg_y} svg:width=\"{}\" fo:min-height=\"{min_h}\"><draw:text-box>{inner}</draw:text-box></draw:frame>",
            self.n_frame,
            pt(width)
        ));
        self.close_para();
    }

    fn html(&mut self, s: &str) {
        let t = s.trim();
        let single_line = t.lines().count() == 1;
        if crate::word::tablefmt::is_comment(t) && self.table.is_none() {
            self.pending_tfmt = crate::word::tablefmt::TableFmt::from_comment(t);
        } else if let Some((start, id)) = review::parse_marker(t) {
            self.comment_marker(start, id);
        } else if let Some(open) = review::parse_open_tag(t) {
            self.review.cur = Some(open).filter(|_| !self.in_note);
        } else if review::is_close_tag(t) {
            self.review.cur = None;
        } else if t == crate::word::nav::TOC_LINE {
            self.close_para();
            self.toc();
        } else if let Some(props) = sections::SectionProps::from_comment(t).filter(|_| self.table.is_none()) {
            self.section_marker(&props);
        } else if sections::is_column_break(t) && self.table.is_none() {
            self.close_para();
            self.pending_break = Some(if self.section_open { "column" } else { "page" }.to_owned());
        } else if sections::is_page_break(t) && self.table.is_none() {
            self.close_para();
            // Ett väntande avsnittsbyte (sidmall) börjar redan en ny sida.
            if self.pending_break.is_none() {
                self.pending_break = Some("page".to_owned());
                // Först i dokumentet bärs brytningen av ett eget tomt stycke: läsaren hoppar
                // över en brytning på första stycket med text, men inte på ett tomt.
                if self.body.is_empty() {
                    self.flush_break();
                }
            }
        } else if let (Some((_, head)), true) = (crate::docx_tbox::parse_head(t), self.table.is_none()) {
            self.close_para();
            self.boxes.push((self.body.len(), None, head));
        } else if t == crate::docx_tbox::CLOSE && !self.boxes.is_empty() {
            self.close_para();
            self.finish_box();
        } else if let Some(pp) = crate::word::pbox::PPr::parse_marker(t) {
            // Styckeutseende: bara ramen i en textruta tas med (rutans fyllning, kant och bredd).
            if let Some(b) = self.boxes.last_mut().filter(|b| b.1.is_none()) {
                b.1 = Some(pp);
            }
        } else if t.eq_ignore_ascii_case("<br>") || t.eq_ignore_ascii_case("<br/>") || t.eq_ignore_ascii_case("<br />") {
            self.line_break();
        } else if let Some(style) = style::parse_span_open(s) {
            let merged = self.spans.last().map_or(style.clone(), |outer| outer.merged(&style));
            self.spans.push(merged);
        } else if style::is_span_close(s) {
            self.spans.pop();
        } else if let Some(align) = style::parse_div_open(s).filter(|_| single_line) {
            self.close_para();
            self.aligns.push(align);
        } else if single_line && style::is_div_close(s) {
            self.close_para();
            self.aligns.pop();
        } else if t.eq_ignore_ascii_case("<u>") {
            self.underline += 1;
        } else if t.eq_ignore_ascii_case("</u>") {
            self.underline = self.underline.saturating_sub(1);
        } else if t.len() > 4 && t[..4].eq_ignore_ascii_case("<img") && t[4..].starts_with(char::is_whitespace) {
            let Some(end) = t.find('>') else {
                self.run(s.trim_end_matches('\n'), false);
                return;
            };
            let attrs = parse_attrs(&t[4..end]);
            let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
            let num = |k: &str| get(k).and_then(|v| v.trim().trim_end_matches("px").trim().parse::<f32>().ok()).filter(|v| *v > 0.0);
            let (src, alt) = (get("src").unwrap_or("").to_owned(), get("alt").unwrap_or("").to_owned());
            let float = match get("align").map(|a| a.trim().to_ascii_lowercase()).as_deref() {
                Some("left") => Some(Float::Left),
                Some("right") => Some(Float::Right),
                _ => None,
            };
            let anchor = Anchor::from_attrs(&|k| get(k).map(str::to_owned));
            let crop = get("crop").and_then(|v| {
                let n: Vec<f32> = v.split_whitespace().filter_map(|s| s.parse::<f32>().ok()).filter(|x| x.is_finite() && (0.0..100.0).contains(x)).collect();
                (n.len() == 4 && n[0] + n[2] < 100.0 && n[1] + n[3] < 100.0 && n.iter().any(|x| *x > 0.0)).then(|| [n[0], n[1], n[2], n[3]])
            });
            self.image(&src, &alt, num("width"), num("height"), float.filter(|_| anchor.is_none()), anchor, crop);
            // Det som följer på nästa rader i samma HTML-block är vanlig text.
            let rest = t[end + 1..].trim_matches('\n');
            if !rest.trim().is_empty() {
                self.line_break();
                self.run(rest.trim_start(), false);
            }
        } else {
            self.run(s.trim_end_matches('\n'), false);
        }
    }

    /// Byter platshållarna mot `text:note`; en andra referens till samma etikett får en kopia.
    fn resolve_notes(&mut self) {
        if self.note_order.is_empty() {
            return;
        }
        let mut out = String::with_capacity(self.body.len());
        let mut rest = self.body.as_str();
        while let Some(i) = rest.find('\u{E004}') {
            out.push_str(&rest[..i]);
            let r = &rest[i + '\u{E004}'.len_utf8()..];
            let Some(e) = r.find('\u{E005}') else {
                rest = "";
                break;
            };
            if let Some(n) = r[..e].parse::<usize>().ok().filter(|n| *n < self.note_order.len()) {
                let label = &self.note_order[n];
                let paras = self
                    .notes
                    .iter()
                    .find(|(l, _)| l == label)
                    .map(|(_, x)| x.as_str())
                    .filter(|x| !x.is_empty())
                    .unwrap_or("<text:p text:style-name=\"Footnote\"/>");
                let k = self.note_order[..=n].iter().filter(|l| !crate::word::markup::is_endnote_label(l)).count();
                if crate::word::markup::is_endnote_label(label) {
                    // Slutnoterna numreras för sig, med romerska tal.
                    let m = self.note_order[..=n].iter().filter(|l| crate::word::markup::is_endnote_label(l)).count();
                    let cit = self.defaults.notes.number(true, m);
                    out.push_str(&format!(
                        "<text:note text:id=\"edn{m}\" text:note-class=\"endnote\"><text:note-citation>{cit}</text:note-citation><text:note-body>{paras}</text:note-body></text:note>"
                    ));
                } else {
                    let cit = self.defaults.notes.number(false, k);
                    out.push_str(&format!(
                        "<text:note text:id=\"ftn{k}\" text:note-class=\"footnote\"><text:note-citation>{cit}</text:note-citation><text:note-body>{paras}</text:note-body></text:note>"
                    ));
                }
            }
            rest = &r[e + '\u{E005}'.len_utf8()..];
        }
        out.push_str(rest);
        self.body = out;
    }

    fn start_list(&mut self, start: Option<u64>) {
        self.close_para();
        self.flush_break();
        self.n_list += 1;
        let name = format!("L{}", self.n_list);
        self.auto.push_str(&list_style_xml(&name, start.is_some(), start.unwrap_or(1)));
        self.body.push_str(&format!("<text:list text:style-name=\"{name}\">"));
    }
}

/// Formatmallens namn i `.odt` för en av Kladds formatmallar.
fn odt_style_name(id: style::StyleId) -> String {
    match id {
        style::StyleId::Body => "Text_20_body".to_owned(),
        style::StyleId::Heading(n) => format!("Heading_20_{n}"),
        style::StyleId::Quote => "Quotations".to_owned(),
        style::StyleId::Title => "Title".to_owned(),
        style::StyleId::Subtitle => "Subtitle".to_owned(),
    }
}

fn build_body<'a>(md: &str, resolve: &'a dyn Fn(&str) -> Option<Image>, text_w: f32, defaults: &DocDefaults, revs: Vec<RevRec>, comments: Vec<CommentRec>) -> W<'a> {
    let mut w = W::new(resolve, text_w);
    w.review.revs = revs.into_iter().map(|r| (r.id, r)).collect();
    w.review.comments = comments.into_iter().map(|c| (c.id, c)).collect();
    w.defaults = defaults.clone();
    w.first = sections::Section::resolve(&sections::SectionProps::default(), defaults);
    w.cur = w.first.clone();
    if md.contains(crate::word::nav::TOC_LINE) {
        let heads = crate::word::nav::headings_scan(md);
        w.toc = crate::word::nav::toc_entries(&heads)
            .into_iter()
            // Grov uppskattning: ca 3 000 tecken per sida (LibreOffice räknar om vid uppdatering).
            .map(|h| (h.level, h.text.clone(), 1 + md[..h.start].chars().count() / 3000))
            .collect();
    }
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_MATH);
    opts.insert(titles::OPTS);
    // Pågående fotnotsdefinition: (etikett, brödtexten som lagts åt sidan).
    let mut note_cur: Option<(String, String)> = None;
    for (ev, range) in Parser::new_ext(md, opts).into_offset_iter() {
        match ev {
            MdEvent::Start(Tag::FootnoteDefinition(label)) => {
                w.close_para();
                note_cur = Some((label.to_string(), std::mem::take(&mut w.body)));
                w.in_note = true;
            }
            MdEvent::End(TagEnd::FootnoteDefinition) => {
                w.close_para();
                if let Some((label, saved)) = note_cur.take() {
                    let xml = std::mem::replace(&mut w.body, saved);
                    w.notes.push((label, xml));
                }
                w.in_note = false;
            }
            MdEvent::FootnoteReference(t) if w.in_note => w.run(&format!("[{t}]"), false),
            MdEvent::FootnoteReference(t) => {
                // Själva noten sätts in när definitionerna lästs (`resolve_notes`).
                w.open_para();
                w.note_order.push(t.to_string());
                let n = w.note_order.len() - 1;
                w.body.push_str(&format!("\u{E004}{n}\u{E005}"));
                w.after_space = false;
            }
            MdEvent::Start(tag) => match tag {
                Tag::Paragraph => w.close_para(),
                Tag::Heading { level, classes, .. } => {
                    w.close_para();
                    // Titel och underrubrik är vanliga stycken med egen mall (inte `text:h`).
                    match titles::kind(&classes) {
                        Some(k) => w.title = Some(k),
                        None => w.heading = Some(heading_num(level)),
                    }
                }
                Tag::BlockQuote(_) => {
                    w.close_para();
                    w.quote += 1;
                }
                Tag::CodeBlock(_) => {
                    w.close_para();
                    w.code = true;
                }
                Tag::List(start) => w.start_list(start),
                Tag::Item => {
                    w.close_para();
                    w.body.push_str("<text:list-item>");
                }
                Tag::Table(aligns) => {
                    w.close_para();
                    w.flush_break();
                    let cols = aligns.len().max(1);
                    w.n_table += 1;
                    let n = w.n_table;
                    if !w.cell_style {
                        w.cell_style = true;
                        w.auto.push_str("<style:style style:name=\"TabellCell\" style:family=\"table-cell\"><style:table-cell-properties fo:padding=\"0.097cm\" fo:border=\"0.5pt solid #000000\"/></style:style>");
                    }
                    let src = markup::table_src(md.get(range.clone()).unwrap_or(""));
                    let tfmt = w.pending_tfmt.take();
                    // Tabellens bredd och läge (`w:tblW`, `w:jc`, `w:tblInd`).
                    let (table_w, align, margin_left) = match &tfmt {
                        Some(f) => {
                            use crate::word::tablefmt::{TAlign, Width};
                            let tw = match f.width {
                                Width::Pct(p) => w.text_w * f32::from(p) / 100.0,
                                Width::Pt(p) => f32::from(p),
                                Width::Auto => w.text_w,
                            }
                            .min(w.text_w);
                            match f.align {
                                TAlign::Center => (tw, "center", None),
                                TAlign::Right => (tw, "right", None),
                                TAlign::Left if f.indent.abs() > 0.01 => (tw, "left", Some(f.indent)),
                                TAlign::Left => (tw, if tw < w.text_w { "left" } else { "margins" }, None),
                            }
                        }
                        None => (w.text_w, "margins", None),
                    };
                    let ml = margin_left.map(|v| format!(" fo:margin-left=\"{}\"", cm(v))).unwrap_or_default();
                    w.auto.push_str(&format!(
                        "<style:style style:name=\"Tabell{n}\" style:family=\"table\"><style:table-properties style:width=\"{tw}\" table:align=\"{align}\"{ml}/></style:style>",
                        tw = cm(table_w)
                    ));
                    let text_w_cols = table_w;
                    w.body.push_str(&format!("<table:table table:name=\"Tabell{n}\" table:style-name=\"Tabell{n}\">"));
                    match src.weights.as_ref().filter(|ws| ws.len() == cols) {
                        // Olika många streck: en kolumnformat per kolumn med proportionell bredd.
                        Some(ws) => {
                            let sum: usize = ws.iter().sum();
                            for (j, &d) in ws.iter().enumerate() {
                                w.auto.push_str(&format!(
                                    "<style:style style:name=\"Tabell{n}.C{j}\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{}\"/></style:style>",
                                    cm(text_w_cols * d as f32 / sum as f32)
                                ));
                                w.body.push_str(&format!("<table:table-column table:style-name=\"Tabell{n}.C{j}\"/>"));
                            }
                        }
                        None => {
                            w.auto.push_str(&format!(
                                "<style:style style:name=\"Tabell{n}.A\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{cw}\"/></style:style>",
                                cw = cm(text_w_cols / cols as f32)
                            ));
                            w.body.push_str(&format!(
                                "<table:table-column table:style-name=\"Tabell{n}.A\" table:number-columns-repeated=\"{cols}\"/>"
                            ));
                        }
                    }
                    w.table = Some(WTable { in_head: false, spans: src.spans, rows: 0, cell: 0, covered: false, fmt: tfmt, n });
                }
                Tag::TableHead => {
                    if let Some(t) = w.table.as_mut() {
                        t.in_head = true;
                        t.rows += 1;
                        t.cell = 0;
                    }
                    w.body.push_str("<table:table-header-rows><table:table-row>");
                }
                Tag::TableRow => {
                    if let Some(t) = w.table.as_mut() {
                        t.rows += 1;
                        t.cell = 0;
                    }
                    w.body.push_str("<table:table-row>");
                }
                Tag::TableCell => {
                    let span = w.table.as_mut().map_or(1, |t| {
                        let span = t.spans.get(t.rows.saturating_sub(1)).and_then(|r| r.get(t.cell)).copied().unwrap_or(1);
                        t.covered = span == 0;
                        span
                    });
                    // Cellformat ur tabellformatet: ett eget cellformat per formaterad cell.
                    let style = match w.table.as_ref().filter(|_| span > 0) {
                        Some(t) if t.fmt.is_some() => {
                            let (r, c) = (t.rows.saturating_sub(1), t.cell);
                            let name = format!("Tabell{}.R{r}C{c}", t.n);
                            let xml = t.fmt.as_ref().map(|f| odt_cell_style(f, &name, r, c, t.spans.len(), c + span >= t.spans.get(r).map_or(0, Vec::len))).unwrap_or_default();
                            w.auto.push_str(&xml);
                            name
                        }
                        _ => "TabellCell".to_owned(),
                    };
                    match span {
                        // Täckt av cellen före: ingen text, bara platshållaren.
                        0 => w.body.push_str("<table:covered-table-cell/>"),
                        1 => w.body.push_str(&format!("<table:table-cell table:style-name=\"{style}\" office:value-type=\"string\">")),
                        n => w.body.push_str(&format!(
                            "<table:table-cell table:style-name=\"{style}\" office:value-type=\"string\" table:number-columns-spanned=\"{n}\">"
                        )),
                    }
                }
                Tag::Emphasis => w.italic += 1,
                Tag::Strong => w.bold += 1,
                Tag::Strikethrough => w.strike += 1,
                Tag::Link { dest_url, .. } => {
                    let url = dest_url.to_string();
                    if !url.is_empty() && !url.starts_with('#') {
                        w.link = Some(url);
                    }
                }
                Tag::Image { dest_url, .. } => w.image = Some((dest_url.to_string(), String::new())),
                _ => {}
            },
            MdEvent::End(tag) => match tag {
                TagEnd::Paragraph => w.close_para(),
                TagEnd::Heading(_) => {
                    w.close_para();
                    w.heading = None;
                    w.title = None;
                }
                TagEnd::BlockQuote(_) => {
                    w.close_para();
                    w.quote = w.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => w.code = false,
                TagEnd::List(_) => {
                    w.close_para();
                    w.body.push_str("</text:list>");
                }
                TagEnd::Item => {
                    w.close_para();
                    w.body.push_str("</text:list-item>");
                }
                TagEnd::Table => {
                    w.body.push_str("</table:table>");
                    w.table = None;
                }
                TagEnd::TableHead => {
                    w.body.push_str("</table:table-row></table:table-header-rows>");
                    if let Some(t) = w.table.as_mut() {
                        t.in_head = false;
                    }
                }
                TagEnd::TableRow => w.body.push_str("</table:table-row>"),
                TagEnd::TableCell => {
                    let covered = w.table.as_mut().is_some_and(|t| {
                        t.cell += 1;
                        std::mem::take(&mut t.covered)
                    });
                    if !covered {
                        w.open_para(); // en cell ska innehålla minst ett stycke
                        w.close_para();
                        w.body.push_str("</table:table-cell>");
                    }
                }
                TagEnd::Emphasis => w.italic = w.italic.saturating_sub(1),
                TagEnd::Strong => w.bold = w.bold.saturating_sub(1),
                TagEnd::Strikethrough => w.strike = w.strike.saturating_sub(1),
                TagEnd::Link => {
                    w.close_link();
                    w.link = None;
                }
                TagEnd::Image => {
                    if let Some((src, alt)) = w.image.take() {
                        w.image(&src, &alt, None, None, None, None, None);
                    }
                }
                _ => {}
            },
            MdEvent::Text(t) | MdEvent::Code(t) if w.image.is_some() => {
                if let Some((_, alt)) = w.image.as_mut() {
                    alt.push_str(&t);
                }
            }
            MdEvent::Text(t) if w.code => {
                let t = t.strip_suffix('\n').unwrap_or(&t).to_owned();
                for line in t.split('\n') {
                    w.open_para();
                    w.run(line, false);
                    w.close_para();
                }
            }
            MdEvent::Text(t) => w.run(&t, false),
            MdEvent::Code(t) => w.run(&t, true),
            MdEvent::Html(t) | MdEvent::InlineHtml(t) => w.html(&t),
            MdEvent::SoftBreak | MdEvent::HardBreak => w.line_break(),
            MdEvent::Rule => {
                w.close_para();
                w.flush_break();
                w.body.push_str("<text:p text:style-name=\"Horizontal_20_Line\"/>");
            }
            MdEvent::TaskListMarker(done) => w.run(if done { "☒ " } else { "☐ " }, false),
            MdEvent::InlineMath(t) => w.math(&t, false),
            MdEvent::DisplayMath(t) => w.math(&t, true),
        }
    }
    w.close_para();
    w.flush_break();
    w.close_text_section();
    w.resolve_notes();
    w
}

const NS: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\" xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:loext=\"urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\"";
const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

fn now_w3cdtf() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Sidhuvudets eller sidfotens innehåll: text och fälten (`{sida}` …) som ODF-fält.
/// En rad i en kommentar som `text:p`-innehåll (flera mellanslag och flikar bevaras).
fn annotation_line(line: &str) -> String {
    let mut out = String::new();
    let mut prev_space = true;
    for c in line.chars() {
        match c {
            ' ' => {
                out.push_str(if prev_space { "<text:s/>" } else { " " });
                prev_space = true;
            }
            '\t' => {
                out.push_str("<text:tab/>");
                prev_space = true;
            }
            '\r' => {}
            c => {
                out.push_str(&esc(&c.to_string()));
                prev_space = false;
            }
        }
    }
    out
}

fn hf_inline(text: &str, f: &style::Fields, out: &mut hf::HfOut<'_>) -> String {
    out.inline(text, f)
}

/// Sidhuvudets eller sidfotens stycke. En mall med `|` blir vänster, mitten och höger skilda av
/// flikar (stycketypen har tabbstopp mitt och längst till höger).
fn hf_paragraph(template: &str, style: &str, align: Align, f: &style::Fields, out: &mut hf::HfOut<'_>) -> String {
    let rows = crate::hf::rows(template);
    // Flera rader (stycken) skrivs alla med flikar, så att varje rads delar hamnar rätt.
    let tabbed = rows.len() > 1;
    let mut xml = String::new();
    for row in rows {
        let inner = if tabbed || style::split_parts(row).is_some() {
            let p = style::parts_of(row, align);
            format!("{}<text:tab/>{}<text:tab/>{}", hf_inline(&p[0], f, out), hf_inline(&p[1], f, out), hf_inline(&p[2], f, out))
        } else {
            let text = style::parts_of(row, align).into_iter().find(|t| !t.is_empty()).unwrap_or_default();
            hf_inline(&text, f, out)
        };
        xml.push_str(&format!("<text:p text:style-name=\"{style}\">{inner}</text:p>"));
    }
    xml
}

fn hf_text(t: &str) -> String {
    let mut out = String::new();
    let mut prev_space = true;
    for c in t.chars() {
        if c == ' ' {
            if prev_space {
                out.push_str("<text:s/>");
            } else {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push_str(&esc(&c.to_string()));
            prev_space = false;
        }
    }
    out
}

fn styles_xml(d: &DocDefaults, fields: &style::Fields, first: &sections::Section, masters: &[sections::Section], env: &mut hf::HfEnv<'_>) -> String {
    let font = d.font().to_owned();
    let size = pt_str(d.size());
    let mut fonts = BTreeSet::new();
    fonts.insert(font.clone());
    fonts.insert("Liberation Mono".to_owned());
    fonts.extend(style::STYLE_IDS.iter().filter_map(|&id| d.styles.get(id).font_family.clone()));
    let (pw, ph) = first.page;
    let m = first.margin;
    let orient = if pw > ph { "landscape" } else { "portrait" };
    // Sidhuvudets och sidfotens utrymme: sidmallens övre/nedre marginal är avståndet till kanten, och
    // sidhuvudet/sidfoten får så hög minsta höjd att brödtexten börjar vid Kladds marginal.
    let hf_style = |tag: &str, body_margin: f32, present: bool| -> (f32, String) {
        let dist = if tag == "header" { d.header_dist } else { d.footer_dist }.unwrap_or(crate::hf::DEFAULT_DIST);
        let (edge, min_h) = if present { (dist, (body_margin - dist - 8.5).max(0.0)) } else { (body_margin, 0.0) };
        (
            edge,
            format!(
                "<style:{tag}-style><style:header-footer-properties fo:min-height=\"{}\" fo:margin-left=\"0cm\" fo:margin-right=\"0cm\" fo:margin-{side}=\"0.3cm\"/></style:{tag}-style>",
                cm(min_h),
                side = if tag == "header" { "bottom" } else { "top" }
            ),
        )
    };
    // Sidhuvud och sidfot, och första sidans och jämna sidors varianter som `style:header-first`/
    // `style:header-left` m.fl. Flikarna för tre delar följer sidmallens textbredd, så varje sidmall
    // får egna styckeformat (`sfx`). `si` är avsnittet sidmallen hör till; ger (xml, finns huvud, finns fot).
    let hf_parts = |text_pt: f32, sfx: &str, si: usize, auto: &mut String, env: &mut hf::HfEnv<'_>| -> (String, bool, bool) {
        let text_w = cm(text_pt.max(36.0));
        let half_w = cm(text_pt.max(36.0) / 2.0);
        let r = env.plan.secs.get(si.min(env.plan.secs.len().saturating_sub(1))).cloned().unwrap_or_default();
        let even = env.plan.even_odd;
        let some = |t: &Option<String>| t.clone().filter(|t| !t.is_empty());
        let mut out = String::new();
        let (mut has_h, mut has_f) = (false, false);
        for (tag, text, align, auto_name, parent) in [
            ("header", some(&r.header), d.header_align, "MP1", "Header"),
            ("footer", some(&r.footer), d.footer_align, "MP2", "Footer"),
            ("header-first", r.title_pg.then(|| r.header_first.clone().unwrap_or_default()), d.header_align, "MP3", "Header"),
            ("footer-first", r.title_pg.then(|| r.footer_first.clone().unwrap_or_default()), d.footer_align, "MP4", "Footer"),
            ("header-left", even.then(|| r.header_even.clone().unwrap_or_default()), d.header_align, "MP5", "Header"),
            ("footer-left", even.then(|| r.footer_even.clone().unwrap_or_default()), d.footer_align, "MP6", "Footer"),
        ] {
            let Some(t) = text else { continue };
            let header = tag.starts_with("header");
            if !t.is_empty() {
                if header {
                    has_h = true;
                } else {
                    has_f = true;
                }
            }
            let split = style::split_parts(&t).is_some() || crate::hf::rows(&t).len() > 1;
            let tabs = if split {
                format!("<style:tab-stops><style:tab-stop style:position=\"{half_w}\" style:type=\"center\"/><style:tab-stop style:position=\"{text_w}\" style:type=\"right\"/></style:tab-stops>")
            } else {
                String::new()
            };
            let al = if split { "start" } else { align_attr(align) };
            let name = format!("{auto_name}{sfx}");
            let st = if header { &d.header_style } else { &d.footer_style };
            let border = st.line.map_or(String::new(), |(c, w)| {
                let side = if header { "bottom" } else { "top" };
                format!(" fo:border-{side}=\"{}pt solid {}\" fo:padding-{side}=\"0.1cm\"", fmt_num(w, 2), style::hex(c))
            });
            let mut tp = String::new();
            if let Some(sz) = st.size {
                tp.push_str(&format!(" fo:font-size=\"{}\"", pt_str(sz)));
            }
            if let Some(c) = st.color {
                tp.push_str(&format!(" fo:color=\"{}\"", style::hex(c)));
            }
            if st.bold {
                tp.push_str(" fo:font-weight=\"bold\"");
            }
            if st.italic {
                tp.push_str(" fo:font-style=\"italic\"");
            }
            if st.underline {
                tp.push_str(" style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"");
            }
            let tp = if tp.is_empty() { String::new() } else { format!("<style:text-properties{tp}/>") };
            auto.push_str(&format!(
                "<style:style style:name=\"{name}\" style:family=\"paragraph\" style:parent-style-name=\"{parent}\"><style:paragraph-properties fo:text-align=\"{al}\" style:justify-single-word=\"false\"{border}>{tabs}</style:paragraph-properties>{tp}</style:style>"
            ));
            out.push_str(&format!("<style:{tag}>{}</style:{tag}>", hf_paragraph(&t, &name, align, fields, &mut env.out)));
        }
        (out, has_h, has_f)
    };
    // Sidmallens sidformat: `style:num-format` är sidnumrets format (`page-format`).
    let num_fmt = d.page_format.map_or("1", |f| f.odf());
    let first_no = d.page_start.map_or(String::new(), |n| format!(" style:first-page-number=\"{n}\""));
    let mut hf_auto = String::new();
    let (master_inner, has_h, has_f) = hf_parts(pw - m[1] - m[3], "", 0, &mut hf_auto, env);
    let (top0, hs) = hf_style("header", m[0], has_h);
    let (bot0, fs) = hf_style("footer", m[2], has_f);
    let mut auto = format!(
        "<style:page-layout style:name=\"pm1\"><style:page-layout-properties fo:page-width=\"{}\" fo:page-height=\"{}\" style:num-format=\"{num_fmt}\"{first_no} style:print-orientation=\"{orient}\" fo:margin-top=\"{}\" fo:margin-bottom=\"{}\" fo:margin-left=\"{}\" fo:margin-right=\"{}\" style:writing-mode=\"lr-tb\"/>",
        cm(pw),
        cm(ph),
        cm(top0),
        cm(bot0),
        cm(m[3]),
        cm(m[1])
    );
    auto.push_str(&hs);
    auto.push_str(&fs);
    auto.push_str("</style:page-layout>");
    auto.push_str(&hf_auto);
    let mut master = format!("<style:master-page style:name=\"Standard\" style:page-layout-name=\"pm1\">{}</style:master-page>", master_inner);
    // Avsnitten med egen sida: en sidmall (M1, M2 …) med eget sidformat och avsnittets sidhuvud och sidfot.
    for (i, sec) in masters.iter().enumerate() {
        let n = i + 1;
        let (w, h) = sec.page;
        let mm = sec.margin;
        let si = env.master_idx.get(i).copied().unwrap_or(0);
        let mut ma = String::new();
        let (hf_xml, mh, mf) = {
            let r = hf_parts(w - mm[1] - mm[3], &format!("_{n}"), si, &mut ma, env);
            (r.0, r.1, r.2)
        };
        let (t, hs) = hf_style("header", mm[0], mh);
        let (b, fs) = hf_style("footer", mm[2], mf);
        auto.push_str(&format!(
            "<style:page-layout style:name=\"pm{}\"><style:page-layout-properties fo:page-width=\"{}\" fo:page-height=\"{}\" style:num-format=\"{num_fmt}\" style:print-orientation=\"{}\" fo:margin-top=\"{}\" fo:margin-bottom=\"{}\" fo:margin-left=\"{}\" fo:margin-right=\"{}\" style:writing-mode=\"lr-tb\"/>{hs}{fs}</style:page-layout>{ma}",
            n + 1,
            cm(w),
            cm(h),
            if w > h { "landscape" } else { "portrait" },
            cm(t),
            cm(b),
            cm(mm[3]),
            cm(mm[1])
        ));
        master.push_str(&format!("<style:master-page style:name=\"M{n}\" style:page-layout-name=\"pm{}\">{hf_xml}</style:master-page>", n + 1));
    }

    let mut named = String::new();
    named.push_str(&format!(
        "<style:default-style style:family=\"paragraph\"><style:paragraph-properties fo:orphans=\"2\" fo:widows=\"2\" style:tab-stop-distance=\"1.25cm\"/><style:text-properties style:font-name=\"{f}\" fo:font-size=\"{size}\" fo:language=\"sv\" fo:country=\"SE\"/></style:default-style>",
        f = esc(&font)
    ));
    named.push_str("<style:style style:name=\"Standard\" style:family=\"paragraph\" style:class=\"text\"/>");
    named.push_str("<style:style style:name=\"Text_20_body\" style:display-name=\"Text body\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"text\"><style:paragraph-properties fo:margin-top=\"0cm\" fo:margin-bottom=\"0.247cm\" fo:line-height=\"115%\"/></style:style>");
    named.push_str("<style:style style:name=\"Heading\" style:display-name=\"Heading\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:next-style-name=\"Text_20_body\" style:class=\"text\"><style:paragraph-properties fo:margin-top=\"0.423cm\" fo:margin-bottom=\"0.212cm\" fo:keep-with-next=\"always\"/><style:text-properties fo:font-weight=\"bold\" style:font-weight-asian=\"bold\" style:font-weight-complex=\"bold\"/></style:style>");
    for (n, (size, color)) in [(18, "1F3864"), (15, "2F5496"), (13, "2F5496"), (12, "2F5496"), (11, "2F5496"), (11, "2F5496")].into_iter().enumerate() {
        let n = n + 1;
        named.push_str(&format!(
            "<style:style style:name=\"Heading_20_{n}\" style:display-name=\"Heading {n}\" style:family=\"paragraph\" style:parent-style-name=\"Heading\" style:next-style-name=\"Text_20_body\" style:default-outline-level=\"{n}\" style:class=\"text\"><style:text-properties fo:font-size=\"{size}pt\" fo:color=\"#{color}\"/></style:style>"
        ));
    }
    // Innehållsförteckningen: rubrik och tre nivåer med högerställd flik med prickar.
    let text_w_pt = (pw - m[1] - m[3]).max(36.0);
    named.push_str("<style:style style:name=\"Contents_20_Heading\" style:display-name=\"Contents Heading\" style:family=\"paragraph\" style:parent-style-name=\"Heading\" style:class=\"index\"><style:paragraph-properties fo:margin-top=\"0.423cm\" fo:margin-bottom=\"0.212cm\"/><style:text-properties fo:font-size=\"18pt\" fo:color=\"#1F3864\"/></style:style>");
    for (n, indent_pt) in [(1, 0.0f32), (2, 14.17), (3, 28.35)] {
        let (indent, tab) = (cm(indent_pt), cm((text_w_pt - indent_pt).max(36.0)));
        named.push_str(&format!(
            "<style:style style:name=\"Contents_20_{n}\" style:display-name=\"Contents {n}\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"index\"><style:paragraph-properties fo:margin-left=\"{indent}\" fo:margin-top=\"0cm\" fo:margin-bottom=\"0.1cm\"><style:tab-stops><style:tab-stop style:position=\"{tw}\" style:type=\"right\" style:leader-style=\"dotted\" style:leader-text=\".\"/></style:tab-stops></style:paragraph-properties></style:style>",
            tw = tab
        ));
    }
    named.push_str("<style:style style:name=\"Title\" style:display-name=\"Title\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:next-style-name=\"Text_20_body\" style:class=\"chapter\"><style:paragraph-properties fo:margin-top=\"0cm\" fo:margin-bottom=\"0.212cm\"/><style:text-properties fo:font-size=\"28pt\" fo:color=\"#1F3864\"/></style:style>");
    named.push_str("<style:style style:name=\"Subtitle\" style:display-name=\"Subtitle\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:next-style-name=\"Text_20_body\" style:class=\"chapter\"><style:paragraph-properties fo:margin-top=\"0cm\" fo:margin-bottom=\"0.212cm\"/><style:text-properties fo:font-size=\"14pt\" fo:color=\"#595959\"/></style:style>");
    named.push_str("<style:style style:name=\"Quotations\" style:display-name=\"Quotations\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"html\"><style:paragraph-properties fo:margin-left=\"1cm\" fo:margin-right=\"0cm\" fo:margin-top=\"0cm\" fo:margin-bottom=\"0.247cm\" fo:padding-left=\"0.3cm\" fo:border-left=\"1.5pt solid #A6A6A6\" fo:border-top=\"none\" fo:border-bottom=\"none\" fo:border-right=\"none\"/><style:text-properties fo:color=\"#404040\"/></style:style>");
    named.push_str("<style:style style:name=\"Preformatted_20_Text\" style:display-name=\"Preformatted Text\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"html\"><style:paragraph-properties fo:margin-top=\"0cm\" fo:margin-bottom=\"0cm\"/><style:text-properties style:font-name=\"Liberation Mono\" fo:font-size=\"10pt\"/></style:style>");
    named.push_str("<style:style style:name=\"Source_20_Text\" style:display-name=\"Source Text\" style:family=\"text\"><style:text-properties style:font-name=\"Liberation Mono\"/></style:style>");
    named.push_str("<style:style style:name=\"Horizontal_20_Line\" style:display-name=\"Horizontal Line\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"html\"><style:paragraph-properties fo:margin-top=\"0cm\" fo:margin-bottom=\"0.2cm\" fo:border-bottom=\"0.5pt solid #000000\" fo:border-top=\"none\" fo:border-left=\"none\" fo:border-right=\"none\" fo:padding=\"0cm\"/><style:text-properties fo:font-size=\"6pt\"/></style:style>");
    named.push_str("<style:style style:name=\"Table_20_Contents\" style:display-name=\"Table Contents\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"extra\"/>");
    named.push_str("<style:style style:name=\"Table_20_Heading\" style:display-name=\"Table Heading\" style:family=\"paragraph\" style:parent-style-name=\"Table_20_Contents\" style:class=\"extra\"><style:text-properties fo:font-weight=\"bold\" style:font-weight-asian=\"bold\" style:font-weight-complex=\"bold\"/></style:style>");
    named.push_str("<style:style style:name=\"Footnote\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"extra\"><style:paragraph-properties fo:margin-left=\"0cm\" fo:margin-right=\"0cm\" fo:text-indent=\"0cm\"/><style:text-properties fo:font-size=\"10pt\"/></style:style>");
    named.push_str("<style:style style:name=\"Footnote_20_Symbol\" style:display-name=\"Footnote Symbol\" style:family=\"text\"/>");
    named.push_str("<style:style style:name=\"Footnote_20_anchor\" style:display-name=\"Footnote anchor\" style:family=\"text\"><style:text-properties style:text-position=\"super 58%\"/></style:style>");
    named.push_str("<style:style style:name=\"Header\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"extra\"/>");
    named.push_str("<style:style style:name=\"Footer\" style:family=\"paragraph\" style:parent-style-name=\"Standard\" style:class=\"extra\"/>");
    named.push_str("<style:style style:name=\"Graphics\" style:family=\"graphic\"><style:graphic-properties text:anchor-type=\"paragraph\" svg:x=\"0cm\" svg:y=\"0cm\" style:wrap=\"none\" style:vertical-pos=\"top\" style:vertical-rel=\"paragraph\"/></style:style>");
    named.push_str("<style:style style:name=\"Internet_20_link\" style:display-name=\"Internet link\" style:family=\"text\"><style:text-properties fo:color=\"#0563C1\" style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"/></style:style>");
    named.push_str("<style:style style:name=\"Visited_20_Internet_20_Link\" style:display-name=\"Visited Internet Link\" style:family=\"text\"><style:text-properties fo:color=\"#0563C1\" style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"/></style:style>");

    apply_style_overrides(&mut named, &d.styles);
    named.push_str(&format!("<text:notes-configuration text:note-class=\"footnote\" style:num-format=\"{}\" text:start-value=\"{}\" text:footnotes-position=\"page\" text:start-numbering-at=\"document\" text:citation-style-name=\"Footnote_20_Symbol\" text:citation-body-style-name=\"Footnote_20_anchor\"/>", d.notes.foot.fmt.odf(), d.notes.foot.start - 1));
    named.push_str(&format!("<text:notes-configuration text:note-class=\"endnote\" style:num-format=\"{}\" text:start-value=\"{}\"/>", d.notes.end.fmt.odf(), d.notes.end.start - 1));

    format!(
        "{XML_DECL}<office:document-styles {NS} office:version=\"1.2\">{}<office:styles>{named}</office:styles><office:automatic-styles>{auto}</office:automatic-styles><office:master-styles>{master}</office:master-styles></office:document-styles>",
        font_decls(&fonts)
    )
}

/// Formatmallarna Kladd själv skriver (utan ändringar), som `StyleSet` att jämföra med vid läsning.
fn kladd_style_set() -> style::StyleSet {
    let d = DocDefaults::default();
    let first = sections::Section::resolve(&sections::SectionProps::default(), &d);
    let none: &dyn Fn(&str) -> Option<Image> = &|_| None;
    let mut env = hf::HfEnv::new(none, &d, &[], &[]);
    let xml = styles_xml(&d, &style::Fields::for_export("", "odt"), &first, &[], &mut env);
    let mut st = Styles::default();
    if let Ok(root) = parse_xml(&xml) {
        st.read_fonts(&root);
        st.read_styles(&root);
    }
    st.style_set()
}

/// Sätter `key="value"` i elementet `tag` i formatmallens XML (`el`), och skapar elementet om det saknas.
fn set_style_attrs(el: &mut String, tag: &str, attrs: &[(&str, String)]) {
    if attrs.is_empty() {
        return;
    }
    let open = format!("<{tag}");
    let at = match el.find(&open) {
        Some(at) => at,
        None => {
            // Egenskapselementet saknas: stycke före text, båda före slutet.
            let end = el.rfind("</style:style>").unwrap_or(el.len());
            let at = if tag == "style:paragraph-properties" { el.find("<style:text-properties").unwrap_or(end) } else { end };
            el.insert_str(at, &format!("<{tag}/>"));
            at
        }
    };
    let tag_end = el[at..].find('>').map_or(el.len(), |i| at + i);
    let self_closing = el[at..tag_end].ends_with('/');
    let mut head = el[at..tag_end].trim_end_matches('/').to_owned();
    for (k, v) in attrs {
        // Befintligt värde byts ut.
        if let Some(p) = head.find(&format!(" {k}=\"")) {
            let q = head[p + k.len() + 3..].find('"').map_or(head.len(), |i| p + k.len() + 3 + i + 1);
            head.replace_range(p..q, "");
        }
        head.push_str(&format!(" {k}=\"{}\"", esc(v)));
    }
    el.replace_range(at..tag_end, &format!("{head}{}", if self_closing { "/" } else { "" }));
}

/// Lägger dokumentets formatmallsändringar på Brödtext, Rubrik 1-6 och Citat i `named`.
fn apply_style_overrides(named: &mut String, set: &style::StyleSet) {
    for id in style::STYLE_IDS {
        let o = set.get(id);
        if o.is_empty() {
            continue;
        }
        let name = match id {
            style::StyleId::Body => "Text_20_body".to_owned(),
            style::StyleId::Heading(n) => format!("Heading_20_{n}"),
            style::StyleId::Quote => "Quotations".to_owned(),
            style::StyleId::Title => "Title".to_owned(),
            style::StyleId::Subtitle => "Subtitle".to_owned(),
        };
        let start_tag = format!("<style:style style:name=\"{name}\"");
        let Some(a) = named.find(&start_tag) else { continue };
        let Some(len) = named[a..].find("</style:style>") else { continue };
        let b = a + len + "</style:style>".len();
        let mut el = named[a..b].to_owned();
        let mut pp: Vec<(&str, String)> = Vec::new();
        if let Some(v) = o.align {
            pp.push(("fo:text-align", align_attr(v).to_owned()));
        }
        if let Some(v) = o.space_before {
            pp.push(("fo:margin-top", cm(v)));
        }
        if let Some(v) = o.space_after {
            pp.push(("fo:margin-bottom", cm(v)));
        }
        if let Some(v) = o.line_spacing {
            pp.push(("fo:line-height", format!("{}%", fmt_num(v * 100.0, 0))));
        }
        if let Some(v) = o.first_indent {
            pp.push(("fo:text-indent", cm(v)));
        }
        let mut tp: Vec<(&str, String)> = Vec::new();
        if let Some(f) = &o.font_family {
            tp.push(("style:font-name", f.clone()));
        }
        if let Some(v) = o.size {
            tp.push(("fo:font-size", pt_str(v)));
        }
        if let Some(v) = o.bold {
            let w = if v { "bold" } else { "normal" };
            tp.push(("fo:font-weight", w.into()));
            tp.push(("style:font-weight-asian", w.into()));
            tp.push(("style:font-weight-complex", w.into()));
        }
        if let Some(v) = o.italic {
            tp.push(("fo:font-style", if v { "italic" } else { "normal" }.into()));
        }
        if let Some(c) = o.color {
            tp.push(("fo:color", style::hex(c)));
        }
        set_style_attrs(&mut el, "style:paragraph-properties", &pp);
        set_style_attrs(&mut el, "style:text-properties", &tp);
        named.replace_range(a..b, &el);
    }
}

fn meta_xml(title: &str) -> String {
    let now = now_w3cdtf();
    format!(
        "{XML_DECL}<office:document-meta {NS} office:version=\"1.2\"><office:meta><meta:generator>Kladd</meta:generator><dc:title>{}</dc:title><meta:creation-date>{now}</meta:creation-date><dc:date>{now}</dc:date></office:meta></office:document-meta>",
        esc(title)
    )
}

fn manifest_xml(media: &[(String, Vec<u8>, &'static str)]) -> String {
    let mut s = format!(
        "{XML_DECL}<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.2\"><manifest:file-entry manifest:full-path=\"/\" manifest:version=\"1.2\" manifest:media-type=\"{MIME}\"/>"
    );
    for (path, mt) in [("content.xml", "text/xml"), ("styles.xml", "text/xml"), ("meta.xml", "text/xml")] {
        s.push_str(&format!("<manifest:file-entry manifest:full-path=\"{path}\" manifest:media-type=\"{mt}\"/>"));
    }
    for (path, _, mime) in media {
        s.push_str(&format!("<manifest:file-entry manifest:full-path=\"{path}\" manifest:media-type=\"{mime}\"/>"));
    }
    s.push_str("</manifest:manifest>");
    s
}

/// Bygger ett komplett .odt-paket av Markdown-text.
/// `resolve` hämtar bilddata för en bildkälla (`bild:namn`, en sökväg …).
pub fn write(markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<Image>) -> Result<Written, String> {
    // Spårade ändringar och kommentarer: posterna lyfts ur texten och skrivs som `text:tracked-changes`
    // och `office:annotation`.
    let (stripped, revs, comments) = review::split_records(markdown);
    let markdown = stripped.as_str();
    let first = markdown.lines().next().unwrap_or("");
    let (defaults, markdown) = match DocDefaults::from_comment(first) {
        Some(d) => (d, markdown[first.len()..].trim_start_matches(['\n', '\r'])),
        None => (DocDefaults::default(), markdown),
    };
    let (pw, _) = defaults.page();
    let m = defaults.margins();
    let text_w = (pw - m[1] - m[3]).max(36.0);
    let mut w = build_body(markdown, resolve, text_w, &defaults, revs, comments);
    w.fonts.insert(defaults.font().to_owned());
    w.fonts.extend(style::STYLE_IDS.iter().filter_map(|&id| defaults.styles.get(id).font_family.clone()));
    let content = format!(
        "{XML_DECL}<office:document-content {NS} office:version=\"1.2\">{}<office:automatic-styles>{}</office:automatic-styles><office:body><office:text>{}{}</office:text></office:body></office:document-content>",
        font_decls(&w.fonts),
        w.auto,
        w.tracked_changes(),
        w.body
    );

    // Sidhuvuden och sidfötter först: deras bilder hör till paketet (och manifestet).
    let all_secs = sections::scan(markdown, &defaults);
    let mut env = hf::HfEnv::new(resolve, &defaults, &all_secs, &w.masters);
    let styles_xml_text = styles_xml(&defaults, &style::Fields::for_export(title, "odt"), &w.first, &w.masters, &mut env);
    w.media.append(&mut env.out.media);
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);
        let deflated = stored.compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6));
        // `mimetype` först och okomprimerad, så att filtypen kan kännas igen på de första byten.
        zip.start_file("mimetype", stored).map_err(|e| e.to_string())?;
        zip.write_all(MIME.as_bytes()).map_err(|e| e.to_string())?;
        for (name, text) in [
            ("META-INF/manifest.xml", manifest_xml(&w.media)),
            ("content.xml", content),
            ("styles.xml", styles_xml_text.clone()),
            ("meta.xml", meta_xml(title)),
        ] {
            zip.start_file(name, deflated).map_err(|e| e.to_string())?;
            zip.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        for (path, bytes, _) in w.media.iter().filter(|m| !m.0.ends_with('/')) {
            zip.start_file(path.as_str(), stored).map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(Written { bytes: buf.into_inner(), missing_images: w.missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// En liten giltig PNG (1x1 pixel).
    fn png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0, 1, 0, 0,
            5, 0, 1, 0x0D, 0x0A, 0x2D, 0xB4, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    const RICH: &str = "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt; page-size: Letter; page-margin: 25mm 20mm 25mm 20mm; header: \"Rapport\"; header-align: right; footer: \"Sida {sida} av {sidor}\"; footer-align: center -->

# Rubrik ett

## <span style=\"color: #C00000\">Röd rubrik</span>

Text med **fet**, *kursiv*, ~~struken~~, <u>understruken</u>, <u>**fet understruken**</u>, `kod` och [en länk](https://example.com/x).

<span style=\"font-family: Arial; font-size: 14pt; color: #C00000; background-color: #FFFF00\">Alla egenskaper</span> och <span style=\"color: #0000FF\"><u>**blå**</u></span>.

Rad ett
Rad två med & och \\<tecken>

<div align=\"center\">

Mitten

</div>

<div align=\"right\">

Höger

</div>

<div align=\"justify\">

Marginal

</div>

- ett
- två
    - tre
    - fyra
        1. djup
        2. djupare

3. tredje
4. fjärde
    - punkt

> Ett citat.

```
kod rad 1
  rad 2
```

| A | B |
| --- | --- |
| 1 | **2** |

![En bild](bild:image1.png)

<img src=\"bild:image1.png\" alt=\"Liten\" width=\"240\">

Slut.
";

    fn resolver() -> impl Fn(&str) -> Option<Image> {
        let p = png();
        move |src| (src == "bild:image1.png").then(|| Image { bytes: p.clone(), extent: None, svg: None })
    }

    #[test]
    fn rich_roundtrip_is_identical() {
        let w = write(RICH, "Test", &resolver()).unwrap();
        assert!(w.missing_images.is_empty());
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        assert_eq!(back.markdown, RICH, "\n--- fick ---\n{}", back.markdown);
        assert_eq!(back.images.len(), 1, "samma bild bäddas in en gång");
        // Och en gång till med bilderna från dokumentet.
        let imgs = back.images.clone();
        let w2 = write(&back.markdown, "Test", &move |src| {
            let n = src.strip_prefix(IMAGE_SCHEME)?;
            imgs.iter().find(|(m, _)| m == n).map(|(_, i)| i.clone())
        })
        .unwrap();
        assert_eq!(read(&w2.bytes).unwrap().markdown, RICH);
    }

    #[test]
    fn package_layout() {
        let w = write("Hej\n", "T", &|_| None).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(&w.bytes)).unwrap();
        {
            let first = z.by_index(0).unwrap();
            assert_eq!(first.name(), "mimetype");
            assert_eq!(first.compression(), zip::CompressionMethod::Stored);
            assert_eq!(first.size() as usize, MIME.len());
        }
        // Rå kontroll: namnet och innehållet ligger okomprimerade först i filen.
        assert_eq!(&w.bytes[30..38], b"mimetype");
        assert_eq!(&w.bytes[38..38 + MIME.len()], MIME.as_bytes());
        let mut names: Vec<String> = z.file_names().map(str::to_owned).collect();
        names.sort();
        assert_eq!(names, ["META-INF/manifest.xml", "content.xml", "meta.xml", "mimetype", "styles.xml"]);
        let mut s = String::new();
        z.by_name("META-INF/manifest.xml").unwrap().read_to_string(&mut s).unwrap();
        for n in ["content.xml", "styles.xml", "meta.xml"] {
            assert!(s.contains(&format!("full-path=\"{n}\"")), "{s}");
        }
        let mut meta = String::new();
        z.by_name("meta.xml").unwrap().read_to_string(&mut meta).unwrap();
        assert!(meta.contains("<meta:generator>Kladd</meta:generator>") && meta.contains("<dc:title>T</dc:title>"));
        for n in ["content.xml", "styles.xml", "meta.xml", "META-INF/manifest.xml"] {
            let mut x = String::new();
            z.by_name(n).unwrap().read_to_string(&mut x).unwrap();
            parse_xml(&x).unwrap_or_else(|e| panic!("{n}: {e}"));
        }
    }

    #[test]
    fn images_listed_in_manifest() {
        let w = write("![x](bild:image1.png)\n", "T", &resolver()).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(&w.bytes)).unwrap();
        assert!(z.by_name("Pictures/image1.png").is_ok());
        let mut s = String::new();
        z.by_name("META-INF/manifest.xml").unwrap().read_to_string(&mut s).unwrap();
        assert!(s.contains("Pictures/image1.png") && s.contains("image/png"));
        let missing = write("![x](saknas.png)\n", "T", &|_| None).unwrap();
        assert_eq!(missing.missing_images, vec!["saknas.png".to_owned()]);
        assert_eq!(read(&missing.bytes).unwrap().markdown, "\\[bild: x\\]\n");
    }

    fn content_xml(bytes: &[u8]) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        z.by_name("content.xml").unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn style_overrides_roundtrip_through_styles_xml() {
        let md = "<!-- kladd: h1: \"Georgia\" 22pt nobold #112233 center space-before 18pt space-after 6pt line-spacing 1.5 indent 12pt; body: justify line-spacing 1.2; quote: italic 11pt -->\n\n# Rubrik\n\nText\n\n> citat\n";
        let w = write(md, "T", &|_| None).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(&w.bytes)).unwrap();
        let mut styles = String::new();
        z.by_name("styles.xml").unwrap().read_to_string(&mut styles).unwrap();
        let h1 = &styles[styles.find("style:name=\"Heading_20_1\"").unwrap()..];
        let h1 = &h1[..h1.find("</style:style>").unwrap()];
        assert!(h1.contains("fo:text-align=\"center\"") && h1.contains("fo:margin-top=\"0.635cm\""), "{h1}");
        assert!(h1.contains("fo:line-height=\"150%\"") && h1.contains("fo:font-size=\"22pt\"") && h1.contains("fo:color=\"#112233\""), "{h1}");
        // Attributet finns bara en gång även om standardvärdet fanns.
        assert_eq!(h1.matches("fo:font-size=").count(), 1);
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        let first = back.markdown.lines().next().unwrap();
        assert!(first.contains("h1: \"Georgia\" 22pt nobold #112233 center space-before 18pt space-after 6pt line-spacing 1.5 indent 12pt"), "{first}");
        assert!(first.contains("body: justify line-spacing 1.2"), "{first}");
        // Dokument utan ändringar får ingen formatmallsrad.
        let plain = read(&write("# Rubrik\n\nText\n", "T", &|_| None).unwrap().bytes).unwrap();
        assert_eq!(plain.markdown, "# Rubrik\n\nText\n");
    }

    #[test]
    fn floating_image_roundtrip() {
        let md = "<img src=\"bild:image1.png\" alt=\"Sol\" width=\"200\" align=\"left\">Text bredvid.\n\n<img src=\"bild:image1.png\" width=\"100\" align=\"right\">\n\nSlut\n";
        let w = write(md, "T", &resolver()).unwrap();
        assert!(w.missing_images.is_empty());
        let c = content_xml(&w.bytes);
        assert!(c.contains("style:horizontal-pos=\"left\"") && c.contains("style:wrap=\"right\""), "{c}");
        assert!(c.contains("style:horizontal-pos=\"right\"") && c.contains("style:wrap=\"left\""));
        assert_eq!(c.matches("text:anchor-type=\"paragraph\"").count(), 2);
        assert!(!c.contains("text:anchor-type=\"as-char\""));
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        assert_eq!(back.markdown, md, "\n--- fick ---\n{}", back.markdown);
        // En bild som ligger i texten flyter inte.
        let inline = write("<img src=\"bild:image1.png\" width=\"200\">\n", "T", &resolver()).unwrap();
        assert!(!read(&inline.bytes).unwrap().markdown.contains("align="));
    }

    #[test]
    fn anchored_image_roundtrip() {
        // Röd 1x1-bild skalas till 200 px; beskärningen räknas mot bildens 96-dpi-mått (1 px), så
        // en beskärning testas på en större bild nedan.
        let md = concat!(
            "<img src=\"bild:image1.png\" alt=\"A\" width=\"200\" hrel=\"page\" x=\"96\" vrel=\"page\" y=\"bottom\" wrap=\"behind\" dist=\"0 6 0 6\">Text\n\n",
            "<img src=\"bild:image1.png\" width=\"100\" x=\"center\" y=\"12\" wrap=\"top\">\n\nMellan\n\n",
            "<img src=\"bild:image1.png\" width=\"50\" hrel=\"margin\" x=\"right\" vrel=\"margin\" y=\"20\" dist=\"3 4 5 6\">\n\nSlut\n"
        );
        let w = write(md, "T", &resolver()).unwrap();
        let c = content_xml(&w.bytes);
        assert!(c.contains("style:wrap=\"run-through\"") && c.contains("style:run-through=\"background\""), "{c}");
        assert!(c.contains("style:horizontal-rel=\"page\"") && c.contains("style:vertical-pos=\"bottom\""), "{c}");
        assert!(c.contains("svg:x=\"2.540cm\""), "{c}");
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        assert_eq!(back.markdown, md, "\n--- fick ---\n{}\n{c}", back.markdown);
    }

    #[test]
    fn text_box_roundtrip() {
        let pp = "<!--k:pp bdr=\"t=single:6:6:112233 l=single:6:6:112233 b=single:6:6:112233 r=single:6:6:112233\" shd=\"FFF2CC\" fr=\"w=200 x=end pl=6.75 pr=6.75\"-->";
        let md = format!("Före\n\n<!--k:tb 0-->\n\n**Faktaruta**{pp}\n\nText i rutan{pp}\n\n<!--k:/tb-->\n\nEfter\n");
        let w = write(&md, "T", &resolver()).unwrap();
        let c = content_xml(&w.bytes);
        assert!(c.contains("<draw:text-box>") && c.contains("svg:width=\"200.000pt\""), "{c}");
        assert!(c.contains("draw:fill-color=\"#FFF2CC\"") && c.contains("style:horizontal-pos=\"right\""), "{c}");
        assert!(c.contains("Text i rutan") && !c.contains("k:tb") && !c.contains("k:pp"), "{c}");
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        let m = back.markdown;
        assert!(m.contains("<!--k:tb 0 ") && m.contains("<!--k:/tb-->"), "{m}");
        let block = m.split("<!--k:tb 0 ").nth(1).unwrap().split("<!--k:/tb-->").next().unwrap();
        assert!(block.contains("**Faktaruta**") && block.contains("Text i rutan") && block.contains("shd=\"FFF2CC\""), "{m}");
        assert!(block.contains("x=end") && block.contains("w=200"), "{m}");
        assert!(m.find("Före").unwrap() < m.find("k:tb").unwrap() && m.find("k:/tb").unwrap() < m.find("Efter").unwrap(), "{m}");
    }

    #[test]
    fn dragged_text_box_roundtrip() {
        use crate::word::{anchor::Pos, tboxui};
        let pp = "<!--k:pp bdr=\"t=single:6:6:112233 l=single:6:6:112233 b=single:6:6:112233 r=single:6:6:112233\" shd=\"FFF2CC\" fr=\"w=112.5 x=end pl=6.75 pr=6.75\"-->";
        let md = format!("Före\n\n<!--k:tb 0 w=\"150\" h=\"90\" x=\"right\" y=\"0\"-->\n\n**Faktaruta**{pp}\n\n<!--k:/tb-->\n\nEfter\n");
        let o = md.find("<!--k:tb").unwrap();
        let open = o..md[o..].find("-->").unwrap() + o + 3;
        let mut head = crate::docx_tbox::parse_head(&md[open.clone()]).unwrap().1;
        let mut a = head.anchor.unwrap();
        (a.h, a.v) = (Pos::Off(73.5), Pos::Off(21.0));
        head.anchor = Some(a);
        let md = tboxui::apply(&md, &tboxui::Change { open, head, x_pt: Some(73.5 * 0.75) }).unwrap();
        let w = write(&md, "T", &resolver()).unwrap();
        let back = read(&w.bytes).unwrap().markdown;
        let o = back.find("<!--k:tb").unwrap();
        let h = crate::docx_tbox::parse_head(&back[o..o + back[o..].find("-->").unwrap() + 3]).unwrap().1;
        let a = h.anchor.unwrap();
        assert_eq!((a.h, a.v), (Pos::Off(73.5), Pos::Off(21.0)), "{back}");
        assert!((h.w - 150.0).abs() < 0.6 && (h.h - 90.0).abs() < 0.6, "{back}");
    }

    #[test]
    fn text_box_in_the_margin_roundtrip() {
        use crate::word::{anchor::Pos, tboxui};
        let pp = |w: f32| format!("<!--k:pp bdr=\"t=single:6:6:112233 l=single:6:6:112233 b=single:6:6:112233 r=single:6:6:112233\" shd=\"FFF2CC\" fr=\"w={w} x=end pl=6.75 pr=6.75\"-->");
        // A4 med 2,54 cm marginal (textytan 602 px): 80 px bred ruta delvis i högermarginalen, hela i den, och i vänstermarginalen.
        for x in [560.0f32, 610.0, -70.0] {
            let md = format!("Före\n\n<!--k:tb 0 w=\"80\" h=\"90\" x=\"right\" y=\"0\"-->\n\n**Faktaruta**{}\n\n<!--k:/tb-->\n\nEfter\n", pp(60.0));
            let o = md.find("<!--k:tb").unwrap();
            let open = o..md[o..].find("-->").unwrap() + o + 3;
            let mut head = crate::docx_tbox::parse_head(&md[open.clone()]).unwrap().1;
            let mut a = head.anchor.unwrap();
            (a.h, a.v) = (Pos::Off(x), Pos::Off(21.0));
            head.anchor = Some(a);
            let md = tboxui::apply(&md, &tboxui::Change { open, head, x_pt: Some(x * 0.75) }).unwrap();
            let w = write(&md, "T", &resolver()).unwrap();
            let c = content_xml(&w.bytes);
            assert!(c.contains(&format!("svg:x=\"{:.5}pt\"", x * 0.75)), "{c}");
            let back = read(&w.bytes).unwrap();
            assert!(back.dropped.is_empty(), "{:?}", back.dropped);
            let m = back.markdown;
            let o = m.find("<!--k:tb").unwrap();
            let h = crate::docx_tbox::parse_head(&m[o..o + m[o..].find("-->").unwrap() + 3]).unwrap().1;
            assert_eq!(h.anchor.unwrap().h, Pos::Off(x), "{m}");
            // Ramens läge utanför spalten bevaras (inte inklämt i den).
            assert!(m.contains(&format!("x={}", x * 0.75)), "{m}");
        }
    }

    #[test]
    fn frame_crop_and_page_anchor_are_read() {
        let c = LO_CONTENT.replace("svg:width=\"0.0265cm\" svg:height=\"0.0265cm\"", "svg:width=\"6.35cm\" svg:height=\"6.35cm\" svg:x=\"1cm\" svg:y=\"2cm\"");
        let c = c.replace("<draw:frame draw:name=\"Bild1\" text:anchor-type=\"as-char\"", "<draw:frame draw:style-name=\"fr9\" draw:name=\"Bild1\" text:anchor-type=\"page\"");
        let style = "<style:style style:name=\"fr9\" style:family=\"graphic\"><style:graphic-properties style:wrap=\"none\" style:horizontal-pos=\"from-left\" style:horizontal-rel=\"page\" style:vertical-pos=\"from-top\" style:vertical-rel=\"page\"/></style:style>";
        let c = c.replacen("</office:automatic-styles>", &format!("{style}</office:automatic-styles>"), 1);
        let m = read(&make_odt(&c, LO_STYLES, &[("Pictures/a.png", &png())])).unwrap().markdown;
        assert!(m.contains("hrel=\"page\" x=\"37.8\" vrel=\"page\" y=\"75.6\" wrap=\"top\""), "{m}");
    }

    #[test]
    fn wrap_values_are_mapped() {
        let base = |wrap: &str, pos: &str| {
            let c = LO_CONTENT.replace("svg:width=\"0.0265cm\" svg:height=\"0.0265cm\"", "svg:width=\"6.35cm\" svg:height=\"6.35cm\"");
            let c = c.replace("<draw:frame draw:name=\"Bild1\" text:anchor-type=\"as-char\"", "<draw:frame draw:style-name=\"fr9\" draw:name=\"Bild1\" text:anchor-type=\"paragraph\"");
            let style = format!("<style:style style:name=\"fr9\" style:family=\"graphic\"><style:graphic-properties style:wrap=\"{wrap}\" style:horizontal-pos=\"{pos}\"/></style:style>");
            let c = c.replacen("</office:automatic-styles>", &format!("{style}</office:automatic-styles>"), 1);
            let l = read(&make_odt(&c, LO_STYLES, &[("Pictures/a.png", &png())])).unwrap();
            // Bara <img>-taggen (stycket kan ha en egen <div align>).
            let m = l.markdown;
            let i = m.find("<img").expect("bildtagg");
            m[i..i + m[i..].find('>').unwrap()].to_owned()
        };
        // Utan margin är det ett ankare (kortformen `align` kräver Kladds normala 12 px luft).
        assert!(base("right", "center").contains("x=\"center\""));
        assert!(base("parallel", "right").contains("x=\"right\""));
        assert!(base("dynamic", "left").contains("x=\"left\""));
        assert!(base("none", "left").contains("wrap=\"top\""));
        assert!(base("run-through", "left").contains("wrap=\"front\""));
        assert!(base("parallel", "center").contains("x=\"center\"") && !base("parallel", "center").contains("wrap="));
    }

    /// LibreOffice ska rendera den skrivna filen med texten runt bilden. Kräver `soffice` och `pdftotext`.
    #[test]
    #[ignore]
    fn floating_image_libreoffice() {
        let dir = std::env::temp_dir().join("kladd-float-lo-odt");
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image::RgbImage::new(240, 160);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb(if (x / 20 + y / 20) % 2 == 0 { [60, 120, 200] } else { [255, 200, 40] });
        }
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris.";
        let md = format!("<img src=\"b.png\" alt=\"\" width=\"240\" align=\"left\">{lorem}\n\n{lorem}\n\n{lorem}\n\n{lorem}\n\n{lorem}\n\n{lorem}\n");
        let w = write(&md, "x", &|_| Some(Image { bytes: png.clone(), extent: None, svg: None })).unwrap();
        let odt = dir.join("f.odt");
        std::fs::write(&odt, w.bytes).unwrap();
        let st = std::process::Command::new("soffice")
            .arg(format!("-env:UserInstallation=file://{}/profile", dir.display()))
            .args(["--headless", "--convert-to", "pdf", "--outdir"]).arg(&dir).arg(&odt).output().unwrap();
        assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
        let out = std::process::Command::new("pdftotext").args(["-bbox"]).arg(dir.join("f.pdf")).arg("-").output().unwrap();
        let html = String::from_utf8_lossy(&out.stdout).into_owned();
        let xs: Vec<f32> = html
            .lines()
            .filter(|l| l.contains(">Lorem<"))
            .filter_map(|l| l.split("xMin=\"").nth(1)?.split('"').next()?.parse().ok())
            .collect();
        assert!(xs.len() >= 6, "{xs:?}");
        assert!(xs[0] > 200.0, "första stycket vid x = {}", xs[0]);
        assert!(xs.iter().any(|&x| x < 100.0), "inget stycke under bilden: {xs:?}");
    }

    #[test]
    fn toc_line_roundtrips_as_table_of_content() {
        let md = "# Ett\n\n<!-- kladd: toc -->\n\n## Två\n\n#### Fyra\n\nSlut.\n";
        let w = write(md, "T", &|_| None).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(&w.bytes)).unwrap();
        let mut content = String::new();
        z.by_name("content.xml").unwrap().read_to_string(&mut content).unwrap();
        assert!(content.contains("<text:table-of-content "), "{content}");
        assert!(content.contains("text:outline-level=\"3\""));
        assert!(content.contains(">Två<text:tab/>1</text:p>"), "{content}");
        assert!(!content.contains("Fyra</text:p><text:tab"), "H4 ska inte vara med");
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        assert_eq!(back.markdown, md, "\n--- fick ---\n{}", back.markdown);
    }

    #[test]
    fn numbered_list_start_and_spaces() {
        let md = "5. fem\n6. sex\n\nTre    mellanslag och\ttab.\n";
        assert_eq!(read(&write(md, "T", &|_| None).unwrap().bytes).unwrap().markdown, md);
    }

    fn md_of_body(auto: &str, body: &str) -> String {
        let c = format!(r##"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3"><office:automatic-styles>{auto}</office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>"##);
        read(&make_odt(&c, LO_STYLES, &[])).unwrap().markdown
    }

    const BRK_AUTO: &str = r#"<style:style style:name="PB" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:break-before="page"/></style:style><style:style style:name="PA" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:break-after="page"/></style:style>"#;

    #[test]
    fn break_before_and_after_give_page_break_rows() {
        let brk = "<!-- kladd: sidbrytning -->";
        let md = md_of_body(BRK_AUTO, r#"<text:p text:style-name="Standard">A</text:p><text:p text:style-name="PB">B</text:p>"#);
        assert!(md.contains(&format!("A\n\n{brk}\n\nB")), "{md}");
        let md = md_of_body(BRK_AUTO, r#"<text:p text:style-name="PA">A</text:p><text:p text:style-name="Standard">B</text:p>"#);
        assert!(md.contains(&format!("A\n\n{brk}\n\nB")), "{md}");
        // Efter + före på granna stycken ger bara en rad (och först i dokumentet ingen).
        let md = md_of_body(BRK_AUTO, r#"<text:p text:style-name="PB">A</text:p><text:p text:style-name="PA">B</text:p><text:p text:style-name="PB">C</text:p>"#);
        assert_eq!(md.matches(brk).count(), 1, "{md}");
        assert!(!md.contains(&format!("{brk}\n\nA")), "{md}");
    }

    #[test]
    fn page_break_row_roundtrips() {
        let md = "Före\n\n<!-- kladd: sidbrytning -->\n\nEfter\n";
        let back = read(&write(md, "T", &|_| None).unwrap().bytes).unwrap().markdown;
        assert_eq!(back, md);
    }

    fn make_odt(content: &str, styles: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            z.start_file("mimetype", o).unwrap();
            z.write_all(MIME.as_bytes()).unwrap();
            z.start_file("content.xml", o).unwrap();
            z.write_all(content.as_bytes()).unwrap();
            z.start_file("styles.xml", o).unwrap();
            z.write_all(styles.as_bytes()).unwrap();
            for (n, b) in extra {
                z.start_file(*n, o).unwrap();
                z.write_all(b).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    const LO_STYLES: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.3">
<office:font-face-decls>
<style:font-face style:name="Liberation Serif" svg:font-family="'Liberation Serif'" style:font-family-generic="roman"/>
<style:font-face style:name="Liberation Sans" svg:font-family="'Liberation Sans'" style:font-family-generic="swiss"/>
<style:font-face style:name="Liberation Mono" svg:font-family="'Liberation Mono'" style:font-family-generic="modern"/>
</office:font-face-decls>
<office:styles>
<style:default-style style:family="paragraph"><style:paragraph-properties fo:orphans="2" fo:widows="2"/><style:text-properties style:font-name="Liberation Serif" fo:font-size="12pt" fo:language="sv"/></style:default-style>
<style:style style:name="Standard" style:family="paragraph" style:class="text"/>
<style:style style:name="Heading" style:family="paragraph" style:parent-style-name="Standard" style:class="text"><style:text-properties style:font-name="Liberation Sans" fo:font-size="14pt"/></style:style>
<style:style style:name="Text_20_body" style:display-name="Text body" style:family="paragraph" style:parent-style-name="Standard" style:class="text"><style:paragraph-properties fo:margin-bottom="0.247cm"/></style:style>
<style:style style:name="Heading_20_1" style:display-name="Heading 1" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="1" style:class="text"><style:text-properties fo:font-size="130%" fo:font-weight="bold"/></style:style>
<style:style style:name="Quotations" style:family="paragraph" style:parent-style-name="Standard" style:class="html"/>
<style:style style:name="Preformatted_20_Text" style:display-name="Preformatted Text" style:family="paragraph" style:parent-style-name="Standard"><style:text-properties style:font-name="Liberation Mono" fo:font-size="10pt"/></style:style>
<style:style style:name="Source_20_Text" style:display-name="Source Text" style:family="text"><style:text-properties style:font-name="Liberation Mono"/></style:style>
<style:style style:name="Footer" style:family="paragraph" style:parent-style-name="Standard"/>
</office:styles>
<office:automatic-styles>
<style:page-layout style:name="Mpm1"><style:page-layout-properties fo:page-width="21.59cm" fo:page-height="27.94cm" fo:margin-top="2cm" fo:margin-bottom="2cm" fo:margin-left="2.5cm" fo:margin-right="2.5cm"/></style:page-layout>
<style:style style:name="MP1" style:family="paragraph" style:parent-style-name="Footer"><style:paragraph-properties fo:text-align="center"/></style:style>
</office:automatic-styles>
<office:master-styles><style:master-page style:name="Standard" style:page-layout-name="Mpm1"><style:footer><text:p text:style-name="MP1">Sida <text:page-number text:select-page="current">1</text:page-number> av <text:page-count>3</text:page-count></text:p></style:footer></style:master-page></office:master-styles>
</office:document-styles>"##;

    const LO_CONTENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.3">
<office:automatic-styles>
<style:style style:name="P1" style:family="paragraph" style:parent-style-name="Text_20_body"><style:paragraph-properties fo:text-align="center"/></style:style>
<style:style style:name="P2" style:family="paragraph" style:parent-style-name="Text_20_body"><style:paragraph-properties fo:text-align="justify"/></style:style>
<style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
<style:style style:name="T2" style:family="text"><style:text-properties fo:font-style="italic" style:text-underline-style="solid" style:text-line-through-style="solid"/></style:style>
<style:style style:name="T3" style:family="text"><style:text-properties fo:color="#ff0000" fo:background-color="#ffff00" fo:font-size="16pt" style:font-name="Liberation Sans"/></style:style>
<style:style style:name="T4" style:family="text"><style:text-properties fo:background-color="transparent"/></style:style>
<text:list-style style:name="L1"><text:list-level-style-bullet text:level="1" text:bullet-char="•"/><text:list-level-style-bullet text:level="2" text:bullet-char="◦"/></text:list-style>
<text:list-style style:name="L2"><text:list-level-style-number text:level="1" style:num-format="1"/><text:list-level-style-number text:level="2" style:num-format="a"/></text:list-style>
</office:automatic-styles>
<office:body><office:text>
<text:sequence-decls/>
<text:h text:style-name="Heading_20_1" text:outline-level="1">Rubrik <text:span text:style-name="T1">ett</text:span></text:h>
<text:p text:style-name="Text_20_body">Vanlig <text:span text:style-name="T1">fet</text:span> och <text:span text:style-name="T2">kursiv struken</text:span> samt <text:span text:style-name="T3">röd</text:span><text:span text:style-name="T4">!</text:span>
 med   <text:s text:c="2"/>mellanrum<text:tab/>tab<text:line-break/>ny rad och <text:a xlink:type="simple" xlink:href="https://example.com" text:style-name="Internet_20_link">länk</text:a><text:note text:note-class="footnote"><text:note-body><text:p>Fotnot</text:p></text:note-body></text:note>.</text:p>
<text:p text:style-name="P1">Centrerat</text:p>
<text:p text:style-name="P2">Marginal</text:p>
<text:list text:style-name="L1"><text:list-item><text:p text:style-name="Text_20_body">ett</text:p></text:list-item><text:list-item><text:p text:style-name="Text_20_body">två</text:p><text:list text:style-name="L2"><text:list-item><text:p text:style-name="Text_20_body">a</text:p></text:list-item><text:list-item><text:p text:style-name="Text_20_body">b</text:p></text:list-item></text:list></text:list-item></text:list>
<text:p text:style-name="Quotations">Ett citat</text:p>
<text:p text:style-name="Preformatted_20_Text">fn main() {</text:p>
<text:p text:style-name="Preformatted_20_Text"><text:s text:c="4"/>x();</text:p>
<text:p text:style-name="Preformatted_20_Text">}</text:p>
<table:table table:name="Tabell1"><table:table-column table:number-columns-repeated="2"/>
<table:table-row><table:table-cell office:value-type="string"><text:p>Namn</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>Antal</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell office:value-type="string"><text:p>Äpple | rött</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>3</text:p></table:table-cell></table:table-row></table:table>
<text:p text:style-name="Text_20_body"><draw:frame draw:name="Bild1" text:anchor-type="as-char" svg:width="0.0265cm" svg:height="0.0265cm"><draw:image xlink:href="Pictures/a.png" xlink:type="simple"/><svg:desc>Alternativtext</svg:desc></draw:frame></text:p>
<text:p text:style-name="Text_20_body">Sista</text:p>
</office:text></office:body></office:document-content>"##;

    #[test]
    fn tracked_changes_become_review_markup() {
        let c = r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" office:version="1.3"><office:body><office:text><text:tracked-changes text:track-changes="true"><text:changed-region text:id="ct1"><text:insertion><office:change-info><dc:creator>Ada</dc:creator><dc:date>2026-05-01T10:00:00</dc:date></office:change-info></text:insertion></text:changed-region><text:changed-region text:id="ct2"><text:deletion><office:change-info><dc:creator>Bo</dc:creator><dc:date>2026-05-02T10:00:00</dc:date></office:change-info><text:p text:style-name="Standard">Borttagen</text:p></text:deletion></text:changed-region></text:tracked-changes><text:p text:style-name="Standard">Hej <text:change-start text:change-id="ct1"/>nya<text:change-end text:change-id="ct1"/> värld<text:change text:change-id="ct2"/>.</text:p></office:text></office:body></office:document-content>"##;
        let l = read(&make_odt(c, LO_STYLES, &[])).unwrap();
        assert!(l.markdown.contains("Hej <ins data-k=\"1\">nya</ins> värld<del data-k=\"2\">Borttagen</del>."), "{}", l.markdown);
        assert!(crate::review::accept_all(&l.markdown).trim().ends_with("Hej nya värld."));
        assert!(l.markdown.contains("author=\"Ada\"") && l.markdown.contains("author=\"Bo\""), "{}", l.markdown);
    }

    #[test]
    fn reads_libreoffice_style_document() {
        let bytes = make_odt(LO_CONTENT, LO_STYLES, &[("Pictures/a.png", &png())]);
        let l = read(&bytes).unwrap();
        assert!(l.dropped.is_empty(), "{:?}", l.dropped);
        let md = &l.markdown;
        assert!(
            md.starts_with("<!-- kladd: font-family: \"Liberation Serif\"; font-size: 12pt; page-size: Letter; page-margin: 20mm 25mm 20mm 25mm; footer: \"Sida {sida} av {sidor}\"; footer-align: center; footer-dist: 20mm; h1: \"Liberation Sans\" 18.2pt -->\n\n"),
            "{md}"
        );
        assert!(md.contains("\n# Rubrik ett\n"), "fetstil i rubriken kommer från formatmallen: {md}");
        assert!(md.contains("Vanlig **fet** och <u>*~~kursiv struken~~*</u> samt "), "{md}");
        assert!(
            md.contains("<span style=\"font-family: 'Liberation Sans'; font-size: 16pt; color: #FF0000; background-color: #FFFF00\">röd</span>!"),
            "{md}"
        );
        assert!(md.contains("med   mellanrum\ttab\nny rad och [länk](https://example.com)"), "{md}");
        assert!(md.contains("<div align=\"center\">\n\nCentrerat\n\n</div>"), "{md}");
        assert!(md.contains("<div align=\"justify\">\n\nMarginal\n\n</div>"), "{md}");
        assert!(md.contains("\n- ett\n- två\n    1. a\n    2. b\n"), "{md}");
        assert!(md.contains("\n> Ett citat\n"), "{md}");
        assert!(md.contains("```\nfn main() {\n    x();\n}\n```"), "{md}");
        assert!(md.contains("| Namn | Antal |\n| --- | --- |\n| Äpple \\| rött | 3 |"), "{md}");
        assert!(md.contains("![Alternativtext](bild:a.png)"), "{md}");
        // Fotnoten blir en referens och en definition sist.
        assert!(md.contains("[länk](https://example.com)[^1]."), "{md}");
        assert!(md.ends_with("Sista\n\n[^1]: Fotnot\n"), "{md}");
        assert_eq!(l.images.len(), 1);
    }

    fn odt_part(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut out = String::new();
        std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut out).unwrap();
        out
    }

    #[test]
    fn header_fields_parts_and_first_page_roundtrip() {
        let md = "<!-- kladd: header: \"Rapport|{titel}|{datum}\"; footer: \"Sida {sida} av {sidor}\"; footer-align: center; header-first: \"\"; footer-first: \"{fil}\" -->\n\nText\n";
        let bytes = write(md, "Min rapport", &|_| None).unwrap().bytes;
        let styles = odt_part(&bytes, "styles.xml");
        assert!(styles.contains("<text:title>Min rapport</text:title>"), "{styles}");
        assert!(styles.contains("<text:date text:date-value=\""), "{styles}");
        assert!(styles.contains("<text:file-name text:display=\"name-and-extension\">Min rapport.odt</text:file-name>"), "{styles}");
        assert!(styles.contains("<text:tab/>") && styles.contains("style:type=\"right\""), "{styles}");
        assert!(styles.contains("<style:header-first>") && styles.contains("<style:footer-first>"), "{styles}");
        assert!(styles.contains("<text:page-count>"), "{styles}");
        let back = read(&bytes).unwrap();
        let got = DocDefaults::from_comment(back.markdown.lines().next().unwrap()).unwrap();
        assert_eq!(got, DocDefaults::from_comment(md.lines().next().unwrap()).unwrap());
        // Utan första sida skrivs inga `-first`-delar.
        let plain = write("<!-- kladd: footer: \"{sida}\" -->\n\nText\n", "x", &|_| None).unwrap().bytes;
        let s = odt_part(&plain, "styles.xml");
        assert!(!s.contains("header-first") && !s.contains("footer-first"), "{s}");
    }

    #[test]
    fn frame_with_other_size_gets_width() {
        let c = LO_CONTENT.replace("svg:width=\"0.0265cm\" svg:height=\"0.0265cm\"", "svg:width=\"6.35cm\" svg:height=\"6.35cm\"");
        let l = read(&make_odt(&c, LO_STYLES, &[("Pictures/a.png", &png())])).unwrap();
        assert!(l.markdown.contains("<img src=\"bild:a.png\" alt=\"Alternativtext\" width=\"240\">"), "{}", l.markdown);
    }

    #[test]
    fn rejects_other_formats() {
        assert!(read(b"inte en zip").is_err());
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("mimetype", o).unwrap();
            z.write_all(b"application/vnd.oasis.opendocument.spreadsheet").unwrap();
            z.finish().unwrap();
        }
        assert!(read(&buf.into_inner()).err().unwrap().contains("inte ett OpenDocument"));
    }

    fn content_part(bytes: &[u8]) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut out = String::new();
        Read::read_to_string(&mut z.by_name("content.xml").unwrap(), &mut out).unwrap();
        out
    }

    #[test]
    fn table_merge_widths_and_breaks_roundtrip() {
        let md = "| Rubrik över två || x |\n| ------ | -- | ---- |\n| a | b<br>rad | c |\n| d |||\n";
        let w = write(md, "T", &resolver()).unwrap();
        let xml = content_part(&w.bytes);
        assert!(xml.contains("table:number-columns-spanned=\"2\""), "{xml}");
        assert!(xml.contains("table:number-columns-spanned=\"3\""));
        assert_eq!(xml.matches("<table:covered-table-cell/>").count(), 3, "{xml}");
        assert!(xml.contains("b<text:line-break/>rad"), "{xml}");
        // En kolumnformat per kolumn, i förhållande 6:2:4.
        let width = |name: &str| -> f32 {
            let i = xml.find(&format!("style:name=\"{name}\"")).unwrap();
            let rest = &xml[i..];
            let j = rest.find("style:column-width=\"").unwrap() + 20;
            rest[j..].split("cm").next().unwrap().parse().unwrap()
        };
        let (a, b, c) = (width("Tabell1.C0"), width("Tabell1.C1"), width("Tabell1.C2"));
        assert!((a / b - 3.0).abs() < 0.05 && (c / b - 2.0).abs() < 0.05, "{a} {b} {c}");
        let back = read(&w.bytes).unwrap();
        let ts = markup::table_src(&back.markdown);
        assert_eq!(ts.spans, vec![vec![2, 0, 1], vec![1, 1, 1], vec![3, 0, 0]], "{}", back.markdown);
        assert_eq!(ts.weights, Some(vec![20, 7, 13]), "{}", back.markdown);
        assert!(back.markdown.contains("| a | b<br>rad | c |"), "{}", back.markdown);
    }

    #[test]
    fn table_equal_widths_use_one_repeated_column() {
        let md = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let w = write(md, "T", &resolver()).unwrap();
        let xml = content_part(&w.bytes);
        assert!(xml.contains("table:number-columns-repeated=\"2\""), "{xml}");
        assert!(!xml.contains("covered-table-cell") && !xml.contains("columns-spanned"));
        assert!(read(&w.bytes).unwrap().markdown.contains("| A | B |\n| --- | --- |\n| 1 | 2 |"));
    }

    #[test]
    fn reads_spans_covered_cells_and_widths_from_libreoffice() {
        let table = r#"<table:table table:name="T"><table:table-column table:style-name="co1"/><table:table-column table:style-name="co2" table:number-columns-repeated="2"/>
<table:table-row><table:table-cell table:number-columns-spanned="2" office:value-type="string"><text:p>Bred</text:p></table:table-cell><table:covered-table-cell/><table:table-cell office:value-type="string"><text:p>Id</text:p></table:table-cell></table:table-row>
<table:table-row><table:table-cell table:number-rows-spanned="2" office:value-type="string"><text:p>Lodrät</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>rad<text:line-break/>två</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>c</text:p></table:table-cell></table:table-row>
<table:table-row><table:covered-table-cell/><table:table-cell office:value-type="string"><text:p>e</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>f</text:p></table:table-cell></table:table-row></table:table>"#;
        let styles = r#"<style:style style:name="co1" style:family="table-column"><style:table-column-properties style:column-width="8cm"/></style:style><style:style style:name="co2" style:family="table-column"><style:table-column-properties style:column-width="4cm"/></style:style>"#;
        let a = LO_CONTENT.find("<table:table ").unwrap();
        let b = LO_CONTENT.find("</table:table>").unwrap() + "</table:table>".len();
        let c = format!("{}{}{}", &LO_CONTENT[..a], table, &LO_CONTENT[b..]).replacen("<office:automatic-styles>", &format!("<office:automatic-styles>{styles}"), 1);
        let l = read(&make_odt(&c, LO_STYLES, &[("Pictures/a.png", &png())])).unwrap();
        let md = &l.markdown;
        let start = md.find("| Bred").unwrap();
        let tbl = &md[start..md[start..].find("\n\n").map_or(md.len(), |i| start + i)];
        let ts = markup::table_src(tbl);
        // Bred över två kolumner; den lodräta täckningen blir en tom cell, inte en sammanfogning.
        assert_eq!(ts.spans, vec![vec![2, 0, 1], vec![1, 1, 1], vec![1, 1, 1]], "{tbl}");
        // 8 cm : 4 cm : 4 cm av 40 streck = 20:10:10.
        assert_eq!(ts.weights, Some(vec![20, 10, 10]), "{tbl}");
        assert!(tbl.contains("| Lodrät | rad<br>två | c |"), "{tbl}");
        assert!(tbl.contains("|   | e | f |"), "{tbl}");
    }

    /// Konverterar Kladds .odt med LibreOffice; kör med `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn libreoffice_converts_our_output() {
        let dir = std::env::temp_dir().join("kladd-test-odt-soffice");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let w = write(RICH, "Test", &resolver()).unwrap();
        let odt = dir.join("rik.odt");
        std::fs::write(&odt, &w.bytes).unwrap();
        let out = std::process::Command::new("soffice")
            .args(["--headless", "--convert-to", "docx", "--outdir"])
            .arg(&dir)
            .arg(&odt)
            .output()
            .expect("soffice saknas");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let docx = std::fs::read(dir.join("rik.docx")).expect("ingen docx skapades");
        let back = crate::docx::read(&docx).unwrap();
        assert!(back.markdown.contains("Rubrik ett") && back.markdown.contains("fjärde"), "{}", back.markdown);
        eprintln!("{}", back.markdown);
    }

    #[test]
    fn table_format_gives_cell_styles_and_hides_the_comment() {
        use crate::word::tablefmt::{TableFmt, Width};
        let mut f = TableFmt::word_base();
        f.cell_mut(0, 0).fill = Some([0x5B, 0x3A, 0x9E]);
        f.width = Width::Pct(50);
        let md = format!("{}\n\n| A | B |\n| - | - |\n| c | d |\n", f.to_comment());
        let w = write(&md, "x", &|_| None).unwrap();
        let content = {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(&w.bytes)).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut z.by_name("content.xml").unwrap(), &mut s).unwrap();
            s
        };
        assert!(!content.contains("kladd-tabell"), "{content}");
        assert!(content.contains("fo:background-color=\"#5B3A9E\""), "{content}");
        assert!(content.contains("fo:border-top=\"none\""), "{content}");
        assert!(content.contains("table:style-name=\"Tabell1.R0C0\""), "{content}");
    }
}

#[cfg(test)]
#[path = "odt_fn_tests.rs"]
mod fn_tests;

#[cfg(test)]
#[path = "odt_review_tests.rs"]
mod review_tests;

#[cfg(test)]
#[path = "odt_section_tests.rs"]
mod section_tests;

#[path = "odt_hf.rs"]
mod hf;
