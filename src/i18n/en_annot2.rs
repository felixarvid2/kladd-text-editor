//! Engelska texter för området annot2: redigera och ta bort befintliga PDF-anteckningar (`pdfview/annot_edit.rs`).

pub const EN: &[(&str, &str)] = &[
    ("Rita fritt (dra); högerklick tar bort en anteckning", "Draw freehand (drag); right-click removes an annotation"),
    // Typer
    ("Markering", "Highlight"),
    ("Vågig understrykning", "Squiggly underline"),
    ("Ritning", "Drawing"),
    ("Rektangel", "Rectangle"),
    ("Ellips", "Ellipse"),
    ("Linje", "Line"),
    // Ruta intill vald anteckning
    ("Okänd författare", "Unknown author"),
    ("Kommentar", "Comment"),
    ("Ta bort", "Delete"),
    // Sidopanel
    ("Kommentarer", "Comments"),
    ("Inga kommentarer", "No comments"),
    ("Sida {n} · {kind}", "Page {n} · {kind}"),
    ("Sida {n} · {who}", "Page {n} · {who}"),
];
