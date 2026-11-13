//! Stavningskontroll: Hunspell-ordlistor via `spellbook`, ordregler och egen ordlista.
//! Ordlistorna buntas inte utan läses från systemet vid körning.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

/// En installerad ordlista.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lang {
    /// Språkkod, t.ex. "sv_SE".
    pub code: String,
    /// Visningsnamn, t.ex. "Svenska".
    pub name: String,
    pub aff: PathBuf,
    pub dic: PathBuf,
}

const NAMES: &[(&str, &str)] = &[
    ("sv_SE", "Svenska"),
    ("sv_FI", "Svenska (Finland)"),
    ("en_US", "Engelska (USA)"),
    ("en_GB", "Engelska (Storbritannien)"),
    ("de_DE", "Tyska"),
    ("fr_FR", "Franska"),
    ("es_ES", "Spanska"),
    ("nb_NO", "Norska (bokmål)"),
    ("nn_NO", "Norska (nynorsk)"),
    ("da_DK", "Danska"),
    ("fi_FI", "Finska"),
];

fn lang_name(code: &str) -> String {
    NAMES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| code.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Os {
    Linux,
    Mac,
    Windows,
}

const OS: Os = if cfg!(target_os = "macos") {
    Os::Mac
} else if cfg!(windows) {
    Os::Windows
} else {
    Os::Linux
};

/// Slår upp en miljövariabel som sökväg (tom variabel räknas som osatt).
type Env<'a> = &'a dyn Fn(&str) -> Option<PathBuf>;

fn real_env(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home(env: Env) -> Option<PathBuf> {
    env("HOME").or_else(|| env("USERPROFILE")).or_else(crate::platform::home_dir)
}

/// Mappen dit hämtade ordlistor sparas (Kladds datamapp, se `platform::data_dir`).
fn install_dir() -> PathBuf {
    crate::platform::data_dir().join("ordlistor")
}

/// Mappar som innehåller LibreOffice-tillägg (`dict-*`) med ordlistor.
fn libreoffice_ext_parents(os: Os, env: Env) -> Vec<PathBuf> {
    match os {
        Os::Linux => Vec::new(),
        Os::Mac => vec![PathBuf::from("/Applications/LibreOffice.app/Contents/Resources/extensions")],
        Os::Windows => {
            let mut v = Vec::new();
            for (var, fallback) in [("ProgramFiles", "C:\\Program Files"), ("ProgramFiles(x86)", "C:\\Program Files (x86)")] {
                let base = env(var).unwrap_or_else(|| PathBuf::from(fallback));
                v.push(base.join("LibreOffice").join("share").join("extensions"));
            }
            if let Some(l) = env("LOCALAPPDATA") {
                v.push(l.join("Programs").join("LibreOffice").join("share").join("extensions"));
            }
            v
        }
    }
}

/// `dict-*`-undermapparna i `parents`.
fn expand_dict_dirs(parents: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for parent in parents {
        let Ok(rd) = std::fs::read_dir(parent) else { continue };
        let mut subs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("dict-")))
            .collect();
        subs.sort();
        out.extend(subs);
    }
    out
}

/// Mappar där ordlistor letas, i prioritetsordning (utan LibreOffices `dict-*`-mappar).
fn dict_dirs(os: Os, env: Env, install: PathBuf) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = env("DICPATH") {
        dirs.extend(std::env::split_paths(&p).filter(|d| !d.as_os_str().is_empty()));
    }
    dirs.push(install);
    match os {
        Os::Linux => {
            for d in ["/usr/share/hunspell", "/usr/share/myspell", "/usr/share/myspell/dicts"] {
                dirs.push(PathBuf::from(d));
            }
        }
        Os::Mac => {
            dirs.push(PathBuf::from("/Library/Spelling"));
            if let Some(h) = home(env) {
                dirs.push(h.join("Library").join("Spelling"));
            }
        }
        Os::Windows => {}
    }
    dirs
}

/// Alla mappar där ordlistor letas på den här datorn.
fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = dict_dirs(OS, &real_env, install_dir());
    dirs.extend(expand_dict_dirs(&libreoffice_ext_parents(OS, &real_env)));
    dirs
}

fn default_user_dict() -> PathBuf {
    crate::platform::config_dir().join("ordlista.txt")
}

/// Hittar installerade ordlistor i `dirs`. Första träffen per kod vinner.
fn discover(dirs: &[PathBuf]) -> Vec<Lang> {
    let mut out: Vec<Lang> = Vec::new();
    let mut seen_files: HashSet<PathBuf> = HashSet::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for dic in entries {
            if dic.extension().and_then(|e| e.to_str()) != Some("dic") {
                continue;
            }
            let Some(code) = dic.file_stem().and_then(|s| s.to_str()) else { continue };
            if code.starts_with("hyph_") || code.starts_with("th_") {
                continue;
            }
            let aff = dic.with_extension("aff");
            if !aff.is_file() || !dic.is_file() {
                continue;
            }
            // Symlänkar till samma fil (sv_FI -> sv_SE) ger inget nytt språk.
            let real = std::fs::canonicalize(&dic).unwrap_or_else(|_| dic.clone());
            if out.iter().any(|l| l.code == code) || !seen_files.insert(real) {
                continue;
            }
            out.push(Lang { code: code.to_string(), name: lang_name(code), aff, dic });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.code.cmp(&b.code)));
    out
}

/// Gör om en språketikett (`sv-SE`, `sv_SE.UTF-8`, `en-US-u-hc-h12`) till Hunspell-kod (`sv_SE`).
/// `C`/`POSIX` ger `None`.
fn locale_to_code(tag: &str) -> Option<String> {
    let tag = tag.split(['.', '@']).next().unwrap_or("");
    let mut parts = tag.split(['-', '_']);
    let lang = parts.next().unwrap_or("");
    if lang.len() < 2 || !lang.chars().all(|c| c.is_ascii_alphabetic()) || lang == "POSIX" {
        return None;
    }
    let lang = lang.to_ascii_lowercase();
    match parts.next() {
        // Skript (Latn) hoppas över; region är två bokstäver eller tre siffror.
        Some(r) if r.len() == 2 && r.chars().all(|c| c.is_ascii_alphabetic()) => {
            Some(format!("{lang}_{}", r.to_ascii_uppercase()))
        }
        Some(s) if s.len() == 4 => match parts.next() {
            Some(r) if r.len() == 2 => Some(format!("{lang}_{}", r.to_ascii_uppercase())),
            _ => Some(lang),
        },
        _ => Some(lang),
    }
}

/// Språkkod från miljön: `$LANG`, annars operativsystemets språk.
pub(crate) fn system_locale_code() -> Option<String> {
    std::env::var("LANG")
        .ok()
        .and_then(|l| locale_to_code(&l))
        .or_else(|| sys_locale::get_locale().and_then(|l| locale_to_code(&l)))
}

/// Väljer standardspråk: sv_SE, annars systemspråket `locale`, annars en_US, annars första.
fn default_index_for(langs: &[Lang], locale: Option<&str>) -> Option<usize> {
    let find = |c: &str| langs.iter().position(|l| l.code == c);
    if let Some(i) = find("sv_SE") {
        return Some(i);
    }
    if let Some(code) = locale {
        if let Some(i) = find(code) {
            return Some(i);
        }
        let prefix = code.split('_').next().unwrap_or("");
        if prefix.len() >= 2 {
            if let Some(i) = langs.iter().position(|x| x.code.split('_').next() == Some(prefix)) {
                return Some(i);
            }
        }
    }
    find("en_US").or(if langs.is_empty() { None } else { Some(0) })
}

fn default_index(langs: &[Lang]) -> Option<usize> {
    default_index_for(langs, system_locale_code().as_deref())
}

/// Teckenkodning enligt `SET` i .aff (UTF-8 om den saknas eller är okänd).
fn aff_encoding(aff: &[u8]) -> &'static encoding_rs::Encoding {
    for line in aff.split(|&b| b == b'\n') {
        let line = String::from_utf8_lossy(line);
        let line = line.trim_start_matches('\u{feff}').trim();
        if let Some(rest) = line.strip_prefix("SET") {
            if rest.starts_with(char::is_whitespace) {
                let name = rest.trim();
                let lower = name.to_ascii_lowercase();
                if lower == "utf-8" || lower == "utf8" {
                    return encoding_rs::UTF_8;
                }
                if lower == "iso8859-1" || lower == "iso-8859-1" {
                    return encoding_rs::WINDOWS_1252;
                }
                let alt = lower.replacen("iso8859-", "iso-8859-", 1);
                return encoding_rs::Encoding::for_label(alt.as_bytes())
                    .or_else(|| encoding_rs::Encoding::for_label(name.as_bytes()))
                    .unwrap_or(encoding_rs::UTF_8);
            }
        }
    }
    encoding_rs::UTF_8
}

/// Läser och tolkar en ordlista. Texten avkodas till UTF-8 och `SET` skrivs om till UTF-8.
fn load_dictionary(aff: &Path, dic: &Path) -> Option<spellbook::Dictionary> {
    let aff_bytes = std::fs::read(aff).ok()?;
    let dic_bytes = std::fs::read(dic).ok()?;
    let enc = aff_encoding(&aff_bytes);
    let (aff_text, _, _) = enc.decode(&aff_bytes);
    let (dic_text, _, _) = enc.decode(&dic_bytes);
    let aff_text: String = aff_text
        .lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with("SET") && t[3..].starts_with(char::is_whitespace) {
                "SET UTF-8"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    spellbook::Dictionary::new(&aff_text, &dic_text).ok()
}

/// En ordlista som kan hämtas från LibreOffices ordlistearkiv.
pub struct Download {
    /// Språkkod den sparas som, t.ex. "sv_SE".
    pub code: &'static str,
    /// Visningsnamn i hämtningslistan.
    pub name: &'static str,
    /// Sökvägar i arkivet (efter `DICT_BASE`).
    pub aff: &'static str,
    pub dic: &'static str,
    /// README/licensfil att hämta bredvid, om arkivet har någon.
    pub license: Option<&'static str>,
}

pub const DICT_BASE: &str = "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/";

/// Licensanmärkning som visas vid hämtning.
pub const LICENSE_NOTE: &str = "Ordlistan kommer från LibreOffice och har egen licens (LGPL/MPL …)";

pub const DOWNLOADS: &[Download] = &[
    Download {
        code: "sv_SE",
        name: "Svenska",
        aff: "sv_SE/dictionaries/sv_SE.aff",
        dic: "sv_SE/dictionaries/sv_SE.dic",
        license: Some("sv_SE/LICENSE_en_US.txt"),
    },
    Download {
        code: "en_US",
        name: "Engelska (USA)",
        aff: "en/en_US.aff",
        dic: "en/en_US.dic",
        license: Some("en/README_en_US.txt"),
    },
    Download {
        code: "en_GB",
        name: "Engelska (Storbritannien)",
        aff: "en/en_GB.aff",
        dic: "en/en_GB.dic",
        license: Some("en/README_en_GB.txt"),
    },
    Download {
        code: "nb_NO",
        name: "Norska",
        aff: "no/nb_NO.aff",
        dic: "no/nb_NO.dic",
        license: Some("no/README_NO.txt"),
    },
    Download {
        code: "da_DK",
        name: "Danska",
        aff: "da_DK/da_DK.aff",
        dic: "da_DK/da_DK.dic",
        license: Some("da_DK/README_da_DK.txt"),
    },
    Download {
        code: "de_DE",
        name: "Tyska",
        aff: "de/de_DE_frami.aff",
        dic: "de/de_DE_frami.dic",
        license: Some("de/README_de_DE_frami.txt"),
    },
    Download {
        code: "fr_FR",
        name: "Franska",
        aff: "fr_FR/dictionaries/fr.aff",
        dic: "fr_FR/dictionaries/fr.dic",
        license: Some("fr_FR/dictionaries/README_dict_fr.txt"),
    },
    Download { code: "es_ES", name: "Spanska", aff: "es/es_ES.aff", dic: "es/es_ES.dic", license: None },
];

fn http_get(agent: &ureq::Agent, path: &str) -> Result<Vec<u8>, String> {
    let url = format!("{DICT_BASE}{path}");
    let mut resp = agent.get(&url).call().map_err(|e| format!("{e}"))?;
    resp.body_mut().with_config().limit(64 * 1024 * 1024).read_to_vec().map_err(|e| format!("{e}"))
}

/// Hämtar `d` till `dir` som `<kod>.aff`/`<kod>.dic` (+ `<kod>.LICENSE.txt`). Filerna skrivs
/// först under tillfälligt namn så att en avbruten hämtning inte lämnar halva ordlistor.
fn download_to(d: &Download, dir: &Path) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .build()
        .into();
    std::fs::create_dir_all(dir).map_err(|e| format!("kunde inte skapa {}: {e}", dir.display()))?;
    let aff = http_get(&agent, d.aff)?;
    let dic = http_get(&agent, d.dic)?;
    let save = |name: String, bytes: &[u8]| -> Result<(), String> {
        let tmp = dir.join(format!("{name}.part"));
        std::fs::write(&tmp, bytes).map_err(|e| format!("kunde inte spara {name}: {e}"))?;
        std::fs::rename(&tmp, dir.join(&name)).map_err(|e| format!("kunde inte spara {name}: {e}"))
    };
    // Licensen är bara en bonus: misslyckas den får ordlistan ändå användas.
    if let Some(lic) = d.license {
        if let Ok(b) = http_get(&agent, lic) {
            let _ = save(format!("{}.LICENSE.txt", d.code), &b);
        }
    }
    save(format!("{}.aff", d.code), &aff)?;
    save(format!("{}.dic", d.code), &dic)
}

/// Hämtar ordlistan `code` till `dir` (blockerar; kör i egen tråd).
pub fn download_dictionary(code: &str, dir: &Path) -> Result<(), String> {
    let d = DOWNLOADS.iter().find(|d| d.code == code).ok_or_else(|| format!("okänt språk: {code}"))?;
    download_to(d, dir)
}

pub struct Speller {
    langs: Vec<Lang>,
    cur: Option<usize>,
    dict: Option<spellbook::Dictionary>,
    rx: Option<Receiver<Option<spellbook::Dictionary>>>,
    cache: HashMap<String, bool>,
    ignored: HashSet<String>,
    user: HashSet<String>,
    user_path: PathBuf,
    generation: u64,
    dirs: Vec<PathBuf>,
    install_dir: PathBuf,
    /// Pågående hämtning: (språkkod, visningsnamn, kanal med resultatet).
    download: Option<(String, String, Receiver<Result<(), String>>)>,
}

impl Speller {
    pub fn new(lang: Option<&str>) -> Self {
        let mut s = Self::with_paths(default_dirs(), default_user_dict(), lang);
        s.install_dir = install_dir();
        s
    }

    pub fn with_paths(dirs: Vec<PathBuf>, user_dict: PathBuf, lang: Option<&str>) -> Self {
        let langs = discover(&dirs);
        let cur = lang
            .and_then(|c| langs.iter().position(|l| l.code == c))
            .or_else(|| default_index(&langs));
        let user = std::fs::read_to_string(&user_dict)
            .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
            .unwrap_or_default();
        let mut s = Self {
            langs,
            cur,
            dict: None,
            rx: None,
            cache: HashMap::new(),
            ignored: HashSet::new(),
            user,
            user_path: user_dict,
            generation: 0,
            install_dir: dirs.first().cloned().unwrap_or_default(),
            dirs,
            download: None,
        };
        s.start_loading();
        s
    }

    fn start_loading(&mut self) {
        self.dict = None;
        self.rx = None;
        let Some(l) = self.cur.map(|i| &self.langs[i]) else { return };
        let (aff, dic) = (l.aff.clone(), l.dic.clone());
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(load_dictionary(&aff, &dic));
        });
    }

    fn bump(&mut self) {
        self.generation += 1;
        self.cache.clear();
    }

    /// Hämtar ordlistan `code` i bakgrunden till Kladds ordlistemapp.
    pub fn start_download(&mut self, code: &str) {
        if self.download.is_some() {
            return;
        }
        let Some(d) = DOWNLOADS.iter().find(|d| d.code == code) else { return };
        let (tx, rx) = mpsc::channel();
        let dir = self.install_dir.clone();
        std::thread::spawn(move || {
            let _ = tx.send(download_to(d, &dir));
        });
        self.download = Some((d.code.to_string(), d.name.to_string(), rx));
    }

    /// Visningsnamnet på ordlistan som hämtas just nu.
    pub fn downloading(&self) -> Option<&str> {
        self.download.as_ref().map(|d| d.1.as_str())
    }

    /// Är hämtningen klar? Då söks ordlistorna om och den nya väljs; `Ok` ger visningsnamnet,
    /// `Err` ett felmeddelande.
    pub fn poll_download(&mut self) -> Option<Result<String, String>> {
        let (code, name, rx) = self.download.as_ref()?;
        let res = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("hämtningen avbröts".to_owned()),
        };
        let (code, name) = (code.clone(), name.clone());
        self.download = None;
        Some(res.and_then(|()| {
            self.rescan();
            let i = self
                .langs
                .iter()
                .position(|l| l.code == code)
                .ok_or("ordlistan hittades inte efter hämtningen")?;
            self.cur = Some(i);
            self.start_loading();
            self.bump();
            Ok(name)
        }))
    }

    /// Söker igenom ordlistemapparna på nytt och behåller valt språk om det finns kvar.
    pub fn rescan(&mut self) {
        let keep = self.lang().map(|l| l.code.clone());
        self.langs = discover(&self.dirs);
        self.cur =
            keep.and_then(|c| self.langs.iter().position(|l| l.code == c)).or_else(|| default_index(&self.langs));
    }

    pub fn languages(&self) -> &[Lang] {
        &self.langs
    }

    pub fn lang(&self) -> Option<&Lang> {
        self.cur.map(|i| &self.langs[i])
    }

    pub fn set_lang(&mut self, code: &str) {
        let Some(i) = self.langs.iter().position(|l| l.code == code) else { return };
        if self.cur == Some(i) {
            return;
        }
        self.cur = Some(i);
        self.start_loading();
        self.bump();
    }

    /// Sant när ordlistan är laddad. `generation` ökar när den blir klar så att
    /// markeringar räknas om.
    pub fn ready(&mut self) -> bool {
        if self.dict.is_some() {
            return true;
        }
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(d) => {
                    self.rx = None;
                    self.dict = d;
                    self.bump();
                    return self.dict.is_some();
                }
                Err(mpsc::TryRecvError::Disconnected) => self.rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        false
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn known(&self, word: &str) -> bool {
        let set_has = |s: &HashSet<String>| {
            s.contains(word) || {
                let lower = word.to_lowercase();
                lower != word && s.contains(&lower)
            }
        };
        set_has(&self.user) || set_has(&self.ignored)
    }

    fn check_uncached(&self, word: &str) -> bool {
        if self.known(word) {
            return true;
        }
        let Some(dict) = &self.dict else { return true };
        if dict.check(word) {
            return true;
        }
        if word.contains('-') {
            return word
                .split('-')
                .filter(|p| !p.is_empty())
                .all(|p| p.chars().count() < 2 || self.known(p) || dict.check(p));
        }
        false
    }

    /// Sant om ordet är rättstavat (eller om ordlistan ännu inte är laddad).
    pub fn check_word(&mut self, word: &str) -> bool {
        if self.dict.is_none() && !self.ready() {
            return true;
        }
        let word = word.replace('\u{2019}', "'");
        if let Some(&r) = self.cache.get(&word) {
            return r;
        }
        let r = self.check_uncached(&word);
        self.cache.insert(word, r);
        r
    }

    pub fn misspelled(&mut self, text: &str, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        for r in ranges {
            for w in words(text, r.clone()) {
                if !self.check_word(&text[w.clone()]) {
                    out.push(w);
                }
            }
        }
        out
    }

    /// Som `misspelled`, men med en cache per stycke (hash av styckets text -> felstavade ord
    /// relativt styckets början): oförändrade stycken kontrolleras inte om. `cache` ersätts med
    /// de stycken som användes, så att den inte växer.
    pub fn misspelled_cached(
        &mut self,
        text: &str,
        ranges: &[Range<usize>],
        cache: &mut HashMap<u64, Vec<Range<usize>>>,
    ) -> Vec<Range<usize>> {
        let mut fresh: HashMap<u64, Vec<Range<usize>>> = HashMap::with_capacity(ranges.len());
        let mut found = Vec::new();
        for r in ranges {
            let key = crate::word::layout::fast_hash(&text[r.clone()]);
            let rel = match fresh.get(&key).or_else(|| cache.get(&key)) {
                Some(v) => v.clone(),
                None => self
                    .misspelled(text, std::slice::from_ref(r))
                    .into_iter()
                    .map(|w| w.start - r.start..w.end - r.start)
                    .collect(),
            };
            found.extend(rel.iter().map(|w| w.start + r.start..w.end + r.start));
            fresh.insert(key, rel);
        }
        *cache = fresh;
        found
    }

    pub fn suggest(&self, word: &str) -> Vec<String> {
        let Some(dict) = &self.dict else { return Vec::new() };
        let word = word.replace('\u{2019}', "'");
        let mut out = Vec::new();
        dict.suggest(&word, &mut out);
        out.truncate(6);
        out
    }

    /// Ignorerar ordet resten av sessionen.
    pub fn ignore_all(&mut self, word: &str) {
        self.ignored.insert(word.replace('\u{2019}', "'"));
        self.bump();
    }

    /// Lägger ordet i den egna ordlistan (sparas på disk).
    pub fn add_to_dictionary(&mut self, word: &str) {
        let word = word.replace('\u{2019}', "'");
        if word.is_empty() || word.contains('\n') {
            return;
        }
        if self.user.insert(word.clone()) {
            if let Some(dir) = self.user_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.user_path) {
                let _ = writeln!(f, "{word}");
            }
        }
        self.bump();
    }
}

fn is_apos(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

/// Byte-intervall för ord att kontrollera inom `range`.
pub fn words(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let mut end = range.end.min(text.len());
    let mut start = range.start.min(end);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let chars: Vec<(usize, char)> = text[start..end].char_indices().map(|(i, c)| (i + start, c)).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].1.is_alphanumeric() {
            i += 1;
            continue;
        }
        let first = i;
        let mut j = i;
        while j < chars.len() {
            let c = chars[j].1;
            if c.is_alphanumeric() {
                j += 1;
            } else if (is_apos(c) || c == '-') && j + 1 < chars.len() && chars[j + 1].1.is_alphanumeric() {
                j += 2;
            } else {
                break;
            }
        }
        i = j;
        let s = chars[first].0;
        let e = chars.get(j).map_or(end, |c| c.0);
        let tok = &text[s..e];
        if tok.chars().count() < 2
            || tok.chars().any(char::is_numeric)
            || !tok.chars().any(char::is_lowercase)
        {
            continue;
        }
        let before: Vec<char> = text[..s].chars().rev().take(2).collect();
        let after: Vec<char> = text[e..].chars().take(2).collect();
        let b1 = before.first().copied();
        let a1 = after.first().copied();
        let glued = |c: Option<char>, other: Option<char>| match c {
            Some('/' | '@' | '\\' | '_') => true,
            // "Obs: text" är en vanlig mening; "kl:12" och "http://" är det inte.
            Some(':') => other.is_some_and(|o| !o.is_whitespace()),
            _ => false,
        };
        if glued(b1, before.get(1).copied()) || glued(a1, after.get(1).copied()) {
            continue;
        }
        // Filnamn och domäner: punkt med bokstav på andra sidan.
        if a1 == Some('.') && after.get(1).is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        if b1 == Some('.') && before.get(1).is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        out.push(s..e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(text: &str) -> Vec<&str> {
        words(text, 0..text.len()).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn svenska_ord() {
        assert_eq!(toks("Räksmörgåsen är åäö, ÖL."), vec!["Räksmörgåsen", "är", "åäö"]);
    }

    #[test]
    fn apostrof_och_bindestreck() {
        assert_eq!(toks("don't och it’s e-post 'citat' x- -y"), vec!["don't", "och", "it’s", "e-post", "citat"]);
    }

    #[test]
    fn adresser_hoppas_over() {
        assert_eq!(toks("se https://example.com/sida nu"), vec!["se", "nu"]);
        assert_eq!(toks("mejla namn@exempel.se idag"), vec!["mejla", "idag"]);
        assert_eq!(toks("fil.txt och C:\\mapp\\fil snake_case"), vec!["och"]);
        assert_eq!(toks("Obs: ord. Nästa"), vec!["Obs", "ord", "Nästa"]);
    }

    #[test]
    fn siffror_versaler_korta() {
        assert_eq!(toks("abc123 NATO a ok 2024 iPhone"), vec!["ok", "iPhone"]);
    }

    #[test]
    fn teckengranser() {
        let t = "åä öl";
        // Start mitt i ett tecken ska inte panikera.
        let r = words(t, 1..t.len());
        assert!(r.iter().all(|r| t.is_char_boundary(r.start) && t.is_char_boundary(r.end)));
        assert_eq!(words(t, 0..100).len(), 2);
        assert_eq!(words(t, 0..3), Vec::<Range<usize>>::new()); // klipps till "å"
        assert_eq!(words(t, 0..4), vec![0..4]);
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kladd-spell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_dict(dir: &Path, code: &str, words: &[&str]) {
        std::fs::write(dir.join(format!("{code}.aff")), "SET UTF-8\n").unwrap();
        std::fs::write(dir.join(format!("{code}.dic")), format!("{}\n{}\n", words.len(), words.join("\n"))).unwrap();
    }

    fn wait(s: &mut Speller) {
        let t = std::time::Instant::now();
        while !s.ready() {
            assert!(t.elapsed().as_secs() < 5, "ordlistan laddades inte");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn upptackt() {
        let d = tmp("upptackt");
        make_dict(&d, "xx_XX", &["hej"]);
        make_dict(&d, "en_GB", &["hello"]);
        make_dict(&d, "hyph_xx_XX", &["a"]);
        make_dict(&d, "th_xx_XX", &["a"]);
        std::fs::write(d.join("lonely.dic"), "0\n").unwrap();
        let d2 = tmp("upptackt2");
        make_dict(&d2, "xx_XX", &["annat"]);
        let s = Speller::with_paths(vec![d.clone(), d2], d.join("u.txt"), None);
        let codes: Vec<_> = s.languages().iter().map(|l| (l.code.as_str(), l.name.as_str())).collect();
        assert_eq!(codes, vec![("en_GB", "Engelska (Storbritannien)"), ("xx_XX", "xx_XX")]);
        assert_eq!(s.languages()[1].dic, d.join("xx_XX.dic"));
        assert!(s.lang().is_some());
        let s = Speller::with_paths(vec![d.clone()], d.join("u.txt"), Some("xx_XX"));
        assert_eq!(s.lang().unwrap().code, "xx_XX");
        let s = Speller::with_paths(vec![d.clone()], d.join("u.txt"), Some("finns_ej"));
        assert!(s.lang().is_some());
    }

    #[test]
    fn symlankar_dedupas() {
        let d = tmp("symlank");
        make_dict(&d, "aa_AA", &["hej"]);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("aa_AA.dic"), d.join("bb_BB.dic")).unwrap();
            std::os::unix::fs::symlink(d.join("aa_AA.aff"), d.join("bb_BB.aff")).unwrap();
            let s = Speller::with_paths(vec![d.clone()], d.join("u.txt"), None);
            assert_eq!(s.languages().len(), 1);
        }
    }

    #[test]
    fn kontroll_ignorera_lagg_till() {
        let d = tmp("kontroll");
        make_dict(&d, "xx_XX", &["hej", "värld"]);
        let user = d.join("conf").join("ordlista.txt");
        let mut s = Speller::with_paths(vec![d.clone()], user.clone(), None);
        wait(&mut s);
        assert!(s.check_word("hej"));
        assert!(!s.check_word("blorp"));
        assert!(s.check_word("hej-värld"));
        assert!(!s.check_word("hej-blorp"));
        let g = s.generation();
        s.ignore_all("blorp");
        assert!(s.generation() > g);
        assert!(s.check_word("blorp"));
        let g = s.generation();
        s.add_to_dictionary("Zork");
        assert!(s.generation() > g);
        assert!(s.check_word("Zork"));
        assert_eq!(std::fs::read_to_string(&user).unwrap(), "Zork\n");
        // Ny instans läser den egna ordlistan; versal start godtas via gemen post.
        let mut s2 = Speller::with_paths(vec![d.clone()], user, None);
        wait(&mut s2);
        assert!(s2.check_word("Zork"));
        s2.add_to_dictionary("kväll");
        assert!(s2.check_word("Kväll"));
        let r = s2.misspelled("hej blorp värld", &[0..16]);
        assert_eq!(r, vec![4..9]);
        assert!(s2.suggest("hjej").len() <= 6);
    }

    #[test]
    fn laddar_gammal_teckenkodning() {
        let d = tmp("latin1");
        std::fs::write(d.join("yy_YY.aff"), "SET ISO8859-1\n").unwrap();
        let mut dic = b"1\nv".to_vec();
        dic.push(0xE4); // ä
        dic.extend_from_slice(b"rld\n");
        std::fs::write(d.join("yy_YY.dic"), dic).unwrap();
        let mut s = Speller::with_paths(vec![d.clone()], d.join("u.txt"), None);
        wait(&mut s);
        assert!(s.check_word("värld"));
        assert!(!s.check_word("vorld"));
    }

    #[test]
    fn byte_språk_startar_om_laddning() {
        let d = tmp("byt");
        make_dict(&d, "aa_AA", &["hej"]);
        make_dict(&d, "bb_BB", &["tja"]);
        let mut s = Speller::with_paths(vec![d.clone()], d.join("u.txt"), Some("aa_AA"));
        wait(&mut s);
        assert!(!s.check_word("tja"));
        let g = s.generation();
        s.set_lang("bb_BB");
        assert!(s.generation() > g);
        assert!(s.check_word("blorp")); // laddar: inga våglinjer
        wait(&mut s);
        assert!(s.check_word("tja"));
        assert!(!s.check_word("hej"));
    }

    #[test]
    fn svensk_ordlista() {
        let p = Path::new("/usr/share/hunspell/sv_SE.dic");
        if !p.exists() {
            return;
        }
        let d = tmp("sv");
        let mut s = Speller::with_paths(vec![PathBuf::from("/usr/share/hunspell")], d.join("u.txt"), Some("sv_SE"));
        wait(&mut s);
        assert_eq!(s.lang().unwrap().name, "Svenska");
        assert!(s.check_word("sjukhusparkeringen"));
        assert!(!s.check_word("dokumnet"));
        assert!(s.suggest("dokumnet").iter().any(|w| w == "dokument"));
    }

    fn fake_env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<PathBuf> + 'a {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| PathBuf::from(v))
    }

    #[test]
    fn mappar_per_os() {
        let inst = PathBuf::from("/data/Kladd/ordlistor");
        // Linux.
        let env = fake_env(&[("HOME", "/home/a")]);
        let d = dict_dirs(Os::Linux, &env, inst.clone());
        assert_eq!(d[0], inst);
        assert!(d.contains(&PathBuf::from("/usr/share/hunspell")));
        let env = fake_env(&[("HOME", "/h"), ("DICPATH", "/dic")]);
        assert_eq!(dict_dirs(Os::Linux, &env, inst.clone())[..2], [PathBuf::from("/dic"), inst.clone()]);

        // macOS.
        let env = fake_env(&[("HOME", "/Users/a")]);
        let d = dict_dirs(Os::Mac, &env, inst.clone());
        assert_eq!(d[0], inst);
        assert!(d.contains(&PathBuf::from("/Library/Spelling")));
        assert!(d.contains(&PathBuf::from("/Users/a/Library/Spelling")));
        assert_eq!(
            libreoffice_ext_parents(Os::Mac, &env),
            vec![PathBuf::from("/Applications/LibreOffice.app/Contents/Resources/extensions")]
        );

        // Windows.
        let env = fake_env(&[("LOCALAPPDATA", "C:\\Users\\a\\AppData\\Local"), ("ProgramFiles", "C:\\Program Files")]);
        assert_eq!(dict_dirs(Os::Windows, &env, inst.clone()), vec![inst]);
        let lo = libreoffice_ext_parents(Os::Windows, &env);
        assert_eq!(lo.len(), 3);
        assert!(lo[0].starts_with("C:\\Program Files") && lo[0].ends_with("extensions"));
        assert!(lo[1].to_string_lossy().contains("Program Files (x86)"));
        assert!(lo[2].to_string_lossy().contains("Programs"));
    }

    #[test]
    fn libreoffice_tillagg_hittas() {
        let root = tmp("lo");
        let ext = root.join("extensions");
        for (sub, code) in [("dict-sv", "sv_SE"), ("dict-en", "en_US")] {
            std::fs::create_dir_all(ext.join(sub)).unwrap();
            make_dict(&ext.join(sub), code, &["hej"]);
        }
        std::fs::create_dir_all(ext.join("other")).unwrap();
        make_dict(&ext.join("other"), "xx_XX", &["a"]);
        let dirs = expand_dict_dirs(&[ext.clone(), root.join("saknas")]);
        assert_eq!(dirs, vec![ext.join("dict-en"), ext.join("dict-sv")]);
        let codes: Vec<_> = discover(&dirs).into_iter().map(|l| l.code).collect();
        assert_eq!(codes, vec!["en_US", "sv_SE"]); // sorterat på namn: Engelska, Svenska
    }

    #[test]
    fn sprakkod_fran_locale() {
        assert_eq!(locale_to_code("sv-SE").as_deref(), Some("sv_SE"));
        assert_eq!(locale_to_code("en-US").as_deref(), Some("en_US"));
        assert_eq!(locale_to_code("sv_SE.UTF-8").as_deref(), Some("sv_SE"));
        assert_eq!(locale_to_code("de_DE@euro").as_deref(), Some("de_DE"));
        assert_eq!(locale_to_code("en-US-u-hc-h12").as_deref(), Some("en_US"));
        assert_eq!(locale_to_code("zh-Hans-CN").as_deref(), Some("zh_CN"));
        assert_eq!(locale_to_code("nb").as_deref(), Some("nb"));
        assert_eq!(locale_to_code("C"), None);
        assert_eq!(locale_to_code("POSIX"), None);
        assert_eq!(locale_to_code(""), None);
    }

    #[test]
    fn standardsprak() {
        let mk = |c: &str| Lang { code: c.into(), name: c.into(), aff: PathBuf::new(), dic: PathBuf::new() };
        let langs = vec![mk("da_DK"), mk("en_US"), mk("sv_SE")];
        assert_eq!(default_index_for(&langs, Some("da_DK")), Some(2)); // svenska först
        let langs = vec![mk("da_DK"), mk("en_GB"), mk("en_US"), mk("nb_NO")];
        assert_eq!(default_index_for(&langs, Some("nb_NO")), Some(3));
        assert_eq!(default_index_for(&langs, Some("en_GB")), Some(1));
        assert_eq!(default_index_for(&langs, Some("da")), Some(0)); // bara språkdelen
        assert_eq!(default_index_for(&langs, Some("ja_JP")), Some(2)); // en_US
        assert_eq!(default_index_for(&langs, None), Some(2));
        assert_eq!(default_index_for(&langs[..1], Some("ja_JP")), Some(0));
        assert_eq!(default_index_for(&[], Some("sv_SE")), None);
    }

    #[test]
    fn hamtningslistan_ar_hel() {
        let mut codes = HashSet::new();
        for d in DOWNLOADS {
            assert!(codes.insert(d.code), "dubblett: {}", d.code);
            assert!(d.aff.ends_with(".aff") && d.dic.ends_with(".dic"), "{}", d.code);
            // aff och dic ligger i samma mapp och har samma namn.
            assert_eq!(d.aff.trim_end_matches(".aff"), d.dic.trim_end_matches(".dic"), "{}", d.code);
            assert!(!d.name.is_empty());
            for path in [Some(d.aff), Some(d.dic), d.license].into_iter().flatten() {
                assert!(!path.starts_with('/') && !path.contains(".."), "{path}");
            }
        }
        for c in ["sv_SE", "en_US", "en_GB", "nb_NO", "da_DK", "de_DE", "fr_FR", "es_ES"] {
            assert!(codes.contains(c), "saknar {c}");
        }
        assert!(DICT_BASE.starts_with("https://") && DICT_BASE.ends_with('/'));
        // Hämtade filer måste kunna hittas av `discover` under koden.
        assert!(download_dictionary("finns_ej", Path::new("x")).is_err());
    }

    #[test]
    fn rescan_hittar_nyinstallerad() {
        let d = tmp("rescan");
        make_dict(&d, "aa_AA", &["hej"]);
        let mut s = Speller::with_paths(vec![d.clone()], d.join("u.txt"), None);
        assert_eq!(s.languages().len(), 1);
        make_dict(&d, "bb_BB", &["tja"]);
        s.rescan();
        assert_eq!(s.languages().len(), 2);
        assert_eq!(s.lang().unwrap().code, "aa_AA"); // valt språk behålls
    }

    /// Hämtar svenska från LibreOffices arkiv. Kräver nät: `cargo test -- --ignored hamta_svensk`.
    #[test]
    #[ignore]
    fn hamta_svensk_ordlista() {
        let d = tmp("hamta");
        download_dictionary("sv_SE", &d).unwrap();
        assert!(d.join("sv_SE.aff").is_file() && d.join("sv_SE.dic").is_file());
        assert!(d.join("sv_SE.LICENSE.txt").is_file());
        let mut s = Speller::with_paths(vec![d.clone()], d.join("u.txt"), None);
        wait(&mut s);
        assert_eq!(s.lang().unwrap().code, "sv_SE");
        assert!(s.check_word("sjukhusparkeringen"));
        assert!(!s.check_word("dokumnet"));
    }
}
