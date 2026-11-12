//! Last known positions of other commanders, from EDSM's
//! `api-logs-v1/get-position`. The game never exposes player positions, so
//! only commanders who upload their journals to EDSM (EDMC, EDDiscovery...)
//! and leave their flight log public are visible. EDSM answers `msgnum` 203
//! for a name it doesn't know, and `null` fields for what a profile keeps
//! private, so a position can come with no date or no coordinates.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::details::url_encode;
use crate::Vec3;

pub fn url(name: &str) -> String {
    format!("https://www.edsm.net/api-logs-v1/get-position?commanderName={}&showCoordinates=1", url_encode(name))
}

/// What EDSM last knew about one commander.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendPosition {
    pub name: String,
    /// False when EDSM has no commander by this name (or the profile hides
    /// everything); the other fields are then empty.
    pub known: bool,
    pub system: Option<String>,
    /// In-game coordinates (ly); missing when the system has none on EDSM.
    pub pos: Option<Vec3>,
    /// When EDSM last saw them there ("YYYY-MM-DD HH:MM:SS" UTC); private
    /// profiles hide it.
    pub date: Option<String>,
    /// The station they were docked at, when docked.
    pub docked_at: Option<String>,
    pub ship: Option<String>,
    /// Their EDSM profile page.
    pub profile_url: Option<String>,
}

impl FriendPosition {
    fn unknown(name: &str) -> Self {
        Self {
            name: name.into(),
            known: false,
            system: None,
            pos: None,
            date: None,
            docked_at: None,
            ship: None,
            profile_url: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    msgnum: i64,
    msg: Option<String>,
    system: Option<String>,
    coordinates: Option<Coordinates>,
    is_docked: Option<bool>,
    station: Option<String>,
    ship_type: Option<String>,
    date: Option<String>,
    date_last_activity: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize)]
struct Coordinates {
    x: f64,
    y: f64,
    z: f64,
}

/// Reads one `get-position` answer. `Err` only for a damaged or refused
/// answer; a commander EDSM doesn't know comes back with `known: false`.
pub fn parse(name: &str, body: &str) -> Result<FriendPosition> {
    let r: Response = serde_json::from_str(body).context("unexpected EDSM answer")?;
    match r.msgnum {
        100 => {}
        203 => return Ok(FriendPosition::unknown(name)),
        n => bail!("EDSM answered {n}: {}", r.msg.unwrap_or_default()),
    }
    Ok(FriendPosition {
        name: name.into(),
        known: true,
        system: r.system,
        pos: r.coordinates.map(|c| [c.x as f32, c.y as f32, c.z as f32]),
        date: r.date.or(r.date_last_activity),
        docked_at: r.station.filter(|_| r.is_docked == Some(true)),
        ship: r.ship_type,
        profile_url: r.url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_request_url() {
        assert_eq!(
            url("Jameson & Co"),
            "https://www.edsm.net/api-logs-v1/get-position?commanderName=Jameson%20%26%20Co&showCoordinates=1"
        );
    }

    #[test]
    fn parses_a_public_position() {
        // A real answer (date withheld by the profile's privacy settings).
        let body = r#"{"msgnum":100,"msg":"OK","system":"Stuemeae FG-Y d7561","firstDiscover":false,"date":null,
            "coordinates":{"x":28.6875,"y":-19.78125,"z":25899.6875},"isDocked":true,"station":"Explorer's Anchorage",
            "dateDocked":null,"shipId":0,"shipType":"Asp Explorer","shipFuel":null,"dateLastActivity":null,
            "url":"https:\/\/www.edsm.net\/en\/user\/profile\/id\/952\/cmdr\/Anthor"}"#;
        let p = parse("Anthor", body).unwrap();
        assert!(p.known);
        assert_eq!(p.system.as_deref(), Some("Stuemeae FG-Y d7561"));
        assert_eq!(p.pos, Some([28.6875, -19.78125, 25_899.688]));
        assert_eq!(p.docked_at.as_deref(), Some("Explorer's Anchorage"));
        assert_eq!(p.ship.as_deref(), Some("Asp Explorer"));
        assert_eq!(p.date, None);
        assert_eq!(p.profile_url.as_deref(), Some("https://www.edsm.net/en/user/profile/id/952/cmdr/Anthor"));
    }

    #[test]
    fn a_dated_position_without_coordinates_still_parses() {
        let body = r#"{"msgnum":100,"msg":"OK","system":"Sol","date":"2026-11-01 10:00:00","isDocked":false}"#;
        let p = parse("X", body).unwrap();
        assert!(p.known);
        assert_eq!(p.system.as_deref(), Some("Sol"));
        assert_eq!(p.pos, None);
        assert_eq!(p.date.as_deref(), Some("2026-11-01 10:00:00"));
        assert_eq!(p.docked_at, None);
    }

    #[test]
    fn an_unknown_commander_is_not_an_error() {
        let p = parse("Nobody", r#"{"msgnum":203,"msg":"Commander name\/API Key not found"}"#).unwrap();
        assert_eq!(p, FriendPosition::unknown("Nobody"));
    }

    #[test]
    fn refusals_and_garbage_are_errors() {
        assert!(parse("X", r#"{"msgnum":429,"msg":"Rate limit reached"}"#).is_err());
        assert!(parse("X", "<html>").is_err());
    }
}
