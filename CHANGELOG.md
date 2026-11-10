# Changelog

## 0.2.0 (2026-11-10)

### New
- **Map layers.** Layers → "Colour systems by" colours every populated system by
  population, allegiance, government, economy, security, Powerplay or minor
  faction, with a legend, hover text and click-through to system details. The
  data comes from Spansh's populated-systems dump (about 4.7 GB streamed, 17 MB
  kept).
- **Points of interest.** Guardian and Thargoid sites, carrier hubs, megaships,
  nebulae, stellar phenomena and other sights, from EDAstro and EDSM's Galactic
  Mapping Project. A Places directory under the search box lists them by
  distance from your commander, a marker layer draws them on the map, and
  system details and search include them.
- **Keyboard controls.** Fly, orbit, zoom, jump and toggle layers from the
  keyboard. Every action has two keys, and every key can be changed (F1).
- **Jump buttons** for the home system and the squadron capital.
- **Update reminders.** When the catalogue, the map layers or the points of
  interest are over a week old, a reminder offers to refresh them (Settings →
  Updates turns it off).
- **Startup splash** with a line from Captain Picard, so the map no longer pops
  in piece by piece.
- **App icon and favicon** from the Stellar Cartography logo.

### Changed
- The UI is restyled on the Stellar Cartography design system, with its fonts
  bundled for offline use.
- Catalogue builds use about 30% less memory (peak 2.1 GiB, was 3.0 GiB) and
  check free disk space before they start.

## 0.1.0 (2026-11-08)

First personal-use build: the EDSM catalogue as a level-of-detail point cloud,
travel history from the journals, in-app catalogue builds, settings, GPU
picking, and a per-user NSIS installer.
