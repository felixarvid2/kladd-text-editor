//! Tabellvy för avgränsad text (csv, tsv med flera): tolkning, redigering, sortering och vyn.
//!
//! Dokumentet behåller texten. Allt som ändras i rutnätet är textändringar (`model::Patch`) som
//! bara rör de rader som berörs, så att övriga rader förblir byte för byte lika.

pub mod collate;
pub mod model;
pub mod sniff;
pub mod stats;
pub mod view;

#[cfg(test)]
mod tests;
