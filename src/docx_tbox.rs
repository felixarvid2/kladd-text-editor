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

use crate::word::anchor::{Anchor, HRel, Pos, VRel, Wrap};
use crate::word::pbox::{Border, Borders, Frame, FrameX, LineStyle, PPr};

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

/// Rutans mått och läge i öppningsraden: `<!--k:tb 7 w="216" h="90" x="right" y="0" wrap="top"-->`
/// (px, samma ankarattribut som `<img>`; se `word::anchor`). Utan ankare ligger rutan i flödet.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Head {
    /// Bredd och fast höjd (px); 0 = okänd.
    pub w: f32,
    pub h: f32,
    pub anchor: Option<Anchor>,
}

fn num(v: f32) -> String {
    format!("{}", (v * 10.0).round() / 10.0)
}

impl Head {
    /// Huvudet som det blir efter en tur genom texten (avrundat som i öppningsraden), så att en orörd ruta
    /// jämförs lika med sig själv.
    pub fn rounded(&self) -> Head {
        parse_head(&open_tag_head(0, self)).map_or(*self, |(_, h)| h)
    }
}

pub fn open_tag_head(n: usize, h: &Head) -> String {
    let mut s = format!("{OPEN}{n}");
    if h.w > 0.0 {
        s.push_str(&format!(" w=\"{}\"", num(h.w)));
    }
    if h.h > 0.0 {
        s.push_str(&format!(" h=\"{}\"", num(h.h)));
    }
    if let Some(a) = &h.anchor {
        a.write_attrs(&mut s);
    }
    s.push_str("-->");
    s
}

/// `nyckel="värde"`-par ur en attributsträng.
fn kv_pairs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while let Some(eq) = rest.find("=\"") {
        let key = rest[..eq].trim().to_owned();
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else { break };
        out.push((key, after[..end].to_owned()));
        rest = after[end + 1..].trim_start();
    }
    out
}

/// Numret och måtten/läget ur en öppningsrad.
pub fn parse_head(t: &str) -> Option<(usize, Head)> {
    let body = t.trim().strip_prefix(OPEN)?.strip_suffix("-->")?.trim();
    let (n, rest) = match body.split_once(char::is_whitespace) {
        Some((n, r)) => (n, r),
        None => (body, ""),
    };
    let n: usize = n.parse().ok()?;
    let kv = kv_pairs(rest);
    let get = |k: &str| kv.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    let f = |k: &str| get(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0);
    Some((n, Head { w: f("w"), h: f("h"), anchor: Anchor::from_attrs(&get) }))
}

/// Numret ur en öppningsrad.
pub fn parse_open(t: &str) -> Option<usize> {
    parse_head(t).map(|h| h.0)
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

/// Styckeutseendet som får rutan att se ut som en ruta: ram och fyllning runt styckena i en ram
/// med rutans bredd och läge (`pbox::Frame`). `width` är rutans bredd (pt, 0 = hela spalten).
pub fn frame(sh: &Shape, x: FrameX, width: f32, height: f32) -> PPr {
    if sh.fill.is_none() && sh.line.is_none() && width <= 1.0 {
        return PPr::default();
    }
    let [il, it, ir, ib] = sh.ins.unwrap_or([7.2, 3.6, 7.2, 3.6]);
    let side = |pad: f32| {
        sh.line.map(|(w, c)| Border { style: LineStyle::Single, sz: ((w * 8.0).round() as u16).clamp(2, 96), space: pad.round().clamp(0.0, 31.0) as u16, color: Some(c) })
    };
    let borders = Borders { top: side(it), left: side(il), bottom: side(ib), right: side(ir), between: None };
    let lw = sh.line.map_or(0.0, |(w, _)| w);
    let w = if width > 1.0 { width } else { 10_000.0 };
    // Rutans ytterkant ligger där Word har den; texten ligger innanför luften och linjen.
    PPr { borders, shd: sh.fill, frame: Some(Frame { w, x, pl: il.min(31.0) + lw, pr: ir.min(31.0) + lw, h: height.max(0.0) }), ..Default::default() }
}

/// Ett block ur Markdown-texten: rutans nummer och innehållet mellan markörerna.
#[derive(Debug, PartialEq)]
pub struct Found {
    pub token: usize,
    pub inner: String,
    /// Mått och läge ur öppningsraden.
    pub head: Head,
}

/// Numret på en ny ruta (Infoga → Textruta): ingen ruta i originalet har det.
pub const NEW_TOKEN: usize = 4_000_000_000;

/// Plockar ut rutblocken ur texten (de ska inte bli stycken av sig själva). Det som blir kvar är
/// texten utan rutorna. Block vars nummer `known` inte godkänner är nya rutor: de ersätts av en
/// rad `<!--k:N-->` med ett nytt nummer från `next` och kommer med i andra listan.
pub fn extract(md: &str, known: &dyn Fn(usize) -> bool, next: usize) -> (String, Vec<Found>, Vec<Found>) {
    if !md.contains(OPEN) {
        return (md.to_owned(), Vec::new(), Vec::new());
    }
    let mut out = String::with_capacity(md.len());
    let (mut kept, mut fresh) = (Vec::new(), Vec::new());
    let mut cur: Option<(usize, Head, String, usize)> = None;
    let mut skip_blank = false;
    for line in md.split_inclusive('\n') {
        let t = line.trim();
        if let Some((n, head, buf, at)) = cur.as_mut() {
            if t == CLOSE {
                let (n, head, inner) = (*n, *head, buf.trim_matches(['\n', '\r']).to_owned());
                cur = None;
                skip_blank = true;
                if known(n) {
                    kept.push(Found { token: n, inner, head });
                } else {
                    // Ny ruta: en bärarrad med ett nytt markörnummer tar dess plats.
                    let idx = next + fresh.len();
                    out.push_str(&format!("<!--k:{idx}-->\n\n"));
                    fresh.push(Found { token: idx, inner, head });
                }
            } else if let Some((m, mh)) = parse_head(t) {
                // Ett block i ett block: det första avbryts och läggs tillbaka som vanlig text.
                out.insert_str(*at, &format!("{}\n", open_tag_head(*n, head)));
                out.push_str(buf);
                cur = Some((m, mh, String::new(), out.len()));
            } else {
                buf.push_str(line);
            }
            continue;
        }
        if let Some((n, head)) = parse_head(t) {
            cur = Some((n, head, String::new(), out.len()));
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
    if let Some((n, head, buf, at)) = cur {
        out.insert_str(at, &format!("{}\n", open_tag_head(n, &head)));
        out.push_str(&buf);
    }
    (out, kept, fresh)
}

/// Ramen för en ny ruta ur dess första markör med `fr=`, och texten utan ramarna (de skrivs inte som
/// styckeram inne i rutan).
pub fn split_frame(inner: &str) -> (PPr, String) {
    let mut look = PPr::default();
    let mut clean = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(i) = rest.find(crate::word::pbox::MARK.trim_end()) {
        clean.push_str(&rest[..i]);
        let Some(e) = rest[i..].find("-->") else { break };
        let tag = &rest[i..i + e + 3];
        match PPr::parse_marker(tag) {
            Some(pp) if pp.frame.is_some() => {
                if look.is_empty() {
                    look = pp;
                }
            }
            _ => clean.push_str(tag),
        }
        rest = &rest[i + e + 3..];
    }
    clean.push_str(rest);
    (look, clean.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_owned())
}

/// Texten som Infoga → Textruta sätter in: en ruta med ram, 3 tum bred.
pub fn new_block() -> String {
    let sh = Shape { fill: None, line: Some((0.75, [0, 0, 0])), ins: Some([5.0; 4]) };
    let pp = frame(&sh, FrameX::Start, 216.0, 0.0);
    format!("{}\n\nText{}\n\n{CLOSE}", open_tag(NEW_TOKEN), pp.marker())
}

/// Originalkörningen (`w:r` med en förankrad `wps`-textruta) för en ny ruta. `content` är styckena som XML,
/// `pp` rutans ram (bredd, läge, kant och fyllning), `head` måtten och läget ur öppningsraden (en dragen eller
/// ändrad ruta behåller sitt läge, omflöde och avstånd), `text_w` textytans bredd i punkter och `lines` ungefär
/// hur många rader texten har (för starthöjden; rutan växer med texten i Word).
pub fn new_box_run(content: &str, pp: &PPr, head: &Head, id: u32, text_w: f32, lines: usize) -> String {
    let w = pp.frame.map(|f| f.w).filter(|w| *w < 5000.0).unwrap_or(text_w).clamp(36.0, text_w.max(36.0));
    let h = (lines as f32 * 18.0 + 14.0).max(30.0);
    let emu = |v: f32| (v * 12700.0).round() as i64;
    let col = |c: [u8; 3]| format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
    let fill = pp.shd.map_or("<a:noFill/>".to_owned(), |c| format!("<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>", col(c)));
    let ln = match pp.borders.left {
        Some(b) => format!(
            "<a:ln w=\"{}\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill></a:ln>",
            emu(b.width_pt()),
            b.color.map_or("000000".to_owned(), col)
        ),
        None => "<a:ln><a:noFill/></a:ln>".to_owned(),
    };
    let ins = |b: Option<Border>| emu(b.map_or(0.0, |b| f32::from(b.space)).max(2.0));
    let pos = match pp.frame.map(|f| f.x) {
        Some(FrameX::Center) => "<wp:align>center</wp:align>".to_owned(),
        Some(FrameX::End) => "<wp:align>right</wp:align>".to_owned(),
        Some(FrameX::Off(v)) => format!("<wp:posOffset>{}</wp:posOffset>", emu(v)),
        _ => "<wp:align>left</wp:align>".to_owned(),
    };
    let (cx, cy) = (emu(w), emu(h));
    let run = format!(
        "<w:r><w:drawing><wp:anchor xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" distT=\"0\" distB=\"91440\" distL=\"114300\" distR=\"114300\" simplePos=\"0\" relativeHeight=\"251659264\" behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"column\">{pos}</wp:positionH><wp:positionV relativeFrom=\"paragraph\"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:wrapTopAndBottom/><wp:docPr id=\"{id}\" name=\"Textruta {id}\"/><wp:cNvGraphicFramePr/><a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\"><wps:wsp><wps:cNvSpPr txBox=\"1\"/><wps:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>{fill}{ln}</wps:spPr><wps:txbx><w:txbxContent>{content}</w:txbxContent></wps:txbx><wps:bodyPr rot=\"0\" vert=\"horz\" wrap=\"square\" lIns=\"{}\" tIns=\"{}\" rIns=\"{}\" bIns=\"{}\" anchor=\"t\" anchorCtr=\"0\"><a:spAutoFit/></wps:bodyPr></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>",
        ins(pp.borders.left),
        ins(pp.borders.top),
        ins(pp.borders.right),
        ins(pp.borders.bottom)
    );
    place_new_run(run, head)
}

/// Lägger öppningsradens mått och ankare (läge, omflöde, avstånd) i en nyskriven rutkörning.
fn place_new_run(mut x: String, head: &Head) -> String {
    let emu = |px: f32| (px * 9525.0).round() as i64;
    if head.w > 0.0 && head.h > 0.0 {
        x = set_cx_cy(&x, "<wp:extent", emu(head.w), emu(head.h));
        x = set_cx_cy(&x, "<a:ext ", emu(head.w), emu(head.h));
    }
    let Some(a) = head.anchor else { return x };
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
    x = set_position(&x, "positionH", hrel, a.h, ["left", "center", "right"]);
    x = set_position(&x, "positionV", vrel, a.v, ["top", "center", "bottom"]);
    let (wrap, behind) = match a.wrap {
        Wrap::Square => ("<wp:wrapSquare wrapText=\"bothSides\"/>", "0"),
        Wrap::TopBottom => ("<wp:wrapTopAndBottom/>", "0"),
        Wrap::Behind => ("<wp:wrapNone/>", "1"),
        Wrap::Front => ("<wp:wrapNone/>", "0"),
    };
    x = x.replacen("<wp:wrapTopAndBottom/>", wrap, 1).replacen("behindDoc=\"0\"", &format!("behindDoc=\"{behind}\""), 1);
    let d = a.dist;
    x.replacen("distT=\"0\" distB=\"91440\" distL=\"114300\" distR=\"114300\"", &format!("distT=\"{}\" distB=\"{}\" distL=\"{}\" distR=\"{}\"", emu(d[0]), emu(d[2]), emu(d[3]), emu(d[1])), 1)
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

/// Byter `cx`/`cy` i alla taggar som börjar med `prefix` och har dem (`<wp:extent`, `<a:ext `).
fn set_cx_cy(xml: &str, prefix: &str, cx: i64, cy: i64) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find(prefix) {
        let Some(gt) = rest[i..].find('>').map(|g| g + i) else { break };
        out.push_str(&rest[..i]);
        let tag = &rest[i..gt];
        let tag = if tag.contains("cx=\"") && tag.contains("cy=\"") { set_attr(&set_attr(tag, "cx", &cx.to_string()), "cy", &cy.to_string()) } else { tag.to_owned() };
        out.push_str(&tag);
        rest = &rest[gt..];
    }
    out.push_str(rest);
    out
}

/// Byter värdet på attributet `name="…"` i en tagg (finns det inte, görs ingenting).
fn set_attr(tag: &str, name: &str, val: &str) -> String {
    let key = format!(" {name}=\"");
    match tag.find(&key) {
        Some(i) => {
            let v0 = i + key.len();
            match tag[v0..].find('"') {
                Some(e) => format!("{}{}{}", &tag[..v0], val, &tag[v0 + e..]),
                None => tag.to_owned(),
            }
        }
        None => tag.to_owned(),
    }
}

/// Byter ut elementet `wp:positionH`/`wp:positionV` mot ett nytt.
fn set_position(xml: &str, tag: &str, rel: &str, pos: Pos, names: [&str; 3]) -> String {
    let open = format!("<wp:{tag}");
    let close = format!("</wp:{tag}>");
    let inner = match pos {
        Pos::Off(px) => format!("<wp:posOffset>{}</wp:posOffset>", (px * 9525.0).round() as i64),
        Pos::Start => format!("<wp:align>{}</wp:align>", names[0]),
        Pos::Center => format!("<wp:align>{}</wp:align>", names[1]),
        Pos::End => format!("<wp:align>{}</wp:align>", names[2]),
    };
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let Some(e) = rest[i..].find(&close).map(|e| e + i + close.len()) else { break };
        out.push_str(&rest[..i]);
        out.push_str(&format!("{open} relativeFrom=\"{rel}\">{inner}{close}"));
        rest = &rest[e..];
    }
    out.push_str(rest);
    out
}

/// Sätter (eller lägger till) en egenskap i ett VML-`style` (`a:1;b:2`); tomt värde tar bort den.
fn set_prop(style: &str, key: &str, val: &str) -> String {
    let mut props: Vec<(String, String)> = style
        .split(';')
        .filter_map(|kv| kv.split_once(':').map(|(a, b)| (a.trim().to_owned(), b.trim().to_owned())))
        .collect();
    props.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
    if !val.is_empty() {
        props.push((key.to_owned(), val.to_owned()));
    }
    props.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(";")
}

/// Bearbetar varje VML-`style="…"` som har ett mått eller en förskjutning.
fn map_vml_style(xml: &str, f: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find(" style=\"") {
        let v0 = i + 8;
        let Some(e) = rest[v0..].find('"').map(|e| e + v0) else { break };
        out.push_str(&rest[..v0]);
        let v = &rest[v0..e];
        if v.contains("width:") || v.contains("margin-left:") {
            out.push_str(&f(v));
        } else {
            out.push_str(v);
        }
        rest = &rest[e..];
    }
    out.push_str(rest);
    out
}

/// Skriver ny storlek och nytt läge (`new`) i originalkörningen (`wp:extent`, `a:ext`, `wp:positionH/V` och
/// VML-`style`). Det som inte ändrats (`old`) lämnas orört, och en ruta i flödet får bara ny storlek.
pub fn set_geometry(xml: &str, old: &Head, new: &Head) -> String {
    let mut x = xml.to_owned();
    let emu = |px: f32| (px * 9525.0).round() as i64;
    let (w, h) = (if new.w > 0.0 { new.w } else { old.w }, if new.h > 0.0 { new.h } else { old.h });
    if (w - old.w).abs() > 0.4 || (h - old.h).abs() > 0.4 {
        x = set_cx_cy(&x, "<wp:extent", emu(w), emu(h));
        x = set_cx_cy(&x, "<a:ext ", emu(w), emu(h));
        let (wp, hp) = (format!("{}pt", num(w * 0.75)), format!("{}pt", num(h * 0.75)));
        x = map_vml_style(&x, &|s| set_prop(&set_prop(s, "width", &wp), "height", &hp));
    }
    if let (Some(o), Some(n)) = (old.anchor, new.anchor) {
        if (o.hrel, o.h, o.vrel, o.v) != (n.hrel, n.h, n.vrel, n.v) {
            let hrel = match n.hrel {
                HRel::Column => "column",
                HRel::Margin => "margin",
                HRel::Page => "page",
            };
            let vrel = match n.vrel {
                VRel::Paragraph => "paragraph",
                VRel::Margin => "margin",
                VRel::Page => "page",
            };
            x = set_position(&x, "positionH", hrel, n.h, ["left", "center", "right"]);
            x = set_position(&x, "positionV", vrel, n.v, ["top", "center", "bottom"]);
            x = x.replace(" simplePos=\"1\"", " simplePos=\"0\"");
            let pt = |p: Pos| if let Pos::Off(px) = p { format!("{}pt", num(px * 0.75)) } else { "0".to_owned() };
            let al = |p: Pos, n: [&str; 3]| match p {
                Pos::Off(_) => String::new(),
                Pos::Start => n[0].to_owned(),
                Pos::Center => n[1].to_owned(),
                Pos::End => n[2].to_owned(),
            };
            let (hr, vr) = (
                match n.hrel {
                    HRel::Column => "text",
                    HRel::Margin => "margin",
                    HRel::Page => "page",
                },
                match n.vrel {
                    VRel::Paragraph => "text",
                    VRel::Margin => "margin",
                    VRel::Page => "page",
                },
            );
            x = map_vml_style(&x, &|s| {
                let s = set_prop(&set_prop(s, "margin-left", &pt(n.h)), "margin-top", &pt(n.v));
                let s = set_prop(&set_prop(&s, "mso-position-horizontal", &al(n.h, ["left", "center", "right"])), "mso-position-vertical", &al(n.v, ["top", "center", "bottom"]));
                set_prop(&set_prop(&s, "mso-position-horizontal-relative", hr), "mso-position-vertical-relative", vr)
            });
        }
    }
    x
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
        let (rest, found, fresh) = extract(md, &|n| n == 3, 100);
        assert_eq!(rest, "A\n\nB\n");
        assert_eq!(found, vec![Found { token: 3, inner: "In **rutan**\n\nTvå".into(), head: Head::default() }]);
        assert!(fresh.is_empty());
        // Okänt nummer: en ny ruta, ersatt av en bärarrad med nytt nummer.
        let (rest, found, fresh) = extract(md, &|_| false, 100);
        assert_eq!(rest, "A\n\n<!--k:100-->\n\nB\n");
        assert!(found.is_empty());
        assert_eq!(fresh, vec![Found { token: 100, inner: "In **rutan**\n\nTvå".into(), head: Head::default() }]);
        // Ett block utan slut är vanlig text.
        let (rest, found, _) = extract("x\n<!--k:tb 1-->\ny\n", &|_| true, 0);
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
    fn frame_has_borders_and_geometry() {
        let sh = Shape { fill: Some([255, 255, 255]), line: Some((0.75, [0, 0, 0])), ins: None };
        let pp = frame(&sh, FrameX::Center, 200.0, 0.0);
        assert!(pp.borders.any());
        assert_eq!(pp.shd, Some([255, 255, 255]));
        let (tl, tr, l, w) = pp.frame_geom(450.0).unwrap();
        assert_eq!((l, w), (125.0, 200.0));
        assert!(tl > l && tr > 125.0);
        // Hela spalten när bredden saknas, och markören överlever en tur och retur.
        assert_eq!(frame(&sh, FrameX::Start, 0.0, 0.0).frame_geom(300.0).map(|g| g.3), Some(300.0));
        assert_eq!(PPr::parse_marker(&pp.marker()), Some(pp));
        assert!(frame(&Shape::default(), FrameX::Start, 0.0, 0.0).is_empty());
    }
}
