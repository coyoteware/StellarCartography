// Scene, camera, controls and the render loop.
//
// The galaxy is drawn into half-float targets so dense regions can add up
// past 1, then tone mapped onto the canvas. Overlays (route, markers) are
// drawn on top afterwards, untouched by tone mapping.
//
// Stars and glow go into separate targets. The glow stands for every system
// in the galaxy, the stars only for the catalogued ones, so adding them as
// they are counted the well-explored regions twice: around the bubble that
// doubled the brightness, tinted it orange, and left a hard edge where the
// catalogue thins out. Instead the tone map pass:
// - averages the stars' shown brightness over a patch of screen (a coarse
//   mip of starLumaTarget) and turns the glow down by that much, as far as
//   the stars actually cover the screen, so the glow only fills in what the
//   drawn stars don't already give;
// - dims those stars where they'd outshine the glow (a complete route seen
//   end on), since the glow is the better count of what's there;
// - gives that light the glow's colour, so the two blend.
// Only stars far enough away for the glow to stand for them count (the star
// target's alpha, see pointcloud.ts). It's done on screen because that's
// where the stars' light ends up: how much of it lands where depends on the
// level of detail, the point budget and the distance.
//
// The viewer is meant to run next to the game, so it only renders at full
// rate while something is changing and drops to a trickle when idle.

import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { FullScreenQuad } from "three/addons/postprocessing/Pass.js";

type FrameCallback = (time: number, dt: number) => void;

/**
 * Keyboard flight for one frame. Each axis is -1, 0 or 1: moves are along the
 * galactic plane relative to where the camera faces (up is galactic north),
 * orbits circle the target, zoom closes on it.
 */
export interface Drive {
  forward: number;
  right: number;
  up: number;
  orbitRight: number;
  orbitUp: number;
  zoomIn: number;
}

/** Keyboard speeds: moves in view distances, orbits in radians, zoom in e-folds, all per second. */
const MOVE_SPEED = 1.2;
const ORBIT_SPEED = 1.6;
const ZOOM_SPEED = 1.6;
const WORLD_UP = new THREE.Vector3(0, 1, 0);

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

/** Width, in CSS pixels, over which the stars' brightness is averaged before the glow gives way to it. */
const COMPLEMENT_BLUR_PX = 24;
/** The least share of the glow's shown brightness that stays, however well the stars cover it. */
const GLOW_FLOOR = 0.15;

const toneMapCommon = /* glsl */ `
  const vec3 LUMA = vec3(0.2126, 0.7152, 0.0722);

  // Compress by the brightest channel, so dense regions keep their colour
  // instead of clipping to white; faint light passes through almost as is.
  vec3 compress(vec3 c) {
    float peak = max(max(c.r, c.g), c.b);
    return peak > 0.0 ? c * (1.0 - exp(-peak)) / peak : c;
  }
`;

// Per pixel, for the tone map pass below to read averaged over an area (from
// the mips of the target this writes), counting only the share of the stars'
// light that the glow also stands for (the star target's alpha, see
// pointcloud.ts):
// - r: those stars' brightness as it will be shown. Averaging what's shown
//   rather than the light matters: the stars' peaks lose more to compression
//   than an even glow of the same light would, so matching light for light
//   left the explored regions looking darker than the glow around them.
// - g: how much of the glow's shown brightness they match here, up to all of
//   it. The drawn stars are points with gaps between them, and the glow can
//   only step back as far as they fill the screen: where it went regardless,
//   dense regions looked sooty.
const starLumaFragment = /* glsl */ `
  ${toneMapCommon}
  uniform sampler2D tStars;
  uniform sampler2D tGlow;
  uniform float uExposure;
  varying vec2 vUv;
  void main() {
    vec4 s = texture2D(tStars, vUv);
    float light = dot(s.rgb, LUMA);
    float share = light > 0.0 ? min(s.a / light, 1.0) : 0.0;
    float stars = dot(compress(s.rgb * uExposure), LUMA) * share;
    float glow = dot(compress(texture2D(tGlow, vUv).rgb * uExposure), LUMA);
    gl_FragColor = vec4(stars, glow > 0.0 ? min(stars / glow, 1.0) : 1.0, 0.0, 1.0);
  }
`;

const toneMapFragment = /* glsl */ `
  ${toneMapCommon}
  uniform sampler2D tStars;
  uniform sampler2D tStarLuma;
  uniform sampler2D tGlow;
  uniform float uExposure;
  uniform vec3 uBackground;
  uniform float uComplement;
  uniform float uFloor;
  uniform float uBlurLod;
  uniform vec2 uTexel;
  varying vec2 vUv;

  // The stars' shown brightness and cover (see starLumaFragment) averaged
  // around this pixel: a 3x3 tent over a coarse mip.
  vec2 localStars() {
    vec2 step = uTexel * exp2(uBlurLod);
    vec2 sum = vec2(0.0);
    for (int j = -1; j <= 1; j++) {
      for (int i = -1; i <= 1; i++) {
        float w = (i == 0 ? 2.0 : 1.0) * (j == 0 ? 2.0 : 1.0);
        sum += w * textureLod(tStarLuma, vUv + vec2(i, j) * step, uBlurLod).rg;
      }
    }
    return sum / 16.0;
  }

  // Splits the glow's shown brightness between the glow and the stars that
  // stand in for it, so that on average the two together look as bright as
  // the glow alone. Returns how much of the glow to keep (solving
  // luma(compress(keep * glow)) = target) and how far to scale those stars.
  // The glow steps back as far as the stars cover the screen; where they'd
  // give more than the room it leaves, as along a well-travelled route seen
  // end on, they're dimmed to fit, since the glow counts every system there
  // and they're only some of them.
  vec2 balance(vec3 glow, vec2 stars) {
    float g = dot(glow, LUMA);
    float peak = max(max(glow.r, glow.g), glow.b);
    if (g <= 0.0 || uComplement <= 0.0) return vec2(1.0);
    float shown = g * (1.0 - exp(-peak)) / peak;
    float target = max(shown - stars.x, shown * max(1.0 - stars.y, uFloor));
    float keep = -log(1.0 - min(target * peak / g, 0.9999)) / peak;
    float room = shown - target;
    return vec2(keep, stars.x > room ? room / stars.x : 1.0);
  }

  void main() {
    vec4 s = texture2D(tStars, vUv);
    vec3 stars = s.rgb * uExposure;
    vec3 glow = texture2D(tGlow, vUv).rgb * uExposure;
    vec2 fit = balance(glow, localStars() * uComplement);
    // The light the glow also stands for takes on the glow's colour here, at
    // the same brightness. Otherwise, where the stars stand in for the glow,
    // they'd show as a warm patch on a blue disc: most catalogued systems are
    // cool dwarfs, while the glow is coloured like a galaxy photograph.
    float light = dot(s.rgb, LUMA);
    float g = dot(glow, LUMA);
    if (light > 0.0 && g > 0.0) {
      float share = min(s.a / light, 1.0) * uComplement;
      stars = mix(stars, glow * (dot(stars, LUMA) / g), share) * mix(1.0, fit.y, share);
    }
    vec3 c = stars + glow * fit.x;
    float peak = max(max(c.r, c.g), c.b);
    c = compress(c);
    // Very bright cores still bleach a little, like an overexposed photo.
    float bleach = 0.5 * (1.0 - exp(-max(peak - 3.0, 0.0) * 0.3));
    c = mix(c, vec3(max(max(c.r, c.g), c.b)), bleach);
    gl_FragColor = vec4(uBackground + c, 1.0);
  }
`;

export class View {
  readonly renderer: THREE.WebGLRenderer;
  /** The catalogued stars. */
  readonly stars = new THREE.Scene();
  /** The galaxy glow, which gives way where the stars are already bright. */
  readonly backdrop = new THREE.Scene();
  /** Drawn after tone mapping, straight onto the canvas. */
  readonly overlay = new THREE.Scene();
  readonly camera: THREE.PerspectiveCamera;
  readonly controls: OrbitControls;
  /** False when the GPU can't render to half-float targets; brightness then clips at 1. */
  readonly hdr: boolean;
  private readonly starTarget: THREE.WebGLRenderTarget;
  /** The stars' shown brightness, mipmapped so the tone map pass can read it averaged over an area. */
  private readonly starLumaTarget: THREE.WebGLRenderTarget;
  private readonly glowTarget: THREE.WebGLRenderTarget;
  private readonly starLuma: FullScreenQuad;
  private readonly starLumaMaterial: THREE.ShaderMaterial;
  private readonly toneMap: FullScreenQuad;
  private readonly toneMapMaterial: THREE.ShaderMaterial;
  private readonly callbacks: FrameCallback[] = [];
  private awakeUntil = 0;
  private lastFrame = 0;
  private flight: { from: THREE.Vector3; to: THREE.Vector3; fromDist: number; toDist: number; start: number; duration: number } | null = null;
  private driver: (() => Drive) | null = null;

  constructor(readonly container: HTMLElement) {
    this.renderer = new THREE.WebGLRenderer({ antialias: false, powerPreference: "high-performance" });
    this.renderer.setPixelRatio(window.devicePixelRatio);
    container.appendChild(this.renderer.domElement);

    const ext = this.renderer.extensions;
    this.hdr = ext.has("EXT_color_buffer_half_float") || ext.has("EXT_color_buffer_float");
    const type = this.hdr ? THREE.HalfFloatType : THREE.UnsignedByteType;
    const flat = { type, depthBuffer: false, minFilter: THREE.NearestFilter, magFilter: THREE.NearestFilter };
    this.starTarget = new THREE.WebGLRenderTarget(1, 1, flat);
    this.glowTarget = new THREE.WebGLRenderTarget(1, 1, flat);
    this.starLumaTarget = new THREE.WebGLRenderTarget(1, 1, {
      type,
      format: THREE.RGFormat,
      depthBuffer: false,
      generateMipmaps: true,
      minFilter: THREE.LinearMipmapLinearFilter,
      magFilter: THREE.LinearFilter,
    });
    const exposure = { value: 1 };
    this.starLumaMaterial = new THREE.ShaderMaterial({
      vertexShader: toneMapVertex,
      fragmentShader: starLumaFragment,
      uniforms: { tStars: { value: this.starTarget.texture }, tGlow: { value: this.glowTarget.texture }, uExposure: exposure },
      depthTest: false,
      depthWrite: false,
    });
    this.starLuma = new FullScreenQuad(this.starLumaMaterial);
    this.toneMapMaterial = new THREE.ShaderMaterial({
      vertexShader: toneMapVertex,
      fragmentShader: toneMapFragment,
      uniforms: {
        tStars: { value: this.starTarget.texture },
        tStarLuma: { value: this.starLumaTarget.texture },
        tGlow: { value: this.glowTarget.texture },
        uExposure: exposure,
        uBackground: { value: BACKGROUND },
        uComplement: { value: 1 },
        uFloor: { value: GLOW_FLOOR },
        uBlurLod: { value: 5 },
        uTexel: { value: new THREE.Vector2(1, 1) },
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

  /**
   * How far the glow gives way to the stars' light: 1 (the default) fills in
   * only what the stars don't already give, 0 adds the two as they are and
   * leaves the stars their own colours (as before). For comparing from DevTools.
   */
  get complement(): number {
    return this.toneMapMaterial.uniforms.uComplement.value;
  }

  set complement(v: number) {
    this.toneMapMaterial.uniforms.uComplement.value = v;
    this.wake();
  }

  onFrame(cb: FrameCallback): void {
    this.callbacks.push(cb);
  }

  /** Where keyboard flight comes from; read every frame before the controls update. */
  setDriver(driver: () => Drive): void {
    this.driver = driver;
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
      this.stepDrive(dt);
      this.controls.update();
      for (const cb of this.callbacks) cb(now, dt);
      this.render();
    };
    requestAnimationFrame(loop);
  }

  private render(): void {
    const r = this.renderer;
    r.setClearColor(0x000000, 1);
    r.setRenderTarget(this.glowTarget);
    r.clear();
    r.render(this.backdrop, this.camera);
    // Alpha accumulates the stars' shared light (pointcloud.ts), so it starts at 0.
    r.setClearColor(0x000000, 0);
    r.setRenderTarget(this.starTarget);
    r.clear();
    r.render(this.stars, this.camera);
    r.setClearColor(0x000000, 1);
    // Rendering into a mipmapped target rebuilds its mips afterwards.
    r.setRenderTarget(this.starLumaTarget);
    this.starLuma.render(r);
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

  private stepDrive(dt: number): void {
    const d = this.driver?.();
    if (!d || !(d.forward || d.right || d.up || d.orbitRight || d.orbitUp || d.zoomIn)) return;
    // Taking the controls cancels a flight, as a drag does.
    this.flight = null;
    // The first frame after idling can be a long one.
    dt = Math.min(dt, 0.1);
    const target = this.controls.target;
    const offset = this.camera.position.clone().sub(target);

    // Ahead is the camera's heading flattened onto the plane, so W goes the
    // same way however steeply the camera looks down.
    const ahead = new THREE.Vector3(-offset.x, 0, -offset.z).normalize();
    const right = new THREE.Vector3(-ahead.z, 0, ahead.x);
    const move = new THREE.Vector3().addScaledVector(ahead, d.forward).addScaledVector(right, d.right).addScaledVector(WORLD_UP, d.up);
    if (move.lengthSq() > 0) target.add(move.setLength(MOVE_SPEED * offset.length() * dt));

    const s = new THREE.Spherical().setFromVector3(offset);
    s.theta += d.orbitRight * ORBIT_SPEED * dt;
    s.phi = THREE.MathUtils.clamp(s.phi - d.orbitUp * ORBIT_SPEED * dt, 0.01, Math.PI - 0.01);
    s.radius = THREE.MathUtils.clamp(
      s.radius * Math.exp(-d.zoomIn * ZOOM_SPEED * dt),
      this.controls.minDistance,
      this.controls.maxDistance,
    );
    this.camera.position.copy(target).add(offset.setFromSpherical(s));
    this.wake();
  }

  private resize(): void {
    const { clientWidth: w, clientHeight: h } = this.container;
    if (w === 0 || h === 0) return;
    this.renderer.setSize(w, h);
    const size = this.renderer.getDrawingBufferSize(new THREE.Vector2());
    this.starTarget.setSize(size.x, size.y);
    this.starLumaTarget.setSize(size.x, size.y);
    this.glowTarget.setSize(size.x, size.y);
    const u = this.toneMapMaterial.uniforms;
    u.uTexel.value.set(1 / size.x, 1 / size.y);
    u.uBlurLod.value = Math.log2(COMPLEMENT_BLUR_PX * this.renderer.getPixelRatio());
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    this.wake();
  }
}
