//! Animeringar (`p:timing`): tidsträdet tolkas till klickgrupper med effekter, och formernas tillstånd
//! (synlig, opacitet, förskjutning, skala, vridning, avslöjning) räknas ut för en given tidpunkt.
//! Rent och provat här; bildspelet (`presenter.rs`) ritar formerna som egna lager med tillståndet.
//!
//! Huvudsekvensen (`nodeType="mainSeq"`) har en `p:par` per klick. En klickgrupp som startar av sig
//! själv (`evt="onBegin"`, "med föregående"/"efter föregående" först på bilden) körs när bilden visas
//! eller när gruppen före är klar. I gruppen ligger tidsförskjutna `p:par` (efter föregående: start =
//! max(skriven fördröjning, förra delens slut)) med effekterna (`presetClass`, `presetID`, `nodeType`),
//! vars beteenden tolkas generellt: `p:set` (synlighet), `p:animEffect` (toning och avslöjningsformer),
//! `p:anim` (`ppt_x`/`ppt_y`/`ppt_w`/`ppt_h`/rotation/opacitet med formler, `anim_expr.rs`),
//! `p:animScale`, `p:animRot`, `p:animMotion` (bana) och `p:animClr` (färgbyte ≈ blink). Okända
//! beteenden gör inget, så formen visas eller göms ändå i rätt ordning av sina `p:set`.
//! Interaktiva sekvenser (utlösare, `interactiveSeq`) hoppas över: de formerna visas som vanligt.

use super::anim_expr::{self, Vars};
use super::transition::{Dir, Reveal};
use super::xml::Node;

/// Det en effekt verkar på: en form (`p:cNvPr/@id`) eller ett stycke i dess text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Target {
    pub shape: u32,
    pub para: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Entrance,
    Exit,
    Emphasis,
    Path,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attr {
    X,
    Y,
    W,
    H,
    Rot,
    Opacity,
}

#[derive(Clone, Debug, PartialEq)]
enum Beh {
    Visible(bool),
    /// `p:animEffect`: avslöjning (`None` = toning) in eller ut.
    Filter { reveal: Option<Reveal>, entering: bool },
    Prop { attr: Attr, keys: Vec<(f32, String)>, discrete: bool, by: Option<String>, from: Option<String>, to: Option<String> },
    Scale { from: (f32, f32), to: (f32, f32) },
    Rotate { from: f32, to: f32 },
    /// Banan i andelar av bilden (relativt formens läge).
    Motion { pts: Vec<(f32, f32)> },
    Blink,
}

#[derive(Clone, Debug, PartialEq)]
struct Behavior {
    beh: Beh,
    /// Start relativt effekten (s).
    begin: f32,
    dur: f32,
    accel: f32,
    decel: f32,
    auto_rev: bool,
    repeat: f32,
}

impl Behavior {
    fn total(&self) -> f32 {
        self.dur * if self.auto_rev { 2.0 } else { 1.0 } * self.repeat
    }

    /// Förloppet 0..1 vid `local` sekunder efter effektens start (`None` = inte börjat).
    fn progress(&self, local: f32) -> Option<f32> {
        let t = local - self.begin;
        if t < 0.0 {
            return None;
        }
        if self.dur <= 0.0 {
            return Some(1.0);
        }
        if t >= self.total() {
            return Some(if self.auto_rev { 0.0 } else { 1.0 });
        }
        let cycle = self.dur * if self.auto_rev { 2.0 } else { 1.0 };
        let mut p = (t % cycle) / self.dur;
        if p > 1.0 {
            p = 2.0 - p;
        }
        Some(accel_decel(p, self.accel, self.decel))
    }
}

/// SMIL:s accel/decel (andelar av tiden med acceleration i början och inbromsning i slutet).
fn accel_decel(t: f32, a: f32, d: f32) -> f32 {
    let (a, d) = (a.clamp(0.0, 1.0), d.clamp(0.0, 1.0));
    if a + d <= 0.0 || a + d > 1.0 {
        return t;
    }
    let r = 1.0 / (1.0 - a / 2.0 - d / 2.0);
    if t < a {
        r * t * t / (2.0 * a)
    } else if t <= 1.0 - d {
        r * (t - a / 2.0)
    } else {
        let tt = 1.0 - t;
        r * (1.0 - a / 2.0 - d / 2.0 - tt * tt / (2.0 * d))
    }
}

/// En effekt (en `p:par` med `presetClass`).
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub target: Target,
    pub class: Class,
    pub preset: i64,
    /// Start i klickgruppen (s).
    pub begin: f32,
    /// Längd (s).
    pub dur: f32,
    hold: bool,
    behaviors: Vec<Behavior>,
}

/// En klickgrupp.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Startar av sig själv (när bilden visas eller när gruppen före är klar).
    pub auto: bool,
    pub effects: Vec<Effect>,
    pub dur: f32,
}

/// En bilds animeringar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    pub groups: Vec<Group>,
}

/// Formens läge och storlek i andelar av bilden (för formlerna).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

/// Ett lagers tillstånd vid en tidpunkt. Förskjutningen i andelar av bilden; skala och vridning runt
/// lagrets mitt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub visible: bool,
    pub opacity: f32,
    pub dx: f32,
    pub dy: f32,
    pub sx: f32,
    pub sy: f32,
    pub rot: f32,
    /// Bara den här delen syns (avslöjningsform och förlopp 0..1).
    pub reveal: Option<(Reveal, f32)>,
}

impl State {
    pub const SHOWN: State = State { visible: true, opacity: 1.0, dx: 0.0, dy: 0.0, sx: 1.0, sy: 1.0, rot: 0.0, reveal: None };

    /// Formens tillstånd ovanpå styckets (stycket ärver formens synlighet och rörelse).
    pub fn compose(&self, inner: &State) -> State {
        State {
            visible: self.visible && inner.visible,
            opacity: self.opacity * inner.opacity,
            dx: self.dx + inner.dx,
            dy: self.dy + inner.dy,
            sx: self.sx * inner.sx,
            sy: self.sy * inner.sy,
            rot: self.rot + inner.rot,
            reveal: inner.reveal.or(self.reveal),
        }
    }

    /// Syns något alls?
    pub fn drawn(&self) -> bool {
        self.visible && self.opacity > 0.001 && self.reveal.is_none_or(|(_, p)| p > 0.0) && self.sx.abs() > 1e-4 && self.sy.abs() > 1e-4
    }
}

impl Timeline {
    /// Bildens tidsträd (`p:sld/p:timing`); utan animeringar en tom tidslinje.
    pub fn of_slide(root: &Node) -> Timeline {
        let Some(timing) = root.child("p:timing") else { return Timeline::default() };
        let Some(main) = find_main_seq(timing) else { return Timeline::default() };
        let mut groups = Vec::new();
        for click in main.child("p:childTnLst").into_iter().flat_map(|l| l.children_named("p:par")) {
            let Some(ctn) = click.child("p:cTn") else { continue };
            let auto = is_auto(ctn);
            let mut effects = Vec::new();
            let mut prev_end = 0.0f32;
            for off in ctn.child("p:childTnLst").into_iter().flat_map(|l| l.children_named("p:par")) {
                let Some(oc) = off.child("p:cTn") else { continue };
                let written = delay_of(oc);
                let first_after = oc
                    .child("p:childTnLst")
                    .and_then(|l| l.children_named("p:par").next())
                    .and_then(|p| p.child("p:cTn"))
                    .is_some_and(|c| c.attr("nodeType") == Some("afterEffect"));
                let start = if first_after { written.max(prev_end) } else { written };
                let mut end = start;
                for ep in oc.child("p:childTnLst").into_iter().flat_map(|l| l.children_named("p:par")) {
                    let Some(ec) = ep.child("p:cTn") else { continue };
                    for e in parse_effect(ec, start + delay_of(ec)) {
                        end = end.max(e.begin + e.dur);
                        effects.push(e);
                    }
                }
                prev_end = end;
            }
            let dur = effects.iter().map(|e| e.begin + e.dur).fold(0.0f32, f32::max);
            groups.push(Group { auto, effects, dur });
        }
        Timeline { groups }
    }

    /// Antal klick på bilden.
    pub fn clicks(&self) -> usize {
        self.groups.iter().filter(|g| !g.auto).count()
    }

    /// Antal grupper som är klara i vila efter `step` klick (självstartande grupper efter ett klick
    /// räknas med).
    pub fn groups_at_step(&self, step: usize) -> usize {
        let mut seen = 0;
        let mut i = 0;
        while i < self.groups.len() {
            if !self.groups[i].auto {
                if seen == step {
                    break;
                }
                seen += 1;
            }
            i += 1;
        }
        i
    }

    /// Grupperna som spelas när man kommer till `step` (för steg 0 de självstartande i början).
    pub fn chain(&self, step: usize) -> std::ops::Range<usize> {
        let from = if step == 0 { 0 } else { self.groups_at_step(step - 1) };
        from..self.groups_at_step(step).max(from)
    }

    /// Längden på kedjan för `step` (s).
    pub fn chain_dur(&self, step: usize) -> f32 {
        self.chain(step).map(|g| self.groups[g].dur).sum()
    }

    /// Alla mål, sorterade.
    pub fn targets(&self) -> Vec<Target> {
        let mut v: Vec<Target> = self.groups.iter().flat_map(|g| g.effects.iter().map(|e| e.target)).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Är målet dolt innan något har hänt (första effekten som visar eller döljer det är en in-effekt)?
    pub fn initially_hidden(&self, t: Target) -> bool {
        for g in &self.groups {
            let mut es: Vec<&Effect> = g.effects.iter().filter(|e| e.target == t).collect();
            es.sort_by(|a, b| a.begin.total_cmp(&b.begin));
            for e in es {
                match e.class {
                    Class::Entrance => return true,
                    Class::Exit => return false,
                    _ => {
                        // Okänd klass: avgörs av första synlighetsbeteendet.
                        for b in &e.behaviors {
                            match b.beh {
                                Beh::Visible(v) | Beh::Filter { entering: v, .. } => return v,
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Målets tillstånd när grupperna före `chain.start` är klara och grupperna i `chain` har spelats
    /// i `t` sekunder (efter varandra); grupperna efter har inte börjat. `t = f32::INFINITY` ger vilan.
    pub fn state(&self, target: Target, geom: Geom, chain: std::ops::Range<usize>, t: f32) -> State {
        let mut st = State::SHOWN;
        st.visible = !self.initially_hidden(target);
        let mut offset = 0.0f32;
        for (gi, g) in self.groups.iter().enumerate() {
            if gi >= chain.end.max(chain.start) {
                break;
            }
            let time = if gi < chain.start {
                f32::INFINITY
            } else {
                let tt = t - offset;
                offset += g.dur;
                tt
            };
            let mut es: Vec<&Effect> = g.effects.iter().filter(|e| e.target == target).collect();
            es.sort_by(|a, b| a.begin.total_cmp(&b.begin));
            for e in es {
                let local = time - e.begin;
                if local < 0.0 {
                    continue;
                }
                apply_effect(&mut st, e, local, geom);
            }
        }
        st
    }
}

fn apply_effect(st: &mut State, e: &Effect, local: f32, geom: Geom) {
    let done = local >= e.dur;
    if done && !e.hold {
        // Effekten tas bort när den är klar; ut-effekter döljer ändå.
        if e.class == Class::Exit {
            st.visible = false;
        }
        return;
    }
    let vars = |p: f32| Vars { x: geom.cx, y: geom.cy, w: geom.w, h: geom.h, t: p };
    for b in &e.behaviors {
        let Some(p) = b.progress(local) else { continue };
        match &b.beh {
            Beh::Visible(v) => st.visible = *v,
            Beh::Filter { reveal, entering } => {
                let shown = if *entering { p } else { 1.0 - p };
                if *entering {
                    st.visible = true;
                }
                if p >= 1.0 {
                    if !*entering {
                        st.visible = false;
                    }
                } else {
                    match reveal {
                        Some(r) => st.reveal = Some((*r, shown)),
                        None => st.opacity *= shown,
                    }
                }
            }
            Beh::Prop { attr, keys, discrete, by, from, to } => {
                let v = vars(p);
                let base = match attr {
                    Attr::X => geom.cx,
                    Attr::Y => geom.cy,
                    Attr::W => geom.w,
                    Attr::H => geom.h,
                    Attr::Rot => 0.0,
                    Attr::Opacity => 1.0,
                };
                let value = if !keys.is_empty() {
                    key_value(keys, *discrete, p, &v)
                } else if let Some(by) = by.as_deref().and_then(|s| anim_expr::eval(s, &v)) {
                    let f = from.as_deref().and_then(|s| anim_expr::eval(s, &v)).unwrap_or(base);
                    Some(f + by * p)
                } else if let Some(to) = to.as_deref().and_then(|s| anim_expr::eval(s, &v)) {
                    let f = from.as_deref().and_then(|s| anim_expr::eval(s, &v)).unwrap_or(base);
                    Some(f + (to - f) * p)
                } else {
                    None
                };
                let Some(value) = value else { continue };
                match attr {
                    Attr::X => st.dx += value - geom.cx,
                    Attr::Y => st.dy += value - geom.cy,
                    Attr::W => st.sx *= if geom.w > 0.0 { value / geom.w } else { 1.0 },
                    Attr::H => st.sy *= if geom.h > 0.0 { value / geom.h } else { 1.0 },
                    Attr::Rot => st.rot += value,
                    Attr::Opacity => st.opacity *= value.clamp(0.0, 1.0),
                }
            }
            Beh::Scale { from, to } => {
                st.sx *= from.0 + (to.0 - from.0) * p;
                st.sy *= from.1 + (to.1 - from.1) * p;
            }
            Beh::Rotate { from, to } => st.rot += from + (to - from) * p,
            Beh::Motion { pts } => {
                let (x, y) = along(pts, p);
                st.dx += x;
                st.dy += y;
            }
            Beh::Blink => {
                if p < 1.0 {
                    st.opacity *= 1.0 - 0.65 * (p * std::f32::consts::PI).sin();
                }
            }
        }
    }
}

/// Värdet mellan nyckelbilderna (tid 0..1, formel).
fn key_value(keys: &[(f32, String)], discrete: bool, p: f32, v: &Vars) -> Option<f32> {
    let i = keys.iter().rposition(|(t, _)| *t <= p).unwrap_or(0);
    let a = anim_expr::eval(&keys[i].1, v)?;
    if discrete || i + 1 >= keys.len() || p <= keys[i].0 {
        return Some(a);
    }
    let b = anim_expr::eval(&keys[i + 1].1, v)?;
    let span = keys[i + 1].0 - keys[i].0;
    let k = if span > 0.0 { (p - keys[i].0) / span } else { 1.0 };
    Some(a + (b - a) * k)
}

/// Punkten på banan vid andelen `p` av längden.
fn along(pts: &[(f32, f32)], p: f32) -> (f32, f32) {
    let Some(&first) = pts.first() else { return (0.0, 0.0) };
    let lens: Vec<f32> = pts.windows(2).map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt()).collect();
    let total: f32 = lens.iter().sum();
    if total <= 0.0 {
        return *pts.last().unwrap_or(&first);
    }
    let mut d = p.clamp(0.0, 1.0) * total;
    for (i, l) in lens.iter().enumerate() {
        if d <= *l || i + 1 == lens.len() {
            let k = if *l > 0.0 { (d / l).min(1.0) } else { 1.0 };
            let (a, b) = (pts[i], pts[i + 1]);
            return (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k);
        }
        d -= l;
    }
    *pts.last().unwrap_or(&first)
}

fn find_main_seq(timing: &Node) -> Option<&Node> {
    fn walk(n: &Node) -> Option<&Node> {
        for c in &n.children {
            if c.name == "p:seq" {
                if let Some(ctn) = c.child("p:cTn") {
                    if ctn.attr("nodeType") == Some("mainSeq") {
                        return Some(ctn);
                    }
                }
                continue;
            }
            if let Some(f) = walk(c) {
                return Some(f);
            }
        }
        None
    }
    walk(timing)
}

/// Startar gruppen av sig själv (inte på klick)?
fn is_auto(ctn: &Node) -> bool {
    let Some(conds) = ctn.child("p:stCondLst") else { return false };
    let mut indefinite = false;
    let mut other = false;
    for c in conds.children_named("p:cond") {
        if c.attr("evt") == Some("onBegin") {
            return true;
        }
        match c.attr("delay") {
            Some("indefinite") => indefinite = true,
            Some(_) if c.attr("evt").is_none() => other = true,
            _ => {}
        }
    }
    other && !indefinite
}

/// Fördröjningen i `p:stCondLst` (s).
fn delay_of(ctn: &Node) -> f32 {
    ctn.child("p:stCondLst")
        .and_then(|l| l.children_named("p:cond").find(|c| c.attr("evt").is_none()))
        .and_then(|c| c.attr("delay"))
        .and_then(|d| d.parse::<f32>().ok())
        .map_or(0.0, |ms| (ms / 1000.0).max(0.0))
}

fn ms(ctn: &Node, name: &str) -> Option<f32> {
    ctn.attr(name).and_then(|d| d.parse::<f32>().ok()).map(|v| (v / 1000.0).max(0.0))
}

/// En effekt (en per stycke om den gäller ett styckeintervall).
fn parse_effect(ec: &Node, begin: f32) -> Vec<Effect> {
    let class = match ec.attr("presetClass") {
        Some("entr") => Class::Entrance,
        Some("exit") => Class::Exit,
        Some("emph") => Class::Emphasis,
        Some("path") => Class::Path,
        _ => Class::Other,
    };
    let preset = ec.attr_i64("presetID").unwrap_or(0);
    let hold = ec.attr("fill") != Some("remove");
    let mut behaviors = Vec::new();
    let mut target: Option<(u32, Option<(u32, u32)>)> = None;
    collect(ec.child("p:childTnLst"), 0.0, class, &mut behaviors, &mut target);
    let Some((shape, paras)) = target else { return Vec::new() };
    // En blink räcker per effekt.
    let mut blink = false;
    behaviors.retain(|b| !matches!(b.beh, Beh::Blink) || !std::mem::replace(&mut blink, true));
    let dur = behaviors.iter().map(|b| b.begin + b.total()).fold(0.0f32, f32::max);
    let make = |para| Effect { target: Target { shape, para }, class, preset, begin, dur, hold, behaviors: behaviors.clone() };
    match paras {
        Some((st, end)) => (st..=end.min(st + 999)).map(|p| make(Some(p))).collect(),
        None => vec![make(None)],
    }
}

/// Beteendena under `list` (även i inre `p:par`/`p:seq`), med fördröjningarna summerade.
fn collect(list: Option<&Node>, offset: f32, class: Class, out: &mut Vec<Behavior>, target: &mut Option<(u32, Option<(u32, u32)>)>) {
    let Some(list) = list else { return };
    for c in &list.children {
        match c.name.as_str() {
            "p:par" | "p:seq" => {
                if let Some(ctn) = c.child("p:cTn") {
                    collect(ctn.child("p:childTnLst"), offset + delay_of(ctn), class, out, target);
                }
            }
            "p:set" | "p:anim" | "p:animEffect" | "p:animScale" | "p:animRot" | "p:animMotion" | "p:animClr" => {
                let Some(cb) = c.child("p:cBhvr") else { continue };
                let Some(ctn) = cb.child("p:cTn") else { continue };
                let tgt = cb.child("p:tgtEl").and_then(|t| t.child("p:spTgt"));
                if let Some(t) = tgt {
                    let Some(spid) = t.attr_i64("spid").and_then(|v| u32::try_from(v).ok()) else { continue };
                    let paras = t.child("p:txEl").and_then(|x| x.child("p:pRg")).map(|r| {
                        let st = r.attr_i64("st").unwrap_or(0).max(0) as u32;
                        (st, (r.attr_i64("end").unwrap_or(st as i64).max(st as i64)) as u32)
                    });
                    match target {
                        None => *target = Some((spid, paras)),
                        // Beteenden för ett annat mål i samma effekt hoppas över.
                        Some(tg) if *tg != (spid, paras) => continue,
                        _ => {}
                    }
                } else if target.is_none() {
                    continue;
                }
                let attrs: Vec<&str> = cb.child("p:attrNameLst").into_iter().flat_map(|l| l.children_named("p:attrName")).map(|a| a.text.trim()).collect();
                let dur = match ctn.attr("dur") {
                    Some("indefinite") | None => 0.0,
                    _ => ms(ctn, "dur").unwrap_or(0.0).min(600.0),
                };
                let repeat = match ctn.attr("repeatCount") {
                    Some("indefinite") => 1.0,
                    Some(_) => ctn.attr_f64("repeatCount").map_or(1.0, |r| (r as f32 / 1000.0).clamp(0.001, 100.0)),
                    None => 1.0,
                };
                let accel = ctn.attr_f64("accel").map_or(0.0, |v| v as f32 / 100000.0);
                let decel = ctn.attr_f64("decel").map_or(0.0, |v| v as f32 / 100000.0);
                let Some(beh) = behavior(c, &attrs, class) else { continue };
                out.push(Behavior { beh, begin: offset + delay_of(ctn), dur, accel, decel, auto_rev: ctn.attr_bool("autoRev") == Some(true), repeat });
            }
            _ => {}
        }
    }
}

fn behavior(c: &Node, attrs: &[&str], class: Class) -> Option<Beh> {
    match c.name.as_str() {
        "p:set" => {
            if attrs.contains(&"style.visibility") {
                let v = c.child("p:to").and_then(|t| t.child("p:strVal")).and_then(|s| s.attr("val"));
                return Some(Beh::Visible(v != Some("hidden")));
            }
            // Färger och fyllning ändras inte (ritat i förväg): en blink markerar betoningen.
            let color = attrs.iter().any(|a| matches!(*a, "style.color" | "fillcolor" | "stroke.color" | "fill.type" | "fill.on" | "stroke.on"));
            (color && class == Class::Emphasis).then_some(Beh::Blink)
        }
        "p:animClr" => Some(Beh::Blink),
        "p:animEffect" => {
            let entering = c.attr("transition") != Some("out");
            let reveal = c.attr("filter").and_then(filter_reveal);
            Some(Beh::Filter { reveal, entering })
        }
        "p:anim" => {
            let attr = match attrs.first().copied() {
                Some("ppt_x") => Attr::X,
                Some("ppt_y") => Attr::Y,
                Some("ppt_w") => Attr::W,
                Some("ppt_h") => Attr::H,
                Some("style.rotation") | Some("r") => Attr::Rot,
                Some("style.opacity") => Attr::Opacity,
                _ => return None,
            };
            let mut keys = Vec::new();
            let tavs: Vec<&Node> = c.child("p:tavLst").into_iter().flat_map(|l| l.children_named("p:tav")).collect();
            for (i, tav) in tavs.iter().enumerate() {
                let tm = match tav.attr("tm") {
                    Some("indefinite") | None => {
                        if tavs.len() > 1 {
                            i as f32 / (tavs.len() - 1) as f32
                        } else {
                            0.0
                        }
                    }
                    _ => tav.attr_f64("tm").map_or(0.0, |v| (v as f32 / 100000.0).clamp(0.0, 1.0)),
                };
                let Some(val) = tav.child("p:val").and_then(|v| v.children.first()).and_then(|v| v.attr("val")) else { continue };
                keys.push((tm, val.to_owned()));
            }
            Some(Beh::Prop {
                attr,
                keys,
                discrete: c.attr("calcmode") == Some("discrete"),
                by: c.attr("by").map(str::to_owned),
                from: c.attr("from").map(str::to_owned),
                to: c.attr("to").map(str::to_owned),
            })
        }
        "p:animScale" => {
            let xy = |n: Option<&Node>| n.map(|n| (n.attr_f64("x").unwrap_or(100000.0) as f32 / 100000.0, n.attr_f64("y").unwrap_or(100000.0) as f32 / 100000.0));
            let from = xy(c.child("p:from")).unwrap_or((1.0, 1.0));
            let to = match (xy(c.child("p:to")), xy(c.child("p:by"))) {
                (Some(t), _) => t,
                (None, Some(b)) => (from.0 * b.0, from.1 * b.1),
                _ => from,
            };
            Some(Beh::Scale { from, to })
        }
        "p:animRot" => {
            let deg = |n: &str| c.attr_f64(n).map(|v| v as f32 / 60000.0);
            let from = deg("from").unwrap_or(0.0);
            let to = match (deg("to"), deg("by")) {
                (Some(t), _) => t,
                (None, Some(b)) => from + b,
                _ => from,
            };
            Some(Beh::Rotate { from, to })
        }
        "p:animMotion" => Some(Beh::Motion { pts: motion_path(c.attr("path").unwrap_or("")) }),
        _ => None,
    }
}

/// `filter="wipe(down)"` → avslöjningsform (`None` = toning).
fn filter_reveal(f: &str) -> Option<Reveal> {
    let (name, arg) = match f.split_once('(') {
        Some((n, a)) => (n.trim(), a.trim_end_matches(')').trim()),
        None => (f.trim(), ""),
    };
    Some(match name {
        // Torka åt ett håll: "wipe(down)" börjar uppifrån och går nedåt.
        "wipe" => Reveal::Wipe(match arg {
            "up" => Dir::Up,
            "down" => Dir::Down,
            "left" => Dir::Left,
            _ => Dir::Right,
        }),
        "barn" => Reveal::Split { vertical: arg.contains("Vertical"), out: arg.starts_with("out") },
        "circle" => Reveal::Circle { out: arg != "in" },
        "diamond" => Reveal::Diamond { out: arg != "in" },
        "plus" => Reveal::Plus { out: arg != "in" },
        "box" => Reveal::Box { out: arg != "in" },
        "blinds" => Reveal::Blinds { vertical: arg == "vertical" },
        "checkerboard" => Reveal::Checker { vertical: arg == "down" },
        "randombar" => Reveal::RandomBars { vertical: arg == "vertical" },
        "dissolve" => Reveal::Dissolve,
        "wheel" => Reveal::Wheel { spokes: arg.parse().unwrap_or(1) },
        "wedge" => Reveal::Wedge,
        "strips" => Reveal::Strips(match arg {
            "upLeft" => Dir::LeftUp,
            "upRight" => Dir::RightUp,
            "downLeft" => Dir::LeftDown,
            _ => Dir::RightDown,
        }),
        // "slide(fromTop)" m.fl.: toning.
        _ => return None,
    })
}

/// `path="M 0 0 L 0.25 0.1 C … E"` → polylinje (andelar av bilden, relativt startläget).
fn motion_path(s: &str) -> Vec<(f32, f32)> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphabetic() && !(matches!(ch, 'e' | 'E') && cur.chars().last().is_some_and(|c| c.is_ascii_digit() || c == '.')) {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            toks.push(ch.to_string());
        } else if ch.is_whitespace() || ch == ',' {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
        } else if ch == '-' && !cur.is_empty() && !cur.ends_with(['e', 'E']) {
            toks.push(std::mem::take(&mut cur));
            cur.push(ch);
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    let mut pts: Vec<(f32, f32)> = Vec::new();
    let mut i = 0;
    let mut cmd = 'M';
    let mut last = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    let num = |i: usize| toks.get(i).and_then(|t| t.parse::<f32>().ok());
    while i < toks.len() {
        if let Some(c) = toks[i].chars().next().filter(|c| c.is_ascii_alphabetic() && toks[i].len() == 1) {
            cmd = c;
            i += 1;
            if matches!(c, 'Z' | 'z') {
                pts.push(start);
                last = start;
            }
            if matches!(c, 'E' | 'e') {
                break;
            }
            continue;
        }
        let rel = cmd.is_ascii_lowercase();
        let abs = |x: f32, y: f32, last: (f32, f32)| if rel { (last.0 + x, last.1 + y) } else { (x, y) };
        match cmd.to_ascii_uppercase() {
            'M' | 'L' => {
                let (Some(x), Some(y)) = (num(i), num(i + 1)) else { break };
                last = abs(x, y, last);
                if cmd.eq_ignore_ascii_case(&'M') {
                    start = last;
                }
                pts.push(last);
                i += 2;
            }
            'C' => {
                let v: Option<Vec<f32>> = (0..6).map(|k| num(i + k)).collect();
                let Some(v) = v else { break };
                let p0 = last;
                let p1 = abs(v[0], v[1], last);
                let p2 = abs(v[2], v[3], last);
                let p3 = abs(v[4], v[5], last);
                for k in 1..=8 {
                    let t = k as f32 / 8.0;
                    let u = 1.0 - t;
                    let x = u * u * u * p0.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0;
                    let y = u * u * u * p0.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1;
                    pts.push((x, y));
                }
                last = p3;
                i += 6;
            }
            _ => i += 1,
        }
    }
    if pts.is_empty() {
        pts.push((0.0, 0.0));
    }
    pts
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn timing(groups: &str) -> Node {
        let x = format!(
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst>{groups}</p:childTnLst></p:cTn><p:prevCondLst/><p:nextCondLst/></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing></p:sld>"#
        );
        let root = super::super::xml::parse(x.as_bytes()).unwrap();
        assert_eq!(root.name, "p:sld");
        root
    }

    fn g() -> Geom {
        Geom { cx: 0.5, cy: 0.5, w: 0.2, h: 0.1 }
    }


    /// En klickgrupp: `auto` = startar av sig själv; `parts` = tidsförskjutna delar (fördröjning, effekter).
    pub(crate) fn click(auto: bool, parts: &[(u32, String)]) -> String {
        let cond = if auto { r#"<p:cond delay="indefinite"/><p:cond evt="onBegin" delay="0"><p:tn val="2"/></p:cond>"# } else { r#"<p:cond delay="indefinite"/>"# };
        let mut s = format!(r#"<p:par><p:cTn fill="hold"><p:stCondLst>{cond}</p:stCondLst><p:childTnLst>"#);
        for (delay, effects) in parts {
            s += &format!(r#"<p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="{delay}"/></p:stCondLst><p:childTnLst>{effects}</p:childTnLst></p:cTn></p:par>"#);
        }
        s + "</p:childTnLst></p:cTn></p:par>"
    }

    fn tgt(spid: u32, para: Option<u32>) -> String {
        match para {
            Some(p) => format!(r#"<p:tgtEl><p:spTgt spid="{spid}"><p:txEl><p:pRg st="{p}" end="{p}"/></p:txEl></p:spTgt></p:tgtEl>"#),
            None => format!(r#"<p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl>"#),
        }
    }

    fn set_vis(spid: u32, para: Option<u32>, v: &str, delay: u32) -> String {
        format!(
            r#"<p:set><p:cBhvr><p:cTn dur="1" fill="hold"><p:stCondLst><p:cond delay="{delay}"/></p:stCondLst></p:cTn>{}<p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="{v}"/></p:to></p:set>"#,
            tgt(spid, para)
        )
    }

    /// Effekt: `class` entr/exit/emph/path, `node` clickEffect/withEffect/afterEffect, `body` beteenden.
    fn effect(class: &str, id: i64, node: &str, delay: u32, body: &str) -> String {
        format!(r#"<p:par><p:cTn presetID="{id}" presetClass="{class}" presetSubtype="0" fill="hold" nodeType="{node}"><p:stCondLst><p:cond delay="{delay}"/></p:stCondLst><p:childTnLst>{body}</p:childTnLst></p:cTn></p:par>"#)
    }

    /// Visas (in-effekt 1).
    pub(crate) fn appear(spid: u32, para: Option<u32>, node: &str) -> String {
        effect("entr", 1, node, 0, &set_vis(spid, para, "visible", 0))
    }

    /// Tona in (in-effekt 10) på `ms`.
    pub(crate) fn fade_in(spid: u32, para: Option<u32>, node: &str, ms: u32) -> String {
        let body = set_vis(spid, para, "visible", 0)
            + &format!(r#"<p:animEffect transition="in" filter="fade"><p:cBhvr><p:cTn dur="{ms}"/>{}</p:cBhvr></p:animEffect>"#, tgt(spid, para));
        effect("entr", 10, node, 0, &body)
    }

    fn wipe_out(spid: u32, node: &str, ms: u32) -> String {
        let body = format!(r#"<p:animEffect transition="out" filter="wipe(down)"><p:cBhvr><p:cTn dur="{ms}"/>{}</p:cBhvr></p:animEffect>"#, tgt(spid, None))
            + &set_vis(spid, None, "hidden", ms.saturating_sub(1));
        effect("exit", 22, node, 0, &body)
    }

    fn fly_in(spid: u32, node: &str, ms: u32) -> String {
        let body = set_vis(spid, None, "visible", 0)
            + &format!(
                r##"<p:anim calcmode="lin" valueType="num"><p:cBhvr additive="base"><p:cTn dur="{ms}" fill="hold"/>{}<p:attrNameLst><p:attrName>ppt_x</p:attrName></p:attrNameLst></p:cBhvr><p:tavLst><p:tav tm="0"><p:val><p:strVal val="0-#ppt_w/2"/></p:val></p:tav><p:tav tm="100000"><p:val><p:strVal val="#ppt_x"/></p:val></p:tav></p:tavLst></p:anim>"##,
                tgt(spid, None)
            );
        effect("entr", 2, node, 0, &body)
    }

    fn motion(spid: u32, node: &str, ms: u32) -> String {
        let body = format!(r#"<p:animMotion origin="layout" path="M 0 0 L 0.25 0 E" pathEditMode="relative"><p:cBhvr><p:cTn dur="{ms}" fill="hold"/>{}<p:attrNameLst><p:attrName>ppt_x</p:attrName><p:attrName>ppt_y</p:attrName></p:attrNameLst></p:cBhvr></p:animMotion>"#, tgt(spid, None));
        effect("path", 42, node, 0, &body)
    }

    fn spin(spid: u32, node: &str, ms: u32) -> String {
        let body = format!(r#"<p:animRot by="21600000"><p:cBhvr><p:cTn dur="{ms}" fill="hold"/>{}<p:attrNameLst><p:attrName>r</p:attrName></p:attrNameLst></p:cBhvr></p:animRot>"#, tgt(spid, None));
        effect("emph", 8, node, 0, &body)
    }

    fn vis(t: &Timeline, target: Target, step: usize) -> bool {
        let end = t.groups_at_step(step);
        t.state(target, g(), end..end, f32::INFINITY).visible
    }

    #[test]
    fn klick_steg_och_synlighet_per_steg() {
        let a = Target { shape: 4, para: None };
        let b = Target { shape: 5, para: None };
        let c = Target { shape: 6, para: None };
        // Klick 1: A tonas in, B visas samtidigt (med föregående); klick 2: C flyger in och efter det
        // torkas A ut (efter föregående).
        let xml = click(false, &[(0, fade_in(4, None, "clickEffect", 500) + &appear(5, None, "withEffect"))])
            + &click(false, &[(0, fly_in(6, "clickEffect", 1000)), (1000, wipe_out(4, "afterEffect", 500))]);
        let t = Timeline::of_slide(&timing(&xml));
        assert_eq!(t.clicks(), 2);
        assert_eq!(t.targets(), vec![a, b, c]);
        assert!(t.initially_hidden(a) && t.initially_hidden(b) && t.initially_hidden(c));
        assert_eq!((vis(&t, a, 0), vis(&t, b, 0), vis(&t, c, 0)), (false, false, false));
        assert_eq!((vis(&t, a, 1), vis(&t, b, 1), vis(&t, c, 1)), (true, true, false));
        assert_eq!((vis(&t, a, 2), vis(&t, b, 2), vis(&t, c, 2)), (false, true, true));
        // Längder: grupp 1 0,5 s, grupp 2 1 s + 0,5 s.
        assert!((t.chain_dur(1) - 0.5).abs() < 1e-3);
        assert!((t.chain_dur(2) - 1.5).abs() < 1e-3);
        // Mitt i toningen: halvt genomskinlig.
        let st = t.state(a, g(), t.chain(1), 0.25);
        assert!(st.visible && (st.opacity - 0.5).abs() < 0.01, "{st:?}");
        // Mitt i inflygningen: formen är till vänster om sitt läge.
        let st = t.state(c, g(), t.chain(2), 0.5);
        assert!(st.visible && st.dx < -0.2, "{st:?}");
        // Efter inflygningen (1,2 s) torkas A ut: halva syns.
        let st = t.state(a, g(), t.chain(2), 1.25);
        assert!(st.visible && matches!(st.reveal, Some((Reveal::Wipe(Dir::Down), p)) if (p - 0.5).abs() < 0.01), "{st:?}");
        // Under inflygningen syns A fortfarande helt.
        let st = t.state(a, g(), t.chain(2), 0.5);
        assert_eq!(st, State::SHOWN);
    }

    #[test]
    fn efter_föregående_utan_skriven_fördröjning_väntar_ändå() {
        // Fördröjningen 0 på delen efter föregående: starten blir förra delens slut (2 s).
        let xml = click(false, &[(0, fade_in(4, None, "clickEffect", 2000)), (0, appear(5, None, "afterEffect"))]);
        let t = Timeline::of_slide(&timing(&xml));
        let b = Target { shape: 5, para: None };
        assert!(!t.state(b, g(), t.chain(1), 1.0).visible);
        assert!(t.state(b, g(), t.chain(1), 2.01).visible);
    }

    #[test]
    fn självstartande_grupp_när_bilden_visas() {
        let xml = click(true, &[(0, appear(4, None, "withEffect")), (0, fade_in(5, None, "afterEffect", 1000))]) + &click(false, &[(0, appear(6, None, "clickEffect"))]);
        let t = Timeline::of_slide(&timing(&xml));
        assert_eq!(t.clicks(), 1);
        assert_eq!(t.chain(0), 0..1);
        assert_eq!(t.chain(1), 1..2);
        assert!((t.chain_dur(0) - 1.0).abs() < 1e-2);
        let a = Target { shape: 4, para: None };
        assert!(!t.state(a, g(), t.chain(0), -1.0).visible, "dold innan gruppen börjat");
        assert!(t.state(a, g(), t.chain(0), 0.0).visible);
        assert!(vis(&t, a, 0));
        assert!(!vis(&t, Target { shape: 6, para: None }, 0));
    }

    #[test]
    fn stycke_för_stycke() {
        // bldP: stycke 0 och 1 var för sig på klick, stycke 2 med det andra.
        let xml = click(false, &[(0, appear(3, Some(0), "clickEffect"))]) + &click(false, &[(0, appear(3, Some(1), "clickEffect") + &appear(3, Some(2), "withEffect"))]);
        let t = Timeline::of_slide(&timing(&xml));
        let p = |n| Target { shape: 3, para: Some(n) };
        assert_eq!(t.targets(), vec![p(0), p(1), p(2)]);
        assert_eq!((vis(&t, p(0), 0), vis(&t, p(1), 0), vis(&t, p(2), 0)), (false, false, false));
        assert_eq!((vis(&t, p(0), 1), vis(&t, p(1), 1), vis(&t, p(2), 1)), (true, false, false));
        assert_eq!((vis(&t, p(0), 2), vis(&t, p(1), 2), vis(&t, p(2), 2)), (true, true, true));
        // Formen själv animeras inte: den syns hela tiden.
        assert!(vis(&t, Target { shape: 3, para: None }, 0));
        // Ett intervall av stycken blir ett mål per stycke.
        let xml = click(false, &[(0, effect("entr", 1, "clickEffect", 0, &r#"<p:set><p:cBhvr><p:cTn dur="1" fill="hold"/><p:tgtEl><p:spTgt spid="3"><p:txEl><p:pRg st="1" end="3"/></p:txEl></p:spTgt></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set>"#.to_string()))]);
        assert_eq!(Timeline::of_slide(&timing(&xml)).targets(), vec![p(1), p(2), p(3)]);
    }

    #[test]
    fn rörelsebana_snurr_och_ut_effekt_först() {
        let xml = click(false, &[(0, motion(4, "clickEffect", 1000))]) + &click(false, &[(0, spin(4, "clickEffect", 1000))]) + &click(false, &[(0, wipe_out(5, "clickEffect", 500))]);
        let t = Timeline::of_slide(&timing(&xml));
        let a = Target { shape: 4, para: None };
        assert!(!t.initially_hidden(a), "bana: formen syns från början");
        assert!(!t.initially_hidden(Target { shape: 5, para: None }), "ut-effekt först: syns från början");
        let st = t.state(a, g(), t.chain(1), 0.5);
        assert!((st.dx - 0.125).abs() < 1e-4 && st.dy.abs() < 1e-6);
        // Banan står kvar efteråt (fill="hold"), och snurren går ett varv.
        let st = t.state(a, g(), t.chain(2), 0.5);
        assert!((st.dx - 0.25).abs() < 1e-4 && (st.rot - 180.0).abs() < 0.01, "{st:?}");
        let end = t.groups_at_step(3);
        let st = t.state(a, g(), end..end, f32::INFINITY);
        assert!((st.rot - 360.0).abs() < 0.01);
        assert!(!vis(&t, Target { shape: 5, para: None }, 3));
    }

    #[test]
    fn okända_beteenden_och_interaktiva_sekvenser() {
        // Okänt filter blir toning; okänd effekt med bara synlighet visas i rätt ordning.
        let odd = effect("entr", 777, "clickEffect", 0, &(set_vis(4, None, "visible", 0) + r#"<p:animEffect transition="in" filter="okänt(x)"><p:cBhvr><p:cTn dur="500"/><p:tgtEl><p:spTgt spid="4"/></p:tgtEl></p:cBhvr></p:animEffect><p:cmd type="verb" cmd="play"><p:cBhvr><p:cTn dur="1"/><p:tgtEl><p:spTgt spid="4"/></p:tgtEl></p:cBhvr></p:cmd>"#));
        let t = Timeline::of_slide(&timing(&click(false, &[(0, odd)])));
        let a = Target { shape: 4, para: None };
        assert!(!vis(&t, a, 0) && vis(&t, a, 1));
        assert!((t.state(a, g(), t.chain(1), 0.25).opacity - 0.5).abs() < 0.01);
        // En utlösare (interactiveSeq) räknas inte: formen syns.
        let x = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:timing><p:tnLst><p:par><p:cTn nodeType="tmRoot"><p:childTnLst><p:seq><p:cTn nodeType="interactiveSeq"><p:childTnLst><p:par><p:cTn><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:par><p:cTn presetID="1" presetClass="entr" nodeType="clickEffect"><p:childTnLst><p:set><p:cBhvr><p:cTn dur="1"/><p:tgtEl><p:spTgt spid="9"/></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing></p:sld>"#;
        let t = Timeline::of_slide(&super::super::xml::parse(x.as_bytes()).unwrap());
        assert!(t.targets().is_empty());
    }

    #[test]
    fn tom_tidslinje() {
        let t = Timeline::of_slide(&timing(""));
        assert_eq!(t.clicks(), 0);
        assert_eq!(t.chain(0), 0..0);
        assert!(t.targets().is_empty());
    }

    #[test]
    fn accel_decel_kurvan() {
        assert_eq!(accel_decel(0.3, 0.0, 0.0), 0.3);
        for (a, d) in [(0.5, 0.5), (0.2, 0.0), (0.0, 0.3)] {
            assert!(accel_decel(0.0, a, d).abs() < 1e-6);
            assert!((accel_decel(1.0, a, d) - 1.0).abs() < 1e-5);
            let mut prev = 0.0;
            for k in 1..=20 {
                let v = accel_decel(k as f32 / 20.0, a, d);
                assert!(v >= prev - 1e-6);
                prev = v;
            }
        }
    }

    #[test]
    fn banor() {
        let p = motion_path("M -6.94444E-6 -6.93889E-18 L -6.94444E-6 0.13333 E");
        assert_eq!(p.len(), 2);
        assert!((p[1].1 - 0.13333).abs() < 1e-5);
        let (x, y) = along(&p, 0.5);
        assert!(x.abs() < 1e-4 && (y - 0.066665).abs() < 1e-4);
        let c = motion_path("M 0 0 C 0.1 0 0.2 0 0.3 0 E");
        assert!((c.last().unwrap().0 - 0.3).abs() < 1e-6);
        assert_eq!(c.len(), 9);
        let r = motion_path("m 0 0 l 0.1 0.1 l 0.1 0 Z");
        assert!((r[2].0 - 0.2).abs() < 1e-6 && (r[2].1 - 0.1).abs() < 1e-6);
        assert_eq!(*r.last().unwrap(), (0.0, 0.0));
        assert_eq!(motion_path("trasig"), vec![(0.0, 0.0)]);
    }
}
