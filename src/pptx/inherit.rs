//! Arvet i PresentationML: platshållare bild → layout → mall (matchning på `type`/`idx`),
//! textstilar per nivå (formens `lstStyle` → layoutens → mallens platshållare → mallens
//! `titleStyle`/`bodyStyle`/`otherStyle` → presentationens `defaultTextStyle`), temats stilmatris
//! via `p:style` (`lnRef`/`fillRef`/`effectRef`/`fontRef`) och färgsammanhanget (`clrMap`/`clrMapOvr`).
//!
//! Funktionerna här svarar bara på "vilken nod gäller", inte "hur ritas den"; det gör `render.rs`.

use super::color::{self, ColorCtx, Rgba};
use super::model::{Deck, Layout, Master, Slide, Theme};
use super::xml::Node;

/// Vilket blad en form kommer från.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Slide,
    Layout,
    Master,
}

/// Bilden, dess layout, mall och tema, och färgsammanhanget.
pub struct Sheets<'a> {
    pub deck: &'a Deck,
    pub slide: Option<&'a Slide>,
    pub layout: Option<&'a Layout>,
    pub master: Option<&'a Master>,
    pub theme: &'a Theme,
    pub colors: ColorCtx,
}

fn empty_theme() -> &'static Theme {
    static T: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    T.get_or_init(Theme::default)
}

impl<'a> Sheets<'a> {
    pub fn for_slide(deck: &'a Deck, i: usize) -> Sheets<'a> {
        let slide = deck.slides.get(i);
        let layout = deck.layout_of(i);
        let master = layout.and_then(|l| deck.master_of_layout(l));
        Self::new(deck, slide, layout, master)
    }

    pub fn new(deck: &'a Deck, slide: Option<&'a Slide>, layout: Option<&'a Layout>, master: Option<&'a Master>) -> Sheets<'a> {
        let theme: &'a Theme = match master.and_then(|m| deck.theme_of_master(m)) {
            Some(t) => t,
            None => empty_theme(),
        };
        let colors = deck.color_ctx(slide, layout, master);
        Sheets { deck, slide, layout, master, theme, colors }
    }

    /// Bladets formträd.
    pub fn tree(&self, owner: Owner) -> Option<&'a Node> {
        let root = match owner {
            Owner::Slide => &self.slide?.root,
            Owner::Layout => &self.layout?.root,
            Owner::Master => &self.master?.root,
        };
        root.path(&["p:cSld", "p:spTree"])
    }

    pub fn root(&self, owner: Owner) -> Option<&'a Node> {
        Some(match owner {
            Owner::Slide => &self.slide?.root,
            Owner::Layout => &self.layout?.root,
            Owner::Master => &self.master?.root,
        })
    }

    /// Bladets relationer.
    pub fn rels(&self, owner: Owner) -> &'a [super::package::Rel] {
        match owner {
            Owner::Slide => self.slide.map(|s| s.rels.as_slice()).unwrap_or(&[]),
            Owner::Layout => self.layout.map(|s| s.rels.as_slice()).unwrap_or(&[]),
            Owner::Master => self.master.map(|s| s.rels.as_slice()).unwrap_or(&[]),
        }
    }

    pub fn color(&self, n: &Node, ph: Option<Rgba>) -> Option<Rgba> {
        color::resolve(n, &self.colors, ph)
    }
}

/// En platshållare: typ (standard `obj`) och index.
#[derive(Clone, Debug, PartialEq)]
pub struct Ph {
    pub typ: String,
    pub idx: Option<String>,
}

/// `p:nvPr` för alla formtyper.
pub fn nv_pr(shape: &Node) -> Option<&Node> {
    shape
        .children
        .iter()
        .find(|c| matches!(c.name.as_str(), "p:nvSpPr" | "p:nvPicPr" | "p:nvGraphicFramePr" | "p:nvCxnSpPr" | "p:nvGrpSpPr" | "p:nvContentPartPr"))
        .and_then(|n| n.child("p:nvPr"))
}

/// Formens platshållare, om den är en.
pub fn placeholder(shape: &Node) -> Option<Ph> {
    let ph = nv_pr(shape)?.child("p:ph")?;
    Some(Ph { typ: ph.attr("type").unwrap_or("obj").to_owned(), idx: ph.attr("idx").map(str::to_owned) })
}

/// Typen som mallens platshållare har för en layouttyp.
fn master_type(t: &str) -> &str {
    match t {
        "ctrTitle" | "title" => "title",
        "dt" => "dt",
        "ftr" => "ftr",
        "sldNum" => "sldNum",
        "hdr" => "hdr",
        _ => "body",
    }
}

fn same_kind(a: &str, b: &str) -> bool {
    a == b || master_type(a) == "title" && master_type(b) == "title"
}

/// Alla formers platshållare i ett träd (rekursivt genom grupper).
fn placeholders(tree: &Node) -> Vec<(&Node, Ph)> {
    let mut out = Vec::new();
    for c in &tree.children {
        if c.name == "p:grpSp" {
            out.extend(placeholders(c));
        } else if let Some(ph) = placeholder(c) {
            out.push((c, ph));
        }
    }
    out
}

/// Layoutens platshållare för en bilds platshållare: samma `idx`, annars samma typ.
pub fn find_layout_ph<'a>(layout_tree: &'a Node, ph: &Ph) -> Option<&'a Node> {
    let all = placeholders(layout_tree);
    if let Some(idx) = &ph.idx {
        if let Some((n, _)) = all.iter().find(|(_, p)| p.idx.as_ref() == Some(idx) && (ph.typ == "obj" || same_kind(&p.typ, &ph.typ) || master_type(&p.typ) == master_type(&ph.typ))) {
            return Some(n);
        }
        if let Some((n, _)) = all.iter().find(|(_, p)| p.idx.as_ref() == Some(idx)) {
            return Some(n);
        }
    }
    if ph.typ != "obj" || ph.idx.is_none() {
        if let Some((n, _)) = all.iter().find(|(_, p)| p.typ == ph.typ) {
            return Some(n);
        }
        if let Some((n, _)) = all.iter().find(|(_, p)| same_kind(&p.typ, &ph.typ)) {
            return Some(n);
        }
    }
    None
}

/// Mallens platshållare för en typ.
pub fn find_master_ph<'a>(master_tree: &'a Node, typ: &str) -> Option<&'a Node> {
    let want = master_type(typ);
    placeholders(master_tree).into_iter().find(|(_, p)| master_type(&p.typ) == want && (p.typ == want || want == "body")).map(|(n, _)| n)
}

/// Kedjan av formnoder för en form: formen själv, sedan motsvarande platshållare i layout och mall.
pub fn shape_chain<'a>(sheets: &Sheets<'a>, shape: &'a Node, owner: Owner) -> Vec<&'a Node> {
    let mut chain = vec![shape];
    let Some(ph) = placeholder(shape) else { return chain };
    let mut typ = ph.typ.clone();
    if owner == Owner::Slide {
        if let Some(lt) = sheets.tree(Owner::Layout) {
            if let Some(l) = find_layout_ph(lt, &ph) {
                chain.push(l);
                if let Some(lp) = placeholder(l) {
                    typ = lp.typ;
                }
            }
        }
    }
    if owner != Owner::Master {
        if let Some(mt) = sheets.tree(Owner::Master) {
            if let Some(m) = find_master_ph(mt, &typ) {
                chain.push(m);
            }
        }
    }
    chain
}

/// Vilken av mallens textstilar en form använder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TxKind {
    Title,
    Body,
    Other,
}

pub fn tx_kind(shape: &Node) -> TxKind {
    match placeholder(shape) {
        Some(ph) => match ph.typ.as_str() {
            "title" | "ctrTitle" => TxKind::Title,
            "dt" | "ftr" | "sldNum" | "hdr" => TxKind::Other,
            _ => TxKind::Body,
        },
        None => TxKind::Other,
    }
}

/// Listorna med nivåstilar (`a:lstStyle`-liknande) i prioritetsordning för en form.
pub fn list_styles<'a>(sheets: &Sheets<'a>, chain: &[&'a Node], kind: TxKind) -> Vec<&'a Node> {
    let mut out = Vec::new();
    for n in chain {
        if let Some(ls) = n.path(&["p:txBody", "a:lstStyle"]) {
            out.push(ls);
        }
    }
    if let Some(m) = sheets.master {
        let name = match kind {
            TxKind::Title => "p:titleStyle",
            TxKind::Body => "p:bodyStyle",
            TxKind::Other => "p:otherStyle",
        };
        if let Some(s) = m.root.path(&["p:txStyles", name]) {
            out.push(s);
        }
    }
    if let Some(d) = &sheets.deck.default_text_style {
        out.push(d);
    }
    out
}

/// Nivåstilarna (`a:lvlNpPr`) för nivå `lvl` (0-baserad) ur listorna.
pub fn level_chain<'a>(lists: &[&'a Node], lvl: usize) -> Vec<&'a Node> {
    let name = format!("a:lvl{}pPr", lvl.min(8) + 1);
    let mut out = Vec::new();
    for l in lists {
        if let Some(p) = l.child(&name) {
            out.push(p);
        }
        if let Some(p) = l.child("a:defPPr") {
            out.push(p);
        }
    }
    out
}

/// Första attributvärdet i en kedja.
pub fn first_attr<'a>(chain: &[&'a Node], name: &str) -> Option<&'a str> {
    chain.iter().find_map(|n| n.attr(name))
}

/// Första barnet bland namnen i en kedja (t.ex. punktgruppen `buNone|buChar|buAutoNum|buBlip`).
pub fn first_child_of<'a>(chain: &[&'a Node], names: &[&str]) -> Option<&'a Node> {
    chain.iter().find_map(|n| n.children.iter().find(|c| names.contains(&c.name.as_str())))
}

/// Fyllningselementen.
pub const FILLS: &[&str] = &["a:noFill", "a:solidFill", "a:gradFill", "a:blipFill", "a:pattFill", "a:grpFill"];

/// En fyllning eller linje ur temats stilmatris: noden och platshållarfärgen.
pub struct StyleRef<'a> {
    pub node: &'a Node,
    pub ph: Option<Rgba>,
}

/// `p:style/a:fillRef` → temats fyllning (1–999 `fillStyleLst`, 1001– `bgFillStyleLst`).
pub fn style_fill<'a>(sheets: &Sheets<'a>, style: Option<&Node>) -> Option<StyleRef<'a>> {
    let r = style?.child("a:fillRef")?;
    theme_fill(sheets, r.attr_i64("idx").unwrap_or(0), color::first_color(r).and_then(|c| sheets.color(c, None)))
}

pub fn theme_fill<'a>(sheets: &Sheets<'a>, idx: i64, ph: Option<Rgba>) -> Option<StyleRef<'a>> {
    let t = sheets.theme;
    let node = if idx >= 1001 {
        t.bg_fill_styles.get((idx - 1001) as usize)?
    } else if idx >= 1 {
        t.fill_styles.get((idx - 1) as usize)?
    } else {
        return None;
    };
    Some(StyleRef { node, ph })
}

/// `p:style/a:lnRef` → temats linje (`a:ln`).
pub fn style_line<'a>(sheets: &Sheets<'a>, style: Option<&Node>) -> Option<StyleRef<'a>> {
    let r = style?.child("a:lnRef")?;
    let idx = r.attr_i64("idx").unwrap_or(0);
    if idx < 1 {
        return None;
    }
    let node = sheets.theme.line_styles.get((idx - 1) as usize)?;
    Some(StyleRef { node, ph: color::first_color(r).and_then(|c| sheets.color(c, None)) })
}

/// `p:style/a:fontRef`: temats typsnitt (`major`/`minor`) och textfärgen.
pub fn style_font(sheets: &Sheets<'_>, style: Option<&Node>) -> (Option<String>, Option<Rgba>) {
    let Some(r) = style.and_then(|s| s.child("a:fontRef")) else { return (None, None) };
    let font = match r.attr("idx") {
        Some("major") => Some("+mj-lt".to_owned()),
        Some("minor") => Some("+mn-lt".to_owned()),
        _ => None,
    };
    (font, color::first_color(r).and_then(|c| sheets.color(c, None)))
}

/// Bakgrunden för en bild: bildens, layoutens eller mallens `p:bg`, med bladet den kom från.
pub fn background<'a>(sheets: &Sheets<'a>) -> Option<(&'a Node, Owner)> {
    for owner in [Owner::Slide, Owner::Layout, Owner::Master] {
        if let Some(bg) = sheets.root(owner).and_then(|r| r.path(&["p:cSld", "p:bg"])) {
            return Some((bg, owner));
        }
    }
    None
}

/// Ett temats typsnittsnamn (`+mj-lt`, `+mn-ea` …) till ett riktigt namn.
pub fn theme_font(theme: &Theme, name: &str, lang: Option<&str>) -> String {
    let (scheme, slot) = match name {
        "+mj-lt" => (&theme.major, 0),
        "+mn-lt" => (&theme.minor, 0),
        "+mj-ea" => (&theme.major, 1),
        "+mn-ea" => (&theme.minor, 1),
        "+mj-cs" => (&theme.major, 2),
        "+mn-cs" => (&theme.minor, 2),
        other => return other.to_owned(),
    };
    let base = match slot {
        0 => &scheme.latin,
        1 => &scheme.ea,
        _ => &scheme.cs,
    };
    if !base.is_empty() || slot == 0 {
        return base.clone();
    }
    // Tomt ea/cs: skriftens typsnitt efter språket.
    let script = match lang.unwrap_or("").split('-').next().unwrap_or("") {
        "ja" => "Jpan",
        "zh" if lang.is_some_and(|l| l.contains("TW") || l.contains("HK")) => "Hant",
        "zh" => "Hans",
        "ko" => "Hang",
        "he" => "Hebr",
        "ar" => "Arab",
        "th" => "Thai",
        "ru" | "uk" | "bg" => return String::new(),
        _ => "",
    };
    scheme.scripts.get(script).cloned().unwrap_or_default()
}
