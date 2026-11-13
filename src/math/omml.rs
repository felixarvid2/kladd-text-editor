//! Office Math (OMML, `m:oMath`) i docx: skrivs ur syntaxträdet och läses tillbaka till LaTeX.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::ast::*;
use super::latex;
use super::parse;

pub const NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

// ============================================================== skriva

/// `m:oMath` (eller `m:oMathPara` för en egen rad) med `xmlns:m` på elementet, att läggas i ett `w:p`.
pub fn to_omml(nodes: &[Node], display: bool) -> String {
    let mut w = W { out: String::new() };
    if display {
        w.out.push_str(&format!("<m:oMathPara xmlns:m=\"{NS}\"><m:oMath>"));
    } else {
        w.out.push_str(&format!("<m:oMath xmlns:m=\"{NS}\">"));
    }
    w.list(nodes, FontStyle::Auto);
    w.out.push_str("</m:oMath>");
    if display {
        w.out.push_str("</m:oMathPara>");
    }
    w.out
}

struct W {
    out: String,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn is_integral(c: char) -> bool {
    matches!(c, '∫' | '∬' | '∭' | '∮' | '∯')
}

fn space_char(s: Space) -> &'static str {
    match s {
        Space::Thin => "\u{2009}",
        Space::Med => "\u{205F}",
        Space::Thick => "\u{2005}",
        Space::Quad => "\u{2003}",
        Space::QQuad => "\u{2003}\u{2003}",
        Space::Normal => "\u{A0}",
        Space::NegThin => "",
    }
}

/// Teckentext som går in i en körning, eller `None` för strukturella noder.
fn run_text(n: &Node) -> Option<String> {
    match n {
        Node::Num(s) => Some(s.clone()),
        Node::Var(c) => Some(c.to_string()),
        Node::Sym(c, class) if *class != Class::Op => Some(c.to_string()),
        Node::Space(s) => Some(space_char(*s).to_owned()),
        _ => None,
    }
}

impl W {
    fn run(&mut self, text: &str, font: FontStyle, extra: &str) {
        if text.is_empty() && extra.is_empty() {
            return;
        }
        let (scr, sty) = match font {
            FontStyle::Auto => ("", ""),
            FontStyle::Rm => ("", "p"),
            FontStyle::It => ("", "i"),
            FontStyle::Bf => ("", "b"),
            FontStyle::BfIt => ("", "bi"),
            FontStyle::Cal => ("script", "p"),
            FontStyle::Frak => ("fraktur", "p"),
            FontStyle::Bb => ("double-struck", "p"),
            FontStyle::Sf => ("sans-serif", "p"),
            FontStyle::Tt => ("monospace", "p"),
        };
        let mut pr = String::new();
        if extra.contains("nor") {
            pr.push_str("<m:nor/>");
        } else {
            if !scr.is_empty() {
                pr.push_str(&format!("<m:scr m:val=\"{scr}\"/>"));
            }
            if !sty.is_empty() {
                pr.push_str(&format!("<m:sty m:val=\"{sty}\"/>"));
            }
        }
        if extra.contains("aln") {
            pr.push_str("<m:aln/>");
        }
        self.out.push_str("<m:r>");
        if !pr.is_empty() {
            self.out.push_str(&format!("<m:rPr>{pr}</m:rPr>"));
        }
        self.out.push_str(&format!("<m:t xml:space=\"preserve\">{}</m:t></m:r>", esc(text)));
    }

    fn cont(&mut self, tag: &str, nodes: &[Node], font: FontStyle) {
        self.out.push_str(&format!("<m:{tag}>"));
        self.list(nodes, font);
        self.out.push_str(&format!("</m:{tag}>"));
    }

    fn list(&mut self, nodes: &[Node], font: FontStyle) {
        let mut buf = String::new();
        let mut i = 0;
        while i < nodes.len() {
            if let Some(t) = run_text(&nodes[i]) {
                buf.push_str(&t);
                i += 1;
                continue;
            }
            if !buf.is_empty() {
                let b = std::mem::take(&mut buf);
                self.run(&b, font, "");
            }
            // Stora operatorer tar nästa nod som operand.
            let is_nary = matches!(&nodes[i], Node::Sym(_, Class::Op))
                || matches!(&nodes[i], Node::Scripts { base, .. } if matches!(&**base, Node::Sym(_, Class::Op)));
            let operand = if is_nary {
                match nodes.get(i + 1) {
                    Some(Node::Sym(_, Class::Bin | Class::Rel | Class::Close | Class::Punct)) | Some(Node::Space(_)) | None => None,
                    Some(n) => Some(n),
                }
            } else {
                None
            };
            self.node(&nodes[i], operand, font);
            i += 1 + usize::from(operand.is_some());
        }
        if !buf.is_empty() {
            self.run(&buf, font, "");
        }
    }

    fn node(&mut self, n: &Node, operand: Option<&Node>, font: FontStyle) {
        match n {
            Node::Num(_) | Node::Var(_) | Node::Space(_) => {}
            Node::Sym(c, Class::Op) => self.nary(*c, None, None, Limits::Default, operand, font),
            Node::Sym(..) => {}
            Node::Text(t) => self.run(t, font, "nor"),
            Node::Func { name, .. } => self.run(name, FontStyle::Rm, ""),
            Node::Group(l) => self.list(l, font),
            Node::Scripts { base, sub, sup, limits } => self.scripts(base, sub.as_deref(), sup.as_deref(), *limits, operand, font),
            Node::Frac { kind, num, den } => {
                if *kind == FracKind::Binom {
                    self.out.push_str("<m:d><m:dPr><m:begChr m:val=\"(\"/><m:endChr m:val=\")\"/></m:dPr><m:e>");
                }
                let ty = if *kind == FracKind::Binom { "<m:fPr><m:type m:val=\"noBar\"/></m:fPr>" } else { "" };
                self.out.push_str(&format!("<m:f>{ty}"));
                self.cont("num", num, font);
                self.cont("den", den, font);
                self.out.push_str("</m:f>");
                if *kind == FracKind::Binom {
                    self.out.push_str("</m:e></m:d>");
                }
            }
            Node::Sqrt { index, body } => {
                self.out.push_str("<m:rad>");
                match index {
                    Some(i) => {
                        self.out.push_str("<m:radPr/>");
                        self.cont("deg", i, font);
                    }
                    None => self.out.push_str("<m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/>"),
                }
                self.cont("e", body, font);
                self.out.push_str("</m:rad>");
            }
            Node::Delim { open, close, body } => self.delim(*open, *close, body, font),
            Node::Middle(c) => self.run(&c.to_string(), font, ""),
            Node::BigDelim { ch, .. } => {
                if *ch != '.' {
                    self.run(&ch.to_string(), font, "");
                }
            }
            Node::Matrix { env, rows, .. } => self.matrix(*env, rows, font),
            Node::Accent { kind, body } => {
                self.out.push_str(&format!("<m:acc><m:accPr><m:chr m:val=\"{}\"/></m:accPr>", kind.mark()));
                self.cont("e", body, font);
                self.out.push_str("</m:acc>");
            }
            Node::Line { over, body } => {
                self.out.push_str(&format!("<m:bar><m:barPr><m:pos m:val=\"{}\"/></m:barPr>", if *over { "top" } else { "bot" }));
                self.cont("e", body, font);
                self.out.push_str("</m:bar>");
            }
            Node::Brace { over, body } => self.group_chr(*over, body, font),
            Node::Stack { over, note, base } => {
                let tag = if *over { "limUpp" } else { "limLow" };
                self.out.push_str(&format!("<m:{tag}>"));
                self.cont("e", base, font);
                self.cont("lim", note, font);
                self.out.push_str(&format!("</m:{tag}>"));
            }
            Node::Font { style, body } => self.list(body, *style),
            Node::Styled { body, .. } | Node::Classed { body, .. } => self.list(body, font),
        }
    }

    fn group_chr(&mut self, over: bool, body: &[Node], font: FontStyle) {
        let (chr, pos) = if over { ('\u{23DE}', "top") } else { ('\u{23DF}', "bot") };
        self.out.push_str(&format!("<m:groupChr><m:groupChrPr><m:chr m:val=\"{chr}\"/><m:pos m:val=\"{pos}\"/></m:groupChrPr>"));
        self.cont("e", body, font);
        self.out.push_str("</m:groupChr>");
    }

    fn nary(&mut self, c: char, sub: Option<&[Node]>, sup: Option<&[Node]>, limits: Limits, operand: Option<&Node>, font: FontStyle) {
        let under_over = limits == Limits::Limits || (limits == Limits::Default && !is_integral(c));
        let mut pr = format!("<m:chr m:val=\"{c}\"/><m:limLoc m:val=\"{}\"/>", if under_over { "undOvr" } else { "subSup" });
        if sub.is_none() {
            pr.push_str("<m:subHide m:val=\"1\"/>");
        }
        if sup.is_none() {
            pr.push_str("<m:supHide m:val=\"1\"/>");
        }
        self.out.push_str(&format!("<m:nary><m:naryPr>{pr}</m:naryPr>"));
        self.cont("sub", sub.unwrap_or(&[]), font);
        self.cont("sup", sup.unwrap_or(&[]), font);
        self.out.push_str("<m:e>");
        if let Some(o) = operand {
            self.list(std::slice::from_ref(o), font);
        }
        self.out.push_str("</m:e></m:nary>");
    }

    fn scripts(&mut self, base: &Node, sub: Option<&[Node]>, sup: Option<&[Node]>, limits: Limits, operand: Option<&Node>, font: FontStyle) {
        if let Node::Sym(c, Class::Op) = base {
            self.nary(*c, sub, sup, limits, operand, font);
            return;
        }
        let lim_like = matches!(base, Node::Brace { .. }) || matches!(base, Node::Func { limits: true, .. }) && limits != Limits::NoLimits;
        let base_list = std::slice::from_ref(base);
        if lim_like {
            match (sub, sup) {
                (Some(s), Some(u)) => {
                    self.out.push_str("<m:limUpp><m:e><m:limLow>");
                    self.cont("e", base_list, font);
                    self.cont("lim", s, font);
                    self.out.push_str("</m:limLow></m:e>");
                    self.cont("lim", u, font);
                    self.out.push_str("</m:limUpp>");
                }
                (Some(s), None) => {
                    self.out.push_str("<m:limLow>");
                    self.cont("e", base_list, font);
                    self.cont("lim", s, font);
                    self.out.push_str("</m:limLow>");
                }
                (None, Some(u)) => {
                    self.out.push_str("<m:limUpp>");
                    self.cont("e", base_list, font);
                    self.cont("lim", u, font);
                    self.out.push_str("</m:limUpp>");
                }
                (None, None) => self.node(base, None, font),
            }
            return;
        }
        match (sub, sup) {
            (Some(s), Some(u)) => {
                self.out.push_str("<m:sSubSup>");
                self.cont("e", base_list, font);
                self.cont("sub", s, font);
                self.cont("sup", u, font);
                self.out.push_str("</m:sSubSup>");
            }
            (Some(s), None) => {
                self.out.push_str("<m:sSub>");
                self.cont("e", base_list, font);
                self.cont("sub", s, font);
                self.out.push_str("</m:sSub>");
            }
            (None, Some(u)) => {
                self.out.push_str("<m:sSup>");
                self.cont("e", base_list, font);
                self.cont("sup", u, font);
                self.out.push_str("</m:sSup>");
            }
            (None, None) => self.list(base_list, font),
        }
    }

    fn delim(&mut self, open: Option<char>, close: Option<char>, body: &[Node], font: FontStyle) {
        let chr = |t: &str, c: Option<char>| format!("<m:{t} m:val=\"{}\"/>", c.map(|c| esc(&c.to_string())).unwrap_or_default());
        let mut parts: Vec<Vec<Node>> = vec![Vec::new()];
        let mut sep = None;
        for n in body {
            if let Node::Middle(c) = n {
                sep.get_or_insert(*c);
                parts.push(Vec::new());
            } else if let Some(p) = parts.last_mut() {
                p.push(n.clone());
            }
        }
        let mut pr = format!("{}{}", chr("begChr", open), chr("endChr", close));
        if let Some(s) = sep {
            pr.push_str(&chr("sepChr", Some(s)));
        }
        self.out.push_str(&format!("<m:d><m:dPr>{pr}</m:dPr>"));
        for p in &parts {
            self.cont("e", p, font);
        }
        self.out.push_str("</m:d>");
    }

    fn matrix(&mut self, env: MatrixEnv, rows: &[Vec<Vec<Node>>], font: FontStyle) {
        let (open, close) = env.delims();
        let wrapped = open.is_some() || close.is_some();
        if wrapped {
            let chr = |t: &str, c: Option<char>| format!("<m:{t} m:val=\"{}\"/>", c.map(|c| esc(&c.to_string())).unwrap_or_default());
            self.out.push_str(&format!("<m:d><m:dPr>{}{}</m:dPr><m:e>", chr("begChr", open), chr("endChr", close)));
        }
        match env {
            MatrixEnv::Aligned | MatrixEnv::Gathered | MatrixEnv::SubStack => {
                self.out.push_str("<m:eqArr>");
                for r in rows {
                    self.out.push_str("<m:e>");
                    for (j, c) in r.iter().enumerate() {
                        if j > 0 {
                            // Justeringspunkt: första körningen i cellen märks med `m:aln`.
                            self.out.push_str("<m:r><m:rPr><m:aln/></m:rPr><m:t></m:t></m:r>");
                        }
                        self.list(c, font);
                    }
                    self.out.push_str("</m:e>");
                }
                self.out.push_str("</m:eqArr>");
            }
            _ => {
                let jc = if env == MatrixEnv::Cases { "<m:mPr><m:mcs><m:mc><m:mcPr><m:count m:val=\"2\"/><m:mcJc m:val=\"left\"/></m:mcPr></m:mc></m:mcs></m:mPr>" } else { "" };
                self.out.push_str(&format!("<m:m>{jc}"));
                for r in rows {
                    self.out.push_str("<m:mr>");
                    for c in r {
                        self.cont("e", c, font);
                    }
                    self.out.push_str("</m:mr>");
                }
                self.out.push_str("</m:m>");
            }
        }
        if wrapped {
            self.out.push_str("</m:e></m:d>");
        }
    }
}

// ============================================================== läsa

/// En element i XML-trädet (bara det som OMML-läsningen behöver).
struct X {
    pre: String,
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<X>,
    text: String,
}

impl X {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn kid(&self, name: &str) -> Option<&X> {
        self.kids.iter().find(|k| k.pre == "m" && k.name == name)
    }

    /// Värdet `m:val` på barnet `name`, om barnet finns.
    fn val(&self, name: &str) -> Option<String> {
        self.kid(name).map(|k| k.attr("val").unwrap_or("").to_owned())
    }
}

fn split_name(raw: &[u8]) -> (String, String) {
    let s = String::from_utf8_lossy(raw).into_owned();
    match s.split_once(':') {
        Some((p, n)) => (p.to_owned(), n.to_owned()),
        None => (String::new(), s),
    }
}

fn start_of(e: &BytesStart<'_>) -> X {
    let (pre, name) = split_name(e.name().as_ref());
    let attrs = e
        .attributes()
        .flatten()
        .map(|a| {
            let (_, k) = split_name(a.key.as_ref());
            let v = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned());
            (k, v)
        })
        .collect();
    X { pre, name, attrs, kids: Vec::new(), text: String::new() }
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

fn parse_xml(s: &str) -> Option<X> {
    let mut r = Reader::from_str(s);
    let mut stack: Vec<X> = Vec::new();
    let mut root: Option<X> = None;
    loop {
        match r.read_event().ok()? {
            Event::Start(e) => stack.push(start_of(&e)),
            Event::Empty(e) => {
                let x = start_of(&e);
                match stack.last_mut() {
                    Some(p) => p.kids.push(x),
                    None => root = Some(x),
                }
            }
            Event::End(_) => {
                let x = stack.pop()?;
                match stack.last_mut() {
                    Some(p) => p.kids.push(x),
                    None => root = Some(x),
                }
            }
            Event::Eof => break,
            ev => {
                let t = text_of(&ev);
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t);
                }
            }
        }
    }
    root
}

type R<T> = Result<T, ()>;

/// Läser `m:oMath`/`m:oMathPara` till träd; `true` om formeln står på egen rad.
pub fn read(xml: &str) -> Option<(Vec<Node>, bool)> {
    let root = parse_xml(xml)?;
    if root.pre != "m" {
        return None;
    }
    let (nodes, display) = match root.name.as_str() {
        "oMathPara" => {
            let mut all = Vec::new();
            for k in root.kids.iter().filter(|k| k.pre == "m" && k.name == "oMath") {
                if !all.is_empty() {
                    all.push(Node::Space(Space::Quad));
                }
                all.extend(nodes_of(&k.kids).ok()?);
            }
            (all, true)
        }
        "oMath" => (nodes_of(&root.kids).ok()?, false),
        _ => return None,
    };
    Some((nodes, display))
}

/// OMML som LaTeX. `None` om formeln har något som Kladd inte kan skriva som LaTeX (då bevaras
/// den som ogenomskinligt objekt) eller om texten inte skulle läsas in igen.
pub fn to_latex(xml: &str) -> Option<(String, bool)> {
    let (nodes, display) = read(xml)?;
    let l = latex::to_latex(&nodes);
    if l.is_empty() || parse::parse(&l).is_err() || l.contains('$') {
        return None;
    }
    Some((l, display))
}

fn single_or_group(nodes: Vec<Node>) -> Node {
    if nodes.len() == 1 { nodes.into_iter().next().unwrap_or(Node::Group(Vec::new())) } else { Node::Group(nodes) }
}

fn nodes_of(kids: &[X]) -> R<Vec<Node>> {
    let mut out = Vec::new();
    for k in kids {
        out.extend(elem(k)?);
    }
    Ok(out)
}

/// Innehållet i barnet `name` (tomt om det saknas).
fn cont(x: &X, name: &str) -> R<Vec<Node>> {
    match x.kid(name) {
        Some(k) => nodes_of(&k.kids),
        None => Ok(Vec::new()),
    }
}

fn on(v: Option<String>) -> bool {
    !matches!(v.as_deref(), None | Some("0" | "false" | "off"))
}

fn chr_val(pr: Option<&X>, name: &str, default: Option<char>) -> Option<char> {
    match pr.and_then(|p| p.val(name)) {
        None => default,
        Some(v) => v.chars().next(),
    }
}

fn elem(x: &X) -> R<Vec<Node>> {
    if x.pre != "m" {
        // `w:rPr`, `w:ins` … ryms inte i formler; ignorera.
        return Ok(Vec::new());
    }
    if x.name.ends_with("Pr") {
        return Ok(Vec::new());
    }
    match x.name.as_str() {
        "r" => run(x),
        "oMath" => nodes_of(&x.kids),
        "f" => {
            let ty = x.kid("fPr").and_then(|p| p.val("type")).unwrap_or_default();
            let kind = if ty == "noBar" { FracKind::Binom } else { FracKind::Frac };
            Ok(vec![Node::Frac { kind, num: cont(x, "num")?, den: cont(x, "den")? }])
        }
        "rad" => {
            let hide = on(x.kid("radPr").and_then(|p| p.val("degHide")));
            let deg = cont(x, "deg")?;
            Ok(vec![Node::Sqrt { index: (!hide && !deg.is_empty()).then_some(deg), body: cont(x, "e")? }])
        }
        "sSup" => Ok(vec![Node::Scripts { base: Box::new(single_or_group(cont(x, "e")?)), sub: None, sup: Some(cont(x, "sup")?), limits: Limits::Default }]),
        "sSub" => Ok(vec![Node::Scripts { base: Box::new(single_or_group(cont(x, "e")?)), sub: Some(cont(x, "sub")?), sup: None, limits: Limits::Default }]),
        "sSubSup" => Ok(vec![Node::Scripts {
            base: Box::new(single_or_group(cont(x, "e")?)),
            sub: Some(cont(x, "sub")?),
            sup: Some(cont(x, "sup")?),
            limits: Limits::Default,
        }]),
        "nary" => {
            let pr = x.kid("naryPr");
            let c = chr_val(pr, "chr", Some('∫')).ok_or(())?;
            let sub = (!on(pr.and_then(|p| p.val("subHide")))).then(|| cont(x, "sub")).transpose()?;
            let sup = (!on(pr.and_then(|p| p.val("supHide")))).then(|| cont(x, "sup")).transpose()?;
            let loc = pr.and_then(|p| p.val("limLoc")).unwrap_or_default();
            let limits = match (loc.as_str(), is_integral(c)) {
                ("undOvr", true) => Limits::Limits,
                ("subSup", false) => Limits::NoLimits,
                _ => Limits::Default,
            };
            let mut out = if sub.is_none() && sup.is_none() {
                vec![Node::Sym(c, Class::Op)]
            } else {
                vec![Node::Scripts { base: Box::new(Node::Sym(c, Class::Op)), sub, sup, limits }]
            };
            out.extend(cont(x, "e")?);
            Ok(out)
        }
        "func" => {
            let mut out = cont(x, "fName")?;
            out.extend(cont(x, "e")?);
            Ok(out)
        }
        "d" => delim(x),
        "m" => {
            let mut rows = Vec::new();
            for mr in x.kids.iter().filter(|k| k.pre == "m" && k.name == "mr") {
                let mut cells = Vec::new();
                for e in mr.kids.iter().filter(|k| k.pre == "m" && k.name == "e") {
                    cells.push(nodes_of(&e.kids)?);
                }
                rows.push(cells);
            }
            Ok(vec![Node::Matrix { env: MatrixEnv::Matrix, rows, cols: String::new() }])
        }
        "eqArr" => {
            let mut rows = Vec::new();
            for e in x.kids.iter().filter(|k| k.pre == "m" && k.name == "e") {
                rows.push(cells_of(e)?);
            }
            let aligned = rows.iter().any(|r| r.len() > 1);
            Ok(vec![Node::Matrix { env: if aligned { MatrixEnv::Aligned } else { MatrixEnv::Gathered }, rows, cols: String::new() }])
        }
        "acc" => {
            let c = chr_val(x.kid("accPr"), "chr", Some('\u{302}')).ok_or(())?;
            let body = cont(x, "e")?;
            let wide = body.len() > 1;
            let kind = match c {
                '\u{302}' | '^' | 'ˆ' => if wide { AccentKind::WideHat } else { AccentKind::Hat },
                '\u{303}' | '~' | '˜' => if wide { AccentKind::WideTilde } else { AccentKind::Tilde },
                '\u{304}' | '\u{305}' | '¯' => AccentKind::Bar,
                '\u{307}' | '˙' => AccentKind::Dot,
                '\u{308}' | '¨' => AccentKind::DDot,
                '\u{30C}' | 'ˇ' => AccentKind::Check,
                '\u{306}' | '˘' => AccentKind::Breve,
                '\u{301}' | '´' => AccentKind::Acute,
                '\u{300}' | '`' => AccentKind::Grave,
                '\u{30A}' | '˚' => AccentKind::Ring,
                '\u{20D7}' | '→' => if wide { AccentKind::OverRightArrow } else { AccentKind::Vec },
                '\u{20D6}' | '←' => AccentKind::OverLeftArrow,
                _ => return Err(()),
            };
            Ok(vec![Node::Accent { kind, body }])
        }
        "bar" => {
            let top = x.kid("barPr").and_then(|p| p.val("pos")).as_deref() == Some("top");
            Ok(vec![Node::Line { over: top, body: cont(x, "e")? }])
        }
        "groupChr" => {
            let pr = x.kid("groupChrPr");
            let c = chr_val(pr, "chr", Some('\u{23DF}')).ok_or(())?;
            let over = match c {
                '\u{23DE}' => true,
                '\u{23DF}' => false,
                _ => return Err(()),
            };
            Ok(vec![Node::Brace { over, body: cont(x, "e")? }])
        }
        "limLow" | "limUpp" => {
            let over = x.name == "limUpp";
            let e = cont(x, "e")?;
            let lim = cont(x, "lim")?;
            Ok(vec![lim_node(over, e, lim)])
        }
        _ => Err(()),
    }
}

fn is_integral_like(c: char) -> bool {
    is_integral(c)
}

/// `limLow`/`limUpp`: gränser på en funktion, klammer eller operator, annars en `\overset`/`\underset`.
fn lim_node(over: bool, e: Vec<Node>, lim: Vec<Node>) -> Node {
    if let [only] = e.as_slice() {
        match only {
            Node::Brace { over: b, .. } if *b == over => {
                return Node::Scripts {
                    base: Box::new(only.clone()),
                    sub: (!over).then_some(lim.clone()),
                    sup: over.then_some(lim),
                    limits: Limits::Default,
                };
            }
            Node::Func { .. } => {
                return Node::Scripts {
                    base: Box::new(only.clone()),
                    sub: (!over).then_some(lim.clone()),
                    sup: over.then_some(lim),
                    limits: Limits::Default,
                };
            }
            Node::Scripts { base, sub, sup, limits } if matches!(&**base, Node::Func { .. } | Node::Brace { .. }) => {
                // limUpp runt limLow: båda gränserna.
                if over && sup.is_none() {
                    return Node::Scripts { base: base.clone(), sub: sub.clone(), sup: Some(lim), limits: *limits };
                }
                if !over && sub.is_none() {
                    return Node::Scripts { base: base.clone(), sub: Some(lim), sup: sup.clone(), limits: *limits };
                }
            }
            Node::Sym(c, Class::Op) if !is_integral_like(*c) => {
                return Node::Scripts {
                    base: Box::new(only.clone()),
                    sub: (!over).then_some(lim.clone()),
                    sup: over.then_some(lim),
                    limits: Limits::Limits,
                };
            }
            _ => {}
        }
    }
    Node::Stack { over, note: lim, base: e }
}

fn delim(x: &X) -> R<Vec<Node>> {
    let pr = x.kid("dPr");
    let open = chr_val(pr, "begChr", Some('('));
    let close = chr_val(pr, "endChr", Some(')'));
    let sep = chr_val(pr, "sepChr", Some('|')).unwrap_or('|');
    let mut parts: Vec<Vec<Node>> = Vec::new();
    for e in x.kids.iter().filter(|k| k.pre == "m" && k.name == "e") {
        parts.push(nodes_of(&e.kids)?);
    }
    if parts.len() == 1 {
        let only = &parts[0];
        // `\begin{pmatrix}` & co: avgränsare runt en enda matris.
        if let [Node::Matrix { env: MatrixEnv::Matrix, rows, cols }] = only.as_slice() {
            let env = match (open, close) {
                (Some('('), Some(')')) => Some(MatrixEnv::PMatrix),
                (Some('['), Some(']')) => Some(MatrixEnv::BMatrix),
                (Some('{'), Some('}')) => Some(MatrixEnv::BbMatrix),
                (Some('|'), Some('|')) => Some(MatrixEnv::VMatrix),
                (Some('‖'), Some('‖')) => Some(MatrixEnv::VvMatrix),
                (Some('{'), None) => Some(MatrixEnv::Cases),
                _ => None,
            };
            if let Some(env) = env {
                return Ok(vec![Node::Matrix { env, rows: rows.clone(), cols: cols.clone() }]);
            }
        }
        if let [Node::Matrix { env: env @ (MatrixEnv::Aligned | MatrixEnv::Gathered), rows, cols }] = only.as_slice() {
            if (open, close) == (Some('{'), None) {
                return Ok(vec![Node::Matrix { env: if *env == MatrixEnv::Gathered { MatrixEnv::Gathered } else { MatrixEnv::Cases }, rows: rows.clone(), cols: cols.clone() }]);
            }
        }
        if let [Node::Frac { kind: FracKind::Binom, .. }] = only.as_slice() {
            if (open, close) == (Some('('), Some(')')) {
                return Ok(parts.remove(0));
            }
        }
    }
    let mut body = Vec::new();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            body.push(Node::Middle(sep));
        }
        body.extend(p);
    }
    Ok(vec![Node::Delim { open, close, body }])
}

/// Celler i en rad i `m:eqArr`: justeringspunkter (`m:aln`) delar raden.
fn cells_of(e: &X) -> R<Vec<Vec<Node>>> {
    let mut cells: Vec<Vec<Node>> = Vec::new();
    let mut cur: Vec<Node> = Vec::new();
    for k in &e.kids {
        let aln = k.pre == "m" && k.name == "r" && k.kid("rPr").is_some_and(|p| p.kid("aln").is_some());
        if aln {
            cells.push(std::mem::take(&mut cur));
        }
        cur.extend(elem(k)?);
    }
    cells.push(cur);
    Ok(cells)
}

fn unstyle(c: char) -> Option<(FontStyle, char)> {
    let u = c as u32;
    let ranges: [(FontStyle, u32, u32); 8] = [
        (FontStyle::It, 0x1D434, 0x1D44E),
        (FontStyle::Bf, 0x1D400, 0x1D41A),
        (FontStyle::BfIt, 0x1D468, 0x1D482),
        (FontStyle::Cal, 0x1D49C, 0x1D4B6),
        (FontStyle::Frak, 0x1D504, 0x1D51E),
        (FontStyle::Bb, 0x1D538, 0x1D552),
        (FontStyle::Sf, 0x1D5A0, 0x1D5BA),
        (FontStyle::Tt, 0x1D670, 0x1D68A),
    ];
    for (st, cap, small) in ranges {
        if (cap..cap + 26).contains(&u) {
            return char::from_u32('A' as u32 + (u - cap)).map(|a| (st, a));
        }
        if (small..small + 26).contains(&u) {
            return char::from_u32('a' as u32 + (u - small)).map(|a| (st, a));
        }
    }
    Some(match c {
        'ℎ' => (FontStyle::It, 'h'),
        'ℝ' => (FontStyle::Bb, 'R'),
        'ℂ' => (FontStyle::Bb, 'C'),
        'ℕ' => (FontStyle::Bb, 'N'),
        'ℤ' => (FontStyle::Bb, 'Z'),
        'ℚ' => (FontStyle::Bb, 'Q'),
        'ℙ' => (FontStyle::Bb, 'P'),
        'ℍ' => (FontStyle::Bb, 'H'),
        'ℒ' => (FontStyle::Cal, 'L'),
        'ℋ' => (FontStyle::Cal, 'H'),
        'ℬ' => (FontStyle::Cal, 'B'),
        'ℰ' => (FontStyle::Cal, 'E'),
        'ℱ' => (FontStyle::Cal, 'F'),
        'ℳ' => (FontStyle::Cal, 'M'),
        'ℛ' => (FontStyle::Cal, 'R'),
        _ => return None,
    })
}

fn run(x: &X) -> R<Vec<Node>> {
    let pr = x.kid("rPr");
    let nor = pr.is_some_and(|p| p.kid("nor").is_some());
    let sty = pr.and_then(|p| p.val("sty")).unwrap_or_default();
    let scr = pr.and_then(|p| p.val("scr")).unwrap_or_default();
    let text: String = x.kids.iter().filter(|k| k.pre == "m" && k.name == "t").map(|k| k.text.as_str()).collect();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    if nor {
        return Ok(vec![Node::Text(text)]);
    }
    let base_font = match scr.as_str() {
        "script" => FontStyle::Cal,
        "fraktur" => FontStyle::Frak,
        "double-struck" => FontStyle::Bb,
        "sans-serif" => FontStyle::Sf,
        "monospace" => FontStyle::Tt,
        _ => match sty.as_str() {
            "b" => FontStyle::Bf,
            "bi" => FontStyle::BfIt,
            "p" => FontStyle::Rm,
            _ => FontStyle::Auto,
        },
    };
    let chars: Vec<char> = text.chars().collect();
    // Upprätt flerbokstavstext är ett funktionsnamn (`sin`, `lim`, `rank`).
    if sty == "p" && scr.is_empty() && chars.len() >= 2 && chars.iter().all(|c| c.is_alphabetic()) {
        let name: String = chars.iter().collect();
        let limits = super::symbols::function_limits(&name).unwrap_or(false);
        return Ok(vec![Node::Func { name, limits }]);
    }
    let mut out: Vec<Node> = Vec::new();
    let wrap = |n: Node, font: FontStyle| -> Node {
        if matches!(font, FontStyle::Auto | FontStyle::It) { n } else { Node::Font { style: font, body: vec![n] } }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let mut n = String::new();
            while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()) && !n.contains('.'))) {
                n.push(chars[i]);
                i += 1;
            }
            out.push(if matches!(base_font, FontStyle::Rm) { Node::Num(n) } else { wrap(Node::Num(n), base_font) });
            continue;
        }
        i += 1;
        match c {
            '\u{2009}' => out.push(Node::Space(Space::Thin)),
            '\u{205F}' => out.push(Node::Space(Space::Med)),
            '\u{2005}' => out.push(Node::Space(Space::Thick)),
            '\u{2003}' => {
                if chars.get(i) == Some(&'\u{2003}') {
                    i += 1;
                    out.push(Node::Space(Space::QQuad));
                } else {
                    out.push(Node::Space(Space::Quad));
                }
            }
            '\u{A0}' | ' ' => out.push(Node::Space(Space::Normal)),
            c => {
                if let Some((st, a)) = unstyle(c) {
                    let font = if st == FontStyle::It { base_font } else { st };
                    out.push(wrap(Node::Var(a), font));
                } else if let n @ Node::Var(_) = parse::char_node(c) {
                    out.push(wrap(n, base_font));
                } else {
                    out.push(parse::char_node(c));
                }
            }
        }
    }
    // Närliggande tecken i samma stil delar en `\mathbf{…}`.
    let mut merged: Vec<Node> = Vec::new();
    for n in out {
        if let (Some(Node::Font { style: a, body: ab }), Node::Font { style: b, body: bb }) = (merged.last_mut(), &n) {
            if a == b {
                ab.extend(bb.clone());
                continue;
            }
        }
        merged.push(n);
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::parse::parse;

    fn omml(src: &str, display: bool) -> String {
        to_omml(&parse(src).unwrap_or_else(|e| panic!("{src}: {e}")), display)
    }

    fn back(src: &str) -> String {
        let x = omml(src, false);
        let (l, _) = to_latex(&x).unwrap_or_else(|| panic!("läser inte tillbaka: {src}\n{x}"));
        l
    }

    fn same(src: &str) {
        let a = parse(src).unwrap();
        let l = back(src);
        let b = parse(&l).unwrap_or_else(|e| panic!("{src} -> {l}: {e}"));
        assert_eq!(a, b, "{src} -> {l}");
    }

    #[test]
    fn writes_the_core_elements() {
        let x = omml(r"x^2+\frac{a}{b}", false);
        assert!(x.starts_with("<m:oMath xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\">"));
        assert!(x.contains("<m:sSup>") && x.contains("<m:f>") && x.contains("<m:num>"));
        assert!(x.ends_with("</m:oMath>"));
        let d = omml("x", true);
        assert!(d.starts_with("<m:oMathPara") && d.ends_with("</m:oMathPara>"));
        assert!(omml(r"\sqrt[3]{x}", false).contains("<m:deg><m:r>"));
        assert!(omml(r"\sqrt{x}", false).contains("<m:degHide m:val=\"1\"/>"));
        assert!(omml(r"\sum_{i=1}^n i", false).contains("<m:nary>"));
        assert!(omml(r"\left( x \right)", false).contains("<m:begChr m:val=\"(\"/>"));
        assert!(omml(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}", false).contains("<m:m>"));
        assert!(omml(r"\hat{x}", false).contains("<m:acc>"));
        assert!(omml(r"a<b", false).contains("a&lt;b"));
    }

    #[test]
    fn round_trips_the_common_constructs() {
        for s in [
            "x+y=z",
            "x^2+y_i^3",
            r"e^{i\pi}+1=0",
            r"\frac{a+b}{c}",
            r"\sqrt{x^2+1}",
            r"\sqrt[3]{x}",
            r"\sum_{i=1}^{n} i^2",
            r"\int_0^1 f(x)\,dx",
            r"\prod_{k} a_k",
            r"\lim_{x\to 0} f(x)",
            r"\left( \frac{a}{b} \right)",
            r"\left[ x \right)",
            r"\binom{n}{k}",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}",
            r"\begin{cases} 1 & x>0 \\ 0 & x\le 0 \end{cases}",
            r"\hat{x}+\vec{v}+\bar{y}",
            r"\widehat{abc}",
            r"\overline{x}+\underline{y}",
            r"\mathbb{R}^n+\mathbf{v}",
            r"\sin x+\cos y",
            r"\alpha+\beta\leq\infty",
            r"\text{om } x",
            r"a\quad b",
            r"\underbrace{a+b}_{n}",
            r"\overset{def}{=}",
            r"\left\{ x \middle| x>0 \right\}",
        ] {
            same(s);
        }
    }

    #[test]
    fn aligned_equations_keep_their_alignment_points() {
        let l = back(r"\begin{aligned} a &= b \\ &= c \end{aligned}");
        let n = parse(&l).unwrap();
        match &n[0] {
            Node::Matrix { env: MatrixEnv::Aligned, rows, .. } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
                assert_eq!(rows[1].len(), 2);
                assert!(rows[1][0].is_empty());
            }
            n => panic!("{n:?}"),
        }
    }

    #[test]
    fn reads_what_word_writes() {
        // Utdrag ur hur Word skriver x^2 + 1/2 med egna egenskapselement.
        let xml = "<m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\" xmlns:w=\"w\"><m:oMath>\
            <m:sSup><m:sSupPr><m:ctrlPr><w:rPr><w:rFonts w:ascii=\"Cambria Math\"/><w:i/></w:rPr></m:ctrlPr></m:sSupPr>\
            <m:e><m:r><w:rPr><w:rFonts w:ascii=\"Cambria Math\" w:hAnsi=\"Cambria Math\"/></w:rPr><m:t>x</m:t></m:r></m:e>\
            <m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>\
            <m:r><m:t>+</m:t></m:r>\
            <m:f><m:fPr><m:ctrlPr/></m:fPr><m:num><m:r><m:t>1</m:t></m:r></m:num><m:den><m:r><m:t>2</m:t></m:r></m:den></m:f>\
            </m:oMath></m:oMathPara>";
        let (l, display) = to_latex(xml).unwrap();
        assert!(display);
        assert_eq!(l, r"x^2+\frac{1}{2}");
        // Word: funktion med argument.
        let f = "<m:oMath xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><m:func><m:fName><m:r><m:rPr><m:sty m:val=\"p\"/></m:rPr><m:t>sin</m:t></m:r></m:fName><m:e><m:r><m:t>x</m:t></m:r></m:e></m:func></m:oMath>";
        assert_eq!(to_latex(f).unwrap().0, r"\sin x");
        // Integral med operand.
        let i = "<m:oMath xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><m:nary><m:naryPr><m:chr m:val=\"∫\"/></m:naryPr><m:sub><m:r><m:t>0</m:t></m:r></m:sub><m:sup><m:r><m:t>1</m:t></m:r></m:sup><m:e><m:r><m:t>f</m:t></m:r></m:e></m:nary></m:oMath>";
        assert_eq!(to_latex(i).unwrap().0, r"\int_0^1f");
    }

    #[test]
    fn unsupported_or_broken_omml_is_refused() {
        let ns = "xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"";
        assert!(to_latex(&format!("<m:oMath {ns}><m:phant><m:e/></m:phant></m:oMath>")).is_none());
        assert!(to_latex(&format!("<m:oMath {ns}><m:sPre><m:e/></m:sPre></m:oMath>")).is_none());
        assert!(to_latex("<m:oMath xmlns:m=\"x\"><m:f>").is_none());
        assert!(to_latex("<w:p/>").is_none());
        assert!(to_latex(&format!("<m:oMath {ns}/>")).is_none());
    }
}
