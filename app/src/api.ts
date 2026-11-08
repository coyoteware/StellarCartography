// Typed wrappers around the Tauri backend (app/src-tauri/src/lib.rs).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** A position in light years in the game's own frame (Sol at the origin, +z towards Sgr A*). */
export type Vec3 = [number, number, number];

/** State of the galaxy data folder (app/src-tauri/src/data.rs). */
export type DataStatus =
  | { state: "ready"; dir: string; systems: string | null; source: string | null; sourceLastModified: string | null }
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
}

export interface Settings {
  dataDir: string;
  dataDirOrigin: DirOrigin;
  journalDir: string | null;
  journalDirCustom: boolean;
  journalDirFound: boolean;
  view: ViewSettings;
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
  rowid: number;
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

export const datasetInfo = () => invoke<DataStatus>("dataset_info");
export const visitedSystems = () => invoke<VisitedSystem[]>("visited_systems");
export const searchSystems = (query: string) => invoke<SystemInfo[]>("search_systems", { query });
export const getSystem = (rowid: number) => invoke<SystemInfo | null>("get_system", { rowid });
export const commanderHistory = () => invoke<CommanderPayload>("commander_history");

export const getSettings = () => invoke<Settings>("get_settings");
export const saveViewSettings = (view: ViewSettings) => invoke<void>("save_view_settings", { view });
/** Switches to an existing build in `dir` (or its `data` subfolder). */
export const useDataDir = (dir: string) => invoke<DataStatus>("use_data_dir", { dir });
/** `null` goes back to the game's default journal folder. */
export const setJournalDir = (dir: string | null) => invoke<void>("set_journal_dir", { dir });
export const pickFolder = (title: string, start?: string) => invoke<string | null>("pick_folder", { title, start });

/** Builds the catalogue from EDSM into `dir` (default: the current data folder). */
export const startBuild = (dir?: string, limit?: number) => invoke<string>("start_build", { dir, limit });
export const cancelBuild = () => invoke<boolean>("cancel_build");
export const buildStatus = () => invoke<BuildStatus | null>("build_status");

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
