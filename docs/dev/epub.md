# EPUB-läsläget (2.8.0, del 2 i 2.9.0)

En `.epub` öppnas i ett läsläge för bara läsning. Fliken har ingen text och blir aldrig "ändrad":
`Document::epub: Option<Box<EpubTab>>` (som `pdfv` för PDF). *Redigera en kopia* gör bokens text
till ett nytt osparat dokument; boken själv skrivs aldrig.

Studerat: Foliate, Readest, Koodo Reader, calibre, KOReader och Thorium (rullning kapitel för
kapitel, innehållsförteckning i sidopanel, fotnotsruta, framstegslinje, typografimeny, sepia och
mörk sida).

## Moduler (`src/epub/`)

| Fil | Innehåll |
|---|---|
| `read.rs` | Zip-arkivet (`Archive`, tål versaler och bakstreck), sökvägar (`resolve`: `../`, `%20`, `#id`), `container.xml`, OPF (metadata, manifest, spine, omslag), DRM-kontroll, innehållsförteckning (EPUB 3 `nav`, EPUB 2 NCX). XML läses med den tåliga HTML-tolken i `paste.rs` (`parse`, `El`, `Node`), så en trasig fil fäller inte boken |
| `html.rs` | Ett kapitel (XHTML) till Markdown: block, inline, listor, tabeller, bilder, länkar, noter. Länkmål bärs som osynliga markörer genom omvandlingen och läses av i slutet (`finish`) |
| `mod.rs` | `Book::load`: bygger kapitel (`Chapter`), delar (`Section`), länkmål (`anchors`), noter, innehållsförteckning (`TocEntry`) och omslagssida; `split_ranges`, `unlink`, `plain_lines`, `to_markdown` |
| `view.rs` | `EpubTab` (läge, historik, sök, notruta), ritning, tangenter, Aa-meny, sidopanel, statusrad, `Prefs` och `Memory` |
| `glue.rs` | `impl KladdApp` (undermodul till `app`, som `commands.rs`): flikens sida, sidopanel, överlägg, lägesminne, *Redigera en kopia* |
| `css.rs` | Förlagets formatering: förenklad CSS-tolk (`Sheet`), väljare, kaskad (`Computed`) och `BlockLook` |
| `anchor.rs` | Bokmärken och understrykningar (`Note`): ren text ur Markdown med källpositioner (`plain_map`, `locate`), förankring som tål omläggning (`make`, `resolve`) och export |
| `store.rs` | Biblioteket (`Store`, `BookRecord`) i `<data>/epub/bocker.json` och omslagsminiatyrer i `<data>/epub/omslag/` |
| `selection.rs` | Insticksprogram som fångar `TextSelectionSettled` ur bildrutans utdata |
| `view/annot.rs` | Menyn vid en markering, understrykningarna som `Mark`, listorna i sidopanelen |
| `tests.rs` | Böcker byggda i testet (zip i minnet), tur och retur med `export::epub` |

Gränssnittstester: `src/ui_tests/epub.rs`.

## Inläsning

- **Läsordning**: spine-filerna som finns i arkivet. `linear="no"` och filer som bara länkas till (noter) tas med som
  kapitel men ingår inte i det löpande flödet (`Chapter::linear`); de går att nå via länkar och förteckningen.
- **Omslag**: EPUB 3 `cover-image`, EPUB 2 `<meta name="cover">`, `guide`, annars en bild som heter cover. Börjar läsordningen inte
  med omslagsbilden får boken en egen första sida; en sida med bara omslagsbilden ritas skalad efter fönstrets höjd.
- **Kapitel till Markdown** (`html.rs`): text i Kladds Markdown-dialekt för läsläget. Markdown-tecken skyddas (`\*`, `\_`, `\[` …)
  och radstarter som skulle bli syntax (`#`, `-`, `1.`) likaså; hårda radbrytningar blir `\` + nyrad. Upphöjd och nedsänkt text blir
  Unicode (`m²`, `H₂O`). Osynliga tecken som ordbindare (`U+2060`, `U+FEFF`) tas bort. Dolda element, sidbrytningsmarkörer
  (`epub:type="pagebreak"`), skript och stilmallar hoppas över. Förlagets CSS och typsnitt ignoreras.
- **Länkmål**: varje element med `id` (eller `name`) ger en markör i texten; efter sammanslagningen har varje mål en byteposition i delens
  Markdown. Ritaren översätter byteposition till y med `CommonMarkCache::line_points` (källbyte och y för varje block) och rullar dit.
- **Länkar** i texten är `kl:KAPITEL:N` (index i `Book::links`). Varje sådan adress i en del registreras som *länkkrok*
  (`CommonMarkCache::add_link_hook`), så att ett klick fångas av läsläget i stället för att öppnas som webbadress. Externa länkar
  (`http`, `mailto`) är vanliga länkar.
- **Noter**: fotnotsreferenser (`epub:type="noteref"`, `role="doc-noteref"` eller en liten siffra i `<sup>` som länkar vidare) hittas i ett
  första varv över alla kapitel; målets text (utan bakåtlänkar) sparas som not. Ett klick visar noten i en ruta med läsytans färger.
- **Delning**: ett kapitel över `SECTION_LIMIT` (24 000 byte Markdown) delas vid styckegränser (tomma rader utanför kodstaket) i delar,
  eftersom `egui_commonmark` ritar hela delen varje bildruta. Mätt i ett debugbygge: cirka 28 ms per bildruta för 24 kB (46 ms för
  40 kB), ett kapitel på 400 kB blir 18 delar. Delarna är osynliga för läsaren utom att *Fortsätt ›* står längst ner och att
  framstegen och Tillbaka räknar dem.
- **Bilder** ligger kvar i zip-filen och läses första gången en del visas (`Book::image`), registreras som `bytes://kladd-epub/{id}/{sökväg}`
  och skalas till spaltbredden men aldrig upp. SVG ritas om till PNG (`imgfmt::svg_to_png`) och får `.png` på adressen.
- **DRM**: `META-INF/rights.xml`, `sinf.xml` eller en `encryption.xml` med annan metod än IDPF/Adobe-typsnittsobfuskering ger felet
  *Boken är kopieringsskyddad (DRM) och kan inte öppnas*.
- **Bakgrund**: `.epub` över 100 kB läses in i en bakgrundstråd med förlopp (`bigfiles.rs`, samma som Word).

## Ritning och läge

- Rullytan har ett `ScrollArea` per del (id med del-numret). Hopp (`Jump`: början, slutet, andel, förskjutning, byteposition) görs
  först när delen har ritats en gång (innehållets höjd är då känd); under den bildrutan ritas delen genomskinlig, så att man inte ser
  ett hopp. Mellanslag, Page Down, piltangenter m.m. sätter en förskjutning som ritas nästa bildruta.
- Typografin (`Prefs`) sparas i `Settings::epub`; läget per fil (del och rullningsandel, `Memory`) i `Settings::epub_memory`
  (högst 100, nyast först), som `pdf_memory`.
- Radavstånd: den lappade `egui_commonmark_backend` läser ett radavstånd ur `Context::data` (`LINE_FACTOR_ID`, se
  `vendor/egui_commonmark_backend/KLADD.md`). Serifftypsnittet är Caladea (`theme::serif`).
- Sök i boken: ren text per del (`plain_lines`) byggs första gången sökfältet öppnas; träffarna markeras av ritaren
  (`CommonMarkCache::update_search_matches`) och `go_to_match` rullar till träff nummer *n* i delen.
- Genvägarna följer PDF-visaren där det går (`Alt+Vänster`/`Alt+Höger`, `Ctrl+F`, `F3`). `Ctrl+I`, `[`, `]` och mellanslag är
  fria i läsläget (redigerartangenterna är avstängda).

## Förlagets formatering och justerad text (2.9.0)

- **CSS** (`css.rs`): `Book::load` läser varje kapitels länkade stilmallar (`<link rel=stylesheet>`, högst 16 och 2 MB per fil, cachade per fil)
  och `<style>` (tolken i `paste.rs` behåller inte `<style>`, så de plockas ur råtexten med `css::inline_styles`). Bara `text-align`,
  `text-indent`, `margin-left`/`padding-left`/`margin`-genvägen, `font-variant: small-caps`, `font-style` och `font-weight` läses. Väljare:
  element, `.klass`, `element.klass`, `#id` och efterkommande kedjor (högst fyra delar); allt med `>`, `+`, `~`, `[`, `:` hoppas över
  (en regel som inte förstås ska aldrig ge fel utseende). Kaskaden följer specificitet och ordning; justering, indrag, små versaler, kursiv och
  fetstil ärvs, vänstermarginaler summeras. `html.rs` håller en stack av element (`ElInfo`) och deras `Computed`.
- **Fetstil och kursiv** (från CSS-klasser och `style`) bakas in i Markdown-texten (`**…**`, `*…*`) och går inte att slå av med *Förlagets
  formatering*. **Justering, indrag, vänstermarginal och små versaler** bärs som osynliga markörer (samma markörer som länkmålen, id som börjar
  med `\u{1}`) som `finish` gör om till `Converted::styles` (byteposition → `BlockLook`), och `Section::blocks` (delens egna positioner).
  Små versaler gäller hela stycket eller rubriken, inte enstaka ord.
- **Ritning**: vyn bygger varje bildruta en `ViewExtras` (justering, blockstilar om förlaget är på, understrykningar) som läggs i
  `egui::Context::data` (`egui_commonmark_backend::misc::set_extras`). Den vendrade `egui_commonmark` ritar då ett stycke eller en rubrik på
  översta nivån som ett enda galleri (en `LayoutJob` för hela blocket, `Pulldown::try_galley`) när blocket har blockstil, när justering är på
  (brödtextstycken) eller när en markering rör det. Galleriet har `justify`/`halign` från egui (sista raden justeras inte), indrag som
  `leading_space`, vänstermarginal som förskjutning, små versaler som versaler i 80 % storlek (byte-längden bevaras så att källpositioner
  stämmer), länkar som färg (som den vanliga etikettvägen) med träffprov via `Galley::cursor_from_pos`, sökträffar och markeringar som bakgrunder, och
  egui:s egen textmarkering (`LabelSelectionState::label_text_selection`). Innehåller blocket något annat (bild, formel, fotnotsreferens) ritas
  det på det gamla sättet. Stycken med indrag som följer på ett stycke får ingen extra luft emellan. Listor, citat och tabeller ritas som förut
  (utan justering).
- Radbrytningen är egui:s (ingen avstavning): en smal spalt med långa ord kan ge stora luckor i justerad text.

## Bokmärken, understrykningar och anteckningar (2.9.0)

- **Data** (`anchor.rs`): `Note` har kapitel (index i `Book::chapters`), byteområde i kapitlets Markdown (delarna hopsatta, `Book::chapter_md`),
  `raw` (Markdown-texten i området), `before`/`after` (32 byte omgivning), `quote` (ren text för listor), färg 0–3, anteckning. `resolve` använder
  området om `raw` fortfarande står där, annars söks `raw` i kapitlet och träffen med mest lik omgivning (och därefter närmast det gamla stället)
  väljs; ingen träff ger `None` (posten finns kvar i listan men ritas inte). Därför tål posterna ändrad delning i delar (`SECTION_LIMIT`),
  ändrad textstorlek (påverkar inte Markdown) och texter som flyttats.
- **Markering**: egui ger ingen åtkomst till markerad text i etiketter. `selection::Tap` är ett `egui::Plugin` vars `output_hook` läser
  `OutputCommand::TextSelectionSettled(text)` och sparar texten och pekarens läge; nästa bildruta tar vyn (`selection::take`) och hittar
  texten i delens Markdown med `anchor::locate` (blanksteg och versaler normaliseras; finns den på flera ställen väljs träffen närmast pekaren via
  `line_points`). Då visas `AnnMenu`. Klick på en understrykning rapporteras av galleriet (`put_mark_hit`) och öppnar menyn för posten.
  Understrykningar i listor, citat och tabeller ritas som bakgrund men går inte att klicka (de ritas inte som galleri).
- **Bokmärke**: `Ctrl+D` (fritt i läsläget; flerpekaren i redigeraren är avstängd här). "Stället" är det som syns: finns ett bokmärke vars
  plats ligger mellan översta synliga blocket och nederkanten tas det bort, annars sätts ett vid översta blockets början.
- **Lagring** (`store.rs`): `<data>/epub/bocker.json` (`Store` → `BookRecord` med `Note`-lista), skriven via en tillfällig fil. Nyckel: OPF:s
  `dc:identifier` som paketets `unique-identifier` pekar på (annars den första), reserv `path:{sökväg}`; en bok hittas via id eller sökväg så att
  en flyttad fil behåller sina poster. Tak: 2 000 poster per bok (äldsta först) och 300 böcker (äldsta utan poster först). `KladdApp::library`
  läses vid start; `epub_sync_notes` (glue) sparar när `EpubTab::take_dirty` är sant, och läget/framsteget sparas som `epub_memory`.
- **Biblioteket**: `epub_restore` gör boken senast läst och sparar omslagsminiatyren (`store::save_cover`: PNG, 160 px hög, en fil per bok);
  `epub_shelf` (glue) ritar de sex senast lästa på startsidan (`welcome` i `app.rs`). Saknad fil visas nedtonad. Menyn *Arkiv → Öppna senaste*
  finns inte i Kladd (de senast öppnade syns på startsidan), så ingen sådan ändring gjordes.
- **Export**: `anchor::export_markdown` (titel, författare, kapitelrubriker i läsordning, citat med `>`, anteckningar, bokmärken) öppnas som ett
  nytt osparat dokument (`epub_export_notes`, kommandot `file.export_notes`).

## Senare (inte gjort)

- Sidvis vy och tvåspaltsvy, helskärm, bildförstoring (klick på en bild).
- Förlagets typsnitt, färger och initialer; små versaler för enstaka ord; avstavning.
- Klick på understrykningar i listor, citat och tabeller.
- Skriv ut och *Spara som* direkt från boken (nu via *Redigera en kopia*).
