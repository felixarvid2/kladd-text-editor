//! Matematik i LaTeX-syntax: `$…$` (löpande) och `$$…$$` (egen rad) i Markdown.
//!
//! * `parse`: LaTeX till syntaxträd (`ast`).
//! * `layout`: träd till glyfer och linjer med STIX Two Math (`font`).
//! * `latex`, `mathml`, `omml`: träd till/från andra format.
//! * `raster`: rastrerar en utläggning till en alfamask (galleriet, förhandsvisningen).
//! * `gui`: texturer för egui.

#![allow(dead_code)]

pub mod ast;
pub mod font;
pub mod gui;
pub mod insert;
pub mod latex;
pub mod layout;
pub mod mathml;
pub mod omml;
pub mod parse;
pub mod preview;
pub mod raster;
pub mod symbols;

#[cfg(test)]
mod export_tests;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use layout::Layout;

/// Utlagd formel och dess träd, delad via cachen.
pub struct Formula {
    pub nodes: Vec<ast::Node>,
    pub layout: Layout,
}

type Cache = HashMap<(String, bool), Option<Arc<Formula>>>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Tolkar och lägger ut `src`. `None` om LaTeX-koden inte går att tolka (då visas källtexten).
/// Resultatet cachas; cachen töms när den blir stor.
pub fn formula(src: &str, display: bool) -> Option<Arc<Formula>> {
    let key = (src.to_owned(), display);
    if let Ok(c) = CACHE.lock() {
        if let Some(f) = c.get(&key) {
            return f.clone();
        }
    }
    let f = parse::parse(src).ok().and_then(|nodes| {
        let layout = layout::layout(&nodes, display)?;
        Some(Arc::new(Formula { nodes, layout }))
    });
    if let Ok(mut c) = CACHE.lock() {
        if c.len() > 512 {
            c.clear();
        }
        c.insert(key, f.clone());
    }
    f
}

/// En formel i Markdown-texten: byteintervall för hela `$…$`/`$$…$$` och LaTeX-koden.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub range: std::ops::Range<usize>,
    pub src: String,
    pub display: bool,
}

/// Formlerna i `text` enligt Markdown-tolkaren (inte i kod, och med `\$` som vanligt dollartecken).
pub fn spans(text: &str) -> Vec<Span> {
    use pulldown_cmark::{Event, Options, Parser};
    if !text.contains('$') {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (ev, r) in Parser::new_ext(text, Options::ENABLE_MATH | Options::ENABLE_TABLES).into_offset_iter() {
        match ev {
            Event::InlineMath(s) => out.push(Span { range: r, src: s.trim().to_owned(), display: false }),
            Event::DisplayMath(s) => out.push(Span { range: r, src: s.trim().to_owned(), display: true }),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_inline_and_display_math() {
        let t = "Text $x^2$ och\n\n$$\\frac{a}{b}$$\n\nslut $a_i$.";
        let s = spans(t);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].src, "x^2");
        assert!(!s[0].display);
        assert_eq!(&t[s[0].range.clone()], "$x^2$");
        assert!(s[1].display);
        assert_eq!(s[1].src, "\\frac{a}{b}");
        assert_eq!(s[2].src, "a_i");
    }

    #[test]
    fn money_code_and_escapes_are_not_math() {
        assert!(spans("Det kostar $5 och $6 idag").is_empty());
        assert!(spans("`$x$`\n\n```\n$y$\n```").is_empty());
        assert!(spans(r"Pris \$x\$ kr").is_empty());
        assert_eq!(spans("$$ x $$").len(), 1);
    }

    #[test]
    fn display_math_can_span_lines() {
        let s = spans("$$\na = b\n$$\n");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].src, "a = b");
    }

    #[test]
    fn cache_returns_the_same_formula() {
        let a = formula("x^2", false).unwrap();
        let b = formula("x^2", false).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(formula("\\unknown", false).is_none());
        assert!(a.layout.w > 0.0);
    }
}
