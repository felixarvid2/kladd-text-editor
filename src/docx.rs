//! Läser och skriver Word-dokument (.docx) som Markdown-text.
//!
//! Tekniken följer WordCraft (github.com/storytold/wordcraft, MIT/Apache-2.0):
//! paketet öppnas med `zip`, antalet poster och storleken på varje del kontrolleras
//! innan något packas upp, huvuddokumentet hittas via `_rels/.rels`, och XML läses
//! händelse för händelse med `quick-xml`, där bara lokala namn jämförs så att både
//! vanlig och "Strict" OOXML fungerar.
//!
//! I Kladd visas dokumentet som Markdown: rubriker blir `#`, fetstil `**`, kursiv `*`,
//! listor `-`/`1.`, citat `>`, tabeller `|` och länkar `[text](url)`. När filen sparas
//! byggs ett nytt, giltigt Word-dokument av Markdown-texten.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::style::{self, Align, DocDefaults, SpanStyle};

/// Högsta antal poster i ett paket (WordCraft: zip_guard).
const MAX_ENTRIES: u64 = 20_000;
/// Största uppackade storlek för en enskild XML-del.
const MAX_PART_BYTES: u64 = 128 * 1024 * 1024;
/// Största .docx-fil som öppnas över huvud taget.
const MAX_FILE_BYTES: usize = 512 * 1024 * 1024;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// En bild i ett Word-dokument.
#[derive(Clone, Debug)]
pub struct Image {
    pub bytes: Vec<u8>,
    /// Storleken i originalet (EMU), om den är känd.
    pub extent: Option<(i64, i64)>,
}

/// Prefix för bilder som finns inbäddade i dokumentet, t.ex. `![Sol](bild:image1.png)`.
pub const IMAGE_SCHEME: &str = "bild:";

pub struct Loaded {
    pub markdown: String,
    /// Inbäddade bilder: namn → bild, i dokumentordning.
    pub images: Vec<(String, Image)>,
    /// Saker i originalet som Kladd inte kan visa eller spara, i läsbar form.
    pub dropped: Vec<&'static str>,
}

// ============================================================== läsning

/// Kontrollerar ett zip-pakets slutpost innan `zip` får tolka det, så att ett
/// paket som påstår sig ha miljontals poster avvisas direkt (jfr WordCraft #3660).
fn check_entry_count(bytes: &[u8]) -> Result<(), String> {
    const EOCD: &[u8] = b"PK\x05\x06";
    let start = bytes.len().saturating_sub(22 + 65_535);
    let Some(rel) = bytes[start..].windows(4).rposition(|w| w == EOCD) else {
        return Err("Filen är inte ett giltigt Word-dokument (zip-katalogen saknas).".into());
    };
    let at = start + rel;
    let total = bytes
        .get(at + 10..at + 12)
        .map(|b| u16::from_le_bytes([b[0], b[1]]) as u64)
        .unwrap_or(0);
    if total == u16::MAX as u64 {
        // ZIP64: läs det riktiga antalet från ZIP64-slutposten om den finns.
        if let Some(loc) = at.checked_sub(20) {
            if bytes.get(loc..loc + 4) == Some(b"PK\x06\x07".as_slice()) {
                let off = bytes
                    .get(loc + 8..loc + 16)
                    .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                    .unwrap_or(0) as usize;
                let n = bytes
                    .get(off + 32..off + 40)
                    .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                    .unwrap_or(u64::MAX);
                if n > MAX_ENTRIES {
                    return Err("Word-dokumentet innehåller orimligt många delar och öppnas inte.".into());
                }
            }
        }
    } else if total > MAX_ENTRIES {
        return Err("Word-dokumentet innehåller orimligt många delar och öppnas inte.".into());
    }
    Ok(())
}

/// Största sammanlagda uppackade mängd som läses ur ett paket.
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

struct Package<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    total: u64,
}

impl Package<'_> {
    fn part(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(buf) = self.part_bytes(name)? else { return Ok(None) };
        // XML i OOXML är UTF-8 (ibland med BOM) eller UTF-16 med BOM.
        let text = if buf.starts_with(&[0xFF, 0xFE]) {
            encoding_rs::UTF_16LE.decode_with_bom_removal(&buf).0.into_owned()
        } else if buf.starts_with(&[0xFE, 0xFF]) {
            encoding_rs::UTF_16BE.decode_with_bom_removal(&buf).0.into_owned()
        } else {
            let b = buf.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&buf);
            String::from_utf8_lossy(b).into_owned()
        };
        Ok(Some(text))
    }

    fn part_bytes(&mut self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let name = name.trim_start_matches('/');
        // Delnamn jämförs utan hänsyn till skiftläge (som i WordCraft).
        let real = if self.zip.index_for_name(name).is_some() {
            name.to_owned()
        } else {
            match self.zip.file_names().find(|n| n.trim_start_matches('/').eq_ignore_ascii_case(name)) {
                Some(n) => n.to_owned(),
                None => return Ok(None),
            }
        };
        let room = MAX_PART_BYTES.min(MAX_TOTAL_BYTES.saturating_sub(self.total));
        let Ok(mut file) = self.zip.by_name(&real) else { return Ok(None) };
        if file.size() > room {
            return Err("En del av Word-dokumentet är för stor för att öppnas.".into());
        }
        let mut buf = Vec::with_capacity(file.size() as usize);
        let res = file.by_ref().take(room + 1).read_to_end(&mut buf);
        // Räkna det som packats upp även om läsningen misslyckas (jfr WordCraft #3723).
        self.total += buf.len() as u64;
        res.map_err(|e| format!("Word-dokumentet är skadat: {e}"))?;
        if buf.len() as u64 > room {
            return Err("En del av Word-dokumentet är för stor för att öppnas.".into());
        }
        Ok(Some(buf))
    }
}

fn local(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Värdet på ett attribut, matchat på lokalt namn (`w:val`, `r:id` …).
fn attr(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| local(a.key.as_ref()) == name).map(|a| {
        a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned())
    })
}

/// `<w:b/>`, `<w:b w:val="1"/>` och `<w:b w:val="true"/>` betyder på; `0/false/off` betyder av.
fn on_off(e: &BytesStart<'_>) -> bool {
    !matches!(attr(e, b"val").as_deref(), Some("0" | "false" | "off" | "none"))
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

/// Relationer i en `.rels`-del: id → (mål, extern).
fn parse_rels(xml: &str) -> HashMap<String, (String, String, bool)> {
    let mut out = HashMap::new();
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if local(e.name().as_ref()) == b"Relationship" => {
                let id = attr(&e, b"Id").unwrap_or_default();
                let target = attr(&e, b"Target").unwrap_or_default();
                let kind = attr(&e, b"Type").unwrap_or_default();
                let external = attr(&e, b"TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
                out.insert(id, (target, kind, external));
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn resolve_target(base_dir: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_owned();
    }
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Teckenegenskaper från Word, så som de står i XML:en. `None` = inte angivet (ärvs).
#[derive(Default, Clone, PartialEq, Debug)]
struct RProps {
    font: Option<String>,
    /// Punkter.
    size: Option<f32>,
    color: Option<[u8; 3]>,
    /// `w:highlight`; `Some(None)` är `none` (slår av en ärvd markering).
    highlight: Option<Option<[u8; 3]>>,
    /// `w:shd w:fill`.
    shading: Option<[u8; 3]>,
}

impl RProps {
    /// `inner` ovanpå `self` (det mer specifika vinner).
    fn over(&self, inner: &RProps) -> RProps {
        RProps {
            font: inner.font.clone().or_else(|| self.font.clone()),
            size: inner.size.or(self.size),
            color: inner.color.or(self.color),
            highlight: inner.highlight.or(self.highlight),
            shading: inner.shading.or(self.shading),
        }
    }

    /// Bakgrundsfärgen: markering ligger ovanpå skuggning, som i Word.
    fn background(&self) -> Option<[u8; 3]> {
        match self.highlight {
            Some(h) => h.or(self.shading),
            None => self.shading,
        }
    }
}

/// Temats typsnitt (`word/theme/theme1.xml`).
#[derive(Default, Clone)]
struct Theme {
    major: Option<String>,
    minor: Option<String>,
}

impl Theme {
    /// `minorHAnsi`, `majorAscii` … → typsnittets namn.
    fn font(&self, theme_name: &str) -> Option<String> {
        let n = theme_name.to_ascii_lowercase();
        if n.starts_with("major") {
            self.major.clone()
        } else if n.starts_with("minor") {
            self.minor.clone()
        } else {
            None
        }
    }
}

fn parse_theme(xml: &str) -> Theme {
    let mut t = Theme::default();
    let mut r = Reader::from_str(xml);
    // 0 = utanför, 1 = majorFont, 2 = minorFont
    let mut cur = 0u8;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match local(e.name().as_ref()) {
                b"majorFont" => cur = 1,
                b"minorFont" => cur = 2,
                b"latin" => theme_latin(&mut t, cur, &e),
                _ => {}
            },
            Ok(Event::Empty(e)) if local(e.name().as_ref()) == b"latin" => theme_latin(&mut t, cur, &e),
            Ok(Event::End(e)) if matches!(local(e.name().as_ref()), b"majorFont" | b"minorFont") => cur = 0,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    t
}

fn theme_latin(t: &mut Theme, cur: u8, e: &BytesStart<'_>) {
    let face = attr(e, b"typeface").filter(|f| !f.trim().is_empty());
    match cur {
        1 => t.major = face,
        2 => t.minor = face,
        _ => {}
    }
}

/// Läser ett element i en `w:rPr` (typsnitt, storlek, färg, markering, skuggning) in i `rp`.
fn apply_rpr_elem(rp: &mut RProps, name: &[u8], e: &BytesStart<'_>, theme: &Theme) {
    match name {
        b"rFonts" => {
            let themed = attr(e, b"asciiTheme").or_else(|| attr(e, b"hAnsiTheme")).and_then(|t| theme.font(&t));
            let named = || attr(e, b"ascii").or_else(|| attr(e, b"hAnsi")).filter(|f| !f.trim().is_empty());
            if let Some(f) = themed.or_else(named) {
                rp.font = Some(f);
            }
        }
        b"color" => {
            if let Some(c) = attr(e, b"val").filter(|v| !v.eq_ignore_ascii_case("auto")).and_then(|v| style::parse_color(&v)) {
                rp.color = Some(c);
            }
        }
        b"sz" => {
            if let Some(half) = attr(e, b"val").and_then(|v| v.parse::<f32>().ok()).filter(|h| *h > 0.0) {
                rp.size = Some(half / 2.0);
            }
        }
        b"highlight" => match attr(e, b"val").as_deref() {
            Some("none") => rp.highlight = Some(None),
            Some(n) => {
                if let Some(c) = style::highlight_rgb(n) {
                    rp.highlight = Some(Some(c));
                }
            }
            None => {}
        },
        b"shd" => {
            if let Some(c) = attr(e, b"fill").filter(|v| !v.eq_ignore_ascii_case("auto")).and_then(|v| style::parse_color(&v)) {
                rp.shading = Some(c);
            }
        }
        _ => {}
    }
}

#[derive(Default, Clone)]
struct StyleInfo {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    /// Lista som formatmallen själv har (t.ex. Words "Punktlista").
    num: Option<(String, u8)>,
    /// Teckenegenskaper (`w:rPr`) i formatmallen.
    rpr: RProps,
    /// Justering (`w:jc`) i formatmallen.
    jc: Option<Align>,
}

/// Formatmallarna i `styles.xml`: stycke- och teckenmallar plus dokumentets standardvärden.
#[derive(Default)]
struct Styles {
    map: HashMap<String, StyleInfo>,
    /// `w:docDefaults/w:rPrDefault`.
    defaults: RProps,
    /// Standardstyckemallen (`w:default="1"`, oftast `Normal`).
    default_para: String,
}

impl Styles {
    /// Egenskaperna i en formatmall och de den bygger på (utan docDefaults).
    fn rchain(&self, id: &str) -> RProps {
        let mut chain = Vec::new();
        let mut cur = Some(id);
        while let Some(sid) = cur {
            if chain.len() >= 10 {
                break;
            }
            let Some(info) = self.map.get(sid) else { break };
            chain.push(&info.rpr);
            cur = info.based_on.as_deref();
        }
        chain.into_iter().rev().fold(RProps::default(), |acc, r| acc.over(r))
    }

    fn para_id<'a>(&'a self, style: Option<&'a str>) -> &'a str {
        style.unwrap_or(if self.default_para.is_empty() { "Normal" } else { &self.default_para })
    }

    /// docDefaults + stycketypens formatmall (eller standardmallen).
    fn para_props(&self, style: Option<&str>) -> RProps {
        self.defaults.over(&self.rchain(self.para_id(style)))
    }

    /// Justering som formatmallen (eller den den bygger på) anger.
    fn jc(&self, style: Option<&str>) -> Option<Align> {
        let mut cur = Some(self.para_id(style));
        for _ in 0..10 {
            let info = self.map.get(cur?)?;
            if info.jc.is_some() {
                return info.jc;
            }
            cur = info.based_on.as_deref();
        }
        None
    }

    /// Dokumentets grundformat: typsnitt och storlek hos Normal (docDefaults + Normal).
    fn base(&self) -> DocDefaults {
        let p = self.para_props(None);
        DocDefaults { font_family: p.font, font_size: p.size, ..Default::default() }
    }
}

fn parse_styles(xml: &str, theme: &Theme) -> Styles {
    let mut out = Styles::default();
    let mut r = Reader::from_str(xml);
    let mut cur: Option<(String, StyleInfo)> = None;
    let mut cur_default_para = false;
    let mut in_defaults = false;
    let mut in_rpr = false;
    let mut in_ppr = false;
    loop {
        let ev = r.read_event();
        let (e, empty) = match &ev {
            Ok(Event::Start(e)) => (e, false),
            Ok(Event::Empty(e)) => (e, true),
            Ok(Event::End(e)) => {
                match local(e.name().as_ref()) {
                    b"style" => {
                        if let Some((id, info)) = cur.take() {
                            if cur_default_para {
                                out.default_para = id.clone();
                            }
                            out.map.insert(id, info);
                        }
                    }
                    b"rPrDefault" => in_defaults = false,
                    b"pPr" => in_ppr = false,
                    b"rPr" => in_rpr = false,
                    _ => {}
                }
                continue;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => continue,
        };
        let qname = e.name();
        let name = local(qname.as_ref());
        if name == b"style" && !empty {
            let ty = attr(e, b"type");
            let is_para = ty.as_deref().is_none_or(|t| t == "paragraph");
            let keep = is_para || ty.as_deref() == Some("character");
            cur_default_para = is_para && attr(e, b"default").is_some_and(|d| d == "1" || d == "true");
            cur = keep.then(|| (attr(e, b"styleId").unwrap_or_default(), StyleInfo::default()));
            continue;
        }
        match name {
            b"rPrDefault" => in_defaults = !empty,
            b"pPr" => in_ppr = !empty,
            b"rPr" if !in_ppr => in_rpr = !empty,
            _ if in_rpr => {
                if let Some((_, info)) = cur.as_mut() {
                    apply_rpr_elem(&mut info.rpr, name, e, theme);
                } else if in_defaults {
                    apply_rpr_elem(&mut out.defaults, name, e, theme);
                }
            }
            _ => {
                if let Some((_, info)) = cur.as_mut() {
                    match name {
                        b"name" => info.name = attr(e, b"val").unwrap_or_default().to_lowercase(),
                        b"basedOn" => info.based_on = attr(e, b"val"),
                        b"jc" if in_ppr => info.jc = attr(e, b"val").map(|v| Align::from_jc(&v)),
                        b"outlineLvl" => {
                            info.outline = attr(e, b"val").and_then(|v| v.parse::<u8>().ok()).filter(|&v| v < 9)
                        }
                        b"numId" => {
                            let id = attr(e, b"val").unwrap_or_default();
                            let lvl = info.num.as_ref().map_or(0, |n| n.1);
                            info.num = (id != "0").then_some((id, lvl));
                        }
                        b"ilvl" => {
                            let lvl = attr(e, b"val").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0).min(8);
                            let id = info.num.as_ref().map(|n| n.0.clone()).unwrap_or_default();
                            info.num = Some((id, lvl));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ParaKind {
    Normal,
    Heading(u8),
    Quote,
    Code,
}

fn heading_from_name(s: &str) -> Option<u8> {
    let s = s.to_lowercase().replace([' ', '_', '-'], "");
    if s == "title" || s == "rubrik" || s == "titel" {
        return Some(1);
    }
    for prefix in ["heading", "rubrik", "überschrift", "titre", "kop", "otsikko", "overskrift"] {
        if let Some(n) = s.strip_prefix(prefix) {
            if let Ok(n) = n.parse::<u8>() {
                return Some(n.clamp(1, 6));
            }
        }
    }
    None
}

fn list_for_style(styles: &HashMap<String, StyleInfo>, id: &str) -> Option<(String, u8)> {
    let mut cur = Some(id.to_owned());
    for _ in 0..10 {
        let info = styles.get(cur.as_deref()?)?;
        if let Some((num, lvl)) = &info.num {
            if !num.is_empty() {
                return Some((num.clone(), *lvl));
            }
        }
        cur = info.based_on.clone();
    }
    None
}

fn kind_for_style(styles: &HashMap<String, StyleInfo>, id: &str) -> ParaKind {
    let mut cur = Some(id.to_owned());
    for _ in 0..10 {
        let Some(sid) = cur else { break };
        if let Some(h) = heading_from_name(&sid) {
            return ParaKind::Heading(h);
        }
        let Some(info) = styles.get(&sid) else { break };
        if let Some(h) = heading_from_name(&info.name) {
            return ParaKind::Heading(h);
        }
        if let Some(lvl) = info.outline {
            return ParaKind::Heading((lvl + 1).min(6));
        }
        let n = info.name.as_str();
        if n.contains("quote") || n.contains("citat") {
            return ParaKind::Quote;
        }
        if n.contains("code") || n.contains("kod") || n == "html preformatted" || n == "plain text" {
            return ParaKind::Code;
        }
        cur = info.based_on.clone();
    }
    ParaKind::Normal
}

/// Listdefinitioner per numId.
#[derive(Default)]
struct Numbering {
    /// För varje nivå: är listan numrerad (inte punktlista)?
    ordered: HashMap<String, Vec<bool>>,
    /// För varje nivå: första numret (`w:start`, ersatt av `w:startOverride`).
    starts: HashMap<String, Vec<u64>>,
}

/// numId → format och startnummer per nivå.
fn parse_numbering(xml: &str) -> Numbering {
    let mut abstract_fmt: HashMap<String, Vec<bool>> = HashMap::new();
    let mut abstract_start: HashMap<String, Vec<u64>> = HashMap::new();
    let mut overrides: HashMap<(String, usize), u64> = HashMap::new();
    let mut ovr_lvl: usize = 0;
    let mut num_to_abs: HashMap<String, String> = HashMap::new();
    let mut r = Reader::from_str(xml);
    let mut cur_abs: Option<String> = None;
    let mut cur_lvl: usize = 0;
    let mut cur_num: Option<String> = None;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match local(e.name().as_ref()) {
                b"abstractNum" => cur_abs = attr(&e, b"abstractNumId"),
                b"lvl" => cur_lvl = attr(&e, b"ilvl").and_then(|v| v.parse().ok()).unwrap_or(0).min(8),
                b"numFmt" => {
                    if let Some(a) = &cur_abs {
                        let ordered = !matches!(attr(&e, b"val").as_deref(), Some("bullet" | "none") | None);
                        let v = abstract_fmt.entry(a.clone()).or_insert_with(|| vec![false; 9]);
                        v[cur_lvl] = ordered;
                    }
                }
                b"start" => {
                    if let (Some(a), Some(v)) = (&cur_abs, attr(&e, b"val").and_then(|v| v.parse().ok())) {
                        abstract_start.entry(a.clone()).or_insert_with(|| vec![1; 9])[cur_lvl] = v;
                    }
                }
                b"lvlOverride" => ovr_lvl = attr(&e, b"ilvl").and_then(|v| v.parse().ok()).unwrap_or(0).min(8),
                b"startOverride" => {
                    if let (Some(n), Some(v)) = (&cur_num, attr(&e, b"val").and_then(|v| v.parse().ok())) {
                        overrides.insert((n.clone(), ovr_lvl), v);
                    }
                }
                b"num" => cur_num = attr(&e, b"numId"),
                b"abstractNumId" => {
                    if let (Some(n), Some(a)) = (&cur_num, attr(&e, b"val")) {
                        num_to_abs.insert(n.clone(), a);
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match local(e.name().as_ref()) {
                b"abstractNum" => cur_abs = None,
                b"num" => cur_num = None,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let mut out = Numbering::default();
    for (n, a) in num_to_abs {
        let Some(fmt) = abstract_fmt.get(&a) else { continue };
        let mut starts = abstract_start.get(&a).cloned().unwrap_or_else(|| vec![1; 9]);
        for (lvl, st) in starts.iter_mut().enumerate() {
            if let Some(v) = overrides.get(&(n.clone(), lvl)) {
                *st = *v;
            }
        }
        out.ordered.insert(n.clone(), fmt.clone());
        out.starts.insert(n, starts);
    }
    out
}

#[derive(Default, Clone, PartialEq)]
struct Fmt {
    bold: bool,
    italic: bool,
    strike: bool,
    /// Direkt understrykning (`w:u`), skrivs som `<u>…</u>`.
    underline: bool,
    code: bool,
    /// Typsnitt, storlek och färger som avviker från grundformatet (`<span style="…">`).
    span: SpanStyle,
}

#[derive(Clone)]
struct Run {
    text: String,
    fmt: Fmt,
    link: Option<String>,
    /// Färdig Markdown (t.ex. en bild) som inte ska skyddas med `\`.
    raw: bool,
}

#[derive(Clone)]
struct Para {
    style: Option<String>,
    kind: ParaKind,
    list: Option<(String, u8)>,
    runs: Vec<Run>,
    /// Justering (`w:jc`), ärvd från formatmallen om stycket inte anger någon.
    align: Align,
    /// Justering som stycket anger själv.
    direct_align: Option<Align>,
}

impl Para {
    fn new() -> Para {
        Para { style: None, kind: ParaKind::Normal, list: None, runs: Vec::new(), align: Align::Left, direct_align: None }
    }
}

enum Block {
    Para(Para),
    Table(Vec<Vec<String>>),
}

#[derive(Default)]
struct Dropped {
    images: bool,
    linked: bool,
    notes: bool,
    comments: bool,
    textboxes: bool,
    tracked: bool,
    equations: bool,
    headers: bool,
}

impl Dropped {
    fn list(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.images {
            v.push("diagram och former");
        }
        if self.linked {
            v.push("länkade bilder");
        }
        if self.textboxes {
            v.push("textrutor och former");
        }
        if self.notes {
            v.push("fotnoter");
        }
        if self.comments {
            v.push("kommentarer");
        }
        if self.equations {
            v.push("formler");
        }
        if self.tracked {
            v.push("spårade ändringar");
        }
        if self.headers {
            v.push("sidhuvud och sidfot");
        }
        v
    }
}

pub fn read(bytes: &[u8]) -> Result<Loaded, String> {
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Err(
            "Dokumentet är lösenordsskyddat eller sparat i det gamla .doc-formatet. Öppna det i Word och spara som .docx utan lösenord."
                .into(),
        );
    }
    if !bytes.starts_with(b"PK") {
        return Err("Filen är inte ett Word-dokument (.docx).".into());
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err("Word-dokumentet är för stort för att öppnas.".into());
    }
    check_entry_count(bytes)?;
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("Word-dokumentet är skadat: {e}"))?;
    let mut pkg = Package { zip, total: 0 };

    // Huvuddokumentet pekas ut av paketets relationer.
    let main = pkg
        .part("_rels/.rels")?
        .map(|x| parse_rels(&x))
        .and_then(|rels| {
            rels.into_values()
                .find(|(_, kind, _)| kind.ends_with("/officeDocument"))
                .map(|(t, _, _)| resolve_target("", &t))
        })
        .unwrap_or_else(|| "word/document.xml".to_owned());
    let (dir, file) = main.rsplit_once('/').unwrap_or(("", main.as_str()));
    let Some(doc_xml) = pkg.part(&main)? else {
        return Err("Word-dokumentet saknar sitt huvudinnehåll.".into());
    };

    let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
    let rels = pkg.part(&rels_name)?.map(|x| parse_rels(&x)).unwrap_or_default();
    let find_part = |suffix: &str| {
        rels.values()
            .find(|(_, kind, ext)| !ext && kind.ends_with(suffix))
            .map(|(t, _, _)| resolve_target(dir, t))
    };
    // Temats typsnitt behövs för `w:asciiTheme` i formatmallarna och texten.
    let theme_part = find_part("/theme").unwrap_or_else(|| "word/theme/theme1.xml".to_owned());
    let theme = pkg.part(&theme_part)?.map(|x| parse_theme(&x)).unwrap_or_default();
    let styles = match find_part("/styles") {
        Some(p) => pkg.part(&p)?.map(|x| parse_styles(&x, &theme)).unwrap_or_default(),
        None => Styles::default(),
    };
    let numbering = match find_part("/numbering") {
        Some(p) => pkg.part(&p)?.map(|x| parse_numbering(&x)).unwrap_or_default(),
        None => Numbering::default(),
    };
    let mut dropped = Dropped {
        headers: rels.values().any(|(_, k, _)| k.ends_with("/header") || k.ends_with("/footer")),
        ..Default::default()
    };
    // Sidhuvuden räknas bara om de faktiskt innehåller text.
    if dropped.headers {
        let mut any_text = false;
        let parts: Vec<String> = rels
            .values()
            .filter(|(_, k, e)| !e && (k.ends_with("/header") || k.ends_with("/footer")))
            .map(|(t, _, _)| resolve_target(dir, t))
            .collect();
        for p in parts {
            if let Some(x) = pkg.part(&p)? {
                if x.contains("<w:t") && x.split("<w:t").skip(1).any(|s| s.split('<').next().is_some_and(|t| t.contains(|c: char| c.is_alphanumeric()))) {
                    any_text = true;
                }
            }
        }
        dropped.headers = any_text;
    }

    let mut found: Vec<FoundImage> = Vec::new();
    let blocks = parse_body(&doc_xml, &styles, &theme, &numbering, &rels, &mut dropped, &mut found, dir)?;

    // Packa upp bilderna som dokumentet faktiskt använder.
    let mut images: Vec<(String, Image)> = Vec::new();
    let mut by_target: HashMap<String, String> = HashMap::new();
    let mut markdown = to_markdown(&blocks, &numbering);
    // Grundformatet (typsnitt och storlek hos Normal) står först när det avviker från Calibri 11 pt.
    let comment = styles.base().to_comment();
    if !comment.is_empty() {
        markdown = format!("{comment}\n\n{markdown}");
    }
    for f in found {
        let name = if let Some(n) = by_target.get(&f.target) {
            n.clone()
        } else {
            let Some(bytes) = pkg.part_bytes(&f.target)? else {
                markdown = markdown.replace(&f.placeholder, "");
                dropped.images = true;
                continue;
            };
            if !is_supported_image(&bytes) {
                markdown = markdown.replace(&f.placeholder, "");
                dropped.images = true;
                continue;
            }
            let base = f.target.rsplit('/').next().unwrap_or("bild").to_owned();
            let mut name = base.clone();
            let mut n = 2;
            while images.iter().any(|(m, _)| *m == name) {
                name = format!("{n}-{base}");
                n += 1;
            }
            images.push((name.clone(), Image { bytes, extent: f.extent }));
            by_target.insert(f.target.clone(), name.clone());
            name
        };
        let alt = escape_alt(&f.alt);
        markdown = markdown.replace(&f.placeholder, &format!("![{alt}]({IMAGE_SCHEME}{})", name.replace(' ', "%20")));
    }
    Ok(Loaded { markdown, images, dropped: dropped.list() })
}

/// En bild som hittats i texten; platshållaren byts mot `![…](bild:…)` när bilden lästs.
struct FoundImage {
    placeholder: String,
    target: String,
    alt: String,
    extent: Option<(i64, i64)>,
}

#[derive(Default)]
struct Drawing {
    depth: usize,
    blip: Option<String>,
    linked: bool,
    alt: String,
    title: String,
    extent: Option<(i64, i64)>,
}

fn escape_alt(s: &str) -> String {
    s.replace(['\n', '\r'], " ").replace('\\', "\\\\").replace('[', "\\[").replace(']', "\\]")
}

pub fn is_supported_image(bytes: &[u8]) -> bool {
    image_ext(bytes).is_some()
}

/// Filändelse för de bildformat Word kan visa, utifrån innehållet.
pub fn image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

/// Det som skiljer en körning från dokumentets grundformat och från det som styckets slag redan ger.
/// Rubriker och citat jämförs med sin egen formatmall (Kladd ritar dem med egna formatmallar),
/// vanliga stycken med grundformatet (typsnitt och storlek hos Normal, ingen färg).
fn run_span(
    styles: &Styles,
    base: &DocDefaults,
    para: Option<&Para>,
    direct: &RProps,
    rstyle: Option<&str>,
    linked: bool,
    code: bool,
) -> SpanStyle {
    let style = para.and_then(|p| p.style.as_deref());
    let para_eff = styles.para_props(style);
    let mut eff = para_eff.clone();
    if let Some(rs) = rstyle {
        eff = eff.over(&styles.rchain(rs));
    }
    let eff = eff.over(direct);
    let mut reference = match para.map_or(ParaKind::Normal, |p| p.kind) {
        ParaKind::Heading(_) | ParaKind::Quote => para_eff,
        _ => RProps { font: Some(base.font().to_owned()), size: Some(base.size()), ..Default::default() },
    };
    // Länkar har Words länkfärg; den ska inte bli en span.
    if linked || rstyle == Some("Hyperlink") {
        reference.color = styles.rchain("Hyperlink").color;
    }
    if code {
        // Kod får sitt typsnitt och sin bakgrund av teckenmallen.
        if let Some(rs) = rstyle {
            reference = reference.over(&styles.rchain(rs));
        }
    }
    let ref_font = reference.font.as_deref().unwrap_or(style::DEFAULT_FONT);
    let ref_size = reference.size.unwrap_or(style::DEFAULT_SIZE);
    SpanStyle {
        font_family: eff.font.clone().filter(|f| !code && !f.eq_ignore_ascii_case(ref_font)),
        font_size: eff.size.filter(|s| (s - ref_size).abs() > 0.01),
        color: eff.color.filter(|c| Some(*c) != reference.color),
        background: eff.background().filter(|c| Some(*c) != reference.background()),
    }
}

fn parse_body(
    xml: &str,
    styles: &Styles,
    theme: &Theme,
    numbering: &Numbering,
    rels: &HashMap<String, (String, String, bool)>,
    dropped: &mut Dropped,
    found: &mut Vec<FoundImage>,
    dir: &str,
) -> Result<Vec<Block>, String> {
    let _ = numbering;
    let mut drawing: Option<Drawing> = None;
    let mut r = Reader::from_str(xml);
    let mut blocks: Vec<Block> = Vec::new();

    let mut para: Option<Para> = None;
    let mut fmt = Fmt::default();
    // Körningens egna egenskaper (`w:rPr`) och teckenmall (`w:rStyle`).
    let mut direct = RProps::default();
    let mut run_rstyle: Option<String> = None;
    let base = styles.base();
    let mut in_rpr = false;
    let mut in_t = false;
    let mut link: Vec<Option<String>> = Vec::new();
    // Hoppa över innehåll i dessa element (djupräknare).
    let mut skip = 0usize;
    // Fältkoder: mellan begin och separate visas inget.
    let mut field_depth: Vec<bool> = Vec::new();
    // Tabeller: stack av (rader, aktuell rad, aktuell cell)
    let mut tables: Vec<(Vec<Vec<String>>, Vec<String>, Option<String>)> = Vec::new();
    let mut in_ppr = false;
    let mut alt_depth: Vec<bool> = Vec::new(); // AlternateContent: har Choice använts?

    let push_text = |para: &mut Option<Para>, text: &str, fmt: &Fmt, link: &Option<String>| {
        let p = para.get_or_insert_with(Para::new);
        if let Some(last) = p.runs.last_mut() {
            if !last.raw && last.fmt == *fmt && last.link == *link {
                last.text.push_str(text);
                return;
            }
        }
        p.runs.push(Run { text: text.to_owned(), fmt: fmt.clone(), link: link.clone(), raw: false });
    };
    let hidden = |field_depth: &Vec<bool>| field_depth.iter().any(|&in_instr| in_instr);

    loop {
        let ev = r.read_event().map_err(|e| format!("Word-dokumentet innehåller trasig XML: {e}"))?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let empty = matches!(ev, Event::Empty(_));
                let name = local(e.name().as_ref()).to_vec();
                if skip > 0 {
                    if !empty {
                        skip += 1;
                    }
                    continue;
                }
                // Inuti en bild: leta bara efter bilddata, beskrivning och storlek.
                if let Some(d) = drawing.as_mut() {
                    if !empty {
                        d.depth += 1;
                    }
                    match name.as_slice() {
                        b"blip" | b"imagedata" => {
                            if let Some(id) = attr(e, b"embed").or_else(|| attr(e, b"id")) {
                                if let Some((t, _, ext)) = rels.get(&id) {
                                    if *ext {
                                        d.linked = true;
                                    } else if d.blip.is_none() {
                                        d.blip = Some(t.clone());
                                    }
                                }
                            } else if attr(e, b"link").is_some() {
                                d.linked = true;
                            }
                        }
                        b"docPr" => {
                            d.alt = attr(e, b"descr").unwrap_or_default();
                            d.title = attr(e, b"title").unwrap_or_default();
                        }
                        b"extent" if d.extent.is_none() => {
                            let cx = attr(e, b"cx").and_then(|v| v.parse::<i64>().ok());
                            let cy = attr(e, b"cy").and_then(|v| v.parse::<i64>().ok());
                            if let (Some(cx), Some(cy)) = (cx, cy) {
                                if cx > 0 && cy > 0 {
                                    d.extent = Some((cx, cy));
                                }
                            }
                        }
                        b"txbxContent" => {
                            dropped.textboxes = true;
                            if !empty {
                                d.depth -= 1;
                                skip = 1;
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if in_rpr {
                    apply_rpr_elem(&mut direct, &name, e, theme);
                }
                match name.as_slice() {
                    // Bilder: samla ihop till ett `![…](bild:…)` när elementet tar slut.
                    b"drawing" | b"pict" | b"object" => {
                        if !empty {
                            drawing = Some(Drawing { depth: 1, ..Default::default() });
                        } else {
                            dropped.images = true;
                        }
                    }
                    b"txbxContent" => {
                        dropped.textboxes = true;
                        if !empty {
                            skip = 1;
                        }
                    }
                    b"oMath" | b"oMathPara" => {
                        dropped.equations = true;
                        if !empty {
                            skip = 1;
                        }
                    }
                    b"del" | b"moveFrom" => {
                        dropped.tracked = true;
                        if !empty {
                            skip = 1;
                        }
                    }
                    b"ins" | b"moveTo" => dropped.tracked = true,
                    b"footnoteReference" | b"endnoteReference" => dropped.notes = true,
                    b"commentReference" | b"commentRangeStart" => dropped.comments = true,
                    b"sdtPr" | b"sdtEndPr" | b"instrText" | b"delText" | b"pPrChange" | b"rPrChange"
                    | b"sectPr" | b"tblPr" | b"tblGrid" | b"trPr" | b"tcPr" | b"fldData" => {
                        if !empty {
                            skip = 1;
                        }
                    }
                    b"AlternateContent" => alt_depth.push(false),
                    b"Choice" => {
                        // Första Choice används; resten och Fallback hoppas över.
                        if let Some(used) = alt_depth.last_mut() {
                            if *used && !empty {
                                skip = 1;
                            } else {
                                *used = true;
                            }
                        }
                    }
                    b"Fallback" => {
                        if alt_depth.last().copied().unwrap_or(false) && !empty {
                            skip = 1;
                        }
                    }
                    b"p" => {
                        para = Some(Para::new());
                    }
                    b"pPr" => in_ppr = !empty,
                    b"pStyle" if in_ppr => {
                        if let (Some(p), Some(id)) = (para.as_mut(), attr(e, b"val")) {
                            p.kind = kind_for_style(&styles.map, &id);
                            p.style = Some(id);
                        }
                    }
                    b"outlineLvl" if in_ppr => {
                        if let (Some(p), Some(l)) = (para.as_mut(), attr(e, b"val").and_then(|v| v.parse::<u8>().ok())) {
                            if l < 9 {
                                p.kind = ParaKind::Heading((l + 1).min(6));
                            }
                        }
                    }
                    b"ilvl" if in_ppr => {
                        if let Some(p) = para.as_mut() {
                            let lvl = attr(e, b"val").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0).min(8);
                            let id = p.list.as_ref().map(|l| l.0.clone()).unwrap_or_default();
                            p.list = Some((id, lvl));
                        }
                    }
                    b"numId" if in_ppr => {
                        if let Some(p) = para.as_mut() {
                            let id = attr(e, b"val").unwrap_or_default();
                            let lvl = p.list.as_ref().map(|l| l.1).unwrap_or(0);
                            // numId 0 betyder "ingen lista".
                            p.list = (id != "0").then_some((id, lvl));
                        }
                    }
                    b"jc" if in_ppr => {
                        if let Some(p) = para.as_mut() {
                            p.direct_align = attr(e, b"val").map(|v| Align::from_jc(&v));
                        }
                    }
                    b"rPr" if !in_ppr => in_rpr = !empty,
                    b"r" => {
                        fmt = Fmt::default();
                        direct = RProps::default();
                        run_rstyle = None;
                        let linked = link.last().is_some_and(Option::is_some);
                        fmt.span = run_span(styles, &base, para.as_ref(), &direct, None, linked, false);
                    }
                    b"b" | b"bCs" if in_rpr => fmt.bold = on_off(e),
                    b"i" | b"iCs" if in_rpr => fmt.italic = on_off(e),
                    b"strike" | b"dstrike" if in_rpr => fmt.strike = on_off(e),
                    // Saknas `w:val` är det enkel understrykning; "none" slår av den.
                    b"u" if in_rpr => fmt.underline = on_off(e),
                    b"rStyle" if in_rpr => {
                        run_rstyle = attr(e, b"val");
                        let s = attr(e, b"val").unwrap_or_default().to_lowercase();
                        if s.contains("code") || s.contains("kod") {
                            fmt.code = true;
                        }
                        if s == "strong" || s.contains("fet") {
                            fmt.bold = true;
                        }
                        if s == "emphasis" || s.contains("betoning") {
                            fmt.italic = true;
                        }
                    }
                    b"rFonts" if in_rpr => {
                        let f = attr(e, b"ascii").unwrap_or_default().to_lowercase();
                        if ["courier", "consolas", "mono", "menlo", "monaco"].iter().any(|m| f.contains(m)) {
                            fmt.code = true;
                        }
                    }
                    b"t" => in_t = !empty,
                    b"tab" if !in_ppr && para.is_some() && !hidden(&field_depth) => {
                        push_text(&mut para, "\t", &fmt, &link.last().cloned().flatten())
                    }
                    b"br" | b"cr" if !hidden(&field_depth) => {
                        push_text(&mut para, "\n", &Fmt::default(), &None);
                    }
                    b"noBreakHyphen" if !hidden(&field_depth) => {
                        push_text(&mut para, "\u{2011}", &fmt, &link.last().cloned().flatten())
                    }
                    b"sym" if !hidden(&field_depth) => {
                        let c = attr(e, b"char")
                            .and_then(|h| u32::from_str_radix(&h, 16).ok())
                            .and_then(|mut v| {
                                // Symboltecken ligger ofta i F000-området.
                                if (0xF000..=0xF0FF).contains(&v) {
                                    v -= 0xF000;
                                }
                                char::from_u32(v)
                            })
                            .filter(|c| !c.is_control());
                        if let Some(c) = c {
                            push_text(&mut para, &c.to_string(), &fmt, &link.last().cloned().flatten());
                        }
                    }
                    b"fldChar" => match attr(e, b"fldCharType").as_deref() {
                        Some("begin") => field_depth.push(true),
                        Some("separate") => {
                            if let Some(l) = field_depth.last_mut() {
                                *l = false;
                            }
                        }
                        Some("end") => {
                            field_depth.pop();
                        }
                        _ => {}
                    },
                    b"hyperlink" if !empty => {
                        let url = attr(e, b"id")
                            .and_then(|id| rels.get(&id))
                            .filter(|(_, _, ext)| *ext)
                            .map(|(t, _, _)| t.clone());
                        link.push(url);
                    }
                    b"tbl" if !empty => {
                        if let Some(p) = para.take() {
                            blocks.push(Block::Para(p));
                        }
                        tables.push((Vec::new(), Vec::new(), None));
                    }
                    b"tr" if !empty => {
                        if let Some(t) = tables.last_mut() {
                            t.1 = Vec::new();
                        }
                    }
                    b"tc" if !empty => {
                        if let Some(t) = tables.last_mut() {
                            t.2 = Some(String::new());
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                if let Some(d) = drawing.as_mut() {
                    d.depth -= 1;
                    if d.depth > 0 {
                        continue;
                    }
                    let d = drawing.take().unwrap_or_default();
                    match d.blip {
                        Some(target) if !hidden(&field_depth) => {
                            let placeholder = format!("\u{E000}kladd-bild-{}\u{E001}", found.len());
                            let alt = if d.alt.trim().is_empty() { d.title } else { d.alt };
                            let p = para.get_or_insert_with(Para::new);
                            p.runs.push(Run { text: placeholder.clone(), fmt: Fmt::default(), link: None, raw: true });
                            found.push(FoundImage {
                                placeholder,
                                target: resolve_target(dir, &target),
                                alt: alt.trim().to_owned(),
                                extent: d.extent,
                            });
                        }
                        Some(_) => {}
                        None if d.linked => dropped.linked = true,
                        None => dropped.images = true,
                    }
                    continue;
                }
                match local(e.name().as_ref()) {
                    b"pPr" => in_ppr = false,
                    b"rPr" => {
                        if in_rpr {
                            in_rpr = false;
                            let linked = link.last().is_some_and(Option::is_some);
                            fmt.span = run_span(styles, &base, para.as_ref(), &direct, run_rstyle.as_deref(), linked, fmt.code);
                        }
                    }
                    b"t" => in_t = false,
                    b"r" => fmt = Fmt::default(),
                    b"hyperlink" => {
                        link.pop();
                    }
                    b"AlternateContent" => {
                        alt_depth.pop();
                    }
                    b"p" => {
                        if let Some(mut p) = para.take() {
                            p.align = p.direct_align.or_else(|| styles.jc(p.style.as_deref())).unwrap_or(Align::Left);
                            if p.list.is_none() {
                                p.list = p.style.as_deref().and_then(|s| list_for_style(&styles.map, s));
                            } else if p.list.as_ref().is_some_and(|l| l.0.is_empty()) {
                                // Bara nivå angiven: numId kommer från formatmallen.
                                let lvl = p.list.as_ref().map_or(0, |l| l.1);
                                p.list = p.style.as_deref().and_then(|s| list_for_style(&styles.map, s)).map(|(n, _)| (n, lvl));
                            }
                            if let Some(t) = tables.last_mut() {
                                // Stycke i en tabellcell: lägg texten i cellen.
                                if let Some(cell) = t.2.as_mut() {
                                    let txt: String = p.runs.iter().map(|r| inline_md(r)).collect();
                                    let txt = txt.trim();
                                    if !txt.is_empty() {
                                        if !cell.is_empty() {
                                            cell.push_str("<br>");
                                        }
                                        cell.push_str(&txt.replace('\n', "<br>").replace('|', "\\|"));
                                    }
                                }
                            } else {
                                blocks.push(Block::Para(p));
                            }
                        }
                    }
                    b"tc" => {
                        if let Some(t) = tables.last_mut() {
                            let cell = t.2.take().unwrap_or_default();
                            t.1.push(cell);
                        }
                    }
                    b"tr" => {
                        if let Some(t) = tables.last_mut() {
                            let row = std::mem::take(&mut t.1);
                            t.0.push(row);
                        }
                    }
                    b"tbl" => {
                        if let Some((rows, _, _)) = tables.pop() {
                            if let Some(outer) = tables.last_mut() {
                                // Nästlad tabell: platta till i den yttre cellen.
                                let flat: Vec<String> = rows.iter().map(|r| r.join(" ")).collect();
                                if let Some(cell) = outer.2.as_mut() {
                                    if !cell.is_empty() {
                                        cell.push_str("<br>");
                                    }
                                    cell.push_str(&flat.join("<br>"));
                                }
                            } else if !rows.is_empty() {
                                blocks.push(Block::Table(rows));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) => {
                if skip == 0 && in_t && !hidden(&field_depth) {
                    let t = text_of(&ev).replace(['\n', '\r'], " ");
                    if !t.is_empty() {
                        push_text(&mut para, &t, &fmt, &link.last().cloned().flatten());
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(p) = para.take() {
        blocks.push(Block::Para(p));
    }
    Ok(blocks)
}

/// Skyddar tecken som annars skulle tolkas som Markdown.
pub(crate) fn escape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '~') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Skyddar början av en rad som annars blir en rubrik, lista, citat osv.
pub(crate) fn escape_line_start(line: &str) -> String {
    let trimmed = line.trim_start_matches(' ');
    let indent = &line[..line.len() - trimmed.len()];
    let first = trimmed.chars().next();
    if matches!(first, Some('#' | '>' | '-' | '+' | '|' | '=')) {
        return format!("{indent}\\{trimmed}");
    }
    // "1. " eller "1) " i början blir annars en numrerad lista.
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() && digits.len() <= 9 {
        let rest = &trimmed[digits.len()..];
        if rest.starts_with(". ") || rest.starts_with(") ") || rest == "." || rest == ")" {
            return format!("{indent}{digits}\\{rest}");
        }
    }
    if indent.len() >= 4 {
        // Fyra mellanslag blir annars ett kodblock.
        return format!("&#32;{}", &line[1..]);
    }
    line.to_owned()
}

pub(crate) fn wrap(text: &str, marker: &str) -> String {
    // Markörer får inte ligga mot mellanslag: flytta ut dem.
    let start = text.len() - text.trim_start().len();
    let end = text.trim_end().len();
    if start >= end {
        return text.to_owned();
    }
    format!("{}{marker}{}{marker}{}", &text[..start], &text[start..end], &text[end..])
}

/// Omger texten med en HTML-tagg; mellanslag i kanterna stannar utanför, som i [`wrap`].
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
            // Utanför fetstil/kursiv, så att fet+understruken blir `<u>**x**</u>`.
            s = wrap_tag(&s, "<u>", "</u>");
        }
        if !run.fmt.span.is_empty() {
            // Ytterst: `<span …><u>**x**</u></span>`.
            s = wrap_tag(&s, &run.fmt.span.open_tag(), "</span>");
        }
        if let Some(url) = &run.link {
            s = format!("[{s}]({})", url.replace(' ', "%20").replace(')', "%29"));
        }
        parts.push(s);
    }
    parts.concat()
}

fn para_md(p: &Para) -> String {
    let mut s: String = p.runs.iter().map(inline_md).collect();
    // Ta bort mellanslag i slutet av varje rad (de syns inte och blir "hårda radbrytningar").
    s = s.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    s.trim_matches('\n').to_owned()
}

fn to_markdown(blocks: &[Block], numbering: &Numbering) -> String {
    let mut out = String::new();
    let mut prev_list = false;
    // numId för den yttersta listan: en ny lista på nivå 0 får en tom rad före sig.
    let mut top_num: Option<String> = None;
    // Räknare per (numId, nivå); fortsätter när samma lista återupptas efter ett avbrott.
    let mut counters: HashMap<(String, usize), u64> = HashMap::new();
    // Justeringen hos den `<div>` som är öppen just nu (`Left` = ingen).
    let mut cur_align = Align::Left;
    for b in blocks {
        match b {
            Block::Para(p) => {
                let text = para_md(p);
                if text.trim().is_empty() {
                    continue;
                }
                // Kodblock justeras inte.
                let align = if p.kind == ParaKind::Code { Align::Left } else { p.align };
                let is_list = p.list.is_some() && !matches!(p.kind, ParaKind::Heading(_));
                let same_list = match &p.list {
                    Some((num, 0)) => top_num.as_deref() == Some(num.as_str()),
                    Some(_) => true,
                    None => false,
                };
                if align != cur_align {
                    // Ny div (eller slut på den förra) i stället för en vanlig styckegräns.
                    set_align(&mut out, &mut cur_align, align);
                } else if !out.is_empty() {
                    out.push_str(if is_list && prev_list && same_list { "\n" } else { "\n\n" });
                }
                if let Some((num, 0)) = &p.list {
                    top_num = Some(num.clone());
                } else if !is_list {
                    top_num = None;
                }
                prev_list = is_list;
                match (p.kind, &p.list) {
                    (ParaKind::Heading(n), _) => {
                        let one_line = text.replace('\n', " ");
                        out.push_str(&format!("{} {}", "#".repeat(n as usize), one_line.trim()));
                    }
                    (_, Some((num, lvl))) => {
                        let l = *lvl as usize;
                        let ordered = numbering.ordered.get(num).map(|v| v[l]).unwrap_or(false);
                        let indent = "    ".repeat(l);
                        let marker = if ordered {
                            // Ett objekt nollställer räknarna för djupare nivåer i samma lista.
                            counters.retain(|(n, d), _| n != num || *d <= l);
                            let start = numbering.starts.get(num).map(|v| v[l]).unwrap_or(1);
                            let c = counters.entry((num.clone(), l)).and_modify(|c| *c += 1).or_insert(start);
                            format!("{c}. ")
                        } else {
                            "- ".to_owned()
                        };
                        let cont = format!("{indent}{}", " ".repeat(marker.len()));
                        let mut lines = text.lines();
                        let first = lines.next().unwrap_or("");
                        out.push_str(&format!("{indent}{marker}{}", escape_line_start(first)));
                        for l in lines {
                            out.push('\n');
                            out.push_str(&cont);
                            out.push_str(l);
                        }
                    }
                    (ParaKind::Quote, None) => {
                        let q: Vec<String> = text.lines().map(|l| format!("> {}", escape_line_start(l))).collect();
                        out.push_str(&q.join("\n"));
                    }
                    (ParaKind::Code, None) => {
                        let raw: String = p.runs.iter().map(|r| r.text.as_str()).collect();
                        let fence = if raw.contains("```") { "~~~~" } else { "```" };
                        out.push_str(&format!("{fence}\n{}\n{fence}", raw.trim_matches('\n')));
                    }
                    (ParaKind::Normal, None) => {
                        let lines: Vec<String> = text.lines().map(escape_line_start).collect();
                        out.push_str(&lines.join("\n"));
                    }
                }
            }
            Block::Table(rows) => {
                prev_list = false;
                top_num = None;
                // Rubrikraden skrivs i fetstil av Kladd; visa den utan **.
                let mut rows = rows.clone();
                if rows[0].iter().all(|c| c.is_empty() || (c.len() > 4 && c.starts_with("**") && c.ends_with("**"))) {
                    for c in rows[0].iter_mut() {
                        if c.len() > 4 {
                            *c = c[2..c.len() - 2].to_owned();
                        }
                    }
                }
                let cols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
                // Tabeller justeras inte.
                if cur_align != Align::Left {
                    set_align(&mut out, &mut cur_align, Align::Left);
                } else if !out.is_empty() {
                    out.push_str("\n\n");
                }
                let row_md = |r: &Vec<String>| {
                    let mut cells: Vec<String> = r.iter().map(|c| if c.is_empty() { " ".into() } else { c.clone() }).collect();
                    cells.resize(cols, " ".into());
                    format!("| {} |", cells.join(" | "))
                };
                out.push_str(&row_md(&rows[0]));
                out.push('\n');
                out.push_str(&format!("|{}", " --- |".repeat(cols)));
                for r in &rows[1..] {
                    out.push('\n');
                    out.push_str(&row_md(r));
                }
            }
        }
    }
    if cur_align != Align::Left {
        out.push_str("\n\n</div>");
    }
    out.push('\n');
    out
}

/// Byter öppen `<div>`: stänger den förra och öppnar en ny, och lämnar `out` redo för nästa block
/// (avslutad med tom rad).
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

// ============================================================== skrivning

fn xml_escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Kontrolltecken är inte tillåtna i XML 1.0.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
}

#[derive(Default, Clone)]
struct WFmt {
    bold: u32,
    italic: u32,
    strike: u32,
    underline: u32,
    link: Option<String>,
}

struct Writer {
    body: String,
    rels: Vec<(String, String)>, // (id, url)
    /// Numrerade listor får egna w:num så att numreringen börjar om.
    ordered_nums: Vec<u64>,
    para_open: bool,
    ppr: String,
    fmt: WFmt,
    /// Listor: (ordnad, numId)
    lists: Vec<(bool, u32)>,
    item_first_para: bool,
    quote: u32,
    heading: Option<u8>,
    table: Option<TableState>,
    in_code_block: bool,
    /// Bild som håller på att läsas: (källa, alt-text).
    image: Option<(String, String)>,
    /// Inbäddade bilder: (relations-id, filnamn i word/media, data).
    media: Vec<(String, String, Vec<u8>)>,
    /// Bilder som inte kunde bäddas in.
    missing: Vec<String>,
    /// Öppna `<span style="…">`, var och en sammanslagen med de utanför.
    spans: Vec<SpanStyle>,
    /// Öppna `<div align="…">`.
    aligns: Vec<Align>,
}

struct TableState {
    cols: usize,
    in_head: bool,
}

impl Writer {
    fn new() -> Self {
        Self {
            body: String::new(),
            rels: Vec::new(),
            ordered_nums: Vec::new(),
            para_open: false,
            ppr: String::new(),
            fmt: WFmt::default(),
            lists: Vec::new(),
            item_first_para: false,
            quote: 0,
            heading: None,
            table: None,
            in_code_block: false,
            image: None,
            media: Vec::new(),
            missing: Vec::new(),
            spans: Vec::new(),
            aligns: Vec::new(),
        }
    }

    /// Bäddar in en bild som en egen körning i aktuellt stycke.
    fn picture(&mut self, bytes: Vec<u8>, extent: Option<(i64, i64)>, alt: &str) {
        let Some(ext) = image_ext(&bytes) else { return };
        // Samma bild flera gånger bäddas bara in en gång.
        let existing = self.media.iter().find(|(_, _, b)| *b == bytes).map(|(id, _, _)| id.clone());
        let n = self.media.len() + 1;
        let (cx, cy) = image_extent(&bytes, extent);
        let id = match existing {
            Some(id) => id,
            None => {
                let id = format!("rIdImg{n}");
                self.media.push((id.clone(), format!("image{n}.{ext}"), bytes));
                id
            }
        };
        let doc_pr = self.body.matches("<wp:docPr ").count() + 1;
        let mut alt_x = String::new();
        xml_escape(alt, &mut alt_x);
        self.open_para();
        self.body.push_str(&format!(
            r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{doc_pr}" name="Bild {doc_pr}" descr="{alt_x}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{doc_pr}" name="Bild {doc_pr}" descr="{alt_x}"/><pic:cNvPicPr><a:picLocks noChangeAspect="1" noChangeArrowheads="1"/></pic:cNvPicPr></pic:nvPicPr><pic:blipFill><a:blip r:embed="{id}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr bwMode="auto"><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
        ));
    }

    fn para_props(&self) -> String {
        let mut ppr = String::new();
        if let Some(h) = self.heading {
            ppr.push_str(&format!("<w:pStyle w:val=\"Heading{h}\"/>"));
        } else if self.in_code_block {
            ppr.push_str("<w:pStyle w:val=\"Code\"/>");
        } else if self.quote > 0 {
            ppr.push_str("<w:pStyle w:val=\"Quote\"/>");
        } else if !self.lists.is_empty() {
            ppr.push_str("<w:pStyle w:val=\"ListParagraph\"/>");
        }
        if self.table.as_ref().is_some_and(|t| t.in_head) {
            ppr.push_str("<w:keepNext/>");
        }
        if let Some(&(_, num)) = self.lists.last() {
            if self.heading.is_none() {
                let lvl = (self.lists.len() - 1).min(8);
                if self.item_first_para {
                    ppr.push_str(&format!("<w:numPr><w:ilvl w:val=\"{lvl}\"/><w:numId w:val=\"{num}\"/></w:numPr>"));
                } else {
                    ppr.push_str(&format!("<w:ind w:left=\"{}\"/>", 720 * (lvl + 1)));
                }
            }
        }
        // Justering från en omgivande `<div>`; inte i tabeller eller kodblock.
        if let Some(a) = self.aligns.last().filter(|a| **a != Align::Left) {
            if self.table.is_none() && !self.in_code_block {
                ppr.push_str(&format!("<w:jc w:val=\"{}\"/>", a.as_jc()));
            }
        }
        ppr
    }

    fn open_para(&mut self) {
        if self.para_open {
            return;
        }
        let ppr = self.para_props();
        self.body.push_str("<w:p>");
        if !ppr.is_empty() {
            self.body.push_str(&format!("<w:pPr>{ppr}</w:pPr>"));
        }
        self.ppr = ppr;
        self.para_open = true;
        self.item_first_para = false;
    }

    fn close_para(&mut self) {
        if self.para_open {
            self.body.push_str("</w:p>");
            self.para_open = false;
        }
    }

    fn run(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        self.open_para();
        let mut rpr = String::new();
        if self.fmt.link.is_some() {
            rpr.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
        } else if code {
            rpr.push_str("<w:rStyle w:val=\"CodeChar\"/>");
        }
        // Ordningen följer schemat: rStyle, rFonts, b, i, strike, color, sz, szCs, highlight, u, shd.
        let span = self.spans.last().cloned().unwrap_or_default();
        if let Some(f) = &span.font_family {
            let mut name = String::new();
            xml_escape(f, &mut name);
            rpr.push_str(&format!("<w:rFonts w:ascii=\"{name}\" w:hAnsi=\"{name}\" w:cs=\"{name}\" w:eastAsia=\"{name}\"/>"));
        }
        if self.fmt.bold > 0 || self.table.as_ref().is_some_and(|t| t.in_head) {
            rpr.push_str("<w:b/>");
        }
        if self.fmt.italic > 0 {
            rpr.push_str("<w:i/>");
        }
        if self.fmt.strike > 0 {
            rpr.push_str("<w:strike/>");
        }
        if let Some(c) = span.color {
            rpr.push_str(&format!("<w:color w:val=\"{:02X}{:02X}{:02X}\"/>", c[0], c[1], c[2]));
        }
        if let Some(pt) = span.font_size {
            let half = (pt * 2.0).round().max(1.0) as u32;
            rpr.push_str(&format!("<w:sz w:val=\"{half}\"/><w:szCs w:val=\"{half}\"/>"));
        }
        // Exakt en av Words markeringsfärger blir markering, annars skuggning.
        let shading = span.background.and_then(|bg| match style::highlight_name(bg) {
            Some(name) => {
                rpr.push_str(&format!("<w:highlight w:val=\"{name}\"/>"));
                None
            }
            None => Some(bg),
        });
        if self.fmt.underline > 0 {
            rpr.push_str("<w:u w:val=\"single\"/>");
        }
        if let Some(c) = shading {
            rpr.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{:02X}{:02X}{:02X}\"/>", c[0], c[1], c[2]));
        }
        let mut xml = String::from("<w:r>");
        if !rpr.is_empty() {
            xml.push_str(&format!("<w:rPr>{rpr}</w:rPr>"));
        }
        let mut first = true;
        for line in text.split('\n') {
            if !first {
                xml.push_str("<w:br/>");
            }
            first = false;
            for (i, piece) in line.split('\t').enumerate() {
                if i > 0 {
                    xml.push_str("<w:tab/>");
                }
                if !piece.is_empty() {
                    xml.push_str("<w:t xml:space=\"preserve\">");
                    xml_escape(piece, &mut xml);
                    xml.push_str("</w:t>");
                }
            }
        }
        xml.push_str("</w:r>");
        if let Some(url) = self.fmt.link.clone() {
            let id = self.link_rel(&url);
            self.body.push_str(&format!("<w:hyperlink r:id=\"{id}\" w:history=\"1\">{xml}</w:hyperlink>"));
        } else {
            self.body.push_str(&xml);
        }
    }

    fn link_rel(&mut self, url: &str) -> String {
        if let Some((id, _)) = self.rels.iter().find(|(_, u)| u == url) {
            return id.clone();
        }
        let id = format!("rIdL{}", self.rels.len() + 1);
        self.rels.push((id.clone(), url.to_owned()));
        id
    }

    fn line_break(&mut self) {
        self.open_para();
        self.body.push_str("<w:r><w:br/></w:r>");
    }
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

fn body_from_markdown(md: &str, resolve: &dyn Fn(&str) -> Option<Image>) -> Writer {
    let mut w = Writer::new();
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let mut next_num: u32 = 2; // 1 = punktlistor
    for ev in Parser::new_ext(md, opts) {
        match ev {
            MdEvent::Start(tag) => match tag {
                Tag::Paragraph => {
                    w.close_para();
                    w.open_para();
                }
                Tag::Heading { level, .. } => {
                    w.close_para();
                    w.heading = Some(heading_num(level));
                    w.open_para();
                }
                Tag::BlockQuote(_) => {
                    w.close_para();
                    w.quote += 1;
                }
                Tag::CodeBlock(_) => {
                    w.close_para();
                    w.in_code_block = true;
                    w.open_para();
                }
                Tag::List(start) => {
                    w.close_para();
                    if start.is_some() {
                        w.ordered_nums.push(start.unwrap_or(1));
                        w.lists.push((true, next_num));
                        next_num += 1;
                    } else {
                        w.lists.push((false, 1));
                    }
                }
                Tag::Item => {
                    w.close_para();
                    w.item_first_para = true;
                    w.open_para();
                }
                Tag::Table(aligns) => {
                    w.close_para();
                    let cols = aligns.len().max(1);
                    let col_w = 9026 / cols; // A4-bredd minus marginaler, i tjugondels punkter
                    w.body.push_str(
                        "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"5000\" w:type=\"pct\"/><w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"1\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>",
                    );
                    for _ in 0..cols {
                        w.body.push_str(&format!("<w:gridCol w:w=\"{col_w}\"/>"));
                    }
                    w.body.push_str("</w:tblGrid>");
                    w.table = Some(TableState { cols, in_head: false });
                }
                Tag::TableHead => {
                    if let Some(t) = w.table.as_mut() {
                        t.in_head = true;
                    }
                    w.body.push_str("<w:tr><w:trPr><w:tblHeader/></w:trPr>");
                }
                Tag::TableRow => w.body.push_str("<w:tr>"),
                Tag::TableCell => {
                    let cols = w.table.as_ref().map_or(1, |t| t.cols);
                    w.body.push_str(&format!(
                        "<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/></w:tcPr>",
                        9026 / cols
                    ));
                    w.open_para();
                }
                Tag::Emphasis => w.fmt.italic += 1,
                Tag::Strong => w.fmt.bold += 1,
                Tag::Strikethrough => w.fmt.strike += 1,
                Tag::Link { dest_url, .. } => {
                    let url = dest_url.to_string();
                    // Interna ankare (#…) och tomma länkar blir vanlig text.
                    if !url.is_empty() && !url.starts_with('#') {
                        w.fmt.link = Some(url);
                    }
                }
                Tag::Image { dest_url, .. } => {
                    w.image = Some((dest_url.to_string(), String::new()));
                }
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
                TagEnd::CodeBlock => {
                    // Ta bort en avslutande radbrytning i kodblocket.
                    if w.body.ends_with("<w:r><w:br/></w:r>") {
                        let n = w.body.len() - "<w:r><w:br/></w:r>".len();
                        w.body.truncate(n);
                    }
                    w.close_para();
                    w.in_code_block = false;
                }
                TagEnd::List(_) => {
                    w.close_para();
                    w.lists.pop();
                }
                TagEnd::Item => w.close_para(),
                TagEnd::Table => {
                    w.body.push_str("</w:tbl>");
                    w.table = None;
                }
                TagEnd::TableHead => {
                    w.body.push_str("</w:tr>");
                    if let Some(t) = w.table.as_mut() {
                        t.in_head = false;
                    }
                }
                TagEnd::TableRow => w.body.push_str("</w:tr>"),
                TagEnd::TableCell => {
                    w.open_para(); // en cell måste innehålla minst ett stycke
                    w.close_para();
                    w.body.push_str("</w:tc>");
                }
                TagEnd::Emphasis => w.fmt.italic = w.fmt.italic.saturating_sub(1),
                TagEnd::Strong => w.fmt.bold = w.fmt.bold.saturating_sub(1),
                TagEnd::Strikethrough => w.fmt.strike = w.fmt.strike.saturating_sub(1),
                TagEnd::Link => w.fmt.link = None,
                TagEnd::Image => {
                    if let Some((src, alt)) = w.image.take() {
                        match resolve(&src) {
                            Some(img) if is_supported_image(&img.bytes) => w.picture(img.bytes, img.extent, &alt),
                            _ => {
                                w.missing.push(src.clone());
                                // Behåll åtminstone texten.
                                let label = if alt.is_empty() { format!("[bild: {src}]") } else { format!("[bild: {alt}]") };
                                w.run(&label, false);
                            }
                        }
                    }
                }
                _ => {}
            },
            MdEvent::Text(t) if w.image.is_some() => {
                if let Some((_, alt)) = w.image.as_mut() {
                    alt.push_str(&t);
                }
            }
            MdEvent::Code(t) if w.image.is_some() => {
                if let Some((_, alt)) = w.image.as_mut() {
                    alt.push_str(&t);
                }
            }
            MdEvent::Text(t) => {
                if w.in_code_block {
                    let t = t.to_string();
                    w.run(&t, true);
                } else {
                    w.run(&t, false);
                }
            }
            MdEvent::Code(t) => w.run(&t, true),
            MdEvent::Html(t) | MdEvent::InlineHtml(t) => {
                let s = t.to_string();
                let single_line = s.trim().lines().count() == 1;
                if s.trim().eq_ignore_ascii_case("<br>") || s.trim().eq_ignore_ascii_case("<br/>") {
                    w.line_break();
                } else if let Some(style) = style::parse_span_open(&s) {
                    let merged = w.spans.last().map_or(style.clone(), |outer| outer.merged(&style));
                    w.spans.push(merged);
                } else if style::is_span_close(&s) {
                    w.spans.pop();
                } else if let Some(align) = style::parse_div_open(&s).filter(|_| single_line) {
                    w.close_para();
                    w.aligns.push(align);
                } else if single_line && style::is_div_close(&s) {
                    w.close_para();
                    w.aligns.pop();
                } else if s.trim().eq_ignore_ascii_case("<u>") {
                    w.fmt.underline += 1;
                } else if s.trim().eq_ignore_ascii_case("</u>") {
                    w.fmt.underline = w.fmt.underline.saturating_sub(1);
                } else {
                    w.run(s.trim_end_matches('\n'), false);
                }
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => w.line_break(),
            MdEvent::Rule => {
                w.close_para();
                w.body.push_str("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"auto\"/></w:pBdr></w:pPr></w:p>");
            }
            MdEvent::TaskListMarker(done) => w.run(if done { "☒ " } else { "☐ " }, false),
            MdEvent::FootnoteReference(t) => w.run(&format!("[{t}]"), false),
            MdEvent::InlineMath(t) | MdEvent::DisplayMath(t) => w.run(&t, false),
        }
    }
    w.close_para();
    w
}

/// Textytans bredd på en A4-sida med 2,54 cm marginaler, i EMU.
const MAX_IMAGE_WIDTH_EMU: i64 = 9026 * 635;
const EMU_PER_PIXEL: i64 = 9525; // 96 dpi

fn image_extent(bytes: &[u8], extent: Option<(i64, i64)>) -> (i64, i64) {
    let (mut cx, mut cy) = extent.unwrap_or_else(|| match imagesize::blob_size(bytes) {
        Ok(sz) => (sz.width as i64 * EMU_PER_PIXEL, sz.height as i64 * EMU_PER_PIXEL),
        Err(_) => (MAX_IMAGE_WIDTH_EMU / 2, MAX_IMAGE_WIDTH_EMU / 3),
    });
    cx = cx.max(1);
    cy = cy.max(1);
    if cx > MAX_IMAGE_WIDTH_EMU {
        cy = cy * MAX_IMAGE_WIDTH_EMU / cx;
        cx = MAX_IMAGE_WIDTH_EMU;
    }
    (cx, cy.max(1))
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="jpeg" ContentType="image/jpeg"/><Default Extension="gif" ContentType="image/gif"/><Default Extension="bmp" ContentType="image/bmp"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#;

const SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:defaultTabStop w:val="708"/><w:characterSpacingControl w:val="doNotCompress"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#;

fn heading_style(n: u8, size: u32, color: &str) -> String {
    format!(
        r#"<w:style w:type="paragraph" w:styleId="Heading{n}"><w:name w:val="heading {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="{}" w:after="80"/><w:outlineLvl w:val="{}"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi"/><w:b/><w:color w:val="{color}"/><w:sz w:val="{size}"/><w:szCs w:val="{size}"/></w:rPr></w:style>"#,
        if n == 1 { 360 } else { 240 },
        n - 1
    )
}

fn styles_xml(defaults: &DocDefaults) -> String {
    // Dokumentets grundformat: ett namngivet typsnitt ersätter temats, och storleken räknas i halvpunkter.
    let fonts = match &defaults.font_family {
        Some(f) => {
            let mut name = String::new();
            xml_escape(f, &mut name);
            format!("<w:rFonts w:ascii=\"{name}\" w:eastAsia=\"{name}\" w:hAnsi=\"{name}\" w:cs=\"{name}\"/>")
        }
        None => r#"<w:rFonts w:asciiTheme="minorHAnsi" w:eastAsiaTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi" w:cstheme="minorBidi"/>"#.to_owned(),
    };
    let half = (defaults.size() * 2.0).round().max(1.0) as u32;
    let mut s = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr>{fonts}<w:sz w:val="{half}"/><w:szCs w:val="{half}"/><w:lang w:val="sv-SE" w:eastAsia="en-US" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/><w:unhideWhenUsed/></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style><w:style w:type="numbering" w:default="1" w:styleId="NoList"><w:name w:val="No List"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/></w:style>"#,
    );
    let heads = [(1, 36, "1F3864"), (2, 30, "2F5496"), (3, 26, "2F5496"), (4, 24, "2F5496"), (5, 22, "2F5496"), (6, 22, "404040")];
    for (n, size, color) in heads {
        s.push_str(&heading_style(n, size, color));
    }
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="29"/><w:qFormat/><w:pPr><w:spacing w:before="120"/><w:ind w:left="567" w:right="567"/><w:pBdr><w:left w:val="single" w:sz="12" w:space="8" w:color="A0A0A0"/></w:pBdr></w:pPr><w:rPr><w:i/><w:color w:val="404040"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="34"/><w:qFormat/><w:pPr><w:spacing w:after="60"/><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="39"/><w:qFormat/><w:pPr><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style><w:style w:type="character" w:styleId="CodeChar"><w:name w:val="Code Char"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="39"/><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:rPr></w:style><w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:uiPriority w:val="39"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style></w:styles>"#);
    s
}

fn numbering_xml(ordered_starts: &[u64]) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/>"#,
    );
    let bullets = ["•", "◦", "▪"];
    for lvl in 0..9 {
        let b = bullets[lvl % 3];
        let left = 720 * (lvl + 1);
        s.push_str(&format!(
            r#"<w:lvl w:ilvl="{lvl}"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="{b}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#
        ));
    }
    s.push_str(r#"</w:abstractNum><w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="hybridMultilevel"/>"#);
    let fmts = ["decimal", "lowerLetter", "lowerRoman"];
    for lvl in 0..9 {
        let f = fmts[lvl % 3];
        let left = 720 * (lvl + 1);
        s.push_str(&format!(
            r#"<w:lvl w:ilvl="{lvl}"><w:start w:val="1"/><w:numFmt w:val="{f}"/><w:lvlText w:val="%{}."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#,
            lvl + 1
        ));
    }
    s.push_str(r#"</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#);
    for (i, start) in ordered_starts.iter().enumerate() {
        s.push_str(&format!(
            r#"<w:num w:numId="{}"><w:abstractNumId w:val="1"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="{}"/></w:lvlOverride></w:num>"#,
            i + 2,
            start
        ));
    }
    s.push_str("</w:numbering>");
    s
}

fn now_w3cdtf() -> String {
    // Utan extra beroenden: räkna ut UTC-datum från Unix-tid.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Algoritm från Howard Hinnant (civil_from_days).
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
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn core_xml(title: &str, created: &str) -> String {
    let now = now_w3cdtf();
    let mut t = String::new();
    xml_escape(title, &mut t);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{t}</dc:title><cp:lastModifiedBy>Kladd</cp:lastModifiedBy><dcterms:created xsi:type="dcterms:W3CDTF">{created}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{now}</dcterms:modified></cp:coreProperties>"#,
        created = if created.is_empty() { now.as_str() } else { created }
    )
}

const APP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Application>Kladd</Application><DocSecurity>0</DocSecurity></Properties>"#;

/// Resultatet av att spara: filens innehåll och bilder som inte kunde bäddas in.
pub struct Written {
    pub bytes: Vec<u8>,
    pub missing_images: Vec<String>,
}

/// Bygger ett komplett .docx-paket av Markdown-text.
/// `resolve` hämtar bilddata för en bildkälla (`bild:namn`, en sökväg …).
pub fn write(markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<Image>) -> Result<Written, String> {
    // Första raden kan vara dokumentets grundformat; den är inte text i dokumentet.
    let first = markdown.lines().next().unwrap_or("");
    let (defaults, markdown) = match DocDefaults::from_comment(first) {
        Some(d) => (d, markdown[first.len()..].trim_start_matches(['\n', '\r'])),
        None => (DocDefaults::default(), markdown),
    };
    let w = body_from_markdown(markdown, resolve);
    let mut body = w.body;
    // Ett dokument som slutar med en tabell måste ha ett stycke efter den.
    if body.is_empty() || body.ends_with("</w:tbl>") {
        body.push_str("<w:p/>");
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="{W_NS}" xmlns:r="{R_NS}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/><w:cols w:space="708"/></w:sectPr></w:body></w:document>"#
    );
    let mut doc_rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>"#,
    );
    for (id, url) in &w.rels {
        let mut u = String::new();
        xml_escape(url, &mut u);
        doc_rels.push_str(&format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="{u}" TargetMode="External"/>"#
        ));
    }
    for (id, file, _) in &w.media {
        doc_rels.push_str(&format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{file}"/>"#
        ));
    }
    doc_rels.push_str("</Relationships>");

    let parts: [(&str, String); 9] = [
        ("[Content_Types].xml", CONTENT_TYPES.to_owned()),
        ("_rels/.rels", ROOT_RELS.to_owned()),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", doc_rels),
        ("docProps/core.xml", core_xml(title, "")),
        ("docProps/app.xml", APP_XML.to_owned()),
        ("word/styles.xml", styles_xml(&defaults)),
        ("word/numbering.xml", numbering_xml(&w.ordered_nums)),
        ("word/settings.xml", SETTINGS.to_owned()),
    ];
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(6))
            .unix_permissions(0o644);
        for (name, content) in parts {
            zip.start_file(name, opts).map_err(|e| e.to_string())?;
            zip.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
        }
        // Bilder är redan komprimerade och lagras som de är.
        let stored = opts.compression_method(zip::CompressionMethod::Stored).compression_level(None);
        for (_, file, bytes) in &w.media {
            zip.start_file(format!("word/media/{file}"), stored).map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(Written { bytes: buf.into_inner(), missing_images: w.missing })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Planering för fritids\n\nVi ska **baka** och *måla* i dag, se [Skolverket](https://www.skolverket.se).\n\n## Att ta med\n\n- Förkläde\n- Mjöl\n    - Vetemjöl\n\n1. Värm ugnen\n2. Blanda\n\n> Kom ihåg att tvätta händerna.\n\n| Dag | Aktivitet |\n| --- | --- |\n| Måndag | Bakning |\n| Tisdag | Utflykt |\n\nPris: 5\\*3 kr och `kod` här.\n";

    #[test]
    fn roundtrip_keeps_structure() {
        let bytes = write(SAMPLE, "Test", &|_| None).unwrap().bytes;
        let back = read(&bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        assert_eq!(back.markdown, SAMPLE, "\n--- fick ---\n{}", back.markdown);
    }

    fn rt(md: &str) -> String {
        let bytes = write(md, "T", &|_| None).unwrap().bytes;
        read(&bytes).unwrap().markdown
    }

    #[test]
    fn numbered_list_keeps_numbers() {
        assert_eq!(rt("1. a\n2. b\n3. c\n"), "1. a\n2. b\n3. c\n");
        assert_eq!(rt("3. c\n4. d\n"), "3. c\n4. d\n");
    }

    #[test]
    fn numbered_list_after_interruption_keeps_start() {
        let md = "1. a\n2. b\n\nEtt stycke.\n\n3. c\n";
        assert_eq!(rt(md), md);
    }

    #[test]
    fn nested_numbers_restart_per_parent() {
        let md = "1. a\n    1. x\n    2. y\n2. b\n    1. z\n";
        assert_eq!(rt(md), md);
    }

    #[test]
    fn numbering_resets_deeper_levels_and_resumes() {
        let numbering = parse_numbering(
            r#"<w:numbering xmlns:w="w"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="5"/><w:numFmt w:val="decimal"/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#,
        );
        let item = |t: &str, l: u8| Block::Para(Para {
            style: None,
            runs: vec![Run { text: t.into(), fmt: Fmt::default(), link: None, raw: false }],
            kind: ParaKind::Normal,
            list: Some(("1".into(), l)),
            ..Para::new()
        });
        let blocks = vec![item("a", 0), item("x", 1), item("y", 1), item("b", 0), item("z", 1)];
        assert_eq!(to_markdown(&blocks, &numbering), "5. a\n    1. x\n    2. y\n6. b\n    1. z\n");
    }

    #[test]
    fn plain_text_survives() {
        let text = "Rad ett\nRad två med tab\there\n\nNytt stycke & <tecken> \"citat\"\n";
        let bytes = write(text, "x", &|_| None).unwrap().bytes;
        let back = read(&bytes).unwrap();
        assert_eq!(back.markdown, "Rad ett\nRad två med tab\there\n\nNytt stycke & \\<tecken> \"citat\"\n");
    }

    #[test]
    fn images_roundtrip() {
        // En liten giltig PNG (1x1 pixel).
        let png: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0, 1, 0, 0,
            5, 0, 1, 0x0D, 0x0A, 0x2D, 0xB4, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let md = "# Bilder\n\nFöre ![En sol](sol.png) efter.\n\n![](saknas.png)\n\n![Igen](sol.png)\n";
        let p2 = png.clone();
        let w = write(md, "x", &move |src| (src == "sol.png").then(|| Image { bytes: p2.clone(), extent: None })).unwrap();
        assert_eq!(w.missing_images, vec!["saknas.png".to_owned()]);
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        assert_eq!(back.images.len(), 1, "samma bild bäddas in en gång");
        assert_eq!(back.images[0].1.bytes, png);
        assert!(back.images[0].1.extent.is_some());
        let name = &back.images[0].0;
        assert_eq!(
            back.markdown,
            format!("# Bilder\n\nFöre ![En sol](bild:{name}) efter.\n\n\\[bild: saknas.png\\]\n\n![Igen](bild:{name})\n")
        );
        // Och tillbaka igen med bilderna från dokumentet.
        let imgs = back.images.clone();
        let w2 = write(&back.markdown, "x", &move |src| {
            let n = src.strip_prefix(IMAGE_SCHEME)?;
            imgs.iter().find(|(m, _)| m == n).map(|(_, i)| i.clone())
        })
        .unwrap();
        assert!(w2.missing_images.is_empty());
        assert_eq!(read(&w2.bytes).unwrap().markdown, back.markdown);
    }

    /// Skriver om en del (t.ex. `word/styles.xml`) i ett dokument; finns den inte läggs den till
    /// (då får `f` en tom sträng).
    fn patch_part(bytes: &[u8], part: &str, f: impl Fn(&str) -> String) -> Vec<u8> {
        let mut src = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut out = Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut out);
            let opts = zip::write::SimpleFileOptions::default();
            let mut found = false;
            for i in 0..src.len() {
                let mut file = src.by_index(i).unwrap();
                let name = file.name().to_owned();
                let mut data = Vec::new();
                std::io::Read::read_to_end(&mut file, &mut data).unwrap();
                if name == part {
                    found = true;
                    data = f(&String::from_utf8(data).unwrap()).into_bytes();
                }
                zw.start_file(name, opts).unwrap();
                zw.write_all(&data).unwrap();
            }
            if !found {
                zw.start_file(part, opts).unwrap();
                zw.write_all(f("").as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        out.into_inner()
    }

    /// Skriver om `word/document.xml` i ett dokument (för att prova läsningen av handgjord XML).
    fn patch_document(bytes: &[u8], f: impl Fn(&str) -> String) -> Vec<u8> {
        patch_part(bytes, "word/document.xml", f)
    }

    /// Skriver om `word/styles.xml`.
    fn patch_styles(bytes: &[u8], f: impl Fn(&str) -> String) -> Vec<u8> {
        patch_part(bytes, "word/styles.xml", f)
    }

    #[test]
    fn underline_roundtrip() {
        let md = "En <u>under</u> och <u>**båda**</u> här.\n";
        let w = write(md, "x", &|_| None).unwrap();
        let back = read(&w.bytes).unwrap();
        assert_eq!(back.markdown, md);
        let xml = patch_document(&w.bytes, |x| x.to_owned());
        let mut z = zip::ZipArchive::new(Cursor::new(xml)).unwrap();
        let mut doc = String::new();
        std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut doc).unwrap();
        assert!(doc.contains("<w:u w:val=\"single\"/>"));
        assert!(!doc.contains("&lt;u&gt;") && !doc.contains("<u>"));
        // Schemaordning: b, i, strike före u.
        assert!(doc.contains("<w:b/><w:u w:val=\"single\"/>"));
    }

    #[test]
    fn reads_underline_from_runs() {
        let w = write("a b c\n", "x", &|_| None).unwrap();
        let patched = patch_document(&w.bytes, |x| {
            let run = |rpr: &str, t: &str| format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{t}</w:t></w:r>");
            let start = x.find("<w:r>").unwrap();
            let end = x.rfind("</w:r>").unwrap() + "</w:r>".len();
            let runs = [
                run("<w:u w:val=\"single\"/>", "ett "),
                run("<w:u w:val=\"none\"/>", "två "),
                run("<w:u/>", "tre"),
            ]
            .concat();
            format!("{}{}{}", &x[..start], runs, &x[end..])
        });
        let back = read(&patched).unwrap();
        assert_eq!(back.markdown, "<u>ett</u> två <u>tre</u>\n");
    }

    /// Läser en del som text ur ett skrivet dokument.
    fn part_text(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut out = String::new();
        std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut out).unwrap();
        out
    }

    const STYLED: &str = "<!-- kladd: font-family: \"Times New Roman\"; font-size: 12pt -->

<div align=\"center\">

# Rubrik

Mitten med <span style=\"font-size: 20pt\">stor</span> text.

</div>

Typsnitt <span style=\"font-family: Arial\">Arial</span>, <span style=\"font-family: 'Gill Sans MT'\">Gill</span>, <span style=\"color: #C00000\">röd</span>, <span style=\"font-size: 14.5pt\">större</span>, <span style=\"background-color: #FFFF00\">gul</span> och <span style=\"background-color: #123456\">skuggad</span>.

Alla: <span style=\"font-family: Arial; font-size: 14pt; color: #C00000; background-color: #FFFF00\">allt</span> och <span style=\"color: #00AA00\"><u>**fet understruken**</u></span> samt <span style=\"color: #0000FF\">*kursiv*</span> och <span style=\"color: #0000FF\">`kod`</span>.

# <span style=\"color: #C00000\">Röd rubrik</span>

<div align=\"right\">

- ett
- två
    - tre

</div>

<div align=\"justify\">

En lång mening som justeras.

Och en till.

</div>

Sist vänster.
";

    #[test]
    fn styles_roundtrip() {
        let w = write(STYLED, "x", &|_| None).unwrap();
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        assert_eq!(back.markdown, STYLED, "\n--- fick ---\n{}", back.markdown);
        // Kommentarraden och taggarna får inte bli text i dokumentet.
        let doc = part_text(&w.bytes, "word/document.xml");
        assert!(!doc.contains("kladd:") && !doc.contains("&lt;span") && !doc.contains("&lt;div"), "{doc}");
        // Grundformatet hamnar i docDefaults.
        let styles = part_text(&w.bytes, "word/styles.xml");
        assert!(styles.contains(r#"<w:rFonts w:ascii="Times New Roman" w:eastAsia="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:sz w:val="24"/><w:szCs w:val="24"/>"#), "{styles}");
    }

    #[test]
    fn writes_span_properties_in_schema_order() {
        let md = "<span style=\"font-family: Arial; font-size: 14pt; color: #C00000; background-color: #FFFF00\"><u>**x**</u></span> <span style=\"background-color: #123456\"><u>y</u></span>\n\n<div align=\"center\">\n\n# Mitt\n\n</div>\n";
        let doc = part_text(&write(md, "x", &|_| None).unwrap().bytes, "word/document.xml");
        assert!(doc.contains(r#"<w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial" w:eastAsia="Arial"/><w:b/><w:color w:val="C00000"/><w:sz w:val="28"/><w:szCs w:val="28"/><w:highlight w:val="yellow"/><w:u w:val="single"/></w:rPr>"#), "{doc}");
        assert!(doc.contains(r#"<w:u w:val="single"/><w:shd w:val="clear" w:color="auto" w:fill="123456"/></w:rPr>"#), "{doc}");
        assert!(doc.contains(r#"<w:pPr><w:pStyle w:val="Heading1"/><w:jc w:val="center"/></w:pPr>"#), "{doc}");
    }

    #[test]
    fn plain_documents_get_no_comment_or_spans() {
        // Standardformatet (Calibri 11 pt) ger varken kommentarrad eller spans, även med länk, citat och kod.
        let md = "# Rubrik\n\nText med [länk](https://example.com) och `kod`.\n\n> Citat med **fet**.\n\n```\nkod\n```\n\n- punkt\n";
        assert_eq!(rt(md), md);
    }

    #[test]
    fn html_lookalikes_in_text_are_escaped() {
        let md = "\\<span style=\"color: red\"> och \\<div align=\"center\"> slut.\n\n\\</div>\n";
        let bytes = write(md, "x", &|_| None).unwrap().bytes;
        let doc = part_text(&bytes, "word/document.xml");
        assert!(doc.contains("&lt;span style=&quot;color: red&quot;&gt;") && doc.contains("&lt;div align=&quot;center&quot;&gt;"), "{doc}");
        assert!(!doc.contains("<w:jc") && !doc.contains("<w:color"), "{doc}");
        assert_eq!(read(&bytes).unwrap().markdown, md);
    }

    const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:fontScheme name="X"><a:majorFont><a:latin typeface="Georgia"/><a:ea typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Cambria"/><a:ea typeface=""/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;

    const HAND_STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi"/><w:sz w:val="24"/></w:rPr></w:rPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Rubrik1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi"/><w:color w:val="2F5496"/><w:sz w:val="40"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Mitt"><w:name w:val="Centered"/><w:basedOn w:val="Normal"/><w:pPr><w:jc w:val="center"/><w:shd w:val="clear" w:color="auto" w:fill="EEEEEE"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="MittBla"><w:name w:val="Centered Blue"/><w:basedOn w:val="Mitt"/><w:rPr><w:rFonts w:ascii="Verdana" w:hAnsi="Verdana"/><w:color w:val="0000FF"/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Rod"><w:name w:val="Red"/><w:rPr><w:color w:val="FF0000"/></w:rPr></w:style>
<w:style w:type="table" w:styleId="Tabell"><w:name w:val="Tabell"/><w:rPr><w:color w:val="123456"/></w:rPr></w:style>
</w:styles>"#;

    #[test]
    fn reads_fonts_sizes_colors_and_alignment() {
        let w = write("x\n", "x", &|_| None).unwrap();
        let bytes = patch_part(&w.bytes, "word/theme/theme1.xml", |_| THEME.to_owned());
        let bytes = patch_styles(&bytes, |_| HAND_STYLES.to_owned());
        let bytes = patch_document(&bytes, |x| {
            let start = x.find("<w:body>").unwrap() + "<w:body>".len();
            let end = x.find("<w:sectPr>").unwrap();
            let run = |rpr: &str, t: &str| format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{t}</w:t></w:r>");
            let para = |ppr: &str, runs: &str| format!("<w:p><w:pPr>{ppr}</w:pPr>{runs}</w:p>");
            let body = [
                // Rubrik: formatmallens färg, storlek och typsnitt ger ingen span.
                para("<w:pStyle w:val=\"Rubrik1\"/>", &run("", "Titel")),
                // ... men direkt färg på rubriken ger det.
                para("<w:pStyle w:val=\"Rubrik1\"/>", &run("<w:color w:val=\"FF0000\"/>", "Röd")),
                // Direkt färg, storlek och markering; samma format i två körningar slås ihop.
                para(
                    "",
                    &[
                        run("", "vanlig "),
                        run("<w:color w:val=\"C00000\"/><w:sz w:val=\"28\"/><w:highlight w:val=\"yellow\"/>", "stor"),
                        run("<w:color w:val=\"C00000\"/><w:sz w:val=\"28\"/><w:highlight w:val=\"yellow\"/>", " röd"),
                        run("<w:color w:val=\"auto\"/><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"auto\"/>", " auto"),
                        run("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"123456\"/>", " skugga"),
                        run("<w:rStyle w:val=\"Rod\"/>", " mall"),
                        run("<w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:sz w:val=\"24\"/>", " arial"),
                    ]
                    .concat(),
                ),
                // Justering ärvd genom basedOn (Mitt <- MittBla), plus typsnitt och färg från mallen.
                para("<w:pStyle w:val=\"Mitt\"/>", &run("", "Mitt")),
                para("<w:pStyle w:val=\"MittBla\"/>", &run("", "Blå mitt")),
                // Direkt jc slår formatmallens.
                para("<w:pStyle w:val=\"Mitt\"/><w:jc w:val=\"left\"/>", &run("", "Vänster")),
                para("<w:jc w:val=\"right\"/>", &run("", "Höger")),
                para("<w:jc w:val=\"both\"/>", &run("", "Marginal")),
            ]
            .concat();
            format!("{}{}{}", &x[..start], body, &x[end..])
        });
        let back = read(&bytes).unwrap();
        let expected = "<!-- kladd: font-family: Cambria; font-size: 12pt -->

# Titel

# <span style=\"color: #FF0000\">Röd</span>

vanlig <span style=\"font-size: 14pt; color: #C00000; background-color: #FFFF00\">stor röd</span> auto <span style=\"background-color: #123456\">skugga</span> <span style=\"color: #FF0000\">mall</span> <span style=\"font-family: Arial\">arial</span>

<div align=\"center\">

Mitt

<span style=\"font-family: Verdana; color: #0000FF\">Blå mitt</span>

</div>

Vänster

<div align=\"right\">

Höger

</div>

<div align=\"justify\">

Marginal

</div>
";
        assert_eq!(back.markdown, expected, "\n--- fick ---\n{}", back.markdown);
    }

    #[test]
    fn rejects_garbage_and_old_doc() {
        assert!(read(b"hello").is_err());
        assert!(read(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]).is_err());
    }

    #[test]
    fn escapes_markdown_lookalikes() {
        assert_eq!(escape_line_start("# inte rubrik"), "\\# inte rubrik");
        assert_eq!(escape_line_start("1. inte lista"), "1\\. inte lista");
        assert_eq!(escape_line_start("- inte punkt"), "\\- inte punkt");
        assert_eq!(escape_line_start("vanlig text"), "vanlig text");
    }
}

/// Manuell provning mot andra program:
/// `KLADD_DOCX_IN=a.docx KLADD_MD_IN=b.md KLADD_DOCX_OUT=c.docx cargo test docx_manual -- --ignored --nocapture`
#[cfg(test)]
mod manual {
    #[test]
    #[ignore]
    fn docx_manual() {
        if let Ok(p) = std::env::var("KLADD_DOCX_IN") {
            for f in p.split(':') {
                let r = super::read(&std::fs::read(f).unwrap());
                match r {
                    Ok(l) => println!(
                        "===== {f} (tappat: {:?}, bilder: {:?})\n{}",
                        l.dropped,
                        l.images.iter().map(|(n, i)| (n.clone(), i.bytes.len(), i.extent)).collect::<Vec<_>>(),
                        l.markdown
                    ),
                    Err(e) => println!("===== {f}: FEL {e}"),
                }
            }
        }
        if let (Ok(i), Ok(o)) = (std::env::var("KLADD_MD_IN"), std::env::var("KLADD_DOCX_OUT")) {
            let md = std::fs::read_to_string(i).unwrap();
            let dir = std::path::Path::new(&o).parent().unwrap().to_path_buf();
            let w = super::write(&md, "Prov", &|src| {
                std::fs::read(dir.join(src)).ok().map(|bytes| super::Image { bytes, extent: None })
            })
            .unwrap();
            println!("saknade bilder: {:?}", w.missing_images);
            std::fs::write(o, w.bytes).unwrap();
        }
    }
}
