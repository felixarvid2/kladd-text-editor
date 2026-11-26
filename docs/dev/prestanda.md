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

## Starttid

`KLADD_TIMING=1 kladd` skriver tiden sedan processstart till stderr vid varje steg fram till första bilden
(`src/timing.rs`, `timing::mark`). Mätt i debugbygge (inte `--release`), Xvfb med programvaru-GL, median av 7
(5 för docx) omväxlande körningar av före- och efterbygget, `--new-window` med tom miljö:

| Fall | Före | Efter |
|---|---|---|
| Utan fil: första bilden | 314 ms | 182 ms |
| Markdown-fil: första bilden | 291 ms | 148 ms |
| `gen-skolrapport.docx`: första bilden | 785 ms | 666 ms |
| `KladdApp::new` (utan fil) | ca 140 ms | ca 1 ms |

Vad som tog tid i `KladdApp::new` (debug): Markdown-cachen `CommonMarkCache` byggde en hel syntect-uppsättning
(syntaxer och teman) i sin `Default`, och det finns tre (appen, uppdateringsrutan, språkservern): ca 30–40 ms
var. Det är nu lat i den vendrade `egui_commonmark_backend` (`OnceLock`, läses vid första kodblocket).
Editorns egna `Highlighter` läste syntaxerna (ca 25 ms); de läses nu i en bakgrundstråd (`highlight::preload`)
som startas innan fönstret skapas och är klara när en kodfil ska färgas.

Det som är kvar ligger utanför Kladds kod eller är nödvändigt för bilden: ca 80–100 ms innan `KladdApp::new`
körs (winit, GL-kontext och eframe), och i första bildrutan själva layouten (för en `.docx` ca 0,5 s i debug,
där dokumentet tolkas och läggs ut). Typsnitt för dokumentläget läses inte vid start: `fc-match` körs och
fontdb skannar systemtypsnitten först när ett dokument använder ett typsnitt som inte är inbyggt. Stavningsordlistor
(`speller`) och uppdateringskontrollen (egen tråd) var redan uppskjutna. Release-bygget är snabbare i alla steg
men mättes inte (disken räckte inte till ett `--release`-bygge).

## Dokumentläge

`word::incr` (`analyze_incremental_with`) tolkar efter en liten ändring (högst 4 kB, i dokument över 200 kB)
bara stycket med ändringen plus ett stycke på var sida. De yttre styckena är kanariefåglar: ger de inte
exakt samma bitar som i den gamla analysen (något sträcker sig över styckegränsen) görs full analys.
Tre tilda, oavslutade kommentarer, referenslänkar (när fönstret har hakparenteser), kodstaket som inte är
välformade, ändringar på grundformatsraden och nya eller ändrade fotnotsetiketter och avsnittsrader ger full analys; se
"Tangenttryck och öppning, andra omgången" nedan för vad som går inkrementellt. Slumpade ändringar mot
`markup::analyze` ingår som test. Tabeller, bilder och typsnitt återanvänds när fönstret inte rört dem.

Efter en inkrementell analys återanvänder `word::layout_memo` styckena, fotnotsplanen, grupperna och
cachenycklarna från förra omläggningen (se nedan), så att ett tangenttryck inte längre kostar tid i proportion till
dokumentets längd i de steg som var dominerande. I ett block av fotnotsdefinitioner är fönstret bara definitionen med
ändringen och dess grannar, och `word::layout_resume` återupptar utläggningen, sammanfogningen och sidindelningen vid den
ändrade gruppen och tar resten från förra omläggningen (se "Fotnoter och 2000 sidor, tredje omgången").

## Öppna stora Word-dokument (100–400 sidor)

Mål: första sidan syns direkt och fönstret fryser aldrig, hur stort dokumentet än är.

### Mätning

`bench_docx_open` i `src/word/bench.rs` (ignorerat test) läser ett `.docx`/`.odt` och mäter varje steg, och en
stegvis öppning som i appen. Testdokumenten byggs av `tests/corpus/docx/gen-skolrapport.docx` upprepad N gånger
(rubriker, tabeller, bilder, fotnoter, tvåspaltsavsnitt, textrutor, sidhuvud; skriptet ligger inte i repot, det
räcker att klistra in dokumentets `<w:body>` N gånger och numrera om fotnoterna):

```
cargo test --no-run
KLADD_BENCH_DOCX=stor.docx target/debug/deps/kladd-<hash> --ignored bench_docx_open --nocapture --test-threads=1
```

`KLADD_BENCH_NO_INCR=1` slår av den inkrementella analysen (så som det var före, för jämförelse), `KLADD_BENCH_VERBOSE=1`
skriver varje bildrutas analystid och varför den inkrementella analysen avstod, `KLADD_BENCH_DUMP=fil` sparar Markdown.
Alla siffror är debugbygge (inte `--release`; disken räckte inte till ett optimerat bygge) på en delad 4-kärnig maskin,
så de varierar ±30 %. Ett optimerat bygge har inte mätts; det brukar vara en tiondel av debugtiden i layout och analys.

| Dokument | 202 sidor (330 kB Markdown) | 323 sidor (528 kB) | 376 sidor (624 kB, mycket unik text) |
|---|---|---|---|
| Läsning zip/XML till Markdown | 0,4–0,6 s | 0,7–0,9 s | 0,6 s |
| Markup-analys, hel text | 80–110 ms | 125–185 ms | 155 ms |
| Stavning, kall (svensk ordlista) | 20–45 ms | 25–30 ms | – |
| Utläggning + sidindelning, kall | 2,0–2,6 s | 2,8–3,8 s | 6,8 s |
| Före: fönstret frös under läsning + analys + utläggning | ca 3,0 s | ca 4,7 s | ca 7,6 s |
| Efter: längsta bildruta, stegvis | 0,3–0,4 s | 0,4–0,6 s | 0,4 s |
| Efter: första sidan syns | efter första bildrutan | efter första bildrutan | efter första bildrutan |
| Efter: allt utlagt (tid i bildrutor) | 3,2–3,8 s i 16 bildrutor | 6,1–8,1 s i 22–25 bildrutor | 13,6 s i 51 bildrutor |
| Tangenttryck, median av 8 ställen: före | 203 ms | 370 ms | 437 ms |
| Tangenttryck, median av 8 ställen: efter | 135 ms | 290 ms | 302 ms |
| Tangenttryck, sämsta av 8: före / efter | 252 / 213 ms | 449 / 578 ms | 544 / 415 ms |
| Oförändrad bildruta | 0,1 ms | 0,1 ms | 0,1 ms |

Slutsatser av mätningen: textformningen av styckena (`loop`, 1,9–6,5 s kallt) är helt dominerande vid öppning;
läsningen är nästa (0,4–0,9 s) och analysen av hela texten (80–185 ms) kommer en gång vid öppning och igen vid varje
tangent som inte går att analysera inkrementellt. Stavningskontrollen är ingen flaskhals (styckecache, ordcache och
ordlistan läses i en bakgrundstråd), och styckecachen träffar: ett tecken ger 1–3 omlagda stycken av över tusen.

### Vad som gjordes

- **Läsning i bakgrunden** (`bigfiles::wants_background`): `.docx`, `.dotx` och `.odt` över 100 kB (packat) läses i
  samma bakgrundstråd som stora textfiler, med förloppsruta (stapeln står på 90 % när filen är läst och tolkningen
  återstår). Under tiden fryser ingenting och inga andra flikar påverkas. Startar Kladd med en sådan fil som argument
  är `KladdApp::new` klar efter ca 100 ms i stället för efter läsningen.
- **Stegvis utläggning** (`word/layout_lazy.rs`, `Cache::galley_paged_lazy`): `relayout_pass` lägger ut stycken tills
  tiden för stycken som inte finns i styckecachen överstiger budgeten (25 ms, växer till 120 ms per bildruta, och
  alltid minst lika mycket som resten av varvet). Resten av dokumentet blir platshållare: stycken med rätt antal
  tecken men utan glyfer, en höjd uppskattad från de stycken som redan lagts ut, och inget att rita. Sidindelning och
  fotnoter körs som vanligt över galleriet, så sidantal och rullningslist är uppskattade tills allt är klart, och
  statusraden visar *Sida X av ca N (lägger ut dokumentet: Y %)*. Nästa bildruta fortsätter där den förra slutade
  (färdiga stycken kommer ur cachen) och till slut är resultatet detsamma som i en enda omgång: samma kod och samma
  nycklar, galleriet jämförs med `==` i `lazy_tests`. Ett ofullständigt galleri cachas aldrig som bildrutans galleri
  och spalt-jämningen görs först när allt är utlagt.
- **Hopp och tangenter**: en tangent (text, piltangenter, `Ctrl+End`), `scroll_to_char` (sök, rubrikträd, fotnotshopp)
  och att bara platshållare syns i fönstret ger ett komplett galleri direkt (budget `None`); syns platshållare längst
  ned i fönstret får nästa bildruta 250 ms. Dokument under 60 kB läggs alltid ut i en omgång, som förut.
- **Inkrementell analys för Word-dokument** (`word/incr.rs`): fotnoter och avsnitt (vanliga i Word-dokument) gav
  tidigare alltid full analys av hela texten vid varje tangent. Nu räcker det att fönstret (stycket med ändringen och ett
  stycke på var sida) inte rör dem: fotnotsreferenser tolkas med en tillfällig definition per etikett efter fönstret,
  avsnitten efter fönstret flyttas med texten, `<div align>`-block hålls hela genom att fönstret vidgas, och gamla bitar
  klipps vid fönstrets kant (bitar slås ihop över styckegränser). Kontrollerna är strängare än förut (raderna före och
  efter raden med ändringen jämförs med den gamla analysen), och fuzz mot `markup::analyze` ingår som test, med fotnoter,
  avsnitt och trasig markup i dokumenten (`KLADD_FUZZ_SEEDS=150` kör fler frön). Fuzzningen hittade en gammal brist: en
  bit som sträckte sig över fönstrets kant gav en annan tolkning av kanten; den är rättad.

### Tangenttryck och öppning, andra omgången

Mätning: `bench_docx_open` skriver nu tiden för ett tecken på tio ställen (`KLADD_BENCH_PLACES=1` visar varje): början,
25 %, mitten, 75 %, slutet av brödtexten, i en tabellcell, vid en fotnotsreferens, i stycket närmast
fotnotsdefinitionerna, i första och i sista fotnotsdefinitionen. Geometrin är som i appen (`Geom::for_sections`,
`edit_w`) och sidantalet under stegvis utläggning är det som statusraden visar. `bench_docx_keys`
(`KLADD_BENCH_KEYS=antal`) skriver bara tangenttryck i mitten, för profilering (`valgrind --tool=callgrind` på
testbinären; ett opt-level 2-bygge av kladd och egui med `--config profile.dev.package.<krate>.opt-level=2` räcker).
Dokumenten är `gen-skolrapport.docx` upprepad 50, 75 och 100 gånger (202, 303 och 404 sidor, 325–650 kB Markdown).
Siffrorna är medianen av 2–3 körningar på en delad 4-kärnig maskin (±30 %); opt-level 2 gäller bara kladd och egui med
närmaste krater, resten är debug.

| Mått (ms) | 202 sidor före / efter | 303 sidor före / efter | 404 sidor före / efter |
|---|---|---|---|
| Tangenttryck, median av 10 ställen, opt-level 2 | 17,5 / 7,0 | 34,0 / 8,7 | 56,1 / 11,2 |
| Tangenttryck, sämsta av 10, opt-level 2 | 53,4 / 19,2 | 102,0 / 27,2 | 133,5 / 37,2 |
| Tangenttryck, median av 10 ställen, debug | 186 / 32 | 337 / 49 | 524 / 68 |
| Tangenttryck, sämsta av 10, debug | 261 / 79 | 479 / 115 | 677 / 163 |
| Första bildrutan (första sidan syns), opt-level 2 | 85 / 98 | 143 / 130 | 182 / 161 |
| Första bildrutan, debug | 288 / 257 | 482 / 391 | 540 / 488 |
| Kall utläggning, allt i en bildruta, opt-level 2 | 536 / 550 | 712 / 696 | 1014 / 935 |
| Stegvis tills allt är utlagt, opt-level 2 | 603 / 571 | 977 / 742 | 1216 / 941 |
| Stegvis tills allt är utlagt, debug | 3448 / 2147 | 7055 / 3420 | 7897 / 5367 |

Per ställe vid 404 sidor (opt-level 2, före / efter): början 51 / 19, 25 % 44 / 11, mitten 47 / 11, 75 % 43 / 10, slutet av
brödtexten 131 / 10, i tabell 46 / 11, vid fotnotsreferens 48 / 10, närmast definitionerna 124 / 20, första definitionen
126 / 37, sista definitionen 128 / 34. Tangenttrycket växer nu knappt med dokumentets längd i de steg som var dominerande;
det som återstår är linjärt men med liten konstant (sidindelning och sammanfogning av galleriet, själva styckeloopen).
Små dokument är inte långsammare (Markdown 30 kB / 55 kB, debug: tecken 22 / 28 ms före, 17 / 26 ms efter; markören byter
rad 6 / 9 ms före, 1,5 / 2,9 ms efter; 600 kB opt-level 2: tecken 34 / 13,5 ms, markörbyte 22 / 6 ms).

Vad som gjordes, i den ordning mätningen pekade ut (callgrind på 30 tangenttryck i ett 404-sidors dokument):

- **Appen använde aldrig den inkrementella analysen.** `word_page` höll en `Arc<Analysis>` under hela bildrutan, så
  `Cache::state` kunde inte uppdatera analysen på plats när TextEdit lade ut galleriet igen efter en ändring (`Arc::get_mut`
  misslyckades) och gjorde full analys (80–185 ms debug) vid varje tangent. Referensen släpps nu före TextEdit och hämtas
  om efter. Mätverktygen når inte det här (de håller ingen referens), så det syntes bara i appen (`KLADD_TIMING=1` skriver nu
  *analys efter ändring: X ms (inkrementell/full)* och *omläggning: …* per ändring).
- **Fotnoterna var kvadratiska.** `notes::spec` slog upp radnumret för varje fotnotsreferens med egui:s `layout_from_cursor`
  (går igenom alla rader, ca hälften av tangenttrycket vid 404 sidor): nu en binärsökning över radernas slutlägen.
  `has_def` i `incr` jämförde varje byteläge i hela texten (`windows` + `memcmp`) och söker nu bakifrån med `rfind`;
  `layout_def` sökte i alla bitar efter etikettens bit och `plan` jämförde etiketter parvis; fotnotsdefinitionen för en
  grupp hittades med en linjär sökning.
- **Förberedelsen återanvänds** (`word/layout_memo.rs`): stycken (`paragraphs`), fotnotsplanen, grupperna och den statiska
  delen av cachenyckeln (`unit_key_static`: text, bitar, markör) sparas från förra omläggningen. Efter en inkrementell analys
  (som anger vilket område som ändrats, `Delta::we_old`/`we_new`) räknas bara fönster om (`paragraphs_in`): ändringen, markörens
  gamla och nya rader (vidgade till bitarna som ingår i markörens rader) och en ruta som fönstret rör vid, med tre
  kontrollstycken plus ett kantstycke på var sida. Kontrollstyckena jämförs med de gamla (annan luft, justering eller
  radbrytning ger full omräkning), och reglerna för det som fönstret inte ser avvisar återanvändningen: fotnotsdefinitioner i
  eller intill fönstret, ändrade fotnotsreferenser (`Delta::notes_changed`), öppna textrutor, matchande sökträffar, oavslutade
  `<!--k:`-markörer (`pbox::scan` hoppar då över text som ett fönster inte ser), ändrad innehållsförteckning, mer än tre
  fönster. Resten av styckena, grupperna och nycklarna flyttas bara (byteintervallen efter ändringen förskjuts).
  Utläggningen av grupperna är oförändrad: `relayout_pass` går fortfarande igenom alla grupper, men utan att hasha texten, de
  bitar som hör till den eller tolka om styckena. Tester (`layout_memo::tests`): slumpade ändringar och markörflyttar mot
  full beräkning av stycken, fotnotsplan, grupper, nycklar och innehållsförteckning (`KLADD_FUZZ_SEEDS=400` körs), och
  galleriet efter ändringar (`galley_after_edits_equals_fresh_cache`) mot en ny cache.
- **Billigare hash och sökningar:** styckecachen använder en hasher som inte hashar nycklarna igen, `Stubs::note` räknar inte
  tecken när allt ändå läggs ut, och styckenycklarna är delade i en statisk och en dynamisk del.
- **Analysen i inläsningstråden:** `bigfiles` gör `markup::analyze` (80–185 ms debug) i tråden som läser dokumentet och lämnar
  över den (`layout::Prepared`, kontrollerad mot texten) till `Cache::preload` innan fliken installeras; första
  bildrutan hoppar över den.
- **Färre fall av full analys** (`incr`, med fuzz): text i en fotnotsdefinition (etiketterna oförändrade), avsnittsrader i
  fönstret som ändringen inte rör (avsnitten flyttas), en vanlig första rad (bara grundformatsraden ger full analys),
  dokument med kodstaket så länge alla staket står först på en rad, är ett jämnt antal och fönstret ligger utanför ett
  kodblock, och referenslänkar när fönstret saknar hakparenteser. De globala kontrollerna (tre tilda, referenslänkar,
  oavslutade kommentarer, kodstaket) räknas nu per fönster (`incr::Facts`, uppdateras inkrementellt) i stället för att söka
  igenom hela texten vid varje tangent. Fuzzningen med kodstaket och referenslänkar i dokumenten hittade en avvikelse mot
  full analys för dokument med ett udda antal staket (en trasig `<div` efter ett oparat staket): de avvisas nu.
- **Sidantal och platshållare:** platshållare kalibreras per styckeslag (brödtext, rubrik, lista, citat, kod, tabellrad;
  `layout_lazy::Kind`) mot det som redan är utlagt, med synliga tecken (inte markup), radlängd per px radbredd (spalter och
  indrag räknas rätt) och sista radens fyllnad. Platshållarna börjar först efter minst sex brödtextstycken. Under stegvis
  utläggning räknar statusraden upp sidantalet från det utlagda (`Cache::estimated_pages`: sidor utlagda på riktigt delat
  med den del av texten de utgör) i stället för platshållarnas egen sidindelning, som hoppar när spalter eller sidbrytningar
  inte jämnats ut än (91–150 sidor för ett dokument på 120 sidor); efter 8 % visas *ca N* som i mätningen ligger inom 10 %
  av slutvärdet (404 sidor: 404–441), före det visas bara förloppet. Sidor som bara är platshållare ritas med en diskret
  *Läggs ut…*, och framkanten av det utlagda räknas från raderna före första platshållaren (en rad i en textruta kan ligga
  högre upp). Test: `uppskattat_sidantal_ligger_nara_det_slutliga`.

### Fotnoter och 2000 sidor, tredje omgången

Mätning som förra omgången (`bench_docx_open` med `KLADD_BENCH_PLACES=1`, som nu också skriver tiden per steg och om
utläggningen återupptogs), med `gen-skolrapport.docx` upprepad 100 och 500 gånger (404 och 2021 sidor, 650 kB och 3,3 MB
Markdown, ca 13 000 och 65 000 grupper). Opt-level 2 för kladd och egui med närmaste krater (`epaint`, `emath`, `ecolor`,
`harfrust`, `skrifa`, `read-fonts`), medianen av tre körningar; debug en till tre körningar.

| Mått (ms) | 404 sidor före / efter | 2021 sidor före / efter |
|---|---|---|
| Tangenttryck, median av 10 ställen, opt-level 2 | 13,7 / 1,5 | 112,8 / 4,6 |
| Tangenttryck, sämsta av 10, opt-level 2 | 38,8 / 13,7 | 604,5 / 17,3 |
| Första fotnotsdefinitionen, opt-level 2 | 38,8 / 1,3 | 604,5 / 4,6 |
| Sista definitionen (en slutnot), opt-level 2 | 33,7 / 0,8 | 581,0 / 3,1 |
| Stycket närmast definitionerna, opt-level 2 | 20,0 / 0,9 | 185,6 / 4,4 |
| Tangenttryck, median av 10 ställen, debug | 83,4 / 6,3 | 683,7 / 12,9 |
| Tangenttryck, sämsta av 10, debug | 150,8 / 65,6 | 1676 / 54,5 |
| Kall utläggning, allt i en bildruta, opt-level 2 | 422 / 353 | 1560 / 1344 |
| Stegvis: första bildrutan / tills allt är utlagt, opt-level 2 | 148 / 423 och 143 / 410 | 730 / 2511 och 598 / 1674 |
| Stegvis tills allt är utlagt, debug | 4343 / 3236 | 65 700 / 24 045 |
| Oförändrad bildruta, opt-level 2 | 0,1 / 0,1 | 1,4 / 1,3 |

Per ställe vid 404 sidor (opt-level 2, efter): början 13,7 (raden bryts om, se nedan), 25 % 4,2, mitten 1,5, 75 % 1,5,
slutet av brödtexten 1,3, i tabell 2,1, vid fotnotsreferens 1,3, närmast definitionerna 0,9, första definitionen 1,3, sista
0,8. Vid 2021 sidor: 17,3, 5,6, 4,5, 3,5, 3,3, 7,5, 6,8, 4,4, 4,6, 3,1. Små dokument är inte långsammare (Markdown 30 kB /
55 kB, opt-level 2: tecken 6,0 / 10,7 ms före, 5,9 / 9,2 ms efter; debug 16,4 / 26,3 före, 13,2 / 21,9 efter; markören byter
rad i debug 1,4 / 2,3 före, 1,0 / 1,4 efter; 600 kB opt-level 2: tecken 18,9 / 7,6 ms, markörbyte 6,8 / 3,0 ms).

Målen: text i en fotnotsdefinition tar 1,3 ms vid 404 sidor (mål 15) och ungefär lika lång tid oavsett antalet fotnoter,
stycket närmast definitionerna är som övrig brödtext. Tangenttrycket vid 2021 sidor är 4,6 / 1,5 = 3,1 gånger det vid 404
sidor i opt-level 2 (målet var högst 2) och 12,9 / 6,3 = 2,0 gånger i debug; före var det 8 gånger. Det som återstår och växer
med dokumentet står under *Kvar*.

Vad som gjordes, i den ordning mätningen (callgrind på tangenttryck i 2021-sidorsdokumentet, `bench_docx_keys` som nu
har tangenttrycken i en egen funktion, `--toggle-collect='*keys_loop*'`) pekade ut:

- **Två kvadratiska sökningar.** Sidindelningen sökte bildhöjden för varje rad bland alla ankare (`floats.iter().find`), och
  textrutornas omflöde (`apply_pulls`) sökte rutans intervall för varje rad bland alla rutor. Med tusentals bilder och rutor
  var det över hälften av tangenttrycket vid 2000 sidor. Nu går båda i takt med raderna (`pages::FloatAt`).
- **Fotnotsdefinitionerna som fönster** (`incr::def_window`). Word-läsaren skriver definitionerna som ett block utan
  tomrader, så en ändring i en definition tolkade om hela blocket (53 kB vid 404 sidor, mer än gränsen för fönstret vid 2000
  sidor). Varje definitionsrad börjar en ny definition, så fönstret är nu definitionen med ändringen och en definition på var
  sida som kanariefåglar, och stycket före respektive efter ett block tar bara blockets sista respektive första definition.
  Gränserna kontrolleras mot analysen (etikettens bit), och slutnoter (`[^ei]`) hanteras likadant.
- **Fotnotsplanen i fönstret** (`layout_memo::plan_window`). Återanvändningen avstod när fönstret låg i eller nära
  definitionerna. Nu räknas planen om i fönstret: definitionerna där ska ha samma etiketter i samma ordning som förut (annars
  full omräkning), tomma stycken före en definition krymps, och luften över första slutnoten och de krympta styckena efter
  sista definitionen räknas om när fönstret ligger efter den. `notes::plan` är omskriven kring samma regel per stycke
  (`defs_of`) så att de inte kan glida isär.
- **Återupptagen utläggning** (`word/layout_resume.rs`). Varvet över grupperna sparar tillståndet före varje grupp (läge,
  flytande bilder, öppen textruta, sidsimulering) och vad den lade till (gallerier, ankare, tabeller, fotnoter …), i täta listor
  med en post per grupp och glesa för det sällsynta. Nästa varv börjar vid den första ändrade gruppen; när tillståndet före en
  oförändrad grupp är exakt detsamma som förra gången (jämfört bit för bit, också y) tas resten av sträckan från förra varvet,
  flyttad i rader och byte. Listorna lappas på plats (`patch_dense`, `patch_sparse`) i stället för att kopieras. Ändras höjden
  (raden bryts om) blir tillståndet aldrig detsamma igen; efter 64 oförändrade grupper i följd ges varvet upp och resten görs
  som vanligt (`Fast::abandon`).
- **Sammanfogning och sidindelning från den ändrade sidan.** Sammanfogningen räknar som `Galley::concat` (samma avrundning)
  men grupp för grupp från den sparade rektangeln, och oförändrade sträckor ska börja med samma rektangel och samma drag från
  textrutorna. Sidindelningen (ett avsnitt) sparar tillståndet före första raden på varje sida och återupptas därifrån före varje
  ändring (ändrade rader och referenser till noter som fått annan höjd), tills en ny sida börjar med samma tillstånd som förut;
  raderna efter behåller sina lägen i förra galleriet, vars radlista och textbuffert återanvänds. Fotnoternas referenser flyttas
  med sträckorna och söks bara i de ändrade grupperna; summorna (hörn, ritytor) hålls per block om 256 grupper. Galleriet byggs
  i båda vägarna av färdiga rader (`galley_from`), så egui:s `intrinsic_size` är noll (den används bara för `Response`).
- **Mindre per tangent:** tabellerna och kodblocken efter fönstret flyttas i stället för att byggas om, antalet rubriker i
  innehållsförteckningen kommer från analysen (`Delta::toc_delta`) i stället för en genomgång av alla rubriker, grupperna görs om
  bara kring fönstren (`regroup`), textrutornas markörer ligger i en lista (`Prep::marks`), bildernas storleksnycklar sparas,
  minnets text byter buffert i stället för att kopieras, och början och slutet av en ändring hittas med större block.

Prov: `layout::resume::tests` (slumpade ändringar i dokument med flytande bilder, textrutor, fotnoter som block, tabeller,
sidbrytningar, ibland spalter; galleriet, rektangeln, ritytan, arken med fotnoterna, noterna och tabellerna jämförs med en
cache som alltid gör hela varvet och ibland med en ny cache), `layout_memo` (planen i fönstret och galleriet med fotnoterna
mot full omräkning och ny cache) och `incr` (fönster i definitionsblock och slutnoter mot `analyze`). Alla körda med
`KLADD_FUZZ_SEEDS=300`. I appen (`KLADD_TIMING=1`, 404 sidor, debug) skriver *omläggning* nu också om den återupptogs (och
varför inte): skrivning i brödtext och i en fotnot (redigerad på sidan) gav 3,6–6 ms per omläggning med återupptagning,
ångra återställde texten, och markering med Skift+pil återupptogs också. Med sökträffar räknas allt om som förut.

### Kvar

- Tangenttrycket växer fortfarande med dokumentet (3,1 gånger från 404 till 2021 sidor i opt-level 2), med små konstanter:
  varje anrop till `Cache` jämför hela texten med sin kopia (memcmp; bänken och vyn gör det fyra–fem gånger per bildruta,
  ca 0,15 ms vardera vid 3,3 MB), den inkrementella analysen flyttar alla bitar efter fönstret (`Piece` är stor, ca 0,7 ms),
  förberedelsen flyttar styckena och grupperna efter ändringen (ca 0,5 ms), och texten kopieras två gånger (cachens kopia och
  galleriets). Att jämföra texten bara när den kan ha ändrats kräver en ändringsräknare från vyn; att slippa flytta bitar och
  stycken kräver relativa lägen.
- En ändring som ändrar en grupps höjd flyttar allt efter: då görs hela varvet, sammanfogningen och sidindelningen (13,7 ms
  vid 404 sidor och 17 ms vid 2021 i opt-level 2, mot 20 och över 100 ms före). Sidindelningen kunde återupptas vid
  tvingade sidbrytningar om lägena räknades i hela bildpunkter.
- Dokument med flera avsnitt (`paginate_sections`, spalter) återupptar varvet över grupperna men gör sammanfogning och
  sidindelning helt; olika breda spalter (spaltutjämning) gör allt.
- Den inkrementella analysen avstår fortfarande vid ändrade fotnotsetiketter och -definitioner (en ny eller borttagen
  definition), referenslänkar med hakparenteser i fönstret, kodstaket i dokument med udda antal staket eller staket mitt på
  en rad, tre tilda och ändringar på grundformatsraden. Sökträffar ger full omräkning av förberedelsen.
- Stegvis utläggning, första sidan och bilder som i förra omgången (nedan).

### Kvar efter andra omgången

- (Åtgärdat i tredje omgången ovan: genomgången av alla grupper, sammanfogningen och sidindelningen vid varje tangent, och
  fotnotsdefinitionerna som ett enda fönster.)
- Den inkrementella analysen avstår fortfarande vid ändrade fotnotsetiketter och -definitioner, referenslänkar med
  hakparenteser i fönstret, kodstaket i dokument med udda antal staket eller staket mitt på en rad, tre tilda, ändringar
  på grundformatsraden och när kontrollerna i fönstrets kant inte stämmer.
- Stegvis utläggning kostar fortfarande en genomgång av alla grupper (och en platshållare per ej utlagd grupp) per
  bildruta: i debug ca 150–200 ms per bildruta för 400 sidor (opt-level 2 ca en tiondel), så att fylla i hela dokumentet tar
  längre i debug än den kalla utläggningen. Bildrutornas platshållare kunde cachas mellan bildrutorna.
- Första sidan syns efter 0,16 s (opt-level 2) och 0,49 s (debug) för 404 sidor, räknat från att dokumentet är inläst;
  läsningen av zip/XML (0,4–0,8 s) sker i bakgrundstråden. I debug går ca 0,35 s mellan inläst dokument och första
  utläggningen (inte närmare undersökt) och förberedelsen av stycken tar ca 80 ms.
- Bilder avkodas av egui:s bildinläsare och påverkar inte layouten förrän storleken är känd (`word_image` rensar då
  layoutcachen, en gång per bild), så ett dokument med hundratals bilder lägger ut om en gång i början.

## Läsläget (Markdown och EPUB), 2.10

Mätt med `markdown_large_frame_time` (`KLADD_BENCH_MD_KB=24,100,400,4900`, `src/ui_tests/text.rs`) och
`epub_large_chapter_frame_time` (`src/ui_tests/epub.rs`): CPU-tid per `ui.h.step()` i kittest, debugbygge, delad 4-kärnig
maskin (±30 %). Markdown-filen har rubriker, stycken med formatering, listor, kodblock, tabeller, uppgiftslistor och citat.

| Fall (debug) | Före | Efter |
|---|---|---|
| Markdown 24 kB, stilla / rullning | 158 / 159 ms | 7 / 9 ms |
| Markdown 100 kB | 665 / 670 ms | 9 / 9 ms |
| Markdown 400 kB | ca 2,7 s (linjärt, ej körd hela vägen) | 18 / 19 ms |
| Markdown 5 MB (öppnades inte i läsläget) | ca 33 s (uppskattat, 6,5 ms/kB) | 6,3 / 6,3 ms (de ritade blocken ca 4,5 ms) |
| EPUB-kapitel 400 kB (18 delar) | 23 ms | 4,1 ms (en del) |

Före 2.10 ritade `egui_commonmark` hela texten varje bildruta (och förbehandlingen med formler, diagram, HTML och rubrik-id
gjordes om varje bildruta), så kostnaden växte linjärt med storleken: 6,5 ms/kB. Därför delades EPUB-kapitel vid 24 kB och `.md`-filer
från 5 MB öppnades inte i läsläget.

**Vad som gjordes** (`src/mdwin.rs`, `vendor/egui_commonmark`):
- Texten delas i toppnivåblock (`split`, samma `pulldown-cmark`-flaggor som visningen). `CommonMarkViewer::show_slice` ritar ett
  block med källpositioner räknade i hela texten, så sökträffar, markeringar, kryssrutor och blockstil fungerar oförändrat. Länk- och
  fotnotsdefinitioner långt från blocket läggs efter det, och nästa block läses fram (ritas inte) så att listor och tabeller ser
  samma efterföljande händelser som i en helritning.
- `Win::show` ritar bara blocken i `ui.clip_rect()`; varje blocks höjd mäts när det ritas och blocken sätts på sin uträknade plats.
  Ej ritade block har en uppskattad höjd ur textlängden, som räknas om efter 4, 32 och 256 kB uppmätt text. Dokument under 64 kB
  ritas helt första bildrutan så att alla höjder är exakta (ingen uppskattning, samma rullningslist som förut).
- När ett block ovanför det man tittar på får sin riktiga höjd (eller uppskattningen räknas om) flyttas rullningen lika mycket och
  bildrutan ritas om (`request_discard`), så texten står still. Hopp till en rubrik (länk, navigeringsfönstret) och till sökträffen
  rullar först till blockets uppskattade plats och sedan exakt när blocket ritats.
- Texten förbehandlas en gång per version (`MdState::prep`) i stället för varje bildruta; ordantalet i statusraden räknas om bara
  när texten ändrats; översättningen av översta positionen till dokumentets text görs bara när positionen ändras.
- `CommonMarkCache::line_points` fylls med punkter för blocken nära det synliga (dubbelklick, rullningssynk, EPUB-understrykningar);
  `Win::y_of_byte` ger blockets överkant för hopp (`Jump::Byte`). Sökträffarnas läge ges av `sync_search_layout`.
- `.md`-filer över 5 MB öppnas nu i läsläget (Redigera ger den stora textvyn som förut).

**Kontroller**: `blocken_hamnar_som_i_en_helritning` och `ritade_block_ser_ut_som_i_helritningen` (`mdwin::tests`) jämför blockens
överkanter, texter och galleriformer med en helritning (listor, kod, tabeller, citat, HTML, fotnoter, regler);
`stor_text_ritar_bara_det_synliga_och_hamnar_som_i_en_helritning` rullar genom en text på 190 kB (som mest 60 block ritas per
bildruta) och jämför alla överkanter med en helritning efteråt.

**Kvar**: ritningen av de synliga blocken kostar ca 0,5 ms per block i debug (egui:s textutläggning); mer än så kräver att
galleriet cachas mellan bildrutor. Höjden på ej ritade block är en uppskattning, så rullningslisten kan hoppa lite i stora texter;
sparade lägen i EPUB använder källpositionen (`Memory::byte`) i stället för andelen av uppskattad höjd.

## PDF-visaren

Layouten (`build_layout`) är O(sidor) med binärsökning, miniatyrpanelen är virtualiserad och texturcachen har
tak (160 MB, 240 miniatyrer). Nytt: textlagren (`PageText`, ~0,2 MB per sida) har tak (`MAX_TEXT_PAGES` = 160)
och släpps långt från vyn; sidor som redan sökts hämtas inte om, den aktuella träffens sida hämtas vid behov
och "markera allt" behåller allt. `find_status` bygger inte längre träfflistan varje bildruta.

### Rullning i en stor PDF, 2.10

Mätt med `pdf_scroll_bench` (`KLADD_BENCH_PDF_PAGES=1200`, `pdfview/view.rs`; 1863 sidor, fönster 900x700, 16 ms mellan bildrutor, tre
körningar, debug): "promenad" 60 px per bildruta och "snabb" 700 px per bildruta (som att dra i rullningslisten). Tomma = en sida i
bild utan någon bild alls; grova = bild i fel skala.

| Fall | Före | Efter |
|---|---|---|
| Promenad: bildruta medel / p95 | 0,7–0,9 / 1,5–3,4 ms | 0,9–1,0 / 1,9–4,0 ms (brus; visaren är inte flaskhalsen) |
| Promenad: tomma / grova sidor i bild (av 617) | 0 / 0 | 0 / 0 |
| Snabb: bildruta medel | 0,6–0,9 ms | 0,6–1,1 ms |
| Snabb: tomma sidor i bild (av 248) | 153–196 | 0–83 (resten visas grovt), grova 108–191 |
| Snabb: skarp efter stopp | 0,6–0,9 s | 0,6–1,0 s (oförändrat: hayro-renderingen) |
| Snabb: texturer i minnet | 33–34 st, 137–141 MB | 34–36 st, 97–109 MB |
| Toppminne (process) | 105–108 MB | 107–111 MB |

**Vad som gjordes** (`pdfview/view.rs`): rullhastigheten och riktningen mäts (`track_scroll`); förrenderingen tar fler sidor i
rullningsriktningen (4 framåt, 1 bakåt, annars 2/2); rullar vyn snabbare än tio vyhöjder per sekund (`FAST_SCROLL`, motorn hinner
inte med skarpa sidor) renderas sidor som saknar bild i 35 % av skalan (snabbt att rita) och byts mot skarpa 140 ms efter att
rullningen stannat; en grov bild som kommer efter en skarp ersätter den inte; texturcachen har tak 160 MB (förut 256 MB) och släpper
aldrig synliga eller förrenderade sidor. Promenad (under tröskeln) är oförändrad.

**Kvar**: tiden till skarp bild efter en snabb rullning ändrades inte mätbart: den bestäms av hur snabbt hayro renderar de synliga
sidorna (en tråd per sida), och renderingar som redan pågått kan inte avbrytas. Ett snabbare spår kräver avbrytbar rendering eller
bandvis rendering av en sida.

## PDF-export (`pdf_write.rs`)

Mätning: `src/pdf_bench.rs` (`pdf_bench`, bygger ett stort dokument av `gen-skolrapport.docx` och
`gen-nyhetsbrev.docx` upprepade `KLADD_BENCH_PDF_REPS` gånger, standard 40 = 961 sidor med spaltavsnitt,
fotnoter, tabeller, bilder, textrutor och innehållsförteckning) och `balance_bench` i `pdf_section_tests.rs`
(`KLADD_BENCH_PDF_SECTIONS` spaltavsnitt, jämför nya och gamla utjämningen). Båda är `#[ignore]`; fasernas
tider skrivs med `KLADD_TIMING=1`. Opt-level 2 bara för kladd och PDF-beroendena i samma målkatalog (ingen
fullständig optimerad byggnad, disken räcker inte):

```
args=(); for p in kladd krilla rustybuzz skrifa read-fonts ttf-parser subsetter write-fonts pdf-writer flate2 miniz_oxide png zlib-rs fdeflate kurbo tiny-skia-path image-webp zune-jpeg hashbrown indexmap smallvec crc32fast; do args+=(--config "profile.dev.package.$p.opt-level=2"); done
KLADD_TIMING=1 KLADD_BENCH_PDF_REPS=40 cargo test "${args[@]}" pdf_bench -- --ignored --nocapture
```

| Fas (961 sidor, opt-level 2) | Före | Efter |
|---|---|---|
| Förberedelse (rensning, tolkning) | 24 ms | 22–32 ms |
| Utläggning inkl. utjämning (första passet) | 113 + 115 ms | 64–68 ms |
| Utläggning, pass för innehållsförteckning | 118 ms | 77 ms |
| Sidhuvud och sidfot | 760 ms | 15 ms |
| Ritning av sidor (formning, krilla) | 5 820 ms | 2 850 ms |
| Skrivning (delmängder, komprimering) | 85 ms | 83 ms |
| **Hela exporten** | **7,1 s** | **3,1–3,2 s** |
| 67 sidor, debug (opt-level 0) | 16,2 s | 2,4 s |
| 150 spaltavsnitt (123 sidor), bara utläggning | 31–34 s | 0,36–0,45 s |
| 40 spaltavsnitt (32 sidor), debug | 22,2 s | 0,41 s |

(Delad 4-kärnig maskin, ±30 %.) PDF:erna för 19 och 67 sidor är byte för byte lika före och efter.

Vad som tog tid och vad som gjordes:

- **Utjämning av spalter.** Förut lades hela dokumentet ut för varje försök i binärsökningen per avsnitt
  (kvadratiskt). `Layout::run_balanced` lägger ut avsnitten ett i taget framåt: vid början av varje avsnitt tas
  en ögonblicksbild (`Layout::snapshot`, allt utom de växande listorna, plus den senaste sidan) och sökningen
  återställer den (`rollback`) och lägger bara ut det avsnittet igen. Sidorna före avsnittet påverkas inte av
  dess spalthöjd. Den gamla metoden finns kvar som referens i `pdf_section_tests.rs`
  (`lay_out_reference`) och ett test jämför sidor, positioner och spalthöjder.
- **Bilder.** Sidhuvudets bilder avkodades för varje sida och bodybilder i varje pass; `Layout::load_pic`
  sparar den avkodade bilden per källa (delas mellan passen) och sidhuvudet lägger in varje bild en gång.
- **Textmätning.** Mätcachen och typsnittsytorna delas mellan passen (`Rc`), och cachen slår upp utan att
  allokera en nyckelsträng.
- **Formning vid ritning.** `Surface::draw_text` tolkar om hela typsnittets tabeller för varje textkörning.
  Nu formas texten med `Shaper` (återanvända typsnittsytor och formningsplaner, samma glyfer som
  `rustybuzz::shape`, testat) på upp till 8 trådar, 32 sidor i taget, och ritas med `draw_glyphs`.
- Återstår: formningen och krillas `draw_glyphs` är nu det mesta av tiden (~3 s för 125 000 textkörningar);
  PDF-exporten körs fortfarande på gränssnittstråden (Arkiv → Spara som PDF och utskrift).

## Öppning, gapet efter inläsningen, uppstart och PDF-trådar (2.10)

Mätt i optimerat läge (opt-level 2 för hela beroendeträdet i en egen målkatalog), median av 9 körningar, omväxlande
före- och efterbygget, på en delad 4-kärnig maskin med andra byggen igång (lastsnitt 4–10, så siffrorna varierar
±25 %; ordningen mellan före och efter är stabil). Testdokument: `gen-skolrapport.docx` upprepad 100 respektive 300
gånger (`wave-view/gen-stor-docx.py`; 404 sidor/3 MB XML och 1 212 sidor/9 MB XML, lagrade eller packade), och
samma dokument som `.odt` (skrivet av Kladd, 10 MB `content.xml`). `bench_docx_read` (ny, ignorerad) läser en fil
`KLADD_BENCH_READS` gånger och skriver median och minimum; `KLADD_TIMING=1` skriver delsteg (`docx: …`, `odt: …`).

### 7. Läsning av Word och ODT

| Fall | Före (median / minst) | Efter (median / minst) |
|---|---|---|
| `.docx` 404 sidor, 3 MB XML, lagrad | 141 / 133 ms | 74 / 71 ms |
| `.docx` 404 sidor, packad (66 kB) | 156 / 136 ms | 84 / 79 ms |
| `.docx` 1 212 sidor, 9 MB XML, packad (149 kB) | 829 / 769 ms | 294 / 267 ms |
| `.odt` 10 MB `content.xml` | 229 / 211 ms | 162 / 145 ms |

Delsteg för 404 sidor (före → efter): `collect_parts` (källans delar för den källbevarande sparningen) 57–92 ms →
0 på huvudtråden, tolkningen av brödtexten (`parse_body`, 64 ms) är nu den dominerande och enda stora delen; 1 212 sidor:
förteckningsnycklarna 500 ms → 49 ms.

- **Källans delar i egen tråd.** `collect_parts` packar upp alla filer i paketet (bilder, XML) och behövs först allra
  sist (`Source::parts`); den körs nu i en egen tråd (`std::thread::scope`, egen `ZipArchive` över samma byte, samma
  gränser och samma resultat) medan resten läses. I ett dokument med stora foton är det den största enskilda kostnaden.
  Bilderna som dokumentet visar packas fortfarande upp bara om de används (`pkg.part_bytes`).
- **Förteckningsnycklarna** (`toc_unit_key`) sökte fram rubrikerna i hela texten en gång per förteckning; nu delas
  sökningen av alla förteckningar (ett dokument har oftast en, mätdokumentet 300).
- **Fot- och slutnoter** tolkas i egna trådar (`read_footnotes` delades i `raw_footnotes`, som läser ur paketet, och
  tolkningen) medan brödtexten tolkas; fel vid uppackningen visas efter brödtextens, som förut.
- **XML-delarna** (`Package::part`, `Pkg::text`) blir `String` utan kopia när de är giltig UTF-8.
- **ODT:** `office:text` i `content.xml` tolkas på upp till 8 trådar (`parse_xml_par`): en lätt skanning
  (`text_children`, hanterar citattecken i attribut, kommentarer, CDATA och processinstruktioner) hittar barnens
  starttaggar, skelettet (allt utom barnen) tolkas för sig och bitarna sätts in i `office:text`. Ger skanningen eller
  någon bit upp, eller har dokumentet en annan form, tolkas det i ett stycke med samma fel som förut. Testat mot den
  sekventiella tolkningen (`odt_par_tests.rs`: träden jämförs för olika innehåll, trådantal och felfall). XML-träden
  rivs i en bakgrundstråd (10 % av tiden).
- **Bilder, formatmallar och sidhuvud** mättes: under 2 ms vardera i testdokumenten (som har en bild; inte mätt med många foton). Resten av `docx::read` är en
  platt profil (`callgrind`: ingen funktion över 6 % utom `quick_xml` själv och minnesallokering); `parse_body` bär
  global ordning (fotnotsnummer, markörnummer, ändringsnummer, bildplatshållare) och delades inte.

### 8. Gapet efter inläsningen

Orsaken var två saker i första bildrutan efter att dokumentet installerats, inte teckensnitt, stavning, rubrikträd
eller historik (alla under 1 ms):

1. `word_image` anropade `Cache::forget` så fort en bilds storlek blev känd ("layoutcachen nycklar inte på
   bildstorlekar"), och det händer i första bildrutan. Det kastade den färdiga analysen som inläsningstråden gjort
   (`Prepared`) och tvingade fram en ny `markup::analyze` (ca 60 ms i opt, 0,3 s i debug) mitt i bildrutan. Nu
   behåller `Cache::images_sized` allt när inget lagts ut än; efter en utläggning kasseras det som förut.
2. Statusraden räknade orden i hela dokumentet i varje bildruta (`markup::count` bygger en kopia av hela texten):
   ca 5 ms per bildruta vid 650 kB. Ordantalet cachas nu på (flik, textlängd, texthash).

| Mått | Före | Efter |
|---|---|---|
| Färdig inläsning → första sidan klar, 404 sidor (3 MB lagrad) | 186 ms (173–195) | 69 ms (64–106) |
| Samma, `.docx` packat till 66 kB | 572 ms | 102 ms |
| Ordräkning i statusraden, per bildruta (404 sidor) | ca 5 ms | cachas; kvar är hashen av texten (ca 0,1 ms, uppskattad, inte mätt i appen) |

(Siffrorna inkluderar själva utläggningen, 50–60 ms.) En ändring av ett Word-dokument med redan utlagda sidor
kasserar cachen som förut (`images_sized` gör `forget` när något lagts ut).

### 10. Uppstart

`KLADD_TIMING=1`, tom miljö, `--new-window`, Xvfb med programvaru-GL, median av 5:

| Steg (tom start, ms sedan processstart) | Före | Efter |
|---|---|---|
| `main` till `run_native` (argument, enkelinstans, ikon) | 0,5 | 0,5 |
| winit, X11, GL-kontext (Mesa/llvmpipe), eframe | 64–79 | 64–79 (utanför Kladd; `strace` visar bara bibliotek, X11 och Mesa) |
| `KladdApp::new` (typsnitt, stilar, inställningar, återställning, syntaxer, bibliotek, enkelinstans-socket) | 1,0 | 1,0 |
| första `logic()` och `ui()` till första bilden | 2–6 | 2–6 |
| **Första bilden: tom start** | 80 | 79 |
| Första bilden: textfil / Markdown-fil | 79–126 / 76–85 | 79–86 / 71–95 (brus) |
| **Första bilden: `.docx` 66 kB packat (3 MB XML)** | **386 (minst 321)** | **96 (minst 77)** |
| Första bilden: `.docx` 3 MB lagrad (läses redan i bakgrunden) | 128 | 143 (brus) |

Allt som kan skjutas upp var redan uppskjutet förra omgången (typsnitt, stavning, syntaxer, uppdateringskontroll).
Det som återstår av Kladds del före första bilden är under 10 ms, resten är ramverk och drivrutiner. Enda ändringen:
Word- och ODT-filer som är små packade men har över 1 MB XML (`WORD_BACKGROUND_UNPACKED_BYTES`, ur zip-katalogen utan
att packa upp något) läses nu i bakgrunden i stället för att hålla första bilden (tid till första sidan är oförändrad
inom bruset). Mätningar utan ändring: typsnittsskanning (`fontdb`/`fc-match`) sker först när ett dokument kräver ett
typsnitt som inte är inbyggt; `install_fonts` tar under 0,1 ms.

### 13. PDF-export på flera trådar

| Fall (opt-level 2) | Före | Efter |
|---|---|---|
| 321 sidor (`pdf_bench`, 40 upprepningar), median / minst | 0,60 / 0,56 s | 0,51 / 0,46 s |
| 641 sidor (80 upprepningar) | 1,12 s | 0,93 s |
| 81 sidor (10 upprepningar) | 0,16 s | 0,13 s |

PDF:erna är byte för byte lika före och efter (`cmp` på 81, 321 och 641 sidor, och testet
`threaded_shaping_gives_the_same_bytes` jämför flertrådad och enkeltrådad formning).

Profilen (`callgrind`, huvudtråden + arbetstrådar): formningen (rustybuzz) var 40 % av instruktionerna och låg redan
på trådar men i grupper som huvudtråden väntade på; krillas ritning (`draw_glyphs`) 17 %, komprimeringen av varje
sidas innehållsström i `Page::drop` 17 %, utläggningen 16 %. Nu formas grupperna om 32 sidor av upp till sju
arbetstrådar (var och en med egna typsnittsytor och formningsplaner som återanvänds för alla grupper) medan
huvudtråden ritar de redan formade sidorna i ordning (`ShapeQueue`; arbetstrådarna ligger högst `2·trådar + 2` grupper
före). Formningens väntetid på huvudtråden gick från hela formningstiden till 75–130 ms av 0,5 s.

Det som återstår är enkeltrådat och kan inte delas utan att ändra krilla: `Document` och `Surface` är `&mut`,
glyfregistreringen och sidornas objektnummer beror på ordningen (byte för byte-kravet), och `Page::finish`
komprimerar på huvudtråden. krillas `rayon`-funktion lades inte till (rayon finns inte i beroendeträdet). Utläggningen
(16 %) är sekventiell av samma skäl: varje avsnitt börjar där förra slutade. Små dokument (under 32 sidor eller 400
texter) formas som förut på huvudtråden.

## Stavning och grammatik

Körs bara i dokumentläget, som redan bara kontrollerar ändrade stycken (`misspelled_cached`); grammatiken
skickar bara ändrade stycken. Oförändrat.

## Sökträffar som lager, ångra-minne och ingen omritning i vila

Tre saker som hör ihop: sökningen får inte lägga om dokumentet, ångra-historiken får inte vara en kopia av texten
per steg, och appen ska inte rita om när ingenting händer. Mätt på samma maskin (delad, 4 kärnor, ±30 %),
opt-level 2 för kladd och egui, medianer. Dokumenten är `gen-skolrapport.docx` upprepad 100 och 500 gånger
(404 och 2021 sidor, `scratchpad/wave-view/gen-stor-docx.py`).

### 4. Sökträffar som lager

Mått: ett tecken in eller ut med sökningen öppen (analys, `search::search_doc`, `search::highlights`, galleri, och efter
ändringen också lagret för fönstret), och "nästa träff" (bara aktuell träff byts). `bench_docx_search`
(`KLADD_BENCH_DOCX=sökväg [KLADD_BENCH_FIND=ord] cargo test -- --ignored bench_docx_search --nocapture`), sökning på "och".

| Mått (ms) | 404 sidor (3 000 träffar) före / efter | 2021 sidor (15 000 träffar) före / efter |
|---|---|---|
| Tangent utan sökning | 0,9 / 0,9 | 3,2 / 3,5 |
| Tangent med sökningen öppen | 41,5 / 8,6 | 511 / 51,6 |
| varav analys + `search_doc` | 7,0 / 7,2 | 43 / 43 |
| Nästa träff (bara aktuell träff byts) | 34,6 / 0,08 | 497 / 0,47 |

`Params::matches` var en del av utläggningen (`layout.rs`): nyckeln för bildrutan, för varje stycke och för minnet i
`layout_memo::reuse` innehöll träffarna, så med sökningen öppen lades allt om för varje tangent och för varje byte av
aktuell träff. Träffarna är nu borta ur `Params` och ritas av `word/matchlayer.rs` som ett lager mellan
cellskuggningen och texten: rektanglarna byggs ur galleriets glyfer för raderna i fönstret (±300 px) på samma sätt som epaints
`add_row_backgrounds` (en rektangel per följd glyfer med samma över- och underkant, en pixel vidgad, avrundad till hela
pixlar), så att utseendet är detsamma. Skärmbilder före och efter (404-sidors rapport, ljust och mörkt, sökning på "och" med
aktuell träff, i brödtext, listor, tabeller, fotnoter och textrutor, och med markering över träffarna) är pixel för pixel lika
utom markörens blink (högst 46 bildpunkter).

Kvar: det som återstår av tangenten är sökningen själv (`search_doc` går igenom hela texten för varje ändring, 7 ms vid 404 och 43 ms
vid 2021 sidor) och `fast_hash` av hela texten för sökcachen i `search_results`. En inkrementell sökning är nästa steg.

Ändrat utseende: text som har en egen bakgrund (markerad text, inline-kod, kommentarsområden) har förut fått träffens färg i stället för sin
egen. Ett lager bakom texten kan inte ersätta den, så där ritas träffen ovanpå (med sin genomskinliga färg): träffen syns, men över den egna
färgen. Alla andra träffar är oförändrade.

Textläget (`highlight.rs`) fick inget lager: `bench_text_search` (300 kB Markdown, 818 träffar) ger 12,4 ms för en tangent med träffar
mot 10,2 ms utan, och 8,9 ms för nästa träff (en tangent utan träffar tar lika lång tid, egui lägger ut stycke för stycke och
återanvänder oförändrade). Vinsten vore högst 2 ms, så ingen ändring. Läsläget (Markdown och EPUB) målar träffarna med
`egui_commonmark`s egna sökfärger och har ingen utläggning med träffar i.

### 14. Minne i stora dokument

Mått: `VmRSS`/`VmHWM` ur `/proc/<pid>/status` för det riktiga programmet i Xvfb (`.docx` öppnat, markören i slutet, 20 skurar om 5
tecken med 1,3 s mellan (egui sparar en ångra-punkt först när texten stått stilla en sekund, så 100 tangenter i ett svep ger bara
en eller två punkter), därefter 20 gånger Ctrl+Z).

| RSS (MB) | 404 sidor före / efter | 2021 sidor före / efter |
|---|---|---|
| Efter öppning | 266,8 / 269,5 | 549,2 / 548,6 |
| Efter 100 tangenttryck | 294,2 / 278,4 | 648,5 / 592,1 |
| Efter 20 ångra | 294,2 / 278,8 | 648,5 / 592,1 |
| Tillväxt av 100 tangenttryck | 27,4 / 8,9 | 99,3 / 43,5 |

Kopior av texten: själva `String` (1), `DocState::text` i `word::layout::Cache` (1), galleriets `LayoutJob::text` (1–2 medan nästa lagts
ut), `memo_text` (1, bara mellan en ändring och nästa omläggning), egui:s ångra-historik (**en hel kopia per ångra-punkt, upp till 100,
plus en för texten som håller på att ändras och en per gör om-steg**), och `Document::docx_source` (originalpaketet, delas som `Arc`).
Autosparning, återställning och historiken sparar bara hashar eller skriver direkt till disk; sökningen sparar träffar, inte text.
Ångra-historiken var alltså det enda som växte med antalet steg (3,3 MB per punkt vid 2021 sidor, 330 MB vid 100 punkter) och
`TextEditState::undoer()` klonade dessutom hela historiken vid varje Ctrl+Z i menyn.

`src/undo.rs` ersätter den: fälten i dokumentläget och textläget tar Ctrl+Z, Ctrl+Y och Ctrl+Skift+Z ur indata innan `TextEdit`
ser dem och använder en egen historik med egui:s regler (100 punkter, en punkt efter en sekunds stillhet eller var 30:e sekund i
ständig ändring, samma ångra/gör om-logik) men som sparar ändringar: bara den senaste punkten är en hel text, varje äldre är ändringen
(gemensamt början och slut, det som skilde i mitten) som tar tillbaka den. Texten som håller på att ändras jämförs med längd och hash
i stället för en kopia. egui:s egen historik töms efter varje bildruta. `tests::som_egui` kör slumpade anrop (läge, tid, ångra, gör om,
tillägg) mot egui:s `Undoer` och jämför varje resultat (`KLADD_FUZZ_SEEDS`); ångra, gör om och Ctrl+Skift+Z ger pixel för pixel samma
skärmbilder som förut i både dokumentläget och textläget.

Kvar: efter 100 tangenttryck har RSS fortfarande växt 9 respektive 44 MB, och toppen (`VmHWM` 873 MB vid 2021 sidor) är densamma. Det
är inte ångra-historiken (hela historiken är nu cirka en kopia av texten). Troligen är det utläggningen (galleriet byggs om medan det
förra finns kvar, utlagda stycken i `Cache::units`), men det är inte undersökt: det ligger i `word::layout::Cache`, som hör till
den som arbetar med utläggningen.

### 15. Ingen omritning i vila

Mått: bildrutor per sekund (`KLADD_TIMING=1` skriver `bildrutor: N på T s` och vad som begärde dem, fil:rad, var femte sekund) och CPU-tid
(`utime + stime` ur `/proc/<pid>/stat`) under 30 s i vila, fönstret med och utan fokus (ett annat fönster har fokus), för den tomma
fliken, en textfil, ett Word-dokument i dokumentläget (`gen-skolrapport.docx`, 4 sidor), Markdown i läsläget, EPUB, PDF och CSV. Skripten ligger inte i
repot; mätningen är `idle.sh` (starta, vänta, läs `/proc`).

| Vy | Före: bildrutor/s, CPU ms/s (fokus / inget fokus) | Efter: bildrutor/s, CPU ms/s (fokus / inget fokus) |
|---|---|---|
| Tom flik | 98 / 134 fps, 761 / 1190 | 1,9 / 0,05 fps, 19 / 0 |
| Textfil | 163 / 174 fps, 1432 / 1449 | 2,1 / 0,05 fps, 22 / 0 |
| Dokumentläge | 101 / 80 fps, 1819 / 1255 | 2,1 / 0,05 fps, 44 / 0 |
| Läsläge | 129 / 96 fps, 1421 / 982 | 0,15 / 0,05 fps, 0 / 0 |
| EPUB | 142 / 91 fps, 1369 / 946 | 0,15 / 0,05 fps, 0 / 0 |
| PDF | 32 / 42 fps, 978 / 1144 | 1,06 / 0,05 fps, 39 / 1 |
| CSV | 170 / 109 fps, 1577 / 1194 | 0,15 / 0,05 fps, 0 / 0 |

(Före ritades appen om 80–170 gånger i sekunden med en kärna på 100 % i alla vyer; maskinen var delad, så
CPU-talen före är osäkra uppåt.) Orsaken var en rad: `Lsp::poll_ops` (`lsp/mod.rs`) anropade `ctx.request_repaint()` varje
bildruta, också utan väntande anrop, och kördes från `logic()` i varje vy. Nu begärs en bildruta bara när ett svar kommit. Det som
är kvar i vila är det som behövs: markörens blink (2 bildrutor/s, bara med fokus; egui:s eget), återställningsfilen var 30:e sekund
och i PDF-fliken filkontrollen en gång i sekunden (39 ms CPU per bildruta, bara med fokus).

Fler källor som gjordes villkorade (hittade genom att gå igenom varje `request_repaint`):

- Välkomstsidans blinkande markör ritade om 2 gånger/s också utan fokus; den står nu still utan fokus.
- PDF-flikens filkontroll (`pdf_frame`), arbetsmappens uppdatering (`workspace_refresh`, var 3:e sekund) och git-statusen (`git_ui`,
  var 15:e sekund) begärde en bildruta utan fokus fast de inte gör något då; de väntar nu tills fokus kommer tillbaka (då är
  kontrollen förfallen och görs direkt).
- Stavningskontrollen (`word/view.rs`) begärde en bildruta var 120:e ms för alltid när språket saknar ordlista (`ready()` blir
  aldrig sann); nu bara medan ordlistan läses in (`Speller::loading`).
- En bild som inte gick att läsa in (saknad eller trasig bild i sidhuvud, ett Mermaid-diagram som inte kan ritas) gjorde att
  `request_repaint()` begärdes varje bildruta (sidhuvudet) respektive var 50:e ms (kodblock) för alltid; nu bara medan bilden
  verkligen läses in (`images::image_pending`).
- Ingen av de övriga (`request_repaint` efter dialoger, kanaler och drag) körs utan en händelse.

Utanför mätningen: uppdateringskontrollen, språkservern (en bildruta per 100 ms bara medan anrop väntar) och inläsningar (100 ms bara
medan de pågår) är redan villkorade.
