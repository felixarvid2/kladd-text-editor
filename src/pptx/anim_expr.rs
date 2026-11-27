//! Formlerna i animeringarnas nyckelbilder (`p:tav val="0-#ppt_w/2"`, `p:anim by="…"`): tal, variablerna
//! `#ppt_x`, `#ppt_y` (formens mitt), `#ppt_w`, `#ppt_h` (storlek, allt i andelar av bilden), `$` (tiden
//! 0..1), `+ - * / ^`, parenteser och funktionerna `sin cos tan abs sqrt min max` (ECMA-376 del 1
//! §19.5.8). En formel som inte kan tolkas ger `None`; beteendet hoppas då över.

/// Variablernas värden.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vars {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub t: f32,
}

/// Räknar ut formeln.
pub fn eval(s: &str, v: &Vars) -> Option<f32> {
    let toks = lex(s)?;
    let mut p = Parser { toks: &toks, i: 0, v };
    let r = p.expr()?;
    (p.i == toks.len() && r.is_finite()).then_some(r)
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f32),
    Var(char),
    Name(String),
    Op(char),
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_digit() || ch == '.' {
            let st = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            // Exponent (1E-05).
            if i < c.len() && (c[i] == 'e' || c[i] == 'E') && i + 1 < c.len() && (c[i + 1].is_ascii_digit() || ((c[i + 1] == '-' || c[i + 1] == '+') && i + 2 < c.len() && c[i + 2].is_ascii_digit())) {
                i += 2;
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            out.push(Tok::Num(c[st..i].iter().collect::<String>().parse().ok()?));
        } else if ch == '#' {
            let st = i + 1;
            i += 1;
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            let name: String = c[st..i].iter().collect();
            out.push(Tok::Var(match name.as_str() {
                "ppt_x" => 'x',
                "ppt_y" => 'y',
                "ppt_w" => 'w',
                "ppt_h" => 'h',
                _ => return None,
            }));
        } else if ch == '$' {
            out.push(Tok::Var('t'));
            i += 1;
        } else if ch.is_ascii_alphabetic() {
            let st = i;
            while i < c.len() && c[i].is_ascii_alphanumeric() {
                i += 1;
            }
            out.push(Tok::Name(c[st..i].iter().collect::<String>().to_ascii_lowercase()));
        } else if "+-*/^(),".contains(ch) {
            out.push(Tok::Op(ch));
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

struct Parser<'a> {
    toks: &'a [Tok],
    i: usize,
    v: &'a Vars,
}

impl Parser<'_> {
    fn peek_op(&self, op: char) -> bool {
        self.toks.get(self.i) == Some(&Tok::Op(op))
    }

    fn expr(&mut self) -> Option<f32> {
        let mut a = self.term()?;
        loop {
            if self.peek_op('+') {
                self.i += 1;
                a += self.term()?;
            } else if self.peek_op('-') {
                self.i += 1;
                a -= self.term()?;
            } else {
                return Some(a);
            }
        }
    }

    fn term(&mut self) -> Option<f32> {
        let mut a = self.power()?;
        loop {
            if self.peek_op('*') {
                self.i += 1;
                a *= self.power()?;
            } else if self.peek_op('/') {
                self.i += 1;
                let b = self.power()?;
                a = if b == 0.0 { 0.0 } else { a / b };
            } else {
                return Some(a);
            }
        }
    }

    fn power(&mut self) -> Option<f32> {
        let a = self.unary()?;
        if self.peek_op('^') {
            self.i += 1;
            let b = self.power()?;
            return Some(a.powf(b));
        }
        Some(a)
    }

    fn unary(&mut self) -> Option<f32> {
        if self.peek_op('-') {
            self.i += 1;
            return Some(-self.unary()?);
        }
        if self.peek_op('+') {
            self.i += 1;
            return self.unary();
        }
        self.atom()
    }

    fn atom(&mut self) -> Option<f32> {
        let t = self.toks.get(self.i)?.clone();
        self.i += 1;
        match t {
            Tok::Num(n) => Some(n),
            Tok::Var(c) => Some(match c {
                'x' => self.v.x,
                'y' => self.v.y,
                'w' => self.v.w,
                'h' => self.v.h,
                _ => self.v.t,
            }),
            Tok::Op('(') => {
                let a = self.expr()?;
                self.peek_op(')').then(|| self.i += 1)?;
                Some(a)
            }
            Tok::Name(n) => {
                match n.as_str() {
                    "pi" => return Some(std::f32::consts::PI),
                    "e" => return Some(std::f32::consts::E),
                    _ => {}
                }
                self.peek_op('(').then(|| self.i += 1)?;
                let mut args = vec![self.expr()?];
                while self.peek_op(',') {
                    self.i += 1;
                    args.push(self.expr()?);
                }
                self.peek_op(')').then(|| self.i += 1)?;
                let a = args[0];
                let b = args.get(1).copied();
                // Vinklar i grader (som PowerPoints formler).
                Some(match n.as_str() {
                    "sin" => a.to_radians().sin(),
                    "cos" => a.to_radians().cos(),
                    "tan" => a.to_radians().tan(),
                    "abs" => a.abs(),
                    "sqrt" => a.max(0.0).sqrt(),
                    "min" => a.min(b?),
                    "max" => a.max(b?),
                    "rand" => 0.5 * a,
                    _ => return None,
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formler_ur_powerpoint() {
        let v = Vars { x: 0.4, y: 0.6, w: 0.2, h: 0.1, t: 0.5 };
        let e = |s: &str| eval(s, &v).unwrap();
        assert!((e("#ppt_x") - 0.4).abs() < 1e-6);
        assert!((e("0-#ppt_w/2") + 0.1).abs() < 1e-6);
        assert!((e("1+#ppt_h/2") - 1.05).abs() < 1e-6);
        assert!((e("#ppt_y+.1") - 0.7).abs() < 1e-6);
        assert!((e("-6.94444E-6") + 6.94444e-6).abs() < 1e-9);
        assert!((e("(1 + 2) * 3 ^ 2") - 27.0).abs() < 1e-5);
        assert!((e("#ppt_x+sin(90)*#ppt_w") - 0.6).abs() < 1e-6);
        assert!((e("max(1,2)") - 2.0).abs() < 1e-6);
        assert!((e("$*2") - 1.0).abs() < 1e-6);
        assert!(eval("#okänd", &v).is_none());
        assert!(eval("1 +", &v).is_none());
        assert!(eval("(1", &v).is_none());
        assert!(eval("foo(1)", &v).is_none());
        assert_eq!(eval("1/0", &v), Some(0.0));
    }
}
