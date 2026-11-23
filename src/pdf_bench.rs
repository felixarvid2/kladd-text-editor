//! Mätning av PDF-exporten på ett stort dokument (ignorerat test; se `docs/dev/prestanda.md`):
//! `KLADD_TIMING=1 cargo test pdf_bench -- --ignored --nocapture`. Dokumentet byggs av testfixturerna
//! `gen-skolrapport.docx` (fotnoter, tabeller, bilder, textruta) och `gen-nyhetsbrev.docx` (2 och 3
//! spalter i löpande avsnitt, bilder) som upprepas `KLADD_BENCH_PDF_REPS` gånger (standard 40).

use std::time::Instant;

fn load(name: &str) -> crate::docx::Loaded {
    let bytes = std::fs::read(format!("{}/tests/corpus/docx/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    crate::docx::read(&bytes).unwrap()
}

/// Stort dokument (Markdown) och bilderna det pekar på.
pub fn big_document(reps: usize) -> (String, Vec<(String, crate::docx::Image)>) {
    let (a, b) = (load("gen-skolrapport.docx"), load("gen-nyhetsbrev.docx"));
    let mut md = String::new();
    for _ in 0..reps {
        md.push_str(&a.markdown);
        md.push_str("\n\n<!-- kladd: avsnitt -->\n\n");
        md.push_str(&b.markdown);
        md.push_str("\n\n<!-- kladd: avsnitt -->\n\n");
    }
    let mut images = a.images;
    images.extend(b.images);
    (md, images)
}

#[test]
#[ignore]
fn pdf_bench() {
    let reps: usize = std::env::var("KLADD_BENCH_PDF_REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let (md, images) = big_document(reps);
    let base = std::path::PathBuf::new();
    let t = Instant::now();
    let w = crate::pdf_write::write(&md, "Bänk", &|src| crate::doc::resolve_image(src, &base, &images)).unwrap();
    if let Some(out) = std::env::var_os("KLADD_BENCH_PDF_OUT") {
        std::fs::write(out, &w.bytes).unwrap();
    }
    let pages = count_pages(&w.bytes);
    eprintln!("pdf_bench: {reps} upprepningar, {} kB Markdown, {pages} sidor, {} kB PDF, export {:.2} s", md.len() / 1024, w.bytes.len() / 1024, t.elapsed().as_secs_f64());
}

fn count_pages(bytes: &[u8]) -> usize {
    // Antal sidor ur PDF-texten (`/Type /Page` utan `/Pages`), tillräckligt för en mätning.
    let s = String::from_utf8_lossy(bytes);
    s.matches("/Type /Page\n").count() + s.matches("/Type/Page/").count() + s.matches("/Type /Page/").count()
}
