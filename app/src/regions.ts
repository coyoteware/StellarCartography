// The galaxy's 42 codex regions: the names the galaxy map's atlas shows, the
// journal records and the codex files discoveries under.
//
// The map is klightspeed's EliteDangerousRegionMap (data/region-map.json, MIT,
// see data/region-map.LICENSE), mapped by following the region edges in game.
// It's a 2048 x 2048 grid over the galactic plane, of cells 4096/83 ly (about
// 49 ly) square, each row run-length encoded as [length, region] pairs from
// the lowest z up. Regions are columns: height makes no difference.
//
// The game names the region at a system's coordinates when you jump there;
// the journal and codex use the corner of its boxel, which can differ within
// a boxel's width of an edge. This uses the coordinates.
//
// Alongside them are the Galactic Mapping Project's names for parts of the
// galaxy (data/gmp-regions.json: the Abyssal Plain, Mare Desperationis...),
// drawn by explorers before the game had regions of its own. They're
// outlines on the plane, from EDSM's galactic mapping, and they overlap the
// codex regions and sometimes each other.

import type { Vec3 } from "./api";
import gmp from "./data/gmp-regions.json";
import data from "./data/region-map.json";

/** The grid's corner (lowest x and z) in the game's frame, ly. */
const X0 = -49985;
const Z0 = -24105;
/** A cell's width, ly. */
export const REGION_CELL = 4096 / 83;
/** Cells along each side. */
const SIZE = 2048;
/** Edges are straightened to within this many cells of the grid's (see `trace`). */
const STRAIGHTEN = 0.6;

const ROWS = data.regionmap as [number, number][][];

/** Region names by id; id 0 is outside the map. */
export const REGION_NAMES: readonly string[] = data.regions.map((n) => n ?? "");

/** The region id at a point in the game's frame (0 outside the map). */
export function regionAt(x: number, z: number): number {
  const px = Math.floor(((x - X0) * 83) / 4096);
  const pz = Math.floor(((z - Z0) * 83) / 4096);
  if (px < 0 || pz < 0 || px >= SIZE || pz >= SIZE) return 0;
  let start = 0;
  for (const [length, id] of ROWS[pz]) {
    start += length;
    if (px < start) return id;
  }
  return 0;
}

/** The name of the region a position is in, or null outside the map. */
export function regionName(pos: Vec3): string | null {
  return REGION_NAMES[regionAt(pos[0], pos[2])] || null;
}

/** Where a region's name goes: the point deepest inside it. */
export interface RegionLabel {
  id: number;
  name: string;
  /** In the game's frame, ly. */
  x: number;
  z: number;
  /** How far that point is from the region's nearest edge, ly. */
  room: number;
}

export interface RegionAtlas {
  /** Edges between regions, as line segments in the game's frame: x0, z0, x1, z1 per segment (ly). */
  edges: Float32Array;
  /** One per region, largest room first. */
  labels: RegionLabel[];
}

let atlas: RegionAtlas | null = null;

/** Edges and label anchors for the whole map, worked out once (~100 ms). */
export function regionAtlas(): RegionAtlas {
  if (!atlas) {
    const grid = decode();
    atlas = { edges: trace(grid), labels: anchors(grid) };
  }
  return atlas;
}

function decode(): Uint8Array {
  const grid = new Uint8Array(SIZE * SIZE);
  ROWS.forEach((row, z) => {
    let x = z * SIZE;
    for (const [length, id] of row) {
      grid.fill(id, x, x + length);
      x += length;
    }
  });
  return grid;
}

/**
 * The edges between regions as straight segments. The grid's edges are
 * staircases of 49 ly steps; drawn as they are, the far-off ones break up
 * into dots (steps shorter than a pixel draw nothing). So each edge between
 * two regions is followed from one junction to the next and straightened
 * (Douglas-Peucker) to within STRAIGHTEN cells, which is within 30 ly of
 * where the game puts it. The map's outer rim isn't an edge.
 */
function trace(grid: Uint8Array): Float32Array {
  // Unit edges on the lattice of cell corners, vertex = j * W + i.
  const W = SIZE + 1;
  const from: number[] = [];
  const to: number[] = [];
  const pair: number[] = [];
  const incident = new Map<number, number[]>();
  const add = (v0: number, v1: number, a: number, b: number) => {
    const e = from.length;
    from.push(v0);
    to.push(v1);
    pair.push(a < b ? a * 64 + b : b * 64 + a);
    for (const v of [v0, v1]) {
      const list = incident.get(v);
      if (list) list.push(e);
      else incident.set(v, [e]);
    }
  };
  for (let j = 1; j < SIZE; j++) {
    for (let i = 0; i < SIZE; i++) {
      const a = grid[(j - 1) * SIZE + i];
      const b = grid[j * SIZE + i];
      if (a !== b && a && b) add(j * W + i, j * W + i + 1, a, b);
    }
  }
  for (let j = 0; j < SIZE; j++) {
    for (let i = 1; i < SIZE; i++) {
      const a = grid[j * SIZE + i - 1];
      const b = grid[j * SIZE + i];
      if (a !== b && a && b) add(j * W + i, (j + 1) * W + i, a, b);
    }
  }

  // Chains of unit edges between the same two regions, junction to junction.
  const done = new Uint8Array(from.length);
  const junction = (v: number) => {
    const list = incident.get(v)!;
    return list.length !== 2 || pair[list[0]] !== pair[list[1]];
  };
  const chains: number[][] = [];
  const follow = (v: number, e: number) => {
    const chain = [v];
    for (;;) {
      done[e] = 1;
      const next = from[e] === v ? to[e] : from[e];
      chain.push(next);
      if (junction(next)) break;
      const list = incident.get(next)!;
      const e2 = list[0] === e ? list[1] : list[0];
      if (done[e2]) break;
      v = next;
      e = e2;
    }
    chains.push(chain);
  };
  for (const [v, list] of incident) {
    if (!junction(v)) continue;
    for (const e of list) if (!done[e]) follow(v, e);
  }
  // Whatever's left are loops round an enclave, with no junction on them.
  for (let e = 0; e < from.length; e++) if (!done[e]) follow(from[e], e);

  const xs = (v: number) => v % W;
  const zs = (v: number) => Math.floor(v / W);
  const out: number[] = [];
  const emit = (chain: number[]) => {
    const kept = straighten(chain, xs, zs);
    for (let k = 1; k < kept.length; k++) {
      const a = kept[k - 1];
      const b = kept[k];
      out.push(X0 + xs(a) * REGION_CELL, Z0 + zs(a) * REGION_CELL, X0 + xs(b) * REGION_CELL, Z0 + zs(b) * REGION_CELL);
    }
  };
  for (const chain of chains) {
    if (chain[0] !== chain[chain.length - 1]) {
      emit(chain);
      continue;
    }
    // A loop: split it at the point farthest from where it starts.
    let far = 1;
    let farD = -1;
    for (let k = 1; k < chain.length - 1; k++) {
      const d = Math.hypot(xs(chain[k]) - xs(chain[0]), zs(chain[k]) - zs(chain[0]));
      if (d > farD) [far, farD] = [k, d];
    }
    emit(chain.slice(0, far + 1));
    emit(chain.slice(far));
  }
  return Float32Array.from(out);
}

/** Douglas-Peucker: the points of `chain` needed to stay within STRAIGHTEN cells of it. */
function straighten(chain: number[], xs: (v: number) => number, zs: (v: number) => number): number[] {
  const keep = new Uint8Array(chain.length);
  keep[0] = keep[chain.length - 1] = 1;
  const stack: [number, number][] = [[0, chain.length - 1]];
  while (stack.length) {
    const [a, b] = stack.pop()!;
    const ax = xs(chain[a]);
    const az = zs(chain[a]);
    const dx = xs(chain[b]) - ax;
    const dz = zs(chain[b]) - az;
    const length = Math.hypot(dx, dz);
    let worst = -1;
    let worstD = STRAIGHTEN;
    for (let k = a + 1; k < b; k++) {
      const qx = xs(chain[k]) - ax;
      const qz = zs(chain[k]) - az;
      const d = length > 0 ? Math.abs(qx * dz - qz * dx) / length : Math.hypot(qx, qz);
      if (d > worstD) [worst, worstD] = [k, d];
    }
    if (worst >= 0) {
      keep[worst] = 1;
      stack.push([a, worst], [worst, b]);
    }
  }
  return chain.filter((_, k) => keep[k]);
}

/**
 * For each region, the cell farthest from any other region or the map's rim
 * (a chamfer distance transform), so its name sits well inside it. Where a
 * ridge of cells ties for farthest, as along a curved arm, the one nearest
 * the ridge's middle.
 */
function anchors(grid: Uint8Array): RegionLabel[] {
  const N = SIZE;
  const dist = new Float32Array(N * N);
  for (let z = 0; z < N; z++) {
    for (let x = 0; x < N; x++) {
      const k = z * N + x;
      const id = grid[k];
      if (!id) continue;
      const edge =
        x === 0 || z === 0 || x === N - 1 || z === N - 1 || grid[k - 1] !== id || grid[k + 1] !== id || grid[k - N] !== id || grid[k + N] !== id;
      dist[k] = edge ? 1 : Infinity;
    }
  }
  const D = Math.SQRT2;
  for (let z = 1; z < N; z++) {
    for (let x = 1; x < N - 1; x++) {
      const k = z * N + x;
      if (dist[k] > 1) dist[k] = Math.min(dist[k], dist[k - 1] + 1, dist[k - N] + 1, dist[k - N - 1] + D, dist[k - N + 1] + D);
    }
  }
  for (let z = N - 2; z >= 0; z--) {
    for (let x = N - 2; x >= 1; x--) {
      const k = z * N + x;
      if (dist[k] > 1) dist[k] = Math.min(dist[k], dist[k + 1] + 1, dist[k + N] + 1, dist[k + N + 1] + D, dist[k + N - 1] + D);
    }
  }

  const count = REGION_NAMES.length;
  const best = new Float32Array(count);
  for (let k = 0; k < N * N; k++) if (dist[k] > best[grid[k]]) best[grid[k]] = dist[k];
  // The ridge: cells within half a cell of the best, and their middle.
  const sx = new Float64Array(count);
  const sz = new Float64Array(count);
  const n = new Float64Array(count);
  const ridge = (k: number) => grid[k] && dist[k] >= best[grid[k]] - 0.5;
  for (let k = 0; k < N * N; k++) {
    if (!ridge(k)) continue;
    const id = grid[k];
    sx[id] += k % N;
    sz[id] += Math.floor(k / N);
    n[id]++;
  }
  const pick = new Int32Array(count).fill(-1);
  const pickD = new Float64Array(count).fill(Infinity);
  for (let k = 0; k < N * N; k++) {
    if (!ridge(k)) continue;
    const id = grid[k];
    const d = (k % N - sx[id] / n[id]) ** 2 + (Math.floor(k / N) - sz[id] / n[id]) ** 2;
    if (d < pickD[id]) [pick[id], pickD[id]] = [k, d];
  }

  const labels: RegionLabel[] = [];
  for (let id = 1; id < count; id++) {
    const k = pick[id];
    if (k < 0) continue;
    labels.push({
      id,
      name: REGION_NAMES[id],
      x: X0 + ((k % N) + 0.5) * REGION_CELL,
      z: Z0 + (Math.floor(k / N) + 0.5) * REGION_CELL,
      room: best[id] * REGION_CELL,
    });
  }
  return labels.sort((a, b) => b.room - a.room);
}

// --- Galactic Mapping Project ---------------------------------------------------

/** A place a Galactic Mapping Project region's name can go. */
export interface MappedSpot {
  /** In the game's frame, ly. */
  x: number;
  z: number;
  /** How far it is from the outline, ly. */
  room: number;
  /**
   * Which way the region runs there, as an angle in the x-z plane (radians
   * from +x towards +z), or null where it's about as wide as it is long.
   */
  along: number | null;
}

export interface MappedLabel {
  name: string;
  /** Where its name can go, best first: the point deepest inside it, then others well apart along it. */
  spots: MappedSpot[];
}

/** A region runs one way when it's this many times longer than it's wide, near a name. */
const ELONGATED = 1.6;
/** Other spots for a name are at least this many times the best one's room from each other... */
const SPOT_APART = 3;
/** ...and have at least this share of its room. */
const SPOT_ROOM = 0.5;
const MAX_SPOTS = 4;

let mapped: MappedLabel[] | null = null;

/** The Galactic Mapping Project's region names and where they can go, largest room first. */
export function mappedLabels(): MappedLabel[] {
  mapped ??= gmp.regions.map((r) => placeName(r.name, r.outline)).sort((a, b) => b.spots[0].room - a.spots[0].room);
  return mapped;
}

/**
 * Finds where a region's name can go inside its outline (x, z pairs). The
 * outline is sampled on a grid; the deepest sample, refined on a finer grid
 * round it, is the best spot. A name there often lands on a codex region's
 * (the two maps often agree), so a few more spots well apart along the region
 * are kept for it to move to. Which way the region runs at each comes from
 * the spread of the samples round it.
 */
function placeName(name: string, outline: number[]): MappedLabel {
  let [x0, z0, x1, z1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (let k = 0; k < outline.length; k += 2) {
    x0 = Math.min(x0, outline[k]);
    x1 = Math.max(x1, outline[k]);
    z0 = Math.min(z0, outline[k + 1]);
    z1 = Math.max(z1, outline[k + 1]);
  }
  const step = Math.max(x1 - x0, z1 - z0) / 64;
  const samples: { x: number; z: number; d: number }[] = [];
  for (let x = x0 + step / 2; x < x1; x += step) {
    for (let z = z0 + step / 2; z < z1; z += step) {
      if (inside(outline, x, z)) samples.push({ x, z, d: toEdge(outline, x, z) });
    }
  }
  if (samples.length === 0) samples.push({ x: (x0 + x1) / 2, z: (z0 + z1) / 2, d: 0 });
  samples.sort((a, b) => b.d - a.d);

  const spots: MappedSpot[] = [];
  const apart = SPOT_APART * samples[0].d;
  for (const s of samples) {
    if (spots.length === MAX_SPOTS || s.d < SPOT_ROOM * samples[0].d) break;
    if (spots.some((p) => Math.hypot(p.x - s.x, p.z - s.z) < apart)) continue;
    // The deepest point near this sample, on a grid an eighth of the size.
    let best = s;
    for (let i = -8; i <= 8; i++) {
      for (let j = -8; j <= 8; j++) {
        const [x, z] = [s.x + (i * step) / 8, s.z + (j * step) / 8];
        if (!inside(outline, x, z)) continue;
        const d = toEdge(outline, x, z);
        if (d > best.d) best = { x, z, d };
      }
    }
    spots.push({ x: best.x, z: best.z, room: best.d, along: runs(samples, best, Math.max(4 * best.d, 2 * step)) });
  }
  return { name, spots };
}

/**
 * Which way a region runs at a point: the long axis of the spread
 * (covariance) of its samples within `reach`, or null if it isn't clearly
 * longer one way than the other.
 */
function runs(samples: { x: number; z: number }[], at: { x: number; z: number }, reach: number): number | null {
  let [n, sx, sz, sxx, szz, sxz] = [0, 0, 0, 0, 0, 0];
  for (const s of samples) {
    const [dx, dz] = [s.x - at.x, s.z - at.z];
    if (dx * dx + dz * dz > reach * reach) continue;
    n++;
    sx += dx;
    sz += dz;
    sxx += dx * dx;
    szz += dz * dz;
    sxz += dx * dz;
  }
  if (n < 3) return null;
  const [mx, mz] = [sx / n, sz / n];
  const [cxx, czz, cxz] = [sxx / n - mx * mx, szz / n - mz * mz, sxz / n - mx * mz];
  const mid = (cxx + czz) / 2;
  const spread = Math.hypot((cxx - czz) / 2, cxz);
  if (Math.sqrt((mid + spread) / Math.max(mid - spread, 1e-9)) < ELONGATED) return null;
  return Math.atan2(2 * cxz, cxx - czz) / 2;
}

/** Even-odd test: is (x, z) inside the outline? */
function inside(outline: number[], x: number, z: number): boolean {
  let within = false;
  const n = outline.length;
  for (let i = 0, j = n - 2; i < n; j = i, i += 2) {
    const [xi, zi, xj, zj] = [outline[i], outline[i + 1], outline[j], outline[j + 1]];
    if (zi > z !== zj > z && x < ((xj - xi) * (z - zi)) / (zj - zi) + xi) within = !within;
  }
  return within;
}

/** Distance from (x, z) to the nearest edge of the outline. */
function toEdge(outline: number[], x: number, z: number): number {
  let best = Infinity;
  const n = outline.length;
  for (let i = 0, j = n - 2; i < n; j = i, i += 2) {
    const [ax, az, bx, bz] = [outline[j], outline[j + 1], outline[i], outline[i + 1]];
    const [dx, dz] = [bx - ax, bz - az];
    const t = Math.min(Math.max(((x - ax) * dx + (z - az) * dz) / (dx * dx + dz * dz || 1), 0), 1);
    best = Math.min(best, Math.hypot(x - ax - t * dx, z - az - t * dz));
  }
  return best;
}
