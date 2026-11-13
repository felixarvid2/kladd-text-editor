# Översättning av gränssnittet (svenska och engelska)

Kladd visar gränssnittet på svenska eller engelska. Valet görs i huvudmenyn (*Språk / Language*:
Automatiskt, Svenska, English) och sparas i `Settings.lang`. Automatiskt följer systemets språk
(svenska för `sv*`, annars engelska). Bytet gäller från nästa bild, utan omstart.

Systemet ligger i `src/i18n.rs` och är gettext-stil: **källtexten i koden är den svenska texten**, och
den engelska finns i tabeller under `src/i18n/`. Saknas en text i tabellen visas svenskan, så en
halvt konverterad fil fungerar alltid.

## API

```rust
use crate::i18n::t;   // t("Spara") -> &'static str
use crate::tf;        // tf!("Sparade {namn}", namn = x) -> String
```

- `t(sv: &'static str) -> &'static str` ger texten på aktuellt språk. Argumentet måste vara en
  `&'static str` (literal eller `const`).
- `tf!("mall {a} {b}", a = x, b = y)` översätter mallen och byter ut `{a}`, `{b}`. Bara **namngivna**
  platshållare, och argumenten anges alltid (`name = name`, ingen inbäddad `{name}`-fångst som i
  `format!`). Värdena formateras med `Display`. Formatspecifikationer (`{x:.1}`, `{n:02}`) stöds inte:
  formatera först (`i18n::num(x, 1)`, `format!("{n:02}")`) och skicka strängen. Klammrar skrivs `{{` / `}}`.
  Mallen måste vara en literal.
- Språk och datum/tal: `i18n::lang()`, `Lang::{Sv, En}`, `i18n::num(v, decimaler)` (komma/punkt),
  `i18n::clock(h, m)` (24 h / 12 h med AM/PM), `i18n::today_at(h, m)` ("i dag 13:05" / "today 1:05 PM"),
  `i18n::date(y, m, d)`. Använd dem där koden i dag skriver datum, klockslag eller decimalkomma
  för hand (t.ex. `recovery.rs`, `pdfview/chrome.rs`, `word/view.rs`).
- Listor "a, b och c": `doc::join_sv` skriver redan "and" på engelska.
- Kortkommandon i texter (`"Ny flik (Ctrl+T)"`) lämnas som de är i både svenska och engelska
  texten; `crate::keys::shortcut` gör om dem till ⌘ på Mac. Skriv alltid `Ctrl+`, `Shift+`, `Alt+`
  på engelska (`Ctrl+klick` blir `Ctrl+click`).

## Tabellerna

Varje tabell är `pub const EN: &[(&str, &str)] = &[("svenska", "english"), …];` och slås ihop vid
start. Använd rätt fil så att flera agenter inte krockar:

| Fil | Område |
|---|---|
| `en_app.rs` | `app.rs`, `main.rs`, `doc.rs`-titlar, gemensamma ord (Avbryt, Spara …) |
| `en_word.rs` | `word/*` (dokumentläget, formatmallar, navigering, granskning), `style*.rs`, `search.rs`, `review.rs` |
| `en_pdf.rs` | `pdfview/*`, `ocr.rs` |
| `en_doc.rs` | `docx*.rs`, `odt.rs`, `pdf_read.rs`, `pdf_write.rs`, `export.rs`, `paste.rs`, `preview.rs` (felmeddelanden och notiser) |
| `en_misc.rs` | `spell`, `grammar`, `grammar_ui`, `update`, `workspace`, `find_files`, `history*`, `print`, `recovery`, `autosave`, `panes`, `multicursor`, `single` … |

Samma svenska text får finnas i flera tabeller bara om översättningen är **identisk**; enhetstestet
`i18n::tests::tables_are_consistent` misslyckas annars. Testet kontrollerar också att varje
`{platshållare}` i en svensk nyckel finns i översättningen och tvärtom (och att ingen översättning är tom).

Nyckeln måste vara tecken för tecken samma som i koden, efter att `\`-radslut i literaler har
tolkats. Håll nycklarna korta: en mening per nyckel, inte hela stycken, utom när meningen är en enhet.

## Så konverterar du en fil

1. Lägg `use crate::i18n::t;` (och `use crate::tf;` om du behöver mallar) överst.
2. Gå igenom filen och hitta text som en användare ser. Sök med
   `grep -n '"[^"]*[A-Za-zåäöÅÄÖ]\{2\}' fil.rs`; kom ihåg svenska ord utan å, ä, ö.
3. Konvertera enligt mönstren nedan, och lägg varje nyckel i rätt `en_*.rs`.
4. `cargo build` och `cargo test`. Starta Kladd med *Språk / Language* = English och titta på ytorna.

Tips: dumpa alla `t("…")` och `tf!("…"` ur filen med `grep -o` och kontrollera att varje nyckel finns i tabellen
(en nyckel som saknas visas bara på svenska, så felet syns inte i bygget).

### Mönster

**Literal som visas direkt** – slå in i `t`:

```rust
ui.button("Avbryt")                    // före
ui.button(t("Avbryt"))                 // efter
ui.label(RichText::new(t("Lösenord")).color(c))
response.on_hover_text(t("Skilj på stora och små bokstäver"));
TextEdit::singleline(&mut s).hint_text(t("Sök"))
menu_item(ui, p, t("Spara som…"), "Ctrl+Shift+S", None, true)   // kortkommandot översätts inte
self.notify(ctx, t("Sparat automatiskt"), false);
rfd::FileDialog::new().set_title(t("Öppna fil")).add_filter(t("Bilder"), EXT)
```

Tillägg, filändelser och egennamn som inte ska översättas lämnas som literaler (`"PDF"`, `"Markdown"`,
`"Kladd"`, `"docx"`).

**`format!` med text** – `tf!` med namngivna argument:

```rust
format!("Sparade {title}")                         // före
tf!("Sparade {title}", title = title)              // efter
format!("{} av {}", i + 1, n)                      // före
tf!("{i} av {n}", i = i + 1, n = n)                // efter (ge platshållarna meningsfulla namn)
format!("{x:.1} MB")                               // före
tf!("{x} MB", x = crate::i18n::num(x, 1))          // efter
```

Är `format!` bara sammansättning utan ord (`format!("{}.{ext}", stem)`, URI:er, sökvägar) rörs det inte.

**Plural** – två nycklar, aldrig "träff(ar)":

```rust
if n == 1 { t("1 träff").to_owned() } else { tf!("{n} träffar", n = n) }
```

Engelska plural-former skrivs ut i respektive nyckel ("1 match", "{n} matches").

**Meningar som byggs av delar** – bygg om till hela meningar. `format!("{a} {b}")` där `b` är ett
översatt ord ger fel ordföljd på engelska. Gör i stället två hela nycklar, eller en mall med
platshållare för det som varierar (se `missing_images` i `app.rs`: `"{done}, men hittade inte {what} {list}"`).

**`const`/`static` med texter** – låt konstanten vara svensk och översätt vid visning:

```rust
const COLUMNS: &[(&str, Kind)] = &[("Namn", Kind::Name), ("Storlek", Kind::Size)];
for (label, kind) in COLUMNS {
    ui.label(t(label));          // label: &&'static str -> t(*label) om kompilatorn kräver det
}
```

Det fungerar eftersom `t` tar `&'static str`. Slå **inte** in literalerna i konstanten med `t(...)`
(konstanter evalueras en gång och språket kan bytas).

**Arrayer och combo-listor** – samma sak: lagra svenska, översätt där raden ritas. Är värdet också
det som sparas eller jämförs (inställning, filtyp) lagrar du en enum eller ett id och översätter bara
etiketten:

```rust
for (id, label) in [("a4", "Stående"), ("a4l", "Liggande")] {
    ui.selectable_value(&mut cur, id, t(label));   // id sparas, label visas
}
```

**Funktioner som returnerar text** (`fn label(&self) -> &'static str`) – låt dem returnera
`t("…")` (anropet sker vid visning, så språket är aktuellt), eller svensk literal och översätt hos
anroparen. Blir returtypen `String` räcker `t(…).to_owned()`.

**Felmeddelanden från andra moduler** (`Err(String)` som visas i en notis) – översätt där de skapas
(`tf!("Kunde inte läsa {name}: {e}", …)`), inte i `notify`. Bäddas ett systemfel in (`{e}`) översätts
bara ramtexten.

**Text som jämförs i logiken** – översätt aldrig en sträng som används som nyckel, id, filändelse,
formatnamn, `match`-arm eller jämförs med `==`. Exempel: `doc.language() == "OpenDocument"` i `app.rs`
jämför ett internt namn och lämnas orört. Är det en synlig etikett som också jämförs: jämför mot id:t,
inte mot den översatta texten. Använd aldrig `t()` i `Id::new`/`id_salt` (egui-id:n ska vara stabila
när språket byts).

### Översätt inte

- Det Kladd skriver i filer: docx/odt/pdf-metadata och stilnamn, `<!-- kladd: … -->`-dialektens
  nyckelord (`sida`, `{sida}` m.fl. fält), Markdown, exporterad HTML/EPUB. Filernas innehåll ska vara
  detsamma oavsett gränssnittsspråk.
- Teststrängar och `assert!`-text, loggtext som inte visas för användaren (`eprintln!` för felsökning,
  `expect("…")`), kommentarer.
- Kortkommandotecken (`Ctrl+`, ⌘ hanteras av `crate::keys`).
- Egennamn: Kladd, Markdown, PDF, Word, OpenDocument, LanguageTool.

### Engelsk stil

Korta, naturliga UI-texter i stil med VS Code / Word / Firefox: *Save*, *Save As…*, *Save a Copy…*,
*Find in Files*, *Track Changes*, *Page Setup…*. Menyer och knappar i Title Case, meningar och
tooltips med bara första ordet versalt och punkt endast om svenskan har punkt. Behåll `…` där
svenskan har det. Skriv "color", "recognize" (amerikansk stavning).

## Test

- `cargo test` kör `i18n::tests` (dubbletter, platshållare).
- Skärmtest: starta Kladd, välj *Språk / Language* i huvudmenyn (längst ned, menyn rullar) och
  kontrollera menyer, dialoger, tooltips och statusrad på båda språken. Byt tillbaka till
  Svenska och se att inget har ändrats.
