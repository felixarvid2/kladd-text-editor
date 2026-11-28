//! Layoutträdet: layoutdefinitionen (`layoutN.xml`, `dgm:layoutDef`) körd mot datamodellen
//! (ECMA-376 del 1 §21.4.2), med villkor (`choose`/`if`), upprepningar (`forEach`), villkor på
//! storlekar (`constrLst`) och algoritmerna som placerar barnen (`alg`).
//!
//! Avstånd är i punkter. Värden i layoutdefinitionen (`constr@val`) är i millimeter för längder och
//! i punkter för teckenstorlekar.

use std::collections::HashMap;

use super::super::xml::Node;
use super::data::{Kind, Model};

/// En rektangel (x, y, b, h) i punkter, i ramens koordinater.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn union(self, o: Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
}

const MM: f64 = 72.0 / 25.4;

/// En layoutnod i trädet (en instans av `dgm:layoutNode` för en datapunkt).
pub struct LNode<'a> {
    pub name: String,
    /// Datapunkten noden hör till.
    pub pt: usize,
    pub style_lbl: Option<String>,
    pub alg: Option<&'a Node>,
    pub shape: Option<&'a Node>,
    pub pres_of: Option<&'a Node>,
    constrs: Vec<&'a Node>,
    pub rules: Vec<&'a Node>,
    vars: Option<&'a Node>,
    pub kids: Vec<usize>,
    pub parent: Option<usize>,
    /// Presentationspunkten i datamodellen (formatering, `custScaleX` …).
    pub pres: Option<usize>,
    /// Värden ur villkoren (`w`, `h`, `l`, `primFontSz`, `lMarg` …).
    pub vals: HashMap<String, f64>,
    /// Grupper för teckenstorlekar med `op="equ"`: typ → (layoutnod, villkorets nummer).
    pub font_groups: HashMap<String, (usize, usize)>,
    /// Gränser ur villkor med `op="gte"`/`"lte"`.
    bounds: Vec<(String, bool, f64)>,
    /// Vad bredden och höjden räknades ur (`refType`): `w`/`h` → `w`, `h` eller `none`.
    size_ref: HashMap<String, String>,
    /// Värden som föräldrar och förfäder satt; nodens egna villkor ändrar dem inte.
    from_above: std::collections::HashSet<String>,
    pub rect: Rect,
}

pub struct Tree<'a> {
    pub nodes: Vec<LNode<'a>>,
    pub m: &'a Model<'a>,
    def: &'a Node,
    seen: HashMap<(usize, String), usize>,
    /// Kopplingar som brutna linjer: nod → punkter (ramens koordinater).
    pub paths: HashMap<usize, Vec<(f64, f64)>>,
    /// Böjda kopplingar som ritas som cirkelbågspilar.
    pub arcs: std::collections::HashSet<usize>,
    /// Faktor för teckenstorlekarna i villkoren (`primFontSz`/`secFontSz` med `val`): rutor som växer med
    /// texten (`rule type="h" val="INF"`) läggs ut för en gemensam storlek i taget.
    pub font_k: f64,
    /// Lägsta höjd per nod (rutor som vuxit med texten).
    pub grow: HashMap<usize, f64>,
    /// Krymp inte för att få plats (`lin` och helheten): storleken väljs i stället med `font_k`.
    pub no_shrink: bool,
}

fn alg_type<'a>(n: &LNode<'a>) -> &'a str {
    n.alg.and_then(|a| a.attr("type")).unwrap_or("composite")
}

pub fn param<'a>(n: &LNode<'a>, name: &str) -> Option<&'a str> {
    n.alg?.children_named("dgm:param").find(|p| p.attr("type") == Some(name)).and_then(|p| p.attr("val"))
}

fn param_f(n: &LNode<'_>, name: &str) -> Option<f64> {
    param(n, name).and_then(|v| v.parse().ok())
}

/// Längdtyper (millimeter i `val`).
fn is_length(t: &str) -> bool {
    matches!(
        t,
        "w" | "h" | "l" | "t" | "r" | "b" | "ctrX" | "ctrY" | "diam" | "sp" | "sibSp" | "secSibSp" | "connDist" | "begPad" | "endPad" | "stemThick" | "bendDist" | "lMarg" | "rMarg" | "tMarg" | "bMarg" | "begMarg" | "endMarg" | "lOff" | "tOff" | "rOff" | "bOff" | "wOff" | "hOff" | "ctrXOff" | "ctrYOff"
    )
}

impl<'a> Tree<'a> {
    /// Bygger trädet ur layoutdefinitionen. `None` om definitionen eller dokumentpunkten saknas.
    pub fn build(m: &'a Model<'a>, def: &'a Node) -> Option<Tree<'a>> {
        let root_def = def.child("dgm:layoutNode")?;
        let root_pt = m.root?;
        let mut t = Tree { nodes: Vec::new(), m, def, seen: HashMap::new(), paths: HashMap::new(), arcs: Default::default(), font_k: 1.0, grow: HashMap::new(), no_shrink: false };
        let id = t.new_node(root_def, root_pt, None);
        t.process(id, &root_def.children, root_pt, 1, 1, 0);
        Some(t)
    }

    fn new_node(&mut self, def: &'a Node, pt: usize, parent: Option<usize>) -> usize {
        let name = def.attr("name").unwrap_or("").to_owned();
        let nth = self.seen.entry((pt, name.clone())).or_insert(0);
        let pres = self.m.pres_point(pt, &name, *nth);
        *nth += 1;
        let id = self.nodes.len();
        self.nodes.push(LNode {
            name,
            pt,
            style_lbl: def.attr("styleLbl").map(str::to_owned),
            alg: None,
            shape: None,
            pres_of: None,
            constrs: Vec::new(),
            rules: Vec::new(),
            vars: None,
            kids: Vec::new(),
            parent,
            pres,
            vals: HashMap::new(),
            font_groups: HashMap::new(),
            bounds: Vec::new(),
            size_ref: HashMap::new(),
            from_above: Default::default(),
            rect: Rect::default(),
        });
        if let Some(p) = parent {
            self.nodes[p].kids.push(id);
        }
        id
    }

    fn find_for_each(&self, name: &str) -> Option<&'a Node> {
        fn find<'n>(n: &'n Node, name: &str) -> Option<&'n Node> {
            if n.name == "dgm:forEach" && n.attr("name") == Some(name) {
                return Some(n);
            }
            n.children.iter().find_map(|c| find(c, name))
        }
        find(self.def, name)
    }

    fn process(&mut self, ln: usize, elems: &'a [Node], pt: usize, pos: usize, cnt: usize, depth: usize) {
        if depth > 48 || self.nodes.len() > 4000 {
            return;
        }
        for e in elems {
            match e.name.as_str() {
                "dgm:alg" => self.nodes[ln].alg = Some(e),
                "dgm:shape" => self.nodes[ln].shape = Some(e),
                "dgm:presOf" => self.nodes[ln].pres_of = Some(e),
                "dgm:constrLst" => self.nodes[ln].constrs.extend(e.children_named("dgm:constr")),
                "dgm:ruleLst" => self.nodes[ln].rules.extend(e.children_named("dgm:rule")),
                "dgm:varLst" => self.nodes[ln].vars = Some(e),
                "dgm:layoutNode" => {
                    let k = self.new_node(e, pt, Some(ln));
                    self.process(k, &e.children, pt, pos, cnt, depth + 1);
                }
                "dgm:forEach" => {
                    let fe = match e.attr("ref") {
                        Some(r) => self.find_for_each(r).unwrap_or(e),
                        None => e,
                    };
                    let sel = self.m.select(pt, fe);
                    let n = sel.len();
                    for (i, p) in sel.into_iter().enumerate() {
                        self.process(ln, &fe.children, p, i + 1, n, depth + 1);
                    }
                }
                "dgm:choose" => {
                    for c in &e.children {
                        let take = match c.name.as_str() {
                            "dgm:if" => self.cond(ln, c, pt, pos, cnt),
                            "dgm:else" => true,
                            _ => false,
                        };
                        if take {
                            self.process(ln, &c.children, pt, pos, cnt, depth + 1);
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// En layoutvariabel (`dir`, `hierBranch` …): presentationspunktens `presLayoutVars` på noden
    /// eller dess förfäder, annars layoutnodens `varLst`, annars standardvärdet.
    pub fn var(&self, ln: usize, name: &str) -> String {
        let key = format!("dgm:{name}");
        let mut cur = Some(ln);
        while let Some(i) = cur {
            let n = &self.nodes[i];
            let from_pres = n.pres.and_then(|p| self.m.prset(p)).and_then(|s| s.child("dgm:presLayoutVars")).and_then(|v| v.child(&key)).and_then(|v| v.attr("val"));
            if let Some(v) = from_pres.or_else(|| n.vars.and_then(|v| v.child(&key)).and_then(|v| v.attr("val"))) {
                return v.to_owned();
            }
            cur = n.parent;
        }
        match name {
            "dir" => "norm",
            "hierBranch" => "std",
            "chMax" | "chPref" => "-1",
            "resizeHandles" => "rel",
            "animLvl" | "animOne" => "none",
            _ => "0",
        }
        .to_owned()
    }

    fn cond(&self, ln: usize, e: &Node, pt: usize, pos: usize, cnt: usize) -> bool {
        let m = self.m;
        let lhs: String = match e.attr("func").unwrap_or("") {
            "cnt" => m.select(pt, e).len().to_string(),
            "pos" => pos.to_string(),
            "revPos" => (cnt + 1 - pos.min(cnt)).to_string(),
            "posEven" => ((pos % 2 == 0) as u8).to_string(),
            "posOdd" => ((pos % 2 == 1) as u8).to_string(),
            "depth" => m.depth(pt).to_string(),
            "maxDepth" => {
                let base = m.depth(pt);
                let mut d = 0;
                for q in m.axis(pt, "des", true) {
                    if m.is_type(q, "node") {
                        d = d.max(m.depth(q) - base);
                    }
                }
                d.to_string()
            }
            "var" => self.var(ln, e.attr("arg").unwrap_or("")),
            _ => return false,
        };
        let rhs = e.attr("val").unwrap_or("");
        let num = |s: &str| match s {
            "true" => Some(1.0),
            "false" => Some(0.0),
            _ => s.parse::<f64>().ok(),
        };
        let op = e.attr("op").unwrap_or("equ");
        match (num(&lhs), num(rhs)) {
            (Some(a), Some(b)) => match op {
                "equ" => (a - b).abs() < 1e-9,
                "neq" => (a - b).abs() >= 1e-9,
                "gt" => a > b,
                "lt" => a < b,
                "gte" => a >= b,
                "lte" => a <= b,
                _ => false,
            },
            _ => match op {
                "equ" => lhs == rhs,
                "neq" => lhs != rhs,
                _ => false,
            },
        }
    }

    // ---- Villkor -------------------------------------------------------------------------------

    fn matches(&self, id: usize, name: Option<&str>, pt_type: Option<&str>) -> bool {
        let n = &self.nodes[id];
        name.is_none_or(|f| f.is_empty() || n.name == f) && pt_type.is_none_or(|t| self.m.is_type(n.pt, t))
    }

    fn descendants(&self, id: usize, out: &mut Vec<usize>) {
        for &k in &self.nodes[id].kids {
            out.push(k);
            self.descendants(k, out);
        }
    }

    fn targets(&self, id: usize, rel: &str, name: Option<&str>, pt_type: Option<&str>) -> Vec<usize> {
        match rel {
            "ch" => self.nodes[id].kids.iter().copied().filter(|k| self.matches(*k, name, pt_type)).collect(),
            "des" => {
                let mut v = Vec::new();
                self.descendants(id, &mut v);
                v.retain(|k| self.matches(*k, name, pt_type));
                v
            }
            _ => vec![id],
        }
    }

    /// Ett värde på en nod: villkorets värde, annars storleken den fått.
    fn value(&self, id: usize, ty: &str) -> Option<f64> {
        let n = &self.nodes[id];
        if let Some(v) = n.vals.get(ty) {
            return Some(*v);
        }
        match ty {
            "w" => Some(n.rect.w),
            "h" => Some(n.rect.h),
            "l" => Some(0.0),
            "t" => Some(0.0),
            "r" => Some(n.vals.get("l").copied().unwrap_or(0.0) + n.vals.get("w").copied().unwrap_or(n.rect.w)),
            "b" => Some(n.vals.get("t").copied().unwrap_or(0.0) + n.vals.get("h").copied().unwrap_or(n.rect.h)),
            "ctrX" => Some(n.vals.get("l").copied().unwrap_or(0.0) + n.vals.get("w").copied().unwrap_or(n.rect.w) / 2.0),
            "ctrY" => Some(n.vals.get("t").copied().unwrap_or(0.0) + n.vals.get("h").copied().unwrap_or(n.rect.h) / 2.0),
            _ => None,
        }
    }

    /// Tillämpar nodens villkor på den själv, barnen och ättlingarna.
    fn apply_constraints(&mut self, id: usize) {
        // Två varv: ett villkor kan hänvisa till ett värde som ett senare villkor sätter.
        for _ in 0..2 {
            self.apply_constraints_once(id);
        }
    }

    fn apply_constraints_once(&mut self, id: usize) {
        let constrs = self.nodes[id].constrs.clone();
        for (ci, c) in constrs.iter().enumerate() {
            let Some(ty) = c.attr("type") else { continue };
            if ty == "none" {
                continue;
            }
            let targets = self.targets(id, c.attr("for").unwrap_or("self"), c.attr("forName"), c.attr("ptType"));
            if targets.is_empty() {
                continue;
            }
            let fact = c.attr_f64("fact").unwrap_or(1.0);
            let ref_type = c.attr("refType").unwrap_or("none");
            // Marginaler i förhållande till teckenstorleken räknas om när texten fått sin storlek.
            if ty.ends_with("Marg") && ref_type.ends_with("FontSz") {
                for t in self.targets(id, c.attr("for").unwrap_or("self"), c.attr("forName"), c.attr("ptType")) {
                    self.nodes[t].vals.insert(format!("{ty}%"), fact);
                }
            }
            let value = match ref_type {
                "none" => {
                    let v = c.attr_f64("val").unwrap_or(0.0);
                    if is_length(ty) {
                        v * MM
                    } else if ty.ends_with("FontSz") {
                        v * self.font_k
                    } else {
                        v
                    }
                }
                rt => {
                    let rel = c.attr("refFor").unwrap_or("self");
                    let refs = self.targets(id, rel, c.attr("refForName"), c.attr("refPtType"));
                    let Some(v) = refs.first().and_then(|r| self.value(*r, rt)) else { continue };
                    // En längd ur en teckenstorlek: storleken räknas som millimeter. En marginal ur en
                    // längd: längden räknas i millimeter och marginalen i punkter (`vList2`:s
                    // `lMarg = 0,09 w` ger 16,3 pt för en ram på 513 pt, som PowerPoint).
                    if is_length(ty) && rt.ends_with("FontSz") {
                        v * fact * MM
                    } else if ty.ends_with("Marg") && is_length(rt) {
                        v / MM * fact
                    } else {
                        v * fact
                    }
                }
            };
            let op = c.attr("op").unwrap_or("none");
            // `op="equ"` utan värde gör bara värdena lika (grupp), sätter inget.
            if op != "none" && ref_type == "none" && c.attr("val").is_none() {
                // Teckenstorlekar och höjder/bredder (rutor som växer med texten blir lika stora).
                if ty.ends_with("FontSz") || ty == "h" || ty == "w" {
                    for t in &targets {
                        self.nodes[*t].font_groups.insert(ty.to_owned(), (id, ci));
                    }
                }
                continue;
            }
            if !value.is_finite() {
                continue;
            }
            for t in targets {
                let n = &mut self.nodes[t];
                // Nodens egna villkor är standardvärden: förälderns villkor för den går före.
                if t == id && n.from_above.contains(ty) {
                    continue;
                }
                if t != id {
                    n.from_above.insert(ty.to_owned());
                }
                match op {
                    "gte" => n.bounds.push((ty.to_owned(), true, value)),
                    "lte" => n.bounds.push((ty.to_owned(), false, value)),
                    _ => {
                        if ty == "w" || ty == "h" {
                            n.size_ref.insert(ty.to_owned(), ref_type.to_owned());
                        }
                        n.vals.insert(ty.to_owned(), value);
                        if op == "equ" && ty.ends_with("FontSz") {
                            n.font_groups.insert(ty.to_owned(), (id, ci));
                        }
                    }
                }
            }
        }
    }

    /// Reglerna (`ruleLst`) som gäller noden: dess egna (`for="self"`), förälderns för barnen och
    /// förfädernas för ättlingarna, med `forName`/`ptType`.
    pub fn rules_for(&self, id: usize) -> Vec<&'a Node> {
        let mut out: Vec<&'a Node> = self.nodes[id].rules.iter().copied().filter(|r| r.attr("for").unwrap_or("self") == "self").collect();
        let mut cur = self.nodes[id].parent;
        let mut first = true;
        while let Some(p) = cur {
            for r in &self.nodes[p].rules {
                let ok = match r.attr("for").unwrap_or("self") {
                    "ch" => first,
                    "des" => true,
                    _ => false,
                };
                if ok && self.matches(id, r.attr("forName"), r.attr("ptType")) {
                    out.push(r);
                }
            }
            first = false;
            cur = self.nodes[p].parent;
        }
        out
    }

    /// Värdet med gränserna (`gte`/`lte`).
    pub fn bounded(&self, id: usize, ty: &str) -> Option<f64> {
        let n = &self.nodes[id];
        let mut v = *n.vals.get(ty)?;
        for (t, lower, b) in &n.bounds {
            if t == ty {
                v = if *lower { v.max(*b) } else { v.min(*b) };
            }
        }
        Some(v)
    }

    // ---- Placering -------------------------------------------------------------------------------

    fn cust(&self, id: usize, name: &str) -> Option<f64> {
        let p = self.nodes[id].pres?;
        self.m.prset(p)?.attr_f64(name).map(|v| v / 100_000.0)
    }

    fn shift(&mut self, id: usize, dx: f64, dy: f64) {
        self.nodes[id].rect.x += dx;
        self.nodes[id].rect.y += dy;
        for k in self.nodes[id].kids.clone() {
            self.shift(k, dx, dy);
        }
    }

    /// Utbredningen av det som ritas under noden (barnens utbredning; utan barn noden själv).
    pub fn extent(&self, id: usize) -> Rect {
        let n = &self.nodes[id];
        let mut r: Option<Rect> = None;
        for &k in &n.kids {
            let e = self.extent(k);
            r = Some(match r {
                Some(r) => r.union(e),
                None => e,
            });
        }
        // Utan barn: rutan om noden ritas eller är ett mellanrum, annars ingenting.
        r.unwrap_or_else(|| if n.kids.is_empty() && !self.draws(id) && !matches!(alg_type(n), "sp" | "tx" | "conn") { Rect { w: 0.0, h: 0.0, ..n.rect } } else { n.rect })
    }

    fn is_space(&self, id: usize) -> bool {
        alg_type(&self.nodes[id]) == "sp" && self.nodes[id].kids.is_empty()
    }

    /// Lägger ut noden i rektangeln (förälderns koordinater) och sedan dess barn.
    pub fn layout(&mut self, id: usize, mut r: Rect, depth: usize) {
        if depth > 40 {
            return;
        }
        // Egen skalning ur PowerPoint (`custScaleX`/`custScaleY`) runt rutans mitt.
        if let Some(sx) = self.cust(id, "custScaleX").filter(|v| *v > 0.0) {
            r.x += r.w * (1.0 - sx) / 2.0;
            r.w *= sx;
        }
        if let Some(sy) = self.cust(id, "custScaleY").filter(|v| *v > 0.0) {
            r.y += r.h * (1.0 - sy) / 2.0;
            r.h *= sy;
        }
        // En ruta som vuxit med sin text växer nedåt.
        if let Some(g) = self.grow.get(&id) {
            r.h = r.h.max(*g);
        }
        let alg = alg_type(&self.nodes[id]);
        if alg == "composite" {
            if let Some(ar) = param_f(&self.nodes[id], "ar").filter(|a| *a > 0.0) {
                if r.h > 0.0 && r.w / r.h > ar {
                    let nw = r.h * ar;
                    r.x += (r.w - nw) / 2.0;
                    r.w = nw;
                } else if ar > 0.0 {
                    let nh = r.w / ar;
                    r.y += (r.h - nh) / 2.0;
                    r.h = nh;
                }
            }
        }
        self.nodes[id].rect = r;
        self.nodes[id].vals.insert("w".into(), r.w);
        self.nodes[id].vals.insert("h".into(), r.h);
        self.apply_constraints(id);
        match alg {
            "lin" => self.lin(id, depth),
            "snake" => self.snake(id, depth),
            "cycle" => self.cycle(id, depth),
            "pyra" => self.pyra(id, depth),
            "hierChild" => self.hier_child(id, depth),
            "hierRoot" => self.hier_root(id, depth),
            "tx" | "sp" | "conn" => {
                // Löv; barn (sällsynt) läggs i samma ruta.
                for k in self.nodes[id].kids.clone() {
                    self.layout(k, r, depth + 1);
                }
            }
            _ => self.composite(id, depth),
        }
    }

    /// Barnets storlek ur villkoren, annars `def`.
    fn size_of(&self, k: usize, def: (f64, f64)) -> (f64, f64) {
        let v = &self.nodes[k].vals;
        let w = v.get("w").copied().or_else(|| Some(v.get("r")? - v.get("l")?)).unwrap_or(def.0);
        let h = v.get("h").copied().or_else(|| Some(v.get("b")? - v.get("t")?)).unwrap_or(def.1);
        (w.max(0.0), h.max(0.0))
    }

    fn composite(&mut self, id: usize, depth: usize) {
        let r = self.nodes[id].rect;
        for k in self.nodes[id].kids.clone() {
            let (w, h) = self.size_of(k, (r.w, r.h));
            let v = &self.nodes[k].vals;
            let g = |n: &str| v.get(n).copied();
            let mut x = g("l").or_else(|| Some(g("r")? - w)).or_else(|| Some(g("ctrX")? - w / 2.0)).unwrap_or(0.0);
            let mut y = g("t").or_else(|| Some(g("b")? - h)).or_else(|| Some(g("ctrY")? - h / 2.0)).unwrap_or(0.0);
            x += g("lOff").unwrap_or(0.0) + g("ctrXOff").unwrap_or(0.0);
            y += g("tOff").unwrap_or(0.0) + g("ctrYOff").unwrap_or(0.0);
            let w = w + g("wOff").unwrap_or(0.0);
            let h = h + g("hOff").unwrap_or(0.0);
            self.layout(k, Rect { x: r.x + x, y: r.y + y, w, h }, depth + 1);
        }
        self.curves(id);
    }

    /// Böjda kopplingar (`connRout="curve"`, t.ex. kugghjulens pilar): en cirkelbågspil runt formen
    /// som källnoden hör till (`gear1srcNode` → `gear1`), med diametern `diam` (negativ = moturs,
    /// `leftCircularArrow`), från källans till målets vinkel. Tjockleken är kopplingens `h`.
    fn curves(&mut self, id: usize) {
        let kids = self.nodes[id].kids.clone();
        let rect_of = |t: &Tree<'_>, name: &str| kids.iter().find(|k| t.nodes[**k].name == name).map(|k| t.nodes[*k].rect);
        for &c in &kids {
            let n = &self.nodes[c];
            if !(self.is_conn(c) && param(n, "connRout") == Some("curve")) {
                continue;
            }
            let (Some(src), Some(dst)) = (param(n, "srcNode"), param(n, "dstNode")) else { continue };
            let (Some(s), Some(d)) = (rect_of(self, src), rect_of(self, dst)) else { continue };
            // Runt formen källnoden hör till (`gear1srcNode` → `gear1`). Andra böjda kopplingar (t.ex.
            // `cycle8`:s pilar mellan hjälpnoder) ritas inte än.
            let Some(around) = src.strip_suffix("srcNode").filter(|b| !b.is_empty()).and_then(|b| rect_of(self, b)) else { continue };
            let Some(diam) = n.vals.get("diam").copied().filter(|d| d.abs() > 0.0) else { continue };
            let thick = n.vals.get("h").copied().unwrap_or(diam.abs() * 0.05).abs();
            let (cx, cy) = (around.x + around.w / 2.0, around.y + around.h / 2.0);
            let ang = |r: Rect| ((r.y + r.h / 2.0 - cy).atan2(r.x + r.w / 2.0 - cx).to_degrees()).rem_euclid(360.0);
            let size = diam.abs();
            let v = &mut self.nodes[c].vals;
            v.insert("kladdArcSt".into(), ang(s));
            v.insert("kladdArcEnd".into(), ang(d));
            v.insert("kladdArcW".into(), (thick / size).min(0.25));
            v.insert("kladdArcLeft".into(), if diam < 0.0 { 1.0 } else { 0.0 });
            self.nodes[c].rect = Rect { x: cx - size / 2.0, y: cy - size / 2.0, w: size, h: size };
            self.arcs.insert(c);
        }
    }

    /// Lägger ut barnen i rutor av storleken `sizes × s` (från origo) och ger deras utbredningar.
    fn lay_kids(&mut self, kids: &[usize], sizes: &[(f64, f64)], s: f64, horiz: bool, avail_c: f64, depth: usize) -> Vec<Rect> {
        kids.iter()
            .zip(sizes)
            .map(|(&k, &(w, h))| {
                // Krympningen gäller flödesriktningen; tvärs över bara om storleken räknats ur den
                // andra (t.ex. `h` ur `w`), annars högst det som finns.
                let (p, c) = if horiz { ("w", "h") } else { ("h", "w") };
                let cross_follows = self.nodes[k].size_ref.get(c).is_some_and(|r| r == p);
                let cs = if horiz { h } else { w };
                let cross = if cross_follows { cs * s } else { cs.min(avail_c.max(0.0)) };
                let (w, h) = if horiz { (w * s, cross) } else { (cross, h * s) };
                self.layout(k, Rect { x: 0.0, y: 0.0, w, h }, depth + 1);
                if self.is_space(k) || alg_type(&self.nodes[k]) == "conn" { self.nodes[k].rect } else { self.extent(k) }
            })
            .collect()
    }

    /// Linjärt flöde (`lin`): barnen efter varandra längs `linDir`, ihop centrerade längs flödet och
    /// justerade tvärs över (`nodeHorzAlign`/`nodeVertAlign`). Får de inte plats krymps alla lika.
    fn lin(&mut self, id: usize, depth: usize) {
        let n = &self.nodes[id];
        let r = n.rect;
        let dir = param(n, "linDir").unwrap_or("fromL");
        let horiz = matches!(dir, "fromL" | "fromR");
        let rev = matches!(dir, "fromR" | "fromB");
        let cross_align = if horiz { param(n, "nodeVertAlign").unwrap_or("mid") } else { param(n, "nodeHorzAlign").unwrap_or("ctr") };
        let kids = n.kids.clone();
        let sizes: Vec<(f64, f64)> = kids.iter().map(|&k| self.size_of(k, if self.is_space(k) { (0.0, 0.0) } else { (r.w, r.h) })).collect();
        let (avail_p, avail_c) = if horiz { (r.w, r.h) } else { (r.h, r.w) };
        let along = |e: &Rect| if horiz { e.w } else { e.h };
        let across = |e: &Rect| if horiz { e.h } else { e.w };
        let mut ext = self.lay_kids(&kids, &sizes, 1.0, horiz, avail_c, depth);
        let total: f64 = ext.iter().map(along).sum();
        let cross = ext.iter().map(across).fold(0.0, f64::max);
        let mut s = 1.0f64;
        if total > avail_p && total > 0.0 && !self.no_shrink {
            s = s.min(avail_p / total);
        }
        let _ = cross;
        if s < 0.9999 {
            ext = self.lay_kids(&kids, &sizes, s, horiz, avail_c, depth);
        }
        let total: f64 = ext.iter().map(along).sum();
        let mut cursor = (avail_p - total) / 2.0;
        let order: Vec<usize> = if rev { (0..kids.len()).rev().collect() } else { (0..kids.len()).collect() };
        for &i in &order {
            let e = ext[i];
            let k = kids[i];
            // Längs flödet efter utbredningen, tvärs över efter barnets ruta.
            let slot = self.nodes[k].rect;
            let (a, c) = (along(&e), across(&slot));
            let off_c = match cross_align {
                "t" | "l" => 0.0,
                "b" | "r" => avail_c - c,
                _ => (avail_c - c) / 2.0,
            };
            let (dx, dy) = if horiz { (r.x + cursor - e.x, r.y + off_c - slot.y) } else { (r.x + off_c - slot.x, r.y + cursor - e.y) };
            self.shift(k, dx, dy);
            cursor += a;
        }
        // Egna förskjutningar (`custLinFactX/Y` × egen storlek, `custLinFactNeighborX/Y` × grannens).
        for i in 0..kids.len() {
            let k = kids[i];
            let nb = if i > 0 { i - 1 } else { i + 1 };
            let sc = |(w, h): (f64, f64)| if horiz { (w * s, h) } else { (w, h * s) };
            let nsz = sizes.get(nb).copied().map(sc).unwrap_or((0.0, 0.0));
            let own = sc(sizes[i]);
            let dx = self.cust(k, "custLinFactX").unwrap_or(0.0) * own.0 + self.cust(k, "custLinFactNeighborX").unwrap_or(0.0) * nsz.0;
            let dy = self.cust(k, "custLinFactY").unwrap_or(0.0) * own.1 + self.cust(k, "custLinFactNeighborY").unwrap_or(0.0) * nsz.1;
            if dx != 0.0 || dy != 0.0 {
                self.shift(k, dx, dy);
            }
        }
    }

    /// Orm (`snake`): barnen i rader (eller kolumner) med det antal per rad som ger störst storlek.
    fn snake(&mut self, id: usize, depth: usize) {
        let n = &self.nodes[id];
        let r = n.rect;
        let by_col = param(n, "flowDir") == Some("col");
        let gr = param(n, "grDir").unwrap_or("tL");
        let kids = n.kids.clone();
        let items: Vec<usize> = kids.iter().copied().filter(|k| !self.is_space(*k) && alg_type(&self.nodes[*k]) != "conn").collect();
        if items.is_empty() {
            return;
        }
        let gap_kid = kids.iter().copied().find(|k| self.is_space(*k));
        let (w, h) = self.size_of(items[0], (r.w, r.h));
        let gap_w = gap_kid.map(|g| self.size_of(g, (0.0, 0.0)).0).or_else(|| self.nodes[id].vals.get("sibSp").copied()).unwrap_or(0.0);
        let gap_h = self.nodes[id].vals.get("sp").copied().or_else(|| gap_kid.map(|g| self.size_of(g, (0.0, 0.0)).1)).unwrap_or(gap_w);
        let n_items = items.len();
        let mut best = (1usize, 0.0f64);
        for per in 1..=n_items {
            let lines = n_items.div_ceil(per);
            let (cols, rows) = if by_col { (lines, per) } else { (per, lines) };
            let need_w = cols as f64 * w + (cols as f64 - 1.0) * gap_w;
            let need_h = rows as f64 * h + (rows as f64 - 1.0) * gap_h;
            let s = (r.w / need_w.max(1e-6)).min(r.h / need_h.max(1e-6)).min(1.0);
            if s > best.1 + 1e-9 {
                best = (per, s);
            }
        }
        let (per, s) = best;
        let lines = n_items.div_ceil(per);
        let (cols, rows) = if by_col { (lines, per) } else { (per, lines) };
        let (cw, ch, gw, gh) = (w * s, h * s, gap_w * s, gap_h * s);
        let grid_w = cols as f64 * cw + (cols as f64 - 1.0) * gw;
        let grid_h = rows as f64 * ch + (rows as f64 - 1.0) * gh;
        let (ox, oy) = (r.x + (r.w - grid_w) / 2.0, r.y + (r.h - grid_h) / 2.0);
        let from_right = gr.ends_with('R');
        let from_bottom = gr.starts_with('b');
        for (i, &k) in items.iter().enumerate() {
            let (line, idx) = (i / per, i % per);
            let in_line = if line + 1 == lines { n_items - line * per } else { per };
            // Den sista, ofullständiga raden centreras.
            let pad = (per - in_line) as f64 / 2.0;
            let (mut col, mut row) = if by_col { (line as f64, idx as f64 + pad) } else { (idx as f64 + pad, line as f64) };
            if from_right {
                col = cols as f64 - 1.0 - col;
            }
            if from_bottom {
                row = rows as f64 - 1.0 - row;
            }
            let rect = Rect { x: ox + col * (cw + gw), y: oy + row * (ch + gh), w: cw, h: ch };
            self.layout(k, rect, depth + 1);
        }
        // Mellanrum och kopplingar mellan två noder i samma rad.
        for (j, &k) in kids.iter().enumerate() {
            if items.contains(&k) {
                continue;
            }
            let prev = kids[..j].iter().rev().find(|x| items.contains(x)).copied();
            let next = kids[j + 1..].iter().find(|x| items.contains(x)).copied();
            let rect = match (prev, next) {
                (Some(a), Some(b)) => {
                    let (ra, rb) = (self.nodes[a].rect, self.nodes[b].rect);
                    if (ra.y - rb.y).abs() < 1e-6 && !by_col {
                        let (x0, x1) = if ra.x < rb.x { (ra.x + ra.w, rb.x) } else { (rb.x + rb.w, ra.x) };
                        Rect { x: x0, y: ra.y, w: (x1 - x0).max(0.0), h: ra.h }
                    } else {
                        let (y0, y1) = if ra.y < rb.y { (ra.y + ra.h, rb.y) } else { (rb.y + rb.h, ra.y) };
                        Rect { x: ra.x, y: y0, w: ra.w, h: (y1 - y0).max(0.0) }
                    }
                }
                _ => Rect::default(),
            };
            self.layout(k, rect, depth + 1);
        }
    }

    /// Cykel (`cycle`): noderna på en cirkel med lika vinkelsteg från `stAng` (0 = överst, medurs).
    fn cycle(&mut self, id: usize, depth: usize) {
        let n = &self.nodes[id];
        let r = n.rect;
        let st = param_f(n, "stAng").unwrap_or(0.0);
        let span = param_f(n, "spanAng").unwrap_or(360.0);
        let ctr_first = param(n, "ctrShpMap") == Some("fNode");
        let kids = n.kids.clone();
        let mut items: Vec<usize> = kids.iter().copied().filter(|k| !self.is_space(*k) && alg_type(&self.nodes[*k]) != "conn").collect();
        if items.is_empty() {
            return;
        }
        let center = if ctr_first { Some(items.remove(0)) } else { None };
        let cnt = items.len().max(1);
        let (w, h) = items.first().map(|k| self.size_of(*k, (r.w / 3.0, r.h / 3.0))).unwrap_or((0.0, 0.0));
        let sp = self.nodes[id].vals.get("sibSp").copied().unwrap_or(0.0);
        let step = if (span - 360.0).abs() < 1e-6 || cnt == 1 { span / cnt as f64 } else { span / (cnt as f64 - 1.0) };
        let size = w.max(h);
        let rad = match self.nodes[id].vals.get("diam") {
            Some(d) if *d > 0.0 => d / 2.0,
            _ if cnt > 1 => (size + sp) / (2.0 * (step.abs().min(180.0).to_radians() / 2.0).sin().max(0.05)),
            _ => 0.0,
        };
        let cw = center.map(|c| self.size_of(c, (w, h))).unwrap_or((0.0, 0.0));
        let total_w = (2.0 * rad + w).max(cw.0);
        let total_h = (2.0 * rad + h).max(cw.1);
        let s = (r.w / total_w.max(1e-6)).min(r.h / total_h.max(1e-6)).min(1.0);
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let mut angles = Vec::new();
        for (i, &k) in items.iter().enumerate() {
            let a = (st + step * i as f64).to_radians();
            angles.push(a);
            let (px, py) = (cx + rad * s * a.sin(), cy - rad * s * a.cos());
            self.layout(k, Rect { x: px - w * s / 2.0, y: py - h * s / 2.0, w: w * s, h: h * s }, depth + 1);
        }
        if let Some(c) = center {
            self.layout(c, Rect { x: cx - cw.0 * s / 2.0, y: cy - cw.1 * s / 2.0, w: cw.0 * s, h: cw.1 * s }, depth + 1);
        }
        // Kopplingar mitt emellan två noder, vridna längs cirkeln.
        let mut ci = 0;
        for &k in &kids {
            if items.contains(&k) || Some(k) == center {
                continue;
            }
            if alg_type(&self.nodes[k]) != "conn" || angles.is_empty() {
                self.layout(k, Rect::default(), depth + 1);
                continue;
            }
            let a = angles[ci % angles.len()] + step.to_radians() / 2.0;
            ci += 1;
            let (kw, kh) = self.size_of(k, (size * 0.3, size * 0.3));
            let (px, py) = (cx + rad * s * a.sin(), cy - rad * s * a.cos());
            self.layout(k, Rect { x: px - kw * s / 2.0, y: py - kh * s / 2.0, w: kw * s, h: kh * s }, depth + 1);
            self.nodes[k].vals.insert("kladdRot".into(), a.to_degrees() + 90.0);
        }
    }

    /// Pyramid (`pyra`): nivåerna staplade i flödesriktningen, var och en en trapets vars sidor
    /// följer pyramidens. `linDir="fromT"` ger den omvända pyramiden (bredast överst, trapetsen
    /// vänd 180°). Nivåns form (`pyraLvlNode`) får nivåns ruta och dess text samma mitt; sidofälten
    /// (`pyraAcctBkgdNode`/`pyraAcctTxNode`) ritas bara när `pyraAcctRatio` > 0.
    fn pyra(&mut self, id: usize, depth: usize) {
        let n = &self.nodes[id];
        let r = n.rect;
        let inverted = param(n, "linDir") == Some("fromT");
        let lvl_name = param(n, "pyraLvlNode").unwrap_or("level").to_owned();
        let acct = [param(n, "pyraAcctBkgdNode").unwrap_or("acctBkgd").to_owned(), param(n, "pyraAcctTxNode").unwrap_or("acctTx").to_owned()];
        let acct_ratio = self.nodes[id].vals.get("pyraAcctRatio").copied().unwrap_or(0.0);
        let kids = n.kids.clone();
        let cnt = kids.len().max(1);
        let lh = r.h / cnt as f64;
        let inset = r.w / (2.0 * cnt as f64);
        for (i, &k) in kids.iter().enumerate() {
            let frac = if inverted { (cnt - i) as f64 } else { (i + 1) as f64 } / cnt as f64;
            let w = r.w * frac;
            let rect = Rect { x: r.x + (r.w - w) / 2.0, y: r.y + lh * i as f64, w, h: lh };
            self.layout(k, rect, depth + 1);
            let parts: Vec<usize> = if self.nodes[k].name == lvl_name { vec![k] } else { self.nodes[k].kids.clone() };
            for c in parts {
                let name = self.nodes[c].name.clone();
                if name == lvl_name {
                    self.nodes[c].rect = rect;
                    self.nodes[c].vals.insert("kladdPyra".into(), inset);
                    if inverted {
                        self.nodes[c].vals.insert("kladdRot".into(), 180.0);
                    }
                } else if acct.contains(&name) && acct_ratio <= 0.0 {
                    self.nodes[c].rect = Rect::default();
                } else if !acct.contains(&name) {
                    let fact = self.ref_fact(k, &name, "w", &lvl_name).unwrap_or(1.0);
                    let tw = w * fact;
                    self.nodes[c].rect = Rect { x: rect.x + (w - tw) / 2.0, y: rect.y, w: tw, h: lh };
                }
            }
        }
    }

    /// Faktorn i villkoret `ty` för barnet `name` som hänvisar till syskonet `ref_name` (på `id`).
    fn ref_fact(&self, id: usize, name: &str, ty: &str, ref_name: &str) -> Option<f64> {
        self.nodes[id]
            .constrs
            .iter()
            .find(|c| c.attr("type") == Some(ty) && c.attr("forName") == Some(name) && c.attr("refForName") == Some(ref_name))
            .map(|c| c.attr_f64("fact").unwrap_or(1.0))
    }

    /// Lägger ut barnet från origo och ger dess utbredning.
    fn lay_alone(&mut self, k: usize, def: (f64, f64), depth: usize) -> Rect {
        let (w, h) = self.size_of(k, def);
        self.layout(k, Rect { x: 0.0, y: 0.0, w, h }, depth + 1);
        self.extent(k)
    }

    fn is_conn(&self, id: usize) -> bool {
        alg_type(&self.nodes[id]) == "conn"
    }

    /// Hierarkins barn (`hierChild`): undernoderna efter varandra längs `linDir` (standard `fromL`)
    /// med `sibSp` emellan (`secSibSp` lodrätt), justerade tvärs över efter `chAlign`. Kopplingarna
    /// läggs efteråt (`connect`).
    fn hier_child(&mut self, id: usize, depth: usize) {
        let n = &self.nodes[id];
        let r = n.rect;
        let dir = param(n, "linDir").unwrap_or("fromL");
        let vertical = matches!(dir, "fromT" | "fromB");
        let rev = matches!(dir, "fromR" | "fromB");
        let align = param(n, "chAlign").unwrap_or(if vertical { "l" } else { "t" });
        let kids: Vec<usize> = n.kids.iter().copied().filter(|k| !self.is_conn(*k)).collect();
        let v = &self.nodes[id].vals;
        let gap = if vertical { v.get("secSibSp").or(v.get("sibSp")) } else { v.get("sibSp") }.copied().unwrap_or(0.0);
        let ext: Vec<Rect> = kids.iter().map(|&k| self.lay_alone(k, (r.w, r.h), depth)).collect();
        let cross = ext.iter().map(|e| if vertical { e.w } else { e.h }).fold(0.0, f64::max);
        let order: Vec<usize> = if rev { (0..kids.len()).rev().collect() } else { (0..kids.len()).collect() };
        let mut cursor = 0.0;
        for i in order {
            let e = ext[i];
            let (a, c) = if vertical { (e.h, e.w) } else { (e.w, e.h) };
            let off = match align {
                "r" | "b" => cross - c,
                "ctr" | "mid" => (cross - c) / 2.0,
                _ => 0.0,
            };
            let (x, y) = if vertical { (r.x + off, r.y + cursor) } else { (r.x + cursor, r.y + off) };
            self.shift(kids[i], x - e.x, y - e.y);
            cursor += a + gap;
        }
    }

    /// Är det en rad med assistenter (`hierChild` vars barn hör till `asst`-punkter)?
    fn is_asst_row(&self, id: usize) -> bool {
        let kids = &self.nodes[id].kids;
        !kids.is_empty() && kids.iter().all(|k| self.is_conn(*k) || self.m.pts[self.nodes[*k].pt].kind == Kind::Asst)
    }

    /// Hierarkins rot (`hierRoot`): rotens form, under den assistenterna (växelvis vänster och höger
    /// om den lodräta linjen) och sedan barnens rad, `sp` emellan. `hierAlign`: `tCtrCh` (standard)
    /// centrerar roten över raden, `tL`/`tR` låter raden hänga under roten förskjuten `alignOff` av
    /// rotens bredd, `lCtrCh`/`lT` m.fl. lägger raden till höger om roten (vågrät hierarki).
    fn hier_root(&mut self, id: usize, depth: usize) {
        let n = &self.nodes[id];
        let r = n.rect;
        let align = param(n, "hierAlign").unwrap_or("tCtrCh").to_owned();
        let align_off = self.nodes[id].vals.get("alignOff").copied().unwrap_or(0.0);
        let kids: Vec<usize> = n.kids.iter().copied().filter(|k| !self.is_conn(*k)).collect();
        let v = &self.nodes[id].vals;
        let gap = v.get("sp").copied().unwrap_or(0.0);
        let sib = v.get("sibSp").copied().unwrap_or(gap);
        let sec = v.get("secSibSp").copied().unwrap_or(gap);
        let (heads, rows): (Vec<usize>, Vec<usize>) = kids.iter().partition(|k| alg_type(&self.nodes[**k]) != "hierChild");
        let (assts, mains): (Vec<usize>, Vec<usize>) = rows.iter().partition(|k| self.is_asst_row(**k));
        // Roten (normalt en form) i origo.
        let mut head = Rect::default();
        let mut y = 0.0;
        for (i, &k) in heads.iter().enumerate() {
            let e = self.lay_alone(k, (r.w, r.h), depth);
            self.shift(k, -e.x, y - e.y);
            let e = self.extent(k);
            head = if i == 0 { e } else { head.union(e) };
            y += e.h;
        }
        let horizontal = align.starts_with('l') || align.starts_with('r');
        let cx = head.x + head.w / 2.0;
        let mut bottom = head.y + head.h;
        // Assistenterna två och två under roten, på var sin sida om linjen.
        for &row in &assts {
            let items: Vec<usize> = self.nodes[row].kids.iter().copied().filter(|k| !self.is_conn(*k)).collect();
            let mut line_y = bottom + gap;
            let mut line_h: f64 = 0.0;
            for (i, &k) in items.iter().enumerate() {
                let e = self.lay_alone(k, (r.w, r.h), depth);
                let x = if i % 2 == 0 { cx - sib / 2.0 - e.w } else { cx + sib / 2.0 };
                self.shift(k, x - e.x, line_y - e.y);
                line_h = line_h.max(e.h);
                if i % 2 == 1 || i + 1 == items.len() {
                    line_y += line_h + sec;
                    line_h = 0.0;
                }
            }
            bottom = line_y - sec;
            self.nodes[row].rect = self.extent(row);
        }
        for &row in &mains {
            self.layout(row, Rect { x: 0.0, y: 0.0, w: r.w, h: r.h }, depth + 1);
            let e = self.extent(row);
            if e.w <= 0.0 && e.h <= 0.0 {
                continue;
            }
            let (x, y) = if horizontal {
                let x = if align.starts_with('l') { head.x + head.w + gap } else { head.x - gap - e.w };
                let y = match &align[1..] {
                    "T" => head.y,
                    "B" => head.y + head.h - e.h,
                    _ => head.y + head.h / 2.0 - e.h / 2.0,
                };
                (x, y)
            } else {
                let x = match align.as_str() {
                    "tL" | "bL" => head.x + align_off * head.w,
                    "tR" | "bR" => head.x + head.w - align_off * head.w - e.w,
                    _ => cx - e.w / 2.0,
                };
                (x, bottom + gap)
            };
            self.shift(row, x - e.x, y - e.y);
            if !horizontal {
                bottom = y + e.h;
            }
        }
        // Allt till nodens ruta.
        let e = self.extent(id);
        self.shift(id, r.x - e.x, r.y - e.y);
    }

    /// Kopplingarna med `dim="1D"` eller `connRout="bend"` i hierarkier: en bruten linje från källans
    /// `begPts` till målets `endPts` (`bCtr` → `tCtr` med knäet `bendDist` före målet, eller till
    /// närmaste sida `midL`/`midR`). Källa och mål är formerna för övergångens förälder och barn
    /// (`srcNode`/`dstNode` väljer en namngiven nod för samma punkt).
    fn connect(&mut self) {
        let conns: Vec<usize> = (0..self.nodes.len()).filter(|i| self.is_conn(*i) && param(&self.nodes[*i], "dim") == Some("1D")).collect();
        for c in conns {
            let n = &self.nodes[c];
            let Some((par, ch)) = self.m.trans_ends(n.pt) else { continue };
            let find = |pt: usize, name: Option<&str>| -> Option<Rect> {
                let by_name = name.and_then(|nm| (0..self.nodes.len()).find(|i| self.nodes[*i].pt == pt && self.nodes[*i].name == nm));
                let by_text = || (0..self.nodes.len()).find(|i| self.nodes[*i].pt == pt && alg_type(&self.nodes[*i]) == "tx" && self.nodes[*i].rect.w > 0.0);
                by_name.or_else(by_text).map(|i| self.nodes[i].rect)
            };
            let (Some(s), Some(d)) = (find(par, param(n, "srcNode")), find(ch, param(n, "dstNode"))) else { continue };
            // Punkterna på en ruta (`ST_ConnectorPoint`).
            let at = |r: Rect, p: &str| -> (f64, f64) {
                match p {
                    "tCtr" => (r.x + r.w / 2.0, r.y),
                    "midL" => (r.x, r.y + r.h / 2.0),
                    "midR" => (r.x + r.w, r.y + r.h / 2.0),
                    "tL" => (r.x, r.y),
                    "tR" => (r.x + r.w, r.y),
                    "bL" => (r.x, r.y + r.h),
                    "bR" => (r.x + r.w, r.y + r.h),
                    "ctr" => (r.x + r.w / 2.0, r.y + r.h / 2.0),
                    _ => (r.x + r.w / 2.0, r.y + r.h),
                }
            };
            let dist = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2);
            let begs: Vec<&str> = param(n, "begPts").unwrap_or("bCtr").split_whitespace().collect();
            let ends: Vec<&str> = param(n, "endPts").unwrap_or("tCtr").split_whitespace().collect();
            // Av flera möjliga punkter de som ligger närmast varandra.
            let mut best = None;
            for bp in &begs {
                for ep in &ends {
                    let (b, e) = (at(s, bp), at(d, ep));
                    if best.as_ref().is_none_or(|(_, _, _, _, dd)| dist(b, e) < *dd) {
                        best = Some((b, e, *bp, *ep, dist(b, e)));
                    }
                }
            }
            let Some((beg, end, bp, ep, _)) = best else { continue };
            let from_end = param(n, "bendPt") == Some("end");
            let horiz_beg = matches!(bp, "midL" | "midR");
            let horiz_end = matches!(ep, "midL" | "midR");
            let pts = if horiz_beg && horiz_end {
                // Vågrätt ut och in: knä på mitten (eller `bendDist` före målet).
                let bx = match n.vals.get("bendDist") {
                    Some(b) if from_end => end.0 - b.copysign(end.0 - beg.0),
                    _ => (beg.0 + end.0) / 2.0,
                };
                vec![beg, (bx, beg.1), (bx, end.1), end]
            } else if horiz_end {
                // Lodrätt ned längs linjen, sedan vågrätt in i målets sida.
                vec![beg, (beg.0, end.1), end]
            } else if horiz_beg {
                vec![beg, (end.0, beg.1), end]
            } else {
                let bend = n.vals.get("bendDist").copied().unwrap_or(((end.1 - beg.1) / 2.0).abs());
                let by = if from_end { end.1 - bend.copysign(end.1 - beg.1) } else { beg.1 + bend.copysign(end.1 - beg.1) };
                vec![beg, (beg.0, by), (end.0, by), end]
            };
            let x0 = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
            let y0 = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let x1 = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
            let y1 = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            self.nodes[c].rect = Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
            self.paths.insert(c, pts);
        }
    }

    /// Hela trädet i ramen `w`×`h`: utläggning och sedan krympning och centrering om det inte
    /// får plats.
    pub fn run(&mut self, w: f64, h: f64) {
        if self.nodes.is_empty() {
            return;
        }
        self.layout(0, Rect { x: 0.0, y: 0.0, w, h }, 0);
        self.connect();
        let e = self.shapes_extent();
        if let Some(e) = e {
            if (e.w > w + 0.5 || e.h > h + 0.5) && !self.no_shrink {
                let s = (w / e.w).min(h / e.h);
                for n in &mut self.nodes {
                    n.rect = Rect { x: (n.rect.x - e.x) * s, y: (n.rect.y - e.y) * s, w: n.rect.w * s, h: n.rect.h * s };
                }
                let (nw, nh) = (e.w * s, e.h * s);
                let (dx, dy) = ((w - nw) / 2.0, (h - nh) / 2.0);
                for n in &mut self.nodes {
                    n.rect.x += dx;
                    n.rect.y += dy;
                }
                for pts in self.paths.values_mut() {
                    for p in pts.iter_mut() {
                        *p = ((p.0 - e.x) * s + dx, (p.1 - e.y) * s + dy);
                    }
                }
            }
        }
    }

    /// Växer nodens ruta med texten (en regel `type="h" val="INF"` på en textnod)?
    pub fn grows_with_text(&self, id: usize) -> bool {
        alg_type(&self.nodes[id]) == "tx" && self.rules_for(id).iter().any(|r| r.attr("type") == Some("h") && r.attr("val") == Some("INF"))
    }

    /// Ryms formerna i ramen `w`×`h`?
    pub fn fits_frame(&self, w: f64, h: f64) -> bool {
        self.shapes_extent().is_none_or(|e| e.x >= -0.5 && e.y >= -0.5 && e.x + e.w <= w + 0.5 && e.y + e.h <= h + 0.5)
    }

    /// Utbredningen av formerna som ritas.
    fn shapes_extent(&self) -> Option<Rect> {
        let mut r: Option<Rect> = None;
        for (i, n) in self.nodes.iter().enumerate() {
            // Bågpilarna får sticka ut ur ramen (som i PowerPoint).
            if !self.draws(i) || n.rect.w <= 0.0 || n.rect.h <= 0.0 || self.arcs.contains(&i) {
                continue;
            }
            r = Some(match r {
                Some(r) => r.union(n.rect),
                None => n.rect,
            });
        }
        r
    }

    /// Har noden en synlig form (inte dold geometri, inte en tom typ)?
    pub fn geometry(&self, id: usize) -> Option<&'a str> {
        let n = &self.nodes[id];
        let sh = n.shape?;
        if sh.attr_bool("hideGeom").unwrap_or(false) {
            return None;
        }
        // Böjda kopplingar som cirkelbågspilar; linjer (`dim="1D"`) bara med en bana.
        if alg_type(n) == "conn" && param(n, "connRout") == Some("curve") {
            if !self.arcs.contains(&id) {
                return None;
            }
            return Some(if n.vals.get("kladdArcLeft").is_some_and(|v| *v > 0.5) { "leftCircularArrow" } else { "circularArrow" });
        }
        if alg_type(n) == "conn" && param(n, "dim") == Some("1D") {
            return None;
        }
        match sh.attr("type") {
            None | Some("none") | Some("") => None,
            Some(t) => Some(t),
        }
    }

    /// Ritas noden (form eller text)?
    pub fn draws(&self, id: usize) -> bool {
        self.geometry(id).is_some() || self.paths.contains_key(&id) || !self.text_points(id).is_empty()
    }

    /// Datapunkterna vars text noden visar (`presOf`), med nivå (0 = punkten själv).
    pub fn text_points(&self, id: usize) -> Vec<(usize, usize)> {
        let n = &self.nodes[id];
        // Bara textalgoritmen ritar text; `presOf` på andra noder kopplar bara formen till punkten.
        if alg_type(n) != "tx" {
            return Vec::new();
        }
        // Utan `presOf` (äldre layouter, t.ex. `orgChart1#2`) visar textnoden sin egen punkt; ett tomt
        // `<presOf/>` visar ingenting.
        let Some(po) = n.pres_of else {
            return self.m.text(n.pt).filter(|_| self.m.is_type(n.pt, "node")).map(|_| vec![(n.pt, 0)]).unwrap_or_default();
        };
        if po.attr("axis").is_none() {
            return Vec::new();
        }
        let base = self.m.depth(n.pt);
        self.m
            .select(n.pt, po)
            .into_iter()
            .filter(|p| matches!(self.m.pts[*p].kind, Kind::Node | Kind::Asst | Kind::Doc | Kind::ParTrans | Kind::SibTrans))
            .filter(|p| self.m.text(*p).is_some())
            .map(|p| (p, self.m.depth(p).saturating_sub(base)))
            .collect()
    }
}
