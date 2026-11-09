//! Builds StellarCartography's galaxy data from the EDSM
//! `systemsWithCoordinates.json.gz` dump: `galaxy.sqlite`, octree tiles and
//! density maps. Used by the `ingest` CLI and by the app's build flow.
//! [`populated`] builds the separate populated-systems file behind the map's
//! overlay layers, from Spansh.
//!
//! The dump can be streamed straight from EDSM, so the 3.7 GB file never has
//! to be kept on disk. Everything is written to a staging folder inside the
//! output folder first; [`Built::install`] swaps it in once the whole build
//! has succeeded, so a failed or cancelled build leaves the old data alone.

mod density;
pub mod populated;
mod source;
pub mod space;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use galaxy_core::octree::{self, BuildParams, Point};
use serde::{Deserialize, Serialize};

pub const EDSM_URL: &str = "https://www.edsm.net/dump/systemsWithCoordinates.json.gz";
pub const USER_AGENT: &str = concat!("EDCD-StellarCartography-", env!("CARGO_PKG_VERSION"));

/// Files and folders a build produces, all swapped in together.
const OUTPUTS: [&str; 4] = ["galaxy.sqlite", "tiles", "density.png", "coverage.png"];
const STAGING: &str = ".building";
/// Where the previous build's outputs wait while a new build is swapped in.
const REPLACED: &str = ".replaced";

pub enum Input {
    Url(String),
    File(PathBuf),
}

pub struct Options {
    pub input: Input,
    /// Also save the compressed bytes here while streaming from a URL.
    pub tee: Option<PathBuf>,
    /// Output folder.
    pub out: PathBuf,
    /// Keep only systems inside this sphere (centre, radius in ly).
    pub sphere: Option<([f32; 3], f32)>,
    /// Stop after reading this many records.
    pub limit: Option<u64>,
}

impl Options {
    /// Stream the latest EDSM dump into `out`.
    pub fn edsm(out: impl Into<PathBuf>) -> Self {
        Self { input: Input::Url(EDSM_URL.into()), tee: None, out: out.into(), sphere: None, limit: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Downloading, decompressing and parsing the dump; rows go to SQLite as they arrive.
    Reading,
    Octree,
    DensityMaps,
    /// Building the SQLite name and id64 indexes (no finer progress available).
    Indexing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    /// Compressed bytes of the dump consumed so far.
    pub bytes_read: u64,
    pub bytes_total: Option<u64>,
    pub records: u64,
    pub seconds: f64,
}

#[derive(Serialize, Default, Debug, Clone)]
pub struct Stats {
    /// When the build finished, in Unix seconds.
    pub built_at: u64,
    pub source: String,
    pub source_last_modified: Option<String>,
    pub compressed_bytes: u64,
    pub records_read: u64,
    pub malformed: u64,
    pub systems_stored: u64,
    pub outside_octree: u64,
    pub distinct_boxels: u64,
    pub octree_nodes: usize,
    pub seconds_stream: f64,
    pub seconds_indexes: f64,
    pub seconds_octree: f64,
    pub seconds_density: f64,
    pub seconds_total: f64,
    pub sqlite_bytes: u64,
    pub tiles_bytes: u64,
    pub density_bytes: u64,
}

/// A finished build waiting in the staging folder.
pub struct Built {
    out: PathBuf,
    staging: PathBuf,
    pub stats: Stats,
}

impl Built {
    /// Replaces the output folder's data with the new build. Anything holding
    /// `galaxy.sqlite` open must close it first (Windows won't move open files).
    ///
    /// The old outputs are only renamed aside until every new one is in place,
    /// so a failure part way through can put the old data back.
    pub fn install(self) -> Result<Stats> {
        let aside_dir = self.out.join(REPLACED);
        let _ = std::fs::remove_dir_all(&aside_dir);
        std::fs::create_dir_all(&aside_dir)?;
        let (mut aside, mut placed) = (Vec::new(), Vec::new());
        let mut failure = None;
        for name in OUTPUTS {
            let (new, current, old) = (self.staging.join(name), self.out.join(name), aside_dir.join(name));
            if current.exists() {
                if let Err(e) = std::fs::rename(&current, &old) {
                    failure = Some(anyhow::Error::from(e).context(format!("move old {} aside", current.display())));
                    break;
                }
                aside.push(name);
            }
            if let Err(e) = std::fs::rename(&new, &current) {
                failure = Some(anyhow::Error::from(e).context(format!("move new {} into place", current.display())));
                break;
            }
            placed.push(name);
        }
        if let Some(e) = failure {
            for name in placed {
                let _ = std::fs::rename(self.out.join(name), self.staging.join(name));
            }
            for name in aside {
                let _ = std::fs::rename(aside_dir.join(name), self.out.join(name));
            }
            let _ = std::fs::remove_dir_all(&aside_dir);
            let _ = std::fs::remove_dir_all(&self.staging);
            return Err(e);
        }
        std::fs::write(self.out.join("ingest-stats.json"), serde_json::to_string_pretty(&self.stats)?)?;
        let _ = std::fs::remove_dir_all(&aside_dir);
        let _ = std::fs::remove_dir_all(&self.staging);
        Ok(self.stats)
    }
}

#[derive(Deserialize)]
struct Record<'a> {
    id64: Option<u64>,
    #[serde(borrow)]
    name: std::borrow::Cow<'a, str>,
    coords: Coords,
}

#[derive(Deserialize)]
struct Coords {
    x: f64,
    y: f64,
    z: f64,
}

struct Row {
    rowid: u32,
    id64: Option<u64>,
    name: String,
    /// Coordinates in 1/32 ly, the game's own precision.
    pos32: [i64; 3],
}

const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Compressed bytes per record in EDSM's dump: 3,694,367,811 bytes for
/// 99,390,303 systems (October 2026).
const DUMP_BYTES_PER_RECORD: f64 = 37.2;

/// How many systems to make room for up front: from the dump's size (plus 3%
/// for a denser dump), capped by `limit`. Sizing the points from the start,
/// rather than letting the Vec double, keeps a 2 GB array from briefly
/// needing 6 GB once the catalogue passes a power of two.
fn expected_records(compressed: Option<u64>, limit: Option<u64>) -> usize {
    let from_size = compressed.map(|b| (b as f64 / DUMP_BYTES_PER_RECORD * 1.03) as u64);
    let n = match (from_size, limit) {
        (Some(s), Some(l)) => s.min(l),
        (Some(n), None) | (None, Some(n)) => n,
        (None, None) => 100_000_000,
    };
    n.min(u64::from(Point::MAX_ROWID)) as usize
}

/// Runs the whole build into the staging folder. `progress` is called a few
/// times a second; setting `cancel` stops the build with an error.
pub fn build(opts: &Options, progress: &mut dyn FnMut(&Progress), cancel: Arc<AtomicBool>) -> Result<Built> {
    let staging = opts.out.join(STAGING);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).with_context(|| format!("create {}", staging.display()))?;
    let result = build_into(opts, &staging, progress, cancel);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    Ok(Built { out: opts.out.clone(), staging, stats: result? })
}

fn build_into(opts: &Options, staging: &Path, progress: &mut dyn FnMut(&Progress), cancel: Arc<AtomicBool>) -> Result<Stats> {
    let started = Instant::now();
    let (input, info) = match &opts.input {
        Input::File(path) => source::open_file(path)?,
        Input::Url(url) => source::open_url(url, opts.tee.as_deref())?,
    };
    space::check(&opts.out, space::catalogue_needs(&opts.out, info.total, opts.limit), "the catalogue build")?;
    let mut stats = Stats {
        source: info.description.clone(),
        source_last_modified: info.last_modified.clone(),
        ..Default::default()
    };
    let mut report = |phase: Phase, records: u64| {
        progress(&Progress {
            phase,
            bytes_read: info.bytes_read.load(Ordering::Relaxed),
            bytes_total: info.total,
            records,
            seconds: started.elapsed().as_secs_f64(),
        })
    };

    let sqlite = staging.join("galaxy.sqlite");
    let (tx, rx) = mpsc::sync_channel::<Vec<Row>>(16);
    let writer = {
        let path = sqlite.clone();
        let cancel = cancel.clone();
        std::thread::spawn(move || write_sqlite(&path, rx, cancel))
    };

    let expected = expected_records(info.total, opts.limit);
    let mut points: Vec<Point> = Vec::with_capacity(expected);
    let mut density = density::Accumulator::with_capacity((expected as f64 * density::BOXELS_PER_SYSTEM) as usize);
    let mut batch = Vec::with_capacity(10_000);
    let mut rowid: u32 = 0;
    let mut last_report = Instant::now();

    let decoder = flate2::read::MultiGzDecoder::new(input);
    let mut reader = BufReader::with_capacity(8 << 20, decoder);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim().trim_end_matches(',');
        if !trimmed.starts_with('{') {
            continue;
        }
        stats.records_read += 1;
        let rec: Record = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(_) => {
                stats.malformed += 1;
                continue;
            }
        };
        let pos = [rec.coords.x as f32, rec.coords.y as f32, rec.coords.z as f32];
        if let Some((c, r)) = opts.sphere {
            let d2: f32 = (0..3).map(|i| (pos[i] - c[i]).powi(2)).sum();
            if d2 > r * r {
                continue;
            }
        }
        if rowid >= Point::MAX_ROWID {
            bail!("the dump has more than {} systems, more than the tile format can number", Point::MAX_ROWID);
        }
        rowid += 1;
        if let Some(id) = rec.id64 {
            density.add_system(id, pos);
        }
        if octree::contains_root(pos) {
            let mass_code = rec.id64.map_or(0, |id| (id & 7) as u8);
            if points.len() == points.capacity() {
                // A bigger dump than expected: grow by an eighth, not double.
                points.reserve_exact(points.len() / 8 + 1024);
            }
            points.push(Point::new(pos, rowid, mass_code));
        } else {
            stats.outside_octree += 1;
        }
        batch.push(Row {
            rowid,
            id64: rec.id64,
            name: rec.name.into_owned(),
            pos32: [to32(rec.coords.x), to32(rec.coords.y), to32(rec.coords.z)],
        });
        if batch.len() == batch.capacity() {
            tx.send(std::mem::replace(&mut batch, Vec::with_capacity(10_000)))
                .map_err(|_| anyhow::anyhow!("sqlite writer stopped"))?;
        }
        if last_report.elapsed() > PROGRESS_EVERY {
            last_report = Instant::now();
            report(Phase::Reading, stats.records_read);
            if cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        if opts.limit.is_some_and(|l| stats.records_read >= l) {
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        drop(tx);
        let _ = writer.join();
        bail!("cancelled");
    }
    tx.send(batch).ok();
    drop(tx);
    stats.compressed_bytes = info.bytes_read.load(Ordering::Relaxed);
    stats.systems_stored = u64::from(rowid);
    stats.seconds_stream = started.elapsed().as_secs_f64();
    if rowid == 0 {
        let _ = writer.join();
        bail!("no systems read");
    }

    // The SQLite thread builds its indexes while the octree is built here.
    report(Phase::Octree, stats.records_read);
    let t = Instant::now();
    let index = octree::build(&mut points, &staging.join("tiles"), BuildParams::default())?;
    stats.octree_nodes = index.nodes.len();
    stats.seconds_octree = t.elapsed().as_secs_f64();
    drop(points);

    report(Phase::DensityMaps, stats.records_read);
    let t = Instant::now();
    stats.distinct_boxels = density.boxels() as u64;
    density.write_pngs(staging)?;
    stats.seconds_density = t.elapsed().as_secs_f64();
    drop(density);

    report(Phase::Indexing, stats.records_read);
    stats.seconds_indexes = writer.join().map_err(|_| anyhow::anyhow!("sqlite writer panicked"))??;
    stats.built_at = unix_now();
    finish_sqlite(&sqlite, &stats)?;

    stats.sqlite_bytes = std::fs::metadata(&sqlite)?.len();
    stats.tiles_bytes = dir_size(&staging.join("tiles"))?;
    stats.density_bytes = ["density.png", "coverage.png"]
        .iter()
        .filter_map(|f| std::fs::metadata(staging.join(f)).ok())
        .map(|m| m.len())
        .sum();
    stats.seconds_total = started.elapsed().as_secs_f64();
    Ok(stats)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn to32(v: f64) -> i64 {
    (v * 32.0).round() as i64
}

pub fn parse_sphere(s: &str) -> Result<([f32; 3], f32)> {
    let v: Vec<f32> = s.split(',').map(|p| p.trim().parse()).collect::<Result<_, _>>()?;
    match v.as_slice() {
        [x, y, z, r] => Ok(([*x, *y, *z], *r)),
        _ => bail!("--sphere expects x,y,z,radius"),
    }
}

/// Runs on its own thread: inserts every batch, then builds the indexes.
/// Returns the seconds spent building indexes.
fn write_sqlite(path: &Path, rx: mpsc::Receiver<Vec<Row>>, cancel: Arc<AtomicBool>) -> Result<f64> {
    let mut db = rusqlite::Connection::open(path)?;
    // Index builds dominate the ingest. `threads` lets SQLite's sorter sort
    // and merge its runs in parallel. Leave the page cache small (it sets the
    // run size, and big runs sort on one thread) and keep the sorter's temp
    // files on disk: on 10M systems this took the indexes from 27 s to 4 s,
    // and `temp_store = MEMORY` alone made them 4x slower.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    db.execute_batch(&format!(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA locking_mode = EXCLUSIVE;
         PRAGMA threads = {threads};
         CREATE TABLE systems (
             id   INTEGER PRIMARY KEY,
             id64 INTEGER,
             name TEXT NOT NULL COLLATE NOCASE,
             x32  INTEGER NOT NULL,
             y32  INTEGER NOT NULL,
             z32  INTEGER NOT NULL
         );
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);"
    ))?;
    let txn = db.transaction()?;
    {
        let mut insert = txn.prepare("INSERT INTO systems (id, id64, name, x32, y32, z32) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        for batch in rx {
            for r in batch {
                insert.execute(rusqlite::params![r.rowid, r.id64.map(|v| v as i64), r.name, r.pos32[0], r.pos32[1], r.pos32[2]])?;
            }
        }
    }
    txn.commit()?;
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    let t = Instant::now();
    // Returning true from the handler interrupts a running statement.
    let interrupt = cancel.clone();
    db.progress_handler(100_000, Some(move || interrupt.load(Ordering::Relaxed)));
    db.execute_batch(
        "CREATE INDEX systems_name ON systems (name);
         CREATE INDEX systems_id64 ON systems (id64);",
    )
    .map_err(|e| if cancel.load(Ordering::Relaxed) { anyhow::anyhow!("cancelled") } else { e.into() })?;
    Ok(t.elapsed().as_secs_f64())
}

fn finish_sqlite(path: &Path, stats: &Stats) -> Result<()> {
    let db = rusqlite::Connection::open(path)?;
    let mut put = db.prepare("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?;
    put.execute(["source", &stats.source])?;
    put.execute(["source_last_modified", stats.source_last_modified.as_deref().unwrap_or("")])?;
    put.execute(["systems", &stats.systems_stored.to_string()])?;
    put.execute(["built_at", &stats.built_at.to_string()])?;
    put.execute(["coordinate_scale", "32"])?;
    Ok(())
}

fn dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        total += entry?.metadata()?.len();
    }
    Ok(total)
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_dump(path: &Path, systems: &[(&str, u64, [f64; 3])]) {
        let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast());
        writeln!(gz, "[").unwrap();
        for (i, (name, id64, [x, y, z])) in systems.iter().enumerate() {
            let comma = if i + 1 < systems.len() { "," } else { "" };
            writeln!(gz, r#"{{"id":{i},"id64":{id64},"name":"{name}","coords":{{"x":{x},"y":{y},"z":{z}}},"date":"2020-01-01 00:00:00"}}{comma}"#).unwrap();
        }
        writeln!(gz, "]").unwrap();
        gz.finish().unwrap();
    }

    const SYSTEMS: [(&str, u64, [f64; 3]); 3] = [
        ("Sol", 10477373803, [0.0, 0.0, 0.0]),
        ("Sagittarius A*", 20578934, [25.21875, -20.90625, 25899.96875]),
        ("Colonia", 3238296097059, [-9530.5, -910.28125, 19808.125]),
    ];

    #[test]
    fn builds_and_installs_over_old_data() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("dump.json.gz");
        write_dump(&dump, &SYSTEMS);
        let out = dir.path().join("data");
        std::fs::create_dir_all(out.join("tiles")).unwrap();
        std::fs::write(out.join("tiles").join("stale.bin"), b"old").unwrap();
        std::fs::write(out.join("galaxy.sqlite"), b"old").unwrap();

        let opts = Options { input: Input::File(dump), tee: None, out: out.clone(), sphere: None, limit: None };
        let mut phases = Vec::new();
        let built = build(&opts, &mut |p| phases.push(p.phase), Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(built.stats.systems_stored, 3);
        assert!(phases.contains(&Phase::Indexing));
        // Nothing is replaced until install.
        assert_eq!(std::fs::read(out.join("galaxy.sqlite")).unwrap(), b"old");
        built.install().unwrap();

        assert!(!out.join(STAGING).exists());
        assert!(!out.join(REPLACED).exists());
        assert!(!out.join("tiles").join("stale.bin").exists());
        assert!(out.join("tiles").join("index.json").is_file());
        assert!(out.join("density.png").is_file());
        let db = rusqlite::Connection::open(out.join("galaxy.sqlite")).unwrap();
        let x32: i64 = db.query_row("SELECT x32 FROM systems WHERE name = 'colonia'", [], |r| r.get(0)).unwrap();
        assert_eq!(x32, -9530 * 32 - 16);
        let built_at: String = db.query_row("SELECT value FROM meta WHERE key = 'built_at'", [], |r| r.get(0)).unwrap();
        assert!(built_at.parse::<u64>().unwrap() > 1_700_000_000);
    }

    #[test]
    fn capacity_comes_from_the_dump_size_or_limit() {
        // The October 2026 dump: room for its 99.4M systems plus 3%.
        let full = expected_records(Some(3_694_367_811), None);
        assert!((99_390_303..103_000_000).contains(&full), "{full}");
        assert_eq!(expected_records(Some(3_694_367_811), Some(2_000_000)), 2_000_000);
        assert_eq!(expected_records(None, Some(5)), 5);
        assert_eq!(expected_records(None, None), 100_000_000);
    }

    #[test]
    fn cancelled_build_leaves_old_data() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("dump.json.gz");
        write_dump(&dump, &SYSTEMS);
        let out = dir.path().join("data");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("galaxy.sqlite"), b"old").unwrap();

        let opts = Options { input: Input::File(dump), tee: None, out: out.clone(), sphere: None, limit: None };
        let result = build(&opts, &mut |_| {}, Arc::new(AtomicBool::new(true)));
        assert!(result.is_err());
        assert!(!out.join(STAGING).exists());
        assert_eq!(std::fs::read(out.join("galaxy.sqlite")).unwrap(), b"old");
    }
}
