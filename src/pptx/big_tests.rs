//! Stora presentationer: en syntetisk presentation med hundratals bilder (text, bilder, tabeller) och mått på
//! öppning, första bild, miniatyrer, sök, PDF-export, en redigering och minnestopp. Ignorerat prov:
//! `KLADD_PBIG=600 cargo test stora_presentationer -- --ignored --nocapture`.

use std::io::{Read, Write};
use std::time::Instant;

use super::tests::Builder;
use super::{edit_ops, view, Deck};

const PNG_CT: &str = r#"<Default Extension="png" ContentType="image/png"/>"#;

/// En PNG med ett mönster som beror på `seed` (så att bilderna inte delar cache).
fn png(seed: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(320, 200, |x, y| image::Rgba([(x as u8).wrapping_add(seed as u8), (y as u8).wrapping_mul(2), ((x + y) as u8) ^ (seed as u8), 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn table(rows: usize) -> String {
    let cell = |t: String| format!(r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE" sz="1400"/><a:t>{t}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#);
    let mut r = String::new();
    for i in 0..rows {
        r.push_str(&format!(r#"<a:tr h="370840">{}{}{}</a:tr>"#, cell(format!("Rad {i}")), cell(format!("{}", i * 17)), cell("åäö text".into())));
    }
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Tabell"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="3900000"/><a:ext cx="6400000" cy="1500000"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1"/><a:tblGrid><a:gridCol w="2000000"/><a:gridCol w="2000000"/><a:gridCol w="2400000"/></a:tblGrid>{r}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#
    )
}

/// `n` bilder: rubrik och punkter på alla, en bild på var annan (en av 40 olika) och en tabell på var femte.
pub fn big_deck(n: usize) -> Vec<u8> {
    let mut b = Builder::new();
    for i in 0..n {
        let title = format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="R"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="sv-SE"/><a:t>Bild {i}: rubrik med lite längre text åäö</a:t></a:r></a:p></p:txBody></p:sp>"#);
        let mut bullets = String::new();
        for k in 0..6 {
            bullets.push_str(&format!(r#"<a:p><a:r><a:rPr lang="sv-SE"/><a:t>Punkt {k} på bild {i} med en del text som kan behöva brytas över flera rader om den är lång</a:t></a:r></a:p>"#));
        }
        let body = format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="I"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr><a:normAutofit/></a:bodyPr><a:lstStyle/>{bullets}</p:txBody></p:sp>"#);
        let pic = if i % 2 == 0 {
            r#"<p:pic><p:nvPicPr><p:cNvPr id="11" name="Bild"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="6000000" y="1500000"/><a:ext cx="2800000" cy="1750000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#
        } else {
            ""
        };
        let tbl = if i % 5 == 0 { table(4) } else { String::new() };
        b = b.slide("", &format!("{title}{body}{pic}{tbl}"));
    }
    let base = b.build();
    // Lägg till bilderna och slide-relationerna.
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(base)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    for k in 0..zin.len() {
        let mut f = zin.by_index(k).unwrap();
        let name = f.name().to_owned();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        if name == "[Content_Types].xml" {
            buf = String::from_utf8(buf).unwrap().replace("<Override", &format!("{PNG_CT}<Override")).into_bytes();
        }
        if let Some(num) = name.strip_prefix("ppt/slides/_rels/slide").and_then(|r| r.strip_suffix(".xml.rels")) {
            let i: usize = num.parse::<usize>().unwrap() - 1;
            buf = String::from_utf8(buf).unwrap().replace("</Relationships>", &format!(r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/img{}.png"/></Relationships>"#, i % 40)).into_bytes();
        }
        out.start_file(&name, opt).unwrap();
        out.write_all(&buf).unwrap();
    }
    for k in 0..40 {
        out.start_file(format!("ppt/media/img{k}.png"), opt.compression_method(zip::CompressionMethod::Stored)).unwrap();
        out.write_all(&png(k)).unwrap();
    }
    out.finish().unwrap().into_inner()
}

/// Högsta resident minne (MB) ur `/proc/self/status`.
fn hwm_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok()))
        .map_or(0.0, |kb| kb / 1024.0)
}

#[test]
#[ignore]
fn stora_presentationer() {
    let n: usize = std::env::var("KLADD_PBIG").ok().and_then(|v| v.parse().ok()).unwrap_or(600);
    let bytes = big_deck(n);
    eprintln!("bilder: {n}, filstorlek {:.1} MB, minne efter bygge {:.0} MB", bytes.len() as f64 / 1e6, hwm_mb());
    let t = Instant::now();
    let deck = std::sync::Arc::new(Deck::open(&bytes).unwrap());
    eprintln!("öppna:          {:>8.0} ms   (HWM {:.0} MB)", t.elapsed().as_secs_f64() * 1e3, hwm_mb());
    assert_eq!(deck.slides.len(), n);
    let t = Instant::now();
    let _ = super::render_slide(&deck, 0, 1280, 720).unwrap();
    eprintln!("första bilden:  {:>8.0} ms", t.elapsed().as_secs_f64() * 1e3);
    // Var tiden går: visningslistan (text, form) mot rastreringen, per slag av bild.
    for (label, ids) in [("text", vec![1usize, 3, 7, 9, 11]), ("text+bild", vec![2, 4, 6, 8, 12]), ("text+tabell+bild", vec![0, 10, 20, 30, 40])] {
        let t = Instant::now();
        let scenes: Vec<_> = ids.iter().map(|&i| super::render::slide_scene(&deck, i)).collect();
        let a = t.elapsed().as_secs_f64() * 1e3 / ids.len() as f64;
        let t = Instant::now();
        for sc in &scenes {
            let _ = super::raster::render(sc, 192, 108);
        }
        eprintln!("  {label:<18} visningslista {a:>6.1} ms, rastrering 192 px {:>6.1} ms", t.elapsed().as_secs_f64() * 1e3 / ids.len() as f64);
    }
    // Miniatyrer: två trådar (som förut) mot `max_jobs` trådar (nu).
    let thumbs = |threads: usize| {
        let t = Instant::now();
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if i >= n {
                            break;
                        }
                        let _ = super::render_slide(&deck, i, 192, 108);
                    }
                });
            }
        });
        t.elapsed().as_secs_f64() * 1e3
    };
    let (a, b) = (thumbs(2), thumbs(super::view::max_jobs()));
    eprintln!("miniatyrer:     {a:>8.0} ms med 2 trådar, {b:>8.0} ms med {} (HWM {:.0} MB)", super::view::max_jobs(), hwm_mb());
    // Sökregistret: en tråd (förut) mot alla arbetstrådar (nu).
    let index_of = |i: usize| super::find::slide_text(&super::render::slide_scene(&deck, i)).text.len() + super::notes::notes_text(&deck, i).len();
    let ids: Vec<usize> = (0..n).collect();
    let t = Instant::now();
    let chars: usize = ids.iter().map(|&i| index_of(i)).sum();
    let a = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let chars2: usize = super::par::map(&ids, |&i| index_of(i)).iter().sum();
    assert_eq!(chars, chars2);
    eprintln!("sökregister:    {a:>8.0} ms med 1 tråd, {:>8.0} ms med {} ({chars} tecken)", t.elapsed().as_secs_f64() * 1e3, super::par::workers());
    // PDF: bara visningslistorna i följd, och hela exporten (parallell).
    let t = Instant::now();
    for i in 0..n {
        let _ = super::render::slide_scene(&deck, i);
    }
    let a = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let pdf = view::pdf_of(&deck).unwrap();
    eprintln!("PDF-export:     {:>8.0} ms (bara visningslistor i följd {a:.0} ms; {:.1} MB, HWM {:.0} MB)", t.elapsed().as_secs_f64() * 1e3, pdf.len() as f64 / 1e6, hwm_mb());
    // En redigering: ombyggnaden av hela presentationen.
    let mut tab = view::PptxTab::with_source(Deck::open(&bytes).unwrap(), bytes.clone());
    let t = Instant::now();
    assert!(tab.apply(None, |x| edit_ops::set_notes(x, 3, "ny anteckning")));
    eprintln!("en redigering:  {:>8.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let copy = tab.recovery_bytes().unwrap();
    eprintln!("säkerhetskopia: {:>8.0} ms   ({:.1} MB)", t.elapsed().as_secs_f64() * 1e3, copy.len() as f64 / 1e6);
    eprintln!("minnestopp:     {:>8.0} MB", hwm_mb());
}
