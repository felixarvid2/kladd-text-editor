# EMF/WMF-uppspelaren

Uppspelaren i den här mappen är skriven för Kladd efter Microsofts specifikationer MS-EMF och MS-WMF
(Open Specifications). Apache POI:s `hemf`/`hwmf` (`poi-scratchpad/src/main/java/org/apache/poi/hemf`,
`.../hwmf`, trunk november 2026) har lästs som förebild för beteendet; ingen kod är kopierad rad för rad,
men följande följer POI (Apache License 2.0, se `../geom/LICENSE-APACHE-POI.txt`):

- stockobjekten (`0x80000000 | n`: vit/grå/svart pensel, `NULL_BRUSH`, vita/svarta pennan, `NULL_PEN`);
- banparentesen (`BeginPath` … `EndPath`, en saknad `MoveTo` börjar i aktuell punkt);
- `ModifyWorldTransform`-lägena och att EMF+ hoppas över när EMF-posterna finns.

Apache POI:s NOTICE:

    Apache POI
    Copyright 2003-2026 The Apache Software Foundation

    This product includes software developed at
    The Apache Software Foundation (https://www.apache.org/).

## Egna val i Kladd

- Koordinaterna avbildas som i GDI: logiskt → världstransform → sidtransform (fönster/viewport,
  mappningsläge) → enhetens bildpunkter → 0,01 mm (huvudets `szlMillimeters`/`szlMicrometers`) → ramen
  `rclFrame` → bildens ram. POI väljer i stället den av huvud-/fönster-/viewport-rektanglarna som
  passar posterna bäst.
- Kosmetiska pennor är 0,75 pt breda; text placeras glyf för glyf efter `dx`-listan med typsnittet ur
  den gemensamma typsnittsuppslagningen; text som glyfnummer (`ETO_GLYPH_INDEX`) ritas inte.
- Regioner (`ExtSelectClipRgn`) blir sina rektanglar; `ExcludeClipRect` och andra regionoperationer än
  snitt/ersätt görs inte. Streckade penslar ritas som en blandfärg och mönsterpenslar som bitmappens
  medelfärg.
