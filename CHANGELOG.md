# Ändringslogg

Här står vad som är nytt i varje version av Kladd, med den senaste överst.

## [Ej släppt]

### Nytt

- **Kladd för Windows och macOS.** Varje release innehåller nu ett installationsprogram och en zip för Windows (x64) samt en universell `.dmg` för macOS (Apple Silicon och Intel), förutom `.deb` för Linux. Paketen är inte signerade; se "Kom igång" i README för hur första starten går till.
- Windows-installationsprogrammet installerar per användare utan administratörsrättigheter, lägger Kladd i Startmenyn, kan lägga en genväg på skrivbordet och kan visa Kladd under *Öppna med* för `.docx`, `.odt`, `.md` och `.txt`. Det finns på svenska och engelska.
- Ikon och versionsinformation (Kladd, version, upphovsman) finns i `kladd.exe`, och Kladd har en macOS-ikon (`kladd.icns`).
- Testerna körs nu på Linux, Windows och macOS i CI.

## [1.7.0] – 2026-11-12

### Nytt

- **Automatisk säkerhetskopia.** Osparade ändringar sparas undan var 30:e sekund. Kraschar Kladd erbjuds du att återställa dem nästa gång programmet startar.
- **Exportera till HTML och EPUB** med *Spara som*. Formatering, bilder, tabeller, fotnoter och innehållsförteckning följer med, och EPUB-filerna klarar epubcheck.
- **Bättre sök och ersätt.** Hela ord, reguljära uttryck med `$1` i ersättningen, och i dokumentläget sökning i den synliga texten, sökning efter formatering (fetstil, färg, markering …) och ersättning som formaterar träffarna.
- **Tabeller som i Word.** Långa celler bryts inuti sin kolumn och raden blir lika hög som den högsta cellen. Enter (och Skift+Enter) i en cell lägger in en radbrytning (`<br>`), och Upp/Ned flyttar mellan cellens rader och mellan celler. Dra i en kolumnkant för att ändra bredden: den skrivs som olika antal bindestreck i avgränsarraden (`| ------ | -- |`), en linje visar var kanten hamnar och ändringen är ett ångra-steg. Verktygsraden och högerklicksmenyn har *Sammanfoga celler* och *Dela cell* (`| Rubrik över två ||`). Word (`w:gridSpan`, `w:tblGrid`, `w:br`), OpenDocument, PDF, HTML och EPUB har sammanfogade celler, kolumnbredder och radbrytningar; lodrätt sammanfogade Word-celler läses som tomma celler.
- **Navigeringsfönster.** En panel till vänster visar rubrikerna (H1–H6) som ett träd med hopfällbara nivåer, markerar rubriken där markören står och har ett filter ("Sök i rubriker"). Klicka på en rubrik för att hoppa dit, i både dokumentläge och textläge. Slå på och av med knappen i titelraden, *Navigeringsfönster* i menyn eller `Ctrl+Shift+O`; valet sparas.
- **Innehållsförteckning.** *Infoga → Innehållsförteckning* (i formateringsfältet eller huvudmenyn) sätter in raden `<!-- kladd: toc -->`. Dokumentläget ritar rubrikerna H1–H3 med prickad ledlinje och sidnummer, och posterna är klickbara. Word sparas som ett riktigt innehållsförteckningsfält (`TOC`) som Word uppdaterar när filen öppnas, OpenDocument som `text:table-of-content` och PDF med riktiga sidnummer och länkar till rubrikerna. En innehållsförteckning i ett öppnat `.docx` eller `.odt` visas som samma rad.
- **Fotnoter som går att se och redigera.** Fotnotsreferenser visas som små upphöjda siffror och fotnoterna under en skiljelinje sist i dokumentet. *Infoga fotnot* (`Ctrl+Alt+F`) numrerar om automatiskt, Ctrl+klick på numret hoppar mellan referens och fotnot, och Backspace efter en referens tar bort hela fotnoten. Word (`footnotes.xml`), OpenDocument (`text:note`), PDF (längst ned på sidan), HTML och EPUB har fotnoter.
- **Flytta bilder genom att dra.** Dra en markerad bild till ett annat ställe: en linje visar var den hamnar (före eller efter stycket under pekaren), för flytande bilder också bildens ruta vid kanten. Esc eller släpp utanför sidan avbryter. Ett steg att ångra.
- **Text som flyter runt bilder.** Verktygsraden vid en bild har tre knappar för radbrytning: *I texten*, *Vänster* och *Höger*. Med vänster eller höger flyter stycken efter bilden runt den, rad för rad. Flytande bilder läses och skrivs som förankrade bilder med textomflöde i `.docx`, som ramar med omflöde i `.odt` och som CSS-flöde i HTML, EPUB och PDF.

### Ändrat

- **Stora dokument går snabbt.** Bara de stycken som ändras läggs ut på nytt, så skrivande och markörflyttar i dokument på hundratals sidor är snabba igen, i både dokumentläge och textläge.
- **Menyn *Infoga*** i formateringsfältet samlar bild, tabell, fotnot och innehållsförteckning.
- **Release-anteckningar på svenska**, hämtade ur den här filen.

### Rättat

- Smala tabeller från Word och LibreOffice behåller sin bredd i stället för att fylla hela sidan.

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
