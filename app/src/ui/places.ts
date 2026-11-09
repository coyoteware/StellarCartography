// The points-of-interest directory: the "Places" panel under the search box
// (collapsed to its header until opened: filter, category switches, list by
// distance), the marker layer it drives, and downloading or updating the
// catalogue from EDAstro.

import {
  cancelPoiUpdate,
  onPoiDone,
  onPoiProgress,
  poiUpdateStatus,
  startPoiUpdate,
  type PoiDone,
  type PoiProgress,
  type Vec3,
  type ViewSettings,
} from "../api";
import { CATEGORIES, categoryInfo, glyph, loadPoi, poisInSystem, searchPoi, type Poi, type PoiData } from "../poi";
import { distance } from "../renderer/coords";
import { PoiLayer } from "../renderer/poi";
import { button, h, type Child } from "./dom";
import { DOWNLOAD_SIZES, DownloadJob } from "./download";
import { errorText, type Toasts } from "./toast";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

/** Rows drawn at once; the filter narrows the rest. */
const MAX_ROWS = 200;

const PHASE_NAMES: Record<PoiProgress["phase"], string> = {
  gec: "Galactic Exploration Catalog",
  edsm: "Galactic Mapping Project",
  carriers: "fleet carriers",
  codex: "codex reports",
};

const mb = (bytes: number) => `${Math.round(bytes / 1e6).toLocaleString()} MB`;
const ly = (d: number) => `${d < 10 ? d.toFixed(1) : Math.round(d).toLocaleString()} ly`;

/** "Megaship · in Duamta", "Carrier hub · 120 carriers"... */
export function describePoi(p: Poi): string {
  const parts = [p.kind];
  if (p.weight != null && p.category === "carrierHub") parts.push(`${p.weight.toLocaleString()} carriers`);
  if (p.system && p.system.toLowerCase() !== p.name.toLowerCase()) parts.push(`in ${p.system}`);
  return parts.join(" · ");
}

export interface PlacesDeps {
  toasts: Toasts;
  view: () => ViewSettings;
  /** Saves new view settings; they come back through `sync`. */
  onView: (view: ViewSettings) => void;
  /** Where distances are measured from: the commander, else the view's centre. */
  origin: () => { pos: Vec3; what: string };
  onPick: (poi: Poi) => void;
  /** A new marker layer (after loading or an update), or none. */
  onChange: (layer: PoiLayer | null) => void;
}

type DataState = "idle" | "loading" | "missing" | "error" | "ready";

export class PlacesUi {
  layer: PoiLayer | null = null;
  private data: PoiData | null = null;
  private state: DataState = "idle";
  private error = "";
  private readonly job: DownloadJob<PoiProgress, PoiDone>;
  /** Kept across reloads, so the marker stays lit when an update swaps the layer. */
  private selectedId: string | null = null;
  /** Bumped by each load: a slower, older load mustn't install its layer last. */
  private loads = 0;
  private readonly panel = $("places-panel");
  private readonly body = h("div", "places-body");
  private readonly toggle = button("", () => this.setOpen(!this.open), "places-toggle");
  private readonly total = h("span", "count");
  private open = false;
  private readonly filter: HTMLInputElement;
  private readonly chips = h("div", "poi-chips");
  private readonly list = h("ul", "poi-list");
  private readonly more = h("div", "muted small");
  private readonly status = h("div", "poi-status");
  private readonly footer = h("div", "muted small source");
  private filterTimer = 0;
  /** The settings the layer and list were last brought in line with. */
  private applied = "";
  /** Settings panel status line, refreshed when anything changes. */
  onStatus: () => void = () => {};

  constructor(private readonly deps: PlacesDeps) {
    this.filter = h("input");
    this.filter.type = "search";
    this.filter.placeholder = "Filter places…";
    this.filter.spellcheck = false;
    this.filter.autocomplete = "off";
    this.filter.addEventListener("input", () => {
      window.clearTimeout(this.filterTimer);
      this.filterTimer = window.setTimeout(() => this.renderList(), 100);
    });
    this.filter.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      // Escape here shouldn't also close the system panel.
      e.preventDefault();
      e.stopPropagation();
      if (this.filter.value) {
        this.filter.value = "";
        this.renderList();
      } else this.setOpen(false);
    });

    this.job = new DownloadJob<PoiProgress, PoiDone>({
      start: startPoiUpdate,
      cancel: cancelPoiUpdate,
      status: poiUpdateStatus,
      onProgressEvent: onPoiProgress,
      onDoneEvent: onPoiDone,
      describe: progressText,
      fraction: (p) => (p?.bytesTotal ? p.bytesRead / p.bytesTotal : null),
      onRunningChange: () => this.render(),
      onProgress: () => this.onStatus(),
      onDone: (d) => void this.done(d),
      onStartError: (e) => this.deps.toasts.error(`Couldn't start the download: ${errorText(e)}`),
    });

    this.toggle.setAttribute("aria-expanded", "false");
    this.toggle.title = "Points of interest: Guardian and Thargoid sites, carrier hubs, megaships, nebulae, sights";
    this.toggle.append(h("span", "chevron", "▸"), h("h2", undefined, "Places"), this.total);
    this.body.append(this.status, this.filter, this.chips, this.list, this.more, this.footer);
    this.body.hidden = true;
    this.panel.append(this.toggle, this.body);
  }

  get points(): PoiData | null {
    return this.data;
  }

  get running(): boolean {
    return this.job.running;
  }

  setOpen(open: boolean): void {
    this.open = open;
    this.body.hidden = !open;
    this.panel.classList.toggle("open", open);
    this.toggle.setAttribute("aria-expanded", String(open));
    if (open) {
      this.render();
      if (this.state === "ready") this.filter.focus();
    }
  }

  /** Loads poi.json (again, after an update) and picks up a download already running. */
  async load(): Promise<void> {
    const mine = ++this.loads;
    this.state = "loading";
    this.render();
    try {
      await this.job.resume();
      const data = await loadPoi();
      if (mine !== this.loads) return;
      this.layer?.dispose();
      this.data = data;
      this.layer = data && new PoiLayer(data.pois, $("poi-labels"));
      this.state = data ? "ready" : "missing";
      // The new layer starts with nothing lit: light the place still selected.
      const selected = this.selectedId && data?.pois.find((p) => p.id === this.selectedId);
      this.highlight(selected || null);
    } catch (e) {
      if (mine !== this.loads) return;
      console.error("points of interest:", e);
      this.layer?.dispose();
      this.data = this.layer = null;
      this.state = "error";
      this.error = errorText(e);
    }
    this.applied = "";
    this.sync();
  }

  /** Brings the layer, the category switches and the list in line with the view settings. */
  sync(): void {
    const view = this.deps.view();
    const key = JSON.stringify([view.poi, view.poiHidden, this.state, this.data?.builtAt]);
    if (key === this.applied) return;
    this.applied = key;
    if (this.layer) {
      this.layer.setHidden(new Set(view.poiHidden));
      this.layer.visible = view.poi;
    }
    this.render();
    this.deps.onChange(this.layer);
  }

  /** Points of interest in a system, for the details panel. */
  inSystem(id64: string | null, name: string): Poi[] {
    return this.data ? poisInSystem(this.data, id64, name) : [];
  }

  /** Search-box matches, best first. */
  search(query: string, limit: number): Poi[] {
    return this.data && query.trim().length >= 2 ? searchPoi(this.data, query, undefined, limit) : [];
  }

  /** Lights `poi`'s marker; null clears it. */
  highlight(poi: Poi | null): void {
    this.selectedId = poi?.id ?? null;
    this.layer?.setSelected(poi ? this.layer.indexOf(poi) : null);
  }

  /** One line for the settings panel. */
  statusText(): string {
    if (this.job.running) return this.job.text();
    switch (this.state) {
      case "ready": {
        const d = this.data!;
        const when = d.builtAt ? `, downloaded ${new Date(d.builtAt * 1000).toLocaleDateString()}` : "";
        return `${d.pois.length.toLocaleString()} places${when}.`;
      }
      case "missing":
        return "Not downloaded yet.";
      case "error":
        return `Can't be used: ${this.error}`;
      default:
        return "Loading…";
    }
  }

  startUpdate(): Promise<void> {
    return this.job.start();
  }

  private async done(d: PoiDone): Promise<void> {
    if (d.ok) {
      const minutes = d.seconds ? ` in ${Math.max(1, Math.round(d.seconds / 60))} min` : "";
      this.deps.toasts.show(`Points of interest updated: ${(d.pois ?? 0).toLocaleString()} places${minutes}.`);
      await this.load();
    } else if (d.cancelled) {
      this.deps.toasts.show("Download cancelled; the points of interest are unchanged.");
      this.render();
    } else {
      this.deps.toasts.error(`Couldn't update the points of interest: ${d.error ?? "unknown error"}`, {
        action: { label: "Try again", run: () => void this.startUpdate() },
      });
      this.render();
    }
  }

  private toggleCategory(id: string): void {
    const view = this.deps.view();
    const hidden = new Set(view.poiHidden);
    if (hidden.has(id)) hidden.delete(id);
    else hidden.add(id);
    this.deps.onView({ ...view, poiHidden: [...hidden] });
  }

  // --- Rendering ------------------------------------------------------------

  private render(): void {
    this.onStatus();
    const ready = this.state === "ready" && !!this.data;
    const parts: Child[] = [];
    if (this.job.running) parts.push(this.job.block());
    if (this.state === "loading" || this.state === "idle") parts.push(h("div", "muted small", "Loading points of interest…"));
    else if (this.state === "missing" && !this.job.running) parts.push(this.downloadPrompt());
    else if (this.state === "error") parts.push(this.errorBlock());
    this.status.replaceChildren(...parts.filter((p): p is Node | string => !!p));
    this.status.hidden = parts.length === 0;
    this.filter.hidden = this.chips.hidden = this.list.hidden = this.more.hidden = this.footer.hidden = !ready;
    this.total.textContent = ready ? this.data!.pois.length.toLocaleString() : "";
    if (!ready || !this.open) return;
    this.renderChips();
    this.renderList();
    const d = this.data!;
    this.footer.textContent = `From EDAstro${d.builtAt ? `, downloaded ${new Date(d.builtAt * 1000).toLocaleDateString()}` : ""}.`;
  }

  private renderChips(): void {
    const d = this.data!;
    const hidden = new Set(this.deps.view().poiHidden);
    this.chips.replaceChildren(
      ...CATEGORIES.filter((c) => (d.counts[c.id] ?? 0) > 0).map((c) => {
        const on = !hidden.has(c.id);
        const b = button("", () => this.toggleCategory(c.id), on ? "poi-chip on" : "poi-chip");
        b.setAttribute("aria-pressed", String(on));
        b.title = on ? "Shown: click to hide" : "Hidden: click to show";
        b.append(glyph(c.shape), h("span", "label", c.label), h("span", "count", (d.counts[c.id] ?? 0).toLocaleString()));
        return b;
      }),
    );
  }

  private renderList(): void {
    const d = this.data;
    if (!d || !this.open) return;
    const hidden = new Set(this.deps.view().poiHidden);
    const query = this.filter.value;
    const shown = (p: Poi) => !hidden.has(p.category);
    const { pos: origin, what } = this.deps.origin();
    const found = query.trim() ? searchPoi(d, query, shown) : d.pois.filter(shown);
    const withDistance = found.map((p) => ({ p, d: distance(p.pos, origin) }));
    // With a filter, best matches first; otherwise nearest first.
    if (!query.trim()) withDistance.sort((a, b) => a.d - b.d);
    this.list.replaceChildren(...withDistance.slice(0, MAX_ROWS).map(({ p, d }) => this.row(p, d)));
    const rest = withDistance.length - MAX_ROWS;
    this.more.textContent =
      withDistance.length === 0
        ? query.trim()
          ? "Nothing matches."
          : "Every category is hidden."
        : rest > 0
          ? `${rest.toLocaleString()} more: refine the filter. Distances are from ${what}.`
          : `Distances are from ${what}.`;
  }

  private row(p: Poi, dist: number): HTMLLIElement {
    const b = button("", () => this.deps.onPick(p), "poi-row");
    b.title = p.summary ?? describePoi(p);
    b.append(
      glyph(categoryInfo(p.category).shape),
      h("span", "text", h("span", "name", p.name), h("span", "meta", describePoi(p))),
      h("span", "dist", ly(dist)),
    );
    return h("li", undefined, b);
  }

  private downloadPrompt(): HTMLElement {
    return h(
      "div",
      "overlay-prompt",
      h(
        "p",
        "small",
        `Guardian and Thargoid sites, carrier hubs, megaships, nebulae and sights come from EDAstro's catalogues: ${DOWNLOAD_SIZES.poi} to download (not kept), a few minutes. The map stays usable meanwhile.`,
      ),
      button("Download", () => void this.startUpdate(), "primary"),
    );
  }

  private errorBlock(): HTMLElement {
    return h(
      "div",
      "overlay-prompt",
      h("p", "problem small", `The points of interest can't be read: ${this.error}`),
      button("Download again", () => void this.startUpdate()),
    );
  }
}

function progressText(p: PoiProgress | null): string {
  if (!p) return "Connecting to EDAstro…";
  const of = p.bytesTotal ? `${mb(p.bytesRead)} of ${mb(p.bytesTotal)}` : mb(p.bytesRead);
  return `Downloading ${PHASE_NAMES[p.phase]} (${p.step} of ${p.steps}): ${of}`;
}
