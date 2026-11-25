# EPUB-läsläget (2.8.0)

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

## Senare (inte gjort)

- Sidvis vy och tvåspaltsvy, bokmärken, understrykningar och anteckningar, helskärm, bildförstoring (klick på en bild).
- Förlagets typsnitt och CSS (centrerade rubriker, versaler, initialer), justerad text.
- Centrerade stycken (`text-align: center`) ritas vänsterställda.
- Skriv ut och *Spara som* direkt från boken (nu via *Redigera en kopia*).
