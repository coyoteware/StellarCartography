//! Populated systems for the map's overlay layers (population, allegiance,
//! government, economy, security, Powerplay and minor factions), from
//! Spansh's `galaxy_populated.json.gz` dump.
//!
//! Spansh's dump is the only one with the controlling power of every
//! system: EDSM's `powerPlay.json.gz` lists each power present without
//! saying which one controls, and misses systems such as Achenar. The dump
//! is big (about 4.7 GB compressed, mostly bodies and station markets), but
//! only a few top-level fields per system are kept, so the result is a file
//! of a few megabytes that the web view reads in one go.
//!
//! `populated.bin` layout, little-endian:
//!
//! ```text
//!  0  magic      b"SCPOP\0\0\0"
//!  8  u32        format version (VERSION)
//! 12  u32        header length in bytes
//! 16  header     UTF-8 JSON (`Header`): system and faction names, string
//!                tables, and where each column is
//!     padding    to a multiple of 8
//!     columns    each at data start + `offset`, 8-byte aligned
//! ```
//!
//! Systems are sorted by population, smallest first, so the most populous
//! draw last (on top). Categorical columns are `u8` indexes into the
//! header's tables, with `NONE_U8` for unknown; faction columns are `u32`
//! indexes into `factions`, with `NONE_U32` for none.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{source, space, Input};

pub const SPANSH_POPULATED_URL: &str = "https://downloads.spansh.co.uk/galaxy_populated.json.gz";
/// The output file, in the data folder next to the catalogue.
pub const FILE_NAME: &str = "populated.bin";
pub const MAGIC: &[u8; 8] = b"SCPOP\0\0\0";
pub const VERSION: u32 = 1;
pub const NONE_U8: u8 = u8::MAX;
pub const NONE_U32: u32 = u32::MAX;

pub struct Options {
    pub input: Input,
    /// Data folder; the file is written as `populated.bin` inside it.
    pub out: PathBuf,
    /// Stop after reading this many systems.
    pub limit: Option<u64>,
}

impl Options {
    /// Stream the latest Spansh dump into `out`.
    pub fn spansh(out: impl Into<PathBuf>) -> Self {
        Self { input: Input::Url(SPANSH_POPULATED_URL.into()), out: out.into(), limit: None }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Compressed bytes of the dump consumed so far.
    pub bytes_read: u64,
    pub bytes_total: Option<u64>,
    pub systems: u64,
    pub seconds: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub source: String,
    pub source_last_modified: Option<String>,
    pub newest_update: Option<String>,
    pub systems: u64,
    pub presences: u64,
    pub malformed: u64,
    pub compressed_bytes: u64,
    pub file_bytes: u64,
    pub seconds: f64,
}

/// The JSON header. The web view reads the same fields (app/src/renderer/populated-data.ts).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub version: u32,
    pub count: u32,
    /// Entries in the faction presence columns.
    pub presences: u32,
    pub source: String,
    pub source_last_modified: Option<String>,
    /// The newest `date` among the systems: how current the dump is.
    pub newest_update: Option<String>,
    /// Unix seconds.
    pub built_at: u64,
    /// System names, in file order.
    pub names: Vec<String>,
    pub tables: Tables,
    pub factions: Vec<String>,
    pub columns: BTreeMap<String, Column>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tables {
    pub allegiance: Vec<String>,
    pub government: Vec<String>,
    /// Shared by the `economy` and `secondEconomy` columns.
    pub economy: Vec<String>,
    pub security: Vec<String>,
    /// Shared by `power` (the controlling power) and the `powers` bitmask.
    pub power: Vec<String>,
    pub power_state: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    /// Bytes from the start of the data section.
    pub offset: u64,
    /// "u8", "u16", "u32", "u64", "f32" or "f64".
    #[serde(rename = "type")]
    pub kind: String,
    /// Number of elements.
    pub len: u64,
}

// --- Reading the dump --------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    id64: u64,
    name: String,
    coords: Coords,
    population: Option<u64>,
    allegiance: Option<String>,
    government: Option<String>,
    primary_economy: Option<String>,
    secondary_economy: Option<String>,
    security: Option<String>,
    controlling_faction: Option<NamedFaction>,
    #[serde(default)]
    factions: Vec<FactionPresence>,
    controlling_power: Option<String>,
    #[serde(default)]
    powers: Vec<String>,
    power_state: Option<String>,
    date: Option<String>,
}

#[derive(Deserialize)]
struct Coords {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Deserialize)]
struct NamedFaction {
    name: String,
}

#[derive(Deserialize)]
struct FactionPresence {
    name: String,
    influence: Option<f64>,
}

/// One system as kept from the dump.
struct System {
    id64: u64,
    name: String,
    pos: [f32; 3],
    population: u64,
    allegiance: Option<String>,
    government: Option<String>,
    economy: Option<String>,
    second_economy: Option<String>,
    security: Option<String>,
    controlling_faction: Option<String>,
    factions: Vec<(String, f64)>,
    power: Option<String>,
    powers: Vec<String>,
    power_state: Option<String>,
    date: Option<String>,
}

/// Missing, empty and "None" all mean "not known / not applicable".
fn value(s: Option<String>) -> Option<String> {
    let s = s?;
    let t = s.trim();
    (!t.is_empty() && !t.eq_ignore_ascii_case("none")).then(|| t.to_string())
}

fn parse_line(line: &str) -> Option<System> {
    let r: Record = serde_json::from_str(line).ok()?;
    Some(System {
        id64: r.id64,
        name: r.name,
        pos: [r.coords.x as f32, r.coords.y as f32, r.coords.z as f32],
        population: r.population.unwrap_or(0),
        allegiance: value(r.allegiance),
        government: value(r.government),
        economy: value(r.primary_economy),
        second_economy: value(r.secondary_economy),
        security: value(r.security),
        controlling_faction: value(r.controlling_faction.map(|f| f.name)),
        factions: r.factions.into_iter().filter(|f| !f.name.trim().is_empty()).map(|f| (f.name, f.influence.unwrap_or(0.0))).collect(),
        power: value(r.controlling_power),
        powers: r.powers.into_iter().filter_map(|p| value(Some(p))).collect(),
        power_state: value(r.power_state),
        date: r.date,
    })
}

const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// Lines per batch handed to a parser thread. Lines average ~200 KB.
const BATCH_LINES: usize = 32;

/// Streams the dump and writes `populated.bin` into `opts.out`, replacing any
/// earlier copy only once the new one is complete. `progress` is called a few
/// times a second; setting `cancel` stops with an error and leaves the old
/// file alone.
pub fn build(opts: &Options, progress: &mut dyn FnMut(&Progress), cancel: Arc<AtomicBool>) -> Result<Stats> {
    let started = Instant::now();
    let (input, info) = match &opts.input {
        Input::File(path) => source::open_file(path)?,
        Input::Url(url) => source::open_url(url, None)?,
    };
    space::check(&opts.out, space::populated_needs(&opts.out), "the populated-systems update")?;
    let report = |systems: u64, progress: &mut dyn FnMut(&Progress)| {
        progress(&Progress {
            bytes_read: info.bytes_read.load(Ordering::Relaxed),
            bytes_total: info.total,
            systems,
            seconds: started.elapsed().as_secs_f64(),
        })
    };

    // Decompressing is one thread's work; parsing the ~200 KB lines is
    // spread over the others.
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 6));
    let (tx, rx) = mpsc::sync_channel::<Vec<String>>(workers * 2);
    let rx = Arc::new(Mutex::new(rx));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let rx = rx.clone();
            std::thread::spawn(move || {
                let (mut systems, mut malformed) = (Vec::new(), 0u64);
                loop {
                    let batch = match rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => break,
                    };
                    let Ok(batch) = batch else { break };
                    for line in batch {
                        match parse_line(&line) {
                            Some(s) => systems.push(s),
                            None => malformed += 1,
                        }
                    }
                }
                (systems, malformed)
            })
        })
        .collect();

    let mut reader = BufReader::with_capacity(8 << 20, flate2::read::MultiGzDecoder::new(input));
    let mut read: u64 = 0;
    let mut batch = Vec::with_capacity(BATCH_LINES);
    let mut last_report = Instant::now();
    let mut line = String::new();
    let read_result: Result<()> = (|| {
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let trimmed = line.trim_end().trim_end_matches(',');
            if !trimmed.trim_start().starts_with('{') {
                continue;
            }
            read += 1;
            batch.push(trimmed.to_string());
            if batch.len() == BATCH_LINES {
                tx.send(std::mem::replace(&mut batch, Vec::with_capacity(BATCH_LINES)))
                    .map_err(|_| anyhow::anyhow!("parser threads stopped"))?;
            }
            if last_report.elapsed() > PROGRESS_EVERY {
                last_report = Instant::now();
                report(read, progress);
                if cancel.load(Ordering::Relaxed) {
                    bail!("cancelled");
                }
            }
            if opts.limit.is_some_and(|l| read >= l) {
                break;
            }
        }
        Ok(())
    })();
    if read_result.is_ok() && !batch.is_empty() {
        let _ = tx.send(batch);
    }
    drop(tx);
    let mut systems = Vec::with_capacity(read as usize);
    let mut malformed = 0;
    for h in handles {
        let (s, m) = h.join().map_err(|_| anyhow::anyhow!("parser thread panicked"))?;
        systems.extend(s);
        malformed += m;
    }
    read_result?;
    if cancel.load(Ordering::Relaxed) {
        bail!("cancelled");
    }
    if systems.is_empty() {
        bail!("no systems read from {}", info.description);
    }
    report(read, progress);

    let mut stats = Stats {
        source: info.description.clone(),
        source_last_modified: info.last_modified.clone(),
        malformed,
        compressed_bytes: info.bytes_read.load(Ordering::Relaxed),
        ..Default::default()
    };
    let path = opts.out.join(FILE_NAME);
    let partial = opts.out.join(format!("{FILE_NAME}.partial"));
    std::fs::create_dir_all(&opts.out).with_context(|| format!("create {}", opts.out.display()))?;
    let written = write_file(&partial, systems, &mut stats);
    let written = written.and_then(|bytes| {
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

// --- Writing the file --------------------------------------------------------

/// Strings to small integers, in order of first appearance.
#[derive(Default)]
struct Interner {
    ids: HashMap<String, u32>,
    list: Vec<String>,
}

impl Interner {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.ids.get(s) {
            return id;
        }
        let id = self.list.len() as u32;
        self.ids.insert(s.to_string(), id);
        self.list.push(s.to_string());
        id
    }

    /// A `u8` column value: NONE_U8 for none, and for anything past the
    /// table's 255th entry (never seen in practice).
    fn small(&mut self, s: Option<&str>) -> u8 {
        match s {
            Some(s) if self.ids.contains_key(s) || self.list.len() < NONE_U8 as usize => self.id(s) as u8,
            _ => NONE_U8,
        }
    }
}

/// Appends columns, each 8-byte aligned, and records where they went.
struct Columns {
    data: Vec<u8>,
    index: BTreeMap<String, Column>,
}

impl Columns {
    fn put(&mut self, name: &str, kind: &str, len: usize, bytes: impl IntoIterator<Item = u8>) {
        while !self.data.len().is_multiple_of(8) {
            self.data.push(0);
        }
        let offset = self.data.len() as u64;
        self.data.extend(bytes);
        self.index.insert(name.into(), Column { offset, kind: kind.into(), len: len as u64 });
    }
}

fn write_file(path: &Path, mut systems: Vec<System>, stats: &mut Stats) -> Result<u64> {
    // Most populous last, so they draw on top; id64 makes the order stable.
    systems.sort_by_key(|s| (s.population, s.id64));
    let n = systems.len();

    let (mut allegiance, mut government, mut economy, mut security, mut power, mut power_state) =
        (Interner::default(), Interner::default(), Interner::default(), Interner::default(), Interner::default(), Interner::default());
    let mut factions = Interner::default();
    let mut cols = Columns { data: Vec::new(), index: BTreeMap::new() };

    cols.put("id64", "u64", n, systems.iter().flat_map(|s| s.id64.to_le_bytes()));
    cols.put("pos", "f32", n * 3, systems.iter().flat_map(|s| s.pos).flat_map(f32::to_le_bytes));
    cols.put("population", "f64", n, systems.iter().flat_map(|s| (s.population as f64).to_le_bytes()));
    let u8_column = |interner: &mut Interner, get: &dyn Fn(&System) -> Option<&str>| -> Vec<u8> {
        systems.iter().map(|s| interner.small(get(s))).collect()
    };
    let col = u8_column(&mut allegiance, &|s| s.allegiance.as_deref());
    cols.put("allegiance", "u8", n, col);
    let col = u8_column(&mut government, &|s| s.government.as_deref());
    cols.put("government", "u8", n, col);
    let col = u8_column(&mut economy, &|s| s.economy.as_deref());
    cols.put("economy", "u8", n, col);
    let col = u8_column(&mut economy, &|s| s.second_economy.as_deref());
    cols.put("secondEconomy", "u8", n, col);
    let col = u8_column(&mut security, &|s| s.security.as_deref());
    cols.put("security", "u8", n, col);
    let col = u8_column(&mut power, &|s| s.power.as_deref());
    cols.put("power", "u8", n, col);
    let col = u8_column(&mut power_state, &|s| s.power_state.as_deref());
    cols.put("powerState", "u8", n, col);

    // Every power present, as a bitmask over the power table.
    let masks: Vec<u32> = systems
        .iter()
        .map(|s| {
            s.powers.iter().fold(0u32, |mask, p| {
                let id = power.id(p);
                if id < 32 {
                    mask | (1 << id)
                } else {
                    mask
                }
            })
        })
        .collect();
    cols.put("powers", "u32", n, masks.iter().flat_map(|m| m.to_le_bytes()));

    let controlling: Vec<u32> =
        systems.iter().map(|s| s.controlling_faction.as_deref().map_or(NONE_U32, |f| factions.id(f))).collect();
    cols.put("controllingFaction", "u32", n, controlling.iter().flat_map(|v| v.to_le_bytes()));
    // Faction presences, CSR style: system i's factions are entries
    // factionStart[i] .. factionStart[i + 1].
    let mut starts = Vec::with_capacity(n + 1);
    let (mut ids, mut influence) = (Vec::new(), Vec::new());
    for s in &systems {
        starts.push(ids.len() as u32);
        for (name, inf) in &s.factions {
            ids.push(factions.id(name));
            influence.push((inf.clamp(0.0, 1.0) * 10_000.0).round() as u16);
        }
    }
    starts.push(ids.len() as u32);
    cols.put("factionStart", "u32", n + 1, starts.iter().flat_map(|v| v.to_le_bytes()));
    cols.put("factionId", "u32", ids.len(), ids.iter().flat_map(|v| v.to_le_bytes()));
    // Influence in units of 0.01%.
    cols.put("factionInfluence", "u16", influence.len(), influence.iter().flat_map(|v| v.to_le_bytes()));

    stats.systems = n as u64;
    stats.presences = ids.len() as u64;
    stats.newest_update = systems.iter().filter_map(|s| s.date.as_deref()).max().map(str::to_string);
    let header = Header {
        version: VERSION,
        count: n as u32,
        presences: ids.len() as u32,
        source: stats.source.clone(),
        source_last_modified: stats.source_last_modified.clone(),
        newest_update: stats.newest_update.clone(),
        built_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        names: systems.into_iter().map(|s| s.name).collect(),
        tables: Tables {
            allegiance: allegiance.list,
            government: government.list,
            economy: economy.list,
            security: security.list,
            power: power.list,
            power_state: power_state.list,
        },
        factions: factions.list,
        columns: cols.index,
    };
    let json = serde_json::to_vec(&header)?;

    let mut out = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?);
    out.write_all(MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    out.write_all(&(json.len() as u32).to_le_bytes())?;
    out.write_all(&json)?;
    let pad = data_start(json.len()) - 16 - json.len();
    out.write_all(&vec![0; pad])?;
    out.write_all(&cols.data)?;
    out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok((data_start(json.len()) + cols.data.len()) as u64)
}

/// Where the columns start, after the preamble and the padded header.
pub fn data_start(header_len: usize) -> usize {
    (16 + header_len).div_ceil(8) * 8
}

/// Reads the header of a `populated.bin`.
pub fn read_header(bytes: &[u8]) -> Result<Header> {
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        bail!("not a populated-systems file");
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into()?);
    if version != VERSION {
        bail!("populated-systems file version {version}, expected {VERSION}");
    }
    let len = u32::from_le_bytes(bytes[12..16].try_into()?) as usize;
    let json = bytes.get(16..16 + len).context("truncated header")?;
    Ok(serde_json::from_slice(json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_dump(path: &Path, lines: &[&str]) {
        let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast());
        writeln!(gz, "[").unwrap();
        for (i, l) in lines.iter().enumerate() {
            writeln!(gz, "\t{l}{}", if i + 1 < lines.len() { "," } else { "" }).unwrap();
        }
        writeln!(gz, "]").unwrap();
        gz.finish().unwrap();
    }

    // Trimmed from the live dump (2026-11-09); bodies and stations shortened.
    const SOL: &str = r#"{"id64":10477373803,"name":"Sol","coords":{"x":0.0,"y":0.0,"z":0.0},"allegiance":"Federation","government":"Democracy","primaryEconomy":"Refinery","secondaryEconomy":"Service","security":"High","population":18320926115,"bodyCount":40,"controllingFaction":{"name":"Mother Gaia","state":"Expansion","government":"Democracy","allegiance":"Federation","activeStates":[{"state":"Expansion"}]},"factions":[{"name":"Federal Congress","allegiance":"Federation","government":"Democracy","influence":0.176471,"state":"None"},{"name":"Mother Gaia","allegiance":"Federation","government":"Democracy","influence":0.3001,"state":"Expansion","activeStates":[{"state":"Civil Liberty"}]}],"controllingPower":"Jerome Archer","powers":["Aisling Duval","Edmund Mahon","Jerome Archer"],"powerState":"Stronghold","powerStateControlProgress":0.604611,"date":"2026-11-08T20:36:20Z","bodies":[{"name":"Sol","type":"Star","subType":"G (White-Yellow) Star"}],"stations":[{"name":"Abraham Lincoln","market":{"commodities":[{"name":"Gold","buyPrice":1}]}}]}"#;
    const CONTESTED: &str = r#"{"id64":5068732573097,"name":"LHS 3558","coords":{"x":-30.5,"y":12.25,"z":7.125},"allegiance":"Independent","government":"Anarchy","primaryEconomy":"Extraction","secondaryEconomy":"None","security":"Anarchy","population":5254,"controllingFaction":{"name":"LHS 3558 Gang"},"factions":[{"name":"LHS 3558 Gang","influence":1.0}],"powers":["Aisling Duval","Jerome Archer"],"powerState":"Unoccupied","powerConflictProgress":[{"power":"Jerome Archer","progress":0.53}],"date":"2026-11-07T18:30:45Z","bodies":[],"stations":[]}"#;
    const BARE: &str = r#"{"id64":42,"name":"Quiet","coords":{"x":1,"y":2,"z":3},"allegiance":"None","government":"None","primaryEconomy":"None","security":"Low","population":0,"factions":[],"date":"2025-01-01T00:00:00Z"}"#;

    fn column<'a>(bytes: &'a [u8], h: &Header, name: &str) -> &'a [u8] {
        let c = &h.columns[name];
        let width = match c.kind.as_str() {
            "u8" => 1,
            "u16" => 2,
            "u32" | "f32" => 4,
            _ => 8,
        };
        let start = data_start(u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize) + c.offset as usize;
        assert_eq!(start % 8, 0, "{name} is aligned");
        &bytes[start..start + width * c.len as usize]
    }

    fn u32s(b: &[u8]) -> Vec<u32> {
        b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
    }

    #[test]
    fn keeps_the_fields_the_layers_need() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("dump.json.gz");
        write_dump(&dump, &[SOL, "{not json", CONTESTED, BARE]);
        let out = dir.path().join("data");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(FILE_NAME), b"old").unwrap();

        let opts = Options { input: Input::File(dump), out: out.clone(), limit: None };
        let stats = build(&opts, &mut |_| {}, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!((stats.systems, stats.malformed, stats.presences), (3, 1, 3));
        assert_eq!(stats.newest_update.as_deref(), Some("2026-11-08T20:36:20Z"));
        assert!(!out.join(format!("{FILE_NAME}.partial")).exists());

        let bytes = std::fs::read(out.join(FILE_NAME)).unwrap();
        assert_eq!(bytes.len() as u64, stats.file_bytes);
        let h = read_header(&bytes).unwrap();
        // Smallest population first.
        assert_eq!(h.names, ["Quiet", "LHS 3558", "Sol"]);
        let id64: Vec<u64> = column(&bytes, &h, "id64").as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect();
        assert_eq!(id64, [42, 5068732573097, 10477373803]);
        let pos: Vec<f32> = column(&bytes, &h, "pos").as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        assert_eq!(&pos[3..6], &[-30.5, 12.25, 7.125]);
        let population = f64::from_le_bytes(column(&bytes, &h, "population")[16..24].try_into().unwrap());
        assert_eq!(population, 18320926115.0);

        // "None" is no value at all.
        assert_eq!(column(&bytes, &h, "allegiance")[0], NONE_U8);
        assert_eq!(h.tables.allegiance, ["Independent", "Federation"]);
        assert_eq!(column(&bytes, &h, "economy"), &[NONE_U8, 0, 1]);
        assert_eq!(column(&bytes, &h, "secondEconomy"), &[NONE_U8, NONE_U8, 2]);
        assert_eq!(h.tables.economy, ["Extraction", "Refinery", "Service"]);
        assert_eq!(h.tables.security, ["Low", "Anarchy", "High"]);

        let power = column(&bytes, &h, "power");
        assert_eq!(power[0], NONE_U8);
        assert_eq!(power[1], NONE_U8, "an unoccupied system has no controlling power");
        assert_eq!(h.tables.power[power[2] as usize], "Jerome Archer");
        let states: Vec<&str> = column(&bytes, &h, "powerState")
            .iter()
            .map(|&i| h.tables.power_state.get(i as usize).map_or("-", String::as_str))
            .collect();
        assert_eq!(states, ["-", "Unoccupied", "Stronghold"]);
        let masks = u32s(column(&bytes, &h, "powers"));
        let names = |mask: u32| -> Vec<&str> {
            (0..32).filter(|b| mask & (1 << b) != 0).map(|b| h.tables.power[b].as_str()).collect()
        };
        assert_eq!(masks[0], 0);
        assert_eq!(names(masks[1]), ["Jerome Archer", "Aisling Duval"]);
        assert_eq!(names(masks[2]).len(), 3);

        let starts = u32s(column(&bytes, &h, "factionStart"));
        assert_eq!(starts, [0, 0, 1, 3]);
        let ids = u32s(column(&bytes, &h, "factionId"));
        let controlling = u32s(column(&bytes, &h, "controllingFaction"));
        assert_eq!(controlling[0], NONE_U32);
        assert_eq!(h.factions[controlling[2] as usize], "Mother Gaia");
        assert_eq!(h.factions[ids[2] as usize], "Mother Gaia");
        let influence: Vec<u16> =
            column(&bytes, &h, "factionInfluence").as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        assert_eq!(influence, [10_000, 1765, 3001]);
    }

    #[test]
    fn cancelled_or_empty_builds_keep_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("data");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(FILE_NAME), b"old").unwrap();

        let dump = dir.path().join("dump.json.gz");
        write_dump(&dump, &[SOL, CONTESTED]);
        let opts = Options { input: Input::File(dump.clone()), out: out.clone(), limit: None };
        assert!(build(&opts, &mut |_| {}, Arc::new(AtomicBool::new(true))).is_err());
        assert_eq!(std::fs::read(out.join(FILE_NAME)).unwrap(), b"old");

        write_dump(&dump, &["<html>", "{broken"]);
        let err = build(&opts, &mut |_| {}, Arc::new(AtomicBool::new(false))).unwrap_err();
        assert!(err.to_string().contains("no systems"), "{err}");
        assert_eq!(std::fs::read(out.join(FILE_NAME)).unwrap(), b"old");
    }

    #[test]
    fn limit_stops_early() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("dump.json.gz");
        write_dump(&dump, &[SOL, CONTESTED, BARE]);
        let opts = Options { input: Input::File(dump), out: dir.path().to_path_buf(), limit: Some(2) };
        assert_eq!(build(&opts, &mut |_| {}, Arc::new(AtomicBool::new(false))).unwrap().systems, 2);
    }

    #[test]
    fn rejects_foreign_files() {
        assert!(read_header(b"old").is_err());
        assert!(read_header(b"SCPOP\0\0\0\x09\0\0\0\0\0\0\0").is_err(), "unknown version");
    }
}
