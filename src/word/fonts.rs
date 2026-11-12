//! Systemtypsnitt för dokumentläget. På Linux via fontconfig (`fc-match`), så att t.ex. Arial
//! blir Liberation Sans och Times New Roman Liberation Serif på Ubuntu, precis som i
//! LibreOffice. På Windows och macOS (och på Linux utan fontconfig) via `fontdb`, som läser
//! systemets typsnittsmappar direkt.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Once, OnceLock};

use egui::{FontData, FontDefinitions, FontFamily};

use crate::theme;

/// Vad fontconfig svarade för ett typsnitt.
#[derive(Clone, Debug, PartialEq)]
struct Match {
    family: String,
    file: PathBuf,
    index: u32,
}

enum Entry {
    /// Ingen användbar systemfont: använd Carlito.
    Fallback,
    /// Registrerad hos egui men inte aktiv än (aktiveras vid nästa frame).
    Pending(FontFamily),
    Ready(FontFamily),
}

pub struct DocFonts {
    cache: HashMap<(String, bool, bool), Entry>,
    /// Egui:s typsnittsdefinitioner med våra tillägg (set_fonts ersätter allt, så vi samlar själva).
    defs: Option<FontDefinitions>,
    /// Filer som redan lästs: (sökväg, index) -> namn i `font_data`.
    files: HashMap<(PathBuf, u32), String>,
    generation: u64,
    installed: Option<Vec<String>>,
    /// Tillfällig lista medan fontdb fortfarande läser in typsnitten.
    installed_wait: Vec<String>,
    /// Har vi sett att fontdb blev klar (och räknat upp `generation`)?
    db_seen: bool,
    checked_frame: Option<u64>,
}

impl DocFonts {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            defs: None,
            files: HashMap::new(),
            generation: 0,
            installed: None,
            installed_wait: Vec::new(),
            db_seen: false,
            checked_frame: None,
        }
    }

    /// Ökar när ett typsnitt blivit användbart, så att anroparen kan göra om cachade layouter.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Egui-familj för ett dokumenttypsnitt. Ger Carlito tills systemtypsnittet är aktivt.
    /// Typsnittet räknas som klart när egui:s `families()` innehåller familjen (egui
    /// kraschar annars på en okänd `FontFamily::Name`), alltså först när definitionerna
    /// tillämpats av en frame. Måste anropas medan en frame pågår.
    pub fn family(
        &mut self,
        ctx: &egui::Context,
        name: Option<&str>,
        bold: bool,
        italic: bool,
    ) -> FontFamily {
        let fallback = theme::doc_font(bold, italic);
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
            return fallback;
        };
        // På Windows/macOS finns ofta det riktiga Calibri (Office): använd det före Carlito.
        let real_calibri = prefer_real_calibri() && normalize(name) == "calibri";
        if is_bundled(name) && !real_calibri {
            return fallback;
        }
        // Databasen läses in i bakgrunden; när den blivit klar kan tidigare Carlito-svar bytas.
        if !use_fontconfig() && !self.db_seen && db().is_some() {
            self.db_seen = true;
            self.generation += 1;
        }
        self.recheck(ctx);
        let key = (name.to_lowercase(), bold, italic);
        match self.cache.get(&key) {
            Some(Entry::Fallback) => return fallback,
            Some(Entry::Ready(fam)) => return fam.clone(),
            Some(Entry::Pending(fam)) => {
                let fam = fam.clone();
                if ctx.fonts(|f| f.families().contains(&fam)) {
                    self.generation += 1;
                    self.cache.insert(key, Entry::Ready(fam.clone()));
                    return fam;
                }
                return fallback;
            }
            None => {}
        }
        let entry = match resolve(name, bold, italic) {
            Resolved::Found(m) => self.register(ctx, name, bold, italic, m),
            Resolved::None => Entry::Fallback,
            Resolved::Wait => {
                // Inte cachat: vi frågar igen när databasen är klar.
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return fallback;
            }
        };
        self.cache.insert(key, entry);
        fallback
    }

    /// Typsnitt som installerats på systemet, för en typsnittsväljare. Med Calibri (Carlito).
    pub fn installed(&mut self) -> &[String] {
        if self.installed.is_none() && !use_fontconfig() && db().is_none() {
            self.installed_wait = vec!["Calibri".to_owned()];
            return &self.installed_wait;
        }
        self.installed.get_or_insert_with(|| {
            let mut v = list_installed();
            v.retain(|n| !is_bundled(n));
            v.push("Calibri".to_owned());
            v.sort_by_key(|s| s.to_lowercase());
            v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
            v
        })
    }

    /// Lägger filen i egui:s definitioner och en familj `doc:<namn>:<b|r><i|r>`.
    fn register(&mut self, ctx: &egui::Context, name: &str, bold: bool, italic: bool, m: Match) -> Entry {
        let data_key = (m.file.clone(), m.index);
        let data_name = match self.files.get(&data_key) {
            Some(n) => n.clone(),
            None => {
                let Ok(bytes) = std::fs::read(&m.file) else {
                    return Entry::Fallback;
                };
                let n = format!("doc-file:{}:{}", m.file.display(), m.index);
                let mut data = FontData::from_owned(bytes);
                data.index = m.index;
                self.files.insert(data_key, n.clone());
                self.defs
                    .get_or_insert_with(|| ctx.fonts(|f| f.definitions().clone()))
                    .font_data
                    .insert(n.clone(), Arc::new(data));
                n
            }
        };
        let defs = self
            .defs
            .get_or_insert_with(|| ctx.fonts(|f| f.definitions().clone()));
        let fam = FontFamily::Name(
            format!(
                "doc:{name}:{}{}",
                if bold { 'b' } else { 'r' },
                if italic { 'i' } else { 'r' }
            )
            .into(),
        );
        // Resten av kedjan (Proportional) som reserv för tecken som saknas i typsnittet.
        let mut chain = vec![data_name];
        chain.extend(
            defs.families
                .get(&FontFamily::Proportional)
                .cloned()
                .unwrap_or_default(),
        );
        defs.families.insert(fam.clone(), chain);
        ctx.set_fonts(defs.clone());
        Entry::Pending(fam)
    }

    /// En gång per frame: har egui tappat våra typsnitt (t.ex. `install_fonts` igen) läggs de
    /// tillbaka.
    fn recheck(&mut self, ctx: &egui::Context) {
        let frame = ctx.cumulative_frame_nr();
        if self.checked_frame == Some(frame) {
            return;
        }
        self.checked_frame = Some(frame);
        let Some(defs) = &self.defs else { return };
        let ready: Vec<FontFamily> = self
            .cache
            .values()
            .filter_map(|e| match e {
                Entry::Ready(f) => Some(f.clone()),
                _ => None,
            })
            .collect();
        if ready.is_empty() {
            return;
        }
        let known: HashSet<FontFamily> = ctx.fonts(|f| f.families()).into_iter().collect();
        if ready.iter().all(|f| known.contains(f)) {
            return;
        }
        for e in self.cache.values_mut() {
            if let Entry::Ready(f) = e {
                *e = Entry::Pending(f.clone());
            }
        }
        ctx.set_fonts(defs.clone());
    }
}

impl Default for DocFonts {
    fn default() -> Self {
        Self::new()
    }
}

// ---- rena hjälpfunktioner ----

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Calibri/Carlito finns inbyggt.
fn is_bundled(name: &str) -> bool {
    matches!(normalize(name).as_str(), "calibri" | "carlito")
}

/// Metriskt kompatibla ersättare som fontconfig får välja (DejaVu räknas inte).
fn is_metric_alias(family: &str) -> bool {
    let n = normalize(family);
    n.starts_with("liberation") || n == "carlito" || n == "caladea"
}

/// Godtas fontconfigs svar `got` för det begärda namnet `wanted`?
fn accepts(wanted: &str, got: &str) -> bool {
    normalize(wanted) == normalize(got) || is_metric_alias(got)
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Class {
    Serif,
    Mono,
    Sans,
}

fn classify(name: &str) -> Class {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(&w.to_lowercase()));
    // Mono först så att "Courier New" och "Lucida Console" inte fångas av serifreglerna.
    if has(&["courier", "consolas", "mono", "menlo", "lucida console"]) {
        Class::Mono
    } else if has(&[
        "times", "georgia", "garamond", "cambria", "book", "serif", "palatino", "baskerville",
        "minion", "constantia",
    ]) {
        Class::Serif
    } else {
        Class::Sans
    }
}

/// Tolkar `family\nfile\nindex` från `fc-match -f`.
fn parse_match(out: &str) -> Option<Match> {
    let mut l = out.lines();
    let family = l.next()?.trim().to_owned();
    let file = PathBuf::from(l.next()?.trim());
    let index = l.next().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) & 0xFFFF;
    if family.is_empty() || file.as_os_str().is_empty() {
        return None;
    }
    Some(Match { family, file, index })
}

fn supported_file(m: &Match) -> bool {
    m.file
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_lowercase().as_str(), "ttf" | "otf" | "ttc"))
}

/// Skyddar tecken som har betydelse i fontconfigs mönsterspråk.
fn escape_pattern(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if matches!(c, '\\' | '-' | ':' | ',' | '=' | '[' | ']') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

fn fc_match(pattern: &str) -> Option<Match> {
    let out = Command::new("fc-match")
        .arg("-f")
        .arg("%{family[0]}\n%{file}\n%{index}")
        .arg(pattern)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_match(&String::from_utf8_lossy(&out.stdout))
}

/// Resultat av en uppslagning.
enum Resolved {
    Found(Match),
    /// Ingen användbar systemfont: använd Carlito.
    None,
    /// fontdb är inte klar än: använd Carlito nu och fråga igen senare.
    Wait,
}

/// Slår upp namnet enligt reglerna i dokumentationen.
fn resolve(name: &str, bold: bool, italic: bool) -> Resolved {
    if use_fontconfig() {
        return match resolve_fc(name, bold, italic) {
            Some(m) => Resolved::Found(m),
            None => Resolved::None,
        };
    }
    match db() {
        None => Resolved::Wait,
        Some(db) => match resolve_db(db, name, bold, italic) {
            Some(m) => Resolved::Found(m),
            None => Resolved::None,
        },
    }
}

// ---- fontdb (Windows, macOS, Linux utan fontconfig) ----

/// Använd fontconfig? Bara på Linux/BSD, och bara om `fc-match` finns.
fn use_fontconfig() -> bool {
    static FC: OnceLock<bool> = OnceLock::new();
    cfg!(all(unix, not(target_os = "macos")))
        && *FC.get_or_init(|| Command::new("fc-match").arg("--version").output().is_ok())
}

/// Windows och macOS har ofta det riktiga Calibri installerat.
fn prefer_real_calibri() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

static DB: OnceLock<fontdb::Database> = OnceLock::new();

/// Typsnittsdatabasen, eller `None` medan den läses in. Första anropet startar inläsningen i
/// en bakgrundstråd (skanningen av systemmapparna kan ta en stund).
fn db() -> Option<&'static fontdb::Database> {
    static START: Once = Once::new();
    START.call_once(|| {
        let _ = std::thread::Builder::new().name("fontdb".into()).spawn(|| {
            let mut d = fontdb::Database::new();
            d.load_system_fonts();
            if let Some(n) = generic_candidates(Class::Serif).iter().find(|n| has_family(&d, n)) {
                d.set_serif_family(*n);
            }
            if let Some(n) = generic_candidates(Class::Mono).iter().find(|n| has_family(&d, n)) {
                d.set_monospace_family(*n);
            }
            if let Some(n) = generic_candidates(Class::Sans).iter().find(|n| has_family(&d, n)) {
                d.set_sans_serif_family(*n);
            }
            let _ = DB.set(d);
        });
    });
    DB.get()
}

fn has_family(db: &fontdb::Database, name: &str) -> bool {
    let n = normalize(name);
    db.faces().any(|f| f.families.iter().any(|(fam, _)| normalize(fam) == n))
}

/// Generiska familjer per plattform, i prioritetsordning. Sans på Linux ger Carlito (som förr).
fn generic_candidates(class: Class) -> &'static [&'static str] {
    if cfg!(windows) {
        match class {
            Class::Serif => &["Times New Roman"],
            Class::Mono => &["Consolas", "Courier New"],
            Class::Sans => &["Arial", "Segoe UI"],
        }
    } else if cfg!(target_os = "macos") {
        match class {
            Class::Serif => &["Times New Roman", "Times"],
            Class::Mono => &["Menlo", "Courier New"],
            Class::Sans => &["Helvetica", "Arial"],
        }
    } else {
        match class {
            Class::Serif => &["Times New Roman", "Liberation Serif", "DejaVu Serif"],
            Class::Mono => &["Courier New", "Liberation Mono", "DejaVu Sans Mono"],
            Class::Sans => &[],
        }
    }
}

/// Metriskt kompatibla ersättare om originalet saknas (som fontconfigs alias).
fn metric_alias(name: &str) -> Option<&'static str> {
    match normalize(name).as_str() {
        "arial" => Some("Liberation Sans"),
        "timesnewroman" => Some("Liberation Serif"),
        "couriernew" => Some("Liberation Mono"),
        "cambria" => Some("Caladea"),
        _ => None,
    }
}

/// Exakt familjenamn + vikt + stil -> fil och face-index. Bara ttf/otf/ttc från fil.
fn query_db(db: &fontdb::Database, family: &str, bold: bool, italic: bool) -> Option<Match> {
    if !has_family(db, family) {
        return None;
    }
    let q = fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: if bold { fontdb::Weight::BOLD } else { fontdb::Weight::NORMAL },
        stretch: fontdb::Stretch::Normal,
        style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
    };
    let face = db.face(db.query(&q)?)?;
    let file = match &face.source {
        fontdb::Source::File(p) => p.clone(),
        fontdb::Source::SharedFile(p, _) => p.clone(),
        fontdb::Source::Binary(_) => return None,
    };
    let m = Match {
        family: face.families.first().map(|f| f.0.clone()).unwrap_or_else(|| family.to_owned()),
        file,
        index: face.index,
    };
    supported_file(&m).then_some(m)
}

fn resolve_db(db: &fontdb::Database, name: &str, bold: bool, italic: bool) -> Option<Match> {
    if let Some(m) = query_db(db, name, bold, italic) {
        return Some(m);
    }
    // Det riktiga Calibri saknas: Carlito.
    if is_bundled(name) {
        return None;
    }
    if let Some(m) = metric_alias(name).and_then(|a| query_db(db, a, bold, italic)) {
        return Some(m);
    }
    generic_candidates(classify(name))
        .iter()
        .find_map(|g| query_db(db, g, bold, italic))
}

fn list_installed_db() -> Vec<String> {
    let Some(db) = db() else { return Vec::new() };
    db.faces()
        .filter_map(|f| f.families.first().map(|(n, _)| n.trim().to_owned()))
        .filter(|n| !n.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

/// Slår upp namnet via fontconfig. `None` = använd Carlito.
fn resolve_fc(name: &str, bold: bool, italic: bool) -> Option<Match> {
    let style = format!(
        ":weight={}:slant={}",
        if bold { "bold" } else { "regular" },
        if italic { "italic" } else { "roman" }
    );
    let usable = |m: &Match| supported_file(m) && !is_bundled(&m.family);
    if let Some(m) = fc_match(&format!("{}{style}", escape_pattern(name))) {
        if accepts(name, &m.family) {
            return usable(&m).then_some(m);
        }
    }
    let generic = match classify(name) {
        Class::Serif => "serif",
        Class::Mono => "monospace",
        Class::Sans => return None,
    };
    fc_match(&format!("{generic}{style}")).filter(usable)
}

fn list_installed() -> Vec<String> {
    if !use_fontconfig() {
        return list_installed_db();
    }
    let Ok(out) = Command::new("fc-list").args([":", "family"]).output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let set: HashSet<String> = text
        .lines()
        .filter_map(|l| l.split(',').next())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Color32, FontId};

    #[test]
    fn normalisering_och_alias() {
        assert_eq!(normalize("Times New Roman"), "timesnewroman");
        assert!(accepts("Times New Roman", "times new roman"));
        assert!(accepts("Arial", "Liberation Sans"));
        assert!(accepts("Courier New", "Liberation Mono"));
        assert!(accepts("Cambria", "Caladea"));
        assert!(!accepts("Georgia", "DejaVu Serif"));
        assert!(is_bundled("Calibri") && is_bundled(" carlito "));
        assert!(!is_bundled("Arial"));
    }

    #[test]
    fn klassificering() {
        assert_eq!(classify("Georgia"), Class::Serif);
        assert_eq!(classify("Book Antiqua"), Class::Serif);
        assert_eq!(classify("Courier New"), Class::Mono);
        assert_eq!(classify("Lucida Console"), Class::Mono);
        assert_eq!(classify("Verdana"), Class::Sans);
    }

    #[test]
    fn tolka_fc_match() {
        let m = parse_match("Liberation Sans\n/usr/share/fonts/LiberationSans-Regular.ttf\n0").unwrap();
        assert_eq!(m.family, "Liberation Sans");
        assert_eq!(m.index, 0);
        assert!(supported_file(&m));
        let m = parse_match("X\n/a/b.pfb\n2").unwrap();
        assert_eq!(m.index, 2);
        assert!(!supported_file(&m));
        assert!(parse_match("").is_none());
        assert_eq!(escape_pattern("A-B"), "A\\-B");
    }

    fn have_fc() -> bool {
        use_fontconfig()
    }

    /// Väntar på bakgrundsinläsningen av fontdb.
    fn wait_db() -> &'static fontdb::Database {
        for _ in 0..1200 {
            if let Some(d) = db() {
                return d;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("fontdb blev aldrig klar");
    }

    /// Kör några frames; returnerar svaret på första och sista anropet.
    fn run_family(ctx: &egui::Context, df: &mut DocFonts, name: &str) -> (FontFamily, FontFamily) {
        let mut first = None;
        let mut last = None;
        for _ in 0..4 {
            let out = ctx.run_ui(Default::default(), |_| {});
            std::mem::forget(out);
            let f = df.family(ctx, Some(name), false, false);
            first.get_or_insert(f.clone());
            last = Some(f);
        }
        (first.unwrap(), last.unwrap())
    }

    fn width(ctx: &egui::Context, fam: FontFamily) -> f32 {
        ctx.fonts_mut(|f| {
            f.layout_no_wrap("Hej världen WWW iii".into(), FontId::new(14.0, fam), Color32::BLACK)
                .size()
                .x
        })
    }

    #[test]
    fn arial_blir_systemtypsnitt() {
        if !have_fc() {
            eprintln!("fc-match saknas, hoppar över");
            return;
        }
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut df = DocFonts::new();
        let out = ctx.run_ui(Default::default(), |_| {});
        std::mem::forget(out);
        assert_eq!(df.family(&ctx, Some("Arial"), false, false), theme::doc_font(false, false));
        let (_, fam) = run_family(&ctx, &mut df, "Arial");
        let FontFamily::Name(n) = &fam else {
            eprintln!("Arial hittades inte, hoppar över");
            return;
        };
        assert!(n.starts_with("doc:"));
        assert!(df.generation() > 0);
        let w = width(&ctx, fam);
        let c = width(&ctx, theme::doc_font(false, false));
        assert!((w - c).abs() > 0.5, "{w} {c}");
        assert_eq!(df.family(&ctx, None, false, false), theme::doc_font(false, false));
        assert_eq!(df.family(&ctx, Some("Calibri"), true, false), theme::doc_font(true, false));
    }

    #[test]
    fn times_blir_serif() {
        if !have_fc() {
            eprintln!("fc-match saknas, hoppar över");
            return;
        }
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut df = DocFonts::new();
        let (_, fam) = run_family(&ctx, &mut df, "Times New Roman");
        let FontFamily::Name(_) = &fam else {
            eprintln!("ingen serif installerad, hoppar över");
            return;
        };
        let _ = width(&ctx, fam);
        let (_, f2) = run_family(&ctx, &mut df, "Georgia");
        let _ = width(&ctx, f2);
        assert!(df.installed().iter().any(|s| s == "Calibri"));
    }

    #[test]
    fn generiska_kandidater_och_alias() {
        assert!(!generic_candidates(Class::Serif).is_empty());
        assert!(!generic_candidates(Class::Mono).is_empty());
        assert_eq!(metric_alias("Times New Roman"), Some("Liberation Serif"));
        assert_eq!(metric_alias("Verdana"), None);
    }

    #[test]
    fn fontdb_hittar_familj_och_egui_kan_rita() {
        let db = wait_db();
        // Första familjen i databasen med en användbar fil.
        let Some((fam, m)) = db.faces().find_map(|f| {
            let n = f.families.first()?.0.clone();
            let m = query_db(db, &n, false, false)?;
            (!is_bundled(&m.family)).then_some((n, m))
        }) else {
            eprintln!("inga systemtypsnitt, hoppar över");
            return;
        };
        assert!(supported_file(&m));
        assert!(!std::fs::read(&m.file).unwrap().is_empty());
        // Fet/kursiv ska gå att fråga efter; okänd familj ger None.
        let _ = query_db(db, &fam, true, true);
        assert!(query_db(db, "Finns Inte Alls 12345", false, false).is_none());

        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut df = DocFonts::new();
        let out = ctx.run_ui(Default::default(), |_| {});
        std::mem::forget(out);
        let entry = df.register(&ctx, &fam, false, false, m);
        df.cache.insert((fam.to_lowercase(), false, false), entry);
        let (_, f) = run_family(&ctx, &mut df, &fam);
        assert!(matches!(f, FontFamily::Name(_)), "familjen blev aldrig aktiv");
        assert!(df.generation() > 0);
        assert!(width(&ctx, f) > 1.0);
    }

    #[test]
    fn fontdb_okand_familj_och_calibri() {
        let db = wait_db();
        if let Some(m) = resolve_db(db, "Zzz Serif Okänd", false, false) {
            assert!(supported_file(&m));
        }
        if let Some(m) = resolve_db(db, "Calibri", false, false) {
            assert!(supported_file(&m));
        }
    }
}
