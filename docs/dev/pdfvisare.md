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
48. PDF-flikar räknas inte som ändrade och frågar aldrig om att spara.
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
