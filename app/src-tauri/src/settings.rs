//! User settings, kept as JSON in the app's config folder
//! (`%APPDATA%\io.github.stellarcartography.viewer\settings.json` on Windows).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::details::OnlineSource;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Galaxy data folder; unset means "find it" (see `resolve_data_dir`).
    pub data_dir: Option<PathBuf>,
    /// Journal folder; unset means the game's default location.
    pub journal_dir: Option<PathBuf>,
    /// Where the details panel looks systems up online.
    pub details_source: OnlineSource,
    /// Offer an update when the catalogue or populated systems are over a week old.
    pub update_reminders: bool,
    /// Commander names whose last known position is fetched from EDSM.
    pub friends: Vec<String>,
    pub view: ViewSettings,
    /// Keyboard shortcuts the user changed: action id → its two keys ("" is
    /// an empty slot). Actions not listed keep the web view's defaults.
    pub keys: BTreeMap<String, Vec<String>>,
    /// The system behind the Home jump button; unset hides the button.
    pub home_system: Option<SavedSystem>,
    /// The system behind the Squadron jump button; unset hides the button.
    pub squadron_system: Option<SavedSystem>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            data_dir: None,
            journal_dir: None,
            details_source: OnlineSource::default(),
            update_reminders: true,
            friends: Vec::new(),
            view: ViewSettings::default(),
            keys: BTreeMap::new(),
            home_system: None,
            squadron_system: None,
        }
    }
}

/// A system kept for a jump button, as the catalogue had it when it was
/// picked. No catalogue row: rebuilds renumber the rows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedSystem {
    /// Kept as a string, as the web view has it: addresses can exceed what a
    /// JavaScript number holds exactly.
    pub id64: Option<String>,
    pub name: String,
    /// Light years, Sol at the origin.
    pub pos: [f64; 3],
}

impl SavedSystem {
    /// Why this can't be saved, if it can't.
    pub fn problem(&self) -> Option<&'static str> {
        if self.name.trim().is_empty() {
            Some("The system has no name.")
        } else if !self.pos.iter().all(|v| v.is_finite()) {
            Some("The system has no position.")
        } else if self.id64.as_deref().is_some_and(|id| id.parse::<u64>().is_err()) {
            Some("The system's id64 isn't a number.")
        } else {
            None
        }
    }
}

/// The jump buttons whose system the user picks.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Bookmark {
    Home,
    Squadron,
}

/// Display preferences owned by the web view; the backend only stores them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewSettings {
    pub point_budget: u32,
    /// Tone-mapping exposure (1 = default brightness).
    pub exposure: f32,
    pub glow: bool,
    pub stars: bool,
    pub route: bool,
    /// Draw the plotted route (the travel history is `route`).
    pub plot: bool,
    /// What colours the populated-systems overlay ("population", "power", ...); "none" hides it.
    pub overlay: String,
    /// Per overlay, the categories picked out in colour (a minor faction's name for "faction").
    pub highlights: BTreeMap<String, Vec<String>>,
    /// Draw the points-of-interest markers.
    pub poi: bool,
    /// Point-of-interest categories switched off, in the directory and on the map.
    pub poi_hidden: Vec<String>,
    /// Draw friends' last known positions (the names are `Settings::friends`).
    pub friends: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            point_budget: 5_000_000,
            exposure: 1.0,
            glow: true,
            stars: true,
            route: true,
            plot: true,
            overlay: "none".into(),
            highlights: BTreeMap::new(),
            poi: true,
            // Thousands of catalogue entries: they'd crowd out everything else.
            poi_hidden: vec!["planetaryNebula".into()],
            friends: true,
        }
    }
}

impl Settings {
    pub fn bookmark_mut(&mut self, slot: Bookmark) -> &mut Option<SavedSystem> {
        match slot {
            Bookmark::Home => &mut self.home_system,
            Bookmark::Squadron => &mut self.squadron_system,
        }
    }

    /// Reads the settings file, falling back to defaults if it's missing or
    /// unreadable (a broken file is reported, then replaced on the next save).
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("{}: {e}; using defaults", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes via a temporary file so a crash can't leave half a file behind.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_or_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());

        let s = Settings {
            data_dir: Some(PathBuf::from(r"D:\galaxy")),
            view: ViewSettings {
                point_budget: 10_000_000,
                overlay: "power".into(),
                highlights: BTreeMap::from([("power".into(), vec!["Jerome Archer".into()])]),
                poi: false,
                poi_hidden: vec![],
                ..Default::default()
            },
            keys: BTreeMap::from([("moveUp".into(), vec!["Space".into(), String::new()])]),
            home_system: Some(SavedSystem {
                id64: Some("10477373803".into()),
                name: "Sol".into(),
                pos: [0.0, 0.0, 0.0],
            }),
            ..Default::default()
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);

        // Older or hand-edited files may lack fields: fill them with defaults.
        std::fs::write(&path, r#"{"view":{"exposure":1.5}}"#).unwrap();
        let partial = Settings::load(&path);
        assert_eq!(partial.view.exposure, 1.5);
        assert_eq!(partial.view.point_budget, 5_000_000);
        assert_eq!(partial.view.overlay, "none", "files from before the overlay layers");
        assert!(partial.view.highlights.is_empty());
        assert!(partial.view.poi, "files from before the points of interest");
        assert_eq!(partial.view.poi_hidden, ["planetaryNebula"]);
        assert_eq!(partial.details_source, OnlineSource::Spansh);
        assert!(partial.view.friends, "files from before the friends layer");
        assert!(partial.friends.is_empty(), "files from before the friends list");

        assert!(partial.update_reminders, "reminders are on unless turned off");
        assert!(partial.keys.is_empty(), "files from before the key bindings");
        assert_eq!(partial.home_system, None, "files from before the jump-button systems");
        assert_eq!(partial.squadron_system, None);

        std::fs::write(&path, r#"{"detailsSource":"off","updateReminders":false}"#).unwrap();
        assert_eq!(Settings::load(&path).details_source, OnlineSource::Off);
        assert!(!Settings::load(&path).update_reminders);

        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn bookmarks_go_in_their_own_slot_and_are_checked() {
        let sol = SavedSystem { id64: Some("10477373803".into()), name: "Sol".into(), pos: [0.0; 3] };
        let mut s = Settings::default();
        *s.bookmark_mut(Bookmark::Squadron) = Some(sol.clone());
        assert_eq!(s.squadron_system.as_ref(), Some(&sol));
        assert_eq!(s.home_system, None);

        let slot: Bookmark = serde_json::from_str(r#""home""#).unwrap();
        assert_eq!(slot, Bookmark::Home);

        assert_eq!(sol.problem(), None);
        assert_eq!(SavedSystem { id64: None, ..sol.clone() }.problem(), None, "id64 is optional");
        assert!(SavedSystem { name: " ".into(), ..sol.clone() }.problem().is_some());
        assert!(SavedSystem { pos: [f64::NAN, 0.0, 0.0], ..sol.clone() }.problem().is_some());
        assert!(SavedSystem { id64: Some("Sol".into()), ..sol }.problem().is_some());
    }
}
