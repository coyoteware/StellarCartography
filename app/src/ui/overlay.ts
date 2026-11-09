// The overlay layers in the layers panel: the "Colour systems by" picker,
// its legend (which also picks what's coloured), the minor-faction search,
// and downloading or updating the populated systems from Spansh.

import {
  cancelPopulatedUpdate,
  onPopulatedDone,
  onPopulatedProgress,
  populatedUpdateStatus,
  startPopulatedUpdate,
  type PopulatedDone,
  type PopulatedProgress,
  type SystemInfo,
  type ViewSettings,
} from "../api";
import {
  MAX_HIGHLIGHTS,
  OVERLAYS,
  defaultHighlights,
  isNominal,
  isOverlayId,
  overlayStyle,
  searchFactions,
  type Legend,
  type LegendItem,
  type OverlayId,
  type OverlayStyle,
  type Tick,
} from "../overlays";
import { PopulatedLayer } from "../renderer/populated";
import { loadPopulated, tableFor, type PopulatedData } from "../renderer/populated-data";
import { button, h, type Child } from "./dom";
import { DOWNLOAD_SIZES, DownloadJob } from "./download";
import { errorText, type Toasts } from "./toast";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const gb = (bytes: number) => (bytes / 2 ** 30).toFixed(1) + " GB";
const clock = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
const day = (iso: string | null) => (iso && !Number.isNaN(Date.parse(iso)) ? new Date(iso).toLocaleDateString() : null);

export interface OverlayDeps {
  toasts: Toasts;
  view: () => ViewSettings;
  /** Saves new view settings; they come back through `sync`. */
  onView: (view: ViewSettings) => void;
  /** The drawn overlay changed: a new layer, style or visibility. */
  onChange: (layer: PopulatedLayer | null) => void;
}

type DataState = "idle" | "loading" | "missing" | "error" | "ready";

export class OverlayUi {
  layer: PopulatedLayer | null = null;
  private data: PopulatedData | null = null;
  private state: DataState = "idle";
  private error = "";
  private readonly job: DownloadJob<PopulatedProgress, PopulatedDone>;
  /** Ordinal layers: values switched off in the legend, for this session. */
  private readonly hidden = new Map<OverlayId, Set<string>>();
  /** A one-off hint under the legend (cleared by the next change). */
  private hint = "";
  /** The settings the drawn style was made from, to skip needless restyles. */
  private applied = "";
  private style: OverlayStyle | null = null;
  private readonly select = $<HTMLSelectElement>("overlay-select");
  private readonly body = $("overlay-body");
  private readonly factionInput: HTMLInputElement;
  private readonly factionResults = h("ul", "faction-results");
  private factionActive = 0;
  /** Settings panel status line, refreshed when anything changes. */
  onStatus: () => void = () => {};

  constructor(private readonly deps: OverlayDeps) {
    for (const o of OVERLAYS) {
      const opt = h("option", undefined, o.label);
      opt.value = o.id;
      this.select.append(opt);
    }
    this.select.addEventListener("change", () => {
      this.hint = "";
      this.deps.onView({ ...this.deps.view(), overlay: this.select.value });
    });

    this.factionInput = h("input");
    this.factionInput.type = "search";
    this.factionInput.placeholder = "Find a minor faction…";
    this.factionInput.spellcheck = false;
    this.factionInput.autocomplete = "off";
    this.factionInput.addEventListener("input", () => {
      this.factionActive = 0;
      this.renderFactionResults();
    });
    this.factionInput.addEventListener("keydown", (e) => {
      const items = this.factionResults.querySelectorAll("button");
      if (e.key === "ArrowDown") this.factionActive = Math.min(this.factionActive + 1, items.length - 1);
      else if (e.key === "ArrowUp") this.factionActive = Math.max(this.factionActive - 1, 0);
      else if (e.key === "Enter") items[this.factionActive]?.click();
      else if (e.key === "Escape") this.factionInput.value = "";
      else return;
      e.preventDefault();
      // Escape here shouldn't also close the system panel.
      e.stopPropagation();
      this.renderFactionResults();
    });

    // Its progress block is kept across renders, so ticks don't rebuild the
    // legend (or take the faction search's focus).
    this.job = new DownloadJob<PopulatedProgress, PopulatedDone>({
      start: startPopulatedUpdate,
      cancel: cancelPopulatedUpdate,
      status: populatedUpdateStatus,
      onProgressEvent: onPopulatedProgress,
      onDoneEvent: onPopulatedDone,
      describe: progressText,
      fraction: (p) => (p?.bytesTotal ? p.bytesRead / p.bytesTotal : null),
      onRunningChange: () => this.render(),
      onProgress: () => this.onStatus(),
      onDone: (d) => void this.done(d),
      onStartError: (e) => this.deps.toasts.error(`Couldn't start the download: ${errorText(e)}`),
    });
  }

  get mode(): OverlayId {
    const m = this.deps.view().overlay;
    return isOverlayId(m) ? m : "none";
  }

  /** True while a layer is drawn over the stars. */
  get active(): boolean {
    return this.mode !== "none" && this.state === "ready" && !!this.layer;
  }

  get populated(): PopulatedData | null {
    return this.data;
  }

  /** Loads populated.bin (again, after an update) and picks up a download already running. */
  async load(): Promise<void> {
    this.state = "loading";
    this.render();
    try {
      await this.job.resume();
      const data = await loadPopulated();
      this.data = data;
      this.layer = data && new PopulatedLayer(data);
      this.state = data ? "ready" : "missing";
    } catch (e) {
      console.error("populated systems:", e);
      this.data = this.layer = null;
      this.state = "error";
      this.error = errorText(e);
    }
    this.applied = "";
    this.sync();
  }

  /** Brings the picker, the drawn style and the legend in line with the view settings. */
  sync(): void {
    const mode = this.mode;
    this.select.value = mode;
    const view = this.deps.view();
    const key = JSON.stringify([mode, view.highlights[mode] ?? null, [...(this.hidden.get(mode) ?? [])], this.state]);
    if (key === this.applied) return;
    this.applied = key;
    this.style = null;
    if (this.layer && this.data) {
      this.style = overlayStyle(this.data, mode, { highlights: this.highlights(mode), hidden: this.hiddenFor(mode) });
      this.layer.setStyle(this.style);
      this.layer.object.visible = mode !== "none";
    }
    this.render();
    this.deps.onChange(this.layer);
  }

  /** The system behind dot i, for the details panel. */
  systemInfo(i: number): SystemInfo | null {
    const d = this.data;
    if (!d || i < 0 || i >= d.count) return null;
    return { rowid: null, id64: d.id64[i].toString(), name: d.names[i], pos: [d.pos[i * 3], d.pos[i * 3 + 1], d.pos[i * 3 + 2]] };
  }

  /** One line for the settings panel. */
  statusText(): string {
    if (this.job.running) return this.job.text();
    switch (this.state) {
      case "ready": {
        const d = this.data!;
        const when = day(d.newestUpdate) ?? day(d.sourceLastModified);
        return `${d.count.toLocaleString()} systems${when ? `, Spansh data up to ${when}` : ""}.`;
      }
      case "missing":
        return "Not downloaded yet.";
      case "error":
        return `Can't be used: ${this.error}`;
      default:
        return "Loading…";
    }
  }

  get running(): boolean {
    return this.job.running;
  }

  startUpdate(): Promise<void> {
    return this.job.start();
  }

  private async done(d: PopulatedDone): Promise<void> {
    if (d.ok) {
      const minutes = d.seconds ? ` in ${Math.max(1, Math.round(d.seconds / 60))} min` : "";
      this.deps.toasts.show(`Populated systems updated: ${(d.systems ?? 0).toLocaleString()} systems${minutes}.`);
      await this.load();
    } else if (d.cancelled) {
      this.deps.toasts.show("Download cancelled; the populated systems are unchanged.");
      this.render();
    } else {
      this.deps.toasts.error(`Couldn't update the populated systems: ${d.error ?? "unknown error"}`, {
        action: { label: "Try again", run: () => void this.startUpdate() },
      });
      this.render();
    }
  }

  /** Picked values by slot, without any the data doesn't have. */
  private highlights(mode: OverlayId): string[] {
    const saved = this.deps.view().highlights[mode];
    if (!this.data) return saved ?? [];
    if (mode === "faction") return saved ?? [];
    if (!saved) return defaultHighlights(this.data, mode);
    const column = mode === "power" ? "power" : (mode as "allegiance" | "government" | "economy");
    const table = isNominal(mode) ? tableFor(this.data, column) : [];
    return saved.slice(0, MAX_HIGHLIGHTS).map((k) => (table.includes(k) ? k : ""));
  }

  private hiddenFor(mode: OverlayId): Set<string> {
    let set = this.hidden.get(mode);
    if (!set) this.hidden.set(mode, (set = new Set()));
    return set;
  }

  private saveHighlights(mode: OverlayId, values: string[]): void {
    const view = this.deps.view();
    // Keep empty slots in the middle so the others keep their colours.
    while (values.length && !values[values.length - 1]) values.pop();
    this.deps.onView({ ...view, highlights: { ...view.highlights, [mode]: values } });
  }

  private toggleHighlight(mode: OverlayId, key: string): void {
    const slots = this.highlights(mode);
    while (slots.length < MAX_HIGHLIGHTS) slots.push("");
    const at = slots.indexOf(key);
    if (at >= 0) {
      slots[at] = "";
      this.hint = "";
    } else {
      const free = slots.indexOf("");
      if (free < 0) {
        this.hint = `${MAX_HIGHLIGHTS} colours at most: click a coloured one to clear it first.`;
        this.render();
        return;
      }
      slots[free] = key;
      this.hint = "";
    }
    this.saveHighlights(mode, slots);
  }

  private toggleHidden(mode: OverlayId, key: string): void {
    const set = this.hiddenFor(mode);
    if (set.has(key)) set.delete(key);
    else set.add(key);
    this.sync();
  }

  private pickFaction(name: string | null): void {
    this.factionInput.value = "";
    this.saveHighlights("faction", name ? [name] : []);
  }

  // --- Rendering ------------------------------------------------------------

  private render(): void {
    this.onStatus();
    const mode = this.mode;
    const parts: Child[] = [];
    if (this.job.running) parts.push(this.job.block());
    if (mode !== "none") {
      if (this.state === "loading" || this.state === "idle") parts.push(h("div", "muted small", "Loading populated systems…"));
      else if (this.state === "missing" && !this.job.running) parts.push(this.downloadPrompt());
      else if (this.state === "error") parts.push(this.errorBlock());
      else if (this.state === "ready" && this.layer) parts.push(...this.legend(mode));
    }
    this.body.replaceChildren(...parts.filter((p): p is Node | string => !!p));
    this.body.hidden = parts.length === 0;
  }

  private downloadPrompt(): HTMLElement {
    return h(
      "div",
      "overlay-prompt",
      h(
        "p",
        "small",
        `The coloured layers need Spansh's populated-systems dump: ${DOWNLOAD_SIZES.populated} to download (not kept), a few minutes. The map stays usable meanwhile.`,
      ),
      button("Download", () => void this.startUpdate(), "primary"),
    );
  }

  private errorBlock(): HTMLElement {
    return h(
      "div",
      "overlay-prompt",
      h("p", "problem small", `The populated systems can't be read: ${this.error}`),
      button("Download again", () => void this.startUpdate()),
    );
  }

  private legend(mode: OverlayId): Child[] {
    const legend: Legend = this.style?.legend ?? { kind: "none" };
    const out: Child[] = [];
    switch (legend.kind) {
      case "nominal":
        out.push(
          this.items("nominal", legend.items, (key) => this.toggleHighlight(mode, key), "Click to colour or grey"),
          h("div", "muted small", legend.note),
        );
        break;
      case "ordinal":
        out.push(
          this.items("ordinal", legend.items, (key) => this.toggleHidden(mode, key), "Click to hide or show"),
          h("div", "muted small", legend.note),
        );
        break;
      case "ramp":
        out.push(ramp(legend.colours, legend.ticks), h("div", "muted small", legend.note));
        break;
      case "faction":
        out.push(...this.factionLegend(legend));
        break;
      case "none":
        break;
    }
    if (this.hint) out.push(h("div", "problem small", this.hint));
    const d = this.data;
    const when = d && (day(d.newestUpdate) ?? day(d.sourceLastModified));
    if (when) out.push(h("div", "muted small source", `Spansh, data up to ${when}`));
    return out;
  }

  private items(kind: "nominal" | "ordinal", items: LegendItem[], onClick: (key: string) => void, title: string): HTMLElement {
    const list = h("ul", `legend ${kind}`);
    for (const it of items) {
      const swatch = h("span", "swatch");
      swatch.style.background = it.colour;
      const row = button("", () => onClick(it.key), it.on ? "legend-row on" : "legend-row");
      row.title = title;
      row.setAttribute("aria-pressed", String(it.on));
      row.append(swatch, h("span", "key", it.key), h("span", "count", it.count.toLocaleString()));
      list.append(h("li", undefined, row));
    }
    return list;
  }

  private factionLegend(legend: Extract<Legend, { kind: "faction" }>): Child[] {
    const out: Child[] = [];
    if (legend.faction) {
      const clear = button("×", () => this.pickFaction(null), "icon");
      clear.setAttribute("aria-label", "Clear the faction");
      out.push(h("div", "faction-picked", h("span", "name", legend.faction), clear));
      if (legend.present) {
        out.push(
          h("div", "small", `In ${legend.present.toLocaleString()} systems, controlling ${legend.controls.toLocaleString()}.`),
          ramp(legend.colours, legend.ticks),
          h("div", "muted small", "Colour is influence; bigger dots are systems it controls."),
        );
      } else {
        out.push(h("div", "muted small", "Not present in any populated system."));
      }
    } else {
      out.push(h("div", "muted small", "Pick a faction to see where it is and how strong."));
    }
    out.push(this.factionInput, this.factionResults);
    this.renderFactionResults();
    return out;
  }

  private renderFactionResults(): void {
    const found = this.data ? searchFactions(this.data, this.factionInput.value) : [];
    this.factionActive = Math.min(this.factionActive, Math.max(found.length - 1, 0));
    this.factionResults.replaceChildren(
      ...found.map((f, i) => {
        const b = button("", () => this.pickFaction(f.name), i === this.factionActive ? "active" : "");
        b.append(h("span", "name", f.name), h("span", "count", f.systems.toLocaleString()));
        // Keep the input's focus until the click lands.
        b.addEventListener("mousedown", (e) => e.preventDefault());
        return h("li", undefined, b);
      }),
    );
    this.factionResults.hidden = found.length === 0;
  }
}

function progressText(p: PopulatedProgress | null): string {
  if (!p) return "Connecting to Spansh…";
  const of = p.bytesTotal ? `${gb(p.bytesRead)} of ${gb(p.bytesTotal)}` : gb(p.bytesRead);
  return `Downloading: ${of} · ${p.systems.toLocaleString()} systems · ${clock(p.seconds)}`;
}

function ramp(colours: string[], ticks: Tick[]): HTMLElement {
  const bar = h("div", "ramp-bar");
  bar.style.background = `linear-gradient(to right, ${colours.join(", ")})`;
  const labels = h("div", "ramp-ticks");
  for (const t of ticks) {
    const span = h("span", undefined, t.label);
    span.style.left = `${t.at * 100}%`;
    // Keep the end labels inside the bar's width.
    span.style.transform = t.at <= 0 ? "none" : t.at >= 1 ? "translateX(-100%)" : "translateX(-50%)";
    labels.append(span);
  }
  return h("div", "ramp", bar, labels);
}

