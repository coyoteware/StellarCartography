import * as THREE from "three";
import {
  commanderHistory,
  datasetInfo,
  getSystem,
  onCommanderJump,
  visitedSystems,
  type CommanderPayload,
  type Jump,
  type SystemInfo,
} from "./api";
import { CommanderLayer, SelectionMarker } from "./renderer/commander";
import { SGR_A, SOL, edToWorld, worldToEd } from "./renderer/coords";
import { GalaxyGlow } from "./renderer/glow";
import { PointCloud } from "./renderer/pointcloud";
import { View } from "./renderer/view";
import { Hud } from "./ui/hud";
import { showSystem } from "./ui/info";
import { setupSearch } from "./ui/search";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const view = new View($("viewport"));
const hud = new Hud($("hud"));
const infoPanel = $("info-panel");
const commander = new CommanderLayer();
const selection = new SelectionMarker();
view.scene.add(commander.object, selection.object);

let cloud: PointCloud | null = null;
let glow: GalaxyGlow | null = null;
let commanders: CommanderPayload | null = null;
let selectedFid: string | null = null;
let visitedCache: { total: number; approximate: number } | null = null;

const showGalaxy = () => view.lookAt(edToWorld(SGR_A), 95_000, new THREE.Vector3(0, 0.8, 0.6));
const showSol = () => view.flyTo(edToWorld(SOL), 120);

function select(system: SystemInfo, fly: boolean): void {
  const pos = edToWorld(system.pos);
  selection.set(pos);
  showSystem(infoPanel, system, commander.current()?.pos);
  if (fly) view.flyTo(pos, 60);
}

function showNotice(html: string): void {
  const notice = $("notice");
  notice.innerHTML = html;
  notice.hidden = false;
}

async function loadGalaxy(): Promise<void> {
  const info = await datasetInfo();
  if (!info) {
    showNotice(
      "No galaxy data found.<br>Build it from the EDSM dump with<br>" +
        "<code>cargo run --release -p ingest -- --out data</code><br>from the repository root, then restart.",
    );
    return;
  }
  console.info(`galaxy data: ${info.systems} systems from ${info.source} (${info.sourceLastModified})`);
  const [c, g] = await Promise.allSettled([PointCloud.load(), GalaxyGlow.load()]);
  if (c.status === "fulfilled") {
    cloud = c.value;
    view.scene.add(cloud.object);
  } else console.error("point cloud:", c.reason);
  if (g.status === "fulfilled") {
    glow = g.value;
    view.scene.add(glow.mesh);
  } else console.error("galaxy glow:", g.reason);
}

function setCommander(fid: string): void {
  const track = commanders?.commanders.find((c) => c.fid === fid);
  if (!track) return;
  selectedFid = fid;
  commander.setTrack(track);
  updateLocation();
}

function updateLocation(): void {
  const cur = commander.current();
  const el = $("commander-location");
  el.innerHTML = "";
  if (!cur) {
    el.textContent = "No jumps recorded";
    return;
  }
  const b = document.createElement("b");
  b.textContent = cur.system;
  el.append("At ", b, ` · ${commander.jumpCount.toLocaleString()} jumps, ${commander.visitedCount.toLocaleString()} systems`);
  if (visitedCache) {
    const note = document.createElement("div");
    note.textContent =
      `Visited-stars cache: ${visitedCache.total.toLocaleString()} systems` +
      (visitedCache.approximate ? ` (${visitedCache.approximate} placed by boxel)` : "");
    el.append(note);
  }
}

async function loadVisitedCache(): Promise<void> {
  const systems = await visitedSystems();
  if (systems.length === 0) return;
  visitedCache = { total: systems.length, approximate: systems.filter((s) => s.approximate).length };
  commander.setVisitedCache(systems);
  updateLocation();
}

async function loadCommander(): Promise<void> {
  commanders = await commanderHistory();
  if (!commanders || commanders.commanders.length === 0) return;
  const select = $<HTMLSelectElement>("commander-select");
  for (const c of commanders.commanders) {
    const opt = document.createElement("option");
    opt.value = c.fid;
    opt.textContent = `CMDR ${c.name || c.fid} (${c.jumps.length} jumps)`;
    select.append(opt);
  }
  select.value = commanders.latestFid ?? commanders.commanders[0].fid;
  select.addEventListener("change", () => setCommander(select.value));
  setCommander(select.value);
  $("commander").hidden = false;
  $("go-cmdr").hidden = false;

  await onCommanderJump((fid: string, jump: Jump) => {
    const track = commanders?.commanders.find((c) => c.fid === fid);
    if (track) track.jumps.push(jump);
    if (fid === selectedFid) {
      commander.addJump(jump);
      updateLocation();
    }
  });
}

// --- UI wiring -------------------------------------------------------------

setupSearch($("search"), $("search-results"), (s) => select(s, true));
$("go-sol").addEventListener("click", showSol);
$("go-galaxy").addEventListener("click", () => view.flyTo(edToWorld(SGR_A), 95_000, 2));
$("go-cmdr").addEventListener("click", () => {
  const cur = commander.current();
  if (cur) view.flyTo(edToWorld(cur.pos), 80);
});
$<HTMLInputElement>("layer-glow").addEventListener("change", (e) => {
  if (glow) glow.mesh.visible = (e.target as HTMLInputElement).checked;
});
$<HTMLInputElement>("layer-stars").addEventListener("change", (e) => {
  if (cloud) cloud.object.visible = (e.target as HTMLInputElement).checked;
});
$<HTMLInputElement>("layer-route").addEventListener("change", (e) => {
  commander.object.visible = (e.target as HTMLInputElement).checked;
});
$<HTMLSelectElement>("budget").addEventListener("change", (e) => {
  if (cloud) cloud.pointBudget = Number((e.target as HTMLSelectElement).value);
});

// Click (not drag) on a star to identify it.
const canvas = view.renderer.domElement;
let down: { x: number; y: number } | null = null;
canvas.addEventListener("pointerdown", (e) => {
  down = e.button === 0 ? { x: e.clientX, y: e.clientY } : null;
});
canvas.addEventListener("pointerup", async (e) => {
  if (!down || !cloud || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 4) return;
  const rect = canvas.getBoundingClientRect();
  const ndc = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
  const t0 = performance.now();
  const hit = cloud.pick(view.camera, ndc, view.height);
  console.debug(`pick took ${(performance.now() - t0).toFixed(1)} ms`);
  if (!hit) return;
  const system = await getSystem(hit.rowid);
  if (system) select(system, false);
});

view.onFrame((now, dt) => {
  if (cloud && cloud.object.visible) cloud.update(view.camera, view.height, view.pixelRatio);
  commander.update(now);
  hud.update(
    now,
    dt,
    cloud?.stats() ?? null,
    cloud?.totalPoints ?? 0,
    worldToEd(view.controls.target),
    view.camera.position.distanceTo(view.controls.target),
  );
});

// Debug handle for driving the app from DevTools in dev builds.
if (import.meta.env.DEV) {
  Object.assign(window, {
    sc: {
      view,
      commander,
      get cloud() {
        return cloud;
      },
      get glow() {
        return glow;
      },
      THREE,
      edToWorld,
    },
  });
}

showGalaxy();
view.start();
void loadGalaxy();
void loadCommander()
  .then(loadVisitedCache)
  .catch((e) => console.error("commander data:", e));
