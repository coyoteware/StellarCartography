# Data pipeline

How StellarCartography gets its data, what it does with it, and where it
keeps the results. Figures are from builds on 9 November 2026, on a 4-core
cloud machine downloading at about 25 MB/s.

There are five inputs, and only the first three are built into stored datasets:

| Input | Source | Built into | Refreshed |
| --- | --- | --- | --- |
| Galaxy catalogue | EDSM dump, 3.4 GB | `galaxy.sqlite`, `tiles/`, density maps | weekly reminder, or Settings → Rebuild from EDSM |
| Populated systems | Spansh dump, 4.7 GB | `populated.bin` | weekly reminder, or Settings → Update from Spansh |
| Points of interest | four EDAstro files, about 900 MB | `poi.json` | weekly reminder, or Settings → Update from EDAstro |
| System details | Spansh and EDSM APIs, per click | `details-cache.sqlite` (raw answers) | automatically, hourly |
| Game files | journals, `VisitedStarsCache.dat` | nothing (read into memory) | live |

## 1. Galaxy catalogue

Every known system, drawn as the star field. Code:
[`crates/ingest/src/lib.rs`](../crates/ingest/src/lib.rs),
[`octree.rs`](../crates/galaxy-core/src/octree.rs),
[`density.rs`](../crates/ingest/src/density.rs).

### Ingest

- **Source:** EDSM's nightly `https://www.edsm.net/dump/systemsWithCoordinates.json.gz`. It's 3.4 GB compressed, with 99.6M systems, one per line.
- **Download:** streamed over HTTP and decompressed on the fly. A dropped connection resumes with a Range request, up to 6 times with growing waits ([`source.rs`](../crates/ingest/src/source.rs)).
- **Raw dump:** never written to disk. The CLI's `--tee file` keeps a copy, and `--input file` reads one.
- **Fields kept:** id64, name and coordinates. Lines that don't parse are counted and skipped.

### Processing

One pass over the stream feeds three outputs.

**`galaxy.sqlite`**
- A separate thread inserts rows in batches of 10,000.
- Table `systems (id INTEGER PRIMARY KEY, id64, name COLLATE NOCASE, x32, y32, z32)`.
- Coordinates are integers in 1/32 ly, the game's own precision.
- After the load, it builds indexes on `name` and `id64`. That step is parallelised with `PRAGMA threads`, and its sort runs spill to disk on purpose; see the [feasibility report](feasibility-report.md#measurements).
- A `meta` table holds the source URL, the dump's Last-Modified date, the system count and when the build finished (`built_at`).

**Star tiles (`tiles/`)**
- Every point is held in memory: its position plus one `u32` holding the row id (low 29 bits) and mass code (top 3). That's 16 bytes per star, 1.5 GiB for the full dump.
- The array is sized from the dump's size before reading starts, so it never doubles.
- They're sorted into an octree over a fixed root cube 100,000 ly on a side.
- Each node keeps at most one star per cell of a 128³ grid, up to 40,000 stars, and passes the rest to its children. Maximum depth is 20.
- Drawing a node together with its ancestors draws every star in its region, so the viewer can stream detail by distance.
- There's one file per node, `tiles/<key>.bin`: a 24-byte header, then per star a `u32` row id, three `u16` coordinates quantised across the node, and a `u8` mass code. That's 11 bytes per star.
- `tiles/index.json` lists every node's key, star count and children.

**Density maps**
- Two 2048×2048 PNGs of the galactic plane.
- `coverage.png` counts catalogued systems per pixel, so it shows where players have been.
- `density.png` estimates true star density, and drives the galaxy glow:
  - an id64 includes the system's index within its boxel;
  - the highest index seen in a boxel is a lower bound on how many systems it holds;
  - the map sums those bounds over about 52M boxels.
- While reading, each boxel costs one `u64`: the id64, minus its body bits, of the highest-numbered system seen there. That one value holds both the boxel and the index. The table is sized up front, about 0.6 GiB.

### Storage

- Everything is built in `<data>/.building/`.
- When the build finishes, the current outputs are moved to `.replaced/`, the new ones are moved in, and `.replaced/` is deleted.
- Any failure along the way puts the old outputs back, so a failed or cancelled build leaves existing data untouched.
- Build figures are written to `ingest-stats.json`.

| Output | Size |
| --- | --- |
| `galaxy.sqlite` | 8.7 GiB |
| `tiles/` (8,607 files) | 1.0 GiB |
| `density.png` + `coverage.png` | 3.5 MB |

| Figure | Value |
| --- | --- |
| Time | 3.9 minutes: 2.5 downloading and parsing, 0.2 for tiles and density maps, 1.4 indexing |
| Peak memory | 2.1 GiB (it was 3.0 GiB before points and boxels were packed) |
| Peak disk use | 11.1 GiB: 9.7 GiB of new outputs, plus 1.4 GiB of SQLite sort files in the system temp folder |

### Checks before a build

A build checks free space before it writes anything, and refuses with a
message naming the drive, how much it needs and how much is free
([`space.rs`](../crates/ingest/src/space.rs)).
- **New outputs:** the previous build's size plus 10%. Without a previous build, the dump size × 2.9, plus a 10% margin.
- **SQLite sort files:** a quarter of the outputs, in the system temp folder. When that's on the same drive as the data folder, the drive needs room for both.
- **Result:** for a full build on one drive, the check asks for about 13.5 GiB, against the 11.1 GiB measured.
- **Where it runs:**
  - when a build or download is started, so the app can refuse straight away;
  - again inside the build, once the dump's size is known;
  - on the first-run card, which shows the space needed and free, and disables the build button when the drive is short.

## 2. Populated systems

Population, allegiance, government, economy, security, Powerplay and minor
factions, behind the map's overlay layers. Code:
[`crates/ingest/src/populated.rs`](../crates/ingest/src/populated.rs).

### Why Spansh

Spansh's `galaxy_populated.json.gz` is the only dump that names each system's
controlling power. EDSM's `powerPlay.json.gz` (3.9 MB) has one row per power
present in a system, each with the system's state. It never says which power
controls the system, and it misses systems such as Achenar and Canopus.

### Ingest

- **Source:** `https://downloads.spansh.co.uk/galaxy_populated.json.gz`, rebuilt daily.
  - 4.7 GB compressed, about 33 GB decompressed.
  - Lines average about 200 KB, mostly bodies and station markets.
- **Download:** the same resumable stream as the catalogue. Nothing raw is kept.
- **Parsing:** one thread decompresses. Lines go in batches of 32 to up to 6 parser threads.
- **Fields kept:** only the top-level ones; bodies, stations and markets are skipped unread.
  - id64, name, coordinates, population;
  - allegiance, government, primary and secondary economy, security;
  - controlling faction, and each faction present with its influence;
  - controlling power, powers present, Powerplay state;
  - date.

### Processing

- Empty and `"None"` values become "no value".
- Systems are sorted by population, smallest first, so the most populous draw on top.
- Repeated strings are interned into tables: categories into `u8` indexes, faction names into `u32`.

### Storage: `populated.bin`

Little-endian:

```text
 0  magic      b"SCPOP\0\0\0"
 8  u32        format version (1)
12  u32        header length
16  header     JSON: system names, category tables, faction names, column offsets
    padding    to a multiple of 8
    columns    each 8-byte aligned
```

| Column | Type | Contents |
| --- | --- | --- |
| `id64` | u64 | system address |
| `pos` | f32 × 3 | x, y, z in ly |
| `population` | f64 | |
| `allegiance`, `government`, `economy`, `secondEconomy`, `security`, `power`, `powerState` | u8 | index into the header's tables; 255 = none |
| `powers` | u32 | bitmask of every power present |
| `controllingFaction` | u32 | faction index; `0xFFFFFFFF` = none |
| `factionStart` | u32 × (n+1) | system i's factions are entries `factionStart[i]` to `factionStart[i+1]` |
| `factionId`, `factionInfluence` | u32, u16 | faction index; influence in units of 0.01% |

- The file is written as `populated.bin.partial` and renamed into place when complete. The update first checks for room for a second copy.
- The web view maps each column straight onto a typed array, without decoding ([`populated-data.ts`](../app/src/renderer/populated-data.ts)).
- Catalogue rebuilds never touch this file.
- When the data folder changes (Settings → "Use another folder…", or a build into a new folder), the file is copied into the new folder unless that already has a newer copy, so the newest layers come along. The copy keeps the original's modification time, so going back doesn't copy it again.
- A download that finishes after the folder changed (it keeps writing where it started) is copied into the current folder too. That copy happens under the same lock as the change of folder, so whichever comes second sees the other's result.
- `poi.json` is carried the same way.

| Figure | Value |
| --- | --- |
| Systems | 156,627 |
| Faction presences | 657,758 |
| Distinct factions | 77,675 |
| File size | 16.7 MB (5.4 MB of it header) |
| Time | 4 min 33 s; limited by the download, 2 min of CPU |

## 3. Points of interest

The Places directory and the map's markers: Guardian and Thargoid sites,
carrier hubs, megaships, nebulae and sights. Code:
[`crates/ingest/src/poi.rs`](../crates/ingest/src/poi.rs),
[`app/src/poi.ts`](../app/src/poi.ts).

### Sources

Four files that EDAstro publishes, read in this order:

| File | Size | Kept |
| --- | --- | --- |
| `gec/json/all`: the Galactic Exploration Catalog | 2.2 MB of JSON | all 658 entries, with summaries and links |
| `mapcharts/files/edsmPOI.csv`: EDSM's Galactic Mapping Project | 1.3 MB | 8,398 rows of the types below |
| `mapcharts/files/fleetcarriers.csv`: every known fleet carrier | 21 MB, 92k carriers | 199 carrier hubs |
| `mapcharts/files/codex-data.csv`: every codex entry players have reported, per system | 875 MB, 5.0M rows | 383 Guardian and 398 Thargoid systems |

Why these:
- **Guardian and Thargoid sites:** the codex reports are the only full record. EDSM's own Guardian entries are unnamed points on a 10 ly grid, and the GEC names only 17 Guardian sites.
- **Carrier hubs:** the carrier list includes carriers parked in unpopulated systems. Counting carriers in Spansh's populated dump would miss those.
- **EDSM's mapping project:** EDSM's own JSON of it sits behind a browser check that blocks downloads. EDAstro refreshes its copy daily.
- **Engineers aren't included.** Neither catalogue lists them, and Spansh's and EDSM's station data don't mark engineer bases: the "Workshop" service they show is at most stations. Their locations exist only inside other apps' source code.

### Processing

- **Download:** each file is streamed with the same resumable reader as the dumps. Nothing raw is kept.
  - The GEC is read whole. It's capped at 64 MB, so a wrong Content-Length or an unexpected body fails the update instead of making the app reserve the memory.
  - The CSVs are read row by row.
- **Codex filter:** the codex reports are read row by row. Only rows whose Codex ID starts with `codex_ent_guardian_`, `codex_ent_thargoid_` or `codex_ent_tg_` are decoded.
- **Categories:** every entry gets one of ten categories and a readable kind, from the mapping tables in `poi.rs`.
  - Types the tables don't know are filed under sights & history and counted in the build log.
  - The web view gives each category its own marker shape.
- **Skipped from EDSM's file:**
  - personal carrier bookmarks (`carrier`);
  - the unnamed Guardian grid points (`guardian`);
  - bare system names (`canonn`);
  - route waypoints (`trit_hwy*`) and legend markers (`STAR*`);
  - DSSA carriers that aren't deployed;
  - its older copy of the GEC.
- **Codex grouping:** one place per system and category. The strongest entry names the site:
  - Guardian: Structure, then Ruins, then Beacon, else "Guardian site";
  - Thargoid: Structure, then Spire site, then Barnacle site, else "Thargoid site".

  The summary lists every entry reported there. Names come from the Codex ID, because the file's own entry names are in whatever language each reporter's game used.
- **Which codex reports count:** entries are credited to the system the commander was in, and in the codex's first weeks (11 December 2018 to the end of January 2019) that wasn't always where the thing is. Shinrarta Dezhra got Guardian and Thargoid entries that way, and Meene a Guardian Beacon. So:
  - **Sites themselves** (ruins, structures, beacons, spires, barnacles) count whenever they were reported.
  - **Things found at sites** (data logs, terminals, pylons, sentinels, barnacle barbs, Thargoid pods and devices) make a place on their own only if one was reported after that window. This keeps 128 Guardian and 124 Thargoid sites known only from such finds, all in Guardian or Pleiades space.
  - **A site known only from reports in that window** needs another site of its kind within 100 ly that's known from other reports. Real sites reported then sit within 45 ly of others; Meene's nearest Guardian site is 240 ly away.
  - Together these drop 32 places from the November 2026 data. Reports without a date (most ruins and structures) count as outside the window.
- **Carrier hubs:** a hub is a system with 10 or more carriers seen in the 30 days up to the newest sighting in the file. Sightings dated more than a day ahead of the clock are skipped, so a bad clock can't move that window. Bigger hubs get bigger markers.
- **Summaries:** cut to about 300 characters.

### Storage: `poi.json`

- **Header:** format version (1), `builtAt`, each source's URL and Last-Modified date, and counts per category.
- **Entries:** `pois`, each with:
  - `id` (`<source>:<id>`), `source`, `category`, `kind`, `name`;
  - optional `system`, `id64` (as a string), `region`, `summary`, `url`, and `weight` (carriers);
  - `pos`.
- **Future sources:** user-added places would be one more `source` in the same list.
- **Writing:** the file is written as `poi.json.partial` and renamed into place when complete, so a failed or cancelled download leaves the old one.
- **Reading:** the web view reads it whole.
- **Positions:** come from the sources themselves. EDSM's rows carry no id64, so when one is picked, the system's id64 is looked up in `galaxy.sqlite` by name.

| Figure | Value |
| --- | --- |
| Places | 10,017 (5,870 of them planetary nebulae, hidden by default) |
| File size | 2.2 MB |
| Downloaded | 858 MB, 96% of it codex reports |
| Time | 61 s, nearly all of it the download |
| Peak memory | 37 MB |

## 4. System details

Fetched when a star is clicked. Code:
[`app/src-tauri/src/details.rs`](../app/src-tauri/src/details.rs),
[`crates/galaxy-core/src/details/`](../crates/galaxy-core/src/details/).

- **Sources:** Spansh's system API (`/api/system/{id64}`) by default, and EDSM's system, factions and stations APIs as the fallback. Settings can swap the order or turn lookups off.
- **Rate limit:** requests to each site are spaced at least 250 ms apart.
- **Merging:** online answers are combined with the journal's latest record of the system, section by section, and the newer one wins. Each section keeps its source and date.
- **Cache:** `%LOCALAPPDATA%\io.github.stellarcartography.viewer\details-cache.sqlite` stores the raw answers, keyed by site and id64. EDSM is keyed by name when there's no id64.
  - Answers under an hour old are reused without a request.
  - If a site can't be reached, an older copy is shown and marked.
  - Entries older than 30 days are deleted at startup.

## 5. Game files

Code: [`journal.rs`](../crates/galaxy-core/src/journal.rs), [`visited.rs`](../crates/galaxy-core/src/visited.rs).

**Journals (`Journal.*.log`)**
- **First read:** every file is read once, on first use. Kept in memory:
  - jumps and locations (`FSDJump`, `CarrierJump`, `Location`);
  - which commander is playing (`Commander`, `LoadGame`);
  - the latest `Docked` and `ApproachSettlement` lines;
  - the latest line describing each system and station, parsed only when its details are shown.
- **Live updates:** after that, the newest file is read every second from where reading last stopped, so new jumps reach the map within about 2 seconds.
- Nothing from the journals is written to disk.

**`VisitedStarsCache.dat`**
- Read when the viewer asks. The format is undocumented and decoded by our own reader; the layout is in `visited.rs`.
- Each id64 is looked up in `galaxy.sqlite`. Systems missing from the catalogue (14% for one tested account) are placed at their boxel's centre, accurate to 5–640 ly.

## Where everything lives

| File | Location |
| --- | --- |
| Catalogue, tiles, density maps, `populated.bin`, `poi.json`, `ingest-stats.json` | the data folder (below) |
| `details-cache.sqlite` | `%LOCALAPPDATA%\io.github.stellarcartography.viewer\` |
| `settings.json` (written via a temporary file and a rename) | `%APPDATA%\io.github.stellarcartography.viewer\` |

The data folder is the first of:
1. `STELLAR_DATA_DIR`;
2. the folder saved in Settings;
3. a `data/` folder holding a build, above the working directory or the executable;
4. `%LOCALAPPDATA%\io.github.stellarcartography.viewer\data`.

The web view reads the data folder through the backend's `galaxy` URL scheme.
That scheme refuses any path that would leave the folder. `galaxy.sqlite` is
opened read-only for search and lookups.

## Refreshing

- **The reminder:** when the catalogue, the populated systems or the points of interest are over a week old, a toast says so, with a Rebuild or Update button. Nothing downloads until it's clicked.
- **When it checks:** a minute after start, then hourly. It reminds at most once a day per dataset, and stays quiet while a build or download runs.
- **Off switch:** Settings → Updates.
- **What it compares:** the catalogue's `built_at`, or the database file's age for builds made before it was recorded, and the `builtAt` of `populated.bin` and `poi.json`.
- **Full downloads, on purpose:** the datasets are always downloaded in full (3.4 GB, 4.7 GB and 0.9 GB), which is fine at once a week. EDSM's 7-day dump and Spansh's 1-day dump would allow incremental updates, but appending to SQLite and inserting into existing tiles isn't worth it at that pace.

## Limits

- **Build memory:** 2.1 GiB at peak, almost all of it the 16-byte points (1.5 GiB) and the boxel table (0.6 GiB). Going lower means building the octree out of core: spilling points to disk and shuffling them there. That isn't done.
- **Catalogue size:** the packed points allow up to 536M systems. A larger dump stops with an error naming the limit.
