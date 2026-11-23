//! Fot- och slutnoternas talformat i .docx: `w:footnotePr`/`w:endnotePr` (`w:numFmt`, `w:numStart`) i
//! settings.xml och i avsnittens `w:sectPr`. Läsningen ger `word::notefmt::NoteFormats`; skrivningen ändrar
//! bara det som skiljer sig mellan det som lästes och det nya, så orörda dokument skrivs byte för byte.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::word::notefmt::{NoteFmt, NoteFormats, NoteStyle};

fn local(name: &[u8]) -> &[u8] {
    name.iter().rposition(|&b| b == b':').map_or(name, |i| &name[i + 1..])
}

fn attr(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| local(a.key.as_ref()) == name).map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

/// Wordformatet (`w:numFmt w:val`) som Kladd känner igen; okända (`decimalZero` …) blir `None`.
fn fmt_of(v: &str) -> Option<NoteFmt> {
    NoteFmt::ALL.into_iter().find(|f| f.name() == v)
}

/// Formaten i `xml` (settings.xml eller en `w:sectPr`): (fotnoter, slutnoter) som `(format, start)`.
fn read_one(xml: &str) -> [(Option<NoteFmt>, Option<u32>); 2] {
    let mut out = [(None, None), (None, None)];
    let mut cur: Option<usize> = None;
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match local(e.name().as_ref()) {
                b"footnotePr" => cur = Some(0),
                b"endnotePr" => cur = Some(1),
                _ => {}
            },
            Ok(Event::End(e)) if matches!(local(e.name().as_ref()), b"footnotePr" | b"endnotePr") => cur = None,
            Ok(Event::Empty(e)) => {
                if let Some(c) = cur {
                    match local(e.name().as_ref()) {
                        b"numFmt" => out[c].0 = attr(&e, b"val").as_deref().and_then(fmt_of).or(out[c].0),
                        b"numStart" => out[c].1 = attr(&e, b"val").and_then(|v| v.parse().ok()).filter(|n| *n >= 1),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Dokumentets talformat: settings.xml först, och avsnittens `w:sectPr` (det första som anger något) går före.
pub fn read(settings: Option<&str>, sects: &[&str]) -> NoteFormats {
    let mut f = NoteFormats::default();
    let mut layers: Vec<[(Option<NoteFmt>, Option<u32>); 2]> = Vec::new();
    layers.extend(settings.map(read_one));
    for s in sects {
        if s.contains("footnotePr") || s.contains("endnotePr") {
            layers.push(read_one(s));
            break;
        }
    }
    for l in layers {
        for (i, st) in [&mut f.foot, &mut f.end].into_iter().enumerate() {
            if let Some(x) = l[i].0 {
                st.fmt = x;
            }
            if let Some(n) = l[i].1 {
                st.start = n;
            }
        }
    }
    f
}

/// Sätter formatet i `w:footnotePr`/`w:endnotePr` (`end`) i `x`. `create` = lägg till elementet om det saknas
/// (i settings.xml; i en `w:sectPr` skapas det aldrig, där gäller dokumentets).
fn patch_pr(x: &str, end: bool, st: NoteStyle, create: bool) -> String {
    let tag = if end { "w:endnotePr" } else { "w:footnotePr" };
    let open = format!("<{tag}");
    let new_kids = {
        let mut k = format!("<w:numFmt w:val=\"{}\"/>", st.fmt.name());
        if st.start != 1 {
            k.push_str(&format!("<w:numStart w:val=\"{}\"/>", st.start));
        }
        k
    };
    let mut from = 0;
    while let Some(i) = x[from..].find(&open).map(|i| i + from) {
        let after = x[i + open.len()..].chars().next();
        if !matches!(after, Some('>' | '/' | ' ')) {
            from = i + open.len();
            continue;
        }
        let Some(j) = x[i..].find('>').map(|j| i + j) else { return x.to_owned() };
        let self_closing = x[..j].ends_with('/');
        let (head, inner, tail) = if self_closing {
            (format!("{}>", x[i..j - 1].trim_end()), String::new(), x[j + 1..].to_owned())
        } else {
            let close = format!("</{tag}>");
            let Some(c) = x[j..].find(&close).map(|c| j + c) else { return x.to_owned() };
            (x[i..=j].to_owned(), x[j + 1..c].to_owned(), x[c + close.len()..].to_owned())
        };
        let mut inner = strip_tag(&strip_tag(&inner, "<w:numFmt"), "<w:numStart");
        let at = inner.find("<w:pos").and_then(|p| inner[p..].find('>').map(|e| p + e + 1)).unwrap_or(0);
        inner.insert_str(at, &new_kids);
        return format!("{}{head}{inner}</{tag}>{tail}", &x[..i]);
    }
    if !create {
        return x.to_owned();
    }
    let add = format!("<{tag}>{new_kids}</{tag}>");
    // Schemat (CT_Settings): fotnoteregler före slutnoteregler, båda före `w:compat`.
    let markers: &[&str] = if end {
        &["<w:compat", "<w:docVars", "<w:rsids", "<m:mathPr", "<w:themeFontLang", "<w:clrSchemeMapping", "</w:settings>"]
    } else {
        &["<w:endnotePr", "<w:compat", "<w:docVars", "<w:rsids", "<m:mathPr", "<w:themeFontLang", "<w:clrSchemeMapping", "</w:settings>"]
    };
    match markers.iter().filter_map(|m| x.find(m)).min() {
        Some(at) => format!("{}{add}{}", &x[..at], &x[at..]),
        None => x.to_owned(),
    }
}

/// Tar bort första `<w:…/>` som börjar med `start`.
fn strip_tag(x: &str, start: &str) -> String {
    let Some(i) = x.find(start) else { return x.to_owned() };
    let Some(j) = x[i..].find('>') else { return x.to_owned() };
    format!("{}{}", &x[..i], &x[i + j + 1..])
}

/// settings.xml med de format som ändrats (`old` → `new`).
pub fn patch_settings(settings: &str, old: &NoteFormats, new: &NoteFormats) -> String {
    let mut x = settings.to_owned();
    if old.foot != new.foot {
        x = patch_pr(&x, false, new.foot, true);
    }
    if old.end != new.end {
        x = patch_pr(&x, true, new.end, true);
    }
    x
}

/// Alla `w:sectPr` i `doc` (avsnittsslut i stycken och på slutet) med de format som ändrats, där de fanns.
pub fn patch_sects(doc: &str, old: &NoteFormats, new: &NoteFormats) -> String {
    if old == new || !doc.contains("<w:sectPr") {
        return doc.to_owned();
    }
    let mut out = String::with_capacity(doc.len() + 64);
    let mut rest = doc;
    while let Some(i) = rest.find("<w:sectPr") {
        let after = rest[i + 9..].chars().next();
        let end = match rest[i..].find("</w:sectPr>") {
            Some(e) if matches!(after, Some('>' | ' ')) => i + e + "</w:sectPr>".len(),
            _ => {
                out.push_str(&rest[..i + 9]);
                rest = &rest[i + 9..];
                continue;
            }
        };
        out.push_str(&rest[..i]);
        let mut one = rest[i..end].to_owned();
        if old.foot != new.foot && one.contains("<w:footnotePr") {
            one = patch_pr(&one, false, new.foot, false);
        }
        if old.end != new.end && one.contains("<w:endnotePr") {
            one = patch_pr(&one, true, new.end, false);
        }
        out.push_str(&one);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = r#"<w:settings xmlns:w="w"><w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr><w:endnotePr><w:numFmt w:val="decimal"/><w:numStart w:val="3"/><w:endnote w:id="-1"/></w:endnotePr><w:compat/></w:settings>"#;

    #[test]
    fn reads_settings_and_sect_override() {
        let f = read(Some(SETTINGS), &[]);
        assert_eq!(f.foot, NoteFormats::default().foot);
        assert_eq!(f.end, NoteStyle { fmt: NoteFmt::Decimal, start: 3 });
        let sect = r#"<w:sectPr><w:footnotePr><w:numFmt w:val="chicago"/></w:footnotePr><w:endnotePr><w:numFmt w:val="upperLetter"/></w:endnotePr></w:sectPr>"#;
        let g = read(Some(SETTINGS), &[sect]);
        assert_eq!(g.foot.fmt, NoteFmt::Chicago);
        assert_eq!(g.end.fmt, NoteFmt::UpperLetter);
        assert_eq!(g.end.start, 3, "numStart ärvs från settings.xml");
    }

    #[test]
    fn unknown_format_keeps_default() {
        let x = r#"<w:settings xmlns:w="w"><w:footnotePr><w:numFmt w:val="decimalZero"/></w:footnotePr></w:settings>"#;
        assert_eq!(read(Some(x), &[]), NoteFormats::default());
    }

    #[test]
    fn patch_roundtrips_and_leaves_rest() {
        let old = read(Some(SETTINGS), &[]);
        let mut new = old;
        new.foot.fmt = NoteFmt::LowerRoman;
        new.end = NoteStyle { fmt: NoteFmt::Chicago, start: 1 };
        let x = patch_settings(SETTINGS, &old, &new);
        assert_eq!(read(Some(&x), &[]), new);
        assert!(x.contains(r#"<w:endnote w:id="-1"/>"#) && x.contains(r#"<w:footnote w:id="0"/>"#) && x.ends_with("<w:compat/></w:settings>"));
        assert!(!x.contains("numStart"));
        assert_eq!(patch_settings(SETTINGS, &old, &old), SETTINGS);
    }

    #[test]
    fn creates_missing_element_in_order() {
        let x = r#"<w:settings xmlns:w="w"><w:compat/></w:settings>"#;
        let mut new = NoteFormats::default();
        new.end.fmt = NoteFmt::Decimal;
        new.foot.fmt = NoteFmt::UpperRoman;
        let p = patch_settings(x, &NoteFormats::default(), &new);
        assert!(p.find("<w:footnotePr").unwrap() < p.find("<w:endnotePr").unwrap());
        assert!(p.find("<w:endnotePr").unwrap() < p.find("<w:compat").unwrap());
        assert_eq!(read(Some(&p), &[]), new);
    }

    #[test]
    fn patches_only_existing_sect_elements() {
        let doc = r#"<w:body><w:p><w:pPr><w:sectPr><w:footnotePr><w:numFmt w:val="decimal"/></w:footnotePr></w:sectPr></w:pPr></w:p><w:sectPr><w:pgSz/></w:sectPr></w:body>"#;
        let old = NoteFormats::default();
        let mut new = old;
        new.foot.fmt = NoteFmt::LowerLetter;
        let p = patch_sects(doc, &old, &new);
        assert!(p.contains(r#"w:val="lowerLetter""#));
        assert_eq!(p.matches("footnotePr").count(), 2, "inget nytt element i sista sectPr");
        assert_eq!(patch_sects(doc, &old, &old), doc);
    }
}
