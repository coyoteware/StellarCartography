//! System details from Spansh's system API,
//! `https://spansh.co.uk/api/system/{id64}`, which answers `{"record": {...}}`
//! in snake_case. It carries Powerplay 2.0 and landing pad counts, but has a
//! single system-wide `updated_at` and no per-station economy, government or
//! date. That time dates the system-level sections; stations are left
//! undated and the time goes in `listed_at` (see `combine`).

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{
    clean, links, max_pad, normalize_time, pad_for_kind, plausible_progress, scoopable, security_name, services,
    state_name, states, station_kind, Bodies, Faction, Factions, Overview, PowerProgress, Powerplay, Source, Station,
    SystemDetails,
};

pub fn url(id64: u64) -> String {
    format!("https://spansh.co.uk/api/system/{id64}")
}

#[derive(Deserialize)]
struct Response {
    record: Record,
}

#[derive(Deserialize)]
struct Record {
    id64: Option<u64>,
    name: Option<String>,
    updated_at: Option<String>,
    allegiance: Option<String>,
    government: Option<String>,
    primary_economy: Option<String>,
    secondary_economy: Option<String>,
    security: Option<String>,
    population: Option<u64>,
    controlling_minor_faction: Option<String>,
    controlling_minor_faction_state: Option<String>,
    needs_permit: Option<bool>,
    controlling_power: Option<String>,
    power: Option<Vec<String>>,
    power_state: Option<String>,
    power_state_control_progress: Option<f64>,
    power_state_reinforcement: Option<i64>,
    power_state_undermining: Option<i64>,
    power_conflict_progress: Option<Vec<PowerProgressRaw>>,
    #[serde(default)]
    minor_faction_presences: Vec<SpanshFaction>,
    #[serde(default)]
    stations: Vec<SpanshStation>,
    body_count: Option<u32>,
    #[serde(default)]
    bodies: Vec<SpanshBody>,
}

#[derive(Deserialize)]
struct PowerProgressRaw {
    power: String,
    progress: f64,
}

#[derive(Deserialize)]
struct SpanshFaction {
    name: String,
    allegiance: Option<String>,
    government: Option<String>,
    influence: Option<f64>,
    state: Option<String>,
    happiness: Option<String>,
    #[serde(default)]
    active_states: Vec<StateEntry>,
    #[serde(default)]
    pending_states: Vec<StateEntry>,
    #[serde(default)]
    recovering_states: Vec<StateEntry>,
}

/// The system API lists states as plain strings; the dump format uses
/// `{state, trend}` objects. Accept either.
#[derive(Deserialize)]
#[serde(untagged)]
enum StateEntry {
    Name(String),
    Object { state: String },
}

impl StateEntry {
    fn name(&self) -> &str {
        match self {
            StateEntry::Name(s) | StateEntry::Object { state: s } => s,
        }
    }
}

#[derive(Deserialize)]
struct SpanshStation {
    name: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    market_id: Option<u64>,
    distance_to_arrival: Option<f64>,
    large_pads: Option<u32>,
    medium_pads: Option<u32>,
    small_pads: Option<u32>,
    has_large_pad: Option<bool>,
    #[serde(default)]
    services: Vec<String>,
    controlling_minor_faction: Option<String>,
    government: Option<String>,
    allegiance: Option<String>,
    primary_economy: Option<String>,
}

#[derive(Deserialize)]
struct SpanshBody {
    is_main_star: Option<bool>,
    subtype: Option<String>,
}

/// Parses a `/api/system/{id64}` response.
pub fn parse(json: &str) -> Result<SystemDetails> {
    let r = serde_json::from_str::<Response>(json).context("unexpected Spansh response")?.record;
    let as_of = r.updated_at.as_deref().and_then(normalize_time);
    let controlling = clean(r.controlling_minor_faction.as_deref());

    let overview = Overview {
        source: Source::Spansh,
        as_of: as_of.clone(),
        allegiance: clean(r.allegiance.as_deref()),
        government: clean(r.government.as_deref()),
        economy: clean(r.primary_economy.as_deref()),
        second_economy: clean(r.secondary_economy.as_deref()),
        security: security_name(r.security.as_deref()),
        population: r.population,
        controlling_faction: controlling.clone(),
        controlling_faction_state: clean(r.controlling_minor_faction_state.as_deref()).map(|s| state_name(&s)),
        permit: r.needs_permit.unwrap_or(false).then(|| "Required".to_string()),
    };
    let has_overview = overview.allegiance.is_some()
        || overview.government.is_some()
        || overview.population.is_some()
        || overview.controlling_faction.is_some()
        || overview.permit.is_some();

    let powers = r.power.unwrap_or_default();
    let conflict: Vec<PowerProgress> = r
        .power_conflict_progress
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| Some(PowerProgress { progress: plausible_progress(Some(c.progress))?, power: c.power }))
        .collect();
    let powerplay = (r.power_state.is_some() || r.controlling_power.is_some() || !powers.is_empty()).then(|| Powerplay {
        source: Source::Spansh,
        as_of: as_of.clone(),
        controlling_power: clean(r.controlling_power.as_deref()),
        state: clean(r.power_state.as_deref()),
        control_progress: plausible_progress(r.power_state_control_progress),
        reinforcement: r.power_state_reinforcement,
        undermining: r.power_state_undermining,
        powers,
        conflict,
    });

    let factions: Vec<Faction> = r
        .minor_faction_presences
        .into_iter()
        .map(|f| Faction {
            controlling: controlling.as_deref() == Some(f.name.as_str()),
            allegiance: clean(f.allegiance.as_deref()),
            government: clean(f.government.as_deref()),
            influence: f.influence,
            state: clean(f.state.as_deref()).map(|s| state_name(&s)),
            active_states: states(f.active_states.iter().map(StateEntry::name)),
            pending_states: states(f.pending_states.iter().map(StateEntry::name)),
            recovering_states: states(f.recovering_states.iter().map(StateEntry::name)),
            happiness: clean(f.happiness.as_deref()),
            my_reputation: None,
            name: f.name,
        })
        .collect();
    let factions =
        (!factions.is_empty()).then(|| Factions { source: Source::Spansh, as_of: as_of.clone(), factions, conflicts: vec![] });

    let stations = r
        .stations
        .into_iter()
        .filter_map(|s| {
            let kind = s.kind.as_deref().and_then(station_kind);
            let max_pad = max_pad(s.large_pads, s.medium_pads, s.small_pads)
                .or_else(|| s.has_large_pad.unwrap_or(false).then(|| "L".to_string()))
                .or_else(|| pad_for_kind(kind.as_deref()));
            Some(Station {
                source: Source::Spansh,
                as_of: None,
                name: clean(s.name.as_deref())?,
                market_id: s.market_id,
                distance_ls: s.distance_to_arrival,
                max_pad,
                services: services(s.services.iter().map(String::as_str)),
                economy: clean(s.primary_economy.as_deref()),
                faction: clean(s.controlling_minor_faction.as_deref()).filter(|f| f != "FleetCarrier"),
                government: clean(s.government.as_deref()),
                allegiance: clean(s.allegiance.as_deref()),
                kind,
            })
        })
        .collect();

    let main_star = r.bodies.iter().find(|b| b.is_main_star == Some(true)).and_then(|b| clean(b.subtype.as_deref()));
    let bodies = (r.body_count.is_some() || main_star.is_some()).then(|| Bodies {
        source: Source::Spansh,
        as_of,
        count: r.body_count,
        scoopable: main_star.as_deref().map(scoopable),
        primary_star: main_star,
    });

    Ok(SystemDetails {
        overview: has_overview.then_some(overview),
        powerplay,
        factions,
        stations,
        bodies,
        remote: vec![],
        links: links(r.id64, r.name.as_deref().unwrap_or_default(), None),
        listed_at: r.updated_at.as_deref().and_then(normalize_time),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from live responses (2026-11-08).
    const SOL: &str = r#"{"record":{"allegiance":"Federation","bodies":[{"distance_to_arrival":0.0,"id64":10477373803,"is_main_star":true,"name":"Sol","subtype":"G (White-Yellow) Star","type":"Star"},{"is_main_star":false,"name":"Earth","subtype":"Earth-like world","type":"Planet"}],"body_count":40,"controlling_minor_faction":"Mother Gaia","controlling_minor_faction_state":"Expansion","controlling_power":"Jerome Archer","government":"Democracy","id64":10477373803,"known_permit":true,"needs_permit":true,"minor_faction_presences":[{"allegiance":"Federation","government":"Democracy","influence":0.176471,"name":"Federal Congress","recovering_states":["Expansion"],"state":"None"},{"active_states":["Civil Liberty","Expansion"],"allegiance":"Federation","government":"Democracy","influence":0.3001,"name":"Mother Gaia","state":"Expansion"}],"name":"Sol","population":18320926115,"power":["Aisling Duval","Edmund Mahon","Jerome Archer"],"power_state":"Stronghold","power_state_control_progress":0.604611,"power_state_reinforcement":81293,"power_state_undermining":157282,"primary_economy":"Refinery","secondary_economy":"Service","security":"High","stations":[{"controlling_minor_faction":"Mother Gaia","distance_to_arrival":506.806863,"has_large_pad":true,"has_market":true,"large_pads":5,"market_id":128016896,"medium_pads":9,"small_pads":8,"name":"Abraham Lincoln","services":["Dock","Autodock","Market","Contacts","Shipyard","Outfitting","Powerplay","Interstellar Factors Contact"],"type":"Orbis Starport"},{"controlling_minor_faction":"Sol Constitution Party","distance_to_arrival":2609.784276,"has_large_pad":false,"market_id":3534390528,"name":"Illy Enterprise"},{"controlling_minor_faction":"FleetCarrier","distance_to_arrival":10.0,"name":"K7Q-BQL","type":"Drake-Class Carrier","services":["Dock","Refuel"]}],"updated_at":"2026-11-08T20:36:20Z","x":0.0,"y":0.0,"z":0.0}}"#;
    const UNOCCUPIED: &str = r#"{"record":{"id64":5068732573097,"name":"LHS 3558","power":["Aisling Duval","Jerome Archer"],"power_conflict_progress":[{"power":"Jerome Archer","progress":0.53145},{"power":"Aisling Duval","progress":0.073975}],"power_state":"Unoccupied","pending_states":[],"updated_at":"2026-11-07T18:30:45Z","body_count":3}}"#;

    #[test]
    fn sol() {
        let d = parse(SOL).unwrap();
        let o = d.overview.unwrap();
        assert_eq!(o.source, Source::Spansh);
        assert_eq!(o.as_of.as_deref(), Some("2026-11-08T20:36:20Z"));
        assert_eq!(o.economy.as_deref(), Some("Refinery"));
        assert_eq!(o.second_economy.as_deref(), Some("Service"));
        assert_eq!(o.controlling_faction_state.as_deref(), Some("Expansion"));
        assert_eq!(o.permit.as_deref(), Some("Required"));
        let p = d.powerplay.unwrap();
        assert_eq!(p.controlling_power.as_deref(), Some("Jerome Archer"));
        assert_eq!(p.state.as_deref(), Some("Stronghold"));
        assert_eq!((p.control_progress, p.reinforcement, p.undermining), (Some(0.604611), Some(81293), Some(157282)));
        assert_eq!(p.powers.len(), 3);
        let f = d.factions.unwrap();
        assert!(f.factions[1].controlling);
        assert_eq!(f.factions[1].active_states, ["Civil Liberty", "Expansion"]);
        assert_eq!(f.factions[0].recovering_states, ["Expansion"]);
        assert_eq!(f.factions[0].state, None);
        assert_eq!(d.stations.len(), 3);
        let lincoln = &d.stations[0];
        assert_eq!(lincoln.as_of, None, "Spansh doesn't date stations");
        assert_eq!(d.listed_at.as_deref(), Some("2026-11-08T20:36:20Z"));
        assert_eq!(lincoln.kind.as_deref(), Some("Orbis starport"));
        assert_eq!(lincoln.max_pad.as_deref(), Some("L"));
        assert_eq!(lincoln.services, ["Shipyard", "Outfitting", "Market", "Interstellar Factors", "Powerplay"]);
        assert_eq!(d.stations[1].kind, None);
        assert_eq!(d.stations[1].max_pad, None, "has_large_pad false says nothing about smaller pads");
        assert_eq!(d.stations[2].kind.as_deref(), Some("Fleet carrier"));
        assert_eq!(d.stations[2].faction, None);
        let b = d.bodies.unwrap();
        assert_eq!(b.count, Some(40));
        assert_eq!(b.primary_star.as_deref(), Some("G (White-Yellow) Star"));
        assert_eq!(b.scoopable, Some(true));
        assert_eq!(d.links[0].url, "https://spansh.co.uk/system/10477373803");
    }

    #[test]
    fn unoccupied_and_unpopulated() {
        let d = parse(UNOCCUPIED).unwrap();
        assert!(d.overview.is_none());
        let p = d.powerplay.unwrap();
        assert_eq!(p.controlling_power, None);
        assert_eq!(p.conflict[0].power, "Jerome Archer");
        assert!(d.factions.is_none());
        assert_eq!(d.bodies.unwrap().primary_star, None);
    }

    #[test]
    fn rejects_unexpected_shapes() {
        assert!(parse(r#"{"error":"Not found"}"#).is_err());
        assert!(parse("<html>").is_err());
    }
}
