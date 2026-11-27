//! Text i en form under redigering: en modell av `p:txBody` (stycken med `a:pPr`, körningar med
//! `a:rPr`, radbrytningar och fält), redigering med markörlägen och utskrift tillbaka till XML där
//! orörda stycken behåller sina byte. Markörens läge räknas med samma textmotor som ritar bilden
//! (`text::layout_carets`), så markören står där texten ritas.

use super::edit_xml::{self, XElem, XNode};
use super::geom;
use super::inherit::{self, Owner, Sheets};
use super::model::Deck;
use super::render::frame_of;
use super::richtext::{self, TextEnv};
use super::scene::Affine;
use super::text::{self, CaretStop};
use super::xml::{self, Node};

/// Ett läge i texten: stycke och tecken i stycket (radbrytningar och fält räknas som ett tecken).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub para: usize,
    pub ch: usize,
}

/// En del av ett stycke.
#[derive(Clone, Debug)]
pub enum Item {
    Run { rpr: Option<XElem>, text: String },
    /// `a:br`.
    Break(XElem),
    /// `a:fld` (ett tecken).
    Field(XElem),
    /// Annat (formler, okända element): följer med orört, upptar inget tecken.
    Other(XNode),
}

impl Item {
    pub fn len(&self) -> usize {
        match self {
            Item::Run { text, .. } => text.chars().count(),
            Item::Break(_) | Item::Field(_) => 1,
            Item::Other(_) => 0,
        }
    }
}

/// Ett stycke.
#[derive(Clone, Debug)]
pub struct TPara {
    pub ppr: Option<XElem>,
    pub items: Vec<Item>,
    pub end: Option<XElem>,
    /// Originalet, så länge stycket inte har ändrats (skrivs då ut som det var).
    orig: Option<XElem>,
}

impl TPara {
    pub fn len(&self) -> usize {
        self.items.iter().map(Item::len).sum()
    }

    /// Styckets text (radbrytning = `\n`, fält = dess text).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for it in &self.items {
            match it {
                Item::Run { text, .. } => s.push_str(text),
                Item::Break(_) => s.push('\n'),
                Item::Field(f) => s.push(f.child("a:t").map(|t| t.text()).and_then(|t| t.chars().next()).unwrap_or(' ')),
                Item::Other(_) => {}
            }
        }
        s
    }

    fn touch(&mut self) {
        self.orig = None;
    }

    pub fn level(&self) -> i64 {
        self.ppr.as_ref().and_then(|p| p.attr_i64("lvl")).unwrap_or(0)
    }
}

/// Textkroppen under redigering.
#[derive(Clone, Debug)]
pub struct TextModel {
    pub paras: Vec<TPara>,
    /// Prefixet för DrawingML (`a`) i bildens XML och rotens namnrymder (för nya element).
    a: String,
    scope: Vec<(String, String)>,
}

fn q(a: &str, l: &str) -> String {
    if a.is_empty() { l.to_owned() } else { format!("{a}:{l}") }
}

impl TextModel {
    /// Läser `p:txBody` (eller en tom kropp med ett tomt stycke om den saknas).
    pub fn from_body(body: Option<&XElem>, scope: &[(String, String)], a: &str) -> TextModel {
        let mut paras = Vec::new();
        if let Some(b) = body {
            for p in b.elems().filter(|e| e.name == "a:p") {
                let mut tp = TPara { ppr: None, items: Vec::new(), end: None, orig: Some(p.clone()) };
                for c in &p.children {
                    match c {
                        XNode::Elem(e) => match e.name.as_str() {
                            "a:pPr" => tp.ppr = Some(e.clone()),
                            "a:endParaRPr" => tp.end = Some(e.clone()),
                            "a:r" => tp.items.push(Item::Run { rpr: e.child("a:rPr").cloned(), text: e.child("a:t").map(|t| t.text()).unwrap_or_default() }),
                            "a:br" => tp.items.push(Item::Break(e.clone())),
                            "a:fld" => tp.items.push(Item::Field(e.clone())),
                            _ => tp.items.push(Item::Other(c.clone())),
                        },
                        XNode::Other(_) => tp.items.push(Item::Other(c.clone())),
                        XNode::Text(_) => {}
                    }
                }
                paras.push(tp);
            }
        }
        if paras.is_empty() {
            paras.push(TPara { ppr: None, items: Vec::new(), end: None, orig: None });
        }
        TextModel { paras, a: a.to_owned(), scope: scope.to_vec() }
    }

    fn frag(&self, xml: &str) -> Option<XElem> {
        edit_xml::fragment_in(&self.scope, xml).into_iter().next()
    }

    /// Prefixet som modellen skriver DrawingML-element med.
    pub fn prefix(&self) -> &str {
        &self.a
    }

    /// Hela texten (stycken med `\n`).
    pub fn text(&self) -> String {
        self.paras.iter().map(TPara::text).collect::<Vec<_>>().join("\n")
    }

    pub fn end(&self) -> Pos {
        let p = self.paras.len() - 1;
        Pos { para: p, ch: self.paras[p].len() }
    }

    pub fn clamp(&self, p: Pos) -> Pos {
        let para = p.para.min(self.paras.len() - 1);
        Pos { para, ch: p.ch.min(self.paras[para].len()) }
    }

    /// Delar körningen vid tecknet `ch` så att ett item börjar där; ger itemets index.
    fn split_at(&mut self, para: usize, ch: usize) -> usize {
        let items = &mut self.paras[para].items;
        let mut at = 0;
        for i in 0..items.len() {
            let len = items[i].len();
            if ch == at {
                return i;
            }
            if ch < at + len {
                if let Item::Run { rpr, text } = &items[i] {
                    let k = text.char_indices().nth(ch - at).map_or(text.len(), |(b, _)| b);
                    let (l, r) = (text[..k].to_owned(), text[k..].to_owned());
                    let rpr = rpr.clone();
                    items[i] = Item::Run { rpr: rpr.clone(), text: l };
                    items.insert(i + 1, Item::Run { rpr, text: r });
                    return i + 1;
                }
                return i + 1;
            }
            at += len;
        }
        items.len()
    }

    /// Formateringen för text som skrivs vid läget: körningen till vänster, annars till höger,
    /// annars styckets slutformat.
    fn rpr_at(&self, p: Pos) -> Option<XElem> {
        let items = &self.paras[p.para].items;
        let mut at = 0;
        let mut left: Option<&Option<XElem>> = None;
        let mut right: Option<&Option<XElem>> = None;
        for it in items {
            let len = it.len();
            if let Item::Run { rpr, .. } = it {
                if at < p.ch || (at == p.ch && len == 0 && left.is_none()) {
                    left = Some(rpr);
                }
                if right.is_none() && at + len > p.ch {
                    right = Some(rpr);
                }
            }
            at += len;
        }
        if let Some(r) = left.or(right) {
            return r.clone();
        }
        self.paras[p.para].end.as_ref().map(|e| {
            let mut r = e.clone();
            r.rename(&q(&self.a, "rPr"), "a:rPr");
            r.remove_attr("dirty");
            r
        })
    }

    /// Skriver text vid läget (radslut i texten ger nya stycken). Ger läget efter texten.
    pub fn insert(&mut self, p: Pos, s: &str) -> Pos {
        let mut p = self.clamp(p);
        for (i, line) in s.split('\n').enumerate() {
            if i > 0 {
                p = self.split_para(p);
            }
            let line = line.replace('\r', "").replace('\u{b}', "");
            if line.is_empty() {
                continue;
            }
            p = self.insert_plain(p, &line);
        }
        p
    }

    fn insert_plain(&mut self, p: Pos, s: &str) -> Pos {
        let n = s.chars().count();
        let rpr = self.rpr_at(p);
        let para = &mut self.paras[p.para];
        para.touch();
        // Skriv i körningen som slutar (eller börjar) vid läget om formateringen är densamma.
        let mut at = 0;
        let mut target: Option<(usize, usize)> = None;
        for (i, it) in para.items.iter().enumerate() {
            let len = it.len();
            if let Item::Run { rpr: r, .. } = it {
                let same = r.as_ref().map(XElem::to_xml) == rpr.as_ref().map(XElem::to_xml);
                if same && at <= p.ch && p.ch <= at + len {
                    target = Some((i, p.ch - at));
                    if p.ch < at + len || p.ch == at + len {
                        break;
                    }
                }
            }
            at += len;
        }
        match target {
            Some((i, off)) => {
                if let Item::Run { text, .. } = &mut para.items[i] {
                    let k = text.char_indices().nth(off).map_or(text.len(), |(b, _)| b);
                    text.insert_str(k, s);
                }
            }
            None => {
                let i = self.split_at(p.para, p.ch);
                self.paras[p.para].items.insert(i, Item::Run { rpr, text: s.to_owned() });
            }
        }
        Pos { para: p.para, ch: p.ch + n }
    }

    /// Delar stycket vid läget (Enter): det nya stycket ärver `a:pPr` och slutformatet.
    pub fn split_para(&mut self, p: Pos) -> Pos {
        let p = self.clamp(p);
        let i = self.split_at(p.para, p.ch);
        let para = &mut self.paras[p.para];
        para.touch();
        let rest: Vec<Item> = para.items.drain(i..).collect();
        // Slutformatet för det första stycket: formateringen där man stod.
        let new = TPara { ppr: para.ppr.clone(), items: rest, end: para.end.clone(), orig: None };
        self.paras.insert(p.para + 1, new);
        Pos { para: p.para + 1, ch: 0 }
    }

    /// Radbrytning i stycket (Skift+Enter).
    pub fn insert_break(&mut self, p: Pos) -> Pos {
        let p = self.clamp(p);
        let rpr = self.rpr_at(p).map(|r| r.to_xml()).unwrap_or_default();
        let tag = q(&self.a, "br");
        let Some(br) = self.frag(&format!("<{tag}>{rpr}</{tag}>")) else { return p };
        let i = self.split_at(p.para, p.ch);
        let para = &mut self.paras[p.para];
        para.touch();
        para.items.insert(i, Item::Break(br));
        Pos { para: p.para, ch: p.ch + 1 }
    }

    /// Tar bort texten mellan två lägen. Ger startläget.
    pub fn delete(&mut self, a: Pos, b: Pos) -> Pos {
        let (a, b) = if a <= b { (self.clamp(a), self.clamp(b)) } else { (self.clamp(b), self.clamp(a)) };
        if a == b {
            return a;
        }
        if a.para == b.para {
            // Början först: en delning där flyttar item efter den.
            let i = self.split_at(a.para, a.ch);
            let j = self.split_at(a.para, b.ch);
            let para = &mut self.paras[a.para];
            para.touch();
            // Item som inte upptar tecken (formler) mellan lägena följer med bort bara om de omges helt.
            para.items.drain(i..j);
            para.items.retain(|it| !matches!(it, Item::Run { text, .. } if text.is_empty()));
            return a;
        }
        let j = self.split_at(b.para, b.ch);
        let tail: Vec<Item> = self.paras[b.para].items.drain(j..).collect();
        // (Olika stycken: delningarna påverkar inte varandra.)
        let end = self.paras[b.para].end.clone();
        let i = self.split_at(a.para, a.ch);
        let para = &mut self.paras[a.para];
        para.touch();
        para.items.truncate(i);
        para.items.extend(tail);
        para.items.retain(|it| !matches!(it, Item::Run { text, .. } if text.is_empty()));
        if end.is_some() {
            para.end = end;
        }
        self.paras.drain(a.para + 1..=b.para);
        a
    }

    /// Läget ett tecken till vänster/höger (över styckegränser).
    pub fn step(&self, p: Pos, forward: bool) -> Pos {
        let p = self.clamp(p);
        if forward {
            if p.ch < self.paras[p.para].len() {
                Pos { para: p.para, ch: p.ch + 1 }
            } else if p.para + 1 < self.paras.len() {
                Pos { para: p.para + 1, ch: 0 }
            } else {
                p
            }
        } else if p.ch > 0 {
            Pos { para: p.para, ch: p.ch - 1 }
        } else if p.para > 0 {
            Pos { para: p.para - 1, ch: self.paras[p.para - 1].len() }
        } else {
            p
        }
    }

    /// Nästa/föregående ordgräns.
    pub fn word(&self, p: Pos, forward: bool) -> Pos {
        let p = self.clamp(p);
        let chars: Vec<char> = self.paras[p.para].text().chars().collect();
        let mut i = p.ch;
        if forward {
            if i >= chars.len() {
                return self.step(p, true);
            }
            while i < chars.len() && chars[i].is_alphanumeric() {
                i += 1;
            }
            while i < chars.len() && !chars[i].is_alphanumeric() {
                i += 1;
            }
        } else {
            if i == 0 {
                return self.step(p, false);
            }
            while i > 0 && !chars[i - 1].is_alphanumeric() {
                i -= 1;
            }
            while i > 0 && chars[i - 1].is_alphanumeric() {
                i -= 1;
            }
        }
        Pos { para: p.para, ch: i }
    }

    /// Texten mellan två lägen.
    pub fn slice(&self, a: Pos, b: Pos) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for pi in a.para..=b.para.min(self.paras.len() - 1) {
            let t: Vec<char> = self.paras[pi].text().chars().collect();
            let from = if pi == a.para { a.ch.min(t.len()) } else { 0 };
            let to = if pi == b.para { b.ch.min(t.len()) } else { t.len() };
            out.extend(&t[from..to.max(from)]);
            if pi != b.para {
                out.push('\n');
            }
        }
        out
    }

    /// Nivå upp/ned (Tab/Skift+Tab) för styckena mellan lägena (0–8).
    pub fn indent(&mut self, a: Pos, b: Pos, delta: i64) {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let tag = q(&self.a, "pPr");
        for pi in a.para..=b.para.min(self.paras.len() - 1) {
            let lvl = (self.paras[pi].level() + delta).clamp(0, 8);
            if lvl == self.paras[pi].level() {
                continue;
            }
            if self.paras[pi].ppr.is_none() {
                self.paras[pi].ppr = self.frag(&format!("<{tag}/>"));
            }
            let para = &mut self.paras[pi];
            para.touch();
            if let Some(ppr) = &mut para.ppr {
                if lvl == 0 {
                    ppr.remove_attr("lvl");
                } else {
                    ppr.set_attr("lvl", &lvl.to_string());
                }
                // Ett eget indrag på stycket skulle annars ta över nivåns.
                ppr.remove_attr("marL");
                ppr.remove_attr("indent");
            }
        }
    }

    /// Slår på/av ett körningsattribut (`b`, `i`, `u`) mellan lägena. Utan markering gäller det ordet
    /// vid markören. Ger det nya värdet.
    pub fn toggle(&mut self, a: Pos, b: Pos, attr: &str) -> bool {
        let (mut a, mut b) = if a <= b { (self.clamp(a), self.clamp(b)) } else { (self.clamp(b), self.clamp(a)) };
        if a == b {
            a = self.word_start(a);
            b = self.word_end(b);
        }
        let on_val = if attr == "u" { "sng" } else { "1" };
        let is_on = |r: &Option<XElem>| r.as_ref().and_then(|r| r.attr(attr)).is_some_and(|v| v != "0" && v != "none" && v != "false");
        // Splitta vid gränserna och samla körningarna i intervallet.
        let mut ranges = Vec::new();
        for pi in a.para..=b.para {
            let len = self.paras[pi].len();
            let from = if pi == a.para { a.ch } else { 0 };
            let to = if pi == b.para { b.ch } else { len };
            if from >= to {
                continue;
            }
            let i = self.split_at(pi, from);
            let j = self.split_at(pi, to);
            ranges.push((pi, i, j));
        }
        let all_on = ranges.iter().all(|&(pi, i, j)| self.paras[pi].items[i..j].iter().all(|it| !matches!(it, Item::Run { rpr, .. } if !is_on(rpr))));
        let value = !all_on;
        let tag = q(&self.a, "rPr");
        let empty = self.frag(&format!("<{tag}/>"));
        for (pi, i, j) in ranges {
            let para = &mut self.paras[pi];
            para.touch();
            for it in &mut para.items[i..j] {
                if let Item::Run { rpr, .. } = it {
                    if rpr.is_none() {
                        *rpr = empty.clone();
                    }
                    if let Some(r) = rpr {
                        if value {
                            r.set_attr(attr, on_val);
                        } else if attr == "u" {
                            r.set_attr("u", "none");
                        } else {
                            r.set_attr(attr, "0");
                        }
                    }
                }
            }
        }
        value
    }

    fn word_start(&self, p: Pos) -> Pos {
        let chars: Vec<char> = self.paras[p.para].text().chars().collect();
        let mut i = p.ch.min(chars.len());
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        Pos { para: p.para, ch: i }
    }

    fn word_end(&self, p: Pos) -> Pos {
        let chars: Vec<char> = self.paras[p.para].text().chars().collect();
        let mut i = p.ch.min(chars.len());
        while i < chars.len() && chars[i].is_alphanumeric() {
            i += 1;
        }
        Pos { para: p.para, ch: i }
    }

    /// Styckena som XML (`a:p`). Orörda stycken skrivs som de var.
    fn paras_xml(&self) -> Vec<XElem> {
        let mut out = Vec::new();
        for p in &self.paras {
            if let Some(o) = &p.orig {
                out.push(o.clone());
                continue;
            }
            let mut s = String::new();
            let ptag = q(&self.a, "p");
            s.push_str(&format!("<{ptag}>"));
            if let Some(ppr) = &p.ppr {
                s.push_str(&ppr.to_xml());
            }
            for it in &p.items {
                match it {
                    Item::Run { rpr, text } => {
                        if text.is_empty() {
                            continue;
                        }
                        let (r, t) = (q(&self.a, "r"), q(&self.a, "t"));
                        s.push_str(&format!("<{r}>{}<{t}>{}</{t}></{r}>", rpr.as_ref().map(XElem::to_xml).unwrap_or_default(), edit_xml::escape(text)));
                    }
                    Item::Break(e) | Item::Field(e) => s.push_str(&e.to_xml()),
                    Item::Other(n) => match n {
                        XNode::Elem(e) => s.push_str(&e.to_xml()),
                        XNode::Other(t) | XNode::Text(t) => s.push_str(t),
                    },
                }
            }
            if let Some(e) = &p.end {
                s.push_str(&e.to_xml());
            }
            s.push_str(&format!("</{ptag}>"));
            if let Some(e) = self.frag(&s) {
                out.push(e);
            }
        }
        out
    }

    /// Skriver styckena in i formens `p:txBody` (skapas om den saknas; `bodyPr` och `lstStyle` behålls).
    pub fn write_into(&self, sp: &mut XElem, p_prefix: &str) {
        if sp.child("p:txBody").is_none() {
            let a = &self.a;
            let tag = q(p_prefix, "txBody");
            let Some(body) = self.frag(&format!("<{tag}><{}/><{}/></{tag}>", q(a, "bodyPr"), q(a, "lstStyle"))) else { return };
            // `p:txBody` efter `p:style` och före `p:extLst`.
            sp.insert_before(body, &["p:extLst"]);
        }
        let paras = self.paras_xml();
        let Some(body) = sp.child_mut("p:txBody") else { return };
        let at = body.index_of("a:p").unwrap_or(body.children.len());
        body.remove_children("a:p");
        let at = at.min(body.children.len());
        for (k, p) in paras.into_iter().enumerate() {
            body.children.insert(at + k, XNode::Elem(p));
        }
    }
}

/// Byter formens text mot ren text (en rad = ett stycke), med formateringen från första körningen.
pub fn set_plain_text(sp: &mut XElem, scope: &[(String, String)], p: &str, a: &str, text: &str) -> Result<(), String> {
    let old = TextModel::from_body(sp.child("p:txBody"), scope, a);
    if old.text() == text {
        return Ok(());
    }
    // Formateringen från första körningen och första stycket.
    let first = old.paras.first();
    let rpr = old.paras.iter().flat_map(|p| p.items.iter()).find_map(|it| if let Item::Run { rpr, .. } = it { Some(rpr.clone()) } else { None }).flatten();
    let (ppr, end) = (first.and_then(|p| p.ppr.clone()), first.and_then(|p| p.end.clone()));
    let mut m = old.clone();
    m.paras = text
        .split('\n')
        .map(|line| {
            let line = line.replace('\r', "");
            let items = if line.is_empty() { Vec::new() } else { vec![Item::Run { rpr: rpr.clone(), text: line }] };
            TPara { ppr: ppr.clone(), items, end: end.clone(), orig: None }
        })
        .collect();
    m.write_into(sp, p);
    Ok(())
}

// ------------------------------------------------------------------ markörlägen

/// Markörgeometrin för en form under textredigering.
pub struct Carets {
    /// Per stycke: (tecken, x, radnummer, baslinje, uppstapel, nedstapel) i textrutans koordinater.
    pub stops: Vec<Vec<(usize, f32, usize, f32, f32, f32)>>,
    /// Textrutan → bilden (punkter).
    pub t: Affine,
}

/// Tolkar ett element ur den förlustfria XML:en som ritmodellens `Node`.
fn to_node(e: &XElem, scope: &[(String, String)]) -> Option<Node> {
    let decls: String = scope.iter().map(|(p, u)| if p.is_empty() { format!(" xmlns=\"{}\"", edit_xml::escape(u)) } else { format!(" xmlns:{p}=\"{}\"", edit_xml::escape(u)) }).collect();
    let n = xml::parse(format!("<kladd-wrap{decls}>{}</kladd-wrap>", e.to_xml()).as_bytes())?;
    n.children.into_iter().next()
}

impl TextModel {
    /// Lägger ut texten i formen (bild `slide`, formen `sp` i ritmodellen) som `render.rs` gör och ger
    /// markörlägena. `None` om formen saknar placering.
    pub fn carets(&self, deck: &Deck, slide: usize, sp: &Node) -> Option<Carets> {
        let sheets = Sheets::for_slide(deck, slide);
        let chain = inherit::shape_chain(&sheets, sp, Owner::Slide);
        let xfrm = chain.iter().find_map(|s| s.path(&["p:spPr", "a:xfrm"]))?;
        let f = frame_of(xfrm, &Affine::IDENTITY)?;
        let geom = match chain.iter().find_map(|s| s.child("p:spPr").and_then(|p| p.children.iter().find(|c| c.name == "a:prstGeom" || c.name == "a:custGeom"))) {
            Some(n) if n.name == "a:prstGeom" => geom::preset(n.attr("prst").unwrap_or("rect"), n.child("a:avLst"), f.w as f64, f.h as f64),
            Some(n) => Some(geom::evaluate(n, None, f.w as f64, f.h as f64)),
            None => geom::preset("rect", None, f.w as f64, f.h as f64),
        };
        let rect = geom.map(|g| g.text_rect).unwrap_or((0.0, 0.0, f.w, f.h));
        let style = chain.iter().find_map(|s| s.child("p:style"));
        let bodies: Vec<&Node> = chain.iter().filter_map(|s| s.path(&["p:txBody", "a:bodyPr"])).collect();
        let bp = richtext::body_props(&bodies);
        let lists = inherit::list_styles(&sheets, &chain, inherit::tx_kind(sp));
        let (font_ref, font_color) = inherit::style_font(&sheets, style);
        let env = TextEnv { sheets: &sheets, lists, font_ref, font_color, font_scale: bp.font_scale, slide_no: deck.slide_number(slide) };
        // Styckena ur modellen (samma XML som skrivs in i bilden), en körning per item.
        let mut body_xml = String::from("<a:txBody>");
        let mut run_map: Vec<Vec<(usize, usize)>> = Vec::new();
        for p in &self.paras_xml() {
            body_xml.push_str(&p.to_xml());
        }
        body_xml.push_str("</a:txBody>");
        let body = to_node(
            &edit_xml::fragment_in(&self.scope, &body_xml.replace("<a:txBody>", &format!("<{}>", q(&self.a, "txBody"))).replace("</a:txBody>", &format!("</{}>", q(&self.a, "txBody")))).into_iter().next()?,
            &self.scope,
        )?;
        let mut fr = richtext::frame(&env, &body, &bp);
        for (pi, para) in fr.paras.iter_mut().enumerate() {
            let Some(tp) = self.paras.get(pi) else { break };
            let lvl = tp.level().clamp(0, 8) as usize;
            let levels = inherit::level_chain(&env.lists, lvl);
            let mut runs = Vec::new();
            let mut map = Vec::new();
            let mut start = 0;
            for it in &tp.items {
                match it {
                    Item::Run { rpr, text } => {
                        let rn = rpr.as_ref().and_then(|r| to_node(r, &self.scope));
                        let (st, _) = env.run_style(rn.as_ref(), &levels);
                        runs.push(text::Run { text: text.clone(), style: st, line_break: false });
                        map.push((start, 0));
                    }
                    Item::Break(e) => {
                        let rn = e.child("a:rPr").and_then(|r| to_node(r, &self.scope));
                        let (st, _) = env.run_style(rn.as_ref(), &levels);
                        runs.push(text::Run { text: String::new(), style: st, line_break: true });
                        map.push((start, 1));
                    }
                    Item::Field(e) => {
                        let rn = e.child("a:rPr").and_then(|r| to_node(r, &self.scope));
                        let (st, _) = env.run_style(rn.as_ref(), &levels);
                        let mut t = e.child("a:t").map(|t| t.text()).unwrap_or_default();
                        if e.attr("type").as_deref() == Some("slidenum") {
                            t = env.slide_no.to_string();
                        }
                        runs.push(text::Run { text: t, style: st, line_break: false });
                        map.push((start, 1));
                    }
                    Item::Other(_) => {}
                }
                start += it.len();
            }
            para.runs = runs;
            run_map.push(map);
        }
        // Placeringen som i `render::Builder::place_text`.
        let (mut l, t, mut r, b) = rect;
        if f.flip_h {
            (l, r) = (f.w - r, f.w - l);
        }
        let (mut tw, mut th) = ((r - l).max(1.0), (b - t).max(1.0));
        let mut extra = bp.rot;
        let vertical = match bp.vert.as_str() {
            "vert" | "eaVert" | "wordArtVert" | "mongolianVert" => Some(90.0),
            "vert270" => Some(270.0),
            _ => None,
        };
        if let Some(v) = vertical {
            extra += v;
            std::mem::swap(&mut tw, &mut th);
        }
        let (_, stops) = text::layout_carets(&fr, tw, th, &deck.fonts);
        let (cx, cy) = (l + (r - l) / 2.0, t + (b - t) / 2.0);
        let local = Affine::translate(-tw / 2.0, -th / 2.0).then(&Affine::rotate(extra)).then(&Affine::translate(cx, cy));
        let mut rot = f.rot;
        if f.flip_v {
            rot += 180.0;
        }
        let ta = Affine::rotate_at(rot, f.w / 2.0, f.h / 2.0).then(&Affine::translate(f.x, f.y)).then(&f.parent);
        let mut per: Vec<Vec<(usize, f32, usize, f32, f32, f32)>> = vec![Vec::new(); self.paras.len()];
        for s in &stops {
            let CaretStop { para, run, ch, x, base, asc, desc, line, .. } = *s;
            let Some(list) = per.get_mut(para) else { continue };
            let pos = if run == usize::MAX {
                self.paras[para].len()
            } else {
                match run_map.get(para).and_then(|m| m.get(run)) {
                    Some(&(start, 0)) => start + ch,
                    Some(&(start, _)) => start + ch.min(1),
                    None => continue,
                }
            };
            list.push((pos, x, line, base, asc, desc));
        }
        Some(Carets { stops: per, t: local.then(&ta) })
    }
}

impl Carets {
    /// Markörens linje (överkant, underkant) i bildens punkter för läget.
    pub fn caret(&self, p: Pos) -> Option<((f32, f32), (f32, f32))> {
        let list = self.stops.get(p.para)?;
        let s = list.iter().find(|s| s.0 == p.ch).or_else(|| list.iter().rev().find(|s| s.0 <= p.ch)).or(list.first())?;
        let (_, x, _, base, asc, desc) = *s;
        Some((self.t.apply(x, base - asc), self.t.apply(x, base + desc)))
    }

    /// Rad och x för läget (för upp/ned).
    fn line_x(&self, p: Pos) -> Option<(usize, f32)> {
        let list = self.stops.get(p.para)?;
        let s = list.iter().find(|s| s.0 == p.ch).or_else(|| list.iter().rev().find(|s| s.0 <= p.ch))?;
        Some((s.2, s.1))
    }

    /// Läget på raden ovanför/nedanför (`dir` −1/+1) närmast samma x. `None` = ingen sådan rad.
    pub fn vertical(&self, p: Pos, dir: i32) -> Option<Pos> {
        let (line, x) = self.line_x(p)?;
        let want = line as i64 + dir as i64;
        if want < 0 {
            return None;
        }
        self.nearest_on_line(want as usize, x)
    }

    fn nearest_on_line(&self, line: usize, x: f32) -> Option<Pos> {
        let mut best: Option<(f32, Pos)> = None;
        for (pi, list) in self.stops.iter().enumerate() {
            for s in list.iter().filter(|s| s.2 == line) {
                let d = (s.1 - x).abs();
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, Pos { para: pi, ch: s.0 }));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// Radens början/slut för läget.
    pub fn line_edge(&self, p: Pos, end: bool) -> Option<Pos> {
        let (line, _) = self.line_x(p)?;
        let list = self.stops.get(p.para)?;
        let on: Vec<_> = list.iter().filter(|s| s.2 == line).collect();
        let s = if end { on.iter().max_by_key(|s| s.0) } else { on.iter().min_by_key(|s| s.0) }?;
        Some(Pos { para: p.para, ch: s.0 })
    }

    /// Läget närmast en punkt (bildens punkter).
    pub fn hit(&self, px: f32, py: f32) -> Option<Pos> {
        let inv = self.t.invert()?;
        let (x, y) = inv.apply(px, py);
        // Raden vars lodräta spann ligger närmast.
        let mut best_line: Option<(f32, usize)> = None;
        for list in &self.stops {
            for s in list {
                let (top, bot) = (s.3 - s.4, s.3 + s.5);
                let d = if y < top { top - y } else if y > bot { y - bot } else { 0.0 };
                if best_line.is_none_or(|b| d < b.0) {
                    best_line = Some((d, s.2));
                }
            }
        }
        self.nearest_on_line(best_line?.1, x)
    }

    /// Markeringens rektanglar (fyra hörn i bildens punkter) mellan två lägen.
    pub fn selection(&self, a: Pos, b: Pos) -> Vec<[(f32, f32); 4]> {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut lines: std::collections::BTreeMap<usize, (f32, f32, f32, f32)> = Default::default();
        for (pi, list) in self.stops.iter().enumerate() {
            for (k, s) in list.iter().enumerate() {
                let p = Pos { para: pi, ch: s.0 };
                if p < a || p >= b {
                    continue;
                }
                // Klustrets högerkant: nästa läge på samma rad.
                let x1 = list.get(k + 1).filter(|n| n.2 == s.2).map_or(s.1 + 4.0, |n| n.1);
                let e = lines.entry(s.2).or_insert((s.1, x1, s.3 - s.4, s.3 + s.5));
                e.0 = e.0.min(s.1);
                e.1 = e.1.max(x1);
            }
        }
        lines.values().map(|&(x0, x1, y0, y1)| [self.t.apply(x0, y0), self.t.apply(x1, y0), self.t.apply(x1, y1), self.t.apply(x0, y1)]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

    fn model(body: &str) -> (TextModel, edit_xml::XDoc) {
        let doc = edit_xml::parse(format!("<p:sp {NS}><p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>").as_bytes()).unwrap();
        (TextModel::from_body(doc.root.child("p:txBody"), &doc.root_scope(), "a"), doc)
    }

    fn write(m: &TextModel, doc: &edit_xml::XDoc) -> String {
        let mut sp = doc.root.clone();
        m.write_into(&mut sp, "p");
        sp.to_xml()
    }

    #[test]
    fn skriva_behaller_formateringen_och_orora_stycken() {
        let p2 = r#"<a:p><a:pPr lvl="1"/><a:r><a:rPr lang="sv-SE"  b="1"/><a:t>Orört</a:t></a:r></a:p>"#;
        let (mut m, doc) = model(&format!(r#"<a:p><a:r><a:rPr lang="sv-SE" sz="2400" i="1"/><a:t>Hej</a:t></a:r><a:endParaRPr lang="sv-SE"/></a:p>{p2}"#));
        let p = m.insert(Pos { para: 0, ch: 3 }, " du");
        assert_eq!(p, Pos { para: 0, ch: 6 });
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:r><a:rPr lang="sv-SE" sz="2400" i="1"/><a:t>Hej du</a:t></a:r><a:endParaRPr lang="sv-SE"/>"#), "{out}");
        assert!(out.contains(p2), "orört stycke skrivs byte för byte: {out}");
    }

    #[test]
    fn enter_arver_ppr_och_backsteg_slar_ihop() {
        let (mut m, doc) = model(r#"<a:p><a:pPr lvl="2" algn="ctr"/><a:r><a:rPr b="1"/><a:t>Ettva</a:t></a:r></a:p>"#);
        let p = m.split_para(Pos { para: 0, ch: 3 });
        assert_eq!(m.paras.len(), 2);
        assert_eq!(m.text(), "Ett\nva");
        assert_eq!(m.paras[1].level(), 2);
        let out = write(&m, &doc);
        assert_eq!(out.matches(r#"<a:pPr lvl="2" algn="ctr"/>"#).count(), 2, "{out}");
        assert_eq!(out.matches(r#"<a:rPr b="1"/>"#).count(), 2, "{out}");
        let back = m.step(p, false);
        m.delete(back, p);
        assert_eq!(m.text(), "Ettva");
        assert_eq!(m.paras.len(), 1);
    }

    #[test]
    fn radbrytning_nivå_och_fetstil() {
        let (mut m, doc) = model(r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>ett två</a:t></a:r></a:p>"#);
        let p = m.insert_break(Pos { para: 0, ch: 3 });
        assert_eq!(p.ch, 4);
        assert_eq!(m.text(), "ett\n två");
        m.indent(Pos::default(), Pos::default(), 1);
        assert_eq!(m.paras[0].level(), 1);
        assert!(m.toggle(Pos { para: 0, ch: 0 }, Pos { para: 0, ch: 3 }, "b"));
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:rPr lang="sv-SE" b="1"/><a:t>ett</a:t>"#), "{out}");
        assert!(out.contains(r#"<a:br><a:rPr lang="sv-SE"/></a:br>"#), "{out}");
        assert!(out.contains(r#"<a:pPr lvl="1"/>"#), "{out}");
        // Av igen.
        assert!(!m.toggle(Pos { para: 0, ch: 0 }, Pos { para: 0, ch: 3 }, "b"));
    }

    #[test]
    fn backsteg_och_fetstil_mitt_i_en_korning() {
        let (mut m, doc) = model(r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>Hej på dig</a:t></a:r><a:r><a:rPr lang="sv-SE" i="1"/><a:t> du</a:t></a:r></a:p>"#);
        // Backsteg sist i en körning, mitt i en körning och över två körningar.
        let p = m.delete(Pos { para: 0, ch: 12 }, Pos { para: 0, ch: 13 });
        assert_eq!((m.text().as_str(), p.ch), ("Hej på dig d", 12));
        m.delete(Pos { para: 0, ch: 3 }, Pos { para: 0, ch: 4 });
        assert_eq!(m.text(), "Hejpå dig d");
        m.delete(Pos { para: 0, ch: 8 }, Pos { para: 0, ch: 10 });
        assert_eq!(m.text(), "Hejpå did");
        // Fetstil på ordet i mitten: tre körningar.
        let (mut m, doc2) = model(r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>ett två tre</a:t></a:r></a:p>"#);
        m.toggle(Pos { para: 0, ch: 4 }, Pos { para: 0, ch: 7 }, "b");
        let out = write(&m, &doc2);
        assert!(out.contains(r#"<a:r><a:rPr lang="sv-SE"/><a:t>ett </a:t></a:r><a:r><a:rPr lang="sv-SE" b="1"/><a:t>två</a:t></a:r><a:r><a:rPr lang="sv-SE"/><a:t> tre</a:t></a:r>"#), "{out}");
        let _ = doc;
    }

    #[test]
    fn skriva_i_tomt_stycke_med_slutformat_ger_giltig_xml() {
        let (mut m, doc) = model(r#"<a:p><a:endParaRPr lang="sv-SE" dirty="0"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:endParaRPr></a:p>"#);
        m.insert(Pos::default(), "x");
        let out = write(&m, &doc);
        assert!(out.contains(r#"<a:r><a:rPr lang="sv-SE"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:rPr><a:t>x</a:t></a:r>"#), "{out}");
        // Strikt kontroll av sluttaggarna.
        let mut r = quick_xml::Reader::from_str(&out);
        loop {
            match r.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("{e}: {out}"),
            }
        }
    }

    #[test]
    fn tom_kropp_och_ren_text() {
        let doc = edit_xml::parse(format!("<p:sp {NS}><p:nvSpPr/><p:spPr/></p:sp>").as_bytes()).unwrap();
        let mut sp = doc.root.clone();
        set_plain_text(&mut sp, &doc.root_scope(), "p", "a", "Rad 1\nRad <2>").unwrap();
        let out = sp.to_xml();
        assert!(out.contains("<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Rad 1</a:t></a:r></a:p><a:p><a:r><a:t>Rad &lt;2&gt;</a:t></a:r></a:p></p:txBody>"), "{out}");
        let m = TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), "a");
        assert_eq!(m.text(), "Rad 1\nRad <2>");
        assert_eq!(m.slice(Pos { para: 0, ch: 4 }, Pos { para: 1, ch: 3 }), "1\nRad");
    }
}
