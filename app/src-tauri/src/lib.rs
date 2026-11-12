//! Tauri backend: serves the generated galaxy data to the web view, answers
//! system lookups from `galaxy.sqlite`, tails the player journal, keeps the
//! user's settings, runs the catalogue build (first run and rebuilds), the
//! populated-systems download behind the overlay layers and the
//! points-of-interest download, and assembles per-system details from the
//! journal and online sources.

mod data;
mod details;
mod routes;
mod settings;

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use data::{DataStatus, Dataset, DirOrigin};
use details::{Online, OnlineSource};
use galaxy_core::details::SystemDetails;
use galaxy_core::id64::Id64;
use galaxy_core::journal::{Carrier, CommanderSource, CommanderTrack, Jump, JournalSource};
use galaxy_core::visited::{self, VisitedEntry};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use settings::{Bookmark, SavedSystem, Settings, ViewSettings};
use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

/// The custom scheme the web view fetches tiles and density maps from:
/// `http://galaxy.localhost/<path>` on Windows, `galaxy://localhost/<path>` elsewhere.
const SCHEME: &str = "galaxy";

struct AppState {
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    data: RwLock<DataSlot>,
    commander: Mutex<Option<JournalSource>>,
    build: Mutex<Option<BuildJob>>,
    populated: Mutex<Option<PopulatedJob>>,
    poi: Mutex<Option<PoiJob>>,
    online: Online,
    plotter: routes::Plotter,
    /// Where the active plotted route is kept across restarts.
    route_path: PathBuf,
}

/// The current data folder and, when it holds a usable build, the open dataset.
struct DataSlot {
    dir: PathBuf,
    origin: DirOrigin,
    dataset: Option<Dataset>,
    error: Option<String>,
}

impl DataSlot {
    fn load(dir: PathBuf, origin: DirOrigin) -> Self {
        let (dataset, error) = match data::open(&dir) {
            Ok(ds) => (ds, None),
            Err(e) => {
                eprintln!("galaxy data in {}: {e}", dir.display());
                (None, Some(e))
            }
        };
        Self { dir, origin, dataset, error }
    }

    fn status(&self) -> DataStatus {
        let dir = self.dir.display().to_string();
        match (&self.dataset, &self.error) {
            (Some(ds), _) => ds.status(),
            (None, Some(error)) => DataStatus::Broken { dir, error: error.clone() },
            (None, None) => DataStatus::Missing { dir },
        }
    }
}

struct BuildJob {
    dir: PathBuf,
    cancel: Arc<AtomicBool>,
    last: Option<ingest::Progress>,
}

/// A download of Spansh's populated-systems dump into `populated.bin`.
struct PopulatedJob {
    cancel: Arc<AtomicBool>,
    last: Option<ingest::populated::Progress>,
}

/// A download of EDAstro's catalogues into `poi.json`.
struct PoiJob {
    cancel: Arc<AtomicBool>,
    last: Option<ingest::poi::Progress>,
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
    journal_dir: Option<String>,
    /// False when the journal folder doesn't exist.
    found: bool,
    commanders: Vec<CommanderTrack>,
    latest_fid: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JumpEvent {
    fid: String,
    jump: Jump,
}

/// A commander's own fleet carrier changed (or went: `None`).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CarrierEvent {
    fid: String,
    carrier: Option<Carrier>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    data_dir: String,
    data_dir_origin: DirOrigin,
    journal_dir: Option<String>,
    journal_dir_custom: bool,
    journal_dir_found: bool,
    details_source: OnlineSource,
    update_reminders: bool,
    friends: Vec<String>,
    view: ViewSettings,
    keys: BTreeMap<String, Vec<String>>,
    home_system: Option<SavedSystem>,
    squadron_system: Option<SavedSystem>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BuildStatus {
    dir: String,
    progress: Option<ingest::Progress>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BuildDone {
    ok: bool,
    cancelled: bool,
    error: Option<String>,
    systems: Option<u64>,
    seconds: Option<f64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PopulatedStatus {
    progress: Option<ingest::populated::Progress>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PopulatedDone {
    ok: bool,
    cancelled: bool,
    error: Option<String>,
    systems: Option<u64>,
    seconds: Option<f64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PoiStatus {
    progress: Option<ingest::poi::Progress>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PoiDone {
    ok: bool,
    cancelled: bool,
    error: Option<String>,
    pois: Option<u64>,
    seconds: Option<f64>,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

/// Runs `f` against the open database, or returns `empty` when there's no data.
fn with_db<T>(state: &AppState, empty: T, f: impl FnOnce(&Connection) -> CmdResult<T>) -> CmdResult<T> {
    let slot = state.data.read().map_err(err)?;
    let Some(ds) = &slot.dataset else { return Ok(empty) };
    let db = ds.db.lock().map_err(err)?;
    f(&db)
}

#[tauri::command]
fn dataset_info(state: State<AppState>) -> CmdResult<DataStatus> {
    Ok(state.data.read().map_err(err)?.status())
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
    // `%` and `_` are LIKE wildcards; system names don't use them.
    let q: String = query.trim().chars().filter(|c| *c != '%' && *c != '_').collect();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    with_db(&state, Vec::new(), |db| {
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
    })
}

#[tauri::command]
fn get_system(state: State<AppState>, rowid: u32) -> CmdResult<Option<SystemInfo>> {
    with_db(&state, None, |db| {
        let mut stmt = db
            .prepare_cached(&format!("SELECT {SYSTEM_COLUMNS} FROM systems WHERE id = ?1"))
            .map_err(err)?;
        stmt.query_row([rowid], row_to_system).optional().map_err(err)
    })
}

#[tauri::command]
fn commander_history(state: State<AppState>) -> CmdResult<CommanderPayload> {
    let configured = journal_dir(&*state.settings.lock().map_err(err)?);
    let mut guard = state.commander.lock().map_err(err)?;
    let Some(source) = guard.as_mut() else {
        return Ok(CommanderPayload {
            journal_dir: configured.as_deref().map(display),
            found: false,
            commanders: Vec::new(),
            latest_fid: None,
        });
    };
    let dir = display(source.dir());
    let history = source.history().map_err(err)?;
    let latest_fid = history.latest().map(|c| c.fid.clone());
    let mut commanders: Vec<CommanderTrack> = history.commanders.values().cloned().collect();
    // Not while holding the journal: placing takes the catalogue's lock.
    drop(guard);
    for carrier in commanders.iter_mut().filter_map(|c| c.carrier.as_mut()) {
        place_carrier(&state, carrier);
    }
    Ok(CommanderPayload { journal_dir: Some(dir), found: true, latest_fid, commanders })
}

/// Gives the carrier's places coordinates where the journal had none: from
/// the catalogue, or the boxel centre for a system it doesn't hold.
fn place_carrier(state: &AppState, carrier: &mut Carrier) {
    let slot = state.data.read().ok();
    let db = slot.as_ref().and_then(|s| s.dataset.as_ref()).and_then(|d| d.db.lock().ok());
    let places = carrier.at.iter_mut().chain(carrier.jump.as_mut().map(|j| &mut j.to));
    for place in places {
        let Some(id64) = place.id64.filter(|_| place.pos.is_none()) else { continue };
        let found = db.as_ref().and_then(|db| {
            let mut stmt = db.prepare_cached("SELECT x32, y32, z32 FROM systems WHERE id64 = ?1").ok()?;
            stmt.query_row([id64 as i64], |r| {
                let c = |i| -> rusqlite::Result<f32> { Ok(r.get::<_, i64>(i)? as f32 / 32.0) };
                Ok([c(0)?, c(1)?, c(2)?])
            })
            .optional()
            .ok()
            .flatten()
        });
        place.pos = Some(found.unwrap_or_else(|| Id64::decode(id64).boxel_centre()));
    }
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

    let slot = state.data.read().map_err(err)?;
    let db = slot.dataset.as_ref().map(|d| d.db.lock()).transpose().map_err(err)?;
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

// --- Settings ----------------------------------------------------------------

fn journal_dir(settings: &Settings) -> Option<PathBuf> {
    settings.journal_dir.clone().or_else(galaxy_core::journal::default_journal_dir)
}

fn open_commander(dir: PathBuf) -> Option<JournalSource> {
    dir.is_dir().then(|| JournalSource::new(dir))
}

fn save_settings(state: &AppState, settings: &Settings) -> CmdResult<()> {
    settings.save(&state.settings_path).map_err(|e| format!("couldn't save settings: {e}"))
}

// Lock order, where more than one is held: build, then data, then settings.

#[tauri::command]
fn get_settings(state: State<AppState>) -> CmdResult<SettingsView> {
    let (data_dir, data_dir_origin) = {
        let slot = state.data.read().map_err(err)?;
        (display(&slot.dir), slot.origin)
    };
    let settings = state.settings.lock().map_err(err)?;
    let journal = journal_dir(&settings);
    Ok(SettingsView {
        data_dir,
        data_dir_origin,
        journal_dir_found: journal.as_ref().is_some_and(|d| d.is_dir()),
        journal_dir: journal.as_deref().map(display),
        journal_dir_custom: settings.journal_dir.is_some(),
        details_source: settings.details_source,
        update_reminders: settings.update_reminders,
        friends: settings.friends.clone(),
        view: settings.view.clone(),
        keys: settings.keys.clone(),
        home_system: settings.home_system.clone(),
        squadron_system: settings.squadron_system.clone(),
    })
}

#[tauri::command]
fn save_view_settings(state: State<AppState>, view: ViewSettings) -> CmdResult<()> {
    let mut settings = state.settings.lock().map_err(err)?;
    if settings.view != view {
        settings.view = view;
        save_settings(&state, &settings)?;
    }
    Ok(())
}

/// Saves the keyboard shortcuts the user changed (the web view owns the defaults).
#[tauri::command]
fn save_keys(state: State<AppState>, keys: BTreeMap<String, Vec<String>>) -> CmdResult<()> {
    let mut settings = state.settings.lock().map_err(err)?;
    if settings.keys != keys {
        settings.keys = keys;
        save_settings(&state, &settings)?;
    }
    Ok(())
}

/// Puts `system` behind a jump button; `None` clears it (and hides the button).
#[tauri::command]
fn set_bookmark(state: State<AppState>, slot: Bookmark, system: Option<SavedSystem>) -> CmdResult<()> {
    if let Some(problem) = system.as_ref().and_then(SavedSystem::problem) {
        return Err(problem.into());
    }
    let mut settings = state.settings.lock().map_err(err)?;
    let saved = settings.bookmark_mut(slot);
    if *saved != system {
        *saved = system;
        save_settings(&state, &settings)?;
    }
    Ok(())
}

/// Switches to an existing build in `dir` (or in a `data` folder inside it).
#[tauri::command]
fn use_data_dir(state: State<AppState>, dir: String) -> CmdResult<DataStatus> {
    let found = data::locate_existing(Path::new(&dir))
        .ok_or("That folder doesn't hold StellarCartography data (a tiles folder and galaxy.sqlite).")?;
    if state.build.lock().map_err(err)?.is_some() {
        return Err("Wait for the catalogue build to finish first.".into());
    }
    let mut slot = state.data.write().map_err(err)?;
    if slot.origin == DirOrigin::Environment {
        return Err("The data folder is set by STELLAR_DATA_DIR.".into());
    }
    let next = DataSlot::load(found.clone(), DirOrigin::Settings);
    if let Some(e) = &next.error {
        return Err(format!("Couldn't open the data there: {e}"));
    }
    let mut settings = state.settings.lock().map_err(err)?;
    settings.data_dir = Some(found.clone());
    save_settings(&state, &settings)?;
    carry_files(&slot.dir, &found);
    *slot = next;
    Ok(slot.status())
}

/// Sets the journal folder; `None` goes back to the game's default.
#[tauri::command]
fn set_journal_dir(state: State<AppState>, dir: Option<String>) -> CmdResult<()> {
    let dir = dir.map(PathBuf::from);
    if let Some(d) = &dir {
        if !d.is_dir() {
            return Err(format!("{} isn't a folder.", d.display()));
        }
    }
    let mut settings = state.settings.lock().map_err(err)?;
    settings.journal_dir = dir;
    save_settings(&state, &settings)?;
    *state.commander.lock().map_err(err)? = journal_dir(&settings).and_then(open_commander);
    Ok(())
}

#[tauri::command]
fn set_details_source(state: State<AppState>, source: OnlineSource) -> CmdResult<()> {
    let mut settings = state.settings.lock().map_err(err)?;
    settings.details_source = source;
    save_settings(&state, &settings)
}

#[tauri::command]
fn set_update_reminders(state: State<AppState>, on: bool) -> CmdResult<()> {
    let mut settings = state.settings.lock().map_err(err)?;
    settings.update_reminders = on;
    save_settings(&state, &settings)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiskSpace {
    /// Free on the data folder's drive.
    free: u64,
    /// Roughly what a catalogue build there needs, temporary files included.
    needed: u64,
    /// Why a build there would be refused, if it would.
    problem: Option<String>,
}

/// Free space for a catalogue build into `dir`, for the first-run card.
#[tauri::command]
fn disk_space(dir: String) -> CmdResult<DiskSpace> {
    let dir = PathBuf::from(dir);
    let needs = ingest::space::catalogue_needs(&dir, None, None);
    Ok(DiskSpace {
        free: ingest::space::available(&dir).map_err(|e| format!("{e:#}"))?,
        needed: needs.data + needs.temp,
        problem: ingest::space::check(&dir, needs, "the catalogue build").err().map(|e| format!("{e:#}")),
    })
}

/// Downloaded datasets that live in the data folder but aren't part of a
/// catalogue build.
const CARRIED_FILES: [&str; 2] = [ingest::populated::FILE_NAME, ingest::poi::FILE_NAME];

/// Brings the map layers' `populated.bin` and the points of interest along
/// to a new data folder (see `carry_file`).
fn carry_files(from: &Path, to: &Path) {
    if from == to {
        return;
    }
    for name in CARRIED_FILES {
        carry_file(from, to, name);
    }
}

/// Copies one downloaded dataset from `from` into `to` when `to` has none or
/// an older one (by modification time, which the copy keeps), so the newest
/// data follows a change of folder. Best effort: a failure only means
/// downloading it again.
fn carry_file(from: &Path, to: &Path, name: &str) {
    let (src, dst) = (from.join(name), to.join(name));
    let Ok(meta) = std::fs::metadata(&src) else { return };
    let Ok(src_time) = meta.modified() else { return };
    if !meta.is_file() || std::fs::metadata(&dst).and_then(|m| m.modified()).is_ok_and(|t| t >= src_time) {
        return;
    }
    // Not `<name>.partial`: a download may be writing that in `to` right now.
    let temp = to.join(format!("{name}.carry"));
    let copied = std::fs::copy(&src, &temp)
        .and_then(|_| std::fs::File::options().write(true).open(&temp)?.set_modified(src_time))
        .and_then(|_| std::fs::rename(&temp, &dst));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&temp);
        eprintln!("couldn't copy {} to {}: {e}", src.display(), to.display());
    }
}

/// A download finished into `dir`. If the data folder changed while it ran
/// (Settings → "Use another folder…", or a build into a new folder), the
/// file goes to the current folder too. Those switches hold the data lock
/// for writing, so they and this run one after the other: either the switch
/// carries the finished file, or this does.
fn deliver(data: &RwLock<DataSlot>, dir: &Path, name: &str) {
    let Ok(slot) = data.read() else { return };
    if slot.dir != dir {
        carry_file(dir, &slot.dir, name);
    }
}

// --- System details ----------------------------------------------------------

/// Everything known about a system: the journal's snapshot when the
/// commander has been there, merged with an online lookup (unless that's
/// switched off). `refresh` skips the hour-long online cache.
#[tauri::command]
async fn system_details(
    state: State<'_, AppState>,
    id64: Option<String>,
    name: String,
    refresh: bool,
) -> CmdResult<SystemDetails> {
    let id64: Option<u64> = id64.and_then(|s| s.parse().ok());
    // The journal is a bonus: if it can't be read, the online answer still shows.
    let journal = match id64 {
        Some(id) => {
            let mut source = state.commander.lock().map_err(err)?;
            match source.as_mut().map(|s| s.snapshots()) {
                Some(Ok(snaps)) => galaxy_core::details::journal::from_snapshots(snaps.systems.get(&id), snaps.stations_in(id)),
                Some(Err(e)) => {
                    eprintln!("journal unavailable for system details: {e:#}");
                    SystemDetails::default()
                }
                None => SystemDetails::default(),
            }
        }
        None => SystemDetails::default(),
    };
    let first = state.settings.lock().map_err(err)?.details_source;
    let online = state.online.lookup(first, id64, &name, refresh).await;
    let base = SystemDetails { links: galaxy_core::details::links(id64, &name, None), ..Default::default() };
    Ok(galaxy_core::details::merge(galaxy_core::details::combine(journal, online), base))
}

// --- Friends -------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FriendStatus {
    name: String,
    /// `None` when the lookup failed; `error` says why.
    position: Option<galaxy_core::friends::FriendPosition>,
    error: Option<String>,
}

/// Saves the friends list: trimmed, deduplicated case-insensitively, order kept.
#[tauri::command]
fn set_friends(state: State<AppState>, names: Vec<String>) -> CmdResult<()> {
    let mut seen: Vec<String> = Vec::new();
    let mut friends = Vec::new();
    for name in names {
        let name = name.trim().to_string();
        let key = name.to_lowercase();
        if !name.is_empty() && !seen.contains(&key) {
            seen.push(key);
            friends.push(name);
        }
    }
    let mut settings = state.settings.lock().map_err(err)?;
    if settings.friends != friends {
        settings.friends = friends;
        save_settings(&state, &settings)?;
    }
    Ok(())
}

/// The last known EDSM position of every commander on the friends list, one
/// lookup each (throttled). A friend who failed keeps their place in the
/// list with the error. `refresh` skips the short cache.
#[tauri::command]
async fn friend_positions(state: State<'_, AppState>, refresh: bool) -> CmdResult<Vec<FriendStatus>> {
    let names = state.settings.lock().map_err(err)?.friends.clone();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        match state.online.friend_position(&name, refresh).await {
            Ok(p) => out.push(FriendStatus { name, position: Some(p), error: None }),
            Err(e) => out.push(FriendStatus { name, position: None, error: Some(format!("{e:#}")) }),
        }
    }
    Ok(out)
}

/// Commander names seen in the journals' `Friends` and wing events, newest
/// sighting first: suggestions for the friends list.
#[tauri::command]
fn journal_friends(state: State<AppState>) -> CmdResult<Vec<String>> {
    let mut source = state.commander.lock().map_err(err)?;
    match source.as_mut() {
        Some(s) => s.friend_names().map_err(err),
        None => Ok(Vec::new()),
    }
}

// --- Route plotting ------------------------------------------------------------

/// The ship to plot galaxy routes with: the newest `Loadout` in the journals
/// for `fid` (or for any commander), reduced to its fuel model. `None` when
/// the journals haven't recorded one.
#[tauri::command]
fn ship_loadout(state: State<AppState>, fid: Option<String>) -> CmdResult<Option<galaxy_core::ship::ShipSpec>> {
    let mut source = state.commander.lock().map_err(err)?;
    let Some(source) = source.as_mut() else { return Ok(None) };
    let snapshot = source.loadout(fid.as_deref()).map_err(err)?;
    Ok(snapshot.and_then(|s| galaxy_core::ship::parse_loadout(&s.line)))
}

/// Plots a route on Spansh (neutron, galaxy or fleet-carrier plotter, by the
/// request's `mode`). Takes seconds to minutes: Spansh queues the job and
/// this polls for the answer. A new plot or `cancel_plot` abandons it.
#[tauri::command]
async fn plot_route(state: State<'_, AppState>, request: routes::RouteRequest) -> CmdResult<routes::PlottedRoute> {
    state.plotter.plot(request).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn cancel_plot(state: State<AppState>) -> CmdResult<()> {
    state.plotter.cancel();
    Ok(())
}

#[tauri::command]
fn saved_route(state: State<AppState>) -> CmdResult<Option<routes::SavedRoute>> {
    Ok(routes::load_saved(&state.route_path))
}

/// Keeps the active route (and progress along it) across restarts; `None` clears it.
#[tauri::command]
fn save_route(state: State<AppState>, route: Option<routes::SavedRoute>) -> CmdResult<()> {
    routes::store_saved(&state.route_path, route.as_ref()).map_err(|e| format!("couldn't save the route: {e}"))
}

/// Sites the details panel links to; nothing else is opened.
const EXTERNAL_SITES: [&str; 4] = ["https://spansh.co.uk/", "https://inara.cz/", "https://www.edsm.net/", "https://edastro.com/"];

#[tauri::command]
fn open_external(app: AppHandle, url: String) -> CmdResult<()> {
    if !EXTERNAL_SITES.iter().any(|site| url.starts_with(site)) {
        return Err(format!("not opening {url}: only Spansh, Inara, EDSM and EDAstro links are allowed"));
    }
    app.opener().open_url(url, None::<&str>).map_err(err)
}

#[tauri::command]
async fn pick_folder(app: AppHandle, title: String, start: Option<String>) -> Option<String> {
    let mut dialog = app.dialog().file().set_title(title);
    if let Some(start) = start.filter(|s| Path::new(s).is_dir()) {
        dialog = dialog.set_directory(start);
    }
    dialog.blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| display(&p))
}

// --- Catalogue build ---------------------------------------------------------

/// Starts building the catalogue from EDSM into `dir` (default: the current
/// data folder). Progress arrives as `build-progress` events and the end as
/// `build-done`. `limit` stops after that many systems, for testing.
#[tauri::command]
fn start_build(app: AppHandle, state: State<AppState>, dir: Option<String>, limit: Option<u64>) -> CmdResult<String> {
    let mut job = state.build.lock().map_err(err)?;
    if job.is_some() {
        return Err("A build is already running.".into());
    }
    let (current, origin) = {
        let slot = state.data.read().map_err(err)?;
        (slot.dir.clone(), slot.origin)
    };
    let target = dir.map(PathBuf::from).unwrap_or(current.clone());
    if origin == DirOrigin::Environment && target != current {
        return Err("The data folder is set by STELLAR_DATA_DIR.".into());
    }
    std::fs::create_dir_all(&target).map_err(|e| format!("Can't create {}: {e}", target.display()))?;
    // An early, rough check (the build checks again once the dump's size is known).
    let needs = ingest::space::catalogue_needs(&target, None, limit);
    ingest::space::check(&target, needs, "the catalogue build").map_err(|e| format!("{e:#}"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    *job = Some(BuildJob { dir: target.clone(), cancel: cancel.clone(), last: None });
    let t = target.clone();
    std::thread::Builder::new()
        .name("catalogue-build".into())
        .spawn(move || run_build(app, t, limit, cancel))
        .map_err(err)?;
    Ok(display(&target))
}

#[tauri::command]
fn cancel_build(state: State<AppState>) -> CmdResult<bool> {
    let job = state.build.lock().map_err(err)?;
    if let Some(job) = job.as_ref() {
        job.cancel.store(true, Ordering::Relaxed);
    }
    Ok(job.is_some())
}

#[tauri::command]
fn build_status(state: State<AppState>) -> CmdResult<Option<BuildStatus>> {
    let job = state.build.lock().map_err(err)?;
    Ok(job.as_ref().map(|j| BuildStatus { dir: display(&j.dir), progress: j.last.clone() }))
}

fn run_build(app: AppHandle, dir: PathBuf, limit: Option<u64>, cancel: Arc<AtomicBool>) {
    let state = app.state::<AppState>();
    let mut opts = ingest::Options::edsm(&dir);
    opts.limit = limit;
    let built = ingest::build(
        &opts,
        &mut |p| {
            if let Ok(mut job) = state.build.lock() {
                if let Some(job) = job.as_mut() {
                    job.last = Some(p.clone());
                }
            }
            let _ = app.emit("build-progress", p);
        },
        cancel.clone(),
    );

    let result = match state.data.write() {
        Ok(mut slot) => {
            let result = built.and_then(|built| {
                // Windows won't replace open files: let go of the database first.
                if slot.dir == dir {
                    slot.dataset = None;
                }
                built.install()
            });
            // Reopen whatever is there now: the new build, or the old one if this failed.
            let (next_dir, origin) = match (&result, slot.origin) {
                (Ok(_), DirOrigin::Environment) => (dir.clone(), DirOrigin::Environment),
                (Ok(_), _) => (dir.clone(), DirOrigin::Settings),
                (Err(_), origin) => (slot.dir.clone(), origin),
            };
            if result.is_ok() {
                carry_files(&slot.dir, &dir);
            }
            *slot = DataSlot::load(next_dir, origin);
            if result.is_ok() && origin == DirOrigin::Settings {
                if let Ok(mut settings) = state.settings.lock() {
                    settings.data_dir = Some(dir.clone());
                    if let Err(e) = save_settings(&state, &settings) {
                        eprintln!("{e}");
                    }
                }
            }
            result
        }
        Err(_) => Err(anyhow::anyhow!("internal error: data lock poisoned")),
    };

    let cancelled = cancel.load(Ordering::Relaxed);
    let done = match result {
        Ok(stats) => BuildDone {
            ok: true,
            cancelled: false,
            error: None,
            systems: Some(stats.systems_stored),
            seconds: Some(stats.seconds_total),
        },
        Err(e) => {
            if !cancelled {
                eprintln!("catalogue build failed: {e:#}");
            }
            BuildDone { ok: false, cancelled, error: (!cancelled).then(|| format!("{e:#}")), systems: None, seconds: None }
        }
    };
    if let Ok(mut job) = state.build.lock() {
        *job = None;
    }
    let _ = app.emit("build-done", done);
}

// --- Populated systems (overlay layers) --------------------------------------

/// Downloads Spansh's populated-systems dump into the current data folder's
/// `populated.bin`. Progress arrives as `populated-progress` events and the
/// end as `populated-done`. Independent of catalogue builds: those never
/// touch `populated.bin`.
#[tauri::command]
fn start_populated_update(app: AppHandle, state: State<AppState>) -> CmdResult<()> {
    let mut job = state.populated.lock().map_err(err)?;
    if job.is_some() {
        return Err("The populated systems are already being downloaded.".into());
    }
    let dir = {
        let slot = state.data.read().map_err(err)?;
        if slot.dataset.is_none() {
            return Err("Build the galaxy catalogue first.".into());
        }
        slot.dir.clone()
    };
    ingest::space::check(&dir, ingest::space::populated_needs(&dir), "the populated-systems update").map_err(|e| format!("{e:#}"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    *job = Some(PopulatedJob { cancel: cancel.clone(), last: None });
    let spawned = std::thread::Builder::new()
        .name("populated-update".into())
        .spawn(move || run_populated_update(app, dir, cancel));
    if let Err(e) = spawned {
        *job = None;
        return Err(err(e));
    }
    Ok(())
}

#[tauri::command]
fn cancel_populated_update(state: State<AppState>) -> CmdResult<bool> {
    let job = state.populated.lock().map_err(err)?;
    if let Some(job) = job.as_ref() {
        job.cancel.store(true, Ordering::Relaxed);
    }
    Ok(job.is_some())
}

#[tauri::command]
fn populated_update_status(state: State<AppState>) -> CmdResult<Option<PopulatedStatus>> {
    let job = state.populated.lock().map_err(err)?;
    Ok(job.as_ref().map(|j| PopulatedStatus { progress: j.last.clone() }))
}

fn run_populated_update(app: AppHandle, dir: PathBuf, cancel: Arc<AtomicBool>) {
    let state = app.state::<AppState>();
    let result = ingest::populated::build(
        &ingest::populated::Options::spansh(&dir),
        &mut |p| {
            if let Ok(mut job) = state.populated.lock() {
                if let Some(job) = job.as_mut() {
                    job.last = Some(p.clone());
                }
            }
            let _ = app.emit("populated-progress", p);
        },
        cancel.clone(),
    );
    let cancelled = cancel.load(Ordering::Relaxed);
    let done = match result {
        Ok(stats) => {
            eprintln!(
                "populated systems: {} in {:.0}s ({} malformed), {} bytes",
                stats.systems, stats.seconds, stats.malformed, stats.file_bytes
            );
            deliver(&state.data, &dir, ingest::populated::FILE_NAME);
            PopulatedDone { ok: true, cancelled: false, error: None, systems: Some(stats.systems), seconds: Some(stats.seconds) }
        }
        Err(e) => {
            if !cancelled {
                eprintln!("populated systems download failed: {e:#}");
            }
            PopulatedDone { ok: false, cancelled, error: (!cancelled).then(|| format!("{e:#}")), systems: None, seconds: None }
        }
    };
    if let Ok(mut job) = state.populated.lock() {
        *job = None;
    }
    let _ = app.emit("populated-done", done);
}

// --- Points of interest ------------------------------------------------------

/// Downloads EDAstro's catalogues into the current data folder's `poi.json`.
/// Progress arrives as `poi-progress` events and the end as `poi-done`.
/// Independent of the other builds and downloads.
#[tauri::command]
fn start_poi_update(app: AppHandle, state: State<AppState>) -> CmdResult<()> {
    let mut job = state.poi.lock().map_err(err)?;
    if job.is_some() {
        return Err("The points of interest are already being downloaded.".into());
    }
    let dir = {
        let slot = state.data.read().map_err(err)?;
        if slot.dataset.is_none() {
            return Err("Build the galaxy catalogue first.".into());
        }
        slot.dir.clone()
    };
    ingest::space::check(&dir, ingest::space::poi_needs(&dir), "the points-of-interest update").map_err(|e| format!("{e:#}"))?;
    let cancel = Arc::new(AtomicBool::new(false));
    *job = Some(PoiJob { cancel: cancel.clone(), last: None });
    let spawned = std::thread::Builder::new().name("poi-update".into()).spawn(move || run_poi_update(app, dir, cancel));
    if let Err(e) = spawned {
        *job = None;
        return Err(err(e));
    }
    Ok(())
}

#[tauri::command]
fn cancel_poi_update(state: State<AppState>) -> CmdResult<bool> {
    let job = state.poi.lock().map_err(err)?;
    if let Some(job) = job.as_ref() {
        job.cancel.store(true, Ordering::Relaxed);
    }
    Ok(job.is_some())
}

#[tauri::command]
fn poi_update_status(state: State<AppState>) -> CmdResult<Option<PoiStatus>> {
    let job = state.poi.lock().map_err(err)?;
    Ok(job.as_ref().map(|j| PoiStatus { progress: j.last.clone() }))
}

fn run_poi_update(app: AppHandle, dir: PathBuf, cancel: Arc<AtomicBool>) {
    let state = app.state::<AppState>();
    let result = ingest::poi::build(
        &ingest::poi::Options::online(&dir),
        &mut |p| {
            if let Ok(mut job) = state.poi.lock() {
                if let Some(job) = job.as_mut() {
                    job.last = Some(p.clone());
                }
            }
            let _ = app.emit("poi-progress", p);
        },
        cancel.clone(),
    );
    let cancelled = cancel.load(Ordering::Relaxed);
    let done = match result {
        Ok(stats) => {
            eprintln!(
                "points of interest: {} in {:.0}s ({} malformed, unmapped {:?}), {} bytes",
                stats.pois, stats.seconds, stats.malformed, stats.unknown_types, stats.file_bytes
            );
            deliver(&state.data, &dir, ingest::poi::FILE_NAME);
            PoiDone { ok: true, cancelled: false, error: None, pois: Some(stats.pois), seconds: Some(stats.seconds) }
        }
        Err(e) => {
            if !cancelled {
                eprintln!("points-of-interest download failed: {e:#}");
            }
            PoiDone { ok: false, cancelled, error: (!cancelled).then(|| format!("{e:#}")), pois: None, seconds: None }
        }
    };
    if let Ok(mut job) = state.poi.lock() {
        *job = None;
    }
    let _ = app.emit("poi-done", done);
}

// --- Data scheme and journal watcher -----------------------------------------

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

/// Polls the journal once a second and forwards new jumps, and changes to
/// commanders' own carriers, to the web view. Polling rather than
/// file-change notifications: the game keeps the journal open and Windows
/// doesn't reliably report its writes.
fn spawn_journal_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let mut sent: HashMap<String, Option<Carrier>> = HashMap::new();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let state = app.state::<AppState>();
            let (new, carriers) = match state.commander.lock() {
                Ok(mut source) => match source.as_mut() {
                    Some(s) => {
                        let new = s.poll();
                        let carriers: Vec<(String, Option<Carrier>)> = s
                            .history()
                            .map(|h| h.commanders.iter().map(|(fid, c)| (fid.clone(), c.carrier.clone())).collect())
                            .unwrap_or_default();
                        (new, carriers)
                    }
                    None => continue,
                },
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
            for (fid, carrier) in carriers {
                if sent.get(&fid) == Some(&carrier) {
                    continue;
                }
                sent.insert(fid.clone(), carrier.clone());
                let mut carrier = carrier;
                if let Some(c) = carrier.as_mut() {
                    place_carrier(&state, c);
                }
                let _ = app.emit("commander-carrier", CarrierEvent { fid, carrier });
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol(SCHEME, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                let dir = state.data.read().ok().filter(|s| s.dataset.is_some()).map(|s| s.dir.clone());
                responder.respond(serve(dir.as_deref(), &path));
            });
        })
        .invoke_handler(tauri::generate_handler![
            dataset_info,
            search_systems,
            get_system,
            commander_history,
            visited_systems,
            get_settings,
            save_view_settings,
            save_keys,
            set_bookmark,
            use_data_dir,
            set_journal_dir,
            pick_folder,
            start_build,
            cancel_build,
            build_status,
            set_details_source,
            set_update_reminders,
            disk_space,
            system_details,
            open_external,
            start_populated_update,
            cancel_populated_update,
            populated_update_status,
            start_poi_update,
            cancel_poi_update,
            poi_update_status,
            ship_loadout,
            set_friends,
            friend_positions,
            journal_friends,
            plot_route,
            cancel_plot,
            saved_route,
            save_route,
        ])
        .setup(|app| {
            let paths = app.path();
            let settings_path = paths.app_config_dir()?.join("settings.json");
            let local = paths.app_local_data_dir()?;
            let default_data_dir = local.join("data");
            let online = Online::new(&local.join("details-cache.sqlite"));
            let settings = Settings::load(&settings_path);
            let (dir, origin) = data::resolve_data_dir(&settings, &default_data_dir);
            eprintln!("data dir: {} ({origin:?})", dir.display());
            let commander = journal_dir(&settings).and_then(open_commander);
            app.manage(AppState {
                settings_path,
                data: RwLock::new(DataSlot::load(dir, origin)),
                settings: Mutex::new(settings),
                commander: Mutex::new(commander),
                build: Mutex::new(None),
                populated: Mutex::new(None),
                poi: Mutex::new(None),
                online,
                plotter: routes::Plotter::default(),
                route_path: local.join("route.json"),
            });
            spawn_journal_watcher(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running StellarCartography");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `body` to `path` with a modification time `secs` after an arbitrary start.
    fn write_at(path: &Path, body: &[u8], secs: u64) {
        std::fs::write(path, body).unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn downloaded_datasets_follow_a_new_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let (layers, poi) = (ingest::populated::FILE_NAME, ingest::poi::FILE_NAME);

        carry_files(&old, &new);
        assert!(!new.join(layers).exists() && !new.join(poi).exists(), "nothing to carry");

        write_at(&old.join(layers), b"layers", 0);
        carry_files(&old, &new);
        assert_eq!(std::fs::read(new.join(layers)).unwrap(), b"layers");
        assert!(!new.join(format!("{layers}.carry")).exists());
        assert!(!new.join(poi).exists(), "only what's there is copied");
        let modified = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
        assert_eq!(modified(&new.join(layers)), modified(&old.join(layers)), "the copy keeps its age");
        carry_files(&new, &old);
        assert_eq!(std::fs::read(old.join(layers)).unwrap(), b"layers", "no copying back and forth");

        // The newer copy wins either way; the other file still comes along.
        write_at(&old.join(layers), b"newer elsewhere", 10);
        write_at(&old.join(poi), b"places", 0);
        carry_files(&old, &new);
        assert_eq!(std::fs::read(new.join(layers)).unwrap(), b"newer elsewhere");
        assert_eq!(std::fs::read(new.join(poi)).unwrap(), b"places");
        write_at(&new.join(layers), b"newest here", 20);
        carry_files(&old, &new);
        assert_eq!(std::fs::read(new.join(layers)).unwrap(), b"newest here");

        // A download's own temporary file in the destination is left alone.
        std::fs::write(new.join(format!("{poi}.partial")), b"half a download").unwrap();
        write_at(&old.join(poi), b"newer places", 30);
        carry_files(&old, &new);
        assert_eq!(std::fs::read(new.join(poi)).unwrap(), b"newer places");
        assert_eq!(std::fs::read(new.join(format!("{poi}.partial"))).unwrap(), b"half a download");
    }

    #[test]
    fn a_download_finishing_after_a_folder_change_follows_it() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let poi = ingest::poi::FILE_NAME;
        // The folder the user switched to meanwhile has an older copy.
        write_at(&new.join(poi), b"stale", 0);
        write_at(&old.join(poi), b"fresh", 10);
        let data = RwLock::new(DataSlot { dir: new.clone(), origin: DirOrigin::Settings, dataset: None, error: None });

        deliver(&data, &old, poi);
        assert_eq!(std::fs::read(new.join(poi)).unwrap(), b"fresh");

        // No change of folder: nothing to do.
        write_at(&new.join(poi), b"current", 20);
        deliver(&data, &new, poi);
        assert_eq!(std::fs::read(new.join(poi)).unwrap(), b"current");
    }

    #[test]
    fn data_paths_cannot_escape_the_data_dir() {
        let root = Path::new("/data");
        assert_eq!(resolve_data_path(root, "/tiles/r0.bin"), Some(root.join("tiles/r0.bin")));
        assert_eq!(resolve_data_path(root, "/../secret"), None);
        assert_eq!(resolve_data_path(root, "/tiles/../../x"), None);
        assert_eq!(resolve_data_path(root, "/C:/Windows/win.ini"), None);
    }
}
