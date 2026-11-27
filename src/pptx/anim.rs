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
//!
//! Utlösare (`interactiveSeq`, `Timeline::triggers`): en sekvens som startar när man klickar på en form.
//! Den ger egna grupper som spelas efter varandra vid ett klick (`Trigger::state`); former som en
//! utlösare visar in är dolda tills den klickats. En utlösare vars form inte går att träffa tas bort
//! (`Timeline::retain_triggers`), så att dess mål visas som vanligt.
//!
//! Bokstav för bokstav (`p:iterate`) och teckenintervall (`p:charRg`): en effekt får `iter` (slag,
//! steg, antal delar) respektive `chars`. Delarna (bokstäver eller ord) får sin egen fördröjning
//! (`Timeline::state_unit`); lagerdelningen (`anim_layers.rs`) ger ett lager per del.
//!
//! Mediakommandon (`p:cmd`, `MediaCmd`): läggs på gruppen (`Group::cmds`) och ges ut av uppspelningen
//! (`slideshow::Playback::take_media`). Ingen ljud- eller videokod här.

use super::anim_expr::{self, Vars};
use super::transition::{Dir, Reveal};
use super::xml::Node;

/// Det en effekt verkar på: en form (`p:cNvPr/@id`) eller ett stycke i dess text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Target {
    pub shape: u32,
    pub para: Option<u32>,
}

/// Vad ett medieskommando (`p:cmd`) gör med en ljud- eller videoform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaKind {
    /// `playFrom(t)` (och `play`, från 0): starta från `from_s` sekunder.
    Play { from_s: f32 },
    /// `togglePause`.
    TogglePause,
    /// `stop`: stoppa och gå till början.
    Stop,
    /// `pause`.
    Pause,
    /// `resume`: fortsätt efter paus.
    Resume,
}

/// Ett mediakommando till formen `shape_id` (`p:cNvPr/@id`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MediaCmd {
    pub shape_id: u32,
    pub kind: MediaKind,
}

/// En ljud- eller videoform på bilden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaShape {
    pub id: u32,
    /// Video (annars ljud).
    pub video: bool,
}

/// Tolkar `p:cmd/@cmd`.
fn parse_cmd(s: &str) -> Option<MediaKind> {
    let s = s.trim();
    let (name, arg) = match s.split_once('(') {
        Some((n, a)) => (n.trim(), a.trim_end_matches(')').trim()),
        None => (s, ""),
    };
    Some(match name.to_ascii_lowercase().as_str() {
        "playfrom" => MediaKind::Play { from_s: arg.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0) },
        "play" => MediaKind::Play { from_s: 0.0 },
        "togglepause" => MediaKind::TogglePause,
        "stop" => MediaKind::Stop,
        "pause" => MediaKind::Pause,
        "resume" => MediaKind::Resume,
        _ => return None,
    })
}

/// Vad `p:iterate` delar texten i.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    /// `type="lt"`: tecken.
    Letters,
    /// `type="wd"`: ord (`scene::word_index`).
    Words,
}

/// `p:iterate`: varje del börjar `step` sekunder efter föregående. `n` är antalet delar (räknat ur
/// bildens text; delar bortom `n` följer den sista).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Iter {
    pub kind: UnitKind,
    pub step: f32,
    pub n: u32,
    pub backwards: bool,
}

/// En text som delas i delar: stycket (`para`) eller hela formens text i formen `shape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitScope {
    pub shape: u32,
    pub para: Option<u32>,
    pub kind: UnitKind,
}

/// Flest delar per effekt (fler ger en vanlig effekt för hela texten).
const MAX_UNITS: usize = 600;

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
    /// Längd (s), med fördröjningen för sista delen när effekten delas (`iter`).
    pub dur: f32,
    /// Längden för en del (s).
    base_dur: f32,
    hold: bool,
    behaviors: Vec<Behavior>,
    /// Bokstav för bokstav eller ord för ord.
    pub iter: Option<Iter>,
    /// Bara tecknen `st..=end` i formens text (`p:charRg`).
    pub chars: Option<(u32, u32)>,
}

impl Effect {
    /// Fördröjningen (s) för delen `unit` av texten; `None` om effekten inte gäller den. `unit = None`
    /// är texten som helhet (utan delar) och får ingen fördröjning.
    fn shift(&self, unit: Option<u32>) -> Option<f32> {
        let idx = match (self.chars, unit) {
            (Some((a, b)), Some(k)) if (a..=b).contains(&k) => Some(k - a),
            (Some(_), _) => None,
            (None, k) => Some(k.unwrap_or(0)),
        }?;
        Some(match (self.iter, unit) {
            (Some(it), Some(_)) => {
                let n = it.n.max(1);
                let i = idx.min(n - 1);
                (if it.backwards { n - 1 - i } else { i }) as f32 * it.step
            }
            _ => 0.0,
        })
    }
}

/// En klickgrupp.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Startar av sig själv (när bilden visas eller när gruppen före är klar).
    pub auto: bool,
    pub effects: Vec<Effect>,
    pub dur: f32,
    /// Mediakommandon (`p:cmd`) och när de ges (s efter gruppens start). Räknas inte in i `dur`.
    pub cmds: Vec<(f32, MediaCmd)>,
}

/// En utlösare (`interactiveSeq`): ett klick på formen `shape` spelar `groups` efter varandra.
#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    pub shape: u32,
    pub groups: Vec<Group>,
}

impl Trigger {
    /// Hur länge sekvensen tar (s).
    pub fn dur(&self) -> f32 {
        self.groups.iter().map(|g| g.dur).sum()
    }

    /// Mediakommandona med tiden (s) efter klicket.
    pub fn cmds(&self) -> Vec<(f32, MediaCmd)> {
        cmds_of(&self.groups, 0..self.groups.len())
    }

    /// Verkar sekvensen på målet?
    pub fn affects(&self, t: Target) -> bool {
        self.groups.iter().flat_map(|g| &g.effects).any(|e| e.target == t)
    }

    /// Målets (eller delens) tillstånd `t` sekunder efter klicket (`t < 0` = inte klickad än,
    /// `f32::INFINITY` = klar).
    pub fn state(&self, target: Target, unit: Option<u32>, geom: Geom, t: f32) -> State {
        let chain = if t < 0.0 { 0..0 } else { 0..self.groups.len() };
        groups_state(&self.groups, target, unit, geom, chain, t.max(0.0))
    }
}

/// En bilds animeringar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    pub groups: Vec<Group>,
    /// Utlösare (`interactiveSeq`), i filens ordning.
    pub triggers: Vec<Trigger>,
    /// Ljud- och videoformerna på bilden.
    pub media: Vec<MediaShape>,
}

/// Kommandona i grupperna `range` med tiden från första gruppens start.
fn cmds_of(groups: &[Group], range: std::ops::Range<usize>) -> Vec<(f32, MediaCmd)> {
    let mut out = Vec::new();
    let mut offset = 0.0;
    for g in &groups[range] {
        out.extend(g.cmds.iter().map(|(t, c)| (offset + t, *c)));
        offset += g.dur;
    }
    out
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

    /// Samma tillstånd (inom flyttalsfel)? Avgör om en text som delats i delar kan ritas som helhet.
    pub fn approx_eq(&self, o: &State) -> bool {
        let e = 1e-5;
        self.visible == o.visible
            && (self.opacity - o.opacity).abs() < e
            && (self.dx - o.dx).abs() < e
            && (self.dy - o.dy).abs() < e
            && (self.sx - o.sx).abs() < e
            && (self.sy - o.sy).abs() < e
            && (self.rot - o.rot).abs() < e * 100.0
            && match (self.reveal, o.reveal) {
                (None, None) => true,
                (Some((a, p)), Some((b, q))) => a == b && (p - q).abs() < e,
                _ => false,
            }
    }

    /// Syns något alls?
    pub fn drawn(&self) -> bool {
        self.visible && self.opacity > 0.001 && self.reveal.is_none_or(|(_, p)| p > 0.0) && self.sx.abs() > 1e-4 && self.sy.abs() > 1e-4
    }
}

/// Klickgrupperna under en sekvens (`p:cTn` för huvudsekvensen eller en utlösare).
fn parse_groups(seq: &Node, texts: &Texts) -> Vec<Group> {
    let mut groups = Vec::new();
    for click in seq.child("p:childTnLst").into_iter().flat_map(|l| l.children_named("p:par")) {
        let Some(ctn) = click.child("p:cTn") else { continue };
        let auto = is_auto(ctn);
        let mut effects = Vec::new();
        let mut cmds = Vec::new();
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
                for e in parse_effect(ec, start + delay_of(ec), texts, &mut cmds) {
                    end = end.max(e.begin + e.dur);
                    effects.push(e);
                }
            }
            prev_end = end;
        }
        let dur = effects.iter().map(|e| e.begin + e.dur).fold(0.0f32, f32::max);
        groups.push(Group { auto, effects, dur, cmds });
    }
    groups
}

impl Timeline {
    /// Bildens tidsträd (`p:sld/p:timing`); utan animeringar en tom tidslinje.
    pub fn of_slide(root: &Node) -> Timeline {
        let media = media_shapes(root);
        let Some(timing) = root.child("p:timing") else { return Timeline { media, ..Timeline::default() } };
        let texts = if timing.find("p:iterate").is_some() || timing.find("p:charRg").is_some() { Texts::of(root) } else { Texts::default() };
        let groups = find_seqs(timing, "mainSeq").first().map(|m| parse_groups(m, &texts)).unwrap_or_default();
        let triggers = find_seqs(timing, "interactiveSeq")
            .into_iter()
            .filter_map(|ctn| Some(Trigger { shape: trigger_shape(ctn)?, groups: parse_groups(ctn, &texts) }))
            .collect();
        Timeline { groups, triggers, media }
    }

    /// Behåller bara utlösarna vars form går att träffa (`hittable`); de andra tas bort så att deras mål
    /// visas som vanligt (aldrig dolda för alltid).
    pub fn retain_triggers(&mut self, hittable: impl Fn(u32) -> bool) {
        self.triggers.retain(|t| hittable(t.shape));
    }

    /// Alla effekter, i huvudsekvensen och i utlösarna.
    pub(crate) fn all_effects(&self) -> impl Iterator<Item = &Effect> {
        self.groups.iter().chain(self.triggers.iter().flat_map(|t| &t.groups)).flat_map(|g| &g.effects)
    }

    /// Mediakommandona i klickstegets kedja `step`, med tiden (s) från kedjans start.
    pub fn chain_cmds(&self, step: usize) -> Vec<(f32, MediaCmd)> {
        cmds_of(&self.groups, self.chain(step))
    }

    /// Texterna som delas i delar (bokstäver eller ord) av `p:iterate` eller `p:charRg`.
    pub fn unit_scopes(&self) -> Vec<UnitScope> {
        let mut out: Vec<UnitScope> = Vec::new();
        for e in self.all_effects() {
            let kind = match (e.iter, e.chars) {
                (Some(it), None) => it.kind,
                (_, Some(_)) => UnitKind::Letters,
                _ => continue,
            };
            if !out.iter().any(|s| s.shape == e.target.shape && s.para == e.target.para) {
                out.push(UnitScope { shape: e.target.shape, para: e.target.para, kind });
            }
        }
        out
    }

    /// Är `t` ett mål för någon effekt?
    pub fn has_target(&self, t: Target) -> bool {
        self.all_effects().any(|e| e.target == t)
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
        let mut v: Vec<Target> = self.all_effects().map(|e| e.target).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Är målet dolt innan något har hänt (första effekten som visar eller döljer det är en in-effekt)?
    /// Bara huvudsekvensen räknas; utlösarnas mål döljs av `Trigger::state`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn initially_hidden(&self, t: Target) -> bool {
        initially_hidden_in(&self.groups, t, None)
    }

    /// Målets tillstånd när grupperna före `chain.start` är klara och grupperna i `chain` har spelats
    /// i `t` sekunder (efter varandra); grupperna efter har inte börjat. `t = f32::INFINITY` ger vilan.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn state(&self, target: Target, geom: Geom, chain: std::ops::Range<usize>, t: f32) -> State {
        groups_state(&self.groups, target, None, geom, chain, t)
    }

    /// Lagrets tillstånd: huvudsekvensen och utlösarna (`trig`: deras lägen, se `slideshow::SlideState`)
    /// för målet eller dess del `unit`; ett stycke följer också formens egna effekter.
    pub fn layer_state(&self, target: Target, unit: Option<u32>, geom: Geom, chain: std::ops::Range<usize>, t: f32, trig: &[f32]) -> State {
        let scoped = |tg: Target, u: Option<u32>| {
            let mut s = groups_state(&self.groups, tg, u, geom, chain.clone(), t);
            for (i, tr) in self.triggers.iter().enumerate().filter(|(_, tr)| tr.affects(tg)) {
                s = s.compose(&tr.state(tg, u, geom, trig.get(i).copied().unwrap_or(-1.0)));
            }
            s
        };
        let whole = Target { shape: target.shape, para: None };
        match target.para {
            Some(_) if self.has_target(whole) => scoped(whole, None).compose(&scoped(target, unit)),
            _ => scoped(target, unit),
        }
    }

    /// Som `state` men för delen `unit` (bokstav eller ord) av målets text.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn state_unit(&self, target: Target, unit: Option<u32>, geom: Geom, chain: std::ops::Range<usize>, t: f32) -> State {
        groups_state(&self.groups, target, unit, geom, chain, t)
    }
}

/// Om alla delar har samma tillstånd: det. Då ritas texten som helhet (exakt samma bild som utan
/// delning); annars ritas delarna var för sig.
pub fn uniform(states: &[State]) -> Option<State> {
    let first = *states.first()?;
    states.iter().all(|s| s.approx_eq(&first)).then_some(first)
}

fn initially_hidden_in(groups: &[Group], t: Target, unit: Option<u32>) -> bool {
    for g in groups {
        let mut es: Vec<&Effect> = g.effects.iter().filter(|e| e.target == t && e.shift(unit).is_some()).collect();
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

fn groups_state(groups: &[Group], target: Target, unit: Option<u32>, geom: Geom, chain: std::ops::Range<usize>, t: f32) -> State {
    let mut st = State::SHOWN;
    st.visible = !initially_hidden_in(groups, target, unit);
    let mut offset = 0.0f32;
    for (gi, g) in groups.iter().enumerate() {
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
        let mut es: Vec<(&Effect, f32)> = g.effects.iter().filter(|e| e.target == target).filter_map(|e| Some((e, e.shift(unit)?))).collect();
        es.sort_by(|a, b| a.0.begin.total_cmp(&b.0.begin));
        for (e, shift) in es {
            let local = time - e.begin - shift;
            if local < 0.0 {
                continue;
            }
            apply_effect(&mut st, e, local, geom);
        }
    }
    st
}

fn apply_effect(st: &mut State, e: &Effect, local: f32, geom: Geom) {
    let done = local >= e.base_dur;
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

/// Sekvenserna (`p:seq/p:cTn`) med `nodeType` (`mainSeq` eller `interactiveSeq`), i filens ordning.
fn find_seqs<'a>(timing: &'a Node, node_type: &str) -> Vec<&'a Node> {
    fn walk<'a>(n: &'a Node, node_type: &str, out: &mut Vec<&'a Node>) {
        for c in &n.children {
            if c.name == "p:seq" {
                if let Some(ctn) = c.child("p:cTn") {
                    if ctn.attr("nodeType") == Some(node_type) {
                        out.push(ctn);
                    }
                }
                continue;
            }
            walk(c, node_type, out);
        }
    }
    let mut out = Vec::new();
    walk(timing, node_type, &mut out);
    out
}

/// Formen som startar en utlösare: `p:stCondLst/p:cond[@evt="onClick"]/p:tgtEl/p:spTgt/@spid`.
fn trigger_shape(ctn: &Node) -> Option<u32> {
    ctn.child("p:stCondLst")?
        .children_named("p:cond")
        .filter(|c| c.attr("evt") == Some("onClick"))
        .find_map(|c| c.path(&["p:tgtEl", "p:spTgt"])?.attr_i64("spid"))
        .and_then(|v| u32::try_from(v).ok())
}

/// Ljud- och videoformerna på bilden: former med `a:videoFile`, `a:audioFile`, `a:wavAudioFile`,
/// `a:quickTimeFile` eller `p14:media` i `p:nvPr`.
pub fn media_shapes(root: &Node) -> Vec<MediaShape> {
    let mut out: Vec<MediaShape> = Vec::new();
    root.walk(&mut |n| {
        let (Some(id), Some(nv)) = (n.child("p:cNvPr").and_then(|c| c.attr_i64("id")), n.child("p:nvPr")) else { return };
        let Ok(id) = u32::try_from(id) else { return };
        let (mut video, mut audio, mut any) = (false, false, false);
        nv.walk(&mut |m| match m.name.as_str() {
            "a:videoFile" | "a:quickTimeFile" => (video, any) = (true, true),
            "a:audioFile" | "a:wavAudioFile" => (audio, any) = (true, true),
            "p14:media" => any = true,
            _ => {}
        });
        if any && !out.iter().any(|m| m.id == id) {
            out.push(MediaShape { id, video: video || !audio });
        }
    });
    out
}

/// Formernas stycken som tecken, för att räkna delarna vid `p:iterate` (samma uppdelning som
/// ritningen: `a:t` i körningar och fält, ett tecken för `a:br`).
#[derive(Default)]
struct Texts {
    shapes: std::collections::HashMap<u32, Vec<Vec<char>>>,
}

impl Texts {
    fn of(root: &Node) -> Texts {
        let mut shapes = std::collections::HashMap::new();
        root.walk(&mut |n| {
            let Some(tb) = n.child("p:txBody") else { return };
            let Some(id) = n.children.first().and_then(|nv| nv.child("p:cNvPr")).and_then(|c| c.attr_i64("id")).and_then(|v| u32::try_from(v).ok()) else { return };
            let paras = tb
                .children_named("a:p")
                .map(|p| {
                    let mut chars = Vec::new();
                    for r in &p.children {
                        match r.name.as_str() {
                            "a:r" | "a:fld" => chars.extend(r.child("a:t").map_or("", |t| t.text.as_str()).chars()),
                            "a:br" => chars.push('\n'),
                            _ => {}
                        }
                    }
                    chars
                })
                .collect();
            shapes.insert(id, paras);
        });
        Texts { shapes }
    }

    /// Antal delar i stycket (eller hela formens text): tecken eller ord.
    fn units(&self, shape: u32, para: Option<u32>, kind: UnitKind) -> usize {
        let Some(paras) = self.shapes.get(&shape) else { return 0 };
        let count = |p: &Vec<char>| match kind {
            UnitKind::Letters => p.len(),
            UnitKind::Words => p.last().map_or(0, |_| *super::scene::word_index(p).last().unwrap_or(&0) as usize + 1),
        };
        match para {
            Some(i) => paras.get(i as usize).map_or(0, count),
            None => paras.iter().map(count).sum(),
        }
    }
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

/// Målet för en effekt: formen, styckeintervall (`p:pRg`) och teckenintervall (`p:charRg`).
type Tgt = (u32, Option<(u32, u32)>, Option<(u32, u32)>);

/// En effekt (en per stycke om den gäller ett styckeintervall). Mediakommandon ur effekten läggs i
/// `cmds` med tiden i gruppen.
fn parse_effect(ec: &Node, begin: f32, texts: &Texts, cmds: &mut Vec<(f32, MediaCmd)>) -> Vec<Effect> {
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
    let mut target: Option<Tgt> = None;
    let mut found = Vec::new();
    collect(ec.child("p:childTnLst"), 0.0, class, &mut behaviors, &mut target, &mut found);
    cmds.extend(found.into_iter().map(|(t, c)| (begin + t, c)));
    let Some((shape, paras, chars)) = target else { return Vec::new() };
    // En blink räcker per effekt.
    let mut blink = false;
    behaviors.retain(|b| !matches!(b.beh, Beh::Blink) || !std::mem::replace(&mut blink, true));
    let dur = behaviors.iter().map(|b| b.begin + b.total()).fold(0.0f32, f32::max);
    // `p:iterate`: tid mellan delarna (`tmAbs` i ms eller `tmPct` av en dels längd).
    let iterate = ec.child("p:iterate").and_then(|it| {
        let kind = match it.attr("type") {
            Some("lt") => UnitKind::Letters,
            Some("wd") => UnitKind::Words,
            _ => return None,
        };
        let step = if let Some(a) = it.child("p:tmAbs") {
            a.attr_f64("val").unwrap_or(0.0) as f32 / 1000.0
        } else if let Some(p) = it.child("p:tmPct") {
            p.attr_f64("val").unwrap_or(10000.0) as f32 / 100000.0 * dur
        } else {
            0.1 * dur
        };
        Some((kind, step.max(0.0), it.attr_bool("backwards") == Some(true)))
    });
    let make = |para: Option<u32>, chars: Option<(u32, u32)>, n: usize| {
        let iter = iterate.filter(|_| (2..=MAX_UNITS).contains(&n)).map(|(kind, step, backwards)| Iter { kind, step, n: n as u32, backwards });
        let total = dur + iter.map_or(0.0, |i| (i.n - 1) as f32 * i.step);
        Effect { target: Target { shape, para }, class, preset, begin, dur: total, base_dur: dur, hold, behaviors: behaviors.clone(), iter, chars }
    };
    let kind = iterate.map_or(UnitKind::Letters, |i| i.0);
    match (paras, chars) {
        (_, Some((a, b))) => {
            let b = b.max(a).min(a.saturating_add(MAX_UNITS as u32 - 1));
            // Teckenintervallet delas alltid i tecken.
            let n = (b - a + 1) as usize;
            let mut e = make(None, Some((a, b)), n);
            if let Some(i) = &mut e.iter {
                i.kind = UnitKind::Letters;
            }
            vec![e]
        }
        (Some((st, end)), None) => (st..=end.min(st + 999)).map(|p| make(Some(p), None, texts.units(shape, Some(p), kind))).collect(),
        (None, None) => vec![make(None, None, texts.units(shape, None, kind))],
    }
}

/// Beteendena under `list` (även i inre `p:par`/`p:seq`), med fördröjningarna summerade.
fn collect(list: Option<&Node>, offset: f32, class: Class, out: &mut Vec<Behavior>, target: &mut Option<Tgt>, cmds: &mut Vec<(f32, MediaCmd)>) {
    let Some(list) = list else { return };
    for c in &list.children {
        match c.name.as_str() {
            "p:par" | "p:seq" => {
                if let Some(ctn) = c.child("p:cTn") {
                    collect(ctn.child("p:childTnLst"), offset + delay_of(ctn), class, out, target, cmds);
                }
            }
            "p:cmd" => {
                let Some(cb) = c.child("p:cBhvr") else { continue };
                let Some(ctn) = cb.child("p:cTn") else { continue };
                let shape = cb.path(&["p:tgtEl", "p:spTgt"]).and_then(|t| t.attr_i64("spid")).and_then(|v| u32::try_from(v).ok());
                let kind = c.attr("cmd").and_then(parse_cmd);
                if let (Some(shape_id), Some(kind), true) = (shape, kind, matches!(c.attr("type"), None | Some("call"))) {
                    cmds.push((offset + delay_of(ctn), MediaCmd { shape_id, kind }));
                }
            }
            "p:set" | "p:anim" | "p:animEffect" | "p:animScale" | "p:animRot" | "p:animMotion" | "p:animClr" => {
                let Some(cb) = c.child("p:cBhvr") else { continue };
                let Some(ctn) = cb.child("p:cTn") else { continue };
                let tgt = cb.child("p:tgtEl").and_then(|t| t.child("p:spTgt"));
                if let Some(t) = tgt {
                    let Some(spid) = t.attr_i64("spid").and_then(|v| u32::try_from(v).ok()) else { continue };
                    let range = |n: &Node| {
                        let st = n.attr_i64("st").unwrap_or(0).max(0) as u32;
                        (st, (n.attr_i64("end").unwrap_or(st as i64).max(st as i64)) as u32)
                    };
                    let paras = t.child("p:txEl").and_then(|x| x.child("p:pRg")).map(range);
                    let chars = t.child("p:txEl").and_then(|x| x.child("p:charRg")).map(range);
                    let tg: Tgt = (spid, paras, chars);
                    match target {
                        None => *target = Some(tg),
                        // Beteenden för ett annat mål i samma effekt hoppas över.
                        Some(old) if *old != tg => continue,
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
    fn okända_beteenden_och_sekvens_utan_utlösare() {
        // Okänt filter blir toning; okänd effekt med bara synlighet visas i rätt ordning.
        let odd = effect("entr", 777, "clickEffect", 0, &(set_vis(4, None, "visible", 0) + r#"<p:animEffect transition="in" filter="okänt(x)"><p:cBhvr><p:cTn dur="500"/><p:tgtEl><p:spTgt spid="4"/></p:tgtEl></p:cBhvr></p:animEffect><p:cmd type="verb" cmd="play"><p:cBhvr><p:cTn dur="1"/><p:tgtEl><p:spTgt spid="4"/></p:tgtEl></p:cBhvr></p:cmd>"#));
        let t = Timeline::of_slide(&timing(&click(false, &[(0, odd)])));
        let a = Target { shape: 4, para: None };
        assert!(!vis(&t, a, 0) && vis(&t, a, 1));
        assert!((t.state(a, g(), t.chain(1), 0.25).opacity - 0.5).abs() < 0.01);
        // En interaktiv sekvens utan klickvillkor på en form är ingen utlösare: formen syns.
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

    const SLD_NS: &str = r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

    /// En bild med formen 4 (texten `text`, stycken skilda av `|`), `extra` i formträdet, huvudsekvensen
    /// `main` och ytterligare sekvenser `seqs` (utlösare).
    fn slide(text: &str, extra: &str, main: &str, seqs: &str) -> Node {
        let paras: String = text.split('|').map(|t| format!("<a:p><a:r><a:t>{t}</a:t></a:r></a:p>")).collect();
        let x = format!(
            r#"<p:sld {SLD_NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/></p:nvGrpSpPr><p:sp><p:nvSpPr><p:cNvPr id="4" name="Text"/></p:nvSpPr><p:txBody><a:bodyPr/>{paras}</p:txBody></p:sp>{extra}</p:spTree></p:cSld><p:timing><p:tnLst><p:par><p:cTn id="1" nodeType="tmRoot"><p:childTnLst><p:seq><p:cTn id="2" nodeType="mainSeq"><p:childTnLst>{main}</p:childTnLst></p:cTn></p:seq>{seqs}</p:childTnLst></p:cTn></p:par></p:tnLst></p:timing></p:sld>"#
        );
        super::super::xml::parse(x.as_bytes()).unwrap()
    }

    /// En effekt med toning in på `ms` på `tgt`, med `iterate` (`it`, t.ex. `<p:iterate type="lt">…`).
    fn fade_iter(tgt: &str, it: &str, ms: u32, node: &str) -> String {
        format!(
            r#"<p:par><p:cTn presetID="10" presetClass="entr" presetSubtype="0" fill="hold" nodeType="{node}"><p:stCondLst><p:cond delay="0"/></p:stCondLst>{it}<p:childTnLst><p:set><p:cBhvr><p:cTn dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn>{tgt}<p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set><p:animEffect transition="in" filter="fade"><p:cBhvr><p:cTn dur="{ms}"/>{tgt}</p:cBhvr></p:animEffect></p:childTnLst></p:cTn></p:par>"#
        )
    }

    fn click_of(effects: &str) -> String {
        format!(r#"<p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst><p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst>{effects}</p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par>"#)
    }

    fn interactive(shape: u32, effects: &str) -> String {
        format!(
            r#"<p:seq concurrent="1" nextAc="seek"><p:cTn restart="whenNotActive" fill="hold" evtFilter="cancelBubble" nodeType="interactiveSeq"><p:stCondLst><p:cond evt="onClick" delay="0"><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl></p:cond></p:stCondLst><p:endSync evt="end" delay="0"><p:rtn val="all"/></p:endSync><p:childTnLst><p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:par><p:cTn fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst>{effects}</p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:seq>"#
        )
    }

    fn cmd_effect(shape: u32, cmd: &str, dur: u32) -> String {
        format!(
            r#"<p:par><p:cTn presetID="2" presetClass="mediacall" presetSubtype="0" fill="hold" nodeType="clickEffect"><p:stCondLst><p:cond delay="0"/></p:stCondLst><p:childTnLst><p:cmd type="call" cmd="{cmd}"><p:cBhvr><p:cTn dur="{dur}" fill="hold"/><p:tgtEl><p:spTgt spid="{shape}"/></p:tgtEl></p:cBhvr></p:cmd></p:childTnLst></p:cTn></p:par>"#
        )
    }

    fn unit(shape: u32, para: Option<u32>) -> Target {
        Target { shape, para }
    }

    #[test]
    fn utlösarens_tidsträd() {
        let tgt = r#"<p:tgtEl><p:spTgt spid="4"/></p:tgtEl>"#;
        let t = Timeline::of_slide(&slide("Hej", "", "", &interactive(8, &fade_iter(tgt, "", 1000, "clickEffect"))));
        assert_eq!(t.clicks(), 0, "utlösaren är inget klick-steg");
        assert_eq!(t.triggers.len(), 1);
        assert_eq!(t.triggers[0].shape, 8);
        assert!((t.triggers[0].dur() - 1.0).abs() < 1e-6);
        let a = unit(4, None);
        assert_eq!(t.targets(), vec![a]);
        // Huvudsekvensen visar målet som vanligt; utlösaren döljer det tills klicket.
        assert!(t.state(a, g(), 0..0, f32::INFINITY).visible);
        let tr = &t.triggers[0];
        assert!(!tr.state(a, None, g(), -1.0).visible);
        let mid = tr.state(a, None, g(), 0.5);
        assert!(mid.visible && (mid.opacity - 0.5).abs() < 0.01, "{mid:?}");
        let done = tr.state(a, None, g(), f32::INFINITY);
        assert!(done.visible && done.opacity > 0.999);
        // Utan klickvillkor är det ingen utlösare.
        let no_cond = interactive(8, &fade_iter(tgt, "", 1000, "clickEffect")).replace(r#"evt="onClick" "#, "");
        assert!(Timeline::of_slide(&slide("Hej", "", "", &no_cond)).triggers.is_empty());
    }

    #[test]
    fn utlösare_som_inte_kan_träffas_lämnar_målen_synliga() {
        let tgt = r#"<p:tgtEl><p:spTgt spid="4"/></p:tgtEl>"#;
        let mut t = Timeline::of_slide(&slide("Hej", "", "", &interactive(8, &fade_iter(tgt, "", 1000, "clickEffect"))));
        t.retain_triggers(|id| id == 99);
        assert!(t.triggers.is_empty() && t.targets().is_empty());
        let a = unit(4, None);
        assert!(!t.initially_hidden(a) && t.state(a, g(), 0..0, 0.0).visible);
    }

    #[test]
    fn iterate_bokstav_och_ord_med_fördröjning() {
        let tgt = r#"<p:tgtEl><p:spTgt spid="4"/></p:tgtEl>"#;
        // 12 tecken, steg 10 % av 500 ms = 50 ms: sista delen börjar efter 550 ms och är klar efter 1,05 s.
        let lt = fade_iter(tgt, r#"<p:iterate type="lt"><p:tmPct val="10000"/></p:iterate>"#, 500, "clickEffect");
        let t = Timeline::of_slide(&slide("Hej du glada", "", &click_of(&lt), ""));
        let e = &t.groups[0].effects[0];
        let it = e.iter.unwrap();
        assert_eq!((it.kind, it.n), (UnitKind::Letters, 12));
        assert!((it.step - 0.05).abs() < 1e-6);
        assert!((t.groups[0].dur - 1.05).abs() < 1e-5, "{}", t.groups[0].dur);
        let a = unit(4, None);
        assert_eq!(t.unit_scopes(), vec![UnitScope { shape: 4, para: None, kind: UnitKind::Letters }]);
        let st = |u: u32, tt: f32| t.state_unit(a, Some(u), g(), t.chain(1), tt);
        assert!(!st(0, -1.0).visible || st(0, -1.0).opacity < 0.01);
        assert!((st(0, 0.25).opacity - 0.5).abs() < 0.01);
        // Del 11 har inte börjat vid 0,25 s och är halvvägs vid 0,55 + 0,25 s.
        assert!(!st(11, 0.25).visible);
        assert!((st(11, 0.8).opacity - 0.5).abs() < 0.01);
        for u in 0..12 {
            let s = st(u, 1.05);
            assert!(s.visible && s.opacity > 0.999, "del {u}: {s:?}");
            assert!(!st(u, 0.0).visible || st(u, 0.0).opacity < 0.01 || u == 0);
        }
        // Delar bortom antalet följer den sista.
        assert!(!st(40, 0.5).visible);
        // Ord, tmAbs 200 ms: tre ord, andra ordet börjar efter 0,2 s.
        let wd = fade_iter(tgt, r#"<p:iterate type="wd"><p:tmAbs val="200"/></p:iterate>"#, 500, "clickEffect");
        let t = Timeline::of_slide(&slide("Hej du glada", "", &click_of(&wd), ""));
        let it = t.groups[0].effects[0].iter.unwrap();
        assert_eq!((it.kind, it.n), (UnitKind::Words, 3));
        assert!((t.groups[0].dur - 0.9).abs() < 1e-5);
        assert!(!t.state_unit(a, Some(1), g(), t.chain(1), 0.1).visible);
        assert!(t.state_unit(a, Some(1), g(), t.chain(1), 0.3).visible);
        // Bakifrån: del 0 börjar sist.
        let back = fade_iter(tgt, r#"<p:iterate type="wd" backwards="1"><p:tmAbs val="200"/></p:iterate>"#, 500, "clickEffect");
        let t = Timeline::of_slide(&slide("Hej du glada", "", &click_of(&back), ""));
        assert!(!t.state_unit(a, Some(0), g(), t.chain(1), 0.3).visible);
        assert!(t.state_unit(a, Some(2), g(), t.chain(1), 0.1).visible);
    }

    #[test]
    fn iterate_per_stycke_räknar_stycket() {
        let tgt = r#"<p:tgtEl><p:spTgt spid="4"><p:txEl><p:pRg st="1" end="1"/></p:txEl></p:spTgt></p:tgtEl>"#;
        let lt = fade_iter(tgt, r#"<p:iterate type="lt"><p:tmAbs val="100"/></p:iterate>"#, 500, "clickEffect");
        let t = Timeline::of_slide(&slide("Ett två tre|Fyra", "", &click_of(&lt), ""));
        let e = &t.groups[0].effects[0];
        assert_eq!(e.target, unit(4, Some(1)));
        assert_eq!(e.iter.unwrap().n, 4, "stycke 1 (Fyra) har fyra tecken");
        // Ett enda tecken delas inte.
        let one = fade_iter(r#"<p:tgtEl><p:spTgt spid="4"><p:txEl><p:pRg st="1" end="1"/></p:txEl></p:spTgt></p:tgtEl>"#, r#"<p:iterate type="lt"><p:tmAbs val="100"/></p:iterate>"#, 500, "clickEffect");
        let t = Timeline::of_slide(&slide("Ett två tre|F", "", &click_of(&one), ""));
        assert!(t.groups[0].effects[0].iter.is_none() && t.unit_scopes().is_empty());
    }

    #[test]
    fn teckenintervall_delas_som_bokstäver() {
        let tgt = r#"<p:tgtEl><p:spTgt spid="4"><p:txEl><p:charRg st="2" end="4"/></p:txEl></p:spTgt></p:tgtEl>"#;
        let e = fade_iter(tgt, r#"<p:iterate type="lt"><p:tmAbs val="100"/></p:iterate>"#, 500, "clickEffect");
        let t = Timeline::of_slide(&slide("Hej du glada", "", &click_of(&e), ""));
        assert_eq!(t.unit_scopes(), vec![UnitScope { shape: 4, para: None, kind: UnitKind::Letters }]);
        let a = unit(4, None);
        let st = |u: Option<u32>, tt: f32| t.state_unit(a, u, g(), t.chain(1), tt);
        // Tecken utanför intervallet och helheten påverkas inte.
        assert!(st(Some(0), 0.0).visible && st(Some(9), 0.0).visible && st(None, 0.0).visible);
        assert!(!t.initially_hidden(a));
        // Tecknen 2, 3, 4 kommer med 100 ms mellanrum.
        assert!(!st(Some(2), -0.0).visible || st(Some(2), 0.0).opacity < 0.01);
        assert!((st(Some(2), 0.25).opacity - 0.5).abs() < 0.01);
        assert!(!st(Some(4), 0.1).visible);
        assert!((st(Some(4), 0.45).opacity - 0.5).abs() < 0.01);
        assert!(st(Some(4), 0.7).opacity > 0.999);
        assert!((t.groups[0].dur - 0.7).abs() < 1e-5);
        // Utan iterate: alla tecken i intervallet samtidigt.
        let plain = fade_iter(tgt, "", 500, "clickEffect");
        let t = Timeline::of_slide(&slide("Hej du glada", "", &click_of(&plain), ""));
        assert!(!t.state_unit(a, Some(3), g(), t.chain(1), 0.0).visible || t.state_unit(a, Some(3), g(), t.chain(1), 0.0).opacity < 0.01);
        assert!(t.state_unit(a, Some(1), g(), t.chain(1), 0.0).visible);
    }

    #[test]
    fn mediakommandon_ur_tidsträdet() {
        let main = click_of(&cmd_effect(8, "playFrom(2.5)", 30503)) + &click_of(&(cmd_effect(8, "togglePause", 1) + &cmd_effect(9, "stop", 1)));
        let seqs = interactive(8, &cmd_effect(8, "togglePause", 1)) + &interactive(9, &(cmd_effect(9, "pause", 1) + &cmd_effect(9, "resume", 1)));
        let t = Timeline::of_slide(&slide("Hej", "", &main, &seqs));
        assert_eq!(t.clicks(), 2);
        assert!(t.targets().is_empty(), "kommandon ger inga animerade mål");
        let play = MediaCmd { shape_id: 8, kind: MediaKind::Play { from_s: 2.5 } };
        assert_eq!(t.chain_cmds(1), vec![(0.0, play)]);
        assert_eq!(
            t.chain_cmds(2),
            vec![(0.0, MediaCmd { shape_id: 8, kind: MediaKind::TogglePause }), (0.0, MediaCmd { shape_id: 9, kind: MediaKind::Stop })]
        );
        assert_eq!(t.groups[0].dur, 0.0, "kommandots längd räknas inte");
        assert_eq!(t.triggers.len(), 2);
        assert_eq!(t.triggers[0].cmds(), vec![(0.0, MediaCmd { shape_id: 8, kind: MediaKind::TogglePause })]);
        let kinds: Vec<MediaKind> = t.triggers[1].cmds().iter().map(|c| c.1.kind).collect();
        assert_eq!(kinds, vec![MediaKind::Pause, MediaKind::Resume]);
        assert!(t.chain_cmds(0).is_empty());
        // Kommandotyper som inte är anrop och okända kommandon ignoreras.
        let bad = click_of(&(cmd_effect(8, "gotoBookmark", 1) + &cmd_effect(8, "okänt(1)", 1) + &cmd_effect(8, "play", 1).replace(r#"type="call""#, r#"type="verb""#)));
        assert!(Timeline::of_slide(&slide("Hej", "", &bad, "")).chain_cmds(1).is_empty());
    }

    #[test]
    fn kommandonamn() {
        assert_eq!(parse_cmd("playFrom(0.0)"), Some(MediaKind::Play { from_s: 0.0 }));
        assert_eq!(parse_cmd("playFrom(12.5)"), Some(MediaKind::Play { from_s: 12.5 }));
        assert_eq!(parse_cmd("playFrom(-3)"), Some(MediaKind::Play { from_s: 0.0 }));
        assert_eq!(parse_cmd("play"), Some(MediaKind::Play { from_s: 0.0 }));
        assert_eq!(parse_cmd("togglePause"), Some(MediaKind::TogglePause));
        assert_eq!(parse_cmd("stop"), Some(MediaKind::Stop));
        assert_eq!(parse_cmd("pause"), Some(MediaKind::Pause));
        assert_eq!(parse_cmd("resume"), Some(MediaKind::Resume));
        assert_eq!(parse_cmd("gotoBookmark(x)"), None);
    }

    #[test]
    fn mediaformer_på_bilden() {
        let pic = |id: u32, file: &str| {
            format!(r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="M"/><p:cNvPicPr/><p:nvPr>{file}</p:nvPr></p:nvPicPr></p:pic>"#)
        };
        let extra = pic(8, r#"<a:videoFile r:link="rId1"/><p:extLst><p:ext uri="x"><p14:media r:embed="rId2"/></p:ext></p:extLst>"#)
            + &pic(9, r#"<a:audioFile r:link="rId3"/><p:extLst><p:ext uri="x"><p14:media r:embed="rId4"/></p:ext></p:extLst>"#)
            + &pic(10, "")
            + &pic(11, r#"<a:wavAudioFile r:embed="rId5" name="x.wav"/>"#);
        let t = Timeline::of_slide(&slide("Hej", &extra, "", ""));
        assert_eq!(t.media, vec![MediaShape { id: 8, video: true }, MediaShape { id: 9, video: false }, MediaShape { id: 11, video: false }]);
        assert!(media_shapes(&slide("Hej", "", "", "")).is_empty());
    }

    #[test]
    fn delarna_ritas_som_helhet_när_alla_har_samma_tillstånd() {
        let a = State::SHOWN;
        let b = State { opacity: 0.5, ..State::SHOWN };
        assert_eq!(uniform(&[a, a, State { opacity: 1.0 - 1e-7, ..a }]), Some(a));
        assert_eq!(uniform(&[a, b]), None);
        assert_eq!(uniform(&[]), None);
    }
}
