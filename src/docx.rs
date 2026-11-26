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

use crate::docx_tbox;
use crate::review::{self, CommentRec, RevKind, RevRec};
use crate::word::sections;
use crate::style::titles::{self, TitleKind};
use crate::style::{self, Align, DocDefaults, SpanStyle};
use crate::word::anchor::{Anchor, HRel, Pos, VRel, Wrap};
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
    /// Rastret: PNG/JPEG/GIF/BMP, eller för en SVG dess PNG-reserv (visning, utskrift, HTML-reserv).
    pub bytes: Vec<u8>,
    /// Storleken i originalet (EMU), om den är känd.
    pub extent: Option<(i64, i64)>,
    /// SVG-källan när bilden är en vektorbild; skrivs som vektor i .docx, .odt, PDF och HTML.
    pub svg: Option<Svg>,
}

/// En SVG-fil och dess naturliga storlek (CSS-pixlar).
#[derive(Clone, Debug, PartialEq)]
pub struct Svg {
    pub data: Vec<u8>,
    pub size: (f32, f32),
}

impl Image {
    /// Raster utan vektorkälla.
    pub fn raster(bytes: Vec<u8>, extent: Option<(i64, i64)>) -> Self {
        Image { bytes, extent, svg: None }
    }

    fn bytes_and_svg(self) -> (Vec<u8>, Option<Svg>) {
        (self.bytes, self.svg)
    }

    /// Den naturliga storleken i bildpunkter (96 dpi). För en SVG är det SVG:ns egen storlek, inte
    /// reservrastrets (som ritas i högre upplösning).
    pub fn natural_px(&self) -> Option<(f32, f32)> {
        match &self.svg {
            Some(s) => Some(s.size),
            None => imagesize::blob_size(&self.bytes).ok().map(|s| (s.width as f32, s.height as f32)),
        }
    }
}

/// Namnrymden för `asvg:svgBlip` (Word 2016+: SVG-bild med PNG-reserv).
const SVG_NS: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";

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
    /// Sant för en slutnot (`w:endnoteReference`), som blir `[^ei]` i texten.
    endnote: bool,
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
    /// Originalets slutnoter (endnotes.xml), om dokumentet har några.
    endnotes: Option<FnPart>,
    /// Sida, sidhuvud och sidfot så som de lästes (övriga fält i DocDefaults är tomma).
    page: DocDefaults,
    /// Formatmallarna så som de lästes ur `styles.xml` (se `docx_styles`).
    styles: style::StyleSet,
    /// Originalets kommentarer (comments.xml), om dokumentet har några.
    comments: Option<review_xml::CmPart>,
    /// Textrutornas ram (markören som tas bort igen) och text så som den lästes, per markörnummer.
    tboxes: HashMap<usize, (String, String, docx_tbox::Head)>,
    /// Sidhuvud och sidfot per avsnitt så som de lästes (för att se vad som ändrats).
    hf_secs: Vec<crate::hf::SecHf>,
    /// Alla avsnittens `w:sectPr` i ordning (för avsnitt som skrivs om från grunden).
    sect_prs: Vec<String>,
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

/// Som `decode_xml`, men giltig UTF-8 (det vanliga) blir en `String` utan kopia.
fn decode_xml_owned(mut buf: Vec<u8>) -> String {
    if buf.starts_with(&[0xFF, 0xFE]) || buf.starts_with(&[0xFE, 0xFF]) {
        return decode_xml(&buf);
    }
    if buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
        buf.drain(..3);
    }
    String::from_utf8(buf).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

struct Package<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    total: u64,
}

impl Package<'_> {
    fn part(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(buf) = self.part_bytes(name)? else { return Ok(None) };
        Ok(Some(decode_xml_owned(buf)))
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
/// Ett på/av-element (`<w:pageBreakBefore/>`, `w:val="0"` är av).
fn on_val(e: &BytesStart<'_>) -> bool {
    !matches!(attr(e, b"val").as_deref(), Some("0" | "false" | "off"))
}

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
    /// Fet och kursiv (`w:b`, `w:i`): används för teckenmallar som Betoning och Stark.
    bold: Option<bool>,
    italic: Option<bool>,
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
            bold: inner.bold.or(self.bold),
            italic: inner.italic.or(self.italic),
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
    /// `a:clrScheme`: dk1, lt1, dk2, lt2, accent1-6, hlink, folHlink (gemener).
    colors: HashMap<String, [u8; 3]>,
}

impl Theme {
    /// `w:themeColor` (`accent1`, `text2`, `background1` …) → temats färg, utan tint/shade.
    fn color(&self, name: &str) -> Option<[u8; 3]> {
        let key = match name {
            "text1" | "dark1" => "dk1",
            "background1" | "light1" => "lt1",
            "text2" | "dark2" => "dk2",
            "background2" | "light2" => "lt2",
            "hyperlink" => "hlink",
            "followedHyperlink" => "folhlink",
            n => return self.colors.get(&n.to_ascii_lowercase()).copied(),
        };
        self.colors.get(key).copied()
    }

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
    // Färgen i `a:clrScheme` som just läses (dk1, accent1 …).
    let mut slot: Option<String> = None;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if matches!(local(e.name().as_ref()), b"srgbClr" | b"sysClr") => {
                let c = attr(&e, b"val").filter(|_| local(e.name().as_ref()) == b"srgbClr").or_else(|| attr(&e, b"lastClr"));
                if let (Some(s), Some(c)) = (&slot, c.and_then(|c| style::parse_color(&c))) {
                    t.colors.entry(s.clone()).or_insert(c);
                }
            }
            Ok(Event::Start(e)) => match local(e.name().as_ref()) {
                b"majorFont" => cur = 1,
                b"minorFont" => cur = 2,
                b"latin" => theme_latin(&mut t, cur, &e),
                n @ (b"dk1" | b"lt1" | b"dk2" | b"lt2" | b"accent1" | b"accent2" | b"accent3" | b"accent4" | b"accent5"
                | b"accent6" | b"hlink" | b"folHlink") => slot = Some(String::from_utf8_lossy(n).to_ascii_lowercase()),
                _ => {}
            },
            Ok(Event::Empty(e)) if local(e.name().as_ref()) == b"latin" => theme_latin(&mut t, cur, &e),
            Ok(Event::End(e))
                if matches!(
                    local(e.name().as_ref()),
                    b"dk1" | b"lt1" | b"dk2" | b"lt2" | b"accent1" | b"accent2" | b"accent3" | b"accent4" | b"accent5" | b"accent6" | b"hlink" | b"folHlink"
                ) =>
            {
                slot = None
            }
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
            let tc = crate::docx_styles::Theme { font: &|_| None, color: &|n| theme.color(n) };
            if let Some(c) = crate::docx_styles::color_of(e, &tc) {
                rp.color = Some(c);
            }
        }
        b"sz" => {
            if let Some(half) = attr(e, b"val").and_then(|v| v.parse::<f32>().ok()).filter(|h| *h > 0.0) {
                rp.size = Some(half / 2.0);
            }
        }
        b"b" => rp.bold = Some(!matches!(attr(e, b"val").as_deref(), Some("0" | "false" | "off" | "none"))),
        b"i" => rp.italic = Some(!matches!(attr(e, b"val").as_deref(), Some("0" | "false" | "off" | "none"))),
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
    /// `w:pageBreakBefore` i formatmallen.
    page_before: Option<bool>,
}

/// Formatmallarna i `styles.xml`: stycke- och teckenmallar plus dokumentets standardvärden.
#[derive(Default, Clone)]
struct Styles {
    map: HashMap<String, StyleInfo>,
    /// `w:docDefaults/w:rPrDefault`.
    defaults: RProps,
    /// Standardstyckemallen (`w:default="1"`, oftast `Normal`).
    default_para: String,
    /// Tabellmallarna (ramar, skuggning och villkorsformatering).
    tables: crate::docx_tables::TableStyles,
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

    /// Börjar stycken med formatmallen alltid på en ny sida (`w:pageBreakBefore`, även ärvt)?
    fn page_before(&self, style: Option<&str>) -> bool {
        let mut cur = Some(self.para_id(style));
        for _ in 0..10 {
            let Some(info) = cur.and_then(|c| self.map.get(c)) else { return false };
            if let Some(v) = info.page_before {
                return v;
            }
            cur = info.based_on.as_deref();
        }
        false
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
                        b"pageBreakBefore" if in_ppr => info.page_before = Some(on_val(e)),
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
    /// Wordstilen Title / Subtitle (Kladds `Block::Title` / `Block::Subtitle`).
    Title,
    Subtitle,
    Quote,
    Code,
}

/// Wordstilarna Title och Subtitle (id eller namn; "Rubrik" är svenska Words Title).
fn title_from_name(s: &str) -> Option<ParaKind> {
    match s.to_lowercase().replace([' ', '_', '-'], "").as_str() {
        "title" | "rubrik" | "titel" => Some(ParaKind::Title),
        "subtitle" | "underrubrik" | "undertitel" => Some(ParaKind::Subtitle),
        _ => None,
    }
}

fn heading_from_name(s: &str) -> Option<u8> {
    let s = s.to_lowercase().replace([' ', '_', '-'], "");
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
        if let Some(k) = title_from_name(&sid) {
            return k;
        }
        if let Some(h) = heading_from_name(&sid) {
            return ParaKind::Heading(h);
        }
        let Some(info) = styles.get(&sid) else { break };
        if let Some(k) = title_from_name(&info.name) {
            return k;
        }
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
    /// Ram, skuggning, avstånd och indrag som hamnar i dialektens markör (`word::pbox`).
    ppr: crate::word::pbox::PPr,
    /// Teckenstorlek (pt) på stycketecknet i ett tomt stycke, om stycket anger den.
    mark_sz: Option<f32>,
    /// Textrutor förankrade i stycket (`docx_tbox`).
    boxes: Vec<BoxRead>,
}

/// En textruta som lästs: innehållet som block, formen och läget.
#[derive(Clone)]
struct BoxRead {
    /// Markören (`<!--k:N-->`) som bär originalkörningen.
    token: usize,
    shape: docx_tbox::Shape,
    blocks: Vec<Block>,
    /// Bredd (pt) och vågrätt läge ur ankaret (`wp:anchor`), annars rutan i flödet.
    width: f32,
    x: crate::word::pbox::FrameX,
    /// `x` är ett avstånd från sidans vänsterkant (räknas om till spalten när sidan är känd).
    page_rel: bool,
    /// Ankaret ur `wp:anchor`/VML och rutans fasta höjd (px); `head` räknas ut när sidan är känd (`place_box`).
    pos: Option<AnchorPos>,
    height: f32,
    head: docx_tbox::Head,
}

impl Para {
    fn new() -> Para {
        Para { style: None, kind: ParaKind::Normal, list: None, runs: Vec::new(), align: Align::Left, direct_align: None, origin: usize::MAX, ppr: Default::default(), mark_sz: None, boxes: Vec::new() }
    }
}

#[derive(Clone)]
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
    /// Var `<w:tbl>` börjar i dokumentet och tabellens format (skuggning, ramar …).
    start: usize,
    read: Option<crate::docx_tables::TableRead>,
    /// Textrutor förankrade i stycken i cellerna: visas före tabellen (`docx_tbox`).
    boxes: Vec<BoxRead>,
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
    /// Läser en textruta: den får inte innehålla fler rutor.
    in_box: bool,
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
    crate::timing::lap_start();
    check_entry_count(bytes)?;
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| crate::tf!("Word-dokumentet är skadat: {e}", e = e))?;
    let pkg = Package { zip, total: 0 };
    crate::timing::lap("docx: zip-katalog");
    // Källans delar (alla filer i paketet, för den källbevarande sparningen) packas upp i en egen tråd medan
    // dokumentet tolkas: de behövs först allra sist och beror inte på tolkningen.
    std::thread::scope(|sc| {
        let parts = sc.spawn(|| collect_parts(bytes));
        read_package(pkg, || parts.join().ok().flatten())
    })
}

fn read_package(mut pkg: Package<'_>, take_parts: impl FnOnce() -> Option<Vec<(String, Vec<u8>)>>) -> Result<Loaded, String> {
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

    crate::timing::lap("docx: document.xml uppackad");
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
    let mut para_styles = crate::docx_para::ParaStyles::default();
    let styles = match find_part("/styles") {
        Some(p) => pkg
            .part(&p)?
            .map(|x| {
                para_styles = crate::docx_para::ParaStyles::parse(&x);
                let kladd = format!("<w:styles>{}</w:styles>", kladd_styles());
                let th = crate::docx_styles::Theme { font: &|t| theme.font(t), color: &|n| theme.color(n) };
                style_set = crate::docx_styles::read_relative(&x, &th, &kladd);
                let mut st = parse_styles(&x, &theme);
                st.tables = crate::docx_tables::TableStyles::parse(&x);
                st
            })
            .unwrap_or_default(),
        None => Styles::default(),
    };
    let numbering = match find_part("/numbering") {
        Some(p) => pkg.part(&p)?.map(|x| parse_numbering(&x)).unwrap_or_default(),
        None => Numbering::default(),
    };
    crate::timing::lap("docx: tema, formatmallar, numrering");
    let mut dropped = Dropped::default();

    let mut found: Vec<FoundImage> = Vec::new();
    let mut info = BodyInfo { track: true, ..Default::default() };
    // Fot- och slutnoternas delar packas upp här och tolkas i egna trådar medan brödtexten tolkas (de beror bara
    // på formatmallar, tema och numrering). Fel från uppackningen visas efter brödtextens, som förut.
    let fn_raw = find_part("/footnotes").map(|name| raw_footnotes(&mut pkg, &name));
    let en_raw = find_part("/endnotes").map(|name| raw_footnotes(&mut pkg, &name));
    let (body, footnotes, endnotes) = std::thread::scope(|sc| {
        let fnh = fn_raw.as_ref().and_then(|r| r.as_ref().ok()).map(|raw| sc.spawn(|| read_footnotes(raw, &styles, &theme, &numbering, "footnote")));
        let enh = en_raw.as_ref().and_then(|r| r.as_ref().ok()).map(|raw| sc.spawn(|| read_footnotes(raw, &styles, &theme, &numbering, "endnote")));
        let body = parse_body(&doc_xml, &styles, &theme, &numbering, &rels, &mut dropped, &mut found, dir, &mut info);
        let join = |h: Option<std::thread::ScopedJoinHandle<'_, Option<FnPart>>>| h.and_then(|h| h.join().ok()).flatten();
        (body, join(fnh), join(enh))
    });
    let mut blocks = body?;
    for r in [fn_raw, en_raw].into_iter().flatten() {
        r?;
    }
    crate::timing::lap("docx: parse_body");
    // Kommentarerna: posterna läggs sist i texten, och bara de som har ett område.
    let comments = match find_part("/comments") {
        Some(name) => {
            let ext = review_xml::ext_rel_target(&rels, dir);
            review_xml::read_comments(&mut pkg, &name, ext.as_deref())?
        }
        None => None,
    };

    // Fotnoterna (texten ur footnotes.xml läggs som definitioner sist i dokumentet) och slutnoterna (endnotes.xml,
    // `ei`, `eii` …) är lästa ovan.

    crate::timing::lap("docx: kommentarer, fotnoter, slutnoter");
    // Sida, sidhuvud och sidfot ur första avsnittet (de övriga avsnitten blir avsnittsrader).
    let page = match info.sects.first().or(info.sect_pr.as_ref()) {
        Some(sect) => read_page(sect)?,
        None => DocDefaults::default(),
    };
    let mut all_sects: Vec<&str> = info.sects.iter().map(String::as_str).collect();
    all_sects.extend(info.sect_pr.as_deref());
    let sect_prs: Vec<String> = all_sects.iter().map(|x| (*x).to_owned()).collect();
    // Sidhuvud och sidfot per avsnitt (bilderna i dem blir dokumentets bilder).
    let even_odd = find_part("/settings").and_then(|p| pkg.part(&p).ok().flatten()).is_some_and(|x| hf::even_and_odd(&x));
    let mut hf_images = hf::HfImages::default();
    let mut sect_hf: Vec<hf::SectHfRead> = Vec::new();
    for (i, sx) in all_sects.iter().enumerate() {
        let raw = read_sect(sx);
        let geo = hf::PageGeo { size: raw.size.unwrap_or_else(|| page.page()), margin: raw.margin.unwrap_or_else(|| page.margins()) };
        sect_hf.push(hf::read_sect(sx, i == 0, even_odd, geo, &mut pkg, &rels, dir, &mut hf_images)?);
    }
    let mut page = page;
    // Fot- och slutnoternas talformat (settings.xml och avsnittens `w:sectPr`).
    page.notes = {
        let settings = find_part("/settings").and_then(|p| pkg.part(&p).ok().flatten());
        crate::docx_notes::read(settings.as_deref(), &all_sects)
    };
    if let Some(h) = sect_hf.first() {
        page.header = h.own.header.clone();
        page.footer = h.own.footer.clone();
        page.header_first = h.own.header_first.clone();
        page.footer_first = h.own.footer_first.clone();
        page.header_even = h.own.header_even.clone();
        page.footer_even = h.own.footer_even.clone();
        page.header_align = h.header_align;
        page.footer_align = h.footer_align;
        page.header_style = h.header_style.clone();
        page.footer_style = h.footer_style.clone();
        page.header_dist = h.header_dist;
        page.footer_dist = h.footer_dist;
        page.page_start = h.own.page_start;
        page.page_format = h.own.page_fmt;
    }
    crate::timing::lap("docx: sidhuvud/sidfot");
    fill_section_rows(&mut blocks, &all_sects, &page, &sect_hf);

    // Packa upp bilderna som dokumentet faktiskt använder.
    let mut images: Vec<(String, Image)> = std::mem::take(&mut hf_images.list);
    let mut by_target: HashMap<String, String> = std::mem::take(&mut hf_images.by_target);
    let text_w = {
        let (w, _) = page.page();
        let m = page.margins();
        Some(((w - m[1] - m[3]) * 20.0) as f64)
    };
    fill_para_looks(&mut blocks, &info.elems, &para_styles);
    // Rutornas läge i spalten och ankaret i öppningsraden (sidan är känd först nu).
    for b in blocks.iter_mut() {
        match b {
            Block::Para(p) => p.boxes.iter_mut().for_each(|bx| place_box(bx, &page)),
            Block::Table(t, _) => t.boxes.iter_mut().for_each(|bx| {
                place_box(bx, &page);
                // Rutor i tabellceller ligger i flödet ovanför tabellen.
                bx.head.anchor = None;
            }),
        }
    }
    let mut tboxes: HashMap<usize, (String, String, docx_tbox::Head)> = HashMap::new();
    for b in &blocks {
        let (list, shown) = match b {
            Block::Para(p) => (&p.boxes, box_shown(p)),
            Block::Table(t, _) => (&t.boxes, true),
        };
        for bx in list.iter().filter(|_| shown) {
            let (marker, md) = box_md(bx, &numbering, text_w);
            if !md.is_empty() {
                tboxes.entry(bx.token).or_insert_with(|| (marker.clone(), strip_frame(&md, &marker), bx.head.rounded()));
            }
        }
    }
    crate::timing::lap("docx: fill_*, rutor");
    let (mut markdown, mut units) = to_markdown_units(&blocks, &numbering, text_w);
    crate::timing::lap("docx: to_markdown_units");
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
    base.header_even = page.header_even.clone();
    base.footer_even = page.footer_even.clone();
    base.header_style = page.header_style.clone();
    base.footer_style = page.footer_style.clone();
    base.header_dist = page.header_dist;
    base.footer_dist = page.footer_dist;
    base.page_start = page.page_start;
    base.page_format = page.page_format;
    base.notes = page.notes;
    base.styles = style_set;
    if !info.fn_refs.is_empty() {
        markdown.push('\n');
        // Fotnoterna först (numrerade 1..n), sedan slutnoterna (`e1`, `e2` …), var och en i textens ordning.
        for end in [false, true] {
            let part = if end { endnotes.as_ref() } else { footnotes.as_ref() };
            for (i, r) in info.fn_refs.iter().filter(|r| r.endnote == end).enumerate() {
                let md = part
                    .and_then(|f| f.items.iter().find(|x| x.id == r.id && x.ty.is_none()))
                    .map_or("", |x| x.md.as_str());
                let label = if end { crate::word::markup::endnote_label(i + 1) } else { (i + 1).to_string() };
                markdown.push_str(&format!("[^{label}]: {md}\n"));
            }
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
    crate::timing::lap("docx: noter, kommentarer, review");
    for f in found {
        // Ett diagram som Kladd skrivit: koden i alt-texten blir ett ```mermaid-block igen (inte en bild).
        if let Some(code) = mermaid_code(&f.alt) {
            let alone = markdown.find(&f.placeholder).is_some_and(|at| {
                let ls = markdown[..at].rfind('\n').map_or(0, |i| i + 1);
                let le = markdown[at..].find('\n').map_or(markdown.len(), |i| at + i);
                markdown[ls..le].trim() == f.placeholder
            });
            if alone {
                let fence = if code.contains("```") { "~~~~" } else { "```" };
                let repl = format!("{fence}mermaid\n{code}\n{fence}");
                markdown = markdown.replace(&f.placeholder, &repl);
                for (k, u) in keys.iter_mut().zip(units.iter_mut()).filter(|(k, _)| k.contains(&f.placeholder)) {
                    *k = norm(&repl);
                    u.desc = "c|left".to_owned();
                }
                continue;
            }
        }
        let name = if let Some(n) = by_target.get(&f.target) {
            Some(n.clone())
        } else {
            // Word 2016+: SVG-delen först (PNG-reserven blir rastret); annars bara bilden.
            let vector = match &f.svg {
                Some(t) => pkg.part_bytes(t)?.and_then(|svg| crate::imgfmt::from_package(svg, pkg.part_bytes(&f.target).ok().flatten())),
                None => None,
            };
            let plain = match vector {
                Some(_) => None,
                None => pkg.part_bytes(&f.target)?.filter(|b| is_supported_image(b)),
            };
            match vector.map(|v| v.bytes_and_svg()).or_else(|| plain.map(|b| (b, None))) {
                None => None,
                Some((bytes, svg)) => {
                    let base = f.target.rsplit('/').next().unwrap_or("bild").to_owned();
                    let mut name = base.clone();
                    let mut n = 2;
                    while images.iter().any(|(m, _)| *m == name) {
                        name = format!("{n}-{base}");
                        n += 1;
                    }
                    images.push((name.clone(), Image { bytes, extent: f.extent, svg }));
                    by_target.insert(f.target.clone(), name.clone());
                    Some(name)
                }
            }
        };
        let repl = match name {
            Some(name) => {
                let img = images.iter().find(|(n, _)| *n == name).map(|(_, i)| i);
                // Bilder i en tabellcell flyter inte (cellen har sin egen bredd): vanlig bild.
                let in_cell = markdown.find(&f.placeholder).is_some_and(|at| {
                    let ls = markdown[..at].rfind('\n').map_or(0, |i| i + 1);
                    markdown[ls..].trim_start().starts_with('|')
                });
                let anchor = f.anchor.as_ref().filter(|_| !in_cell).map(|a| anchor_of(a, &page));
                image_md(&name, &f.alt, f.extent, img, anchor, f.crop)
            }
            // Bilden går inte att visa: originalkörningen bevaras som markör.
            None => f.fallback.map(token_text).unwrap_or_default(),
        };
        markdown = markdown.replace(&f.placeholder, &repl);
        for k in keys.iter_mut().filter(|k| k.contains(&f.placeholder)) {
            *k = k.replace(&f.placeholder, &repl);
        }
    }

    crate::timing::lap("docx: bilder");
    let mut toc_heads = None;
    for k in keys.iter_mut().filter(|k| crate::word::nav::is_toc_line(k)) {
        *k = toc_unit_key(k, &markdown, &mut toc_heads);
    }
    drop(toc_heads);

    crate::timing::lap("docx: förteckningsnycklar");
    let source = take_parts().and_then(|parts| {
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
        Some(Source { parts, main: real, head: info.head, tail: info.tail, sect_pr: info.sect_pr, elems, tokens: info.tokens, footnotes, endnotes, page, styles: read_styles, comments, hf_secs: sect_hf.iter().map(|h| h.own.clone()).collect(), sect_prs, tboxes })
    });
    crate::timing::lap("docx: källa (collect_parts)");
    Ok(Loaded { source, markdown, images, dropped: dropped.list() })
}

/// Delar upp footnotes.xml: allt före och efter, och varje `<w:footnote>` med id, typ, hela
/// elementet och innehållets intervall (utan start- och sluttaggen).
fn split_footnotes(xml: &str, tag: &str) -> Option<(String, Vec<(String, Option<String>, String, std::ops::Range<usize>)>, String)> {
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
                if depth == 1 && local(e.name().as_ref()) == tag.as_bytes() {
                    first.get_or_insert(before);
                    cur = Some((before, after, attr(e, b"id").unwrap_or_default(), attr(e, b"type")));
                }
                depth += 1;
            }
            Event::Empty(e) => {
                if depth == 1 && local(e.name().as_ref()) == tag.as_bytes() {
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

/// Det som läses ur paketet för fot- eller slutnoter: delens namn, XML och relationer.
struct RawNotes {
    name: String,
    xml: String,
    /// Hyperlänkar i noterna pekar på delens egna relationer.
    rels: HashMap<String, (String, String, bool)>,
}

fn raw_footnotes(pkg: &mut Package<'_>, name: &str) -> Result<Option<RawNotes>, String> {
    let Some(xml) = pkg.part(name)? else { return Ok(None) };
    let (dir, file) = name.rsplit_once('/').unwrap_or(("", name));
    let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
    let rels = pkg.part(&rels_name)?.map(|x| parse_rels(&x)).unwrap_or_default();
    Ok(Some(RawNotes { name: name.to_owned(), xml, rels }))
}

fn read_footnotes(raw: &Option<RawNotes>, styles: &Styles, theme: &Theme, numbering: &Numbering, tag: &str) -> Option<FnPart> {
    let RawNotes { name, xml, rels } = raw.as_ref()?;
    let (head, raw, tail) = split_footnotes(xml, tag)?;
    let items = raw
        .into_iter()
        .map(|(id, ty, xml_el, inner)| {
            let md = if ty.is_none() { footnote_md(&xml[inner], styles, theme, numbering, rels) } else { String::new() };
            FnOrig { id, ty, md, xml: xml_el }
        })
        .collect();
    Some(FnPart { name: name.to_owned(), head, tail, items })
}

/// Hur en bild skrivs i texten: `![alt](bild:namn)`, eller `<img … width>` när storleken avviker
/// från bildens egen (96 dpi) eller från den som redan lagrats för bilden.
fn image_md(name: &str, alt: &str, extent: Option<(i64, i64)>, img: Option<&Image>, anchor: Option<Anchor>, crop: Option<[i64; 4]>) -> String {
    let float = anchor.as_ref().and_then(Anchor::legacy_side);
    let anchor = anchor.filter(|_| float.is_none());
    // Beskärning i procent (`a:srcRect` är i tusendels procent).
    let crop = crop
        .map(|c| c.map(|v| v as f32 / 1000.0))
        .filter(|c| c.iter().all(|v| (0.0..100.0).contains(v)) && c[0] + c[2] < 100.0 && c[1] + c[3] < 100.0);
    let floating = float.is_some() || anchor.is_some();
    let differs = |a: f64, b: f64| b > 0.0 && (a - b).abs() / b > 0.02;
    let want = extent.map(|(cx, _)| cx as f64 / EMU_PER_PIXEL as f64);
    let nat = img.and_then(Image::natural_px).map(|s| (s.0 as f64, s.1 as f64));
    let natural = nat.map(|n| n.0);
    let stored = img.and_then(|i| i.extent).map(|(cx, _)| cx as f64 / EMU_PER_PIXEL as f64);
    let src = format!("{IMAGE_SCHEME}{}", name.replace(' ', "%20"));
    // En flytande eller beskuren bild skrivs alltid som `<img … width …>`.
    let want = if floating || crop.is_some() { want.or(natural) } else { want };
    match want {
        Some(w) if floating || crop.is_some() || natural.is_some_and(|n| differs(w, n)) || stored.is_some_and(|st| differs(w, st)) => {
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
            // Höjden skrivs när den inte följer av bredden (beskuren eller sträckt bild).
            let h = extent.map(|(_, cy)| cy as f64 / EMU_PER_PIXEL as f64).filter(|h| {
                let ratio = match (nat, crop) {
                    (Some((nw, nh)), Some(c)) => Some(nh * (1.0 - (c[1] + c[3]) as f64 / 100.0) / (nw * (1.0 - (c[0] + c[2]) as f64 / 100.0))),
                    (Some((nw, nh)), None) => Some(nh / nw),
                    _ => None,
                };
                match ratio {
                    Some(r) => differs(*h, w * r),
                    None => crop.is_some(),
                }
            });
            let mut tag = format!("<img src=\"{src}\" alt=\"{a}\" width=\"{}\"", w.round().max(1.0) as i64);
            if let Some(h) = h {
                tag.push_str(&format!(" height=\"{}\"", h.round().max(1.0) as i64));
            }
            match (anchor, float) {
                (Some(an), _) => an.write_attrs(&mut tag),
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
        _ => format!("![{}]({src})", escape_alt(alt)),
    }
}

/// Alla poster i paketet i ordning, eller `None` om de blir för stora.
fn collect_parts(bytes: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut out = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).ok()?;
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
    /// Spaltbredder i pt när spalterna är olika breda (`w:equalWidth="0"`), annars tom.
    widths: Vec<f32>,
    /// Skiljelinje mellan spalterna (`w:sep`).
    rule: bool,
    continuous: bool,
}

fn read_sect(sect: &str) -> RawSect {
    let mut o = RawSect { size: None, margin: None, cols: 1, gap: None, widths: Vec::new(), rule: false, continuous: false };
    let mut unequal = false;
    let mut col_ws: Vec<f32> = Vec::new();
    let mut first_space: Option<f32> = None;
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
                    o.rule = matches!(attr(&e, b"sep").as_deref(), Some("1" | "true" | "on"));
                    unequal = matches!(attr(&e, b"equalWidth").as_deref(), Some("0" | "false" | "off"));
                }
                // Olika breda spalter: bredderna samlas, mellanrummet tas från första spalten.
                b"col" => {
                    if let Some(w) = tw(&e, b"w").filter(|w| *w > 0.0) {
                        col_ws.push(w / 20.0);
                    }
                    if col_ws.len() == 1 {
                        first_space = tw(&e, b"space").map(|v| v / 20.0);
                    }
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
    if unequal && col_ws.len() > 1 {
        o.cols = col_ws.len().min(sections::MAX_COLUMNS);
        col_ws.truncate(o.cols);
        if let Some(g) = first_space {
            o.gap = Some(g);
        }
        // Lika breda i praktiken: vanliga spalter.
        if col_ws.iter().any(|w| (w - col_ws[0]).abs() > 0.5) {
            o.widths = col_ws;
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
        p.column_widths = Some(raw.widths.clone()).filter(|w| w.len() > 1);
        p.column_rule = raw.rule;
    }
    p.continuous = raw.continuous;
    p
}

/// Byter platshållarna i avsnittsraderna mot raderna med respektive avsnitts egenskaper.
/// `sects[n]` är `w:sectPr` för avsnitt n (0 = första); rad n börjar avsnitt n + 1.
fn fill_section_rows(blocks: &mut [Block], sects: &[&str], d: &DocDefaults, hfs: &[hf::SectHfRead]) {
    for b in blocks {
        let Block::Para(p) = b else { continue };
        let [run] = p.runs.as_mut_slice() else { continue };
        let Some(n) = run.text.strip_prefix('\u{E010}').and_then(|r| r.strip_suffix('\u{E011}')).and_then(|r| r.parse::<usize>().ok())
        else {
            continue;
        };
        let mut props = section_props(sects.get(n + 1).copied(), d);
        if let Some(h) = hfs.get(n + 1) {
            props.hf = h.own.clone();
        }
        run.text = props.to_comment();
    }
}

/// Sidstorlek, marginaler, sidhuvud och sidfot ur en `w:sectPr`.
fn read_page(sect: &str) -> Result<DocDefaults, String> {
    let mut d = DocDefaults::default();
    let mut size: Option<(f32, f32)> = None;
    let mut mar = [style::DEFAULT_MARGIN; 4];
    let mut has_mar = false;
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
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    d.page_size = size;
    d.page_margin = has_mar.then_some(mar);
    Ok(d)
}

/// En bild som hittats i texten; platshållaren byts mot `![…](bild:…)` när bilden lästs.
struct FoundImage {
    placeholder: String,
    target: String,
    /// SVG-delen (`asvg:svgBlip`); `target` är då PNG-reserven.
    svg: Option<String>,
    alt: String,
    extent: Option<(i64, i64)>,
    /// Förankrad bild (`wp:anchor`): läge, omflöde och avstånd.
    anchor: Option<AnchorPos>,
    /// Beskärning (`a:srcRect`): vänster, ovan, höger, under i tusendels procent.
    crop: Option<[i64; 4]>,
    /// Markör för hela körningen, om bilden inte går att visa.
    fallback: Option<usize>,
}

/// Placeringen i en `wp:anchor`: `positionH`/`positionV`, omflöde och avstånd.
#[derive(Default, Clone)]
struct AnchorPos {
    /// `relativeFrom` ("column", "margin", "page" …) i sidled.
    rel: String,
    /// `wp:align`: left, right, center, inside, outside.
    align: Option<String>,
    /// `wp:posOffset` i EMU.
    off: Option<i64>,
    /// Samma i höjdled.
    rel_v: String,
    align_v: Option<String>,
    off_v: Option<i64>,
    /// Omflöde; `None` om ingen `wrap*`-tagg fanns (då ligger bilden framför texten).
    wrap: Option<Wrap>,
    /// `behindDoc="1"`.
    behind: bool,
    /// Avstånd till texten (ovan, under, vänster, höger) i EMU.
    dist: [i64; 4],
    /// `simplePos="1"`: läget i `wp:simplePos` (EMU från sidans hörn).
    simple: Option<(i64, i64)>,
}

/// Dialektens ankare för en förankrad bild. Lägen relativt kanter som Kladd inte har egna
/// (`leftMargin`, `bottomMargin` …) räknas om till sidan eller marginalen.
fn anchor_of(a: &AnchorPos, page: &DocDefaults) -> Anchor {
    let emu = |v: i64| (v as f64 / EMU_PER_PIXEL as f64) as f32;
    let (pw, ph) = page.page();
    let [_, mr, mb, _] = page.margins();
    let half = |v: Option<&str>| match v {
        Some("left" | "top" | "inside") => Some(Pos::Start),
        Some("center") => Some(Pos::Center),
        Some("right" | "bottom" | "outside") => Some(Pos::End),
        _ => None,
    };
    let (mut hrel, hbase) = match a.rel.as_str() {
        "page" => (HRel::Page, None),
        "margin" | "insideMargin" | "outsideMargin" => (HRel::Margin, None),
        "leftMargin" => (HRel::Page, None),
        "rightMargin" => (HRel::Page, Some(pw - mr)),
        _ => (HRel::Column, None),
    };
    let mut hpos = half(a.align.as_deref()).unwrap_or_else(|| Pos::Off(emu(a.off.unwrap_or(0)) + hbase.unwrap_or(0.0)));
    let (mut vrel, vbase) = match a.rel_v.as_str() {
        "page" | "topMargin" => (VRel::Page, None),
        "margin" | "insideMargin" | "outsideMargin" => (VRel::Margin, None),
        "bottomMargin" => (VRel::Page, Some(ph - mb)),
        _ => (VRel::Paragraph, None),
    };
    let mut vpos = half(a.align_v.as_deref()).unwrap_or_else(|| Pos::Off(emu(a.off_v.unwrap_or(0)) + vbase.unwrap_or(0.0)));
    if vrel == VRel::Paragraph && !matches!(vpos, Pos::Off(_)) {
        vpos = Pos::Off(0.0);
    }
    if let Some((x, y)) = a.simple {
        (hrel, hpos, vrel, vpos) = (HRel::Page, Pos::Off(emu(x)), VRel::Page, Pos::Off(emu(y)));
    }
    let wrap = match a.wrap {
        Some(w) => w,
        None if a.behind => Wrap::Behind,
        None => Wrap::Front,
    };
    Anchor { hrel, h: hpos, vrel, v: vpos, wrap, dist: [emu(a.dist[0]), emu(a.dist[3]), emu(a.dist[1]), emu(a.dist[2])] }
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
/// Första `w:id` för språkbokmärkena i kodstycken (se `codehl::lang_bookmark`), över Words egna nummer.
const LANG_BOOKMARK_ID: u32 = 880_000;

fn push_token(para: &mut Option<Para>, info: &mut BodyInfo, xml: &str) {
    let n = new_token(info, xml);
    let p = para.get_or_insert_with(Para::new);
    p.runs.push(Run { text: token_text(n), fmt: Fmt::default(), link: None, raw: true });
}

#[derive(Default)]
struct Drawing {
    depth: usize,
    blip: Option<String>,
    /// SVG-delen i `asvg:svgBlip` (Word 2016+), om bilden har en.
    svg: Option<String>,
    linked: bool,
    alt: String,
    title: String,
    extent: Option<(i64, i64)>,
    /// `wp:anchor` (inte `wp:inline`).
    anchored: bool,
    pos: AnchorPos,
    /// Inuti `positionH` (1) eller `positionV` (2).
    in_pos: u8,
    /// Vad nästa text är: 1 = `wp:align`, 2 = `wp:posOffset`.
    text_of: u8,
    /// `a:srcRect` (vänster, ovan, höger, under; tusendels procent).
    crop: Option<[i64; 4]>,
    /// Läget kommer från en VML-form (`v:shape`).
    vml: bool,
    /// Var elementet börjar i XML:en.
    start: usize,
    /// Intervallet för innehållet i `w:txbxContent`.
    tx: Option<(usize, usize)>,
}

/// Läser storlek och läge ur `style` på en VML-form (`width:72pt;margin-left:10pt;position:absolute …`).
fn vml_style(style: &str, d: &mut Drawing) {
    let get = |k: &str| {
        style.split(';').find_map(|kv| {
            let (a, b) = kv.split_once(':')?;
            (a.trim().eq_ignore_ascii_case(k)).then(|| b.trim().to_owned())
        })
    };
    // Längd i EMU.
    let emu = |v: &str| -> Option<i64> {
        let v = v.trim().to_ascii_lowercase();
        let (n, f) = if let Some(n) = v.strip_suffix("pt") {
            (n, 12700.0)
        } else if let Some(n) = v.strip_suffix("in") {
            (n, 914_400.0)
        } else if let Some(n) = v.strip_suffix("cm") {
            (n, 360_000.0)
        } else if let Some(n) = v.strip_suffix("mm") {
            (n, 36_000.0)
        } else if let Some(n) = v.strip_suffix("px") {
            (n, 9525.0)
        } else {
            (v.as_str(), 9525.0)
        };
        n.trim().parse::<f64>().ok().filter(|x| x.is_finite() && x.abs() < 1.0e5).map(|x| (x * f) as i64)
    };
    let (w, h) = (get("width").and_then(|v| emu(&v)), get("height").and_then(|v| emu(&v)));
    if let (Some(w), Some(h)) = (w, h) {
        if w > 0 && h > 0 {
            d.extent = Some((w, h));
        }
    }
    d.vml = true;
    if get("position").as_deref() == Some("absolute") {
        d.anchored = true;
        d.pos.off = get("margin-left").or_else(|| get("left")).and_then(|v| emu(&v)).or(Some(0));
        d.pos.off_v = get("margin-top").or_else(|| get("top")).and_then(|v| emu(&v)).or(Some(0));
        d.pos.rel = match get("mso-position-horizontal-relative").as_deref() {
            Some("page") => "page",
            Some("margin") => "margin",
            _ => "column",
        }
        .to_owned();
        d.pos.rel_v = match get("mso-position-vertical-relative").as_deref() {
            Some("page") => "page",
            Some("margin") => "margin",
            _ => "paragraph",
        }
        .to_owned();
        if let Some(a) = get("mso-position-horizontal") {
            if matches!(a.as_str(), "left" | "center" | "right" | "inside" | "outside") {
                d.pos.align = Some(a);
            }
        }
        if let Some(a) = get("mso-position-vertical") {
            if matches!(a.as_str(), "top" | "center" | "bottom" | "inside" | "outside") {
                d.pos.align_v = Some(a);
            }
        }
        d.pos.behind = get("z-index").and_then(|z| z.parse::<i64>().ok()).is_some_and(|z| z < 0);
        d.pos.dist = [0, 0, 114_300, 114_300];
        // Utan `w10:wrap` ligger formen framför texten.
    }
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
        ParaKind::Heading(_) | ParaKind::Quote | ParaKind::Title | ParaKind::Subtitle => para_eff,
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
/// Körningen som markerar en sidbrytning i ett stycke som läses (tas bort av `split_page_breaks`).
const PAGE_BREAK_RUN: &str = "\u{E0F0}";

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

/// Fältkoden (`TOC \o "1-3" \h \z \u`) för förteckningen i `xml`, om den har en.
fn toc_instr(xml: &str) -> Option<String> {
    let mut rest = xml;
    while let Some(i) = rest.find("instrText") {
        let after = &rest[i..];
        let gt = after.find('>')?;
        let body = &after[gt + 1..];
        let end = body.find('<').unwrap_or(body.len());
        let instr = body[..end].trim_start();
        if instr.len() >= 3 && instr[..3].eq_ignore_ascii_case("TOC") && !instr[3..].starts_with(|c: char| c.is_ascii_alphanumeric()) {
            return Some(instr[3..].replace("&quot;", "\"").replace("&amp;", "&"));
        }
        rest = &body[end..];
    }
    None
}

/// Läser en textruta: innehållet som block (egna markörer räknas på samma lista som resten) och formen.
/// `None` om innehållet inte går att visa som text (bilder, fotnoter) eller är tomt.
fn read_box(
    inner: &str,
    shape_xml: &str,
    d: &Drawing,
    (styles, theme, numbering, rels): (&Styles, &Theme, &Numbering, &HashMap<String, (String, String, bool)>),
    dropped: &mut Dropped,
    found: &mut Vec<FoundImage>,
    dir: &str,
    info: &mut BodyInfo,
) -> Option<BoxRead> {
    let doc = format!("<w:body>{inner}</w:body>");
    let mut sub = BodyInfo { in_box: true, tokens: std::mem::take(&mut info.tokens), ..Default::default() };
    // Bilder i rutan läggs på dokumentets lista (deras platshållare räknas där).
    let n_found = found.len();
    let blocks = parse_body(&doc, styles, theme, numbering, rels, dropped, found, dir, &mut sub);
    info.tokens = std::mem::take(&mut sub.tokens);
    let Ok(mut blocks) = blocks else {
        found.truncate(n_found);
        return None;
    };
    if blocks.is_empty() {
        found.truncate(n_found);
        return None;
    }
    // Fotnotsreferenser i rutan visas inte (texten visas, referensen bevaras i originalet).
    if !sub.fn_refs.is_empty() {
        for b in blocks.iter_mut() {
            if let Block::Para(p) = b {
                p.runs.retain(|r| !(r.raw && r.text.starts_with("[^") && r.text.ends_with(']')));
            }
        }
    }
    let shape = docx_tbox::read_shape(shape_xml, &|n| theme.color(n));
    use crate::word::pbox::FrameX;
    let width = d.extent.map_or(0.0, |(cx, _)| cx as f32 / 12700.0);
    // Läget i spalten: justering eller förskjutning från kolumn/marginal. Sidrelativa lägen är inte kända här.
    let x = match (d.anchored, d.pos.align.as_deref()) {
        (true, Some("center")) => FrameX::Center,
        (true, Some("right" | "outside")) => FrameX::End,
        (true, Some(_)) => FrameX::Start,
        (true, None) if matches!(d.pos.rel.as_str(), "" | "column" | "margin" | "character") => FrameX::Off(d.pos.off.unwrap_or(0) as f32 / 12700.0),
        _ => FrameX::Start,
    };
    let page_rel = d.anchored && d.pos.align.is_none() && matches!(d.pos.rel.as_str(), "page" | "leftMargin");
    let x = match (page_rel, x) {
        (true, _) => FrameX::Off(d.pos.off.unwrap_or(0) as f32 / 12700.0),
        (_, x) => x,
    };
    let height = d.extent.map_or(0.0, |(_, cy)| cy as f32 / EMU_PER_PIXEL as f32);
    Some(BoxRead { token: 0, shape, blocks, width, x, page_rel, pos: d.anchored.then(|| d.pos.clone()), height, head: Default::default() })
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
    // `w:pageBreakBefore` i stycket självt (`Some(false)` upphäver formatmallens).
    let mut para_page_before: Option<bool> = None;
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
    let mut run_fnend = false;
    // Hur långt stycket var när körningen började, så att den kan backas om den blir en markör.
    let mut run_mark = (0usize, 0usize);
    let mut run_found_mark = 0usize;
    let mut omath_start: Option<usize> = None;
    // Textrutan i bilden/formen som läses (innehållets början) och den färdigläsa rutan i körningen.
    let mut txbx_start: Option<usize> = None;
    let mut run_box: Option<BoxRead> = None;
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
                            let spec = toc_instr(slice).map(|i| crate::word::nav::TocSpec::from_field_code(&i)).unwrap_or_default();
                            p.runs.push(Run { text: spec.line(), fmt: Fmt::default(), link: None, raw: true });
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
                        b"svgBlip" => {
                            if let Some((t, _, false)) = attr(e, b"embed").and_then(|id| rels.get(&id)) {
                                d.svg.get_or_insert_with(|| t.clone());
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
                                if d.tx.is_none() && !info.in_box {
                                    txbx_start = Some(after);
                                }
                            }
                        }
                        b"anchor" => {
                            d.anchored = true;
                            let n = |k: &[u8]| attr(e, k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).max(0);
                            d.pos.dist = [n(b"distT"), n(b"distB"), n(b"distL"), n(b"distR")];
                            d.pos.behind = matches!(attr(e, b"behindDoc").as_deref(), Some("1" | "true"));
                            if matches!(attr(e, b"simplePos").as_deref(), Some("1" | "true")) {
                                d.pos.simple = Some((0, 0));
                            }
                        }
                        b"simplePos" if d.pos.simple.is_some() => {
                            let n = |k: &[u8]| attr(e, k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                            d.pos.simple = Some((n(b"x"), n(b"y")));
                        }
                        b"wrapSquare" | b"wrapTight" | b"wrapThrough" | b"wrapTopAndBottom" => {
                            d.pos.wrap = Some(if name == b"wrapTopAndBottom" { Wrap::TopBottom } else { Wrap::Square });
                            // Avstånd på wrap-taggen går före anchorns.
                            for (i, k) in [&b"distT"[..], b"distB", b"distL", b"distR"].iter().enumerate() {
                                if let Some(v) = attr(e, k).and_then(|v| v.parse::<i64>().ok()) {
                                    d.pos.dist[i] = v.max(0);
                                }
                            }
                        }
                        // Äldre VML-bild (`v:shape` med `v:imagedata`): storlek och läge ur `style`.
                        b"shape" if d.extent.is_none() => {
                            if let Some(st) = attr(e, b"style") {
                                vml_style(&st, d);
                            }
                        }
                        b"wrap" if d.vml => {
                            d.pos.wrap = match attr(e, b"type").as_deref() {
                                Some("square" | "tight" | "through") => Some(Wrap::Square),
                                Some("topAndBottom") => Some(Wrap::TopBottom),
                                _ => None,
                            };
                        }
                        b"srcRect" => {
                            let n = |k: &[u8]| attr(e, k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                            let c = [n(b"l"), n(b"t"), n(b"r"), n(b"b")];
                            if d.crop.is_none() && c.iter().any(|v| *v != 0) {
                                d.crop = Some(c);
                            }
                        }
                        b"positionH" => {
                            d.pos.rel = attr(e, b"relativeFrom").unwrap_or_default();
                            d.in_pos = if empty { 0 } else { 1 };
                        }
                        b"positionV" => {
                            d.pos.rel_v = attr(e, b"relativeFrom").unwrap_or_default();
                            d.in_pos = if empty { 0 } else { 2 };
                        }
                        b"align" if d.in_pos != 0 && !empty => d.text_of = 1,
                        b"posOffset" if d.in_pos != 0 && !empty => d.text_of = 2,
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
                        b"commentReference" | b"fldChar" | b"instrText" | b"sym" | b"object"
                    )
                {
                    run_opaque = true;
                }
                match name.as_slice() {
                    // Fotnotsreferens: blir `[^N]` i texten (anpassad hänvisning och dolda fält förblir markörer).
                    b"footnoteReference" | b"endnoteReference" if in_run => match attr(e, b"id") {
                        Some(id) if attr(e, b"customMarkFollows").is_none() && !hidden(&field_depth) => {
                            run_fnref = Some(id);
                            run_fnend = name.as_slice() == b"endnoteReference";
                        }
                        _ => run_opaque = true,
                    },
                    // Bilder: samla ihop till ett `![…](bild:…)` när elementet tar slut.
                    b"drawing" | b"pict" => {
                        if !empty {
                            drawing = Some(Drawing { depth: 1, start: before, ..Default::default() });
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
                    // Språkbokmärket i ett kodstycke (`codehl::lang_bookmark`): språket följer med som en dold körning
                    // och bokmärket skrivs om av Kladd; slutet på det (vårt id-intervall) släpps.
                    b"bookmarkStart"
                        if empty
                            && !in_run
                            && para.as_ref().is_some_and(|p| p.kind == ParaKind::Code)
                            && attr(e, b"name").and_then(|n| crate::codehl::lang_from_bookmark(&n)).is_some() =>
                    {
                        let lang = attr(e, b"name").and_then(|n| crate::codehl::lang_from_bookmark(&n)).unwrap_or_default();
                        if let Some(p) = para.as_mut() {
                            p.runs.push(Run { text: crate::codehl::lang_marker(&lang), fmt: Fmt::default(), link: None, raw: true });
                        }
                    }
                    b"bookmarkEnd"
                        if empty
                            && !in_run
                            && para.as_ref().is_some_and(|p| p.kind == ParaKind::Code)
                            && attr(e, b"id").and_then(|v| v.parse::<u32>().ok()).is_some_and(|id| id > LANG_BOOKMARK_ID && id <= LANG_BOOKMARK_ID + 1_000_000) => {}
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
                    b"p" if empty => {
                        // `<w:p/>` på toppnivå: ett tomt stycke (utan egen stängningstagg).
                        if tables.is_empty() && info.body_depth == Some(depth) {
                            let mut p = Para::new();
                            p.origin = info.elems.len().wrapping_sub(1);
                            blocks.push(Block::Para(p));
                        }
                    }
                    b"p" => {
                        para = Some(Para::new());
                        para_marks.clear();
                        para_fchange = None;
                        para_page_before = None;
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
                        // Andra teckenmallar (Subtil betoning, Bokens titel, egna) med fet/kursiv.
                        // (Inte i rubriker: deras teckenmallar är bara rubrikens eget utseende.)
                        let in_head = para.as_ref().is_some_and(|p| matches!(p.kind, ParaKind::Heading(_) | ParaKind::Title | ParaKind::Subtitle));
                        let rp = styles.rchain(attr(e, b"val").unwrap_or_default().as_str());
                        if let (Some(b), false) = (rp.bold, in_head) {
                            fmt.bold = b;
                        }
                        if let (Some(i), false) = (rp.italic, in_head) {
                            fmt.italic = i;
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
                    // Sidbrytning i en körning: stycket delas vid den när det avslutas (`split_page_breaks`).
                    b"br" if !hidden(&field_depth) && attr(e, b"type").as_deref() == Some("page") && tables.is_empty() && para.is_some() && skip == 0 => {
                        para.as_mut().expect("stycke").runs.push(Run { text: PAGE_BREAK_RUN.to_owned(), fmt: Fmt::default(), link: None, raw: true });
                    }
                    b"pageBreakBefore" if in_ppr && para.is_some() => para_page_before = Some(on_val(e)),
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
                        tables.push(Tbl { start: before, ..Tbl::default() });
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
                        if let Some(s) = txbx_start.take() {
                            if let (Some(d), Some(e)) = (drawing.as_mut(), xml[..after].rfind("</")) {
                                d.tx = Some((s, e));
                            }
                        }
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
                        b"positionH" | b"positionV" => d.in_pos = 0,
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
                                svg: d.svg.map(|t| resolve_target(dir, &t)),
                                alt: alt.trim().to_owned(),
                                extent: d.extent,
                                anchor: d.anchored.then_some(d.pos),
                                crop: d.crop,
                                fallback: None,
                            });
                        }
                        Some(_) => {}
                        // Former, diagram, textrutor och länkade bilder bevaras som markörer.
                        None => {
                            run_opaque = true;
                            // Textrutan visas som stycken före ankarstycket (`docx_tbox`).
                            if let Some((s, e)) = d.tx.filter(|_| !info.in_box && !hidden(&field_depth)) {
                                let shape_xml = format!("{}{}", &xml[d.start..s], &xml[e..after]);
                                run_box = read_box(&xml[s..e], &shape_xml, &d, (styles, theme, numbering, rels), dropped, found, dir, info);
                            }
                        }
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
                                let endnote = std::mem::take(&mut run_fnend);
                                info.fn_refs.push(FnRefInfo { id, origin: info.elems.len().wrapping_sub(1), endnote });
                                let label = if endnote {
                                    format!("[^{}]", crate::word::markup::endnote_label(info.fn_refs.iter().filter(|r| r.endnote).count()))
                                } else {
                                    format!("[^{}]", info.fn_refs.iter().filter(|r| !r.endnote).count())
                                };
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
                                if let Some(mut b) = run_box.take() {
                                    b.token = info.tokens.len() - 1;
                                    para.get_or_insert_with(Para::new).boxes.push(b);
                                }
                            } else if found.len() > run_found_mark {
                                // Bilder som inte går att visa ersätts av hela körningen.
                                let n = new_token(info, run_xml);
                                for f in &mut found[run_found_mark..] {
                                    f.fallback = Some(n);
                                }
                            }
                            run_opaque = false;
                            run_drop = false;
                            run_box = None;
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
                                t.boxes.append(&mut p.boxes);
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
                                // Sidbrytning före stycket (`w:pageBreakBefore`, också ur formatmallen) och i det.
                                let before = para_page_before.take().unwrap_or_else(|| styles.page_before(p.style.as_deref()));
                                let pieces = split_page_breaks(p);
                                let n = pieces.len();
                                if before && !matches!(blocks.last(), Some(Block::Para(q)) if q.origin == origin && q.runs.first().is_some_and(|r| r.raw && r.text == sections::PAGE_BREAK_LINE)) {
                                    blocks.push(Block::Para(raw_para(sections::PAGE_BREAK_LINE.to_owned(), origin)));
                                }
                                let has_sect = para_sect.is_some();
                                for (i, mut p) in pieces.into_iter().enumerate() {
                                    if i > 0 {
                                        blocks.push(Block::Para(raw_para(sections::PAGE_BREAK_LINE.to_owned(), origin)));
                                    }
                                    // Ett stycke som bara bär en sidbrytning blir bara brytningsraden.
                                    if n > 1 && is_blank_para(&p) && !(has_sect && i + 1 == n) {
                                        continue;
                                    }
                                    // Stycket som bär en avsnitts- eller spaltbrytning är ingen blankrad.
                                    if (has_sect || para_col_break) && is_blank_para(&p) {
                                        p.runs.push(Run { text: String::new(), fmt: Fmt::default(), link: None, raw: true });
                                    }
                                    if i == 0 && std::mem::take(&mut para_col_break) {
                                        blocks.push(Block::Para(raw_para(sections::COLUMN_BREAK_LINE.to_owned(), origin)));
                                    }
                                    blocks.push(Block::Para(p));
                                }
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
                        if let Some(mut t) = tables.pop() {
                            if tables.is_empty() {
                                t.read = crate::docx_tables::read(&xml[t.start..after], &styles.tables);
                            }
                            if let Some(outer) = tables.last_mut() {
                                // Nästlad tabell: platta till i den yttre cellen.
                                let flat: Vec<String> = t.rows.iter().map(|r| r.iter().filter(|c| !c.is_empty()).cloned().collect::<Vec<_>>().join(" ")).collect();
                                outer.boxes.append(&mut t.boxes);
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
                    let v = d.in_pos == 2;
                    match (d.text_of, v) {
                        (1, false) => d.pos.align = Some(t.trim().to_ascii_lowercase()),
                        (1, true) => d.pos.align_v = Some(t.trim().to_ascii_lowercase()),
                        (_, false) => d.pos.off = t.trim().parse().ok(),
                        (_, true) => d.pos.off_v = t.trim().parse().ok(),
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

/// Styckeutseendet (ram, skuggning, avstånd, indrag) ur stycket, dess mall och docDefaults, för
/// de stycken som kommer från ett toppnivåelement i brödtexten (`word::pbox`, markören `k:pp`).
/// Ett tomt stycke (bara blanktecken, inga objekt): används som lodrätt avstånd i Word.
/// Delar ett stycke vid sidbrytningarna (`PAGE_BREAK_RUN`) i dess körningar; ett stycke utan
/// sidbrytning blir ett enda. Listpunkten (och dess tecken) gäller bara första delen.
fn split_page_breaks(p: Para) -> Vec<Para> {
    if !p.runs.iter().any(|r| r.raw && r.text == PAGE_BREAK_RUN) {
        return vec![p];
    }
    let mut out: Vec<Para> = Vec::new();
    let rest = Para { runs: Vec::new(), list: None, ..p.clone() };
    let mut cur = Para { runs: Vec::new(), ..p.clone() };
    for r in p.runs {
        if r.raw && r.text == PAGE_BREAK_RUN {
            out.push(std::mem::replace(&mut cur, rest.clone()));
        } else {
            cur.runs.push(r);
        }
    }
    out.push(cur);
    out
}

fn is_blank_para(p: &Para) -> bool {
    p.kind == ParaKind::Normal && p.list.is_none() && p.origin != usize::MAX && p.runs.iter().all(|r| !r.raw && r.link.is_none() && r.text.trim().is_empty())
}

fn fill_para_looks(blocks: &mut [Block], elems: &[String], ps: &crate::docx_para::ParaStyles) {
    for b in blocks.iter_mut() {
        let Block::Para(p) = b else { continue };
        let Some(xml) = elems.get(p.origin).filter(|x| x.starts_with("<w:p>") || x.starts_with("<w:p ")) else { continue };
        if p.runs.iter().any(|r| r.raw && r.text.starts_with("<!-- kladd:")) || matches!(p.kind, ParaKind::Code | ParaKind::Quote) || p.list.is_some() {
            continue;
        }
        let direct = crate::docx_para::direct_ppr(xml);
        let plain = p.kind == ParaKind::Normal;
        // Rubriker, Titel och Underrubrik: deras mallar (med ram och skuggning) ligger redan i
        // grundformatets formatmallar (`StyleSet`); bara stycket eget utseende blir en markör.
        let eff = if plain { ps.resolve(p.style.as_deref(), &direct) } else { direct.clone().finish() };
        p.ppr = crate::docx_para::for_marker(&eff, plain);
        if is_blank_para(p) {
            p.mark_sz = crate::docx_para::mark_size(xml);
        }
    }
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

/// Visas styckets textrutor? Inte i kodblock.
fn box_shown(p: &Para) -> bool {
    !p.boxes.is_empty() && p.kind != ParaKind::Code
}

/// Rutans läge i spalten och dess öppningsrad (`docx_tbox::Head`) ur ankaret. Justering mot kolumnen/marginalen
/// följer spalten; allt annat räknas om till ett avstånd från spaltens vänsterkant.
fn place_box(bx: &mut BoxRead, page: &DocDefaults) {
    let Some(pos) = bx.pos.as_ref() else { return };
    use crate::word::anchor::PageCtx;
    use crate::word::pbox::FrameX;
    let a = anchor_of(pos, page);
    let k = 96.0 / 72.0;
    let (pw, ph) = page.page();
    let m = page.margins();
    let ctx = PageCtx { page_w: pw * k, page_h: ph * k, margin: m.map(|v| v * k), col_left: m[3] * k, col_w: (pw - m[1] - m[3]) * k };
    let w_px = bx.width * k;
    bx.x = match (a.hrel, a.h) {
        (HRel::Column | HRel::Margin, Pos::Start) => FrameX::Start,
        (HRel::Column | HRel::Margin, Pos::Center) => FrameX::Center,
        (HRel::Column | HRel::Margin, Pos::End) => FrameX::End,
        // Rutan får stå ut i marginalen (som i Word) men klämms mot papperets kant.
        _ => FrameX::Off((a.x_in_column(w_px, &ctx) / k).clamp(-m[3], (pw - m[3] - bx.width).max(-m[3]))),
    };
    bx.page_rel = false;
    bx.head = docx_tbox::Head { w: w_px, h: bx.height, anchor: Some(a) };
}

/// Rutans styckemarkör (ram, fyllning och bredd) och innehåll som Markdown.
fn box_md(b: &BoxRead, numbering: &Numbering, text_w: Option<f64>) -> (String, String) {
    // Rutans bredd och läge räknas mot spaltens bredd först när dokumentet ritas (`pbox::Frame`).
    let _ = text_w;
    let pp = docx_tbox::frame(&b.shape, b.x, b.width, b.height * 0.75);
    let mut blocks = b.blocks.clone();
    for bl in blocks.iter_mut() {
        if let Block::Para(p) = bl {
            if p.ppr.is_empty() && !is_blank_para(p) {
                p.ppr = pp.clone();
            }
        }
    }
    (pp.marker(), to_markdown_units(&blocks, numbering, None).0.trim().to_owned())
}

/// Rutans text utan den syntetiska ramen, för att jämföra mot originalet.
fn strip_frame(md: &str, marker: &str) -> String {
    let s = if marker.is_empty() { md.to_owned() } else { md.replace(marker, "") };
    s.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_owned()
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
    // Ett dokument som bara består av ett tomt stycke (som `write` själv lägger dit) ger ingen rad.
    let only_blank = matches!(blocks, [Block::Para(p)] if is_blank_para(p) && para_md(p).trim().is_empty());
    // Words obligatoriska tomma slutstycke efter en avslutande tabell (som `write` också lägger dit) ger ingen rad.
    let after_table_end = matches!(blocks, [.., Block::Table(..), Block::Para(p)] if is_blank_para(p) && para_md(p).trim().is_empty());
    let last_content = if only_blank {
        0
    } else if after_table_end {
        blocks.len() - 1
    } else {
        blocks.len()
    };
    for (bi, b) in blocks.iter().enumerate() {
        match b {
            Block::Para(p) => {
                // Textrutor: block före stycket (`docx_tbox`), inga enheter av sig själva.
                if box_shown(p) {
                    for b in &p.boxes {
                        let md = box_md(b, numbering, text_w).1;
                        if md.is_empty() {
                            continue;
                        }
                        if cur_align == Align::Left {
                            if !out.is_empty() {
                                out.push_str("\n\n");
                            }
                        } else {
                            out.push_str("\n\n");
                        }
                        out.push_str(&format!("{}\n\n{md}\n\n{}", docx_tbox::open_tag_head(b.token, &b.head), docx_tbox::CLOSE));
                        prev_list = false;
                        top_num = None;
                    }
                }
                let mut text = para_md(p);
                // Tomt stycke: en egen rad `<!--k:ep-->` med stycketeckens storlek och avstånd.
                let blank = is_blank_para(p) && text.trim().is_empty() && bi < last_content;
                if blank {
                    text = crate::word::pbox::empty_marker(&p.ppr, p.mark_sz);
                } else if without_tokens(&text).trim().is_empty() {
                    continue;
                }
                // Ram, skuggning, avstånd och indrag: dold markör sist i stycket (spårade stycketecken
                // `k:pm` ska stå sist, så de stycken får ingen).
                if !blank && !p.ppr.is_empty() && !text.contains("<!--k:pm ") && !text.trim_start().starts_with("<!--") && !text.starts_with("&#8203;") && !text.trim_start().starts_with('\u{e000}') {
                    text.push_str(&p.ppr.marker());
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
                let is_list = p.list.is_some() && !matches!(p.kind, ParaKind::Heading(_) | ParaKind::Title | ParaKind::Subtitle);
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
                    (k @ (ParaKind::Title | ParaKind::Subtitle), _) => {
                        let tk = if k == ParaKind::Title { TitleKind::Title } else { TitleKind::Subtitle };
                        let one_line = text.replace('\n', " ");
                        let h = format!("{} {} {}", "#".repeat(tk.level() as usize), one_line.trim(), tk.marker());
                        units.push(MdUnit { origin: p.origin, desc: format!("{}|{}", tk.class(), align.as_attr()), key: norm(&h) });
                        out.push_str(&h);
                    }
                    (ParaKind::Quote, None) => {
                        let q: Vec<String> = text.lines().map(|l| format!("> {}", escape_line_start(l))).collect();
                        let q = q.join("\n");
                        units.push(MdUnit { origin: p.origin, desc: format!("q|{}", align.as_attr()), key: norm(&q) });
                        out.push_str(&q);
                    }
                    (ParaKind::Code, None) => {
                        // Språket (dold körning, se `codehl::lang_bookmark`) blir inhägnadens infosträng.
                        let lang = p.runs.iter().find_map(|r| r.raw.then(|| crate::codehl::parse_lang_marker(&r.text)).flatten()).unwrap_or("");
                        let raw: String = p.runs.iter().filter(|r| !(r.raw && crate::codehl::parse_lang_marker(&r.text).is_some())).map(|r| r.text.as_str()).collect();
                        let fence = if raw.contains("```") { "~~~~" } else { "```" };
                        let code = format!("{fence}{lang}\n{}\n{fence}", raw.trim_matches('\n'));
                        units.push(MdUnit { origin: p.origin, desc: format!("c|{}", align.as_attr()), key: norm(&code) });
                        out.push_str(&code);
                    }
                    (ParaKind::Normal, None) if blank => {
                        units.push(MdUnit { origin: p.origin, desc: format!("e|{}", align.as_attr()), key: norm(&text) });
                        out.push_str(&text);
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
                for b in &tbl.boxes {
                    let md = box_md(b, numbering, text_w).1;
                    if md.is_empty() {
                        continue;
                    }
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(&format!("{}\n\n{md}\n\n{}", docx_tbox::open_tag_head(b.token, &b.head), docx_tbox::CLOSE));
                }
                prev_list = false;
                top_num = None;
                // Rubrikraden skrivs i fetstil av Kladd; visa den utan **.
                let mut rows = tbl.rows.clone();
                let (fmt, widths) = crate::docx_tables::prepare(&mut rows, &tbl.spans, tbl.read.as_ref(), &tbl.widths, text_w);
                if tbl.read.is_none() && rows[0].iter().all(|c| c.is_empty() || (c.len() > 4 && c.starts_with("**") && c.ends_with("**"))) {
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
                // Tabellens format (skuggning, ramar …) i en dold kommentarsrad före tabellen.
                if let Some(f) = &fmt {
                    out.push_str(&f.to_comment());
                    out.push_str("\n\n");
                }
                let start = out.len();
                out.push_str(&markup::table_markdown(&rows, &tbl.spans, &widths));
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
    /// Titel/Underrubrik (`{.title}` / `{.subtitle}`): stycket får Wordstilen Title/Subtitle.
    title: Option<TitleKind>,
    table: Option<TableState>,
    in_code_block: bool,
    /// Pågående kodblock: språket ur inhägnaden och koden hittills (färgas när blocket tar slut).
    code_lang: String,
    code_buf: String,
    /// Var i brödtexten det pågående kodblocket börjar (ett ritat Mermaid-diagram ersätter det).
    code_mark: usize,
    /// Antal skrivna språkbokmärken (för unika `w:id`).
    n_lang: u32,
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
    /// Rubriker och bildtexter med sidor ur Kladds utläggning (för sidnumren i förteckningar).
    toc_laid: Option<Vec<(u8, String, usize)>>,
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
    /// Styckeutseende (`<!--k:pp …-->`) som väntar på styckets slut.
    pending_pp: Option<crate::word::pbox::PPr>,
    /// Tabellformatet (`<!-- kladd-tabell: … -->`) som väntar på tabellen efter det.
    pending_tfmt: Option<crate::word::tablefmt::TableFmt>,
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
    /// Tabellens format, om den har något.
    fmt: Option<crate::word::tablefmt::TableFmt>,
    /// Justering för styckena i cellen som skrivs.
    cell_jc: Option<&'static str>,
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
            pending_pp: None,
            fmt: WFmt::default(),
            lists: Vec::new(),
            item_first_para: false,
            quote: 0,
            heading: None,
            title: None,
            pending_tfmt: None,
            table: None,
            in_code_block: false,
            code_lang: String::new(),
            code_buf: String::new(),
            code_mark: 0,
            n_lang: 0,
            image: None,
            media: Vec::new(),
            missing: Vec::new(),
            spans: Vec::new(),
            aligns: Vec::new(),
            footnotes: Vec::new(),
            fn_order: Vec::new(),
            in_footnote: false,
            toc_n: 0,
            toc_laid: None,
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
    /// resultat är posterna (rubriker i `spec`s nivåer, eller bildtexter) med sidnumren ur Kladds egen
    /// utläggning (samma som PDF-exporten). Word visar de sparade numren tills fältet uppdateras (F9);
    /// `w:updateFields` begärs bara när utläggningen inte gick att para ihop med posterna.
    fn toc(&mut self, md: &str, spec: &crate::word::nav::TocSpec) {
        use crate::word::nav;
        self.toc_n += 1;
        let heads = nav::headings_scan(md);
        let entries = nav::toc_items(md, &heads, spec);
        let pages = self.toc_laid.as_ref().map(|l| crate::pdf_write::toc_pages_of(l, spec)).filter(|p| p.len() == entries.len());
        let stale = pages.is_none() && !entries.is_empty();
        // Textytans bredd i tjugondels punkter (EMU / 635).
        let width = if self.ctx.max_width > 0 { self.ctx.max_width / 635 } else { 9026 };
        let tab = format!("<w:tabs><w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"{width}\"/></w:tabs>");
        let id = 1_000_000 + self.toc_n;
        let tag = if stale { TOC_STALE_TAG } else { TOC_TAG };
        let mut x = format!(
            "<w:sdt><w:sdtPr><w:id w:val=\"{id}\"/><w:docPartObj><w:docPartGallery w:val=\"Table of Contents\"/><w:docPartUnique/></w:docPartObj><w:tag w:val=\"{tag}\"/></w:sdtPr><w:sdtContent>"
        );
        x.push_str(&format!("<w:p><w:pPr><w:pStyle w:val=\"TOCHeading\"/></w:pPr><w:r><w:t>{}</w:t></w:r></w:p>", spec.title()));
        let begin = format!(
            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\">{}</w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>",
            spec.field_code()
        );
        for (i, h) in entries.iter().enumerate() {
            // Posterna följer Words TOC 1–9: indrag per nivå (bildtextlistor har ingen).
            let ind = if spec.kind == nav::TocKind::Headings { (i32::from(h.level) - 1) * 220 } else { 0 };
            let mut t = String::new();
            xml_escape(&h.text, &mut t);
            let page = pages.as_ref().map_or(1, |p| p[i]);
            let level = if spec.kind == nav::TocKind::Headings { h.level.min(9) } else { 1 };
            x.push_str(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"TOC{level}\"/>{tab}<w:spacing w:after=\"100\"/><w:ind w:left=\"{ind}\"/></w:pPr>{}<w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>{page}</w:t></w:r></w:p>",
                if i == 0 { begin.as_str() } else { "" },
            ));
        }
        if entries.is_empty() {
            x.push_str(&format!(
                "<w:p><w:pPr><w:pStyle w:val=\"TOC1\"/>{tab}</w:pPr>{begin}<w:r><w:t>Inga poster</w:t></w:r></w:p>"
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

    /// Bäddar in en mediefil (en gång per innehåll) och ger dess relations-id. En fil som redan finns i
    /// originalpaketet återanvänds.
    fn media_id(&mut self, bytes: Vec<u8>, ext: &str) -> String {
        let existing = self
            .media
            .iter()
            .find(|(_, _, b)| *b == bytes)
            .map(|(id, _, _)| id.clone())
            .or_else(|| self.ctx.media.iter().find(|(_, b)| *b == bytes.as_slice()).map(|(id, _)| id.clone()));
        match existing {
            Some(id) => id,
            None => {
                let id = self.fresh_id("rIdImg");
                let mut n = self.media.len() + 1;
                let file = loop {
                    let f = format!("image{n}.{ext}");
                    if !self.ctx.taken.contains(&format!("word/media/{f}").to_lowercase()) && !self.media.iter().any(|(_, m, _)| *m == f) {
                        break f;
                    }
                    n += 1;
                };
                self.media.push((id.clone(), file, bytes));
                id
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn picture(&mut self, bytes: Vec<u8>, svg: Option<Vec<u8>>, natural: Option<(f32, f32)>, extent: Option<(i64, i64)>, alt: &str, place: Option<(Option<Float>, Option<Anchor>)>, crop: Option<[f64; 4]>) {
        let Some(ext) = image_ext(&bytes) else { return };
        let (cx, cy) = image_extent(natural.or_else(|| imagesize::blob_size(&bytes).ok().map(|s| (s.width as f32, s.height as f32))), extent, self.ctx.max_width);
        let id = self.media_id(bytes, ext);
        // Vektorbild som Word 2016+: PNG-reserven i `a:blip` och SVG-filen i tillägget `asvg:svgBlip`.
        let svg_ext = match svg {
            Some(data) => {
                let sid = self.media_id(data, "svg");
                format!(r#"<a:extLst><a:ext uri="{{96DAC541-7B7A-43D3-8B79-37D633B846F1}}"><asvg:svgBlip xmlns:asvg="{SVG_NS}" r:embed="{sid}"/></a:ext></a:extLst>"#)
            }
            None => String::new(),
        };
        let doc_pr = self.doc_pr_next;
        self.doc_pr_next += 1;
        let mut alt_x = String::new();
        xml_escape(alt, &mut alt_x);
        // Radbrytningar i ett attribut måste vara teckenreferenser, annars blir de blanksteg.
        let alt_x = alt_x.replace('\r', "").replace('\n', "&#10;");
        self.open_para();
        const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
        let blip = if svg_ext.is_empty() { format!(r#"<a:blip r:embed="{id}"/>"#) } else { format!(r#"<a:blip r:embed="{id}">{svg_ext}</a:blip>"#) };
        // Gemensam del efter placeringen: storlek, beskrivning och själva bilden.
        let tail = format!(
            r#"<wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>{{WRAP}}<wp:docPr id="{doc_pr}" name="Bild {doc_pr}" descr="{alt_x}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{doc_pr}" name="Bild {doc_pr}" descr="{alt_x}"/><pic:cNvPicPr><a:picLocks noChangeAspect="1" noChangeArrowheads="1"/></pic:cNvPicPr></pic:nvPicPr><pic:blipFill>{blip}{{SRC_RECT}}<a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr bwMode="auto"><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>"#
        );
        let src_rect = crop
            .map(|c| format!(r#"<a:srcRect l="{}" t="{}" r="{}" b="{}"/>"#, (c[0] * 1000.0).round() as i64, (c[1] * 1000.0).round() as i64, (c[2] * 1000.0).round() as i64, (c[3] * 1000.0).round() as i64))
            .unwrap_or_default();
        let tail = tail.replace("{SRC_RECT}", &src_rect);
        // Dialektens ankare, eller kortformen `align` (vid vänster/höger textkant, 12 px luft).
        let anchor = match place {
            Some((_, Some(a))) => Some(a),
            Some((Some(side), None)) => Some(Anchor {
                h: if side == Float::Left { Pos::Start } else { Pos::End },
                dist: if side == Float::Left { [0.0, 12.0, 0.0, 0.0] } else { [0.0, 0.0, 0.0, 12.0] },
                ..Anchor::default()
            }),
            _ => None,
        };
        let drawing = match anchor {
            Some(a) => {
                let emu = |v: f32| ((v as f64) * EMU_PER_PIXEL as f64).round() as i64;
                let pos = |p: Pos, names: [&str; 3]| match p {
                    Pos::Off(o) => format!("<wp:posOffset>{}</wp:posOffset>", emu(o)),
                    Pos::Start => format!("<wp:align>{}</wp:align>", names[0]),
                    Pos::Center => format!("<wp:align>{}</wp:align>", names[1]),
                    Pos::End => format!("<wp:align>{}</wp:align>", names[2]),
                };
                let hrel = match a.hrel {
                    HRel::Column => "column",
                    HRel::Margin => "margin",
                    HRel::Page => "page",
                };
                let vrel = match a.vrel {
                    VRel::Paragraph => "paragraph",
                    VRel::Margin => "margin",
                    VRel::Page => "page",
                };
                let wrap = match a.wrap {
                    Wrap::Square => r#"<wp:wrapSquare wrapText="bothSides"/>"#,
                    Wrap::TopBottom => "<wp:wrapTopAndBottom/>",
                    Wrap::Behind | Wrap::Front => "<wp:wrapNone/>",
                };
                let [dt, dr, db, dl] = a.dist.map(emu);
                format!(
                    r#"<wp:anchor xmlns:wp="{WP}" xmlns:r="{R_NS}" distT="{dt}" distB="{db}" distL="{dl}" distR="{dr}" simplePos="0" relativeHeight="{}" behindDoc="{}" locked="0" layoutInCell="1" allowOverlap="{}"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="{hrel}">{}</wp:positionH><wp:positionV relativeFrom="{vrel}">{}</wp:positionV>{}</wp:anchor>"#,
                    251_658_240 + doc_pr,
                    u8::from(a.wrap == Wrap::Behind),
                    u8::from(!a.wrap.affects_text()),
                    pos(a.h, ["left", "center", "right"]),
                    pos(a.v, ["top", "center", "bottom"]),
                    tail.replace("{WRAP}", wrap)
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
        if let Some(t) = self.title {
            ppr.push_str(match t {
                TitleKind::Title => "<w:pStyle w:val=\"Title\"/>",
                TitleKind::Subtitle => "<w:pStyle w:val=\"Subtitle\"/>",
            });
        } else if let Some(h) = self.heading {
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
        if let Some(j) = self.table.as_ref().and_then(|t| t.cell_jc) {
            ppr.push_str(&format!("<w:jc w:val=\"{j}\"/>"));
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
            self.apply_para_look();
            self.apply_para_marks();
            self.body.push_str("</w:p>");
            self.para_open = false;
        }
        self.pending_pm.clear();
        self.pending_pp = None;
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

    /// Ram, skuggning, avstånd och indrag ur markören `<!--k:pp …-->` skrivs in i styckets `w:pPr`
    /// (i schemaordning). Orörda och omskrivna stycken får sin ursprungliga `w:pPr` tillbaka senare.
    fn apply_para_look(&mut self) {
        let Some(pp) = self.pending_pp.take() else { return };
        let head = self.para_at;
        if !self.body[head..].starts_with("<w:p>") {
            return;
        }
        let old_len = if self.ppr.is_empty() { 0 } else { 7 + self.ppr.len() + 8 };
        let merged = crate::docx_para::merge_ppr(&self.ppr, &pp);
        if merged.is_empty() || !self.body.is_char_boundary(head + 5 + old_len) {
            return;
        }
        self.body.replace_range(head + 5..head + 5 + old_len, &format!("<w:pPr>{merged}</w:pPr>"));
        self.ppr = merged;
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

    /// Skriver kodblocket som körningar i kodformatet, färgade per token (`w:color`). Texten är oförändrad.
    fn flush_code(&mut self) {
        let buf = std::mem::take(&mut self.code_buf);
        let code = buf.strip_suffix('\n').unwrap_or(&buf);
        let colors = crate::codehl::CodeColors::paper();
        let hl = crate::codehl::tokens_wait(&self.code_lang, code);
        for (range, class, _bold, italic) in crate::codehl::segments(&hl, code) {
            let colored = class != crate::codehl::Class::Plain;
            if colored {
                self.spans.push(SpanStyle { color: Some(colors.rgb(class)), ..Default::default() });
            }
            if italic {
                self.fmt.italic += 1;
            }
            self.run(&code[range], true);
            if italic {
                self.fmt.italic -= 1;
            }
            if colored {
                self.spans.pop();
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
    fn image_ref(&mut self, tag: &ImgTag, resolve: &dyn Fn(&str) -> Option<Image>) {
        let (src, alt) = (tag.src.as_str(), tag.alt.as_str());
        if self.in_footnote {
            self.run(alt, false);
            return;
        }
        match resolve(src) {
            Some(img) if is_supported_image(&img.bytes) => {
                let extent = sized_extent(&img, (tag.width, tag.height), tag.crop);
                // En bild i en tabellcell eller i en lista flyter inte.
                let place = if self.table.is_some() { None } else { Some((tag.align, tag.anchor)) };
                let natural = img.natural_px();
                self.picture(img.bytes, img.svg.map(|s| s.data), natural, extent, alt, place, tag.crop);
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
    anchor: Option<Anchor>,
    /// Beskärning i procent: vänster, ovan, höger, under.
    crop: Option<[f64; 4]>,
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
    let mut extra: Vec<(String, String)> = Vec::new();
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
            _ => extra.push((name, value.clone())),
        }
    }
    let anchor = Anchor::from_attrs(&|k| extra.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()));
    let crop = extra.iter().find(|(n, _)| n == "crop").and_then(|(_, v)| {
        let n: Vec<f64> = v.split_whitespace().filter_map(|s| s.parse::<f64>().ok()).filter(|x| x.is_finite() && (0.0..100.0).contains(x)).collect();
        (n.len() == 4 && n[0] + n[2] < 100.0 && n[1] + n[3] < 100.0 && n.iter().any(|x| *x > 0.0)).then(|| [n[0], n[1], n[2], n[3]])
    });
    Some(ImgTag { src: src.filter(|s| !s.is_empty())?, alt, width, height, align, anchor, crop })
}

/// Bildens storlek i EMU när taggen anger bredd och/eller höjd i CSS-pixlar (bevarar proportionerna).
fn sized_extent(img: &Image, size: (Option<f64>, Option<f64>), crop: Option<[f64; 4]>) -> Option<(i64, i64)> {
    let px = EMU_PER_PIXEL as f64;
    let natural = img.natural_px().map(|s| (s.0 as f64, s.1 as f64)).or(img.extent.map(|(x, y)| (x as f64, y as f64)));
    // Den synliga delen av en beskuren bild bestämmer proportionerna och grundstorleken.
    let natural = natural.map(|(w, h)| match crop {
        Some(c) => (w * (1.0 - (c[0] + c[2]) / 100.0), h * (1.0 - (c[1] + c[3]) / 100.0)),
        None => (w, h),
    });
    match (size, natural) {
        ((None, None), _) => img.extent,
        ((Some(w), Some(h)), _) => Some(((w * px) as i64, (h * px) as i64)),
        ((Some(w), None), Some((nw, nh))) => Some(((w * px) as i64, (w * nh / nw * px) as i64)),
        ((None, Some(h)), Some((nw, nh))) => Some(((h * nw / nh * px) as i64, (h * px) as i64)),
        _ => img.extent,
    }
}

/// Enhetsnyckeln för en förteckningsrad: raden och en kontrollsumma över posterna, så att en förteckning
/// skrivs om när rubrikerna (eller bildtexterna) ändrats och annars behåller originalets XML.
///
/// `heads` är rubrikerna i `md`; de söks fram första gången en förteckning behöver dem och delas av alla
/// förteckningar i texten (annars skannas hela dokumentet en gång per förteckning).
fn toc_unit_key(key: &str, md: &str, heads: &mut Option<Vec<crate::word::nav::Heading>>) -> String {
    use std::hash::{Hash, Hasher};
    let Some(spec) = crate::word::nav::parse_toc(key) else { return key.to_owned() };
    let heads = heads.get_or_insert_with(|| crate::word::nav::headings_scan(md));
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for e in crate::word::nav::toc_items(md, heads, &spec) {
        (e.level, e.text.trim()).hash(&mut h);
    }
    format!("{key}\u{1}{:016x}", h.finish())
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
    opts.insert(titles::OPTS);
    let mut next_num: u32 = base + 1; // base = punktlistor
    // Rubrikerna i `md` för förteckningarnas kontrollsummor (söks fram första gången de behövs).
    let mut toc_heads: Option<Vec<crate::word::nav::Heading>> = None;
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
                        Tag::HtmlBlock if crate::word::pbox::parse_empty(md.get(range.clone()).unwrap_or("").trim()).is_some() => "e".to_owned(),
                        Tag::Paragraph | Tag::HtmlBlock => "p".to_owned(),
                        Tag::Heading { level, classes, .. } => match titles::kind(classes) {
                            Some(k) => k.class().to_owned(),
                            None => format!("h{}", heading_num(*level)),
                        },
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
                Tag::Heading { level, classes, .. } => {
                    w.close_para();
                    w.title = titles::kind(&classes);
                    w.heading = Some(heading_num(level));
                    w.open_para();
                }
                Tag::BlockQuote(_) => {
                    w.close_para();
                    w.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    w.close_para();
                    w.in_code_block = true;
                    w.code_lang = match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(info) => crate::codehl::lang_of(&info),
                        pulldown_cmark::CodeBlockKind::Indented => String::new(),
                    };
                    w.code_buf.clear();
                    w.code_mark = w.body.len();
                    w.open_para();
                    // Språket sparas i ett dolt bokmärke först i stycket (se `lang_bookmark`).
                    if let Some(name) = crate::codehl::lang_bookmark(&w.code_lang).filter(|_| !w.code_lang.is_empty()) {
                        w.n_lang += 1;
                        let id = LANG_BOOKMARK_ID + w.n_lang;
                        w.body.push_str(&format!("<w:bookmarkStart w:id=\"{id}\" w:name=\"{name}\"/><w:bookmarkEnd w:id=\"{id}\"/>"));
                    }
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
                    let text_total = if w.ctx.max_width > 0 { (w.ctx.max_width / 635) as u32 } else { 9026 };
                    let tfmt = w.pending_tfmt.take();
                    let total = match tfmt.as_ref().map(|f| f.width) {
                        Some(crate::word::tablefmt::Width::Pct(p)) => (f64::from(text_total) * f64::from(p) / 100.0).round() as u32,
                        Some(crate::word::tablefmt::Width::Pt(p)) => u32::from(p) * 20,
                        _ => text_total,
                    };
                    // Kolumnbredd efter antalet streck i avgränsarraden, annars lika breda.
                    let widths: Vec<u32> = match src.weights.as_ref().filter(|ws| ws.len() == cols) {
                        Some(ws) => {
                            let sum: usize = ws.iter().sum();
                            ws.iter().map(|&n| (f64::from(total) * n as f64 / sum as f64).round() as u32).collect()
                        }
                        None => vec![total / cols as u32; cols],
                    };
                    match &tfmt {
                        Some(f) => w.body.push_str(&format!(
                            "<w:tbl><w:tblPr>{}<w:tblLook w:val=\"04A0\" w:firstRow=\"0\" w:lastRow=\"0\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"1\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>",
                            crate::docx_tables::tbl_pr_xml(f)
                        )),
                        None => w.body.push_str(
                            "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"5000\" w:type=\"pct\"/><w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"1\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>",
                        ),
                    }
                    for cw in &widths {
                        w.body.push_str(&format!("<w:gridCol w:w=\"{cw}\"/>"));
                    }
                    w.body.push_str("</w:tblGrid>");
                    w.table = Some(TableState { in_head: false, widths, spans: src.spans, rows: 0, cell: 0, covered: false, fmt: tfmt, cell_jc: None });
                }
                Tag::TableHead => {
                    if let Some(t) = w.table.as_mut() {
                        t.in_head = true;
                        t.rows += 1;
                        t.cell = 0;
                    }
                    let pr = match w.table.as_ref().and_then(|t| t.fmt.as_ref()) {
                        Some(f) => crate::docx_tables::tr_pr_xml(f, 0, true),
                        None => "<w:tblHeader/>".to_owned(),
                    };
                    w.body.push_str(&format!("<w:tr><w:trPr>{pr}</w:trPr>"));
                }
                Tag::TableRow => {
                    if let Some(t) = w.table.as_mut() {
                        t.rows += 1;
                        t.cell = 0;
                    }
                    let pr = match w.table.as_ref() {
                        Some(t) => t.fmt.as_ref().map(|f| crate::docx_tables::tr_pr_xml(f, t.rows - 1, false)).unwrap_or_default(),
                        None => String::new(),
                    };
                    if pr.is_empty() {
                        w.body.push_str("<w:tr>");
                    } else {
                        w.body.push_str(&format!("<w:tr><w:trPr>{pr}</w:trPr>"));
                    }
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
                        let extra = match w.table.as_ref() {
                            Some(t) => t.fmt.as_ref().map(|f| crate::docx_tables::tc_pr_xml(f, t.rows.saturating_sub(1), t.cell)).unwrap_or_default(),
                            None => String::new(),
                        };
                        let jc = w.table.as_ref().and_then(|t| {
                            let f = t.fmt.as_ref()?;
                            match f.cell(t.rows.saturating_sub(1), t.cell)?.halign {
                                crate::word::tablefmt::TAlign::Left => None,
                                crate::word::tablefmt::TAlign::Center => Some("center"),
                                crate::word::tablefmt::TAlign::Right => Some("right"),
                            }
                        });
                        if let Some(t) = w.table.as_mut() {
                            t.cell_jc = jc;
                        }
                        w.body.push_str(&format!("<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>{grid}{extra}</w:tcPr>"));
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
                    w.title = None;
                }
                TagEnd::BlockQuote(_) => {
                    w.close_para();
                    w.quote = w.quote.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    // Ett Mermaid-block som går att rita skrivs som bild (SVG + PNG-reserv) med koden i alt-texten.
                    let diagram = (w.table.is_none() && !w.in_footnote && crate::mermaid::is_lang(&w.code_lang))
                        .then(|| crate::mermaid::image(&w.code_buf).and_then(|k| crate::mermaid::resolve(&k)))
                        .flatten();
                    if let Some(img) = diagram {
                        w.body.truncate(w.code_mark);
                        w.para_open = false;
                        w.pending_pm.clear();
                        w.pending_pp = None;
                        w.fchange_para = None;
                        let alt = format!("{MERMAID_ALT}{}", w.code_buf.trim_end_matches('\n'));
                        w.code_buf.clear();
                        w.in_code_block = false;
                        w.code_lang.clear();
                        let natural = img.natural_px();
                        w.picture(img.bytes, img.svg.map(|s| s.data), natural, None, &alt, None, None);
                        w.close_para();
                    } else {
                        w.flush_code();
                        // Ta bort en avslutande radbrytning i kodblocket.
                        if w.body.ends_with("<w:r><w:br/></w:r>") {
                            let n = w.body.len() - "<w:r><w:br/></w:r>".len();
                            w.body.truncate(n);
                        }
                        w.close_para();
                        w.in_code_block = false;
                        w.code_lang.clear();
                    }
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
                        w.image_ref(&ImgTag { src, alt, width: None, height: None, align: None, anchor: None, crop: None }, resolve);
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
                    w.code_buf.push_str(&t);
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
                if crate::word::tablefmt::is_comment(s.trim()) && w.table.is_none() {
                    // Tabellens format: gäller tabellen som följer.
                    w.pending_tfmt = crate::word::tablefmt::TableFmt::from_comment(s.trim());
                } else if let Some(ns) = parse_tokens(&s) {
                    for n in ns {
                        w.token(n);
                    }
                } else if let Some((start, id)) = review::parse_marker(s.trim()) {
                    w.comment_marker(start, id);
                } else if let (Some((pp, sz)), true) = (crate::word::pbox::parse_empty(s.trim()), w.table.is_none() && w.quote == 0) {
                    // Tomt stycke med stycketecknets storlek och avstånd.
                    w.close_para();
                    let mut inner = w.para_props();
                    if let Some(sz) = sz {
                        let half = (sz * 2.0).round() as i64;
                        inner.push_str(&format!("<w:rPr><w:sz w:val=\"{half}\"/><w:szCs w:val=\"{half}\"/></w:rPr>"));
                    }
                    let inner = crate::docx_para::merge_ppr(&inner, &pp);
                    if inner.is_empty() {
                        w.body.push_str("<w:p/>");
                    } else {
                        w.body.push_str(&format!("<w:p><w:pPr>{inner}</w:pPr></w:p>"));
                    }
                } else if let Some(pp) = crate::word::pbox::PPr::parse_marker(s.trim()) {
                    w.pending_pp = Some(pp);
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
                    w.image_ref(&img, resolve);
                } else if let (Some(spec), true) = (crate::word::nav::parse_toc(s.trim()), w.table.is_none() && w.quote == 0) {
                    w.close_para();
                    if w.toc_laid.is_none() {
                        w.toc_laid = Some(crate::pdf_write::toc_laid_pages(md, resolve));
                    }
                    w.toc(md, &spec);
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
                } else if sections::is_page_break(&s) && w.table.is_none() && w.quote == 0 {
                    w.close_para();
                    w.body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
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
                if crate::word::markup::is_endnote_label(&t) {
                    w.body.push_str(&format!(
                        "<w:r><w:rPr><w:rStyle w:val=\"EndnoteReference\"/></w:rPr><w:endnoteReference w:id=\"{ph}\"/></w:r>"
                    ));
                } else {
                    w.body.push_str(&format!(
                        "<w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteReference w:id=\"{ph}\"/></w:r>"
                    ));
                }
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
                            || crate::word::nav::is_toc_line(&key)
                            || sections::SectionProps::from_comment(&key).is_some()
                            || sections::is_column_break(&key)
                            || sections::is_page_break(&key)
                            || crate::word::pbox::parse_empty(&key).is_some();
                        if ok && !key.is_empty() {
                            // En förteckning är oförändrad bara om rubrikerna är det.
                            let key = toc_unit_key(&key, md, &mut toc_heads);
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

fn image_extent(natural: Option<(f32, f32)>, extent: Option<(i64, i64)>, max_width: i64) -> (i64, i64) {
    let max_width = if max_width > 0 { max_width } else { MAX_IMAGE_WIDTH_EMU };
    let (mut cx, mut cy) = extent.unwrap_or_else(|| match natural {
        Some((w, h)) => (w as i64 * EMU_PER_PIXEL, h as i64 * EMU_PER_PIXEL),
        None => (MAX_IMAGE_WIDTH_EMU / 2, MAX_IMAGE_WIDTH_EMU / 3),
    });
    cx = cx.max(1);
    cy = cy.max(1);
    if cx > max_width {
        cy = cy * max_width / cx;
        cx = max_width;
    }
    (cx, cy.max(1))
}

/// Början på alt-texten för ett Mermaid-diagram som bild: resten är diagramkoden, som läses tillbaka som ```mermaid.
const MERMAID_ALT: &str = "kladd-mermaid\n";

/// Diagramkoden ur en alt-text som Kladd skrivit för ett Mermaid-diagram.
fn mermaid_code(alt: &str) -> Option<&str> {
    alt.strip_prefix(MERMAID_ALT).map(|c| c.trim_matches('\n')).filter(|c| !c.trim().is_empty())
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="svg" ContentType="image/svg+xml"/><Default Extension="jpeg" ContentType="image/jpeg"/><Default Extension="gif" ContentType="image/gif"/><Default Extension="bmp" ContentType="image/bmp"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/></Relationships>"#;

/// Märker en innehållsförteckning som Kladd skrivit (då begärs en uppdatering av fälten).
const TOC_TAG: &str = "kladd-toc";

/// Märker en innehållsförteckning vars sidnummer inte kunde läggas ut: då begärs en uppdatering av fälten.
const TOC_STALE_TAG: &str = "kladd-toc-stale";

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
    // Word-lika Title och Subtitle (Calibri Light 28 pt respektive 14 pt).
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="10"/><w:qFormat/><w:pPr><w:spacing w:after="120" w:line="240" w:lineRule="auto"/><w:contextualSpacing/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi"/><w:color w:val="1F3864"/><w:sz w:val="56"/><w:szCs w:val="56"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Subtitle"><w:name w:val="Subtitle"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="11"/><w:qFormat/><w:pPr><w:spacing w:after="160"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi"/><w:color w:val="595959"/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style>"#);
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="TOCHeading"><w:name w:val="TOC Heading"/><w:basedOn w:val="Heading1"/><w:next w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:qFormat/><w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>"#);
    for n in 1..=9 {
        let ind = (n - 1) * 220;
        s.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="TOC{n}"><w:name w:val="toc {n}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:autoRedefine/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9016"/></w:tabs><w:spacing w:after="100"/><w:ind w:left="{ind}"/></w:pPr></w:style>"#
        ));
    }
    s.push_str(r#"<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="29"/><w:qFormat/><w:pPr><w:spacing w:before="120"/><w:ind w:left="567" w:right="567"/><w:pBdr><w:left w:val="single" w:sz="12" w:space="8" w:color="A0A0A0"/></w:pBdr></w:pPr><w:rPr><w:i/><w:color w:val="404040"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="34"/><w:qFormat/><w:pPr><w:spacing w:after="60"/><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="39"/><w:qFormat/><w:pPr><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style><w:style w:type="character" w:styleId="CodeChar"><w:name w:val="Code Char"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="39"/><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:rPr></w:style><w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style><w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="footnote reference"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="FootnoteText"><w:name w:val="footnote text"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style><w:style w:type="character" w:styleId="EndnoteReference"><w:name w:val="endnote reference"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="EndnoteText"><w:name w:val="endnote text"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:uiPriority w:val="39"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style>"#);
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
/// byter den. `title_pg` slår på eller av annorlunda första sida, `pg_num` ändrar sidnumreringen
/// (start, format) och `dist` avståndet till sidkanten (sidhuvud, sidfot).
#[derive(Default)]
struct SectEdit {
    hdr: Option<Option<String>>,
    hdr_first: Option<Option<String>>,
    hdr_even: Option<Option<String>>,
    ftr: Option<Option<String>>,
    ftr_first: Option<Option<String>>,
    ftr_even: Option<Option<String>>,
    title_pg: Option<bool>,
    pg_num: Option<(Option<u32>, Option<crate::hf::NumFmt>)>,
    dist: Option<(Option<f32>, Option<f32>)>,
}

impl SectEdit {
    fn is_empty(&self) -> bool {
        self.hdr.is_none()
            && self.hdr_first.is_none()
            && self.hdr_even.is_none()
            && self.ftr.is_none()
            && self.ftr_first.is_none()
            && self.ftr_even.is_none()
            && self.title_pg.is_none()
            && self.pg_num.is_none()
            && self.dist.is_none()
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
    if let Some((h, f)) = edit.dist {
        x = edit_tag(&x, "<w:pgMar", |t| {
            let mut t = t.to_owned();
            for (n, v) in [("header", h), ("footer", f)] {
                t = set_xml_attr(&t, &format!("w:{n}"), &twips(v.unwrap_or(crate::hf::DEFAULT_DIST)).to_string());
            }
            if !t.contains("w:gutter=") {
                t = set_xml_attr(&t, "w:gutter", "0");
            }
            t
        });
    }
    for (tag, ty, change) in [
        ("<w:footerReference", "even", &edit.ftr_even),
        ("<w:footerReference", "first", &edit.ftr_first),
        ("<w:headerReference", "even", &edit.hdr_even),
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
    if let Some((start, fmt)) = edit.pg_num {
        x = remove_tag(&x, "<w:pgNumType");
        if start.is_some() || fmt.is_some() {
            let mut tag = String::from("<w:pgNumType");
            if let Some(f) = fmt {
                tag.push_str(&format!(" w:fmt=\"{}\"", f.docx_val()));
            }
            if let Some(s) = start {
                tag.push_str(&format!(" w:start=\"{s}\""));
            }
            tag.push_str("/>");
            // Schemat: efter pgMar (och ramar, radnummer), före cols.
            let at = ["<w:cols", "<w:formProt", "<w:vAlign", "<w:noEndnote", "<w:titlePg", "<w:textDirection", "<w:bidi", "<w:rtlGutter", "<w:docGrid", "<w:printerSettings", "</w:sectPr>"]
                .iter()
                .filter_map(|m| x.find(m))
                .min();
            if let Some(at) = at {
                x.insert_str(at, &tag);
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
    hdr_even: Option<String>,
    ftr: Option<String>,
    ftr_first: Option<String>,
    ftr_even: Option<String>,
}

impl HfRefs {
    /// Skriver de delar av `d` som har text och ger deras id:n.
    fn make(extras: &mut Extras, d: &DocDefaults, f: &style::Fields, ids: &mut std::collections::HashSet<String>, taken: &mut std::collections::HashSet<String>, resolve: &dyn Fn(&str) -> Option<Image>) -> HfRefs {
        let mut one = |header: bool, text: Option<&str>| {
            let align = if header { d.header_align } else { d.footer_align };
            text.filter(|t| !t.is_empty()).map(|t| extras.header_footer(header, t, align, d, f, ids, taken, resolve))
        };
        HfRefs {
            hdr: one(true, d.header.as_deref()),
            hdr_first: one(true, d.header_first.as_deref()),
            hdr_even: one(true, d.header_even.as_deref()),
            ftr: one(false, d.footer.as_deref()),
            ftr_first: one(false, d.footer_first.as_deref()),
            ftr_even: one(false, d.footer_even.as_deref()),
        }
    }
}

/// Tomt stycke som avslutar ett avsnitt; `fix_sections` fyller i egenskaperna.
/// Innehållet i `w:sectPr` i `SECT_PARA` (ett avsnitt som skrivs om från grunden).
const GEN_SECT: &str = "<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr>";
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

/// Spaltbredderna `a` och `b` har samma proportioner (båda tomma = lika breda).
fn same_widths(a: &[f32], b: &[f32]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let (sa, sb): (f32, f32) = (a.iter().sum(), b.iter().sum());
    a.iter().zip(b).all(|(x, y)| (x / sa - y / sb).abs() < 0.01)
}

/// `w:cols` för ett avsnitt med flera spalter: antal, mellanrum, skiljelinje och olika bredder.
fn cols_xml(sec: &sections::Section) -> String {
    let sep = if sec.rule { " w:sep=\"1\"" } else { "" };
    if sec.col_widths.len() != sec.columns {
        return format!("<w:cols w:num=\"{}\" w:space=\"{}\"{sep}/>", sec.columns, twips(sec.gap));
    }
    // Bredderna skalas så att de med mellanrummen fyller textytan.
    let n = sec.columns as f32;
    let text_w = (sec.page.0 - sec.margin[1] - sec.margin[3]).max(1.0);
    let avail = (text_w - sec.gap * (n - 1.0)).max(1.0);
    let sum: f32 = sec.col_widths.iter().sum();
    let mut out = format!("<w:cols w:num=\"{}\"{sep} w:equalWidth=\"0\">", sec.columns);
    for (i, w) in sec.col_widths.iter().enumerate() {
        let w = twips(w / sum * avail);
        if i + 1 < sec.columns {
            out.push_str(&format!("<w:col w:w=\"{w}\" w:space=\"{}\"/>", twips(sec.gap)));
        } else {
            out.push_str(&format!("<w:col w:w=\"{w}\"/>"));
        }
    }
    out.push_str("</w:cols>");
    out
}

/// Sätter sida, marginaler, spalter och avsnittstyp i en `w:sectPr` efter avsnittet `sec`. Är allt
/// redan lika lämnas XML:en orörd (förlustfri sparning). `refs` = sidhuvud- och sidfotsreferenser att
/// lägga först om `x` saknar egna (första avsnittet när brödtextens sectPr bär dem).
fn apply_section(orig: &str, sec: &sections::Section, refs: Option<&str>) -> String {
    let raw = read_sect(orig);
    let (w, h) = sec.page;
    let size_ok = raw.size.is_some_and(|(rw, rh)| same_pt(rw, w, 1.0) && same_pt(rh, h, 1.0));
    let mar_ok = raw.margin.is_some_and(|m| m.iter().zip(sec.margin).all(|(a, b)| same_pt(*a, b, 0.6)));
    let cols_ok = raw.cols == sec.columns
        && (sec.columns == 1 || (raw.gap.is_some_and(|g| same_pt(g, sec.gap, 1.0)) && raw.rule == sec.rule && same_widths(&raw.widths, &sec.col_widths)));
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
            cols_xml(sec)
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
            if one.contains("w:type=\"default\"") || one.contains("w:type=\"first\"") || one.contains("w:type=\"even\"") {
                out.push_str(one);
            }
            rest = &rest[i + j + 1..];
        }
    }
    out
}

/// Sätter egenskaperna för avsnittens slut (`w:sectPr` i stycken, i ordning) efter avsnitten i texten.
/// `refs` = sidhuvuds- och sidfotsreferenserna som också ska gälla första avsnittet om det saknar egna.
fn fix_sections(body: &str, secs: &[sections::Section], refs: &str, replace_refs: bool, edits: &[SectEdit], orig: &[String]) -> String {
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
        // Ett avsnitt som skrivs om från grunden får originalets sidhuvud, sidfot och sidnumrering.
        // Första avsnittet utan egna referenser (dess `w:sectPr` skrevs om eller byttes) får originalets.
        let no_refs = !one.contains("<w:headerReference") && !one.contains("<w:footerReference");
        let seed_from = orig.get(k).filter(|_| one == GEN_SECT).or_else(|| orig.first().filter(|_| k == 0 && no_refs && !header_refs(&orig[0]).is_empty()));
        let seeded = match seed_from {
            Some(o) => seed_hf(&one, o),
            None => one.clone(),
        };
        let one = seeded.as_str();
        let edited;
        let one = match edits.get(k).filter(|e| !e.is_empty()) {
            Some(e) => {
                edited = patch_sect(one, &DocDefaults::default(), false, e);
                edited.as_str()
            }
            None => one,
        };
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
        ("header", "even", &refs.hdr_even),
        ("footer", "default", &refs.ftr),
        ("footer", "first", &refs.ftr_first),
        ("footer", "even", &refs.ftr_even),
    ] {
        if let Some(id) = id {
            s.push_str(&format!("<w:{tag}Reference w:type=\"{ty}\" r:id=\"{id}\"/>"));
        }
    }
    let orient = if w > h { " w:orient=\"landscape\"" } else { "" };
    let pg_num = match (d.page_start, d.page_format) {
        (None, None) => String::new(),
        (st, fm) => format!(
            "<w:pgNumType{}{}/>",
            fm.map_or(String::new(), |f| format!(" w:fmt=\"{}\"", f.docx_val())),
            st.map_or(String::new(), |v| format!(" w:start=\"{v}\""))
        ),
    };
    s.push_str(&format!(
        "<w:pgSz w:w=\"{}\" w:h=\"{}\"{orient}/><w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/>{pg_num}<w:cols w:space=\"708\"/>{title_pg}</w:sectPr>",
        twips(w),
        twips(h),
        twips(m[0]),
        twips(m[1]),
        twips(m[2]),
        twips(m[3]),
        d.header_dist.map_or(708, twips),
        d.footer_dist.map_or(708, twips),
        title_pg = if d.different_first() { "<w:titlePg/>" } else { "" }
    ));
    s
}

/// Delar som läggs till i ett paket: relationer, innehållstyper och nya filer.
#[derive(Default)]
struct Extras {
    rels: String,
    types: String,
    parts: Vec<(String, Vec<u8>)>,
    /// Filändelser för bilder i sidhuvuden och sidfötter (innehållstyper).
    exts: Vec<String>,
}

impl Extras {
    /// Lägger till sidhuvud/sidfot och ger dess relations-id. `align` gäller ett enkelt värde utan `|`,
    /// `st` är textens utseende, `geo` sidans mått och `text_w` textbredden.
    #[allow(clippy::too_many_arguments)]
    fn header_footer(
        &mut self,
        header: bool,
        text: &str,
        align: Align,
        d: &DocDefaults,
        f: &style::Fields,
        ids: &mut std::collections::HashSet<String>,
        taken: &mut std::collections::HashSet<String>,
        resolve: &dyn Fn(&str) -> Option<Image>,
    ) -> String {
        let text_w = twips(((d.page().0 - d.margins()[1] - d.margins()[3]).max(36.0)) as f32) as i64;
        let st = if header { &d.header_style } else { &d.footer_style };
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
        let out = hf::write_part(root, text, align, st, text_w, f, resolve, taken, d.page());
        if !out.rels.is_empty() {
            self.parts.push((format!("word/_rels/{stem}{n}.xml.rels"), hf::rels_xml(&out.rels).into_bytes()));
        }
        for (file, bytes) in out.media {
            if let Some(ext) = file.rsplit('.').next() {
                self.exts.push(ext.to_owned());
            }
            self.parts.push((format!("word/media/{file}"), bytes));
        }
        self.parts.push((name, out.xml.into_bytes()));
        rid
    }
}

fn image_mime(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "svg" => "image/svg+xml",
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
    // Textrutornas block hör inte till brödtexten: med original byts deras innehåll in i rutan,
    // utan original blir stycknen vanliga stycken.
    let (markdown, boxes, new_boxes) = match source {
        Some(src) => docx_tbox::extract(&markdown, &|n| src.tboxes.contains_key(&n), src.tokens.len()),
        None => docx_tbox::extract(&markdown, &|_| false, 0),
    };
    match source {
        Some(src) => write_patched(src, defaults, &markdown, (&boxes, &new_boxes), title, resolve, &rv),
        None => write_fresh(defaults, &markdown, &new_boxes, title, resolve, &rv),
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

fn write_fresh(defaults: DocDefaults, markdown: &str, new_boxes: &[docx_tbox::Found], title: &str, resolve: &dyn Fn(&str) -> Option<Image>, rv: &Review) -> Result<Written, String> {
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
    let mut tokens: Vec<Opaque> = Vec::new();
    let mut doc_pr = 1u32;
    add_new_boxes(new_boxes, &mut tokens, resolve, 1, &mut doc_pr, (text_width_emu(&defaults) / 12700) as f32);
    let mut ctx = Ctx {
        tokens: tokens.len(),
        num_base: 1,
        doc_pr_start: doc_pr,
        max_width: text_width_emu(&defaults),
        used_ids: ["rId1", "rId2", "rId3"].into_iter().map(str::to_owned).collect(),
        lead_section: secs[0].marker.is_some(),
        ..Default::default()
    };
    rv.apply(&mut ctx);
    let mut w = body_from_markdown(markdown, resolve, ctx);
    let mut body = std::mem::take(&mut w.body);
    if !tokens.is_empty() {
        body = resolve_markers(&[(false, body)], &tokens, &mut std::collections::HashSet::new()).0;
    }
    // Ett dokument som slutar med en tabell måste ha ett stycke efter den.
    if body.is_empty() || body.ends_with("</w:tbl>") {
        body.push_str("<w:p/>");
    }
    let (body, footnotes_xml, endnotes_xml) = resolve_footnotes(body, &w, &HashMap::new(), None, None);
    let comments_out = review_xml::build_comments(None, &rv.comments, &review_xml::anchored_ids(&body));
    let mut extras = Extras::default();
    let fields = style::Fields::for_export(title, "docx");
    let refs = HfRefs::make(&mut extras, &defaults, &fields, &mut w.ids, &mut taken, resolve);
    let sect = sect_pr_new(&defaults, &refs);
    // Senare avsnitt med egna sidhuvuden, sidfötter eller sidnumrering skriver egna referenser.
    let mut edits: Vec<SectEdit> = (0..secs.len())
        .map(|i| {
            if i == 0 {
                SectEdit::default()
            } else {
                section_hf_edit(None, &secs[i].props.hf, &crate::hf::SecHf::default(), &defaults, &mut extras, &fields, &mut w.ids, &mut taken, resolve)
            }
        })
        .collect();
    // Avståndet till sidkanten gäller alla avsnitt; sidnumreringen är avsnittets egen (första: grundformatet).
    let dist = (defaults.header_dist.is_some() || defaults.footer_dist.is_some()).then_some((defaults.header_dist, defaults.footer_dist));
    if secs.len() > 1 {
        for (i, e) in edits.iter_mut().enumerate() {
            e.dist = dist;
            if i == 0 && (defaults.page_start.is_some() || defaults.page_format.is_some()) {
                e.pg_num = Some((defaults.page_start, defaults.page_format));
            }
        }
    }
    let mut last_edit = edits.pop().unwrap_or_default();
    if secs.len() > 1 {
        last_edit.dist = dist;
        if let Some(l) = secs.last() {
            last_edit.pg_num = Some((l.props.hf.page_start, l.props.hf.page_fmt));
        }
    }
    let body = fix_sections(&body, &secs, &header_refs(&sect), false, &edits, &[]);
    // Sidhuvud och sidfot hör till första avsnittet (de senare ärver); annorlunda första sida gäller bara det.
    let sect = secs.last().map_or(sect.clone(), |s| apply_section(&sect, s, None));
    let sect = if secs.len() > 1 { remove_title_pg(&sect) } else { sect };
    let sect = if last_edit.is_empty() { sect } else { patch_sect(&sect, &defaults, false, &last_edit) };
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
    if endnotes_xml.is_some() {
        doc_rels.push_str(&format!(r#"<Relationship Id="rIdKladdEn1" Type="{REL_NS}/endnotes" Target="endnotes.xml"/>"#));
        extras.types.push_str(&format!(r#"<Override PartName="/word/endnotes.xml" ContentType="{ENDNOTES_MIME}"/>"#));
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
        ("[Content_Types].xml".into(), patch_content_types(CONTENT_TYPES, &{ let mut e: Vec<&str> = extras.exts.iter().map(String::as_str).collect(); e.sort_unstable(); e.dedup(); e }, &extras.types).into_bytes()),
        ("_rels/.rels".into(), ROOT_RELS.as_bytes().to_vec()),
        ("word/document.xml".into(), document.into_bytes()),
        ("word/_rels/document.xml.rels".into(), doc_rels.into_bytes()),
        ("docProps/core.xml".into(), core_xml(title, "").into_bytes()),
        ("docProps/app.xml".into(), APP_XML.as_bytes().to_vec()),
        ("word/styles.xml".into(), crate::docx_styles::patch_styles(&styles_xml(&defaults), &style::StyleSet::default(), &defaults.styles).into_bytes()),
        ("word/numbering.xml".into(), numbering_xml(&w.ordered_nums).into_bytes()),
        (
            "word/settings.xml".into(),
            if body.contains(TOC_STALE_TAG) { with_update_fields(SETTINGS) } else { SETTINGS.to_owned() }.into_bytes(),
        ),
    ];
    if let Some(xml) = footnotes_xml {
        parts.push(("word/footnotes.xml".into(), xml.into_bytes()));
        if let Some(p) = parts.iter_mut().find(|(n, _)| n == "word/settings.xml") {
            let cur = String::from_utf8_lossy(&p.1).into_owned();
            p.1 = cur.replace("<w:compat>", r#"<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr><w:compat>"#).into_bytes();
        }
    }
    if let Some(xml) = endnotes_xml {
        parts.push(("word/endnotes.xml".into(), xml.into_bytes()));
        if let Some(p) = parts.iter_mut().find(|(n, _)| n == "word/settings.xml") {
            let cur = String::from_utf8_lossy(&p.1).into_owned();
            p.1 = cur.replace("<w:compat>", r#"<w:endnotePr><w:endnote w:id="-1"/><w:endnote w:id="0"/></w:endnotePr><w:compat>"#).into_bytes();
        }
    }
    if defaults.notes != crate::word::notefmt::NoteFormats::default() {
        if let Some(p) = parts.iter_mut().find(|(n, _)| n == "word/settings.xml") {
            p.1 = crate::docx_notes::patch_settings(&decode_xml(&p.1), &Default::default(), &defaults.notes).into_bytes();
        }
    }
    if let Some(c) = comments_out {
        parts.push(("word/comments.xml".into(), c.xml.into_bytes()));
        if let Some(e) = c.ext {
            parts.push(("word/commentsExtended.xml".into(), e.into_bytes()));
        }
    }
    if defaults.header_even.is_some() || defaults.footer_even.is_some() {
        if let Some(p) = parts.iter_mut().find(|(n, _)| n == "word/settings.xml") {
            p.1 = hf::with_even_odd(&decode_xml(&p.1), true).into_bytes();
        }
    }
    parts.extend(extras.parts);
    for (_, file, bytes) in &w.media {
        parts.push((format!("word/media/{file}"), bytes.clone()));
    }
    Ok(Written { bytes: write_zip(parts)?, missing_images: w.missing })
}

/// Kopierar sidhuvud-/sidfotsreferenser, sidnumrering, annorlunda första sida och avstånden till
/// sidkanten från originalets `w:sectPr` (`orig`) till en nyskriven (`x`).
fn seed_hf(x: &str, orig: &str) -> String {
    let mut out = x.to_owned();
    let refs = header_refs(orig);
    if let Some(oe) = out.find('>') {
        out.insert_str(oe + 1, &refs);
    }
    let tag_of = |name: &str| orig.find(name).and_then(|i| orig[i..].find('>').map(|j| orig[i..i + j + 1].to_owned()));
    let mut edit = SectEdit::default();
    if let Some(t) = tag_of("<w:pgNumType") {
        let n = |k: &str| t.split(&format!("w:{k}=\"")).nth(1).and_then(|r| r.split('"').next()).map(str::to_owned);
        edit.pg_num = Some((n("start").and_then(|v| v.parse().ok()), n("fmt").and_then(|v| crate::hf::NumFmt::from_docx(&v))));
    }
    if let Some(t) = tag_of("<w:pgMar") {
        let n = |k: &str| t.split(&format!("w:{k}=\"")).nth(1).and_then(|r| r.split('"').next()).and_then(|v| v.parse::<f32>().ok()).map(|v| v / 20.0);
        edit.dist = Some((n("header"), n("footer")));
    }
    if orig.contains("<w:titlePg") && !orig.contains("<w:titlePg w:val=\"0\"") {
        edit.title_pg = Some(true);
    }
    patch_sect(&out, &DocDefaults::default(), false, &edit)
}

/// Ändringarna av ett avsnitts sidhuvuden och sidfötter (`new` mot `old`, avsnittets egna värden).
/// `aligns` finns för första avsnittet (dokumentets grundformat): ((huvud, fot) nu, (huvud, fot) förut).
/// I senare avsnitt betyder `Some("")` en uttrycklig tom del och `None` att föregående avsnitts ärvs.
#[allow(clippy::too_many_arguments)]
fn section_hf_edit(
    aligns: Option<((Align, Align), (Align, Align))>,
    new: &crate::hf::SecHf,
    old: &crate::hf::SecHf,
    d: &DocDefaults,
    extras: &mut Extras,
    f: &style::Fields,
    ids: &mut std::collections::HashSet<String>,
    taken: &mut std::collections::HashSet<String>,
    resolve: &dyn Fn(&str) -> Option<Image>,
) -> SectEdit {
    let first_sec = aligns.is_some();
    let ((nh, nf), (oh, of)) = aligns.unwrap_or(((Align::Left, Align::Left), (Align::Left, Align::Left)));
    let mut e = SectEdit::default();
    let mut slot = |header: bool, kind: u8, nt: &Option<String>, ot: &Option<String>| -> Option<Option<String>> {
        let (na, oa) = if header { (nh, oh) } else { (nf, of) };
        let changed = if first_sec {
            if kind == 0 { hf_changed(nt.as_deref(), na, ot.as_deref(), oa) } else { first_changed(nt.as_deref(), na, ot.as_deref(), oa) }
        } else {
            nt != ot
        };
        if !changed {
            return None;
        }
        let align = if header { d.header_align } else { d.footer_align };
        let mut make = |t: &str| extras.header_footer(header, t, align, d, f, ids, taken, resolve);
        Some(match nt {
            Some(t) if first_sec => (!t.is_empty()).then(|| make(t)),
            Some(t) => Some(make(t)),
            None => None,
        })
    };
    e.hdr = slot(true, 0, &new.header, &old.header);
    e.ftr = slot(false, 0, &new.footer, &old.footer);
    e.hdr_first = slot(true, 1, &new.header_first, &old.header_first);
    e.ftr_first = slot(false, 1, &new.footer_first, &old.footer_first);
    e.hdr_even = slot(true, 2, &new.header_even, &old.header_even);
    e.ftr_even = slot(false, 2, &new.footer_even, &old.footer_even);
    let (nt, ot) = (new.title_pg == Some(true), old.title_pg == Some(true));
    if nt != ot {
        e.title_pg = Some(nt);
    }
    if (new.page_start, new.page_fmt) != (old.page_start, old.page_fmt) {
        e.pg_num = Some((new.page_start, new.page_fmt));
    }
    e
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
fn footnote_ids_in(xml: &str, end: bool) -> Vec<String> {
    let open = if end { "<w:endnoteReference" } else { "<w:footnoteReference" };
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(open) {
        rest = &rest[i + open.len()..];
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
fn footnote_xml(id: &str, paras: &str, end: bool) -> String {
    const LEAD: &str = "<w:pPr><w:pStyle w:val=\"FootnoteText\"/></w:pPr><w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space=\"preserve\"> </w:t></w:r>";
    const LEAD_END: &str = "<w:pPr><w:pStyle w:val=\"EndnoteText\"/></w:pPr><w:r><w:rPr><w:rStyle w:val=\"EndnoteReference\"/></w:rPr><w:endnoteRef/></w:r><w:r><w:t xml:space=\"preserve\"> </w:t></w:r>";
    let lead = if end { LEAD_END } else { LEAD };
    let body = match paras.strip_prefix("<w:p>") {
        Some(rest) => {
            let rest = if rest.starts_with("<w:pPr>") { rest.find("</w:pPr>").map_or(rest, |e| &rest[e + 8..]) } else { rest };
            format!("<w:p>{lead}{rest}")
        }
        None => format!("<w:p>{lead}</w:p>"),
    };
    let tag = if end { "endnote" } else { "footnote" };
    format!("<w:{tag} w:id=\"{id}\">{body}</w:{tag}>")
}

const FOOTNOTES_HEAD: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    "\n",
    r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#
);
const FOOTNOTE_SEPARATORS: &str = r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#;
const ENDNOTES_HEAD: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    "\n",
    r#"<w:endnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#
);
const ENDNOTE_SEPARATORS: &str = r#"<w:endnote w:type="separator" w:id="-1"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:separator/></w:r></w:p></w:endnote><w:endnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:endnote>"#;
const ENDNOTES_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml";
const FOOTNOTES_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";

/// Knyter ihop fotnotsreferenserna i brödtexten med definitionerna: ger varje referens ett id,
/// byter platshållarna mot idn och bygger footnotes.xml. `bound` är referenser (index i
/// `w.fn_order`) som ligger i orört originalstycke och redan har sitt id. En fotnot vars text är
/// oförändrad får sin ursprungliga XML (med sitt id); övriga skrivs om. Svaret är den färdiga
/// brödtexten och, om dokumentet har fotnoter, hela footnotes.xml.
fn resolve_footnotes(body: String, w: &Writer<'_>, bound: &HashMap<usize, String>, orig: Option<&FnPart>, orig_end: Option<&FnPart>) -> (String, Option<String>, Option<String>) {
    let (ids_fn, xml_fn) = resolve_note_kind(&body, w, bound, orig, false);
    let (ids_en, xml_en) = resolve_note_kind(&body, w, bound, orig_end, true);
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
        if let Some(n) = r[..e].parse::<usize>().ok() {
            if let Some(id) = ids_fn.get(n).and_then(Option::as_ref).or(ids_en.get(n).and_then(Option::as_ref)) {
                out.push_str(id);
            }
        }
        rest = &r[e + '\u{E005}'.len_utf8()..];
    }
    out.push_str(rest);
    (out, xml_fn, xml_en)
}

/// Fotnoterna (`end` = falskt) eller slutnoterna (`end` = sant): id för varje referens i `fn_order`
/// (`None` för den andra sortens) och hela footnotes.xml / endnotes.xml.
fn resolve_note_kind(body: &str, w: &Writer<'_>, bound: &HashMap<usize, String>, orig: Option<&FnPart>, end: bool) -> (Vec<Option<String>>, Option<String>) {
    let mine = |label: &str| crate::word::markup::is_endnote_label(label) == end;
    let present = footnote_ids_in(body, end);
    let mut ids: Vec<Option<String>> = vec![None; w.fn_order.len()];
    if !w.fn_order.iter().any(|l| mine(l)) && present.is_empty() && orig.is_none() {
        return (ids, None);
    }
    let items: &[FnOrig] = orig.map_or(&[], |o| o.items.as_slice());
    let normal = |id: &str| items.iter().find(|f| f.id == id && f.ty.is_none());
    // Nästa lediga id: över alla som finns eller används.
    let mut next_id = items
        .iter()
        .map(|f| f.id.as_str())
        .chain(present.iter().map(String::as_str))
        .chain(bound.iter().filter(|(i, _)| w.fn_order.get(**i).is_some_and(|l| mine(l))).map(|(_, v)| v.as_str()))
        .filter_map(|i| i.parse::<i64>().ok())
        .max()
        .unwrap_or(0)
        .max(0)
        + 1;
    let mut taken_ids: std::collections::HashSet<String> = present.iter().cloned().collect();
    let mut entries: Vec<(String, String)> = Vec::new(); // (id, XML)
    let mut label_done: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (i, label) in w.fn_order.iter().enumerate() {
        if !mine(label) {
            continue;
        }
        let def = w.footnotes.iter().find(|d| d.label == *label);
        let first = label_done.insert(label.as_str());
        let id = match bound.get(&i) {
            Some(id) => id.clone(),
            None => {
                // Oförändrad text: den ursprungliga noten (om ingen annan redan använder den).
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
        ids[i] = Some(id.clone());
        if bound.contains_key(&i) && !first {
            continue; // en andra referens i ett orört stycke: noten finns redan i originalet
        }
        let Some(def) = def else { continue };
        let xml = match normal(&id) {
            Some(f) if norm(&f.md) == def.key => f.xml.clone(),
            _ => footnote_xml(&id, &def.xml, end),
        };
        entries.push((id, xml));
    }

    if entries.is_empty() && orig.is_none() {
        return (ids, None);
    }
    let emitted: std::collections::HashSet<&str> = entries.iter().map(|(i, _)| i.as_str()).collect();
    let mut xml = String::new();
    let (head, tail) = match (orig, end) {
        (Some(o), _) => (o.head.as_str(), o.tail.as_str()),
        (None, false) => (FOOTNOTES_HEAD, "</w:footnotes>"),
        (None, true) => (ENDNOTES_HEAD, "</w:endnotes>"),
    };
    xml.push_str(head);
    // Skiljelinjerna först, sedan noterna i textens ordning, sist de som bara hänvisas till av okända objekt.
    if orig.is_none() {
        xml.push_str(if end { ENDNOTE_SEPARATORS } else { FOOTNOTE_SEPARATORS });
    }
    for f in items.iter().filter(|f| f.ty.is_some()) {
        xml.push_str(&f.xml);
    }
    // Noter som fanns förut behåller sin ordning i filen; nya kommer efter, i textens ordning.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&k| items.iter().position(|f| f.id == entries[k].0).unwrap_or(usize::MAX));
    for k in order {
        xml.push_str(&entries[k].1);
    }
    for f in items.iter().filter(|f| f.ty.is_none() && present.contains(&f.id) && !emitted.contains(f.id.as_str())) {
        xml.push_str(&f.xml);
    }
    xml.push_str(tail);
    (ids, Some(xml))
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

/// Originalkörningar för nya textrutor (Infoga → Textruta): token för token, i samma ordning som
/// `docx_tbox::extract` numrerade dem (de börjar där `tokens` slutar).
fn add_new_boxes(new: &[docx_tbox::Found], tokens: &mut Vec<Opaque>, resolve: &dyn Fn(&str) -> Option<Image>, num_base: u32, doc_pr: &mut u32, text_w_pt: f32) {
    for f in new {
        let (pp, md) = docx_tbox::split_frame(&f.inner);
        let ctx = Ctx { tokens: tokens.len(), num_base, doc_pr_start: *doc_pr, ..Default::default() };
        let bw = body_from_markdown(&md, resolve, ctx);
        let (inner, _) = resolve_markers(&[(false, bw.body)], tokens, &mut std::collections::HashSet::new());
        let inner = if inner.is_empty() { "<w:p/>".to_owned() } else { inner };
        let lines = md.lines().filter(|l| !l.trim().is_empty()).count().max(1);
        tokens.push(Opaque { xml: docx_tbox::new_box_run(&inner, &pp, &f.head, *doc_pr, text_w_pt, lines), fld: 0 });
        *doc_pr += 1;
    }
}

fn write_patched(src: &Source, defaults: DocDefaults, markdown: &str, (boxes, new_boxes): (&[docx_tbox::Found], &[docx_tbox::Found]), title: &str, resolve: &dyn Fn(&str) -> Option<Image>, rv: &Review) -> Result<Written, String> {
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
    let mut doc_pr_start = src
        .elems
        .iter()
        .map(|e| e.xml.as_str())
        .chain(src.tokens.iter().map(|t| t.xml.as_str()))
        .filter_map(|x| max_id(x, "docPr id=\""))
        .max()
        .map_or(1, |m| m + 1);
    let mut taken: std::collections::HashSet<String> = src.parts.iter().map(|(n, _)| n.trim_start_matches('/').to_lowercase()).collect();
    let secs = sections::scan(markdown, &defaults);
    let mut tokens = src.tokens.clone();
    add_new_boxes(new_boxes, &mut tokens, resolve, num_base, &mut doc_pr_start, (text_width_emu(&defaults) / 12700) as f32);
    let mut ctx = Ctx {
        tokens: tokens.len(),
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
    // Ändrade textrutor: originalkörningen med nytt `w:txbxContent`.
    let mut swaps: Vec<(String, String)> = Vec::new();
    for f in boxes {
        let Some((marker, orig, orig_head)) = src.tboxes.get(&f.token) else { continue };
        let orig_xml = &src.tokens[f.token].xml;
        if swaps.iter().any(|(o, _)| o == orig_xml) {
            continue;
        }
        let mut xml = orig_xml.clone();
        // Ny storlek eller nytt läge (dragen i dokumentläget).
        if f.head != *orig_head {
            xml = docx_tbox::set_geometry(&xml, orig_head, &f.head);
        }
        let md = strip_frame(&f.inner, marker);
        if md != *orig && !md.is_empty() && !md.contains("![") && !md.contains("<img") && !md.contains("[^") {
            let bctx = Ctx { tokens: tokens.len(), num_base, doc_pr_start, ..Default::default() };
            let bw = body_from_markdown(&md, resolve, bctx);
            let (inner, _) = resolve_markers(&[(false, bw.body)], &tokens, &mut std::collections::HashSet::new());
            let inner = if inner.is_empty() { "<w:p/>".to_owned() } else { inner };
            if let Some(new) = docx_tbox::replace_content(&xml, &inner) {
                xml = new;
            }
        }
        if xml != *orig_xml {
            swaps.push((orig_xml.clone(), xml.clone()));
            tokens[f.token].xml = xml;
        }
    }
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
    for (old, new) in &swaps {
        for (verbatim, text) in segs.iter_mut() {
            if *verbatim && text.contains(old.as_str()) {
                *text = text.replacen(old.as_str(), new, 1);
            }
        }
    }
    let (mut body, open_fields) = resolve_markers(&segs, &tokens, &mut used);
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
    // Har dokumentet egna rubrikmallar under andra id (Words `Rubrik1`) får de nya rubrikerna dem, inte Kladds.
    let heading_remap: Vec<(String, String)> = styles_name
        .as_deref()
        .and_then(|n| src.part(n).map(decode_xml))
        .filter(|x| x.contains("</w:styles>"))
        .map(|x| crate::docx_styles::foreign_heading_ids(&x).into_iter().map(|(n, id)| (format!("Heading{n}"), id)).collect())
        .unwrap_or_default();
    let body = heading_remap.iter().fold(body, |b, (k, v)| b.replace(&format!("<w:pStyle w:val=\"{k}\"/>"), &format!("<w:pStyle w:val=\"{v}\"/>")));
    let (body, footnotes_xml, endnotes_xml) = resolve_footnotes(body, &w, &bound, src.footnotes.as_ref(), src.endnotes.as_ref());
    let comments_out = review_xml::build_comments(src.comments.as_ref(), &rv.comments, &review_xml::anchored_ids(&body));

    // ---- sida, sidhuvud och sidfot: per avsnitt, bara det som ändrats skrivs om
    let page_changed = !same_page(&defaults, &src.page);
    let mut extras = Extras::default();
    let mut ids = w.ids.clone();
    let fields = style::Fields::for_export(title, "docx");
    let n_secs = secs.len();
    let dist_changed = {
        let diff = |a: Option<f32>, b: Option<f32>| (a.unwrap_or(crate::hf::DEFAULT_DIST) - b.unwrap_or(crate::hf::DEFAULT_DIST)).abs() > 0.6;
        diff(defaults.header_dist, src.page.header_dist) || diff(defaults.footer_dist, src.page.footer_dist)
    };
    let mut edits: Vec<SectEdit> = Vec::with_capacity(n_secs);
    for i in 0..n_secs {
        let (new, old) = if i == 0 {
            (hf::own_of_defaults(&defaults), hf::own_of_defaults(&src.page))
        } else {
            (secs[i].props.hf.clone(), if src.hf_secs.len() == n_secs { src.hf_secs[i].clone() } else { crate::hf::SecHf::default() })
        };
        let aligns = if i == 0 { Some(((defaults.header_align, defaults.footer_align), (src.page.header_align, src.page.footer_align))) } else { None };
        let mut e = section_hf_edit(aligns, &new, &old, &defaults, &mut extras, &fields, &mut ids, &mut taken, resolve);
        if i == 0 && dist_changed {
            e.dist = Some((defaults.header_dist, defaults.footer_dist));
        }
        edits.push(e);
    }
    let last_edit = edits.pop().unwrap_or_default();
    let sect = match &src.sect_pr {
        None => {
            let refs = HfRefs::make(&mut extras, &defaults, &fields, &mut ids, &mut taken, resolve);
            sect_pr_new(&defaults, &refs)
        }
        Some(orig) => {
            if !page_changed && last_edit.is_empty() { orig.clone() } else { patch_sect(orig, &defaults, page_changed, &last_edit) }
        }
    };
    let body = fix_sections(&body, &secs, "", false, &edits, &src.sect_prs);
    let sect = secs.last().map_or(sect.clone(), |s| apply_section(&sect, s, None));
    let document = format!("{}{body}{sect}{}", fix_head(&src.head), src.tail);
    let document = crate::docx_notes::patch_sects(&document, &src.page.notes, &defaults.notes);

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
                // Har dokumentet redan en rubrikmall (t.ex. `Rubrik1`) används den i stället för Kladds.
                .filter(|(id, _)| !heading_remap.iter().any(|(k, _)| k == id))
                .map(|(id, x)| (id, heading_remap.iter().fold(x, |x, (k, v)| x.replace(&format!("w:val=\"{k}\""), &format!("w:val=\"{v}\"")))))
                // Titel och Underrubrik läggs bara till när de används och dokumentet saknar en mall med namnet.
                .filter(|(id, _)| match id.as_str() {
                    "Title" | "Subtitle" => {
                        w.body.contains(&format!("<w:pStyle w:val=\"{id}\"/>")) && !xml.to_lowercase().contains(&format!("<w:name w:val=\"{}\"/>", id.to_lowercase()))
                    }
                    _ => true,
                })
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

    // Udda/jämna sidhuvuden slås på eller av i inställningarna.
    let even_on = |d: &DocDefaults| d.header_even.is_some() || d.footer_even.is_some();
    if even_on(&defaults) != even_on(&src.page) {
        if let Some(name) = find_rel("/settings") {
            if let Some(xml) = src.part(&name).map(decode_xml) {
                replaced.insert(name.to_lowercase(), fix_decl(&hf::with_even_odd(&xml, even_on(&defaults))).into_bytes());
            }
        }
    }
    // En ny innehållsförteckning utan utlagda sidnummer: be Word uppdatera fälten vid öppning.
    if body.contains(TOC_STALE_TAG) {
        if let Some(name) = find_rel("/settings") {
            let cur = replaced.get(&name.to_lowercase()).map(|b| decode_xml(b));
            if let Some(xml) = cur.or_else(|| src.part(&name).map(decode_xml)) {
                replaced.insert(name.to_lowercase(), fix_decl(&with_update_fields(&xml)).into_bytes());
            }
        }
    }

    // ---- slutnoter
    if let Some(xml) = endnotes_xml {
        match &src.endnotes {
            Some(o) => {
                replaced.insert(o.name.trim_start_matches('/').to_lowercase(), fix_decl(&xml).into_bytes());
            }
            None => {
                let (part, xml) = fresh_part("word/endnotes.xml", "endnotes", ENDNOTES_MIME, xml, &mut doc_rel_add, &mut type_add);
                new_parts.push((part, xml.into_bytes()));
                if let Some((name, x)) = find_rel("/settings").and_then(|n| src.part(&n).map(|b| (n, decode_xml(b)))) {
                    let x = replaced.get(&name.to_lowercase()).map_or(x, |b| decode_xml(b));
                    if x.contains("</w:settings>") && !x.contains("<w:endnotePr") {
                        let add = r#"<w:endnotePr><w:endnote w:id="-1"/><w:endnote w:id="0"/></w:endnotePr>"#;
                        let at = ["<w:compat", "<w:docVars", "<w:rsids", "<m:mathPr", "<w:themeFontLang", "<w:clrSchemeMapping", "</w:settings>"]
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

    // ---- fot- och slutnoternas talformat (sektionernas `w:sectPr` är redan ändrade ovan)
    if defaults.notes != src.page.notes {
        if let Some(name) = find_rel("/settings") {
            let cur = replaced.get(&name.to_lowercase()).map(|b| decode_xml(b));
            if let Some(xml) = cur.or_else(|| src.part(&name).map(decode_xml)) {
                replaced.insert(name.to_lowercase(), fix_decl(&crate::docx_notes::patch_settings(&xml, &src.page.notes, &defaults.notes)).into_bytes());
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
    let mut exts = media_exts(&w);
    exts.extend(extras.exts.iter().map(String::as_str));
    exts.sort_unstable();
    exts.dedup();
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
        let w = write(md, "x", &move |src| (src == "sol.png").then(|| Image { bytes: p2.clone(), extent: None, svg: None }), None).unwrap();
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

    /// Läser `word/document.xml` med handgjorda stycken före ett avslutande stycke "Slut".
    fn md_with_body(body: &str) -> String {
        let base = write("Slut\n", "T", &|_| None, None).unwrap().bytes;
        let xml = patch_document(&base, |x| x.replacen("<w:body>", &format!("<w:body>{body}"), 1));
        read(&xml).unwrap().markdown
    }

    const BRK: &str = "<!-- kladd: sidbrytning -->";

    #[test]
    fn page_break_in_run_splits_paragraph() {
        let md = md_with_body(r#"<w:p><w:r><w:t>Före</w:t></w:r><w:r><w:br w:type="page"/></w:r><w:r><w:t>Efter</w:t></w:r></w:p>"#);
        assert!(md.contains(&format!("Före\n\n{BRK}\n\nEfter")), "{md}");
    }

    #[test]
    fn page_break_alone_in_paragraph() {
        let md = md_with_body(r#"<w:p><w:r><w:t>A</w:t></w:r></w:p><w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        assert!(md.contains(&format!("A\n\n{BRK}\n\nSlut")), "{md}");
    }

    #[test]
    fn page_break_before_direct_and_off() {
        let md = md_with_body(r#"<w:p><w:r><w:t>A</w:t></w:r></w:p><w:p><w:pPr><w:pageBreakBefore/></w:pPr><w:r><w:t>B</w:t></w:r></w:p>"#);
        assert!(md.contains(&format!("A\n\n{BRK}\n\nB")), "{md}");
        let md = md_with_body(r#"<w:p><w:r><w:t>A</w:t></w:r></w:p><w:p><w:pPr><w:pageBreakBefore w:val="0"/></w:pPr><w:r><w:t>B</w:t></w:r></w:p>"#);
        assert!(!md.contains(BRK), "{md}");
    }

    #[test]
    fn page_break_before_from_style() {
        let base = write("Slut\n", "T", &|_| None, None).unwrap().bytes;
        let base = patch_styles(&base, |x| x.replacen("</w:styles>", r#"<w:style w:type="paragraph" w:styleId="Nysida"><w:name w:val="Nysida"/><w:pPr><w:pageBreakBefore/></w:pPr></w:style></w:styles>"#, 1));
        let xml = patch_document(&base, |x| x.replacen("<w:body>", r#"<w:body><w:p><w:r><w:t>A</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Nysida"/></w:pPr><w:r><w:t>B</w:t></w:r></w:p>"#, 1));
        let md = read(&xml).unwrap().markdown;
        assert!(md.contains(&format!("A\n\n{BRK}\n\nB")), "{md}");
    }

    #[test]
    fn page_break_roundtrips() {
        let md = format!("Före\n\n{BRK}\n\nEfter\n");
        assert_eq!(rt(&md), md);
        let again = rt(&rt(&md));
        assert_eq!(again, md);
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
        let expected = "<!-- kladd: font-family: Cambria; font-size: 12pt; h1: \"Georgia\" 20pt nobold #2F5496 -->

# Titel

# <span style=\"color: #FF0000\">Röd</span>

vanlig <span style=\"font-size: 14pt; color: #C00000; background-color: #FFFF00\">stor röd</span> auto <span style=\"background-color: #123456\">skugga</span> <span style=\"color: #FF0000\">mall</span> <span style=\"font-family: Arial\">arial</span>

<div align=\"center\">

Mitt<!--k:pp shd=\"EEEEEE\"-->

<span style=\"font-family: Verdana; color: #0000FF\">Blå mitt</span><!--k:pp shd=\"EEEEEE\"-->

</div>

Vänster<!--k:pp shd=\"EEEEEE\"-->

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
        // Textrutans text visas som ett block före stycket den sitter i.
        assert!(l.markdown.contains("<!--k:tb ") && l.markdown.contains("Inuti rutan"), "{}", l.markdown);
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

    const MERMAID_MD: &str = "# Rapport\n\nFöre.\n\n```mermaid\ngraph TD\n    A[Start] --> B{Klart?}\n    B -->|Ja| C[Slut]\n```\n\nEfter.\n\n```rust\nfn main() {}\n```\n\n```mermaid\nsom inte går att rita\n```\n";

    /// Alla XML-delar i paketet är välformade och varje relation och innehållstyp har sin del.
    fn assert_package_valid(bytes: &[u8]) {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_owned()).collect();
        let mut rels = String::new();
        for n in &names {
            if n.ends_with(".xml") || n.ends_with(".rels") {
                let mut x = String::new();
                std::io::Read::read_to_string(&mut z.by_name(n).unwrap(), &mut x).unwrap();
                let mut r = Reader::from_str(&x);
                loop {
                    match r.read_event() {
                        Ok(Event::Eof) => break,
                        Err(e) => panic!("{n} är inte välformad: {e}"),
                        _ => {}
                    }
                }
                if n == "word/_rels/document.xml.rels" {
                    rels = x;
                }
            }
        }
        // Varje intern relation pekar på en del som finns.
        for t in rels.split("Target=\"").skip(1) {
            let t = &t[..t.find('"').unwrap()];
            if !t.contains("://") {
                assert!(names.iter().any(|n| n == &format!("word/{t}")), "relationen {t} saknar del");
            }
        }
        let ct = {
            let mut x = String::new();
            std::io::Read::read_to_string(&mut z.by_name("[Content_Types].xml").unwrap(), &mut x).unwrap();
            x
        };
        for n in names.iter().filter(|n| n.starts_with("word/media/")) {
            let ext = n.rsplit('.').next().unwrap();
            assert!(ct.contains(&format!("Extension=\"{ext}\"")), "innehållstyp för .{ext} saknas");
        }
    }

    #[test]
    fn mermaid_block_is_written_as_svg_picture_and_reads_back() {
        let out = write(MERMAID_MD, "T", &|_| None, None).unwrap().bytes;
        assert_package_valid(&out);
        let doc = part_text(&out, "word/document.xml");
        // Diagrammet är en bild med SVG-del och PNG-reserv; koden står i alt-texten (radbrytningar som teckenreferenser).
        assert_eq!(doc.matches("asvg:svgBlip").count(), 1, "{doc}");
        assert!(doc.contains("descr=\"kladd-mermaid&#10;graph TD&#10;"), "{doc}");
        assert!(!doc.contains(">graph TD"), "koden ska inte stå som text");
        // Det som inte går att rita är kvar som kod.
        assert!(doc.contains("som inte går att rita") && doc.contains(">main<"));
        let mut z = zip::ZipArchive::new(Cursor::new(&out)).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_owned()).collect();
        assert!(names.iter().any(|n| n.starts_with("word/media/") && n.ends_with(".svg")));
        assert!(names.iter().any(|n| n.starts_with("word/media/") && n.ends_with(".png")));
        // Tur och retur: blocket kommer tillbaka som ```mermaid, inte som bild.
        let l = read(&out).unwrap();
        assert_eq!(l.markdown, MERMAID_MD, "\n--- fick ---\n{}", l.markdown);
        assert!(l.images.is_empty(), "diagrammet ska inte bli en bildfil i dokumentet");
        // Orört dokument: samma XML vid ny sparning; ändrad text runt omkring behåller diagrammet.
        let again = write(&l.markdown, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
        assert_eq!(part_text(&again, "word/document.xml"), doc);
        let md2 = l.markdown.replace("Efter.", "Efteråt.");
        let edited = write(&md2, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
        assert_package_valid(&edited);
        assert_eq!(read(&edited).unwrap().markdown, md2);
        assert_eq!(part_text(&edited, "word/document.xml").matches("asvg:svgBlip").count(), 1);
    }

    #[test]
    fn mermaid_picture_is_limited_to_the_text_width() {
        let wide = format!("```mermaid\ngraph LR\n{}\n```\n", (0..14).map(|i| format!("    N{i}[Ett ganska långt namn nummer {i}] --> N{}[Nästa steg {}]", i + 1, i + 1)).collect::<Vec<_>>().join("\n"));
        let out = write(&wide, "T", &|_| None, None).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        let cx: i64 = doc.split("<wp:extent cx=\"").nth(1).unwrap().split('"').next().unwrap().parse().unwrap();
        assert!(cx > 0 && cx <= MAX_IMAGE_WIDTH_EMU, "{cx}");
    }

    #[test]
    #[ignore]
    fn libreoffice_renders_mermaid() {
        let dir = std::env::var_os("KLADD_TEST_OUT").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("kladd-test-docx-mermaid"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mermaid.docx"), write(MERMAID_MD, "T", &|_| None, None).unwrap().bytes).unwrap();
        std::fs::write(dir.join("mermaid.odt"), crate::odt::write(MERMAID_MD, "T", &|_| None).unwrap().bytes).unwrap();
        for f in ["mermaid.docx", "mermaid.odt"] {
            let st = std::process::Command::new("soffice").args(["--headless", "--convert-to", "pdf", "--outdir"]).arg(&dir).arg(dir.join(f)).output().expect("soffice saknas");
            assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
        }
        assert!(dir.join("mermaid.pdf").exists());
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
        // Sidnumren kommer ur Kladds utläggning, så Word behöver inte be om en uppdatering vid öppning.
        assert!(!part_text(&out, "word/settings.xml").contains("updateFields"));
        let styles = part_text(&out, "word/styles.xml");
        assert!((1..=9).all(|n| styles.contains(&format!("w:styleId=\"TOC{n}\""))), "TOC1–TOC9");
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

    /// Sidnumren i fältresultatet, i ordning.
    fn toc_page_numbers(doc: &str) -> Vec<String> {
        let sdt = &doc[doc.find("<w:sdt>").unwrap()..doc.find("</w:sdt>").unwrap()];
        sdt.split("<w:tab/></w:r><w:r><w:t>").skip(1).map(|r| r[..r.find('<').unwrap()].to_owned()).collect()
    }

    #[test]
    fn toc_page_numbers_follow_the_layout_and_page_breaks() {
        let pb = crate::word::sections::PAGE_BREAK_LINE;
        let md = format!("# Ett\n\n<!-- kladd: toc -->\n\nText.\n\n{pb}\n\n# Två\n\n## Två b\n\n{pb}\n\nMellan.\n\n{pb}\n\n# Fyra\n");
        let out = write(&md, "T", &|_| None, None).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert_eq!(toc_page_numbers(&doc), ["1", "2", "2", "4"], "{doc}");
        assert!(!part_text(&out, "word/settings.xml").contains("updateFields"));
        // Ett långt dokument: rubrikerna längre fram hamnar på senare sidor (inte alla på sida 1).
        let mut long = String::from("<!-- kladd: toc -->\n\n");
        for i in 1..=6 {
            long.push_str(&format!("# Kapitel {i}\n\n"));
            for _ in 0..16 {
                long.push_str("Brödtext som fyller ut sidan så att kapitlen hamnar på olika sidor i dokumentet. Och lite till så att raden bryts.\n\n");
            }
        }
        let doc = part_text(&write(&long, "T", &|_| None, None).unwrap().bytes, "word/document.xml");
        let pages: Vec<usize> = toc_page_numbers(&doc).iter().map(|p| p.parse().unwrap()).collect();
        assert_eq!(pages.len(), 6);
        assert!(pages.windows(2).all(|w| w[0] <= w[1]) && pages[5] > pages[0] + 1, "{pages:?}");
    }

    #[test]
    fn toc_levels_roundtrip_and_foreign_levels_are_read() {
        let md = "# A\n\n<!-- kladd: toc; levels: 2-3 -->\n\n## B\n\n### C\n\n#### D\n";
        let out = write(md, "T", &|_| None, None).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains(r#"TOC \o "2-3" \h \z \u"#), "{doc}");
        // H1 ligger utanför nivåerna, H4 också.
        let sdt = &doc[doc.find("<w:sdt>").unwrap()..doc.find("</w:sdt>").unwrap()];
        assert!(sdt.contains(">B</w:t>") && sdt.contains(">C</w:t>") && !sdt.contains(">A</w:t>") && !sdt.contains(">D</w:t>"));
        assert!(sdt.contains("w:val=\"TOC2\"") && sdt.contains("w:val=\"TOC3\""));
        assert_eq!(read(&out).unwrap().markdown, md);
        // En främmande förteckning med `\o "1-5"` och `\o "2"`.
        let foreign = |levels: &str| {
            let patched = patch_part(&out, "word/document.xml", |x| x.replace(r#"\o "2-3""#, &format!(r#"\o "{levels}""#)));
            read(&patched).unwrap().markdown
        };
        assert!(foreign("1-5").contains("<!-- kladd: toc; levels: 1-5 -->"));
        assert!(foreign("2").contains("<!-- kladd: toc; levels: 2 -->"));
        assert!(foreign("1-3").contains("<!-- kladd: toc -->"));
    }

    #[test]
    fn changed_headings_rewrite_the_toc_and_untouched_keeps_it() {
        let out = write(TOC_MD, "T", &|_| None, None).unwrap().bytes;
        let marked = patch_part(&out, "word/document.xml", |x| x.replacen("<w:sdt>", "<w:sdt w:rsidR=\"00AB\">", 1));
        let l = read(&marked).unwrap();
        // Orörd: förteckningen behåller sitt XML.
        let same = write(&l.markdown, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
        assert_eq!(part_text(&same, "word/document.xml").matches("w:rsidR=\"00AB\"").count(), 1);
        // En rubrik ändras: förteckningen skrivs om med den nya texten.
        let md2 = l.markdown.replace("Bakgrund", "Förutsättningar");
        let edited = write(&md2, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
        let doc = part_text(&edited, "word/document.xml");
        assert_eq!(doc.matches("w:rsidR=\"00AB\"").count(), 0, "den gamla förteckningen ska bort");
        assert_eq!(doc.matches(">Förutsättningar</w:t>").count(), 2, "rubrik och förteckningspost");
        assert_eq!(read(&edited).unwrap().markdown, md2);
    }

    #[test]
    fn figure_list_is_written_from_captions_and_reads_back() {
        let md = "# Rapport\n\n<!-- kladd: toc; kind: figures -->\n\n<!-- kladd: toc; kind: tables -->\n\nFigur 1: Första bilden\n\nTabell 1: Siffror\n\n**Figur 2:** Andra bilden\n";
        let out = write(md, "T", &|_| None, None).unwrap().bytes;
        let doc = part_text(&out, "word/document.xml");
        assert!(doc.contains(r#"TOC \h \z \c "Figur""#) && doc.contains(r#"TOC \h \z \c "Tabell""#), "{doc}");
        assert_eq!(doc.matches(">Figurförteckning</w:t>").count(), 1);
        assert_eq!(doc.matches(">Figur 1: Första bilden</w:t>").count(), 2, "bildtext och förteckningspost");
        assert_eq!(doc.matches(">Figur 2: Andra bilden</w:t>").count(), 1, "bara förteckningsposten: bildtexten är fet och delad i körningar");
        assert_eq!(read(&out).unwrap().markdown, md);
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
        let resolve = move |src: &str| (src == "bild:a.png").then(|| Image { bytes: p.clone(), extent: None, svg: None });
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
            "<img src=\"bild:{name}\" alt=\"Sol &amp; måne\" width=\"240\">\n\nText med <img src=\"bild:{name}\" alt=\"\" width=\"100\" height=\"50\"> inne.\n\n<img src=\"bild:{name}\" alt=\"Ren\" width=\"1\">\n"
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
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None, svg: None });
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
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None, svg: None });
        let md = "<img src=\"bild:a.png\" alt=\"\" width=\"200\" align=\"left\">\n";
        let w = write(md, "x", &resolve, None).unwrap().bytes;
        // Byter `from` mot `to` i document.xml och ger bildtaggens attribut vid inläsning.
        let read_with = |from: &str, to: &str| {
            let b = patch_document(&w, |x| x.replace(from, to));
            let m = read(&b).unwrap().markdown;
            let t = m.split("<img ").nth(1).and_then(|t| t.split('>').next()).unwrap_or("").to_owned();
            t.split("alt=\"\" ").nth(1).unwrap_or("").to_owned()
        };
        let al = |a: &str| read_with("<wp:align>left</wp:align>", &format!("<wp:align>{a}</wp:align>"));
        assert_eq!(al("left"), "width=\"200\" align=\"left\"");
        // (Avståndet 12 px ligger på höger sida i den skrivna bilden, så detta är inte "align".)
        assert_eq!(al("right"), "width=\"200\" x=\"right\" y=\"0\" dist=\"0 12 0 0\"");
        assert_eq!(al("center"), "width=\"200\" x=\"center\" y=\"0\" dist=\"0 12 0 0\"");
        // wrapTight och wrapThrough flyter också; wrapTopAndBottom och wrapNone får egna lägen.
        let sq = r#"<wp:wrapSquare wrapText="bothSides"/>"#;
        let poly = r#"<wp:wrapPolygon edited="0"><wp:start x="0" y="0"/></wp:wrapPolygon>"#;
        assert_eq!(read_with(sq, &format!(r#"<wp:wrapTight wrapText="bothSides">{poly}</wp:wrapTight>"#)), "width=\"200\" align=\"left\"");
        assert_eq!(read_with(sq, &format!(r#"<wp:wrapThrough wrapText="bothSides">{poly}</wp:wrapThrough>"#)), "width=\"200\" align=\"left\"");
        assert_eq!(read_with(sq, "<wp:wrapTopAndBottom/>"), "width=\"200\" x=\"left\" y=\"0\" wrap=\"top\" dist=\"0 12 0 0\"");
        assert_eq!(read_with(sq, "<wp:wrapNone/>"), "width=\"200\" x=\"left\" y=\"0\" wrap=\"front\" dist=\"0 12 0 0\"");
        let behind = patch_document(&w, |x| x.replace(sq, "<wp:wrapNone/>").replace("behindDoc=\"0\"", "behindDoc=\"1\""));
        assert!(read(&behind).unwrap().markdown.contains("wrap=\"behind\""));
        // Förskjutning i stället för align, med referens.
        let h = r#"<wp:positionH relativeFrom="column"><wp:align>left</wp:align></wp:positionH>"#;
        let off = |rel: &str, o: i64| read_with(h, &format!(r#"<wp:positionH relativeFrom="{rel}"><wp:posOffset>{o}</wp:posOffset></wp:positionH>"#));
        assert_eq!(off("column", 0), "width=\"200\" x=\"0\" y=\"0\" dist=\"0 12 0 0\"");
        assert_eq!(off("column", 1_905_000), "width=\"200\" x=\"200\" y=\"0\" dist=\"0 12 0 0\"");
        assert_eq!(off("page", 720_000), "width=\"200\" hrel=\"page\" x=\"75.6\" y=\"0\" dist=\"0 12 0 0\"");
        // Lodrät placering mot sidan, avstånd och beskärning.
        let v = r#"<wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>"#;
        assert_eq!(
            read_with(v, r#"<wp:positionV relativeFrom="page"><wp:align>bottom</wp:align></wp:positionV>"#),
            "width=\"200\" x=\"left\" vrel=\"page\" y=\"bottom\" dist=\"0 12 0 0\""
        );
        assert_eq!(read_with("distT=\"0\" distB=\"0\"", "distT=\"95250\" distB=\"190500\""), "width=\"200\" x=\"left\" y=\"0\" dist=\"10 12 20 0\"");
        let cropped = read_with("<a:stretch>", "<a:srcRect l=\"10000\" t=\"0\" r=\"5500\" b=\"0\"/><a:stretch>");
        assert_eq!(cropped, "width=\"200\" height=\"200\" align=\"left\" crop=\"10 0 5.5 0\"");
        // Inline-bilder blir aldrig flytande.
        let inl = w_inline();
        assert!(!read(&inl).unwrap().markdown.contains("align="));
    }

    #[test]
    fn anchored_image_roundtrip() {
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None, svg: None });
        let md = concat!(
            "<img src=\"bild:a.png\" alt=\"A\" width=\"200\" hrel=\"page\" x=\"96\" vrel=\"page\" y=\"bottom\" wrap=\"behind\" dist=\"0 6 0 6\"> Text\n\n",
            "<img src=\"bild:a.png\" alt=\"\" width=\"100\" height=\"60\" x=\"center\" y=\"12\" wrap=\"top\" crop=\"10 0 5.5 0\">\n\n",
            "<img src=\"bild:a.png\" alt=\"\" width=\"50\" hrel=\"margin\" x=\"right\" vrel=\"margin\" y=\"20\" dist=\"3 4 5 6\">\n\nSlut\n"
        );
        let out = write(md, "x", &resolve, None).unwrap();
        let doc = part_text(&out.bytes, "word/document.xml");
        assert_eq!(doc.matches("<wp:anchor ").count(), 3, "{doc}");
        assert!(doc.contains(r#"behindDoc="1""#) && doc.contains("<wp:wrapNone/>") && doc.contains("<wp:wrapTopAndBottom/>"));
        assert!(doc.contains(r#"<wp:positionH relativeFrom="page"><wp:posOffset>914400</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:align>bottom</wp:align></wp:positionV>"#), "{doc}");
        assert!(doc.contains(r#"<wp:positionH relativeFrom="column"><wp:align>center</wp:align></wp:positionH>"#));
        assert!(doc.contains(r#"<a:srcRect l="10000" t="0" r="5500" b="0"/>"#));
        assert!(doc.contains(r#"distT="28575" distB="47625" distL="57150" distR="38100""#), "{doc}");
        // Tillbaka: samma dialekt.
        let back = read(&out.bytes).unwrap();
        let name = &back.images[0].0;
        assert_eq!(back.markdown, md.replace("bild:a.png", &format!("bild:{name}")), "{}", back.markdown);
        // Och igen med Source: oförändrat.
        let imgs = back.images.clone();
        let again = write(&back.markdown, "x", &move |s| imgs.iter().find(|(n, _)| Some(n.as_str()) == s.strip_prefix(IMAGE_SCHEME)).map(|(_, i)| i.clone()), back.source.as_ref()).unwrap();
        assert_eq!(read(&again.bytes).unwrap().markdown, back.markdown);
    }

    #[test]
    fn anchored_image_in_table_cell_is_inline() {
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None, svg: None });
        let w = write("<img src=\"bild:a.png\" alt=\"\" width=\"60\" align=\"left\">\n", "x", &resolve, None).unwrap().bytes;
        // Flytta in anchor-elementet i en tabellcell.
        let b = patch_document(&w, |x| {
            let i = x.find("<w:p>").unwrap();
            let j = x.find("</w:p>").unwrap() + 6;
            let p = x[i..j].to_owned();
            let tbl = format!("<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"4000\"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w=\"4000\" w:type=\"dxa\"/></w:tcPr>{p}</w:tc></w:tr></w:tbl><w:p/>");
            format!("{}{}{}", &x[..i], tbl, &x[j..])
        });
        let m = read(&b).unwrap().markdown;
        assert!(m.contains("<img") && !m.contains("align=") && !m.contains(" x="), "{m}");
    }

    #[test]
    fn vml_picture_position_is_read() {
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None, svg: None });
        let w = write("<img src=\"bild:a.png\" alt=\"\" width=\"60\" align=\"left\">\n", "x", &resolve, None).unwrap().bytes;
        let b = patch_document(&w, |x| {
            let id = x.split("r:embed=\"").nth(1).and_then(|r| r.split('"').next()).unwrap().to_owned();
            let i = x.find("<w:drawing>").unwrap();
            let j = x.find("</w:drawing>").unwrap() + "</w:drawing>".len();
            let pict = format!(
                "<w:pict><v:shape id=\"s1\" type=\"#_x0000_t75\" style=\"position:absolute;margin-left:72pt;margin-top:36pt;width:144pt;height:72pt;z-index:-1;mso-position-horizontal-relative:page;mso-position-vertical-relative:page\"><v:imagedata r:id=\"{id}\"/><w10:wrap type=\"square\"/></v:shape></w:pict>"
            );
            format!("{}{}{}", &x[..i], pict, &x[j..])
        });
        let m = read(&b).unwrap().markdown;
        assert!(m.contains("width=\"192\" height=\"96\" hrel=\"page\" x=\"96\" vrel=\"page\" y=\"48\">"), "{m}");
    }

    fn w_inline() -> Vec<u8> {
        let resolve = |src: &str| (src == "bild:a.png").then(|| Image { bytes: TEST_PNG.to_vec(), extent: None, svg: None });
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
        let w = write(&md, "x", &|_| Some(Image { bytes: png.clone(), extent: None, svg: None }), None).unwrap();
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
        // Tabellformatet (här: inga ramar, ingen automatisk fetstil) står i en dold rad före tabellen.
        let (meta, md) = md.split_once("\n\n").expect("formatrad");
        assert!(meta.starts_with("<!-- kladd-tabell:") && meta.contains("header off"), "{meta}");
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

    // ------------------------------------------------------------ tabellformat (Word-struktur, påhittad text)

    const W_BOX: &str = "<w:tbl><w:tblPr><w:tblW w:type=\"pct\" w:w=\"100%\"/><w:tblBorders><w:top w:val=\"single\" w:color=\"5B3E96\" w:sz=\"4\"/><w:left w:val=\"single\" w:color=\"5B3E96\" w:sz=\"4\"/><w:bottom w:val=\"single\" w:color=\"5B3E96\" w:sz=\"4\"/><w:right w:val=\"single\" w:color=\"5B3E96\" w:sz=\"4\"/><w:insideH w:val=\"none\"/><w:insideV w:val=\"none\"/></w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w=\"100\"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:type=\"pct\" w:w=\"100%\"/><w:shd w:fill=\"EFEAF7\" w:val=\"clear\"/><w:tcMar><w:top w:type=\"dxa\" w:w=\"150\"/><w:left w:type=\"dxa\" w:w=\"200\"/><w:bottom w:type=\"dxa\" w:w=\"150\"/><w:right w:type=\"dxa\" w:w=\"200\"/></w:tcMar></w:tcPr><w:p><w:r><w:rPr><w:b/><w:color w:val=\"5B3E96\"/></w:rPr><w:t>Rubrik i rutan</w:t></w:r></w:p><w:p><w:r><w:t>Första raden</w:t></w:r><w:r><w:br/></w:r><w:r><w:t>Efter radbrytning</w:t></w:r></w:p><w:p><w:r><w:t>Sista stycket</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";

    const W_DATA: &str = "<w:tbl><w:tblPr><w:tblW w:type=\"pct\" w:w=\"100%\"/><w:tblBorders><w:top w:val=\"single\" w:color=\"999999\" w:sz=\"4\"/><w:left w:val=\"single\" w:color=\"999999\" w:sz=\"4\"/><w:bottom w:val=\"single\" w:color=\"999999\" w:sz=\"4\"/><w:right w:val=\"single\" w:color=\"999999\" w:sz=\"4\"/><w:insideH w:val=\"single\" w:color=\"999999\" w:sz=\"4\"/><w:insideV w:val=\"single\" w:color=\"999999\" w:sz=\"4\"/></w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w=\"2200\"/><w:gridCol w:w=\"5700\"/></w:tblGrid>\
<w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"2200\"/><w:shd w:fill=\"2E5395\" w:val=\"clear\"/><w:tcMar><w:top w:type=\"dxa\" w:w=\"100\"/><w:left w:type=\"dxa\" w:w=\"100\"/><w:bottom w:type=\"dxa\" w:w=\"100\"/><w:right w:type=\"dxa\" w:w=\"100\"/></w:tcMar></w:tcPr><w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:rPr><w:b/><w:color w:val=\"FFFFFF\"/></w:rPr><w:t>Namn</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"5700\"/><w:shd w:fill=\"2E5395\" w:val=\"clear\"/><w:tcMar><w:top w:type=\"dxa\" w:w=\"100\"/><w:left w:type=\"dxa\" w:w=\"100\"/><w:bottom w:type=\"dxa\" w:w=\"100\"/><w:right w:type=\"dxa\" w:w=\"100\"/></w:tcMar></w:tcPr><w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:rPr><w:b/><w:color w:val=\"FFFFFF\"/></w:rPr><w:t>Beskrivning</w:t></w:r></w:p></w:tc></w:tr>\
<w:tr><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"2200\"/><w:tcMar><w:top w:type=\"dxa\" w:w=\"100\"/><w:left w:type=\"dxa\" w:w=\"100\"/><w:bottom w:type=\"dxa\" w:w=\"100\"/><w:right w:type=\"dxa\" w:w=\"100\"/></w:tcMar></w:tcPr><w:p><w:r><w:t>alfa</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:type=\"dxa\" w:w=\"5700\"/><w:tcMar><w:top w:type=\"dxa\" w:w=\"100\"/><w:left w:type=\"dxa\" w:w=\"100\"/><w:bottom w:type=\"dxa\" w:w=\"100\"/><w:right w:type=\"dxa\" w:w=\"100\"/></w:tcMar></w:tcPr><w:p><w:r><w:t>Första cellen med en lång text</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";

    fn with_tables(tables: &[&str]) -> (Vec<u8>, Loaded) {
        let w = write("x\n", "x", &|_| None, None).unwrap();
        let all: String = tables.iter().map(|t| format!("{t}<w:p><w:r><w:t>mellan</w:t></w:r></w:p>")).collect();
        let patched = patch_document(&w.bytes, |x| {
            let start = x.find("<w:p>").unwrap();
            let end = x.rfind("</w:p>").unwrap() + "</w:p>".len();
            format!("{}{all}{}", &x[..start], &x[end..])
        });
        let l = read(&patched).unwrap();
        (patched, l)
    }

    fn document_xml(bytes: &[u8]) -> String {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut s).unwrap();
        s
    }

    #[test]
    fn word_box_is_a_shaded_single_cell_table_with_line_breaks() {
        let (_, l) = with_tables(&[W_BOX]);
        let md = &l.markdown;
        let meta = md.lines().find(|l| l.starts_with("<!-- kladd-tabell:")).expect(md);
        let f = crate::word::tablefmt::TableFmt::from_comment(meta).unwrap();
        assert_eq!(f.fill(0, 0), Some([0xEF, 0xEA, 0xF7]), "{meta}");
        assert!(!f.header_bold, "{meta}");
        assert_eq!(f.width, crate::word::tablefmt::Width::Pct(100));
        assert_eq!(f.borders[0].color, Some([0x5B, 0x3E, 0x96]));
        assert!(!f.borders[4].visible() && !f.borders[5].visible());
        assert!((f.pad[0] - 10.0).abs() < 0.01 && (f.pad[2] - 7.5).abs() < 0.01, "{:?}", f.pad);
        // Stycken och radbrytning blir <br> i cellen; ingen fetstil på allt.
        assert!(md.contains("Första raden<br>Efter radbrytning<br>Sista stycket"), "{md}");
        assert!(md.contains("**Rubrik i rutan**"), "{md}");
        assert!(!md.contains("**Första"), "{md}");
    }

    #[test]
    fn word_table_header_shading_widths_and_alignment() {
        let (_, l) = with_tables(&[W_DATA]);
        let md = &l.markdown;
        let meta = md.lines().find(|l| l.starts_with("<!-- kladd-tabell:")).expect(md);
        let f = crate::word::tablefmt::TableFmt::from_comment(meta).unwrap();
        assert_eq!(f.fill(0, 0), Some([0x2E, 0x53, 0x95]));
        assert_eq!(f.fill(0, 1), Some([0x2E, 0x53, 0x95]));
        assert_eq!(f.fill(1, 0), None);
        assert_eq!(f.cell(0, 0).unwrap().halign, crate::word::tablefmt::TAlign::Center);
        assert_eq!(f.cells.get(&(1, 0)), None);
        // Rubrikraden är helt fet och vit: fet via Kladd, färgen som span.
        assert!(f.header_bold && md.contains("color: #FFFFFF"), "{md}");
        assert!(!md.contains("**Namn"), "{md}");
        // Kolumnbredderna 2200:5700 (11:28,5 streck) och tabellen i full bredd.
        let table = md.split("\n\n").find(|b| b.starts_with("| ")).unwrap();
        let w = markup::table_src(table).weights.unwrap();
        assert!((w[0] * 5).abs_diff(w[1] * 2) <= 6, "{w:?}");
        // Cellmarginaler 100 twips = 5 pt på alla sidor.
        assert!((f.pad[2] - 5.0).abs() < 0.01 && (f.pad[0] - 5.0).abs() < 0.01);
    }

    #[test]
    fn untouched_word_tables_are_written_back_byte_for_byte() {
        let (bytes, l) = with_tables(&[W_BOX, W_DATA]);
        let w = write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap();
        let (a, b) = (document_xml(&bytes), document_xml(&w.bytes));
        assert!(b.contains(W_BOX) && b.contains(W_DATA), "{b}");
        assert_eq!(a, b);
    }

    #[test]
    fn edited_word_table_keeps_shading_borders_and_widths() {
        let (_, l) = with_tables(&[W_DATA]);
        let md = l.markdown.replace("Första cellen", "Ändrad cell");
        let w = write(&md, "x", &|_| None, l.source.as_ref()).unwrap();
        let xml = document_xml(&w.bytes);
        // Skrivs om, men med skuggning, ramar, marginaler, justering och upprepad rubrikrad.
        assert!(xml.contains("Ändrad cell"));
        assert!(xml.contains("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"2E5395\"/>"), "{xml}");
        assert!(xml.contains("<w:tblBorders><w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>"), "{xml}");
        assert!(xml.contains("<w:tblCellMar>") && xml.contains("<w:tblHeader/>"));
        assert!(xml.contains("<w:jc w:val=\"center\"/>"));
        let again = read(&w.bytes).unwrap().markdown;
        let meta = |m: &str| m.lines().find(|l| l.starts_with("<!-- kladd-tabell:")).unwrap().to_owned();
        assert_eq!(meta(&again), meta(&l.markdown), "{again}");
        assert!(again.contains("Ändrad cell"));
    }

    #[test]
    fn table_without_format_stays_free_of_format_lines() {
        let md = "| a | b |\n| - | - |\n| c | d |\n";
        let w = write(md, "x", &|_| None, None).unwrap();
        let back = read(&w.bytes).unwrap().markdown;
        assert!(!back.contains("kladd-tabell"), "{back}");
        assert!(back.contains("| a | b |"));
    }

    #[test]
    fn format_line_survives_a_markdown_round_trip_of_a_new_table() {
        let mut f = crate::word::tablefmt::TableFmt::word_base();
        f.cell_mut(0, 0).fill = Some([1, 2, 3]);
        f.width = crate::word::tablefmt::Width::Pct(60);
        f.align = crate::word::tablefmt::TAlign::Center;
        let md = format!("{}\n\n| a | b |\n| - | - |\n| c | d |\n", f.to_comment());
        let w = write(&md, "x", &|_| None, None).unwrap();
        let xml = document_xml(&w.bytes);
        assert!(xml.contains("<w:tblW w:w=\"3000\" w:type=\"pct\"/>") && xml.contains("<w:jc w:val=\"center\"/>"), "{xml}");
        assert!(!xml.contains("kladd-tabell"));
        let back = read(&w.bytes).unwrap().markdown;
        let meta = back.lines().find(|l| l.starts_with("<!-- kladd-tabell:")).expect(&back);
        let g = crate::word::tablefmt::TableFmt::from_comment(meta).unwrap();
        assert_eq!(g.fill(0, 0), Some([1, 2, 3]));
        assert_eq!(g.width, crate::word::tablefmt::Width::Pct(60));
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
                std::fs::read(dir.join(src)).ok().map(|bytes| super::Image { bytes, extent: None, svg: None })
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

#[path = "docx_hf.rs"]
mod hf;

#[cfg(test)]
#[path = "docx_section_tests.rs"]
mod section_tests;

#[cfg(test)]
#[path = "docx_para_tests.rs"]
mod para_tests;
#[cfg(test)]
#[path = "docx_track2_tests.rs"]
mod track2_tests;
#[cfg(test)]
#[path = "docx_tbox_tests.rs"]
mod tbox_tests;
