// Draws the populated systems as flat coloured dots over the tone-mapped
// galaxy (it belongs in View.overlay), styled by an OverlayStyle from
// overlays.ts. Dots keep exact palette colours: no additive blending and no
// tone mapping. Dots big enough to carry one get a thin ring in the
// background colour, so neighbours stay apart where they overlap; on small
// dots the ring would swallow the colour.
//
// Picking is on the CPU: projecting ~150k points takes a millisecond or two,
// cheap next to the GPU id pass the star tiles need.

import * as THREE from "three";
import type { OverlayStyle } from "../overlays";
import type { PopulatedData } from "./populated-data";

/** Distance (ly) at which dots are drawn at their base size; nearer they grow, further they shrink. */
const SIZE_REF_DISTANCE = 300;
const MIN_ZOOM = 0.6;
const MAX_ZOOM = 2.2;

const vertexShader = /* glsl */ `
  attribute vec3 colour;
  attribute float size;
  uniform float uPixelRatio;
  varying vec3 vColour;
  varying float vRing;

  void main() {
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_Position = projectionMatrix * mv;
    float zoom = clamp(${SIZE_REF_DISTANCE.toFixed(1)} / max(-mv.z, 1e-3), ${MIN_ZOOM.toFixed(2)}, ${MAX_ZOOM.toFixed(2)});
    float px = size * zoom;
    gl_PointSize = px * uPixelRatio;
    vColour = colour;
    // Where the ring starts, as a fraction of the radius: about 1 CSS px wide,
    // and only on dots at least 4 px across.
    vRing = px >= 4.0 ? 1.0 - 2.0 / px : 2.0;
  }
`;

const fragmentShader = /* glsl */ `
  uniform vec3 uRing;
  varying vec3 vColour;
  varying float vRing;

  void main() {
    float r = length(gl_PointCoord - 0.5) * 2.0;
    if (r > 1.0) discard;
    // Palette colours are sRGB already; write them as they are.
    gl_FragColor = vec4(r > vRing ? uRing : vColour, 1.0);
  }
`;

export class PopulatedLayer {
  readonly object: THREE.Points;
  private readonly geometry = new THREE.BufferGeometry();
  private readonly material: THREE.ShaderMaterial;
  /** World positions (z flipped, see coords.ts). */
  private readonly world: Float32Array;
  private style: OverlayStyle | null = null;
  private readonly matrix = new THREE.Matrix4();

  constructor(readonly data: PopulatedData) {
    this.world = new Float32Array(data.pos.length);
    for (let i = 0; i < data.pos.length; i += 3) {
      this.world[i] = data.pos[i];
      this.world[i + 1] = data.pos[i + 1];
      this.world[i + 2] = -data.pos[i + 2];
    }
    this.geometry.setAttribute("position", new THREE.BufferAttribute(this.world, 3));
    this.geometry.setAttribute("colour", new THREE.BufferAttribute(new Uint8Array(data.count * 3), 3, true));
    this.geometry.setAttribute("size", new THREE.BufferAttribute(new Float32Array(data.count), 1));
    // Room for every system; each style fills the front and sets the draw range.
    this.geometry.setIndex(new THREE.BufferAttribute(new Uint32Array(data.count), 1));
    this.geometry.setDrawRange(0, 0);
    this.material = new THREE.ShaderMaterial({
      vertexShader,
      fragmentShader,
      uniforms: {
        uPixelRatio: { value: 1 },
        // The map background (view.ts), so rings read as gaps.
        uRing: { value: new THREE.Vector3(3, 4, 10).divideScalar(255) },
      },
      depthTest: false,
      depthWrite: false,
    });
    this.object = new THREE.Points(this.geometry, this.material);
    this.object.frustumCulled = false;
    // Under the route and markers, which share the overlay scene.
    this.object.renderOrder = -1;
    this.object.name = "populated-systems";
    this.object.visible = false;
  }

  /** Recolours and resizes every system, and sets which are drawn in what order. */
  setStyle(style: OverlayStyle): void {
    this.style = style;
    const colour = this.geometry.getAttribute("colour") as THREE.BufferAttribute;
    (colour.array as Uint8Array).set(style.colour);
    colour.needsUpdate = true;
    const size = this.geometry.getAttribute("size") as THREE.BufferAttribute;
    (size.array as Float32Array).set(style.size);
    size.needsUpdate = true;
    const index = this.geometry.getIndex()!;
    (index.array as Uint32Array).set(style.order);
    index.clearUpdateRanges();
    index.addUpdateRange(0, style.order.length);
    index.needsUpdate = true;
    this.geometry.setDrawRange(0, style.order.length);
  }

  update(pixelRatio: number): void {
    this.material.uniforms.uPixelRatio.value = pixelRatio;
  }

  /**
   * The system under (x, y), in CSS pixels from the canvas's top left: the
   * topmost dot covering it, or else the nearest within `radius` pixels.
   */
  pick(camera: THREE.PerspectiveCamera, width: number, height: number, x: number, y: number, radius = 6): number | null {
    const style = this.style;
    if (!this.object.visible || !style || style.order.length === 0) return null;
    const e = this.matrix.multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse).elements;
    const w = this.world;
    let covering = -1;
    let nearest = -1;
    let nearestD = radius * radius;
    for (const i of style.order) {
      const px = w[i * 3];
      const py = w[i * 3 + 1];
      const pz = w[i * 3 + 2];
      const cw = e[3] * px + e[7] * py + e[11] * pz + e[15];
      if (cw <= 0) continue;
      const sx = ((e[0] * px + e[4] * py + e[8] * pz + e[12]) / cw + 1) * 0.5 * width;
      const sy = (1 - (e[1] * px + e[5] * py + e[9] * pz + e[13]) / cw) * 0.5 * height;
      const d = (sx - x) ** 2 + (sy - y) ** 2;
      // For a perspective camera clip w is the view depth, as in the shader.
      const r = (style.size[i] * Math.min(Math.max(SIZE_REF_DISTANCE / cw, MIN_ZOOM), MAX_ZOOM)) / 2;
      // Later dots draw on top, so the last one covering the point wins.
      if (d <= r * r) covering = i;
      else if (d <= nearestD) {
        nearestD = d;
        nearest = i;
      }
    }
    const hit = covering >= 0 ? covering : nearest;
    return hit >= 0 ? hit : null;
  }

  /** Hover text for system i under the current style. */
  describe(i: number): string {
    return this.style?.describe(i) ?? "";
  }
}
