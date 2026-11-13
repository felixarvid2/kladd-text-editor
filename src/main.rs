//! Kladd – en lugn och snygg texteditor.

// Ingen konsol på Windows i release (i debug behålls den så att stderr syns).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod autosave;
mod doc;
mod docx;
mod export;
mod grammar;
mod grammar_ui;
mod odt;
mod pdf_read;
mod pdf_write;
mod pdfview;
mod search;
mod preview;
mod print;
mod recovery;
mod highlight;
mod history;
mod history_ui;
mod icons;
mod keys;
mod macopen;
mod multicursor;
mod paste;
mod panes;
mod platform;
mod single;
mod spell;
mod style;
mod theme;
mod update;
mod word;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let mut files = Vec::new();
    let mut new_window = false;
    for arg in std::env::args_os().skip(1) {
        let s = arg.to_string_lossy();
        if s == "--version" || s == "-V" {
            println!("kladd {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        if s == "--help" || s == "-h" {
            println!(
                "Användning: kladd [--new-window] [FIL ...]\n\n\
                 Öppnar filerna i varsin flik. Om Kladd redan är öppet läggs\n\
                 filerna till som nya flikar där.\n\n\
                 -n, --new-window   öppna ett separat fönster"
            );
            return Ok(());
        }
        if s == "--new-window" || s == "-n" {
            new_window = true;
            continue;
        }
        if s.starts_with('-') {
            continue;
        }
        let path = match s.strip_prefix("file://") {
            Some(rest) => PathBuf::from(percent_decode(rest)),
            None => PathBuf::from(&arg),
        };
        if !path.exists() {
            // Skapa filen så att `kladd ny.md` fungerar som i andra editorer.
            if let Err(e) = std::fs::write(&path, "") {
                eprintln!("kladd: kunde inte skapa {}: {e}", path.display());
                continue;
            }
        }
        files.push(path);
    }

    // Kör Kladd redan? Skicka då filerna dit och avsluta.
    if !new_window && single::hand_over(&files) {
        return Ok(());
    }

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../packaging/icons/256/kladd.png"))
        .expect("giltig ikon");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Kladd")
            .with_app_id("kladd")
            .with_inner_size([1120.0, 740.0])
            .with_min_inner_size([560.0, 360.0])
            .with_icon(icon)
            .with_drag_and_drop(true)
            // Kladd ritar sin egen titelrad med stäng, minimera och maximera.
            .with_decorations(false),
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "kladd",
        options,
        Box::new(move |cc| Ok(Box::new(app::KladdApp::new(cc, files, !new_window)))),
    )
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
