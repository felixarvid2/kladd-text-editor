//! Engelska texter för inkrementell sparning av PDF och signaturer (`pdfview/incremental.rs`, egenskapsdialogen, sidändringar i signerad PDF).

pub const EN: &[(&str, &str)] = &[
    ("Signerad", "Signed"),
    ("ja (1 signatur)", "yes (1 signature)"),
    ("ja ({n} signaturer)", "yes ({n} signatures)"),
    ("Att ordna sidor tar bort signaturerna", "Organizing pages removes the signatures"),
    (
        "PDF:en är digitalt signerad. Ändrade sidor skrivs om hela filen, och då slutar signaturerna att gälla. Anteckningar och ifyllda fält påverkar inte signaturerna. Originalfilen ändras inte förrän du sparar.",
        "This PDF is digitally signed. Changing pages rewrites the whole file, which invalidates the signatures. Annotations and filled-in fields do not affect the signatures. The original file is not changed until you save.",
    ),
    ("Fortsätt", "Continue"),
];
