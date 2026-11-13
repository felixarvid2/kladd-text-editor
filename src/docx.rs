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

use pulldown_cmark::{CowStr, Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::review::{self, CommentRec, RevKind, RevRec};
use crate::word::sections;
use crate::style::{self, Align, DocDefaults, SpanStyle};
use crate::word::markup::{self, Float};

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
    /// Originalpaketet för förlustfri sparning (saknas om paketet inte kunde läsas helt).
    pub source: Option<Source>,
    pub markdown: String,
    /// Inbäddade bilder: namn → bild, i dokumentordning.
    pub images: Vec<(String, Image)>,
    /// Saker i originalet som Kladd inte kan visa eller spara, i läsbar form.
    pub dropped: Vec<&'static str>,
}

/// Ett objekt som Kladd inte modellerar (fotnotsreferens, fält, bokmärke, textruta …).
/// Originalets XML skrivs tillbaka där markören `<!--k:N-->` står i texten.
#[derive(Clone, Debug)]
struct Opaque {
    xml: String,
    /// Antal fältstarter minus fältslut i XML:en (för att hålla fälten balanserade).
    fld: i32,
}

/// Ett toppnivåelement i `w:body` (stycke, tabell, innehållskontroll …) med sin ursprungliga XML
/// och de Markdown-enheter (beskrivning, nyckel) det gav upphov till. Inga enheter = tyst
/// (tomt stycke, bokmärke, sidbrytning); det följer då med sitt föregående element.
#[derive(Clone, Debug)]
struct SrcElem {
    xml: String,
    units: Vec<(String, String)>,
    /// Id:n (`w:footnoteReference w:id`) för fotnotsreferenserna som blev `[^N]` i elementet, i ordning.
    fn_ids: Vec<String>,
}

/// En fotnot i originalets footnotes.xml.
#[derive(Clone, Debug)]
struct FnOrig {
    id: String,
    /// `w:type` (separator, continuationSeparator …); `None` för vanliga fotnoter.
    ty: Option<String>,
    /// Texten i Kladds Markdown (en rad, stycken skilda med `<br>`).
    md: String,
    /// Hela `<w:footnote>`-elementet.
    xml: String,
}

/// Originalets footnotes.xml: delens namn, allt före och efter fotnoterna och fotnoterna själva.
#[derive(Clone, Debug)]
struct FnPart {
    name: String,
    head: String,
    tail: String,
    items: Vec<FnOrig>,
}

/// En fotnotsreferens i brödtexten: id och toppnivåelementet den står i.
struct FnRefInfo {
    id: String,
    origin: usize,
}

/// Originalet av ett öppnat Word-dokument: alla delar i paketet, `w:sectPr`, varje blocks
/// ursprungliga XML och dess Markdown, samt de okända objekt som markörerna pekar på.
/// `write` använder det för att bara skriva om det som ändrats.
#[derive(Clone, Debug)]
pub struct Source {
    /// Alla poster i zip-filen, i ordning.
    parts: Vec<(String, Vec<u8>)>,
    /// Huvuddokumentets sökväg i paketet.
    main: String,
    /// Allt i huvuddokumentet fram till och med `<w:body>`.
    head: String,
    /// Allt från `</w:body>` till slutet.
    tail: String,
    sect_pr: Option<String>,
    elems: Vec<SrcElem>,
    tokens: Vec<Opaque>,
    /// Originalets fotnoter (footnotes.xml), om dokumentet har några.
    footnotes: Option<FnPart>,
    /// Sida, sidhuvud och sidfot så som de lästes (övriga fält i DocDefaults är tomma).
    page: DocDefaults,
    /// Formatmallarna så som de lästes ur `styles.xml` (se `docx_styles`).
    styles: style::StyleSet,
    /// Originalets kommentarer (comments.xml), om dokumentet har några.
    comments: Option<review_xml::CmPart>,
    /// Sidhuvuds- och sidfotsreferenserna i första avsnittets `w:sectPr` (de gäller också senare avsnitt).
    first_refs: String,
}

// ============================================================== läsning

/// Kontrollerar ett zip-pakets slutpost innan `zip` får tolka det, så att ett
/// paket som påstår sig ha miljontals poster avvisas direkt (jfr WordCraft #3660).
fn check_entry_count(bytes: &[u8]) -> Result<(), String> {
    const EOCD: &[u8] = b"PK\x05\x06";
    let start = bytes.len().saturating_sub(22 + 65_535);
    let Some(rel) = bytes[start..].windows(4).rposition(|w| w == EOCD) else {
        return Err(crate::i18n::t("Filen är inte ett giltigt Word-dokument (zip-katalogen saknas).").into());
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
                    return Err(crate::i18n::t("Word-dokumentet innehåller orimligt många delar och öppnas inte.").into());
                }
            }
        }
    } else if total > MAX_ENTRIES {
        return Err(crate::i18n::t("Word-dokumentet innehåller orimligt många delar och öppnas inte.").into());
    }
    Ok(())
}

/// Största sammanlagda uppackade mängd som läses ur ett paket.
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// XML i OOXML är UTF-8 (ibland med BOM) eller UTF-16 med BOM.
fn decode_xml(buf: &[u8]) -> String {
    if buf.starts_with(&[0xFF, 0xFE]) {
        encoding_rs::UTF_16LE.decode_with_bom_removal(buf).0.into_owned()
    } else if buf.starts_with(&[0xFE, 0xFF]) {
        encoding_rs::UTF_16BE.decode_with_bom_removal(buf).0.into_owned()
    } else {
        let b = buf.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(buf);
        String::from_utf8_lossy(b).into_owned()
    }
}

struct Package<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    total: u64,
}

impl Package<'_> {
    fn part(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(buf) = self.part_bytes(name)? else { return Ok(None) };
        Ok(Some(decode_xml(&buf)))
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
            return Err(crate::i18n::t("En del av Word-dokumentet är för stor för att öppnas.").into());
        }
        let mut buf = Vec::with_capacity(file.size() as usize);
        let res = file.by_ref().take(room + 1).read_to_end(&mut buf);
        // Räkna det som packats upp även om läsningen misslyckas (jfr WordCraft #3723).
        self.total += buf.len() as u64;
        res.map_err(|e| crate::tf!("Word-dokumentet är skadat: {e}", e = e))?;
        if buf.len() as u64 > room {
            return Err(crate::i18n::t("En del av Word-dokumentet är för stor för att öppnas.").into());
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
#[derive(Default, Clone)]
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
    /// Spårad ändring (`w:ins`/`w:del`) som körningen ligger i: slag och ändringens nummer.
    rev: Option<(RevKind, u32)>,
    /// Spårad teckenformatering (`w:rPrChange`): ändringens nummer (det gamla formatet står i posten).
    fchange: Option<u32>,
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
    /// Indexet i `BodyInfo::elems` för toppnivåelementet stycket kommer från.
    origin: usize,
}

impl Para {
    fn new() -> Para {
        Para { style: None, kind: ParaKind::Normal, list: None, runs: Vec::new(), align: Align::Left, direct_align: None, origin: usize::MAX }
    }
}

enum Block {
    Para(Para),
    /// Tabellen och toppnivåelementets index.
    Table(Tbl, usize),
}

/// En tabell som läses: celler per rad (en sammanfogad cell följs av tomma platshållare),
/// deras spann (0 = täckt av cellen före) och kolumnbredderna ur `w:tblGrid` (tjugondels punkter).
#[derive(Default, Clone)]
struct Tbl {
    rows: Vec<Vec<String>>,
    spans: Vec<Vec<usize>>,
    widths: Vec<f64>,
    /// Pågående rad och cell.
    row: Vec<String>,
    row_spans: Vec<usize>,
    cell: Option<String>,
    span: usize,
}

/// Det som faktiskt går förlorat: det mesta bevaras nu orört, men spårade formateringsändringar på
/// stycketecknet (och när spårning inte läses) godtas när ett stycke skrivs om.
#[derive(Default)]
struct Dropped {
    tracked: bool,
}

impl Dropped {
    fn list(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.tracked {
            v.push(crate::i18n::t("spårade formateringsändringar"));
        }
        v
    }
}

/// Det parse_body samlar om originalet.
#[derive(Default)]
struct BodyInfo {
    /// Djupet (antal öppna element) för barnen till `w:body`.
    body_depth: Option<usize>,
    head: String,
    tail: String,
    sect_pr: Option<String>,
    /// `w:sectPr` i stycken (slut på varje avsnitt utom det sista), i dokumentordning.
    sects: Vec<String>,
    /// XML för varje toppnivåelement utom `w:sectPr`.
    elems: Vec<String>,
    tokens: Vec<Opaque>,
    /// Fotnotsreferenserna i dokumentordning (referens nummer N är `[^N]`).
    fn_refs: Vec<FnRefInfo>,
    /// Läs spårade ändringar och kommentarer till Kladds markörer (huvuddokumentet, inte fotnoterna).
    track: bool,
    /// Författare och datum per spårad ändring (numret är `data-k`).
    revs: Vec<RevRec>,
    /// Kommentarer som har ett område i texten.
    comment_ids: Vec<u32>,
}

pub fn read(bytes: &[u8]) -> Result<Loaded, String> {
    if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Err(
            crate::i18n::t("Dokumentet är lösenordsskyddat eller sparat i det gamla .doc-formatet. Öppna det i Word och spara som .docx utan lösenord.")
                .into(),
        );
    }
    if !bytes.starts_with(b"PK") {
        return Err(crate::i18n::t("Filen är inte ett Word-dokument (.docx).").into());
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(crate::i18n::t("Word-dokumentet är för stort för att öppnas.").into());
    }
    check_entry_count(bytes)?;
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| crate::tf!("Word-dokumentet är skadat: {e}", e = e))?;
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
        return Err(crate::i18n::t("Word-dokumentet saknar sitt huvudinnehåll.").into());
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
    let mut style_set = style::StyleSet::default();
    let styles = match find_part("/styles") {
        Some(p) => pkg
            .part(&p)?
            .map(|x| {
                let kladd = format!("<w:styles>{}</w:styles>", kladd_styles());
                style_set = crate::docx_styles::read_relative(&x, &|t| theme.font(t), &kladd);
                parse_styles(&x, &theme)
            })
            .unwrap_or_default(),
        None => Styles::default(),
    };
    let numbering = match find_part("/numbering") {
        Some(p) => pkg.part(&p)?.map(|x| parse_numbering(&x)).unwrap_or_default(),
        None => Numbering::default(),
    };
    let mut dropped = Dropped::default();

    let mut found: Vec<FoundImage> = Vec::new();
    let mut info = BodyInfo { track: true, ..Default::default() };
    let mut blocks = parse_body(&doc_xml, &styles, &theme, &numbering, &rels, &mut dropped, &mut found, dir, &mut info)?;
    // Kommentarerna: posterna läggs sist i texten, och bara de som har ett område.
    let comments = match find_part("/comments") {
        Some(name) => {
            let ext = review_xml::ext_rel_target(&rels, dir);
            review_xml::read_comments(&mut pkg, &name, ext.as_deref())?
        }
        None => None,
    };

    // Fotnoterna: texten läses ur footnotes.xml och läggs som definitioner sist i dokumentet.
    let footnotes = match find_part("/footnotes") {
        Some(name) => read_footnotes(&mut pkg, &name, &styles, &theme, &numbering)?,
        None => None,
    };

    // Sida, sidhuvud och sidfot ur första avsnittet (de övriga avsnitten blir avsnittsrader).
    let page = match info.sects.first().or(info.sect_pr.as_ref()) {
        Some(sect) => read_page(sect, &mut pkg, &rels, dir)?,
        None => DocDefaults::default(),
    };
    let mut all_sects: Vec<&str> = info.sects.iter().map(String::as_str).collect();
    all_sects.extend(info.sect_pr.as_deref());
    fill_section_rows(&mut blocks, &all_sects, &page);

    // Packa upp bilderna som dokumentet faktiskt använder.
    let mut images: Vec<(String, Image)> = Vec::new();
    let mut by_target: HashMap<String, String> = HashMap::new();
    let text_w = {
        let (w, _) = page.page();
        let m = page.margins();
        Some(((w - m[1] - m[3]) * 20.0) as f64)
    };
    let (mut markdown, units) = to_markdown_units(&blocks, &numbering, text_w);
    let mut keys: Vec<String> = units.iter().map(|u| u.key.clone()).collect();
    // Grundformatet (typsnitt, storlek och sida) står först när det avviker från standard.
    let mut base = styles.base();
    base.page_size = page.page_size;
    base.page_margin = page.page_margin;
    base.header = page.header.clone();
    base.header_align = page.header_align;
    base.footer = page.footer.clone();
    base.footer_align = page.footer_align;
    base.header_first = page.header_first.clone();
    base.footer_first = page.footer_first.clone();
    base.styles = style_set;
    if !info.fn_refs.is_empty() {
        markdown.push('\n');
        for (i, r) in info.fn_refs.iter().enumerate() {
            let md = footnotes
                .as_ref()
                .and_then(|f| f.items.iter().find(|x| x.id == r.id && x.ty.is_none()))
                .map_or("", |x| x.md.as_str());
            markdown.push_str(&format!("[^{}]: {md}\n", i + 1));
        }
    }
    let mut comments = comments;
    let mut comment_recs: Vec<CommentRec> = Vec::new();
    if let Some((part, recs)) = comments.as_mut() {
        comment_recs = recs.iter().filter(|c| info.comment_ids.contains(&c.id)).cloned().collect();
        part.items.retain(|i| info.comment_ids.contains(&i.id));
    }
    if !info.revs.is_empty() || !comment_recs.is_empty() {
        // Poster utan tagg eller markör (t.ex. ett tomt stycke med spårat stycketecken) skrivs inte.
        let sc = review::scan(&markdown);
        let used: std::collections::HashSet<u32> = sc.revs.iter().map(|r| r.id).chain(sc.pmarks.iter().map(|m| m.id)).collect();
        info.revs.retain(|r| used.contains(&r.id));
        markdown = review::put_records(&markdown, &info.revs, &comment_recs);
    }
    let comments = comments.map(|(p, _)| p);
    let comment = base.to_comment();
    // Formatmallarna som `write` får tillbaka ur kommentarraden (avrundade som i texten).
    let read_styles = DocDefaults::from_comment(&comment).map(|d| d.styles).unwrap_or_default();
    if !comment.is_empty() {
        markdown = format!("{comment}\n\n{markdown}");
    }
    for f in found {
        let name = if let Some(n) = by_target.get(&f.target) {
            Some(n.clone())
        } else {
            match pkg.part_bytes(&f.target)?.filter(|b| is_supported_image(b)) {
                None => None,
                Some(bytes) => {
                    let base = f.target.rsplit('/').next().unwrap_or("bild").to_owned();
                    let mut name = base.clone();
                    let mut n = 2;
                    while images.iter().any(|(m, _)| *m == name) {
                        name = format!("{n}-{base}");
                        n += 1;
                    }
                    images.push((name.clone(), Image { bytes, extent: f.extent }));
                    by_target.insert(f.target.clone(), name.clone());
                    Some(name)
                }
            }
        };
        let repl = match name {
            Some(name) => {
                let img = images.iter().find(|(n, _)| *n == name).map(|(_, i)| i);
                let float = f.anchor.as_ref().and_then(|a| anchor_float(a, f.extent, &page));
                image_md(&name, &f.alt, f.extent, img, float)
            }
            // Bilden går inte att visa: originalkörningen bevaras som markör.
            None => f.fallback.map(token_text).unwrap_or_default(),
        };
        markdown = markdown.replace(&f.placeholder, &repl);
        for k in keys.iter_mut().filter(|k| k.contains(&f.placeholder)) {
            *k = k.replace(&f.placeholder, &repl);
        }
    }

    let source = collect_parts(&mut pkg).and_then(|parts| {
        // Delen får det namn den har i paketet (skiftläge kan skilja).
        let real = parts.iter().find(|(n, _)| n.trim_start_matches('/').eq_ignore_ascii_case(&main)).map(|(n, _)| n.clone())?;
        if !info.head.contains("<w:document") {
            return None;
        }
        let mut elems: Vec<SrcElem> = info.elems.iter().map(|x| SrcElem { xml: x.clone(), units: Vec::new(), fn_ids: Vec::new() }).collect();
        for (u, key) in units.iter().zip(keys) {
            if let Some(e) = elems.get_mut(u.origin) {
                e.units.push((u.desc.clone(), key));
            }
        }
        for r in &info.fn_refs {
            if let Some(e) = elems.get_mut(r.origin) {
                e.fn_ids.push(r.id.clone());
            }
        }
        let first_refs = info.sects.first().or(info.sect_pr.as_ref()).map(|x| header_refs(x)).unwrap_or_default();
        Some(Source { parts, main: real, head: info.head, tail: info.tail, sect_pr: info.sect_pr, elems, tokens: info.tokens, footnotes, page, styles: read_styles, comments, first_refs })
    });
    Ok(Loaded { source, markdown, images, dropped: dropped.list() })
}

/// Delar upp footnotes.xml: allt före och efter, och varje `<w:footnote>` med id, typ, hela
/// elementet och innehållets intervall (utan start- och sluttaggen).
fn split_footnotes(xml: &str) -> Option<(String, Vec<(String, Option<String>, String, std::ops::Range<usize>)>, String)> {
    let mut r = Reader::from_str(xml);
    let mut depth = 0usize;
    let mut cur: Option<(usize, usize, String, Option<String>)> = None; // (start, inner start, id, typ)
    let mut items: Vec<(String, Option<String>, String, std::ops::Range<usize>)> = Vec::new();
    let mut first: Option<usize> = None;
    let mut last_end = 0usize;
    let mut close_root: Option<usize> = None;
    loop {
        let before = r.buffer_position() as usize;
        let ev = r.read_event().ok()?;
        let after = r.buffer_position() as usize;
        match &ev {
            Event::Start(e) => {
                if depth == 1 && local(e.name().as_ref()) == b"footnote" {
                    first.get_or_insert(before);
                    cur = Some((before, after, attr(e, b"id").unwrap_or_default(), attr(e, b"type")));
                }
                depth += 1;
            }
            Event::Empty(e) => {
                if depth == 1 && local(e.name().as_ref()) == b"footnote" {
                    first.get_or_insert(before);
                    items.push((attr(e, b"id").unwrap_or_default(), attr(e, b"type"), xml[before..after].to_owned(), after..after));
                    last_end = after;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 1 {
                    if let Some((start, inner, id, ty)) = cur.take() {
                        items.push((id, ty, xml[start..after].to_owned(), inner..before));
                        last_end = after;
                    }
                } else if depth == 0 {
                    close_root = Some(before);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let close = close_root?;
    let head = xml[..first.unwrap_or(close)].to_owned();
    let tail = xml[if first.is_some() { last_end } else { close }..].to_owned();
    Some((head, items, tail))
}

/// Fotnotens text som Kladd-Markdown på en rad: stycken skilda med `<br>`, utan markörer.
fn footnote_md(
    inner: &str,
    styles: &Styles,
    theme: &Theme,
    numbering: &Numbering,
    rels: &HashMap<String, (String, String, bool)>,
) -> String {
    let doc = format!("<w:document><w:body>{inner}</w:body></w:document>");
    // Fotnotstexten jämförs med fotnotsmallens egna typsnitt och storlek (inte brödtextens), så att
    // den inte fylls med `<span style>`; visningen gör den mindre än brödtexten av sig själv.
    let mut styles = styles.clone();
    if let Some(i) = inner.find("<w:pStyle w:val=\"") {
        let v = &inner[i + 17..];
        if let Some(e) = v.find('"') {
            styles.default_para = v[..e].to_owned();
        }
    }
    let mut dropped = Dropped::default();
    let mut found = Vec::new();
    let mut info = BodyInfo::default();
    let blocks = parse_body(&doc, &styles, theme, numbering, rels, &mut dropped, &mut found, "word", &mut info).unwrap_or_default();
    let mut paras: Vec<String> = Vec::new();
    for b in &blocks {
        if let Block::Para(p) = b {
            let mut t = without_tokens(&para_md(p));
            // Bilder i fotnoter visas inte.
            while let (Some(a), Some(z)) = (t.find('\u{E000}'), t.find('\u{E001}')) {
                if z < a {
                    break;
                }
                t.replace_range(a..z + '\u{E001}'.len_utf8(), "");
            }
            let t = t.replace('\n', "<br>");
            if !t.trim().is_empty() {
                paras.push(t.trim().to_owned());
            }
        }
    }
    let joined = paras.join("<br>");
    escape_line_start(&joined)
}

fn read_footnotes(pkg: &mut Package<'_>, name: &str, styles: &Styles, theme: &Theme, numbering: &Numbering) -> Result<Option<FnPart>, String> {
    let Some(xml) = pkg.part(name)? else { return Ok(None) };
    let Some((head, raw, tail)) = split_footnotes(&xml) else { return Ok(None) };
    // Hyperlänkar i fotnoterna pekar på footnotes.xml:s egna relationer.
    let (dir, file) = name.rsplit_once('/').unwrap_or(("", name));
    let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
    let rels = pkg.part(&rels_name)?.map(|x| parse_rels(&x)).unwrap_or_default();
    let items = raw
        .into_iter()
        .map(|(id, ty, xml_el, inner)| {
            let md = if ty.is_none() { footnote_md(&xml[inner], styles, theme, numbering, &rels) } else { String::new() };
            FnOrig { id, ty, md, xml: xml_el }
        })
        .collect();
    Ok(Some(FnPart { name: name.to_owned(), head, tail, items }))
}

/// Hur en bild skrivs i texten: `![alt](bild:namn)`, eller `<img … width>` när storleken avviker
/// från bildens egen (96 dpi) eller från den som redan lagrats för bilden.
fn image_md(name: &str, alt: &str, extent: Option<(i64, i64)>, img: Option<&Image>, float: Option<Float>) -> String {
    let differs = |a: f64, b: f64| b > 0.0 && (a - b).abs() / b > 0.02;
    let want = extent.map(|(cx, _)| cx as f64 / EMU_PER_PIXEL as f64);
    let natural = img.and_then(|i| imagesize::blob_size(&i.bytes).ok()).map(|s| s.width as f64);
    let stored = img.and_then(|i| i.extent).map(|(cx, _)| cx as f64 / EMU_PER_PIXEL as f64);
    let src = format!("{IMAGE_SCHEME}{}", name.replace(' ', "%20"));
    // En flytande bild skrivs alltid som `<img … width align>`.
    let want = if float.is_some() { want.or(natural) } else { want };
    match want {
        Some(w) if float.is_some() || natural.is_some_and(|n| differs(w, n)) || stored.is_some_and(|st| differs(w, st)) => {
            let mut a = String::new();
            for c in alt.chars() {
                match c {
                    '&' => a.push_str("&amp;"),
                    '"' => a.push_str("&quot;"),
                    '<' => a.push_str("&lt;"),
                    '>' => a.push_str("&gt;"),
                    '\n' | '\r' => a.push(' '),
                    c => a.push(c),
                }
            }
            let al = match float {
                Some(Float::Left) => " align=\"left\"",
                Some(Float::Right) => " align=\"right\"",
                None => "",
            };
            format!("<img src=\"{src}\" alt=\"{a}\" width=\"{}\"{al}>", w.round().max(1.0) as i64)
        }
        _ => format!("![{}]({src})", escape_alt(alt)),
    }
}

/// Alla poster i paketet i ordning, eller `None` om de blir för stora.
fn collect_parts(pkg: &mut Package<'_>) -> Option<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut total = 0u64;
    for i in 0..pkg.zip.len() {
        let mut f = pkg.zip.by_index(i).ok()?;
        if f.is_dir() {
            continue;
        }
        total += f.size();
        if f.size() > MAX_PART_BYTES || total > MAX_TOTAL_BYTES {
            return None;
        }
        let name = f.name().to_owned();
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.by_ref().take(MAX_PART_BYTES + 1).read_to_end(&mut buf).ok()?;
        if buf.len() as u64 > MAX_PART_BYTES {
            return None;
        }
        out.push((name, buf));
    }
    Some(out)
}

/// Sida, spalter och avsnittstyp ur en `w:sectPr` (utan sidhuvud och sidfot).
#[derive(Debug, Clone, PartialEq)]
struct RawSect {
    size: Option<(f32, f32)>,
    margin: Option<[f32; 4]>,
    cols: usize,
    /// Mellanrum mellan spalter i pt.
    gap: Option<f32>,
    continuous: bool,
}

fn read_sect(sect: &str) -> RawSect {
    let mut o = RawSect { size: None, margin: None, cols: 1, gap: None, continuous: false };
    let mut mar = [style::DEFAULT_MARGIN; 4];
    let mut r = Reader::from_str(sect);
    let tw = |e: &BytesStart<'_>, n: &[u8]| attr(e, n).and_then(|v| v.parse::<f32>().ok());
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => match local(e.name().as_ref()) {
                b"pgSz" => {
                    if let (Some(w), Some(h)) = (tw(&e, b"w"), tw(&e, b"h")) {
                        if w > 0.0 && h > 0.0 {
                            let (w, h) = (w / 20.0, h / 20.0);
                            let land = attr(&e, b"orient").as_deref() == Some("landscape");
                            o.size = Some(if land && w < h { (h, w) } else { (w, h) });
                        }
                    }
                }
                b"pgMar" => {
                    for (i, n) in [b"top".as_slice(), b"right", b"bottom", b"left"].into_iter().enumerate() {
                        if let Some(v) = tw(&e, n) {
                            mar[i] = (v / 20.0).max(0.0);
                            o.margin = Some(mar);
                        }
                    }
                }
                b"cols" => {
                    o.cols = attr(&e, b"num").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, sections::MAX_COLUMNS);
                    o.gap = tw(&e, b"space").map(|v| v / 20.0);
                }
                // Olika breda spalter: antalet räknas, mellanrummet tas från första spalten.
                b"col" => {
                    if o.gap.is_none() {
                        o.gap = tw(&e, b"space").map(|v| v / 20.0);
                    }
                }
                b"type" => o.continuous = attr(&e, b"val").as_deref() == Some("continuous"),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    o
}

/// Avsnittsradens egenskaper för ett avsnitt vars `w:sectPr` är `sect`, mot dokumentets grundformat `d`.
fn section_props(sect: Option<&str>, d: &DocDefaults) -> sections::SectionProps {
    let Some(sect) = sect else { return sections::SectionProps::default() };
    let raw = read_sect(sect);
    let mut p = sections::SectionProps::default();
    p.page_size = raw.size.filter(|s| !style::same_size(*s, d.page()));
    p.page_margin = raw.margin.filter(|m| m.iter().zip(d.margins()).any(|(a, b)| (a - b).abs() > 0.6));
    if raw.cols > 1 {
        p.columns = Some(raw.cols);
        p.column_gap = raw.gap.filter(|g| (g - sections::DEFAULT_GAP).abs() > 0.5);
    }
    p.continuous = raw.continuous;
    p
}

/// Byter platshållarna i avsnittsraderna mot raderna med respektive avsnitts egenskaper.
/// `sects[n]` är `w:sectPr` för avsnitt n (0 = första); rad n börjar avsnitt n + 1.
fn fill_section_rows(blocks: &mut [Block], sects: &[&str], d: &DocDefaults) {
    for b in blocks {
        let Block::Para(p) = b else { continue };
        let [run] = p.runs.as_mut_slice() else { continue };
        let Some(n) = run.text.strip_prefix('\u{E010}').and_then(|r| r.strip_suffix('\u{E011}')).and_then(|r| r.parse::<usize>().ok())
        else {
            continue;
        };
        run.text = section_props(sects.get(n + 1).copied(), d).to_comment();
    }
}

/// Sidstorlek, marginaler, sidhuvud och sidfot ur en `w:sectPr`.
fn read_page(sect: &str, pkg: &mut Package<'_>, rels: &HashMap<String, (String, String, bool)>, dir: &str) -> Result<DocDefaults, String> {
    let mut d = DocDefaults::default();
    let mut size: Option<(f32, f32)> = None;
    let mut mar = [style::DEFAULT_MARGIN; 4];
    let mut has_mar = false;
    let mut hdr: Option<String> = None;
    let mut ftr: Option<String> = None;
    let mut hdr_first: Option<String> = None;
    let mut ftr_first: Option<String> = None;
    let mut title_pg = false;
    let mut r = Reader::from_str(sect);
    let twips = |e: &BytesStart<'_>, n: &[u8]| attr(e, n).and_then(|v| v.parse::<f32>().ok());
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => match local(e.name().as_ref()) {
                b"pgSz" => {
                    if let (Some(w), Some(h)) = (twips(&e, b"w"), twips(&e, b"h")) {
                        if w > 0.0 && h > 0.0 {
                            let (w, h) = (w / 20.0, h / 20.0);
                            // `w:orient` utan att måtten är vända: vänd dem.
                            let land = attr(&e, b"orient").as_deref() == Some("landscape");
                            size = Some(if land && w < h { (h, w) } else { (w, h) });
                        }
                    }
                }
                b"pgMar" => {
                    for (i, n) in [b"top".as_slice(), b"right", b"bottom", b"left"].into_iter().enumerate() {
                        if let Some(v) = twips(&e, n) {
                            mar[i] = (v / 20.0).max(0.0);
                            has_mar = true;
                        }
                    }
                }
                b"headerReference" | b"footerReference" if matches!(attr(&e, b"type").as_deref(), Some("default" | "first")) => {
                    let id = attr(&e, b"id");
                    let (header, first) = (local(e.name().as_ref()) == b"headerReference", attr(&e, b"type").as_deref() == Some("first"));
                    match (header, first) {
                        (true, false) => hdr = id,
                        (true, true) => hdr_first = id,
                        (false, false) => ftr = id,
                        (false, true) => ftr_first = id,
                    }
                }
                b"titlePg" => title_pg = !matches!(attr(&e, b"val").as_deref(), Some("0" | "false" | "off")),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    d.page_size = size;
    d.page_margin = has_mar.then_some(mar);
    for (id, header) in [(hdr, true), (ftr, false)] {
        let Some((target, _, false)) = id.and_then(|i| rels.get(&i)) else { continue };
        let Some(xml) = pkg.part(&resolve_target(dir, target))? else { continue };
        if let Some((text, align)) = hf_text(&xml) {
            if header {
                d.header = Some(text);
                d.header_align = align;
            } else {
                d.footer = Some(text);
                d.footer_align = align;
            }
        }
    }
    // Annorlunda första sida (`w:titlePg`): en saknad eller oläslig del blir en tom första sida.
    if title_pg {
        for (id, header) in [(hdr_first, true), (ftr_first, false)] {
            let mut found = None;
            if let Some((target, _, false)) = id.and_then(|i| rels.get(&i)) {
                if let Some(xml) = pkg.part(&resolve_target(dir, target))? {
                    found = hf_text(&xml);
                }
            }
            let text = found.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
            if header {
                d.header_first = Some(text);
                if d.header.is_none() {
                    d.header_align = found.map_or(Align::Left, |(_, a)| a);
                }
            } else {
                d.footer_first = Some(text);
                if d.footer.is_none() {
                    d.footer_align = found.map_or(Align::Left, |(_, a)| a);
                }
            }
        }
    }
    Ok(d)
}

/// `{sida}` för PAGE, `{sidor}` för NUMPAGES/SECTIONPAGES, `{datum}` för DATE/TIME, `{titel}` för
/// TITLE och `{fil}` för FILENAME.
fn field_placeholder(instr: &str) -> Option<&'static str> {
    match instr.split_whitespace().next().map(str::to_ascii_uppercase).as_deref() {
        Some("PAGE") => Some("{sida}"),
        Some("NUMPAGES" | "SECTIONPAGES") => Some("{sidor}"),
        Some("DATE" | "TIME") => Some("{datum}"),
        Some("TITLE") => Some("{titel}"),
        Some("FILENAME") => Some("{fil}"),
        _ => None,
    }
}

/// Texten i ett sidhuvud eller en sidfot: styckenas text sammanfogad med mellanslag, fält som
/// `{sida}`/`{sidor}`/`{datum}`/`{titel}`/`{fil}`, och det första styckets justering. Ett enda
/// stycke med en eller två flikar blir vänster|mitten|höger (`a|b|c`); `|` i texten skrivs `\|`.
fn hf_text(xml: &str) -> Option<(String, Align)> {
    let mut r = Reader::from_str(xml);
    let mut paras: Vec<String> = Vec::new();
    // Styckenas delar uppdelade vid flikar, och det senaste styckets egna tabbstopp.
    let mut para_segs: Vec<Vec<String>> = Vec::new();
    let mut segs: Vec<String> = Vec::new();
    let mut tab_stops: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut align: Option<Align> = None;
    let mut seen_p = false;
    let mut in_p = false;
    let mut in_ppr = false;
    let mut in_t = false;
    let mut in_instr = false;
    // Komplexa fält: (kod, i resultatet, ersättning).
    let mut fields: Vec<(String, bool, Option<&'static str>)> = Vec::new();
    let mut simples: Vec<bool> = Vec::new();
    loop {
        let ev = r.read_event().ok()?;
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let empty = matches!(ev, Event::Empty(_));
                match local(e.name().as_ref()) {
                    b"p" if !empty => {
                        in_p = true;
                        cur.clear();
                        segs.clear();
                        tab_stops.clear();
                        if !seen_p {
                            seen_p = true;
                        }
                    }
                    b"pPr" => in_ppr = !empty,
                    b"jc" if in_ppr && align.is_none() && paras.is_empty() => align = attr(e, b"val").map(|v| Align::from_jc(&v)),
                    b"t" => in_t = !empty,
                    b"instrText" => in_instr = !empty,
                    b"tab" if in_p && !in_ppr => segs.push(std::mem::take(&mut cur)),
                    b"tab" if in_ppr => {
                        if let Some(v) = attr(e, b"val").filter(|v| v != "clear") {
                            tab_stops.push(v);
                        }
                    }
                    b"fldChar" => match attr(e, b"fldCharType").as_deref() {
                        Some("begin") => fields.push((String::new(), false, None)),
                        Some("separate") => {
                            if let Some(f) = fields.last_mut() {
                                f.1 = true;
                                f.2 = field_placeholder(&f.0);
                                if let Some(ph) = f.2 {
                                    cur.push_str(ph);
                                }
                            }
                        }
                        Some("end") => {
                            fields.pop();
                        }
                        _ => {}
                    },
                    b"fldSimple" => {
                        let ph = attr(e, b"instr").and_then(|i| field_placeholder(&i));
                        if let Some(ph) = ph {
                            cur.push_str(ph);
                        }
                        if !empty {
                            simples.push(ph.is_some());
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => match local(e.name().as_ref()) {
                b"p" => {
                    if in_p {
                        segs.push(std::mem::take(&mut cur));
                        let parts: Vec<String> = segs.iter().map(|s| s.trim().replace('|', "\\|")).collect();
                        let t = parts.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
                        if !t.is_empty() {
                            paras.push(t);
                            // Två delar utan egna tabbstopp: Words standard är mitt och höger.
                            let mut p = parts;
                            if p.len() == 2 {
                                let right = tab_stops.len() == 1 && tab_stops[0] == "right";
                                p = if right { vec![p[0].clone(), String::new(), p[1].clone()] } else { vec![p[0].clone(), p[1].clone(), String::new()] };
                            }
                            para_segs.push(p);
                        } else if paras.is_empty() {
                            // Ett tomt första stycke ger ingen justering.
                            align = None;
                        }
                        in_p = false;
                    }
                }
                b"pPr" => in_ppr = false,
                b"t" => in_t = false,
                b"instrText" => in_instr = false,
                b"fldSimple" => {
                    simples.pop();
                }
                _ => {}
            },
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) => {
                let t = text_of(&ev);
                if in_instr {
                    if let Some(f) = fields.last_mut() {
                        f.0.push_str(&t);
                    }
                } else if in_t && in_p {
                    let hidden = fields.iter().any(|f| !f.1) || fields.iter().any(|f| f.1 && f.2.is_some()) || simples.iter().any(|&x| x);
                    if !hidden {
                        cur.push_str(&t.replace(['\n', '\r'], " "));
                    }
                }
            }
            _ => {}
        }
    }
    if let [p] = para_segs.as_slice() {
        if p.len() == 3 {
            return Some((p.join("|"), Align::Left));
        }
    }
    let text = paras.join(" ");
    (!text.is_empty()).then(|| (text, align.unwrap_or(Align::Left)))
}

/// En bild som hittats i texten; platshållaren byts mot `![…](bild:…)` när bilden lästs.
struct FoundImage {
    placeholder: String,
    target: String,
    alt: String,
    extent: Option<(i64, i64)>,
    /// Förankrad bild med omflöde (`wp:anchor` + `wrapSquare/Tight/Through`): läget i sidled.
    anchor: Option<AnchorPos>,
    /// Markör för hela körningen, om bilden inte går att visa.
    fallback: Option<usize>,
}

/// `wp:positionH` för en förankrad bild med textomflöde.
#[derive(Default, Clone)]
struct AnchorPos {
    /// `relativeFrom` ("column", "margin", "page" …).
    rel: String,
    /// `wp:align`: left, right, center, inside, outside.
    align: Option<String>,
    /// `wp:posOffset` i EMU.
    off: Option<i64>,
}

/// Sidan en förankrad bild flyter på: `align` left/right, eller en förskjutning nära vänster
/// respektive höger marginal. Mitt på raden, "inside/outside" och okända lägen ger `None`
/// (bilden blir en egen rad).
fn anchor_float(a: &AnchorPos, extent: Option<(i64, i64)>, page: &DocDefaults) -> Option<Float> {
    match a.align.as_deref() {
        Some("left") => return Some(Float::Left),
        Some("right") => return Some(Float::Right),
        Some(_) => return None,
        None => {}
    }
    let off = a.off? as f64;
    let (pw, _) = page.page();
    let [_, r, _, l] = page.margins();
    let (l, text_w) = (l as f64 * 12700.0, (pw - l - r) as f64 * 12700.0);
    let w = extent.map_or(0.0, |e| e.0 as f64);
    // Bildens vänsterkant räknad från textspaltens vänsterkant.
    let left = match a.rel.as_str() {
        "page" => off - l,
        "column" | "margin" | "leftMargin" | "character" | "" => off,
        _ => return None,
    };
    if left <= 0.25 * text_w {
        Some(Float::Left)
    } else if left + w >= 0.75 * text_w {
        Some(Float::Right)
    } else {
        None
    }
}

/// Lägger originalets XML i listan över okända objekt och ger dess nummer.
fn new_token(info: &mut BodyInfo, xml: &str) -> usize {
    let begins = xml.matches("w:fldCharType=\"begin\"").count() as i32;
    let ends = xml.matches("w:fldCharType=\"end\"").count() as i32;
    info.tokens.push(Opaque { xml: xml.to_owned(), fld: begins - ends });
    info.tokens.len() - 1
}

/// Texten som står i stycket för ett okänt objekt.
fn token_text(n: usize) -> String {
    format!("<!--k:{n}-->")
}

/// Lägger en markör som en rå körning i stycket.
fn push_token(para: &mut Option<Para>, info: &mut BodyInfo, xml: &str) {
    let n = new_token(info, xml);
    let p = para.get_or_insert_with(Para::new);
    p.runs.push(Run { text: token_text(n), fmt: Fmt::default(), link: None, raw: true });
}

#[derive(Default)]
struct Drawing {
    depth: usize,
    blip: Option<String>,
    linked: bool,
    alt: String,
    title: String,
    extent: Option<(i64, i64)>,
    /// `wp:anchor` (inte `wp:inline`).
    anchored: bool,
    /// `wrapSquare`, `wrapTight` eller `wrapThrough`: texten flyter runt bilden.
    wraps: bool,
    pos: AnchorPos,
    in_pos_h: bool,
    /// Vad nästa text är: 1 = `wp:align`, 2 = `wp:posOffset`.
    text_of: u8,
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

/// Slutet (exklusivt) på elementet som börjar vid `from` i `xml`.
/// Ett stycke med bara en rad råtext (Kladds egna markörer: avsnitt, spaltbrytning).
fn raw_para(text: String, origin: usize) -> Para {
    let mut p = Para::new();
    p.runs.push(Run { text, fmt: Fmt::default(), link: None, raw: true });
    p.origin = origin;
    p
}

/// Platshållare för avsnittsraden n; fylls i när alla avsnitts egenskaper lästs.
fn section_placeholder(n: usize) -> String {
    format!("\u{E010}{n}\u{E011}")
}

fn element_end(xml: &str, from: usize) -> Option<usize> {
    let mut r = Reader::from_str(&xml[from..]);
    let mut depth = 0usize;
    loop {
        match r.read_event().ok()? {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(from + r.buffer_position() as usize);
                }
            }
            Event::Empty(_) if depth == 0 => return Some(from + r.buffer_position() as usize),
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// Antal fältstarter minus fältslut i en bit XML.
fn field_balance(xml: &str) -> i32 {
    xml.matches("w:fldCharType=\"begin\"").count() as i32 - xml.matches("w:fldCharType=\"end\"").count() as i32
}

/// Är elementet en innehållsförteckning? Words innehållskontroll med galleriet "Table of Contents"
/// eller ett fält vars instruktion börjar med `TOC`.
fn is_toc_xml(xml: &str) -> bool {
    if xml.contains("docPartGallery") && xml.contains("Table of Contents") {
        return true;
    }
    let mut rest = xml;
    while let Some(i) = rest.find("instrText") {
        let after = &rest[i..];
        let Some(gt) = after.find('>') else { break };
        let body = &after[gt + 1..];
        let end = body.find('<').unwrap_or(body.len());
        let instr = body[..end].trim_start();
        if instr.len() >= 3 && instr[..3].eq_ignore_ascii_case("TOC") && !instr[3..].starts_with(|c: char| c.is_ascii_alphanumeric()) {
            return true;
        }
        rest = &body[end..];
    }
    xml.contains("w:instr=\" TOC") || xml.contains("w:instr=\"TOC")
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
    info: &mut BodyInfo,
) -> Result<Vec<Block>, String> {
    let _ = numbering;
    let mut drawing: Option<Drawing> = None;
    let mut r = Reader::from_str(xml);
    let mut blocks: Vec<Block> = Vec::new();

    let mut para: Option<Para> = None;
    // Avsnittsslut (`w:sectPr` i `w:pPr`) och spaltbrytning i stycket som läses.
    let mut para_sect: Option<String> = None;
    let mut para_col_break = false;
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
    let mut tables: Vec<Tbl> = Vec::new();
    let mut in_ppr = false;
    let mut alt_depth: Vec<bool> = Vec::new(); // AlternateContent: har Choice använts?
    // Originalets XML: djup i trädet, toppnivåelementets start och körningen som läses.
    let mut depth = 0usize;
    let mut top_start = 0usize;
    let mut top_is_sect = false;
    let mut in_run = false;
    let mut run_start = 0usize;
    let mut run_opaque = false;
    let mut run_fnref: Option<String> = None;
    // Hur långt stycket var när körningen började, så att den kan backas om den blir en markör.
    let mut run_mark = (0usize, 0usize);
    let mut run_found_mark = 0usize;
    let mut omath_start: Option<usize> = None;
    let mut simple_fields: Vec<()> = Vec::new();
    // Spårade ändringar: öppna `w:ins`/`w:del` och den som gäller just nu.
    let mut rev_stack: Vec<Option<(RevKind, u32)>> = Vec::new();
    let mut cur_rev: Option<(RevKind, u32)> = None;
    // Körningen är bara en kommentarsreferens: den ersätts av områdets slutmarkör.
    let mut run_drop = false;
    // Spårade stycketecken (`w:ins`/`w:del` i styckets `w:rPr`) och styckeformatering (`w:pPrChange`).
    let mut para_marks: Vec<(RevKind, u32)> = Vec::new();
    let mut para_fchange: Option<u32> = None;
    // Bara dokument som nämner en innehållsförteckning behöver sökas igenom efter en.
    let toc_hint = xml.contains("Table of Contents") || xml.contains("TOC");

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
        let before = r.buffer_position() as usize;
        let ev = r.read_event().map_err(|e| crate::tf!("Word-dokumentet innehåller trasig XML: {e}", e = e))?;
        let after = r.buffer_position() as usize;
        // Toppnivåelementen i w:body sparas som de är.
        match &ev {
            Event::Start(e) => {
                if info.body_depth == Some(depth) {
                    top_start = before;
                    top_is_sect = local(e.name().as_ref()) == b"sectPr";
                    if !top_is_sect {
                        info.elems.push(String::new());
                    }
                }
                depth += 1;
                if info.body_depth.is_none() && local(e.name().as_ref()) == b"body" {
                    info.body_depth = Some(depth);
                    info.head = xml[..after].to_owned();
                }
            }
            Event::Empty(e) => {
                if info.body_depth == Some(depth) {
                    if local(e.name().as_ref()) == b"sectPr" {
                        info.sect_pr = Some(xml[before..after].to_owned());
                    } else {
                        info.elems.push(xml[before..after].to_owned());
                    }
                }
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                if info.body_depth == Some(depth) {
                    let slice = xml[top_start..after].to_owned();
                    if top_is_sect {
                        info.sect_pr = Some(slice);
                    } else if let Some(last) = info.elems.last_mut() {
                        *last = slice;
                    }
                } else if info.body_depth == Some(depth + 1) && local(e.name().as_ref()) == b"body" {
                    info.tail = xml[before..].to_owned();
                }
            }
            _ => {}
        }
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
                // Innehållsförteckning på toppnivå (innehållskontroll eller ett fält i ett stycke):
                // hela elementet blir raden `<!-- kladd: toc -->`; originalet följer med orört.
                if toc_hint
                    && !empty
                    && drawing.is_none()
                    && tables.is_empty()
                    && para.is_none()
                    && info.body_depth.is_some_and(|b| depth == b + 1)
                    && matches!(name.as_slice(), b"sdt" | b"p")
                {
                    if let Some(end) = element_end(xml, before) {
                        let slice = &xml[before..end];
                        if is_toc_xml(slice) && (name == b"sdt" || field_balance(slice) == 0) {
                            let mut p = Para::new();
                            p.runs.push(Run { text: crate::word::nav::TOC_LINE.to_owned(), fmt: Fmt::default(), link: None, raw: true });
                            p.origin = info.elems.len().wrapping_sub(1);
                            blocks.push(Block::Para(p));
                            skip = 1;
                            continue;
                        }
                    }
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
                            if !empty {
                                d.depth -= 1;
                                skip = 1;
                            }
                        }
                        b"anchor" => d.anchored = true,
                        b"wrapSquare" | b"wrapTight" | b"wrapThrough" => d.wraps = true,
                        b"positionH" => {
                            d.pos.rel = attr(e, b"relativeFrom").unwrap_or_default();
                            d.in_pos_h = !empty;
                        }
                        b"align" if d.in_pos_h && !empty => d.text_of = 1,
                        b"posOffset" if d.in_pos_h && !empty => d.text_of = 2,
                        _ => {}
                    }
                    continue;
                }
                if in_rpr {
                    apply_rpr_elem(&mut direct, &name, e, theme);
                }
                // Objekt Kladd inte visar: hela körningen blir en markör när den tar slut.
                if in_run && name == b"commentReference" && info.track && attr(e, b"id").is_some() {
                    run_drop = true;
                } else if in_run
                    && matches!(
                        name.as_slice(),
                        b"endnoteReference" | b"commentReference" | b"fldChar" | b"instrText" | b"sym" | b"object"
                    )
                {
                    run_opaque = true;
                }
                match name.as_slice() {
                    // Fotnotsreferens: blir `[^N]` i texten (anpassad hänvisning och dolda fält förblir markörer).
                    b"footnoteReference" if in_run => match attr(e, b"id") {
                        Some(id) if attr(e, b"customMarkFollows").is_none() && !hidden(&field_depth) => run_fnref = Some(id),
                        _ => run_opaque = true,
                    },
                    // Bilder: samla ihop till ett `![…](bild:…)` när elementet tar slut.
                    b"drawing" | b"pict" => {
                        if !empty {
                            drawing = Some(Drawing { depth: 1, ..Default::default() });
                        } else {
                            run_opaque = true;
                        }
                    }
                    b"object" => {
                        if !empty {
                            skip = 1;
                        }
                    }
                    b"txbxContent" => {
                        run_opaque = true;
                        if !empty {
                            skip = 1;
                        }
                    }
                    b"oMath" | b"oMathPara" => {
                        if !empty {
                            skip = 1;
                            omath_start = Some(before);
                        }
                    }
                    // Bokmärken och kommentarsankare mellan körningar.
                    b"commentRangeStart" | b"commentRangeEnd"
                        if info.track && empty && !in_run && para.is_some() && attr(e, b"id").and_then(|v| v.parse::<u32>().ok()).is_some() =>
                    {
                        let id = attr(e, b"id").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
                        let start = name == b"commentRangeStart";
                        if start && !info.comment_ids.contains(&id) {
                            info.comment_ids.push(id);
                        }
                        let text = if start { review::marker_start(id) } else { review::marker_end(id) };
                        let p = para.get_or_insert_with(Para::new);
                        p.runs.push(Run { text, fmt: Fmt::default(), link: None, raw: true });
                    }
                    b"bookmarkStart" | b"bookmarkEnd" | b"commentRangeStart" | b"commentRangeEnd" | b"permStart" | b"permEnd"
                        if empty && !in_run && para.is_some() =>
                    {
                        push_token(&mut para, info, &xml[before..after]);
                    }
                    // Enkelt fält: blir ett vanligt fält (start- och slutmarkör) runt resultatet.
                    b"fldSimple" if !empty => {
                        let instr = attr(e, b"instr").unwrap_or_default();
                        let mut x = String::new();
                        xml_escape(&instr, &mut x);
                        let begin = format!(
                            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\">{x}</w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>"
                        );
                        push_token(&mut para, info, &begin);
                        simple_fields.push(());
                    }
                    // Spårat stycketecken: `w:ins`/`w:del` i styckets egna `w:rPr`.
                    b"ins" | b"del" | b"moveTo" | b"moveFrom" if info.track && in_ppr && para.is_some() && tables.is_empty() => {
                        let kind = if matches!(name.as_slice(), b"ins" | b"moveTo") { RevKind::Ins } else { RevKind::Del };
                        let id = info.revs.iter().map(|r| r.id).max().unwrap_or(0) + 1;
                        info.revs.push(RevRec { id, author: attr(e, b"author").unwrap_or_default(), date: attr(e, b"date").unwrap_or_default(), old: None });
                        para_marks.push((kind, id));
                    }
                    // Spårade ändringar runt körningar (inte styckemarkeringens `w:ins`/`w:del` i `w:rPr`).
                    b"ins" | b"del" | b"moveTo" | b"moveFrom" if info.track && !empty && !in_rpr && !in_ppr => {
                        let kind = if matches!(name.as_slice(), b"ins" | b"moveTo") { RevKind::Ins } else { RevKind::Del };
                        let id = info.revs.iter().map(|r| r.id).max().unwrap_or(0) + 1;
                        info.revs.push(RevRec { id, author: attr(e, b"author").unwrap_or_default(), date: attr(e, b"date").unwrap_or_default(), old: None });
                        cur_rev = Some((kind, id));
                        rev_stack.push(cur_rev);
                    }
                    b"del" | b"moveFrom" => {
                        if !empty && !in_rpr && !in_ppr {
                            skip = 1;
                        }
                    }
                    b"ins" | b"moveTo" => {}
                    // Spårad teckenformatering: det gamla formatet blir en post, körningen får en `<fmt>`.
                    b"rPrChange" if info.track && in_rpr && !in_ppr && !empty && element_end(xml, before).is_some() => {
                        let end = element_end(xml, before).unwrap_or(after);
                        let linked = link.last().is_some_and(Option::is_some);
                        let old = review_xml::parse_old_rpr(&xml[before..end], styles, &base, para.as_ref(), theme, linked);
                        let rec = RevRec { id: 0, author: attr(e, b"author").unwrap_or_default(), date: attr(e, b"date").unwrap_or_default(), old: Some(old.encode()) };
                        fmt.fchange = Some(review_xml::fmt_rev_id(info, rec));
                        skip = 1;
                    }
                    // Spårad styckeformatering: samma sak med en markör sist i stycket.
                    b"pPrChange" if info.track && in_ppr && !empty && para.is_some() && tables.is_empty() && element_end(xml, before).is_some() => {
                        let end = element_end(xml, before).unwrap_or(after);
                        let old = review_xml::parse_old_ppr(&xml[before..end], styles, numbering);
                        let rec = RevRec { id: 0, author: attr(e, b"author").unwrap_or_default(), date: attr(e, b"date").unwrap_or_default(), old: Some(old.encode()) };
                        para_fchange = Some(review_xml::fmt_rev_id(info, rec));
                        skip = 1;
                    }
                    b"pPrChange" | b"rPrChange" => {
                        dropped.tracked = true;
                        if !empty {
                            skip = 1;
                        }
                    }
                    // Avsnittsslut i ett toppnivåstycke: egenskaperna blir en avsnittsrad efter stycket.
                    b"sectPr" if in_ppr && tables.is_empty() && para.is_some() && para_sect.is_none() => {
                        if empty {
                            para_sect = Some(xml[before..after].to_owned());
                        } else if let Some(end) = element_end(xml, before) {
                            para_sect = Some(xml[before..end].to_owned());
                            skip = 1;
                        }
                    }
                    b"sdtPr" | b"sdtEndPr" | b"instrText" | b"sectPr" | b"tblPr" | b"trPr" | b"fldData" => {
                        if !empty {
                            skip = 1;
                        }
                    }
                    // Raderad text ligger i `w:delText`; utan spårning är den borta redan.
                    b"delText" => {
                        if info.track {
                            in_t = !empty;
                        } else if !empty {
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
                        para_marks.clear();
                        para_fchange = None;
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
                        if !empty {
                            in_run = true;
                            run_start = before;
                            run_opaque = false;
                            run_mark = para.as_ref().map_or((0, 0), |p| (p.runs.len(), p.runs.last().map_or(0, |l| l.text.len())));
                            run_found_mark = found.len();
                            run_drop = false;
                        }
                        fmt = Fmt::default();
                        fmt.rev = cur_rev;
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
                    b"br" if !hidden(&field_depth) && attr(e, b"type").as_deref() == Some("column") && tables.is_empty() => {
                        para_col_break = true;
                    }
                    b"br" | b"cr" if !hidden(&field_depth) => {
                        // En infogad eller struken radbrytning får en markör före sig.
                        if let Some((kind @ (RevKind::Ins | RevKind::Del), id)) = cur_rev {
                            let p = para.get_or_insert_with(Para::new);
                            p.runs.push(Run { text: review::pmark_text(kind, id, None), fmt: Fmt::default(), link: None, raw: true });
                        }
                        push_text(&mut para, "\n", &Fmt::default(), &None);
                    }
                    b"noBreakHyphen" if !hidden(&field_depth) => {
                        push_text(&mut para, "\u{2011}", &fmt, &link.last().cloned().flatten())
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
                        tables.push(Tbl::default());
                    }
                    b"tr" if !empty => {
                        if let Some(t) = tables.last_mut() {
                            t.row = Vec::new();
                            t.row_spans = Vec::new();
                        }
                    }
                    b"tc" if !empty => {
                        if let Some(t) = tables.last_mut() {
                            t.cell = Some(String::new());
                            t.span = 1;
                        }
                    }
                    // Kolumnbredd och sammanfogning. En lodrätt sammanfogad fortsättningscell
                    // (`w:vMerge`) läses som en tom cell: Kladd har bara vågräta sammanfogningar.
                    b"gridCol" => {
                        if let Some(t) = tables.last_mut() {
                            t.widths.push(attr(e, b"w").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0));
                        }
                    }
                    b"gridSpan" => {
                        if let Some(t) = tables.last_mut() {
                            t.span = attr(e, b"val").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, 63);
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                if skip > 0 {
                    skip -= 1;
                    if skip == 0 {
                        if let Some(start) = omath_start.take() {
                            // Formel: LaTeX om den går att uttrycka, annars en markör.
                            match crate::math::omml::to_latex(&xml[start..after]) {
                                Some((latex, display)) => {
                                    let d = if display { "$$" } else { "$" };
                                    let p = para.get_or_insert_with(Para::new);
                                    p.runs.push(Run { text: format!("{d}{latex}{d}"), fmt: Fmt::default(), link: None, raw: true });
                                }
                                None => push_token(&mut para, info, &xml[start..after]),
                            }
                        }
                    }
                    continue;
                }
                if let Some(d) = drawing.as_mut() {
                    d.depth -= 1;
                    match local(e.name().as_ref()) {
                        b"positionH" => d.in_pos_h = false,
                        b"align" | b"posOffset" => d.text_of = 0,
                        _ => {}
                    }
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
                                anchor: (d.anchored && d.wraps).then_some(d.pos),
                                fallback: None,
                            });
                        }
                        Some(_) => {}
                        // Former, diagram, textrutor och länkade bilder bevaras som markörer.
                        None => run_opaque = true,
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
                    b"t" | b"delText" => in_t = false,
                    b"ins" | b"del" | b"moveTo" | b"moveFrom" if info.track && !in_rpr && !in_ppr && rev_stack.pop().is_some() => {
                        cur_rev = rev_stack.last().copied().flatten();
                    }
                    b"r" => {
                        fmt = Fmt::default();
                        if in_run {
                            in_run = false;
                            let run_xml = &xml[run_start..after];
                            let fnref = run_fnref.take();
                            if run_drop && !run_opaque {
                                // Kommentarsreferensen: det som står kvar är slutmarkören före den.
                                if let Some(p) = para.as_mut() {
                                    p.runs.truncate(run_mark.0);
                                    if let Some(l) = p.runs.last_mut() {
                                        l.text.truncate(run_mark.1);
                                    }
                                }
                            } else if let (false, Some(id)) = (run_opaque, fnref) {
                                // Backa körningen och lägg `[^N]` i stället; N räknas i dokumentordning.
                                if let Some(p) = para.as_mut() {
                                    p.runs.truncate(run_mark.0);
                                    if let Some(l) = p.runs.last_mut() {
                                        l.text.truncate(run_mark.1);
                                    }
                                }
                                info.fn_refs.push(FnRefInfo { id, origin: info.elems.len().wrapping_sub(1) });
                                let label = format!("[^{}]", info.fn_refs.len());
                                let p = para.get_or_insert_with(Para::new);
                                p.runs.push(Run { text: label, fmt: Fmt::default(), link: None, raw: true });
                            } else if run_opaque {
                                // Backa det körningen redan lagt till och lägg en markör i stället.
                                if let Some(p) = para.as_mut() {
                                    p.runs.truncate(run_mark.0);
                                    if let Some(l) = p.runs.last_mut() {
                                        l.text.truncate(run_mark.1);
                                    }
                                }
                                push_token(&mut para, info, run_xml);
                            } else if found.len() > run_found_mark {
                                // Bilder som inte går att visa ersätts av hela körningen.
                                let n = new_token(info, run_xml);
                                for f in &mut found[run_found_mark..] {
                                    f.fallback = Some(n);
                                }
                            }
                            run_opaque = false;
                            run_drop = false;
                        }
                    }
                    b"fldSimple" => {
                        if simple_fields.pop().is_some() {
                            push_token(&mut para, info, "<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>");
                        }
                    }
                    b"hyperlink" => {
                        link.pop();
                    }
                    b"AlternateContent" => {
                        alt_depth.pop();
                    }
                    b"p" => {
                        if let Some(mut p) = para.take() {
                            // Spårade stycketecken och styckeformatering: markörer sist i stycket.
                            if tables.is_empty() {
                                if let Some(id) = para_fchange.take() {
                                    p.runs.push(Run { text: review::pmark_text(RevKind::Fmt, id, None), fmt: Fmt::default(), link: None, raw: true });
                                }
                                for (kind, id) in para_marks.drain(..) {
                                    p.runs.push(Run { text: review::pmark_text(kind, id, None), fmt: Fmt::default(), link: None, raw: true });
                                }
                            }
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
                                if let Some(cell) = t.cell.as_mut() {
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
                                p.origin = info.elems.len().wrapping_sub(1);
                                // Spaltbrytning före stycket (eller ensam), avsnittsrad efter det.
                                let origin = p.origin;
                                if std::mem::take(&mut para_col_break) {
                                    blocks.push(Block::Para(raw_para(sections::COLUMN_BREAK_LINE.to_owned(), origin)));
                                }
                                blocks.push(Block::Para(p));
                                if let Some(x) = para_sect.take() {
                                    info.sects.push(x);
                                    blocks.push(Block::Para(raw_para(section_placeholder(info.sects.len() - 1), origin)));
                                }
                            }
                        } else {
                            para_sect = None;
                            para_col_break = false;
                        }
                    }
                    b"tc" => {
                        if let Some(t) = tables.last_mut() {
                            let cell = t.cell.take().unwrap_or_default();
                            let span = t.span.max(1);
                            t.row.push(cell);
                            t.row_spans.push(span);
                            for _ in 1..span {
                                t.row.push(String::new());
                                t.row_spans.push(0);
                            }
                        }
                    }
                    b"tr" => {
                        if let Some(t) = tables.last_mut() {
                            let row = std::mem::take(&mut t.row);
                            let sp = std::mem::take(&mut t.row_spans);
                            t.rows.push(row);
                            t.spans.push(sp);
                        }
                    }
                    b"tbl" => {
                        if let Some(t) = tables.pop() {
                            if let Some(outer) = tables.last_mut() {
                                // Nästlad tabell: platta till i den yttre cellen.
                                let flat: Vec<String> = t.rows.iter().map(|r| r.iter().filter(|c| !c.is_empty()).cloned().collect::<Vec<_>>().join(" ")).collect();
                                if let Some(cell) = outer.cell.as_mut() {
                                    if !cell.is_empty() {
                                        cell.push_str("<br>");
                                    }
                                    cell.push_str(&flat.join("<br>"));
                                }
                            } else if !t.rows.is_empty() {
                                blocks.push(Block::Table(t, info.elems.len().wrapping_sub(1)));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) => {
                if let Some(d) = drawing.as_mut().filter(|d| d.text_of != 0) {
                    let t = text_of(&ev);
                    match d.text_of {
                        1 => d.pos.align = Some(t.trim().to_ascii_lowercase()),
                        _ => d.pos.off = t.trim().parse().ok(),
                    }
                } else if skip == 0 && in_t && !hidden(&field_depth) {
                    let t = text_of(&ev).replace(['\n', '\r'], " ");
                    if !t.is_empty() {
                        push_text(&mut para, &t, &fmt, &link.last().cloned().flatten());
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(mut p) = para.take() {
        p.origin = info.elems.len().wrapping_sub(1);
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
    // En tabb först (tabbstopp i början av stycket) blir annars också ett kodblock.
    let lead = line.trim_start_matches([' ', '\t']);
    if line[..line.len() - lead.len()].contains('\t') {
        let at = line.find('\t').unwrap_or(0);
        return format!("{}&#9;{}", &line[..at], &line[at + 1..]);
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
        if let Some(id) = run.fmt.fchange {
            s = format!("{}{s}{}", review::open_tag(RevKind::Fmt, id), review::close_tag(RevKind::Fmt));
        }
        if let Some((kind, id)) = run.fmt.rev {
            s = format!("{}{s}{}", review::open_tag(kind, id), review::close_tag(kind));
        }
        parts.push(s);
    }
    parts.concat()
}

fn para_md(p: &Para) -> String {
    let mut s: String = p.runs.iter().map(inline_md).collect();
    // Ta bort mellanslag i slutet av varje rad (de syns inte och blir "hårda radbrytningar").
    // En rad som börjar med en markör skulle annars tolkas som ett HTML-block; en osynlig
    // tecknet-referens framför gör den till vanlig text (`write` tar bort den igen).
    s = s
        .lines()
        .map(|l| {
            let l = l.trim_end();
            if l.starts_with("<!--k:") { format!("&#8203;{l}") } else { l.to_owned() }
        })
        .collect::<Vec<_>>()
        .join("\n");
    s.trim_matches('\n').to_owned()
}

/// Texten utan markörer (`<!--k:N-->`); tom för ett stycke som bara innehåller okända objekt.
fn without_tokens(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<!--k:") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.replace('\u{200B}', "").replace("&#8203;", "")
}

/// Jämförbar form av en blocktext: varje rad trimmad (indrag och markörer spelar ingen roll).
fn norm(s: &str) -> String {
    strip_fn_labels(&s.lines().map(str::trim).collect::<Vec<_>>().join("\n")).trim().to_owned()
}

/// `[^3]` → `[^]`: fotnotsnumren ändras när fotnoter läggs till och ska inte räknas som en ändring
/// av stycket.
fn strip_fn_labels(s: &str) -> String {
    if !s.contains("[^") {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("[^") {
        let after = &rest[i + 2..];
        match after.find(']') {
            Some(j) if j > 0 && !after[..j].contains(|c: char| c.is_whitespace() || c == '[') => {
                out.push_str(&rest[..i + 2]);
                out.push(']');
                rest = &after[j + 1..];
            }
            _ => {
                out.push_str(&rest[..i + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Ett Markdown-block som kan matchas mot originalets XML: toppnivåelementet det kom från,
/// en beskrivning av slaget (rubrik, listnivå, justering …) och den normaliserade texten.
struct MdUnit {
    origin: usize,
    desc: String,
    key: String,
}

#[cfg(test)]
fn to_markdown(blocks: &[Block], numbering: &Numbering) -> String {
    to_markdown_units(blocks, numbering, None).0
}

/// `text_w` är textytans bredd i twips: tabeller som är klart smalare får automatisk bredd i
/// stället för proportionella kolumner (som annars fyller hela bredden).
fn to_markdown_units(blocks: &[Block], numbering: &Numbering, text_w: Option<f64>) -> (String, Vec<MdUnit>) {
    let mut units: Vec<MdUnit> = Vec::new();
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
                if without_tokens(&text).trim().is_empty() {
                    continue;
                }
                // Kodblock justeras inte; Kladds egna markörrader följer omgivningen.
                let is_marker = matches!(p.runs.as_slice(), [r] if r.raw && r.text.starts_with("<!-- kladd:"));
                let align = if p.kind == ParaKind::Code {
                    Align::Left
                } else if is_marker {
                    cur_align
                } else {
                    p.align
                };
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
                        let h = format!("{} {}", "#".repeat(n as usize), one_line.trim());
                        units.push(MdUnit { origin: p.origin, desc: format!("h{n}|{}", align.as_attr()), key: norm(&h) });
                        out.push_str(&h);
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
                        let mut key = escape_line_start(first);
                        out.push_str(&format!("{indent}{marker}{key}"));
                        for l in lines {
                            out.push('\n');
                            out.push_str(&cont);
                            out.push_str(l);
                            key.push('\n');
                            key.push_str(l);
                        }
                        let kind = if ordered { 'o' } else { 'u' };
                        units.push(MdUnit { origin: p.origin, desc: format!("l{l}{kind}|{}", align.as_attr()), key: norm(&key) });
                    }
                    (ParaKind::Quote, None) => {
                        let q: Vec<String> = text.lines().map(|l| format!("> {}", escape_line_start(l))).collect();
                        let q = q.join("\n");
                        units.push(MdUnit { origin: p.origin, desc: format!("q|{}", align.as_attr()), key: norm(&q) });
                        out.push_str(&q);
                    }
                    (ParaKind::Code, None) => {
                        let raw: String = p.runs.iter().map(|r| r.text.as_str()).collect();
                        let fence = if raw.contains("```") { "~~~~" } else { "```" };
                        let code = format!("{fence}\n{}\n{fence}", raw.trim_matches('\n'));
                        units.push(MdUnit { origin: p.origin, desc: format!("c|{}", align.as_attr()), key: norm(&code) });
                        out.push_str(&code);
                    }
                    (ParaKind::Normal, None) => {
                        let lines: Vec<String> = text.lines().map(escape_line_start).collect();
                        let body = lines.join("\n");
                        units.push(MdUnit { origin: p.origin, desc: format!("p|{}", align.as_attr()), key: norm(&body) });
                        out.push_str(&body);
                    }
                }
            }
            Block::Table(tbl, origin) => {
                prev_list = false;
                top_num = None;
                // Rubrikraden skrivs i fetstil av Kladd; visa den utan **.
                let mut rows = tbl.rows.clone();
                if rows[0].iter().all(|c| c.is_empty() || (c.len() > 4 && c.starts_with("**") && c.ends_with("**"))) {
                    for c in rows[0].iter_mut() {
                        if c.len() > 4 {
                            *c = c[2..c.len() - 2].to_owned();
                        }
                    }
                }
                // Tabeller justeras inte.
                if cur_align != Align::Left {
                    set_align(&mut out, &mut cur_align, Align::Left);
                } else if !out.is_empty() {
                    out.push_str("\n\n");
                }
                let start = out.len();
                let narrow = text_w.is_some_and(|tw| tw > 0.0 && tbl.widths.iter().sum::<f64>() < tw * 0.9);
                let widths: &[f64] = if narrow { &[] } else { &tbl.widths };
                out.push_str(&markup::table_markdown(&rows, &tbl.spans, widths));
                units.push(MdUnit { origin: *origin, desc: "t|left".to_owned(), key: norm(&out[start..]) });
            }
        }
    }
    if cur_align != Align::Left {
        out.push_str("\n\n</div>");
    }
    out.push('\n');
    (out, units)
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

/// Det `Writer` behöver veta om ett befintligt paket (tomt när dokumentet skrivs från grunden).
#[derive(Default)]
struct Ctx<'a> {
    /// Antal okända objekt som markörerna kan peka på.
    tokens: usize,
    /// numId för punktlistan; numrerade listor får de följande.
    num_base: u32,
    /// Första `wp:docPr`-id för nya bilder.
    doc_pr_start: u32,
    /// Textytans bredd i EMU (största bildbredd).
    max_width: i64,
    /// Delnamn som redan finns (gemener).
    taken: std::collections::HashSet<String>,
    /// Relations-id som redan finns.
    used_ids: std::collections::HashSet<String>,
    /// Befintliga externa länkar: (id, url).
    links: Vec<(String, String)>,
    /// Befintliga bilder: (relations-id, innehåll).
    media: Vec<(String, &'a [u8])>,
    /// Författare och datum per spårad ändring (efter `data-k`).
    revs: HashMap<u32, RevRec>,
    /// Kommentarer som finns som poster (områden utan post skrivs inte).
    comments: std::collections::HashSet<u32>,
    /// Första avsnittsraden står före all text och gäller första avsnittet (ingen sektionsbrytning).
    lead_section: bool,
}

/// En enhet i den skrivna brödtexten (stycke, listpunkt, tabell …) som kan bytas mot originalet.
struct GenUnit {
    desc: String,
    key: String,
    range: std::ops::Range<usize>,
}

struct Writer<'a> {
    ctx: Ctx<'a>,
    units: Vec<GenUnit>,
    /// Nästa `wp:docPr`-id.
    doc_pr_next: u32,
    /// Relations-id som delats ut hittills.
    ids: std::collections::HashSet<String>,
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
    /// Fotnotsdefinitionerna (etikett, normaliserad text, `<w:p>`-XML), i textens ordning.
    footnotes: Vec<GenNote>,
    /// Etiketten för varje fotnotsreferens i dokumentordning; platshållaren i XML:en är dess index.
    fn_order: Vec<String>,
    /// Sant medan en fotnotsdefinition skrivs (inga länkar eller bilder, eftersom delen har egna relationer).
    in_footnote: bool,
    /// Antal innehållsförteckningar skrivna hittills (för unika id).
    toc_n: u32,
    /// Öppen `<ins>`/`<del>`.
    rev: Option<(RevKind, u32)>,
    /// Öppen `<fmt>` (spårad teckenformatering).
    fchange: Option<u32>,
    /// Styckeformatering (`<!--k:pm fmt-->`) som väntar på styckets slut.
    fchange_para: Option<u32>,
    /// Stycke- och radbrytningsmarkörer (`<!--k:pm …-->`) som väntar på brytningen eller styckets slut.
    pending_pm: Vec<(RevKind, u32)>,
    /// Var det öppna stycket börjar i `body`.
    para_at: usize,
    /// Nästa `w:id` för en skriven ändring.
    rev_next: u32,
    /// Kommentarsområden som redan skrivits (ett område får bara finnas en gång).
    c_started: std::collections::HashSet<u32>,
    c_ended: std::collections::HashSet<u32>,
}

/// En fotnotsdefinition som skrivits ur Markdown.
struct GenNote {
    label: String,
    key: String,
    xml: String,
}

struct TableState {
    in_head: bool,
    /// Kolumnbredder i tjugondels punkter.
    widths: Vec<u32>,
    /// Spann per rad och cell ur tabellens källa (0 = täckt cell).
    spans: Vec<Vec<usize>>,
    /// Antal påbörjade rader och cellen i raden som läses.
    rows: usize,
    cell: usize,
    /// Den här cellen är täckt av cellen före och skrivs inte.
    covered: bool,
}

/// Platshållare i den skrivna texten för markör nummer N (byts mot originalets XML när
/// brödtexten sätts ihop, så att dubbletter och bortvalda enheter kan hanteras).
fn marker(n: usize) -> String {
    format!("\u{E002}{n}\u{E003}")
}

impl<'a> Writer<'a> {
    fn new(ctx: Ctx<'a>) -> Self {
        Self {
            doc_pr_next: ctx.doc_pr_start.max(1),
            ids: ctx.used_ids.clone(),
            ctx,
            units: Vec::new(),
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
            footnotes: Vec::new(),
            fn_order: Vec::new(),
            in_footnote: false,
            toc_n: 0,
            rev: None,
            fchange: None,
            fchange_para: None,
            pending_pm: Vec::new(),
            para_at: 0,
            rev_next: 5_000_000,
            c_started: Default::default(),
            c_ended: Default::default(),
        }
    }

    /// Skriver en innehållsförteckning: en innehållskontroll med ett `TOC`-fält vars sparade
    /// resultat är rubrikerna H1–H3 (sidnummer grovt uppskattade). Word och LibreOffice uppdaterar
    /// fältet vid öppning (`w:updateFields` i settings.xml).
    fn toc(&mut self, md: &str) {
        use crate::word::nav;
        self.toc_n += 1;
        let heads = nav::headings_scan(md);
        let entries = nav::toc_entries(&heads);
        // Textytans bredd i tjugondels punkter (EMU / 635).
        let width = if self.ctx.max_width > 0 { self.ctx.max_width / 635 } else { 9026 };
        let tab = format!("<w:tabs><w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"{width}\"/></w:tabs>");
        let id = 1_000_000 + self.toc_n;
        let mut x = format!(
            "<w:sdt><w:sdtPr><w:id w:val=\"{id}\"/><w:docPartObj><w:docPartGallery w:val=\"Table of Contents\"/><w:docPartUnique/></w:docPartObj><w:tag w:val=\"{TOC_TAG}\"/></w:sdtPr><w:sdtContent>"
        );
        x.push_str("<w:p><w:pPr><w:pStyle w:val=\"TOCHeading\"/></w:pPr><w:r><w:t>Innehåll</w:t></w:r></w:p>");
        let begin = "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> TOC \\o \"1-3\" \\h \\z \\u </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>";
        for (i, h) in entries.iter().enumerate() {
            let ind = (i32::from(h.level) - 1) * 220;
            let mut t = String::new();
            xml_escape(&h.text, &mut t);
            // Grov uppskattning: ca 3 000 tecken per sida.
            let page = 1 + md[..h.start.min(md.len())].chars().count() / 3000;
            x.push_str(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"TOC{}\"/>{tab}<w:spacing w:after=\"100\"/><w:ind w:left=\"{ind}\"/></w:pPr>{}<w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>{page}</w:t></w:r></w:p>",
                h.level,
                if i == 0 { begin } else { "" },
            ));
        }
        if entries.is_empty() {
            x.push_str(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"TOC1\"/>{tab}</w:pPr>{begin}<w:r><w:t>Inga rubriker</w:t></w:r></w:p>"
            ));
        }
        x.push_str("<w:p><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p></w:sdtContent></w:sdt>");
        self.body.push_str(&x);
    }

    /// Bäddar in en bild som en egen körning i aktuellt stycke.
    /// Ett relations-id som varken finns i originalet eller har delats ut.
    fn fresh_id(&mut self, prefix: &str) -> String {
        let mut n = 1;
        loop {
            let id = format!("{prefix}{n}");
            if self.ids.insert(id.clone()) {
                return id;
            }
            n += 1;
        }
    }

    fn picture(&mut self, bytes: Vec<u8>, extent: Option<(i64, i64)>, alt: &str, float: Option<Float>) {
        let Some(ext) = image_ext(&bytes) else { return };
        // Samma bild flera gånger bäddas bara in en gång; en bild som redan finns i
        // originalpaketet återanvänds.
        let existing = self
            .media
            .iter()
            .find(|(_, _, b)| *b == bytes)
            .map(|(id, _, _)| id.clone())
            .or_else(|| self.ctx.media.iter().find(|(_, b)| *b == bytes.as_slice()).map(|(id, _)| id.clone()));
        let (cx, cy) = image_extent(&bytes, extent, self.ctx.max_width);
        let id = match existing {
            Some(id) => id,
            None => {
                let id = self.fresh_id("rIdImg");
                let mut n = self.media.len() + 1;
                let file = loop {
                    let f = format!("image{n}.{ext}");
                    if !self.ctx.taken.contains(&format!("word/media/{f}").to_lowercase()) {
                        break f;
                    }
                    n += 1;
                };
                self.media.push((id.clone(), file, bytes));
                id
            }
        };
        let doc_pr = self.doc_pr_next;
        self.doc_pr_next += 1;
        let mut alt_x = String::new();
        xml_escape(alt, &mut alt_x);
        self.open_para();
        const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
        // Gemensam del efter placeringen: storlek, beskrivning och själva bilden.
        let tail = format!(
            r#"<wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>{{WRAP}}<wp:docPr id="{doc_pr}" name="Bild {doc_pr}" descr="{alt_x}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{doc_pr}" name="Bild {doc_pr}" descr="{alt_x}"/><pic:cNvPicPr><a:picLocks noChangeAspect="1" noChangeArrowheads="1"/></pic:cNvPicPr></pic:nvPicPr><pic:blipFill><a:blip r:embed="{id}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr bwMode="auto"><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>"#
        );
        let drawing = match float {
            // Flytande bild: förankrad vid stycket, vid vänster/höger textkant, text runt (12 px luft).
            Some(side) => {
                let (al, dist_l, dist_r) = match side {
                    Float::Left => ("left", 0, 114300),
                    Float::Right => ("right", 114300, 0),
                };
                format!(
                    r#"<wp:anchor xmlns:wp="{WP}" xmlns:r="{R_NS}" distT="0" distB="0" distL="{dist_l}" distR="{dist_r}" simplePos="0" relativeHeight="{}" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="0"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:align>{al}</wp:align></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>{}</wp:anchor>"#,
                    251_658_240 + doc_pr,
                    tail.replace("{WRAP}", r#"<wp:wrapSquare wrapText="bothSides"/>"#)
                )
            }
            None => format!(
                r#"<wp:inline xmlns:wp="{WP}" xmlns:r="{R_NS}" distT="0" distB="0" distL="0" distR="0">{}</wp:inline>"#,
                tail.replace("{WRAP}", "")
            ),
        };
        self.body.push_str(&format!("<w:r><w:drawing>{drawing}</w:drawing></w:r>"));
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
        self.para_at = self.body.len();
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
            self.apply_para_marks();
            self.body.push_str("</w:p>");
            self.para_open = false;
        }
        self.pending_pm.clear();
        self.fchange_para = None;
    }

    /// Ett `w:id`, författare och datum för en skriven ändring ur posten `id`.
    fn rev_attrs(&mut self, id: u32) -> String {
        let rec = self.ctx.revs.get(&id).cloned().unwrap_or_else(|| RevRec { id, author: "Okänd".into(), date: String::new(), old: None });
        let (mut a, mut d) = (String::new(), String::new());
        xml_escape(&rec.author, &mut a);
        xml_escape(&rec.date, &mut d);
        let date = if d.is_empty() { String::new() } else { format!(" w:date=\"{d}\"") };
        self.rev_next += 1;
        format!("w:id=\"{}\" w:author=\"{a}\"{date}", self.rev_next)
    }

    /// Spårade stycketecken (`w:rPr/w:ins|w:del`) och styckeformatering (`w:pPrChange`) läggs i styckets
    /// `w:pPr` när stycket stängs (markörerna står sist i stycket).
    fn apply_para_marks(&mut self) {
        let fmt_id = self.fchange_para.take();
        if self.pending_pm.is_empty() && fmt_id.is_none() {
            return;
        }
        let marks = std::mem::take(&mut self.pending_pm);
        let mut inner = String::new();
        let mut rpr = String::new();
        for (kind, id) in marks {
            let attrs = self.rev_attrs(id);
            match kind {
                RevKind::Ins => rpr.push_str(&format!("<w:ins {attrs}/>")),
                RevKind::Del => rpr.push_str(&format!("<w:del {attrs}/>")),
                RevKind::Fmt => {}
            }
        }
        if !rpr.is_empty() {
            inner.push_str(&format!("<w:rPr>{rpr}</w:rPr>"));
        }
        if let Some(id) = fmt_id {
            let old = self.ctx.revs.get(&id).and_then(|r| r.old.clone());
            if let Some(old) = old {
                let attrs = self.rev_attrs(id);
                let old_ppr = review_xml::old_ppr_xml(&review::fmt::ParaFmt::decode(&old), self.ctx.num_base, !self.ordered_nums.is_empty());
                inner.push_str(&format!("<w:pPrChange {attrs}><w:pPr>{old_ppr}</w:pPr></w:pPrChange>"));
            }
        }
        if inner.is_empty() {
            return;
        }
        let head = self.para_at;
        if !self.body[head..].starts_with("<w:p>") {
            return;
        }
        if self.ppr.is_empty() {
            self.body.insert_str(head + 5, &format!("<w:pPr>{inner}</w:pPr>"));
        } else {
            let at = head + 5 + 7 + self.ppr.len();
            if self.body[head..].starts_with("<w:p><w:pPr>") && self.body.is_char_boundary(at) {
                self.body.insert_str(at, &inner);
            }
        }
    }

    /// En formel (`$…$`/`$$…$$`) som Office Math. Går den inte att tolka skrivs källtexten som text.
    fn math(&mut self, src: &str, display: bool) {
        let src = src.trim();
        match crate::math::parse::parse(src) {
            Ok(nodes) if !nodes.is_empty() => {
                self.open_para();
                self.body.push_str(&crate::math::omml::to_omml(&nodes, display));
            }
            _ => {
                let d = if display { "$$" } else { "$" };
                self.run(&format!("{d}{src}{d}"), false);
            }
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
        // Spårad teckenformatering: det gamla formatet sist i `w:rPr`.
        if let Some(fid) = self.fchange {
            if let Some(old) = self.ctx.revs.get(&fid).and_then(|r| r.old.clone()) {
                let attrs = self.rev_attrs(fid);
                let old_rpr = review_xml::old_rpr_xml(&review::fmt::CharFmt::decode(&old));
                rpr.push_str(&format!("<w:rPrChange {attrs}><w:rPr>{old_rpr}</w:rPr></w:rPrChange>"));
            }
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
        if let Some((kind, id)) = self.rev {
            let (tag, ins) = match kind {
                RevKind::Ins => ("w:ins", true),
                RevKind::Del | RevKind::Fmt => ("w:del", false),
            };
            if !ins {
                xml = xml.replace("<w:t ", "<w:delText ").replace("</w:t>", "</w:delText>");
            }
            let attrs = self.rev_attrs(id);
            xml = format!("<{tag} {attrs}>{xml}</{tag}>");
        }
        if let Some(url) = self.fmt.link.clone() {
            let id = self.link_rel(&url);
            self.body.push_str(&format!("<w:hyperlink xmlns:r=\"{R_NS}\" r:id=\"{id}\" w:history=\"1\">{xml}</w:hyperlink>"));
        } else {
            self.body.push_str(&xml);
        }
    }

    fn link_rel(&mut self, url: &str) -> String {
        if let Some((id, _)) = self.rels.iter().find(|(_, u)| u == url) {
            return id.clone();
        }
        if let Some((id, _)) = self.ctx.links.iter().find(|(_, u)| u == url) {
            return id.clone();
        }
        let id = self.fresh_id("rIdL");
        self.rels.push((id.clone(), url.to_owned()));
        id
    }

    /// Lägger en markör för ett okänt objekt i stycket.
    fn token(&mut self, n: usize) {
        if n < self.ctx.tokens {
            self.open_para();
            self.body.push_str(&marker(n));
        }
    }

    /// Kommentarsområdets början eller slut (med referensen efter slutet).
    fn comment_marker(&mut self, start: bool, id: u32) {
        if self.in_footnote || !self.ctx.comments.contains(&id) {
            return;
        }
        if start {
            if !self.c_started.insert(id) {
                return;
            }
            self.open_para();
            self.body.push_str(&format!("<w:commentRangeStart w:id=\"{id}\"/>"));
        } else if self.c_started.contains(&id) && self.c_ended.insert(id) {
            self.open_para();
            self.body.push_str(&format!(
                "<w:commentRangeEnd w:id=\"{id}\"/><w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:commentReference w:id=\"{id}\"/></w:r>"
            ));
        }
    }

    /// Bäddar in en bild från en `![](…)` eller `<img>`; saknas bilden stannar texten.
    fn image_ref(&mut self, src: &str, alt: &str, size: (Option<f64>, Option<f64>), float: Option<Float>, resolve: &dyn Fn(&str) -> Option<Image>) {
        if self.in_footnote {
            self.run(alt, false);
            return;
        }
        match resolve(src) {
            Some(img) if is_supported_image(&img.bytes) => {
                let extent = sized_extent(&img, size);
                self.picture(img.bytes, extent, alt, float);
            }
            _ => {
                self.missing.push(src.to_owned());
                // Behåll åtminstone texten.
                let label = if alt.is_empty() { format!("[bild: {src}]") } else { format!("[bild: {alt}]") };
                self.run(&label, false);
            }
        }
    }

    fn line_break(&mut self) {
        self.open_para();
        let mut xml = String::from("<w:r><w:br/></w:r>");
        // Spårad radbrytning: `w:ins`/`w:del` runt körningen.
        if let Some((kind, id)) = std::mem::take(&mut self.pending_pm).into_iter().find(|(k, _)| *k != RevKind::Fmt) {
            let attrs = self.rev_attrs(id);
            let tag = if kind == RevKind::Ins { "w:ins" } else { "w:del" };
            xml = format!("<{tag} {attrs}>{xml}</{tag}>");
        }
        self.body.push_str(&xml);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EvEnd {
    None,
    Paragraph,
    Heading,
    Code,
    Table,
    Quote,
    Html,
    Item,
}

struct OpenUnit {
    desc: String,
    code: EvEnd,
    start: usize,
    src: std::ops::Range<usize>,
}

struct OpenItem {
    desc: String,
    start: usize,
    /// Källtextens område för punktens egen text.
    content: Option<(usize, usize)>,
    collecting: bool,
}

fn finish_item(w: &mut Writer<'_>, md: &str, desc: String, start: usize, content: Option<(usize, usize)>) {
    if let Some((a, b)) = content {
        let key = norm(md.get(a..b).unwrap_or(""));
        if !key.is_empty() {
            w.units.push(GenUnit { desc, key, range: start..w.body.len() });
        }
    }
}

/// `<!--k:3--><!--k:4-->` (bara markörer) → [3, 4].
fn parse_tokens(s: &str) -> Option<Vec<usize>> {
    let mut rest = s.trim();
    let mut out = Vec::new();
    while let Some(r) = rest.strip_prefix("<!--k:") {
        let end = r.find("-->")?;
        out.push(r[..end].trim().parse().ok()?);
        rest = r[end + 3..].trim_start();
    }
    (rest.is_empty() && !out.is_empty()).then_some(out)
}

/// En `<img src alt width height>`-tagg.
struct ImgTag {
    src: String,
    alt: String,
    width: Option<f64>,
    height: Option<f64>,
    align: Option<Float>,
}

fn parse_img(s: &str) -> Option<ImgTag> {
    let t = s.trim();
    if t.len() < 5 || !t.is_char_boundary(4) || !t[..4].eq_ignore_ascii_case("<img") || !t.ends_with('>') {
        return None;
    }
    let inner = t[4..t.len() - 1].trim_end_matches('/');
    if !inner.starts_with(char::is_whitespace) || inner.contains('<') {
        return None;
    }
    let decode = |v: &str| v.replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
    let (mut src, mut alt, mut width, mut height, mut align) = (None, String::new(), None, None, None);
    let mut chars = inner.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        // Attributets namn.
        let mut end = inner.len();
        while let Some(&(j, c)) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                end = j;
                break;
            }
            chars.next();
        }
        let name = inner[i..end].to_ascii_lowercase();
        while chars.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek().is_some_and(|&(_, c)| c == '=') {
            chars.next();
            while chars.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
                chars.next();
            }
            match chars.peek().copied() {
                Some((_, q @ ('"' | '\''))) => {
                    chars.next();
                    for (_, c) in chars.by_ref() {
                        if c == q {
                            break;
                        }
                        value.push(c);
                    }
                }
                _ => {
                    while let Some(&(_, c)) = chars.peek() {
                        if c.is_whitespace() {
                            break;
                        }
                        value.push(c);
                        chars.next();
                    }
                }
            }
        }
        let value = decode(&value);
        let num = || value.trim().trim_end_matches("px").trim().parse::<f64>().ok().filter(|v| *v > 0.0 && *v < 100_000.0);
        match name.as_str() {
            "src" => src = Some(value.clone()),
            "alt" => alt = value.clone(),
            "width" => width = num(),
            "height" => height = num(),
            "align" => {
                align = match value.trim().to_ascii_lowercase().as_str() {
                    "left" => Some(Float::Left),
                    "right" => Some(Float::Right),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    Some(ImgTag { src: src.filter(|s| !s.is_empty())?, alt, width, height, align })
}

/// Bildens storlek i EMU när taggen anger bredd och/eller höjd i CSS-pixlar (bevarar proportionerna).
fn sized_extent(img: &Image, size: (Option<f64>, Option<f64>)) -> Option<(i64, i64)> {
    let px = EMU_PER_PIXEL as f64;
    let natural = imagesize::blob_size(&img.bytes).ok().map(|s| (s.width as f64, s.height as f64)).or(img.extent.map(|(x, y)| (x as f64, y as f64)));
    match (size, natural) {
        ((None, None), _) => img.extent,
        ((Some(w), Some(h)), _) => Some(((w * px) as i64, (h * px) as i64)),
        ((Some(w), None), Some((nw, nh))) => Some(((w * px) as i64, (w * nh / nw * px) as i64)),
        ((None, Some(h)), Some((nw, nh))) => Some(((h * nw / nh * px) as i64, (h * px) as i64)),
        _ => img.extent,
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

fn body_from_markdown<'a>(md: &str, resolve: &dyn Fn(&str) -> Option<Image>, ctx: Ctx<'a>) -> Writer<'a> {
    let base = ctx.num_base.max(1);
    let mut w = Writer::new(ctx);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_MATH);
    let mut next_num: u32 = base + 1; // base = punktlistor
    // Enheter (stycke, listpunkt, tabell …) i brödtexten, för att kunna bytas mot originalet.
    let mut cur: Option<OpenUnit> = None;
    let mut items: Vec<OpenItem> = Vec::new();
    // Pågående fotnotsdefinition: (etikett, normaliserad text, brödtexten som lagts åt sidan).
    let mut fn_cur: Option<(String, String, String)> = None;
    for (ev, range) in Parser::new_ext(md, opts).into_offset_iter() {
        let eligible = w.quote == 0 && w.table.is_none() && !w.in_footnote;
        let mut ended = EvEnd::None;
        match &ev {
            MdEvent::Start(tag) => match tag {
                Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::Table(_) | Tag::BlockQuote(_) | Tag::HtmlBlock
                    if eligible && w.lists.is_empty() && cur.is_none() =>
                {
                    w.close_para();
                    let kind = match tag {
                        Tag::Paragraph | Tag::HtmlBlock => "p".to_owned(),
                        Tag::Heading { level, .. } => format!("h{}", heading_num(*level)),
                        Tag::CodeBlock(_) => "c".to_owned(),
                        Tag::Table(_) => "t".to_owned(),
                        _ => "q".to_owned(),
                    };
                    let code = match tag {
                        Tag::Paragraph => EvEnd::Paragraph,
                        Tag::Heading { .. } => EvEnd::Heading,
                        Tag::CodeBlock(_) => EvEnd::Code,
                        Tag::Table(_) => EvEnd::Table,
                        Tag::HtmlBlock => EvEnd::Html,
                        _ => EvEnd::Quote,
                    };
                    let align = if code == EvEnd::Table || code == EvEnd::Code { Align::Left } else { w.aligns.last().copied().unwrap_or(Align::Left) };
                    cur = Some(OpenUnit { desc: format!("{kind}|{}", align.as_attr()), code, start: w.body.len(), src: range.clone() });
                }
                Tag::Item if eligible => {
                    w.close_para();
                    let (ordered, _) = w.lists.last().copied().map_or((false, 0), |(o, n)| (o, n));
                    let kind = if ordered { 'o' } else { 'u' };
                    let align = w.aligns.last().copied().unwrap_or(Align::Left);
                    let desc = format!("l{}{kind}|{}", w.lists.len().saturating_sub(1), align.as_attr());
                    items.push(OpenItem { desc, start: w.body.len(), content: None, collecting: true });
                }
                Tag::List(_) => {
                    // En underlista avslutar föräldrapunktens egen text.
                    if let Some(it) = items.last_mut().filter(|i| i.collecting) {
                        w.close_para();
                        it.collecting = false;
                        let (desc, start, content) = (it.desc.clone(), it.start, it.content);
                        finish_item(&mut w, md, desc, start, content);
                    }
                }
                _ => {}
            },
            MdEvent::End(tag) => {
                ended = match tag {
                    TagEnd::Paragraph => EvEnd::Paragraph,
                    TagEnd::Heading(_) => EvEnd::Heading,
                    TagEnd::CodeBlock => EvEnd::Code,
                    TagEnd::Table => EvEnd::Table,
                    TagEnd::BlockQuote(_) => EvEnd::Quote,
                    TagEnd::HtmlBlock => EvEnd::Html,
                    TagEnd::Item => EvEnd::Item,
                    _ => EvEnd::None,
                };
            }
            _ => {}
        }
        // Punktens egen text: allt fram till en eventuell underlista.
        if !matches!(&ev, MdEvent::Start(Tag::Item) | MdEvent::End(TagEnd::Item)) {
            if let Some(it) = items.last_mut().filter(|i| i.collecting) {
                let (a, b) = it.content.unwrap_or((range.start, range.end));
                it.content = Some((a.min(range.start), b.max(range.end)));
            }
        }
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
                        w.lists.push((false, base));
                    }
                }
                Tag::Item => {
                    w.close_para();
                    w.item_first_para = true;
                    w.open_para();
                }
                Tag::Table(aligns) => {
                    w.close_para();
                    let src = markup::table_src(md.get(range.clone()).unwrap_or(""));
                    let cols = aligns.len().max(1);
                    // Textytans bredd i tjugondels punkter (A4 med tums marginaler: 9026).
                    let total = if w.ctx.max_width > 0 { (w.ctx.max_width / 635) as u32 } else { 9026 };
                    // Kolumnbredd efter antalet streck i avgränsarraden, annars lika breda.
                    let widths: Vec<u32> = match src.weights.as_ref().filter(|ws| ws.len() == cols) {
                        Some(ws) => {
                            let sum: usize = ws.iter().sum();
                            ws.iter().map(|&n| (f64::from(total) * n as f64 / sum as f64).round() as u32).collect()
                        }
                        None => vec![total / cols as u32; cols],
                    };
                    w.body.push_str(
                        "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"5000\" w:type=\"pct\"/><w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"1\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>",
                    );
                    for cw in &widths {
                        w.body.push_str(&format!("<w:gridCol w:w=\"{cw}\"/>"));
                    }
                    w.body.push_str("</w:tblGrid>");
                    w.table = Some(TableState { in_head: false, widths, spans: src.spans, rows: 0, cell: 0, covered: false });
                }
                Tag::TableHead => {
                    if let Some(t) = w.table.as_mut() {
                        t.in_head = true;
                        t.rows += 1;
                        t.cell = 0;
                    }
                    w.body.push_str("<w:tr><w:trPr><w:tblHeader/></w:trPr>");
                }
                Tag::TableRow => {
                    if let Some(t) = w.table.as_mut() {
                        t.rows += 1;
                        t.cell = 0;
                    }
                    w.body.push_str("<w:tr>");
                }
                Tag::TableCell => {
                    let (span, width) = match w.table.as_mut() {
                        Some(t) => {
                            let span = t.spans.get(t.rows.saturating_sub(1)).and_then(|r| r.get(t.cell)).copied().unwrap_or(1);
                            t.covered = span == 0;
                            let width: u32 = t.widths.iter().skip(t.cell).take(span.max(1)).sum();
                            (span, width)
                        }
                        None => (1, 9026),
                    };
                    // En täckt cell skrivs inte: den före den spänner över den (`w:gridSpan`).
                    if span > 0 {
                        let grid = if span > 1 { format!("<w:gridSpan w:val=\"{span}\"/>") } else { String::new() };
                        w.body.push_str(&format!("<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>{grid}</w:tcPr>"));
                        w.open_para();
                    }
                }
                Tag::FootnoteDefinition(label) => {
                    w.close_para();
                    let src = md.get(range.clone()).unwrap_or("");
                    let after = src.find("]:").map_or(0, |i| i + 2);
                    fn_cur = Some((label.to_string(), norm(&src[after..]), std::mem::take(&mut w.body)));
                    w.in_footnote = true;
                }
                Tag::Emphasis => w.fmt.italic += 1,
                Tag::Strong => w.fmt.bold += 1,
                Tag::Strikethrough => w.fmt.strike += 1,
                Tag::Link { dest_url, .. } => {
                    let url = dest_url.to_string();
                    // Interna ankare (#…) och tomma länkar blir vanlig text.
                    if !url.is_empty() && !url.starts_with('#') && !w.in_footnote {
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
                    let covered = w.table.as_mut().is_some_and(|t| {
                        t.cell += 1;
                        std::mem::take(&mut t.covered)
                    });
                    if !covered {
                        w.open_para(); // en cell måste innehålla minst ett stycke
                        w.close_para();
                        w.body.push_str("</w:tc>");
                    }
                }
                TagEnd::FootnoteDefinition => {
                    w.close_para();
                    if let Some((label, key, saved)) = fn_cur.take() {
                        let xml = std::mem::replace(&mut w.body, saved);
                        w.footnotes.push(GenNote { label, key, xml });
                    }
                    w.in_footnote = false;
                }
                TagEnd::Emphasis => w.fmt.italic = w.fmt.italic.saturating_sub(1),
                TagEnd::Strong => w.fmt.bold = w.fmt.bold.saturating_sub(1),
                TagEnd::Strikethrough => w.fmt.strike = w.fmt.strike.saturating_sub(1),
                TagEnd::Link => w.fmt.link = None,
                TagEnd::Image => {
                    if let Some((src, alt)) = w.image.take() {
                        w.image_ref(&src, &alt, (None, None), None, resolve);
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
                // Den osynliga tecknet-referensen framför en markör i radens början.
                let t: CowStr<'_> = if t.contains('\u{200B}') { t.replace('\u{200B}', "").into() } else { t };
                if w.in_code_block {
                    let t = t.to_string();
                    w.run(&t, true);
                } else {
                    w.run(&t, false);
                }
            }
            MdEvent::Code(t) => w.run(&t, true),
            MdEvent::InlineMath(t) => w.math(&t, false),
            MdEvent::DisplayMath(t) => w.math(&t, true),
            MdEvent::Html(t) | MdEvent::InlineHtml(t) => {
                let s = t.to_string();
                let single_line = s.trim().lines().count() == 1;
                if let Some(ns) = parse_tokens(&s) {
                    for n in ns {
                        w.token(n);
                    }
                } else if let Some((start, id)) = review::parse_marker(s.trim()) {
                    w.comment_marker(start, id);
                } else if let Some((kind, id)) = review::parse_pmark_exact(s.trim()) {
                    if kind == RevKind::Fmt {
                        w.fchange_para = Some(id);
                    } else {
                        w.pending_pm.push((kind, id));
                    }
                } else if let Some((kind, id)) = review::parse_open_tag(s.trim()) {
                    if kind == RevKind::Fmt {
                        w.fchange = Some(id);
                    } else {
                        w.rev = Some((kind, id));
                    }
                } else if review::is_close_tag(s.trim()) {
                    if s.trim() == "</fmt>" {
                        w.fchange = None;
                    } else {
                        w.rev = None;
                    }
                } else if let Some(img) = parse_img(&s) {
                    w.image_ref(&img.src, &img.alt, (img.width, img.height), img.align, resolve);
                } else if s.trim() == crate::word::nav::TOC_LINE && w.table.is_none() && w.quote == 0 {
                    w.close_para();
                    w.toc(md);
                } else if sections::SectionProps::from_comment(&s).is_some() && w.table.is_none() && w.quote == 0 {
                    // Avsnittsslut: ett tomt stycke med `w:sectPr` (egenskaperna sätts av `fix_sections`).
                    w.close_para();
                    if std::mem::take(&mut w.ctx.lead_section) {
                        // Första avsnittets egenskaper: ingen brytning.
                    } else {
                        w.body.push_str(SECT_PARA);
                    }
                } else if sections::is_column_break(&s) && w.table.is_none() && w.quote == 0 {
                    w.close_para();
                    w.body.push_str("<w:p><w:r><w:br w:type=\"column\"/></w:r></w:p>");
                } else if s.trim().eq_ignore_ascii_case("<br>") || s.trim().eq_ignore_ascii_case("<br/>") {
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
            MdEvent::FootnoteReference(t) if w.in_footnote => w.run(&format!("[{t}]"), false),
            MdEvent::FootnoteReference(t) => {
                // Id:t sätts när brödtexten och fotnoterna knyts ihop (`resolve_footnotes`).
                w.open_para();
                w.fn_order.push(t.to_string());
                let n = w.fn_order.len() - 1;
                let ph = format!("\u{E004}{n}\u{E005}");
                w.body.push_str(&format!(
                    "<w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteReference w:id=\"{ph}\"/></w:r>"
                ));
            }
        }
        match ended {
            EvEnd::None => {}
            EvEnd::Item => {
                if let Some(it) = items.pop().filter(|i| i.collecting) {
                    w.close_para();
                    finish_item(&mut w, md, it.desc, it.start, it.content);
                }
            }
            code => {
                // Citat avslutas först när det yttersta citatet är slut.
                if cur.as_ref().is_some_and(|c| c.code == code && (code != EvEnd::Quote || w.quote == 0)) {
                    if let Some(c) = cur.take() {
                        w.close_para();
                        let key = norm(md.get(c.src.clone()).unwrap_or(""));
                        // Ett HTML-block är bara en enhet när det är en ensam bild.
                        let ok = code != EvEnd::Html
                            || (key.to_ascii_lowercase().starts_with("<img") && !key.contains('\n'))
                            || key == crate::word::nav::TOC_LINE
                            || sections::SectionProps::from_comment(&key).is_some()
                            || sections::is_column_break(&key);
                        if ok && !key.is_empty() {
                            w.units.push(GenUnit { desc: c.desc, key, range: c.start..w.body.len() });
                        }
                    }
                }
            }
        }
    }
    w.close_para();
    w
}

/// Textytans bredd på en A4-sida med 2,54 cm marginaler, i EMU.
const MAX_IMAGE_WIDTH_EMU: i64 = 9026 * 635;
const EMU_PER_PIXEL: i64 = 9525; // 96 dpi

fn image_extent(bytes: &[u8], extent: Option<(i64, i64)>, max_width: i64) -> (i64, i64) {
    let max_width = if max_width > 0 { max_width } else { MAX_IMAGE_WIDTH_EMU };
    let (mut cx, mut cy) = extent.unwrap_or_else(|| match imagesize::blob_size(bytes) {
        Ok(sz) => (sz.width as i64 * EMU_PER_PIXEL, sz.height as i64 * EMU_PER_PIXEL),
        Err(_) => (MAX_IMAGE_WIDTH_EMU / 2, MAX_IMAGE_WIDTH_EMU / 3),
    });
    cx = cx.max(1);
    cy = cy.max(1);
    if cx > max_width {
        cy = cy * max_width / cx;
        cx = max_width;
    }
    (cx, cy.max(1))
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="jpeg" ContentType="image/jpeg"/><Default Extension="gif" ContentType="image/gif"/><Default Extension="bmp" ContentType="image/bmp"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#;

/// Märker en innehållsförteckning som Kladd skrivit (då begärs en uppdatering av fälten).
const TOC_TAG: &str = "kladd-toc";

/// Ber Word uppdatera fälten (innehållsförteckningen) när dokumentet öppnas.
fn with_update_fields(settings: &str) -> String {
    if settings.contains("w:updateFields") {
        return settings.to_owned();
    }
    // Schemat föreskriver ordningen: `updateFields` står före dessa element.
    let before = ["<w:hdrShapeDefaults", "<w:footnotePr", "<w:endnotePr", "<w:compat", "<w:docVars", "<w:rsids", "<m:mathPr", "<w:attachedSchema", "<w:themeFontLang", "<w:clrSchemeMapping", "<w:doNotIncludeSubdocsInStats", "<w:doNotAutoCompressPictures", "<w:forceUpgrade", "<w:captions", "<w:readModeInkLockDown", "<w:smartTagType", "<sl:schemaLibrary", "<w:shapeDefaults", "<w:doNotEmbedSmartTags", "<w:decimalSymbol", "<w:listSeparator", "</w:settings>"];
    let at = before.iter().filter_map(|m| settings.find(m)).min();
    match at {
        Some(i) => format!("{}<w:updateFields w:val=\"true\"/>{}", &settings[..i], &settings[i..]),
        None => settings.to_owned(),
    }
}

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
    s.push_str(&kladd_styles());
    s.push_str("</w:styles>");
    s
}

/// De formatmallar Kladd själv använder (rubriker, citat, kod, länk, tabellrutnät).
fn kladd_styles() -> String {
    let mut s = String::new();
    let heads = [(1, 36, "1F3864"), (2, 30, "2F5496"), (3, 26, "2F5496"), (4, 24, "2F5496"), (5, 22, "2F5496"), (6, 22, "404040")];
    for (n, size, color) in heads {
        s.push_str(&heading_style(n, size, color));
    }
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="TOCHeading"><w:name w:val="TOC Heading"/><w:basedOn w:val="Heading1"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:qFormat/><w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>"#);
    for (n, ind) in [(1, 0), (2, 220), (3, 440)] {
        s.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="TOC{n}"><w:name w:val="toc {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:autoRedefine/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9016"/></w:tabs><w:spacing w:after="100"/><w:ind w:left="{ind}"/></w:pPr></w:style>"#
        ));
    }
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="29"/><w:qFormat/><w:pPr><w:spacing w:before="120"/><w:ind w:left="567" w:right="567"/><w:pBdr><w:left w:val="single" w:sz="12" w:space="8" w:color="A0A0A0"/></w:pBdr></w:pPr><w:rPr><w:i/><w:color w:val="404040"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="34"/><w:qFormat/><w:pPr><w:spacing w:after="60"/><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="39"/><w:qFormat/><w:pPr><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style><w:style w:type="character" w:styleId="CodeChar"><w:name w:val="Code Char"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="39"/><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:rPr></w:style><w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style><w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:uiPriority w:val="39"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style>"#);
    s
}

/// Delar upp en lista formatmallar i (styleId, XML).
fn split_styles(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<w:style ") {
        let Some(len) = rest[i..].find("</w:style>") else { break };
        let one = &rest[i..i + len + "</w:style>".len()];
        if let Some(a) = one.find("w:styleId=\"") {
            let id = &one[a + 11..];
            if let Some(b) = id.find('"') {
                out.push((id[..b].to_owned(), one.to_owned()));
            }
        }
        rest = &rest[i + len + "</w:style>".len()..];
    }
    out
}

/// Kladds listdefinitioner: (abstractNum-elementen, num-elementen). Punktlistan får numId
/// `num_base` och de numrerade `num_base + 1 ...`.
fn numbering_defs(abs_bullet: u32, abs_ordered: u32, num_base: u32, ordered_starts: &[u64]) -> (String, String) {
    let mut a = format!(r#"<w:abstractNum w:abstractNumId="{abs_bullet}"><w:multiLevelType w:val="hybridMultilevel"/>"#);
    let bullets = ["•", "◦", "▪"];
    for lvl in 0..9 {
        let b = bullets[lvl % 3];
        let left = 720 * (lvl + 1);
        a.push_str(&format!(
            r#"<w:lvl w:ilvl="{lvl}"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="{b}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#
        ));
    }
    a.push_str(&format!(r#"</w:abstractNum><w:abstractNum w:abstractNumId="{abs_ordered}"><w:multiLevelType w:val="hybridMultilevel"/>"#));
    let fmts = ["decimal", "lowerLetter", "lowerRoman"];
    for lvl in 0..9 {
        let f = fmts[lvl % 3];
        let left = 720 * (lvl + 1);
        a.push_str(&format!(
            r#"<w:lvl w:ilvl="{lvl}"><w:start w:val="1"/><w:numFmt w:val="{f}"/><w:lvlText w:val="%{}."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#,
            lvl + 1
        ));
    }
    a.push_str("</w:abstractNum>");
    let mut n = format!(r#"<w:num w:numId="{num_base}"><w:abstractNumId w:val="{abs_bullet}"/></w:num>"#);
    for (i, start) in ordered_starts.iter().enumerate() {
        n.push_str(&format!(
            r#"<w:num w:numId="{}"><w:abstractNumId w:val="{abs_ordered}"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="{}"/></w:lvlOverride></w:num>"#,
            num_base + 1 + i as u32,
            start
        ));
    }
    (a, n)
}

fn numbering_xml(ordered_starts: &[u64]) -> String {
    let (a, n) = numbering_defs(0, 1, 1, ordered_starts);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{a}{n}</w:numbering>"#
    )
}

pub(crate) fn now_w3cdtf() -> String {
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

/// Uppdaterar `dcterms:modified` och `cp:lastModifiedBy` i en befintlig core.xml.
/// `None` om ingen av dem finns (då lämnas delen som den är).
fn touch_core(xml: &str, now: &str) -> Option<String> {
    fn set(xml: &str, open_prefix: &str, close: &str, value: &str) -> Option<String> {
        let start = xml.find(open_prefix)?;
        let gt = start + xml[start..].find('>')?;
        if xml[..gt].ends_with('/') {
            return None;
        }
        let end = gt + xml[gt..].find(close)?;
        Some(format!("{}{value}{}", &xml[..=gt], &xml[end..]))
    }
    let a = set(xml, "<dcterms:modified", "</dcterms:modified>", now);
    let b = set(a.as_deref().unwrap_or(xml), "<cp:lastModifiedBy", "</cp:lastModifiedBy>", "Kladd");
    b.or(a)
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

const XML_DECL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";

impl Source {
    /// En del i paketet (delnamn jämförs utan hänsyn till skiftläge).
    fn part(&self, name: &str) -> Option<&[u8]> {
        let name = name.trim_start_matches('/');
        self.parts.iter().find(|(n, _)| n.trim_start_matches('/').eq_ignore_ascii_case(name)).map(|(_, b)| b.as_slice())
    }
}

/// Textytans bredd i EMU för dokumentets sida och marginaler.
fn text_width_emu(d: &DocDefaults) -> i64 {
    let (w, _) = d.page();
    let m = d.margins();
    (((w - m[1] - m[3]).max(72.0)) as f64 * 12_700.0) as i64
}

/// Större tal efter varje förekomst av `needle` i `xml` (t.ex. `<w:num w:numId="`).
fn max_id(xml: &str, needle: &str) -> Option<u32> {
    let mut best = None;
    let mut rest = xml;
    while let Some(i) = rest.find(needle) {
        rest = &rest[i + needle.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u32>() {
            best = Some(best.map_or(n, |b: u32| b.max(n)));
        }
    }
    best
}

fn insert_before(xml: &str, marker: &str, add: &str) -> String {
    match xml.rfind(marker) {
        Some(i) => format!("{}{add}{}", &xml[..i], &xml[i..]),
        None => xml.to_owned(),
    }
}

fn twips(pt: f32) -> i64 {
    (pt * 20.0).round() as i64
}

/// Sätter (eller lägger till) ett attribut i en starttagg.
fn set_xml_attr(tag: &str, name: &str, val: &str) -> String {
    let pat = format!(" {name}=\"");
    if let Some(i) = tag.find(&pat) {
        let v = i + pat.len();
        if let Some(e) = tag[v..].find('"') {
            return format!("{}{}{}", &tag[..v], val, &tag[v + e..]);
        }
    }
    let close = if tag.ends_with("/>") { tag.len() - 2 } else { tag.len() - 1 };
    format!("{} {name}=\"{val}\"{}", tag[..close].trim_end(), &tag[close..])
}

fn remove_xml_attr(tag: &str, name: &str) -> String {
    let pat = format!(" {name}=\"");
    if let Some(i) = tag.find(&pat) {
        let v = i + pat.len();
        if let Some(e) = tag[v..].find('"') {
            return format!("{}{}", &tag[..i], &tag[v + e + 1..]);
        }
    }
    tag.to_owned()
}

/// Ändrar första taggen som börjar med `start` (`<w:pgSz`); saknas den läggs den till före `<w:cols`
/// eller slutet.
fn edit_tag(x: &str, start: &str, f: impl Fn(&str) -> String) -> String {
    if let Some(i) = x.find(start) {
        if let Some(j) = x[i..].find('>') {
            return format!("{}{}{}", &x[..i], f(&x[i..i + j + 1]), &x[i + j + 1..]);
        }
    }
    let tag = f(&format!("{start}/>"));
    if x.contains("<w:cols") {
        insert_before(x, "<w:cols", &tag)
    } else {
        insert_before(x, "</w:sectPr>", &tag)
    }
}

/// Tar bort `w:headerReference`/`w:footerReference` av typen "default".
fn remove_default_ref(x: &str, tag: &str, ty: &str) -> String {
    let mut out = String::new();
    let mut rest = x;
    while let Some(i) = rest.find(tag) {
        let Some(j) = rest[i..].find('>') else { break };
        let one = &rest[i..i + j + 1];
        out.push_str(&rest[..i]);
        if !one.contains(&format!("w:type=\"{ty}\"")) {
            out.push_str(one);
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

/// Ändringar av sidhuvud och sidfot i en sektion: `Some(None)` tar bort referensen, `Some(Some(id))`
/// byter den. `title_pg` slår på eller av annorlunda första sida.
#[derive(Default)]
struct SectEdit {
    hdr: Option<Option<String>>,
    hdr_first: Option<Option<String>>,
    ftr: Option<Option<String>>,
    ftr_first: Option<Option<String>>,
    title_pg: Option<bool>,
}

impl SectEdit {
    fn is_empty(&self) -> bool {
        self.hdr.is_none() && self.hdr_first.is_none() && self.ftr.is_none() && self.ftr_first.is_none() && self.title_pg.is_none()
    }
}

/// Anpassar originalets `w:sectPr` efter ändrad sida (`page`) eller ändrat sidhuvud/sidfot.
fn patch_sect(orig: &str, d: &DocDefaults, page: bool, edit: &SectEdit) -> String {
    let mut x = orig.to_owned();
    if x.trim_end().ends_with("/>") && !x.contains("</w:sectPr>") {
        x = format!("{}></w:sectPr>", x.trim_end().trim_end_matches("/>").trim_end());
    }
    if page {
        let (w, h) = d.page();
        let m = d.margins();
        x = edit_tag(&x, "<w:pgSz", |t| {
            let t = set_xml_attr(t, "w:w", &twips(w).to_string());
            let t = set_xml_attr(&t, "w:h", &twips(h).to_string());
            if w > h { set_xml_attr(&t, "w:orient", "landscape") } else { remove_xml_attr(&t, "w:orient") }
        });
        x = edit_tag(&x, "<w:pgMar", |t| {
            let mut t = t.to_owned();
            for (n, v) in ["top", "right", "bottom", "left"].into_iter().zip(m) {
                t = set_xml_attr(&t, &format!("w:{n}"), &twips(v).to_string());
            }
            t
        });
    }
    for (tag, ty, change) in [
        ("<w:footerReference", "first", &edit.ftr_first),
        ("<w:headerReference", "first", &edit.hdr_first),
        ("<w:footerReference", "default", &edit.ftr),
        ("<w:headerReference", "default", &edit.hdr),
    ] {
        let Some(new) = change.as_ref() else { continue };
        x = remove_default_ref(&x, tag, ty);
        if let Some(rid) = new {
            if let Some(open_end) = x.find('>') {
                x.insert_str(open_end + 1, &format!("{tag} w:type=\"{ty}\" r:id=\"{rid}\"/>"));
            }
        }
    }
    if let Some(on) = edit.title_pg {
        x = remove_title_pg(&x);
        if on {
            x = insert_title_pg(&x);
        }
    }
    x
}

/// Lägger `<w:titlePg/>` på rätt plats i en sektion.
fn insert_title_pg(x: &str) -> String {
    // `w:titlePg` står före textriktning, `w:docGrid` m.fl. i schemat.
    let at = ["<w:textDirection", "<w:bidi", "<w:rtlGutter", "<w:docGrid", "<w:printerSettings", "</w:sectPr>"]
        .iter()
        .filter_map(|m| x.find(m))
        .min();
    match at {
        Some(at) => format!("{}<w:titlePg/>{}", &x[..at], &x[at..]),
        None => x.to_owned(),
    }
}

/// Tar bort `<w:titlePg …/>` ur en sektion.
fn remove_title_pg(x: &str) -> String {
    let Some(i) = x.find("<w:titlePg") else { return x.to_owned() };
    let Some(j) = x[i..].find('>') else { return x.to_owned() };
    format!("{}{}", &x[..i], &x[i + j + 1..])
}

/// Relations-id:n för sidhuvuden och sidfötter som ska refereras.
#[derive(Default)]
struct HfRefs {
    hdr: Option<String>,
    hdr_first: Option<String>,
    ftr: Option<String>,
    ftr_first: Option<String>,
}

impl HfRefs {
    /// Skriver de delar av `d` som har text och ger deras id:n.
    fn make(extras: &mut Extras, d: &DocDefaults, f: &style::Fields, ids: &mut std::collections::HashSet<String>, taken: &mut std::collections::HashSet<String>) -> HfRefs {
        let mut one = |header: bool, text: Option<&str>| text.filter(|t| !t.is_empty()).map(|t| extras.header_footer(header, t, d, f, ids, taken));
        HfRefs {
            hdr: one(true, d.header.as_deref()),
            hdr_first: one(true, d.header_first.as_deref()),
            ftr: one(false, d.footer.as_deref()),
            ftr_first: one(false, d.footer_first.as_deref()),
        }
    }
}

/// Tomt stycke som avslutar ett avsnitt; `fix_sections` fyller i egenskaperna.
const SECT_PARA: &str = "<w:p><w:pPr><w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:pPr></w:p>";

fn same_pt(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

/// Tar bort första taggen som börjar med `start` (`<w:type`), hela elementet om det har innehåll.
fn remove_tag(x: &str, start: &str) -> String {
    let Some(i) = x.find(start) else { return x.to_owned() };
    let Some(j) = x[i..].find('>') else { return x.to_owned() };
    let mut end = i + j + 1;
    if !x[i..end].ends_with("/>") {
        let close = format!("</{}>", &start[1..]);
        if let Some(k) = x[end..].find(&close) {
            end += k + close.len();
        }
    }
    format!("{}{}", &x[..i], &x[end..])
}

/// Sätter sida, marginaler, spalter och avsnittstyp i en `w:sectPr` efter avsnittet `sec`. Är allt
/// redan lika lämnas XML:en orörd (förlustfri sparning). `refs` = sidhuvud- och sidfotsreferenser att
/// lägga först om `x` saknar egna (första avsnittet när brödtextens sectPr bär dem).
fn apply_section(orig: &str, sec: &sections::Section, refs: Option<&str>) -> String {
    let raw = read_sect(orig);
    let (w, h) = sec.page;
    let size_ok = raw.size.is_some_and(|(rw, rh)| same_pt(rw, w, 1.0) && same_pt(rh, h, 1.0));
    let mar_ok = raw.margin.is_some_and(|m| m.iter().zip(sec.margin).all(|(a, b)| same_pt(*a, b, 0.6)));
    let cols_ok = raw.cols == sec.columns && (sec.columns == 1 || raw.gap.is_some_and(|g| same_pt(g, sec.gap, 1.0)));
    let type_ok = raw.continuous == sec.continuous;
    let refs_add = refs.filter(|r| !r.is_empty() && !orig.contains("headerReference") && !orig.contains("footerReference"));
    if size_ok && mar_ok && cols_ok && type_ok && refs_add.is_none() {
        return orig.to_owned();
    }
    let mut x = orig.to_owned();
    if x.trim_end().ends_with("/>") && !x.contains("</w:sectPr>") {
        x = format!("{}></w:sectPr>", x.trim_end().trim_end_matches("/>").trim_end());
    }
    if !size_ok {
        x = edit_tag(&x, "<w:pgSz", |t| {
            let t = set_xml_attr(t, "w:w", &twips(w).to_string());
            let t = set_xml_attr(&t, "w:h", &twips(h).to_string());
            if w > h { set_xml_attr(&t, "w:orient", "landscape") } else { remove_xml_attr(&t, "w:orient") }
        });
    }
    if !mar_ok {
        x = edit_tag(&x, "<w:pgMar", |t| {
            let mut t = t.to_owned();
            for (n, v) in ["top", "right", "bottom", "left"].into_iter().zip(sec.margin) {
                t = set_xml_attr(&t, &format!("w:{n}"), &twips(v).to_string());
            }
            for n in ["header", "footer"] {
                if !t.contains(&format!("w:{n}=")) {
                    t = set_xml_attr(&t, &format!("w:{n}"), "708");
                }
            }
            if !t.contains("w:gutter=") {
                t = set_xml_attr(&t, "w:gutter", "0");
            }
            t
        });
    }
    if !cols_ok {
        x = remove_tag(&x, "<w:cols");
        let cols = if sec.columns > 1 {
            format!("<w:cols w:num=\"{}\" w:space=\"{}\"/>", sec.columns, twips(sec.gap))
        } else {
            "<w:cols w:space=\"708\"/>".to_owned()
        };
        // Schemat: cols kommer efter pgMar och före docGrid.
        x = if x.contains("<w:docGrid") {
            insert_before(&x, "<w:docGrid", &cols)
        } else {
            insert_before(&x, "</w:sectPr>", &cols)
        };
    }
    if !type_ok {
        x = remove_tag(&x, "<w:type");
        if sec.continuous {
            let tag = "<w:type w:val=\"continuous\"/>";
            x = if x.contains("<w:pgSz") { insert_before(&x, "<w:pgSz", tag) } else { insert_before(&x, "</w:sectPr>", tag) };
        }
    }
    if let Some(r) = refs_add {
        if let Some(open_end) = x.find('>') {
            x.insert_str(open_end + 1, r);
        }
        if r.contains("w:type=\"first\"") && !x.contains("<w:titlePg") {
            x = insert_title_pg(&x);
        }
    }
    x
}

/// Alla sidhuvuds- och sidfotsreferenser (standard och första sida) i en `w:sectPr`.
fn header_refs(sect: &str) -> String {
    let mut out = String::new();
    for tag in ["<w:headerReference", "<w:footerReference"] {
        let mut rest = sect;
        while let Some(i) = rest.find(tag) {
            let Some(j) = rest[i..].find('>') else { break };
            let one = &rest[i..i + j + 1];
            if one.contains("w:type=\"default\"") || one.contains("w:type=\"first\"") {
                out.push_str(one);
            }
            rest = &rest[i + j + 1..];
        }
    }
    out
}

/// Referenserna för sektionen `sect` med originalets första avsnitt (`base`) som reserv per plats:
/// en plats som ändrats (`changed`: huvud, huvud första, fot, fot första) tas bara från `sect`.
fn merge_refs(sect: &str, base: &str, changed: [bool; 4]) -> String {
    let (own, old) = (header_refs(sect), base);
    let mut out = String::new();
    for (k, (tag, ty)) in [("<w:headerReference", "default"), ("<w:headerReference", "first"), ("<w:footerReference", "default"), ("<w:footerReference", "first")].into_iter().enumerate() {
        let pick = |from: &str| {
            let mut rest = from;
            while let Some(i) = rest.find(tag) {
                let Some(j) = rest[i..].find('>') else { break };
                let one = &rest[i..i + j + 1];
                if one.contains(&format!("w:type=\"{ty}\"")) {
                    return Some(one.to_owned());
                }
                rest = &rest[i + j + 1..];
            }
            None
        };
        if let Some(one) = pick(&own).or_else(|| if changed[k] { None } else { pick(old) }) {
            out.push_str(&one);
        }
    }
    out
}

/// Sätter egenskaperna för avsnittens slut (`w:sectPr` i stycken, i ordning) efter avsnitten i texten.
/// `refs` = sidhuvuds- och sidfotsreferenserna som också ska gälla första avsnittet om det saknar egna.
fn fix_sections(body: &str, secs: &[sections::Section], refs: &str, replace_refs: bool) -> String {
    if secs.len() < 2 || !body.contains("<w:sectPr") {
        return body.to_owned();
    }
    let mut out = String::with_capacity(body.len() + 256);
    let mut rest = body;
    let mut k = 0;
    while let Some(i) = rest.find("<w:sectPr") {
        let Some(j) = rest[i..].find('>') else { break };
        let open_end = i + j + 1;
        let end = if rest[i..open_end].ends_with("/>") {
            open_end
        } else {
            match rest[open_end..].find("</w:sectPr>") {
                Some(e) => open_end + e + "</w:sectPr>".len(),
                None => break,
            }
        };
        out.push_str(&rest[..i]);
        let mut one = rest[i..end].to_owned();
        // Ändrat sidhuvud eller sidfot gäller alla avsnitt som hade egna referenser.
        if replace_refs && !one.ends_with("/>") && (one.contains("<w:headerReference") || one.contains("<w:footerReference")) {
            for (tag, ty) in [("<w:headerReference", "default"), ("<w:headerReference", "first"), ("<w:footerReference", "default"), ("<w:footerReference", "first")] {
                one = remove_default_ref(&one, tag, ty);
            }
            if let Some(oe) = one.find('>') {
                one.insert_str(oe + 1, refs);
            }
            one = remove_title_pg(&one);
            if refs.contains("w:type=\"first\"") {
                one = insert_title_pg(&one);
            }
        }
        let one = one.as_str();
        match secs.get(k).filter(|_| k + 1 < secs.len()) {
            Some(sec) => out.push_str(&apply_section(one, sec, (k == 0).then_some(refs))),
            None => out.push_str(one),
        }
        rest = &rest[end..];
        k += 1;
    }
    out.push_str(rest);
    out
}

/// Ny `w:sectPr` av DocDefaults.
fn sect_pr_new(d: &DocDefaults, refs: &HfRefs) -> String {
    let (w, h) = d.page();
    let m = d.margins();
    let mut s = String::from("<w:sectPr>");
    for (tag, ty, id) in [
        ("header", "default", &refs.hdr),
        ("header", "first", &refs.hdr_first),
        ("footer", "default", &refs.ftr),
        ("footer", "first", &refs.ftr_first),
    ] {
        if let Some(id) = id {
            s.push_str(&format!("<w:{tag}Reference w:type=\"{ty}\" r:id=\"{id}\"/>"));
        }
    }
    let orient = if w > h { " w:orient=\"landscape\"" } else { "" };
    s.push_str(&format!(
        "<w:pgSz w:w=\"{}\" w:h=\"{}\"{orient}/><w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/><w:cols w:space=\"708\"/>{title_pg}</w:sectPr>",
        twips(w),
        twips(h),
        twips(m[0]),
        twips(m[1]),
        twips(m[2]),
        twips(m[3]),
        title_pg = if d.different_first() { "<w:titlePg/>" } else { "" }
    ));
    s
}

/// Ett fält (sidnummer) som en följd körningar.
fn field_runs(instr: &str, shown: &str) -> String {
    format!(
        "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> {instr} </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>{shown}</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r>"
    )
}

/// Körningar för en text där fälten (`{sida}` …) blir Word-fält med värdena i `f` som förhandsvisning.
fn hf_runs(text: &str, f: &style::Fields) -> String {
    let fields: [(&str, &str, String); 5] = [
        ("{sida}", "PAGE", f.page.to_string()),
        ("{sidor}", "NUMPAGES", f.pages.to_string()),
        ("{datum}", "DATE \\@ \"yyyy-MM-dd\"", f.date.clone()),
        ("{titel}", "TITLE", f.title.clone()),
        ("{fil}", "FILENAME", f.file.clone()),
    ];
    let mut runs = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let next = fields.iter().filter_map(|(p, i, sh)| rest.find(p).map(|at| (at, p.len(), i, sh))).min_by_key(|(at, ..)| *at);
        let (plain, field) = match next {
            Some((at, len, i, sh)) => (&rest[..at], Some((len, i, sh))),
            None => (rest, None),
        };
        if !plain.is_empty() {
            let mut t = String::new();
            xml_escape(plain, &mut t);
            runs.push_str(&format!("<w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r>"));
        }
        match field {
            Some((len, i, sh)) => {
                let mut shown = String::new();
                xml_escape(sh, &mut shown);
                runs.push_str(&field_runs(i, &shown));
                rest = &rest[plain.len() + len..];
            }
            None => rest = "",
        }
    }
    runs
}

/// Ett sidhuvud (`hdr`) eller en sidfot (`ftr`) med ett stycke. Fälten blir Word-fält; en mall med
/// `|` blir vänster, mitten och höger med flikar (mitt och höger tabbstopp över textbredden
/// `text_w` i twips).
fn hf_part(root: &str, template: &str, align: Align, text_w: i64, f: &style::Fields) -> String {
    let (ppr, runs) = if style::split_parts(template).is_some() {
        let p = style::parts_of(template, align);
        let tab = "<w:r><w:tab/></w:r>";
        let ppr = format!("<w:pPr><w:tabs><w:tab w:val=\"center\" w:pos=\"{}\"/><w:tab w:val=\"right\" w:pos=\"{text_w}\"/></w:tabs></w:pPr>", text_w / 2);
        (ppr, format!("{}{tab}{}{tab}{}", hf_runs(&p[0], f), hf_runs(&p[1], f), hf_runs(&p[2], f)))
    } else {
        let [l, c, r] = style::parts_of(template, align);
        let text = [l, c, r].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
        let ppr = if align == Align::Left { String::new() } else { format!("<w:pPr><w:jc w:val=\"{}\"/></w:pPr>", align.as_jc()) };
        (ppr, hf_runs(&text, f))
    };
    format!("{XML_DECL}\n<w:{root} xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\"><w:p>{ppr}{runs}</w:p></w:{root}>")
}

/// Delar som läggs till i ett paket: relationer, innehållstyper och nya filer.
#[derive(Default)]
struct Extras {
    rels: String,
    types: String,
    parts: Vec<(String, Vec<u8>)>,
}

impl Extras {
    /// Lägger till sidhuvud/sidfot och ger dess relations-id.
    fn header_footer(&mut self, header: bool, text: &str, d: &DocDefaults, f: &style::Fields, ids: &mut std::collections::HashSet<String>, taken: &mut std::collections::HashSet<String>) -> String {
        let align = if header { d.header_align } else { d.footer_align };
        let text_w = twips(((d.page().0 - d.margins()[1] - d.margins()[3]).max(36.0)) as f32) as i64;
        let (stem, root, kind, mime) = if header {
            ("header", "hdr", "header", "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml")
        } else {
            ("footer", "ftr", "footer", "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml")
        };
        let mut n = 1;
        let name = loop {
            let name = format!("word/{stem}{n}.xml");
            if taken.insert(name.to_lowercase()) {
                break name;
            }
            n += 1;
        };
        let mut k = 1;
        let rid = loop {
            let id = format!("rIdKladd{}{k}", if header { "Hdr" } else { "Ftr" });
            if ids.insert(id.clone()) {
                break id;
            }
            k += 1;
        };
        self.rels.push_str(&format!(r#"<Relationship Id="{rid}" Type="{REL_NS}/{kind}" Target="{stem}{n}.xml"/>"#));
        self.types.push_str(&format!(r#"<Override PartName="/{name}" ContentType="{mime}"/>"#));
        self.parts.push((name, hf_part(root, text, align, text_w, f).into_bytes()));
        rid
    }
}

fn image_mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    }
}

/// Lägger till innehållstyper (`Default` för filändelser och `Override` för delar) som saknas.
fn patch_content_types(ct: &str, media_exts: &[&str], overrides: &str) -> String {
    let lower = ct.to_lowercase();
    let mut add = String::new();
    for ext in media_exts {
        if !lower.contains(&format!("extension=\"{ext}\"")) {
            add.push_str(&format!(r#"<Default Extension="{ext}" ContentType="{}"/>"#, image_mime(ext)));
        }
    }
    add.push_str(overrides);
    insert_before(ct, "</Types>", &add)
}

fn write_zip(parts: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let deflate = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(6))
            .unix_permissions(0o644);
        // Bilder är redan komprimerade och lagras som de är.
        let stored = deflate.compression_method(zip::CompressionMethod::Stored).compression_level(None);
        for (name, bytes) in parts {
            let lower = name.to_lowercase();
            let opts = if [".png", ".jpg", ".jpeg", ".gif"].iter().any(|e| lower.ends_with(e)) { stored } else { deflate };
            zip.start_file(name, opts).map_err(|e| e.to_string())?;
            zip.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf.into_inner())
}

/// Relationerna för hyperlänkar och nya bilder som `Writer` har skapat.
fn writer_rels(w: &Writer<'_>) -> String {
    let mut rels = String::new();
    for (id, url) in &w.rels {
        let mut u = String::new();
        xml_escape(url, &mut u);
        rels.push_str(&format!(r#"<Relationship Id="{id}" Type="{REL_NS}/hyperlink" Target="{u}" TargetMode="External"/>"#));
    }
    for (id, file, _) in &w.media {
        rels.push_str(&format!(r#"<Relationship Id="{id}" Type="{REL_NS}/image" Target="media/{file}"/>"#));
    }
    rels
}

fn media_exts<'a>(w: &'a Writer<'_>) -> Vec<&'a str> {
    let mut v: Vec<&str> = w.media.iter().filter_map(|(_, f, _)| f.rsplit('.').next()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Resultatet av att spara: filens innehåll och bilder som inte kunde bäddas in.
pub struct Written {
    pub bytes: Vec<u8>,
    pub missing_images: Vec<String>,
}

/// Bygger ett komplett .docx-paket av Markdown-text.
/// `resolve` hämtar bilddata för en bildkälla (`bild:namn`, en sökväg …).
/// Med `source` (det som `read` gav) skrivs bara det som ändrats om: alla övriga delar och
/// alla oförändrade block följer med orörda från originalet.
pub fn write(markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<Image>, source: Option<&Source>) -> Result<Written, String> {
    // Första raden kan vara dokumentets grundformat; den är inte text i dokumentet.
    let first = markdown.lines().next().unwrap_or("");
    let (defaults, markdown) = match DocDefaults::from_comment(first) {
        Some(d) => (d, markdown[first.len()..].trim_start_matches(['\n', '\r'])),
        None => (DocDefaults::default(), markdown),
    };
    // Ändrings- och kommentarsposterna sist i texten är inte text i dokumentet.
    let (markdown, revs, comments) = review::split_records(markdown);
    let rv = Review { revs, comments };
    match source {
        Some(src) => write_patched(src, defaults, &markdown, title, resolve, &rv),
        None => write_fresh(defaults, &markdown, title, resolve, &rv),
    }
}

/// Posterna för spårade ändringar och kommentarer ur Markdown-texten.
struct Review {
    revs: Vec<RevRec>,
    comments: Vec<CommentRec>,
}

impl Review {
    fn apply(&self, ctx: &mut Ctx<'_>) {
        ctx.revs = self.revs.iter().map(|r| (r.id, r.clone())).collect();
        ctx.comments = self.comments.iter().map(|c| c.id).collect();
    }
}

fn write_fresh(defaults: DocDefaults, markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<Image>, rv: &Review) -> Result<Written, String> {
    let mut taken: std::collections::HashSet<String> = [
        "[content_types].xml",
        "_rels/.rels",
        "word/document.xml",
        "word/_rels/document.xml.rels",
        "docprops/core.xml",
        "docprops/app.xml",
        "word/styles.xml",
        "word/numbering.xml",
        "word/settings.xml",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let secs = sections::scan(markdown, &defaults);
    let mut ctx = Ctx {
        num_base: 1,
        doc_pr_start: 1,
        max_width: text_width_emu(&defaults),
        used_ids: ["rId1", "rId2", "rId3"].into_iter().map(str::to_owned).collect(),
        lead_section: secs[0].marker.is_some(),
        ..Default::default()
    };
    rv.apply(&mut ctx);
    let mut w = body_from_markdown(markdown, resolve, ctx);
    let mut body = std::mem::take(&mut w.body);
    // Ett dokument som slutar med en tabell måste ha ett stycke efter den.
    if body.is_empty() || body.ends_with("</w:tbl>") {
        body.push_str("<w:p/>");
    }
    let (body, footnotes_xml) = resolve_footnotes(body, &w, &HashMap::new(), None);
    let comments_out = review_xml::build_comments(None, &rv.comments, &review_xml::anchored_ids(&body));
    let mut extras = Extras::default();
    let fields = style::Fields::for_export(title, "docx");
    let refs = HfRefs::make(&mut extras, &defaults, &fields, &mut w.ids, &mut taken);
    let sect = sect_pr_new(&defaults, &refs);
    let body = fix_sections(&body, &secs, &header_refs(&sect), false);
    // Sidhuvud och sidfot hör till första avsnittet (de senare ärver); annorlunda första sida gäller bara det.
    let sect = secs.last().map_or(sect.clone(), |s| apply_section(&sect, s, None));
    let sect = if secs.len() > 1 { remove_title_pg(&sect) } else { sect };
    let document = format!(
        r#"{XML_DECL}
<w:document xmlns:w="{W_NS}" xmlns:r="{R_NS}" xmlns:wp="{WP_NS}"><w:body>{body}{sect}</w:body></w:document>"#
    );
    let mut doc_rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>"#,
    );
    doc_rels.push_str(&writer_rels(&w));
    doc_rels.push_str(&extras.rels);
    if footnotes_xml.is_some() {
        doc_rels.push_str(&format!(r#"<Relationship Id="rIdKladdFn1" Type="{REL_NS}/footnotes" Target="footnotes.xml"/>"#));
        extras.types.push_str(&format!(r#"<Override PartName="/word/footnotes.xml" ContentType="{FOOTNOTES_MIME}"/>"#));
    }
    if let Some(c) = &comments_out {
        doc_rels.push_str(&format!(r#"<Relationship Id="rIdKladdCm1" Type="{REL_NS}/comments" Target="comments.xml"/>"#));
        extras.types.push_str(&format!(r#"<Override PartName="/word/comments.xml" ContentType="{}"/>"#, review_xml::COMMENTS_MIME));
        if c.ext.is_some() {
            doc_rels.push_str(&review_xml::ext_rel_xml("rIdKladdCm2", "commentsExtended.xml"));
            extras.types.push_str(&format!(r#"<Override PartName="/word/commentsExtended.xml" ContentType="{}"/>"#, review_xml::COMMENTS_EXT_MIME));
        }
    }
    doc_rels.push_str("</Relationships>");

    let mut parts: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), patch_content_types(CONTENT_TYPES, &[], &extras.types).into_bytes()),
        ("_rels/.rels".into(), ROOT_RELS.as_bytes().to_vec()),
        ("word/document.xml".into(), document.into_bytes()),
        ("word/_rels/document.xml.rels".into(), doc_rels.into_bytes()),
        ("docProps/core.xml".into(), core_xml(title, "").into_bytes()),
        ("docProps/app.xml".into(), APP_XML.as_bytes().to_vec()),
        ("word/styles.xml".into(), crate::docx_styles::patch_styles(&styles_xml(&defaults), &style::StyleSet::default(), &defaults.styles).into_bytes()),
        ("word/numbering.xml".into(), numbering_xml(&w.ordered_nums).into_bytes()),
        (
            "word/settings.xml".into(),
            if body.contains(TOC_TAG) { with_update_fields(SETTINGS) } else { SETTINGS.to_owned() }.into_bytes(),
        ),
    ];
    if let Some(xml) = footnotes_xml {
        parts.push(("word/footnotes.xml".into(), xml.into_bytes()));
        let settings = SETTINGS.replace("<w:compat>", r#"<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr><w:compat>"#);
        if let Some(p) = parts.iter_mut().find(|(n, _)| n == "word/settings.xml") {
            p.1 = settings.into_bytes();
        }
    }
    if let Some(c) = comments_out {
        parts.push(("word/comments.xml".into(), c.xml.into_bytes()));
        if let Some(e) = c.ext {
            parts.push(("word/commentsExtended.xml".into(), e.into_bytes()));
        }
    }
    parts.extend(extras.parts);
    for (_, file, bytes) in &w.media {
        parts.push((format!("word/media/{file}"), bytes.clone()));
    }
    Ok(Written { bytes: write_zip(parts)?, missing_images: w.missing })
}

/// Jämför två sidinställningar med rimlig marginal (mm-avrundning i texten).
fn same_page(a: &DocDefaults, b: &DocDefaults) -> bool {
    let (pa, pb) = (a.page(), b.page());
    let (ma, mb) = (a.margins(), b.margins());
    (pa.0 - pb.0).abs() < 1.0 && (pa.1 - pb.1).abs() < 1.0 && ma.iter().zip(mb).all(|(x, y)| (x - y).abs() < 0.6)
}

/// Har första sidans sidhuvud/sidfot ändrats? `None` = ingen egen första sida, `Some("")` = tom.
fn first_changed(new: Option<&str>, new_align: Align, old: Option<&str>, old_align: Align) -> bool {
    match (new, old) {
        (None, None) => false,
        (Some(a), Some(b)) => a != b || (!a.is_empty() && new_align != old_align),
        _ => true,
    }
}

/// Har sidhuvudets/sidfotens text eller justering ändrats?
fn hf_changed(new: Option<&str>, new_align: Align, old: Option<&str>, old_align: Align) -> bool {
    let (new, old) = (new.filter(|t| !t.is_empty()), old.filter(|t| !t.is_empty()));
    match (new, old) {
        (None, None) => false,
        (Some(a), Some(b)) => a != b || new_align != old_align,
        _ => true,
    }
}

/// Markörerna `<!--k:N-->` i en text.
fn tokens_in(s: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("<!--k:") {
        rest = &rest[i + 6..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse() {
            out.push(n);
        }
    }
    out
}

fn unit_hash(desc: &str, key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    desc.hash(&mut h);
    key.hash(&mut h);
    h.finish()
}

/// För varje element i `b`: indexet i `a` det matchats mot, enligt längsta gemensamma delföljd.
fn lcs_match(a: &[u64], b: &[u64]) -> Vec<Option<usize>> {
    let mut m = vec![None; b.len()];
    let mut lo = 0;
    while lo < a.len() && lo < b.len() && a[lo] == b[lo] {
        m[lo] = Some(lo);
        lo += 1;
    }
    let (mut ea, mut eb) = (a.len(), b.len());
    while ea > lo && eb > lo && a[ea - 1] == b[eb - 1] {
        ea -= 1;
        eb -= 1;
        m[eb] = Some(ea);
    }
    let (a2, b2) = (&a[lo..ea], &b[lo..eb]);
    if a2.is_empty() || b2.is_empty() {
        return m;
    }
    if a2.len().saturating_mul(b2.len()) <= 9_000_000 {
        let w = b2.len() + 1;
        let mut l = vec![0u32; (a2.len() + 1) * w];
        for i in (0..a2.len()).rev() {
            for j in (0..b2.len()).rev() {
                l[i * w + j] = if a2[i] == b2[j] { l[(i + 1) * w + j + 1] + 1 } else { l[(i + 1) * w + j].max(l[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < a2.len() && j < b2.len() {
            if a2[i] == b2[j] {
                m[lo + j] = Some(lo + i);
                i += 1;
                j += 1;
            } else if l[(i + 1) * w + j] >= l[i * w + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    } else {
        // Mycket långa dokument: girig matchning framåt.
        let mut pos: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, h) in a2.iter().enumerate() {
            pos.entry(*h).or_default().push(i);
        }
        let mut from = 0;
        for (j, h) in b2.iter().enumerate() {
            if let Some(&i) = pos.get(h).and_then(|v| v.iter().find(|&&i| i >= from)) {
                m[lo + j] = Some(lo + i);
                from = i + 1;
            }
        }
    }
    m
}

fn count_paras(x: &str) -> usize {
    x.matches("<w:p>").count() + x.matches("<w:p ").count()
}

/// Det nyskrivna stycket `made` med styckeegenskaperna (`w:pPr`) från originalstycket `orig`,
/// så att det som Kladd inte modellerar (egna formatmallar, avstånd, tabbar …) består.
fn with_orig_ppr(made: &str, orig: &str) -> Option<String> {
    if !made.starts_with("<w:p>") || !made.ends_with("</w:p>") || count_paras(made) != 1 || made.contains("<w:tbl") {
        return None;
    }
    if !orig.starts_with("<w:p") || count_paras(orig) != 1 || orig.contains("<w:tbl") {
        return None;
    }
    let open_end = orig.find('>')?;
    if orig[..open_end].ends_with('/') {
        return None;
    }
    let after = &orig[open_end + 1..];
    let ppr = if after.starts_with("<w:pPr>") { &after[..after.find("</w:pPr>")? + 8] } else { "" };
    let rest = made.strip_prefix("<w:p>")?;
    let rest = if rest.starts_with("<w:pPr>") { &rest[rest.find("</w:pPr>")? + 8..] } else { rest };
    Some(format!("<w:p>{ppr}{rest}"))
}

/// Platshållarens index för varje fotnotsreferens i `xml`, i ordning.
fn fn_placeholders(xml: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('\u{E004}') {
        rest = &rest[i + '\u{E004}'.len_utf8()..];
        let Some(e) = rest.find('\u{E005}') else { break };
        if let Ok(n) = rest[..e].parse() {
            out.push(n);
        }
        rest = &rest[e + '\u{E005}'.len_utf8()..];
    }
    out
}

/// Id:n för alla `w:footnoteReference` i `xml` (utom platshållare), i ordning.
fn footnote_ids_in(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<w:footnoteReference") {
        rest = &rest[i + 20..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        if let Some(a) = tag.find("w:id=\"") {
            let v = &tag[a + 6..];
            if let Some(b) = v.find('"') {
                if !v.starts_with('\u{E004}') {
                    out.push(v[..b].to_owned());
                }
            }
        }
        rest = &rest[end..];
    }
    out
}

/// Fotnotselementet för en definition som skrivits ur Markdown: första stycket får Words
/// fotnotsnummer och ett blanksteg före texten.
fn footnote_xml(id: &str, paras: &str) -> String {
    const LEAD: &str = "<w:pPr><w:pStyle w:val=\"FootnoteText\"/></w:pPr><w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space=\"preserve\"> </w:t></w:r>";
    let body = match paras.strip_prefix("<w:p>") {
        Some(rest) => {
            let rest = if rest.starts_with("<w:pPr>") { rest.find("</w:pPr>").map_or(rest, |e| &rest[e + 8..]) } else { rest };
            format!("<w:p>{LEAD}{rest}")
        }
        None => format!("<w:p>{LEAD}</w:p>"),
    };
    format!("<w:footnote w:id=\"{id}\">{body}</w:footnote>")
}

const FOOTNOTES_HEAD: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    "\n",
    r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#
);
const FOOTNOTE_SEPARATORS: &str = r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#;
const FOOTNOTES_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";

/// Knyter ihop fotnotsreferenserna i brödtexten med definitionerna: ger varje referens ett id,
/// byter platshållarna mot idn och bygger footnotes.xml. `bound` är referenser (index i
/// `w.fn_order`) som ligger i orört originalstycke och redan har sitt id. En fotnot vars text är
/// oförändrad får sin ursprungliga XML (med sitt id); övriga skrivs om. Svaret är den färdiga
/// brödtexten och, om dokumentet har fotnoter, hela footnotes.xml.
fn resolve_footnotes(body: String, w: &Writer<'_>, bound: &HashMap<usize, String>, orig: Option<&FnPart>) -> (String, Option<String>) {
    let present = footnote_ids_in(&body);
    if w.fn_order.is_empty() && present.is_empty() && orig.is_none() {
        return (body, None);
    }
    let items: &[FnOrig] = orig.map_or(&[], |o| o.items.as_slice());
    let normal = |id: &str| items.iter().find(|f| f.id == id && f.ty.is_none());
    // Nästa lediga id: över alla som finns eller används.
    let mut next_id = items
        .iter()
        .map(|f| f.id.as_str())
        .chain(present.iter().map(String::as_str))
        .chain(bound.values().map(String::as_str))
        .filter_map(|i| i.parse::<i64>().ok())
        .max()
        .unwrap_or(0)
        .max(0)
        + 1;
    let mut taken_ids: std::collections::HashSet<String> = present.iter().cloned().collect();
    let mut ids: Vec<String> = Vec::with_capacity(w.fn_order.len());
    let mut entries: Vec<(String, String)> = Vec::new(); // (id, XML)
    let mut label_done: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (i, label) in w.fn_order.iter().enumerate() {
        let def = w.footnotes.iter().find(|d| d.label == *label);
        let first = label_done.insert(label.as_str());
        let id = match bound.get(&i) {
            Some(id) => id.clone(),
            None => {
                // Oförändrad text: den ursprungliga fotnoten (om ingen annan redan använder den).
                let same = def.filter(|_| first).and_then(|d| {
                    items.iter().find(|f| f.ty.is_none() && !taken_ids.contains(&f.id) && norm(&f.md) == d.key).map(|f| f.id.clone())
                });
                same.unwrap_or_else(|| loop {
                    let c = next_id.to_string();
                    next_id += 1;
                    if !taken_ids.contains(&c) {
                        break c;
                    }
                })
            }
        };
        taken_ids.insert(id.clone());
        ids.push(id.clone());
        if bound.contains_key(&i) && !first {
            continue; // en andra referens i ett orört stycke: dess fotnot finns redan i originalet
        }
        let Some(def) = def else { continue };
        let xml = match normal(&id) {
            Some(f) if norm(&f.md) == def.key => f.xml.clone(),
            _ => footnote_xml(&id, &def.xml),
        };
        entries.push((id, xml));
    }
    // Platshållarna byts mot idn.
    let mut out = String::with_capacity(body.len());
    let mut rest = body.as_str();
    while let Some(i) = rest.find('\u{E004}') {
        out.push_str(&rest[..i]);
        let r = &rest[i + '\u{E004}'.len_utf8()..];
        let Some(e) = r.find('\u{E005}') else {
            rest = "";
            break;
        };
        if let Some(id) = r[..e].parse::<usize>().ok().and_then(|n| ids.get(n)) {
            out.push_str(id);
        }
        rest = &r[e + '\u{E005}'.len_utf8()..];
    }
    out.push_str(rest);

    if entries.is_empty() && orig.is_none() {
        return (out, None);
    }
    let emitted: std::collections::HashSet<&str> = entries.iter().map(|(i, _)| i.as_str()).collect();
    let mut xml = String::new();
    let (head, tail) = match orig {
        Some(o) => (o.head.as_str(), o.tail.as_str()),
        None => (FOOTNOTES_HEAD, "</w:footnotes>"),
    };
    xml.push_str(head);
    // Skiljelinjerna först, sedan fotnoterna i textens ordning, sist de som bara hänvisas till av okända objekt.
    if orig.is_none() {
        xml.push_str(FOOTNOTE_SEPARATORS);
    }
    for f in items.iter().filter(|f| f.ty.is_some()) {
        xml.push_str(&f.xml);
    }
    // Fotnoter som fanns förut behåller sin ordning i filen; nya kommer efter, i textens ordning.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&k| items.iter().position(|f| f.id == entries[k].0).unwrap_or(usize::MAX));
    for k in order {
        xml.push_str(&entries[k].1);
    }
    for f in items.iter().filter(|f| f.ty.is_none() && present.contains(&f.id) && !emitted.contains(f.id.as_str())) {
        xml.push_str(&f.xml);
    }
    xml.push_str(tail);
    (out, Some(xml))
}

/// Sätter ihop brödtexten: originalets XML för oförändrade block, nyskrivet för resten.
/// Returnerar texten och antalet fält som är öppna i slutet.
fn resolve_markers(segs: &[(bool, String)], tokens: &[Opaque], used: &mut std::collections::HashSet<usize>) -> (String, i32) {
    let mut out = String::new();
    let mut depth = 0i32;
    for (verbatim, text) in segs {
        if *verbatim {
            depth += text.matches("w:fldCharType=\"begin\"").count() as i32 - text.matches("w:fldCharType=\"end\"").count() as i32;
            out.push_str(text);
            continue;
        }
        let mut rest = text.as_str();
        while let Some(i) = rest.find('\u{E002}') {
            out.push_str(&rest[..i]);
            let r = &rest[i + '\u{E002}'.len_utf8()..];
            let Some(e) = r.find('\u{E003}') else {
                rest = "";
                break;
            };
            if let Some((n, t)) = r[..e].parse::<usize>().ok().and_then(|n| tokens.get(n).map(|t| (n, t))) {
                // Varje objekt skrivs högst en gång, och fält hålls balanserade.
                if depth + t.fld >= 0 && used.insert(n) {
                    out.push_str(&t.xml);
                    depth += t.fld;
                }
            }
            rest = &r[e + '\u{E003}'.len_utf8()..];
        }
        out.push_str(rest);
    }
    (out, depth)
}

/// Rensar deklarationen och ser till att de namnrymder nyskrivna delar använder finns.
fn fix_head(head: &str) -> String {
    let mut h = head.to_owned();
    if h.trim_start().starts_with("<?xml") {
        if let Some(e) = h.find("?>") {
            h = format!("{XML_DECL}{}", &h[e + 2..]);
        }
    } else {
        h = format!("{XML_DECL}\n{h}");
    }
    if let Some(i) = h.find("<w:document") {
        if let Some(j) = h[i..].find('>') {
            let tag = h[i..i + j].to_owned();
            let at = i + j - usize::from(tag.ends_with('/'));
            let mut add = String::new();
            if !tag.contains("xmlns:r=") {
                add.push_str(&format!(" xmlns:r=\"{R_NS}\""));
            }
            if !tag.contains("xmlns:wp=") {
                add.push_str(&format!(" xmlns:wp=\"{WP_NS}\""));
            }
            h.insert_str(at, &add);
        }
    }
    h
}

fn write_patched(src: &Source, defaults: DocDefaults, markdown: &str, title: &str, resolve: &dyn Fn(&str) -> Option<Image>, rv: &Review) -> Result<Written, String> {
    let main_dir = src.main.rsplit_once('/').map_or("", |(d, _)| d);
    let main_file = src.main.rsplit('/').next().unwrap_or("document.xml");
    let rels_name = if main_dir.is_empty() { format!("_rels/{main_file}.rels") } else { format!("{main_dir}/_rels/{main_file}.rels") };
    let rels_xml = src.part(&rels_name).map(decode_xml);
    let rels = rels_xml.as_deref().map(parse_rels).unwrap_or_default();
    let find_rel = |suffix: &str| rels.values().find(|(_, k, e)| !e && k.ends_with(suffix)).map(|(t, _, _)| resolve_target(main_dir, t));
    let styles_name = find_rel("/styles");
    let numbering_name = find_rel("/numbering");
    let numbering_orig = numbering_name.as_deref().and_then(|n| src.part(n)).map(decode_xml);

    // Listornas id får inte krocka med originalets.
    let (num_base, abs_bullet) = match &numbering_orig {
        Some(x) => (max_id(x, "<w:num w:numId=\"").map_or(1, |m| m + 1), max_id(x, "w:abstractNumId=\"").map_or(0, |m| m + 1)),
        None => (1, 0),
    };
    let doc_pr_start = src
        .elems
        .iter()
        .map(|e| e.xml.as_str())
        .chain(src.tokens.iter().map(|t| t.xml.as_str()))
        .filter_map(|x| max_id(x, "docPr id=\""))
        .max()
        .map_or(1, |m| m + 1);
    let mut taken: std::collections::HashSet<String> = src.parts.iter().map(|(n, _)| n.trim_start_matches('/').to_lowercase()).collect();
    let secs = sections::scan(markdown, &defaults);
    let mut ctx = Ctx {
        tokens: src.tokens.len(),
        lead_section: secs[0].marker.is_some(),
        num_base,
        doc_pr_start,
        max_width: text_width_emu(&defaults),
        taken: taken.clone(),
        used_ids: rels.keys().cloned().collect(),
        links: rels.iter().filter(|(_, (_, k, e))| *e && k.ends_with("/hyperlink")).map(|(id, (t, _, _))| (id.clone(), t.clone())).collect(),
        media: rels
            .iter()
            .filter(|(_, (_, k, e))| !*e && k.ends_with("/image"))
            .filter_map(|(id, (t, _, _))| src.part(&resolve_target(main_dir, t)).map(|b| (id.clone(), b)))
            .collect(),
        ..Default::default()
    };
    rv.apply(&mut ctx);
    let mut w = body_from_markdown(markdown, resolve, ctx);
    w.units.sort_by_key(|u| u.range.start);
    // Fotnotsreferenserna (index i `fn_order`) i varje enhet.
    let unit_occ: Vec<Vec<usize>> = w.units.iter().map(|u| fn_placeholders(&w.body[u.range.clone()])).collect();
    let mut bound: HashMap<usize, String> = HashMap::new();

    // ---- vilka block är oförändrade?
    let mut s_tok: Vec<(usize, usize)> = Vec::new();
    let mut s_hash: Vec<u64> = Vec::new();
    for (e, el) in src.elems.iter().enumerate() {
        for (p, (d, k)) in el.units.iter().enumerate() {
            s_tok.push((e, p));
            s_hash.push(unit_hash(d, k));
        }
    }
    let n_hash: Vec<u64> = w.units.iter().map(|u| unit_hash(&u.desc, &u.key)).collect();
    let mut m = lcs_match(&s_hash, &n_hash);
    // Ett element med flera block (t.ex. en innehållskontroll) tas bara om alla block är oförändrade
    // och ligger intill varandra.
    let mut first_tok = vec![0usize; src.elems.len()];
    let mut t = 0;
    for (e, el) in src.elems.iter().enumerate() {
        first_tok[e] = t;
        t += el.units.len();
    }
    let mut new_of_src: Vec<Option<usize>> = vec![None; s_tok.len()];
    for (j, mj) in m.iter().enumerate() {
        if let Some(i) = mj {
            // Hashen räcker inte som bevis.
            let (e, p) = s_tok[*i];
            let (d, k) = &src.elems[e].units[p];
            if *d == w.units[j].desc && *k == w.units[j].key {
                new_of_src[*i] = Some(j);
            }
        }
    }
    for (e, el) in src.elems.iter().enumerate() {
        let k = el.units.len();
        if k == 0 {
            continue;
        }
        let ft = first_tok[e];
        let js: Vec<Option<usize>> = (0..k).map(|p| new_of_src[ft + p]).collect();
        let mut ok = js[0].is_some() && (1..k).all(|p| js[p].is_some() && js[p] == js[p - 1].map(|x| x + 1));
        // Fotnotsreferenserna i originalet och i texten ska vara lika många, så att de kan paras ihop.
        if ok && js.iter().flatten().map(|&j| unit_occ[j].len()).sum::<usize>() != el.fn_ids.len() {
            ok = false;
        }
        if ok {
            let mut ids = el.fn_ids.iter();
            for &j in js.iter().flatten() {
                for &occ in &unit_occ[j] {
                    if let Some(id) = ids.next() {
                        bound.insert(occ, id.clone());
                    }
                }
            }
        } else {
            for p in 0..k {
                new_of_src[ft + p] = None;
            }
        }
    }
    m = vec![None; w.units.len()];
    for (i, nj) in new_of_src.iter().enumerate() {
        if let Some(j) = nj {
            m[*j] = Some(i);
        }
    }
    // ---- oförändrade block får originalets XML; ändrade stycken behåller originalets styckeegenskaper
    #[derive(Clone, Copy)]
    enum Slot {
        Gen,
        Orig(usize),
        Skip,
        Paired(usize),
    }
    let mut slots = vec![Slot::Gen; w.units.len()];
    for (e, el) in src.elems.iter().enumerate() {
        for p in 0..el.units.len() {
            if let Some(j) = new_of_src[first_tok[e] + p] {
                slots[j] = if p == 0 { Slot::Orig(e) } else { Slot::Skip };
            }
        }
    }
    // Parar ihop ändrade stycken med originalstycken i samma lucka och av samma slag.
    let mut next_matched = vec![usize::MAX; w.units.len() + 1];
    for j in (0..w.units.len()).rev() {
        next_matched[j] = m[j].unwrap_or(next_matched[j + 1]);
    }
    let mut paired_src = vec![false; s_tok.len()];
    let mut prev_s: isize = -1;
    let mut sp = 0usize;
    for j in 0..w.units.len() {
        if let Some(i) = m[j] {
            prev_s = i as isize;
            sp = i + 1;
            continue;
        }
        sp = sp.max((prev_s + 1) as usize);
        let end = next_matched[j].min(s_tok.len());
        for t in sp..end {
            let (e, p) = s_tok[t];
            let el = &src.elems[e];
            if new_of_src[t].is_none() && !paired_src[t] && el.units.len() == 1 && el.units[p].0 == w.units[j].desc {
                paired_src[t] = true;
                slots[j] = Slot::Paired(e);
                sp = t + 1;
                break;
            }
        }
    }
    // Tysta element (tomma stycken, bokmärken, sidbrytningar) följer med närmast föregående block.
    let mut after: Vec<Vec<usize>> = vec![Vec::new(); w.units.len()];
    let mut leading: Vec<usize> = Vec::new();
    let mut cursor: Option<usize> = None;
    for (e, el) in src.elems.iter().enumerate() {
        if el.units.is_empty() {
            match cursor {
                Some(j) => after[j].push(e),
                None => leading.push(e),
            }
            continue;
        }
        let ft = first_tok[e];
        if let Some(j) = new_of_src[ft + el.units.len() - 1] {
            cursor = Some(j);
        } else if el.units.len() == 1 {
            if let Some(j) = slots.iter().position(|s| matches!(s, Slot::Paired(x) if *x == e)) {
                cursor = Some(j);
            }
        }
    }
    // Markörer som redan finns i orörda block får inte skrivas en gång till.
    let mut used: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for s in &slots {
        if let Slot::Orig(e) = s {
            for (_, k) in &src.elems[*e].units {
                used.extend(tokens_in(k));
            }
        }
    }

    let mut segs: Vec<(bool, String)> = Vec::new();
    for e in &leading {
        segs.push((true, src.elems[*e].xml.clone()));
    }
    let mut at = 0usize;
    for (j, u) in w.units.iter().enumerate() {
        if u.range.start < at {
            continue;
        }
        segs.push((false, w.body[at..u.range.start].to_owned()));
        match slots[j] {
            Slot::Orig(e) => segs.push((true, src.elems[e].xml.clone())),
            Slot::Skip => {}
            Slot::Paired(e) => {
                let made = &w.body[u.range.clone()];
                segs.push((false, with_orig_ppr(made, &src.elems[e].xml).unwrap_or_else(|| made.to_owned())));
            }
            Slot::Gen => segs.push((false, w.body[u.range.clone()].to_owned())),
        }
        for e in &after[j] {
            segs.push((true, src.elems[*e].xml.clone()));
        }
        at = u.range.end;
    }
    segs.push((false, w.body[at..].to_owned()));
    let (mut body, open_fields) = resolve_markers(&segs, &src.tokens, &mut used);
    if open_fields > 0 {
        // Ett fält utan slut skulle göra dokumentet felaktigt.
        body.push_str("<w:p>");
        for _ in 0..open_fields {
            body.push_str("<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>");
        }
        body.push_str("</w:p>");
    }
    if body.is_empty() || body.ends_with("</w:tbl>") {
        body.push_str("<w:p/>");
    }
    let (body, footnotes_xml) = resolve_footnotes(body, &w, &bound, src.footnotes.as_ref());
    let comments_out = review_xml::build_comments(src.comments.as_ref(), &rv.comments, &review_xml::anchored_ids(&body));

    // ---- sida, sidhuvud och sidfot
    let page_changed = !same_page(&defaults, &src.page);
    let hdr_changed = hf_changed(defaults.header.as_deref(), defaults.header_align, src.page.header.as_deref(), src.page.header_align);
    let ftr_changed = hf_changed(defaults.footer.as_deref(), defaults.footer_align, src.page.footer.as_deref(), src.page.footer_align);
    let hdr1_changed = first_changed(defaults.header_first.as_deref(), defaults.header_align, src.page.header_first.as_deref(), src.page.header_align);
    let ftr1_changed = first_changed(defaults.footer_first.as_deref(), defaults.footer_align, src.page.footer_first.as_deref(), src.page.footer_align);
    let mut extras = Extras::default();
    let mut ids = w.ids.clone();
    let fields = style::Fields::for_export(title, "docx");
    // Utan originalets sektionsegenskaper skrivs allt om.
    let refs_changed = src.sect_pr.is_some() && (hdr_changed || ftr_changed || hdr1_changed || ftr1_changed);
    let sect = match &src.sect_pr {
        None => {
            let refs = HfRefs::make(&mut extras, &defaults, &fields, &mut ids, &mut taken);
            sect_pr_new(&defaults, &refs)
        }
        Some(orig) => {
            let mut edit = SectEdit::default();
            let mut make = |header: bool, text: Option<&str>| text.filter(|t| !t.is_empty()).map(|t| extras.header_footer(header, t, &defaults, &fields, &mut ids, &mut taken));
            if hdr_changed {
                edit.hdr = Some(make(true, defaults.header.as_deref()));
            }
            if ftr_changed {
                edit.ftr = Some(make(false, defaults.footer.as_deref()));
            }
            if hdr1_changed {
                edit.hdr_first = Some(make(true, defaults.header_first.as_deref()));
            }
            if ftr1_changed {
                edit.ftr_first = Some(make(false, defaults.footer_first.as_deref()));
            }
            if defaults.different_first() != src.page.different_first() {
                edit.title_pg = Some(defaults.different_first());
            }
            if !page_changed && edit.is_empty() { orig.clone() } else { patch_sect(orig, &defaults, page_changed, &edit) }
        }
    };
    let refs = merge_refs(&sect, &src.first_refs, [hdr_changed, hdr1_changed, ftr_changed, ftr1_changed]);
    let body = fix_sections(&body, &secs, &refs, refs_changed);
    let sect = secs.last().map_or(sect.clone(), |s| apply_section(&sect, s, None));
    let document = format!("{}{body}{sect}{}", fix_head(&src.head), src.tail);

    // ---- formatmallar och numrering: originalen kompletteras bara
    let mut replaced: HashMap<String, Vec<u8>> = HashMap::new();
    // Senast ändrad: datum och program uppdateras, resten av core.xml lämnas orört.
    if let Some(core) = src.part("docProps/core.xml").map(decode_xml).and_then(|x| touch_core(&x, &now_w3cdtf())) {
        replaced.insert("docprops/core.xml".into(), fix_decl(&core).into_bytes());
    }
    let mut doc_rel_add = writer_rels(&w);
    doc_rel_add.push_str(&extras.rels);
    let mut type_add = std::mem::take(&mut extras.types);
    let mut new_parts = std::mem::take(&mut extras.parts);
    let mut ids = ids;
    let mut fresh_part = |name: &str, kind: &str, mime: &str, xml: String, doc_rel_add: &mut String, type_add: &mut String| {
        let mut n = 0;
        let part = loop {
            let p = if n == 0 { name.to_owned() } else { format!("{}{n}.xml", name.trim_end_matches(".xml")) };
            if taken.insert(p.to_lowercase()) {
                break p;
            }
            n += 1;
        };
        let mut k = 1;
        let short = kind.rsplit('/').next().unwrap_or(kind);
        let rid = loop {
            let id = format!("rIdKladd{short}{k}");
            if ids.insert(id.clone()) {
                break id;
            }
            k += 1;
        };
        let target = part.strip_prefix(&format!("{main_dir}/")).unwrap_or(&part).to_owned();
        let rel_type = if kind.contains("://") { kind.to_owned() } else { format!("{REL_NS}/{kind}") };
        doc_rel_add.push_str(&format!(r#"<Relationship Id="{rid}" Type="{rel_type}" Target="{target}"/>"#));
        type_add.push_str(&format!(r#"<Override PartName="/{part}" ContentType="{mime}"/>"#));
        (part, xml)
    };
    match styles_name.as_deref().and_then(|n| src.part(n).map(|b| (n, decode_xml(b)))) {
        Some((name, xml)) if xml.contains("</w:styles>") => {
            let missing: String = split_styles(&kladd_styles())
                .into_iter()
                .filter(|(id, _)| !xml.contains(&format!("w:styleId=\"{id}\"")))
                .map(|(_, x)| x)
                .collect();
            let mut patched = if missing.is_empty() { xml.clone() } else { insert_before(&xml, "</w:styles>", &missing) };
            // Formatmallar som ändrats i Kladd skrivs in; oförändrade lämnas som de var.
            if defaults.styles != src.styles {
                patched = crate::docx_styles::patch_styles(&patched, &src.styles, &defaults.styles);
            }
            if patched != xml {
                replaced.insert(name.to_lowercase(), fix_decl(&patched).into_bytes());
            }
        }
        Some(_) => {}
        None => {
            let styles = crate::docx_styles::patch_styles(&styles_xml(&defaults), &style::StyleSet::default(), &defaults.styles);
            let (part, xml) = fresh_part("word/styles.xml", "styles", "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml", styles, &mut doc_rel_add, &mut type_add);
            new_parts.push((part, xml.into_bytes()));
        }
    }
    let (abs, nums) = numbering_defs(abs_bullet, abs_bullet + 1, num_base, &w.ordered_nums);
    match (numbering_name.as_deref(), numbering_orig) {
        (Some(name), Some(xml)) if xml.contains("</w:numbering>") => {
            let mut x = match xml.find("<w:num ") {
                Some(_) => {
                    let i = xml.find("<w:num ").unwrap_or(0);
                    format!("{}{abs}{}", &xml[..i], &xml[i..])
                }
                None => insert_before(&xml, "</w:numbering>", &abs),
            };
            // `w:numIdMacAtCleanup` ska stå sist.
            x = if x.contains("<w:numIdMacAtCleanup") { insert_before(&x, "<w:numIdMacAtCleanup", &nums) } else { insert_before(&x, "</w:numbering>", &nums) };
            replaced.insert(name.to_lowercase(), fix_decl(&x).into_bytes());
        }
        (Some(_), _) => {}
        (None, _) => {
            let xml = numbering_xml(&w.ordered_nums);
            let (part, xml) = fresh_part("word/numbering.xml", "numbering", "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml", xml, &mut doc_rel_add, &mut type_add);
            new_parts.push((part, xml.into_bytes()));
        }
    }

    // En ny innehållsförteckning har gissade sidnummer: be Word uppdatera fälten vid öppning.
    if body.contains(TOC_TAG) {
        if let Some(name) = find_rel("/settings") {
            if let Some(xml) = src.part(&name).map(decode_xml) {
                replaced.insert(name.to_lowercase(), fix_decl(&with_update_fields(&xml)).into_bytes());
            }
        }
    }

    // ---- fotnoter
    if let Some(xml) = footnotes_xml {
        match &src.footnotes {
            Some(o) => {
                replaced.insert(o.name.trim_start_matches('/').to_lowercase(), fix_decl(&xml).into_bytes());
            }
            None => {
                let (part, xml) = fresh_part("word/footnotes.xml", "footnotes", FOOTNOTES_MIME, xml, &mut doc_rel_add, &mut type_add);
                new_parts.push((part, xml.into_bytes()));
                // Words inställningar nämner skiljelinjerna (på toppen av en eventuell ändring ovan).
                if let Some((name, x)) = find_rel("/settings").and_then(|n| src.part(&n).map(|b| (n, decode_xml(b)))) {
                    let x = replaced.get(&name.to_lowercase()).map_or(x, |b| decode_xml(b));
                    if x.contains("</w:settings>") && !x.contains("<w:footnotePr") {
                        let add = r#"<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>"#;
                        let at = ["<w:endnotePr", "<w:compat", "<w:docVars", "<w:rsids", "<m:mathPr", "<w:themeFontLang", "<w:clrSchemeMapping", "</w:settings>"]
                            .iter()
                            .filter_map(|m| x.find(m))
                            .min()
                            .unwrap_or(0);
                        let patched = format!("{}{add}{}", &x[..at], &x[at..]);
                        replaced.insert(name.to_lowercase(), fix_decl(&patched).into_bytes());
                    }
                }
            }
        }
    }

    // ---- kommentarer
    if let Some(c) = comments_out {
        match &src.comments {
            Some(o) => {
                replaced.insert(o.name.trim_start_matches('/').to_lowercase(), fix_decl(&c.xml).into_bytes());
                if let Some(ext) = c.ext {
                    match &o.ext_name {
                        Some(n) => {
                            replaced.insert(n.trim_start_matches('/').to_lowercase(), fix_decl(&ext).into_bytes());
                        }
                        None => {
                            let (part, xml) = fresh_part(
                                "word/commentsExtended.xml",
                                "http://schemas.microsoft.com/office/2011/relationships/commentsExtended",
                                review_xml::COMMENTS_EXT_MIME,
                                ext,
                                &mut doc_rel_add,
                                &mut type_add,
                            );
                            new_parts.push((part, xml.into_bytes()));
                        }
                    }
                }
            }
            None => {
                let (part, xml) = fresh_part("word/comments.xml", "comments", review_xml::COMMENTS_MIME, c.xml, &mut doc_rel_add, &mut type_add);
                new_parts.push((part, xml.into_bytes()));
                if let Some(ext) = c.ext {
                    let (part, xml) = fresh_part(
                        "word/commentsExtended.xml",
                        "http://schemas.microsoft.com/office/2011/relationships/commentsExtended",
                        review_xml::COMMENTS_EXT_MIME,
                        ext,
                        &mut doc_rel_add,
                        &mut type_add,
                    );
                    new_parts.push((part, xml.into_bytes()));
                }
            }
        }
    }

    // ---- relationer och innehållstyper
    let rels_patched = match &rels_xml {
        Some(x) => insert_before(x, "</Relationships>", &doc_rel_add),
        None => format!("{XML_DECL}\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{doc_rel_add}</Relationships>"),
    };
    replaced.insert(rels_name.to_lowercase(), fix_decl(&rels_patched).into_bytes());
    let exts = media_exts(&w);
    if let Some(ct) = src.part("[Content_Types].xml") {
        replaced.insert("[content_types].xml".into(), fix_decl(&patch_content_types(&decode_xml(ct), &exts, &type_add)).into_bytes());
    }

    // ---- paketet: alla originaldelar, de ändrade utbytta
    let main_key = src.main.trim_start_matches('/').to_lowercase();
    let mut parts: Vec<(String, Vec<u8>)> = Vec::with_capacity(src.parts.len() + 4);
    let mut have_rels = false;
    for (name, bytes) in &src.parts {
        let key = name.trim_start_matches('/').to_lowercase();
        if key == main_key {
            parts.push((name.clone(), document.clone().into_bytes()));
        } else if let Some(new) = replaced.get(&key) {
            have_rels |= key == rels_name.to_lowercase();
            parts.push((name.clone(), new.clone()));
        } else {
            parts.push((name.clone(), bytes.clone()));
        }
    }
    if !have_rels {
        if let Some(new) = replaced.get(&rels_name.to_lowercase()) {
            parts.push((rels_name.clone(), new.clone()));
        }
    }
    parts.extend(new_parts);
    for (_, file, bytes) in &w.media {
        parts.push((format!("word/media/{file}"), bytes.clone()));
    }
    Ok(Written { bytes: write_zip(parts)?, missing_images: w.missing })
}

/// Byter deklarationen i en omskriven del mot en UTF-8-deklaration.
fn fix_decl(xml: &str) -> String {
    if xml.trim_start().starts_with("<?xml") {
        if let Some(e) = xml.find("?>") {
            return format!("{XML_DECL}{}", &xml[e + 2..]);
        }
    }
    format!("{XML_DECL}\n{xml}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Planering för fritids\n\nVi ska **baka** och *måla* i dag, se [Skolverket](https://www.skolverket.se).\n\n## Att ta med\n\n- Förkläde\n- Mjöl\n    - Vetemjöl\n\n1. Värm ugnen\n2. Blanda\n\n> Kom ihåg att tvätta händerna.\n\n| Dag | Aktivitet |\n| --- | --- |\n| Måndag | Bakning |\n| Tisdag | Utflykt |\n\nPris: 5\\*3 kr och `kod` här.\n";

    #[test]
    fn roundtrip_keeps_structure() {
        let bytes = write(SAMPLE, "Test", &|_| None, None).unwrap().bytes;
        let back = read(&bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        assert_eq!(back.markdown, SAMPLE, "\n--- fick ---\n{}", back.markdown);
    }

    fn rt(md: &str) -> String {
        let bytes = write(md, "T", &|_| None, None).unwrap().bytes;
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
        let bytes = write(text, "x", &|_| None, None).unwrap().bytes;
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
        let w = write(md, "x", &move |src| (src == "sol.png").then(|| Image { bytes: p2.clone(), extent: None }), None).unwrap();
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
        }, None)
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
        let w = write(md, "x", &|_| None, None).unwrap();
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
        let w = write("a b c\n", "x", &|_| None, None).unwrap();
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
        let w = write(STYLED, "x", &|_| None, None).unwrap();
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
        let doc = part_text(&write(md, "x", &|_| None, None).unwrap().bytes, "word/document.xml");
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
        let bytes = write(md, "x", &|_| None, None).unwrap().bytes;
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
        let w = write("x\n", "x", &|_| None, None).unwrap();
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
        let expected = "<!-- kladd: font-family: Cambria; font-size: 12pt; h1: \"Georgia\" 20pt #2F5496 -->

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
        assert_eq!(escape_line_start("\ttabb först"), "&#9;tabb först");
        assert_eq!(escape_line_start("  \ttabb"), "  &#9;tabb");
    }

    // ---------------------------------------------------------- förlustfri sparning

    const RICH_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;
    const P_HEAD: &str = r#"<w:p w:rsidR="00A1"><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:bookmarkStart w:id="0" w:name="_Toc1"/><w:r><w:t>Rapport</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#;
    const P_EDIT: &str = r#"<w:p><w:pPr><w:pStyle w:val="Brodtext2"/><w:spacing w:before="240" w:after="120"/><w:ind w:left="567"/></w:pPr><w:r><w:t xml:space="preserve">Första stycket med </w:t></w:r><w:commentRangeStart w:id="5"/><w:r><w:t>kommentar</w:t></w:r><w:commentRangeEnd w:id="5"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="5"/></w:r><w:r><w:t xml:space="preserve"> och fotnot</w:t></w:r><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="2"/></w:r><w:r><w:t>.</w:t></w:r></w:p>"#;
    const P_FIELD: &str = r#"<w:p><w:r><w:t xml:space="preserve">Sida </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    const P_BOX: &str = r#"<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:inline><wp:extent cx="100" cy="100"/><wp:docPr id="7" name="Textruta"/><a:graphic><a:graphicData uri="x"><wps:wsp><wps:txbx><w:txbxContent><w:p><w:r><w:t>Inuti rutan</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice><mc:Fallback><w:pict/></mc:Fallback></mc:AlternateContent></w:r><w:r><w:t>Efter rutan</w:t></w:r></w:p>"#;
    const P_SDT: &str = r#"<w:sdt><w:sdtPr><w:alias w:val="Kontroll"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Innehåll i kontroll</w:t></w:r></w:p></w:sdtContent></w:sdt>"#;
    const P_EMPTY: &str = r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#;
    const P_LAST: &str = r#"<w:p><w:r><w:t>Slutstycke</w:t></w:r></w:p>"#;
    const SECT: &str = r#"<w:sectPr><w:headerReference w:type="default" r:id="rId10"/><w:footerReference w:type="default" r:id="rId11"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1134" w:bottom="1417" w:left="1134" w:header="708" w:footer="708" w:gutter="0"/><w:cols w:space="708"/></w:sectPr>"#;
    const HEADER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t>Rapport</w:t></w:r></w:p></w:hdr>"#;
    const FOOTER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">Sida </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> av </w:t></w:r><w:fldSimple w:instr=" NUMPAGES "><w:r><w:t>9</w:t></w:r></w:fldSimple></w:p></w:ftr>"#;
    const FOOTNOTES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:id="2"><w:p><w:r><w:t>Fotnotstext</w:t></w:r></w:p></w:footnote></w:footnotes>"#;
    const COMMENTS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:comment w:id="5" w:author="Ada"><w:p><w:r><w:t>Kommentar</w:t></w:r></w:p></w:comment></w:comments>"#;
    const RICH_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Brodtext2"><w:name w:val="Brödtext 2"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="200"/></w:pPr></w:style></w:styles>"#;
    const RICH_NUMBERING: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="4"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="-"/></w:lvl></w:abstractNum><w:num w:numId="7"><w:abstractNumId w:val="4"/></w:num></w:numbering>"#;

    fn rich_document() -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {RICH_NS}><w:body>{P_HEAD}{P_EDIT}{P_FIELD}{P_BOX}{P_SDT}{P_EMPTY}{P_LAST}{SECT}</w:body></w:document>"#
        )
    }

    fn rich_docx() -> Vec<u8> {
        let rel = |id: &str, kind: &str, target: &str| format!(r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#);
        let rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}{}{}{}{}{}{}</Relationships>"#,
            rel("rId1", "styles", "styles.xml"),
            rel("rId2", "numbering", "numbering.xml"),
            rel("rId8", "footnotes", "footnotes.xml"),
            rel("rId9", "comments", "comments.xml"),
            rel("rId10", "header", "header1.xml"),
            rel("rId11", "footer", "footer1.xml"),
            rel("rId12", "settings", "settings.xml"),
        );
        let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/footnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml"/><Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/></Types>"#;
        let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        let parts: Vec<(&str, String)> = vec![
            ("[Content_Types].xml", types.to_owned()),
            ("_rels/.rels", root.to_owned()),
            ("word/document.xml", rich_document()),
            ("word/_rels/document.xml.rels", rels),
            ("word/styles.xml", RICH_STYLES.to_owned()),
            ("word/numbering.xml", RICH_NUMBERING.to_owned()),
            ("word/footnotes.xml", FOOTNOTES_XML.to_owned()),
            ("word/comments.xml", COMMENTS_XML.to_owned()),
            ("word/header1.xml", HEADER_XML.to_owned()),
            ("word/footer1.xml", FOOTER_XML.to_owned()),
            ("word/settings.xml", "<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"/>".to_owned()),
            ("docProps/app.xml", "<Properties/>".to_owned()),
        ];
        write_zip(parts.into_iter().map(|(n, c)| (n.to_owned(), c.into_bytes())).collect()).unwrap()
    }

    #[test]
    fn rich_document_reads_with_tokens_and_page_setup() {
        let docx = rich_docx();
        let l = read(&docx).unwrap();
        assert!(l.dropped.is_empty(), "{:?}", l.dropped);
        let src = l.source.as_ref().expect("källa");
        // Fotnot, kommentar, bokmärke och fält är markörer; texten syns.
        assert!(l.markdown.contains("<!--k:"), "{}", l.markdown);
        assert!(l.markdown.contains("Första stycket med"), "{}", l.markdown);
        assert!(l.markdown.contains("Sida "), "{}", l.markdown);
        assert!(l.markdown.contains("Efter rutan"), "{}", l.markdown);
        assert!(l.markdown.contains("Innehåll i kontroll"), "{}", l.markdown);
        assert!(!l.markdown.contains("Inuti rutan"));
        // Sida, sidhuvud och sidfot.
        let first = l.markdown.lines().next().unwrap();
        let d = DocDefaults::from_comment(first).expect("grundformat");
        assert_eq!(d.header.as_deref(), Some("Rapport"));
        assert_eq!(d.header_align, Align::Right);
        assert_eq!(d.footer.as_deref(), Some("Sida {sida} av {sidor}"));
        assert_eq!(d.footer_align, Align::Center);
        let m = d.margins();
        assert!((m[0] - 70.85).abs() < 0.5 && (m[1] - 56.7).abs() < 0.5, "{m:?}");
        assert!(src.sect_pr.is_some());
    }

    #[test]
    fn rich_document_edit_one_word_keeps_everything_else() {
        let docx = rich_docx();
        let l = read(&docx).unwrap();
        let edited = l.markdown.replace("Första", "Andra");
        assert_ne!(edited, l.markdown);
        let out = write(&edited, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        // Orörda delar är byte för byte lika.
        for (name, orig) in [
            ("word/footnotes.xml", FOOTNOTES_XML),
            ("word/comments.xml", COMMENTS_XML),
            ("word/header1.xml", HEADER_XML),
            ("word/footer1.xml", FOOTER_XML),
            ("docProps/app.xml", "<Properties/>"),
        ] {
            assert_eq!(part_text(&out, name), orig, "{name}");
        }
        let doc = part_text(&out, "word/document.xml");
        for p in [P_HEAD, P_FIELD, P_BOX, P_SDT, P_EMPTY, P_LAST, SECT] {
            assert!(doc.contains(p), "saknas: {p}\n{doc}");
        }
        // Det ändrade stycket har det nya ordet, behåller sina objekt och sina styckeegenskaper.
        assert!(doc.contains("Andra stycket med"), "{doc}");
        assert!(!doc.contains("Första"));
        for keep in [
            r#"<w:commentRangeStart w:id="5"/>"#,
            r#"<w:commentReference w:id="5"/>"#,
            r#"<w:footnoteReference w:id="2"/>"#,
            r#"<w:pStyle w:val="Brodtext2"/>"#,
            r#"<w:ind w:left="567"/>"#,
        ] {
            assert_eq!(doc.matches(keep).count(), 1, "{keep}\n{doc}");
        }
        // Och markörerna går runt: läser man om filen blir texten densamma.
        let again = read(&out).unwrap();
        assert!(again.dropped.is_empty());
        assert_eq!(again.markdown, edited);
        // Originaldelarna finns kvar i paketet och formatmallarna kompletterades bara.
        let styles = part_text(&out, "word/styles.xml");
        assert!(styles.contains("Brodtext2") && styles.contains("styleId=\"Quote\"") && styles.contains("styleId=\"CodeChar\""));
        assert_eq!(styles.matches("styleId=\"Heading1\"").count(), 1);
        let ct = part_text(&out, "[Content_Types].xml");
        assert!(ct.contains("/word/footnotes.xml") && ct.contains("/word/comments.xml"));
        let rels = part_text(&out, "word/_rels/document.xml.rels");
        assert!(rels.contains("rId8") && rels.contains("rId10"));
    }

    #[test]
    fn rich_document_unchanged_is_identical() {
        let docx = rich_docx();
        let l = read(&docx).unwrap();
        let out = write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        assert_eq!(part_text(&out, "word/document.xml"), rich_document());
        assert_eq!(part_text(&out, "word/styles.xml").contains("Brodtext2"), true);
        // Ingen ny sektion eller sidhuvud skapas.
        assert!(zip::ZipArchive::new(Cursor::new(&out)).unwrap().file_names().all(|n| n != "word/header2.xml"));
    }

    // ---------------------------------------------------------- sidhuvud och sidfot: fält, delar, första sidan

    const WNS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

    /// `rich_docx` med Words sidhuvud (vänster, titel och datum med flikar), en sidfot med bara ett
    /// högerställt sidnummer och en egen första sida (tomt sidhuvud, sidfot "Omslag").
    fn word_hf_docx() -> (Vec<u8>, String, String) {
        let hdr = format!(
            r#"<w:hdr xmlns:w="{WNS}"><w:p><w:pPr><w:tabs><w:tab w:val="center" w:pos="4680"/><w:tab w:val="right" w:pos="9360"/></w:tabs></w:pPr><w:r><w:t>Rapport</w:t></w:r><w:r><w:tab/></w:r><w:fldSimple w:instr=" TITLE "><w:r><w:t>Min titel</w:t></w:r></w:fldSimple><w:r><w:tab/></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> DATE \@ "d MMMM yyyy" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1 januari 2020</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:hdr>"#
        );
        let ftr = format!(
            r#"<w:ftr xmlns:w="{WNS}"><w:p><w:pPr><w:tabs><w:tab w:val="right" w:pos="9360"/></w:tabs></w:pPr><w:r><w:tab/></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:ftr>"#
        );
        let ftr1 = format!(r#"<w:ftr xmlns:w="{WNS}"><w:p><w:r><w:t>Omslag</w:t></w:r></w:p></w:ftr>"#);
        let hdr1 = format!(r#"<w:hdr xmlns:w="{WNS}"><w:p/></w:hdr>"#);
        let sect = SECT
            .replace(
                r#"<w:footerReference w:type="default" r:id="rId11"/>"#,
                r#"<w:footerReference w:type="default" r:id="rId11"/><w:headerReference w:type="first" r:id="rId13"/><w:footerReference w:type="first" r:id="rId14"/>"#,
            )
            .replace("</w:sectPr>", "<w:titlePg/></w:sectPr>");
        let mut b = patch_part(&rich_docx(), "word/header1.xml", |_| hdr.clone());
        b = patch_part(&b, "word/footer1.xml", |_| ftr.clone());
        b = patch_part(&b, "word/header2.xml", |_| hdr1.clone());
        b = patch_part(&b, "word/footer2.xml", |_| ftr1.clone());
        b = patch_part(&b, "word/document.xml", |x| x.replace(SECT, &sect));
        b = patch_part(&b, "word/_rels/document.xml.rels", |x| {
            x.replace(
                "</Relationships>",
                &format!(
                    r#"<Relationship Id="rId13" Type="{REL_NS}/header" Target="header2.xml"/><Relationship Id="rId14" Type="{REL_NS}/footer" Target="footer2.xml"/></Relationships>"#
                ),
            )
        });
        (b, hdr, ftr)
    }

    #[test]
    fn word_header_with_tabs_fields_and_first_page_imports() {
        let (docx, ..) = word_hf_docx();
        let l = read(&docx).unwrap();
        let d = DocDefaults::from_comment(l.markdown.lines().next().unwrap()).expect("grundformat");
        assert_eq!(d.header.as_deref(), Some("Rapport|{titel}|{datum}"));
        assert_eq!(d.footer.as_deref(), Some("||{sida}"));
        assert_eq!(d.header_first.as_deref(), Some(""));
        assert_eq!(d.footer_first.as_deref(), Some("Omslag"));
        assert!(d.different_first());
        // Utan `w:titlePg` ignoreras första sidans delar.
        let no_first = patch_part(&docx, "word/document.xml", |x| x.replace("<w:titlePg/>", ""));
        let d2 = DocDefaults::from_comment(read(&no_first).unwrap().markdown.lines().next().unwrap()).unwrap();
        assert!(!d2.different_first());
    }

    #[test]
    fn word_header_unchanged_keeps_original_parts_and_changes_are_patched() {
        let (docx, hdr, ftr) = word_hf_docx();
        let l = read(&docx).unwrap();
        let src = l.source.as_ref();
        // Oförändrat: dokument, sidhuvud och sidfot är orörda.
        let out = write(&l.markdown, "x", &|_| None, src).unwrap().bytes;
        assert_eq!(part_text(&out, "word/header1.xml"), hdr);
        assert_eq!(part_text(&out, "word/footer1.xml"), ftr);
        assert!(part_text(&out, "word/document.xml").contains("<w:titlePg/>"));
        // Nytt sidhuvud: en ny del ersätter standardreferensen, första sidan och `titlePg` består.
        let md = l.markdown.replacen("header: \"Rapport|{titel}|{datum}\"", "header: \"Ny|{titel}|{sida}\"", 1);
        assert_ne!(md, l.markdown);
        let out = write(&md, "x", &|_| None, src).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains("<w:titlePg/>") && doc.contains(r#"w:type="first""#), "{doc}");
        assert!(doc.contains(r#"<w:headerReference w:type="default" r:id="rIdKladdHdr1"/>"#), "{doc}");
        assert_eq!(part_text(&out, "word/footer1.xml"), ftr);
        let back = read(&out).unwrap();
        assert!(back.markdown.contains("header: \"Ny|{titel}|{sida}\""), "{}", back.markdown);
        // Utan första sida: referenserna och `titlePg` tas bort.
        let md = l.markdown.replace("; header-first: \"\"", "").replace("; footer-first: \"Omslag\"", "");
        assert!(!md.contains("-first"), "{md}");
        let out = write(&md, "x", &|_| None, src).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(!doc.contains("titlePg") && !doc.contains(r#"w:type="first""#), "{doc}");
        assert!(!read(&out).unwrap().markdown.contains("-first"));
    }

    #[test]
    fn written_header_has_fields_tabs_and_first_page_variant() {
        let md = "<!-- kladd: header: \"Rapport|{titel}|{datum}\"; footer: \"Sida {sida} av {sidor}\"; footer-align: center; header-first: \"\"; footer-first: \"Omslag\" -->\n\nText\n";
        let out = write(md, "Min rapport", &|_| None, None).unwrap().bytes;
        let hdr = part_text(&out, "word/header1.xml");
        assert!(hdr.contains(" TITLE ") && hdr.contains("DATE ") && hdr.contains("<w:tab/>"), "{hdr}");
        assert!(hdr.contains("w:val=\"center\"") && hdr.contains("w:val=\"right\""), "{hdr}");
        assert!(hdr.contains("<w:t>Min rapport</w:t>"), "förhandsvisning av titeln: {hdr}");
        let ftr = part_text(&out, "word/footer1.xml");
        assert!(ftr.contains(" PAGE ") && ftr.contains(" NUMPAGES ") && !ftr.contains("<w:tab/>"), "{ftr}");
        assert!(part_text(&out, "word/footer2.xml").contains("Omslag"));
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains("<w:titlePg/>") && doc.contains(r#"<w:footerReference w:type="first""#), "{doc}");
        // Tomt första sidhuvud ger ingen del, och allt läses tillbaka.
        assert!(!doc.contains(r#"<w:headerReference w:type="first""#), "{doc}");
        let want = DocDefaults::from_comment(md.lines().next().unwrap()).unwrap();
        let back = read(&out).unwrap();
        assert_eq!(DocDefaults::from_comment(back.markdown.lines().next().unwrap()), Some(want));
    }

    #[test]
    fn deleting_and_adding_blocks_keeps_matching() {
        let docx = rich_docx();
        let l = read(&docx).unwrap();
        // Stryk rubriken, lägg till ett nytt stycke i mitten.
        let body_start = l.markdown.find("# ").unwrap();
        let head_end = l.markdown[body_start..].find("\n\n").unwrap() + body_start + 2;
        let mut md = format!("{}{}", &l.markdown[..body_start], &l.markdown[head_end..]);
        md = md.replace("Slutstycke", "Nytt stycke i mitten\n\nSlutstycke");
        let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        for p in [P_EDIT, P_FIELD, P_BOX, P_SDT, P_EMPTY, P_LAST] {
            assert!(doc.contains(p), "saknas: {p}");
        }
        assert!(!doc.contains("Rapport"));
        assert!(doc.contains("Nytt stycke i mitten"));
    }

    #[test]
    fn edited_content_control_is_rewritten_not_lost() {
        let docx = rich_docx();
        let l = read(&docx).unwrap();
        let md = l.markdown.replace("Innehåll i kontroll", "Ändrat innehåll");
        let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains("Ändrat innehåll") && !doc.contains("Innehåll i kontroll"));
        assert!(doc.contains(P_LAST) && doc.contains(P_FIELD));
    }

    #[test]
    fn unchanged_blocks_of_ordinary_documents_are_kept() {
        let md = format!("{SAMPLE}\n```\nlet x = 1;\n```\n\n<div align=\"center\">\n\nMitt i\n\n</div>\n");
        let fresh = write(&md, "T", &|_| None, None).unwrap().bytes;
        // Märk varje stycke så att originalets XML går att skilja från nyskriven.
        let marked = patch_part(&fresh, "word/document.xml", |x| x.replace("<w:p>", "<w:p w:rsidR=\"00AB\">"));
        let n = part_text(&marked, "word/document.xml").matches("w:rsidR=\"00AB\"").count();
        assert!(n > 10);
        let l = read(&marked).unwrap();
        let again = write(&l.markdown, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
        assert_eq!(part_text(&again, "word/document.xml").matches("w:rsidR=\"00AB\"").count(), n);
        // Ändra ett ord i en punkt, en cell och en rubrik: bara de skrivs om, med listans numrering kvar.
        let md2 = l.markdown.replace("Mjöl", "Havregryn").replace("Bakning", "Pyssel").replace("Planering", "Plan");
        let out = write(&md2, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains("Havregryn") && doc.contains("Pyssel") && doc.contains("Plan för"));
        assert!(doc.matches("w:rsidR=\"00AB\"").count() < n);
        assert_eq!(read(&out).unwrap().markdown, md2);
    }

    const TOC_MD: &str = "# Inledning\n\nText.\n\n<!-- kladd: toc -->\n\n## Bakgrund\n\nMer text.\n\n### Detalj\n\n#### För djup\n";

    #[test]
    fn toc_line_writes_a_toc_field_and_reads_back() {
        let out = write(TOC_MD, "T", &|_| None, None).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains("w:docPartGallery w:val=\"Table of Contents\""), "{doc}");
        assert!(doc.contains(r#"TOC \o "1-3" \h \z \u"#), "{doc}");
        // H1–H3 finns i förteckningen, H4 inte.
        for h in ["Inledning", "Bakgrund", "Detalj"] {
            assert!(doc.contains(&format!(">{h}</w:t>")), "{h}");
        }
        assert!(part_text(&out, "word/settings.xml").contains("<w:updateFields w:val=\"true\"/>"));
        assert!(part_text(&out, "word/styles.xml").contains("w:styleId=\"TOC1\""));
        let l = read(&out).unwrap();
        assert_eq!(l.markdown, TOC_MD, "\n--- fick ---\n{}", l.markdown);
        // Orörd förteckning behåller originalets XML vid ny sparning.
        let marked = patch_part(&out, "word/document.xml", |x| x.replace("<w:sdt>", "<w:sdt w:rsidR=\"00AB\">").replace("<w:sdt><", "<w:sdt w:rsidR=\"00AB\"><"));
        let l = read(&marked).unwrap();
        assert_eq!(l.markdown, TOC_MD);
        let again = write(&l.markdown, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
        let doc = part_text(&again, "word/document.xml");
        assert_eq!(doc.matches("<w:sdt ").count(), 1, "{doc}");
        assert_eq!(doc.matches("Table of Contents").count(), 1);
    }

    /// Låter LibreOffice göra PDF av ett dokument med innehållsförteckning och kontrollerar att
    /// förteckningen syns; kör med `cargo test -- --ignored libreoffice_renders_toc`.
    #[test]
    #[ignore]
    fn libreoffice_renders_toc() {
        let dir = std::env::temp_dir().join("kladd-test-docx-toc");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = write(TOC_MD, "T", &|_| None, None).unwrap().bytes;
        let path = dir.join("toc.docx");
        std::fs::write(&path, &out).unwrap();
        let st = std::process::Command::new("soffice")
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(&dir)
            .arg(&path)
            .output()
            .expect("soffice saknas");
        assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
        let pdf = std::fs::read(dir.join("toc.pdf")).expect("ingen pdf skapades");
        let back = crate::pdf_read::read(&pdf, None).ok().expect("läsbar PDF");
        for h in ["Innehåll", "Inledning", "Bakgrund", "Detalj"] {
            assert!(back.markdown.contains(h), "{h} saknas:\n{}", back.markdown);
        }
        eprintln!("{}", back.markdown);
    }

    #[test]
    fn foreign_toc_sdt_and_bare_field_read_as_toc_line() {
        let sdt = r#"<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val="TOCHeading"/></w:pPr><w:r><w:t>Innehåll</w:t></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TOC \o "1-3" \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Rubrik 3</w:t></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:sdtContent></w:sdt>"#;
        let bare = r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">TOC \o "1-3"</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Rubrik 3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
        let base = write("# Rubrik\n\nXXX\n\nSlut.\n", "T", &|_| None, None).unwrap().bytes;
        for xml in [sdt, bare] {
            let patched = patch_part(&base, "word/document.xml", |x| x.replace("<w:p><w:r><w:t xml:space=\"preserve\">XXX</w:t></w:r></w:p>", xml).replace("<w:p><w:r><w:t>XXX</w:t></w:r></w:p>", xml));
            assert!(part_text(&patched, "word/document.xml").contains("TOC"), "mönstret hittades inte");
            let l = read(&patched).unwrap();
            assert_eq!(l.markdown, "# Rubrik\n\n<!-- kladd: toc -->\n\nSlut.\n", "{xml}");
            // Oförändrad text: originalets XML följer med.
            let again = write(&l.markdown, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
            assert!(part_text(&again, "word/document.xml").contains("Rubrik 3"));
            assert!(!part_text(&again, "word/document.xml").contains("kladd-toc"));
        }
    }

    #[test]
    fn page_setup_and_header_roundtrip_without_source() {
        let md = "<!-- kladd: page-size: Letter landscape; page-margin: 20mm 15mm; header: \"Rapport\"; header-align: right; footer: \"Sida {sida} av {sidor}\"; footer-align: center -->\n\nText\n";
        let out = write(md, "x", &|_| None, None).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains("w:orient=\"landscape\"") && doc.contains("w:w=\"15840\" w:h=\"12240\""), "{doc}");
        assert!(doc.contains("w:headerReference") && doc.contains("w:footerReference"));
        let footer = part_text(&out, "word/footer1.xml");
        assert!(footer.contains("PAGE") && footer.contains("NUMPAGES") && footer.contains("w:val=\"center\""), "{footer}");
        let back = read(&out).unwrap();
        let d = DocDefaults::from_comment(back.markdown.lines().next().unwrap()).unwrap();
        let want = DocDefaults::from_comment(md.lines().next().unwrap()).unwrap();
        assert_eq!(d.header, want.header);
        assert_eq!(d.footer, want.footer);
        assert_eq!((d.header_align, d.footer_align), (Align::Right, Align::Center));
        assert!((d.page().0 - 792.0).abs() < 1.0 && (d.page().1 - 612.0).abs() < 1.0);
        assert!((d.margins()[0] - want.margins()[0]).abs() < 0.6 && (d.margins()[1] - want.margins()[1]).abs() < 0.6);
        // Ett dokument utan sidinställningar får A4 och inga sidhuvuden.
        let plain = write("Text\n", "x", &|_| None, None).unwrap().bytes;
        let doc = part_text(&plain, "word/document.xml");
        assert!(doc.contains("w:w=\"11906\" w:h=\"16838\"") && !doc.contains("headerReference"));
    }

    #[test]
    fn page_setup_changes_patch_the_original_section() {
        let docx = rich_docx();
        let l = read(&docx).unwrap();
        // Oförändrat: originalets sektion och delar används.
        let same = write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        assert!(part_text(&same, "word/document.xml").contains(SECT));
        // Nytt sidfotstext, liggande sida och andra marginaler.
        let md = l.markdown.replacen(
            "footer: \"Sida {sida} av {sidor}\"",
            "footer: \"Sidan {sida}\"",
            1,
        );
        let md = md.replacen("<!-- kladd: ", "<!-- kladd: page-size: A4 landscape; ", 1);
        assert_ne!(md, l.markdown);
        let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains("w:orient=\"landscape\""), "{doc}");
        assert!(doc.contains("w:w=\"16838\" w:h=\"11906\"") || doc.contains("w:h=\"11906\""), "{doc}");
        // Sidhuvudet är orört, sidfoten ny.
        assert!(doc.contains(r#"<w:headerReference w:type="default" r:id="rId10"/>"#));
        assert!(!doc.contains(r#"r:id="rId11""#));
        assert_eq!(part_text(&out, "word/header1.xml"), HEADER_XML);
        let rels = part_text(&out, "word/_rels/document.xml.rels");
        assert!(rels.contains("footer2.xml") || rels.contains("footer1.xml") && rels.contains("rIdKladdFtr"), "{rels}");
        let again = read(&out).unwrap();
        let d = DocDefaults::from_comment(again.markdown.lines().next().unwrap()).unwrap();
        assert_eq!(d.footer.as_deref(), Some("Sidan {sida}"));
        assert_eq!(d.header.as_deref(), Some("Rapport"));
        assert!(d.page().0 > d.page().1);
        // Ta bort sidfoten helt.
        let md = l.markdown.replacen("; footer: \"Sida {sida} av {sidor}\"; footer-align: center", "", 1);
        let out = write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        assert!(!part_text(&out, "word/document.xml").contains("footerReference"));
    }

    #[test]
    fn image_width_roundtrip() {
        let png: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0, 1, 0, 0,
            5, 0, 1, 0x0D, 0x0A, 0x2D, 0xB4, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let p = png.clone();
        let resolve = move |src: &str| (src == "bild:a.png").then(|| Image { bytes: p.clone(), extent: None });
        let md = "<img src=\"bild:a.png\" alt=\"Sol &amp; måne\" width=\"240\">\n\nText med <img src=\"bild:a.png\" width=\"100\" height=\"50\"> inne.\n\n![Ren](bild:a.png)\n";
        let out = write(md, "x", &resolve, None).unwrap();
        assert!(out.missing_images.is_empty(), "{:?}", out.missing_images);
        let doc = part_text(&out.bytes, "word/document.xml");
        assert!(doc.contains(&format!("<wp:extent cx=\"{}\" cy=\"{}\"/>", 240 * 9525, 240 * 9525)), "{doc}");
        assert!(doc.contains(&format!("<wp:extent cx=\"{}\" cy=\"{}\"/>", 100 * 9525, 50 * 9525)));
        assert!(doc.contains(&format!("<wp:extent cx=\"{}\" cy=\"{}\"/>", 9525, 9525)));
        let back = read(&out.bytes).unwrap();
        let name = &back.images[0].0;
        let want = format!(
            "<img src=\"bild:{name}\" alt=\"Sol &amp; måne\" width=\"240\">\n\nText med <img src=\"bild:{name}\" alt=\"\" width=\"100\"> inne.\n\n<img src=\"bild:{name}\" alt=\"Ren\" width=\"1\">\n"
        );
        assert_eq!(back.markdown, want);
        // Och tillbaka igen, nu med Source, med bilderna från dokumentet.
        let imgs = back.images.clone();
        let again = write(&back.markdown, "x", &move |s| imgs.iter().find(|(n, _)| Some(n.as_str()) == s.strip_prefix(IMAGE_SCHEME)).map(|(_, i)| i.clone()), back.source.as_ref()).unwrap();
        assert!(again.missing_images.is_empty());
        let doc2 = part_text(&again.bytes, "word/document.xml");
        assert_eq!(doc2.matches("<wp:extent ").count(), 3);
        assert_eq!(read(&again.bytes).unwrap().markdown, back.markdown);
    }

    const TEST_PNG: [u8; 67] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1,
        8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0, 1, 0, 0,
        5, 0, 1, 0x0D, 0x0A, 0x2D, 0xB4, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn floating_image_roundtrip() {
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None });
        let md = "<img src=\"bild:a.png\" alt=\"Sol\" width=\"200\" align=\"left\">Text bredvid.\n\n<img src=\"bild:a.png\" alt=\"\" width=\"100\" align=\"right\">\n\nSlut\n";
        let out = write(md, "x", &resolve, None).unwrap();
        assert!(out.missing_images.is_empty());
        let doc = part_text(&out.bytes, "word/document.xml");
        assert_eq!(doc.matches("<wp:anchor ").count(), 2, "{doc}");
        assert_eq!(doc.matches("<wp:inline ").count(), 0);
        assert!(doc.contains(r#"distT="0" distB="0" distL="0" distR="114300" simplePos="0""#), "{doc}");
        assert!(doc.contains(r#"distL="114300" distR="0" simplePos="0""#));
        assert!(doc.contains(r#"<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:align>left</wp:align></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="1905000" cy="1905000"/>"#));
        assert!(doc.contains("<wp:align>right</wp:align>"));
        assert_eq!(doc.matches(r#"<wp:wrapSquare wrapText="bothSides"/>"#).count(), 2);
        // Schemaordning: wrap före docPr.
        assert!(doc.find("<wp:wrapSquare").unwrap() < doc.find("<wp:docPr").unwrap());
        // relativeHeight är unika.
        let rh: Vec<&str> = doc.split("relativeHeight=\"").skip(1).map(|x| x.split('"').next().unwrap()).collect();
        assert_eq!(rh.len(), 2);
        assert_ne!(rh[0], rh[1]);

        let back = read(&out.bytes).unwrap();
        let name = &back.images[0].0;
        let want = format!(
            "<img src=\"bild:{name}\" alt=\"Sol\" width=\"200\" align=\"left\">Text bredvid.\n\n<img src=\"bild:{name}\" alt=\"\" width=\"100\" align=\"right\">\n\nSlut\n"
        );
        assert_eq!(back.markdown, want);
        // Och tillbaka igen med Source: oförändrat.
        let imgs = back.images.clone();
        let again = write(&back.markdown, "x", &move |s| imgs.iter().find(|(n, _)| Some(n.as_str()) == s.strip_prefix(IMAGE_SCHEME)).map(|(_, i)| i.clone()), back.source.as_ref()).unwrap();
        assert_eq!(read(&again.bytes).unwrap().markdown, back.markdown);
    }

    #[test]
    fn anchor_variants_are_read() {
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None });
        let md = "<img src=\"bild:a.png\" alt=\"\" width=\"200\" align=\"left\">\n";
        let w = write(md, "x", &resolve, None).unwrap().bytes;
        // Byter `from` mot `to` i document.xml och ger sidan bilden får vid inläsning.
        let read_with = |from: &str, to: &str| {
            let b = patch_document(&w, |x| x.replace(from, to));
            let m = read(&b).unwrap().markdown;
            if m.contains("align=\"left\"") {
                "left"
            } else if m.contains("align=\"right\"") {
                "right"
            } else {
                "none"
            }
        };
        let al = |a: &str| read_with("<wp:align>left</wp:align>", &format!("<wp:align>{a}</wp:align>"));
        assert_eq!(al("left"), "left");
        assert_eq!(al("right"), "right");
        assert_eq!(al("center"), "none");
        // wrapTight och wrapThrough flyter också; wrapTopAndBottom och wrapNone gör det inte.
        let sq = r#"<wp:wrapSquare wrapText="bothSides"/>"#;
        let poly = r#"<wp:wrapPolygon edited="0"><wp:start x="0" y="0"/></wp:wrapPolygon>"#;
        assert_eq!(read_with(sq, &format!(r#"<wp:wrapTight wrapText="bothSides">{poly}</wp:wrapTight>"#)), "left");
        assert_eq!(read_with(sq, &format!(r#"<wp:wrapThrough wrapText="bothSides">{poly}</wp:wrapThrough>"#)), "left");
        assert_eq!(read_with(sq, "<wp:wrapTopAndBottom/>"), "none");
        assert_eq!(read_with(sq, "<wp:wrapNone/>"), "none");
        // Förskjutning nära vänster/höger marginal i stället för align (A4: textbredd 5731510 EMU).
        let h = r#"<wp:positionH relativeFrom="column"><wp:align>left</wp:align></wp:positionH>"#;
        let off = |rel: &str, o: i64| read_with(h, &format!(r#"<wp:positionH relativeFrom="{rel}"><wp:posOffset>{o}</wp:posOffset></wp:positionH>"#));
        assert_eq!(off("column", 0), "left");
        assert_eq!(off("column", 3_800_000), "right");
        assert_eq!(off("column", 2_000_000), "none");
        assert_eq!(off("page", 720_000), "left");
        // Inline-bilder blir aldrig flytande.
        let inl = w_inline();
        assert!(!read(&inl).unwrap().markdown.contains("align="));
    }

    fn w_inline() -> Vec<u8> {
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None });
        write("<img src=\"bild:a.png\" width=\"50\">\n", "x", &resolve, None).unwrap().bytes
    }

    /// Skriver ett dokument med en flytande bild och låter LibreOffice göra PDF: texten bredvid
    /// bilden ska börja till höger om den. Kräver `soffice` och `pdftotext`.
    #[test]
    #[ignore]
    fn floating_image_libreoffice() {
        let dir = std::env::temp_dir().join("kladd-float-lo");
        std::fs::create_dir_all(&dir).unwrap();
        // Rutig bild så att den syns i PDF:en.
        let mut img = image::RgbImage::new(240, 160);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb(if (x / 20 + y / 20) % 2 == 0 { [60, 120, 200] } else { [255, 200, 40] });
        }
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris.";
        let md = format!("<img src=\"b.png\" alt=\"\" width=\"240\" align=\"left\">{lorem}\n\n{lorem}\n\n{lorem}\n\n{lorem}\n\n{lorem}\n\n{lorem}\n");
        let w = write(&md, "x", &|_| Some(Image { bytes: png.clone(), extent: None }), None).unwrap();
        let docx = dir.join("f.docx");
        std::fs::write(&docx, w.bytes).unwrap();
        let st = std::process::Command::new("soffice")
            .arg(format!("-env:UserInstallation=file://{}/profile", dir.display()))
            .args(["--headless", "--convert-to", "pdf", "--outdir"]).arg(&dir).arg(&docx).output().unwrap();
        assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
        let out = std::process::Command::new("pdftotext").args(["-bbox"]).arg(dir.join("f.pdf")).arg("-").output().unwrap();
        let html = String::from_utf8_lossy(&out.stdout).into_owned();
        // x för varje stycke-startord "Lorem".
        let xs: Vec<f32> = html
            .lines()
            .filter(|l| l.contains(">Lorem<"))
            .filter_map(|l| l.split("xMin=\"").nth(1)?.split('"').next()?.parse().ok())
            .collect();
        assert!(xs.len() >= 6, "{xs:?}");
        // Första stycket (bredvid bilden) börjar efter bildens 180 pt + marginalen 72 pt.
        assert!(xs[0] > 240.0, "första stycket vid x = {}", xs[0]);
        assert!(xs.iter().any(|&x| x < 100.0), "inget stycke under bilden: {xs:?}");
    }

    #[test]
    fn img_tag_parsing() {
        let t = parse_img("<img src='bild:x%20y.png' alt=\"a &quot;b&quot;\" width=\"12.5px\" />").unwrap();
        assert_eq!((t.src.as_str(), t.alt.as_str(), t.width, t.height), ("bild:x%20y.png", "a \"b\"", Some(12.5), None));
        assert!(parse_img("<img alt=\"x\">").is_none());
        assert!(parse_img("<imgx src=\"a\">").is_none());
        assert_eq!(parse_tokens("<!--k:3--><!--k:12-->"), Some(vec![3, 12]));
        assert_eq!(parse_tokens("<!--k:3--> text"), None);
    }

    #[test]
    fn paragraph_starting_with_token_stays_a_paragraph() {
        let doc = format!(
            r#"<w:document {RICH_NS}><w:body><w:p><w:bookmarkStart w:id="1" w:name="a"/><w:r><w:t>Rad med text</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>{SECT}</w:body></w:document>"#
        );
        let docx = patch_part(&rich_docx(), "word/document.xml", move |_| doc.clone());
        let l = read(&docx).unwrap();
        assert!(l.markdown.contains("&#8203;<!--k:0-->Rad med text"), "{}", l.markdown);
        let out = write(&l.markdown.replace("Rad", "Ny rad"), "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains("Ny rad med text") && doc.contains("<w:bookmarkStart w:id=\"1\" w:name=\"a\"/>") && !doc.contains('\u{200B}'), "{doc}");
    }

    /// Manuell kontroll av en riktig fil: läs, skriv (oförändrat och med ett ändrat ord) och jämför
    /// delarna. `KLADD_ROUNDTRIP=/tmp/a.docx:/tmp/b.docx cargo test roundtrip_files -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn roundtrip_files() {
        let Ok(list) = std::env::var("KLADD_ROUNDTRIP") else { return };
        for f in list.split(':') {
            let bytes = std::fs::read(f).unwrap();
            let l = read(&bytes).unwrap();
            let out = write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
            let mut a = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
            let mut b = zip::ZipArchive::new(Cursor::new(&out)).unwrap();
            let names: Vec<String> = a.file_names().map(str::to_owned).collect();
            let mut same = 0;
            for n in &names {
                let (mut x, mut y) = (Vec::new(), Vec::new());
                std::io::Read::read_to_end(&mut a.by_name(n).unwrap(), &mut x).unwrap();
                match b.by_name(n) {
                    Ok(mut e) => std::io::Read::read_to_end(&mut e, &mut y).unwrap(),
                    Err(_) => {
                        println!("{f}: SAKNAS {n}");
                        continue;
                    }
                };
                if x == y {
                    same += 1;
                } else {
                    println!("{f}: ändrad {n} ({} -> {} byte)", x.len(), y.len());
                }
            }
            println!("{f}: {same}/{} delar identiska, tappat: {:?}, markörer: {}", names.len(), l.dropped, l.markdown.matches("<!--k:").count());
            std::fs::write(format!("{f}.kladd-ut.docx"), out).unwrap();
        }
    }

    #[test]
    fn table_merge_widths_and_breaks_roundtrip() {
        let md = "| Rubrik över två || x |\n| ------ | -- | ---- |\n| a | b<br>rad | c |\n| d |||\n";
        let w = write(md, "T", &|_| None, None).unwrap();
        let xml = part_text(&w.bytes, "word/document.xml");
        // Sammanfogad cell, kolumnbredder i förhållande 6:2:4 och radbrytning i cellen.
        assert!(xml.contains("<w:gridSpan w:val=\"2\"/>"), "{xml}");
        assert!(xml.contains("<w:gridSpan w:val=\"3\"/>"));
        assert!(xml.contains("<w:gridCol w:w=\"4513\"/><w:gridCol w:w=\"1504\"/><w:gridCol w:w=\"3008\"/>"), "{xml}");
        assert!(xml.contains("<w:tcW w:w=\"6017\" w:type=\"dxa\"/><w:gridSpan w:val=\"2\"/>"));
        assert!(xml.contains("<w:r><w:br/></w:r>"));
        // Täckta celler skrivs inte: 2 + 3 + 1 celler.
        assert_eq!(xml.matches("<w:tc>").count(), 6, "{xml}");
        let back = read(&w.bytes).unwrap();
        assert!(back.dropped.is_empty(), "{:?}", back.dropped);
        let ts = markup::table_src(&back.markdown);
        assert_eq!(ts.cols, 3);
        assert_eq!(ts.spans, vec![vec![2, 0, 1], vec![1, 1, 1], vec![3, 0, 0]], "{}", back.markdown);
        assert_eq!(ts.weights, Some(vec![20, 7, 13]), "{}", back.markdown);
        assert!(back.markdown.contains("| a | b<br>rad | c |"), "{}", back.markdown);
        // Och en gång till ger samma text.
        assert_eq!(rt(&back.markdown), back.markdown);
    }

    #[test]
    fn table_equal_widths_stay_plain() {
        let md = "| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        assert_eq!(rt(md), md);
        let xml = part_text(&write(md, "T", &|_| None, None).unwrap().bytes, "word/document.xml");
        assert!(xml.contains("<w:gridCol w:w=\"4512\"/><w:gridCol w:w=\"4512\"/>"), "{xml}");
        assert!(!xml.contains("gridSpan"));
    }

    #[test]
    fn reads_grid_span_widths_and_vertical_merge_from_word() {
        let w = write("x\n", "x", &|_| None, None).unwrap();
        let patched = patch_document(&w.bytes, |x| {
            let cell = |pr: &str, t: &str| {
                format!("<w:tc><w:tcPr>{pr}</w:tcPr><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc>")
            };
            let tbl = format!(
                "<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"6000\"/><w:gridCol w:w=\"2000\"/></w:tblGrid>\
                 <w:tr>{}{}</w:tr>\
                 <w:tr>{}{}{}</w:tr>\
                 <w:tr>{}{}{}</w:tr></w:tbl>",
                cell("<w:tcW w:w=\"8000\" w:type=\"dxa\"/><w:gridSpan w:val=\"2\"/>", "Bred"),
                cell("<w:tcW w:w=\"2000\" w:type=\"dxa\"/>", "Id"),
                cell("<w:vMerge w:val=\"restart\"/>", "Lodrät"),
                cell("", "b"),
                cell("", "c"),
                cell("<w:vMerge/>", ""),
                cell("", "e"),
                cell("", "f"),
            );
            let start = x.find("<w:p>").unwrap();
            let end = x.rfind("</w:p>").unwrap() + "</w:p>".len();
            format!("{}{}{}", &x[..start], tbl, &x[end..])
        });
        let back = read(&patched).unwrap();
        let md = back.markdown.trim_end();
        let ts = markup::table_src(md);
        assert_eq!(ts.spans, vec![vec![2, 0, 1], vec![1, 1, 1], vec![1, 1, 1]], "{md}");
        // 2000:6000:2000 av 40 streck = 8:24:8.
        assert_eq!(ts.weights, Some(vec![8, 24, 8]), "{md}");
        // Lodrätt sammanfogad fortsättningscell blir en tom cell.
        assert!(md.contains("| Lodrät | b | c |"), "{md}");
        assert!(md.contains("|   | e | f |"), "{md}");
    }

    #[test]
    fn narrow_word_tables_keep_auto_width() {
        // En smal tabell (som LibreOffice skriver) ska inte bli proportionell över hela bredden.
        let w = write("x\n", "x", &|_| None, None).unwrap();
        let patched = patch_document(&w.bytes, |x| {
            let tbl = "<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"1201\"/><w:gridCol w:w=\"991\"/></w:tblGrid>\
                       <w:tr><w:tc><w:p><w:r><w:t>Område</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Resultat</w:t></w:r></w:p></w:tc></w:tr>\
                       <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";
            let start = x.find("<w:p>").unwrap();
            let end = x.rfind("</w:p>").unwrap() + "</w:p>".len();
            format!("{}{tbl}<w:p/>{}", &x[..start], &x[end..])
        });
        let md = read(&patched).unwrap().markdown;
        assert!(md.contains("| --- | --- |"), "{md}");
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
            }, None)
            .unwrap();
            println!("saknade bilder: {:?}", w.missing_images);
            std::fs::write(o, w.bytes).unwrap();
        }
    }
    #[test]
    fn touch_core_updates_modified() {
        let xml = r#"<cp:coreProperties><dc:title>T</dc:title><cp:lastModifiedBy>Anna</cp:lastModifiedBy><dcterms:modified xsi:type="dcterms:W3CDTF">2020-01-01T00:00:00Z</dcterms:modified></cp:coreProperties>"#;
        let out = crate::docx::touch_core(xml, "2026-11-12T10:00:00Z").unwrap();
        assert!(out.contains(r#"<dcterms:modified xsi:type="dcterms:W3CDTF">2026-11-12T10:00:00Z</dcterms:modified>"#));
        assert!(out.contains("<cp:lastModifiedBy>Kladd</cp:lastModifiedBy>"));
        assert!(out.contains("<dc:title>T</dc:title>"));
        assert_eq!(crate::docx::touch_core("<cp:coreProperties/>", "x"), None);
    }

}

#[cfg(test)]
#[path = "docx_fn_tests.rs"]
mod fn_tests;

#[path = "docx_review.rs"]
mod review_xml;

#[cfg(test)]
#[path = "docx_section_tests.rs"]
mod section_tests;

#[cfg(test)]
#[path = "docx_track2_tests.rs"]
mod track2_tests;
