//! Textläget: öppna, skriva, ångra, spara, flikar, sök, delad vy m.m.

use egui::{Key, Modifiers};
use egui_kittest::kittest::Queryable;

use super::Ui;

const CTRL: Modifiers = Modifiers::COMMAND;

#[test]
fn open_type_undo_save_txt() {
    let mut ui = Ui::new();
    let file = ui.dir.write("anteckning.txt", "hej\n");
    ui.open(&file);
    assert_eq!(ui.text(), "hej\n");
    assert!(ui.has_contains("anteckning.txt"));

    ui.focus_editor();
    ui.key(CTRL, Key::A);
    ui.type_text("världen");
    assert_eq!(ui.text(), "världen");
    assert!(ui.app().docs[0].dirty);

    ui.key(CTRL, Key::Z);
    assert_eq!(ui.text(), "hej\n", "ångra tar tillbaka det som skrevs över");

    ui.key(CTRL, Key::A);
    ui.type_text("igen");
    ui.key(CTRL, Key::S);
    assert!(!ui.app().docs[0].dirty);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "igen");
}

#[test]
fn markdown_preview_toggle() {
    let mut ui = Ui::new();
    let file = ui.dir.write("rubrik.md", "# Unik rubrik\n\ntext\n");
    ui.open(&file);
    assert!(ui.app().settings.preview);
    assert!(ui.has("Dölj förhandsvisning (Ctrl+Shift+M)"));
    // Förhandsvisningen ritar rubriken som egen text vid sidan av redigeraren.
    assert!(ui.h.query_all_by_label("Unik rubrik").count() >= 1, "rubriken syns i förhandsvisningen");

    ui.click("Dölj förhandsvisning (Ctrl+Shift+M)");
    assert!(!ui.app().settings.preview);
    assert!(ui.has("Visa förhandsvisning (Ctrl+Shift+M)"));
    assert_eq!(ui.h.query_all_by_label("Unik rubrik").count(), 0, "ingen förhandsvisning kvar");

    ui.click("Visa förhandsvisning (Ctrl+Shift+M)");
    assert!(ui.app().settings.preview);
}

#[test]
fn find_and_replace_with_regex() {
    let mut ui = Ui::new();
    let file = ui.dir.write("siffror.txt", "rad 12 och rad 345\nrad 6\n");
    ui.open(&file);

    ui.key(CTRL, Key::H);
    assert!(ui.app().find.open && ui.app().find.replace);
    ui.click_contains("Reguljära uttryck");
    assert!(ui.app().find.regex);
    ui.focus_nth_input(0);
    ui.type_text(r"(\d+)");
    ui.focus_nth_input(1);
    ui.type_text("<$1>");
    assert_eq!(ui.app().find.query, r"(\d+)");
    assert_eq!(ui.app().find.replacement, "<$1>");

    ui.click("Ersätt alla");
    assert_eq!(ui.text(), "rad <12> och rad <345>\nrad <6>\n");
}

#[test]
fn tabs_new_switch_and_close_with_discard() {
    let mut ui = Ui::new();
    let file = ui.dir.write("a.txt", "första");
    ui.open(&file);
    assert_eq!(ui.app().docs.len(), 1);

    ui.click("Ny flik (Ctrl+T)");
    assert_eq!(ui.app().docs.len(), 2);
    assert_eq!(ui.app().active, 1);
    ui.focus_editor();
    ui.type_text("osparat");
    assert_eq!(ui.text(), "osparat");

    // Växla till den första fliken genom att klicka på den.
    ui.click_contains("a.txt");
    assert_eq!(ui.app().active, 0);
    assert_eq!(ui.text(), "första");

    // Stäng den osparade fliken: frågan visas och "Släng ändringar" stänger utan att spara.
    ui.click("Inte sparad än");
    assert_eq!(ui.app().active, 1);
    ui.key(CTRL, Key::W);
    assert!(ui.has("Släng ändringar"), "frågan om osparade ändringar visas");
    assert_eq!(ui.app().docs.len(), 2);
    ui.click("Släng ändringar");
    assert_eq!(ui.app().docs.len(), 1);
    assert_eq!(ui.text(), "första");
}

#[test]
fn split_view_toggle() {
    let mut ui = Ui::new();
    let file = ui.dir.write("delad.txt", "samma text");
    ui.open(&file);
    assert!(!ui.app().panes.is_split());

    ui.key(CTRL, Key::Backslash);
    assert!(ui.app().panes.is_split());
    assert!(ui.app().panes.same_doc(), "båda rutorna visar samma flik först");
    assert_eq!(ui.h.query_all_by_role(egui::accesskit::Role::MultilineTextInput).count(), 2);

    ui.key(CTRL, Key::Backslash);
    assert!(!ui.app().panes.is_split());
    assert_eq!(ui.h.query_all_by_role(egui::accesskit::Role::MultilineTextInput).count(), 1);
}

#[test]
fn folder_tree_opens_file() {
    let mut ui = Ui::new();
    ui.dir.write("rot.txt", "i roten");
    ui.dir.write("under/inne.txt", "inne i mappen");
    let root = ui.dir.path().to_path_buf();
    ui.app_mut().settings.workspace = Some(root);
    ui.app_mut().settings.workspace_panel = true;
    ui.settle();
    ui.wait_for("under");

    ui.click("under");
    assert!(ui.has("inne.txt"), "mappen expanderas");
    ui.click("inne.txt");
    assert_eq!(ui.app().docs.len(), 1);
    assert_eq!(ui.text(), "inne i mappen");
}

#[test]
fn quick_open_picks_file() {
    let mut ui = Ui::new();
    ui.dir.write("alfa.txt", "A");
    ui.dir.write("sub/beta-unik.txt", "B");
    let root = ui.dir.path().to_path_buf();
    ui.app_mut().settings.workspace = Some(root);
    ui.settle();

    ui.key(CTRL, Key::E);
    ui.type_text("beta");
    // Mappen läses i en bakgrundstråd; vänta tills träffen dyker upp.
    ui.wait_for("sub/beta-unik.txt");
    ui.click("sub/beta-unik.txt");
    assert_eq!(ui.app().docs.len(), 1);
    assert_eq!(ui.text(), "B");
}

#[test]
fn switch_language_to_english() {
    let mut ui = Ui::new();
    assert!(ui.has("Ny flik (Ctrl+T)"));
    assert!(!ui.has("New Tab (Ctrl+T)"));

    // Kommandopaletten: skriv språket och tryck Enter.
    ui.key(CTRL | Modifiers::SHIFT, Key::P);
    ui.type_text("Språk: English");
    ui.key(Modifiers::NONE, Key::Enter);
    assert_eq!(ui.app().settings.lang, crate::i18n::LangSetting::En);
    assert!(ui.has("New Tab (Ctrl+T)"), "knapparnas etiketter är översatta");
    assert!(!ui.has("Ny flik (Ctrl+T)"));
}

#[test]
fn multicursor_ctrl_d_typing() {
    let mut ui = Ui::new();
    let file = ui.dir.write("flera.txt", "foo bar foo baz foo");
    ui.open(&file);
    ui.focus_editor();
    ui.key(Modifiers::NONE, Key::Home);
    // Första Ctrl+D väljer ordet, de följande lägger till nästa förekomst.
    ui.key(CTRL, Key::D);
    ui.key(CTRL, Key::D);
    ui.key(CTRL, Key::D);
    ui.type_text("x");
    assert_eq!(ui.text(), "x bar x baz x");
}

#[test]
fn history_restore() {
    let mut ui = Ui::new();
    let file = ui.dir.write("hist.txt", "ett");
    ui.open(&file);
    ui.focus_editor();
    ui.key(CTRL, Key::A);
    ui.type_text("två");
    ui.key(CTRL, Key::S);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "två");

    ui.key(CTRL, Key::A);
    ui.type_text("tre");
    assert_eq!(ui.text(), "tre");

    ui.click("Arkiv");
    ui.click("Versionshistorik…");
    assert!(ui.has_contains("Versionshistorik för"));
    ui.click("Återställ");
    assert_eq!(ui.text(), "två", "den sparade versionen är tillbaka i fliken");
}
