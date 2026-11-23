//! Startmätning: med `KLADD_TIMING=1` skrivs tiden sedan processen startade till stderr vid
//! varje `mark`, så att man ser vad som tar tid innan första bilden. Utan variabeln kostar
//! varje anrop en atomläsning.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ON: OnceLock<bool> = OnceLock::new();

/// Anropas allra först i `main`: startar klockan.
pub fn init() {
    START.get_or_init(Instant::now);
}

fn enabled() -> bool {
    *ON.get_or_init(|| std::env::var_os("KLADD_TIMING").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// Skriver `[timing] <ms> ms <etikett>` till stderr om mätningen är på.
pub fn mark(label: &str) {
    if enabled() {
        let t = START.get_or_init(Instant::now).elapsed();
        eprintln!("[timing] {:8.1} ms  {label}", t.as_secs_f64() * 1000.0);
    }
}

/// Som `mark`, men bara första gången (för bildrutor).
pub fn mark_once(flag: &'static std::sync::atomic::AtomicBool, label: &str) {
    if enabled() && !flag.swap(true, std::sync::atomic::Ordering::Relaxed) {
        mark(label);
    }
}

thread_local! {
    static LAP: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Delmätning inom en åtgärd (PDF-export): skriver tiden sedan förra `lap` (eller `lap_start`) i
/// tråden som `[timing] <etikett>: <ms> ms`, om mätningen är på.
pub fn lap_start() {
    if enabled() {
        LAP.with(|l| l.set(Some(Instant::now())));
    }
}

pub fn lap(label: &str) {
    if enabled() {
        let now = Instant::now();
        if let Some(t) = LAP.with(|l| l.replace(Some(now))) {
            eprintln!("[timing] {label}: {:.1} ms", now.duration_since(t).as_secs_f64() * 1000.0);
        }
    }
}

#[cfg(test)]
mod tests {
    /// Markdown-cachen får inte läsa in syntect-uppsättningarna när den skapas: den skapas
    /// tre gånger vid start (appen, uppdateringsrutan, språkservern) och kostade tiotals ms var.
    #[test]
    fn markdown_cachen_ar_billig_att_skapa() {
        let t = std::time::Instant::now();
        let caches: Vec<_> = (0..100).map(|_| egui_commonmark::CommonMarkCache::default()).collect();
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
        assert_eq!(caches.len(), 100);
    }

    #[test]
    fn mark_utan_miljovariabel_gor_inget() {
        super::init();
        super::mark("test");
    }
}
