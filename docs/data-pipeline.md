# Data pipeline

How StellarCartography gets its data, what it does with it, and where it
keeps the results. Figures are from builds on 30 October 2026 (catalogue) and
9 November 2026 (populated systems).

There are four inputs, and only the first two are built into stored datasets:

| Input | Source | Built into | Refreshed |
| --- | --- | --- | --- |
| Galaxy catalogue | EDSM dump, 3.4 GB | `galaxy.sqlite`, `tiles/`, density maps | Settings → Rebuild from EDSM |
| Populated systems | Spansh dump, 4.7 GB | `populated.bin` | Settings → Update from Spansh |
| System details | Spansh and EDSM APIs, per click | `details-cache.sqlite` (raw answers) | automatically, hourly |
| Game files | journals, `VisitedStarsCache.dat` | nothing (read into memory) | live |

## 1. Galaxy catalogue

Every known system, drawn as the star field. Code:
[`crates/ingest/src/lib.rs`](../crates/ingest/src/lib.rs),
[`octree.rs`](../crates/galaxy-core/src/octree.rs),
[`density.rs`](../crates/ingest/src/density.rs).

### Ingest

- **Source:** EDSM's nightly `https://www.edsm.net/dump/systemsWithCoordinates.json.gz`. It's 3.4 GB compressed, with 99.4M systems, one per line.
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
- A `meta` table holds the source URL, the dump's Last-Modified date and the system count.

**Star tiles (`tiles/`)**
- Every point (position, row id, mass code) is held in memory, about 2 GB for the full dump.
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

### Storage

- Everything is built in `<data>/.building/`.
- When the build finishes, the current outputs are moved to `.replaced/`, the new ones are moved in, and `.replaced/` is deleted.
- Any failure along the way puts the old outputs back, so a failed or cancelled build leaves existing data untouched.
- Build figures are written to `ingest-stats.json`.

| Output | Size |
| --- | --- |
| `galaxy.sqlite` | 8.7 GB |
| `tiles/` (8,592 files) | 1.0 GB |
| `density.png` + `coverage.png` | 3.5 MB |

Time: 11.3 minutes for the full dump, before the index build was made about 5× faster (not re-measured at full size since).

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

- The file is written as `populated.bin.partial` and renamed into place when complete.
- The web view maps each column straight onto a typed array, without decoding ([`populated-data.ts`](../app/src/renderer/populated-data.ts)).
- Catalogue rebuilds never touch this file.

| Figure | Value |
| --- | --- |
| Systems | 156,627 |
| Faction presences | 657,758 |
| Distinct factions | 77,675 |
| File size | 16.7 MB (5.4 MB of it header) |
| Time | 4 min 33 s; limited by the download, 2 min of CPU |

## 3. System details

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

## 4. Game files

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
| Catalogue, tiles, density maps, `populated.bin`, `ingest-stats.json` | the data folder (below) |
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

## Known gaps

- **Full re-downloads:** both dumps are downloaded in full every time (3.4 GB and 4.7 GB). EDSM's 7-day dump and Spansh's 1-day dump would allow incremental updates; neither is implemented.
- **Build memory:** the catalogue build holds every point, and a map of about 52M boxels, in memory.
- **Disk during rebuild:** a rebuild needs room for a second copy of the catalogue until the swap.
- **Moving the data folder:** `populated.bin` stays in the old folder, so the layers need downloading again.
