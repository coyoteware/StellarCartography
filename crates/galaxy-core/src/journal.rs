//! Commander travel history from the player journal.
//!
//! The game writes `Journal.<timestamp>.<part>.log` files (one JSON object
//! per line) to `%USERPROFILE%\Saved Games\Frontier Developments\Elite Dangerous`.
//! Only the events that carry a position (`FSDJump`, `CarrierJump`, `Location`)
//! and the ones that say which commander is playing (`Commander`, `LoadGame`)
//! are read.

use std::collections::BTreeMap;
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
    Jump(Jump),
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
}

const INTERESTING: [&str; 5] = ["\"FSDJump\"", "\"CarrierJump\"", "\"Location\"", "\"Commander\"", "\"LoadGame\""];

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
            Some(Event::Jump(Jump {
                timestamp: raw.timestamp.unwrap_or_default(),
                kind: raw.event,
                system: raw.star_system?,
                id64: raw.system_address,
                pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            }))
        }
        "Commander" => Some(Event::Commander { fid: raw.fid?, name: raw.name? }),
        "LoadGame" => Some(Event::Commander { fid: raw.fid?, name: raw.commander? }),
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

/// Journal files in chronological order. Both the old
/// (`Journal.170101120000.01.log`) and current (`Journal.2024-01-01T120000.01.log`)
/// names sort correctly by file name.
pub fn journal_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("Journal.") && n.ends_with(".log"))
        })
        .collect();
    files.sort();
    Ok(files)
}

/// Incremental reader that remembers which commander is active and how far
/// into the current file it has read, so a watcher can pick up new lines.
#[derive(Debug, Default)]
pub struct Reader {
    pub history: History,
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
        let mut line = String::new();
        let mut new = Vec::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            // Stop before a partial last line; it will be re-read once complete.
            if n == 0 || !line.ends_with('\n') {
                break;
            }
            self.offset += n as u64;
            match parse_line(line.trim_end()) {
                Some(Event::Commander { fid, name }) => {
                    let track = self.history.commanders.entry(fid.clone()).or_default();
                    track.fid = fid.clone();
                    track.name = name;
                    self.current_fid = Some(fid);
                }
                Some(Event::Jump(jump)) => {
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
    const LOCATION: &str = r#"{ "timestamp":"2026-10-01T18:00:05Z", "event":"Location", "Docked":true, "StarSystem":"Sol", "SystemAddress":10477373803, "StarPos":[0.00000,0.00000,0.00000] }"#;
    const FSDJUMP: &str = r#"{ "timestamp":"2026-10-01T18:10:00Z", "event":"FSDJump", "Taxi":false, "StarSystem":"Sagittarius A*", "SystemAddress":20578934, "StarPos":[25.21875,-20.90625,25899.96875], "JumpDist":42.1 }"#;
    const CARRIER: &str = r#"{ "timestamp":"2026-10-01T19:00:00Z", "event":"CarrierJump", "Docked":true, "StarSystem":"Colonia", "SystemAddress":3238296097059, "StarPos":[-9530.50000,-910.28125,19808.12500] }"#;

    #[test]
    fn parses_position_events() {
        let Some(Event::Jump(j)) = parse_line(FSDJUMP) else { panic!("no jump") };
        assert_eq!(j.system, "Sagittarius A*");
        assert_eq!(j.id64, Some(20578934));
        assert_eq!(j.pos, [25.21875, -20.90625, 25899.96875]);
        assert_eq!(j.kind, "FSDJump");

        let Some(Event::Jump(j)) = parse_line(CARRIER) else { panic!("no carrier jump") };
        assert_eq!(j.system, "Colonia");
        assert_eq!(j.kind, "CarrierJump");

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
}
