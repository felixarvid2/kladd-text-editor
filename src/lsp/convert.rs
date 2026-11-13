//! Tolkning av serverns JSON: förmågor, diagnostik, kompletteringar, hover, signaturhjälp, platser och
//! `WorkspaceEdit`. Inga egna typer från `lsp-types`: bara de fält Kladd använder plockas ut, och
//! fel eller saknade fält ger standardvärden (en server som skickar något oväntat får aldrig Kladd att krascha).

use std::path::PathBuf;

use serde_json::Value;

use super::pos::{Edit, LineIndex, Remap, uri_to_path};

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn u32_of(v: &Value, k: &str) -> u32 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0).min(u32::MAX as u64) as u32
}

// ------------------------------------------------------------------ förmågor

/// Det servern kan, ur svaret på `initialize`.
#[derive(Clone, Debug, Default)]
pub struct Caps {
    /// 0 = ingen synkning, 1 = hel text, 2 = inkrementell.
    pub sync_kind: u8,
    pub save: bool,
    pub save_text: bool,
    pub completion: bool,
    pub completion_triggers: Vec<char>,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub rename: bool,
    pub prepare_rename: bool,
    pub formatting: bool,
    pub signature: bool,
    pub signature_triggers: Vec<char>,
}

fn has(caps: &Value, k: &str) -> bool {
    match caps.get(k) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(_) => true,
    }
}

fn chars_of(v: Option<&Value>) -> Vec<char> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter_map(|s| s.chars().next()).collect())
        .unwrap_or_default()
}

pub fn parse_caps(init: &Value) -> Caps {
    let c = init.get("capabilities").unwrap_or(&Value::Null);
    let sync = c.get("textDocumentSync");
    let (sync_kind, save, save_text) = match sync {
        Some(Value::Number(n)) => (n.as_u64().unwrap_or(0) as u8, false, false),
        Some(o @ Value::Object(_)) => {
            let save = o.get("save");
            (
                u32_of(o, "change") as u8,
                save.is_some_and(|s| !matches!(s, Value::Bool(false) | Value::Null)),
                save.and_then(|s| s.get("includeText")).and_then(Value::as_bool).unwrap_or(false),
            )
        }
        _ => (0, false, false),
    };
    Caps {
        sync_kind,
        save,
        save_text,
        completion: has(c, "completionProvider"),
        completion_triggers: chars_of(c.get("completionProvider").and_then(|p| p.get("triggerCharacters"))),
        hover: has(c, "hoverProvider"),
        definition: has(c, "definitionProvider"),
        references: has(c, "referencesProvider"),
        rename: has(c, "renameProvider"),
        prepare_rename: c.get("renameProvider").and_then(|r| r.get("prepareProvider")).and_then(Value::as_bool).unwrap_or(false),
        formatting: has(c, "documentFormattingProvider"),
        signature: has(c, "signatureHelpProvider"),
        signature_triggers: chars_of(c.get("signatureHelpProvider").and_then(|p| p.get("triggerCharacters"))),
    }
}

// ------------------------------------------------------------------ diagnostik

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    fn from_lsp(n: u64) -> Self {
        match n {
            2 => Severity::Warning,
            3 => Severity::Info,
            4 => Severity::Hint,
            _ => Severity::Error,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
    /// Serverns position (rad, UTF-16-kolumn).
    pub start: (u32, u32),
    pub end: (u32, u32),
    /// Teckenintervall i den öppna filen; flyttas när texten ändras. `None` för filer som inte är öppna.
    pub range: Option<(usize, usize)>,
}

/// Tolkar `params.diagnostics` i `publishDiagnostics`. `text`: filens text om den är öppen (för teckenintervallen).
pub fn parse_diagnostics(params: &Value, text: Option<&str>) -> Vec<Diag> {
    let idx = text.map(LineIndex::new);
    let mut out: Vec<Diag> = params
        .get("diagnostics")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| {
                    let r = d.get("range")?;
                    let pos = |p: &Value| (u32_of(p, "line"), u32_of(p, "character"));
                    let (start, end) = (pos(r.get("start")?), pos(r.get("end")?));
                    let code = d.get("code").and_then(|c| match c {
                        Value::String(s) => Some(s.clone()),
                        Value::Number(n) => Some(n.to_string()),
                        _ => None,
                    });
                    let mut dg = Diag {
                        severity: Severity::from_lsp(d.get("severity").and_then(Value::as_u64).unwrap_or(1)),
                        message: str_of(d, "message").unwrap_or("").to_owned(),
                        source: str_of(d, "source").map(str::to_owned),
                        code,
                        start,
                        end,
                        range: None,
                    };
                    if let (Some(i), Some(t)) = (&idx, text) {
                        dg.range = Some(char_range(i, t, start, end));
                    }
                    Some(dg)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|d| (d.start, d.severity));
    out
}

pub fn char_range(idx: &LineIndex, text: &str, start: (u32, u32), end: (u32, u32)) -> (usize, usize) {
    let a = idx.pos_to_char(text, start.0, start.1);
    (a, idx.pos_to_char(text, end.0, end.1).max(a))
}

/// Flyttar teckenintervallen efter en textändring.
pub fn shift_ranges(diags: &mut [Diag], r: &Remap) {
    for d in diags {
        if let Some((a, b)) = d.range {
            d.range = Some((r.map(a), r.map(b)));
        }
    }
}

// ------------------------------------------------------------------ komplettering

#[derive(Clone, Debug, Default)]
pub struct CompItem {
    pub label: String,
    pub kind: u8,
    pub detail: String,
    pub filter: String,
    pub sort: String,
    /// Text som ersätter `range` (utan `textEdit` ersätts ordet vid markören).
    pub new_text: String,
    pub range: Option<((u32, u32), (u32, u32))>,
    pub snippet: bool,
    pub additional: Vec<Edit>,
}

pub fn parse_edit(e: &Value) -> Option<Edit> {
    let r = e.get("range")?;
    let pos = |p: &Value| (u32_of(p, "line"), u32_of(p, "character"));
    Some(Edit { start: pos(r.get("start")?), end: pos(r.get("end")?), new_text: str_of(e, "newText")?.to_owned() })
}

/// Tolkar svaret på `textDocument/completion` (lista eller `CompletionList`). Ger objekten och om listan är ofullständig.
pub fn parse_completion(v: &Value) -> (Vec<CompItem>, bool) {
    let (items, incomplete) = match v {
        Value::Array(a) => (Some(a), false),
        Value::Object(_) => (v.get("items").and_then(Value::as_array), v.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false)),
        _ => (None, false),
    };
    let list = items
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    let label = str_of(i, "label")?.to_owned();
                    let te = i.get("textEdit");
                    // `InsertReplaceEdit` har `insert` och `replace`: infoga, som de flesta editorer.
                    let (range, edit_text) = match te {
                        Some(t) => {
                            let r = t.get("range").or_else(|| t.get("insert"));
                            let pos = |p: &Value| (u32_of(p, "line"), u32_of(p, "character"));
                            let range = r.and_then(|r| Some((pos(r.get("start")?), pos(r.get("end")?))));
                            (range, str_of(t, "newText").map(str::to_owned))
                        }
                        None => (None, None),
                    };
                    let new_text = edit_text.or_else(|| str_of(i, "insertText").map(str::to_owned)).unwrap_or_else(|| label.clone());
                    let detail = str_of(i, "detail")
                        .map(str::to_owned)
                        .or_else(|| i.get("labelDetails").and_then(|d| str_of(d, "description")).map(str::to_owned))
                        .unwrap_or_default();
                    Some(CompItem {
                        filter: str_of(i, "filterText").unwrap_or(&label).to_owned(),
                        sort: str_of(i, "sortText").unwrap_or(&label).to_owned(),
                        kind: u32_of(i, "kind") as u8,
                        detail,
                        new_text,
                        range,
                        snippet: u32_of(i, "insertTextFormat") == 2,
                        additional: i
                            .get("additionalTextEdits")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(parse_edit).collect())
                            .unwrap_or_default(),
                        label,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (list, incomplete)
}

/// Poäng för hur väl `prefix` passar `item` (högre är bättre, `None` = passar inte): början av
/// texten väger tyngst, sedan början av ett ord, sedan utspridda tecken i ordning.
pub fn match_score(prefix: &str, item: &str) -> Option<i32> {
    if prefix.is_empty() {
        return Some(0);
    }
    let (p, s): (Vec<char>, Vec<char>) = (prefix.to_lowercase().chars().collect(), item.to_lowercase().chars().collect());
    if s.starts_with(&p) {
        return Some(1000 - (s.len() - p.len()).min(500) as i32 + if item.starts_with(prefix) { 100 } else { 0 });
    }
    let mut score = 0;
    let mut at = 0;
    let mut prev: Option<usize> = None;
    for &c in &p {
        let off = s[at..].iter().position(|&x| x == c)?;
        let i = at + off;
        score += match prev {
            Some(pv) if i == pv + 1 => 8,
            _ if i == 0 || !s[i - 1].is_alphanumeric() => 6,
            _ => 1,
        };
        prev = Some(i);
        at = i + 1;
    }
    Some(score)
}

// ------------------------------------------------------------------ hover och signatur

/// Hovertexten som Markdown, och intervallet om servern gav ett.
pub fn parse_hover(v: &Value) -> Option<(String, Option<((u32, u32), (u32, u32))>)> {
    fn one(c: &Value) -> String {
        match c {
            Value::String(s) => s.clone(),
            // `MarkedString { language, value }` eller `MarkupContent { kind, value }`.
            Value::Object(_) => {
                let val = str_of(c, "value").unwrap_or("");
                match str_of(c, "language") {
                    Some(l) => format!("```{l}\n{val}\n```"),
                    None => val.to_owned(),
                }
            }
            _ => String::new(),
        }
    }
    let contents = v.get("contents")?;
    let md = match contents {
        Value::Array(a) => a.iter().map(one).filter(|s| !s.trim().is_empty()).collect::<Vec<_>>().join("\n\n---\n\n"),
        c => one(c),
    };
    if md.trim().is_empty() {
        return None;
    }
    let range = v.get("range").and_then(|r| {
        let pos = |p: &Value| (u32_of(p, "line"), u32_of(p, "character"));
        Some((pos(r.get("start")?), pos(r.get("end")?)))
    });
    Some((md, range))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    /// Parametrarna som teckenintervall i `label`.
    pub params: Vec<(usize, usize)>,
    pub active: Option<usize>,
    pub doc: String,
}

fn doc_text(d: Option<&Value>) -> String {
    match d {
        Some(Value::String(s)) => s.clone(),
        Some(o @ Value::Object(_)) => str_of(o, "value").unwrap_or("").to_owned(),
        _ => String::new(),
    }
}

pub fn parse_signature(v: &Value) -> Option<Signature> {
    let sigs = v.get("signatures")?.as_array()?;
    let i = (u32_of(v, "activeSignature") as usize).min(sigs.len().saturating_sub(1));
    let s = sigs.get(i)?;
    let label = str_of(s, "label")?.to_owned();
    let chars: Vec<char> = label.chars().collect();
    let params = s
        .get("parameters")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| match p.get("label")? {
                    // `[start, slut]` i UTF-16-enheter av etiketten.
                    Value::Array(r) => {
                        let (a, b) = (r.first()?.as_u64()? as usize, r.get(1)?.as_u64()? as usize);
                        let u16_to_char = |u: usize| {
                            let mut n = 0;
                            for (ci, c) in label.chars().enumerate() {
                                if n >= u {
                                    return ci;
                                }
                                n += c.len_utf16();
                            }
                            chars.len()
                        };
                        Some((u16_to_char(a), u16_to_char(b)))
                    }
                    Value::String(t) => {
                        let at = label.find(t.as_str())?;
                        let a = label[..at].chars().count();
                        Some((a, a + t.chars().count()))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let active = s.get("activeParameter").or_else(|| v.get("activeParameter")).and_then(Value::as_u64).map(|n| n as usize);
    Some(Signature { label, params, active, doc: doc_text(s.get("documentation")) })
}

// ------------------------------------------------------------------ platser och WorkspaceEdit

/// En plats i en fil (serverns positioner).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub start: (u32, u32),
    pub end: (u32, u32),
}

/// Tolkar `Location`, `Location[]` och `LocationLink[]`.
pub fn parse_locations(v: &Value) -> Vec<Location> {
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![v],
        _ => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|l| {
            let (uri, r) = match (str_of(l, "uri"), str_of(l, "targetUri")) {
                (Some(u), _) => (u, l.get("range")?),
                (None, Some(u)) => (u, l.get("targetSelectionRange").or_else(|| l.get("targetRange"))?),
                _ => return None,
            };
            let pos = |p: &Value| (u32_of(p, "line"), u32_of(p, "character"));
            Some(Location { path: uri_to_path(uri)?, start: pos(r.get("start")?), end: pos(r.get("end")?) })
        })
        .collect()
}

/// Ändringar per fil ur en `WorkspaceEdit` (`changes` eller `documentChanges`). Ger också antalet
/// filåtgärder (skapa, byt namn, ta bort) som Kladd inte utför.
pub fn parse_workspace_edit(v: &Value) -> (Vec<(PathBuf, Vec<Edit>)>, usize) {
    let mut files: Vec<(PathBuf, Vec<Edit>)> = Vec::new();
    let mut skipped = 0;
    let mut add = |uri: &str, edits: &Value| {
        let (Some(path), Some(a)) = (uri_to_path(uri), edits.as_array()) else { return };
        let list: Vec<Edit> = a.iter().filter_map(parse_edit).collect();
        match files.iter_mut().find(|(p, _)| *p == path) {
            Some((_, e)) => e.extend(list),
            None => files.push((path, list)),
        }
    };
    if let Some(Value::Array(dc)) = v.get("documentChanges") {
        for c in dc {
            if let Some(td) = c.get("textDocument") {
                add(str_of(td, "uri").unwrap_or(""), c.get("edits").unwrap_or(&Value::Null));
            } else if c.get("kind").is_some() {
                skipped += 1;
            }
        }
    } else if let Some(Value::Object(m)) = v.get("changes") {
        for (uri, edits) in m {
            add(uri, edits);
        }
    }
    (files, skipped)
}

/// Ordet (bokstäver, siffror, `_`) som omger teckenindex `ci`, som teckenintervall.
pub fn word_at(text: &str, ci: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let is_w = |c: char| c.is_alphanumeric() || c == '_';
    let ci = ci.min(chars.len());
    let mut a = ci;
    let mut b = ci;
    while a > 0 && is_w(chars[a - 1]) {
        a -= 1;
    }
    while b < chars.len() && is_w(chars[b]) {
        b += 1;
    }
    (a < b).then_some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn caps_from_initialize() {
        let c = parse_caps(&json!({"capabilities": {
            "textDocumentSync": {"change": 2, "save": {"includeText": true}},
            "completionProvider": {"triggerCharacters": [".", ":"]},
            "hoverProvider": true,
            "renameProvider": {"prepareProvider": true},
            "documentFormattingProvider": false,
            "signatureHelpProvider": {"triggerCharacters": ["(", ","]}
        }}));
        assert_eq!((c.sync_kind, c.save, c.save_text), (2, true, true));
        assert_eq!(c.completion_triggers, ['.', ':']);
        assert!(c.hover && c.completion && c.rename && c.prepare_rename && c.signature);
        assert!(!c.formatting && !c.definition);
        assert_eq!(c.signature_triggers, ['(', ',']);
        assert_eq!(parse_caps(&json!({"capabilities": {"textDocumentSync": 1}})).sync_kind, 1);
        assert_eq!(parse_caps(&Value::Null).sync_kind, 0);
    }

    #[test]
    fn diagnostics_mapping() {
        let text = "let ä = \"😀\"; x\nrad två\n";
        let p = json!({"uri": "file:///x", "diagnostics": [
            {"range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 7}},
             "severity": 2, "message": "varning", "source": "rustc", "code": "W1"},
            {"range": {"start": {"line": 0, "character": 14}, "end": {"line": 0, "character": 15}},
             "severity": 1, "message": "fel", "code": 308},
            {"range": {"start": {"line": 0}}, "message": "trasig"},
            {"severity": 9, "message": "ingen range"}
        ]});
        let d = parse_diagnostics(&p, Some(text));
        assert_eq!(d.len(), 2);
        // Sorterade efter plats: felet på rad 0 först. Emojin är två UTF-16-enheter, ett tecken.
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].code.as_deref(), Some("308"));
        let (a, b) = d[0].range.unwrap();
        assert_eq!(text.chars().skip(a).take(b - a).collect::<String>(), "x");
        let (a, b) = d[1].range.unwrap();
        assert_eq!(text.chars().skip(a).take(b - a).collect::<String>(), "två");
        assert_eq!((d[1].severity, d[1].source.as_deref()), (Severity::Warning, Some("rustc")));
        // Utan text (filen är inte öppen): bara serverns positioner.
        assert!(parse_diagnostics(&p, None).iter().all(|d| d.range.is_none()));
        assert!(parse_diagnostics(&json!({}), Some(text)).is_empty());
    }

    #[test]
    fn severity_order_and_ranges_shift() {
        assert!(Severity::Error < Severity::Warning && Severity::Warning < Severity::Hint);
        let mk = |a, b| Diag { severity: Severity::Error, message: String::new(), source: None, code: None, start: (0, 0), end: (0, 0), range: Some((a, b)) };
        let mut v = vec![mk(0, 2), mk(5, 8), mk(3, 6), mk(10, 12)];
        // Tecknen 4..5 ersätts av tre tecken.
        shift_ranges(&mut v, &Remap::new("0123456789abcdef", "0123xyz56789abcdef"));
        let r: Vec<_> = v.iter().map(|d| d.range.unwrap()).collect();
        assert_eq!(r, [(0, 2), (7, 10), (3, 8), (12, 14)]);
    }

    #[test]
    fn completion_parsing() {
        let v = json!({"isIncomplete": true, "items": [
            {"label": "push", "kind": 2, "detail": "fn(&mut self)", "insertText": "push($0)", "insertTextFormat": 2,
             "textEdit": {"range": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 6}}, "newText": "push($1)"},
             "additionalTextEdits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "use a;\n"}],
             "filterText": "push", "sortText": "0001"},
            {"label": "len"},
            {"kind": 3}
        ]});
        let (items, inc) = parse_completion(&v);
        assert!(inc);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].new_text, "push($1)");
        assert!(items[0].snippet);
        assert_eq!(items[0].range, Some(((3, 4), (3, 6))));
        assert_eq!(items[0].additional.len(), 1);
        assert_eq!((items[1].new_text.as_str(), items[1].filter.as_str(), items[1].range), ("len", "len", None));
        // InsertReplaceEdit: infoga-intervallet.
        let v = json!([{"label": "x", "textEdit": {"insert": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 2}},
            "replace": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 5}}, "newText": "xy"}}]);
        assert_eq!(parse_completion(&v).0[0].range, Some(((0, 1), (0, 2))));
        assert!(parse_completion(&Value::Null).0.is_empty());
    }

    #[test]
    fn scoring() {
        assert!(match_score("pu", "push").unwrap() > match_score("pu", "compute").unwrap());
        assert!(match_score("PU", "push").is_some());
        assert!(match_score("zz", "push").is_none());
        // Prefix slår utspritt, ordgräns slår mitt i ord.
        assert!(match_score("ab", "abc").unwrap() > match_score("ab", "a_b").unwrap());
        assert!(match_score("gb", "get_buf").unwrap() > match_score("gb", "dgxb").unwrap());
        assert_eq!(match_score("", "x"), Some(0));
        // Exakt versalskiftläge gynnas.
        assert!(match_score("Vec", "Vec").unwrap() > match_score("Vec", "vec").unwrap());
    }

    #[test]
    fn hover_forms() {
        let (md, r) = parse_hover(&json!({"contents": {"kind": "markdown", "value": "```rust\nfn x()\n```"},
            "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 3}}}))
        .unwrap();
        assert!(md.contains("fn x()"));
        assert_eq!(r, Some(((1, 2), (1, 3))));
        let (md, _) = parse_hover(&json!({"contents": ["text", {"language": "py", "value": "def f(): ..."}]})).unwrap();
        assert!(md.contains("text") && md.contains("```py\ndef f(): ...\n```"));
        assert!(parse_hover(&json!({"contents": ""})).is_none());
        assert!(parse_hover(&Value::Null).is_none());
    }

    #[test]
    fn signature_params() {
        let s = parse_signature(&json!({"signatures": [{"label": "fn f(å: u8, b: 😀)",
            "parameters": [{"label": [5, 10]}, {"label": "b: 😀"}], "documentation": {"kind": "markdown", "value": "doc"}}],
            "activeParameter": 1}))
        .unwrap();
        let chars: Vec<char> = s.label.chars().collect();
        let part = |(a, b): (usize, usize)| chars[a..b].iter().collect::<String>();
        assert_eq!(part(s.params[0]), "å: u8");
        assert_eq!(part(s.params[1]), "b: 😀");
        assert_eq!((s.active, s.doc.as_str()), (Some(1), "doc"));
        assert!(parse_signature(&json!({"signatures": []})).is_none());
    }

    #[test]
    fn locations_and_edits() {
        let l = parse_locations(&json!([
            {"uri": "file:///a/b.rs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 4}}},
            {"targetUri": "file:///c.rs", "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
             "targetSelectionRange": {"start": {"line": 3, "character": 1}, "end": {"line": 3, "character": 2}}},
            {"uri": "http://nej", "range": {}}
        ]));
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].start, (3, 1));
        assert_eq!(parse_locations(&json!({"uri": "file:///z", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}})).len(), 1);

        let e = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///a.rs", "version": 1}, "edits": [
                {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "x"}]},
            {"textDocument": {"uri": "file:///a.rs"}, "edits": [
                {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}, "newText": "y"}]},
            {"kind": "rename", "oldUri": "file:///a.rs", "newUri": "file:///b.rs"}
        ]});
        let (files, skipped) = parse_workspace_edit(&e);
        assert_eq!(skipped, 1);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1.len(), 2);
        let (files, _) = parse_workspace_edit(&json!({"changes": {"file:///q.rs": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "z"}]}}));
        assert_eq!(files[0].0, PathBuf::from("/q.rs"));
    }

    #[test]
    fn words() {
        assert_eq!(word_at("let åäö_1 = 2;", 6), Some((4, 9)));
        assert_eq!(word_at("let åäö_1 = 2;", 9), Some((4, 9)));
        assert_eq!(word_at("a  b", 2), None);
        assert_eq!(word_at("", 0), None);
    }
}
