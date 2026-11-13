//! Syntaxträd som LaTeX-text (för OMML → LaTeX och normalisering).

use super::ast::*;
use super::symbols;

pub fn to_latex(nodes: &[Node]) -> String {
    let mut out = String::new();
    list(nodes, &mut out);
    out.trim().to_owned()
}

/// Lägger till `s`; ett blanksteg sätts in om föregående kommando annars skulle växa ihop med den.
fn push(out: &mut String, s: &str) {
    if s.is_empty() {
        return;
    }
    if s.starts_with(|c: char| c.is_ascii_alphabetic()) && ends_with_command(out) {
        out.push(' ');
    }
    out.push_str(s);
}

fn ends_with_command(out: &str) -> bool {
    let b = out.as_bytes();
    let mut i = b.len();
    while i > 0 && b[i - 1].is_ascii_alphabetic() {
        i -= 1;
    }
    i < b.len() && i > 0 && b[i - 1] == b'\\' && !(i > 1 && b[i - 2] == b'\\')
}

fn list(nodes: &[Node], out: &mut String) {
    for n in nodes {
        node(n, out);
    }
}

fn braced(nodes: &[Node], out: &mut String) {
    out.push('{');
    list(nodes, out);
    out.push('}');
}

/// Ett skriptargument: utan klammrar om det är ett enda tecken.
fn script_arg(nodes: &[Node], out: &mut String) {
    let simple = match nodes {
        [Node::Var(c)] => c.is_ascii_alphabetic(),
        [Node::Num(n)] => n.len() == 1,
        [Node::Sym(c, _)] => c.is_ascii() && !matches!(c, '{' | '}' | '\\' | '_' | '^' | '%' | '$' | '#' | '&'),
        _ => false,
    };
    if simple {
        let mut s = String::new();
        list(nodes, &mut s);
        out.push_str(&s);
    } else {
        braced(nodes, out);
    }
}

fn delim_text(c: Option<char>) -> String {
    match c {
        None => ".".into(),
        Some('{') => "\\{".into(),
        Some('}') => "\\}".into(),
        Some('‖') => "\\|".into(),
        Some('|') => "|".into(),
        Some('\\') => "\\backslash".into(),
        Some(c) => match symbols::command_for(c) {
            Some(n) => format!("\\{n}"),
            None => c.to_string(),
        },
    }
}

fn escape_text(t: &str) -> String {
    let mut o = String::new();
    for c in t.chars() {
        match c {
            '{' | '}' | '$' | '%' | '&' | '#' | '_' => {
                o.push('\\');
                o.push(c);
            }
            '\\' => o.push_str("\\textbackslash "),
            c => o.push(c),
        }
    }
    o
}

fn node(n: &Node, out: &mut String) {
    match n {
        Node::Num(s) => push(out, s),
        Node::Var(c) => push(out, &c.to_string()),
        Node::Sym(c, _) => match c {
            '\u{2212}' => out.push('-'),
            '\u{2217}' => push(out, "\\ast"),
            '{' | '}' | '%' | '$' | '#' | '&' | '_' => {
                out.push('\\');
                out.push(*c);
            }
            '\\' => push(out, "\\backslash"),
            c if c.is_ascii() => out.push(*c),
            c => match symbols::command_for(*c) {
                Some(name) => push(out, &format!("\\{name}")),
                None => out.push(*c),
            },
        },
        Node::Text(t) => {
            push(out, "\\text");
            out.push('{');
            out.push_str(&escape_text(t));
            out.push('}');
        }
        Node::Func { name, limits } => {
            if symbols::function_limits(name).is_some() {
                push(out, &format!("\\{name}"));
            } else {
                push(out, if *limits { "\\operatorname*" } else { "\\operatorname" });
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        Node::Group(l) => braced(l, out),
        Node::Scripts { base, sub, sup, limits } => {
            match &**base {
                Node::Group(g) if g.is_empty() => out.push_str("{}"),
                b => node(b, out),
            }
            match limits {
                Limits::Limits => push(out, "\\limits"),
                Limits::NoLimits => push(out, "\\nolimits"),
                Limits::Default => {}
            }
            if let Some(s) = sub {
                out.push('_');
                script_arg(s, out);
            }
            if let Some(s) = sup {
                if matches!(s.as_slice(), [Node::Sym('′', _)]) {
                    out.push('\'');
                } else {
                    out.push('^');
                    script_arg(s, out);
                }
            }
        }
        Node::Frac { kind, num, den } => {
            push(
                out,
                match kind {
                    FracKind::Frac => "\\frac",
                    FracKind::DFrac => "\\dfrac",
                    FracKind::TFrac => "\\tfrac",
                    FracKind::Binom => "\\binom",
                },
            );
            braced(num, out);
            braced(den, out);
        }
        Node::Sqrt { index, body } => {
            push(out, "\\sqrt");
            if let Some(i) = index {
                out.push('[');
                list(i, out);
                out.push(']');
            }
            braced(body, out);
        }
        Node::Delim { open, close, body } => {
            push(out, "\\left");
            push(out, &delim_text(*open));
            out.push(' ');
            list(body, out);
            out.push(' ');
            push(out, "\\right");
            push(out, &delim_text(*close));
        }
        Node::Middle(c) => {
            push(out, "\\middle");
            push(out, &delim_text(Some(*c)));
        }
        Node::BigDelim { ch, size, class } => {
            let base = ["big", "Big", "bigg", "Bigg"][usize::from((*size).clamp(1, 4)) - 1];
            let suffix = match class {
                Class::Open => "l",
                Class::Close => "r",
                _ => "",
            };
            push(out, &format!("\\{base}{suffix}"));
            push(out, &delim_text(if *ch == '.' { None } else { Some(*ch) }));
        }
        Node::Matrix { env, rows, cols } => {
            let sub = *env == MatrixEnv::SubStack;
            if sub {
                push(out, "\\substack{");
            } else {
                push(out, &format!("\\begin{{{}}}", env.name()));
                if *env == MatrixEnv::Array {
                    out.push('{');
                    out.push_str(cols);
                    out.push('}');
                }
            }
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str(" \\\\ ");
                } else {
                    out.push(' ');
                }
                for (j, c) in r.iter().enumerate() {
                    if j > 0 {
                        out.push_str(" & ");
                    }
                    list(c, out);
                }
            }
            if sub {
                out.push('}');
            } else {
                push(out, &format!(" \\end{{{}}}", env.name()));
            }
        }
        Node::Accent { kind, body } => {
            push(out, &format!("\\{}", kind.command()));
            braced(body, out);
        }
        Node::Line { over, body } => {
            push(out, if *over { "\\overline" } else { "\\underline" });
            braced(body, out);
        }
        Node::Brace { over, body } => {
            push(out, if *over { "\\overbrace" } else { "\\underbrace" });
            braced(body, out);
        }
        Node::Stack { over, note, base } => {
            push(out, if *over { "\\overset" } else { "\\underset" });
            braced(note, out);
            braced(base, out);
        }
        Node::Font { style, body } => {
            push(out, &format!("\\{}", style.command()));
            braced(body, out);
        }
        Node::Space(s) => out.push_str(match s {
            Space::Thin => "\\,",
            Space::Med => "\\:",
            Space::Thick => "\\;",
            Space::NegThin => "\\!",
            Space::Quad => "\\quad",
            Space::QQuad => "\\qquad",
            Space::Normal => "\\ ",
        }),
        Node::Styled { style, body } => {
            push(
                out,
                match style {
                    MStyle::Display => "\\displaystyle",
                    MStyle::Text => "\\textstyle",
                    MStyle::Script => "\\scriptstyle",
                    MStyle::ScriptScript => "\\scriptscriptstyle",
                },
            );
            out.push(' ');
            list(body, out);
        }
        Node::Classed { class, body } => {
            push(
                out,
                match class {
                    Class::Bin => "\\mathbin",
                    Class::Rel => "\\mathrel",
                    Class::Op => "\\mathop",
                    Class::Open => "\\mathopen",
                    Class::Close => "\\mathclose",
                    Class::Punct => "\\mathpunct",
                    Class::Inner => "\\mathinner",
                    Class::Ord => "\\mathord",
                },
            );
            braced(body, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::parse::parse;

    fn rt(src: &str) {
        let a = parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        let l = to_latex(&a);
        let b = parse(&l).unwrap_or_else(|e| panic!("{src} -> {l}: {e}"));
        assert_eq!(a, b, "{src} -> {l}");
    }

    #[test]
    fn latex_round_trips_through_the_parser() {
        for s in [
            "x+y=z",
            "x^2_i",
            "e^{i\\pi}+1=0",
            r"\frac{a+b}{c}",
            r"\dfrac12 + \tfrac{3}{4}",
            r"\sqrt[3]{x^2+1}",
            r"\sum_{i=1}^{n} i^2",
            r"\int_0^\infty f(x)\,dx",
            r"\lim_{x\to 0} \frac{\sin x}{x}",
            r"\left( \frac{a}{b} \right)",
            r"\left\{ x \middle| x>0 \right.",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"\begin{cases} 1 & x>0 \\ 0 & \text{annars} \end{cases}",
            r"\begin{aligned} a &= b \\ &= c \end{aligned}",
            r"\sum_{\substack{i<n \\ j<m}} a_{ij}",
            r"\mathbf{v}\cdot\mathbb{R}^n",
            r"\hat{x}+\widehat{xy}+\vec v",
            r"\overline{x}+\underline{y}+\overbrace{abc}^{n}",
            r"\overset{def}{=} \underset{n}{\max}",
            r"\text{om } x \quad y \; z",
            r"\binom{n}{k} \operatorname{rank} A",
            r"\Big( x \Big)",
            r"f'(x) \neq \infty",
            r"\alpha\beta\Gamma",
            r"\sin\theta \cos x",
            r"a\mathrm{d}b",
        ] {
            rt(s);
        }
    }

    #[test]
    fn output_is_compact_latex() {
        assert_eq!(to_latex(&parse(r"\frac{a}{b}").unwrap()), r"\frac{a}{b}");
        assert_eq!(to_latex(&parse("x^2+y_1").unwrap()), "x^2+y_1");
        assert_eq!(to_latex(&parse(r"\alpha x").unwrap()), r"\alpha x");
        assert_eq!(to_latex(&parse(r"\sin x").unwrap()), r"\sin x");
        assert_eq!(to_latex(&parse("a-b").unwrap()), "a-b");
    }
}
