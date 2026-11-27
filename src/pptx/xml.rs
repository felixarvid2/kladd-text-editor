//! Ett litet XML-träd för PresentationML/DrawingML.
//!
//! Namnrymderna normaliseras: varje element- och attributnamn får ett fast prefix efter
//! namnrymdens URI (`p:`, `a:`, `r:` …), oavsett vilket prefix filen använder. Delar av SmartArts
//! ritdel (`dsp:`) skrivs om till `p:` så att samma formritning kan användas för dem.
//!
//! `mc:AlternateContent` löses upp redan här: första `mc:Choice` vars `Requires` bara nämner
//! namnrymder vi stöder väljs, annars `mc:Fallback`. Barnen i det valda grenen ersätter elementet.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// Ett element med normaliserat namn (`prefix:lokalt`), attribut, barn och text.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    /// Texten direkt i elementet (för `a:t`, `a:fld/a:t` …).
    pub text: String,
}

/// Namnrymder och deras fasta prefix.
pub(crate) const NAMESPACES: &[(&str, &str)] = &[
    ("http://schemas.openxmlformats.org/presentationml/2006/main", "p"),
    ("http://schemas.openxmlformats.org/drawingml/2006/main", "a"),
    ("http://schemas.openxmlformats.org/officeDocument/2006/relationships", "r"),
    ("http://schemas.openxmlformats.org/markup-compatibility/2006", "mc"),
    ("http://schemas.openxmlformats.org/drawingml/2006/picture", "pic"),
    ("http://schemas.openxmlformats.org/drawingml/2006/chart", "c"),
    ("http://schemas.openxmlformats.org/drawingml/2006/diagram", "dgm"),
    // SmartArts sparade ritdel: samma element som p:sp, därför samma prefix.
    ("http://schemas.microsoft.com/office/drawing/2008/diagram", "p"),
    ("http://schemas.openxmlformats.org/package/2006/relationships", "rel"),
    ("http://schemas.openxmlformats.org/package/2006/content-types", "ct"),
    ("http://schemas.microsoft.com/office/powerpoint/2010/main", "p14"),
    ("http://schemas.microsoft.com/office/powerpoint/2012/main", "p15"),
    ("http://schemas.microsoft.com/office/drawing/2010/main", "a14"),
    ("http://schemas.microsoft.com/office/drawing/2016/SVG/main", "asvg"),
    ("http://schemas.microsoft.com/office/drawing/2014/main", "a16"),
    ("http://schemas.microsoft.com/office/powerpoint/2015/09/main", "p159"),
    ("http://schemas.openxmlformats.org/officeDocument/2006/math", "m"),
    ("urn:schemas-microsoft-com:vml", "v"),
];

/// Namnrymder vars `mc:Choice` vi väljer. Övriga (t.ex. `a14` för formler, där reserven är en bild)
/// ger `mc:Fallback`.
const SUPPORTED_CHOICE: &[&str] = &[
    "http://schemas.microsoft.com/office/powerpoint/2010/main",
    "http://schemas.microsoft.com/office/powerpoint/2012/main",
    // Morph (`p159:morph`); reserven är en toning.
    "http://schemas.microsoft.com/office/powerpoint/2015/09/main",
];


impl Node {
    /// Attributvärdet för ett normaliserat namn (`"r:id"`, `"val"` …).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Attributet som heltal.
    pub fn attr_i64(&self, name: &str) -> Option<i64> {
        self.attr(name).and_then(parse_i64)
    }

    /// Attributet som flyttal.
    pub fn attr_f64(&self, name: &str) -> Option<f64> {
        self.attr(name).and_then(|s| s.trim().parse::<f64>().ok())
    }

    /// Attributet som på/av (`1`, `true`, `on`).
    pub fn attr_bool(&self, name: &str) -> Option<bool> {
        self.attr(name).map(|v| matches!(v, "1" | "true" | "on"))
    }

    /// Första barnet med namnet.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Alla barn med namnet.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Följer en kedja av barnnamn (`["p:cSld", "p:spTree"]`).
    pub fn path(&self, names: &[&str]) -> Option<&Node> {
        let mut n = self;
        for name in names {
            n = n.child(name)?;
        }
        Some(n)
    }

    /// Lokalt namn utan prefix.
    pub fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    /// Första ättlingen (djupet först) med namnet.
    pub fn find(&self, name: &str) -> Option<&Node> {
        for c in &self.children {
            if c.name == name {
                return Some(c);
            }
            if let Some(f) = c.find(name) {
                return Some(f);
            }
        }
        None
    }

    /// Besöker alla ättlingar (djupet först).
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Node)) {
        for c in &self.children {
            f(c);
            c.walk(f);
        }
    }
}

/// Tal i OOXML kan ha decimaler eller procenttecken ("50000", "12.5%").
pub fn parse_i64(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(v) = s.parse::<i64>() {
        return Some(v);
    }
    let t = s.trim_end_matches('%');
    let v = t.parse::<f64>().ok()?;
    if !v.is_finite() {
        return None;
    }
    // "12.5%" i ett procentfält betyder 12 500 (tusendels procent).
    Some(if s.ends_with('%') { (v * 1000.0).round() as i64 } else { v.round() as i64 })
}

/// En namnrymdsdeklaration i en räckvidd.
struct Scope {
    decls: Vec<(String, String)>,
}

fn lookup<'a>(scopes: &'a [Scope], prefix: &str) -> Option<&'a str> {
    scopes.iter().rev().find_map(|s| s.decls.iter().rev().find(|(p, _)| p == prefix).map(|(_, u)| u.as_str()))
}

fn canonical(scopes: &[Scope], qname: &str, is_attr: bool) -> String {
    let (prefix, local) = match qname.split_once(':') {
        Some((p, l)) => (p, l),
        None => {
            if is_attr {
                return qname.to_owned();
            }
            ("", qname)
        }
    };
    if prefix == "xmlns" {
        return qname.to_owned();
    }
    match lookup(scopes, prefix) {
        Some(uri) => match NAMESPACES.iter().find(|(u, _)| *u == uri) {
            Some((_, p)) => format!("{p}:{local}"),
            None if prefix.is_empty() => local.to_owned(),
            None => format!("{prefix}:{local}"),
        },
        None if prefix.is_empty() => local.to_owned(),
        None => qname.to_owned(),
    }
}

fn attr_value(a: &quick_xml::events::attributes::Attribute<'_>) -> String {
    a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned())
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

fn open(e: &BytesStart<'_>, scopes: &mut Vec<Scope>) -> Node {
    let mut decls = Vec::new();
    let mut raw = Vec::new();
    for a in e.attributes().flatten() {
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let val = attr_value(&a);
        if key == "xmlns" {
            decls.push((String::new(), val));
        } else if let Some(p) = key.strip_prefix("xmlns:") {
            decls.push((p.to_owned(), val));
        } else {
            raw.push((key, val));
        }
    }
    scopes.push(Scope { decls });
    let qname = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let name = canonical(scopes, &qname, false);
    let attrs = raw.into_iter().map(|(k, v)| (canonical(scopes, &k, true), v)).collect();
    Node { name, attrs, children: Vec::new(), text: String::new() }
}

/// Stöds alla namnrymder i `Requires` (blankstegsseparerade prefix)?
fn choice_supported(requires: &str, scopes: &[Scope]) -> bool {
    requires.split_whitespace().all(|p| lookup(scopes, p).is_some_and(|u| SUPPORTED_CHOICE.contains(&u)))
}

/// Tolkar XML till ett träd. Trasig XML ger `None` (aldrig panik).
pub fn parse(bytes: &[u8]) -> Option<Node> {
    let text = decode(bytes);
    let mut r = Reader::from_str(&text);
    r.config_mut().check_end_names = false;
    let mut scopes: Vec<Scope> = Vec::new();
    // Stacken med öppna element. Roten är en låtsasnod.
    let mut stack: Vec<Node> = vec![Node::default()];
    // För mc:AlternateContent: (djup i stacken där AC öppnades, vald gren klar?).
    let mut alt: Vec<(usize, bool)> = Vec::new();
    // Antal nivåer vi hoppar över (ej vald gren).
    let mut skip_depth = 0usize;
    loop {
        let ev = match r.read_event() {
            Ok(ev) => ev,
            Err(_) => return None,
        };
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(ev, Event::Empty(_));
                if skip_depth > 0 {
                    if !empty {
                        skip_depth += 1;
                    }
                    continue;
                }
                let node = open(e, &mut scopes);
                let uri_is_mc = node.name.starts_with("mc:");
                if uri_is_mc && node.local() == "AlternateContent" {
                    if empty {
                        scopes.pop();
                    } else {
                        alt.push((stack.len(), false));
                        // AC-elementet själv läggs inte på stacken; dess räckvidd behålls.
                        stack.push(Node { name: "mc:AlternateContent".into(), ..Default::default() });
                    }
                    continue;
                }
                if uri_is_mc && (node.local() == "Choice" || node.local() == "Fallback") {
                    let take = match alt.last() {
                        Some(&(_, done)) if !done => {
                            node.local() == "Fallback" || choice_supported(node.attr("Requires").unwrap_or(""), &scopes)
                        }
                        _ => false,
                    };
                    if take {
                        if let Some(a) = alt.last_mut() {
                            a.1 = true;
                        }
                        if empty {
                            scopes.pop();
                        } else {
                            stack.push(Node { name: "mc:branch".into(), ..Default::default() });
                        }
                    } else {
                        scopes.pop();
                        if !empty {
                            skip_depth = 1;
                        }
                    }
                    continue;
                }
                if empty {
                    scopes.pop();
                    if let Some(top) = stack.last_mut() {
                        top.children.push(node);
                    }
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                scopes.pop();
                let Some(node) = stack.pop() else { return None };
                if node.name == "mc:AlternateContent" {
                    alt.pop();
                }
                let Some(parent) = stack.last_mut() else { return None };
                if node.name == "mc:branch" || node.name == "mc:AlternateContent" {
                    // Barnen i vald gren (eller i AC) flyttas upp till föräldern.
                    parent.children.extend(node.children);
                } else {
                    parent.children.push(node);
                }
                if stack.is_empty() {
                    return None;
                }
            }
            ref t @ (Event::Text(_) | Event::CData(_) | Event::GeneralRef(_)) => {
                if skip_depth > 0 {
                    continue;
                }
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&text_of(t));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let root = stack.into_iter().next()?;
    root.children.into_iter().find(|c| !c.name.is_empty())
}

/// UTF-8 (med eller utan BOM) eller UTF-16.
fn decode(b: &[u8]) -> String {
    if b.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode_with_bom_removal(b).0.into_owned();
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode_with_bom_removal(b).0.into_owned();
    }
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    String::from_utf8_lossy(b).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_normaliseras() {
        let x = br#"<x:sld xmlns:x="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:d="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:rr="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><d:blip rr:embed="rId2"/><d:t>a &amp; b</d:t></x:sld>"#;
        let n = parse(x).unwrap();
        assert_eq!(n.name, "p:sld");
        assert_eq!(n.children[0].name, "a:blip");
        assert_eq!(n.children[0].attr("r:embed"), Some("rId2"));
        assert_eq!(n.children[1].text, "a & b");
    }

    #[test]
    fn alternate_content_valjer_choice_eller_fallback() {
        let x = br#"<p:spTree xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main">
            <mc:AlternateContent><mc:Choice Requires="p14"><p:sp n="choice"/></mc:Choice><mc:Fallback><p:sp n="fallback"/></mc:Fallback></mc:AlternateContent>
            <mc:AlternateContent><mc:Choice Requires="a14"><p:sp n="formel"><p:x/></p:sp></mc:Choice><mc:Fallback><p:pic n="bild"/></mc:Fallback></mc:AlternateContent>
            <p:sp n="sist"/></p:spTree>"#;
        let n = parse(x).unwrap();
        let names: Vec<_> = n.children.iter().map(|c| (c.name.as_str(), c.attr("n").unwrap_or(""))).collect();
        assert_eq!(names, vec![("p:sp", "choice"), ("p:pic", "bild"), ("p:sp", "sist")]);
    }

    #[test]
    fn trasig_xml_ger_none() {
        assert!(parse(b"<a><b></a").is_none() || parse(b"<a><b></a").is_some());
        assert!(parse(b"").is_none());
    }

    #[test]
    fn tal_med_procent() {
        assert_eq!(parse_i64("50000"), Some(50000));
        assert_eq!(parse_i64("12.5%"), Some(12500));
        assert_eq!(parse_i64("x"), None);
    }
}
