import { Vector3 } from "three";
import type { Vec3 } from "../api";

// The game's coordinate frame is left-handed. Negating z maps it into
// three.js's right-handed frame without mirroring the galaxy: seen from
// galactic north, Sagittarius A* is "up" from Sol and Colonia is to the left.

export function edToWorld(p: Vec3, out = new Vector3()): Vector3 {
  return out.set(p[0], p[1], -p[2]);
}

export function worldToEd(v: Vector3): Vec3 {
  return [v.x, v.y, -v.z];
}

export function distance(a: Vec3, b: Vec3): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
}

export const SOL: Vec3 = [0, 0, 0];
export const SGR_A: Vec3 = [25.21875, -20.90625, 25899.96875];
