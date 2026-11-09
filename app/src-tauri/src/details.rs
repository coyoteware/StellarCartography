//! Online system details: fetches a system from Spansh or EDSM, keeps the
//! raw answers in a small SQLite cache, and falls back to the other site or
//! to a stale cached copy when a lookup fails. Mapping the answers onto the
//! shared model lives in `galaxy_core::details`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use galaxy_core::details::{self, iso_from_unix, RemoteStatus, Source, SystemDetails};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Which site to ask first (settings). The other one is the fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnlineSource {
    #[default]
    Spansh,
    Edsm,
    /// Journals only: no network requests.
    Off,
}

/// Answers younger than this are served from the cache without asking again.
const FRESH: Duration = Duration::from_secs(60 * 60);
/// Cached answers older than this are deleted at startup.
const KEEP_DAYS: i64 = 30;
/// Minimum gap between requests to the same site.
const MIN_GAP: Duration = Duration::from_millis(250);

pub struct Online {
    client: reqwest::Client,
    cache: Option<Mutex<Connection>>,
    last_request: Mutex<HashMap<&'static str, Instant>>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Online {
    pub fn new(cache_path: &Path) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(ingest::USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("HTTP client");
        let cache = open_cache(cache_path).map_err(|e| eprintln!("details cache unavailable: {e:#}")).ok().map(Mutex::new);
        Self { client, cache, last_request: Mutex::new(HashMap::new()) }
    }

    /// Looks the system up on `first`, and on the other site unless `first`
    /// gave a fresh answer: when it failed, only had an old cached copy, or
    /// has no record of the system. A stale copy still fills the gaps of the
    /// other site's answer (Powerplay, pads). The statuses say what happened
    /// with each attempt.
    pub async fn lookup(&self, first: OnlineSource, id64: Option<u64>, name: &str, refresh: bool) -> SystemDetails {
        let order = match first {
            OnlineSource::Off => return SystemDetails::default(),
            OnlineSource::Spansh => [Source::Spansh, Source::Edsm],
            OnlineSource::Edsm => [Source::Edsm, Source::Spansh],
        };
        let mut statuses = Vec::new();
        let mut stale: Option<SystemDetails> = None;
        for source in order {
            match self.lookup_one(source, id64, name, refresh).await {
                Ok((d, status)) if status.ok && !status.missing => {
                    statuses.push(status);
                    let d = match stale.take() {
                        Some(s) => details::merge(d, s),
                        None => d,
                    };
                    return SystemDetails { remote: statuses, ..d };
                }
                Ok((d, status)) => {
                    statuses.push(status);
                    if !d.is_empty() && stale.is_none() {
                        stale = Some(d);
                    }
                }
                Err(e) => statuses.push(RemoteStatus {
                    source,
                    ok: false,
                    missing: false,
                    error: Some(format!("{e:#}")),
                    fetched_at: None,
                }),
            }
        }
        SystemDetails { remote: statuses, ..stale.unwrap_or_default() }
    }

    async fn lookup_one(&self, source: Source, id64: Option<u64>, name: &str, refresh: bool) -> Result<(SystemDetails, RemoteStatus)> {
        let key = match id64 {
            Some(id) => id.to_string(),
            None if source == Source::Spansh => bail!("Spansh needs the system's id64"),
            None => format!("name:{}", name.to_lowercase()),
        };
        // A successful answer with nothing in it means the site has no record.
        let answered = |d: SystemDetails, fetched: i64| {
            let missing = d.is_empty();
            (d, RemoteStatus { source, ok: true, missing, error: None, fetched_at: Some(iso_from_unix(fetched)) })
        };
        let cached = self.cached(source, &key);
        if let Some((fetched, body)) = &cached {
            if !refresh && now() - fetched < FRESH.as_secs() as i64 {
                return Ok(answered(parse(source, body)?, *fetched));
            }
        }
        match self.fetch(source, id64, name).await {
            Ok(body) => {
                // Parse before caching, so a garbled answer isn't kept.
                let d = parse(source, &body)?;
                self.store(source, &key, &body);
                Ok(answered(d, now()))
            }
            Err(e) if e.is::<NotFound>() => Ok(answered(SystemDetails::default(), now())),
            Err(e) => match cached {
                Some((fetched, body)) => {
                    let d = parse(source, &body)?;
                    let error = format!("{e:#}; showing the copy from {}", iso_from_unix(fetched));
                    let status = RemoteStatus {
                        source,
                        ok: false,
                        missing: false,
                        error: Some(error),
                        fetched_at: Some(iso_from_unix(fetched)),
                    };
                    Ok((d, status))
                }
                None => Err(e),
            },
        }
    }

    async fn fetch(&self, source: Source, id64: Option<u64>, name: &str) -> Result<String> {
        match source {
            Source::Spansh => {
                let id64 = id64.ok_or_else(|| anyhow!("Spansh needs the system's id64"))?;
                self.get("Spansh", &details::spansh::url(id64)).await
            }
            Source::Edsm => {
                let [system, factions, stations] = details::edsm::urls(id64, name);
                let body = EdsmBody {
                    system: self.get("EDSM", &system).await?,
                    factions: self.get("EDSM", &factions).await?,
                    stations: self.get("EDSM", &stations).await?,
                };
                Ok(serde_json::to_string(&body)?)
            }
            Source::Journal => bail!("not an online source"),
        }
    }

    async fn get(&self, site: &'static str, url: &str) -> Result<String> {
        self.throttle(site).await;
        let resp = self.client.get(url).send().await.map_err(|e| {
            if e.is_timeout() {
                anyhow!("{site} didn't answer in time")
            } else if e.is_connect() {
                anyhow!("couldn't reach {site} (offline?)")
            } else {
                anyhow!("{site} request failed: {e}")
            }
        })?;
        let status = resp.status();
        match status.as_u16() {
            200..=299 => resp.text().await.with_context(|| format!("reading the {site} answer")),
            404 => Err(NotFound.into()),
            429 => bail!("{site} is rate limiting requests; try again in a minute"),
            // Spansh answers systems it doesn't have with a 500 and a JSON
            // `{"error": ...}` body; a real outage tends to be an HTML page.
            500 if site == "Spansh" => {
                let body = resp.text().await.unwrap_or_default();
                let json_error = serde_json::from_str::<serde_json::Value>(&body).is_ok_and(|v| v.get("error").is_some());
                if json_error {
                    Err(NotFound.into())
                } else {
                    bail!("{site} answered HTTP {status}")
                }
            }
            _ => bail!("{site} answered HTTP {status}"),
        }
    }

    async fn throttle(&self, site: &'static str) {
        let wait = {
            let mut last = self.last_request.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let next = last.get(site).map_or(now, |t| (*t + MIN_GAP).max(now));
            last.insert(site, next);
            next - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    fn cached(&self, source: Source, key: &str) -> Option<(i64, String)> {
        let db = self.cache.as_ref()?.lock().ok()?;
        db.query_row(
            "SELECT fetched_at, body FROM remote WHERE source = ?1 AND key = ?2",
            [source_key(source), key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()?
    }

    fn store(&self, source: Source, key: &str, body: &str) {
        let Some(db) = self.cache.as_ref().and_then(|c| c.lock().ok()) else { return };
        let r = db.execute(
            "INSERT OR REPLACE INTO remote (source, key, fetched_at, body) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![source_key(source), key, now(), body],
        );
        if let Err(e) = r {
            eprintln!("details cache write failed: {e}");
        }
    }
}

/// The site has no record of the system.
#[derive(Debug)]
struct NotFound;

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("no record of this system")
    }
}

impl std::error::Error for NotFound {}

/// The three EDSM answers, cached together as one JSON object.
#[derive(Serialize, Deserialize)]
struct EdsmBody {
    system: String,
    factions: String,
    stations: String,
}

fn parse(source: Source, body: &str) -> Result<SystemDetails> {
    match source {
        Source::Spansh => details::spansh::parse(body),
        Source::Edsm => {
            let b: EdsmBody = serde_json::from_str(body).context("damaged EDSM cache entry")?;
            details::edsm::parse(&b.system, &b.factions, &b.stations)
        }
        Source::Journal => bail!("not an online source"),
    }
}

fn source_key(source: Source) -> &'static str {
    match source {
        Source::Spansh => "spansh",
        Source::Edsm => "edsm",
        Source::Journal => "journal",
    }
}

fn open_cache(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let db = Connection::open(path)?;
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS remote (
             source TEXT NOT NULL,
             key TEXT NOT NULL,
             fetched_at INTEGER NOT NULL,
             body TEXT NOT NULL,
             PRIMARY KEY (source, key)
         );",
    )?;
    db.execute("DELETE FROM remote WHERE fetched_at < ?1", [now() - KEEP_DAYS * 86_400])?;
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trip_and_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("cache.sqlite");
        let online = Online::new(&path);
        assert!(online.cached(Source::Spansh, "1").is_none());
        online.store(Source::Spansh, "1", "{}");
        let (fetched, body) = online.cached(Source::Spansh, "1").unwrap();
        assert_eq!(body, "{}");
        assert!(now() - fetched < 5);
        assert!(online.cached(Source::Edsm, "1").is_none(), "sources don't share entries");

        // Entries past KEEP_DAYS go at the next start.
        online
            .cache
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .execute("UPDATE remote SET fetched_at = 0", [])
            .unwrap();
        drop(online);
        assert!(Online::new(&path).cached(Source::Spansh, "1").is_none());
    }

    #[test]
    fn edsm_cache_entries_parse() {
        let body = serde_json::to_string(&EdsmBody { system: "[]".into(), factions: "[]".into(), stations: "[]".into() }).unwrap();
        assert_eq!(parse(Source::Edsm, &body).unwrap(), SystemDetails::default());
        assert!(parse(Source::Edsm, "garbage").is_err());
    }

    #[test]
    fn off_makes_no_requests() {
        let dir = tempfile::tempdir().unwrap();
        let online = Online::new(&dir.path().join("c.sqlite"));
        let d = tauri::async_runtime::block_on(online.lookup(OnlineSource::Off, Some(1), "x", false));
        assert_eq!(d, SystemDetails::default());
    }

    #[test]
    fn fresh_cache_is_served_without_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let online = Online::new(&dir.path().join("c.sqlite"));
        let body = r#"{"record":{"id64":7,"name":"Cached","population":5,"updated_at":"2026-11-08T00:00:00Z"}}"#;
        online.store(Source::Spansh, "7", body);
        let d = tauri::async_runtime::block_on(online.lookup(OnlineSource::Spansh, Some(7), "Cached", false));
        assert_eq!(d.overview.unwrap().population, Some(5));
        assert_eq!(d.remote.len(), 1);
        assert!(d.remote[0].ok && !d.remote[0].missing);
    }

    #[test]
    fn an_empty_cached_answer_is_missing_and_falls_through() {
        let dir = tempfile::tempdir().unwrap();
        let online = Online::new(&dir.path().join("c.sqlite"));
        let empty = serde_json::to_string(&EdsmBody { system: "[]".into(), factions: "[]".into(), stations: "[]".into() }).unwrap();
        online.store(Source::Edsm, "7", &empty);
        let body = r#"{"record":{"id64":7,"name":"Cached","population":5,"updated_at":"2026-11-08T00:00:00Z"}}"#;
        online.store(Source::Spansh, "7", body);
        let d = tauri::async_runtime::block_on(online.lookup(OnlineSource::Edsm, Some(7), "Cached", false));
        assert!(d.remote[0].missing, "EDSM has no record");
        assert_eq!(d.remote[1].source, Source::Spansh);
        assert_eq!(d.overview.unwrap().population, Some(5), "answered by the other site");
    }
}
