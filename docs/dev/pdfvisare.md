# PDF-visaren (1.9)

När en `.pdf` öppnas visar Kladd sidorna som de ser ut, som en vanlig PDF-visare. Knappen
**Redigera** i verktygsraden gör om PDF:en till ett redigerbart dokument (dagens väg via
`pdf_read` → dokumentläget). Fram till dess är fliken skrivskyddad och aldrig "ändrad".

Studerat: pdf.js (Firefox), GNOME Papers/Evince, Okular, Zathura, SumatraPDF, Förhandsvisning,
Chrome och PdfCraft (storytold/pdfcraft, Rust + egui + hayro).

## Teknik

- **Rendering**: `hayro` 0.9 (ren Rust, Apache-2.0/MIT, samma upphovsman som krilla). Prototyp:
  utseende som poppler, 1–30 ms per sida, inga krascher på 900 trasiga filer. `Pdf` är `Send +
  Sync`; `RenderCache`/`InterpreterCache` är inte `Send` → en per arbetstråd. Varje rendering
  och textutdragning körs i `catch_unwind` (som PdfCraft).
- **Text med positioner**: en `hayro_interpret::Device` som samlar glyfer (`as_unicode()` +
  rektangel) per sida; rader och ord grupperas av Kladd.
- **Innehållsförteckning, länkar, sidetiketter, dokumentinfo**: `lopdf` (finns redan).
- **Koordinater**: allt i *visningspunkter*: origo uppe till vänster i sidans beskärningsruta
  efter sidans `/Rotate`, y nedåt, 1 enhet = 1 pt (samma rum som hayros
  `page.initial_transform(true)` och `page.render_dimensions()`). Användarens extra rotation
  (90° steg) läggs på i vyn.

## Moduler (`src/pdfview/`)

| Fil | Innehåll |
|---|---|
| `engine.rs` | `PdfEngine`: öppnar med hayro, trådpool, prioritetskö, rendering, textutdragning, omfärgning |
| `text.rs` | `PageText`: glyfer, rader, ord, sökning, träffrektanglar, markering |
| `meta.rs` | `PdfMeta`: innehållsförteckning, länkar, sidetiketter, dokumentinfo (lopdf) |
| `view.rs` | `PdfView`: själva visaren (layout, zoom, rendering till texturer, navigering, markering, sök, länkar) |
| `organize.rs` | Sidoperationer på PDF-bytes med lopdf (ordna, ta bort, rotera, extrahera, infoga) och `History` för Ångra |
| `pages.rs` | Kopplar `organize` till fliken: `OrgOp`, urval, omladdning av visaren, Ångra/Gör om, `PdfTab::pages_changed` |
| `ocr.rs` | `OcrState`: letar skannade sidor, renderar i 300 dpi på egen motor, kör Tesseract (`crate::ocr`) och ger `PageText` via `PdfView::set_ocr_text` |
| `chrome.rs` | Verktygsrad, sidopanel (miniatyrer, innehåll), sökfält, egenskaper, presentationsläge, statusrad |

## Förbättringar (minst 40)

Visning
1. Riktiga sidor renderade med hayro i stället för omgjord text.
2. Rendering i skärmens upplösning (pixels per point) – skarp text på HiDPI.
3. Grå arbetsyta runt sidorna (#D4D4D7 ljust, #2A2A2E mörkt, som pdf.js), vita sidor med skugga.
4. Kontinuerlig lodrät rullning med ~10 px mellanrum mellan sidorna, sidorna centrerade.
5. Platshållare i rätt storlek medan en sida renderas (och en snurra efter 400 ms).
6. Vid zoom sträcks den gamla bilden direkt och en skarp bild renderas efteråt.
7. Bara synliga sidor (och grannar) renderas; synliga först (prioritetskö), övriga släpps (cache med tak i MB).
8. Rendering i bakgrundstrådar, flera sidor parallellt; UI:t fryser aldrig.
9. Stora sidor begränsas (max ~32 MP per bild, kantlängd inom texturgränsen).
10. Uppslag: en sida / två sidor (udda/jämna, med eller utan omslag).
11. Rotera vyn 90° åt höger/vänster (R / Shift+R).
12. Mörkt läge: sidorna är vita som standard; val för inverterade färger och sepia.

Zoom
13. "Automatisk" standardzoom som pdf.js: stående sidor passar bredden (högst 125 %), liggande hela sidan.
14. Zoomval: Automatisk, Verklig storlek, Hel sida, Sidbredd, 50–400 %.
15. Ctrl+plus/minus i steg om ×1,1, Ctrl+0 = 100 %, Ctrl+1 verklig storlek, Ctrl+2 sidbredd, Ctrl+3 hel sida.
16. Ctrl+mushjul och nypning zoomar kring pekaren.
17. Zoomen följer fönstrets storlek i lägena Automatisk/Sidbredd/Hel sida.
18. Zoom 10–2500 %.

Navigering
19. Sidfält "3 / 12" i verktygsraden (skriv ett nummer + Enter), Ctrl+G fokuserar det.
20. Föregående/nästa sida (knappar, PgUp/PgDn, Mellanslag/Shift+Mellanslag, N/P, J/K).
21. Home/End och Ctrl+Upp/Ned till första/sista sidan.
22. Pilar rullar; vänster/höger byter sida när hela sidan syns.
23. Historik bakåt/framåt (Alt+Vänster/Höger) efter länkar och hopp.
24. Sidetiketter ("iv (4 av 12)") när PDF:en har sådana.
25. Kom ihåg sida, zoom och rotation per fil.
26. Handverktyg: dra för att panorera (mittenknapp/blanksteg+dra, eller H-verktyget).

Sidopanel
27. Sidopanel (F4) med miniatyrer, aktuell sida markerad, klick hoppar.
28. Innehållsförteckning (bokmärken) med expandera/fäll ihop, klick hoppar till rätt ställe.
29. Panelen följer med: aktuell sida/avsnitt hålls synligt.

Text och sök
30. Markera text med musen (dra; dubbelklick = ord, trippelklick = rad), Ctrl+C kopierar, Ctrl+A markerar allt.
31. Sök (Ctrl+F) i hela dokumentet med "3 av 17", alla träffar markerade, aktuell träff starkare.
32. Sökval: skilj på stora/små bokstäver, hela ord; ignorera diakritiska tecken (é = e) som standard.
33. Enter/Shift+Enter, F3/Shift+F3 stegar mellan träffar runt hela dokumentet; träffen rullas in i bild.
34. "Hittades inte" när inget matchar; sökningen pågår i bakgrunden sida för sida.

Länkar
35. Klickbara länkar: interna hoppar (med historik), externa öppnas i webbläsaren efter bekräftelse.
36. Handpekare och verktygstips med adressen när pekaren är över en länk.

Fil och verktyg
37. **Redigera**-knapp: gör om PDF:en till ett redigerbart dokument (med förklaring om att layouten kan ändras).
38. Skriv ut skickar originalfilen (inte en omrendering).
39. "Spara kopia" sparar originalfilen oförändrad.
40. Dokumentegenskaper: titel, författare, ämne, nyckelord, skapad/ändrad, program, PDF-version, sidstorlek, antal sidor, filstorlek.
41. Lösenordsskyddade PDF:er öppnas efter lösenord (befintlig dialog).
42. Läs om automatiskt när filen ändras på disk (behåll position); R i menyn för att läsa om.
43. Presentationsläge (F5): helskärm, en sida åt gången, pilar/klick, Esc avslutar.
44. Öppna i systemets PDF-visare.
45. Statusrad: sidetikett, "Sida x av y", zoom i procent.
46. Kopiera sidan som bild.
47. Trasiga sidor visar ett felmeddelande på sidan i stället för att krascha.
48. PDF-flikar är bara ändrade efter sidändringar (`pages_changed`, se punkt 51) och frågar annars aldrig om att spara.
49. ⌘ i stället för Ctrl på Mac i alla kortkommandon.
50. Formulärfält och anteckningar visas (hayro ritar deras utseende).

## API-kontrakt (våg A levererar, våg B använder)

```rust
// src/pdfview/engine.rs
pub enum OpenError { NeedsPassword, WrongPassword, Invalid(String) }
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Recolor { #[default] None, Invert, Sepia }
#[derive(Clone, Copy, Debug)]
pub struct PageSize { pub width: f32, pub height: f32 }   // visningspunkter (efter /Rotate)
#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub page: usize,
    pub scale: f32,        // bildpunkter per pt (zoom × 96/72 × pixels_per_point)
    pub rotation: u16,     // användarens extra rotation: 0, 90, 180, 270
    pub recolor: Recolor,
}
pub enum EngineEvent {
    Rendered { req: RenderRequest, image: egui::ColorImage },
    Text { page: usize, text: std::sync::Arc<crate::pdfview::text::PageText> },
    Failed { page: usize, error: String },
}
pub struct PdfEngine { .. }
impl PdfEngine {
    pub fn open(bytes: std::sync::Arc<Vec<u8>>, password: Option<&str>, ctx: egui::Context) -> Result<Self, OpenError>;
    pub fn page_count(&self) -> usize;
    pub fn page_size(&self, page: usize) -> PageSize;
    /// Ersätter kön: listan är i prioritetsordning (först = viktigast). Gamla, ännu ej påbörjade jobb stryks.
    pub fn request(&self, reqs: Vec<RenderRequest>);
    /// Textlager för sidorna (i prioritetsordning); levereras en gång per sida och cachas av motorn.
    pub fn request_text(&self, pages: Vec<usize>);
    /// Hämtar färdiga resultat utan att blockera.
    pub fn poll(&self) -> Vec<EngineEvent>;
}

// src/pdfview/text.rs
pub struct Glyph { pub text: String, pub rect: egui::Rect }      // visningspunkter
pub struct PageText { pub glyphs: Vec<Glyph>, /* + rader/ord, intern sökbar text */ }
impl PageText {
    pub fn text(&self) -> &str;                                       // läsordning, " " mellan ord, "\n" mellan rader
    pub fn find(&self, query: &str, case_sensitive: bool, whole_word: bool) -> Vec<std::ops::Range<usize>>; // glyfintervall; diakritiska tecken ignoreras när case_sensitive == false
    pub fn rects(&self, glyphs: std::ops::Range<usize>) -> Vec<egui::Rect>;  // sammanslagna per rad
    pub fn glyph_at(&self, p: egui::Pos2) -> Option<usize>;          // närmaste glyf på raden under punkten
    pub fn word_at(&self, glyph: usize) -> std::ops::Range<usize>;
    pub fn line_at(&self, glyph: usize) -> std::ops::Range<usize>;
    pub fn text_of(&self, glyphs: std::ops::Range<usize>) -> String;  // med mellanslag/radbrytningar
}

// src/pdfview/meta.rs
pub struct Dest { pub page: usize, pub y: Option<f32> }             // y = överkant i visningspunkter
pub enum LinkTarget { Page(Dest), Uri(String) }
pub struct Link { pub rect: egui::Rect, pub target: LinkTarget }   // visningspunkter
pub struct OutlineItem { pub title: String, pub dest: Option<Dest>, pub children: Vec<OutlineItem> }
#[derive(Default)]
pub struct DocInfo { pub title: Option<String>, pub author: Option<String>, pub subject: Option<String>,
    pub keywords: Option<String>, pub creator: Option<String>, pub producer: Option<String>,
    pub created: Option<String>, pub modified: Option<String>, pub version: String }
#[derive(Default)]
pub struct PdfMeta { pub outline: Vec<OutlineItem>, pub links: Vec<Vec<Link>>, pub labels: Vec<String>, pub info: DocInfo }
pub fn read_meta(bytes: &[u8], password: Option<&str>) -> PdfMeta;  // aldrig panik; tomt vid fel
```

## API-kontrakt för våg B

`src/pdfview/view.rs` (visaren, agent 1) äger `PdfView`. `src/pdfview/chrome.rs` och
integrationen i app.rs/doc.rs (agent 2) använder bara metoderna nedan.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ZoomMode { #[default] Auto, ActualSize, PageFit, PageWidth, Custom }
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Spread { #[default] None, Odd, Even }
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tool { #[default] Select, Hand }

/// Sparas per fil i inställningarna (kom ihåg läget).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ViewMemory { pub page: usize, pub y_frac: f32, pub zoom_mode: ZoomMode, pub zoom: f32, pub rotation: u16, pub spread: Spread }

pub struct PdfView { /* engine, meta, layout, texturer, markering, sök, historik … */ }
impl PdfView {
    pub fn new(engine: PdfEngine, meta: PdfMeta, bytes: Arc<Vec<u8>>, memory: Option<ViewMemory>) -> Self;
    /// Ritar sidorna i hela `ui`-ytan och hanterar mus/tangenter när den har fokus.
    pub fn show(&mut self, ui: &mut egui::Ui, palette: &crate::theme::Palette, dark: bool);
    pub fn handle_keys(&mut self, ctx: &egui::Context);      // anropas från app.rs handle_shortcuts i PDF-läge
    pub fn page_count(&self) -> usize;
    pub fn current_page(&self) -> usize;                      // 0-baserad, sidan i mitten av vyn
    pub fn page_label(&self, page: usize) -> String;          // "iv" eller "4"
    pub fn go_to_page(&mut self, page: usize);
    pub fn go_to(&mut self, dest: &Dest);                     // lägger till i historiken
    pub fn back(&mut self); pub fn forward(&mut self); pub fn can_back(&self) -> bool; pub fn can_forward(&self) -> bool;
    pub fn zoom(&self) -> f32;                                // 1.0 = 100 %
    pub fn zoom_mode(&self) -> ZoomMode;
    pub fn set_zoom_mode(&mut self, m: ZoomMode);
    pub fn set_zoom(&mut self, z: f32);                       // → Custom
    pub fn zoom_in(&mut self); pub fn zoom_out(&mut self);    // ×1,1
    pub fn rotate(&mut self, clockwise: bool);
    pub fn spread(&self) -> Spread; pub fn set_spread(&mut self, s: Spread);
    pub fn recolor(&self) -> Recolor; pub fn set_recolor(&mut self, r: Recolor);
    pub fn tool(&self) -> Tool; pub fn set_tool(&mut self, t: Tool);
    // Sök
    pub fn find_set(&mut self, query: &str, case_sensitive: bool, whole_word: bool); // startar/uppdaterar
    pub fn find_next(&mut self, backwards: bool);
    pub fn find_status(&self) -> FindStatus;                  // { searching: bool, current: Option<usize>, total: usize, done_pages: usize }
    pub fn find_clear(&mut self);
    // Text
    pub fn selected_text(&self) -> Option<String>; pub fn select_all(&mut self); pub fn copy_selection(&self, ctx: &egui::Context);
    // Presentationsläge (helskärm, en sida åt gången)
    pub fn presenting(&self) -> bool; pub fn set_presenting(&mut self, on: bool);
    pub fn memory(&self) -> ViewMemory;
    pub fn meta(&self) -> &PdfMeta; pub fn engine(&self) -> &PdfEngine; pub fn bytes(&self) -> &Arc<Vec<u8>>;
    /// Bild av en sida för miniatyrer (begär rendering i liten skala; None tills den finns).
    pub fn thumbnail(&mut self, ctx: &egui::Context, page: usize, width_px: f32) -> Option<egui::TextureHandle>;
    /// Ersätter dokumentet efter att filen ändrats på disk; behåller sida och zoom.
    pub fn reload(&mut self, engine: PdfEngine, meta: PdfMeta, bytes: Arc<Vec<u8>>);
}
```

## Ordna sidor

- `organize.rs` arbetar på bytes: `reorder`, `delete`, `rotate`, `extract`, `insert`. Sidträdet plattas ut först (ärvda `Resources`/`MediaBox`/`CropBox`/`Rotate` skrivs in på sidorna). Vid borttagning blir referenser till borttagna sidor `null` och `prune_objects` rensar oanvänt. `insert` numrerar om den infogade filens objekt (`renumber_objects_with`) och flyttar in dem; dess bokmärken och formulär tas inte med.
- Krypterade filer avkrypteras (som `print::decrypted_pdf`) vid första ändringen. Resultatet är okrypterat, och sparas det över originalet försvinner lösenordsskyddet.
- `pages::run` gör ändringen, läser om visaren (`PdfView::reload`) och lägger föregående bytes i `History` (högst 30 versioner och 256 MB). `PdfTab::pages_changed` sätter `Document::dirty`; Ctrl+S skriver bytes över originalet med `write_atomic`. Ändrad fil på disk läses inte om automatiskt medan sidändringar är osparade.
51. Miniatyrer: flerval, drag för att flytta, högerklicksmeny, Delete, Ångra/Gör om, Slå ihop med PDF.
