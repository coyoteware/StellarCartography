// Overlay layers over the populated systems: what each one colours, how big
// each system is drawn, its legend and its hover text. Pure functions over
// PopulatedData; renderer/populated.ts draws whatever style comes out.
//
// Colour rules:
// * Nominal categories (allegiance, government, economy, Powerplay power)
//   colour at most three values at once, each in a fixed slot it keeps for
//   as long as it stays picked; every other value is grey. On a map any two
//   dots can sit side by side, and only three hues stay distinguishable
//   pairwise for colour-blind viewers on the map's dark background.
// * Ordered categories (security, Powerplay state) take one hue in steps.
// * Magnitudes (population, influence) take one hue, dark (low) to light (high).
// All hexes come from the validated reference palette (dark-mode steps).

import { NONE_U8, tableFor, type CategoryColumn, type PopulatedData } from "./renderer/populated-data";

export type OverlayId = "none" | "population" | "allegiance" | "government" | "economy" | "security" | "power" | "powerState" | "faction";

export const OVERLAYS: { id: OverlayId; label: string }[] = [
  { id: "none", label: "Nothing (stars only)" },
  { id: "population", label: "Population" },
  { id: "allegiance", label: "Allegiance" },
  { id: "government", label: "Government" },
  { id: "economy", label: "Economy" },
  { id: "security", label: "Security" },
  { id: "power", label: "Powerplay power" },
  { id: "powerState", label: "Powerplay state" },
  { id: "faction", label: "Minor faction" },
];

export function isOverlayId(s: string): s is OverlayId {
  return OVERLAYS.some((o) => o.id === s);
}

/** Categorical slots 1-3, dark steps: they pass the all-pairs colour-blind checks on the map background. */
export const SLOT_COLOURS = ["#3987e5", "#d95926", "#199e70"];
/** Every value not picked out in a nominal layer. */
export const OTHER_COLOUR = "#898781";
/** Ordinal ramp, low to high (blue 600, 500, 400, 300, 150). */
const ORDINAL = ["#184f95", "#256abf", "#3987e5", "#6da7ec", "#b7d3f6"];
/** Sequential ramp, low to high (blue 600, 450, 350, 250, 150, 100). */
export const SEQUENTIAL = ["#184f95", "#2a78d6", "#5598e7", "#86b6ef", "#b7d3f6", "#cde2fb"];

export const MAX_HIGHLIGHTS = SLOT_COLOURS.length;

/** Picks and switches for one layer, from the legend. */
export interface OverlayState {
  /** Nominal layers: picked values by colour slot, "" for an empty slot. Faction layer: [faction name]. */
  highlights: string[];
  /** Ordinal layers: values switched off in the legend. */
  hidden: ReadonlySet<string>;
}

export interface LegendItem {
  key: string;
  count: number;
  colour: string;
  /** Nominal: picked out in colour. Ordinal: shown. */
  on: boolean;
}

export interface Tick {
  /** 0..1 along the ramp. */
  at: number;
  label: string;
}

export type Legend =
  | { kind: "none" }
  | { kind: "nominal"; items: LegendItem[]; note: string }
  | { kind: "ordinal"; items: LegendItem[]; note: string }
  | { kind: "ramp"; colours: string[]; ticks: Tick[]; note: string }
  | { kind: "faction"; faction: string | null; controls: number; present: number; colours: string[]; ticks: Tick[] };

export interface OverlayStyle {
  /** RGB per system, 0..255. */
  colour: Uint8Array;
  /** Base point size in CSS pixels per system (unused for systems not in `order`). */
  size: Float32Array;
  /** Systems to draw, back to front. */
  order: Uint32Array;
  legend: Legend;
  /** Hover text for system i, shown after its name. */
  describe(i: number): string;
}

// --- Defaults ---------------------------------------------------------------

/** The values a nominal layer picks out before the user has chosen any. */
export function defaultHighlights(data: PopulatedData, id: OverlayId): string[] {
  const column = NOMINAL_COLUMNS[id];
  if (!column) return [];
  const table = tableFor(data, column);
  if (id === "allegiance") return ["Federation", "Empire", "Alliance"].filter((a) => table.includes(a));
  return Array.from(categoryCounts(data, column), (count, v) => ({ v, count }))
    .sort((a, b) => b.count - a.count)
    .slice(0, MAX_HIGHLIGHTS)
    .filter((c) => c.count > 0)
    .map((c) => table[c.v]);
}

const NOMINAL_COLUMNS: Partial<Record<OverlayId, CategoryColumn>> = {
  allegiance: "allegiance",
  government: "government",
  economy: "economy",
  power: "power",
};

export function isNominal(id: OverlayId): boolean {
  return id in NOMINAL_COLUMNS;
}

export function isOrdinal(id: OverlayId): boolean {
  return id === "security" || id === "powerState";
}

// --- Styles -------------------------------------------------------------------

export function overlayStyle(data: PopulatedData, id: OverlayId, state: OverlayState): OverlayStyle {
  switch (id) {
    case "none":
      return { ...blank(data), legend: { kind: "none" }, describe: () => "" };
    case "population":
      return population(data);
    case "allegiance":
      return nominal(data, "allegiance", state, { note: "" });
    case "government":
      return nominal(data, "government", state, { note: "" });
    case "economy":
      return nominal(data, "economy", state, {
        note: "Primary economy.",
        describe: (i) => {
          const second = label(data, "secondEconomy", i);
          return second ? `${label(data, "economy", i)} / ${second}` : label(data, "economy", i);
        },
      });
    case "security":
      return ordinal(data, "security", ["Anarchy", "Lawless", "Low", "Medium", "High"], state, {
        note: "Click a level to hide or show it.",
        describe: (i) => `${label(data, "security", i)} security`,
      });
    case "power":
      return powerStyle(data, state);
    case "powerState":
      return ordinal(data, "powerState", ["Unoccupied", "Exploited", "Fortified", "Stronghold"], state, {
        note: "Unoccupied systems are being acquired. Click a state to hide or show it.",
        describe: (i) => {
          const s = label(data, "powerState", i);
          const controller = label(data, "power", i);
          if (controller) return `${s} (${controller})`;
          const present = powersPresent(data, i);
          return present.length ? `${s}: ${present.join(", ")}` : s;
        },
      });
    case "faction":
      return factionStyle(data, state.highlights[0] || null);
  }
}

function blank(data: PopulatedData): Pick<OverlayStyle, "colour" | "size" | "order"> {
  return { colour: new Uint8Array(data.count * 3), size: new Float32Array(data.count), order: new Uint32Array(0) };
}

function rgb(hex: string): [number, number, number] {
  const v = parseInt(hex.slice(1), 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

function setColour(out: Uint8Array, i: number, c: [number, number, number]): void {
  out[i * 3] = c[0];
  out[i * 3 + 1] = c[1];
  out[i * 3 + 2] = c[2];
}

/** Colour at t (0..1) along a ramp, interpolating between its steps. */
export function rampAt(ramp: string[], t: number): [number, number, number] {
  const x = Math.min(Math.max(t, 0), 1) * (ramp.length - 1);
  const k = Math.min(Math.floor(x), ramp.length - 2);
  const f = x - k;
  const a = rgb(ramp[k]);
  const b = rgb(ramp[k + 1]);
  return [0, 1, 2].map((j) => Math.round(a[j] + (b[j] - a[j]) * f)) as [number, number, number];
}

function label(data: PopulatedData, column: CategoryColumn, i: number): string {
  const v = data.categories[column][i];
  return v === NONE_U8 ? "" : (tableFor(data, column)[v] ?? "");
}

function powersPresent(data: PopulatedData, i: number): string[] {
  const mask = data.powers[i];
  return data.tables.power.filter((_, b) => b < 32 && mask & (1 << b));
}

const counted = new WeakMap<PopulatedData, Map<CategoryColumn, Uint32Array>>();

/** Systems per value of a column. */
export function categoryCounts(data: PopulatedData, column: CategoryColumn): Uint32Array {
  let byColumn = counted.get(data);
  if (!byColumn) counted.set(data, (byColumn = new Map()));
  let counts = byColumn.get(column);
  if (!counts) {
    counts = new Uint32Array(tableFor(data, column).length);
    for (const v of data.categories[column]) if (v !== NONE_U8 && v < counts.length) counts[v]++;
    byColumn.set(column, counts);
  }
  return counts;
}

const compact = new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 });

/** Log10 population mapped onto the ramp: 1,000 or fewer at the bottom, 10 billion at the top. */
const POP_MIN_LOG = 3;
const POP_MAX_LOG = 10;

function population(data: PopulatedData): OverlayStyle {
  const { colour, size } = blank(data);
  const order: number[] = [];
  let unknown = 0;
  for (let i = 0; i < data.count; i++) {
    const p = data.population[i];
    if (!(p > 0)) {
      unknown++;
      continue;
    }
    const t = Math.min(Math.max((Math.log10(p) - POP_MIN_LOG) / (POP_MAX_LOG - POP_MIN_LOG), 0), 1);
    setColour(colour, i, rampAt(SEQUENTIAL, t));
    // Squared, so the many small colonies recede and the big systems stand out.
    size[i] = 2 + 4.5 * t * t;
    // The file is sorted by population, so the most populous end up on top.
    order.push(i);
  }
  const at = (log: number) => (log - POP_MIN_LOG) / (POP_MAX_LOG - POP_MIN_LOG);
  const note =
    `${order.length.toLocaleString()} systems, log scale; bigger dots are more populous.` +
    (unknown ? ` ${unknown.toLocaleString()} with no recorded population aren't shown.` : "");
  return {
    colour,
    size,
    order: Uint32Array.from(order),
    legend: {
      kind: "ramp",
      colours: SEQUENTIAL,
      ticks: [
        { at: 0, label: "1K" },
        { at: at(6), label: "1M" },
        { at: at(9), label: "1B" },
        { at: 1, label: "10B" },
      ],
      note,
    },
    describe: (i) => `${compact.format(data.population[i])} people`,
  };
}

interface NominalOptions {
  note: string;
  describe?: (i: number) => string;
  /** Base size of a picked system; others are drawn at 70% of it. */
  size?: (i: number) => number;
  visible?: (i: number) => boolean;
}

function nominal(data: PopulatedData, column: CategoryColumn, state: OverlayState, opts: NominalOptions): OverlayStyle {
  const { colour, size } = blank(data);
  const table = tableFor(data, column);
  const values = data.categories[column];
  const slotOf = new Int8Array(table.length).fill(-1);
  state.highlights.slice(0, MAX_HIGHLIGHTS).forEach((key, slot) => {
    const v = key ? table.indexOf(key) : -1;
    if (v >= 0) slotOf[v] = slot;
  });
  const slotRgb = SLOT_COLOURS.map(rgb);
  const otherRgb = rgb(OTHER_COLOUR);
  const counts = new Uint32Array(table.length);
  const grey: number[] = [];
  const picked: number[] = [];
  for (let i = 0; i < data.count; i++) {
    const v = values[i];
    if (v === NONE_U8 || v >= table.length || (opts.visible && !opts.visible(i))) continue;
    counts[v]++;
    const slot = slotOf[v];
    const base = opts.size?.(i) ?? 4;
    if (slot >= 0) {
      setColour(colour, i, slotRgb[slot]);
      size[i] = base;
      picked.push(i);
    } else {
      setColour(colour, i, otherRgb);
      size[i] = base * 0.7;
      grey.push(i);
    }
  }
  const items: LegendItem[] = table
    .map((key, v) => ({
      key,
      count: counts[v],
      colour: slotOf[v] >= 0 ? SLOT_COLOURS[slotOf[v]] : OTHER_COLOUR,
      on: slotOf[v] >= 0,
    }))
    .filter((it) => it.count > 0)
    .sort((a, b) => b.count - a.count || a.key.localeCompare(b.key));
  const pickNote = `Click to colour up to ${MAX_HIGHLIGHTS}; the rest are grey.`;
  return {
    colour,
    size,
    // Grey first so the picked values draw on top.
    order: Uint32Array.from([...grey, ...picked]),
    legend: { kind: "nominal", items, note: [opts.note, pickNote].filter(Boolean).join(" ") },
    describe: opts.describe ?? ((i) => label(data, column, i)),
  };
}

const STATE_SIZE: Record<string, number> = { Stronghold: 6, Fortified: 4.75, Exploited: 3.5 };

function powerStyle(data: PopulatedData, state: OverlayState): OverlayStyle {
  return nominal(data, "power", state, {
    note: "Controlled systems; bigger dots are Fortified, biggest Strongholds.",
    size: (i) => STATE_SIZE[label(data, "powerState", i)] ?? 3.5,
    describe: (i) => [label(data, "power", i), label(data, "powerState", i)].filter(Boolean).join(" · "),
  });
}

interface OrdinalOptions {
  note: string;
  describe: (i: number) => string;
}

function ordinal(data: PopulatedData, column: CategoryColumn, scale: string[], state: OverlayState, opts: OrdinalOptions): OverlayStyle {
  const { colour, size } = blank(data);
  const table = tableFor(data, column);
  const values = data.categories[column];
  const counts = categoryCounts(data, column);
  // Ramp steps for the known levels present, low to high; anything else is grey.
  const present = scale.filter((key) => {
    const v = table.indexOf(key);
    return v >= 0 && counts[v] > 0;
  });
  const colourOf = new Map<string, string>();
  present.forEach((key, k) => {
    const step = present.length === 1 ? ORDINAL.length - 1 : Math.round((k * (ORDINAL.length - 1)) / (present.length - 1));
    colourOf.set(key, ORDINAL[step]);
  });
  const rank = (key: string) => (colourOf.has(key) ? present.indexOf(key) : -1);
  const valueRgb = table.map((key) => rgb(colourOf.get(key) ?? OTHER_COLOUR));
  const shown = table.map((key) => !state.hidden.has(key));

  // Draw higher levels on top.
  const buckets: number[][] = table.map(() => []);
  for (let i = 0; i < data.count; i++) {
    const v = values[i];
    if (v === NONE_U8 || v >= table.length || !shown[v]) continue;
    setColour(colour, i, valueRgb[v]);
    size[i] = 3 + rank(table[v]) * 0.5 + (colourOf.has(table[v]) ? 0.5 : 0);
    buckets[v].push(i);
  }
  const byRank = table.map((key, v) => ({ v, r: rank(key) })).sort((a, b) => a.r - b.r);
  const order = Uint32Array.from(byRank.flatMap(({ v }) => buckets[v]));

  const items: LegendItem[] = table
    .map((key, v) => ({ key, count: counts[v], colour: colourOf.get(key) ?? OTHER_COLOUR, on: shown[v] }))
    .filter((it) => it.count > 0)
    // Highest level first, unranked values last.
    .sort((a, b) => {
      const ra = rank(a.key);
      const rb = rank(b.key);
      if (ra < 0 || rb < 0) return rb - ra || b.count - a.count;
      return rb - ra;
    });
  return { colour, size, order, legend: { kind: "ordinal", items, note: opts.note }, describe: opts.describe };
}

// --- Minor factions ---------------------------------------------------------

interface FactionIndex {
  byName: Map<string, number>;
  lower: string[];
  /** Systems each faction is present in. */
  systems: Uint32Array;
}

const factionIndexes = new WeakMap<PopulatedData, FactionIndex>();

function factionIndex(data: PopulatedData): FactionIndex {
  let index = factionIndexes.get(data);
  if (!index) {
    const systems = new Uint32Array(data.factions.length);
    for (const f of data.factionId) if (f < systems.length) systems[f]++;
    index = {
      byName: new Map(data.factions.map((name, i) => [name, i])),
      lower: data.factions.map((name) => name.toLowerCase()),
      systems,
    };
    factionIndexes.set(data, index);
  }
  return index;
}

/** Minor factions whose names contain `query`: prefix matches first, then by systems they're in. */
export function searchFactions(data: PopulatedData, query: string, limit = 8): { name: string; systems: number }[] {
  const q = query.trim().toLowerCase();
  if (q.length < 2) return [];
  const index = factionIndex(data);
  const hits: { f: number; prefix: boolean }[] = [];
  index.lower.forEach((name, f) => {
    const at = name.indexOf(q);
    if (at >= 0 && index.systems[f] > 0) hits.push({ f, prefix: at === 0 });
  });
  hits.sort((a, b) => Number(b.prefix) - Number(a.prefix) || index.systems[b.f] - index.systems[a.f]);
  return hits.slice(0, limit).map(({ f }) => ({ name: data.factions[f], systems: index.systems[f] }));
}

const INFLUENCE_TICKS: Tick[] = [
  { at: 0, label: "0%" },
  { at: 0.5, label: "50%" },
  { at: 1, label: "100%" },
];

function factionStyle(data: PopulatedData, name: string | null): OverlayStyle {
  const { colour, size } = blank(data);
  const f = name ? factionIndex(data).byName.get(name) : undefined;
  if (f === undefined) {
    return {
      colour,
      size,
      order: new Uint32Array(0),
      legend: { kind: "faction", faction: name, controls: 0, present: 0, colours: SEQUENTIAL, ticks: INFLUENCE_TICKS },
      describe: () => "",
    };
  }
  const influence = new Float32Array(data.count).fill(-1);
  const present: number[] = [];
  const controlled: number[] = [];
  for (let i = 0; i < data.count; i++) {
    for (let k = data.factionStart[i]; k < data.factionStart[i + 1]; k++) {
      if (data.factionId[k] !== f) continue;
      const inf = data.factionInfluence[k] / 10_000;
      influence[i] = inf;
      setColour(colour, i, rampAt(SEQUENTIAL, inf));
      const controls = data.controllingFaction[i] === f;
      size[i] = controls ? 6.5 : 4;
      (controls ? controlled : present).push(i);
      break;
    }
  }
  const pct = (v: number) => `${(v * 100).toFixed(1)}%`;
  return {
    colour,
    size,
    order: Uint32Array.from([...present, ...controlled]),
    legend: {
      kind: "faction",
      faction: name,
      controls: controlled.length,
      present: present.length + controlled.length,
      colours: SEQUENTIAL,
      ticks: INFLUENCE_TICKS,
    },
    describe: (i) => {
      if (influence[i] < 0) return "";
      const controls = data.controllingFaction[i] === f;
      return `${name} ${pct(influence[i])}${controls ? ", controlling" : ""}`;
    },
  };
}

