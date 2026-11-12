# Changelog

## Unreleased

### New
- **Friends on the map.** Settings → Friends takes commander names and marks
  each one's last known position as an ice-blue pin with a name label (its
  own layer, Layers → Friends). Positions come from EDSM, so a friend shows
  up only if they upload their journals there with a public flight log, and
  only as fresh as their last upload (hover a marker for the system, ship
  and when they were seen; click to select the system). Names seen on your
  in-game friends list or in your wing are suggested as you type.
- **Route plotting.** A Route panel under the search box plots routes on
  Spansh: the neutron plotter (jump range and efficiency), the fuel-exact
  galaxy plotter (using the ship from your journals, engineering included),
  and the fleet-carrier router (with tritium stops, multiple destinations and
  squadron carriers). The route draws on the map as its own layer, the
  waypoint list follows your jumps, and the next system can be copied to the
  clipboard after each jump for the in-game galaxy map. The route survives a
  restart.
- **Your fleet carrier on the map.** The journals' carrier events (bought,
  stats, location at login, jumps booked and cancelled, riding it through a
  jump) place your own carrier as a peach pin labelled with its name; it moves
  when a booked jump departs, and hovering it says where it's jumping next.
  Squadron carriers are left out. It goes with Layers → Travel history.

### Changed
- **Home and Squadron go where you choose.** They no longer point at fixed
  systems: Settings → Jump buttons sets and clears them. A button stays hidden
  until it has a system.
- **Map markers.** You, your friends, your home system and your carrier are
  LCARS block pins: a solid block in the system's colour (accent, ice, lilac,
  peach) with a pictogram cut out of it, standing beside its star and pointing
  at it. Hover a pin for what it is, click it to select the system. Megaships,
  the expeditions' carriers (DSSA, OASIS, PIONEER) and carrier hubs are drawn
  as ships seen from above instead of squares and dots.
- **No more edge around the explored regions.** The galaxy glow stands for
  every system, while the stars show only the catalogued ones, which
  are nearly complete within about 1,000 ly of Sol and around 1% beyond
  10,000 ly. Drawn on top of each other, the bubble and the busy routes
  came out twice as bright as their surroundings and tinted orange, with a
  hard edge where the catalogue thins out. Now the glow steps back by as much
  light as the stars there already give, and distant stars take on the
  glow's colour, so explored and unexplored space blend. Stars close to the
  camera keep their own colours. Where stars would outshine the glow, as
  along a busy route seen end on, they're dimmed to match it. The map is a
  little darker overall, since nothing is counted twice; Settings → Display →
  Brightness brings it back up.
- **A galaxy glow that doesn't follow exploration.** The glow's density map
  counted only the boxels someone had logged a system in, so it was brightest
  where players had been: around the bubble, and in streaks along the busy
  routes. It now treats the logged boxels around each spot as a sample of all
  of them there and scales up, so the arms carry on through unexplored space,
  the streaks are gone, and the outer arms reach the galaxy's edge. The new
  map comes with the next catalogue rebuild, or straight away from an
  existing build with `ingest --maps --out <data folder>` (about a minute, no
  download).

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
