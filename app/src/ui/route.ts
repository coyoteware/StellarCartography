// The route planner: a panel under the search box (collapsed to its header
// until opened) that asks Spansh to plot a route — the neutron plotter, the
// fuel-exact galaxy plotter (using the ship from the journals), or the
// fleet-carrier router — then draws it on the map, lists the waypoints, and
// follows the commander's jumps along it, copying the next system to the
// clipboard for the in-game galaxy map.

import {
  cancelPlot,
  plotRoute,
  saveRoute,
  savedRoute,
  searchSystems,
  shipLoadout,
  type Jump,
  type PlottedRoute,
  type RouteMode,
  type RouteRequest,
  type SavedRoute,
  type ShipSpec,
  type SystemInfo,
  type Waypoint,
} from "../api";
import type { PlotLayer } from "../renderer/route";
import { button, h, type Child } from "./dom";
import { errorText, type Toasts } from "./toast";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

/** Waypoint rows drawn at once, centred on the commander's progress. */
const MAX_ROWS = 150;

const MODES: { id: RouteMode; label: string; hint: string }[] = [
  { id: "neutron", label: "Neutron", hint: "Fast long-range legs between neutron stars (Spansh's neutron plotter)." },
  { id: "galaxy", label: "Ship", hint: "Every jump, fuel-exact, for the ship in your journals (Spansh's galaxy plotter)." },
  { id: "carrier", label: "Carrier", hint: "500 ly fleet-carrier hops with tritium stops (Spansh's carrier router)." },
];

const fmtLy = (d: number) => `${d < 10 ? d.toFixed(1) : Math.round(d).toLocaleString()} ly`;

/** "Corsair", from the journal's internal name ("corsair", "python_nx"). */
function shipLabel(ship: ShipSpec): string {
  const type = ship.ship.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
  const name = ship.shipName.trim();
  return name ? `${name} (${type} ${ship.shipIdent})` : `${type} ${ship.shipIdent}`;
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // The webview may keep the async API behind a permission; the old way works.
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
}

export interface RouteDeps {
  toasts: Toasts;
  layer: PlotLayer;
  /** The selected commander's FID, for the ship loadout. */
  fid: () => string | null;
  /** The commander's current system, if the journals name one. */
  currentSystem: () => string | null;
  onPick: (system: SystemInfo) => void;
  /** The layer changed: wake the renderer. */
  onChange: () => void;
}

export class RouteUi {
  private saved: SavedRoute | null = null;
  private mode: RouteMode = "neutron";
  private ship: ShipSpec | null = null;
  private plotting = false;
  private plotStarted = 0;
  private plotTimer = 0;
  private open = false;

  private readonly panel = $("route-panel");
  private readonly toggle = button("", () => this.toggleOpen(), "places-toggle");
  private readonly total = h("span", "count");
  private readonly body = h("div", "route-body");
  private readonly modes = h("div", "segs route-modes");
  private readonly from: HTMLInputElement;
  private readonly to: HTMLInputElement;
  private readonly suggestions = h("datalist");
  private readonly params = h("div", "route-params");
  private readonly actions = h("div", "buttons");
  private readonly status = h("div", "muted small route-status");
  private readonly result = h("div", "route-result");
  private suggestTimer = 0;
  private suggestSeq = 0;

  // Galaxy and carrier options keep their state across re-renders.
  private readonly fields = {
    range: numberField(1, 500, 0.01),
    efficiency: numberField(1, 100, 1),
    cargo: numberField(0, 1600, 1),
    capacityUsed: numberField(0, 60000, 1),
    useSupercharge: checkbox(true),
    refuelEveryScoopable: checkbox(false),
    isSupercharged: checkbox(false),
    useInjections: checkbox(false),
    excludeSecondary: checkbox(false),
    calculateStartingFuel: checkbox(false),
    squadron: checkbox(false),
  };

  constructor(private readonly deps: RouteDeps) {
    this.suggestions.id = "route-systems";
    this.from = this.systemField("From");
    this.to = this.systemField("To");
    this.toggle.setAttribute("aria-expanded", "false");
    this.toggle.title = "Plot a route on Spansh: neutron highway, fuel-exact ship route, or fleet-carrier hops";
    this.toggle.append(h("span", "chevron", "▸"), h("h2", undefined, "Route"), this.total);
    this.fields.efficiency.value = "60";

    const useHere = button("⌖", () => this.fillCurrent(), "quiet route-here");
    useHere.title = "Start from your current location";
    const swap = button("⇅", () => this.swapEnds(), "quiet route-swap");
    swap.title = "Swap the ends";
    const fromRow = h("div", "route-ends", this.from, useHere);
    const toRow = h("div", "route-ends", this.to, swap);

    this.body.append(this.modes, fromRow, toRow, this.params, this.actions, this.status, this.result, this.suggestions);
    this.body.hidden = true;
    this.panel.append(this.toggle, this.body);
    this.renderModes();
    this.renderParams();
    this.renderActions();
    this.renderResult();
  }

  // --- Loading and persistence ------------------------------------------------

  /** Restores the saved route and reads the ship from the journals. */
  async load(): Promise<void> {
    try {
      const saved = await savedRoute();
      if (saved) {
        this.saved = saved;
        this.mode = saved.route.mode;
        this.show();
      }
    } catch (e) {
      console.error("saved route:", e);
    }
    await this.loadShip();
    this.renderModes();
    this.renderParams();
  }

  private async loadShip(): Promise<void> {
    try {
      this.ship = await shipLoadout(this.deps.fid());
      if (this.ship && !this.fields.range.value) {
        this.fields.range.value = this.ship.maxJumpRange.toFixed(2);
      }
      if (this.ship && !this.fields.cargo.value) {
        this.fields.cargo.value = "0";
      }
    } catch (e) {
      console.error("ship loadout:", e);
    }
  }

  private persist(): void {
    saveRoute(this.saved).catch((e) => this.deps.toasts.error(`Couldn't save the route: ${errorText(e)}`));
  }

  /** Puts the route (or nothing) on the map and in the list. */
  private show(): void {
    const r = this.saved;
    this.deps.layer.setRoute(r?.route.waypoints ?? [], r?.progress ?? 0);
    this.deps.onChange();
    this.total.textContent = r ? `${r.route.waypoints.length}` : "";
    this.renderResult();
  }

  // --- Following the commander --------------------------------------------------

  /** A journal jump arrived: advance along the route if it reached a waypoint. */
  onJump(jump: Jump): void {
    const saved = this.saved;
    if (!saved || this.plotting) return;
    const { waypoints } = saved.route;
    const matches = (w: Waypoint) => (jump.id64 && w.id64 === jump.id64) || w.name.toLowerCase() === jump.system.toLowerCase();
    let reached = -1;
    for (let i = saved.progress + 1; i < waypoints.length; i++) {
      if (matches(waypoints[i])) {
        reached = i;
        break;
      }
    }
    if (reached < 0) return;
    saved.progress = reached;
    this.persist();
    this.deps.layer.setProgress(reached);
    this.deps.onChange();
    this.renderResult();
    const next = waypoints[reached + 1];
    if (!next) {
      this.deps.toasts.show(`Route complete: ${saved.route.destination}.`);
      return;
    }
    if (saved.copyNext) {
      void copyText(next.name);
      this.deps.toasts.show(`Waypoint ${reached + 1} of ${waypoints.length - 1} · next: ${next.name} (copied)`, "info", { timeout: 6 });
    }
  }

  // --- The panel ----------------------------------------------------------------

  toggleOpen(): void {
    this.setOpen(!this.open);
  }

  setOpen(open: boolean): void {
    this.open = open;
    this.body.hidden = !open;
    this.panel.classList.toggle("open", open);
    this.toggle.setAttribute("aria-expanded", String(open));
    if (open) {
      if (!this.from.value) this.fillCurrent(false);
      this.renderResult();
    }
  }

  private fillCurrent(interactive = true): void {
    const here = this.deps.currentSystem();
    if (here) this.from.value = here;
    else if (interactive) this.deps.toasts.show("There's no commander location yet.", "info", { timeout: 4 });
  }

  private swapEnds(): void {
    [this.from.value, this.to.value] = [this.to.value, this.from.value];
  }

  /** A system-name input that suggests catalogue names as you type. */
  private systemField(placeholder: string): HTMLInputElement {
    const input = h("input");
    input.type = "text";
    input.placeholder = placeholder;
    input.spellcheck = false;
    input.autocomplete = "off";
    input.setAttribute("list", "route-systems");
    input.addEventListener("input", () => {
      window.clearTimeout(this.suggestTimer);
      this.suggestTimer = window.setTimeout(async () => {
        const query = input.value.trim();
        if (query.length < 2) return;
        const mine = ++this.suggestSeq;
        const found = await searchSystems(query).catch(() => []);
        if (mine !== this.suggestSeq) return;
        this.suggestions.replaceChildren(
          ...found.slice(0, 10).map((s) => {
            const option = document.createElement("option");
            option.value = s.name;
            return option;
          }),
        );
      }, 150);
    });
    return input;
  }

  private renderModes(): void {
    this.modes.replaceChildren(
      ...MODES.map((m) => {
        const b = button(m.label, () => {
          this.mode = m.id;
          this.renderModes();
          this.renderParams();
        });
        b.title = m.hint;
        b.setAttribute("aria-pressed", String(m.id === this.mode));
        b.classList.toggle("on", m.id === this.mode);
        return b;
      }),
    );
  }

  private renderParams(): void {
    const f = this.fields;
    const parts: Child[] = [];
    if (this.mode === "neutron") {
      parts.push(
        labelled("Jump range (ly)", f.range, "Your ship's full-tank jump range"),
        labelled("Efficiency %", f.efficiency, "Lower is more direct; 60 is Spansh's default"),
      );
    } else if (this.mode === "galaxy") {
      parts.push(
        h(
          "p",
          this.ship ? "muted small" : "problem small",
          this.ship
            ? `${shipLabel(this.ship)} · ${this.ship.maxJumpRange.toFixed(1)} ly, from the journals.`
            : "No ship loadout in the journals yet: start the game once with the ship you'll fly, or plot a neutron route instead.",
        ),
        labelled("Cargo (t)", f.cargo, "Tonnes aboard while flying the route"),
        option("Use neutron supercharges", f.useSupercharge),
        option("Refuel at every scoopable star", f.refuelEveryScoopable),
        option("Already supercharged now", f.isSupercharged),
        option("Use FSD injections", f.useInjections),
        option("Exclude secondary stars", f.excludeSecondary),
      );
    } else {
      this.to.placeholder = "To (commas for stops: A, B, C)";
      parts.push(
        labelled("Tritium and cargo aboard (t)", f.capacityUsed, "Used capacity from the carrier management screen"),
        option("Work out the starting tritium", f.calculateStartingFuel),
        option("Squadron carrier", f.squadron),
      );
    }
    if (this.mode !== "carrier") this.to.placeholder = "To";
    this.params.replaceChildren(...parts.filter((p): p is Node | string => !!p));
  }

  private renderActions(): void {
    const plot = button(this.plotting ? "Plotting…" : "Plot route", () => void this.plot(), "primary");
    plot.disabled = this.plotting;
    const cancel = button("Cancel", () => void cancelPlot().catch(() => {}));
    cancel.hidden = !this.plotting;
    this.actions.replaceChildren(plot, cancel);
  }

  // --- Plotting -------------------------------------------------------------------

  private request(): RouteRequest | string {
    const from = this.from.value.trim();
    const to = this.to.value.trim();
    if (!from || !to) return "Give both ends of the route.";
    const f = this.fields;
    switch (this.mode) {
      case "neutron": {
        const range = Number(f.range.value);
        if (!(range > 0)) return "Give your ship's jump range.";
        const efficiency = Math.min(100, Math.max(1, Math.round(Number(f.efficiency.value) || 60)));
        return { mode: "neutron", from, to, range, efficiency };
      }
      case "galaxy": {
        if (!this.ship) return "No ship loadout in the journals yet; plot a neutron route instead.";
        return {
          mode: "galaxy",
          source: from,
          destination: to,
          ship: this.ship,
          cargo: Math.max(0, Math.round(Number(f.cargo.value) || 0)),
          isSupercharged: f.isSupercharged.checked,
          useSupercharge: f.useSupercharge.checked,
          useInjections: f.useInjections.checked,
          excludeSecondary: f.excludeSecondary.checked,
          refuelEveryScoopable: f.refuelEveryScoopable.checked,
        };
      }
      case "carrier": {
        const destinations = to.split(",").map((s) => s.trim()).filter(Boolean);
        return {
          mode: "carrier",
          source: from,
          destinations,
          capacityUsed: Math.max(0, Math.round(Number(f.capacityUsed.value) || 0)),
          calculateStartingFuel: f.calculateStartingFuel.checked,
          squadron: f.squadron.checked,
        };
      }
    }
  }

  private async plot(): Promise<void> {
    if (this.plotting) return;
    const request = this.request();
    if (typeof request === "string") {
      this.deps.toasts.error(request, { timeout: 6 });
      return;
    }
    this.plotting = true;
    this.plotStarted = Date.now();
    this.renderActions();
    this.status.textContent = "Asking Spansh…";
    this.plotTimer = window.setInterval(() => {
      const s = Math.round((Date.now() - this.plotStarted) / 1000);
      this.status.textContent = `Spansh is plotting… ${s} s`;
    }, 1000);
    try {
      const route = await plotRoute(request);
      this.saved = { route, progress: this.startingProgress(route), copyNext: this.saved?.copyNext ?? true };
      this.persist();
      this.show();
    } catch (e) {
      if (!String(e).includes("cancelled")) {
        this.deps.toasts.error(`Couldn't plot the route: ${errorText(e)}`, { timeout: 10 });
      }
    } finally {
      this.plotting = false;
      window.clearInterval(this.plotTimer);
      this.status.textContent = "";
      this.renderActions();
    }
  }

  /** Starts the route wherever the commander already is on it. */
  private startingProgress(route: PlottedRoute): number {
    const here = this.deps.currentSystem()?.toLowerCase();
    if (!here) return 0;
    const i = route.waypoints.findIndex((w) => w.name.toLowerCase() === here);
    return Math.max(0, i);
  }

  private clear(): void {
    this.saved = null;
    this.persist();
    this.show();
  }

  // --- The plotted route ------------------------------------------------------------

  private renderResult(): void {
    const saved = this.saved;
    if (!saved) {
      this.result.replaceChildren();
      return;
    }
    const { route } = saved;
    const copyNext = checkbox(saved.copyNext);
    copyNext.addEventListener("change", () => {
      saved.copyNext = copyNext.checked;
      this.persist();
    });
    const next = route.waypoints[saved.progress + 1];
    const copyNow = next
      ? button("Copy next", () => {
          void copyText(next.name);
          this.deps.toasts.show(`Copied ${next.name}.`, "info", { timeout: 3 });
        })
      : null;
    this.result.replaceChildren(
      h("div", "route-summary", this.summary(route, saved.progress)),
      option("Copy the next system after each jump", copyNext),
      this.renderList(saved),
      h("div", "buttons", copyNow, button("Clear route", () => this.clear(), "quiet push")),
    );
  }

  private summary(route: PlottedRoute, progress: number): string {
    const mode = MODES.find((m) => m.id === route.mode)?.label ?? route.mode;
    const n = route.waypoints.length;
    const parts = [`${route.source} → ${route.destination}`, `${mode}: ${n} waypoints, ${fmtLy(route.totalDistance)}`];
    if (route.mode === "neutron" && route.totalJumps != null) parts.push(`about ${route.totalJumps} jumps`);
    const restock = route.waypoints.reduce((sum, w) => sum + (w.restock ?? 0), 0);
    if (restock > 0) parts.push(`load ${Math.round(restock).toLocaleString()} t of tritium along the way`);
    if (progress > 0 && progress < n - 1) {
      parts.push(`at waypoint ${progress + 1}, ${fmtLy(route.waypoints[progress].distanceLeft)} to go`);
    }
    if (progress >= n - 1) parts.push("complete");
    return parts.join(" · ");
  }

  private renderList(saved: SavedRoute): HTMLElement {
    const { waypoints } = saved.route;
    // A window around the commander: the last flown waypoint, then what's ahead.
    const start = Math.max(0, saved.progress - 1);
    const end = Math.min(waypoints.length, start + MAX_ROWS);
    const list = h("ol", "route-list");
    for (let i = start; i < end; i++) {
      list.append(this.row(waypoints[i], i, saved.progress));
    }
    const notes: string[] = [];
    if (start > 0) notes.push(`${start} flown`);
    if (end < waypoints.length) notes.push(`${waypoints.length - end} more`);
    const wrap = h("div", "route-listing", list);
    if (notes.length) wrap.append(h("div", "muted small", `(${notes.join(" · ")} not listed)`));
    return wrap;
  }

  private row(w: Waypoint, i: number, progress: number): HTMLLIElement {
    const b = button("", () => this.deps.onPick({ rowid: null, id64: w.id64, name: w.name, pos: w.pos }), "poi-row route-row");
    const badges: Child[] = [];
    if (w.neutron) badges.push(h("span", "route-badge neutron", "neutron"));
    if (w.scoopable && !w.refuel) badges.push(h("span", "route-badge", "scoopable"));
    if (w.refuel && w.restock == null) badges.push(h("span", "route-badge fuel", "refuel"));
    if (w.restock != null) badges.push(h("span", "route-badge fuel", `+${Math.round(w.restock).toLocaleString()} t`));
    if (w.icyRing) badges.push(h("span", "route-badge", w.pristine ? "pristine ice" : "icy ring"));
    if (w.destination) badges.push(h("span", "route-badge dest", "stop"));
    const legs: string[] = [];
    if (i > 0 && w.distance > 0) legs.push(fmtLy(w.distance));
    if (w.jumps != null && i > 0) legs.push(`~${w.jumps} jumps`);
    const title: string[] = [];
    if (w.fuelUsed != null && i > 0) title.push(`Fuel used: ${w.fuelUsed.toFixed(1)} t`);
    if (w.fuelLeft != null) title.push(`Fuel after: ${w.fuelLeft.toFixed(1)} t`);
    if (title.length) b.title = title.join(" · ");
    b.append(
      h("span", "route-index", `${i + 1}`),
      h("span", "text", h("span", "name", w.name), badges.length ? h("span", "meta route-badges", ...badges) : null),
      h("span", "dist", legs.join(" · ")),
    );
    const li = h("li", undefined, b);
    if (i === progress) li.className = "here";
    else if (i === progress + 1) li.className = "next";
    else if (i < progress) li.className = "flown";
    return li;
  }
}

// --- Small form helpers ------------------------------------------------------------

function numberField(min: number, max: number, step: number): HTMLInputElement {
  const input = h("input");
  input.type = "number";
  input.min = String(min);
  input.max = String(max);
  input.step = String(step);
  return input;
}

function checkbox(checked: boolean): HTMLInputElement {
  const box = h("input");
  box.type = "checkbox";
  box.checked = checked;
  return box;
}

function labelled(text: string, field: HTMLInputElement, title?: string): HTMLLabelElement {
  const label = h("label", "row route-field", text, field);
  if (title) label.title = title;
  return label;
}

function option(text: string, box: HTMLInputElement, title?: string): HTMLLabelElement {
  const label = h("label", "route-option", box, text);
  if (title) label.title = title;
  return label;
}
