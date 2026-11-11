// Typed wrappers around the Tauri backend (app/src-tauri/src/lib.rs).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** A position in light years in the game's own frame (Sol at the origin, +z towards Sgr A*). */
export type Vec3 = [number, number, number];

/** State of the galaxy data folder (app/src-tauri/src/data.rs). */
export type DataStatus =
  | {
      state: "ready";
      dir: string;
      systems: string | null;
      source: string | null;
      sourceLastModified: string | null;
      /** When the catalogue was built, in Unix seconds. */
      builtAt: number | null;
    }
  | { state: "missing"; dir: string }
  | { state: "broken"; dir: string; error: string };

/** Where the data folder came from: env var, saved setting, found near the app, or the default. */
export type DirOrigin = "environment" | "settings" | "found" | "default";

export interface ViewSettings {
  pointBudget: number;
  exposure: number;
  glow: boolean;
  stars: boolean;
  route: boolean;
  /** Draw the plotted route (`route` is the travel history). */
  plot: boolean;
  /** What colours the populated-systems overlay (an `OverlayId`, see overlays.ts); "none" hides it. */
  overlay: string;
  /** Per overlay, the categories picked out in colour; "" marks an empty colour slot. */
  highlights: Record<string, string[]>;
  /** Draw the points-of-interest markers. */
  poi: boolean;
  /** Point-of-interest categories switched off (a `PoiCategory` id, see poi.ts). */
  poiHidden: string[];
  /** Draw the distant galaxies behind the Milky Way. */
  backdrop: boolean;
}

/** Where the details panel looks systems up online; the other site is the fallback. */
export type OnlineSource = "spansh" | "edsm" | "off";

export interface Settings {
  dataDir: string;
  dataDirOrigin: DirOrigin;
  journalDir: string | null;
  journalDirCustom: boolean;
  journalDirFound: boolean;
  detailsSource: OnlineSource;
  /** Offer an update when the catalogue or populated systems are over a week old. */
  updateReminders: boolean;
  view: ViewSettings;
  /** Keyboard shortcuts the user changed: action id → its two keys (see keys.ts). */
  keys: Record<string, string[]>;
}

/** Free space for a catalogue build in a folder (bytes). */
export interface DiskSpace {
  free: number;
  needed: number;
  /** Why a build there would be refused, if it would. */
  problem: string | null;
}

export type BuildPhase = "reading" | "octree" | "densityMaps" | "indexing";

export interface BuildProgress {
  phase: BuildPhase;
  /** Compressed bytes of the dump consumed so far. */
  bytesRead: number;
  bytesTotal: number | null;
  records: number;
  seconds: number;
}

export interface BuildStatus {
  dir: string;
  progress: BuildProgress | null;
}

export interface BuildDone {
  ok: boolean;
  cancelled: boolean;
  error: string | null;
  systems: number | null;
  seconds: number | null;
}

export interface SystemInfo {
  /** Row in the catalogue; null for a system picked from the populated-systems overlay. */
  rowid: number | null;
  /** Kept as a string: addresses can exceed Number.MAX_SAFE_INTEGER. */
  id64: string | null;
  name: string;
  pos: Vec3;
}

export interface Jump {
  timestamp: string;
  kind: "FSDJump" | "CarrierJump" | "Location";
  system: string;
  id64: string | null;
  pos: Vec3;
}

export interface CommanderTrack {
  fid: string;
  name: string;
  jumps: Jump[];
}

export interface CommanderPayload {
  journalDir: string | null;
  /** False when the journal folder doesn't exist. */
  found: boolean;
  commanders: CommanderTrack[];
  latestFid: string | null;
}

/** An entry from the game's VisitedStarsCache.dat. */
export interface VisitedSystem {
  id64: string;
  visits: number;
  /** Days since 1601-01-01. */
  lastVisitDay: number;
  name: string | null;
  pos: Vec3;
  /** Not in the catalogue: `pos` is the boxel centre from the id64. */
  approximate: boolean;
}

// --- System details (app/src-tauri/src/details.rs) ----------------------------

/** Where a piece of system detail came from. */
export type DetailsSource = "journal" | "edsm" | "spansh";

export interface Sourced {
  source: DetailsSource;
  /** ISO timestamp the source last saw this, when known. */
  asOf: string | null;
}

export interface SystemOverview extends Sourced {
  allegiance: string | null;
  government: string | null;
  economy: string | null;
  secondEconomy: string | null;
  security: string | null;
  population: number | null;
  controllingFaction: string | null;
  controllingFactionState: string | null;
  /** Permit name when the system needs one. */
  permit: string | null;
}

export interface PowerplayInfo extends Sourced {
  controllingPower: string | null;
  /** Powerplay 2.0 state: Exploited, Fortified, Stronghold, Unoccupied, ... */
  state: string | null;
  /** 0..1 progress towards the next state, when reported. */
  controlProgress: number | null;
  reinforcement: number | null;
  undermining: number | null;
  powers: string[];
  /** Powers contesting an unoccupied system, with their progress (0..1). */
  conflict: { power: string; progress: number }[];
}

export interface FactionInfo {
  name: string;
  allegiance: string | null;
  government: string | null;
  /** 0..1 */
  influence: number | null;
  state: string | null;
  activeStates: string[];
  pendingStates: string[];
  recoveringStates: string[];
  happiness: string | null;
  controlling: boolean;
  /** Your reputation with them (journal only), -100..100. */
  myReputation: number | null;
}

/** A war, civil war or election (journal only). */
export interface ConflictInfo {
  kind: string;
  status: string | null;
  sides: { faction: string; stake: string | null; wonDays: number | null }[];
}

export interface FactionsInfo extends Sourced {
  factions: FactionInfo[];
  conflicts: ConflictInfo[];
}

export interface StationInfo extends Sourced {
  name: string;
  /** Coriolis, Orbis, Outpost, FleetCarrier, CraterOutpost, OnFootSettlement, ... */
  kind: string | null;
  marketId: number | null;
  distanceLs: number | null;
  /** Largest landing pad: "L", "M", "S", or null when unknown. */
  maxPad: "L" | "M" | "S" | null;
  services: string[];
  economy: string | null;
  faction: string | null;
  government: string | null;
  allegiance: string | null;
}

export interface BodiesInfo extends Sourced {
  count: number | null;
  primaryStar: string | null;
  scoopable: boolean | null;
}

export interface RemoteStatus {
  source: DetailsSource;
  /** False when the lookup failed; `error` says why. */
  ok: boolean;
  /** The site answered but has no record of the system. */
  missing: boolean;
  error: string | null;
  /** When the cached copy was fetched (ISO). */
  fetchedAt: string | null;
}

export interface SystemDetails {
  overview: SystemOverview | null;
  powerplay: PowerplayInfo | null;
  factions: FactionsInfo | null;
  stations: StationInfo[];
  bodies: BodiesInfo | null;
  /** Online lookups made (or served from the cache) for this answer. */
  remote: RemoteStatus[];
  /** Pages about this system on EDSM, Inara and Spansh. */
  links: { label: string; url: string }[];
}

// --- Route plotting (app/src-tauri/src/routes.rs) ------------------------------

/** A ship's fuel model from the journal's Loadout event (galaxy-core/src/ship.rs). */
export interface ShipSpec {
  /** When the loadout was written (ISO). */
  timestamp: string;
  /** Internal ship type ("corsair", "python_nx", ...). */
  ship: string;
  shipName: string;
  shipIdent: string;
  baseMass: number;
  tankSize: number;
  internalTankSize: number;
  cargoCapacity: number;
  /** The game's own current-fuel maximum range (ly). */
  maxJumpRange: number;
  fuelPower: number;
  fuelMultiplier: number;
  optimalMass: number;
  maxFuelPerJump: number;
  /** Guardian FSD booster range (ly), 0 without one. */
  rangeBoost: number;
  superchargeMultiplier: number;
}

export type RouteMode = "neutron" | "galaxy" | "carrier";

export type RouteRequest =
  | { mode: "neutron"; from: string; to: string; range: number; efficiency: number }
  | {
      mode: "galaxy";
      source: string;
      destination: string;
      ship: ShipSpec;
      cargo: number;
      isSupercharged: boolean;
      useSupercharge: boolean;
      useInjections: boolean;
      excludeSecondary: boolean;
      refuelEveryScoopable: boolean;
    }
  | {
      mode: "carrier";
      source: string;
      destinations: string[];
      capacityUsed: number;
      calculateStartingFuel: boolean;
      squadron: boolean;
    };

/** One stop on a plotted route; which extras are filled depends on the plotter. */
export interface Waypoint {
  name: string;
  id64: string | null;
  pos: Vec3;
  /** Light years from the previous waypoint. */
  distance: number;
  /** Light years still to go after this one. */
  distanceLeft: number;
  /** Neutron plotter: estimated jumps to get here from the previous waypoint. */
  jumps: number | null;
  neutron: boolean;
  scoopable: boolean;
  /** Scoop here (galaxy plotter) or load tritium here (carrier). */
  refuel: boolean;
  /** Carrier: tonnes of tritium to load here. */
  restock: number | null;
  icyRing: boolean;
  pristine: boolean;
  fuelUsed: number | null;
  fuelLeft: number | null;
  /** Carrier: one of the systems the route was asked to reach. */
  destination: boolean;
}

export interface PlottedRoute {
  mode: RouteMode;
  source: string;
  destination: string;
  waypoints: Waypoint[];
  /** Light years along the route. */
  totalDistance: number;
  /** Neutron: Spansh's estimate of real jumps; others: one per waypoint. */
  totalJumps: number | null;
  /** Unix seconds. */
  plottedAt: number;
}

/** The active route as kept across restarts. */
export interface SavedRoute {
  route: PlottedRoute;
  /** Index of the waypoint the commander has reached. */
  progress: number;
  /** Copy the next waypoint's name to the clipboard after each jump. */
  copyNext: boolean;
}

/** The newest journal loadout for a commander (or any, when `fid` is unset). */
export const shipLoadout = (fid?: string | null) => invoke<ShipSpec | null>("ship_loadout", { fid: fid ?? null });
/** Plots on Spansh; takes seconds to minutes. Rejects with "cancelled" when superseded. */
export const plotRoute = (request: RouteRequest) => invoke<PlottedRoute>("plot_route", { request });
export const cancelPlot = () => invoke<void>("cancel_plot");
export const savedRoute = () => invoke<SavedRoute | null>("saved_route");
/** Persists the active route and progress; `null` clears it. */
export const saveRoute = (route: SavedRoute | null) => invoke<void>("save_route", { route });

export const datasetInfo = () => invoke<DataStatus>("dataset_info");
/** Everything known about a system, from the journals and online sources. `refresh` skips the cache. */
export const systemDetails = (id64: string | null, name: string, refresh = false) =>
  invoke<SystemDetails>("system_details", { id64, name, refresh });
/** Opens an EDSM, Inara or Spansh page in the default browser. */
export const openExternal = (url: string) => invoke<void>("open_external", { url });
export const visitedSystems = () => invoke<VisitedSystem[]>("visited_systems");
export const searchSystems = (query: string) => invoke<SystemInfo[]>("search_systems", { query });
export const getSystem = (rowid: number) => invoke<SystemInfo | null>("get_system", { rowid });
export const commanderHistory = () => invoke<CommanderPayload>("commander_history");

export const getSettings = () => invoke<Settings>("get_settings");
export const saveViewSettings = (view: ViewSettings) => invoke<void>("save_view_settings", { view });
/** Saves the keyboard shortcuts that differ from the defaults. */
export const saveKeys = (keys: Record<string, string[]>) => invoke<void>("save_keys", { keys });
/** Switches to an existing build in `dir` (or its `data` subfolder). */
export const useDataDir = (dir: string) => invoke<DataStatus>("use_data_dir", { dir });
/** `null` goes back to the game's default journal folder. */
export const setJournalDir = (dir: string | null) => invoke<void>("set_journal_dir", { dir });
export const pickFolder = (title: string, start?: string) => invoke<string | null>("pick_folder", { title, start });
export const setDetailsSource = (source: OnlineSource) => invoke<void>("set_details_source", { source });
export const setUpdateReminders = (on: boolean) => invoke<void>("set_update_reminders", { on });
export const diskSpace = (dir: string) => invoke<DiskSpace>("disk_space", { dir });

/** Builds the catalogue from EDSM into `dir` (default: the current data folder). */
export const startBuild = (dir?: string, limit?: number) => invoke<string>("start_build", { dir, limit });
export const cancelBuild = () => invoke<boolean>("cancel_build");
export const buildStatus = () => invoke<BuildStatus | null>("build_status");

/** Progress of the populated-systems download (crates/ingest/src/populated.rs). */
export interface PopulatedProgress {
  bytesRead: number;
  bytesTotal: number | null;
  systems: number;
  seconds: number;
}

export interface PopulatedDone {
  ok: boolean;
  cancelled: boolean;
  error: string | null;
  systems: number | null;
  seconds: number | null;
}

/** Downloads Spansh's populated-systems dump into the data folder's populated.bin. */
export const startPopulatedUpdate = () => invoke<void>("start_populated_update");
export const cancelPopulatedUpdate = () => invoke<boolean>("cancel_populated_update");
/** Non-null while a download runs. */
export const populatedUpdateStatus = () => invoke<{ progress: PopulatedProgress | null } | null>("populated_update_status");

export function onPopulatedProgress(handler: (p: PopulatedProgress) => void): Promise<UnlistenFn> {
  return listen<PopulatedProgress>("populated-progress", (e) => handler(e.payload));
}

export function onPopulatedDone(handler: (d: PopulatedDone) => void): Promise<UnlistenFn> {
  return listen<PopulatedDone>("populated-done", (e) => handler(e.payload));
}

/** Progress of the points-of-interest download (crates/ingest/src/poi.rs). */
export interface PoiProgress {
  phase: "gec" | "edsm" | "carriers" | "codex";
  /** 1-based position of `phase` among `steps` files. */
  step: number;
  steps: number;
  /** Bytes of the current file read so far. */
  bytesRead: number;
  bytesTotal: number | null;
  pois: number;
  seconds: number;
}

export interface PoiDone {
  ok: boolean;
  cancelled: boolean;
  error: string | null;
  pois: number | null;
  seconds: number | null;
}

/** Downloads EDAstro's catalogues into the data folder's poi.json. */
export const startPoiUpdate = () => invoke<void>("start_poi_update");
export const cancelPoiUpdate = () => invoke<boolean>("cancel_poi_update");
/** Non-null while a download runs. */
export const poiUpdateStatus = () => invoke<{ progress: PoiProgress | null } | null>("poi_update_status");

export function onPoiProgress(handler: (p: PoiProgress) => void): Promise<UnlistenFn> {
  return listen<PoiProgress>("poi-progress", (e) => handler(e.payload));
}

export function onPoiDone(handler: (d: PoiDone) => void): Promise<UnlistenFn> {
  return listen<PoiDone>("poi-done", (e) => handler(e.payload));
}

export function onCommanderJump(handler: (fid: string, jump: Jump) => void): Promise<UnlistenFn> {
  return listen<{ fid: string; jump: Jump }>("commander-jump", (e) => handler(e.payload.fid, e.payload.jump));
}

export function onBuildProgress(handler: (p: BuildProgress) => void): Promise<UnlistenFn> {
  return listen<BuildProgress>("build-progress", (e) => handler(e.payload));
}

export function onBuildDone(handler: (d: BuildDone) => void): Promise<UnlistenFn> {
  return listen<BuildDone>("build-done", (e) => handler(e.payload));
}

/** URL of a file in the generated data dir, served by the backend's `galaxy` scheme. */
export function dataUrl(path: string): string {
  const windows = navigator.userAgent.includes("Windows");
  return windows ? `http://galaxy.localhost/${path}` : `galaxy://localhost/${path}`;
}

const MASS_CODES = "abcdefgh";

/** Mass-code letter (boxel size class) encoded in the low 3 bits of an id64. */
export function massCode(id64: string): string {
  return MASS_CODES[Number(BigInt(id64) & 7n)];
}
