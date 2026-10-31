// Level-of-detail point cloud over the octree tiles written by `ingest`
// (format documented in crates/galaxy-core/src/octree.rs).
//
// Every frame the visible nodes are chosen greedily by projected screen size
// until the point budget is spent. Each node holds a subsample of the region
// it covers and its descendants add the rest, so drawing a node together with
// its ancestors draws every star in that region.

import * as THREE from "three";
import { dataUrl, type Vec3 } from "../api";

interface IndexJson {
  version: number;
  root_min: Vec3;
  root_size: number;
  sample_grid: number;
  total_points: number;
  nodes: { key: string; count: number; children: number }[];
}

const HEADER_LEN = 24;
/** Refine a node only once it covers at least this many pixels on screen. */
const REFINE_PX = 160;
const MAX_CONCURRENT_LOADS = 8;
const MAX_LOAD_ATTEMPTS = 4;

class OctreeNode {
  readonly children: OctreeNode[] = [];
  /** World-space bounds (z flipped, see coords.ts). */
  readonly box: THREE.Box3;
  points: THREE.Points | null = null;
  rowids: Uint32Array | null = null;
  quantised: Uint16Array | null = null;
  state: "idle" | "loading" | "loaded" | "failed" = "idle";
  failures = 0;
  lastVisibleFrame = -1;

  constructor(
    readonly key: string,
    readonly count: number,
    readonly min: Vec3,
    readonly size: number,
  ) {
    this.box = new THREE.Box3(
      new THREE.Vector3(min[0], min[1], -(min[2] + size)),
      new THREE.Vector3(min[0] + size, min[1] + size, -min[2]),
    );
  }

  worldPosition(i: number, out: THREE.Vector3): THREE.Vector3 {
    const q = this.quantised!;
    const s = this.size / 65535;
    return out.set(this.min[0] + q[i * 3] * s, this.min[1] + q[i * 3 + 1] * s, -(this.min[2] + q[i * 3 + 2] * s));
  }
}

const vertexShader = /* glsl */ `
  attribute float massCode;
  uniform float uProjScale;
  uniform float uPixelRatio;
  varying vec3 vColour;
  varying float vAlpha;

  // Mass codes a..h run from small cool dwarfs to large hot stars.
  const vec3 COLOURS[8] = vec3[8](
    vec3(1.00, 0.46, 0.30), vec3(1.00, 0.62, 0.38), vec3(1.00, 0.78, 0.55), vec3(1.00, 0.92, 0.78),
    vec3(0.96, 0.96, 1.00), vec3(0.78, 0.86, 1.00), vec3(0.64, 0.76, 1.00), vec3(0.58, 0.68, 1.00)
  );

  void main() {
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_Position = projectionMatrix * mv;
    float dist = max(-mv.z, 1e-3);
    float m = massCode;
    float lum = 0.7 + 0.08 * m;
    // Apparent size: a disc ~0.25 ly across, clamped to a sensible pixel range.
    float px = 0.25 * uProjScale / dist;
    gl_PointSize = clamp(px * lum, 1.5, 9.0) * uPixelRatio;
    // Far away, stars fade almost out and the glow carries the shape of the
    // galaxy; this also hides the density steps between LOD levels.
    vAlpha = clamp(px * 2.5, 0.012, 1.0);
    vColour = COLOURS[int(m + 0.5)] * lum;
  }
`;

const fragmentShader = /* glsl */ `
  varying vec3 vColour;
  varying float vAlpha;
  void main() {
    vec2 c = gl_PointCoord - 0.5;
    float r2 = dot(c, c) * 4.0;
    if (r2 > 1.0) discard;
    float falloff = (1.0 - r2) * (1.0 - r2);
    gl_FragColor = vec4(vColour * falloff * vAlpha, 1.0);
  }
`;

export interface CloudStats {
  visibleNodes: number;
  visiblePoints: number;
  loadedNodes: number;
  loadedPoints: number;
  loading: number;
}

export interface PickResult {
  rowid: number;
  position: THREE.Vector3;
}

export class PointCloud {
  readonly object = new THREE.Group();
  readonly totalPoints: number;
  pointBudget = 5_000_000;

  private readonly root: OctreeNode;
  private readonly material: THREE.ShaderMaterial;
  private visible: OctreeNode[] = [];
  private loaded = new Set<OctreeNode>();
  private inflight = 0;
  private frame = 0;
  private readonly frustum = new THREE.Frustum();
  private readonly matrix = new THREE.Matrix4();

  static async load(): Promise<PointCloud> {
    const res = await fetch(dataUrl("tiles/index.json"));
    if (!res.ok) throw new Error(`tiles/index.json: HTTP ${res.status}`);
    return new PointCloud((await res.json()) as IndexJson);
  }

  private constructor(index: IndexJson) {
    this.totalPoints = index.total_points;
    const byKey = new Map<string, OctreeNode>();
    for (const n of index.nodes) {
      const { min, size } = nodeBounds(n.key, index.root_min, index.root_size);
      const node = new OctreeNode(n.key, n.count, min, size);
      byKey.set(n.key, node);
      byKey.get(n.key.slice(0, -1))?.children.push(node);
    }
    const root = byKey.get("r");
    if (!root) throw new Error("octree index has no root node");
    this.root = root;
    this.material = new THREE.ShaderMaterial({
      vertexShader,
      fragmentShader,
      uniforms: { uProjScale: { value: 1 }, uPixelRatio: { value: 1 } },
      transparent: true,
      depthWrite: false,
      depthTest: false,
      blending: THREE.AdditiveBlending,
    });
    this.object.name = "known-systems";
  }

  update(camera: THREE.PerspectiveCamera, viewportHeight: number, pixelRatio: number): void {
    this.frame++;
    const projScale = viewportHeight / (2 * Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2));
    this.material.uniforms.uProjScale.value = projScale;
    this.material.uniforms.uPixelRatio.value = pixelRatio;

    this.matrix.multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse);
    this.frustum.setFromProjectionMatrix(this.matrix);
    const eye = camera.position;

    // Greedy selection by projected size, largest first.
    const heap = new MaxHeap<OctreeNode>();
    heap.push(this.root, Infinity);
    let budget = this.pointBudget;
    const selected: OctreeNode[] = [];
    while (heap.size > 0) {
      const node = heap.pop()!;
      if (node.count > budget) continue;
      budget -= node.count;
      selected.push(node);
      for (const child of node.children) {
        if (!this.frustum.intersectsBox(child.box)) continue;
        const dist = child.box.distanceToPoint(eye);
        const projected = dist === 0 ? Infinity : (child.size * projScale) / dist;
        if (projected >= REFINE_PX) heap.push(child, projected);
      }
    }

    for (const node of this.visible) if (node.points) node.points.visible = false;
    for (const node of selected) {
      node.lastVisibleFrame = this.frame;
      if (node.points) node.points.visible = true;
      else if (node.state === "idle" && this.inflight < MAX_CONCURRENT_LOADS) void this.loadNode(node);
    }
    this.visible = selected;
    this.evict();
  }

  stats(): CloudStats {
    let visiblePoints = 0;
    for (const n of this.visible) if (n.points) visiblePoints += n.count;
    let loadedPoints = 0;
    for (const n of this.loaded) loadedPoints += n.count;
    return {
      visibleNodes: this.visible.filter((n) => n.points).length,
      visiblePoints,
      loadedNodes: this.loaded.size,
      loadedPoints,
      loading: this.inflight,
    };
  }

  /**
   * Nearest drawn star to the cursor, within `tolerancePx` pixels. Checks
   * every point in the visible nodes on the CPU, a few tens of ms at a 5M budget.
   */
  pick(camera: THREE.PerspectiveCamera, ndc: THREE.Vector2, viewportHeight: number, tolerancePx = 8): PickResult | null {
    const ray = new THREE.Raycaster();
    ray.setFromCamera(ndc, camera);
    const { origin, direction } = ray.ray;
    const projScale = viewportHeight / (2 * Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2));
    const tol = tolerancePx / projScale;
    const tol2 = tol * tol;
    let best: { node: OctreeNode; i: number } | null = null;
    let bestAngle = Infinity;
    let bestT = Infinity;
    const p = new THREE.Vector3();
    for (const node of this.visible) {
      if (!node.quantised) continue;
      for (let i = 0; i < node.count; i++) {
        node.worldPosition(i, p).sub(origin);
        const t = p.dot(direction);
        if (t <= camera.near) continue;
        const angle2 = (p.lengthSq() - t * t) / (t * t);
        if (angle2 > tol2) continue;
        // Prefer the star closest to the cursor; break near-ties by distance.
        if (angle2 < bestAngle * 0.8 || (angle2 < bestAngle * 1.25 && t < bestT)) {
          best = { node, i };
          bestAngle = angle2;
          bestT = t;
        }
      }
    }
    if (!best) return null;
    return { rowid: best.node.rowids![best.i], position: best.node.worldPosition(best.i, new THREE.Vector3()) };
  }

  private async loadNode(node: OctreeNode): Promise<void> {
    node.state = "loading";
    this.inflight++;
    try {
      const res = await fetch(dataUrl(`tiles/${node.key}.bin`));
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const buf = await res.arrayBuffer();
      const n = new DataView(buf).getUint32(4, true);
      if (n !== node.count) throw new Error(`expected ${node.count} points, tile has ${n}`);
      // Zero-copy views straight onto the tile: no decode step needed.
      node.rowids = new Uint32Array(buf, HEADER_LEN, n);
      node.quantised = new Uint16Array(buf, HEADER_LEN + 4 * n, 3 * n);
      const mass = new Uint8Array(buf, HEADER_LEN + 10 * n, n);

      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(node.quantised, 3, true));
      geometry.setAttribute("massCode", new THREE.BufferAttribute(mass, 1, false));
      geometry.boundingSphere = new THREE.Sphere(new THREE.Vector3(0.5, 0.5, 0.5), Math.sqrt(3) / 2);

      const points = new THREE.Points(geometry, this.material);
      points.position.set(node.min[0], node.min[1], -node.min[2]);
      points.scale.set(node.size, node.size, -node.size);
      points.frustumCulled = false;
      points.matrixAutoUpdate = false;
      points.updateMatrix();
      points.visible = node.lastVisibleFrame === this.frame;
      node.points = points;
      node.state = "loaded";
      this.loaded.add(node);
      this.object.add(points);
    } catch (e) {
      // Retry with backoff (fetches also fail transiently while the app reloads).
      node.failures++;
      node.state = "failed";
      if (node.failures < MAX_LOAD_ATTEMPTS) {
        window.setTimeout(() => (node.state = "idle"), 500 * 2 ** node.failures);
      } else {
        console.error(`tile ${node.key}: giving up after ${node.failures} attempts:`, e);
      }
    } finally {
      this.inflight--;
    }
  }

  /** Drops the least recently visible tiles once more than 2.5x the budget is loaded. */
  private evict(): void {
    let loadedPoints = 0;
    for (const n of this.loaded) loadedPoints += n.count;
    const limit = this.pointBudget * 2.5;
    if (loadedPoints <= limit) return;
    const candidates = [...this.loaded]
      .filter((n) => n.lastVisibleFrame !== this.frame)
      .sort((a, b) => a.lastVisibleFrame - b.lastVisibleFrame);
    for (const node of candidates) {
      if (loadedPoints <= limit) break;
      this.object.remove(node.points!);
      node.points!.geometry.dispose();
      node.points = null;
      node.rowids = null;
      node.quantised = null;
      node.state = "idle";
      this.loaded.delete(node);
      loadedPoints -= node.count;
    }
  }
}

function nodeBounds(key: string, rootMin: Vec3, rootSize: number): { min: Vec3; size: number } {
  const min: Vec3 = [...rootMin];
  let size = rootSize;
  for (let i = 1; i < key.length; i++) {
    size /= 2;
    const octant = key.charCodeAt(i) - 48;
    for (let axis = 0; axis < 3; axis++) if (octant & (1 << axis)) min[axis] += size;
  }
  return { min, size };
}

class MaxHeap<T> {
  private items: T[] = [];
  private keys: number[] = [];

  get size(): number {
    return this.items.length;
  }

  push(item: T, key: number): void {
    this.items.push(item);
    this.keys.push(key);
    let i = this.items.length - 1;
    while (i > 0) {
      const parent = (i - 1) >> 1;
      if (this.keys[parent] >= this.keys[i]) break;
      this.swap(i, parent);
      i = parent;
    }
  }

  pop(): T | undefined {
    if (this.items.length === 0) return undefined;
    const top = this.items[0];
    const lastItem = this.items.pop()!;
    const lastKey = this.keys.pop()!;
    if (this.items.length > 0) {
      this.items[0] = lastItem;
      this.keys[0] = lastKey;
      let i = 0;
      for (;;) {
        const l = 2 * i + 1;
        const r = l + 1;
        let largest = i;
        if (l < this.keys.length && this.keys[l] > this.keys[largest]) largest = l;
        if (r < this.keys.length && this.keys[r] > this.keys[largest]) largest = r;
        if (largest === i) break;
        this.swap(i, largest);
        i = largest;
      }
    }
    return top;
  }

  private swap(a: number, b: number): void {
    [this.items[a], this.items[b]] = [this.items[b], this.items[a]];
    [this.keys[a], this.keys[b]] = [this.keys[b], this.keys[a]];
  }
}
