//! Uppdateringskontroll: frågar GitHub efter senaste utgåvan (högst en gång per dygn och när man väljer
//! "Sök efter uppdateringar"), visar ett diskret fält när en nyare version finns och hämtar
//! installationsfilen för det aktuella systemet. Inget körs utan att användaren klickar.
//!
//! Nätverksdelen (`fetch_latest`, `download`) blockerar och körs i bakgrundstrådar. Resten, som
//! versionsjämförelse, tolkning av utgåvan, val av fil och tidsspärr, är rena funktioner med tester.

use std::cmp::Ordering;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::{Align, Color32, FontId, Frame, Id, Key, Layout, Margin, Panel, RichText, Stroke, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde::Deserialize;

use crate::app::{Settings, quiet_button};
use crate::i18n::t;
use crate::tf;
use crate::theme::{self, Palette};

const API_URL: &str = "https://api.github.com/repos/felixarvid2/kladd-text-editor/releases/latest";
/// Den här versionen av Kladd.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Minsta tid mellan två automatiska kontroller.
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
const BANNER_H: f32 = 36.0;

// ------------------------------------------------------------------ versioner

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    nums: [u64; 3],
    /// Förutgåva efter bindestreck ("rc.1"); en sådan är äldre än samma version utan.
    pre: Option<String>,
}

/// Tolkar "1.10.0", "v1.10", "1.2.3-rc.1" och "1.2.3+bygge". Saknade delar räknas som 0.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let s = s.split('+').next()?;
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p.to_owned())),
        None => (s, None),
    };
    let mut nums = [0u64; 3];
    let mut n = 0;
    for part in core.split('.') {
        if n == 3 {
            return None;
        }
        nums[n] = part.parse().ok()?;
        n += 1;
    }
    (n > 0).then_some(Version { nums, pre })
}

fn cmp_pre(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.split('.'), b.split('.'));
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.nums.cmp(&other.nums).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => cmp_pre(a, b),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Är `candidate` en nyare version än `current`? Otolkbara versioner räknas aldrig som nyare.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

// ------------------------------------------------------------------ utgåvan

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Asset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    /// Versionen utan inledande "v".
    pub fn version(&self) -> String {
        self.tag_name.trim().trim_start_matches(['v', 'V']).to_owned()
    }

    fn notes(&self) -> &str {
        self.body.as_deref().map(str::trim).filter(|b| !b.is_empty()).unwrap_or(t("Inga utgåvanteckningar."))
    }
}

pub fn parse_release(json: &[u8]) -> Result<Release, String> {
    serde_json::from_slice(json).map_err(|e| tf!("kunde inte tolka svaret från GitHub: {e}", e = e))
}

// ------------------------------------------------------------------ installationssätt

/// Hur den här installationen av Kladd uppdateras.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Windows,
    Mac,
    /// Installerad från .deb (Ubuntu/Debian).
    Deb,
    AppImage,
    /// Okänt: öppna utgåvans webbsida.
    Other,
}

pub fn detect_target() -> Target {
    if cfg!(windows) {
        Target::Windows
    } else if cfg!(target_os = "macos") {
        Target::Mac
    } else if std::env::var_os("APPIMAGE").is_some_and(|p| Path::new(&p).is_file()) {
        Target::AppImage
    } else if installed_from_deb() {
        Target::Deb
    } else {
        Target::Other
    }
}

fn installed_from_deb() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    if !exe.starts_with("/usr/bin") {
        return false;
    }
    Command::new("dpkg").arg("-S").arg(&exe).output().is_ok_and(|o| o.status.success())
}

/// Plattformens namn på processorarkitekturen i filnamn: (AppImage/Rust, .deb).
fn arch_names(arch: &str) -> (&'static str, &'static str) {
    match arch {
        "aarch64" | "arm64" => ("aarch64", "arm64"),
        _ => ("x86_64", "amd64"),
    }
}

/// Väljer filen som ska hämtas för `target`. `arch` är `std::env::consts::ARCH`.
pub fn pick_asset<'a>(assets: &'a [Asset], target: Target, version: &str, arch: &str) -> Option<&'a Asset> {
    let (arch_a, arch_deb) = arch_names(arch);
    let lower = |a: &Asset| a.name.to_lowercase();
    let find = |pred: &dyn Fn(&str) -> bool| assets.iter().find(|a| pred(&lower(a)));
    match target {
        Target::Windows => {
            let exact = format!("Kladd-{version}-windows-x64-setup.exe");
            assets
                .iter()
                .find(|a| a.name == exact)
                .or_else(|| find(&|n| n.ends_with("setup.exe") && n.contains("win")))
                .or_else(|| find(&|n| n.ends_with(".exe") && n.contains("setup")))
        }
        Target::Mac => {
            let aliases: &[&str] = if arch_a == "aarch64" { &["arm64", "aarch64"] } else { &["x86_64", "x64", "intel"] };
            find(&|n| n.ends_with(".dmg") && aliases.iter().any(|a| n.contains(a)))
                .or_else(|| find(&|n| n.ends_with(".dmg") && n.contains("universal")))
                .or_else(|| find(&|n| n.ends_with(".dmg")))
        }
        Target::Deb => find(&|n| n.ends_with(".deb") && n.contains(arch_deb)).or_else(|| find(&|n| n.ends_with(".deb"))),
        Target::AppImage => {
            let exact = format!("Kladd-{version}-{arch_a}.AppImage");
            assets
                .iter()
                .find(|a| a.name == exact)
                .or_else(|| find(&|n| n.ends_with(".appimage") && n.contains(arch_a)))
        }
        Target::Other => None,
    }
}

// ------------------------------------------------------------------ tidsspärr

/// Är det dags för en automatisk kontroll? `last` och `now` är sekunder sedan 1970 (0 = aldrig).
pub fn due(last: u64, now: u64) -> bool {
    last == 0 || now < last || now - last >= CHECK_INTERVAL_SECS
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ------------------------------------------------------------------ nätverk

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(format!("Kladd/{CURRENT}"))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// Hämtar senaste utgåvan (blockerar).
pub fn fetch_latest() -> Result<Release, String> {
    let mut resp = agent()
        .get(API_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| tf!("kunde inte nå GitHub: {e}", e = e))?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| tf!("kunde inte läsa svaret från GitHub: {e}", e = e))?;
    parse_release(&bytes)
}

/// Framsteg för en pågående hämtning; delas med gränssnittet.
#[derive(Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

/// Hämtar `asset` till `dest` via en tillfällig `.part`-fil som döps om när allt är hämtat och
/// storleken stämmer (blockerar; kan avbrytas via `prog`).
pub fn download(asset: &Asset, dest: &Path, prog: &Progress) -> Result<(), String> {
    if !asset.browser_download_url.starts_with("https://") {
        return Err(t("hämtningsadressen är inte https").into());
    }
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let result = download_inner(asset, &part, prog).and_then(|()| {
        std::fs::rename(&part, dest).map_err(|e| tf!("kunde inte spara {path}: {e}", path = dest.display(), e = e))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

fn download_inner(asset: &Asset, part: &Path, prog: &Progress) -> Result<(), String> {
    let resp = agent().get(&asset.browser_download_url).call().map_err(|e| tf!("hämtningen misslyckades: {e}", e = e))?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(asset.size);
    prog.total.store(total, AtomicOrdering::Relaxed);
    prog.done.store(0, AtomicOrdering::Relaxed);
    let mut body = resp.into_body();
    let mut reader = body.with_config().limit(u64::MAX).reader();
    let mut file = std::fs::File::create(part).map_err(|e| tf!("kunde inte skapa {path}: {e}", path = part.display(), e = e))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        if prog.cancel.load(AtomicOrdering::Relaxed) {
            return Err("avbruten".into());
        }
        let n = reader.read(&mut buf).map_err(|e| tf!("hämtningen avbröts: {e}", e = e))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| tf!("kunde inte skriva filen: {e}", e = e))?;
        written += n as u64;
        prog.done.store(written, AtomicOrdering::Relaxed);
    }
    file.flush().map_err(|e| tf!("kunde inte skriva filen: {e}", e = e))?;
    if asset.size > 0 && written != asset.size {
        return Err(tf!("filen blev {written} byte men skulle vara {size}", written = written, size = asset.size));
    }
    Ok(())
}

/// Byter ut den körande AppImage-filen mot den nyhämtade `new` (som får körrättighet).
fn replace_appimage(new: &Path, old: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| tf!("kunde inte göra filen körbar: {e}", e = e))?;
    }
    std::fs::rename(new, old).map_err(|e| tf!("kunde inte ersätta {path}: {e}", path = old.display(), e = e))
}

/// Ett filnamn utan sökväg, så att en utgåva aldrig kan skriva utanför målmappen.
fn safe_file_name(name: &str) -> Option<String> {
    let n = Path::new(name).file_name()?.to_string_lossy().into_owned();
    (!n.is_empty() && n != "." && n != "..").then_some(n)
}

// ------------------------------------------------------------------ tillstånd och gränssnitt

type CheckResult = (bool, Result<Release, String>);

/// Vad som ska köras när Kladd avslutas (efter kontrollen av osparade ändringar).
pub enum Launch {
    /// Öppna med systemet (installationsprogram).
    Open(PathBuf),
    /// Starta programfilen i ett nytt fönster (omstart efter AppImage-uppdatering).
    Restart(PathBuf),
}

/// Önskemål från uppdateringsdelen till appen.
pub enum Out {
    Notify(String, bool),
    Quit,
}

struct Dl {
    version: String,
    target: Target,
    dest: PathBuf,
    prog: Arc<Progress>,
    rx: Receiver<Result<(), String>>,
}

struct Ready {
    version: String,
    target: Target,
    path: PathBuf,
}

#[derive(Default)]
pub struct State {
    check_rx: Option<Receiver<CheckResult>>,
    auto_tried: bool,
    /// Nyare utgåva som hittats.
    available: Option<Release>,
    /// "Senare": fältet göms tills nästa start.
    hidden: bool,
    news_open: bool,
    cache: CommonMarkCache,
    dl: Option<Dl>,
    ready: Option<Ready>,
    launch_on_exit: Option<Launch>,
    outs: Vec<Out>,
}

impl State {
    pub fn take_outs(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.outs)
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.outs.push(Out::Notify(text.into(), error));
    }

    /// "Sök efter uppdateringar" i menyn.
    pub fn check_now(&mut self, ctx: &egui::Context) {
        if self.check_rx.is_none() {
            self.start_check(ctx, true);
            self.notify(t("Söker efter uppdateringar…"), false);
        }
    }

    fn start_check(&mut self, ctx: &egui::Context, manual: bool) {
        let (tx, rx) = channel();
        self.check_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((manual, fetch_latest()));
            ctx.request_repaint();
        });
    }

    /// Körs varje bildruta: startar den dagliga kontrollen och tar emot färdiga svar.
    pub fn tick(&mut self, ctx: &egui::Context, settings: &mut Settings) {
        if settings.auto_update_check && !self.auto_tried && self.check_rx.is_none() && due(settings.update_last_check, unix_now()) {
            self.auto_tried = true;
            self.start_check(ctx, false);
        }
        if let Some(rx) = &self.check_rx {
            if let Ok((manual, result)) = rx.try_recv() {
                self.check_rx = None;
                self.on_checked(manual, result, settings);
            }
        }
        let finished = self.dl.as_ref().and_then(|d| d.rx.try_recv().ok());
        if let Some(result) = finished {
            let dl = self.dl.take().expect("hämtning pågår");
            self.on_downloaded(dl, result);
        } else if self.dl.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_checked(&mut self, manual: bool, result: Result<Release, String>, settings: &mut Settings) {
        match result {
            Ok(rel) => {
                settings.update_last_check = unix_now();
                let v = rel.version();
                if !is_newer(&v, CURRENT) {
                    if manual {
                        self.notify(tf!("Kladd är redan uppdaterat (version {version})", version = CURRENT), false);
                    }
                } else if manual || settings.update_skip.as_deref() != Some(v.as_str()) {
                    self.hidden = false;
                    self.available = Some(rel);
                }
            }
            Err(e) if manual => self.notify(tf!("Uppdateringskontrollen misslyckades: {e}", e = e), true),
            Err(e) => eprintln!("kladd: uppdateringskontroll: {e}"),
        }
    }

    fn on_downloaded(&mut self, dl: Dl, result: Result<(), String>) {
        if let Err(e) = result {
            if e != "avbruten" {
                self.notify(tf!("Hämtningen misslyckades: {e}", e = e), true);
            }
            return;
        }
        let path = match dl.target {
            Target::AppImage => {
                let Some(old) = std::env::var_os("APPIMAGE").map(PathBuf::from) else {
                    self.notify(t("Kunde inte hitta den körande AppImage-filen"), true);
                    return;
                };
                if let Err(e) = replace_appimage(&dl.dest, &old) {
                    let _ = std::fs::remove_file(&dl.dest);
                    self.notify(e, true);
                    return;
                }
                old
            }
            // Dessa två öppnas direkt (användaren har just klickat på hämta).
            Target::Mac | Target::Deb => {
                if let Err(e) = crate::platform::open(&dl.dest) {
                    self.notify(tf!("Kunde inte öppna {path}: {e}", path = dl.dest.display(), e = e), true);
                }
                dl.dest.clone()
            }
            _ => dl.dest.clone(),
        };
        self.available = None;
        self.ready = Some(Ready { version: dl.version, target: dl.target, path });
    }

    /// Användaren ångrade avslutet: kör inget installationsprogram vid nästa avslut.
    pub fn cancel_launch(&mut self) {
        self.launch_on_exit = None;
    }

    /// Anropas när Kladd avslutas: startar installationsprogrammet eller den nya versionen.
    pub fn run_on_exit(&mut self) {
        match self.launch_on_exit.take() {
            Some(Launch::Open(p)) => {
                let _ = crate::platform::open(&p);
            }
            Some(Launch::Restart(p)) => {
                let mut cmd = Command::new(p);
                cmd.arg("--new-window");
                // Den nya AppImage-filen ska montera sig själv, inte ärva den gamlas variabler.
                for k in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
                    cmd.env_remove(k);
                }
                let _ = crate::platform::quiet(&mut cmd).spawn();
            }
            None => {}
        }
    }

    fn start_download(&mut self, ctx: &egui::Context, rel: &Release) {
        let target = detect_target();
        let version = rel.version();
        let Some(asset) = pick_asset(&rel.assets, target, &version, std::env::consts::ARCH) else {
            // Ingen passande fil (eller okänt installationssätt): visa utgåvans sida.
            self.open_release_page(rel);
            return;
        };
        let Some(name) = safe_file_name(&asset.name) else {
            self.notify(t("Ogiltigt filnamn i utgåvan"), true);
            return;
        };
        let dir = match target {
            Target::AppImage => std::env::var_os("APPIMAGE")
                .map(PathBuf::from)
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .unwrap_or_else(std::env::temp_dir),
            _ => std::env::temp_dir(),
        };
        // AppImage hämtas under annat namn bredvid den gamla och byter plats när allt är klart.
        let dest = if target == Target::AppImage { dir.join(format!("{name}.new")) } else { dir.join(&name) };
        let prog = Arc::new(Progress::default());
        prog.total.store(asset.size, AtomicOrdering::Relaxed);
        let (tx, rx) = channel();
        let (a, p, c) = (asset.clone(), prog.clone(), ctx.clone());
        let d = dest.clone();
        std::thread::spawn(move || {
            let _ = tx.send(download(&a, &d, &p));
            c.request_repaint();
        });
        self.dl = Some(Dl { version, target, dest, prog, rx });
    }

    fn open_release_page(&mut self, rel: &Release) {
        if !rel.html_url.starts_with("https://github.com/") {
            self.notify(t("Ogiltig adress till utgåvan"), true);
            return;
        }
        if let Err(e) = crate::platform::open(Path::new(&rel.html_url)) {
            self.notify(tf!("Kunde inte öppna webbläsaren: {e}", e = e), true);
        }
    }

    /// Det diskreta fältet ovanför statusraden.
    pub fn banner(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, p: &Palette, settings: &mut Settings) {
        if self.hidden || self.dl.is_some() || self.ready.is_some() || self.news_open {
            return;
        }
        let Some(rel) = self.available.clone() else { return };
        let (mut news, mut get, mut later, mut skip) = (false, false, false, false);
        Panel::bottom("kladd-update")
            .exact_size(BANNER_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.surface).inner_margin(Margin::symmetric(14, 0)))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().hline((r.left() - 14.0)..=(r.right() + 14.0), r.top(), Stroke::new(1.0, p.line));
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(
                        RichText::new(tf!("Kladd {version} finns", version = rel.version()))
                            .font(FontId::new(13.5, theme::medium()))
                            .color(p.accent),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if ui.add(quiet_button(t("Hoppa över"), p)).clicked() {
                            skip = true;
                        }
                        if ui.add(quiet_button(t("Senare"), p)).clicked() {
                            later = true;
                        }
                        if ui.add(quiet_button(t("Visa nyheter"), p)).clicked() {
                            news = true;
                        }
                        let b = egui::Button::new(
                            RichText::new(t("Hämta och installera")).font(FontId::new(13.0, theme::medium())).color(p.on_accent),
                        )
                        .fill(p.accent)
                        .stroke(Stroke::NONE);
                        if ui.add(b).clicked() {
                            get = true;
                        }
                    });
                });
            });
        if news {
            self.news_open = true;
        }
        if get {
            self.start_download(ctx, &rel);
        }
        if later {
            self.hidden = true;
        }
        if skip {
            settings.update_skip = Some(rel.version());
            self.available = None;
        }
    }

    /// Nyhetsfönster, hämtningsfönster och "klart"-fönster.
    pub fn dialogs(&mut self, ctx: &egui::Context, p: &Palette, settings: &mut Settings) {
        self.news_dialog(ctx, p, settings);
        self.download_dialog(ctx, p);
        self.ready_dialog(ctx, p);
    }

    fn news_dialog(&mut self, ctx: &egui::Context, p: &Palette, settings: &mut Settings) {
        if !self.news_open {
            return;
        }
        let Some(rel) = self.available.clone() else {
            self.news_open = false;
            return;
        };
        let (mut get, mut close, mut skip) = (false, false, false);
        let cache = &mut self.cache;
        let modal = modal(ctx, p, "kladd-update-news").show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(
                RichText::new(tf!("Nyheter i Kladd {version}", version = rel.version()))
                    .font(FontId::new(17.0, theme::semibold()))
                    .color(p.text),
            );
            ui.add_space(10.0);
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                CommonMarkViewer::new().show(ui, cache, rel.notes());
            });
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.add(primary_button(t("Hämta och installera"), p)).clicked() {
                    get = true;
                }
                if ui.add(quiet_button(t("Stäng"), p).min_size(vec2(0.0, 32.0))).clicked() {
                    close = true;
                }
                if ui.add(quiet_button(t("Hoppa över den här versionen"), p).min_size(vec2(0.0, 32.0))).clicked() {
                    skip = true;
                }
            });
        });
        if modal.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        if get {
            self.news_open = false;
            self.start_download(ctx, &rel);
        } else if skip {
            self.news_open = false;
            settings.update_skip = Some(rel.version());
            self.available = None;
        } else if close {
            self.news_open = false;
        }
    }

    fn download_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(dl) = &self.dl else { return };
        let done = dl.prog.done.load(AtomicOrdering::Relaxed);
        let total = dl.prog.total.load(AtomicOrdering::Relaxed);
        let mb = |n: u64| n as f64 / (1024.0 * 1024.0);
        let mut cancel = false;
        modal(ctx, p, "kladd-update-download").show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                RichText::new(tf!("Hämtar Kladd {version}", version = dl.version)).font(FontId::new(17.0, theme::semibold())).color(p.text),
            );
            ui.add_space(12.0);
            let bar = if total > 0 {
                egui::ProgressBar::new((done as f32 / total as f32).clamp(0.0, 1.0))
                    .text(tf!("{done} av {total} MB", done = format!("{:.1}", mb(done)), total = format!("{:.1}", mb(total))))
            } else {
                egui::ProgressBar::new(0.0).animate(true).text(tf!("{done} MB", done = format!("{:.1}", mb(done))))
            };
            ui.add(bar.fill(p.accent));
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(quiet_button(t("Avbryt"), p).min_size(vec2(0.0, 32.0))).clicked() {
                    cancel = true;
                }
            });
        });
        if cancel || ctx.input(|i| i.key_pressed(Key::Escape)) {
            if let Some(dl) = self.dl.take() {
                // Tråden städar bort den halva filen; fältet visas igen eftersom utgåvan finns kvar.
                dl.prog.cancel.store(true, AtomicOrdering::Relaxed);
            }
        }
    }

    fn ready_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(r) = &self.ready else { return };
        let path = r.path.clone();
        let deb_cmd = format!("sudo apt install '{}'", path.display());
        let (mut primary, mut close) = (false, false);
        let (title, body, label): (String, String, Option<&str>) = match r.target {
            Target::Windows => (
                tf!("Kladd {version} är hämtad", version = r.version),
                t("Installationsprogrammet startar när Kladd avslutas. Osparade ändringar frågar Kladd efter först.").into(),
                Some(t("Installera och avsluta")),
            ),
            Target::Mac => (
                tf!("Kladd {version} är hämtad", version = r.version),
                t("Skivavbilden har öppnats. Dra Kladd till Program i fönstret som öppnades och starta om Kladd.").into(),
                None,
            ),
            Target::Deb => (
                tf!("Kladd {version} är hämtad", version = r.version),
                tf!("Paketet har öppnats i programinstallationen. Du kan också installera det i en terminal:\n\n{command}", command = deb_cmd),
                Some(t("Kopiera kommando")),
            ),
            Target::AppImage => (
                tf!("Kladd {version} är installerad", version = r.version),
                t("AppImage-filen har bytts ut. Starta om Kladd för att använda den nya versionen.").into(),
                Some(t("Starta om")),
            ),
            Target::Other => (tf!("Kladd {version} är hämtad", version = r.version), path.display().to_string(), None),
        };
        let target = r.target;
        let modal = modal(ctx, p, "kladd-update-ready").show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(title).font(FontId::new(17.0, theme::semibold())).color(p.text));
            ui.add_space(8.0);
            ui.label(RichText::new(body).color(p.muted));
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(label) = label {
                    if ui.add(primary_button(label, p)).clicked() {
                        primary = true;
                    }
                }
                let text = if label.is_some() { t("Senare") } else { t("Stäng") };
                if ui.add(quiet_button(text, p).min_size(vec2(0.0, 32.0))).clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)) {
            close = true;
        }
        if primary {
            match target {
                Target::Windows => {
                    self.launch_on_exit = Some(Launch::Open(path));
                    self.outs.push(Out::Quit);
                }
                Target::AppImage => {
                    self.launch_on_exit = Some(Launch::Restart(path));
                    self.outs.push(Out::Quit);
                }
                Target::Deb => {
                    ctx.copy_text(deb_cmd);
                    self.notify(t("Kommandot är kopierat"), false);
                    return;
                }
                _ => {}
            }
            self.ready = None;
        } else if close {
            self.ready = None;
        }
    }
}

fn modal(ctx: &egui::Context, p: &Palette, id: &'static str) -> egui::Modal {
    egui::Modal::new(Id::unique(id))
        .backdrop_color(Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
        .frame(
            Frame::NONE
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(14)
                .inner_margin(Margin::same(24))
                .shadow(ctx.global_style().visuals.window_shadow),
        )
}

fn primary_button(text: &str, p: &Palette) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).font(FontId::new(13.5, theme::medium())).color(p.on_accent))
        .fill(p.accent)
        .stroke(Stroke::NONE)
        .min_size(vec2(84.0, 32.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r###"{
        "url": "https://api.github.com/repos/felixarvid2/kladd-text-editor/releases/1",
        "tag_name": "v1.10.0",
        "name": "Kladd 1.10.0",
        "html_url": "https://github.com/felixarvid2/kladd-text-editor/releases/tag/v1.10.0",
        "draft": false,
        "body": "## Nytt\n\n- Uppdateringskontroll\n",
        "assets": [
            {"name": "Kladd-1.10.0-windows-x64-setup.exe", "size": 1234, "browser_download_url": "https://github.com/x/releases/download/v1.10.0/Kladd-1.10.0-windows-x64-setup.exe"},
            {"name": "Kladd-1.10.0-macos-arm64.dmg", "size": 10, "browser_download_url": "https://github.com/x/a.dmg"},
            {"name": "Kladd-1.10.0-macos-x86_64.dmg", "size": 11, "browser_download_url": "https://github.com/x/b.dmg"},
            {"name": "kladd_1.10.0-1_amd64.deb", "size": 12, "browser_download_url": "https://github.com/x/c.deb"},
            {"name": "kladd_1.10.0-1_arm64.deb", "size": 13, "browser_download_url": "https://github.com/x/d.deb"},
            {"name": "Kladd-1.10.0-x86_64.AppImage", "size": 14, "browser_download_url": "https://github.com/x/e.AppImage"}
        ]
    }"###;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(is_newer("v2.0.0", "1.99.99"));
        assert!(is_newer("1.9.1", "1.9"));
        assert!(!is_newer("1.9.0", "1.9.0"));
        assert!(!is_newer("1.8.9", "1.9.0"));
        assert!(!is_newer("skräp", "1.9.0"));
        assert!(!is_newer("1.10.0", "skräp"));
        assert!(!is_newer("1.2.3.4", "1.0.0"));
    }

    #[test]
    fn prerelease_is_older_than_release() {
        assert!(is_newer("1.10.0", "1.10.0-rc.1"));
        assert!(!is_newer("1.10.0-rc.1", "1.10.0"));
        assert!(is_newer("1.10.0-rc.2", "1.10.0-rc.1"));
        assert!(is_newer("1.10.0-rc.10", "1.10.0-rc.9"));
        assert!(!is_newer("1.10.0+bygge5", "1.10.0"));
    }

    #[test]
    fn current_version_parses() {
        assert!(parse_version(CURRENT).is_some());
    }

    #[test]
    fn parses_release() {
        let r = parse_release(SAMPLE.as_bytes()).unwrap();
        assert_eq!(r.tag_name, "v1.10.0");
        assert_eq!(r.version(), "1.10.0");
        assert!(r.html_url.ends_with("/v1.10.0"));
        assert!(r.notes().starts_with("## Nytt"));
        assert_eq!(r.assets.len(), 6);
        assert_eq!(r.assets[0].size, 1234);
    }

    #[test]
    fn release_without_body_or_assets() {
        let r = parse_release(br#"{"tag_name":"v2.0.0","html_url":"https://github.com/a/b","body":null}"#).unwrap();
        assert!(r.assets.is_empty());
        assert_eq!(r.notes(), "Inga utgåvanteckningar.");
        assert!(parse_release(b"inte json").is_err());
        assert!(parse_release(br#"{"message":"Not Found"}"#).is_err());
    }

    #[test]
    fn picks_asset_per_platform() {
        let r = parse_release(SAMPLE.as_bytes()).unwrap();
        let pick = |t, arch| pick_asset(&r.assets, t, "1.10.0", arch).map(|a| a.name.as_str());
        assert_eq!(pick(Target::Windows, "x86_64"), Some("Kladd-1.10.0-windows-x64-setup.exe"));
        assert_eq!(pick(Target::Mac, "aarch64"), Some("Kladd-1.10.0-macos-arm64.dmg"));
        assert_eq!(pick(Target::Mac, "x86_64"), Some("Kladd-1.10.0-macos-x86_64.dmg"));
        assert_eq!(pick(Target::Deb, "x86_64"), Some("kladd_1.10.0-1_amd64.deb"));
        assert_eq!(pick(Target::Deb, "aarch64"), Some("kladd_1.10.0-1_arm64.deb"));
        assert_eq!(pick(Target::AppImage, "x86_64"), Some("Kladd-1.10.0-x86_64.AppImage"));
        assert_eq!(pick(Target::AppImage, "aarch64"), None);
        assert_eq!(pick(Target::Other, "x86_64"), None);
    }

    #[test]
    fn missing_assets_give_none() {
        assert!(pick_asset(&[], Target::Windows, "1.10.0", "x86_64").is_none());
        let only_deb = [Asset { name: "kladd.deb".into(), size: 1, browser_download_url: "https://x".into() }];
        assert!(pick_asset(&only_deb, Target::Windows, "1.10.0", "x86_64").is_none());
        assert!(pick_asset(&only_deb, Target::Deb, "1.10.0", "aarch64").is_some());
    }

    #[test]
    fn daily_throttle() {
        let day = CHECK_INTERVAL_SECS;
        assert!(due(0, 1_000_000));
        assert!(!due(1_000_000, 1_000_000 + day - 1));
        assert!(due(1_000_000, 1_000_000 + day));
        // Klockan har ställts bakåt: kontrollera hellre än att vänta i åratal.
        assert!(due(2_000_000, 1_000_000));
    }

    #[test]
    fn file_names_cannot_escape() {
        assert_eq!(safe_file_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(safe_file_name("Kladd.dmg").as_deref(), Some("Kladd.dmg"));
        assert_eq!(safe_file_name(".."), None);
        assert_eq!(safe_file_name(""), None);
    }

    #[test]
    fn appimage_replacement_keeps_exec_bit() {
        let dir = std::env::temp_dir().join(format!("kladd-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (old, new) = (dir.join("Kladd.AppImage"), dir.join("Kladd.AppImage.new"));
        std::fs::write(&old, b"gammal").unwrap();
        std::fs::write(&new, b"ny").unwrap();
        replace_appimage(&new, &old).unwrap();
        assert_eq!(std::fs::read(&old).unwrap(), b"ny");
        assert!(!new.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&old).unwrap().permissions().mode() & 0o111, 0o111);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Mot det riktiga GitHub-API:t; kör med `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn real_api_answers() {
        let r = fetch_latest().expect("GitHub svarar");
        assert!(parse_version(&r.tag_name).is_some(), "tag: {}", r.tag_name);
    }
}
