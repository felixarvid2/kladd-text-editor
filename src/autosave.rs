//! Automatisk sparning: ändrade flikar med sökväg sparas till sin egen fil en stund efter att
//! skrivandet stannat och när fönstret tappar fokus eller fliken byts. Reglerna för vad som får
//! sparas automatiskt och kontrollen av att filen inte ändrats på disk ligger här som rena funktioner.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use crate::doc::Document;
use crate::tf;

/// Sekunder utan ändringar innan en flik sparas.
pub const DELAY_SECS: f64 = 2.0;

/// Ändringstid och storlek, som när filen öppnades eller senast sparades av Kladd.
pub type Stamp = (SystemTime, u64);

pub fn stamp_of(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Det som avgör om en flik får sparas automatiskt.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    pub dirty: bool,
    pub has_path: bool,
    pub needs_save_as: bool,
    pub pdf_viewer: bool,
    /// PDF som öppnats som text: att spara skriver om hela PDF:en.
    pub converted_pdf: bool,
    /// Word/ODT med innehåll som Kladd inte kan skriva tillbaka.
    pub has_dropped: bool,
}

impl Facts {
    pub fn of(doc: &Document) -> Self {
        Self {
            dirty: doc.dirty,
            has_path: doc.path.is_some(),
            needs_save_as: doc.needs_save_as(),
            pdf_viewer: doc.pdfv.is_some(),
            converted_pdf: doc.pdf,
            has_dropped: !doc.docx_dropped.is_empty(),
        }
    }
}

/// Sant om fliken får sparas automatiskt. Allt som ger dataförlust eller kräver ett aktivt val
/// (Spara som, varningsdialogen) sparas bara med ett uttryckligt Spara.
pub fn eligible(f: &Facts) -> bool {
    f.dirty && f.has_path && !f.needs_save_as && !f.pdf_viewer && !f.converted_pdf && !f.has_dropped
}

/// Hur filen på disk förhåller sig till det Kladd senast såg.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disk {
    Unchanged,
    /// Ändrad av någon annan sedan den öppnades eller sparades.
    Changed,
    /// Borttagen eller flyttad.
    Missing,
}

pub fn check_disk(recorded: Option<Stamp>, current: Option<Stamp>) -> Disk {
    match (recorded, current) {
        (_, None) if recorded.is_some() => Disk::Missing,
        (a, b) if a == b => Disk::Unchanged,
        _ => Disk::Changed,
    }
}

/// Sant när tystnaden efter senaste ändringen räckt.
pub fn due(last_change: f64, now: f64) -> bool {
    now - last_change >= DELAY_SECS
}

/// Läge för den automatiska sparningen i appen.
#[derive(Default)]
pub struct Autosave {
    stamps: HashMap<u64, Stamp>,
    /// Senast sedda textens hash och tidpunkten den ändrades, för den aktiva fliken.
    seen: HashMap<u64, (u64, f64)>,
    /// Flikar där en notis om ändring på disk redan visats (med stämpeln den gällde).
    warned: HashMap<u64, Option<Stamp>>,
    /// Flikar vars senaste automatiska sparning misslyckades, med textens hash (så att felet inte upprepas).
    failed: HashMap<u64, u64>,
    last_active: Option<u64>,
    had_focus: bool,
}

/// Något som appen ska göra efter en genomgång.
pub enum Step {
    /// Spara fliken med detta id automatiskt.
    Save(u64),
    /// Visa en notis (filen har ändrats på disk).
    Notice(String),
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl Autosave {
    /// Minns filens stämpel (vid öppning och efter varje sparning).
    pub fn record_stamp(&mut self, doc_id: u64, path: &Path) {
        match stamp_of(path) {
            Some(s) => {
                self.stamps.insert(doc_id, s);
            }
            None => {
                self.stamps.remove(&doc_id);
            }
        }
        self.warned.remove(&doc_id);
        self.seen.remove(&doc_id);
        self.failed.remove(&doc_id);
    }

    /// Kom ihåg att sparningen av den här texten misslyckades: nytt försök först när texten ändras.
    pub fn mark_failed(&mut self, doc_id: u64, text: &str) {
        self.failed.insert(doc_id, hash(text));
    }

    pub fn forget(&mut self, doc_id: u64) {
        self.failed.remove(&doc_id);
        self.stamps.remove(&doc_id);
        self.seen.remove(&doc_id);
        self.warned.remove(&doc_id);
    }

    /// Jämför mot disken. `Unchanged` betyder att det är säkert att skriva.
    pub fn disk_state(&self, doc_id: u64, path: &Path) -> Disk {
        match self.stamps.get(&doc_id) {
            // Okänd stämpel (borde inte hända): spara inte i blindo.
            None if stamp_of(path).is_some() => Disk::Changed,
            recorded => check_disk(recorded.copied(), stamp_of(path)),
        }
    }

    /// Går igenom flikarna och ger vad som ska göras. `now` är egui-tid i sekunder.
    pub fn poll(&mut self, docs: &[Document], active: usize, now: f64, focused: bool, enabled: bool) -> Vec<Step> {
        let active_id = docs.get(active).map(|d| d.id);
        let lost_focus = self.had_focus && !focused;
        let switched = self.last_active.is_some() && self.last_active != active_id;
        let prev_active = self.last_active;
        self.had_focus = focused;
        self.last_active = active_id;
        if !enabled {
            self.seen.clear();
            return Vec::new();
        }
        let mut ids: Vec<u64> = Vec::new();
        if lost_focus {
            ids.extend(docs.iter().filter(|d| eligible(&Facts::of(d))).map(|d| d.id));
        }
        if switched {
            ids.extend(prev_active.filter(|id| docs.iter().any(|d| d.id == *id && eligible(&Facts::of(d)))));
        }
        if let Some(doc) = docs.get(active) {
            if eligible(&Facts::of(doc)) {
                let h = hash(&doc.text);
                let entry = self.seen.entry(doc.id).or_insert((h, now));
                if entry.0 != h {
                    *entry = (h, now);
                }
                if due(entry.1, now) {
                    ids.push(doc.id);
                }
            } else {
                self.seen.remove(&doc.id);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids.retain(|id| {
            let text = docs.iter().find(|d| d.id == *id).map(|d| hash(&d.text));
            self.failed.get(id).is_none_or(|h| Some(*h) != text)
        });
        let mut steps = Vec::new();
        for id in ids {
            let Some(path) = docs.iter().find(|d| d.id == id).and_then(|d| d.path.as_deref()) else { continue };
            match self.disk_state(id, path) {
                Disk::Unchanged => steps.push(Step::Save(id)),
                state => {
                    let current = stamp_of(path);
                    // Visa notisen en gång per ändring, inte varannan sekund.
                    if self.warned.get(&id) != Some(&current) {
                        self.warned.insert(id, current);
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        steps.push(Step::Notice(if state == Disk::Missing {
                            tf!("{name} har tagits bort. Sparar inte automatiskt. Spara manuellt för att skriva över.", name = name)
                        } else {
                            tf!("{name} har ändrats på disk. Sparar inte automatiskt. Spara manuellt för att skriva över.", name = name)
                        }));
                    }
                }
            }
            // Ett nytt försök tidigast efter nästa ändring.
            self.seen.remove(&id);
        }
        steps
    }

    /// Sant om appen behöver väckas igen för att tiden ska kunna löpa ut.
    pub fn pending(&self) -> bool {
        !self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ok() -> Facts {
        Facts { dirty: true, has_path: true, ..Facts::default() }
    }

    #[test]
    fn plain_dirty_file_with_path_is_eligible() {
        assert!(eligible(&ok()));
    }

    #[test]
    fn clean_or_untitled_tabs_are_never_saved() {
        assert!(!eligible(&Facts { dirty: false, ..ok() }));
        assert!(!eligible(&Facts { has_path: false, ..ok() }));
    }

    #[test]
    fn lossy_or_readonly_formats_need_an_explicit_save() {
        assert!(!eligible(&Facts { needs_save_as: true, ..ok() }));
        assert!(!eligible(&Facts { pdf_viewer: true, ..ok() }));
        assert!(!eligible(&Facts { converted_pdf: true, ..ok() }));
        assert!(!eligible(&Facts { has_dropped: true, ..ok() }));
    }

    #[test]
    fn facts_follow_the_document() {
        let mut d = Document::new_untitled(1, 1);
        d.dirty = true;
        assert!(!eligible(&Facts::of(&d)), "namnlös");
        d.path = Some("/tmp/x.txt".into());
        assert!(eligible(&Facts::of(&d)));
        d.docx = true;
        d.docx_dropped = vec!["bilder"];
        assert!(!eligible(&Facts::of(&d)));
        d.docx_dropped.clear();
        d.path = Some("/tmp/x.docm".into());
        assert!(!eligible(&Facts::of(&d)), "makron kräver Spara som");
        d.pdf = true;
        d.path = Some("/tmp/x.pdf".into());
        d.docx = false;
        assert!(!eligible(&Facts::of(&d)));
    }

    fn stamp(secs: u64, len: u64) -> Stamp {
        (SystemTime::UNIX_EPOCH + Duration::from_secs(secs), len)
    }

    #[test]
    fn mtime_or_size_change_is_a_conflict() {
        assert_eq!(check_disk(Some(stamp(5, 10)), Some(stamp(5, 10))), Disk::Unchanged);
        assert_eq!(check_disk(Some(stamp(5, 10)), Some(stamp(6, 10))), Disk::Changed);
        assert_eq!(check_disk(Some(stamp(5, 10)), Some(stamp(5, 11))), Disk::Changed);
        assert_eq!(check_disk(Some(stamp(5, 10)), None), Disk::Missing);
        assert_eq!(check_disk(None, None), Disk::Unchanged);
        assert_eq!(check_disk(None, Some(stamp(5, 10))), Disk::Changed);
    }

    #[test]
    fn due_after_two_seconds_of_silence() {
        assert!(!due(10.0, 11.9));
        assert!(due(10.0, 12.0));
    }

    fn dirty_doc(id: u64, path: &Path) -> Document {
        let mut d = Document::new_untitled(id, 1);
        d.path = Some(path.to_path_buf());
        d.text = "ändrad".into();
        d.dirty = true;
        d
    }

    #[test]
    fn poll_saves_after_silence_and_resets_on_typing() {
        let dir = std::env::temp_dir().join(format!("kladd-auto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let mut a = Autosave::default();
        a.record_stamp(1, &file);
        let mut docs = vec![dirty_doc(1, &file)];
        assert!(a.poll(&docs, 0, 0.0, true, true).is_empty());
        assert!(a.pending());
        docs[0].text.push('!');
        assert!(a.poll(&docs, 0, 1.5, true, true).is_empty(), "ny ändring startar om tiden");
        assert!(a.poll(&docs, 0, 3.0, true, true).is_empty());
        let steps = a.poll(&docs, 0, 3.6, true, true);
        assert!(matches!(steps.as_slice(), [Step::Save(1)]));
        // Avstängd: inget sparas och inget väntar.
        let mut b = Autosave::default();
        b.record_stamp(1, &file);
        assert!(b.poll(&docs, 0, 0.0, true, false).is_empty());
        assert!(b.poll(&docs, 0, 10.0, true, false).is_empty());
        assert!(!b.pending());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn poll_saves_on_focus_loss_and_tab_switch() {
        let dir = std::env::temp_dir().join(format!("kladd-auto2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (f1, f2) = (dir.join("a.txt"), dir.join("b.txt"));
        std::fs::write(&f1, "x").unwrap();
        std::fs::write(&f2, "y").unwrap();
        let mut a = Autosave::default();
        a.record_stamp(1, &f1);
        a.record_stamp(2, &f2);
        let docs = vec![dirty_doc(1, &f1), dirty_doc(2, &f2)];
        assert!(a.poll(&docs, 0, 0.0, true, true).is_empty());
        // Flikbyte: den förra sparas direkt.
        let steps = a.poll(&docs, 1, 0.1, true, true);
        assert!(matches!(steps.as_slice(), [Step::Save(1)]));
        // Fokus förlorat: alla ändrade flikar sparas.
        let steps = a.poll(&docs, 1, 0.2, false, true);
        assert!(matches!(steps.as_slice(), [Step::Save(1), Step::Save(2)]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn poll_refuses_changed_file_and_warns_once() {
        let dir = std::env::temp_dir().join(format!("kladd-auto3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let mut a = Autosave::default();
        a.record_stamp(1, &file);
        std::fs::write(&file, "ändrad av någon annan").unwrap();
        let docs = vec![dirty_doc(1, &file)];
        let _ = a.poll(&docs, 0, 0.0, true, true);
        let steps = a.poll(&docs, 0, 2.5, true, true);
        assert!(matches!(steps.as_slice(), [Step::Notice(_)]));
        let _ = a.poll(&docs, 0, 3.0, true, true);
        assert!(a.poll(&docs, 0, 6.0, true, true).is_empty(), "samma ändring varnar inte igen");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
