//! Engelska för presentationer (`src/pptx/`).

pub const EN: &[(&str, &str)] = &[
    ("Filen är inte en giltig PowerPoint-fil (inget zip-arkiv).", "The file is not a valid PowerPoint file (not a zip archive)."),
    ("Presentationen är lösenordsskyddad eller i det äldre .ppt-formatet, som Kladd inte kan öppna än.", "The presentation is password-protected or in the older .ppt format, which Kladd cannot open yet."),
    ("PowerPoint-filen är för stor uppackad.", "The PowerPoint file is too large when unpacked."),
    ("Filen saknar en PowerPoint-presentation (presentation.xml).", "The file has no PowerPoint presentation (presentation.xml)."),
    ("SmartArt", "SmartArt"),
    ("Diagram", "Chart"),
    ("Bild (EMF)", "Picture (EMF)"),
    ("Bild (WMF)", "Picture (WMF)"),
    ("Ogiltig bildstorlek.", "Invalid slide size."),
    ("diagram", "charts"),
    ("EMF/WMF-bilder", "EMF/WMF pictures"),
    ("komprimerade inbäddade typsnitt", "compressed embedded fonts"),
    ("{name} innehåller {lost} som ännu inte visas som i PowerPoint", "{name} contains {lost} that are not yet shown as in PowerPoint"),
    ("Presentationen har inga bilder.", "The presentation has no slides."),
    ("Bild {n} (dold)", "Slide {n} (hidden)"),
    ("Bild {n}", "Slide {n}"),
    ("Bild {n} av {total}", "Slide {n} of {total}"),
    ("PowerPoint (.pptx)", "PowerPoint (.pptx)"),
    ("Presentationen visas bara; den kan inte sparas från Kladd än.", "The presentation is view-only; Kladd cannot save it yet."),
    ("Presentationer kan inte skrivas ut än.", "Presentations cannot be printed yet."),
];
