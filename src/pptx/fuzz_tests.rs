//! Slumpprov (fuzz) av läsning, ritning, sökning, tidslinjer, redigering och sparande.
//!
//! En egen deterministisk mutator (xorshift) tar korpusfiler och förstör dem på flera nivåer: zip (ta bort och byta
//! namn på poster, trunkera, byta byte), XML (ta bort/duplicera/byta element och attribut, extrema tal, ogiltig UTF-8,
//! okända namnrymder) och relationer (mål som saknas eller pekar på delen själv). Varje fall körs i en egen tråd under
//! `catch_unwind` med tidsgräns. Ignorerat prov: `KLADD_PPTX_FUZZ=<antal>` (och `KLADD_PPTX_REFS=<mapp med .pptx>`;
//! `KLADD_PPTX_FUZZ_SEED`, `KLADD_PPTX_FUZZ_OUT=<mapp för felande filer>`). Själva mutatorn och en liten körning
//! på en egen fixtur ingår i vanliga `cargo test`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::Duration;

use super::edit::Session;
use super::edit_ops::{self, Xf};
use super::edit_tests::validate;
use super::Deck;

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

const EXTREMA: &[&str] = &["0", "-1", "1", "2147483647", "-2147483648", "4294967296", "99999999999999999999", "-9223372036854775808", "1e308", "NaN", "", "-0", "100000000"];

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
    let mut seen = std::collections::HashSet::new();
    for (n, b) in parts {
        if !seen.insert(n.clone()) || z.start_file(n, opt).is_err() {
            continue;
        }
        let _ = z.write_all(b);
    }
    z.finish().map(|c| c.into_inner()).unwrap_or_default()
}

/// Lägen (byteindex) för `needle` i `hay`.
fn positions(hay: &[u8], needle: u8) -> Vec<usize> {
    hay.iter().enumerate().filter(|(_, b)| **b == needle).map(|(i, _)| i).collect()
}

/// Ett element ur XML-texten: från en `<` till motsvarande `>` (grovt, räcker för att förstöra).
fn mutate_xml(r: &mut Rng, b: &mut Vec<u8>) {
    if b.is_empty() {
        return;
    }
    let lts = positions(b, b'<');
    match r.below(12) {
        0 => {
            // Ta bort ett stycke mellan två `<`.
            if lts.len() > 2 {
                let (i, j) = (r.below(lts.len() - 1), r.below(lts.len() - 1));
                let (a, z) = (lts[i.min(j)], lts[i.max(j) + 1]);
                b.drain(a..z);
            }
        }
        1 => {
            // Duplicera ett stycke.
            if lts.len() > 2 {
                let i = r.below(lts.len() - 1);
                let (a, z) = (lts[i], lts[(i + 1 + r.below(3)).min(lts.len() - 1)]);
                let piece = b[a..z].to_vec();
                let at = lts[r.below(lts.len())];
                for (k, x) in piece.iter().enumerate() {
                    b.insert(at + k, *x);
                }
            }
        }
        2 => {
            // Byt ett talvärde i ett attribut mot ett extremt.
            let quotes = positions(b, b'"');
            for _ in 0..20 {
                if quotes.len() < 2 {
                    break;
                }
                let i = r.below(quotes.len() - 1);
                let (a, z) = (quotes[i], quotes[i + 1]);
                if z > a + 1 && z - a < 24 && b[a + 1..z].iter().all(|c| c.is_ascii_digit() || *c == b'-') {
                    let v = EXTREMA[r.below(EXTREMA.len())].as_bytes().to_vec();
                    b.splice(a + 1..z, v);
                    break;
                }
            }
        }
        3 => {
            // Ta bort ett attribut (` namn="värde"`).
            let quotes = positions(b, b'"');
            if quotes.len() > 4 {
                let i = r.below(quotes.len() / 2) * 2;
                let (a, z) = (quotes[i], quotes[i + 1]);
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
            // Byt tecken mot slumpbyte (ogiltig UTF-8 möjlig).
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
            // Okänd namnrymd på ett elementnamn.
            if !lts.is_empty() {
                let i = lts[r.below(lts.len())];
                b.splice(i + 1..i + 1, b"zz:".iter().copied());
            }
        }
        7 => {
            // Byt tagg mot en annan förekommande tagg.
            if lts.len() > 3 {
                let name = |at: usize, b: &[u8]| -> (usize, usize) {
                    let mut e = at + 1;
                    while e < b.len() && !matches!(b[e], b' ' | b'>' | b'/') {
                        e += 1;
                    }
                    (at + 1, e)
                };
                let (a0, a1) = name(lts[r.below(lts.len())], b);
                let (b0, b1) = name(lts[r.below(lts.len())], b);
                if b[a0..a1].first() != Some(&b'/') && b[b0..b1].first() != Some(&b'/') && b1 > b0 {
                    let rep = b[b0..b1].to_vec();
                    b.splice(a0..a1, rep);
                }
            }
        }
        8 => {
            // Stor siffra som text/attribut: lägg till en lång nollföljd efter en siffra.
            let digits: Vec<usize> = b.iter().enumerate().filter(|(_, c)| c.is_ascii_digit()).map(|(i, _)| i).collect();
            if !digits.is_empty() {
                let i = digits[r.below(digits.len())];
                b.splice(i..i, std::iter::repeat_n(b'9', 5 + r.below(30)));
            }
        }
        9 => {
            // Negativ storlek: sätt ett minustecken före en siffra.
            let digits: Vec<usize> = b.iter().enumerate().filter(|(i, c)| c.is_ascii_digit() && *i > 0 && b[*i - 1] == b'"').map(|(i, _)| i).collect();
            if !digits.is_empty() {
                let i = digits[r.below(digits.len())];
                b.insert(i, b'-');
            }
        }
        10 => {
            // Töm text i ett `<a:t>`.
            if let Some(p) = b.windows(5).position(|w| w == b"<a:t>") {
                if let Some(q) = b[p..].windows(6).position(|w| w == b"</a:t>") {
                    b.drain(p + 5..p + q);
                }
            }
        }
        _ => {
            // Dubbla alla barn i ett litet stycke (rekursionsdjup): upprepa en öppningstagg.
            if !lts.is_empty() {
                let i = lts[r.below(lts.len())];
                let tag: Vec<u8> = b[i..].iter().take_while(|c| **c != b'>').copied().chain([b'>']).collect();
                if tag.len() < 80 && !tag.starts_with(b"</") && !tag.starts_with(b"<?") {
                    let rep: Vec<u8> = std::iter::repeat_n(tag, 5 + r.below(40)).flatten().collect();
                    b.splice(i..i, rep);
                }
            }
        }
    }
}

/// Muterar en presentation (zip-bytes) och ger nya zip-bytes.
pub fn mutate(r: &mut Rng, orig: &[u8]) -> Vec<u8> {
    let Some(mut parts) = read_zip(orig) else { return orig.to_vec() };
    let rounds = 1 + r.below(3);
    for _ in 0..rounds {
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
            2 => {
                // Mutera en relationsfil: peka mål på sig själv, en annan del eller en saknad.
                let rels: Vec<usize> = (0..parts.len()).filter(|&i| parts[i].0.ends_with(".rels")).collect();
                if !rels.is_empty() {
                    let i = rels[r.below(rels.len())];
                    let s = String::from_utf8_lossy(&parts[i].1).into_owned();
                    let target = match r.below(3) {
                        0 => parts[i].0.replace("_rels/", "").trim_end_matches(".rels").to_owned(),
                        1 => parts[r.below(parts.len())].0.clone(),
                        _ => "missing/part.xml".to_owned(),
                    };
                    if let Some(p) = s.find("Target=\"") {
                        let q = s[p + 8..].find('"').map_or(s.len(), |q| p + 8 + q);
                        let mut out = s[..p + 8].to_owned();
                        out.push_str(&target);
                        out.push_str(&s[q..]);
                        parts[i].1 = out.into_bytes();
                    }
                }
            }
            _ => {
                let xmls: Vec<usize> = (0..parts.len()).filter(|&i| parts[i].0.ends_with(".xml") || parts[i].0.ends_with(".rels")).collect();
                if !xmls.is_empty() {
                    let i = xmls[r.below(xmls.len())];
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
    match r.below(12) {
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

/// Panikplats och meddelande, fångade av en panikkrok under körningen.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn vm_hwm_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok())))
        .unwrap_or(0)
}

/// Kör alla steg på en (muterad) presentation. `Err` = ett fel som provet ska rapportera.
pub fn exercise(bytes: Vec<u8>) -> Result<(), String> {
    // `validate` (provets hjälpfunktion) packar upp med unwrap: en trasig zip räknas som underkänd i stället för panik.
    let safe_validate = |b: &[u8]| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| validate(b))).unwrap_or_else(|_| vec!["zip kan inte packas upp".into()]);
    let pre = safe_validate(&bytes);
    let Ok(deck) = Deck::open(&bytes) else { return Ok(()) };
    let n = deck.slides.len().min(6);
    for i in 0..n {
        let scene = super::render::slide_scene(&deck, i);
        let _ = super::raster::render(&scene, 240, 135);
        let st = super::find::slide_text(&scene);
        let _ = super::find::hits_for(i, &st, "", "a", false);
    }
    let _ = super::pdf::write(&deck, &(0..n).collect::<Vec<_>>());
    let tls: Vec<std::sync::Arc<super::anim::Timeline>> = deck.slides.iter().take(n).map(|s| std::sync::Arc::new(super::anim::Timeline::of_slide(&s.root))).collect();
    let _ = super::presenter::timings(&deck, &tls, true);
    let _ = deck.size_pt();
    // Redigering och sparande.
    let mut s = Session::new(bytes.clone(), &deck);
    let mut deck = deck;
    let step = |s: &mut Session, deck: &mut Deck, f: &dyn Fn(&mut super::edit::Txn) -> Result<(), String>| {
        if let Ok(Some(a)) = s.apply(deck, None, f) {
            *deck = a.deck;
        }
    };
    if n > 0 {
        if let Some(sh) = edit_ops::shapes(&deck, 0).into_iter().next() {
            for xf in [Xf { x: sh.xf.x.wrapping_add(12_700), ..sh.xf }, Xf { x: i64::MAX, y: i64::MIN, cx: -1, cy: i64::MAX, ..sh.xf }, Xf { cx: 0, cy: 0, ..sh.xf }] {
                step(&mut s, &mut deck, &|t| edit_ops::set_xf(t, 0, sh.index, sh.id, xf));
            }
            let _ = edit_ops::shapes(&deck, 0);
            step(&mut s, &mut deck, &|t| edit_ops::duplicate_shape(t, 0, sh.index, sh.id).map(|_| ()));
            step(&mut s, &mut deck, &|t| edit_ops::delete_shape(t, 0, sh.index, sh.id));
        }
        // Text: tecken, radbrytning, formatering och autoanpassning i första formen med text.
        if let Some(sh) = edit_ops::shapes(&deck, 0).into_iter().find(|s| s.kind == edit_ops::Kind::Shape && !s.alternate) {
            let part0 = deck.slides[0].part.clone();
            if let Some(doc) = deck.pkg.read(&part0).and_then(|b| super::edit_xml::parse(b)) {
                if let Some(sp) = edit_ops::raw_shape(&doc, sh.index) {
                    let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
                    let mut m = super::edit_text::TextModel::from_body(sp.child("p:txBody"), &doc.root_scope(), &a);
                    let end = m.end();
                    let p = m.insert(end, "Kladd ");
                    let p = m.delete(m.step(p, false), p);
                    let p = m.split_para(p);
                    m.insert(p, "ny rad");
                    m.toggle(super::edit_text::Pos { para: 0, ch: 0 }, m.end(), "b");
                    let node = edit_ops::node(&deck, 0, sh.index);
                    let fmts = node.and_then(|n| m.carets(&deck, 0, n)).map(|c| c.paras).unwrap_or_default();
                    let end = m.end();
                    m.step_size(super::edit_text::Pos::default(), end, true, &fmts);
                    m.set_align(super::edit_text::Pos::default(), end, "ctr");
                    m.set_bullet(super::edit_text::Pos::default(), end, true);
                    let fit = node.and_then(|n| m.autofit(&deck, 0, n, sh.xf, None));
                    step(&mut s, &mut deck, &|t| super::edit_fit::set_text_fit(t, 0, sh.index, sh.id, &m, fit, sh.xf));
                }
            }
        }
        // Första tabellens första cell.
        if let Some(sh) = edit_ops::shapes(&deck, 0).into_iter().find(|s| super::edit_table::is_editable_table(&deck, 0, s.index)) {
            let part0 = deck.slides[0].part.clone();
            if let Some(doc) = deck.pkg.read(&part0).and_then(|b| super::edit_xml::parse(b)) {
                let a = doc.prefix_for(super::edit::NS_A).unwrap_or_else(|| "a".into());
                if let Some(mut m) = super::edit_table::cell_model(&doc, sh.index, (0, 0), &a) {
                    let end = m.end();
                    m.insert(end, " ändrad");
                    step(&mut s, &mut deck, &|t| super::edit_table::set_cell_text(t, 0, sh.index, sh.id, (0, 0), &m));
                }
            }
        }
        step(&mut s, &mut deck, &|t| edit_ops::insert_textbox(t, 0, Xf { x: 0, y: 0, cx: 914_400, cy: 457_200, rot: 0, flip_h: false, flip_v: false }).map(|_| ()));
        step(&mut s, &mut deck, &|t| edit_ops::set_notes(t, 0, "Prov"));
        step(&mut s, &mut deck, &|t| edit_ops::duplicate_slide(t, 0).map(|_| ()));
        step(&mut s, &mut deck, &|t| edit_ops::move_slide(t, 0, 1));
        step(&mut s, &mut deck, &|t| edit_ops::set_hidden(t, 0, true));
    }
    if !deck.layouts.is_empty() {
        let layout = deck.slides.first().and_then(|s| s.layout).unwrap_or(0);
        step(&mut s, &mut deck, &|t| edit_ops::new_slide(t, if n > 0 { Some(0) } else { None }, layout).map(|_| ()));
    }
    if !deck.slides.is_empty() {
        let last = deck.slides.len() - 1;
        step(&mut s, &mut deck, &|t| edit_ops::delete_slide(t, last));
    }
    let Ok((out, _)) = s.save_bytes(&deck, "pptx", "pptx") else { return Ok(()) };
    if pre.is_empty() {
        let errs: Vec<String> = safe_validate(&out);
        if !errs.is_empty() {
            return Err(format!("validate: {errs:?}"));
        }
    }
    if let Ok(d) = Deck::open(&out) {
        for i in 0..d.slides.len().min(3) {
            let _ = super::render_slide(&d, i, 160, 90);
        }
    }
    Ok(())
}

/// Resultat för ett fall: klassnyckel (panikplats, "hängning", "validate") och text.
fn run_case(bytes: Vec<u8>, limit: Duration) -> Option<(String, String)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let before = PANICS.lock().map(|p| p.len()).unwrap_or(0);
    let h = std::thread::Builder::new().stack_size(64 << 20).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exercise(bytes)));
        let _ = tx.send(r);
    });
    if h.is_err() {
        return Some(("trådfel".into(), String::new()));
    }
    match rx.recv_timeout(limit) {
        Ok(Ok(Ok(()))) => None,
        Ok(Ok(Err(e))) => Some(("validate".into(), e)),
        Ok(Err(_)) => {
            let p = PANICS.lock().map(|p| p.get(before).cloned().unwrap_or_default()).unwrap_or_default();
            let key = p.split(" | ").next().unwrap_or("panik").to_owned();
            Some((format!("panik {key}"), p))
        }
        Err(_) => Some(("hängning".into(), String::new())),
    }
}

fn corpus(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pptx")) && std::fs::metadata(&p).is_ok_and(|m| m.len() < 3_000_000) {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(root, &mut v);
    v.sort();
    v
}

/// Slumpprovet. `KLADD_PPTX_FUZZ=<antal>` fall över korpusen i `KLADD_PPTX_REFS`.
#[test]
#[ignore]
fn slumpprov_av_presentationer() {
    let Some(n) = std::env::var("KLADD_PPTX_FUZZ").ok().and_then(|v| v.parse::<usize>().ok()) else { return };
    let Some(root) = std::env::var_os("KLADD_PPTX_REFS") else { return };
    let seed: u64 = std::env::var("KLADD_PPTX_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let out_dir = std::env::var_os("KLADD_PPTX_FUZZ_OUT").map(std::path::PathBuf::from);
    let files = corpus(std::path::Path::new(&root));
    assert!(!files.is_empty(), "ingen korpus");
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        if let Ok(mut p) = PANICS.lock() {
            p.push(format!("{loc} | {}", msg.chars().take(160).collect::<String>()));
        }
    }));
    let mut classes: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut hangs = 0;
    let hwm0 = vm_hwm_kb();
    let mut max_case_mem = (0u64, 0usize);
    for case in 0..n {
        let case_seed = seed.wrapping_mul(1_000_003).wrapping_add(case as u64);
        let mut r = Rng::new(case_seed);
        let f = &files[r.below(files.len())];
        let Ok(orig) = std::fs::read(f) else { continue };
        let bytes = mutate(&mut r, &orig);
        // Loggraden skrivs före körningen: en avbruten process (minnesbrist) pekar ut fallet.
        println!("fall {case} seed {case_seed} {}", f.display());
        let mem0 = vm_hwm_kb();
        let res = run_case(bytes.clone(), Duration::from_secs(40));
        let grow = vm_hwm_kb().saturating_sub(mem0);
        if grow > max_case_mem.0 {
            max_case_mem = (grow, case);
        }
        if let Some((key, text)) = res {
            let e = classes.entry(key.clone()).or_insert((0, String::new()));
            e.0 += 1;
            if e.0 == 1 {
                e.1 = format!("fall {case} seed {case_seed} {} :: {text}", f.display());
                if let Some(d) = &out_dir {
                    let _ = std::fs::create_dir_all(d);
                    let safe: String = key.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                    let _ = std::fs::write(d.join(format!("{safe}.pptx")), &bytes);
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
    println!("== {n} fall, seed {seed}, korpus {} filer, minne {} -> {} kB (störst ökning {} kB i fall {})", files.len(), hwm0, vm_hwm_kb(), max_case_mem.0, max_case_mem.1);
    for (k, (c, first)) in &classes {
        println!("KLASS {c:5} × {k}\n        {first}");
    }
    println!("{} klasser", classes.len());
    assert!(classes.is_empty(), "{} felklasser", classes.len());
}

/// Mutatorn är deterministisk och ger zip-filer som oftast går att läsa; en liten körning på ett minimalt
/// dokument får inte panika.
#[test]
fn mutatorn_ar_deterministisk_och_kor_utan_panik() {
    use super::tests::Builder;
    let base = Builder::new().slide("", r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="T"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hej</a:t></a:r></a:p></p:txBody></p:sp>"#).build();
    let a = mutate(&mut Rng::new(7), &base);
    let b = mutate(&mut Rng::new(7), &base);
    assert_eq!(a, b);
    for seed in 0..40 {
        let m = mutate(&mut Rng::new(seed), &base);
        let r = std::panic::catch_unwind(|| exercise(m));
        assert!(r.is_ok(), "panik för seed {seed}");
    }
}

/// Kör ett enda fall ur en fil (`KLADD_PPTX_FUZZ_FILE=fil.pptx`) och skriver tiden per steg: för att
/// avgöra om en "hängning" är en riktig hängning eller bara ett långsamt fall i ett obyggt (debug) prov.
#[test]
#[ignore]
fn slumpprov_ett_fall() {
    let Some(f) = std::env::var_os("KLADD_PPTX_FUZZ_FILE") else { return };
    let bytes = std::fs::read(f).unwrap();
    let t = std::time::Instant::now();
    let deck = Deck::open(&bytes);
    println!("öppna: {:?} ({} bilder)", t.elapsed(), deck.as_ref().map_or(0, |d| d.slides.len()));
    let Ok(deck) = deck else { return };
    for i in 0..deck.slides.len().min(6) {
        let t = std::time::Instant::now();
        let scene = super::render::slide_scene(&deck, i);
        let a = t.elapsed();
        let _ = super::raster::render(&scene, 240, 135);
        println!("bild {i}: scen {a:?}, raster {:?}", t.elapsed() - a);
    }
    let t = std::time::Instant::now();
    let _ = super::pdf::write(&deck, &(0..deck.slides.len().min(6)).collect::<Vec<_>>());
    println!("pdf: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let r = exercise(bytes);
    println!("exercise: {:?} {r:?}", t.elapsed());
}

/// Byter text i en del av en minimal presentation (för regressionsprov med orimliga värden).
#[cfg(test)]
fn patch(bytes: &[u8], part: &str, from: &str, to: &str) -> Vec<u8> {
    let mut parts = read_zip(bytes).unwrap();
    let p = parts.iter_mut().find(|(n, _)| n == part).unwrap();
    let s = String::from_utf8(std::mem::take(&mut p.1)).unwrap().replace(from, to);
    assert!(s.contains(to), "{from} finns inte i {part}");
    p.1 = s.into_bytes();
    write_zip(&parts)
}

#[cfg(test)]
fn minimal_med_form(off_x: &str) -> Vec<u8> {
    use super::tests::Builder;
    Builder::new()
        .slide("", &format!(r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="T"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{off_x}" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Hej</a:t></a:r></a:p></p:txBody></p:sp>"#))
        .build()
}

/// Klass 1 (slumpprov): orimlig `p:notesSz` gav overflow när anteckningsmallen skapades.
#[test]
fn orimlig_anteckningsstorlek_ger_inte_overflow() {
    let bytes = patch(&minimal_med_form("0"), "ppt/presentation.xml", r#"<p:notesSz cx="6858000" cy="9144000"/>"#, r#"<p:notesSz cx="9223372036854775807" cy="-9223372036854775808"/>"#);
    let deck = Deck::open(&bytes).unwrap();
    assert!(deck.notes_size_emu.0 <= 51_206_400 && deck.notes_size_emu.1 >= 12_700);
    let mut s = Session::new(bytes, &deck);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.apply(&deck, None, |t| edit_ops::set_notes(t, 0, "x")).map(|_| ())));
    assert!(r.is_ok(), "panik i set_notes");
}

/// Klass 2 (slumpprov): `duplicate_shape` lade 228 600 till en position nära i64::MAX.
#[test]
fn duplicera_form_vid_ytterkant_ger_inte_overflow() {
    let bytes = minimal_med_form("9223372036854775807");
    let deck = Deck::open(&bytes).unwrap();
    let sh = edit_ops::shapes(&deck, 0).into_iter().next().unwrap();
    let mut s = Session::new(bytes, &deck);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.apply(&deck, None, |t| edit_ops::duplicate_shape(t, 0, sh.index, sh.id).map(|_| ())).map(|_| ())));
    assert!(r.is_ok(), "panik i duplicate_shape");
}
