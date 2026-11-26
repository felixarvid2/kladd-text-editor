//! Förlagets formatering, förenklad och säker: bokens stilmallar (CSS-filer, `<style>` och `style`-attribut)
//! läses för bara det som ändrar hur texten läses: `text-align`, `text-indent`, `margin-left`,
//! `font-variant: small-caps`, `font-style` och `font-weight`. Selektorer är element, `.klass`,
//! `element.klass`, `#id` och enkla efterkommande (`div.x p`); allt annat (pseudoklasser, `>`, `+`,
//! attributväljare, @-regler) hoppas över. Inga färger, typsnitt eller absoluta storlekar tas med.

use crate::paste::El;

use super::read::local;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

/// Det en regel (eller ett `style`-attribut) sätter. `None`: inget sagt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decl {
    pub align: Option<Align>,
    /// Första radens indrag i `em`.
    pub indent: Option<f32>,
    /// Indrag från vänster i `em`.
    pub margin_left: Option<f32>,
    pub small_caps: Option<bool>,
    pub italic: Option<bool>,
    pub bold: Option<bool>,
}

impl Decl {
    /// Låter `o` gälla där den har ett värde.
    pub fn merge(&mut self, o: &Decl) {
        macro_rules! m {
            ($f:ident) => {
                if o.$f.is_some() {
                    self.$f = o.$f;
                }
            };
        }
        m!(align);
        m!(indent);
        m!(margin_left);
        m!(small_caps);
        m!(italic);
        m!(bold);
    }

    pub fn is_empty(&self) -> bool {
        *self == Decl::default()
    }
}

/// Ett element som det ser ut för väljarna.
#[derive(Clone, Debug, Default)]
pub struct ElInfo {
    pub tag: String,
    pub classes: Vec<String>,
    pub id: Option<String>,
}

impl ElInfo {
    pub fn of(e: &El) -> ElInfo {
        ElInfo {
            tag: local(&e.tag).to_ascii_lowercase(),
            classes: e.attr("class").map(|c| c.split_whitespace().map(str::to_owned).collect()).unwrap_or_default(),
            id: e.attr("id").map(str::to_owned),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Simple {
    tag: Option<String>,
    classes: Vec<String>,
    id: Option<String>,
}

impl Simple {
    fn matches(&self, e: &ElInfo) -> bool {
        self.tag.as_ref().is_none_or(|t| *t == e.tag)
            && self.id.as_ref().is_none_or(|i| e.id.as_ref() == Some(i))
            && self.classes.iter().all(|c| e.classes.contains(c))
    }
}

#[derive(Clone, Debug)]
struct Rule {
    /// Efterkommande-kedjan: sista delen är elementet självt.
    chain: Vec<Simple>,
    decl: Decl,
    spec: (u32, u32, u32),
    order: usize,
}

/// Bokens regler (alla stilmallar ett kapitel länkar till, i ordning).
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    rules: Vec<Rule>,
}

impl Sheet {
    pub fn parse(css: &str) -> Sheet {
        let mut s = Sheet::default();
        s.add(css);
        s
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Lägger till reglerna i `css` efter de som finns.
    pub fn add(&mut self, css: &str) {
        let css = strip_comments(css);
        let mut rest = css.as_str();
        while let Some(open) = rest.find(['{', ';']) {
            let head = rest[..open].trim();
            if rest.as_bytes()[open] == b';' {
                // `@import …;` och liknande utan block.
                rest = &rest[open + 1..];
                continue;
            }
            let Some(close) = matching_brace(rest, open) else { break };
            let body = &rest[open + 1..close];
            rest = &rest[close + 1..];
            if head.starts_with('@') || head.is_empty() {
                continue; // @media, @font-face, @page …: ignoreras
            }
            let decl = parse_decls(body);
            if decl.is_empty() {
                continue;
            }
            for sel in head.split(',') {
                if let Some((chain, spec)) = parse_selector(sel) {
                    let order = self.rules.len();
                    self.rules.push(Rule { chain, decl: decl.clone(), spec, order });
                }
            }
        }
    }

    pub fn extend(&mut self, other: &Sheet) {
        let base = self.rules.len();
        for r in &other.rules {
            let mut r = r.clone();
            r.order += base;
            self.rules.push(r);
        }
    }

    /// Reglerna som gäller `el` (med förfäderna, yttersta först), i kaskadordning.
    pub fn declared(&self, el: &ElInfo, ancestors: &[ElInfo]) -> Decl {
        let mut hits: Vec<&Rule> = self.rules.iter().filter(|r| chain_matches(&r.chain, el, ancestors)).collect();
        hits.sort_by_key(|r| (r.spec, r.order));
        let mut d = Decl::default();
        for r in hits {
            d.merge(&r.decl);
        }
        d
    }
}

fn chain_matches(chain: &[Simple], el: &ElInfo, ancestors: &[ElInfo]) -> bool {
    let Some((last, before)) = chain.split_last() else { return false };
    if !last.matches(el) {
        return false;
    }
    // Resten måste finnas bland förfäderna, i ordning (grådigt bakifrån).
    let mut at = ancestors.len();
    for part in before.iter().rev() {
        loop {
            if at == 0 {
                return false;
            }
            at -= 1;
            if part.matches(&ancestors[at]) {
                break;
            }
        }
    }
    true
}

/// Innehållet i `<style>`-element i en sida (tolken i `paste.rs` behåller inte dem).
pub fn inline_styles(raw: &str) -> Vec<String> {
    let lower = raw.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(p) = lower[at..].find("<style") {
        let open = at + p;
        let Some(gt) = lower[open..].find('>') else { break };
        let from = open + gt + 1;
        let Some(end) = lower[from..].find("</style") else { break };
        out.push(raw[from..from + end].to_owned());
        at = from + end;
    }
    out
}

/// `style`-attributets deklarationer.
pub fn inline_decl(style: &str) -> Decl {
    parse_decls(style)
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(p) = rest.find("/*") {
        out.push_str(&rest[..p]);
        rest = match rest[p + 2..].find("*/") {
            Some(q) => &rest[p + 2 + q + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// Index för den `}` som hör till `{` vid `open`.
fn matching_brace(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

fn ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
}

/// En enkel väljare: `tag`, `.klass`, `tag.klass`, `#id`, `tag#id.klass`.
fn parse_simple(s: &str) -> Option<Simple> {
    let mut out = Simple::default();
    let mut rest = s;
    let first_end = rest.find(['.', '#']).unwrap_or(rest.len());
    let tag = &rest[..first_end];
    if !tag.is_empty() && tag != "*" {
        let tag = tag.rsplit('|').next().unwrap_or(tag); // namnrymd
        if !ident(tag) {
            return None;
        }
        out.tag = Some(tag.to_ascii_lowercase());
    }
    rest = &rest[first_end..];
    while !rest.is_empty() {
        let kind = rest.as_bytes()[0];
        let body = &rest[1..];
        let end = body.find(['.', '#']).unwrap_or(body.len());
        let name = &body[..end];
        if !ident(name) {
            return None;
        }
        match kind {
            b'.' => out.classes.push(name.to_owned()),
            _ => out.id = Some(name.to_owned()),
        }
        rest = &body[end..];
    }
    (out != Simple::default() || s == "*").then_some(out)
}

fn parse_selector(sel: &str) -> Option<(Vec<Simple>, (u32, u32, u32))> {
    let sel = sel.trim();
    if sel.is_empty() || sel.contains(['>', '+', '~', '[', ':', '(', '\\']) {
        return None;
    }
    let mut chain = Vec::new();
    let mut spec = (0, 0, 0);
    for part in sel.split_whitespace() {
        let s = parse_simple(part)?;
        spec.0 += u32::from(s.id.is_some());
        spec.1 += s.classes.len() as u32;
        spec.2 += u32::from(s.tag.is_some());
        chain.push(s);
    }
    (!chain.is_empty() && chain.len() <= 4).then_some((chain, spec))
}

/// Längd i `em` (px räknas mot 16, pt mot 12). Negativa och oanvändbara värden ger `None`.
fn length_em(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let split = v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(v.len());
    let (num, unit) = v.split_at(split);
    let n: f32 = num.parse().ok()?;
    let em = match unit.trim() {
        "em" | "rem" => n,
        "px" => n / 16.0,
        "pt" => n / 12.0,
        "ex" | "ch" => n * 0.5,
        "" if n == 0.0 => 0.0,
        _ => return None,
    };
    (em >= 0.0).then_some(em.min(10.0))
}

fn parse_decls(body: &str) -> Decl {
    let mut d = Decl::default();
    for decl in body.split(';') {
        let Some((prop, val)) = decl.split_once(':') else { continue };
        let prop = prop.trim().to_ascii_lowercase();
        let val = val.replace("!important", "");
        let val = val.trim().to_ascii_lowercase();
        match prop.as_str() {
            "text-align" => {
                d.align = match val.as_str() {
                    "left" | "start" => Some(Align::Left),
                    "center" => Some(Align::Center),
                    "right" | "end" => Some(Align::Right),
                    "justify" => Some(Align::Justify),
                    _ => d.align,
                }
            }
            "text-indent" => d.indent = length_em(val.split_whitespace().next().unwrap_or("")).or(d.indent),
            "margin-left" | "padding-left" | "margin-inline-start" | "padding-inline-start" => {
                d.margin_left = length_em(&val).or(d.margin_left);
            }
            "margin" | "padding" => {
                let v: Vec<&str> = val.split_whitespace().collect();
                let left = match v.len() {
                    1 => Some(v[0]),
                    2 | 3 => Some(v[1]),
                    4 => Some(v[3]),
                    _ => None,
                };
                if let Some(l) = left {
                    // Ett `margin` utan vänstermarginal påverkar inte `padding` och tvärtom: de läggs ihop av anroparen.
                    d.margin_left = length_em(l).or(d.margin_left);
                }
            }
            "font-variant" | "font-variant-caps" => {
                if val.contains("small-caps") {
                    d.small_caps = Some(true);
                } else if val == "normal" {
                    d.small_caps = Some(false);
                }
            }
            "font-style" => match val.as_str() {
                "italic" | "oblique" => d.italic = Some(true),
                "normal" => d.italic = Some(false),
                _ => {}
            },
            "font-weight" => match val.as_str() {
                "bold" | "bolder" => d.bold = Some(true),
                "normal" | "lighter" => d.bold = Some(false),
                n => {
                    if let Ok(n) = n.parse::<u32>() {
                        d.bold = Some(n >= 600);
                    }
                }
            },
            _ => {}
        }
    }
    d
}

/// Det som gäller ett element efter kaskaden, med det som ärvs från föräldern.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Computed {
    pub align: Option<Align>,
    pub indent: f32,
    /// Summan av vänstermarginalerna från omgivande element.
    pub margin_left: f32,
    pub small_caps: bool,
    pub italic: bool,
    pub bold: bool,
}

impl Computed {
    /// Ärvda värden från föräldern och därefter det elementet själv säger.
    pub fn child(&self, own: &Decl) -> Computed {
        Computed {
            align: own.align.or(self.align),
            indent: own.indent.unwrap_or(self.indent),
            margin_left: (self.margin_left + own.margin_left.unwrap_or(0.0)).min(12.0),
            small_caps: own.small_caps.unwrap_or(self.small_caps),
            italic: own.italic.unwrap_or(self.italic),
            bold: own.bold.unwrap_or(self.bold),
        }
    }

    /// Är något sagt som ändrar hur blocket ritas (inte fetstil/kursiv, som följer med i texten)?
    pub fn block_style(&self) -> Option<BlockLook> {
        let l = BlockLook { align: self.align, indent: self.indent, margin_left: self.margin_left, small_caps: self.small_caps };
        (l != BlockLook::default()).then_some(l)
    }
}

/// Blockets utseende som läsvyn ritar det (se `egui_commonmark_backend::misc::BlockStyle`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockLook {
    pub align: Option<Align>,
    pub indent: f32,
    pub margin_left: f32,
    pub small_caps: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(tag: &str, classes: &[&str], id: Option<&str>) -> ElInfo {
        ElInfo { tag: tag.into(), classes: classes.iter().map(|c| (*c).into()).collect(), id: id.map(Into::into) }
    }

    #[test]
    fn väljare_element_klass_id_och_efterkommande() {
        let s = Sheet::parse(
            "p { text-indent: 1em; margin: 0 }\n.center { text-align: center }\np.note { text-indent: 0 }\n#x { text-align: right }\ndiv.q p { margin-left: 2em }",
        );
        let p = el("p", &[], None);
        assert_eq!(s.declared(&p, &[]).indent, Some(1.0));
        let c = el("p", &["center"], None);
        assert_eq!(s.declared(&c, &[]).align, Some(Align::Center));
        let n = el("p", &["note"], None);
        assert_eq!(s.declared(&n, &[]).indent, Some(0.0), "p.note vinner över p (högre specificitet)");
        assert_eq!(s.declared(&el("div", &[], Some("x")), &[]).align, Some(Align::Right));
        let anc = [el("body", &[], None), el("div", &["q"], None)];
        assert_eq!(s.declared(&p, &anc).margin_left, Some(2.0), "div.q p");
        assert_eq!(s.declared(&p, &[el("div", &["z"], None)]).margin_left, Some(0.0), "div.z ger inte div.q-regeln");
    }

    #[test]
    fn det_som_inte_stöds_ignoreras_utan_att_fälla_resten() {
        let s = Sheet::parse(
            "@import url(x.css);\n@font-face { font-family: F; src: url(f.ttf) }\n@media print { p { text-align: right } }\n\
             p:first-child { text-indent: 0 }\np > span { text-align: right }\nh1 + p { margin-left: 3em }\na[href] { font-style: italic }\n\
             /* kommentar { text-align: center } */\n.ok { color: red; font-family: Georgia; font-size: 20px; text-align: justify }",
        );
        assert_eq!(s.declared(&el("p", &[], None), &[el("body", &[], None)]), Decl::default());
        assert_eq!(s.declared(&el("p", &["ok"], None), &[]).align, Some(Align::Justify), "färg, typsnitt och storlek hoppas över");
    }

    #[test]
    fn längder_och_genvägar() {
        assert_eq!(length_em("1.5em"), Some(1.5));
        assert_eq!(length_em("16px"), Some(1.0));
        assert_eq!(length_em("12pt"), Some(1.0));
        assert_eq!(length_em("0"), Some(0.0));
        assert_eq!(length_em("-1em"), None, "hängande indrag ignoreras");
        assert_eq!(length_em("50%"), None);
        assert_eq!(length_em("auto"), None);
        let d = parse_decls("margin: 0 0 0 2em; text-indent: 1.2em; font-variant: small-caps; font-style: italic; font-weight: 700");
        assert_eq!((d.margin_left, d.indent, d.small_caps, d.italic, d.bold), (Some(2.0), Some(1.2), Some(true), Some(true), Some(true)));
        let d = parse_decls("margin: 1em 2em; font-weight: normal; font-variant: normal !important");
        assert_eq!((d.margin_left, d.bold, d.small_caps), (Some(2.0), Some(false), Some(false)));
        // Trasig CSS: inga panics.
        for bad in ["{", "}}}", "p {", "p { text-indent", ":;{}", "@media {", "\u{0}"] {
            let _ = Sheet::parse(bad);
        }
    }

    #[test]
    fn kaskad_ärver_justering_och_summerar_marginaler() {
        let root = Computed::default();
        let div = root.child(&Decl { align: Some(Align::Center), margin_left: Some(1.0), ..Decl::default() });
        let p = div.child(&Decl { margin_left: Some(2.0), indent: Some(1.0), ..Decl::default() });
        assert_eq!((p.align, p.margin_left, p.indent), (Some(Align::Center), 3.0, 1.0));
        assert!(root.block_style().is_none());
        assert!(p.block_style().is_some());
        // Fetstil och kursiv ensamma gör inget blockutseende.
        assert!(root.child(&Decl { italic: Some(true), bold: Some(true), ..Decl::default() }).block_style().is_none());
    }
}
