//! Ett öppet dokument: text, sökväg, teckenkodning och radslut.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl TextEncoding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 med BOM",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Windows1252 => "Windows-1252",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
        }
    }
}

pub struct Document {
    pub id: u64,
    pub path: Option<PathBuf>,
    pub text: String,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    pub dirty: bool,
    saved_hash: u64,
    saved_meta: (TextEncoding, LineEnding),
    untitled_index: usize,
}

/// Filtyper som visas i öppna/spara-dialogerna.
pub const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rs", "py", "js", "mjs", "ts", "tsx", "jsx", "json", "toml", "yaml",
    "yml", "html", "htm", "css", "scss", "xml", "svg", "sh", "bash", "zsh", "c", "h", "cpp",
    "hpp", "cc", "java", "kt", "go", "rb", "php", "lua", "sql", "ini", "conf", "cfg", "env",
    "csv", "tsv", "log", "tex", "rst", "org", "srt", "desktop", "service",
];

fn hash_text(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

fn decode(bytes: &[u8]) -> Result<(String, TextEncoding), String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return match std::str::from_utf8(rest) {
            Ok(s) => Ok((s.to_owned(), TextEncoding::Utf8Bom)),
            Err(_) => Err("Filen har en UTF-8-markering men innehåller ogiltiga tecken.".into()),
        };
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let (enc, kind) = if bytes[0] == 0xFF {
            (encoding_rs::UTF_16LE, TextEncoding::Utf16Le)
        } else {
            (encoding_rs::UTF_16BE, TextEncoding::Utf16Be)
        };
        let (text, had_errors) = enc.decode_with_bom_removal(bytes);
        if had_errors {
            return Err("Filen ser ut som UTF-16 men är skadad.".into());
        }
        return Ok((text.into_owned(), kind));
    }
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return Err("Filen verkar vara binär och kan inte öppnas som text.".into());
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok((s.to_owned(), TextEncoding::Utf8)),
        Err(_) => {
            let (text, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
            Ok((text.into_owned(), TextEncoding::Windows1252))
        }
    }
}

fn encode(text: &str, enc: TextEncoding) -> Result<Vec<u8>, String> {
    Ok(match enc {
        TextEncoding::Utf8 => text.as_bytes().to_vec(),
        TextEncoding::Utf8Bom => {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(text.as_bytes());
            v
        }
        TextEncoding::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            for u in text.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        }
        TextEncoding::Utf16Be => {
            let mut v = vec![0xFE, 0xFF];
            for u in text.encode_utf16() {
                v.extend_from_slice(&u.to_be_bytes());
            }
            v
        }
        TextEncoding::Windows1252 => {
            let (bytes, _, unmappable) = encoding_rs::WINDOWS_1252.encode(text);
            if unmappable {
                return Err(
                    "Texten innehåller tecken som inte finns i Windows-1252. Byt kodning till UTF-8 i statusraden och spara igen."
                        .into(),
                );
            }
            bytes.into_owned()
        }
    })
}

impl Document {
    pub fn new_untitled(id: u64, untitled_index: usize) -> Self {
        Self {
            id,
            path: None,
            text: String::new(),
            encoding: TextEncoding::Utf8,
            line_ending: LineEnding::Lf,
            dirty: false,
            saved_hash: hash_text(""),
            saved_meta: (TextEncoding::Utf8, LineEnding::Lf),
            untitled_index,
        }
    }

    pub fn load(id: u64, path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| describe_io_error(&e, path))?;
        let (mut text, encoding) = decode(&bytes)?;
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        // Bara rena CRLF-filer konverteras; blandade radslut lämnas orörda.
        let line_ending = if crlf > 0 && crlf == lf {
            text = text.replace("\r\n", "\n");
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        Ok(Self {
            id,
            path: Some(path.to_path_buf()),
            saved_hash: hash_text(&text),
            saved_meta: (encoding, line_ending),
            text,
            encoding,
            line_ending,
            dirty: false,
            untitled_index: 0,
        })
    }

    pub fn save_to(&mut self, path: &Path) -> Result<(), String> {
        let text = match self.line_ending {
            LineEnding::Lf => std::borrow::Cow::Borrowed(self.text.as_str()),
            LineEnding::CrLf => std::borrow::Cow::Owned(self.text.replace('\n', "\r\n")),
        };
        let bytes = encode(&text, self.encoding)?;
        write_atomic(path, &bytes).map_err(|e| describe_io_error(&e, path))?;
        self.path = Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
        self.saved_hash = hash_text(&self.text);
        self.saved_meta = (self.encoding, self.line_ending);
        self.dirty = false;
        Ok(())
    }

    /// Ska anropas när texten har ändrats i editorn.
    pub fn on_edited(&mut self) {
        self.dirty = hash_text(&self.text) != self.saved_hash
            || (self.encoding, self.line_ending) != self.saved_meta;
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
            None if self.untitled_index <= 1 => "Namnlös".to_owned(),
            None => format!("Namnlös {}", self.untitled_index),
        }
    }

    pub fn extension(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.extension())
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    pub fn is_markdown(&self) -> bool {
        matches!(self.extension().as_str(), "md" | "markdown" | "mdown" | "mkd")
    }

    pub fn language(&self) -> String {
        let ext = self.extension();
        let name = match ext.as_str() {
            "" | "txt" | "text" => "Text",
            "md" | "markdown" | "mdown" | "mkd" => "Markdown",
            "rs" => "Rust",
            "py" | "pyw" => "Python",
            "js" | "mjs" | "cjs" => "JavaScript",
            "jsx" => "JSX",
            "ts" => "TypeScript",
            "tsx" => "TSX",
            "json" => "JSON",
            "toml" => "TOML",
            "yaml" | "yml" => "YAML",
            "html" | "htm" => "HTML",
            "css" => "CSS",
            "scss" => "SCSS",
            "xml" | "svg" => "XML",
            "sh" | "bash" | "zsh" => "Shell",
            "c" | "h" => "C",
            "cpp" | "hpp" | "cc" | "cxx" => "C++",
            "java" => "Java",
            "kt" => "Kotlin",
            "go" => "Go",
            "rb" => "Ruby",
            "php" => "PHP",
            "lua" => "Lua",
            "sql" => "SQL",
            "ini" | "conf" | "cfg" | "env" | "desktop" | "service" => "Konfiguration",
            "csv" => "CSV",
            "tsv" => "TSV",
            "log" => "Logg",
            "tex" => "LaTeX",
            "rst" => "reStructuredText",
            "srt" => "Undertexter",
            other => return other.to_uppercase(),
        };
        name.to_owned()
    }

    /// Namn som syntect kan slå upp en syntax med.
    pub fn syntax_hint(&self) -> String {
        match self.extension().as_str() {
            "ts" | "tsx" | "mjs" | "cjs" | "jsx" => "js".into(),
            "htm" => "html".into(),
            "yml" => "yaml".into(),
            "markdown" | "mdown" | "mkd" => "md".into(),
            "zsh" | "bash" => "sh".into(),
            "svg" => "xml".into(),
            other => other.into(),
        }
    }
}

/// Skriver till en temporär fil bredvid målet och byter sedan namn,
/// så att en krasch eller full disk aldrig lämnar en halvskriven fil.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.kladd-{}.tmp", std::process::id()));
    let result = (|| {
        std::fs::write(&tmp, bytes)?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        // Sista utvägen, t.ex. om mappen inte är skrivbar men filen är det.
        if std::fs::metadata(&target).is_ok() {
            return std::fs::write(&target, bytes);
        }
    }
    result
}

fn describe_io_error(e: &std::io::Error, path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("{name} finns inte längre."),
        std::io::ErrorKind::PermissionDenied => {
            format!("Du har inte behörighet till {name}.")
        }
        _ => format!("{name}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encodings() {
        let text = "Hej på dig, åäö ÅÄÖ\nrad två\n";
        for enc in [
            TextEncoding::Utf8,
            TextEncoding::Utf8Bom,
            TextEncoding::Utf16Le,
            TextEncoding::Utf16Be,
        ] {
            let bytes = encode(text, enc).unwrap();
            let (back, detected) = decode(&bytes).unwrap();
            assert_eq!(back, text);
            assert_eq!(detected, enc);
        }
        let latin = encode(text, TextEncoding::Windows1252).unwrap();
        assert!(encode("emoji 😀", TextEncoding::Windows1252).is_err());
        let (back, detected) = decode(&latin).unwrap();
        assert_eq!(back, text);
        assert_eq!(detected, TextEncoding::Windows1252);
    }

    #[test]
    fn crlf_is_preserved() {
        let dir = std::env::temp_dir().join("kladd-test-crlf");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        std::fs::write(&p, "ett\r\ntvå\r\n").unwrap();
        let mut d = Document::load(1, &p).unwrap();
        assert_eq!(d.text, "ett\ntvå\n");
        assert_eq!(d.line_ending, LineEnding::CrLf);
        d.text.push_str("tre\n");
        d.save_to(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "ett\r\ntvå\r\ntre\r\n");
        assert!(!d.dirty);
        d.line_ending = LineEnding::Lf;
        d.on_edited();
        assert!(d.dirty, "byte av radslut räknas som ändring");
        // blandade radslut lämnas orörda
        std::fs::write(&p, "a\r\nb\n").unwrap();
        let d = Document::load(2, &p).unwrap();
        assert_eq!(d.line_ending, LineEnding::Lf);
        assert_eq!(d.text, "a\r\nb\n");
    }
}
