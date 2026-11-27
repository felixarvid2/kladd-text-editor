//! Datamodellen (`ppt/diagrams/dataN.xml`): punkterna, `parOf`-trädet med övergångarna och de
//! axlar som layoutdefinitionens `forEach`, `if` och `presOf` går längs (ECMA-376 del 1 §21.4.3,
//! `ST_AxisType`, `ST_ElementType`).

use std::collections::HashMap;

use super::super::xml::Node;

/// Punkttyp (`dgm:pt@type`, standard `node`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Doc,
    Node,
    Asst,
    ParTrans,
    SibTrans,
    Pres,
}

pub struct Point<'a> {
    pub id: &'a str,
    pub kind: Kind,
    pub node: &'a Node,
}

/// Ett barn i `parOf`-trädet med sina övergångar.
#[derive(Clone, Copy)]
struct Kid {
    pt: usize,
    par_trans: Option<usize>,
    sib_trans: Option<usize>,
}

pub struct Model<'a> {
    pub pts: Vec<Point<'a>>,
    index: HashMap<&'a str, usize>,
    kids: HashMap<usize, Vec<Kid>>,
    parent: HashMap<usize, usize>,
    /// Presentationspunkter efter (presAssocID, presName), i dokumentordning.
    pres: HashMap<(&'a str, &'a str), Vec<usize>>,
    /// Dokumentpunkten.
    pub root: Option<usize>,
}

impl<'a> Model<'a> {
    pub fn new(data: &'a Node) -> Model<'a> {
        let mut m = Model { pts: Vec::new(), index: HashMap::new(), kids: HashMap::new(), parent: HashMap::new(), pres: HashMap::new(), root: None };
        if let Some(list) = data.child("dgm:ptLst") {
            for p in list.children_named("dgm:pt") {
                let Some(id) = p.attr("modelId") else { continue };
                let kind = match p.attr("type").unwrap_or("node") {
                    "doc" => Kind::Doc,
                    "asst" => Kind::Asst,
                    "parTrans" => Kind::ParTrans,
                    "sibTrans" => Kind::SibTrans,
                    "pres" => Kind::Pres,
                    _ => Kind::Node,
                };
                if m.index.contains_key(id) {
                    continue;
                }
                m.index.insert(id, m.pts.len());
                if kind == Kind::Doc && m.root.is_none() {
                    m.root = Some(m.pts.len());
                }
                m.pts.push(Point { id, kind, node: p });
            }
        }
        for (i, p) in m.pts.iter().enumerate() {
            if p.kind != Kind::Pres {
                continue;
            }
            if let Some(set) = p.node.child("dgm:prSet") {
                if let (Some(a), Some(n)) = (set.attr("presAssocID"), set.attr("presName")) {
                    m.pres.entry((a, n)).or_default().push(i);
                }
            }
        }
        let mut kids: HashMap<usize, Vec<(i64, Kid)>> = HashMap::new();
        for c in data.child("dgm:cxnLst").map(|l| l.children_named("dgm:cxn").collect::<Vec<_>>()).unwrap_or_default() {
            if c.attr("type").is_some_and(|t| t != "parOf") {
                continue;
            }
            let (Some(s), Some(d)) = (c.attr("srcId").and_then(|s| m.index.get(s)), c.attr("destId").and_then(|d| m.index.get(d))) else { continue };
            let (s, d) = (*s, *d);
            if !matches!(m.pts[d].kind, Kind::Node | Kind::Asst) || s == d || m.parent.contains_key(&d) {
                continue;
            }
            let tr = |a: &str| c.attr(a).and_then(|t| m.index.get(t)).copied();
            m.parent.insert(d, s);
            kids.entry(s).or_default().push((c.attr_i64("srcOrd").unwrap_or(0), Kid { pt: d, par_trans: tr("parTransId"), sib_trans: tr("sibTransId") }));
        }
        for (k, mut v) in kids {
            v.sort_by_key(|x| x.0);
            m.kids.insert(k, v.into_iter().map(|x| x.1).collect());
        }
        // Utan dokumentpunkt: den första punkten utan förälder.
        if m.root.is_none() {
            m.root = (0..m.pts.len()).find(|i| m.pts[*i].kind != Kind::Pres && !m.parent.contains_key(i));
        }
        m
    }

    /// Presentationspunkten för layoutnoden `name` och datapunkten `pt` (den `nth` i ordning).
    pub fn pres_point(&self, pt: usize, name: &str, nth: usize) -> Option<usize> {
        self.pres.get(&(self.pts[pt].id, name)).and_then(|v| v.get(nth).copied())
    }

    pub fn prset(&self, i: usize) -> Option<&'a Node> {
        self.pts.get(i)?.node.child("dgm:prSet")
    }

    /// Djupet i trädet (dokumentet 0, noderna direkt under det 1).
    pub fn depth(&self, mut pt: usize) -> usize {
        let mut d = 0;
        while let Some(p) = self.parent.get(&pt) {
            pt = *p;
            d += 1;
            if d > 64 {
                break;
            }
        }
        d
    }

    /// Övergångens nod (för `parTrans`/`sibTrans`): den vars koppling nämner den.
    fn owner_of_trans(&self, t: usize) -> Option<usize> {
        self.kids.values().flatten().find(|k| k.par_trans == Some(t) || k.sib_trans == Some(t)).map(|k| k.pt)
    }

    /// Barnsekvensen under `pt`: övergången till föräldern, noden och syskonövergången för varje barn.
    /// `hide_last` tar bort det sista barnets syskonövergång.
    fn child_seq(&self, pt: usize, hide_last: bool) -> Vec<usize> {
        let kids = self.kids.get(&pt).map(Vec::as_slice).unwrap_or(&[]);
        let mut out = Vec::new();
        for (i, k) in kids.iter().enumerate() {
            out.extend(k.par_trans);
            out.push(k.pt);
            if !(hide_last && i + 1 == kids.len()) {
                out.extend(k.sib_trans);
            }
        }
        out
    }

    fn descendants(&self, pt: usize, out: &mut Vec<usize>, depth: usize) {
        if depth > 32 {
            return;
        }
        for k in self.kids.get(&pt).map(Vec::as_slice).unwrap_or(&[]) {
            out.extend(k.par_trans);
            out.push(k.pt);
            out.extend(k.sib_trans);
            self.descendants(k.pt, out, depth + 1);
        }
    }

    /// Punkterna längs en axel från `pt` (utan typfilter).
    pub fn axis(&self, pt: usize, axis: &str, hide_last: bool) -> Vec<usize> {
        match axis {
            "self" | "none" | "" => vec![pt],
            "ch" => self.child_seq(pt, hide_last),
            "des" => {
                let mut v = Vec::new();
                self.descendants(pt, &mut v, 0);
                v
            }
            "desOrSelf" => {
                let mut v = vec![pt];
                self.descendants(pt, &mut v, 0);
                v
            }
            "par" => self.parent_of(pt).into_iter().collect(),
            "ancst" | "ancstOrSelf" => {
                let mut v = if axis == "ancstOrSelf" { vec![pt] } else { Vec::new() };
                let mut cur = pt;
                while let Some(p) = self.parent_of(cur) {
                    v.push(p);
                    cur = p;
                    if v.len() > 64 {
                        break;
                    }
                }
                v
            }
            "root" => self.root.into_iter().collect(),
            "followSib" | "precedSib" => {
                let Some(par) = self.parent_of(pt) else { return Vec::new() };
                let seq = self.child_seq(par, hide_last);
                let Some(i) = seq.iter().position(|x| *x == pt) else { return Vec::new() };
                if axis == "followSib" { seq[i + 1..].to_vec() } else { seq[..i].iter().rev().copied().collect() }
            }
            _ => Vec::new(),
        }
    }

    fn parent_of(&self, pt: usize) -> Option<usize> {
        match self.pts.get(pt)?.kind {
            Kind::ParTrans | Kind::SibTrans => self.owner_of_trans(pt).and_then(|o| self.parent.get(&o).copied()),
            _ => self.parent.get(&pt).copied(),
        }
    }

    /// Passar punkten typfiltret (`ST_ElementType`)?
    pub fn is_type(&self, pt: usize, ty: &str) -> bool {
        let k = self.pts[pt].kind;
        match ty {
            "all" | "" => true,
            "doc" => k == Kind::Doc,
            "node" => matches!(k, Kind::Node | Kind::Asst),
            "norm" => k == Kind::Node,
            "nonNorm" => k != Kind::Node,
            "asst" => k == Kind::Asst,
            "nonAsst" => k != Kind::Asst,
            "parTrans" => k == Kind::ParTrans,
            "sibTrans" => k == Kind::SibTrans,
            "pres" => k == Kind::Pres,
            _ => false,
        }
    }

    /// Urvalet för `forEach`/`if`/`presOf`: axlar och typer (mellanslagsseparerade listor) steg för
    /// steg, sedan `st`, `cnt` och `step` (1-baserade, `cnt` 0 = alla).
    pub fn select(&self, pt: usize, n: &Node) -> Vec<usize> {
        let axes: Vec<&str> = n.attr("axis").unwrap_or("none").split_whitespace().collect();
        let types: Vec<&str> = n.attr("ptType").unwrap_or("all").split_whitespace().collect();
        let hide_last = n.attr_bool("hideLastTrans").unwrap_or(true);
        let ints = |name: &str| -> Vec<i64> { n.attr(name).map(|s| s.split_whitespace().filter_map(|x| x.parse().ok()).collect()).unwrap_or_default() };
        let (st, cnt, step) = (ints("st"), ints("cnt"), ints("step"));
        let mut cur = vec![pt];
        for (i, ax) in axes.iter().enumerate() {
            let ty = types.get(i).or(types.last()).copied().unwrap_or("all");
            let mut next = Vec::new();
            for p in &cur {
                for q in self.axis(*p, ax, hide_last) {
                    if self.is_type(q, ty) && !next.contains(&q) {
                        next.push(q);
                    }
                }
            }
            let st = st.get(i).copied().unwrap_or(1);
            let cnt = cnt.get(i).copied().unwrap_or(0);
            let step = step.get(i).copied().unwrap_or(1);
            let start = if st > 0 { (st - 1) as usize } else { next.len().saturating_sub((-st) as usize) };
            let mut v = Vec::new();
            if step > 0 {
                let mut j = start;
                while j < next.len() {
                    v.push(next[j]);
                    if cnt > 0 && v.len() as i64 >= cnt {
                        break;
                    }
                    j += step as usize;
                }
            } else if step < 0 {
                let mut j = start as i64;
                while j >= 0 && (j as usize) < next.len() {
                    v.push(next[j as usize]);
                    if cnt > 0 && v.len() as i64 >= cnt {
                        break;
                    }
                    j += step;
                }
            }
            cur = v;
        }
        cur
    }

    /// Punktens text (`dgm:t`), om den har synlig text.
    pub fn text(&self, pt: usize) -> Option<&'a Node> {
        let t = self.pts.get(pt)?.node.child("dgm:t")?;
        let has = t.children_named("a:p").any(|p| p.children.iter().any(|r| matches!(r.name.as_str(), "a:r" | "a:fld") && r.child("a:t").is_some_and(|t| !t.text.is_empty())));
        has.then_some(t)
    }

    /// Har punkten egen textformatering (`custT`)?
    pub fn custom_text(&self, pt: usize) -> bool {
        self.prset(pt).and_then(|s| s.attr_bool("custT")).unwrap_or(false)
    }
}
