//! Engelska texter för kommandoregistret (`commands.rs`), menyraden, kommandopaletten och
//! inställningsfönstret (`settings_ui.rs`). Svensk källtext först, engelsk text sist.
//! Etiketter som redan finns i andra tabeller (Spara, Öppna…, Sök …) läggs inte till här igen.

pub const EN: &[(&str, &str)] = &[
    // Menyer (kategorier)
    ("Arkiv", "File"),
    ("Verktyg", "Tools"),
    ("Hjälp", "Help"),
    // Arkiv
    ("Exportera som webbsida…", "Export as Web Page…"),
    ("Exportera som e-bok…", "Export as E-book…"),
    ("Exportera", "Export"),
    // Redigera
    ("Gör om", "Redo"),
    ("Nästa träff", "Find Next"),
    ("Föregående träff", "Find Previous"),
    // Visa
    ("Växla ruta", "Switch Pane"),
    ("Sidofält", "Sidebar"),
    ("Gå till sida…", "Go to Page…"),
    ("Läs om från disk", "Reload from Disk"),
    // Infoga
    ("Infoga tabell", "Insert Table"),
    ("Infoga fotnot", "Insert Footnote"),
    ("Sidhuvud…", "Header…"),
    ("Sidfot…", "Footer…"),
    ("Spalter…", "Columns…"),
    // Format
    ("Större tecken", "Larger Characters"),
    ("Mindre tecken", "Smaller Characters"),
    ("Vänsterjustera", "Align Left"),
    ("Högerjustera", "Align Right"),
    ("Marginaljustera", "Justify"),
    ("Rubrik 1", "Heading 1"),
    ("Rubrik 2", "Heading 2"),
    ("Rubrik 3", "Heading 3"),
    ("Sidinställningar…", "Page Setup…"),
    // Verktyg
    ("Inställningar…", "Settings…"),
    ("Kommandopalett…", "Command Palette…"),
    ("Grammatikkontroll", "Grammar Check"),
    ("Grammatik och stil…", "Grammar and Style…"),
    ("Redigera PDF som text…", "Edit PDF as Text…"),
    ("Rotera sidan åt höger", "Rotate Page Right"),
    ("Rotera sidan åt vänster", "Rotate Page Left"),
    ("Lägg till PDF i slutet…", "Append PDF…"),
    ("Öppna i extern PDF-visare", "Open in External PDF Viewer"),
    ("Språk: Automatiskt", "Language: Automatic"),
    ("Språk: Svenska", "Language: Swedish"),
    ("Språk: English", "Language: English"),
    // Hjälp
    ("Om Kladd", "About Kladd"),
    ("Kladd {version}", "Kladd {version}"),
    // Kommandopaletten
    ("Skriv ett kommando", "Type a command"),
    // Inställningsfönstret: sektioner
    ("Inställningar", "Settings"),
    ("Allmänt", "General"),
    ("Redigerare", "Editor"),
    ("Dokument", "Document"),
    ("PDF", "PDF"),
    ("Avancerat", "Advanced"),
    // Allmänt
    ("Språk", "Language"),
    ("Tema", "Theme"),
    ("Mörkt", "Dark"),
    ("Ljust", "Light"),
    ("Textstorlek", "Text size"),
    ("Normal", "Default"),
    ("Uppdateringar", "Updates"),
    ("Sök efter uppdateringar nu", "Check for Updates Now"),
    ("Spara", "Save"),
    ("Sparar ändrade flikar som har en sökväg när skrivandet stannat", "Saves changed tabs that have a file path when you stop typing"),
    ("Osparade flikar kan återställas efter en krasch", "Unsaved tabs can be restored after a crash"),
    // Redigerare
    ("Rubrikträd för Markdown-baserade flikar", "Heading tree for Markdown-based tabs"),
    // Dokument
    ("Stavning", "Spelling"),
    // PDF
    ("Kladd minns sida, zoom och rotation för de senast öppnade PDF-filerna.", "Kladd remembers the page, zoom and rotation of the most recently opened PDF files."),
    ("1 PDF-fil har en sparad vy", "1 PDF file has a saved view"),
    ("{n} PDF-filer har en sparad vy", "{n} PDF files have a saved view"),
    ("Glöm sparade vyer", "Forget Saved Views"),
    // Avancerat
    ("Visa dolda filer och mappar i mappträdet", "Show hidden files and folders in the folder tree"),
    ("Som .git, target och node_modules", "Such as .git, target and node_modules"),
    ("Rensa listan över senaste filer", "Clear Recent Files"),
    ("Ställer tillbaka alla inställningar till grundvärdena. Öppna filer, senaste filer och arbetsmappen behålls.", "Resets all settings to their defaults. Open files, recent files and the working folder are kept."),
    ("Återställ alla inställningar…", "Reset All Settings…"),
    ("Ja, återställ", "Yes, Reset"),
    ("Inställningarna är återställda", "Settings have been reset"),
];
