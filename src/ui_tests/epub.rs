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

// ---------------------------------------------------------------- bokmärken, understrykningar, bibliotek

fn goto_first_chapter(ui: &mut Ui) {
    ui.key(Modifiers::NONE, Key::CloseBracket);
    ui.settle();
    assert_eq!(tab(ui).chapter_label().2, "Första kapitlet");
}

/// Lämnar en färdig markering av `text` mitt i läsytan, som om musen just släppts efter en markering.
fn select(ui: &mut Ui, text: &str) {
    crate::epub::selection::inject(text, egui::pos2(560.0, 300.0));
    ui.h.step();
    ui.settle();
}

#[test]
fn epub_ctrl_d_toggles_a_bookmark_that_survives_closing_the_book() {
    let mut ui = Ui::new();
    let file = open_book(&mut ui);
    goto_first_chapter(&mut ui);
    assert!(tab(&ui).notes.is_empty());
    ui.key(CTRL, Key::D);
    ui.settle();
    assert_eq!(tab(&ui).bookmarks().len(), 1, "Ctrl+D sätter ett bokmärke");
    assert!(tab(&ui).bookmarks()[0].quote.starts_with("Första kapitlet"), "{:?}", tab(&ui).bookmarks()[0].quote);
    let path = file.to_string_lossy().into_owned();
    assert_eq!(ui.app().library.notes_for("", &path).len(), 1, "sparat i biblioteket (nyckel: sökvägen, boken saknar id)");
    // Ctrl+D igen på samma ställe tar bort det.
    ui.key(CTRL, Key::D);
    ui.settle();
    assert!(tab(&ui).notes.is_empty());
    assert!(ui.app().library.notes_for("", &path).is_empty());
    // Sätt det igen, stäng boken och öppna den: bokmärket finns kvar.
    ui.key(CTRL, Key::D);
    ui.settle();
    ui.key(CTRL, Key::W);
    ui.open(&file);
    ui.wait_until("boken är inläst", |app| app.docs.first().is_some_and(|d| d.epub.is_some()));
    assert_eq!(tab(&ui).bookmarks().len(), 1, "bokmärket lästes ur biblioteket");
    // Boken själv är orörd.
    assert!(ui.app().docs[0].epub.is_some() && !ui.app().docs[0].dirty);
}

#[test]
fn epub_selection_menu_makes_a_highlight_with_a_note_and_exports_it() {
    let mut ui = Ui::new();
    open_book(&mut ui);
    goto_first_chapter(&mut ui);
    select(&mut ui, "Börjar här");
    assert!(tab(&ui).has_menu(), "menyn visas vid markeringen");
    ui.click("Anteckning…");
    ui.type_text("Min anteckning om början");
    ui.click("Spara");
    assert!(!tab(&ui).has_menu());
    let hl = tab(&ui).highlights();
    assert_eq!(hl.len(), 1, "{:?}", tab(&ui).notes);
    assert_eq!((hl[0].quote.as_str(), hl[0].note.as_str()), ("Börjar här", "Min anteckning om början"));
    assert!(hl[0].raw.starts_with("Börjar här"));
    // Sparat per bok.
    let path = ui.app().docs[0].path.as_ref().unwrap().to_string_lossy().into_owned();
    assert_eq!(ui.app().library.notes_for("", &path)[0].note, "Min anteckning om början");

    // Anteckningarna som Markdown i ett nytt osparat dokument.
    ui.app_mut().settings.nav_pane = true;
    ui.app_mut().docs[0].epub.as_deref_mut().unwrap().side = crate::epub::view::annot::SideTab::Notes;
    ui.settle();
    ui.click("Exportera anteckningar…");
    assert_eq!(ui.app().docs.len(), 2);
    let md = &ui.app().docs[1].text;
    assert!(ui.app().docs[1].path.is_none());
    assert!(md.starts_with("# Testboken\n\n*A. Författare, B. Medförfattare*\n\n## Första kapitlet\n\n> Börjar här\n"), "{md}");
    assert!(md.contains("**Anteckning:** Min anteckning om början"), "{md}");
}

#[test]
fn epub_highlight_color_can_be_changed_and_removed_from_its_menu() {
    let mut ui = Ui::new();
    open_book(&mut ui);
    goto_first_chapter(&mut ui);
    select(&mut ui, "andra kapitlet");
    // Första färgen (gul) är den första cirkeln i menyn: fönstret är 1100 bred och pekaren står vid x=560.
    let pos = egui::pos2((560.0f32 - 131.0).clamp(0.0, 2000.0) + 10.0 + 13.0, 300.0 + 14.0 + 10.0 + 13.0);
    ui.h.hover_at(pos);
    ui.h.step();
    for pressed in [true, false] {
        ui.h.input_mut().events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    ui.h.step();
    ui.settle();
    assert_eq!(tab(&ui).highlights().len(), 1, "ett klick på en färg sparar understrykningen");
    assert_eq!(tab(&ui).highlights()[0].color, 0);
    assert!(!tab(&ui).has_menu());
    // Ta bort via menyn för den befintliga understrykningen.
    let id = tab(&ui).highlights()[0].id;
    ui.app_mut().docs[0].epub.as_deref_mut().unwrap().remove_note(id);
    ui.settle();
    assert!(tab(&ui).notes.is_empty());
}

#[test]
fn epub_books_show_on_the_start_page_and_open_where_you_left_off() {
    let mut ui = Ui::new();
    let file = open_book(&mut ui);
    ui.key(Modifiers::NONE, Key::CloseBracket);
    ui.key(Modifiers::NONE, Key::CloseBracket);
    let section = tab(&ui).section;
    ui.key(CTRL, Key::W);
    ui.settle();
    assert!(ui.app().docs.is_empty());
    assert!(ui.has("Böcker"), "startsidan visar bokraden");
    assert!(ui.has("Testboken"), "kortet för boken");
    let rec = &ui.app().library.books[0];
    assert_eq!((rec.title.as_str(), rec.author.as_str()), ("Testboken", "A. Författare, B. Medförfattare"));
    assert!(!rec.cover.is_empty() && crate::epub::store::cover_dir().join(&rec.cover).exists(), "omslagsminiatyren är sparad");
    assert!(rec.progress > 0.0, "framsteget är sparat: {}", rec.progress);
    ui.click("Testboken");
    ui.wait_until("boken är inläst", |app| app.docs.first().is_some_and(|d| d.epub.is_some()));
    assert_eq!(tab(&ui).section, section, "boken öppnas där man slutade");
    // En bok vars fil saknas visas nedtonad och med en anmärkning.
    ui.key(CTRL, Key::W);
    ui.settle();
    std::fs::remove_file(&file).unwrap();
    ui.settle();
    assert!(ui.has("Testboken, Filen hittades inte"), "saknad fil");
}

#[test]
fn epub_book_can_be_removed_from_the_shelf_but_keeps_its_notes() {
    let mut ui = Ui::new();
    let file = open_book(&mut ui);
    goto_first_chapter(&mut ui);
    ui.key(CTRL, Key::D);
    ui.settle();
    ui.key(CTRL, Key::W);
    ui.settle();
    let path = file.to_string_lossy().into_owned();
    ui.app_mut().library.hide(&path);
    ui.settle();
    assert!(!ui.has("Böcker"), "borttagen ur listan");
    assert_eq!(ui.app().library.notes_for("", &path).len(), 1, "anteckningarna finns kvar");
    ui.open(&file);
    ui.wait_until("boken är inläst", |app| app.docs.first().is_some_and(|d| d.epub.is_some()));
    assert_eq!(tab(&ui).bookmarks().len(), 1);
    assert!(!ui.app().library.books[0].hidden, "boken är med i listan igen när den öppnats");
}

#[test]
fn epub_clicking_a_highlight_opens_its_menu_with_remove() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut ui = Ui::new();
    open_book(&mut ui);
    goto_first_chapter(&mut ui);
    select(&mut ui, "Börjar här");
    ui.click("Anteckning…");
    ui.type_text("Anteckningen");
    ui.click("Spara");
    assert_eq!(tab(&ui).highlights().len(), 1);
    assert!(!tab(&ui).has_menu());
    // Ett klick mitt i den understrukna texten (stycket börjar med "Börjar här").
    let node = ui.h.get_all_by_label_contains("Börjar här").next().expect("stycket syns");
    let r = node.accesskit_node().raw_bounds().expect("läge");
    let pos = egui::pos2(r.x0 as f32 + 24.0, ((r.y0 + r.y1) / 2.0 - (r.y1 - r.y0) / 2.0) as f32 + 10.0);
    ui.h.hover_at(pos);
    ui.h.step();
    for pressed in [true, false] {
        ui.h.input_mut().events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    ui.h.step();
    ui.settle();
    assert!(tab(&ui).has_menu(), "ett klick på understrykningen öppnar menyn");
    assert!(tab(&ui).menu_is_existing(), "menyn gäller den befintliga understrykningen");
    assert!(ui.has("Redigera anteckning…") && ui.has("Ta bort"), "menyn för en befintlig understrykning har Ta bort och redigera");
    ui.click("Ta bort");
    assert!(tab(&ui).highlights().is_empty() && !tab(&ui).has_menu());
}

#[test]
fn epub_plain_click_on_text_does_not_open_the_selection_menu() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut ui = Ui::new();
    open_book(&mut ui);
    goto_first_chapter(&mut ui);
    let node = ui.h.get_all_by_label_contains("Börjar här").next().expect("stycket syns");
    let r = node.accesskit_node().raw_bounds().expect("läge");
    let pos = egui::pos2(r.x0 as f32 + 24.0, r.y0 as f32 + 10.0);
    ui.h.hover_at(pos);
    ui.h.step();
    for pressed in [true, false] {
        ui.h.input_mut().events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    ui.h.step();
    ui.settle();
    assert!(!tab(&ui).has_menu(), "ett vanligt klick markerar ingenting");
}
