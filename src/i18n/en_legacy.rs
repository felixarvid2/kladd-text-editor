//! Engelska texter för gamla dokumentformat (.doc, .rtf) och bildformat (svensk källtext, engelsk text).

pub const EN: &[(&str, &str)] = &[
    ("Äldre dokument (via LibreOffice)", "Older documents (via LibreOffice)"),
    ("Kunde inte skapa en temporär katalog: {e}", "Could not create a temporary folder: {e}"),
    ("Kunde inte starta LibreOffice: {e}", "Could not start LibreOffice: {e}"),
    ("LibreOffice svarade inte inom {s} sekunder. Konverteringen avbröts.", "LibreOffice did not respond within {s} seconds. The conversion was cancelled."),
    ("LibreOffice kunde inte konvertera filen.", "LibreOffice could not convert the file."),
    ("{name} kan bara öppnas med LibreOffice, som inte hittades. Installera det från libreoffice.org eller öppna filen i Word och spara som .docx.", "{name} can only be opened with LibreOffice, which was not found. Install it from libreoffice.org or open the file in Word and save it as .docx."),
    ("{name} konverterades med LibreOffice. Originalet ändras inte: Spara som skapar en .docx.", "{name} was converted with LibreOffice. The original is left unchanged: Save as creates a .docx."),
    ("Konverteringen avbröts.", "The conversion was cancelled."),
    ("Konverterar {name} med LibreOffice … {s} s", "Converting {name} with LibreOffice … {s} s"),
    ("Enkel läsning av RTF utan LibreOffice: bara text, fet, kursiv och stycken. Installera LibreOffice för full återgivning. Spara som skapar en .docx.", "Basic RTF reading without LibreOffice: only text, bold, italic and paragraphs. Install LibreOffice for full fidelity. Save as creates a .docx."),
];
