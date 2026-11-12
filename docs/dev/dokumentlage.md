# Dokumentläge (Word-läge) – design

Kladd visar `.txt`, `.md` och kod som i dag: kodeditor med radnummer, syntaxfärgning och
Markdown-förhandsvisning. När en flik är ett Word-dokument (`doc.docx == true`) byter
gränssnittet till **dokumentläget**: en formateringsrad under flikraden, en vit A4-sida på grå
bakgrund och en statusrad med ord och sidor. Knappen *Markdown* i formateringsraden växlar
fliken till vanlig textvy och tillbaka.

## Beslut: Markdown är fortfarande modellen

Wordcraft, FreeWord och MiniPad bygger alla en egen rich-text-motor (egen layout, markör,
markering, IME, ångra) på 15 000–400 000 rader. Kladd läser och skriver `.docx` via Markdown
(`docx::read` → `doc.text`, `docx::write` ← `doc.text`) och har inget behov av en egen motor.
Dokumentläget är därför en **vy** över samma `String`:

- Redigeringen sker fortfarande i `egui::TextEdit` (markör, markering, IME, ångra gratis).
- En egen `layouter` ritar Markdown som ett Word-dokument: rubriker stora och blå, fetstil
  fet, kursiv kursiv och så vidare, i typsnittet Carlito (metriskt lika Calibri, Words
  standardtypsnitt).
- **Markupen döljs** (transparent och nästan noll bred) utom på de rader som markören eller
  markeringen står på, där den visas dämpad. Samma idé som "live preview" i Obsidian.
- Knapparna i formateringsraden och Ctrl+B/I/U redigerar Markdown-texten (lägger till eller
  tar bort `**`, `*`, `<u>`, `~~`, `# ` …).

Idéer som lånats från referensprojekten: grå arbetsyta med vita sidor och skugga förskjuten
2 px nedåt (Wordcraft `canvas.rs`, FreeWord `editor.rs`), A4 i 96 dpi med en tums marginaler,
knappar med "aktiv"-läge som tonad rundad ruta med accentkant (Wordcraft `widgets/buttons.rs`),
"slå på om inte allt markerat redan är på" (Wordcraft `cmd/format.rs`), statusrad med
"Sida x av y", ordräkning och zoom.

## Moduler

| Fil | Innehåll |
|---|---|
| `src/word/mod.rs` | `pub mod markup; pub mod layout; pub mod view;` |
| `src/word/markup.rs` | Ren logik utan UI: analys av Markdown, aktiv formatering vid markören, redigeringsoperationer, ordräkning. Enhetstestad. |
| `src/word/layout.rs` | Bygger `egui::text::LayoutJob` för dokumentläget. |
| `src/word/view.rs` | Formateringsraden, sidan, statusraden för dokumentläget. |
| `src/theme.rs` | Carlito-typsnitten och dokumentlägets färger. |
| `src/icons.rs` | Nya ikoner. |
| `src/docx.rs`, `src/pdf_write.rs` | Understrykning (`<u>…</u>`). |

## API (kontrakt mellan modulerna)

### `word::markup`

```rust
pub enum Block { Normal, Heading(u8 /*1..=6*/), Quote, Bullet, Numbered, Code, Table }
pub enum Inline { Bold, Italic, Underline, Strike }

/// En bit av texten med enhetlig stil. Bitarna täcker hela texten i ordning, utan glapp.
pub struct Piece {
    pub range: std::ops::Range<usize>, // byteoffset i texten
    pub block: Block,
    pub bold: bool, pub italic: bool, pub underline: bool, pub strike: bool,
    pub code: bool,       // inline-kod eller kodblock
    pub link: bool,       // länktext
    pub markup: bool,     // Markdown-syntax som döljs: `**`, `# `, `](url)`, `<u>` …
    pub list_marker: bool,// "- " / "1. " i början av ett listobjekt (markup = true för "- ")
    pub image: Option<ImageRef>, // hela `![alt](src)`-biten
}
pub struct ImageRef { pub alt: String, pub src: String }

pub struct Analysis { pub pieces: Vec<Piece> }
pub fn analyze(text: &str) -> Analysis;

pub struct Active { pub bold: bool, pub italic: bool, pub underline: bool, pub strike: bool, pub block: Block }
/// Formatering i markeringen `sel` (byteoffset). Tom markering: vid markören.
pub fn active(text: &str, a: &Analysis, sel: std::ops::Range<usize>) -> Active;

pub struct Edit { pub text: String, pub sel: std::ops::Range<usize> }
pub fn toggle_inline(text: &str, sel: std::ops::Range<usize>, m: Inline) -> Edit;
pub fn set_block(text: &str, sel: std::ops::Range<usize>, b: Block) -> Edit;
pub fn insert_table(text: &str, at: usize) -> Edit;

/// Ord och tecken i den synliga texten (utan markup).
pub fn count(text: &str, a: &Analysis) -> (usize /*ord*/, usize /*tecken*/);
/// Byteintervall för de hela rader som `sel` berör (där markupen ska visas).
pub fn reveal_range(text: &str, sel: std::ops::Range<usize>) -> std::ops::Range<usize>;
```

Markdown-dialekten är den som `docx::read` skriver och `docx::write` läser:
pulldown-cmark med `ENABLE_TABLES | ENABLE_STRIKETHROUGH | ENABLE_TASKLISTS`; fetstil `**`,
kursiv `*` (även `_` läses), genomstruken `~~`, understruken `<u>…</u>`, rubriker `#`–`######`,
listor `- ` och `1. ` (nästlade med indrag), citat `> `, tabeller `|`, bilder
`![alt](bild:namn.png)` eller sökväg, länkar `[text](url)`.

### `word::layout`

```rust
pub struct Params<'a> {
    pub palette: &'a crate::theme::Palette,
    pub zoom: f32,                       // 1.0 = 100 %
    pub reveal: std::ops::Range<usize>,  // från markup::reveal_range
    pub query: &'a str, pub case_sensitive: bool, pub current_match: Option<usize>,
    pub image_size: &'a dyn Fn(&str) -> Option<egui::Vec2>, // src -> visningsstorlek i px vid zoom
    pub measure: &'a dyn Fn(&str, egui::FontId) -> f32,     // textbredd, för tabellkolumner och listindrag
}
pub fn layout_job(text: &str, a: &markup::Analysis, p: &Params) -> egui::text::LayoutJob;
```

Typografi (pt × 96/72 × zoom px): brödtext Carlito 11 pt, radhöjd 1,15; Rubrik 1 18 pt
#1F3864, Rubrik 2 15 pt #2F5496, Rubrik 3 13 pt #2F5496, Rubrik 4 12 pt, 5–6 11 pt (rubriker
fetstil). Samma värden som `docx::styles_xml`. Kod och tabeller: IBM Plex Mono 10 pt.
Länkar #0563C1 understrukna. Citat kursiv #404040. I mörkt tema används ljusare toner från
paletten.

### Paletten (`theme::Palette`, nya fält)

`desk` (grå arbetsyta), `page` (papper), `page_shadow`, `page_text`, `page_muted`,
`page_heading`, `page_heading1`, `page_link`, `page_code_bg`, `page_markup` (dämpad markup
på markörens rad). Ljust: desk #E6E6E6, page vit, text #1F1F1F. Mörkt: desk #0C1118, page
#1B2330, text #E3E7EE.

### Typsnitt (`theme`)

`theme::doc_font(bold: bool, italic: bool) -> egui::FontFamily` ger Carlito Regular, Bold,
Italic och BoldItalic, inbäddade som delmängd (latin, latin-1, latin extended-A, allmän
interpunktion) i `assets/fonts/`.

## Gränssnittet i dokumentläget

- **Formateringsrad** (höjd 40 px, under flikraden): Stil-väljare (Brödtext, Rubrik 1–3,
  Citat) | **F** *K* U ~~S~~ | punktlista, numrerad lista, citat | bild, tabell |
  ångra, gör om. Till höger: zoom (−, %, +) och växeln *Markdown*. Aktiva knappar visar
  tonad bakgrund med accentkant.
- **Sidan**: A4 (794 × 1123 px vid 100 %), marginaler 96 px, vit på grått, skugga. Sidan blir
  lika lång som texten men minst en sida. Ungefärliga sidbrytningar markeras med en
  streckad linje mellan två rader och "Sida N" i marginalen.
- **Listor** dras in som i Word (punkt vid 0,25 tum, text vid 0,5 tum, +0,25 tum per nivå);
  punkterna ritas ovanpå den dolda `- `-markeringen.
- **Tabeller** visas som ett rutnät (Table Grid) med fet rubrikrad. Står markören i tabellen
  visas hela tabellen som Markdown så att den går att redigera.
- **Dold markup** ritas med teckenstorlek 0,01 px och blir därmed i praktiken noll bred.
- **Statusrad**: "Sida x av y", "N ord", zoom. Kodspecifika fält (rad/kolumn, kodning,
  radslut) döljs.
- **Dolt i dokumentläget**: radnummer, förhandsvisningsknappen och förhandsvisningspanelen.
- **Kortkommandon**: Ctrl+B fet, Ctrl+I kursiv, Ctrl+U understruken, Ctrl+Alt+1/2/3 rubrik,
  Ctrl+Alt+0 brödtext, Ctrl+plus/minus/0 zoomar sidan.

## Stavningskontroll (bara i dokumentläget)

Studerat: Wordcraft (egen motor `craft-proof`, Hunspell-ordlistor som läses vid körning,
ordet som skrivs får ingen våglinje förrän markören lämnat det), FreeWord och MiniPad
(`spellbook` 0.4, ordlista laddas i en bakgrundstråd, ordcache, högerklicksmeny med upp till
8 förslag, "Ignorera alla" för sessionen, egen ordlista som textfil), LibreOffice (Hunspell,
röd våglinje medan man skriver, högerklick med förslag, Ignorera alla, Lägg till i ordlistan;
på Ubuntu läses systemets ordlistor i `/usr/share/hunspell`).

Beslut:

- Motor: `spellbook` 0.4 (ren Rust, Hunspell-format, MPL-2.0). Testat med `sv_SE` från
  `hunspell-sv`: laddning ≈ 0,1 s, kontroll några µs per ord, sammansättningar fungerar,
  förslag 30–160 ms (räknas bara vid högerklick).
- Ordlistor buntas **inte** (sv_SE är LGPL-3, stora filer). De läses från `$DICPATH`,
  `~/.local/share/kladd/ordlistor`, `/usr/share/hunspell` och `/usr/share/myspell`.
  `.deb`-paketet rekommenderar `hunspell-sv, hunspell-en-us`.
- Språk: inställningen `spell_lang`; standard `sv_SE` om den finns, annars språket i `$LANG`,
  annars `en_US`, annars första hittade. Visas i statusraden ("Svenska"), klick öppnar en
  lista med installerade språk och växeln "Stavningskontroll".
- Bara dokumentläget (`.docx`). Kod, länkar, bilder, markup och tabeller kontrolleras inte.
  Ordet som markören står i får ingen våglinje.
- Ordregler: bokstäver och siffror (åäö ingår), inre `'`, `’` och `-` ingår om en bokstav
  följer. Hoppa över ord kortare än 2 tecken, ord med siffror, ord med bara versaler,
  webbadresser/e-post/sökvägar. Bindestrecksord godkänns om alla delar godkänns.
- Våglinje i `palette.error`, amplitud ≈ 1,2 px, period ≈ 4 px, under textens baslinje.
- Högerklick på ett felstavat ord: upp till 6 förslag, "Ignorera alla" (sessionen),
  "Lägg till i ordlistan" (sparas i `~/.config/kladd/ordlista.txt`, ett ord per rad).

### `crate::spell`

```rust
pub struct Speller { /* bakgrundsladdad spellbook::Dictionary, ordcache, ignorerade ord, egen ordlista */ }
impl Speller {
    pub fn new(lang: Option<&str>) -> Self;          // börjar ladda i bakgrunden
    pub fn languages(&self) -> &[Lang];              // installerade ordlistor
    pub fn lang(&self) -> Option<&Lang>;
    pub fn set_lang(&mut self, code: &str);
    pub fn ready(&mut self) -> bool;                 // ordlistan är laddad
    pub fn generation(&self) -> u64;                 // ökar när språk/ordlista/ignorerade ändras
    pub fn check_word(&mut self, word: &str) -> bool;
    pub fn misspelled(&mut self, text: &str, ranges: &[std::ops::Range<usize>]) -> Vec<std::ops::Range<usize>>;
    pub fn suggest(&self, word: &str) -> Vec<String>; // högst 6
    pub fn ignore_all(&mut self, word: &str);
    pub fn add_to_dictionary(&mut self, word: &str);
}
pub struct Lang { pub code: String /* "sv_SE" */, pub name: String /* "Svenska" */, pub aff: PathBuf, pub dic: PathBuf }
pub fn words(text: &str, range: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>>; // ordregler ovan
```

## Typsnitt, storlekar, färger och justering (1.5)

Formatet i Markdown-texten är HTML, så att det också fungerar i andra Markdown-program och på
GitHub. Allt tolkas och skrivs med `src/style.rs`:

- Tecken: `<span style="font-family: Arial; font-size: 14pt; color: #C00000; background-color: #FFFF00">…</span>`.
  Bara egenskaper som skiljer sig från dokumentets grundformat skrivs. Markering
  (`w:highlight`) och skuggning (`w:shd`) blir båda `background-color`; vid skrivning blir en
  exakt markeringsfärg `w:highlight`, annars `w:shd`. Spans ligger ytterst:
  `<span …><u>**x**</u></span>`.
- Stycken: `<div align="center">`, tom rad, styckena, tom rad, `</div>` (som GitHub-README:s).
  `left` skrivs aldrig. Intilliggande stycken med samma justering delar en div.
- Grundformat: första raden `<!-- kladd: font-family: "Times New Roman"; font-size: 12pt -->`
  när dokumentets standardtypsnitt (docDefaults + formatmallen Normal) inte är Calibri 11 pt.
- Typsnitt hämtas från systemet med fontconfig (`fc-match`), så Arial blir Liberation Sans och
  Times New Roman Liberation Serif på Ubuntu precis som i LibreOffice. Hittas inget används
  Carlito (sans) eller systemets serif/monospace efter namnet.
- Mörkt tema: dokumentfärger med för låg kontrast mot sidan ljusas upp (som Words mörka läge).

Dokumentläget lägger ut varje stycke för sig (egen radbredd, justering och hängande indrag)
och fogar ihop styckena med `Galley::concat`. Marginaljustering fördelar utrymmet på
mellanrummen i alla rader utom styckets sista.

## 1.6: förlustfri Word, sidor, inklistring, tabeller, bilder, utskrift, ODT

Gemensamma format (alla moduler ska använda dessa):

- **Sida, sidhuvud och sidfot** står i grundformatskommentaren (`style::DocDefaults`):
  `<!-- kladd: page-size: A4; page-margin: 25mm 20mm; header: "Rapport"; header-align: right;
  footer: "Sida {sida} av {sidor}"; footer-align: center -->`. Standard är A4 med en tums
  marginaler och inget sidhuvud eller sidfot. `style::fill_fields` fyller i `{sida}`/`{sidor}`.
- **Bilder med storlek**: `<img src="bild:image1.png" alt="Diagram" width="240">` (bredd i
  CSS-pixlar, 1/96 tum; `height` valfri, annars bevaras proportionerna). Bilder utan storlek
  skrivs som förut, `![alt](src)`. Justering av en bild görs med `<div align>` runt den.
- **Okända Word-objekt** (fotnotsreferenser, kommentarsankare, bokmärken, fält, textrutor,
  former, symboler) blir osynliga markörer `<!--k:N-->` i texten. N pekar på originalets XML i
  `Document`, och vid sparning skrivs originalet tillbaka. I dokumentläget är markörerna dolda
  (noll bredd); i Markdown-förhandsvisningen tas de bort.
- **Förlustfri .docx**: originalpaketet sparas i `Document`. Vid sparning kopieras alla delar
  som Kladd inte skriver om (sidhuvuden, sidfötter, fotnoter, kommentarer, inställningar, tema,
  typsnittstabell …), `w:sectPr` behålls, och varje stycke vars Markdown inte har ändrats skrivs
  tillbaka med sin ursprungliga XML.

## Inklistring av formaterad text (`src/paste.rs`)

Ctrl+V i dokumentläget läser HTML från systemets urklipp (`arboard`) och gör om den till
Kladds Markdown med `paste::html_to_markdown(html, Some(&defaults))`: rubriker (även Words
`MsoTitle`/`MsoHeading*`/`mso-outline-level`), fet/kursiv/understruken/struken (taggar och CSS),
`<span style>` för typsnitt, storlek, färg och bakgrund (utom svart text, vit bakgrund och
dokumentets grundformat), `<div align>`, listor (även Words `mso-list`-låtsaslistor och Google
Dokuments platta `aria-level`-listor), tabeller (cellerna slås ihop till en rad), länkar och
`data:`-bilder som `bild:inklistrad-N.ext` (med `width` om den är känd). Övriga bilder blir sin
alternativtext. HTML läses av en egen förlåtande tolk, utan extra crate.

`word_paste_html` (view.rs) tar bort `Event::Paste` innan TextEdit ser den och sätter in
resultatet med `paste::insert` (block får tomma rader runt sig, ett inline-stycke sätts in
direkt). Ctrl+Skift+V och urklipp utan HTML går som förut. egui skickar `Paste` bara när
urklippet också har vanlig text, vilket Word, LibreOffice, Google Dokument och webbläsare alltid
lägger dit.

### OpenDocument-text (.odt)

`src/odt.rs` läser och skriver `.odt` (LibreOffice) med samma Markdown-dialekt och dokumentläge som
`.docx`: `odt::read(bytes) -> docx::Loaded` och `odt::write(markdown, titel, resolve) -> docx::Written`.
Formatmallar (`office:styles` och `office:automatic-styles` i `styles.xml`/`content.xml`) löses längs
`style:parent-style-name`; bara det som skiljer sig från grundformatet (standardstycket + Standard)
blir `<span style>`. Fotnoter, kommentarer, former/diagram och länkade bilder tas bort och rapporteras
som borttagna (som för .docx). Sida, marginaler samt standardsidans sidhuvud/sidfot (`text:page-number`
→ `{sida}`, `text:page-count` → `{sidor}`) hamnar i grundformatskommentaren. Vid skrivning ligger
`mimetype` först och okomprimerad. `Document.docx` är sant även för `.odt` (dokumentläget);
`language()` visar "OpenDocument".

## 1.7: säkerhetskopior, fotnoter, tabeller, flytande bilder, navigering, export, sök, prestanda

Gemensamma format (alla moduler ska använda dessa):

- **Fotnoter**: Markdown-fotnoter. `[^1]` i texten och definitionen `[^1]: Fotnotens text` i slutet
  av dokumentet (en definition per rad, efter all annan text, separerade från den med en tom rad).
  Etiketterna är löpnummer 1, 2, 3 … i den ordning referenserna kommer. pulldown-cmark med
  `Options::ENABLE_FOOTNOTES`. Word: `w:footnoteReference` + footnotes.xml; ODT: `text:note`.
- **Tabeller**:
  - Radbrytning i en cell: `<br>`.
  - Sammanfogade celler (vågrätt): en cell följs av extra pipes, `| Rubrik över två ||`
    (MultiMarkdown). Antalet extra `|` = antalet celler den spänner över minus ett. Lodrätt
    sammanfogade celler stöds inte (de skrivs som tomma celler).
  - Kolumnbredd: antalet bindestreck i avgränsarraden, `| ------ | -- |`, ger kolumnernas relativa
    bredd (som Pandoc). Lika många streck (eller `---` överallt) = automatisk bredd.
- **Flytande bilder**: `<img src="…" alt="…" width="200" align="left">` (eller `right`): texten i
  följande stycken flyter runt bilden. Utan `align` är bilden en egen rad som förut.
- **Innehållsförteckning**: en egen rad `<!-- kladd: toc -->`. Dokumentläget ritar rubrikerna med
  sidnummer (klickbara); Word skriver ett TOC-fält, ODT `text:table-of-content`, PDF/HTML/EPUB en
  lista med rubrikerna.

## Prestanda

Mätt med `cargo test [--release] -- --ignored bench --nocapture --test-threads=1`
(`src/word/bench.rs`): ett genererat dokument på 600 KB / 249 sidor (rubriker, fetstil,
listor, tabeller) i dokumentläget och 300 KB Markdown i textläget.

| Mätning | Release före | Release efter | Debug före | Debug efter |
|---|---|---|---|---|
| (a) kall full layout (analys + galleri + sidbrytning) | 342 ms | 340–380 ms | 8 770 ms | 8 980 ms |
| (b) ett tecken i mitten | 81 ms | 35–44 ms | 357 ms | 137 ms |
| (b2) markören byter rad | 42 ms | 7–10 ms | 262 ms | 41 ms |
| (c) oförändrad bildruta | 2,0 ms | 0,1 ms | 10,8 ms | 0,1 ms |
| (d) stavning efter ett tecken | 12 ms | 4–8 ms | 90 ms | 27 ms |
| (e) textläge, kall färgning + layout | 622 ms | 540–590 ms | 10 173 ms | 8 950 ms |
| (e) textläge, ett tecken | 350 ms | 12 ms | 3 646 ms | 105 ms |
| (e) textläge, oförändrad bildruta | 3,7 ms | 0,0 ms | 34,8 ms | 0,0 ms |

Hur det fungerar:

- **Ändringsdetektor.** `layout::Cache` håller en kopia av texten per dokument och jämför
  (memcmp) i stället för att hasha 600 KB varje bildruta. Ändrad text ger ny revision, ny
  analys och nya listor över tabeller, bilder och typsnitt; annars görs ingenting.
- **Styckecache.** `galley_paged` lägger ut dokumentet stycke för stycke (en tabell är ett
  stycke). Nyckeln är styckets text, dess bitar (relativt styckets början), geometri
  (justering, listindrag), markörens träff och sökträffar, plus parametrar som zoom, tema,
  bredd och typsnittsgeneration. Efter en ändring läggs bara berörda stycken om; markören som
  byter rad berör två. Sedan fogas styckena ihop (`Galley::concat`) och sidbryts. Cachen
  rensas på stycken som inte användes i senaste bildrutan.
- **Ritning.** Punkter, citatstreck, bilder, tabellkanter och våglinjer itereras bara för det
  som är synligt (bitarna slås upp med binärsökning från galleriets synliga tecken).
- **Stavning.** Resultatet cachas per revision och per stycke.
- **Textläge.** `highlight::Highlighter` sparar syntects tillstånd före varje rad. En ändring
  färgar om raderna från den ändrade tills tillståndet efter en rad är lika med det förra
  (då gäller resten av färgningen), och färdigt galleri cachas per revision och bredd.
- **Analysen** (`markup::analyze`, ca 28 ms release / 85 ms debug för 600 KB) görs fortfarande
  om för hela texten efter varje ändring; den är nu det som dominerar. Inkrementell analys per
  block bedömdes osäker (spann och dividrar går över styckegränser) och är inte gjord.

Testerna `incremental_layout_equals_full_layout` och `incremental_highlight_equals_full`
kontrollerar att det inkrementella resultatet är identiskt med full omläggning efter
slumpmässiga ändringar (deterministiskt frö).
