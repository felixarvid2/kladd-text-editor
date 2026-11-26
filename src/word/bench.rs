//! Prestandamätning för stora dokument (ignorerade tester; kör med
//! `cargo test --release -- --ignored bench --nocapture --test-threads=1`).
//! Genererar ett Markdown-dokument på ~300 sidor och mäter en bildruta i dokumentläget
//! (analys, tabeller, galleri, sidbrytning), stavningskontroll och textlägets färgning.

use std::time::Instant;

use egui::{Color32, FontFamily, FontId};

use super::{layout, markup, pages};
use crate::theme;

/// Deterministisk slumpgenerator (xorshift) så att mätningar och tester är repeterbara.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "dokument", "sida", "stycke", "rubrik", "textbehandling", "redigering", "markör", "tabell", "lista", "bild",
    "snabb", "stor", "liten", "viktig", "enkel", "säker", "program", "fönster", "knapp", "meny", "spara", "öppna",
    "skriva", "läsa", "ändra", "ordna", "visa", "dölja", "hitta", "ersätta", "och", "eller", "men", "att", "som",
    "för", "med", "utan", "under", "över", "mellan", "efter", "före", "genom", "vid", "från", "till", "hos",
];

fn sentence(r: &mut Rng, n: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push(' ');
        }
        let w = WORDS[r.below(WORDS.len())];
        match r.below(14) {
            0 => s.push_str(&format!("**{w}**")),
            1 => s.push_str(&format!("*{w}*")),
            2 => s.push_str(&format!("<u>{w}</u>")),
            _ => s.push_str(w),
        }
    }
    s.push('.');
    s
}

/// Markdown på ungefär `bytes` byte: rubriker, stycken med fet/kursiv, listor och tabeller.
pub fn big_markdown(bytes: usize, seed: u64) -> String {
    let mut r = Rng(seed | 1);
    let mut out = String::with_capacity(bytes + 4096);
    let mut n = 0;
    while out.len() < bytes {
        n += 1;
        if n % 12 == 1 {
            out.push_str(&format!("# Kapitel {n}\n\n"));
        } else if n % 4 == 0 {
            out.push_str(&format!("## Avsnitt {n}\n\n"));
        }
        if n % 9 == 0 {
            for _ in 0..4 {
                out.push_str(&format!("- {}\n", sentence(&mut r, 8)));
            }
            out.push('\n');
        }
        if n % 40 == 0 {
            out.push_str("| Namn | Värde | Anmärkning |\n| --- | --- | --- |\n");
            for _ in 0..5 {
                out.push_str(&format!("| {} | {} | {} |\n", sentence(&mut r, 2), r.below(1000), sentence(&mut r, 4)));
            }
            out.push('\n');
        }
        let len = 6 + r.below(30);
        out.push_str(&sentence(&mut r, len));
        out.push_str("\n\n");
    }
    out
}

fn measure_fn(ctx: &egui::Context) -> impl Fn(&str, FontId) -> f32 + '_ {
    move |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x)
}

fn fam(_: Option<&str>, bold: bool, italic: bool) -> FontFamily {
    theme::doc_font(bold, italic)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// En bildruta i dokumentläget utan UI: det som `word_page` gör före TextEdit.
pub(crate) fn frame(c: &mut layout::Cache, ctx: &egui::Context, text: &str, reveal: std::ops::Range<usize>) -> usize {
    frame_lazy(c, ctx, text, reveal, None)
}

fn frame_lazy(
    c: &mut layout::Cache,
    ctx: &egui::Context,
    text: &str,
    reveal: std::ops::Range<usize>,
    budget: Option<std::time::Duration>,
) -> usize {
    // Som i ett optimerat bygge: inga extra kontroller över hela dokumentet (görs bara i debugbyggen).
    #[cfg(debug_assertions)]
    layout::CHECKS.store(false, std::sync::atomic::Ordering::Relaxed);
    let pal = theme::palette(false);
    let t = Instant::now();
    // Som i appen: texten jämförs med cachens kopia en gång per bildruta (`Cache::seen`).
    let seen = c.seen(1, text);
    let a = c.analysis(1, &seen);
    let t_an = ms(t);
    // Som i appen: sidgeometri per avsnitt (arken kan ha olika storlek och spalter).
    let geom = pages::Geom::for_sections(1.0, &a.defaults, &a.sections);
    let t = Instant::now();
    let _tabs = c.tables(1, &seen);
    let _fonts = c.fonts(1, &seen);
    // `KLADD_BENCH_VIEW=1`: också det vyn hämtar ur cachen varje bildruta (rubriker, styckerutor, bilder, kodblock).
    if std::env::var_os("KLADD_BENCH_VIEW").is_some() {
        let t = Instant::now();
        let _ = c.headings(1, &seen);
        let t_h = ms(t);
        let t = Instant::now();
        let _ = c.para_boxes(1, &seen);
        let t_p = ms(t);
        let t = Instant::now();
        let _ = (c.images(1, &seen), c.code_blocks(1, &seen));
        if std::env::var_os("KLADD_BENCH_VERBOSE").is_some() {
            eprintln!("    vyn: rubriker {t_h:.1} ms, styckerutor {t_p:.1} ms, bilder och kodblock {:.1} ms", ms(t));
        }
    }
    if std::env::var_os("KLADD_BENCH_VERBOSE").is_some() {
        eprintln!(
            "    frame: analys/tillstånd {t_an:.1} ms, tabeller+typsnitt {:.1} ms (inkrementell analys avstod: {:?})",
            ms(t),
            super::incr::WHY.with(|w| w.borrow().last().copied())
        );
    }
    let img = |_: &str| None;
    let measure = measure_fn(ctx);
    let p = layout::Params {
        palette: &pal,
        zoom: 1.0,
        reveal,
        image_size: &img,
        measure: &measure,
        font_family: &fam,
        fonts_gen: 0,
    };
    let g = c.galley_paged_lazy(ctx, 1, &seen, &p, geom.edit_w(), Some(&geom), budget);
    // Som statusraden: under stegvis utläggning den uppräknade uppskattningen (0 = ingen än), annars det riktiga antalet.
    let boxes = c.page_boxes(1, &seen);
    if c.partial(1) {
        c.estimated_pages(1, text.len(), &g, &boxes).unwrap_or(0)
    } else if boxes.is_empty() {
        pages::page_count(&g, &geom)
    } else {
        boxes.len()
    }
}

pub(crate) fn in_ui(ctx: &egui::Context, f: impl FnOnce()) {
    let mut f = Some(f);
    let out = ctx.run_ui(Default::default(), |_| {
        if let Some(f) = f.take() {
            f();
        }
    });
    std::mem::forget(out);
}

#[test]
#[ignore]
fn bench_document_mode() {
    let bytes = std::env::var("KLADD_BENCH_MD_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(600_000);
    let text = big_markdown(bytes, 7);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    // Värm upp typsnitten.
    in_ui(&ctx, || {});
    let mut c = layout::Cache::default();
    let mid = text[..text.len() / 2].rfind(' ').unwrap();
    let mut edited = text.clone();
    edited.insert(mid, 'x');
    let mut results: Vec<(&str, f64)> = Vec::new();
    let mut pages_n = 0;
    in_ui(&ctx, || {
        let t = Instant::now();
        let a = markup::analyze(&text);
        results.push(("analys (kall)", ms(t)));
        drop(a);

        let t = Instant::now();
        pages_n = frame(&mut c, &ctx, &text, 0..0);
        results.push(("(a) kall full layout", ms(t)));

        let t = Instant::now();
        frame(&mut c, &ctx, &edited, 0..0);
        results.push(("(b) ett tecken in i mitten", ms(t)));

        let t = Instant::now();
        for _ in 0..10 {
            frame(&mut c, &ctx, &edited, 0..0);
        }
        results.push(("(c) oförändrad bildruta", ms(t) / 10.0));

        let r1 = mid..mid + 1;
        let t = Instant::now();
        frame(&mut c, &ctx, &edited, r1);
        results.push(("(b2) markören byter rad", ms(t)));
    });
    println!("--- dokumentläge: {} KB, {} sidor", text.len() / 1000, pages_n);
    for (n, v) in results {
        println!("{n:<32} {v:>10.1} ms");
    }
}

#[test]
#[ignore]
fn bench_spell() {
    let dir = std::env::temp_dir().join(format!("kladd-bench-spell-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let words: Vec<&str> = WORDS.iter().copied().take(WORDS.len() - 4).collect();
    std::fs::write(dir.join("xx_XX.aff"), "SET UTF-8\n").unwrap();
    std::fs::write(dir.join("xx_XX.dic"), format!("{}\n{}\n", words.len(), words.join("\n"))).unwrap();
    let mut sp = crate::spell::Speller::with_paths(vec![dir.clone()], dir.join("u.txt"), Some("xx_XX"));
    let t = Instant::now();
    while !sp.ready() {
        assert!(t.elapsed().as_secs() < 10);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let text = big_markdown(600_000, 7);
    let a = markup::analyze(&text);
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for pc in &a.pieces {
        if pc.markup || pc.code || pc.link || pc.image.is_some() {
            continue;
        }
        match ranges.last_mut() {
            Some(l) if l.end == pc.range.start => l.end = pc.range.end,
            _ => ranges.push(pc.range.clone()),
        }
    }
    let t = Instant::now();
    let found = sp.misspelled(&text, &ranges);
    println!("--- stavning: kall (inkl. ordcache) {:.1} ms, {} träffar", ms(t), found.len());
    let t = Instant::now();
    let found = sp.misspelled(&text, &ranges);
    println!("--- stavning: varm ordcache         {:.1} ms, {} träffar", ms(t), found.len());
    // Styckecachen: kall, sedan efter ett tecken i mitten.
    let mut per = std::collections::HashMap::new();
    let t = Instant::now();
    sp.misspelled_cached(&text, &ranges, &mut per);
    println!("--- stavning: stycke-cache, kall    {:.1} ms", ms(t));
    let mut edited = text.clone();
    let mid = text[..text.len() / 2].rfind(' ').unwrap();
    edited.insert(mid, 'x');
    let a2 = markup::analyze(&edited);
    let mut ranges2: Vec<std::ops::Range<usize>> = Vec::new();
    for pc in &a2.pieces {
        if pc.markup || pc.code || pc.link || pc.image.is_some() {
            continue;
        }
        match ranges2.last_mut() {
            Some(l) if l.end == pc.range.start => l.end = pc.range.end,
            _ => ranges2.push(pc.range.clone()),
        }
    }
    let t = Instant::now();
    let f2 = sp.misspelled_cached(&edited, &ranges2, &mut per);
    println!("--- stavning: stycke-cache, ett tecken {:.1} ms (spann + hash + sammanfogning), {} träffar", ms(t), f2.len());
    assert_eq!(f2, sp.misspelled(&edited, &ranges2));
}

#[test]
#[ignore]
fn bench_text_mode() {
    let text = big_markdown(300_000, 9);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    in_ui(&ctx, || {});
    let pal = theme::palette(true);
    let mut hl = crate::highlight::Highlighter::new();
    let mid = text[..text.len() / 2].rfind(' ').unwrap();
    let mut edited = text.clone();
    edited.insert(mid, 'x');
    let mut res: Vec<(&str, f64)> = Vec::new();
    let mut nrows = 0;
    in_ui(&ctx, || {
        let run = |hl: &mut crate::highlight::Highlighter, t: &str| {
            let p = crate::highlight::Params {
                doc_id: 1,
                syntax_hint: "md",
                palette: &pal,
                font_size: 14.0,
                matches: &[],
            };
            let g = hl.galley(&ctx, t, &p, 800.0);
            g.rows.len()
        };
        let t = Instant::now();
        nrows = run(&mut hl, &text);
        res.push(("(e) färgning + layout, kall", ms(t)));
        let t = Instant::now();
        run(&mut hl, &edited);
        res.push(("(e) efter ett tecken", ms(t)));
        let t = Instant::now();
        for _ in 0..10 {
            run(&mut hl, &edited);
        }
        res.push(("(e) oförändrad bildruta", ms(t) / 10.0));
    });
    println!("--- textläge: {} KB, {} rader", text.len() / 1000, nrows);
    for (n, v) in res {
        println!("{n:<32} {v:>10.1} ms");
    }
}

/// Skriver det genererade dokumentet till `KLADD_BIG_MD` (för att prova i appen).
#[test]
#[ignore]
fn bench_write_big_markdown() {
    if let Ok(p) = std::env::var("KLADD_BIG_MD") {
        std::fs::write(p, big_markdown(600_000, 7)).unwrap();
    }
}

/// Bara läsningen (zip/XML till Markdown) av `KLADD_BENCH_DOCX` (.docx eller .odt), `KLADD_BENCH_READS` gånger
/// (standard 9): median och minimum i ms. Samma som första raden i `bench_docx_open`, men utan resten.
#[test]
#[ignore]
fn bench_docx_read() {
    let Ok(path) = std::env::var("KLADD_BENCH_DOCX") else { return };
    let n: usize = std::env::var("KLADD_BENCH_READS").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    let bytes = std::fs::read(&path).unwrap();
    // KLADD_BENCH_MAKE_ODT=fil: skriv dokumentet som .odt (ett stort .odt att mäta på) och avsluta.
    if let Ok(out) = std::env::var("KLADD_BENCH_MAKE_ODT") {
        let l = crate::docx::read(&bytes).unwrap();
        let w = crate::odt::write(&l.markdown, "Bänk", &|src| crate::doc::resolve_image(src, std::path::Path::new(""), &l.images)).unwrap();
        std::fs::write(out, &w.bytes).unwrap();
        return;
    }
    let mut t: Vec<f64> = (0..n)
        .map(|_| {
            let t = Instant::now();
            let l = if path.ends_with(".odt") { crate::odt::read(&bytes) } else { crate::docx::read(&bytes) }.unwrap();
            let d = ms(t);
            drop(l);
            d
        })
        .collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!("läsning: median {:.1} ms, minst {:.1} ms ({n} körningar, {} kB)", t[t.len() / 2], t[0], bytes.len() / 1024);
}

/// Öppning av ett stort Word-dokument (`KLADD_BENCH_DOCX=sökväg`, se docs/dev/prestanda.md):
/// läsning, analys, stavning, utläggning (med delsteg), en redigering och en oförändrad bildruta.
#[test]
#[ignore]
fn bench_docx_open() {
    let Ok(path) = std::env::var("KLADD_BENCH_DOCX") else { return };
    // KLADD_BENCH_NO_INCR=1: full analys vid varje ändring (som före den inkrementella analysen för Word-dokument).
    super::incr::DISABLED.store(std::env::var_os("KLADD_BENCH_NO_INCR").is_some(), std::sync::atomic::Ordering::Relaxed);
    let bytes = std::fs::read(&path).unwrap();
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    in_ui(&ctx, || {});
    let mut res: Vec<(String, f64)> = Vec::new();
    let t = Instant::now();
    let loaded = if path.ends_with(".odt") { crate::odt::read(&bytes) } else { crate::docx::read(&bytes) }.unwrap();
    res.push(("läsning (zip/XML till Markdown)".into(), ms(t)));
    let text = loaded.markdown;
    if let Ok(p) = std::env::var("KLADD_BENCH_DUMP") {
        std::fs::write(p, &text).unwrap();
    }
    let t = Instant::now();
    let a = markup::analyze(&text);
    res.push(("markup-analys".into(), ms(t)));
    drop(a);
    // Stavning: systemets svenska ordlista om den finns, annars en med dokumentets alla ord.
    let dir = std::env::temp_dir().join(format!("kladd-bench-docx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sys = std::path::Path::new("/usr/share/hunspell");
    let (paths, lang) = if sys.join("sv_SE.dic").exists() {
        (vec![sys.to_path_buf()], "sv_SE")
    } else {
        let mut words: Vec<&str> = text.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).collect();
        words.sort_unstable();
        words.dedup();
        std::fs::write(dir.join("xx_XX.aff"), "SET UTF-8\n").unwrap();
        std::fs::write(dir.join("xx_XX.dic"), format!("{}\n{}\n", words.len(), words.join("\n"))).unwrap();
        (vec![dir.clone()], "xx_XX")
    };
    let mut sp = crate::spell::Speller::with_paths(paths, dir.join("u.txt"), Some(lang));
    let t0 = Instant::now();
    while !sp.ready() {
        assert!(t0.elapsed().as_secs() < 60);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    res.push(("(stavning: ordlistan laddad i bakgrunden)".into(), ms(t0)));
    let a = markup::analyze(&text);
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for pc in &a.pieces {
        if pc.markup || pc.code || pc.link || pc.image.is_some() || matches!(pc.block, markup::Block::Code | markup::Block::Table) {
            continue;
        }
        match spans.last_mut() {
            Some(l) if l.end == pc.range.start => l.end = pc.range.end,
            _ => spans.push(pc.range.clone()),
        }
    }
    let mut per = std::collections::HashMap::new();
    let t = Instant::now();
    sp.misspelled_cached(&text, &spans, &mut per);
    res.push(("stavning (kall, hela dokumentet)".into(), ms(t)));
    let t = Instant::now();
    sp.misspelled_cached(&text, &spans, &mut per);
    res.push(("stavning (varm, oförändrad text)".into(), ms(t)));
    let mut c = layout::Cache::default();
    let mut pages_n = 0;
    let mid = text[..text.len() / 2].rfind(' ').unwrap();
    let mut edited = text.clone();
    edited.insert(mid, 'x');
    let mut first = text.clone();
    // Första stycket efter rubrikraden/standardraden (en ändring på rad 1 ändrar dokumentets grundformat).
    let at = text[text.find("\n\n").unwrap() + 2..].find(' ').unwrap() + text.find("\n\n").unwrap() + 2;
    first.insert(at, 'x');
    let mut stats = Vec::new();
    in_ui(&ctx, || {
        let t = Instant::now();
        pages_n = frame(&mut c, &ctx, &text, 0..0);
        res.push(("utläggning + sidindelning, kall (bildruta 1)".into(), ms(t)));
        stats.push(("kall", c.stats(1)));
        // Ett tecken på tio ställen i dokumentet (varje gång från den oförändrade texten): början, 25 %, mitten, 75 %,
        // slutet av brödtexten, i en tabell, i ett stycke med fotnotsreferens, i stycket närmast fotnotsdefinitionerna
        // och i första respektive sista fotnotsdefinitionen.
        let places = places(&text);
        let mut times = Vec::new();
        for (k, (name, at)) in places.iter().enumerate() {
            let mut e = text.clone();
            e.insert(*at, 'x');
            frame(&mut c, &ctx, &text, 0..0);
            let t = Instant::now();
            frame(&mut c, &ctx, &e, 0..0);
            times.push(ms(t));
            stats.push(("tecken", c.stats(1)));
            if std::env::var_os("KLADD_BENCH_VERBOSE").is_some() || std::env::var_os("KLADD_BENCH_PLACES").is_some() {
                let st = c.stats(1);
                eprintln!(
                    "    ställe {} ({name}): {:.1} ms, återanvänd {}, skäl {}, återupptagen {} ({}) | plan {:.1} loop {:.1} concat {:.1} sidor {:.1} ms",
                    k + 1,
                    times.last().unwrap(),
                    st.reused,
                    st.memo_why,
                    st.resumed,
                    st.resume_why,
                    st.plan_ms,
                    st.loop_ms,
                    st.concat_ms,
                    st.pages_ms
                );
            }
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        res.push((format!("tangenttryck: median av {} ställen (hela bildrutan)", times.len()), times[times.len() / 2]));
        res.push((format!("tangenttryck: sämsta av {} ställen", times.len()), times[times.len() - 1]));
        let _ = (&edited, &first);
        frame(&mut c, &ctx, &text, 0..0);
        let t = Instant::now();
        for _ in 0..10 {
            frame(&mut c, &ctx, &text, 0..0);
        }
        res.push(("oförändrad bildruta".into(), ms(t) / 10.0));
    });
    // Stegvis utläggning som i appen: budget per bildruta enligt `lazy_budget`, ny cache och nytt
    // sammanhang (typsnittsatlasen är kall, som vid öppning).
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    in_ui(&ctx, || {});
    let mut c2 = layout::Cache::default();
    let (mut frames, mut worst, mut first, mut est) = (0u32, 0.0f64, 0.0f64, 0usize);
    let mut ests: Vec<usize> = Vec::new();
    let t_all = Instant::now();
    in_ui(&ctx, || loop {
        let t = Instant::now();
        let b = layout::lazy_budget(text.len(), frames, false, false);
        let n = frame_lazy(&mut c2, &ctx, &text, 0..0, b);
        let d = ms(t);
        if frames == 0 {
            first = d;
            est = n;
        }
        ests.push(n);
        worst = worst.max(d);
        frames += 1;
        if !c2.partial(1) {
            break;
        }
    });
    let all_ms = ms(t_all);
    res.push((format!("stegvis: första bildrutan (uppskattat {est} sidor, 0 = ingen än)"), first));
    let spread = |v: &[usize]| v.iter().copied().filter(|n| *n > 0).fold((usize::MAX, 0), |(lo, hi), n| (lo.min(n), hi.max(n)));
    res.push((format!("stegvis: uppskattat sidantal {:?} (sista {})", spread(&ests), ests.last().copied().unwrap_or(0)), 0.0));
    res.push((format!("stegvis: längsta bildrutan ({frames} bildrutor)"), worst));
    res.push(("stegvis: summa tills allt är utlagt".into(), all_ms));
    println!("--- {path}: {} KB markdown, {} sidor", text.len() / 1000, pages_n);
    for (n, v) in res {
        println!("{n:<48} {v:>10.1} ms");
    }
    for (n, s) in stats.into_iter().take(2) {
        println!(
            "  {n:<18} träffar {:>6} missar {:>6} varv {} | plan {:.0} loop {:.0} concat {:.0} sidor {:.0} ms",
            s.hits, s.misses, s.varv, s.plan_ms, s.loop_ms, s.concat_ms, s.pages_ms
        );
        println!("    återanvänd förberedelse: {}", s.reused);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ställena för tangenttrycken i `bench_docx_open` (se där).
fn places(text: &str) -> Vec<(&'static str, usize)> {
    // Första tecknet i ett ord i löpande text (inte i markup eller en etikett).
    let word_at = |from: usize| -> usize {
        let mut at = from.min(text.len());
        while !text.is_char_boundary(at) || at == 0 || text.as_bytes()[at - 1] != b' ' || !text[at..].starts_with(|c: char| c.is_alphabetic()) {
            at += 1;
        }
        at
    };
    let first_def = text.find("\n[^1]:").unwrap_or(text.len());
    let mut places: Vec<(&str, usize)> = vec![
        ("början", word_at(text.len() / 50)),
        ("25 %", word_at(text.len() / 4)),
        ("mitten", word_at(text.len() / 2)),
        ("75 %", word_at(text.len() * 3 / 4)),
        ("slutet av brödtexten", word_at(first_def.saturating_sub(3000))),
    ];
    if let Some(t) = text[text.len() / 2..].find("\n| ") {
        places.push(("i tabell", text.len() / 2 + t + 3));
    }
    if let Some(r) = text[text.len() * 2 / 5..].find("[^") {
        places.push(("vid fotnotsreferens", text.len() * 2 / 5 + r));
    }
    places.push(("närmast fotnotsdefinitionerna", word_at(first_def.saturating_sub(300))));
    if first_def < text.len() {
        places.push(("första fotnotsdefinitionen", word_at(first_def + 10)));
        places.push(("sista fotnotsdefinitionen", word_at(text.rfind("\n[^").unwrap_or(0) + 10)));
    }
    places
}

/// Ett tecken in och ut om vartannat på ett av ställena i `bench_docx_open` (`KLADD_BENCH_PLACE=1..10`), många gånger
/// (för profilering: `--toggle-collect='*place_loop*'`). `KLADD_BENCH_TEXT=sträng` skriver den i stället för ett tecken.
#[test]
#[ignore]
fn bench_docx_place() {
    let Ok(path) = std::env::var("KLADD_BENCH_DOCX") else { return };
    let n: usize = std::env::var("KLADD_BENCH_KEYS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let k: usize = std::env::var("KLADD_BENCH_PLACE").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let ins = std::env::var("KLADD_BENCH_TEXT").unwrap_or_else(|_| "x".into());
    let bytes = std::fs::read(&path).unwrap();
    let mut text = crate::docx::read(&bytes).unwrap().markdown;
    // `KLADD_BENCH_PRE=text`: ett stycke med den texten först i dokumentet (efter grundformatsraden), t.ex. ett kodstaket
    // mitt i en rad eller en referenslänk, som påverkar om analysen kan göras inkrementellt.
    if let Ok(pre) = std::env::var("KLADD_BENCH_PRE") {
        let at = text.find("\n\n").map_or(0, |i| i + 2);
        text.insert_str(at, &format!("{pre}\n\n"));
    }
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    in_ui(&ctx, || {});
    let mut c = layout::Cache::default();
    in_ui(&ctx, || {
        frame(&mut c, &ctx, &text, 0..0);
        // `KLADD_BENCH_FIND=text`: skriv efter första förekomsten av texten i stället för på ett av ställena.
        let (name, at) = match std::env::var("KLADD_BENCH_FIND").ok().and_then(|f| text.find(&f).map(|i| i + f.len())) {
            Some(at) => ("KLADD_BENCH_FIND", at),
            None => places(&text)[k - 1],
        };
        let mut e = text.clone();
        e.insert_str(at, &ins);
        // En ändring och tillbaka först: förekomsterna för den inkrementella analysen (`incr::Facts`) räknas en gång för
        // hela texten efter en full analys, det hör inte till tangenttrycken.
        frame(&mut c, &ctx, &e, 0..0);
        frame(&mut c, &ctx, &text, 0..0);
        let mut times = Vec::new();
        place_loop(&mut c, &ctx, &text, &e, n, &mut times);
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let st = c.stats(1);
        eprintln!(
            "ställe {k} ({name}), {n} gånger: median {:.2} ms, bästa {:.2} ms | förberedelsen återanvänd {} ({}) | återupptagen {} ({}) | plan {:.1} loop {:.1} concat {:.1} sidor {:.1} ms",
            times[times.len() / 2],
            times[0],
            st.reused,
            st.memo_why,
            st.resumed,
            st.resume_why,
            st.plan_ms,
            st.loop_ms,
            st.concat_ms,
            st.pages_ms
        );
    });
}

#[inline(never)]
fn place_loop(c: &mut layout::Cache, ctx: &egui::Context, text: &str, edited: &str, n: usize, times: &mut Vec<f64>) {
    for k in 0..n {
        let t = Instant::now();
        frame(c, ctx, if k % 2 == 0 { edited } else { text }, 0..0);
        times.push(ms(t));
    }
}

/// Bara tangenttryck i mitten av ett stort Word-dokument, många gånger (för profilering med `valgrind --tool=callgrind`):
/// `KLADD_BENCH_DOCX=sökväg KLADD_BENCH_KEYS=antal`.
#[test]
#[ignore]
fn bench_docx_keys() {
    let Ok(path) = std::env::var("KLADD_BENCH_DOCX") else { return };
    let n: usize = std::env::var("KLADD_BENCH_KEYS").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let bytes = std::fs::read(&path).unwrap();
    let text = crate::docx::read(&bytes).unwrap().markdown;
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    in_ui(&ctx, || {});
    let mut c = layout::Cache::default();
    in_ui(&ctx, || {
        frame(&mut c, &ctx, &text, 0..0);
        let mut at = text.len() / 2;
        while !text.is_char_boundary(at) || at == 0 || text.as_bytes()[at - 1] != b' ' || !text[at..].starts_with(|c: char| c.is_alphabetic()) {
            at += 1;
        }
        let t = Instant::now();
        keys_loop(&mut c, &ctx, &text, at, n);
        eprintln!("{n} tangenttryck: {:.2} ms per bildruta", ms(t) / n as f64);
    });
}

/// Tangenttrycken i `bench_docx_keys`, i en egen funktion så att profileringen kan begränsas till dem
/// (`valgrind --tool=callgrind --collect-atstart=no --toggle-collect='*keys_loop*'`).
#[inline(never)]
fn keys_loop(c: &mut layout::Cache, ctx: &egui::Context, text: &str, at: usize, n: usize) {
    let mut e = text.to_owned();
    for k in 0..n {
        // Tecknet in och ut om vartannat: texten ändras varje bildruta.
        if k % 2 == 0 {
            e.insert(at, 'x');
        } else {
            e.remove(at);
        }
        frame(c, ctx, &e, 0..0);
    }
}
