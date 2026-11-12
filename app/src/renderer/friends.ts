// Friends layer: other commanders' last known positions (from EDSM), drawn
// as ice-blue pins (renderer/pins.ts) with always-on name labels.

import * as THREE from "three";
import type { Vec3 } from "../api";
import { PinSet } from "./pins";

/** A friend placed on the map (see main.ts for how positions resolve). */
export interface FriendMarker {
  name: string;
  system: string;
  pos: Vec3;
  /** "YYYY-MM-DD HH:MM:SS" UTC, when their EDSM profile shares it. */
  date: string | null;
  /** The station they were docked at, when docked. */
  dockedAt: string | null;
  ship: string | null;
}

export class FriendsLayer {
  private readonly pins: PinSet<FriendMarker & { label: string }>;

  constructor(labelRoot: HTMLElement) {
    this.pins = new PinSet("friend", labelRoot);
    this.pins.object.name = "friends";
  }

  get object(): THREE.Object3D {
    return this.pins.object;
  }

  get friends(): FriendMarker[] {
    return this.pins.items;
  }

  set(friends: FriendMarker[]): void {
    this.pins.set(friends.map((f) => ({ ...f, label: f.name })));
  }

  get visible(): boolean {
    return this.pins.visible;
  }

  set visible(on: boolean) {
    this.pins.visible = on;
  }

  /** The friend whose pin is under (x, y) in CSS pixels from the canvas's top left, or whose star is within `radius`. */
  pick(camera: THREE.PerspectiveCamera, width: number, height: number, x: number, y: number, radius = 6): number | null {
    return this.pins.pick(camera, width, height, x, y, radius);
  }

  /** Per frame: sizes the pins and keeps the name labels beside them. */
  update(camera: THREE.PerspectiveCamera, width: number, height: number, moving: boolean): void {
    this.pins.update(camera, width, height, moving);
  }
}
