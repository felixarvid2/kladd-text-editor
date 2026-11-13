//! Översättning av gränssnittet (svenska och engelska).
//!
//! Gettext-stil, nycklad på den svenska källtexten: koden förblir läsbar och en text som saknas i
//! den engelska tabellen visas på svenska. Hur en fil konverteras beskrivs i `docs/dev/oversattning.md`.
//!
//! * `t("Spara")` ger den text som gäller för aktuellt språk (`&'static str`).
//! * `tf!("Sparade {namn}", namn = x)` översätter mallen och byter ut `{namn}` (bara namngivna
//!   platshållare; värdena formateras med `Display` innan de sätts in). Klammrar skrivs `{{` och `}}`
//!   som i `format!`. Formatspecifikationer (`{x:.1}`) stöds inte: formatera värdet först.
//! * Tabellerna ligger i `i18n/en_*.rs`, en per område, så att flera kan fylla dem samtidigt.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

mod en_annot2;
mod en_app;
mod en_autocorrect;
mod en_csv;
mod en_doc;
mod en_incr;
mod en_menus;
mod en_git;
mod en_lsp;
mod en_misc;
mod en_odtrev;
mod en_pdf;
mod en_review;
mod en_templates;
mod en_track2;
mod en_word;

/// Språket gränssnittet visas på.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Sv,
    En,
}

/// Inställningen "Språk / Language".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum LangSetting {
    /// Följ systemets språk: svenska för sv*, annars engelska.
    #[default]
    Auto,
    Sv,
    En,
}

impl LangSetting {
    pub const ALL: [LangSetting; 3] = [LangSetting::Auto, LangSetting::Sv, LangSetting::En];

    /// Namnet i rullgardinen.
    pub fn label(self) -> &'static str {
        match self {
            LangSetting::Auto => t("Automatiskt"),
            LangSetting::Sv => "Svenska",
            LangSetting::En => "English",
        }
    }

    pub fn resolve(self) -> Lang {
        match self {
            LangSetting::Sv => Lang::Sv,
            LangSetting::En => Lang::En,
            LangSetting::Auto => sys_locale::get_locale().map_or(Lang::En, |l| lang_from_locale(&l)),
        }
    }
}

/// Svenska för "sv", "sv-SE", "sv_FI" …, annars engelska.
pub fn lang_from_locale(locale: &str) -> Lang {
    let l = locale.to_ascii_lowercase();
    if l == "sv" || l.starts_with("sv-") || l.starts_with("sv_") { Lang::Sv } else { Lang::En }
}

#[cfg(not(test))]
static CURRENT: AtomicU8 = AtomicU8::new(0);

// I tester har varje testtråd sitt eget språk, så att ett test som byter till engelska inte
// påverkar de andra som kör samtidigt.
#[cfg(test)]
thread_local! {
    static CURRENT: AtomicU8 = const { AtomicU8::new(0) };
}

/// Sätter aktuellt språk. Gäller direkt för alla efterföljande `t()`-anrop.
pub fn set_lang(l: Lang) {
    let v = if l == Lang::En { 1 } else { 0 };
    #[cfg(not(test))]
    CURRENT.store(v, Ordering::Relaxed);
    #[cfg(test)]
    CURRENT.with(|c| c.store(v, Ordering::Relaxed));
}

pub fn lang() -> Lang {
    #[cfg(not(test))]
    let v = CURRENT.load(Ordering::Relaxed);
    #[cfg(test)]
    let v = CURRENT.with(|c| c.load(Ordering::Relaxed));
    if v == 1 { Lang::En } else { Lang::Sv }
}

static EN: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    for &tab in tables() {
        for &(sv, en) in tab {
            m.insert(sv, en);
        }
    }
    m
});

fn tables() -> &'static [&'static [(&'static str, &'static str)]] {
    &[en_app::EN, en_word::EN, en_pdf::EN, en_doc::EN, en_misc::EN, en_review::EN, en_incr::EN, en_odtrev::EN, en_autocorrect::EN, en_annot2::EN, en_menus::EN, en_git::EN, en_templates::EN, en_track2::EN, en_csv::EN, en_lsp::EN]
}

/// Översätter en svensk text. Saknas översättningen visas svenskan.
pub fn t(sv: &'static str) -> &'static str {
    match lang() {
        Lang::Sv => sv,
        Lang::En => EN.get(sv).copied().unwrap_or(sv),
    }
}

/// Texten på ett bestämt språk, oavsett aktuellt språk (kommandopaletten söker i båda).
pub fn translate(sv: &'static str, lang: Lang) -> &'static str {
    match lang {
        Lang::Sv => sv,
        Lang::En => EN.get(sv).copied().unwrap_or(sv),
    }
}

/// Finns en engelsk översättning av den svenska texten?
#[allow(dead_code)]
pub fn has_en(sv: &str) -> bool {
    EN.contains_key(sv)
}

/// Översätter mallen och byter ut `{namn}` mot värdena. Se `tf!`.
pub fn tf(template: &'static str, args: &[(&str, String)]) -> String {
    subst(t(template), args)
}

fn subst(tpl: &str, args: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(tpl.len() + 16);
    let mut rest = tpl;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let c = rest.as_bytes()[i];
        let after = &rest[i + 1..];
        if after.as_bytes().first() == Some(&c) {
            out.push(c as char);
            rest = &after[1..];
        } else if c == b'{'
            && let Some(end) = after.find('}')
            && let Some((_, v)) = args.iter().find(|(k, _)| *k == &after[..end])
        {
            out.push_str(v);
            rest = &after[end + 1..];
        } else {
            out.push(c as char);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// `tf!("Sparade {namn}", namn = x)`: som `format!` men med översättning och bara namngivna argument.
#[macro_export]
macro_rules! tf {
    ($tpl:literal $(, $k:ident = $v:expr)* $(,)?) => {
        $crate::i18n::tf($tpl, &[$((stringify!($k), ($v).to_string())),*])
    };
}

// Datum och tal.

/// Formaterar ett tal med `dec` decimaler och språkets decimaltecken (komma / punkt).
#[allow(dead_code)]
pub fn num(v: f64, dec: usize) -> String {
    let s = format!("{v:.dec$}");
    if lang() == Lang::En { s } else { s.replace('.', ",") }
}

/// Klockslag: 24-timmars på svenska, 12-timmars med AM/PM på engelska.
#[allow(dead_code)]
pub fn clock(h: u32, m: u32) -> String {
    match lang() {
        Lang::Sv => format!("{h:02}:{m:02}"),
        Lang::En => format!("{}:{m:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" }),
    }
}

/// Dagens datum med klockslag i språkets stil: "i dag 13:05" / "today 1:05 PM".
#[allow(dead_code)]
pub fn today_at(h: u32, m: u32) -> String {
    match lang() {
        Lang::Sv => format!("i dag {}", clock(h, m)),
        Lang::En => format!("today {}", clock(h, m)),
    }
}

/// Datum: svenska ISO "2026-11-13", engelska "Nov 13, 2026".
#[allow(dead_code)]
pub fn date(y: i32, mo: u32, d: u32) -> String {
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    match lang() {
        Lang::Sv => format!("{y:04}-{mo:02}-{d:02}"),
        Lang::En => format!("{} {d}, {y}", M[(mo.clamp(1, 12) - 1) as usize]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut v = Vec::new();
        // `${namn}` (regex-grupp i exempeltext) är ingen platshållare.
        let mut rest = s.replace("{{", "").replace("}}", "").replace("${", "$(");
        while let Some(i) = rest.find('{') {
            let Some(j) = rest[i..].find('}') else { break };
            v.push(rest[i + 1..i + j].to_owned());
            rest = rest[i + j + 1..].to_owned();
        }
        v.sort();
        v
    }

    #[test]
    fn tables_are_consistent() {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for &tab in tables() {
            for &(sv, en) in tab {
                if let Some(prev) = seen.insert(sv, en) {
                    assert_eq!(prev, en, "dubblett med olika översättning: {sv:?}");
                }
                assert_eq!(placeholders(sv), placeholders(en), "platshållare skiljer: {sv:?} / {en:?}");
                assert!(!en.is_empty(), "tom översättning: {sv:?}");
            }
        }
    }

    #[test]
    fn substitution() {
        let a = [("n", "3".to_owned()), ("x", "ab".to_owned())];
        assert_eq!(subst("{n} av {x}", &a), "3 av ab");
        assert_eq!(subst("{{n}} {n}", &a), "{n} 3");
        assert_eq!(subst("okänd {q}", &a), "okänd {q}");
    }

    #[test]
    fn locale_mapping() {
        assert_eq!(lang_from_locale("sv-SE"), Lang::Sv);
        assert_eq!(lang_from_locale("sv_FI.UTF-8"), Lang::Sv);
        assert_eq!(lang_from_locale("en-US"), Lang::En);
        assert_eq!(lang_from_locale("nb-NO"), Lang::En);
    }
}
