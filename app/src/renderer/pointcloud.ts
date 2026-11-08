// Level-of-detail point cloud over the octree tiles written by `ingest`
// (format documented in crates/galaxy-core/src/octree.rs).
//
// Every frame the visible nodes are chosen greedily by projected screen size
// until the point budget is spent. Each node holds a subsample of the region
// it covers and its descendants add the rest, so drawing a node together with
// its ancestors draws every star in that region.
//
// Where a node is drawn but its child in some octant isn't, the points drawn
// there stand in for the child's stars too. The drawn part of the tree goes
// into a small texture each frame; the vertex shader walks it to find how
// many stars each point represents and scales its brightness to match, so
// regions drawn at different depths come out equally bright. That needs
// values above 1, hence the HDR target in view.ts.

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
/** LOD tree texture: four RGBA texels per drawn node (8 child slots, 8 weights). */
const TREE_WIDTH = 1024;
const TREE_ROWS = 32;
const MAX_TREE_NODES = (TREE_WIDTH * TREE_ROWS) / 4;

class OctreeNode {
  readonly children: OctreeNode[] = [];
  readonly byOctant: (OctreeNode | null)[] = new Array(8).fill(null);
  /** Points in this node and all of its descendants. */
  subtree: number;
  /** World-space bounds (z flipped, see coords.ts). */
  readonly box: THREE.Box3;
  points: THREE.Points | null = null;
  material: THREE.ShaderMaterial | null = null;
  pickMaterial: THREE.ShaderMaterial | null = null;
  rowids: Uint32Array | null = null;
  quantised: Uint16Array | null = null;
  /** This node's own points per octant, counted when the tile loads. */
  octantCounts: Uint32Array | null = null;
  state: "idle" | "loading" | "loaded" | "failed" = "idle";
  failures = 0;
  lastVisibleFrame = -1;
  /** Slot in the LOD tree texture while drawn, else -1. */
  treeIndex = -1;
  /** First pick id of this node's points in the current pick pass. */
  pickBase = 0;

  constructor(
    readonly key: string,
    readonly count: number,
    readonly min: Vec3,
    readonly size: number,
  ) {
    this.subtree = count;
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

// Shared by the draw and pick shaders. `position` is the point's place inside
// its own node, 0..1 on each axis in the game's frame.
const commonVertex = /* glsl */ `
  attribute float massCode;
  uniform float uProjScale;
  uniform float uPixelRatio;
  uniform sampler2D uLodTree;
  uniform int uNodeIndex;

  vec4 treeTexel(int i) {
    return texelFetch(uLodTree, ivec2(i % ${TREE_WIDTH}, i / ${TREE_WIDTH}), 0);
  }

  // How many stars this point stands in for. Walks down the drawn nodes from
  // the point's own; where the next node down isn't drawn, the texture holds
  // 1 + (stars under that child) / (this node's points in that octant).
  float lodWeight(vec3 p) {
    int node = uNodeIndex;
    if (node < 0) return 1.0;
    for (int level = 0; level < 16; level++) {
      ivec3 b = ivec3(step(vec3(0.5), p));
      int octant = b.x | (b.y << 1) | (b.z << 2);
      vec4 kids = treeTexel(node * 4 + (octant >> 2));
      float child = kids[octant & 3];
      if (child < 0.0) {
        vec4 weights = treeTexel(node * 4 + 2 + (octant >> 2));
        return weights[octant & 3];
      }
      node = int(child);
      p = p * 2.0 - vec3(b);
    }
    return 1.0;
  }

  // Apparent size in pixels of a disc ~0.25 ly across.
  float starPx(vec4 mv) {
    return 0.25 * uProjScale / max(-mv.z, 1e-3);
  }

  float spriteSize(float px, float lum) {
    return clamp(px * lum, 1.5, 9.0) * uPixelRatio;
  }
`;

const vertexShader = /* glsl */ `
  ${commonVertex}
  uniform float uGain;
  uniform float uFalloff;
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
    float px = starPx(mv);
    float m = massCode;
    float lum = 0.7 + 0.08 * m;
    gl_PointSize = spriteSize(px, lum);
    // Unresolved stars dim with the square of distance, so far away the glow
    // carries the shape of the galaxy. That also keeps the weighted points
    // faint enough for tone mapping to stay near linear, which is what makes
    // regions at different depths add up to the same brightness. (Growing
    // the sprites instead, Potree style, turned distant stars into fog.)
    float fade = pow(clamp(px * 2.5, 0.0, 1.0), uFalloff);
    vAlpha = fade * lodWeight(position) * uGain;
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

// Pick pass: each point writes its id (pick base + vertex index + 1) as four
// bytes; 0 means no star.
const pickVertexShader = /* glsl */ `
  ${commonVertex}
  uniform int uPickBase;
  flat varying vec4 vPick;

  void main() {
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_Position = projectionMatrix * mv;
    gl_PointSize = spriteSize(starPx(mv), 0.7 + 0.08 * massCode);
    int id = uPickBase + gl_VertexID + 1;
    vPick = vec4(ivec4(id, id >> 8, id >> 16, id >> 24) & 255) / 255.0;
  }
`;

const pickFragmentShader = /* glsl */ `
  flat varying vec4 vPick;
  void main() {
    vec2 c = gl_PointCoord - 0.5;
    if (dot(c, c) > 0.25) discard;
    gl_FragColor = vPick;
  }
`;

export interface CloudStats {
  visibleNodes: number;
  visiblePoints: number;
  loadedNodes: number;
  loadedPoints: number;
  loading: number;
  failed: number;
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
  /** Uniforms shared by every node's material. */
  private readonly shared = {
    uProjScale: { value: 1 },
    uPixelRatio: { value: 1 },
    uLodTree: { value: null as THREE.DataTexture | null },
    uGain: { value: 1 },
    uFalloff: { value: 2 },
  };
  private readonly treeData = new Float32Array(TREE_WIDTH * TREE_ROWS * 4);
  private readonly treeTexture: THREE.DataTexture;
  private visible: OctreeNode[] = [];
  /** Visible nodes that are loaded, in tree texture order. */
  private drawn: OctreeNode[] = [];
  private loaded = new Set<OctreeNode>();
  private inflight = 0;
  private failed = 0;
  private frame = 0;
  private readonly frustum = new THREE.Frustum();
  private readonly matrix = new THREE.Matrix4();
  private readonly pickCamera = new THREE.PerspectiveCamera();
  private readonly pickTarget = new THREE.WebGLRenderTarget(1, 1, { depthBuffer: true });
  private pickBuffer = new Uint8Array(4);

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
      const parent = byKey.get(n.key.slice(0, -1));
      if (parent) {
        parent.children.push(node);
        parent.byOctant[n.key.charCodeAt(n.key.length - 1) - 48] = node;
      }
    }
    const root = byKey.get("r");
    if (!root) throw new Error("octree index has no root node");
    this.root = root;
    sumSubtrees(root);

    this.treeTexture = new THREE.DataTexture(this.treeData, TREE_WIDTH, TREE_ROWS, THREE.RGBAFormat, THREE.FloatType);
    this.treeTexture.minFilter = this.treeTexture.magFilter = THREE.NearestFilter;
    this.treeTexture.needsUpdate = true;
    this.shared.uLodTree.value = this.treeTexture;
    this.object.name = "known-systems";
  }

  /** Overall star brightness, before tone mapping. */
  set gain(v: number) {
    this.shared.uGain.value = v;
  }

  /** True while tiles are loading (the view should keep rendering). */
  get busy(): boolean {
    return this.inflight > 0;
  }

  /** The shader's tuning uniforms, for experimenting from DevTools. */
  get tuning(): Record<string, { value: unknown }> {
    return this.shared;
  }

  update(camera: THREE.PerspectiveCamera, viewportHeight: number, pixelRatio: number): void {
    this.frame++;
    const projScale = viewportHeight / (2 * Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2));
    this.shared.uProjScale.value = projScale;
    this.shared.uPixelRatio.value = pixelRatio;

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
    const drawn: OctreeNode[] = [];
    for (const node of selected) {
      node.lastVisibleFrame = this.frame;
      if (node.points) {
        node.points.visible = true;
        drawn.push(node);
      } else if (node.state === "idle" && this.inflight < MAX_CONCURRENT_LOADS) void this.loadNode(node);
    }
    this.visible = selected;
    if (!sameNodes(drawn, this.drawn)) this.uploadTree(drawn);
    this.evict();
  }

  stats(): CloudStats {
    let visiblePoints = 0;
    for (const n of this.drawn) visiblePoints += n.count;
    let loadedPoints = 0;
    for (const n of this.loaded) loadedPoints += n.count;
    return {
      visibleNodes: this.drawn.length,
      visiblePoints,
      loadedNodes: this.loaded.size,
      loadedPoints,
      loading: this.inflight,
      failed: this.failed,
    };
  }

  /**
   * The drawn star nearest to (x, y), in CSS pixels from the canvas's top
   * left, within `radius` pixels. Renders the point ids of a small window
   * around the cursor on the GPU and reads them back; nearer stars win where
   * they overlap.
   */
  pick(renderer: THREE.WebGLRenderer, camera: THREE.PerspectiveCamera, x: number, y: number, radius = 6): PickResult | null {
    if (!this.object.visible || this.drawn.length === 0) return null;
    const canvas = renderer.domElement;
    const span = 2 * radius + 1;
    const size = Math.ceil(span * renderer.getPixelRatio());
    if (this.pickBuffer.length !== size * size * 4) this.pickBuffer = new Uint8Array(size * size * 4);
    this.pickTarget.setSize(size, size);

    const cam = this.pickCamera.copy(camera);
    cam.setViewOffset(canvas.clientWidth, canvas.clientHeight, x - radius, y - radius, span, span);

    let base = 0;
    for (const node of this.drawn) {
      node.pickBase = base;
      base += node.count;
      const pm = (node.pickMaterial ??= this.createMaterial(node, true));
      pm.uniforms.uPickBase.value = node.pickBase;
      node.points!.material = pm;
    }
    const prevTarget = renderer.getRenderTarget();
    const prevClear = renderer.getClearColor(new THREE.Color());
    const prevAlpha = renderer.getClearAlpha();
    try {
      renderer.setRenderTarget(this.pickTarget);
      renderer.setClearColor(0x000000, 0);
      renderer.clear();
      renderer.render(this.object, cam);
      renderer.readRenderTargetPixels(this.pickTarget, 0, 0, size, size, this.pickBuffer);
    } finally {
      renderer.setRenderTarget(prevTarget);
      renderer.setClearColor(prevClear, prevAlpha);
      for (const node of this.drawn) node.points!.material = node.material!;
    }

    // Nearest non-empty pixel to the centre of the window.
    const buf = this.pickBuffer;
    const centre = (size - 1) / 2;
    let bestId = 0;
    let bestD = Infinity;
    for (let row = 0; row < size; row++) {
      for (let col = 0; col < size; col++) {
        const o = (row * size + col) * 4;
        const id = (buf[o] | (buf[o + 1] << 8) | (buf[o + 2] << 16) | (buf[o + 3] << 24)) >>> 0;
        if (id === 0) continue;
        const d = (row - centre) ** 2 + (col - centre) ** 2;
        if (d < bestD) {
          bestD = d;
          bestId = id;
        }
      }
    }
    if (bestId === 0) return null;
    const index = bestId - 1;
    const node = this.drawn.find((n) => index >= n.pickBase && index < n.pickBase + n.count);
    if (!node?.rowids) return null;
    const i = index - node.pickBase;
    return { rowid: node.rowids[i], position: node.worldPosition(i, new THREE.Vector3()) };
  }

  /** Writes the drawn nodes' child slots and octant weights into the tree texture. */
  private uploadTree(drawn: OctreeNode[]): void {
    for (const node of this.drawn) node.treeIndex = -1;
    drawn.forEach((node, i) => (node.treeIndex = i < MAX_TREE_NODES ? i : -1));
    const data = this.treeData;
    for (const node of drawn) {
      if (node.treeIndex < 0) continue;
      const o = node.treeIndex * 16;
      for (let k = 0; k < 8; k++) {
        const child = node.byOctant[k];
        data[o + k] = child ? child.treeIndex : -1;
        data[o + 8 + k] = child ? 1 + child.subtree / Math.max(node.octantCounts![k], 1) : 1;
      }
      node.material!.uniforms.uNodeIndex.value = node.treeIndex;
      if (node.pickMaterial) node.pickMaterial.uniforms.uNodeIndex.value = node.treeIndex;
    }
    this.treeTexture.needsUpdate = true;
    this.drawn = drawn;
  }

  private createMaterial(node: OctreeNode, pick: boolean): THREE.ShaderMaterial {
    const uniforms = { ...this.shared, uNodeIndex: { value: node.treeIndex } };
    if (pick) {
      return new THREE.ShaderMaterial({
        vertexShader: pickVertexShader,
        fragmentShader: pickFragmentShader,
        uniforms: { ...uniforms, uPickBase: { value: 0 } },
        blending: THREE.NoBlending,
      });
    }
    return new THREE.ShaderMaterial({
      vertexShader,
      fragmentShader,
      uniforms,
      transparent: true,
      depthWrite: false,
      depthTest: false,
      blending: THREE.AdditiveBlending,
    });
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
      node.octantCounts = countOctants(node.quantised);

      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(node.quantised, 3, true));
      geometry.setAttribute("massCode", new THREE.BufferAttribute(mass, 1, false));
      geometry.boundingSphere = new THREE.Sphere(new THREE.Vector3(0.5, 0.5, 0.5), Math.sqrt(3) / 2);

      node.material = this.createMaterial(node, false);
      const points = new THREE.Points(geometry, node.material);
      points.position.set(node.min[0], node.min[1], -node.min[2]);
      points.scale.set(node.size, node.size, -node.size);
      points.frustumCulled = false;
      points.matrixAutoUpdate = false;
      points.updateMatrix();
      // Shown from the next update, once it has a slot in the tree texture.
      points.visible = false;
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
        this.failed++;
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
      node.material!.dispose();
      node.pickMaterial?.dispose();
      node.points = node.material = node.pickMaterial = null;
      node.rowids = node.quantised = node.octantCounts = null;
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

function sumSubtrees(node: OctreeNode): number {
  for (const child of node.children) node.subtree += sumSubtrees(child);
  return node.subtree;
}

/** Points per octant of the node, using the same split as the shader. */
function countOctants(q: Uint16Array): Uint32Array {
  const counts = new Uint32Array(8);
  for (let i = 0; i < q.length; i += 3) {
    counts[(q[i] >= 32768 ? 1 : 0) | (q[i + 1] >= 32768 ? 2 : 0) | (q[i + 2] >= 32768 ? 4 : 0)]++;
  }
  return counts;
}

function sameNodes(a: OctreeNode[], b: OctreeNode[]): boolean {
  return a.length === b.length && a.every((n, i) => n === b[i]);
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
