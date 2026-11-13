//! Ett öppet dokument: text, sökväg, teckenkodning och radslut.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
            Self::Utf8Bom => crate::i18n::t("UTF-8 med BOM"),
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Windows1252 => "Windows-1252",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Sant om filen är en PDF som visas som sidor i PDF-visaren (inget Markdown, aldrig ändrad).
    pub pdfv: Option<Box<crate::pdfview::chrome::PdfTab>>,
    /// Meddelande att visa när filen har öppnats (t.ex. vad som inte kommer med).
    pub open_note: Option<String>,
    /// Innehåll i ett öppnat Word-dokument som Kladd inte kan spara tillbaka.
    pub docx_dropped: Vec<&'static str>,
    /// Bilder från ett öppnat Word-dokument, refererade som `bild:namn` i texten.
    pub images: Vec<(String, crate::docx::Image)>,
    /// Originalpaketet för ett öppnat Word-dokument, så att sparning bara skriver om det som ändrats.
    pub docx_source: Option<std::sync::Arc<crate::docx::Source>>,
    saved_hash: u64,
    saved_meta: (TextEncoding, LineEnding),
    untitled_index: usize,
    /// Räknas upp vid varje ändring av texten (för cachar som inte vill hasha hela texten).
    pub rev: u64,
}

/// Filtyper som visas i öppna/spara-dialogerna.
pub const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "docx", "odt", "pdf", "rs", "py", "js", "mjs", "ts", "tsx", "jsx", "json", "toml", "yaml",
    "yml", "html", "htm", "css", "scss", "xml", "svg", "sh", "bash", "zsh", "c", "h", "cpp",
    "hpp", "cc", "java", "kt", "go", "rb", "php", "lua", "sql", "ini", "conf", "cfg", "env",
    "csv", "tsv", "log", "tex", "rst", "org", "srt", "desktop", "service",
];

/// Fingeravtryck av en text (ändringsdetektor). Små texter hashas som förut; stora (över en
/// megabyte) med en snabb hash som läser åtta byte åt gången, så att 100 MB tar ~15 ms.
pub(crate) fn hash_text(text: &str) -> u64 {
    if text.len() <= 1_000_000 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        return h.finish();
    }
    let mut h = 0x9E37_79B9_7F4A_7C15u64 ^ text.len() as u64;
    let mut chunks = text.as_bytes().chunks_exact(8);
    for c in &mut chunks {
        let v = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = (h.rotate_left(5) ^ v).wrapping_mul(0x517C_C1B7_2722_0A95);
    }
    for &b in chunks.remainder() {
        h = (h.rotate_left(5) ^ b as u64).wrapping_mul(0x517C_C1B7_2722_0A95);
    }
    h
}

/// Läser en fil i bitar och rapporterar hur många byte som lästs (förloppsvisning vid stora filer).
pub fn read_file_progress(path: &Path, progress: Option<&std::sync::atomic::AtomicU64>) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata().map(|m| m.len()).unwrap_or(0) as usize;
    let mut buf = Vec::with_capacity(size + 1);
    let mut chunk = vec![0u8; 8 << 20];
    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = progress {
            p.store(buf.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }
    Ok(buf)
}

fn decode(bytes: &[u8]) -> Result<(String, TextEncoding), String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return match std::str::from_utf8(rest) {
            Ok(s) => Ok((s.to_owned(), TextEncoding::Utf8Bom)),
            Err(_) => Err(crate::i18n::t("Filen har en UTF-8-markering men innehåller ogiltiga tecken.").into()),
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
            return Err(crate::i18n::t("Filen ser ut som UTF-16 men är skadad.").into());
        }
        return Ok((text.into_owned(), kind));
    }
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return Err(crate::i18n::t("Filen verkar vara binär och kan inte öppnas som text.").into());
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok((s.to_owned(), TextEncoding::Utf8)),
        Err(_) => {
            let (text, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
            Ok((text.into_owned(), TextEncoding::Windows1252))
        }
    }
}

/// Som `decode`, men tar över byten: vanlig UTF-8 blir en `String` utan kopia.
fn decode_owned(bytes: Vec<u8>) -> Result<(String, TextEncoding), String> {
    let head = &bytes[..bytes.len().min(8192)];
    let plain = !bytes.starts_with(&[0xEF, 0xBB, 0xBF])
        && !bytes.starts_with(&[0xFF, 0xFE])
        && !bytes.starts_with(&[0xFE, 0xFF]);
    if plain && !head.contains(&0) {
        return match String::from_utf8(bytes) {
            Ok(s) => Ok((s, TextEncoding::Utf8)),
            Err(e) => {
                let (text, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(e.as_bytes());
                Ok((text.into_owned(), TextEncoding::Windows1252))
            }
        };
    }
    decode(&bytes)
}

/// Gör om CRLF till LF på plats (utan en andra kopia av texten).
fn crlf_to_lf(text: String) -> String {
    let mut b = text.into_bytes();
    let mut w = 0;
    for r in 0..b.len() {
        if b[r] == b'\r' && b.get(r + 1) == Some(&b'\n') {
            continue;
        }
        b[w] = b[r];
        w += 1;
    }
    b.truncate(w);
    // Bara ASCII-byte togs bort ur giltig UTF-8, så resultatet är giltigt.
    String::from_utf8(b).unwrap_or_default()
}

fn encode(text: &str, enc: TextEncoding) -> Result<std::borrow::Cow<'_, [u8]>, String> {
    use std::borrow::Cow;
    Ok(Cow::Owned(match enc {
        TextEncoding::Utf8 => return Ok(Cow::Borrowed(text.as_bytes())),
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
                    crate::i18n::t("Texten innehåller tecken som inte finns i Windows-1252. Byt kodning till UTF-8 i statusraden och spara igen.")
                        .into(),
                );
            }
            bytes.into_owned()
        }
    }))
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
            pdfv: None,
            open_note: None,
            docx_dropped: Vec::new(),
            images: Vec::new(),
            docx_source: None,
            saved_hash: hash_text(""),
            saved_meta: (TextEncoding::Utf8, LineEnding::Lf),
            untitled_index,
            rev: 0,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn load(id: u64, path: &Path) -> Result<Self, String> {
        Self::load_with_password(id, path, None).map_err(|e| match e {
            LoadError::NeedsPassword => crate::i18n::t("PDF-filen är lösenordsskyddad.").to_owned(),
            LoadError::WrongPassword => crate::i18n::t("Fel lösenord.").to_owned(),
            LoadError::Message(m) => m,
        })
    }

    pub fn load_with_password(id: u64, path: &Path, password: Option<&str>) -> Result<Self, LoadError> {
        Self::load_with_progress(id, path, password, None)
    }

    /// Som `load_with_password`; `progress` får antalet inlästa byte (för en bakgrundstråd).
    pub fn load_with_progress(
        id: u64,
        path: &Path,
        password: Option<&str>,
        progress: Option<&std::sync::atomic::AtomicU64>,
    ) -> Result<Self, LoadError> {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > crate::bigtext::MAX_BYTES) {
            return Err(LoadError::Message(crate::i18n::t("Filen är för stor för Kladd (över 2 GB).").to_owned()));
        }
        let bytes =
            read_file_progress(path, progress).map_err(|e| LoadError::Message(describe_io_error(&e, path)))?;
        let ext = ext_of(path);
        if ext == "pdf" || bytes.starts_with(b"%PDF-") {
            return Self::from_pdf_text(id, path, &bytes, password);
        }
        Self::load_bytes(id, path, bytes, &ext).map_err(LoadError::Message)
    }

    /// Läser en sparad version (de sparade byten) som ett namnlöst, ändrat dokument i filens format.
    /// `name` ger format (filändelse) och titel.
    pub fn from_version(id: u64, name: &Path, bytes: &[u8]) -> Result<Self, String> {
        let ext = ext_of(name);
        let mut doc = if ext == "pdf" || bytes.starts_with(b"%PDF-") {
            Self::from_pdf_text(id, name, bytes, None).map_err(|e| match e {
                LoadError::NeedsPassword | LoadError::WrongPassword => crate::i18n::t("PDF-versionen är lösenordsskyddad.").to_owned(),
                LoadError::Message(m) => m,
            })?
        } else {
            Self::load_bytes(id, name, bytes.to_vec(), &ext)?
        };
        doc.path = None;
        doc.dirty = true;
        Ok(doc)
    }

    /// Gör om en PDF till redigerbar Markdown-text (dagens väg, används av Redigera i PDF-visaren).
    pub fn from_pdf_text(id: u64, path: &Path, bytes: &[u8], password: Option<&str>) -> Result<Self, LoadError> {
        Self::from_pdf_text_ocr(id, path, bytes, password, &std::collections::HashMap::new())
    }

    /// Som `from_pdf_text`, med igenkänd text (sida → stycken) för sidor som saknar text.
    pub fn from_pdf_text_ocr(id: u64, path: &Path, bytes: &[u8], password: Option<&str>, ocr: &std::collections::HashMap<usize, String>) -> Result<Self, LoadError> {
        let loaded = crate::pdf_read::read_with_ocr(bytes, password, ocr).map_err(|e| match e {
            crate::pdf_read::ReadError::NeedsPassword => LoadError::NeedsPassword,
            crate::pdf_read::ReadError::WrongPassword => LoadError::WrongPassword,
            crate::pdf_read::ReadError::Other(m) => LoadError::Message(m),
        })?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let open_note = if loaded.markdown.trim().is_empty() {
            Some(crate::tf!("{name} innehåller ingen text som kan läsas (skannade sidor?)", name = name))
        } else if loaded.pages_without_text > 0 {
            let n = loaded.pages_without_text;
            Some(crate::tf!("{n} av {total} sidor i {name} saknar text (skannade sidor?)", n = n, total = loaded.page_count, name = name))
        } else {
            None
        };
        Ok(Self {
            id,
            path: Some(path.to_path_buf()),
            saved_hash: hash_text(&loaded.markdown),
            saved_meta: (TextEncoding::Utf8, LineEnding::Lf),
            text: loaded.markdown,
            encoding: TextEncoding::Utf8,
            line_ending: LineEnding::Lf,
            dirty: false,
            docx: false,
            word_view: true,
            pdf: true,
            pdfv: None,
            open_note,
            docx_dropped: loaded.dropped,
            images: loaded.images,
            docx_source: None,
            untitled_index: 0,
            rev: 0,
        })
    }

    /// En flik i PDF-visaren: ingen text, aldrig ändrad.
    pub fn pdf_tab(id: u64, path: &Path, tab: crate::pdfview::chrome::PdfTab) -> Self {
        let mut d = Self::new_untitled(id, 0);
        d.path = Some(path.to_path_buf());
        d.pdfv = Some(Box::new(tab));
        d
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
                pdfv: None,
                open_note: (!loaded.dropped.is_empty()).then(|| {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    crate::tf!("{name} innehåller {lost} som inte visas i Kladd", name = name, lost = join_sv(&loaded.dropped))
                }),
                docx_dropped: loaded.dropped,
                images: loaded.images,
                docx_source: loaded.source.map(std::sync::Arc::new),
                untitled_index: 0,
                rev: 0,
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
                pdfv: None,
                open_note: (!loaded.dropped.is_empty()).then(|| {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    crate::tf!("{name} innehåller {lost} som inte visas i Kladd", name = name, lost = join_sv(&loaded.dropped))
                }),
                docx_dropped: loaded.dropped,
                images: loaded.images,
                docx_source: None,
                untitled_index: 0,
                rev: 0,
            });
        }
        if ext == "doc" && bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            return Err(crate::i18n::t("Det gamla .doc-formatet stöds inte. Öppna filen i Word eller LibreOffice och spara som .docx.").into());
        }
        let (mut text, encoding) = decode_owned(bytes)?;
        let crlf = text.as_bytes().windows(2).filter(|w| w == b"\r\n").count();
        let lf = text.bytes().filter(|&b| b == b'\n').count();
        // Bara rena CRLF-filer konverteras; blandade radslut lämnas orörda.
        let line_ending = if crlf > 0 && crlf == lf {
            text = crlf_to_lf(text);
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
            pdfv: None,
            open_note: None,
            docx_dropped: Vec::new(),
            images: Vec::new(),
            docx_source: None,
            untitled_index: 0,
            rev: 0,
        })
    }

    /// Sparar dokumentet. Returnerar varningar (t.ex. bilder som inte hittades).
    pub fn save_to(&mut self, path: &Path) -> Result<Vec<String>, String> {
        if self.is_export_target(path) {
            // Export (webbsida, e-bok): filen skrivs men fliken behåller sin sökväg, sitt format
            // och sitt ändrat-läge, till skillnad från PDF som blir flikens nya format.
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let images = &self.images;
            let resolve = |src: &str| resolve_image(src, &base, images);
            let written = if ext_of(path) == "epub" {
                crate::export::epub(&self.text, &title, &resolve)?
            } else {
                crate::export::html(&self.text, &title, &resolve)?
            };
            write_atomic(path, &written.bytes).map_err(|e| describe_io_error(&e, path))?;
            return Ok(written.missing_images);
        }
        let to_word = ext_of(path) == "docx";
        let to_odt = ext_of(path) == "odt";
        let to_pdf = ext_of(path) == "pdf";
        let mut warnings = Vec::new();
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let bytes: std::borrow::Cow<[u8]> = if to_pdf {
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let images = &self.images;
            let written = crate::pdf_write::write(&self.text, &title, &|src| resolve_image(src, &base, images))?;
            warnings.extend(written.missing_images);
            written.bytes.into()
        } else if to_odt {
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let images = &self.images;
            let written = crate::odt::write(&self.text, &title, &|src| resolve_image(src, &base, images))?;
            warnings.extend(written.missing_images);
            written.bytes.into()
        } else if to_word {
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let images = &self.images;
            let source = self.docx_source.as_deref();
            let written = crate::docx::write(&self.text, &title, &|src| resolve_image(src, &base, images), source)?;
            warnings.extend(written.missing_images);
            written.bytes.into()
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
            match self.line_ending {
                // Utan kopia av texten (vid 100 MB spelar det roll).
                LineEnding::Lf => encode(&self.text, self.encoding)?,
                LineEnding::CrLf => encode(&self.text.replace('\n', "\r\n"), self.encoding)?.into_owned().into(),
            }
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

    /// Sant om att spara till `path` är en export (.epub, eller .html/.htm från ett Markdown-baserat
    /// dokument) som lämnar flikens eget format och sökväg orörda.
    pub fn is_export_target(&self, path: &Path) -> bool {
        match ext_of(path).as_str() {
            "epub" => true,
            "html" | "htm" => self.is_markdown(),
            _ => false,
        }
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

    /// Lägger in text från en säkerhetskopia: dokumentet blir osparat även om texten råkar vara lika.
    pub fn set_recovered(&mut self, text: String, encoding: TextEncoding, line_ending: LineEnding) {
        self.text = text;
        self.encoding = encoding;
        self.line_ending = line_ending;
        self.dirty = true;
    }

    /// Ska anropas när texten har ändrats i editorn.
    pub fn on_edited(&mut self) {
        self.rev = self.rev.wrapping_add(1);
        // Stora texter hashas inte vid varje tangenttryck: de räknas som ändrade efter första ändringen.
        if self.text.len() >= crate::bigtext::BIG_BYTES {
            self.dirty = true;
            return;
        }
        self.dirty = hash_text(&self.text) != self.saved_hash
            || (self.encoding, self.line_ending) != self.saved_meta;
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
            None if self.untitled_index <= 1 => crate::i18n::t("Namnlös").to_owned(),
            None => crate::tf!("Namnlös {n}", n = self.untitled_index),
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
        if self.pdf || self.pdfv.is_some() {
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

    /// `language()` för visning i statusraden (på aktuellt språk); jämför alltid mot `language()`.
    pub fn language_label(&self) -> String {
        let l = self.language();
        match l.as_str() {
            "Konfiguration" => crate::i18n::t("Konfiguration").to_owned(),
            "Logg" => crate::i18n::t("Logg").to_owned(),
            "Undertexter" => crate::i18n::t("Undertexter").to_owned(),
            _ => l,
        }
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
        crate::platform::home_dir()?.join(rest)
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
        [rest @ .., last] => format!("{} {} {last}", rest.join(", "), crate::i18n::t("och")),
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
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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

pub(crate) fn describe_io_error(e: &std::io::Error, path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match e.kind() {
        std::io::ErrorKind::NotFound => crate::tf!("{name} finns inte längre.", name = name),
        std::io::ErrorKind::PermissionDenied => {
            crate::tf!("Du har inte behörighet till {name}.", name = name)
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

    #[test]
    fn crlf_conversion_in_place_and_owned_decode() {
        assert_eq!(crlf_to_lf("a\r\nb\r\n\u{e5}".to_owned()), "a\nb\n\u{e5}");
        assert_eq!(crlf_to_lf("a\rb\r\n".to_owned()), "a\rb\n");
        let (t, e) = decode_owned("h\u{e4}r".as_bytes().to_vec()).unwrap();
        assert_eq!((t.as_str(), e), ("h\u{e4}r", TextEncoding::Utf8));
        let (t, e) = decode_owned(vec![b'a', 0xE5]).unwrap();
        assert_eq!((t.as_str(), e), ("a\u{e5}", TextEncoding::Windows1252));
        assert!(decode_owned(vec![b'a', 0, b'b']).is_err());
        let (_, e) = decode_owned([&[0xEF, 0xBB, 0xBF][..], b"x"].concat()).unwrap();
        assert_eq!(e, TextEncoding::Utf8Bom);
    }

    #[test]
    fn large_texts_hash_fast_but_distinguish_changes() {
        let a = "x".repeat(2_000_000);
        let mut b = a.clone();
        b.replace_range(1_500_000..1_500_001, "y");
        assert_ne!(hash_text(&a), hash_text(&b));
        assert_eq!(hash_text(&a), hash_text(&a.clone()));
        // Små texter hashas som förut (samma som DefaultHasher).
        let mut h = std::collections::hash_map::DefaultHasher::new();
        "kort".hash(&mut h);
        assert_eq!(hash_text("kort"), h.finish());
    }

    #[test]
    fn big_documents_are_dirty_after_edit_without_hashing_and_load_with_progress() {
        let dir = std::env::temp_dir().join(format!("kladd-doc-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stor.log");
        let text = "rad av en logg\r\n".repeat(crate::bigtext::BIG_BYTES / 10);
        std::fs::write(&path, &text).unwrap();
        let progress = std::sync::atomic::AtomicU64::new(0);
        let mut d = Document::load_with_progress(1, &path, None, Some(&progress)).ok().unwrap();
        assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), text.len() as u64);
        assert_eq!(d.line_ending, LineEnding::CrLf);
        assert!(!d.text.contains('\r') && !d.dirty);
        d.text.push('x');
        d.on_edited();
        assert!(d.dirty);
        // Sparar utan kopia och ger tillbaka CRLF.
        let out = dir.join("ut.log");
        d.save_to(&out).unwrap();
        assert!(!d.dirty);
        let back = std::fs::read_to_string(&out).unwrap();
        assert!(back.ends_with("\r\nx") && back.len() == text.len() + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
