//! Tauri backend: serves the generated galaxy data to the web view, answers
//! system lookups from `galaxy.sqlite`, and tails the player journal.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use galaxy_core::id64::Id64;
use galaxy_core::journal::{CommanderSource, CommanderTrack, Jump, JournalSource};
use galaxy_core::visited::{self, VisitedEntry};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use tauri::http::{header, Response, StatusCode};
use tauri::{Emitter, Manager, State};

/// The custom scheme the web view fetches tiles and density maps from:
/// `http://galaxy.localhost/<path>` on Windows, `galaxy://localhost/<path>` elsewhere.
const SCHEME: &str = "galaxy";

struct AppState {
    data_dir: Option<PathBuf>,
    db: Option<Mutex<Connection>>,
    commander: Option<Mutex<JournalSource>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DatasetInfo {
    data_dir: String,
    systems: Option<String>,
    source: Option<String>,
    source_last_modified: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    rowid: u32,
    #[serde(serialize_with = "galaxy_core::serialize_id64")]
    id64: Option<u64>,
    name: String,
    pos: [f64; 3],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommanderPayload {
    journal_dir: String,
    commanders: Vec<CommanderTrack>,
    latest_fid: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JumpEvent {
    fid: String,
    jump: Jump,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn dataset_info(state: State<AppState>) -> Option<DatasetInfo> {
    let dir = state.data_dir.as_ref()?;
    let meta = |key: &str| -> Option<String> {
        let db = state.db.as_ref()?.lock().ok()?;
        db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional().ok()?
    };
    Some(DatasetInfo {
        data_dir: dir.display().to_string(),
        systems: meta("systems"),
        source: meta("source"),
        source_last_modified: meta("source_last_modified"),
    })
}

fn row_to_system(r: &rusqlite::Row) -> rusqlite::Result<SystemInfo> {
    let id64: Option<i64> = r.get(1)?;
    let coord = |i| -> rusqlite::Result<f64> { Ok(r.get::<_, i64>(i)? as f64 / 32.0) };
    Ok(SystemInfo {
        rowid: r.get(0)?,
        id64: id64.map(|v| v as u64),
        name: r.get(2)?,
        pos: [coord(3)?, coord(4)?, coord(5)?],
    })
}

const SYSTEM_COLUMNS: &str = "id, id64, name, x32, y32, z32";

/// Case-insensitive prefix search; an exact match is listed first.
#[tauri::command]
fn search_systems(state: State<AppState>, query: String) -> CmdResult<Vec<SystemInfo>> {
    let Some(db) = &state.db else { return Ok(Vec::new()) };
    // `%` and `_` are LIKE wildcards; system names don't use them.
    let q: String = query.trim().chars().filter(|c| *c != '%' && *c != '_').collect();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let db = db.lock().map_err(err)?;
    let mut exact = db
        .prepare_cached(&format!("SELECT {SYSTEM_COLUMNS} FROM systems WHERE name = ?1 LIMIT 5"))
        .map_err(err)?
        .query_map([&q], row_to_system)
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    let prefix = db
        .prepare_cached(&format!("SELECT {SYSTEM_COLUMNS} FROM systems WHERE name LIKE ?1 ORDER BY name LIMIT 20"))
        .map_err(err)?
        .query_map([format!("{q}%")], row_to_system)
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    for s in prefix {
        if exact.len() >= 20 {
            break;
        }
        if !exact.iter().any(|e| e.rowid == s.rowid) {
            exact.push(s);
        }
    }
    Ok(exact)
}

#[tauri::command]
fn get_system(state: State<AppState>, rowid: u32) -> CmdResult<Option<SystemInfo>> {
    let Some(db) = &state.db else { return Ok(None) };
    let db = db.lock().map_err(err)?;
    let mut stmt = db
        .prepare_cached(&format!("SELECT {SYSTEM_COLUMNS} FROM systems WHERE id = ?1"))
        .map_err(err)?;
    stmt.query_row([rowid], row_to_system).optional().map_err(err)
}

#[tauri::command]
fn commander_history(state: State<AppState>) -> CmdResult<Option<CommanderPayload>> {
    let Some(source) = &state.commander else { return Ok(None) };
    let mut source = source.lock().map_err(err)?;
    let journal_dir = source.dir().display().to_string();
    let history = source.history().map_err(err)?;
    Ok(Some(CommanderPayload {
        journal_dir,
        latest_fid: history.latest().map(|c| c.fid.clone()),
        commanders: history.commanders.values().cloned().collect(),
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VisitedSystem {
    #[serde(flatten)]
    entry: VisitedEntry,
    name: Option<String>,
    pos: [f64; 3],
    /// True when the system isn't in the catalogue and `pos` is only its boxel centre.
    approximate: bool,
}

/// Systems from the game's visited-stars cache, placed using the catalogue
/// where possible and the id64 boxel centre otherwise.
#[tauri::command]
fn visited_systems(state: State<AppState>) -> CmdResult<Vec<VisitedSystem>> {
    let mut entries: Vec<VisitedEntry> = Vec::new();
    for path in visited::find_cache_files() {
        match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|d| visited::parse(&d)) {
            Ok(cache) => entries.extend(cache.entries),
            Err(e) => eprintln!("{}: {e}", path.display()),
        }
    }
    entries.sort_by_key(|e| e.id64);
    entries.dedup_by_key(|e| e.id64);

    let db = state.db.as_ref().map(|d| d.lock()).transpose().map_err(err)?;
    let mut lookup = match &db {
        Some(db) => Some(db.prepare_cached("SELECT name, x32, y32, z32 FROM systems WHERE id64 = ?1").map_err(err)?),
        None => None,
    };
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let found = match &mut lookup {
            Some(stmt) => stmt
                .query_row([entry.id64 as i64], |r| {
                    let c = |i| -> rusqlite::Result<f64> { Ok(r.get::<_, i64>(i)? as f64 / 32.0) };
                    Ok((r.get::<_, String>(0)?, [c(1)?, c(2)?, c(3)?]))
                })
                .optional()
                .map_err(err)?,
            None => None,
        };
        out.push(match found {
            Some((name, pos)) => VisitedSystem { entry, name: Some(name), pos, approximate: false },
            None => {
                let c = Id64::decode(entry.id64).boxel_centre();
                VisitedSystem { entry, name: None, pos: c.map(f64::from), approximate: true }
            }
        });
    }
    Ok(out)
}

/// `STELLAR_DATA_DIR`, else the nearest `data/` folder holding tiles, searching
/// upwards from the working directory and then from the executable.
fn find_data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("STELLAR_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
    starts.into_iter().flatten().find_map(|start| {
        start
            .ancestors()
            .map(|a| a.join("data"))
            .find(|d| d.join("tiles").join("index.json").is_file())
    })
}

/// Maps a request path onto a file inside the data dir, refusing anything
/// that would climb out of it.
fn resolve_data_path(data_dir: &Path, request_path: &str) -> Option<PathBuf> {
    let rel = Path::new(request_path.trim_start_matches('/'));
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(data_dir.join(rel))
}

fn serve(data_dir: Option<&Path>, path: &str) -> Response<Vec<u8>> {
    let respond = |status: StatusCode, mime: &str, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(body)
            .unwrap()
    };
    let Some(file) = data_dir.and_then(|d| resolve_data_path(d, path)) else {
        return respond(StatusCode::NOT_FOUND, "text/plain", b"not found".to_vec());
    };
    let mime = match file.extension().and_then(|e| e.to_str()) {
        Some("json") => "application/json",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    match std::fs::read(&file) {
        Ok(bytes) => respond(StatusCode::OK, mime, bytes),
        Err(_) => respond(StatusCode::NOT_FOUND, "text/plain", b"not found".to_vec()),
    }
}

/// Polls the journal once a second and forwards new jumps to the web view.
/// Polling rather than file-change notifications: the game keeps the journal
/// open and Windows doesn't reliably report its writes.
fn spawn_journal_watcher(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let state = app.state::<AppState>();
        let Some(source) = &state.commander else { return };
        let new = match source.lock() {
            Ok(mut s) => s.poll(),
            Err(_) => return,
        };
        match new {
            Ok(jumps) => {
                for (fid, jump) in jumps {
                    let _ = app.emit("commander-jump", JumpEvent { fid, jump });
                }
            }
            Err(e) => eprintln!("journal poll failed: {e}"),
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let data_dir = find_data_dir();
    let db = data_dir.as_ref().and_then(|d| {
        Connection::open_with_flags(
            d.join("galaxy.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| eprintln!("galaxy.sqlite unavailable: {e}"))
        .ok()
        .map(Mutex::new)
    });
    let commander = galaxy_core::journal::default_journal_dir()
        .filter(|d| d.is_dir())
        .map(|d| Mutex::new(JournalSource::new(d)));
    eprintln!(
        "data dir: {}",
        data_dir.as_ref().map_or("not found".into(), |d| d.display().to_string())
    );

    let protocol_dir = data_dir.clone();
    tauri::Builder::default()
        .manage(AppState { data_dir, db, commander })
        .register_asynchronous_uri_scheme_protocol(SCHEME, move |_ctx, request, responder| {
            let dir = protocol_dir.clone();
            let path = request.uri().path().to_string();
            std::thread::spawn(move || responder.respond(serve(dir.as_deref(), &path)));
        })
        .invoke_handler(tauri::generate_handler![dataset_info, search_systems, get_system, commander_history, visited_systems])
        .setup(|app| {
            spawn_journal_watcher(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running StellarCartography");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_paths_cannot_escape_the_data_dir() {
        let root = Path::new("/data");
        assert_eq!(resolve_data_path(root, "/tiles/r0.bin"), Some(root.join("tiles/r0.bin")));
        assert_eq!(resolve_data_path(root, "/../secret"), None);
        assert_eq!(resolve_data_path(root, "/tiles/../../x"), None);
        assert_eq!(resolve_data_path(root, "/C:/Windows/win.ini"), None);
    }
}
