// Points of interest: what poi.json holds (written by crates/ingest/src/poi.rs
// from EDAstro's catalogues), the categories and their marker shapes, and
// lookups for the directory, the search box and the details panel. Pure
// functions over PoiData; renderer/poi.ts draws the markers.
//
// Every category has its own shape and all markers share one colour, so
// categories never depend on telling hues apart and never compete with the
// overlay layers' three colours.

import { dataUrl, type Vec3 } from "./api";

export type PoiCategory =
  | "guardian"
  | "thargoid"
  | "xeno"
  | "carrierHub"
  | "outpost"
  | "nebula"
  | "planetaryNebula"
  | "stellar"
  | "planet"
  | "sights";

/** Where an entry came from. Points of interest added by the user would be one more source. */
export type PoiSource = "gec" | "edsm" | "codex" | "carriers";

/** Marker shapes; the number is what the shader draws (renderer/poi.ts). */
export const SHAPES = {
  dot: 0,
  triangle: 1,
  triangleDown: 2,
  diamond: 3,
  square: 4,
  ring: 5,
  ringDot: 6,
  star: 7,
  hexagon: 8,
  plus: 9,
} as const;
export type Shape = keyof typeof SHAPES;

export interface CategoryInfo {
  id: PoiCategory;
  label: string;
  shape: Shape;
}

/** In directory order. */
export const CATEGORIES: CategoryInfo[] = [
  { id: "guardian", label: "Guardian sites", shape: "triangle" },
  { id: "thargoid", label: "Thargoid sites", shape: "triangleDown" },
  { id: "xeno", label: "Mysteries", shape: "diamond" },
  { id: "carrierHub", label: "Carrier hubs", shape: "dot" },
  { id: "outpost", label: "Outposts & megaships", shape: "square" },
  { id: "nebula", label: "Nebulae & clusters", shape: "ring" },
  { id: "planetaryNebula", label: "Planetary nebulae", shape: "ringDot" },
  { id: "stellar", label: "Stellar phenomena", shape: "star" },
  { id: "planet", label: "Planets & life", shape: "hexagon" },
  { id: "sights", label: "Sights & history", shape: "plus" },
];

const CATEGORY = new Map(CATEGORIES.map((c) => [c.id, c]));

export function categoryInfo(id: PoiCategory): CategoryInfo {
  return CATEGORY.get(id) ?? CATEGORY.get("sights")!;
}

export interface Poi {
  /** Unique: `<source>:<id in that source>`. */
  id: string;
  source: PoiSource;
  category: PoiCategory;
  /** What it is, more precisely: "Guardian Structure", "Megaship", ... */
  kind: string;
  name: string;
  /** The system it's in (every current entry has one; regions might not). */
  system?: string;
  /** Kept as a string: addresses can exceed Number.MAX_SAFE_INTEGER. */
  id64?: string;
  /** Game frame, ly. */
  pos: Vec3;
  region?: string;
  summary?: string;
  /** A page about it on EDAstro. */
  url?: string;
  /** Carrier hubs: how many carriers. */
  weight?: number;
}

export interface PoiData {
  pois: Poi[];
  /** Unix seconds. */
  builtAt: number;
  sources: { name: string; url: string; lastModified: string | null }[];
  counts: Partial<Record<PoiCategory, number>>;
  /** By id64 and by lower-case system name. */
  bySystem: Map<string, Poi[]>;
  /** Lower-case name, system and kind per entry, for searching. */
  searchText: string[];
}

const VERSION = 1;

/** Fetches poi.json; null when it hasn't been downloaded yet. */
export async function loadPoi(): Promise<PoiData | null> {
  // Never cached: the file is replaced in place by updates.
  const res = await fetch(dataUrl("poi.json"), { cache: "no-store" });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`poi.json: HTTP ${res.status}`);
  return parsePoi(await res.json());
}

export function parsePoi(json: unknown): PoiData {
  const file = json as Partial<{ version: number; builtAt: number; sources: PoiData["sources"]; pois: Poi[] }>;
  if (!file || typeof file !== "object" || !Array.isArray(file.pois)) throw new Error("poi.json isn't a points-of-interest file");
  if (file.version !== VERSION) {
    throw new Error(`poi.json is format ${file.version}; this version of the app reads ${VERSION}. Update it from Settings.`);
  }
  const pois = file.pois
    .filter((p) => p && typeof p.name === "string" && Array.isArray(p.pos) && p.pos.length === 3)
    .map((p) => (CATEGORY.has(p.category) ? p : { ...p, category: "sights" as const }));
  const bySystem = new Map<string, Poi[]>();
  const add = (key: string, p: Poi) => {
    const list = bySystem.get(key);
    if (list) list.push(p);
    else bySystem.set(key, [p]);
  };
  const counts: PoiData["counts"] = {};
  for (const p of pois) {
    counts[p.category] = (counts[p.category] ?? 0) + 1;
    if (p.id64) add(p.id64, p);
    if (p.system) add(p.system.toLowerCase(), p);
  }
  return {
    pois,
    builtAt: file.builtAt ?? 0,
    sources: file.sources ?? [],
    counts,
    bySystem,
    searchText: pois.map((p) => `${p.name}\n${p.system ?? ""}\n${p.kind}`.toLowerCase()),
  };
}

/** Points of interest in a system, by its id64 or, failing that, its name. */
export function poisInSystem(data: PoiData, id64: string | null, name: string): Poi[] {
  const found = new Set<Poi>();
  if (id64) for (const p of data.bySystem.get(id64) ?? []) found.add(p);
  for (const p of data.bySystem.get(name.toLowerCase()) ?? []) found.add(p);
  return [...found].sort((a, b) => categoryOrder(a.category) - categoryOrder(b.category) || a.name.localeCompare(b.name));
}

function categoryOrder(id: PoiCategory): number {
  return CATEGORIES.findIndex((c) => c.id === id);
}

/**
 * Entries whose name, system or kind contains every word of `query`, best
 * first: names starting with the query, then names containing it, then
 * systems starting with it, then the rest ("guardian structure" finds the
 * codex sites, which are named after their systems).
 * `include` filters (e.g. by visible category); `limit` caps the result.
 */
export function searchPoi(data: PoiData, query: string, include: (p: Poi) => boolean = () => true, limit = Infinity): Poi[] {
  const q = query.trim().toLowerCase();
  const words = q.split(/\s+/).filter(Boolean);
  const hits: { p: Poi; score: number }[] = [];
  data.pois.forEach((p, i) => {
    if (!include(p)) return;
    const text = data.searchText[i];
    if (!words.every((w) => text.includes(w))) return;
    const name = p.name.toLowerCase();
    const score = name.startsWith(q) ? 0 : name.includes(q) ? 1 : (p.system ?? "").toLowerCase().startsWith(q) ? 2 : 3;
    hits.push({ p, score });
  });
  hits.sort((a, b) => a.score - b.score || a.p.name.localeCompare(b.p.name));
  return (Number.isFinite(limit) ? hits.slice(0, limit) : hits).map((h) => h.p);
}

/** Inline SVG of a category's marker, for lists and chips. */
export function glyph(shape: Shape, size = 12): SVGSVGElement {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "-6 -6 12 12");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("aria-hidden", "true");
  svg.classList.add("glyph");
  const el = (tag: string, attrs: Record<string, string>) => {
    const e = document.createElementNS(ns, tag);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
    svg.append(e);
  };
  const solid = { fill: "currentColor", stroke: "none" };
  const line = { fill: "none", stroke: "currentColor", "stroke-width": "1.6" };
  switch (shape) {
    case "dot":
      el("circle", { r: "4", ...solid });
      break;
    case "triangle":
      el("path", { d: "M0 -5 L5 4 L-5 4 Z", ...solid });
      break;
    case "triangleDown":
      el("path", { d: "M0 5 L5 -4 L-5 -4 Z", ...solid });
      break;
    case "diamond":
      el("path", { d: "M0 -5.5 L5 0 L0 5.5 L-5 0 Z", ...solid });
      break;
    case "square":
      el("rect", { x: "-4", y: "-4", width: "8", height: "8", ...solid });
      break;
    case "ring":
      el("circle", { r: "4", ...line });
      break;
    case "ringDot":
      el("circle", { r: "4.2", ...line });
      el("circle", { r: "1.5", ...solid });
      break;
    case "star":
      el("path", { d: "M0 -5.5 L1.4 -1.4 L5.5 0 L1.4 1.4 L0 5.5 L-1.4 1.4 L-5.5 0 L-1.4 -1.4 Z", ...solid });
      break;
    case "hexagon":
      el("path", { d: "M0 -5 L4.3 -2.5 L4.3 2.5 L0 5 L-4.3 2.5 L-4.3 -2.5 Z", ...solid });
      break;
    case "plus":
      el("path", { d: "M-1.5 -5 H1.5 V-1.5 H5 V1.5 H1.5 V5 H-1.5 V1.5 H-5 V-1.5 H-1.5 Z", ...solid });
      break;
  }
  return svg;
}
