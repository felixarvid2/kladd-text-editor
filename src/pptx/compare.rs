//! Jämförelse mot riktiga PowerPoint-renderingar.
//!
//! `powerpoint_reference` (ignorerat prov) renderar varje bild i presentationerna under
//! `KLADD_PPTX_REFS` (en mapp per presentation med `deck.pptx`, `ref-NN.png` från PowerPoints
//! PDF-export i 72 dpi och gärna `lo-NN.png` från LibreOffice) och mäter likheten med samma mått
//! för Kladd och LibreOffice. `pageToSlide` läses ur `../decks.json` om den finns (dolda bilder
//! saknas i PDF:en). Resultatet (tabell och sida-vid-sida-bilder PowerPoint | Kladd | skillnad)
//! skrivs till `KLADD_PPTX_OUT` (standard: en mapp under systemets tempkatalog).
//!
//! Måttet: gråskala, lätt gaussisk oskärpa (σ = 1 px) och SSIM i 8×8-fönster med steg 4, medel över
//! bilden. 1,0 är identiskt. Oskärpan gör måttet tåligt mot små skillnader i kantutjämning.
//!
//! Körs med
//! `KLADD_PPTX_REFS=…/pptx/downloads cargo test pptx::compare -- --ignored --nocapture`;
//! `KLADD_PPTX_DECKS=a,b` och `KLADD_PPTX_SLIDES=3,5` begränsar urvalet.

#![cfg(test)]

use std::path::{Path, PathBuf};

/// En gråskalebild.
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Gray {
    pub fn from_rgba(w: u32, h: u32, rgba: &[u8]) -> Gray {
        let px = rgba
            .chunks_exact(4)
            .map(|p| {
                // Mot vit bakgrund.
                let a = p[3] as f32 / 255.0;
                let y = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0;
                y * a + (1.0 - a)
            })
            .collect();
        Gray { w: w as usize, h: h as usize, px }
    }

    pub fn load(path: &Path) -> Option<Gray> {
        let img = image::open(path).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        Some(Gray::from_rgba(w, h, img.as_raw()))
    }

    /// I storleken `w`×`h`. Skiljer det högst 2 bildpunkter (avrundning av sidstorleken, t.ex.
    /// LibreOffices 961×540 mot 960×540) beskärs eller fylls bilden ut med vitt i stället för att skalas, så
    /// att ingen kolumn försvinner mitt i bilden; annars närmaste granne.
    pub fn resized(&self, w: usize, h: usize) -> Gray {
        if self.w == w && self.h == h {
            return Gray { w, h, px: self.px.clone() };
        }
        if self.w.abs_diff(w) <= 2 && self.h.abs_diff(h) <= 2 {
            let mut px = vec![1.0; w * h];
            for y in 0..h.min(self.h) {
                for x in 0..w.min(self.w) {
                    px[y * w + x] = self.px[y * self.w + x];
                }
            }
            return Gray { w, h, px };
        }
        let mut px = vec![1.0; w * h];
        for y in 0..h {
            let sy = (y * self.h / h.max(1)).min(self.h.saturating_sub(1));
            for x in 0..w {
                let sx = (x * self.w / w.max(1)).min(self.w.saturating_sub(1));
                px[y * w + x] = self.px[sy * self.w + sx];
            }
        }
        Gray { w, h, px }
    }

    fn blurred(&self) -> Gray {
        let k = [0.0044, 0.054, 0.242, 0.399, 0.242, 0.054, 0.0044];
        let sum: f32 = k.iter().sum();
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    let xx = (x as i64 + i as i64 - 3).clamp(0, w as i64 - 1) as usize;
                    acc += kv * self.px[y * w + xx];
                }
                tmp[y * w + x] = acc / sum;
            }
        }
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    let yy = (y as i64 + i as i64 - 3).clamp(0, h as i64 - 1) as usize;
                    acc += kv * tmp[yy * w + x];
                }
                out[y * w + x] = acc / sum;
            }
        }
        Gray { w, h, px: out }
    }
}

/// Likhetsmåttet (medel-SSIM efter oskärpa). Bilderna måste vara lika stora.
pub fn similarity(a: &Gray, b: &Gray) -> f64 {
    let b = b.resized(a.w, a.h);
    let a = a.blurred();
    let b = b.blurred();
    let (w, h) = (a.w, a.h);
    let (c1, c2) = (0.01f64 * 0.01, 0.03f64 * 0.03);
    let win = 8usize;
    let step = 4usize;
    let mut total = 0.0f64;
    let mut n = 0usize;
    let mut y = 0;
    while y + win <= h {
        let mut x = 0;
        while x + win <= w {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for yy in y..y + win {
                for xx in x..x + win {
                    let pa = a.px[yy * w + xx] as f64;
                    let pb = b.px[yy * w + xx] as f64;
                    sa += pa;
                    sb += pb;
                    saa += pa * pa;
                    sbb += pb * pb;
                    sab += pa * pb;
                }
            }
            let k = (win * win) as f64;
            let (ma, mb) = (sa / k, sb / k);
            let va = (saa / k - ma * ma).max(0.0);
            let vb = (sbb / k - mb * mb).max(0.0);
            let cov = sab / k - ma * mb;
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            n += 1;
            x += step;
        }
        y += step;
    }
    if n == 0 { 1.0 } else { total / n as f64 }
}

/// Sida-vid-sida: PowerPoint | Kladd | skillnad (rött där de skiljer sig).
pub fn side_by_side(reference: &image::RgbaImage, ours: &image::RgbaImage, out: &Path) {
    let (w, h) = reference.dimensions();
    let ours = if ours.dimensions() != (w, h) { image::imageops::resize(ours, w, h, image::imageops::FilterType::Triangle) } else { ours.clone() };
    let mut img = image::RgbaImage::from_pixel(w * 3 + 8, h, image::Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut img, reference, 0, 0);
    image::imageops::overlay(&mut img, &ours, (w + 4) as i64, 0);
    for y in 0..h {
        for x in 0..w {
            let a = reference.get_pixel(x, y);
            let b = ours.get_pixel(x, y);
            let ga = (a[0] as i32 + a[1] as i32 + a[2] as i32) / 3;
            let gb = (b[0] as i32 + b[1] as i32 + b[2] as i32) / 3;
            let d = (ga - gb).unsigned_abs().min(255) as u8;
            let base = (255 - (255 - ga.clamp(0, 255) as u8) / 4) as u8;
            let px = if d > 40 { image::Rgba([255, 255 - d, 255 - d, 255]) } else { image::Rgba([base, base, base, 255]) };
            img.put_pixel(2 * w + 8 + x, y, px);
        }
    }
    let _ = img.save(out);
}

/// `pageToSlide` (1-baserade bildnummer över alla bilder, `null` = ingen säker matchning) per presentation.
fn page_maps(refs: &Path) -> std::collections::HashMap<String, Vec<Option<usize>>> {
    let mut out = std::collections::HashMap::new();
    let Some(text) = refs.parent().map(|p| p.join("decks.json")).and_then(|p| std::fs::read_to_string(p).ok()) else { return out };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return out };
    for d in v.get("decks").and_then(|d| d.as_array()).into_iter().flatten() {
        let (Some(name), Some(map)) = (d.get("name").and_then(|n| n.as_str()), d.get("pageToSlide").and_then(|m| m.as_array())) else { continue };
        out.insert(name.to_owned(), map.iter().map(|x| x.as_u64().map(|v| v as usize)).collect());
    }
    out
}

fn numbered(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_prefix(prefix)?.strip_suffix(".png")?.parse::<usize>().ok()?;
            Some((n, e.path()))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
}

/// Resultat för en bild.
#[derive(Clone, Debug)]
pub struct SlideScore {
    pub page: usize,
    pub slide: usize,
    pub kladd: f64,
    pub lo: Option<f64>,
    /// Bilden har diagram eller EMF/WMF.
    pub chart_or_emf: bool,
}

/// Mäter en presentation. Skriver sida-vid-sida-bilder till `out` om den anges.
pub fn score_deck(dir: &Path, map: Option<&Vec<Option<usize>>>, out: Option<&Path>, slide_filter: &[usize]) -> Result<Vec<SlideScore>, String> {
    let bytes = std::fs::read(dir.join("deck.pptx")).map_err(|e| e.to_string())?;
    let deck = super::Deck::open(&bytes)?;
    let refs = numbered(dir, "ref-");
    let los = numbered(dir, "lo-");
    let visible: Vec<usize> = (0..deck.slides.len()).filter(|&i| !deck.slides[i].hidden).collect();
    let mut jobs = Vec::new();
    for (pi, rp) in refs.iter().enumerate() {
        // Sida → bild (0-baserat index).
        let slide = match map {
            Some(m) => match m.get(pi).copied().flatten() {
                Some(s) => s - 1,
                None => continue,
            },
            None => match visible.get(pi) {
                Some(&s) => s,
                None => continue,
            },
        };
        if slide >= deck.slides.len() || (!slide_filter.is_empty() && !slide_filter.contains(&(slide + 1))) {
            continue;
        }
        jobs.push((pi, rp.clone(), slide));
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let scores = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    std::thread::scope(|sc| {
        for _ in 0..threads {
            std::thread::Builder::new().stack_size(16 << 20).spawn_scoped(sc, || loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((pi, rp, slide)) = jobs.get(k).cloned() else { break };
                let Ok(ref_img) = image::open(&rp).map(|i| i.to_rgba8()) else { continue };
                let (w, h) = ref_img.dimensions();
                let scene = super::render::slide_scene(&deck, slide);
                let Some(pm) = super::raster::render(&scene, w, h) else { continue };
                let (_, _, rgba) = super::raster::to_rgba(pm);
                let Some(ours) = image::RgbaImage::from_raw(w, h, rgba) else { continue };
                let g_ref = Gray::from_rgba(w, h, ref_img.as_raw());
                let kladd = similarity(&g_ref, &Gray::from_rgba(w, h, ours.as_raw()));
                // LibreOffice: sida k = k:te synliga bilden.
                let lo = visible.iter().position(|&s| s == slide).and_then(|k| los.get(k)).and_then(|p| Gray::load(p)).map(|g| similarity(&g_ref, &g));
                let f = deck.features(slide);
                if let Some(out) = out {
                    let _ = std::fs::create_dir_all(out);
                    side_by_side(&ref_img, &ours, &out.join(format!("{:02}.png", pi + 1)));
                }
                scores.lock().unwrap().push(SlideScore { page: pi + 1, slide: slide + 1, kladd, lo, chart_or_emf: f.chart || f.emf });
            })
            .expect("tråd");
        }
    });
    let mut scores = scores.into_inner().unwrap();
    scores.sort_by_key(|s| s.page);
    Ok(scores)
}

fn mean(v: impl Iterator<Item = f64>) -> Option<f64> {
    let (s, n) = v.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    (n > 0).then(|| s / n as f64)
}

fn fmt(v: Option<f64>) -> String {
    v.map_or("–".into(), |v| format!("{v:.3}"))
}

#[test]
#[ignore]
fn powerpoint_reference() {
    let Some(refs) = std::env::var_os("KLADD_PPTX_REFS").map(PathBuf::from) else {
        eprintln!("KLADD_PPTX_REFS saknas: hoppar över");
        return;
    };
    let out = std::env::var_os("KLADD_PPTX_OUT").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("kladd-pptx-compare"));
    let decks_filter: Vec<String> = std::env::var("KLADD_PPTX_DECKS").ok().map(|s| s.split(',').map(|x| x.trim().to_owned()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    let slide_filter: Vec<usize> = std::env::var("KLADD_PPTX_SLIDES").ok().map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default();
    let sides = std::env::var_os("KLADD_PPTX_NO_SIDES").is_none();
    let maps = page_maps(&refs);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&refs)
        .expect("KLADD_PPTX_REFS")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("deck.pptx").exists() && p.join("ref-01.png").exists())
        .filter(|p| decks_filter.is_empty() || decks_filter.iter().any(|d| p.file_name().is_some_and(|n| n.to_string_lossy() == *d)))
        .collect();
    dirs.sort();
    let threads = 1;
    let results: std::sync::Mutex<Vec<(String, Result<Vec<SlideScore>, String>)>> = std::sync::Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(dir) = dirs.get(i) else { break };
                let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let side_dir = out.join(&name);
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| score_deck(dir, maps.get(&name), sides.then_some(side_dir.as_path()), &slide_filter)))
                    .unwrap_or_else(|_| Err("panik".into()));
                results.lock().unwrap().push((name, r));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    let mut table = String::from(
        "| presentation | bilder | Kladd medel | Kladd sämsta (bild) | LO medel | LO sämsta (bild) | Kladd utan diagram/EMF | LO utan diagram/EMF | Kladd bättre |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    let mut csv = String::from("deck,page,slide,kladd,lo,chart_or_emf\n");
    for (name, r) in &results {
        match r {
            Ok(sc) if !sc.is_empty() => {
                let km = mean(sc.iter().map(|s| s.kladd));
                let lm = mean(sc.iter().filter_map(|s| s.lo));
                let kw = sc.iter().min_by(|a, b| a.kladd.total_cmp(&b.kladd)).map(|s| format!("{:.3} ({})", s.kladd, s.slide)).unwrap_or_default();
                let lw = sc.iter().filter(|s| s.lo.is_some()).min_by(|a, b| a.lo.unwrap_or(1.0).total_cmp(&b.lo.unwrap_or(1.0))).map(|s| format!("{:.3} ({})", s.lo.unwrap_or(0.0), s.slide)).unwrap_or("–".into());
                let kp = mean(sc.iter().filter(|s| !s.chart_or_emf).map(|s| s.kladd));
                let lp = mean(sc.iter().filter(|s| !s.chart_or_emf).filter_map(|s| s.lo));
                let wins = sc.iter().filter(|s| s.lo.is_some_and(|l| s.kladd > l)).count();
                let with_lo = sc.iter().filter(|s| s.lo.is_some()).count();
                table.push_str(&format!(
                    "| {name} | {} | {} | {kw} | {} | {lw} | {} | {} | {} |\n",
                    sc.len(),
                    fmt(km),
                    fmt(lm),
                    fmt(kp),
                    fmt(lp),
                    if with_lo > 0 { format!("{wins}/{with_lo}") } else { "–".into() }
                ));
                for s in sc {
                    csv.push_str(&format!("{name},{},{},{:.4},{},{}\n", s.page, s.slide, s.kladd, s.lo.map_or(String::new(), |v| format!("{v:.4}")), s.chart_or_emf));
                }
            }
            Ok(_) => table.push_str(&format!("| {name} | 0 | – | – | – | – | – | – | – |\n")),
            Err(e) => table.push_str(&format!("| {name} | fel: {e} | | | | | | | |\n")),
        }
    }
    let _ = std::fs::create_dir_all(&out);
    let _ = std::fs::write(out.join("table.md"), &table);
    let _ = std::fs::write(out.join("scores.csv"), &csv);
    println!("{table}\nUtdata: {}", out.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matt_ar_ett_for_lika_och_lagre_for_olika() {
        let w = 64;
        let h = 48;
        let a = Gray { w, h, px: (0..w * h).map(|i| if (i / w) % 10 < 5 { 0.0 } else { 1.0 }).collect() };
        assert!((similarity(&a, &a) - 1.0).abs() < 1e-9);
        let b = Gray { w, h, px: vec![1.0; w * h] };
        assert!(similarity(&a, &b) < 0.5);
        // En pixels förskjutning straffas mindre än en helt annan bild.
        let c = Gray { w, h, px: (0..w * h).map(|i| if ((i / w) + 1) % 10 < 5 { 0.0 } else { 1.0 }).collect() };
        assert!(similarity(&a, &c) > similarity(&a, &b));
    }
}
