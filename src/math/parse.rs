//! LaTeX-matematik (delmängden i `README.md`) till syntaxträd.

use super::ast::*;
use super::symbols;

/// Tolkar LaTeX-matematik. Fel (okänt kommando, saknad `}`) ger en kort beskrivning.
pub fn parse(src: &str) -> Result<Vec<Node>, String> {
    let mut p = P { s: src.chars().collect(), i: 0, depth: 0 };
    let list = p.list(false)?;
    match p.peek() {
        None => Ok(list),
        Some('}') => Err("oväntad }".into()),
        Some(']') => Ok(list),
        Some('&') => Err("& utanför en matris".into()),
        Some(_) => Err(p.stop_error()),
    }
}

struct P {
    s: Vec<char>,
    i: usize,
    depth: usize,
}

const MAX_DEPTH: usize = 64;

impl P {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.s.get(self.i + n).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.i += 1;
            } else if c == '%' {
                while self.peek().is_some_and(|c| c != '\n') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    /// Kommandonamnet som börjar på `\` vid `self.i` (utan att flytta).
    fn peek_command(&self) -> Option<String> {
        if self.peek() != Some('\\') {
            return None;
        }
        let mut j = self.i + 1;
        let mut name = String::new();
        while let Some(&c) = self.s.get(j) {
            if c.is_ascii_alphabetic() {
                name.push(c);
                j += 1;
            } else {
                break;
            }
        }
        if name.is_empty() {
            if let Some(&c) = self.s.get(j) {
                name.push(c);
            }
        }
        (!name.is_empty()).then_some(name)
    }

    fn eat_command(&mut self, name: &str) {
        self.i += 1 + name.chars().count();
    }

    fn stop_error(&self) -> String {
        match self.peek_command().as_deref() {
            Some("right") => "\\right utan \\left".into(),
            Some("end") => "\\end utan \\begin".into(),
            Some("\\") => "\\\\ utanför en matris".into(),
            _ => "oväntat tecken".into(),
        }
    }

    /// Är nästa token ett slut på listan (utan att flytta)?
    fn at_stop(&self, bracket: bool) -> bool {
        match self.peek() {
            None | Some('}') | Some('&') => true,
            Some(']') => bracket,
            Some('\\') => matches!(self.peek_command().as_deref(), Some("right" | "end" | "\\")),
            _ => false,
        }
    }

    /// Läser noder tills ett slut (`}`, `&`, `\\`, `\right`, `\end`, `]` om `bracket`, filslut) som lämnas oläst.
    fn list(&mut self, bracket: bool) -> Result<Vec<Node>, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("för djupt nästlat".into());
        }
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            if self.at_stop(bracket) {
                break;
            }
            if self.peek_command().as_deref() == Some("middle") {
                self.eat_command("middle");
                let d = self.delim()?.unwrap_or('|');
                out.push(Node::Middle(d));
                continue;
            }
            // `\displaystyle` & co gäller resten av gruppen.
            if let Some(style) = self.peek_command().as_deref().and_then(style_switch) {
                let n = self.peek_command().unwrap_or_default();
                self.eat_command(&n);
                let body = self.list(bracket)?;
                out.push(Node::Styled { style, body });
                break;
            }
            let atom = self.atom_with_scripts()?;
            out.push(atom);
        }
        self.depth -= 1;
        Ok(out)
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        self.skip_ws();
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("saknar {c}"))
        }
    }

    /// Ett argument: `{…}` eller en enda token.
    fn arg(&mut self) -> Result<Vec<Node>, String> {
        self.skip_ws();
        match self.peek() {
            Some('{') => {
                self.i += 1;
                let l = self.list(false)?;
                self.expect('}')?;
                Ok(l)
            }
            None => Err("argument saknas".into()),
            Some('}') | Some('&') => Err("argument saknas".into()),
            // Ett argument utan klammrar är en enda tecken: `\frac12` är 1 över 2.
            Some(d) if d.is_ascii_digit() => {
                self.i += 1;
                Ok(vec![Node::Num(d.to_string())])
            }
            Some(_) => Ok(vec![self.atom()?]),
        }
    }

    /// Rå text i `{…}` (för `\text`): nästlade klamrar behålls inte, blanksteg bevaras.
    fn raw_arg(&mut self) -> Result<String, String> {
        self.skip_ws();
        if self.peek() != Some('{') {
            return match self.peek() {
                Some(c) if c != '}' => {
                    self.i += 1;
                    Ok(c.to_string())
                }
                _ => Err("argument saknas".into()),
            };
        }
        self.i += 1;
        let mut depth = 1;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                }
                '\\' => match self.peek() {
                    Some(n @ ('{' | '}' | '$' | '%' | '&' | '#' | '_')) => {
                        out.push(n);
                        self.i += 1;
                    }
                    Some(' ') => {
                        out.push(' ');
                        self.i += 1;
                    }
                    _ => {}
                },
                _ => out.push(c),
            }
        }
        Err("saknar }".into())
    }

    fn atom_with_scripts(&mut self) -> Result<Node, String> {
        let mut base = self.atom()?;
        let (mut sub, mut sup, mut limits) = (None, None, Limits::Default);
        loop {
            self.skip_ws();
            match self.peek() {
                Some('^') => {
                    self.i += 1;
                    let a = self.arg()?;
                    sup.get_or_insert_with(Vec::new).extend(a);
                }
                Some('_') => {
                    self.i += 1;
                    let a = self.arg()?;
                    sub.get_or_insert_with(Vec::new).extend(a);
                }
                Some('\'') => {
                    self.i += 1;
                    sup.get_or_insert_with(Vec::new).push(Node::Sym('′', Class::Ord));
                }
                Some('\\') => match self.peek_command().as_deref() {
                    Some("limits") => {
                        self.eat_command("limits");
                        limits = Limits::Limits;
                    }
                    Some("nolimits") => {
                        self.eat_command("nolimits");
                        limits = Limits::NoLimits;
                    }
                    _ => break,
                },
                _ => break,
            }
        }
        if sub.is_none() && sup.is_none() {
            if limits != Limits::Default {
                base = Node::Scripts { base: Box::new(base), sub: None, sup: None, limits };
            }
            return Ok(base);
        }
        Ok(Node::Scripts { base: Box::new(base), sub, sup, limits })
    }

    fn atom(&mut self) -> Result<Node, String> {
        self.skip_ws();
        let Some(c) = self.peek() else { return Err("slut på formeln".into()) };
        match c {
            '{' => {
                self.i += 1;
                let l = self.list(false)?;
                self.expect('}')?;
                Ok(Node::Group(l))
            }
            '^' | '_' | '\'' => Ok(Node::Group(Vec::new())),
            '}' | ']' | '&' => Err("oväntat tecken".into()),
            '\\' => self.command(),
            '~' => {
                self.i += 1;
                Ok(Node::Space(Space::Normal))
            }
            '0'..='9' => {
                let mut n = String::new();
                while let Some(d) = self.peek() {
                    if d.is_ascii_digit() || (d == '.' && self.peek_at(1).is_some_and(|x| x.is_ascii_digit()) && !n.contains('.')) {
                        n.push(d);
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                Ok(Node::Num(n))
            }
            _ => {
                self.i += 1;
                Ok(char_node(c))
            }
        }
    }

    fn delim(&mut self) -> Result<Option<char>, String> {
        self.skip_ws();
        let Some(c) = self.peek() else { return Err("avgränsare saknas".into()) };
        if c == '\\' {
            let name = self.peek_command().ok_or("avgränsare saknas")?;
            self.eat_command(&name);
            return Ok(match name.as_str() {
                "{" | "lbrace" => Some('{'),
                "}" | "rbrace" => Some('}'),
                "|" | "Vert" | "lVert" | "rVert" => Some('‖'),
                "vert" | "lvert" | "rvert" => Some('|'),
                "backslash" => Some('\\'),
                "." => None,
                n => match symbols::symbol(n) {
                    Some((ch, _)) => Some(ch),
                    None => return Err(format!("okänd avgränsare \\{n}")),
                },
            });
        }
        self.i += 1;
        Ok(if c == '.' { None } else { Some(c) })
    }

    fn command(&mut self) -> Result<Node, String> {
        let name = self.peek_command().ok_or("oväntat \\")?;
        self.eat_command(&name);
        let n = name.as_str();
        // Funktionsnamn och symboler.
        if let Some(limits) = symbols::function_limits(n) {
            return Ok(Node::Func { name: n.to_owned(), limits });
        }
        match n {
            "frac" | "dfrac" | "tfrac" | "cfrac" | "binom" | "dbinom" | "tbinom" => {
                let num = self.arg()?;
                let den = self.arg()?;
                let kind = match n {
                    "dfrac" | "cfrac" => FracKind::DFrac,
                    "tfrac" => FracKind::TFrac,
                    "binom" | "dbinom" | "tbinom" => FracKind::Binom,
                    _ => FracKind::Frac,
                };
                Ok(Node::Frac { kind, num, den })
            }
            "sqrt" => {
                self.skip_ws();
                let index = if self.peek() == Some('[') {
                    self.i += 1;
                    let l = self.list(true)?;
                    self.expect(']')?;
                    Some(l)
                } else {
                    None
                };
                let body = self.arg()?;
                Ok(Node::Sqrt { index, body })
            }
            "text" | "textrm" | "textnormal" | "mbox" | "hbox" => Ok(Node::Text(self.raw_arg()?)),
            "textbf" => Ok(Node::Font { style: FontStyle::Bf, body: vec![Node::Text(self.raw_arg()?)] }),
            "textit" | "emph" => Ok(Node::Font { style: FontStyle::It, body: vec![Node::Text(self.raw_arg()?)] }),
            "mathrm" | "mathbf" | "mathit" | "mathcal" | "mathscr" | "mathfrak" | "mathbb" | "mathsf" | "mathtt" | "boldsymbol" | "bm"
            | "mathnormal" | "pmb" => {
                let style = match n {
                    "mathrm" => FontStyle::Rm,
                    "mathbf" => FontStyle::Bf,
                    "mathit" | "mathnormal" => FontStyle::It,
                    "mathcal" | "mathscr" => FontStyle::Cal,
                    "mathfrak" => FontStyle::Frak,
                    "mathbb" => FontStyle::Bb,
                    "mathsf" => FontStyle::Sf,
                    "mathtt" => FontStyle::Tt,
                    _ => FontStyle::BfIt,
                };
                Ok(Node::Font { style, body: self.arg()? })
            }
            "operatorname" => {
                let limits = self.peek() == Some('*');
                if limits {
                    self.i += 1;
                }
                Ok(Node::Func { name: self.raw_arg()?, limits })
            }
            "left" => {
                let open = self.delim()?;
                let body = self.list(false)?;
                if self.peek_command().as_deref() != Some("right") {
                    return Err("\\left utan \\right".into());
                }
                self.eat_command("right");
                let close = self.delim()?;
                Ok(Node::Delim { open, close, body })
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "Bigl" | "biggl" | "Biggl" | "bigr" | "Bigr" | "biggr" | "Biggr" | "bigm" | "Bigm"
            | "biggm" | "Biggm" => {
                let size = if n.starts_with("bigg") {
                    3
                } else if n.starts_with("Bigg") {
                    4
                } else if n.starts_with("Big") {
                    2
                } else {
                    1
                };
                let class = match n.chars().last() {
                    Some('l') => Class::Open,
                    Some('r') => Class::Close,
                    _ => Class::Ord,
                };
                let ch = self.delim()?.unwrap_or('.');
                Ok(Node::BigDelim { ch, size, class })
            }
            "hat" | "check" | "tilde" | "acute" | "grave" | "dot" | "ddot" | "breve" | "bar" | "vec" | "mathring" | "widehat" | "widetilde"
            | "overrightarrow" | "overleftarrow" => {
                let kind = match n {
                    "hat" => AccentKind::Hat,
                    "check" => AccentKind::Check,
                    "tilde" => AccentKind::Tilde,
                    "acute" => AccentKind::Acute,
                    "grave" => AccentKind::Grave,
                    "dot" => AccentKind::Dot,
                    "ddot" => AccentKind::DDot,
                    "breve" => AccentKind::Breve,
                    "bar" => AccentKind::Bar,
                    "vec" => AccentKind::Vec,
                    "mathring" => AccentKind::Ring,
                    "widehat" => AccentKind::WideHat,
                    "widetilde" => AccentKind::WideTilde,
                    "overrightarrow" => AccentKind::OverRightArrow,
                    _ => AccentKind::OverLeftArrow,
                };
                Ok(Node::Accent { kind, body: self.arg()? })
            }
            "overline" => Ok(Node::Line { over: true, body: self.arg()? }),
            "underline" => Ok(Node::Line { over: false, body: self.arg()? }),
            "overbrace" => Ok(Node::Brace { over: true, body: self.arg()? }),
            "underbrace" => Ok(Node::Brace { over: false, body: self.arg()? }),
            "overset" | "stackrel" => {
                let note = self.arg()?;
                let base = self.arg()?;
                Ok(Node::Stack { over: true, note, base })
            }
            "underset" => {
                let note = self.arg()?;
                let base = self.arg()?;
                Ok(Node::Stack { over: false, note, base })
            }
            "mathbin" | "mathrel" | "mathop" | "mathord" | "mathopen" | "mathclose" | "mathpunct" | "mathinner" => {
                let class = match n {
                    "mathbin" => Class::Bin,
                    "mathrel" => Class::Rel,
                    "mathop" => Class::Op,
                    "mathopen" => Class::Open,
                    "mathclose" => Class::Close,
                    "mathpunct" => Class::Punct,
                    "mathinner" => Class::Inner,
                    _ => Class::Ord,
                };
                Ok(Node::Classed { class, body: self.arg()? })
            }
            "bmod" => Ok(Node::Classed { class: Class::Bin, body: vec![Node::Func { name: "mod".into(), limits: false }] }),
            "mod" => Ok(Node::Group(vec![Node::Space(Space::Quad), Node::Func { name: "mod".into(), limits: false }, Node::Space(Space::Normal)])),
            "pmod" => {
                let arg = self.arg()?;
                let mut body = vec![Node::Space(Space::Quad), Node::Sym('(', Class::Open), Node::Func { name: "mod".into(), limits: false }, Node::Space(Space::Normal)];
                body.extend(arg);
                body.push(Node::Sym(')', Class::Close));
                Ok(Node::Group(body))
            }
            "quad" => Ok(Node::Space(Space::Quad)),
            "qquad" => Ok(Node::Space(Space::QQuad)),
            "," | "thinspace" => Ok(Node::Space(Space::Thin)),
            ":" | ">" | "medspace" => Ok(Node::Space(Space::Med)),
            ";" | "thickspace" => Ok(Node::Space(Space::Thick)),
            "!" | "negthinspace" => Ok(Node::Space(Space::NegThin)),
            " " | "enspace" => Ok(Node::Space(Space::Normal)),
            "not" => {
                let next = self.atom()?;
                let mapped = match &next {
                    Node::Sym('=', _) => Some('≠'),
                    Node::Sym('∈', _) => Some('∉'),
                    Node::Sym('⊂', _) => Some('⊄'),
                    Node::Sym('⊃', _) => Some('⊅'),
                    Node::Sym('⊆', _) => Some('⊈'),
                    Node::Sym('≡', _) => Some('≢'),
                    Node::Sym('∃', _) => Some('∄'),
                    Node::Sym('<', _) => Some('≮'),
                    Node::Sym('>', _) => Some('≯'),
                    Node::Sym('≈', _) => Some('≉'),
                    Node::Sym('∼', _) => Some('≁'),
                    Node::Sym('≤', _) => Some('≰'),
                    Node::Sym('≥', _) => Some('≱'),
                    _ => None,
                };
                match mapped {
                    Some(c) => Ok(Node::Sym(c, Class::Rel)),
                    None => Err("\\not med det tecknet stöds inte".into()),
                }
            }
            "substack" => {
                self.skip_ws();
                if self.peek() != Some('{') {
                    return Err("\\substack saknar {".into());
                }
                self.i += 1;
                let rows = self.rows(None)?;
                Ok(Node::Matrix { env: MatrixEnv::SubStack, rows, cols: String::new() })
            }
            "begin" => self.environment(),
            "hline" | "hfill" | "nonumber" | "notag" | "displaylimits" | "relax" | "protect" | "allowbreak" => Ok(Node::Group(Vec::new())),
            _ => {
                if let Some((ch, class)) = symbols::symbol(n) {
                    return Ok(Node::Sym(ch, class));
                }
                Err(format!("okänt kommando \\{n}"))
            }
        }
    }

    /// Rader av celler fram till `\end{namn}` (om `end` är `Some`) eller `}`.
    fn rows(&mut self, end: Option<&str>) -> Result<Vec<Vec<Vec<Node>>>, String> {
        let mut rows: Vec<Vec<Vec<Node>>> = Vec::new();
        let mut row: Vec<Vec<Node>> = Vec::new();
        loop {
            let cell = self.list(false)?;
            self.skip_ws();
            match self.peek() {
                Some('&') => {
                    self.i += 1;
                    row.push(cell);
                }
                Some('\\') => match self.peek_command().as_deref() {
                    Some("\\") => {
                        self.i += 2;
                        // `\\[2pt]`: avståndet ignoreras.
                        self.skip_ws();
                        if self.peek() == Some('[') {
                            while self.peek().is_some_and(|c| c != ']') {
                                self.i += 1;
                            }
                            self.i += 1;
                        }
                        row.push(cell);
                        rows.push(std::mem::take(&mut row));
                    }
                    Some("end") if end.is_some() => {
                        self.eat_command("end");
                        let name = self.raw_arg()?;
                        if Some(name.as_str()) != end {
                            return Err("\\end matchar inte \\begin".into());
                        }
                        let blank = row.is_empty() && cell.is_empty();
                        if !blank {
                            row.push(cell);
                            rows.push(row);
                        }
                        return Ok(rows);
                    }
                    _ => return Err(self.stop_error()),
                },
                Some('}') if end.is_none() => {
                    self.i += 1;
                    let blank = row.is_empty() && cell.is_empty();
                    if !blank {
                        row.push(cell);
                        rows.push(row);
                    }
                    return Ok(rows);
                }
                _ => return Err("matrisen är inte avslutad".into()),
            }
        }
    }

    fn environment(&mut self) -> Result<Node, String> {
        let raw = self.raw_arg()?;
        let name = raw.trim_end_matches('*').to_owned();
        let env = match name.as_str() {
            "matrix" => MatrixEnv::Matrix,
            "pmatrix" => MatrixEnv::PMatrix,
            "bmatrix" => MatrixEnv::BMatrix,
            "Bmatrix" => MatrixEnv::BbMatrix,
            "vmatrix" => MatrixEnv::VMatrix,
            "Vmatrix" => MatrixEnv::VvMatrix,
            "smallmatrix" => MatrixEnv::SmallMatrix,
            "cases" => MatrixEnv::Cases,
            "aligned" | "align" | "split" | "alignedat" | "flalign" => MatrixEnv::Aligned,
            "gathered" | "gather" | "multline" => MatrixEnv::Gathered,
            "array" => MatrixEnv::Array,
            _ => return Err(format!("okänd miljö {name}")),
        };
        let mut cols = String::new();
        if env == MatrixEnv::Array {
            cols = self.raw_arg()?.chars().filter(|c| matches!(c, 'l' | 'c' | 'r')).collect();
        }
        if name == "alignedat" {
            self.raw_arg()?;
        }
        let rows = self.rows(Some(&raw))?;
        Ok(Node::Matrix { env, rows, cols })
    }
}

fn style_switch(name: &str) -> Option<MStyle> {
    match name {
        "displaystyle" => Some(MStyle::Display),
        "textstyle" => Some(MStyle::Text),
        "scriptstyle" => Some(MStyle::Script),
        "scriptscriptstyle" => Some(MStyle::ScriptScript),
        _ => None,
    }
}

/// Noden för ett lösryckt tecken.
pub(super) fn char_node(c: char) -> Node {
    match c {
        '-' => Node::Sym('\u{2212}', Class::Bin),
        '*' => Node::Sym('\u{2217}', Class::Bin),
        '`' => Node::Sym('\u{2018}', Class::Ord),
        c if symbols::SYMBOLS.iter().any(|(_, x, _)| *x == c) && !c.is_ascii() => Node::Sym(c, symbols::class_of_char(c)),
        c if c.is_alphabetic() => Node::Var(c),
        c => Node::Sym(c, symbols::class_of_char(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<Node> {
        parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn letters_digits_and_operators() {
        assert_eq!(
            p("x+12.5"),
            vec![Node::Var('x'), Node::Sym('+', Class::Bin), Node::Num("12.5".into())]
        );
        assert_eq!(p("a-b")[1], Node::Sym('−', Class::Bin));
        assert_eq!(p("a=b")[1], Node::Sym('=', Class::Rel));
    }

    #[test]
    fn scripts_combine_in_any_order() {
        let a = p("x^2_i");
        let b = p("x_i^2");
        assert_eq!(a, b);
        match &a[0] {
            Node::Scripts { base, sub, sup, .. } => {
                assert_eq!(**base, Node::Var('x'));
                assert_eq!(sub.as_deref(), Some(&[Node::Var('i')][..]));
                assert_eq!(sup.as_deref(), Some(&[Node::Num("2".into())][..]));
            }
            n => panic!("{n:?}"),
        }
        // Enstaka tecken utan klammrar tar bara en token.
        let n = p("e^x y");
        assert_eq!(n.len(), 2);
    }

    #[test]
    fn primes_become_superscripts() {
        match &p("f'")[0] {
            Node::Scripts { sup, .. } => assert_eq!(sup.as_deref(), Some(&[Node::Sym('′', Class::Ord)][..])),
            n => panic!("{n:?}"),
        }
    }

    #[test]
    fn fractions_roots_and_text() {
        assert!(matches!(&p(r"\frac{a}{b+1}")[0], Node::Frac { kind: FracKind::Frac, num, den } if num.len() == 1 && den.len() == 3));
        assert!(matches!(&p(r"\sqrt[3]{x}")[0], Node::Sqrt { index: Some(i), .. } if i.len() == 1));
        assert!(matches!(&p(r"\sqrt x")[0], Node::Sqrt { index: None, body } if body == &[Node::Var('x')]));
        assert_eq!(p(r"\text{om } x")[0], Node::Text("om ".into()));
        assert_eq!(p(r"\binom{n}{k}").len(), 1);
    }

    #[test]
    fn big_operators_and_limits() {
        match &p(r"\sum_{i=1}^{n} i")[0] {
            Node::Scripts { base, sub: Some(s), sup: Some(u), .. } => {
                assert_eq!(**base, Node::Sym('∑', Class::Op));
                assert_eq!(s.len(), 3);
                assert_eq!(u.len(), 1);
            }
            n => panic!("{n:?}"),
        }
        assert!(matches!(&p(r"\int\limits_0^1")[0], Node::Scripts { limits: Limits::Limits, .. }));
        assert!(matches!(&p(r"\lim_{x\to 0}")[0], Node::Scripts { base, .. } if matches!(**base, Node::Func { limits: true, .. })));
    }

    #[test]
    fn left_right_and_matrices() {
        assert!(matches!(&p(r"\left( x \right)")[0], Node::Delim { open: Some('('), close: Some(')'), .. }));
        assert!(matches!(&p(r"\left. x \right|")[0], Node::Delim { open: None, close: Some('|'), .. }));
        match &p(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}")[0] {
            Node::Matrix { env: MatrixEnv::PMatrix, rows, .. } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
            }
            n => panic!("{n:?}"),
        }
        match &p(r"\begin{cases} 1 & x>0 \\ 0 & \text{annars} \end{cases}")[0] {
            Node::Matrix { env: MatrixEnv::Cases, rows, .. } => assert_eq!(rows.len(), 2),
            n => panic!("{n:?}"),
        }
        assert!(matches!(&p(r"\begin{aligned} a &= b \\ &= c \end{aligned}")[0], Node::Matrix { env: MatrixEnv::Aligned, rows, .. } if rows.len() == 2));
    }

    #[test]
    fn fonts_accents_and_spacing() {
        assert!(matches!(&p(r"\mathbf{v}")[0], Node::Font { style: FontStyle::Bf, .. }));
        assert!(matches!(&p(r"\mathbb{R}")[0], Node::Font { style: FontStyle::Bb, .. }));
        assert!(matches!(&p(r"\hat{x}")[0], Node::Accent { kind: AccentKind::Hat, .. }));
        assert!(matches!(&p(r"\vec v")[0], Node::Accent { kind: AccentKind::Vec, .. }));
        assert_eq!(p(r"a\,b")[1], Node::Space(Space::Thin));
        assert_eq!(p(r"a\quad b")[1], Node::Space(Space::Quad));
        assert!(matches!(&p(r"\displaystyle \frac12")[0], Node::Styled { style: MStyle::Display, .. }));
    }

    #[test]
    fn greek_and_symbols() {
        assert_eq!(p(r"\alpha")[0], Node::Sym('α', Class::Ord));
        assert_eq!(p(r"\leq")[0], Node::Sym('≤', Class::Rel));
        assert_eq!(p(r"\infty")[0], Node::Sym('∞', Class::Ord));
        assert_eq!(p("α")[0], Node::Sym('α', Class::Ord));
        assert!(matches!(&p(r"\sin x")[0], Node::Func { name, .. } if name == "sin"));
    }

    #[test]
    fn errors_are_reported_not_panics() {
        assert!(parse(r"\frac{a}").is_err());
        assert!(parse(r"\unknowncmd").is_err());
        assert!(parse("{a").is_err());
        assert!(parse("a}").is_err());
        assert!(parse(r"\left( x").is_err());
        assert!(parse(r"\begin{pmatrix} a").is_err());
        assert!(parse("a & b").is_err());
        assert!(parse(&"{".repeat(500)).is_err());
        assert!(parse("").unwrap().is_empty());
    }
}
