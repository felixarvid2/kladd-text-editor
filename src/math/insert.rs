//! Infoga ekvationer: en tom `$$ $$` eller en färdig struktur (bråk, rot …) ur paletten.

use std::ops::Range;

/// Markör i en mall: här hamnar textmarkören.
const CARET: char = '¤';

/// Paletten: (svensk etikett, LaTeX-mall med `¤` där markören ska stå).
pub const PALETTE: &[(&str, &str)] = &[
    ("Bråk", r"\frac{¤}{}"),
    ("Kvadratrot", r"\sqrt{¤}"),
    ("Upphöjt", r"^{¤}"),
    ("Nedsänkt", r"_{¤}"),
    ("Summa", r"\sum_{i=1}^{n} ¤"),
    ("Integral", r"\int_{a}^{b} ¤ \, dx"),
    ("Gräns", r"\lim_{x \to 0} ¤"),
    ("Parenteser", r"\left( ¤ \right)"),
    ("Matris", r"\begin{pmatrix} ¤ & \\ & \end{pmatrix}"),
    ("Grekiska bokstäver", r"\alpha \beta \gamma ¤"),
];

pub struct Edit {
    pub text: String,
    pub sel: Range<usize>,
}

/// Infogar `$$ $$` på en egen rad med markören mellan blanksteget.
pub fn insert_equation(text: &str, at: usize) -> Edit {
    // Står markören i en formel läggs den nya efter raden, så att formeln inte delas.
    let at = if inside_math(text, &(at..at)) { text[at..].find('\n').map_or(text.len(), |i| at + i) } else { at };
    block(text, at, "$$ ", " $$")
}

/// Infogar mallen `tpl` (se `PALETTE`): i formeln om markören står i en, annars som ny `$$`-formel.
pub fn insert_template(text: &str, sel: Range<usize>, tpl: &str) -> Edit {
    let (pre, post) = tpl.split_once(CARET).unwrap_or((tpl, ""));
    let inside = inside_math(text, &sel);
    if inside {
        let mut t = String::with_capacity(text.len() + tpl.len());
        t.push_str(&text[..sel.start]);
        t.push_str(pre);
        let caret = t.len();
        t.push_str(post);
        t.push_str(&text[sel.end..]);
        return Edit { text: t, sel: caret..caret };
    }
    block(text, sel.end, &format!("$$ {pre}"), &format!("{post} $$"))
}

/// Står markeringen mellan ett par `$…$`/`$$…$$` på samma rad? Raden tolkas tecken för tecken så att
/// också en halvskriven eller tom formel (`$$  $$`) räknas.
fn inside_math(text: &str, sel: &Range<usize>) -> bool {
    let ls = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
    let le = text[sel.end..].find('\n').map_or(text.len(), |i| sel.end + i);
    let b = text.as_bytes();
    let (mut i, mut open): (usize, Option<(usize, usize)>) = (ls, None);
    while i < le {
        match b[i] {
            b'\\' => i += 2,
            b'$' => {
                let n = if b.get(i + 1) == Some(&b'$') { 2 } else { 1 };
                match open {
                    None => open = Some((i + n, n)),
                    Some((start, d)) if d == n => {
                        if start <= sel.start && sel.end <= i {
                            return true;
                        }
                        open = None;
                    }
                    Some(_) => {}
                }
                i += n;
            }
            _ => i += 1,
        }
    }
    false
}

/// En `$$`-formel på egen rad (tomma rader runt) med markören mellan `open` och `close`.
fn block(text: &str, at: usize, open: &str, close: &str) -> Edit {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let before = &text[..at];
    let after = &text[at..];
    let lead = if before.is_empty() || before.ends_with("\n\n") {
        ""
    } else if before.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let trail = if after.is_empty() { "\n" } else if after.starts_with("\n\n") { "" } else if after.starts_with('\n') { "\n" } else { "\n\n" };
    let mut t = String::with_capacity(text.len() + open.len() + close.len() + 4);
    t.push_str(before);
    t.push_str(lead);
    t.push_str(open);
    let caret = t.len();
    t.push_str(close);
    t.push_str(trail);
    t.push_str(after);
    Edit { text: t, sel: caret..caret }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_equation_gets_its_own_paragraph_with_caret_inside() {
        let e = insert_equation("Hej", 3);
        assert_eq!(e.text, "Hej\n\n$$  $$\n");
        assert_eq!(&e.text[..e.sel.start], "Hej\n\n$$ ");
        let e = insert_equation("", 0);
        assert_eq!(e.text, "$$  $$\n");
        assert_eq!(e.sel, 3..3);
        let e = insert_equation("$$ a $$", 4);
        assert_eq!(e.text, "$$ a $$\n\n$$  $$\n");
        let e = insert_equation("a\n\nb", 3);
        assert_eq!(e.text, "a\n\n$$  $$\n\nb");
    }

    #[test]
    fn template_goes_into_an_existing_formula_at_the_caret() {
        let t = "Se $x +$ nu";
        let caret = t.find("$ nu").unwrap();
        let e = insert_template(t, caret..caret, r"\frac{¤}{}");
        assert_eq!(e.text, r"Se $x +\frac{}{}$ nu");
        assert_eq!(&e.text[..e.sel.start], r"Se $x +\frac{");
    }

    #[test]
    fn template_goes_into_a_fresh_empty_equation() {
        let e = insert_equation("", 0);
        let e2 = insert_template(&e.text, e.sel.clone(), r"\frac{¤}{}");
        assert_eq!(e2.text, "$$ \\frac{}{} $$\n");
        assert_eq!(&e2.text[..e2.sel.start], "$$ \\frac{");
    }

    #[test]
    fn template_outside_a_formula_makes_a_display_equation() {
        let e = insert_template("Text", 4..4, r"\sqrt{¤}");
        assert_eq!(e.text, "Text\n\n$$ \\sqrt{} $$\n");
        assert_eq!(&e.text[..e.sel.start], "Text\n\n$$ \\sqrt{");
        // Alla mallar ger tolkbar LaTeX.
        for (_, tpl) in PALETTE {
            let src = tpl.replace(CARET, "x");
            assert!(crate::math::parse::parse(&src).is_ok(), "{src}");
        }
    }
}
