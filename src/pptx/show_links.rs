//! Länkar och åtgärdsknappar i bildspelet: `a:hlinkClick` och `a:hlinkHover` på former (`p:cNvPr`) och på
//! textkörningar (`a:rPr`). Rent: läser bildens XML och ritmodellens glyfkörningar och ger områden med
//! åtgärder; `presenter.rs` träffar på dem och utför åtgärden.
//!
//! Textlänkar hittas genom att formens körningstext (utan blanktecken) letas upp i glyfkörningarna som ligger
//! inom formen, i ordning: glyfkörningarna känner inte till länkar, men deras text och lägen går att para ihop
//! med körningarna i XML:en. Det som inte hittas (punkttecken, fält) hoppas över.

use super::edit_ops::{self, ShapeInfo};
use super::find::{self, SlideText};
use super::model::Deck;
use super::package;
use super::xml::Node;

/// Vad en länk gör.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Next,
    Prev,
    First,
    Last,
    /// Senast visade bilden.
    LastViewed,
    End,
    /// Bilden (index i presentationen).
    Slide(usize),
    /// Webbadress eller `mailto:`.
    Url(String),
}

/// Ett områdes åtgärder: vid klick och när pekaren kommer över.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Link {
    pub click: Option<Action>,
    pub hover: Option<Action>,
}

impl Link {
    fn is_some(&self) -> bool {
        self.click.is_some() || self.hover.is_some()
    }
}

/// Ett område på bilden med en länk.
#[derive(Clone, Debug)]
pub struct Area {
    /// Formens id (för att dölja länken medan formen är dold av en animering).
    pub shape: u32,
    /// Hela formen (`None` för en textlänk).
    form: Option<ShapeInfo>,
    /// Textlänkens rektanglar (x0, y0, x1, y1) i punkter på bilden.
    rects: Vec<[f32; 4]>,
    pub link: Link,
}

impl Area {
    fn hit(&self, x: f32, y: f32) -> bool {
        match &self.form {
            Some(f) => f.hit(x, y, 0.0),
            None => self.rects.iter().any(|r| x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]),
        }
    }
}

/// En bilds länkar.
#[derive(Clone, Debug, Default)]
pub struct SlideLinks {
    /// Formlänkar i ritordning, sedan textlänkar.
    areas: Vec<Area>,
}

impl SlideLinks {
    /// Området under (`x`, `y`) i punkter på bilden: textlänkar före formlänkar, översta formen först.
    /// `visible` avgör om formen med id:t syns (en animerad form som ännu inte visats kan inte klickas).
    pub fn at(&self, x: f32, y: f32, visible: &dyn Fn(u32) -> bool) -> Option<(usize, &Area)> {
        self.areas.iter().enumerate().rev().find(|(_, a)| visible(a.shape) && a.hit(x, y))
    }
}

/// Åtgärden som en `a:hlinkClick`/`a:hlinkHover` står för.
fn action_of(deck: &Deck, slide: usize, h: &Node) -> Option<Action> {
    let action = h.attr("action").unwrap_or("");
    let id = h.attr("r:id").unwrap_or("");
    if let Some(jump) = action.strip_prefix("ppaction://hlinkshowjump?jump=") {
        return match jump {
            "nextslide" => Some(Action::Next),
            "previousslide" => Some(Action::Prev),
            "firstslide" => Some(Action::First),
            "lastslide" => Some(Action::Last),
            "lastslideviewed" => Some(Action::LastViewed),
            "endshow" => Some(Action::End),
            _ => None,
        };
    }
    let rel = || deck.slides.get(slide).and_then(|s| package::rel(&s.rels, id));
    if action.starts_with("ppaction://hlinksldjump") {
        let target = rel().filter(|r| r.kind == "slide" && !r.external)?.target.trim_start_matches('/');
        return deck.slides.iter().position(|s| s.part.trim_start_matches('/').eq_ignore_ascii_case(target)).map(Action::Slide);
    }
    // Andra åtgärder (`hlinkfile`, `hlinkpres`, `program`, `macro`, `noaction` …) utförs inte.
    if action.starts_with("ppaction://") || id.is_empty() {
        return None;
    }
    let r = rel().filter(|r| r.external)?;
    safe_url(&r.target).map(Action::Url)
}

/// Bara webbadresser och e-post öppnas; en presentation ska inte kunna starta filer eller program.
fn safe_url(url: &str) -> Option<String> {
    let u = url.trim();
    let low = u.to_ascii_lowercase();
    ["http://", "https://", "mailto:"].iter().any(|p| low.starts_with(p)).then(|| u.to_owned())
}

fn link_of(deck: &Deck, slide: usize, holder: &Node) -> Link {
    Link {
        click: holder.child("a:hlinkClick").and_then(|h| action_of(deck, slide, h)),
        hover: holder.child("a:hlinkHover").and_then(|h| action_of(deck, slide, h)),
    }
}

/// Bildens länkar. Ritmodellen byggs bara om någon textlänk finns.
pub fn of_slide(deck: &Deck, slide: usize) -> SlideLinks {
    let mut out = SlideLinks::default();
    let Some(tree) = deck.slides.get(slide).and_then(|s| s.root.path(&["p:cSld", "p:spTree"])) else { return out };
    let mut any = false;
    tree.walk(&mut |n| any |= matches!(n.name.as_str(), "a:hlinkClick" | "a:hlinkHover"));
    if !any {
        return out;
    }
    let infos = edit_ops::shapes(deck, slide);
    let mut text_areas = Vec::new();
    let mut text: Option<SlideText> = None;
    for info in infos.iter().filter(|i| !i.empty_ph) {
        let Some(c) = tree.children.get(info.index) else { continue };
        // Textlänkarna i formen (och i dess tabellceller och gruppbarn).
        let mut runs: Vec<(String, Link)> = Vec::new();
        c.walk(&mut |n| {
            if n.name == "a:r" {
                if let (Some(rpr), Some(t)) = (n.child("a:rPr"), n.child("a:t")) {
                    let link = link_of(deck, slide, rpr);
                    if link.is_some() {
                        runs.push((t.text.clone(), link));
                    }
                }
            }
        });
        if !runs.is_empty() {
            let st = text.get_or_insert_with(|| find::slide_text(&super::render::slide_scene(deck, slide)));
            for (rects, link) in text_link_rects(st, info, &runs) {
                text_areas.push(Area { shape: info.id, form: None, rects, link });
            }
        }
        // Formens egen länk (åtgärdsknappar, bilder, former med länk).
        if let Some(cnv) = c.children.first().and_then(|nv| nv.child("p:cNvPr")) {
            let link = link_of(deck, slide, cnv);
            if link.is_some() {
                out.areas.push(Area { shape: info.id, form: Some(info.clone()), rects: Vec::new(), link });
            }
        }
    }
    // Textlänkarna sist: `at` letar bakifrån, så de går före formlänkarna.
    out.areas.append(&mut text_areas);
    out
}

/// Rektanglarna för textkörningarna med länk i formen `info`: körningens text (utan blanktecken) letas upp
/// i ordning bland glyfkörningarna vars mitt ligger i formen.
fn text_link_rects(st: &SlideText, info: &ShapeInfo, runs: &[(String, Link)]) -> Vec<(Vec<[f32; 4]>, Link)> {
    // Formens tecken (utan blanktecken) med läge i bildens söktext.
    let mut g: Vec<(char, usize)> = Vec::new();
    for r in &st.runs {
        let rects = find::hit_rects(st, r.start, r.end);
        let inside = rects.first().is_some_and(|q| info.hit((q[0] + q[2]) / 2.0, (q[1] + q[3]) / 2.0, 2.0));
        if !inside {
            continue;
        }
        for (o, ch) in st.text[r.start..r.end].char_indices() {
            if !ch.is_whitespace() {
                g.push((lower(ch), r.start + o));
            }
        }
    }
    let mut out = Vec::new();
    let mut cursor = 0;
    for (t, link) in runs {
        let needle: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).map(lower).collect();
        if needle.is_empty() {
            continue;
        }
        let Some(p) = (cursor..g.len().saturating_sub(needle.len() - 1)).find(|&p| g[p..p + needle.len()].iter().map(|x| x.0).eq(needle.iter().copied())) else { continue };
        let (start, last) = (g[p].1, g[p + needle.len() - 1].1);
        let end = last + st.text[last..].chars().next().map_or(1, char::len_utf8);
        let rects = find::hit_rects(st, start, end);
        if !rects.is_empty() {
            out.push((rects, link.clone()));
        }
        cursor = p + needle.len();
    }
    out
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

