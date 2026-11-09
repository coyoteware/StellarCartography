//! The galaxy data folder: where it is, whether it holds a usable build, and
//! the read-only SQLite connection used for lookups.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;

use crate::settings::Settings;

pub struct Dataset {
    pub dir: PathBuf,
    pub db: Mutex<Connection>,
}

/// What the web view needs to decide between the map, first-run setup and
/// an error screen.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DataStatus {
    Ready {
        dir: String,
        systems: Option<String>,
        source: Option<String>,
        source_last_modified: Option<String>,
        /// When the catalogue was built (Unix seconds), for the weekly reminder.
        built_at: Option<u64>,
    },
    /// No data there yet: the first-run case.
    Missing { dir: String },
    /// Something is there but it can't be used.
    Broken { dir: String, error: String },
}

/// Where the data folder came from, so the settings panel can explain it.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DirOrigin {
    /// `STELLAR_DATA_DIR`; can't be changed from the app.
    Environment,
    Settings,
    /// A `data/` folder above the working directory or executable (development).
    Found,
    /// Nothing configured or found: the app's local data folder.
    Default,
}

pub fn resolve_data_dir(settings: &Settings, default_dir: &Path) -> (PathBuf, DirOrigin) {
    if let Some(dir) = std::env::var_os("STELLAR_DATA_DIR") {
        return (PathBuf::from(dir), DirOrigin::Environment);
    }
    if let Some(dir) = &settings.data_dir {
        return (dir.clone(), DirOrigin::Settings);
    }
    if let Some(dir) = find_data_dir() {
        return (dir, DirOrigin::Found);
    }
    (default_dir.to_path_buf(), DirOrigin::Default)
}

/// The nearest `data/` folder holding a build, searching upwards from the
/// working directory and then from the executable.
fn find_data_dir() -> Option<PathBuf> {
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
    starts
        .into_iter()
        .flatten()
        .find_map(|start| start.ancestors().map(|a| a.join("data")).find(|d| has_data(d)))
}

fn has_data(dir: &Path) -> bool {
    dir.join("tiles").join("index.json").is_file() && dir.join("galaxy.sqlite").is_file()
}

/// The folder holding a build: `picked` itself, or a `data` folder inside it
/// (so picking the repository root works too).
pub fn locate_existing(picked: &Path) -> Option<PathBuf> {
    [picked.to_path_buf(), picked.join("data")].into_iter().find(|d| has_data(d))
}

/// Opens the data in `dir`: `Ok(None)` if there's nothing there yet.
pub fn open(dir: &Path) -> Result<Option<Dataset>, String> {
    let (index, sqlite) = (dir.join("tiles").join("index.json"), dir.join("galaxy.sqlite"));
    match (index.is_file(), sqlite.is_file()) {
        (false, false) => return Ok(None),
        (true, false) => return Err("galaxy.sqlite is missing".into()),
        (false, true) => return Err("tiles/index.json is missing".into()),
        (true, true) => {}
    }
    let db = Connection::open_with_flags(&sqlite, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("can't open galaxy.sqlite: {e}"))?;
    // A truncated or foreign file opens fine and only fails on first use.
    db.query_row("SELECT count(*) FROM meta", [], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("galaxy.sqlite isn't a StellarCartography database: {e}"))?;
    Ok(Some(Dataset { dir: dir.to_path_buf(), db: Mutex::new(db) }))
}

impl Dataset {
    pub fn status(&self) -> DataStatus {
        let meta = |key: &str| -> Option<String> {
            let db = self.db.lock().ok()?;
            db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional().ok()?
        };
        DataStatus::Ready {
            dir: self.dir.display().to_string(),
            systems: meta("systems"),
            source: meta("source"),
            source_last_modified: meta("source_last_modified"),
            // Builds from before `built_at` was recorded: the database's age.
            built_at: meta("built_at").and_then(|v| v.parse().ok()).or_else(|| modified_secs(&self.dir.join("galaxy.sqlite"))),
        }
    }
}

/// A file's modification time in Unix seconds.
pub fn modified_secs(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_reports_missing_partial_and_foreign_data() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert!(matches!(open(d), Ok(None)));

        std::fs::create_dir_all(d.join("tiles")).unwrap();
        std::fs::write(d.join("tiles").join("index.json"), "{}").unwrap();
        assert!(open(d).err().unwrap().contains("galaxy.sqlite"));

        std::fs::write(d.join("galaxy.sqlite"), b"definitely not sqlite").unwrap();
        assert!(open(d).is_err());

        std::fs::remove_file(d.join("galaxy.sqlite")).unwrap();
        let db = Connection::open(d.join("galaxy.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT); INSERT INTO meta VALUES ('systems', '3');")
            .unwrap();
        drop(db);
        let ds = open(d).unwrap().unwrap();
        assert!(matches!(ds.status(), DataStatus::Ready { systems: Some(s), .. } if s == "3"));
        // No built_at in the meta table: the file's age stands in.
        let DataStatus::Ready { built_at: Some(t), .. } = ds.status() else { panic!("no built_at") };
        assert_eq!(Some(t), modified_secs(&d.join("galaxy.sqlite")));
        assert_eq!(locate_existing(d), Some(d.to_path_buf()));
        assert_eq!(locate_existing(dir.path().parent().unwrap()), None);
    }
}
