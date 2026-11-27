//! PowerPoint (.pptx): läsning, arv, ritmotor och visning.
//!
//! En ritväg för allt: modell (`model.rs`) → visningslista (`scene.rs`, byggd av `render.rs`) →
//! (a) raster med tiny-skia (`raster.rs`: skärm, miniatyrer, jämförelseprov) och (b) PDF med krilla
//! (`pdf.rs`). Text formas och läggs ut en gång i `text.rs`, så skärm och PDF blir identiska.
//! Se `docs/dev/pptx.md` för arkitekturen, proven och vad som återstår.

pub mod charts;
pub mod color;
pub mod compare;
pub mod fonts;
pub mod geom;
pub mod inherit;
pub mod media;
pub mod model;
pub mod package;
pub mod pdf;
pub mod raster;
pub mod render;
pub mod richtext;
pub mod scene;
pub mod smartart;
pub mod tables;
pub mod text;
pub mod view;
pub mod xml;

#[cfg(test)]
mod tests;

pub use model::Deck;

/// Renderar bild `i` till RGBA i `w`×`h` bildpunkter.
#[allow(dead_code)]
pub fn render_slide(deck: &Deck, i: usize, w: u32, h: u32) -> Option<(u32, u32, Vec<u8>)> {
    let scene = render::slide_scene(deck, i);
    raster::render(&scene, w, h).map(raster::to_rgba)
}
