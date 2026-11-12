// Galaxy backdrop: ray-marches a slab around the galactic plane, sampling
// the top-down `density.png` written by `ingest` and spreading it vertically
// with a disc profile. The idea of ray-marching a 2D density map follows
// spansh/elite-galaxy-map; the shader and data here are our own.

import * as THREE from "three";
import { dataUrl } from "../api";
import { SGR_A, edToWorld } from "./coords";

// Must match the octree root / density map extent in galaxy-core.
const MAP_MIN_X = -50000;
const MAP_MIN_Z = -25000;
const MAP_SIZE = 100000;
const HALF_THICKNESS = 4000;
const INTENSITY = 1 / 900;
/** The glow fades in over these distances (ly) from the camera, where stars are resolved points. */
export const GLOW_NEAR_FADE = [30, 1500] as const;

const vertexShader = /* glsl */ `
  varying vec3 vWorld;
  void main() {
    vec4 w = modelMatrix * vec4(position, 1.0);
    vWorld = w.xyz;
    gl_Position = projectionMatrix * viewMatrix * w;
  }
`;

const fragmentShader = /* glsl */ `
  uniform sampler2D uDensity;
  uniform vec3 uBoxMin;
  uniform vec3 uBoxMax;
  uniform vec3 uCore;
  uniform float uIntensity;
  varying vec3 vWorld;

  const int STEPS = 72;
  const vec2 MAP_MIN = vec2(${MAP_MIN_X.toFixed(1)}, ${MAP_MIN_Z.toFixed(1)});
  const float MAP_SIZE = ${MAP_SIZE.toFixed(1)};

  vec2 slab(vec3 ro, vec3 rd) {
    vec3 inv = 1.0 / rd;
    vec3 a = (uBoxMin - ro) * inv;
    vec3 b = (uBoxMax - ro) * inv;
    vec3 lo = min(a, b);
    vec3 hi = max(a, b);
    return vec2(max(max(lo.x, lo.y), lo.z), min(min(hi.x, hi.y), hi.z));
  }

  void main() {
    vec3 ro = cameraPosition;
    vec3 rd = normalize(vWorld - ro);
    vec2 t = slab(ro, rd);
    float t0 = max(t.x, 0.0);
    float t1 = t.y;
    if (t1 <= t0) discard;

    float dt = (t1 - t0) / float(STEPS);
    float jitter = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
    vec3 acc = vec3(0.0);
    for (int i = 0; i < STEPS; i++) {
      float tt = t0 + (float(i) + jitter) * dt;
      vec3 p = ro + rd * tt;
      // World -> game frame: x stays, z flips.
      vec2 uv = (vec2(p.x, -p.z) - MAP_MIN) / MAP_SIZE;
      float d = texture2D(uDensity, uv).r;
      float r = length(vec2(p.x - uCore.x, p.z - uCore.z));
      // Thicker bulge near the core, thin disc further out.
      float h = mix(1600.0, 320.0, smoothstep(0.0, 9000.0, r));
      float profile = exp(-abs(p.y - uCore.y) / h);
      vec3 colour = mix(vec3(1.0, 0.80, 0.55), vec3(0.58, 0.70, 1.0), smoothstep(2500.0, 15000.0, r));
      // Fade out right next to the camera, where stars are resolved points.
      float near = smoothstep(${GLOW_NEAR_FADE[0].toFixed(1)}, ${GLOW_NEAR_FADE[1].toFixed(1)}, tt);
      acc += colour * d * d * profile * near * dt;
    }
    // Linear light; the tone mapping pass in view.ts compresses it.
    gl_FragColor = vec4(acc * uIntensity, 1.0);
  }
`;

export class GalaxyGlow {
  readonly mesh: THREE.Mesh;
  private readonly material: THREE.ShaderMaterial;

  static async load(): Promise<GalaxyGlow> {
    const texture = await new THREE.TextureLoader().loadAsync(dataUrl("density.png"));
    texture.flipY = false; // row 0 of the PNG is minimum z
    texture.colorSpace = THREE.NoColorSpace;
    texture.minFilter = THREE.LinearMipmapLinearFilter;
    texture.magFilter = THREE.LinearFilter;
    texture.needsUpdate = true;
    return new GalaxyGlow(texture);
  }

  private constructor(texture: THREE.Texture) {
    const centre = edToWorld([MAP_MIN_X + MAP_SIZE / 2, 0, MAP_MIN_Z + MAP_SIZE / 2]);
    const half = new THREE.Vector3(MAP_SIZE / 2, HALF_THICKNESS, MAP_SIZE / 2);
    this.material = new THREE.ShaderMaterial({
      vertexShader,
      fragmentShader,
      uniforms: {
        uDensity: { value: texture },
        uBoxMin: { value: centre.clone().sub(half) },
        uBoxMax: { value: centre.clone().add(half) },
        uCore: { value: edToWorld(SGR_A) },
        uIntensity: { value: INTENSITY },
      },
      side: THREE.BackSide,
      transparent: true,
      depthWrite: false,
      depthTest: false,
      blending: THREE.AdditiveBlending,
    });
    this.mesh = new THREE.Mesh(new THREE.BoxGeometry(MAP_SIZE, HALF_THICKNESS * 2, MAP_SIZE), this.material);
    this.mesh.position.copy(centre);
    this.mesh.frustumCulled = false;
    this.mesh.renderOrder = -1;
    this.mesh.name = "galaxy-glow";
  }

  set intensity(v: number) {
    this.material.uniforms.uIntensity.value = v;
  }

  /** Fraction of the normal brightness (1 = as designed); lowered under the overlay layers. */
  set brightness(f: number) {
    this.material.uniforms.uIntensity.value = INTENSITY * f;
  }
}
