//! Utskrift: dialogruta (Ctrl+P) och hjälpfunktioner för CUPS (`lpstat`/`lp`) på Linux och macOS
//! samt PowerShell på Windows. Dokumentet renderas till en tillfällig PDF med `pdf_write`
//! som skickas till `lp` (Windows: PDF-läsarens utskriftsverb).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, SystemTime};

use egui::{Align, Color32, FontId, Frame, Id, Key, Layout, Margin, RichText, Stroke, TextEdit, vec2};

use crate::platform;
use crate::app::{KladdApp, quiet_button};
use crate::doc::{Document, resolve_image};
use crate::theme::{self, Palette};

/// Tillfälliga PDF-filer som är äldre än så här tas bort.
const MAX_AGE: Duration = Duration::from_secs(600);

/// Windows kan inte styra kopior, sidor eller dubbelsidigt via skalets utskriftsverb.
const WINDOWS: bool = cfg!(windows);

type Printers = (Vec<String>, Option<String>);

/// Dialogens tillstånd.
struct PrintDialog {
    /// `None` medan skrivarna söks.
    printers: Option<Printers>,
    lookup: Option<Receiver<Printers>>,
    printer: String,
    copies: u32,
    all_pages: bool,
    range: String,
    duplex: bool,
}

/// Fältet i `KladdApp`.
#[derive(Default)]
pub struct PrintState {
    dialog: Option<PrintDialog>,
    /// Svar från `lp`-tråden: Ok(skrivare) eller felmeddelande.
    result: Option<Receiver<Result<String, String>>>,
}

impl PrintState {
    pub fn is_open(&self) -> bool {
        self.dialog.is_some()
    }
}

impl PrintDialog {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(find_printers());
            ctx.request_repaint();
        });
        Self {
            printers: None,
            lookup: Some(rx),
            printer: String::new(),
            copies: 1,
            all_pages: true,
            range: String::new(),
            duplex: false,
        }
    }
}

// ------------------------------------------------------------ rena hjälpfunktioner

/// Skrivarnamn ur `lpstat -e` (ett per rad).
pub fn parse_printers(out: &str) -> Vec<String> {
    out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
}

/// Standardskrivaren ur `lpstat -d` ("system default destination: NAME").
pub fn parse_default(out: &str) -> Option<String> {
    let rest = out.lines().find_map(|l| l.split_once("default destination:"))?.1.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Radlista från PowerShell (ett skrivarnamn per rad), `Get-Printer | Select -ExpandProperty Name`.
/// Samma format som `lpstat -e`.
pub fn parse_ps_printers(out: &str) -> Vec<String> {
    parse_printers(out)
}

/// Text i ett PowerShell-literal med enkla citattecken (`'` dubbleras).
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// PowerShell-skriptet som skriver ut PDF:en. Utan skrivare används standardskrivaren (`Print`).
pub fn ps_print_script(printer: Option<&str>, file: &Path) -> String {
    let f = ps_quote(&file.to_string_lossy());
    match printer {
        Some(p) => {
            // PrintTo vill ha skrivarnamnet inom dubbla citattecken.
            let arg = ps_quote(&format!("\"{p}\""));
            format!("Start-Process -FilePath {f} -Verb PrintTo -ArgumentList {arg}")
        }
        None => format!("Start-Process -FilePath {f} -Verb Print"),
    }
}

/// Sidintervall som "1-3,5": bara siffror, kommatecken och bindestreck, med giltiga delar.
pub fn valid_range(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    let num = |x: &str| x.trim().parse::<u32>().ok().filter(|n| *n > 0 && x.trim().bytes().all(|c| c.is_ascii_digit()));
    s.split(',').all(|part| match part.split_once('-') {
        None => num(part).is_some(),
        Some((a, b)) => matches!((num(a), num(b)), (Some(a), Some(b)) if a <= b),
    })
}

/// Argumentlistan till `lp`.
pub fn lp_args(printer: &str, copies: u32, range: Option<&str>, duplex: bool, title: &str, file: &Path) -> Vec<String> {
    let mut a = vec!["-d".to_string(), printer.to_string(), "-n".into(), copies.clamp(1, 99).to_string()];
    if let Some(r) = range {
        a.push("-P".into());
        a.push(r.split_whitespace().collect());
    }
    if duplex {
        a.push("-o".into());
        a.push("sides=two-sided-long-edge".into());
    }
    a.push("-t".into());
    a.push(title.to_string());
    a.push(file.to_string_lossy().into_owned());
    a
}

/// Lägger text i ett kodblock (staket längre än alla backtick-följder i texten) så att den skrivs ut ordagrant.
pub fn fence_code(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}\n{}\n{fence}\n", text.trim_end_matches('\n'))
}

/// Markdown att rendera för dokumentet: Word/PDF/Markdown som det är, övrig text som kodblock.
fn printable_text(doc: &Document) -> String {
    if doc.is_markdown() { doc.text.clone() } else { fence_code(&doc.text) }
}

// ------------------------------------------------------------ system

fn run_out(cmd: &str, args: &[&str]) -> Option<String> {
    let out = platform::quiet(Command::new(cmd).args(args)).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn powershell(script: &str) -> Command {
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    c
}

fn run_ps(script: &str) -> Option<String> {
    let out = platform::quiet(&mut powershell(script)).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn find_printers() -> Printers {
    if WINDOWS {
        let list = run_ps("Get-Printer | Select-Object -ExpandProperty Name").map(|o| parse_ps_printers(&o)).unwrap_or_default();
        let default = run_ps("(Get-CimInstance Win32_Printer -Filter \"Default=true\").Name")
            .and_then(|o| parse_printers(&o).into_iter().next());
        return (list, default);
    }
    let list = run_out("lpstat", &["-e"]).map(|o| parse_printers(&o)).unwrap_or_default();
    let default = run_out("lpstat", &["-d"]).and_then(|o| parse_default(&o));
    (list, default)
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("kladd-print")
}

/// Skriver dokumentet till en tillfällig PDF och rensar gamla.
fn render_pdf(doc: &Document) -> Result<PathBuf, String> {
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let age = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
            if age.is_some_and(|d| d > MAX_AGE) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let title = doc.title();
    // En PDF i visaren skrivs ut som originalfilen, inte som en omrendering av texten.
    let bytes = match &doc.pdfv {
        Some(tab) => {
            // Osparade ändringar (sidor, anteckningar, formulärvärden) ska med på pappret.
            let base = if tab.unsaved() { tab.edited_bytes().unwrap_or_else(|_| tab.view.bytes().as_ref().clone()) } else { tab.view.bytes().as_ref().clone() };
            match tab.password.as_deref() {
                // Skrivaren kan inte lösenordet: skriv ut en avkrypterad kopia.
                Some(pw) => decrypted_pdf(&base, pw).unwrap_or(base),
                None => base,
            }
        }
        None => {
            let base = doc.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf).unwrap_or_default();
            let images = &doc.images;
            crate::pdf_write::write(&printable_text(doc), &title, &|src| resolve_image(src, &base, images))?.bytes
        }
    };
    let stem: String = title.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = dir.join(format!("{stem}-{stamp}.pdf"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// En lösenordsskyddad PDF utan kryptering, för utskrift. None om det inte går.
pub fn decrypted_pdf(bytes: &[u8], password: &str) -> Option<Vec<u8>> {
    let mut opts = lopdf::LoadOptions::default();
    opts.password = Some(password.to_owned());
    let mut doc = lopdf::Document::load_mem_with_options(bytes, opts).ok()?;
    if doc.is_encrypted() {
        doc.decrypt(password).ok()?;
    }
    doc.trailer.remove(b"Encrypt");
    doc.encryption_state = None;
    let mut out = Vec::new();
    doc.save_to(&mut out).ok()?;
    Some(out)
}

// ------------------------------------------------------------ dialog

enum Action {
    Print,
    OpenPdf,
    Cancel,
}

impl KladdApp {
    /// Öppnar utskriftsdialogen för aktiv flik.
    pub(crate) fn open_print(&mut self, ctx: &egui::Context) {
        if self.docs.is_empty() || self.print.dialog.is_some() {
            return;
        }
        self.print.dialog = Some(PrintDialog::new(ctx));
    }

    pub(crate) fn print_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        if let Some(rx) = &self.print.result
            && let Ok(res) = rx.try_recv()
        {
            self.print.result = None;
            match res {
                Ok(name) => self.notify(ctx, format!("Skickat till {name}"), false),
                Err(e) => self.notify(ctx, e, true),
            }
        }
        let Some(d) = self.print.dialog.as_mut() else { return };
        if let Some(rx) = &d.lookup
            && let Ok(found) = rx.try_recv()
        {
            d.printer = found.1.clone().filter(|n| found.0.contains(n)).or_else(|| found.0.first().cloned()).unwrap_or_default();
            d.printers = Some(found);
            d.lookup = None;
        }
        let have_printer = d.printers.as_ref().is_some_and(|(l, _)| !l.is_empty());
        let range_ok = WINDOWS || d.all_pages || valid_range(&d.range);
        let mut action = None;
        let modal = egui::Modal::new(Id::unique("kladd-print"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 140 } else { 70 }))
            .frame(
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(14)
                    .inner_margin(Margin::same(24))
                    .shadow(ctx.global_style().visuals.window_shadow),
            )
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new("Skriv ut").font(FontId::new(17.0, theme::semibold())).color(p.text));
                ui.add_space(14.0);
                egui::Grid::new("kladd-print-grid").num_columns(2).spacing(vec2(14.0, 10.0)).show(ui, |ui| {
                    ui.label(RichText::new("Skrivare").color(p.muted));
                    match &d.printers {
                        None => {
                            ui.label(RichText::new("Söker skrivare…").color(p.muted));
                        }
                        Some((list, _)) if list.is_empty() => {
                            ui.label(RichText::new("Ingen skrivare hittades").color(p.error));
                        }
                        Some((list, _)) => {
                            egui::ComboBox::from_id_salt("kladd-print-printer")
                                .selected_text(d.printer.clone())
                                .width(240.0)
                                .show_ui(ui, |ui| {
                                    for name in list {
                                        ui.selectable_value(&mut d.printer, name.clone(), name);
                                    }
                                });
                        }
                    }
                    ui.end_row();
                    // På Windows väljs kopior och sidor i PDF-läsarens utskriftsdialog.
                    let hint = "Välj kopior och sidor i PDF-läsarens utskriftsdialog";
                    ui.label(RichText::new("Kopior").color(p.muted));
                    ui.add_enabled(!WINDOWS, egui::DragValue::new(&mut d.copies).range(1..=99).speed(0.2))
                        .on_disabled_hover_text(hint);
                    ui.end_row();
                    ui.label(RichText::new("Sidor").color(p.muted));
                    ui.add_enabled_ui(!WINDOWS, |ui| {
                      ui.horizontal(|ui| {
                        ui.radio_value(&mut d.all_pages, true, "Alla");
                        ui.radio_value(&mut d.all_pages, false, "Sidor:");
                        ui.add_enabled(
                            !d.all_pages,
                            TextEdit::singleline(&mut d.range).hint_text("1-3,5").desired_width(110.0).margin(Margin::symmetric(8, 5)),
                        );
                      });
                    })
                    .response
                    .on_disabled_hover_text(hint);
                    ui.end_row();
                    ui.label("");
                    ui.add_enabled(!WINDOWS, egui::Checkbox::new(&mut d.duplex, "Dubbelsidigt")).on_disabled_hover_text(hint);
                    ui.end_row();
                });
                if !range_ok && !d.range.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Ogiltigt sidintervall. Använd t.ex. 1-3,5.").color(p.error));
                }
                ui.add_space(20.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let print = egui::Button::new(
                        RichText::new("Skriv ut").font(FontId::new(13.5, theme::medium())).color(p.on_accent),
                    )
                    .fill(p.accent)
                    .stroke(Stroke::NONE)
                    .min_size(vec2(84.0, 32.0));
                    let can = have_printer && range_ok;
                    if ui.add_enabled(can, print).clicked() || (can && ui.input(|i| i.key_pressed(Key::Enter))) {
                        action = Some(Action::Print);
                    }
                    if ui.add(quiet_button("Öppna som PDF", p).min_size(vec2(0.0, 32.0))).clicked() {
                        action = Some(Action::OpenPdf);
                    }
                    if ui.add(quiet_button("Avbryt", p).min_size(vec2(0.0, 32.0))).clicked() {
                        action = Some(Action::Cancel);
                    }
                });
            });
        if modal.should_close() && action.is_none() {
            action = Some(Action::Cancel);
        }
        let Some(action) = action else { return };
        let Some(d) = self.print.dialog.take() else { return };
        if matches!(action, Action::Cancel) {
            return;
        }
        let Some(doc) = self.docs.get(self.active) else { return };
        let title = doc.title();
        let pdf = match render_pdf(doc) {
            Ok(f) => f,
            Err(e) => return self.notify(ctx, format!("Kunde inte skapa PDF: {e}"), true),
        };
        if matches!(action, Action::OpenPdf) {
            if let Err(e) = platform::open(&pdf) {
                self.notify(ctx, format!("Kunde inte öppna PDF: {e}"), true);
            }
            return;
        }
        let range = (!d.all_pages).then(|| d.range.clone());
        let args = lp_args(&d.printer, d.copies, range.as_deref(), d.duplex, &title, &pdf);
        let printer = d.printer.clone();
        let (tx, rx) = channel();
        self.print.result = Some(rx);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let out = if WINDOWS {
                // Windows: PDF-läsarens utskriftsverb; utan vald skrivare används standardskrivaren.
                let sel = (!printer.is_empty()).then_some(printer.as_str());
                platform::quiet(&mut powershell(&ps_print_script(sel, &pdf))).output()
            } else {
                platform::quiet(Command::new("lp").args(&args)).output()
            };
            let res = match out {
                Ok(o) if o.status.success() => Ok(printer),
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    Err(if err.is_empty() { "Utskriften misslyckades".to_string() } else { err })
                }
                Err(e) => Err(format!("Kunde inte skriva ut: {e}")),
            };
            let _ = tx.send(res);
            repaint.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn decrypted_copy_opens_without_password() {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/krypterad.pdf")).unwrap();
        let ctx = egui::Context::default();
        assert!(crate::pdfview::engine::PdfEngine::open(std::sync::Arc::new(bytes.clone()), None, ctx.clone()).is_err());
        let plain = decrypted_pdf(&bytes, "hemligt").expect("avkrypterad kopia");
        let engine = crate::pdfview::engine::PdfEngine::open(std::sync::Arc::new(plain), None, ctx).expect("öppnas utan lösenord");
        assert!(engine.page_count() > 0);
        assert!(crate::pdf_read::read(&decrypted_pdf(&bytes, "hemligt").unwrap(), None).ok().unwrap().markdown.trim().len() > 0);
    }

    use super::*;

    #[test]
    fn parses_lpstat() {
        assert_eq!(parse_printers("HP_1\n\n Kontor \n"), vec!["HP_1", "Kontor"]);
        assert!(parse_printers("").is_empty());
        assert_eq!(parse_default("system default destination: HP_1\n").as_deref(), Some("HP_1"));
        assert_eq!(parse_default("no system default destination\n"), None);
    }

    #[test]
    fn builds_powershell_print() {
        let f = Path::new("C:\\t\\a b.pdf");
        assert_eq!(ps_print_script(None, f), "Start-Process -FilePath 'C:\\t\\a b.pdf' -Verb Print");
        assert_eq!(
            ps_print_script(Some("Kontor's"), f),
            "Start-Process -FilePath 'C:\\t\\a b.pdf' -Verb PrintTo -ArgumentList '\"Kontor''s\"'"
        );
    }

    #[test]
    fn validates_ranges() {
        for ok in ["1", "1-3,5", "2-2", " 1 , 4-6 "] {
            assert!(valid_range(ok), "{ok}");
        }
        for bad in ["", "a", "1-", "-3", "3-1", "1,,2", "0", "1-2-3", "1;2", "1,"] {
            assert!(!valid_range(bad), "{bad}");
        }
    }

    #[test]
    fn builds_lp_args() {
        let a = lp_args("HP", 2, Some("1-3, 5"), true, "Brev", Path::new("/t/a.pdf"));
        assert_eq!(a, ["-d", "HP", "-n", "2", "-P", "1-3,5", "-o", "sides=two-sided-long-edge", "-t", "Brev", "/t/a.pdf"]);
        let b = lp_args("HP", 1, None, false, "X", Path::new("f"));
        assert_eq!(b, ["-d", "HP", "-n", "1", "-t", "X", "f"]);
    }

    #[test]
    fn fences_longer_than_backticks() {
        assert_eq!(fence_code("a\n"), "```\na\n```\n");
        assert!(fence_code("x ```` y").starts_with("`````\n"));
    }
}
