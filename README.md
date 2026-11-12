<p align="center">
  <img src="packaging/icons/128/kladd.png" width="96" alt="Kladd">
</p>

<h1 align="center">Kladd</h1>

<p align="center">
  <strong>En lugn och snygg texteditor för Linux.</strong><br>
  För anteckningar, Markdown och kod – utan krångel.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/plattform-Linux-4c6ef5" alt="Linux">
  <img src="https://img.shields.io/badge/licens-MIT-2f9e44" alt="MIT">
</p>

Kladd är en snabb och ren texteditor med flikar, syntaxfärgning, sök och ersätt och en förhandsvisning av Markdown. Den finns i ett mörkt och ett ljust tema och är gjord för att kännas lika hemma i en anteckningsfil som i ett kodprojekt.

| Mörkt tema | Ljust tema |
|---|---|
| ![Kladd i mörkt tema med Markdown-förhandsvisning](docs/markdown-morkt.png) | ![Kladd i ljust tema med sök och ersätt](docs/sok-ljust.png) |

## Funktioner

- **Flikar** – öppna flera filer samtidigt. `Ctrl+T` skapar en ny flik.
- **Säkerhetskopia och återställning** – osparade flikar sparas undan var 30:e sekund; efter en krasch erbjuder Kladd att återställa dem vid nästa start. Kan stängas av i huvudmenyn.
- **Öppna och spara** `.txt`, `.md`, Word-dokument (`.docx`), OpenDocument-text (`.odt`), PDF och många kodformat.
- **Utskrift** med `Ctrl+P` till CUPS-skrivare (kopior, sidintervall, dubbelsidigt) eller som PDF. PDF-filen följer dokumentets sidstorlek och marginaler och har sidnummer i sidhuvud och sidfot.
- **Klistra in formaterad text** från Word, LibreOffice, Google Dokument och webbsidor. Ctrl+Shift+V klistrar in utan formatering.
- **Förlustfri Word-sparning** – Word-filer sparas med alla sina delar kvar, se [Word-dokument](#word-dokument).
- **Syntaxfärgning** och radnummer.
- **Sök och ersätt** med `Ctrl+F` och `Ctrl+H`, med alternativ för versaler, hela ord och reguljära uttryck (`$1` och `${namn}` fungerar i ersättningen). I dokumentläget söker Kladd i den synliga texten, så "fet text" hittar `**fet** text`. Du kan också söka på formatering (fetstil, kursiv, färg, markering, rubrik) och låta *Ersätt* lägga på eller ta bort formatering på träffarna.
- **Markdownförhandsvisning** med `Ctrl+Shift+M`.
- **Två teman** – *Gryning* och *Skymning*.
- **Teckenkodning** – UTF-8, UTF-16 och Windows-1252, med bevarade radslut.
- **Dra och släpp** filer direkt i fönstret.
- **Ett fönster** – filer som öppnas från filhanteraren eller terminalen läggs som nya flikar i det Kladd-fönster som redan är öppet. Använd `kladd --new-window` för ett separat fönster.
- **Dokumentläge** – Word- och OpenDocument-dokument visas som ett dokument med riktiga sidor, formateringsfält och sidnummer. Se [Word-dokument](#word-dokument).
- **Stavningskontroll** i dokumentläget, med svenska och engelska ordlistor från systemet.

## Kom igång

Ladda ner det senaste `.deb`-paketet från [Releases](https://github.com/felixarvid2/kladd-text-editor/releases/latest) och installera det med:

```bash
sudo apt install ./kladd_*.deb
```

Nyheter i varje version står i [CHANGELOG.md](CHANGELOG.md).

Du kan också bygga Kladd från källkoden:

```bash
cargo build --release
./target/release/kladd
```

<details>
<summary>Bygg ett .deb-paket</summary>

```bash
cargo install cargo-deb --locked   # en gång
cargo deb                          # skapar paketet i target/debian/
```

Avinstallera med `sudo apt remove kladd`.

En ny release skapas automatiskt när en versionstagg pushas. Höj först `version` i `Cargo.toml`, och tagga sedan samma version:

```bash
git tag v1.4.0
git push origin v1.4.0
```

Det går också att köra workflowen *Release* för hand under Actions och kryssa i *Publicera release*. Då skapas taggen från versionen i `Cargo.toml`.

Release-anteckningarna hämtas från avsnittet för versionen i `CHANGELOG.md`; finns inget avsnitt skapas anteckningarna automatiskt. Skriv anteckningarna i `CHANGELOG.md` innan du taggar. För en redan publicerad release: kör workflowen för hand, fyll i *update_notes* med taggen (till exempel `v1.5.0`) och lämna *Publicera release* ikryssad av. Då byts bara anteckningarna ut.

</details>

## Word-dokument

Kladd öppnar `.docx` och `.odt` (LibreOffice) i dokumentläge, som liknar ett Word-dokument. När du sparar som `.docx` blir det ett riktigt Word-dokument som öppnas i Word, LibreOffice och Google Dokument. `.odt` sparas som OpenDocument-text. Med *Spara som* kan vilken flik som helst sparas som `.docx`.

### Dokumentläge

![Ett Word-dokument i Kladds dokumentläge](docs/dokumentlage-ljust.png)

Ett öppnat `.docx` visas på vita sidor mot grå bakgrund, med sidstorlek och marginaler från dokumentet och typografi som liknar Word. Dokumentets typsnitt, storlekar, färger, markeringar och styckejustering (vänster, centrerat, höger och marginaljustering) visas som i Word. Typsnitten hämtas från systemet precis som i LibreOffice, så Arial visas med Liberation Sans och Times New Roman med Liberation Serif. Calibri visas med Carlito, som har samma mått. Listor dras in med hängande indrag, tabeller ritas som rutnät och bilder visas i texten. Markdown-koden är dold; den visas nedtonad bara på raden där markören står.

Formateringsfältet ovanför texten har stilval (Brödtext, Rubrik 1–3, Citat), typsnitt och storlek, textfärg och markeringsfärg, vänster-, center-, höger- och marginaljustering, knapparna F, K, U och S för fet, kursiv, understruken och genomstruken, punktlista, numrerad lista, citat, infoga bild och tabell, ångra och gör om samt zoom. Statusfältet visar "Sida x av y", antal ord, språk för stavningskontrollen och zoomnivå.

### Förlustfri sparning

Kladd behåller alla delar av originalfilen, som sidhuvud, sidfot, fotnoter, kommentarer, teman och inställningar. Vid sparning skrivs bara de stycken om som du har ändrat; oförändrade stycken sparas exakt som de var. Objekt som Kladd inte kan visa, till exempel fotnotsreferenser, kommentarer, fält och textrutor, blir osynliga markörer `<!--k:N-->` i Markdown-texten och skrivs tillbaka när du sparar. Det enda som kan försvinna är spårade ändringar i stycken som du ändrar.

### Sidor och utskrift

Knappen *Sidinställningar* (eller dubbelklick på sidhuvud eller sidfot) öppnar en dialog för sidstorlek (A4, A5, A3, Letter, Legal), stående eller liggande, marginaler, sidhuvud och sidfot. I sidhuvud och sidfot betyder `{sida}` och `{sidor}` sidnummer och antal sidor. Inställningarna sparas i grundformatskommentaren:

```
<!-- kladd: page-size: A4; page-margin: 25mm; footer: "Sida {sida} av {sidor}"; footer-align: center -->
```

### Tabeller, bilder och inklistring

Tabeller redigeras direkt i rutnätet. Tab och Shift+Tab flyttar mellan cellerna, och Tab i sista cellen lägger till en rad. När markören står i en tabell visar formateringsfältet knappar för att lägga till och ta bort rader och kolumner eller hela tabellen. Samma funktioner finns i högerklicksmenyn.

Klicka på en bild för att markera den. Dra i hörnen för att ändra storlek (håll Skift för fri proportion). En liten verktygsrad vid bilden justerar läget (vänster, centrerat eller höger), återställer originalstorleken eller tar bort bilden. Delete tar bort en markerad bild.

Ctrl+V klistrar in formaterad text med rubriker, fetstil, kursiv, understrykning, färger, typsnitt, storlekar, justering, listor, tabeller, länkar och inbäddade bilder. Ctrl+Shift+V klistrar in oformaterad text.

Vill du se dokumentet som vanlig Markdown-text klickar du på *Markdown* i formateringsfältet. Klicka på *Visa som dokument* i statusfältet för att gå tillbaka.

### Stavningskontroll

I dokumentläget markeras felstavade ord med ett rött vågigt understreck medan du skriver. Högerklicka på ett ord för att se förslag, välja *Ignorera alla* eller *Lägg till i ordlistan*. Tillagda ord sparas i `~/.config/kladd/ordlista.txt`.

Kladd använder de Hunspell-ordlistor som finns i `/usr/share/hunspell`, samma som LibreOffice använder på Ubuntu. Installera svenska med `sudo apt install hunspell-sv`; .deb-paketet föreslår `hunspell-sv` och `hunspell-en-us`. Språk och på/av väljer du genom att klicka på språket i statusfältet. Standardspråket är svenska när det är installerat.

| I Word | I Kladd |
|---|---|
| Rubriker och titel | `#` … `######` |
| Fetstil, kursiv, understruken, genomstruken | `**fet**`, `*kursiv*`, `<u>understruken</u>`, `~~struken~~` |
| Typsnitt, storlek, textfärg, markering | `<span style="font-family: Arial; font-size: 14pt; color: #C00000; background-color: #FFFF00">…</span>` |
| Centrerat, högerjusterat, marginaljusterat | `<div align="center">` … `</div>` |
| Dokumentets grundtypsnitt | `<!-- kladd: font-family: "Times New Roman"; font-size: 12pt -->` på första raden |
| Sidstorlek, marginaler, sidhuvud och sidfot | Samma kommentar: `page-size: A4; page-margin: 25mm; footer: "Sida {sida} av {sidor}"` |
| Fotnoter, kommentarer, fält, textrutor m.m. | `<!--k:N-->` (osynlig markör, skrivs tillbaka vid sparning) |
| Punktlistor och numrerade listor | `-` och `1.` |
| Citat | `>` |
| Tabeller | `\| kolumn \| kolumn \|` |
| Länkar | `[text](https://…)` |
| Kod | `` `kod` `` och kodblock |
| Bilder (PNG, JPEG, GIF, BMP) | `![text](bild:image1.png)` |
| Bildstorlek | `<img src="…" alt="…" width="240">` |

Bilder i dokumentet följer med när du sparar och syns i förhandsvisningen. Lägg till egna bilder med *Infoga bild…* (`Ctrl+Shift+I`), genom att dra in en bildfil i fönstret eller genom att skriva `![text](sökväg/till/bild.png)`. Sökvägen kan vara relativ till dokumentet. När dokumentet sparas som `.docx` bäddas bilderna in. Sparar du ett Word-dokument som Markdown hamnar bilderna i en mapp bredvid filen.

Diagram, former, textrutor, kommentarer, fotnoter och sidhuvuden visas inte i texten, men de bevaras när filen sparas (se [Förlustfri sparning](#förlustfri-sparning)). Kladd frågar innan originalet skrivs över. Lösenordsskyddade filer och det gamla `.doc`-formatet stöds inte.

Läsning och skrivning bygger på samma teknik som [WordCraft](https://github.com/storytold/wordcraft) (MIT/Apache-2.0).

## PDF

Kladd läser texten i en PDF och visar den som Markdown: rubriker, stycken, punkt- och numrerade listor, tabeller, fetstil, kursiv och bilder. Läsordningen följer spalterna, avstavning vid radslut tas bort, sidnummer i sidhuvud och sidfot hoppas över och stycken som fortsätter på nästa sida fogas ihop. Lösenordsskyddade PDF:er öppnas efter att du skrivit lösenordet.

Spara som `.pdf` för att få en A4-PDF med inbäddade typsnitt (IBM Plex), sökbar text, klickbara länkar, bokmärken från rubrikerna, bilder och sidnummer.

En PDF är byggd för utskrift, inte för redigering. När du sparar en öppnad PDF skapar Kladd därför en ny PDF av texten, och originalets sidlayout och typsnitt ersätts. Kladd frågar innan originalet skrivs över. Sidor som bara består av bilder (skannade sidor) har ingen text att läsa; Kladd säger till när det gäller.

Läsningen bygger på samma metoder som [PdfCraft](https://github.com/storytold/pdfcraft) och skrivningen på samma upplägg som [WordCraft](https://github.com/storytold/wordcraft) (båda MIT/Apache-2.0), med [lopdf](https://crates.io/crates/lopdf) och [krilla](https://crates.io/crates/krilla).

## Kortkommandon

| Kommando | Funktion |
|---|---|
| `Ctrl+T` / `Ctrl+N` | Ny flik |
| `Ctrl+O` | Öppna fil |
| `Ctrl+S` | Spara |
| `Ctrl+Shift+S` | Spara som |
| `Ctrl+P` | Skriv ut (alla flikar) |
| `Ctrl+Shift+V` | Klistra in oformaterat (endast dokumentläge) |
| `Tab` / `Shift+Tab` | Nästa / föregående cell i tabell (endast dokumentläge) |
| `Delete` | Ta bort markerad bild (endast dokumentläge) |
| `Ctrl+W` | Stäng flik |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Nästa / föregående flik |
| `Ctrl+Shift+I` | Infoga bild |
| `Ctrl+F` / `Ctrl+H` | Sök / ersätt |
| `F3` / `Shift+F3` | Nästa / föregående träff |
| `Esc` | Stäng sökning |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Textstorlek (i dokumentläge: zoom av sidan) |
| `Ctrl+Scroll` | Zoom (endast dokumentläge) |
| `Ctrl+B` / `Ctrl+I` / `Ctrl+U` | Fet / kursiv / understruken (endast dokumentläge) |
| `Ctrl+Alt+1` / `Ctrl+Alt+2` / `Ctrl+Alt+3` | Rubrik 1 / 2 / 3 (endast dokumentläge) |
| `Ctrl+Alt+0` | Brödtext (endast dokumentläge) |
| `Ctrl+L` / `Ctrl+E` / `Ctrl+R` / `Ctrl+J` | Vänster-, center-, höger- och marginaljustera (endast dokumentläge) |
| `Ctrl+Shift+>` / `Ctrl+Shift+<` (eller `Ctrl+]` / `Ctrl+[`) | Större / mindre text, ett steg (endast dokumentläge) |
| `Alt+Z` | Radbrytning på/av |
| `Ctrl+Shift+M` | Markdownförhandsvisning |
| `Ctrl+Shift+L` | Byt tema |
| `Ctrl+Q` | Avsluta |

## Licens

Kladd är licensierat under MIT, se [LICENSE](LICENSE). Typsnitten IBM Plex Sans och IBM Plex Mono används under SIL Open Font License.

Typsnittet Carlito (metriskt lika Calibri) används för dokumentläget under SIL Open Font License.
