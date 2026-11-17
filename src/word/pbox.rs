//! Styckeutseende i dokumentläget: ramar (`w:pBdr`), skuggning (`w:shd`), exakt avstånd
//! (`w:spacing`) och indrag (`w:ind`), per stycke.
//!
//! Dialekten är en dold markör sist på styckets sista rad:
//!
//! ```text
//! Text i rutan<!--k:pp bdr="t=single:8:4:6E4BAA l=single:8:4:6E4BAA" shd="EDE7F6" sp="b=0 a=0"-->
//! ```
//!
//! * `bdr`: sidorna `t` `l` `b` `r` och `w` (mellan stycken i samma ruta), var och en
//!   `stil:sz:space:färg` (stil `single|double|dashed|dotted|thick`, `sz` i åttondels punkter,
//!   `space` i punkter, färg `RRGGBB` eller `auto`).
//! * `shd`: fyllnadsfärg `RRGGBB`.
//! * `sp`: `b=` före, `a=` efter (punkter), `l=` radavstånd (`1.15` = gånger enkelt, `exact:14`
//!   eller `least:14` i punkter), `c=1` = `contextualSpacing`.
//! * `ind`: `l=` vänster, `r=` höger, `f=` första raden (negativt = hängande), punkter.
//!
//! Stycken med lika ram och skuggning efter varandra bildar en enda ruta (som i Word). Avståndet
//! mellan två stycken blir största av "efter" och "före" (som Word och LibreOffice) plus ramens
//! luft; layouten lägger det på den tomma raden mellan styckena (`layout::paragraphs`).
//! Algoritmen för att slå ihop stycken till en ruta är densamma som Wordcrafts `layout`
//! (MIT/Apache-2.0): lika ramar och skuggning och inga andra stycken emellan.

use std::ops::Range;
use std::sync::Arc;

/// Markörens inledning.
pub const MARK: &str = "<!--k:pp ";

/// Standardavstånd som layouten utgår från (blanksteg mellan stycken): 8 pt efter, 0 före,
/// radavstånd 1,15.
pub const BASE_AFTER: f32 = 8.0;
pub const BASE_LINE: f32 = 1.15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineStyle {
    #[default]
    Single,
    Double,
    Dashed,
    Dotted,
    Thick,
    /// `nil`/`none`: ingen kant (släcker en kant som mallen anger).
    None,
}

impl LineStyle {
    pub fn name(self) -> &'static str {
        match self {
            LineStyle::Single => "single",
            LineStyle::Double => "double",
            LineStyle::Dashed => "dashed",
            LineStyle::Dotted => "dotted",
            LineStyle::Thick => "thick",
            LineStyle::None => "none",
        }
    }

    pub fn from_name(s: &str) -> Option<LineStyle> {
        Some(match s {
            "single" => LineStyle::Single,
            "double" => LineStyle::Double,
            "dashed" => LineStyle::Dashed,
            "dotted" => LineStyle::Dotted,
            "thick" => LineStyle::Thick,
            "none" => LineStyle::None,
            _ => return None,
        })
    }

    /// Words `w:val` (`ST_Border`) → stil; okända varianter blir närmaste.
    pub fn from_word(v: &str) -> LineStyle {
        match v {
            "nil" | "none" => LineStyle::None,
            "double" | "triple" | "thinThickSmallGap" | "thickThinSmallGap" | "thinThickThinSmallGap" | "thinThickMediumGap"
            | "thickThinMediumGap" | "thinThickThinMediumGap" | "thinThickLargeGap" | "thickThinLargeGap" | "thinThickThinLargeGap" => {
                LineStyle::Double
            }
            "dashed" | "dashSmallGap" | "dotDash" | "dotDotDash" | "dashDotStroked" => LineStyle::Dashed,
            "dotted" => LineStyle::Dotted,
            "thick" | "single-thick" => LineStyle::Thick,
            _ => LineStyle::Single,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            LineStyle::None => "nil",
            s => s.name(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub style: LineStyle,
    /// Linjetjocklek i åttondels punkter (`w:sz`).
    pub sz: u16,
    /// Luft mellan text och kant, punkter (`w:space`).
    pub space: u16,
    /// `None` = automatisk (textfärgen).
    pub color: Option<[u8; 3]>,
}

impl Border {
    /// Linjens tjocklek (punkter) så som den ritas: minst en halv punkt, `double` tre gånger
    /// (två linjer och glapp), `thick` minst 1,5 pt.
    pub fn width_pt(&self) -> f32 {
        let w = (f32::from(self.sz) / 8.0).max(0.5);
        match self.style {
            LineStyle::Double => w * 3.0,
            LineStyle::Thick => w.max(1.5),
            _ => w,
        }
    }

    /// Hur långt kanten tar plats utanför texten: luft plus linje (punkter).
    pub fn pad_pt(&self) -> f32 {
        f32::from(self.space) + self.width_pt()
    }

    fn encode(&self) -> String {
        let c = match self.color {
            Some(c) => format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2]),
            None => "auto".to_owned(),
        };
        format!("{}:{}:{}:{c}", self.style.name(), self.sz, self.space)
    }

    fn decode(s: &str) -> Option<Border> {
        let mut it = s.split(':');
        let style = LineStyle::from_name(it.next()?)?;
        let sz = it.next()?.parse().ok()?;
        let space = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let color = it.next().and_then(parse_hex);
        Some(Border { style, sz, space, color })
    }
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn hex(c: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Borders {
    pub top: Option<Border>,
    pub left: Option<Border>,
    pub bottom: Option<Border>,
    pub right: Option<Border>,
    /// Mellan två stycken i samma ruta (`w:between`).
    pub between: Option<Border>,
}

impl Borders {
    fn sides(&self) -> [(&'static str, Option<Border>); 5] {
        [("t", self.top), ("l", self.left), ("b", self.bottom), ("r", self.right), ("w", self.between)]
    }

    fn slots(&mut self) -> [&mut Option<Border>; 5] {
        [&mut self.top, &mut self.left, &mut self.bottom, &mut self.right, &mut self.between]
    }

    /// Kanter som är släckta (`nil`) räknas bort.
    fn cleaned(mut self) -> Borders {
        for s in self.slots() {
            if s.is_some_and(|b| b.style == LineStyle::None) {
                *s = None;
            }
        }
        self
    }

    pub fn any(&self) -> bool {
        self.top.is_some() || self.left.is_some() || self.bottom.is_some() || self.right.is_some()
    }

    fn over(&self, base: &Borders) -> Borders {
        let mut out = *base;
        for (o, n) in out.slots().into_iter().zip([self.top, self.left, self.bottom, self.right, self.between]) {
            if n.is_some() {
                *o = n;
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineRule {
    /// Gånger enkelt radavstånd.
    Auto(f32),
    Exact(f32),
    AtLeast(f32),
}

/// Vågrätt läge för en ram med egen bredd (textruta).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameX {
    /// Avstånd från spaltens vänsterkant (punkter).
    Off(f32),
    Start,
    Center,
    End,
}

/// En ram med egen bredd och eget läge i spalten (textruta): stycket och dess grannar i rutan
/// får indrag så att texten hamnar i rutan. Längder i punkter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Rutans ytterbredd.
    pub w: f32,
    pub x: FrameX,
    /// Luft mellan rutans kant och texten (vänster, höger), inklusive kantlinjen.
    pub pl: f32,
    pub pr: f32,
}

impl Frame {
    /// Rutans vänsterkant och ytterbredd i en spalt `avail` punkter bred.
    pub fn span(&self, avail: f32) -> (f32, f32) {
        let w = self.w.clamp(1.0, avail.max(1.0));
        let l = match self.x {
            FrameX::Off(v) => v,
            FrameX::Start => 0.0,
            FrameX::Center => (avail - w) / 2.0,
            FrameX::End => avail - w,
        };
        (l.clamp(0.0, (avail - w).max(0.0)), w)
    }
}

/// Styckeegenskaper (allt valfritt: bara det som anges gäller). Längder i punkter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PPr {
    pub borders: Borders,
    pub shd: Option<[u8; 3]>,
    pub before: Option<f32>,
    pub after: Option<f32>,
    pub line: Option<LineRule>,
    pub ctx: Option<bool>,
    pub left: Option<f32>,
    pub right: Option<f32>,
    /// Första radens indrag; negativt = hängande.
    pub first: Option<f32>,
    /// Ram med egen bredd och eget läge (textruta); går före `left`/`right`.
    pub frame: Option<Frame>,
}

fn num(v: f32) -> String {
    let s = format!("{:.2}", (v * 100.0).round() / 100.0);
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl PPr {
    /// `self` ovanpå `base` (egenskaper som `self` anger går före).
    pub fn over(&self, base: &PPr) -> PPr {
        PPr {
            borders: self.borders.over(&base.borders),
            shd: self.shd.or(base.shd),
            before: self.before.or(base.before),
            after: self.after.or(base.after),
            line: self.line.or(base.line),
            ctx: self.ctx.or(base.ctx),
            left: self.left.or(base.left),
            right: self.right.or(base.right),
            first: self.first.or(base.first),
            frame: self.frame.or(base.frame),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == PPr::default()
    }

    /// Ritas stycket i en ruta (kant eller skuggning)?
    pub fn has_box(&self) -> bool {
        self.borders.any() || self.shd.is_some()
    }

    /// Ger samma ruta som `o` (Word slår ihop stycken med lika ramar och indrag).
    pub fn same_box(&self, o: &PPr) -> bool {
        self.has_box()
            && self.borders == o.borders
            && self.shd == o.shd
            && self.left.unwrap_or(0.0) == o.left.unwrap_or(0.0)
            && self.right.unwrap_or(0.0) == o.right.unwrap_or(0.0)
            && self.frame == o.frame
    }

    /// Textens indrag (vänster, höger) och rutans vänsterkant och bredd i en spalt `avail` punkter bred,
    /// om stycket ligger i en ram med egen bredd.
    pub fn frame_geom(&self, avail: f32) -> Option<(f32, f32, f32, f32)> {
        let f = self.frame?;
        let (l, w) = f.span(avail);
        Some((l + f.pl, (avail - l - w).max(0.0) + f.pr, l, w))
    }

    pub fn has_spacing(&self) -> bool {
        self.before.is_some() || self.after.is_some() || self.line.is_some() || self.ctx.is_some()
    }

    pub fn has_indent(&self) -> bool {
        self.left.is_some() || self.right.is_some() || self.first.is_some()
    }

    /// Luft ovanför första stycket i en ruta (kantens luft och linje), punkter.
    pub fn pad_top(&self) -> f32 {
        self.borders.top.map_or(0.0, |b| b.pad_pt())
    }

    pub fn pad_bottom(&self) -> f32 {
        self.borders.bottom.map_or(0.0, |b| b.pad_pt())
    }

    /// Hur långt rutan når utanför textens vänster- respektive högerkant, punkter.
    pub fn pad_left(&self) -> f32 {
        self.borders.left.map_or(0.0, |b| b.pad_pt())
    }

    pub fn pad_right(&self) -> f32 {
        self.borders.right.map_or(0.0, |b| b.pad_pt())
    }

    /// Markören `<!--k:pp …-->`; tom text för ett tomt `PPr`.
    pub fn marker(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut s = String::from(MARK.trim_end());
        let bdr: Vec<String> = self.borders.sides().iter().filter_map(|(k, b)| b.map(|b| format!("{k}={}", b.encode()))).collect();
        if !bdr.is_empty() {
            s.push_str(&format!(" bdr=\"{}\"", bdr.join(" ")));
        }
        if let Some(c) = self.shd {
            s.push_str(&format!(" shd=\"{}\"", hex(c)));
        }
        let mut sp: Vec<String> = Vec::new();
        if let Some(v) = self.before {
            sp.push(format!("b={}", num(v)));
        }
        if let Some(v) = self.after {
            sp.push(format!("a={}", num(v)));
        }
        match self.line {
            Some(LineRule::Auto(m)) => sp.push(format!("l={}", num(m))),
            Some(LineRule::Exact(p)) => sp.push(format!("l=exact:{}", num(p))),
            Some(LineRule::AtLeast(p)) => sp.push(format!("l=least:{}", num(p))),
            None => {}
        }
        if self.ctx == Some(true) {
            sp.push("c=1".into());
        }
        if !sp.is_empty() {
            s.push_str(&format!(" sp=\"{}\"", sp.join(" ")));
        }
        let mut ind: Vec<String> = Vec::new();
        for (k, v) in [("l", self.left), ("r", self.right), ("f", self.first)] {
            if let Some(v) = v {
                ind.push(format!("{k}={}", num(v)));
            }
        }
        if !ind.is_empty() {
            s.push_str(&format!(" ind=\"{}\"", ind.join(" ")));
        }
        if let Some(f) = self.frame {
            let x = match f.x {
                FrameX::Off(v) => num(v),
                FrameX::Start => "start".into(),
                FrameX::Center => "center".into(),
                FrameX::End => "end".into(),
            };
            s.push_str(&format!(" fr=\"w={} x={x} pl={} pr={}\"", num(f.w), num(f.pl), num(f.pr)));
        }
        s.push_str("-->");
        s
    }

    /// Läser en markör (hela taggen). `None` om texten inte är en styckemarkör.
    pub fn parse_marker(tag: &str) -> Option<PPr> {
        let body = tag.trim().strip_prefix("<!--k:pp")?.strip_suffix("-->")?;
        if !body.is_empty() && !body.starts_with(' ') {
            return None;
        }
        let mut pp = PPr::default();
        for (k, v) in attrs(body) {
            match k.as_str() {
                "bdr" => {
                    for part in v.split_whitespace() {
                        let Some((side, b)) = part.split_once('=') else { continue };
                        let b = Border::decode(b);
                        match side {
                            "t" => pp.borders.top = b,
                            "l" => pp.borders.left = b,
                            "b" => pp.borders.bottom = b,
                            "r" => pp.borders.right = b,
                            "w" => pp.borders.between = b,
                            _ => {}
                        }
                    }
                }
                "shd" => pp.shd = parse_hex(&v),
                "sp" => {
                    for part in v.split_whitespace() {
                        let Some((k, x)) = part.split_once('=') else { continue };
                        match k {
                            "b" => pp.before = x.parse().ok(),
                            "a" => pp.after = x.parse().ok(),
                            "c" => pp.ctx = Some(x == "1"),
                            "l" => {
                                pp.line = if let Some(p) = x.strip_prefix("exact:") {
                                    p.parse().ok().map(LineRule::Exact)
                                } else if let Some(p) = x.strip_prefix("least:") {
                                    p.parse().ok().map(LineRule::AtLeast)
                                } else {
                                    x.parse().ok().map(LineRule::Auto)
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "ind" => {
                    for part in v.split_whitespace() {
                        let Some((k, x)) = part.split_once('=') else { continue };
                        let x = x.parse().ok();
                        match k {
                            "l" => pp.left = x,
                            "r" => pp.right = x,
                            "f" => pp.first = x,
                            _ => {}
                        }
                    }
                }
                "fr" => {
                    let (mut w, mut x, mut pl, mut pr) = (None, FrameX::Start, 0.0, 0.0);
                    for part in v.split_whitespace() {
                        let Some((k, v)) = part.split_once('=') else { continue };
                        match k {
                            "w" => w = v.parse::<f32>().ok(),
                            "pl" => pl = v.parse().unwrap_or(0.0),
                            "pr" => pr = v.parse().unwrap_or(0.0),
                            "x" => {
                                x = match v {
                                    "start" => FrameX::Start,
                                    "center" => FrameX::Center,
                                    "end" => FrameX::End,
                                    o => FrameX::Off(o.parse().unwrap_or(0.0)),
                                }
                            }
                            _ => {}
                        }
                    }
                    pp.frame = w.filter(|w| *w > 0.0 && w.is_finite()).map(|w| Frame { w, x, pl, pr });
                }
                _ => {}
            }
        }
        Some(pp)
    }

    /// Efter sammanslagning: släckta kanter bort.
    pub fn finish(mut self) -> PPr {
        self.borders = self.borders.cleaned();
        self
    }
}

/// `nyckel="värde"`-par i en markör.
fn attrs(s: &str) -> Vec<(String, String)> {
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

/// Ett tomt stycke (`<!--k:ep sz="11" sp="b=0 a=8"-->`): en egen rad som layouten ger ett styckes
/// höjd (styckemärkets teckenstorlek `sz` i punkter, radavstånd och avstånd som `pp`). Samma
/// attribut som `k:pp`.
pub const EMPTY: &str = "<!--k:ep";

pub fn empty_marker(pp: &PPr, sz: Option<f32>) -> String {
    let inner = pp.marker();
    let mut s = if inner.is_empty() { String::from("<!--k:ep-->") } else { inner.replacen("<!--k:pp", EMPTY, 1) };
    if let Some(sz) = sz {
        s = s.replacen(EMPTY, &format!("{EMPTY} sz=\"{}\"", num(sz)), 1);
    }
    s
}

/// Läser en tomt-stycke-markör: egenskaper och teckenstorlek.
pub fn parse_empty(tag: &str) -> Option<(PPr, Option<f32>)> {
    let t = tag.trim();
    let rest = t.strip_prefix(EMPTY)?;
    if !rest.starts_with(' ') && rest != "-->" {
        return None;
    }
    let pp = PPr::parse_marker(&format!("<!--k:pp{rest}"))?;
    let sz = attrs(rest.trim_end_matches("-->")).into_iter().find(|(k, _)| k == "sz").and_then(|(_, v)| v.parse().ok());
    Some((pp, sz))
}

/// Är `t` (en hel tagg) en styckemarkör?
pub fn is_marker(t: &str) -> bool {
    PPr::parse_marker(t).is_some()
}

/// Ett stycke med markör: byteintervallet för styckets rader (utan avslutande `\n`).
#[derive(Clone, Debug)]
pub struct ParaSpec {
    pub range: Range<usize>,
    pub pp: Arc<PPr>,
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// Börjar raden ett nytt stycke eller listobjekt (så att den inte fortsätter förra raden)?
fn starts_block(line: &str) -> bool {
    let t = line.trim_start();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("> ")
        || t.starts_with('#')
        || t.starts_with('|')
        || t.starts_with("```")
        || (digits > 0 && t[digits..].starts_with(". "))
}

/// Alla stycken med markör, i textens ordning.
pub fn scan(text: &str) -> Vec<ParaSpec> {
    let mut out: Vec<ParaSpec> = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(MARK) {
        let at = from + i;
        let Some(len) = text[at..].find("-->") else { break };
        let end = at + len + 3;
        from = end;
        let le = text[end..].find('\n').map_or(text.len(), |j| end + j);
        // Markören ska stå sist på raden.
        if !text[end..le].trim().is_empty() {
            continue;
        }
        let Some(pp) = PPr::parse_marker(&text[at..end]) else { continue };
        let mut ls = line_start(text, at);
        // Styckets tidigare rader (tills en tom rad, ett annat stycke eller en ny post).
        loop {
            let this_end = text[ls..].find('\n').map_or(text.len(), |j| ls + j);
            if ls == 0 || starts_block(&text[ls..this_end]) {
                break;
            }
            let ps = line_start(text, ls - 1);
            let prev = &text[ps..ls - 1];
            if prev.trim().is_empty() || prev.contains(MARK) {
                break;
            }
            ls = ps;
        }
        out.push(ParaSpec { range: ls..le, pp: Arc::new(pp) });
    }
    out
}

/// En ruta: efterföljande stycken med samma ram och skuggning (bara tomma rader emellan).
#[derive(Clone, Debug)]
pub struct BoxRun {
    /// Index i specifikationslistan.
    pub specs: Range<usize>,
    /// Från första styckets början till sista styckets slut.
    pub range: Range<usize>,
}

/// Slår ihop stycken till rutor. Stycken utan kant eller skuggning ger ingen ruta.
pub fn box_runs(text: &str, specs: &[ParaSpec]) -> Vec<BoxRun> {
    let mut out: Vec<BoxRun> = Vec::new();
    for (i, s) in specs.iter().enumerate() {
        if !s.pp.has_box() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            let prev = &specs[last.specs.end - 1];
            if last.specs.end == i && prev.pp.same_box(&s.pp) && text[prev.range.end..s.range.start].trim().is_empty() {
                last.specs.end = i + 1;
                last.range.end = s.range.end;
                continue;
            }
        }
        out.push(BoxRun { specs: i..i + 1, range: s.range.clone() });
    }
    out
}

/// Avståndet (punkter) på den tomma raden mellan stycke `a` och `b`: största av efter och före
/// plus rutans luft, eller mellan två stycken i samma ruta bara avståndet (och mellanlinjen).
/// `a_after`/`b_before` = värden när stycket saknar eget avstånd.
pub fn gap_pt(a: Option<&PPr>, b: Option<&PPr>, a_after: f32, b_before: f32) -> f32 {
    let after = a.and_then(|p| p.after).unwrap_or(a_after);
    let before = b.and_then(|p| p.before).unwrap_or(b_before);
    let ctx = a.is_some_and(|p| p.ctx == Some(true)) && b.is_some_and(|p| p.ctx == Some(true));
    let mut gap = if ctx { 0.0 } else { after.max(before) };
    match (a, b) {
        (Some(a), Some(b)) if a.same_box(b) => {
            if let Some(w) = a.borders.between {
                gap = gap.max(f32::from(w.space) * 2.0 + w.width_pt());
            }
        }
        _ => {
            gap += a.map_or(0.0, |p| if p.has_box() { p.pad_bottom() } else { 0.0 });
            gap += b.map_or(0.0, |p| if p.has_box() { p.pad_top() } else { 0.0 });
        }
    }
    gap.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bd(c: u8) -> Border {
        Border { style: LineStyle::Single, sz: 8, space: 4, color: Some([c, 0x4B, 0xAA]) }
    }

    #[test]
    fn marker_round_trip() {
        let pp = PPr {
            borders: Borders { top: Some(bd(0x6E)), left: Some(Border { style: LineStyle::Double, sz: 24, space: 8, color: None }), ..Default::default() },
            shd: Some([0xED, 0xE7, 0xF6]),
            before: Some(0.0),
            after: Some(6.5),
            line: Some(LineRule::Exact(14.0)),
            ctx: Some(true),
            left: Some(36.0),
            right: None,
            first: Some(-18.0),
            frame: Some(Frame { w: 200.0, x: FrameX::Off(12.5), pl: 8.0, pr: 8.0 }),
        };
        let m = pp.marker();
        assert!(m.starts_with("<!--k:pp ") && m.ends_with("-->") && !m.contains("--->") && !m[4..m.len() - 3].contains("--"), "{m}");
        assert_eq!(PPr::parse_marker(&m), Some(pp));
        assert_eq!(PPr::default().marker(), "");
        assert_eq!(PPr::parse_marker("<!--k:12-->"), None);
        assert_eq!(PPr::parse_marker("<!--k:pm ins 5-->"), None);
    }

    #[test]
    fn scan_finds_paragraphs_and_boxes() {
        let b = PPr { borders: Borders { top: Some(bd(1)), ..Default::default() }, shd: Some([1, 2, 3]), after: Some(0.0), ..Default::default() };
        let m = b.marker();
        let other = PPr { shd: Some([9, 9, 9]), ..Default::default() }.marker();
        let t = format!("Rubrik\n\nett{m}\n\ntvå{m}\n\ntre{other}\n\nfyra{m}\n");
        let specs = scan(&t);
        assert_eq!(specs.len(), 4);
        assert_eq!(&t[specs[0].range.clone()], format!("ett{m}"));
        let runs = box_runs(&t, &specs);
        assert_eq!(runs.len(), 3, "{runs:?}");
        assert_eq!(runs[0].specs, 0..2);
        assert_eq!(&t[runs[0].range.clone()], format!("ett{m}\n\ntvå{m}"));
    }

    #[test]
    fn unrelated_paragraph_splits_a_box() {
        let m = PPr { shd: Some([1, 2, 3]), ..Default::default() }.marker();
        let t = format!("a{m}\n\nmellan\n\nb{m}\n");
        assert_eq!(box_runs(&t, &scan(&t)).len(), 2);
    }

    #[test]
    fn marker_must_end_the_line() {
        let m = PPr { shd: Some([1, 2, 3]), ..Default::default() }.marker();
        assert!(scan(&format!("a{m} mer\n")).is_empty());
        assert_eq!(scan(&format!("a{m}")).len(), 1);
    }

    #[test]
    fn multi_line_paragraph_range() {
        let m = PPr { shd: Some([1, 2, 3]), ..Default::default() }.marker();
        let t = format!("före\n\nförsta raden\nandra raden{m}\n");
        let s = scan(&t);
        assert_eq!(&t[s[0].range.clone()], format!("första raden\nandra raden{m}"));
        let l = format!("- ett\n- två{m}\n");
        assert_eq!(&l[scan(&l)[0].range.clone()], format!("- två{m}"));
    }

    #[test]
    fn gaps() {
        let tight = PPr { after: Some(0.0), shd: Some([1, 1, 1]), ..Default::default() };
        // samma ruta, inget avstånd
        assert_eq!(gap_pt(Some(&tight), Some(&tight), 8.0, 0.0), 0.0);
        // största av efter och före
        let a = PPr { after: Some(6.0), ..Default::default() };
        let b = PPr { before: Some(10.0), ..Default::default() };
        assert_eq!(gap_pt(Some(&a), Some(&b), 8.0, 0.0), 10.0);
        // ram: luft under och över, utanför avståndet
        let boxed = PPr { borders: Borders { top: Some(bd(1)), bottom: Some(bd(1)), ..Default::default() }, after: Some(0.0), ..Default::default() };
        let w = bd(1).pad_pt();
        assert_eq!(gap_pt(Some(&boxed), None, 8.0, 0.0), w);
        assert_eq!(gap_pt(None, Some(&boxed), 8.0, 0.0), 8.0 + w);
        // contextualSpacing
        let c = PPr { ctx: Some(true), after: Some(8.0), ..Default::default() };
        assert_eq!(gap_pt(Some(&c), Some(&c), 8.0, 0.0), 0.0);
        // mellanlinje
        let w = PPr { shd: Some([1, 1, 1]), after: Some(0.0), borders: Borders { between: Some(bd(1)), left: Some(bd(1)), ..Default::default() }, ..Default::default() };
        assert!(gap_pt(Some(&w), Some(&w), 8.0, 0.0) > 8.0);
    }

    #[test]
    fn word_border_names() {
        assert_eq!(LineStyle::from_word("single"), LineStyle::Single);
        assert_eq!(LineStyle::from_word("thinThickSmallGap"), LineStyle::Double);
        assert_eq!(LineStyle::from_word("dotDash"), LineStyle::Dashed);
        assert_eq!(LineStyle::from_word("nil"), LineStyle::None);
    }
}
