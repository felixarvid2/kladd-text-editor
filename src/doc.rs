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
    /// Sant om filen är ett Word-dokument som visas som Markdown.
    pub docx: bool,
    /// Sant när ett Word-dokument visas i dokumentläget (sida, formateringsrad) i stället för som Markdown-text.
    pub word_view: bool,
    /// Sant om filen är en PDF vars text visas som Markdown.
    pub pdf: bool,
    /// Meddelande att visa när filen har öppnats (t.ex. vad som inte kommer med).
    pub open_note: Option<String>,
    /// Innehåll i ett öppnat Word-dokument som Kladd inte kan spara tillbaka.
    pub docx_dropped: Vec<&'static str>,
    /// Bilder från ett öppnat Word-dokument, refererade som `bild:namn` i texten.
    pub images: Vec<(String, crate::docx::Image)>,
    saved_hash: u64,
    saved_meta: (TextEncoding, LineEnding),
    untitled_index: usize,
}

/// Filtyper som visas i öppna/spara-dialogerna.
pub const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "docx", "odt", "pdf", "rs", "py", "js", "mjs", "ts", "tsx", "jsx", "json", "toml", "yaml",
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
            docx: false,
            word_view: false,
            pdf: false,
            open_note: None,
            docx_dropped: Vec::new(),
            images: Vec::new(),
            saved_hash: hash_text(""),
            saved_meta: (TextEncoding::Utf8, LineEnding::Lf),
            untitled_index,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn load(id: u64, path: &Path) -> Result<Self, String> {
        Self::load_with_password(id, path, None).map_err(|e| match e {
            LoadError::NeedsPassword => "PDF-filen är lösenordsskyddad.".to_owned(),
            LoadError::WrongPassword => "Fel lösenord.".to_owned(),
            LoadError::Message(m) => m,
        })
    }

    pub fn load_with_password(id: u64, path: &Path, password: Option<&str>) -> Result<Self, LoadError> {
        let bytes = std::fs::read(path).map_err(|e| LoadError::Message(describe_io_error(&e, path)))?;
        let ext = ext_of(path);
        if ext == "pdf" || bytes.starts_with(b"%PDF-") {
            let loaded = crate::pdf_read::read(&bytes, password).map_err(|e| match e {
                crate::pdf_read::ReadError::NeedsPassword => LoadError::NeedsPassword,
                crate::pdf_read::ReadError::WrongPassword => LoadError::WrongPassword,
                crate::pdf_read::ReadError::Other(m) => LoadError::Message(m),
            })?;
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let open_note = if loaded.markdown.trim().is_empty() {
                Some(format!("{name} innehåller ingen text som kan läsas (skannade sidor?)"))
            } else if loaded.pages_without_text > 0 {
                let n = loaded.pages_without_text;
                Some(format!("{n} av {} sidor i {name} saknar text (skannade sidor?)", loaded.page_count))
            } else {
                None
            };
            return Ok(Self {
                id,
                path: Some(path.to_path_buf()),
                saved_hash: hash_text(&loaded.markdown),
                saved_meta: (TextEncoding::Utf8, LineEnding::Lf),
                text: loaded.markdown,
                encoding: TextEncoding::Utf8,
                line_ending: LineEnding::Lf,
                dirty: false,
                docx: false,
                word_view: false,
                pdf: true,
                open_note,
                docx_dropped: loaded.dropped,
                images: loaded.images,
                untitled_index: 0,
            });
        }
        Self::load_bytes(id, path, bytes, &ext).map_err(LoadError::Message)
    }

    fn load_bytes(id: u64, path: &Path, bytes: Vec<u8>, ext: &str) -> Result<Self, String> {
        let ext = ext.to_owned();
        // Word-dokument känns igen på filändelsen och på innehållet (zip-signaturen).
        if is_word_ext(&ext) || (bytes.starts_with(b"PK\x03\x04") && ext == "doc") {
            let loaded = crate::docx::read(&bytes)?;
            return Ok(Self {
                id,
                path: Some(path.to_path_buf()),
                saved_hash: hash_text(&loaded.markdown),
                saved_meta: (TextEncoding::Utf8, LineEnding::Lf),
                text: loaded.markdown,
                encoding: TextEncoding::Utf8,
                line_ending: LineEnding::Lf,
                dirty: false,
                docx: true,
                word_view: true,
                pdf: false,
                open_note: (!loaded.dropped.is_empty()).then(|| {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    format!("{name} innehåller {} som inte visas i Kladd", join_sv(&loaded.dropped))
                }),
                docx_dropped: loaded.dropped,
                images: loaded.images,
                untitled_index: 0,
            });
        }
        // OpenDocument-text visas i samma dokumentläge som Word.
        if is_odt_ext(&ext) {
            let loaded = crate::odt::read(&bytes)?;
            return Ok(Self {
                id,
                path: Some(path.to_path_buf()),
                saved_hash: hash_text(&loaded.markdown),
                saved_meta: (TextEncoding::Utf8, LineEnding::Lf),
                text: loaded.markdown,
                encoding: TextEncoding::Utf8,
                line_ending: LineEnding::Lf,
                dirty: false,
                docx: true,
                word_view: true,
                pdf: false,
                open_note: (!loaded.dropped.is_empty()).then(|| {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    format!("{name} innehåller {} som inte visas i Kladd", join_sv(&loaded.dropped))
                }),
                docx_dropped: loaded.dropped,
                images: loaded.images,
                untitled_index: 0,
            });
        }
        if ext == "doc" && bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            return Err("Det gamla .doc-formatet stöds inte. Öppna filen i Word eller LibreOffice och spara som .docx.".into());
        }
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
            docx: false,
            word_view: false,
            pdf: false,
            open_note: None,
            docx_dropped: Vec::new(),
            images: Vec::new(),
            untitled_index: 0,
        })
    }

    /// Sparar dokumentet. Returnerar varningar (t.ex. bilder som inte hittades).
    pub fn save_to(&mut self, path: &Path) -> Result<Vec<String>, String> {
        let to_word = ext_of(path) == "docx";
        let to_odt = ext_of(path) == "odt";
        let to_pdf = ext_of(path) == "pdf";
        let mut warnings = Vec::new();
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let bytes = if to_pdf {
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let images = &self.images;
            let written = crate::pdf_write::write(&self.text, &title, &|src| resolve_image(src, &base, images))?;
            warnings.extend(written.missing_images);
            written.bytes
        } else if to_odt {
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let images = &self.images;
            let written = crate::odt::write(&self.text, &title, &|src| resolve_image(src, &base, images))?;
            warnings.extend(written.missing_images);
            written.bytes
        } else if to_word {
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let images = &self.images;
            let written = crate::docx::write(&self.text, &title, &|src| resolve_image(src, &base, images))?;
            warnings.extend(written.missing_images);
            written.bytes
        } else {
            if !self.images.is_empty() && self.text.contains(crate::docx::IMAGE_SCHEME) {
                // Från Word till text: lägg bilderna i en mapp bredvid filen.
                self.export_images(path)?;
            }
            if self.docx || self.pdf {
                // Från Word/PDF till vanlig text: spara som UTF-8.
                self.encoding = TextEncoding::Utf8;
                self.line_ending = LineEnding::Lf;
            }
            let text = match self.line_ending {
                LineEnding::Lf => std::borrow::Cow::Borrowed(self.text.as_str()),
                LineEnding::CrLf => std::borrow::Cow::Owned(self.text.replace('\n', "\r\n")),
            };
            encode(&text, self.encoding)?
        };
        write_atomic(path, &bytes).map_err(|e| describe_io_error(&e, path))?;
        self.docx = to_word || to_odt;
        self.pdf = to_pdf;
        self.docx_dropped.clear();
        self.path = Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
        self.saved_hash = hash_text(&self.text);
        self.saved_meta = (self.encoding, self.line_ending);
        self.dirty = false;
        Ok(warnings)
    }

    /// Skriver bilderna till `<namn>-bilder/` och byter `bild:x` mot sökvägar i texten.
    fn export_images(&mut self, target: &Path) -> Result<(), String> {
        let stem = target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "dokument".into());
        let folder_name = format!("{stem}-bilder");
        let folder = target.parent().unwrap_or(Path::new(".")).join(&folder_name);
        let mut text = self.text.clone();
        for (name, img) in &self.images {
            let marker = format!("]({}{})", crate::docx::IMAGE_SCHEME, name.replace(' ', "%20"));
            if !text.contains(&marker) {
                continue;
            }
            std::fs::create_dir_all(&folder).map_err(|e| describe_io_error(&e, &folder))?;
            let file = folder.join(name);
            std::fs::write(&file, &img.bytes).map_err(|e| describe_io_error(&e, &file))?;
            let rel = format!("{folder_name}/{name}").replace(' ', "%20");
            text = text.replace(&marker, &format!("]({rel})"));
        }
        self.text = text;
        Ok(())
    }

    /// Mappen som relativa bildsökvägar utgår från.
    pub fn base_dir(&self) -> Option<PathBuf> {
        self.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf)
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
        self.docx || self.pdf || matches!(self.extension().as_str(), "md" | "markdown" | "mdown" | "mkd")
    }

    /// Word-filer som inte kan sparas tillbaka i samma format (makron, mallar).
    pub fn needs_save_as(&self) -> bool {
        self.docx && matches!(self.extension().as_str(), "docm" | "dotx" | "dotm" | "doc" | "ott")
    }

    pub fn language(&self) -> String {
        if self.docx {
            return if is_odt_ext(&self.extension()) { "OpenDocument" } else { "Word" }.to_owned();
        }
        if self.pdf {
            return "PDF".to_owned();
        }
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
        if self.docx || self.pdf {
            return "md".into();
        }
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

/// Hämtar bilddata för en bildkälla i texten: `bild:namn`, `~/…`, `file://…`,
/// en absolut sökväg eller en sökväg relativt dokumentets mapp.
pub fn resolve_image(src: &str, base: &Path, images: &[(String, crate::docx::Image)]) -> Option<crate::docx::Image> {
    let src = src.trim();
    if let Some(name) = src.strip_prefix(crate::docx::IMAGE_SCHEME) {
        let name = name.replace("%20", " ");
        return images.iter().find(|(n, _)| *n == name).map(|(_, i)| i.clone());
    }
    let path = image_path(src, base)?;
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    Some(crate::docx::Image { bytes, extent: None })
}

/// Gör om en bildkälla till en sökväg på disken (eller `None` för webbadresser).
pub fn image_path(src: &str, base: &Path) -> Option<PathBuf> {
    let src = src.trim().trim_start_matches('<').trim_end_matches('>');
    if src.contains("://") && !src.starts_with("file://") {
        return None;
    }
    let src = src.strip_prefix("file://").unwrap_or(src).replace("%20", " ");
    let path = if let Some(rest) = src.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else {
        let p = PathBuf::from(&src);
        if p.is_absolute() { p } else { base.join(p) }
    };
    Some(path)
}

pub enum LoadError {
    NeedsPassword,
    WrongPassword,
    Message(String),
}

/// "bilder", "bilder och kommentarer", "bilder, fotnoter och kommentarer".
pub fn join_sv(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [a] => (*a).to_owned(),
        [rest @ .., last] => format!("{} och {last}", rest.join(", ")),
    }
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_odt_ext(ext: &str) -> bool {
    matches!(ext, "odt" | "ott")
}

fn is_word_ext(ext: &str) -> bool {
    matches!(ext, "docx" | "docm" | "dotx" | "dotm")
}

/// Skriver till en temporär fil bredvid målet och byter sedan namn,
/// så att en krasch eller full disk aldrig lämnar en halvskriven fil.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.kladd-{}.tmp", std::process::id()));
    let result = (|| {
        {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            std::io::Write::write_all(&mut f, bytes)?;
            f.sync_all()?;
        }
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
    fn docx_document_roundtrip() {
        let dir = std::env::temp_dir().join("kladd-test-docx");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("brev.docx");
        let mut d = Document::new_untitled(1, 1);
        d.text = "# Hej\n\nEn **fet** rad.\n".into();
        d.save_to(&p).unwrap();
        assert!(d.docx && !d.dirty);
        assert!(d.images.is_empty());
        let back = Document::load(2, &p).unwrap();
        assert!(back.docx);
        assert_eq!(back.text, "# Hej\n\nEn **fet** rad.\n");
        assert_eq!(back.language(), "Word");
        // Spara samma dokument som vanlig text
        let mut back = back;
        let txt = dir.join("brev.md");
        back.save_to(&txt).unwrap();
        assert!(!back.docx);
        assert_eq!(std::fs::read_to_string(&txt).unwrap(), "# Hej\n\nEn **fet** rad.\n");
    }

    #[test]
    fn odt_document_roundtrip() {
        let dir = std::env::temp_dir().join("kladd-test-odt");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("brev.odt");
        let mut d = Document::new_untitled(1, 1);
        d.text = "# Hej\n\nEn **fet** rad.\n".into();
        d.save_to(&p).unwrap();
        assert!(d.docx && !d.dirty);
        let back = Document::load(2, &p).unwrap();
        assert!(back.docx && back.word_view && !back.needs_save_as());
        assert_eq!(back.text, "# Hej\n\nEn **fet** rad.\n");
        assert_eq!(back.language(), "OpenDocument");
        // Och vidare som vanlig text.
        let mut back = back;
        let md = dir.join("brev.md");
        back.save_to(&md).unwrap();
        assert!(!back.docx);
        assert_eq!(std::fs::read_to_string(&md).unwrap(), "# Hej\n\nEn **fet** rad.\n");
    }

    #[test]
    fn docx_images_survive_save_as_markdown() {
        let dir = std::env::temp_dir().join("kladd-test-docx-img");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 2x1 BMP räcker som testbild
        let bmp: Vec<u8> = {
            let mut v = b"BM".to_vec();
            v.extend_from_slice(&(62u32).to_le_bytes());
            v.extend_from_slice(&[0, 0, 0, 0]);
            v.extend_from_slice(&(54u32).to_le_bytes());
            v.extend_from_slice(&(40u32).to_le_bytes());
            v.extend_from_slice(&(2i32).to_le_bytes());
            v.extend_from_slice(&(1i32).to_le_bytes());
            v.extend_from_slice(&(1u16).to_le_bytes());
            v.extend_from_slice(&(24u16).to_le_bytes());
            v.extend_from_slice(&[0; 24]);
            v.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
            v
        };
        std::fs::write(dir.join("röd.bmp"), &bmp).unwrap();
        let mut d = Document::new_untitled(1, 1);
        d.text = "Bild: ![Röd](röd.bmp)\n".into();
        let doc = dir.join("med bild.docx");
        assert!(d.save_to(&doc).unwrap().is_empty(), "bilden hittas relativt dokumentet");
        let mut back = Document::load(2, &doc).unwrap();
        assert_eq!(back.images.len(), 1);
        assert!(back.text.contains("](bild:"), "{}", back.text);
        // Spara som Markdown: bilden hamnar i en mapp bredvid
        let md = dir.join("med bild.md");
        back.save_to(&md).unwrap();
        assert!(back.text.contains("](med%20bild-bilder/"), "{}", back.text);
        let saved = std::fs::read_to_string(&md).unwrap();
        let rel = saved.split("](").nth(1).unwrap().split(')').next().unwrap().replace("%20", " ");
        assert_eq!(std::fs::read(dir.join(rel)).unwrap(), bmp);
    }

    #[test]
    fn pdf_save_and_open() {
        let dir = std::env::temp_dir().join("kladd-test-pdf");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("brev.pdf");
        let mut d = Document::new_untitled(1, 1);
        d.text = "# Hej\n\nEn rad med åäö.\n".into();
        d.save_to(&p).unwrap();
        assert!(d.pdf && !d.docx && !d.dirty);
        let back = Document::load(2, &p).unwrap();
        assert!(back.pdf);
        assert_eq!(back.language(), "PDF");
        assert_eq!(back.text, "# Hej\n\nEn rad med åäö.\n");
        assert!(back.docx_dropped.is_empty(), "Kladds egen PDF förlorar inget");
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
