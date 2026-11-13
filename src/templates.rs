//! Mallar för nya dokument: de inbyggda (`assets/templates/`), egna mallar (`mallar/` i datakatalogen),
//! platshållare `{{namn}}` och import av .docx/.odt som mall. Ren logik utan gränssnitt; dialogen ligger i
//! `templates_ui.rs`.
//!
//! En mall är Kladds Markdown-dialekt (grundformatsraden `<!-- kladd: … -->`, avsnitt, tabeller,
//! `<!-- kladd: toc -->`, se `docs/dev/dokumentlage.md`) med en första rad som beskriver mallen:
//!
//! ```text
//! <!-- mall; kategori: brev; läge: dokument; namn: Formellt brev; beskrivning: …; skapad: 2026-11-13 -->
//! ```
//!
//! `läge` är `dokument` (öppnas i dokumentläget, sparas som .docx) eller `text` (Markdown i textvyn).
//! Värdena får inte innehålla `;` eller `-->`. De inbyggda mallarna finns på svenska och engelska
//! (språket följer gränssnittet) med samma id; egna mallar har inget språk.

use std::path::{Path, PathBuf};

use crate::i18n::{self, Lang, t};

/// Raden som beskriver mallen börjar med `<!-- ` och det här ordet.
const META_KEY: &str = "mall";
/// Platshållarens bakgrund för de som lämnats tomma i dokumentläget (Words gula markering).
const HIGHLIGHT: [u8; 3] = [0xFF, 0xFF, 0x00];
/// Längsta namnet på en platshållare.
const MAX_KEY: usize = 40;
/// Längsta namnet på en egen mall (tecken).
const MAX_NAME: usize = 80;

/// Mallens kategori i galleriet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Letter,
    Report,
    Minutes,
    Agenda,
    Cv,
    Invoice,
    Essay,
    Notes,
    Readme,
    Blank,
    /// Egna mallar.
    Mine,
}

impl Category {
    /// Kategorierna som filtrerar inbyggda mallar, i galleriets ordning.
    pub const BUILTIN: [Category; 10] = [
        Category::Letter,
        Category::Report,
        Category::Minutes,
        Category::Agenda,
        Category::Cv,
        Category::Invoice,
        Category::Essay,
        Category::Notes,
        Category::Readme,
        Category::Blank,
    ];

    fn from_key(k: &str) -> Option<Category> {
        Some(match k {
            "brev" => Category::Letter,
            "rapport" => Category::Report,
            "protokoll" => Category::Minutes,
            "dagordning" => Category::Agenda,
            "cv" => Category::Cv,
            "faktura" => Category::Invoice,
            "uppsats" => Category::Essay,
            "anteckningar" => Category::Notes,
            "readme" => Category::Readme,
            "tom" => Category::Blank,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Letter => t("Brev"),
            Category::Report => t("Rapport"),
            Category::Minutes => t("Mötesprotokoll"),
            Category::Agenda => t("Dagordning"),
            Category::Cv => t("CV"),
            Category::Invoice => t("Faktura"),
            Category::Essay => t("Uppsats och PM"),
            Category::Notes => t("Anteckningar"),
            Category::Readme => t("README"),
            Category::Blank => t("Tom"),
            Category::Mine => t("Mina mallar"),
        }
    }
}

/// En mall med metadata och brödtext (utan beskrivningsraden).
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// Inbyggd: id (filnamnet i `assets/templates`). Egen: filnamnet utan `.md`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: Category,
    /// Dokumentläge (docx-lik) eller Markdown i textvyn.
    pub doc: bool,
    pub body: String,
    pub user: bool,
}

/// Läser beskrivningsraden. Returnerar nyckel/värde-paren och resten av texten (utan raden och
/// radbrytningen efter den). Saknas raden är paren tomma och hela texten brödtext.
pub fn split_meta(src: &str) -> (Vec<(String, String)>, &str) {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let (first, rest) = match src.split_once('\n') {
        Some((a, b)) => (a, b),
        None => (src, ""),
    };
    let line = first.trim();
    let inner = line
        .strip_prefix("<!--")
        .and_then(|l| l.strip_suffix("-->"))
        .map(str::trim)
        .and_then(|l| l.strip_prefix(META_KEY));
    let Some(inner) = inner.filter(|r| r.is_empty() || r.starts_with(';') || r.starts_with(char::is_whitespace)) else {
        return (Vec::new(), src);
    };
    let pairs = inner
        .split(';')
        .filter_map(|p| {
            let (k, v) = p.split_once(':')?;
            Some((k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect();
    (pairs, rest)
}

/// Beskrivningsraden för en egen mall.
fn meta_line(name: &str, doc: bool) -> String {
    let clean = |s: &str| s.replace([';', '\n', '\r'], ",").replace("-->", "—>");
    format!(
        "<!-- {META_KEY}; namn: {}; läge: {}; skapad: {} -->",
        clean(name),
        if doc { "dokument" } else { "text" },
        crate::style::today()
    )
}

/// Tolkar en mallfil. `id` blir mallens id; `user` sätter egen mall.
pub fn parse(id: &str, src: &str, user: bool) -> Template {
    let (meta, body) = split_meta(src);
    let get = |k: &str| meta.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    let category = if user { Category::Mine } else { get("kategori").and_then(|k| Category::from_key(&k)).unwrap_or(Category::Blank) };
    Template {
        id: id.to_owned(),
        name: get("namn").filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_owned()),
        description: get("beskrivning").unwrap_or_default(),
        category,
        doc: get("läge").is_none_or(|m| m != "text"),
        body: body.to_owned(),
        user,
    }
}

macro_rules! builtin {
    ($id:literal) => {
        (
            $id,
            include_str!(concat!("../assets/templates/sv/", $id, ".md")),
            include_str!(concat!("../assets/templates/en/", $id, ".md")),
        )
    };
}

/// Inbyggda mallar: (id, svenska, engelska).
const BUILTIN: [(&str, &str, &str); 13] = [
    builtin!("brev-formellt"),
    builtin!("brev-personligt"),
    builtin!("rapport"),
    builtin!("motesprotokoll"),
    builtin!("dagordning"),
    builtin!("cv-klassiskt"),
    builtin!("cv-modernt"),
    builtin!("faktura"),
    builtin!("uppsats"),
    builtin!("anteckningar"),
    builtin!("readme"),
    builtin!("tom-a4"),
    builtin!("tom-letter"),
];

/// De inbyggda mallarna på språket `lang`.
pub fn builtins(lang: Lang) -> Vec<Template> {
    BUILTIN.iter().map(|&(id, sv, en)| parse(id, if lang == Lang::En { en } else { sv }, false)).collect()
}

// ---------------------------------------------------------------- platshållare

/// Giltigt namn på en platshållare: bokstäver, siffror, `-` och `_`.
fn valid_key(k: &str) -> bool {
    !k.is_empty() && k.chars().count() <= MAX_KEY && k.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Platshållarna `{{namn}}` i texten: (byteintervall för hela `{{…}}`, namnet).
fn find_placeholders(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find("{{") {
        let start = at + i;
        let inner = start + 2;
        match text[inner..].find("}}") {
            Some(j) if valid_key(&text[inner..inner + j]) => {
                out.push((start..inner + j + 2, &text[inner..inner + j]));
                at = inner + j + 2;
            }
            _ => at = start + 1,
        }
    }
    out
}

/// Namnen på platshållarna, en gång var, i den ordning de först förekommer.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (_, k) in find_placeholders(text) {
        if !keys.iter().any(|x| x == k) {
            keys.push(k.to_owned());
        }
    }
    keys
}

/// Fältets namn i formuläret: nyckeln med stor bokstav och mellanslag i stället för `-` och `_`.
pub fn label(key: &str) -> String {
    // `e-post` behåller sitt bindestreck (en enda bokstav före det); annars blir `-` och `_` mellanslag.
    let spaced = if key.split_once('-').is_some_and(|(a, _)| a.chars().count() == 1) {
        key.replace('_', " ")
    } else {
        key.replace(['-', '_'], " ")
    };
    let mut c = spaced.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Är det datumet (`datum`/`date`)?
fn is_date_key(k: &str) -> bool {
    matches!(k.to_lowercase().as_str(), "datum" | "date")
}

/// Är det skribentens namn?
fn is_author_key(k: &str) -> bool {
    matches!(k.to_lowercase().as_str(), "namn" | "name" | "författare" | "author" | "avsändare" | "sender")
}

/// Dagens datum i gränssnittsspråkets format.
pub fn today_text() -> String {
    use chrono::Datelike;
    let d = chrono::Local::now();
    i18n::date(d.year(), d.month(), d.day())
}

/// Förifyllda värden: dagens datum för `datum`, författarnamnet för `namn`, annars tomt.
pub fn prefill(keys: &[String], author: &str) -> Vec<String> {
    let today = today_text();
    keys.iter()
        .map(|k| {
            if is_date_key(k) {
                today.clone()
            } else if is_author_key(k) {
                author.trim().to_owned()
            } else {
                String::new()
            }
        })
        .collect()
}

/// Byter ut `{{nyckel}}` mot värdena. Tomma värden (och nycklar utan värde) lämnas kvar som
/// platshållare; med `highlight` markeras de gula, utom på kommentarsrader (`<!-- … -->`) där
/// ingen markering får plats. Värden med radbrytning blir en rad; `|` i en tabellrad skyddas.
pub fn fill(body: &str, values: &[(String, String)], highlight: bool) -> String {
    let mut out = String::with_capacity(body.len() + 64);
    for (n, line) in body.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let comment = line.trim_start().starts_with("<!--");
        let table = line.trim_start().starts_with('|');
        let mut last = 0;
        for (range, key) in find_placeholders(line) {
            out.push_str(&line[last..range.start]);
            last = range.end;
            let value = values.iter().find(|(k, _)| k == key).map(|(_, v)| v.trim()).unwrap_or("");
            if value.is_empty() {
                if highlight && !comment {
                    out.push_str(&crate::style::SpanStyle { background: Some(HIGHLIGHT), ..Default::default() }.open_tag());
                    out.push_str(&line[range]);
                    out.push_str("</span>");
                } else {
                    out.push_str(&line[range]);
                }
            } else {
                let v = value.replace(['\r', '\n'], " ");
                out.push_str(&if table { v.replace('|', "\\|") } else { v });
            }
        }
        out.push_str(&line[last..]);
    }
    out
}

/// Texten i det nya dokumentet: mallen med värdena ifyllda.
pub fn create(t: &Template, values: &[(String, String)]) -> String {
    fill(&t.body, values, t.doc)
}

// ---------------------------------------------------------------- egna mallar

/// Katalogen med egna mallar.
pub fn user_dir() -> PathBuf {
    crate::platform::data_dir().join("mallar")
}

/// Ett namn som går att använda som filnamn: tecken som är förbjudna i filnamn byts mot `-`.
/// `None` om inget återstår.
pub fn sanitize_name(name: &str) -> Option<String> {
    let s: String = name
        .trim()
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '-' } else { c })
        .collect();
    let s = s.trim().trim_start_matches('.').trim().chars().take(MAX_NAME).collect::<String>();
    let s = s.trim_end().to_owned();
    (!s.is_empty()).then_some(s)
}

fn user_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.md"))
}

/// Egna mallar i `dir`, sorterade på namn.
pub fn list_user(dir: &Path) -> Vec<Template> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Template> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter_map(|p| {
            let id = p.file_stem()?.to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&p).ok()?;
            let mut t = parse(&id, &src, true);
            // Filnamnet är mallens namn: byter man namn på filen i en filhanterare följer det med.
            t.name = id;
            Some(t)
        })
        .collect();
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// Sparar en egen mall och ger dess id. `overwrite` styr om en befintlig mall med samma namn ersätts.
pub fn save_user(dir: &Path, name: &str, text: &str, doc: bool, overwrite: bool) -> Result<String, String> {
    let id = sanitize_name(name).ok_or_else(|| t("Ge mallen ett namn.").to_owned())?;
    let path = user_path(dir, &id);
    if path.exists() && !overwrite {
        return Err(crate::tf!("Det finns redan en mall som heter {name}.", name = id));
    }
    std::fs::create_dir_all(dir).map_err(|e| crate::tf!("Mallen kunde inte sparas: {e}", e = e))?;
    let (_, body) = split_meta(text);
    let src = format!("{}\n{}", meta_line(&id, doc), body);
    std::fs::write(&path, src).map_err(|e| crate::tf!("Mallen kunde inte sparas: {e}", e = e))?;
    Ok(id)
}

/// Läser en egen mall.
#[cfg_attr(not(test), allow(dead_code))]
pub fn load_user(dir: &Path, id: &str) -> Result<Template, String> {
    let src = std::fs::read_to_string(user_path(dir, id)).map_err(|e| crate::tf!("Mallen kunde inte läsas: {e}", e = e))?;
    let mut t = parse(id, &src, true);
    t.name = id.to_owned();
    Ok(t)
}

/// Tar bort en egen mall.
pub fn delete_user(dir: &Path, id: &str) -> Result<(), String> {
    std::fs::remove_file(user_path(dir, id)).map_err(|e| crate::tf!("Mallen kunde inte tas bort: {e}", e = e))
}

/// Byter namn på en egen mall och ger det nya id:t.
pub fn rename_user(dir: &Path, id: &str, new_name: &str) -> Result<String, String> {
    let new_id = sanitize_name(new_name).ok_or_else(|| t("Ge mallen ett namn.").to_owned())?;
    if new_id == id {
        return Ok(new_id);
    }
    let to = user_path(dir, &new_id);
    if to.exists() {
        return Err(crate::tf!("Det finns redan en mall som heter {name}.", name = new_id));
    }
    let src = std::fs::read_to_string(user_path(dir, id)).map_err(|e| crate::tf!("Mallen kunde inte läsas: {e}", e = e))?;
    let (_, body) = split_meta(&src);
    let doc = parse(id, &src, true).doc;
    let out = format!("{}\n{}", meta_line(&new_id, doc), body);
    std::fs::write(&to, out).map_err(|e| crate::tf!("Mallen kunde inte sparas: {e}", e = e))?;
    std::fs::remove_file(user_path(dir, id)).map_err(|e| crate::tf!("Mallen kunde inte tas bort: {e}", e = e))?;
    Ok(new_id)
}

// ---------------------------------------------------------------- import och rensning

/// Tar bort det som inte följer med en mall: bilder (`bild:`-referenser pekar på ett paket som inte
/// finns) och osynliga objektmarkörer `<!--k:N-->` (pekar på originalets XML).
pub fn strip_unportable(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    for (n, line) in md.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        out.push_str(&strip_line(line));
    }
    out
}

fn strip_line(line: &str) -> String {
    let mut s = line.to_owned();
    // `<!--k:12-->`
    while let Some(i) = s.find("<!--k:") {
        match s[i..].find("-->") {
            Some(j) => s.replace_range(i..i + j + 3, ""),
            None => break,
        }
    }
    // `![alt](bild:namn.png)` och `<img src="bild:namn.png" …>`
    while let Some(i) = s.find("](bild:") {
        let Some(open) = s[..i].rfind("![") else { break };
        match s[i..].find(')') {
            Some(j) => s.replace_range(open..i + j + 1, ""),
            None => break,
        }
    }
    while let Some(i) = s.find("<img") {
        match s[i..].find('>') {
            Some(j) if s[i..i + j].contains("bild:") => s.replace_range(i..i + j + 1, ""),
            _ => break,
        }
    }
    s
}

/// Läser en .docx eller .odt som en mall: ger (Markdown utan bilder, antal borttagna bilder).
pub fn import_bytes(bytes: &[u8], ext: &str) -> Result<(String, usize), String> {
    let md = match ext.to_lowercase().as_str() {
        "docx" | "dotx" | "docm" => crate::docx::read(bytes)?.markdown,
        "odt" | "ott" => crate::odt::read(bytes)?.markdown,
        _ => return Err(t("Bara .docx och .odt kan importeras som mall.").to_owned()),
    };
    let images = md.matches("bild:").count();
    Ok((strip_unportable(&md), images))
}

/// Filens namn utan ändelse, som förslag på mallens namn.
pub fn name_from_path(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

// ---------------------------------------------------------------- förhandsvisning

/// En rad i miniatyren på kortet i galleriet.
#[derive(Clone, Debug, PartialEq)]
pub enum Sketch {
    /// Rubrik av nivå 1–3 (bredd i delar av raden, 0.0–1.0).
    Heading(u8, f32),
    /// Löptext.
    Line(f32),
    /// Punkt eller numrerad punkt.
    Item(f32),
    /// Tabellrad med så här många kolumner.
    Row(usize),
    /// Luft mellan stycken.
    Gap,
    /// Centrerad rad.
    Centered(f32),
}

/// Sidans bredd delat på höjden för miniatyren (A4 om grundformatet saknas).
pub fn page_aspect(body: &str) -> f32 {
    let d = body.lines().take(4).find_map(crate::style::DocDefaults::from_comment).unwrap_or_default();
    let (w, h) = d.page();
    (w / h).clamp(0.4, 2.5)
}

/// Ritar upp mallen som några rader för miniatyren: högst `max` rader.
pub fn sketch(body: &str, max: usize) -> Vec<Sketch> {
    let mut out: Vec<Sketch> = Vec::new();
    let mut centered = false;
    let mut fence = false;
    for line in body.lines() {
        if out.len() >= max {
            break;
        }
        let t = line.trim();
        if t.starts_with("```") {
            fence = !fence;
            continue;
        }
        if t.is_empty() {
            if !matches!(out.last(), Some(Sketch::Gap) | None) {
                out.push(Sketch::Gap);
            }
            continue;
        }
        if t.starts_with("<!--") {
            continue;
        }
        if t.starts_with("<div") {
            centered = t.contains("center");
            continue;
        }
        if t == "</div>" {
            centered = false;
            continue;
        }
        let w = (t.chars().count() as f32 / 48.0).clamp(0.12, 1.0);
        let level = t.chars().take_while(|&c| c == '#').count();
        out.push(if t.starts_with('|') {
            if t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                continue;
            }
            Sketch::Row(t.matches('|').count().saturating_sub(1).max(1))
        } else if (1..=6).contains(&level) && t[level..].starts_with(' ') {
            Sketch::Heading(level.min(3) as u8, (w * 0.9).clamp(0.2, 0.8))
        } else if t.starts_with("- ") || t.starts_with("* ") || t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains(". ") {
            Sketch::Item(w)
        } else if centered {
            Sketch::Centered(w.min(0.8))
        } else if fence {
            Sketch::Item(w)
        } else {
            Sketch::Line(w)
        });
    }
    while matches!(out.last(), Some(Sketch::Gap)) {
        out.pop();
    }
    out
}

#[cfg(test)]
#[path = "templates_tests.rs"]
mod tests;
