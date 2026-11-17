//! Formatmallar i Words `styles.xml`: läsning av Normal, Rubrik 1-6 och Citat till Kladds
//! formatmallsändringar (`style::StyleSet`) och skrivning av ändrade formatmallar tillbaka
//! utan att röra resten av filen.
//!
//! Läsningen tar bara det en formatmall själv (och de den bygger på) anger, aldrig
//! docDefaults. Skrivningen ändrar bara de egenskaper som skiljer sig mellan `old` (det som
//! lästes) och `new`, så ett oförändrat dokument skrivs byte för byte som det var.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::style::{Align, Border, STYLE_IDS, StyleId, StyleOverride, StyleSet};

/// Temats typsnitt och färger för `w:asciiTheme` / `w:themeColor` (se `docx::Theme`).
pub struct Theme<'a> {
    /// `majorHAnsi`, `minorAscii` … → typsnittets namn.
    pub font: &'a dyn Fn(&str) -> Option<String>,
    /// `accent1`, `text2` … → temats färg (före tint/shade).
    pub color: &'a dyn Fn(&str) -> Option<[u8; 3]>,
}

impl Theme<'_> {
    /// Ett tema utan typsnitt och färger (Kladds egna mallar läses så).
    pub const NONE: Theme<'static> = Theme { font: &|_| None, color: &|_| None };
}

/// Wordtonen `w:themeTint` / `w:themeShade` (hex 00-FF) på en temafärg: tint ljusar mot vitt,
/// shade mörkar mot svart (samma formler som Word skriver ut sin färg med).
pub fn tint_shade(c: [u8; 3], tint: Option<&str>, shade: Option<&str>) -> [u8; 3] {
    let hex = |v: Option<&str>| v.and_then(|v| u8::from_str_radix(v, 16).ok()).map(|v| f32::from(v) / 255.0);
    let mut out = c.map(f32::from);
    if let Some(t) = hex(tint) {
        out = out.map(|v| 255.0 - (255.0 - v) * t);
    }
    if let Some(sh) = hex(shade) {
        out = out.map(|v| v * sh);
    }
    // Word avrundar nedåt.
    out.map(|v| (v + 0.0001).floor().clamp(0.0, 255.0) as u8)
}

/// Färgen i ett `w:color`-element: `w:val` om den är en färg, annars temafärgen.
pub fn color_of(e: &BytesStart<'_>, th: &Theme) -> Option<[u8; 3]> {
    let direct = attr(e, b"val").filter(|v| !v.eq_ignore_ascii_case("auto")).and_then(|v| crate::style::parse_color(&v));
    if direct.is_some() {
        return direct;
    }
    let base = attr(e, b"themeColor").and_then(|n| (th.color)(&n))?;
    Some(tint_shade(base, attr(e, b"themeTint").as_deref(), attr(e, b"themeShade").as_deref()))
}

/// Ett barn i en `w:rPr` (typsnitt, storlek, fet, kursiv, färg, teckenavstånd, versaler).
fn rpr_elem(o: &mut StyleOverride, name: &[u8], e: &BytesStart<'_>, th: &Theme) {
    match name {
        b"rFonts" => {
            let themed = attr(e, b"asciiTheme").or_else(|| attr(e, b"hAnsiTheme")).and_then(|t| (th.font)(&t));
            let named = || attr(e, b"ascii").or_else(|| attr(e, b"hAnsi")).filter(|f| !f.trim().is_empty());
            if let Some(f) = themed.or_else(named) {
                o.font_family = Some(f);
            }
        }
        b"sz" => o.size = attr(e, b"val").and_then(|v| v.parse::<f32>().ok()).filter(|h| *h > 0.0).map(|h| h / 2.0),
        b"b" => o.bold = Some(on_off(e)),
        b"i" => o.italic = Some(on_off(e)),
        b"color" => o.color = color_of(e, th),
        b"spacing" => o.letter_spacing = twips_signed(attr(e, b"val")),
        b"caps" => o.caps = Some(on_off(e)),
        b"smallCaps" => o.small_caps = Some(on_off(e)),
        _ => {}
    }
}

/// `w:pBdr/w:bottom` som en linje (tjocklek i åttondels punkt, avstånd i punkter). `nil`/`none` = ingen.
fn border_of(e: &BytesStart<'_>, th: &Theme) -> Option<Border> {
    if matches!(attr(e, b"val").as_deref(), None | Some("nil" | "none")) {
        return None;
    }
    let width = attr(e, b"sz").and_then(|v| v.parse::<f32>().ok()).map_or(0.5, |v| (v / 8.0).clamp(0.25, 12.0));
    let space = attr(e, b"space").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0).clamp(0.0, 31.0);
    // Automatisk färg är svart i Word; `w:color` är här färgen själv.
    let direct = attr(e, b"color").filter(|v| !v.eq_ignore_ascii_case("auto")).and_then(|v| crate::style::parse_color(&v));
    let themed = || {
        let base = attr(e, b"themeColor").and_then(|n| (th.color)(&n))?;
        Some(tint_shade(base, attr(e, b"themeTint").as_deref(), attr(e, b"themeShade").as_deref()))
    };
    Some(Border { width, color: direct.or_else(themed).unwrap_or([0, 0, 0]), space })
}

fn local(name: &[u8]) -> &[u8] {
    name.iter().rposition(|&b| b == b':').map_or(name, |i| &name[i + 1..])
}

fn attr(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| local(a.key.as_ref()) == name).map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn on_off(e: &BytesStart<'_>) -> bool {
    !matches!(attr(e, b"val").as_deref(), Some("0" | "false" | "off" | "none"))
}

/// Det en formatmall själv anger.
#[derive(Default, Clone, Debug)]
struct St {
    name: String,
    based_on: Option<String>,
    paragraph: bool,
    default: bool,
    props: StyleOverride,
}

impl St {
    /// `inner` ovanpå `self`.
    fn over(&mut self, inner: &StyleOverride) {
        let p = &mut self.props;
        p.font_family = inner.font_family.clone().or(p.font_family.take());
        p.size = inner.size.or(p.size);
        p.bold = inner.bold.or(p.bold);
        p.italic = inner.italic.or(p.italic);
        p.color = inner.color.or(p.color);
        p.align = inner.align.or(p.align);
        p.space_before = inner.space_before.or(p.space_before);
        p.space_after = inner.space_after.or(p.space_after);
        p.line_spacing = inner.line_spacing.or(p.line_spacing);
        p.first_indent = inner.first_indent.or(p.first_indent);
        p.letter_spacing = inner.letter_spacing.or(p.letter_spacing);
        p.caps = inner.caps.or(p.caps);
        p.small_caps = inner.small_caps.or(p.small_caps);
        p.border_bottom = inner.border_bottom.or(p.border_bottom);
    }
}

/// Som `twips` men tillåter negativa värden (sammandraget teckenavstånd).
fn twips_signed(v: Option<String>) -> Option<f32> {
    v.and_then(|s| s.parse::<f32>().ok()).map(|t| (t / 20.0 * 100.0).round() / 100.0)
}

fn twips(v: Option<String>) -> Option<f32> {
    v.and_then(|s| s.parse::<f32>().ok()).filter(|t| *t >= 0.0).map(|t| (t / 20.0 * 100.0).round() / 100.0)
}

/// Läser formatmallarna ur `styles.xml`. `theme` ger typsnittet för ett temanamn (`majorHAnsi`).
#[cfg(test)]
pub fn read(xml: &str, th: &Theme) -> StyleSet {
    parse(xml, th, None).0
}

/// Läser formatmallarna och docDefaults; `forced` ersätter docDefaults (för Kladds egna mallar,
/// som saknar egna men ska jämföras med dokumentets).
fn parse(xml: &str, th: &Theme, forced: Option<&StyleOverride>) -> (StyleSet, StyleOverride) {
    let mut map: HashMap<String, St> = HashMap::new();
    let mut default_para = String::new();
    let mut r = Reader::from_str(xml);
    let mut cur: Option<(String, St)> = None;
    let (mut in_ppr, mut in_rpr, mut in_defaults, mut in_drpr, mut in_pbdr) = (false, false, false, false, false);
    // docDefaults/rPrDefault: teckenegenskaper som gäller om en formatmall inte anger något.
    let mut defaults = StyleOverride::default();
    loop {
        let ev = r.read_event();
        let (e, empty) = match &ev {
            Ok(Event::Start(e)) => (e, false),
            Ok(Event::Empty(e)) => (e, true),
            Ok(Event::End(e)) => {
                match local(e.name().as_ref()) {
                    b"style" => {
                        if let Some((id, st)) = cur.take() {
                            if st.default && st.paragraph {
                                default_para = id.to_lowercase();
                            }
                            map.insert(id.to_lowercase(), st);
                        }
                    }
                    b"pPr" => in_ppr = false,
                    b"pBdr" => in_pbdr = false,
                    b"rPr" => {
                        in_rpr = false;
                        in_drpr = false;
                    }
                    b"docDefaults" => in_defaults = false,
                    _ => {}
                }
                continue;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => continue,
        };
        let qn = e.name();
        let name = local(qn.as_ref());
        if name == b"docDefaults" {
            in_defaults = !empty;
            continue;
        }
        if name == b"style" && !empty {
            let ty = attr(e, b"type");
            let st = St {
                paragraph: ty.as_deref().is_none_or(|t| t == "paragraph"),
                default: attr(e, b"default").is_some_and(|d| d == "1" || d == "true"),
                ..Default::default()
            };
            cur = Some((attr(e, b"styleId").unwrap_or_default(), st));
            continue;
        }
        if in_defaults && !empty && name == b"rPr" {
            in_drpr = true;
            continue;
        }
        if in_drpr {
            rpr_elem(&mut defaults, name, e, th);
            continue;
        }
        let Some((_, st)) = cur.as_mut().filter(|_| !in_defaults) else {
            if name == b"pPr" {
                in_ppr = !empty;
            } else if name == b"rPr" && !in_ppr {
                in_rpr = !empty;
            }
            continue;
        };
        match name {
            b"pPr" => in_ppr = !empty,
            b"pBdr" if in_ppr => in_pbdr = !empty,
            b"rPr" if !in_ppr => in_rpr = !empty,
            b"name" if !in_rpr && !in_ppr => st.name = attr(e, b"val").unwrap_or_default().to_lowercase(),
            b"basedOn" => st.based_on = attr(e, b"val").map(|v| v.to_lowercase()),
            _ if in_rpr => rpr_elem(&mut st.props, name, e, th),
            _ if in_ppr => match name {
                b"bottom" if in_pbdr => st.props.border_bottom = border_of(e, th),
                b"jc" => st.props.align = attr(e, b"val").map(|v| Align::from_jc(&v)),
                b"spacing" => {
                    st.props.space_before = twips(attr(e, b"before"));
                    st.props.space_after = twips(attr(e, b"after"));
                    // Bara automatiskt radavstånd (240 = enkelt) går att uttrycka.
                    let auto = attr(e, b"lineRule").is_none_or(|r| r == "auto");
                    st.props.line_spacing = attr(e, b"line")
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|l| auto && *l > 0.0)
                        .map(|l| (l / 240.0 * 100.0).round() / 100.0);
                }
                b"ind" => st.props.first_indent = twips(attr(e, b"firstLine")),
                _ => {}
            },
            _ => {}
        }
    }

    if let Some(f) = forced {
        defaults = f.clone();
    }
    let chain = |id: &str| -> St {
        let mut ids = Vec::new();
        let mut c = Some(id.to_owned());
        while let Some(i) = c {
            if ids.len() >= 10 {
                break;
            }
            let Some(st) = map.get(&i) else { break };
            ids.push(i);
            c = st.based_on.clone();
        }
        let mut acc = St::default();
        for i in ids.iter().rev() {
            acc.over(&map[i].props);
        }
        acc
    };
    let find = |keys: &[&str]| -> Option<String> {
        map.iter().find(|(id, st)| st.paragraph && (keys.contains(&id.as_str()) || keys.contains(&st.name.as_str()))).map(|(id, _)| id.clone())
    };
    let mut set = StyleSet::default();
    for id in STYLE_IDS {
        let sid = match id {
            StyleId::Body => Some(if default_para.is_empty() { "normal".to_owned() } else { default_para.clone() }),
            StyleId::Heading(n) => find(&[&format!("heading{n}"), &format!("heading {n}")]),
            StyleId::Quote => find(&["quote"]),
            StyleId::Title => find(&["title"]),
            StyleId::Subtitle => find(&["subtitle"]),
        };
        let Some(sid) = sid.filter(|s| map.contains_key(s)) else { continue };
        let mut o = chain(&sid).props;
        if id != StyleId::Body {
            // Words docDefaults gäller det formatmallen inte anger (typsnittet undantaget: Kladds
            // rubriker har sitt eget typsnitt).
            o = StyleOverride {
                size: o.size.or(defaults.size),
                // En mall som inte anger fet/kursiv/färg är vanlig svart text i Word (inte Kladds blå rubrik).
                bold: o.bold.or(defaults.bold).or(Some(false)),
                italic: o.italic.or(defaults.italic).or(Some(false)),
                color: o.color.or(defaults.color).or(Some([0, 0, 0])),
                letter_spacing: o.letter_spacing.or(defaults.letter_spacing),
                caps: o.caps.or(defaults.caps),
                small_caps: o.small_caps.or(defaults.small_caps),
                ..o
            };
        }
        if id == StyleId::Body {
            // Brödtextens typsnitt och storlek är dokumentets grundformat.
            o.font_family = None;
            o.size = None;
        }
        set.set(id, o);
    }
    (set, defaults)
}

/// Som `read`, men utan det som är lika med Kladds egna formatmallar (`kladd_styles`, inpackade i
/// `w:styles`). Ett dokument skrivet av Kladd, eller ett som aldrig ändrat rubrikerna, får då inga
/// formatmallsändringar.
pub fn read_relative(xml: &str, th: &Theme, kladd_xml: &str) -> StyleSet {
    let (doc, defaults) = parse(xml, th, None);
    doc.minus(&parse(kladd_xml, &Theme::NONE, Some(&defaults)).0)
}

// ------------------------------------------------------------------ skrivning

/// Ordningen på barnen i `w:pPr` och `w:rPr` (schemats), så att Word accepterar filen.
const PPR_ORDER: &[&str] = &[
    "pStyle", "keepNext", "keepLines", "pageBreakBefore", "framePr", "widowControl", "numPr", "suppressLineNumbers",
    "pBdr", "shd", "tabs", "suppressAutoHyphens", "kinsoku", "wordWrap", "overflowPunct", "topLinePunct", "autoSpaceDE",
    "autoSpaceDN", "bidi", "adjustRightInd", "snapToGrid", "spacing", "ind", "contextualSpacing", "mirrorIndents",
    "suppressOverlap", "jc", "textDirection", "textAlignment", "textboxTightWrap", "outlineLvl", "divId", "cnfStyle",
    "rPr", "sectPr", "pPrChange",
];
const RPR_ORDER: &[&str] = &[
    "rStyle", "rFonts", "b", "bCs", "i", "iCs", "caps", "smallCaps", "strike", "dstrike", "outline", "shadow", "emboss",
    "imprint", "noProof", "snapToGrid", "vanish", "webHidden", "color", "spacing", "w", "kern", "position", "sz", "szCs",
    "highlight", "u", "effect", "bdr", "shd", "fitText", "vertAlign", "rtl", "cs", "em", "lang", "eastAsianLayout",
    "specVanish", "oMath",
];

/// Slutet (exklusivt) på elementet som börjar vid `from` i `xml`.
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

fn elem_name(x: &str) -> &str {
    let t = x.trim_start_matches('<');
    let end = t.find(|c: char| c.is_whitespace() || c == '/' || c == '>').unwrap_or(t.len());
    t[..end].rsplit(':').next().unwrap_or("")
}

/// Barnen på översta nivån i `inner` som (lokalt namn, xml).
fn children(inner: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = inner[pos..].find('<') {
        let start = pos + i;
        let Some(end) = element_end(inner, start) else { break };
        out.push((elem_name(&inner[start..end]).to_owned(), inner[start..end].to_owned()));
        pos = end;
    }
    out
}

/// `key="value"`-par i ett elements starttagg.
fn attrs_of(x: &str) -> Vec<(String, String)> {
    let end = x.find('>').unwrap_or(x.len());
    let tag = &x[..end];
    let mut out = Vec::new();
    let mut rest = tag;
    while let Some(eq) = rest.find("=\"") {
        let key_start = rest[..eq].rfind(char::is_whitespace).map_or(0, |i| i + 1);
        let key = rest[key_start..eq].to_owned();
        let val_start = eq + 2;
        let Some(val_len) = rest[val_start..].find('"') else { break };
        out.push((key, rest[val_start..val_start + val_len].to_owned()));
        rest = &rest[val_start + val_len + 1..];
    }
    out
}

fn build(name: &str, attrs: &[(String, String)]) -> String {
    let mut s = format!("<w:{name}");
    for (k, v) in attrs {
        s.push_str(&format!(" {k}=\"{v}\""));
    }
    s.push_str("/>");
    s
}

fn w(k: &str) -> String {
    format!("w:{k}")
}

fn set_child(list: &mut Vec<(String, String)>, order: &[&str], name: &str, xml: Option<String>) {
    list.retain(|(n, _)| n != name);
    let Some(xml) = xml else { return };
    let idx = order.iter().position(|n| *n == name).unwrap_or(usize::MAX);
    let at = list
        .iter()
        .position(|(n, _)| order.iter().position(|o| o == n).is_some_and(|i| i > idx))
        .unwrap_or(list.len());
    list.insert(at, (name.to_owned(), xml));
}

/// Ändrar attributen i det befintliga barnet `name` (eller skapar det); ett barn utan attribut tas bort.
fn edit_attrs(list: &mut Vec<(String, String)>, order: &[&str], name: &str, f: impl FnOnce(&mut Vec<(String, String)>)) {
    let mut a = list.iter().find(|(n, _)| n == name).map(|(_, x)| attrs_of(x)).unwrap_or_default();
    f(&mut a);
    let xml = (!a.is_empty()).then(|| build(name, &a));
    set_child(list, order, name, xml);
}

fn put(a: &mut Vec<(String, String)>, k: &str, v: Option<String>) {
    a.retain(|(n, _)| n != k);
    if let Some(v) = v {
        a.push((k.to_owned(), v));
    }
}

fn tw(pt: f32) -> String {
    format!("{}", (pt * 20.0).round() as i64)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Skriver de egenskaper som skiljer `old` från `new` in i formatmallens XML (`style`).
fn patch_style(style: &str, old: &StyleOverride, new: &StyleOverride, body: bool) -> String {
    let Some(open_end) = style.find('>') else { return style.to_owned() };
    let Some(close) = style.rfind("</w:style>") else { return style.to_owned() };
    let head = &style[..=open_end];
    let inner = &style[open_end + 1..close];
    let mut top = children(inner);
    let take = |top: &mut Vec<(String, String)>, name: &str| -> Vec<(String, String)> {
        match top.iter().position(|(n, _)| n == name) {
            Some(i) => {
                let x = top[i].1.clone();
                top.remove(i);
                let o = x.find('>').map_or(0, |i| i + 1);
                if x[..o].ends_with("/>") { Vec::new() } else { children(&x[o..x.rfind("</").unwrap_or(x.len())]) }
            }
            None => Vec::new(),
        }
    };
    let mut ppr = take(&mut top, "pPr");
    let mut rpr = take(&mut top, "rPr");

    if old.font_family != new.font_family && !body {
        let x = new.font_family.as_deref().map(|f| {
            let f = escape(f);
            format!("<w:rFonts w:ascii=\"{f}\" w:eastAsia=\"{f}\" w:hAnsi=\"{f}\" w:cs=\"{f}\"/>")
        });
        set_child(&mut rpr, RPR_ORDER, "rFonts", x);
    }
    if old.size != new.size && !body {
        let h = new.size.map(|s| format!("{}", (s * 2.0).round() as i64));
        set_child(&mut rpr, RPR_ORDER, "sz", h.as_ref().map(|h| format!("<w:sz w:val=\"{h}\"/>")));
        set_child(&mut rpr, RPR_ORDER, "szCs", h.map(|h| format!("<w:szCs w:val=\"{h}\"/>")));
    }
    if old.bold != new.bold {
        let x = new.bold.map(|b| if b { "<w:b/>".to_owned() } else { "<w:b w:val=\"0\"/>".to_owned() });
        set_child(&mut rpr, RPR_ORDER, "b", x);
    }
    if old.italic != new.italic {
        let x = new.italic.map(|b| if b { "<w:i/>".to_owned() } else { "<w:i w:val=\"0\"/>".to_owned() });
        set_child(&mut rpr, RPR_ORDER, "i", x);
    }
    if old.color != new.color {
        let x = new.color.map(|c| format!("<w:color w:val=\"{}\"/>", crate::style::hex(c).trim_start_matches('#')));
        set_child(&mut rpr, RPR_ORDER, "color", x);
    }
    if old.letter_spacing != new.letter_spacing {
        let x = new.letter_spacing.map(|v| format!("<w:spacing w:val=\"{}\"/>", (v * 20.0).round() as i64));
        set_child(&mut rpr, RPR_ORDER, "spacing", x);
    }
    for (o, n, tag) in [(old.caps, new.caps, "caps"), (old.small_caps, new.small_caps, "smallCaps")] {
        if o != n {
            let x = n.map(|b| if b { format!("<w:{tag}/>") } else { format!("<w:{tag} w:val=\"0\"/>") });
            set_child(&mut rpr, RPR_ORDER, tag, x);
        }
    }
    if old.border_bottom != new.border_bottom {
        // `w:pBdr` kan ha andra kanter; bara `w:bottom` byts.
        let mut edges = ppr.iter().find(|(n, _)| n == "pBdr").map(|(_, x)| {
            let o = x.find('>').map_or(0, |i| i + 1);
            if x[..o].ends_with("/>") { Vec::new() } else { children(&x[o..x.rfind("</").unwrap_or(x.len())]) }
        }).unwrap_or_default();
        let b = new.border_bottom.map(|b| {
            format!(
                "<w:bottom w:val=\"single\" w:sz=\"{}\" w:space=\"{}\" w:color=\"{}\"/>",
                ((b.width * 8.0).round() as i64).max(2),
                b.space.round() as i64,
                crate::style::hex(b.color).trim_start_matches('#')
            )
        });
        set_child(&mut edges, &["top", "left", "bottom", "right", "between", "bar"], "bottom", b);
        let x = (!edges.is_empty()).then(|| format!("<w:pBdr>{}</w:pBdr>", edges.iter().map(|(_, x)| x.as_str()).collect::<String>()));
        set_child(&mut ppr, PPR_ORDER, "pBdr", x);
    }
    if old.align != new.align {
        let x = new.align.map(|a| format!("<w:jc w:val=\"{}\"/>", a.as_jc()));
        set_child(&mut ppr, PPR_ORDER, "jc", x);
    }
    if old.space_before != new.space_before || old.space_after != new.space_after || old.line_spacing != new.line_spacing {
        edit_attrs(&mut ppr, PPR_ORDER, "spacing", |a| {
            if old.space_before != new.space_before {
                put(a, &w("before"), new.space_before.map(tw));
            }
            if old.space_after != new.space_after {
                put(a, &w("after"), new.space_after.map(tw));
            }
            if old.line_spacing != new.line_spacing {
                put(a, &w("line"), new.line_spacing.map(|l| format!("{}", (l * 240.0).round() as i64)));
                put(a, &w("lineRule"), new.line_spacing.map(|_| "auto".to_owned()));
            }
        });
    }
    if old.first_indent != new.first_indent {
        edit_attrs(&mut ppr, PPR_ORDER, "ind", |a| {
            put(a, &w("firstLine"), new.first_indent.map(tw));
            if new.first_indent.is_some() {
                put(a, &w("hanging"), None);
                put(a, &w("firstLineChars"), None);
            }
        });
    }

    // Egenskapsblocken står efter formatmallens namn m.m. och före tabellegenskaperna.
    let join = |l: &[(String, String)]| l.iter().map(|(_, x)| x.as_str()).collect::<String>();
    let mut blocks = Vec::new();
    if !ppr.is_empty() {
        blocks.push(("pPr", format!("<w:pPr>{}</w:pPr>", join(&ppr))));
    }
    if !rpr.is_empty() {
        blocks.push(("rPr", format!("<w:rPr>{}</w:rPr>", join(&rpr))));
    }
    for (name, xml) in blocks {
        let after = ["pPr", "rPr", "tblPr", "trPr", "tcPr", "tblStylePr"];
        let idx = after.iter().position(|n| *n == name).unwrap_or(0);
        let at = top.iter().position(|(n, _)| after.iter().position(|a| a == n).is_some_and(|i| i > idx)).unwrap_or(top.len());
        top.insert(at, (name.to_owned(), xml));
    }
    format!("{head}{}</w:style>", join(&top))
}

/// Formatmallens XML-element i `xml` för `id`: (start, slut) som byteintervall.
fn locate(xml: &str, id: StyleId, default_para: bool) -> Option<(usize, usize)> {
    let mut at = 0;
    let keys: Vec<String> = match id {
        StyleId::Body => vec!["normal".into()],
        StyleId::Heading(n) => vec![format!("heading{n}"), format!("heading {n}")],
        StyleId::Quote => vec!["quote".into()],
        StyleId::Title => vec!["title".into()],
        StyleId::Subtitle => vec!["subtitle".into()],
    };
    let mut fallback = None;
    while let Some(i) = xml[at..].find("<w:style ") {
        let start = at + i;
        let end = xml[start..].find("</w:style>")? + start + "</w:style>".len();
        let el = &xml[start..end];
        let head = &el[..el.find('>').unwrap_or(0)];
        let sid = attrs_of(head).into_iter().find(|(k, _)| k.ends_with("styleId")).map(|(_, v)| v.to_lowercase());
        let is_para = attrs_of(head).into_iter().find(|(k, _)| k.ends_with(":type")).is_none_or(|(_, v)| v == "paragraph");
        let name = el.find("<w:name ").map(|n| attrs_of(&el[n..]).into_iter().find(|(k, _)| k.ends_with("val")).map(|(_, v)| v.to_lowercase())).unwrap_or(None);
        if is_para && (sid.as_ref().is_some_and(|s| keys.contains(s)) || name.as_ref().is_some_and(|n| keys.contains(n))) {
            return Some((start, end));
        }
        if default_para && is_para && head.contains("w:default=\"1\"") {
            fallback = Some((start, end));
        }
        at = end;
    }
    fallback
}

/// Skriver de formatmallar som skiljer `old` från `new` in i `styles.xml`. Allt annat lämnas orört.
pub fn patch_styles(xml: &str, old: &StyleSet, new: &StyleSet) -> String {
    let mut out = xml.to_owned();
    for id in STYLE_IDS {
        let (o, n) = (old.get(id), new.get(id));
        if o == n {
            continue;
        }
        let Some((s, e)) = locate(&out, id, id == StyleId::Body) else { continue };
        let patched = patch_style(&out[s..e], o, n, id == StyleId::Body);
        out.replace_range(s..e, &patched);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/><w:pPr><w:jc w:val="both"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="0"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi"/><w:color w:val="2F5496"/><w:sz w:val="32"/><w:szCs w:val="32"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="864" w:firstLine="360"/></w:pPr><w:rPr><w:i/><w:iCs/></w:rPr></w:style></w:styles>"#;

    fn theme_font(t: &str) -> Option<String> {
        (t == "majorHAnsi").then(|| "Calibri Light".to_owned())
    }

    fn theme_color(n: &str) -> Option<[u8; 3]> {
        match n {
            "accent1" => Some([0x4F, 0x81, 0xBD]),
            "text2" => Some([0x1F, 0x49, 0x7D]),
            _ => None,
        }
    }

    const THEME: Theme<'static> = Theme { font: &theme_font, color: &theme_color };

    #[test]
    fn reads_headings_normal_and_quote() {
        let s = read(STYLES, &THEME);
        let h1 = s.get(StyleId::Heading(1));
        assert_eq!(h1.font_family.as_deref(), Some("Calibri Light"));
        assert_eq!(h1.size, Some(16.0));
        assert_eq!(h1.color, Some([0x2F, 0x54, 0x96]));
        assert_eq!((h1.space_before, h1.space_after), (Some(12.0), Some(0.0)));
        // Heading1 bygger på Normal och ärver justeringen.
        assert_eq!(h1.align, Some(Align::Justify));
        assert_eq!(s.get(StyleId::Body).align, Some(Align::Justify));
        assert_eq!(s.get(StyleId::Quote).italic, Some(true));
        assert_eq!(s.get(StyleId::Quote).first_indent, Some(18.0));
        // docDefaults räknas inte med.
        assert_eq!(s.get(StyleId::Body).space_after, None);
        assert!(s.get(StyleId::Heading(2)).is_empty());
    }

    #[test]
    fn relative_read_drops_kladd_defaults() {
        let kladd = r#"<w:styles><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:spacing w:before="360" w:after="80"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/></w:rPr></w:style></w:styles>"#;
        let xml = kladd.replace("w:val=\"36\"", "w:val=\"40\"");
        let s = read_relative(&xml, &Theme::NONE, kladd);
        assert_eq!(s.get(StyleId::Heading(1)), &StyleOverride { size: Some(20.0), ..Default::default() });
        assert!(read_relative(kladd, &Theme::NONE, kladd).is_empty());
    }

    #[test]
    fn untouched_set_leaves_xml_alone() {
        let s = read(STYLES, &THEME);
        assert_eq!(patch_styles(STYLES, &s, &s), STYLES);
    }

    #[test]
    fn changed_properties_are_written_in_schema_order() {
        let old = read(STYLES, &THEME);
        let mut new = old.clone();
        let mut h1 = old.get(StyleId::Heading(1)).clone();
        h1.size = Some(20.0);
        h1.bold = Some(true);
        h1.space_after = Some(6.0);
        h1.line_spacing = Some(1.5);
        h1.font_family = Some("Georgia".into());
        new.set(StyleId::Heading(1), h1);
        let out = patch_styles(STYLES, &old, &new);
        let h = &out[out.find("w:styleId=\"Heading1\"").unwrap()..];
        let h = &h[..h.find("</w:style>").unwrap()];
        assert!(h.contains(r#"<w:rFonts w:ascii="Georgia" w:eastAsia="Georgia" w:hAnsi="Georgia" w:cs="Georgia"/><w:b/><w:color w:val="2F5496"/><w:sz w:val="40"/><w:szCs w:val="40"/>"#), "{h}");
        assert!(h.contains(r#"<w:spacing w:before="240" w:after="120" w:line="360" w:lineRule="auto"/><w:outlineLvl"#), "{h}");
        assert!(h.contains("<w:keepNext/>"));
        // Läser man tillbaka får man det nya.
        assert_eq!(read(&out, &THEME), new);
        // Andra mallar är orörda.
        assert!(out.contains(r#"<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="864" w:firstLine="360"/></w:pPr><w:rPr><w:i/><w:iCs/></w:rPr></w:style>"#));
    }

    #[test]
    fn reset_removes_and_new_blocks_are_created() {
        let old = read(STYLES, &THEME);
        let mut new = old.clone();
        let mut q = old.get(StyleId::Quote).clone();
        q.first_indent = None;
        q.align = Some(Align::Center);
        q.color = Some([1, 2, 3]);
        new.set(StyleId::Quote, q);
        let out = patch_styles(STYLES, &old, &new);
        assert!(out.contains(r#"<w:ind w:left="864"/><w:jc w:val="center"/></w:pPr><w:rPr><w:i/><w:iCs/><w:color w:val="010203"/></w:rPr>"#), "{out}");
        // En mall utan egna blocks får dem.
        let mut b = StyleSet::default();
        b.set(StyleId::Heading(2), StyleOverride { size: Some(14.0), ..Default::default() });
        let xml = "<w:styles><w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:qFormat/></w:style></w:styles>";
        let out = patch_styles(xml, &StyleSet::default(), &b);
        assert!(out.contains(r#"<w:qFormat/><w:rPr><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style>"#), "{out}");
    }

    // ---------------------------------------------------------- hela vägen genom docx

    fn part(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut f = z.by_name(name).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut f, &mut s).unwrap();
        s
    }

    fn with_part(bytes: &[u8], name: &str, xml: &str) -> Vec<u8> {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut out);
            for i in 0..z.len() {
                let mut f = z.by_index(i).unwrap();
                let n = f.name().to_owned();
                let mut data = Vec::new();
                std::io::Read::read_to_end(&mut f, &mut data).unwrap();
                w.start_file(n.clone(), zip::write::SimpleFileOptions::default()).unwrap();
                std::io::Write::write_all(&mut w, if n == name { xml.as_bytes() } else { &data }).unwrap();
            }
            w.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn docx_styles_become_overrides_and_are_written_back() {
        let base = crate::docx::write("Hej\n", "x", &|_| None, None).unwrap().bytes;
        let styles_xml = STYLES.replace(r#"w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi""#, r#"w:ascii="Calibri Light" w:hAnsi="Calibri Light""#);
        let docx = with_part(&base, "word/styles.xml", &styles_xml);
        let l = crate::docx::read(&docx).unwrap();
        let first = l.markdown.lines().next().unwrap();
        assert!(first.contains("h1: \"Calibri Light\" 16pt"), "{first}");
        assert!(first.contains("space-before 12pt space-after 0pt"), "{first}");
        assert!(first.contains("quote: #000000 justify indent 18pt"), "{first}");
        // Oförändrat dokument: formatmallsdelen är byte för byte lika.
        let same = crate::docx::write(&l.markdown, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        // (Kladds egna saknade mallar, t.ex. Heading2, läggs till som förut; de som fanns är orörda.)
        let same_styles = part(&same, "word/styles.xml");
        assert!(same_styles.starts_with(styles_xml.trim_end_matches("</w:styles>")), "orörda mallar ändrades");
        // Ändrad Rubrik 1 skrivs till styles.xml, övriga mallar är orörda.
        let md = l.markdown.replacen("h1: \"Calibri Light\" 16pt", "h1: \"Georgia\" 22pt bold", 1);
        let out = crate::docx::write(&md, "x", &|_| None, l.source.as_ref()).unwrap().bytes;
        let styles = part(&out, "word/styles.xml");
        assert!(styles.contains(r#"<w:rFonts w:ascii="Georgia""#), "{styles}");
        assert!(styles.contains(r#"<w:sz w:val="44"/>"#), "{styles}");
        assert!(styles.contains("<w:b/>"), "{styles}");
        assert!(styles.contains(r#"<w:style w:type="paragraph" w:styleId="Quote">"#));
        // Och läses tillbaka som samma formatmall.
        let back = crate::docx::read(&out).unwrap();
        assert!(back.markdown.lines().next().unwrap().contains("h1: \"Georgia\" 22pt"), "{}", back.markdown);
    }

    #[test]
    fn fresh_docx_gets_overrides_in_styles_xml() {
        let md = "<!-- kladd: h2: 14pt italic #112233 center space-after 9pt; body: line-spacing 1.5 -->\n\n## Rubrik\n\nText\n";
        let bytes = crate::docx::write(md, "x", &|_| None, None).unwrap().bytes;
        let styles = part(&bytes, "word/styles.xml");
        let h2 = &styles[styles.find(r#"w:styleId="Heading2""#).unwrap()..];
        let h2 = &h2[..h2.find("</w:style>").unwrap()];
        assert!(h2.contains(r#"<w:jc w:val="center"/>"#) && h2.contains(r#"w:after="180""#), "{h2}");
        assert!(h2.contains(r#"<w:i/>"#) && h2.contains(r#"<w:color w:val="112233"/>"#) && h2.contains(r#"<w:sz w:val="28"/>"#), "{h2}");
        let normal = &styles[styles.find(r#"w:styleId="Normal""#).unwrap()..];
        assert!(normal[..normal.find("</w:style>").unwrap()].contains(r#"w:line="360""#));
        let back = crate::docx::read(&bytes).unwrap();
        assert!(back.markdown.lines().next().unwrap().contains("line-spacing 1.5"), "{}", back.markdown);
    }

    // ---------------------------------------------------------- Titel och Underrubrik

    const TITLE_STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="8" w:space="4" w:color="4F81BD" w:themeColor="accent1"/></w:pBdr><w:spacing w:after="300"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi"/><w:color w:themeColor="text2" w:themeShade="BF"/><w:spacing w:val="5"/><w:sz w:val="52"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Subtitle"><w:name w:val="Subtitle"/><w:basedOn w:val="Normal"/><w:rPr><w:i/><w:caps/><w:color w:val="4F81BD"/><w:spacing w:val="15"/><w:sz w:val="24"/></w:rPr></w:style></w:styles>"#;

    #[test]
    fn title_and_subtitle_are_resolved_with_theme_colors_and_borders() {
        let s = read(TITLE_STYLES, &THEME);
        let t = s.get(StyleId::Title);
        assert_eq!(t.font_family.as_deref(), Some("Calibri Light"));
        assert_eq!(t.size, Some(26.0));
        // text2 1F497D med shade BF = 17365D (Words egen utskrivna färg).
        assert_eq!(t.color, Some([0x17, 0x36, 0x5D]));
        assert_eq!(t.letter_spacing, Some(0.25));
        assert_eq!(t.space_after, Some(15.0));
        assert_eq!(t.border_bottom, Some(Border { width: 1.0, color: [0x4F, 0x81, 0xBD], space: 4.0 }));
        let st = s.get(StyleId::Subtitle);
        assert_eq!((st.italic, st.caps, st.letter_spacing, st.size), (Some(true), Some(true), Some(0.75), Some(12.0)));
        // Rubrik 1 saknas: ingen ändring.
        assert!(s.get(StyleId::Heading(1)).is_empty());
    }

    #[test]
    fn tint_and_shade_follow_word() {
        assert_eq!(tint_shade([0x1F, 0x49, 0x7D], None, Some("BF")), [0x17, 0x36, 0x5D]);
        assert_eq!(tint_shade([0x4F, 0x81, 0xBD], Some("99"), None), [0x95, 0xB3, 0xD7]);
    }

    #[test]
    fn title_style_round_trips_through_docx_and_back() {
        let base = crate::docx::write("Hej\n", "x", &|_| None, None).unwrap().bytes;
        let docx = with_part(&base, "word/styles.xml", &TITLE_STYLES.replace("<w:styles ", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles "));
        // Ett dokument med en Title-rad: pStyle Title.
        let loaded = crate::docx::read(&docx).unwrap();
        let head = loaded.markdown.lines().next().unwrap().to_owned();
        let md = format!("{head}\n\n# Mitt dokument {{.title}}\n\n## Under {{.subtitle}}\n\nText\n");
        let out = crate::docx::write(&md, "x", &|_| None, loaded.source.as_ref()).unwrap().bytes;
        let doc = part(&out, "word/document.xml");
        assert!(doc.contains(r#"<w:pStyle w:val="Title"/>"#) && doc.contains(r#"<w:pStyle w:val="Subtitle"/>"#), "{doc}");
        let back = crate::docx::read(&out).unwrap();
        assert!(back.markdown.contains("# Mitt dokument {.title}"), "{}", back.markdown);
        assert!(back.markdown.contains("## Under {.subtitle}"), "{}", back.markdown);
        // Dokumentets Title-mall (kantlinje, teckenavstånd) finns kvar i dialektraden.
        assert!(back.markdown.lines().next().unwrap().contains("border-bottom 1pt #4F81BD 4pt"), "{}", back.markdown);
        // Ny fil utan källa får Kladds egna Title/Subtitle-mallar.
        let fresh = crate::docx::write("# Mitt dokument {.title}\n", "x", &|_| None, None).unwrap().bytes;
        let styles = part(&fresh, "word/styles.xml");
        assert!(styles.contains(r#"w:styleId="Title""#) && styles.contains(r#"w:styleId="Subtitle""#));
        assert!(crate::docx::read(&fresh).unwrap().markdown.contains("# Mitt dokument {.title}"));
    }

    #[test]
    fn bare_title_style_is_black_not_kladd_blue() {
        let xml = r#"<w:styles xmlns:w="x"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:rPr><w:sz w:val="56"/></w:rPr></w:style></w:styles>"#;
        let kladd = r#"<w:styles><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:rPr><w:color w:val="1F3864"/><w:sz w:val="56"/></w:rPr></w:style></w:styles>"#;
        let s = read_relative(xml, &Theme::NONE, kladd);
        assert_eq!(s.get(StyleId::Title).color, Some([0, 0, 0]));
        assert_eq!(s.get(StyleId::Title).size, None);
    }
}
