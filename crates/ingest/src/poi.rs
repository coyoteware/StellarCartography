//! Points of interest for the map's directory and marker layer, from four
//! files EDAstro publishes:
//!
//! * the Galactic Exploration Catalog (`gec/json/all`): curated sights, with
//!   summaries and links;
//! * `edsmPOI.csv`: EDSM's Galactic Mapping Project (megaships, DSSA
//!   carriers, nebulae, stellar remnants, historic sites...);
//! * `fleetcarriers.csv`: every known fleet carrier and where it was last
//!   seen, counted per system into carrier hubs;
//! * `codex-data.csv`: every codex entry players have reported, per system,
//!   filtered down to Guardian and Thargoid sites. It's the big one (about
//!   875 MB); it's streamed and only a few thousand rows are kept.
//!
//! Each entry is normalised into a [`Poi`] with one of a small set of
//! categories, and all of them are written to `poi.json` in the data folder,
//! which the web view reads as it is.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use galaxy_core::details::{iso_from_unix, unix_from_iso};
use serde::{Deserialize, Serialize};

use crate::{source, space};

/// The output file, in the data folder next to the catalogue.
pub const FILE_NAME: &str = "poi.json";
pub const VERSION: u32 = 1;

const EDASTRO: &str = "https://edastro.com";

/// Carriers count towards a hub if they were seen within this many days of
/// the newest sighting in the file.
pub const CARRIER_WINDOW_DAYS: i64 = 30;
/// Carriers a system needs (within the window) to be listed as a hub.
pub const CARRIER_HUB_MIN: u32 = 10;
/// Summaries are cut to about this many characters.
const SUMMARY_CHARS: usize = 300;

/// One of the four files, in the order they're read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Gec,
    Edsm,
    Carriers,
    Codex,
}

impl Phase {
    pub const ALL: [Phase; 4] = [Phase::Gec, Phase::Edsm, Phase::Carriers, Phase::Codex];

    /// The file's name on EDAstro, and in an `--input` folder.
    pub fn file_name(self) -> &'static str {
        match self {
            Phase::Gec => "gec.json",
            Phase::Edsm => "edsmPOI.csv",
            Phase::Carriers => "fleetcarriers.csv",
            Phase::Codex => "codex-data.csv",
        }
    }

    pub fn url(self) -> String {
        match self {
            Phase::Gec => format!("{EDASTRO}/gec/json/all"),
            _ => format!("{EDASTRO}/mapcharts/files/{}", self.file_name()),
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Phase::Gec => "the Galactic Exploration Catalog",
            Phase::Edsm => "EDSM's Galactic Mapping Project",
            Phase::Carriers => "the fleet carrier list",
            Phase::Codex => "the codex reports",
        }
    }
}

pub enum Sources {
    /// Stream everything from EDAstro.
    Online,
    /// Read local copies, named as [`Phase::file_name`] says.
    Dir(PathBuf),
}

pub struct Options {
    pub sources: Sources,
    /// Data folder; the file is written as `poi.json` inside it.
    pub out: PathBuf,
}

impl Options {
    pub fn online(out: impl Into<PathBuf>) -> Self {
        Self { sources: Sources::Online, out: out.into() }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: Phase,
    /// 1-based position of `phase` among [`Phase::ALL`].
    pub step: u32,
    pub steps: u32,
    /// Bytes of the current file read so far.
    pub bytes_read: u64,
    pub bytes_total: Option<u64>,
    /// Points of interest found so far.
    pub pois: u64,
    pub seconds: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub pois: u64,
    pub by_category: BTreeMap<String, u64>,
    /// Types none of the mapping tables know, filed under "sights".
    pub unknown_types: BTreeMap<String, u64>,
    /// Entries or rows that couldn't be read.
    pub malformed: u64,
    pub bytes_downloaded: u64,
    pub file_bytes: u64,
    pub seconds: f64,
}

/// What `poi.json` holds. The web view reads the same fields (app/src/poi.ts).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoiFile {
    pub version: u32,
    /// Unix seconds.
    pub built_at: u64,
    pub sources: Vec<SourceStamp>,
    /// Points of interest per category.
    pub counts: BTreeMap<String, u64>,
    pub pois: Vec<Poi>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceStamp {
    pub name: String,
    pub url: String,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Poi {
    /// Unique within the file: `<source>:<id in that source>`.
    pub id: String,
    /// "gec", "edsm", "codex" or "carriers" (and later, "user").
    pub source: String,
    /// One of [`Category::id`].
    pub category: String,
    /// What it is, more precisely: "Guardian Structure", "Megaship", ...
    pub kind: String,
    pub name: String,
    /// The system it's in, when it's in one (some nebulae are regions).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub system: Option<String>,
    /// Kept as a string: addresses can exceed 2^53.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id64: Option<String>,
    /// x, y, z in ly, game frame.
    pub pos: [f32; 3],
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,
    /// Carrier hubs: how many carriers.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub weight: Option<u32>,
}

// --- Categories ----------------------------------------------------------------

/// The directory's categories; the web view gives each a marker shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Guardian,
    Thargoid,
    Xeno,
    CarrierHub,
    Outpost,
    Nebula,
    PlanetaryNebula,
    Stellar,
    Planet,
    Sights,
}

impl Category {
    pub fn id(self) -> &'static str {
        match self {
            Category::Guardian => "guardian",
            Category::Thargoid => "thargoid",
            Category::Xeno => "xeno",
            Category::CarrierHub => "carrierHub",
            Category::Outpost => "outpost",
            Category::Nebula => "nebula",
            Category::PlanetaryNebula => "planetaryNebula",
            Category::Stellar => "stellar",
            Category::Planet => "planet",
            Category::Sights => "sights",
        }
    }
}

/// GEC's types. EDSM's file repeats an older copy of the GEC under the same
/// type names; those rows are skipped in favour of the GEC's own.
fn gec_category(kind: &str) -> Option<Category> {
    Some(match kind {
        "Mystery and Xenology" => Category::Xeno,
        "Deep Space Outpost" => Category::Outpost,
        "Nebulae" => Category::Nebula,
        "Notable Stellar Phenomena" | "Stellar Features" => Category::Stellar,
        "Planetary Features" | "Organic" | "Green Gas Giants" | "Planetary Circumnavigation" => Category::Planet,
        "Sights and Scenery" | "Tourist Beacons" | "Community" | "Glitches" | "Historical" | "Memorials"
        | "Inhabited System" => Category::Sights,
        _ => return None,
    })
}

enum EdsmType {
    Keep(Category, &'static str),
    Skip,
    Unknown,
}

/// EDSM's Galactic Mapping types: the category and a readable kind.
fn edsm_type(kind: &str) -> EdsmType {
    use Category::*;
    use EdsmType::{Keep, Skip};
    match kind {
        "megaship" | "megaship2" => Keep(Outpost, "Megaship"),
        "DSSAcarrier" => Keep(Outpost, "DSSA carrier"),
        "OASIScarrier" => Keep(Outpost, "OASIS carrier"),
        "PIONEERcarrier" => Keep(Outpost, "PIONEER carrier"),
        "independentOutpost" => Keep(Outpost, "Independent outpost"),
        "deepSpaceOutpost" => Keep(Outpost, "Deep space outpost"),
        "mysteryPOI" => Keep(Xeno, "Mystery"),
        "nebula" => Keep(Nebula, "Nebula"),
        "starCluster" => Keep(Nebula, "Star cluster"),
        "planetaryNebula" => Keep(PlanetaryNebula, "Planetary nebula"),
        "stellarRemnant" => Keep(Stellar, "Stellar remnant"),
        "blackHole" => Keep(Stellar, "Black hole"),
        "pulsar" => Keep(Stellar, "Pulsar"),
        "CCLNSP" => Keep(Stellar, "Notable stellar phenomenon"),
        "planetFeatures" => Keep(Planet, "Planetary feature"),
        "geyserPOI" => Keep(Planet, "Geysers"),
        "surfacePOI" => Keep(Planet, "Surface feature"),
        "organicPOI" => Keep(Planet, "Organic"),
        "CCLHBIO" | "CCLOBIO" => Keep(Planet, "Biology"),
        "GGG" => Keep(Planet, "Green gas giant"),
        "jumponiumRichSystem" => Keep(Planet, "Jumponium-rich system"),
        "minorPOI" => Keep(Sights, "Point of interest"),
        "regional" => Keep(Sights, "Regional landmark"),
        "historicalLocation" => Keep(Sights, "Historical location"),
        "restrictedSectors" => Keep(Sights, "Permit-locked region"),
        // Personal carrier bookmarks: hubs come from the carrier list instead.
        "carrier"
        // Unnamed 10 ly grid points, superseded by the codex reports.
        | "guardian"
        // Bare system names with no description.
        | "canonn"
        // DSSA carriers no longer deployed, or not yet.
        | "DSSAundeploy" | "DSSAtmp" | "DSSAseen" => Skip,
        // Route waypoints and map-legend markers.
        k if k.starts_with("trit_hwy") || k.starts_with("STAR") => Skip,
        k if gec_category(k).is_some() => Skip,
        _ => EdsmType::Unknown,
    }
}

/// A Guardian or Thargoid codex entry: its category, a readable label, and
/// how strongly it marks a site (the strongest names the site's kind).
fn codex_entry(id: &str) -> Option<(Category, &'static str, u8, &'static str)> {
    use Category::{Guardian, Thargoid};
    let rest = id.strip_prefix("codex_ent_")?;
    if let Some(g) = rest.strip_prefix("guardian_") {
        let (label, rank, kind) = match g {
            "structure" => ("Guardian Structure", 4, "Guardian Structure"),
            "ruins" => ("Guardian Ruins", 3, "Guardian Ruins"),
            "beacons" => ("Guardian Beacon", 2, "Guardian Beacon"),
            "terminal" => ("Guardian Data Terminal", 1, "Guardian site"),
            "pylon" => ("Guardian Pylon", 1, "Guardian site"),
            "sentinel" => ("Guardian Sentinel", 1, "Guardian site"),
            "data_logs" => ("Guardian Codex", 1, "Guardian site"),
            _ => ("Guardian artefact", 0, "Guardian site"),
        };
        return Some((Guardian, label, rank, kind));
    }
    if let Some(t) = rest.strip_prefix("thargoid_") {
        let (label, rank, kind) = match t {
            "structure" => ("Thargoid Structure", 4, "Thargoid Structure"),
            t if t.starts_with("tower") => ("Thargoid Spire", 3, "Thargoid Spire site"),
            t if t.starts_with("coral") => ("Thargoid Coral", 3, "Thargoid Spire site"),
            t if t.starts_with("barnacle_matrix") => ("Barnacle Matrix", 3, "Thargoid Spire site"),
            "large_spike" => ("Hardened Spike", 3, "Thargoid Spire site"),
            "barnacle_01" => ("Common Thargoid Barnacle", 2, "Thargoid Barnacle site"),
            "barnacle_02" => ("Large Thargoid Barnacle", 2, "Thargoid Barnacle site"),
            "barnacle_spikes" => ("Thargoid Barnacle Barbs", 2, "Thargoid Barnacle site"),
            _ => ("Thargoid artefact", 0, "Thargoid site"),
        };
        return Some((Thargoid, label, rank, kind));
    }
    if let Some(t) = rest.strip_prefix("tg_") {
        let label = match t {
            "pod" => "Thargoid Pod",
            "datascan" => "Thargoid Uplink Device",
            "transmitter" => "Thargoid Device",
            _ => "Thargoid artefact",
        };
        return Some((Thargoid, label, 1, "Thargoid surface site"));
    }
    None
}

// --- Building ------------------------------------------------------------------

const PROGRESS_EVERY: Duration = Duration::from_millis(250);

struct Run<'a> {
    started: Instant,
    progress: &'a mut dyn FnMut(&Progress),
    cancel: Arc<AtomicBool>,
    last_report: Instant,
    pois: Vec<Poi>,
    stats: Stats,
}

impl Run<'_> {
    fn report(&mut self, phase: Phase, bytes_read: u64, bytes_total: Option<u64>) -> Result<()> {
        let step = Phase::ALL.iter().position(|p| *p == phase).unwrap_or(0) as u32 + 1;
        (self.progress)(&Progress {
            phase,
            step,
            steps: Phase::ALL.len() as u32,
            bytes_read,
            bytes_total,
            pois: self.pois.len() as u64,
            seconds: self.started.elapsed().as_secs_f64(),
        });
        if self.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(())
    }

    /// Reports at most a few times a second.
    fn tick(&mut self, phase: Phase, info: &source::SourceInfo) -> Result<()> {
        if self.last_report.elapsed() < PROGRESS_EVERY {
            return Ok(());
        }
        self.last_report = Instant::now();
        self.report(phase, info.bytes_read.load(Ordering::Relaxed), info.total)
    }

    fn malformed(&mut self) {
        self.stats.malformed += 1;
    }
}

/// Downloads (or reads) the four files and writes `poi.json` into
/// `opts.out`, replacing any earlier copy only once the new one is complete.
/// `progress` is called a few times a second; setting `cancel` stops with an
/// error and leaves the old file alone.
pub fn build(opts: &Options, progress: &mut dyn FnMut(&Progress), cancel: Arc<AtomicBool>) -> Result<Stats> {
    let started = Instant::now();
    space::check(&opts.out, space::poi_needs(&opts.out), "the points-of-interest update")?;
    let mut run = Run { started, progress, cancel, last_report: Instant::now(), pois: Vec::new(), stats: Stats::default() };
    let mut stamps = Vec::new();
    for phase in Phase::ALL {
        run.report(phase, 0, None)?;
        let (input, info) = match &opts.sources {
            Sources::Online => source::open_url(&phase.url(), None),
            Sources::Dir(dir) => source::open_file(&dir.join(phase.file_name())),
        }
        .with_context(|| format!("couldn't get {}", phase.describe()))?;
        let read = match phase {
            Phase::Gec => read_gec(input, &info, &mut run),
            Phase::Edsm => read_edsm(input, &info, &mut run),
            Phase::Carriers => read_carriers(input, &info, &mut run),
            Phase::Codex => read_codex(input, &info, &mut run),
        };
        if run.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        read.with_context(|| format!("couldn't read {} ({})", phase.describe(), info.description))?;
        let bytes = info.bytes_read.load(Ordering::Relaxed);
        run.stats.bytes_downloaded += bytes;
        run.report(phase, bytes, info.total)?;
        stamps.push(SourceStamp { name: phase.describe().into(), url: phase.url(), last_modified: info.last_modified.clone() });
    }
    if run.pois.is_empty() {
        bail!("no points of interest found");
    }

    let Run { pois, mut stats, .. } = run;
    let mut counts = BTreeMap::new();
    for p in &pois {
        *counts.entry(p.category.clone()).or_insert(0) += 1;
    }
    stats.pois = pois.len() as u64;
    stats.by_category = counts.clone();
    let file = PoiFile {
        version: VERSION,
        built_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        sources: stamps,
        counts,
        pois,
    };

    let path = opts.out.join(FILE_NAME);
    let partial = opts.out.join(format!("{FILE_NAME}.partial"));
    std::fs::create_dir_all(&opts.out).with_context(|| format!("create {}", opts.out.display()))?;
    let written = write_file(&partial, &file).and_then(|bytes| {
        std::fs::rename(&partial, &path).with_context(|| format!("move {} into place", path.display()))?;
        Ok(bytes)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    stats.file_bytes = written?;
    stats.seconds = started.elapsed().as_secs_f64();
    Ok(stats)
}

fn write_file(path: &Path, file: &PoiFile) -> Result<u64> {
    let json = serde_json::to_vec(file)?;
    let mut out = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    out.write_all(&json)?;
    out.sync_all()?;
    Ok(json.len() as u64)
}

/// Reads a `poi.json`, refusing other files and other format versions.
pub fn read(bytes: &[u8]) -> Result<PoiFile> {
    #[derive(Deserialize)]
    struct Version {
        version: u32,
    }
    let v: Version = serde_json::from_slice(bytes).context("not a points-of-interest file")?;
    if v.version != VERSION {
        bail!("points-of-interest file version {}, expected {VERSION}", v.version);
    }
    Ok(serde_json::from_slice(bytes)?)
}

// --- Text helpers ----------------------------------------------------------------

/// Trimmed, or `None` when empty.
fn text(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Cuts at a word boundary near `SUMMARY_CHARS`, with an ellipsis.
fn summary(s: &str) -> Option<String> {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        return None;
    }
    if s.chars().count() <= SUMMARY_CHARS {
        return Some(s);
    }
    let cut: String = s.chars().take(SUMMARY_CHARS).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    Some(format!("{}…", cut.trim_end_matches([',', ';', '.', ' '])))
}

/// EDSM's notes: `+|+` separates fields, and some carry a literal `\n`.
fn edsm_notes(notes: &str) -> Option<String> {
    let parts: Vec<String> =
        notes.replace("\\n", " ").split("+|+").filter_map(|p| text(p).map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))).collect();
    summary(&parts.join("; "))
}

fn finite3(x: f64, y: f64, z: f64) -> Option<[f32; 3]> {
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some([x as f32, y as f32, z as f32])
}

// --- GEC ----------------------------------------------------------------------------

fn read_gec(mut input: source::Source, info: &source::SourceInfo, run: &mut Run) -> Result<()> {
    let mut bytes = Vec::with_capacity(info.total.unwrap_or(4 << 20) as usize);
    input.read_to_end(&mut bytes)?;
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&bytes).context("expected a JSON array")?;
    for entry in entries {
        match gec_poi(&entry) {
            Some((poi, known)) => {
                if !known {
                    *run.stats.unknown_types.entry(format!("GEC: {}", poi.kind)).or_insert(0) += 1;
                }
                run.pois.push(poi);
            }
            None => run.malformed(),
        }
    }
    Ok(())
}

/// One GEC entry, and whether its type was known.
fn gec_poi(v: &serde_json::Value) -> Option<(Poi, bool)> {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).and_then(text);
    let id = v.get("id")?.as_u64()?;
    let name = s("name")?;
    let c = v.get("coordinates")?.as_array()?;
    let pos = finite3(c.first()?.as_f64()?, c.get(1)?.as_f64()?, c.get(2)?.as_f64()?)?;
    // A few entries leave the main type empty and use the second one.
    let kind = s("type").or_else(|| s("type2")).unwrap_or_else(|| "Sights and Scenery".into());
    let category = gec_category(&kind);
    let id64 = v.get("id64").and_then(|x| x.as_u64()).filter(|&x| x > 0).map(|x| x.to_string());
    Some((
        Poi {
            id: format!("gec:{id}"),
            source: "gec".into(),
            category: category.unwrap_or(Category::Sights).id().into(),
            kind,
            name,
            system: s("galMapSearch"),
            id64,
            pos,
            region: s("region"),
            summary: s("summary").as_deref().and_then(summary),
            url: s("poiUrl").filter(|u| u.starts_with(EDASTRO)),
            weight: None,
        },
        category.is_some(),
    ))
}

// --- CSV helpers -------------------------------------------------------------------

/// Column positions by header name, so a reordered file still reads.
struct Columns(HashMap<String, usize>);

impl Columns {
    fn new(reader: &mut csv::Reader<impl Read>, required: &[&str]) -> Result<Self> {
        let headers = reader.byte_headers().context("no header row")?;
        let map: HashMap<String, usize> =
            headers.iter().enumerate().map(|(i, h)| (String::from_utf8_lossy(h).trim().to_string(), i)).collect();
        for name in required {
            if !map.contains_key(*name) {
                bail!("no \"{name}\" column: the file's format has changed");
            }
        }
        Ok(Self(map))
    }

    fn get<'r>(&self, row: &'r csv::ByteRecord, name: &str) -> &'r str {
        self.0.get(name).and_then(|&i| row.get(i)).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }

    fn pos(&self, row: &csv::ByteRecord, x: &str, y: &str, z: &str) -> Option<[f32; 3]> {
        let f = |name| self.get(row, name).trim().parse::<f64>().ok();
        finite3(f(x)?, f(y)?, f(z)?)
    }
}

fn csv_reader(input: source::Source) -> csv::Reader<BufReader<source::Source>> {
    csv::ReaderBuilder::new().flexible(true).from_reader(BufReader::with_capacity(1 << 20, input))
}

// --- EDSM Galactic Mapping ------------------------------------------------------------

fn read_edsm(input: source::Source, info: &source::SourceInfo, run: &mut Run) -> Result<()> {
    let mut reader = csv_reader(input);
    let cols = Columns::new(&mut reader, &["POI Type", "ID", "Name", "X", "Y", "Z", "Reference System", "Notes"])?;
    let mut row = csv::ByteRecord::new();
    while reader.read_byte_record(&mut row)? {
        run.tick(Phase::Edsm, info)?;
        let raw_type = cols.get(&row, "POI Type").trim();
        let (category, kind) = match edsm_type(raw_type) {
            EdsmType::Keep(c, k) => (c, k.to_string()),
            EdsmType::Skip => continue,
            EdsmType::Unknown => {
                if raw_type.is_empty() {
                    run.malformed();
                    continue;
                }
                *run.stats.unknown_types.entry(format!("EDSM: {raw_type}")).or_insert(0) += 1;
                (Category::Sights, raw_type.to_string())
            }
        };
        let (Some(name), Some(pos), Some(id)) = (text(cols.get(&row, "Name")), cols.pos(&row, "X", "Y", "Z"), text(cols.get(&row, "ID")))
        else {
            run.malformed();
            continue;
        };
        let notes = edsm_notes(cols.get(&row, "Notes"));
        let summary = match (category, notes) {
            (Category::Outpost, Some(n)) if raw_type == "DSSAcarrier" => Some(format!("Services: {n}")),
            (_, n) => n,
        };
        run.pois.push(Poi {
            id: format!("edsm:{raw_type}:{id}"),
            source: "edsm".into(),
            category: category.id().into(),
            kind,
            name,
            system: text(cols.get(&row, "Reference System")),
            id64: None,
            pos,
            region: None,
            summary,
            url: None,
            weight: None,
        });
    }
    Ok(())
}

// --- Fleet carriers -----------------------------------------------------------------------

struct Hub {
    name: String,
    id64: Option<String>,
    pos: [f32; 3],
    region: Option<String>,
    carriers: u32,
}

fn read_carriers(input: source::Source, info: &source::SourceInfo, run: &mut Run) -> Result<()> {
    let mut reader = csv_reader(input);
    let cols = Columns::new(&mut reader, &["LastUpdated", "LastSystem", "SystemAddress", "Coord_X", "Coord_Y", "Coord_Z"])?;
    // Every sighting first: the window ends at the newest one in the file.
    let mut seen: Vec<(i64, Hub)> = Vec::new();
    let mut row = csv::ByteRecord::new();
    while reader.read_byte_record(&mut row)? {
        run.tick(Phase::Carriers, info)?;
        let Some(system) = text(cols.get(&row, "LastSystem")) else { continue };
        let (Some(when), Some(pos)) = (unix_from_iso(cols.get(&row, "LastUpdated")), cols.pos(&row, "Coord_X", "Coord_Y", "Coord_Z"))
        else {
            run.malformed();
            continue;
        };
        let id64 = text(cols.get(&row, "SystemAddress")).filter(|s| s.parse::<u64>().is_ok_and(|v| v > 0));
        seen.push((when, Hub { name: system, id64, pos, region: text(cols.get(&row, "EstimatedRegion")), carriers: 0 }));
    }
    let Some(newest) = seen.iter().map(|s| s.0).max() else { return Ok(()) };
    let since = newest - CARRIER_WINDOW_DAYS * 86_400;
    let mut hubs: HashMap<String, Hub> = HashMap::new();
    for (when, sighting) in seen {
        if when < since {
            continue;
        }
        let key = sighting.id64.clone().unwrap_or_else(|| sighting.name.to_lowercase());
        hubs.entry(key).or_insert(sighting).carriers += 1;
    }
    let to = iso_from_unix(newest);
    let to = &to[..10];
    let mut hubs: Vec<Hub> = hubs.into_values().filter(|h| h.carriers >= CARRIER_HUB_MIN).collect();
    hubs.sort_by(|a, b| b.carriers.cmp(&a.carriers).then_with(|| a.name.cmp(&b.name)));
    for h in hubs {
        run.pois.push(Poi {
            id: format!("carriers:{}", h.id64.clone().unwrap_or_else(|| h.name.to_lowercase())),
            source: "carriers".into(),
            category: Category::CarrierHub.id().into(),
            kind: "Carrier hub".into(),
            summary: Some(format!("{} fleet carriers seen here in the {CARRIER_WINDOW_DAYS} days to {to}.", h.carriers)),
            name: h.name.clone(),
            system: Some(h.name),
            id64: h.id64,
            pos: h.pos,
            region: h.region,
            url: None,
            weight: Some(h.carriers),
        });
    }
    Ok(())
}

// --- Codex: Guardian and Thargoid sites --------------------------------------------------

struct Site {
    category: Category,
    name: String,
    id64: Option<String>,
    pos: [f32; 3],
    region: Option<String>,
    /// The strongest entry's rank and the kind it gives the site.
    rank: u8,
    kind: &'static str,
    /// Entries reported here, deduplicated, in the order first seen.
    parts: Vec<&'static str>,
}

const CODEX_ID: &str = "Codex ID";
const CODEX_ID64: &str = "System Address / ID64";

fn read_codex(input: source::Source, info: &source::SourceInfo, run: &mut Run) -> Result<()> {
    let mut reader = csv_reader(input);
    let cols = Columns::new(&mut reader, &[CODEX_ID, "System", "X", "Y", "Z", CODEX_ID64])?;
    let id_col = cols.0[CODEX_ID];
    let mut sites: HashMap<(Category, String), Site> = HashMap::new();
    let mut order: Vec<(Category, String)> = Vec::new();
    let mut row = csv::ByteRecord::new();
    let mut n: u64 = 0;
    while reader.read_byte_record(&mut row)? {
        n += 1;
        if n.is_multiple_of(4096) {
            run.tick(Phase::Codex, info)?;
        }
        // Most of the 5M rows are biology and stars: check the id's bytes first.
        let Some(id) = row.get(id_col) else { continue };
        if !(id.starts_with(b"codex_ent_guardian_") || id.starts_with(b"codex_ent_thargoid_") || id.starts_with(b"codex_ent_tg_")) {
            continue;
        }
        let Some((category, label, rank, kind)) = std::str::from_utf8(id).ok().and_then(codex_entry) else { continue };
        let (Some(system), Some(pos)) = (text(cols.get(&row, "System")), cols.pos(&row, "X", "Y", "Z")) else {
            run.malformed();
            continue;
        };
        let id64 = text(cols.get(&row, CODEX_ID64)).filter(|s| s.parse::<u64>().is_ok_and(|v| v > 0));
        let key = (category, id64.clone().unwrap_or_else(|| system.to_lowercase()));
        let site = sites.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            Site { category, name: system, id64, pos, region: text(cols.get(&row, "Region")), rank: 0, kind, parts: Vec::new() }
        });
        if rank > site.rank || site.parts.is_empty() {
            site.rank = rank;
            site.kind = kind;
        }
        if !site.parts.contains(&label) {
            site.parts.push(label);
        }
    }
    for key in order {
        let Some(site) = sites.remove(&key) else { continue };
        let source_id = site.id64.clone().unwrap_or_else(|| site.name.to_lowercase());
        run.pois.push(Poi {
            id: format!("codex:{}:{source_id}", site.category.id()),
            source: "codex".into(),
            category: site.category.id().into(),
            kind: site.kind.into(),
            summary: Some(format!("Reported here: {}.", site.parts.join(", "))),
            name: site.name.clone(),
            system: Some(site.name),
            id64: site.id64,
            pos: site.pos,
            region: site.region,
            url: None,
            weight: None,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from the live files (2026-11-09).
    const GEC: &str = r#"[
{"id":10,"type":"Sights and Scenery","type2":"","name":"The Ammonia Lyceum","region":"Empyrean Straits","galMapSearch":"Athaip WR-H d11-7577","coordinates":[334.969,-55.4375,23014.3],"summary":"Body 3 b a is a nested moon close to its parent atmospheric body, which features green glowing reflections at certain times of the day.","id64":67199985,"poiUrl":"https://edastro.com/gec/view/10","source":"GEC"},
{"id":77,"type":"Mystery and Xenology","type2":"","name":"Guardian Ruins - IC 4673 nebula","region":"Sagittarius-Carina Arm","galMapSearch":"","coordinates":[-2131.31,-55.25,7993.38],"summary":"","id64":0,"poiUrl":"https://example.com/elsewhere","source":"GEC"},
{"id":91,"type":"","type2":"Organic","name":"Untyped","region":"","galMapSearch":"Sol","coordinates":[0,0,0],"summary":"x","id64":10477373803,"poiUrl":null},
{"id":92,"type":"Something New","name":"New kind","coordinates":[1,2,3]},
{"id":93,"name":"No coordinates","type":"Nebulae"}
]"#;

    const EDSM: &str = "POI Type,ID,Name,X,Y,Z,Reference System,Notes
megaship,256949,Orunmilla,-23.5,10,40.25,Duamta,
DSSAcarrier,DSSA1,DSSA Sleeper Service [TFF-34Z] #01.01.001 (Galactic Centre),2123.66,2833.06,29718.6,Oorb Broae DF-A e6,\"Repair, Armoury, Outfitting, UC\"
CCLNSP,CCL1,Phooe Auf HV-I c24-0,-15057.2,1832.88,26889.7,Phooe Auf HV-I c24-0,+|+Feature: E01-Type Anomaly+|+Discovered by: Qildail
planetaryNebula,CGN II-1,River of Woe,16276.2,619.719,54315.8,Syrivu GG-X e1-0,
nebula,CGN III-1,Colonia Nebula,-9534,-907,19787,,
carrier,FC1,COFFEE'S Bar [Q7K-T1N],-7876.03,-740.312,16177.6,Boeph IG-C c14-103,DockingPermit: ALL
guardian,GUARDIAN1,Guardian Sites,-995,0,13175,,
trit_hwyORANGE,1,Tritium Highway,-10157.4,-918.625,25734.5,BYOOMAI XO-C C27-299,Body: A6\\n
Sights and Scenery,14,Dueling Rings,8814.06,-85,-5725.47,Qiefaa EB-U d4-20,
brandNewType,7,Odd One,1,2,3,Somewhere,
minorPOI,23,Broken,abc,2,3,Somewhere,
";

    const CARRIERS: &str = "Callsign,Name,Owner,LastUpdated,LastMoved,LastSystem,SystemAddress,Coord_X,Coord_Y,Coord_Z,SolDistance,EstimatedRegion,LocationHistory,DockingsEDDN,Services
0000,SSL SEVEN LORDS,,\"2026-01-26 13:57:21\",\"2025-11-14 21:10:12\",,,,,,0.00,,0,0,
";

    fn carrier_rows() -> String {
        let mut s = CARRIERS.to_string();
        // Ten carriers in Deciat this month, nine in Khun, and Deciat's old ones.
        for i in 0..10 {
            s += &format!("D{i:03},,,\"2026-11-0{} 10:00:00\",,Deciat,6681123623626,122.625,-0.8125,-47.28125,135.6,Inner Orion Spur,1,1,\n", i % 9 + 1);
        }
        for i in 0..9 {
            s += &format!("K{i:03},,,\"2026-11-08 10:00:00\",,Khun,3657130755,-171.59375,19.96875,-56.96875,182.5,Inner Orion Spur,1,1,\n");
        }
        for i in 0..5 {
            s += &format!("O{i:03},,,\"2026-09-01 10:00:00\",,Deciat,6681123623626,122.625,-0.8125,-47.28125,135.6,Inner Orion Spur,1,1,\n");
        }
        s += "BAD,,,not a date,,Deciat,6681123623626,122.625,-0.8125,-47.28125,135.6,,1,1,\n";
        s
    }

    const CODEX: &str = "Codex Entry,Codex ID,First Reported,Odyssey,Region,System,X,Y,Z,Main Star Type,\"System Address / ID64\"
,codex_ent_fonticulus_02,,,Inner Orion Spur,10 Canum Venaticorum,-9.375,55.4375,-7,\"F (White) Star\",79196866923
Guardian Codex,codex_ent_guardian_data_logs,,0,Outer Orion Spur,Synuefe XR-H d11-102,357.34375,-49.34375,-74.75,\"M (Red dwarf) Star\",3515254557027
Guardian Structure,codex_ent_guardian_structure,,0,Outer Orion Spur,Synuefe XR-H d11-102,357.34375,-49.34375,-74.75,\"M (Red dwarf) Star\",3515254557027
Guardian Pylon,codex_ent_guardian_pylon,,0,Outer Orion Spur,Synuefe XR-H d11-102,357.34375,-49.34375,-74.75,\"M (Red dwarf) Star\",3515254557027
Guardian Ruins,codex_ent_guardian_ruins,,0,Norma Expanse,Blaa Hypai EU-D c13-0,1244.25,-752.188,12307,\"K (Yellow-Orange) Star\",85932285042
Thargoid-Raumpocken-Matrix,codex_ent_thargoid_barnacle_matrix_submerged,,1,Inner Orion Spur,HIP 20567,-17.5,-70.5,-133.25,\"K (Yellow-Orange) Star\",908619975354
Common Thargoid Barnacle,codex_ent_thargoid_barnacle_01,,0,Inner Orion Spur,HIP 20567,-17.5,-70.5,-133.25,\"K (Yellow-Orange) Star\",908619975354
Thargoid Pod,codex_ent_tg_pod,,0,Inner Orion Spur,Pleiades Sector AB-W b2-4,-81,-149.5,-343.375,\"M (Red dwarf) Star\",9466778822857
Broken,codex_ent_guardian_ruins,,0,Somewhere,Nowhere,x,y,z,,
";

    fn sources(dir: &Path) -> PathBuf {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(Phase::Gec.file_name()), GEC).unwrap();
        std::fs::write(src.join(Phase::Edsm.file_name()), EDSM).unwrap();
        std::fs::write(src.join(Phase::Carriers.file_name()), carrier_rows()).unwrap();
        std::fs::write(src.join(Phase::Codex.file_name()), CODEX).unwrap();
        src
    }

    fn by_id<'a>(f: &'a PoiFile, id: &str) -> &'a Poi {
        f.pois.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("no {id}"))
    }

    #[test]
    fn builds_every_category_from_the_four_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("data");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(FILE_NAME), b"old").unwrap();
        let opts = Options { sources: Sources::Dir(sources(dir.path())), out: out.clone() };
        let mut phases = Vec::new();
        let stats = build(&opts, &mut |p| phases.push(p.phase), Arc::new(AtomicBool::new(false))).unwrap();
        phases.dedup();
        assert_eq!(phases, Phase::ALL);
        assert!(!out.join(format!("{FILE_NAME}.partial")).exists());

        let bytes = std::fs::read(out.join(FILE_NAME)).unwrap();
        assert_eq!(bytes.len() as u64, stats.file_bytes);
        let f = read(&bytes).unwrap();
        assert_eq!(f.sources.len(), 4);
        assert_eq!(f.pois.len() as u64, stats.pois);
        // GEC: 4 readable entries (one has no coordinates); EDSM: 6 kept;
        // carriers: 1 hub; codex: 4 sites (one row is broken).
        assert_eq!(stats.pois, 4 + 6 + 1 + 4);
        assert_eq!(stats.malformed, 1 + 1 + 1 + 1);
        assert_eq!(
            stats.unknown_types.keys().collect::<Vec<_>>(),
            ["EDSM: brandNewType", "GEC: Something New"],
            "unknown types are kept and counted"
        );
        assert_eq!(f.counts.values().sum::<u64>(), stats.pois);

        let lyceum = by_id(&f, "gec:10");
        assert_eq!((lyceum.category.as_str(), lyceum.kind.as_str()), ("sights", "Sights and Scenery"));
        assert_eq!(lyceum.system.as_deref(), Some("Athaip WR-H d11-7577"));
        assert_eq!(lyceum.id64.as_deref(), Some("67199985"));
        assert_eq!(lyceum.url.as_deref(), Some("https://edastro.com/gec/view/10"));
        assert_eq!(lyceum.pos, [334.969, -55.4375, 23014.3]);
        let ruins = by_id(&f, "gec:77");
        assert_eq!(ruins.category, "xeno");
        assert_eq!((ruins.system.as_deref(), ruins.id64.as_deref(), ruins.summary.as_deref()), (None, None, None));
        assert_eq!(ruins.url, None, "links only to EDAstro");
        assert_eq!((by_id(&f, "gec:91").kind.as_str(), by_id(&f, "gec:91").category.as_str()), ("Organic", "planet"));

        let dssa = by_id(&f, "edsm:DSSAcarrier:DSSA1");
        assert_eq!((dssa.category.as_str(), dssa.kind.as_str()), ("outpost", "DSSA carrier"));
        assert_eq!(dssa.summary.as_deref(), Some("Services: Repair, Armoury, Outfitting, UC"));
        let anomaly = by_id(&f, "edsm:CCLNSP:CCL1");
        assert_eq!(anomaly.summary.as_deref(), Some("Feature: E01-Type Anomaly; Discovered by: Qildail"));
        assert_eq!(by_id(&f, "edsm:planetaryNebula:CGN II-1").category, "planetaryNebula");
        assert_eq!(by_id(&f, "edsm:nebula:CGN III-1").system, None, "a nebula with no reference system");
        assert_eq!(by_id(&f, "edsm:megaship:256949").kind, "Megaship");
        assert_eq!(by_id(&f, "edsm:brandNewType:7").category, "sights");
        for skipped in ["edsm:carrier:FC1", "edsm:guardian:GUARDIAN1", "edsm:trit_hwyORANGE:1", "edsm:Sights and Scenery:14"] {
            assert!(f.pois.iter().all(|p| p.id != skipped), "{skipped} is skipped");
        }

        let hub = by_id(&f, "carriers:6681123623626");
        assert_eq!((hub.name.as_str(), hub.weight, hub.category.as_str()), ("Deciat", Some(10), "carrierHub"));
        assert_eq!(hub.summary.as_deref(), Some("10 fleet carriers seen here in the 30 days to 2026-11-09."));
        assert!(f.pois.iter().all(|p| p.name != "Khun"), "9 carriers isn't a hub");

        let structure = by_id(&f, "codex:guardian:3515254557027");
        assert_eq!(structure.kind, "Guardian Structure", "the strongest entry names the site");
        assert_eq!(structure.summary.as_deref(), Some("Reported here: Guardian Codex, Guardian Structure, Guardian Pylon."));
        assert_eq!(structure.region.as_deref(), Some("Outer Orion Spur"));
        assert_eq!(by_id(&f, "codex:guardian:85932285042").kind, "Guardian Ruins");
        let spire = by_id(&f, "codex:thargoid:908619975354");
        assert_eq!(spire.kind, "Thargoid Spire site");
        assert_eq!(spire.summary.as_deref(), Some("Reported here: Barnacle Matrix, Common Thargoid Barnacle."), "labels aren't the localised ones");
        assert_eq!(by_id(&f, "codex:thargoid:9466778822857").kind, "Thargoid surface site");
    }

    #[test]
    fn cancelled_or_failed_updates_keep_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("data");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(FILE_NAME), b"old").unwrap();
        let src = sources(dir.path());
        let opts = Options { sources: Sources::Dir(src.clone()), out: out.clone() };

        let err = build(&opts, &mut |_| {}, Arc::new(AtomicBool::new(true))).unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
        assert_eq!(std::fs::read(out.join(FILE_NAME)).unwrap(), b"old");

        std::fs::write(src.join(Phase::Codex.file_name()), "Something,Else\n1,2\n").unwrap();
        let err = build(&opts, &mut |_| {}, Arc::new(AtomicBool::new(false))).unwrap_err();
        assert!(format!("{err:#}").contains("\"Codex ID\" column"), "{err:#}");
        assert_eq!(std::fs::read(out.join(FILE_NAME)).unwrap(), b"old");

        std::fs::remove_file(src.join(Phase::Gec.file_name())).unwrap();
        let err = build(&opts, &mut |_| {}, Arc::new(AtomicBool::new(false))).unwrap_err();
        assert!(err.to_string().contains("Galactic Exploration Catalog"), "{err}");
        assert_eq!(std::fs::read(out.join(FILE_NAME)).unwrap(), b"old");
    }

    #[test]
    fn summaries_are_cut_at_a_word() {
        assert_eq!(summary("  a \n b  "), Some("a b".into()));
        assert_eq!(summary(""), None);
        let long = "word ".repeat(100);
        let s = summary(&long).unwrap();
        assert!(s.ends_with("word…") && s.chars().count() <= SUMMARY_CHARS + 1, "{s}");
        assert_eq!(edsm_notes("+|+Body: A6\\n+|++|+"), Some("Body: A6".into()));
    }

    #[test]
    fn rejects_foreign_files() {
        assert!(read(b"old").is_err());
        assert!(read(br#"{"version":9,"pois":[]}"#).is_err(), "unknown version");
        assert!(read(br#"{"version":1,"builtAt":0,"sources":[],"counts":{},"pois":[]}"#).is_ok());
    }
}
