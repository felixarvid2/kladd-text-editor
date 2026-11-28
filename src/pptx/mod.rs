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
pub mod media_audio;
pub mod media_model;
pub mod media_play;
pub mod media_video;
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
pub mod table_builtin;
// Omgång 2, ritmotorn (w-prend).
pub mod effects;
pub mod fills;
pub mod mtx;
// Redigering och sparande (w-pedit).
pub mod edit;
pub mod new_deck;
pub mod edit_bar;
pub mod edit_fit;
pub mod edit_group;
pub mod edit_format;
pub mod edit_new;
pub mod edit_ops;
pub mod edit_table;
pub mod edit_text;
pub mod edit_view;
pub mod edit_xml;
pub mod save;
pub mod recover;
pub mod par;
// Omgång 3, bildspelet (w-panim): övergångar, animeringar och föredragsvy.
pub mod anim;
pub mod anim_expr;
pub mod anim_layers;
pub mod morph;
pub mod presenter;
pub mod show_links;
pub mod show_screens;
pub mod transition;
// Omgång 3, 3D i diagram och former (w-p3d).
pub mod bevel;
pub mod chart_3d;

// Omgång 3, SmartArt-layout (w-psmart).
pub mod dgm;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod view_tests;
#[cfg(test)]
mod chart_tests;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod new_deck_tests;
#[cfg(test)]
mod smartart_tests;
// Omgång 4, utlösare och bokstav för bokstav (w-ptrig).
#[cfg(test)]
mod anim_pieces_tests;
// Omgång 5, länkar och video i bildspelet (w-pshow2).
#[cfg(test)]
mod show_links_tests;
// Omgång 8, redigera diagramdata (w-pchartedit).
pub mod edit_chart;
pub mod edit_chart_ui;
pub mod edit_chart_xlsx;
#[cfg(test)]
mod edit_chart_tests;
// Omgång 8, stora presentationer (w-precov).
#[cfg(test)]
mod big_tests;

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
