//! Commander travel history from the player journal.
//!
//! The game writes `Journal.<timestamp>.<part>.log` files (one JSON object
//! per line) to `%USERPROFILE%\Saved Games\Frontier Developments\Elite Dangerous`.
//! Only the events that carry a position (`FSDJump`, `CarrierJump`, `Location`),
//! the ones that say which commander is playing (`Commander`, `LoadGame`),
//! `Docked` and `ApproachSettlement` are read. The arrival, docking and
//! approach events also describe the system and station (factions,
//! Powerplay, services...); the latest line of each is kept as a
//! [`Snapshot`] and parsed on demand (see [`crate::details::journal`]).

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Vec3;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Jump {
    pub timestamp: String,
    /// `FSDJump`, `CarrierJump` or `Location`.
    pub kind: String,
    pub system: String,
    #[serde(serialize_with = "crate::serialize_id64")]
    pub id64: Option<u64>,
    pub pos: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Commander { fid: String, name: String },
    /// An arrival or login position; `docked_at` is the MarketID of the
    /// station the ship is docked at, when the event says so.
    Jump { jump: Jump, docked_at: Option<u64> },
    Docked { timestamp: String, id64: u64, market_id: u64 },
    /// Approaching a planetary port or settlement.
    ApproachSettlement { timestamp: String, id64: u64, market_id: u64 },
    /// The current ship's modules; the line is kept whole and parsed on
    /// demand (see [`crate::ship`]).
    Loadout { timestamp: String },
}

/// The latest journal line describing a system or station.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Snapshot {
    pub timestamp: String,
    pub line: String,
}

/// What the journals last said about one station. Each kind of event
/// carries different fields (only `Docked` has landing pads, only
/// `ApproachSettlement` the body), so the latest of each is kept.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StationSnapshots {
    /// The system it was in at the newest snapshot (fleet carriers move).
    pub system: u64,
    pub docked: Option<Snapshot>,
    /// Docked at login (`Location`) or aboard a carrier as it jumped (`CarrierJump`).
    pub docked_arrival: Option<Snapshot>,
    pub approach: Option<Snapshot>,
}

impl StationSnapshots {
    fn newest(&self) -> Option<&str> {
        [&self.docked, &self.docked_arrival, &self.approach]
            .into_iter()
            .flatten()
            .map(|s| s.timestamp.as_str())
            .max()
    }
}

#[derive(Debug, Clone, Copy)]
enum StationEvent {
    Docked,
    DockedArrival,
    Approach,
}

/// Latest snapshots seen in the journals, for every commander.
#[derive(Debug, Default)]
pub struct Snapshots {
    /// Latest `FSDJump` / `CarrierJump` / `Location` per system id64.
    pub systems: HashMap<u64, Snapshot>,
    /// Per station MarketID.
    pub stations: HashMap<u64, StationSnapshots>,
}

impl Snapshots {
    /// Stations whose newest snapshot was in system `id64`.
    pub fn stations_in(&self, id64: u64) -> impl Iterator<Item = &StationSnapshots> {
        self.stations.values().filter(move |s| s.system == id64)
    }

    fn record_system(&mut self, id64: u64, timestamp: &str, line: &str) {
        let newer = self.systems.get(&id64).is_none_or(|s| s.timestamp.as_str() <= timestamp);
        if newer {
            self.systems.insert(id64, Snapshot { timestamp: timestamp.into(), line: line.into() });
        }
    }

    fn record_station(&mut self, kind: StationEvent, market_id: u64, id64: u64, timestamp: &str, line: &str) {
        let station = self.stations.entry(market_id).or_default();
        if station.newest().is_none_or(|t| t <= timestamp) {
            station.system = id64;
        }
        let slot = match kind {
            StationEvent::Docked => &mut station.docked,
            StationEvent::DockedArrival => &mut station.docked_arrival,
            StationEvent::Approach => &mut station.approach,
        };
        if slot.as_ref().is_none_or(|s| s.timestamp.as_str() <= timestamp) {
            *slot = Some(Snapshot { timestamp: timestamp.into(), line: line.into() });
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommanderTrack {
    pub fid: String,
    pub name: String,
    pub jumps: Vec<Jump>,
}

/// Travel history for every commander found in the journals, keyed by FID.
#[derive(Debug, Default, Clone, Serialize)]
pub struct History {
    pub commanders: BTreeMap<String, CommanderTrack>,
}

impl History {
    /// The commander with the most recent position event.
    pub fn latest(&self) -> Option<&CommanderTrack> {
        self.commanders
            .values()
            .filter_map(|c| c.jumps.last().map(|j| (j.timestamp.as_str(), c)))
            .max_by(|a, b| a.0.cmp(b.0))
            .map(|(_, c)| c)
    }
}

#[derive(Deserialize)]
struct RawEvent {
    event: String,
    timestamp: Option<String>,
    #[serde(rename = "StarSystem")]
    star_system: Option<String>,
    #[serde(rename = "SystemAddress")]
    system_address: Option<u64>,
    #[serde(rename = "StarPos")]
    star_pos: Option<[f64; 3]>,
    #[serde(rename = "FID")]
    fid: Option<String>,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Commander")]
    commander: Option<String>,
    #[serde(rename = "MarketID")]
    market_id: Option<u64>,
    #[serde(rename = "Docked")]
    docked: Option<bool>,
}

const INTERESTING: [&str; 8] = [
    "\"FSDJump\"",
    "\"CarrierJump\"",
    "\"Location\"",
    "\"Commander\"",
    "\"LoadGame\"",
    "\"Docked\"",
    "\"ApproachSettlement\"",
    "\"Loadout\"",
];

/// Parses one journal line. Returns `None` for events we don't use or for
/// malformed lines (the game occasionally leaves a half-written last line).
pub fn parse_line(line: &str) -> Option<Event> {
    if !INTERESTING.iter().any(|tag| line.contains(tag)) {
        return None;
    }
    let raw: RawEvent = serde_json::from_str(line).ok()?;
    match raw.event.as_str() {
        "FSDJump" | "CarrierJump" | "Location" => {
            let p = raw.star_pos?;
            let docked_at = raw.market_id.filter(|_| raw.docked == Some(true));
            let jump = Jump {
                timestamp: raw.timestamp.unwrap_or_default(),
                kind: raw.event,
                system: raw.star_system?,
                id64: raw.system_address,
                pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            };
            Some(Event::Jump { jump, docked_at })
        }
        "Docked" => Some(Event::Docked {
            timestamp: raw.timestamp.unwrap_or_default(),
            id64: raw.system_address?,
            market_id: raw.market_id?,
        }),
        "ApproachSettlement" => Some(Event::ApproachSettlement {
            timestamp: raw.timestamp.unwrap_or_default(),
            id64: raw.system_address?,
            market_id: raw.market_id?,
        }),
        "Commander" => Some(Event::Commander { fid: raw.fid?, name: raw.name? }),
        "LoadGame" => Some(Event::Commander { fid: raw.fid?, name: raw.commander? }),
        "Loadout" => Some(Event::Loadout { timestamp: raw.timestamp.unwrap_or_default() }),
        _ => None,
    }
}

/// Default journal folder on Windows. `ED_JOURNAL_DIR` overrides it.
pub fn default_journal_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ED_JOURNAL_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")?;
    Some(Path::new(&home).join("Saved Games").join("Frontier Developments").join("Elite Dangerous"))
}

/// Journal files in chronological order. Names are either the old
/// `Journal.YYMMDDhhmmss.NN.log` or the current `Journal.YYYY-MM-DDThhmmss.NN.log`,
/// which don't sort together by name ("Journal.22..." comes after
/// "Journal.2026-..."), so they're ordered by the time in the name.
pub fn journal_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?;
            (name.starts_with("Journal.") && name.ends_with(".log")).then(|| (sort_key(name), p.clone()))
        })
        .collect();
    files.sort();
    Ok(files.into_iter().map(|(_, p)| p).collect())
}

/// `YYYY-MM-DDThhmmss.NN` for either journal name format.
fn sort_key(name: &str) -> String {
    let Some(rest) = name.strip_prefix("Journal.").and_then(|n| n.strip_suffix(".log")) else {
        return name.to_string();
    };
    let (stamp, part) = rest.split_once('.').unwrap_or((rest, ""));
    let b = stamp.as_bytes();
    let old_format = b.len() == 12 && b.iter().all(u8::is_ascii_digit);
    if old_format {
        format!("20{}-{}-{}T{}.{part}", &stamp[0..2], &stamp[2..4], &stamp[4..6], &stamp[6..12])
    } else {
        format!("{stamp}.{part}")
    }
}

/// Incremental reader that remembers which commander is active and how far
/// into the current file it has read, so a watcher can pick up new lines.
#[derive(Debug, Default)]
pub struct Reader {
    pub history: History,
    pub snapshots: Snapshots,
    /// The newest `Loadout` line per commander FID, for the route plotter.
    pub loadouts: HashMap<String, Snapshot>,
    current_fid: Option<String>,
    current_file: Option<PathBuf>,
    offset: u64,
}

impl Reader {
    /// Reads every journal in `dir` from the start.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut reader = Self::default();
        for file in journal_files(dir)? {
            reader.read_from(&file)?;
        }
        Ok(reader)
    }

    /// Reads lines appended since the last call, switching to the newest
    /// journal in `dir` if the game has started a new one. Returns the new jumps.
    pub fn poll(&mut self, dir: &Path) -> Result<Vec<(String, Jump)>> {
        let mut new = Vec::new();
        let files = journal_files(dir)?;
        let start = match &self.current_file {
            Some(cur) => files.iter().position(|f| f == cur).unwrap_or(0),
            None => 0,
        };
        for file in &files[start..] {
            new.extend(self.read_from(file)?);
        }
        Ok(new)
    }

    fn read_from(&mut self, path: &Path) -> Result<Vec<(String, Jump)>> {
        if self.current_file.as_deref() != Some(path) {
            self.current_file = Some(path.to_path_buf());
            self.offset = 0;
        }
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < self.offset {
            self.offset = 0; // truncated or replaced
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file);
        let mut buf = Vec::new();
        let mut new = Vec::new();
        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            // Stop before a partial last line; it will be re-read once complete.
            if n == 0 || !buf.ends_with(b"\n") {
                break;
            }
            self.offset += n as u64;
            // A line the game left with broken UTF-8 (say, a crash mid-write)
            // is decoded lossily rather than failing the whole read.
            let text = String::from_utf8_lossy(&buf);
            let line = text.trim_end();
            match parse_line(line) {
                Some(Event::Commander { fid, name }) => {
                    let track = self.history.commanders.entry(fid.clone()).or_default();
                    track.fid = fid.clone();
                    track.name = name;
                    self.current_fid = Some(fid);
                }
                Some(Event::Jump { jump, docked_at }) => {
                    if let Some(id64) = jump.id64 {
                        self.snapshots.record_system(id64, &jump.timestamp, line);
                        if let Some(market_id) = docked_at {
                            self.snapshots.record_station(StationEvent::DockedArrival, market_id, id64, &jump.timestamp, line);
                        }
                    }
                    let fid = self.current_fid.clone().unwrap_or_else(|| "unknown".into());
                    let track = self.history.commanders.entry(fid.clone()).or_default();
                    track.fid = fid.clone();
                    // `Location` is written on every login; skip it when it
                    // repeats the system we are already in.
                    let repeat = jump.kind == "Location"
                        && track.jumps.last().is_some_and(|last| last.system == jump.system);
                    if !repeat {
                        track.jumps.push(jump.clone());
                        new.push((fid, jump));
                    }
                }
                Some(Event::Docked { timestamp, id64, market_id }) => {
                    self.snapshots.record_station(StationEvent::Docked, market_id, id64, &timestamp, line);
                }
                Some(Event::ApproachSettlement { timestamp, id64, market_id }) => {
                    self.snapshots.record_station(StationEvent::Approach, market_id, id64, &timestamp, line);
                }
                Some(Event::Loadout { timestamp }) => {
                    let fid = self.current_fid.clone().unwrap_or_else(|| "unknown".into());
                    let slot = self.loadouts.entry(fid).or_default();
                    if slot.timestamp.as_str() <= timestamp.as_str() {
                        *slot = Snapshot { timestamp, line: line.into() };
                    }
                }
                None => {}
            }
        }
        Ok(new)
    }
}

/// Where commander data comes from. Journals are the only implementation for
/// now; a Companion API (CAPI) source can implement this once a Frontier
/// client ID is approved.
pub trait CommanderSource {
    fn history(&mut self) -> Result<&History>;
    /// New jumps since the previous call, tagged with the commander FID.
    fn poll(&mut self) -> Result<Vec<(String, Jump)>>;
}

pub struct JournalSource {
    dir: PathBuf,
    reader: Option<Reader>,
}

impl JournalSource {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, reader: None }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// System and station snapshots from every journal read so far.
    pub fn snapshots(&mut self) -> Result<&Snapshots> {
        self.history()?;
        Ok(&self.reader.as_ref().unwrap().snapshots)
    }

    /// The newest `Loadout` line for `fid`, or for any commander when `None`.
    pub fn loadout(&mut self, fid: Option<&str>) -> Result<Option<&Snapshot>> {
        self.history()?;
        let loadouts = &self.reader.as_ref().unwrap().loadouts;
        Ok(match fid {
            Some(fid) => loadouts.get(fid),
            None => loadouts.values().max_by(|a, b| a.timestamp.cmp(&b.timestamp)),
        })
    }
}

impl CommanderSource for JournalSource {
    fn history(&mut self) -> Result<&History> {
        if self.reader.is_none() {
            self.reader = Some(Reader::load_dir(&self.dir)?);
        }
        Ok(&self.reader.as_ref().unwrap().history)
    }

    fn poll(&mut self) -> Result<Vec<(String, Jump)>> {
        match &mut self.reader {
            Some(r) => r.poll(&self.dir),
            None => {
                self.history()?;
                Ok(Vec::new())
            }
        }
    }
}

#[cfg(test)]
// Coordinates are exact multiples of 1/32 ly, which f32 holds exactly;
// clippy only sees more digits than f32 prints.
#[allow(clippy::excessive_precision)]
mod tests {
    use super::*;
    use std::io::Write;

    const COMMANDER: &str = r#"{ "timestamp":"2026-10-01T18:00:00Z", "event":"Commander", "FID":"F1234", "Name":"Test Pilot" }"#;
    const LOCATION: &str = r#"{ "timestamp":"2026-10-01T18:00:05Z", "event":"Location", "Docked":true, "StationName":"Abraham Lincoln", "MarketID":128016640, "StarSystem":"Sol", "SystemAddress":10477373803, "StarPos":[0.00000,0.00000,0.00000] }"#;
    const FSDJUMP: &str = r#"{ "timestamp":"2026-10-01T18:10:00Z", "event":"FSDJump", "Taxi":false, "StarSystem":"Sagittarius A*", "SystemAddress":20578934, "StarPos":[25.21875,-20.90625,25899.96875], "JumpDist":42.1 }"#;
    const DOCKED: &str = r#"{ "timestamp":"2026-10-01T18:20:00Z", "event":"Docked", "StationName":"Test Port", "StationType":"Coriolis", "StarSystem":"Sagittarius A*", "SystemAddress":20578934, "MarketID":3228100352 }"#;
    const APPROACH: &str = r#"{ "timestamp":"2026-10-01T18:15:00Z", "event":"ApproachSettlement", "Name":"Test Port", "MarketID":3228100352, "SystemAddress":20578934, "BodyID":3, "BodyName":"Sagittarius A* 1" }"#;
    const CARRIER: &str = r#"{ "timestamp":"2026-10-01T19:00:00Z", "event":"CarrierJump", "Docked":true, "StarSystem":"Colonia", "SystemAddress":3238296097059, "StarPos":[-9530.50000,-910.28125,19808.12500] }"#;

    #[test]
    fn parses_position_events() {
        let Some(Event::Jump { jump: j, docked_at: None }) = parse_line(FSDJUMP) else { panic!("no jump") };
        assert_eq!(j.system, "Sagittarius A*");
        assert_eq!(j.id64, Some(20578934));
        assert_eq!(j.pos, [25.21875, -20.90625, 25899.96875]);
        assert_eq!(j.kind, "FSDJump");

        let Some(Event::Jump { jump: j, .. }) = parse_line(CARRIER) else { panic!("no carrier jump") };
        assert_eq!(j.system, "Colonia");
        assert_eq!(j.kind, "CarrierJump");

        let Some(Event::Jump { docked_at, .. }) = parse_line(LOCATION) else { panic!("no location") };
        assert_eq!(docked_at, Some(128016640));
        assert_eq!(
            parse_line(DOCKED),
            Some(Event::Docked { timestamp: "2026-10-01T18:20:00Z".into(), id64: 20578934, market_id: 3228100352 })
        );
        assert_eq!(
            parse_line(APPROACH),
            Some(Event::ApproachSettlement {
                timestamp: "2026-10-01T18:15:00Z".into(),
                id64: 20578934,
                market_id: 3228100352
            })
        );

        assert_eq!(
            parse_line(COMMANDER),
            Some(Event::Commander { fid: "F1234".into(), name: "Test Pilot".into() })
        );
    }

    #[test]
    fn ignores_other_events_and_garbage() {
        assert_eq!(parse_line(r#"{ "timestamp":"x", "event":"Music", "MusicTrack":"Exploration" }"#), None);
        assert_eq!(parse_line(r#"{ "timestamp":"x", "event":"FSDJump", "StarSys"#), None);
    }

    #[test]
    fn reader_tracks_commander_and_tails_new_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Journal.2026-10-01T180000.01.log");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "{COMMANDER}\n{LOCATION}\n{LOCATION}").unwrap();
        // A partial line must not be consumed yet.
        write!(f, "{}", &FSDJUMP[..40]).unwrap();
        f.flush().unwrap();

        let mut source = JournalSource::new(dir.path().to_path_buf());
        let history = source.history().unwrap();
        let cmdr = &history.commanders["F1234"];
        assert_eq!(cmdr.name, "Test Pilot");
        assert_eq!(cmdr.jumps.len(), 1, "repeated Location should be collapsed");

        writeln!(f, "{}", &FSDJUMP[40..]).unwrap();
        f.flush().unwrap();
        let new = source.poll().unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].0, "F1234");
        assert_eq!(new[0].1.system, "Sagittarius A*");

        // A new journal file is picked up by the next poll.
        let mut g = File::create(dir.path().join("Journal.2026-10-01T190000.01.log")).unwrap();
        writeln!(g, "{COMMANDER}\n{CARRIER}").unwrap();
        g.flush().unwrap();
        let new = source.poll().unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].1.system, "Colonia");
        assert_eq!(source.history().unwrap().latest().unwrap().jumps.len(), 3);
    }

    #[test]
    fn keeps_the_latest_system_and_station_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = File::create(dir.path().join("Journal.2026-10-01T180000.01.log")).unwrap();
        let older = FSDJUMP.replace("18:10:00", "17:00:00");
        // Out of order on purpose: the newer line must win.
        writeln!(f, "{COMMANDER}\n{LOCATION}\n{FSDJUMP}\n{older}\n{APPROACH}\n{DOCKED}").unwrap();
        f.flush().unwrap();

        let mut source = JournalSource::new(dir.path().to_path_buf());
        let snaps = source.snapshots().unwrap();
        assert_eq!(snaps.systems[&20578934].line, FSDJUMP);
        assert_eq!(snaps.systems[&10477373803].line, LOCATION);
        let stations: Vec<_> = snaps.stations_in(20578934).collect();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].docked.as_ref().unwrap().line, DOCKED);
        assert_eq!(stations[0].approach.as_ref().unwrap().line, APPROACH);
        // Docked at login: the Location line doubles as a station snapshot.
        let sol = snaps.stations_in(10477373803).next().unwrap();
        assert_eq!(sol.docked_arrival.as_ref().unwrap().line, LOCATION);
        assert!(sol.docked.is_none());
    }

    #[test]
    fn invalid_utf8_lines_do_not_stop_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = File::create(dir.path().join("Journal.2026-10-01T180000.01.log")).unwrap();
        writeln!(f, "{COMMANDER}").unwrap();
        f.write_all(b"{ \"event\":\"Music\", \"MusicTrack\":\"\xd0\x9f\xd1\" }\n").unwrap();
        writeln!(f, "{FSDJUMP}").unwrap();
        f.flush().unwrap();
        let mut source = JournalSource::new(dir.path().to_path_buf());
        assert_eq!(source.history().unwrap().commanders["F1234"].jumps.len(), 1);
    }

    #[test]
    fn a_moved_carrier_is_listed_where_it_was_seen_last() {
        let mut snaps = Snapshots::default();
        snaps.record_station(StationEvent::Docked, 7, 1, "2026-10-01T10:00:00Z", "docked in 1");
        snaps.record_station(StationEvent::DockedArrival, 7, 2, "2026-10-02T10:00:00Z", "jumped to 2");
        // An older line read later (file order) must not move it back.
        snaps.record_station(StationEvent::Docked, 7, 1, "2026-09-01T10:00:00Z", "older");
        assert_eq!(snaps.stations_in(1).count(), 0);
        let c = snaps.stations_in(2).next().unwrap();
        assert_eq!(c.docked.as_ref().unwrap().line, "docked in 1");
        assert_eq!(c.docked_arrival.as_ref().unwrap().line, "jumped to 2");
    }

    #[test]
    fn journal_files_sort_by_time_across_name_formats() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "Journal.2026-11-08T111224.01.log",
            "Journal.220130155905.01.log",
            "Journal.2022-08-23T222932.01.log",
            "Journal.211105145112.02.log",
            "Journal.211105145112.01.log",
            "Journal.log",
            "notes.txt",
        ] {
            File::create(dir.path().join(name)).unwrap();
        }
        let names: Vec<String> = journal_files(dir.path())
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "Journal.211105145112.01.log",
                "Journal.211105145112.02.log",
                "Journal.220130155905.01.log",
                "Journal.2022-08-23T222932.01.log",
                "Journal.2026-11-08T111224.01.log",
                "Journal.log",
            ]
        );
    }
}
