// Intergalactic sky: what lies beyond the Milky Way.
//
// There are no foreground stars here; the catalogue draws the galaxy's own.
// The sky is distant galaxies, about twenty thousand of them, nearly all
// unresolved smudges. They gather into clusters, groups and the filaments between them,
// the way the large-scale structure looks in projection, over a field that
// thins into voids. Small, faint ones are the most distant and lean red.
// The Local Group is in its true places: Andromeda and Triangulum at
// infinity in their real directions, and the Magellanic Clouds as objects in
// the scene at their real distances, so they shift as the view flies.
//
// Each galaxy is one instanced quad laid on the sky (a tangent plane at its
// direction, so it keeps its orientation however the camera rolls), added
// into the star target as light the glow doesn't stand for (view.ts), so it
// shows as it is: the glow doesn't step back for it, and it keeps its own
// colours. A galaxy smaller than a
// pixel is drawn as a blur of fixed size carrying its total light, so nothing
// shimmers as the view turns; larger ones fade into a resolved profile:
// spirals with a bulge, winding arms, pink star-forming knots and a dust lane
// when seen edge-on, ellipticals with a bright core and a soft halo, and
// clumpy irregulars.
//
// The Milky Way's dust hides what lies behind its disc, as the zone of
// avoidance does from Earth: each galaxy's light is reddened and dimmed by
// the dust column along its sightline, taken from the same density map the
// glow draws.

import * as THREE from "three";
import { SGR_A, edToWorld } from "./coords";

// Must match glow.ts (the density map's extent and slab).
const MAP_MIN_X = -50000;
const MAP_MIN_Z = -25000;
const MAP_SIZE = 100000;
const HALF_THICKNESS = 4000;

/** Same seed, same sky, every run. */
const SEED = 0x5ca1ab1e;
const LY_PER_KPC = 3261.56;
const DEG = Math.PI / 180;
const ARCMIN = DEG / 60;

const ELLIPTICAL = 0;
const SPIRAL = 1;
const IRREGULAR = 2;
type Kind = typeof ELLIPTICAL | typeof SPIRAL | typeof IRREGULAR;

type Rgb = [number, number, number];

interface Galaxy {
  /** Unit direction at infinity, or (when `placed`) a world position in ly. */
  pos: THREE.Vector3;
  placed: boolean;
  /** Major axis: any world vector off the sightline; it's projected onto the sky. */
  axis: THREE.Vector3;
  /** Scale radius: radians on the sky, or ly when placed. */
  scale: number;
  /** Apparent minor/major axis ratio. */
  q: number;
  /** Surface brightness at the centre of the disc (linear HDR units). */
  sb: number;
  kind: Kind;
  disc: Rgb;
  core: Rgb;
  /** Share of the light in the bulge (spirals) or bar (irregulars). */
  bulge: number;
  seed: number;
  /** Spirals: winding, arm contrast, arm count, phase. Irregulars: bar, bar offset, phase, clumpiness. */
  extra: [number, number, number, number];
}

// --- Colours -----------------------------------------------------------------

/** Scaled to unit luminance, so a tint never changes how bright a galaxy is. */
function tint(c: Rgb): Rgb {
  const y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  return [c[0] / y, c[1] / y, c[2] / y];
}

const OLD_STARS: Rgb = [1.0, 0.8, 0.58];
const ELLIPTICAL_HALO: Rgb = [1.0, 0.86, 0.7];
const YOUNG_DISC: Rgb = [0.7, 0.8, 1.0];
const IRREGULAR_DISC: Rgb = [0.66, 0.79, 1.0];
/** Where distant galaxies' light drifts as it's redshifted. */
const REDSHIFTED: Rgb = [1.0, 0.55, 0.38];

function redshift(c: Rgb, z: number): Rgb {
  return tint([c[0] + (REDSHIFTED[0] - c[0]) * z, c[1] + (REDSHIFTED[1] - c[1]) * z, c[2] + (REDSHIFTED[2] - c[2]) * z]);
}

// --- SkyRng geometry ------------------------------------------------------------

/** J2000 equatorial to galactic (Hipparcos). */
const EQ_TO_GAL = [
  [-0.0548755604, -0.873437090, -0.4838350155],
  [0.4941094279, -0.44482963, 0.7469822445],
  [-0.867666149, -0.1980763734, 0.4559837762],
];

/** An equatorial vector (x to RA 0, z to the celestial pole) in world axes. */
function eqToWorld(x: number, y: number, z: number): THREE.Vector3 {
  const [g0, g1, g2] = EQ_TO_GAL.map((r) => r[0] * x + r[1] * y + r[2] * z);
  // Galactic x points at the centre, y at l = 90°, z at the north pole; the
  // game has x = -y, y = z, z = x, and the world flips z (coords.ts).
  return new THREE.Vector3(-g1, g2, -g0);
}

/** Direction and major axis of something at (RA, Dec), its axis at position angle `pa` (north through east), all in degrees. */
function skyPlace(ra: number, dec: number, pa: number): { dir: THREE.Vector3; axis: THREE.Vector3 } {
  const a = ra * DEG;
  const d = dec * DEG;
  const p = pa * DEG;
  const north = [-Math.sin(d) * Math.cos(a), -Math.sin(d) * Math.sin(a), Math.cos(d)];
  const east = [-Math.sin(a), Math.cos(a), 0];
  return {
    dir: eqToWorld(Math.cos(d) * Math.cos(a), Math.cos(d) * Math.sin(a), Math.sin(d)),
    axis: eqToWorld(
      Math.cos(p) * north[0] + Math.sin(p) * east[0],
      Math.cos(p) * north[1] + Math.sin(p) * east[1],
      Math.cos(p) * north[2] + Math.sin(p) * east[2],
    ),
  };
}

// --- Random sky ----------------------------------------------------------------

function mulberry32(seed: number): () => number {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

class SkyRng {
  private readonly rng = mulberry32(SEED);

  random(): number {
    return this.rng();
  }

  range(lo: number, hi: number): number {
    return lo + (hi - lo) * this.rng();
  }

  gauss(): number {
    return Math.sqrt(-2 * Math.log(1 - this.rng())) * Math.cos(2 * Math.PI * this.rng());
  }

  /** Scale radius from N(>a) ∝ a^-slope between lo and hi. */
  powerLaw(lo: number, hi: number, slope: number): number {
    const span = 1 - Math.pow(hi / lo, -slope);
    return lo * Math.pow(1 - this.rng() * span, -1 / slope);
  }

  direction(): THREE.Vector3 {
    const z = 2 * this.rng() - 1;
    const phi = 2 * Math.PI * this.rng();
    const r = Math.sqrt(1 - z * z);
    return new THREE.Vector3(r * Math.cos(phi), z, r * Math.sin(phi));
  }

  /** A direction `dx`, `dy` radians from `d` on the sky, with a random bearing. */
  near(d: THREE.Vector3, dx: number, dy: number): THREE.Vector3 {
    const [e1, e2] = basis(d);
    return d.clone().addScaledVector(e1, dx).addScaledVector(e2, dy).normalize();
  }

  /** A random direction in the sky's tangent plane at `d`. */
  bearing(d: THREE.Vector3): THREE.Vector3 {
    const [e1, e2] = basis(d);
    const t = 2 * Math.PI * this.rng();
    return e1.multiplyScalar(Math.cos(t)).addScaledVector(e2, Math.sin(t));
  }
}

function basis(d: THREE.Vector3): [THREE.Vector3, THREE.Vector3] {
  const helper = Math.abs(d.y) < 0.9 ? new THREE.Vector3(0, 1, 0) : new THREE.Vector3(1, 0, 0);
  const e1 = new THREE.Vector3().crossVectors(helper, d).normalize();
  return [e1, new THREE.Vector3().crossVectors(d, e1)];
}

// Sizes on the sky. The smallest are far below a pixel (about 0.05° at 1080p).
const FIELD_MIN = 0.018 * DEG;
const FIELD_MAX = 0.4 * DEG;

/**
 * One galaxy of `kind` at `dir`, `scale` radians across its scale radius.
 * `depth` (0 near, 1 far) reddens it and is otherwise only colour.
 */
function makeGalaxy(sky: SkyRng, dir: THREE.Vector3, kind: Kind, scale: number, depth: number): Galaxy {
  const z = Math.min(Math.max(depth, 0), 1) ** 1.4 * sky.range(0.45, 1);
  const g: Galaxy = {
    pos: dir,
    placed: false,
    axis: sky.bearing(dir),
    scale,
    q: 1,
    sb: 0,
    kind,
    disc: YOUNG_DISC,
    core: OLD_STARS,
    bulge: 0,
    seed: sky.random(),
    extra: [0, 0, 0, 0],
  };
  // A little scatter in surface brightness, as among real galaxies of a type.
  const scatter = Math.pow(10, 0.12 * sky.gauss());
  if (kind === SPIRAL) {
    // Random inclination; a disc is never thinner than about an eighth.
    const cosI = sky.random();
    g.q = Math.sqrt(cosI * cosI * (1 - 0.13 ** 2) + 0.13 ** 2);
    g.sb = 0.28 * scatter;
    g.bulge = sky.range(0.08, 0.45);
    g.disc = redshift(YOUNG_DISC, z);
    g.core = redshift(OLD_STARS, z);
    const pitch = sky.range(10, 28) * DEG;
    g.extra = [(sky.random() < 0.5 ? -1 : 1) / Math.tan(pitch), sky.range(0.5, 1), sky.random() < 0.8 ? 2 : 3, sky.range(0, 2 * Math.PI)];
  } else if (kind === ELLIPTICAL) {
    g.q = sky.range(0.5, 1);
    g.sb = 0.11 * scatter;
    g.disc = redshift(ELLIPTICAL_HALO, z);
    g.core = redshift(OLD_STARS, z);
  } else {
    g.q = sky.range(0.45, 1);
    g.sb = 0.18 * scatter;
    g.disc = redshift(IRREGULAR_DISC, z);
    g.core = redshift(YOUNG_DISC, z);
    g.bulge = sky.random() < 0.4 ? sky.range(0.2, 0.6) : 0;
    g.extra = [g.bulge, sky.range(-0.4, 0.4), sky.range(0, Math.PI), sky.range(0.6, 1)];
  }
  return g;
}

/** Depth for a field galaxy: the smaller it looks, the further away. */
function fieldDepth(scale: number): number {
  return 1 - Math.log(scale / FIELD_MIN) / Math.log(FIELD_MAX / FIELD_MIN);
}

function pickKind(sky: SkyRng, ellipticals: number): Kind {
  const u = sky.random();
  if (u < ellipticals) return ELLIPTICAL;
  return u < ellipticals + (1 - ellipticals) * 0.82 ? SPIRAL : IRREGULAR;
}

function distantGalaxies(): Galaxy[] {
  const sky = new SkyRng();
  const out: Galaxy[] = [];

  // Superclusters and voids: a lumpy weight over the whole sky that the field
  // follows loosely and the clusters follow closely.
  const lumps = Array.from({ length: 28 }, () => ({
    dir: sky.direction(),
    width: sky.range(12, 30) * DEG,
    weight: sky.range(0.4, 1),
  }));
  const lumpiness = (d: THREE.Vector3) => {
    let w = 0;
    for (const l of lumps) w += l.weight * Math.exp(-((d.angleTo(l.dir) / l.width) ** 2));
    return Math.min(w, 1);
  };
  const lumpyDirection = (floor: number) => {
    for (;;) {
      const d = sky.direction();
      if (sky.random() < floor + (1 - floor) * lumpiness(d)) return d;
    }
  };

  // Clusters: a few dozen, mostly far, each with a giant elliptical at its
  // heart and ellipticals crowding the core.
  const clusters = Array.from({ length: 60 }, () => {
    const depth = 1 - sky.random() ** 1.6 * 0.8;
    return { dir: lumpyDirection(0.15), depth, near: 1 - depth * 0.85 };
  });
  for (const c of clusters) {
    const radius = 1.4 * DEG * c.near;
    out.push(makeGalaxy(sky, c.dir, ELLIPTICAL, 0.08 * DEG * c.near * sky.range(0.7, 1.3), c.depth));
    const members = Math.round(15 + 110 * sky.random() ** 2);
    for (let i = 0; i < members; i++) {
      // Concentrated towards the middle, with a long tail.
      const r = radius * Math.abs(sky.gauss()) * (0.3 + 0.7 * sky.random());
      const t = 2 * Math.PI * sky.random();
      const dir = sky.near(c.dir, r * Math.cos(t), r * Math.sin(t));
      const kind = pickKind(sky, 0.75 - 0.4 * Math.min(r / radius, 1));
      const scale = sky.powerLaw(FIELD_MIN, 0.05 * DEG, 2.2) * Math.max(c.near * 1.6, 0.6);
      out.push(makeGalaxy(sky, dir, kind, scale, c.depth));
    }
  }

  // Filaments: each cluster reaches for its nearest neighbours, and galaxies
  // string out along the arcs, some of them in knots.
  const linked = new Set<string>();
  clusters.forEach((c, i) => {
    const near = clusters
      .map((o, j) => ({ j, angle: c.dir.angleTo(o.dir) }))
      .filter((o) => o.j !== i && o.angle < 32 * DEG)
      .sort((a, b) => a.angle - b.angle)
      .slice(0, 2);
    for (const { j, angle } of near) {
      const key = i < j ? `${i}-${j}` : `${j}-${i}`;
      if (linked.has(key)) continue;
      linked.add(key);
      const other = clusters[j];
      const width = sky.range(0.6, 1.6) * DEG;
      const count = Math.round((angle / DEG) * sky.range(2.5, 5));
      let knot: { dir: THREE.Vector3; left: number } | null = null;
      for (let k = 0; k < count; k++) {
        let dir: THREE.Vector3;
        if (knot && knot.left-- > 0) {
          dir = sky.near(knot.dir, sky.gauss() * 0.25 * DEG, sky.gauss() * 0.25 * DEG);
        } else {
          const along = c.dir.clone().lerp(other.dir, sky.random()).normalize();
          dir = sky.near(along, sky.gauss() * width, sky.gauss() * width);
          knot = sky.random() < 0.12 ? { dir, left: Math.round(sky.range(2, 7)) } : null;
        }
        const depth = (c.depth + other.depth) / 2;
        const scale = sky.powerLaw(FIELD_MIN, 0.06 * DEG, 2.1) * Math.max((1 - depth * 0.85) * 1.6, 0.6);
        out.push(makeGalaxy(sky, dir, pickKind(sky, 0.3), scale, depth));
      }
    }
  });

  // Compact groups scattered through the field.
  for (let i = 0; i < 140; i++) {
    const centre = lumpyDirection(0.3);
    const spread = sky.range(0.1, 0.45) * DEG;
    const scale = sky.powerLaw(FIELD_MIN * 2, FIELD_MAX / 3, 2);
    const depth = fieldDepth(scale);
    const n = Math.round(sky.range(3, 8));
    for (let k = 0; k < n; k++) {
      const dir = sky.near(centre, sky.gauss() * spread, sky.gauss() * spread);
      out.push(makeGalaxy(sky, dir, pickKind(sky, 0.4), scale * sky.range(0.4, 1.2), depth));
    }
  }

  // The field: everything else, thinning into the voids.
  for (let i = 0; i < 12000; i++) {
    const dir = lumpyDirection(0.35);
    const scale = sky.powerLaw(FIELD_MIN, FIELD_MAX, 1.9);
    out.push(makeGalaxy(sky, dir, pickKind(sky, 0.22), scale, fieldDepth(scale)));
  }
  return out;
}

// --- The Local Group -----------------------------------------------------------

function localGroup(): Galaxy[] {
  const sky = new SkyRng();
  const out: Galaxy[] = [];

  // Andromeda (M31): 3.2° by 1° of bright disc, bulge-heavy, inclined 77°.
  const m31 = skyPlace(10.6847, 41.2690, 38);
  out.push({
    pos: m31.dir,
    placed: false,
    axis: m31.axis,
    scale: 0.42 * DEG,
    q: 0.3,
    sb: 0.5,
    kind: SPIRAL,
    disc: tint(YOUNG_DISC),
    core: tint(OLD_STARS),
    bulge: 0.38,
    seed: sky.random(),
    extra: [1 / Math.tan(12 * DEG), 0.75, 2, 1.2],
  });

  // Triangulum (M33): 71' by 42', small bulge, loose flocculent arms.
  const m33 = skyPlace(23.4621, 30.6599, 23);
  out.push({
    pos: m33.dir,
    placed: false,
    axis: m33.axis,
    scale: 13 * ARCMIN,
    q: 0.59,
    sb: 0.32,
    kind: SPIRAL,
    disc: tint(YOUNG_DISC),
    core: tint(OLD_STARS),
    bulge: 0.06,
    seed: sky.random(),
    extra: [1 / Math.tan(24 * DEG), 1, 2, 4.0],
  });

  // The Magellanic Clouds sit inside the map's range, so they're placed in
  // the scene (Sol is the world origin) rather than at infinity.
  const clouds = [
    // LMC: 49.6 kpc, about 10° across from Earth, with its off-centre bar.
    { ra: 80.894, dec: -69.756, pa: 170, kpc: 49.6, scale: 4300, q: 0.82, sb: 0.65, bar: 0.55, offset: 0.35 },
    // SMC: 62.4 kpc, about 5° by 3°.
    { ra: 13.187, dec: -72.829, pa: 45, kpc: 62.4, scale: 2300, q: 0.55, sb: 0.5, bar: 0.3, offset: -0.2 },
  ];
  for (const c of clouds) {
    const place = skyPlace(c.ra, c.dec, c.pa);
    out.push({
      pos: place.dir.multiplyScalar(c.kpc * LY_PER_KPC),
      placed: true,
      axis: place.axis,
      scale: c.scale,
      q: c.q,
      sb: c.sb,
      kind: IRREGULAR,
      disc: tint(IRREGULAR_DISC),
      core: tint([0.95, 0.88, 0.8]),
      bulge: c.bar,
      seed: sky.random(),
      extra: [c.bar, c.offset, 0.4, 1],
    });
  }
  return out;
}

// --- Shaders -----------------------------------------------------------------

const vertexShader = /* glsl */ `
  attribute vec4 aPos;     // direction (w = 0) or world position (w = 1)
  attribute vec3 aAxis;    // major axis, projected onto the sky here
  attribute vec4 aShape;   // scale radius, axis ratio, surface brightness, kind
  attribute vec4 aDisc;    // disc colour, bulge share
  attribute vec4 aCore;    // core colour, seed
  attribute vec4 aExtra;   // per kind (see Galaxy.extra)

  uniform float uPixelAngle;
  uniform float uGain;
  uniform sampler2D uDust;
  uniform float uDustOn;
  uniform vec3 uBoxMin;
  uniform vec3 uBoxMax;
  uniform vec3 uCore;

  varying vec2 vBlob;      // position in sigmas of the unresolved blur
  varying vec2 vDisc;      // position in the galaxy's own plane, in scale radii
  varying vec4 vShape;     // kind, how resolved, seed, one pixel in scale radii
  varying vec4 vDiscColour;
  varying vec3 vCoreColour;
  varying vec4 vExtra;
  varying vec3 vLight;     // dust transmission x gain x fade
  varying vec3 vBright;    // unresolved peak, resolved surface brightness, axis ratio
  varying vec2 vEdge;      // position on the quad, -1 to 1

  const vec2 MAP_MIN = vec2(${MAP_MIN_X.toFixed(1)}, ${MAP_MIN_Z.toFixed(1)});
  const float MAP_SIZE = ${MAP_SIZE.toFixed(1)};
  /** Sigma of the blur every galaxy gets, in pixels. */
  const float BLUR = 1.0;
  /** Dust opacity per unit of (density² x ly). Toward the poles from Sol, about 0.07. */
  const float KAPPA = 0.0012;
  const int DUST_STEPS = 16;

  vec2 slab(vec3 ro, vec3 rd) {
    vec3 inv = 1.0 / rd;
    vec3 a = (uBoxMin - ro) * inv;
    vec3 b = (uBoxMax - ro) * inv;
    vec3 lo = min(a, b);
    vec3 hi = max(a, b);
    return vec2(max(max(lo.x, lo.y), lo.z), min(min(hi.x, hi.y), hi.z));
  }

  // Dust column along the sightline up to tMax. The disc's vertical profile
  // is integrated exactly over each step, so a thin disc crossed face-on in a
  // single step still counts in full.
  float dustColumn(vec3 ro, vec3 rd, float tMax) {
    vec2 t = slab(ro, rd);
    float t0 = max(t.x, 0.0);
    float t1 = min(t.y, tMax);
    if (t1 <= t0) return 0.0;
    float seg = (t1 - t0) / float(DUST_STEPS);
    float sum = 0.0;
    for (int i = 0; i < DUST_STEPS; i++) {
      float a = t0 + float(i) * seg;
      float ya = ro.y + rd.y * a - uCore.y;
      float yb = ya + rd.y * seg;
      bool crosses = ya * yb < 0.0;
      // Sample the map where the step comes closest to the plane.
      float tc = crosses ? a - ya / rd.y : (abs(ya) < abs(yb) ? a : a + seg);
      vec3 p = ro + rd * tc;
      vec2 uv = (vec2(p.x, -p.z) - MAP_MIN) / MAP_SIZE;
      if (any(lessThan(uv, vec2(0.0))) || any(greaterThan(uv, vec2(1.0)))) continue;
      float d = textureLod(uDust, uv, 2.0).r;
      float r = length(vec2(p.x - uCore.x, p.z - uCore.z));
      // Half the stars' scale height (glow.ts): dust hugs the plane.
      float h = mix(800.0, 160.0, smoothstep(0.0, 9000.0, r));
      float ea = exp(-abs(ya) / h);
      float eb = exp(-abs(yb) / h);
      float vertical;
      if (abs(yb - ya) < 0.01 * h) vertical = 0.5 * (ea + eb) * seg;
      else if (crosses) vertical = h / abs(rd.y) * (2.0 - ea - eb);
      else vertical = h / abs(rd.y) * abs(ea - eb);
      sum += d * d * vertical;
    }
    return sum;
  }

  void main() {
    vec3 dir;
    float scale;
    float tMax = 1e9;
    float fade = 1.0;
    if (aPos.w > 0.5) {
      vec3 rel = aPos.xyz - cameraPosition;
      float dist = length(rel);
      dir = rel / dist;
      scale = atan(aShape.x / dist);
      tMax = dist;
      // Fly into a cloud and it thins away rather than filling the sky.
      fade = smoothstep(1.5, 4.0, dist / aShape.x);
    } else {
      dir = aPos.xyz;
      scale = aShape.x;
    }
    vec3 major = normalize(aAxis - dir * dot(dir, aAxis));
    vec3 minor = cross(dir, major);

    float q = aShape.y;
    float kind = aShape.w;
    float aPx = scale / uPixelAngle;
    // Unresolved, the light is as concentrated as the galaxy's inner parts:
    // an elliptical's in its core, a spiral's a little less so.
    float inner = kind < 0.5 ? 0.45 : (kind < 1.5 ? 0.8 : 1.0);
    float sMaj = sqrt(aPx * aPx * inner * inner + BLUR * BLUR);
    float sMin = sqrt(aPx * aPx * inner * inner * q * q + BLUR * BLUR);
    // Quad half-size in pixels: the blur, or seven scale radii once resolved.
    vec2 extent = max(vec2(3.5 * sMaj, 3.5 * sMin), 7.0 * aPx * vec2(1.0, q));
    vec2 px = position.xy * extent;

    vBlob = px / vec2(sMaj, sMin);
    vEdge = position.xy;
    vDisc = vec2(px.x / aPx, px.y / (aPx * q));
    vShape = vec4(kind, smoothstep(1.5, 4.0, aPx), aCore.w, 0.7 / aPx);
    vDiscColour = aDisc;
    vCoreColour = aCore.rgb;
    vExtra = aExtra;
    // Light per pixel: resolved, the surface brightness times the profile;
    // unresolved, the same total spread over the blur.
    float sb = aShape.z;
    vBright = vec3(sb * aPx * aPx * q / (sMaj * sMin), sb, q);

    vec3 trans = vec3(1.0);
    if (uDustOn > 0.5) {
      float tau = KAPPA * dustColumn(cameraPosition, dir, tMax);
      // Redder light gets through more of it.
      trans = exp(-tau * vec3(0.75, 1.0, 1.32));
    }
    vLight = trans * uGain * fade;

    // On a tangent plane at the galaxy's direction, then out to infinity:
    // w = 0 makes the projection ignore where the camera is, and z = 0 keeps
    // it inside the depth range (clipping still drops what's behind).
    vec2 ang = px * uPixelAngle;
    vec3 world = dir + major * ang.x + minor * ang.y;
    gl_Position = projectionMatrix * vec4(mat3(viewMatrix) * world, 0.0);
    gl_Position.z = 0.0;
  }
`;

const fragmentShader = /* glsl */ `
  varying vec2 vBlob;
  varying vec2 vDisc;
  varying vec4 vShape;
  varying vec4 vDiscColour;
  varying vec3 vCoreColour;
  varying vec4 vExtra;
  varying vec3 vLight;
  varying vec3 vBright;
  varying vec2 vEdge;

  const vec3 STAR_FORMING = vec3(1.0, 0.42, 0.62);

  float hash(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
  }

  float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + 1.0), f.x), f.y);
  }

  // Noise that evens out where its cells would be under a few pixels (px is
  // pixels per unit of p), so small galaxies come out soft, not as static.
  float noisePx(vec2 p, float px) {
    return mix(0.5, noise(p), smoothstep(2.0, 6.0, px));
  }

  float fbm(vec2 p, float px) {
    return 0.5 * noisePx(p, px) + 0.3 * noisePx(p * 2.03 + 1.7, px * 0.5) + 0.2 * noisePx(p * 4.01 + 3.1, px * 0.25);
  }

  // Star-forming knots: round blobs scattered about one per two cells,
  // fading out where the cells would be too small to show them.
  float knots(vec2 p, float px) {
    float fade = smoothstep(3.0, 8.0, px);
    if (fade <= 0.0) return 0.0;
    vec2 i = floor(p);
    vec2 f = fract(p);
    float k = 0.0;
    for (int y = -1; y <= 1; y++) {
      for (int x = -1; x <= 1; x++) {
        vec2 cell = i + vec2(float(x), float(y));
        float h = hash(cell);
        if (h < 0.55) continue;
        vec2 d = vec2(float(x), float(y)) + vec2(hash(cell + 17.3), hash(cell + 41.7)) - f;
        float r = 0.07 + 0.1 * hash(cell + 5.1);
        k += exp(-dot(d, d) / (r * r)) * (h - 0.55) / 0.45;
      }
    }
    return k * fade;
  }

  vec2 rotate(vec2 p, float a) {
    float c = cos(a);
    float s = sin(a);
    return vec2(c * p.x - s * p.y, s * p.x + c * p.y);
  }

  // Each profile integrates to 2π over the galaxy's plane, the same as the
  // unresolved blur, so a galaxy keeps its total light as it resolves.

  vec3 spiral(vec2 Q, float q, float soft, float seed, float detail) {
    float r = length(Q);
    float rs = sqrt(r * r + soft * soft);
    vec2 n0 = vec2(seed * 71.0, seed * 37.0);
    float aPx = 0.7 / soft;

    // Logarithmic arms, broken up into clumps, fading in as they resolve.
    float theta = atan(Q.y, Q.x);
    float arm = pow(0.5 + 0.5 * cos(vExtra.z * (theta - vExtra.x * log(rs + 0.05)) + vExtra.w), 3.0);
    float clump = fbm(Q * 3.0 + n0, aPx / 3.0);
    float armed = vExtra.y * detail * smoothstep(0.2, 0.7, r);
    float disc = exp(-rs) * mix(1.0, (0.3 + 2.4 * arm) * (0.55 + 0.9 * clump), armed);
    float hii = knots(Q * 6.0 + n0.yx, aPx / 6.0) * arm * armed;

    // The bulge is rounder than the disc and seen as it is on the sky.
    vec2 S = vec2(Q.x, Q.y * q);
    float bq = mix(q, 1.0, 0.65);
    const float RB = 0.15;
    float rb = length(vec2(S.x, S.y / bq));
    float bulge = exp(-sqrt(rb * rb + soft * soft) / RB) * q / (RB * RB * bq);

    float share = vDiscColour.w;
    vec3 c = vDiscColour.rgb * disc * (1.0 - share) + vCoreColour * bulge * share + STAR_FORMING * hii * exp(-rs) * 1.6;

    // A dust lane across the near side, when the disc is close to edge-on.
    float lane = (1.0 - smoothstep(0.18, 0.5, q)) * exp(-pow((S.y - 0.04) / 0.05, 2.0)) * smoothstep(4.0, 0.8, abs(S.x));
    return c * (1.0 - 0.75 * lane * detail);
  }

  vec3 elliptical(vec2 Q, float soft) {
    float rs = sqrt(dot(Q, Q) + soft * soft);
    // A tight bright core and a wide soft halo.
    float core = 0.62 * exp(-rs / 0.22) / (0.22 * 0.22);
    float halo = 0.38 * exp(-rs / 1.1) / (1.1 * 1.1);
    return mix(vDiscColour.rgb, vCoreColour, exp(-rs / 0.4)) * (core + halo);
  }

  vec3 irregular(vec2 Q, float soft, float seed, float detail) {
    vec2 n0 = vec2(seed * 53.0, seed * 19.0);
    float aPx = 0.7 / soft;
    // A lopsided outline, lumpy light and star-forming knots.
    float warp = fbm(Q * 0.7 + n0, aPx / 0.7) - 0.5;
    float rs = sqrt(dot(Q, Q) + soft * soft) * (1.0 + 0.6 * warp * detail);
    float base = exp(-rs);
    float clump = fbm(Q * 2.4 + n0.yx, aPx / 2.4);
    float lumpy = mix(1.0, (0.25 + 2.6 * clump * clump) / 0.95, vExtra.w * detail);
    float hii = knots(Q * 3.5 + n0 * 1.3, aPx / 3.5) * base * detail;
    // A bar, off-centre in Magellanic irregulars.
    vec2 B = rotate(Q, vExtra.z) - vec2(vExtra.y, 0.0);
    float bar = exp(-pow(abs(B.x) / 0.85, 2.5) - pow(B.y / 0.2, 2.0)) / 0.69;
    float share = vExtra.x;
    return vDiscColour.rgb * base * lumpy * (1.0 - share) + vCoreColour * bar * share + STAR_FORMING * hii * 1.4;
  }

  void main() {
    float kind = vShape.x;
    float detail = vShape.y;
    // Too small to make out: a blur in the galaxy's overall colour.
    vec3 overall = mix(vDiscColour.rgb, vCoreColour, kind < 0.5 ? 0.6 : vDiscColour.w);
    vec3 c = overall * vBright.x * exp(-0.5 * dot(vBlob, vBlob));
    if (detail > 0.0) {
      vec3 shape;
      if (kind < 0.5) shape = elliptical(vDisc, vShape.w);
      else if (kind < 1.5) shape = spiral(vDisc, vBright.z, vShape.w, vShape.z, detail);
      else shape = irregular(vDisc, vShape.w, vShape.z, detail);
      // Taper off before the quad's edge, which a lopsided outline can reach.
      float edge = 1.0 - smoothstep(0.7, 1.0, length(vEdge));
      c = mix(c, shape * vBright.y * edge, detail);
    }
    // Alpha counts the stars' light the glow also stands for (pointcloud.ts);
    // none of this is.
    gl_FragColor = vec4(c * vLight, 0.0);
  }
`;

// --- The layer -----------------------------------------------------------------

export class IntergalacticSky {
  readonly mesh: THREE.Mesh;
  readonly count: number;
  private readonly material: THREE.ShaderMaterial;
  private readonly size = new THREE.Vector2();

  constructor() {
    const galaxies = [...distantGalaxies(), ...localGroup()];
    this.count = galaxies.length;
    const n = galaxies.length;
    const pos = new Float32Array(n * 4);
    const axis = new Float32Array(n * 3);
    const shape = new Float32Array(n * 4);
    const disc = new Float32Array(n * 4);
    const core = new Float32Array(n * 4);
    const extra = new Float32Array(n * 4);
    galaxies.forEach((g, i) => {
      pos.set([g.pos.x, g.pos.y, g.pos.z, g.placed ? 1 : 0], i * 4);
      axis.set([g.axis.x, g.axis.y, g.axis.z], i * 3);
      shape.set([g.scale, g.q, g.sb, g.kind], i * 4);
      disc.set([...g.disc, g.bulge], i * 4);
      core.set([...g.core, g.seed], i * 4);
      extra.set(g.extra, i * 4);
    });

    const quad = new THREE.PlaneGeometry(2, 2);
    const geometry = new THREE.InstancedBufferGeometry();
    geometry.index = quad.index;
    geometry.setAttribute("position", quad.getAttribute("position"));
    geometry.setAttribute("aPos", new THREE.InstancedBufferAttribute(pos, 4));
    geometry.setAttribute("aAxis", new THREE.InstancedBufferAttribute(axis, 3));
    geometry.setAttribute("aShape", new THREE.InstancedBufferAttribute(shape, 4));
    geometry.setAttribute("aDisc", new THREE.InstancedBufferAttribute(disc, 4));
    geometry.setAttribute("aCore", new THREE.InstancedBufferAttribute(core, 4));
    geometry.setAttribute("aExtra", new THREE.InstancedBufferAttribute(extra, 4));
    geometry.instanceCount = n;

    const centre = edToWorld([MAP_MIN_X + MAP_SIZE / 2, 0, MAP_MIN_Z + MAP_SIZE / 2]);
    const half = new THREE.Vector3(MAP_SIZE / 2, HALF_THICKNESS, MAP_SIZE / 2);
    this.material = new THREE.ShaderMaterial({
      vertexShader,
      fragmentShader,
      uniforms: {
        uPixelAngle: { value: 0.001 },
        uGain: { value: 1 },
        uDust: { value: null },
        uDustOn: { value: 0 },
        uBoxMin: { value: centre.clone().sub(half) },
        uBoxMax: { value: centre.clone().add(half) },
        uCore: { value: edToWorld(SGR_A) },
      },
      // Seen from inside the sky, the quads face away.
      side: THREE.DoubleSide,
      transparent: true,
      depthWrite: false,
      depthTest: false,
      // Plain addition, alpha included (and it adds nothing to that).
      blending: THREE.CustomBlending,
      blendEquation: THREE.AddEquation,
      blendSrc: THREE.OneFactor,
      blendDst: THREE.OneFactor,
    });
    this.mesh = new THREE.Mesh(geometry, this.material);
    this.mesh.frustumCulled = false;
    this.mesh.renderOrder = -2;
    this.mesh.name = "intergalactic-sky";
    // Sizes are worked out in pixels, so it needs the pixel's angle on the
    // sky at the size and field of view being drawn.
    this.mesh.onBeforeRender = (renderer, _scene, camera) => {
      const cam = camera as THREE.PerspectiveCamera;
      renderer.getDrawingBufferSize(this.size);
      const span = 2 * Math.tan((cam.fov * DEG) / 2) / cam.zoom;
      this.material.uniforms.uPixelAngle.value = span / Math.max(this.size.y, 1);
    };
  }

  /** The glow's density map, so the Milky Way's dust can hide what's behind it. */
  setDust(texture: THREE.Texture): void {
    this.material.uniforms.uDust.value = texture;
    this.material.uniforms.uDustOn.value = 1;
  }

  /** Fraction of the normal brightness; lowered under the overlay layers. */
  set brightness(f: number) {
    this.material.uniforms.uGain.value = f;
  }
}
