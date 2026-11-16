// The galactic regions drawn over the galaxy (it belongs in View.overlay):
// hairlines along the edges between the game's codex regions on the galactic
// plane, each codex region's name inside it, and, a step quieter, the
// Galactic Mapping Project's names for parts of the galaxy.
//
// They follow the design system's map labels: Saira in map-label with the
// halo, level and the same size on screen at any zoom, like the other labels
// on the map. Codex names are body-strong (13px) and the Mapping Project's
// micro (11px) in text-muted, so the two read apart by size and lightness.
// The edges are the line token's hairline. The names are painted at the
// screen's own pixel size and drawn on whole pixels, so they're as sharp as
// the HTML labels, but they stay under the markers and pins.
//
// Names show where they fit: the codex region the view is centred on first,
// then the other codex regions by room, then the Mapping Project's by room.
// A Mapping Project name has a few spots along its region (see regions.ts):
// it keeps its spot while that's free and otherwise moves to the first free
// one. Like the rest of the map they appear and go without fading.
//
// The edges fade out as the view comes level with the plane, where they'd
// crowd into one line, and towards the map's rim, where the outermost ones
// simply stop.

import * as THREE from "three";
import { mappedLabels, regionAtlas } from "../regions";
import { SGR_A, edToWorld } from "./coords";
import { projector } from "./pick";
import { srgb } from "./poi";

/** Design-system `line`: rgba(255, 170, 60, 0.28), the decorative hairline. */
const LINE = "#ffaa3c";
const LINE_ALPHA = 0.28;
/**
 * The edges fade out between these distances from the galactic centre (ly).
 * The map's rim is 44,000 to 51,000 ly out.
 */
const RIM_FADE = [38_000, 47_000] as const;
/** Design-system `halo`: 0 0 3px bg-void, twice. */
const HALO = { colour: "#03040a", blurPx: 3, times: 2 };
/** Clear space kept round each name when deciding which ones fit, CSS pixels. */
const GAP_PX = 6;
/** Lines of a codex name longer than this are broken up (see `wrap`). */
const MAX_LINE_CHARS = 16;

/** A kind of name: a design-system text style and colour. */
interface Style {
  family: string;
  weight: number;
  sizePx: number;
  lineHeightPx: number;
  colour: string;
  /** Mapping Project names draw under the codex names. */
  renderOrder: number;
}

const SANS = `Saira, Bahnschrift, "Segoe UI", system-ui, sans-serif`;
/** body-strong in map-label. */
const CODEX: Style = { family: SANS, weight: 600, sizePx: 13, lineHeightPx: 18, colour: "#e6e1d6", renderOrder: -2 };
/** micro in text-muted. */
const MAPPED: Style = { family: SANS, weight: 400, sizePx: 11, lineHeightPx: 16, colour: "#8a92a6", renderOrder: -2.5 };

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

// A name is a quad of its texture's size in device pixels, centred on its
// spot's place on screen with its corner on a whole pixel.
const nameVertex = /* glsl */ `
  uniform vec3 uAnchor;
  uniform vec2 uSize;
  uniform vec2 uViewport;
  varying vec2 vUv;
  void main() {
    vec4 clip = projectionMatrix * viewMatrix * vec4(uAnchor, 1.0);
    vec2 centre = (clip.xy / clip.w * 0.5 + 0.5) * uViewport;
    vec2 corner = floor(centre - uSize * 0.5 + 0.5);
    vec2 px = corner + (position.xy + 0.5) * uSize;
    gl_Position = vec4(px / uViewport * 2.0 - 1.0, 0.0, 1.0);
    vUv = uv;
  }
`;

// The texture holds the canvas's sRGB as it is, written straight out.
const nameFragment = /* glsl */ `
  uniform sampler2D uMap;
  varying vec2 vUv;
  void main() {
    gl_FragColor = texture2D(uMap, vUv);
  }
`;

/** One quad for every name: corners at ±0.5, scaled to its texture in the shader. */
const QUAD = new THREE.PlaneGeometry(1, 1);

interface Name {
  style: Style;
  lines: string[];
  /** The codex region's id, or 0 for a Mapping Project name. */
  id: number;
  mesh: THREE.Mesh<THREE.PlaneGeometry, THREE.ShaderMaterial>;
  /** Its size on screen, CSS pixels. */
  width: number;
  height: number;
  /** World positions it can go at, on the plane, best first (a codex name has one). */
  spots: THREE.Vector3[];
  /** The one it's shown at. */
  spot: number;
}

export class RegionLayer {
  readonly object = new THREE.Group();
  private readonly edges: THREE.LineSegments<THREE.BufferGeometry, THREE.ShaderMaterial>;
  /** Largest room first. */
  private readonly codex: Name[];
  private readonly mapped: Name[];
  private on = true;
  /** The device pixels per CSS pixel the names are painted for. */
  private pixelRatio = 0;
  /** Show the Mapping Project's names as well as the codex regions'. */
  showMapped = true;

  private constructor() {
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
          uColour: { value: srgb(LINE) },
          uOpacity: { value: LINE_ALPHA },
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

    this.codex = atlas.labels.map((l) => name(CODEX, wrap(l.name), l.id, [edToWorld([l.x, 0, l.z])]));
    this.mapped = mappedLabels().map((l) =>
      name(
        MAPPED,
        [l.name],
        0,
        l.spots.map((s) => edToWorld([s.x, 0, s.z])),
      ),
    );
    this.object.add(this.edges, ...this.codex.map((n) => n.mesh), ...this.mapped.map((n) => n.mesh));
    this.object.name = "galactic-regions";
  }

  /** Builds the layer once the names' fonts have loaded. */
  static async create(): Promise<RegionLayer> {
    await Promise.all([CODEX, MAPPED].map((s) => document.fonts.load(font(s, 1)).catch(() => [])));
    return new RegionLayer();
  }

  get visible(): boolean {
    return this.on;
  }

  set visible(on: boolean) {
    this.on = on;
    this.object.visible = on;
  }

  /**
   * Per frame: shows the names that fit and places them; `focus` is the
   * codex region the view is centred on (0 for none), whose name goes first.
   * `width` and `height` are the canvas's size in CSS pixels.
   */
  update(camera: THREE.PerspectiveCamera, width: number, height: number, pixelRatio: number, focus: number): void {
    if (!this.on) return;
    const names = [...this.codex, ...this.mapped];
    // Painted again for a screen of another density (the window moved).
    if (pixelRatio !== this.pixelRatio) {
      this.pixelRatio = pixelRatio;
      for (const n of names) repaint(n, pixelRatio);
    }

    // The edges are only lines on the plane: seen level with it they're gone.
    const back = new THREE.Vector3().setFromMatrixColumn(camera.matrixWorld, 2);
    this.edges.material.uniforms.uOpacity.value = LINE_ALPHA * smoothstep(0.04, 0.2, Math.abs(back.y));

    // As three.js sizes the drawing buffer.
    const viewport = new THREE.Vector2(Math.floor(width * pixelRatio), Math.floor(height * pixelRatio));
    const project = projector(camera, width, height);
    const placed: [number, number, number, number][] = [];
    const centred = this.codex.find((n) => n.id === focus);
    const order = [...(centred ? [centred] : []), ...this.codex.filter((n) => n !== centred), ...this.mapped];
    for (const n of order) {
      n.mesh.visible = false;
      if (!n.id && !this.showMapped) continue;
      // The first spot where it fits, trying the one it's at first.
      for (const k of [n.spot, ...n.spots.keys()].filter((k, i, all) => all.indexOf(k) === i)) {
        const at = n.spots[k];
        const s = project(at.x, at.y, at.z);
        if (!s) continue;
        const box: [number, number, number, number] = [
          s.sx - n.width / 2 - GAP_PX,
          s.sy - n.height / 2 - GAP_PX,
          s.sx + n.width / 2 + GAP_PX,
          s.sy + n.height / 2 + GAP_PX,
        ];
        // Wholly on screen: a name cut off at the edge reads as another word.
        if (box[0] + GAP_PX < 0 || box[1] + GAP_PX < 0 || box[2] - GAP_PX > width || box[3] - GAP_PX > height) continue;
        if (placed.some((b) => box[0] < b[2] && b[0] < box[2] && box[1] < b[3] && b[1] < box[3])) continue;
        placed.push(box);
        n.spot = k;
        const u = n.mesh.material.uniforms;
        u.uAnchor.value.copy(at);
        u.uViewport.value.copy(viewport);
        n.mesh.visible = true;
        break;
      }
    }
  }
}

/** The canvas font for a style at a pixel ratio. */
function font(s: Style, pixelRatio: number): string {
  return `${s.weight} ${s.sizePx * pixelRatio}px ${s.family}`;
}

/** A name's mesh, to be shown at one of `spots`; painted on its first update. */
function name(style: Style, lines: string[], id: number, spots: THREE.Vector3[]): Name {
  const material = new THREE.ShaderMaterial({
    vertexShader: nameVertex,
    fragmentShader: nameFragment,
    uniforms: {
      uMap: { value: null },
      uAnchor: { value: new THREE.Vector3() },
      uSize: { value: new THREE.Vector2() },
      uViewport: { value: new THREE.Vector2(1, 1) },
    },
    transparent: true,
    depthTest: false,
    depthWrite: false,
  });
  const mesh = new THREE.Mesh(QUAD, material);
  mesh.frustumCulled = false;
  mesh.visible = false;
  mesh.renderOrder = style.renderOrder;
  return { style, lines, id, mesh, width: 0, height: 0, spots, spot: 0 };
}

/**
 * Paints a name's lines, centred, at the screen's own pixel size, with the
 * halo round them as CSS draws a text-shadow: blurred copies underneath,
 * then the text.
 */
function repaint(n: Name, pixelRatio: number): void {
  const s = n.style;
  const canvas = document.createElement("canvas");
  let c = canvas.getContext("2d")!;
  c.font = font(s, pixelRatio);
  const widths = n.lines.map((l) => c.measureText(l).width);
  const pad = Math.ceil(HALO.blurPx * 2 * pixelRatio);
  const line = s.lineHeightPx * pixelRatio;
  canvas.width = Math.ceil(Math.max(...widths)) + 2 * pad;
  canvas.height = Math.ceil(n.lines.length * line) + 2 * pad;
  // Resizing the canvas resets its state.
  c = canvas.getContext("2d")!;
  c.font = font(s, pixelRatio);
  c.textBaseline = "middle";
  const draw = () => n.lines.forEach((text, k) => c.fillText(text, (canvas.width - widths[k]) / 2, pad + (k + 0.5) * line));
  // The shadows alone: the text itself is drawn a canvas width off to the left.
  c.save();
  c.fillStyle = HALO.colour;
  c.shadowColor = HALO.colour;
  c.shadowBlur = HALO.blurPx * pixelRatio;
  c.shadowOffsetX = canvas.width;
  c.translate(-canvas.width, 0);
  for (let i = 0; i < HALO.times; i++) draw();
  c.restore();
  c.fillStyle = s.colour;
  draw();

  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.NoColorSpace;
  texture.minFilter = THREE.NearestFilter;
  texture.magFilter = THREE.NearestFilter;
  texture.generateMipmaps = false;
  const u = n.mesh.material.uniforms;
  (u.uMap.value as THREE.Texture | null)?.dispose();
  u.uMap.value = texture;
  u.uSize.value.set(canvas.width, canvas.height);
  n.width = canvas.width / pixelRatio;
  n.height = canvas.height / pixelRatio;
}

/**
 * A codex region's name broken into as few lines as keep each within
 * MAX_LINE_CHARS (one line if it's that short), as evenly as possible.
 * Breaks go at spaces and after hyphens: "Inner Orion-" "Perseus Conflux".
 */
function wrap(name: string): string[] {
  const words = name.match(/[^\s-]+-?/g) ?? [name];
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

function smoothstep(a: number, b: number, x: number): number {
  const t = THREE.MathUtils.clamp((x - a) / (b - a), 0, 1);
  return t * t * (3 - 2 * t);
}
