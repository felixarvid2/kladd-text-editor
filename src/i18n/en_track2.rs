//! Engelska texter för spårade stycketecken och formateringsändringar (svensk källtext, engelsk text).

pub const EN: &[(&str, &str)] = &[
    ("Inte fet", "Not bold"),
    ("Inte kursiv", "Not italic"),
    ("Inte understruken", "Not underlined"),
    ("Inte genomstruken", "Not struck through"),
    ("Teckenstorlek", "Font size"),
    ("Markeringsfärg", "Highlight color"),
    ("Vänsterjusterad", "Left aligned"),
    ("Högerjusterad", "Right aligned"),
    ("Marginaljusterad", "Justified"),
    ("Formaterat: {what} ({author})", "Formatted: {what} ({author})"),
    ("Infogat stycketecken ({author})", "Inserted paragraph mark ({author})"),
    ("Borttaget stycketecken ({author})", "Deleted paragraph mark ({author})"),
];
