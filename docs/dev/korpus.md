# Dokumentkorpus

`tests/corpus/` har riktiga dokument som testas i `src/corpus_tests.rs` (vanliga `cargo test`, alla OS).
Källa och licens per fil står i `tests/corpus/SOURCES.md`; bara tillåtande licenser (MIT, BSD, Apache-2.0, MPL-2.0).
Håll den under ca 15 MB och föredra små filer.

## Vad som testas

- `corpus_docx`, `corpus_odt`: filen läses utan panik, texten är rimlig (inga styrtecken, inget läckt XML, inte tom), antalet
  rubriker och de tappade delarna (`dropped`) stämmer med manifestet. Sedan sparas den oförändrad: paketet är giltigt,
  omläsning ger samma Markdown (bildnamn och grundformatsraden `<!-- kladd: … -->` undantagna) och en andra sparning
  ändrar inget. Till sist läggs ett stycke sist; efter sparning och omläsning ska allt annat vara kvar.
- `corpus_pdf`: `pdf_read::read` utan panik, `PdfEngine::open` ger samma sidantal, första sidan renderas i låg upplösning.
  Krypterade filer ska ge `NeedsPassword`.
- `corpus_pptx_miniatyrer` (`src/pptx/tests.rs`): bild 1 i varje `pptx/*.pptx` jämförs med PowerPoints egen miniatyr
  (`docProps/thumbnail.jpeg`); måttet i `pptx/manifest.toml` får inte sjunka mer än 0,03. Se `docs/dev/pptx.md`.
- `corpus_soffice_docx`, `corpus_soffice_odt` (`#[ignore]`): de sparade filerna, orörda och ändrade, konverteras med
  `soffice --headless --convert-to pdf`. Kör med `KLADD_CORPUS_SOFFICE=1 cargo test corpus_soffice -- --ignored`;
  utan variabeln hoppas testet över om `soffice` saknas, med den är det ett fel. CI-jobbet *korpus i LibreOffice* gör detta.

## Manifestet

`tests/corpus/manifest.toml` har en sektion per fil. `KLADD_CORPUS_BLESS=1 cargo test corpus_` skriver om de uppmätta
värdena (`headings`, `dropped`, `pages`, `error`) och lämnar handskrivna nycklar: `empty`, `note` och
`skip_idempotent`, `skip_edit`, `skip_render`, `skip_soffice` (värdet är orsaken, och är ett känt fel eller en gräns).
Granska diffen i manifestet innan du checkar in den.

Felsökning: `KLADD_CORPUS_DUMP=docx/namn.docx cargo test corpus_dump -- --ignored --nocapture` skriver ut Markdown
före och efter sparning.

## Känt kvar (se `skip_*` i manifestet)

- Nästlade listor med 4+ mellanslags indrag, och förformaterad text, blir kodblock vid omläsning.
- Odt: tomt inledande avsnitt tappas; dokument med bara tomma avsnitt blir tomma; fetstil i span i tabellceller och
  textfärg från sidmall tappas; bilder bredare än textytan krymps; blanksteg efter en löpande bild tappas.
- Docx: i dokument utan egna rubrikmallar får grundformatsraden `h1: "Cambria"; …` (temats typsnitt) efter första sparningen, eftersom Kladds egna rubrikmallar läggs till. Brödtexten påverkas inte, och en andra sparning ändrar inget; testet jämför därför texten utan grundformatsraden.
- Diagram och textrutor syns inte som text (de bevaras orörda vid sparning).

## Nya filer

Lägg filen i rätt mapp, skriv in källa och licens i `SOURCES.md`, kör med BLESS och granska manifestet.

## Bevarande och slumpprov för Word och ODT (`src/word_fuzz_tests.rs`)

Två ignorerade prov (inget ändras i vanliga `cargo test` utom de små regressionsproven i samma fil).

**`word_bevarande`** (`KLADD_WORD_PRESERVE=1 cargo test word_bevarande -- --ignored --nocapture`): öppnar varje
korpusfil, sparar orörd och efter en liten ändring (ett ord i första textstycket och ett nytt stycke) och jämför
originalets delar med det sparade: oförändrad, omskriven med samma element, ändrad, saknas; samt en inventering av
alla element och attribut (utan brus: `rsid`, `proofErr`, `w14:paraId`) och vad som tappats, per del och per fil, med
och utan att `dropped` varnat. `KLADD_WORD_DETAIL=Docx` skriver förlusterna per fil för den oförändrade sparningen.
`word_en_fil`, `word_en_fil_stycken` och `word_odt_andring` (`KLADD_WORD_FILE=fil`) visar vad som skiljer en enskild fil.

Resultat (Word, 209 filer): alla delar Kladd inte äger (sidhuvud, sidfot, kommentarer, fotnoter, tema, inställningar,
anpassad XML, inbäddade objekt, bilder) följer med byte för byte vid en oförändrad sparning; `styles.xml`,
`numbering.xml`, `[Content_Types].xml`, relationerna och `core.xml` kompletteras (inga element tappas). I
`document.xml` var det som tappades tyst: `w:pgNumType w:fmt="decimal"` (20 filer), första avsnittets
`w:type continuous` (2), listor som hoppar över en nivå (2: skrevs om som kodblock med läckta markörer; `docx4j-cn-1-labels`)
och revisions-id/stavningsmarkeringar (harmlöst). Alla tre rättade (`apply_section_at`, `section_hf_edit`, `list_eff`).
Vid en ändring gick innehållskontrollens (`w:sdt`) tagg, titel och platshållare förlorade när texten i den ändrades
(5 filer); den behålls nu (`with_orig_sdt`). Kvar och inte varnat (kända begränsningar i Markdown-modellen, gäller bara
det ändrade stycket): `w:hyperlink w:anchor` (inre länkar) blir vanlig text, teckenformat som Markdown inte har
(`w:lang`, `w:rFonts`, storlek) och ritningsdetaljer (`a:ln`, `mc:AlternateContent`) i ett ändrat stycke som också har en
bild. `docx4j-NumberingIndents` har text som liknar XML med hakparenteser (`skip_idempotent` i manifestet) och
tappar sin numrering vid sparning.

ODT skrivs alltid om från Markdown (ingen `Source`): brödtextens element bevaras (inventeringen under `office:body`
tappar bara `text:sequence-decls`), men inga delar utanför innehållet följer med: `settings.xml`, `manifest.rdf`,
miniatyren och `Configurations2` (visnings- och verktygsinställningar, 160–180 filer, ingen varning, ofarligt),
inbäddade objekt (5 filer, alla varnade), makron (2 filer, en varnad) och bilder som bara används som bakgrund i
stilar (11 filer, 9 tysta; nu varnade med *bakgrundsbilder*).

**`word_slumpprov`** (`KLADD_WORD_FUZZ=<antal> KLADD_WORD_FUZZ_SEED=<n> [KLADD_WORD_FUZZ_OUT=<mapp>] cargo test
word_slumpprov -- --ignored --nocapture`): seedad mutator över Word- och ODT-korpusen (zip: ta bort och byta namn på
poster, trunkera, byta byte; XML: ta bort/duplicera element och attribut, extrema och negativa tal, ogiltig UTF-8,
okända namnrymder, upprepade och obalanserade taggar; relationer och hänvisningar: mål på sig själv, en annan del eller
en som saknas). Varje fall: öppna, visa (Word-vyns analys, tabeller, typsnitt, sidlayout och sidräkning via egui),
tre slumpade redigeringar av Markdown-texten (skriva, radera, fetstil, nytt stycke, tabellcell, lista) som sparas var
för sig och öppnas igen, med kontroll att varje sparad fil är en giltig zip med välformad huvuddel och (Word)
`[Content_Types].xml`. Under `catch_unwind` med 40 s tidsgräns i en egen tråd; minnet mäts som ökning av `VmHWM`.
Resultat före rättelserna (2 200 fall): en klass (docx utan `[Content_Types].xml` sparades utan det, 11 fall); före
den andra rättelsen (3 200 fall): en klass (ODT-skrivaren skar en sträng mitt i ett flerbytetecken, 1 fall);
efter (3 200 fall): inga. Ingen hängning och ingen minnesbomb (störst ökning per fall ca 120 MB). Samma
flerbytetecken-fel hittades med sökning i `docx.rs` (fältinstruktioner) och rättades med.
