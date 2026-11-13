//! Gemensam testmiljö: en `KladdApp` utan biverkningar i en `egui_kittest::Harness`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::app::KladdApp;

/// Fast fönsterstorlek så att layouten blir densamma överallt.
pub const WINDOW: egui::Vec2 = egui::vec2(1100.0, 760.0);

/// Tillfällig mapp som tas bort när den släpps.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("kladd-uitest-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("skapa testmapp");
        // Kanonisk sökväg: Kladd öppnar filer via `canonicalize` (på macOS är /var en länk till /private/var).
        Self(dir.canonicalize().unwrap_or(dir))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Skriver en fil i mappen och ger sökvägen.
    pub fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let p = self.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("skapa undermapp");
        }
        std::fs::write(&p, contents).expect("skriva testfil");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Pekar om användarmapparna till en gemensam tillfällig mapp, en gång per testprocess.
fn isolate_user_dirs() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let root = std::env::temp_dir().join(format!("kladd-uitest-home-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        crate::platform::set_test_root(root);
    });
}

/// Appen i en harness, plus mappen testet skriver sina filer i.
pub struct Ui {
    pub h: Harness<'static, KladdApp>,
    pub dir: TempDir,
    /// Gränssnittstesterna delar processglobalt tillstånd (testkatalogen, inställningsfilen,
    /// bakgrundstrådar som väcker appen) och körs därför en i taget; övriga tester körs parallellt.
    _serial: std::sync::MutexGuard<'static, ()>,
}

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Ui {
    /// Tom app (startskärmen visas) på svenska.
    pub fn new() -> Self {
        Self::with_files(&[])
    }

    /// App med filerna öppnade, som `kladd fil ...` men utan enkelinstanssocket.
    pub fn with_files(files: &[PathBuf]) -> Self {
        isolate_user_dirs();
        // Ett test som fallerar förgiftar låset; nästa test ska ändå få köra.
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new();
        let files = files.to_vec();
        let mut h = Harness::builder()
            .with_size(WINDOW)
            .with_pixels_per_point(1.0)
            .with_max_steps(30)
            // Editorn och en del egenritade knappar saknar tillgängligt namn; det är inte vad testerna prövar.
            .with_accessibility_check(false)
            // Systemtypsnitt läses inte in i testerna; tecken som saknas i de inbäddade ritas som tofu.
            .allow_missing_glyphs()
            // Fler steg än standard: bakgrundsarbete (typsnitt, git, LSP) kan ta några bilder under last.
            .with_max_steps(64)
            .build_eframe(move |cc| {
                // Ingen enkelinstans (inget uttag), standardinställningar utan sparad konfiguration.
                let mut app = KladdApp::new(cc, Vec::new(), false);
                // Testerna delar en tillfällig datakatalog och körs parallellt: en annan tests sparade
                // inställningar (arbetsmapp, delad vy, språk …) får inte läcka in. Börja alltid från standard.
                app.settings = crate::app::Settings::default();
                app.settings.lang = crate::i18n::LangSetting::Sv;
                crate::i18n::set_lang(crate::i18n::Lang::Sv);
                // Aldrig någon uppdateringskontroll i testerna.
                app.settings.auto_update_check = false;
                for f in &files {
                    app.open_path(&cc.egui_ctx, f);
                }
                app
            });
        // Bakgrundstrådar (git, språkservrar, typsnitt) kan be om fler bilder under start; under hög
        // belastning hinner de inte alltid bli klara inom `max_steps`, så starten får inte paniken.
        h.run_ok();
        h.run_steps(2);
        Self { h, dir, _serial: serial }
    }

    pub fn app(&self) -> &KladdApp {
        self.h.state()
    }

    pub fn app_mut(&mut self) -> &mut KladdApp {
        self.h.state_mut()
    }

    /// Trycker en tangent med modifierare (`Modifiers::COMMAND` = Ctrl, ⌘ på Mac).
    pub fn key(&mut self, m: egui::Modifiers, k: egui::Key) {
        self.h.key_press_modifiers(m, k);
        self.settle();
    }

    /// Klickar på elementet med exakt den här etiketten (knappars verktygstips är deras etikett).
    pub fn click(&mut self, label: &str) {
        // Långa menyer rullar: se till att knappen är synlig innan det klickas.
        self.h.get_by_label(label).scroll_to_me();
        self.settle();
        self.h.get_by_label(label).click();
        self.settle();
    }

    /// Klickar på det första elementet vars etikett innehåller texten.
    pub fn click_contains(&mut self, part: &str) {
        self.h.get_all_by_label_contains(part).next().unwrap_or_else(|| panic!("hittar inget med '{part}'")).scroll_to_me();
        self.settle();
        self.h.get_all_by_label_contains(part).next().unwrap_or_else(|| panic!("hittar inget med '{part}'")).click();
        self.settle();
    }

    /// Klickar på det n:e elementet med rollen (t.ex. den första rullgardinen).
    pub fn click_nth_role(&mut self, role: egui::accesskit::Role, n: usize) {
        self.h.get_all_by_role(role).nth(n).unwrap_or_else(|| panic!("inget {role:?} nummer {n}")).click();
        self.settle();
    }

    /// Finns ett element med exakt den här etiketten eller texten?
    pub fn has(&self, label: &str) -> bool {
        self.h.query_by_label(label).is_some()
    }

    /// Finns ett element vars etikett eller text innehåller `part`?
    pub fn has_contains(&self, part: &str) -> bool {
        self.h.query_all_by_label_contains(part).next().is_some()
    }

    /// Öppnar en fil som om den valts i Öppna-dialogen.
    pub fn open(&mut self, path: &Path) {
        let ctx = self.h.ctx.clone();
        self.h.state_mut().open_path(&ctx, path);
        self.settle();
    }

    /// Väntar (högst fem sekunder) på att ett element med exakt den här etiketten dyker upp, för
    /// arbete som sker i bakgrundstrådar. Egui får ingen väckning av testmiljön, så vi stegar själva.
    pub fn wait_for(&mut self, label: &str) {
        for _ in 0..500 {
            self.h.step();
            if self.has(label) {
                self.settle();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.dump();
        panic!("'{label}' dök aldrig upp");
    }

    /// Väntar (högst tio sekunder) tills villkoret på appens tillstånd gäller; för PDF-sidor och sökning
    /// som arbetar i bakgrundstrådar.
    pub fn wait_until(&mut self, what: &str, cond: impl Fn(&KladdApp) -> bool) {
        for _ in 0..1000 {
            self.h.step();
            if cond(self.app()) {
                self.settle();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("tiden gick ut: {what}");
    }

    /// Drar med musen från en punkt till en annan (tryck, flytta, släpp) och kör bilder emellan.
    pub fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.h.hover_at(from);
        self.h.step();
        self.h.drag_at(from);
        self.h.step();
        self.h.hover_at(to);
        self.h.step();
        self.h.drop_at(to);
        self.settle();
    }

    /// Klickar på det sista elementet med etiketten (t.ex. dialogknappen när en knapp med samma namn finns bakom).
    pub fn click_last(&mut self, label: &str) {
        self.h.get_all_by_label(label).last().unwrap_or_else(|| panic!("hittar inte '{label}'")).click();
        self.settle();
    }

    /// Fokuserar det n:e enradiga textfältet (0 = först i trädet), t.ex. ersättningsfältet i sökraden.
    pub fn focus_nth_input(&mut self, n: usize) {
        self.h.get_all_by_role(egui::accesskit::Role::TextInput).nth(n).expect("textfält saknas").focus();
        self.settle();
    }

    /// Skriver text i det fokuserade fältet.
    pub fn type_text(&mut self, text: &str) {
        self.h.event(egui::Event::Text(text.to_owned()));
        self.settle();
    }

    /// Flytter fokus till textredigeraren (flikens huvudfält).
    pub fn focus_editor(&mut self) {
        self.h.get_by_role(egui::accesskit::Role::MultilineTextInput).focus();
        self.settle();
    }

    /// Texten i den aktiva fliken.
    pub fn text(&self) -> String {
        self.app().docs[self.app().active].text.clone()
    }

    /// Skriver ut AccessKit-trädet (roll, etikett, värde); hjälp när man skriver ett nytt test.
    #[allow(dead_code)]
    pub fn dump(&self) {
        use egui_kittest::kittest::NodeT;
        for n in self.h.root().children_recursive() {
            let a = n.accesskit_node();
            println!("{:?} label={:?} value={:?}", a.role(), a.label(), a.value());
        }
    }

    /// Kör bilder tills inget mer ritas om.
    pub fn settle(&mut self) {
        self.h.run_ok();
        self.h.run_steps(2);
    }
}
