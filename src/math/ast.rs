//! Syntaxträd för LaTeX-matematik. Träden skapas av `parse`, skrivs som LaTeX (`latex`), MathML
//! (`mathml`) och OMML (`omml`) och läggs ut av `layout`.

/// Atomklass (TeX:s) som styr avstånd mellan tecken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Ord,
    /// Stor operator (summa, integral, `\lim` …).
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
}

/// Teckensnittsstil (`\mathbf` …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FontStyle {
    /// Variabler kursiva, resten upprätt.
    #[default]
    Auto,
    /// `\mathrm`
    Rm,
    /// `\mathit`
    It,
    /// `\mathbf`
    Bf,
    /// `\boldsymbol`
    BfIt,
    /// `\mathcal`
    Cal,
    /// `\mathfrak`
    Frak,
    /// `\mathbb`
    Bb,
    /// `\mathsf`
    Sf,
    /// `\mathtt`
    Tt,
}

impl FontStyle {
    pub fn command(self) -> &'static str {
        match self {
            FontStyle::Auto => "",
            FontStyle::Rm => "mathrm",
            FontStyle::It => "mathit",
            FontStyle::Bf => "mathbf",
            FontStyle::BfIt => "boldsymbol",
            FontStyle::Cal => "mathcal",
            FontStyle::Frak => "mathfrak",
            FontStyle::Bb => "mathbb",
            FontStyle::Sf => "mathsf",
            FontStyle::Tt => "mathtt",
        }
    }
}

/// Matematikstil (TeX:s D, T, S, SS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MStyle {
    Display,
    Text,
    Script,
    ScriptScript,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limits {
    /// Efter stilen: ovanför/under i visningsläge för stora operatorer.
    Default,
    Limits,
    NoLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FracKind {
    Frac,
    /// `\dfrac`
    DFrac,
    /// `\tfrac`
    TFrac,
    /// `\binom`: inga streck, parenteser runt.
    Binom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// `\,`
    Thin,
    /// `\:`
    Med,
    /// `\;`
    Thick,
    /// `\!`
    NegThin,
    /// `\quad`
    Quad,
    /// `\qquad`
    QQuad,
    /// `\ ` och `~`
    Normal,
}

impl Space {
    /// Bredd i em.
    pub fn em(self) -> f32 {
        match self {
            Space::Thin => 3.0 / 18.0,
            Space::Med => 4.0 / 18.0,
            Space::Thick => 5.0 / 18.0,
            Space::NegThin => -3.0 / 18.0,
            Space::Quad => 1.0,
            Space::QQuad => 2.0,
            Space::Normal => 1.0 / 3.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixEnv {
    Matrix,
    PMatrix,
    BMatrix,
    BbMatrix,
    VMatrix,
    VvMatrix,
    SmallMatrix,
    Cases,
    /// `aligned`, `align`, `split`: kolumnerna höger, vänster, höger, vänster …
    Aligned,
    /// `gathered`, `gather`: centrerat.
    Gathered,
    /// `array` med kolumnspecifikation.
    Array,
    /// `\substack`
    SubStack,
}

impl MatrixEnv {
    pub fn name(self) -> &'static str {
        match self {
            MatrixEnv::Matrix => "matrix",
            MatrixEnv::PMatrix => "pmatrix",
            MatrixEnv::BMatrix => "bmatrix",
            MatrixEnv::BbMatrix => "Bmatrix",
            MatrixEnv::VMatrix => "vmatrix",
            MatrixEnv::VvMatrix => "Vmatrix",
            MatrixEnv::SmallMatrix => "smallmatrix",
            MatrixEnv::Cases => "cases",
            MatrixEnv::Aligned => "aligned",
            MatrixEnv::Gathered => "gathered",
            MatrixEnv::Array => "array",
            MatrixEnv::SubStack => "substack",
        }
    }

    /// Omslutande tecken (vänster, höger).
    pub fn delims(self) -> (Option<char>, Option<char>) {
        match self {
            MatrixEnv::PMatrix => (Some('('), Some(')')),
            MatrixEnv::BMatrix => (Some('['), Some(']')),
            MatrixEnv::BbMatrix => (Some('{'), Some('}')),
            MatrixEnv::VMatrix => (Some('|'), Some('|')),
            MatrixEnv::VvMatrix => (Some('\u{2016}'), Some('\u{2016}')),
            MatrixEnv::Cases => (Some('{'), None),
            _ => (None, None),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccentKind {
    Hat,
    Check,
    Tilde,
    Acute,
    Grave,
    Dot,
    DDot,
    Breve,
    Bar,
    Vec,
    Ring,
    WideHat,
    WideTilde,
    OverRightArrow,
    OverLeftArrow,
}

impl AccentKind {
    pub fn command(self) -> &'static str {
        match self {
            AccentKind::Hat => "hat",
            AccentKind::Check => "check",
            AccentKind::Tilde => "tilde",
            AccentKind::Acute => "acute",
            AccentKind::Grave => "grave",
            AccentKind::Dot => "dot",
            AccentKind::DDot => "ddot",
            AccentKind::Breve => "breve",
            AccentKind::Bar => "bar",
            AccentKind::Vec => "vec",
            AccentKind::Ring => "mathring",
            AccentKind::WideHat => "widehat",
            AccentKind::WideTilde => "widetilde",
            AccentKind::OverRightArrow => "overrightarrow",
            AccentKind::OverLeftArrow => "overleftarrow",
        }
    }

    /// Kombinerande tecken (Unicode) för accenten.
    pub fn mark(self) -> char {
        match self {
            AccentKind::Hat | AccentKind::WideHat => '\u{302}',
            AccentKind::Check => '\u{30C}',
            AccentKind::Tilde | AccentKind::WideTilde => '\u{303}',
            AccentKind::Acute => '\u{301}',
            AccentKind::Grave => '\u{300}',
            AccentKind::Dot => '\u{307}',
            AccentKind::DDot => '\u{308}',
            AccentKind::Breve => '\u{306}',
            AccentKind::Bar => '\u{304}',
            AccentKind::Vec | AccentKind::OverRightArrow => '\u{20D7}',
            AccentKind::Ring => '\u{30A}',
            AccentKind::OverLeftArrow => '\u{20D6}',
        }
    }

    pub fn is_wide(self) -> bool {
        matches!(self, AccentKind::WideHat | AccentKind::WideTilde | AccentKind::OverRightArrow | AccentKind::OverLeftArrow)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// Siffror, t.ex. `3.14`.
    Num(String),
    /// En variabel (bokstav), kursiv som standard.
    Var(char),
    /// Ett symboltecken med klass.
    Sym(char, Class),
    /// `\text{…}`: upprätt text med mellanslag.
    Text(String),
    /// Funktionsnamn (`\sin`, `\operatorname{rank}`): upprätt operator.
    Func { name: String, limits: bool },
    Group(Vec<Node>),
    Scripts { base: Box<Node>, sub: Option<Vec<Node>>, sup: Option<Vec<Node>>, limits: Limits },
    Frac { kind: FracKind, num: Vec<Node>, den: Vec<Node> },
    Sqrt { index: Option<Vec<Node>>, body: Vec<Node> },
    Delim { open: Option<char>, close: Option<char>, body: Vec<Node> },
    /// `\middle|` inne i en `Delim`.
    Middle(char),
    /// `\big`, `\Big`, `\bigg`, `\Bigg` (storlek 1..=4) och klass.
    BigDelim { ch: char, size: u8, class: Class },
    Matrix { env: MatrixEnv, rows: Vec<Vec<Vec<Node>>>, cols: String },
    Accent { kind: AccentKind, body: Vec<Node> },
    /// `\overline` / `\underline`.
    Line { over: bool, body: Vec<Node> },
    /// `\overbrace` / `\underbrace`.
    Brace { over: bool, body: Vec<Node> },
    /// `\overset{a}{b}`, `\underset{a}{b}`, `\stackrel{a}{b}`.
    Stack { over: bool, note: Vec<Node>, base: Vec<Node> },
    Font { style: FontStyle, body: Vec<Node> },
    Space(Space),
    /// `\displaystyle` …: gäller resten av gruppen.
    Styled { style: MStyle, body: Vec<Node> },
    /// `\mathbin`, `\mathrel`, `\mathop` …
    Classed { class: Class, body: Vec<Node> },
}
