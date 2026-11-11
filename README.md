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
- **Öppna och spara** `.txt`, `.md`, Word-dokument (`.docx`) och många kodformat.
- **Syntaxfärgning** och radnummer.
- **Sök och ersätt** med `Ctrl+F` och `Ctrl+H`.
- **Markdownförhandsvisning** med `Ctrl+Shift+M`.
- **Två teman** – *Gryning* och *Skymning*.
- **Teckenkodning** – UTF-8, UTF-16 och Windows-1252, med bevarade radslut.
- **Dra och släpp** filer direkt i fönstret.
- **Ett fönster** – filer som öppnas från filhanteraren eller terminalen läggs som nya flikar i det Kladd-fönster som redan är öppet. Använd `kladd --new-window` för ett separat fönster.

## Kom igång

Kladd är ett öppet källkodsprojekt och har ännu inget färdigt releasepaket. Bygg det från källkoden:

```bash
cargo build --release
./target/release/kladd
```

Har du redan ett färdigt `.deb`-paket installerar du det med:

```bash
sudo apt install ./kladd_*.deb
```

<details>
<summary>Bygg ett .deb-paket</summary>

```bash
cargo install cargo-deb --locked   # en gång
cargo deb                          # skapar paketet i target/debian/
```

Avinstallera med `sudo apt remove kladd`.

</details>

## Word-dokument

Kladd öppnar `.docx` och visar dokumentet som Markdown, så att formateringen syns i texten och i förhandsvisningen. När du sparar som `.docx` blir det ett riktigt Word-dokument som öppnas i Word, LibreOffice och Google Dokument. Med *Spara som* kan vilken flik som helst sparas som `.docx`.

| I Word | I Kladd |
|---|---|
| Rubriker och titel | `#` … `######` |
| Fetstil, kursiv, genomstruken | `**fet**`, `*kursiv*`, `~~struken~~` |
| Punktlistor och numrerade listor | `-` och `1.` |
| Citat | `>` |
| Tabeller | `\| kolumn \| kolumn \|` |
| Länkar | `[text](https://…)` |
| Kod | `` `kod` `` och kodblock |

Bilder, kommentarer, fotnoter, sidhuvuden och spårade ändringar visas inte. Kladd säger till när ett dokument innehåller sådant och frågar innan originalet skrivs över. Lösenordsskyddade filer och det gamla `.doc`-formatet stöds inte.

Läsning och skrivning bygger på samma teknik som [WordCraft](https://github.com/storytold/wordcraft) (MIT/Apache-2.0).

## Kortkommandon

| Kommando | Funktion |
|---|---|
| `Ctrl+T` / `Ctrl+N` | Ny flik |
| `Ctrl+O` | Öppna fil |
| `Ctrl+S` | Spara |
| `Ctrl+Shift+S` | Spara som |
| `Ctrl+W` | Stäng flik |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Nästa / föregående flik |
| `Ctrl+F` / `Ctrl+H` | Sök / ersätt |
| `F3` / `Shift+F3` | Nästa / föregående träff |
| `Esc` | Stäng sökning |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Textstorlek |
| `Alt+Z` | Radbrytning på/av |
| `Ctrl+Shift+M` | Markdownförhandsvisning |
| `Ctrl+Shift+L` | Byt tema |
| `Ctrl+Q` | Avsluta |

## Licens

Kladd är licensierat under MIT, se [LICENSE](LICENSE). Typsnitten IBM Plex Sans och IBM Plex Mono används under SIL Open Font License.
