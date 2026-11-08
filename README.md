# StellarCartography
standalone viewer for the Elite: Dangerous galaxy map

**Status: personal-use desktop app (Windows).** The original feasibility study,
measurements and MVP status are in [docs/feasibility-report.md](docs/feasibility-report.md).

A desktop app (Tauri 2 + three.js) that renders every system in the EDSM
catalogue as a level-of-detail point cloud over a generated galaxy backdrop,
with your commander's travel history read live from the game's journal files.
It keeps its own copy of the catalogue (about 10 GB), built from EDSM's nightly
dump on first run and rebuilt from Settings whenever you like.

## Layout

| Path | What |
| --- | --- |
| `crates/galaxy-core` | Shared Rust: id64 decoding, octree tile format, journal parser |
| `crates/ingest` | Library + CLI: EDSM dump → `galaxy.sqlite`, `tiles/`, `density.png` (the app runs the same build) |
| `app/` | Vite + TypeScript + three.js frontend |
| `app/src-tauri` | Tauri backend: serves tiles, system search, journal watcher, settings, catalogue builds |
| `data/` | Generated data (gitignored) |

## Prerequisites (Windows)

- Rust (stable, MSVC toolchain) and the Visual Studio 2022 C++ Build Tools
- Node.js 20+ and pnpm
- WebView2 (ships with Windows 10/11)

## Run the viewer

```bash
cd app
pnpm install
pnpm tauri dev
```

(In Windows PowerShell with the default script policy, use `pnpm.cmd` in place of `pnpm`.)

On first run the app offers to **download and build** the catalogue (streams
about 3.4 GB from EDSM, nothing kept but the result; about 10 GB of data;
10–15 minutes), or to **use an existing build** folder. Later, Settings →
"Rebuild from EDSM…" refreshes it in the background while the map stays usable.

The data folder is chosen in this order:
1. `STELLAR_DATA_DIR`, if set.
2. The folder saved in Settings.
3. A `data/` folder above the working directory or the executable. This is how a development checkout finds the repo's `data/`.
4. The app's own data folder, `%LOCALAPPDATA%\io.github.stellarcartography.viewer\data`.

The journal folder defaults to
`%USERPROFILE%\Saved Games\Frontier Developments\Elite Dangerous`. You can change it
in Settings, or override it with `ED_JOURNAL_DIR`. Settings are saved in
`%APPDATA%\io.github.stellarcartography.viewer\settings.json`.

## Build the galaxy data from the command line

The same build is available as a CLI, from the repository root:

```bash
cargo run --release -p ingest -- --out data
```

For a quick development dataset, `--limit 2000000` stops after the first 2M
records. `--sphere x,y,z,radius` keeps only systems inside a sphere, and
`--input file.json.gz` reads a local copy instead of downloading.

## Installer

```bash
cd app
pnpm tauri build
```

This produces an unsigned per-user installer at
`target/release/bundle/nsis/StellarCartography_<version>_x64-setup.exe`. The
installed app won't find the repo's `data/` by itself: on first run, choose
"Already built? → Choose folder…" and pick the repository (or its `data` folder).

## Tests

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pnpm --dir app typecheck
```

GitHub Actions ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs the
same checks on Windows on every push, after a frozen-lockfile install and a
frontend build. Pushes to `main` also build the installer and attach it to the
run as an artifact (kept 14 days); for any other branch, start the workflow
from the Actions tab with "Run workflow".

## Data and credits

- System catalogue: [EDSM](https://www.edsm.net) nightly dump `systemsWithCoordinates.json.gz`.
- The galaxy glow ray-marches a density map built from id64 boxel indices. The
  idea comes from [spansh/elite-galaxy-map](https://github.com/spansh/elite-galaxy-map);
  the shader and data here are our own.
- Elite: Dangerous is © Frontier Developments plc. This project is not
  affiliated with or endorsed by Frontier.
