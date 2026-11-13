//! Formler genom exportvägarna: docx (OMML, tur och retur), odt, pdf, html och epub, samt
//! dokumentläget (analys och utläggning).

use std::io::{Cursor, Read};

const MD: &str = "Inline $x^2+\\frac{a}{b}$ här.\n\n$$\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}$$\n\nSlut.\n";

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn docx_writes_omml_and_reads_latex_back() {
    let bytes = crate::docx::write(MD, "T", &|_| None, None).unwrap().bytes;
    let doc = part(&bytes, "word/document.xml");
    assert!(doc.contains("<m:oMath "), "{doc}");
    assert!(doc.contains("<m:oMathPara "));
    assert!(doc.contains("<m:nary>") && doc.contains("<m:f>"));
    let md = crate::docx::read(&bytes).unwrap().markdown;
    assert!(md.contains("$x^2+\\frac{a}{b}$"), "{md}");
    assert!(md.contains("$$\\sum_{i=1}^ni=\\frac{n(n+1)}{2}$$"), "{md}");
    // Och tillbaka igen ger samma formler.
    let again = crate::docx::write(&md, "T", &|_| None, None).unwrap().bytes;
    assert_eq!(part(&again, "word/document.xml").matches("<m:oMath ").count(), doc.matches("<m:oMath ").count());
}

#[test]
fn docx_keeps_unparsable_math_as_text() {
    let bytes = crate::docx::write("Se $\\bogus{x}$ nu\n", "T", &|_| None, None).unwrap().bytes;
    let doc = part(&bytes, "word/document.xml");
    assert!(!doc.contains("<m:oMath"));
    assert!(doc.contains("$\\bogus{x}$"));
}

#[test]
fn odt_embeds_a_formula_object() {
    let bytes = crate::odt::write(MD, "T", &|_| None).unwrap().bytes;
    let manifest = part(&bytes, "META-INF/manifest.xml");
    assert!(manifest.contains("application/vnd.oasis.opendocument.formula"));
    assert!(manifest.contains("Object 1/content.xml") && manifest.contains("Object 2/content.xml"));
    let obj = part(&bytes, "Object 1/content.xml");
    assert!(obj.contains("<math:math ") && obj.contains("<math:msup>"), "{obj}");
    assert!(part(&bytes, "content.xml").contains("<draw:object xlink:href=\"./Object 1\""));
}

#[test]
fn html_gets_mathml() {
    let h = String::from_utf8(crate::export::html(MD, "T", &|_| None).unwrap().bytes).unwrap();
    assert!(h.contains("<math xmlns=\"http://www.w3.org/1998/Math/MathML\">"), "{h}");
    assert!(h.contains("display=\"block\"") && h.contains("<munderover>") && h.contains("<mfrac>"));
    assert!(!h.contains("class=\"math"));
}

#[test]
fn epub_marks_mathml_pages() {
    let bytes = crate::export::epub(MD, "T", &|_| None).unwrap().bytes;
    let mut z = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let names: Vec<String> = z.file_names().map(str::to_owned).collect();
    let opf = names.iter().find(|n| n.ends_with(".opf")).unwrap().clone();
    let mut s = String::new();
    z.by_name(&opf).unwrap().read_to_string(&mut s).unwrap();
    assert!(s.contains("properties=\"mathml\""), "{s}");
    let ch = names.iter().find(|n| n.ends_with("chapter-1.xhtml")).unwrap().clone();
    let mut x = String::new();
    z.by_name(&ch).unwrap().read_to_string(&mut x).unwrap();
    assert!(x.contains("<math "));
}

#[test]
fn pdf_exports_without_error_and_draws_the_formulas() {
    let plain = crate::pdf_write::write("Inline  här.\n\nSlut.\n", "T", &|_| None).unwrap().bytes;
    let with = crate::pdf_write::write(MD, "T", &|_| None).unwrap().bytes;
    assert!(with.starts_with(b"%PDF"));
    assert!(with.len() > plain.len(), "formlerna ritas som konturer");
    // Trasig formel och formel i rubrik, lista och tabell ger inga fel.
    let odd = "# Rubrik $a_1$\n\n- punkt $\\frac12$\n\n| a | b |\n|---|---|\n| $x^2$ | $\\bogus$ |\n\n$$\n\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}\n$$\n";
    assert!(crate::pdf_write::write(odd, "T", &|_| None).unwrap().bytes.starts_with(b"%PDF"));
}

/// Skriver exempeldokument (docx, odt, pdf, md) i `KLADD_MATH_DUMP` för ögonkontroll.
#[test]
#[ignore]
fn dump_documents() {
    let Ok(dir) = std::env::var("KLADD_MATH_DUMP") else { return };
    let md = "# Ekvationer\n\nEn löpande formel $E = mc^2$ och en med bråk $\\frac{a}{b}+\\sqrt{x}$ mitt i en mening som är lång nog att brytas över flera rader i dokumentet.\n\n$$\\int_0^\\infty e^{-x^2}\\,dx = \\frac{\\sqrt{\\pi}}{2}$$\n\nNästa stycke med $\\alpha_i^2$ och $\\sum_{i=1}^n i$.\n\n$$\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}$$\n\nSlut.\n";
    std::fs::write(format!("{dir}/m.md"), md).unwrap();
    std::fs::write(format!("{dir}/m.docx"), crate::docx::write(md, "T", &|_| None, None).unwrap().bytes).unwrap();
    std::fs::write(format!("{dir}/m.odt"), crate::odt::write(md, "T", &|_| None).unwrap().bytes).unwrap();
    std::fs::write(format!("{dir}/m.pdf"), crate::pdf_write::write(md, "T", &|_| None).unwrap().bytes).unwrap();
}

#[test]
fn document_mode_analysis_marks_math_pieces() {
    use crate::word::markup;
    let a = markup::analyze("Se $x^2$ och $$y$$ samt $5 kr.\n");
    let maths: Vec<_> = a.pieces.iter().filter_map(|p| p.math.as_ref()).collect();
    assert_eq!(maths.len(), 2);
    assert_eq!(maths[0].src, "x^2");
    assert!(!maths[0].display && maths[1].display);
    // Avgränsarna är markup, innehållet inte.
    let open = a.pieces.iter().find(|p| p.markup && p.range.len() == 1 && p.range.start == 3).unwrap();
    assert!(open.math.is_none());
    // Trasig LaTeX lämnas som vanlig text.
    let b = markup::analyze("Fel $\\bogus$ här\n");
    assert!(b.pieces.iter().all(|p| p.math.is_none() && !p.markup));
}
