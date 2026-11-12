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
fn frame(c: &mut layout::Cache, ctx: &egui::Context, text: &str, reveal: std::ops::Range<usize>) -> usize {
    let pal = theme::palette(false);
    let a = c.analysis(1, text);
    let geom = pages::Geom::new(1.0, &a.defaults);
    let _tabs = c.tables(1, text);
    let _fonts = c.fonts(1, text);
    let img = |_: &str| None;
    let measure = measure_fn(ctx);
    let p = layout::Params {
        palette: &pal,
        zoom: 1.0,
        reveal,
        matches: &[],
        image_size: &img,
        measure: &measure,
        font_family: &fam,
        fonts_gen: 0,
    };
    let g = c.galley_paged(ctx, 1, text, &p, geom.text_w(), Some(&geom));
    pages::page_count(&g, &geom)
}

fn in_ui(ctx: &egui::Context, f: impl FnOnce()) {
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
    let text = big_markdown(600_000, 7);
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
