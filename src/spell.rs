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

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Mappar där ordlistor letas, i prioritetsordning.
fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = std::env::var_os("DICPATH") {
        dirs.extend(std::env::split_paths(&p).filter(|d| !d.as_os_str().is_empty()));
    }
    if let Some(d) = env_dir("XDG_DATA_HOME").or_else(|| home().map(|h| h.join(".local/share"))) {
        dirs.push(d.join("kladd").join("ordlistor"));
    }
    for d in ["/usr/share/hunspell", "/usr/share/myspell", "/usr/share/myspell/dicts"] {
        dirs.push(PathBuf::from(d));
    }
    dirs
}

fn default_user_dict() -> PathBuf {
    env_dir("XDG_CONFIG_HOME")
        .or_else(|| home().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("kladd")
        .join("ordlista.txt")
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

/// Väljer standardspråk: sv_SE, annars `$LANG`, annars en_US, annars första.
fn default_index(langs: &[Lang]) -> Option<usize> {
    let find = |c: &str| langs.iter().position(|l| l.code == c);
    if let Some(i) = find("sv_SE") {
        return Some(i);
    }
    if let Ok(l) = std::env::var("LANG") {
        let code = l.split(['.', '@']).next().unwrap_or("");
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
}

impl Speller {
    pub fn new(lang: Option<&str>) -> Self {
        Self::with_paths(default_dirs(), default_user_dict(), lang)
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
}
