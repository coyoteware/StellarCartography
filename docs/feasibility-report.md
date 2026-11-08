# StellarCartography: feasibility report

*Standalone Elite: Dangerous galaxy map viewer · prototype findings · 30 October 2026, updated 8 November 2026*

> **Update, 8 November 2026: personal use.** StellarCartography is now a
> personal-use project. Keeping about 10 GB of data locally is fine, so the
> distribution question this report raised is settled and the hosted-data
> alternatives are dropped. The MVP work that still applied has been done:
> - consistent brightness across detail levels, with HDR tone mapping;
> - GPU picking with hover labels;
> - a 5× faster index build;
> - a first-run flow that builds the catalogue inside the app, plus rebuilds, settings and error states;
> - idle frame throttling, so the viewer sits quietly next to the game;
> - a per-user installer.
>
> See [MVP status](#mvp-status). The CAPI source still waits on a client ID.

## Verdict: feasible, go

A standalone viewer that shows the whole known galaxy, with your own commander
on top of it, is feasible. The prototype in this repository does it today on a
Windows desktop:

- It renders **99.4 million systems** at interactive frame rates.
- It finds any system by name in a few milliseconds.
- It tracks your commander's position live from the game's journal.

The main correction to the original idea: **the Frontier API can't be the data
source for the map.** It only returns data about the logged-in commander. The
galaxy itself has to come from community data (EDSM for now). Frontier's API is
an optional extra for commander data, and the local game files already provide
most of that without it.

The open question in the first draft was **distribution, not rendering**. The
prototype builds about 9.7 GB of local data from a 3.4 GB download, which is a
lot to ask of other people. For a personal tool it's fine, and that's now the
direction (see the update above), so the app builds that data itself on first
run.

![Galaxy overview: generated backdrop, 99.4M-system point cloud, and the commander's Sol–Colonia route](images/galaxy-overview.jpg)

## What the prototype does

- **Galaxy catalogue:**
  - The ingest streams EDSM's nightly `systemsWithCoordinates.json.gz` and writes three outputs:
    - a SQLite database for name search and id64 lookups;
    - an octree of point tiles for rendering;
    - two density maps.
  - The raw dump is never written to disk.
  - The app runs the same build itself: on first run, and later as a rebuild from Settings.
    - Progress is shown live, and the build can be cancelled.
    - A failed or cancelled build leaves the existing data untouched.
- **Renderer:**
  - three.js, inside a Tauri 2 desktop app.
  - Tiles stream in by level of detail under a point budget, so only the visible part of the galaxy is loaded.
  - Stars are coloured by mass code (a = small, cool; h = large, hot).
  - Each drawn star is weighted by how many stars it stands in for, so regions drawn at different depths match in brightness.
  - The scene renders into a half-float target and is tone mapped, so dense regions keep their colour instead of clipping to white.
  - It renders at full rate only while something moves, and at 12 fps otherwise, leaving the GPU to the game.
- **Galaxy backdrop:**
  - A ray-marched glow, built from our own density map. No Frontier artwork is used.
- **Search, fly-to, hover and click-to-identify:**
  - The info panel shows coordinates, distance from Sol and from you, id64 and mass code.
  - Hovering a star shows its name. Picking renders star ids on the GPU.
- **Settings:** data folder, journal folder, point budget, brightness and layer toggles, saved between runs.
- **Commander layer:**
  - Travel route and current-location marker, from all journal files.
  - New jumps appear within about 2 seconds while the game runs.
  - Visited systems come from the game's own `VisitedStarsCache.dat`, decoded as part of this work.

![Colonia close-up after a search, with the info panel](images/colonia-closeup.jpg)

## Data sources

### Frontier Companion API (CAPI): commander data only

| | |
| --- | --- |
| **Provides** | `/profile` (current system with its id64), `/journal` (one day's journal, not real-time), `/fleetcarrier`, `/visitedstars` (a zip of `VisitedStarsCache.dat`), `/market`, `/shipyard`, `/communitygoals`, `/squadron` |
| **Does not provide** | Any galaxy-wide system list, coordinates for systems you haven't visited, or galaxy map imagery |
| **Access** | Apply for a client ID in the Frontier Developer Zone (user.frontierstore.net); approval is "not instant" |
| **Auth** | OAuth2 with PKCE via `auth.frontierstore.net`; access tokens last about 4 hours, refresh tokens at most 25 days, then the user re-authorises |
| **Limits** | About one query a minute is the community norm; Frontier has said more than 2 queries a second may be throttled |
| **Etiquette** | User-Agent `EDCD-<AppName>-<version>` (`EDCD-StellarCartography-0.1.0`) |
| **Hosts** | `companion.orerve.net` (Live), `legacy-companion.orerve.net` (Legacy) |

**Verdict:** CAPI is worth adding for players on consoles, or on another
machine, where the journal files aren't local. A PC player running the viewer
alongside the game gets the same data from local files, live and without a
login. The prototype has a `CommanderSource` trait ready for a `CapiSource`.
Building it waits on a client ID, which we don't have yet.

### Local game files: commander data without an API

- **Journal** (`%USERPROFILE%\Saved Games\Frontier Developments\Elite Dangerous\Journal.*.log`):
  - `FSDJump`, `CarrierJump` and `Location` events carry the system name, id64 and exact coordinates.
  - The prototype reads 323 files (3,670 jumps) and polls the newest one every second.
- **`VisitedStarsCache.dat`** (`%LOCALAPPDATA%\Frontier Developments\Elite Dangerous\<id>\`):
  - Undocumented, but it decoded quickly (format below).
  - It's the same file CAPI's `/visitedstars` returns, so this parser serves both routes.
  - It lists 2,309 systems for this account. **330 of them (14%) aren't in EDSM at all**: visited, but never uploaded. The prototype places those at their id64 boxel centre (accurate to half a boxel, 5–640 ly).

### EDSM: the galaxy catalogue (chosen for now)

| | |
| --- | --- |
| File | `https://www.edsm.net/dump/systemsWithCoordinates.json.gz`, rebuilt nightly |
| Size | 3.44 GiB compressed (3,694,367,811 bytes), last modified 30 Oct 2026 07:25 GMT |
| Records | 99,390,303 systems with id64, name and coordinates; 0 malformed, 0 outside the galaxy box |
| Format | A JSON array with one object per line, which streams easily |
| Range requests | Supported, so an interrupted download can resume (the ingest does this) |

### Considered but not used now

- **Spansh full dump** (`galaxy.json.gz`):
  - About 110 GiB, including bodies and stations.
  - Ruled out: too big to work with locally, and unreasonable to ask users to download.
- **Spansh delta dumps:** 1-day, 7-day and 1-month files of 1.1–5.2 GiB. An update option for later.
- **EDDN:** a live ZeroMQ feed of player journal events with `StarPos`. The natural way to keep a hosted catalogue fresh; not needed for a local prototype.

## Measurements

Test machine:
- Windows 10, 24 threads, 62 GB RAM, NVIDIA RTX 4070 Ti SUPER.
- WebView2 (Edge 156) on a 240 Hz display, so 240 fps is the ceiling.

### Ingest (full EDSM dump, streamed from edsm.net)

| Stage | Time | Output |
| --- | --- | --- |
| Download, decompress and parse | 219 s (454k records/s, ~17 MB/s compressed) | 99.4M systems |
| Octree build | 8 s | 8,592 tiles |
| Density maps | 3 s | 51.7M distinct boxels |
| SQLite indexes (name, id64) | 456 s | the slowest step; runs alongside the octree build |
| **Total** | **677 s (11.3 min)** | |

| Output | Size |
| --- | --- |
| `galaxy.sqlite` | 8.71 GiB |
| `tiles/` | 1.02 GiB (11 bytes per system) |
| `density.png` + `coverage.png` | 3.5 MB |

**Faster index build (8 November).** Two pragmas were holding the 456 s index step back:
- `temp_store = MEMORY` sent SQLite's sorter to slow in-memory temp files.
- A 1 GB page cache made the sorter build huge runs and sort them on one thread.

With temp files on disk, the default cache and `PRAGMA threads = 8`, indexing the first 10M systems of the dump went from **26.9 s to 5.3 s**. The whole 10M ingest (from a local copy) now takes 15 s. The full 99M-system build hasn't been re-run since the change, so the new full-size figure is still to be measured.

### Rendering

| View | Point budget | Points drawn | Tiles | Avg fps | p95 frame |
| --- | --- | --- | --- | --- | --- |
| Whole galaxy (95,000 ly) | any | 1.41M | 51 | 240 (capped) | 4.3 ms |
| Sol bubble (400 ly) | 2M | 2.00M | 61 | 239 | 4.3 ms |
| Sol bubble | 5M (default) | 5.00M | 171 | 186 | 12.5 ms |
| Sol bubble | 10M | 10.0M | 379 | 114 | 20.8 ms |
| Sol bubble | 20M | 12.7M* | 516 | 99 | 20.8 ms |

\* At 20M the selection runs out of tiles worth refining before it runs out of budget.

Other render figures:
- **Tile streaming:** newly visible tiles settle within about 0.5–1 s. A session holds 50–160 MB of tiles.
- **After the 8 November rendering changes:**
  - These figures were taken with Elite Dangerous running on the same GPU.
  - Brightness weighting, the HDR target and tone mapping, and the per-vertex walk of the drawn octree together leave the galaxy view at 244 fps and the 5M-point views at 197–211 fps.
  - With nothing moving, the viewer drops to 12 fps.

![Mid-range view of the bubble and the inner galaxy: no brightness steps between detail levels](images/lod-mid-range.jpg)

### Interaction

| Operation | Time |
| --- | --- |
| Prefix search over 99.4M names ("Sol", "Col 285 Sector", "Beagle Point") | 2–4 ms |
| Click-to-identify, first version (CPU scan of 5M drawn points) | 45 ms |
| Click-to-identify or hover, now (GPU id render of a 13 px window, 5M drawn points) | 4.4 ms median |
| Commander history to the UI (journals are parsed once at startup) | 21 ms |
| Visited-stars cache: decode, plus 2,309 id64 lookups (debug build) | 110 ms |
| New journal jump to marker moved | ≤ 2 s (1 s poll) |

## Findings worth keeping

**id64 reveals the galaxy's shape without Frontier artwork.**
- Every system address encodes its boxel and its index within that boxel.
- The highest index seen in a boxel is therefore a lower bound on how many systems it holds.
- Summing those bounds across the 51.7M boxels EDSM has touched recovers the spiral arms and core (left).
- A plain count of EDSM systems mostly shows where players have travelled (right).
- The left-hand map drives the backdrop glow.

| id64-estimated density (used) | Raw EDSM coverage |
| --- | --- |
| ![](images/density-map.jpg) | ![](images/coverage-map.jpg) |

**id64 decoding.**
- Starting from the low bits: mass code (3 bits), then boxel z (14−mc), boxel y (13−mc), boxel x (14−mc), the system index (11+3·mc), and the body (9 bits).
- Boxels are `10·2^mc` ly on a grid anchored at (−49985, −40985, −24105).
- Checked against six known systems, including Sol and Sagittarius A*.

**`VisitedStarsCache.dat` (version 200).**
- **Header:**
  - The magic `"VisitedStars"`, padded to 16 bytes.
  - Four u32 fields: version, header length (48), record count, record length (16).
  - Two u64 account/commander ids.
- **Records:** 16 bytes each: `u64 id64`, `u32 visit count`, `u32 last visit` (days since 1601-01-01).
- **Trailer:** `0x5AFEC0DE` written twice.
- Implemented in [`crates/galaxy-core/src/visited.rs`](../crates/galaxy-core/src/visited.rs).

**Coordinates.**
- The game's frame is left-handed. Flipping z (`x, y, −z`) displays it in three.js without mirroring.
- Coordinates are exact multiples of 1/32 ly, so SQLite stores them as integers.

![Commander layer: the Sol–Colonia travel route and visited systems](images/commander-route.jpg)

## Known issues in the prototype

Fixed on 8 November:
- **Seams between detail levels.**
  - Neighbouring tiles at different octree depths had visibly different star densities, and additive blending made the denser ones brighter.
  - Now each frame uploads the drawn part of the octree to a small texture. The vertex shader walks it to find how many stars each point stands in for: 1 + the undrawn child's stars divided by this node's points in that octant. It weights the point's brightness by that number.
  - Distant stars fade with the square of distance, which keeps the weighted points faint enough for tone mapping to stay near linear, so the sums match.
  - Growing the sprites instead (Potree's adaptive point size) was tried and dropped: it turned distant stars into fog and cost a lot of fill rate.
- **Bright saturation in dense regions.** The scene renders into a half-float target. Tone mapping compresses by the brightest channel, so dense regions keep their colour; only extreme cores bleach slightly.
- **CPU picking.** Replaced by a GPU id pass over a 13 px window around the cursor. That's fast enough for hover labels.
- **Slow SQLite index build.** About 5× faster; see [Measurements](#measurements).

Still open:
- **Glow streaks.** At mid range (5–10k ly) the backdrop glow shows faint straight streaks from the density map's sampling.
- **Tested on one machine and GPU.** Integrated GPUs will need a lower point budget (2M), which Settings allows.
- **Windows only.** Journal and cache paths are Windows paths; Tauri itself is cross-platform. That's fine for personal use.

## Licensing and IP

- **Frontier assets:**
  - We use no game assets.
  - The galaxy backdrop is generated from catalogue data, and the UI is our own.
  - "Elite: Dangerous" is © Frontier Developments plc; the README says the project is unaffiliated.
- **EDSM data:** the dump's terms of use **still need confirming**. The EDSM pages couldn't be fetched during this investigation. As a personal project that builds its own local copy and redistributes nothing, this matters much less than it would for hosted tiles.
- **CAPI:** Frontier's developer terms are accepted when applying for a client ID; review them then.
- **Third-party code:**
  - **spansh/elite-galaxy-map (MIT):** we took only the idea of ray-marching a density map; no code was copied.
  - **Elite-Dangerous-Almanac/Galaxy-Map:** licensed PolyForm Noncommercial. We avoided it on purpose, to keep commercial options open.

## Risks and open questions

1. **Disk space:** 9.7 GB for the data, plus the same again while a rebuild is in progress (the old copy stays in use until the new one is complete), plus SQLite's sort temp files on the system drive. Fine for personal use; a rebuild fails cleanly if the disk fills.
2. **Reliance on EDSM:** a single community service; availability and future maintenance aren't guaranteed. The ingest's input format is simple to swap.
3. **Catalogue gaps:** 14% of this commander's visited systems are missing from EDSM. Expect more for explorers. Boxel placement covers the gap approximately.
4. **CAPI approval timeline:** unknown. It only blocks the console/remote-player use case.
5. **Dump growth:** EDSM grows daily; tiles and SQLite scale linearly (about 11 bytes per system for tiles, about 94 bytes for SQLite).

## Alternatives considered (dropped for personal use)

The first draft listed ways to avoid a per-user 9.7 GB build. Since this is now a personal project with the data built locally, none of them are planned. They're kept here for reference, in the order they were ranked:

1. **Hosted static tiles:**
   - Publish `tiles/` (1 GB) and the density map on a CDN, rebuilt nightly by the same ingest.
   - The app streams only visible tiles, about 50–160 MB per session as measured.
2. **Search without the 8.7 GB database:**
   - Option (a): a small hosted search API.
   - Option (b): a sharded client-side name index downloaded on demand. Names are most of the SQLite size.
3. **EDSM API on demand** for search and lookups, if its rate limits allow.
4. **Delta updates:** EDSM's 7-day dump (5.7 MB) or Spansh deltas, or EDDN for live updates to a hosted catalogue.
5. **Lite mode:** density backdrop plus populated systems only (`systemsPopulated.json.gz`, 0.5 GB), with the full catalogue as an optional download.

Delta updates are the one idea still worth something locally: applying EDSM's 7-day dump would refresh the catalogue without a 10-minute rebuild. That means appending rows to SQLite and inserting points into existing tiles, so it's a real piece of work.

## MVP status

The first draft estimated about 5–6 weeks for an MVP. With the change to personal use, here is where each item stands:

| Work | Status |
| --- | --- |
| Hosted tile pipeline | Dropped (personal use) |
| Search without local SQLite | Dropped (personal use) |
| LOD brightness weighting, HDR/tone mapping, GPU picking | **Done** (plus hover labels and idle throttling) |
| CAPI source (PKCE login, keychain token storage, `/profile` and `/visitedstars`) | Waiting on a client ID; 3–5 days once one exists |
| Settings, first-run flow, error states, installer | **Done**: in-app build and rebuild, settings saved to `settings.json`, per-user NSIS installer |
| Code signing | Dropped (personal use) |
| Testing on low-end GPUs and on macOS/Linux | Dropped (personal use, Windows) |

## Recommended next steps

1. **Apply for a Frontier CAPI client ID**, if commander data from another machine or a console is wanted. Use the app name `StellarCartography`.
2. **Measure a full rebuild** with the faster index build: run it from Settings, or with the `ingest` CLI.
3. Optional: **delta updates** from EDSM's 7-day dump, and a fix for the glow streaks.

## Reproducing this

```bash
# Run the viewer; on first run it offers to build the data from EDSM
cd app && pnpm install && pnpm tauri dev

# Or build the data from the command line (streams from EDSM, about 9.7 GB in data/)
cargo run --release -p ingest -- --out data

# Installer (per user, unsigned): target/release/bundle/nsis/
cd app && pnpm tauri build

# Tests
cargo test --workspace
pnpm --dir app typecheck
```

Raw numbers from each build are written to `ingest-stats.json` in the data folder.
