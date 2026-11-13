//! Dokumentläget för .docx: formateringsraden, formatmallar, sidhuvud och spårade ändringar.

use std::io::Read;
use std::path::{Path, PathBuf};

use egui::accesskit::Role;
use egui::{Key, Modifiers};

use super::Ui;

const CTRL: Modifiers = Modifiers::COMMAND;

/// Skriver ett litet .docx (rubrik och ett stycke) i testmappen.
fn make_docx(ui: &Ui, name: &str) -> PathBuf {
    let bytes = crate::docx::write("# Rubrik\n\nBrödtext här.\n", "Test", &|_| None, None).expect("skriva docx").bytes;
    ui.dir.write(name, bytes)
}

/// Öppnar en .docx; den visas direkt i dokumentläget med sidor och formateringsrad.
fn open_in_document_mode(ui: &mut Ui, path: &Path) {
    ui.open(path);
    assert!(ui.app().docs[0].docx);
    assert!(ui.app().word_mode(), "dokumentläget är aktivt");
    ui.focus_editor();
}

fn part(path: &Path, name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).expect("öppna docx")).expect("giltigt zip-paket");
    let mut out = String::new();
    zip.by_name(name).unwrap_or_else(|_| panic!("{name} saknas")).read_to_string(&mut out).expect("läsa del");
    out
}

#[test]
fn docx_bold_toolbar_style_and_save() {
    let mut ui = Ui::new();
    let file = make_docx(&ui, "dok.docx");
    open_in_document_mode(&mut ui, &file);
    assert!(!part(&file, "word/document.xml").contains("<w:b/>"), "inget fetstilt från början");

    // Markera allt och klicka på Fetstil i formateringsraden.
    ui.key(CTRL, Key::A);
    ui.click("Fetstil (Ctrl+B)");
    assert!(ui.text().contains("**"), "fetstilen syns som markering i texten: {:?}", ui.text());

    // Stilvalet: rullgardinen byter formatmall för stycket med markören (det sista, efter markeringen).
    ui.focus_editor();
    ui.click_nth_role(Role::ComboBox, 0);
    ui.click("Rubrik 2");
    assert!(ui.text().contains("\n## "), "stilen är rubrik 2: {:?}", ui.text());

    ui.key(CTRL, Key::S);
    assert!(!ui.app().docs[0].dirty);
    let xml = part(&file, "word/document.xml");
    assert!(xml.contains("<w:b/>"), "fetstilen är sparad i filen");
    assert!(xml.contains("Heading2"), "formatmallen är sparad i filen");
    assert!(xml.contains("Brödtext här."));
}

#[test]
fn docx_header_footer_dialog() {
    let mut ui = Ui::new();
    let file = make_docx(&ui, "hf.docx");
    open_in_document_mode(&mut ui, &file);

    // Dubbelklick i sidhuvudet öppnar dialogen. Det egenritade dubbelklicket går inte att simulera
    // tillförlitligt med AccessKit, så dialogen öppnas med samma anrop som klicket gör.
    assert!(ui.has_contains("Dubbelklicka för att ändra sidhuvud och sidfot"));
    ui.app_mut().open_header_dialog(false, 0);
    ui.settle();
    assert!(ui.has("Sidhuvud och sidfot"), "dialogen är öppen");
    ui.focus_nth_input(1);
    ui.type_text("Hemligt sidhuvud");
    ui.click("OK");
    assert!(!ui.has("Sidhuvud och sidfot"), "dialogen är stängd");

    ui.key(CTRL, Key::S);
    let bytes = std::fs::read(&file).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut all = String::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        if f.name().starts_with("word/header") || f.name().starts_with("word/footer") {
            f.read_to_string(&mut all).unwrap();
        }
    }
    assert!(all.contains("Hemligt sidhuvud"), "sidhuvudets text finns i filen");
}

#[test]
fn docx_tracked_changes_then_accept_all() {
    let mut ui = Ui::new();
    let file = make_docx(&ui, "spar.docx");
    open_in_document_mode(&mut ui, &file);

    ui.click("Spåra: av");
    assert!(ui.has("Spårar: på"));
    assert!(ui.app().review_tracking());

    // Skriv en ändring med spårning på: den blir en spårad ändring.
    ui.focus_editor();
    ui.key(CTRL, Key::A);
    ui.type_text("Nytt innehåll");
    assert!(ui.app().review_changes() > 0, "ändringen räknas som spårad");

    ui.click("Granska ▾");
    ui.click("Godkänn alla");
    assert_eq!(ui.app().review_changes(), 0, "alla ändringar är godkända");
    assert!(ui.text().contains("Nytt innehåll"));

    ui.key(CTRL, Key::S);
    let xml = part(&file, "word/document.xml");
    assert!(xml.contains("Nytt innehåll"));
    assert!(!xml.contains("<w:ins "), "inga spårade infogningar kvar");
    assert!(!xml.contains("<w:del "), "inga spårade borttagningar kvar");
}
