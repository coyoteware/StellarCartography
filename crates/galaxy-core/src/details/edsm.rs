//! System details from EDSM's APIs. Three calls per system:
//! `api-v1/system` (allegiance, population, permit, main star),
//! `api-system-v1/factions` and `api-system-v1/stations`. EDSM has no
//! Powerplay data and no landing pad sizes; the system call has no
//! timestamp, so the factions' `lastUpdate` dates the overview too.
//!
//! EDSM answers `[]` or `{}` for systems it doesn't know.

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{
    clean, iso_from_unix, links, normalize_time, pad_for_kind, security_name, services, state_name, states,
    station_kind, Bodies, Faction, Factions, Overview, Source, Station, SystemDetails,
};

const BASE: &str = "https://www.edsm.net";

/// The three request URLs for a system, by id64 when known, else by name.
pub fn urls(id64: Option<u64>, name: &str) -> [String; 3] {
    let q = match id64 {
        Some(id) => format!("systemId64={id}"),
        None => format!("systemName={}", super::url_encode(name)),
    };
    [
        format!("{BASE}/api-v1/system?{q}&showId=1&showInformation=1&showPrimaryStar=1&showPermit=1"),
        format!("{BASE}/api-system-v1/factions?{q}"),
        format!("{BASE}/api-system-v1/stations?{q}"),
    ]
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SystemResponse {
    id: Option<u64>,
    id64: Option<u64>,
    name: Option<String>,
    require_permit: Option<bool>,
    permit_name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    information: Option<Information>,
    #[serde(default, deserialize_with = "lenient")]
    primary_star: Option<PrimaryStar>,
}

/// EDSM sends `[]` where an object is empty; read anything but an object as absent.
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D) -> Result<Option<T>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    if value.is_object() {
        serde_json::from_value(value).map(Some).map_err(serde::de::Error::custom)
    } else {
        Ok(None)
    }
}

/// `information` is `{}` for unpopulated systems (and `[]` in some answers).
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Information {
    allegiance: Option<String>,
    government: Option<String>,
    faction: Option<String>,
    faction_state: Option<String>,
    population: Option<u64>,
    security: Option<String>,
    economy: Option<String>,
    second_economy: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrimaryStar {
    #[serde(rename = "type")]
    kind: Option<String>,
    is_scoopable: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct FactionsResponse {
    controlling_faction: Option<Named>,
    #[serde(default)]
    factions: Vec<EdsmFaction>,
}

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EdsmFaction {
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
    last_update: Option<i64>,
}

#[derive(Deserialize)]
struct StateEntry {
    state: String,
}

#[derive(Deserialize, Default)]
struct StationsResponse {
    #[serde(default)]
    stations: Vec<EdsmStation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EdsmStation {
    name: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    market_id: Option<u64>,
    distance_to_arrival: Option<f64>,
    allegiance: Option<String>,
    government: Option<String>,
    economy: Option<String>,
    have_market: Option<bool>,
    have_shipyard: Option<bool>,
    have_outfitting: Option<bool>,
    #[serde(default)]
    other_services: Vec<String>,
    controlling_faction: Option<Named>,
    update_time: Option<UpdateTime>,
}

#[derive(Deserialize)]
struct UpdateTime {
    information: Option<String>,
}

/// An object, or the type's default when EDSM answered with something else
/// (`[]` for an unknown system). Malformed JSON is still an error.
fn object<T: for<'de> Deserialize<'de> + Default>(json: &str, what: &str) -> Result<T> {
    let value: serde_json::Value = serde_json::from_str(json).with_context(|| format!("EDSM {what}: not JSON"))?;
    if value.is_object() {
        serde_json::from_value(value).with_context(|| format!("unexpected EDSM {what} response"))
    } else {
        Ok(T::default())
    }
}

/// Parses the three responses; also returns EDSM's own system id (for links).
pub fn parse(system: &str, factions: &str, stations: &str) -> Result<SystemDetails> {
    let sys: SystemResponse = object(system, "system")?;
    let fac: FactionsResponse = object(factions, "factions")?;
    let sta: StationsResponse = object(stations, "stations")?;

    let as_of = fac.factions.iter().filter_map(|f| f.last_update).max().map(iso_from_unix);
    let info = sys.information.unwrap_or_default();
    let controlling = clean(info.faction.as_deref()).or_else(|| fac.controlling_faction.and_then(|c| clean(c.name.as_deref())));

    let overview = Overview {
        source: Source::Edsm,
        as_of: as_of.clone(),
        allegiance: clean(info.allegiance.as_deref()),
        government: clean(info.government.as_deref()),
        economy: clean(info.economy.as_deref()),
        second_economy: clean(info.second_economy.as_deref()),
        security: security_name(info.security.as_deref()),
        population: info.population,
        controlling_faction: controlling.clone(),
        controlling_faction_state: clean(info.faction_state.as_deref()).map(|s| state_name(&s)),
        permit: if sys.require_permit == Some(true) {
            clean(sys.permit_name.as_deref()).or_else(|| Some("Required".into()))
        } else {
            None
        },
    };
    let has_overview = overview.allegiance.is_some()
        || overview.government.is_some()
        || overview.population.is_some()
        || overview.controlling_faction.is_some()
        || overview.permit.is_some();

    let factions: Vec<Faction> = fac
        .factions
        .into_iter()
        .map(|f| Faction {
            controlling: controlling.as_deref() == Some(f.name.as_str()),
            allegiance: clean(f.allegiance.as_deref()),
            government: clean(f.government.as_deref()),
            influence: f.influence,
            state: clean(f.state.as_deref()).map(|s| state_name(&s)),
            active_states: states(f.active_states.iter().map(|s| s.state.as_str())),
            pending_states: states(f.pending_states.iter().map(|s| s.state.as_str())),
            recovering_states: states(f.recovering_states.iter().map(|s| s.state.as_str())),
            happiness: clean(f.happiness.as_deref()),
            my_reputation: None,
            name: f.name,
        })
        .collect();
    let factions =
        (!factions.is_empty()).then(|| Factions { source: Source::Edsm, as_of: as_of.clone(), factions, conflicts: vec![] });

    let stations = sta
        .stations
        .into_iter()
        .filter_map(|s| {
            let kind = s.kind.as_deref().and_then(station_kind);
            let mut raw: Vec<&str> = s.other_services.iter().map(String::as_str).collect();
            for (flag, service) in [(s.have_market, "Market"), (s.have_shipyard, "Shipyard"), (s.have_outfitting, "Outfitting")] {
                if flag == Some(true) {
                    raw.push(service);
                }
            }
            Some(Station {
                source: Source::Edsm,
                as_of: s.update_time.and_then(|u| u.information).as_deref().and_then(normalize_time),
                name: clean(s.name.as_deref())?,
                market_id: s.market_id,
                distance_ls: s.distance_to_arrival,
                max_pad: pad_for_kind(kind.as_deref()),
                services: services(raw),
                economy: clean(s.economy.as_deref()),
                faction: s.controlling_faction.and_then(|c| clean(c.name.as_deref())).filter(|f| f != "FleetCarrier"),
                government: clean(s.government.as_deref()),
                allegiance: clean(s.allegiance.as_deref()),
                kind,
            })
        })
        .collect();

    let bodies = sys.primary_star.and_then(|star| {
        let primary_star = clean(star.kind.as_deref())?;
        Some(Bodies { source: Source::Edsm, as_of, count: None, primary_star: Some(primary_star), scoopable: star.is_scoopable })
    });

    Ok(SystemDetails {
        overview: has_overview.then_some(overview),
        powerplay: None,
        factions,
        stations,
        bodies,
        remote: vec![],
        links: if sys.id.is_some() { links(sys.id64, sys.name.as_deref().unwrap_or_default(), sys.id) } else { vec![] },
        // EDSM's station list may leave out carriers and it has no Powerplay,
        // so its answers can't say what's gone.
        listed_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from live responses (2026-11-08).
    const SYSTEM: &str = r#"{"name":"Shinrarta Dezhra","id":4345,"id64":3932277478106,"coords":{"x":55.71875,"y":17.59375,"z":27.15625},"coordsLocked":true,"requirePermit":true,"permitName":"Founders World","information":{"allegiance":"Independent","government":"Democracy","faction":"Pilots' Federation Local Branch","factionState":"None","population":85287324,"security":"High","economy":"High Tech","secondEconomy":"Industrial","reserve":"Common"},"primaryStar":{"type":"K (Yellow-Orange) Star","name":"Shinrarta Dezhra","isScoopable":true}}"#;
    const FACTIONS: &str = r#"{"id":4345,"id64":3932277478106,"name":"Shinrarta Dezhra","url":"https://www.edsm.net/en/system/id/4345/name/Shinrarta+Dezhra","controllingFaction":{"id":81923,"name":"Pilots' Federation Local Branch","allegiance":"Independent","government":"Democracy"},"factions":[{"id":81923,"name":"Pilots' Federation Local Branch","allegiance":"Independent","government":"Democracy","influence":0.308678,"state":"None","activeStates":[],"recoveringStates":[],"pendingStates":[],"happiness":"Happy","isPlayer":false,"lastUpdate":1794170228},{"id":702,"name":"The Dark Wheel","allegiance":"Independent","government":"Democracy","influence":0.27003,"state":"Expansion","activeStates":[{"state":"Expansion"}],"recoveringStates":[{"state":"Civil liberty","trend":0}],"pendingStates":[],"happiness":"Happy","isPlayer":false,"lastUpdate":1794170228}]}"#;
    const STATIONS: &str = r#"{"id":4345,"id64":3932277478106,"name":"Shinrarta Dezhra","stations":[{"id":65,"marketId":128666762,"type":"Orbis Starport","name":"Jameson Memorial","distanceToArrival":3286,"allegiance":"Pilots Federation","government":"Democracy","economy":"High Tech","secondEconomy":"Industrial","haveMarket":true,"haveShipyard":true,"haveOutfitting":true,"otherServices":["Restock","Refuel","Repair","Contacts","Universal Cartographics","Technology Broker"],"controllingFaction":{"id":81923,"name":"Pilots' Federation Local Branch"},"updateTime":{"information":"2026-11-08 20:36:51","market":"2026-11-08 20:37:52"}},{"id":9,"type":"Odyssey Settlement","name":"Qi Cultivation Biome","distanceToArrival":10.5,"haveMarket":false,"otherServices":[],"updateTime":{"information":null}}]}"#;

    #[test]
    fn shinrarta_dezhra() {
        let d = parse(SYSTEM, FACTIONS, STATIONS).unwrap();
        let o = d.overview.unwrap();
        assert_eq!(o.source, Source::Edsm);
        assert_eq!(o.as_of.as_deref(), Some("2026-11-08T20:37:08Z"));
        assert_eq!(o.permit.as_deref(), Some("Founders World"));
        assert_eq!(o.controlling_faction_state, None);
        assert_eq!(o.second_economy.as_deref(), Some("Industrial"));
        assert!(d.powerplay.is_none());
        let f = d.factions.unwrap();
        assert!(f.factions[0].controlling);
        assert_eq!(f.factions[1].recovering_states, ["Civil Liberty"]);
        let jm = &d.stations[0];
        assert_eq!(jm.as_of.as_deref(), Some("2026-11-08T20:36:51Z"));
        assert_eq!(jm.max_pad.as_deref(), Some("L"), "inferred from the station type");
        assert_eq!(jm.services, ["Shipyard", "Outfitting", "Market", "Technology Broker", "Universal Cartographics"]);
        let settlement = &d.stations[1];
        assert_eq!(settlement.kind.as_deref(), Some("Settlement"));
        assert_eq!((settlement.max_pad.as_ref(), settlement.as_of.as_ref()), (None, None));
        let b = d.bodies.unwrap();
        assert_eq!(b.primary_star.as_deref(), Some("K (Yellow-Orange) Star"));
        assert_eq!(b.scoopable, Some(true));
        assert_eq!(d.links.last().unwrap().url, "https://www.edsm.net/en/system/id/4345/name/Shinrarta%20Dezhra");
    }

    #[test]
    fn unknown_and_unpopulated_systems() {
        let d = parse("[]", "[]", "[]").unwrap();
        assert_eq!(d, SystemDetails::default());
        let d = parse(r#"{"name":"Empty","id":1,"id64":2,"information":[],"primaryStar":{"type":"M (Red dwarf) Star","isScoopable":true}}"#, "{}", "{}");
        let d = d.unwrap();
        assert!(d.overview.is_none() && d.factions.is_none());
        assert_eq!(d.bodies.unwrap().primary_star.as_deref(), Some("M (Red dwarf) Star"));
        assert!(parse("<html>", "{}", "{}").is_err());
    }

    #[test]
    fn urls_prefer_id64() {
        assert!(urls(Some(5), "x")[0].contains("systemId64=5"));
        assert!(urls(None, "Col 285")[1].contains("systemName=Col%20285"));
    }
}
