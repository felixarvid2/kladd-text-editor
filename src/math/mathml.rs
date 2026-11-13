//! Syntaxträd som MathML (HTML, EPUB och ODT-formelobjekt).

use super::ast::*;
use super::layout::styled;

pub const NS: &str = "http://www.w3.org/1998/Math/MathML";

/// Ett helt `<math>`-element med `xmlns` (för HTML och EPUB). `display`: egen rad.
pub fn math_element(nodes: &[Node], display: bool) -> String {
    let mut w = W { p: String::new(), out: String::new() };
    w.open("math", &format!(" xmlns=\"{NS}\"{}", if display { " display=\"block\"" } else { "" }));
    w.row(nodes, FontStyle::Auto);
    w.close("math");
    w.out
}

/// Innehållet (utan `<math>`) med elementnamn prefixerade, t.ex. `math:` i ODF.
pub fn body(nodes: &[Node], prefix: &str) -> String {
    let mut w = W { p: prefix.to_owned(), out: String::new() };
    w.row(nodes, FontStyle::Auto);
    w.out
}

struct W {
    p: String,
    out: String,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn is_integral(c: char) -> bool {
    matches!(c, '∫' | '∬' | '∭' | '∮' | '∯')
}

fn accent_char(k: AccentKind) -> char {
    match k {
        AccentKind::Hat | AccentKind::WideHat => '^',
        AccentKind::Check => '\u{2C7}',
        AccentKind::Tilde | AccentKind::WideTilde => '~',
        AccentKind::Acute => '\u{B4}',
        AccentKind::Grave => '`',
        AccentKind::Dot => '\u{2D9}',
        AccentKind::DDot => '\u{A8}',
        AccentKind::Breve => '\u{2D8}',
        AccentKind::Bar => '\u{AF}',
        AccentKind::Vec | AccentKind::OverRightArrow => '→',
        AccentKind::Ring => '\u{2DA}',
        AccentKind::OverLeftArrow => '←',
    }
}

impl W {
    fn open(&mut self, tag: &str, attrs: &str) {
        let p = if tag == "math" { String::new() } else { self.p.clone() };
        self.out.push_str(&format!("<{p}{tag}{attrs}>"));
    }

    fn close(&mut self, tag: &str) {
        let p = if tag == "math" { String::new() } else { self.p.clone() };
        self.out.push_str(&format!("</{p}{tag}>"));
    }

    fn leaf(&mut self, tag: &str, attrs: &str, text: &str) {
        self.open(tag, attrs);
        self.out.push_str(&esc(text));
        self.close(tag);
    }

    fn empty(&mut self, tag: &str, attrs: &str) {
        self.out.push_str(&format!("<{}{tag}{attrs}/>", self.p));
    }

    /// En `<mrow>` av noderna, eller bara barnet om det är ett.
    fn row(&mut self, nodes: &[Node], font: FontStyle) {
        match nodes {
            [one] => self.node(one, font),
            _ => {
                self.open("mrow", "");
                self.list(nodes, font);
                self.close("mrow");
            }
        }
    }

    fn list(&mut self, nodes: &[Node], font: FontStyle) {
        for n in nodes {
            self.node(n, font);
        }
    }

    fn styled_text(&self, s: &str, font: FontStyle, var: bool) -> String {
        s.chars().map(|c| styled(c, font, var)).collect()
    }

    fn mo(&mut self, c: char, attrs: &str) {
        self.leaf("mo", attrs, &c.to_string());
    }

    fn node(&mut self, n: &Node, font: FontStyle) {
        match n {
            Node::Num(s) => {
                let t = self.styled_text(s, font, false);
                self.leaf("mn", "", &t);
            }
            Node::Var(c) => {
                // Enstaka bokstäver i `<mi>` är kursiva av sig själva.
                let t = styled(*c, font, false);
                let attrs = if font == FontStyle::Auto || font == FontStyle::It { "" } else { " mathvariant=\"normal\"" };
                self.leaf("mi", attrs, &t.to_string());
            }
            Node::Sym(c, class) => {
                let c = styled(*c, if font == FontStyle::Auto { FontStyle::Auto } else { font }, false);
                match class {
                    Class::Ord if c.is_alphabetic() || matches!(c, '∞' | '∂' | 'ℏ' | 'ℓ' | '∅' | 'ℵ') => {
                        let attrs = if c.is_uppercase() { " mathvariant=\"normal\"" } else { "" };
                        self.leaf("mi", attrs, &c.to_string());
                    }
                    Class::Op => self.mo(c, if is_integral(c) { " largeop=\"true\"" } else { " largeop=\"true\" movablelimits=\"true\"" }),
                    Class::Open | Class::Close => self.mo(c, " stretchy=\"false\""),
                    _ => self.mo(c, ""),
                }
            }
            Node::Text(t) => {
                let t = self.styled_text(&t.replace(' ', "\u{A0}"), font, false);
                self.leaf("mtext", "", &t);
            }
            Node::Func { name, .. } => {
                self.leaf("mi", "", name);
                self.mo('\u{2061}', "");
            }
            Node::Group(l) => {
                self.open("mrow", "");
                self.list(l, font);
                self.close("mrow");
            }
            Node::Scripts { base, sub, sup, limits } => self.scripts(base, sub.as_deref(), sup.as_deref(), *limits, font),
            Node::Frac { kind, num, den } => {
                let wrap_style = match kind {
                    FracKind::DFrac => Some("true"),
                    FracKind::TFrac => Some("false"),
                    _ => None,
                };
                if let Some(d) = wrap_style {
                    self.open("mstyle", &format!(" displaystyle=\"{d}\""));
                }
                if *kind == FracKind::Binom {
                    self.open("mrow", "");
                    self.mo('(', "");
                }
                self.open("mfrac", if *kind == FracKind::Binom { " linethickness=\"0\"" } else { "" });
                self.row(num, font);
                self.row(den, font);
                self.close("mfrac");
                if *kind == FracKind::Binom {
                    self.mo(')', "");
                    self.close("mrow");
                }
                if wrap_style.is_some() {
                    self.close("mstyle");
                }
            }
            Node::Sqrt { index, body } => match index {
                Some(i) => {
                    self.open("mroot", "");
                    self.row(body, font);
                    self.row(i, font);
                    self.close("mroot");
                }
                None => {
                    self.open("msqrt", "");
                    self.list(body, font);
                    self.close("msqrt");
                }
            },
            Node::Delim { open, close, body } => {
                self.open("mrow", "");
                if let Some(c) = open {
                    self.mo(*c, "");
                }
                self.list(body, font);
                if let Some(c) = close {
                    self.mo(*c, "");
                }
                self.close("mrow");
            }
            Node::Middle(c) => self.mo(*c, ""),
            Node::BigDelim { ch, size, .. } => {
                if *ch != '.' {
                    let em = [1.2, 1.8, 2.4, 3.0][usize::from((*size).clamp(1, 4)) - 1];
                    self.mo(*ch, &format!(" stretchy=\"true\" minsize=\"{em}em\" maxsize=\"{em}em\""));
                }
            }
            Node::Matrix { env, rows, cols } => self.matrix(*env, rows, cols, font),
            Node::Accent { kind, body } => {
                self.open("mover", " accent=\"true\"");
                self.row(body, font);
                self.mo(accent_char(*kind), "");
                self.close("mover");
            }
            Node::Line { over, body } => {
                if *over {
                    self.open("mover", " accent=\"true\"");
                    self.row(body, font);
                    self.mo('\u{AF}', "");
                    self.close("mover");
                } else {
                    self.open("munder", " accentunder=\"true\"");
                    self.row(body, font);
                    self.mo('_', "");
                    self.close("munder");
                }
            }
            Node::Brace { over, body } => {
                let tag = if *over { "mover" } else { "munder" };
                self.open(tag, "");
                self.row(body, font);
                self.mo(if *over { '\u{23DE}' } else { '\u{23DF}' }, "");
                self.close(tag);
            }
            Node::Stack { over, note, base } => {
                let tag = if *over { "mover" } else { "munder" };
                self.open(tag, "");
                self.row(base, font);
                self.row(note, font);
                self.close(tag);
            }
            Node::Font { style, body } => self.list(body, *style),
            Node::Space(s) => {
                let em = s.em();
                self.empty("mspace", &format!(" width=\"{:.4}em\"", em));
            }
            Node::Styled { style, body } => {
                let (d, lvl) = match style {
                    MStyle::Display => ("true", 0),
                    MStyle::Text => ("false", 0),
                    MStyle::Script => ("false", 1),
                    MStyle::ScriptScript => ("false", 2),
                };
                self.open("mstyle", &format!(" displaystyle=\"{d}\" scriptlevel=\"{lvl}\""));
                self.list(body, font);
                self.close("mstyle");
            }
            Node::Classed { body, .. } => self.row(body, font),
        }
    }

    fn scripts(&mut self, base: &Node, sub: Option<&[Node]>, sup: Option<&[Node]>, limits: Limits, font: FontStyle) {
        let natural = match base {
            Node::Sym(c, Class::Op) => !is_integral(*c),
            Node::Func { limits: true, .. } => true,
            _ => false,
        };
        let under = matches!(base, Node::Brace { .. }) || limits == Limits::Limits || (limits == Limits::Default && natural);
        let (tu, to, tuo) = if under { ("munder", "mover", "munderover") } else { ("msub", "msup", "msubsup") };
        let tag = match (sub.is_some(), sup.is_some()) {
            (true, true) => tuo,
            (true, false) => tu,
            (false, true) => to,
            (false, false) => {
                self.node(base, font);
                return;
            }
        };
        self.open(tag, "");
        self.node(base, font);
        if let Some(s) = sub {
            self.row(s, font);
        }
        if let Some(s) = sup {
            self.row(s, font);
        }
        self.close(tag);
    }

    fn matrix(&mut self, env: MatrixEnv, rows: &[Vec<Vec<Node>>], cols: &str, font: FontStyle) {
        let (open, close) = env.delims();
        if open.is_some() || close.is_some() {
            self.open("mrow", "");
        }
        if let Some(c) = open {
            self.mo(c, "");
        }
        let align = match env {
            MatrixEnv::Aligned => " columnalign=\"right left\" columnspacing=\"0em\"".to_owned(),
            MatrixEnv::Cases => " columnalign=\"left left\"".to_owned(),
            MatrixEnv::Array if !cols.is_empty() => {
                let names: Vec<&str> = cols.chars().map(|c| match c { 'l' => "left", 'r' => "right", _ => "center" }).collect();
                format!(" columnalign=\"{}\"", names.join(" "))
            }
            _ => String::new(),
        };
        let ds = if matches!(env, MatrixEnv::Aligned | MatrixEnv::Gathered) { " displaystyle=\"true\"" } else { "" };
        self.open("mtable", &format!("{align}{ds}"));
        for r in rows {
            self.open("mtr", "");
            for c in r {
                self.open("mtd", "");
                self.list(c, font);
                self.close("mtd");
            }
            self.close("mtr");
        }
        self.close("mtable");
        if let Some(c) = close {
            self.mo(c, "");
        }
        if open.is_some() || close.is_some() {
            self.close("mrow");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::parse::parse;

    fn ml(src: &str) -> String {
        math_element(&parse(src).unwrap(), false)
    }

    fn balanced(x: &str) -> bool {
        let mut r = quick_xml::Reader::from_str(x);
        let mut depth = 0i32;
        loop {
            match r.read_event() {
                Ok(quick_xml::events::Event::Start(_)) => depth += 1,
                Ok(quick_xml::events::Event::End(_)) => depth -= 1,
                Ok(quick_xml::events::Event::Eof) => return depth == 0,
                Err(_) => return false,
                _ => {}
            }
        }
    }

    #[test]
    fn basic_elements() {
        let m = ml(r"x^2+\frac{a}{b}");
        assert!(m.starts_with("<math xmlns=\"http://www.w3.org/1998/Math/MathML\">"));
        assert!(m.contains("<msup><mi>𝑥</mi><mn>2</mn></msup>") || m.contains("<msup><mi>x</mi><mn>2</mn></msup>"));
        assert!(m.contains("<mo>+</mo>"));
        assert!(m.contains("<mfrac><mi>"));
        assert!(m.ends_with("</math>"));
        assert!(balanced(&m));
    }

    #[test]
    fn display_and_roots_and_sums() {
        let d = math_element(&parse(r"\sqrt[3]{x}").unwrap(), true);
        assert!(d.contains("display=\"block\""));
        assert!(d.contains("<mroot>"));
        let s = ml(r"\sum_{i=1}^{n} i");
        assert!(s.contains("<munderover>"), "{s}");
        let i = ml(r"\int_0^1 f");
        assert!(i.contains("<msubsup>"), "{i}");
        assert!(ml(r"\sqrt{x}").contains("<msqrt>"));
    }

    #[test]
    fn matrices_text_and_escaping() {
        let m = ml(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}");
        assert_eq!(m.matches("<mtr>").count(), 2);
        assert_eq!(m.matches("<mtd>").count(), 4);
        assert!(m.contains("<mo>(</mo>") && m.contains("<mo>)</mo>"));
        let t = ml(r"a < b \text{ & } c");
        assert!(t.contains("&lt;") && t.contains("&amp;"));
        assert!(balanced(&t));
        let b = ml(r"\mathbf{v}");
        assert!(b.contains("𝐯"), "{b}");
    }

    #[test]
    fn odf_prefix() {
        let b = body(&parse(r"\frac12").unwrap(), "math:");
        assert!(b.starts_with("<math:mfrac>"));
        assert!(b.contains("<math:mn>1</math:mn>"));
    }

    #[test]
    fn all_constructs_stay_well_formed() {
        for s in [
            r"\left( x \middle| y \right]",
            r"\hat{x}\overline{y}\underline{z}\overbrace{ab}^{n}",
            r"\overset{a}{b}\underset{c}{d}",
            r"\begin{cases} 1 & x \\ 0 & y \end{cases}",
            r"\begin{aligned} a &= b \end{aligned}",
            r"\binom{n}{k}\dfrac{1}{2}",
            r"\displaystyle\sum_{i}x_i",
            r"\Big( x \Big)",
            r"\lim_{x\to0}\sin x",
            r"\text{a b}\quad\!\,",
        ] {
            assert!(balanced(&ml(s)), "{s}");
        }
    }
}
