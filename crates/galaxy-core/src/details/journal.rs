//! System details from the commander's own journal snapshots: the latest
//! arrival event in the system (`FSDJump`, `CarrierJump` or `Location`) and
//! the latest docking/approach events at each station there.
//!
//! Field names follow Frontier's journal format. The `*_Localised` strings
//! are preferred for display; the symbols (`$economy_Agri;`) are a fallback.

use serde::Deserialize;

use super::{
    clean, max_pad, normalize_time, pad_for_kind, plausible_progress, security_name, services, state_name, states,
    station_kind, Conflict, ConflictSide, Faction, Factions, Overview, PowerProgress, Powerplay, Source, Station,
    SystemDetails,
};
use crate::journal::{Snapshot, StationSnapshots};

#[derive(Deserialize)]
struct Arrival {
    timestamp: Option<String>,
    #[serde(rename = "SystemAllegiance")]
    allegiance: Option<String>,
    #[serde(rename = "SystemEconomy")]
    economy: Option<String>,
    #[serde(rename = "SystemEconomy_Localised")]
    economy_text: Option<String>,
    #[serde(rename = "SystemSecondEconomy")]
    second_economy: Option<String>,
    #[serde(rename = "SystemSecondEconomy_Localised")]
    second_economy_text: Option<String>,
    #[serde(rename = "SystemGovernment")]
    government: Option<String>,
    #[serde(rename = "SystemGovernment_Localised")]
    government_text: Option<String>,
    #[serde(rename = "SystemSecurity")]
    security: Option<String>,
    #[serde(rename = "SystemSecurity_Localised")]
    security_text: Option<String>,
    #[serde(rename = "Population")]
    population: Option<u64>,
    #[serde(rename = "SystemFaction")]
    system_faction: Option<NamedState>,
    #[serde(rename = "ControllingPower")]
    controlling_power: Option<String>,
    #[serde(rename = "Powers")]
    powers: Option<Vec<String>>,
    #[serde(rename = "PowerplayState")]
    powerplay_state: Option<String>,
    #[serde(rename = "PowerplayStateControlProgress")]
    control_progress: Option<f64>,
    #[serde(rename = "PowerplayStateReinforcement")]
    reinforcement: Option<i64>,
    #[serde(rename = "PowerplayStateUndermining")]
    undermining: Option<i64>,
    #[serde(rename = "PowerplayConflictProgress")]
    conflict_progress: Option<Vec<ConflictProgress>>,
    #[serde(rename = "Factions")]
    factions: Option<Vec<JournalFaction>>,
    #[serde(rename = "Conflicts")]
    conflicts: Option<Vec<JournalConflict>>,
}

#[derive(Deserialize)]
struct NamedState {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "FactionState")]
    state: Option<String>,
}

#[derive(Deserialize)]
struct ConflictProgress {
    #[serde(rename = "Power")]
    power: String,
    #[serde(rename = "ConflictProgress")]
    progress: f64,
}

#[derive(Deserialize)]
struct JournalFaction {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "FactionState")]
    state: Option<String>,
    #[serde(rename = "Government")]
    government: Option<String>,
    #[serde(rename = "Influence")]
    influence: Option<f64>,
    #[serde(rename = "Allegiance")]
    allegiance: Option<String>,
    #[serde(rename = "Happiness_Localised")]
    happiness: Option<String>,
    #[serde(rename = "MyReputation")]
    my_reputation: Option<f64>,
    #[serde(rename = "ActiveStates", default)]
    active: Vec<StateEntry>,
    #[serde(rename = "PendingStates", default)]
    pending: Vec<StateEntry>,
    #[serde(rename = "RecoveringStates", default)]
    recovering: Vec<StateEntry>,
}

#[derive(Deserialize)]
struct StateEntry {
    #[serde(rename = "State")]
    state: String,
}

#[derive(Deserialize)]
struct JournalConflict {
    #[serde(rename = "WarType")]
    war_type: Option<String>,
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "Faction1")]
    faction1: Option<ConflictFaction>,
    #[serde(rename = "Faction2")]
    faction2: Option<ConflictFaction>,
}

#[derive(Deserialize)]
struct ConflictFaction {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Stake")]
    stake: Option<String>,
    #[serde(rename = "WonDays")]
    won_days: Option<u32>,
}

/// Station fields shared by `Docked`, docked `Location`/`CarrierJump` and
/// `ApproachSettlement` (which calls the name `Name`).
#[derive(Deserialize)]
struct JournalStation {
    timestamp: Option<String>,
    #[serde(rename = "StationName")]
    station_name: Option<String>,
    #[serde(rename = "StationName_Localised")]
    station_name_text: Option<String>,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Name_Localised")]
    name_text: Option<String>,
    #[serde(rename = "StationType")]
    station_type: Option<String>,
    #[serde(rename = "MarketID")]
    market_id: Option<u64>,
    #[serde(rename = "SystemAddress")]
    system: Option<u64>,
    #[serde(rename = "DistFromStarLS")]
    distance_ls: Option<f64>,
    #[serde(rename = "LandingPads")]
    pads: Option<Pads>,
    #[serde(rename = "StationServices", default)]
    services: Vec<String>,
    #[serde(rename = "StationEconomy")]
    economy: Option<String>,
    #[serde(rename = "StationEconomy_Localised")]
    economy_text: Option<String>,
    #[serde(rename = "StationFaction")]
    faction: Option<NamedState>,
    #[serde(rename = "StationGovernment")]
    government: Option<String>,
    #[serde(rename = "StationGovernment_Localised")]
    government_text: Option<String>,
    #[serde(rename = "StationAllegiance")]
    allegiance: Option<String>,
}

#[derive(Deserialize)]
struct Pads {
    #[serde(rename = "Small")]
    small: Option<u32>,
    #[serde(rename = "Medium")]
    medium: Option<u32>,
    #[serde(rename = "Large")]
    large: Option<u32>,
}

/// The localised text, or failing that a readable form of the symbol's last
/// word: `$economy_HighTech;` becomes "High Tech", `$SYSTEM_SECURITY_high;` "High".
fn text(localised: Option<String>, symbol: Option<String>) -> Option<String> {
    clean(localised.as_deref()).or_else(|| {
        let s = symbol?;
        let inner = s.trim().trim_start_matches('$').trim_end_matches(';');
        let word = inner.rsplit_once('_').map_or(inner, |(_, w)| w);
        clean(Some(word)).map(|w| state_name(&w))
    })
}

/// Details from the latest arrival snapshot for a system and the station
/// snapshots seen there. Lines that fail to parse are skipped.
pub fn from_snapshots<'a>(arrival: Option<&Snapshot>, stations: impl IntoIterator<Item = &'a StationSnapshots>) -> SystemDetails {
    let mut details = arrival
        .and_then(|s| serde_json::from_str::<Arrival>(&s.line).ok())
        .map(from_arrival)
        .unwrap_or_default();
    details.stations = stations.into_iter().filter_map(station).collect();
    details
}

fn from_arrival(a: Arrival) -> SystemDetails {
    let as_of = a.timestamp.as_deref().and_then(normalize_time);
    let controlling = a.system_faction.as_ref().and_then(|f| clean(f.name.as_deref()));

    let overview = Overview {
        source: Source::Journal,
        as_of: as_of.clone(),
        allegiance: clean(a.allegiance.as_deref()),
        government: text(a.government_text, a.government),
        economy: text(a.economy_text, a.economy),
        second_economy: text(a.second_economy_text, a.second_economy),
        security: security_name(text(a.security_text, a.security).as_deref()),
        population: a.population,
        controlling_faction: controlling.clone(),
        controlling_faction_state: a.system_faction.and_then(|f| clean(f.state.as_deref())).map(|s| state_name(&s)),
        permit: None,
    };

    let powers = a.powers.unwrap_or_default();
    let conflict: Vec<PowerProgress> = a
        .conflict_progress
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| Some(PowerProgress { progress: plausible_progress(Some(c.progress))?, power: c.power }))
        .collect();
    let powerplay = (a.powerplay_state.is_some() || a.controlling_power.is_some() || !powers.is_empty()).then(|| Powerplay {
        source: Source::Journal,
        as_of: as_of.clone(),
        controlling_power: clean(a.controlling_power.as_deref()),
        state: clean(a.powerplay_state.as_deref()),
        control_progress: plausible_progress(a.control_progress),
        reinforcement: a.reinforcement,
        undermining: a.undermining,
        powers,
        conflict,
    });

    let factions: Vec<Faction> = a
        .factions
        .unwrap_or_default()
        .into_iter()
        .map(|f| Faction {
            controlling: controlling.as_deref() == Some(f.name.as_str()),
            allegiance: clean(f.allegiance.as_deref()),
            government: clean(f.government.as_deref()),
            influence: f.influence,
            state: clean(f.state.as_deref()).map(|s| state_name(&s)),
            active_states: states(f.active.iter().map(|s| s.state.as_str())),
            pending_states: states(f.pending.iter().map(|s| s.state.as_str())),
            recovering_states: states(f.recovering.iter().map(|s| s.state.as_str())),
            happiness: clean(f.happiness.as_deref()),
            my_reputation: f.my_reputation,
            name: f.name,
        })
        .collect();
    let conflicts: Vec<Conflict> = a
        .conflicts
        .unwrap_or_default()
        .into_iter()
        .map(|c| Conflict {
            kind: match c.war_type.as_deref().unwrap_or("").to_ascii_lowercase().as_str() {
                "war" => "War".into(),
                "civilwar" => "Civil war".into(),
                "election" => "Election".into(),
                other => state_name(other),
            },
            status: clean(c.status.as_deref()).map(|s| state_name(&s)),
            sides: [c.faction1, c.faction2]
                .into_iter()
                .flatten()
                .map(|f| ConflictSide { faction: f.name, stake: clean(f.stake.as_deref()), won_days: f.won_days })
                .collect(),
        })
        .collect();
    let factions = (!factions.is_empty()).then_some(Factions { source: Source::Journal, as_of, factions, conflicts });

    SystemDetails { overview: Some(overview), powerplay, factions, ..Default::default() }
}

fn station(snaps: &StationSnapshots) -> Option<Station> {
    // Newest first, so each field comes from the latest event that has it.
    let mut lines: Vec<&Snapshot> = [&snaps.docked, &snaps.docked_arrival, &snaps.approach].into_iter().flatten().collect();
    lines.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    let parsed: Vec<JournalStation> = lines.iter().filter_map(|s| serde_json::from_str(&s.line).ok()).collect();
    let first = parsed.first()?;

    let pick = |f: &dyn Fn(&JournalStation) -> Option<String>| parsed.iter().find_map(f);
    let name = pick(&|s| {
        clean(s.station_name_text.as_deref())
            .or_else(|| clean(s.station_name.as_deref()))
            .or_else(|| clean(s.name_text.as_deref()))
            .or_else(|| clean(s.name.as_deref()))
    })?;
    let kind = pick(&|s| s.station_type.as_deref().and_then(station_kind));
    let pads = parsed.iter().find_map(|s| s.pads.as_ref());
    let max_pad = pads.and_then(|p| max_pad(p.large, p.medium, p.small)).or_else(|| pad_for_kind(kind.as_deref()));
    let services = parsed
        .iter()
        .find(|s| !s.services.is_empty())
        .map(|s| services(s.services.iter().map(String::as_str)))
        .unwrap_or_default();
    // Carriers report their "faction" as the literal FleetCarrier.
    let faction = pick(&|s| s.faction.as_ref().and_then(|f| clean(f.name.as_deref()))).filter(|f| f != "FleetCarrier");
    Some(Station {
        source: Source::Journal,
        as_of: first.timestamp.as_deref().and_then(normalize_time),
        market_id: parsed.iter().find_map(|s| s.market_id),
        // Only from events in the system it's listed in: a carrier that jumped
        // with the commander aboard has no distance in the new system yet.
        distance_ls: parsed.iter().filter(|s| s.system == Some(snaps.system)).find_map(|s| s.distance_ls),
        max_pad,
        services,
        economy: pick(&|s| text(s.economy_text.clone(), s.economy.clone())),
        faction,
        government: pick(&|s| text(s.government_text.clone(), s.government.clone())),
        allegiance: pick(&|s| clean(s.allegiance.as_deref())),
        kind,
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(line: &str) -> Snapshot {
        let ts = serde_json::from_str::<serde_json::Value>(line).unwrap()["timestamp"].as_str().unwrap().to_string();
        Snapshot { timestamp: ts, line: line.into() }
    }

    // Trimmed from real journal lines (game 4.4).
    const CONTROLLED: &str = r#"{"timestamp":"2026-11-08T19:23:47Z","event":"FSDJump","StarSystem":"HIP 49987","SystemAddress":869521148275,"StarPos":[252.59375,-46.09375,87.6875],"SystemAllegiance":"Independent","SystemEconomy":"$economy_Tourism;","SystemEconomy_Localised":"Tourism","SystemSecondEconomy":"$economy_Agri;","SystemSecondEconomy_Localised":"Agriculture","SystemGovernment":"$government_Corporate;","SystemGovernment_Localised":"Corporate","SystemSecurity":"$SYSTEM_SECURITY_high;","SystemSecurity_Localised":"High Security","Population":2423907404,"ControllingPower":"A. Lavigny-Duval","Powers":["A. Lavigny-Duval"],"PowerplayState":"Stronghold","PowerplayStateControlProgress":0.08946,"PowerplayStateReinforcement":2260,"PowerplayStateUndermining":0,"Factions":[{"Name":"Ngurii Empire Party","FactionState":"Election","Government":"Patronage","Influence":0.183794,"Allegiance":"Empire","Happiness":"$Faction_HappinessBand2;","Happiness_Localised":"Happy","MyReputation":100.0,"ActiveStates":[{"State":"Election"}]},{"Name":"The Syndicate","FactionState":"CivilUnrest","Government":"Corporate","Influence":0.6,"Allegiance":"Independent","Happiness":"","MyReputation":5.5,"ActiveStates":[{"State":"CivilUnrest"}],"PendingStates":[{"State":"PublicHoliday","Trend":0}],"RecoveringStates":[{"State":"Boom","Trend":0}]}],"SystemFaction":{"Name":"The Syndicate","FactionState":"CivilUnrest"},"Conflicts":[{"WarType":"election","Status":"active","Faction1":{"Name":"Ngurii Empire Party","Stake":"Cryptopolis","WonDays":1},"Faction2":{"Name":"Ancient Legio","Stake":"","WonDays":0}}]}"#;
    const UNOCCUPIED: &str = r#"{"timestamp":"2026-10-31T05:03:41Z","event":"FSDJump","StarSystem":"Crucis Sector MC-V b2-6","SystemAddress":1,"StarPos":[0,0,0],"SystemAllegiance":"","SystemEconomy":"$economy_None;","SystemSecondEconomy":"$economy_None;","SystemGovernment":"$government_None;","SystemSecurity":"$GAlAXY_MAP_INFO_state_anarchy;","Population":0,"Powers":["A. Lavigny-Duval","Aisling Duval"],"PowerplayState":"Unoccupied","PowerplayConflictProgress":[{"Power":"A. Lavigny-Duval","ConflictProgress":0.003075},{"Power":"Aisling Duval","ConflictProgress":0.007817}]}"#;
    const DOCKED: &str = r#"{"timestamp":"2026-11-08T05:47:19Z","event":"Docked","StationName":"Bohm Prominence","StationType":"CraterOutpost","StarSystem":"Synuefe GN-I c26-9","SystemAddress":2558223422178,"MarketID":4338909955,"StationFaction":{"Name":"Ngurii Empire Party"},"StationGovernment":"$government_Patronage;","StationGovernment_Localised":"Patronage","StationAllegiance":"Empire","StationServices":["dock","autodock","commodities","contacts","missions","outfitting","rearm","refuel","repair","shipyard","facilitator"],"StationEconomy":"$economy_Refinery;","StationEconomy_Localised":"Refinery","DistFromStarLS":4745.520645,"LandingPads":{"Small":4,"Medium":4,"Large":4}}"#;
    const LATER_LOGIN: &str = r#"{"timestamp":"2026-11-09T08:00:00Z","event":"Location","Docked":true,"StationName":"Bohm Prominence","StationType":"CraterOutpost","MarketID":4338909955,"StationFaction":{"Name":"Other Faction"},"StationServices":["dock","commodities"],"StarSystem":"Synuefe GN-I c26-9","SystemAddress":2558223422178,"StarPos":[0,0,0]}"#;
    const CARRIER: &str = r#"{"timestamp":"2026-11-01T12:00:00Z","event":"Docked","StationName":"W3H-NVZ","StationType":"FleetCarrier","SystemAddress":5,"MarketID":3708098048,"StationFaction":{"Name":"FleetCarrier"},"StationServices":["dock","refuel","carriermanagement"],"DistFromStarLS":12.5,"LandingPads":{"Small":4,"Medium":4,"Large":8}}"#;

    #[test]
    fn controlled_system() {
        let d = from_snapshots(Some(&snap(CONTROLLED)), []);
        let o = d.overview.unwrap();
        assert_eq!(o.as_of.as_deref(), Some("2026-11-08T19:23:47Z"));
        assert_eq!(o.allegiance.as_deref(), Some("Independent"));
        assert_eq!(o.government.as_deref(), Some("Corporate"));
        assert_eq!(o.economy.as_deref(), Some("Tourism"));
        assert_eq!(o.second_economy.as_deref(), Some("Agriculture"));
        assert_eq!(o.security.as_deref(), Some("High"));
        assert_eq!(o.population, Some(2423907404));
        assert_eq!(o.controlling_faction.as_deref(), Some("The Syndicate"));
        assert_eq!(o.controlling_faction_state.as_deref(), Some("Civil Unrest"));

        let p = d.powerplay.unwrap();
        assert_eq!(p.controlling_power.as_deref(), Some("A. Lavigny-Duval"));
        assert_eq!(p.state.as_deref(), Some("Stronghold"));
        assert_eq!(p.control_progress, Some(0.08946));
        assert_eq!((p.reinforcement, p.undermining), (Some(2260), Some(0)));

        let f = d.factions.unwrap();
        assert_eq!(f.factions.len(), 2);
        let syndicate = &f.factions[1];
        assert!(syndicate.controlling && !f.factions[0].controlling);
        assert_eq!(syndicate.active_states, ["Civil Unrest"]);
        assert_eq!(syndicate.pending_states, ["Public Holiday"]);
        assert_eq!(syndicate.recovering_states, ["Boom"]);
        assert_eq!(syndicate.happiness, None, "empty happiness is dropped");
        assert_eq!(f.factions[0].my_reputation, Some(100.0));
        let c = &f.conflicts[0];
        assert_eq!((c.kind.as_str(), c.status.as_deref()), ("Election", Some("Active")));
        assert_eq!(c.sides[0].stake.as_deref(), Some("Cryptopolis"));
        assert_eq!(c.sides[1].stake, None);
    }

    #[test]
    fn wrapped_progress_values_are_dropped() {
        let line = CONTROLLED.replace("0.08946", "12271.366211");
        let p = from_snapshots(Some(&snap(&line)), []).powerplay.unwrap();
        assert_eq!(p.control_progress, None);
        assert_eq!(p.undermining, Some(0), "the other figures still show");
    }

    #[test]
    fn a_carrier_that_jumped_has_no_distance_in_its_new_system() {
        let jumped = r#"{"timestamp":"2026-11-02T12:00:00Z","event":"CarrierJump","Docked":true,"StationName":"W3H-NVZ","StationType":"FleetCarrier","MarketID":3708098048,"StarSystem":"Elsewhere","SystemAddress":6,"StarPos":[0,0,0]}"#;
        let carrier = StationSnapshots { system: 6, docked: Some(snap(CARRIER)), docked_arrival: Some(snap(jumped)), approach: None };
        let s = &from_snapshots(None, [&carrier]).stations[0];
        assert_eq!(s.distance_ls, None, "12.5 ls was from the previous system");
        assert_eq!(s.max_pad.as_deref(), Some("L"), "pads don't depend on the system");
    }

    #[test]
    fn unoccupied_unpopulated_system_falls_back_to_symbols() {
        let d = from_snapshots(Some(&snap(UNOCCUPIED)), []);
        let o = d.overview.unwrap();
        assert_eq!(o.population, Some(0));
        assert_eq!((o.allegiance, o.economy, o.government), (None, None, None));
        assert_eq!(o.security.as_deref(), Some("Anarchy"), "readable form of the symbol");
        let p = d.powerplay.unwrap();
        assert_eq!(p.controlling_power, None);
        assert_eq!(p.state.as_deref(), Some("Unoccupied"));
        assert_eq!(p.conflict.len(), 2);
        assert_eq!(p.conflict[1].progress, 0.007817);
        assert!(d.factions.is_none());
    }

    #[test]
    fn stations_merge_their_snapshots_newest_first() {
        let port = StationSnapshots {
            system: 2558223422178,
            docked: Some(snap(DOCKED)),
            docked_arrival: Some(snap(LATER_LOGIN)),
            approach: None,
        };
        let carrier = StationSnapshots { system: 5, docked: Some(snap(CARRIER)), ..Default::default() };
        let d = from_snapshots(None, [&port, &carrier]);
        assert!(d.overview.is_none());
        let s = &d.stations[0];
        assert_eq!(s.name, "Bohm Prominence");
        assert_eq!(s.kind.as_deref(), Some("Planetary outpost"));
        assert_eq!(s.as_of.as_deref(), Some("2026-11-09T08:00:00Z"));
        assert_eq!(s.faction.as_deref(), Some("Other Faction"), "the newer login wins");
        assert_eq!(s.services, ["Market"], "the newer list wins");
        assert_eq!(s.max_pad.as_deref(), Some("L"), "pads only come from Docked");
        assert_eq!(s.distance_ls, Some(4745.520645));
        assert_eq!(s.government.as_deref(), Some("Patronage"));
        let c = &d.stations[1];
        assert_eq!(c.kind.as_deref(), Some("Fleet carrier"));
        assert_eq!(c.distance_ls, Some(12.5));
        assert_eq!(c.faction, None);
        assert!(c.services.is_empty(), "only routine services");
    }
}
