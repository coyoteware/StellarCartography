import "@fontsource/antonio/500.css";
import "@fontsource/saira/400.css";
import "@fontsource/saira/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import * as THREE from "three";
import {
  buildStatus,
  commanderHistory,
  datasetInfo,
  friendPositions,
  getSettings,
  getSystem,
  onCommanderCarrier,
  onCommanderJump,
  saveKeys,
  saveViewSettings,
  searchSystems,
  visitedSystems,
  type Bookmark,
  type Carrier,
  type CarrierPlace,
  type CommanderTrack,
  type CommanderPayload,
  type DataStatus,
  type FriendStatus,
  type Jump,
  type SystemInfo,
  type Vec3,
  type ViewSettings,
  type VisitedSystem,
} from "./api";
import { Bindings, Keyboard, chordLabel, loadLayout } from "./keys";
import type { Poi } from "./poi";
import { CommanderLayer, SelectionMarker } from "./renderer/commander";
import { SGR_A, SOL, edToWorld, worldToEd } from "./renderer/coords";
import { FriendsLayer, type FriendMarker } from "./renderer/friends";
import { GalaxyGlow } from "./renderer/glow";
import { PinSet, stackPins, type PinItem } from "./renderer/pins";
import { PointCloud } from "./renderer/pointcloud";
import type { PoiLayer } from "./renderer/poi";
import { PlotLayer } from "./renderer/route";
import { IntergalacticSky } from "./renderer/sky";
import { View } from "./renderer/view";
import { BOOKMARKS, BOOKMARK_NAMES, Bookmarks } from "./ui/bookmarks";
import { CatalogueUi } from "./ui/catalogue";
import { Hud } from "./ui/hud";
import { SystemPanel } from "./ui/details";
import { KeysCard } from "./ui/keys";
import { OverlayUi } from "./ui/overlay";
import { PlacesUi, describePoi } from "./ui/places";
import { Reminders } from "./ui/reminders";
import { RouteUi } from "./ui/route";
import { setupSearch } from "./ui/search";
import { SettingsPanel } from "./ui/settings";
import { Splash } from "./ui/splash";
import { Toasts, errorText } from "./ui/toast";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// First, so it's reading its quote while everything below is set up.
const splash = new Splash($("splash"));
const view = new View($("viewport"));
const hud = new Hud($("hud"));
const toasts = new Toasts($("toasts"));
const catalogue = new CatalogueUi(toasts);
const bookmarks = new Bookmarks(toasts);
const commander = new CommanderLayer();
const selection = new SelectionMarker();
const plot = new PlotLayer();
const friends = new FriendsLayer($("poi-labels"));
const homePin = new PinSet("home", $("poi-labels"));
const squadronPin = new PinSet("squadron", $("poi-labels"));
const carrierPin = new PinSet<PinItem & { carrier: Carrier; place: CarrierPlace }>("carrier", $("poi-labels"));
view.overlay.add(commander.object, selection.object, plot.object, friends.object, homePin.object, squadronPin.object, carrierPin.object);
// Needs no data, so the sky is there from the first frame, setup included.
const sky = new IntergalacticSky();
view.sky.add(sky.mesh);

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
  plot: true,
  overlay: "none",
  highlights: {},
  poi: true,
  poiHidden: ["planetaryNebula"],
  friends: true,
  sky: true,
};
let reportedTileFailures = 0;
/**
 * Bumped by every selection, and by closing the details: a lookup that
 * comes back after something newer was picked is dropped.
 */
let selectionSeq = 0;
/** The marker layer on show, to notice when an update replaces it. */
let poiLayer: PoiLayer | null = null;
let updateReminders = true;
/** When the catalogue on show was built (Unix seconds). */
let catalogueBuiltAt: number | null = null;
/** Called once the stars of the opening view are in (see `starsDrawn`). */
let onStarsDrawn: (() => void) | null = null;

const systemPanel = new SystemPanel($("info-panel"), {
  commanderPos: () => commander.current()?.pos,
  visited: (id64) => visitedById.get(id64),
  places: (id64, name) => places.inSystem(id64, name),
  onClose: () => {
    selectionSeq++;
    selection.set(null);
    places.highlight(null);
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

const places = new PlacesUi({
  toasts,
  view: () => viewSettings,
  onView: (v) => applyView(v),
  origin: () => {
    const cur = commander.current();
    return cur ? { pos: cur.pos, what: "your location" } : { pos: worldToEd(view.controls.target), what: "the view's centre" };
  },
  onPick: (poi) => void selectPoi(poi, true),
  onChange: (layer) => {
    if (layer !== poiLayer) {
      // A new layer (after an update) knows nothing of the old one's hover.
      poiLayer = layer;
      hideHover();
    }
    if (layer && !layer.object.parent) view.overlay.add(layer.object);
    view.wake();
  },
});

const settingsPanel = new SettingsPanel({
  toasts,
  bookmarks,
  view: () => viewSettings,
  onView: (v) => applyView(v),
  onRebuild: () => catalogue.rebuild(),
  buildRunning: () => catalogue.running(),
  populatedStatus: () => overlay.statusText(),
  populatedRunning: () => overlay.running,
  onPopulatedUpdate: () => void overlay.startUpdate(),
  poiStatus: () => places.statusText(),
  poiRunning: () => places.running,
  onPoiUpdate: () => void places.startUpdate(),
  onReminders: (on) => (updateReminders = on),
  onKeys: () => keysCard.open(),
  friends: () => friendStatuses,
  onFriends: () => void refreshFriends(),
});
overlay.onStatus = () => settingsPanel.renderPopulated();
places.onStatus = () => settingsPanel.renderPoi();
bookmarks.onChange = () => {
  renderBookmarkButtons();
  settingsPanel.renderBookmarks();
  // The home and squadron pins mark whatever systems are behind those buttons.
  for (const [slot, pin] of [["home", homePin], ["squadron", squadronPin]] as const) {
    const saved = bookmarks.get(slot);
    pin.set(saved ? [{ pos: saved.pos, label: saved.name }] : []);
  }
  view.wake();
};

const routeUi = new RouteUi({
  toasts,
  layer: plot,
  fid: () => selectedFid,
  currentSystem: () => commander.current()?.system ?? null,
  onPick: (s) => select(s, true),
  onChange: () => view.wake(),
});

const keyboard = new Keyboard({ modal: () => settingsPanel.shown || keysCard.shown });
const keysCard = new KeysCard({
  keyboard,
  onChange: () => {
    updateHelp();
    saveKeys(keyboard.bindings.saved()).catch((e) => toasts.error(`Couldn't save the keys: ${errorText(e)}`));
  },
});

const reminders = new Reminders({
  toasts,
  enabled: () => updateReminders,
  catalogueBuiltAt: () => catalogueBuiltAt,
  populatedBuiltAt: () => overlay.populated?.builtAt ?? null,
  poiBuiltAt: () => places.points?.builtAt ?? null,
  busy: async () => overlay.running || places.running || (await catalogue.running()),
  rebuildCatalogue: () => void catalogue.rebuild(),
  updatePopulated: () => void overlay.startUpdate(),
  updatePoi: () => void places.startUpdate(),
});
const openSettings = { label: "Settings", run: () => void settingsPanel.open() };

const showGalaxy = () => view.lookAt(edToWorld(SGR_A), 95_000, new THREE.Vector3(0, 0.8, 0.6));
const showSol = () => view.flyTo(edToWorld(SOL), 120);

// --- View settings -----------------------------------------------------------

let saveTimer = 0;

/** Under an overlay layer the stars, glow and sky step back so the coloured dots read. */
function applyDimming(): void {
  const dim = overlay.active;
  if (cloud) cloud.gain = dim ? 0.35 : 1;
  if (glow) glow.brightness = dim ? 0.45 : 1;
  sky.brightness = dim ? 0.45 : 1;
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
  sky.mesh.visible = next.sky;
  // Your carrier is journal data like your route, so it goes with it.
  commander.object.visible = next.route;
  carrierPin.visible = next.route;
  plot.object.visible = next.plot;
  friends.visible = next.friends;
  $<HTMLInputElement>("layer-glow").checked = next.glow;
  $<HTMLInputElement>("layer-stars").checked = next.stars;
  $<HTMLInputElement>("layer-route").checked = next.route;
  $<HTMLInputElement>("layer-plot").checked = next.plot;
  $<HTMLInputElement>("layer-poi").checked = next.poi;
  $<HTMLInputElement>("layer-friends").checked = next.friends;
  $<HTMLInputElement>("layer-sky").checked = next.sky;
  overlay.sync();
  places.sync();
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
  selectionSeq++;
  const pos = edToWorld(system.pos);
  selection.set(pos);
  systemPanel.show(system);
  places.highlight(places.inSystem(system.id64, system.name)[0] ?? null);
  if (fly) view.flyTo(pos, 60);
  view.wake();
}

/** Selects a point of interest's system (or, for one outside any system, just the spot). */
async function selectPoi(poi: Poi, fly: boolean): Promise<void> {
  const mine = ++selectionSeq;
  if (!poi.system) {
    const pos = edToWorld(poi.pos);
    systemPanel.close();
    selection.set(pos);
    places.highlight(poi);
    if (fly) view.flyTo(pos, 60);
    view.wake();
    return;
  }
  let system: SystemInfo = { rowid: null, id64: poi.id64 ?? null, name: poi.system, pos: poi.pos };
  if (!poi.id64) {
    // EDSM's mapping entries name the system but carry no id64: take the
    // catalogue's, so the details lookup can ask Spansh.
    const name = poi.system.toLowerCase();
    const found = await searchSystems(poi.system).catch(() => []);
    // Something else was picked, or the details closed, while it looked.
    if (mine !== selectionSeq) return;
    system = found.find((s) => s.name.toLowerCase() === name) ?? system;
  }
  select(system, fly);
  places.highlight(poi);
}

async function loadGalaxy(status: Extract<DataStatus, { state: "ready" }>): Promise<void> {
  console.info(`galaxy data: ${status.systems} systems from ${status.source} (${status.sourceLastModified}) in ${status.dir}`);
  catalogueBuiltAt = status.builtAt;
  const [c, g] = await Promise.allSettled([PointCloud.load(), GalaxyGlow.load()]);
  if (c.status === "fulfilled") {
    cloud = c.value;
    view.stars.add(cloud.object);
  } else {
    toasts.error(`Couldn't load the star tiles: ${errorText(c.reason)}`, { action: openSettings });
  }
  if (g.status === "fulfilled") {
    glow = g.value;
    view.backdrop.add(glow.mesh);
    sky.setDust(glow.density);
  } else {
    toasts.error(`Couldn't load the galaxy glow: ${errorText(g.reason)}`, { action: openSettings });
  }
  applyView(viewSettings, false);
  const layers = [overlay.load(), places.load()];
  reminders.start();
  // The splash lifts when this returns: by then the opening view is whole.
  await Promise.all([...layers, starsDrawn()]);
}

/** Resolves when the star tiles the current view asks for have loaded (or failed). */
function starsDrawn(): Promise<void> {
  return new Promise((resolve) => (onStarsDrawn = resolve));
}

// --- Commander ---------------------------------------------------------------

function setCommander(fid: string): void {
  const track = commanders?.commanders.find((c) => c.fid === fid);
  if (!track) return;
  selectedFid = fid;
  commander.setTrack(track);
  updateLocation();
  showCarrier();
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
      routeUi.onJump(jump);
      updateLocation();
      view.wake();
    }
  });
  await onCommanderCarrier((fid, carrier) => {
    const track = commanders?.commanders.find((c) => c.fid === fid);
    if (!track) return;
    track.carrier = carrier;
    if (fid === selectedFid) showCarrier();
  });
}

// --- Your carrier ----------------------------------------------------------------

let carrierTimer = 0;

/** Where a carrier is now: at a booked jump's destination once it has departed. */
function carrierNow(c: Carrier): CarrierPlace | null {
  if (c.jump && Date.parse(c.jump.departure) <= Date.now()) return c.jump.to;
  return c.at;
}

/** Pins the selected commander's carrier, and moves it again when a booked jump departs. */
function showCarrier(): void {
  window.clearTimeout(carrierTimer);
  const track = commanders?.commanders.find((t) => t.fid === selectedFid);
  const c = track?.carrier ?? null;
  const place = c ? carrierNow(c) : null;
  carrierPin.set(c && place?.pos ? [{ pos: place.pos, label: c.name ?? c.callsign ?? "Carrier", carrier: c, place }] : []);
  if (track && c?.aboard && place) followCarrier(track, place);
  const wait = c?.jump ? Date.parse(c.jump.departure) - Date.now() : NaN;
  if (wait > 0) carrierTimer = window.setTimeout(showCarrier, Math.min(wait + 1000, 2 ** 31 - 1));
  view.wake();
}

/**
 * Docked aboard your own carrier, you go where it goes. The journal only
 * moves you when you ride it through a jump in game; when it jumps while
 * you're logged off (or the game writes only a CarrierLocation) your last
 * position would be left behind, so the carrier's move counts as yours.
 */
function followCarrier(track: CommanderTrack, place: CarrierPlace): void {
  const last = track.jumps[track.jumps.length - 1];
  if (!place.pos || !last || place.system === last.system || place.timestamp <= last.timestamp) return;
  const jump: Jump = { timestamp: place.timestamp, kind: "CarrierLocation", system: place.system, id64: place.id64, pos: place.pos };
  track.jumps.push(jump);
  commander.addJump(jump);
  updateLocation();
}

/** "GSV Name · ABC-123 · System · jumping to Elsewhere at 18:40" for the hover label. */
function describeCarrier(c: Carrier, place: CarrierPlace): string {
  const parts = [c.name, c.callsign, place.system].filter((s): s is string => !!s);
  const jump = c.jump;
  if (jump && Date.parse(jump.departure) > Date.now()) {
    const at = new Date(jump.departure).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    parts.push(`jumping to ${jump.to.system} at ${at}`);
  }
  return parts.join(" · ");
}

// --- Friends -------------------------------------------------------------------

/** How often friends' EDSM positions are re-fetched. */
const FRIEND_POLL_MS = 5 * 60_000;
/** The latest lookup per friend, for the settings panel's status lines. */
let friendStatuses: FriendStatus[] = [];

/** Fetches every friend's last known position and places the markers. */
async function refreshFriends(refresh = false): Promise<void> {
  try {
    friendStatuses = await friendPositions(refresh);
  } catch (e) {
    console.error("friend positions:", e);
    return;
  }
  const markers: FriendMarker[] = [];
  for (const f of friendStatuses) {
    const p = f.position;
    if (!p?.system) continue;
    let pos = p.pos;
    if (!pos) {
      // EDSM knows the system but not its coordinates: try the catalogue.
      const name = p.system.toLowerCase();
      const found = await searchSystems(p.system).catch(() => []);
      pos = found.find((s) => s.name.toLowerCase() === name)?.pos ?? null;
    }
    if (pos) markers.push({ name: p.name, system: p.system, pos, date: p.date, dockedAt: p.dockedAt, ship: p.ship });
  }
  friends.set(markers);
  settingsPanel.renderFriends();
  view.wake();
}

/** Selects a system by name (its catalogue entry when it has one), e.g. a pin's. */
async function selectNamed(name: string, pos: Vec3, id64: string | null = null): Promise<void> {
  const mine = ++selectionSeq;
  const found = await searchSystems(name).catch(() => []);
  if (mine !== selectionSeq) return;
  const match = found.find((s) => s.name.toLowerCase() === name.toLowerCase());
  select(match ?? { rowid: null, id64, name, pos }, false);
}

const selectFriend = (f: FriendMarker) => selectNamed(f.system, f.pos);

/** "CMDR Name · System · docked at X · seen 2026-11-09 18:00" for the hover label. */
function describeFriend(f: FriendMarker): string {
  const parts = [`CMDR ${f.name}`, f.system];
  if (f.dockedAt) parts.push(`docked at ${f.dockedAt}`);
  if (f.ship) parts.push(f.ship);
  if (f.date) parts.push(`seen ${f.date}`);
  return parts.join(" · ");
}

// --- UI wiring ---------------------------------------------------------------

setupSearch($("search"), $("search-results"), {
  onPickSystem: (s) => select(s, true),
  onPickPoi: (p) => void selectPoi(p, true),
  places: (query, limit) => places.search(query, limit),
  onError: (e) => toasts.error(`Search failed: ${errorText(e)}`, { timeout: 8 }),
});
const goGalaxy = () => view.flyTo(edToWorld(SGR_A), 95_000, 2);
const goCommander = () => {
  const cur = commander.current();
  if (cur) view.flyTo(edToWorld(cur.pos), 80);
  // The button only shows once there is one; the key is always there.
  else toasts.show("There's no commander location yet.", "info", { timeout: 4 });
};
// These sit among thousands of neighbours, so they're selected (marker and
// details) as well as flown to.
const goBookmark = (slot: Bookmark) => {
  const system = bookmarks.system(slot);
  if (system) return select(system, true);
  // The button only shows once there is one; the key is always there.
  toasts.show(`There's no ${BOOKMARK_NAMES[slot]} yet. Choose one in Settings → Jump buttons.`, "info", {
    timeout: 6,
    action: openSettings,
  });
};
const goHome = () => goBookmark("home");
const goSquadron = () => goBookmark("squadron");

/** Shows the jump buttons that have a system behind them, named in their tooltips. */
function renderBookmarkButtons(): void {
  for (const slot of BOOKMARKS) {
    const system = bookmarks.get(slot);
    const button = $(`go-${slot}`);
    button.hidden = !system;
    button.title = system ? `${system.name}, your ${BOOKMARK_NAMES[slot]}` : "";
  }
  $("bookmark-buttons").hidden = BOOKMARKS.every((slot) => !bookmarks.get(slot));
}
$("go-sol").addEventListener("click", showSol);
$("go-galaxy").addEventListener("click", goGalaxy);
$("go-cmdr").addEventListener("click", goCommander);
$("go-home").addEventListener("click", goHome);
$("go-squadron").addEventListener("click", goSquadron);
$("open-settings").addEventListener("click", () => void settingsPanel.open());
for (const layer of ["glow", "stars", "route", "plot", "poi", "friends", "sky"] as const) {
  $<HTMLInputElement>(`layer-${layer}`).addEventListener("change", (e) =>
    applyView({ ...viewSettings, [layer]: (e.target as HTMLInputElement).checked }),
  );
}

// --- Keyboard ----------------------------------------------------------------

const HELP = "Drag to rotate · right-drag to pan · scroll to zoom · click a star or marker for details";

/** The hint in the corner names the key for the key list, whatever it is now. */
function updateHelp(): void {
  const key = keyboard.bindings.get("keys").find(Boolean);
  $("help").textContent = key ? `${HELP} · ${chordLabel(key)} for keys` : HELP;
}

keyboard.on("goSol", showSol);
keyboard.on("goGalaxy", goGalaxy);
keyboard.on("goCommander", goCommander);
keyboard.on("goHome", goHome);
keyboard.on("goSquadron", goSquadron);
for (const [id, layer] of [
  ["toggleGlow", "glow"],
  ["toggleStars", "stars"],
  ["toggleRoute", "route"],
  ["togglePoi", "poi"],
  ["togglePlot", "plot"],
  ["toggleSky", "sky"],
] as const) {
  keyboard.on(id, () => applyView({ ...viewSettings, [layer]: !viewSettings[layer] }));
}
keyboard.on("search", () => {
  const search = $<HTMLInputElement>("search");
  search.focus();
  search.select();
});
keyboard.on("places", () => places.toggleOpen());
keyboard.on("route", () => routeUi.toggleOpen());
// One card at a time: each of these closes the other.
keyboard.on("settings", () => {
  keysCard.close();
  if (settingsPanel.shown) settingsPanel.close();
  else void settingsPanel.open();
});
keyboard.on("keys", () => {
  settingsPanel.close();
  keysCard.toggle();
});
keyboard.onHold = () => view.wake();
view.setDriver(() => ({
  forward: keyboard.axis("moveBack", "moveForward"),
  right: keyboard.axis("moveLeft", "moveRight"),
  up: keyboard.axis("moveDown", "moveUp"),
  orbitRight: keyboard.axis("orbitLeft", "orbitRight"),
  orbitUp: keyboard.axis("orbitDown", "orbitUp"),
  zoomIn: keyboard.axis("zoomOut", "zoomIn"),
}));
updateHelp();
// Keys are named as this keyboard's layout prints them, once the browser says.
void loadLayout().then(() => {
  keysCard.render();
  updateHelp();
});

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
  setPoiHover(null);
}

let poiHover: number | null = null;

function setPoiHover(i: number | null): void {
  if (i === poiHover) return;
  poiHover = i;
  places.layer?.setHover(i);
  view.wake();
}

type PinHit = { kind: "you" | "carrier" | "home" | "squadron" | "friend"; index: number };

/** The pin under (x, y): they draw over everything, so they pick first. */
function pickPin(x: number, y: number, radius: number): PinHit | null {
  const [w, h] = [canvas.clientWidth, canvas.clientHeight];
  const sets = [
    ["you", commander.pin],
    ["carrier", carrierPin],
    ["squadron", squadronPin],
    ["home", homePin],
  ] as const;
  for (const [kind, set] of sets) {
    const index = set.pick(view.camera, w, h, x, y, radius);
    if (index !== null) return { kind, index };
  }
  const index = friends.pick(view.camera, w, h, x, y, radius);
  return index === null ? null : { kind: "friend", index };
}

function describePin(hit: PinHit): string {
  switch (hit.kind) {
    case "you": {
      const cur = commander.current();
      const name = commanders?.commanders.find((c) => c.fid === selectedFid)?.name;
      return [name ? `CMDR ${name}` : "You", cur?.system].filter(Boolean).join(" · ");
    }
    case "carrier": {
      const item = carrierPin.items[hit.index];
      return describeCarrier(item.carrier, item.place);
    }
    case "home":
      return ["Home", bookmarks.get("home")?.name].filter(Boolean).join(" · ");
    case "squadron":
      return ["Squadron capital", bookmarks.get("squadron")?.name].filter(Boolean).join(" · ");
    case "friend":
      return describeFriend(friends.friends[hit.index]);
  }
}

function selectPin(hit: PinHit): void {
  switch (hit.kind) {
    case "you": {
      const cur = commander.current();
      if (cur) void selectNamed(cur.system, cur.pos, cur.id64);
      return;
    }
    case "carrier": {
      const { place } = carrierPin.items[hit.index];
      if (place.pos) void selectNamed(place.system, place.pos, place.id64);
      return;
    }
    case "home":
    case "squadron": {
      const system = bookmarks.system(hit.kind);
      if (system) select(system, false);
      return;
    }
    case "friend":
      return void selectFriend(friends.friends[hit.index]);
  }
}

/** The point-of-interest marker under (x, y), when they're drawn. */
function pickPoi(x: number, y: number, radius: number): Poi | null {
  const layer = places.layer;
  if (!layer || !layer.visible) return null;
  const i = layer.pick(view.camera, canvas.clientWidth, canvas.clientHeight, x, y, radius);
  return i === null ? null : layer.pois[i];
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
  // Pins draw on top of everything, so they pick first.
  const pin = pickPin(x, y, 5);
  if (pin !== null) {
    hoverRowid = -1;
    setPoiHover(null);
    canvas.style.cursor = "pointer";
    hoverLabel.style.left = `${x}px`;
    hoverLabel.style.top = `${y}px`;
    hoverLabel.textContent = describePin(pin);
    hoverLabel.hidden = false;
    return;
  }
  // Markers next, then coloured dots: they're what those layers are about.
  const poi = pickPoi(x, y, 5);
  if (poi && places.layer) {
    hoverRowid = -1;
    setPoiHover(places.layer.indexOf(poi));
    canvas.style.cursor = "pointer";
    hoverLabel.style.left = `${x}px`;
    hoverLabel.style.top = `${y}px`;
    hoverLabel.textContent = `${poi.name} · ${describePoi(poi)}`;
    hoverLabel.hidden = false;
    return;
  }
  setPoiHover(null);
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

// The label describes what was under the cursor before the view moved.
view.controls.addEventListener("change", () => {
  if (!hoverLabel.hidden) hideHover();
});
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
  // Keys go to the map once it's clicked, not to the field or list used last.
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  down = e.button === 0 ? { x: e.clientX, y: e.clientY } : null;
});
canvas.addEventListener("pointerup", async (e) => {
  if (!down || Math.hypot(e.clientX - down.x, e.clientY - down.y) > 4) return;
  const { x, y } = canvasPoint(e);
  const pin = pickPin(x, y, 6);
  if (pin !== null) return selectPin(pin);
  const poi = pickPoi(x, y, 6);
  if (poi) return void selectPoi(poi, false);
  const dot = pickOverlay(x, y, 6);
  const dotSystem = dot !== null ? overlay.systemInfo(dot) : null;
  if (dotSystem) return select(dotSystem, false);
  if (!cloud) return;
  const t0 = performance.now();
  const hit = cloud.pick(view.renderer, view.camera, x, y);
  console.debug(`pick took ${(performance.now() - t0).toFixed(1)} ms`);
  if (!hit) return;
  const mine = ++selectionSeq;
  const system = await getSystem(hit.rowid).catch((err) => {
    toasts.error(`Couldn't look up that system: ${errorText(err)}`, { timeout: 8 });
    return null;
  });
  if (system && mine === selectionSeq) select(system, false);
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
  if (onStarsDrawn && !(cloud?.object.visible && cloud.busy)) {
    onStarsDrawn();
    onStarsDrawn = null;
  }
  commander.update(now);
  plot.update(now);
  overlay.layer?.update(view.pixelRatio);
  places.layer?.update(view.camera, canvas.clientWidth, canvas.clientHeight, view.pixelRatio, !view.idle);
  // You at the bottom of any stack, then your carrier, the bookmarks and friends.
  const pinSets = [commander.pin, carrierPin, squadronPin, homePin, friends.pins];
  stackPins(pinSets, view.camera, canvas.clientWidth, canvas.clientHeight);
  for (const pins of pinSets) pins.update(view.camera, canvas.clientWidth, canvas.clientHeight, !view.idle);
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
      friends,
      refreshFriends,
      catalogue,
      overlay,
      places,
      routeUi,
      reminders,
      settingsPanel,
      systemPanel,
      keyboard,
      keysCard,
      get cloud() {
        return cloud;
      },
      get glow() {
        return glow;
      },
      sky,
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
    keyboard.use(new Bindings(settings.keys ?? {}));
    updateHelp();
    bookmarks.load(settings);
  } catch (e) {
    toasts.error(`Couldn't read the settings: ${errorText(e)}`);
  }
  applyView(viewSettings, false);
  if (!view.hdr) console.warn("No half-float render targets: dense regions will clip instead of tone mapping.");

  const [status, build] = await Promise.all([datasetInfo(), buildStatus()]);
  let galaxy: Promise<void> | null = null;
  if (status.state === "ready") {
    galaxy = loadGalaxy(status);
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
  // After the commander, so the saved route and ship know who's flying.
  void routeUi.load();
  void refreshFriends();
  window.setInterval(() => void refreshFriends(), FRIEND_POLL_MS);
  await galaxy;
}

showGalaxy();
view.start();
// The splash covers all of that loading in; whatever went wrong, the app shows.
void boot().finally(() => splash.done());
