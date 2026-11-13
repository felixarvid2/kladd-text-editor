//! Sidorganisering för PDF-visaren: flytta, ta bort, rotera, extrahera och infoga sidor med `lopdf`.
//!
//! Alla operationer tar och ger PDF-bytes. Krypterade filer avkrypteras vid inläsning (som
//! `print::decrypted_pdf`): resultatet är alltid en okrypterad fil, så sparas den över originalet
//! försvinner lösenordsskyddet. Sidor är 0-baserade överallt. Sidträdet plattas ut först (ärvda
//! `Resources`, `MediaBox`, `CropBox` och `Rotate` skrivs in på varje sida), så en sida ser likadan
//! ut efter flytt eller infogning. Referenser till borttagna sidor (bokmärken, länkar) blir `null`.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use lopdf::{Document, Object, ObjectId};

/// Högsta antal sparade versioner för Ångra.
pub const MAX_HISTORY: usize = 30;
/// Högsta sammanlagda storlek på sparade versioner (byte).
pub const MAX_HISTORY_BYTES: usize = 256 * 1024 * 1024;

const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Läser en PDF (avkrypterar om den är krypterad; `password` eller tomt lösenord).
fn load(bytes: &[u8], password: Option<&str>) -> Result<Document, String> {
    let pw = password.unwrap_or("");
    let mut opts = lopdf::LoadOptions::default();
    opts.password = Some(pw.to_owned());
    let mut doc = Document::load_mem_with_options(bytes, opts).map_err(|e| format!("PDF:en kunde inte läsas: {e}"))?;
    if doc.is_encrypted() {
        doc.decrypt(pw).map_err(|_| "PDF:en är lösenordsskyddad".to_owned())?;
    }
    doc.trailer.remove(b"Encrypt");
    doc.encryption_state = None;
    Ok(doc)
}

fn save(mut doc: Document) -> Result<Vec<u8>, String> {
    doc.prune_objects();
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| format!("PDF:en kunde inte skrivas: {e}"))?;
    Ok(out)
}

fn pages_root(doc: &Document) -> Result<ObjectId, String> {
    doc.catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|_| "PDF:en saknar sidträd".to_owned())
}

/// Skriver in ärvda sidegenskaper på varje sida och gör sidorna till direkta barn av roten.
/// Ger sidornas id i ordning.
fn flatten(doc: &mut Document) -> Result<Vec<ObjectId>, String> {
    let root = pages_root(doc)?;
    let ids: Vec<ObjectId> = doc.page_iter().collect();
    for &id in &ids {
        let mut inherited: Vec<(&[u8], Object)> = Vec::new();
        let page = doc.get_dictionary(id).map_err(|e| e.to_string())?;
        let mut parent = page.get(b"Parent").and_then(Object::as_reference).ok();
        let mut hops = 0;
        while let Some(pid) = parent {
            hops += 1;
            let Ok(node) = doc.get_dictionary(pid) else { break };
            for key in INHERITED {
                if !page.has(key) && !inherited.iter().any(|(k, _)| *k == key) {
                    if let Ok(v) = node.get(key) {
                        inherited.push((key, v.clone()));
                    }
                }
            }
            parent = node.get(b"Parent").and_then(Object::as_reference).ok().filter(|_| hops < 64);
        }
        let page = doc.get_dictionary_mut(id).map_err(|e| e.to_string())?;
        for (k, v) in inherited {
            page.set(k.to_vec(), v);
        }
        page.set("Parent", Object::Reference(root));
    }
    set_kids(doc, root, &ids)?;
    Ok(ids)
}

fn set_kids(doc: &mut Document, root: ObjectId, ids: &[ObjectId]) -> Result<(), String> {
    let node = doc.get_dictionary_mut(root).map_err(|e| e.to_string())?;
    node.set("Kids", Object::Array(ids.iter().map(|i| Object::Reference(*i)).collect()));
    node.set("Count", ids.len() as i64);
    Ok(())
}

/// Antal sidor.
pub fn page_count(bytes: &[u8]) -> Result<usize, String> {
    Ok(load(bytes, None)?.page_iter().count())
}

/// Ny ordning: `order[ny plats] = gammalt sidnummer`. Måste vara en permutation av alla sidor.
pub fn reorder(bytes: &[u8], order: &[usize]) -> Result<Vec<u8>, String> {
    let mut doc = load(bytes, None)?;
    let ids = flatten(&mut doc)?;
    let uniq: HashSet<usize> = order.iter().copied().collect();
    if order.len() != ids.len() || uniq.len() != ids.len() || order.iter().any(|&i| i >= ids.len()) {
        return Err("Ogiltig sidordning".to_owned());
    }
    let root = pages_root(&doc)?;
    let new: Vec<ObjectId> = order.iter().map(|&i| ids[i]).collect();
    set_kids(&mut doc, root, &new)?;
    save(doc)
}

/// Behåller bara sidorna i `keep`; övriga tas bort helt.
fn retain_pages(doc: &mut Document, keep: &BTreeSet<usize>) -> Result<(), String> {
    let ids = flatten(doc)?;
    let root = pages_root(doc)?;
    let (kept, removed): (Vec<_>, Vec<_>) = ids.iter().enumerate().partition(|(i, _)| keep.contains(i));
    let kept: Vec<ObjectId> = kept.into_iter().map(|(_, id)| *id).collect();
    let removed: HashSet<ObjectId> = removed.into_iter().map(|(_, id)| *id).collect();
    set_kids(doc, root, &kept)?;
    // Bokmärken och länkar till borttagna sidor ska inte hålla kvar sidorna.
    doc.traverse_objects(|o| {
        if matches!(o, Object::Reference(id) if removed.contains(id)) {
            *o = Object::Null;
        }
    });
    for id in &removed {
        doc.objects.remove(id);
    }
    Ok(())
}

fn check_pages(pages: &[usize], count: usize) -> Result<BTreeSet<usize>, String> {
    let set: BTreeSet<usize> = pages.iter().copied().collect();
    if set.is_empty() || set.iter().any(|&p| p >= count) {
        return Err("Ogiltigt sidurval".to_owned());
    }
    Ok(set)
}

/// Tar bort sidorna. Minst en sida måste vara kvar.
pub fn delete(bytes: &[u8], pages: &[usize]) -> Result<Vec<u8>, String> {
    let mut doc = load(bytes, None)?;
    let count = doc.page_iter().count();
    let del = check_pages(pages, count)?;
    if del.len() >= count {
        return Err("Minst en sida måste vara kvar".to_owned());
    }
    let keep: BTreeSet<usize> = (0..count).filter(|i| !del.contains(i)).collect();
    retain_pages(&mut doc, &keep)?;
    save(doc)
}

/// Ny PDF med bara de valda sidorna (i dokumentets ordning).
pub fn extract(bytes: &[u8], pages: &[usize]) -> Result<Vec<u8>, String> {
    let mut doc = load(bytes, None)?;
    let keep = check_pages(pages, doc.page_iter().count())?;
    retain_pages(&mut doc, &keep)?;
    save(doc)
}

/// Roterar sidorna `quarter_turns` kvarts varv medurs (negativt = moturs), via `/Rotate`.
pub fn rotate(bytes: &[u8], pages: &[usize], quarter_turns: i32) -> Result<Vec<u8>, String> {
    let mut doc = load(bytes, None)?;
    let ids = flatten(&mut doc)?;
    let set = check_pages(pages, ids.len())?;
    for i in set {
        let page = doc.get_dictionary_mut(ids[i]).map_err(|e| e.to_string())?;
        let cur = page.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0);
        page.set("Rotate", (cur + 90 * quarter_turns as i64).rem_euclid(360));
    }
    save(doc)
}

/// Infogar alla sidor i `other` så att de hamnar på plats `at` (0 = först, antal sidor = sist).
/// Objekten numreras om så att inget krockar; typsnitt och bilder följer med. Den infogade
/// filens bokmärken och formulär tas inte med.
pub fn insert(bytes: &[u8], other: &[u8], other_password: Option<&str>, at: usize) -> Result<Vec<u8>, String> {
    let mut doc = load(bytes, None)?;
    let mut ids = flatten(&mut doc)?;
    let mut add = load(other, other_password).map_err(|e| format!("Den infogade filen: {e}"))?;
    add.renumber_objects_with(doc.max_id + 1);
    let add_ids = flatten(&mut add)?;
    if add_ids.is_empty() {
        return Err("Den infogade filen har inga sidor".to_owned());
    }
    let root = pages_root(&doc)?;
    doc.max_id = doc.max_id.max(add.objects.keys().map(|k| k.0).max().unwrap_or(0));
    for (id, obj) in std::mem::take(&mut add.objects) {
        doc.objects.insert(id, obj);
    }
    // Den infogade filens egen sidroot och katalog blir oanvända och rensas av `prune_objects`.
    for &id in &add_ids {
        doc.get_dictionary_mut(id).map_err(|e| e.to_string())?.set("Parent", Object::Reference(root));
    }
    let at = at.min(ids.len());
    ids.splice(at..at, add_ids);
    set_kids(&mut doc, root, &ids)?;
    save(doc)
}

// ------------------------------------------------------------ urval och ordning (utan PDF)

/// Ny sidordning när `selected` flyttas så att de hamnar före sidan `before` (0..=antal, räknat i
/// den gamla ordningen). Ger (ordning, de flyttade sidornas nya platser).
pub fn move_order(count: usize, selected: &BTreeSet<usize>, before: usize) -> (Vec<usize>, BTreeSet<usize>) {
    let moved: Vec<usize> = selected.iter().copied().filter(|&i| i < count).collect();
    let mut order: Vec<usize> = Vec::with_capacity(count);
    let mut new_sel = BTreeSet::new();
    let mut placed = false;
    for i in 0..=count {
        if i == before.min(count) && !placed {
            placed = true;
            for &m in &moved {
                new_sel.insert(order.len());
                order.push(m);
            }
        }
        if i < count && !selected.contains(&i) {
            order.push(i);
        }
    }
    (order, new_sel)
}

/// Urvalet efter borttagning: sidan som hamnat där den första borttagna låg.
pub fn selection_after_delete(count_after: usize, first_deleted: usize) -> BTreeSet<usize> {
    if count_after == 0 {
        return BTreeSet::new();
    }
    BTreeSet::from([first_deleted.min(count_after - 1)])
}

// ------------------------------------------------------------ ångra och gör om

/// Tidigare och senare versioner av filens bytes, med tak på antal och minne.
#[derive(Default)]
pub struct History {
    undo: Vec<Arc<Vec<u8>>>,
    redo: Vec<Arc<Vec<u8>>>,
}

impl History {
    /// Sparar `prev` (versionen före en ändring) och släpper "gör om"-grenen.
    pub fn push(&mut self, prev: Arc<Vec<u8>>) {
        self.redo.clear();
        self.undo.push(prev);
        self.trim();
    }

    fn trim(&mut self) {
        let total = |v: &Vec<Arc<Vec<u8>>>| v.iter().map(|b| b.len()).sum::<usize>();
        while self.undo.len() > MAX_HISTORY || (self.undo.len() > 1 && total(&self.undo) > MAX_HISTORY_BYTES) {
            self.undo.remove(0);
        }
        while self.redo.len() > MAX_HISTORY || (self.redo.len() > 1 && total(&self.redo) > MAX_HISTORY_BYTES) {
            self.redo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Ger föregående version och sparar `current` för "gör om".
    pub fn undo(&mut self, current: Arc<Vec<u8>>) -> Option<Arc<Vec<u8>>> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        self.trim();
        Some(prev)
    }

    /// Ger nästa version och sparar `current` för "ångra".
    pub fn redo(&mut self, current: Arc<Vec<u8>>) -> Option<Arc<Vec<u8>>> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        self.trim();
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Dictionary, Stream, dictionary};

    /// En PDF med sidorna "<tag> 1".."<tag> n". Sidorna delar typsnitt och ärver `MediaBox` och
    /// `Resources` från en mellannod, för att prova utplattningen.
    fn make(n: usize, tag: &str) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let res = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let root = doc.new_object_id();
        let mid = doc.new_object_id();
        let mut kids = Vec::new();
        for i in 1..=n {
            let content = format!("BT /F1 24 Tf 72 700 Td ({tag} {i}) Tj ET");
            let cid = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
            let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => mid, "Contents" => cid });
            kids.push(Object::Reference(page));
        }
        doc.objects.insert(
            mid,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Parent" => root, "Kids" => kids, "Count" => n as i64,
                "MediaBox" => vec![0.into(), 0.into(), 400.into(), 800.into()], "Resources" => res,
            }),
        );
        doc.objects.insert(root, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(mid)], "Count" => n as i64 }));
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
        doc.trailer.set("Root", cat);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    /// Sidornas markörtext i ordning.
    fn markers(bytes: &[u8]) -> Vec<String> {
        let doc = Document::load_mem(bytes).unwrap();
        doc.page_iter()
            .map(|id| {
                let c = String::from_utf8_lossy(&doc.get_page_content(id)).into_owned();
                let a = c.find('(').unwrap() + 1;
                c[a..c.find(')').unwrap()].to_owned()
            })
            .collect()
    }

    fn rotations(bytes: &[u8]) -> Vec<i64> {
        let doc = Document::load_mem(bytes).unwrap();
        doc.page_iter().map(|id| doc.get_dictionary(id).unwrap().get(b"Rotate").and_then(Object::as_i64).unwrap_or(0)).collect()
    }

    /// Alla sidor ska ha egen `MediaBox` och `Resources` med typsnittet, och filen ska öppnas av motorn.
    fn check_opens(bytes: &[u8], pages: usize) {
        let doc = Document::load_mem(bytes).unwrap();
        for id in doc.page_iter() {
            // `Resources` är en referens i testfilen: antingen direkt ordbok eller id i listan.
            let (res, ids) = doc.get_page_resources(id).unwrap();
            assert!(res.is_some_and(|r| r.has(b"Font")) || ids.iter().any(|i| doc.get_dictionary(*i).is_ok_and(|d| d.has(b"Font"))));
            assert!(doc.get_dictionary(id).unwrap().has(b"MediaBox"));
        }
        let engine = crate::pdfview::engine::PdfEngine::open(Arc::new(bytes.to_vec()), None, egui::Context::default()).ok().expect("motorn öppnar filen");
        assert_eq!(engine.page_count(), pages);
    }

    #[test]
    fn reorders_pages() {
        let out = reorder(&make(4, "Sida"), &[2, 0, 3, 1]).unwrap();
        assert_eq!(markers(&out), ["Sida 3", "Sida 1", "Sida 4", "Sida 2"]);
        check_opens(&out, 4);
        assert!(reorder(&make(3, "Sida"), &[0, 1]).is_err());
        assert!(reorder(&make(3, "Sida"), &[0, 1, 1]).is_err());
    }

    #[test]
    fn deletes_pages() {
        let src = make(5, "Sida");
        let out = delete(&src, &[1, 3]).unwrap();
        assert_eq!(markers(&out), ["Sida 1", "Sida 3", "Sida 5"]);
        check_opens(&out, 3);
        assert!(out.len() < src.len());
        assert!(delete(&src, &[0, 1, 2, 3, 4]).is_err());
        assert!(delete(&src, &[9]).is_err());
        assert!(delete(&src, &[]).is_err());
    }

    #[test]
    fn rotates_pages() {
        let out = rotate(&make(3, "Sida"), &[0, 2], 1).unwrap();
        assert_eq!(rotations(&out), [90, 0, 90]);
        let out = rotate(&out, &[0, 1], -1).unwrap();
        assert_eq!(rotations(&out), [0, 270, 90]);
        check_opens(&out, 3);
    }

    #[test]
    fn extracts_pages() {
        let out = extract(&make(5, "Sida"), &[3, 1]).unwrap();
        assert_eq!(markers(&out), ["Sida 2", "Sida 4"]);
        check_opens(&out, 2);
    }

    #[test]
    fn inserts_pages_from_another_file() {
        let a = make(3, "A");
        let b = make(2, "B");
        let out = insert(&a, &b, None, 1).unwrap();
        assert_eq!(markers(&out), ["A 1", "B 1", "B 2", "A 2", "A 3"]);
        check_opens(&out, 5);
        assert_eq!(markers(&insert(&a, &b, None, 0).unwrap()), ["B 1", "B 2", "A 1", "A 2", "A 3"]);
        assert_eq!(markers(&insert(&a, &b, None, 99).unwrap()), ["A 1", "A 2", "A 3", "B 1", "B 2"]);
        // Slå ihop med sig själv: objekten får inte krocka.
        let twice = insert(&a, &a, None, 3).unwrap();
        assert_eq!(markers(&twice), ["A 1", "A 2", "A 3", "A 1", "A 2", "A 3"]);
        check_opens(&twice, 6);
        // Flera operationer efter varandra.
        let r = delete(&reorder(&out, &[4, 3, 2, 1, 0]).unwrap(), &[0]).unwrap();
        assert_eq!(markers(&r), ["A 2", "B 2", "B 1", "A 1"]);
    }

    #[test]
    fn password_on_plain_file_is_harmless() {
        assert_eq!(markers(&insert(&make(1, "A"), &make(1, "B"), Some("x"), 1).unwrap()), ["A 1", "B 1"]);
        let engine = crate::pdfview::engine::PdfEngine::open(Arc::new(make(2, "S")), Some("x"), egui::Context::default());
        assert!(engine.is_ok());
        assert_eq!(page_count(&make(2, "S")).unwrap(), 2);
    }

    #[test]
    fn move_plans() {
        let sel = BTreeSet::from([1, 2]);
        assert_eq!(move_order(5, &sel, 5), (vec![0, 3, 4, 1, 2], BTreeSet::from([3, 4])));
        assert_eq!(move_order(5, &sel, 0), (vec![1, 2, 0, 3, 4], BTreeSet::from([0, 1])));
        assert_eq!(move_order(5, &sel, 2).0, [0, 1, 2, 3, 4]);
        assert_eq!(move_order(5, &BTreeSet::from([4]), 1), (vec![0, 4, 1, 2, 3], BTreeSet::from([1])));
        assert_eq!(selection_after_delete(3, 5), BTreeSet::from([2]));
        assert!(selection_after_delete(0, 0).is_empty());
    }

    #[test]
    fn history_undo_redo_and_cap() {
        let v = |n: u8| Arc::new(vec![n]);
        let mut h = History::default();
        assert!(!h.can_undo());
        h.push(v(1));
        h.push(v(2));
        assert_eq!(h.undo(v(3)).unwrap()[0], 2);
        assert_eq!(h.undo(v(2)).unwrap()[0], 1);
        assert!(h.undo(v(1)).is_none());
        assert_eq!(h.redo(v(1)).unwrap()[0], 2);
        h.push(v(9));
        assert!(!h.can_redo());
        for i in 0..100 {
            h.push(v(i));
        }
        assert_eq!(h.undo.len(), MAX_HISTORY);
        let mut h = History::default();
        h.push(Arc::new(vec![0; MAX_HISTORY_BYTES / 2 + 1]));
        h.push(Arc::new(vec![0; MAX_HISTORY_BYTES / 2 + 1]));
        assert_eq!(h.undo.len(), 1);
    }
}
