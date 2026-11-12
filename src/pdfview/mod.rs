//! PDF-visaren: motor (hayro, trådpool), textlager och metadata (lopdf). Själva vyn och verktygsraden med sidopanel, sök och dialoger ligger i `view` och `chrome`.
#![allow(dead_code)]

pub mod chrome;
pub mod engine;
pub mod meta;
pub mod text;
pub mod view;
