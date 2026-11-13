//! Tester för `Lsp` som helhet: urval av filer, saknad server och (ignorerat) en riktig rust-analyzer.

use super::*;

/// Kör `tick` tills `done` är sant (högst `secs` sekunder).
fn pump(lsp: &mut Lsp, docs: &[Document], secs: u64, mut done: impl FnMut(&mut Lsp) -> bool) -> bool {
    let ctx = egui::Context::default();
    let cfg = Cfg { enabled: true, commands: &[], workspace: None };
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        lsp.tick(&ctx, docs, docs.first().map(|d| d.id), &cfg);
        if done(lsp) {
            return true;
        }
        if std::env::var_os("LSP_DEBUG").is_some() {
            eprintln!("{:?} diags={} prog={:?}", lsp.status(docs[0].id), lsp.diags.len(), lsp.servers.iter().flat_map(|s| s.progress.iter().map(|p| (p.title.clone(), p.message.clone()))).collect::<Vec<_>>());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn eligibility_and_languages() {
    let mut d = Document::new_untitled(1, 1);
    assert!(eligible(&d).is_none());
    d.path = Some(PathBuf::from("/x/a.rs"));
    assert_eq!(eligible(&d).map(|(l, id)| (l.key, id)), Some(("rust", "rust")));
    d.path = Some(PathBuf::from("/x/readme.md"));
    assert!(eligible(&d).is_none());
    d.path = Some(PathBuf::from("/x/a.py"));
    d.pdf = true;
    assert!(eligible(&d).is_none());
}

#[test]
fn disabled_does_nothing() {
    let mut lsp = Lsp::default();
    let mut d = Document::new_untitled(1, 1);
    d.path = Some(PathBuf::from("/x/a.rs"));
    let cfg = Cfg { enabled: false, commands: &[], workspace: None };
    lsp.tick(&egui::Context::default(), &[d], Some(1), &cfg);
    assert!(lsp.servers.is_empty() && lsp.docs.is_empty());
}

#[test]
fn missing_server_is_reported_once() {
    let mut lsp = Lsp::default();
    let dir = std::env::temp_dir().join(format!("kladd-lsp-missing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Document::new_untitled(1, 1);
    d.path = Some(dir.join("a.go"));
    let cmds = vec![("go".to_owned(), "finns-inte-xyz".to_owned())];
    let cfg = Cfg { enabled: true, commands: &cmds, workspace: None };
    let ctx = egui::Context::default();
    let docs = [d];
    lsp.tick(&ctx, &docs, Some(1), &cfg);
    lsp.tick(&ctx, &docs, Some(1), &cfg);
    assert_eq!(lsp.out.notes.len(), 1);
    assert!(matches!(lsp.servers[0].state, SrvState::Failed(_)));
    assert!(lsp.status(1).is_some_and(|(_, err)| err));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mot en riktig rust-analyzer på ett litet tillfälligt crate. Kör med
/// `cargo test lsp_rust_analyzer -- --ignored --nocapture`.
#[test]
#[ignore = "kräver rust-analyzer och tar en stund"]
fn lsp_rust_analyzer_roundtrip() {
    let dir = std::env::temp_dir().join(format!("kladd-lsp-ra-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = "fn main() {\n    let s = \"åäö😀\"; let n: i32 = \"fel\";\n    let mut v: Vec<u8> = Vec::new();\n    v.push(1);\n    println!(\"{} {}\", s, n);\n    let _ = helper();\n}\nfn helper() -> u8 { 1 }\n";
    // Egen målkatalog: en delad (CARGO_TARGET_DIR) kan vara låst av ett annat bygge.
    // SAFETY: ignorerat test; ingen annan tråd läser miljön under tiden.
    unsafe { std::env::set_var("CARGO_TARGET_DIR", dir.join("target")) };
    let file = dir.join("src/main.rs");
    std::fs::write(&file, src).unwrap();
    let doc = Document::load(1, &file.canonicalize().unwrap()).unwrap();
    let docs = [doc];
    let mut lsp = Lsp::default();
    let path = docs[0].path.clone().unwrap();

    // Start, indexering och diagnostik. `"fel"` ligger efter en emoji (två UTF-16-enheter).
    let has_err = |l: &mut Lsp| l.diags.get(&path).is_some_and(|d| d.iter().any(|d| d.severity == convert::Severity::Error));
    assert!(pump(&mut lsp, &docs, 180, has_err), "ingen diagnostik");
    let d = lsp.diags_for(&path).iter().find(|d| d.severity == convert::Severity::Error).unwrap().clone();
    let (a, b) = d.range.unwrap();
    assert_eq!(docs[0].text.chars().skip(a).take(b - a).collect::<String>(), "\"fel\"");
    assert!(lsp.status(1).is_some());

    // Komplettering efter `v.`.
    let at = docs[0].text.find("v.push").unwrap() + 2;
    let caret = docs[0].text[..at].chars().count();
    // Standardbiblioteket kan vara under inläsning: fråga om tills metoderna finns.
    let mut found = false;
    for _ in 0..20 {
        lsp.ui.comp = None;
        lsp.complete(&docs[0], caret, Trigger::Char('.'));
        assert!(pump(&mut lsp, &docs, 60, |l| l.ui.comp.is_some()), "ingen komplettering");
        if lsp.ui.comp.as_ref().unwrap().items.iter().any(|i| i.label.starts_with("push")) {
            found = true;
            break;
        }
        pump(&mut lsp, &docs, 3, |_| false);
    }
    assert!(found, "ingen push bland förslagen");

    // Hover över `helper` i anropet.
    let at = docs[0].text.find("helper()").unwrap();
    let ci = docs[0].text[..at].chars().count();
    lsp.hover(&docs[0], ci + 2);
    assert!(pump(&mut lsp, &docs, 60, |l| l.ui.hover.is_some()), "ingen hover");

    // Gå till definition: raden med `fn helper`.
    lsp.definition(&docs[0], ci + 2);
    assert!(pump(&mut lsp, &docs, 60, |l| l.out.jump.is_some()), "ingen definition");
    let j = lsp.out.jump.take().unwrap();
    assert_eq!(j.path, path);
    assert_eq!(j.start.0 as usize, docs[0].text.lines().position(|l| l.starts_with("fn helper")).unwrap());

    // Byt namn: `helper` på båda ställena.
    lsp.begin_rename(&docs[0], ci + 2);
    assert!(pump(&mut lsp, &docs, 60, |l| l.ui.has_rename()), "ingen namnruta");
    assert_eq!(lsp.ui.rename_name(), Some("helper"));
    lsp.rename(&docs[0], ci + 2, "assistent");
    assert!(pump(&mut lsp, &docs, 60, |l| !l.out.edits.is_empty()), "inga namnändringar");
    let fe = lsp.out.edits.remove(0);
    let mut text = docs[0].text.clone();
    let ce = pos::to_cedits(&text, &fe.edits);
    pos::apply_cedits(&mut text, &ce);
    assert_eq!(text.matches("assistent").count(), 2);
    assert!(!text.contains("helper"));

    lsp.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
