//! Textrutor i Word (`wps:txbx`/`v:textbox` med `w:txbxContent`).
//!
//! Dialekten (se `docs/dev/dokumentlage.md`): en ruta står som ett block före stycket den är
//! förankrad i,
//!
//! ```text
//! <!--k:tb 7-->
//!
//! Vanliga stycken<!--k:pp bdr="…" shd="FFFFFF" ind="l=14 r=40"-->
//!
//! <!--k:/tb-->
//! ```
//!
//! `7` är numret på markören `<!--k:7-->` som bär originalkörningen (rutans form, ram och läge).
//! Styckena i rutan får en syntetisk `k:pp`-markör med rutans fyllning, ram och bredd, så att den
//! ser ut som en ruta i dokumentläget utan egen layout. Rutan ritas i flödet ovanför ankarstycket
//! (text flödar inte runt den). En orörd ruta sparas byte för byte; en ändrad får sitt
//! `w:txbxContent` utbytt i originalkörningen, så form, ram och läge bevaras.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::word::pbox::{Border, Borders, LineStyle, PPr};

pub const OPEN: &str = "<!--k:tb ";
pub const CLOSE: &str = "<!--k:/tb-->";

/// Rutans utseende ur formen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub fill: Option<[u8; 3]>,
    /// Linjens tjocklek (pt) och färg.
    pub line: Option<(f32, [u8; 3])>,
    /// Inre marginal: vänster, ovan, höger, under (pt).
    pub ins: Option<[f32; 4]>,
}

pub fn open_tag(n: usize) -> String {
    format!("{OPEN}{n}-->")
}

/// Numret ur en öppningsrad.
pub fn parse_open(t: &str) -> Option<usize> {
    t.trim().strip_prefix(OPEN)?.strip_suffix("-->")?.trim().parse().ok()
}

/// Öppnings- eller stängningsrad (dold markup).
pub fn is_mark(t: &str) -> bool {
    let t = t.trim();
    t == CLOSE || parse_open(t).is_some()
}

fn local(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn attr(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| local(a.key.as_ref()) == name).map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    match s.to_ascii_lowercase().as_str() {
        "white" => return Some([255, 255, 255]),
        "black" => return Some([0, 0, 0]),
        "red" => return Some([255, 0, 0]),
        "blue" => return Some([0, 0, 255]),
        "yellow" => return Some([255, 255, 0]),
        "green" => return Some([0, 128, 0]),
        "gray" | "grey" => return Some([128, 128, 128]),
        _ => {}
    }
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Längd i punkter ur `7.2pt`, `0.1in`, `2mm` eller ett tal (pt).
fn pt(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let (n, unit) = v.split_at(v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len()));
    let n: f32 = n.trim().parse().ok()?;
    Some(match unit {
        "" | "pt" => n,
        "in" => n * 72.0,
        "mm" => n * 72.0 / 25.4,
        "cm" => n * 72.0 / 2.54,
        "px" => n * 0.75,
        _ => return None,
    })
}

/// Läser fyllning, linje och inre marginal ur hela `w:drawing`/`w:pict`-elementet. Innehållet
/// (`w:txbxContent`) ska vara utklippt. `scheme` slår upp temafärger (`accent2` …).
pub fn read_shape(xml: &str, scheme: &dyn Fn(&str) -> Option<[u8; 3]>) -> Shape {
    let mut sh = Shape::default();
    let mut r = Reader::from_str(xml);
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut fill_none = false;
    let mut line_none = false;
    let mut line_w: Option<f32> = None;
    let mut line_col: Option<[u8; 3]> = None;
    // Reserv ur `wps:style` (temats linje- och fyllningsreferenser).
    let mut style_fill: Option<[u8; 3]> = None;
    let mut style_line: Option<[u8; 3]> = None;
    let mut style_line_on = false;
    let mut style_fill_on = false;
    let clr = |e: &BytesStart<'_>, name: &[u8]| -> Option<[u8; 3]> {
        let v = attr(e, b"val")?;
        if name == b"srgbClr" { hex(&v) } else { scheme(&v) }
    };
    for _ in 0..200_000 {
        let ev = match r.read_event() {
            Ok(e) => e,
            Err(_) => break,
        };
        let (e, empty) = match &ev {
            Event::Eof => break,
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(_) => {
                stack.pop();
                continue;
            }
            _ => continue,
        };
        let name = local(e.name().as_ref()).to_vec();
        let under = |n: &[u8]| stack.iter().any(|s| s == n);
        let parent = stack.last().map(Vec::as_slice);
        match name.as_slice() {
            b"srgbClr" | b"schemeClr" => {
                if let Some(c) = clr(e, &name) {
                    if under(b"style") {
                        if under(b"fillRef") {
                            style_fill = Some(c);
                        } else if under(b"lnRef") {
                            style_line = Some(c);
                        }
                    } else if under(b"ln") && under(b"solidFill") {
                        line_col = line_col.or(Some(c));
                    } else if under(b"spPr") && !under(b"ln") && parent == Some(b"solidFill") {
                        sh.fill = sh.fill.or(Some(c));
                    }
                }
            }
            b"noFill" => match parent {
                Some(b"spPr") => fill_none = true,
                Some(b"ln") => line_none = true,
                _ => {}
            },
            b"ln" => line_w = attr(e, b"w").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 12700.0),
            b"fillRef" => style_fill_on = attr(e, b"idx").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) > 0,
            b"lnRef" => style_line_on = attr(e, b"idx").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) > 0,
            b"bodyPr" => {
                let n = |k: &[u8], d: f32| attr(e, k).and_then(|v| v.parse::<f32>().ok()).map_or(d, |v| v / 12700.0);
                if ["lIns", "tIns", "rIns", "bIns"].iter().any(|k| attr(e, k.as_bytes()).is_some()) {
                    sh.ins = Some([n(b"lIns", 7.2), n(b"tIns", 3.6), n(b"rIns", 7.2), n(b"bIns", 3.6)]);
                }
            }
            // Äldre VML: `v:shape`, `v:rect`, `v:roundrect` och `v:textbox`.
            b"shape" | b"rect" | b"roundrect" | b"oval" if !under(b"ln") => {
                if let Some(c) = attr(e, b"fillcolor").and_then(|v| hex(v.split_whitespace().next().unwrap_or(""))) {
                    sh.fill = Some(c);
                }
                if matches!(attr(e, b"filled").as_deref(), Some("f" | "false")) {
                    fill_none = true;
                }
                if let Some(c) = attr(e, b"strokecolor").and_then(|v| hex(&v)) {
                    line_col = Some(c);
                }
                if matches!(attr(e, b"stroked").as_deref(), Some("f" | "false")) {
                    line_none = true;
                }
                if let Some(w) = attr(e, b"strokeweight").and_then(|v| pt(&v)) {
                    line_w = Some(w);
                }
            }
            b"textbox" => {
                if let Some(v) = attr(e, b"inset") {
                    let p: Vec<Option<f32>> = v.split(',').map(pt).collect();
                    if p.len() == 4 {
                        sh.ins = Some([p[0].unwrap_or(7.2), p[1].unwrap_or(3.6), p[2].unwrap_or(7.2), p[3].unwrap_or(3.6)]);
                    }
                }
            }
            _ => {}
        }
        if !empty {
            stack.push(name);
        }
    }
    if fill_none {
        sh.fill = None;
    } else if sh.fill.is_none() && style_fill_on {
        sh.fill = style_fill;
    }
    if !line_none {
        let col = line_col.or(if style_line_on { style_line } else { None });
        if let Some(c) = col {
            sh.line = Some((line_w.unwrap_or(0.75).max(0.25), c));
        } else if style_line_on && style_line.is_none() && line_w.is_some() {
            sh.line = Some((line_w.unwrap_or(0.75), [0, 0, 0]));
        }
    }
    sh
}

/// Styckeutseendet som får rutan att se ut som en ruta: ram och fyllning runt styckena, och
/// indrag så att den blir lika bred som i dokumentet. `left` är rutans vänsterkant och `width`
/// dess bredd (pt), `text_w` textytans bredd (pt).
pub fn frame(sh: &Shape, left: f32, width: f32, text_w: f32) -> PPr {
    if sh.fill.is_none() && sh.line.is_none() {
        return PPr::default();
    }
    let [il, it, ir, ib] = sh.ins.unwrap_or([7.2, 3.6, 7.2, 3.6]);
    let side = |pad: f32| {
        sh.line.map(|(w, c)| Border { style: LineStyle::Single, sz: ((w * 8.0).round() as u16).clamp(2, 96), space: pad.round().clamp(0.0, 31.0) as u16, color: Some(c) })
    };
    let borders = Borders { top: side(it), left: side(il), bottom: side(ib), right: side(ir), between: None };
    let lw = sh.line.map_or(0.0, |(w, _)| w);
    let left = left.clamp(0.0, (text_w - 36.0).max(0.0));
    let right = (text_w - left - width).max(0.0);
    PPr {
        borders,
        shd: sh.fill,
        // Kanten utanför texten räknas bort så att rutans ytterkant hamnar där Word har den.
        left: Some((left + il.min(31.0) + lw).max(0.0)),
        right: Some((right + ir.min(31.0) + lw).max(0.0)),
        ..Default::default()
    }
}

/// Ett block ur Markdown-texten: rutans nummer och innehållet mellan markörerna.
#[derive(Debug, PartialEq)]
pub struct Found {
    pub token: usize,
    pub inner: String,
}

/// Plockar ut rutblocken ur texten (de ska inte bli stycken av sig själva). Det som blir kvar är
/// texten utan rutorna.
pub fn extract(md: &str) -> (String, Vec<Found>) {
    if !md.contains(OPEN) {
        return (md.to_owned(), Vec::new());
    }
    let mut out = String::with_capacity(md.len());
    let mut found = Vec::new();
    let mut cur: Option<(usize, String, usize)> = None;
    let mut skip_blank = false;
    for line in md.split_inclusive('\n') {
        let t = line.trim();
        if let Some((n, buf, at)) = cur.as_mut() {
            if t == CLOSE {
                found.push(Found { token: *n, inner: buf.trim_matches(['\n', '\r']).to_owned() });
                cur = None;
                skip_blank = true;
            } else if parse_open(t).is_some() {
                // Ett block i ett block: det första avbryts och läggs tillbaka som vanlig text.
                out.insert_str(*at, &format!("{}\n", open_tag(*n)));
                out.push_str(buf);
                cur = parse_open(t).map(|n| (n, String::new(), out.len()));
            } else {
                buf.push_str(line);
            }
            continue;
        }
        if let Some(n) = parse_open(t) {
            cur = Some((n, String::new(), out.len()));
            continue;
        }
        if skip_blank {
            skip_blank = false;
            if t.is_empty() {
                continue;
            }
        }
        out.push_str(line);
    }
    // Ett block utan slut är vanlig text.
    if let Some((n, buf, at)) = cur {
        out.insert_str(at, &format!("{}\n", open_tag(n)));
        out.push_str(&buf);
    }
    (out, found)
}

/// Tar bort bara markörraderna (rutans stycken blir vanliga stycken): för dokument utan original
/// och för andra format.
pub fn strip_tags(md: &str) -> String {
    if !md.contains("<!--k:") {
        return md.to_owned();
    }
    md.split_inclusive('\n').filter(|l| !is_mark(l)).collect()
}

/// Byter innehållet i alla `w:txbxContent` i rutans originalkörning (både `mc:Choice` och
/// `mc:Fallback`). `None` om körningen inte har något innehåll eller har en ruta i rutan.
pub fn replace_content(xml: &str, inner: &str) -> Option<String> {
    let mut out = String::with_capacity(xml.len() + inner.len());
    let mut rest = xml;
    let mut n = 0;
    while let Some(i) = rest.find("<w:txbxContent") {
        let gt = rest[i..].find('>')? + i;
        if rest[i..gt].ends_with('/') {
            return None;
        }
        let end = rest[gt..].find("</w:txbxContent>")? + gt;
        if rest[gt..end].contains("<w:txbxContent") {
            return None;
        }
        out.push_str(&rest[..=gt]);
        out.push_str(inner);
        rest = &rest[end..];
        n += 1;
    }
    out.push_str(rest);
    (n > 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(n: &str) -> Option<[u8; 3]> {
        (n == "accent2").then_some([0xED, 0x7D, 0x31])
    }

    #[test]
    fn shape_from_drawingml() {
        let x = r#"<w:drawing><wp:anchor><wps:wsp><wps:spPr><a:prstGeom prst="rect"/><a:solidFill><a:srgbClr val="FFEEAA"/></a:solidFill><a:ln w="19050"><a:solidFill><a:srgbClr val="112233"/></a:solidFill></a:ln></wps:spPr><wps:bodyPr lIns="91440" tIns="45720" rIns="91440" bIns="45720"/></wps:wsp></wp:anchor></w:drawing>"#;
        let s = read_shape(x, &theme);
        assert_eq!(s.fill, Some([0xFF, 0xEE, 0xAA]));
        assert_eq!(s.line, Some((1.5, [0x11, 0x22, 0x33])));
        assert_eq!(s.ins, Some([7.2, 3.6, 7.2, 3.6]));
    }

    #[test]
    fn shape_from_style_and_nofill() {
        let x = r#"<w:drawing><wps:wsp><wps:spPr><a:noFill/><a:ln><a:noFill/></a:ln></wps:spPr><wps:style><a:lnRef idx="2"><a:schemeClr val="accent2"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent2"/></a:fillRef></wps:style></wps:wsp></w:drawing>"#;
        let s = read_shape(x, &theme);
        assert_eq!(s, Shape::default());
        let y = r#"<w:drawing><wps:wsp><wps:spPr><a:prstGeom prst="rect"/></wps:spPr><wps:style><a:lnRef idx="2"><a:schemeClr val="accent2"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent2"/></a:fillRef></wps:style></wps:wsp></w:drawing>"#;
        let s = read_shape(y, &theme);
        assert_eq!(s.fill, Some([0xED, 0x7D, 0x31]));
        assert_eq!(s.line.map(|l| l.1), Some([0xED, 0x7D, 0x31]));
    }

    #[test]
    fn shape_from_vml() {
        let x = r##"<w:pict><v:rect fillcolor="#ccffcc" strokecolor="black" strokeweight="2pt"><v:textbox inset="0.1in,0,0.1in,0"/></v:rect></w:pict>"##;
        let s = read_shape(x, &theme);
        assert_eq!(s.fill, Some([0xCC, 0xFF, 0xCC]));
        assert_eq!(s.line, Some((2.0, [0, 0, 0])));
        let ins = s.ins.unwrap();
        assert!((ins[0] - 7.2).abs() < 0.01 && ins[1] == 0.0 && (ins[2] - 7.2).abs() < 0.01);
    }

    #[test]
    fn extract_and_strip() {
        let md = "A\n\n<!--k:tb 3-->\n\nIn **rutan**\n\nTvå\n\n<!--k:/tb-->\n\nB\n";
        let (rest, found) = extract(md);
        assert_eq!(rest, "A\n\nB\n");
        assert_eq!(found, vec![Found { token: 3, inner: "In **rutan**\n\nTvå".into() }]);
        assert_eq!(strip_tags(md), "A\n\n\nIn **rutan**\n\nTvå\n\n\nB\n");
        // Ett block utan slut är vanlig text.
        let (rest, found) = extract("x\n<!--k:tb 1-->\ny\n");
        assert!(found.is_empty());
        assert!(rest.contains("y"));
        assert!(rest.contains("k:tb 1"));
    }

    #[test]
    fn replace_content_in_both_branches() {
        let x = "<w:r><mc:Choice><w:txbxContent><w:p><w:r><w:t>a</w:t></w:r></w:p></w:txbxContent></mc:Choice><mc:Fallback><w:txbxContent><w:p/></w:txbxContent></mc:Fallback></w:r>";
        let y = replace_content(x, "<w:p><w:r><w:t>b</w:t></w:r></w:p>").unwrap();
        assert_eq!(y.matches("<w:t>b</w:t>").count(), 2);
        assert!(!y.contains("<w:t>a</w:t>"));
        assert!(replace_content("<w:r/>", "x").is_none());
    }

    #[test]
    fn frame_has_borders_and_indent() {
        let sh = Shape { fill: Some([255, 255, 255]), line: Some((0.75, [0, 0, 0])), ins: None };
        let pp = frame(&sh, 50.0, 200.0, 450.0);
        assert!(pp.borders.any());
        assert_eq!(pp.shd, Some([255, 255, 255]));
        assert!(pp.left.unwrap() > 50.0 && pp.right.unwrap() > 200.0);
        assert!(frame(&Shape::default(), 0.0, 100.0, 400.0).is_empty());
    }
}
