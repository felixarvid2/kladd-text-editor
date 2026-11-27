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
| `pdf.rs` | `write(deck, &[slide]) -> Result<Vec<u8>, String>` (Exportera som PDF och utskrift, via `view::pdf_of`). |
| `view.rs` + `glue.rs` | `PptxTab` (flikvyn), `status`; `KladdApp::pptx_mode`/`pptx_page`. Se *Visningen* nedan. |
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
- **Fyllningar:** enfärg, linjär och radiell övertoning, bildfyllning, mönster (riktiga mönster, se omgång 2), bakgrunder
  (`bg`/`bgRef`).
- **Bilder:** `srcRect`, alfa (`alphaModFix`), gråskala, klippning till formen, SVG via `asvg:svgBlip`, EMF som
  bara bär en bitmapp.
- **Text:** insättningar, `anchor`, `anchorCtr`, `wrap`, `vert`/`vert270`, `rot`, spalter, nivåer, `marL`/
  `indent`/`marR`, tabbar, `spcBef`/`spcAft`/`lnSpc` (procent och punkter), lagrad `normAutofit`
  (`fontScale` på storlekarna, `lnSpcReduction` på radavståndet; räknas inte om), punkter och numrering
  (`buChar`, alla `buAutoNum`-scheman med räknare per nivå, `buFont`/`buClr`/`buSzPct`/`buSzPts`), höger-till-
  vänster (full bidi, se omgång 2), östasiatiska och komplexa skrifter (`ea`/`cs` och skriftens typsnitt ur temat),
  fält (`slidenum`), hyperlänkar (temats länkfärg och understrykning), versaler, upphöjt, teckenavstånd,
  understrykning, genomstrykning, markering.
- **Tabeller:** (se *Tabeller och SmartArt* nedan för inbyggda format m.m.) rutnät, sammanslagna celler, cellmarginaler och förankring, cellernas fyllningar och
  kanter, tabellformat ur `tableStyles.xml` (`wholeTbl`, band, första/sista rad och kolumn), tabeller från
  höger till vänster.
- **SmartArt:** den sparade ritdelen (`dsp:`) ritas som en grupp, med `txXfrm` för texten.
- **Diagram, EMF/WMF:** ritas, se *Diagram och EMF/WMF* nedan. Okända diagramtyper och chartex (`cx:`) blir en grå
  ram med etikett.

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
- **Calibri Light**, **Segoe UI Light/Semilight** och andra tunna vikter som saknas ritas med ersättaren i
  normal vikt men görs syntetiskt tunnare i rastret (se omgång 2 nedan).
- **Inbäddade typsnitt:** okomprimerad och MicroType Express-komprimerad EOT (även XOR-krypterad), rå TrueType
  och GUID-obfuskering läses (se omgång 2 nedan).
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

## Kvar

Sammanställt efter omgång 3 (detaljer i respektive avsnitt nedan):

- **Redigering:** gjord i omgång 3 (se *Redigering och sparande* nedan). Kvar: text i tabellceller, grupper och
  SmartArt; duplicera diagram/SmartArt som enskilda former; flervalsmarkering; textformat utöver fet/kursiv/
  understruken; `spAutoFit` räknas inte om när texten växer; återställning efter krasch (bara automatisk sparning).
- **Visning:** länkar och åtgärdsknappar i bildspelet, utlösare (`interactiveSeq`), video/ljud, morph som riktig
  forminterpolering, föredragsvyns automatiska val av skärm (se *Bildspel* nedan), markering av text på bilden;
  sökmarkeringen täcker hela glyfkörningen vid ligaturer och höger-till-vänster (klusteroffset per glyf saknas
  i `GlyphRun`).
- **Diagram:** sekundäraxlar, datatabeller, trendlinjer, felstaplar, radar, yt-3D (`surface3DChart`), chartex (`cx:`);
  3D-diagrammen utan referensbilder (stapel/linje/yta) är inte kalibrerade (se *3D i diagram och former*).
- **EMF/WMF:** `ExcludeClipRect`, fler regionoperationer, streckade/mönstrade penslar, `ETO_GLYPH_INDEX`, EMF+.
- **Tabeller:** lodrät text i celler låter inte raden växa; några val är inte prövade mot referensbilder (se
  avsnittet).
- **SmartArt:** layouter som PowerPoint lägger om (kugghjulen i `msrc-win32k` bild 4/22) kräver en layoutmotor.
- **Effekter:** oskärpa i spegling; tunna vikter, inre skugga och mjuka kanter i PDF:en; 3D-text (`sp3d` på
  `a:bodyPr`), perspektiv och extrudering bortom en enkel affin förvrängning (se *3D i diagram och former*).
- **Fyllningar:** `rotWithShape` för linjära övertoningar; stoppordningen i banövertoningar är inte verifierad
  (diagrambakgrunden i `msrc-eko19` blir för ljus); bildeffekterna `lum`, `duotone` m.fl.
- **Text:** `fontAlgn`, `buBlip`, WordArt (`prstTxWarp`), bindestreck, `buChar` med flera tecken (owasp-2010-es).
- **Minne:** avkodade bilder hålls i en cache på högst 256 MB per presentation och skalas ner till högst
  2560 bildpunkter.

## Visningen

Presentationsvyn i appen (`view.rs`, `glue.rs`) och de rena delarna den bygger på. Förebilder: PDF-visaren
(verktygsrad, sidopanel, presentationsläge, sökfält; `pdfview/chrome.rs` delar `tool_btn` m.fl.) och EPUB-vyn
(lägesminne, *Redigera en kopia*).

| Modul | Gränssnitt |
|---|---|
| `view.rs` | `PptxTab { deck, current, find, … }`: `toolbar`, `sidebar` (miniatyrer), `show` (bild för bild eller rullande lista, anteckningspanel), `find_bar`, `handle_keys`; bildspel `start_show`/`show_page`/`show_keys`/`stop_show`; `pdf_bytes`, `begin_export`/`poll_export`; `memory`/`restore`. `Prefs` (miniatyrer, anteckningar, läge, `Zoom`) och `Memory` ligger i inställningarna (`settings.pptx`, `settings.pptx_memory`). Rena hjälpare: `scale_for`, `zoom_step`, `parse_slide_input`, `pdf_of`. |
| `glue.rs` | `KladdApp::pptx_*`: verktygsrad, sidopanel, sökfält, tangenter, helskärm (`pptx_sync_fullscreen`), PDF-export, lägesminne, *Redigera en kopia*. |
| `slideshow.rs` | `Show::new(hidden, start)`, `screen()` (bild, svart/vit skärm, slutskärm), `next`/`prev`/`first`/`last`, `toggle_blank`, `upcoming` (förrendering). |
| `find.rs` | `slide_text(&Scene) -> SlideText` (bildens text ur glyfkörningarna med tecknens lägen), `find_all`, `hits_for`, `hit_rects` (rektanglar i punkter). |
| `notes.rs` | `notes(deck, i)`/`notes_text` (anteckningssidans brödtextplatshållare), `paragraphs(txBody)` (nivå, fet, kursiv), `shapes(tree)`. |
| `copy_md.rs` | `deck_to_markdown(deck) -> (String, bilder)` för *Redigera en kopia*. |

- **Rendering:** en pool med högst två samtidiga renderingar (`render_slide` i en egen tråd, panik ger en vit
  bild). Varje bild önskar en textur av ett slag (bildspel, vy, miniatyr) i en viss bredd; i slutet av bildrutan
  startas önskningarna i den ordningen. Miniatyrernas bredd avrundas uppåt till 32 bildpunkter så att en
  storleksändring av panelen inte renderar om allt. Texturer långt från det synliga släpps.
- **Bildspel:** helskärm via `ViewportCommand::Fullscreen`, som följer fliken (`fullscreen_sent`). Byts aktiv
  flik under bildspelet (en fil öppnas utifrån) avslutas det, så att fönstret inte fastnar i helskärm. Medan
  bildspelet pågår går alla tangenter till det (`handle_shortcuts`), och `ui()` ritar bara bilden.
- **Sök:** registret byggs i en bakgrundstråd när sökfältet öppnas: varje bilds visningslista byggs
  (`render::slide_scene`, utan raster) och glyfkörningarnas text och lägen sparas, plus anteckningarna. Träffarna
  visas efter hand. Körningar på samma baslinje som fortsätter där den förra slutade fogas ihop utan mellanrum,
  andra skiljs med ett blanksteg (så att en fras över en radbrytning hittas). Tecken och glyfer avbildas ett till
  ett när antalen stämmer och glyferna går åt höger; annars (ligaturer, höger till vänster) markeras hela
  körningen. **Önskemål till ritmotorn:** klusteroffset per glyf i `GlyphRun` (rustybuzz har dem) skulle ge
  exakta träffrektanglar även för ligaturer och bidi.
- **Redigera en kopia:** rubrik per bild ur titelplatshållaren (`title`/`ctrTitle`, annars "Bild N"); brödtext-
  platshållare (`body`/`obj`/bara `idx`) ger punktlistor med nivåer (fyra blanksteg per nivå, som Word-importen),
  `buAutoNum` numrerade listor och `buNone` vanliga stycken; textrutor ger stycken; tabeller blir Markdown-tabeller
  (första raden rubrikrad, sammanslagna celler tomma, `<br>` för radbrytningar); bilder läggs in med
  `bild:`-adresser (PNG/JPEG/GIF/BMP/WebP/SVG, inte EMF/WMF); SmartArt ger datamodellens text som punktlista;
  sidfot, datum och bildnummer hoppas över; anteckningarna blir ett citat. Provet sparar kopian som .docx och
  läser tillbaka den.
- **Filformat:** `.ppsx`, `.potx`, `.pptm`, `.ppsm`, `.potm` öppnas som `.pptx` (`pptx::EXTENSIONS`; ingen
  kontroll av huvuddelens innehållstyp, makron läses inte). `.ppt` går via `legacy.rs` (`target_ext` → `pptx`)
  och `Document::from_converted` öppnar resultatet i presentationsvyn.
- **Utskrift:** `print.rs` skriver ut `PptxTab::pdf_bytes()` (samma PDF som exporten, utan dolda bilder).
- **Paketering:** `src/pptx/geom/LICENSE-APACHE-POI.txt` och `NOTICE.md` följer med som `POI-LICENSE` och
  `POI-NOTICE.md` bredvid typsnittslicenserna (deb, flatpak, AppImage, Windows, macOS). Kladd har ingen
  om-/licensdialog (*Om Kladd* är en notis), så de listas inte i appen.
- **Prov:** `view_tests.rs` (egen presentation med anteckningar, tabell, bild och dold bild byggd i provet),
  enhetsprov i `slideshow.rs`, `find.rs`, `notes.rs` och `view.rs`. Det ignorerade
  `soffice_konverterar_ppt` gör en `.ppt` av en korpusfil med LibreOffice och öppnar den.
- **Kvar:** bildövergångar och animeringar (bildspelet visar slutläget direkt), föredragsvy med nästa bild och
  timer, länkar och åtgärdsknappar i bildspelet, video/ljud, hoppa till bildnummer med siffror + Enter i
  bildspelet, markering och kopiering av text på bilden, engelsk handbok (handboken finns bara på svenska).
- **Visning:** miniatyrlist, bildspelsläge, sök, PDF-export i menyn, lägesminne, *Redigera en kopia*,
  anteckningar, `.ppt` via LibreOffice, handboken.

## Diagram och EMF/WMF (omgång 2)

### Diagram

`charts.rs` (hooken) → `chart_model.rs` (`c:chartSpace` → `Chart`: plottar, serier ur `c:numCache`/`c:strCache`/
`c:numLit`/`c:strLit`, axlar, förklaring, titel, `c:manualLayout`, `c:view3D`) → `chart_draw.rs` (layout och
ritning till `Scene`) med `chart_fmt.rs` (Excels talformat inklusive datum). Formateringen (`c:spPr`, `c:txPr`,
`c:rich`) går genom `Builder::paint` och `TextEnv::run_style`, så temafärger, övertoningar och typsnitt blir som
i formerna.

- **Typer:** stapel och liggande stapel (grupperad/staplad/100 %, `gapWidth`, `overlap`), linje (markörer,
  `smooth`), yta, cirkel/ring (`firstSliceAng`, `explosion`, `holeSize`), 3D-cirkel (lutning ur `rotX`, en kant
  i mörkare färg; ingen riktig 3D), punktdiagram. Okända typer (radar, yta 3D, aktie …) och `cx:` (chartex) →
  platshållaren. Flera plottar i samma rityta ritas på samma axlar (ingen sekundäraxel).
- **Färger:** seriens/punktens `spPr`, annars diagramformatet `c:style` (1 = gråskala, 2 = accent 1–6 med
  ljusare/mörkare varianter efter sex serier, 3–8 = en accentfärg i nyanser).
- **Layout** (uppmätt i PowerPoints PDF-export av docx4j:s `p4-*`, 432 × 252 pt, text 11,97 pt): yttre marginal
  7 pt; värdeetiketterna slutar 0,92 em före ritytan; kategorietiketternas baslinje 1,57 em under axeln;
  förklaringens baslinje 0,48 em ovanför marginalen, nyckeln 0,55 em, 0,245 em till texten och 0,98 em mellan
  posterna (linjediagram: en linje på 1,59 em); ritytans överkant 0,45 em under marginalen och högerkant
  0,33 em innanför; kategorier på strecken (`midCat`) flyttar in ritytan så att första och sista etiketten får
  plats; cirkelns diameter 92,5 % av ritytans minsta sida (3D: 88 % av det). Med `layoutTarget inner` gäller
  den manuella rektangeln ritytan direkt.
- **Skala:** som Excel: 0 med om det minsta värdet är under 5/6 av det största, största + 5 %, steg
  1/2/5·10ⁿ — det minsta där etiketterna får plats: lodrät axel minst 1,5 em mellan strecken, vågrät minst
  etikettens bredd + 2 em. Ger samma axlar som PowerPoint för alla tretton `p4`-diagrammen.
- **Prov:** `chart_model`/`chart_fmt`/`chart_draw` har enhetsprov, `chart_tests.rs` ritar egna minimala
  diagram genom hela ritvägen. `KLADD_CHART_REFS=…/docx4j-chart-render/src/test/resources/charts/render/office
  cargo test chart_draw::tests::docx4j_referens -- --ignored --nocapture` jämför textens lägen med JSON-filerna
  ur PowerPoints PDF (docx4j, Apache-2.0; filerna ligger inte i git): medelfel 1,8 pt vågrätt och 0,3 pt lodrätt
  över 169 texter (Aptos ersätts med Liberation Sans). Datumetiketterna i `p4-area` skiljer sig: PDF:en är
  gjord med ett annat språk (`m/d/yyyy` visas som `1/05/2002`).

### EMF/WMF

`emf/` är en uppspelare för den vanliga delmängden av MS-EMF och MS-WMF, med Apache POI:s `hemf`/`hwmf` som
förebild för beteendet (se `emf/NOTICE.md`): `records.rs` (EMF), `wmf.rs` (WMF med och utan `Placeable`-huvud),
`player.rs` (ritytans tillstånd och operationerna), `dib.rs` (bitmappar). `media::metafile` spelar upp filen i
bildens ram med `a:srcRect` (negativa värden ger luft); ritar uppspelningen inget (bara EMF+) används den
största bitmappen eller platshållaren som förut. Bildformens egen fyllning (`p:pic/p:spPr`, inte en ärvd från
layoutens platshållare) ritas nu bakom bilden
(`render.rs`, några rader) — metafilerna i `cppcon15-stl-algorithms-mac` är genomskinliga och har en ljus
fyllning.

- Koordinater: logiskt → världstransform → fönster/viewport och mappningsläge → enhetens bildpunkter →
  0,01 mm (`szlMillimeters`, eller `szlMicrometers` om de finns) → `rclFrame` → bildens ram.
- Banor (`BeginPath` … `FillPath`/`StrokePath`/`StrokeAndFillPath`/`SelectClipPath`), polygoner och
  polylinjer (16 och 32 bitar), Bézier, rektanglar, ellipser, bågar, `SetPolyFillMode` (jämn/udda eller
  vindning), stockobjekt (`NULL_PEN` m.fl.), pennor (bredd, streck, ändar, hörn; kosmetiska = 0,75 pt),
  penslar (fyllda; streckade och mönster som blandfärg), `SaveDC`/`RestoreDC`, rektangel- och banklipp (som
  balanserade `PushClip`/`PopClip`), `ExtTextOutW` (typsnitt ur `LOGFONT` via `FontBook`, glyfer på
  `dx`-listans lägen, justering, vridning, understrykning), `StretchDIBits`/`BitBlt`/`StretchBlt`/
  `SetDIBitsToDevice`/`AlphaBlend` (och `BitBlt` utan källa som fyllning).
- Inte gjort: regionoperationer utöver snitt/ersätt, `ExcludeClipRect`, streckmönster i penslar, rasteroperationer
  utöver de vanligaste, text som glyfnummer (`ETO_GLYPH_INDEX`), EMF+.

### Resultat mot PowerPoint (diagram och EMF/WMF)

Jämförelseprovet på alla 33 presentationer med PowerPoint-PDF (1 121 bilder) före och efter: ingen
presentation fick lägre medel, ingen bild blev sämre och alla bilder med diagram eller EMF blev bättre, medel
0,8717 → 0,8735.

| Bild | Innehåll | Före | Efter |
|---|---|---:|---:|
| cppcon15-stl-algorithms-mac 50/55/61/63 | EMF (tabeller, pilar, text som banor) | 0,670–0,806 | 0,880–0,929 |
| cppcon15-stl-algorithms-mac 79–88 | EMF (tusentals banor, klipp) | 0,673–0,880 | 0,726–0,935 |
| cppcon16-iterator-haiku 46 | EMF med text (`ExtTextOutW`), `BitBlt` | 0,575 | 0,687 |
| cppcon17-inherited-codebase-mac 33–36 | EMF med klippbanor och text | 0,802–0,957 | 0,892–0,984 |
| cppcon17-inherited-codebase-mac 7 | 3D-cirkel med etikett | 0,830 | 0,943 |
| msrc-eko19 5 | 100 % staplad yta | 0,688 | 0,773 |
| msrc-eko19 6 | 100 % staplad yta med etiketter | 0,645 | 0,768 |
| msrc-win32k 8 | 3D-cirkel, utskjuten sektor | 0,784 | 0,865 |

Kvar: diagramytans radiella övertoning i `msrc-eko19` (banövertoningen ritas för ljus, `render.rs`), riktig
3D (sidornas skuggning, väggar och golv), sekundäraxlar, datatabeller, trendlinjer, felstaplar, radar- och
yt-3D-diagram, `cx:` (chartex). Tid i debug för den tyngsta EMF-bilden (`cppcon15` bild 83, 22 000 element):
visningslistan 60 ms, rastret 1,8 s (en bild utan EMF i samma presentation: 1,2 s), PDF 0,56 s.

## Tabeller och SmartArt (omgång 2)

Ersätter punkterna *Tabeller* och *SmartArt* under "Kvar till nästa omgång" ovan.

**Tabeller** (`tables.rs`):

- **Formatet** slås upp i filens `ppt/tableStyles.xml` och annars bland PowerPoints 74 inbyggda format
  (`table_builtin.rs`, data i `table_builtin.xml`). Saknas `a:tableStyleId` används inget format (inte
  listans `def`).
- **Formatdelar** i stigande prioritet: `wholeTbl`, `band1H`/`band2H`, `band1V`/`band2V`, `lastCol`,
  `firstCol`, `lastRow`, `firstRow`, `seCell`, `swCell`, `neCell`, `nwCell`. Banden räknas efter
  rubrikraden/-kolumnen och gäller inte första/sista raden eller kolumnen när de är påslagna.
  Hörncellerna gäller när både raden och kolumnen är påslagna.
- **Kanter:** varje del gäller ett område (hela tabellen, en rad eller en kolumn). En cellkant på
  områdets yttersida tar delens `left`/`right`/`top`/`bottom`, en kant inuti området `insideH`/`insideV`.
  Varje linje i rutnätet ritas en gång: cellernas egna `a:lnL`/`lnR`/`lnT`/`lnB` (även `a:noFill`) går
  före formatet, annars vinner delen med högst prioritet av de två cellerna. `a:lnRef` ger temats
  linjestil, `cmpd="dbl"` två linjer à en tredjedel, `prstDash` streckning (mönster under 1 pt ritas
  heldraget, som PowerPoints PDF i 72 dpi visar dem). Diagonaler: `a:lnTlToBr`/`lnBlToTr` och formatets
  `tl2br`/`tr2bl`. Sammanslagna celler tar kanterna från ursprungscellen. Har båda cellerna en egen tunn
  kant (under 1 pt) ritas linjen två gånger, som när PowerPoint ritar varje cells kanter (en delad
  hårfin linje blir mörkare; i PowerPoints PDF i 72 dpi är den en hel svart bildpunkt).
- **Fyllning:** cellens egen (även `a:noFill`) före formatets `a:fill`/`a:fillRef` (temats fyllningar);
  tabellens bakgrund ur `a:tblPr` eller formatets `a:tblBg`.
- **Text:** formatets `a:tcTxStyle` (`b`, `i`, `a:fontRef` major/minor, färg) läggs som en nivålista
  mellan cellens `a:lstStyle` och mallens stilar, så att den går före `otherStyle`/`defaultTextStyle` men
  efter körningens egna egenskaper.
- **Radhöjd:** `h` är minsta höjd; raden växer med texten. En cell över flera rader (`rowSpan`) får
  plats genom att den sista raden växer. Lodrät text (`vert`) får inte raden att växa.

**Inbyggda tabellformat.** `table_builtin_gen.py` läser `ppt/tableStyles.xml` i PowerPoint-sparade filer i
de licensierade provsamlingarna (Apache POI `test-data`, Apache-2.0; Open-XML-SDK, ShapeCrawler,
python-pptx, MIT) och skriver `table_builtin.xml` och `table_builtin_sources.md` (källa per GUID).
47 av 74 format finns i samlingarna. Övriga härleds ur ett syskon i samma familj genom att byta
accentfärgen; regeln provas av skriptet på alla familjer med flera observerade medlemmar (avviker ett
format stannar skriptet), och 7 av de 27 härledda stämmer exakt med PowerPoint-filer i referensmaterialet
som inte är licensierade (de används bara som kontroll). Ej verifierade: Dark Style 1 med accent (regeln i
oxml-drawing, MIT OR Apache-2.0) och Dark Style 2 – Accent 3/4 och 5/6. GUID-listan är Microsofts.

**SmartArt** (`smartart.rs`):

- Ritdelen ritas som förut. Textfärgen (`richtext.rs`, några rader; gäller alla former med `p:style`):
  körningens färg, sedan formens och platshållarnas egna nivåstilar (`a:lstStyle` på bild, layout och
  mall), sedan formens `p:style/a:fontRef`, sedan mallens `p:txStyles` och `defaultTextStyle`. Förut
  gick mallens textstilar före `fontRef`, så SmartArt-former och infogade former fick svart text där
  PowerPoint visar vit (`msrc-identity` bild 22 och 31, pratbubblan i `cppcon19-macrofree` bild 22). Ritdelen slår ihop en nod och dess text; visar layouten texten i
  en egen punkt (`presOf` till en `revTx`-punkt, t.ex. pyramiden i `cppcon17-refactor-or-die` bild 4)
  skriver `prepare_drawing` färgdefinitionens textfärg (`dgm:txFillClrLst`) in i formens `a:fontRef` när
  presentationen läses in (`model.rs`, en rad).
- Utan ritdel: datamodellens noder (`dgm:pt` utan typ, `node`, `asst`) i `parOf`-ordning som punktlista
  i en ruta med accent 1, storleken nedskalad tills texten får plats. Utan text: platshållaren som förut.
- Kvar: PowerPoint lägger om vissa layouter (kugghjulen i `msrc-win32k` bild 4) i stället för att
  använda ritdelen; layoutmotorn för SmartArt (`layoutN.xml`) finns inte.

**Resultat** (jämförelseprovet, SSIM-medel, alla 30 presentationer med PowerPoint-PDF, 1121 bilder; före =
omgång 1). Ingen presentation blev sämre; fyra enskilda bilder sjönk 0,003–0,013 (förklaring nedan).

| Presentation | Bilder | Före | Efter | Bilder med tabell/SmartArt: före → efter |
|---|---:|---:|---:|---|
| cppcon14-decomposing | 48 | 0.8943 | 0.8967 | 4: 0.912 → 0.917 |
| cppcon14-parallel-overview | 37 | 0.8266 | 0.8268 | 2: 0.845 → 0.848 |
| cppcon15-functional | 51 | 0.8518 | 0.8537 | 1: 0.853 → 0.949 |
| cppcon17-refactor-or-die | 13 | 0.9812 | 0.9820 | 6: 0.988 → 0.990 |
| cppcon19-charconv | 64 | 0.8475 | 0.8481 | 10: 0.914 → 0.918 |
| cppcon19-macrofree | 70 | 0.9217 | 0.9221 | (inga; pratbubblor med `fontRef`) |
| ms-copilot-workshop | 58 | 0.9266 | 0.9266 | 2: 0.936 → 0.937 |
| msrc-identity | 34 | 0.9283 | 0.9384 | 9: 0.933 → 0.971 |
| msrc-win32k | 35 | 0.8785 | 0.8793 | 5: 0.802 → 0.806 |
| owasp-top10-2010-es | 22 | 0.8724 | 0.8903 | 20: 0.863 → 0.883 |
| owasp-top10-2017-en/he/ja/ko/ru | 126 | | | +0.0005 – +0.0009 var |
| psconf19-pscore | 21 | 0.9301 | 0.9331 | 2: 0.923 → 0.955 |
| psconf22-chasing-seconds | 21 | 0.9033 | 0.9052 | 1: 0.841 → 0.882 |
| **alla** | 1121 | 0.8717 | 0.8728 | 181: 0.798 → 0.804 |

Sämre bilder: `cppcon19-macrofree` 22 och 51 (pratbubblans text är nu vit som i PowerPoint, men radbrytningen
skiljer eftersom de inbäddade typsnitten inte kan läsas), `msrc-win32k` 4 (kugghjulens text är vit som i
PowerPoint, men PowerPoint lägger om kugghjulen), `owasp-top10-2017-ja` 23 (kanterna ritas en gång per linje
med fyrkantiga ändar i stället för en gång per cellsida; skillnaden är under en bildpunkt).

Obs till textmotorn: SmartArt-texten i `owasp-top10-2010-es` bild 17–20 har `a:buChar char="••"` och ett
indrag som är smalare än punkten; PowerPoint visar ett tecken och texten direkt efter punkten, Kladd två
tecken och texten vid nästa tabbstopp (`richtext.rs`/`text.rs`).

**Prov:** `table_tests.rs` (sammanslagna celler, inbyggt format med band och rubrikrad, filens format
före det inbyggda, diagonaler och egna kanter, dubbel linje och temats linjestil, rader som växer även
över `rowSpan`, formatdelarnas ordning och text, SmartArt med `fontRef`-färg, `revTx`-färg och reserven
utan ritdel) och `table_builtin.rs` (alla 74 format finns och har `wholeTbl`).

## Omgång 2: ritmotor, text och typsnitt

Nya moduler: `effects.rs` (effektlistor → `scene::Effect`, ritningen i rastret), `fills.rs` (mönster,
`a:tile`, rektangulära övertoningar) och `mtx.rs` (MicroType Express). `Scene` har fått markörerna
`Item::BeginEffect(Vec<Effect>)`/`Item::EndEffect`; listan är fortfarande platt, så den som bara läser
glyfkörningar (sökning) kan hoppa över dem. `RunStyle` har nya fält (`effects`, `underline_kind`,
`underline_color`, `strike_double`) med standardvärden, och `FontFace` har `weight`.

- **Effekter:** yttre skugga (`outerShdw`, `prstShdw` som yttre skugga) med oskärpa, avstånd, riktning,
  `sx`/`sy`/`kx`/`ky` runt `algn`; inre skugga; glöd; mjuka kanter; spegling nedåt (`stA`/`endA`/`stPos`/
  `endPos`/`dist`, oskärpan görs inte). Effekterna ärvs som PowerPoint gör: första `a:effectLst` i
  platshållarkedjans `spPr` (en tom lista stänger av), annars `p:style/a:effectRef idx` mot temats
  `a:effectStyleLst` (med stilmatrisens platshållarfärg). Gäller former (formen; utan fyllning och linje
  texten), bilder, grupper (`grpSpPr`) och textkörningar (`a:rPr/a:effectLst`). Rastret ritar innehållet i
  ett eget lager stort som innehållet plus effektens räckvidd och bygger effekterna ur lagrets täckning
  (gaussisk oskärpa som tre lådfilter, σ = halva `blurRad`). PDF:en får skugga, glöd och spegling som en
  rasterbild i 144 dpi under innehållet; mjuka kanter, inre skugga och tunn text görs bara i rastret.
  Bilder med genomskinliga partier får ingen yttre skugga: PowerPoints PDF saknar den i det enda fallet i
  materialet (`psconf22-chasing-seconds` bild 11 och 17), osäkert om det gäller generellt.
- **`objectDefaults`:** gäller bara nya former i PowerPoint och används inte vid ritningen (ingen ändring).
- **Fyllningar:** `a:pattFill` ritas som riktiga mönster (alla 48 förinställda; Kladds egna 8×8-rutor efter
  utseendet i PowerPoint, `pctNN` med en Bayer-matris, en ruta = 6 pt som vid 100 % i PowerPoint); PDF:en
  får mönstrets medelfärg. `a:tile` upprepar bilden (storlek ur bildpunkterna vid 96 dpi × `sx`/`sy`,
  `algn`, `tx`/`ty`, spegling `x`/`y`/`xy`). Banövertoningar med `path="rect"`/`"shape"` får rektangulära
  nivåkurvor från `fillToRect` (en förberäknad bild); `circle` som förut. Stoppens riktning i
  banövertoningar (läge 0 ytterst) är oförändrad från omgång 1 och inte kontrollerad mot materialet.
- **3D** (`sp3d`/`scene3d`, fasning): görs inte. Fasningarna i materialet (owasp-presentationerna,
  cppcon14) sitter på små former där skillnaden inte syns i måttet.
- **Text:** bidi enligt UAX #9 med `unicode-bidi` (nivåer per stycke över alla körningar, formning per nivå,
  omordning per rad enligt L2; körningens text i logisk ordning för sökning och PDF:ens textlager). Stycken
  med `rtl="1"` har indrag och punkt från högerkanten; `algn` är absolut (l = vänster), men ett justerat
  styckes sista rad ställs till höger. Kapitäler (`cap="small"`): gemener som versaler i 80 %, övriga tecken
  i full storlek. Upphöjt/nedsänkt i två tredjedelar av storleken (uppmätt i PowerPoints PDF:
  `cppcon14-parallel-overview` bild 7 och `cppcon19-charconv` bild 50 ger 0,665), förskjutningen räknas på
  full storlek. Alla understrykningar (`dbl`, `heavy`, prickade/streckade varianter, vågiga, `words`) och
  `uFill`; dubbel genomstrykning. `eaVert`: östasiatiska tecken står upprätt (en körning per tecken som
  vrids tillbaka). `kern` (minsta storlek för kerning) prövades men infördes inte: bara 9–11 pt brödtext
  i owasp-presentationerna ändrades, med bättre mått i två och sämre i två presentationer; kerning görs
  alltid som förut.
- **Typsnitt:** MicroType Express avkodas (`mtx.rs`, efter W3C:s MTX-specifikation: LZCOMP med tre
  adaptiva Huffmanträd, sedan CTF → TrueType: `glyf`/`loca` återskapas, hintningen och tabellerna
  `cvt `/`fpgm`/`prep`/`hdmx`/`VDMX`/`LTSH` tas bort; CFF-typsnitt behåller sina tabeller). Alla 49
  `.fntdata` i materialet avkodas; inbäddad Calibri har samma teckenbredder som Carlito. Tunna vikter som
  saknas (`Light`, `Semilight`, `Thin` i namnet) ritas med ersättaren i normal vikt och skrapas
  0,012 em (Light), 0,009 em (Semilight) eller 0,02 em (Thin, ExtraLight) per kant med en tjock linje i
  läget `DestinationOut` (uppskattat ur stamtjockleken: Calibri 0,074 em mot Calibri Light 0,050 em, Segoe
  UI 0,083 em mot Light 0,047 em).
- **Prov:** `effects.rs` (skugga, glöd, mjuka kanter, inre skugga, spegling i rastret; tolkningen av en
  effektlista; `effectRef` mot korpusfilens tema och PDF med effekter), `fills.rs` (mönstertäthet,
  upprepning i rastret, rektangulär övertoning), `text.rs` (bidi med parenteser, punkt till höger,
  kapitäler), `render.rs` (`eaVert`), `mtx.rs` (LZCOMP med bokstavliga symboler, DUP2 och kopior på båda
  sidor om 512, körlängd, CTF-glyfer → TrueType som ttf-parser läser). Ignorerade prov:
  `pptx::mtx::mtx_referenspresentationer` (avkodar alla `.fntdata` under `KLADD_PPTX_REFS`, jämför Calibri
  med Carlito och skriver PDF:er till `KLADD_MTX_PDF`) och `pptx::compare::tid_per_bild`.

### Tid per bild (debug)

`pptx::compare::tid_per_bild` (visningslista + raster i 960 × 540, debug, delad maskin med andra byggen):
median 415 ms per bild över 1 121 bilder, 95:e percentilen 2,3 s. Visningslistan (formning och utläggning
av text) tar 11–600 ms i median per presentation (owasp-presentationerna med mycket 9-punktstext är
tyngst). De riktigt långsamma bilderna (3–35 s) är första avkodningen av stora bilder (PNG/JPEG i
`image` utan optimering i debug), som sedan cachas. Formade textkörningar cachas inte: i debug är
formningen inte det som dominerar, och i en optimerad byggnad är den en bråkdel. Att öppna en presentation
med inbäddade MTX-typsnitt tar längre tid (alla typsnitt avkodas vid öppningen: `cppcon19-macrofree` 0,8 s,
`psconf19-pscore` 1,8 s, `cncf-overview-ja` 1,7 s i debug).

### Resultat mot PowerPoint, omgång 2 (SSIM-medel, alla presentationer med PowerPoint-PDF)

"Före" är omgång 1 (09f28e5), mätt på samma maskin, för alla 33 presentationer med PowerPoint-PDF
(provet tar nu också mappar med `ref-1.png`, som de tre `covid-infographic-*`). Ingen
presentation blev sämre utom `cppcon14-cheap-logging` (−0,00001, bild 1: textskuggan ritas på
ersättaren för det saknade typsnittet Harlow Solid Italic). Bland de 15 presentationerna med
LibreOffice-rendering är Kladd nu närmare PowerPoint på 573 av 632 bilder (539 före).

| Presentation | Bilder | Före | Efter | LibreOffice |
|---|---:|---:|---:|---:|
| cncf-overview-ja | 52 | 0.829 | 0.924 | 0.797 |
| cppcon14-cheap-logging | 18 | 0.876 | 0.876 | – |
| cppcon14-decomposing | 48 | 0.894 | 0.899 | – |
| cppcon14-new-old-thing | 13 | 0.981 | 0.981 | – |
| cppcon14-parallel-overview | 37 | 0.827 | 0.832 | 0.806 |
| cppcon15-functional | 51 | 0.852 | 0.852 | – |
| cppcon15-stl-algorithms-mac | 99 | 0.802 | 0.803 | 0.728 |
| cppcon16-iterator-haiku | 46 | 0.796 | 0.796 | – |
| cppcon17-inherited-codebase-mac | 71 | 0.971 | 0.971 | – |
| cppcon17-refactor-or-die | 13 | 0.981 | 0.985 | 0.965 |
| cppcon18-say-nothing | 35 | 0.956 | 0.957 | 0.912 |
| cppcon18-simplicity | 43 | 0.952 | 0.952 | – |
| cppcon19-charconv | 64 | 0.847 | 0.848 | 0.734 |
| cppcon19-macrofree | 70 | 0.922 | 0.951 | 0.861 |
| cppcon20-stl-features | 45 | 0.857 | 0.857 | – |
| ms-copilot-workshop | 58 | 0.927 | 0.930 | 0.902 |
| ms-workshop-dlcv | 41 | 0.943 | 0.944 | 0.923 |
| msrc-eko19 | 23 | 0.878 | 0.881 | 0.834 |
| msrc-identity | 34 | 0.928 | 0.929 | 0.912 |
| msrc-win32k | 35 | 0.879 | 0.881 | 0.833 |
| owasp-top10-2010-es | 22 | 0.872 | 0.883 | – |
| owasp-top10-2017-en | 25 | 0.876 | 0.882 | 0.768 |
| owasp-top10-2017-he | 25 | 0.680 | 0.698 | 0.632 |
| owasp-top10-2017-ja | 25 | 0.718 | 0.724 | – |
| owasp-top10-2017-ko | 26 | 0.661 | 0.667 | – |
| owasp-top10-2017-ru | 25 | 0.816 | 0.822 | – |
| psconf18-jea | 15 | 0.907 | 0.907 | – |
| psconf19-pscore | 21 | 0.930 | 0.971 | 0.914 |
| psconf22-chasing-seconds | 21 | 0.903 | 0.904 | – |
| psconf22-video-processing | 20 | 0.924 | 0.925 | – |
| covid-infographic-exposed-fr | 1 | 0.726 | 0.726 | – |
| covid-infographic-hospitalized-fr | 1 | 0.640 | 0.640 | – |
| covid-infographic-waiting-fr | 1 | 0.787 | 0.787 | – |
| **alla 33** | 1124 | 0.871 | 0.880 | |

## Redigering och sparande (omgång 3)

Presentationen kan ändras i redigeringsläget och sparas källbevarande. Moduler:

| Modul | Gränssnitt |
|---|---|
| `edit_xml.rs` | Förlustfri XML: `parse(&[u8]) -> XDoc` (UTF-8; UTF-16 och trasig XML ger `None`, delen redigeras inte), `XElem` med normaliserade namn (`p:sp` oavsett prefix, samma tabell som `xml.rs`) och originalets start-/sluttaggar, text och kommentarer; `to_bytes()` ger byte för byte samma del om inget ändrats. En attributändring skriver bara om den taggen. Nya element skapas som text med dokumentets prefix (`XDoc::fragment`, `ensure_prefix`). |
| `edit.rs` | `Session` (utgångsläget: zip-filen som öppnades eller sparades, rörda delar, ångra/gör om som ögonblicksbilder av de delar ett steg ändrade, sammanslagen skrivning inom 1,5 s, `dirty`), `Txn` (delar tolkade för ändring, relationer `add_rel`/`remove_rel`, innehållstyper `ct_override`/`ct_default`, unika delnamn som aldrig återanvänds), `save_bytes(deck, ext)` och `mark_saved`. Efter varje ändring byggs `Deck` om ur de nya delarna (`Deck::from_package` med typsnitt och bildcachar från den förra), så ritmodellen tolkas alltid ur samma byte som sparas. |
| `save.rs` | `write_zip(base, pkg, touched)`: originalets poster i ordning, orörda (och rörda men lika) kopieras komprimerade som de är (`raw_copy_file`), ändrade skrivs om, borttagna utelämnas, nya läggs sist. |
| `edit_ops.rs` | Former: `shapes(deck, bild)` (toppnivåformer med ärvd placering för träffprov), `set_xf`, `delete_shape`, `duplicate_shape`, `reorder_shape`, `insert_textbox`, `insert_picture`, `set_text`. Bilder: `new_slide(after, layout)`, `duplicate_slide`, `delete_slide`, `move_slide`, `set_hidden`. `set_notes`. |
| `edit_text.rs` | `TextModel` (stycken med `a:pPr`, körningar med `a:rPr`, `a:br`, fält och okända element som följer med), redigering med `Pos { para, ch }`, `write_into` (orörda stycken skrivs som de var), `carets` → `Carets` (markör, markering, upp/ned, radens början/slut, träff). |
| `edit_view.rs` | `Editor` i `PptxTab` (redigeringsläget, markering, dragning, text, anteckningar), lagret över bilden, tangenterna, sidopanelens meny och dragning. |

- **Former adresseras** med läget bland bildens toppnivåformer i ritmodellen och `p:cNvPr/@id` som kontroll (id kan
  förekomma två gånger i verkliga filer). I den förlustfria XML:en räknas ett `mc:AlternateContent` som så många
  former som dess valda gren har (samma val som `xml.rs`); en sådan form flyttas i alla grenar (samma id), tas bort
  och dupliceras som helhet, men dess text redigeras inte.
- **Schemaordning:** `a:xfrm` först i `spPr` (skapas när placeringen ärvdes från layouten, med ärvda värden),
  `p:txBody` före `p:extLst`, `sldIdLst` efter mall-/anteckningsmall-/utdelningslistorna, `notesMasterIdLst` direkt
  efter `sldMasterIdLst`, `a:pPr` först och `a:endParaRPr` sist i `a:p`.
- **Ta bort form:** kopplingar (`a:stCxn`/`a:endCxn`) till formen tas bort, effekter i `p:timing` som riktar sig mot
  den (`p:spTgt/@spid`) och `p:bldP` likaså; blir en referens kvar (t.ex. en utlösare) tas hela tidsträdet bort.
- **Duplicera form:** nya id (samma nya id i alla grenar), `a16:creationId` tas bort, förskjutning 0,25 tum.
  Grafiska ramar med diagram/SmartArt/OLE vägras (de skulle dela delar).
- **Ny bild:** layoutens platshållare utom datum, sidfot, bildnummer och sidhuvud, tomma (`p:spPr` tom, `p:txBody`
  med ett tomt stycke om layoutens har text). `sldId` = största + 1 (minst 256); bilden läggs i samma avsnitt
  (`p14:sectionLst`) som bilden före. `docProps/app.xml` `Slides` räknas om.
- **Ta bort bild:** `sldId`, relationen, delen, dess relationer och innehållstyp, anteckningssidan, länkar från andra
  delar (relationen och `a:hlinkClick`/`a:hlinkHover` med det id:t), anpassade visningar och avsnitt; delar som bara
  bilden använde (media, diagram) tas bort om ingen annan relation pekar på dem.
- **Duplicera bild:** delen och dess relationer kopieras; layout, media, länkar och teman delas; anteckningssidan,
  diagram, SmartArt-delar och inbäddningar kopieras (med samma relations-id, så XML:en behöver inte ändras);
  `p14:creationId` får ett nytt värde.
- **Flytta bild:** `sldId` flyttas; i avsnitten flyttar bilden till grannens avsnitt.
- **Anteckningar:** brödtextplatshållaren får en körning per rad med formateringen från första körningen. Saknas
  anteckningssidan skapas den (bildplatshållare och brödtext, relationer åt båda hållen); saknas anteckningsmallen
  skapas en enkel mall med en kopia av första mallens tema.
- **Spara:** `Ctrl+S` → `Document::save_to` → `PptxTab::save_bytes(filändelse)`. Innehållstypen för
  `presentation.xml` ändras bara när filändelsen byter format (`.pptx`/`.ppsx`/`.potx` och makrovarianterna; till ett
  format utan makron tas `vbaProject` bort). Digitala signaturer (`_xmlsignatures/`, ursprungsrelationen och
  innehållstyperna) tas bort vid första ändringen. En `.ppt` som konverterats kräver *Spara som* (`needs_save_as`).
  Automatisk sparning går genom `save_to`; `autosave.rs` mäter tystnad med textens kontrollsumma, som för en
  presentation alltid är tom, så den sparar ~3 s efter att fliken blivit ändrad (också mitt i skrivandet; man kan
  fortsätta skriva, sparningen avslutar bara ångra-sammanslagningen). Återställning efter krasch (`recovery.rs`)
  sparar text och hoppar över presentationer.
- **Markören:** `text::layout_carets` är samma utläggning som `text::layout` och ger dessutom markörlägen (klustrens
  början, radbrytningar och styckenas slut; `Seg` vet vilken körning och vilket tecken det börjar i).
  `TextModel::carets` bygger textramen som `render::Builder::text_body`/`place_text` (arv, `bodyPr`, geometrins
  textrektangel, vridning, lodrät text) med en körning per modellens del. Provet `markorlagen_foljer_texten` jämför
  med glyfernas lägen i den ritade bilden. Höger-till-vänster: markören står vid klustrens logiska ordning (ungefärligt).
- **Rendering under redigering:** `Pool` har en version per bild; en ändrad bild renderas om i bakgrunden och den
  gamla texturen visas tills den nya är klar (en äldre rendering ersätter aldrig en nyare). Ändras antalet eller
  ordningen släpps alla texturer. Sökregistret byggs om.
- **Prov** (`edit_tests.rs`, egen fixtur med tema, mall, två layouter, anteckningar och anteckningsmall, animering,
  grupp, `mc:AlternateContent`, länk mellan bilder, avsnitt, `customXml` och en okänd del): tur och retur utan
  ändring (samma delar i samma ordning, också för alla korpusfiler, och förlustfri XML för varje del), ändrad text
  (formatering kvar, alla andra delar byte för byte lika), flytta/storlek (också ärvd placering och grupp), ångra/
  gör om, ta bort form (animering städas) och duplicera (nya id), ordning, `mc:AlternateContent`, ny bild med
  layout (avsnitt, `app.xml`), ta bort bild (länkar, anteckningar, media), flytta/duplicera/dölja bild, anteckningar
  med och utan mall, infoga textruta och bild, signaturer, format, sammanslagen skrivning och markörlägen. Varje
  sparad fil går genom `validate` (zip läsbar, varje del har innehållstyp och varje `Override` en del, relationernas
  ägare och mål finns, alla `r:id`/`r:embed` finns bland delens relationer, unika form-id per bild, `sldId` ≥ 256 och
  unika, avsnitten nämner bara bilder som finns, Kladd öppnar filen). Ignorerade prov: `validera_fil`
  (`KLADD_PPTX_VALIDATE=fil`) och `redigera_alla` (`KLADD_PPTX_SWEEP=mapp`, `KLADD_PPTX_SWEEP_OUT=mapp`: varje
  presentation sparas oförändrad, redigeras i fem steg, kontrolleras, öppnas och renderas).
- `validate` kontrollerar också sluttaggarnas namn och att alla prefix är deklarerade (med `quick_xml::NsReader`):
  `edit_xml` och `xml.rs` släpper igenom båda. Det hittade två fel som är rättade: ett `a:endParaRPr` som blev
  `a:rPr` behöll sin sluttagg, och en fil som deklarerar `a:` bara på enskilda element fick nya element med ett
  odeklarerat prefix (nu deklareras det på roten).
- **Resultat:** `redigera_alla` på hela materialet under `pptx/downloads` (543 `.pptx`, med python-pptx-, POI-,
  Open-XML-SDK- och ShapeCrawler-proven): oförändrad sparning ger samma delar i alla, och efter sex redigeringar
  (flytta, skriva/backsteg/Enter/fetstil, ny bild, anteckningar, duplicera, ta bort) går alla igenom `validate`
  (utöver fel som originalet redan hade), öppnas igen och renderas. python-pptx öppnar alla redigerade filer vars
  original den öppnar (de som inte öppnas är ISO Strict-filer och en trasig krasch-provfil, som python-pptx inte
  heller öppnar i original). LibreOffice 24.2 (`soffice --convert-to pdf`) konverterade 34 av 40 slumpvis valda
  redigerade filer; de 6 andra konverteras inte heller i original (kontrollerat för 4). Jämförelseprovet mot
  PowerPoint är oförändrat för alla 1 124 bilder (medel 0,8842, samma värde per bild som `integ2`).
- **Inte kontrollerat mot PowerPoint** (finns inte i behållaren), så att PowerPoint öppnar filerna utan
  reparationsdialog är inte bevisat; ordningen i schemat och relationerna är gjorda för det.

## Bildspel: övergångar, animeringar och föredragsvy (omgång 3)

| `scene.rs` | `Item::BeginShape { id }`/`EndShape` runt varje toppnivåform på bilden (`p:cNvPr/@id`; bara bildens eget formträd, inte mall/layout eller former inuti grupper och SmartArt), `Item::Para(n)` före varje stycke i en textram (från `text.rs`), `Scene::shape_ranges() -> Vec<(u32, Range<usize>)>`. Markörerna ritas inte (raster, PDF och sökningen hoppar över dem). |
| `transition.rs` | `Transition::of_slide(root)` (`kind`, `dur`, `adv_click`, `adv_after`); `pieces(kind, p, aspect) -> Vec<Piece>` (konvexa polygoner i texturens koordinater med avbildning till målet och alfa, lager gammal/ny/svart); `reveal(Reveal, p, aspect)` för avslöjningsformerna (används också av animeringarna). |
| `anim.rs` + `anim_expr.rs` | `Timeline::of_slide(root)`: klickgrupper (`Group { auto, effects, dur }`), `clicks()`, `groups_at_step(k)`, `chain(k)` (grupperna som spelas när man kommer till steg k), `targets()`, `initially_hidden(t)`, `state(target, geom, chain, t) -> State` (synlig, opacitet, förskjutning, skala, vridning, avslöjning). Formlerna i `p:tav`/`by`/`from`/`to` räknas av `anim_expr::eval`. |
| `anim_layers.rs` | `split(&Scene, &[Target]) -> Vec<LayerItems>` (fasta lager och mål i ritordning), `filter` (styckeval med balanserade effekt-/klippmarkörer), `render(..) -> Vec<Rendered>` (förmultiplicerad RGBA beskuren till innehållet + marginal för skugga/glöd/spegling, i samma bildpunktsrutnät som hela bilden). |
| `slideshow.rs` | `Show` har klick-steg per bild (`with_steps`, `step`, `peek_next`, `position`); `Playback` (`next(now, click)`, `prev`, `jump`, `tick`, `frame(now) -> Frame`, `animating`, `auto_due`); `Timer` och `clock_text` för föredragsvyn. Rent, provat utan GUI. |
| `presenter.rs` | `ShowView` (ägs av `PptxTab` under bildspelet): bakgrundsrendering av lagren (högst två trådar), bildrutan som egui-nät (`paint_slide`, `clip_convex`), föredragsvyn; `timings(deck, timelines, animate)`. |

- **Tidsträdet:** huvudsekvensens `p:par` är klickgrupper. En grupp med `evt="onBegin"` (eller en fördröjning utan
  `indefinite`) startar själv: först på bilden när övergången är klar, annars när gruppen före är klar. Inom gruppen
  börjar en tidsförskjuten del med *efter föregående* vid max(skriven fördröjning, förra delens slut) – PowerPoint
  skriver den summerade fördröjningen (t.ex. 1000, 3560), men 0 förekommer efter effekter på 1 ms.
  Beteendena tolkas generellt (inte per `presetID`): `p:set style.visibility`, `p:animEffect` (`fade`, `wipe(dir)`,
  `barn`, `circle`, `diamond`, `plus`, `box`, `blinds`, `checkerboard`, `randombar`, `dissolve`, `wheel`, `wedge`,
  `strips`; annat = toning), `p:anim` (`ppt_x`/`ppt_y`/`ppt_w`/`ppt_h`/`style.rotation`/`r`/`style.opacity` med nyckelbilder
  och formler), `p:animScale`, `p:animRot`, `p:animMotion` (banan som polylinje, kurvor i 8 steg, längs båglängden),
  färgbyten (`p:animClr`, `p:set` av färger i betoningar) som en blinkning. `accel`/`decel`, `autoRev` och
  `repeatCount` gäller; `fill="remove"` tar bort effekten när den är klar. `p:txEl/p:pRg` ger ett mål per stycke;
  `charRg`, `graphicEl` (SmartArt/diagram i delar) och `bg` gäller hela formen. Utlösare (`interactiveSeq`) räknas inte:
  de formerna syns som vanligt. Ett mål som inte finns i visningslistan (t.ex. på layouten) ritas som vanligt –
  aldrig "dolt för alltid".
- **Lagren:** en bild utan mål är ett enda lager (samma bild som förut). Med mål delas listan vid varje animerad form
  så att ritordningen behålls (en animerad form under en fast hamnar under den). Stycken i samma form blir egna lager;
  formens geometri och övriga stycken stannar i det fasta lagret. Provet `lagren_ger_samma_bild_som_hela_renderingen`
  visar att lagren ihoplagda ger samma bildpunkter (±2) som hela renderingen.
- **Uppspelning:** övergången ritar den gamla bilden i sitt slutläge och den nya i läget före sina animeringar; nästa
  bild förrenderas. Bakåt hoppar utan animering. `advTm` räknas från stegets början (efter övergången) men väntar
  tills stegets animeringar är klara; finns klick-steg kvar tar den automatiska övergången nästa steg. `advClick="0"`
  gör att musklick inte byter bild (tangenter gör det). Medan lagren renderas visas inget av bilden (svart), så att en
  form med in-effekt aldrig syns för tidigt, och uppspelningens klocka står still så att övergången och de
  självstartande animeringarna inte hinner gå under väntan (föredragsvyns förflutna tid går som vanligt).
- **Föredragsvyn** öppnas med `Alt+F5` i vyn (bildspel från början med föredragsvyn, `PptxTab::handle_keys`) eller
  under bildspelet. Den är en egui-viewport (`show_viewport_immediate`); utan stöd för flera fönster
  (`ctx.embed_viewports()`) växlar `Alt+F5` vyn i samma fönster. Kladd avgör inte själv om det finns två skärmar och
  öppnar inte föredragsvyn automatiskt: talarens fönster öppnas som ett vanligt fönster och publikens är helskärm.
- **Referensmaterialet:** 257 bilder har övergångar (toning 147, genom svart/`p14:reveal` 81, torka 18, push 11) och
  614 klick-steg; alla 1 081 mål hittas i visningslistan (`anim_layers::tests::referensmaterialets_animeringar`,
  ignorerat, `KLADD_PPTX_REFS`).
- **Kvar:** morph som forminterpolering, `p14`-övergångarnas egna utseenden (virvel, glitter …), riktiga färgbyten i
  betoningar (kräver omrendering), `iterate` (bokstav för bokstav), ljud och video, utlösare, val av skärm.

## 3D i diagram och former (omgång 3)

Nya moduler: `chart_3d.rs` (kamera, projektion, belysning, cirkelns och staplarnas ytor, målarordning; rena
funktioner), `chart_3d_draw.rs` (undermodul till `chart_draw.rs`, delar `Draw`) och `bevel.rs` (`a:sp3d`/
`a:scene3d` på former). `scene::Effect` har fått `Bevel`, `Contour` och `Camera` (sist i listan); kroken i
`render.rs` är `bevel::with_3d` för former och bilder, och `pdf.rs` ritar fasningen som ett rasteröverlägg
(`effects::overlay`, bara kantzonen) och former med en 3D-kamera helt som rasterbild (`effects::rasterized`).

**Diagram.**

- **Modellen:** `View3D` har `rotX`/`rotY` som `Option` (standardvärdena beror på typen: cirkel 30/0,
  stapel/linje/yta 15/20), `depthPercent`, `hPercent`, `rAngAx`, `perspective`; `Chart::walls` (`c:floor`,
  `c:sideWall`, `c:backWall`); `Plot::three_d`, `shape` (`c:shape`) och `gap_depth`.
- **Kameran:** rätvinkliga axlar (`rAngAx`, standard) ger en sned parallellprojektion där framsidan är
  oförvrängd och en enhet bakåt flyttar punkten (sin `rotY`, sin `rotX` · cos `rotY`); annars vrids världen
  `rotY` och `rotX` och `perspective` ger synfältet `perspective / 2` grader. Ytornas skuggning räknas alltid
  med den riktiga vridningen.
- **Belysningen:** en riktad ljuskälla (40° till vänster om rakt upp, 36° ut från skärmen i kamerans
  koordinater) och 35 % omgivningsljus, anpassat med minsta kvadrat till sidornas färger i de två 3D-cirklarna
  i PowerPoints PDF-export (`msrc-win32k` bild 8, `rotX` 30: sidan rakt fram 55 % av toppens ljushet,
  vänsterkanten 77 %, högerkanten 39 %; `cppcon17-inherited-codebase-mac` bild 7, `rotX` 50: 38 %).
- **3D-cirkeln:** toppar, sidor (de delar av bågen som vetter mot betraktaren, med en vågrät övertoning i
  ljusheten) och snittytor där en sektor är utskjuten; sidor och snittytor bakifrån och fram, topparna sist.
  Tjockleken 0,30 r vid `hPercent` 100 och storleken 84 % av det största som får plats valdes mot de två
  bilderna (uppmätt tjocklek 0,22–0,28 r; PowerPoints perspektiv i cirkeln görs inte).
- **Stapel, linje och yta:** en låda med bakvägg, sidovägg och golv (deras `spPr`; utan fyllning som
  standard), värdeaxelns rutnät på väggarna, serierna som föremål ritade bakifrån och fram (varje föremåls
  ytor med baksidesgallring och ljushet relativt framsidan). Staplar: lådor, cylindrar (36 sidor), koner och
  pyramider (`coneToMax`/`pyramidToMax` som stympade mot axelns största värde), grupperade, staplade, 100 %
  och `standard` (serierna på djupet när diagrammet har en `serAx`). Linjer som band (ovansida, framsida så tjock
  som 1,6 × linjebredden, minst 2,25 pt, undersida och ändar) och ytor som skivor, på var sin rad på djupet,
  lika djupa som staplarna (`gapDepth`), första serien närmast. Tomma punkter följer `c:dispBlanksAs` (`zero`
  när elementet saknas, som standardvärdet i ECMA-376; `gap`, `span`); ytor har alltid noll. 2D-linjerna
  är oförändrade (luckor). I förklaringen har 3D-linjer en ruta i bandets färg. Axeletiketterna längs lådans främre kanter,
  seriernas namn till höger om djupet.
- **Inte gjort:** `surface3DChart`, perspektiv i 3D-cirkeln, datatabeller i 3D, att räkna in etikettbredden i
  lådans placering vid stor vridning. Stapel/linje/yta i 3D finns inte i referensmaterialet och är bara
  prövade med egna fixturer (`chart_tests.rs`) och genom att titta på bilderna.

**Former** (`bevel.rs`):

- **Arv:** `a:scene3d` och `a:sp3d` var för sig ur första `spPr` i platshållarkedjan, annars ur temats
  effektstil via `effectRef` (Office 2007-temans tredje effektstil har en fasning; `bevel::THEME_3D`).
  `effectRef idx="3"` förekommer bara i `cppcon14-parallel-overview` (1), `cppcon17-inherited-codebase-mac`
  (2) och `owasp-top10-2010-es` (101, SmartArt); med och utan temats 3D gav samma mått i de åtta
  presentationerna med fasningar (cppcon14 ×2, msrc-eko19, owasp-2017 ×5), och owasp-2010-es blev bättre.
- **Fasning** (`bevelT`; `bevelB` och `extrusionH` syns inte framifrån): avståndet till formens kant
  (exakt euklidiskt avståndsfält, Felzenszwalb–Huttenlocher, ur lagrets täckning) → läget i profilen →
  normalen (profilens lutning × `h` / bredden) → ljusheten (omgivning + Lambert + Blinn-glans, relativt en
  plan yta) gånger färgen. De tolv profilerna är uppskattade ur PowerPoints galleri. `threePt` är anpassad
  till kvadraterna i `cppcon14-parallel-overview` bild 14 (PowerPoint rastrerar fasningar i PDF:en i
  200 dpi; fasetternas ljushet uppåt 1,04, nedåt 0,52, vänster 0,81, höger 0,57 → omgivning 0,29, ljuset 25°
  vänster om rakt upp och 61° ut från planet). Ljuset vrids med formens och gruppens rotation men inte med
  speglingar (kvadraterna sitter i en grupp vriden 90° med `flipV` och är ljusast åt höger). `rev` vrider
  riggen (125° i SmartArt-pilarna på samma bild stämmer), utom för de jämna riggarna (`balanced` …).
  Glansen efter materialet (`warmMatte` 0,77, `plastic`/`metal` 1, `flat`/`matte`/`dkEdge` 0), riggen
  `flat` ger ingen skuggning. Den synliga zonen är halva `w` (`bevel::ZONE`): uppmätt 6–8 pt för `w` 13 pt,
  och 0,5 gav bäst mått av 0,3–1,0.
- **Kontur** (`contourW`, `contourClr`): ett band runt formen ur avståndsfältet utanför.
- **Kameror** som inte tittar rakt framifrån (isometriska, perspektiv, egna `a:rot`): formen förvrängs
  affint runt sin mitt (planet vridet `lon`/`lat`, parallellprojektion) och extruderas `extrusionH` längs
  den projicerade djupaxeln i `extrusionClr` (annars 60 % av fyllningen); sneda kameror bara extruderar.
  Texten förvrängs inte. Ingen form i materialet har en sådan kamera.
- **Inte gjort:** 3D-text (`sp3d` på `a:bodyPr`), riktigt perspektiv, ljusare/mörkare extruderingssidor.

**Prov:** `chart_3d.rs` (hörnens lägen vid rätvinklig vy, vridning och perspektiv, belysningen mot
PowerPoints cirklar, cirkelns ytor och ritordning, framsidans vinkelintervall, lådans synliga sidor och
ordning, utåtriktade normaler för alla former), `chart_tests.rs` (3D-stapel genom hela ritvägen med
skuggade sidor, alla typer och former inom ramen, 3D-cirkel med övertonade sidor före topparna),
`bevel.rs` (avståndsfältet, profilernas form, ljuset som i PowerPoint, ljuset vrids med formen men inte med
speglingar, `orthographicFront` utan fasning ger samma visningslista som utan `scene3d`, fasning/kontur/
kamera i rastret och PDF-överlägget, kamerornas förvrängning).

**Resultat** (jämförelseprovet, alla 33 presentationer, 1 124 bilder; före = 2.11.0, `integ2`): medel
0,88424 → 0,88441. Ingen presentation blev sämre; en bild sjönk 0,0009 (`owasp-top10-2017-ko` bild 15,
fasningen på en liten ruta i hörnet), 136 bilder blev bättre.

| Bild | Innehåll | Före | Efter |
|---|---|---:|---:|
| cppcon14-parallel-overview 14 | `coolSlant` på kvadrater och cirklar, SmartArt-pilar | 0,8813 | 0,8998 |
| cppcon14-parallel-overview 33 | fasade former | 0,8145 | 0,8260 |
| cppcon14-parallel-overview 10 / 4 | fasade former | 0,6420 / 0,8794 | 0,6492 / 0,8827 |
| owasp-top10-2010-es 6 / 17 / 19 | fasade SmartArt-former | 0,8863 / 0,9213 / 0,8179 | 0,9003 / 0,9263 / 0,8223 |
| owasp-top10-2017-* (alla fem) | `circle` på rubrikrutan på varje bild | | +0,0009–0,0010 per presentation |
| cppcon14-decomposing 5 / 39 | `coolSlant`, `balanced` vriden 145° | 0,8773 / 0,8912 | 0,8788 / 0,8921 |
| msrc-win32k 8 | 3D-cirkel, utskjuten | 0,8669 | 0,8695 |
| cppcon17-inherited-codebase-mac 7 | 3D-cirkel, `rotX` 50 | 0,9454 | 0,9462 |
