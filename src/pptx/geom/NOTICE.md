# presetShapeDefinitions.xml

`presetShapeDefinitions.xml` i den här mappen kommer från Apache POI
(`poi/src/main/resources/org/apache/poi/sl/draw/geom/presetShapeDefinitions.xml`, trunk november 2026)
och är licensierad under Apache License 2.0, se `LICENSE-APACHE-POI.txt`. Apache POI:s NOTICE:

    Apache POI
    Copyright 2003-2026 The Apache Software Foundation

    This product includes software developed at
    The Apache Software Foundation (https://www.apache.org/).

Filen bygger i sin tur på definitionerna av förinställda former i ECMA-376 del 1
(`OfficeOpenXML-DrawingMLGeometries.zip`), Copyright Ecma International.

## Ändringar i Kladd (Apache-2.0 §4b)

- `pie`: textrektangeln har `t="it" r="ir"` i stället för POI:s `t="ir" r="it"` (felet finns i ECMA-filen;
  LibreOffice har samma rättelse).
- Tomma rader och inledande blanksteg är borttagna (filen bäddas in i programmet). Inget annat är ändrat.

Utvärderaren i `mod.rs` är skriven för Kladd efter POI:s `sl/draw/geom` (formeloperatorerna, de inbyggda
variablerna och `arcTo`-vinklarna).
