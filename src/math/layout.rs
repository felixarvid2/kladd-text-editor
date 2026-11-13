//! Utläggning av formler: träd -> glyfer och linjer på em-rutnät (y uppåt från baslinjen).
//!
//! Följer TeX:s regler (bilaga G) med måtten ur typsnittets MATH-tabell. Alla mått är i em av
//! brödtextstorleken; skript ritas redan förminskade (glyfens `s` är skalan).

use super::ast::*;
use super::font::{self, MathFont, k};

/// En ritbar del: en glyf (origo på baslinjen, skala `s` em) eller en fylld rektangel (`y` = underkant).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Glyph { gid: u16, x: f32, y: f32, s: f32 },
    Rule { x: f32, y: f32, w: f32, h: f32 },
}

/// Färdig formel: bredd, höjd över och djup under baslinjen (em) och delarna.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub w: f32,
    pub asc: f32,
    pub desc: f32,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default)]
struct Bx {
    w: f32,
    asc: f32,
    desc: f32,
    items: Vec<Item>,
    /// Kursivkorrektion (em) för skript.
    italic: f32,
    /// Fästpunkt för accent (x), om känd.
    attach: Option<f32>,
    /// Ett enda tecken (skript tar då inte hänsyn till bashöjd).
    single: bool,
}

impl Bx {
    fn put(&mut self, o: &Bx, dx: f32, dy: f32) {
        for it in &o.items {
            self.items.push(shift(*it, dx, dy));
        }
    }

    fn height(&self) -> f32 {
        self.asc + self.desc
    }
}

fn shift(it: Item, dx: f32, dy: f32) -> Item {
    match it {
        Item::Glyph { gid, x, y, s } => Item::Glyph { gid, x: x + dx, y: y + dy, s },
        Item::Rule { x, y, w, h } => Item::Rule { x: x + dx, y: y + dy, w, h },
    }
}

#[derive(Clone, Copy)]
struct Ctx {
    style: MStyle,
    cramped: bool,
    font: FontStyle,
    /// Höjd (em) som `\middle` ska sträckas till.
    mid: Option<f32>,
}

impl Ctx {
    fn scale(self, f: &MathFont) -> f32 {
        match self.style {
            MStyle::Display | MStyle::Text => 1.0,
            MStyle::Script => f.script_pct,
            MStyle::ScriptScript => f.script_script_pct,
        }
    }

    fn display(self) -> bool {
        self.style == MStyle::Display
    }

    fn script(self) -> bool {
        matches!(self.style, MStyle::Script | MStyle::ScriptScript)
    }

    fn sup(self) -> Ctx {
        Ctx {
            style: match self.style {
                MStyle::Display | MStyle::Text => MStyle::Script,
                _ => MStyle::ScriptScript,
            },
            ..self
        }
    }

    fn sub(self) -> Ctx {
        Ctx { cramped: true, ..self.sup() }
    }

    fn num(self) -> Ctx {
        Ctx {
            style: match self.style {
                MStyle::Display => MStyle::Text,
                MStyle::Text => MStyle::Script,
                _ => MStyle::ScriptScript,
            },
            ..self
        }
    }

    fn den(self) -> Ctx {
        Ctx { cramped: true, ..self.num() }
    }

    fn crampedd(self) -> Ctx {
        Ctx { cramped: true, ..self }
    }
}

/// Lägger ut en formel. `display`: visningsstil (annars löpande text). `None` om typsnittet saknas.
pub fn layout(nodes: &[Node], display: bool) -> Option<Layout> {
    let f = font::font()?;
    let l = L { f };
    let ctx = Ctx { style: if display { MStyle::Display } else { MStyle::Text }, cramped: false, font: FontStyle::Auto, mid: None };
    let b = l.list(nodes, ctx, None);
    Some(Layout { w: b.w, asc: b.asc, desc: b.desc, items: b.items })
}

struct L {
    f: &'static MathFont,
}

/// Avstånd i mu (1/18 em) mellan två klasser. Negativt = bara utanför skriptstilar.
fn space_mu(l: Class, r: Class) -> i8 {
    use Class::*;
    let col = |c: Class| match c {
        Ord => 0,
        Op => 1,
        Bin => 2,
        Rel => 3,
        Open => 4,
        Close => 5,
        Punct => 6,
        Inner => 7,
    };
    const T: [[i8; 8]; 8] = [
        [0, 1, -4, -5, 0, 0, 0, -3],
        [1, 1, 0, -5, 0, 0, 0, -3],
        [-4, -4, 0, 0, -4, 0, 0, -4],
        [-5, -5, 0, 0, -5, 0, 0, -5],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 1, -4, -5, 0, 0, 0, -3],
        [-3, -3, 0, -3, -3, -3, -3, -3],
        [-3, 1, -4, -5, -3, 0, -3, -3],
    ];
    T[col(l)][col(r)]
}

fn is_alpha(c: char) -> bool {
    c.is_ascii_alphabetic()
}

/// Tecknet i Unicodes matematiska alfanumeriska block för stilen (eller tecknet självt).
pub fn styled(c: char, style: FontStyle, var: bool) -> char {
    let style = if style == FontStyle::Auto && var { FontStyle::It } else { style };
    let off = |cap: u32, small: u32| -> Option<u32> {
        if c.is_ascii_uppercase() {
            Some(cap + (c as u32 - 'A' as u32))
        } else if c.is_ascii_lowercase() {
            Some(small + (c as u32 - 'a' as u32))
        } else {
            None
        }
    };
    let digit = |base: u32| -> Option<u32> { c.is_ascii_digit().then(|| base + (c as u32 - '0' as u32)) };
    let greek_lower = ('α'..='ω').contains(&c) || matches!(c, 'ϑ' | 'ϰ' | 'ϕ' | 'ϱ' | 'ϖ' | 'ϵ');
    let greek_upper = ('Α'..='Ω').contains(&c);
    let mapped: Option<u32> = match style {
        FontStyle::Auto | FontStyle::Rm => None,
        FontStyle::It => {
            if c == 'h' {
                Some(0x210E)
            } else if greek_lower {
                Some(match c {
                    'ϵ' => 0x1D716,
                    'ϑ' => 0x1D717,
                    'ϰ' => 0x1D718,
                    'ϕ' => 0x1D719,
                    'ϱ' => 0x1D71A,
                    'ϖ' => 0x1D71B,
                    c => 0x1D6FC + (c as u32 - 'α' as u32),
                })
            } else {
                off(0x1D434, 0x1D44E)
            }
        }
        FontStyle::Bf => {
            if greek_upper {
                Some(0x1D6A8 + (c as u32 - 'Α' as u32))
            } else if greek_lower && c != 'ϵ' && !matches!(c, 'ϑ' | 'ϰ' | 'ϕ' | 'ϱ' | 'ϖ') {
                Some(0x1D6C2 + (c as u32 - 'α' as u32))
            } else {
                off(0x1D400, 0x1D41A).or_else(|| digit(0x1D7CE))
            }
        }
        FontStyle::BfIt => {
            if greek_upper {
                Some(0x1D71C + (c as u32 - 'Α' as u32))
            } else if greek_lower && c != 'ϵ' && !matches!(c, 'ϑ' | 'ϰ' | 'ϕ' | 'ϱ' | 'ϖ') {
                Some(0x1D736 + (c as u32 - 'α' as u32))
            } else {
                off(0x1D468, 0x1D482)
            }
        }
        FontStyle::Cal => off(0x1D49C, 0x1D4B6),
        FontStyle::Frak => off(0x1D504, 0x1D51E),
        FontStyle::Bb => off(0x1D538, 0x1D552).or_else(|| digit(0x1D7D8)),
        FontStyle::Sf => off(0x1D5A0, 0x1D5BA).or_else(|| digit(0x1D7E2)),
        FontStyle::Tt => off(0x1D670, 0x1D68A).or_else(|| digit(0x1D7F6)),
    };
    let Some(m) = mapped else { return c };
    // Hål i blocket: tecknen finns på andra platser.
    let hole = |c: char, style: FontStyle| -> Option<char> {
        Some(match (style, c) {
            (FontStyle::Cal, 'B') => 'ℬ',
            (FontStyle::Cal, 'E') => 'ℰ',
            (FontStyle::Cal, 'F') => 'ℱ',
            (FontStyle::Cal, 'H') => 'ℋ',
            (FontStyle::Cal, 'I') => 'ℐ',
            (FontStyle::Cal, 'L') => 'ℒ',
            (FontStyle::Cal, 'M') => 'ℳ',
            (FontStyle::Cal, 'R') => 'ℛ',
            (FontStyle::Cal, 'e') => 'ℯ',
            (FontStyle::Cal, 'g') => 'ℊ',
            (FontStyle::Cal, 'o') => 'ℴ',
            (FontStyle::Frak, 'C') => 'ℭ',
            (FontStyle::Frak, 'H') => 'ℌ',
            (FontStyle::Frak, 'I') => 'ℑ',
            (FontStyle::Frak, 'R') => 'ℜ',
            (FontStyle::Frak, 'Z') => 'ℨ',
            (FontStyle::Bb, 'C') => 'ℂ',
            (FontStyle::Bb, 'H') => 'ℍ',
            (FontStyle::Bb, 'N') => 'ℕ',
            (FontStyle::Bb, 'P') => 'ℙ',
            (FontStyle::Bb, 'Q') => 'ℚ',
            (FontStyle::Bb, 'R') => 'ℝ',
            (FontStyle::Bb, 'Z') => 'ℤ',
            _ => return None,
        })
    };
    hole(c, style).or_else(|| char::from_u32(m)).unwrap_or(c)
}

impl L {
    // ----------------------------------------------------------- byggstenar

    fn glyph_box(&self, gid: u16, s: f32) -> Bx {
        let (asc, desc) = self.f.bbox(gid).map_or((0.0, 0.0), |(_, y0, _, y1)| (y1.max(0.0) * s, (-y0).max(0.0) * s));
        Bx {
            w: self.f.advance(gid) * s,
            asc,
            desc,
            items: vec![Item::Glyph { gid, x: 0.0, y: 0.0, s }],
            italic: self.f.italic_correction(gid) * s,
            attach: self.f.top_accent(gid).map(|a| a * s),
            single: true,
        }
    }

    /// Tecken för tecken i en rad (siffror, text, funktionsnamn).
    fn run(&self, chars: impl Iterator<Item = char>, ctx: Ctx, font: FontStyle, var: bool) -> Bx {
        let s = ctx.scale(self.f);
        let mut b = Bx::default();
        let mut n = 0;
        for c in chars {
            let m = styled(c, font, var);
            let m = if self.f.has(m) { m } else { c };
            let g = if c == ' ' {
                let mut sp = Bx { w: 0.25 * s, ..Bx::default() };
                sp.single = false;
                sp
            } else {
                self.glyph_box(self.f.glyph(m), s)
            };
            let x = b.w;
            b.put(&g, x, 0.0);
            b.w += g.w;
            b.asc = b.asc.max(g.asc);
            b.desc = b.desc.max(g.desc);
            b.italic = g.italic;
            b.attach = g.attach.map(|a| a + x);
            n += 1;
        }
        b.single = n == 1;
        if n != 1 {
            b.attach = None;
        }
        b
    }

    fn space(&self, em: f32, ctx: Ctx) -> Bx {
        Bx { w: em * ctx.scale(self.f), ..Bx::default() }
    }

    /// Sätter ihop atomer vågrätt med TeX:s avstånd.
    fn list(&self, nodes: &[Node], ctx: Ctx, lead: Option<Class>) -> Bx {
        let mut atoms: Vec<(Bx, Class)> = Vec::new();
        for n in nodes {
            // Mellanrum och stilbyten är inga atomer men påverkar inte klassföljden.
            let a = self.atom(n, ctx);
            atoms.push(a);
        }
        // Binära operatorer som saknar vänster- eller högeroperand blir vanliga tecken.
        let mut prev = lead;
        for i in 0..atoms.len() {
            if atoms[i].1 == Class::Bin {
                let left_ok = !matches!(prev, None | Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct));
                if !left_ok {
                    atoms[i].1 = Class::Ord;
                }
            }
            if matches!(atoms[i].1, Class::Rel | Class::Close | Class::Punct) && i > 0 && atoms[i - 1].1 == Class::Bin {
                atoms[i - 1].1 = Class::Ord;
            }
            if !atoms[i].0.items.is_empty() || atoms[i].0.w != 0.0 || !matches!(nodes[i], Node::Space(_)) {
                prev = Some(atoms[i].1);
            }
        }
        if atoms.last().is_some_and(|a| a.1 == Class::Bin) {
            if let Some(a) = atoms.last_mut() {
                a.1 = Class::Ord;
            }
        }
        let s = ctx.scale(self.f);
        let mut out = Bx::default();
        let mut prev = lead;
        for (i, (b, class)) in atoms.iter().enumerate() {
            let is_space = matches!(nodes[i], Node::Space(_));
            if !is_space {
                if let Some(p) = prev {
                    let mu = space_mu(p, *class);
                    let mu = if mu < 0 { if ctx.script() { 0 } else { -mu } } else { mu };
                    out.w += f32::from(mu) / 18.0 * if ctx.script() { 1.0 } else { s };
                }
                prev = Some(*class);
            }
            let x = out.w;
            out.put(b, x, 0.0);
            out.w += b.w;
            out.asc = out.asc.max(b.asc);
            out.desc = out.desc.max(b.desc);
            out.italic = b.italic;
            out.attach = b.attach.map(|a| a + x);
        }
        out.single = atoms.len() == 1 && atoms[0].0.single;
        if atoms.len() != 1 {
            out.attach = None;
        }
        out
    }

    // ----------------------------------------------------------- atomer

    fn atom(&self, node: &Node, ctx: Ctx) -> (Bx, Class) {
        let f = self.f;
        let s = ctx.scale(f);
        match node {
            Node::Num(n) => (self.run(n.chars(), ctx, ctx.font, false), Class::Ord),
            Node::Var(c) => {
                let font = ctx.font;
                let m = styled(*c, font, true);
                let m = if f.has(m) { m } else { *c };
                (self.glyph_box(f.glyph(m), s), Class::Ord)
            }
            Node::Sym(c, class) => self.symbol(*c, *class, ctx),
            Node::Text(t) => {
                let font = match ctx.font {
                    FontStyle::Auto => FontStyle::Rm,
                    o => o,
                };
                (self.run(t.chars(), ctx, font, false), Class::Ord)
            }
            Node::Func { name, .. } => {
                let font = match ctx.font {
                    FontStyle::Auto | FontStyle::It => FontStyle::Rm,
                    o => o,
                };
                (self.run(name.chars(), ctx, font, false), Class::Op)
            }
            Node::Group(l) => (self.list(l, ctx, None), Class::Ord),
            Node::Scripts { base, sub, sup, limits } => self.scripts(base, sub.as_deref(), sup.as_deref(), *limits, ctx),
            Node::Frac { kind, num, den } => (self.frac(*kind, num, den, ctx), Class::Inner),
            Node::Sqrt { index, body } => (self.sqrt(index.as_deref(), body, ctx), Class::Ord),
            Node::Delim { open, close, body } => (self.delimited(*open, *close, body, ctx), Class::Inner),
            Node::Middle(c) => match ctx.mid {
                Some(h) => (self.stretch_centered(*c, h, ctx), Class::Ord),
                None => self.symbol(*c, Class::Ord, ctx),
            },
            Node::BigDelim { ch, size, class } => {
                let h = [0.85, 1.15, 1.45, 1.75][usize::from((*size).clamp(1, 4)) - 1] * s;
                if *ch == '.' {
                    return (Bx::default(), *class);
                }
                (self.stretch_centered(*ch, h, ctx), *class)
            }
            Node::Matrix { env, rows, cols } => (self.matrix(*env, rows, cols, ctx), Class::Ord),
            Node::Accent { kind, body } => (self.accent(*kind, body, ctx), Class::Ord),
            Node::Line { over, body } => (self.line(*over, body, ctx), Class::Ord),
            Node::Brace { over, body } => (self.brace(*over, body, ctx), Class::Ord),
            Node::Stack { over, note, base } => (self.stack(*over, note, base, ctx), Class::Rel),
            Node::Font { style, body } => (self.list(body, Ctx { font: *style, ..ctx }, None), Class::Ord),
            Node::Space(sp) => (self.space(sp.em(), ctx), Class::Ord),
            Node::Styled { style, body } => (self.list(body, Ctx { style: *style, ..ctx }, None), Class::Ord),
            Node::Classed { class, body } => (self.list(body, ctx, None), *class),
        }
    }

    fn symbol(&self, c: char, class: Class, ctx: Ctx) -> (Bx, Class) {
        let f = self.f;
        let s = ctx.scale(f);
        let m = styled(c, if matches!(ctx.font, FontStyle::Auto | FontStyle::It) { FontStyle::Auto } else { ctx.font }, false);
        let c2 = if f.has(m) { m } else { c };
        // Grekiska gemener är kursiva som variabler.
        let c2 = if ctx.font == FontStyle::Auto && ('α'..='ω').contains(&c2) || matches!(c2, 'ϑ' | 'ϰ' | 'ϕ' | 'ϱ' | 'ϖ' | 'ϵ') {
            let m = styled(c2, FontStyle::It, true);
            if f.has(m) { m } else { c2 }
        } else {
            c2
        };
        let mut gid = f.glyph(c2);
        if class == Class::Op && ctx.display() && !matches!(c, '+') {
            // Stora operatorer i visningsläge: första variant som är minst `display_op_min` hög.
            if let Some(con) = f.vertical(gid) {
                if let Some((g, _)) = con.variants.iter().find(|(_, adv)| *adv >= f.display_op_min).or(con.variants.last()) {
                    gid = *g;
                }
            }
        }
        let mut b = self.glyph_box(gid, s);
        if class == Class::Op && b.height() > 0.0 {
            // Stora operatorer centreras kring axeln.
            let axis = f.k(k::AXIS) * s;
            let cur = (b.asc - b.desc) / 2.0;
            let dy = axis - cur;
            for it in &mut b.items {
                *it = shift(*it, 0.0, dy);
            }
            b.asc += dy;
            b.desc -= dy;
            b.single = false;
        }
        (b, class)
    }

    // ----------------------------------------------------------- skript

    fn scripts(&self, base: &Node, sub: Option<&[Node]>, sup: Option<&[Node]>, limits: Limits, ctx: Ctx) -> (Bx, Class) {
        let f = self.f;
        let s = ctx.scale(f);
        let (b, class) = self.atom(base, ctx);
        // Gränser ovanför/under?
        let natural = match base {
            Node::Sym(c, Class::Op) => ctx.display() && !matches!(c, '∫' | '∬' | '∭' | '∮' | '∯'),
            Node::Func { limits: true, .. } => ctx.display(),
            Node::Brace { .. } | Node::Stack { .. } => true,
            _ => false,
        };
        let over_under = match limits {
            Limits::Limits => class == Class::Op,
            Limits::NoLimits => false,
            Limits::Default => natural,
        } || matches!(base, Node::Brace { .. });
        let ssup = self.opt_list(sup, ctx.sup());
        let ssub = self.opt_list(sub, ctx.sub());
        if over_under {
            let w = b.w.max(ssup.as_ref().map_or(0.0, |x| x.w)).max(ssub.as_ref().map_or(0.0, |x| x.w));
            let mut out = Bx { w, asc: b.asc, desc: b.desc, ..Bx::default() };
            let skew = b.italic / 2.0;
            let bx = (w - b.w) / 2.0;
            out.put(&b, bx, 0.0);
            if let Some(u) = &ssup {
                let gap = (f.k(k::UPPER_LIMIT_GAP) * s).max(f.k(k::UPPER_LIMIT_RISE) * s - u.desc);
                let y = b.asc + gap + u.desc;
                out.put(u, (w - u.w) / 2.0 + skew, y);
                out.asc = y + u.asc;
            }
            if let Some(l) = &ssub {
                let gap = (f.k(k::LOWER_LIMIT_GAP) * s).max(f.k(k::LOWER_LIMIT_DROP) * s - l.asc);
                let y = b.desc + gap + l.asc;
                out.put(l, (w - l.w) / 2.0 - skew, -y);
                out.desc = y + l.desc;
            }
            return (out, class);
        }
        if ssup.is_none() && ssub.is_none() {
            return (b, class);
        }
        let sc = ctx.sup().scale(f);
        let (mut up_init, mut down_init) = (0.0f32, 0.0f32);
        if !b.single {
            up_init = b.asc - f.k(k::SUP_DROP_MAX) * sc;
            down_init = b.desc + f.k(k::SUB_DROP_MIN) * sc;
        }
        let sup_min = if ctx.cramped { f.k(k::SUP_SHIFT_UP_CRAMPED) } else { f.k(k::SUP_SHIFT_UP) } * s;
        let mut out = Bx { w: b.w, asc: b.asc, desc: b.desc, ..Bx::default() };
        out.put(&b, 0.0, 0.0);
        let after = f.k(k::SPACE_AFTER_SCRIPT) * s;
        let (mut up, mut down) = (0.0, 0.0);
        match (&ssup, &ssub) {
            (Some(u), None) => {
                up = up_init.max(sup_min).max(u.desc + f.k(k::SUP_BOTTOM_MIN) * s);
            }
            (None, Some(d)) => {
                down = down_init.max(f.k(k::SUB_SHIFT_DOWN) * s).max(d.asc - f.k(k::SUB_TOP_MAX) * s);
            }
            (Some(u), Some(d)) => {
                up = up_init.max(sup_min).max(u.desc + f.k(k::SUP_BOTTOM_MIN) * s);
                down = down_init.max(f.k(k::SUB_SHIFT_DOWN) * s);
                let gap = (up - u.desc) - (d.asc - down);
                let min = f.k(k::SUBSUP_GAP_MIN) * s;
                if gap < min {
                    down += min - gap;
                }
                let psi = f.k(k::SUP_BOTTOM_MAX_WITH_SUB) * s - (up - u.desc);
                if psi > 0.0 {
                    up += psi;
                    down -= psi;
                }
            }
            (None, None) => {}
        }
        // Stora operatorer (integraler): typsnittets bredd rymmer redan överkanten, underkanten ligger
        // en kursivkorrektion längre in. Bokstäver: överskriftens fot flyttas ut en kursivkorrektion.
        let (sup_x, sub_x) = if class == Class::Op { (b.w, b.w - b.italic) } else { (b.w + b.italic, b.w) };
        let mut right = b.w;
        if let Some(u) = &ssup {
            let x = sup_x;
            out.put(u, x, up);
            out.asc = out.asc.max(up + u.asc);
            right = right.max(x + u.w);
        }
        if let Some(d) = &ssub {
            out.put(d, sub_x, -down);
            out.desc = out.desc.max(down + d.desc);
            right = right.max(sub_x + d.w);
        }
        out.w = right + after;
        (out, class)
    }

    fn opt_list(&self, l: Option<&[Node]>, ctx: Ctx) -> Option<Bx> {
        l.map(|l| self.list(l, ctx, None))
    }

    // ----------------------------------------------------------- bråk

    fn frac(&self, kind: FracKind, num: &[Node], den: &[Node], ctx: Ctx) -> Bx {
        let f = self.f;
        let ctx = match kind {
            FracKind::DFrac => Ctx { style: MStyle::Display, ..ctx },
            FracKind::TFrac => Ctx { style: MStyle::Text, ..ctx },
            _ => ctx,
        };
        let s = ctx.scale(f);
        let disp = ctx.display();
        let n = self.list(num, ctx.num(), None);
        let d = self.list(den, ctx.den(), None);
        let axis = f.k(k::AXIS) * s;
        let bar = kind != FracKind::Binom;
        let (mut up, mut down);
        let mut out = Bx::default();
        let pad = 0.06 * s;
        let w = n.w.max(d.w) + 2.0 * pad;
        let t = f.k(k::FRAC_RULE) * s;
        if bar {
            up = f.k(if disp { k::FRAC_NUM_DISPLAY_UP } else { k::FRAC_NUM_UP }) * s;
            down = f.k(if disp { k::FRAC_DEN_DISPLAY_DOWN } else { k::FRAC_DEN_DOWN }) * s;
            let ng = f.k(if disp { k::FRAC_NUM_DISPLAY_GAP } else { k::FRAC_NUM_GAP }) * s;
            let dg = f.k(if disp { k::FRAC_DEN_DISPLAY_GAP } else { k::FRAC_DEN_GAP }) * s;
            up = up.max(axis + t / 2.0 + ng + n.desc);
            down = down.max(d.asc + dg + t / 2.0 - axis);
            out.items.push(Item::Rule { x: 0.0, y: axis - t / 2.0, w, h: t.max(0.01) });
        } else {
            up = f.k(if disp { k::STACK_TOP_DISPLAY_UP } else { k::STACK_TOP_UP }) * s;
            down = f.k(if disp { k::STACK_BOTTOM_DISPLAY_DOWN } else { k::STACK_BOTTOM_DOWN }) * s;
            let min = f.k(if disp { k::STACK_DISPLAY_GAP } else { k::STACK_GAP }) * s;
            let gap = (up - n.desc) - (d.asc - down);
            if gap < min {
                up += (min - gap) / 2.0;
                down += (min - gap) / 2.0;
            }
        }
        out.put(&n, (w - n.w) / 2.0, up);
        out.put(&d, (w - d.w) / 2.0, -down);
        out.w = w;
        out.asc = (up + n.asc).max(axis + t / 2.0);
        out.desc = (down + d.desc).max(t / 2.0 - axis);
        if kind == FracKind::Binom {
            return self.wrap(Some('('), Some(')'), out, ctx);
        }
        out
    }

    // ----------------------------------------------------------- rot

    fn sqrt(&self, index: Option<&[Node]>, body: &[Node], ctx: Ctx) -> Bx {
        let f = self.f;
        let s = ctx.scale(f);
        let bd = self.list(body, ctx.crampedd(), None);
        let t = f.k(k::RADICAL_RULE) * s;
        let mut gap = f.k(if ctx.display() { k::RADICAL_DISPLAY_GAP } else { k::RADICAL_GAP }) * s;
        let needed = bd.height() + gap + t;
        let rad = self.stretch_v('√', needed, s);
        if rad.h > needed {
            gap += (rad.h - needed) / 2.0;
        }
        let top = bd.asc + gap + t;
        let bottom = top - rad.h;
        let mut out = Bx::default();
        let mut x0 = 0.0;
        let mut deg: Option<Bx> = None;
        if let Some(ix) = index {
            let dg = self.list(ix, Ctx { style: MStyle::ScriptScript, cramped: false, ..ctx }, None);
            x0 = (f.k(k::RADICAL_KERN_BEFORE) * s + dg.w + f.k(k::RADICAL_KERN_AFTER) * s).max(0.0);
            deg = Some(dg);
        }
        for it in &rad.items {
            out.items.push(shift(*it, x0, bottom));
        }
        out.items.push(Item::Rule { x: x0 + rad.w - 0.01, y: top - t, w: bd.w + 0.01, h: t.max(0.01) });
        out.put(&bd, x0 + rad.w, 0.0);
        out.w = x0 + rad.w + bd.w;
        out.asc = top + f.k(k::RADICAL_EXTRA) * s;
        out.desc = (-bottom).max(bd.desc);
        if let Some(dg) = deg {
            let raise = bottom + f.radical_raise_pct * rad.h;
            let dx = (f.k(k::RADICAL_KERN_BEFORE) * s).max(0.0);
            out.put(&dg, dx, raise);
            out.asc = out.asc.max(raise + dg.asc);
        }
        out
    }

    // ----------------------------------------------------------- avgränsare

    fn delimited(&self, open: Option<char>, close: Option<char>, body: &[Node], ctx: Ctx) -> Bx {
        // Första passet utan `\middle` ger höjden som mellanstrecken sträcks till.
        let first = self.list(body, Ctx { mid: None, ..ctx }, None);
        let b = if body.iter().any(|n| matches!(n, Node::Middle(_))) {
            let h = self.delim_target(&first, ctx);
            self.list(body, Ctx { mid: Some(h), ..ctx }, None)
        } else {
            first
        };
        self.wrap(open, close, b, ctx)
    }

    /// Målhöjd för avgränsare runt `b`: TeX:s regel (delimiterfactor 901, shortfall 0,5 em).
    fn delim_target(&self, b: &Bx, ctx: Ctx) -> f32 {
        let s = ctx.scale(self.f);
        let axis = self.f.k(k::AXIS) * s;
        let delta = (b.asc - axis).max(b.desc + axis);
        (2.0 * delta * 0.901).max(2.0 * delta - 0.5 * s)
    }

    fn wrap(&self, open: Option<char>, close: Option<char>, b: Bx, ctx: Ctx) -> Bx {
        let target = self.delim_target(&b, ctx);
        let l = open.map(|c| self.stretch_centered(c, target, ctx));
        let r = close.map(|c| self.stretch_centered(c, target, ctx));
        let mut out = Bx::default();
        let mut x = 0.0;
        if let Some(l) = &l {
            out.put(l, 0.0, 0.0);
            x = l.w;
            out.asc = out.asc.max(l.asc);
            out.desc = out.desc.max(l.desc);
        }
        out.put(&b, x, 0.0);
        out.asc = out.asc.max(b.asc);
        out.desc = out.desc.max(b.desc);
        x += b.w;
        if let Some(r) = &r {
            out.put(r, x, 0.0);
            x += r.w;
            out.asc = out.asc.max(r.asc);
            out.desc = out.desc.max(r.desc);
        }
        out.w = x;
        out
    }

    /// Sträckt tecken (minst `target` högt) centrerat kring axeln.
    fn stretch_centered(&self, c: char, target: f32, ctx: Ctx) -> Bx {
        let s = ctx.scale(self.f);
        let st = self.stretch_v(c, target, s);
        let axis = self.f.k(k::AXIS) * s;
        let bottom = axis - st.h / 2.0;
        let mut b = Bx { w: st.w, asc: bottom + st.h, desc: -bottom, ..Bx::default() };
        for it in &st.items {
            b.items.push(shift(*it, 0.0, bottom));
        }
        b.asc = b.asc.max(0.0);
        b.desc = b.desc.max(0.0);
        b
    }

    /// Lodrät sträckning av `c` till minst `target` (em). Resultatet har underkanten på y = 0.
    fn stretch_v(&self, c: char, target: f32, s: f32) -> Stretch {
        let f = self.f;
        let c = match c {
            '⟨' if !f.has('⟨') => '〈',
            c => c,
        };
        let gid = f.glyph(c);
        let single = |g: u16| -> Stretch {
            let (x0, y0, _, y1) = f.bbox(g).unwrap_or((0.0, 0.0, 0.0, 0.0));
            let _ = x0;
            Stretch { items: vec![Item::Glyph { gid: g, x: 0.0, y: -y0 * s, s }], w: f.advance(g) * s, h: (y1 - y0) * s }
        };
        let Some(con) = f.vertical(gid) else { return single(gid) };
        let size_of = |g: u16| f.bbox(g).map_or(0.0, |(_, y0, _, y1)| (y1 - y0) * s);
        let base = single(gid);
        if base.h >= target {
            return base;
        }
        for (g, _) in &con.variants {
            if size_of(*g) >= target {
                return single(*g);
            }
        }
        if !con.assembly.is_empty() {
            return self.assemble(con, target, s, true);
        }
        match con.variants.last() {
            Some((g, _)) => single(*g),
            None => base,
        }
    }

    /// Sammansättning av delar (lodrät: underifrån och upp; vågrät: vänster till höger).
    fn assemble(&self, con: &font::Construction, target: f32, s: f32, vertical: bool) -> Stretch {
        let f = self.f;
        let parts = &con.assembly;
        let min_ov = f.min_overlap * s;
        let total = |n: usize| -> f32 {
            let mut count = 0;
            let mut sum = 0.0;
            for p in parts {
                let reps = if p.extender { n } else { 1 };
                for _ in 0..reps {
                    sum += p.full * s;
                    count += 1;
                }
            }
            sum - (count.max(1) - 1) as f32 * min_ov
        };
        let mut n = 0;
        while total(n) < target && n < 24 {
            n += 1;
        }
        // En förlängare minst en gång om det finns någon (annars blir mitten tom).
        if n == 0 && parts.iter().any(|p| p.extender) {
            n = 1;
        }
        let mut seq: Vec<&font::Part> = Vec::new();
        for p in parts {
            let reps = if p.extender { n } else { 1 };
            for _ in 0..reps {
                seq.push(p);
            }
        }
        let sum: f32 = seq.iter().map(|p| p.full * s).sum();
        let k = seq.len().max(1);
        let max_ov = seq
            .windows(2)
            .map(|w| w[0].end.min(w[1].start) * s)
            .fold(f32::INFINITY, f32::min);
        let mut ov = if k > 1 { (sum - target) / (k - 1) as f32 } else { 0.0 };
        ov = ov.clamp(min_ov, if max_ov.is_finite() { max_ov.max(min_ov) } else { min_ov });
        let mut items = Vec::new();
        let mut pos = 0.0;
        let mut cross: f32 = 0.0;
        for p in &seq {
            let bb = f.bbox(p.gid);
            if vertical {
                let y0 = bb.map_or(0.0, |b| b.1);
                items.push(Item::Glyph { gid: p.gid, x: 0.0, y: pos - y0 * s, s });
                cross = cross.max(f.advance(p.gid) * s);
            } else {
                items.push(Item::Glyph { gid: p.gid, x: pos, y: 0.0, s });
                cross = cross.max(bb.map_or(0.0, |b| b.3) * s);
            }
            pos += p.full * s - ov;
        }
        let len = pos + ov;
        if vertical { Stretch { items, w: cross, h: len } } else { Stretch { items, w: len, h: cross } }
    }

    // ----------------------------------------------------------- matriser

    fn matrix(&self, env: MatrixEnv, rows: &[Vec<Vec<Node>>], cols: &str, ctx: Ctx) -> Bx {
        let f = self.f;
        let small = matches!(env, MatrixEnv::SmallMatrix | MatrixEnv::SubStack);
        let cctx = if small {
            Ctx { style: MStyle::Script, ..ctx }
        } else if matches!(env, MatrixEnv::Aligned | MatrixEnv::Gathered) {
            Ctx { style: if ctx.display() { MStyle::Display } else { MStyle::Text }, ..ctx }
        } else {
            Ctx { style: if ctx.display() { MStyle::Text } else { ctx.style }, ..ctx }
        };
        let s = ctx.scale(f);
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
        let cells: Vec<Vec<Bx>> = rows
            .iter()
            .map(|r| {
                r.iter()
                    .enumerate()
                    .map(|(j, c)| {
                        let lead = (env == MatrixEnv::Aligned && j % 2 == 1).then_some(Class::Ord);
                        self.list(c, cctx, lead)
                    })
                    .collect()
            })
            .collect();
        let mut widths = vec![0.0f32; ncols];
        for r in &cells {
            for (j, c) in r.iter().enumerate() {
                widths[j] = widths[j].max(c.w);
            }
        }
        let cs = cctx.scale(f);
        let align = |j: usize| -> char {
            match env {
                MatrixEnv::Aligned => {
                    if j % 2 == 0 {
                        'r'
                    } else {
                        'l'
                    }
                }
                MatrixEnv::Cases => 'l',
                MatrixEnv::Array => cols.chars().nth(j).unwrap_or('c'),
                _ => 'c',
            }
        };
        let sep = |j: usize| -> f32 {
            match env {
                MatrixEnv::Aligned => {
                    if j % 2 == 0 {
                        0.0
                    } else {
                        2.0 * cs
                    }
                }
                MatrixEnv::Cases => 1.0 * cs,
                MatrixEnv::SmallMatrix => 0.7 * cs,
                MatrixEnv::SubStack => 0.0,
                _ => 1.0 * cs,
            }
        };
        let (strut_a, strut_d) = if small { (0.7 * 0.9 * cs, 0.3 * 0.9 * cs) } else { (0.84 * cs, 0.36 * cs) };
        let row_gap = if small { 0.02 } else { 0.12 } * cs;
        let mut rh: Vec<(f32, f32)> = Vec::new();
        for r in &cells {
            let a = r.iter().map(|c| c.asc).fold(strut_a, f32::max);
            let d = r.iter().map(|c| c.desc).fold(strut_d, f32::max);
            rh.push((a, d));
        }
        let total_h: f32 = rh.iter().map(|(a, d)| a + d).sum::<f32>() + row_gap * rh.len().saturating_sub(1) as f32;
        let total_w: f32 = widths.iter().sum::<f32>() + (1..ncols).map(sep).sum::<f32>();
        let axis = f.k(k::AXIS) * s;
        // Översta radens baslinje så att matrisen centreras kring axeln.
        let mut y = axis + total_h / 2.0;
        let mut body = Bx { w: total_w, ..Bx::default() };
        for (i, r) in cells.iter().enumerate() {
            y -= rh[i].0;
            let mut x = 0.0;
            for (j, c) in r.iter().enumerate() {
                let dx = match align(j) {
                    'l' => 0.0,
                    'r' => widths[j] - c.w,
                    _ => (widths[j] - c.w) / 2.0,
                };
                body.put(c, x + dx, y);
                x += widths[j] + if j + 1 < ncols { sep(j + 1) } else { 0.0 };
            }
            y -= rh[i].1 + row_gap;
        }
        body.asc = axis + total_h / 2.0;
        body.desc = total_h / 2.0 - axis;
        let (open, close) = env.delims();
        if open.is_none() && close.is_none() {
            return body;
        }
        // Matrisens egen höjd styr avgränsarna (inte TeX:s \left-regel), men minst en rad.
        let w = self.wrap(open, close, body, ctx);
        // Mellanrum mellan avgränsare och innehåll: en tunn rymd på varje sida.
        let pad = 0.1 * s;
        let mut out = Bx { w: w.w + 2.0 * pad, asc: w.asc, desc: w.desc, ..Bx::default() };
        out.put(&w, pad, 0.0);
        out
    }

    // ----------------------------------------------------------- accenter och streck

    fn accent(&self, kind: AccentKind, body: &[Node], ctx: Ctx) -> Bx {
        let f = self.f;
        let s = ctx.scale(f);
        let b = self.list(body, ctx.crampedd(), None);
        let mark = kind.mark();
        let spacing = match mark {
            '\u{302}' => '^',
            '\u{303}' => '~',
            '\u{304}' => '\u{AF}',
            '\u{307}' => '\u{2D9}',
            '\u{308}' => '\u{A8}',
            '\u{30C}' => '\u{2C7}',
            '\u{306}' => '\u{2D8}',
            '\u{301}' => '\u{B4}',
            '\u{300}' => '`',
            '\u{30A}' => '\u{2DA}',
            '\u{20D7}' => '→',
            '\u{20D6}' => '←',
            _ => mark,
        };
        let mut gid = f.glyph(mark);
        if gid == 0 {
            gid = f.glyph(spacing);
        }
        // Brett accent: vågrät variant som täcker basen.
        let mut st: Option<Stretch> = None;
        if kind.is_wide() || b.w > 0.0 {
            if let Some(con) = f.horizontal(gid) {
                let size_of = |g: u16| f.advance(g) * s;
                let want = if kind.is_wide() { b.w } else { 0.0 };
                if want > 0.0 {
                    let pick = con.variants.iter().find(|(g, _)| size_of(*g) >= want).or(con.variants.last()).map(|(g, _)| *g);
                    if let Some(g) = pick {
                        if size_of(g) >= want || con.assembly.is_empty() {
                            st = Some(self.single_h(g, s));
                        } else {
                            st = Some(self.assemble(con, want, s, false));
                        }
                    }
                }
            }
        }
        let st = st.unwrap_or_else(|| self.single_h(gid, s));
        let (ax0, ax1, ay0, ay1) = ink_bounds(&st.items, f);
        let base_attach = b.attach.unwrap_or(b.w / 2.0);
        let dx = base_attach - (ax0 + ax1) / 2.0;
        let delta = (b.asc - f.k(k::ACCENT_BASE) * s).max(0.0);
        let mut out = Bx { w: b.w, asc: b.asc, desc: b.desc, ..Bx::default() };
        out.put(&b, 0.0, 0.0);
        for it in &st.items {
            out.items.push(shift(*it, dx, delta));
        }
        out.asc = out.asc.max(delta + ay1);
        let _ = ay0;
        out.w = b.w.max(0.0);
        out.attach = b.attach;
        out
    }

    fn single_h(&self, g: u16, s: f32) -> Stretch {
        let (_, y0, _, y1) = self.f.bbox(g).unwrap_or((0.0, 0.0, 0.0, 0.0));
        Stretch { items: vec![Item::Glyph { gid: g, x: 0.0, y: 0.0, s }], w: self.f.advance(g) * s, h: (y1 - y0) * s }
    }

    fn line(&self, over: bool, body: &[Node], ctx: Ctx) -> Bx {
        let f = self.f;
        let s = ctx.scale(f);
        let b = self.list(body, if over { ctx.crampedd() } else { ctx }, None);
        let mut out = Bx { w: b.w, asc: b.asc, desc: b.desc, ..Bx::default() };
        out.put(&b, 0.0, 0.0);
        if over {
            let (gap, t) = (f.k(k::OVERBAR_GAP) * s, f.k(k::OVERBAR_RULE) * s);
            out.items.push(Item::Rule { x: 0.0, y: b.asc + gap, w: b.w, h: t.max(0.01) });
            out.asc = b.asc + gap + t + f.k(k::OVERBAR_EXTRA) * s;
        } else {
            let (gap, t) = (f.k(k::UNDERBAR_GAP) * s, f.k(k::UNDERBAR_RULE) * s);
            out.items.push(Item::Rule { x: 0.0, y: -(b.desc + gap + t), w: b.w, h: t.max(0.01) });
            out.desc = b.desc + gap + t + f.k(k::UNDERBAR_EXTRA) * s;
        }
        out
    }

    fn brace(&self, over: bool, body: &[Node], ctx: Ctx) -> Bx {
        let f = self.f;
        let s = ctx.scale(f);
        let b = self.list(body, ctx, None);
        let c = if over { '\u{23DE}' } else { '\u{23DF}' };
        let gid = f.glyph(c);
        let st = match f.horizontal(gid) {
            Some(con) => {
                let size_of = |g: u16| f.advance(g) * s;
                match con.variants.iter().find(|(g, _)| size_of(*g) >= b.w) {
                    Some((g, _)) => self.single_h(*g, s),
                    None if !con.assembly.is_empty() => self.assemble(con, b.w, s, false),
                    None => self.single_h(con.variants.last().map_or(gid, |v| v.0), s),
                }
            }
            None => self.single_h(gid, s),
        };
        let (_, _, y0, y1) = ink_bounds(&st.items, f);
        let gap = 0.08 * s;
        let mut out = Bx { w: b.w.max(st.w), asc: b.asc, desc: b.desc, ..Bx::default() };
        let bx = (out.w - b.w) / 2.0;
        out.put(&b, bx, 0.0);
        let dx = (out.w - st.w) / 2.0;
        let dy = if over { b.asc + gap - y0 } else { -(b.desc + gap + y1) };
        for it in &st.items {
            out.items.push(shift(*it, dx, dy));
        }
        if over {
            out.asc = b.asc + gap + (y1 - y0);
        } else {
            out.desc = b.desc + gap + (y1 - y0);
        }
        out
    }

    fn stack(&self, over: bool, note: &[Node], base: &[Node], ctx: Ctx) -> Bx {
        let f = self.f;
        let s = ctx.scale(f);
        let b = self.list(base, ctx, None);
        let n = self.list(note, ctx.sup(), None);
        let w = b.w.max(n.w);
        let mut out = Bx { w, asc: b.asc, desc: b.desc, ..Bx::default() };
        out.put(&b, (w - b.w) / 2.0, 0.0);
        if over {
            let gap = (f.k(k::UPPER_LIMIT_GAP) * s).max(f.k(k::UPPER_LIMIT_RISE) * s - n.desc);
            let y = b.asc + gap + n.desc;
            out.put(&n, (w - n.w) / 2.0, y);
            out.asc = y + n.asc;
        } else {
            let gap = (f.k(k::LOWER_LIMIT_GAP) * s).max(f.k(k::LOWER_LIMIT_DROP) * s - n.asc);
            let y = b.desc + gap + n.asc;
            out.put(&n, (w - n.w) / 2.0, -y);
            out.desc = y + n.desc;
        }
        out
    }
}

struct Stretch {
    items: Vec<Item>,
    w: f32,
    h: f32,
}

/// Syns utsträckning (x0, x1, y0, y1) i em för glyferna i `items`.
fn ink_bounds(items: &[Item], f: &MathFont) -> (f32, f32, f32, f32) {
    let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for it in items {
        match *it {
            Item::Glyph { gid, x, y, s } => {
                if let Some((a, b, c, d)) = f.bbox(gid) {
                    x0 = x0.min(x + a * s);
                    x1 = x1.max(x + c * s);
                    y0 = y0.min(y + b * s);
                    y1 = y1.max(y + d * s);
                }
            }
            Item::Rule { x, y, w, h } => {
                x0 = x0.min(x);
                x1 = x1.max(x + w);
                y0 = y0.min(y);
                y1 = y1.max(y + h);
            }
        }
    }
    if x0 > x1 { (0.0, 0.0, 0.0, 0.0) } else { (x0, x1, y0, y1) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::parse::parse;

    fn lay(src: &str, display: bool) -> Layout {
        layout(&parse(src).unwrap(), display).expect("layout")
    }

    fn glyphs(l: &Layout) -> Vec<(u16, f32, f32, f32)> {
        l.items
            .iter()
            .filter_map(|i| match *i {
                Item::Glyph { gid, x, y, s } => Some((gid, x, y, s)),
                _ => None,
            })
            .collect()
    }

    fn rules(l: &Layout) -> Vec<(f32, f32, f32, f32)> {
        l.items
            .iter()
            .filter_map(|i| match *i {
                Item::Rule { x, y, w, h } => Some((x, y, w, h)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn single_letter_has_positive_size() {
        let l = lay("x", false);
        assert!(l.w > 0.3 && l.w < 0.8, "w {}", l.w);
        assert!(l.asc > 0.3 && l.asc < 0.8);
        assert_eq!(glyphs(&l).len(), 1);
    }

    #[test]
    fn superscript_is_smaller_and_raised() {
        let l = lay("x^2", false);
        let g = glyphs(&l);
        assert_eq!(g.len(), 2);
        let f = font::font().unwrap();
        assert!((g[0].3 - 1.0).abs() < 1e-6);
        assert!((g[1].3 - f.script_pct).abs() < 1e-6);
        assert!(g[1].2 > 0.3, "höjd {}", g[1].2);
        assert!(g[1].1 >= g[0].1 + 0.4);
        // Subskript sänkt.
        let l = lay("x_i", false);
        let g = glyphs(&l);
        assert!(g[1].2 < -0.05);
    }

    #[test]
    fn script_of_script_shrinks_more() {
        let l = lay("x^{y^z}", false);
        let g = glyphs(&l);
        let f = font::font().unwrap();
        assert!((g[2].3 - f.script_script_pct).abs() < 1e-6);
    }

    #[test]
    fn fraction_has_a_rule_between_numerator_and_denominator() {
        let l = lay(r"\frac{a}{b}", false);
        let r = rules(&l);
        assert_eq!(r.len(), 1);
        let g = glyphs(&l);
        assert_eq!(g.len(), 2);
        assert!(g[0].2 > r[0].1, "täljare över strecket");
        assert!(g[1].2 < r[0].1, "nämnare under strecket");
        assert!(r[0].2 >= g[0].1.max(g[1].1));
        // Strecket ligger kring axeln.
        let axis = font::font().unwrap().k(k::AXIS);
        assert!((r[0].1 + r[0].3 / 2.0 - axis).abs() < 0.02);
        // Visningsstil ger större bråk än löpande text.
        let d = lay(r"\frac{a}{b}", true);
        assert!(d.asc + d.desc > l.asc + l.desc);
    }

    #[test]
    fn sqrt_draws_radical_and_overbar() {
        let l = lay(r"\sqrt{x}", false);
        assert_eq!(rules(&l).len(), 1);
        assert!(glyphs(&l).len() >= 2);
        let (_, y, w, _) = rules(&l)[0];
        assert!(y > 0.5 && w > 0.3);
        // Rotindex tillkommer.
        let n = lay(r"\sqrt[3]{x}", false);
        assert!(n.w > l.w);
    }

    #[test]
    fn big_operator_limits_go_above_and_below_in_display() {
        let d = lay(r"\sum_{i=1}^{n} i", true);
        let g = glyphs(&d);
        // Summatecknet (första glyfen) har gränserna ovanför och under.
        let sum = g[0];
        let max_y = g.iter().map(|x| x.2).fold(f32::MIN, f32::max);
        let min_y = g.iter().map(|x| x.2).fold(f32::MAX, f32::min);
        assert!(max_y > sum.2 + 0.4 && min_y < sum.2 - 0.4, "{max_y} {min_y}");
        // I löpande text hamnar de vid sidan av.
        let t = lay(r"\sum_{i=1}^{n} i", false);
        assert!(t.asc + t.desc < d.asc + d.desc);
        // Integral har skript vid sidan även i visningsläge.
        let i = lay(r"\int_0^1 f", true);
        let g = glyphs(&i);
        assert!(g[1].1 > g[0].1 + 0.3);
    }

    #[test]
    fn delimiters_stretch_with_content() {
        let small = lay(r"\left( x \right)", false);
        let tall = lay(r"\left( \frac{a}{b} \right)", true);
        assert!(tall.asc + tall.desc > small.asc + small.desc);
        let g = glyphs(&tall);
        let paren = g[0];
        let f = font::font().unwrap();
        let (_, y0, _, y1) = f.bbox(paren.0).unwrap();
        let ext = (y1 - y0) * paren.3;
        assert!(ext > 1.0, "parentesen sträckt: {ext}");
        // Mycket hög matris använder sammansättning av delar.
        let huge = lay(r"\left[ \begin{matrix} a \\ b \\ c \\ d \\ e \\ f \\ g \end{matrix} \right]", true);
        assert!(glyphs(&huge).len() > 8);
    }

    #[test]
    fn matrix_cells_form_a_grid() {
        let l = lay(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}", true);
        let g = glyphs(&l);
        // Två parenteser + fyra bokstäver (parenteserna kan vara delar).
        let letters: Vec<_> = g.iter().filter(|x| x.3 == 1.0).collect();
        assert!(letters.len() >= 4);
        let ys: std::collections::BTreeSet<i32> = letters.iter().map(|x| (x.2 * 100.0) as i32).collect();
        assert!(ys.len() >= 2);
        assert!(l.asc + l.desc > 1.8, "två rader hög: {}", l.asc + l.desc);
    }

    #[test]
    fn accents_sit_above_the_base() {
        let l = lay(r"\hat{x}", false);
        let plain = lay("x", false);
        assert!(l.asc > plain.asc + 0.05);
        let g = glyphs(&l);
        assert_eq!(g.len(), 2);
        let b = lay(r"\overline{x+y}", false);
        assert_eq!(rules(&b).len(), 1);
        assert!(rules(&b)[0].1 > b.asc - 0.3);
    }

    #[test]
    fn spacing_follows_atom_classes() {
        let tight = lay("ab", false);
        let rel = lay("a=b", false);
        let ones = lay("a", false).w + lay("=", false).w + lay("b", false).w;
        // Två tjocka mellanrum (5 mu) kring likhetstecknet.
        assert!((rel.w - ones - 2.0 * 5.0 / 18.0).abs() < 0.02, "{} vs {}", rel.w, ones);
        assert!(tight.w > 0.5);
        // Unärt minus får inget binärt avstånd.
        let un = lay("-a", false);
        let m = lay("-", false).w + lay("a", false).w;
        assert!((un.w - m).abs() < 0.01);
        // Skriptstil har inga tjocka avstånd.
        let sc = lay("x^{a=b}", false);
        let sc_ones = lay("a", false).w + lay("=", false).w + lay("b", false).w;
        let script = font::font().unwrap().script_pct;
        let g = glyphs(&sc);
        assert!(g.len() == 4);
        assert!(sc.w < lay("x", false).w + sc_ones * script + 0.2);
    }

    #[test]
    fn font_styles_map_to_math_alphanumerics() {
        assert_eq!(styled('x', FontStyle::Auto, true), '𝑥');
        assert_eq!(styled('x', FontStyle::Rm, true), 'x');
        assert_eq!(styled('R', FontStyle::Bb, false), 'ℝ');
        assert_eq!(styled('v', FontStyle::Bf, false), '𝐯');
        assert_eq!(styled('L', FontStyle::Cal, false), 'ℒ');
        assert_eq!(styled('h', FontStyle::Auto, true), 'ℎ');
        assert_eq!(styled('α', FontStyle::Auto, true), '𝛼');
        let bold = lay(r"\mathbf{v}", false);
        let it = lay("v", false);
        assert_ne!(glyphs(&bold)[0].0, glyphs(&it)[0].0);
    }

    #[test]
    fn text_and_functions_are_upright() {
        let l = lay(r"\sin x + \text{om } y", false);
        assert!(l.w > 2.0);
        let f = font::font().unwrap();
        let sin = glyphs(&lay(r"\sin", false));
        assert_eq!(sin.len(), 3);
        assert_eq!(sin[0].0, f.glyph('s'));
    }

    #[test]
    fn cases_and_aligned_layout() {
        let c = lay(r"f(x)=\begin{cases} 1 & x>0 \\ 0 & x\le 0 \end{cases}", true);
        assert!(c.asc + c.desc > 1.5);
        let a = lay(r"\begin{aligned} a &= b \\ &= c \end{aligned}", true);
        let g = glyphs(&a);
        // Likhetstecknen står i samma kolumn.
        let f = font::font().unwrap();
        let eq = f.glyph('=');
        let xs: Vec<f32> = g.iter().filter(|x| x.0 == eq).map(|x| x.1).collect();
        assert_eq!(xs.len(), 2);
        assert!((xs[0] - xs[1]).abs() < 0.01);
    }

    #[test]
    fn many_constructs_do_not_panic() {
        for src in [
            r"\int_0^\infty e^{-x^2}\,dx=\frac{\sqrt{\pi}}{2}",
            r"\widehat{abc} + \widetilde{xyz} + \vec{v} + \overrightarrow{AB}",
            r"\underbrace{a+b+c}_{n} + \overbrace{x+y}^{m}",
            r"\overset{!}{=} \underset{n\to\infty}{\lim} \stackrel{def}{=}",
            r"\Big( \Bigg[ \big| x \bigg\rangle",
            r"\left\{ x \,\middle|\, x>0 \right\}",
            r"\mathbb{R}^n \mathcal{L} \mathfrak{g} \mathsf{A} \mathtt{x}",
            r"\sum_{\substack{i<n \\ j<m}} a_{ij}",
            r"\begin{array}{c|cc} a & b & c \end{array}",
            r"\binom{n}{k} \dfrac{1}{2} \tfrac{3}{4}",
            r"x'' + f'(x)",
            r"\left. \frac{\partial f}{\partial x} \right|_{x=0}",
        ] {
            let l = lay(src, true);
            assert!(l.w > 0.0 && l.asc.is_finite() && l.desc.is_finite(), "{src}");
        }
    }
}
