//! Spårade ändringar och kommentarer i .docx: kommentarsdelen (comments.xml och
//! commentsExtended.xml) läses till poster i Kladds Markdown (`crate::review`) och byggs upp
//! igen vid sparning. Delmodul till `docx`; själva ändringarna (`w:ins`/`w:del`) och
//! kommentarsankarna hanteras i `parse_body` och `body_from_markdown`.

use super::*;
use std::collections::HashSet;

use crate::review::fmt::{CharFmt, ParaFmt};
use crate::review::{CommentRec, comment_line};
use crate::word::markup::Block as MdBlock;

pub(super) const COMMENTS_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
pub(super) const COMMENTS_EXT_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml";
const COMMENTS_EXT_REL: &str = "http://schemas.microsoft.com/office/2011/relationships/commentsExtended";
const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const W15_NS: &str = "http://schemas.microsoft.com/office/word/2012/wordml";
const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

// ------------------------------------------------------------------ spårad formatering

/// Numret på en formateringsändring: samma författare, datum och gamla format delar nummer (en Word-ändring
/// över flera körningar eller stycken godkänns och avvisas tillsammans).
pub(super) fn fmt_rev_id(info: &mut BodyInfo, mut rec: RevRec) -> u32 {
    if let Some(r) = info.revs.iter().find(|r| r.old.is_some() && r.author == rec.author && r.date == rec.date && r.old == rec.old) {
        return r.id;
    }
    rec.id = info.revs.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    let id = rec.id;
    info.revs.push(rec);
    id
}

/// Det gamla teckenformatet ur ett `w:rPrChange` (hela elementet).
pub(super) fn parse_old_rpr(xml: &str, styles: &Styles, base: &DocDefaults, para: Option<&Para>, theme: &Theme, linked: bool) -> CharFmt {
    let mut f = CharFmt::default();
    let mut rp = RProps::default();
    let mut rstyle: Option<String> = None;
    let mut code = false;
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let name = local(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"b" | b"bCs" => f.bold = on_off(&e),
                    b"i" | b"iCs" => f.italic = on_off(&e),
                    b"strike" | b"dstrike" => f.strike = on_off(&e),
                    b"u" => f.underline = on_off(&e),
                    b"rStyle" => {
                        rstyle = attr(&e, b"val");
                        let s = rstyle.clone().unwrap_or_default().to_lowercase();
                        code |= s.contains("code") || s.contains("kod");
                        f.bold |= s == "strong" || s.contains("fet");
                        f.italic |= s == "emphasis" || s.contains("betoning");
                    }
                    b"rFonts" => {
                        let face = attr(&e, b"ascii").unwrap_or_default().to_lowercase();
                        code |= ["courier", "consolas", "mono", "menlo", "monaco"].iter().any(|m| face.contains(m));
                        apply_rpr_elem(&mut rp, &name, &e, theme);
                    }
                    _ => apply_rpr_elem(&mut rp, &name, &e, theme),
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    f.span = run_span(styles, base, para, &rp, rstyle.as_deref(), linked, code);
    f
}

/// Det gamla styckeformatet ur ett `w:pPrChange` (hela elementet): slag och justering.
pub(super) fn parse_old_ppr(xml: &str, styles: &Styles, numbering: &Numbering) -> ParaFmt {
    let mut style: Option<String> = None;
    let mut outline: Option<u8> = None;
    let mut list: Option<(String, u8)> = None;
    let mut jc: Option<Align> = None;
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => match local(e.name().as_ref()) {
                b"pStyle" => style = attr(&e, b"val"),
                b"outlineLvl" => outline = attr(&e, b"val").and_then(|v| v.parse().ok()),
                b"ilvl" => {
                    let lvl = attr(&e, b"val").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0).min(8);
                    let id = list.as_ref().map(|l| l.0.clone()).unwrap_or_default();
                    list = Some((id, lvl));
                }
                b"numId" => {
                    let id = attr(&e, b"val").unwrap_or_default();
                    let lvl = list.as_ref().map_or(0, |l| l.1);
                    list = (id != "0").then_some((id, lvl));
                }
                b"jc" => jc = attr(&e, b"val").map(|v| Align::from_jc(&v)),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let mut kind = style.as_deref().map_or(ParaKind::Normal, |s| kind_for_style(&styles.map, s));
    if let Some(l) = outline.filter(|l| *l < 9) {
        kind = ParaKind::Heading((l + 1).min(6));
    }
    let list = match list {
        Some((n, l)) if n.is_empty() => style.as_deref().and_then(|s| list_for_style(&styles.map, s)).map(|(n, _)| (n, l)),
        Some(x) => Some(x),
        None => style.as_deref().and_then(|s| list_for_style(&styles.map, s)),
    };
    let block = match (kind, &list) {
        (ParaKind::Heading(n), _) => MdBlock::Heading(n),
        (_, Some((num, lvl))) => {
            if numbering.ordered.get(num).and_then(|v| v.get(usize::from(*lvl))).copied().unwrap_or(false) {
                MdBlock::Numbered
            } else {
                MdBlock::Bullet
            }
        }
        (ParaKind::Quote, None) => MdBlock::Quote,
        (ParaKind::Code, None) => MdBlock::Code,
        _ => MdBlock::Normal,
    };
    ParaFmt { block, align: jc.or_else(|| styles.jc(style.as_deref())).unwrap_or(Align::Left) }
}

/// `w:rPr`-innehållet för ett gammalt teckenformat (samma ordning som `Writer::run`).
pub(super) fn old_rpr_xml(f: &CharFmt) -> String {
    let mut rpr = String::new();
    if let Some(name) = &f.span.font_family {
        let mut n = String::new();
        xml_escape(name, &mut n);
        rpr.push_str(&format!("<w:rFonts w:ascii=\"{n}\" w:hAnsi=\"{n}\" w:cs=\"{n}\" w:eastAsia=\"{n}\"/>"));
    }
    if f.bold {
        rpr.push_str("<w:b/>");
    }
    if f.italic {
        rpr.push_str("<w:i/>");
    }
    if f.strike {
        rpr.push_str("<w:strike/>");
    }
    if let Some(c) = f.span.color {
        rpr.push_str(&format!("<w:color w:val=\"{:02X}{:02X}{:02X}\"/>", c[0], c[1], c[2]));
    }
    if let Some(pt) = f.span.font_size {
        let half = (pt * 2.0).round().max(1.0) as u32;
        rpr.push_str(&format!("<w:sz w:val=\"{half}\"/><w:szCs w:val=\"{half}\"/>"));
    }
    let shading = f.span.background.and_then(|bg| match style::highlight_name(bg) {
        Some(name) => {
            rpr.push_str(&format!("<w:highlight w:val=\"{name}\"/>"));
            None
        }
        None => Some(bg),
    });
    if f.underline {
        rpr.push_str("<w:u w:val=\"single\"/>");
    }
    if let Some(c) = shading {
        rpr.push_str(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{:02X}{:02X}{:02X}\"/>", c[0], c[1], c[2]));
    }
    rpr
}

/// `w:pPr`-innehållet för ett gammalt styckeformat. `ordered` = dokumentet har en numrerad lista att peka på
/// (numrerade listor får numId `num_base + 1`).
pub(super) fn old_ppr_xml(f: &ParaFmt, num_base: u32, ordered: bool) -> String {
    let mut s = String::new();
    match f.block {
        MdBlock::Heading(n) => s.push_str(&format!("<w:pStyle w:val=\"Heading{n}\"/>")),
        MdBlock::Quote => s.push_str("<w:pStyle w:val=\"Quote\"/>"),
        MdBlock::Code => s.push_str("<w:pStyle w:val=\"Code\"/>"),
        MdBlock::Bullet => s.push_str(&format!("<w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{num_base}\"/></w:numPr>")),
        MdBlock::Numbered => {
            s.push_str("<w:pStyle w:val=\"ListParagraph\"/>");
            if ordered {
                s.push_str(&format!("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{}\"/></w:numPr>", num_base + 1));
            }
        }
        _ => {}
    }
    if f.align != Align::Left {
        s.push_str(&format!("<w:jc w:val=\"{}\"/>", f.align.as_jc()));
    }
    s
}

/// En kommentar i originalets comments.xml: nummer, raden den gav i Markdown och hela elementet.
#[derive(Clone, Debug)]
pub(super) struct CmOrig {
    pub id: u32,
    pub rec: String,
    /// Författare, datum och text: det som ligger i elementet (svar och löst-markering ligger i
    /// commentsExtended).
    pub key: String,
    pub xml: String,
}

fn body_key(c: &CommentRec) -> String {
    format!("{}|{}|{}", c.author, c.date, c.text)
}

/// Originalets kommentarsdel: namn, allt före och efter kommentarerna och kommentarerna själva,
/// samt namnet på commentsExtended-delen om den finns.
#[derive(Clone, Debug)]
pub(super) struct CmPart {
    pub name: String,
    pub head: String,
    pub tail: String,
    pub items: Vec<CmOrig>,
    pub ext_name: Option<String>,
}

/// Det som skrivs ut: comments.xml och eventuellt commentsExtended.xml.
pub(super) struct CmOut {
    pub xml: String,
    pub ext: Option<String>,
}

struct RawComment {
    xml: String,
    id: u32,
    author: String,
    date: String,
    text: String,
    last_para: Option<String>,
}

/// Delar upp comments.xml: allt före, varje `<w:comment>` och allt efter.
fn split_comments(xml: &str) -> Option<(String, Vec<RawComment>, String)> {
    let mut r = Reader::from_str(xml);
    let mut depth = 0usize;
    let mut cur: Option<(usize, RawComment)> = None;
    let mut items: Vec<RawComment> = Vec::new();
    let mut first: Option<usize> = None;
    let mut last_end = 0usize;
    let mut close_root: Option<usize> = None;
    let mut in_t = false;
    loop {
        let before = r.buffer_position() as usize;
        let ev = r.read_event().ok()?;
        let after = r.buffer_position() as usize;
        match &ev {
            Event::Start(e) => {
                let name = local(e.name().as_ref()).to_vec();
                if depth == 1 && name == b"comment" {
                    first.get_or_insert(before);
                    let raw = RawComment {
                        xml: String::new(),
                        id: attr(e, b"id").and_then(|v| v.parse().ok()).unwrap_or(u32::MAX),
                        author: attr(e, b"author").unwrap_or_default(),
                        date: attr(e, b"date").unwrap_or_default(),
                        text: String::new(),
                        last_para: None,
                    };
                    cur = Some((before, raw));
                } else if let Some((_, c)) = cur.as_mut() {
                    match name.as_slice() {
                        b"p" => c.last_para = attr(e, b"paraId"),
                        b"t" => in_t = true,
                        _ => {}
                    }
                }
                depth += 1;
            }
            Event::Empty(e) => {
                if let Some((_, c)) = cur.as_mut() {
                    match local(e.name().as_ref()) {
                        b"tab" => c.text.push('\t'),
                        b"br" | b"cr" => c.text.push('\n'),
                        _ => {}
                    }
                }
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                let name = local(e.name().as_ref()).to_vec();
                if depth == 1 {
                    if let Some((start, mut c)) = cur.take() {
                        c.xml = xml[start..after].to_owned();
                        items.push(c);
                        last_end = after;
                    }
                } else if depth == 0 {
                    close_root = Some(before);
                } else if let Some((_, c)) = cur.as_mut() {
                    match name.as_slice() {
                        b"t" => in_t = false,
                        b"p" => c.text.push('\n'),
                        _ => {}
                    }
                }
            }
            Event::Text(_) | Event::GeneralRef(_) | Event::CData(_) => {
                if in_t {
                    if let Some((_, c)) = cur.as_mut() {
                        c.text.push_str(&text_of(&ev));
                    }
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

/// commentsExtended: styckets `paraId` → (förälderns `paraId`, löst).
fn parse_extended(xml: &str) -> HashMap<String, (Option<String>, bool)> {
    let mut out = HashMap::new();
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if local(e.name().as_ref()) == b"commentEx" => {
                if let Some(id) = attr(&e, b"paraId") {
                    out.insert(id, (attr(&e, b"paraIdParent"), matches!(attr(&e, b"done").as_deref(), Some("1" | "true"))));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Läser kommentarerna (och deras svar och löst-markering) som poster.
pub(super) fn read_comments(pkg: &mut Package<'_>, name: &str, ext_name: Option<&str>) -> Result<Option<(CmPart, Vec<CommentRec>)>, String> {
    let Some(xml) = pkg.part(name)? else { return Ok(None) };
    let Some((head, raw, tail)) = split_comments(&xml) else { return Ok(None) };
    let ext = match ext_name {
        Some(n) => pkg.part(n)?.map(|x| parse_extended(&x)).unwrap_or_default(),
        None => HashMap::new(),
    };
    let by_para: HashMap<String, u32> = raw.iter().filter_map(|c| c.last_para.clone().map(|p| (p, c.id))).collect();
    let mut recs: Vec<CommentRec> = Vec::new();
    for c in &raw {
        if c.id == u32::MAX {
            continue;
        }
        let ex = c.last_para.as_ref().and_then(|p| ext.get(p));
        let parent = ex.and_then(|(p, _)| p.as_ref()).and_then(|p| by_para.get(p)).copied().filter(|p| *p != c.id);
        recs.push(CommentRec {
            id: c.id,
            parent,
            author: c.author.clone(),
            date: c.date.clone(),
            done: ex.is_some_and(|(_, d)| *d),
            text: c.text.trim_end_matches('\n').to_owned(),
        });
    }
    // Svar på svar hör till samma tråd som roten.
    let parents: HashMap<u32, Option<u32>> = recs.iter().map(|c| (c.id, c.parent)).collect();
    for c in recs.iter_mut() {
        let mut p = c.parent;
        let mut guard = 0;
        while let Some(Some(pp)) = p.map(|x| parents.get(&x).copied().unwrap_or(None)) {
            p = Some(pp);
            guard += 1;
            if guard > 16 {
                break;
            }
        }
        c.parent = p;
    }
    let items = raw
        .into_iter()
        .filter_map(|c| {
            let rec = recs.iter().find(|r| r.id == c.id)?;
            Some(CmOrig { id: c.id, rec: comment_line(rec), key: body_key(rec), xml: c.xml })
        })
        .collect();
    Ok(Some((CmPart { name: name.to_owned(), head, tail, items, ext_name: ext_name.map(str::to_owned) }, recs)))
}

/// Namnet på commentsExtended-delen i dokumentets relationer.
pub(super) fn ext_rel_target(rels: &HashMap<String, (String, String, bool)>, dir: &str) -> Option<String> {
    rels.values().find(|(_, k, ext)| !ext && k.ends_with("/commentsExtended")).map(|(t, _, _)| resolve_target(dir, t))
}

/// Styckets `w14:paraId` (det sista i elementet) i en kommentar.
fn last_para_id(xml: &str) -> Option<String> {
    let i = xml.rfind("w14:paraId=\"")? + 12;
    let e = xml[i..].find('"')?;
    Some(xml[i..i + e].to_owned())
}

/// En ny kommentars stycke-id (8 hexsiffror under 0x80000000, som Word kräver).
fn gen_para_id(id: u32, k: usize) -> String {
    format!("{:08X}", 0x4B4C_0000u32.wrapping_add(id.wrapping_mul(32)).wrapping_add(k as u32) & 0x7FFF_FFFF)
}

fn generated_comment(c: &CommentRec, para_ids: bool) -> (String, Option<String>) {
    let initials: String = c.author.split_whitespace().filter_map(|w| w.chars().next()).take(3).collect::<String>().to_uppercase();
    let mut a = String::new();
    xml_escape(&c.author, &mut a);
    let mut d = String::new();
    xml_escape(&c.date, &mut d);
    let mut ini = String::new();
    xml_escape(&initials, &mut ini);
    let date = if d.is_empty() { String::new() } else { format!(" w:date=\"{d}\"") };
    let mut x = format!("<w:comment w:id=\"{}\" w:author=\"{a}\"{date} w:initials=\"{ini}\">", c.id);
    let lines: Vec<&str> = if c.text.is_empty() { vec![""] } else { c.text.split('\n').collect() };
    let mut last = None;
    for (k, line) in lines.iter().enumerate() {
        let pid = para_ids.then(|| gen_para_id(c.id, k));
        let attr = pid.as_ref().map(|p| format!(" w14:paraId=\"{p}\"")).unwrap_or_default();
        x.push_str(&format!("<w:p{attr}><w:pPr><w:pStyle w:val=\"CommentText\"/></w:pPr>"));
        if k == 0 {
            x.push_str("<w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:annotationRef/></w:r>");
        }
        if !line.is_empty() {
            let mut t = String::new();
            xml_escape(line, &mut t);
            x.push_str(&format!("<w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r>"));
        }
        x.push_str("</w:p>");
        last = pid;
    }
    x.push_str("</w:comment>");
    (x, last)
}

fn fresh_head() -> String {
    format!(
        "{XML_DECL}\n<w:comments xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\" xmlns:mc=\"{MC_NS}\" xmlns:w14=\"{W14_NS}\" xmlns:w15=\"{W15_NS}\" mc:Ignorable=\"w14 w15\">"
    )
}

/// Bygger kommentarsdelarna av kommentarerna i texten. `anchored` är numren som har ett område
/// i den skrivna brödtexten (kommentarer utan område tas bort). `None` när originalets delar
/// kan följa med orörda (alla kommentarer oförändrade) eller det inte finns några kommentarer.
pub(super) fn build_comments(orig: Option<&CmPart>, comments: &[CommentRec], anchored: &HashSet<u32>) -> Option<CmOut> {
    let keep: Vec<&CommentRec> = comments.iter().filter(|c| anchored.contains(&c.id)).collect();
    match orig {
        None if keep.is_empty() => return None,
        Some(o) => {
            let same = o.items.len() == keep.len() && o.items.iter().zip(&keep).all(|(i, c)| i.id == c.id && i.rec == comment_line(c));
            if same {
                return None;
            }
        }
        None => {}
    }
    let head = match orig {
        Some(o) if o.head.contains("<w:comments") => {
            // w14:paraId kräver namnrymden.
            if o.head.contains("xmlns:w14=") {
                o.head.clone()
            } else {
                o.head.replacen("<w:comments", &format!("<w:comments xmlns:w14=\"{W14_NS}\""), 1)
            }
        }
        _ => fresh_head(),
    };
    let tail = orig.map_or("</w:comments>".to_owned(), |o| o.tail.clone());
    let mut xml = head;
    // Stycke-id (för commentsExtended) per kommentar.
    let mut para: HashMap<u32, String> = HashMap::new();
    for c in &keep {
        let o = orig.and_then(|o| o.items.iter().find(|i| i.id == c.id && i.key == body_key(c)));
        match o {
            Some(i) => {
                if let Some(p) = last_para_id(&i.xml) {
                    para.insert(c.id, p);
                }
                xml.push_str(&i.xml);
            }
            None => {
                let (x, pid) = generated_comment(c, true);
                if let Some(p) = pid {
                    para.insert(c.id, p);
                }
                xml.push_str(&x);
            }
        }
    }
    xml.push_str(&tail);
    let want_ext = keep.iter().any(|c| c.parent.is_some() || c.done) || orig.is_some_and(|o| o.ext_name.is_some());
    let ext = want_ext.then(|| {
        let mut x = format!("{XML_DECL}\n<w15:commentsEx xmlns:w15=\"{W15_NS}\" xmlns:mc=\"{MC_NS}\" mc:Ignorable=\"w15\">");
        for c in &keep {
            let Some(pid) = para.get(&c.id) else { continue };
            let parent = c.parent.and_then(|p| para.get(&p)).map(|p| format!(" w15:paraIdParent=\"{p}\"")).unwrap_or_default();
            x.push_str(&format!("<w15:commentEx w15:paraId=\"{pid}\"{parent} w15:done=\"{}\"/>", u8::from(c.done)));
        }
        x.push_str("</w15:commentsEx>");
        x
    });
    Some(CmOut { xml, ext })
}

/// Relationen och innehållstypen för commentsExtended.
pub(super) fn ext_rel_xml(rid: &str, target: &str) -> String {
    format!("<Relationship Id=\"{rid}\" Type=\"{COMMENTS_EXT_REL}\" Target=\"{target}\"/>")
}

/// Numren i `w:commentRangeStart` i en bit brödtext-XML.
pub(super) fn anchored_ids(body: &str) -> HashSet<u32> {
    let mut out = HashSet::new();
    let mut rest = body;
    while let Some(i) = rest.find("<w:commentRangeStart ") {
        rest = &rest[i + 21..];
        if let Some(j) = rest.find("w:id=\"") {
            let v = &rest[j + 6..];
            if let Some(e) = v.find('"') {
                if let Ok(n) = v[..e].parse() {
                    out.insert(n);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t xml:space="preserve">Hej </w:t></w:r><w:ins w:id="1" w:author="Ada" w:date="2026-05-01T10:00:00Z"><w:r><w:t>snälla </w:t></w:r></w:ins><w:commentRangeStart w:id="0"/><w:commentRangeStart w:id="1"/><w:r><w:t>världen</w:t></w:r><w:commentRangeEnd w:id="0"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="0"/></w:r><w:commentRangeEnd w:id="1"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="1"/></w:r><w:del w:id="2" w:author="Bo" w:date="2026-05-02T10:00:00Z"><w:r><w:delText xml:space="preserve"> idag</w:delText></w:r></w:del><w:r><w:t>.</w:t></w:r></w:p></w:body></w:document>"#;
    const COMMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><w:comment w:id="0" w:author="Ada" w:date="2026-05-01T11:00:00Z" w:initials="A"><w:p w14:paraId="00000010"><w:r><w:annotationRef/></w:r><w:r><w:t>Skriv om</w:t></w:r></w:p></w:comment><w:comment w:id="1" w:author="Bo" w:date="2026-05-01T12:00:00Z" w:initials="B"><w:p w14:paraId="00000020"><w:r><w:t>Klart</w:t></w:r></w:p></w:comment></w:comments>"#;
    const EXT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w15:commentsEx xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml"><w15:commentEx w15:paraId="00000010" w15:done="0"/><w15:commentEx w15:paraId="00000020" w15:paraIdParent="00000010" w15:done="1"/></w15:commentsEx>"#;

    fn package() -> Vec<u8> {
        let rel = |id: &str, kind: &str, target: &str| format!(r#"<Relationship Id="{id}" Type="{kind}" Target="{target}"/>"#);
        let rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}{}</Relationships>"#,
            rel("rId8", "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments", "comments.xml"),
            rel("rId9", COMMENTS_EXT_REL, "commentsExtended.xml"),
        );
        let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
        write_zip(vec![
            ("[Content_Types].xml".into(), types.as_bytes().to_vec()),
            ("_rels/.rels".into(), root.as_bytes().to_vec()),
            ("word/document.xml".into(), DOC.as_bytes().to_vec()),
            ("word/_rels/document.xml.rels".into(), rels.into_bytes()),
            ("word/comments.xml".into(), COMMENTS.as_bytes().to_vec()),
            ("word/commentsExtended.xml".into(), EXT.as_bytes().to_vec()),
        ])
        .unwrap()
    }

    fn part(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    fn has_part(bytes: &[u8], name: &str) -> bool {
        zip::ZipArchive::new(Cursor::new(bytes)).unwrap().by_name(name).is_ok()
    }

    #[test]
    fn read_gives_markers_and_records() {
        let l = read(&package()).unwrap();
        let md = &l.markdown;
        assert!(md.contains("Hej <ins data-k=\"1\">snälla </ins><!--k:c:0--><!--k:c:1-->världen<!--k:/c:0--><!--k:/c:1--><del data-k=\"2\"> idag</del>."), "{md}");
        assert!(md.contains("<!--k:r id=\"1\" author=\"Ada\" date=\"2026-05-01T10:00:00Z\"-->"), "{md}");
        assert!(md.contains("<!--k:r id=\"2\" author=\"Bo\""), "{md}");
        assert!(md.contains("<!--k:c id=\"0\" author=\"Ada\" date=\"2026-05-01T11:00:00Z\" text=\"Skriv om\"-->"), "{md}");
        assert!(md.contains("<!--k:c id=\"1\" parent=\"0\" author=\"Bo\" date=\"2026-05-01T12:00:00Z\" done=\"1\" text=\"Klart\"-->"), "{md}");
        assert!(!l.dropped.iter().any(|d| d.contains("spårade")), "{:?}", l.dropped);
        // Kommentarsreferensen är inte längre en markör.
        assert!(!md.contains("<!--k:0-->"), "{md}");
    }

    #[test]
    fn untouched_document_round_trips_losslessly() {
        let bytes = package();
        let l = read(&bytes).unwrap();
        let w = write(&l.markdown, "t", &|_| None, l.source.as_ref()).unwrap();
        for name in ["word/comments.xml", "word/commentsExtended.xml"] {
            assert_eq!(part(&w.bytes, name), part(&bytes, name), "{name}");
        }
        let para = &DOC[DOC.find("<w:p>").unwrap()..DOC.find("</w:p>").unwrap() + 6];
        assert!(part(&w.bytes, "word/document.xml").contains(para));
    }

    #[test]
    fn rewritten_paragraph_keeps_changes_and_comments() {
        let l = read(&package()).unwrap();
        let md = l.markdown.replacen("Hej ", "Hallå ", 1);
        let w = write(&md, "t", &|_| None, l.source.as_ref()).unwrap();
        let doc = part(&w.bytes, "word/document.xml");
        assert!(doc.contains("<w:ins ") && doc.contains("w:author=\"Ada\"") && doc.contains("w:date=\"2026-05-01T10:00:00Z\""), "{doc}");
        assert!(doc.contains("<w:del ") && doc.contains("w:author=\"Bo\""), "{doc}");
        assert!(doc.contains("<w:delText xml:space=\"preserve\"> idag</w:delText>"), "{doc}");
        assert!(doc.contains("<w:commentRangeStart w:id=\"0\"/>") && doc.contains("<w:commentRangeEnd w:id=\"0\"/>"), "{doc}");
        assert!(doc.contains("<w:commentReference w:id=\"0\"/>"), "{doc}");
        // Kommentarerna är oförändrade: delarna följer med orörda.
        assert_eq!(part(&w.bytes, "word/comments.xml"), COMMENTS);
        // Läser man tillbaka får man samma ändringar.
        let back = read(&w.bytes).unwrap();
        assert!(back.markdown.contains("Hallå <ins data-k=\"1\">snälla </ins><!--k:c:0--><!--k:c:1-->världen<!--k:/c:0--><!--k:/c:1--><del data-k=\"2\"> idag</del>."), "{}", back.markdown);
    }

    #[test]
    fn new_comment_reply_and_done_are_written() {
        let l = read(&package()).unwrap();
        let (e, id) = crate::review::add_comment(&l.markdown, 0..3, "Cy", "2026-06-01T09:00:00Z", "Ny \"kommentar\" & mer");
        let (e2, _) = crate::review::add_reply(&e.text, id, "Di", "2026-06-01T10:00:00Z", "Svar").unwrap();
        let md = crate::review::edit_comment(&e2.text, 0, |c| c.done = true);
        let w = write(&md, "t", &|_| None, l.source.as_ref()).unwrap();
        let cm = part(&w.bytes, "word/comments.xml");
        assert!(cm.contains("w:author=\"Cy\"") && cm.contains("Ny &quot;kommentar&quot; &amp; mer"), "{cm}");
        assert!(cm.contains("w:author=\"Di\"") && cm.contains("Svar"), "{cm}");
        // Originalkommentarerna behåller sin XML.
        assert!(cm.contains("<w:p w14:paraId=\"00000010\">"), "{cm}");
        let ext = part(&w.bytes, "word/commentsExtended.xml");
        assert!(ext.contains("w15:paraId=\"00000010\" w15:done=\"1\""), "{ext}");
        assert!(ext.contains("w15:paraId=\"00000020\" w15:paraIdParent=\"00000010\""), "{ext}");
        assert_eq!(ext.matches("paraIdParent=").count(), 2, "{ext}");
        let back = read(&w.bytes).unwrap();
        let th = crate::review::threads(&back.markdown);
        assert_eq!(th.len(), 2, "{}", back.markdown);
        let t0 = th.iter().find(|t| t.root.id == 0).unwrap();
        assert!(t0.root.done && t0.replies.len() == 1);
        let tn = th.iter().find(|t| t.root.author == "Cy").unwrap();
        assert_eq!(tn.root.text, "Ny \"kommentar\" & mer");
        assert_eq!(tn.replies[0].text, "Svar");
    }

    #[test]
    fn deleting_a_comment_removes_it_from_the_package() {
        let l = read(&package()).unwrap();
        let md = crate::review::delete_comment(&l.markdown, 0);
        let w = write(&md, "t", &|_| None, l.source.as_ref()).unwrap();
        let doc = part(&w.bytes, "word/document.xml");
        assert!(!doc.contains("commentRangeStart") && !doc.contains("commentReference"), "{doc}");
        assert!(!part(&w.bytes, "word/comments.xml").contains("<w:comment "));
    }

    #[test]
    fn typing_with_tracking_writes_w_ins() {
        let l = read(&package()).unwrap();
        let t = crate::review::track_edit(&l.markdown, &l.markdown.replacen("Hej ", "Hej alla ", 1), 4..4, "Cy", "2026-06-01T09:00:00Z").unwrap();
        let w = write(&t.text, "t", &|_| None, l.source.as_ref()).unwrap();
        let doc = part(&w.bytes, "word/document.xml");
        assert!(doc.contains("w:author=\"Cy\"") && doc.contains("alla "), "{doc}");
        let back = read(&w.bytes).unwrap();
        assert!(back.markdown.contains("<ins data-k=\"1\">alla </ins>"), "{}", back.markdown);
        // Avvisa/godkänn ger den vanliga texten.
        assert!(crate::review::accept_all(&back.markdown).starts_with("Hej alla snälla <!--k:c:0-->"), "{}", crate::review::accept_all(&back.markdown));
        assert!(crate::review::reject_all(&back.markdown).starts_with("Hej <!--k:c:0--><!--k:c:1-->världen<!--k:/c:0--><!--k:/c:1--> idag."), "{}", crate::review::reject_all(&back.markdown));
    }

    #[test]
    fn fresh_documents_can_have_comments_and_changes() {
        let (e, _) = crate::review::add_comment("En text med ord.\n", 3..8, "Ada", "2026-06-01T09:00:00Z", "Hej");
        let t = crate::review::track_edit(&e.text, &e.text.replacen("En ", "En ny ", 1), 3..3, "Ada", "2026-06-01T09:00:00Z").unwrap();
        let w = write(&t.text, "t", &|_| None, None).unwrap();
        assert!(has_part(&w.bytes, "word/comments.xml"));
        let ct = part(&w.bytes, "[Content_Types].xml");
        assert!(ct.contains("/word/comments.xml"), "{ct}");
        let rels = part(&w.bytes, "word/_rels/document.xml.rels");
        assert!(rels.contains("relationships/comments\" Target=\"comments.xml\""), "{rels}");
        let doc = part(&w.bytes, "word/document.xml");
        assert!(doc.contains("<w:ins ") && doc.contains("<w:commentRangeStart"), "{doc}");
        let back = read(&w.bytes).unwrap();
        assert_eq!(crate::review::threads(&back.markdown).len(), 1, "{}", back.markdown);
    }
}
