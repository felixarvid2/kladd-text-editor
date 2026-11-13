# Prestanda med mycket stora filer

Mål: 100 MB text/loggar, kodfiler med 1 miljon rader, Markdown med 50 000 rubriker, dokument på ~2000 sidor
och PDF med 1000+ sidor ska gå att öppna och arbeta i. (CSV hanteras separat.)

## Mätning

Generatorer och mätningar ligger i `src/bigbench.rs` (och `bench_pdf_many_pages` i `pdfview/view.rs`), alla
`#[ignore]`. De körs optimerat i en egen målkatalog så att den vanliga inte rörs:

```
CARGO_TARGET_DIR=/tmp/perf-target CARGO_PROFILE_DEV_OPT_LEVEL=2 cargo test --no-run
/tmp/perf-target/debug/deps/kladd-<hash> --ignored bigbench --nocapture --test-threads=1
```

Storlekar styrs med `KLADD_BENCH_MB`, `KLADD_BENCH_LINES`, `KLADD_BENCH_HEADINGS`, `KLADD_BENCH_PAGES` och
`KLADD_BENCH_PDF_PAGES`. "Bildruta" är CPU-tiden för en egui-bildruta (logik och layout, inte tesselering/GPU).
Siffrorna är från en delad 4-kärnig maskin (opt-level 2), så de varierar ±30 %.

## Före och efter

| Fall | Före | Efter |
|---|---|---|
| Textläge, öppna 5 MB (layout) | 1,07 s, +781 MB | under tröskeln: oförändrat vanligt läge |
| Textläge, 20 MB | 4,9 s, +2,9 GB, tecken i slutet 220 ms | 20 MB+ går i den stora vyn (se nedan) |
| Textläge, 100 MB | får inte plats i minnet (~145 MB per MB, ~14 GB) | läs 0,3 s + index/ordräkning 0,3 s, +99 MB |
| 100 MB logg: bildruta | – | 0,03 ms (median), 0,11 ms (max) |
| 100 MB logg: tecken i slutet | – | 0,02 ms (median), 0,3 ms (max) |
| 100 MB logg: tecken i mitten (flytt av svansen) | – | ~7–10 ms |
| 100 MB logg: sök "status=500" | – | första bildrutan 6 ms, hela 0,3 s (taket 50 000 träffar) |
| 100 MB logg: spara | – | 0,2–0,3 s (ingen extra kopia av texten) |
| 100 MB logg: minne efter redigering | – | +126 MB (topp 174) |
| Kodfil 1 000 000 rader (38 MB): öppna | – | läs 0,1 s + index 0,14 s; bildruta 0,02 ms; tecken i mitten 2,7 ms; spara 60 ms |
| Markdown 50 000 rubriker (3,5 MB): analys / rubriklista | 210 ms / 9 ms | oförändrat (under tröskeln); rubrikträdet ritar bara synliga rader och bygger inte om listan varje bildruta |
| Dokument 2006 sidor (4,8 MB): tecken på sida ~1500 | 737 ms median (analys 330, tabeller+typsnitt 35, layout 150+) | ~300 ms median (analys 13, tabeller+typsnitt 9, layout ~280 i det bullriga läget) |
| Dokument 2006 sidor: kall layout / minne | 3,4 s / +1 GB | oförändrat |
| PDF 1863 sidor: sök i alla sidor, textlager i minnet | 1863 sidor, +357 MB | högst 160 sidor, +92 MB (sökningen tar 0,9 s i stället för 0,5 s) |

Tröskeln för den stora textvyn är 5 MB eller 200 000 rader (minst 1 MB). Under den används `TextEdit` med
färgning och radbrytning som förut, med samma beteende.

## Textläge: `bigtext.rs`, `bigfiles.rs`

`TextEdit` lägger ut hela texten varje gång den ändras (~145 byte minne per textbyte, 4,9 s för 20 MB), så
större filer använder en egen vy:

- **Radindex** (`LineIndex`, 32-bitars förskjutningar) som uppdateras inkrementellt vid varje ändring (testas
  mot fullständig ombyggnad med slumpade ändringar). Filer över 2 GB öppnas inte.
- **Virtualiserad ritning:** bara de ~45 synliga raderna läggs ut. Långa rader skärs till ett fönster runt
  den vågräta rullningen (en rad på 100 MB är ingen fara). Egen rullningslist, eftersom `ScrollArea` tappar
  precision över ~16 miljoner pixlar.
- **Redigering:** text, Enter, Tab, Backsteg/Delete (även ord), piltangenter, Home/End, PageUp/PageDown,
  markera med Skift och mus, dubbelklick väljer ord, klipp ut/kopiera/klistra in, Ångra/Gör om (inslagna
  tangenttryck slås ihop, tak 64 MB). Ingen flerkursor, ingen färgning, ingen radbrytning, ingen
  Markdownförhandsvisning i den vyn.
- **Ändrat-markering:** stora texter hashas inte vid varje tangenttryck (`Document::on_edited` sätter bara
  ändrat). Hash för kontroll av ändringar (autosparning, säkerhetskopiering) är snabb och läser åtta byte
  åt gången; autosparningen räknar om den högst två gånger i sekunden för stora texter.
- **Inläsning:** filer över 20 MB läses i en bakgrundstråd (`State::spawn_load`) i bitar med förloppsruta.
  UTF-8 blir en `String` utan kopia; CRLF görs om på plats. Sparning skriver `&str` direkt för UTF-8/LF.
- **Sök:** `State::search` söker i bitar om 256 kB med en tidsbudget om 6 ms per bildruta, slutar vid
  50 000 träffar (*N+ träffar*) och cachas på en ändringsräknare i stället för en hash av texten.
  *Ersätt* ersätter på träffens rader (med ångra); *Ersätt alla* bygger om texten och rensar ångra-historiken.
- **Ordantal** hålls à jour vid varje ändring genom att räkna om de berörda raderna.
- **Rubrikträdet** i Markdown-filer använder ändringsräknaren och markörpositionen från vyn.

Kända begränsningar: kopiering och säkerhetskopiering av en dirty 100 MB-flik klonar texten på UI-tråden
(~30 ms, var 30:e sekund); ångra efter *Ersätt alla* finns inte i den stora vyn.

## Dokumentläge

`word::incr` (`analyze_incremental`) tolkar efter en liten ändring (högst 4 kB, i dokument över 200 kB)
bara stycket med ändringen plus ett stycke på var sida. De yttre styckena är kanariefåglar: ger de inte
exakt samma bitar som i den gamla analysen (något sträcker sig över styckegränsen) görs full analys.
Kodstaket, fotnoter, referenslänkar, avsnitt och ändringar på första raden ger alltid full analys.
Slumpade ändringar mot `markup::analyze` ingår som test. Tabeller, bilder och typsnitt återanvänds när
fönstret inte rört dem.

Kvar: `relayout` går igenom alla stycken varje ändring (stycken ~50 ms, styckeloopen med nycklar ~180 ms,
sammanfogning ~25 ms vid 2000 sidor), så att skriva på sida 1500 av 2000 ligger runt 300 ms, inte under
16 ms. Nästa steg är ett galleri som bara lägger ut och fogar ihop sidorna nära vyn (och uppskattade höjder
för resten), vilket kräver att `word/view.rs` slutar anta att galleriet täcker hela dokumentet.

## PDF-visaren

Layouten (`build_layout`) är O(sidor) med binärsökning, miniatyrpanelen är virtualiserad och texturcachen har
tak (256 MB, 240 miniatyrer). Nytt: textlagren (`PageText`, ~0,2 MB per sida) har tak (`MAX_TEXT_PAGES` = 160)
och släpps långt från vyn; sidor som redan sökts hämtas inte om, den aktuella träffens sida hämtas vid behov
och "markera allt" behåller allt. `find_status` bygger inte längre träfflistan varje bildruta.

## Stavning och grammatik

Körs bara i dokumentläget, som redan bara kontrollerar ändrade stycken (`misspelled_cached`); grammatiken
skickar bara ändrade stycken. Oförändrat.
