//! The game's `VisitedStarsCache.dat`: every system the account has visited.
//!
//! Found at `%LOCALAPPDATA%\Frontier Developments\Elite Dangerous\<id>\VisitedStarsCache.dat`,
//! and also what the Companion API's `/visitedstars` endpoint returns (zipped).
//! The format is undocumented; this layout was worked out from a real file
//! (version 200, October 2026):
//!
//! ```text
//! 0   [u8; 16]  "VisitedStars" + 4 NULs
//! 16  u32       version (200)
//! 20  u32       header length (48)
//! 24  u32       record count n
//! 28  u32       record length (16)
//! 32  u64 x 2   account/commander ids (one matches the folder name)
//! 48  n records: u64 id64, u32 visit count, u32 last visit (days since 1601-01-01)
//! ..  u32 x 2   0x5AFEC0DE end marker
//! ```

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::Serialize;

pub const MAGIC: &[u8; 12] = b"VisitedStars";
/// Days from 1601-01-01 (the Windows FILETIME epoch) to 1970-01-01.
pub const DAYS_1601_TO_1970: u32 = 134_774;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisitedEntry {
    #[serde(serialize_with = "serialize_u64_str")]
    pub id64: u64,
    pub visits: u32,
    /// Days since 1601-01-01.
    pub last_visit_day: u32,
}

fn serialize_u64_str<S: serde::Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
    crate::serialize_id64(&Some(*v), s)
}

impl VisitedEntry {
    /// Last visit as days since the Unix epoch.
    pub fn last_visit_unix_day(&self) -> i64 {
        i64::from(self.last_visit_day) - i64::from(DAYS_1601_TO_1970)
    }
}

#[derive(Debug, Clone)]
pub struct VisitedCache {
    pub version: u32,
    pub ids: [u64; 2],
    pub entries: Vec<VisitedEntry>,
}

pub fn parse(data: &[u8]) -> Result<VisitedCache> {
    if data.len() < 48 || &data[..12] != MAGIC {
        bail!("not a VisitedStarsCache file");
    }
    let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
    let version = u32_at(16);
    let header = u32_at(20) as usize;
    let count = u32_at(24) as usize;
    let record = u32_at(28) as usize;
    if header < 48 || record < 16 {
        bail!("unexpected layout: header {header} bytes, records {record} bytes");
    }
    let end = header + count * record;
    if data.len() < end {
        bail!("truncated: {count} records need {end} bytes, file has {}", data.len());
    }
    let entries = (0..count)
        .map(|i| {
            let o = header + i * record;
            VisitedEntry { id64: u64_at(o), visits: u32_at(o + 8), last_visit_day: u32_at(o + 12) }
        })
        .collect();
    Ok(VisitedCache { version, ids: [u64_at(32), u64_at(40)], entries })
}

/// Every `VisitedStarsCache.dat` under the game's local app data folder.
pub fn find_cache_files() -> Vec<PathBuf> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return Vec::new() };
    let root = Path::new(&local).join("Frontier Developments").join("Elite Dangerous");
    let Ok(dirs) = std::fs::read_dir(root) else { return Vec::new() };
    dirs.filter_map(|e| e.ok())
        .map(|e| e.path().join("VisitedStarsCache.dat"))
        .filter(|p| p.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(records: &[(u64, u32, u32)]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"VisitedStars\0\0\0\0");
        for v in [200u32, 48, records.len() as u32, 16] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&379023u64.to_le_bytes());
        d.extend_from_slice(&603885u64.to_le_bytes());
        for &(id64, visits, day) in records {
            d.extend_from_slice(&id64.to_le_bytes());
            d.extend_from_slice(&visits.to_le_bytes());
            d.extend_from_slice(&day.to_le_bytes());
        }
        d.extend_from_slice(&[0xde, 0xc0, 0xfe, 0x5a, 0xde, 0xc0, 0xfe, 0x5a]);
        d
    }

    #[test]
    fn parses_records() {
        let cache = parse(&sample(&[(10477373803, 3, 155525), (3238296097059, 4, 155210)])).unwrap();
        assert_eq!(cache.version, 200);
        assert_eq!(cache.ids, [379023, 603885]);
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.entries[0], VisitedEntry { id64: 10477373803, visits: 3, last_visit_day: 155525 });
        // 155525 days after 1601-01-01 is 2026-10-25, i.e. Unix day 20751.
        assert_eq!(cache.entries[0].last_visit_unix_day(), 20751);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(b"not a cache").is_err());
        let mut truncated = sample(&[(1, 1, 1), (2, 1, 1)]);
        truncated.truncate(60);
        assert!(parse(&truncated).is_err());
    }
}
