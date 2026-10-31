// Typed wrappers around the Tauri backend (app/src-tauri/src/lib.rs).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** A position in light years in the game's own frame (Sol at the origin, +z towards Sgr A*). */
export type Vec3 = [number, number, number];

export interface DatasetInfo {
  dataDir: string;
  systems: string | null;
  source: string | null;
  sourceLastModified: string | null;
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
  journalDir: string;
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

export const datasetInfo = () => invoke<DatasetInfo | null>("dataset_info");
export const visitedSystems = () => invoke<VisitedSystem[]>("visited_systems");
export const searchSystems = (query: string) => invoke<SystemInfo[]>("search_systems", { query });
export const getSystem = (rowid: number) => invoke<SystemInfo | null>("get_system", { rowid });
export const commanderHistory = () => invoke<CommanderPayload | null>("commander_history");

export function onCommanderJump(handler: (fid: string, jump: Jump) => void): Promise<UnlistenFn> {
  return listen<{ fid: string; jump: Jump }>("commander-jump", (e) => handler(e.payload.fid, e.payload.jump));
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
