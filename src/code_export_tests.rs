//! Tester för syntaxfärgade kodblock i exporterna: PDF, .docx, .odt, HTML och EPUB, och att .docx/.odt
//! läses tillbaka till samma kodblock (med språk). Se `codehl` och `docs/dev/dokumentlage.md`.

use std::collections::BTreeSet;
use std::io::{Cursor, Read};

const RUST: &str = "fn main() {\n    let s = \"hej\"; // x\n    let n = 42 & 1 < 2;\n}";
const PY: &str = "def f(x):\n    return 'a' + str(1)  # c";

fn md() -> String {
    format!("# Rapport\n\nFöre.\n\n```rust\n{RUST}\n```\n\n```python\n{PY}\n```\n\n```\nbara text\n```\n\nEfter.\n")
}

fn no_img(_: &str) -> Option<crate::docx::Image> {
    None
}

fn part(bytes: &[u8], name: &str) -> String {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = String::new();
    z.by_name(name).unwrap().read_to_string(&mut out).unwrap();
    out
}

/// Texten i en XML-del: tagg `br`/`line-break` blir radbrytning, övriga taggar tas bort.
fn xml_text(xml: &str, breaks: &[&str]) -> String {
    let mut s = xml.to_owned();
    for b in breaks {
        s = s.replace(b, "\n");
    }
    let tags = regex::Regex::new("<[^>]*>").unwrap();
    tags.replace_all(&s, "").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")
}

#[test]
fn html_has_colored_spans_and_the_language_class() {
    crate::highlight::syntaxes_wait();
    let html = String::from_utf8(crate::export::html(&md(), "T", &no_img).unwrap().bytes).unwrap();
    assert!(html.contains("<pre><code class=\"language-rust\">"), "{html}");
    assert!(html.contains("<pre><code class=\"language-python\">"));
    // Färgen ligger inline (ingen extern CSS) och minst tre olika färger används i Rust-blocket.
    let spans = regex::Regex::new("<span style=\"color:(#[0-9a-f]{6})").unwrap();
    let rust = &html[html.find("language-rust").unwrap()..html.find("</code>").unwrap()];
    let colors: BTreeSet<&str> = spans.captures_iter(rust).map(|c| c.get(1).unwrap().as_str()).collect();
    assert!(colors.len() >= 3, "{colors:?}");
    // Texten är densamma när taggarna tas bort, och specialtecken är kodade.
    let text = xml_text(rust.split_once('>').unwrap().1, &[]);
    assert_eq!(text, format!("{RUST}\n"));
    assert!(rust.contains("&amp;") && rust.contains("&lt;"));
    // Utan språk: ingen klass och inga spans.
    assert!(html.contains("<pre><code>bara text\n</code></pre>"), "{html}");
}

#[test]
fn epub_keeps_the_colored_code() {
    crate::highlight::syntaxes_wait();
    let epub = crate::export::epub(&md(), "T", &no_img).unwrap().bytes;
    let mut z = zip::ZipArchive::new(Cursor::new(epub)).unwrap();
    let mut found = false;
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        if f.name().ends_with(".xhtml") {
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            found |= s.contains("language-rust") && s.contains("<span style=\"color:#");
        }
    }
    assert!(found, "EPUB ska ha färgade kodspans");
}

#[test]
fn pdf_draws_code_in_several_colors_on_a_shaded_box() {
    let bytes = crate::pdf_write::write(&md(), "T", &|_| None).unwrap().bytes;
    let doc = lopdf::Document::load_mem(&bytes).unwrap();
    let mut fills: BTreeSet<[i32; 3]> = BTreeSet::new();
    for (_, id) in doc.get_pages() {
        let data = doc.get_page_content(id);
        let c = lopdf::content::Content::decode(&data).unwrap();
        for op in c.operations.iter().filter(|o| o.operator == "rg") {
            let v: Vec<i32> = op.operands.iter().map(|o| (o.as_float().unwrap_or(0.0) * 255.0).round() as i32).collect();
            fills.insert([v[0], v[1], v[2]]);
        }
    }
    let paper = crate::codehl::CodeColors::paper();
    let rgb = |c| paper.rgb(c).map(i32::from);
    use crate::codehl::Class;
    for class in [Class::Keyword, Class::Str, Class::Comment, Class::Number] {
        assert!(fills.contains(&rgb(class)), "färgen för {class:?} saknas bland {fills:?}");
    }
    // Rutans bakgrund finns också.
    assert!(fills.contains(&[0xF1, 0xF4, 0xF8]), "{fills:?}");
}

#[test]
fn docx_writes_a_code_style_with_colored_runs_and_reads_back_the_same_block() {
    crate::highlight::syntaxes_wait();
    let w = crate::docx::write(&md(), "T", &no_img, None).unwrap();
    let xml = part(&w.bytes, "word/document.xml");
    assert!(xml.contains("<w:pStyle w:val=\"Code\"/>"));
    assert!(xml.contains("w:name=\"_kladdlang_rust\"") && xml.contains("w:name=\"_kladdlang_python\""), "{xml}");
    let colors: BTreeSet<&str> = regex::Regex::new("<w:color w:val=\"([0-9A-F]{6})\"/>").unwrap().captures_iter(&xml).map(|c| c.get(1).unwrap().as_str()).collect::<Vec<_>>().into_iter().collect();
    assert!(colors.len() >= 4, "{colors:?}");
    // Kodstycket har skuggning och monospace i formatet.
    let styles = part(&w.bytes, "word/styles.xml");
    assert!(styles.contains("w:styleId=\"Code\"") && styles.contains("Consolas") && styles.contains("w:fill=\"F2F2F2\""));
    // Läs tillbaka: samma block, med språket.
    let l = crate::docx::read(&w.bytes).unwrap();
    assert!(l.markdown.contains(&format!("```rust\n{RUST}\n```")), "{}", l.markdown);
    assert!(l.markdown.contains(&format!("```python\n{PY}\n```")), "{}", l.markdown);
    assert!(l.markdown.contains("```\nbara text\n```"), "{}", l.markdown);
    assert!(!l.markdown.contains("kladd-lang") && !l.markdown.contains("<!--k:"), "{}", l.markdown);
    // Orörd läsning och sparning ger samma paket byte för byte.
    let again = crate::docx::write(&l.markdown, "T", &no_img, l.source.as_ref()).unwrap();
    assert_eq!(part(&again.bytes, "word/document.xml"), xml, "ett orört dokument ska sparas byte för byte");
}

#[test]
fn docx_code_text_is_unchanged_by_the_coloring() {
    crate::highlight::syntaxes_wait();
    let w = crate::docx::write(&md(), "T", &no_img, None).unwrap();
    let xml = part(&w.bytes, "word/document.xml");
    let at = xml.find("<w:pStyle w:val=\"Code\"/>").unwrap();
    let end = at + xml[at..].find("</w:p>").unwrap();
    let text = xml_text(&xml[at..end], &["<w:br/>"]);
    assert_eq!(text, RUST);
}

#[test]
fn docx_without_language_gets_no_bookmark_and_old_files_still_read() {
    let w = crate::docx::write("```\nabc\n```\n", "T", &no_img, None).unwrap();
    let xml = part(&w.bytes, "word/document.xml");
    assert!(!xml.contains("_kladdlang_"));
    assert!(crate::docx::read(&w.bytes).unwrap().markdown.contains("```\nabc\n```"));
}

#[test]
fn odt_writes_colored_spans_and_reads_back_the_same_blocks() {
    crate::highlight::syntaxes_wait();
    let w = crate::odt::write(&md(), "T", &no_img).unwrap();
    let xml = part(&w.bytes, "content.xml");
    assert!(xml.contains("text:style-name=\"Preformatted_20_Text\""));
    assert!(xml.contains("<text:bookmark text:name=\"_kladdlang_rust\"/>") && xml.contains("_kladdlang_python"), "{xml}");
    let colors: BTreeSet<&str> = regex::Regex::new("fo:color=\"(#[0-9a-fA-F]{6})\"").unwrap().captures_iter(&xml).map(|c| c.get(1).unwrap().as_str()).collect::<Vec<_>>().into_iter().collect();
    assert!(colors.len() >= 4, "{colors:?}");
    // Läs tillbaka: två intilliggande block hålls isär av språkmarkörerna.
    let l = crate::odt::read(&w.bytes).unwrap();
    assert!(l.markdown.contains(&format!("```rust\n{RUST}\n```")), "{}", l.markdown);
    assert!(l.markdown.contains(&format!("```python\n{PY}\n```")), "{}", l.markdown);
    assert!(l.markdown.contains("```\nbara text\n```"), "{}", l.markdown);
    assert!(!l.markdown.contains("kladd-lang"), "{}", l.markdown);
}

#[test]
fn odt_code_text_is_unchanged_by_the_coloring() {
    crate::highlight::syntaxes_wait();
    let w = crate::odt::write("```rust\nfn  a() {\n\tlet x = 1;\n}\n```\n", "T", &no_img).unwrap();
    let l = crate::odt::read(&w.bytes).unwrap();
    assert!(l.markdown.contains("```rust\nfn  a() {\n\tlet x = 1;\n}\n```"), "{}", l.markdown);
}

/// Skriver demofiler (för skärmbilder) till katalogen i `KLADD_DEMO_OUT`; gör inget annars.
#[test]
fn write_demo_files_when_asked() {
    let Ok(dir) = std::env::var("KLADD_DEMO_OUT") else { return };
    for name in ["kod", "diagram"] {
        let Ok(src) = std::fs::read_to_string(format!("{dir}/{name}.md")) else { continue };
        std::fs::write(format!("{dir}/{name}.docx"), crate::docx::write(&src, name, &no_img, None).unwrap().bytes).unwrap();
        std::fs::write(format!("{dir}/{name}.odt"), crate::odt::write(&src, name, &no_img).unwrap().bytes).unwrap();
        std::fs::write(format!("{dir}/{name}.pdf"), crate::pdf_write::write(&src, name, &|_| None).unwrap().bytes).unwrap();
        std::fs::write(format!("{dir}/{name}.html"), crate::export::html(&src, name, &no_img).unwrap().bytes).unwrap();
    }
}

#[test]
fn pdf_draws_mermaid_as_a_vector_diagram() {
    let flow = "```mermaid\ngraph TD\n    A[Start] --> B{Klart?}\n    B -->|Ja| C[Slut]\n```\n";
    let paths = |md: &str| -> usize {
        let bytes = crate::pdf_write::write(md, "T", &|_| None).unwrap().bytes;
        let doc = lopdf::Document::load_mem(&bytes).unwrap();
        let mut n = 0;
        for (_, id) in doc.get_pages() {
            let c = lopdf::content::Content::decode(&doc.get_page_content(id)).unwrap();
            n += c.operations.iter().filter(|o| matches!(o.operator.as_str(), "c" | "l" | "re")).count();
        }
        n
    };
    // Utan diagram (okänt block) ritas bara kodrutan; med diagram kommer banor för noder och pilar.
    let with = paths(flow);
    let without = paths("```text\ngraph TD\n    A[Start] --> B{Klart?}\n    B -->|Ja| C[Slut]\n```\n");
    assert!(with > without + 5, "diagrammet ska ritas som vektor: {with} mot {without}");
    // Ett block som inte går att rita blir kvar som kod.
    let bad = paths("```mermaid\ndet här är inget diagram\n```\n");
    assert!(bad <= without + 2, "{bad}");
}
