//! Autokorrigering medan man skriver: rena funktioner som får texten och markören efter att ett
//! tecken skrivits och svarar med en ersättning (`Fix`) eller ingenting.
//!
//! Reglerna körs bara när ett ord eller en mening just avslutats (mellanslag, radbrytning,
//! skiljetecken) eller när själva tecknet ska bytas (citattecken, tre punkter, `(c)`). Markdown-syntax
//! (kodstaket, kodspann, länkar, HTML och kommentarer, webbadresser) lämnas alltid orörd.

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// Språket reglerna följer (citattecken och stavfel är språkberoende).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Sv,
    En,
    /// Annat språk: inga citattecken eller stavfelslistor, övriga regler gäller.
    Other,
}

impl Lang {
    /// Från en stavningskod som `sv_SE` eller `en-US`.
    pub fn from_code(code: &str) -> Lang {
        let l = code.to_ascii_lowercase();
        match l.split(['_', '-']).next().unwrap_or("") {
            "sv" => Lang::Sv,
            "en" => Lang::En,
            _ => Lang::Other,
        }
    }
}

/// Inställningarna för autokorrigering (sparas i `Settings`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    /// Autokorrigering i dokumentläget.
    pub document: bool,
    /// Autokorrigering även i Markdown och vanlig text (aldrig i kodfiler).
    pub text: bool,
    pub quotes: bool,
    pub dashes: bool,
    pub ellipsis: bool,
    pub capitalize: bool,
    pub two_capitals: bool,
    pub typos: bool,
    pub symbols: bool,
    /// Användarens egna ersättningar (listan nedan) används.
    pub replacements: bool,
    /// `None` = följ stavningsspråket, annars "sv" eller "en".
    pub lang: Option<String>,
    /// Egna ersättningar: (skrivs, blir).
    pub list: Vec<(String, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            document: true,
            text: false,
            quotes: true,
            dashes: true,
            ellipsis: true,
            capitalize: true,
            two_capitals: true,
            typos: true,
            symbols: true,
            replacements: true,
            lang: None,
            list: Vec::new(),
        }
    }
}

/// En ersättning: byt `range` i texten mot `text`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Fix {
    pub range: Range<usize>,
    pub text: String,
}

impl Fix {
    /// Texten med ersättningen gjord och var en markör på `caret` hamnar.
    pub fn apply(&self, text: &str, caret: usize) -> (String, usize) {
        let mut out = String::with_capacity(text.len() + self.text.len());
        out.push_str(&text[..self.range.start]);
        out.push_str(&self.text);
        out.push_str(&text[self.range.end..]);
        let new_caret = if caret >= self.range.end {
            caret + self.text.len() - self.range.len()
        } else {
            self.range.start + self.text.len()
        };
        (out, new_caret)
    }
}

/// Förkortningar efter vilka en punkt inte avslutar meningen (gemener, utan punkter).
const ABBREVIATIONS: &[&str] = &[
    "bl", "bla", "ca", "dvs", "etc", "fr", "inkl", "exkl", "jfr", "kl", "mfl", "mm", "nr", "osv", "pga", "resp",
    "ang", "enl", "tex", "ev", "sid", "tel", "ung", "omkr", "ex", "vs", "approx", "cf", "eg", "ie", "mr", "mrs", "ms",
    "dr", "prof", "st", "inc", "ltd", "fig", "no", "vol", "dept", "adr", "m", "o", "s", "ss", "red", "obs", "ref",
];

const TYPOS_SV: &[(&str, &str)] = &[
    ("ocskå", "också"),
    ("ocså", "också"),
    ("tilsammans", "tillsammans"),
    ("tillsamans", "tillsammans"),
    ("itne", "inte"),
    ("inet", "inte"),
    ("edt", "det"),
    ("dte", "det"),
    ("ocn", "och"),
    ("ochh", "och"),
    ("hjelp", "hjälp"),
    ("eftre", "efter"),
    ("sjäkv", "själv"),
    ("sjävl", "själv"),
    ("verkligne", "verkligen"),
    ("verkilgen", "verkligen"),
    ("faktikst", "faktiskt"),
    ("altså", "alltså"),
    ("alstå", "alltså"),
    ("nogon", "någon"),
    ("mkyet", "mycket"),
    ("myckte", "mycket"),
];

const TYPOS_EN: &[(&str, &str)] = &[
    ("teh", "the"),
    ("adn", "and"),
    ("taht", "that"),
    ("wich", "which"),
    ("recieve", "receive"),
    ("recieved", "received"),
    ("seperate", "separate"),
    ("definately", "definitely"),
    ("occured", "occurred"),
    ("untill", "until"),
    ("thier", "their"),
    ("becuase", "because"),
    ("beleive", "believe"),
    ("wierd", "weird"),
    ("acheive", "achieve"),
    ("alot", "a lot"),
    ("accomodate", "accommodate"),
    ("tommorow", "tomorrow"),
    ("freind", "friend"),
    ("goverment", "government"),
];

/// Tecken före ett ord som får stå kvar när ordet rättas (Markdown-markörer, citat, parenteser).
const PREFIX_OK: &str = "*_~\"'“”‘’«»„([{";
/// Tecken efter ett ord som får stå kvar när ordet rättas.
const SUFFIX_OK: &str = ".,;:!?*_~\"'“”‘’«»)]}…";

/// Kärnan i ett ord: (början, slut) som byteintervall i `token`.
fn split_core(token: &str) -> (usize, usize) {
    let start = token.char_indices().find(|(_, c)| c.is_alphanumeric()).map_or(token.len(), |(i, _)| i);
    let end = token
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map_or(start, |(i, c)| i + c.len_utf8());
    (start, end.max(start))
}

/// Startbyten för ordet (icke-blanka tecken) som slutar vid `end`.
fn token_start(text: &str, end: usize) -> usize {
    text[..end].char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8())
}

/// Startbyten för raden som `pos` står på.
fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Är texten vid `pos` en plats där Markdown-syntax eller kod gäller, så att inget får ändras?
fn protected(text: &str, pos: usize) -> bool {
    let before = &text[..pos];
    // Frontmatter (`---` överst).
    if let Some(rest) = text.strip_prefix("---\n") {
        let close = rest.find("\n---").map(|i| i + 4);
        if close.is_none_or(|c| pos <= c) {
            return true;
        }
    }
    let ls = line_start(text, pos);
    let line = &before[ls..];
    // Kodstaket: udda antal stängselrader före, eller raden är själv ett stängsel.
    let is_fence = |l: &str| {
        let t = l.trim_start_matches(' ');
        l.len() - t.len() <= 3 && (t.starts_with("```") || t.starts_with("~~~"))
    };
    if is_fence(line) || before[..ls].lines().filter(|l| is_fence(l)).count() % 2 == 1 {
        return true;
    }
    // Kodspann på raden.
    if line.matches('`').count() % 2 == 1 {
        return true;
    }
    // HTML-kommentar.
    if let Some(c) = before.rfind("<!--")
        && !before[c..].contains("-->")
    {
        return true;
    }
    // HTML-tagg.
    if let Some(lt) = before.rfind('<') {
        let tail = &before[lt..];
        if tail.len() < 400
            && !tail.contains('>')
            && tail[1..].chars().next().is_some_and(|c| c.is_alphabetic() || c == '/' || c == '!')
        {
            return true;
        }
    }
    // Länkmål `](…`, fotnotsreferens `[^…` och länkdefinition `[namn]: url`.
    if let Some(i) = line.rfind("](")
        && !line[i..].contains(')')
    {
        return true;
    }
    if let Some(i) = line.rfind("[^")
        && !line[i..].contains(']')
    {
        return true;
    }
    let lt = line.trim_start();
    if lt.starts_with('[') && lt.contains("]:") {
        return true;
    }
    // Webbadress, e-postadress eller sökväg i ordet.
    let tok = &before[token_start(before, pos)..];
    tok.contains("://") || tok.starts_with("www.") || tok.contains('@') || tok.contains('\\') || tok.contains("~/")
}

/// Citattecknet som ska ersätta det skrivna `"` eller `'`.
fn smart_quote(before: &str, typed: char, lang: Lang) -> Option<char> {
    let prev = before.chars().rev().find(|c| !matches!(c, '*' | '_' | '~'));
    let open = prev.is_none_or(|c| c.is_whitespace() || "([{<—–-/“‘«„=".contains(c));
    Some(match (lang, typed) {
        (Lang::Sv, '"') => '”',
        (Lang::Sv, _) => '’',
        (Lang::En, '"') => {
            if open {
                '“'
            } else {
                '”'
            }
        }
        (Lang::En, _) => {
            if open {
                '‘'
            } else {
                '’'
            }
        }
        (Lang::Other, _) => return None,
    })
}

/// `--` → `–` och `---` → `—` i ett ord (bara fristående eller mellan ord/siffror).
fn dashes(token: &str, line_before_token: &str) -> Option<String> {
    if !token.contains("--") {
        return None;
    }
    let chars: Vec<char> = token.chars().collect();
    let mut out = String::with_capacity(token.len());
    let mut i = 0;
    let mut changed = false;
    while i < chars.len() {
        if chars[i] != '-' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j] == '-' {
            j += 1;
        }
        let run = j - i;
        let b = if i == 0 { None } else { Some(chars[i - 1]) };
        let a = chars.get(j).copied();
        let b_ok = b.is_none_or(char::is_alphanumeric);
        let a_ok = a.is_none_or(char::is_alphanumeric);
        // `--flagga` och `---` ensamt på en rad (linje, frontmatter) lämnas.
        let leading_flag = b.is_none() && a.is_some();
        let rule = b.is_none() && a.is_none() && line_before_token.trim().is_empty();
        if (run == 2 || run == 3) && b_ok && a_ok && !leading_flag && !rule {
            out.push(if run == 2 { '–' } else { '—' });
            changed = true;
        } else {
            out.extend(&chars[i..j]);
        }
        i = j;
    }
    changed.then_some(out)
}

fn capitalize_first(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        Some(c) => c.to_uppercase().chain(it).collect(),
        None => String::new(),
    }
}

fn decapitalize_first(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        Some(c) => c.to_lowercase().chain(it).collect(),
        None => String::new(),
    }
}

/// Ger `to` samma versalform som ordet `core` (VERSALER, Versal, gemener).
fn match_case(core: &str, to: &str) -> String {
    let mut letters = core.chars().filter(|c| c.is_alphabetic());
    let all_upper = letters.clone().count() > 1 && letters.all(char::is_uppercase);
    if all_upper {
        to.to_uppercase()
    } else if core.chars().next().is_some_and(char::is_uppercase) {
        capitalize_first(to)
    } else {
        to.to_owned()
    }
}

/// Inleder ordet som börjar vid `token_start` (en byte i `before`) en ny mening eller ett nytt stycke?
fn sentence_start(before: &str, token_start: usize) -> bool {
    let pre = &before[..token_start];
    // Efter rubrik-, lista- eller citatmarkör.
    if is_line_marker(&pre[line_start(pre, pre.len())..]) {
        return true;
    }
    let trimmed = pre.trim_end();
    if trimmed.is_empty() {
        return true;
    }
    let ws = &pre[trimmed.len()..];
    if ws.is_empty() {
        return false;
    }
    if ws.matches('\n').count() >= 2 {
        return true;
    }
    // Sista tecknet före blanktecknen, förbi avslutande citat och markörer.
    let last = trimmed.trim_end_matches(|c| "\"'“”‘’»)]}*_~".contains(c));
    match last.chars().next_back() {
        Some('!' | '?') => true,
        Some('.') => sentence_ends_at(&last[..last.len() - 1]),
        _ => false,
    }
}

/// `prefix` (radens början fram till ordet) är bara markörer (`#`, `>`, `-`, `1.`, `- [ ]`) med mellanrum.
fn is_line_marker(prefix: &str) -> bool {
    if !prefix.ends_with([' ', '\t']) {
        return false;
    }
    let mut rest = prefix.trim_start();
    if rest.is_empty() {
        return false;
    }
    loop {
        let t = rest.trim_start();
        if t.is_empty() {
            return true;
        }
        // Kryssrutan "[ ]" tas som en enhet.
        if let Some(r) = ["[ ]", "[x]", "[X]"].iter().find_map(|b| t.strip_prefix(b)) {
            rest = r;
            continue;
        }
        let end = t.find(char::is_whitespace).unwrap_or(t.len());
        let tok = &t[..end];
        let digits = tok.len() >= 2
            && (tok.ends_with('.') || tok.ends_with(')'))
            && tok[..tok.len() - 1].chars().all(|c| c.is_ascii_digit());
        let ok = tok.chars().all(|c| c == '#') && tok.len() <= 6
            || tok.chars().all(|c| c == '>')
            || matches!(tok, "-" | "*" | "+")
            || digits;
        if !ok {
            return false;
        }
        rest = &t[end..];
    }
}

/// Avslutar en punkt efter `before` (texten före punkten) en mening? Förkortningar, initialer och
/// siffror (3.5, "den 3. januari") gör det inte.
fn sentence_ends_at(before: &str) -> bool {
    let rev: Vec<char> = before.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '.').collect();
    let word: String = rev.into_iter().rev().collect();
    if word.is_empty() {
        return true;
    }
    if word.contains('.') {
        return false;
    }
    let lower = word.to_lowercase();
    if ABBREVIATIONS.contains(&lower.as_str()) {
        return false;
    }
    if word.chars().count() == 1 && word.chars().all(char::is_alphabetic) {
        return false;
    }
    !word.ends_with(|c: char| c.is_ascii_digit())
}

/// Rättar ett ord (kärnan) i tur och ordning: stavfel, två inledande versaler, meningens första bokstav.
fn fix_core(core: &str, cfg: &Config, lang: Lang, starts_sentence: bool) -> Option<String> {
    let mut cur = core.to_owned();
    if cfg.typos {
        let list = match lang {
            Lang::Sv => TYPOS_SV,
            Lang::En => TYPOS_EN,
            Lang::Other => &[],
        };
        let lower = cur.to_lowercase();
        if let Some((_, right)) = list.iter().find(|(w, _)| *w == lower) {
            cur = match_case(&cur, right);
        }
    }
    let plain = cur.chars().all(|c| c.is_alphabetic() || matches!(c, '\'' | '’' | '-'));
    if cfg.two_capitals && plain {
        let up = cur.chars().take_while(|c| c.is_uppercase()).count();
        let rest_lower = cur.chars().skip(up).all(|c| c.is_lowercase() || matches!(c, '\'' | '’' | '-'));
        if up == 2 && cur.chars().count() >= 4 && rest_lower {
            let mut it = cur.chars();
            let first = it.next().unwrap_or(' ');
            let second = it.next().unwrap_or(' ');
            let tail: String = it.collect();
            cur = format!("{first}{}{tail}", second.to_lowercase());
        }
    }
    if cfg.capitalize && plain && starts_sentence {
        let first_lower = cur.chars().next().is_some_and(char::is_lowercase);
        if first_lower && !cur.chars().skip(1).any(char::is_uppercase) {
            cur = capitalize_first(&cur);
        }
    }
    (cur != core).then_some(cur)
}

/// Användarens ersättning för ordet (kärnan, eller hela ordet inklusive skiljetecknet som skrevs).
fn user_replacement(cfg: &Config, word: &str) -> Option<String> {
    if !cfg.replacements || word.is_empty() {
        return None;
    }
    for (from, to) in &cfg.list {
        let from = from.trim();
        if from.is_empty() || from.chars().any(char::is_whitespace) || to.is_empty() {
            continue;
        }
        if from == word {
            return Some(to.clone());
        }
        // "Mvh" matchar "mvh" och ger en versal i början.
        if word != from && decapitalize_first(word) == from {
            return Some(capitalize_first(to));
        }
    }
    None
}

/// Tecknet före `caret` är nyss skrivet: svara med en ersättning, om någon regel gäller.
pub fn on_typed(text: &str, caret: usize, cfg: &Config, lang: Lang) -> Option<Fix> {
    if caret == 0 || caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let typed = text[..caret].chars().next_back()?;
    let tpos = caret - typed.len_utf8();
    if protected(text, tpos) {
        return None;
    }
    let before = &text[..tpos];
    match typed {
        '"' | '\'' => {
            if !cfg.quotes {
                return None;
            }
            let q = smart_quote(before, typed, lang)?;
            return Some(Fix { range: tpos..caret, text: q.to_string() });
        }
        '.' if cfg.ellipsis => {
            if before.ends_with("..") && !before.ends_with("...") && !text[caret..].starts_with(['.', '/']) {
                let s = tpos - 2;
                if !before[..s].ends_with(['.', '/', '\\']) {
                    return Some(Fix { range: s..caret, text: "…".to_owned() });
                }
            }
        }
        ')' if cfg.symbols => {
            let low = before.to_lowercase();
            for (m, sym) in [("(c", "©"), ("(r", "®"), ("(tm", "™")] {
                if low.ends_with(m) {
                    let s = tpos - m.len();
                    let alone = before[line_start(before, s)..s].trim().is_empty();
                    // `(c)` först på raden är en listbokstav, och ord som `abc(c)` är kod.
                    if !(alone && m != "(tm") && !before[..s].ends_with(char::is_alphanumeric) {
                        return Some(Fix { range: s..caret, text: sym.to_owned() });
                    }
                }
            }
        }
        _ => {}
    }
    let boundary = typed.is_whitespace();
    let punct = matches!(typed, ',' | ';' | ':' | '!' | '?' | '.' | ')');
    if !boundary && !punct {
        return None;
    }
    let start = token_start(before, tpos);
    let token = &before[start..];
    if token.is_empty() {
        return None;
    }
    // Egna ersättningar: först hela ordet med skiljetecknet (":)"), sedan ordet för sig.
    if punct && let Some(r) = user_replacement(cfg, &format!("{token}{typed}")) {
        return Some(Fix { range: start..caret, text: r });
    }
    let (cs, ce) = split_core(token);
    let affix_ok = |t: &str, cs: usize, ce: usize| {
        t[..cs].chars().all(|c| PREFIX_OK.contains(c)) && t[ce..].chars().all(|c| SUFFIX_OK.contains(c))
    };
    if ce > cs
        && affix_ok(token, cs, ce)
        && let Some(r) = user_replacement(cfg, &token[cs..ce])
    {
        return Some(Fix { range: start + cs..start + ce, text: r });
    }
    // Övriga ordregler körs när ordet avslutats med mellanslag, radbrytning eller kommatecken m.m.,
    // inte vid punkt eller parentes (t.ex. "t.ex." är inte ett ord som ska rättas).
    if typed == '.' || typed == ')' {
        return None;
    }
    let ls = line_start(before, start);
    let mut new_token = token.to_owned();
    let table_line = before[ls..].trim_start().starts_with('|');
    if cfg.dashes
        && !table_line
        && let Some(d) = dashes(token, &before[ls..start])
    {
        new_token = d;
    }
    let (ncs, nce) = split_core(&new_token);
    if nce > ncs && affix_ok(&new_token, ncs, nce) {
        let starts = cfg.capitalize && sentence_start(before, start);
        if let Some(fixed) = fix_core(&new_token[ncs..nce], cfg, lang, starts) {
            new_token = format!("{}{}{}", &new_token[..ncs], fixed, &new_token[nce..]);
        }
    }
    (new_token != token).then(|| Fix { range: start..tpos, text: new_token })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Lang::{En, Sv};

    /// Skriver `typed` efter `text` (markören sist) och returnerar resultatet.
    fn type_in(text: &str, typed: char, lang: Lang) -> String {
        type_cfg(text, typed, lang, &Config::default())
    }

    fn type_cfg(text: &str, typed: char, lang: Lang, cfg: &Config) -> String {
        let full = format!("{text}{typed}");
        match on_typed(&full, full.len(), cfg, lang) {
            Some(f) => f.apply(&full, full.len()).0,
            None => full,
        }
    }

    #[test]
    fn smart_quotes_swedish() {
        assert_eq!(type_in("Han sa ", '"', Sv), "Han sa ”");
        assert_eq!(type_in("Han sa \"hej", '"', Sv), "Han sa \"hej”");
        assert_eq!(type_in("Hans", '\'', Sv), "Hans’");
        assert_eq!(type_in("", '"', Sv), "”");
    }

    #[test]
    fn smart_quotes_english() {
        assert_eq!(type_in("He said ", '"', En), "He said “");
        assert_eq!(type_in("He said “hi", '"', En), "He said “hi”");
        assert_eq!(type_in("don", '\'', En), "don’");
        assert_eq!(type_in("a ", '\'', En), "a ‘");
        assert_eq!(type_in("(", '"', En), "(“");
        // Öppnande efter Markdown-markör.
        assert_eq!(type_in("**", '"', En), "**“");
    }

    #[test]
    fn quotes_off_and_other_language() {
        let cfg = Config { quotes: false, ..Config::default() };
        assert_eq!(type_cfg("a ", '"', En, &cfg), "a \"");
        assert_eq!(type_in("a ", '"', Lang::Other), "a \"");
    }

    #[test]
    fn dashes_rule() {
        assert_eq!(type_in("ett -- två", ' ', Sv), "ett -- två ");
        assert_eq!(type_in("ett --", ' ', Sv), "ett – ");
        assert_eq!(type_in("ett ---", ' ', Sv), "ett — ");
        assert_eq!(type_in("ord--", ' ', Sv), "ord– ");
        assert_eq!(type_in("1--2", ' ', Sv), "1–2 ");
        // Fyra streck, flaggor, linjer, tabeller och HTML-kommentarer lämnas.
        assert_eq!(type_in("a ----", ' ', Sv), "a ---- ");
        assert_eq!(type_in("kör --help", ' ', En), "kör --help ");
        assert_eq!(type_in("text\n\n---", '\n', Sv), "text\n\n---\n");
        assert_eq!(type_in("---", '\n', Sv), "---\n");
        assert_eq!(type_in("<!--", ' ', Sv), "<!-- ");
        assert_eq!(type_in("| a | --", ' ', Sv), "| a | -- ");
        assert_eq!(type_in("|---|---|", '\n', Sv), "|---|---|\n");
        let cfg = Config { dashes: false, ..Config::default() };
        assert_eq!(type_cfg("ett --", ' ', Sv, &cfg), "ett -- ");
    }

    #[test]
    fn ellipsis_rule() {
        assert_eq!(type_in("Vänta..", '.', Sv), "Vänta…");
        assert_eq!(type_in("Vänta...", '.', Sv), "Vänta....");
        assert_eq!(type_in("cd ../..", '.', Sv), "cd ../...");
        assert_eq!(type_in("a.", '.', Sv), "a..");
        let cfg = Config { ellipsis: false, ..Config::default() };
        assert_eq!(type_cfg("Vänta..", '.', Sv, &cfg), "Vänta...");
    }

    #[test]
    fn symbols_rule() {
        assert_eq!(type_in("Kladd (c", ')', Sv), "Kladd ©");
        assert_eq!(type_in("Kladd (C", ')', Sv), "Kladd ©");
        assert_eq!(type_in("Kladd (tm", ')', En), "Kladd ™");
        assert_eq!(type_in("Kladd (r", ')', En), "Kladd ®");
        // Listpunkt (c) i början av raden och ord som slutar på (c) lämnas.
        assert_eq!(type_in("(c", ')', Sv), "(c)");
        assert_eq!(type_in("a\n(c", ')', Sv), "a\n(c)");
        assert_eq!(type_in("abc(c", ')', Sv), "abc(c)");
        let cfg = Config { symbols: false, ..Config::default() };
        assert_eq!(type_cfg("Kladd (c", ')', Sv, &cfg), "Kladd (c)");
    }

    #[test]
    fn capitalize_sentences() {
        assert_eq!(type_in("hej", ' ', Sv), "Hej ");
        assert_eq!(type_in("Det är bra. nu", ' ', Sv), "Det är bra. Nu ");
        assert_eq!(type_in("Vad? nu", ' ', Sv), "Vad? Nu ");
        assert_eq!(type_in("Vad! \"nu", ' ', En), "Vad! \"Nu ");
        assert_eq!(type_in("Rad ett.\n\nnästa", ' ', Sv), "Rad ett.\n\nNästa ");
        assert_eq!(type_in("Rad ett\n\nnästa", ' ', Sv), "Rad ett\n\nNästa ");
        assert_eq!(type_in("Klart. **nu", ' ', Sv), "Klart. **Nu ");
        assert_eq!(type_in("hej,", ' ', Sv), "Hej, ");
        // Mitt i en mening, efter komma och i hårdbruten text.
        assert_eq!(type_in("Det är bra, nu", ' ', Sv), "Det är bra, nu ");
        assert_eq!(type_in("Det är\nbra", ' ', Sv), "Det är\nbra ");
        // Rubrik-, lista- och citatmarkörer.
        assert_eq!(type_in("# rubrik", ' ', Sv), "# Rubrik ");
        assert_eq!(type_in("Text\n\n- punkt", ' ', Sv), "Text\n\n- Punkt ");
        assert_eq!(type_in("1. första", '\n', Sv), "1. Första\n");
        assert_eq!(type_in("> citat", ' ', Sv), "> Citat ");
        assert_eq!(type_in("- [ ] sak", ' ', Sv), "- [ ] Sak ");
    }

    #[test]
    fn capitalize_skips_abbreviations_numbers_urls() {
        assert_eq!(type_in("Vi har t.ex. en", ' ', Sv), "Vi har t.ex. en ");
        assert_eq!(type_in("Vi har bl.a. en", ' ', Sv), "Vi har bl.a. en ");
        assert_eq!(type_in("We have e.g. a", ' ', En), "We have e.g. a ");
        assert_eq!(type_in("Se ca. tre", ' ', Sv), "Se ca. tre ");
        assert_eq!(type_in("Hej osv. och", ' ', Sv), "Hej osv. och ");
        assert_eq!(type_in("Den 3. januari", ' ', Sv), "Den 3. januari ");
        assert_eq!(type_in("Värdet är 3.5 och", ' ', Sv), "Värdet är 3.5 och ");
        assert_eq!(type_in("A. nilsson", ' ', Sv), "A. nilsson ");
        assert_eq!(type_in("Se https://exempel.se. nu", ' ', Sv), "Se https://exempel.se. nu ");
        assert_eq!(type_in("Klart. www.exempel.se", ' ', Sv), "Klart. www.exempel.se ");
        assert_eq!(type_in("Klart. mig@exempel.se", ' ', Sv), "Klart. mig@exempel.se ");
        // Ord med versaler, understreck, snedstreck eller fyrkant, och ord som inte börjar en mening.
        assert_eq!(type_in("Klart. iPhone", ' ', Sv), "Klart. iPhone ");
        assert_eq!(type_in("Klart. fil_1", ' ', Sv), "Klart. fil_1 ");
        assert_eq!(type_in("Klart. a/b", ' ', Sv), "Klart. a/b ");
        assert_eq!(type_in("Klart. #tagg", ' ', Sv), "Klart. #tagg ");
        let cfg = Config { capitalize: false, ..Config::default() };
        assert_eq!(type_cfg("hej", ' ', Sv, &cfg), "hej ");
    }

    #[test]
    fn two_initial_capitals() {
        assert_eq!(type_in("Hej SVerige", ' ', Sv), "Hej Sverige ");
        assert_eq!(type_in("Hej ÅSbjörn", ' ', Sv), "Hej Åsbjörn ");
        assert_eq!(type_in("Hej SVerige,", ' ', Sv), "Hej Sverige, ");
        // Förkortningar, flertal och blandade ord lämnas.
        assert_eq!(type_in("Hej IDs", ' ', Sv), "Hej IDs ");
        assert_eq!(type_in("Hej PDFer", ' ', Sv), "Hej PDFer ");
        assert_eq!(type_in("Hej USA", ' ', Sv), "Hej USA ");
        assert_eq!(type_in("Hej McDonald", ' ', Sv), "Hej McDonald ");
        let cfg = Config { two_capitals: false, ..Config::default() };
        assert_eq!(type_cfg("Hej SVerige", ' ', Sv, &cfg), "Hej SVerige ");
    }

    #[test]
    fn typo_lists() {
        assert_eq!(type_in("Vi ocskå", ' ', Sv), "Vi också ");
        assert_eq!(type_in("Vi tilsammans", ' ', Sv), "Vi tillsammans ");
        assert_eq!(type_in("Vi Ocskå", ' ', Sv), "Vi Också ");
        assert_eq!(type_in("Vi OCSKÅ", ' ', Sv), "Vi OCKSÅ ");
        assert_eq!(type_in("We recieve", ' ', En), "We receive ");
        assert_eq!(type_in("We recieve,", ' ', En), "We receive, ");
        assert_eq!(type_in("Teh", ' ', En), "The ");
        assert_eq!(type_in("It is alot", ' ', En), "It is a lot ");
        // Fel språk: svenska listan gäller inte engelskan.
        assert_eq!(type_in("Vi ocskå", ' ', En), "Vi ocskå ");
        assert_eq!(type_in("Vi ocskå", ' ', Lang::Other), "Vi ocskå ");
        let cfg = Config { typos: false, ..Config::default() };
        assert_eq!(type_cfg("Vi ocskå", ' ', Sv, &cfg), "Vi ocskå ");
    }

    #[test]
    fn user_replacements() {
        let cfg = Config {
            list: vec![
                ("mvh".into(), "Med vänliga hälsningar".into()),
                (":)".into(), "🙂".into()),
                ("tex ".into(), "ska ignoreras".into()),
            ],
            ..Config::default()
        };
        assert_eq!(type_cfg("Hej\n\nmvh", ' ', Sv, &cfg), "Hej\n\nMed vänliga hälsningar ");
        assert_eq!(type_cfg("Hej\n\nmvh", '\n', Sv, &cfg), "Hej\n\nMed vänliga hälsningar\n");
        assert_eq!(type_cfg("Hej\n\nMvh", ' ', Sv, &cfg), "Hej\n\nMed vänliga hälsningar ");
        assert_eq!(type_cfg("Tack mvh", '.', Sv, &cfg), "Tack Med vänliga hälsningar.");
        assert_eq!(type_cfg("Tack mvh,", ' ', Sv, &cfg), "Tack Med vänliga hälsningar, ");
        assert_eq!(type_cfg("Tack :", ')', Sv, &cfg), "Tack 🙂");
        assert_eq!(type_cfg("Tack xmvh", ' ', Sv, &cfg), "Tack xmvh ");
        let off = Config { replacements: false, ..cfg.clone() };
        assert_eq!(type_cfg("Hej\n\nmvh", ' ', Sv, &off), "Hej\n\nMvh ");
    }

    #[test]
    fn markdown_and_code_untouched() {
        // Kodspann, kodstaket, länkar, HTML och kommentarer.
        assert_eq!(type_in("En `kod", ' ', Sv), "En `kod ");
        assert_eq!(type_in("En `a ocskå", ' ', Sv), "En `a ocskå ");
        assert_eq!(type_in("```rust\nlet s = \"a", '"', En), "```rust\nlet s = \"a\"");
        assert_eq!(type_in("```\nfn tilsammans", ' ', Sv), "```\nfn tilsammans ");
        assert_eq!(type_in("```\nkod\n```\n\nhej", ' ', Sv), "```\nkod\n```\n\nHej ");
        assert_eq!(type_in("[länk](http://x.se \"titel", '"', En), "[länk](http://x.se \"titel\"");
        assert_eq!(type_in("[länk](sida ocskå", ' ', Sv), "[länk](sida ocskå ");
        assert_eq!(type_in("<span style=\"a", '"', En), "<span style=\"a\"");
        assert_eq!(type_in("<!-- ocskå", ' ', Sv), "<!-- ocskå ");
        assert_eq!(type_in("<!-- a -->\n\nhej", ' ', Sv), "<!-- a -->\n\nHej ");
        assert_eq!(type_in("[1]: http://x.se \"a", '"', En), "[1]: http://x.se \"a\"");
        assert_eq!(type_in("---\ntitle: \"a", '"', En), "---\ntitle: \"a\"");
        // Fet och kursiv text rättas men markörerna lämnas.
        assert_eq!(type_in("Klart. **nu**", ' ', Sv), "Klart. **Nu** ");
        assert_eq!(type_in("Klart. _nu_", ' ', Sv), "Klart. _Nu_ ");
        assert_eq!(type_in("En **ocskå**", ' ', Sv), "En **också** ");
        // Ett mindre-än-tecken är inte en tagg.
        assert_eq!(type_in("a < b ocskå", ' ', Sv), "a < b också ");
    }

    #[test]
    fn caret_in_middle_and_multibyte() {
        // Tecknet skrivet mitt i texten: det som står efter markören rörs inte.
        let text = "ocskå  resten";
        let caret = "ocskå ".len();
        let f = on_typed(text, caret, &Config::default(), Sv).unwrap();
        let (out, c) = f.apply(text, caret);
        assert_eq!(out, "Också  resten");
        assert_eq!(&out[..c], "Också ");
        // Flerbytetecken före markören.
        assert_eq!(type_in("Åsa sa: ”hej”. är", ' ', Sv), "Åsa sa: ”hej”. Är ");
        assert!(on_typed("é", 1, &Config::default(), Sv).is_none());
        assert!(on_typed("abc", 99, &Config::default(), Sv).is_none());
        assert!(on_typed("", 0, &Config::default(), Sv).is_none());
    }

    #[test]
    fn enter_trigger() {
        assert_eq!(type_in("hej ocskå", '\n', Sv), "hej också\n");
        assert_eq!(type_in("```\nocskå", '\n', Sv), "```\nocskå\n");
    }

    #[test]
    fn lang_codes() {
        assert_eq!(Lang::from_code("sv_SE"), Lang::Sv);
        assert_eq!(Lang::from_code("en-US"), Lang::En);
        assert_eq!(Lang::from_code("de_DE"), Lang::Other);
    }

    #[test]
    fn config_defaults_from_old_settings() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.document && !c.text && c.quotes && c.list.is_empty());
        let c: Config = serde_json::from_str(r#"{"text":true,"list":[["a","b"]]}"#).unwrap();
        assert!(c.text && c.document);
        assert_eq!(c.list, vec![("a".to_owned(), "b".to_owned())]);
    }
}
