// Draws the points of interest as shape-coded markers over the galaxy (it
// belongs in View.overlay), with name labels for the nearby ones.
//
// Each category has its own glyph, drawn by a signed distance function in
// the fragment shader; every marker is the same warm white with a dark
// outline, so they read over stars and over the overlay layers' coloured
// dots alike. The marker under the cursor and the selected one turn the
// accent colour. Markers keep their size on screen up close (they mark
// places rather than show how big anything is) and shrink to half size far
// off, so the whole galaxy doesn't vanish under them.

import * as THREE from "three";
import { CATEGORIES, SHAPES, categoryInfo, type Poi, type PoiCategory } from "../poi";
import { edToWorld } from "./coords";
import { pickPoint, projector } from "./pick";

/** Marker size in CSS pixels, up close. */
const BASE_SIZE = 13;
/** Markers further than this (ly) shrink, down to MIN_ZOOM of their size. */
const SIZE_REF_DISTANCE = 4000;
const MIN_ZOOM = 0.5;
/** Hovered and selected markers grow by this much. */
const HIGHLIGHT_SCALE = 1.35;
/** Labels only for markers nearer the camera than this (ly), and at most this many. */
const LABEL_RANGE = 2500;
const MAX_LABELS = 40;
const LABEL_FONT_PX = 11;

const MARKER = new THREE.Color("#f2ead8");
const ACCENT = new THREE.Color("#ff9a2e");
/** The map background (view.ts), so outlines read as gaps. */
const OUTLINE = new THREE.Vector3(3, 4, 10).divideScalar(255);

/** Drawn first (underneath) to last (on top). */
const DRAW_ORDER: PoiCategory[] = [...CATEGORIES.map((c) => c.id)].reverse();

const vertexShader = /* glsl */ `
  attribute float shape;
  attribute float size;
  uniform float uPixelRatio;
  uniform int uHover;
  uniform int uSelected;
  varying float vShape;
  varying float vPx;
  varying float vLit;

  void main() {
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_Position = projectionMatrix * mv;
    bool lit = gl_VertexID == uHover || gl_VertexID == uSelected;
    float zoom = clamp(${SIZE_REF_DISTANCE.toFixed(1)} / max(-mv.z, 1e-3), ${MIN_ZOOM.toFixed(2)}, 1.0);
    float px = size * zoom * (lit ? ${HIGHLIGHT_SCALE.toFixed(2)} : 1.0);
    gl_PointSize = px * uPixelRatio;
    vShape = shape;
    vPx = px;
    vLit = lit ? 1.0 : 0.0;
  }
`;

const fragmentShader = /* glsl */ `
  uniform vec3 uColour;
  uniform vec3 uAccent;
  uniform vec3 uOutline;
  varying float vShape;
  varying float vPx;
  varying float vLit;

  float sdBox(vec2 p, vec2 b) {
    vec2 q = abs(p) - b;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
  }

  // Equilateral triangle pointing up, centred (Inigo Quilez).
  float sdTriangle(vec2 p, float r) {
    const float k = 1.7320508;
    p.x = abs(p.x) - r;
    p.y = p.y + r / k;
    if (p.x + k * p.y > 0.0) p = vec2(p.x - k * p.y, -k * p.x - p.y) / 2.0;
    p.x -= clamp(p.x, -2.0 * r, 0.0);
    return -length(p) * sign(p.y);
  }

  float sdHexagon(vec2 p, float r) {
    const vec3 k = vec3(-0.8660254, 0.5, 0.5773503);
    p = abs(p.yx);
    p -= 2.0 * min(dot(k.xy, p), 0.0) * k.xy;
    p -= vec2(clamp(p.x, -k.z * r, k.z * r), r);
    return length(p) * sign(p.y);
  }

  float sdShape(vec2 p, int s) {
    if (s == ${SHAPES.dot}) return length(p) - 0.55;
    if (s == ${SHAPES.triangle}) return sdTriangle(p + vec2(0.0, 0.1), 0.72);
    if (s == ${SHAPES.triangleDown}) return sdTriangle(vec2(p.x, -p.y) + vec2(0.0, 0.1), 0.72);
    if (s == ${SHAPES.diamond}) return (abs(p.x) * 1.25 + abs(p.y) - 0.78) * 0.62;
    if (s == ${SHAPES.square}) return sdBox(p, vec2(0.5));
    if (s == ${SHAPES.ring}) return abs(length(p) - 0.52) - 0.14;
    if (s == ${SHAPES.ringDot}) return min(abs(length(p) - 0.56) - 0.11, length(p) - 0.2);
    if (s == ${SHAPES.star}) {
      vec2 a = abs(p);
      float thin = min(a.x / 0.24 + a.y / 0.82, a.x / 0.82 + a.y / 0.24) - 1.0;
      return thin * 0.2;
    }
    if (s == ${SHAPES.hexagon}) return sdHexagon(p, 0.56);
    if (s == ${SHAPES.plus}) return min(sdBox(p, vec2(0.66, 0.19)), sdBox(p, vec2(0.19, 0.66)));
    return length(p) - 0.5;
  }

  void main() {
    vec2 p = (gl_PointCoord - 0.5) * 2.0;
    p.y = -p.y;
    float d = sdShape(p, int(vShape + 0.5));
    // One CSS pixel in these units, and an outline of 1.5 px (1 px on small markers).
    float px = 2.0 / vPx;
    float outline = (vPx < 10.0 ? 1.0 : 1.5) * px;
    float alpha = 1.0 - smoothstep(outline - 0.5 * px, outline + 0.5 * px, d);
    if (alpha <= 0.0) discard;
    vec3 fill = vLit > 0.5 ? uAccent : uColour;
    vec3 c = mix(fill, uOutline, smoothstep(-0.5 * px, 0.5 * px, d));
    gl_FragColor = vec4(c, alpha);
  }
`;

export class PoiLayer {
  readonly object: THREE.Points;
  private readonly geometry = new THREE.BufferGeometry();
  private readonly material: THREE.ShaderMaterial;
  /** World positions (z flipped, see coords.ts). */
  private readonly world: Float32Array;
  private readonly sizes: Float32Array;
  /** Visible markers in draw order. */
  private order = new Uint32Array(0);
  private readonly index = new Map<Poi, number>();
  private hover = -1;
  private selected = -1;
  private labelsOn = true;
  private readonly labelPool: HTMLDivElement[] = [];
  /** Estimated label widths, by marker. */
  private readonly labelWidth: Float32Array;

  constructor(
    readonly pois: Poi[],
    private readonly labelRoot: HTMLElement,
  ) {
    const n = pois.length;
    this.world = new Float32Array(n * 3);
    this.sizes = new Float32Array(n);
    this.labelWidth = new Float32Array(n);
    const shapes = new Float32Array(n);
    const v = new THREE.Vector3();
    pois.forEach((p, i) => {
      edToWorld(p.pos, v).toArray(this.world, i * 3);
      shapes[i] = SHAPES[categoryInfo(p.category).shape];
      this.sizes[i] = markerSize(p);
      // Roughly: the panel font is ~0.55 em per character, plus padding.
      this.labelWidth[i] = p.name.length * LABEL_FONT_PX * 0.56 + 10;
      this.index.set(p, i);
    });
    this.geometry.setAttribute("position", new THREE.BufferAttribute(this.world, 3));
    this.geometry.setAttribute("shape", new THREE.BufferAttribute(shapes, 1));
    this.geometry.setAttribute("size", new THREE.BufferAttribute(this.sizes, 1));
    this.geometry.setIndex(new THREE.BufferAttribute(new Uint32Array(n), 1));
    this.geometry.setDrawRange(0, 0);
    this.material = new THREE.ShaderMaterial({
      vertexShader,
      fragmentShader,
      uniforms: {
        uPixelRatio: { value: 1 },
        uHover: { value: -1 },
        uSelected: { value: -1 },
        uColour: { value: MARKER },
        uAccent: { value: ACCENT },
        uOutline: { value: OUTLINE },
      },
      transparent: true,
      depthTest: false,
      depthWrite: false,
    });
    this.object = new THREE.Points(this.geometry, this.material);
    this.object.frustumCulled = false;
    // Over the populated-systems dots (-1), under the route and selection ring (0).
    this.object.renderOrder = -0.5;
    this.object.name = "points-of-interest";
  }

  /** Shows only markers whose category isn't in `hidden`. */
  setHidden(hidden: ReadonlySet<string>): void {
    const rank = new Map(DRAW_ORDER.map((c, k) => [c, k]));
    const shown: number[] = [];
    this.pois.forEach((p, i) => {
      if (!hidden.has(p.category)) shown.push(i);
    });
    shown.sort((a, b) => rank.get(this.pois[a].category)! - rank.get(this.pois[b].category)! || a - b);
    this.order = Uint32Array.from(shown);
    const index = this.geometry.getIndex()!;
    (index.array as Uint32Array).set(this.order);
    index.clearUpdateRanges();
    index.addUpdateRange(0, this.order.length);
    index.needsUpdate = true;
    this.geometry.setDrawRange(0, this.order.length);
  }

  get visible(): boolean {
    return this.object.visible;
  }

  set visible(on: boolean) {
    this.object.visible = on;
    if (!on) this.hideLabels();
  }

  /** Shows name labels for nearby markers. */
  set labels(on: boolean) {
    this.labelsOn = on;
    if (!on) this.hideLabels();
  }

  indexOf(poi: Poi): number {
    return this.index.get(poi) ?? -1;
  }

  setHover(i: number | null): void {
    this.hover = i ?? -1;
    this.material.uniforms.uHover.value = this.hover;
  }

  setSelected(i: number | null): void {
    this.selected = i ?? -1;
    this.material.uniforms.uSelected.value = this.selected;
  }

  /** The marker under (x, y) in CSS pixels from the canvas's top left, or the nearest within `radius`. */
  pick(camera: THREE.PerspectiveCamera, width: number, height: number, x: number, y: number, radius = 6): number | null {
    if (!this.object.visible || this.order.length === 0) return null;
    return pickPoint(camera, width, height, this.world, this.order, (i, depth) => this.sizes[i] * zoomAt(depth) * 0.4, x, y, radius);
  }

  /** Per frame: pixel ratio, and labels while the view is changing. */
  update(camera: THREE.PerspectiveCamera, width: number, height: number, pixelRatio: number, moving: boolean): void {
    this.material.uniforms.uPixelRatio.value = pixelRatio;
    if (this.object.visible && this.labelsOn && moving) this.placeLabels(camera, width, height);
  }

  /** Takes the layer off the map (after an update replaces it). */
  dispose(): void {
    this.object.removeFromParent();
    this.geometry.dispose();
    this.material.dispose();
    for (const el of this.labelPool) el.remove();
    this.labelPool.length = 0;
  }

  private hideLabels(): void {
    for (const el of this.labelPool) el.hidden = true;
  }

  /**
   * Labels the nearest markers on screen, nearest first, skipping any whose
   * label would overlap one already placed. The hovered and selected
   * markers always get theirs.
   */
  private placeLabels(camera: THREE.PerspectiveCamera, width: number, height: number): void {
    const project = projector(camera, width, height);
    const cam = camera.position;
    const w = this.world;
    const range2 = LABEL_RANGE * LABEL_RANGE;
    const near: { i: number; d2: number; sx: number; sy: number; r: number }[] = [];
    for (const i of this.order) {
      const d2 = (w[i * 3] - cam.x) ** 2 + (w[i * 3 + 1] - cam.y) ** 2 + (w[i * 3 + 2] - cam.z) ** 2;
      const lit = i === this.hover || i === this.selected;
      if (d2 > range2 && !lit) continue;
      const s = project(w[i * 3], w[i * 3 + 1], w[i * 3 + 2]);
      if (!s || s.sx < 0 || s.sy < 0 || s.sx > width || s.sy > height) continue;
      near.push({ i, d2: lit ? -1 : d2, sx: s.sx, sy: s.sy, r: this.sizes[i] * zoomAt(s.depth) * 0.5 });
    }
    near.sort((a, b) => a.d2 - b.d2);
    const placed: [number, number, number, number][] = [];
    let used = 0;
    for (const m of near) {
      if (used >= MAX_LABELS) break;
      const x0 = m.sx + m.r + 3;
      const y0 = m.sy - LABEL_FONT_PX * 0.75;
      const box: [number, number, number, number] = [x0, y0, x0 + this.labelWidth[m.i], y0 + LABEL_FONT_PX * 1.5];
      const clash = placed.some((b) => box[0] < b[2] && b[0] < box[2] && box[1] < b[3] && b[1] < box[3]);
      if (clash && m.d2 >= 0) continue;
      placed.push(box);
      const el = this.label(used++);
      if (el.textContent !== this.pois[m.i].name) el.textContent = this.pois[m.i].name;
      el.style.transform = `translate(${x0.toFixed(1)}px, ${y0.toFixed(1)}px)`;
      el.classList.toggle("lit", m.d2 < 0);
      el.hidden = false;
    }
    for (let k = used; k < this.labelPool.length; k++) this.labelPool[k].hidden = true;
  }

  private label(k: number): HTMLDivElement {
    let el = this.labelPool[k];
    if (!el) {
      el = document.createElement("div");
      el.className = "poi-label";
      this.labelRoot.append(el);
      this.labelPool.push(el);
    }
    return el;
  }
}

/** How much markers shrink at a view depth (as in the vertex shader). */
function zoomAt(depth: number): number {
  return Math.min(Math.max(SIZE_REF_DISTANCE / depth, MIN_ZOOM), 1);
}

function markerSize(p: Poi): number {
  if (p.category === "carrierHub") {
    // 10 carriers (the least a hub has) to ~120 at the busiest.
    const t = Math.min(Math.max(Math.log10(p.weight ?? 10) - 1, 0), 1.1);
    return BASE_SIZE * (0.75 + 0.45 * t);
  }
  if (p.category === "planetaryNebula") return BASE_SIZE * 0.8;
  return BASE_SIZE;
}
