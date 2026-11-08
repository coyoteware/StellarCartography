//! `ingest`: EDSM `systemsWithCoordinates.json.gz` -> `galaxy.sqlite`, octree
//! tiles and density maps. The app can run the same build itself; this CLI
//! is for scripting and development.
//!
//! ```text
//! cargo run --release -p ingest -- --out data
//! cargo run --release -p ingest -- --limit 2000000 --out data      # quick subset
//! cargo run --release -p ingest -- --input dump.json.gz --out data
//! ```

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use ingest::{human_bytes, Input, Options, Phase, EDSM_URL};

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
