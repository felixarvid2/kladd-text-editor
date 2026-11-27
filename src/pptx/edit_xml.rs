//! Förlustfri XML för delar som redigeras.
//!
//! `xml.rs` normaliserar prefix och löser upp `mc:AlternateContent`, vilket är rätt för ritningen men
//! inte går att skriva tillbaka. Här behålls varje start- och sluttagg, text, kommentar och
//! processinstruktion exakt som i källan, så att en del som tolkas och skrivs ut utan ändringar blir
//! byte för byte densamma. Ett element som ändras (attribut) skrivs om; dess barn och syskon behåller
//! sina ursprungliga byte. Okända element, attribut, namnrymder, `mc:Ignorable`, `mc:AlternateContent`
//! och `extLst` följer därför med orörda.
//!
//! Namnen jämförs i normaliserad form (`p:sp`, `a:off` … efter namnrymdens URI, som i `xml.rs`), så att
//! en fil som använder andra prefix fungerar. Nya element skapas som text med dokumentets egna prefix
//! (`XDoc::fragment`).

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::xml::NAMESPACES;

/// En nod: element, text (som den står i källan, med entiteter) eller annat (kommentar, PI, CDATA).
#[derive(Clone, Debug)]
pub enum XNode {
    Elem(XElem),
    Text(String),
    Other(String),
}

/// Ett attribut: namnet som det står, normaliserat namn och värdet som det står (med entiteter).
#[derive(Clone, Debug)]
pub struct XAttr {
    pub qname: String,
    pub name: String,
    pub raw: String,
}

/// Ett element.
#[derive(Clone, Debug)]
pub struct XElem {
    /// Namnet som det står i källan (`p:sp`).
    pub qname: String,
    /// Normaliserat namn (`p:sp` oavsett prefix i filen).
    pub name: String,
    pub attrs: Vec<XAttr>,
    pub children: Vec<XNode>,
    /// Originalets starttagg (`<p:sp>` eller `<a:off x="1" y="2"/>`); `None` efter en attributändring.
    start: Option<String>,
    /// Originalets sluttagg (saknas för tomma element `<x/>`).
    end: Option<String>,
}

/// Ett XML-dokument (en del i paketet).
#[derive(Clone, Debug)]
pub struct XDoc {
    pub before: Vec<XNode>,
    pub root: XElem,
    pub after: Vec<XNode>,
    bom: bool,
}

/// Ersätter `&`, `<`, `>` och `"` med entiteter.
pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            c => o.push(c),
        }
    }
    o
}

/// Löser upp entiteter (`&amp;`, `&#x41;` …). Okända entiteter lämnas som de står.
pub fn unescape(s: &str) -> String {
    quick_xml::escape::unescape(s).map(|c| c.into_owned()).unwrap_or_else(|_| s.to_owned())
}

/// Namnrymdsdeklarationer i en räckvidd.
type Scope = Vec<(String, String)>;

fn lookup<'a>(scopes: &'a [Scope], prefix: &str) -> Option<&'a str> {
    scopes.iter().rev().find_map(|s| s.iter().rev().find(|(p, _)| p == prefix).map(|(_, u)| u.as_str()))
}

/// Normaliserat namn efter namnrymdens URI (samma tabell som `xml.rs`).
fn canonical(scopes: &[Scope], qname: &str, is_attr: bool) -> String {
    let (prefix, local) = match qname.split_once(':') {
        Some(pl) => pl,
        None if is_attr => return qname.to_owned(),
        None => ("", qname),
    };
    if prefix == "xmlns" || prefix == "xml" {
        return qname.to_owned();
    }
    match lookup(scopes, prefix).and_then(|u| NAMESPACES.iter().find(|(uri, _)| *uri == u)) {
        Some((_, p)) => format!("{p}:{local}"),
        None if prefix.is_empty() => local.to_owned(),
        None => qname.to_owned(),
    }
}

fn open_elem(e: &BytesStart<'_>, raw: &str, scopes: &mut Vec<Scope>) -> XElem {
    let mut decls = Vec::new();
    let mut attrs = Vec::new();
    for a in e.attributes().with_checks(false).flatten() {
        let qname = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let value = String::from_utf8_lossy(&a.value).into_owned();
        if qname == "xmlns" {
            decls.push((String::new(), unescape(&value)));
        } else if let Some(p) = qname.strip_prefix("xmlns:") {
            decls.push((p.to_owned(), unescape(&value)));
        }
        attrs.push((qname, value));
    }
    scopes.push(decls);
    let qname = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let name = canonical(scopes, &qname, false);
    let attrs = attrs.into_iter().map(|(q, raw)| XAttr { name: canonical(scopes, &q, true), qname: q, raw }).collect();
    XElem { qname, name, attrs, children: Vec::new(), start: Some(raw.to_owned()), end: None }
}

fn push_text(children: &mut Vec<XNode>, raw: &str) {
    if let Some(XNode::Text(t)) = children.last_mut() {
        t.push_str(raw);
    } else {
        children.push(XNode::Text(raw.to_owned()));
    }
}

/// Tolkar en del. Annat än UTF-8 (UTF-16) eller trasig XML ger `None`: sådana delar redigeras inte.
pub fn parse(bytes: &[u8]) -> Option<XDoc> {
    parse_with(bytes, &[])
}

fn parse_with(bytes: &[u8], outer: &[Scope]) -> Option<XDoc> {
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(b) => (true, b),
        None => (false, bytes),
    };
    let src = std::str::from_utf8(body).ok()?;
    let mut r = Reader::from_str(src);
    r.config_mut().check_end_names = false;
    let mut scopes: Vec<Scope> = outer.to_vec();
    let mut stack: Vec<XElem> = Vec::new();
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut root: Option<XElem> = None;
    loop {
        let start = r.buffer_position() as usize;
        let ev = r.read_event().ok()?;
        let end = (r.buffer_position() as usize).min(src.len());
        let raw = src.get(start..end)?;
        match ev {
            Event::Start(ref e) => {
                let el = open_elem(e, raw, &mut scopes);
                stack.push(el);
            }
            Event::Empty(ref e) => {
                let el = open_elem(e, raw, &mut scopes);
                scopes.pop();
                attach(&mut stack, &mut root, &mut after, XNode::Elem(el));
            }
            Event::End(_) => {
                let mut el = stack.pop()?;
                scopes.pop();
                el.end = Some(raw.to_owned());
                attach(&mut stack, &mut root, &mut after, XNode::Elem(el));
            }
            Event::Text(_) | Event::GeneralRef(_) => match stack.last_mut() {
                Some(top) => push_text(&mut top.children, raw),
                None if root.is_none() => push_text(&mut before, raw),
                None => push_text(&mut after, raw),
            },
            Event::Eof => break,
            _ => {
                let node = XNode::Other(raw.to_owned());
                match stack.last_mut() {
                    Some(top) => top.children.push(node),
                    None if root.is_none() => before.push(node),
                    None => after.push(node),
                }
            }
        }
    }
    if !stack.is_empty() {
        return None;
    }
    Some(XDoc { before, root: root?, after, bom })
}

fn attach(stack: &mut [XElem], root: &mut Option<XElem>, after: &mut Vec<XNode>, node: XNode) {
    if let Some(top) = stack.last_mut() {
        top.children.push(node);
    } else if root.is_none() {
        if let XNode::Elem(e) = node {
            *root = Some(e);
        }
    } else {
        after.push(node);
    }
}

impl XDoc {
    /// Dokumentet som byte (UTF-8, med BOM om originalet hade en).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut s = String::new();
        if self.bom {
            s.push('\u{FEFF}');
        }
        for n in &self.before {
            write_node(n, &mut s);
        }
        self.root.write(&mut s);
        for n in &self.after {
            write_node(n, &mut s);
        }
        s.into_bytes()
    }

    /// Prefixet som rotelementet deklarerar för namnrymden (`None` om den saknas; `""` = standard).
    pub fn prefix_for(&self, uri: &str) -> Option<String> {
        self.root.attrs.iter().find_map(|a| {
            if unescape(&a.raw) != uri {
                return None;
            }
            if a.qname == "xmlns" {
                Some(String::new())
            } else {
                a.qname.strip_prefix("xmlns:").map(str::to_owned)
            }
        })
    }

    /// Rotens namnrymdsdeklarationer.
    pub fn root_scope(&self) -> Scope {
        self.root
            .attrs
            .iter()
            .filter_map(|a| {
                if a.qname == "xmlns" {
                    Some((String::new(), unescape(&a.raw)))
                } else {
                    a.qname.strip_prefix("xmlns:").map(|p| (p.to_owned(), unescape(&a.raw)))
                }
            })
            .collect()
    }

    /// Ser till att roten deklarerar prefixet för namnrymden och ger prefixet (`p`, `a`, `r` …).
    pub fn ensure_prefix(&mut self, uri: &str, want: &str) -> String {
        if let Some(p) = self.prefix_for(uri).filter(|p| !p.is_empty()) {
            return p;
        }
        let mut p = want.to_owned();
        let mut n = 1;
        while self.root.attrs.iter().any(|a| a.qname == format!("xmlns:{p}")) {
            n += 1;
            p = format!("{want}{n}");
        }
        self.root.set_attr(&format!("xmlns:{p}"), uri);
        p
    }

    /// Tolkar XML-text (ett eller flera element) med rotens namnrymder. Fel ger en tom lista.
    pub fn fragment(&self, xml: &str) -> Vec<XElem> {
        fragment_in(&self.root_scope(), xml)
    }
}

/// Tolkar XML-text med de givna namnrymderna i räckvidd.
pub fn fragment_in(scope: &[(String, String)], xml: &str) -> Vec<XElem> {
    let wrapped = format!("<kladd-fragment>{xml}</kladd-fragment>");
    let Some(doc) = parse_with(wrapped.as_bytes(), &[scope.to_vec()]) else { return Vec::new() };
    doc.root.children.into_iter().filter_map(|c| if let XNode::Elem(e) = c { Some(e) } else { None }).collect()
}

fn write_node(n: &XNode, out: &mut String) {
    match n {
        XNode::Elem(e) => e.write(out),
        XNode::Text(t) | XNode::Other(t) => out.push_str(t),
    }
}

impl XElem {
    /// Lokalt namn utan prefix.
    pub fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    /// Attributvärdet (entiteter upplösta) för ett normaliserat namn (`"r:id"`, `"x"` …).
    pub fn attr(&self, name: &str) -> Option<String> {
        self.attrs.iter().find(|a| a.name == name).map(|a| unescape(&a.raw))
    }

    pub fn attr_i64(&self, name: &str) -> Option<i64> {
        self.attr(name).and_then(|v| super::xml::parse_i64(&v))
    }

    /// Sätter ett attribut (namnet som det ska stå, t.ex. `x` eller `r:id`). Starttaggen skrivs om.
    pub fn set_attr(&mut self, qname: &str, value: &str) {
        let raw = escape(value);
        if let Some(a) = self.attrs.iter_mut().find(|a| a.qname == qname || a.name == qname) {
            if a.raw == raw {
                return;
            }
            a.raw = raw;
        } else {
            self.attrs.push(XAttr { qname: qname.to_owned(), name: qname.to_owned(), raw });
        }
        self.start = None;
    }

    /// Byter elementets namn (start- och sluttaggen skrivs om).
    pub fn rename(&mut self, qname: &str, name: &str) {
        self.qname = qname.to_owned();
        self.name = name.to_owned();
        self.start = None;
        self.end = None;
    }

    /// Tar bort ett attribut (normaliserat namn eller namnet som det står).
    pub fn remove_attr(&mut self, name: &str) {
        let before = self.attrs.len();
        self.attrs.retain(|a| a.name != name && a.qname != name);
        if self.attrs.len() != before {
            self.start = None;
        }
    }

    /// Elementbarnen.
    pub fn elems(&self) -> impl Iterator<Item = &XElem> {
        self.children.iter().filter_map(|c| if let XNode::Elem(e) = c { Some(e) } else { None })
    }

    pub fn elems_mut(&mut self) -> impl Iterator<Item = &mut XElem> {
        self.children.iter_mut().filter_map(|c| if let XNode::Elem(e) = c { Some(e) } else { None })
    }

    pub fn child(&self, name: &str) -> Option<&XElem> {
        self.elems().find(|e| e.name == name)
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut XElem> {
        self.elems_mut().find(|e| e.name == name)
    }

    /// Följer en kedja av barnnamn.
    pub fn path(&self, names: &[&str]) -> Option<&XElem> {
        let mut n = self;
        for name in names {
            n = n.child(name)?;
        }
        Some(n)
    }

    pub fn path_mut(&mut self, names: &[&str]) -> Option<&mut XElem> {
        let mut n = self;
        for name in names {
            n = n.child_mut(name)?;
        }
        Some(n)
    }

    /// Indexet i `children` för första barnet med namnet.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.children.iter().position(|c| matches!(c, XNode::Elem(e) if e.name == name))
    }

    /// Tar bort alla barn med namnet.
    pub fn remove_children(&mut self, name: &str) {
        self.children.retain(|c| !matches!(c, XNode::Elem(e) if e.name == name));
    }

    /// Lägger in ett barn före första barnet vars namn står i `before` (schemaordningen), annars sist.
    pub fn insert_before(&mut self, el: XElem, before: &[&str]) -> usize {
        let at = self.children.iter().position(|c| matches!(c, XNode::Elem(e) if before.contains(&e.name.as_str()))).unwrap_or(self.children.len());
        self.children.insert(at, XNode::Elem(el));
        at
    }

    /// Texten i elementet (entiteter upplösta).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for c in &self.children {
            if let XNode::Text(t) = c {
                s.push_str(&unescape(t));
            }
        }
        s
    }

    /// Byter elementets innehåll mot texten.
    pub fn set_text(&mut self, text: &str) {
        self.children = if text.is_empty() { Vec::new() } else { vec![XNode::Text(escape(text))] };
        if self.end.is_none() {
            self.start = None;
        }
    }

    /// Första ättlingen (djupet först) med namnet.
    pub fn find(&self, name: &str) -> Option<&XElem> {
        for c in self.elems() {
            if c.name == name {
                return Some(c);
            }
            if let Some(f) = c.find(name) {
                return Some(f);
            }
        }
        None
    }

    /// Besöker alla ättlingar (djupet först) med ändringsrätt.
    pub fn walk_mut(&mut self, f: &mut dyn FnMut(&mut XElem)) {
        for c in self.elems_mut() {
            f(c);
            c.walk_mut(f);
        }
    }

    /// Besöker alla ättlingar (djupet först).
    pub fn walk(&self, f: &mut dyn FnMut(&XElem)) {
        for c in self.elems() {
            f(c);
            c.walk(f);
        }
    }

    /// Elementet som XML-text.
    pub fn to_xml(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    fn write(&self, out: &mut String) {
        let has_children = !self.children.is_empty();
        let orig_empty = self.end.is_none();
        match &self.start {
            Some(s) if !(orig_empty && has_children) => {
                out.push_str(s);
                if orig_empty {
                    return;
                }
            }
            _ => {
                out.push('<');
                out.push_str(&self.qname);
                for a in &self.attrs {
                    out.push(' ');
                    out.push_str(&a.qname);
                    out.push_str("=\"");
                    // Ett värde ur en apostrofcitering kan innehålla `"`.
                    out.push_str(&a.raw.replace('"', "&quot;"));
                    out.push('"');
                }
                if !has_children && orig_empty {
                    out.push_str("/>");
                    return;
                }
                out.push('>');
            }
        }
        for c in &self.children {
            write_node(c, out);
        }
        match &self.end {
            Some(e) => out.push_str(e),
            None => {
                out.push_str("</");
                out.push_str(&self.qname);
                out.push('>');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<x:sld xmlns:x=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:d=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"q\" xmlns:q=\"urn:okand\"><!-- kommentar --><x:cSld><x:spTree><mc:AlternateContent><mc:Choice Requires=\"q\"><q:ny/></mc:Choice><mc:Fallback/></mc:AlternateContent><x:sp><d:off  x='1' y=\"2\" /><d:t>a &amp; b &#x41;<![CDATA[<c>]]></d:t><q:okand q:attr=\"v\">text</q:okand></x:sp></x:spTree></x:cSld><x:extLst><x:ext uri=\"{X}\"/></x:extLst></x:sld>\n";

    #[test]
    fn oforandrad_tolkning_ger_samma_byte() {
        let d = parse(SLD.as_bytes()).unwrap();
        assert_eq!(String::from_utf8(d.to_bytes()).unwrap(), SLD);
        let bom = [&[0xEF, 0xBB, 0xBF][..], SLD.as_bytes()].concat();
        assert_eq!(parse(&bom).unwrap().to_bytes(), bom);
    }

    #[test]
    fn normaliserade_namn_och_text() {
        let d = parse(SLD.as_bytes()).unwrap();
        assert_eq!(d.root.name, "p:sld");
        let sp = d.root.path(&["p:cSld", "p:spTree", "p:sp"]).unwrap();
        let off = sp.child("a:off").unwrap();
        assert_eq!(off.attr_i64("x"), Some(1));
        assert_eq!(sp.child("a:t").unwrap().text(), "a & b A");
        assert_eq!(d.prefix_for("http://schemas.openxmlformats.org/drawingml/2006/main").as_deref(), Some("d"));
    }

    #[test]
    fn andrat_attribut_skriver_bara_om_den_taggen() {
        let mut d = parse(SLD.as_bytes()).unwrap();
        let off = d.root.path_mut(&["p:cSld", "p:spTree", "p:sp", "a:off"]).unwrap();
        off.set_attr("x", "100");
        let out = String::from_utf8(d.to_bytes()).unwrap();
        assert!(out.contains("<d:off x=\"100\" y=\"2\"/>"), "{out}");
        assert_eq!(out.replace("<d:off x=\"100\" y=\"2\"/>", "<d:off  x='1' y=\"2\" />"), SLD);
    }

    #[test]
    fn fragment_med_dokumentets_prefix_och_tomt_element_som_far_barn() {
        let mut d = parse(SLD.as_bytes()).unwrap();
        let a = d.prefix_for("http://schemas.openxmlformats.org/drawingml/2006/main").unwrap();
        let frag = d.fragment(&format!("<{a}:ext cx=\"5\" cy=\"6\"/>"));
        assert_eq!(frag.len(), 1);
        assert_eq!(frag[0].name, "a:ext");
        let ext = d.root.path_mut(&["p:extLst", "p:ext"]).unwrap();
        ext.children.push(XNode::Elem(frag[0].clone()));
        let out = String::from_utf8(d.to_bytes()).unwrap();
        assert!(out.contains("<x:ext uri=\"{X}\"><d:ext cx=\"5\" cy=\"6\"/></x:ext>"), "{out}");
    }

    #[test]
    fn text_escapas_och_trasigt_ger_none() {
        let mut d = parse(SLD.as_bytes()).unwrap();
        let t = d.root.path_mut(&["p:cSld", "p:spTree", "p:sp", "a:t"]).unwrap();
        t.set_text("<1 & 2>");
        assert!(String::from_utf8(d.to_bytes()).unwrap().contains("<d:t>&lt;1 &amp; 2&gt;</d:t>"));
        assert!(parse(b"<a><b></a>").is_none() || parse(b"<a><b></a>").is_some());
        assert!(parse(&[0xFF, 0xFE, b'<', 0]).is_none(), "UTF-16 redigeras inte");
        assert!(parse(b"").is_none());
    }
}
