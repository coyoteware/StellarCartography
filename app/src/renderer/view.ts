// Scene, camera, controls and the render loop.
//
// The galaxy (glow and stars) is drawn additively into a half-float target so
// dense regions can add up past 1, then tone mapped onto the canvas. Overlays
// (route, markers) are drawn on top afterwards, untouched by tone mapping.
//
// The viewer is meant to run next to the game, so it only renders at full
// rate while something is changing and drops to a trickle when idle.

import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { FullScreenQuad } from "three/addons/postprocessing/Pass.js";

type FrameCallback = (time: number, dt: number) => void;

const BACKGROUND = new THREE.Vector3(3, 4, 10).divideScalar(255);
/** Frame rate while nothing moves (the location marker still pulses). */
const IDLE_FPS = 12;
/** Seconds of full-rate rendering after the last change. */
const WAKE_SECONDS = 1.5;

const toneMapVertex = /* glsl */ `
  varying vec2 vUv;
  void main() {
    vUv = uv;
    gl_Position = vec4(position.xy, 0.0, 1.0);
  }
`;

const toneMapFragment = /* glsl */ `
  uniform sampler2D tHdr;
  uniform float uExposure;
  uniform vec3 uBackground;
  varying vec2 vUv;
  void main() {
    vec3 c = texture2D(tHdr, vUv).rgb * uExposure;
    // Compress by the brightest channel, so dense regions keep their colour
    // instead of clipping to white; faint light passes through almost as is.
    float peak = max(max(c.r, c.g), c.b);
    if (peak > 0.0) c *= (1.0 - exp(-peak)) / peak;
    // Very bright cores still bleach a little, like an overexposed photo.
    float bleach = 0.5 * (1.0 - exp(-max(peak - 3.0, 0.0) * 0.3));
    c = mix(c, vec3(max(max(c.r, c.g), c.b)), bleach);
    gl_FragColor = vec4(uBackground + c, 1.0);
  }
`;

export class View {
  readonly renderer: THREE.WebGLRenderer;
  readonly scene = new THREE.Scene();
  /** Drawn after tone mapping, straight onto the canvas. */
  readonly overlay = new THREE.Scene();
  readonly camera: THREE.PerspectiveCamera;
  readonly controls: OrbitControls;
  /** False when the GPU can't render to half-float targets; brightness then clips at 1. */
  readonly hdr: boolean;
  private readonly target: THREE.WebGLRenderTarget;
  private readonly toneMap: FullScreenQuad;
  private readonly toneMapMaterial: THREE.ShaderMaterial;
  private readonly callbacks: FrameCallback[] = [];
  private awakeUntil = 0;
  private lastFrame = 0;
  private flight: { from: THREE.Vector3; to: THREE.Vector3; fromDist: number; toDist: number; start: number; duration: number } | null = null;

  constructor(readonly container: HTMLElement) {
    this.renderer = new THREE.WebGLRenderer({ antialias: false, powerPreference: "high-performance" });
    this.renderer.setPixelRatio(window.devicePixelRatio);
    container.appendChild(this.renderer.domElement);

    const ext = this.renderer.extensions;
    this.hdr = ext.has("EXT_color_buffer_half_float") || ext.has("EXT_color_buffer_float");
    this.target = new THREE.WebGLRenderTarget(1, 1, {
      type: this.hdr ? THREE.HalfFloatType : THREE.UnsignedByteType,
      depthBuffer: false,
      minFilter: THREE.NearestFilter,
      magFilter: THREE.NearestFilter,
    });
    this.toneMapMaterial = new THREE.ShaderMaterial({
      vertexShader: toneMapVertex,
      fragmentShader: toneMapFragment,
      uniforms: {
        tHdr: { value: this.target.texture },
        uExposure: { value: 1 },
        uBackground: { value: BACKGROUND },
      },
      depthTest: false,
      depthWrite: false,
    });
    this.toneMap = new FullScreenQuad(this.toneMapMaterial);

    this.camera = new THREE.PerspectiveCamera(55, 1, 0.05, 1_000_000);
    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.12;
    this.controls.screenSpacePanning = true;
    this.controls.zoomToCursor = true;
    this.controls.minDistance = 0.2;
    this.controls.maxDistance = 400_000;
    this.controls.zoomSpeed = 1.4;

    this.controls.addEventListener("change", () => this.wake());
    new ResizeObserver(() => this.resize()).observe(container);
    this.resize();
  }

  /** Renders at full rate for a moment: call when anything visible changes. */
  wake(): void {
    this.awakeUntil = performance.now() / 1000 + WAKE_SECONDS;
  }

  /** True while rendering at the idle rate. */
  get idle(): boolean {
    return performance.now() / 1000 > this.awakeUntil;
  }

  get height(): number {
    return this.renderer.domElement.clientHeight;
  }

  get pixelRatio(): number {
    return this.renderer.getPixelRatio();
  }

  get exposure(): number {
    return this.toneMapMaterial.uniforms.uExposure.value;
  }

  set exposure(v: number) {
    this.toneMapMaterial.uniforms.uExposure.value = v;
    this.wake();
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
    this.wake();
  }

  start(): void {
    this.lastFrame = performance.now() / 1000;
    this.wake();
    const loop = () => {
      requestAnimationFrame(loop);
      const now = performance.now() / 1000;
      if (this.flight) this.wake();
      // Small slack so a 60 Hz display doesn't skip every other idle frame.
      if (this.idle && now - this.lastFrame < 1 / IDLE_FPS - 0.004) return;
      const dt = now - this.lastFrame;
      this.lastFrame = now;
      this.stepFlight(now);
      this.controls.update();
      for (const cb of this.callbacks) cb(now, dt);
      this.render();
    };
    requestAnimationFrame(loop);
  }

  private render(): void {
    const r = this.renderer;
    r.setRenderTarget(this.target);
    r.setClearColor(0x000000, 1);
    r.clear();
    r.render(this.scene, this.camera);
    r.setRenderTarget(null);
    this.toneMap.render(r);
    r.autoClear = false;
    r.render(this.overlay, this.camera);
    r.autoClear = true;
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
    const size = this.renderer.getDrawingBufferSize(new THREE.Vector2());
    this.target.setSize(size.x, size.y);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    this.wake();
  }
}
