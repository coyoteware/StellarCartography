//! Everything known about one system beyond its position: population and
//! government, Powerplay, minor factions, stations and bodies.
//!
//! Three sources feed the same model:
//! * [`journal`]: the commander's own journal (exact, but only for systems
//!   they visited, as of the visit);
//! * [`spansh`]: Spansh's system API (any system, includes Powerplay 2.0);
//! * [`edsm`]: EDSM's system APIs (any system, no Powerplay).
//!
//! Every section records its source and an "as of" time, and [`merge`] keeps
//! the newer of two answers section by section. Names and codes are
//! normalised so the sources look alike: faction states and services get the
//! same spelling, station types become readable, times become UTC RFC 3339.

pub mod edsm;
pub mod journal;
pub mod spansh;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Journal,
    Edsm,
    Spansh,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub source: Source,
    pub as_of: Option<String>,
    pub allegiance: Option<String>,
    pub government: Option<String>,
    pub economy: Option<String>,
    pub second_economy: Option<String>,
    pub security: Option<String>,
    pub population: Option<u64>,
    pub controlling_faction: Option<String>,
    pub controlling_faction_state: Option<String>,
    /// Permit name, or "Required" when a source only says one is needed.
    pub permit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerProgress {
    pub power: String,
    /// 0..1
    pub progress: f64,
}

/// Powerplay 2.0. Controlled systems have a controlling power, control
/// progress and reinforcement/undermining; unoccupied ones have per-power
/// acquisition progress (`conflict`) instead.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Powerplay {
    pub source: Source,
    pub as_of: Option<String>,
    pub controlling_power: Option<String>,
    pub state: Option<String>,
    pub control_progress: Option<f64>,
    pub reinforcement: Option<i64>,
    pub undermining: Option<i64>,
    pub powers: Vec<String>,
    pub conflict: Vec<PowerProgress>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Faction {
    pub name: String,
    pub allegiance: Option<String>,
    pub government: Option<String>,
    /// 0..1
    pub influence: Option<f64>,
    pub state: Option<String>,
    pub active_states: Vec<String>,
    pub pending_states: Vec<String>,
    pub recovering_states: Vec<String>,
    pub happiness: Option<String>,
    pub controlling: bool,
    /// The commander's reputation with the faction (journal only), -100..100.
    pub my_reputation: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSide {
    pub faction: String,
    /// The asset at stake (a station or settlement), when there is one.
    pub stake: Option<String>,
    pub won_days: Option<u32>,
}

/// A war, civil war or election between two factions (journal only).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    /// "War", "Civil war" or "Election".
    pub kind: String,
    /// "Active" or "Pending", when known.
    pub status: Option<String>,
    pub sides: Vec<ConflictSide>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Factions {
    pub source: Source,
    pub as_of: Option<String>,
    pub factions: Vec<Faction>,
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Station {
    pub source: Source,
    pub as_of: Option<String>,
    pub name: String,
    /// Readable type, e.g. "Coriolis starport", "Fleet carrier" (see [`station_kind`]).
    pub kind: Option<String>,
    pub market_id: Option<u64>,
    pub distance_ls: Option<f64>,
    /// Largest landing pad: "L", "M" or "S".
    pub max_pad: Option<String>,
    /// Notable services in a fixed order (see [`service_name`]).
    pub services: Vec<String>,
    pub economy: Option<String>,
    pub faction: Option<String>,
    pub government: Option<String>,
    pub allegiance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bodies {
    pub source: Source,
    pub as_of: Option<String>,
    pub count: Option<u32>,
    pub primary_star: Option<String>,
    pub scoopable: Option<bool>,
}

/// How an online lookup went.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub source: Source,
    pub ok: bool,
    /// The site answered but has no record of the system: not an outage.
    pub missing: bool,
    pub error: Option<String>,
    /// When the data shown was fetched (it may come from the local cache).
    pub fetched_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemDetails {
    pub overview: Option<Overview>,
    pub powerplay: Option<Powerplay>,
    pub factions: Option<Factions>,
    pub stations: Vec<Station>,
    pub bodies: Option<Bodies>,
    pub remote: Vec<RemoteStatus>,
    pub links: Vec<Link>,
    /// When an online source last saw the system as a whole (Spansh's
    /// `updated_at`). Its station list and Powerplay are complete as of then,
    /// which [`combine`] relies on. Not sent to the web view.
    #[serde(skip)]
    pub listed_at: Option<String>,
}

impl SystemDetails {
    /// True when no section has anything in it.
    pub fn is_empty(&self) -> bool {
        self.overview.is_none()
            && self.powerplay.is_none()
            && self.factions.is_none()
            && self.stations.is_empty()
            && self.bodies.is_none()
    }
}

/// Combines two answers about the same system. Each section comes from
/// whichever side saw it more recently; stations are matched by MarketID (or
/// name) and merged field by field, newest first. `remote` and `links` are
/// concatenated.
pub fn merge(a: SystemDetails, b: SystemDetails) -> SystemDetails {
    SystemDetails {
        overview: merge_overview(a.overview, b.overview),
        powerplay: newer(a.powerplay, b.powerplay, |s| &s.as_of),
        factions: merge_factions(a.factions, b.factions),
        bodies: newer(a.bodies, b.bodies, |s| &s.as_of),
        stations: merge_stations(a.stations, b.stations),
        remote: [a.remote, b.remote].concat(),
        links: dedup_links([a.links, b.links].concat()),
        listed_at: a.listed_at.max(b.listed_at),
    }
}

/// How much older than the online listing a journal docking may be and
/// still lead over that site's (undated) record of the station.
const DOCKING_GRACE_DAYS: i64 = 7;

/// Merges the journal's answer with an online one: [`merge`], plus rules
/// that need to know which side is which.
/// * Fleet carriers and megaships move. One the journal last saw here is
///   dropped when a newer online station list doesn't have it.
/// * Journal Powerplay is dropped when a newer online answer has none
///   (Spansh leaves Powerplay out for systems without it). EDSM never has
///   Powerplay, so its answers carry no `listed_at` and change nothing here.
/// * Spansh station records aren't dated. They lose to a journal docking
///   unless the docking is more than a week older than Spansh's listing.
pub fn combine(mut journal: SystemDetails, mut online: SystemDetails) -> SystemDetails {
    let Some(listed) = online.listed_at.clone() else { return merge(journal, online) };
    let older = |as_of: &Option<String>| as_of.as_deref() < Some(listed.as_str());
    let moves = |s: &Station| matches!(s.kind.as_deref(), Some("Fleet carrier" | "Megaship"));
    journal
        .stations
        .retain(|s| !(moves(s) && older(&s.as_of) && !online.stations.iter().any(|o| same_station(o, s))));
    if online.powerplay.is_none() && journal.powerplay.as_ref().is_some_and(|p| older(&p.as_of)) {
        journal.powerplay = None;
    }

    let cutoff = unix_from_iso(&listed).map(|t| iso_from_unix(t - DOCKING_GRACE_DAYS * 86_400));
    let (journal_stations, online_stations) = (std::mem::take(&mut journal.stations), std::mem::take(&mut online.stations));
    let stations = merge_stations_by(journal_stations, online_stations, |existing, incoming| match &incoming.as_of {
        Some(_) => incoming.as_of > existing.as_of,
        None => existing.as_of < cutoff,
    });
    SystemDetails { stations, ..merge(journal, online) }
}

/// The section with the later `as_of`; a known time beats an unknown one,
/// and `a` wins ties.
fn newer<T>(a: Option<T>, b: Option<T>, as_of: impl Fn(&T) -> &Option<String>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if as_of(&b) > as_of(&a) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// The newer overview, with fields it lacks (the journal never knows the
/// permit) filled from the older one.
fn merge_overview(a: Option<Overview>, b: Option<Overview>) -> Option<Overview> {
    let (newer, older) = match (a, b) {
        (Some(a), Some(b)) if b.as_of > a.as_of => (b, a),
        (Some(a), Some(b)) => (a, b),
        (a, b) => return a.or(b),
    };
    Some(Overview {
        allegiance: newer.allegiance.or(older.allegiance),
        government: newer.government.or(older.government),
        economy: newer.economy.or(older.economy),
        second_economy: newer.second_economy.or(older.second_economy),
        security: newer.security.or(older.security),
        population: newer.population.or(older.population),
        controlling_faction: newer.controlling_faction.or(older.controlling_faction),
        controlling_faction_state: newer.controlling_faction_state.or(older.controlling_faction_state),
        permit: newer.permit.or(older.permit),
        ..newer
    })
}

/// The newer factions, keeping what only the journal knows from the older
/// side: the commander's reputation, pending states and conflicts.
fn merge_factions(a: Option<Factions>, b: Option<Factions>) -> Option<Factions> {
    let (mut newer, older) = match (a, b) {
        (Some(a), Some(b)) if b.as_of > a.as_of => (b, a),
        (Some(a), Some(b)) => (a, b),
        (a, b) => return a.or(b),
    };
    for f in &mut newer.factions {
        if let Some(old) = older.factions.iter().find(|o| o.name.eq_ignore_ascii_case(&f.name)) {
            if f.my_reputation.is_none() {
                f.my_reputation = old.my_reputation;
            }
            if f.pending_states.is_empty() {
                f.pending_states = old.pending_states.clone();
            }
        }
    }
    if newer.conflicts.is_empty() {
        newer.conflicts = older.conflicts;
    }
    Some(newer)
}

fn merge_stations(a: Vec<Station>, b: Vec<Station>) -> Vec<Station> {
    merge_stations_by(a, b, |existing, incoming| incoming.as_of > existing.as_of)
}

/// Unions two station lists. A station that matches one already listed is
/// merged into it: `incoming_leads(existing, incoming)` says whose fields
/// win, and the other record fills the gaps.
fn merge_stations_by(a: Vec<Station>, b: Vec<Station>, incoming_leads: impl Fn(&Station, &Station) -> bool) -> Vec<Station> {
    let mut out: Vec<Station> = Vec::with_capacity(a.len() + b.len());
    for s in a.into_iter().chain(b) {
        match out.iter_mut().find(|o| same_station(o, &s)) {
            Some(existing) => {
                let (lead, other) = if incoming_leads(existing, &s) { (s, existing.clone()) } else { (existing.clone(), s) };
                *existing = fill_station(lead, other);
            }
            None => out.push(s),
        }
    }
    out
}

/// Same MarketID, or the same name when either side lacks one.
fn same_station(a: &Station, b: &Station) -> bool {
    match (a.market_id, b.market_id) {
        (Some(x), Some(y)) => x == y,
        _ => a.name.eq_ignore_ascii_case(&b.name),
    }
}

/// `newer`, with its gaps filled from `older`.
fn fill_station(newer: Station, older: Station) -> Station {
    Station {
        kind: newer.kind.or(older.kind),
        market_id: newer.market_id.or(older.market_id),
        distance_ls: newer.distance_ls.or(older.distance_ls),
        max_pad: newer.max_pad.or(older.max_pad),
        services: if newer.services.is_empty() { older.services } else { newer.services },
        economy: newer.economy.or(older.economy),
        faction: newer.faction.or(older.faction),
        government: newer.government.or(older.government),
        allegiance: newer.allegiance.or(older.allegiance),
        ..newer
    }
}

fn dedup_links(links: Vec<Link>) -> Vec<Link> {
    let mut out: Vec<Link> = Vec::new();
    for l in links {
        if !out.iter().any(|o| o.label == l.label) {
            out.push(l);
        }
    }
    out
}

/// Links to the system on the community sites. EDSM needs its own system
/// id, so it's only linked when an EDSM lookup supplied one.
pub fn links(id64: Option<u64>, name: &str, edsm_id: Option<u64>) -> Vec<Link> {
    let mut out = Vec::new();
    if let Some(id) = id64 {
        out.push(Link { label: "Spansh".into(), url: format!("https://spansh.co.uk/system/{id}") });
    }
    out.push(Link { label: "Inara".into(), url: format!("https://inara.cz/elite/starsystem/?search={}", url_encode(name)) });
    if let Some(id) = edsm_id {
        out.push(Link { label: "EDSM".into(), url: format!("https://www.edsm.net/en/system/id/{id}/name/{}", url_encode(name)) });
    }
    out
}

/// Percent-encodes everything except unreserved characters (RFC 3986).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// --- Normalisation -----------------------------------------------------------

/// Trims, and drops empty strings and the placeholder "None".
pub(crate) fn clean(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty() && *s != "None" && *s != "null").map(str::to_string)
}

/// One spelling for faction states across sources: the journal writes
/// `CivilWar`, EDSM "Civil war", Spansh "Civil War"; all become "Civil War".
pub fn state_name(s: &str) -> String {
    let s = s.trim();
    let mut words = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() && !words.ends_with(' ') {
            words.push(' ');
        }
        words.push(c);
    }
    words
        .split_whitespace()
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().chain(cs.map(|c| c.to_ascii_lowercase())).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Faction states with the placeholder "None" removed and spelling unified.
pub(crate) fn states<'a>(it: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    it.into_iter().filter_map(|s| clean(Some(s))).map(|s| state_name(&s)).collect()
}

/// "High Security" (journal) and "High" (Spansh, EDSM) both become "High".
pub(crate) fn security_name(s: Option<&str>) -> Option<String> {
    clean(s).map(|s| s.strip_suffix(" Security").map(str::to_string).unwrap_or(s))
}

fn key(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Readable station type for a journal code (`CraterOutpost`) or a site's
/// label ("Planetary Outpost", "Drake-Class Carrier"). Unknown types pass
/// through unchanged.
pub fn station_kind(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let kind = match key(raw).as_str() {
        "coriolis" | "coriolisstarport" => "Coriolis starport",
        "orbis" | "orbisstarport" => "Orbis starport",
        "ocellus" | "ocellusstarport" | "bernal" => "Ocellus starport",
        "dodec" | "dodecstarport" => "Dodec starport",
        "outpost" => "Outpost",
        "asteroidbase" => "Asteroid base",
        "craterport" | "planetaryport" | "surfacestation" | "dockableplanetstation" => "Planetary port",
        "crateroutpost" | "planetaryoutpost" => "Planetary outpost",
        "onfootsettlement" | "odysseysettlement" | "settlement" => "Settlement",
        "megaship" => "Megaship",
        "fleetcarrier" | "drakeclasscarrier" | "javelinclasscarrier" => "Fleet carrier",
        "spaceconstructiondepot" | "planetaryconstructiondepot" => "Construction site",
        _ => return Some(raw.to_string()),
    };
    Some(kind.to_string())
}

/// The largest pad from pad counts.
pub(crate) fn max_pad(large: Option<u32>, medium: Option<u32>, small: Option<u32>) -> Option<String> {
    let has = |n: Option<u32>| n.is_some_and(|n| n > 0);
    if has(large) {
        Some("L".into())
    } else if has(medium) {
        Some("M".into())
    } else if has(small) {
        Some("S".into())
    } else {
        None
    }
}

/// The largest pad implied by a station type, for sources without pad
/// counts. Only types with a fixed layout; settlements vary.
pub(crate) fn pad_for_kind(kind: Option<&str>) -> Option<String> {
    match kind? {
        "Coriolis starport" | "Orbis starport" | "Ocellus starport" | "Dodec starport" | "Asteroid base"
        | "Planetary port" | "Fleet carrier" => Some("L".into()),
        "Outpost" => Some("M".into()),
        _ => None,
    }
}

/// The services that set one port apart from another, in the order the
/// panel lists them. The left side holds every spelling seen (journal codes,
/// Spansh and EDSM labels), reduced by [`key`]. Services nearly every port
/// has (refuel, repair, restock, missions, search and rescue, the
/// colonisation and transport contacts...) aren't listed.
const SERVICES: &[(&[&str], &str)] = &[
    (&["shipyard"], "Shipyard"),
    (&["outfitting"], "Outfitting"),
    (&["commodities", "market"], "Market"),
    (&["blackmarket"], "Black Market"),
    (&["facilitator", "interstellarfactorscontact", "interstellarfactors"], "Interstellar Factors"),
    (&["materialtrader"], "Material Trader"),
    (&["techbroker", "technologybroker"], "Technology Broker"),
    (&["exploration", "universalcartographics"], "Universal Cartographics"),
    (&["vistagenomics"], "Vista Genomics"),
    (&["bartender"], "Bartender"),
    (&["powerplay"], "Powerplay"),
];

/// Maps raw service names to the notable ones, deduplicated and in a fixed
/// order; everything else is dropped.
pub fn services<'a>(raw: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let keys: Vec<String> = raw.into_iter().map(key).collect();
    SERVICES
        .iter()
        .filter(|(aliases, _)| aliases.iter().any(|a| keys.iter().any(|k| k == a)))
        .map(|(_, name)| name.to_string())
        .collect()
}

/// Powerplay progress as reported, or `None` when it can't be real. The game
/// sometimes writes wrapped values (12271.37, 6607.51) where genuine ones top
/// out around 5 (a Stronghold well past its threshold).
pub(crate) fn plausible_progress(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && (0.0..=100.0).contains(x))
}

/// Whether fuel can be scooped from a main star of this type: only the
/// main-sequence classes K, G, B, F, O, A, M.
pub fn scoopable(star_type: &str) -> bool {
    let t = star_type.trim();
    let class = t.split([' ', '(']).next().unwrap_or("");
    matches!(class, "O" | "B" | "A" | "F" | "G" | "K" | "M")
}

// --- Times -------------------------------------------------------------------

/// UTC RFC 3339 (`2026-11-08T20:36:20Z`) for seconds since 1970.
pub fn iso_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Seconds since 1970 for a time [`normalize_time`] accepts.
pub fn unix_from_iso(s: &str) -> Option<i64> {
    let n = normalize_time(s)?;
    let num = |r: std::ops::Range<usize>| n[r].parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Brings the sources' time formats to `YYYY-MM-DDTHH:MM:SSZ` so they
/// compare as strings: journal and Spansh times are already like that (but
/// may carry fractions), EDSM writes `2026-11-08 20:36:51` in UTC.
pub fn normalize_time(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r).is_some_and(|x| x.iter().all(u8::is_ascii_digit));
    let ok = b.len() >= 19
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && (b[10] == b'T' || b[10] == b' ')
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19);
    ok.then(|| format!("{}T{}Z", &s[..10], &s[11..19]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(source: Source, as_of: &str, name: &str, market_id: Option<u64>) -> Station {
        Station {
            source,
            as_of: Some(as_of.into()),
            name: name.into(),
            kind: None,
            market_id,
            distance_ls: None,
            max_pad: None,
            services: vec![],
            economy: None,
            faction: None,
            government: None,
            allegiance: None,
        }
    }

    #[test]
    fn normalises_names() {
        assert_eq!(state_name("CivilWar"), "Civil War");
        assert_eq!(state_name("Civil liberty"), "Civil Liberty");
        assert_eq!(state_name("Public Holiday"), "Public Holiday");
        assert_eq!(state_name("InfrastructureFailure"), "Infrastructure Failure");
        assert_eq!(states(["None", "Boom", "PirateAttack"]), ["Boom", "Pirate Attack"]);
        assert_eq!(security_name(Some("High Security")).as_deref(), Some("High"));
        assert_eq!(security_name(Some("Anarchy")).as_deref(), Some("Anarchy"));
        assert_eq!(station_kind("CraterOutpost").as_deref(), Some("Planetary outpost"));
        assert_eq!(station_kind("Drake-Class Carrier").as_deref(), Some("Fleet carrier"));
        assert_eq!(station_kind("Odyssey Settlement").as_deref(), Some("Settlement"));
        assert_eq!(station_kind("GameplayPOI").as_deref(), Some("GameplayPOI"));
        assert_eq!(station_kind(" "), None);
        assert_eq!(
            services(["dock", "commodities", "shipyard", "techBroker", "Interstellar Factors Contact", "contacts"]),
            ["Shipyard", "Market", "Interstellar Factors", "Technology Broker"]
        );
        assert!(scoopable("K (Yellow-Orange) Star"));
        assert!(scoopable("M (Red giant) Star"));
        assert!(!scoopable("White Dwarf (DA) Star"));
        assert!(!scoopable("T Tauri Star"));
        assert_eq!(max_pad(Some(0), Some(2), Some(4)).as_deref(), Some("M"));
        assert_eq!(url_encode("Col 285 Sector"), "Col%20285%20Sector");
    }

    #[test]
    fn normalises_times() {
        assert_eq!(iso_from_unix(1794170228), "2026-11-08T20:37:08Z");
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(normalize_time("2026-11-08 20:36:51").as_deref(), Some("2026-11-08T20:36:51Z"));
        assert_eq!(normalize_time("2026-11-08T20:36:20.123Z").as_deref(), Some("2026-11-08T20:36:20Z"));
        assert_eq!(normalize_time("yesterday"), None);
        for t in [0, 951_782_400, 1_794_170_228, 4_102_444_799] {
            assert_eq!(unix_from_iso(&iso_from_unix(t)), Some(t));
        }
        assert_eq!(plausible_progress(Some(5.35)), Some(5.35));
        assert_eq!(plausible_progress(Some(12271.366211)), None);
        assert_eq!(plausible_progress(Some(f64::NAN)), None);
    }

    fn carrier(source: Source, as_of: Option<&str>, market_id: u64) -> Station {
        let mut s = station(source, as_of.unwrap_or_default(), "Carrier", Some(market_id));
        s.as_of = as_of.map(str::to_string);
        s.kind = Some("Fleet carrier".into());
        s
    }

    #[test]
    fn combine_drops_what_a_newer_listing_no_longer_has() {
        let listed = "2026-11-08T20:00:00Z";
        let powerplay = |source, as_of: &str| Powerplay {
            source,
            as_of: Some(as_of.into()),
            controlling_power: Some("Old Power".into()),
            state: Some("Exploited".into()),
            control_progress: None,
            reinforcement: None,
            undermining: None,
            powers: vec![],
            conflict: vec![],
        };
        let journal = SystemDetails {
            powerplay: Some(powerplay(Source::Journal, "2026-10-01T00:00:00Z")),
            stations: vec![
                carrier(Source::Journal, Some("2026-10-01T00:00:00Z"), 1), // left since
                carrier(Source::Journal, Some("2026-11-08T21:00:00Z"), 2), // arrived after the listing
                station(Source::Journal, "2026-10-01T00:00:00Z", "Port", Some(3)),
            ],
            ..Default::default()
        };
        let online = SystemDetails { listed_at: Some(listed.into()), ..Default::default() };
        let c = combine(journal, online);
        assert!(c.powerplay.is_none(), "Spansh newer and has no Powerplay");
        let ids: Vec<_> = c.stations.iter().map(|s| s.market_id.unwrap()).collect();
        assert_eq!(ids, [2, 3], "fixed ports stay, only departed carriers go");

        // Without a listing time (EDSM, or offline) nothing is dropped.
        let journal = SystemDetails { powerplay: Some(powerplay(Source::Journal, "2026-10-01T00:00:00Z")), ..Default::default() };
        assert!(combine(journal, SystemDetails::default()).powerplay.is_some());
    }

    #[test]
    fn combine_lets_recent_dockings_lead_undated_listings() {
        let listed = "2026-11-08T20:00:00Z";
        let mut listing = station(Source::Spansh, "", "Port", Some(1));
        listing.as_of = None;
        listing.distance_ls = Some(10.0);
        listing.faction = Some("Listed".into());
        let mut docked = station(Source::Journal, "2026-11-05T00:00:00Z", "Port", Some(1));
        docked.faction = Some("Docked".into());
        let online = |s: Station| SystemDetails { listed_at: Some(listed.into()), stations: vec![s], ..Default::default() };

        let c = combine(SystemDetails { stations: vec![docked.clone()], ..Default::default() }, online(listing.clone()));
        assert_eq!(c.stations[0].faction.as_deref(), Some("Docked"), "docked 3 days before the listing");
        assert_eq!(c.stations[0].distance_ls, Some(10.0), "gaps still filled from the listing");

        docked.as_of = Some("2026-09-01T00:00:00Z".into());
        let c = combine(SystemDetails { stations: vec![docked], ..Default::default() }, online(listing));
        assert_eq!(c.stations[0].faction.as_deref(), Some("Listed"), "a two-month-old docking loses");
    }

    #[test]
    fn merge_keeps_the_newer_section_and_fills_station_gaps() {
        let overview = |source, as_of: &str, pop| Overview {
            source,
            as_of: Some(as_of.into()),
            allegiance: None,
            government: None,
            economy: None,
            second_economy: None,
            security: None,
            population: Some(pop),
            controlling_faction: None,
            controlling_faction_state: None,
            permit: None,
        };
        let mut docked = station(Source::Journal, "2026-11-01T00:00:00Z", "Jameson Memorial", Some(128666762));
        docked.max_pad = Some("L".into());
        docked.services = vec!["Shipyard".into()];
        let mut listed = station(Source::Spansh, "2026-11-08T00:00:00Z", "Jameson Memorial", Some(128666762));
        listed.distance_ls = Some(3286.0);
        let journal = SystemDetails {
            overview: Some(overview(Source::Journal, "2026-11-01T00:00:00Z", 1)),
            stations: vec![docked, station(Source::Journal, "2026-11-01T00:00:00Z", "Only Mine", None)],
            ..Default::default()
        };
        let spansh = SystemDetails {
            overview: Some(overview(Source::Spansh, "2026-11-08T00:00:00Z", 2)),
            stations: vec![listed],
            links: links(Some(1), "X", None),
            ..Default::default()
        };
        let m = merge(journal, spansh);
        assert_eq!(m.overview.unwrap().population, Some(2));
        assert_eq!(m.stations.len(), 2);
        let jm = &m.stations[0];
        assert_eq!(jm.source, Source::Spansh, "newer record wins");
        assert_eq!(jm.distance_ls, Some(3286.0));
        assert_eq!(jm.max_pad.as_deref(), Some("L"), "gap filled from the older record");
        assert_eq!(jm.services, ["Shipyard"]);
        assert_eq!(m.links.len(), 2);
    }

    #[test]
    fn merge_keeps_journal_only_fields_when_online_is_newer() {
        let faction = |rep: Option<f64>| Faction {
            name: "Mother Gaia".into(),
            allegiance: None,
            government: None,
            influence: Some(0.3),
            state: None,
            active_states: vec![],
            pending_states: vec![],
            recovering_states: vec![],
            happiness: None,
            controlling: true,
            my_reputation: rep,
        };
        let conflict = Conflict { kind: "War".into(), status: None, sides: vec![] };
        let journal = SystemDetails {
            factions: Some(Factions {
                source: Source::Journal,
                as_of: Some("2026-11-08T19:00:00Z".into()),
                factions: vec![faction(Some(75.0))],
                conflicts: vec![conflict],
            }),
            overview: Some(Overview {
                source: Source::Journal,
                as_of: Some("2026-11-08T21:00:00Z".into()),
                allegiance: Some("Federation".into()),
                government: None,
                economy: None,
                second_economy: None,
                security: None,
                population: None,
                controlling_faction: None,
                controlling_faction_state: None,
                permit: None,
            }),
            ..Default::default()
        };
        let spansh = SystemDetails {
            factions: Some(Factions {
                source: Source::Spansh,
                as_of: Some("2026-11-08T19:30:00Z".into()),
                factions: vec![faction(None)],
                conflicts: vec![],
            }),
            overview: Some(Overview {
                source: Source::Spansh,
                as_of: Some("2026-11-08T20:36:00Z".into()),
                allegiance: Some("Empire".into()),
                government: None,
                economy: None,
                second_economy: None,
                security: None,
                population: None,
                controlling_faction: None,
                controlling_faction_state: None,
                permit: Some("Required".into()),
            }),
            ..Default::default()
        };
        let m = merge(journal, spansh);
        let f = m.factions.unwrap();
        assert_eq!(f.source, Source::Spansh);
        assert_eq!(f.factions[0].my_reputation, Some(75.0));
        assert_eq!(f.conflicts.len(), 1);
        let o = m.overview.unwrap();
        assert_eq!(o.source, Source::Journal);
        assert_eq!(o.allegiance.as_deref(), Some("Federation"), "newer value kept");
        assert_eq!(o.permit.as_deref(), Some("Required"), "gap filled from the older side");
    }
}
