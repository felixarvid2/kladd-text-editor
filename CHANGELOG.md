# Ändringslogg

Här står vad som är nytt i varje version av Kladd, med den senaste överst.

## [Ej släppt]

## [1.6.0] – 2026-11-12

### Nytt

- **Förlustfri Word-sparning.** Sidhuvud, sidfot, fotnoter, kommentarer, teman och inställningar följer med när du sparar ett `.docx`. Bara de stycken du har ändrat skrivs om.
- **OpenDocument-text (`.odt`)** kan öppnas och sparas.
- **Riktiga sidor.** Dokumentet visas på sidor med sidstorlek och marginaler från filen, och sidhuvud och sidfot med sidnummer. *Sidinställningar* ändrar storlek (A4, A5, A3, Letter, Legal), stående eller liggande, marginaler, sidhuvud och sidfot.
- **Tabeller som rutnät.** Redigera direkt i cellerna, flytta med Tab och Shift+Tab och lägg till eller ta bort rader och kolumner.
- **Bilder.** Klicka på en bild för att markera den, dra i hörnen för att ändra storlek och justera läget med verktygsraden vid bilden.
- **Klistra in formaterad text** från Word, LibreOffice, Google Dokument och webbsidor. Ctrl+Shift+V klistrar in utan formatering.
- **Utskrift med `Ctrl+P`** till CUPS-skrivare (kopior, sidintervall, dubbelsidigt) eller som PDF.

### Ändrat

- PDF-filer följer dokumentets sidstorlek och marginaler och får sidnummer i sidhuvud och sidfot.
- Smalare formateringsfält.

### Rättat

- Word-filer får uppdaterat datum för senast ändrad när de sparas.

## [1.5.0] – 2026-11-12

### Nytt

- **Typsnitt, storlekar och färger från Word.** Dokumentets typsnitt, textfärger och markeringar visas som i Word. Typsnitten hämtas från systemet, så Arial visas med Liberation Sans och Calibri med Carlito.
- **Styckejustering.** Vänster, centrerat, höger och marginaljustering, med knappar och kortkommandona `Ctrl+L`, `Ctrl+E`, `Ctrl+R` och `Ctrl+J`.
- **Val av typsnitt, storlek, textfärg och markeringsfärg** i formateringsfältet.
- Listor dras in med hängande indrag.
- Typsnitt, storlekar, färger och justering följer med när du sparar som Word eller PDF.

### Ändrat

- Smalare formateringsfält.

### Rättat

- `Ctrl+L`, `Ctrl+E`, `Ctrl+R`, `Ctrl+J` och `Ctrl+B`, `Ctrl+I`, `Ctrl+U` stjäl inte längre kortkommandon som använder Ctrl+Shift.
- Punkter i PDF-listor visas bara om de finns i typsnittet.

## [1.4.0] – 2026-11-11

### Nytt

- **Dokumentläge för Word.** Ett `.docx` visas som ett dokument på en vit sida mot grå bakgrund, med rubriker, fetstil, kursiv, listor och tabeller som i Word. Markdown-koden är dold utom på raden där markören står.
- **Formateringsfält** med stilval (Brödtext, Rubrik 1–3, Citat), fet, kursiv, understruken och genomstruken, listor, citat, ångra och gör om samt zoom.
- **Stavningskontroll** med rött vågigt understreck. Högerklicka för förslag, *Ignorera alla* eller *Lägg till i ordlistan*. Använder systemets ordlistor för svenska och engelska.
- Statusfält med sida, antal ord, språk och zoom.
- Understruken text (`<u>`) i Word och PDF.

### Ändrat

- Numrerade listor från Word får rätt nummer.
- `.deb`-paketet rekommenderar svenska och engelska ordlistor.

## [1.3.0] – 2026-11-11

Innehåller även 1.2.0.

### Nytt

- **Bilder i Word-dokument.** Bilder visas i texten, följer med när du sparar som `.docx` och hamnar i en mapp bredvid filen om du sparar som Markdown. Infoga bilder med `Ctrl+Shift+I` eller genom att dra in en bildfil.
- **PDF.** Kladd läser texten i en PDF och visar den som Markdown, med rubriker, listor, tabeller och bilder. Spara som `.pdf` ger en A4-PDF med sökbar text, bokmärken och klickbara länkar.

## Tidigare versioner

### Nytt

- **Word-dokument (`.docx`).** Öppna och spara Word-filer.
- Flikar, syntaxfärgning, sök och ersätt, Markdownförhandsvisning, två teman och stöd för UTF-8, UTF-16 och Windows-1252.
