//! Route plotting via Spansh's routers (<https://spansh.co.uk/plotter>).
//!
//! Three of them: the neutron plotter (`api/route`), the galaxy plotter
//! (`api/generic/route`, needs the ship's fuel model) and the fleet-carrier
//! router (`api/fleetcarrier/route`). Each takes a form-encoded POST,
//! answers `{"job": ...}`, and the result is polled at `api/results/<job>`
//! until its status turns `ok`. The answers all become one [`Waypoint`]
//! shape for the map and the route panel.
//!
//! The plotted route is kept in `route.json` next to the settings, so it
//! survives a restart; the web view owns its progress along it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

const API: &str = "https://spansh.co.uk/api";
/// How often a queued job is asked about.
const POLL: Duration = Duration::from_millis(1500);
/// A job still queued after this long is given up on.
const TIMEOUT: Duration = Duration::from_secs(300);

/// What the frontend asks to plot.
#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RouteRequest {
    /// The neutron plotter: fast long-range legs between neutron stars.
    Neutron { from: String, to: String, range: f64, efficiency: u32 },
    /// The galaxy plotter: every jump, fuel-exact, needs the ship.
    Galaxy {
        source: String,
        destination: String,
        ship: galaxy_core::ship::ShipSpec,
        cargo: u32,
        is_supercharged: bool,
        use_supercharge: bool,
        use_injections: bool,
        exclude_secondary: bool,
        refuel_every_scoopable: bool,
    },
    /// The fleet-carrier router: 500 ly hops and tritium stops.
    Carrier {
        source: String,
        destinations: Vec<String>,
        capacity_used: u32,
        calculate_starting_fuel: bool,
        /// Squadron carriers hold more and weigh less.
        squadron: bool,
    },
}

impl RouteRequest {
    fn endpoint(&self) -> &'static str {
        match self {
            Self::Neutron { .. } => "route",
            Self::Galaxy { .. } => "generic/route",
            Self::Carrier { .. } => "fleetcarrier/route",
        }
    }

    fn mode(&self) -> &'static str {
        match self {
            Self::Neutron { .. } => "neutron",
            Self::Galaxy { .. } => "galaxy",
            Self::Carrier { .. } => "carrier",
        }
    }

    fn form(&self) -> Vec<(&'static str, String)> {
        let b = |v: bool| (v as u8).to_string();
        match self {
            Self::Neutron { from, to, range, efficiency } => vec![
                ("from", from.clone()),
                ("to", to.clone()),
                ("range", range.to_string()),
                ("efficiency", efficiency.to_string()),
            ],
            Self::Galaxy {
                source,
                destination,
                ship,
                cargo,
                is_supercharged,
                use_supercharge,
                use_injections,
                exclude_secondary,
                refuel_every_scoopable,
            } => vec![
                ("source", source.clone()),
                ("destination", destination.clone()),
                ("is_supercharged", b(*is_supercharged)),
                ("use_supercharge", b(*use_supercharge)),
                ("use_injections", b(*use_injections)),
                ("exclude_secondary", b(*exclude_secondary)),
                ("refuel_every_scoopable", b(*refuel_every_scoopable)),
                ("fuel_power", ship.fuel_power.to_string()),
                ("fuel_multiplier", ship.fuel_multiplier.to_string()),
                ("optimal_mass", ship.optimal_mass.to_string()),
                ("base_mass", ship.base_mass.to_string()),
                ("tank_size", ship.tank_size.to_string()),
                ("internal_tank_size", ship.internal_tank_size.to_string()),
                ("max_fuel_per_jump", ship.max_fuel_per_jump.to_string()),
                ("range_boost", ship.range_boost.to_string()),
                ("supercharge_multiplier", ship.supercharge_multiplier.to_string()),
                ("cargo", cargo.to_string()),
                ("algorithm", "optimistic".into()),
            ],
            Self::Carrier { source, destinations, capacity_used, calculate_starting_fuel, squadron } => {
                let mut form = vec![
                    ("source", source.clone()),
                    ("capacity", if *squadron { "60000" } else { "25000" }.into()),
                    ("mass", if *squadron { "15000" } else { "25000" }.into()),
                    ("capacity_used", capacity_used.to_string()),
                    ("calculate_starting_fuel", b(*calculate_starting_fuel)),
                ];
                form.extend(destinations.iter().map(|d| ("destinations", d.clone())));
                form
            }
        }
    }
}

/// One stop on a plotted route; which extras are filled depends on the plotter.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Waypoint {
    pub name: String,
    #[serde(serialize_with = "galaxy_core::serialize_id64", deserialize_with = "id64_from_saved")]
    pub id64: Option<u64>,
    pub pos: [f64; 3],
    /// Light years from the previous waypoint.
    pub distance: f64,
    /// Light years still to go after this one.
    pub distance_left: f64,
    /// Neutron plotter: estimated jumps to get here from the previous waypoint.
    pub jumps: Option<u32>,
    pub neutron: bool,
    pub scoopable: bool,
    /// Scoop here (galaxy plotter) or load tritium here (carrier).
    pub refuel: bool,
    /// Carrier: tonnes of tritium to load here.
    pub restock: Option<f64>,
    pub icy_ring: bool,
    pub pristine: bool,
    pub fuel_used: Option<f64>,
    pub fuel_left: Option<f64>,
    /// Carrier: one of the systems the route was asked to reach.
    pub destination: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlottedRoute {
    /// "neutron", "galaxy" or "carrier".
    pub mode: String,
    pub source: String,
    pub destination: String,
    pub waypoints: Vec<Waypoint>,
    /// Light years along the route.
    pub total_distance: f64,
    /// Neutron: Spansh's estimate of real jumps; others: one per waypoint.
    pub total_jumps: Option<u32>,
    /// Unix seconds.
    pub plotted_at: i64,
}

/// The active route as kept on disk; progress belongs to the web view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedRoute {
    pub route: PlottedRoute,
    /// Index of the waypoint the commander has reached.
    pub progress: usize,
    /// Copy the next waypoint's name to the clipboard after each jump.
    pub copy_next: bool,
}

pub struct Plotter {
    client: reqwest::Client,
    /// Bumped by every new plot and by cancel: an older plot sees it and stops polling.
    generation: AtomicU64,
}

impl Default for Plotter {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(ingest::USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client");
        Self { client, generation: AtomicU64::new(0) }
    }
}

/// Spansh wraps every answer in this; a failed job carries `error`.
#[derive(Deserialize)]
struct JobReply {
    job: Option<String>,
    status: Option<String>,
    error: Option<String>,
    result: Option<serde_json::Value>,
}

impl Plotter {
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Submits the route to Spansh and polls until it's done. A later plot or
    /// a cancel makes this stop with a "cancelled" error.
    pub async fn plot(&self, request: RouteRequest) -> Result<PlottedRoute> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let url = format!("{API}/{}", request.endpoint());
        let reply = self.send(self.client.post(&url).form(&request.form())).await?;
        let job = match reply {
            JobReply { error: Some(e), .. } => bail!("Spansh: {e}"),
            JobReply { job: Some(job), .. } => job,
            _ => bail!("Spansh didn't accept the route (no job in the answer)"),
        };
        let started = Instant::now();
        loop {
            tokio::time::sleep(POLL).await;
            if self.generation.load(Ordering::Relaxed) != generation {
                bail!("cancelled");
            }
            let reply = self.send(self.client.get(format!("{API}/results/{job}"))).await?;
            match reply {
                JobReply { error: Some(e), .. } => bail!("Spansh: {e}"),
                JobReply { status: Some(s), result: Some(result), .. } if s == "ok" => {
                    return parse_result(&request, result);
                }
                _ if started.elapsed() > TIMEOUT => {
                    bail!("Spansh is taking too long (the queue may be full); try again later")
                }
                _ => {} // still queued
            }
        }
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<JobReply> {
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                anyhow!("Spansh didn't answer in time")
            } else if e.is_connect() {
                anyhow!("couldn't reach Spansh (offline?)")
            } else {
                anyhow!("Spansh request failed: {e}")
            }
        })?;
        let status = resp.status();
        let body = resp.text().await.context("reading the Spansh answer")?;
        // Errors come with a JSON body too ("Could not find system ...");
        // prefer that message to the bare status code.
        match serde_json::from_str::<JobReply>(&body) {
            Ok(reply) => Ok(reply),
            Err(_) if status.as_u16() == 429 => bail!("Spansh is rate limiting requests; try again in a minute"),
            Err(_) => bail!("Spansh answered HTTP {status}"),
        }
    }
}

// --- Result parsing ------------------------------------------------------------

/// An id64, however the plotter spells it (the neutron plotter sends a
/// number, the others strings).
fn id64_value(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// `deserialize_with` twin of [`galaxy_core::serialize_id64`] for re-reading `route.json`.
fn id64_from_saved<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => id64_value(&v).map(Some).ok_or_else(|| D::Error::custom("not an id64")),
    }
}

/// A flag however it's spelt (`true`, or `1` in the carrier answers).
fn flag(v: Option<&serde_json::Value>) -> bool {
    match v {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        _ => false,
    }
}

fn number(v: Option<&serde_json::Value>) -> Option<f64> {
    v.and_then(serde_json::Value::as_f64)
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn parse_result(request: &RouteRequest, result: serde_json::Value) -> Result<PlottedRoute> {
    let (stops, leg_key) = match request {
        RouteRequest::Neutron { .. } => ("system_jumps", "distance_jumped"),
        _ => ("jumps", "distance"),
    };
    let stops = result
        .get(stops)
        .and_then(|j| j.as_array())
        .with_context(|| format!("unexpected Spansh answer (no {stops})"))?;
    let mut waypoints = Vec::with_capacity(stops.len());
    for s in stops {
        let name = s
            .get("system")
            .or_else(|| s.get("name"))
            .and_then(|v| v.as_str())
            .context("unexpected Spansh answer (a stop with no name)")?
            .to_string();
        let pos = ["x", "y", "z"].map(|k| number(s.get(k)).unwrap_or(0.0));
        // A carrier can also sell tritium out of its market.
        let fuel_left = number(s.get("fuel_in_tank")).map(|tank| tank + number(s.get("tritium_in_market")).unwrap_or(0.0));
        waypoints.push(Waypoint {
            name,
            id64: s.get("id64").and_then(id64_value),
            pos,
            distance: number(s.get(leg_key)).unwrap_or(0.0),
            distance_left: number(s.get("distance_left")).or_else(|| number(s.get("distance_to_destination"))).unwrap_or(0.0),
            jumps: number(s.get("jumps")).map(|j| j as u32),
            neutron: flag(s.get("neutron_star")) || flag(s.get("has_neutron")),
            scoopable: flag(s.get("is_scoopable")),
            refuel: flag(s.get("must_refuel")) || flag(s.get("must_restock")),
            restock: number(s.get("restock_amount")).filter(|r| *r > 0.0),
            icy_ring: flag(s.get("has_icy_ring")),
            pristine: flag(s.get("is_system_pristine")),
            fuel_used: number(s.get("fuel_used")),
            fuel_left,
            destination: flag(s.get("is_desired_destination")),
        });
    }
    if waypoints.is_empty() {
        bail!("Spansh answered with an empty route");
    }
    let total_distance = match number(result.get("distance")) {
        Some(d) => d,
        None => waypoints.iter().map(|w| w.distance).sum(),
    };
    let total_jumps = match number(result.get("total_jumps")) {
        Some(j) => Some(j as u32),
        None => Some(waypoints.len().saturating_sub(1) as u32),
    };
    Ok(PlottedRoute {
        mode: request.mode().into(),
        source: waypoints.first().map(|w| w.name.clone()).unwrap_or_default(),
        destination: waypoints.last().map(|w| w.name.clone()).unwrap_or_default(),
        waypoints,
        total_distance,
        total_jumps,
        plotted_at: now(),
    })
}

// --- The saved route -----------------------------------------------------------

pub fn load_saved(path: &PathBuf) -> Option<SavedRoute> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(route) => Some(route),
        Err(e) => {
            eprintln!("{}: {e}; dropping the saved route", path.display());
            None
        }
    }
}

pub fn store_saved(path: &PathBuf, route: Option<&SavedRoute>) -> Result<()> {
    match route {
        Some(route) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(route)?)?;
            std::fs::rename(&tmp, path)?;
        }
        None => {
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(e.into());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neutron_request() -> RouteRequest {
        RouteRequest::Neutron { from: "Sol".into(), to: "Colonia".into(), range: 50.0, efficiency: 60 }
    }

    #[test]
    fn neutron_results_parse() {
        let result: serde_json::Value = serde_json::json!({
            "distance": 22000.5, "total_jumps": 471,
            "system_jumps": [
                {"system": "Sol", "id64": 10477373803u64, "x": 0, "y": 0, "z": 0,
                 "distance_jumped": 0, "distance_left": 22000.5, "jumps": 0, "neutron_star": false},
                {"system": "PSR J1752-2806", "id64": 22712681061u64, "x": -10.96875, "y": -6.84375, "z": 407.28125,
                 "distance_jumped": 407.4, "distance_left": 21629.3, "jumps": 10, "neutron_star": true},
            ],
        });
        let r = parse_result(&neutron_request(), result).unwrap();
        assert_eq!(r.mode, "neutron");
        assert_eq!(r.source, "Sol");
        assert_eq!(r.destination, "PSR J1752-2806");
        assert_eq!(r.total_distance, 22000.5);
        assert_eq!(r.total_jumps, Some(471));
        assert_eq!(r.waypoints.len(), 2);
        let w = &r.waypoints[1];
        assert_eq!(w.id64, Some(22712681061));
        assert!(w.neutron);
        assert_eq!(w.jumps, Some(10));
        assert_eq!(w.distance, 407.4);
    }

    #[test]
    fn carrier_results_parse_with_string_id64s_and_numeric_flags() {
        let result: serde_json::Value = serde_json::json!({
            "jumps": [
                {"name": "Sol", "id64": "10477373803", "x": 0, "y": 0, "z": 0, "distance": 0,
                 "distance_to_destination": 500.0, "fuel_in_tank": 900, "tritium_in_market": 100, "fuel_used": 0,
                 "has_icy_ring": false, "is_system_pristine": false, "must_restock": 1, "restock_amount": 230,
                 "is_desired_destination": 0},
                {"name": "Colonia", "id64": "3238296097059", "x": -9530.5, "y": -910.28125, "z": 19808.125,
                 "distance": 500.0, "distance_to_destination": 0, "fuel_in_tank": 810, "tritium_in_market": 0,
                 "fuel_used": 90, "has_icy_ring": true, "is_system_pristine": true, "must_restock": 0,
                 "restock_amount": 0, "is_desired_destination": 1},
            ],
        });
        let request = RouteRequest::Carrier {
            source: "Sol".into(),
            destinations: vec!["Colonia".into()],
            capacity_used: 0,
            calculate_starting_fuel: false,
            squadron: false,
        };
        let r = parse_result(&request, result).unwrap();
        assert_eq!(r.total_distance, 500.0);
        assert_eq!(r.total_jumps, Some(1));
        let (sol, colonia) = (&r.waypoints[0], &r.waypoints[1]);
        assert_eq!(sol.id64, Some(10477373803));
        assert!(sol.refuel, "must_restock arrives as 1");
        assert_eq!(sol.restock, Some(230.0));
        assert_eq!(sol.fuel_left, Some(1000.0), "tank plus market");
        assert!(colonia.destination);
        assert!(colonia.icy_ring && colonia.pristine);
        assert_eq!(colonia.restock, None, "zero restock is no restock");
    }

    #[test]
    fn empty_or_malformed_results_are_errors() {
        assert!(parse_result(&neutron_request(), serde_json::json!({})).is_err());
        assert!(parse_result(&neutron_request(), serde_json::json!({"system_jumps": []})).is_err());
        assert!(parse_result(&neutron_request(), serde_json::json!({"system_jumps": [{"x": 1}]})).is_err());
    }

    #[test]
    fn requests_become_the_forms_spansh_expects() {
        let form = neutron_request().form();
        assert!(form.contains(&("from", "Sol".into())));
        assert!(form.contains(&("efficiency", "60".into())));

        let request = RouteRequest::Carrier {
            source: "Sol".into(),
            destinations: vec!["Alpha".into(), "Beta".into()],
            capacity_used: 12000,
            calculate_starting_fuel: true,
            squadron: true,
        };
        let form = request.form();
        assert!(form.contains(&("capacity", "60000".into())));
        assert!(form.contains(&("mass", "15000".into())));
        assert!(form.contains(&("calculate_starting_fuel", "1".into())));
        assert_eq!(form.iter().filter(|(k, _)| *k == "destinations").count(), 2, "one field per destination");
    }

    /// The whole submit-poll-parse flow against the live site; run by hand
    /// with `cargo test -p stellar-cartography -- --ignored`.
    #[test]
    #[ignore = "talks to spansh.co.uk"]
    fn live_neutron_plot() {
        let plotter = Plotter::default();
        let request =
            RouteRequest::Neutron { from: "Sol".into(), to: "Alpha Centauri".into(), range: 20.0, efficiency: 60 };
        let r = tauri::async_runtime::block_on(plotter.plot(request)).unwrap();
        assert_eq!(r.mode, "neutron");
        assert_eq!(r.source, "Sol");
        assert_eq!(r.destination, "Alpha Centauri");
        assert!(r.waypoints.len() >= 2);
        assert_eq!(r.waypoints[0].id64, Some(10477373803));
    }

    #[test]
    fn saved_routes_round_trip_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("route.json");
        assert_eq!(load_saved(&path), None);
        let saved = SavedRoute {
            route: PlottedRoute {
                mode: "neutron".into(),
                source: "Sol".into(),
                destination: "Colonia".into(),
                waypoints: vec![Waypoint { name: "Sol".into(), id64: Some(10477373803), ..Default::default() }],
                total_distance: 22000.5,
                total_jumps: Some(471),
                plotted_at: 1_700_000_000,
            },
            progress: 0,
            copy_next: true,
        };
        store_saved(&path, Some(&saved)).unwrap();
        assert_eq!(load_saved(&path), Some(saved));
        store_saved(&path, None).unwrap();
        assert_eq!(load_saved(&path), None);
        store_saved(&path, None).unwrap(); // clearing twice is fine
    }
}
