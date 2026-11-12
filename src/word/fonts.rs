//! Systemtypsnitt för dokumentläget via fontconfig (`fc-match`), så att t.ex. Arial blir
//! Liberation Sans och Times New Roman Liberation Serif på Ubuntu, precis som i LibreOffice.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

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
        if is_bundled(name) {
            return fallback;
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
            Some(m) => self.register(ctx, name, bold, italic, m),
            None => Entry::Fallback,
        };
        self.cache.insert(key, entry);
        fallback
    }

    /// Typsnitt som installerats på systemet, för en typsnittsväljare. Med Calibri (Carlito).
    pub fn installed(&mut self) -> &[String] {
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

/// Slår upp namnet enligt reglerna i dokumentationen. `None` = använd Carlito.
fn resolve(name: &str, bold: bool, italic: bool) -> Option<Match> {
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
        Command::new("fc-match").arg("--version").output().is_ok()
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
}
