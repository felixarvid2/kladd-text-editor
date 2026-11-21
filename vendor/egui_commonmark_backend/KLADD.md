# egui_commonmark_backend 0.26.0, lappad för Kladd

Oförändrad kopia av crates.io-versionen 0.26.0 (MIT/Apache-2.0, se LICENSE-filerna) med en ändring
i `src/misc.rs`: `Style::to_richtext` ritar fet text och rubriknivå 1–3 med en typsnittsfamilj som
appen lägger i `egui::Context::data` under `BOLD_FAMILY_ID`. egui har ingen syntetisk fetstil, och
utan lappen blir fet text bara en starkare färg, som inte syns när förhandsvisningen sätter
`override_text_color`.

Tas bort (rader under `[patch.crates-io]` i Cargo.toml) när egui_commonmark kan rita fet text själv.

## Ändringar för läsläget (2.5.0)

Där finns också en lappad kopia av `egui_commonmark` 0.26.0 i `vendor/egui_commonmark` (samma licens,
`[patch.crates-io]` i Cargo.toml) med dessa tillägg, alla markerade "Kladd" i koden:

- `ViewerCache::line_points` (här) och `CommonMarkCache::line_points`: var varje stycke, rubrik,
  kodblock, listpunkt och tabellrad börjar (källbyte och y). Ger "vilken rad ligger vid y" för
  dubbelklick och rullningssynk mellan läsläge och redigerare.
- `CommonMarkViewer::show_with_id_mut`: som `show_mut` (klickbara kryssrutor) men med visnings-id.
- Tabeller: ramar och linjer, fetstil och färgad bakgrund på rubrikraden, luft i cellerna och
  radbrytning (kolumnbredderna fördelas efter textlängd).

Sökstödet (`regex`-funktionen) är påslaget i Cargo.toml.
