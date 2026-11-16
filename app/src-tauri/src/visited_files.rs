//! The `VisitedStarsCache.dat` files the map reads: the game's own (see
//! [`visited::find_cache_files`]) and one copy the user imported from a
//! backup or another PC, kept in the app's local data folder. The copy
//! stands in for a cache the game no longer has here, so the map can mark
//! the systems an account visited when its journals are gone too.

use std::path::{Path, PathBuf};

use galaxy_core::visited::{self, VisitedCache};
use serde::Serialize;

/// Name of the imported copy in the app's local data folder.
pub const IMPORT_NAME: &str = "imported-VisitedStarsCache.dat";

/// One cache file, as Settings lists it.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CacheFile {
    pub path: String,
    /// The imported copy, rather than one of the game's own.
    pub imported: bool,
    /// Systems it lists; `None` when it can't be read.
    pub systems: Option<usize>,
    /// When the file last changed, in Unix seconds (for the copy: when it was imported).
    pub modified: Option<u64>,
    pub error: Option<String>,
}

/// The game's own files, then the imported copy if there is one.
pub fn sources(import: &Path) -> Vec<(PathBuf, bool)> {
    let mut out: Vec<_> = visited::find_cache_files().into_iter().map(|p| (p, false)).collect();
    if import.is_file() {
        out.push((import.to_path_buf(), true));
    }
    out
}

pub fn read(path: &Path) -> anyhow::Result<VisitedCache> {
    visited::parse(&std::fs::read(path)?)
}

pub fn status(sources: &[(PathBuf, bool)]) -> Vec<CacheFile> {
    sources
        .iter()
        .map(|(path, imported)| {
            let (systems, error) = match read(path) {
                Ok(cache) => (Some(cache.entries.len()), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let modified = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            CacheFile { path: path.display().to_string(), imported: *imported, systems, modified, error }
        })
        .collect()
}

/// Copies the cache at `from` to `import`, replacing any earlier copy, once
/// it has been read as a cache. `game` lists the game's own files, which the
/// map reads anyway.
pub fn import(from: &Path, import: &Path, game: &[PathBuf]) -> Result<(), String> {
    let same = |a: &Path, b: &Path| matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b);
    if game.iter().any(|g| same(g, from)) {
        return Err("That's the game's own cache: the map reads it already.".into());
    }
    let data = std::fs::read(from).map_err(|e| format!("Couldn't read {}: {e}", from.display()))?;
    visited::parse(&data).map_err(|e| format!("{} isn't a visited-stars cache ({e}).", from.display()))?;
    let write = || -> std::io::Result<()> {
        if let Some(dir) = import.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Via a temporary file, so a failed copy leaves the old one whole.
        let tmp = import.with_extension("dat.tmp");
        std::fs::write(&tmp, &data)?;
        std::fs::rename(&tmp, import)
    };
    write().map_err(|e| format!("Couldn't save the copy: {e}"))
}

/// Deletes the imported copy, if there is one.
pub fn remove(import: &Path) -> Result<(), String> {
    match std::fs::remove_file(import) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("Couldn't remove the copy: {e}")),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(records: &[(u64, u32, u32)]) -> Vec<u8> {
        let mut d = b"VisitedStars\0\0\0\0".to_vec();
        for v in [200u32, 48, records.len() as u32, 16] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&[0; 16]);
        for &(id64, visits, day) in records {
            d.extend_from_slice(&id64.to_le_bytes());
            d.extend_from_slice(&visits.to_le_bytes());
            d.extend_from_slice(&day.to_le_bytes());
        }
        d.extend_from_slice(&[0xde, 0xc0, 0xfe, 0x5a, 0xde, 0xc0, 0xfe, 0x5a]);
        d
    }

    #[test]
    fn imports_replaces_and_removes_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join("backup.dat");
        let copy = dir.path().join("app").join(IMPORT_NAME);
        std::fs::write(&backup, cache(&[(10477373803, 3, 155_525), (3238296097059, 1, 155_210)])).unwrap();

        import(&backup, &copy, &[]).unwrap();
        let listed = status(&[(copy.clone(), true)]);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].systems, Some(2));
        assert!(listed[0].imported && listed[0].error.is_none() && listed[0].modified.is_some());

        // A second import replaces the first.
        std::fs::write(&backup, cache(&[(10477373803, 4, 155_600)])).unwrap();
        import(&backup, &copy, &[]).unwrap();
        assert_eq!(read(&copy).unwrap().entries.len(), 1);
        assert!(!copy.with_extension("dat.tmp").exists());

        remove(&copy).unwrap();
        assert!(!copy.exists());
        remove(&copy).unwrap();
    }

    #[test]
    fn refuses_other_files_and_the_games_own() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join(IMPORT_NAME);
        let journal = dir.path().join("Journal.2026-11-16T120000.01.log");
        std::fs::write(&journal, br#"{"event":"Fileheader"}"#).unwrap();
        let err = import(&journal, &copy, &[]).unwrap_err();
        assert!(err.contains("isn't a visited-stars cache"), "{err}");
        assert!(import(&dir.path().join("missing.dat"), &copy, &[]).is_err());

        let game = dir.path().join("VisitedStarsCache.dat");
        std::fs::write(&game, cache(&[(1, 1, 1)])).unwrap();
        let err = import(&game, &copy, std::slice::from_ref(&game)).unwrap_err();
        assert!(err.contains("game's own"), "{err}");
        assert!(!copy.exists(), "nothing written for a refused file");

        std::fs::write(&copy, b"garbage").unwrap();
        let listed = status(&[(copy, true)]);
        assert_eq!(listed[0].systems, None);
        assert!(listed[0].error.is_some());
    }
}
