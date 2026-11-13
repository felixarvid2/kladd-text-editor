# Kommandoregistret, menyraden och kommandopaletten

Alla appens kommandon finns i en lista, `ALL` i `src/commands.rs`. Menyraden (Arkiv · Redigera · Visa ·
Infoga · Verktyg · Hjälp, och Format i dokumentläget), hamburgermenyn i smala fönster och
kommandopaletten (`Ctrl+Skift+P`) byggs ur den. Ett nytt kommando läggs alltså på ett ställe och dyker
upp i alla tre.

| Fil | Innehåll |
|---|---|
| `src/commands.rs` | `Command`, `Category`, `Scope`, `Env`, listan `ALL` och `KladdApp::run_command` |
| `src/menubar.rs` | menyraden och hamburgermenyn (ritning, ingen logik) |
| `src/palette.rs` | kommandopaletten och fuzzy-sökningen (`score`, `rank`) |
| `src/settings_ui.rs` | fönstret Inställningar (`Ctrl+,`) |
| `src/i18n/en_menus.rs` | engelska etiketter för det som ovan lägger till |

`commands.rs` är en undermodul till `app` (`#[path = "commands.rs"] mod commands;` i `app.rs`). Därför kan
kommandona anropa appens privata funktioner och fält direkt; de andra filerna går bara via
`run_command` och `Env`.

## Lägg till ett kommando

1. Lägg en rad i `ALL` (i rätt kategori, på den plats i menyn där den ska stå):

   ```rust
   Command::new("edit.sort_lines", "Sortera rader", Edit, "Ctrl+Alt+S", |e| e.has_doc && !e.pdf, |a, c| a.sort_lines(c)),
   ```

   Argumenten är `id`, svensk etikett, kategori, kortkommando som text (`""` om inget), `enabled` och `run`.
   `enabled` är en closure över [`Env`](#env); `run` får `&mut KladdApp` och `&egui::Context`.
2. Lägg den engelska etiketten i `src/i18n/en_menus.rs` (`("Sortera rader", "Sort Lines")`). Testet
   `every_command_has_both_languages` misslyckas annars. Etiketten är nyckeln, så den måste vara tecken för
   tecken densamma. Finns etiketten redan i en annan tabell (Spara, Sök …) behövs ingen ny rad.
3. Har kommandot ett kortkommando: själva tangenten hanteras fortfarande i `handle_shortcuts` (eller
   i lägets egna `*_shortcuts`). Registret bara visar texten. Kontrollera att kombinationen är ledig
   (sök i `app.rs`, `word/` och `pdfview/`).

Tillägg på raden (byggarmetoder):

| Metod | Betydelse |
|---|---|
| `.sep()` | avdelare före raden i menyn |
| `.checked(\|e\| e.settings.wrap)` | växlare: bock när closuren ger sant |
| `.scope(Scope::Word)` / `Scope::Pdf` | bara i dokumentläget / PDF-visaren: dolt i menyerna i andra lägen, nedtonat i paletten |
| `.palette_only()` | syns inte i menyerna, bara i paletten (t.ex. inställningar som också finns i Inställningar) |

**Id:n** är `grupp.namn`, stabila och unika (testas). De sparas i `Settings.recent_commands`, så byt aldrig id på ett
befintligt kommando.

## Env

`Env` är en ögonblicksbild av appens läge (`has_doc`, `pdf`, `word`, `markdown`, `has_path`, `find_open`, `split`,
`tracking`, `changes` och `settings`) som `Env::of(&app)` bygger en gång per bild. Behöver `enabled` eller `checked`
något nytt: lägg ett fält i `Env`, fyll det i `Env::of` och i `Env::empty` (som testerna använder). Att closures får
`Env` och inte hela `KladdApp` gör att logiken kan enhetstestas utan att starta ett fönster.

## Menyerna

* Kategorierna är `Category::ALL` i den ordningen. En kategori utan synliga kommandon i aktuellt läge visas inte
  (därför syns Format bara i dokumentläget).
* Inom en kategori följer menyn listans ordning; `.sep()` ger avdelare.
* Menyraden ligger bredvid flikarna när det finns plats (`MIN_TABS_W` i `menubar.rs` är minsta bredden för
  flikarna); annars visas hamburgerknappen med kategorierna som undermenyer.
* Inställningar som växlades i huvudmenyn ligger nu i fönstret Inställningar (`settings_ui.rs`). Ny inställning:
  lägg fältet i `Settings` (med `#[serde(default)]` via strukturens attribut) och en rad i rätt sektion i
  `settings_ui.rs`; vill du ha den i paletten också, lägg ett `palette_only()`-kommando.

## Kommandopaletten

`Ctrl+Skift+P` öppnar den; pilar och Enter väljer, Esc stänger. `palette::rank` söker i etiketten på båda språken
(så "find in files" hittar *Sök i filer…* i det svenska gränssnittet), viker å/ä/ö till a/o, och ger poäng för
ordstarter, rader av följande tecken och träffar i början. Senast använda kommandon (högst åtta, `Settings.recent_commands`)
ligger överst när fältet är tomt och får en bonus i en sökning. Kommandon som inte går att köra nu visas nedtonade
och sjunker i listan; Enter kör dem inte.

## Test

`cargo test` kör bland annat `commands::tests` (unika id:n, etikett på båda språken, `enabled` i olika lägen, att
menyerna döljer andra lägens kommandon) och `palette::tests` (fuzzy-rangordning, tvåspråkig träff, senast använda,
nedtonade kommandon). Skärmtest: starta Kladd, kontrollera menyraden i ett brett och ett smalt fönster (ca 700 px),
öppna paletten och Inställningar med båda språken.
