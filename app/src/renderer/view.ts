// Scene, camera, controls and the render loop.

import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";

type FrameCallback = (time: number, dt: number) => void;

export class View {
  readonly renderer: THREE.WebGLRenderer;
  readonly scene = new THREE.Scene();
  readonly camera: THREE.PerspectiveCamera;
  readonly controls: OrbitControls;
  private readonly callbacks: FrameCallback[] = [];
  private flight: { from: THREE.Vector3; to: THREE.Vector3; fromDist: number; toDist: number; start: number; duration: number } | null = null;

  constructor(readonly container: HTMLElement) {
    this.renderer = new THREE.WebGLRenderer({ antialias: false, powerPreference: "high-performance" });
    this.renderer.setPixelRatio(window.devicePixelRatio);
    this.renderer.setClearColor(0x03040a);
    container.appendChild(this.renderer.domElement);

    this.camera = new THREE.PerspectiveCamera(55, 1, 0.05, 1_000_000);
    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.12;
    this.controls.screenSpacePanning = true;
    this.controls.zoomToCursor = true;
    this.controls.minDistance = 0.2;
    this.controls.maxDistance = 400_000;
    this.controls.zoomSpeed = 1.4;

    new ResizeObserver(() => this.resize()).observe(container);
    this.resize();
  }

  get height(): number {
    return this.renderer.domElement.clientHeight;
  }

  get pixelRatio(): number {
    return this.renderer.getPixelRatio();
  }

  onFrame(cb: FrameCallback): void {
    this.callbacks.push(cb);
  }

  /** Puts the camera `distance` ly from `target`, looking along `direction`. */
  lookAt(target: THREE.Vector3, distance: number, direction = new THREE.Vector3(0, 0.75, 0.66)): void {
    this.flight = null;
    this.controls.target.copy(target);
    this.camera.position.copy(target).addScaledVector(direction.normalize(), distance);
    this.controls.update();
  }

  /** Animates the orbit target to `target`, ending `distance` ly away. */
  flyTo(target: THREE.Vector3, distance?: number, duration = 1.4): void {
    const fromDist = this.camera.position.distanceTo(this.controls.target);
    this.flight = {
      from: this.controls.target.clone(),
      to: target.clone(),
      fromDist,
      toDist: distance ?? Math.min(fromDist, 80),
      start: performance.now() / 1000,
      duration,
    };
  }

  start(): void {
    let last = performance.now() / 1000;
    const loop = () => {
      const now = performance.now() / 1000;
      const dt = now - last;
      last = now;
      this.stepFlight(now);
      this.controls.update();
      for (const cb of this.callbacks) cb(now, dt);
      this.renderer.render(this.scene, this.camera);
      requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
  }

  private stepFlight(now: number): void {
    const f = this.flight;
    if (!f) return;
    const t = Math.min((now - f.start) / f.duration, 1);
    const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
    const dir = this.camera.position.clone().sub(this.controls.target).normalize();
    // Log-interpolate the distance so long zooms feel even.
    const dist = Math.exp(THREE.MathUtils.lerp(Math.log(f.fromDist), Math.log(f.toDist), e));
    this.controls.target.lerpVectors(f.from, f.to, e);
    this.camera.position.copy(this.controls.target).addScaledVector(dir, dist);
    if (t >= 1) this.flight = null;
  }

  private resize(): void {
    const { clientWidth: w, clientHeight: h } = this.container;
    if (w === 0 || h === 0) return;
    this.renderer.setSize(w, h);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
  }
}
