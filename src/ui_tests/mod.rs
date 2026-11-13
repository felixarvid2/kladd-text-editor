//! Gränssnittstester: hela `KladdApp` körs i `egui_kittest` med simulerad inmatning och hittar
//! knappar via etikett i AccessKit-trädet. Ingen GPU, inget fönster och inga anrop mot nätet.
//! Så skriver man ett test: `docs/dev/gransnittstester.md`.

mod harness;
mod pdf;
mod text;
mod word;

pub(crate) use harness::Ui;
