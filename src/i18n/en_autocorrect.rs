//! Engelska texter för autokorrigering (svensk källtext, engelsk text).

pub const EN: &[(&str, &str)] = &[
    ("Autokorrigering…", "AutoCorrect…"),
    ("Autokorrigering", "AutoCorrect"),
    ("Rätta medan du skriver", "Correct as you type"),
    ("I dokumentläget", "In document mode"),
    ("I Markdown och vanlig text (aldrig i kodfiler)", "In Markdown and plain text (never in code files)"),
    ("Regler", "Rules"),
    (
        "Typografiska citattecken (”…” på svenska, “…” på engelska)",
        "Smart quotes (”…” in Swedish, “…” in English)",
    ),
    ("-- blir – och --- blir —", "-- becomes – and --- becomes —"),
    ("... blir …", "... becomes …"),
    ("(c) blir ©, (r) blir ® och (tm) blir ™", "(c) becomes ©, (r) becomes ® and (tm) becomes ™"),
    ("Stor bokstav i början av meningar", "Capitalize the first letter of sentences"),
    (
        "Rätta två inledande versaler (SVerige blir Sverige)",
        "Fix two initial capitals (SWeden becomes Sweden)",
    ),
    ("Vanliga stavfel (inbyggd lista)", "Common typos (built-in list)"),
    ("Egna ersättningar", "Custom replacements"),
    ("Språk:", "Language:"),
    ("Följ stavningskontrollen", "Follow the spell checker"),
    (
        "Ordet till vänster byts mot texten till höger när du skriver det och trycker mellanslag.",
        "The word on the left is replaced by the text on the right when you type it and press space.",
    ),
    ("Med vänliga hälsningar", "Kind regards"),
];
