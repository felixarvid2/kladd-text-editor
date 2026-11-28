//! Från det utlagda layoutträdet till en ritdel (`p:spTree` med `p:sp`, som SmartArts sparade
//! `dsp:`-ritdel efter namnrymdsnormaliseringen): geometri, stil ur stildefinitionen
//! (`quickStyleN.xml`) och färgdefinitionen (`colorsN.xml`), text ur datamodellen med
//! teckenstorleken anpassad efter formen (`tx`-algoritmen och `primFontSz`-reglerna).

use std::collections::HashMap;

use super::super::color::Rgba;
use super::super::geom;
use super::super::inherit::{self, Sheets};
use super::super::richtext::{self, TextEnv};
use super::super::text;
use super::super::xml::Node;
use super::tree::{param, Rect, Tree};

const EMU: f64 = 12_700.0;

pub fn el(name: &str, attrs: &[(&str, String)], children: Vec<Node>) -> Node {
    Node { name: name.to_owned(), attrs: attrs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect(), children, text: String::new() }
}

fn emu(v: f64) -> String {
    (v * EMU).round().to_string()
}

fn set_attr(n: &mut Node, k: &str, v: String) {
    match n.attrs.iter_mut().find(|a| a.0 == k) {
        Some(a) => a.1 = v,
        None => n.attrs.push((k.to_owned(), v)),
    }
}

pub struct Styles<'x> {
    pub quick: Option<&'x Node>,
    pub colors: Option<&'x Node>,
}

impl Styles<'_> {
    fn quick_lbl(&self, name: &str) -> Option<&Node> {
        self.quick?.children_named("dgm:styleLbl").find(|s| s.attr("name") == Some(name))
    }

    fn colors_lbl(&self, name: &str) -> Option<&Node> {
        self.colors?.children_named("dgm:styleLbl").find(|s| s.attr("name") == Some(name))
    }

    /// Färgen ur färgdefinitionens lista (`fillClrLst` …) för formen `idx` av `cnt` med samma stil.
    fn color(&self, sheets: &Sheets<'_>, lbl: &str, list: &str, idx: usize, cnt: usize) -> Option<Node> {
        let l = self.colors_lbl(lbl)?.child(list)?;
        let colors: Vec<&Node> = l.children.iter().filter(|c| super::super::color::is_color(c)).collect();
        if colors.is_empty() {
            return None;
        }
        match l.attr("meth").unwrap_or("span") {
            "repeat" | "cycle" => Some(colors[idx % colors.len()].clone()),
            _ => {
                if colors.len() == 1 || cnt <= 1 {
                    return Some(colors[0].clone());
                }
                // Övergång mellan listans färger över formerna.
                let t = idx as f64 / (cnt - 1) as f64 * (colors.len() - 1) as f64;
                let i = (t.floor() as usize).min(colors.len() - 2);
                let k = (t - i as f64) as f32;
                let (a, b) = (sheets.color(colors[i], None)?, sheets.color(colors[i + 1], None)?);
                let m = |x: f32, y: f32| x + (y - x) * k;
                let c = Rgba { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: m(a.a, b.a) };
                let [r, g, bb, _] = c.to_u8();
                let mut n = el("a:srgbClr", &[("val", format!("{r:02X}{g:02X}{bb:02X}"))], Vec::new());
                if c.a < 0.999 {
                    n.children.push(el("a:alpha", &[("val", ((c.a * 100_000.0).round() as i64).to_string())], Vec::new()));
                }
                Some(n)
            }
        }
    }
}

/// Ett format för en form: stilnamn och plats bland formerna med samma stil.
struct StyleRef {
    lbl: String,
    idx: usize,
    cnt: usize,
}

pub struct Emitter<'t, 'a, 's, 'x> {
    pub tree: &'t Tree<'a>,
    pub sheets: &'s Sheets<'s>,
    pub styles: Styles<'x>,
}

/// Teckenstorlekar för en textform.
#[derive(Clone, Copy)]
struct Fonts {
    prim: f64,
    sec: f64,
}

struct Pending {
    id: usize,
    sp: Node,
    /// Textrutan (bredd, höjd) i punkter.
    text_box: (f64, f64),
    start: Fonts,
    min: f64,
    fixed: bool,
    font: Fonts,
}

impl Emitter<'_, '_, '_, '_> {
    fn style_label(&self, id: usize) -> Option<String> {
        let t = self.tree;
        if let Some(l) = t.nodes[id].pres.and_then(|p| t.m.prset(p)).and_then(|s| s.attr("presStyleLbl")) {
            return Some(l.to_owned());
        }
        let mut cur = Some(id);
        while let Some(i) = cur {
            if let Some(l) = &t.nodes[i].style_lbl {
                return Some(l.clone());
            }
            cur = t.nodes[i].parent;
        }
        None
    }

    fn style_refs(&self, drawn: &[usize]) -> HashMap<usize, StyleRef> {
        let t = self.tree;
        let mut by_lbl: HashMap<String, Vec<usize>> = HashMap::new();
        let mut lbls = HashMap::new();
        for &id in drawn {
            if let Some(l) = self.style_label(id) {
                by_lbl.entry(l.clone()).or_default().push(id);
                lbls.insert(id, l);
            }
        }
        let mut out = HashMap::new();
        for (id, lbl) in lbls {
            let set = t.nodes[id].pres.and_then(|p| t.m.prset(p));
            let list = &by_lbl[&lbl];
            let idx = set.and_then(|s| s.attr_i64("presStyleIdx")).map(|v| v.max(0) as usize).unwrap_or_else(|| list.iter().position(|x| *x == id).unwrap_or(0));
            let cnt = set.and_then(|s| s.attr_i64("presStyleCnt")).filter(|c| *c > 0).map(|v| v as usize).unwrap_or(list.len());
            out.insert(id, StyleRef { lbl, idx, cnt });
        }
        out
    }

    /// `p:style` ur stildefinitionen med färgerna ur färgdefinitionen.
    fn style(&self, sr: Option<&StyleRef>) -> Node {
        let q = sr.and_then(|s| self.styles.quick_lbl(&s.lbl)).and_then(|l| l.child("dgm:style"));
        let mut out = el("p:style", &[], Vec::new());
        for (name, list, def_idx) in [("a:lnRef", "dgm:linClrLst", "2"), ("a:fillRef", "dgm:fillClrLst", "1"), ("a:effectRef", "dgm:effectClrLst", "0"), ("a:fontRef", "dgm:txFillClrLst", "minor")] {
            let base = q.and_then(|q| q.child(name));
            let idx = base.and_then(|b| b.attr("idx")).unwrap_or(def_idx).to_owned();
            let mut r = el(name, &[("idx", idx)], Vec::new());
            let color = match sr {
                Some(s) => self.styles.color(self.sheets, &s.lbl, list, s.idx, s.cnt),
                None => None,
            };
            let color = color.or_else(|| {
                // Utan färgdefinition: stildefinitionens färg om den inte bara är en platshållare.
                let c = base.and_then(super::super::color::first_color)?;
                (c.name != "a:scrgbClr").then(|| c.clone())
            });
            let color = color.or_else(|| match (self.styles.colors.is_none(), name) {
                (true, "a:fillRef") => Some(el("a:schemeClr", &[("val", "accent1".into())], Vec::new())),
                (true, "a:lnRef") | (true, "a:fontRef") => Some(el("a:schemeClr", &[("val", "lt1".into())], Vec::new())),
                _ => None,
            });
            match color {
                Some(c) => r.children.push(c),
                // Utan färg ingen linje, fyllning eller effekt.
                None if name != "a:fontRef" => set_attr(&mut r, "idx", "0".into()),
                None => {}
            }
            out.children.push(r);
        }
        out
    }

    /// `a:avLst` ur formens `dgm:adjLst` (index i formdefinitionens ordning, värden i bråkdelar).
    fn adjusts(&self, prst: &str, shape: Option<&Node>, extra: &[(String, f64)]) -> Node {
        let names = geom::adjust_names(prst);
        let mut av = el("a:avLst", &[], Vec::new());
        let mut push = |name: &str, v: f64| {
            av.children.retain(|g: &Node| g.attr("name") != Some(name));
            av.children.push(el("a:gd", &[("name", name.to_owned()), ("fmla", format!("val {}", v.round() as i64))], Vec::new()));
        };
        if let Some(list) = shape.and_then(|s| s.child("dgm:adjLst")) {
            for a in list.children_named("dgm:adj") {
                let (Some(i), Some(v)) = (a.attr_i64("idx"), a.attr_f64("val")) else { continue };
                let Some((name, _)) = names.get((i - 1).max(0) as usize) else { continue };
                // Vinklar anges i grader, övriga i bråkdelar.
                let scale = if prst.contains("Arc") || prst.contains("circularArrow") || prst == "pie" || prst == "chord" { 60_000.0 } else { 100_000.0 };
                push(name, v * scale);
            }
        }
        for (n, v) in extra {
            push(n, *v);
        }
        av
    }

    /// Textstycken för formen med teckenstorlekarna `f`.
    fn body(&self, id: usize, f: Fonts, insets: (f64, f64, f64, f64)) -> Option<Node> {
        let t = self.tree;
        let pts = t.text_points(id);
        if pts.is_empty() {
            return None;
        }
        let node = &t.nodes[id];
        let st_bullet = param(node, "stBulletLvl").and_then(|v| v.parse::<usize>().ok()).unwrap_or(usize::MAX);
        let par_algn = match param(node, "parTxLTRAlign").unwrap_or("ctr") {
            "l" => "l",
            "r" => "r",
            _ => "ctr",
        };
        let ch_algn = match param(node, "shpTxLTRAlignCh").unwrap_or("l") {
            "ctr" => "ctr",
            "r" => "r",
            _ => "l",
        };
        let anchor = match param(node, "txAnchorVert").unwrap_or("mid") {
            "t" => "t",
            "b" => "b",
            _ => "ctr",
        };
        let min_lvl = pts.iter().map(|p| p.1).min().unwrap_or(0);
        let mut paras = Vec::new();
        for (pt, lvl) in pts {
            let Some(tx) = t.m.text(pt) else { continue };
            let lvl = lvl - min_lvl + if min_lvl > 0 && st_bullet != usize::MAX { min_lvl.min(st_bullet) } else { 0 };
            let bullet = lvl >= st_bullet;
            let size = if lvl > 0 { f.sec } else { f.prim };
            let custom = t.m.custom_text(pt);
            for p in tx.children_named("a:p") {
                // Egen textformatering: styckets storlek är körningarnas.
                let first_sz = p.children.iter().filter_map(|r| r.child("a:rPr").and_then(|x| x.attr_f64("sz"))).next();
                let size = match first_sz {
                    Some(sz) if custom => sz / 100.0,
                    _ => size,
                };
                let mut ppr = el("a:pPr", &[("lvl", lvl.min(8).to_string()), ("algn", (if lvl > 0 || bullet { ch_algn } else { par_algn }).to_owned())], Vec::new());
                let mut kids = vec![
                    el("a:lnSpc", &[], vec![el("a:spcPct", &[("val", "90000".into())], Vec::new())]),
                    el("a:spcBef", &[], vec![el("a:spcPct", &[("val", "0".into())], Vec::new())]),
                    el("a:spcAft", &[], vec![el("a:spcPct", &[("val", (if bullet { "15000" } else { "35000" }).into())], Vec::new())]),
                ];
                if bullet {
                    let ind = size * 0.7 * EMU;
                    let depth = (lvl + 1 - st_bullet) as f64;
                    set_attr(&mut ppr, "marL", ((ind * depth).round() as i64).to_string());
                    set_attr(&mut ppr, "indent", ((-ind).round() as i64).to_string());
                    kids.push(el("a:buChar", &[("char", "•".into())], Vec::new()));
                } else {
                    set_attr(&mut ppr, "marL", "0".into());
                    set_attr(&mut ppr, "indent", "0".into());
                    kids.push(el("a:buNone", &[], Vec::new()));
                }
                // Datamodellens egna styckeegenskaper går före.
                if let Some(own) = p.child("a:pPr") {
                    for (k, v) in &own.attrs {
                        if k != "lvl" {
                            set_attr(&mut ppr, k, v.clone());
                        }
                    }
                    for c in &own.children {
                        kids.retain(|k| k.name != c.name && !(c.name.starts_with("a:bu") && k.name.starts_with("a:bu") && c.name != "a:buClr" && c.name != "a:buSzPct" && c.name != "a:buFont"));
                        kids.push(c.clone());
                    }
                }
                ppr.children = kids;
                let mut para = el("a:p", &[], vec![ppr]);
                for r in &p.children {
                    if r.name == "a:pPr" {
                        continue;
                    }
                    let mut r = r.clone();
                    let rpr_name = if r.name == "a:endParaRPr" { None } else { Some("a:rPr") };
                    let target: Option<&mut Node> = match rpr_name {
                        None => Some(&mut r),
                        Some(n) => {
                            if !r.children.iter().any(|c| c.name == n) && matches!(r.name.as_str(), "a:r" | "a:fld" | "a:br") {
                                r.children.insert(0, el(n, &[], Vec::new()));
                            }
                            r.children.iter_mut().find(|c| c.name == n)
                        }
                    };
                    if let Some(rpr) = target {
                        if !(custom && rpr.attr("sz").is_some()) {
                            set_attr(rpr, "sz", ((size * 100.0).round() as i64).to_string());
                        }
                    }
                    para.children.push(r);
                }
                paras.push(para);
            }
        }
        let (l, tp, r, b) = insets;
        let body_pr = el(
            "a:bodyPr",
            &[("wrap", "square".into()), ("lIns", emu(l)), ("tIns", emu(tp)), ("rIns", emu(r)), ("bIns", emu(b)), ("anchor", anchor.into()), ("anchorCtr", "0".into())],
            vec![el("a:noAutofit", &[], Vec::new())],
        );
        let mut children = vec![body_pr, el("a:lstStyle", &[], Vec::new())];
        children.extend(paras);
        Some(el("p:txBody", &[], children))
    }

    /// Den största körningsstorleken när all text i formen har egen formatering (`custT`).
    fn fixed_size(&self, id: usize) -> Option<f64> {
        let t = self.tree;
        let pts = t.text_points(id);
        if pts.is_empty() || !pts.iter().all(|(p, _)| t.m.custom_text(*p)) {
            return None;
        }
        let mut max: Option<f64> = None;
        for (p, _) in pts {
            t.m.text(p)?.walk(&mut |n| {
                if let Some(sz) = (n.name == "a:rPr").then(|| n.attr_f64("sz")).flatten() {
                    max = Some(max.map_or(sz / 100.0, |m: f64| m.max(sz / 100.0)));
                }
            });
        }
        max
    }

    fn insets(&self, id: usize, f: Fonts) -> (f64, f64, f64, f64) {
        let n = &self.tree.nodes[id];
        let size = self.fixed_size(id).unwrap_or(f.prim);
        // Utan villkor för marginalen: textrutans vanliga insättningar (7,2 pt vid sidorna, 3,6 pt upptill
        // och nedtill), högst en tiondel av rutan (organisationsschemats rutor har inga egna).
        let m = |k: &str, def: f64| -> f64 {
            match n.vals.get(&format!("{k}%")) {
                Some(fact) => fact * size,
                None => n.vals.get(k).copied().unwrap_or(def),
            }
        };
        let (dw, dh) = ((n.rect.w * 0.1).min(7.2), (n.rect.h * 0.1).min(3.6));
        (m("lMarg", dw), m("tMarg", dh), m("rMarg", dw), m("bMarg", dh))
    }

    /// Ryms texten i rutan?
    fn fits(&self, sp: &Node, body: &Node, w: f64, h: f64) -> bool {
        let bodies: Vec<&Node> = body.child("a:bodyPr").into_iter().collect();
        let bp = richtext::body_props(&bodies);
        let chain = [sp];
        let lists = inherit::list_styles(self.sheets, &chain, inherit::tx_kind(sp));
        let (font_ref, font_color) = inherit::style_font(self.sheets, sp.child("p:style"));
        let env = TextEnv { sheets: self.sheets, lists, font_ref, font_color, font_scale: bp.font_scale, slide_no: 1 };
        let fr = richtext::frame(&env, body, &bp);
        let fonts = &self.sheets.deck.fonts;
        let laid = text::layout(&fr, w as f32, h as f32, fonts);
        if (laid.height + bp.insets.1 + bp.insets.3) as f64 > h + 0.25 {
            return false;
        }
        // Inte heller bredare än rutan innanför insättningarna (ett ord som inte kan brytas sticker
        // annars ut åt sidan).
        let (left, right) = (bp.insets.0 as f64 - 0.5, w - bp.insets.2 as f64 + 0.5);
        for it in &laid.items {
            if let super::super::scene::Item::Glyphs(g) = it {
                for gl in &g.glyphs {
                    let (x0, _) = g.t.apply(gl.x, 0.0);
                    let (x1, _) = g.t.apply(gl.x + gl.advance, 0.0);
                    if (x0.min(x1) as f64) < left || (x0.max(x1) as f64) > right {
                        return false;
                    }
                }
            }
        }
        // Inget ord får brytas mitt i: de längsta orden ska rymmas på en rad var.
        let mut words: Vec<(&str, &Node)> = Vec::new();
        for r in body.children_named("a:p").flat_map(|p| p.children_named("a:r")) {
            if let Some(t) = r.child("a:t") {
                words.extend(t.text.split_whitespace().filter(|w| w.chars().count() <= 25).map(|w| (w, r)));
            }
        }
        words.sort_by_key(|(w, _)| std::cmp::Reverse(w.chars().count()));
        let first_ppr = body.children_named("a:p").find_map(|p| p.child("a:pPr")).cloned();
        for (word, run) in words.into_iter().take(2) {
            let mut r = run.clone();
            if let Some(t) = r.children.iter_mut().find(|c| c.name == "a:t") {
                t.text = word.to_owned();
            }
            let mut one = body.clone();
            one.children.retain(|c| c.name != "a:p");
            one.children.push(el("a:p", &[], first_ppr.clone().into_iter().chain([r]).collect()));
            let fr = richtext::frame(&env, &one, &bp);
            let narrow = text::layout(&fr, w as f32, h as f32, fonts).height;
            let wide = text::layout(&fr, 1.0e5, h as f32, fonts).height;
            if narrow > wide + 0.5 {
                return false;
            }
        }
        true
    }

    fn fit(&self, p: &Pending) -> Fonts {
        if p.fixed || p.start.prim <= p.min {
            return p.start;
        }
        let mk = |k: f64| Fonts { prim: (p.start.prim * k).max(p.min), sec: (p.start.sec * k).max(p.min) };
        let try_fit = |f: Fonts| -> bool {
            let insets = self.insets(p.id, f);
            match self.body(p.id, f, insets) {
                Some(b) => self.fits(&p.sp, &b, p.text_box.0, p.text_box.1),
                None => true,
            }
        };
        if try_fit(p.start) {
            return p.start;
        }
        // Halva punkter nedåt (binärsökning), minst regelns storlek.
        let steps = ((p.start.prim - p.min) * 2.0).floor() as i64;
        let (mut lo, mut hi) = (0i64, steps.max(0));
        let at = |s: i64| {
            let prim = ((p.start.prim * 2.0).floor() - s as f64) / 2.0;
            mk(prim / p.start.prim)
        };
        while lo < hi {
            let mid = (lo + hi) / 2;
            if try_fit(at(mid)) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        at(lo)
    }

    /// Ritdelens formträd (`p:spTree`).
    /// En bruten linje (koppling i en hierarki) som `a:custGeom` utan fyllning.
    fn polyline(&self, id: usize, pts: &[(f64, f64)], sr: Option<&StyleRef>) -> Node {
        let t = self.tree;
        let x0 = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let y0 = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let w = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) - x0;
        let h = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) - y0;
        let pe = |v: f64| ((v * EMU).round() as i64).max(1).to_string();
        let pt = |p: (f64, f64)| el("a:pt", &[("x", emu(p.0 - x0)), ("y", emu(p.1 - y0))], Vec::new());
        let mut path = el("a:path", &[("w", pe(w)), ("h", pe(h)), ("fill", "none".into())], vec![el("a:moveTo", &[], vec![pt(pts[0])])]);
        for p in &pts[1..] {
            path.children.push(el("a:lnTo", &[], vec![pt(*p)]));
        }
        let geom = el(
            "a:custGeom",
            &[],
            vec![
                el("a:avLst", &[], Vec::new()),
                el("a:gdLst", &[], Vec::new()),
                el("a:rect", &[("l", "0".into()), ("t", "0".into()), ("r", "r".into()), ("b", "b".into())], Vec::new()),
                el("a:pathLst", &[], vec![path]),
            ],
        );
        let xfrm = el("a:xfrm", &[], vec![el("a:off", &[("x", emu(x0)), ("y", emu(y0))], Vec::new()), el("a:ext", &[("cx", emu(w)), ("cy", emu(h))], Vec::new())]);
        let sppr = el("p:spPr", &[], vec![xfrm, geom, el("a:noFill", &[], Vec::new())]);
        let model_id = t.nodes[id].pres.map(|p| t.m.pts[p].id.to_owned()).unwrap_or_default();
        el(
            "p:sp",
            &[("modelId", model_id)],
            vec![el("p:nvSpPr", &[], vec![el("p:cNvPr", &[("id", "0".into()), ("name", String::new())], Vec::new()), el("p:cNvSpPr", &[], Vec::new())]), sppr, self.style(sr)],
        )
    }

    /// Ritdelens formträd, eller `None` om resultatet inte går att läsa: någon text fick krympas
    /// under 5 punkter eller får inte plats, eller mer än hälften av datamodellens text hamnade inte
    /// i någon form (då är reserven, datamodellens text som punktlista, bättre).
    pub fn emit(&self) -> Option<Node> {
        let t = self.tree;
        let mut drawn: Vec<usize> = (0..t.nodes.len()).filter(|i| t.paths.contains_key(i) || (t.draws(*i) && t.nodes[*i].rect.w > 0.01 && t.nodes[*i].rect.h > 0.01)).collect();
        // Linjerna först (under formerna); `zOrderOff` flyttar formen framåt eller bakåt.
        let z = |i: usize| if t.paths.contains_key(&i) { i64::MIN } else { t.nodes[i].shape.and_then(|s| s.attr_i64("zOrderOff")).unwrap_or(0) };
        drawn.sort_by_key(|i| z(*i));
        let refs = self.style_refs(&drawn);
        let mut pending: Vec<Pending> = Vec::new();
        let mut out = Vec::new();
        for &id in &drawn {
            let n = &t.nodes[id];
            let r = n.rect;
            let sr = refs.get(&id);
            if let Some(pts) = t.paths.get(&id) {
                out.push((id, self.polyline(id, pts, sr), None));
                continue;
            }
            let shape = n.shape;
            let pres_sppr = n.pres.and_then(|p| t.m.pts[p].node.child("dgm:spPr"));
            let mut prst = t.geometry(id).map(str::to_owned);
            if prst.as_deref() == Some("conn") {
                prst = Some("rightArrow".into());
            }
            if let Some(g) = pres_sppr.and_then(|s| s.child("a:prstGeom")).and_then(|g| g.attr("prst")) {
                if prst.is_some() {
                    prst = Some(g.to_owned());
                }
            }
            // Formens vridning ur layouten, algoritmen och PowerPoints egen vridning (`custAng`).
            let cust_ang = n.pres.and_then(|p| t.m.prset(p)).and_then(|s| s.attr_f64("custAng")).unwrap_or(0.0) / 60_000.0;
            let mut rot = shape.and_then(|s| s.attr_f64("rot")).unwrap_or(0.0) + n.vals.get("kladdRot").copied().unwrap_or(0.0) + cust_ang;
            if t.geometry(id) == Some("conn") {
                if let Some(p) = n.parent {
                    rot += match param(&t.nodes[p], "linDir") {
                        Some("fromR") => 180.0,
                        Some("fromT") => 90.0,
                        Some("fromB") => 270.0,
                        _ => 0.0,
                    };
                }
            }
            let rot = rot.rem_euclid(360.0);
            // Vid 90°/270° byter bredd och höjd plats så att den vridna formen täcker rutan.
            let lay_rot = (rot - cust_ang).rem_euclid(360.0);
            let quarter = ((lay_rot - 90.0).abs() < 1.0) || ((lay_rot - 270.0).abs() < 1.0);
            let (ew, eh) = if quarter { (r.h, r.w) } else { (r.w, r.h) };
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let mut xfrm = el(
                "a:xfrm",
                &[],
                vec![el("a:off", &[("x", emu(cx - ew / 2.0)), ("y", emu(cy - eh / 2.0))], Vec::new()), el("a:ext", &[("cx", emu(ew)), ("cy", emu(eh))], Vec::new())],
            );
            if rot.abs() > 0.01 {
                set_attr(&mut xfrm, "rot", ((rot * 60_000.0).round() as i64).to_string());
            }
            let mut sppr = el("p:spPr", &[], vec![xfrm]);
            let mut extra = Vec::new();
            if let (Some(st), Some(end), Some(w)) = (n.vals.get("kladdArcSt"), n.vals.get("kladdArcEnd"), n.vals.get("kladdArcW")) {
                // Cirkelbågspilen: tjocklek och pilspets i förhållande till storleken, vinklar i
                // 60 000-dels grader (medurs från x-axeln); `leftCircularArrow` har spetsen i början.
                let left = n.vals.get("kladdArcLeft").is_some_and(|v| *v > 0.5);
                let (a3, a4) = if left { (*st, *end) } else { (*end, *st) };
                extra.push(("adj1".to_owned(), w * 100_000.0));
                extra.push(("adj2".to_owned(), 6.0 * 60_000.0));
                extra.push(("adj3".to_owned(), a3 * 60_000.0));
                extra.push(("adj4".to_owned(), a4 * 60_000.0));
                extra.push(("adj5".to_owned(), w * 1.2 * 100_000.0));
            }
            if let Some(inset) = n.vals.get("kladdPyra") {
                // Trapetsens sidor i linje med pyramidens: insättningen i förhållande till min(b, h).
                extra.push(("adj".to_owned(), inset / ew.min(eh).max(1e-6) * 100_000.0));
            }
            match &prst {
                Some(p) => {
                    let mut g = el("a:prstGeom", &[("prst", p.clone())], Vec::new());
                    g.children.push(self.adjusts(p, shape, &extra));
                    sppr.children.push(g);
                    // Formateringen av den första punkten formen visar och av presentationspunkten
                    // (den senare går före).
                    let first_pt = t.text_points(id).first().map(|p| p.0);
                    let data_sppr = first_pt.and_then(|p| t.m.pts[p].node.child("dgm:spPr"));
                    for own in [data_sppr, pres_sppr].into_iter().flatten() {
                        for c in &own.children {
                            if !matches!(c.name.as_str(), "a:xfrm" | "a:prstGeom" | "a:custGeom") {
                                fn kind(n: &str) -> &str {
                                    if n.ends_with("Fill") { "fill" } else { n }
                                }
                                sppr.children.retain(|x| x.name == "a:xfrm" || x.name == "a:prstGeom" || kind(&x.name) != kind(&c.name));
                                sppr.children.push(c.clone());
                            }
                        }
                    }
                }
                None => {
                    sppr.children.push(el("a:prstGeom", &[("prst", "rect".into())], vec![el("a:avLst", &[], Vec::new())]));
                    sppr.children.push(el("a:noFill", &[], Vec::new()));
                    sppr.children.push(el("a:ln", &[], vec![el("a:noFill", &[], Vec::new())]));
                }
            }
            if prst.is_some() {
                if let Some(q) = sr.and_then(|s| self.styles.quick_lbl(&s.lbl)) {
                    for (from, to) in [("dgm:scene3d", "a:scene3d"), ("dgm:sp3d", "a:sp3d")] {
                        if let Some(c) = q.child(from).filter(|c| !c.children.is_empty() || !c.attrs.is_empty()) {
                            let mut c = c.clone();
                            c.name = to.into();
                            sppr.children.push(c);
                        }
                    }
                }
            }
            let model_id = n.pres.map(|p| t.m.pts[p].id.to_owned()).unwrap_or_default();
            let sp = el(
                "p:sp",
                &[("modelId", model_id)],
                vec![el("p:nvSpPr", &[], vec![el("p:cNvPr", &[("id", "0".into()), ("name", String::new())], Vec::new()), el("p:cNvSpPr", &[], Vec::new())]), sppr, self.style(sr)],
            );
            if t.text_points(id).is_empty() {
                out.push((id, sp, None));
                continue;
            }
            // Textrutan: formens textrektangel i den ovridna rutan.
            let tr = match &prst {
                Some(p) => {
                    let av = self.adjusts(p, shape, &extra);
                    geom::preset(p, Some(&av), r.w, r.h).map(|g| g.text_rect).map(|(l, tp, rr, b)| (l as f64, tp as f64, rr as f64, b as f64)).unwrap_or((0.0, 0.0, r.w, r.h))
                }
                None => (0.0, 0.0, r.w, r.h),
            };
            let text_rect = Rect { x: r.x + tr.0, y: r.y + tr.1, w: (tr.2 - tr.0).max(1.0), h: (tr.3 - tr.1).max(1.0) };
            let prim = t.bounded(id, "primFontSz").unwrap_or(65.0);
            let sec = t.bounded(id, "secFontSz").unwrap_or(prim);
            let min = t.rules_for(id).iter().filter(|r| r.attr("type").is_some_and(|t| t.ends_with("FontSz"))).filter_map(|r| r.attr_f64("val")).fold(f64::INFINITY, f64::min);
            let (start, min) = if min.is_finite() { (Fonts { prim, sec }, min) } else { (Fonts { prim: prim.min(18.0), sec: sec.min(18.0) }, prim.min(18.0)) };
            let fixed = t.text_points(id).iter().all(|(p, _)| t.m.custom_text(*p));
            let mut txx = el(
                "p:txXfrm",
                &[],
                vec![el("a:off", &[("x", emu(text_rect.x)), ("y", emu(text_rect.y))], Vec::new()), el("a:ext", &[("cx", emu(text_rect.w)), ("cy", emu(text_rect.h))], Vec::new())],
            );
            // Texten står upprätt mot layoutens vridning men följer PowerPoints egen (`custAng`).
            let text_rot = (rot - cust_ang).rem_euclid(360.0);
            if text_rot.abs() > 0.01 {
                set_attr(&mut txx, "rot", ((-text_rot * 60_000.0).round() as i64).to_string());
            }
            out.push((id, sp, Some(txx)));
            let sp_ref = out.last().map(|o| o.1.clone()).unwrap_or_default();
            pending.push(Pending { id, sp: sp_ref, text_box: (text_rect.w, text_rect.h), start, min, fixed, font: start });
        }
        // Teckenstorlek per form, sedan den minsta i varje grupp med `op="equ"`.
        for p in &mut pending {
            p.font = self.fit(p);
        }
        let mut group_min: HashMap<(usize, usize), f64> = HashMap::new();
        for p in &pending {
            if let Some(g) = t.nodes[p.id].font_groups.get("primFontSz") {
                let e = group_min.entry(*g).or_insert(f64::INFINITY);
                *e = e.min(p.font.prim / p.start.prim.max(1e-6));
            }
        }
        let mut fonts: HashMap<usize, Fonts> = HashMap::new();
        for p in &pending {
            let mut f = p.font;
            if let Some(k) = t.nodes[p.id].font_groups.get("primFontSz").and_then(|g| group_min.get(g)) {
                if !p.fixed {
                    f = Fonts { prim: (p.start.prim * k).max(p.min), sec: (p.start.sec * k).max(p.min) };
                }
            }
            if !p.fixed && p.min < p.start.prim {
                // Under 5 punkter, eller får texten inte plats ens i regelns minsta storlek.
                let fits = self.body(p.id, f, self.insets(p.id, f)).is_none_or(|b| self.fits(&p.sp, &b, p.text_box.0, p.text_box.1));
                if f.prim.min(f.sec) < 5.0 - 1e-6 || !fits {
                    return None;
                }
            }
            fonts.insert(p.id, f);
        }
        let mut shown = std::collections::HashSet::new();
        for p in &pending {
            shown.extend(t.text_points(p.id).into_iter().map(|x| x.0));
        }
        let with_text: Vec<usize> = (0..t.m.pts.len()).filter(|i| t.m.is_type(*i, "node") && t.m.text(*i).is_some()).collect();
        let lost = with_text.iter().filter(|i| !shown.contains(*i)).count();
        if lost * 2 > with_text.len() {
            return None;
        }
        let mut tree = el(
            "p:spTree",
            &[],
            vec![el("p:nvGrpSpPr", &[], vec![el("p:cNvPr", &[("id", "0".into()), ("name", String::new())], Vec::new()), el("p:cNvGrpSpPr", &[], Vec::new())]), el("p:grpSpPr", &[], Vec::new())],
        );
        for (id, mut sp, txx) in out {
            if let (Some(txx), Some(f)) = (txx, fonts.get(&id)) {
                if let Some(b) = self.body(id, *f, self.insets(id, *f)) {
                    sp.children.push(b);
                    sp.children.push(txx);
                }
            }
            tree.children.push(sp);
        }
        Some(tree)
    }
}
