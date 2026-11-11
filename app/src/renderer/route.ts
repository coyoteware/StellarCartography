// The plotted route on the map: a line through the waypoints, split where the
// commander is (the flown part dims), ring markers coloured by what the stop
// offers (neutron, fuel), and a pulsing ring on the next waypoint.

import * as THREE from "three";
import type { Waypoint } from "../api";
import { RING, overlayPoints, setPositions } from "./commander";
import { edToWorld } from "./coords";

const LINE = 0x46c8e8;
/** Waypoint ring colours. */
const NEUTRON = new THREE.Color(0x86ecff);
const FUEL = new THREE.Color(0xffb347);
const PLAIN = new THREE.Color(0x3e96ad);

function routeLine(opacity: number): THREE.Line {
  const line = new THREE.Line(
    new THREE.BufferGeometry(),
    new THREE.LineBasicMaterial({
      color: LINE,
      transparent: true,
      opacity,
      depthTest: false,
      depthWrite: false,
      blending: THREE.AdditiveBlending,
    }),
  );
  line.frustumCulled = false;
  return line;
}

export class PlotLayer {
  readonly object = new THREE.Group();
  private waypoints: Waypoint[] = [];
  private progress = 0;
  /** Still to fly; drawn bright. */
  private readonly ahead = routeLine(0.8);
  /** Already flown; drawn faint. */
  private readonly flown = routeLine(0.22);
  private readonly stops: THREE.Points;
  private readonly next = overlayPoints(0x86ecff, 18);

  constructor() {
    this.stops = new THREE.Points(
      new THREE.BufferGeometry(),
      new THREE.PointsMaterial({
        size: 7,
        sizeAttenuation: false,
        map: RING,
        vertexColors: true,
        transparent: true,
        depthTest: false,
        depthWrite: false,
        blending: THREE.AdditiveBlending,
      }),
    );
    this.stops.frustumCulled = false;
    this.object.add(this.flown, this.ahead, this.stops, this.next);
    this.object.name = "plotted-route";
  }

  /** Shows a route (or clears it with `[]`) with the commander at waypoint `progress`. */
  setRoute(waypoints: Waypoint[], progress: number): void {
    this.waypoints = waypoints;
    const positions = new Float32Array(waypoints.length * 3);
    const colours = new Float32Array(waypoints.length * 3);
    const v = new THREE.Vector3();
    waypoints.forEach((w, i) => {
      edToWorld(w.pos, v).toArray(positions, i * 3);
      const c = w.neutron ? NEUTRON : w.refuel || w.restock != null ? FUEL : PLAIN;
      c.toArray(colours, i * 3);
    });
    this.stops.geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    this.stops.geometry.setAttribute("color", new THREE.BufferAttribute(colours, 3));
    this.stops.geometry.computeBoundingSphere();
    this.setProgress(progress);
  }

  /** Splits the line at waypoint `i`: up to it is flown, from it is ahead. */
  setProgress(i: number): void {
    this.progress = Math.max(0, Math.min(i, this.waypoints.length - 1));
    setPositions(this.flown.geometry, this.waypoints.slice(0, this.progress + 1));
    setPositions(this.ahead.geometry, this.waypoints.slice(this.progress));
    const next = this.waypoints[this.progress + 1];
    setPositions(this.next.geometry, next ? [next] : []);
  }

  /** Pulses the next-waypoint marker (like the commander's). */
  update(timeSeconds: number): void {
    const material = this.next.material as THREE.PointsMaterial;
    material.size = 15 + 5 * Math.sin(timeSeconds * 3 + 1.5);
  }
}
