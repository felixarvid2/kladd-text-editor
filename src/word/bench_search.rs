//! Mätning av tangenttryck med sökningen öppen i ett stort Word-dokument (ignorerad; kör med
//! `KLADD_BENCH_DOCX=sökväg [KLADD_BENCH_FIND=ord] [KLADD_BENCH_KEYS=antal] cargo test -- --ignored bench_docx_search --nocapture`).
//! Varje bildruta: ett tecken in eller ut, sökningen om (som `KladdApp::search_results`) och galleriet.

use std::time::Instant;

use egui::{Color32, FontFamily, FontId};

use super::{layout, pages};
use crate::{search, theme};

fn fam(_: Option<&str>, bold: bool, italic: bool) -> FontFamily {
    theme::doc_font(bold, italic)
}

#[test]
#[ignore]
fn bench_docx_search() {
    let Ok(path) = std::env::var("KLADD_BENCH_DOCX") else { return };
    let n: usize = std::env::var("KLADD_BENCH_KEYS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let term = std::env::var("KLADD_BENCH_FIND").unwrap_or_else(|_| "och".into());
    let bytes = std::fs::read(&path).unwrap();
    let text = crate::docx::read(&bytes).unwrap().markdown;
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let out = ctx.run_ui(Default::default(), |_| {});
    std::mem::forget(out);
    let mut c = layout::Cache::default();
    let pal = theme::palette(false);
    let spec = search::Spec { query: term.clone(), ..Default::default() };
    let mut at = text.len() / 2;
    while !text.is_char_boundary(at) || at == 0 || text.as_bytes()[at - 1] != b' ' || !text[at..].starts_with(|c: char| c.is_alphabetic()) {
        at += 1;
    }
    let mut e = text.clone();
    let mut run = |label: &str, with_search: bool, n: usize| {
        let mut times = Vec::new();
        let mut search_ms: Vec<f64> = Vec::new();
        for k in 0..n {
            if k % 2 == 0 {
                e.insert(at, 'x');
            } else {
                e.remove(at);
            }
            let out = ctx.run_ui(Default::default(), |_| {
                let t = Instant::now();
                let mut shapes = 0;
                let a = c.analysis(1, &e);
                let matches = if with_search {
                    let r = search::search_doc(&e, &a, &spec);
                    search::highlights(&r, None)
                } else {
                    Vec::new()
                };
                search_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                let geom = pages::Geom::for_sections(1.0, &a.defaults, &a.sections);
                let _ = c.tables(1, &e);
                let _ = c.fonts(1, &e);
                let img = |_: &str| None;
                let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
                let p = layout::Params {
                    palette: &pal,
                    zoom: 1.0,
                    reveal: 0..0,
                    image_size: &img,
                    measure: &measure,
                    font_family: &fam,
                    fonts_gen: 0,
                };
                let g = c.galley_paged_lazy(&ctx, 1, &e, &p, geom.edit_w(), Some(&geom), None);
                // Som vyn: träffarna som lager för fönstret kring markören (bildrutan ritas inte här).
                if !matches.is_empty() {
                    let y = g.pos_from_cursor(egui::text::CCursor::new(e[..at].chars().count())).min.y;
                    let layer = super::matchlayer::build(&g, &e, &matches, egui::Vec2::ZERO, y - 700.0, y + 700.0, &pal);
                    shapes = layer.behind.len() + layer.over.len();
                }
                times.push((t.elapsed().as_secs_f64() * 1000.0, matches.len(), shapes));
            });
            std::mem::forget(out);
        }
        let mut v: Vec<f64> = times.iter().skip(2).map(|t| t.0).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        search_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!("  varav analys+sökning: median {:.2} ms", search_ms[search_ms.len() / 2]);
        eprintln!("{label}: median {:.2} ms, min {:.2}, max {:.2} ({} träffar, {} rektanglar i fönstret)", v[v.len() / 2], v[0], v[v.len() - 1], times[0].1, times[0].2);
    };
    // "Nästa träff": texten är oförändrad, bara den aktuella träffen byts (förut en full omläggning).
    let next = |c: &mut layout::Cache| {
        let a = c.analysis(1, &text);
        let r = search::search_doc(&text, &a, &spec);
        let geom = pages::Geom::for_sections(1.0, &a.defaults, &a.sections);
        let _ = c.tables(1, &text);
        let _ = c.fonts(1, &text);
        let mut v = Vec::new();
        for k in 0..n.min(r.matches.len()) {
            let out = ctx.run_ui(Default::default(), |_| {
                let t = Instant::now();
                let matches = search::highlights(&r, Some(r.matches[k * 7 % r.matches.len()].start));
                let img = |_: &str| None;
                let measure = |s: &str, f: FontId| ctx.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, Color32::WHITE).size().x);
                let p = layout::Params {
                    palette: &pal,
                    zoom: 1.0,
                    reveal: 0..0,
                    image_size: &img,
                    measure: &measure,
                    font_family: &fam,
                    fonts_gen: 0,
                };
                let g = c.galley_paged_lazy(&ctx, 1, &text, &p, geom.edit_w(), Some(&geom), None);
                let y = g.pos_from_cursor(egui::text::CCursor::new(text[..matches.iter().find(|m| m.2).map_or(0, |m| m.0)].chars().count())).min.y;
                let _ = super::matchlayer::build(&g, &text, &matches, egui::Vec2::ZERO, y - 700.0, y + 700.0, &pal);
                v.push(t.elapsed().as_secs_f64() * 1000.0);
            });
            std::mem::forget(out);
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!("nästa träff \"{term}\": median {:.2} ms, min {:.2}, max {:.2}", v[v.len() / 2], v[0], v[v.len() - 1]);
    };
    run("uppvärmning", false, 6);
    run("utan sökning", false, n);
    run(&format!("sökning \"{term}\""), true, n);
    next(&mut c);
}

/// Textläget (färgläggningen i `highlight.rs`): en tangent och "nästa träff" med och utan träffar i galleriet.
/// `KLADD_BENCH_MD_BYTES` (standard 300 000) styr storleken.
#[test]
#[ignore]
fn bench_text_search() {
    let bytes = std::env::var("KLADD_BENCH_MD_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(300_000);
    let text = super::bench::big_markdown(bytes, 9);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    let out = ctx.run_ui(Default::default(), |_| {});
    std::mem::forget(out);
    let pal = theme::palette(false);
    let mut hl = crate::highlight::Highlighter::new();
    let all: Vec<(usize, usize, bool)> = crate::highlight::find_all(&text, "och", false).into_iter().map(|(a, b)| (a, b, false)).collect();
    let mid = text[..text.len() / 2].rfind(' ').unwrap();
    let mut e = text.clone();
    let mut times: Vec<(&str, Vec<f64>)> = vec![("tangent utan träffar", vec![]), ("tangent med träffar", vec![]), ("nästa träff", vec![])];
    let n = 12;
    let mut run = |t: &str, m: &[(usize, usize, bool)]| {
        let mut ms = 0.0;
        let out = ctx.run_ui(Default::default(), |_| {
            let p = crate::highlight::Params { doc_id: 1, syntax_hint: "md", palette: &pal, font_size: 14.0, matches: m };
            let s = Instant::now();
            let g = hl.galley(&ctx, t, &p, 800.0);
            std::hint::black_box(g.rows.len());
            ms = s.elapsed().as_secs_f64() * 1000.0;
        });
        std::mem::forget(out);
        ms
    };
    run(&e, &[]);
    for k in 0..n {
        if k % 2 == 0 {
            e.insert(mid, 'x');
        } else {
            e.remove(mid);
        }
        times[0].1.push(run(&e, &[]));
    }
    for k in 0..n {
        if k % 2 == 0 {
            e.insert(mid, 'x');
        } else {
            e.remove(mid);
        }
        // Träffarna räknas om för den ändrade texten (som appen gör), utanför mätningen.
        let m: Vec<(usize, usize, bool)> = crate::highlight::find_all(&e, "och", false).into_iter().map(|(a, b)| (a, b, false)).collect();
        times[1].1.push(run(&e, &m));
    }
    for k in 0..n {
        let mut m = all.clone();
        m[k * 11 % all.len()].2 = true;
        times[2].1.push(run(&e, &m));
    }
    for (name, mut v) in times {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!("textläge {} kB, {} träffar: {name}: median {:.1} ms (min {:.1}, max {:.1})", text.len() / 1000, all.len(), v[v.len() / 2], v[0], v[v.len() - 1]);
    }
}
