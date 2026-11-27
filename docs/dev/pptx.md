# PowerPoint (.pptx) i Kladd

Första omgången: läsning, arv, ritmotor, jämförelse mot PowerPoint och en enkel visning. Allt ligger i
`src/pptx/`. Presentationen kan inte ändras eller sparas än.

## Arkitektur

En ritväg för allt:

```
.pptx ──package.rs──▶ Package (råa delar + relationer)
        model.rs ───▶ Deck (bilder, layouter, mallar, teman, typsnitt; formträden som xml::Node)
        render.rs ──▶ Scene (visningslista)  ◀── inherit.rs, richtext.rs, text.rs, geom/, color.rs,
                         │                       tables.rs, smartart.rs, charts.rs, media.rs
                         ├─ raster.rs (tiny-skia): skärm, miniatyrer, bildspel, jämförelseprov
                         └─ pdf.rs (krilla): PDF med riktig text
```

- Ingen text läggs ut i egui och ingen SVG-`<text>` används: radbrytning, formning (rustybuzz) och placering
  görs en gång i `text.rs`, och `Scene` har färdigplacerade glyfer. Skärm och PDF ritar samma lista.
- `Scene` är i punkter, origo uppe till vänster. Banor och övertoningar ligger i bildkoordinater; bilder och
  glyfkörningar har en egen affin transform.
- XML läses till ett litet träd (`xml.rs`) med normaliserade prefix (`p:`, `a:`, `r:` … efter namnrymdens URI;
  SmartArts `dsp:` blir `p:` så att samma formritning används). `mc:AlternateContent` löses vid tolkningen:
  `mc:Choice` väljs bara för `p14`/`p15`, annars `mc:Fallback` (formler med `a14` ger reservbilden).

## Moduler och gränssnitt

| Modul | Gränssnitt |
|---|---|
| `package.rs` | `Package::open(&[u8])`, `read(part)`, `xml(part)`, `rels(part) -> Vec<Rel>`; `parts` behåller alla delar orörda (för källbevarande redigering). |
| `model.rs` | `Deck::open(&[u8]) -> Result<Deck, String>`; `slides`/`layouts`/`masters`/`themes`, `size_pt()`, `first_slide_num`, `default_text_style`, `color_ctx(..)`, `image(part)`, `svg(part)`, `drawing(part)` (SmartArt), `features(i)` (diagram/EMF/SmartArt/tabell), `slide_number(i)`. |
| `inherit.rs` | `Sheets::for_slide(deck, i)` (bild, layout, mall, tema, färger), `shape_chain` (form → layoutens → mallens platshållare, `type`/`idx`), `list_styles`/`level_chain` (nivåstilar i prioritetsordning), `style_fill`/`style_line`/`style_font` (`p:style`), `theme_font` (`+mj-lt` …), `background`. |
| `color.rs` | `resolve(node, &ColorCtx, phClr) -> Option<Rgba>`, `apply_mods`; alla färgtyper och modifierare i dokumentordning (tint/shade i linjärt RGB, lum/sat/hue i HSL, som POI). |
| `geom/` | `preset(name, avLst, w, h)`, `evaluate(custGeom, ..)` → `Geometry { paths: Vec<SubPath{path, fill, stroke}>, text_rect }`, `arc_to`. |
| `richtext.rs` | `body_props(chain)`, `TextEnv { sheets, lists, font_ref, font_color, font_scale, slide_no }.paragraphs(txBody)`, `frame(env, body, bp)`, `auto_number`. |
| `text.rs` | Textmotorn: `layout(&Frame, w, h, &FontBook) -> Laid { items, height }`, `transform_items`. Indata: `Frame { paras: Vec<Para>, insets, anchor, wrap, ln_spc_reduction, columns }`. |
| `fonts.rs` | `FontBook::resolve(family, bold, italic) -> Resolved { face, x_scale, fake_bold, fake_italic, symbol_map }`, `fallback(char, face)`, `decode_embedded`, `symbol_to_unicode`, `system_db()`. |
| `render.rs` | `slide_scene(deck, i) -> Scene`; `Builder` med `tree`, `shape`, `picture`, `graphic_frame`, `paint`, `place_text`, `draw_blip`, `placeholder_box`, `rels(owner)` (används av hookarna). |
| `tables.rs` / `smartart.rs` / `charts.rs` / `media.rs` | Hookar: `draw_table(b, tbl, frame, owner)`, `draw_diagram(..)`, `draw_chart(..)`, `metafile(b, part, frame)`. |
| `raster.rs` | `render(&Scene, w, h) -> Pixmap`, `draw(pixmap, scene, transform)`, `to_rgba`, `glyph_path`. |
| `pdf.rs` | `write(deck, &[slide]) -> Result<Vec<u8>, String>` (ännu inte i menyn). |
| `view.rs` + `glue.rs` | `PptxTab` (flikvyn), `status`; `KladdApp::pptx_mode`/`pptx_page`. |
| `compare.rs` | Jämförelsemåttet och det ignorerade jämförelseprovet. |

## Det som stöds

- **Arv:** platshållare bild → layout (samma `idx`, annars samma typ) → mall (typ; `ctrTitle`→`title`,
  övriga→`body`); `xfrm`, geometri, fyllning, linje, `bodyPr` (attribut för attribut) och nivåstilar ärvs.
  Textstilar per nivå: stycke `pPr` → formens `lstStyle` → layoutens → mallens platshållare → mallens
  `titleStyle`/`bodyStyle`/`otherStyle` → `defaultTextStyle` (formar utan platshållare: `otherStyle`).
  `clrMap` + `clrMapOvr`, `p:style` (`lnRef`, `fillRef` 1–999/1001–, `fontRef`), `useBgFill`, `grpFill`,
  `showMasterSp`, dolda former (`hidden`), dolda bilder (`show="0"`), `firstSlideNum`. Tomma platshållare ritas
  inte (som i bildspel).
- **Former:** alla 187 förinställda former och `custGeom` med guider, `arcTo` (synliga vinklar som POI),
  `fill`-lägen (`darken`/`lighten`…), grupper (`chOff`/`chExt`), rotation och spegling, pilspetsar.
- **Fyllningar:** enfärg, linjär och radiell övertoning, bildfyllning, mönster (som blandfärg), bakgrunder
  (`bg`/`bgRef`).
- **Bilder:** `srcRect`, alfa (`alphaModFix`), gråskala, klippning till formen, SVG via `asvg:svgBlip`, EMF som
  bara bär en bitmapp.
- **Text:** insättningar, `anchor`, `anchorCtr`, `wrap`, `vert`/`vert270`, `rot`, spalter, nivåer, `marL`/
  `indent`/`marR`, tabbar, `spcBef`/`spcAft`/`lnSpc` (procent och punkter), lagrad `normAutofit`
  (`fontScale` på storlekarna, `lnSpcReduction` på radavståndet; räknas inte om), punkter och numrering
  (`buChar`, alla `buAutoNum`-scheman med räknare per nivå, `buFont`/`buClr`/`buSzPct`/`buSzPts`), höger-till-
  vänster (förenklad bidi), östasiatiska och komplexa skrifter (`ea`/`cs` och skriftens typsnitt ur temat),
  fält (`slidenum`), hyperlänkar (temats länkfärg och understrykning), versaler, upphöjt, teckenavstånd,
  understrykning, genomstrykning, markering.
- **Tabeller (grund):** rutnät, sammanslagna celler, cellmarginaler och förankring, cellernas fyllningar och
  kanter, tabellformat ur `tableStyles.xml` (`wholeTbl`, band, första/sista rad och kolumn), tabeller från
  höger till vänster.
- **SmartArt:** den sparade ritdelen (`dsp:`) ritas som en grupp, med `txXfrm` för texten.
- **Diagram, EMF/WMF med vektorinnehåll:** grå ram med etikett (hookar för nästa omgång).

## Radhöjd (kalibrerat)

Mätt med `pdftotext -bbox` i PowerPoints PDF (`cppcon18-say-nothing` bild 18, Calibri 20–28 pt, 90 %;
`owasp-top10-2017-en` bild 23, Arial 9,5 pt, 100 %):

- En rad vid 100 % är **1,2 × teckenstorleken** för alla typsnitt (inte typsnittets egna mått), delad mellan
  upp- och nedstapel i förhållandet winAscent:winDescent.
- Vid lägre procent tas skillnaden bort från **uppstapeln**; nedstapeln är oförändrad. Avståndet mellan två
  baslinjer = föregående rads nedstapel + radens minskade uppstapel (+ `spcBef`).
- `spcBef` gäller inte första stycket i en textruta.
- `lnSpcReduction` dras från procenten (90 % − 10 % = 80 %).

## Typsnitt

Ordning: inbäddat i filen → installerat med exakt namn → symboltypsnitt (Wingdings, Symbol …) översätts
till Unicode → Carlito/Caladea (Calibri, Calibri Light, Cambria) → ersättningstabellen → generiskt →
Carlito. Ersättarna har en **vågrät skalning** så att radbrytningen blir som med originalet: originalets
medelbredd (vägd efter engelsk bokstavsfrekvens) delad med ersättarens, mätt ur breddtabellerna
(`/Widths`) i PowerPoints PDF-exporter i referensmaterialet (t.ex. Segoe UI 0,440 em ≈ Liberation Sans
0,441; Segoe UI Semibold 0,454 → Liberation Sans Bold × 0,953; Consolas 0,550 → Liberation Mono × 0,9165;
Aptos 0,425 → Liberation Sans × 0,964; Century Gothic 0,483 → × 1,094 för bokstäver, men siffrorna är lika
breda som Arials, så 1,05 används). Skalningen är en medelvärdesapproximation: enskilda tecken kan bli för
breda eller smala. Mätningen gjordes med pypdf
(`/Widths` för WinAnsi-typsnitten) och fontTools (`hmtx` för ersättarna); bara kvoterna står i `fonts.rs`.
Obs: kvoterna och radhöjden är kalibrerade mot samma PowerPoint-PDF:er som jämförelseprovet mäter.

- **Aptos** har ingen fri, metriskt lik ersättare; Liberation Sans med skalning används.
- **Segoe UI** ersätts av Selawik (Microsoft, OFL) om det är installerat, annars Liberation Sans. Selawik
  följer inte med Kladd.
- **Calibri Light** och **Segoe UI Light** ritas i normal vikt (för kraftigt men med rätt bredd).
- **Inbäddade typsnitt:** okomprimerad EOT (även XOR-krypterad), rå TrueType och GUID-obfuskering läses.
  PowerPoints `.fntdata` är nästan alltid **MicroType Express-komprimerade** (flagga 0x4); de kan inte läsas än
  och ersättningstabellen används (notis vid öppning).
- Tecken som typsnittet saknar tas ur en reservlista (CJK: IPAPGothic/Noto CJK/WenQuanYi; annars DejaVu Sans …).

## Prov

- **Enhetsprov** i varje modul och `src/pptx/tests.rs`: egna minimala .pptx byggs i provet (zip i minnet) för
  platshållararv (läge, storlek, färg, nivåer, punkter), `clrMapOvr`, tomma platshållare, `firstSlideNum`
  och dolda bilder, stilmatrisen, rotation, grupper med barnkoordinater, `mc:AlternateContent` + lagrad
  autoanpassning, radbrytning och radavstånd, trasiga filer (fel, ingen panik) och PDF (sidor och text).
- **Korpus** (`tests/corpus/pptx/`, MIT, python-pptx): `corpus_pptx_miniatyrer` renderar bild 1 i storleken av
  PowerPoints `docProps/thumbnail.jpeg` och jämför; värdet i `manifest.toml` får inte sjunka mer än 0,03.
- **Jämförelseprovet** (`pptx::compare::powerpoint_reference`, ignorerat):

  ```
  KLADD_PPTX_REFS=…/scratchpad/pptx/downloads KLADD_PPTX_OUT=/tmp/jmf \
    cargo test pptx::compare::powerpoint_reference -- --ignored --nocapture
  ```

  `KLADD_PPTX_DECKS=a,b` och `KLADD_PPTX_SLIDES=3,5` begränsar urvalet, `KLADD_PPTX_NO_SIDES=1` hoppar över
  bilderna. Mappen per presentation ska ha `deck.pptx`, `ref-NN.png` (PowerPoints PDF i 72 dpi) och gärna
  `lo-NN.png` (LibreOffice). `pageToSlide` läses ur `../decks.json` (dolda bilder saknas i PDF:en).
  Måttet: gråskala, gaussisk oskärpa σ = 1 px, SSIM i 8×8-fönster med steg 4, medel (1,0 = lika). Utdata:
  `table.md`, `scores.csv` och sida-vid-sida-bilder (PowerPoint | Kladd | skillnad i rött).
- Felsökning: `KLADD_PPTX_DUMP=fil.pptx:7 cargo test pptx::tests::dump_scene -- --ignored --nocapture` skriver
  visningslistan; `KLADD_PPTX_THUMBS=mapp` mäter miniatyrerna i en mapp och skriver sida-vid-sida-bilder.

## Resultat mot PowerPoint (15 presentationer, SSIM-medel)

Mätt med jämförelseprovet (1,0 = lika som PowerPoints PDF). "Före" är den första hela ritvägen (arv, former,
text med kalibrerat radavstånd), "efter" är den här versionen (bland annat `useBgFill`, dolda former, SVG-alfa,
bildtransformen, bidi, SmartArts `txXfrm`, tabeller från höger till vänster, uppmätta breddkvoter för
ersättningstypsnitt). LibreOffice 24.2 är renderad i samma behållare och har därför samma typsnittsbrist
(dess reservtypsnitt är Inter); Kladds breddkvoter och radhöjd är kalibrerade mot samma PDF:er som mäts här.

| Presentation | Bilder | Kladd före | Kladd efter | LibreOffice | Kladd efter, utan diagram/EMF | LO utan diagram/EMF | Bilder där Kladd > LO |
|---|---:|---:|---:|---:|---:|---:|---:|
| cncf-overview-ja | 52 | 0.830 | 0.829 | 0.797 | 0.829 | 0.797 | 38/52 |
| cppcon14-parallel-overview | 37 | 0.828 | 0.827 | 0.806 | 0.827 | 0.806 | 20/37 |
| cppcon15-stl-algorithms-mac | 99 | 0.785 | 0.802 | 0.728 | 0.814 | 0.755 | 93/99 |
| cppcon17-refactor-or-die | 13 | 0.980 | 0.981 | 0.965 | 0.981 | 0.965 | 10/13 |
| cppcon18-say-nothing | 35 | 0.956 | 0.956 | 0.912 | 0.956 | 0.912 | 35/35 |
| cppcon19-charconv | 64 | 0.844 | 0.847 | 0.734 | 0.847 | 0.734 | 64/64 |
| cppcon19-macrofree | 70 | 0.858 | 0.922 | 0.861 | 0.922 | 0.861 | 68/70 |
| ms-copilot-workshop | 58 | 0.892 | 0.927 | 0.902 | 0.927 | 0.902 | 43/58 |
| ms-workshop-dlcv | 41 | 0.913 | 0.943 | 0.923 | 0.943 | 0.923 | 34/41 |
| msrc-eko19 | 23 | 0.861 | 0.878 | 0.834 | 0.899 | 0.847 | 22/23 |
| msrc-identity | 34 | 0.928 | 0.928 | 0.912 | 0.928 | 0.912 | 27/34 |
| msrc-win32k | 35 | 0.862 | 0.879 | 0.833 | 0.881 | 0.833 | 33/35 |
| owasp-top10-2017-en | 25 | 0.850 | 0.876 | 0.768 | 0.876 | 0.768 | 23/25 |
| owasp-top10-2017-he | 25 | 0.675 | 0.680 | 0.632 | 0.680 | 0.632 | 13/25 |
| psconf19-pscore | 21 | 0.930 | 0.930 | 0.914 | 0.930 | 0.913 | 16/21 |
| **alla** | 632 | 0.855 | 0.873 | 0.821 | | | 539/632 |

## Licenser

- `src/pptx/geom/presetShapeDefinitions.xml`: Apache POI, Apache-2.0 (`LICENSE-APACHE-POI.txt`, POI:s NOTICE och
  Kladds ändringar i `geom/NOTICE.md`), ursprungligen ECMA-376 del 1. Ändrat: `pie`-textrektangeln
  (`t`/`r` var omkastade) och borttagna blanksteg.
- Utvärderaren, färgmodifierarna (`tint`/`shade` i scRGB) och autoanpassningen följer POI:s beteende
  (Apache-2.0); LibreOffice (MPL) och ONLYOFFICE (AGPL) har bara lästs för beteende, ingen kod är porterad.
- Korpusfilerna: python-pptx, MIT (`tests/corpus/pptx/SOURCES.md`). Referenspresentationerna (CppCon, MSRC,
  OWASP …) ligger **inte** i git; de flesta saknar licens.

## Kvar till nästa omgång

- **Diagram** (`charts.rs`): stapel/linje/cirkel ur `c:numCache`/`c:strCache`; prova mot docx4j:s `p4-*`
  (Apache-2.0, med text ur PowerPoints PDF).
- **EMF/WMF** (`media.rs`): en delmängd (polygoner, linjer, text, bitmappar) efter POI:s `hemf`/`hwmf`.
- **Tabeller** (`tables.rs`): inbyggda tabellformat när GUID:en saknas i filen, alla formatdelar, diagonaler,
  lodrät text i celler.
- **SmartArt** (`smartart.rs`): reserv utan ritdel (datamodellens text); PowerPoint verkar lägga om vissa
  layouter (kugghjulen i `msrc-win32k` bild 4/22 hamnar annorlunda, som i LibreOffice).
- **Effekter:** yttre skugga, glöd, mjuka kanter, 3D-fasning (`sp3d`) ritas inte; `effectRef` och
  `effectStyleLst` läses men används inte.
- **Temats `objectDefaults`** läses in i `Theme` men används inte i arvet (PowerPoint verkar använda dem bara
  för nya former; inte verifierat).
- **Bildfyllning `a:tile`** ritas utsträckt, inte upprepad; bildeffekterna `lum`, `duotone` m.fl. görs inte.
- **Minne:** avkodade bilder hålls i en cache på högst 256 MB per presentation (`ImageCache`) och skalas ner
  till högst 2560 bildpunkter. `cncf-overview-ja` (30 MB, 158 beskurna bilder) ger 495 MB som mest när alla
  bilder renderas i följd (1,3 GB utan taket).
- **MicroType Express** för inbäddade typsnitt: alla `.fntdata` i referensmaterialet (cppcon19-macrofree,
  psconf19-pscore, cncf-overview-ja) är MTX-komprimerade, så inbäddade typsnitt fungerar i praktiken inte än.
  Tunna vikter (Calibri Light, Segoe UI Light).
- **Text:** full bidi (UAX #9), `fontAlgn`, `buBlip`, WordArt (`prstTxWarp`), vertikal östasiatisk text
  (`eaVert` roteras bara), bindestreck.
- **Visning:** miniatyrlist, bildspelsläge, sök, PDF-export i menyn, lägesminne, *Redigera en kopia*,
  anteckningar, `.ppt` via LibreOffice, handboken.
