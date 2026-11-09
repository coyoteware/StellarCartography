// Picking and labelling on the CPU, for the overlay layers drawn as points
// (populated systems, points of interest): project each candidate into
// screen space and test it against the cursor. A few thousand to ~150k
// points take a millisecond or two, cheap next to the GPU id pass the star
// tiles need.

import * as THREE from "three";

const matrix = new THREE.Matrix4();

/** Projects world positions to CSS pixels from the canvas's top left, with the view depth. */
export type Projector = (x: number, y: number, z: number) => { sx: number; sy: number; depth: number } | null;

export function projector(camera: THREE.PerspectiveCamera, width: number, height: number): Projector {
  const e = matrix.multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse).elements.slice();
  return (px, py, pz) => {
    // For a perspective camera clip w is the view depth, as in the shaders.
    const cw = e[3] * px + e[7] * py + e[11] * pz + e[15];
    if (cw <= 0) return null;
    return {
      sx: ((e[0] * px + e[4] * py + e[8] * pz + e[12]) / cw + 1) * 0.5 * width,
      sy: (1 - (e[1] * px + e[5] * py + e[9] * pz + e[13]) / cw) * 0.5 * height,
      depth: cw,
    };
  };
}

/**
 * The point under (x, y), in CSS pixels from the canvas's top left: the
 * topmost one whose drawn disc covers it, or else the nearest within
 * `radius` pixels. `order` is the draw order (later points are on top);
 * `world` holds xyz per point; `discRadius` gives a point's drawn radius in
 * pixels at a view depth.
 */
export function pickPoint(
  camera: THREE.PerspectiveCamera,
  width: number,
  height: number,
  world: Float32Array,
  order: ArrayLike<number>,
  discRadius: (i: number, depth: number) => number,
  x: number,
  y: number,
  radius: number,
): number | null {
  const project = projector(camera, width, height);
  let covering = -1;
  let nearest = -1;
  let nearestD = radius * radius;
  for (let k = 0; k < order.length; k++) {
    const i = order[k];
    const s = project(world[i * 3], world[i * 3 + 1], world[i * 3 + 2]);
    if (!s) continue;
    const d = (s.sx - x) ** 2 + (s.sy - y) ** 2;
    const r = discRadius(i, s.depth);
    // Later points draw on top, so the last one covering the cursor wins.
    if (d <= r * r) covering = i;
    else if (d <= nearestD) {
      nearestD = d;
      nearest = i;
    }
  }
  const hit = covering >= 0 ? covering : nearest;
  return hit >= 0 ? hit : null;
}
