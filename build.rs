//! Bäddar in ikon och versionsinfo i kladd.exe. Gör ingenting på andra system.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=packaging/icons/kladd.ico");
    #[cfg(windows)]
    embed_windows_resources();
}

// Kräver rc.exe (MSVC) eller windres, vilket finns på GitHubs Windows-runners.
// Kompileras bara när bygget körs på Windows, så Linux och macOS slipper beroendet.
#[cfg(windows)]
fn embed_windows_resources() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let authors = std::env::var("CARGO_PKG_AUTHORS").unwrap_or_default();
    let mut res = winresource::WindowsResource::new();
    res.set_icon("packaging/icons/kladd.ico");
    res.set("FileDescription", "Kladd");
    res.set("ProductName", "Kladd");
    res.set("CompanyName", &authors);
    res.set("LegalCopyright", &format!("Copyright (c) 2026 {authors}"));
    res.set("OriginalFilename", "kladd.exe");
    if let Err(e) = res.compile() {
        // Varning i stället för fel: en saknad rc.exe ska inte fälla hela bygget.
        println!("cargo:warning=kunde inte bädda in Windows-resurser: {e}");
    }
}
