//! PDF-visaren: sidor, zoom, sök, omvandling till dokument och markeringar.

use egui::accesskit::Role;
use egui::{Key, Modifiers};

use super::Ui;

const CTRL: Modifiers = Modifiers::COMMAND;

/// Skriver en PDF med flera sidor (många rader text) i testmappen och öppnar den.
fn open_pdf(ui: &mut Ui) -> std::path::PathBuf {
    let mut md = String::new();
    for i in 0..120 {
        md.push_str(&format!("Rad {i} med ordet kladdtest.\n\n"));
    }
    let bytes = crate::pdf_write::write(&md, "Test", &|_| None).expect("skriva pdf").bytes;
    let file = ui.dir.write("prov.pdf", bytes);
    ui.open(&file);
    ui.wait_until("PDF-sidorna är lästa", |app| app.docs.first().and_then(|d| d.pdfv.as_ref()).is_some_and(|t| t.view.page_count() >= 2));
    file
}

fn page(ui: &Ui) -> usize {
    ui.app().docs[0].pdfv.as_ref().unwrap().view.current_page()
}

#[test]
fn pdf_page_navigation_and_zoom() {
    let mut ui = Ui::new();
    open_pdf(&mut ui);
    assert_eq!(page(&ui), 0);

    ui.click("Nästa sida (PgDn)");
    ui.wait_until("sidan byts", |app| app.docs[0].pdfv.as_ref().unwrap().view.current_page() == 1);
    ui.click("Föregående sida (PgUp)");
    ui.wait_until("sidan byts tillbaka", |app| app.docs[0].pdfv.as_ref().unwrap().view.current_page() == 0);

    // Zoomlistan: välj 200 %.
    ui.click_nth_role(Role::ComboBox, 0);
    ui.click("200 %");
    let zoom = ui.app().docs[0].pdfv.as_ref().unwrap().view.zoom();
    assert!((zoom - 2.0).abs() < 0.01, "zoom {zoom}");
}

#[test]
fn pdf_find_and_edit_to_document_mode() {
    let mut ui = Ui::new();
    open_pdf(&mut ui);

    ui.key(CTRL, Key::F);
    ui.type_text("kladdtest");
    ui.wait_until("sökningen hittar träffar", |app| app.docs[0].pdfv.as_ref().unwrap().view.find_status().total >= 2);

    // Redigera: bekräfta i dialogen så blir PDF:en ett redigerbart dokument.
    // "Redigera" finns också som meny i menyraden; knappen i PDF-verktygsraden kommer sist.
    ui.click_last("Redigera");
    assert!(ui.has("Redigera PDF:en?"));
    ui.click_last("Redigera");
    assert!(ui.app().docs[0].pdfv.is_none(), "PDF-visaren är ersatt");
    assert!(ui.app().docs[0].pdf && ui.app().word_mode(), "dokumentläget är aktivt");
    assert!(ui.text().contains("Rad 0 med ordet kladdtest"));
}

#[test]
fn pdf_highlight_annotation_and_save() {
    let mut ui = Ui::new();
    let file = open_pdf(&mut ui);
    assert!(!std::fs::read(&file).unwrap().windows(10).any(|w| w == b"/Highlight"));

    ui.click("Anteckna");
    ui.click("Markera");
    // Dra över första textraden. Sidans läge beror på zoomläget, så raden söks av uppifrån och ner.
    let unsaved = |ui: &Ui| ui.app().docs[0].pdfv.as_ref().unwrap().unsaved();
    let mut y = 110.0;
    while !unsaved(&ui) && y < 420.0 {
        ui.drag(egui::pos2(380.0, y), egui::pos2(640.0, y));
        y += 6.0;
    }
    assert!(unsaved(&ui), "markeringen blev en väntande anteckning");

    ui.key(CTRL, Key::S);
    assert!(!unsaved(&ui), "sparad");
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.windows(10).any(|w| w == b"/Highlight"), "PDF-filen innehåller en Highlight-anteckning");
}
