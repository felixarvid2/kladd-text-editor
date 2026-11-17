# egui_commonmark_backend 0.26.0, lappad för Kladd

Oförändrad kopia av crates.io-versionen 0.26.0 (MIT/Apache-2.0, se LICENSE-filerna) med en ändring
i `src/misc.rs`: `Style::to_richtext` ritar fet text och rubriknivå 1–3 med en typsnittsfamilj som
appen lägger i `egui::Context::data` under `BOLD_FAMILY_ID`. egui har ingen syntetisk fetstil, och
utan lappen blir fet text bara en starkare färg, som inte syns när förhandsvisningen sätter
`override_text_color`.

Tas bort (rader under `[patch.crates-io]` i Cargo.toml) när egui_commonmark kan rita fet text själv.
