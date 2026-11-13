//! Grammatik- och stilkontroll i dokumentläget med LanguageTool (https://languagetool.org/http-api/).
//!
//! Av som standard. Bara synlig text skickas: Markdown-syntaxen (`**`, `# `, länkadresser, kod,
//! tabeller …) skickas som `markup` i LanguageTools `data`-format, så den ignoreras men
//! träffarnas lägen gäller ändå originaltexten. Kontrollen sker stycke för stycke: bara stycken
//! som ändrats skickas, efter en paus (`DEBOUNCE_SECS`), i bakgrundstrådar och inom
//! den publika serverns gränser (20 anrop/min, 20 kB/anrop). Allt här är fritt från egui.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const PUBLIC_URL: &str = "https://api.languagetool.org";
pub const PUBLIC_LABEL: &str = "LanguageTools server (api.languagetool.org)";
/// Värde i inställningen `grammar_server` för den publika servern.
pub const PUBLIC_SETTING: &str = "public";
/// Förklaringen som visas innan den publika servern används första gången.
pub const PRIVACY_NOTE: &str = "Grammatikkontrollen skickar den synliga texten i dokumentet till LanguageTools server (api.languagetool.org) över internet. Skriv inget som inte får lämna datorn, eller använd en egen server (till exempel http://localhost:8081).";
/// Väntan efter senaste tangenttryckningen innan något skickas.
pub const DEBOUNCE_SECS: f64 = 1.5;
/// Publika serverns gräns är 20 anrop per minut; vi håller oss strax under.
const PUBLIC_REQ_PER_MIN: usize = 18;
/// Publika serverns gräns är 20 kB per anrop; vi håller oss under (JSON-storlek).
const PUBLIC_MAX_BYTES: usize = 18_000;
const CUSTOM_MAX_BYTES: usize = 100_000;
/// Längsta stycke (i bytes) som skickas i ett stycke; längre rader delas vid meningsslut.
const MAX_PARA: usize = 6_000;
/// Väntan efter ett misslyckat anrop innan nästa försök.
const BACKOFF_SECS: f64 = 30.0;
const MAX_REPLACEMENTS: usize = 5;

// --- Server och språk ----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Server {
    Public,
    Custom(String),
}

impl Server {
    /// Tolkar inställningen: "" och "public" är den publika servern, annars en adress.
    pub fn from_setting(s: &str) -> Server {
        let s = s.trim();
        if s.is_empty() || s == PUBLIC_SETTING { Server::Public } else { Server::Custom(s.to_owned()) }
    }

    pub fn is_public(&self) -> bool {
        matches!(self, Server::Public)
    }

    /// Adressen till `/v2/check`. En adress utan schema får `http://`.
    pub fn endpoint(&self) -> String {
        let base = match self {
            Server::Public => PUBLIC_URL.to_owned(),
            Server::Custom(u) => {
                let u = u.trim().trim_end_matches('/');
                if u.contains("://") { u.to_owned() } else { format!("http://{u}") }
            }
        };
        if base.ends_with("/v2/check") { base } else { format!("{base}/v2/check") }
    }

    fn max_bytes(&self) -> usize {
        if self.is_public() { PUBLIC_MAX_BYTES } else { CUSTOM_MAX_BYTES }
    }
}

/// LanguageTool-språk för en stavningsordlista (`sv_SE` -> `sv`, `en_GB` -> `en-GB`); `auto` när
/// språket är okänt.
pub fn language_for(spell_code: Option<&str>) -> String {
    let Some(code) = spell_code else { return "auto".to_owned() };
    let norm = code.replace('_', "-");
    let (lang, region) = match norm.split_once('-') {
        Some((l, r)) => (l.to_ascii_lowercase(), r.to_ascii_uppercase()),
        None => (norm.to_ascii_lowercase(), String::new()),
    };
    match lang.as_str() {
        "" => "auto".to_owned(),
        "en" | "de" | "pt" => {
            let default = match lang.as_str() {
                "en" => "US",
                "de" => "DE",
                _ => "PT",
            };
            format!("{lang}-{}", if region.is_empty() { default } else { &region })
        }
        "nb" | "no" => "nb".to_owned(),
        _ => lang,
    }
}

// --- Stycken och textmappning ------------------------------------------------------------

/// En del av ett stycke, som byteintervall i dokumentets text.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Text(Range<usize>),
    Markup(Range<usize>),
}

/// En rad (eller del av en lång rad) som kontrolleras för sig.
#[derive(Clone, Debug)]
pub struct Para {
    /// Hela styckets byteintervall i dokumentet.
    pub range: Range<usize>,
    pub items: Vec<Item>,
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Byteläget i `s` som motsvarar UTF-16-läget `u` (LanguageTool räknar som Java). `None` om `u`
/// ligger mitt i ett surrogatpar eller utanför.
fn utf16_to_byte(s: &str, u: usize) -> Option<usize> {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        if n == u {
            return Some(i);
        }
        if n > u {
            return None;
        }
        n += c.len_utf16();
    }
    (n == u).then_some(s.len())
}

/// Delar en rad som är längre än `max` bytes vid meningsslut, annars vid mellanslag, annars vid
/// teckengräns.
fn split_long(text: &str, range: Range<usize>, max: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = range.start;
    while range.end - start > max {
        let mut hard = start + max;
        while !text.is_char_boundary(hard) {
            hard -= 1;
        }
        let window = &text[start..hard];
        let cut = [". ", "! ", "? "]
            .iter()
            .filter_map(|p| window.rfind(p).map(|i| i + 2))
            .max()
            .or_else(|| window.rfind(' ').map(|i| i + 1))
            .filter(|&c| c > 0)
            .map_or(hard, |c| start + c);
        out.push(start..cut);
        start = cut;
    }
    out.push(start..range.end);
    out
}

/// Delar dokumentet i stycken. `spans` är dokumentets bitar i ordning: (byteintervall, `true` om
/// det är vanlig text som ska kontrolleras, `false` för markup, kod, länkar, bilder, tabeller).
/// Stycken utan någon bokstav i kontrollerbar text hoppas över.
pub fn paragraphs(text: &str, spans: &[(Range<usize>, bool)]) -> Vec<Para> {
    let mut out = Vec::new();
    let mut ls = 0;
    while ls <= text.len() {
        let nl = text[ls..].find('\n').map_or(text.len(), |i| ls + i);
        let mut le = nl;
        if text[ls..le].ends_with('\r') {
            le -= 1;
        }
        if le > ls {
            for r in split_long(text, ls..le, MAX_PARA) {
                if let Some(p) = make_para(text, spans, r) {
                    out.push(p);
                }
            }
        }
        ls = nl + 1;
    }
    out
}

fn make_para(text: &str, spans: &[(Range<usize>, bool)], r: Range<usize>) -> Option<Para> {
    let mut items: Vec<Item> = Vec::new();
    let mut push = |is_text: bool, range: Range<usize>| {
        if range.is_empty() {
            return;
        }
        match (items.last_mut(), is_text) {
            (Some(Item::Text(l)), true) | (Some(Item::Markup(l)), false) if l.end == range.start => l.end = range.end,
            _ => items.push(if is_text { Item::Text(range) } else { Item::Markup(range) }),
        }
    };
    let mut pos = r.start;
    let first = spans.partition_point(|(s, _)| s.end <= r.start);
    for (s, is_text) in &spans[first..] {
        if s.start >= r.end {
            break;
        }
        let (a, b) = (s.start.max(r.start), s.end.min(r.end));
        if a > pos {
            push(false, pos..a); // lucka mellan bitarna: räknas som markup
        }
        if b > a.max(pos) {
            push(*is_text, a.max(pos)..b);
        }
        pos = pos.max(b);
    }
    push(false, pos..r.end);
    let has_words = items.iter().any(|i| matches!(i, Item::Text(t) if text[t.clone()].chars().any(char::is_alphabetic)));
    has_words.then_some(Para { range: r, items })
}

impl Para {
    pub fn source<'a>(&self, text: &'a str) -> &'a str {
        &text[self.range.clone()]
    }

    /// Posterna i `data`-formatet. Sammanfogade är de exakt styckets källtext.
    fn annotation(&self, text: &str) -> Vec<Value> {
        self.items
            .iter()
            .map(|i| match i {
                Item::Text(r) => json!({ "text": &text[r.clone()] }),
                Item::Markup(r) => json!({ "markup": &text[r.clone()] }),
            })
            .collect()
    }
}

// --- Anrop: paketering ---------------------------------------------------------------------

/// Stycken som skickas i ett anrop.
#[derive(Debug)]
pub struct Batch {
    /// Index i styckelistan.
    pub paras: Vec<usize>,
    /// UTF-16-läge i anropets text där varje stycke börjar.
    pub starts: Vec<usize>,
    /// Värdet till parametern `data`.
    pub data: String,
}

/// Grupperar storlekar (i bytes) i anrop om högst `max` bytes; ett för stort stycke får ett eget.
pub fn pack(sizes: &[usize], max: usize) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut used = 0;
    for (i, &s) in sizes.iter().enumerate() {
        if !cur.is_empty() && used + s > max {
            out.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(i);
        used += s;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

const SEPARATOR: &str = "\n\n";

/// Bygger anrop av `idx` (index i `paras`), högst `max_bytes` JSON per anrop. Styckena skiljs åt
/// av en tom rad, som skickas som markup tolkad som styckebrytning.
pub fn make_batches(text: &str, paras: &[Para], idx: &[usize], max_bytes: usize) -> Vec<Batch> {
    let anns: Vec<Vec<Value>> = idx.iter().map(|&i| paras[i].annotation(text)).collect();
    let sizes: Vec<usize> = anns.iter().map(|a| a.iter().map(|v| v.to_string().len() + 1).sum::<usize>() + 40).collect();
    pack(&sizes, max_bytes)
        .into_iter()
        .map(|group| {
            let mut items: Vec<Value> = Vec::new();
            let mut starts = Vec::new();
            let mut at = 0usize;
            for (n, &g) in group.iter().enumerate() {
                if n > 0 {
                    items.push(json!({ "markup": SEPARATOR, "interpretAs": SEPARATOR }));
                    at += SEPARATOR.len();
                }
                starts.push(at);
                at += utf16_len(paras[idx[g]].source(text));
                items.extend(anns[g].iter().cloned());
            }
            Batch {
                paras: group.iter().map(|&g| idx[g]).collect(),
                starts,
                data: json!({ "annotation": items }).to_string(),
            }
        })
        .collect()
}

// --- Svar ------------------------------------------------------------------------------------

/// En träff som LanguageTool returnerar (lägen i UTF-16-enheter).
#[derive(Clone, Debug, PartialEq)]
pub struct RawMatch {
    pub offset: usize,
    pub length: usize,
    pub message: String,
    pub replacements: Vec<String>,
    pub rule_id: String,
    pub style: bool,
}

/// En träff i dokumentet.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// Byteintervall i dokumentets text.
    pub range: Range<usize>,
    pub message: String,
    pub replacements: Vec<String>,
    pub rule_id: String,
    /// Stilråd (bärnstensfärgad) i stället för grammatik (blå).
    pub style: bool,
    /// Den markerade texten (för "Ignorera").
    pub text: String,
}

/// Tolkar svaret från `/v2/check`. Stavningsträffar (`TYPOS`) tas bort: det är stavningskontrollens jobb.
pub fn parse_response(body: &str) -> Result<Vec<RawMatch>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("oväntat svar: {e}"))?;
    let arr = v.get("matches").and_then(Value::as_array).ok_or("oväntat svar: saknar träfflista")?;
    let mut out = Vec::new();
    for m in arr {
        let (Some(offset), Some(length)) =
            (m.get("offset").and_then(Value::as_u64), m.get("length").and_then(Value::as_u64))
        else {
            continue;
        };
        let rule = m.get("rule");
        let s = |v: Option<&Value>, k: &str| v.and_then(|r| r.get(k)).and_then(Value::as_str).unwrap_or("").to_owned();
        let category = s(rule.and_then(|r| r.get("category")), "id");
        let issue = s(rule, "issueType");
        if category == "TYPOS" || issue == "misspelling" {
            continue;
        }
        let style = matches!(category.as_str(), "STYLE" | "TYPOGRAPHY" | "REDUNDANCY" | "PLAIN_ENGLISH" | "WIKIPEDIA")
            || matches!(issue.as_str(), "style" | "typographical" | "register");
        let replacements = m
            .get("replacements")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|r| r.get("value").and_then(Value::as_str))
                    .filter(|r| !r.is_empty())
                    .take(MAX_REPLACEMENTS)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let message = m.get("message").and_then(Value::as_str).unwrap_or("").to_owned();
        out.push(RawMatch { offset: offset as usize, length: length as usize, message, replacements, rule_id: s(rule, "id"), style });
    }
    Ok(out)
}

/// Lägger träffarna på styckena i `batch`. Resultatet är (styckets index, träff) med träffens
/// intervall *relativt styckets början*. Träffar som hamnar i en skiljelinje eller mitt i ett
/// tecken hoppas över; en träff som går över styckets slut kortas av.
pub fn map_matches(text: &str, paras: &[Para], batch: &Batch, raw: &[RawMatch]) -> Vec<(usize, Match)> {
    let mut out = Vec::new();
    for m in raw {
        let k = batch.starts.partition_point(|&s| s <= m.offset);
        if k == 0 {
            continue;
        }
        let k = k - 1;
        let src = paras[batch.paras[k]].source(text);
        let len16 = utf16_len(src);
        let rel = m.offset - batch.starts[k];
        if rel >= len16 || m.length == 0 {
            continue;
        }
        let end16 = (rel + m.length).min(len16);
        let (Some(a), Some(b)) = (utf16_to_byte(src, rel), utf16_to_byte(src, end16)) else { continue };
        out.push((
            batch.paras[k],
            Match {
                range: a..b,
                message: m.message.clone(),
                replacements: m.replacements.clone(),
                rule_id: m.rule_id.clone(),
                style: m.style,
                text: src[a..b].to_owned(),
            },
        ));
    }
    out
}

// --- Pausa och takt ------------------------------------------------------------------------

/// Väntar tills texten varit orörd i `delay` sekunder. Tider är sekunder från valfri nollpunkt.
#[derive(Debug)]
pub struct Debounce {
    delay: f64,
    last: Option<(u64, u64)>,
    changed_at: f64,
}

impl Debounce {
    pub fn new(delay: f64) -> Self {
        Debounce { delay, last: None, changed_at: f64::NEG_INFINITY }
    }

    /// Anropas varje bildruta. Ändrad revision i samma dokument startar om pausen; ett nytt
    /// dokument (flikbyte, första gången) kontrolleras direkt.
    pub fn observe(&mut self, doc: u64, rev: u64, now: f64) {
        match self.last {
            Some((d, r)) if d == doc && r != rev => self.changed_at = now,
            Some((d, _)) if d == doc => {}
            _ => self.changed_at = f64::NEG_INFINITY,
        }
        self.last = Some((doc, rev));
    }

    /// Sekunder kvar av pausen (0 = klart).
    pub fn remaining(&self, now: f64) -> f64 {
        (self.changed_at + self.delay - now).max(0.0)
    }
}

/// Högst `max` anrop per 60 sekunder (`max == 0`: obegränsat).
#[derive(Debug)]
pub struct RateLimiter {
    max: usize,
    window: f64,
    stamps: VecDeque<f64>,
}

impl RateLimiter {
    pub fn new(max: usize, window: f64) -> Self {
        RateLimiter { max, window, stamps: VecDeque::new() }
    }

    /// Sekunder att vänta innan nästa anrop får göras (0 = nu).
    pub fn wait(&mut self, now: f64) -> f64 {
        while self.stamps.front().is_some_and(|&t| t + self.window <= now) {
            self.stamps.pop_front();
        }
        if self.max == 0 || self.stamps.len() < self.max {
            return 0.0;
        }
        self.stamps[self.stamps.len() - self.max] + self.window - now
    }

    pub fn record(&mut self, now: f64) {
        self.stamps.push_back(now);
    }
}

// --- Kontrollen ----------------------------------------------------------------------------

/// Inställningarna en kontroll gäller för; ändras något börjar cachen om.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub server: Server,
    /// LanguageTool-språk (`language_for`).
    pub lang: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Antal träffar i dokumentet.
    pub count: usize,
    /// Stycken väntar på kontroll eller svar.
    pub pending: bool,
    pub error: Option<String>,
}

struct Reply {
    id: u64,
    result: Result<Vec<RawMatch>, String>,
}

struct InFlight {
    id: u64,
    batch: Batch,
    /// Nyckel per stycke i `batch.paras` (cachenyckeln).
    keys: Vec<u64>,
    /// Texten som anropet byggdes på (träffarna mappas mot den).
    text: Arc<String>,
    paras: Arc<Vec<Para>>,
}

pub struct Checker {
    cfg: Option<Config>,
    clock: Instant,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    next_id: u64,
    inflight: Option<InFlight>,
    /// Träffar per stycke (hash av text + språk), relativt styckets början.
    cache: HashMap<u64, Arc<Vec<Match>>>,
    version: u64,
    /// Dokumentet som `paras` gäller: (dokument, revision, konfigurationsversion).
    doc: Option<(u64, u64)>,
    text: Arc<String>,
    paras: Arc<Vec<Para>>,
    keys: Vec<u64>,
    assembled: Option<(u64, u64, u64, Arc<Vec<Match>>)>,
    debounce: Debounce,
    limiter: RateLimiter,
    backoff_until: f64,
    ignored_rules: HashSet<String>,
    ignored: HashSet<(String, String)>,
    status: Status,
    /// Anropas från bakgrundstrådarna när ett svar kommit (väcker gränssnittet).
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

fn cache_key(para_text: &str, cfg: &Config) -> u64 {
    crate::word::layout::fast_hash(&format!("{}\u{0}{}\u{0}{}", cfg.lang, cfg.server.endpoint(), para_text))
}

impl Checker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Checker {
            cfg: None,
            clock: Instant::now(),
            tx,
            rx,
            next_id: 1,
            inflight: None,
            cache: HashMap::new(),
            version: 0,
            doc: None,
            text: Arc::default(),
            paras: Arc::default(),
            keys: Vec::new(),
            assembled: None,
            debounce: Debounce::new(DEBOUNCE_SECS),
            limiter: RateLimiter::new(0, 60.0),
            backoff_until: 0.0,
            ignored_rules: HashSet::new(),
            ignored: HashSet::new(),
            status: Status::default(),
            wake: None,
        }
    }

    pub fn set_wake(&mut self, f: Arc<dyn Fn() + Send + Sync>) {
        self.wake = Some(f);
    }

    pub fn has_wake(&self) -> bool {
        self.wake.is_some()
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    /// Glöm "Ignorera" för just den här träffen (texten + regeln) resten av sessionen.
    pub fn ignore_match(&mut self, m: &Match) {
        self.ignored.insert((m.rule_id.clone(), m.text.clone()));
        self.version += 1;
    }

    pub fn ignore_rule(&mut self, rule_id: &str) {
        self.ignored_rules.insert(rule_id.to_owned());
        self.version += 1;
    }

    fn is_ignored(&self, m: &Match) -> bool {
        self.ignored_rules.contains(&m.rule_id) || self.ignored.contains(&(m.rule_id.clone(), m.text.clone()))
    }

    /// Slutar kontrollera (funktionen är avslagen): tömmer pågående arbete men behåller cachen.
    pub fn idle(&mut self) {
        self.inflight = None;
        self.status = Status::default();
    }

    fn poll(&mut self, now: f64) {
        while let Ok(reply) = self.rx.try_recv() {
            let Some(fl) = self.inflight.take_if(|f| f.id == reply.id) else { continue };
            match reply.result {
                Ok(raw) => {
                    self.status.error = None;
                    let mut per: HashMap<usize, Vec<Match>> = HashMap::new();
                    for (pi, m) in map_matches(&fl.text, &fl.paras, &fl.batch, &raw) {
                        per.entry(pi).or_default().push(m);
                    }
                    for (n, &pi) in fl.batch.paras.iter().enumerate() {
                        self.cache.insert(fl.keys[n], Arc::new(per.remove(&pi).unwrap_or_default()));
                    }
                    self.version += 1;
                }
                Err(e) => {
                    self.status.error = Some(e);
                    self.backoff_until = now + BACKOFF_SECS;
                }
            }
        }
    }

    /// Anropas varje bildruta i dokumentläget med dokumentets text och bitar (se `paragraphs`).
    /// Returnerar träffarna (sorterade) och begär själv ny bildruta (`repaint_after`) när något
    /// väntar på pausen, takten eller ett svar.
    pub fn update(
        &mut self,
        cfg: &Config,
        doc_id: u64,
        rev: u64,
        text: &str,
        spans: impl FnOnce() -> Vec<(Range<usize>, bool)>,
        repaint_after: &mut dyn FnMut(Duration),
    ) -> Arc<Vec<Match>> {
        let now = self.now();
        if self.cfg.as_ref() != Some(cfg) {
            // Ny server eller nytt språk: allt som cachats gäller inte längre.
            self.cache.clear();
            self.inflight = None;
            self.doc = None;
            self.assembled = None;
            self.backoff_until = 0.0;
            self.limiter = RateLimiter::new(if cfg.server.is_public() { PUBLIC_REQ_PER_MIN } else { 0 }, 60.0);
            self.cfg = Some(cfg.clone());
            self.version += 1;
        }
        self.poll(now);
        self.debounce.observe(doc_id, rev, now);
        if self.doc != Some((doc_id, rev)) {
            let paras = paragraphs(text, &spans());
            self.keys = paras.iter().map(|p| cache_key(p.source(text), cfg)).collect();
            self.paras = Arc::new(paras);
            self.text = Arc::new(text.to_owned());
            self.doc = Some((doc_id, rev));
        }

        let mut missing: Vec<usize> = Vec::new();
        let mut seen = HashSet::new();
        for (i, k) in self.keys.iter().enumerate() {
            if !self.cache.contains_key(k) && seen.insert(*k) {
                missing.push(i);
            }
        }
        if !missing.is_empty() {
            self.dispatch(cfg, &missing, now, repaint_after);
        }
        self.status.pending = !missing.is_empty() && self.status.error.is_none() || self.inflight.is_some();

        let matches = match &self.assembled {
            Some((d, r, v, m)) if (*d, *r, *v) == (doc_id, rev, self.version) => m.clone(),
            _ => {
                let mut all: Vec<Match> = Vec::new();
                for (p, k) in self.paras.iter().zip(&self.keys) {
                    for m in self.cache.get(k).map(|v| v.as_slice()).unwrap_or(&[]) {
                        let m = Match { range: m.range.start + p.range.start..m.range.end + p.range.start, ..m.clone() };
                        if !self.is_ignored(&m) {
                            all.push(m);
                        }
                    }
                }
                all.sort_by_key(|m| m.range.start);
                let all = Arc::new(all);
                self.assembled = Some((doc_id, rev, self.version, all.clone()));
                all
            }
        };
        self.status.count = matches.len();
        matches
    }

    /// Skickar nästa anrop om pausen, takten och en eventuell paus efter fel tillåter det.
    fn dispatch(&mut self, cfg: &Config, missing: &[usize], now: f64, repaint_after: &mut dyn FnMut(Duration)) {
        if self.inflight.is_some() {
            repaint_after(Duration::from_millis(250));
            return;
        }
        let wait = self
            .debounce
            .remaining(now)
            .max(self.backoff_until - now)
            .max(self.limiter.wait(now));
        if wait > 0.0 {
            repaint_after(Duration::from_secs_f64(wait + 0.02));
            return;
        }
        let batches = make_batches(&self.text, &self.paras, missing, cfg.server.max_bytes());
        let Some(batch) = batches.into_iter().next() else { return };
        let keys = batch.paras.iter().map(|&p| self.keys[p]).collect();
        let id = self.next_id;
        self.next_id += 1;
        self.limiter.record(now);
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        let (url, lang, data) = (cfg.server.endpoint(), cfg.lang.clone(), batch.data.clone());
        std::thread::spawn(move || {
            let result = request(&url, &data, &lang);
            let _ = tx.send(Reply { id, result });
            if let Some(w) = wake {
                w();
            }
        });
        self.inflight = Some(InFlight { id, batch, keys, text: self.text.clone(), paras: self.paras.clone() });
        repaint_after(Duration::from_millis(250));
    }
}

/// Ett anrop till `/v2/check` (blockerar; körs i egen tråd). Följer `HTTPS_PROXY` som ureq i övrigt.
fn request(url: &str, data: &str, lang: &str) -> Result<Vec<RawMatch>, String> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into();
    let mut form = vec![("data", data), ("language", lang), ("disabledCategories", "TYPOS")];
    if lang == "auto" {
        form.push(("preferredVariants", "en-US,de-DE,pt-PT"));
    }
    let mut resp = agent.post(url).send_form(form).map_err(|e| match e {
        ureq::Error::StatusCode(429) => "för många anrop till servern, försöker igen strax".to_owned(),
        ureq::Error::StatusCode(code) => format!("servern svarade {code}"),
        e => format!("{e}"),
    })?;
    let body = resp.body_mut().with_config().limit(8 * 1024 * 1024).read_to_string().map_err(|e| format!("{e}"))?;
    parse_response(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "software": {"name": "LanguageTool", "version": "6.5"},
      "language": {"name": "Swedish", "code": "sv", "detectedLanguage": {"name": "Swedish", "code": "sv-SE"}},
      "matches": [
        {"message": "Möjligt skrivfel", "shortMessage": "Stavfel", "offset": 4, "length": 3,
         "replacements": [{"value": "hej"}],
         "context": {"text": "x", "offset": 0, "length": 1}, "sentence": "x",
         "rule": {"id": "MORFOLOGIK_RULE_SV", "description": "stavning", "issueType": "misspelling",
                  "category": {"id": "TYPOS", "name": "Möjliga stavfel"}}},
        {"message": "Använd inte dubbla mellanslag.", "offset": 10, "length": 2,
         "replacements": [{"value": " "}, {"value": "a"}, {"value": "b"}, {"value": "c"}, {"value": "d"}, {"value": "e"}, {"value": ""}],
         "rule": {"id": "WHITESPACE_RULE", "issueType": "whitespace", "category": {"id": "TYPOGRAPHY", "name": "Typografi"}}},
        {"message": "Kongruensfel", "offset": 0, "length": 5, "replacements": [],
         "rule": {"id": "AGREEMENT", "issueType": "grammar", "category": {"id": "GRAMMAR", "name": "Grammatik"}}}
      ]
    }"#;

    #[test]
    fn svar_tolkas() {
        let m = parse_response(SAMPLE).unwrap();
        assert_eq!(m.len(), 2, "stavningsträffen ska bort");
        assert_eq!(m[0].rule_id, "WHITESPACE_RULE");
        assert!(m[0].style);
        assert_eq!((m[0].offset, m[0].length), (10, 2));
        assert_eq!(m[0].replacements, vec![" ", "a", "b", "c", "d"], "högst fem, inga tomma");
        assert_eq!(m[1].message, "Kongruensfel");
        assert!(!m[1].style);
        assert!(parse_response("{}").is_err());
        assert!(parse_response("inte json").is_err());
        assert!(parse_response(r#"{"matches": []}"#).unwrap().is_empty());
    }

    #[test]
    fn serveradresser() {
        assert_eq!(Server::from_setting(""), Server::Public);
        assert_eq!(Server::from_setting("public"), Server::Public);
        assert_eq!(Server::Public.endpoint(), "https://api.languagetool.org/v2/check");
        let local = Server::from_setting(" http://localhost:8081/ ");
        assert_eq!(local.endpoint(), "http://localhost:8081/v2/check");
        assert_eq!(Server::from_setting("localhost:8081").endpoint(), "http://localhost:8081/v2/check");
        assert_eq!(Server::from_setting("http://x/v2/check").endpoint(), "http://x/v2/check");
        assert!(!local.is_public());
    }

    #[test]
    fn sprakkoder() {
        assert_eq!(language_for(None), "auto");
        assert_eq!(language_for(Some("sv_SE")), "sv");
        assert_eq!(language_for(Some("en_GB")), "en-GB");
        assert_eq!(language_for(Some("en")), "en-US");
        assert_eq!(language_for(Some("de_DE")), "de-DE");
        assert_eq!(language_for(Some("nb_NO")), "nb");
        assert_eq!(language_for(Some("fr_FR")), "fr");
    }

    /// Bitar för "Hej **fina** värld": text, markup, text, markup, text.
    fn spans_for(text: &str) -> Vec<(Range<usize>, bool)> {
        let mut v = Vec::new();
        let mut pos = 0;
        while let Some(i) = text[pos..].find("**") {
            if i > 0 {
                v.push((pos..pos + i, true));
            }
            v.push((pos + i..pos + i + 2, false));
            pos += i + 2;
        }
        if pos < text.len() {
            v.push((pos..text.len(), true));
        }
        v
    }

    #[test]
    fn markup_blir_markup_och_styckena_delas() {
        let t = "Hej **fina** värld\n\n# Rubrik\n12 34\nsista";
        let mut spans = spans_for(t);
        // "# " är markup
        let h = t.find("# ").unwrap();
        spans = spans
            .into_iter()
            .flat_map(|(r, tx)| {
                if tx && r.contains(&h) {
                    vec![(r.start..h, true), (h..h + 2, false), (h + 2..r.end, true)]
                } else {
                    vec![(r, tx)]
                }
            })
            .filter(|(r, _)| !r.is_empty())
            .collect();
        let ps = paragraphs(t, &spans);
        assert_eq!(ps.len(), 3, "tom rad och rad utan bokstäver hoppas över");
        assert_eq!(ps[0].source(t), "Hej **fina** värld");
        assert_eq!(
            ps[0].items,
            vec![Item::Text(0..4), Item::Markup(4..6), Item::Text(6..10), Item::Markup(10..12), Item::Text(12..19)]
        );
        assert_eq!(ps[1].source(t), "# Rubrik");
        assert_eq!(ps[1].items[0], Item::Markup(h..h + 2));
        assert_eq!(ps[2].source(t), "sista");
        // Sammanfogade poster är exakt källtexten.
        let ann = ps[0].annotation(t);
        let joined: String = ann.iter().map(|v| v.as_object().unwrap().values().next().unwrap().as_str().unwrap()).collect();
        assert_eq!(joined, ps[0].source(t));
    }

    #[test]
    fn luckor_blir_markup() {
        let t = "abc def";
        let ps = paragraphs(t, &[(0..3, true), (5..7, true)]);
        assert_eq!(ps[0].items, vec![Item::Text(0..3), Item::Markup(3..5), Item::Text(5..7)]);
    }

    #[test]
    fn lagen_genom_markup_och_utf16() {
        // "Hej **fina** värld 😀 slut." Parameter: offset i originaltexten (markup inräknad).
        let t = "Hej **fina** värld 😀 slut.\nNästa rad här";
        let spans = spans_for(t);
        let ps = paragraphs(t, &spans);
        let batches = make_batches(t, &ps, &[0, 1], 100_000);
        assert_eq!(batches.len(), 1);
        let b = &batches[0];
        let l0 = utf16_len(ps[0].source(t));
        assert_eq!(b.starts, vec![0, l0 + 2]);
        // "värld" börjar på UTF-16-läge 13 i första stycket.
        let raw = |offset, length| RawMatch {
            offset,
            length,
            message: "m".into(),
            replacements: vec![],
            rule_id: "R".into(),
            style: false,
        };
        let r = map_matches(t, &ps, b, &[raw(13, 5), raw(l0 + 2 + 6, 3), raw(l0, 1), raw(5, 1000)]);
        assert_eq!(r.len(), 3, "träffen i skiljelinjen hoppas över");
        assert_eq!(r[0].0, 0);
        assert_eq!(r[0].1.text, "värld");
        assert_eq!(&t[r[0].1.range.clone()], "värld");
        assert_eq!(r[1].0, 1);
        assert_eq!(r[1].1.text, "rad");
        // Går över styckets slut: kortas av vid slutet; lägen är relativa styckets början.
        assert_eq!(r[2].1.range.end, ps[0].source(t).len());
        assert_eq!(r[2].1.range.start, 5);
        // Efter emojin: "slut" börjar på UTF-16-läge 22 (😀 är två enheter).
        let r = map_matches(t, &ps, b, &[raw(22, 4)]);
        assert_eq!(r[0].1.text, "slut");
        // Mitt i ett surrogatpar går inte att mappa.
        assert!(map_matches(t, &ps, b, &[raw(20, 1)]).is_empty());
    }

    #[test]
    fn data_ar_giltig_json_med_skiljelinje() {
        let t = "Första.\nAndra.";
        let ps = paragraphs(t, &[(0..t.len(), true)]);
        let b = &make_batches(t, &ps, &[0, 1], 100_000)[0];
        let v: Value = serde_json::from_str(&b.data).unwrap();
        let a = v["annotation"].as_array().unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(a[1]["interpretAs"], "\n\n");
        assert_eq!(a[0]["text"], "Första.");
    }

    #[test]
    fn paketering() {
        assert_eq!(pack(&[], 10), Vec::<Vec<usize>>::new());
        assert_eq!(pack(&[4, 4, 4, 4, 4], 10), vec![vec![0, 1], vec![2, 3], vec![4]]);
        assert_eq!(pack(&[30, 4, 4], 10), vec![vec![0], vec![1, 2]], "för stort stycke får eget anrop");
        let t: String = (0..50).map(|i| format!("Stycke nummer {i} med lite text.\n")).collect();
        let ps = paragraphs(&t, &[(0..t.len(), true)]);
        assert_eq!(ps.len(), 50);
        let idx: Vec<usize> = (0..50).collect();
        let bs = make_batches(&t, &ps, &idx, 600);
        assert!(bs.len() > 3);
        assert!(bs.iter().all(|b| b.data.len() <= 600));
        let all: Vec<usize> = bs.iter().flat_map(|b| b.paras.clone()).collect();
        assert_eq!(all, idx, "alla stycken skickas, i ordning");
    }

    #[test]
    fn langa_rader_delas_vid_meningsslut() {
        let sentence = "Det här är en mening som är ganska lång. ";
        let t = sentence.repeat(400);
        let line = t.trim_end();
        let ps = paragraphs(line, &[(0..line.len(), true)]);
        assert!(ps.len() >= 3);
        for p in &ps {
            assert!(p.range.len() <= MAX_PARA);
            assert!(p.source(line).ends_with(". ") || p.range.end == line.len());
        }
        // Inget går förlorat.
        assert_eq!(ps.iter().map(|p| p.range.len()).sum::<usize>(), line.len());
        // Utan mellanslag klipps vid teckengräns (flerbytestecken hålls ihop).
        let t = "å".repeat(MAX_PARA * 2);
        let ps = paragraphs(&t, &[(0..t.len(), true)]);
        assert!(ps.len() >= 2 && ps.iter().all(|p| t.is_char_boundary(p.range.end)));
    }

    #[test]
    fn pausen_startar_om_vid_andring() {
        let mut d = Debounce::new(1.5);
        d.observe(1, 1, 0.0);
        assert_eq!(d.remaining(0.0), 0.0, "första gången kontrolleras direkt");
        d.observe(1, 2, 10.0);
        assert!((d.remaining(10.0) - 1.5).abs() < 1e-9);
        d.observe(1, 2, 11.0);
        assert!((d.remaining(11.0) - 0.5).abs() < 1e-9, "oförändrad revision startar inte om");
        d.observe(1, 3, 11.2);
        assert!((d.remaining(11.2) - 1.5).abs() < 1e-9, "ny ändring startar om");
        assert_eq!(d.remaining(13.0), 0.0);
        d.observe(2, 1, 13.1);
        assert_eq!(d.remaining(13.1), 0.0, "flikbyte väntar inte");
    }

    #[test]
    fn takten_hallls() {
        let mut l = RateLimiter::new(3, 60.0);
        for t in [0.0, 1.0, 2.0] {
            assert_eq!(l.wait(t), 0.0);
            l.record(t);
        }
        assert!((l.wait(3.0) - 57.0).abs() < 1e-9);
        assert_eq!(l.wait(60.0), 0.0, "första anropet har gått ut");
        l.record(60.0);
        l.record(61.0);
        assert!((l.wait(61.5) - 0.5).abs() < 1e-9, "väntar på äldsta kvarvarande (t=2)");
        let mut free = RateLimiter::new(0, 60.0);
        for _ in 0..100 {
            free.record(0.0);
        }
        assert_eq!(free.wait(0.0), 0.0);
    }

    fn cfg() -> Config {
        Config { server: Server::from_setting("http://localhost:1"), lang: "sv".into() }
    }

    #[test]
    fn kontrollen_vantar_pa_pausen_och_ignorerar() {
        let mut c = Checker::new();
        let t = "Hej värld.";
        let spans = || vec![(0..t.len(), true)];
        let mut after: Vec<Duration> = Vec::new();
        // Ändring i samma dokument: pausen gäller, ingenting skickas.
        c.update(&cfg(), 1, 1, t, spans, &mut |d| after.push(d));
        c.debounce.observe(1, 2, c.now());
        let m = c.update(&cfg(), 1, 2, t, spans, &mut |d| after.push(d));
        assert!(m.is_empty());
        assert!(c.inflight.is_none() || c.next_id == 2);
        // Lägg in ett cachat svar och kontrollera sammanställning och ignorering.
        let key = c.keys[0];
        let mm = Match {
            range: 0..3,
            message: "x".into(),
            replacements: vec![],
            rule_id: "R1".into(),
            style: false,
            text: "Hej".into(),
        };
        c.inflight = None;
        c.cache.insert(key, Arc::new(vec![mm.clone()]));
        c.version += 1;
        let m = c.update(&cfg(), 1, 2, t, spans, &mut |_| {});
        assert_eq!(m.len(), 1);
        assert_eq!(c.status().count, 1);
        c.ignore_match(&mm);
        assert!(c.update(&cfg(), 1, 2, t, spans, &mut |_| {}).is_empty());
        c.ignored.clear();
        c.version += 1;
        assert_eq!(c.update(&cfg(), 1, 2, t, spans, &mut |_| {}).len(), 1);
        c.ignore_rule("R1");
        assert!(c.update(&cfg(), 1, 2, t, spans, &mut |_| {}).is_empty());
    }

    #[test]
    fn svar_hamnar_i_cachen_utan_natverk() {
        let mut c = Checker::new();
        let t = "Hej **världen** nu\n\nandra stycket";
        let sp = spans_for(t);
        let spans = || sp.clone();
        let _ = c.update(&cfg(), 1, 1, t, spans, &mut |_| {});
        // Skicka svaret själva i stället för att nå ett nätverk.
        let fl = InFlight {
            id: 77,
            batch: make_batches(t, &c.paras, &[0], 10_000).remove(0),
            keys: vec![c.keys[0]],
            text: c.text.clone(),
            paras: c.paras.clone(),
        };
        // "världen" börjar på UTF-16-läge 6 (efter "Hej **").
        c.inflight = Some(fl);
        c.tx.send(Reply {
            id: 77,
            result: Ok(vec![RawMatch {
                offset: 6,
                length: 7,
                message: "m".into(),
                replacements: vec!["ö".into()],
                rule_id: "R".into(),
                style: true,
            }]),
        })
        .unwrap();
        let m = c.update(&cfg(), 1, 1, t, spans, &mut |_| {});
        assert_eq!(m.len(), 1);
        assert_eq!(&t[m[0].range.clone()], "världen");
        assert!(c.inflight.is_none() || c.inflight.as_ref().unwrap().id != 77);
        // Ändras språket börjar cachen om.
        let c2 = Config { lang: "en-US".into(), ..cfg() };
        assert!(c.update(&c2, 1, 1, t, spans, &mut |_| {}).is_empty());
    }

    /// Mot den publika servern (kräver nätverk): `cargo test -- --ignored grammatik_publik`.
    #[test]
    #[ignore]
    fn grammatik_publik() {
        let t = "Det är **en** fel mening med dubbla  mellanslag.";
        let ps = paragraphs(t, &spans_for(t));
        let b = &make_batches(t, &ps, &[0], 18_000)[0];
        let raw = request(&Server::Public.endpoint(), &b.data, "sv").unwrap();
        let mapped = map_matches(t, &ps, b, &raw);
        for (_, m) in &mapped {
            eprintln!("{:?} {:?} {}", m.text, m.replacements, m.rule_id);
        }
        assert!(mapped.iter().any(|(_, m)| m.text.contains("  ")));
    }
}
