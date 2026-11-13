//! Språkserverklient (LSP) för kodfiler i textläget: felmarkeringar, komplettering, hover,
//! gå till definition, referenser, byt namn, formatera och signaturhjälp.
//!
//! * `proto`: JSON-RPC över stdio med läsartråd, begäran-id, avbrott och tidsgränser.
//! * `pos`: UTF-16-positioner, textändringar, snippets och URI:er (rena funktioner).
//! * `convert`: tolkning av serverns JSON.
//! * `config`: språk, serverkommandon och projektrot.
//! * `ui`: ritning och kopplingen till editorn (`KladdApp`).
//!
//! [`Lsp`] (här) äger servrarna och synkar dokumenten: `tick` anropas varje bild, startar servern
//! första gången en fil av språket är aktiv, skickar `didOpen`/`didChange` (fördröjt ~200 ms, inkrementellt
//! om servern kan)/`didSave`/`didClose` och hämtar svar från pågående begäran utan att blockera.

pub mod config;
pub mod convert;
pub mod pos;
pub mod proto;
mod ui;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::doc::Document;
use crate::tf;
use config::Lang;
use convert::{Caps, Diag};
use pos::{Edit, LineIndex, Remap};
use proto::{Client, Event, Poll, Request, Waker};

/// Så länge texten ska vara oförändrad innan `didChange` skickas.
const DEBOUNCE: Duration = Duration::from_millis(200);
/// Filer större än så här synkas inte (en server blir seg och editorn likaså).
const MAX_BYTES: usize = 3_000_000;
const INIT_TIMEOUT: Duration = Duration::from_secs(90);

/// Inställningar som `tick` behöver.
pub struct Cfg<'a> {
    pub enabled: bool,
    pub commands: &'a [(String, String)],
    pub workspace: Option<&'a Path>,
}

/// Läge hos en server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SrvState {
    Starting,
    Ready,
    Failed(String),
    Exited,
}

struct Prog {
    token: String,
    title: String,
    message: String,
    pct: Option<u32>,
}

pub struct Server {
    lang: &'static Lang,
    root: PathBuf,
    /// Programnamnet (utan sökväg), visas i statusraden.
    name: String,
    state: SrvState,
    client: Option<Client>,
    init: Option<Request>,
    caps: Caps,
    progress: Vec<Prog>,
    /// Ersatt av en nyare server (efter omstart): hittas inte längre av `assign`.
    retired: bool,
}

/// Ett dokument som hör till en server.
struct Tracked {
    server: usize,
    path: PathBuf,
    uri: String,
    lang_id: &'static str,
    version: i32,
    /// Texten som servern senast fick.
    text: Arc<String>,
    opened: bool,
    was_dirty: bool,
    /// Hash av texten när ändringen först sågs, och när (för fördröjningen).
    seen: Option<u64>,
    changed_at: Instant,
}

/// Vad ett pågående anrop ska bli.
pub enum Kind {
    Completion { anchor: usize, manual: bool },
    Hover { at: usize },
    Definition,
    References,
    PrepareRename { at: usize },
    Rename,
    Format,
    Signature,
}

impl Kind {
    /// Anrop som användaren inte väntar på: fel visas inte.
    fn silent(&self) -> bool {
        matches!(self, Kind::Completion { manual: false, .. } | Kind::Hover { .. } | Kind::Signature)
    }
    fn singleton(&self) -> Option<u8> {
        match self {
            Kind::Completion { .. } => Some(0),
            Kind::Hover { .. } => Some(1),
            Kind::Signature => Some(2),
            _ => None,
        }
    }
}

struct Op {
    doc_id: u64,
    server: usize,
    kind: Kind,
    req: Request,
    /// Texten som positionerna i anropet gäller.
    base: Arc<String>,
}

/// Hur ett komplettering utlöstes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trigger {
    Manual,
    Char(char),
    Incomplete,
}

/// Ett hopp till en plats (rad och UTF-16-kolumn i filen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jump {
    pub path: PathBuf,
    pub start: (u32, u32),
    pub end: (u32, u32),
}

/// Ändringar i en fil som ska tillämpas av fönstret.
pub struct FileEdits {
    pub path: PathBuf,
    pub edits: Vec<Edit>,
    /// Texten ändringarna räknades mot; avviker flikens text hoppar fönstret över filen.
    pub expect: Option<Arc<String>>,
}

/// Resultat som fönstret ska agera på (öppna flikar, ändra text, visa meddelanden).
#[derive(Default)]
pub struct Out {
    pub jump: Option<Jump>,
    pub edits: Vec<FileEdits>,
    pub notes: Vec<(String, bool)>,
}

#[derive(Default)]
pub struct Lsp {
    servers: Vec<Server>,
    docs: HashMap<u64, Tracked>,
    /// Diagnostik per fil (alla filer servern rapporterat om).
    pub(crate) diags: HashMap<PathBuf, Vec<Diag>>,
    ops: Vec<Op>,
    pub(crate) ui: ui::State,
    pub(crate) out: Out,
    warned: HashSet<&'static str>,
    wake: Option<Waker>,
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Språk och languageId för ett dokument som kan ha språkserver.
fn eligible(d: &Document) -> Option<(&'static Lang, &'static str)> {
    if d.docx || d.pdf || d.pdfv.is_some() || d.word_view || d.text.len() > MAX_BYTES {
        return None;
    }
    let path = d.path.as_ref()?;
    config::for_extension(path.extension()?.to_str()?)
}

fn pos_json((l, c): (u32, u32)) -> Value {
    json!({"line": l, "character": c})
}

impl Lsp {
    // ------------------------------------------------------------ varje bild

    pub fn tick(&mut self, ctx: &egui::Context, docs: &[Document], active: Option<u64>, cfg: &Cfg) {
        let wake = self
            .wake
            .get_or_insert_with(|| {
                let c = ctx.clone();
                Arc::new(move || c.request_repaint())
            })
            .clone();
        if !cfg.enabled {
            if !self.servers.is_empty() {
                self.shutdown();
            }
            return;
        }
        self.poll_servers(docs);

        // Stängda eller ändrade dokument (flik stängd, "Spara som" till annan fil).
        let gone: Vec<u64> = self
            .docs
            .iter()
            .filter(|(id, t)| match docs.iter().find(|d| d.id == **id) {
                Some(d) => d.path.as_ref() != Some(&t.path) || eligible(d).is_none(),
                None => true,
            })
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.untrack(id);
        }

        for d in docs {
            if self.docs.contains_key(&d.id) || active != Some(d.id) {
                continue;
            }
            if let Some((lang, lang_id)) = eligible(d) {
                self.assign(d, lang, lang_id, cfg, &wake);
            }
        }
        self.open_pending(docs);
        self.sync_docs(ctx, docs);
        self.poll_ops(ctx, docs);
        // Svar från servern kommer via läsartråden, som väcker fönstret; väntande anrop behöver ändå
        // en bild då och då för tidsgränserna.
        if !self.ops.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Kopplar ett dokument till en server (startar den vid behov).
    fn assign(&mut self, d: &Document, lang: &'static Lang, lang_id: &'static str, cfg: &Cfg, wake: &Waker) {
        let Some(path) = d.path.clone() else { return };
        let root = config::project_root(&path, lang, cfg.workspace);
        let server = match self.servers.iter().position(|s| !s.retired && s.lang.key == lang.key && s.root == root) {
            Some(i) => i,
            None => {
                self.servers.push(Self::start_server(lang, root, cfg, wake));
                let i = self.servers.len() - 1;
                if let SrvState::Failed(_) = self.servers[i].state
                    && self.warned.insert(lang.key)
                {
                    let cmd = lang.defaults.first().and_then(|c| c.first()).copied().unwrap_or("");
                    self.out.notes.push((
                        tf!(
                            "Hittar ingen språkserver för {language}. Installera {command} eller ställ in kommandot under Språkservrar.",
                            language = config::lang_name(lang),
                            command = cmd
                        ),
                        true,
                    ));
                }
                i
            }
        };
        self.docs.insert(
            d.id,
            Tracked {
                server,
                uri: pos::path_to_uri(&path),
                path,
                lang_id,
                version: 0,
                text: Arc::new(String::new()),
                opened: false,
                was_dirty: d.dirty,
                seen: None,
                changed_at: Instant::now(),
            },
        );
    }

    fn start_server(lang: &'static Lang, root: PathBuf, cfg: &Cfg, wake: &Waker) -> Server {
        let mut s = Server {
            lang,
            root,
            name: String::new(),
            state: SrvState::Starting,
            client: None,
            init: None,
            caps: Caps::default(),
            progress: Vec::new(),
            retired: false,
        };
        let Some(cmd) = config::command_for(lang, cfg.commands) else {
            s.name = lang.defaults.first().and_then(|c| c.first()).copied().unwrap_or("").to_owned();
            s.state = SrvState::Failed(crate::i18n::t("hittades inte").into());
            return s;
        };
        s.name = Path::new(&cmd[0]).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| cmd[0].clone());
        match Client::spawn(&cmd, &s.root, wake.clone()) {
            Err(e) => s.state = SrvState::Failed(e.to_string()),
            Ok(c) => {
                let uri = pos::path_to_uri(&s.root);
                let name = s.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                s.init = Some(c.request("initialize", init_params(&uri, &name), INIT_TIMEOUT));
                s.client = Some(c);
            }
        }
        s
    }

    /// Hämtar svar på `initialize` och notiser från varje server.
    fn poll_servers(&mut self, docs: &[Document]) {
        for i in 0..self.servers.len() {
            let s = &mut self.servers[i];
            if let Some(req) = &mut s.init {
                let res = match req.poll() {
                    Poll::Pending => None,
                    Poll::Ready(r) => Some(r.map_err(|e| e.message)),
                    Poll::Timeout => Some(Err("svarade inte".to_owned())),
                };
                if let Some(res) = res {
                    s.init = None;
                    match res {
                        Ok(v) => {
                            s.caps = convert::parse_caps(&v);
                            if let Some(c) = &s.client {
                                c.notify("initialized", json!({}));
                            }
                            s.state = SrvState::Ready;
                        }
                        Err(e) => {
                            s.state = SrvState::Failed(e);
                            s.client = None;
                        }
                    }
                }
            }
            let mut events = Vec::new();
            if let Some(c) = &s.client {
                while let Some(e) = c.next_event() {
                    events.push(e);
                }
            }
            for e in events {
                self.on_event(i, e, docs);
            }
        }
    }

    fn on_event(&mut self, si: usize, ev: Event, _docs: &[Document]) {
        match ev {
            Event::Exited => {
                let s = &mut self.servers[si];
                if matches!(s.state, SrvState::Failed(_)) {
                    return;
                }
                s.state = SrvState::Exited;
                s.client = None;
                s.progress.clear();
                let name = s.name.clone();
                self.docs.retain(|_, t| t.server != si);
                self.ops.retain(|o| o.server != si);
                self.out.notes.push((tf!("Språkservern {server} har avslutats.", server = name), true));
            }
            Event::Notification { method, params } => match method.as_str() {
                "textDocument/publishDiagnostics" => {
                    let Some(path) = params.get("uri").and_then(Value::as_str).and_then(pos::uri_to_path) else { return };
                    let text = self.docs.values().find(|t| t.path == path && t.opened).map(|t| t.text.clone());
                    let list = convert::parse_diagnostics(&params, text.as_deref().map(String::as_str));
                    if list.is_empty() {
                        self.diags.remove(&path);
                    } else {
                        self.diags.insert(path, list);
                    }
                }
                "$/progress" => {
                    let token = match params.get("token") {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                        None => return,
                    };
                    let v = params.get("value").unwrap_or(&Value::Null);
                    let s = &mut self.servers[si];
                    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                    let pct = v.get("percentage").and_then(Value::as_u64).map(|p| p.min(100) as u32);
                    match v.get("kind").and_then(Value::as_str) {
                        Some("begin") => {
                            s.progress.retain(|p| p.token != token);
                            s.progress.push(Prog { token, title: text("title"), message: text("message"), pct });
                        }
                        Some("report") => {
                            if let Some(p) = s.progress.iter_mut().find(|p| p.token == token) {
                                if v.get("message").is_some() {
                                    p.message = text("message");
                                }
                                if pct.is_some() {
                                    p.pct = pct;
                                }
                            }
                        }
                        Some("end") => s.progress.retain(|p| p.token != token),
                        _ => {}
                    }
                }
                "window/showMessage" => {
                    let msg = params.get("message").and_then(Value::as_str).unwrap_or("");
                    if params.get("type").and_then(Value::as_u64) == Some(1) && !msg.is_empty() {
                        self.out.notes.push((msg.to_owned(), true));
                    }
                }
                _ => {}
            },
        }
    }

    /// Skickar `didOpen` för dokument vars server blivit klar.
    fn open_pending(&mut self, docs: &[Document]) {
        for d in docs {
            let Some(t) = self.docs.get_mut(&d.id) else { continue };
            if t.opened {
                continue;
            }
            let s = &self.servers[t.server];
            let (SrvState::Ready, Some(c)) = (&s.state, &s.client) else { continue };
            t.version = 1;
            t.text = Arc::new(d.text.clone());
            c.notify(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": t.uri, "languageId": t.lang_id, "version": t.version, "text": *t.text}}),
            );
            t.opened = true;
            t.was_dirty = d.dirty;
            // Diagnostik som kom före öppnandet (andra filer än de öppna) får teckenintervall.
            if let Some(list) = self.diags.get_mut(&t.path) {
                let idx = LineIndex::new(&t.text);
                for dg in list {
                    dg.range = Some(convert::char_range(&idx, &t.text, dg.start, dg.end));
                }
            }
        }
    }

    fn sync_docs(&mut self, ctx: &egui::Context, docs: &[Document]) {
        for d in docs {
            let Some(t) = self.docs.get_mut(&d.id) else { continue };
            if !t.opened {
                continue;
            }
            let s = &self.servers[t.server];
            if s.state != SrvState::Ready {
                continue;
            }
            if t.was_dirty && !d.dirty {
                Self::send_change(&self.servers, t, &mut self.diags, d);
                if let Some(c) = &self.servers[t.server].client {
                    let caps = &self.servers[t.server].caps;
                    if caps.save {
                        let mut p = json!({"textDocument": {"uri": t.uri}});
                        if caps.save_text {
                            p["text"] = json!(*t.text);
                        }
                        c.notify("textDocument/didSave", p);
                    }
                }
            }
            t.was_dirty = d.dirty;
            if d.text == **t.text {
                t.seen = None;
                continue;
            }
            let h = hash_str(&d.text);
            if t.seen != Some(h) {
                t.seen = Some(h);
                t.changed_at = Instant::now();
            }
            let wait = DEBOUNCE.saturating_sub(t.changed_at.elapsed());
            if wait.is_zero() {
                Self::send_change(&self.servers, t, &mut self.diags, d);
            } else {
                ctx.request_repaint_after(wait);
            }
        }
    }

    /// Skickar skillnaden mellan det servern har och dokumentets text.
    fn send_change(servers: &[Server], t: &mut Tracked, diags: &mut HashMap<PathBuf, Vec<Diag>>, d: &Document) {
        if d.text == **t.text {
            return;
        }
        let s = &servers[t.server];
        let Some(c) = &s.client else { return };
        let new = Arc::new(d.text.clone());
        t.version += 1;
        let changes = match s.caps.sync_kind {
            2 => {
                let e = pos::diff_edit(&t.text, &new);
                Some(json!([{"range": {"start": pos_json(e.start), "end": pos_json(e.end)}, "text": e.new_text}]))
            }
            1 => Some(json!([{"text": *new}])),
            _ => None,
        };
        if let Some(changes) = changes {
            c.notify("textDocument/didChange", json!({"textDocument": {"uri": t.uri, "version": t.version}, "contentChanges": changes}));
        }
        if let Some(v) = diags.get_mut(&t.path) {
            convert::shift_ranges(v, &Remap::new(&t.text, &new));
        }
        t.text = new;
        t.seen = None;
    }

    fn untrack(&mut self, id: u64) {
        let Some(t) = self.docs.remove(&id) else { return };
        if t.opened
            && let Some(c) = &self.servers[t.server].client
        {
            c.notify("textDocument/didClose", json!({"textDocument": {"uri": t.uri}}));
        }
        self.ops.retain(|o| o.doc_id != id);
    }

    // ------------------------------------------------------------ frågor till gränssnittet

    /// Servern för dokumentet, om den är klar och dokumentet öppnat hos den.
    pub fn caps(&self, doc_id: u64) -> Option<&Caps> {
        let t = self.docs.get(&doc_id).filter(|t| t.opened)?;
        let s = &self.servers[t.server];
        (s.state == SrvState::Ready).then_some(&s.caps)
    }

    /// Text för statusraden: serverns läge (gärna med förloppet) och om det är ett fel.
    pub fn status(&self, doc_id: u64) -> Option<(String, bool)> {
        let t = self.docs.get(&doc_id)?;
        let s = &self.servers[t.server];
        let name = s.name.as_str();
        Some(match &s.state {
            SrvState::Starting => (tf!("{server}: startar…", server = name), false),
            SrvState::Failed(why) => (tf!("{server}: kunde inte startas ({why})", server = name, why = why), true),
            SrvState::Exited => (tf!("{server}: avslutad", server = name), true),
            SrvState::Ready => match s.progress.last() {
                None => (tf!("{server}: redo", server = name), false),
                Some(p) => {
                    let title = p.title.to_lowercase();
                    let what = if ["index", "scan", "load", "fetch", "build"].iter().any(|k| title.contains(k)) {
                        crate::i18n::t("indexerar…").to_owned()
                    } else if p.message.is_empty() {
                        format!("{}…", p.title)
                    } else {
                        format!("{} {}…", p.title, p.message)
                    };
                    let pct = p.pct.map(|n| format!(" {n} %")).unwrap_or_default();
                    (format!("{name}: {what}{pct}"), false)
                }
            },
        })
    }

    /// Antal fel och varningar över alla filer servrarna rapporterat.
    pub fn counts(&self) -> (usize, usize) {
        let mut e = 0;
        let mut w = 0;
        for d in self.diags.values().flatten() {
            match d.severity {
                convert::Severity::Error => e += 1,
                convert::Severity::Warning => w += 1,
                _ => {}
            }
        }
        (e, w)
    }

    pub fn diags_for(&self, path: &Path) -> &[Diag] {
        self.diags.get(path).map_or(&[], Vec::as_slice)
    }

    /// Startar om servrar som stannat eller inte kunde startas: dokumenten kopplas om vid nästa bild
    /// (och en ny server startas, med de kommandon som gäller nu).
    pub fn restart_failed(&mut self) {
        for i in 0..self.servers.len() {
            if matches!(self.servers[i].state, SrvState::Failed(_) | SrvState::Exited) {
                self.servers[i].retired = true;
                self.docs.retain(|_, t| t.server != i);
            }
        }
        self.warned.clear();
    }

    /// Startar om alla servrar (efter att kommandon ändrats): de gamla stängs i bakgrunden.
    pub fn restart_all(&mut self) {
        for s in &mut self.servers {
            s.retired = true;
            s.progress.clear();
            if let Some(mut c) = s.client.take() {
                std::thread::spawn(move || c.shutdown());
            }
            s.state = SrvState::Exited;
        }
        self.docs.clear();
        self.ops.clear();
        self.diags.clear();
        self.warned.clear();
    }

    /// Stänger alla servrar ordnat (anropas vid avslut eller när LSP stängs av).
    pub fn shutdown(&mut self) {
        self.ops.clear();
        self.docs.clear();
        self.diags.clear();
        let handles: Vec<_> = std::mem::take(&mut self.servers)
            .into_iter()
            .filter_map(|mut s| s.client.take())
            .map(|mut c| std::thread::spawn(move || c.shutdown()))
            .collect();
        for h in handles {
            let _ = h.join();
        }
    }

    // ------------------------------------------------------------ anrop

    /// Skickar ändringar som väntar så att servern ser samma text som editorn.
    pub fn flush(&mut self, d: &Document) {
        if let Some(t) = self.docs.get_mut(&d.id)
            && t.opened
        {
            Self::send_change(&self.servers, t, &mut self.diags, d);
        }
    }

    fn start_op(&mut self, d: &Document, method: &str, params: Value, kind: Kind, timeout: Duration) -> bool {
        self.flush(d);
        let Some(t) = self.docs.get(&d.id).filter(|t| t.opened) else { return false };
        let s = &self.servers[t.server];
        let (SrvState::Ready, Some(c)) = (&s.state, &s.client) else { return false };
        if let Some(k) = kind.singleton() {
            self.ops.retain(|o| o.kind.singleton() != Some(k));
        }
        let req = c.request(method, params, timeout);
        self.ops.push(Op { doc_id: d.id, server: t.server, kind, req, base: t.text.clone() });
        true
    }

    fn td_pos(&self, d: &Document, ci: usize) -> Option<Value> {
        let t = self.docs.get(&d.id)?;
        let p = pos::char_to_pos(&d.text, ci);
        Some(json!({"textDocument": {"uri": t.uri}, "position": pos_json(p)}))
    }

    fn note(&mut self, text: String, error: bool) {
        self.out.notes.push((text, error));
    }

    /// Anropar `feature` om servern kan; annars ett meddelande.
    fn can(&mut self, d: &Document, what: &'static str, f: impl Fn(&Caps) -> bool) -> bool {
        match self.caps(d.id) {
            None => false,
            Some(c) if f(c) => true,
            Some(_) => {
                self.note(tf!("Språkservern stöder inte {feature}.", feature = crate::i18n::t(what)), false);
                false
            }
        }
    }

    pub fn complete(&mut self, d: &Document, caret: usize, trigger: Trigger) {
        if !self.caps(d.id).is_some_and(|c| c.completion) {
            return;
        }
        let ctx = match trigger {
            Trigger::Manual => json!({"triggerKind": 1}),
            Trigger::Char(c) => json!({"triggerKind": 2, "triggerCharacter": c.to_string()}),
            Trigger::Incomplete => json!({"triggerKind": 3}),
        };
        let anchor = ui::word_start(&d.text, caret);
        if let Some(mut p) = self.td_pos(d, caret) {
            p["context"] = ctx;
            let manual = trigger == Trigger::Manual;
            self.start_op(d, "textDocument/completion", p, Kind::Completion { anchor, manual }, Duration::from_secs(8));
        }
    }

    pub fn hover(&mut self, d: &Document, at: usize) {
        if !self.caps(d.id).is_some_and(|c| c.hover) {
            return;
        }
        if let Some(p) = self.td_pos(d, at) {
            self.start_op(d, "textDocument/hover", p, Kind::Hover { at }, Duration::from_secs(8));
        }
    }

    pub fn signature(&mut self, d: &Document, caret: usize) {
        if !self.caps(d.id).is_some_and(|c| c.signature) {
            return;
        }
        if let Some(p) = self.td_pos(d, caret) {
            self.start_op(d, "textDocument/signatureHelp", p, Kind::Signature, Duration::from_secs(8));
        }
    }

    pub fn definition(&mut self, d: &Document, at: usize) {
        if !self.can(d, "gå till definition", |c| c.definition) {
            return;
        }
        if let Some(p) = self.td_pos(d, at) {
            self.start_op(d, "textDocument/definition", p, Kind::Definition, Duration::from_secs(30));
        }
    }

    pub fn references(&mut self, d: &Document, at: usize) {
        if !self.can(d, "sök referenser", |c| c.references) {
            return;
        }
        if let Some(mut p) = self.td_pos(d, at) {
            p["context"] = json!({"includeDeclaration": true});
            self.start_op(d, "textDocument/references", p, Kind::References, Duration::from_secs(60));
        }
    }

    /// Första steget av "Byt namn": frågar vad som kan döpas om och öppnar sedan namnrutan.
    pub fn begin_rename(&mut self, d: &Document, at: usize) {
        if !self.can(d, "byt namn", |c| c.rename) {
            return;
        }
        let prepare = self.caps(d.id).is_some_and(|c| c.prepare_rename);
        if prepare {
            if let Some(p) = self.td_pos(d, at) {
                self.start_op(d, "textDocument/prepareRename", p, Kind::PrepareRename { at }, Duration::from_secs(15));
            }
        } else if let Some((a, b)) = convert::word_at(&d.text, at) {
            self.ui.open_rename(d.id, at, d.text.chars().skip(a).take(b - a).collect());
        } else {
            self.note(crate::i18n::t("Det går inte att byta namn här.").to_owned(), false);
        }
    }

    pub fn rename(&mut self, d: &Document, at: usize, new_name: &str) {
        if let Some(mut p) = self.td_pos(d, at) {
            p["newName"] = json!(new_name);
            self.start_op(d, "textDocument/rename", p, Kind::Rename, Duration::from_secs(60));
        }
    }

    pub fn format(&mut self, d: &Document) {
        if !self.can(d, "formatering", |c| c.formatting) {
            return;
        }
        let Some(t) = self.docs.get(&d.id) else { return };
        let p = json!({"textDocument": {"uri": t.uri}, "options": {"tabSize": 4, "insertSpaces": true, "trimTrailingWhitespace": true, "insertFinalNewline": true}});
        self.start_op(d, "textDocument/formatting", p, Kind::Format, Duration::from_secs(30));
    }

    // ------------------------------------------------------------ svar

    fn poll_ops(&mut self, ctx: &egui::Context, docs: &[Document]) {
        let ops = std::mem::take(&mut self.ops);
        for mut op in ops {
            let res = match op.req.poll() {
                Poll::Pending => {
                    self.ops.push(op);
                    continue;
                }
                Poll::Ready(r) => r,
                Poll::Timeout => {
                    if !op.kind.silent() {
                        self.note(crate::i18n::t("Språkservern svarade inte i tid.").to_owned(), true);
                    }
                    continue;
                }
            };
            match res {
                // -32800 (avbruten) och -32801 (innehållet ändrades) är normala när man skriver vidare.
                Err(e) if matches!(e.code, -32800 | -32801) => {}
                Err(e) => {
                    if !op.kind.silent() && !e.message.is_empty() {
                        self.note(tf!("Språkservern: {message}", message = e.message), true);
                    }
                }
                Ok(v) => {
                    if let Some(d) = docs.iter().find(|d| d.id == op.doc_id) {
                        self.finish(d, op, v);
                    }
                }
            }
        }
        ctx.request_repaint();
    }

    fn finish(&mut self, d: &Document, op: Op, v: Value) {
        let base = op.base.clone();
        let idx = LineIndex::new(&base);
        match op.kind {
            Kind::Completion { anchor, manual } => {
                let (mut items, incomplete) = convert::parse_completion(&v);
                if items.is_empty() {
                    self.ui.comp = None;
                    if manual {
                        self.note(crate::i18n::t("Inga förslag.").to_owned(), false);
                    }
                    return;
                }
                items.sort_by(|a, b| a.sort.cmp(&b.sort));
                self.ui.set_completion(d.id, base, anchor, items, incomplete);
            }
            Kind::Hover { at } => {
                self.ui.hover_reply(d.id, base.clone(), at, convert::parse_hover(&v).map(|(md, r)| {
                    let range = r.map(|(a, b)| convert::char_range(&idx, &base, a, b));
                    (md, range)
                }));
            }
            Kind::Signature => self.ui.set_signature(d.id, base, convert::parse_signature(&v)),
            Kind::Definition => {
                let locs = convert::parse_locations(&v);
                match locs.len() {
                    0 => self.note(crate::i18n::t("Ingen definition hittades.").to_owned(), false),
                    1 => self.out.jump = Some(Jump { path: locs[0].path.clone(), start: locs[0].start, end: locs[0].end }),
                    _ => self.ui.show_locations(crate::i18n::t("Definitioner"), &locs, docs_text(&self.docs, d)),
                }
            }
            Kind::References => {
                let locs = convert::parse_locations(&v);
                if locs.is_empty() {
                    self.note(crate::i18n::t("Inga referenser hittades.").to_owned(), false);
                } else {
                    self.ui.show_locations(crate::i18n::t("Referenser"), &locs, docs_text(&self.docs, d));
                }
            }
            Kind::PrepareRename { at } => {
                // Null = inget att döpa om. Annars `Range`, `{range, placeholder}` eller `{defaultBehavior}`.
                let range = v.get("range").or(Some(&v)).filter(|r| r.get("start").is_some());
                let name = v.get("placeholder").and_then(Value::as_str).map(str::to_owned).or_else(|| {
                    let r = range?;
                    let g = |p: &Value| Some((p.get("line")?.as_u64()? as u32, p.get("character")?.as_u64()? as u32));
                    let (a, b) = convert::char_range(&idx, &base, g(r.get("start")?)?, g(r.get("end")?)?);
                    Some(base.chars().skip(a).take(b - a).collect())
                });
                let name = name.or_else(|| {
                    if v.get("defaultBehavior").and_then(Value::as_bool) == Some(true) {
                        convert::word_at(&base, at).map(|(a, b)| base.chars().skip(a).take(b - a).collect())
                    } else {
                        None
                    }
                });
                match name {
                    Some(n) if !v.is_null() => self.ui.open_rename(d.id, at, n),
                    _ => self.note(crate::i18n::t("Det går inte att byta namn här.").to_owned(), false),
                }
            }
            Kind::Rename => {
                let (files, skipped) = convert::parse_workspace_edit(&v);
                if files.is_empty() {
                    self.note(crate::i18n::t("Servern föreslog inga ändringar.").to_owned(), false);
                    return;
                }
                if skipped > 0 {
                    self.note(crate::i18n::t("Filer som skulle skapas, flyttas eller tas bort hoppades över.").to_owned(), false);
                }
                let own = d.path.clone();
                let mut n = 0;
                for (path, edits) in files {
                    n += edits.len();
                    let expect = (Some(&path) == own.as_ref()).then(|| base.clone());
                    self.out.edits.push(FileEdits { path, edits, expect });
                }
                let n_files = self.out.edits.len();
                self.note(tf!("Bytte namn på {n} ställen i {files} filer.", n = n, files = n_files), false);
            }
            Kind::Format => {
                let edits: Vec<Edit> = v.as_array().map(|a| a.iter().filter_map(convert::parse_edit).collect()).unwrap_or_default();
                if edits.is_empty() {
                    self.note(crate::i18n::t("Dokumentet är redan formaterat.").to_owned(), false);
                } else if let Some(path) = d.path.clone() {
                    self.out.edits.push(FileEdits { path, edits, expect: Some(base) });
                }
            }
        }
    }
}

/// Texten för en fil som visas i en lista (för förhandsvisning av rader): den öppna flikens synkade text.
fn docs_text(docs: &HashMap<u64, Tracked>, d: &Document) -> Option<(PathBuf, Arc<String>)> {
    let t = docs.get(&d.id)?;
    Some((t.path.clone(), t.text.clone()))
}

fn init_params(root_uri: &str, name: &str) -> Value {
    let md = json!(["markdown", "plaintext"]);
    json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "Kladd", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": root_uri,
        "workspaceFolders": [{"uri": root_uri, "name": name}],
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "window": {"workDoneProgress": true},
            "workspace": {"workspaceFolders": true, "configuration": true, "applyEdit": false},
            "textDocument": {
                "synchronization": {"didSave": true, "dynamicRegistration": false},
                "publishDiagnostics": {"relatedInformation": false},
                "completion": {"completionItem": {"snippetSupport": true, "documentationFormat": md,
                    "insertReplaceSupport": true, "labelDetailsSupport": true}},
                "hover": {"contentFormat": md},
                "signatureHelp": {"signatureInformation": {"documentationFormat": md,
                    "parameterInformation": {"labelOffsetSupport": true}, "activeParameterSupport": true}},
                "definition": {"linkSupport": true},
                "references": {},
                "rename": {"prepareSupport": true},
                "formatting": {}
            }
        }
    })
}
