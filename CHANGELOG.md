# Ändringslogg

Här står vad som är nytt i varje version av Kladd, med den senaste överst.

## [Ej släppt]

### Ändrat

- **Snabbare öppning av Word- och ODT-filer, inget gap efter inläsningen och flertrådad PDF-export.** Ett `.docx` på 404 sidor läses på 77 ms i stället för 160 ms (9 MB XML, 1 212 sidor: 270 ms i stället för 760 ms): källans delar (alla filer i paketet för den källbevarande sparningen) packas upp i en egen tråd medan dokumentet tolkas, fot- och slutnoterna tolkas på egna trådar under tiden brödtexten tolkas, förteckningarnas kontrollsummor delar en rubriksökning i stället för att skanna hela texten per förteckning, och XML-delarna blir `String` utan kopia. Ett `.odt` tolkar `office:text` i `content.xml` på flera trådar (bitar som börjar vid ett barns starttagg, samma träd som förut; testas mot den sekventiella tolkningen) och river träden i bakgrunden: 184 ms i stället för 243 ms för 10 MB XML. Gapet mellan färdig inläsning och första utläggning (0,35 s i debug) berodde på att första bildrutan kastade den färdiga analysen när bilderna blev kända (analysen tolkades om) och på att ordantalet i statusraden räknades om i varje bildruta; analysen behålls nu och ordantalet räknas bara när texten ändrats (82 ms i stället för 222 ms från färdig inläsning till första sidan, opt-level 2). Word- och ODT-filer som är små packade men har över 1 MB XML läses nu också i bakgrunden, så fönstret ritas direkt. PDF-exporten formar texten på arbetstrådar medan huvudtråden ritar sidorna (krilla är enkeltrådat), med egna typsnittsytor per tråd som återanvänds; PDF:en är byte för byte densamma. Uppstarten (fönster, GL, typsnitt, stavning, syntaxfärgning, inställningar) har inget kvar att vinna: Kladds egen del före första bilden är under 10 ms. Mätningar i `docs/dev/prestanda.md`.
- **Läsläget för Markdown och EPUB ritar bara det som syns.** Texten delas i block, varje blocks höjd mäts när det ritas och bara blocken i fönstret ritas (`src/mdwin.rs`, `vendor/egui_commonmark`: `show_slice`). En bildruta kostar därför lika lite för en 5 MB-fil som för en sida (debug: Markdown 24 kB 7 ms i stället för 158 ms, 100 kB 9 ms i stället för 665 ms, 5 MB 6 ms; EPUB-kapitel på 400 kB 4 ms i stället för 23 ms). Stora `.md`-filer (från 5 MB) öppnas nu i läsläget, och EPUB-kapitel delas först vid 600 000 byte i stället för 24 000. Allt fungerar som förut: rubrikhopp från navigeringsfönstret och länkar, sök med F3, länkar, bilder, tabeller, kodblock, mermaid, matematik, uppgiftslistor, dubbelklick till redigeraren, understrykningar, justerad text och fotnotsrutor. Dokument under 64 kB ritas helt första bildrutan så att rullningslisten är exakt. Ett sparat läge i en EPUB-bok från före ändringen räknas om till rätt del. Mätningar i `docs/dev/prestanda.md`.
- **Snabbare rullning i stora PDF-filer.** Sidorna runt de synliga förrenderas, med fler i rullningsriktningen. Rullar du mycket snabbt (som att dra i rullningslisten) visas sidor som saknar bild först i låg upplösning och blir skarpa strax efter att rullningen stannat, i stället för att vara tomma (1863 sidor: tomma sidor i bild 153–196 av 248 före, 0–83 efter). Texturcachen har tak 160 MB i stället för 256 MB (97–109 MB i stället för 137–141 MB vid snabb rullning).
- Ordantalet i statusraden räknas om bara när texten ändrats (förut varje bildruta).
- **Sökträffar i dokumentläget ritas som ett lager ovanpå texten i stället för att läggas in i utläggningen.** Med sökningen öppen lades hela dokumentet om vid varje tangent och varje byte av aktuell träff; nu återanvänds utläggningen. Ett tecken tar 8,6 ms i stället för 41 ms i ett dokument på 404 sidor (3 000 träffar) och 52 ms i stället för 511 ms vid 2021 sidor (15 000 träffar; resten är sökningen själv), och nästa träff 0,1 ms i stället för 35 ms (0,5 i stället för 497 ms). Träffarna ser likadana ut (skärmbilder före och efter är lika bildpunkt för bildpunkt), utom i text med egen bakgrund (markerad text, kod): där ritas träffen över bakgrunden i stället för att ersätta den.
- **Ångra-historiken i dokumentläget och textläget sparar ändringar i stället för en hel kopia av texten per steg.** Vid 2021 sidor tog 20 ångra-steg 66 MB (och 100 steg 330 MB); nu är historiken ungefär en kopia av texten. Samma regler som förut (en ångra-punkt efter en sekunds stillhet, högst 100 steg, Ctrl+Z, Ctrl+Y och Ctrl+Skift+Z) och samma resultat, jämfört mot egui:s egen historik i test. Efter 100 tangenttryck har minnet vuxit 9 i stället för 27 MB vid 404 sidor och 44 i stället för 99 MB vid 2021 sidor.
- **Kladd ritar inte om när ingenting händer.** Förut ritades appen om 80–170 gånger i sekunden med en kärna på 100 % i alla vyer (språkserverns avläsning begärde en ny bildruta varje bildruta, också utan språkserver); nu är det markörens blink (2 gånger i sekunden, och inte alls när fönstret saknar fokus) och i PDF-fliken filkontrollen en gång i sekunden. CPU-tid i vila 15–19 ms/s i stället för 760–1800 ms/s. Välkomstsidans markör, arbetsmappens och git-statusens uppdatering och PDF-filkontrollen väntar tills fönstret får fokus igen, och stavningskontrollen (språk utan ordlista) och bilder som inte går att läsa in begär inte längre bildrutor för alltid. `KLADD_TIMING=1` skriver nu antalet bildrutor och vad som begärde dem var femte sekund. Mätningar i `docs/dev/prestanda.md`.
- **Skrivning i mycket stora Word-dokument är snabbare, också när ett stycke byter höjd, och den stegvisa utläggningen fortsätter där den slutade.** Ändras ett styckes höjd (raden bryts om, Enter, Backspace över en styckegräns) tas resten av dokumentet från förra omläggningen förskjutet i stället för att läggas ut på nytt, och bara lägena, sammanfogningen och sidindelningen räknas om; förut lades allt efter ändringen ut igen efter 64 stycken. Innehållsförteckningar räknades om kvadratiskt när en ändring låg nära en (108 ms i ett dokument på 2021 sidor). Vyn jämför texten med cachens kopia en gång per bildruta i stället för åtta gånger efter TextEdit, och styckenas utseende och rubrikerna läses om bara kring ändringen. Den inkrementella analysen avstår inte längre för hela dokument med ett kodstaket mitt i en rad eller ett `~~~`-block, för referenslänkar eller för en ny fotnotsdefinition utan referenser någon annanstans (ett tecken: ca 300 ms → 2–4 ms i debug vid 404 sidor). Under den stegvisa utläggningen lägger varje bildruta bara ut de nya styckena och tar platshållarna för resten från förra bildrutan. Vid 404 sidor tar ett tecken 0,63 ms i stället för 0,88 ms i median över tio ställen och 5,1 ms i stället för 11,8 ms som sämst; vid 2021 sidor 1,9 ms i stället för 3,6 ms och 3,8 ms i stället för 108 ms (opt-level 2). Hela dokumentet stegvis tar 0,41 s i stället för 0,50 s vid 404 sidor och 1,9 s i stället för 2,7 s vid 2021 sidor. Resultatet är detsamma som en full omläggning (slumpade ändringar jämförs med en cache som gör allt, också med skalning 125 % och 150 %). Mätningar i `docs/dev/prestanda.md`.

### Rättat

- **Programmet kunde krascha i dokument med å, ä eller ö när man skrev före ett felstavat eller markerat ord, eller ångrade en stor inklistring i ett dokument med innehållsförteckning** ("end byte index … is not a char boundary"): stavnings- och grammatikträffarna och rubrikernas lägen gällde texten före ändringen men användes på den nya. Omvandlingen från byte till tecken tål nu ett läge mitt i ett tecken, och rubrikerna hämtas om efter en ändring.
- **Den inkrementella analysen kunde ge andra bitar än en full analys** när en fotnotsreferens hade en rad `[^etikett]:` som inte var någon definition (mitt i ett stycke), och när en HTML-tagg hade ett citattecken utan sin like (taggen fortsätter då efter stycket). Formatering kunde då visas fel kring ändringen tills nästa fulla analys. Hittat med slumpade ändringar.

## [2.9.0] – 2026-11-26

### Nytt

- **Bokmärken, understrykningar och anteckningar i EPUB-läsläget.** `Ctrl+D` eller bokmärkesknappen sätter (och tar bort) ett bokmärke på stället du läser. Markera text så visas en meny vid markeringen med fyra färger (gul, grön, blå, rosa), *Anteckning…* och *Kopiera*; understrykningen ritas som färgad bakgrund med ett litet märke när den har en anteckning, och ett klick på den öppnar menyn igen (byt färg, redigera, ta bort). Sidopanelen har flikarna *Innehåll*, *Bokmärken* och *Anteckningar* med kapitel, utdrag och anteckning, och *Exportera anteckningar…* (sidopanelen och Arkiv-menyn) ger Markdown med titel, författare, kapitelrubriker, citat och anteckningar som ett nytt osparat dokument. Posterna sparas per bok (bokens id ur OPF, sökvägen som reserv) i datamappen och aldrig i boken, förankras med kapitel, plats i kapitlets text och den markerade texten med omgivning så att de hittas igen efter omläggning, och har tak (2 000 per bok, 300 böcker).
- **Justerad text och förlagets formatering i EPUB.** *Aa* har *Justerad text* (av som standard; rak högerkant med sista raden vänsterställd, länkar, fet stil, sökträffar och understrykningar fungerar) och *Förlagets formatering* (på som standard): `text-align` (centrerade titlar och scenbrytningar), `text-indent` (romanernas indrag, utan extra luft mellan stycken), `margin-left`, `font-variant: small-caps`, `font-style` och `font-weight` ur bokens CSS-filer, `<style>` och `style`-attribut (väljare: element, `.klass`, `element.klass`, `#id`, enkla efterkommande; inga färger, typsnitt eller absoluta storlekar). Stycken och rubriker som berörs ritas som ett enda galleri i `vendor/egui_commonmark` (`ViewExtras`).
- **Bibliotek på startsidan.** De senast lästa EPUB-böckerna visas som omslag med titel, författare och framstegsstapel (klick öppnar där du slutade; högerklick: *Ta bort ur listan*, *Visa i filhanteraren*). Omslagsminiatyr och uppgifter sparas när boken öppnas; saknad fil visas nedtonad med *Filen hittades inte*.
- **Mermaid-diagram sparas som bilder i `.docx`, `.odt`, HTML och EPUB, och läses tillbaka som diagram.** Ett `mermaid`-block som går att rita skrivs som en SVG-bild: i Word 2016+ som `a:blip` med PNG-reserv och `asvg:svgBlip` till `word/media/*.svg` (som andra SVG-bilder), i ODT som `draw:frame` med SVG och PNG-reserv, i HTML som SVG-data-URI och i EPUB som `images/imgN.svg`. Storleken är diagrammets egen, begränsad till spaltbredden. Koden följer med i bildens alternativtext (Word, ODT) respektive `data-mermaid` (HTML, EPUB), så att Kladd läser tillbaka ett `mermaid`-block och inte en bild när .docx/.odt öppnas igen; orörda stycken sparas byte för byte som förut. Block som inte går att rita sparas som kodblock.
- **Innehållsförteckningen med nivåval och som figur- och tabellförteckning.** *Infoga → Innehållsförteckning…* öppnar en dialog med val av rubriknivåer (`<!-- kladd: toc; levels: 2-4 -->`, standard 1–3) eller en figur- respektive tabellförteckning ur bildtexter som börjar med *Figur 1* / *Tabell 1* (`<!-- kladd: toc; kind: figures -->`, i Word `TOC \c "Figur"`, i ODT `text:illustration-index`/`text:table-index`). Nivåerna läses tillbaka ur Words `\o "2-4"` och ODT:s `text:outline-level`, posterna dras in per nivå, och HTML/EPUB följer nivåvalet. Formatmallarna Innehåll 1–9 finns i .docx.

### Ändrat

- **Snabbare skrivning i fotnoter och i mycket stora Word-dokument.** Text i en fotnotsdefinition, i stycket närmast definitionerna och i slutnoter tar nu lika lite tid som i vanlig brödtext: den inkrementella analysen tolkar bara definitionen med ändringen och en på var sida (Word-läsaren skriver definitionerna som ett block utan tomrader), och fotnotsplanen räknas om bara i fönstret kring ändringen. Utläggningen återupptas vid det ändrade stycket med det sparade tillståndet (läge, flytande bilder, textrutor) och tar resten från förra omläggningen när tillståndet är detsamma som då, och sidindelningen görs om bara från sidan med ändringen tills sidorna åter börjar som förut (`word/layout_resume.rs`). Två sökningar som var kvadratiska i dokument med tusentals bilder och textrutor är linjära. Ett tecken tar 1,5 ms i stället för 13,7 ms i median över tio ställen i ett dokument på 404 sidor (i den första fotnotsdefinitionen 1,3 ms i stället för 38,8 ms), och 4,6 ms i stället för 113 ms i ett på 2021 sidor (opt-level 2; i debug 6,3 i stället för 83 ms respektive 13 i stället för 684 ms). Ett tecken som ändrar styckets höjd lägger fortfarande om allt efter det (14 ms vid 404 sidor). Resultatet är detsamma som en full omläggning (slumpade ändringar jämförs med en ny cache i test). Mätningar i `docs/dev/prestanda.md`.
- **Sidnumren i Word- och ODT-förteckningen är riktiga.** De uppskattades förut (Word: ett antal tecken per sida, ODT: alltid sida 1) och skrivs nu ur samma utläggning som PDF-exporten. Då behövs inte längre `w:updateFields`, så Word frågar inte om fälten ska uppdateras vid öppning (frågan ställs bara när utläggningen inte kunde paras ihop med posterna); `F9` i Word räknar om efter Words egen sidbrytning. En förteckning i ett öppnat .docx skrivs om när rubrikerna ändras och behåller annars sitt XML. HTML/EPUB tar med rubriknivå 1–3 som övriga format (förut alla nivåer).
- **PDF-exporten läggs ut med samma mått som dokumentläget, så sidindelningen och förteckningens sidnummer blir desamma i vyn, i PDF, .docx och .odt.** Förut hade PDF större rubriker med accentstreck, radhöjd 1,5 och IBM Plex Sans som standardtypsnitt, så att samma dokument fick fler sidor i PDF än i vyn (t.ex. kapitel 2 på sida 3 i PDF men sida 2 i vyn). Nu gäller: Calibri-lika Carlito som standardtypsnitt, radhöjd 1,4 för text och 1,3 för rubriker, rubrikstorlekarna 18/15/13/12/11 pt, 8 pt tom rad efter stycken, raderna avrundade till hela bildpunkter, och förteckningen som en rad med dokumentlägets höjd (högst en sida, längre register kortas med …). PDF:ens utseende blir därmed mer likt vyn och Word; accentstrecket under rubrik 1 är borta. Typsnitt som inte följer med programmet (t.ex. Georgia) ger fortfarande små skillnader i radbrytning mellan vyn och PDF.

### Rättat

- **Dubbelklick i läsläget för Markdown öppnade inte redigeringen första gången** i dokument som börjar med ett HTML-block (som `<p align="center">` i många README-filer): det fanns ingen blockposition före första stycket, så ingen rad hittades och dubbelklicket markerade bara ett ord. HTML-block, tabeller och linjer får nu egna positioner, och ett dubbelklick över första blocket öppnar redigeraren i början.
- **En innehållsförteckning som första rad i ett dokument försvann vid sparning till .docx och .odt.** Raden tolkades som dokumentets grundformat och togs bort.

## [2.8.0] – 2026-11-25

### Nytt

- **EPUB öppnas i ett läsläge.** En `.epub` (Arkiv → Öppna, dra och släpp eller kommandoraden) blir en flik som heter bokens titel och öppnas där du slutade läsa (omslaget första gången). Kapitlen ritas med läsläget för Markdown i en centrerad spalt med serifftext, rubriker, listor, tabeller, citat, kodblock och bilder (SVG-omslag också), och stora kapitel delas vid styckegränser så att rullningen går flytande. Bläddra med mellanslag och Page Down (Skift+mellanslag och Page Up bakåt, vidare till nästa kapitel i slutet), `[` och `]` eller `Alt+Upp`/`Alt+Ner` för kapitel, `Ctrl+Home`/`Ctrl+End`, `Alt+Vänster`/`Alt+Höger` efter länkhopp, och klick i marginalen. Innehållsförteckningen (EPUB 3 `nav`, EPUB 2 `toc.ncx` eller kapitlens rubriker) ligger i sidopanelen (`Ctrl+Skift+O`), `Ctrl+F` söker i hela boken med träfflista, fotnoter öppnas i en liten ruta med *Gå till noten*, framstegslinjen längst ner har streck vid kapitelgränserna och visar tid kvar, och *Aa* ställer in textstorlek, typsnitt, radavstånd, spaltbredd och sidfärg (Auto, Ljus, Sepia, Mörk). `Ctrl+I` visar *Om boken*. *Redigera en kopia* gör bokens text och bilder till ett nytt osparat dokument; boken sparas aldrig. Kopieringsskyddade (DRM) böcker ger ett tydligt fel. Se `docs/dev/epub.md`.

## [2.7.0] – 2026-11-25

### Nytt

- **Kodblock med syntaxfärgning och kopieringsknapp i dokumentläget och exporterna.** Ett ```` ```språk ````-block ritas i dokumentläget som en ruta med svag bakgrund, fast breddsteg och färger per token efter språket (rust, python, js/ts, json, toml, yaml, bash, c/c++, java, go, sql, html, css, xml, markdown med flera; okänt språk är ofärgat), med det ljusa temats färger på vita sidor och det mörka i mörkt tema utan vita sidor. Språknamnet står i hörnet och en kopieringsknapp (när musen är över blocket) kopierar koden utan inhägnaden och visar *Koden kopierades*. Färgningen använder samma syntect-uppsättning som textredigeraren (inläst i bakgrunden; ofärgat tills den finns), ändrar aldrig texten eller markörlägena och cachas per block, så att skrivning i ett annat stycke inte färgar om. PDF ritar kodblock med bakgrund och samma färger, `.docx` skriver kodformatet med färgade körningar (`w:color`) och språket i ett dolt bokmärke som läses tillbaka, `.odt` skriver `text:span`-färger och HTML/EPUB `<pre><code class="language-x">` med färgade `<span style>`. Orörda `.docx` och `.odt` sparas som förut byte för byte. Läsläget visar nu också språknamnet i blockets hörn. Se `docs/dev/dokumentlage.md`.
- **`mermaid`-block ritas som diagram.** Flödesscheman, sekvens-, tillstånds-, klass-, ER- och cirkeldiagram med flera ritas av den rena Rust-crate:n `mermaid-svg` 0.7 (MIT, enda beroendet är `thiserror`; inget JavaScript eller C) i läsläget, i dokumentläget (koden visas när markören står i blocket) och som vektorbild i PDF. Ett block som inte går att rita visas som kod; `.docx`, `.odt`, HTML och EPUB behåller det som kodblock.
- **Tangenttryck i stora Word-dokument är 5–10 gånger snabbare och växer knappt med dokumentets längd.** Efter en ändring räknas bara stycken, fotnotsplan, grupper och cachenycklar om i ett fönster kring ändringen och markören; resten från förra omläggningen förskjuts (`word/layout_memo.rs`, jämförs mot en full omräkning av slumpade ändringar i test). Fotnotsberäkningarna var kvadratiska (en linjär sökning i egui:s galleri för varje fotnotsreferens, och en genomsökning av hela texten för varje etikett) och är nu logaritmiska respektive bakifrån. Ett tecken i ett dokument på 404 sidor (`gen-skolrapport.docx` upprepad) tar 11 ms i stället för 56 ms i median över tio ställen (sämsta 37 ms, i fotnotsdefinitionerna; 133 ms före), och 68 ms i stället för 524 ms i ett debugbygge. Mätningar i `docs/dev/prestanda.md`.
- **Färre fall av full analys i stora dokument.** Den inkrementella analysen gäller nu också text i en fotnotsdefinition, ändringar bredvid avsnittsrader, en vanlig första rad, dokument med välformade kodstaket (Word-dokument med kodstycken) och referenslänkar när fönstret saknar hakparenteser; de globala kontrollerna räknas per fönster och söker inte längre igenom hela texten vid varje tangent.
- **Analysen av ett stort Word-dokument görs i inläsningstråden** i stället för i första bildrutan, och platshållarna under stegvis utläggning kalibreras per styckeslag (brödtext, rubrik, lista, citat, kod, tabellrad). Statusraden räknar upp sidantalet från det utlagda (inom 10 % av slutvärdet i mätningen) i stället för att visa platshållarnas ryckiga sidantal, och sidor som ännu inte lagts ut ritas med *Läggs ut…*. `KLADD_TIMING=1` skriver tiden för analys och omläggning per ändring.

### Rättat

- **Sökningen i PDF-visaren kunde hoppa till fel träff.** Sidornas text läses in i godtycklig ordning, och den första träffen valdes bland de sidor som råkade vara genomsökta, så en träff på sidan 3 kunde bli aktuell före träffarna på sidan 1. Nu väntar hoppet tills alla sidor fram till träffen är genomsökta.
- **Den inkrementella analysen användes aldrig i dokumentläget i appen.** Bildrutan höll en referens till analysen medan TextEdit lade ut galleriet efter en ändring, så analysen kunde inte uppdateras på plats och hela texten tolkades om vid varje tangent (80–185 ms i debug för 400 sidor). Mätverktygen hade ingen sådan referens och visade det aldrig. Referensen släpps nu före TextEdit.

## [2.6.1] – 2026-11-23

### Nytt

- **Stora Word-dokument öppnas utan att fönstret fryser.** En `.docx` eller `.odt` över 100 kB läses in i en bakgrundstråd med förlopp (som stora textfiler), och i dokumentläget läggs dokumentet ut stegvis: tills tidsbudgeten per bildruta är slut läggs stycken ut på riktigt, resten ersätts av platshållare med uppskattad höjd, så första sidan syns direkt och resten fylls i under följande bildrutor. Statusraden visar *Sida X av ca N (lägger ut dokumentet: Y %)*; en tangent, ett hopp (sök, rubrikträd) eller att bara platshållare syns lägger ut resten direkt. Det färdiga galleriet är identiskt med det som lades ut på en gång (täcks av test), dokument under 60 kB påverkas inte, och en redigering lägger fortfarande bara om stycket som ändrats. Mätningar och verktyg (`bench_docx_open`) i `docs/dev/prestanda.md`.
- **Snabbare tangenttryck i stora Word-dokument med fotnoter och avsnitt.** Den inkrementella analysen (bara stycket med ändringen och dess grannar tolkas om) gäller nu också dokument med fotnoter och avsnitt, som tidigare alltid tolkades om i sin helhet vid varje tangent, och rättar en gammal brist där en lång bit över fönstrets kant kunde ge en annan tolkning än full analys.

### Rättat

- **PDF-exporten av stora dokument är mycket snabbare.** Utjämningen av spalter på avsnittets sista sida lade ut hela dokumentet om för varje försök (kvadratisk tid med många spaltavsnitt: 150 spaltavsnitt tog 31 s, nu 0,4 s); nu läggs varje avsnitt ut en gång och bara det avsnittet läggs om vid sökningen, med exakt samma resultat. Sidhuvudets och sidfotens bilder avkodades på varje sida och bilder avkodades på nytt i varje utläggningspass (nu en gång per export), orden mäts en gång per export i stället för en gång per pass och texten formas på flera trådar utan att varje textrad tolkar om typsnittet. Dokument med 961 sidor (spaltavsnitt, fotnoter, tabeller, bilder och textrutor) tar 3 s i stället för 7 s och PDF:en är byte för byte densamma. `KLADD_TIMING=1` skriver tiden per fas vid export. Se `docs/dev/prestanda.md`.

## [2.6.0] – 2026-11-23

### Nytt

- **SVG behåller vektorkvaliteten i Word, LibreOffice och PDF.** I `.docx` skrivs en SVG som Word 2016+ gör (`a:blip` till en PNG-reserv och `asvg:svgBlip` till `word/media/*.svg`), i `.odt` som LibreOffice (en `draw:image` med SVG:n och en med PNG-reserv, med manifest) och i PDF ritas den som vektor med `krilla-svg` (text blir text). HTML och EPUB behåller SVG-filen. Båda formaten läses tillbaka med SVG:n kvar, så den går inte förlorad vid tur och retur (LibreOffice-filer med SVG visar nu bilden i stället för att räknas som bortfallna former). Nya beroenden: `krilla-svg` 0.8 och `resvg` 0.47 (uppgraderad från 0.45; MIT/Apache-2.0).
- **Fot- och slutnoternas talformat följer dokumentet.** `w:numFmt` och `w:numStart` i `w:footnotePr`/`w:endnotePr` (settings.xml och avsnittens `w:sectPr`) och `text:notes-configuration` i `.odt` läses: decimal, små och stora romerska tal, små och stora bokstäver och Chicago (*, †, ‡, §), med eget startnummer. Referenserna och notlistorna visas så i dokumentläget och i PDF-exporten; etiketterna i texten är oförändrade och formatet ligger i grundformatsraden (`endnote-format: decimal; footnote-format: lowerRoman; endnote-start: 3`). Äldre dokument ser ut som förut (fotnoter 1, 2, 3, slutnoter i, ii, iii). Formatet skrivs tillbaka till `.docx` och `.odt` (orörda dokument byte för byte), och *Infoga → Fotnoternas numrering* och *Slutnoternas numrering* i verktygsraden byter format. Formatet gäller hela dokumentet; olika format i olika avsnitt visas med det första avsnittets.
- **Textrutor kan stå ut i marginalen, som i Word.** En ruta vars läge (från spalten, marginalen eller sidan) hamnar delvis eller helt utanför spalten ritas där den står, och bara klämms mot papperets kant. Texten flyter runt den del som ligger i spalten; en ruta helt i marginalen (som faktarutan i `gen-skolrapport.docx`) påverkar inte texten. Dragning får flytta rutan ut i marginalen och läget sparas som det står i `.docx` och `.odt`. Ritning, layout och PDF-export räknar med samma läge.
- **Olika formatering i samma sidhuvud eller sidfot, per körning som i Word.** Mallen får samma inline-formatering som brödtexten: `**fet**`, `*kursiv*`, `<u>understruken</u>` och `<span style="color: #C00000; font-size: 14pt; font-family: Arial">…</span>`, också runt fälten (`Sida **{sida}** av {sidor}`) och i flera rader och delar (`**Skolrapport** · Klass 9B||{sida}`). `header-style` är kvar som standardstil för hela sidhuvudet och körningarna lägger bara till det som avviker; gamla mallar utan märken ser ut som förut, och en ensam `*` eller ett `<u>` utan par är vanlig text (`\*` skriver ett tecken som annars blir märke). Formateringen ritas i dokumentläget och i PDF-exporten (fetare, kursivare, understruken, färg, egen storlek; raden blir så hög som den största körningen). I `.docx` läses `w:rPr` per `w:r` i `header*.xml`/`footer*.xml` (`b`, `i`, `u`, `color`, `sz`, `rFonts`, också för fält som PAGE och NUMPAGES) och skrivs tillbaka som en `w:r` per körning med rätt `w:rPr`; i `.odt` läses `text:span` med automatiska stilar och skrivs som `text:span` med egen teckenstil. Är alla körningar i första stycket fetstil är det i stället standardstilen (`bold`), och typsnittet i första körningen är bastypsnittet. Orörda sidhuvuden i en `.docx` sparas fortfarande byte för byte. *Sidhuvud och sidfot*-dialogen har knapparna **F**, **K** och **U** (och `Ctrl+B`, `Ctrl+I`, `Ctrl+U` i ett fält) för markerad text, och förhandsvisningen visar formateringen.

### Rättat

- **En SVG utan `width` visades i dubbel storlek.** Rastret ritas i dubbel upplösning och styrde storleken; nu används SVG:ns egen storlek (`width`/`height`, annars `viewBox`) i dokumentläget, i sidhuvuden och vid export.
- **Markör och klick bredvid textrutor.** Ett klick i brödtexten bredvid en ruta hamnar i brödtextraden som syns där och ett klick i rutan i rutans text; pil upp/ned i brödtexten hoppar över rutans rader, i rutan rör de sig inom den och lämnar den vid kanterna; drag med musen över en ruta markerar brödtext i läsordning (`src/word/tboxnav.rs`).
- **Faktarutan i `gen-skolrapport.docx` ritades ovanpå brödtexten.** Ritningen klämde in rutan i spalten medan layouten lät den stå i marginalen. Alla lägen (kolumn, marginal, sida, tecken) ritas nu på samma plats som i layouten och i PDF-exporten, med ett regressionstest per läge.

## [2.5.1] – 2026-11-21

### Nytt

- **PDF-exporten jämnar ut spalterna** på avsnittets sista sida före ett löpande avsnittsbyte, för lika och olika breda spalter, så att nästa avsnitt börjar under den högsta spalten och skiljelinjerna följer den utjämnade höjden. Ett avsnitt med manuell spaltbrytning jämnas inte ut, och inte heller det sista avsnittet i dokumentet. Fotnoter fungerar som förut. Ett `text:section` med spalter i en `.odt` (LibreOffice) jämnas ut också när det står sist i dokumentet, i PDF och dokumentläget; spalter från sidmallen och i `.docx` jämnas inte ut sist. `text:dont-balance-text-columns="true"` läses som `column-balance: no` (avsnittet jämnas inte ut, varken i PDF eller dokumentläget) och skrivs bara tillbaka för det; olika breda spalter skrivs inte längre som obalanserade.

### Rättat

- **Text flyter nu alltid runt (eller hoppar förbi) en dragen textruta.** Dras en ruta in mot mitten av en spalt, där det inte finns plats bredvid den på någon sida, ritades texten förut över rutan i dokumentläget: hoppet förbi rutan förbrukades av den dolda taggraden (`<div>`) före stycket och gick förlorat vid spaltindelningen. Hoppet läggs nu på första synliga stycket och bevaras av sidbrytningen i dokument med spalter. En ruta som dragits (`x`, `y`, ny storlek) och sparas i ett nytt `.docx` behåller dessutom läge, omflöde och avstånd (förut föll `y` och omflödet bort). En ruta dragen ut över spaltens kant ritas inom spalten, och texten flyter nu runt den där den syns (layouten använder samma kant som ritningen).

## [2.5.0] – 2026-11-21

### Nytt

- **Textrutor med omflöde, överallt och som går att dra.** En förankrad textruta (`wp:anchor` i Word, `draw:frame` i LibreOffice) ritas på sitt läge mot kolumn, marginal, sida eller stycke med den fasta höjd dokumentet anger (`a:ext`), och texten flyter runt den som runt förankrade bilder (fyrkantigt på den bredare sidan, över och under, bakom och framför), i dokumentläget och i PDF-exporten. Rutor i tabellceller, rubriker och listor, rutor med bilder och rutor med fotnotsreferenser (texten visas) visas nu också. Klicka på rutans kant för att markera den, dra i kanten för att flytta den och i handtagen för att ändra storlek; nytt läge och ny storlek skrivs som `wp:positionH/V`, `wp:extent`, `a:ext` och VML-`style` i `.docx` och som `draw:frame` i `.odt`. Orörda rutor sparas byte för byte som förut.
- **Öppna `.doc`, `.rtf`, `.wpd` och `.wps`.** Kladd konverterar filen till `.docx` med LibreOffice (`soffice --headless`, egen tillfällig profil, tidsgräns 2 minuter) i en bakgrundstråd med *Konverterar …* i hörnet och öppnar resultatet i dokumentläget. Fliken kan bara sparas med *Spara som* (förvalt `.docx` bredvid originalet), så originalet skrivs aldrig över i fel format. Utan LibreOffice visas ett tydligt fel; `.rtf` läses då med en enkel inbyggd läsare (text, fet, kursiv, stycken). Filerna finns i filväljaren som *Äldre dokument*.
- **WebP och SVG som bilder.** *Infoga bild…*, dra och släpp, inklistring (`data:`-bilder) och loggaväljaren tar nu emot `.webp` och `.svg` (SVG ritas med `resvg`). I dokumentläget, förhandsvisningen och exporten (`.docx`, `.odt`, PDF, HTML, EPUB) används en PNG-version, eftersom äldre Word och PDF inte kan formaten.
- **Typsnitt som matchar Office.** En gemensam ersättningstabell (`src/word/fontmap.rs`) används av dokumentläget och PDF-exporten när originalet saknas: Calibri och Calibri Light blir Carlito, Cambria blir Caladea, Arial Liberation Sans, Arial Narrow Liberation Sans Narrow, Times New Roman Liberation Serif, Courier New Liberation Mono och Georgia Gelasio. Caladea (SIL OFL, metriskt lika Cambria) följer nu med i appen, så text i Cambria bryts på samma ställen som i Word på alla plattformar och blir inte längre en sans-serif på system utan Caladea; ett riktigt installerat Cambria används före det. PDF-exporten bäddar in Carlito och Caladea när dokumentet använder Calibri respektive Cambria eller ett annat serif-typsnitt (förut alltid IBM Plex). Calibri Light saknar fri motsvarighet och visas i Carlito. Typsnittslistan i dokumentläget tar med Cambria.
- **Snabbare start.** Tiden till första bilden i ett debugbygge föll från ca 310 till ca 180 ms utan fil och från ca 290 till ca 150 ms med en Markdown-fil: Markdown-förhandsvisningens kodfärgning läses nu först när ett kodblock ritas (den lästes förut tre gånger vid start) och editorns syntaxer läses i en bakgrundstråd medan fönstret skapas. `KLADD_TIMING=1` skriver starttiderna till stderr; se `docs/dev/prestanda.md`.
- **Fotnoter längst ned på sin sida i dokumentläget.** Varje fotnot ritas nu under en kort skiljelinje längst ned på sidan där referensen står, i mindre text med upphöjt nummer, som i Word och i PDF-exporten. Sidbrytningen reserverar plats för sidans fotnoter, och en rad med en referens flyttas till nästa sida om raden och noten inte ryms tillsammans (spalter fungerar också). Klicka på en fotnot, dubbelklicka referensen eller använd *Infoga → Fotnot* för att redigera noten på plats; Enter eller Esc avslutar. Definitionerna ligger kvar sist i texten (dolda i flödet), så markör, klick och markering fungerar som förut. Slutnoter står kvar sist i dokumentet.
- **Dialogen för sidhuvud och sidfot** visar en logga som miniatyr med namn och bredd (en bricka `[Logga 1]` i fältet) i stället för bildmärket `{bild:…}`, har knapparna *Infoga sidnummer*, *Antal sidor*, *Logga…* (välj en bildfil), *Ta bort logga*, *Lägg till rad* och valet *Sidnummerformat* (1, i, I, a, A), och kan ändra loggans bredd.
- **Sidhuvud med flera stycken** behåller varje stycke som en egen rad (`\n` i mallen) vid läsning av `.docx` och `.odt`, i dokumentläget, i PDF och vid skrivning, och dialogen kan lägga till och ta bort rader.
- **Fet, kursiv och understruken text i sidhuvud och sidfot** (`header-style: "bold italic underline"`) ritas nu också i dokumentläget, och fetstil och understrykning i PDF-exporten. Understrykning läses och skrivs i `.docx` och `.odt`.
- **Läsläget för Markdown blir användbart.** `Ctrl+F` söker i den ritade texten och markerar träffarna (F3 och Skift+F3 hoppar och rullar dit) utan att byta till redigeraren; `Ctrl+H` byter till redigeraren. Rubrikerna i navigeringsfönstret rullar läsvyn. Kryssrutor (`- [ ]`/`- [x]`) bockas av med ett klick (bara tecknet i hakparentesen ändras, fliken blir ändrad, `Ctrl+Z` och `Ctrl+Y` fungerar i läsläget). Dubbelklick på ett stycke, en rubrik, en listpunkt eller en tabellrad öppnar redigeraren med markören där, och rullningsläget följer med när man växlar. Kryssrutorna går också att klicka i förhandsvisningen bredvid texten.
- **Snyggare länkar och tabeller i läsläget och förhandsvisningen.** Länkar har länkfärg, och tabeller har linjer, färgad och fet rubrikrad, luft i cellerna och radbrytning.

### Ändrat

- README är nu en kort översikt med en tabell över filformaten Kladd öppnar, sparar och exporterar. Hela beskrivningen av funktionerna finns i `docs/handbok.md`.

### Rättat

- Text i tabellceller i ett sidhuvud placeras efter cellernas bredd (`w:tcW`, `w:gridSpan`, `w:tblGrid`) i stället för efter kolumnernas ordning: en smal logga till vänster och en bred titel hamnar vänster och mitten.
- Sidnumrering som börjar om per avsnitt läses nu också från `.odt` (`style:page-number` på stycket och `style:first-page-number` på sidmallen) och skrivs tillbaka.
- Ett stycke som fortsätter från en bredare spalt in i en smalare delas vid spaltgränsen: raderna bryts i respektive spalts bredd i dokumentläget och i PDF (förut fick hela stycket den smalaste bredden i vyn och den breda spaltens bredd i PDF).
- Spalter med olika bredd jämnas nu ut på avsnittets sista sida före ett löpande avsnittsbyte i dokumentläget (som Word och LibreOffice). I PDF jämnas spalter fortfarande inte ut.
- Kortkommandon med Alt tas inte längre av samma kommando utan Alt: `Ctrl+Alt+F` (fotnot) öppnade sökfältet i stället för att infoga en fotnot, och AltGr-tecken (Ctrl+Alt på Windows) kunde starta kommandon.

## [2.4.0] – 2026-11-17

### Nytt

- **Slutnoter.** Slutnoter i `.docx` (`w:endnoteReference`, `word/endnotes.xml`) och `.odt` (`text:note-class="endnote"`) visas efter dokumentets sista text med romerska tal (i, ii …) och skiljelinje, numrerade för sig så att fotnoterna behåller sina siffror. De sparas tillbaka som riktiga slutnoter (orörda dokument byte för byte), tas med efter sista texten i PDF-exporten, och *Infoga → Slutnot* (`Ctrl+Alt+D`) lägger till nya. Slutnoter är etiketterna `[^ei]`, `[^eii]` … i Markdown-texten. Fotnoter längst ned på varje sida i dokumentläget är inte gjort än.
- **Textrutor syns, går att redigera och infoga.** Texten i en textruta (`w:txbxContent` i Word, `draw:text-box` i LibreOffice) visas som stycken med rutans fyllning, ram, bredd och läge i spalten, före stycket rutan sitter i. Ändringar sparas tillbaka i rutan med form, ram och läge bevarade, och orörda rutor sparas som förut, byte för byte. *Infoga → Textruta* sätter in en ny ruta med ram som sparas som riktig textruta i `.docx` och `.odt`, och PDF-exporten ritar rutans fyllning och ram. Rutan ritas i flödet (text flödar inte runt den). Rutor i tabeller, rubriker och listor, rutor med bilder eller fotnoter, och rutor i rutor visas inte, men bevaras i `.docx`.
- **Fler testdokument.** Testkorpusen har ett nyhetsbrev (spalter, textruta, bild) och en skolrapport (sidhuvud med logga, sidfot, fotnoter, innehållsförteckning), både i Word 365-stil och som LibreOffice skapar dem.
- **Olika breda spalter och skiljelinje.** Spalter med olika bredd (`w:equalWidth="0"` i Word, `style:column` i LibreOffice) och linjen mellan spalterna visas som i Word och LibreOffice, och följer med till `.docx`, `.odt` och PDF (`column-widths` och `column-rule` i avsnittsraden).
- **Sidhuvud och sidfot som i Word.** Loggor i sidhuvud och sidfot (inline och förankrade, från Word och LibreOffice) ritas i dokumentläget och i PDF-exporten och följer med vid sparning. Sidnummerfält läses med format (`\* roman`, `\* ROMAN`, `\* Arabic`), `SECTIONPAGES` och sidnumrering som börjar om per avsnitt, och "Sida X av Y" räknas från den faktiska sidindelningen. Tabbstopp, en tabell utan ramar (logga | titel | datum), textstorlek, färg och linjen under sidhuvudet läses, och annorlunda första sida, olika udda/jämna sidor, egna sidhuvuden per avsnitt (med arv från föregående avsnitt) och avståndet från sidkanten (`w:pgMar w:header/w:footer`) visas som i originalet. Brödtexten skjuts ned om sidhuvudet med logga behöver mer plats. Orörda dokument sparas som förut, och ett sidhuvud som ändras i dialogen skrivs om med loggan och fälten kvar.

### Rättat

- **Sidbrytningar i Word- och LibreOffice-dokument.** En vanlig sidbrytning (`w:br w:type="page"`, `w:pageBreakBefore` direkt eller i en formatmall, `fo:break-before` och `fo:break-after` i `.odt`) lästes tidigare som en radbrytning eller försvann, så ett dokument fick för få sidor. Den blir nu raden `<!-- kladd: sidbrytning -->`, börjar en ny sida i dokumentläget och PDF, och sparas tillbaka som sidbrytning. Nytt: *Infoga → Sidbrytning*.
- **Stabil sparning av rubrikformat och justering.** Ett Word-dokument med svenska rubrikmallar (`Rubrik1` …) fick rubrikformaten omflyttade mellan nivåer vid andra sparningen, eftersom Kladd lade till egna `Heading1` … med samma namn. Nya rubriker använder nu dokumentets egna mallar. I `.odt` blev vänsterställda stycken justerade när brödtextmallen var justerad.
- Spalterna i ett avsnitt jämnas nu ut på avsnittets sista sida, också när avsnittet går över en sidgräns.
- Den osynliga skyddskoden `&#8203;` framför ett bokmärke eller en förankrad ruta syntes som text i dokumentläget.
- Ramar och skuggning runt stycken (`k:pp`) ritades på fel plats i avsnitt med flera spalter, och PDF-exporten ritade inga stycke-ramar alls.

## [2.3.0] – 2026-11-17

### Ändrat

- **Markdown-filer öppnas i läsläge.** En öppnad `.md`-fil visas färdig i hela fönstret i stället för som text med förhandsvisning bredvid. *Redigera* uppe till höger (eller `Ctrl+Shift+M`) byter till texten, och sidknappen i namnlisten eller `Ctrl+Shift+M` går tillbaka. Nya flikar och stora filer öppnas direkt i redigeraren, och sök och hopp i läsläget öppnar redigeraren. Förhandsvisningen bredvid texten finns kvar som val (*Visa → Förhandsvisning bredvid texten*, av som standard).

### Rättat

- Fet text och rubriker på nivå 2–3 visas feta i Markdown-förhandsvisningen. Förut syntes fetstil inte alls.

## [2.2.0] – 2026-11-17

### Nytt

- **Word- och LibreOffice-dokument ser ut som i Word.** Formatmallar, stycken, tabeller och bilder ritas i dokumentläget som i originalet, och orörda delar sparas tillbaka oförändrade.

**Formatmallar**

- **Titel och Underrubrik** som egna stilar i dokumentläget (stilvalet, *Ändra formatmall…*). En Word-titel (Title) är inte längre en liten Rubrik 1: den ritas med sin egen storlek, sitt typsnitt och sin färg, och Underrubrik (Subtitle) med kursiv och teckenavstånd. I Markdown skrivs de `# Text {.title}` och `## Text {.subtitle}` (markören döljs) och följer med som Wordstilarna Title/Subtitle till `.docx`, som `.odt`-stilarna Title/Subtitle, till PDF och som `<h1 class="title">` i HTML. Titel och underrubrik hamnar inte i rubrikträdet. Stilvalet har också Rubrik 4–6.
- **Formatmallarna läses som i Word.** `docDefaults` (storlek, fet, kursiv) och temafärger (`w:themeColor` med tint/shade ur temats `clrScheme`) räknas in, teckenavstånd (`w:spacing`), versaler och kapitäler (`w:caps`, `w:smallCaps`) och linjen under stycket (`w:pBdr/w:bottom`, t.ex. under titeln) läses och skrivs tillbaka, och teckenmallar med fet eller kursiv (Subtil betoning m.fl.) syns i texten. En mall som inte anger färg eller fet är vanlig svart text, inte Kladds blå fetstilsrubrik. Grundformatskommentaren får `letter-spacing`, `caps`, `smallcaps` och `border-bottom`.
- **Vita sidor i mörkt tema** (*Visa → Vita sidor i mörkt tema* och *Inställningar → Dokument*, på som standard): dokumentläget ritar sidan vit med dokumentets riktiga färger och svart text som i Word och LibreOffice, så att mörkblå och lila text går att läsa. Stavningsvågor och markering syns som förut.

**Stycken**

- **Styckeutseende som i Word.** Dokumentläget ritar styckeramar (`w:pBdr`: enkel, dubbel, streckad, prickad och tjock linje med egen färg, tjocklek och luft), skuggade stycken (`w:shd`), exakt avstånd före och efter (`w:spacing` med auto, exakt och minst radavstånd, även ur formatmallar och dokumentets standardvärden, `contextualSpacing`) och indrag (vänster, höger, första raden, hängande). Efterföljande stycken med lika ram och skuggning blir en enda ruta (mellanlinjer ritas), rutan delas vid en sidbrytning, och stycken utan avstånd ligger tätt som i Word och LibreOffice. Läses ur `.docx` till en dold markör `<!--k:pp …-->` sist i stycket (`docs/dev/dokumentlage.md`) och skrivs tillbaka i schemaordning för nya och ändrade stycken; orörda stycken skrivs som förut byte för byte.
- **Tomma stycken bevaras.** Ett tomt Word-stycke (som många använder som lodrätt avstånd) blir en egen rad `<!--k:ep-->` med stycketeckens storlek och avstånd, får höjd i dokumentläget, och skrivs tillbaka som samma XML. Att ta bort eller lägga till raden tar bort respektive lägger till ett tomt stycke.

**Tabeller**

- **Tabeller från Word och LibreOffice ser ut som i originalet.** Cellskuggning (`w:shd`, också från tabellmallar som *Grid Table 4 Accent 1* med villkorsformatering för rubrikrad, första kolumn och radband enligt `w:tblLook`), ramar (`w:tblBorders` och `w:tcBorders`: ingen, enkel, dubbel, streckad, prickad, tjocklek och färg, kraftigaste linjen vinner mellan två celler), tabellbredd (`w:tblW` auto, dxa och procent, `w:tblGrid`, fast utläggning), justering och indrag, cellmarginaler (`w:tblCellMar`/`w:tcMar`), lodrät och vågrät justering i cellen, radhöjd (`w:trHeight`) och lodrätt sammanfogade celler visas i dokumentläget. En tabell utan ramar visar inga linjer (en svag hjälplinje syns bara medan markören står i den), en ensam skuggad cell är en ruta med flera stycken och radbrytningar, och rubrikraden är bara fet när dokumentet gör den fet. Formatet ligger i en dold kommentarsrad före tabellen (`<!-- kladd-tabell: … -->`, se `docs/dev/dokumentlage.md`), orörda tabeller skrivs tillbaka byte för byte och ändrade tabeller behåller formatet (och skriver det som riktiga tabellegenskaper). Formatet följer också med till `.odt`, PDF, HTML och EPUB. Högerklicksmenyn har *Skugga celler*, *Ta bort skuggning*, *Ramar runt alla celler* och *Inga ramar*, och formatet följer med när rader och kolumner läggs till eller tas bort.

**Bilder**

- **Bilder sitter där Word och LibreOffice la dem.** Förankrade bilder (`wp:anchor`, äldre VML-bilder, ODT `draw:frame`) behåller nu sitt läge mot sidan, marginalen, spalten eller stycket (förskjutning eller vänster/mitten/höger respektive topp/mitten/botten), textomflöde (runt, över och under, bakom eller framför texten) och avstånd till texten, och ritas på rätt plats i dokumentläget och i PDF-exporten: texten flyter på den bredare sidan, hoppar förbi bilder över hela bredden, och bilder bakom/framför påverkar inte texten. Beskärning (`a:srcRect`, `fo:clip`) och exakt storlek (också sträckta bilder) bevaras, bilder i tabellceller blir vanliga bilder som inte blir bredare än cellen. Att dra en förankrad bild skriver tillbaka nytt läge till `.docx`/`.odt`. Orörda dokument är oförändrade.

### Rättat

- En tabb först i ett Word-stycke visas som tabbstopp i stället för som `&#9;`.
- När en fil öppnas i dokumentläget hamnar markören i första stycket, och dold markup visas först när du flyttar markören.
- `|` i kod i en tabellcell visas utan backstreck.

## [2.1.0] – 2026-11-13

### Nytt

**Menyer och kommandon**

- **Menyrad i stället för en lång hamburgermeny.** *Arkiv · Redigera · Visa · Infoga · Verktyg · Hjälp* (och *Format* i dokumentläget) ligger i övre raden och byggs ur ett gemensamt kommandoregister (`src/commands.rs`, se `docs/dev/kommandon.md`). Menyerna visar bara det som hör till läget. I smala fönster samlas de under hamburgerknappen som undermenyer. Inställningarna som låg i menyn (språk, säkerhetskopiering, uppdateringar, spara automatiskt) ligger i *Inställningar*.
- **Kommandopalett** (`Ctrl+Skift+P`): suddig sökning bland alla kommandon, på svenska och engelska, med kategori och kortkommando. Senast använda kommandon ligger överst, kommandon som inte går att köra nu är nedtonade, och Enter kör.
- **Inställningar** (`Ctrl+,`): ett eget fönster med Allmänt (språk, tema, textstorlek, uppdateringar, spara automatiskt, säkerhetskopior), Redigerare, Dokument (författarnamn, stavning, grammatik och server), PDF och Avancerat (med *Återställ alla inställningar*).
- **Nya kommandon i menyerna.** *Exportera som webbsida…* och *Exportera som e-bok…*, *Nästa/Föregående träff*, *Visa som dokument*, sidhuvud och sidfot, formatering och granskning samt PDF-åtgärder (rotera sidan, lägg till PDF, OCR, öppna i extern visare) finns nu som menyval.

**Dokumentläget**

- **Spårade stycketecken och formateringsändringar.** Med *Spåra ändringar* på spåras nu också Enter, hopslagning av stycken (Backsteg i styckets början, Delete i slutet) och formatering. Ett infogat stycketecken ritas som ¶ i författarens färg och ett raderat som genomstruket ¶; *Godkänn* behåller brytningen respektive slår ihop styckena, *Avvisa* gör tvärtom. Fet, kursiv, understruken, genomstruken, typsnitt, storlek, färg, markering, rubrik och listtyp, citat och justering sparar det gamla formatet: ändringen får ett streck i vänstermarginalen, teckenformat en prickad understrykning, och pekaren visar *Formaterat: Fet (Ada)*. *Godkänn* behåller det nya formatet, *Avvisa* sätter tillbaka det gamla. Allt läses och skrivs som riktiga Word-ändringar (`w:rPr/w:ins` och `w:del` i `w:pPr`, `w:rPrChange`, `w:pPrChange`); spårade formateringsändringar i Word-filer tappas inte längre när ett stycke skrivs om.
- **Spårade ändringar och kommentarer i `.odt`.** Ändringar (`text:tracked-changes`: infogningar och raderingar med författare och datum) och kommentarer (`office:annotation`, med svar och löst-markering från LibreOffice) visas nu i dokumentläget som i `.docx` och sparas tillbaka som riktiga LibreOffice-ändringar och kommentarer, i stället för att godkännas respektive tas bort. Formatändringar (`text:format-change`) kan inte visas och rapporteras som borttagna.
- **Ekvationer med LaTeX-syntax.** `$…$` (löpande) och `$$…$$` (egen rad, centrerad) visas som riktigt satt matematik i dokumentläget och i Markdown-förhandsvisningen: upphöjt och nedsänkt, bråk, rötter med index, summa/integral/produkt med gränser, `\left( \right)`, grekiska och vanliga symboler, `\text`, `\mathbf`/`\mathbb`/`\mathcal`, matriser, `cases`, `aligned`, accenter och mellanrum. Källtexten visas när markören står på raden. *Infoga → Ekvation* lägger in `$$ $$` med markören inuti, och en palett (bråk, rot, summa, integral, gräns, parenteser, matris …) sätter in färdiga strukturer. Formlerna följer med till `.docx` (Office Math, `m:oMath`; formler i Word-dokument läses tillbaka som LaTeX), `.odt` (formelobjekt med MathML och ersättningsbild), PDF (ritade glyfer), HTML och EPUB (MathML). Typsnittet är STIX Two Math (OFL).
- **Mallar för nya dokument.** *Ny från mall…* (`Ctrl+Shift+N`) öppnar ett galleri med förhandsvisningskort och kategorier: brev (formellt och personligt), rapport med innehållsförteckning och sidnummer, mötesprotokoll, dagordning, CV (två varianter), faktura, uppsats eller PM med egen titelsida, anteckningar och README (Markdown) samt tomt A4 och Letter. Mallarna finns på svenska och engelska efter gränssnittsspråket. Platshållare som `{{namn}}` och `{{datum}}` fylls i i ett litet formulär (dagens datum och författarnamnet är förifyllda); tomma blir gulmarkerade platshållare. Fliken är namnlös, i dokumentläget (sparas som .docx) eller i textvyn för Markdown-mallar. *Spara som mall…* sparar det aktuella dokumentet under *Mina mallar* (med byt namn och ta bort), och en .docx eller .odt kan importeras som mall.
- **Autokorrigering medan du skriver.** I dokumentläget (och, om du väljer det i *Verktyg → Autokorrigering…*, i Markdown och vanlig text, aldrig i kodfiler) rättas texten när du skriver mellanslag, skiljetecken eller Enter: typografiska citattecken efter språk (svenska ”…” och ’, engelska “…” och ‘…’), `--` till – och `---` till —, `...` till …, `(c)` till ©, stor bokstav efter punkt, frågetecken och utropstecken och i början av stycken (inte efter förkortningar som t.ex., bl.a. och e.g., inte i tal som 3.5 eller i webbadresser), två inledande versaler (SVerige blir Sverige), vanliga svenska och engelska stavfel och egna ersättningar (till exempel `mvh` till *Med vänliga hälsningar*). Markdown-syntax, kod, länkar och kommentarer lämnas orörda. Ctrl+Z direkt efter en rättelse tar bara tillbaka rättelsen. Varje regel kan slås av i dialogen, och språket följer stavningskontrollen.
- **Dokumentläge, stora dokument.** Efter en ändring i ett dokument på hundratals sidor tolkas bara stycket med ändringen om (med kontroll mot resten), och tabeller, bilder och typsnitt återanvänds: skrivning på sida 1500 av 2000 gick från ~740 till ~300 ms per bildruta. Rubrikträdet ritar bara synliga rader när det finns tusentals rubriker.

**PDF**

- **Redigera och ta bort befintliga PDF-anteckningar.** Anteckningar som redan finns i filen (markering, understruken, genomstruken, vågig, lapp, textruta, penna, rektangel, ellips, linje) kan väljas med ett klick i *Anteckna*-läget: en ruta visar författare och datum och låter dig ändra kommentaren och färgen eller *Ta bort*. Pekaren över en anteckning visar författare och text, också utanför Anteckna-läget. Ändringarna är osparade tills du sparar (Ctrl+S, *Spara kopia*) och kan ångras (*Ångra*, Ctrl+Z). Utseendet ritas om för det Kladd kan rita; övriga typer behåller sitt utseende och får bara ny text. Fliken *Kommentarer* i sidopanelen listar alla anteckningar med sida, författare och text, och ett klick hoppar dit.
- **Signerade PDF:er.** Egenskapsdialogen visar *Signerad: ja (n signaturer)*, och att ordna sidor i en signerad PDF frågar först (*Att ordna sidor tar bort signaturerna*). Sparningen är inkrementell, så signaturerna består (se Ändrat).
- **PDF med tusentals sidor.** Sökningen släpper textlagret för sidor långt från vyn: minnet vid sökning i 1 800 sidor föll från ~360 till ~90 MB. Träffnumret räknas utan att bygga hela träfflistan.

**Redigeraren och kod**

- **Git i arbetsmappen.** Är den öppna mappen ett git-arkiv visar filträdet status per fil (M, A, U, D, konflikt; mappar med ändringar markeras) och statusraden grenen med ↑/↓ mot fjärrgrenen, uppdaterat när fönstret får fokus och efter sparning. Marginalen i textläget markerar tillagda, ändrade och borttagna rader mot HEAD; ett klick på en markör visar de gamla raderna med *Återställ ändringen*. *Jämför med HEAD…* öppnar en diff och *Visa blame* visar författare, ålder och meddelande per rad. Källkontrollpanelen (`Ctrl+Shift+G`) har köa/ta ur kön, förkasta, incheckningsmeddelande med *Checka in* (`Ctrl+Enter`) och *Checka in och pusha*, hämta/pulla/pusha, byte och skapande av gren och de 50 senaste incheckningarna; fel visas som notiser med git:s meddelande. Använder programmet `git` i bakgrunden (inga nya beroenden); saknas det döljs allt Git-relaterat.
- **Språkservrar (LSP) för kodfiler.** Kladd startar rust-analyzer, pyright-langserver/pylsp, typescript-language-server, gopls eller clangd (de som finns på PATH) när en kodfil öppnas och visar fel och varningar som krumelurer, ikoner i marginalen och hovertext, med en *Problem*-panel där ett klick hoppar till platsen. Komplettering (efter utlösartecken eller `Ctrl+Space`), hover, gå till definition (`F12`, `Ctrl+klick`, även till andra filer), referenser (`Skift+F12`), byt namn (`F2`), formatera (`Skift+Alt+F`) och signaturhjälp. Ändringar skickas inkrementellt med rätt UTF-16-positioner, och statusraden visar serverns förlopp (*rust-analyzer: indexerar…*). *Språkservrar…* i menyn ställer in kommandon per språk.
- **Vim-läge i textläget.** Ny inställning *Visa → Vim-läge* (av som standard; inte i dokumentläget eller PDF) med lägena normal, infoga, markera (tecken, rad, block), ersätt och kommandorad. Normalläget har antal, rörelser (`h j k l w W b B e E ge 0 ^ $ gg G f F t T ; , % { } H M L Ctrl+d Ctrl+u`), operatorer (`d c y > < gu gU ~`) med rörelser och textobjekt (`iw aw iW ip ap i" a" i' i( a( i[ i{ it`), `x X s S r J p P u Ctrl+r . o O a A i I`, register (`"a`–`"z`, `"+`), markeringar, sökning (`/ ? n N * #`) och makron (`q`, `@`). Kommandoraden har `:w :q :wq :x :q! :e`, `:s`/`:%s` (Rusts regex), `:<rad>`, `:set nu/nonu/wrap/nowrap`, `:noh` och `:sp`/`:vs`. Blockmarkör, läge (`-- INFOGA --`) och väntande kommando visas i statusraden. Logiken är en ren tillståndsmaskin (`src/vim.rs`) med många enhetstester.
- **Tabellvy för csv och tsv.** `.csv` och `.tsv` (och annan avgränsad text via *Tabell | Text* i statusraden) visas som ett rutnät med fast rubrikrad och radnummer, virtuell rullning för stora filer, automatiska och justerbara kolumnbredder. Avgränsare, citattecken och rubrikrad gissas och kan ändras per fil. Cellredigering (dubbelklick, F2, skriv; Enter, Tab, piltangenter), markering med Skift/Ctrl, kopiera och klistra in som TSV, infoga och ta bort rader och kolumner, sortering (tal, naturlig ordning, text med svensk ordning), radfilter, sök med markerade celler och statistik för markeringen i statusraden (antal, summa, medel, min, max, även `1 234,5`). Ändringarna är textändringar i dokumentet med ångra och gör om, och orörda rader, avgränsare, citat, radslut och avslutande radbrytning bevaras.
- **Stora filer.** Textfiler från 5 MB eller 200 000 rader (100 MB-loggar, kodfiler med en miljon rader) öppnas i en virtualiserad vy som bara ritar det som syns: bildrutan går på ~0,03 ms i stället för sekunder, minnet är ungefär filens storlek (förut ~145 gånger) och att skriva i slutet av en 100 MB-fil är omedelbart. Filer över 20 MB läses in i en bakgrundstråd med förloppsruta, utan extra kopior av texten. Radbrytning och färgning är avstängda i den vyn (en notis säger det), sökning görs i bitar utan att gränssnittet fryser och slutar vid 50 000 träffar (*N+ träffar*), *Ersätt* ändrar bara träffens rad och kan ångras, och ordräkning, rubrikträd, autosparning och säkerhetskopiering undviker att läsa om hela texten varje bildruta. Vanliga filer beter sig precis som förut.

**Kvalitet**

- **Automatiska gränssnittstester.** Hela appen körs i `egui_kittest` (utan fönster och GPU) med simulerade tangenter och klick: öppna, skriva, ångra och spara, Markdown-förhandsvisning, sök och ersätt med regex, flikar, delad vy, Word (fetstil, formatmall, sidhuvud, spårade ändringar), PDF (sidor, zoom, sök, Redigera, markering som sparas), filträd, snabböppna, språkbyte, flera markörer och versionshistorik. De ingår i `cargo test` i CI på Linux, Windows och macOS. Se `docs/dev/gransnittstester.md`.
- **Korpus av riktiga dokument i testerna.** `tests/corpus/` har 440 `.docx`, `.odt` och `.pdf` från python-docx, mammoth.js, docx4j, Apache POI, odfpy, LibreOffice och pypdf (källor och licenser i `SOURCES.md`) samt egna LibreOffice-dokument med sammanslagna celler, fotnoter, kommentarer, spårade ändringar, sidhuvuden, avsnitt, bilder, listor, innehållsförteckning och formler. Varje fil öppnas, sparas oförändrad och efter en liten ändring, och i en extra Linux-körning med LibreOffice kontrolleras att de sparade filerna går att öppna där.
- **Rättat.** Ett stycke i ett Word-dokument som börjar med en tabb blev ett kodblock vid sparning; tabben skrivs nu som `&#9;`.

### Ändrat

- **PDF sparas som inkrementell uppdatering.** Anteckningar och ifyllda formulärfält läggs sist i den ursprungliga filen (nya objekt, nytt xref-avsnitt och trailer med `/Prev`) i stället för att hela filen skrivs om, så digitala signaturer fortsätter gälla och filen struktureras inte om. Krypterade PDF:er skrivs som förut om avkrypterade.
- **Gränssnittstesterna körs en i taget.**

## [2.0.0] – 2026-11-13

### Nytt

- **Anteckningar i PDF-visaren.** *Anteckna* i verktygsraden ger Markera (gul, grön, blå, rosa), Understruken, Genomstruken, Anteckning (lapp med text), Textruta och Penna (färg och bredd). Markera text med musen för att lägga på anteckningen när du släpper knappen; högerklick tar bort en ny anteckning och *Ångra* tar bort den senaste. Fliken är ändrad tills du sparar: Ctrl+S skriver anteckningarna som riktiga PDF-anteckningar med utseende i filen, och *Spara kopia* tar med dem i kopian. Krypterade PDF:er sparas utan lösenordsskydd, och Kladd säger till.
- **Formulär i PDF-visaren.** PDF:er med ifyllbara fält (AcroForm) får raden *Det här dokumentet har formulärfält* med *Markera fält*. Klicka på ett fält för att skriva text, kryssa, välja alternativ eller ur listor; Tab och Skift+Tab går mellan fälten. Ctrl+S sparar värdena i PDF:en (nya utseenden för fälten, `/NeedAppearances`), fliken märks som ändrad och stängning frågar. *Spara kopia* och utskrift tar med ifyllda värden.
- **Textigenkänning (OCR) i PDF-visaren.** Har datorn Tesseract installerat renderar *Visa → Känn igen text (OCR)* skannade sidor i 300 dpi i bakgrunden, med förloppet i statusraden, så att texten blir sökbar, markerbar och kopierbar. Ett engångsmeddelande visas när dokumentet har skannade sidor, och *Redigera* kan känna igen texten först och lägga den som stycken i dokumentet. Saknas Tesseract förklarar menyvalet hur det installeras.
- **Ordna PDF-sidor.** I miniatyrpanelen markerar du sidor (klick, Ctrl+klick, Skift+klick) och drar dem till ny plats. Högerklicksmenyn roterar åt höger eller vänster, tar bort (Delete), extraherar till ny PDF och infogar en annan PDF före eller efter; *Slå ihop med PDF…* i Visa-menyn lägger till en fil sist. Ändringarna syns direkt och kan ångras och göras om (Ctrl+Z, Ctrl+Shift+Z). Fliken markeras som ändrad, och *Spara kopia* sparar under nytt namn.
- **Panorering och markering i PDF-visaren.** Mellanslag+dra panorerar som handverktyget, och Mellanslag bläddrar bara när man inte dragit. Dra efter dubbel- eller trippelklick utvidgar markeringen med hela ord respektive rader.

- **Spåra ändringar och kommentarer.** Spårade ändringar (`w:ins`, `w:del`, flyttar) och kommentarer med svar och löst-markering i `.docx` visas i stället för att tas bort: infogad text understryks och raderad text stryks i författarens färg, kommenterade områden är markerade och kommentarerna står i ett fält till höger om sidan (författare, datum, text, svar). Ett klick på en kommentar markerar dess område. Menyn *Granska* i formateringsraden har *Spåra ändringar* (`Ctrl+Skift+E`), *Godkänn/Avvisa ändring*, *Godkänn/Avvisa alla*, föregående/nästa ändring, *Ny kommentar* (`Ctrl+Alt+M`), *Svara*, *Ta bort kommentar*, *Visa kommentarer* och *Ditt namn…* (standard är användarnamnet). Ändringar och kommentarer sparas tillbaka som riktiga Word-ändringar och kommentarer, och ett orört dokument sparas som förut. I `.odt` godkänns ändringarna och kommentarerna utelämnas; PDF, HTML och förhandsvisningen visar den godkända texten.
- **Redigerbara sidhuvuden och sidfötter.** Dubbelklicka på sidhuvudet eller sidfoten på en sida i dokumentläget för att redigera dem i tre fält (vänster, mitten, höger) med knappar för Sidnummer, Antal sidor, Datum, Titel och Filnamn, och rutan *Annorlunda första sida*. Nya fält `{datum}`, `{titel}` och `{fil}`, delning med `|` (`header: "Rapport|{titel}|{sida}"`) och `header-first`/`footer-first` fungerar i dokumentläget, PDF, utskrift, `.docx` och `.odt`. Sidhuvud och sidfot med fält, flikar och annorlunda första sida läses också från Word- och LibreOffice-dokument.
- **Ändra formatmall.** Knappen *Ändra formatmall…* bredvid stilvalet (eller högerklick på stilvalet) öppnar en dialog för Brödtext, Rubrik 1–6 och Citat med förhandsvisning: typsnitt, storlek, fet, kursiv, färg, justering, luft före och efter, radavstånd och indrag av första raden. Ändringarna gäller hela dokumentet direkt. *Uppdatera från markering* tar utseendet på texten vid markören och *Återställ* tar bort mallens ändringar. De sparas i grundformatskommentaren (`h1: "Calibri Light" 16pt bold #2F5496 space-before 12pt`). Word- och OpenDocument-filers formatmallar läses in och ändringar skrivs tillbaka till `styles.xml`; PDF, HTML och EPUB följer dem.
- **Avsnitt i dokumentläget.** En del av dokumentet kan få liggande sidor, egna marginaler eller flera spalter, som sektioner i Word. *Infoga* har *Avsnittsbrytning (ny sida)*, *Liggande sidor härifrån*, *Spalter…* (1–3 spalter och mellanrum) och *Spaltbrytning*. Avsnittet börjar vid raden `<!-- kladd: avsnitt; page: A4 landscape; columns: 2; column-gap: 12mm -->` (visas bara när markören står på den); `type: continuous` låter det börja på samma sida. Text, bilder och tabeller flyter i spalter och anpassas till spaltens bredd, och sidnumren löper över avsnitten. Avsnitten läses och skrivs som `w:sectPr` i `.docx` (orörda avsnitt sparas som de var), som sidmallar och `text:section` i `.odt`, och ger rätt sidstorlek och spalter i PDF och utskrift.
- **Grammatik- och stilkontroll** i dokumentläget med LanguageTool. Av som standard: slå på den i statusfältet (*Grammatik*) eller i stavningsmenyn. Grammatikfel får blå och stilråd bärnstensgul våglinje; håll pekaren över eller högerklicka för förklaring, upp till fem förslag, *Ignorera* och *Ignorera regeln*. Bara synlig text skickas (utan Markdown-syntax), bara ändrade stycken, en stund efter att du slutat skriva, och inom publika serverns gränser. Första gången med api.languagetool.org förklaras att texten skickas dit; en egen server (till exempel `http://localhost:8081`) går också att ange.

- **Arbetsmapp.** *Öppna mapp…* (eller `Ctrl+Shift+E`) visar ett filträd i en sidopanel: mappar först, dolda filer och `.git`, `target` och `node_modules` dolda (knappen *Dolda* visar dem), ikoner efter filtyp, och ett klick öppnar filen i en flik. Högerklick ger *Ny fil*, *Ny mapp*, *Byt namn*, *Ta bort* (till papperskorgen) och *Visa i filhanteraren*. Trädet läses om när fönstret får fokus och var tredje sekund, och mappen kommer ihåg till nästa gång.
- **Snabböppna** (`Ctrl+E`, i dokumentläget bara via menyn): suddig sökning på filnamn i arbetsmappen (eller bland senaste filer utan mapp), pilar och Enter.
- **Sök i filer** (`Ctrl+Shift+F`) i arbetsmappen (respekterar `.gitignore`, hoppar över binära och stora filer) eller i alla öppna flikar, med versaler, hela ord och reguljära uttryck. Träffarna visas grupperade per fil med förhandsvisning, sökningen körs i bakgrunden och kan avbrytas, och ett klick öppnar filen och markerar träffen. *Ersätt alla* ersätter över flera filer efter bekräftelse; öppna flikar ändras i redigeraren i stället för på disk.
- **Flera markörer och kolumnmarkering i textläget.** Ctrl+D lägger till nästa förekomst av markeringen, Ctrl+Shift+L markerar alla, Alt+klick lägger till en markör och Alt+Shift+drag (eller mittenknappen) markerar en kolumn. Skrivning, Backsteg/Delete, Enter, Tab, piltangenter (även Home/End och ordhopp), klipp ut/kopiera/klistra in (en rad per markör när antalet stämmer) fungerar på alla markörer, och Ångra tar tillbaka hela steget. Esc återgår till en markör.
- **Delad vy.** *Dela vyn* (Ctrl+\) i huvudmenyn visar två rutor bredvid varandra och *Dela vågrätt* (Ctrl+Skift+\) över varandra. Varje ruta har en egen flikväljare, så den kan visa en annan flik eller samma dokument en gång till med egen markör och rullning (ändringar delas). Dra i skiljelinjen för att ändra storleken (dubbelklick återställer; storleken kommer ihåg). Klicka i en ruta, eller tryck F6, för att göra den aktiv: verktygsrad, sök, kortkommandon och statusrad gäller den aktiva rutan. Fungerar för text, dokumentläge och PDF (samma PDF visas bara i en ruta). Markdownförhandsvisningen döljs i delad vy.
- **Spara automatiskt** (huvudmenyn, av som standard). Ändrade flikar med sökväg sparas till sin egen fil två sekunder efter att skrivandet stannat och när fönstret tappar fokus eller fliken byts. Namnlösa flikar, filer som kräver Spara som, PDF-flikar och format som skulle tappa innehåll (Word/ODT med varning, omvandlade PDF:er) sparas aldrig automatiskt. Har filen ändrats på disk sparas inget, och en notis visas.
- **Versionshistorik.** Varje sparning lagrar en version i Kladds datamapp (högst 50 per fil, gallras till en per timme efter en timme och en per dygn efter ett dygn, äldre än 30 dagar tas bort). *Versionshistorik…* i huvudmenyn visar versionerna med en radvis jämförelse mot aktuell text (för textformat), *Återställ* (går att ångra) och *Öppna som ny flik*.

- **Engelskt gränssnitt.** Ny inställning *Språk / Language* längst ned i huvudmenyn (Automatiskt, Svenska, English; Automatiskt följer systemets språk) som gäller direkt utan omstart. Översättningssystemet (`src/i18n.rs`, tabeller i `src/i18n/`, beskrivet i `docs/dev/oversattning.md`) är nycklat på svensk källtext, så texter som ännu inte översatts visas på svenska. Hela gränssnittet är översatt: menyer, dialoger, notiser, statusrad, dokumentläget, PDF-visaren, granskning, grammatik och stavning, arbetsmapp, uppdatering, historik, utskrift och kommandoradens hjälp. Text som skrivs i filerna (till exempel en innehållsförteckning i en docx) är fortfarande svensk. Huvudmenyn rullar nu om fönstret är för lågt.

- **macOS: öppna filer från Finder.** *Öppna med*, dubbelklick och att dra filer till Dock-ikonen öppnar nu filerna i Kladd (som flikar i det öppna fönstret). Kladd tar emot Apple-händelsen "öppna dokument" och deklarerar `.txt`, `.md`, `.docx`, `.odt` och `.pdf` som dokumenttyper med låg rangordning, så ingen filtyp byter program av sig själv.
- **Uppdateringskontroll.** Kladd frågar högst en gång per dygn GitHub efter en nyare version och visar ett diskret fält med *Visa nyheter* (utgåvans anteckningar som Markdown), *Hämta och installera*, *Senare* och *Hoppa över*. *Sök efter uppdateringar* i huvudmenyn kontrollerar direkt, och *Sök efter uppdateringar automatiskt* stänger av den dagliga kontrollen. Windows hämtar installationsprogrammet och kör det när Kladd avslutas, macOS öppnar skivavbilden, .deb-installationer öppnar paketet (med kommandot att kopiera) och AppImage byter ut sig själv och startar om. Inget körs utan att du klickar.
- **AppImage för Linux.** Varje release innehåller nu `Kladd-<version>-x86_64.AppImage` som körs på de flesta distributioner utan installation.
- **Valfri kodsignering i release-bygget.** Windows (signtool) och macOS (Developer ID, notarisering) signeras om certifikat finns som hemligheter i repot; annars byggs paketen som förut. Beskrivet i `docs/dev/signering.md`.
- **Flatpak-utkast.** Ett utkast till Flatpak-paket finns i `packaging/flatpak/`.

### Ändrat

- **Ctrl+Shift+L och Ctrl+Shift+K.** Ctrl+Shift+L markerar alla förekomster i textläget, och temat byts nu med Ctrl+Shift+K. I dokument- och PDF-läget gäller Ctrl+Shift+L fortfarande temat.
- **Ctrl+Shift+E.** Slår på spårade ändringar i dokumentläget och visar mappträdet i övriga lägen.
- **Ctrl+S i PDF-flikar.** Sparar över originalet när det finns osparade sidändringar, anteckningar eller formulärvärden. Annars används *Spara kopia*.
- **Lösenord i PDF.** Sparade PDF:er med sådana ändringar sparas utan lösenordsskydd.
- **Datum i återställningsdialogen.** Äldre datum visas som ÅÅÅÅ-MM-DD.

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
