//! Prestandamätning för mycket stora filer (ignorerade tester; se `docs/dev/prestanda.md`):
//! `cargo test --release -- --ignored bigbench --nocapture --test-threads=1`
//! (eller en optimerad testprofil i egen målkatalog). Storlekar styrs med miljövariablerna
//! `KLADD_BENCH_MB` (textfil, standard 100), `KLADD_BENCH_LINES` (kodfil, standard 1000000),
//! `KLADD_BENCH_HEADINGS` (Markdown, standard 50000) och `KLADD_BENCH_PAGES` (dokument, standard 2000).

use std::time::Instant;

use crate::word::bench::{Rng, big_markdown};

pub fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Resident minne (och toppen) i MB enligt /proc, 0 där det inte finns.
fn rss_mb() -> (f64, f64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
            .map_or(0.0, |kb| kb / 1024.0)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// Loggfil på ungefär `bytes` byte.
pub fn gen_log(bytes: usize) -> String {
    let mut r = Rng(0x5eed);
    let levels = ["INFO", "DEBUG", "WARN", "ERROR"];
    let mut out = String::with_capacity(bytes + 256);
    let mut n = 0u64;
    while out.len() < bytes {
        n += 1;
        out.push_str(&format!(
            "2026-03-{:02}T{:02}:{:02}:{:02}.{:03}Z {} svc-{}[{}]: förfrågan id={:08x} tog {} ms status={} användare=åsa\n",
            1 + r.below(28),
            r.below(24),
            r.below(60),
            r.below(60),
            r.below(1000),
            levels[r.below(4)],
            r.below(20),
            1000 + r.below(9000),
            r.next() as u32,
            r.below(900),
            [200, 200, 200, 404, 500][r.below(5)],
        ));
        if n % 1000 == 0 {
            out.push_str("    stack: at main::handler (src/main.rs:42)\n");
        }
    }
    out
}

/// Rust-liknande kodfil med `lines` rader.
pub fn gen_code(lines: usize) -> String {
    let mut out = String::with_capacity(lines * 40);
    let mut n = 0;
    let mut f = 0;
    while n < lines {
        f += 1;
        out.push_str(&format!("/// Funktion nummer {f}.\npub fn funktion_{f}(a: u32, b: u32) -> u32 {{\n"));
        for k in 0..8 {
            out.push_str(&format!("    let v{k} = a.wrapping_mul({k}) ^ b; // beräkning {k}\n"));
        }
        out.push_str("    v0 + v7\n}\n\n");
        n += 13;
    }
    out
}

/// Markdown med `n` rubriker (och ett kort stycke under varje).
pub fn gen_headings(n: usize) -> String {
    let mut r = Rng(0xbeef);
    let mut out = String::with_capacity(n * 120);
    for i in 0..n {
        let level = 1 + (i % 3);
        out.push_str(&format!("{} Rubrik {i}\n\nEtt kort stycke under rubrik {i} med lite text {}.\n\n", "#".repeat(level), r.below(1000)));
    }
    out
}

#[test]
fn generators_produce_expected_sizes() {
    let log = gen_log(100_000);
    assert!(log.len() >= 100_000 && log.len() < 101_000);
    assert!(log.lines().count() > 500);
    let code = gen_code(1300);
    assert!(code.lines().count() >= 1300);
    let md = gen_headings(100);
    assert_eq!(md.lines().filter(|l| l.starts_with('#')).count(), 100);
}

/// Textläge FÖRE: det gamla `TextEdit`-flödet (färgning av/på + layout av hela texten).
/// Körs på små storlekar; resultatet skalar linjärt, 100 MB får inte plats i minnet.
#[test]
#[ignore]
fn bigbench_text_old_path() {
    use crate::highlight::{Highlighter, Params};
    for mb in [1usize, 5, 20] {
        let text = gen_log(mb * 1_000_000);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let pal = crate::theme::palette(true);
        let mut hl = Highlighter::new();
        let (rss0, _) = rss_mb();
        let mut cold = 0.0;
        let mut typing = 0.0;
        let mut idle = 0.0;
        let mut rows = 0;
        let out = ctx.run_ui(Default::default(), |_| {
            let run = |hl: &mut Highlighter, t: &str| {
                let p = Params { doc_id: 1, syntax_hint: "log", palette: &pal, font_size: 14.0, matches: &[] };
                hl.galley(&ctx, t, &p, f32::INFINITY).rows.len()
            };
            let t = Instant::now();
            rows = run(&mut hl, &text);
            cold = ms(t);
            let mut edited = text.clone();
            edited.push('x');
            let t = Instant::now();
            run(&mut hl, &edited);
            typing = ms(t);
            let t = Instant::now();
            for _ in 0..5 {
                run(&mut hl, &edited);
            }
            idle = ms(t) / 5.0;
        });
        std::mem::forget(out);
        let (rss1, hwm) = rss_mb();
        println!(
            "FÖRE textläge {mb:>3} MB: {rows} layoutrader, öppna+layout {cold:.0} ms, tecken i slutet {typing:.0} ms, orörd ruta {idle:.1} ms, minne +{:.0} MB (topp {hwm:.0})",
            rss1 - rss0
        );
    }
}

/// Textläge EFTER: den virtualiserade vyn på stora filer.
#[test]
#[ignore]
fn bigbench_text_new_path() {
    use crate::bigtext::{BigView, Motion, Opts};
    let mb = env_num("KLADD_BENCH_MB", 100);
    for (name, text) in [
        (format!("logg {mb} MB"), gen_log(mb * 1_000_000)),
        (format!("kod {} rader", env_num("KLADD_BENCH_LINES", 1_000_000)), gen_code(env_num("KLADD_BENCH_LINES", 1_000_000))),
    ] {
        let dir = std::env::temp_dir().join(format!("kladd-bigbench-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stor.txt");
        std::fs::write(&path, &text).unwrap();
        drop(text);
        let (rss0, _) = rss_mb();

        let t = Instant::now();
        let mut doc = crate::doc::Document::load(1, &path).unwrap();
        let load = ms(t);
        let t = Instant::now();
        let mut view = BigView::new(&doc.text);
        let index = ms(t);
        let (rss1, hwm) = rss_mb();

        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let pal = crate::theme::palette(true);
        let id = egui::Id::unique("bench-big");
        let mut frame_ms = Vec::new();
        let mut type_ms = Vec::new();
        let find;
        let mut save = 0.0;
        let frame = |ctx: &egui::Context, doc: &mut crate::doc::Document, view: &mut BigView, focus: bool| -> f64 {
            let t = Instant::now();
            let out = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))), ..Default::default() }, |ui| {
                let o = Opts { id, palette: &pal, font_size: 14.0, line_numbers: true, matches: &[], focus };
                egui::CentralPanel::default().show(ui, |ui| {
                    view.show(ui, &mut doc.text, &o);
                });
            });
            if std::env::var("KLADD_BENCH_SHAPES").is_ok() {
                println!("   former: {}", out.shapes.len());
            }
            std::mem::forget(out);
            ms(t)
        };
        frame(&ctx, &mut doc, &mut view, true); // värm upp typsnitt
        for _ in 0..20 {
            frame_ms.push(frame(&ctx, &mut doc, &mut view, false));
        }
        // Gå till slutet och skriv där.
        view.go(&doc.text, Motion::DocEnd, false);
        frame(&ctx, &mut doc, &mut view, false);
        for i in 0..50 {
            let t = Instant::now();
            view.insert(&mut doc.text, if i % 10 == 9 { "\n" } else { "x" });
            doc.on_edited();
            let f = frame(&ctx, &mut doc, &mut view, false);
            type_ms.push(ms(t).max(f));
        }
        // Skriv i mitten av filen (ändring + radindex-uppdatering + ritning).
        let mid = doc.text.len() / 2;
        let mid = (mid..doc.text.len()).find(|&i| doc.text.is_char_boundary(i)).unwrap();
        view.select(mid, mid);
        let t = Instant::now();
        view.insert(&mut doc.text, "y");
        let f = frame(&ctx, &mut doc, &mut view, false);
        let mid_typing = ms(t).max(f);

        // Sök: tid till första träffarna och till hela sökningen.
        {
            let mut st = crate::bigfiles::State::default();
            let spec = crate::search::Spec { query: "status=500".into(), ..Default::default() };
            let big = crate::bigtext::is_big(&doc.text);
            assert!(big);
            let _ = st.is_big(&doc);
            let t = Instant::now();
            let first = st.search(&doc, &spec).matches.len();
            let first_ms = ms(t);
            let mut n = first;
            let mut capped = false;
            while st.scanning() {
                let r = st.search(&doc, &spec);
                n = r.matches.len();
                capped = r.partial && n >= crate::bigfiles::MAX_MATCHES;
            }
            let total = ms(t);
            find = (total, n, capped);
            println!("   sök: första bildrutan {first_ms:.1} ms ({first} träffar)");
        }
        let t = Instant::now();
        let target = dir.join("ut.txt");
        doc.save_to(&target).unwrap();
        save += ms(t);

        let mut sorted = frame_ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut ts = type_ms.clone();
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (rss2, hwm2) = rss_mb();
        println!(
            "EFTER {name}: läs {load:.0} ms + index/ord {index:.0} ms, bildruta median {:.2} / max {:.2} ms, tecken i slutet median {:.2} / max {:.2} ms, tecken i mitten {mid_typing:.1} ms, sök hela {:.0} ms ({} träffar{}), spara {save:.0} ms",
            sorted[sorted.len() / 2],
            sorted[sorted.len() - 1],
            ts[ts.len() / 2],
            ts[ts.len() - 1],
            find.0,
            find.1,
            if find.2 { ", taket nått" } else { "" },
        );
        println!(
            "   minne efter öppning +{:.0} MB (topp {hwm:.0}), efter redigering +{:.0} MB (topp {hwm2:.0}), filstorlek {} MB, {} rader",
            rss1 - rss0,
            rss2 - rss0,
            doc.text.len() / 1_000_000,
            view.lines()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Markdown med 50 000 rubriker: öppning, navigeringsruta (rubriklistan) och dokumentlägets analys.
#[test]
#[ignore]
fn bigbench_headings() {
    let n = env_num("KLADD_BENCH_HEADINGS", 50_000);
    let text = gen_headings(n);
    let t = Instant::now();
    let a = crate::word::markup::analyze(&text);
    let analyze = ms(t);
    drop(a);
    let t = Instant::now();
    let heads = crate::word::nav::headings_scan(&text);
    let nav = ms(t);
    println!("RUBRIKER {n} st, {} KB: analys {analyze:.0} ms, navigeringsrutans rubriklista {nav:.0} ms ({} rubriker)", text.len() / 1000, heads.len());
}

/// Dokumentläge: ett dokument på `KLADD_BENCH_PAGES` sidor, skriv på sida 1500.
#[test]
#[ignore]
fn bigbench_document_pages() {
    use crate::word::{layout, pages};
    let target = env_num("KLADD_BENCH_PAGES", 2000);
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let pal = crate::theme::palette(false);
    let mut bytes = 600_000usize;
    let mut text;
    let mut c = layout::Cache::default();
    let mut npages;
    let measure = |ctx: &egui::Context| {
        let ctx = ctx.clone();
        move |s: &str, f: egui::FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, egui::Color32::WHITE).size().x)
    };
    let fam = |_: Option<&str>, bold: bool, italic: bool| crate::theme::doc_font(bold, italic);
    let img = |_: &str| None;
    let run_frame = |c: &mut layout::Cache, text: &str, reveal: std::ops::Range<usize>| -> (usize, f64) {
        let mut pn = 0;
        let mut dt = 0.0;
        let out = ctx.run_ui(Default::default(), |_| {
            let t = Instant::now();
            let a = c.analysis(1, text);
            let t_an = ms(t);
            let geom = pages::Geom::new(1.0, &a.defaults);
            let _t = c.tables(1, text);
            let _f = c.fonts(1, text);
            let t_tf = ms(t) - t_an;
            let m = measure(&ctx);
            let p = layout::Params {
                palette: &pal,
                zoom: 1.0,
                reveal: reveal.clone(),
                matches: &[],
                image_size: &img,
                measure: &m,
                font_family: &fam,
                fonts_gen: 0,
            };
            let g = c.galley_paged(&ctx, 1, text, &p, geom.text_w(), Some(&geom));
            let t_lay = ms(t) - t_an - t_tf;
            pn = pages::page_count(&g, &geom);
            dt = ms(t);
            if std::env::var("KLADD_BENCH_STAGES").is_ok() {
                println!("   steg: analys {t_an:.0}, tabeller+typsnitt {t_tf:.0}, layout {t_lay:.0}, sidräkning {:.0}", dt - t_an - t_tf - t_lay);
            }
        });
        std::mem::forget(out);
        (pn, dt)
    };
    // Kalibrera: sidor per byte.
    text = big_markdown(bytes, 7);
    let (p0, _) = run_frame(&mut layout::Cache::default(), &text, 0..0);
    bytes = (bytes as f64 * target as f64 / p0 as f64) as usize;
    text = big_markdown(bytes, 7);
    let (rss0, _) = rss_mb();
    let (pn, cold) = run_frame(&mut c, &text, 0..0);
    npages = pn;
    let (rss1, hwm) = rss_mb();
    // Tecken på sida 1500 (eller 3/4 in): sök byteläge.
    let at = text.len() * 3 / 4;
    let at = (at..text.len()).find(|&i| text.is_char_boundary(i) && text[..i].ends_with(' ')).unwrap();
    let mut edited = text.clone();
    let mut samples = Vec::new();
    let mut pos = at;
    for _ in 0..20 {
        edited.insert(pos, 'x');
        pos += 1;
        let (_, d) = run_frame(&mut c, &edited, pos..pos);
        samples.push(d);
    }
    let (_, idle) = run_frame(&mut c, &edited, pos..pos);
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    npages = npages.max(1);
    println!(
        "DOKUMENT {npages} sidor, {} KB: kall layout {cold:.0} ms, tecken på sida ~{} median {:.1} / max {:.1} ms, orörd ruta {idle:.1} ms, minne +{:.0} MB (topp {hwm:.0})",
        text.len() / 1000,
        npages * 3 / 4,
        samples[samples.len() / 2],
        samples[samples.len() - 1],
        rss1 - rss0
    );
}
