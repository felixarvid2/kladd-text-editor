//! Biblioteket och anteckningarna för EPUB-böcker. Allt ligger i en fil i datakatalogen
//! (`<data>/epub/bocker.json`, omslagsminiatyrer i `<data>/epub/omslag/`), aldrig i boken. Nyckeln för en bok är
//! dess unika id ur OPF-filen; saknas den används sökvägen. Antalet böcker och poster per bok har tak.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::anchor::{Note, now_secs};

/// Flest böcker som sparas (de senast lästa; böcker med anteckningar gallras sist).
pub const MAX_BOOKS: usize = 300;
/// Flest bokmärken och understrykningar per bok.
pub const MAX_NOTES: usize = 2000;
/// Omslagsminiatyrens höjd (bildpunkter).
pub const COVER_HEIGHT: u32 = 160;
/// Flest böcker som visas som omslag på startsidan, och kortets bredd där.
pub const SHELF_MAX: usize = 6;
pub const CARD_W: f32 = 122.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookRecord {
    pub key: String,
    pub path: String,
    pub title: String,
    pub author: String,
    /// Hur långt boken är läst (0..1).
    pub progress: f32,
    pub last_read: u64,
    /// Omslagsminiatyrens filnamn i omslagsmappen (tomt om boken saknar omslag).
    pub cover: String,
    /// Borttagen ur listan på startsidan (anteckningarna finns kvar).
    pub hidden: bool,
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub books: Vec<BookRecord>,
}

/// Bokens nyckel: id ur OPF-filen, annars sökvägen.
pub fn book_key(identifier: &str, path: &Path) -> String {
    let id = identifier.trim();
    if id.is_empty() { format!("path:{}", path.display()) } else { id.to_owned() }
}

pub fn dir() -> PathBuf {
    crate::platform::data_dir().join("epub")
}

pub fn cover_dir() -> PathBuf {
    dir().join("omslag")
}

impl Store {
    pub fn file() -> PathBuf {
        dir().join("bocker.json")
    }

    /// Läser filen; en saknad eller trasig fil ger ett tomt bibliotek.
    pub fn load() -> Store {
        std::fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Skriver filen (via en tillfällig fil så att en avbruten skrivning inte förstör den gamla).
    pub fn save(&self) {
        let file = Self::file();
        if let Some(d) = file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let Ok(json) = serde_json::to_string(self) else { return };
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }

    pub fn index_of(&self, key: &str, path: &str) -> Option<usize> {
        self.books.iter().position(|b| b.key == key).or_else(|| self.books.iter().position(|b| !path.is_empty() && b.path == path))
    }

    /// Bokens post (finns den inte skapas den). Nyckeln, sökvägen och metadata uppdateras.
    pub fn touch(&mut self, key: &str, path: &str, title: &str, author: &str) -> &mut BookRecord {
        let i = match self.index_of(key, path) {
            Some(i) => i,
            None => {
                self.books.push(BookRecord { key: key.to_owned(), ..BookRecord::default() });
                self.books.len() - 1
            }
        };
        let b = &mut self.books[i];
        b.key = key.to_owned();
        b.path = path.to_owned();
        b.title = title.to_owned();
        b.author = author.to_owned();
        b
    }

    /// Boken öppnas: senast läst, synlig i listan, och först.
    pub fn opened(&mut self, key: &str, path: &str, title: &str, author: &str) {
        self.touch(key, path, title, author);
        let i = self.index_of(key, path).unwrap_or(0);
        let mut b = self.books.remove(i);
        b.last_read = now_secs();
        b.hidden = false;
        self.books.insert(0, b);
        self.prune();
    }

    pub fn notes_for(&self, key: &str, path: &str) -> Vec<Note> {
        self.index_of(key, path).map(|i| self.books[i].notes.clone()).unwrap_or_default()
    }

    pub fn set_notes(&mut self, key: &str, path: &str, mut notes: Vec<Note>) {
        if notes.len() > MAX_NOTES {
            // De äldsta går först.
            notes.sort_by_key(|n| std::cmp::Reverse(n.created));
            notes.truncate(MAX_NOTES);
        }
        if let Some(i) = self.index_of(key, path) {
            self.books[i].notes = notes;
        }
    }

    pub fn set_progress(&mut self, key: &str, path: &str, progress: f32) {
        if let Some(i) = self.index_of(key, path) {
            self.books[i].progress = progress.clamp(0.0, 1.0);
        }
    }

    pub fn hide(&mut self, path: &str) {
        for b in self.books.iter_mut().filter(|b| b.path == path) {
            b.hidden = true;
        }
    }

    /// Böckerna i listan, senast lästa först.
    pub fn shelf(&self) -> Vec<&BookRecord> {
        let mut v: Vec<&BookRecord> = self.books.iter().filter(|b| !b.hidden && !b.path.is_empty()).collect();
        v.sort_by_key(|b| std::cmp::Reverse(b.last_read));
        v
    }

    /// Håller antalet böcker under taket: först de äldsta utan anteckningar, sedan de äldsta.
    pub fn prune(&mut self) {
        while self.books.len() > MAX_BOOKS {
            let pick = self
                .books
                .iter()
                .enumerate()
                .filter(|(_, b)| b.notes.is_empty())
                .min_by_key(|(_, b)| b.last_read)
                .or_else(|| self.books.iter().enumerate().min_by_key(|(_, b)| b.last_read))
                .map(|(i, _)| i);
            let Some(i) = pick else { break };
            let gone = self.books.remove(i);
            if !gone.cover.is_empty() {
                let _ = std::fs::remove_file(cover_dir().join(&gone.cover));
            }
        }
    }
}

/// Skriver omslagsminiatyren (PNG, `COVER_HEIGHT` hög) för en bok och ger filnamnet. Finns den redan görs inget.
pub fn save_cover(key: &str, cover_bytes: impl FnOnce() -> Option<Vec<u8>>) -> Option<String> {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    let name = format!("{:016x}.png", h.finish());
    let file = cover_dir().join(&name);
    if file.exists() {
        return Some(name);
    }
    let bytes = cover_bytes()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let thumb = img.resize(COVER_HEIGHT * 2, COVER_HEIGHT, image::imageops::FilterType::Triangle);
    let mut png = Vec::new();
    thumb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    std::fs::create_dir_all(cover_dir()).ok()?;
    std::fs::write(&file, png).ok()?;
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(store: &mut Store, key: &str, path: &str, t: u64) {
        store.touch(key, path, key, "");
        let i = store.index_of(key, path).unwrap();
        store.books[i].last_read = t;
    }

    #[test]
    fn nyckeln_är_bokens_id_med_sökvägen_som_reserv() {
        assert_eq!(book_key(" urn:uuid:123 ", Path::new("/a/b.epub")), "urn:uuid:123");
        assert_eq!(book_key("", Path::new("/a/b.epub")), "path:/a/b.epub");
    }

    #[test]
    fn samma_bok_hittas_via_id_eller_sökväg() {
        let mut s = Store::default();
        s.touch("id1", "/a/bok.epub", "Titel", "Förf");
        s.set_notes("id1", "/a/bok.epub", vec![Note { id: 1, raw: "x".into(), ..Note::default() }]);
        // Boken har flyttats: id:t hittar posten och sökvägen uppdateras.
        s.touch("id1", "/b/bok.epub", "Titel", "Förf");
        assert_eq!(s.books.len(), 1);
        assert_eq!(s.books[0].path, "/b/bok.epub");
        assert_eq!(s.notes_for("id1", "/b/bok.epub").len(), 1);
        // Ett ändrat id men samma sökväg: posten följer med (och får nytt id).
        s.touch("id2", "/b/bok.epub", "Titel", "Förf");
        assert_eq!(s.books.len(), 1);
        assert_eq!(s.books[0].key, "id2");
        assert_eq!(s.notes_for("id2", "").len(), 1, "anteckningarna finns kvar");
    }

    #[test]
    fn spara_och_läsa_tillbaka_och_trasig_fil() {
        let mut s = Store::default();
        s.opened("k", "/x.epub", "T", "F");
        s.set_notes("k", "/x.epub", vec![Note { id: 4, color: 2, note: "hej".into(), raw: "abc".into(), ..Note::default() }]);
        let json = serde_json::to_string(&s).unwrap();
        let back: Store = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        // Gamla eller främmande fält och fel typer ger tomt i stället för panic.
        assert!(serde_json::from_str::<Store>("{\"books\":[{\"key\":\"a\",\"extra\":1}]}").is_ok());
        assert!(serde_json::from_str::<Store>("inte json").is_err());
        let _ = Store::load(); // ingen panic om filen saknas
    }

    #[test]
    fn taket_gallrar_äldsta_böcker_utan_anteckningar_först() {
        let mut s = Store::default();
        for i in 0..(MAX_BOOKS + 5) {
            rec(&mut s, &format!("b{i}"), &format!("/b{i}.epub"), 1000 + i as u64);
        }
        // Den äldsta har anteckningar och ska därför överleva.
        s.set_notes("b0", "/b0.epub", vec![Note { id: 1, raw: "x".into(), ..Note::default() }]);
        s.prune();
        assert_eq!(s.books.len(), MAX_BOOKS);
        assert!(s.index_of("b0", "").is_some(), "bok med anteckningar behålls");
        assert!(s.index_of("b1", "").is_none() && s.index_of("b5", "").is_none(), "äldsta utan anteckningar går först");
        assert!(s.index_of(&format!("b{}", MAX_BOOKS + 4), "").is_some());
    }

    #[test]
    fn tak_för_poster_per_bok_behåller_de_nyaste() {
        let mut s = Store::default();
        s.touch("k", "/x.epub", "", "");
        let notes: Vec<Note> = (0..MAX_NOTES + 10).map(|i| Note { id: i as u64, created: i as u64, raw: "x".into(), ..Note::default() }).collect();
        s.set_notes("k", "/x.epub", notes);
        let n = s.notes_for("k", "");
        assert_eq!(n.len(), MAX_NOTES);
        assert!(n.iter().all(|n| n.id >= 10), "de tio äldsta föll bort");
    }

    #[test]
    fn hylla_döljer_borttagna_och_sorterar_senast_läst_först() {
        let mut s = Store::default();
        rec(&mut s, "a", "/a.epub", 10);
        rec(&mut s, "b", "/b.epub", 30);
        rec(&mut s, "c", "/c.epub", 20);
        assert_eq!(s.shelf().iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["b", "c", "a"]);
        s.hide("/b.epub");
        assert_eq!(s.shelf().len(), 2);
        s.opened("b", "/b.epub", "b", "");
        assert_eq!(s.shelf()[0].key, "b", "öppnad igen: synlig och först");
    }
}
