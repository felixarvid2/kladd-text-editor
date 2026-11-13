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
