// The galactic regions drawn as an atlas over the galaxy (it belongs in
// View.overlay): hairlines along the edges between the game's codex regions
// on the galactic plane, each codex region's name inside it, and, more
// quietly, the Galactic Mapping Project's names for parts of the galaxy.
//
// Codex names (orange capitals) lie on the plane, turned to read the right
// way up from wherever the camera faces. As the view comes down towards the
// plane they tilt up to meet it, so they never lean back more than MAX_LEAN
// and stay legible seen side on.
//
// Mapping Project names (pale, widely spaced) lie flat on the plane: along
// the way their region runs where that's clear, like a river's name on a
// map, turned to read left to right. Flat names can't be read seen across
// the plane, so they fade out as the view comes down to it.
//
// Names are painted to scale (as on a map) while that's legible, and held
// between a smallest and a largest size on screen beyond it, so the whole
// galaxy's names can be read and a close view isn't one giant word. Where
// names would overlap, the one placed first keeps its place: the codex region
// the view is centred on, the other codex regions by room, then the Mapping
// Project's by room. A Mapping Project name has a few spots along its region
// (see regions.ts): it keeps its spot while that's free and otherwise moves
// to the first free one, fading out and back in.
//
// The edges fade out as the view comes level with the plane, where they'd
// crowd into one line, and towards the map's rim, where the outermost ones
// simply stop.

import * as THREE from "three";
import { mappedLabels, regionAtlas } from "../regions";
import { SGR_A, edToWorld } from "./coords";
import { projector, type Projector } from "./pick";
import { srgb } from "./poi";

/** Design-system accent (the Elite orange): the edges and codex names. */
const ACCENT = "#ff9a2e";
const EDGE_OPACITY = 0.32;
/**
 * The edges fade out between these distances from the galactic centre (ly).
 * The map's rim is 44,000 to 51,000 ly out.
 */
const RIM_FADE = [38_000, 47_000] as const;

/** How far a codex name may lean back from facing the camera before it tilts up off the plane. */
const MAX_LEAN = THREE.MathUtils.degToRad(48);
/** Flat names fade out as the line of sight to them comes down from this steep to this shallow (sine of its angle to the plane). */
const FLAT_FADE = [0.5, 0.3] as const;
/** Clear space kept round each name when deciding which ones fit, CSS pixels. */
const GAP_PX = 6;
/** How quickly names fade in and out, per second. */
const FADE_RATE = 8;
/** Lines of a codex name longer than this are broken up (see `wrap`). */
const MAX_LINE_CHARS = 14;

/** How a kind of name is painted and sized. */
interface Style {
  font: string;
  fontPx: number;
  lineEm: number;
  trackingEm: number;
  colour: string;
  /** The halo round each letter (bg-void), so names read over the bright core. */
  halo: string;
  haloEm: number;
  opacity: number;
  /** Line height when painted to scale (ly), from the room round the name. */
  lineLy: (room: number) => number;
  /** On-screen line height limits, CSS pixels. */
  minPx: number;
  maxPx: number;
  /** Mapping Project names draw under the codex names. */
  renderOrder: number;
}

/** Codex names: the display font (Antonio) in capitals, in the accent. */
const CODEX: Style = {
  font: `500 48px Antonio, "Bahnschrift Condensed", "Arial Narrow", sans-serif`,
  fontPx: 48,
  lineEm: 1.15,
  trackingEm: 0.12,
  colour: ACCENT,
  halo: "rgba(3, 4, 10, 0.85)",
  haloEm: 0.24,
  opacity: 0.8,
  lineLy: () => 1100,
  minPx: 16,
  maxPx: 30,
  renderOrder: -2,
};

/** Mapping Project names: the body font (Saira) as written, widely spaced, in map-label. */
const MAPPED: Style = {
  font: `400 40px Saira, Bahnschrift, "Segoe UI", sans-serif`,
  fontPx: 40,
  lineEm: 1.3,
  trackingEm: 0.3,
  colour: "#e6e1d6",
  halo: "rgba(3, 4, 10, 0.6)",
  haloEm: 0.18,
  opacity: 0.6,
  lineLy: (room) => THREE.MathUtils.clamp(room * 0.5, 250, 1000),
  minPx: 12,
  maxPx: 22,
  renderOrder: -2.5,
};

const edgeVertex = /* glsl */ `
  varying vec2 vPlane;
  void main() {
    vPlane = position.xz;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;

// Writes sRGB straight out: ShaderMaterial does no colour management.
const edgeFragment = /* glsl */ `
  uniform vec3 uColour;
  uniform float uOpacity;
  uniform vec2 uCore;
  varying vec2 vPlane;
  void main() {
    float rim = smoothstep(${RIM_FADE[0].toFixed(1)}, ${RIM_FADE[1].toFixed(1)}, distance(vPlane, uCore));
    gl_FragColor = vec4(uColour, uOpacity * (1.0 - rim));
  }
`;

/** A place a name can go. */
interface Spot {
  /** World position, on the plane. */
  at: THREE.Vector3;
  /** Room round it (ly), for the name's size. */
  room: number;
  /** For a Mapping Project name, the way it runs on the plane (world, unit); null to turn with the camera. */
  along: THREE.Vector3 | null;
}

interface Name {
  style: Style;
  /** The codex region's id, or 0 for a Mapping Project name. */
  id: number;
  mesh: THREE.Mesh<THREE.PlaneGeometry, THREE.MeshBasicMaterial>;
  /** Size in line heights. */
  width: number;
  height: number;
  /** Where it can go, best first (a codex name has one). */
  spots: Spot[];
  /** The one it's shown at. */
  spot: number;
  alpha: number;
}

/** How a name sits in this frame: its turn, its scale (ly per line), and how legible it is (0 to 1). */
interface Pose {
  basis: THREE.Matrix4;
  scale: number;
  legible: number;
}

/** A name's corners on screen, x and y in turn round it. */
type Quad = number[];

export class RegionLayer {
  readonly object = new THREE.Group();
  private readonly edges: THREE.LineSegments<THREE.BufferGeometry, THREE.ShaderMaterial>;
  /** Largest room first. */
  private readonly codex: Name[];
  private readonly mapped: Name[];
  private on = true;
  /** Show the Mapping Project's names as well as the codex regions'. */
  showMapped = true;

  private constructor(anisotropy: number) {
    const atlas = regionAtlas();
    const e = atlas.edges;
    const positions = new Float32Array((e.length / 4) * 6);
    for (let s = 0; s < e.length / 4; s++) {
      positions.set([e[s * 4], 0, -e[s * 4 + 1], e[s * 4 + 2], 0, -e[s * 4 + 3]], s * 6);
    }
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    const core = edToWorld(SGR_A);
    this.edges = new THREE.LineSegments(
      geometry,
      new THREE.ShaderMaterial({
        vertexShader: edgeVertex,
        fragmentShader: edgeFragment,
        uniforms: {
          uColour: { value: srgb(ACCENT) },
          uOpacity: { value: EDGE_OPACITY },
          uCore: { value: new THREE.Vector2(core.x, core.z) },
        },
        transparent: true,
        depthTest: false,
        depthWrite: false,
      }),
    );
    this.edges.frustumCulled = false;
    // Under the populated-systems dots (-1) and the markers above them.
    this.edges.renderOrder = -3;

    this.codex = atlas.labels.map((l) =>
      name(CODEX, wrap(l.name), l.id, [{ at: edToWorld([l.x, 0, l.z]), room: l.room, along: null }], anisotropy),
    );
    this.mapped = mappedLabels().map((l) => {
      const spots = l.spots.map((s) => ({
        at: edToWorld([s.x, 0, s.z]),
        room: s.room,
        // The game's frame has z the other way round (coords.ts).
        along: s.along === null ? null : new THREE.Vector3(Math.cos(s.along), 0, -Math.sin(s.along)),
      }));
      return name(MAPPED, [l.name], 0, spots, anisotropy);
    });
    this.object.add(this.edges, ...this.codex.map((n) => n.mesh), ...this.mapped.map((n) => n.mesh));
    this.object.name = "galactic-regions";
  }

  /** Builds the layer once the names' fonts have loaded. */
  static async create(anisotropy: number): Promise<RegionLayer> {
    await Promise.all([CODEX, MAPPED].map((s) => document.fonts.load(s.font).catch(() => [])));
    return new RegionLayer(anisotropy);
  }

  get visible(): boolean {
    return this.on;
  }

  set visible(on: boolean) {
    this.on = on;
    this.object.visible = on;
    // Shown again, the names fade back in rather than reappear as they were.
    if (!on) for (const n of [...this.codex, ...this.mapped]) n.alpha = 0;
  }

  /**
   * Per frame: turns and sizes the names to the camera and fades them in and
   * out; `focus` is the codex region the view is centred on (0 for none),
   * whose name goes first. Returns true while a fade is under way, to keep
   * the view rendering.
   */
  update(camera: THREE.PerspectiveCamera, width: number, height: number, dt: number, focus: number): boolean {
    if (!this.on) return false;
    const m = camera.matrixWorld.elements;
    const right = new THREE.Vector3(m[0], m[1], m[2]);
    const up = new THREE.Vector3(m[4], m[5], m[6]);
    const back = new THREE.Vector3(m[8], m[9], m[10]);
    const cam = camera.position;

    // The edges are only lines on the plane: seen level with it they're gone.
    this.edges.material.uniforms.uOpacity.value = EDGE_OPACITY * smoothstep(0.04, 0.2, Math.abs(back.y));

    // Codex names face the plane's side the camera is on, unless that leans
    // back from the camera more than MAX_LEAN: then they tilt up towards it.
    const plane = new THREE.Vector3(0, cam.y >= 0 ? 1 : -1, 0);
    const cosLean = plane.dot(back);
    let normal = plane;
    if (cosLean < Math.cos(MAX_LEAN)) {
      const towards = plane.clone().addScaledVector(back, -cosLean);
      // Level with the plane, the camera's up is the plane's normal.
      if (towards.lengthSq() < 1e-8) towards.copy(up);
      normal = back.clone().multiplyScalar(Math.cos(MAX_LEAN)).addScaledVector(towards.normalize(), Math.sin(MAX_LEAN));
    }
    // The camera's right is level (OrbitControls keeps up at +y), so the text runs along it.
    const standing = new THREE.Matrix4().makeBasis(right, normal.clone().cross(right).normalize(), normal);

    const forward = back.clone().negate();
    const pxPerLy = height / (2 * Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2));
    const project = projector(camera, width, height);
    const v = new THREE.Vector3();

    /** How name `n` sits at its spot `k` this frame, or null if that's behind the camera. */
    const pose = (n: Name, k: number): Pose | null => {
      const spot = n.spots[k];
      const depth = v.subVectors(spot.at, cam).dot(forward);
      if (depth <= camera.near) return null;
      let basis = standing;
      let legible = 1;
      if (!n.id) {
        // Flat on the plane, along the region or else along the camera's
        // right, read left to right; faded as the view comes down to it.
        const runs = (spot.along ?? right).clone();
        if (runs.dot(right) < 0) runs.negate();
        basis = new THREE.Matrix4().makeBasis(runs, plane.clone().cross(runs), plane);
        legible = smoothstep(FLAT_FADE[1], FLAT_FADE[0], Math.abs(v.subVectors(spot.at, cam).normalize().y));
      }
      const s = n.style;
      const px = THREE.MathUtils.clamp((s.lineLy(spot.room) * pxPerLy) / depth, s.minPx, s.maxPx);
      return { basis, scale: (px * depth) / pxPerLy, legible };
    };

    const placed: Quad[] = [];
    const fade = Math.min(1, dt * FADE_RATE);
    let fading = false;
    const centred = this.codex.find((n) => n.id === focus);
    const order = [...(centred ? [centred] : []), ...this.codex.filter((n) => n !== centred), ...this.mapped];
    for (const n of order) {
      // The first spot that's free, trying the one it's at first.
      let free = -1;
      let legible = 0;
      if (n.id || this.showMapped) {
        for (const k of [n.spot, ...n.spots.keys()].filter((k, i, all) => all.indexOf(k) === i)) {
          const p = pose(n, k);
          const quad = p && p.legible > 0 ? screenQuad(n, n.spots[k].at, p, project, v) : null;
          if (quad && onScreen(quad, width, height) && !placed.some((q) => overlaps(q, quad, GAP_PX))) {
            placed.push(quad);
            [free, legible] = [k, p!.legible];
            break;
          }
        }
      }
      // Moving to another spot, it fades out where it is first.
      if (free >= 0 && free !== n.spot && n.alpha === 0) n.spot = free;
      const target = free === n.spot ? legible : 0;
      n.alpha += (target - n.alpha) * fade;
      if (Math.abs(target - n.alpha) < 0.01) n.alpha = target;
      else fading = true;
      const shown = n.alpha > 0 ? pose(n, n.spot) : null;
      n.mesh.visible = !!shown;
      if (!shown) continue;
      n.mesh.material.opacity = n.style.opacity * n.alpha;
      n.mesh.matrix.copy(shown.basis).scale(v.setScalar(shown.scale)).setPosition(n.spots[n.spot].at);
      n.mesh.matrixWorldNeedsUpdate = true;
    }
    return fading;
  }
}

/** A name's mesh, painted in its style, to be shown at one of `spots`. */
function name(style: Style, lines: string[], id: number, spots: Spot[], anisotropy: number): Name {
  const { texture, width, height } = paint(lines, style, anisotropy);
  const mesh = new THREE.Mesh(
    new THREE.PlaneGeometry(width, height),
    new THREE.MeshBasicMaterial({ map: texture, transparent: true, opacity: 0, depthTest: false, depthWrite: false }),
  );
  mesh.matrixAutoUpdate = false;
  mesh.frustumCulled = false;
  mesh.visible = false;
  mesh.renderOrder = style.renderOrder;
  return { style, id, mesh, width, height, spots, spot: 0, alpha: 0 };
}

/** A name's corners on screen at `at`, or null if any is behind the camera. */
function screenQuad(n: Name, at: THREE.Vector3, p: Pose, project: Projector, v: THREE.Vector3): Quad | null {
  const quad: Quad = [];
  for (const [cx, cy] of [[-1, -1], [1, -1], [1, 1], [-1, 1]]) {
    v.set((cx * n.width) / 2, (cy * n.height) / 2, 0).applyMatrix4(p.basis).multiplyScalar(p.scale).add(at);
    const s = project(v.x, v.y, v.z);
    if (!s) return null;
    quad.push(s.sx, s.sy);
  }
  return quad;
}

function onScreen(q: Quad, width: number, height: number): boolean {
  const xs = [q[0], q[2], q[4], q[6]];
  const ys = [q[1], q[3], q[5], q[7]];
  return Math.max(...xs) > 0 && Math.min(...xs) < width && Math.max(...ys) > 0 && Math.min(...ys) < height;
}

/** Whether two quads on screen come within `gap` pixels of each other (separating axes). */
function overlaps(a: Quad, b: Quad, gap: number): boolean {
  for (const q of [a, b]) {
    for (let i = 0; i < 4; i++) {
      const j = (i + 1) % 4;
      const nx = q[j * 2 + 1] - q[i * 2 + 1];
      const ny = q[i * 2] - q[j * 2];
      const length = Math.hypot(nx, ny);
      if (length < 1e-6) continue;
      const [a0, a1] = extent(a, nx / length, ny / length);
      const [b0, b1] = extent(b, nx / length, ny / length);
      if (a1 + gap < b0 || b1 + gap < a0) return false;
    }
  }
  return true;
}

function extent(q: Quad, nx: number, ny: number): [number, number] {
  let [lo, hi] = [Infinity, -Infinity];
  for (let k = 0; k < 8; k += 2) {
    const d = q[k] * nx + q[k + 1] * ny;
    lo = Math.min(lo, d);
    hi = Math.max(hi, d);
  }
  return [lo, hi];
}

/**
 * A codex region's name in capitals, broken into as few lines as keep each
 * within MAX_LINE_CHARS (one line if it's that short), as evenly as possible.
 * Breaks go at spaces and after hyphens: "INNER ORION-" "PERSEUS" "CONFLUX".
 */
function wrap(name: string): string[] {
  const words = name.toUpperCase().replace(/'/g, "’").match(/[^\s-]+-?/g) ?? [name];
  const join = (ws: string[]) => ws.join(" ").replace(/- /g, "-");
  if (join(words).length <= MAX_LINE_CHARS) return [join(words)];
  let best: string[] = [join(words)];
  let bestLongest = Infinity;
  for (let lines = 2; lines <= Math.min(3, words.length); lines++) {
    // Every way of cutting the words into `lines` runs.
    const cuts = (start: number, left: number): number[][] =>
      left === 1 ? [[]] : Array.from({ length: words.length - start - left + 1 }, (_, k) => start + k + 1).flatMap((c) => cuts(c, left - 1).map((rest) => [c, ...rest]));
    for (const cut of cuts(0, lines)) {
      const bounds = [0, ...cut, words.length];
      const text = bounds.slice(1).map((end, k) => join(words.slice(bounds[k], end)));
      const longest = Math.max(...text.map((t) => t.length));
      if (longest < bestLongest) [best, bestLongest] = [text, longest];
    }
    if (bestLongest <= MAX_LINE_CHARS) break;
  }
  return best;
}

/** Paints a name's lines into a texture; its size comes back in line heights. */
function paint(lines: string[], style: Style, anisotropy: number): { texture: THREE.CanvasTexture; width: number; height: number } {
  const canvas = document.createElement("canvas");
  let c = canvas.getContext("2d")!;
  const tracking = Math.round(style.trackingEm * style.fontPx);
  const setFont = (ctx: CanvasRenderingContext2D) => {
    ctx.font = style.font;
    ctx.letterSpacing = `${tracking}px`;
  };
  setFont(c);
  // Letter spacing trails the last letter too; it isn't part of the name.
  const widths = lines.map((l) => c.measureText(l).width - tracking);
  const line = style.fontPx * style.lineEm;
  const pad = Math.ceil(style.fontPx * 0.3);
  canvas.width = Math.ceil(Math.max(...widths) + 2 * pad);
  canvas.height = Math.ceil(lines.length * line + 2 * pad);
  // Resizing the canvas resets its state.
  c = canvas.getContext("2d")!;
  setFont(c);
  c.textAlign = "left";
  c.textBaseline = "middle";
  c.lineJoin = "round";
  c.lineWidth = style.fontPx * style.haloEm;
  c.strokeStyle = style.halo;
  c.fillStyle = style.colour;
  lines.forEach((text, k) => {
    const x = (canvas.width - widths[k]) / 2;
    const y = pad + (k + 0.5) * line;
    c.strokeText(text, x, y);
    c.fillText(text, x, y);
  });
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.anisotropy = anisotropy;
  return { texture, width: canvas.width / line, height: canvas.height / line };
}

function smoothstep(a: number, b: number, x: number): number {
  const t = THREE.MathUtils.clamp((x - a) / (b - a), 0, 1);
  return t * t * (3 - 2 * t);
}
