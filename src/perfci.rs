//! Prestandaprov för CI: instruktionsräkning i stället för tid (se `perf/README.md` och `docs/dev/prestanda.md`).
//!
//! Varje `perfci_*`-prov är `#[ignore]` och körs ett i taget under `valgrind --tool=callgrind --collect-atstart=no`
//! av `perf/run.sh`. Bara själva arbetet räknas: `Counting` slår på räkningen med en callgrind-förfrågan
//! (`CALLGRIND_TOGGLE_COLLECT`) när mätningen börjar och av när den är klar, så att uppstart, typsnittsinläsning
//! och det provet bygger upp före mätningen inte ger brus. (Ett funktionsnamnsfilter, `--toggle-collect`, visade sig
//! missa funktioner med stor stackram, därför förfrågan.) Utanför valgrind gör den ingenting.
//! Indata (stora .docx/.odt) skapas av `perfci_fixtures` i `KLADD_PERFCI_DIR` innan mätningarna körs, med fasta frön
//! så att samma kod ger exakt samma filer. Prov som arbetar i trådar (PDF-export, läsning av docx/odt) slutar på `_mt` och använder inte `Counting` och
//! räknas för hela processen (räkningen gäller bara den tråd som slår på den).

use std::hint::black_box;
use std::path::PathBuf;

use egui::{Color32, FontId};

use crate::word::bench::{big_markdown, frame, in_ui};
use crate::word::{layout, matchlayer, pages};
use crate::{search, theme};

/// Storlekar: ett dokument på ~100 sidor (docx) och ett något mindre ODT.
const DOCX_BYTES: usize = 220_000;
const ODT_BYTES: usize = 120_000;

/// Slår på räkningen medan värdet lever. Skapa det först i mätfunktionen så att det släpps sist.
struct Counting;

impl Counting {
    fn on() -> Self {
        toggle_collect();
        Counting
    }
}

impl Drop for Counting {
    fn drop(&mut self) {
        toggle_collect();
    }
}

/// `CALLGRIND_TOGGLE_COLLECT` (valgrind/callgrind.h): `VG_USERREQ_TOOL_BASE('C','T') + 2` via valgrinds
/// speciella instruktionssekvens. Utanför valgrind är sekvensen utan verkan (rotationerna ger tillsammans 128 bitar).
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn toggle_collect() {
    let args: [u64; 6] = [0x4354_0002, 0, 0, 0, 0, 0];
    // SAFETY: bara rotationer som tar ut varandra och `xchg rbx, rbx` (ingen verkan); argumentblocket lever under anropet.
    unsafe {
        std::arch::asm!(
            "rol rdi, 3", "rol rdi, 13", "rol rdi, 61", "rol rdi, 51",
            "xchg rbx, rbx",
            in("rax") args.as_ptr(),
            inout("rdx") 0u64 => _,
            options(nostack),
        );
    }
}

#[cfg(not(all(target_arch = "x86_64", target_os = "linux")))]
fn toggle_collect() {}

pub fn dir() -> PathBuf {
    std::env::var_os("KLADD_PERFCI_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("kladd-perfci"))
}

fn fixture(name: &str) -> Vec<u8> {
    let p = dir().join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e} (kör perfci_fixtures först)", p.display()))
}

/// Skapar indata: stora dokument genererade av Kladds egna skrivare, utan nätverk och utan slump.
#[test]
#[ignore]
fn perfci_fixtures() {
    let d = dir();
    std::fs::create_dir_all(&d).unwrap();
    let none = |_: &str| None;
    let docx = crate::docx::write(&big_markdown(DOCX_BYTES, 7), "Prov", &none, None).unwrap();
    std::fs::write(d.join("stor.docx"), &docx.bytes).unwrap();
    let odt = crate::odt::write(&big_markdown(ODT_BYTES, 11), "Prov", &none).unwrap();
    std::fs::write(d.join("stor.odt"), &odt.bytes).unwrap();
    // Kontroll att filerna går att läsa.
    assert!(crate::docx::read(&docx.bytes).is_ok() && crate::odt::read(&odt.bytes).is_ok());
}

#[inline(never)]
fn perfci_region_docx_read(bytes: &[u8]) {
    black_box(crate::docx::read(black_box(bytes)).unwrap());
}

/// Inläsning av ett stort .docx (zip/XML till Markdown). Läsningen använder arbetstrådar: hela processen räknas (`_mt`).
#[test]
#[ignore]
fn perfci_docx_read_mt() {
    let b = fixture("stor.docx");
    perfci_region_docx_read(&b);
}

#[inline(never)]
fn perfci_region_odt_read(bytes: &[u8]) {
    black_box(crate::odt::read(black_box(bytes)).unwrap());
}

/// Inläsning av ett stort .odt (tolkningen går på flera trådar, antalet beror på maskinen: jämför bara på samma maskin).
#[test]
#[ignore]
fn perfci_odt_read_mt() {
    let b = fixture("stor.odt");
    perfci_region_odt_read(&b);
}

fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    in_ui(&ctx, || {});
    ctx
}

#[inline(never)]
fn perfci_region_docx_open(ctx: &egui::Context, text: &str) {
    let _count = Counting::on();
    let mut c = layout::Cache::default();
    in_ui(ctx, || {
        black_box(frame(&mut c, ctx, text, 0..0));
    });
}

/// Öppning: analys och kall utläggning med sidindelning av ett stort dokument (första bildrutan).
#[test]
#[ignore]
fn perfci_docx_open() {
    let text = crate::docx::read(&fixture("stor.docx")).unwrap().markdown;
    let ctx = ctx();
    perfci_region_docx_open(&ctx, &text);
}

/// Första tecknet i ett ord i löpande text från `from` och framåt.
fn word_at(text: &str, from: usize) -> usize {
    let mut at = from.min(text.len());
    while !text.is_char_boundary(at) || at == 0 || text.as_bytes()[at - 1] != b' ' || !text[at..].starts_with(|c: char| c.is_alphabetic()) {
        at += 1;
    }
    at
}

#[inline(never)]
fn perfci_region_docx_keys(ctx: &egui::Context, c: &mut layout::Cache, text: &str) {
    let _count = Counting::on();
    in_ui(ctx, || {
        // Ett tecken på fem ställen (varje gång från den oförändrade texten), som `bench_docx_open`.
        let mut places = vec![word_at(text, text.len() / 50), word_at(text, text.len() / 4), word_at(text, text.len() / 2), word_at(text, text.len() * 3 / 4)];
        if let Some(t) = text[text.len() / 2..].find("\n| ") {
            places.push(text.len() / 2 + t + 3);
        }
        for at in places {
            let mut e = text.to_owned();
            e.insert(at, 'x');
            frame(c, ctx, text, 0..0);
            black_box(frame(c, ctx, &e, 0..0));
        }
    });
}

/// Tangenttryck på några ställen i ett stort dokument (inkrementell analys och utläggning).
#[test]
#[ignore]
fn perfci_docx_keys() {
    let text = crate::docx::read(&fixture("stor.docx")).unwrap().markdown;
    let ctx = ctx();
    // Kall utläggning före mätningen: bara tangenttrycken räknas.
    let mut c = layout::Cache::default();
    in_ui(&ctx, || {
        frame(&mut c, &ctx, &text, 0..0);
    });
    perfci_region_docx_keys(&ctx, &mut c, &text);
}

#[inline(never)]
fn perfci_region_docx_search(ctx: &egui::Context, c: &mut layout::Cache, text: &str) {
    let _count = Counting::on();
    let pal = theme::palette(false);
    let spec = search::Spec { query: "och".into(), ..Default::default() };
    let at = word_at(text, text.len() / 2);
    let mut e = text.to_owned();
    let fam = |_: Option<&str>, bold: bool, italic: bool| theme::doc_font(bold, italic);
    for k in 0..8 {
        if k % 2 == 0 {
            e.insert(at, 'x');
        } else {
            e.remove(at);
        }
        in_ui(ctx, || {
            let a = c.analysis(1, &e);
            let r = search::search_doc(&e, &a, &spec);
            let matches = search::highlights(&r, None);
            let geom = pages::Geom::for_sections(1.0, &a.defaults, &a.sections);
            let _ = c.tables(1, &e);
            let _ = c.fonts(1, &e);
            let img = |_: &str| None;
            let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
            let p = layout::Params { palette: &pal, zoom: 1.0, reveal: 0..0, image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
            let g = c.galley_paged_lazy(ctx, 1, &e, &p, geom.edit_w(), Some(&geom), None);
            let y = g.pos_from_cursor(egui::text::CCursor::new(e[..at].chars().count())).min.y;
            black_box(matchlayer::build(&g, &e, &matches, egui::Vec2::ZERO, y - 700.0, y + 700.0, &pal));
        });
    }
}

/// Tangenttryck med sökningen öppen ("och" ger tusentals träffar): sökning, träfflager och utläggning.
#[test]
#[ignore]
fn perfci_docx_search() {
    let text = crate::docx::read(&fixture("stor.docx")).unwrap().markdown;
    let ctx = ctx();
    let mut c = layout::Cache::default();
    in_ui(&ctx, || {
        frame(&mut c, &ctx, &text, 0..0);
    });
    perfci_region_docx_search(&ctx, &mut c, &text);
}

/// PDF-export av ett medelstort dokument: 4 upprepningar av skolrapporten och nyhetsbrevet (fotnoter, tabeller,
/// bilder, spalter). Formningen går i trådar, därför räknas hela processen (`_mt`).
#[test]
#[ignore]
fn perfci_pdf_export_mt() {
    let (md, images) = crate::pdf_bench::big_document(4);
    let base = PathBuf::new();
    let w = crate::pdf_write::write(&md, "Prov", &|src| crate::doc::resolve_image(src, &base, &images)).unwrap();
    assert!(w.bytes.starts_with(b"%PDF"));
}

/// Markdown på ungefär `kb` kilobyte: rubriker, formaterade stycken, listor, kodblock, tabeller och citat.
fn markdown(kb: usize) -> String {
    let mut s = String::from("# Stor fil\n\n");
    let mut i = 0;
    while s.len() < kb * 1024 {
        if i % 12 == 0 {
            s.push_str(&format!("## Avsnitt {}\n\n", i / 12));
        }
        match i % 6 {
            0 => s.push_str(&format!(
                "Stycke {i}. Det var en gång en mycket lång berättelse som aldrig tycktes ta slut, med *kursiv* och **fet** text samt en [länk](https://exempel.se/{i}) och `kod`. \
                 Vi fortsätter att skriva för att fylla ut raderna ordentligt så att varje stycke blir flera rader långt.\n\n"
            )),
            1 => s.push_str(&format!("- punkt {i} med lite text\n- punkt två\n  - under {i}\n- punkt tre\n\n")),
            2 => s.push_str(&format!("```rust\nfn rad_{i}() {{\n    println!(\"hej {i}\");\n}}\n```\n\n")),
            3 => s.push_str(&format!("| Namn | Värde |\n|---|---|\n| a{i} | 1 |\n| b | 2 |\n\n")),
            4 => s.push_str(&format!("- [ ] att göra {i}\n- [x] gjort\n\n")),
            _ => s.push_str(&format!("> Ett citat nummer {i} som löper över\n> två rader.\n\n")),
        }
        i += 1;
    }
    s
}

#[inline(never)]
fn perfci_region_md_read(ui: &mut super::ui_tests::Ui) {
    let _count = Counting::on();
    // Stillastående vid början, rullning (ett hjulsteg per bildruta) och stillastående mitt i filen.
    for _ in 0..10 {
        ui.h.step();
    }
    for _ in 0..10 {
        ui.h.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -90.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        });
        ui.h.step();
    }
    ui.app_mut().md.frac_in = Some(0.5);
    for _ in 0..12 {
        ui.h.step();
    }
}

/// Läsläget för en Markdown-fil på ~400 kB: bildrutor stilla, vid rullning och mitt i filen.
#[test]
#[ignore]
fn perfci_md_read() {
    let mut ui = super::ui_tests::Ui::new();
    let file = ui.dir.write("stor.md", markdown(400));
    ui.open(&file);
    ui.settle();
    assert!(ui.app().md_read_mode());
    perfci_region_md_read(&mut ui);
}
