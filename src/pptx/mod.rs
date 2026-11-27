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
// Visningen (w-pview): anteckningar, sök, bildspel och Redigera en kopia.
pub mod copy_md;
pub mod find;
pub mod notes;
pub mod slideshow;
pub mod emf;
pub mod chart_draw;
pub mod chart_fmt;
pub mod chart_model;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod view_tests;
#[cfg(test)]
mod chart_tests;

pub use model::Deck;

/// Filändelser som öppnas i presentationsvyn: presentation, bildspel, mall och deras varianter med
/// makron (makron körs aldrig, de läses inte ens).
pub const EXTENSIONS: &[&str] = &["pptx", "ppsx", "potx", "pptm", "ppsm", "potm"];

/// Öppnas filändelsen (gemener) i presentationsvyn?
pub fn is_ext(ext: &str) -> bool {
    EXTENSIONS.contains(&ext)
}

/// Renderar bild `i` till RGBA i `w`×`h` bildpunkter.
#[allow(dead_code)]
pub fn render_slide(deck: &Deck, i: usize, w: u32, h: u32) -> Option<(u32, u32, Vec<u8>)> {
    let scene = render::slide_scene(deck, i);
    raster::render(&scene, w, h).map(raster::to_rgba)
}
