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
fn markdown_opens_in_reading_view() {
    let mut ui = Ui::new();
    let file = ui.dir.write("rubrik.md", "# Unik rubrik\n\ntext\n");
    ui.open(&file);
    // En öppnad .md-fil visas färdigritad i hela fönstret, utan delad skärm.
    assert!(ui.app().md_read_mode());
    assert!(!ui.app().settings.side_preview);
    assert!(ui.h.query_all_by_label("Unik rubrik").count() >= 1, "rubriken syns färdigritad");
    assert!(ui.has("Redigera (Ctrl+Shift+M)"));

    // Redigera byter till textredigeraren, och förhandsvisningen försvinner.
    ui.click_last("Redigera");
    assert!(!ui.app().md_read_mode());
    assert_eq!(ui.h.query_all_by_label("Unik rubrik").count(), 0, "ingen förhandsvisning bredvid");
    ui.focus_editor();
    ui.type_text("mer");
    assert!(ui.text().contains("mer"), "texten går att redigera");
    assert!(ui.app().docs[0].dirty);

    // Knappen i namnlisten (och Ctrl+Skift+M) går tillbaka till läsläget.
    ui.click("Visa förhandsvisning (Ctrl+Shift+M)");
    assert!(ui.app().md_read_mode());
    ui.key(CTRL | Modifiers::SHIFT, Key::M);
    assert!(!ui.app().md_read_mode());
}

/// En längre fil så att rullning och sökning har något att göra.
fn long_markdown() -> String {
    let mut s = String::from("# Överst\n\nBörjan med nyckelordet ananas här.\n\n- [ ] köp mjölk\n- [x] köp bröd\n\n");
    for i in 0..40 {
        s.push_str(&format!("## Avsnitt {i}\n\nStycke {i} med fyllnadstext som tar plats på sidan.\n\n"));
    }
    s.push_str("## Slutet\n\nSist finns också ananas.\n");
    s
}

#[test]
fn markdown_reading_search_stays_in_reading_view() {
    let mut ui = Ui::new();
    let file = ui.dir.write("lang.md", long_markdown());
    ui.open(&file);
    assert!(ui.app().md_read_mode());

    ui.key(CTRL, Key::F);
    assert!(ui.app().find.open);
    assert!(ui.app().md_read_mode(), "sökfältet öppnas utan att byta till redigeraren");
    ui.focus_nth_input(0);
    ui.type_text("ananas");
    ui.wait_until("två träffar", |a| a.md.count == 2);
    assert!(ui.app().md_read_mode());
    assert_eq!(ui.app().md.current, Some(0), "första träffen är aktiv");

    // F3 går till nästa träff (längst ned) och Skift+F3 tillbaka, båda utan att byta läge.
    ui.key(Modifiers::NONE, Key::F3);
    ui.wait_until("andra träffen", |a| a.md.current == Some(1));
    assert!(ui.app().md_read_mode());
    assert!(ui.app().md.top_byte > 500, "läsvyn rullade ned mot den sista träffen");
    ui.key(Modifiers::SHIFT, Key::F3);
    ui.wait_until("första träffen", |a| a.md.current == Some(0));

    // Esc stänger sökfältet; träffarna släcks.
    ui.key(Modifiers::NONE, Key::Escape);
    assert!(!ui.app().find.open);
    assert!(ui.app().md_read_mode());
    ui.wait_until("inga träffar kvar", |a| a.md.count == 0 || !a.find.open);
}

#[test]
fn markdown_reading_replace_switches_to_editor() {
    let mut ui = Ui::new();
    let file = ui.dir.write("ers.md", "# Titel\n\nett ord\n");
    ui.open(&file);
    assert!(ui.app().md_read_mode());
    ui.key(CTRL, Key::H);
    assert!(!ui.app().md_read_mode(), "Ersätt kräver redigeraren");
    assert!(ui.app().find.open && ui.app().find.replace);
}

#[test]
fn markdown_nav_pane_scrolls_reading_view() {
    let mut ui = Ui::new();
    let file = ui.dir.write("nav.md", long_markdown());
    ui.open(&file);
    ui.key(CTRL | Modifiers::SHIFT, Key::O);
    assert!(ui.app().settings.nav_pane);
    // Ett klick på en rad i navigeringsfönstret slutar i `goto_byte` med rubrikens början.
    let want = ui.text().find("## Avsnitt 20\n").unwrap();
    let ctx = ui.h.ctx.clone();
    ui.app_mut().goto_byte(&ctx, want);
    ui.settle();
    assert!(ui.app().md_read_mode(), "hoppet i navigeringsfönstret byter inte läge");
    ui.settle();
    let top = ui.app().md.top_byte;
    assert!(ui.app().md_read_mode());
    assert!(top + 100 >= want && top <= want + 100, "rubriken ligger överst ({top} mot {want})");
}

#[test]
fn markdown_checkbox_toggles_text_and_undo_works() {
    let mut ui = Ui::new();
    let file = ui.dir.write("lista.md", "# Att göra\n\n- [ ] köp mjölk\n- [x] köp bröd\n");
    ui.open(&file);
    assert!(ui.app().md_read_mode());
    assert!(!ui.app().docs[0].dirty);

    ui.click_nth_role(egui::accesskit::Role::CheckBox, 0);
    assert_eq!(ui.text(), "# Att göra\n\n- [x] köp mjölk\n- [x] köp bröd\n", "bara tecknet i hakparentesen ändras");
    assert!(ui.app().docs[0].dirty, "fliken är ändrad");
    assert!(ui.app().md_read_mode());

    ui.click_nth_role(egui::accesskit::Role::CheckBox, 1);
    assert_eq!(ui.text(), "# Att göra\n\n- [x] köp mjölk\n- [ ] köp bröd\n");

    // Ångra tar tillbaka klicken i tur och ordning, även om editorn aldrig visats.
    ui.key(CTRL, Key::Z);
    assert_eq!(ui.text(), "# Att göra\n\n- [x] köp mjölk\n- [x] köp bröd\n");
    ui.key(CTRL, Key::Z);
    assert_eq!(ui.text(), "# Att göra\n\n- [ ] köp mjölk\n- [x] köp bröd\n");
    assert!(!ui.app().docs[0].dirty, "tillbaka till den sparade texten");
    assert!(ui.app().md_read_mode());
}

#[test]
fn markdown_double_click_opens_editor_at_paragraph() {
    let mut ui = Ui::new();
    let file = ui.dir.write("dubbel.md", &long_markdown());
    ui.open(&file);
    assert!(ui.app().md_read_mode());
    let node = ui.h.get_all_by_label_contains("Början med nyckelordet").next().expect("stycket syns");
    let pos = {
        use egui_kittest::kittest::NodeT;
        let r = node.accesskit_node().raw_bounds().expect("läge");
        egui::pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32)
    };
    // Alla fyra händelserna i samma bild: testmiljön ger varje köad händelse en egen bild på en
    // kvarts sekund, och då hinner dubbelklicket inte registreras.
    {
        let inp = ui.h.input_mut();
        inp.events.push(egui::Event::PointerMoved(pos));
        for pressed in [true, false, true, false] {
            inp.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        }
    }
    ui.h.step();
    ui.settle();
    assert!(!ui.app().md_read_mode(), "dubbelklicket öppnar redigeraren");
    let at = ui.text().find("Början med").unwrap();
    let state = egui::text_edit::TextEditState::load(&ui.h.ctx, crate::app::editor_id(ui.app().docs[0].id)).unwrap();
    let caret = state.cursor.char_range().unwrap().primary.index.0;
    assert_eq!(caret, ui.text()[..at].chars().count(), "markören står i början av stycket");
}

#[test]
fn markdown_toggle_keeps_scroll_roughly() {
    let mut ui = Ui::new();
    let file = ui.dir.write("rull.md", long_markdown());
    ui.open(&file);
    let want = ui.text().find("## Avsnitt 30\n").unwrap();
    let ctx = ui.h.ctx.clone();
    ui.app_mut().goto_byte(&ctx, want);
    ui.wait_until("rullat", |a| a.md.top_byte + 100 >= want);
    // Läsvyn -> redigeraren: den ska rulla till ungefär samma ställe.
    ui.key(CTRL | Modifiers::SHIFT, Key::M);
    assert!(!ui.app().md_read_mode());
    ui.wait_until("redigeraren rullad", |a| a.md.editor_frac > 0.5);
    // Och tillbaka.
    ui.key(CTRL | Modifiers::SHIFT, Key::M);
    assert!(ui.app().md_read_mode());
    ui.wait_until("läsvyn rullad", |a| a.md.top_byte + 600 >= want);
}

#[test]
fn new_markdown_tab_is_edited_directly() {
    let mut ui = Ui::new();
    ui.key(CTRL, Key::N);
    assert!(!ui.app().md_read_mode(), "en namnlös flik öppnas i redigeraren");
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
