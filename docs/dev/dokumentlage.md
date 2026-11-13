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
  marginaler och inget sidhuvud eller sidfot. `style::fill_fields` fyller i `{sida}`, `{sidor}`, `{datum}`,
  `{titel}` och `{fil}` (`style::Fields`). `|` delar i vänster|mitten|höger (`style::parts_of`,
  `join_parts`; `\|` är ett riktigt streck) och `header-first`/`footer-first` är första sidans
  varianter (`Some("")` = tom första sida; `DocDefaults::header_template(sida)`). Dialogen för
  redigering på sidan finns i `word/headers.rs`. docx: `titlePg` + `type="first"`-referenser,
  fält som PAGE/NUMPAGES/DATE/TITLE/FILENAME och flikar för delarna; odt: `style:header-first`.
- **Formatmallar** (`style::StyleSet`, `src/style/formatmall.rs`): en deklaration per mall i
  grundformatskommentaren, `h1`–`h6`, `body`, `quote`, med ord i valfri ordning: `"Typsnitt"`,
  `16pt`, `bold`/`nobold`, `italic`/`noitalic`, `#RRGGBB`, `left|center|right|justify`,
  `space-before L`, `space-after L`, `line-spacing N` (gånger enkelt, `LINE_SINGLE` = 1,25 i
  layouten), `indent L` (första raden). Layouten läser dem i `piece_format` (typsnitt, storlek,
  färg, radhöjd) och `paragraphs` (justering, luft, indrag). Luft efter ett stycke läggs som luft
  före nästa, eftersom `Galley::concat` staplar efter radernas höjd. Docx: `docx_styles.rs`
  läser Normal/Heading1–6/Quote ur `styles.xml` (bara det mallen själv anger, minus det Kladds
  egna mallar anger) och skriver tillbaka bara de egenskaper som ändrats. Odt: `Styles::style_set`
  och `apply_style_overrides`. Dialogen: `word/stylesdlg.rs`.
- **Bilder med storlek**: `<img src="bild:image1.png" alt="Diagram" width="240">` (bredd i
  CSS-pixlar, 1/96 tum; `height` valfri, annars bevaras proportionerna). Bilder utan storlek
  skrivs som förut, `![alt](src)`. Justering av en bild görs med `<div align>` runt den.
- **Okända Word-objekt** (kommentarsankare, bokmärken, fält, textrutor,
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
→ `{sida}`, `text:page-count` → `{sidor}`, `text:date`, `text:title`, `text:file-name`, flikar och `style:header-first`) hamnar i grundformatskommentaren. Vid skrivning ligger
`mimetype` först och okomprimerad. `Document.docx` är sant även för `.odt` (dokumentläget);
`language()` visar "OpenDocument".

## 1.7: säkerhetskopior, fotnoter, tabeller, flytande bilder, navigering, export, sök, prestanda

Gemensamma format (alla moduler ska använda dessa):

- **Fotnoter**: Markdown-fotnoter. `[^1]` i texten och definitionen `[^1]: Fotnotens text` i slutet
  av dokumentet (en definition per rad, efter all annan text, separerade från den med en tom rad).
  Etiketterna är löpnummer 1, 2, 3 … i den ordning referenserna kommer. pulldown-cmark med
  `Options::ENABLE_FOOTNOTES`. Word: `w:footnoteReference` + footnotes.xml; ODT: `text:note`.
  Se [Fotnoter](#fotnoter-17) nedan.
- **Tabeller**:
  - Radbrytning i en cell: `<br>`.
  - Sammanfogade celler (vågrätt): en cell följs av extra pipes, `| Rubrik över två ||`
    (MultiMarkdown). Antalet extra `|` = antalet celler den spänner över minus ett. Lodrätt
    sammanfogade celler stöds inte (de skrivs som tomma celler).
  - Kolumnbredd: antalet bindestreck i avgränsarraden, `| ------ | -- |`, ger kolumnernas relativa
    bredd (som Pandoc). Lika många streck (eller `---` överallt) = automatisk bredd.
- **Flytande bilder**: `<img src="…" alt="…" width="200" align="left">` (eller `right`): texten i
  följande stycken flyter runt bilden. Utan `align` är bilden en egen rad som förut.
  - `markup::ImageRef.align: Option<Float>`; `set_image_float(text, range, Option<Float>)` skriver
    om attributet, `set_image_size` behåller det, `move_image`/`image_block`/`drop_line` flyttar en
    bild (hela raden, och `<div align>`-blocket om bilden var det enda i det).
  - Layout: en bild med `align` på en rad ensam kollapsar raden (~0 höjd, dold). Bilden ritas i en ruta vid
    vänster/höger textkant som börjar vid styckets överkant (även när bilden står i ett stycke med
    text, som Word läses in). Stycken (och styckets egna rader) vars överkant ligger ovanför bildens
    underkant läggs ut med radbredden minskad med bildens bredd + 12 px, vänsterbilder förskjuter
    raderna åt höger. **Per rad**: stycket läggs ut smalt, delas vid första raden som börjar under
    bilden (`layout_para_floats`) och resten läggs ut igen med full bredd. Utläggningen går
    stycke för stycke i ordning (`relayout`) eftersom bredden beror på var bilden slutar; styckecachen
    nycklas också på de aktiva bildernas relativa underkant.
  - Fallgropar: en andra bild på samma sida som en aktiv, eller en bild som lämnar mindre än 96 px
    text, flyter inte (egen rad). Tabeller flyter inte runt bilder.
  - Sidbrytning: `pages::paginate` räknar bildens höjd som en rad på förankringsraden; ryms den inte
    flyttas den, och texten efter den, till nästa sida.
  - Word: `wp:anchor` med `wrapSquare/Tight/Through` och `positionH` (`align` left/right, eller
    `posOffset` nära vänster/höger marginal) läses som `align`; skrivs som `wp:anchor` med
    `wrapSquare wrapText="bothSides"`, `positionH relativeFrom="column"`, `positionV
    relativeFrom="paragraph"` offset 0, `distL`/`distR` 114300 EMU mot texten. ODT: `draw:frame`
    `text:anchor-type="paragraph"` och grafikformat med `style:wrap="right|left"` (bilden
    till vänster/höger) och `style:horizontal-pos`. PDF: samma per-rad-flöde (`FloatBox`). HTML/EPUB:
    CSS `float`.
  - Flytt med musen: dra i bildens mitt (inte ett handtag); `images::Act::Move` utförs via
    `word_edit` (ett ångra-steg). Esc och släpp utanför arken avbryter.
- **Innehållsförteckning**: en egen rad `<!-- kladd: toc -->`. Dokumentläget ritar rubrikerna med
  sidnummer (klickbara); Word skriver ett TOC-fält, ODT `text:table-of-content`, PDF/HTML/EPUB en
  lista med rubrikerna.
  Dokumentläget reserverar raden (`Para::toc_h`, höjd efter antalet H1–H3-rubriker, högst en sida)
  och ritar posterna ovanpå (`navui::paint_toc`); sidnumret är rubrikens sida i det redan
  sidbrutna galleriet, så en passage räcker. PDF gör två passager (`pdf_write::lay_out`).
- **Navigeringsfönster** (`word::nav`, `word::navui`): rubriker ur den cachade analysen
  (`layout::Cache::headings`, dokumentläget) eller en radskanning (`nav::headings_scan`, textläget)
  som görs om bara när texten ändrats.

Tabelllayout (`src/word/grid.rs`): varje tabellrad läggs ut som ett galleri per cell (radbredd =
kolumnbredd − cellmarginal, `<br>` ger ett galleri per del) och cellernas rader placeras bredvid
varandra med `PlacedRow::pos`; raderna ligger i källordning så teckenräkningen är exakt.
Sidbrytningen flyttar en tabellrads alla rader tillsammans (`paginate` med `groups`). Eftersom
raderna överlappar i y duger inte `Galley::cursor_from_pos` och `cursor_up_one_row`: musklick
(`tables::pointer_fix`) och Upp/Ned (`grid::vertical`) räknas ur tabellens geometri (`TableGeom`).
Kolumnkanter dras med egna träffytor ovanpå TextEdit; släpp skriver om avgränsarraden
(`markup::table_set_widths`).

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

## Fotnoter (1.7)

**Modellen** (`word::markup`): `analyze` markerar `[^` och `]` som dold markup och etiketten som en egen bit
(`Piece::footnote_ref`); en definition `[^n]: text` är `Block::Footnote` med `[^` och `]:` som markup och
etiketten som `Piece::footnote_def`. Operationerna arbetar på texten (pulldowns `FootnoteReference` och
`FootnoteDefinition` ger lägena, kod hoppas över):

```rust
pub fn insert_footnote(text: &str, caret: usize) -> Edit;        // [^k] vid markören + tom definition, numrerar om
pub fn renumber_footnotes(text: &str) -> String;                 // 1..n i referensernas ordning, definitioner sist i samma ordning
pub fn renumber_footnotes_at(text: &str, caret: usize) -> (String, usize);
pub fn delete_footnote(text: &str, label: &str) -> String;       // referens + definition, numrerar om
pub fn footnotes_in_order(a: &Analysis) -> bool;                 // snabb kontroll för lat omnumrering
```

Föräldralösa definitioner (utan referens) behålls sist och får bara ny etikett om den krockar. Ordräkningen
räknar inte etiketterna.

**Dokumentläget**: etiketterna ritas med 0,65 × storlek, `valign = TOP` och `line_height = None` (med egen
radhöjd centrerar egui alltid texten, så `valign` gör annars ingenting) i länkfärg; definitionerna med
0,85 × brödtextens storlek under en skiljelinje (en tredjedels textbredd) före den första definitionen.
Hovrar man ett nummer visas "Gå till fotnot" (Ctrl+klick); ett klick hoppar när knappen släpps (markören
flyttas vid nedtryckningen och ändrar layouten, så hoppet kan inte ske vid nedtryckningen). Ett vanligt klick
hoppar bara om raden inte redan visar sin markup, annars krävs Ctrl. Efter varje ändring numreras fotnoterna
om om etiketterna inte längre är 1..n i ordning (`word_renumber_footnotes`). Backspace direkt efter (Delete
direkt före) en referens tar bort hela fotnoten. Verktygsrad: *Infoga fotnot*, `Ctrl+Alt+F`.

**Word** (`docx.rs`): `w:footnoteReference` blir `[^N]` (N i dokumentordning) och texten läses ur
footnotes.xml (formaterade körningar som Kladds inline-dialekt, flera stycken med `<br>`, skiljelinjer
överhoppade; jämförs mot fotnotsmallens eget format så att inga `<span>` behövs). Slutnoter och referenser
med egen märkning (`w:customMarkFollows`) förblir `<!--k:N-->`. `Source` sparar originalets footnotes.xml
(`FnPart`) och id:n per toppnivåelement (`SrcElem::fn_ids`). Vid skrivning:

- stycken jämförs med fotnotsetiketterna borttagna (`norm` → `[^]`), så ett orört stycke behåller sin
  ursprungliga XML (med originalets id) även när fotnoterna numrerats om;
- referenserna i orörda stycken paras ihop med originalets id i ordning; i omskrivna stycken ges varje
  referens en platshållare som `resolve_footnotes` byter mot ett id: originalfotnoten med samma text om det
  finns en, annars ett nytt id;
- footnotes.xml byggs om från definitionerna: skiljelinjer och oförändrade fotnoter ordagrant, ändrade och nya
  genereras (`FootnoteText`, `FootnoteReference`, `w:footnoteRef`); fotnoter som ingen längre refererar till
  faller bort, och fotnoter som okända objekt fortfarande refererar till behålls;
- utan footnotes.xml skapas delen med relation, innehållstyp, skiljelinjer och `w:footnotePr` i settings.xml;
- länkar och bilder i omskrivna fotnoter skrivs som text (delen har egna relationer).

**ODT** (`odt.rs`): `text:note` (fotnot och slutnot) blir `[^N]` och en definition; skrivs som `text:note`
med `text:note-citation` och `text:note-body` (stycke `Footnote`, 10 pt, `text:notes-configuration`).

**PDF** (`pdf_write.rs`): noterna ritas längst ned på sidan där de refereras. `text_block` räknar ut
notens höjd (med avskiljaren) för varje rad med en referens och byter sida om raden och noterna inte ryms;
`reserved` minskar sidans nedre gräns, och `flush_notes` ritar avskiljaren och noterna (0,85 × storlek)
när sidan fylls. Referensen är ett litet upphöjt nummer. En enskild not som är längre än en sida delas inte.

**HTML/EPUB** (`export.rs`) hade redan fotnoter via pulldown (`<aside class="footnote">`, EPUB-`noteref`);
kontrollerat med de befintliga testerna.

## Granska: spårade ändringar och kommentarer

Ren textlogik i `src/review.rs`, .docx-delen i `src/docx_review.rs` (underordnad `docx`), gränssnitt i
`src/word/reviewui.rs`.

**Modellen** (Markdown-dialekten):

- Infogad text `<ins data-k="3">text</ins>`, raderad text `<del data-k="4">text</del>`. `data-k` är ändringens
  nummer; flera taggar kan dela nummer (en Word-ändring över körningar med olika format blir en tagg per
  körning, och godkänn/avvisa tar alla med samma nummer). Taggarna ligger utanför `**`/`<u>`/`<span>` och
  inom ett stycke; en ändring går aldrig över en radbrytning. Författare och datum står inte i taggen utan i
  en postrad `<!--k:r id="3" author="Ada" date="2026-05-01T10:00:00Z"-->`.
- Kommentarens område `<!--k:c:ID-->…<!--k:/c:ID-->` i texten (dold markup som `<!--k:N-->`). Börjar
  området först på raden skyddas det av `&#8203;` framför (annars blir raden ett HTML-block, som för
  objektmarkörerna), och `&#8203;` döljs. Kommentaren själv är en postrad
  `<!--k:c id="1" parent="0" author="Ada" date="…" done="1" text="Hej &amp; hå"-->` (text och namn
  XML-skyddade, `&#10;` för radbrytning). Ett svar har `parent` (trådens rot) och delar områdets läge med
  roten (markörerna ligger intill förälderns). Rotkommentarens nummer är trådens nummer.
- Postraderna ligger sist i dokumentet (efter fotnoterna), en per rad. De är tomma rader i dokumentläget
  (hela raden är dold markup och kollapsar som `<!-- … -->`-rader).
- `review::scan` hittar taggar, markörer och poster (hoppar över kodstaket). `markup::analyze` döljer dem
  och ger bitarna `Piece::rev` (`RevMark`: slag, nummer, författarens färgindex) och `Piece::comment`
  (trådens nummer). Färgindex är författarens plats i ordningen de först förekommer (`review::authors`),
  så två författare aldrig delar färg. Raderad text räknas inte som ord och stavningskontrolleras inte.

**Operationer** (`review.rs`, alla `&str` → `String`/`Edit`): `accept_all`, `reject_all`, `resolve_range`
(godkänn/avvisa det som rör markeringen), `next_change`/`prev_change`, `add_comment`, `add_reply`,
`delete_comment`, `edit_comment`, `threads`, `thread_at`, `prune` samt `clean_for_export` (godkänner allt och
tar bort kommentarer: används av PDF, HTML, EPUB, ODT och förhandsvisningen). Ett stycke som bara bestod av en
godkänd radering tas bort helt.

**Spåra ändringar vid skrivning** (`review::track_edit`): editorn är fortfarande en vanlig `TextEdit`. Efter
bildrutan jämförs texten före och efter (gemensamt prefix/suffix) och redigeringen skrivs om:

- infogad text får `<ins>` (skriver man inne i, eller direkt efter, sin egen infogning utökas den i stället för
  att få en ny tagg), radbrytningar och radprefix (`#`, `-`, `1.`, `>`) ingår inte;
- raderad text får `<del>`; text i sin egen infogning försvinner på riktigt, text som redan är struken och
  taggar/markörer/radbrytningar stannar; Backspace/Delete över en dold tagg går vidare till nästa synliga tecken;
  en radering intill sin egen radering slås ihop med den; markören står före (Backspace) eller efter (Delete);
- infogning inne i en tagg, i en radering eller i någon annans infogning flyttas till efter den;
- ersättning av en markering ger `<del>` följt av `<ins>`; kodblock spåras inte;
- ögonblicksbilden tas bara när tangenter som kan redigera trycktes (inte vid Ctrl+Z/Y), så texten klonas inte
  varje bildruta. Radbrytningar (Enter) och formateringsknappar spåras inte (formatering skrivs direkt).

**Dokumentläget**: infogat understruket och raderat genomstruket i författarens färg (`layout::piece_format`),
kommentarsområdet med gul bakgrund (`review::comments_visible`, inställningen "Visa kommentarer"), och ett fält
till höger om sidan (`reviewui::paint_pane`, 250 px + 16 px luft; sidan flyttas åt vänster och zoomen anpassas så
att båda ryms). Korten ligger vid sin text och skjuts ned så att de inte överlappar; ett klick på ett kort
markerar området, ett dubbelklick redigerar kommentarens text, det valda kortet (markören i området) har svarsfält
samt *Svara*, *Löst* och *Ta bort*. Verktygsraden har *Spåra: på/av* och menyn *Granska* (spåra `Ctrl+Skift+E`,
godkänn/avvisa ändring, godkänn/avvisa alla, föregående/nästa, ny kommentar `Ctrl+Alt+M`, svara, ta bort, visa
kommentarer, *Ditt namn…*). Inställningarna `author_name` (tomt = `$USER`/`$USERNAME`) och `word_comments` ligger
i `Settings`; spårning är per flik och sparas inte.

**Word** (`docx.rs`, `docx_review.rs`): `w:ins`/`w:moveTo` och `w:del`/`w:moveFrom` (utanför styckemarkeringens
`w:rPr`) ger taggar med nytt löpnummer och en postrad (`w:author`, `w:date`); `w:delText` läses som text.
Flyttar blir vanlig infogning och radering (ett `w:del` inuti ett `w:ins` behåller bara raderingen).
`w:commentRangeStart/End` blir markörer och `w:commentReference`-körningen försvinner (den skrivs igen vid
slutmarkören). `comments.xml` ger postrader; svar och löst läses ur `commentsExtended.xml`
(`w15:paraIdParent`, `w15:done`, via styckets `w14:paraId`). Bara kommentarer med ett område i texten tas med.
Vid skrivning: orörda stycken behåller sin ursprungliga XML (med ändringar och kommentarer), omskrivna stycken
får `w:ins`/`w:del` (`w:id` från 5 000 000, författare och datum ur posterna) och
`w:commentRangeStart`/`End` + referens. `comments.xml` följer med orörd om alla poster är oförändrade;
annars byggs den av posterna (oförändrade kommentarer ordagrant, nya med `w14:paraId`) och
`commentsExtended.xml` skrivs om (eller skapas, med relation och innehållstyp). Kommentarer utan område i
brödtexten tas bort. Spårade formateringsändringar (`rPrChange`, `pPrChange`) tas fortfarande bort när ett
stycke skrivs om och rapporteras som "spårade formateringsändringar".

**ODT**: `text:tracked-changes` godkänns vid läsning (infogad text stannar, raderad text kommer inte med),
`office:annotation` tas bort som förut, och `odt::write` godkänner ändringarna och utelämnar kommentarerna.
Spårade ändringar och kommentarer i .odt är alltså inte stödda fullt ut.

## Avsnitt: liggande sidor och spalter

Som Words sektioner. En del av dokumentet kan ha egen sidstorlek (liggande), egna marginaler och flera
spalter. Logiken ligger i `word/sections.rs` (modell, rad, redigering), `word/secpages.rs` (sidbrytning med
spalter) och `word/sectionui.rs` (menyn och dialogen "Spalter"); dessutom `docx.rs`, `odt.rs` och `pdf_write.rs`.

### Formatet

Ett avsnitt börjar vid en egen rad i Markdown-dialekten (en HTML-kommentar, som grundformatraden):

```
<!-- kladd: avsnitt; page: A4 landscape; page-margin: 20mm; columns: 2; column-gap: 12mm; type: continuous -->
<!-- kladd: spaltbrytning -->
```

- `page` (även `page-size`) följer `page-size` i grundformatet (`A4`, `Letter`, `210mm 297mm`, `landscape` efter
  namnet), `page-margin` (även `margin`) följer CSS, `columns` är 1–6, `column-gap` en längd (standard 35,4 pt
  = 1,25 cm, Words 708 twips) och `type: continuous` låter avsnittet börja på samma sida (annars en ny sida).
- **Egenskaper som saknas ärver dokumentets grundformat**, inte föregående avsnitt. Ett avsnitt utan egenskaper
  är alltså "tillbaka till grundformatet".
- En avsnittsrad som står före all text (bara tomma rader och grundformatraden före) gäller **första avsnittet**
  och börjar ingen ny sida. Så får hela dokumentet spalter eller liggande sidor.
- Raden räknas som markup: dold utom när markören står på den (som `<!-- kladd: toc -->`), ingen ordräkning,
  ingen stavningskontroll. `DocDefaults::from_comment` ger `None` för den (den är aldrig grundformat). Rader i
  kodstaket räknas inte.
- `sections::scan(text, &defaults) -> Vec<Section>` ger avsnitten (alltid minst ett) med egenskaperna
  uträknade; `Analysis::sections` har dem. `SectionProps::{from_comment, to_comment}` läser och skriver raden.
  Redigering: `insert_break`, `insert_column_break`, `update_current`, `set_landscape_from`.

### Gränssnittet

*Infoga* i formateringsfältet: *Avsnittsbrytning (ny sida)* (det nya avsnittet börjar som det nuvarande),
*Liggande sidor härifrån* / *Stående sidor härifrån* (ändrar avsnittet om inget står före markören i det,
annars börjar ett nytt avsnitt där), *Spalter…* (dialog: 1–3 spalter och mellanrum i mm; gäller avsnittet
som markören står i och skriver om dess rad, eller lägger en rad före all text i första avsnittet) och
*Spaltbrytning*.

### Utläggning och sidbrytning

Galleriet är fortfarande ett enda `Galley` för TextEdit. `layout::relayout` lägger ut varje stycke med
**spaltens bredd** i dess avsnitt (`Geom::wrap_for`; bredden är med i styckecachens nyckel) och samlar per rad
avsnittet (`row_sec`) och spaltbrytningarna. `secpages::paginate_sections` flyttar sedan raderna i x och y:

- en spalt fylls uppifrån och ned, sedan nästa spalt, sedan ett nytt ark (samma regler som `paginate`: rader
  delas aldrig, en rad högre än spalten står ensam, `groups` flyttas ihop, flytande bilder räknas som en rad);
- ett nytt avsnitt börjar på ett nytt ark med avsnittets storlek och marginaler. Ett **löpande** avsnitt
  (`type: continuous`) med samma sidstorlek och marginaler fortsätter på samma ark under det förra
  (`PageBox::regions`: en del per spaltindelning); spalterna i avsnittet före ett löpande byte **jämnas ut**
  (`balanced_height`) om det ryms på sidan;
- ark med olika storlek centreras på skrivbordet (`Geom::sheet_left`). Galleriets x = 0 ligger vid den
  vänstraste textkanten (`Geom::origin_x`), så alla rader har x ≥ 0 och TextEdits yta (`Geom::edit_w`) täcker allt;
- resultatet är `Paged { boxes: Vec<PageBox>, dx }`; `Cache::page_boxes` ger arken till vyn (ritning av ark,
  sidhuvud/sidfot per ark, sidnummer med `secpages::page_index`). Sidnumren löper över avsnitten och
  sidhuvud/sidfot är gemensamma. Ett vanligt dokument (`Geom::for_sections` ger en `Geom` utan `secs`) går
  den gamla vägen (`pages::paginate`) och får `uniform_boxes`, så ritningen är gemensam.
- Tabeller och bilder anpassas till spaltens bredd (tabellplanen och `images::display_size` får spaltbredden;
  en bild i ett smalare avsnitt kläms mot det avsnittets spalt). Tabellernas kolumnkanter (`TableGeom::col_x`)
  flyttas med radernas dx.
- Raderna i olika spalter överlappar i y, som cellerna i en tabell. `secpages::pointer_fix` sätter därför om
  markören vid klick och drag i flerspaltiga delar (`column_hit`: raden söks bland spaltens egna rader).
  Upp/Ned går rad för rad i läsordningen (egui räknar radindex), så det går över spaltgränsen utan särskild kod.
- Citatstreck, fotnotsavskiljare, flytande bilder, innehållsförteckning och bildflytt använder spaltens
  vänsterkant och bredd (`col_at` i `word_page`).

Begränsningar: en tabell som delas mellan två spalter får bara första radens x för draghandtag och träffyta;
fotnoterna hamnar sist i dokumentet (som förut).

### Word (`docx.rs`)

Läsning: ett toppnivåstycke med `w:pPr/w:sectPr` slutar ett avsnitt; stycket läses som vanligt och följs av
en avsnittsrad med **nästa** avsnitts egenskaper (sista avsnittets kommer från brödtextens `w:sectPr`). Egenskaperna
är `w:pgSz` (w, h, orient), `w:pgMar`, `w:cols` (num, space; olika breda spalter räknas som lika breda) och
`w:type` (`continuous`; övriga typer blir ny sida) och skrivs bara om de skiljer sig från grundformatet. **Grundformatet
(sida, sidhuvud, sidfot) läses ur första avsnittets `sectPr`**, inte det sista. `w:br w:type="column"` blir
`<!-- kladd: spaltbrytning -->`.

Skrivning: en avsnittsrad blir ett tomt stycke med `w:sectPr` (`SECT_PARA`), en spaltbrytning ett stycke med
`<w:br w:type="column"/>`. `fix_sections` går igenom alla `w:sectPr` i stycken i ordning och `apply_section`
sätter sida, marginaler, spalter och typ efter avsnitten i texten, och brödtextens `w:sectPr` får sista
avsnittets. **Är allt redan lika lämnas XML:en orörd**, så orörda dokument sparas byte för byte som förut (en
avsnittsrad är en enhet, `p|…`, som matchas mot originalets stycke precis som innehållsförteckningen; ett
ändrat stycke får behålla originalets `w:pPr` med dess `sectPr`). Sidhuvudets/sidfotens referenser läggs i första
avsnittet om det saknar egna (de ärvs av senare avsnitt), och ändras sidhuvudet byts de i alla avsnitt som hade egna
(`Source::first_refs`).

### OpenDocument (`odt.rs`)

Skrivning: varje avsnitt som börjar på en ny sida får en sidmall `M1`, `M2` … med eget sidformat, delar sidhuvud
och sidfot med `Standard`, och stycket efter brytningen får ett automatiskt format med `style:master-page-name`
(ett tomt bärarstycke före block som inte är stycken, och före ett flerspaltigt avsnitt). **Spalter ligger i ett
`text:section`** med `style:columns` (LibreOffice jämnar ut dem), som ett löpande byte; spaltbrytning är
`fo:break-before="column"` (`page` utan spalter). Läsning: ett stycke vars format har `style:master-page-name`
som skiljer sig från gällande sidmall ger en avsnittsrad med sidmallens mått (och `style:columns` i
sidformatet), och ett `text:section` med spalter ger en löpande rad före och en som återställer efter.
ODT sparas inte förlustfritt, så markören följer med bara så långt `render` når.

### PDF och utskrift (`pdf_write.rs`)

`Block::SectionBreak`/`ColumnBreak`; varje sida bär sina mått (`PageOps::geom`) och PDF-sidorna skapas med
rätt storlek, sidhuvud och sidfot centreras på den egna sidan. Spalter: `new_page` går till nästa spalt innan en
ny sida (`ml`/`cw` sätts om och `rt` är spaltens överkant), texten följer med (`text_block` flyttar x), fotnoterna
ritas över hela textbredden. Ett löpande avsnitt fortsätter under föregående del. Skillnad mot dokumentläget:
spalterna före ett löpande byte jämnas **inte** ut i PDF:en.
