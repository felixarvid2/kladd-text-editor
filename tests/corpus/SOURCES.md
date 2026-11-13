# Källor och licenser för korpusen

Alla filer har en tydlig, tillåtande licens (inga GPL-only-filer). Filerna är orörda kopior av
originalen, utom `gen-*` som skapats här (se `generate/`). Namnprefixet visar källan:

| Prefix | Projekt | Licens |
|---|---|---|
| `pydocx-` | python-docx 1.2.0 (https://github.com/python-openxml/python-docx) | MIT |
| `mammoth-` | mammoth.js (https://github.com/mwilliamson/mammoth.js) | BSD-2-Clause |
| `docx4j-` | docx4j (https://github.com/plutext/docx4j, commit c0a66b6) | Apache-2.0 |
| `poi-` | Apache POI, `test-data/document` (https://github.com/apache/poi) | Apache-2.0 |
| `odfpy-` | odfpy 1.4.1, `tests/examples` (https://github.com/eea/odfpy) | Apache-2.0 (dubbelt med GPL-2.0+; Apache-2.0 används) |
| `lo-import-`, `lo-export-` | LibreOffice core, `sw/qa/extras/odfimport` och `odfexport` (https://github.com/LibreOffice/core) | MPL-2.0 |
| inget prefix (PDF) | pypdf 7.0.1, `resources/` (https://github.com/py-pdf/pypdf) | BSD-3-Clause |
| `gen-` | skapade här med LibreOffice (se nedan) | MIT, som Kladd |

PDF-filerna är valda bland pypdf:s testresurser: sådana som skapats med verktyg som LibreOffice,
pdfLaTeX, ImageMagick, Inkscape och ReportLab, eller små handgjorda testfiler. pypdf distribueras som helhet
under BSD-3-Clause. Filer som tydligt kommer från tredje part (myndighets- och företagsdokument) togs inte
med. Av POI:s och docx4j:s filer togs inte de med som har tredjepartsinnehåll (Tika-filer, nedladdade dokument).

## Genererade filer (`gen-*`)

`generate/*.fodt` är källorna (platt ODF) till sex dokument: sammanslagna celler, fotnoter och kommentarer,
spårade ändringar, sidhuvud/sidfot och spaltsektion, bild och listor, innehållsförteckning och formel. De
konverterades med `soffice --headless --convert-to odt|docx|pdf`. LibreOffice här saknar matematikmodulen och
tappar då formeln, så `gen-innehall-formler.odt` byggs av `generate/mkmath.py` med ett riktigt formelobjekt och
`gen-innehall-formler.docx` får en OMML-formel av `generate/omml.py`. Skripten körs från `generate/`.
`mk.py <utmapp>` skriver `.fodt`-filerna.

## Fil för fil

| Fil | Källa | Licens |
|---|---|---|
| `docx/docx4j-2010-mcAlternateContent-MDP.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/2010/2010-mcAlternateContent-MDP.docx | Apache-2.0 |
| `docx/docx4j-2010-mcAlternateContent-in-header.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/2010/2010-mcAlternateContent-in-header.docx | Apache-2.0 |
| `docx/docx4j-2010-mcAlternateContent.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/2010/2010-mcAlternateContent.docx | Apache-2.0 |
| `docx/docx4j-2010-sample1.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/2010/2010-sample1.docx | Apache-2.0 |
| `docx/docx4j-DrawingML_GraphicData_wps.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/2010/DrawingML_GraphicData_wps.docx | Apache-2.0 |
| `docx/docx4j-HelloWordOnline.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/HelloWordOnline.docx | Apache-2.0 |
| `docx/docx4j-LegacyForms.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/LegacyForms.docx | Apache-2.0 |
| `docx/docx4j-MERGEFIELD.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/MERGEFIELD.docx | Apache-2.0 |
| `docx/docx4j-NumberingImplicitNumId.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/NumberingImplicitNumId.docx | Apache-2.0 |
| `docx/docx4j-NumberingIndents.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/java/org/docx4j/model/listnumbering/NumberingIndents.docx | Apache-2.0 |
| `docx/docx4j-article-section-NotIsLgl.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/java/org/docx4j/model/listnumbering/article-section-NotIsLgl.docx | Apache-2.0 |
| `docx/docx4j-article-section-isLgl.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/java/org/docx4j/model/listnumbering/article-section-isLgl.docx | Apache-2.0 |
| `docx/docx4j-bar-chart.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/charts/bar-chart.docx | Apache-2.0 |
| `docx/docx4j-br-5-hyperlink.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/br-placement/br-5-hyperlink.docx | Apache-2.0 |
| `docx/docx4j-br-6-cell.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/br-placement/br-6-cell.docx | Apache-2.0 |
| `docx/docx4j-br-7-footer.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/br-placement/br-7-footer.docx | Apache-2.0 |
| `docx/docx4j-cn-1-labels.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/clause-numbers/cn-1-labels.docx | Apache-2.0 |
| `docx/docx4j-comments-one.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/comments-one.docx | Apache-2.0 |
| `docx/docx4j-comments-two.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/comments-two.docx | Apache-2.0 |
| `docx/docx4j-embedded-fonts.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/anon/embedded-fonts.docx | Apache-2.0 |
| `docx/docx4j-empty_p.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/empty_p.docx | Apache-2.0 |
| `docx/docx4j-header-section2.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/header-section2.docx | Apache-2.0 |
| `docx/docx4j-header-simple-plus-image.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/header-simple-plus-image.docx | Apache-2.0 |
| `docx/docx4j-header-simple.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/header-simple.docx | Apache-2.0 |
| `docx/docx4j-headers_footers.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docs/headers_footers.docx | Apache-2.0 |
| `docx/docx4j-hyperlink.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/hyperlink.docx | Apache-2.0 |
| `docx/docx4j-image-png1-scaled.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/image-png1-scaled.docx | Apache-2.0 |
| `docx/docx4j-image-png1.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/image-png1.docx | Apache-2.0 |
| `docx/docx4j-image-png2.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/image-png2.docx | Apache-2.0 |
| `docx/docx4j-invoice.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/OpenDoPE/invoice.docx | Apache-2.0 |
| `docx/docx4j-numbering-linked-level-other-style.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/java/org/docx4j/model/listnumbering/numbering-linked-level-other-style.docx | Apache-2.0 |
| `docx/docx4j-numbering-shared-abstract-styles.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/java/org/docx4j/model/listnumbering/numbering-shared-abstract-styles.docx | Apache-2.0 |
| `docx/docx4j-numbering-stories.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/java/org/docx4j/model/listnumbering/numbering-stories.docx | Apache-2.0 |
| `docx/docx4j-numbering-unlinked-level-claim.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/java/org/docx4j/model/listnumbering/numbering-unlinked-level-claim.docx | Apache-2.0 |
| `docx/docx4j-paragraph-single.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/paragraph-single.docx | Apache-2.0 |
| `docx/docx4j-paragraph-two.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/AlteredParts/paragraph-two.docx | Apache-2.0 |
| `docx/docx4j-startOverride.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/java/org/docx4j/model/listnumbering/startOverride.docx | Apache-2.0 |
| `docx/docx4j-w14_mcIgnorable-in-other-parts.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/2010/w14_mcIgnorable-in-other-parts.docx | Apache-2.0 |
| `docx/docx4j-w14_texteffects.docx` | https://github.com/plutext/docx4j/blob/c0a66b6100bb49d93dd0de005f7cdfd3784897ed/docx4j-core-tests/src/test/resources/2010/w14_texteffects.docx | Apache-2.0 |
| `docx/gen-bild-listor.docx` | genererad här, se generate/ | MIT (som Kladd) |
| `docx/gen-fotnoter-kommentarer.docx` | genererad här, se generate/ | MIT (som Kladd) |
| `docx/gen-innehall-formler.docx` | genererad här, se generate/ | MIT (som Kladd) |
| `docx/gen-sidhuvud-sektioner.docx` | genererad här, se generate/ | MIT (som Kladd) |
| `docx/gen-spara-andringar.docx` | genererad här, se generate/ | MIT (som Kladd) |
| `docx/gen-tabeller-sammanslagna.docx` | genererad här, se generate/ | MIT (som Kladd) |
| `docx/mammoth-comments.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/comments.docx | BSD-2-Clause |
| `docx/mammoth-embedded-style-map.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/embedded-style-map.docx | BSD-2-Clause |
| `docx/mammoth-empty.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/empty.docx | BSD-2-Clause |
| `docx/mammoth-endnotes.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/endnotes.docx | BSD-2-Clause |
| `docx/mammoth-external-picture.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/external-picture.docx | BSD-2-Clause |
| `docx/mammoth-footnote-hyperlink.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/footnote-hyperlink.docx | BSD-2-Clause |
| `docx/mammoth-footnotes.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/footnotes.docx | BSD-2-Clause |
| `docx/mammoth-simple-list.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/simple-list.docx | BSD-2-Clause |
| `docx/mammoth-single-paragraph.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/single-paragraph.docx | BSD-2-Clause |
| `docx/mammoth-strict-format.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/strict-format.docx | BSD-2-Clause |
| `docx/mammoth-strikethrough.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/strikethrough.docx | BSD-2-Clause |
| `docx/mammoth-tables.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/tables.docx | BSD-2-Clause |
| `docx/mammoth-text-box.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/text-box.docx | BSD-2-Clause |
| `docx/mammoth-tiny-picture-target-base-relative.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/tiny-picture-target-base-relative.docx | BSD-2-Clause |
| `docx/mammoth-tiny-picture.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/tiny-picture.docx | BSD-2-Clause |
| `docx/mammoth-underline.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/underline.docx | BSD-2-Clause |
| `docx/mammoth-utf8-bom.docx` | https://github.com/mwilliamson/mammoth.js/blob/master/test/test-data/utf8-bom.docx | BSD-2-Clause |
| `docx/poi-52288.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/52288.docx | Apache-2.0 |
| `docx/poi-52449.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/52449.docx | Apache-2.0 |
| `docx/poi-55733.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/55733.docx | Apache-2.0 |
| `docx/poi-55966.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/55966.docx | Apache-2.0 |
| `docx/poi-56392.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/56392.docx | Apache-2.0 |
| `docx/poi-57312.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/57312.docx | Apache-2.0 |
| `docx/poi-58067.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/58067.docx | Apache-2.0 |
| `docx/poi-58618.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/58618.docx | Apache-2.0 |
| `docx/poi-59030.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/59030.docx | Apache-2.0 |
| `docx/poi-59378.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/59378.docx | Apache-2.0 |
| `docx/poi-60293.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/60293.docx | Apache-2.0 |
| `docx/poi-60316.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/60316.docx | Apache-2.0 |
| `docx/poi-60329.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/60329.docx | Apache-2.0 |
| `docx/poi-61470.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/61470.docx | Apache-2.0 |
| `docx/poi-61745.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/61745.docx | Apache-2.0 |
| `docx/poi-61787.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/61787.docx | Apache-2.0 |
| `docx/poi-61991.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/61991.docx | Apache-2.0 |
| `docx/poi-65099.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/65099.docx | Apache-2.0 |
| `docx/poi-Bug54771a.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug54771a.docx | Apache-2.0 |
| `docx/poi-Bug54771b.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug54771b.docx | Apache-2.0 |
| `docx/poi-Bug54849.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug54849.docx | Apache-2.0 |
| `docx/poi-Bug55142.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug55142.docx | Apache-2.0 |
| `docx/poi-Bug60337.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug60337.docx | Apache-2.0 |
| `docx/poi-Bug60341.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug60341.docx | Apache-2.0 |
| `docx/poi-Bug62859.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug62859.docx | Apache-2.0 |
| `docx/poi-Bug64561.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug64561.docx | Apache-2.0 |
| `docx/poi-Bug66263-paragraph.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug66263-paragraph.docx | Apache-2.0 |
| `docx/poi-Bug66263-table.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug66263-table.docx | Apache-2.0 |
| `docx/poi-Bug66988.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Bug66988.docx | Apache-2.0 |
| `docx/poi-ComplexNumberedLists.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/ComplexNumberedLists.docx | Apache-2.0 |
| `docx/poi-DiffFirstPageHeadFoot.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/DiffFirstPageHeadFoot.docx | Apache-2.0 |
| `docx/poi-EmbeddedDocument.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/EmbeddedDocument.docx | Apache-2.0 |
| `docx/poi-EmptyDocumentWithHeaderFooter.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/EmptyDocumentWithHeaderFooter.docx | Apache-2.0 |
| `docx/poi-EnforcedWith.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/EnforcedWith.docx | Apache-2.0 |
| `docx/poi-ExternalEntityInText.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/ExternalEntityInText.docx | Apache-2.0 |
| `docx/poi-FancyFoot.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/FancyFoot.docx | Apache-2.0 |
| `docx/poi-FieldCodes.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/FieldCodes.docx | Apache-2.0 |
| `docx/poi-FldSimple.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/FldSimple.docx | Apache-2.0 |
| `docx/poi-HeaderFooterUnicode.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/HeaderFooterUnicode.docx | Apache-2.0 |
| `docx/poi-Headers.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Headers.docx | Apache-2.0 |
| `docx/poi-IllustrativeCases.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/IllustrativeCases.docx | Apache-2.0 |
| `docx/poi-NoHeadFoot.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/NoHeadFoot.docx | Apache-2.0 |
| `docx/poi-Numbering.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Numbering.docx | Apache-2.0 |
| `docx/poi-NumberingWOverrides.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/NumberingWOverrides.docx | Apache-2.0 |
| `docx/poi-NumberingWithOutOfOrderId.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/NumberingWithOutOfOrderId.docx | Apache-2.0 |
| `docx/poi-PageSpecificHeadFoot.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/PageSpecificHeadFoot.docx | Apache-2.0 |
| `docx/poi-SampleDoc.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/SampleDoc.docx | Apache-2.0 |
| `docx/poi-SimpleHeadThreeColFoot.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/SimpleHeadThreeColFoot.docx | Apache-2.0 |
| `docx/poi-Styles.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/Styles.docx | Apache-2.0 |
| `docx/poi-TestDocument.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/TestDocument.docx | Apache-2.0 |
| `docx/poi-TestPoiXMLDocumentCorePropertiesGetKeywords.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/TestPoiXMLDocumentCorePropertiesGetKeywords.docx | Apache-2.0 |
| `docx/poi-TestTableCellAlign.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/TestTableCellAlign.docx | Apache-2.0 |
| `docx/poi-TestTableColumns.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/TestTableColumns.docx | Apache-2.0 |
| `docx/poi-ThreeColFoot.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/ThreeColFoot.docx | Apache-2.0 |
| `docx/poi-ThreeColHead.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/ThreeColHead.docx | Apache-2.0 |
| `docx/poi-ThreeColHeadFoot.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/ThreeColHeadFoot.docx | Apache-2.0 |
| `docx/poi-WithGIF.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/WithGIF.docx | Apache-2.0 |
| `docx/poi-WithTabs.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/WithTabs.docx | Apache-2.0 |
| `docx/poi-bookmarks.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bookmarks.docx | Apache-2.0 |
| `docx/poi-bug-paragraph-alignment.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug-paragraph-alignment.docx | Apache-2.0 |
| `docx/poi-bug53475-password-is-pass.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug53475-password-is-pass.docx | Apache-2.0 |
| `docx/poi-bug53475-password-is-solrcell.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug53475-password-is-solrcell.docx | Apache-2.0 |
| `docx/poi-bug56075-changeTracking_off.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug56075-changeTracking_off.docx | Apache-2.0 |
| `docx/poi-bug56075-changeTracking_on.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug56075-changeTracking_on.docx | Apache-2.0 |
| `docx/poi-bug56076.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug56076.docx | Apache-2.0 |
| `docx/poi-bug57031.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug57031.docx | Apache-2.0 |
| `docx/poi-bug65738.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug65738.docx | Apache-2.0 |
| `docx/poi-bug66312.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug66312.docx | Apache-2.0 |
| `docx/poi-bug69628.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/bug69628.docx | Apache-2.0 |
| `docx/poi-capitalized.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/capitalized.docx | Apache-2.0 |
| `docx/poi-checkboxes.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/checkboxes.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIFuzzer-6709287337197568.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIFuzzer-6709287337197568.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-4791943399604224.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-4791943399604224.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-4959857092198400.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-4959857092198400.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-4961551840247808.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-4961551840247808.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-5166796835258368.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-5166796835258368.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-5313273089884160.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-5313273089884160.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-5564805011079168.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-5564805011079168.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-5569740188549120.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-5569740188549120.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-6061520554164224.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-6061520554164224.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-6120975439364096.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-6120975439364096.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-6442791109263360.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-6442791109263360.docx | Apache-2.0 |
| `docx/poi-clusterfuzz-testcase-minimized-POIXWPFFuzzer-6733884933668864.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/clusterfuzz-testcase-minimized-POIXWPFFuzzer-6733884933668864.docx | Apache-2.0 |
| `docx/poi-comment.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/comment.docx | Apache-2.0 |
| `docx/poi-crash-517626e815e0afa9decd0ebb6d1dee63fb9907dd.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/crash-517626e815e0afa9decd0ebb6d1dee63fb9907dd.docx | Apache-2.0 |
| `docx/poi-deep-table-cell.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/deep-table-cell.docx | Apache-2.0 |
| `docx/poi-delins.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/delins.docx | Apache-2.0 |
| `docx/poi-documentProperties.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/documentProperties.docx | Apache-2.0 |
| `docx/poi-documentProtection_comments_no_password.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/documentProtection_comments_no_password.docx | Apache-2.0 |
| `docx/poi-documentProtection_forms_no_password.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/documentProtection_forms_no_password.docx | Apache-2.0 |
| `docx/poi-documentProtection_no_protection.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/documentProtection_no_protection.docx | Apache-2.0 |
| `docx/poi-documentProtection_no_protection_tag_existing.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/documentProtection_no_protection_tag_existing.docx | Apache-2.0 |
| `docx/poi-documentProtection_readonly_no_password.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/documentProtection_readonly_no_password.docx | Apache-2.0 |
| `docx/poi-documentProtection_trackedChanges_no_password.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/documentProtection_trackedChanges_no_password.docx | Apache-2.0 |
| `docx/poi-drawing.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/drawing.docx | Apache-2.0 |
| `docx/poi-emptyPPr.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/emptyPPr.docx | Apache-2.0 |
| `docx/poi-endnotes.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/endnotes.docx | Apache-2.0 |
| `docx/poi-footnotes.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/footnotes.docx | Apache-2.0 |
| `docx/poi-form_footnotes.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/form_footnotes.docx | Apache-2.0 |
| `docx/poi-headerFooter.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/headerFooter.docx | Apache-2.0 |
| `docx/poi-headerPic.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/headerPic.docx | Apache-2.0 |
| `docx/poi-heading123.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/heading123.docx | Apache-2.0 |
| `docx/poi-protected_sample.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/protected_sample.docx | Apache-2.0 |
| `docx/poi-rtl.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/rtl.docx | Apache-2.0 |
| `docx/poi-sample.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/sample.docx | Apache-2.0 |
| `docx/poi-shapes-with-text.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/shapes-with-text.docx | Apache-2.0 |
| `docx/poi-smarttag-snippet.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/smarttag-snippet.docx | Apache-2.0 |
| `docx/poi-table-alignment.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/table-alignment.docx | Apache-2.0 |
| `docx/poi-table-indent.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/table-indent.docx | Apache-2.0 |
| `docx/poi-table_footnotes.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/table_footnotes.docx | Apache-2.0 |
| `docx/poi-testComment.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/testComment.docx | Apache-2.0 |
| `docx/poi-truncated62886.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/truncated62886.docx | Apache-2.0 |
| `docx/poi-unicode-path.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/unicode-path.docx | Apache-2.0 |
| `docx/poi-zero-length.docx` | https://github.com/apache/poi/blob/trunk/test-data/document/zero-length.docx | Apache-2.0 |
| `docx/pydocx-blk-containing-table.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/blk-containing-table.docx | MIT |
| `docx/pydocx-blk-inner-content.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/tests/test_files/blk-inner-content.docx | MIT |
| `docx/pydocx-blk-paras-and-tables.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/blk-paras-and-tables.docx | MIT |
| `docx/pydocx-comments-rich-para.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/comments-rich-para.docx | MIT |
| `docx/pydocx-default-template.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/src/docx/templates/default.docx | MIT |
| `docx/pydocx-doc-access-sections.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/doc-access-sections.docx | MIT |
| `docx/pydocx-doc-add-section.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/doc-add-section.docx | MIT |
| `docx/pydocx-doc-coreprops.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/doc-coreprops.docx | MIT |
| `docx/pydocx-doc-default.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/doc-default.docx | MIT |
| `docx/pydocx-doc-no-coreprops.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/doc-no-coreprops.docx | MIT |
| `docx/pydocx-doc-odd-even-hdrs.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/doc-odd-even-hdrs.docx | MIT |
| `docx/pydocx-doc-word-default-blank.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/doc-word-default-blank.docx | MIT |
| `docx/pydocx-fnt-color.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/fnt-color.docx | MIT |
| `docx/pydocx-having-images.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/tests/test_files/having-images.docx | MIT |
| `docx/pydocx-hdr-header-footer.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/hdr-header-footer.docx | MIT |
| `docx/pydocx-num-having-numbering-part.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/num-having-numbering-part.docx | MIT |
| `docx/pydocx-par-alignment.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/par-alignment.docx | MIT |
| `docx/pydocx-par-hlink-frags.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/par-hlink-frags.docx | MIT |
| `docx/pydocx-par-hyperlinks.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/par-hyperlinks.docx | MIT |
| `docx/pydocx-par-known-paragraphs.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/par-known-paragraphs.docx | MIT |
| `docx/pydocx-par-known-styles.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/par-known-styles.docx | MIT |
| `docx/pydocx-par-rendered-page-breaks.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/par-rendered-page-breaks.docx | MIT |
| `docx/pydocx-run-char-style.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/run-char-style.docx | MIT |
| `docx/pydocx-run-enumerated-props.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/run-enumerated-props.docx | MIT |
| `docx/pydocx-sct-first-page-hdrftr.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/sct-first-page-hdrftr.docx | MIT |
| `docx/pydocx-sct-inner-content.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/sct-inner-content.docx | MIT |
| `docx/pydocx-sct-section-props.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/sct-section-props.docx | MIT |
| `docx/pydocx-set-no-settings-part.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/set-no-settings-part.docx | MIT |
| `docx/pydocx-shp-inline-shape-access.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/shp-inline-shape-access.docx | MIT |
| `docx/pydocx-sty-behav-props.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/sty-behav-props.docx | MIT |
| `docx/pydocx-sty-having-no-styles-part.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/sty-having-no-styles-part.docx | MIT |
| `docx/pydocx-sty-having-styles-part.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/sty-having-styles-part.docx | MIT |
| `docx/pydocx-sty-known-styles.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/sty-known-styles.docx | MIT |
| `docx/pydocx-tab-stops.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tab-stops.docx | MIT |
| `docx/pydocx-tbl-2x2-table.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tbl-2x2-table.docx | MIT |
| `docx/pydocx-tbl-cell-access.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tbl-cell-access.docx | MIT |
| `docx/pydocx-tbl-cell-props.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tbl-cell-props.docx | MIT |
| `docx/pydocx-tbl-col-props.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tbl-col-props.docx | MIT |
| `docx/pydocx-tbl-having-applied-style.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tbl-having-applied-style.docx | MIT |
| `docx/pydocx-tbl-having-tables.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tbl-having-tables.docx | MIT |
| `docx/pydocx-tbl-on-off-props.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tbl-on-off-props.docx | MIT |
| `docx/pydocx-tbl-props.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/tbl-props.docx | MIT |
| `docx/pydocx-test.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/tests/test_files/test.docx | MIT |
| `docx/pydocx-txt-font-highlight-color.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/txt-font-highlight-color.docx | MIT |
| `docx/pydocx-txt-font-props.docx` | https://github.com/python-openxml/python-docx/blob/v1.2.0/features/steps/test_files/txt-font-props.docx | MIT |
| `odt/gen-bild-listor.odt` | genererad här, se generate/ | MIT (som Kladd) |
| `odt/gen-fotnoter-kommentarer.odt` | genererad här, se generate/ | MIT (som Kladd) |
| `odt/gen-innehall-formler.odt` | genererad här, se generate/ | MIT (som Kladd) |
| `odt/gen-sidhuvud-sektioner.odt` | genererad här, se generate/ | MIT (som Kladd) |
| `odt/gen-spara-andringar.odt` | genererad här, se generate/ | MIT (som Kladd) |
| `odt/gen-tabeller-sammanslagna.odt` | genererad här, se generate/ | MIT (som Kladd) |
| `odt/lo-export-DUMMY2.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/DUMMY2.odt | MPL-2.0 |
| `odt/lo-export-DUMMY2B.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/DUMMY2B.odt | MPL-2.0 |
| `odt/lo-export-MadeByLO7.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/MadeByLO7.odt | MPL-2.0 |
| `odt/lo-export-WordTest_edit.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/WordTest_edit.odt | MPL-2.0 |
| `odt/lo-export-allow-overlap.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/allow-overlap.odt | MPL-2.0 |
| `odt/lo-export-arabic-zero-numbering.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/arabic-zero-numbering.odt | MPL-2.0 |
| `odt/lo-export-arabic-zero3-numbering.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/arabic-zero3-numbering.odt | MPL-2.0 |
| `odt/lo-export-arabic-zero4-numbering.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/arabic-zero4-numbering.odt | MPL-2.0 |
| `odt/lo-export-arabic-zero5-numbering.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/arabic-zero5-numbering.odt | MPL-2.0 |
| `odt/lo-export-border_types.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/border_types.odt | MPL-2.0 |
| `odt/lo-export-btlr-cell.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/btlr-cell.odt | MPL-2.0 |
| `odt/lo-export-btlr-frame.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/btlr-frame.odt | MPL-2.0 |
| `odt/lo-export-chapter-number-new-line.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/chapter-number-new-line.odt | MPL-2.0 |
| `odt/lo-export-charborder.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/charborder.odt | MPL-2.0 |
| `odt/lo-export-fdo38244.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/fdo38244.odt | MPL-2.0 |
| `odt/lo-export-fdo43807.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/fdo43807.odt | MPL-2.0 |
| `odt/lo-export-fdo60769.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/fdo60769.odt | MPL-2.0 |
| `odt/lo-export-fdo66811-1.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/fdo66811-1.odt | MPL-2.0 |
| `odt/lo-export-fdo79358.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/fdo79358.odt | MPL-2.0 |
| `odt/lo-export-fillbitmap3.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/fillbitmap3.odt | MPL-2.0 |
| `odt/lo-export-framebackgrounds.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/framebackgrounds.odt | MPL-2.0 |
| `odt/lo-export-gerrit13858.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/gerrit13858.odt | MPL-2.0 |
| `odt/lo-export-gutter-left.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/gutter-left.odt | MPL-2.0 |
| `odt/lo-export-listformat.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/listformat.odt | MPL-2.0 |
| `odt/lo-export-nestedTableInFooter.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/nestedTableInFooter.odt | MPL-2.0 |
| `odt/lo-export-ooo321_stylepagenumber.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/ooo321_stylepagenumber.odt | MPL-2.0 |
| `odt/lo-export-page-content-bottom.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/page-content-bottom.odt | MPL-2.0 |
| `odt/lo-export-page-content-top.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/page-content-top.odt | MPL-2.0 |
| `odt/lo-export-pagestyle_background_lo64.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/pagestyle_background_lo64.odt | MPL-2.0 |
| `odt/lo-export-pagestyle_background_lo70.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/pagestyle_background_lo70.odt | MPL-2.0 |
| `odt/lo-export-pagestyle_background_ooo33.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/pagestyle_background_ooo33.odt | MPL-2.0 |
| `odt/lo-export-personalmetadata.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/personalmetadata.odt | MPL-2.0 |
| `odt/lo-export-qrcode-properties.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/qrcode-properties.odt | MPL-2.0 |
| `odt/lo-export-redlineTextFrame.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/redlineTextFrame.odt | MPL-2.0 |
| `odt/lo-export-referencelanguage.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/referencelanguage.odt | MPL-2.0 |
| `odt/lo-export-relh-page.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/relh-page.odt | MPL-2.0 |
| `odt/lo-export-relw-page.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/relw-page.odt | MPL-2.0 |
| `odt/lo-export-resolved-comment.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/resolved-comment.odt | MPL-2.0 |
| `odt/lo-export-rovas-numbering.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/rovas-numbering.odt | MPL-2.0 |
| `odt/lo-export-ruby-position.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/ruby-position.odt | MPL-2.0 |
| `odt/lo-export-sha1_correct.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/sha1_correct.odt | MPL-2.0 |
| `odt/lo-export-sha1_wrong.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/sha1_wrong.odt | MPL-2.0 |
| `odt/lo-export-shape-relsize.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/shape-relsize.odt | MPL-2.0 |
| `odt/lo-export-shape-with-hyperlink.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/shape-with-hyperlink.odt | MPL-2.0 |
| `odt/lo-export-shapes-hyperlink.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/shapes-hyperlink.odt | MPL-2.0 |
| `odt/lo-export-spellout-numberingtypes.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/spellout-numberingtypes.odt | MPL-2.0 |
| `odt/lo-export-sw_hatch.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/sw_hatch.odt | MPL-2.0 |
| `odt/lo-export-table-in-frame-in-table-in-header-base.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/table-in-frame-in-table-in-header-base.odt | MPL-2.0 |
| `odt/lo-export-table_styles_1.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/table_styles_1.odt | MPL-2.0 |
| `odt/lo-export-table_styles_2.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/table_styles_2.odt | MPL-2.0 |
| `odt/lo-export-table_styles_3.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/table_styles_3.odt | MPL-2.0 |
| `odt/lo-export-table_styles_4.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/table_styles_4.odt | MPL-2.0 |
| `odt/lo-export-tdf100492.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf100492.odt | MPL-2.0 |
| `odt/lo-export-tdf101710.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf101710.odt | MPL-2.0 |
| `odt/lo-export-tdf101856.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf101856.odt | MPL-2.0 |
| `odt/lo-export-tdf101856_overlapped.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf101856_overlapped.odt | MPL-2.0 |
| `odt/lo-export-tdf103567.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf103567.odt | MPL-2.0 |
| `odt/lo-export-tdf107292.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf107292.odt | MPL-2.0 |
| `odt/lo-export-tdf107696.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf107696.odt | MPL-2.0 |
| `odt/lo-export-tdf111891_frameVertStyle.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf111891_frameVertStyle.odt | MPL-2.0 |
| `odt/lo-export-tdf114287.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf114287.odt | MPL-2.0 |
| `odt/lo-export-tdf115815.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf115815.odt | MPL-2.0 |
| `odt/lo-export-tdf118637.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf118637.odt | MPL-2.0 |
| `odt/lo-export-tdf124470TableAndEmbeddedUsedFonts.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf124470TableAndEmbeddedUsedFonts.odt | MPL-2.0 |
| `odt/lo-export-tdf131025_noZerosInTable.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf131025_noZerosInTable.odt | MPL-2.0 |
| `odt/lo-export-tdf131812.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf131812.odt | MPL-2.0 |
| `odt/lo-export-tdf132642_keepWithNextTable.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf132642_keepWithNextTable.odt | MPL-2.0 |
| `odt/lo-export-tdf135338_firstLeftPageFooter.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf135338_firstLeftPageFooter.odt | MPL-2.0 |
| `odt/lo-export-tdf136645.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf136645.odt | MPL-2.0 |
| `odt/lo-export-tdf139126.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf139126.odt | MPL-2.0 |
| `odt/lo-export-tdf140437.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf140437.odt | MPL-2.0 |
| `odt/lo-export-tdf142483.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf142483.odt | MPL-2.0 |
| `odt/lo-export-tdf143605.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf143605.odt | MPL-2.0 |
| `odt/lo-export-tdf145361.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf145361.odt | MPL-2.0 |
| `odt/lo-export-tdf145871.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf145871.odt | MPL-2.0 |
| `odt/lo-export-tdf146264.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf146264.odt | MPL-2.0 |
| `odt/lo-export-tdf152710.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf152710.odt | MPL-2.0 |
| `odt/lo-export-tdf159027.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf159027.odt | MPL-2.0 |
| `odt/lo-export-tdf160700.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf160700.odt | MPL-2.0 |
| `odt/lo-export-tdf160877.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf160877.odt | MPL-2.0 |
| `odt/lo-export-tdf164712.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf164712.odt | MPL-2.0 |
| `odt/lo-export-tdf169882.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf169882.odt | MPL-2.0 |
| `odt/lo-export-tdf57317_autoListName.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf57317_autoListName.odt | MPL-2.0 |
| `odt/lo-export-tdf66305.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf66305.odt | MPL-2.0 |
| `odt/lo-export-tdf71583.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf71583.odt | MPL-2.0 |
| `odt/lo-export-tdf77961.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/tdf77961.odt | MPL-2.0 |
| `odt/lo-export-testTdf52065_centerTabs.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/testTdf52065_centerTabs.odt | MPL-2.0 |
| `odt/lo-export-textbox-rounded-corners.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/textbox-rounded-corners.odt | MPL-2.0 |
| `odt/lo-export-textframe-gradient.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/textframe-gradient.odt | MPL-2.0 |
| `odt/lo-export-textframe-transparent-shadow.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/textframe-transparent-shadow.odt | MPL-2.0 |
| `odt/lo-export-textframe-vertadjust.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/textframe-vertadjust.odt | MPL-2.0 |
| `odt/lo-export-user-field-decl-fly.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/user-field-decl-fly.odt | MPL-2.0 |
| `odt/lo-export-user-field-decl.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/user-field-decl.odt | MPL-2.0 |
| `odt/lo-export-userdefattr-tablecell.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/userdefattr-tablecell.odt | MPL-2.0 |
| `odt/lo-export-whitespace.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/whitespace.odt | MPL-2.0 |
| `odt/lo-export-z-index-header.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfexport/data/z-index-header.odt | MPL-2.0 |
| `odt/lo-import-BibliographyEntryField.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/BibliographyEntryField.odt | MPL-2.0 |
| `odt/lo-import-PageBackground.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/PageBackground.odt | MPL-2.0 |
| `odt/lo-import-Word2010AsCharShape.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/Word2010AsCharShape.odt | MPL-2.0 |
| `odt/lo-import-ZoneMacroTest.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/ZoneMacroTest.odt | MPL-2.0 |
| `odt/lo-import-annotation-formatting.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/annotation-formatting.odt | MPL-2.0 |
| `odt/lo-import-borders_ooo33.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/borders_ooo33.odt | MPL-2.0 |
| `odt/lo-import-dateFormFormats.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/dateFormFormats.odt | MPL-2.0 |
| `odt/lo-import-empty-svg-family-name.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/empty-svg-family-name.odt | MPL-2.0 |
| `odt/lo-import-fdo37606.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo37606.odt | MPL-2.0 |
| `odt/lo-import-fdo53210.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo53210.odt | MPL-2.0 |
| `odt/lo-import-fdo55814.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo55814.odt | MPL-2.0 |
| `odt/lo-import-fdo56272.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo56272.odt | MPL-2.0 |
| `odt/lo-import-fdo60842.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo60842.odt | MPL-2.0 |
| `odt/lo-import-fdo68839.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo68839.odt | MPL-2.0 |
| `odt/lo-import-fdo69862.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo69862.odt | MPL-2.0 |
| `odt/lo-import-fdo69979.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo69979.odt | MPL-2.0 |
| `odt/lo-import-fdo75872_aoo40.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo75872_aoo40.odt | MPL-2.0 |
| `odt/lo-import-fdo75872_ooo33.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo75872_ooo33.odt | MPL-2.0 |
| `odt/lo-import-fdo79269.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo79269.odt | MPL-2.0 |
| `odt/lo-import-fdo79269_header.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo79269_header.odt | MPL-2.0 |
| `odt/lo-import-fdo81223.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo81223.odt | MPL-2.0 |
| `odt/lo-import-fdo82165.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo82165.odt | MPL-2.0 |
| `odt/lo-import-fdo90130-1.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo90130-1.odt | MPL-2.0 |
| `odt/lo-import-fdo90130-2.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fdo90130-2.odt | MPL-2.0 |
| `odt/lo-import-feature_table.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/feature_table.odt | MPL-2.0 |
| `odt/lo-import-feature_table_merged-cells.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/feature_table_merged-cells.odt | MPL-2.0 |
| `odt/lo-import-feature_table_merged-cells_all.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/feature_table_merged-cells_all.odt | MPL-2.0 |
| `odt/lo-import-feature_table_text.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/feature_table_text.odt | MPL-2.0 |
| `odt/lo-import-feature_text.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/feature_text.odt | MPL-2.0 |
| `odt/lo-import-feature_text_background-color.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/feature_text_background-color.odt | MPL-2.0 |
| `odt/lo-import-feature_text_bold.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/feature_text_bold.odt | MPL-2.0 |
| `odt/lo-import-feature_text_italic.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/feature_text_italic.odt | MPL-2.0 |
| `odt/lo-import-fill-image-name-missing-definition.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/fill-image-name-missing-definition.odt | MPL-2.0 |
| `odt/lo-import-hello.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/hello.odt | MPL-2.0 |
| `odt/lo-import-incorrectsum.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/incorrectsum.odt | MPL-2.0 |
| `odt/lo-import-ooo77837-1.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/ooo77837-1.odt | MPL-2.0 |
| `odt/lo-import-paste-first-para-direct-format.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/paste-first-para-direct-format.odt | MPL-2.0 |
| `odt/lo-import-space.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/space.odt | MPL-2.0 |
| `odt/lo-import-spellmenu-redline.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/spellmenu-redline.odt | MPL-2.0 |
| `odt/lo-import-tdf100033_1.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf100033_1.odt | MPL-2.0 |
| `odt/lo-import-tdf100033_2.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf100033_2.odt | MPL-2.0 |
| `odt/lo-import-tdf103025.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf103025.odt | MPL-2.0 |
| `odt/lo-import-tdf108482.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf108482.odt | MPL-2.0 |
| `odt/lo-import-tdf109080_loext_ns.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf109080_loext_ns.odt | MPL-2.0 |
| `odt/lo-import-tdf109080_style_ns.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf109080_style_ns.odt | MPL-2.0 |
| `odt/lo-import-tdf109228.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf109228.odt | MPL-2.0 |
| `odt/lo-import-tdf113289.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf113289.odt | MPL-2.0 |
| `odt/lo-import-tdf123829.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf123829.odt | MPL-2.0 |
| `odt/lo-import-tdf123968.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf123968.odt | MPL-2.0 |
| `odt/lo-import-tdf128737.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf128737.odt | MPL-2.0 |
| `odt/lo-import-tdf133459.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf133459.odt | MPL-2.0 |
| `odt/lo-import-tdf134971a.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf134971a.odt | MPL-2.0 |
| `odt/lo-import-tdf134971b.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf134971b.odt | MPL-2.0 |
| `odt/lo-import-tdf138879.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf138879.odt | MPL-2.0 |
| `odt/lo-import-tdf146257.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf146257.odt | MPL-2.0 |
| `odt/lo-import-tdf146597.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf146597.odt | MPL-2.0 |
| `odt/lo-import-tdf165156.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf165156.odt | MPL-2.0 |
| `odt/lo-import-tdf41542_borderlessPadding.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf41542_borderlessPadding.odt | MPL-2.0 |
| `odt/lo-import-tdf74524.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf74524.odt | MPL-2.0 |
| `odt/lo-import-tdf75221.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf75221.odt | MPL-2.0 |
| `odt/lo-import-tdf76349_1columnBreak.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf76349_1columnBreak.odt | MPL-2.0 |
| `odt/lo-import-tdf92586.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf92586.odt | MPL-2.0 |
| `odt/lo-import-tdf94882.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf94882.odt | MPL-2.0 |
| `odt/lo-import-tdf96113.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/tdf96113.odt | MPL-2.0 |
| `odt/lo-import-timeFormFormats.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/timeFormFormats.odt | MPL-2.0 |
| `odt/lo-import-unreferenced_stream.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/unreferenced_stream.odt | MPL-2.0 |
| `odt/lo-import-vmerge-cell-border.odt` | https://github.com/LibreOffice/core/blob/master/sw/qa/extras/odfimport/data/vmerge-cell-border.odt | MPL-2.0 |
| `odt/odfpy-contextualspacing.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/contextualspacing.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-definitionlists.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/definitionlists.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-headerfooter.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/headerfooter.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-images.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/images.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-imageslabels.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/imageslabels.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-nasty.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/nasty.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-no_userfields.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/no_userfields.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-parastyles.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/parastyles.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-serious_poem.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/serious_poem.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-simplelist.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/simplelist.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-simplestyles.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/simplestyles.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-simpletable.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/simpletable.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-twolevellist.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/twolevellist.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-userfields.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/userfields.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `odt/odfpy-userfields_ooo3.odt` | https://github.com/eea/odfpy/blob/master/tests/examples/userfields_ooo3.odt | Apache-2.0 (dubbelt med GPL-2.0+, Apache-2.0 används) |
| `pdf/AutoCad_Simple.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/AutoCad_Simple.pdf | BSD-3-Clause |
| `pdf/FormTestFromOo.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/FormTestFromOo.pdf | BSD-3-Clause |
| `pdf/Sample_Td-matrix.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/Sample_Td-matrix.pdf | BSD-3-Clause |
| `pdf/box.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/box.pdf | BSD-3-Clause |
| `pdf/bytes.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/bytes.pdf | BSD-3-Clause |
| `pdf/commented.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/commented.pdf | BSD-3-Clause |
| `pdf/crazyones.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/crazyones.pdf | BSD-3-Clause |
| `pdf/encrypted-file.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/encrypted-file.pdf | BSD-3-Clause |
| `pdf/encryption-r4-aes-user-password.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/encryption/r4-aes-user-password.pdf | BSD-3-Clause |
| `pdf/encryption-unencrypted.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/encryption/unencrypted.pdf | BSD-3-Clause |
| `pdf/form.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/form.pdf | BSD-3-Clause |
| `pdf/form_evince.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/form_evince.pdf | BSD-3-Clause |
| `pdf/gen-bild-listor.pdf` | genererad här, se generate/ | MIT (som Kladd) |
| `pdf/gen-fotnoter-kommentarer.pdf` | genererad här, se generate/ | MIT (som Kladd) |
| `pdf/gen-innehall-formler.pdf` | genererad här, se generate/ | MIT (som Kladd) |
| `pdf/gen-sidhuvud-sektioner.pdf` | genererad här, se generate/ | MIT (som Kladd) |
| `pdf/gen-tabeller-sammanslagna.pdf` | genererad här, se generate/ | MIT (som Kladd) |
| `pdf/hello-world.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/hello-world.pdf | BSD-3-Clause |
| `pdf/imagemagick-ASCII85Decode.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/imagemagick-ASCII85Decode.pdf | BSD-3-Clause |
| `pdf/imagemagick-CCITTFaxDecode.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/imagemagick-CCITTFaxDecode.pdf | BSD-3-Clause |
| `pdf/imagemagick-images.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/imagemagick-images.pdf | BSD-3-Clause |
| `pdf/imagemagick-lzw.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/imagemagick-lzw.pdf | BSD-3-Clause |
| `pdf/inkscape-abc.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/inkscape-abc.pdf | BSD-3-Clause |
| `pdf/issue-297.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/issue-297.pdf | BSD-3-Clause |
| `pdf/jpeg.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/jpeg.pdf | BSD-3-Clause |
| `pdf/libreoffice-form.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/libreoffice-form.pdf | BSD-3-Clause |
| `pdf/libreoffice-writer-password.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/libreoffice-writer-password.pdf | BSD-3-Clause |
| `pdf/metadata.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/metadata.pdf | BSD-3-Clause |
| `pdf/multilang.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/multilang.pdf | BSD-3-Clause |
| `pdf/pdflatex-forms.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/pdflatex-forms.pdf | BSD-3-Clause |
| `pdf/pdflatex-outline.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/pdflatex-outline.pdf | BSD-3-Clause |
| `pdf/reportlab-inline-image.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/reportlab-inline-image.pdf | BSD-3-Clause |
| `pdf/superscript-pdftex.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/superscript-pdftex.pdf | BSD-3-Clause |
| `pdf/toy.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/toy.pdf | BSD-3-Clause |
| `pdf/two-different-pages.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/two-different-pages.pdf | BSD-3-Clause |
| `pdf/visitor_text_position.pdf` | https://github.com/py-pdf/pypdf/blob/main/resources/visitor_text_position.pdf | BSD-3-Clause |
