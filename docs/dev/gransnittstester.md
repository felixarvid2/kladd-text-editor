# Gränssnittstester

Testerna i `src/ui_tests/` kör hela `KladdApp` i [`egui_kittest`](https://crates.io/crates/egui_kittest):
simulerade tangenter och klick, knappar hittas via etikett i AccessKit-trädet, och inget fönster, ingen
GPU och inget nätverk behövs. De körs av vanliga `cargo test` (och därmed i CI-jobbet `test` på Linux,
Windows och macOS). Hela mappen tar några sekunder.

```
cargo test ui_tests            # bara gränssnittstesterna
cargo test ui_tests::word -- --nocapture
```

## Så är det uppbyggt

* `ui_tests` är en `#[cfg(test)]`-modul under `app.rs` (`#[path = "ui_tests/mod.rs"]`), så att den når
  appens privata fält. Filerna: `harness.rs` (testmiljön), `text.rs`, `word.rs` och `pdf.rs`.
* `Ui::new()` bygger appen med `KladdApp::new` utan sparad konfiguration. Inga biverkningar:
  * ingen enkelinstanssocket (`single_instance = false`),
  * ingen uppdateringskontroll (`auto_update_check = false`),
  * användarmapparna (`platform::data_dir`, `config_dir`, `home_dir`) pekar om till en tillfällig mapp
    via `platform::set_test_root` (bara i testbygget), så historik och återställningsfiler hamnar där
    och återställningsfrågan visas aldrig,
  * språket är svenska. Språket är per tråd i testbygget (`i18n.rs`), så ett test som byter till engelska
    stör inte de andra,
  * fast fönsterstorlek (1100x760, 1 px per punkt) och de inbäddade typsnitten.
* Varje `Ui` har en egen tillfällig mapp (`ui.dir`) som tas bort när testet är slut.

## Skriva ett test

```rust
#[test]
fn spara_text() {
    let mut ui = Ui::new();
    let fil = ui.dir.write("a.txt", "hej");
    ui.open(&fil);                       // som valt i Öppna-dialogen
    ui.focus_editor();
    ui.key(Modifiers::COMMAND, Key::A);  // COMMAND = Ctrl, ⌘ på Mac
    ui.type_text("nytt");
    ui.key(Modifiers::COMMAND, Key::S);
    assert_eq!(std::fs::read_to_string(&fil).unwrap(), "nytt");
}
```

Hjälpmetoderna i `harness.rs`:

| Metod | Gör |
|---|---|
| `click("etikett")`, `click_contains("del")` | klickar (rullar fram knappen först; menyerna är långa) |
| `click_nth_role(Role::ComboBox, 0)` | klickar på n:e elementet med rollen, t.ex. en rullgardin |
| `click_last("etikett")` | sista träffen, när en dialogknapp har samma namn som en knapp bakom |
| `key(mod, tangent)`, `type_text("x")` | tangenttryck och text till det fokuserade fältet |
| `focus_editor()`, `focus_nth_input(n)` | flyttar fokus till redigeraren eller n:e enradiga fältet |
| `drag(från, till)` | drar med musen |
| `has("etikett")`, `has_contains("del")` | finns elementet? |
| `wait_for("etikett")`, `wait_until(..)` | väntar på bakgrundsarbete (filträd, PDF, sök) |
| `app()`, `app_mut()`, `text()` | appens tillstånd; `text()` är aktiv flik |
| `dump()` | skriver ut AccessKit-trädet: så hittar man etiketten att klicka på |

## Hitta etiketter

Ikonknappar har sitt verktygstips som etikett (`"Fetstil (Ctrl+B)"`), menyrader, trädrader,
snabböppnaträffar och statusknappar sin text, rullgardiner roll `ComboBox` med den valda texten som
värde. Saknar något namn kan man lägga till `resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text))`
på knappen: det hjälper även skärmläsare. Kör `ui.dump()` med `--nocapture` för att se vad som finns.

## Att tänka på

* Klicka eller skriv i rätt ordning: efter ett knappklick har fokus lämnat redigeraren, så anropa
  `focus_editor()` före tangenter och text. Använd `Modifiers::COMMAND` (inte `CTRL`) för kortkommandon
  och undvik Ctrl+Home/End för att flytta markören (olika på Mac); markera med Ctrl+A i stället.
* Det som sker i bakgrundstrådar (filträd, snabböppna, PDF-sidor, sökning) syns först efter en väntan:
  använd `wait_for`/`wait_until`, som stegar bilder med korta pauser.
* Dubbelklick på egenritade ytor går inte att simulera pålitligt; anropa då samma metod som klicket
  (se sidhuvudstestet, `open_header_dialog`).
* Inga tester får nå nätet eller riktiga användarmappar. Skriv filer med `ui.dir.write`.
* Verifiera att ett nytt test kan misslyckas: ändra en `assert` tillfälligt och se att det blir rött.
