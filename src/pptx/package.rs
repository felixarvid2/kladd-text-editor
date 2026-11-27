//! OPC-paketet: zip-delarna, `[Content_Types].xml` och relationerna.
//!
//! Alla delar packas upp vid öppning och behålls som råa byte (`parts`), så att en senare
//! källbevarande redigering kan skriva tillbaka orörda delar exakt som de var.

use std::collections::HashMap;
use std::io::Read;

use super::xml::{self, Node};

/// En relation från en del.
#[derive(Clone, Debug)]
pub struct Rel {
    pub id: String,
    /// Typens sista led (`slideLayout`, `image`, `diagramDrawing` …).
    pub kind: String,
    /// Absolut delnamn utan inledande snedstreck (för interna mål), annars målet som det står.
    pub target: String,
    #[allow(dead_code)]
    pub external: bool,
}

/// Ett uppackat paket.
pub struct Package {
    /// Delnamn (utan inledande `/`) → innehåll.
    pub parts: HashMap<String, Vec<u8>>,
    /// Delnamn → innehållstyp (från `Override`, annars `Default` efter filändelse).
    pub content_types: HashMap<String, String>,
    defaults: HashMap<String, String>,
}

/// Största uppackade storlek vi accepterar (skydd mot zip-bomber).
const MAX_TOTAL: u64 = 1500 * 1024 * 1024;

impl Package {
    pub fn open(bytes: &[u8]) -> Result<Package, String> {
        // OLE-behållare: krypterad .pptx eller det äldre .ppt-formatet.
        if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            return Err(crate::i18n::t("Presentationen är lösenordsskyddad eller i det äldre .ppt-formatet, som Kladd inte kan öppna än.").to_owned());
        }
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|_| crate::i18n::t("Filen är inte en giltig PowerPoint-fil (inget zip-arkiv).").to_owned())?;
        let mut parts = HashMap::new();
        let mut total = 0u64;
        for i in 0..zip.len() {
            let Ok(mut f) = zip.by_index(i) else { continue };
            if f.is_dir() {
                continue;
            }
            total = total.saturating_add(f.size());
            if total > MAX_TOTAL {
                return Err(crate::i18n::t("PowerPoint-filen är för stor uppackad.").to_owned());
            }
            let name = f.name().trim_start_matches('/').to_owned();
            let mut buf = Vec::with_capacity(f.size().min(64 * 1024 * 1024) as usize);
            if f.read_to_end(&mut buf).is_err() {
                continue;
            }
            parts.insert(name, buf);
        }
        let mut pkg = Package { parts, content_types: HashMap::new(), defaults: HashMap::new() };
        if let Some(ct) = pkg.parts.get("[Content_Types].xml").and_then(|b| xml::parse(b)) {
            for c in &ct.children {
                match c.local() {
                    "Default" => {
                        if let (Some(e), Some(t)) = (c.attr("Extension"), c.attr("ContentType")) {
                            pkg.defaults.insert(e.to_lowercase(), t.to_owned());
                        }
                    }
                    "Override" => {
                        if let (Some(p), Some(t)) = (c.attr("PartName"), c.attr("ContentType")) {
                            pkg.content_types.insert(p.trim_start_matches('/').to_owned(), t.to_owned());
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(pkg)
    }

    /// Delens innehållstyp.
    #[allow(dead_code)]
    pub fn content_type(&self, part: &str) -> Option<&str> {
        if let Some(t) = self.content_types.get(part) {
            return Some(t);
        }
        let ext = part.rsplit('.').next()?.to_lowercase();
        self.defaults.get(&ext).map(|s| s.as_str())
    }

    /// Råa byte för en del. Namnet jämförs också utan skiftlägeskänslighet (vissa verktyg skriver fel).
    pub fn read(&self, part: &str) -> Option<&[u8]> {
        let part = part.trim_start_matches('/');
        if let Some(b) = self.parts.get(part) {
            return Some(b);
        }
        let low = part.to_lowercase();
        self.parts.iter().find(|(k, _)| k.to_lowercase() == low).map(|(_, v)| v.as_slice())
    }

    /// En XML-del som träd.
    pub fn xml(&self, part: &str) -> Option<Node> {
        xml::parse(self.read(part)?)
    }

    /// Relationerna från en del (`ppt/slides/slide1.xml` → `ppt/slides/_rels/slide1.xml.rels`).
    pub fn rels(&self, part: &str) -> Vec<Rel> {
        let (dir, file) = match part.rfind('/') {
            Some(i) => (&part[..i], &part[i + 1..]),
            None => ("", part),
        };
        let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
        let Some(root) = self.xml(&rels_name) else { return Vec::new() };
        root.children
            .iter()
            .filter(|c| c.local() == "Relationship")
            .filter_map(|c| {
                let id = c.attr("Id")?.to_owned();
                let kind = c.attr("Type").unwrap_or("").rsplit('/').next().unwrap_or("").to_owned();
                let raw = c.attr("Target")?;
                let external = c.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
                let target = if external { raw.to_owned() } else { resolve(dir, raw) };
                Some(Rel { id, kind, target, external })
            })
            .collect()
    }
}

/// Löser ett relativt mål mot källdelens katalog (`ppt/slides` + `../media/a.png` → `ppt/media/a.png`).
pub fn resolve(dir: &str, target: &str) -> String {
    let target = crate::epub::read::percent_decode(target);
    let mut out: Vec<&str> = if target.starts_with('/') { Vec::new() } else { dir.split('/').filter(|s| !s.is_empty()).collect() };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

/// Relationen med id.
pub fn rel<'a>(rels: &'a [Rel], id: &str) -> Option<&'a Rel> {
    rels.iter().find(|r| r.id == id)
}

/// Första relationen av en typ.
pub fn rel_of_kind<'a>(rels: &'a [Rel], kind: &str) -> Option<&'a Rel> {
    rels.iter().find(|r| r.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relativa_mal() {
        assert_eq!(resolve("ppt/slides", "../media/image1.png"), "ppt/media/image1.png");
        assert_eq!(resolve("ppt/slides", "/ppt/media/a.png"), "ppt/media/a.png");
        assert_eq!(resolve("ppt", "slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(resolve("", "ppt/presentation.xml"), "ppt/presentation.xml");
        assert_eq!(resolve("ppt/slides", "../media/bild%201.png"), "ppt/media/bild 1.png");
    }
}
