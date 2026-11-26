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

## Kodblock: språknamn (ej släppt)

`elements::code_block` tar ett extra argument `lang` och ritar språket ur inhägnaden (` ```rust `) diskret
till vänster om kopieringsikonen. Kopieringsknappen fanns redan i originalet.

## Radavstånd (EPUB-läsläget)

`Style::to_richtext` läser ett radavstånd (`LINE_FACTOR_ID`, gånger brödtextens storlek) ur `egui::Context::data`
och sätter det som radhöjd på löptext (inte rubriker eller kod). Läsläget för EPUB lägger dit värdet; utan värde
är allt som förut.

## Blockpositioner för HTML, tabeller och linjer (2.9.0)

`line_points` får också en post för HTML-block, tabeller, definitionsrubriker och linjer. Ett dokument som börjar med
`<p align="center">` hade tidigare ingen position före första stycket, så dubbelklicket i läsläget hittade ingen rad
och bara markerade ett ord.

## Galleri för stycken: justering, förlagets stil och markeringar (EPUB, 2.9.0)

`misc::ViewExtras` (läggs i `egui::Context::data` med `set_extras`) ber visningen rita vissa stycken och rubriker som ett enda
galleri. `egui_commonmark::parsers::pulldown` (`try_galley`, `galley_block`) samlar blockets händelser och bygger en `LayoutJob`:
`justify`/`halign`, indrag som `leading_space`, vänstermarginal, små versaler (gemener som versaler i 80 % storlek med samma byte-längd),
länkar (färg, träffprov med `Galley::cursor_from_pos`, samma länkhakar som förut), sökträffar och `Mark`-bakgrunder, ett
anteckningsmärke efter markeringar med anteckning, och egui:s textmarkering. Klick på en markering lämnas i `MARK_HIT_ID` (`take_mark_hit`).
Block med bild, formel, fotnotsreferens eller HTML ritas på det gamla sättet. Utan `ViewExtras` är allt som förut.
