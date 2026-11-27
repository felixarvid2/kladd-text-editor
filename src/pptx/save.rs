//! Sparar en redigerad presentation som zip.
//!
//! Posterna skrivs i originalets ordning. En post som ingen redigering har rört kopieras som den
//! komprimerade posten den är (`raw_copy_file`), så okända delar, `customXml`, typsnitt och media följer
//! med byte för byte. Ändrade delar skrivs om, borttagna utelämnas och nya läggs sist.

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};

use super::package::Package;

fn options(name: &str) -> zip::write::SimpleFileOptions {
    let lower = name.to_lowercase();
    let deflate = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6));
    // Bilder är redan komprimerade.
    if [".png", ".jpg", ".jpeg", ".gif"].iter().any(|e| lower.ends_with(e)) {
        deflate.compression_method(zip::CompressionMethod::Stored).compression_level(None)
    } else {
        deflate
    }
}

/// Skriver paketet. `base` är zip-filen som öppnades (eller senast sparades) och `touched` de delar
/// som kan skilja sig från den.
pub fn write_zip(base: &[u8], pkg: &Package, touched: &HashSet<String>) -> Result<Vec<u8>, String> {
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut seen: HashSet<String> = HashSet::new();
    match zip::ZipArchive::new(Cursor::new(base)) {
        Ok(mut zin) => {
            for i in 0..zin.len() {
                let (name, is_dir) = match zin.by_index_raw(i) {
                    Ok(f) => (f.name().to_owned(), f.is_dir()),
                    Err(_) => continue,
                };
                let norm = name.trim_start_matches('/').to_owned();
                seen.insert(norm.clone());
                let copy_raw = |zin: &mut zip::ZipArchive<Cursor<&[u8]>>, out: &mut zip::ZipWriter<Cursor<Vec<u8>>>| -> Result<(), String> {
                    let f = zin.by_index_raw(i).map_err(|e| e.to_string())?;
                    out.raw_copy_file(f).map_err(|e| e.to_string())
                };
                if is_dir || !touched.contains(&norm) {
                    copy_raw(&mut zin, &mut out)?;
                    continue;
                }
                let Some(cur) = pkg.parts.get(&norm) else { continue };
                // Rörd men lika som originalet (t.ex. ändrad och ångrad): kopiera posten.
                let same = zin.by_index(i).ok().and_then(|mut f| {
                    let mut buf = Vec::with_capacity(f.size() as usize);
                    f.read_to_end(&mut buf).ok().map(|_| buf == **cur)
                });
                if same == Some(true) {
                    copy_raw(&mut zin, &mut out)?;
                } else {
                    out.start_file(name.as_str(), options(&name)).map_err(|e| e.to_string())?;
                    out.write_all(cur).map_err(|e| e.to_string())?;
                }
            }
        }
        Err(_) => {
            // Utan original (ska inte hända): alla delar, innehållstyperna först.
            seen.clear();
        }
    }
    let mut fresh: Vec<&String> = pkg.parts.keys().filter(|k| !seen.contains(*k)).collect();
    fresh.sort_by_key(|k| (k.as_str() != "[Content_Types].xml", k.as_str()));
    for name in fresh {
        out.start_file(name.as_str(), options(name)).map_err(|e| e.to_string())?;
        out.write_all(&pkg.parts[name]).map_err(|e| e.to_string())?;
    }
    out.finish().map(|c| c.into_inner()).map_err(|e| e.to_string())
}
