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
//! - Fotnoter och slutnoter (`text:note`) blir `[^N]` och en definition `[^N]: text` sist i dokumentet
//!   (slutnoter blir fotnoter). Kommentarer (`office:annotation`) tas bort ur texten och rapporteras
//!   som borttagna ("kommentarer"), på samma sätt som för .docx.
//! - Diagram, former och inbäddade objekt (`draw:*` utom bilder) rapporteras som borttagna.
//! - Länkade bilder (utanför paketet) och bildformat som Kladd inte kan visa (svg, wmf …) likaså.
//! - Sidnummer och antal sidor i sidhuvud/sidfot blir `{sida}` och `{sidor}`; bara standardsidans
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
use crate::style::{self, Align, DocDefaults, SpanStyle};
use crate::word::markup::{self, Float};

const MIME: &str = "application/vnd.oasis.opendocument.text";
const MAX_ENTRIES: usize = 20_000;
const MAX_PART_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 512 * 1024 * 1024;
const MAX_DEPTH: usize = 256;

// ============================================================== XML-träd

struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Kid>,
}

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
                    return Err("OpenDocument-filen är för djupt nästlad för att öppnas.".into());
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
            Err(e) => return Err(format!("OpenDocument-filen är skadad: {e}")),
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
            return Err("En del av dokumentet är för stor för att öppnas.".into());
        }
        let mut buf = Vec::with_capacity(file.size() as usize);
        let res = file.by_ref().take(room + 1).read_to_end(&mut buf);
        self.total += buf.len() as u64;
        res.map_err(|e| format!("OpenDocument-filen är skadad: {e}"))?;
        if buf.len() as u64 > room {
            return Err("En del av dokumentet är för stor för att öppnas.".into());
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

#[derive(Clone, Default)]
struct StyleDef {
    name: String,
    display: String,
    parent: Option<String>,
    auto: bool,
    tp: TProps,
    align: Option<Align>,
    /// `style:wrap` och `style:horizontal-pos` i `style:graphic-properties`.
    wrap: Option<String>,
    hpos: Option<String>,
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
            ..Default::default()
        };
        for c in s.els() {
            match c.name.as_str() {
                "style:text-properties" => d.tp = self.text_props(c),
                "style:paragraph-properties" => {
                    if let Some(a) = c.attr("fo:text-align") {
                        d.align = Some(parse_align(a));
                    }
                }
                "style:graphic-properties" => {
                    d.wrap = c.attr("style:wrap").map(str::to_owned);
                    d.hpos = c.attr("style:horizontal-pos").map(str::to_owned);
                }
                _ => {}
            }
        }
        d
    }

    /// Sidan en ram med grafikformatet `name` flyter på. `style:wrap="right"` betyder att texten
    /// står till höger om bilden (bilden till vänster), `left` tvärtom; vid `parallel`, `dynamic`
    /// och `biggest` avgör `style:horizontal-pos` (`left`/`right`). Allt annat (`none`, `run-through`,
    /// ramar som ligger i texten) är ingen flytande bild.
    fn float_of(&self, name: &str) -> Option<Float> {
        let (mut wrap, mut hpos) = (None, None);
        for d in self.chain("graphic", name) {
            wrap = d.wrap.as_ref().or(wrap);
            hpos = d.hpos.as_ref().or(hpos);
        }
        match wrap?.as_str() {
            "right" => Some(Float::Left),
            "left" => Some(Float::Right),
            "parallel" | "dynamic" | "biggest" => match hpos.map(String::as_str) {
                Some("left") => Some(Float::Left),
                Some("right") => Some(Float::Right),
                _ => None,
            },
            _ => None,
        }
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
}

#[derive(Default)]
struct Dropped {
    shapes: bool,
    linked: bool,
    textboxes: bool,
    comments: bool,
}

impl Dropped {
    fn list(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.shapes {
            v.push("diagram och former");
        }
        if self.linked {
            v.push("länkade bilder");
        }
        if self.textboxes {
            v.push("textrutor");
        }
        if self.comments {
            v.push("kommentarer");
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
    notes: Vec<String>,
    /// Sant medan en fotnots stycken läses.
    in_note: bool,
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
                "title" => return Kind::Heading(1),
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
            "office:annotation" => self.dropped.comments = true,
            "text:soft-page-break" | "text:bookmark" | "text:bookmark-start" | "text:bookmark-end" | "text:reference-mark"
            | "text:reference-mark-start" | "text:reference-mark-end" | "office:annotation-end" | "text:index-mark"
            | "text:index-mark-start" | "text:index-mark-end" | "text:changed-region" | "text:change" | "text:change-start"
            | "text:change-end" | "text:toc-mark" | "text:toc-mark-start" | "text:toc-mark-end" => {}
            n if n.starts_with("draw:") => self.dropped.shapes = true,
            _ => self.inline(e, st, pc, runs),
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
        self.notes.push(escape_line_start(&parts.join("<br>")));
        let label = format!("[^{}]", self.notes.len());
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
        if e.child("draw:text-box").is_some() {
            self.dropped.textboxes = true;
            return;
        }
        let Some(img) = e.child("draw:image") else {
            self.dropped.shapes = true;
            return;
        };
        let Some(href) = img.attr("xlink:href").map(|h| h.trim().to_owned()) else {
            self.dropped.shapes = true;
            return;
        };
        if href.contains("://") || href.starts_with("..") || href.starts_with('/') {
            self.dropped.linked = true;
            return;
        }
        let target = href.trim_start_matches("./").to_owned();
        let name = if let Some(n) = self.by_target.get(&target) {
            n.clone()
        } else {
            let Some(bytes) = self.pkg.bytes(&target).ok().flatten() else {
                self.dropped.shapes = true;
                return;
            };
            if !is_supported_image(&bytes) {
                self.dropped.shapes = true;
                return;
            }
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
            self.images.push((name.clone(), Image { bytes, extent }));
            self.by_target.insert(target.clone(), name.clone());
            name
        };
        let alt = ["svg:desc", "svg:title"]
            .iter()
            .find_map(|k| e.child(k).map(|n| n.deep_text()).filter(|t| !t.trim().is_empty()))
            .unwrap_or_default();
        let (_, image) = self.images.iter().find(|(m, _)| *m == name).unwrap();
        let natural = imagesize::blob_size(&image.bytes).ok().map(|s| (s.width as f32, s.height as f32));
        let fw = e.attr("svg:width").and_then(style::parse_length).map(|pt| pt * 96.0 / 72.0);
        let fh = e.attr("svg:height").and_then(style::parse_length).map(|pt| pt * 96.0 / 72.0);
        let src = format!("{IMAGE_SCHEME}{}", name.replace(' ', "%20"));
        // Ramar som ligger i texten (`as-char`) flyter aldrig.
        let float = e
            .attr("draw:style-name")
            .filter(|_| e.attr("text:anchor-type") != Some("as-char"))
            .and_then(|n| self.st.float_of(n));
        let md = match (natural, fw) {
            (Some((nw, nh)), Some(w))
                if float.is_some() || (w - nw).abs() > 1.5 || fh.is_some_and(|h| (h - w * nh / nw).abs() > 1.5) =>
            {
                let mut tag = format!("<img src=\"{}\"", html_escape_attr(&src));
                if !alt.is_empty() {
                    tag.push_str(&format!(" alt=\"{}\"", html_escape_attr(&alt)));
                }
                tag.push_str(&format!(" width=\"{}\"", w.round() as i64));
                if let Some(h) = fh.filter(|h| (h - w * nh / nw).abs() > 1.5) {
                    tag.push_str(&format!(" height=\"{}\"", h.round() as i64));
                }
                match float {
                    Some(Float::Left) => tag.push_str(" align=\"left\""),
                    Some(Float::Right) => tag.push_str(" align=\"right\""),
                    None => {}
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

    fn emit(&mut self, p: Para) {
        if p.kind == Kind::Rule {
            self.blocks.push(Block::Rule);
            return;
        }
        if p.kind == Kind::Code && p.list.is_none() {
            if let Some(Block::Para(last)) = self.blocks.last_mut() {
                if last.kind == Kind::Code && last.list.is_none() {
                    let raw: String = p.runs.iter().map(|r| r.text.as_str()).collect();
                    let first = &mut last.runs[0];
                    first.text.push('\n');
                    first.text.push_str(&raw);
                    return;
                }
            }
            // Ett kodblock samlas i en enda körning.
            let raw: String = p.runs.iter().map(|r| r.text.as_str()).collect();
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

    fn walk(&mut self, n: &Node) {
        match n.name.as_str() {
            "text:p" | "text:h" => {
                let p = self.build_para(n, None);
                self.emit(p);
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

/// Text i ett sidhuvud eller en sidfot med fält som `{sida}` och `{sidor}`.
fn hf_collect(n: &Node, out: &mut String) {
    for k in &n.kids {
        match k {
            Kid::Text(t) => {
                for c in t.chars() {
                    if c.is_whitespace() {
                        if !out.ends_with(' ') && !out.is_empty() {
                            out.push(' ');
                        }
                    } else {
                        out.push(c);
                    }
                }
            }
            Kid::El(e) => match e.name.as_str() {
                "text:page-number" => out.push_str("{sida}"),
                "text:page-count" => out.push_str("{sidor}"),
                "text:s" => {
                    let c = e.attr("text:c").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(100);
                    out.push_str(&" ".repeat(c));
                }
                "text:tab" | "text:line-break" => out.push(' '),
                "text:note" | "office:annotation" => {}
                _ => hf_collect(e, out),
            },
        }
    }
}

fn header_footer(st: &Styles, n: &Node) -> Option<(String, Align)> {
    let mut parts = Vec::new();
    let mut align = None;
    for p in n.els().filter(|p| matches!(p.name.as_str(), "text:p" | "text:h")) {
        let mut s = String::new();
        hf_collect(p, &mut s);
        let s = s.trim().to_owned();
        if !s.is_empty() {
            if align.is_none() {
                align = Some(st.align(p.attr("text:style-name")));
            }
            parts.push(s);
        }
    }
    (!parts.is_empty()).then(|| (parts.join(" "), align.unwrap_or(Align::Left)))
}

fn read_page(st: &Styles, styles_root: &Node, d: &mut DocDefaults) {
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
    if let Some((t, a)) = master.child("style:header").and_then(|h| header_footer(st, h)) {
        d.header = Some(t);
        d.header_align = a;
    }
    if let Some((t, a)) = master.child("style:footer").and_then(|h| header_footer(st, h)) {
        d.footer = Some(t);
        d.footer_align = a;
    }
}

/// Läser ett OpenDocument-textdokument som Markdown.
pub fn read(bytes: &[u8]) -> Result<Loaded, String> {
    if !bytes.starts_with(b"PK") {
        return Err("Filen är inte ett OpenDocument-textdokument (.odt).".into());
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err("Dokumentet är för stort för att öppnas.".into());
    }
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("OpenDocument-filen är skadad: {e}"))?;
    if zip.len() > MAX_ENTRIES {
        return Err("Dokumentet innehåller orimligt många delar och öppnas inte.".into());
    }
    let mut pkg = Pkg { zip, total: 0 };
    if let Some(m) = pkg.text("mimetype")? {
        if !m.trim().starts_with("application/vnd.oasis.opendocument.text") {
            return Err("Filen är inte ett OpenDocument-textdokument (.odt).".into());
        }
    }
    if pkg.text("META-INF/manifest.xml")?.is_some_and(|m| m.contains("encryption-data")) {
        return Err("Dokumentet är lösenordsskyddat. Öppna det i LibreOffice och spara det utan lösenord.".into());
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
        .ok_or_else(|| "Filen är inte ett OpenDocument-textdokument (.odt).".to_owned())?;

    let mut rd = Rd {
        st: &st,
        base: base.clone(),
        pkg,
        dropped: Dropped::default(),
        images: Vec::new(),
        by_target: HashMap::new(),
        blocks: Vec::new(),
        list_id: 0,
        notes: Vec::new(),
        in_note: false,
    };
    for c in body.els() {
        rd.walk(c);
    }
    let mut defaults = DocDefaults { font_family: base.font.clone(), font_size: base.size, ..Default::default() };
    if let Some(s) = &sroot {
        read_page(&st, s, &mut defaults);
    }
    let text_w = {
        let (w, _) = defaults.page();
        let m = defaults.margins();
        f64::from(w - m[1] - m[3])
    };
    let mut markdown = render(&rd.blocks, Some(text_w));
    if !rd.notes.is_empty() {
        markdown.push('\n');
        for (i, n) in rd.notes.iter().enumerate() {
            markdown.push_str(&format!("[^{}]: {n}\n", i + 1));
        }
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

fn cm(pt: f32) -> String {
    format!("{}cm", fmt_num(pt * 2.54 / 72.0, 4))
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
    quote: u32,
    code: bool,
    table: Option<WTable>,
    cell_style: bool,
    frame_style: bool,
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
            quote: 0,
            code: false,
            table: None,
            cell_style: false,
            frame_style: false,
            image: None,
            media: Vec::new(),
            missing: Vec::new(),
            text_w,
            notes: Vec::new(),
            note_order: Vec::new(),
            in_note: false,
            toc: Vec::new(),
        }
    }

    fn base_para_style(&self) -> String {
        if self.in_note {
            "Footnote".into()
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
        if align == Align::Left || self.code {
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
        let style = self.para_style();
        if let Some(h) = self.heading {
            self.body.push_str(&format!("<text:h text:style-name=\"{style}\" text:outline-level=\"{h}\">"));
            self.open = Some("</text:h>");
        } else {
            self.body.push_str(&format!("<text:p text:style-name=\"{style}\">"));
            self.open = Some("</text:p>");
        }
        self.after_space = true;
    }

    fn close_link(&mut self) {
        if self.link_open {
            self.body.push_str("</text:a>");
            self.link_open = false;
        }
    }

    fn close_para(&mut self) {
        if let Some(t) = self.open.take() {
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
        if text.is_empty() {
            return;
        }
        self.open_para();
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

    fn image(&mut self, src: &str, alt: &str, width: Option<f32>, height: Option<f32>, float: Option<Float>) {
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
        let path = match self.media.iter().find(|(_, b, _)| *b == img.bytes) {
            Some((p, _, _)) => p.clone(),
            None => {
                let p = format!("Pictures/image{}.{ext}", self.media.len() + 1);
                self.media.push((p.clone(), img.bytes.clone(), mime));
                p
            }
        };
        let natural = imagesize::blob_size(&img.bytes).ok().map(|s| (s.width as f32, s.height as f32));
        let ratio = natural.map(|(w, h)| h / w.max(1.0)).unwrap_or(0.66);
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
        // Flytande bild: förankrad vid stycket, vid kanten, text runt med 12 px (0,318 cm) luft.
        let (style_name, anchor) = match float {
            Some(side) => {
                let (name, pos, wrap, ml, mr) = match side {
                    Float::Left => ("frL", "left", "right", "0cm", "0.318cm"),
                    Float::Right => ("frR", "right", "left", "0.318cm", "0cm"),
                };
                if !self.auto.contains(&format!("style:name=\"{name}\"")) {
                    self.auto.push_str(&format!("<style:style style:name=\"{name}\" style:family=\"graphic\" style:parent-style-name=\"Graphics\"><style:graphic-properties style:vertical-pos=\"top\" style:vertical-rel=\"paragraph\" style:horizontal-pos=\"{pos}\" style:horizontal-rel=\"paragraph\" style:wrap=\"{wrap}\" style:number-wrapped-paragraphs=\"no-limit\" style:run-through=\"foreground\" fo:margin-left=\"{ml}\" fo:margin-right=\"{mr}\" fo:margin-top=\"0cm\" fo:margin-bottom=\"0cm\"/></style:style>"));
                }
                (name, "paragraph")
            }
            None => ("fr1", "as-char"),
        };
        self.open_para();
        self.open_link_if_needed();
        self.n_frame += 1;
        let n = self.n_frame;
        let desc = if alt.is_empty() { String::new() } else { format!("<svg:desc>{}</svg:desc>", esc(alt)) };
        self.body.push_str(&format!(
            "<draw:frame draw:style-name=\"{style_name}\" draw:name=\"Bild{n}\" text:anchor-type=\"{anchor}\" svg:width=\"{}\" svg:height=\"{}\" draw:z-index=\"{}\"><draw:image xlink:href=\"{path}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\" draw:mime-type=\"{mime}\"/>{desc}</draw:frame>",
            cm(w_pt),
            cm(h_pt),
            n - 1
        ));
        self.after_space = false;
    }

    /// Skriver innehållsförteckningen: `text:table-of-content` med rubriknivåer 1–3 och en
    /// färdig förteckning (LibreOffice uppdaterar den via Verktyg > Uppdatera).
    fn toc(&mut self) {
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

    fn html(&mut self, s: &str) {
        let t = s.trim();
        let single_line = t.lines().count() == 1;
        if t == crate::word::nav::TOC_LINE {
            self.close_para();
            self.toc();
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
            self.image(&src, &alt, num("width"), num("height"), float);
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
                let k = n + 1;
                out.push_str(&format!(
                    "<text:note text:id=\"ftn{k}\" text:note-class=\"footnote\"><text:note-citation>{k}</text:note-citation><text:note-body>{paras}</text:note-body></text:note>"
                ));
            }
            rest = &r[e + '\u{E005}'.len_utf8()..];
        }
        out.push_str(rest);
        self.body = out;
    }

    fn start_list(&mut self, start: Option<u64>) {
        self.close_para();
        self.n_list += 1;
        let name = format!("L{}", self.n_list);
        self.auto.push_str(&list_style_xml(&name, start.is_some(), start.unwrap_or(1)));
        self.body.push_str(&format!("<text:list text:style-name=\"{name}\">"));
    }
}

fn build_body<'a>(md: &str, resolve: &'a dyn Fn(&str) -> Option<Image>, text_w: f32) -> W<'a> {
    let mut w = W::new(resolve, text_w);
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
                Tag::Heading { level, .. } => {
                    w.close_para();
                    w.heading = Some(heading_num(level));
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
                    let cols = aligns.len().max(1);
                    w.n_table += 1;
                    let n = w.n_table;
                    if !w.cell_style {
                        w.cell_style = true;
                        w.auto.push_str("<style:style style:name=\"TabellCell\" style:family=\"table-cell\"><style:table-cell-properties fo:padding=\"0.097cm\" fo:border=\"0.5pt solid #000000\"/></style:style>");
                    }
                    let src = markup::table_src(md.get(range.clone()).unwrap_or(""));
                    w.auto.push_str(&format!(
                        "<style:style style:name=\"Tabell{n}\" style:family=\"table\"><style:table-properties style:width=\"{tw}\" table:align=\"margins\"/></style:style>",
                        tw = cm(w.text_w)
                    ));
                    w.body.push_str(&format!("<table:table table:name=\"Tabell{n}\" table:style-name=\"Tabell{n}\">"));
                    match src.weights.as_ref().filter(|ws| ws.len() == cols) {
                        // Olika många streck: en kolumnformat per kolumn med proportionell bredd.
                        Some(ws) => {
                            let sum: usize = ws.iter().sum();
                            for (j, &d) in ws.iter().enumerate() {
                                w.auto.push_str(&format!(
                                    "<style:style style:name=\"Tabell{n}.C{j}\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{}\"/></style:style>",
                                    cm(w.text_w * d as f32 / sum as f32)
                                ));
                                w.body.push_str(&format!("<table:table-column table:style-name=\"Tabell{n}.C{j}\"/>"));
                            }
                        }
                        None => {
                            w.auto.push_str(&format!(
                                "<style:style style:name=\"Tabell{n}.A\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{cw}\"/></style:style>",
                                cw = cm(w.text_w / cols as f32)
                            ));
                            w.body.push_str(&format!(
                                "<table:table-column table:style-name=\"Tabell{n}.A\" table:number-columns-repeated=\"{cols}\"/>"
                            ));
                        }
                    }
                    w.table = Some(WTable { in_head: false, spans: src.spans, rows: 0, cell: 0, covered: false });
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
                    match span {
                        // Täckt av cellen före: ingen text, bara platshållaren.
                        0 => w.body.push_str("<table:covered-table-cell/>"),
                        1 => w.body.push_str("<table:table-cell table:style-name=\"TabellCell\" office:value-type=\"string\">"),
                        n => w.body.push_str(&format!(
                            "<table:table-cell table:style-name=\"TabellCell\" office:value-type=\"string\" table:number-columns-spanned=\"{n}\">"
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
                        w.image(&src, &alt, None, None, None);
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
                w.body.push_str("<text:p text:style-name=\"Horizontal_20_Line\"/>");
            }
            MdEvent::TaskListMarker(done) => w.run(if done { "☒ " } else { "☐ " }, false),
            MdEvent::InlineMath(t) | MdEvent::DisplayMath(t) => w.run(&t, false),
        }
    }
    w.close_para();
    w.resolve_notes();
    w
}

const NS: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\" xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\"";
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

/// Sidhuvudets eller sidfotens stycke med fälten `{sida}` och `{sidor}`.
fn hf_paragraph(text: &str, style: &str) -> String {
    let mut inner = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let s = rest.find("{sida}");
        let n = rest.find("{sidor}");
        let (at, len, field) = match (s, n) {
            (Some(a), Some(b)) if b < a => (b, 7, "<text:page-count>1</text:page-count>"),
            (Some(a), _) => (a, 6, "<text:page-number text:select-page=\"current\">1</text:page-number>"),
            (None, Some(b)) => (b, 7, "<text:page-count>1</text:page-count>"),
            (None, None) => {
                inner.push_str(&hf_text(rest));
                break;
            }
        };
        inner.push_str(&hf_text(&rest[..at]));
        inner.push_str(field);
        rest = &rest[at + len..];
    }
    format!("<text:p text:style-name=\"{style}\">{inner}</text:p>")
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

fn styles_xml(d: &DocDefaults) -> String {
    let font = d.font().to_owned();
    let size = pt_str(d.size());
    let mut fonts = BTreeSet::new();
    fonts.insert(font.clone());
    fonts.insert("Liberation Mono".to_owned());
    let (pw, ph) = d.page();
    let m = d.margins();
    let orient = if pw > ph { "landscape" } else { "portrait" };
    let header = d.header.as_deref().filter(|t| !t.is_empty());
    let footer = d.footer.as_deref().filter(|t| !t.is_empty());
    let hf_style = |tag: &str| {
        format!(
            "<style:{tag}-style><style:header-footer-properties fo:min-height=\"0.6cm\" fo:margin-left=\"0cm\" fo:margin-right=\"0cm\" fo:margin-{side}=\"0.3cm\"/></style:{tag}-style>",
            side = if tag == "header" { "bottom" } else { "top" }
        )
    };
    let mut auto = format!(
        "<style:page-layout style:name=\"pm1\"><style:page-layout-properties fo:page-width=\"{}\" fo:page-height=\"{}\" style:num-format=\"1\" style:print-orientation=\"{orient}\" fo:margin-top=\"{}\" fo:margin-bottom=\"{}\" fo:margin-left=\"{}\" fo:margin-right=\"{}\" style:writing-mode=\"lr-tb\"/>",
        cm(pw),
        cm(ph),
        cm(m[0]),
        cm(m[2]),
        cm(m[3]),
        cm(m[1])
    );
    auto.push_str(&hf_style("header"));
    auto.push_str(&hf_style("footer"));
    auto.push_str("</style:page-layout>");
    let mut master = String::from("<style:master-page style:name=\"Standard\" style:page-layout-name=\"pm1\">");
    for (tag, text, align, auto_name, parent) in [
        ("header", header, d.header_align, "MP1", "Header"),
        ("footer", footer, d.footer_align, "MP2", "Footer"),
    ] {
        if let Some(t) = text {
            auto.push_str(&format!(
                "<style:style style:name=\"{auto_name}\" style:family=\"paragraph\" style:parent-style-name=\"{parent}\"><style:paragraph-properties fo:text-align=\"{}\" style:justify-single-word=\"false\"/></style:style>",
                align_attr(align)
            ));
            master.push_str(&format!("<style:{tag}>{}</style:{tag}>", hf_paragraph(t, auto_name)));
        }
    }
    master.push_str("</style:master-page>");

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

    named.push_str("<text:notes-configuration text:note-class=\"footnote\" style:num-format=\"1\" text:start-value=\"0\" text:footnotes-position=\"page\" text:start-numbering-at=\"document\" text:citation-style-name=\"Footnote_20_Symbol\" text:citation-body-style-name=\"Footnote_20_anchor\"/>");

    format!(
        "{XML_DECL}<office:document-styles {NS} office:version=\"1.2\">{}<office:styles>{named}</office:styles><office:automatic-styles>{auto}</office:automatic-styles><office:master-styles>{master}</office:master-styles></office:document-styles>",
        font_decls(&fonts)
    )
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
    let first = markdown.lines().next().unwrap_or("");
    let (defaults, markdown) = match DocDefaults::from_comment(first) {
        Some(d) => (d, markdown[first.len()..].trim_start_matches(['\n', '\r'])),
        None => (DocDefaults::default(), markdown),
    };
    let (pw, _) = defaults.page();
    let m = defaults.margins();
    let text_w = (pw - m[1] - m[3]).max(36.0);
    let mut w = build_body(markdown, resolve, text_w);
    w.fonts.insert(defaults.font().to_owned());
    let content = format!(
        "{XML_DECL}<office:document-content {NS} office:version=\"1.2\">{}<office:automatic-styles>{}</office:automatic-styles><office:body><office:text>{}</office:text></office:body></office:document-content>",
        font_decls(&w.fonts),
        w.auto,
        w.body
    );

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
            ("styles.xml", styles_xml(&defaults)),
            ("meta.xml", meta_xml(title)),
        ] {
            zip.start_file(name, deflated).map_err(|e| e.to_string())?;
            zip.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        for (path, bytes, _) in &w.media {
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
        move |src| (src == "bild:image1.png").then(|| Image { bytes: p.clone(), extent: None })
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
        assert!(base("right", "center").contains("align=\"left\""));
        assert!(base("left", "center").contains("align=\"right\""));
        assert!(base("parallel", "right").contains("align=\"right\""));
        assert!(base("dynamic", "left").contains("align=\"left\""));
        assert!(!base("none", "left").contains("align="));
        assert!(!base("run-through", "left").contains("align="));
        assert!(!base("parallel", "center").contains("align="));
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
        let w = write(&md, "x", &|_| Some(Image { bytes: png.clone(), extent: None })).unwrap();
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
    fn reads_libreoffice_style_document() {
        let bytes = make_odt(LO_CONTENT, LO_STYLES, &[("Pictures/a.png", &png())]);
        let l = read(&bytes).unwrap();
        assert!(l.dropped.is_empty(), "{:?}", l.dropped);
        let md = &l.markdown;
        assert!(
            md.starts_with("<!-- kladd: font-family: \"Liberation Serif\"; font-size: 12pt; page-size: Letter; page-margin: 20mm 25mm 20mm 25mm; footer: \"Sida {sida} av {sidor}\"; footer-align: center -->\n\n"),
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
}

#[cfg(test)]
#[path = "odt_fn_tests.rs"]
mod fn_tests;
