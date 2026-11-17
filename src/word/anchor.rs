//! Förankrade bilder: var en flytande bild sitter (sidan, marginalen, spalten eller stycket,
//! med förskjutning eller justering), hur texten går runt den och avstånd till texten.
//!
//! Dialekten är attribut på `<img>`: `hrel`/`x`, `vrel`/`y`, `wrap`, `dist` (se
//! docs/dev/dokumentlage.md). Allt här är rena funktioner utan gränssnitt så att docx, odt, pdf,
//! layouten och ritningen räknar likadant. Mått är CSS-pixlar (ozoomade) om inget annat sägs.
//! Modellen och vilka lägen som finns följer Words `wp:anchor` (och ODF:s `draw:frame`).

/// Hur texten går runt bilden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Wrap {
    /// Text på sidan/sidorna av bilden (kvadratisk, tät och genomgående räknas som samma).
    #[default]
    Square,
    /// Ingen text vid sidorna: texten fortsätter under bilden.
    TopBottom,
    /// Bakom texten (ingen påverkan på texten).
    Behind,
    /// Framför texten (ingen påverkan på texten).
    Front,
}

impl Wrap {
    /// Påverkar bilden textens placering?
    pub fn affects_text(self) -> bool {
        matches!(self, Wrap::Square | Wrap::TopBottom)
    }
}

/// Vad vågrät placering räknas från.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HRel {
    #[default]
    Column,
    Margin,
    Page,
}

/// Vad lodrät placering räknas från.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VRel {
    #[default]
    Paragraph,
    Margin,
    Page,
}

/// Läge längs en axel: förskjutning från referenskanten, eller justering mot referensen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pos {
    Off(f32),
    /// Vänster respektive överkant.
    Start,
    Center,
    /// Höger respektive nederkant.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub hrel: HRel,
    pub h: Pos,
    pub vrel: VRel,
    pub v: Pos,
    pub wrap: Wrap,
    /// Avstånd till texten: ovan, höger, under, vänster.
    pub dist: [f32; 4],
}

/// Luft mot texten som Word och Kladd normalt använder (12 px = 114300 EMU).
pub const DEFAULT_DIST: [f32; 4] = [0.0, 12.0, 0.0, 12.0];

impl Default for Anchor {
    fn default() -> Self {
        Anchor { hrel: HRel::Column, h: Pos::Off(0.0), vrel: VRel::Paragraph, v: Pos::Off(0.0), wrap: Wrap::Square, dist: DEFAULT_DIST }
    }
}

fn num(v: f32) -> String {
    let r = (v * 10.0).round() / 10.0;
    format!("{r}")
}

fn parse_pos(v: &str, names: [&str; 3]) -> Option<Pos> {
    let t = v.trim().to_ascii_lowercase();
    if t == names[0] || (names[0] == "left" && t == "inside") {
        return Some(Pos::Start);
    }
    if t == names[1] {
        return Some(Pos::Center);
    }
    if t == names[2] || (names[2] == "right" && t == "outside") {
        return Some(Pos::End);
    }
    let n = t.strip_suffix("px").unwrap_or(&t).trim();
    n.parse::<f32>().ok().filter(|x| x.is_finite() && x.abs() < 1.0e6).map(Pos::Off)
}

fn fmt_pos(p: Pos, names: [&str; 3]) -> String {
    match p {
        Pos::Off(o) => num(o),
        Pos::Start => names[0].into(),
        Pos::Center => names[1].into(),
        Pos::End => names[2].into(),
    }
}

impl Anchor {
    /// Läser ankarattributen; `None` om inget av dem finns (då är bilden inte förankrad).
    pub fn from_attrs(get: &dyn Fn(&str) -> Option<String>) -> Option<Anchor> {
        let keys = ["x", "y", "hrel", "vrel", "wrap", "dist"];
        if keys.iter().all(|k| get(k).is_none()) {
            return None;
        }
        let mut a = Anchor::default();
        if let Some(v) = get("hrel") {
            a.hrel = match v.trim().to_ascii_lowercase().as_str() {
                "margin" => HRel::Margin,
                "page" => HRel::Page,
                _ => HRel::Column,
            };
        }
        if let Some(v) = get("vrel") {
            a.vrel = match v.trim().to_ascii_lowercase().as_str() {
                "margin" => VRel::Margin,
                "page" => VRel::Page,
                _ => VRel::Paragraph,
            };
        }
        if let Some(p) = get("x").and_then(|v| parse_pos(&v, ["left", "center", "right"])) {
            a.h = p;
        }
        if let Some(p) = get("y").and_then(|v| parse_pos(&v, ["top", "center", "bottom"])) {
            a.v = p;
        }
        if let Some(v) = get("wrap") {
            a.wrap = match v.trim().to_ascii_lowercase().as_str() {
                "top" | "topbottom" => Wrap::TopBottom,
                "behind" => Wrap::Behind,
                "front" | "none" => Wrap::Front,
                _ => Wrap::Square,
            };
        }
        if let Some(v) = get("dist") {
            let n: Vec<f32> = v.split_whitespace().filter_map(|s| s.parse::<f32>().ok()).filter(|x| x.is_finite() && *x >= 0.0).collect();
            if n.len() == 4 {
                a.dist = [n[0], n[1], n[2], n[3]];
            }
        }
        Some(a)
    }

    /// Attributen (med inledande blanksteg) som ger tillbaka samma `Anchor` vid läsning. `x` och
    /// `y` skrivs alltid så att en förankrad bild med standardvärden ändå är förankrad.
    pub fn write_attrs(&self, s: &mut String) {
        if self.hrel != HRel::Column {
            s.push_str(match self.hrel {
                HRel::Margin => " hrel=\"margin\"",
                _ => " hrel=\"page\"",
            });
        }
        s.push_str(&format!(" x=\"{}\"", fmt_pos(self.h, ["left", "center", "right"])));
        if self.vrel != VRel::Paragraph {
            s.push_str(match self.vrel {
                VRel::Margin => " vrel=\"margin\"",
                _ => " vrel=\"page\"",
            });
        }
        s.push_str(&format!(" y=\"{}\"", fmt_pos(self.v, ["top", "center", "bottom"])));
        match self.wrap {
            Wrap::Square => {}
            Wrap::TopBottom => s.push_str(" wrap=\"top\""),
            Wrap::Behind => s.push_str(" wrap=\"behind\""),
            Wrap::Front => s.push_str(" wrap=\"front\""),
        }
        if self.dist != DEFAULT_DIST {
            s.push_str(&format!(" dist=\"{} {} {} {}\"", num(self.dist[0]), num(self.dist[1]), num(self.dist[2]), num(self.dist[3])));
        }
    }

    /// Bildens sida för det äldre `align`-läget (vänster/höger om mitten, utan sidgeometri).
    pub fn side_hint(&self) -> super::markup::Float {
        match self.h {
            Pos::End => super::markup::Float::Right,
            Pos::Off(o) if o > 200.0 => super::markup::Float::Right,
            _ => super::markup::Float::Left,
        }
    }

    /// Kortformen `align="left|right"`: en bild längst ut i spalten, vid stycket, med normalt
    /// avstånd och omflöde (så som Kladd själv skriver flytande bilder).
    pub fn legacy_side(&self) -> Option<super::markup::Float> {
        use super::markup::Float;
        let plain = self.hrel == HRel::Column
            && self.vrel == VRel::Paragraph
            && matches!(self.v, Pos::Off(0.0) | Pos::Start)
            && self.wrap == Wrap::Square
            && self.dist[0] == 0.0
            && self.dist[2] == 0.0;
        let side = match self.h {
            Pos::Start => Float::Left,
            Pos::End => Float::Right,
            _ => return None,
        };
        let (dl, dr) = (self.dist[3], self.dist[1]);
        let same = |x: f32, y: f32| (x - y).abs() < 0.5;
        let ok = match side {
            Float::Left => same(dr, 12.0) && (same(dl, 0.0) || same(dl, 12.0)),
            Float::Right => same(dl, 12.0) && (same(dr, 0.0) || same(dr, 12.0)),
        };
        (plain && ok).then_some(side)
    }

    /// Referenskantens vänsterkant (från spaltens vänsterkant) som `Off` räknas från.
    pub fn h_ref_left(&self, c: &PageCtx) -> f32 {
        match self.hrel {
            HRel::Column => 0.0,
            HRel::Margin => c.margin[3] - c.col_left,
            HRel::Page => -c.col_left,
        }
    }

    /// Referenskantens överkant från arkets överkant; `None` när den räknas från stycket.
    pub fn v_ref_top(&self, c: &PageCtx) -> Option<f32> {
        match self.vrel {
            VRel::Paragraph => None,
            VRel::Margin => Some(c.margin[0]),
            VRel::Page => Some(0.0),
        }
    }

    /// Mått i CSS-px till zoomade px: förskjutningar och avstånd skalas.
    pub fn scaled(&self, z: f32) -> Anchor {
        let sc = |p: Pos| match p {
            Pos::Off(o) => Pos::Off(o * z),
            other => other,
        };
        Anchor { h: sc(self.h), v: sc(self.v), dist: self.dist.map(|d| d * z), ..*self }
    }

    /// Ändrar bara läget (förskjutningarna), så att flytt med musen behåller omflöde och avstånd.
    pub fn with_offsets(mut self, x: f32, y: f32) -> Anchor {
        self.h = Pos::Off(x);
        self.v = Pos::Off(y);
        self
    }
}

/// Sidgeometrin runt ett stycke som placeringen räknas mot (px, samma enhet som bilden).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageCtx {
    pub page_w: f32,
    pub page_h: f32,
    /// Marginaler: ovan, höger, under, vänster.
    pub margin: [f32; 4],
    /// Spaltens vänsterkant från arkets vänsterkant och spaltens bredd.
    pub col_left: f32,
    pub col_w: f32,
}

impl PageCtx {
    /// Utan sidor (textläge): spalten är hela ytan och marginalerna noll.
    pub fn column_only(col_w: f32) -> PageCtx {
        PageCtx { page_w: col_w, page_h: f32::INFINITY, margin: [0.0; 4], col_left: 0.0, col_w }
    }
}

/// Lodrätt läge: `Para(d)` = `d` under styckets överkant, `Page(y)` = `y` under arkets överkant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VAt {
    Para(f32),
    Page(f32),
}

impl Anchor {
    /// Bildens vänsterkant (bredd `w`) räknad från spaltens vänsterkant.
    pub fn x_in_column(&self, w: f32, c: &PageCtx) -> f32 {
        let (left, width) = match self.hrel {
            HRel::Column => (0.0, c.col_w),
            HRel::Margin => (c.margin[3] - c.col_left, c.page_w - c.margin[1] - c.margin[3]),
            HRel::Page => (-c.col_left, c.page_w),
        };
        match self.h {
            Pos::Off(o) => left + o,
            Pos::Start => left,
            Pos::Center => left + (width - w) / 2.0,
            Pos::End => left + width - w,
        }
    }

    /// Bildens överkant (höjd `h`).
    pub fn y_at(&self, h: f32, c: &PageCtx) -> VAt {
        let (top, height) = match self.vrel {
            VRel::Paragraph => {
                return VAt::Para(match self.v {
                    Pos::Off(o) => o,
                    _ => 0.0,
                });
            }
            VRel::Margin => (c.margin[0], c.page_h - c.margin[0] - c.margin[2]),
            VRel::Page => (0.0, c.page_h),
        };
        VAt::Page(match self.v {
            Pos::Off(o) => top + o,
            Pos::Start => top,
            Pos::Center => top + (height - h) / 2.0,
            Pos::End => top + height - h,
        })
    }
}

/// Hur en text-påverkande bild tar plats i en spalt av bredd `col_w`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reserve {
    /// Text bara till höger om bilden: bredd (från vänsterkanten) som tas.
    Left(f32),
    /// Text bara till vänster om bilden: bredd (från högerkanten) som tas.
    Right(f32),
    /// Hela raden tas (text över och under bilden).
    Full,
}

/// Var texten får plats bredvid en bild med vänsterkant `x` (från spaltens vänsterkant) och bredd
/// `w`. Text på båda sidor går inte att lägga ut i en galleri-rad, så texten läggs på den sida som
/// har mest plats (Words "bothSides" ser likadant ut när bilden sitter vid en kant).
pub fn reserve(a: &Anchor, x: f32, w: f32, col_w: f32) -> Reserve {
    match a.wrap {
        Wrap::TopBottom => return Reserve::Full,
        Wrap::Behind | Wrap::Front => return Reserve::Full,
        Wrap::Square => {}
    }
    let left_room = (x - a.dist[3]).max(0.0);
    let right_room = (col_w - (x + w + a.dist[1])).max(0.0);
    if right_room >= left_room {
        Reserve::Left((x + w + a.dist[1]).clamp(0.0, col_w))
    } else {
        Reserve::Right((col_w - (x - a.dist[3])).clamp(0.0, col_w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(kv: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| kv.iter().find(|(a, _)| *a == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn attrs_roundtrip() {
        let a = Anchor::from_attrs(&get(&[("hrel", "page"), ("x", "120.5"), ("vrel", "margin"), ("y", "bottom"), ("wrap", "behind"), ("dist", "1 2 3 4")])).unwrap();
        assert_eq!(a.hrel, HRel::Page);
        assert_eq!(a.h, Pos::Off(120.5));
        assert_eq!((a.vrel, a.v, a.wrap), (VRel::Margin, Pos::End, Wrap::Behind));
        assert_eq!(a.dist, [1.0, 2.0, 3.0, 4.0]);
        let mut s = String::new();
        a.write_attrs(&mut s);
        assert_eq!(s, " hrel=\"page\" x=\"120.5\" vrel=\"margin\" y=\"bottom\" wrap=\"behind\" dist=\"1 2 3 4\"");
        assert_eq!(Anchor::from_attrs(&get(&[])), None);
        let d = Anchor::from_attrs(&get(&[("x", "0")])).unwrap();
        assert_eq!(d, Anchor::default());
        let mut s = String::new();
        d.write_attrs(&mut s);
        assert_eq!(s, " x=\"0\" y=\"0\"");
    }

    #[test]
    fn geometry() {
        let c = PageCtx { page_w: 800.0, page_h: 1000.0, margin: [100.0, 100.0, 100.0, 100.0], col_left: 100.0, col_w: 600.0 };
        let page_left = Anchor { hrel: HRel::Page, h: Pos::Off(50.0), ..Anchor::default() };
        assert_eq!(page_left.x_in_column(200.0, &c), -50.0);
        let ctr = Anchor { hrel: HRel::Margin, h: Pos::Center, ..Anchor::default() };
        assert_eq!(ctr.x_in_column(200.0, &c), 200.0);
        let right = Anchor { h: Pos::End, ..Anchor::default() };
        assert_eq!(right.x_in_column(200.0, &c), 400.0);
        let bot = Anchor { vrel: VRel::Page, v: Pos::End, ..Anchor::default() };
        assert_eq!(bot.y_at(300.0, &c), VAt::Page(700.0));
        let m = Anchor { vrel: VRel::Margin, v: Pos::Off(10.0), ..Anchor::default() };
        assert_eq!(m.y_at(300.0, &c), VAt::Page(110.0));
        assert_eq!(Anchor::default().y_at(5.0, &c), VAt::Para(0.0));
    }

    #[test]
    fn reserve_sides() {
        let a = Anchor::default();
        assert_eq!(reserve(&a, 0.0, 100.0, 600.0), Reserve::Left(112.0));
        assert_eq!(reserve(&a, 500.0, 100.0, 600.0), Reserve::Right(112.0));
        // I mitten: texten på den bredare sidan.
        assert_eq!(reserve(&a, 150.0, 100.0, 600.0), Reserve::Left(262.0));
        assert_eq!(reserve(&Anchor { wrap: Wrap::TopBottom, ..a }, 0.0, 100.0, 600.0), Reserve::Full);
    }
}
