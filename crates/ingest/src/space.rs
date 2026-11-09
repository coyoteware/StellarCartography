//! Free-space checks before a build starts, so a short disk fails up front
//! with a clear message instead of part way through.
//!
//! A catalogue build needs room in two places: the new outputs are built in
//! the data folder beside the old ones until the swap, and SQLite sorts its
//! indexes through temporary files in the system temp folder. When both are
//! on the same volume, the volume needs room for both.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

const GIB: f64 = (1u64 << 30) as f64;

/// Outputs per compressed byte of EDSM's dump: 9.75 GiB from 3.70 GB (November 2026).
const OUTPUT_PER_COMPRESSED_BYTE: f64 = 2.9;
/// Outputs per system, for builds stopped early with `--limit` (105 bytes measured).
const OUTPUT_PER_SYSTEM: u64 = 110;
/// When nothing else is known: a full build plus some growth.
const DEFAULT_OUTPUT: u64 = 11 << 30;
/// SQLite's index sort files, as a share of the outputs: they peaked at about
/// 1.4 GiB beside 9.75 GiB of outputs on the full dump (November 2026);
/// rounded up for headroom.
const SORT_TEMP_SHARE: f64 = 0.25;

/// Space a build needs: `data` bytes in the data folder, `temp` in the system temp folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    pub data: u64,
    pub temp: u64,
}

/// What a catalogue build into `out` will need. `compressed` is the dump's
/// size when known, `limit` a record limit.
pub fn catalogue_needs(out: &Path, compressed: Option<u64>, limit: Option<u64>) -> Needs {
    let mut output = previous_outputs(out)
        .map(|b| (b as f64 * 1.1) as u64)
        .or_else(|| compressed.map(|c| (c as f64 * OUTPUT_PER_COMPRESSED_BYTE) as u64))
        .unwrap_or(DEFAULT_OUTPUT);
    if let Some(limit) = limit {
        output = output.min(limit.saturating_mul(OUTPUT_PER_SYSTEM));
    }
    let margin = (output / 10).max(64 << 20);
    Needs { data: output + margin, temp: (output as f64 * SORT_TEMP_SHARE) as u64 }
}

/// What a populated-systems update into `out` will need: the new file is
/// written beside the old one.
pub fn populated_needs(out: &Path) -> Needs {
    let current = std::fs::metadata(out.join(crate::populated::FILE_NAME)).map_or(0, |m| m.len());
    Needs { data: (2 * current).max(50 << 20) + (200 << 20), temp: 0 }
}

/// The previous build's output sizes, from its `ingest-stats.json`.
fn previous_outputs(out: &Path) -> Option<u64> {
    #[derive(Deserialize)]
    struct Sizes {
        sqlite_bytes: u64,
        tiles_bytes: u64,
        density_bytes: u64,
    }
    let json = std::fs::read(out.join("ingest-stats.json")).ok()?;
    let s: Sizes = serde_json::from_slice(&json).ok()?;
    Some(s.sqlite_bytes + s.tiles_bytes + s.density_bytes).filter(|&b| b > 0)
}

/// Free space for `needs`, checked where `what` will write: `dir` and the
/// system temp folder.
pub fn check(dir: &Path, needs: Needs, what: &str) -> Result<()> {
    let temp = std::env::temp_dir();
    let dir_free = available(dir)?;
    let temp_free = if needs.temp > 0 { available(&temp)? } else { u64::MAX };
    check_with(dir, dir_free, &temp, temp_free, same_volume(dir, &temp), needs, what)
}

fn check_with(dir: &Path, dir_free: u64, temp: &Path, temp_free: u64, same: bool, needs: Needs, what: &str) -> Result<()> {
    if same || needs.temp == 0 {
        let total = needs.data + needs.temp;
        if dir_free < total {
            bail!("Not enough free space for {what}: it needs about {} on the drive holding {}, and {} is free.", gb(total), dir.display(), gb(dir_free));
        }
        return Ok(());
    }
    if dir_free < needs.data {
        bail!("Not enough free space for {what}: it needs about {} in {}, and {} is free.", gb(needs.data), dir.display(), gb(dir_free));
    }
    if temp_free < needs.temp {
        bail!(
            "Not enough free space for {what}: SQLite needs about {} for temporary files in {}, and {} is free.",
            gb(needs.temp),
            temp.display(),
            gb(temp_free)
        );
    }
    Ok(())
}

/// Bytes free to this user on the volume holding `path`, or its nearest
/// existing parent (a data folder may not exist yet).
pub fn available(path: &Path) -> Result<u64> {
    let existing = existing_ancestor(path);
    fs4::available_space(&existing).with_context(|| format!("checking free space in {}", existing.display()))
}

fn existing_ancestor(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    absolute.ancestors().find(|p| p.exists()).map_or(absolute.clone(), Path::to_path_buf)
}

/// Whether two paths are on the same volume: the same drive (or share) on
/// Windows, the same device elsewhere.
fn same_volume(a: &Path, b: &Path) -> bool {
    let (a, b) = (existing_ancestor(a), existing_ancestor(b));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(&a), std::fs::metadata(&b)) {
            (Ok(ma), Ok(mb)) => ma.dev() == mb.dev(),
            _ => true,
        }
    }
    #[cfg(not(unix))]
    {
        let prefix = |p: &Path| match p.components().next() {
            Some(std::path::Component::Prefix(pre)) => Some(pre.as_os_str().to_ascii_lowercase()),
            _ => None,
        };
        // Unknown counts as the same volume: that check is the stricter one.
        prefix(&a).is_none() || prefix(&a) == prefix(&b)
    }
}

pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: u64 = 1 << 30;

    #[test]
    fn same_volume_needs_room_for_both() {
        let needs = Needs { data: 11 * G, temp: 4 * G };
        let (d, t) = (Path::new("D:/galaxy"), Path::new("C:/Temp"));
        assert!(check_with(d, 16 * G, t, 16 * G, true, needs, "the build").is_ok());
        let err = check_with(d, 14 * G, t, 14 * G, true, needs, "the build").unwrap_err().to_string();
        assert!(err.contains("15.0 GB") && err.contains("14.0 GB is free"), "{err}");
    }

    #[test]
    fn separate_volumes_are_checked_separately() {
        let needs = Needs { data: 11 * G, temp: 4 * G };
        let (d, t) = (Path::new("D:/galaxy"), Path::new("C:/Temp"));
        assert!(check_with(d, 12 * G, t, 5 * G, false, needs, "the build").is_ok());
        let err = check_with(d, 10 * G, t, 50 * G, false, needs, "the build").unwrap_err().to_string();
        assert!(err.contains("galaxy") && err.contains("11.0 GB"), "{err}");
        let err = check_with(d, 50 * G, t, 3 * G, false, needs, "the build").unwrap_err().to_string();
        assert!(err.contains("SQLite") && err.contains("Temp"), "{err}");
    }

    #[test]
    fn estimates_use_the_previous_build_then_the_dump_size() {
        let dir = tempfile::tempdir().unwrap();
        let from_dump = catalogue_needs(dir.path(), Some(3_694_367_811), None);
        assert!(from_dump.data > 10 * G && from_dump.data < 13 * G, "{from_dump:?}");
        assert!(from_dump.temp > 2 * G, "{from_dump:?}");

        std::fs::write(
            dir.path().join("ingest-stats.json"),
            r#"{"sqlite_bytes": 1000000000, "tiles_bytes": 100000000, "density_bytes": 0, "source": "x"}"#,
        )
        .unwrap();
        let from_previous = catalogue_needs(dir.path(), Some(3_694_367_811), None);
        assert!(from_previous.data < 2 * G, "{from_previous:?}");

        let limited = catalogue_needs(dir.path(), None, Some(1000));
        assert_eq!(limited.data, 110_000 + (64 << 20));
    }

    #[test]
    fn available_space_walks_up_to_an_existing_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(available(&dir.path().join("not").join("yet")).unwrap() > 0);
        assert!(same_volume(dir.path(), &dir.path().join("missing")));
    }
}
