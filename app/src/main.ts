import * as THREE from "three";
import {
  buildStatus,
  commanderHistory,
  datasetInfo,
  getSettings,
  getSystem,
  onCommanderJump,
  saveViewSettings,
  visitedSystems,
  type CommanderPayload,
  type DataStatus,
  type Jump,
  type SystemInfo,
  type ViewSettings,
  type VisitedSystem,
} from "./api";
import { CommanderLayer, SelectionMarker } from "./renderer/commander";
import { SGR_A, SOL, edToWorld, worldToEd } from "./renderer/coords";
import { GalaxyGlow } from "./renderer/glow";
import { PointCloud } from "./renderer/pointcloud";
import { View } from "./renderer/view";
import { CatalogueUi } from "./ui/catalogue";
import { Hud } from "./ui/hud";
import { SystemPanel } from "./ui/details";
import { OverlayUi } from "./ui/overlay";
import { Reminders } from "./ui/reminders";
import { setupSearch } from "./ui/search";
import { SettingsPanel } from "./ui/settings";
import { Toasts, errorText } from "./ui/toast";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const view = new View($("viewport"));
const hud = new Hud($("hud"));
const toasts = new Toasts($("toasts"));
const catalogue = new CatalogueUi(toasts);
const commander = new CommanderLayer();
const selection = new SelectionMarker();
view.overlay.add(commander.object, selection.object);

let cloud: PointCloud | null = null;
let glow: GalaxyGlow | null = null;
let commanders: CommanderPayload | null = null;
let selectedFid: string | null = null;
let visitedCache: { total: number; approximate: number } | null = null;
const visitedById = new Map<string, VisitedSystem>();
let viewSettings: ViewSettings = {
  pointBudget: 5_000_000,
  exposure: 1,
  glow: true,
  stars: true,
  route: true,
  overlay: "none",
  highlights: {},
};
let reportedTileFailures = 0;
let updateReminders = true;
/** When the catalogue on show was built (Unix seconds). */
let catalogueBuiltAt: number | null = null;

const systemPanel = new SystemPanel($("info-panel"), {
  commanderPos: () => commander.current()?.pos,
  visited: (id64) => visitedById.get(id64),
  onClose: () => {
    selection.set(null);
    view.wake();
  },
  onError: (message) => toasts.error(message, { timeout: 8 }),
});

const overlay = new OverlayUi({
  toasts,
  view: () => viewSettings,
  onView: (v) => applyView(v),
  onChange: (layer) => {
    if (layer && !layer.object.parent) view.overlay.add(layer.object);
    // Old layers (from before an update) go.
    for (const child of [...view.overlay.children]) {
      if (child.name === "populated-systems" && child !== layer?.object) view.overlay.remove(child);
    }
    applyDimming();
    view.wake();
  },
});

const settingsPanel = new SettingsPanel({
  toasts,
  view: () => viewSettings,
  onView: (v) => applyView(v),
  onRebuild: () => catalogue.rebuild(),
  buildRunning: () => catalogue.running(),
  populatedStatus: () => overlay.statusText(),
  populatedRunning: () => overlay.running,
  onPopulatedUpdate: () => void overlay.startUpdate(),
  onReminders: (on) => (updateReminders = on),
});
overlay.onStatus = () => settingsPanel.renderPopulated();

const reminders = new Reminders({
  toasts,
  enabled: () => updateReminders,
  catalogueBuiltAt: () => catalogueBuiltAt,
  populatedBuiltAt: () => overlay.populated?.builtAt ?? null,
  busy: async () => overlay.running || (await catalogue.running()),
  rebuildCatalogue: () => void catalogue.rebuild(),
  updatePopulated: () => void overlay.startUpdate(),
});
const openSettings = { label: "Settings", run: () => void settingsPanel.open() };

const showGalaxy = () => view.lookAt(edToWorld(SGR_A), 95_000, new THREE.Vector3(0, 0.8, 0.6));
const showSol = () => view.flyTo(edToWorld(SOL), 120);

// --- View settings -----------------------------------------------------------

let saveTimer = 0;

/** Under an overlay layer the stars and glow step back so the coloured dots read. */
function applyDimming(): void {
  const dim = overlay.active;
  if (cloud) cloud.gain = dim ? 0.35 : 1;
  if (glow) glow.brightness = dim ? 0.45 : 1;
}

/** Applies display preferences to the scene and (debounced) saves them. */
function applyView(next: ViewSettings, persist = true): void {
  viewSettings = next;
  view.exposure = next.exposure;
  if (cloud) {
    cloud.pointBudget = next.pointBudget;
    cloud.object.visible = next.stars;
  }
  if (glow) glow.mesh.visible = next.glow;
  commander.object.visible = next.route;
  $<HTMLInputElement>("layer-glow").checked = next.glow;
  $<HTMLInputElement>("layer-stars").checked = next.stars;
  $<HTMLInputElement>("layer-route").checked = next.route;
  overlay.sync();
  applyDimming();
  view.wake();
  if (!persist) return;
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    saveViewSettings(viewSettings).catch((e) => toasts.error(`Couldn't save settings: ${errorText(e)}`));
  }, 400);
}

// --- Galaxy ------------------------------------------------------------------

function select(system: SystemInfo, fly: boolean): void {
  const pos = edToWorld(system.pos);
  selection.set(pos);
  systemPanel.show(system);
  if (fly) view.flyTo(pos, 60);
  view.wake();
}

async function loadGalaxy(status: Extract<DataStatus, { state: "ready" }>): Promise<void> {
  console.info(`galaxy data: ${status.systems} systems from ${status.source} (${status.sourceLastModified}) in ${status.dir}`);
  catalogueBuiltAt = status.builtAt;
  const [c, g] = await Promise.allSettled([PointCloud.load(), GalaxyGlow.load()]);
  if (c.status === "fulfilled") {
    cloud = c.value;
    view.scene.add(cloud.object);
  } else {
    toasts.error(`Couldn't load the star tiles: ${errorText(c.reason)}`, { action: openSettings });
  }
  if (g.status === "fulfilled") {
    glow = g.value;
    view.scene.add(glow.mesh);
  } else {
    toasts.error(`Couldn't load the galaxy backdrop: ${errorText(g.reason)}`, { action: openSettings });
  }
  applyView(viewSettings, false);
  void overlay.load();
  reminders.start();
}

// --- Commander ---------------------------------------------------------------

function setCommander(fid: string): void {
  const track = commanders?.commanders.find((c) => c.fid === fid);
  if (!track) return;
  selectedFid = fid;
  commander.setTrack(track);
  updateLocation();
  view.wake();
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
  for (const s of systems) visitedById.set(s.id64, s);
  updateLocation();
  view.wake();
}

function addCommanderOption(fid: string, name: string, jumps: number): void {
  const opt = document.createElement("option");
  opt.value = fid;
  opt.textContent = `CMDR ${name || fid} (${jumps} jumps)`;
  $<HTMLSelectElement>("commander-select").append(opt);
}

async function loadCommander(): Promise<void> {
  commanders = await commanderHistory();
  const panel = $("commander");
  const picker = $<HTMLSelectElement>("commander-select");
  const location = $("commander-location");
  panel.hidden = false;

  if (!commanders.found) {
    picker.hidden = true;
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = "Set folder…";
    button.addEventListener("click", () => void settingsPanel.open());
    const msg = document.createElement("div");
    msg.className = "problem";
    msg.textContent = "Journal folder not found, so there's no travel history.";
    location.replaceChildren(msg, button);
    return;
  }

  if (commanders.commanders.length === 0) {
    picker.hidden = true;
    location.textContent = "No jumps in the journals yet.";
  } else {
    for (const c of commanders.commanders) addCommanderOption(c.fid, c.name, c.jumps.length);
    picker.value = commanders.latestFid ?? commanders.commanders[0].fid;
    picker.addEventListener("change", () => setCommander(picker.value));
    setCommander(picker.value);
    $("go-cmdr").hidden = false;
  }

  await onCommanderJump((fid: string, jump: Jump) => {
    let track = commanders?.commanders.find((c) => c.fid === fid);
    if (!track) {
      // First jump of a commander the journals hadn't seen yet.
      track = { fid, name: "", jumps: [] };
      commanders?.commanders.push(track);
      addCommanderOption(fid, "", 0);
      picker.hidden = false;
      $("go-cmdr").hidden = false;
      if (!selectedFid) {
        picker.value = fid;
        selectedFid = fid;
      }
    }
    track.jumps.push(jump);
    // The journal now has a fresh snapshot of that system.
    systemPanel.refreshIfShowing(jump.id64);
    if (fid === selectedFid) {
      commander.addJump(jump);
      updateLocation();
      view.wake();
    }
  });
}

// --- UI wiring ---------------------------------------------------------------

setupSearch(
  $("search"),
  $("search-results"),
  (s) => select(s, true),
  (e) => toasts.error(`Search failed: ${errorText(e)}`, { timeout: 8 }),
);
$("go-sol").addEventListener("click", showSol);
$("go-galaxy").addEventListener("click", () => view.flyTo(edToWorld(SGR_A), 95_000, 2));
$("go-cmdr").addEventListener("click", () => {
  const cur = commander.current();
  if (cur) view.flyTo(edToWorld(cur.pos), 80);
});
$("open-settings").addEventListener("click", () => void settingsPanel.open());
for (const layer of ["glow", "stars", "route"] as const) {
  $<HTMLInputElement>(`layer-${layer}`).addEventListener("change", (e) =>
    applyView({ ...viewSettings, [layer]: (e.target as HTMLInputElement).checked }),
  );
}

// Click (not drag) on a star to identify it; hover to see its name.
const canvas = view.renderer.domElement;
const hoverLabel = $("hover-label");
let down: { x: number; y: number } | null = null;
let hoverAt: { x: number; y: number } | null = null;
let hoverTimer = 0;
let hoverRowid = -1;

const canvasPoint = (e: PointerEvent) => {
  const rect = canvas.getBoundingClientRect();
  return { x: e.clientX - rect.left, y: e.clientY - rect.top };
};

function hideHover(): void {
  hoverRowid = -1;
  hoverLabel.hidden = true;
  canvas.style.cursor = "";
}

/** The overlay dot under (x, y), when an overlay layer is drawn. */
function pickOverlay(x: number, y: number, radius: number): number | null {
  if (!overlay.active || !overlay.layer) return null;
  return overlay.layer.pick(view.camera, canvas.clientWidth, canvas.clientHeight, x, y, radius);
}

function hover(): void {
  hoverTimer = 0;
  if (!hoverAt) return;
  const { x, y } = hoverAt;
  // Coloured dots first: they're what the overlay is about.
  const dot = pickOverlay(x, y, 5);
  if (dot !== null && overlay.layer && overlay.populated) {
    hoverRowid = -1;
    canvas.style.cursor = "pointer";
    hoverLabel.style.left = `${x}px`;
    hoverLabel.style.top = `${y}px`;
    const what = overlay.layer.describe(dot);
    hoverLabel.textContent = what ? `${overlay.populated.names[dot]} · ${what}` : overlay.populated.names[dot];
    hoverLabel.hidden = false;
    return;
  }
  if (!cloud) return hideHover();
  const hit = cloud.pick(view.renderer, view.camera, x, y, 5);
  if (!hit) return hideHover();
  canvas.style.cursor = "pointer";
  hoverLabel.style.left = `${x}px`;
  hoverLabel.style.top = `${y}px`;
  if (hit.rowid === hoverRowid) return;
  const rowid = (hoverRowid = hit.rowid);
  void getSystem(rowid).then((system) => {
    if (rowid !== hoverRowid || !system) return;
    hoverLabel.textContent = system.name;
    hoverLabel.hidden = false;
  });
}

canvas.addEventListener("pointermove", (e) => {
  if (e.buttons !== 0) return hideHover();
  hoverAt = canvasPoint(e);
  // Each pick renders the visible points once, so don't do it every event.
  if (!hoverTimer) hoverTimer = window.setTimeout(hover, 80);
});
canvas.addEventListener("pointerleave", () => {
  hoverAt = null;
  hideHover();
});
canvas.addEventListener("pointerdown", (e) => {
  hideHover();
  down = e.button === 0 ? { x: e.clientX, y: e.clientY } : null;
});
canvas.addEventListener("pointerup", async (e) => {
  if (!down || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 4) return;
  const { x, y } = canvasPoint(e);
  const dot = pickOverlay(x, y, 6);
  const dotSystem = dot !== null ? overlay.systemInfo(dot) : null;
  if (dotSystem) return select(dotSystem, false);
  if (!cloud) return;
  const t0 = performance.now();
  const hit = cloud.pick(view.renderer, view.camera, x, y);
  console.debug(`pick took ${(performance.now() - t0).toFixed(1)} ms`);
  if (!hit) return;
  const system = await getSystem(hit.rowid).catch((err) => {
    toasts.error(`Couldn't look up that system: ${errorText(err)}`, { timeout: 8 });
    return null;
  });
  if (system) select(system, false);
});

canvas.addEventListener("webglcontextlost", (e) => {
  e.preventDefault();
  toasts.error("The graphics driver reset and the map stopped drawing.", {
    action: { label: "Reload", run: () => location.reload() },
  });
});

view.onFrame((now, dt) => {
  if (cloud && cloud.object.visible) {
    cloud.update(view.camera, view.height, view.pixelRatio);
    if (cloud.busy) view.wake();
  }
  commander.update(now);
  overlay.layer?.update(view.pixelRatio);
  const stats = cloud?.stats() ?? null;
  if (stats && stats.failed > reportedTileFailures) {
    if (reportedTileFailures === 0) {
      toasts.error("Some star tiles couldn't be loaded, so parts of the map are missing. The data folder may be damaged.", {
        action: openSettings,
      });
    }
    reportedTileFailures = stats.failed;
  }
  hud.update(
    now,
    dt,
    view.idle,
    stats,
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
      catalogue,
      overlay,
      reminders,
      settingsPanel,
      systemPanel,
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

async function boot(): Promise<void> {
  try {
    const settings = await getSettings();
    viewSettings = settings.view;
    updateReminders = settings.updateReminders;
  } catch (e) {
    toasts.error(`Couldn't read the settings: ${errorText(e)}`);
  }
  applyView(viewSettings, false);
  if (!view.hdr) console.warn("No half-float render targets: dense regions will clip instead of tone mapping.");

  const [status, build] = await Promise.all([datasetInfo(), buildStatus()]);
  if (status.state === "ready") {
    void loadGalaxy(status);
    if (build) catalogue.resume(build, true);
  } else if (build) {
    catalogue.resume(build, false);
  } else {
    catalogue.showSetup(status);
  }

  try {
    await loadCommander();
    await loadVisitedCache();
  } catch (e) {
    console.error("commander data:", e);
    toasts.error(`Couldn't read the commander data: ${errorText(e)}`);
  }
}

showGalaxy();
view.start();
void boot();
