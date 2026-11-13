//! Git: allt som rör programmet `git` utan gränssnitt. Kommandona körs via `platform::quiet` (inget
//! libgit2), utdata tolkas här och är testat mot tillfälliga arkiv. Saknas `git` döljs allt Git-relaterat.
//!
//! Ingen funktion här får anropas från gränssnittstråden annat än de rena tolkarna och `hunks`;
//! `git_ui.rs` kör resten i bakgrundstrådar.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use crate::i18n::t;
use crate::tf;

/// Större filer får ingen marginalmarkering, blame eller jämförelse.
pub const MAX_TEXT_BYTES: usize = 2 << 20;
/// Så många incheckningar visas i källkontrollpanelen.
pub const LOG_LIMIT: usize = 50;

// ============================================================== köra git

static AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Finns `git` på datorn? Startar programmet första gången (anropas i en bakgrundstråd).
pub fn probe() -> bool {
    *AVAILABLE.get_or_init(|| {
        let mut cmd = Command::new("git");
        cmd.arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        crate::platform::quiet(&mut cmd).status().is_ok_and(|s| s.success())
    })
}

/// Svaret från ett git-kommando.
pub struct Output {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Felet att visa: stderr, annars stdout (t.ex. "nothing to commit" skrivs dit).
    pub fn error_text(&self) -> String {
        let e = self.stderr.trim();
        if e.is_empty() { self.text().trim().to_owned() } else { e.to_owned() }
    }
}

/// Kör `git args…` i `cwd`. `input` skickas på stdin.
pub fn run(cwd: &Path, args: &[&str], input: Option<&[u8]>) -> std::io::Result<Output> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .args(args)
        // Fråga aldrig efter lösenord i en terminal som inte finns, och ta inga onödiga lås i bakgrunden.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let mut child = crate::platform::quiet(&mut cmd).spawn()?;
    // Stdin skrivs i en egen tråd så att git inte kan fastna med full stdout-ledning.
    let writer = input.map(|data| {
        let mut stdin = child.stdin.take();
        let data = data.to_vec();
        std::thread::spawn(move || {
            if let Some(s) = stdin.as_mut() {
                let _ = s.write_all(&data);
            }
        })
    });
    let out = child.wait_with_output()?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    Ok(Output { ok: out.status.success(), stdout: out.stdout, stderr: String::from_utf8_lossy(&out.stderr).into_owned() })
}

/// Som `run` men ger stdout som text, eller felmeddelandet.
pub fn run_text(cwd: &Path, args: &[&str]) -> Result<String, String> {
    match run(cwd, args, None) {
        Ok(o) if o.ok => Ok(o.text()),
        Ok(o) => Err(o.error_text()),
        Err(e) => Err(tf!("Git kunde inte startas: {e}", e = e)),
    }
}

// ============================================================== status

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl Change {
    /// Bokstaven i filträdet och i listan.
    pub fn letter(self) -> char {
        match self {
            Change::Modified => 'M',
            Change::Added => 'A',
            Change::Deleted => 'D',
            Change::Renamed => 'R',
            Change::Untracked => 'U',
            Change::Conflicted => '!',
        }
    }
}

/// En rad ur `git status`. `path` är relativ arkivets rot och har `/` som skiljetecken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStatus {
    pub path: String,
    /// Gamla namnet vid omdöpning.
    pub orig: Option<String>,
    /// Ändring som är köad (i indexet).
    pub index: Option<Change>,
    /// Ändring i arbetskatalogen (inte köad). Orörda filer utan spår: `Untracked`.
    pub work: Option<Change>,
}

impl FileStatus {
    pub fn conflicted(&self) -> bool {
        self.index == Some(Change::Conflicted)
    }

    /// Ändringen som visas i trädet: konflikt först, sedan arbetskatalogen, sedan indexet.
    pub fn shown(&self) -> Change {
        if self.conflicted() { Change::Conflicted } else { self.work.or(self.index).unwrap_or(Change::Modified) }
    }

    pub fn staged(&self) -> bool {
        !self.conflicted() && self.index.is_some()
    }

    pub fn unstaged(&self) -> bool {
        !self.conflicted() && self.work.is_some()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Grenens namn; `None` när HEAD är frånkopplad.
    pub branch: Option<String>,
    pub detached: bool,
    /// Kortad incheckning vid frånkopplad HEAD.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileStatus>,
}

fn xy_change(c: char) -> Option<Change> {
    match c {
        'M' | 'T' => Some(Change::Modified),
        'A' => Some(Change::Added),
        'D' => Some(Change::Deleted),
        'R' | 'C' => Some(Change::Renamed),
        _ => None,
    }
}

/// Tolkar `git status --porcelain=v2 -z --branch`.
pub fn parse_status(out: &str) -> Status {
    let mut st = Status::default();
    let mut parts = out.split('\0');
    while let Some(rec) = parts.next() {
        if let Some(h) = rec.strip_prefix("# ") {
            if let Some(v) = h.strip_prefix("branch.head ") {
                if v == "(detached)" {
                    st.detached = true;
                } else {
                    st.branch = Some(v.to_owned());
                }
            } else if let Some(v) = h.strip_prefix("branch.oid ") {
                if v != "(initial)" {
                    st.oid = Some(v.chars().take(7).collect());
                }
            } else if let Some(v) = h.strip_prefix("branch.upstream ") {
                st.upstream = Some(v.to_owned());
            } else if let Some(v) = h.strip_prefix("branch.ab ") {
                for tok in v.split_whitespace() {
                    if let Some(n) = tok.strip_prefix('+') {
                        st.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = tok.strip_prefix('-') {
                        st.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        let kind = rec.as_bytes().first().copied();
        let xy = |s: &str| -> (Option<Change>, Option<Change>) {
            let mut c = s.chars();
            (xy_change(c.next().unwrap_or('.')), xy_change(c.next().unwrap_or('.')))
        };
        match kind {
            Some(b'1') => {
                let f: Vec<&str> = rec.splitn(9, ' ').collect();
                if f.len() == 9 {
                    let (index, work) = xy(f[1]);
                    st.files.push(FileStatus { path: f[8].to_owned(), orig: None, index, work });
                }
            }
            Some(b'2') => {
                let f: Vec<&str> = rec.splitn(10, ' ').collect();
                let orig = parts.next().map(str::to_owned);
                if f.len() == 10 {
                    let (index, work) = xy(f[1]);
                    st.files.push(FileStatus { path: f[9].to_owned(), orig, index, work });
                }
            }
            Some(b'u') => {
                let f: Vec<&str> = rec.splitn(11, ' ').collect();
                if f.len() == 11 {
                    st.files.push(FileStatus {
                        path: f[10].to_owned(),
                        orig: None,
                        index: Some(Change::Conflicted),
                        work: Some(Change::Conflicted),
                    });
                }
            }
            Some(b'?') => {
                if let Some(p) = rec.strip_prefix("? ") {
                    st.files.push(FileStatus { path: p.to_owned(), orig: None, index: None, work: Some(Change::Untracked) });
                }
            }
            _ => {}
        }
    }
    st
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    /// Sekunder sedan 1970.
    pub time: i64,
    pub subject: String,
}

/// Tolkar `git log --pretty=format:%H%x1f%an%x1f%at%x1f%s%x1e`.
pub fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .filter_map(|rec| {
            let mut f = rec.trim_start_matches('\n').splitn(4, '\x1f');
            let hash = f.next()?.trim();
            if hash.is_empty() {
                return None;
            }
            Some(Commit {
                hash: hash.to_owned(),
                author: f.next()?.to_owned(),
                time: f.next()?.trim().parse().unwrap_or(0),
                subject: f.next().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

/// Allt panelen och trädet behöver veta om arkivet.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Arkivets rotmapp.
    pub root: std::path::PathBuf,
    /// Arbetsmappen relativt roten, med `/` och avslutande `/` (tom när de är samma).
    pub prefix: String,
    pub status: Status,
    pub branches: Vec<String>,
    pub log: Vec<Commit>,
}

/// Läser arkivets tillstånd för mappen `dir`. `None` om mappen inte ligger i ett arkiv.
pub fn snapshot(dir: &Path) -> Option<Snapshot> {
    let top = run_text(dir, &["rev-parse", "--show-toplevel", "--show-prefix"]).ok()?;
    let mut lines = top.lines();
    let root = lines.next()?.trim();
    if root.is_empty() {
        return None;
    }
    let prefix = lines.next().unwrap_or("").trim().to_owned();
    let root = std::path::PathBuf::from(root);
    let status = parse_status(&run_text(&root, &["status", "--porcelain=v2", "-z", "--branch"]).ok()?);
    let branches = run_text(&root, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .map(|s| s.lines().map(str::to_owned).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    let n = format!("-n{LOG_LIMIT}");
    let log = run_text(&root, &["log", &n, "--pretty=format:%H%x1f%an%x1f%at%x1f%s%x1e"]).map(|s| parse_log(&s)).unwrap_or_default();
    Some(Snapshot { root, prefix, status, branches, log })
}

// ============================================================== ändringar mot HEAD

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}

/// En sammanhängande ändring mellan HEAD-versionen och texten. Radnummer räknas från 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub kind: HunkKind,
    /// Första raden i nya texten (för `Deleted`: raden där de borttagna raderna satt).
    pub new_start: usize,
    pub new_len: usize,
    /// Gamla raderna (med radslut).
    pub old: Vec<String>,
}

/// Ändringarna från `old` (HEAD) till `new`, i radordning.
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    use similar::{DiffOp, TextDiff};
    let diff = TextDiff::configure().timeout(Duration::from_millis(150)).diff_lines(old, new);
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let take = |i: usize, n: usize| old_lines[i.min(old_lines.len())..(i + n).min(old_lines.len())].iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let mut out = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert { new_index, new_len, .. } => {
                out.push(Hunk { kind: HunkKind::Added, new_start: new_index, new_len, old: Vec::new() })
            }
            DiffOp::Delete { old_index, old_len, new_index } => {
                out.push(Hunk { kind: HunkKind::Deleted, new_start: new_index, new_len: 0, old: take(old_index, old_len) })
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                out.push(Hunk { kind: HunkKind::Modified, new_start: new_index, new_len, old: take(old_index, old_len) })
            }
        }
    }
    out
}

/// Vilken ändring (index i `hunks`) gäller rad `line` (från 0)? Borttagna rader hör till raden under.
pub fn hunk_at(hunks: &[Hunk], line: usize) -> Option<usize> {
    let i = hunks.partition_point(|h| h.new_start + h.new_len.max(1) <= line);
    hunks.get(i).filter(|h| line >= h.new_start && line < h.new_start + h.new_len.max(1)).map(|_| i)
}

/// Texten med ändringen ångrad (de nya raderna ersätts av de gamla).
pub fn revert_hunk(text: &str, h: &Hunk) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start = h.new_start.min(lines.len());
    let end = (start + h.new_len).min(lines.len());
    let mut old = h.old.concat();
    // En gammal sista rad utan radslut får inte smälta ihop med raden efter.
    if !old.is_empty() && !old.ends_with('\n') && end < lines.len() {
        old.push('\n');
    }
    let mut out = lines[..start].concat();
    out.push_str(&old);
    out.push_str(&lines[end..].concat());
    out
}

/// Filens innehåll i HEAD som text för jämförelse med `name` i mappen `dir`.
/// `None` när ingen jämförelse ska visas: inte i ett arkiv, ignorerad, binär eller för stor.
/// En fil som inte finns i HEAD (ny eller arkivet saknar incheckningar) jämförs mot tom text.
pub fn head_text(dir: &Path, name: &str) -> Option<String> {
    if run_text(dir, &["rev-parse", "--is-inside-work-tree"]).ok()?.trim() != "true" {
        return None;
    }
    if run(dir, &["check-ignore", "-q", "--", name], None).ok()?.ok {
        return None;
    }
    let spec = format!("HEAD:./{name}");
    let bytes = match run(dir, &["show", &spec], None) {
        Ok(o) if o.ok => o.stdout,
        Ok(_) => Vec::new(),
        Err(_) => return None,
    };
    if bytes.len() > MAX_TEXT_BYTES || bytes.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).replace("\r\n", "\n"))
}

// ============================================================== blame

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub sha: String,
    pub author: String,
    /// Sekunder sedan 1970.
    pub time: i64,
    pub summary: String,
}

impl BlameLine {
    /// Raden är inte incheckad än.
    pub fn uncommitted(&self) -> bool {
        self.sha.bytes().all(|b| b == b'0')
    }
}

/// Tolkar `git blame --porcelain`: en post per rad i radordning.
pub fn parse_blame(out: &str) -> Vec<BlameLine> {
    let mut info: HashMap<&str, (String, i64, String)> = HashMap::new();
    let mut cur: Option<&str> = None;
    let mut lines = Vec::new();
    for l in out.split('\n') {
        if l.starts_with('\t') {
            if let Some(sha) = cur {
                let (author, time, summary) = info.get(sha).cloned().unwrap_or_default();
                lines.push(BlameLine { sha: sha.to_owned(), author, time, summary });
            }
            continue;
        }
        let mut tok = l.split_whitespace();
        if let (Some(a), Some(b), Some(c)) = (tok.next(), tok.next(), tok.next())
            && a.len() >= 40
            && a.bytes().all(|x| x.is_ascii_hexdigit())
            && b.parse::<usize>().is_ok()
            && c.parse::<usize>().is_ok()
        {
            cur = Some(a);
            info.entry(a).or_default();
            continue;
        }
        let Some(sha) = cur else { continue };
        let Some(e) = info.get_mut(sha) else { continue };
        if let Some(v) = l.strip_prefix("author ") {
            e.0 = v.to_owned();
        } else if let Some(v) = l.strip_prefix("author-time ") {
            e.1 = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("summary ") {
            e.2 = v.to_owned();
        }
    }
    lines
}

/// Blame för `text` (bufferten, inte filen på disk) som om den låg på `name` i `dir`.
pub fn blame(dir: &Path, name: &str, text: &str) -> Option<Vec<BlameLine>> {
    let o = run(dir, &["blame", "--porcelain", "--contents", "-", "--", name], Some(text.as_bytes())).ok()?;
    if !o.ok {
        return None;
    }
    Some(parse_blame(&o.text()))
}

// ============================================================== åtgärder

/// Ett git-kommando i en kedja av åtgärder (en knapp kan behöva flera).
#[derive(Clone, Debug)]
pub struct Step {
    pub args: Vec<String>,
    pub stdin: Option<String>,
}

pub fn step(args: &[&str]) -> Step {
    Step { args: args.iter().map(|s| (*s).to_owned()).collect(), stdin: None }
}

fn with_paths(base: &[&str], paths: &[String]) -> Step {
    let mut args: Vec<String> = base.iter().map(|s| (*s).to_owned()).collect();
    args.push("--".into());
    args.extend(paths.iter().cloned());
    Step { args, stdin: None }
}

/// Köa (`git add`).
pub fn stage_step(paths: &[String]) -> Step {
    with_paths(&["add"], paths)
}

/// Ta ur kön (`git reset`, fungerar också i ett arkiv utan incheckningar).
pub fn unstage_step(paths: &[String]) -> Step {
    with_paths(&["reset", "-q"], paths)
}

/// Förkasta ändringarna i arbetskatalogen: ospårade filer tas bort, andra återställs från indexet
/// (det som är köat påverkas inte).
pub fn discard_steps(files: &[FileStatus]) -> Vec<Step> {
    let (untracked, tracked): (Vec<&FileStatus>, Vec<&FileStatus>) = files.iter().partition(|f| f.index.is_none() && f.work == Some(Change::Untracked));
    let paths = |v: Vec<&FileStatus>| v.into_iter().map(|f| f.path.clone()).collect::<Vec<_>>();
    let (untracked, tracked) = (paths(untracked), paths(tracked));
    let mut v = Vec::new();
    if !untracked.is_empty() {
        v.push(with_paths(&["clean", "-fdq"], &untracked));
    }
    if !tracked.is_empty() {
        v.push(with_paths(&["checkout", "-q"], &tracked));
    }
    v
}

pub fn commit_step(message: &str) -> Step {
    Step { args: vec!["commit".into(), "-F".into(), "-".into()], stdin: Some(message.to_owned()) }
}

/// Pusha grenen; saknas fjärrgren skapas den (`-u origin`).
pub fn push_step(branch: Option<&str>, has_upstream: bool) -> Step {
    match (has_upstream, branch) {
        (false, Some(b)) => step(&["push", "--set-upstream", "origin", b]),
        _ => step(&["push"]),
    }
}

pub fn switch_step(name: &str) -> Step {
    step(&["checkout", name, "--"])
}

pub fn create_branch_step(name: &str) -> Step {
    step(&["checkout", "-b", name, "--"])
}

/// Kör stegen i ordning och stannar vid första felet. Ger sammanfogad utdata eller felet.
pub fn run_steps(root: &Path, steps: &[Step]) -> Result<String, String> {
    let mut all = String::new();
    for s in steps {
        let args: Vec<&str> = s.args.iter().map(String::as_str).collect();
        match run(root, &args, s.stdin.as_deref().map(str::as_bytes)) {
            Ok(o) if o.ok => {
                let text = o.text();
                let err = o.stderr.trim();
                // Pull och push skriver förloppet på stderr.
                for part in [text.trim(), err] {
                    if !part.is_empty() {
                        all.push_str(part);
                        all.push('\n');
                    }
                }
            }
            Ok(o) => return Err(o.error_text()),
            Err(e) => return Err(tf!("Git kunde inte startas: {e}", e = e)),
        }
    }
    Ok(all.trim().to_owned())
}

/// Ett namn på en gren som git accepterar (förenklad kontroll; git säger ifrån om resten).
pub fn valid_branch_name(name: &str) -> Result<&str, &'static str> {
    let n = name.trim();
    if n.is_empty() {
        return Err(t("Ange ett namn."));
    }
    if n.starts_with('-') || n.starts_with('/') || n.ends_with('/') || n.ends_with('.') || n.contains("..") || n.contains("//") {
        return Err(t("Ogiltigt namn på gren."));
    }
    if n.chars().any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c)) {
        return Err(t("Ogiltigt namn på gren."));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kladd-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Ett nytt arkiv med en användare och grenen `main`, eller `None` om git saknas.
    fn repo(name: &str) -> Option<PathBuf> {
        if !probe() {
            return None;
        }
        let d = tmp(name);
        run_text(&d, &["init", "-q"]).unwrap();
        run_text(&d, &["symbolic-ref", "HEAD", "refs/heads/main"]).unwrap();
        run_text(&d, &["config", "user.name", "Test Person"]).unwrap();
        run_text(&d, &["config", "user.email", "test@example.com"]).unwrap();
        run_text(&d, &["config", "commit.gpgsign", "false"]).unwrap();
        // Git för Windows har core.autocrlf=true globalt; testerna jämför radslut byte för byte.
        run_text(&d, &["config", "core.autocrlf", "false"]).unwrap();
        Some(d.canonicalize().unwrap())
    }

    fn status_of(root: &Path) -> Status {
        snapshot(root).unwrap().status
    }

    #[test]
    fn parses_status_v2() {
        let out = "# branch.oid 0123456789abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
                   1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\0\
                   1 A. N... 000000 100644 100644 000 ccc new.txt\0\
                   1 MD N... 100644 100644 000000 aaa bbb both.txt\0\
                   2 R. N... 100644 100644 100644 aaa bbb R100 neu.txt\0alt.txt\0\
                   u UU N... 100644 100644 100644 100644 a b c konflikt.txt\0\
                   ? dir/\0? lös.txt\0";
        let st = parse_status(out);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert!(!st.detached);
        assert_eq!(st.oid.as_deref(), Some("0123456"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.files.len(), 7);
        let f = |p: &str| st.files.iter().find(|f| f.path == p).unwrap();
        assert_eq!((f("src/a b.rs").index, f("src/a b.rs").work), (None, Some(Change::Modified)));
        assert_eq!((f("new.txt").index, f("new.txt").work), (Some(Change::Added), None));
        assert_eq!((f("both.txt").index, f("both.txt").work), (Some(Change::Modified), Some(Change::Deleted)));
        assert!(f("both.txt").staged() && f("both.txt").unstaged());
        assert_eq!(f("neu.txt").orig.as_deref(), Some("alt.txt"));
        assert_eq!(f("neu.txt").index, Some(Change::Renamed));
        assert!(f("konflikt.txt").conflicted());
        assert_eq!(f("konflikt.txt").shown(), Change::Conflicted);
        assert!(!f("konflikt.txt").staged() && !f("konflikt.txt").unstaged());
        assert_eq!(f("lös.txt").shown(), Change::Untracked);
        assert_eq!(Change::Untracked.letter(), 'U');
    }

    #[test]
    fn parses_detached_and_initial() {
        let st = parse_status("# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.oid, None);
        assert!(st.files.is_empty());
        let st = parse_status("# branch.oid abcdef0123\0# branch.head (detached)\0");
        assert!(st.detached);
        assert_eq!(st.branch, None);
        assert_eq!(st.oid.as_deref(), Some("abcdef0"));
    }

    #[test]
    fn parses_log() {
        let out = "aaa\x1fAnna\x1f1700000000\x1fFörsta\x1e\nbbb\x1fBo\x1f1700000100\x1fAndra: med kolon\x1e";
        let l = parse_log(out);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].hash.as_str(), l[0].author.as_str(), l[0].time, l[0].subject.as_str()), ("aaa", "Anna", 1_700_000_000, "Första"));
        assert_eq!(l[1].subject, "Andra: med kolon");
        assert!(parse_log("").is_empty());
    }

    #[test]
    fn hunks_to_markers() {
        let old = "a\nb\nc\nd\ne\nf\n";
        // b ändrad, e borttagen, ny rad g sist.
        let new = "a\nB\nc\nd\nf\ng\n";
        let h = hunks(old, new);
        assert_eq!(h.len(), 3, "{h:?}");
        assert_eq!(h[0], Hunk { kind: HunkKind::Modified, new_start: 1, new_len: 1, old: vec!["b\n".into()] });
        assert_eq!(h[1], Hunk { kind: HunkKind::Deleted, new_start: 4, new_len: 0, old: vec!["e\n".into()] });
        assert_eq!(h[2], Hunk { kind: HunkKind::Added, new_start: 5, new_len: 1, old: vec![] });
        // Radvis uppslag: B ändrad, f (där e satt) markerad som borttagen, g ny.
        let marks: Vec<Option<HunkKind>> = (0..6).map(|l| hunk_at(&h, l).map(|i| h[i].kind)).collect();
        assert_eq!(marks, vec![None, Some(HunkKind::Modified), None, None, Some(HunkKind::Deleted), Some(HunkKind::Added)]);
        assert_eq!(hunk_at(&h, 6), None);
        assert!(hunks("x\n", "x\n").is_empty());
        let all = hunks("", "a\nb\n");
        assert_eq!(all, vec![Hunk { kind: HunkKind::Added, new_start: 0, new_len: 2, old: vec![] }]);
    }

    #[test]
    fn reverting_hunks() {
        let old = "a\nb\nc\nd\ne\nf\n";
        let new = "a\nB\nc\nd\nf\ng\n";
        let h = hunks(old, new);
        // Ångra alla bakifrån så att radnumren stämmer ger gamla texten.
        let mut t = new.to_owned();
        for hk in h.iter().rev() {
            t = revert_hunk(&t, hk);
        }
        assert_eq!(t, old);
        // Ångra bara den ändrade raden.
        assert_eq!(revert_hunk(new, &h[0]), "a\nb\nc\nd\nf\ng\n");
        // Sista raden utan radslut.
        let h = hunks("a\nb", "a\nX\nc\n");
        let mut t = "a\nX\nc\n".to_owned();
        for hk in h.iter().rev() {
            t = revert_hunk(&t, hk);
        }
        assert_eq!(t, "a\nb");
        let h = hunks("a\nb", "a\nb\nc");
        assert_eq!(revert_hunk("a\nb\nc", &h[0]), "a\nb");
    }

    #[test]
    fn parses_blame_porcelain() {
        let s1 = "1111111111111111111111111111111111111111";
        let s2 = "0000000000000000000000000000000000000000";
        let out = format!(
            "{s1} 1 1 2\nauthor Anna A\nauthor-mail <a@x>\nauthor-time 1700000000\nauthor-tz +0100\ncommitter Anna A\n\
             summary Första raden\nfilename f.txt\n\tett\n{s1} 2 2\n\ttvå\n\
             {s2} 3 3 1\nauthor Not Committed Yet\nauthor-time 1700000500\nsummary Version of f.txt from f.txt\nfilename f.txt\n\ttre\n"
        );
        let b = parse_blame(&out);
        assert_eq!(b.len(), 3);
        assert_eq!((b[0].author.as_str(), b[0].time, b[0].summary.as_str()), ("Anna A", 1_700_000_000, "Första raden"));
        assert_eq!(b[1], b[0]);
        assert!(!b[0].uncommitted());
        assert!(b[2].uncommitted());
        assert_eq!(b[2].author, "Not Committed Yet");
        assert!(parse_blame("").is_empty());
    }

    #[test]
    fn branch_names() {
        assert_eq!(valid_branch_name(" feat/x "), Ok("feat/x"));
        assert!(valid_branch_name("").is_err());
        assert!(valid_branch_name("a b").is_err());
        assert!(valid_branch_name("-x").is_err());
        assert!(valid_branch_name("a..b").is_err());
        assert!(valid_branch_name("a:b").is_err());
    }

    #[test]
    fn stage_unstage_commit_flow() {
        let Some(root) = repo("flow") else { return };
        std::fs::write(root.join("a.txt"), "ett\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("b.txt"), "två\n").unwrap();

        // Ett arkiv utan incheckningar: allt är ospårat.
        let snap = snapshot(&root).unwrap();
        // Git skriver "C:/…" på Windows; jämför kanoniskt.
        assert_eq!(snap.root.canonicalize().unwrap(), root.canonicalize().unwrap());
        assert_eq!(snap.prefix, "");
        assert_eq!(snap.status.branch.as_deref(), Some("main"));
        assert!(snap.status.files.iter().all(|f| f.work == Some(Change::Untracked) && !f.staged()));
        assert!(snap.log.is_empty());

        // Köa a.txt, ta ur kön igen (fungerar även utan HEAD), köa och checka in.
        run_steps(&root, &[stage_step(&["a.txt".into()])]).unwrap();
        let f = status_of(&root).files.into_iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((f.index, f.work), (Some(Change::Added), None));
        run_steps(&root, &[unstage_step(&["a.txt".into()])]).unwrap();
        let f = status_of(&root).files.into_iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((f.index, f.work), (None, Some(Change::Untracked)));
        run_steps(&root, &[stage_step(&["a.txt".into()]), commit_step("Första\n\nlång text")]).unwrap();
        let snap = snapshot(&root).unwrap();
        assert_eq!(snap.log.len(), 1);
        assert_eq!(snap.log[0].subject, "Första");
        assert_eq!(snap.log[0].author, "Test Person");
        assert_eq!(snap.status.files.len(), 1, "bara sub/ är ospårad: {:?}", snap.status.files);

        // Ändra, köa, ta ur kön.
        std::fs::write(root.join("a.txt"), "ett\nändrad\n").unwrap();
        let f = status_of(&root).files.into_iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((f.index, f.work), (None, Some(Change::Modified)));
        run_steps(&root, &[stage_step(&["a.txt".into()])]).unwrap();
        let f = status_of(&root).files.into_iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((f.index, f.work), (Some(Change::Modified), None));
        run_steps(&root, &[unstage_step(&["a.txt".into()])]).unwrap();
        let f = status_of(&root).files.into_iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((f.index, f.work), (None, Some(Change::Modified)));

        // Checka in utan något köat ger git:s eget fel.
        let e = run_steps(&root, &[commit_step("tom")]).unwrap_err();
        assert!(!e.is_empty());

        // Förkasta: ändrad fil återställs, ospårad fil tas bort, det köade lämnas orört.
        std::fs::write(root.join("ny.txt"), "x").unwrap();
        std::fs::write(root.join("a.txt"), "ett\nköad\n").unwrap();
        run_steps(&root, &[stage_step(&["a.txt".into()])]).unwrap();
        std::fs::write(root.join("a.txt"), "ett\nköad\nokönad\n").unwrap();
        let files = status_of(&root).files;
        let pick = |p: &str| files.iter().find(|f| f.path == p).unwrap().clone();
        let steps = discard_steps(&[pick("a.txt"), pick("sub/"), pick("ny.txt")]);
        run_steps(&root, &steps).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "ett\nköad\n");
        assert!(!root.join("sub").exists());
        assert!(!root.join("ny.txt").exists());
        let f = status_of(&root).files.into_iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((f.index, f.work), (Some(Change::Modified), None));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn branches_and_switching() {
        let Some(root) = repo("branch") else { return };
        std::fs::write(root.join("a.txt"), "ett\n").unwrap();
        run_steps(&root, &[stage_step(&["a.txt".into()]), commit_step("a")]).unwrap();
        run_steps(&root, &[create_branch_step("feat")]).unwrap();
        let s = snapshot(&root).unwrap();
        assert_eq!(s.status.branch.as_deref(), Some("feat"));
        assert_eq!(s.branches, vec!["feat".to_owned(), "main".to_owned()]);
        run_steps(&root, &[switch_step("main")]).unwrap();
        assert_eq!(snapshot(&root).unwrap().status.branch.as_deref(), Some("main"));
        assert!(run_steps(&root, &[switch_step("finns-inte")]).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn head_text_and_markers_from_real_repo() {
        let Some(root) = repo("head") else { return };
        std::fs::create_dir(root.join("d")).unwrap();
        std::fs::write(root.join("d").join("f.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(root.join(".gitignore"), "skip.txt\n").unwrap();
        // Ingen incheckning än: filen jämförs mot tom text.
        assert_eq!(head_text(&root.join("d"), "f.txt").as_deref(), Some(""));
        run_steps(&root, &[stage_step(&[".".into()]), commit_step("start")]).unwrap();
        let base = head_text(&root.join("d"), "f.txt").unwrap();
        assert_eq!(base, "a\nb\nc\n");
        std::fs::write(root.join("skip.txt"), "x").unwrap();
        assert_eq!(head_text(&root, "skip.txt"), None);
        let h = hunks(&base, "a\nB\nc\nd\n");
        assert_eq!(h.iter().map(|h| h.kind).collect::<Vec<_>>(), vec![HunkKind::Modified, HunkKind::Added]);
        // Utanför ett arkiv: ingen jämförelse.
        let outside = tmp("outside");
        assert_eq!(head_text(&outside, "f.txt"), None);
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn blame_of_buffer() {
        let Some(root) = repo("blame") else { return };
        std::fs::write(root.join("f.txt"), "ett\ntvå\n").unwrap();
        run_steps(&root, &[stage_step(&["f.txt".into()]), commit_step("Skapa filen")]).unwrap();
        let b = blame(&root, "f.txt", "ett\ntvå\nnyss skriven\n").unwrap();
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].author, "Test Person");
        assert_eq!(b[0].summary, "Skapa filen");
        assert!(!b[1].uncommitted());
        assert!(b[2].uncommitted());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn subfolder_snapshot_has_prefix() {
        let Some(root) = repo("prefix") else { return };
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("x.txt"), "x").unwrap();
        let s = snapshot(&root.join("sub")).unwrap();
        assert_eq!(s.prefix, "sub/");
        assert_eq!(s.root.canonicalize().unwrap(), root.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }
}
