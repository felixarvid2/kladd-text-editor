//! Bevarande och slumpprov för Word (.docx) och OpenDocument (.odt).
//!
//! 1. `word_bevarande` (ignorerat, `KLADD_WORD_PRESERVE=1`): öppnar varje korpusfil, sparar orörd och efter en
//!    liten ändring, och jämför originalets delar och XML-element/attribut med det sparade: vad som tappas tyst
//!    (utan att `dropped` varnar) är fel. Skriver en sammanställning.
//! 2. `word_slumpprov` (ignorerat, `KLADD_WORD_FUZZ=<antal>`): en egen seedad mutator (zip, XML, relationer) över
//!    korpusen; varje steg (öppna, redigera i Markdown, spara, öppna igen, inget hänger) under `catch_unwind`
//!    och med tidsgräns.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::docx::Image;

// ------------------------------------------------------------------ gemensamt

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Docx,
    Odt,
}

fn corpus_dir(kind: Kind) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("corpus").join(if kind == Kind::Docx { "docx" } else { "odt" })
}

fn corpus(kind: Kind) -> Vec<PathBuf> {
    let ext = if kind == Kind::Docx { "docx" } else { "odt" };
    let mut v: Vec<PathBuf> = std::fs::read_dir(corpus_dir(kind)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == ext)).collect();
    v.sort();
    v
}

fn read_zip(bytes: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let Ok(mut f) = z.by_index(i) else { continue };
        let mut b = Vec::new();
        let _ = f.read_to_end(&mut b);
        out.push((f.name().to_owned(), b));
    }
    Some(out)
}

fn write_zip(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default();
    let mut seen = BTreeSet::new();
    for (n, b) in parts {
        if !seen.insert(n.clone()) || z.start_file(n, opt).is_err() {
            continue;
        }
        let _ = z.write_all(b);
    }
    z.finish().map(|c| c.into_inner()).unwrap_or_default()
}

/// Öppnat dokument: Markdown, bilder, tappat och originalet (Word).
struct Opened {
    markdown: String,
    images: Vec<(String, Image)>,
    dropped: Vec<&'static str>,
    source: Option<crate::docx::Source>,
}

fn open(kind: Kind, bytes: &[u8]) -> Result<Opened, String> {
    match kind {
        Kind::Docx => crate::docx::read(bytes).map(|l| Opened { markdown: l.markdown, images: l.images, dropped: l.dropped, source: l.source }),
        Kind::Odt => crate::odt::read(bytes).map(|l| Opened { markdown: l.markdown, images: l.images, dropped: l.dropped, source: None }),
    }
}

fn save(kind: Kind, md: &str, o: &Opened) -> Result<Vec<u8>, String> {
    let resolve = |src: &str| crate::doc::resolve_image(src, Path::new("."), &o.images);
    match kind {
        Kind::Docx => crate::docx::write(md, "Prov", &resolve, o.source.as_ref()).map(|w| w.bytes),
        Kind::Odt => crate::odt::write(md, "Prov", &resolve).map(|w| w.bytes),
    }
}

// ------------------------------------------------------------------ inventering av XML

/// Räknar element (`namn`) och attribut (`namn@attr`) med sina fullständiga namn.
fn inventory(xml: &[u8]) -> BTreeMap<String, usize> {
    inventory_in(xml, None)
}

/// Som `inventory`, men räknar bara inne i det första elementet med namnet `root` (ODT: `office:body`).
fn inventory_in(xml: &[u8], root: Option<&str>) -> BTreeMap<String, usize> {
    use quick_xml::events::Event;
    let mut m = BTreeMap::new();
    let s = String::from_utf8_lossy(xml);
    let mut r = quick_xml::Reader::from_str(&s);
    // Djup inne i `root` (0 = utanför, eller utan `root` alltid med).
    let mut depth = 0usize;
    let mut done = false;
    let add = |e: &quick_xml::events::BytesStart<'_>, m: &mut BTreeMap<String, usize>| {
        let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
        *m.entry(name.clone()).or_insert(0) += 1;
        for a in e.attributes().flatten() {
            let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            if !k.starts_with("xmlns") {
                *m.entry(format!("{name}@{k}")).or_insert(0) += 1;
            }
        }
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match root {
                    None => add(&e, &mut m),
                    Some(rt) => {
                        if depth > 0 || (!done && name == rt) {
                            depth += 1;
                            add(&e, &mut m);
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                if root.is_none() || depth > 0 {
                    add(&e, &mut m);
                }
            }
            Ok(Event::End(_)) => {
                if root.is_some() && depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        done = true;
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    m
}

/// Inventering av en del: ODT:s `content.xml` räknas bara inne i `office:body` (stilar och typsnittslistor skrivs
/// alltid om och är inte innehåll).
fn inv_for(part: &str, xml: &[u8]) -> BTreeMap<String, usize> {
    if part == "content.xml" { inventory_in(xml, Some("office:body")) } else { inventory(xml) }
}

/// Delnamn utan löpnummer (`word/media/image12.png` → `word/media/image#.png`).
fn kind_of_part(n: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in n.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

fn is_xml(n: &str) -> bool {
    n.ends_with(".xml") || n.ends_with(".rels") || n.ends_with(".vml")
}

/// Smalnar av ett paket för jämförelse: namn → innehåll.
fn parts_map(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    read_zip(bytes).unwrap_or_default().into_iter().collect()
}

#[derive(Default)]
struct Tally {
    /// Del (utan löpnummer) → (filer där den fanns, identisk, omskriven men samma inventering, annan inventering, saknas)
    parts: BTreeMap<String, [usize; 5]>,
    /// Element/attribut → (totalt tappat antal, antal filer, antal filer där `dropped` varnade ändå)
    lost: BTreeMap<String, (usize, usize, usize)>,
    /// Filer med tyst förlust (någon del saknas eller något element tappas utan varning).
    silent_files: BTreeSet<String>,
    files: usize,
    /// ODT: delar som bär innehåll (bilder efter innehåll, inbäddade objekt, makron) och inte finns i det sparade:
    /// klass → (filer, filer där `dropped` varnade ändå)
    content_lost: BTreeMap<String, (usize, usize)>,
}

/// Tappade element/attribut (utan brus) i XML-delarna: nyckel → antal.
fn losses(orig: &[u8], out: &[u8]) -> BTreeMap<String, usize> {
    let (a, b) = (parts_map(orig), parts_map(out));
    let mut m = BTreeMap::new();
    for (n, x) in &a {
        if !is_xml(n) {
            continue;
        }
        let Some(y) = b.get(n) else { continue };
        if x == y {
            continue;
        }
        let (ia, ib) = (inv_for(n, x), inv_for(n, y));
        for (k, ca) in &ia {
            let cb = ib.get(k).copied().unwrap_or(0);
            if cb < *ca && !is_noise(k) {
                *m.entry(format!("{}: {k}", kind_of_part(n))).or_insert(0) += ca - cb;
            }
        }
    }
    m
}

/// ODT: vilka innehållsbärande delar (bilder jämförs efter innehåll) som saknas i det sparade.
fn odt_content_lost(orig: &[u8], out: &[u8], warned: bool, t: &mut Tally) {
    let (a, b) = (parts_map(orig), parts_map(out));
    let have: BTreeSet<&Vec<u8>> = b.values().collect();
    let mut classes: BTreeSet<&str> = BTreeSet::new();
    for (n, x) in &a {
        if n.ends_with('/') {
            continue;
        }
        let class = if n.starts_with("Pictures/") && !have.contains(x) {
            "bild (innehållet finns inte i det sparade)"
        } else if (n.starts_with("Object ") || n.starts_with("Obj")) && !b.contains_key(n) {
            "inbäddat objekt (formel, diagram, OLE)"
        } else if n.starts_with("ObjectReplacements/") && !b.contains_key(n) {
            "ersättningsbild för inbäddat objekt"
        } else if (n.starts_with("Basic/") || n.starts_with("Scripts/")) && !b.contains_key(n) {
            "makro/skript"
        } else if n == "settings.xml" && !b.contains_key(n) {
            "settings.xml (visnings- och skrivarinställningar)"
        } else if n == "manifest.rdf" && !b.contains_key(n) {
            "manifest.rdf (metadata)"
        } else {
            continue;
        };
        classes.insert(class);
    }
    for c in classes {
        let e = t.content_lost.entry(c.to_owned()).or_default();
        e.0 += 1;
        if warned {
            e.1 += 1;
        }
    }
}

/// Jämför originalet med det sparade; fyller `t`. `warned` = `dropped` var inte tom.
fn compare(name: &str, orig: &[u8], out: &[u8], warned: bool, t: &mut Tally) {
    let (a, b) = (parts_map(orig), parts_map(out));
    for (n, x) in &a {
        if n.ends_with('/') {
            continue; // katalogposter i zip-filen
        }
        let k = kind_of_part(n);
        let e = t.parts.entry(k).or_default();
        e[0] += 1;
        match b.get(n) {
            None => {
                e[4] += 1;
                if !warned {
                    t.silent_files.insert(format!("{name} (del saknas: {n})"));
                }
            }
            Some(y) if x == y => e[1] += 1,
            Some(y) => {
                if is_xml(n) {
                    let (ia, ib) = (inv_for(n, x), inv_for(n, y));
                    if ia == ib {
                        e[2] += 1;
                    } else {
                        e[3] += 1;
                        for (k, ca) in &ia {
                            let cb = ib.get(k).copied().unwrap_or(0);
                            if cb < *ca {
                                if DETAIL.load(std::sync::atomic::Ordering::Relaxed) && !is_noise(k) {
                                    println!("  [{name}] {} förlorade {} {k}", kind_of_part(n), ca - cb);
                                }
                                let l = t.lost.entry(format!("{}: {k}", kind_of_part(n))).or_default();
                                l.0 += ca - cb;
                                l.1 += 1;
                                if warned {
                                    l.2 += 1;
                                } else {
                                    t.silent_files.insert(name.to_owned());
                                }
                            }
                        }
                    }
                } else {
                    e[3] += 1;
                    if !warned {
                        t.silent_files.insert(format!("{name} (binär del ändrad: {n})"));
                    }
                }
            }
        }
    }
}

static DETAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Förlust som inte spelar någon roll för innehållet (revisions-id, stavningsmarkeringar, id:n).
fn is_noise(k: &str) -> bool {
    k.contains("@w:rsid") || k.contains("w:proofErr") || k.contains("@w14:paraId") || k.contains("@w14:textId") || k.contains("w:lastRenderedPageBreak")
}

/// Lägger ett nytt stycke efter det första vanliga textstycket (ett block som börjar med en bokstav; inte en
/// lista, tabell, kodblock eller markör). Finns inget sådant läggs det sist.
fn with_added(md: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for block in md.split_inclusive("\n\n") {
        out.push_str(block);
        let t = block.trim_start();
        if !done && t.chars().next().is_some_and(|c| c.is_alphabetic()) && !block.contains("\n    ") && !t.starts_with("[^") {
            if !block.ends_with("\n\n") {
                out.push_str("\n\n");
            }
            out.push_str("Tillägg ÅÄÖ.\n\n");
            done = true;
        }
    }
    if !done {
        out.push_str("\n\nTillägg ÅÄÖ.\n");
    }
    out
}

/// Ändrar ett ord i första vanliga textraden (en riktig redigering mitt i dokumentet).
fn with_word_edit(md: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for l in md.split_inclusive('\n') {
        let t = l.trim_start();
        if !done && t.chars().next().is_some_and(|c| c.is_alphabetic()) && !t.starts_with('<') && l.trim_end().len() > 8 {
            let body = l.trim_end_matches('\n');
            out.push_str(body);
            out.push_str(" Kladd");
            out.push('\n');
            done = true;
        } else {
            out.push_str(l);
        }
    }
    out
}

fn print_tally(kind: Kind, label: &str, t: &Tally) {
    println!("\n===== {kind:?} {label}: {} filer =====", t.files);
    println!("delar (identisk / samma element / andra element / saknas av antal):");
    for (k, v) in &t.parts {
        if v[1] != v[0] {
            println!("  {k}: {} filer, {} identiska, {} omskrivna lika, {} ändrade, {} saknas", v[0], v[1], v[2], v[3], v[4]);
        }
    }
    println!("tappade element/attribut (totalt antal, filer, filer med varning):");
    let mut l: Vec<_> = t.lost.iter().collect();
    l.sort_by_key(|(_, v)| std::cmp::Reverse(v.1));
    for (k, v) in l.iter().take(60) {
        println!("  {k}: {} st i {} filer ({} med varning)", v.0, v.1, v.2);
    }
    if !t.content_lost.is_empty() {
        println!("innehållsbärande delar som saknas efter sparning (filer, varav varnade):");
        for (k, v) in &t.content_lost {
            println!("  {k}: {} filer, {} med varning", v.0, v.1);
        }
    }
    println!("filer med tyst förlust: {}", t.silent_files.len());
}

/// Sammanställning av vad som bevaras. `KLADD_WORD_PRESERVE=1 cargo test word_bevarande -- --ignored --nocapture`.
#[test]
#[ignore]
fn word_bevarande() {
    if std::env::var_os("KLADD_WORD_PRESERVE").is_none() {
        return;
    }
    for kind in [Kind::Docx, Kind::Odt] {
        let (mut same_t, mut edit_t) = (Tally::default(), Tally::default());
        // Följdskador av en redigering: förlust efter ändring minus förlust vid oförändrad sparning.
        let mut collateral: BTreeMap<String, (usize, usize, Vec<String>)> = BTreeMap::new();
        for f in corpus(kind) {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let Ok(bytes) = std::fs::read(&f) else { continue };
            let Ok(Ok(o)) = std::panic::catch_unwind(|| open(kind, &bytes)) else { continue };
            let warned = !o.dropped.is_empty();
            same_t.files += 1;
            edit_t.files += 1;
            DETAIL.store(std::env::var("KLADD_WORD_DETAIL").is_ok_and(|v| v == format!("{kind:?}")), std::sync::atomic::Ordering::Relaxed);
            if let Ok(out) = save(kind, &o.markdown, &o) {
                compare(&name, &bytes, &out, warned, &mut same_t);
                if kind == Kind::Odt {
                    odt_content_lost(&bytes, &out, warned, &mut same_t);
                }
            }
            DETAIL.store(false, std::sync::atomic::Ordering::Relaxed);
            let edited = with_word_edit(&with_added(&o.markdown));
            if let Ok(out) = save(kind, &edited, &o) {
                compare(&name, &bytes, &out, warned, &mut edit_t);
                let base = save(kind, &o.markdown, &o).map(|b| losses(&bytes, &b)).unwrap_or_default();
                for (k, v) in losses(&bytes, &out) {
                    let extra = v.saturating_sub(base.get(&k).copied().unwrap_or(0));
                    if extra > 0 {
                        let e = collateral.entry(k).or_default();
                        e.0 += extra;
                        e.1 += 1;
                        if e.2.len() < 3 {
                            e.2.push(name.clone());
                        }
                    }
                }
            }
        }
        println!("\n===== {kind:?}: följdskador av en redigering (förlust utöver oförändrad sparning) =====");
        let mut c: Vec<_> = collateral.iter().collect();
        c.sort_by_key(|(_, v)| std::cmp::Reverse(v.1));
        for (k, v) in c.iter().take(40) {
            println!("  {k}: {} st i {} filer, t.ex. {:?}", v.0, v.1, v.2);
        }
        print_tally(kind, "spara orörd", &same_t);
        print_tally(kind, "spara efter ändring", &edit_t);
        let mut s: Vec<&String> = edit_t.silent_files.iter().collect();
        s.sort();
        for x in s.iter().take(40) {
            println!("  tyst: {x}");
        }
    }
}

/// Felsökning: `KLADD_WORD_FILE=fil cargo test word_en_fil -- --ignored --nocapture` skriver ut hur en fil
/// sparas orörd (delar som skiljer sig, och `w:sectPr` före och efter).
#[test]
#[ignore]
fn word_en_fil() {
    let Some(f) = std::env::var_os("KLADD_WORD_FILE") else { return };
    let kind = if f.to_string_lossy().ends_with(".odt") { Kind::Odt } else { Kind::Docx };
    let bytes = std::fs::read(f).unwrap();
    let o = open(kind, &bytes).unwrap();
    println!("tappat: {:?}", o.dropped);
    let out = save(kind, &o.markdown, &o).unwrap();
    let (a, b) = (parts_map(&bytes), parts_map(&out));
    for (n, x) in &a {
        match b.get(n) {
            Some(y) if y == x => {}
            Some(y) => {
                println!("ändrad: {n} ({} -> {})", x.len(), y.len());
                if n == "word/document.xml" || n == "content.xml" {
                    let (sx, sy) = (String::from_utf8_lossy(x).into_owned(), String::from_utf8_lossy(y).into_owned());
                    let at = sx.chars().zip(sy.chars()).position(|(p, q)| p != q).unwrap_or(0);
                    let cut = |s: &str| s.chars().skip(at.saturating_sub(150)).take(400).collect::<String>();
                    println!("  orig : {}\n  sparad: {}", cut(&sx), cut(&sy));
                }
            }
            None if n.ends_with('/') => {}
            None => println!("saknas: {n}"),
        }
    }
}

/// Felsökning: de toppnivåelement i `word/document.xml` (uppdelat vid `<w:p`/`<w:tbl`) som finns i originalet men
/// inte i den oförändrat sparade filen.
#[test]
#[ignore]
fn word_en_fil_stycken() {
    let Some(f) = std::env::var_os("KLADD_WORD_FILE") else { return };
    let bytes = std::fs::read(f).unwrap();
    let o = open(Kind::Docx, &bytes).unwrap();
    let out = save(Kind::Docx, &o.markdown, &o).unwrap();
    let (a, b) = (parts_map(&bytes), parts_map(&out));
    let split = |x: &[u8]| -> Vec<String> {
        let s = String::from_utf8_lossy(x).into_owned();
        s.replace("<w:p>", "\n<w:p>").replace("<w:p ", "\n<w:p ").replace("<w:tbl>", "\n<w:tbl>").lines().map(str::to_owned).collect()
    };
    let (la, lb) = (split(&a["word/document.xml"]), split(&b["word/document.xml"]));
    let sb: BTreeSet<&String> = lb.iter().collect();
    let sa: BTreeSet<&String> = la.iter().collect();
    println!("markdown:\n{}", o.markdown.chars().take(1500).collect::<String>());
    for l in la.iter().filter(|l| !sb.contains(l)).take(6) {
        println!("--- bara i originalet:\n{}", l.chars().take(700).collect::<String>());
    }
    for l in lb.iter().filter(|l| !sa.contains(l)).take(6) {
        println!("--- bara i sparade:\n{}", l.chars().take(700).collect::<String>());
    }
}

/// Felsökning (ODT): Markdown före och efter en liten ändring och sparning.
#[test]
#[ignore]
fn word_odt_andring() {
    let Some(f) = std::env::var_os("KLADD_WORD_FILE") else { return };
    let bytes = std::fs::read(f).unwrap();
    let o = open(Kind::Odt, &bytes).unwrap();
    let edited = with_word_edit(&with_added(&o.markdown));
    let same = open(Kind::Odt, &save(Kind::Odt, &o.markdown, &o).unwrap()).unwrap().markdown;
    let after = open(Kind::Odt, &save(Kind::Odt, &edited, &o).unwrap()).unwrap().markdown;
    println!("--- original-md\n{}\n--- oförändrad omläst\n{}\n--- efter ändring omläst\n{}", o.markdown.chars().take(1200).collect::<String>(), same.chars().take(1200).collect::<String>(), after.chars().take(1200).collect::<String>());
}

// ================================================================== slumpprov

/// Enkel seedad slumpgenerator (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const EXTREMA: &[&str] = &["0", "-1", "1", "2147483647", "-2147483648", "4294967296", "99999999999999999999", "-9223372036854775808", "", "-0", "100000000", "65536"];

fn positions(hay: &[u8], needle: u8) -> Vec<usize> {
    hay.iter().enumerate().filter(|(_, b)| **b == needle).map(|(i, _)| i).collect()
}

/// Förstör XML-text på ett slumpat sätt (grovt, räcker för att förvränga).
fn mutate_xml(r: &mut Rng, b: &mut Vec<u8>) {
    if b.is_empty() {
        return;
    }
    let lts = positions(b, b'<');
    match r.below(12) {
        0 => {
            if lts.len() > 2 {
                let (i, j) = (r.below(lts.len() - 1), r.below(lts.len() - 1));
                b.drain(lts[i.min(j)]..lts[i.max(j) + 1]);
            }
        }
        1 => {
            if lts.len() > 2 {
                let i = r.below(lts.len() - 1);
                let piece = b[lts[i]..lts[(i + 1 + r.below(3)).min(lts.len() - 1)]].to_vec();
                let at = lts[r.below(lts.len())];
                for (k, x) in piece.iter().enumerate() {
                    b.insert(at + k, *x);
                }
            }
        }
        2 => {
            let q = positions(b, b'"');
            for _ in 0..20 {
                if q.len() < 2 {
                    break;
                }
                let i = r.below(q.len() - 1);
                let (a, z) = (q[i], q[i + 1]);
                if z > a + 1 && z - a < 24 && b[a + 1..z].iter().all(|c| c.is_ascii_digit() || *c == b'-') {
                    let v = EXTREMA[r.below(EXTREMA.len())].as_bytes().to_vec();
                    b.splice(a + 1..z, v);
                    break;
                }
            }
        }
        3 => {
            let q = positions(b, b'"');
            if q.len() > 4 {
                let i = r.below(q.len() / 2) * 2;
                let (a, z) = (q[i], q[i + 1]);
                let mut s = a;
                while s > 0 && b[s - 1] != b' ' {
                    s -= 1;
                }
                if s > 0 && z > s {
                    b.drain(s - 1..=z);
                }
            }
        }
        4 => {
            for _ in 0..1 + r.below(4) {
                let i = r.below(b.len());
                b[i] = r.next() as u8;
            }
        }
        5 => {
            let at = r.below(b.len());
            b.truncate(at);
        }
        6 => {
            if !lts.is_empty() {
                let i = lts[r.below(lts.len())];
                b.splice(i + 1..i + 1, b"zz:".iter().copied());
            }
        }
        7 => {
            let digits: Vec<usize> = b.iter().enumerate().filter(|(_, c)| c.is_ascii_digit()).map(|(i, _)| i).collect();
            if !digits.is_empty() {
                let i = digits[r.below(digits.len())];
                b.splice(i..i, std::iter::repeat_n(b'9', 5 + r.below(30)));
            }
        }
        8 => {
            let digits: Vec<usize> = b.iter().enumerate().filter(|(i, c)| c.is_ascii_digit() && *i > 0 && b[*i - 1] == b'"').map(|(i, _)| i).collect();
            if !digits.is_empty() {
                let i = digits[r.below(digits.len())];
                b.insert(i, b'-');
            }
        }
        9 => {
            // Upprepa en öppningstagg (djup nästling).
            if !lts.is_empty() {
                let i = lts[r.below(lts.len())];
                let tag: Vec<u8> = b[i..].iter().take_while(|c| **c != b'>').copied().chain([b'>']).collect();
                if tag.len() < 80 && !tag.starts_with(b"</") && !tag.starts_with(b"<?") {
                    let rep: Vec<u8> = std::iter::repeat_n(tag, 5 + r.below(60)).flatten().collect();
                    b.splice(i..i, rep);
                }
            }
        }
        10 => {
            // Ta bort en sluttagg (obalanserat).
            let ends: Vec<usize> = lts.iter().copied().filter(|&i| b.get(i + 1) == Some(&b'/')).collect();
            if !ends.is_empty() {
                let i = ends[r.below(ends.len())];
                let e = b[i..].iter().position(|c| *c == b'>').map_or(b.len(), |p| i + p + 1);
                b.drain(i..e);
            }
        }
        _ => {
            // Byt ett attributvärde mot ett annat attributs värde (id som pekar fel).
            let q = positions(b, b'"');
            if q.len() > 6 {
                let (i, j) = (r.below(q.len() / 2) * 2, r.below(q.len() / 2) * 2);
                let v = b[q[j] + 1..q[j + 1]].to_vec();
                b.splice(q[i] + 1..q[i + 1], v);
            }
        }
    }
}

/// Förvränger ett paket (docx eller odt): zip-nivå, XML-nivå och relationer/hänvisningar.
pub fn mutate(r: &mut Rng, orig: &[u8]) -> Vec<u8> {
    let Some(mut parts) = read_zip(orig) else { return orig.to_vec() };
    for _ in 0..1 + r.below(3) {
        if parts.is_empty() {
            break;
        }
        match r.below(10) {
            0 => {
                let i = r.below(parts.len());
                parts.remove(i);
            }
            1 => {
                let i = r.below(parts.len());
                let mut n = parts[i].0.clone().into_bytes();
                if !n.is_empty() {
                    let k = r.below(n.len());
                    n[k] = b'a' + r.below(26) as u8;
                }
                parts[i].0 = String::from_utf8_lossy(&n).into_owned();
            }
            2 | 3 => {
                // Relationer: peka mål på sig själv, på en annan del eller på en saknad (docx: `.rels`; odt: manifestet
                // och `xlink:href` i innehållet).
                let cand: Vec<usize> = (0..parts.len()).filter(|&i| parts[i].0.ends_with(".rels") || parts[i].0 == "META-INF/manifest.xml" || parts[i].0 == "content.xml").collect();
                if !cand.is_empty() {
                    let i = cand[r.below(cand.len())];
                    let s = String::from_utf8_lossy(&parts[i].1).into_owned();
                    let target = match r.below(3) {
                        0 => parts[i].0.clone(),
                        1 => parts[r.below(parts.len())].0.clone(),
                        _ => "missing/part.xml".to_owned(),
                    };
                    for key in ["Target=\"", "xlink:href=\"", "manifest:full-path=\""] {
                        if let Some(p) = s.find(key) {
                            let q = s[p + key.len()..].find('"').map_or(s.len(), |q| p + key.len() + q);
                            let mut out = s[..p + key.len()].to_owned();
                            out.push_str(&target);
                            out.push_str(&s[q..]);
                            parts[i].1 = out.into_bytes();
                            break;
                        }
                    }
                }
            }
            _ => {
                let xmls: Vec<usize> = (0..parts.len()).filter(|&i| is_xml(&parts[i].0)).collect();
                // Mest brödtext: document.xml / content.xml.
                let main: Vec<usize> = xmls.iter().copied().filter(|&i| parts[i].0 == "word/document.xml" || parts[i].0 == "content.xml").collect();
                let pool = if !main.is_empty() && r.below(3) != 0 { main } else { xmls };
                if !pool.is_empty() {
                    let i = pool[r.below(pool.len())];
                    let mut b = std::mem::take(&mut parts[i].1);
                    for _ in 0..1 + r.below(3) {
                        mutate_xml(r, &mut b);
                    }
                    parts[i].1 = b;
                }
            }
        }
    }
    let mut out = write_zip(&parts);
    match r.below(14) {
        0 => {
            let at = r.below(out.len().max(1));
            out.truncate(at);
        }
        1 => {
            if !out.is_empty() {
                for _ in 0..1 + r.below(8) {
                    let i = r.below(out.len());
                    out[i] = r.next() as u8;
                }
            }
        }
        _ => {}
    }
    out
}

// ------------------------------------------------------------------ redigering (Markdown)

fn boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// En redigering som appen gör i texten: skriva, radera, fetstil, nytt stycke, tabellcell.
fn edit_markdown(r: &mut Rng, md: &str) -> String {
    let mut s = md.to_owned();
    for _ in 0..1 + r.below(3) {
        if s.is_empty() {
            s.push_str("Ny text");
            continue;
        }
        let at = boundary(&s, r.below(s.len() + 1));
        match r.below(7) {
            0 => s.insert_str(at, "Skriver ÅÄÖ "),
            1 => {
                let end = boundary(&s, at + 1 + r.below(200));
                s.replace_range(at..end, "");
            }
            2 => {
                let end = boundary(&s, at + 1 + r.below(60));
                if at < end {
                    let sel = s[at..end].to_owned();
                    s.replace_range(at..end, &format!("**{sel}**"));
                }
            }
            3 => s.insert_str(at, "\n\nNytt stycke\n\n"),
            4 => {
                // Tabellcell: lägg text i en rad som börjar med `|`.
                if let Some(l) = s.lines().position(|l| l.starts_with('|')) {
                    let start: usize = s.lines().take(l).map(|x| x.len() + 1).sum();
                    let line = s.lines().nth(l).unwrap_or("").to_owned();
                    let cut = boundary(&s, start + line.rfind('|').unwrap_or(0));
                    s.insert_str(cut, " cell ");
                }
            }
            5 => s.insert_str(at, "\n- punkt\n    - under\n"),
            _ => {
                let w = at.min(s.len());
                s.insert_str(w, "<!--k:9999-->");
            }
        }
    }
    s
}

// ------------------------------------------------------------------ visning

fn ctx() -> &'static egui::Context {
    static C: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let c = egui::Context::default();
        crate::theme::install_fonts(&c);
        c
    })
}

/// Det Word-vyn gör för att visa en text: analys, tabeller, typsnitt, sidlayout och sidräkning.
fn display(md: &str) {
    use crate::word::{layout, pages};
    // Begränsa texten: layouten av stora dokument tar tid och är redan mätt separat.
    let text: String = md.chars().take(120_000).collect();
    let pal = crate::theme::palette(false);
    let c = ctx().clone();
    let measure = {
        let c = c.clone();
        move |s: &str, f: egui::FontId| c.fonts_mut(|fo| fo.layout_no_wrap(s.to_owned(), f, egui::Color32::WHITE).size().x)
    };
    let fam = |_: Option<&str>, bold: bool, italic: bool| crate::theme::doc_font(bold, italic);
    let img = |_: &str| None;
    let mut cache = layout::Cache::default();
    let out = c.run_ui(Default::default(), |_| {
        let a = cache.analysis(1, text.as_str());
        let geom = pages::Geom::new(1.0, &a.defaults);
        let _ = cache.tables(1, text.as_str());
        let _ = cache.fonts(1, text.as_str());
        let _ = cache.headings(1, text.as_str());
        let _ = cache.images(1, text.as_str());
        let p = layout::Params { palette: &pal, zoom: 1.0, reveal: 0..0, image_size: &img, measure: &measure, font_family: &fam, fonts_gen: 0 };
        let g = cache.galley_paged(&c, 1, text.as_str(), &p, geom.text_w(), Some(&geom));
        let _ = pages::page_count(&g, &geom);
    });
    std::mem::forget(out);
}

// ------------------------------------------------------------------ ett fall

static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn vm_hwm_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok())))
        .unwrap_or(0)
}

/// Är en sparad fil ett giltigt paket? Zip går att läsa, huvuddelen finns och är välformad XML.
fn check_saved(kind: Kind, bytes: &[u8]) -> Result<(), String> {
    let parts = read_zip(bytes).ok_or("sparad fil är ingen zip")?;
    // Huvuddelen kan heta något annat än `word/document.xml` (`word/document2.xml`): hitta den på innehållet.
    let main = if kind == Kind::Docx { "word/*.xml med w:body" } else { "content.xml" };
    let x = parts
        .iter()
        .find(|(n, b)| if kind == Kind::Odt { n == "content.xml" } else { n.starts_with("word/") && n.ends_with(".xml") && b.windows(7).any(|w| w == b"<w:body") })
        .ok_or_else(|| format!("{main} saknas i sparad fil"))?;
    let s = std::str::from_utf8(&x.1).map_err(|_| format!("{} är inte UTF-8", x.0))?;
    let mut r = quick_xml::Reader::from_str(s);
    loop {
        match r.read_event() {
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => return Err(format!("{} är inte välformad XML: {e}", x.0)),
            _ => {}
        }
    }
    if kind == Kind::Docx && !parts.iter().any(|(n, _)| n == "[Content_Types].xml") {
        return Err("[Content_Types].xml saknas".into());
    }
    Ok(())
}

/// Kör alla steg för en (muterad) fil. `Err` = något att rapportera.
pub fn exercise(kind: Kind, bytes: Vec<u8>, seed: u64) -> Result<(), String> {
    let Ok(o) = open(kind, &bytes) else { return Ok(()) };
    let mut r = Rng::new(seed ^ 0xABCD);
    display(&o.markdown);
    // Oförändrad sparning och några redigeringar: varje sparad fil ska vara giltig och gå att läsa om.
    let mut variants = vec![o.markdown.clone()];
    for _ in 0..3 {
        variants.push(edit_markdown(&mut r, &o.markdown));
    }
    for (i, md) in variants.iter().enumerate() {
        let out = save(kind, md, &o).map_err(|e| format!("spara variant {i}: {e}"));
        let Ok(out) = out else {
            // Ett fel (Err) vid sparning är tillåtet för en trasig fil, men inte för den oförändrade.
            if i == 0 {
                return out.map(|_| ());
            }
            continue;
        };
        check_saved(kind, &out).map_err(|e| format!("variant {i}: {e}"))?;
        let again = open(kind, &out).map_err(|e| format!("sparad variant {i} kan inte läsas om: {e}"))?;
        if i == 1 {
            display(&again.markdown);
        }
    }
    Ok(())
}

fn run_case(kind: Kind, bytes: Vec<u8>, seed: u64, limit: Duration) -> Option<(String, String)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let before = PANICS.lock().map(|p| p.len()).unwrap_or(0);
    let h = std::thread::Builder::new().stack_size(64 << 20).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(kind, bytes, seed)));
        let _ = tx.send(r);
    });
    if h.is_err() {
        return Some(("trådfel".into(), String::new()));
    }
    match rx.recv_timeout(limit) {
        Ok(Ok(Ok(()))) => None,
        Ok(Ok(Err(e))) => Some((format!("ogiltigt: {}", e.chars().take(60).collect::<String>()), e)),
        Ok(Err(_)) => {
            let p = PANICS.lock().map(|p| p.get(before).cloned().unwrap_or_default()).unwrap_or_default();
            let key = p.split(" | ").next().unwrap_or("panik").to_owned();
            Some((format!("panik {key}"), p))
        }
        Err(_) => Some(("hängning".into(), String::new())),
    }
}

/// Slumpprovet. `KLADD_WORD_FUZZ=<antal>` fall över Word- och ODT-korpusen (`KLADD_WORD_FUZZ_SEED`,
/// `KLADD_WORD_FUZZ_OUT=<mapp för felande filer>`).
#[test]
#[ignore]
fn word_slumpprov() {
    let Some(n) = std::env::var("KLADD_WORD_FUZZ").ok().and_then(|v| v.parse::<usize>().ok()) else { return };
    let seed: u64 = std::env::var("KLADD_WORD_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let out_dir = std::env::var_os("KLADD_WORD_FUZZ_OUT").map(PathBuf::from);
    let mut files: Vec<(Kind, PathBuf)> = corpus(Kind::Docx).into_iter().map(|p| (Kind::Docx, p)).collect();
    files.extend(corpus(Kind::Odt).into_iter().map(|p| (Kind::Odt, p)));
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        if let Ok(mut p) = PANICS.lock() {
            p.push(format!("{loc} | {}", msg.chars().take(160).collect::<String>()));
        }
    }));
    let mut classes: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let (mut hangs, mut max_mem) = (0, (0u64, 0usize));
    let hwm0 = vm_hwm_kb();
    for case in 0..n {
        let case_seed = seed.wrapping_mul(1_000_003).wrapping_add(case as u64);
        let mut r = Rng::new(case_seed);
        let (kind, f) = files[r.below(files.len())].clone();
        let Ok(orig) = std::fs::read(&f) else { continue };
        // Var tredje fall är orört (bara redigeringarna), resten förvrängs.
        let bytes = if r.below(3) == 0 { orig.clone() } else { mutate(&mut r, &orig) };
        println!("fall {case} seed {case_seed} {kind:?} {}", f.file_name().unwrap().to_string_lossy());
        let m0 = vm_hwm_kb();
        let res = run_case(kind, bytes.clone(), case_seed, Duration::from_secs(40));
        let grow = vm_hwm_kb().saturating_sub(m0);
        if grow > max_mem.0 {
            max_mem = (grow, case);
        }
        if let Some((key, text)) = res {
            let e = classes.entry(key.clone()).or_insert((0, String::new()));
            e.0 += 1;
            if e.0 == 1 {
                e.1 = format!("fall {case} seed {case_seed} {kind:?} {} :: {text}", f.display());
                if let Some(d) = &out_dir {
                    let _ = std::fs::create_dir_all(d);
                    let safe: String = key.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).take(60).collect();
                    let _ = std::fs::write(d.join(format!("{safe}.{}", if kind == Kind::Docx { "docx" } else { "odt" })), &bytes);
                }
            }
            if key == "hängning" {
                hangs += 1;
                if hangs >= 3 {
                    println!("tre hängningar: avbryter");
                    break;
                }
            }
        }
    }
    println!("== {n} fall, seed {seed}, korpus {} filer, minne {} -> {} kB (störst ökning {} kB i fall {})", files.len(), hwm0, vm_hwm_kb(), max_mem.0, max_mem.1);
    for (k, (c, first)) in &classes {
        println!("KLASS {c:5} × {k}\n        {first}");
    }
    println!("{} klasser", classes.len());
    assert!(classes.is_empty(), "{} felklasser", classes.len());
}

/// Kör ett enda fall ur en fil (`KLADD_WORD_FILE`, `KLADD_WORD_SEED`) och skriver resultatet.
#[test]
#[ignore]
fn word_ett_fall() {
    let Some(f) = std::env::var_os("KLADD_WORD_FILE") else { return };
    let kind = if f.to_string_lossy().ends_with(".odt") { Kind::Odt } else { Kind::Docx };
    let bytes = std::fs::read(f).unwrap();
    let seed: u64 = std::env::var("KLADD_WORD_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let t = std::time::Instant::now();
    println!("{:?} i {:?}", exercise(kind, bytes, seed), t.elapsed());
}

// ------------------------------------------------------------------ regressionsprov

/// Klass 1 (slumpprov): ett paket utan `[Content_Types].xml` som ändå gick att läsa sparades utan det, så Word
/// vägrade öppna filen.
#[test]
fn paket_utan_innehallstyper_far_det_vid_sparning() {
    let md = "Rad ett\n\nRad två\n";
    let w = crate::docx::write(md, "T", &|_| None, None).unwrap().bytes;
    let parts: Vec<(String, Vec<u8>)> = read_zip(&w).unwrap().into_iter().filter(|(n, _)| n != "[Content_Types].xml").collect();
    let broken = write_zip(&parts);
    let l = crate::docx::read(&broken).unwrap();
    let out = crate::docx::write(&l.markdown.replace("ett", "1"), "T", &|_| None, l.source.as_ref()).unwrap().bytes;
    let ct = String::from_utf8(parts_map(&out)["[Content_Types].xml"].clone()).unwrap();
    assert!(ct.contains("/word/document.xml") && ct.contains("wordprocessingml.document.main+xml"), "{ct}");
    assert!(ct.contains("/word/styles.xml"), "{ct}");
    check_saved(Kind::Docx, &out).unwrap();
    assert!(crate::docx::read(&out).unwrap().markdown.contains("Rad 1"));
}

/// Klass 2 (slumpprov): en rad inne i ett HTML-block som börjar med ett flerbytetecken (`ㄕ`) skars vid byte 4 mitt i
/// tecknet när ODT-skrivaren letade efter `<img`.
#[test]
fn html_rad_som_borjar_med_flerbytetecken_ger_inte_panik() {
    for md in ["<div>\nㄕㄕㄕ\n\nText\n", "<div>\nÅÄÖÅÄÖ\n</div>\n", "<span>\n\u{1F600}\u{1F600}\n"] {
        let out = std::panic::catch_unwind(|| crate::odt::write(md, "T", &|_| None));
        assert!(out.is_ok(), "panik för {md:?}");
        let out = std::panic::catch_unwind(|| crate::docx::write(md, "T", &|_| None, None));
        assert!(out.is_ok(), "panik (docx) för {md:?}");
    }
}

/// Fältinstruktion som börjar med ett flerbytetecken (`ÄTOC`) skars vid byte 3 i `docx.rs`.
#[test]
fn faltinstruktion_med_flerbytetecken_ger_inte_panik() {
    let w = crate::docx::write("x\n", "T", &|_| None, None).unwrap().bytes;
    let mut parts = read_zip(&w).unwrap();
    let doc = parts.iter_mut().find(|(n, _)| n == "word/document.xml").unwrap();
    let xml = String::from_utf8(std::mem::take(&mut doc.1)).unwrap();
    let field = "<w:p><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> ÅÅ TOC </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>x</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>";
    doc.1 = xml.replacen("<w:body>", &format!("<w:body>{field}"), 1).into_bytes();
    let bytes = write_zip(&parts);
    let r = std::panic::catch_unwind(|| crate::docx::read(&bytes));
    assert!(r.is_ok(), "panik vid läsning");
}

/// Lagar om en del i en docx med en funktion och ger nya bytes.
fn patch_part(bytes: &[u8], part: &str, f: impl Fn(String) -> String) -> Vec<u8> {
    let mut parts = read_zip(bytes).unwrap();
    let p = parts.iter_mut().find(|(n, _)| n == part).unwrap();
    p.1 = f(String::from_utf8(std::mem::take(&mut p.1)).unwrap()).into_bytes();
    write_zip(&parts)
}

fn part_str(bytes: &[u8], part: &str) -> String {
    String::from_utf8(parts_map(bytes)[part].clone()).unwrap()
}

/// Bevarande 1: en lista som hoppar över en nivå (ilvl 0, sedan 2) skrevs om som kodblock vid en oförändrad
/// sparning, med listmarkörer som läckte ut som text. Nu behåller stycket sin numrering orört.
#[test]
fn hoppad_listniva_bevaras_vid_oforandrad_sparning() {
    let base = crate::docx::write("1. a\n    1. b\n        1. c\n", "T", &|_| None, None).unwrap().bytes;
    let skipped = patch_part(&base, "word/document.xml", |x| x.replace("<w:ilvl w:val=\"1\"/>", "<w:ilvl w:val=\"2\"/>"));
    let before = part_str(&skipped, "word/document.xml");
    assert!(before.matches("<w:ilvl w:val=\"2\"/>").count() == 2 && !before.contains("<w:ilvl w:val=\"1\"/>"), "{before}");
    let l = crate::docx::read(&skipped).unwrap();
    let out = crate::docx::write(&l.markdown, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
    let after = part_str(&out, "word/document.xml");
    assert_eq!(after.matches("<w:numPr>").count(), 3, "{after}");
    assert_eq!(after.matches("<w:ilvl w:val=\"2\"/>").count(), 2, "{after}");
    assert!(!after.contains("Code") && !after.contains("&lt;!--k:"), "{after}");
}

/// Bevarande 2: `w:type continuous` i det första (här enda) avsnittet och `w:pgNumType w:fmt="decimal"` ska stå kvar
/// orörda vid en oförändrad sparning (de tappades tyst).
#[test]
fn avsnittstyp_och_sidnumrering_bevaras_vid_oforandrad_sparning() {
    let base = crate::docx::write("Text\n", "T", &|_| None, None).unwrap().bytes;
    let doc = patch_part(&base, "word/document.xml", |x| {
        let x = x.replacen("<w:pgSz", "<w:type w:val=\"continuous\"/><w:pgSz", 1);
        let end = x.find("<w:cols").unwrap_or(x.rfind("</w:sectPr>").unwrap());
        format!("{}<w:pgNumType w:fmt=\"decimal\"/>{}", &x[..end], &x[end..])
    });
    let l = crate::docx::read(&doc).unwrap();
    let out = crate::docx::write(&l.markdown, "T", &|_| None, l.source.as_ref()).unwrap().bytes;
    let after = part_str(&out, "word/document.xml");
    assert!(after.contains("<w:type w:val=\"continuous\"/>"), "{after}");
    assert!(after.contains("<w:pgNumType w:fmt=\"decimal\"/>"), "{after}");
}

/// Felsökning (ODT): vilka bilder i originalet som saknas (efter innehåll) i den oförändrat sparade filen.
#[test]
#[ignore]
fn word_odt_bilder() {
    let mut n = 0;
    for f in corpus(Kind::Odt) {
        let bytes = std::fs::read(&f).unwrap();
        let Ok(Ok(o)) = std::panic::catch_unwind(|| open(Kind::Odt, &bytes)) else { continue };
        let Ok(out) = save(Kind::Odt, &o.markdown, &o) else { continue };
        let (a, b) = (parts_map(&bytes), parts_map(&out));
        let have: BTreeSet<&Vec<u8>> = b.values().collect();
        let content = a.get("content.xml").map(|x| String::from_utf8_lossy(x).into_owned()).unwrap_or_default();
        for (name, x) in &a {
            if name.starts_with("Pictures/") && !name.ends_with('/') && !have.contains(x) {
                n += 1;
                let referenced = content.contains(name.as_str());
                let in_md = o.markdown.contains("bild:");
                println!("{}: {name} ({} byte) refererad i content.xml: {referenced}; md har bild: {in_md}; varnat: {:?}", f.file_name().unwrap().to_string_lossy(), x.len(), o.dropped);
            }
        }
    }
    println!("{n} bilder saknas");
}
