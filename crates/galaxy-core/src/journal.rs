//! Commander travel history from the player journal.
//!
//! The game writes `Journal.<timestamp>.<part>.log` files (one JSON object
//! per line) to `%USERPROFILE%\Saved Games\Frontier Developments\Elite Dangerous`.
//! Only the events that carry a position (`FSDJump`, `CarrierJump`, `Location`),
//! the ones that say which commander is playing (`Commander`, `LoadGame`),
//! `Docked` and `ApproachSettlement` are read, and the owner's fleet-carrier
//! events (`CarrierBuy`, `CarrierStats`, `CarrierLocation`,
//! `CarrierJumpRequest`, `CarrierJumpCancelled`). The arrival, docking and
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
    /// Names seen on the friends list (`Friends`) or in a wing (`WingAdd`,
    /// `WingJoin`), for suggesting commanders to follow. `status` is the
    /// `Friends` event's (`Online`, `Added`, `Lost`...); wing events have none.
    FriendsSeen { timestamp: String, names: Vec<String>, status: Option<String> },
    /// One of the owner's events about their own fleet carrier (`id` is its
    /// CarrierID, which is also its MarketID). Squadron carriers are left out.
    Carrier { timestamp: String, id: u64, update: CarrierUpdate },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CarrierUpdate {
    /// `CarrierBuy` (callsign, and where it was bought) or `CarrierStats` (name and callsign).
    Identity { name: Option<String>, callsign: Option<String>, at: Option<(String, u64)> },
    /// `CarrierLocation`, written at login.
    At { system: String, id64: u64 },
    /// `CarrierJumpRequest`: it jumps to `system` at `departure`.
    JumpBooked { system: String, id64: u64, departure: String },
    /// `CarrierJumpCancelled`.
    JumpCancelled,
}

/// The commander's own fleet carrier, as the owner's events describe it.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Carrier {
    /// Its CarrierID, which is also its MarketID.
    pub id: u64,
    pub name: Option<String>,
    pub callsign: Option<String>,
    /// Where it was last seen.
    pub at: Option<CarrierPlace>,
    /// A jump booked since: from `departure` on it's at the destination.
    pub jump: Option<CarrierJump>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CarrierPlace {
    pub timestamp: String,
    pub system: String,
    #[serde(serialize_with = "crate::serialize_id64")]
    pub id64: Option<u64>,
    /// From the event when it has one (`CarrierJump`, `Location`); the app looks the rest up.
    pub pos: Option<Vec3>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CarrierJump {
    /// When it was booked.
    pub timestamp: String,
    /// When it jumps.
    pub departure: String,
    pub to: CarrierPlace,
}

impl Carrier {
    fn apply(&mut self, timestamp: &str, update: CarrierUpdate) {
        let place = |system: String, id64: u64| CarrierPlace { timestamp: timestamp.into(), system, id64: Some(id64), pos: None };
        match update {
            CarrierUpdate::Identity { name, callsign, at } => {
                if name.is_some() {
                    self.name = name;
                }
                if callsign.is_some() {
                    self.callsign = callsign;
                }
                if let Some((system, id64)) = at {
                    self.arrive(place(system, id64));
                }
            }
            CarrierUpdate::At { system, id64 } => self.arrive(place(system, id64)),
            CarrierUpdate::JumpBooked { system, id64, departure } => {
                let to = CarrierPlace { timestamp: departure.clone(), system, id64: Some(id64), pos: None };
                self.jump = Some(CarrierJump { timestamp: timestamp.into(), departure, to });
            }
            CarrierUpdate::JumpCancelled => self.jump = None,
        }
    }

    /// Seen at `place`: a jump due by then has happened (or been called off).
    fn arrive(&mut self, place: CarrierPlace) {
        if self.jump.as_ref().is_some_and(|j| j.departure <= place.timestamp) {
            self.jump = None;
        }
        self.at = Some(place);
    }
}

/// The latest `Friends`/wing sighting of one name.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FriendSighting {
    pub timestamp: String,
    /// The last `Friends` status; `None` when only seen in a wing.
    pub status: Option<String>,
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
    /// Their own fleet carrier, when the journals mention one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carrier: Option<Carrier>,
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
    #[serde(rename = "Status")]
    status: Option<String>,
    /// `WingJoin`'s wing members: names, or objects with a `Name`, depending
    /// on the game version.
    #[serde(rename = "Others")]
    others: Option<Vec<serde_json::Value>>,
    #[serde(rename = "CarrierID")]
    carrier_id: Option<u64>,
    #[serde(rename = "CarrierType")]
    carrier_type: Option<String>,
    #[serde(rename = "Callsign")]
    callsign: Option<String>,
    /// `CarrierJumpRequest`'s destination.
    #[serde(rename = "SystemName")]
    system_name: Option<String>,
    #[serde(rename = "DepartureTime")]
    departure_time: Option<String>,
    /// `CarrierBuy`'s system.
    #[serde(rename = "Location")]
    location: Option<String>,
}

const INTERESTING: [&str; 16] = [
    "\"FSDJump\"",
    "\"CarrierJump\"",
    "\"Location\"",
    "\"Commander\"",
    "\"LoadGame\"",
    "\"Docked\"",
    "\"ApproachSettlement\"",
    "\"Loadout\"",
    "\"Friends\"",
    "\"WingAdd\"",
    "\"WingJoin\"",
    "\"CarrierBuy\"",
    "\"CarrierStats\"",
    "\"CarrierLocation\"",
    "\"CarrierJumpRequest\"",
    "\"CarrierJumpCancelled\"",
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
        "Friends" => Some(Event::FriendsSeen {
            timestamp: raw.timestamp.unwrap_or_default(),
            names: vec![raw.name?],
            status: raw.status,
        }),
        "WingAdd" => Some(Event::FriendsSeen {
            timestamp: raw.timestamp.unwrap_or_default(),
            names: vec![raw.name?],
            status: None,
        }),
        "WingJoin" => {
            let names: Vec<String> = raw
                .others?
                .into_iter()
                .filter_map(|v| match v {
                    serde_json::Value::String(s) => Some(s),
                    serde_json::Value::Object(o) => o.get("Name").and_then(|n| n.as_str()).map(String::from),
                    _ => None,
                })
                .collect();
            (!names.is_empty()).then(|| Event::FriendsSeen {
                timestamp: raw.timestamp.unwrap_or_default(),
                names,
                status: None,
            })
        }
        "CarrierBuy" | "CarrierStats" | "CarrierLocation" | "CarrierJumpRequest" | "CarrierJumpCancelled" => {
            // Squadron carriers write these too; older journals don't say which.
            if raw.carrier_type.as_deref().is_some_and(|t| t != "FleetCarrier") {
                return None;
            }
            let update = match raw.event.as_str() {
                "CarrierBuy" => CarrierUpdate::Identity { name: None, callsign: raw.callsign, at: raw.location.zip(raw.system_address) },
                "CarrierStats" => CarrierUpdate::Identity { name: raw.name, callsign: raw.callsign, at: None },
                "CarrierLocation" => CarrierUpdate::At { system: raw.star_system?, id64: raw.system_address? },
                "CarrierJumpRequest" => CarrierUpdate::JumpBooked {
                    system: raw.system_name?,
                    id64: raw.system_address?,
                    departure: raw.departure_time?,
                },
                _ => CarrierUpdate::JumpCancelled,
            };
            Some(Event::Carrier { timestamp: raw.timestamp.unwrap_or_default(), id: raw.carrier_id?, update })
        }
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
    /// Latest friends-list or wing sighting per name, across all commanders.
    pub friends: BTreeMap<String, FriendSighting>,
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
                    // Aboard their own carrier as it jumped, or logged in docked on it.
                    if let Some(carrier) = track.carrier.as_mut().filter(|c| docked_at == Some(c.id)) {
                        carrier.arrive(CarrierPlace {
                            timestamp: jump.timestamp.clone(),
                            system: jump.system.clone(),
                            id64: jump.id64,
                            pos: Some(jump.pos),
                        });
                    }
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
                Some(Event::FriendsSeen { timestamp, names, status }) => {
                    for name in names {
                        let slot = self.friends.entry(name).or_default();
                        if slot.timestamp.as_str() <= timestamp.as_str() {
                            // A wing sighting keeps the last Friends status:
                            // it says they're in a wing, not off the list.
                            *slot = FriendSighting { timestamp: timestamp.clone(), status: status.clone().or(slot.status.take()) };
                        }
                    }
                }
                Some(Event::Carrier { timestamp, id, update }) => {
                    // Only the owner writes these, so they belong to whoever is playing.
                    let Some(fid) = self.current_fid.clone() else { continue };
                    let track = self.history.commanders.entry(fid.clone()).or_default();
                    track.fid = fid;
                    let carrier = track.carrier.get_or_insert_with(Carrier::default);
                    if carrier.id != id {
                        // A new carrier (or the first): start afresh.
                        *carrier = Carrier { id, ..Carrier::default() };
                    }
                    carrier.apply(&timestamp, update);
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

    /// Names seen on the friends list or in a wing, newest sighting first.
    /// Leaves out names whose last `Friends` status says they're not a friend
    /// (`Lost`, `Declined`, `Requested`), and the journals' own commanders.
    pub fn friend_names(&mut self) -> Result<Vec<String>> {
        self.history()?;
        let reader = self.reader.as_ref().unwrap();
        let own: Vec<String> = reader.history.commanders.values().map(|c| c.name.to_lowercase()).collect();
        let mut seen: Vec<(&String, &FriendSighting)> = reader
            .friends
            .iter()
            .filter(|(name, s)| {
                let gone = matches!(s.status.as_deref(), Some("Lost" | "Declined" | "Requested"));
                !gone && !own.contains(&name.to_lowercase())
            })
            .collect();
        seen.sort_by(|a, b| b.1.timestamp.cmp(&a.1.timestamp).then(a.0.cmp(b.0)));
        Ok(seen.into_iter().map(|(name, _)| name.clone()).collect())
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

    const CARRIER_BUY: &str = r#"{ "timestamp":"2026-10-01T18:30:00Z", "event":"CarrierBuy", "CarrierType":"FleetCarrier", "CarrierID":3708098048, "BoughtAtMarket":3228100352, "Location":"Sagittarius A*", "SystemAddress":20578934, "Price":5000000000, "Variant":"CarrierDockB", "Callsign":"W3H-NVZ" }"#;
    const CARRIER_STATS: &str = r#"{ "timestamp":"2026-10-01T18:31:00Z", "event":"CarrierStats", "CarrierID":3708098048, "CarrierType":"FleetCarrier", "Callsign":"W3H-NVZ", "Name":"TEST CARRIER", "DockingAccess":"all", "FuelLevel":745, "SpaceUsage":{ "TotalCapacity":25000 }, "Crew":[ { "CrewRole":"Captain", "Activated":true } ] }"#;
    const SQUADRON_LOCATION: &str = r#"{ "timestamp":"2026-10-01T18:32:00Z", "event":"CarrierLocation", "CarrierType":"SquadronCarrier", "CarrierID":3713195264, "StarSystem":"HIP 49987", "SystemAddress":869521148275, "BodyID":35 }"#;
    const JUMP_REQUEST: &str = r#"{ "timestamp":"2026-10-01T18:40:00Z", "event":"CarrierJumpRequest", "CarrierType":"FleetCarrier", "CarrierID":3708098048, "SystemName":"Colonia", "Body":"Colonia", "SystemAddress":3238296097059, "BodyID":0, "DepartureTime":"2026-10-01T18:55:00Z" }"#;
    const OWN_CARRIER_JUMP: &str = r#"{ "timestamp":"2026-10-01T18:56:00Z", "event":"CarrierJump", "Docked":true, "StationName":"W3H-NVZ", "StationType":"FleetCarrier", "MarketID":3708098048, "StarSystem":"Colonia", "SystemAddress":3238296097059, "StarPos":[-9530.50000,-910.28125,19808.12500] }"#;

    #[test]
    fn follows_the_commanders_own_carrier() {
        assert_eq!(parse_line(SQUADRON_LOCATION), None, "squadron carriers are left out");
        let dir = std::env::temp_dir().join(format!("sc-journal-carrier-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Journal.2026-10-01T180000.01.log");
        let mut f = File::create(&path).unwrap();
        writeln!(f, "{COMMANDER}\n{LOCATION}\n{CARRIER_BUY}\n{CARRIER_STATS}\n{SQUADRON_LOCATION}\n{JUMP_REQUEST}").unwrap();
        drop(f);
        let mut reader = Reader::load_dir(&dir).unwrap();
        let carrier = reader.history.commanders["F1234"].carrier.clone().unwrap();
        assert_eq!(carrier.id, 3708098048);
        assert_eq!(carrier.name.as_deref(), Some("TEST CARRIER"));
        assert_eq!(carrier.callsign.as_deref(), Some("W3H-NVZ"));
        assert_eq!(carrier.at.as_ref().map(|p| p.system.as_str()), Some("Sagittarius A*"));
        let jump = carrier.jump.as_ref().unwrap();
        assert_eq!((jump.to.system.as_str(), jump.departure.as_str()), ("Colonia", "2026-10-01T18:55:00Z"));

        // Riding it through the jump places it, with coordinates, and settles the booking.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{OWN_CARRIER_JUMP}").unwrap();
        drop(f);
        reader.poll(&dir).unwrap();
        let carrier = reader.history.commanders["F1234"].carrier.clone().unwrap();
        let at = carrier.at.unwrap();
        assert_eq!((at.system.as_str(), at.pos), ("Colonia", Some([-9530.5, -910.28125, 19808.125])));
        assert_eq!(carrier.jump, None);
        std::fs::remove_dir_all(&dir).ok();
    }

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
    fn collects_friend_names_for_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = File::create(dir.path().join("Journal.2026-10-01T180000.01.log")).unwrap();
        writeln!(f, "{COMMANDER}").unwrap();
        writeln!(f, r#"{{ "timestamp":"2026-10-01T18:01:00Z", "event":"Friends", "Status":"Online", "Name":"Alice" }}"#).unwrap();
        writeln!(f, r#"{{ "timestamp":"2026-10-01T18:02:00Z", "event":"Friends", "Status":"Lost", "Name":"Bob" }}"#).unwrap();
        writeln!(f, r#"{{ "timestamp":"2026-10-01T18:03:00Z", "event":"WingAdd", "Name":"Carol" }}"#).unwrap();
        writeln!(f, r#"{{ "timestamp":"2026-10-01T18:04:00Z", "event":"WingJoin", "Others":["Dave", {{"Name":"Erin"}}] }}"#).unwrap();
        // Our own commander coming online on a friend's list must not suggest ourselves.
        writeln!(f, r#"{{ "timestamp":"2026-10-01T18:05:00Z", "event":"Friends", "Status":"Online", "Name":"Test Pilot" }}"#).unwrap();
        // A wing sighting after "Lost" doesn't make Bob a friend again...
        writeln!(f, r#"{{ "timestamp":"2026-10-01T18:06:00Z", "event":"WingAdd", "Name":"Bob" }}"#).unwrap();
        f.flush().unwrap();

        let mut source = JournalSource::new(dir.path().to_path_buf());
        assert_eq!(source.friend_names().unwrap(), ["Dave", "Erin", "Carol", "Alice"]);

        // ...but being re-added does.
        writeln!(f, r#"{{ "timestamp":"2026-10-01T18:07:00Z", "event":"Friends", "Status":"Added", "Name":"Bob" }}"#).unwrap();
        f.flush().unwrap();
        source.poll().unwrap();
        assert_eq!(source.friend_names().unwrap(), ["Bob", "Dave", "Erin", "Carol", "Alice"]);
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
