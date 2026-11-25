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
dokumentets längd i de steg som var dominerande.

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

### Kvar

- Tangenttryck kostar fortfarande en genomgång av alla grupper (styckeloopen), `Galley::concat` och sidindelningen (~3 ms
  var vid 404 sidor, opt-level 2): linjärt med liten konstant (uppskattat ca 50 ms vid 2000 sidor). Nästa steg är det som
  står i tidigare avsnitt: återuppta utläggningen vid den ändrade gruppen med sparat tillstånd (sidposition, flytande bilder,
  textrutor) och sluta när tillståndet stämmer med det gamla, och sidindela från den ändrade sidan.
- Fotnotsdefinitioner står som ett enda stycke (en rad per definition, utan tomrad emellan): text i en definition är då ett
  fönster på hela definitionsblocket (53 kB vid 404 sidor), så det blir inkrementell analys men full omräkning av
  förberedelsen (34–37 ms opt-level 2). Ett stycke i brödtextens sista del intill definitionerna avvisas av
  återanvändningen (20 ms): fotnotsplanen ändrar styckena kring definitionerna och kan inte räknas om i ett fönster.
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

## PDF-visaren

Layouten (`build_layout`) är O(sidor) med binärsökning, miniatyrpanelen är virtualiserad och texturcachen har
tak (256 MB, 240 miniatyrer). Nytt: textlagren (`PageText`, ~0,2 MB per sida) har tak (`MAX_TEXT_PAGES` = 160)
och släpps långt från vyn; sidor som redan sökts hämtas inte om, den aktuella träffens sida hämtas vid behov
och "markera allt" behåller allt. `find_status` bygger inte längre träfflistan varje bildruta.

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

## Stavning och grammatik

Körs bara i dokumentläget, som redan bara kontrollerar ändrade stycken (`misspelled_cached`); grammatiken
skickar bara ändrade stycken. Oförändrat.
