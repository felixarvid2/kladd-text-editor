//! Styckeutseende i `.docx`: ramar (`w:pBdr`), skuggning (`w:shd`), avstånd (`w:spacing`) och
//! indrag (`w:ind`) läses ur stycket, dess formatmall (med `w:basedOn`) och `w:docDefaults`
//! till `word::pbox::PPr`, och skrivs tillbaka för stycken som Kladd skapar.
//!
//! Orörda stycken skrivs alltid tillbaka med sin ursprungliga XML (se `docx::write`); det här
//! gäller bara det som läses in i dialekten (`<!--k:pp …-->`) och nya stycken.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::word::pbox::{Border, LineRule, LineStyle, PPr};

fn local(name: &[u8]) -> &[u8] {
    name.iter().rposition(|&b| b == b':').map_or(name, |i| &name[i + 1..])
}

fn attr(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| local(a.key.as_ref()) == name).map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn tw(e: &BytesStart<'_>, name: &[u8]) -> Option<f32> {
    attr(e, name).and_then(|v| v.trim().parse::<f32>().ok()).map(|v| v / 20.0)
}

fn on(e: &BytesStart<'_>) -> bool {
    !matches!(attr(e, b"val").as_deref(), Some("0" | "false" | "off"))
}

fn hex_color(v: &str) -> Option<[u8; 3]> {
    if v.len() != 6 || v.eq_ignore_ascii_case("auto") || !v.is_ascii() {
        return None;
    }
    let n = u32::from_str_radix(v, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

fn border(e: &BytesStart<'_>) -> Border {
    let style = LineStyle::from_word(attr(e, b"val").as_deref().unwrap_or("single"));
    Border {
        style,
        sz: attr(e, b"sz").and_then(|v| v.parse().ok()).unwrap_or(4),
        space: attr(e, b"space").and_then(|v| v.parse::<f32>().ok()).map_or(0, |v| v.round().clamp(0.0, 31.0) as u16),
        color: attr(e, b"color").as_deref().and_then(hex_color),
    }
}

/// Läser ett `w:pPr`-element: `r` står strax efter dess starttagg och läser till sluttaggen.
/// Spårade gamla format (`w:pPrChange`) och teckenegenskaper (`w:rPr`) hoppas över.
fn parse_ppr(r: &mut Reader<&[u8]>) -> PPr {
    let mut pp = PPr::default();
    let mut in_bdr = false;
    loop {
        let (e, empty) = match r.read_event() {
            Ok(Event::Start(e)) => (e.into_owned(), false),
            Ok(Event::Empty(e)) => (e.into_owned(), true),
            Ok(Event::End(e)) => {
                match local(e.name().as_ref()) {
                    b"pBdr" => in_bdr = false,
                    b"pPr" => break,
                    _ => {}
                }
                continue;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => continue,
        };
        let qn = e.name();
        let name = local(qn.as_ref());
        match name {
            b"pPrChange" | b"rPr" | b"numPr" | b"sectPr" | b"tabs" | b"framePr" => {
                if !empty {
                    let _ = r.read_to_end(qn.clone());
                }
            }
            b"pBdr" => in_bdr = !empty,
            b"top" if in_bdr => pp.borders.top = Some(border(&e)),
            b"left" | b"start" if in_bdr => pp.borders.left = Some(border(&e)),
            b"bottom" if in_bdr => pp.borders.bottom = Some(border(&e)),
            b"right" | b"end" if in_bdr => pp.borders.right = Some(border(&e)),
            b"between" if in_bdr => pp.borders.between = Some(border(&e)),
            b"shd" => {
                let val = attr(&e, b"val").unwrap_or_default();
                let fill = attr(&e, b"fill").as_deref().and_then(hex_color);
                let color = attr(&e, b"color").as_deref().and_then(hex_color);
                pp.shd = match val.as_str() {
                    "nil" => None,
                    "solid" => color.or(fill),
                    _ => fill,
                };
            }
            b"spacing" => {
                let auto = |k: &[u8]| attr(&e, k).is_some_and(|v| v == "1" || v == "true");
                if auto(b"beforeAutospacing") {
                    pp.before = Some(14.0);
                } else if let Some(v) = tw(&e, b"before") {
                    pp.before = Some(v);
                }
                if auto(b"afterAutospacing") {
                    pp.after = Some(14.0);
                } else if let Some(v) = tw(&e, b"after") {
                    pp.after = Some(v);
                }
                if let Some(l) = attr(&e, b"line").and_then(|v| v.parse::<f32>().ok()).filter(|l| *l > 0.0) {
                    pp.line = Some(match attr(&e, b"lineRule").as_deref() {
                        Some("exact") => LineRule::Exact(l / 20.0),
                        Some("atLeast") => LineRule::AtLeast(l / 20.0),
                        _ => LineRule::Auto(l / 240.0),
                    });
                }
            }
            b"ind" => {
                if let Some(v) = tw(&e, b"left").or_else(|| tw(&e, b"start")) {
                    pp.left = Some(v);
                }
                if let Some(v) = tw(&e, b"right").or_else(|| tw(&e, b"end")) {
                    pp.right = Some(v);
                }
                if let Some(v) = tw(&e, b"hanging") {
                    pp.first = Some(-v);
                } else if let Some(v) = tw(&e, b"firstLine") {
                    pp.first = Some(v);
                }
            }
            b"contextualSpacing" => pp.ctx = Some(on(&e)),
            _ => {}
        }
    }
    pp
}

/// Stycketaggens egna `w:pPr` (det som står direkt på stycket). `xml` är stycket (`<w:p>…</w:p>`).
pub fn direct_ppr(xml: &str) -> PPr {
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) if local(e.name().as_ref()) == b"pPr" => return parse_ppr(&mut r),
            // Första körningen eller innehållet: stycket har ingen `w:pPr`.
            Ok(Event::Start(e) | Event::Empty(e)) if matches!(local(e.name().as_ref()), b"r" | b"hyperlink" | b"ins" | b"del" | b"bookmarkStart") => {
                return PPr::default();
            }
            Ok(Event::Eof) | Err(_) => return PPr::default(),
            _ => {}
        }
    }
}

/// Teckenstorleken (punkter) som stycket anger på sitt stycketecken (`w:pPr/w:rPr/w:sz`).
pub fn mark_size(xml: &str) -> Option<f32> {
    let mut r = Reader::from_str(xml);
    let (mut in_ppr, mut in_rpr) = (false, false);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match local(e.name().as_ref()) {
                b"pPr" => in_ppr = true,
                b"rPr" if in_ppr => in_rpr = true,
                b"pPrChange" => {
                    let _ = r.read_to_end(e.name());
                }
                b"r" => return None,
                _ => {}
            },
            Ok(Event::Empty(e)) if in_rpr && local(e.name().as_ref()) == b"sz" => {
                return attr(&e, b"val").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 2.0);
            }
            Ok(Event::End(e)) => match local(e.name().as_ref()) {
                b"rPr" => in_rpr = false,
                b"pPr" => return None,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// Styckeegenskaperna i `styles.xml`: dokumentets standard och varje styckemall.
#[derive(Default, Clone, Debug)]
pub struct ParaStyles {
    defaults: PPr,
    map: HashMap<String, (Option<String>, PPr)>,
    default_id: String,
}

impl ParaStyles {
    pub fn parse(xml: &str) -> ParaStyles {
        let mut out = ParaStyles::default();
        let mut r = Reader::from_str(xml);
        // (id, basedOn, egenskaper, är styckemall, är standard)
        let mut cur: Option<(String, Option<String>, PPr, bool, bool)> = None;
        let mut in_defaults = false;
        loop {
            match r.read_event() {
                Ok(Event::Start(e)) => {
                    let qn = e.name();
                    match local(qn.as_ref()) {
                        b"pPrDefault" => in_defaults = true,
                        b"style" => {
                            let para = attr(&e, b"type").is_none_or(|t| t == "paragraph");
                            let dflt = attr(&e, b"default").is_some_and(|d| d == "1" || d == "true");
                            cur = Some((attr(&e, b"styleId").unwrap_or_default(), None, PPr::default(), para, dflt));
                        }
                        b"pPr" => {
                            let pp = parse_ppr(&mut r);
                            if in_defaults {
                                out.defaults = pp;
                            } else if let Some(c) = cur.as_mut() {
                                c.2 = pp;
                            }
                        }
                        // Teckenegenskaper och tabellformat har egna `pPr`/`spacing` som inte gäller stycken.
                        b"rPr" | b"rPrDefault" | b"tblPr" | b"tblStylePr" => {
                            let _ = r.read_to_end(qn.clone());
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    if local(e.name().as_ref()) == b"basedOn" {
                        if let Some(c) = cur.as_mut() {
                            c.1 = attr(&e, b"val");
                        }
                    }
                }
                Ok(Event::End(e)) => match local(e.name().as_ref()) {
                    b"pPrDefault" => in_defaults = false,
                    b"style" => {
                        if let Some((id, based, pp, para, dflt)) = cur.take() {
                            if para {
                                if dflt {
                                    out.default_id = id.clone();
                                }
                                out.map.insert(id, (based, pp));
                            }
                        }
                    }
                    _ => {}
                },
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        out
    }

    /// Standardvärden + mallkedjan + stycket självt.
    pub fn resolve(&self, style: Option<&str>, direct: &PPr) -> PPr {
        let id = style.unwrap_or(if self.default_id.is_empty() { "Normal" } else { &self.default_id });
        let mut chain: Vec<&PPr> = Vec::new();
        let mut cur = Some(id);
        while let Some(sid) = cur {
            if chain.len() >= 10 {
                break;
            }
            let Some((based, pp)) = self.map.get(sid) else { break };
            chain.push(pp);
            cur = based.as_deref();
        }
        // Standardstycket är roten också när mallen inte säger det.
        let mut acc = self.defaults.clone();
        for pp in chain.into_iter().rev() {
            acc = pp.over(&acc);
        }
        direct.over(&acc).finish()
    }
}

/// Det som hamnar i dialektens markör för ett stycke: ram och skuggning alltid; avstånd och
/// indrag för vanliga brödtextstycken när de avviker från det layouten själv utgår från.
/// `plain` = ett vanligt stycke (inte rubrik, lista, citat eller kod).
pub fn for_marker(eff: &PPr, plain: bool) -> PPr {
    use crate::word::pbox::{BASE_AFTER, BASE_LINE};
    let mut out = PPr { borders: eff.borders, shd: eff.shd, ..Default::default() };
    if !plain {
        return out;
    }
    let before = eff.before.unwrap_or(0.0);
    let after = eff.after.unwrap_or(0.0);
    let line_differs = match eff.line {
        // Words standard (1,08) och layoutens 1,15 ser likadana ut.
        Some(LineRule::Auto(m)) => (m - BASE_LINE).abs() > 0.1,
        Some(_) => true,
        None => false,
    };
    let ctx = eff.ctx == Some(true);
    // Anger varken dokumentet, mallen eller stycket något avstånd lämnas det till layouten.
    let specified = eff.before.is_some() || eff.after.is_some() || eff.line.is_some() || ctx;
    if specified && (before.abs() > 0.5 || (after - BASE_AFTER).abs() > 0.5 || line_differs || ctx) {
        out.before = Some(before);
        out.after = Some(after);
        out.line = eff.line.filter(|_| line_differs);
        out.ctx = ctx.then_some(true);
    }
    let (l, r, f) = (eff.left.unwrap_or(0.0), eff.right.unwrap_or(0.0), eff.first.unwrap_or(0.0));
    if l.abs() > 0.5 || r.abs() > 0.5 || f.abs() > 0.5 {
        out.left = Some(l);
        out.right = Some(r);
        out.first = Some(f);
    }
    out
}

// ------------------------------------------------------------------ skrivning

fn side_xml(tag: &str, b: &Border) -> String {
    let color = b.color.map_or("auto".to_owned(), crate::word::pbox::hex);
    format!("<w:{tag} w:val=\"{}\" w:sz=\"{}\" w:space=\"{}\" w:color=\"{color}\"/>", b.style.word(), b.sz.max(2), b.space)
}

/// Barnen i ett `w:pPr` i schemaordning, med `pp` inlagt (ersätter det som redan finns av
/// samma slag). Styckets övriga barn (`pStyle`, `numPr`, `jc` …) behålls.
pub fn merge_ppr(inner: &str, pp: &PPr) -> String {
    // Schemaordning (ECMA-376 CT_PPrBase), de vi känner igen.
    const ORDER: [&str; 12] = ["pStyle", "keepNext", "keepLines", "pageBreakBefore", "numPr", "pBdr", "shd", "tabs", "spacing", "ind", "contextualSpacing", "jc"];
    let mut slots: Vec<String> = vec![String::new(); ORDER.len()];
    let mut tail = String::new();
    let mut rest = inner;
    while let Some(i) = rest.find("<w:") {
        let name_end = rest[i + 3..].find(|c: char| c == ' ' || c == '/' || c == '>').map_or(rest.len(), |j| i + 3 + j);
        let name = &rest[i + 3..name_end];
        let elem_end = if let Some(close) = rest[i..].find('>') {
            let selfclosing = rest[i..i + close].ends_with('/');
            if selfclosing { i + close + 1 } else { rest[i..].find(&format!("</w:{name}>")).map_or(rest.len(), |j| i + j + name.len() + 5) }
        } else {
            rest.len()
        };
        let xml = &rest[i..elem_end];
        match ORDER.iter().position(|o| *o == name) {
            Some(k) => slots[k].push_str(xml),
            None => tail.push_str(xml),
        }
        rest = &rest[elem_end..];
    }
    let pos = |n: &str| ORDER.iter().position(|o| *o == n).unwrap_or(0);
    if pp.borders.any() {
        let mut x = String::from("<w:pBdr>");
        for (tag, b) in [("top", pp.borders.top), ("left", pp.borders.left), ("bottom", pp.borders.bottom), ("right", pp.borders.right), ("between", pp.borders.between)] {
            if let Some(b) = b {
                x.push_str(&side_xml(tag, &b));
            }
        }
        x.push_str("</w:pBdr>");
        slots[pos("pBdr")] = x;
    }
    if let Some(c) = pp.shd {
        slots[pos("shd")] = format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>", crate::word::pbox::hex(c));
    }
    if pp.before.is_some() || pp.after.is_some() || pp.line.is_some() {
        let t = |v: f32| (v * 20.0).round() as i64;
        let mut x = String::from("<w:spacing");
        if let Some(v) = pp.before {
            x.push_str(&format!(" w:before=\"{}\"", t(v)));
        }
        if let Some(v) = pp.after {
            x.push_str(&format!(" w:after=\"{}\"", t(v)));
        }
        match pp.line {
            Some(LineRule::Auto(m)) => x.push_str(&format!(" w:line=\"{}\" w:lineRule=\"auto\"", (m * 240.0).round() as i64)),
            Some(LineRule::Exact(p)) => x.push_str(&format!(" w:line=\"{}\" w:lineRule=\"exact\"", t(p))),
            Some(LineRule::AtLeast(p)) => x.push_str(&format!(" w:line=\"{}\" w:lineRule=\"atLeast\"", t(p))),
            None => {}
        }
        x.push_str("/>");
        slots[pos("spacing")] = x;
    }
    if pp.has_indent() {
        let t = |v: f32| (v * 20.0).round() as i64;
        let mut x = format!("<w:ind w:left=\"{}\" w:right=\"{}\"", t(pp.left.unwrap_or(0.0)), t(pp.right.unwrap_or(0.0)));
        match pp.first {
            Some(f) if f < 0.0 => x.push_str(&format!(" w:hanging=\"{}\"", t(-f))),
            Some(f) if f > 0.0 => x.push_str(&format!(" w:firstLine=\"{}\"", t(f))),
            _ => {}
        }
        x.push_str("/>");
        slots[pos("ind")] = x;
    }
    if pp.ctx == Some(true) {
        slots[pos("contextualSpacing")] = "<w:contextualSpacing/>".to_owned();
    }
    let mut out: String = slots.concat();
    out.push_str(&tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::pbox::Borders;

    const STYLES: &str = r#"<w:styles xmlns:w="w"><w:docDefaults><w:rPrDefault><w:rPr><w:spacing w:val="40"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="200" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Ruta"><w:basedOn w:val="Normal"/><w:pPr><w:pBdr><w:top w:val="single" w:sz="8" w:space="4" w:color="FF0000"/></w:pBdr><w:shd w:val="clear" w:fill="EEEEEE"/><w:spacing w:before="120" w:after="0"/><w:ind w:left="720" w:hanging="360"/></w:pPr><w:rPr><w:spacing w:val="20"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Ruta2"><w:basedOn w:val="Ruta"/><w:pPr><w:spacing w:after="240"/><w:contextualSpacing/></w:pPr></w:style>
<w:style w:type="table" w:styleId="T"><w:pPr><w:spacing w:after="999"/></w:pPr></w:style></w:styles>"#;

    #[test]
    fn style_chain_and_defaults() {
        let s = ParaStyles::parse(STYLES);
        let n = s.resolve(None, &PPr::default());
        assert_eq!((n.before, n.after), (None, Some(10.0)));
        assert_eq!(n.line, Some(LineRule::Auto(1.15)));
        let r = s.resolve(Some("Ruta"), &PPr::default());
        assert_eq!((r.before, r.after), (Some(6.0), Some(0.0)));
        assert_eq!(r.shd, Some([0xEE; 3]));
        assert_eq!(r.borders.top.map(|b| (b.sz, b.space, b.color)), Some((8, 4, Some([255, 0, 0]))));
        assert_eq!((r.left, r.first), (Some(36.0), Some(-18.0)));
        assert_eq!(r.line, Some(LineRule::Auto(1.15)), "docDefaults gäller under mallen");
        let r2 = s.resolve(Some("Ruta2"), &PPr::default());
        assert_eq!((r2.after, r2.ctx, r2.before), (Some(12.0), Some(true), Some(6.0)));
        assert!(r2.borders.top.is_some(), "ärvs");
        // tabellmallens spacing hör inte till stycken
        assert_eq!(s.resolve(Some("T"), &PPr::default()).after, Some(10.0));
    }

    #[test]
    fn direct_overrides_and_skips_changes() {
        let s = ParaStyles::parse(STYLES);
        let p = r#"<w:p><w:pPr><w:pStyle w:val="Ruta"/><w:pBdr><w:top w:val="nil"/><w:left w:val="double" w:sz="12" w:space="2"/></w:pBdr><w:spacing w:after="60" w:line="300" w:lineRule="exact"/><w:rPr><w:shd w:val="clear" w:fill="00FF00"/></w:rPr><w:pPrChange w:id="1"><w:pPr><w:shd w:val="clear" w:fill="111111"/></w:pPr></w:pPrChange></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#;
        let d = direct_ppr(p);
        assert_eq!(d.shd, None, "rPr och pPrChange räknas inte");
        let e = s.resolve(Some("Ruta"), &d);
        assert!(e.borders.top.is_none(), "nil släcker mallens kant");
        assert_eq!(e.borders.left.map(|b| (b.style, b.sz)), Some((LineStyle::Double, 12)));
        assert_eq!((e.after, e.line), (Some(3.0), Some(LineRule::Exact(15.0))));
        assert_eq!(e.shd, Some([0xEE; 3]));
        assert_eq!(direct_ppr("<w:p><w:r><w:t>x</w:t></w:r></w:p>"), PPr::default());
    }

    #[test]
    fn autospacing_and_shd_solid() {
        let p = r#"<w:p><w:pPr><w:shd w:val="solid" w:color="336699" w:fill="auto"/><w:spacing w:before="0" w:beforeAutospacing="1" w:after="0" w:afterAutospacing="1"/></w:pPr></w:p>"#;
        let d = direct_ppr(p);
        assert_eq!((d.before, d.after, d.shd), (Some(14.0), Some(14.0), Some([0x33, 0x66, 0x99])));
    }

    #[test]
    fn marker_policy() {
        let s = ParaStyles::parse(STYLES);
        // docDefaults 10 pt efter, 1,15: avviker från layoutens 8 pt
        let n = for_marker(&s.resolve(None, &PPr::default()), true);
        assert_eq!((n.before, n.after, n.line), (Some(0.0), Some(10.0), None));
        // exakt basvärdet: ingen markör
        let base = PPr { after: Some(8.0), line: Some(LineRule::Auto(1.15)), ..Default::default() };
        assert!(for_marker(&base, true).is_empty());
        // rubriker: bara ram och skuggning
        let h = for_marker(&PPr { after: Some(0.0), shd: Some([1, 2, 3]), ..Default::default() }, false);
        assert_eq!((h.after, h.shd), (None, Some([1, 2, 3])));
    }

    #[test]
    fn merge_keeps_order_and_replaces() {
        let pp = PPr {
            borders: Borders { left: Some(Border { style: LineStyle::Single, sz: 24, space: 8, color: Some([0x2E, 0x7D, 0x32]) }), ..Default::default() },
            shd: Some([1, 2, 3]),
            after: Some(0.0),
            left: Some(36.0),
            first: Some(-18.0),
            ..Default::default()
        };
        let out = merge_ppr(r#"<w:pStyle w:val="X"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="3"/></w:numPr><w:ind w:left="1"/><w:jc w:val="center"/>"#, &pp);
        let order: Vec<usize> = ["w:pStyle", "w:numPr", "w:pBdr", "w:shd", "w:spacing", "w:ind ", "w:jc"].iter().map(|k| out.find(k).unwrap()).collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{out}");
        assert!(out.contains(r#"<w:left w:val="single" w:sz="24" w:space="8" w:color="2E7D32"/>"#), "{out}");
        assert!(out.contains(r#"<w:ind w:left="720" w:right="0" w:hanging="360"/>"#) && !out.contains(r#"w:left="1""#), "{out}");
        assert!(out.contains(r#"<w:spacing w:after="0"/>"#));
    }
}
