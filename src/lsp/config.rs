//! Vilka språk som har språkserver, hur de startas och var projektets rot ligger.

use std::path::{Path, PathBuf};

use crate::i18n::t;

/// Ett språk med språkserver. `key` sparas i inställningarna.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lang {
    pub key: &'static str,
    /// Namn i gränssnittet (svensk källtext).
    pub name: &'static str,
    /// Förvalda kommandon, i prioritetsordning: det första som finns på PATH används.
    pub defaults: &'static [&'static [&'static str]],
    /// Filer och mappar som markerar projektets rot.
    pub markers: &'static [&'static str],
}

pub const LANGS: &[Lang] = &[
    Lang { key: "rust", name: "Rust", defaults: &[&["rust-analyzer"]], markers: &["Cargo.toml", "rust-project.json"] },
    Lang {
        key: "python",
        name: "Python",
        defaults: &[&["pyright-langserver", "--stdio"], &["pylsp"]],
        markers: &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "pyrightconfig.json"],
    },
    Lang {
        key: "typescript",
        name: "TypeScript / JavaScript",
        defaults: &[&["typescript-language-server", "--stdio"]],
        markers: &["tsconfig.json", "jsconfig.json", "package.json"],
    },
    Lang { key: "go", name: "Go", defaults: &[&["gopls"]], markers: &["go.mod", "go.work"] },
    Lang {
        key: "cpp",
        name: "C / C++",
        defaults: &[&["clangd"]],
        markers: &["compile_commands.json", "CMakeLists.txt", ".clangd", "Makefile"],
    },
];

/// Markörer som gäller alla språk, sist i prioritet.
const COMMON_MARKERS: &[&str] = &["Cargo.toml", "pyproject.toml", "package.json", "go.mod", ".git"];

/// Språk och LSP-`languageId` för en filändelse.
pub fn for_extension(ext: &str) -> Option<(&'static Lang, &'static str)> {
    let (key, id) = match ext.to_ascii_lowercase().as_str() {
        "rs" => ("rust", "rust"),
        "py" | "pyi" | "pyw" => ("python", "python"),
        "ts" | "mts" | "cts" => ("typescript", "typescript"),
        "tsx" => ("typescript", "typescriptreact"),
        "js" | "mjs" | "cjs" => ("typescript", "javascript"),
        "jsx" => ("typescript", "javascriptreact"),
        "go" => ("go", "go"),
        "c" | "h" => ("cpp", "c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => ("cpp", "cpp"),
        _ => return None,
    };
    LANGS.iter().find(|l| l.key == key).map(|l| (l, id))
}

/// Delar upp en kommandorad i ord (citattecken och `\` före mellanslag stöds).
pub fn split_command(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('"' | '\'', None) => {
                quote = Some(c);
                has = true;
            }
            (q, Some(open)) if q == open => quote = None,
            ('\\', None) if cfg!(not(windows)) && chars.peek().is_some_and(|n| n.is_whitespace()) => {
                cur.push(chars.next().unwrap_or(' '));
                has = true;
            }
            (c, None) if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (c, _) => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// Hittar en körbar fil på PATH (eller använder en angiven sökväg som den är).
pub fn which(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    let exts: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat"] } else { &[""] };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for e in exts {
            let c = dir.join(format!("{name}{e}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// Kommandot att starta för språket: ett eget från inställningarna (om ifyllt), annars det första
/// förvalda som finns på PATH. Första ordet byts mot den hittade sökvägen.
pub fn command_for(lang: &Lang, custom: &[(String, String)]) -> Option<Vec<String>> {
    let own = custom.iter().find(|(k, _)| k == lang.key).map(|(_, c)| c.trim()).filter(|c| !c.is_empty());
    let candidates: Vec<Vec<String>> = match own {
        Some(c) => vec![split_command(c)],
        None => lang.defaults.iter().map(|d| d.iter().map(|s| (*s).to_owned()).collect()).collect(),
    };
    for mut cmd in candidates {
        let Some(first) = cmd.first() else { continue };
        if let Some(found) = which(first) {
            cmd[0] = found.to_string_lossy().into_owned();
            return Some(cmd);
        }
    }
    None
}

/// Förvalt kommando som text (för platshållaren i inställningsrutan).
pub fn default_text(lang: &Lang) -> String {
    lang.defaults.iter().map(|d| d.join(" ")).collect::<Vec<_>>().join("  |  ")
}

/// Projektets rot för en fil: arbetsmappen om den har en projektmarkör, annars närmaste mapp
/// uppåt med en markör, annars arbetsmappen (om filen ligger i den), annars filens mapp.
pub fn project_root(file: &Path, lang: &Lang, workspace: Option<&Path>) -> PathBuf {
    let has_marker = |dir: &Path, markers: &[&str]| markers.iter().any(|m| dir.join(m).exists());
    let in_ws = workspace.filter(|w| file.starts_with(w));
    if let Some(w) = in_ws
        && (has_marker(w, lang.markers) || has_marker(w, COMMON_MARKERS))
    {
        return w.to_path_buf();
    }
    let mut dir = file.parent();
    while let Some(d) = dir {
        if has_marker(d, lang.markers) {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    let mut dir = file.parent();
    while let Some(d) = dir {
        if has_marker(d, COMMON_MARKERS) {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    in_ws.map(Path::to_path_buf).or_else(|| file.parent().map(Path::to_path_buf)).unwrap_or_default()
}

/// Språkets namn på aktuellt gränssnittsspråk.
pub fn lang_name(l: &Lang) -> &'static str {
    match l.key {
        "typescript" => t("TypeScript / JavaScript"),
        _ => l.name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(for_extension("RS").map(|(l, id)| (l.key, id)), Some(("rust", "rust")));
        assert_eq!(for_extension("tsx").map(|(l, id)| (l.key, id)), Some(("typescript", "typescriptreact")));
        assert_eq!(for_extension("h").map(|(l, id)| (l.key, id)), Some(("cpp", "c")));
        assert!(for_extension("md").is_none());
    }

    #[test]
    fn command_splitting() {
        assert_eq!(split_command("pylsp --verbose"), ["pylsp", "--verbose"]);
        assert_eq!(split_command("  \"/opt/my tools/ls\"   --stdio "), ["/opt/my tools/ls", "--stdio"]);
        assert_eq!(split_command("a '' b"), ["a", "", "b"]);
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn custom_command_overrides_default() {
        let rust = &LANGS[0];
        // Ett eget kommando som inte finns ger inget (och faller inte tillbaka på förvalet).
        let custom = vec![("rust".to_owned(), "finns-inte-xyz --x".to_owned())];
        assert_eq!(command_for(rust, &custom), None);
        // Tomt eget kommando = förval.
        let empty = vec![("rust".to_owned(), "  ".to_owned())];
        assert_eq!(command_for(rust, &empty), command_for(rust, &[]));
        // Ett som finns: första ordet blir sökvägen, resten behålls.
        let sh = vec![("rust".to_owned(), "sh -c exit".to_owned())];
        if let Some(c) = command_for(rust, &sh) {
            assert!(c[0].ends_with("sh") || c[0].contains("sh"));
            assert_eq!(&c[1..], ["-c", "exit"]);
        }
    }

    #[test]
    fn root_detection() {
        let base = std::env::temp_dir().join(format!("kladd-lsp-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let sub = base.join("ws/crates/inner/src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join("ws/crates/inner/Cargo.toml"), "").unwrap();
        let file = sub.join("lib.rs");
        let rust = &LANGS[0];
        // Närmaste Cargo.toml när arbetsmappen saknar markör.
        let ws = base.join("ws");
        assert_eq!(project_root(&file, rust, Some(&ws)), base.join("ws/crates/inner"));
        // Arbetsmappen vinner när den har en markör.
        std::fs::write(ws.join("Cargo.toml"), "").unwrap();
        assert_eq!(project_root(&file, rust, Some(&ws)), ws);
        // Utan arbetsmapp: närmaste markör.
        assert_eq!(project_root(&file, rust, None), base.join("ws/crates/inner"));
        // Ingen markör alls: filens mapp.
        let lone = base.join("lone/x.go");
        std::fs::create_dir_all(lone.parent().unwrap()).unwrap();
        assert_eq!(project_root(&lone, &LANGS[3], None), base.join("lone"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
