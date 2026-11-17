//! Hjälpare för att undersöka sidbrytningar och sparningens stabilitet i en fil (se `dump_markdown`).

/// `KLADD_DUMP=fil cargo test dump_markdown -- --ignored --nocapture` skriver Markdown efter öppning
/// och efter öppna → spara → öppna.
#[test]
#[ignore]
fn dump_markdown() {
    let Some(p) = std::env::var_os("KLADD_DUMP") else { return };
    let p = std::path::PathBuf::from(p);
    let bytes = std::fs::read(&p).unwrap();
    let odt = p.extension().is_some_and(|e| e == "odt");
    let rd = |b: &[u8]| if odt { crate::odt::read(b) } else { crate::docx::read(b) };
    let l = rd(&bytes).unwrap();
    let out = std::env::var("KLADD_DUMP_OUT").unwrap_or_else(|_| "/tmp/w-brk-dump".into());
    std::fs::write(format!("{out}.1.md"), &l.markdown).unwrap();
    let resolve = |s: &str| crate::doc::resolve_image(s, std::path::Path::new("."), &l.images);
    let save = |md: &str, src: Option<&crate::docx::Source>| {
        if odt { crate::odt::write(md, "K", &resolve).unwrap().bytes } else { crate::docx::write(md, "K", &resolve, src).unwrap().bytes }
    };
    let b2 = save(&l.markdown, l.source.as_ref());
    std::fs::write(format!("{out}.2.bin"), &b2).unwrap();
    let l2 = rd(&b2).unwrap();
    std::fs::write(format!("{out}.2.md"), &l2.markdown).unwrap();
    let b3 = save(&l2.markdown, l2.source.as_ref());
    std::fs::write(format!("{out}.3.bin"), &b3).unwrap();
    let l3 = rd(&b3).unwrap();
    std::fs::write(format!("{out}.3.md"), &l3.markdown).unwrap();
}
