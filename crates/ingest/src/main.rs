//! `ingest`: EDSM `systemsWithCoordinates.json.gz` -> `galaxy.sqlite`, octree
//! tiles and density maps.
//!
//! The dump can be streamed straight from EDSM, so the 3.7 GB file never has
//! to be kept on disk:
//!
//! ```text
//! cargo run --release -p ingest -- --out data
//! cargo run --release -p ingest -- --limit 2000000 --out data      # quick subset
//! cargo run --release -p ingest -- --input dump.json.gz --out data
//! ```

mod density;
mod source;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Parser;
use galaxy_core::octree::{self, BuildParams, Point};
use serde::{Deserialize, Serialize};

pub const EDSM_URL: &str = "https://www.edsm.net/dump/systemsWithCoordinates.json.gz";
pub const USER_AGENT: &str = concat!("EDCD-StellarCartography-", env!("CARGO_PKG_VERSION"));

#[derive(Parser)]
#[command(about = "Build StellarCartography's galaxy data from the EDSM systems dump")]
struct Args {
    /// Read a local copy of the dump instead of streaming it.
    #[arg(long, conflicts_with = "url")]
    input: Option<PathBuf>,
    /// URL to stream the dump from.
    #[arg(long)]
    url: Option<String>,
    /// Also save the compressed bytes to this file while streaming.
    #[arg(long, requires = "url")]
    tee: Option<PathBuf>,
    /// Output directory.
    #[arg(long, default_value = "data")]
    out: PathBuf,
    /// Keep only systems inside a sphere, given as "x,y,z,radius" in ly.
    #[arg(long)]
    sphere: Option<String>,
    /// Stop after reading this many records from the dump.
    #[arg(long)]
    limit: Option<u64>,
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

#[derive(Serialize, Default)]
struct Stats {
    source: String,
    source_last_modified: Option<String>,
    compressed_bytes: u64,
    records_read: u64,
    malformed: u64,
    systems_stored: u64,
    outside_octree: u64,
    distinct_boxels: u64,
    octree_nodes: usize,
    seconds_stream: f64,
    seconds_indexes: f64,
    seconds_octree: f64,
    seconds_density: f64,
    seconds_total: f64,
    sqlite_bytes: u64,
    tiles_bytes: u64,
    density_bytes: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let started = Instant::now();
    let sphere = args.sphere.as_deref().map(parse_sphere).transpose()?;
    std::fs::create_dir_all(&args.out)?;

    let (input, info) = match &args.input {
        Some(path) => source::open_file(path)?,
        None => source::open_url(args.url.as_deref().unwrap_or(EDSM_URL), args.tee.as_deref())?,
    };
    let mut stats = Stats {
        source: info.description.clone(),
        source_last_modified: info.last_modified.clone(),
        ..Default::default()
    };
    eprintln!("reading {} ({})", info.description, info.total.map_or("size unknown".into(), human_bytes));

    let sqlite_tmp = args.out.join("galaxy.sqlite.tmp");
    let _ = std::fs::remove_file(&sqlite_tmp);
    let (tx, rx) = mpsc::sync_channel::<Vec<Row>>(16);
    let writer = {
        let path = sqlite_tmp.clone();
        std::thread::spawn(move || write_sqlite(&path, rx))
    };

    let mut points: Vec<Point> = Vec::with_capacity(if args.limit.is_some() { 1 << 20 } else { 100_000_000 });
    let mut density = density::Accumulator::new();
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
        if let Some((c, r)) = sphere {
            let d2: f32 = (0..3).map(|i| (pos[i] - c[i]).powi(2)).sum();
            if d2 > r * r {
                continue;
            }
        }
        rowid += 1;
        if let Some(id) = rec.id64 {
            density.add_system(id, pos);
        }
        if octree::contains_root(pos) {
            let mass_code = rec.id64.map_or(0, |id| (id & 7) as u8);
            points.push(Point { pos, rowid, mass_code });
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
                .context("sqlite writer stopped")?;
        }
        if last_report.elapsed() > Duration::from_secs(5) {
            last_report = Instant::now();
            let read = info.bytes_read.load(Ordering::Relaxed);
            let pct = info.total.map_or(String::new(), |t| format!(" ({:.1}%)", read as f64 * 100.0 / t as f64));
            eprintln!(
                "  {} compressed{pct}, {} records, {:.0} records/s",
                human_bytes(read),
                stats.records_read,
                stats.records_read as f64 / started.elapsed().as_secs_f64()
            );
        }
        if args.limit.is_some_and(|l| stats.records_read >= l) {
            break;
        }
    }
    tx.send(batch).ok();
    drop(tx);
    stats.compressed_bytes = info.bytes_read.load(Ordering::Relaxed);
    stats.systems_stored = u64::from(rowid);
    stats.seconds_stream = started.elapsed().as_secs_f64();
    eprintln!(
        "stream done: {} systems in {:.0}s ({} malformed, {} outside the octree)",
        rowid, stats.seconds_stream, stats.malformed, stats.outside_octree
    );
    if rowid == 0 {
        bail!("no systems read");
    }

    // The SQLite thread builds its indexes while the octree is built here.
    let t = Instant::now();
    let tiles_tmp = args.out.join("tiles.tmp");
    let _ = std::fs::remove_dir_all(&tiles_tmp);
    let index = octree::build(&mut points, &tiles_tmp, BuildParams::default())?;
    stats.octree_nodes = index.nodes.len();
    stats.seconds_octree = t.elapsed().as_secs_f64();
    eprintln!("octree: {} nodes in {:.0}s", index.nodes.len(), stats.seconds_octree);
    drop(points);

    let t = Instant::now();
    stats.distinct_boxels = density.boxels() as u64;
    density.write_pngs(&args.out)?;
    stats.seconds_density = t.elapsed().as_secs_f64();
    eprintln!("density maps: {} boxels in {:.0}s", stats.distinct_boxels, stats.seconds_density);

    let index_secs = writer.join().expect("sqlite writer panicked")?;
    stats.seconds_indexes = index_secs;
    finish_sqlite(&sqlite_tmp, &stats)?;

    // Swap the new data in only once everything has succeeded.
    let sqlite = args.out.join("galaxy.sqlite");
    let tiles = args.out.join("tiles");
    let _ = std::fs::remove_file(&sqlite);
    let _ = std::fs::remove_dir_all(&tiles);
    std::fs::rename(&sqlite_tmp, &sqlite)?;
    std::fs::rename(&tiles_tmp, &tiles)?;

    stats.sqlite_bytes = std::fs::metadata(&sqlite)?.len();
    stats.tiles_bytes = dir_size(&tiles)?;
    stats.density_bytes = ["density.png", "coverage.png"]
        .iter()
        .filter_map(|f| std::fs::metadata(args.out.join(f)).ok())
        .map(|m| m.len())
        .sum();
    stats.seconds_total = started.elapsed().as_secs_f64();
    std::fs::write(args.out.join("ingest-stats.json"), serde_json::to_string_pretty(&stats)?)?;
    eprintln!(
        "done in {:.0}s: sqlite {}, tiles {}, density maps {}",
        stats.seconds_total,
        human_bytes(stats.sqlite_bytes),
        human_bytes(stats.tiles_bytes),
        human_bytes(stats.density_bytes)
    );
    Ok(())
}

fn to32(v: f64) -> i64 {
    (v * 32.0).round() as i64
}

fn parse_sphere(s: &str) -> Result<([f32; 3], f32)> {
    let v: Vec<f32> = s.split(',').map(|p| p.trim().parse()).collect::<Result<_, _>>()?;
    match v.as_slice() {
        [x, y, z, r] => Ok(([*x, *y, *z], *r)),
        _ => bail!("--sphere expects x,y,z,radius"),
    }
}

/// Runs on its own thread: inserts every batch, then builds the indexes.
/// Returns the seconds spent building indexes.
fn write_sqlite(path: &Path, rx: mpsc::Receiver<Vec<Row>>) -> Result<f64> {
    let mut db = rusqlite::Connection::open(path)?;
    db.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA locking_mode = EXCLUSIVE;
         PRAGMA cache_size = -1000000;
         PRAGMA temp_store = MEMORY;
         CREATE TABLE systems (
             id   INTEGER PRIMARY KEY,
             id64 INTEGER,
             name TEXT NOT NULL COLLATE NOCASE,
             x32  INTEGER NOT NULL,
             y32  INTEGER NOT NULL,
             z32  INTEGER NOT NULL
         );
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
    )?;
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
    let t = Instant::now();
    db.execute_batch(
        "CREATE INDEX systems_name ON systems (name);
         CREATE INDEX systems_id64 ON systems (id64);",
    )?;
    Ok(t.elapsed().as_secs_f64())
}

fn finish_sqlite(path: &Path, stats: &Stats) -> Result<()> {
    let db = rusqlite::Connection::open(path)?;
    let mut put = db.prepare("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?;
    put.execute(["source", &stats.source])?;
    put.execute(["source_last_modified", stats.source_last_modified.as_deref().unwrap_or("")])?;
    put.execute(["systems", &stats.systems_stored.to_string()])?;
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
