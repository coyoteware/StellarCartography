// Commander layer: travel route, visited systems and current location (a
// pin over a pulsing ring).

import * as THREE from "three";
import type { CommanderTrack, Jump, Vec3, VisitedSystem } from "../api";
import { edToWorld } from "./coords";
import { PinSet } from "./pins";

function ringTexture(): THREE.Texture {
  const size = 64;
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = size;
  const ctx = canvas.getContext("2d")!;
  ctx.strokeStyle = "white";
  ctx.lineWidth = 6;
  ctx.beginPath();
  ctx.arc(size / 2, size / 2, size / 2 - 5, 0, Math.PI * 2);
  ctx.stroke();
  return new THREE.CanvasTexture(canvas);
}

export const RING = ringTexture();

/** Unlit ring sprites drawn over everything, shared by the overlay layers. */
export function overlayPoints(colour: number, sizePx: number): THREE.Points {
  const material = new THREE.PointsMaterial({
    color: colour,
    size: sizePx,
    sizeAttenuation: false,
    map: RING,
    transparent: true,
    depthTest: false,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
  });
  const points = new THREE.Points(new THREE.BufferGeometry(), material);
  points.frustumCulled = false;
  return points;
}

export function setPositions(geometry: THREE.BufferGeometry, items: { pos: Vec3 }[]): void {
  const arr = new Float32Array(items.length * 3);
  const v = new THREE.Vector3();
  items.forEach((j, i) => edToWorld(j.pos, v).toArray(arr, i * 3));
  geometry.setAttribute("position", new THREE.BufferAttribute(arr, 3));
  geometry.computeBoundingSphere();
}

export class CommanderLayer {
  readonly object = new THREE.Group();
  private jumps: Jump[] = [];
  /** From VisitedStarsCache.dat; when present it replaces the journal-derived set. */
  private cache: VisitedSystem[] | null = null;
  private readonly route: THREE.Line;
  private readonly visited: THREE.Points;
  private readonly marker: THREE.Points;
  /** Your pin; the frame loop sizes it (see PinSet.update). */
  readonly pin = new PinSet("you");

  constructor() {
    this.route = new THREE.Line(
      new THREE.BufferGeometry(),
      new THREE.LineBasicMaterial({
        color: 0xff9a2e,
        transparent: true,
        opacity: 0.55,
        depthTest: false,
        depthWrite: false,
        blending: THREE.AdditiveBlending,
      }),
    );
    this.route.frustumCulled = false;
    this.visited = overlayPoints(0x2c8a62, 4);
    this.marker = overlayPoints(0xff9a2e, 14);
    this.object.add(this.route, this.visited, this.marker, this.pin.object);
    this.object.name = "commander";
  }

  setTrack(track: CommanderTrack): void {
    this.jumps = [...track.jumps];
    this.rebuild();
  }

  addJump(jump: Jump): void {
    this.jumps.push(jump);
    this.rebuild();
  }

  current(): Jump | undefined {
    return this.jumps[this.jumps.length - 1];
  }

  get jumpCount(): number {
    return this.jumps.length;
  }

  setVisitedCache(systems: VisitedSystem[]): void {
    this.cache = systems;
    this.rebuild();
  }

  /** Distinct systems in the track. */
  get visitedCount(): number {
    return new Set(this.jumps.map((j) => j.id64 ?? j.system)).size;
  }

  update(timeSeconds: number): void {
    const material = this.marker.material as THREE.PointsMaterial;
    material.size = 13 + 3 * Math.sin(timeSeconds * 3);
  }

  private rebuild(): void {
    setPositions(this.route.geometry, this.jumps);
    setPositions(this.visited.geometry, this.cache ?? this.jumps);
    const last = this.current();
    setPositions(this.marker.geometry, last ? [last] : []);
    this.pin.set(last ? [{ pos: last.pos }] : []);
  }
}

/** Highlight ring around the system picked or searched for. */
export class SelectionMarker {
  readonly object = overlayPoints(0xffffff, 16);

  set(pos: THREE.Vector3 | null): void {
    const geometry = this.object.geometry;
    if (!pos) {
      geometry.deleteAttribute("position");
      return;
    }
    geometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(pos.toArray()), 3));
    geometry.computeBoundingSphere();
  }
}
