//! `ingest`: EDSM `systemsWithCoordinates.json.gz` -> `galaxy.sqlite`, octree
//! tiles and density maps; with `--populated`, Spansh's
//! `galaxy_populated.json.gz` -> `populated.bin` for the map's overlay layers;
//! with `--poi`, EDAstro's catalogues -> `poi.json` for the points of interest.
//! The app can run the same builds itself; this CLI is for scripting and
//! development.
//!
//! ```text
//! cargo run --release -p ingest -- --out data
//! cargo run --release -p ingest -- --limit 2000000 --out data      # quick subset
//! cargo run --release -p ingest -- --input dump.json.gz --out data
//! cargo run --release -p ingest -- --populated --out data          # overlay layers
//! cargo run --release -p ingest -- --poi --out data                # points of interest
//! cargo run --release -p ingest -- --maps --out data               # redraw the density maps only
//! ```

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use ingest::{human_bytes, poi, populated, Input, Options, Phase, EDSM_URL};

#[derive(Parser)]
#[command(about = "Build StellarCartography's galaxy data from the EDSM systems dump")]
struct Args {
    /// Build populated.bin (the overlay layers) from Spansh's populated-systems
    /// dump instead of the catalogue.
    #[arg(long, conflicts_with_all = ["tee", "sphere"])]
    populated: bool,
    /// Build poi.json (the points of interest) from EDAstro's catalogues
    /// instead. With --input, read local copies from that folder (gec.json,
    /// edsmPOI.csv, fleetcarriers.csv, codex-data.csv).
    #[arg(long, conflicts_with_all = ["populated", "tee", "sphere", "url", "limit"])]
    poi: bool,
    /// Redraw density.png and coverage.png from the galaxy.sqlite already in
    /// --out, without downloading anything.
    #[arg(long, conflicts_with_all = ["populated", "poi", "input", "tee", "sphere", "url", "limit"])]
    maps: bool,
    /// Read a local copy of the dump instead of streaming it.
    #[arg(long, conflicts_with = "url")]
    input: Option<PathBuf>,
    /// URL to stream the dump from (default: EDSM's, or Spansh's with --populated).
    #[arg(long)]
    url: Option<String>,
    /// Also save the compressed bytes to this file while streaming.
    #[arg(long, conflicts_with = "input")]
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

fn main() -> Result<()> {
    let args = Args::parse();
    if args.populated {
        return build_populated(args);
    }
    if args.poi {
        return build_poi(args);
    }
    if args.maps {
        let s = ingest::rebuild_maps(&args.out)?;
        eprintln!(
            "done: density maps redrawn from {} systems ({:.0}s reading galaxy.sqlite, {:.0}s drawing)",
            s.systems, s.seconds_read, s.seconds_draw
        );
        return Ok(());
    }
    let opts = Options {
        input: match args.input {
            Some(path) => Input::File(path),
            None => Input::Url(args.url.unwrap_or_else(|| EDSM_URL.into())),
        },
        tee: args.tee,
        out: args.out,
        sphere: args.sphere.as_deref().map(ingest::parse_sphere).transpose()?,
        limit: args.limit,
    };

    let mut last_print = Instant::now();
    let mut last_phase = None;
    let built = ingest::build(
        &opts,
        &mut |p| {
            if last_phase != Some(p.phase) {
                last_phase = Some(p.phase);
                eprintln!("[{:4.0}s] {:?}", p.seconds, p.phase);
            } else if p.phase == Phase::Reading && last_print.elapsed().as_secs() >= 5 {
                last_print = Instant::now();
                let pct = p.bytes_total.map_or(String::new(), |t| format!(" ({:.1}%)", p.bytes_read as f64 * 100.0 / t as f64));
                eprintln!(
                    "  {} compressed{pct}, {} records, {:.0} records/s",
                    human_bytes(p.bytes_read),
                    p.records,
                    p.records as f64 / p.seconds
                );
            }
        },
        Arc::new(AtomicBool::new(false)),
    )?;
    let stats = built.install()?;
    eprintln!(
        "done in {:.0}s: {} systems ({} malformed, {} outside the octree), {} octree nodes, indexes {:.0}s",
        stats.seconds_total, stats.systems_stored, stats.malformed, stats.outside_octree, stats.octree_nodes, stats.seconds_indexes
    );
    eprintln!(
        "sqlite {}, tiles {}, density maps {}",
        human_bytes(stats.sqlite_bytes),
        human_bytes(stats.tiles_bytes),
        human_bytes(stats.density_bytes)
    );
    Ok(())
}

fn build_populated(args: Args) -> Result<()> {
    let opts = populated::Options {
        input: match args.input {
            Some(path) => Input::File(path),
            None => Input::Url(args.url.unwrap_or_else(|| populated::SPANSH_POPULATED_URL.into())),
        },
        out: args.out,
        limit: args.limit,
    };
    let mut last_print = Instant::now();
    let stats = populated::build(
        &opts,
        &mut |p| {
            if last_print.elapsed().as_secs() >= 5 {
                last_print = Instant::now();
                let pct = p.bytes_total.map_or(String::new(), |t| format!(" ({:.1}%)", p.bytes_read as f64 * 100.0 / t as f64));
                eprintln!("  [{:4.0}s] {} compressed{pct}, {} systems", p.seconds, human_bytes(p.bytes_read), p.systems);
            }
        },
        Arc::new(AtomicBool::new(false)),
    )?;
    eprintln!(
        "done in {:.0}s: {} systems ({} malformed), {} faction presences, newest update {}; {} written",
        stats.seconds,
        stats.systems,
        stats.malformed,
        stats.presences,
        stats.newest_update.as_deref().unwrap_or("unknown"),
        human_bytes(stats.file_bytes)
    );
    Ok(())
}

fn build_poi(args: Args) -> Result<()> {
    let opts = poi::Options {
        sources: match args.input {
            Some(dir) => poi::Sources::Dir(dir),
            None => poi::Sources::Online,
        },
        out: args.out,
    };
    let mut last_print = Instant::now();
    let stats = poi::build(
        &opts,
        &mut |p| {
            if last_print.elapsed().as_secs() >= 5 {
                last_print = Instant::now();
                let pct = p.bytes_total.map_or(String::new(), |t| format!(" ({:.1}%)", p.bytes_read as f64 * 100.0 / t as f64));
                eprintln!(
                    "  [{:4.0}s] {}/{} {}: {}{pct}, {} points of interest",
                    p.seconds,
                    p.step,
                    p.steps,
                    p.phase.file_name(),
                    human_bytes(p.bytes_read),
                    p.pois
                );
            }
        },
        Arc::new(AtomicBool::new(false)),
    )?;
    let counts: Vec<String> = stats.by_category.iter().map(|(k, v)| format!("{k} {v}")).collect();
    eprintln!(
        "done in {:.0}s: {} points of interest ({}), {} malformed, {} codex launch credits dropped; {} downloaded, {} written",
        stats.seconds,
        stats.pois,
        counts.join(", "),
        stats.malformed,
        stats.launch_credits,
        human_bytes(stats.bytes_downloaded),
        human_bytes(stats.file_bytes)
    );
    if !stats.unknown_types.is_empty() {
        eprintln!("unmapped types (filed as sights): {:?}", stats.unknown_types);
    }
    Ok(())
}
