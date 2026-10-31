# StellarCartography
standalone viewer for the Elite: Dangerous galaxy map

**Status: prototype / feasibility spike.** The findings and measurements are in
[docs/feasibility-report.md](docs/feasibility-report.md).

A desktop app (Tauri 2 + three.js) that renders every system in the EDSM
catalogue as a level-of-detail point cloud over a generated galaxy backdrop,
with your commander's travel history read live from the game's journal files.

## Layout

| Path | What |
| --- | --- |
| `crates/galaxy-core` | Shared Rust: id64 decoding, octree tile format, journal parser |
| `crates/ingest` | CLI: EDSM dump → `data/galaxy.sqlite`, `data/tiles/`, `data/density.png` |
| `app/` | Vite + TypeScript + three.js frontend |
| `app/src-tauri` | Tauri backend: serves tiles, system search, journal watcher |
| `data/` | Generated data (gitignored) |

## Prerequisites (Windows)

- Rust (stable, MSVC toolchain) and the Visual Studio 2022 C++ Build Tools
- Node.js 20+ and pnpm
- WebView2 (ships with Windows 10/11)

## Build the galaxy data

From the repository root, stream the EDSM dump (about 3.7 GB, ~87M systems)
straight into `data/`. The raw dump is not saved to disk.

```bash
cargo run --release -p ingest -- --out data
```

For a quick development dataset, `--limit 2000000` stops after the first 2M
records. `--sphere x,y,z,radius` keeps only systems inside a sphere, and
`--input file.json.gz` reads a local copy instead of downloading.

## Run the viewer

```bash
cd app
pnpm install
pnpm tauri dev
```

The app looks for `data/` by walking up from its working directory. Set
`STELLAR_DATA_DIR` to point it elsewhere, and `ED_JOURNAL_DIR` if your
journals aren't in `%USERPROFILE%\Saved Games\Frontier Developments\Elite Dangerous`.

## Tests

```bash
cargo test --workspace
pnpm --dir app typecheck
```

## Data and credits

- System catalogue: [EDSM](https://www.edsm.net) nightly dump `systemsWithCoordinates.json.gz`.
- The galaxy glow ray-marches a density map built from id64 boxel indices. The
  idea comes from [spansh/elite-galaxy-map](https://github.com/spansh/elite-galaxy-map);
  the shader and data here are our own.
- Elite: Dangerous is © Frontier Developments plc. This project is not
  affiliated with or endorsed by Frontier.
