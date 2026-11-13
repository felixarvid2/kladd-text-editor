# Ändringslogg

Här står vad som är nytt i varje version av Kladd, med den senaste överst.

## [Ej släppt]

### Nytt

- **AppImage för Linux.** Varje release innehåller nu `Kladd-<version>-x86_64.AppImage` som körs på de flesta distributioner utan installation.

### Ändrat

- **Valfri kodsignering i release-bygget.** Windows (signtool) och macOS (Developer ID, notarisering) signeras om certifikat finns som hemligheter i repot; annars byggs paketen som förut. Beskrivet i `docs/dev/signering.md`. Ett utkast till Flatpak-paket finns i `packaging/flatpak/`.
- **macOS: öppna filer från Finder.** *Öppna med*, dubbelklick och att dra filer till Dock-ikonen öppnar nu filerna i Kladd (som flikar i det öppna fönstret). Kladd tar emot Apple-händelsen "öppna dokument" och deklarerar `.txt`, `.md`, `.docx`, `.odt` och `.pdf` som dokumenttyper med låg rangordning, så ingen filtyp byter program av sig själv.
- **Uppdateringskontroll.** Kladd frågar högst en gång per dygn GitHub efter en nyare version och visar ett diskret fält med *Visa nyheter* (utgåvans anteckningar som Markdown), *Hämta och installera*, *Senare* och *Hoppa över*. *Sök efter uppdateringar* i huvudmenyn kontrollerar direkt, och *Sök efter uppdateringar automatiskt* stänger av den dagliga kontrollen. Windows hämtar installationsprogrammet och kör det när Kladd avslutas, macOS öppnar skivavbilden, .deb-installationer öppnar paketet (med kommandot att kopiera) och AppImage byter ut sig själv och startar om. Inget körs utan att du klickar.
- **Spara automatiskt** (huvudmenyn, av som standard). Ändrade flikar med sökväg sparas till sin egen fil två sekunder efter att skrivandet stannat och när fönstret tappar fokus eller fliken byts. Namnlösa flikar, filer som kräver Spara som, PDF-flikar och format som skulle tappa innehåll (Word/ODT med varning, omvandlade PDF:er) sparas aldrig automatiskt. Har filen ändrats på disk sparas inget, och en notis visas.
- **Versionshistorik.** Varje sparning lagrar en version i Kladds datamapp (högst 50 per fil, gallras till en per timme efter en timme och en per dygn efter ett dygn, äldre än 30 dagar tas bort). *Versionshistorik…* i huvudmenyn visar versionerna med en radvis jämförelse mot aktuell text (för textformat), *Återställ* (går att ångra) och *Öppna som ny flik*.

## [1.9.0] – 2026-11-12

### Nytt

- **PDF-visare.** PDF:er öppnas nu i en riktig PDF-visare som visar sidorna som de ser ut. Sidorna renderas med hayro (ren Rust) och är skarpa på HiDPI-skärmar. Renderingen sker i bakgrundstrådar, så gränssnittet aldrig fryser. Grå arbetsyta med vita sidor och skugga, och kontinuerlig rullning.
- **Zoom.** Automatisk (som pdf.js), Verklig storlek, Hel sida, Sidbredd och 50–400 %. Ctrl+plus/minus, Ctrl+0 (100 %), Ctrl+1/2/3, Ctrl+mushjul och nypning kring pekaren. Zoomen går från 10 till 2500 %.
- **Uppslag.** Visa en sida eller två sidor, med udda eller jämna sidor till vänster. Rotera med R och Shift+R. Färgval: normala, inverterade eller sepia.
- **Navigering.** Sidfältet visar "3 / 12" (Ctrl+G). PgUp/PgDn, Mellanslag, N/P, J/K, Home/End. Alt+Vänster och Alt+Höger går bakåt och framåt i historiken efter länkar och hopp. Sidetiketter (till exempel "iv") visas när PDF:en har sådana. Handverktyget (H) och dra med mittenknappen. Sida, zoom och rotation sparas per fil.
- **Sidopanel** (F4) med miniatyrer och innehållsförteckning (bokmärken) som följer aktuell sida.
- **Markera text** med musen: dubbelklick markerar ett ord och trippelklick en rad. Ctrl+C kopierar och Ctrl+A markerar allt.
- **Sök** (Ctrl+F) i hela dokumentet: "3 av 17", och alla träffar markeras. Sökningen skiljer på stora och små bokstäver och på hela ord när det är valt. Diakritiska tecken ignoreras. Enter, Shift+Enter och F3 går mellan träffarna.
- **Klickbara länkar.** Interna länkar hoppar i dokumentet. Externa länkar öppnas i webbläsaren efter bekräftelse.
- **Redigera**-knappen gör om PDF:en till ett redigerbart dokument i dokumentläget. Omvandlingen är samma som tidigare: text, rubriker, listor, tabeller och bilder. Layouten kan ändras, och originalet ändras inte förrän du sparar.
- **Utskrift** skickar originalfilen. Lösenordsskyddade PDF:er skrivs ut som en avkrypterad kopia. *Spara kopia* (Ctrl+S) sparar originalet oförändrat.
- **Dokumentegenskaper:** titel, författare, ämne, nyckelord, datum, program, PDF-version, sidstorlek med pappersnamn, antal sidor och filstorlek.
- **Presentationsläge** (F5) visar helskärm med en sida åt gången. Esc avslutar.
- **Läser om automatiskt** när filen ändras på disk, och positionen behålls. *Läs om* finns också i Visa-menyn.
- **Kopiera sidan som bild** och **Öppna i systemets PDF-visare.** Statusraden visar sida, PDF-version och zoom.
- Trasiga sidor visar ett fel på sidan i stället för att krascha. PDF-flikar räknas aldrig som ändrade.

### Ändrat

- En PDF visas inte längre som Markdown-text när den öppnas. Tryck **Redigera** för det. PDF:er som visaren inte kan läsa öppnas som text, som förut.
- Numrerade listor från PDF där numret ligger i två textbitar ("1" + "1.") blir rätt nummer (11) vid omvandlingen.

## [1.8.0] – 2026-11-12

### Nytt

- **Kladd för Windows och macOS.** Varje release innehåller nu ett installationsprogram och en zip för Windows (x64) samt en universell `.dmg` för macOS (Apple Silicon och Intel), förutom `.deb` för Linux. Paketen är inte signerade; se "Kom igång" i README för hur första starten går till.
- Windows-installationsprogrammet installerar per användare utan administratörsrättigheter, lägger Kladd i Startmenyn, kan lägga en genväg på skrivbordet och kan visa Kladd under *Öppna med* för `.docx`, `.odt`, `.md` och `.txt`. Det finns på svenska och engelska.
- Ikon och versionsinformation (Kladd, version, upphovsman) finns i `kladd.exe`, och Kladd har en macOS-ikon (`kladd.icns`).
- Testerna körs nu på Linux, Windows och macOS i CI.
- **⌘ på Mac.** Kortkommandon, menyer och verktygstips använder ⌘ och ⌥ på Mac och Ctrl och Alt på Linux och Windows.
- **Stavningskontroll på Windows och macOS.** Kladd hittar LibreOffices ordlistor och kan hämta ordlistor (svenska, engelska, norska, danska, tyska, franska, spanska) direkt i appen. Språket väljs efter systemets inställning.
- **Dokumentens typsnitt på Windows och macOS**, från systemets egna typsnitt (Arial, Times New Roman, Calibri om Office finns …).
- **Utskrift på Windows** via systemets skrivare och PDF-läsaren. På macOS och Linux används CUPS som tidigare.

### Ändrat

- Egen ordlista, hämtade ordlistor och säkerhetskopior ligger i `%APPDATA%\Kladd` på Windows och `~/Library/Application Support/Kladd` på macOS. Linux är oförändrat.


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
