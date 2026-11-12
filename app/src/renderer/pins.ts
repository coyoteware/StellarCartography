// Pins for the commander's own markers: you, friends, home, the squadron
// capital and your fleet carrier. Each is an LCARS block (a solid colour with
// the logo's big top-left corner, the pictogram cut out of it) standing up and
// to the right of its star and pointing at it from its bottom-left corner,
// with a dot on the star. Pins keep their size on screen (they mark places
// rather than show how big anything is), draw over everything, and are few
// enough to pick and label on the CPU.
//
// Pins that would overlap on screen stack instead (see `stackPins`): the
// later ones sit on top of the first, a hairline apart, without tail or dot.

import * as THREE from "three";
import type { Vec3 } from "../api";
import { edToWorld } from "./coords";
import { projector } from "./pick";

export type PinKind = "you" | "friend" | "home" | "squadron" | "carrier";

/**
 * Design-system colours: accent, lcars-ice, lcars-lilac, lcars-peach. Home and
 * the squadron capital are both jump-button bookmarks, so they share lilac.
 */
export const PIN_COLOUR: Record<PinKind, string> = {
  you: "#ff9a2e",
  friend: "#8fb8ff",
  home: "#c9a3e6",
  squadron: "#c9a3e6",
  carrier: "#ffc48c",
};
const VOID = "#03040a";

// Drawing units are CSS pixels on the map. The block is 24 square, its
// bottom-left tail ends at the anchor dot on the star.
const BLOCK = "M2.6 29.4L6 21.4V9A9 9 0 0 1 15 0H28A2 2 0 0 1 30 2V22A2 2 0 0 1 28 24H8.6Z";
/** The block on its own, for a pin stacked on another (no tail, no dot). */
const STACKED_BLOCK = "M6 9A9 9 0 0 1 15 0H28A2 2 0 0 1 30 2V22A2 2 0 0 1 28 24H8A2 2 0 0 1 6 22Z";
/** A stacked pin sits this far above the one below: the block and the space-hair gap. */
const STEP = 26;
const ANCHOR = { x: 1, y: 31.4 };
/** The texture's extent in drawing units, with room for the dot and smoothing. */
const BOX = { x: -1, y: -1, w: 32, h: 35 };
/** Where the block is, relative to the anchor, for picking. */
const HIT = { left: 4, right: 29, top: -31.4, bottom: -5 };
/** Labels start right of the block, level with its middle. */
const LABEL_DX = 31;
const LABEL_DY = -19.4;
const LABEL_FONT_PX = 11;
/** Texture pixels per drawing unit: crisp at 2x screens, mipmapped below. */
const TEXEL = 4;

type Glyph = (c: CanvasRenderingContext2D, fg: string, gap: string) => void;

const circle = (cx: number, cy: number, r: number) => {
  const p = new Path2D();
  p.arc(cx, cy, r, 0, Math.PI * 2);
  return p;
};

/** Pictograms in a 16-unit box; `fg` is the cut-out colour, `gap` the block's. */
const GLYPHS: Record<PinKind, Glyph> = {
  // A ship's arrowhead, pointing up.
  you: (c, fg) => {
    c.fillStyle = fg;
    c.fill(new Path2D("M8 1.2L14.6 14.8L8 11.4L1.4 14.8Z"));
  },
  // Two commanders, the second behind.
  friend: (c, fg, gap) => {
    c.fillStyle = fg;
    c.fill(circle(11.3, 4.3, 2.4));
    c.fill(new Path2D("M7.2 13.2V12C7.2 9.9 9.1 8.3 11.3 8.3S15.4 9.9 15.4 12V13.2Z"));
    c.strokeStyle = gap;
    c.lineWidth = 1.4;
    for (const p of [circle(5.9, 5.6, 2.9), new Path2D("M0.5 15V13.6C0.5 11.1 2.9 9.6 5.9 9.6S11.3 11.1 11.3 13.6V15Z")]) {
      c.stroke(p);
      c.fill(p);
    }
  },
  // A squadron's shield with a chevron in it.
  squadron: (c, fg, gap) => {
    c.fillStyle = fg;
    c.fill(new Path2D("M8 0.8L14.8 3.2V8C14.8 11.9 12 14.5 8 15.6C4 14.5 1.2 11.9 1.2 8V3.2Z"));
    c.fillStyle = gap;
    c.fill(new Path2D("M4.2 5.6L8 8.5L11.8 5.6V8.2L8 11.1L4.2 8.2Z"));
  },
  // A house with a star in it.
  home: (c, fg, gap) => {
    c.fillStyle = fg;
    c.fill(new Path2D("M8 0.8L15.6 8H13.6V15.2H2.4V8H0.4Z"));
    c.fillStyle = gap;
    c.fill(new Path2D("M8 7.3L8.9 9.7L11.3 10.6L8.9 11.5L8 13.9L7.1 11.5L4.7 10.6L7.1 9.7Z"));
  },
  // A fleet carrier from above, as on the points-of-interest map: pointed
  // hull, engines across the stern, the landing pad cut out.
  carrier: (c, fg, gap) => {
    c.fillStyle = fg;
    c.fill(new Path2D("M2 5.2H11.2L15.6 8L11.2 10.8H2Z"));
    c.fill(new Path2D("M0.9 3.4H2.5A0.5 0.5 0 0 1 3 3.9V12.1A0.5 0.5 0 0 1 2.5 12.6H0.9A0.5 0.5 0 0 1 0.4 12.1V3.9A0.5 0.5 0 0 1 0.9 3.4Z"));
    c.fillStyle = gap;
    c.fillRect(5.4, 6.4, 3.2, 3.2);
  },
};

const textures = new Map<string, THREE.Texture>();

/** The pin for `kind` (`stacked`: without its tail and dot), drawn once and shared. */
export function pinTexture(kind: PinKind, stacked = false): THREE.Texture {
  const key = stacked ? `${kind}-stacked` : kind;
  let t = textures.get(key);
  if (t) return t;
  const canvas = document.createElement("canvas");
  canvas.width = BOX.w * TEXEL;
  canvas.height = BOX.h * TEXEL;
  const c = canvas.getContext("2d")!;
  c.scale(TEXEL, TEXEL);
  c.translate(-BOX.x, -BOX.y);
  const colour = PIN_COLOUR[kind];
  c.fillStyle = colour;
  if (stacked) {
    c.fill(new Path2D(STACKED_BLOCK));
  } else {
    c.fill(new Path2D(BLOCK));
    c.fill(circle(ANCHOR.x, ANCHOR.y, 1.7));
  }
  c.translate(11, 5);
  c.scale(0.875, 0.875);
  GLYPHS[kind](c, VOID, colour);
  t = new THREE.CanvasTexture(canvas);
  t.colorSpace = THREE.SRGBColorSpace;
  t.minFilter = THREE.LinearMipmapLinearFilter;
  textures.set(key, t);
  return t;
}

export interface PinItem {
  pos: Vec3;
  /** Shown beside the pin; none when null. */
  label?: string | null;
}

/**
 * A set of pins of one kind. `object` goes in the view's overlay scene;
 * call `update` every frame (it keeps the pins their size on screen).
 */
export class PinSet<T extends PinItem = PinItem> {
  readonly object = new THREE.Group();
  items: T[] = [];
  private readonly material: THREE.SpriteMaterial;
  private readonly stackedMaterial: THREE.SpriteMaterial;
  private readonly sprites: THREE.Sprite[] = [];
  /** Each item's own star; a stacked pin's sprite sits on the stack's instead. */
  private readonly anchors: THREE.Vector3[] = [];
  /** How many pins each one sits above (0: on its own star). */
  private readonly lifts: number[] = [];
  private readonly labelPool: HTMLDivElement[] = [];
  /** Labels need placing although the view hasn't moved (after `set`). */
  private dirty = false;

  constructor(
    readonly kind: PinKind,
    /** Where labels go; none when null. */
    private readonly labelRoot: HTMLElement | null = null,
  ) {
    const material = (stacked: boolean) =>
      new THREE.SpriteMaterial({
        map: pinTexture(kind, stacked),
        sizeAttenuation: false,
        transparent: true,
        depthTest: false,
        depthWrite: false,
      });
    this.material = material(false);
    this.stackedMaterial = material(true);
    this.object.name = `${kind}-pins`;
    // Over the route, the selection ring and the other markers.
    this.object.renderOrder = 2;
  }

  set(items: T[]): void {
    this.items = items;
    while (this.sprites.length < items.length) {
      const s = new THREE.Sprite(this.material);
      s.center.set((ANCHOR.x - BOX.x) / BOX.w, 1 - (ANCHOR.y - BOX.y) / BOX.h);
      s.renderOrder = 2;
      s.frustumCulled = false;
      this.sprites.push(s);
      this.object.add(s);
    }
    this.sprites.forEach((s, i) => {
      s.visible = i < items.length;
      if (!s.visible) return;
      this.anchors[i] = edToWorld(items[i].pos, this.anchors[i]);
      this.place(i, this.anchors[i], 0);
    });
    this.dirty = true;
  }

  /** Item `i`'s own star. */
  anchor(i: number): THREE.Vector3 {
    return this.anchors[i];
  }

  /** Draws item `i` `lift` pins up from the pin on `base` (its own star when 0). */
  place(i: number, base: THREE.Vector3, lift: number): void {
    const s = this.sprites[i];
    if (this.lifts[i] === lift && s.position.equals(base)) return;
    this.lifts[i] = lift;
    s.position.copy(base);
    s.material = lift > 0 ? this.stackedMaterial : this.material;
    s.center.set((ANCHOR.x - BOX.x) / BOX.w, 1 - (ANCHOR.y + lift * STEP - BOX.y) / BOX.h);
    this.dirty = true;
  }

  /** Drawn: this set and every group it's in are visible. */
  get visible(): boolean {
    for (let o: THREE.Object3D | null = this.object; o; o = o.parent) if (!o.visible) return false;
    return true;
  }

  set visible(on: boolean) {
    this.object.visible = on;
    if (on) this.dirty = true;
    else this.hideLabels();
  }

  /** The pin under (x, y) in CSS pixels from the canvas's top left: its block, or within `radius` of its star. Nearest first. */
  pick(camera: THREE.PerspectiveCamera, width: number, height: number, x: number, y: number, radius = 6): number | null {
    if (!this.visible || this.items.length === 0) return null;
    const project = projector(camera, width, height);
    let best: number | null = null;
    let bestDepth = Infinity;
    for (let i = 0; i < this.items.length; i++) {
      const p = this.sprites[i].position;
      const s = project(p.x, p.y, p.z);
      if (!s || s.depth >= bestDepth) continue;
      const lift = this.lifts[i] ?? 0;
      const dx = x - s.sx;
      const dy = y - s.sy + lift * STEP;
      const onBlock = dx >= HIT.left && dx <= HIT.right && dy >= HIT.top && dy <= HIT.bottom;
      // Only the bottom pin of a stack has its dot on the star.
      if (onBlock || (lift === 0 && dx * dx + dy * dy <= radius * radius)) {
        best = i;
        bestDepth = s.depth;
      }
    }
    return best;
  }

  /** Per frame: sizes the pins for the screen, and places labels while the view moves. */
  update(camera: THREE.PerspectiveCamera, width: number, height: number, moving: boolean): void {
    if (!this.visible) {
      // Hidden with a parent group: take the labels down, put them back when shown.
      if (this.labelPool.some((el) => !el.hidden)) this.hideLabels();
      this.dirty = true;
      return;
    }
    // With sizeAttenuation off a sprite's scale is in units of the view's
    // height over the projection's y scale; this makes one drawing unit one CSS pixel.
    const k = 2 / (camera.projectionMatrix.elements[5] * height);
    for (const s of this.sprites) s.scale.set(BOX.w * k, BOX.h * k, 1);
    if (!this.labelRoot || (!moving && !this.dirty)) return;
    this.dirty = false;
    const project = projector(camera, width, height);
    let used = 0;
    for (let i = 0; i < this.items.length; i++) {
      const text = this.items[i].label;
      if (!text) continue;
      const p = this.sprites[i].position;
      const s = project(p.x, p.y, p.z);
      if (!s || s.sx < -40 || s.sy < 0 || s.sx > width || s.sy > height + 32) continue;
      const el = this.label(used++);
      if (el.textContent !== text) el.textContent = text;
      const y = s.sy + LABEL_DY - LABEL_FONT_PX * 0.75 - (this.lifts[i] ?? 0) * STEP;
      el.style.transform = `translate(${(s.sx + LABEL_DX).toFixed(1)}px, ${y.toFixed(1)}px)`;
      el.hidden = false;
    }
    for (let k = used; k < this.labelPool.length; k++) this.labelPool[k].hidden = true;
  }

  private hideLabels(): void {
    for (const el of this.labelPool) el.hidden = true;
  }

  private label(k: number): HTMLDivElement {
    let el = this.labelPool[k];
    if (!el) {
      el = document.createElement("div");
      el.className = `poi-label pin-label ${this.kind}`;
      this.labelRoot!.append(el);
      this.labelPool.push(el);
    }
    return el;
  }
}

/**
 * Keeps pins from hiding one another. Taking the sets in order (the first
 * goes to the bottom), a pin whose block would overlap one already placed
 * goes on top of that one's stack instead, a hairline above; only the bottom
 * pin of a stack keeps its tail and its dot on the star. Call every frame,
 * before the sets' `update`.
 */
export function stackPins(sets: readonly PinSet[], camera: THREE.PerspectiveCamera, width: number, height: number): void {
  const project = projector(camera, width, height);
  const stacks: { sx: number; sy: number; base: THREE.Vector3; size: number }[] = [];
  for (const set of sets) {
    const shown = set.visible;
    for (let i = 0; i < set.items.length; i++) {
      const own = set.anchor(i);
      const s = shown ? project(own.x, own.y, own.z) : null;
      const stack = s && stacks.find((t) => Math.abs(s.sx - t.sx) < HIT.right - HIT.left && Math.abs(s.sy - t.sy) < STEP);
      if (stack) {
        set.place(i, stack.base, stack.size++);
      } else {
        set.place(i, own, 0);
        if (s) stacks.push({ sx: s.sx, sy: s.sy, base: own, size: 1 });
      }
    }
  }
}
