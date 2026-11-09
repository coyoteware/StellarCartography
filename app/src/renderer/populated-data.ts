// Reads populated.bin, the populated systems behind the overlay layers,
// written by crates/ingest/src/populated.rs (the layout is documented there).
// Columns are typed-array views straight onto the file: no decode step.

import { dataUrl } from "../api";

/** "No value" in a u8 categorical column. */
export const NONE_U8 = 255;
/** "No faction" in a u32 faction column. */
export const NONE_U32 = 0xffffffff;

const MAGIC = "SCPOP\0\0\0";
const VERSION = 1;

interface Column {
  offset: number;
  type: "u8" | "u16" | "u32" | "u64" | "f32" | "f64";
  len: number;
}

interface Header {
  version: number;
  count: number;
  presences: number;
  source: string;
  sourceLastModified: string | null;
  newestUpdate: string | null;
  builtAt: number;
  names: string[];
  tables: Tables;
  factions: string[];
  columns: Record<string, Column>;
}

export interface Tables {
  allegiance: string[];
  government: string[];
  /** Shared by `economy` and `secondEconomy`. */
  economy: string[];
  security: string[];
  /** Shared by `power` and the `powers` bitmask. */
  power: string[];
  powerState: string[];
}

export type CategoryColumn = "allegiance" | "government" | "economy" | "secondEconomy" | "security" | "power" | "powerState";

export interface PopulatedData {
  count: number;
  names: string[];
  id64: BigUint64Array;
  /** Game frame (see coords.ts), x y z per system. */
  pos: Float32Array;
  population: Float64Array;
  categories: Record<CategoryColumn, Uint8Array>;
  /** Bitmask over `tables.power` of every power present. */
  powers: Uint32Array;
  controllingFaction: Uint32Array;
  /** System i's factions are entries factionStart[i] .. factionStart[i + 1]. */
  factionStart: Uint32Array;
  factionId: Uint32Array;
  /** Units of 0.01%. */
  factionInfluence: Uint16Array;
  tables: Tables;
  factions: string[];
  source: string;
  sourceLastModified: string | null;
  newestUpdate: string | null;
  /** Unix seconds. */
  builtAt: number;
}

/** The table a categorical column indexes. */
export function tableFor(data: PopulatedData, column: CategoryColumn): string[] {
  return column === "secondEconomy" ? data.tables.economy : data.tables[column];
}

/** Fetches populated.bin; null when it hasn't been downloaded yet. */
export async function loadPopulated(): Promise<PopulatedData | null> {
  // Never cached: the file is replaced in place by updates.
  const res = await fetch(dataUrl("populated.bin"), { cache: "no-store" });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`populated.bin: HTTP ${res.status}`);
  return parsePopulated(await res.arrayBuffer());
}

export function parsePopulated(buf: ArrayBuffer): PopulatedData {
  const view = new DataView(buf);
  const magic = new TextDecoder().decode(new Uint8Array(buf, 0, Math.min(8, buf.byteLength)));
  if (buf.byteLength < 16 || magic !== MAGIC) throw new Error("populated.bin isn't a populated-systems file");
  const version = view.getUint32(8, true);
  if (version !== VERSION) throw new Error(`populated.bin is format ${version}; this version of the app reads ${VERSION}. Update it from Settings.`);
  const headerLen = view.getUint32(12, true);
  const header = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 16, headerLen))) as Header;
  const dataStart = Math.ceil((16 + headerLen) / 8) * 8;

  const column = <T>(name: string, type: Column["type"], make: (buf: ArrayBuffer, offset: number, len: number) => T): T => {
    const c = header.columns[name];
    if (!c) throw new Error(`populated.bin has no ${name} column`);
    if (c.type !== type) throw new Error(`populated.bin: ${name} is ${c.type}, expected ${type}`);
    return make(buf, dataStart + c.offset, c.len);
  };
  const u8 = (name: string) => column(name, "u8", (b, o, n) => new Uint8Array(b, o, n));
  const u32 = (name: string) => column(name, "u32", (b, o, n) => new Uint32Array(b, o, n));

  return {
    count: header.count,
    names: header.names,
    id64: column("id64", "u64", (b, o, n) => new BigUint64Array(b, o, n)),
    pos: column("pos", "f32", (b, o, n) => new Float32Array(b, o, n)),
    population: column("population", "f64", (b, o, n) => new Float64Array(b, o, n)),
    categories: {
      allegiance: u8("allegiance"),
      government: u8("government"),
      economy: u8("economy"),
      secondEconomy: u8("secondEconomy"),
      security: u8("security"),
      power: u8("power"),
      powerState: u8("powerState"),
    },
    powers: u32("powers"),
    controllingFaction: u32("controllingFaction"),
    factionStart: u32("factionStart"),
    factionId: u32("factionId"),
    factionInfluence: column("factionInfluence", "u16", (b, o, n) => new Uint16Array(b, o, n)),
    tables: header.tables,
    factions: header.factions,
    source: header.source,
    sourceLastModified: header.sourceLastModified,
    newestUpdate: header.newestUpdate,
    builtAt: header.builtAt,
  };
}
