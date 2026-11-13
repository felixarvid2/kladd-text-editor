//! Versionshistorik: varje sparning av en fil lagrar en kopia av de sparade byten i appens datamapp
//! (`<data>/historik/<hash av sökvägen>/<tidsstämpel i ms>`, plus `index.json` med originalsökvägen).
//! Lika versioner sparas inte två gånger och gamla gallras enligt en fast policy.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;

/// Gallringsregler för en fils historik.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Allt som är yngre än så här behålls.
    pub keep_all_ms: u64,
    /// Därefter en per timme tills detta.
    pub keep_hourly_ms: u64,
    /// Därefter en per dygn tills detta; äldre tas bort.
    pub keep_daily_ms: u64,
    pub max_versions: usize,
    /// Största sammanlagda storlek för en fils versioner.
    pub max_bytes: u64,
    /// Största sammanlagda storlek för hela historikmappen.
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            keep_all_ms: HOUR_MS,
            keep_hourly_ms: DAY_MS,
            keep_daily_ms: 30 * DAY_MS,
            max_versions: 50,
            max_bytes: 64 << 20,
            max_total_bytes: 512 << 20,
        }
    }
}

/// En sparad version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    /// Tid för sparningen, millisekunder sedan epoken (också filens namn).
    pub ts_ms: u64,
    pub size: u64,
}

#[derive(Serialize, Deserialize)]
struct Index {
    path: String,
}

/// Mappen där all historik ligger.
pub fn root() -> PathBuf {
    crate::platform::data_dir().join("historik")
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// FNV-1a: stabil mellan Rust-versioner, till skillnad från `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Mappen för en fils versioner, namngiven efter hashen av den kanoniska sökvägen.
pub fn dir_for(root: &Path, path: &Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    root.join(format!("{:016x}", fnv1a(canonical.to_string_lossy().as_bytes())))
}

/// Binära format (visas inte som text i diffen).
pub fn is_binary_path(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "docx" | "docm" | "dotx" | "dotm" | "odt" | "ott" | "pdf")
}

fn version_file(dir: &Path, ts_ms: u64) -> PathBuf {
    dir.join(ts_ms.to_string())
}

/// Versionerna för en katalog, nyast först. Filer som inte är tidsstämplar ignoreras.
fn list_dir(dir: &Path) -> Vec<Version> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Version> = rd
        .flatten()
        .filter_map(|e| {
            let ts_ms = e.file_name().to_string_lossy().parse::<u64>().ok()?;
            let size = e.metadata().ok()?.len();
            Some(Version { ts_ms, size })
        })
        .collect();
    v.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms));
    v
}

/// Alla versioner av en fil, nyast först.
pub fn list(root: &Path, path: &Path) -> Vec<Version> {
    list_dir(&dir_for(root, path))
}

/// Läser de sparade byten för en version.
pub fn read(root: &Path, path: &Path, ts_ms: u64) -> std::io::Result<Vec<u8>> {
    std::fs::read(version_file(&dir_for(root, path), ts_ms))
}

/// Originalsökvägen som står i indexet för en historikmapp.
#[cfg_attr(not(test), allow(dead_code))]
pub fn original_path(dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dir.join("index.json")).ok()?;
    let idx: Index = serde_json::from_str(&text).ok()?;
    Some(PathBuf::from(idx.path))
}

/// Sparar en version av `path` med de sparade byten. Ger `Ok(true)` om en ny version lades till,
/// `Ok(false)` om innehållet är identiskt med den senaste.
pub fn record(root: &Path, path: &Path, bytes: &[u8], now_ms: u64, limits: &Limits) -> std::io::Result<bool> {
    let dir = dir_for(root, path);
    std::fs::create_dir_all(&dir)?;
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let index = dir.join("index.json");
    let want = serde_json::to_string(&Index { path: canonical.to_string_lossy().into_owned() }).unwrap_or_default();
    if std::fs::read_to_string(&index).ok().as_deref() != Some(want.as_str()) {
        std::fs::write(&index, want)?;
    }
    let versions = list_dir(&dir);
    if let Some(last) = versions.first() {
        if last.size == bytes.len() as u64 && std::fs::read(version_file(&dir, last.ts_ms)).ok().as_deref() == Some(bytes) {
            return Ok(false);
        }
    }
    // Tidsstämpeln är namnet: den får aldrig krocka med eller hamna före en äldre version.
    let newest = versions.first().map_or(0, |v| v.ts_ms);
    let ts = now_ms.max(newest + 1);
    crate::doc::write_atomic(&version_file(&dir, ts), bytes)?;
    prune_dir(&dir, now_ms, limits);
    prune_total(root, limits.max_total_bytes);
    Ok(true)
}

/// Vilka versioner (tidsstämplar) som ska tas bort. `versions` ska vara nyast först.
/// Den nyaste behålls alltid.
pub fn prune_plan(versions: &[Version], now_ms: u64, limits: &Limits) -> Vec<u64> {
    let mut remove = Vec::new();
    let mut kept: Vec<&Version> = Vec::new();
    let mut last_bucket: Option<(u8, u64)> = None;
    for (i, v) in versions.iter().enumerate() {
        let age = now_ms.saturating_sub(v.ts_ms);
        let keep = if i == 0 || age <= limits.keep_all_ms {
            true
        } else {
            let bucket = if age <= limits.keep_hourly_ms {
                Some((1, v.ts_ms / HOUR_MS))
            } else if age <= limits.keep_daily_ms {
                Some((2, v.ts_ms / DAY_MS))
            } else {
                None
            };
            match bucket {
                // Nyast först: den första i varje timme/dygn är den som behålls.
                Some(b) if last_bucket != Some(b) => {
                    last_bucket = Some(b);
                    true
                }
                _ => false,
            }
        };
        if keep {
            kept.push(v);
        } else {
            remove.push(v.ts_ms);
        }
    }
    // Antal och storlek: ta bort de äldsta, men aldrig den nyaste.
    while kept.len() > limits.max_versions.max(1) {
        remove.push(kept.pop().map(|v| v.ts_ms).unwrap_or(0));
    }
    let mut total: u64 = kept.iter().map(|v| v.size).sum();
    while kept.len() > 1 && total > limits.max_bytes {
        let v = kept.pop().unwrap();
        total -= v.size;
        remove.push(v.ts_ms);
    }
    remove
}

fn prune_dir(dir: &Path, now_ms: u64, limits: &Limits) {
    let versions = list_dir(dir);
    for ts in prune_plan(&versions, now_ms, limits) {
        let _ = std::fs::remove_file(version_file(dir, ts));
    }
}

/// Håller hela historikmappen under taket genom att ta bort de äldsta versionerna, men aldrig den
/// nyaste versionen av någon fil.
pub fn prune_total(root: &Path, max_total_bytes: u64) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    // (tid, storlek, fil, är nyast i sin mapp)
    let mut all: Vec<(u64, u64, PathBuf, bool)> = Vec::new();
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        for (i, v) in list_dir(&dir).into_iter().enumerate() {
            all.push((v.ts_ms, v.size, version_file(&dir, v.ts_ms), i == 0));
        }
    }
    let mut total: u64 = all.iter().map(|a| a.1).sum();
    if total <= max_total_bytes {
        return;
    }
    all.sort_by_key(|a| a.0);
    for (_, size, file, newest) in all {
        if total <= max_total_bytes {
            break;
        }
        if newest {
            continue;
        }
        if std::fs::remove_file(&file).is_ok() {
            total -= size;
        }
    }
}

// ------------------------------------------------------------------------------------------ diff

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Same,
    /// Finns bara i den aktuella texten.
    Added,
    /// Finns bara i versionen.
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
}

/// Radvis diff från versionen (`old`) till aktuell text (`new`).
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    TextDiff::from_lines(old, new)
        .iter_all_changes()
        .map(|c| DiffLine {
            kind: match c.tag() {
                ChangeTag::Equal => LineKind::Same,
                ChangeTag::Insert => LineKind::Added,
                ChangeTag::Delete => LineKind::Removed,
            },
            text: c.value().trim_end_matches(['\n', '\r']).to_owned(),
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffRow {
    Line(DiffLine),
    /// Så många oförändrade rader är dolda.
    Gap(usize),
}

/// Döljer långa oförändrade stycken och behåller `context` rader runt varje ändring.
pub fn collapse(lines: Vec<DiffLine>, context: usize) -> Vec<DiffRow> {
    let n = lines.len();
    let mut show = vec![false; n];
    for (i, l) in lines.iter().enumerate() {
        if l.kind != LineKind::Same {
            let from = i.saturating_sub(context);
            let to = (i + context + 1).min(n);
            show[from..to].iter_mut().for_each(|s| *s = true);
        }
    }
    let mut rows = Vec::new();
    let mut hidden = 0;
    for (l, s) in lines.into_iter().zip(show) {
        if s {
            if hidden > 0 {
                rows.push(DiffRow::Gap(hidden));
                hidden = 0;
            }
            rows.push(DiffRow::Line(l));
        } else {
            hidden += 1;
        }
    }
    if hidden > 0 {
        rows.push(DiffRow::Gap(hidden));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kladd-hist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn v(ts: u64, size: u64) -> Version {
        Version { ts_ms: ts, size }
    }

    const NOW: u64 = 100 * DAY_MS;

    #[test]
    fn prune_keeps_everything_from_the_last_hour() {
        let vs: Vec<Version> = (0..10).map(|i| v(NOW - i * 5 * 60_000, 1)).collect();
        // 0..=45 minuter gammalt: allt behålls (bara de som är äldre än en timme gallras).
        assert!(prune_plan(&vs, NOW, &Limits::default()).is_empty());
    }

    #[test]
    fn prune_thins_to_one_per_hour_for_a_day() {
        // Fyra versioner i samma timme, två timmar sedan.
        let base = (NOW - 2 * HOUR_MS) / HOUR_MS * HOUR_MS;
        let vs = vec![v(NOW, 1), v(base + 50 * 60_000, 1), v(base + 40 * 60_000, 1), v(base + 10 * 60_000, 1)];
        let mut gone = prune_plan(&vs, NOW, &Limits::default());
        gone.sort();
        assert_eq!(gone, vec![base + 10 * 60_000, base + 40 * 60_000]);
    }

    #[test]
    fn prune_thins_to_one_per_day_for_a_month_and_drops_older() {
        let day = (NOW - 5 * DAY_MS) / DAY_MS * DAY_MS;
        let vs = vec![
            v(NOW, 1),
            v(day + 20 * HOUR_MS, 1),
            v(day + 3 * HOUR_MS, 1),
            v(NOW - 31 * DAY_MS, 1),
        ];
        let mut gone = prune_plan(&vs, NOW, &Limits::default());
        gone.sort();
        assert_eq!(gone, vec![NOW - 31 * DAY_MS, day + 3 * HOUR_MS]);
    }

    #[test]
    fn prune_caps_count_but_never_drops_newest() {
        let limits = Limits { max_versions: 3, ..Limits::default() };
        let vs: Vec<Version> = (0..6).map(|i| v(NOW - i * 1000, 1)).collect();
        let mut gone = prune_plan(&vs, NOW, &limits);
        gone.sort();
        assert_eq!(gone, vec![NOW - 5000, NOW - 4000, NOW - 3000]);
    }

    #[test]
    fn prune_caps_size_and_keeps_newest_even_if_huge() {
        let limits = Limits { max_bytes: 100, ..Limits::default() };
        let vs = vec![v(NOW, 500), v(NOW - 1000, 10), v(NOW - 2000, 10)];
        let mut gone = prune_plan(&vs, NOW, &limits);
        gone.sort();
        assert_eq!(gone, vec![NOW - 2000, NOW - 1000]);
    }

    #[test]
    fn record_stores_dedups_and_indexes() {
        let root = tmp("record");
        let file = root.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let l = Limits::default();
        assert!(record(&root, &file, b"ett", 1000, &l).unwrap());
        assert!(!record(&root, &file, b"ett", 2000, &l).unwrap(), "identisk med senaste hoppas över");
        assert!(record(&root, &file, b"tva", 3000, &l).unwrap());
        // Samma innehåll som en äldre (men inte den senaste) version lagras igen.
        assert!(record(&root, &file, b"ett", 4000, &l).unwrap());
        let list = list(&root, &file);
        assert_eq!(list.iter().map(|v| v.ts_ms).collect::<Vec<_>>(), vec![4000, 3000, 1000]);
        assert_eq!(read(&root, &file, 3000).unwrap(), b"tva");
        let dir = dir_for(&root, &file);
        assert_eq!(original_path(&dir).unwrap(), file.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn record_never_collides_with_a_newer_timestamp() {
        let root = tmp("collide");
        let file = root.join("a.txt");
        let l = Limits::default();
        record(&root, &file, b"a", 5000, &l).unwrap();
        record(&root, &file, b"b", 5000, &l).unwrap();
        record(&root, &file, b"c", 100, &l).unwrap();
        let ts: Vec<u64> = list(&root, &file).iter().map(|v| v.ts_ms).collect();
        assert_eq!(ts, vec![5002, 5001, 5000]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn different_paths_get_different_dirs() {
        let root = Path::new("/r");
        assert_ne!(dir_for(root, Path::new("/x/a.txt")), dir_for(root, Path::new("/x/b.txt")));
        assert_eq!(dir_for(root, Path::new("/x/a.txt")), dir_for(root, Path::new("/x/a.txt")));
    }

    #[test]
    fn total_cap_removes_oldest_but_keeps_each_files_newest() {
        let root = tmp("total");
        let (a, b) = (root.join("a.txt"), root.join("b.txt"));
        let l = Limits::default();
        record(&root, &a, &[0; 100], 1000, &l).unwrap();
        record(&root, &a, &[1; 100], 3000, &l).unwrap();
        record(&root, &b, &[2; 100], 2000, &l).unwrap();
        prune_total(&root, 250);
        assert_eq!(list(&root, &a).iter().map(|v| v.ts_ms).collect::<Vec<_>>(), vec![3000]);
        assert_eq!(list(&root, &b).len(), 1);
        prune_total(&root, 0);
        assert_eq!(list(&root, &a).len(), 1);
        assert_eq!(list(&root, &b).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diff_marks_added_and_removed_lines() {
        let d = diff_lines("a\nb\nc\n", "a\nc\nd\n");
        let kinds: Vec<(LineKind, &str)> = d.iter().map(|l| (l.kind, l.text.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (LineKind::Same, "a"),
                (LineKind::Removed, "b"),
                (LineKind::Same, "c"),
                (LineKind::Added, "d"),
            ]
        );
        assert!(diff_lines("x\ny", "x\ny").iter().all(|l| l.kind == LineKind::Same));
    }

    #[test]
    fn collapse_hides_long_unchanged_runs() {
        let old: String = (0..20).map(|i| format!("{i}\n")).collect();
        let new = old.replace("10\n", "tio\n");
        let rows = collapse(diff_lines(&old, &new), 2);
        assert_eq!(rows.first(), Some(&DiffRow::Gap(8)));
        assert_eq!(rows.last(), Some(&DiffRow::Gap(7)));
        // 2 före + borttagen + tillagd + 2 efter, plus två luckor.
        assert_eq!(rows.len(), 2 + 6);
        assert_eq!(collapse(diff_lines("a\n", "a\n"), 2), vec![DiffRow::Gap(1)]);
    }

    #[test]
    fn binary_formats_are_recognised() {
        assert!(is_binary_path(Path::new("a.DOCX")));
        assert!(is_binary_path(Path::new("a.odt")));
        assert!(is_binary_path(Path::new("a.pdf")));
        assert!(!is_binary_path(Path::new("a.md")));
        assert!(!is_binary_path(Path::new("Makefile")));
    }
}
