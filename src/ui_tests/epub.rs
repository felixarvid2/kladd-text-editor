//! EPUB-läsläget: öppna, bläddra, kapitel, länkar och Tillbaka, sök, Redigera en kopia och att boken aldrig sparas.

use egui::{Key, Modifiers};

use super::Ui;
use crate::epub::tests::{CONTAINER, epub3, xhtml, zip_of};

const CTRL: Modifiers = Modifiers::COMMAND;

fn open_book(ui: &mut Ui) -> std::path::PathBuf {
    let file = ui.dir.write("prov.epub", epub3());
    ui.open(&file);
    ui.wait_until("boken är inläst", |app| app.docs.first().is_some_and(|d| d.epub.is_some()));
    ui.settle();
    file
}

fn tab(ui: &Ui) -> &crate::epub::view::EpubTab {
    ui.app().docs[0].epub.as_deref().unwrap()
}

#[test]
fn epub_opens_in_reading_mode_with_the_book_title() {
    let mut ui = Ui::new();
    open_book(&mut ui);
    assert!(ui.app().epub_mode());
    assert_eq!(ui.app().docs[0].title(), "Testboken", "fliken heter bokens titel");
    assert!(ui.app().docs[0].text.is_empty() && !ui.app().docs[0].dirty);
    // Första gången öppnas boken vid början: omslagssidan.
    assert_eq!(tab(&ui).section, 0);
    assert!(ui.has("Redigera en kopia"));
}

#[test]
fn epub_keys_move_between_chapters_and_back_through_links() {
    let mut ui = Ui::new();
    open_book(&mut ui);
    ui.key(Modifiers::NONE, Key::CloseBracket);
    let ch = |ui: &Ui| ui.app().docs[0].epub.as_ref().unwrap().chapter_label().2;
    assert_eq!(ch(&ui), "Första kapitlet");
    ui.key(Modifiers::NONE, Key::CloseBracket);
    assert_eq!(ch(&ui), "Andra kapitlet");
    ui.key(Modifiers::NONE, Key::OpenBracket);
    assert_eq!(ch(&ui), "Första kapitlet");
    // Alt+Ner/Upp är också kapitelbyte.
    ui.key(Modifiers::ALT, Key::ArrowDown);
    assert_eq!(ch(&ui), "Andra kapitlet");
    // Ctrl+Home och Ctrl+End: bokens början och slut.
    ui.key(CTRL, Key::End);
    assert_eq!(ch(&ui), "Noter");
    ui.key(CTRL, Key::Home);
    assert_eq!(ch(&ui), "Omslag");
    // En länk (innehållsförteckningen) och Tillbaka med Alt+Vänster.
    let d = tab(&ui).book.toc[2].dest.unwrap();
    ui.app_mut().docs[0].epub.as_deref_mut().unwrap().follow(d);
    ui.settle();
    assert_eq!(ch(&ui), "Andra kapitlet");
    ui.key(Modifiers::ALT, Key::ArrowLeft);
    assert_eq!(ch(&ui), "Omslag");
    ui.key(Modifiers::ALT, Key::ArrowRight);
    assert_eq!(ch(&ui), "Andra kapitlet");
}

#[test]
fn epub_space_goes_to_the_next_chapter_at_the_end() {
    let mut ui = Ui::new();
    open_book(&mut ui);
    // Omslaget ryms i fönstret: mellanslag går vidare till nästa del, Skift+mellanslag tillbaka.
    ui.key(Modifiers::NONE, Key::Space);
    ui.settle();
    assert_eq!(tab(&ui).chapter_label().2, "Första kapitlet");
    ui.key(Modifiers::SHIFT, Key::Space);
    ui.settle();
    assert_eq!(tab(&ui).chapter_label().2, "Omslag");
}

#[test]
fn epub_search_finds_hits_in_all_chapters() {
    let mut ui = Ui::new();
    open_book(&mut ui);
    ui.key(CTRL, Key::F);
    assert!(tab(&ui).find.open);
    ui.type_text("noten");
    ui.settle();
    let hits = tab(&ui).find.hits.len();
    assert_eq!(hits, 1, "träffen ligger i notkapitlet, långt från omslaget");
    assert_eq!(tab(&ui).chapter_label().2, "Noter", "sökningen hoppar till träffens kapitel");
    ui.key(Modifiers::NONE, Key::Escape);
    assert!(!tab(&ui).find.open);
}

#[test]
fn epub_is_never_saved_and_edit_copy_makes_a_new_document() {
    let mut ui = Ui::new();
    let file = open_book(&mut ui);
    let before = std::fs::read(&file).unwrap();
    ui.key(CTRL, Key::S);
    ui.key(CTRL | Modifiers::SHIFT, Key::S);
    assert_eq!(std::fs::read(&file).unwrap(), before, "boken skrivs aldrig över");
    assert_eq!(ui.app().docs.len(), 1);

    ui.click("Redigera en kopia");
    assert_eq!(ui.app().docs.len(), 2);
    let copy = &ui.app().docs[1];
    assert!(copy.path.is_none() && copy.dirty, "ett nytt osparat dokument");
    assert!(copy.text.starts_with("# Testboken"), "{}", copy.text);
    assert!(copy.text.contains("Börjar här") && copy.text.contains("Den första noten."));
    assert_eq!(copy.images.len(), 1, "bilden i läsordningen följer med");
    assert!(ui.app().docs[0].epub.is_some(), "boken finns kvar");
}

#[test]
fn epub_position_is_remembered_per_file() {
    let mut ui = Ui::new();
    let file = open_book(&mut ui);
    ui.key(Modifiers::NONE, Key::CloseBracket);
    ui.key(Modifiers::NONE, Key::CloseBracket);
    let section = tab(&ui).section;
    assert!(section > 1);
    let idx = ui.app().active;
    ui.app_mut().store_epub_memory(idx);
    assert_eq!(ui.app().settings.epub_memory.len(), 1);
    // Stäng och öppna igen: boken står kvar där man slutade.
    ui.key(CTRL, Key::W);
    assert!(ui.app().docs.is_empty() || ui.app().docs[0].epub.is_none());
    ui.open(&file);
    ui.wait_until("boken är inläst", |app| app.docs.first().is_some_and(|d| d.epub.is_some()));
    assert_eq!(tab(&ui).section, section);
}

#[test]
fn epub_drm_and_broken_files_show_an_error() {
    let mut ui = Ui::new();
    let enc = r#"<encryption xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/></enc:EncryptedData></encryption>"#;
    let drm = ui.dir.write("drm.epub", zip_of(&[("META-INF/container.xml", CONTAINER.as_bytes().to_vec()), ("META-INF/encryption.xml", enc.as_bytes().to_vec())]));
    ui.open(&drm);
    ui.settle();
    // Ingen flik öppnas (felet visas som notis och står i `Book::load`-testerna).
    assert!(ui.app().docs.is_empty());
    assert!(ui.app().settings.recent.iter().all(|p| !p.ends_with("drm.epub")));
    let junk = ui.dir.write("trasig.epub", b"inte en zip".to_vec());
    ui.open(&junk);
    ui.settle();
    assert!(ui.app().docs.is_empty());
}

/// Mäter hur lång en bildruta tar med ett mycket stort kapitel (körs för hand med `-- --ignored --nocapture`).
#[test]
#[ignore]
fn epub_large_chapter_frame_time() {
    let mut body = String::from("<h1>Det stora kapitlet</h1>");
    for i in 0..1500 {
        body.push_str(&format!(
            "<p>Stycke {i}. Det var en gång en mycket lång berättelse som aldrig tycktes ta slut, med <i>kursiv</i> och <b>fet</b> text samt en <a href=\"#s{i}\">länk</a>. \
             Vi fortsätter att skriva för att fylla ut raderna ordentligt så att varje stycke blir flera rader långt och kapitlet växer.</p>"
        ));
    }
    let opf = r#"<package><metadata><title>Stor bok</title></metadata><manifest><item id="a" href="a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#;
    let mut ui = Ui::new();
    let file = ui.dir.write(
        "stor.epub",
        zip_of(&[("META-INF/container.xml", CONTAINER.replace("OEBPS/content.opf", "o.opf").into_bytes()), ("o.opf", opf.as_bytes().to_vec()), ("a.xhtml", xhtml(&body))]),
    );
    ui.open(&file);
    ui.wait_until("boken är inläst", |app| app.docs.first().is_some_and(|d| d.epub.is_some()));
    let t = tab(&ui);
    println!("kapitlet: {} byte text i {} delar, största delen {} byte", t.book.sections.iter().map(|s| s.md.len()).sum::<usize>(), t.book.sections.len(), t.book.sections.iter().map(|s| s.md.len()).max().unwrap());
    ui.settle();
    let start = std::time::Instant::now();
    let n = 40;
    for _ in 0..n {
        ui.h.step();
    }
    println!("medel per bildruta: {:?}", start.elapsed() / n);
}
