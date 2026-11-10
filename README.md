# Kladd

Kladd är en enkel och snygg texteditor för Ubuntu. Den är snabb, ren och gjord för att skriva anteckningar, Markdown och kod utan krångel.

![Kladd i mörkt tema med Markdown-förhandsvisning](docs/markdown-morkt.png)

![Kladd i ljust tema med sök och ersätt](docs/sok-ljust.png)

## Funktioner

- Flikar – öppna flera filer samtidigt, `Ctrl+T` skapar en ny flik
- Öppna och spara `.txt`, `.md` och många kodformat
- Syntaxfärgning och radnummer
- Sök och ersätt
- Förhandsvisning av Markdown
- Ljust och mörkt tema: **Gryning** och **Skymning**
- Automatisk teckenkodning (UTF-8, UTF-16, Windows-1252); radslut (CRLF/LF) bevaras
- Dra och släpp filer i fönstret
- Ett enda fönster: filer du öppnar från filhanteraren eller terminalen läggs till som nya flikar i det Kladd-fönster som redan är öppet (`kladd --new-window` öppnar ett separat fönster)

## Kortkommandon

| Kommando | Funktion |
|---|---|
| `Ctrl+T` / `Ctrl+N` | Ny flik |
| `Ctrl+O` | Öppna fil |
| `Ctrl+S` | Spara |
| `Ctrl+Shift+S` | Spara som |
| `Ctrl+W` | Stäng flik |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Nästa / föregående flik |
| `Ctrl+F` | Sök |
| `Ctrl+H` | Ersätt |
| `F3` / `Shift+F3` | Nästa / föregående träff |
| `Esc` | Stäng sökning |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | Större / mindre / standardstorlek på text |
| `Alt+Z` | Radbrytning på/av |
| `Ctrl+Shift+M` | Förhandsvisning av Markdown |
| `Ctrl+Shift+L` | Byt tema |
| `Ctrl+Q` | Avsluta |

## Installation

```sh
sudo apt install ./kladd_1.0.2-1_amd64.deb
```

## Avinstallation

```sh
sudo apt remove kladd
```

## Bygga från källkod

Kräver [Rust](https://rustup.rs/).

```sh
cargo build --release
cargo install cargo-deb --locked   # en gång
cargo deb                          # skapar .deb i target/debian/
```

## Licens och tack

Kladd är licensierat under MIT, se [LICENSE](LICENSE).

Typsnitten är IBM Plex Sans och IBM Plex Mono under SIL Open Font License.
