//! Läser EPUB-paketet: zip-arkivet, `META-INF/container.xml`, OPF-filen (metadata, manifest, spine,
//! omslag) och innehållsförteckningen (EPUB 3 `nav` eller EPUB 2 `toc.ncx`).
//!
//! Alla XML-filer läses med den tåliga HTML-tolken i `paste.rs` (verkliga böcker är ofta slarviga),
//! så ett litet fel i en fil fäller inte hela boken.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::Arc;

use crate::paste::{El, Node, parse};

/// Så mycket packas som mest upp ur en enskild fil (skydd mot zip-bomber).
const MAX_ENTRY: u64 = 96 * 1024 * 1024;

/// Zip-arkivet med uppslagning som tål olika versaler och bakstreck.
pub struct Archive {
    zip: zip::ZipArchive<Cursor<Arc<[u8]>>>,
    exact: HashMap<String, usize>,
    lower: HashMap<String, usize>,
    names: Vec<String>,
}

impl Archive {
    pub fn open(bytes: Arc<[u8]>) -> Result<Self, String> {
        let zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|_| crate::i18n::t("Filen är inte en giltig EPUB (zip-arkivet kan inte läsas).").to_owned())?;
        let mut exact = HashMap::new();
        let mut lower = HashMap::new();
        let mut names = Vec::new();
        for i in 0..zip.len() {
            let Some(name) = zip.name_for_index(i) else { continue };
            if name.ends_with('/') {
                continue;
            }
            let name = name.replace('\\', "/");
            exact.entry(name.clone()).or_insert(i);
            lower.entry(name.to_lowercase()).or_insert(i);
            names.push(name);
        }
        Ok(Archive { zip, exact, lower, names })
    }

    /// Arkivets filnamn.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    fn index(&self, path: &str) -> Option<usize> {
        self.exact.get(path).or_else(|| self.lower.get(&path.to_lowercase())).copied()
    }

    pub fn has(&self, path: &str) -> bool {
        self.index(path).is_some()
    }

    pub fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        let i = self.index(path)?;
        let file = self.zip.by_index(i).ok()?;
        let mut out = Vec::with_capacity((file.size() as usize).min(1 << 24));
        file.take(MAX_ENTRY).read_to_end(&mut out).ok()?;
        Some(out)
    }

    pub fn read_text(&mut self, path: &str) -> Option<String> {
        self.read(path).map(|b| text_of_bytes(&b))
    }
}

/// Bytes som text: UTF-8 (med eller utan BOM), UTF-16 eller, om inget annat passar, Windows-1252.
pub fn text_of_bytes(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if b.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode_with_bom_removal(b).0.into_owned();
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode_with_bom_removal(b).0.into_owned();
    }
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::WINDOWS_1252.decode_without_bom_handling(b).0.into_owned(),
    }
}

// ------------------------------------------------------------------ sökvägar

/// `%20` och andra %-kodade tecken tillbaka till text.
pub fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = h {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Har adressen ett schema (`http:`, `mailto:` …)?
pub fn has_scheme(href: &str) -> bool {
    let Some(colon) = href.find(':') else { return false };
    colon > 1 && href[..colon].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Löser `href` (relativ mot katalogen `base_dir` i arkivet) till en sökväg i arkivet och ett fragment.
/// `../`, `./`, %-kodning, frågetecken och ledande snedstreck hanteras.
pub fn resolve(base_dir: &str, href: &str) -> (String, Option<String>) {
    let (path, frag) = match href.split_once('#') {
        Some((p, f)) => (p, Some(percent_decode(f))),
        None => (href, None),
    };
    let path = path.split('?').next().unwrap_or("");
    // Bara ett fragment (`#id`): samma fil, vilket anroparen avgör.
    if path.is_empty() {
        return (String::new(), frag.filter(|f| !f.is_empty()));
    }
    let path = percent_decode(path).replace('\\', "/");
    let mut parts: Vec<&str> = if path.starts_with('/') || base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    (parts.join("/"), frag.filter(|f| !f.is_empty()))
}

/// Katalogdelen av en sökväg i arkivet (utan avslutande snedstreck).
pub fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

// ------------------------------------------------------------------ XML-hjälpare

/// Taggnamnet utan namnrymdsprefix (`dc:title` blir `title`).
pub fn local(tag: &str) -> &str {
    tag.rsplit_once(':').map_or(tag, |(_, l)| l)
}

fn each<'a>(el: &'a El, f: &mut dyn FnMut(&'a El)) {
    for k in &el.kids {
        if let Node::El(e) = k {
            f(e);
            each(e, f);
        }
    }
}

fn find<'a>(el: &'a El, name: &str) -> Option<&'a El> {
    let mut r = None;
    each(el, &mut |e| {
        if r.is_none() && local(&e.tag) == name {
            r = Some(e);
        }
    });
    r
}

/// All text under elementet, med blanksteg hopdraget.
pub fn plain_text(el: &El) -> String {
    fn go(el: &El, out: &mut String) {
        for k in &el.kids {
            match k {
                Node::Text(t) => out.push_str(t),
                Node::El(e) => {
                    if matches!(local(&e.tag), "script" | "style") {
                        continue;
                    }
                    go(e, out);
                    if matches!(local(&e.tag), "p" | "div" | "br" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                        out.push(' ');
                    }
                }
            }
        }
    }
    let mut s = String::new();
    go(el, &mut s);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------ paketet

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: String,
    pub authors: Vec<String>,
    pub publisher: String,
    pub language: String,
    pub date: String,
    /// Beskrivningen som ren text (förlagen skriver ofta HTML i fältet).
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub path: String,
    pub media: String,
    pub props: String,
}

#[derive(Clone, Debug)]
pub struct SpineRef {
    pub path: String,
    pub linear: bool,
}

/// En post i innehållsförteckningen som den står i boken.
#[derive(Clone, Debug, PartialEq)]
pub struct RawToc {
    pub title: String,
    pub depth: usize,
    /// Sökväg i arkivet och fragment.
    pub target: Option<(String, Option<String>)>,
}

#[derive(Debug, Default)]
pub struct Package {
    pub meta: Meta,
    pub items: HashMap<String, Item>,
    pub spine: Vec<SpineRef>,
    /// Omslagsbildens sökväg.
    pub cover: Option<String>,
    pub toc: Vec<RawToc>,
}

/// Fel för kopieringsskyddade böcker.
pub fn drm_message() -> String {
    crate::i18n::t("Boken är kopieringsskyddad (DRM) och kan inte öppnas.").to_owned()
}

/// Ser efter `encryption.xml` (annat än typsnittsobfuskering) och DRM-filer.
fn check_drm(ar: &mut Archive) -> Result<(), String> {
    if ar.has("META-INF/rights.xml") || ar.has("META-INF/sinf.xml") {
        return Err(drm_message());
    }
    let Some(xml) = ar.read_text("META-INF/encryption.xml") else { return Ok(()) };
    let root = parse(&xml);
    let mut drm = false;
    each(&root, &mut |e| {
        if local(&e.tag) == "encryptionmethod" {
            let alg = e.attr("algorithm").unwrap_or("");
            // IDPF- och Adobe-obfuskeringen av typsnitt är inget skydd av innehållet.
            let font = alg == "http://www.idpf.org/2008/embedding" || alg == "http://ns.adobe.com/pdf/enc#RC";
            if !font {
                drm = true;
            }
        }
    });
    if drm { Err(drm_message()) } else { Ok(()) }
}

/// Läser paketet ur arkivet.
pub fn read_package(ar: &mut Archive) -> Result<Package, String> {
    check_drm(ar)?;
    let container = ar
        .read_text("META-INF/container.xml")
        .ok_or_else(|| crate::i18n::t("EPUB-filen saknar META-INF/container.xml.").to_owned())?;
    let croot = parse(&container);
    let mut opf_path = None;
    each(&croot, &mut |e| {
        if local(&e.tag) == "rootfile" && opf_path.is_none() {
            if let Some(p) = e.attr("full-path") {
                opf_path = Some(resolve("", p).0);
            }
        }
    });
    // Saknas container.xml-posten: första .opf i arkivet.
    let opf_path = opf_path
        .or_else(|| ar.names().iter().find(|n| n.to_lowercase().ends_with(".opf")).cloned())
        .ok_or_else(|| crate::i18n::t("EPUB-filen saknar paketfil (.opf).").to_owned())?;
    let opf = ar
        .read_text(&opf_path)
        .ok_or_else(|| crate::i18n::t("EPUB-filens paketfil (.opf) kan inte läsas.").to_owned())?;
    let root = parse(&opf);
    let dir = dir_of(&opf_path).to_owned();
    let mut pkg = Package::default();

    // Metadata.
    let mut cover_id = None;
    if let Some(md) = find(&root, "metadata") {
        for k in &md.kids {
            let Node::El(e) = k else { continue };
            let text = plain_text(e);
            match local(&e.tag) {
                "title" if pkg.meta.title.is_empty() => pkg.meta.title = text,
                "creator" if !text.is_empty() => pkg.meta.authors.push(text),
                "publisher" if pkg.meta.publisher.is_empty() => pkg.meta.publisher = text,
                "language" if pkg.meta.language.is_empty() => pkg.meta.language = text,
                "date" if pkg.meta.date.is_empty() => pkg.meta.date = text,
                "description" if pkg.meta.description.is_empty() => {
                    // Beskrivningen kan vara HTML som text (`&lt;p&gt;…`): tolka den en gång till.
                    let inner = if text.contains('<') { plain_text(&parse(&text)) } else { text };
                    pkg.meta.description = inner;
                }
                "meta" if e.attr("name") == Some("cover") => cover_id = e.attr("content").map(str::to_owned),
                _ => {}
            }
        }
    }

    // Manifest.
    let mut nav_path = None;
    let mut cover_prop = None;
    if let Some(man) = find(&root, "manifest") {
        for k in &man.kids {
            let Node::El(e) = k else { continue };
            if local(&e.tag) != "item" {
                continue;
            }
            let (Some(id), Some(href)) = (e.attr("id"), e.attr("href")) else { continue };
            let item = Item {
                path: resolve(&dir, href).0,
                media: e.attr("media-type").unwrap_or("").to_ascii_lowercase(),
                props: e.attr("properties").unwrap_or("").to_owned(),
            };
            if item.props.split_whitespace().any(|p| p == "nav") {
                nav_path = Some(item.path.clone());
            }
            if item.props.split_whitespace().any(|p| p == "cover-image") && cover_prop.is_none() {
                cover_prop = Some(item.path.clone());
            }
            pkg.items.insert(id.to_owned(), item);
        }
    }

    // Spine.
    let mut ncx_id = None;
    if let Some(sp) = find(&root, "spine") {
        ncx_id = sp.attr("toc").map(str::to_owned);
        for k in &sp.kids {
            let Node::El(e) = k else { continue };
            if local(&e.tag) != "itemref" {
                continue;
            }
            let Some(item) = e.attr("idref").and_then(|id| pkg.items.get(id)) else { continue };
            let linear = !e.attr("linear").is_some_and(|l| l.eq_ignore_ascii_case("no"));
            pkg.spine.push(SpineRef { path: item.path.clone(), linear });
        }
    }
    if pkg.spine.is_empty() {
        return Err(crate::i18n::t("Boken har ingen text att läsa (tom läsordning).").to_owned());
    }

    // Omslag: EPUB 3-egenskap, EPUB 2-meta, guide, sist ett namn som liknar omslag.
    let is_image = |p: &str| {
        let l = p.to_lowercase();
        [".jpg", ".jpeg", ".png", ".gif", ".webp", ".svg", ".bmp"].iter().any(|e| l.ends_with(e))
    };
    pkg.cover = cover_prop
        .or_else(|| {
            let i = pkg.items.get(cover_id.as_deref()?)?;
            (is_image(&i.path) || i.media.starts_with("image/")).then(|| i.path.clone())
        })
        .or_else(|| {
            let mut r = None;
            if let Some(g) = find(&root, "guide") {
                each(g, &mut |e| {
                    if r.is_none() && local(&e.tag) == "reference" && e.attr("type") == Some("cover") {
                        if let Some(h) = e.attr("href") {
                            let p = resolve(&dir, h).0;
                            if is_image(&p) {
                                r = Some(p);
                            }
                        }
                    }
                });
            }
            r
        })
        .or_else(|| {
            let mut c: Vec<&Item> = pkg
                .items
                .iter()
                .filter(|(id, i)| i.media.starts_with("image/") && (id.to_lowercase().contains("cover") || i.path.to_lowercase().contains("cover")))
                .map(|(_, i)| i)
                .collect();
            c.sort_by(|a, b| a.path.cmp(&b.path));
            c.first().map(|i| i.path.clone())
        })
        .filter(|p| ar.has(p));

    // Innehållsförteckning: nav först, annars NCX.
    let ncx_path = ncx_id
        .and_then(|id| pkg.items.get(&id).map(|i| i.path.clone()))
        .or_else(|| pkg.items.values().find(|i| i.media == "application/x-dtbncx+xml").map(|i| i.path.clone()));
    if let Some(np) = nav_path.as_ref().filter(|p| ar.has(p)) {
        if let Some(x) = ar.read_text(np) {
            pkg.toc = parse_nav(&x, dir_of(np));
        }
    }
    if pkg.toc.is_empty() {
        if let Some(np) = ncx_path.as_ref().filter(|p| ar.has(p)) {
            if let Some(x) = ar.read_text(np) {
                pkg.toc = parse_ncx(&x, dir_of(np));
            }
        }
    }
    if pkg.meta.title.trim().is_empty() {
        pkg.meta.title = String::new();
    }
    Ok(pkg)
}

/// EPUB 3: `<nav epub:type="toc">` med nästlade `<ol>`.
pub fn parse_nav(xhtml: &str, base_dir: &str) -> Vec<RawToc> {
    let root = parse(xhtml);
    let mut navs: Vec<&El> = Vec::new();
    each(&root, &mut |e| {
        if local(&e.tag) == "nav" {
            navs.push(e);
        }
    });
    let is_toc = |e: &&&El| e.attr("epub:type").or_else(|| e.attr("type")).is_some_and(|t| t.split_whitespace().any(|w| w == "toc"));
    let nav = navs.iter().find(is_toc).or_else(|| navs.first());
    let mut out = Vec::new();
    if let Some(nav) = nav {
        for k in &nav.kids {
            if let Node::El(e) = k {
                nav_list(e, 0, base_dir, &mut out);
            }
        }
    }
    out
}

fn nav_list(el: &El, depth: usize, base: &str, out: &mut Vec<RawToc>) {
    match local(&el.tag) {
        "ol" | "ul" => {
            for k in &el.kids {
                if let Node::El(li) = k {
                    nav_list(li, depth, base, out);
                }
            }
        }
        "li" => {
            // Etiketten: första länken eller spannet; en nästlad lista räknas inte in i titeln.
            let mut title = String::new();
            let mut target = None;
            for k in &el.kids {
                let Node::El(e) = k else { continue };
                match local(&e.tag) {
                    "a" if title.is_empty() => {
                        title = plain_text(e);
                        target = e.attr("href").filter(|h| !h.is_empty() && !has_scheme(h)).map(|h| resolve(base, h));
                    }
                    "span" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" if title.is_empty() => {
                        title = plain_text(e);
                        if let Some(Node::El(a)) = e.kids.iter().find(|n| matches!(n, Node::El(x) if local(&x.tag) == "a")) {
                            target = a.attr("href").filter(|h| !h.is_empty() && !has_scheme(h)).map(|h| resolve(base, h));
                        }
                    }
                    _ => {}
                }
            }
            if !title.is_empty() {
                out.push(RawToc { title, depth, target });
            }
            for k in &el.kids {
                if let Node::El(e) = k {
                    if matches!(local(&e.tag), "ol" | "ul") {
                        nav_list(e, depth + 1, base, out);
                    }
                }
            }
        }
        _ => {}
    }
}

/// EPUB 2: `toc.ncx` med nästlade `navPoint`.
pub fn parse_ncx(xml: &str, base_dir: &str) -> Vec<RawToc> {
    let root = parse(xml);
    let mut out = Vec::new();
    if let Some(map) = find(&root, "navmap") {
        for k in &map.kids {
            if let Node::El(e) = k {
                ncx_point(e, 0, base_dir, &mut out);
            }
        }
    }
    out
}

fn ncx_point(el: &El, depth: usize, base: &str, out: &mut Vec<RawToc>) {
    if local(&el.tag) != "navpoint" {
        return;
    }
    let mut title = String::new();
    let mut target = None;
    for k in &el.kids {
        let Node::El(e) = k else { continue };
        match local(&e.tag) {
            "navlabel" if title.is_empty() => title = plain_text(e),
            "content" if target.is_none() => {
                target = e.attr("src").filter(|h| !h.is_empty() && !has_scheme(h)).map(|h| resolve(base, h));
            }
            _ => {}
        }
    }
    out.push(RawToc { title, depth, target });
    for k in &el.kids {
        if let Node::El(e) = k {
            ncx_point(e, depth + 1, base, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sökvägar_löses_med_punkter_kodning_och_fragment() {
        assert_eq!(resolve("OEBPS/text", "../images/a%20b.png"), ("OEBPS/images/a b.png".to_owned(), None));
        assert_eq!(resolve("OEBPS", "ch1.xhtml#fn%31"), ("OEBPS/ch1.xhtml".to_owned(), Some("fn1".to_owned())));
        assert_eq!(resolve("OEBPS", "./x/./y.xhtml?q=1"), ("OEBPS/x/y.xhtml".to_owned(), None));
        assert_eq!(resolve("a/b", "/root.xhtml"), ("root.xhtml".to_owned(), None));
        assert_eq!(resolve("a", "../../../x"), ("x".to_owned(), None), "uppåt förbi roten stannar i roten");
        assert_eq!(resolve("a", "#kap"), (String::new(), Some("kap".to_owned())), "bara fragment: samma fil");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%C3%A5"), "å");
        assert!(has_scheme("https://x.se") && has_scheme("mailto:a@b") && !has_scheme("ch1.xhtml#a:b") && !has_scheme("C:"));
    }

    #[test]
    fn nav_och_ncx_ger_nästlad_förteckning() {
        let nav = r#"<html xmlns:epub="x"><body><nav epub:type="landmarks"><ol><li><a href="c.xhtml">Omslag</a></li></ol></nav>
            <nav epub:type="toc"><ol><li><a href="a.xhtml">Del ett</a><ol><li><a href="a.xhtml#s1">Avsnitt</a></li></ol></li>
            <li><span>Utan länk</span></li><li><a href="../b.xhtml">Del två</a></li></ol></nav></body></html>"#;
        let t = parse_nav(nav, "OEBPS/nav");
        let rows: Vec<(String, usize)> = t.iter().map(|r| (r.title.clone(), r.depth)).collect();
        assert_eq!(rows, [("Del ett".into(), 0), ("Avsnitt".into(), 1), ("Utan länk".into(), 0), ("Del två".into(), 0)]);
        assert_eq!(t[1].target, Some(("OEBPS/nav/a.xhtml".into(), Some("s1".into()))));
        assert_eq!(t[2].target, None);
        assert_eq!(t[3].target, Some(("OEBPS/b.xhtml".into(), None)));
        let ncx = r#"<ncx><navMap><navPoint id="1"><navLabel><text>Ett</text></navLabel><content src="a.html"/>
            <navPoint id="2"><navLabel><text>Ett.ett</text></navLabel><content src="a.html#x"/></navPoint></navPoint>
            <navPoint id="3"><navLabel><text>Två</text></navLabel><content src="b.html"/></navPoint></navMap></ncx>"#;
        let t = parse_ncx(ncx, "");
        let rows: Vec<(String, usize)> = t.iter().map(|r| (r.title.clone(), r.depth)).collect();
        assert_eq!(rows, [("Ett".into(), 0), ("Ett.ett".into(), 1), ("Två".into(), 0)]);
        assert_eq!(t[1].target, Some(("a.html".into(), Some("x".into()))));
    }

    #[test]
    fn text_avkodas_från_olika_kodningar() {
        assert_eq!(text_of_bytes(b"\xEF\xBB\xBFh\xC3\xA5"), "hå");
        assert_eq!(text_of_bytes(b"h\xE5"), "hå", "Windows-1252 som reserv");
        assert_eq!(text_of_bytes(&[0xFF, 0xFE, b'a', 0, b'b', 0]), "ab");
    }
}
