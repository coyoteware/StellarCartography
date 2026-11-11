// System details panel. Shows the basics as soon as a star is picked, then
// fills in population, Powerplay, factions, stations and bodies from the
// journals and online sources, each section labelled with where it came from
// and how old it is. Points of interest in the system are listed straight
// away, from the downloaded catalogue. The system can be marked as the home
// system or squadron capital, for those jump buttons.

import {
  massCode,
  openExternal,
  systemDetails,
  type ConflictInfo,
  type DetailsSource,
  type FactionInfo,
  type Sourced,
  type StationInfo,
  type SystemDetails,
  type SystemInfo,
  type Vec3,
  type VisitedSystem,
} from "../api";
import { categoryInfo, glyph, type Poi } from "../poi";
import { SOL, distance } from "../renderer/coords";
import { BOOKMARKS, BOOKMARK_NAMES, type Bookmarks } from "./bookmarks";
import { h, type Child } from "./dom";
import { errorText } from "./toast";

const SOURCE_NAMES: Record<DetailsSource, string> = { journal: "your journal", edsm: "EDSM", spansh: "Spansh" };

/** Days between 1601-01-01 (VisitedStarsCache dates) and 1970-01-01. */
const DAYS_1601_TO_1970 = 134_774;

const num = (v: number, digits = 0) => v.toLocaleString(undefined, { maximumFractionDigits: digits });
const pct = (v: number) => `${(v * 100).toFixed(1)}%`;

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

/** "today", "yesterday", "3 weeks ago"... for a count of whole days. */
function daysAgo(days: number): string {
  if (days <= 0) return "today";
  if (days < 14) return rtf.format(-days, "day");
  if (days < 60) return rtf.format(-Math.round(days / 7), "week");
  if (days < 730) return rtf.format(-Math.round(days / 30.44), "month");
  return rtf.format(-Math.round(days / 365.25), "year");
}

/** "3 days ago", "last month"... for an ISO timestamp. */
function ago(iso: string | null): string {
  if (!iso) return "date unknown";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "date unknown";
  const seconds = (t - Date.now()) / 1000;
  const steps: [Intl.RelativeTimeFormatUnit, number][] = [
    ["year", 365 * 86400],
    ["month", 30 * 86400],
    ["day", 86400],
    ["hour", 3600],
    ["minute", 60],
  ];
  for (const [unit, size] of steps) {
    if (Math.abs(seconds) >= size) return rtf.format(Math.round(seconds / size), unit);
  }
  return "just now";
}

function rows(pairs: [string, string | null | undefined][]): HTMLDListElement {
  const dl = h("dl");
  for (const [k, v] of pairs) if (v) dl.append(h("dt", undefined, k), h("dd", undefined, v));
  return dl;
}

function chip(text: string, cls = ""): HTMLSpanElement {
  return h("span", `chip ${cls}`.trim(), text);
}

function bar(fraction: number, cls = ""): HTMLDivElement {
  const fill = h("div");
  fill.style.width = `${Math.max(0, Math.min(1, fraction)) * 100}%`;
  return h("div", `meter ${cls}`.trim(), fill);
}

function stamp(s: Sourced): HTMLSpanElement {
  const span = h("span", "stamp", `${SOURCE_NAMES[s.source]}, ${ago(s.asOf)}`);
  if (s.asOf) span.title = new Date(s.asOf).toLocaleString();
  return span;
}

function section(title: string, meta: Child, ...body: Child[]): HTMLElement {
  return h("section", undefined, h("h3", undefined, title, meta), ...body);
}

export interface PanelDeps {
  /** Current commander position, for a "from you" distance. */
  commanderPos: () => Vec3 | undefined;
  /** This system's entry in the game's visited-stars cache. */
  visited: (id64: string) => VisitedSystem | undefined;
  /** Points of interest in the system. */
  places: (id64: string | null, name: string) => Poi[];
  /** The systems behind the Home and Squadron jump buttons. */
  bookmarks: Bookmarks;
  onClose: () => void;
  onError: (message: string) => void;
}

export class SystemPanel {
  private system: SystemInfo | null = null;
  private seq = 0;
  /** A load is in flight; a quiet reload asked for meanwhile waits for it. */
  private loading = false;
  private reloadAfter = false;
  /** The "Mark as" toggles of the system on show. */
  private readonly marks = h("div", "segs bookmark-toggles");

  constructor(
    private readonly root: HTMLElement,
    private readonly deps: PanelDeps,
  ) {
    document.addEventListener("keydown", (e) => {
      // Search uses Escape to clear its list, and Settings and the key list close themselves first.
      const cardOpen = document.querySelector("#settings:not([hidden]), #keys:not([hidden])") !== null;
      if (e.key === "Escape" && !e.defaultPrevented && !cardOpen && !this.root.hidden) this.close();
    });
  }

  get shown(): SystemInfo | null {
    return this.system;
  }

  show(system: SystemInfo): void {
    this.system = system;
    this.reloadAfter = false;
    this.root.hidden = false;
    this.root.scrollTop = 0;
    void this.load(false);
  }

  /** Reloads quietly if `id64` is the system on show (e.g. the commander just arrived there). */
  refreshIfShowing(id64: string | null): void {
    if (!id64 || this.system?.id64 !== id64) return;
    // Don't overtake a load in flight (it may be a refresh the user asked
    // for); reload from what it leaves in the cache once it lands.
    if (this.loading) this.reloadAfter = true;
    else void this.load(false, true);
  }

  /** Redraws the "Mark as" toggles (the jump buttons changed). */
  syncBookmarks(): void {
    const system = this.system;
    if (!system) return;
    this.marks.replaceChildren(
      ...BOOKMARKS.map((slot) => {
        const { button, long } = BOOKMARK_NAMES[slot];
        const on = this.deps.bookmarks.is(slot, system);
        const b = h("button", undefined, button);
        b.type = "button";
        b.setAttribute("aria-pressed", String(on));
        b.title = on ? `Your ${long}. Click to clear it.` : `Make this your ${long}`;
        b.addEventListener("click", () => void this.deps.bookmarks.set(slot, on ? null : system));
        return b;
      }),
    );
  }

  close(): void {
    this.seq++;
    this.loading = this.reloadAfter = false;
    this.system = null;
    this.root.hidden = true;
    this.deps.onClose();
  }

  private async load(refresh: boolean, quiet = false): Promise<void> {
    const system = this.system;
    if (!system) return;
    const mine = ++this.seq;
    this.loading = true;
    if (!quiet) this.render(system, { loading: true });
    try {
      const details = await systemDetails(system.id64, system.name, refresh);
      if (mine === this.seq) this.render(system, { details });
    } catch (e) {
      if (mine === this.seq) this.render(system, { error: errorText(e) });
    } finally {
      if (mine === this.seq) {
        this.loading = false;
        if (this.reloadAfter) {
          this.reloadAfter = false;
          void this.load(false, true);
        }
      }
    }
  }

  private render(system: SystemInfo, state: { loading?: boolean; details?: SystemDetails; error?: string }): void {
    const d = state.details;
    const o = d?.overview;

    const refresh = h("button", "icon", "↻");
    refresh.type = "button";
    refresh.title = "Refresh from the online sources";
    refresh.disabled = !!state.loading;
    refresh.addEventListener("click", () => void this.load(true));
    const close = h("button", "icon close", "×");
    close.type = "button";
    close.setAttribute("aria-label", "Close");
    close.addEventListener("click", () => this.close());

    const tagline = o
      ? [o.allegiance, o.government, o.population ? `${num(o.population)} people` : "Unpopulated"].filter(Boolean).join(" · ")
      : state.loading
        ? "Loading details…"
        : "";
    const head = h(
      "header",
      undefined,
      h("div", "title", h("h2", undefined, system.name), tagline && h("div", "tagline", tagline)),
      h("div", "head-buttons", refresh, close),
    );

    this.syncBookmarks();
    const parts: Child[] = [head, this.places(system), this.overview(system, d)];
    if (state.error) {
      const retry = h("button", undefined, "Try again");
      retry.type = "button";
      retry.addEventListener("click", () => void this.load(false));
      parts.push(h("p", "problem", `Couldn't load the details: ${state.error}`), retry);
    } else if (d) {
      parts.push(
        this.powerplay(d),
        this.factions(d),
        this.stations(d),
        this.bodies(d),
        this.visits(system),
        this.footer(d),
      );
    } else if (state.loading) {
      parts.push(h("p", "muted loading", "Looking up factions, Powerplay and stations…"));
    }
    this.root.replaceChildren(...parts.filter((p): p is Node | string => !!p));
  }

  private places(system: SystemInfo): HTMLElement | null {
    const places = this.deps.places(system.id64, system.name);
    if (places.length === 0) return null;
    const list = h("ul", "places");
    for (const p of places) {
      const info = categoryInfo(p.category);
      // Codex sites and carrier hubs are named after their system: say what they are instead.
      const named = p.name.toLowerCase() !== system.name.toLowerCase();
      const li = h(
        "li",
        "place",
        h("div", "row", glyph(info.shape), h("span", "name", named ? p.name : p.kind)),
        named && h("div", "muted small", p.kind),
        p.summary && h("div", "small", p.summary),
      );
      if (p.url) li.append(this.link("More on EDAstro", p.url));
      list.append(li);
    }
    return section(`Points of interest (${places.length})`, h("span", "stamp", "EDAstro"), list);
  }

  /** A link opened in the default browser (the backend only opens known sites). */
  private link(label: string, url: string): HTMLAnchorElement {
    const a = h("a", undefined, label);
    a.href = url;
    a.addEventListener("click", (e) => {
      e.preventDefault();
      openExternal(url).catch((err) => this.deps.onError(`Couldn't open the link: ${errorText(err)}`));
    });
    return a;
  }

  private overview(system: SystemInfo, d: SystemDetails | undefined): HTMLElement {
    const o = d?.overview;
    const you = this.deps.commanderPos();
    const economy = o?.economy && o.secondEconomy && o.secondEconomy !== "None" ? `${o.economy} / ${o.secondEconomy}` : o?.economy;
    const faction = o?.controllingFaction
      ? o.controllingFactionState && o.controllingFactionState !== "None"
        ? `${o.controllingFaction} (${o.controllingFactionState})`
        : o.controllingFaction
      : null;
    return section(
      "System",
      o && stamp(o),
      rows([
        ["Allegiance", o?.allegiance],
        ["Government", o?.government],
        ["Economy", economy],
        ["Security", o?.security],
        ["Population", o?.population != null ? num(o.population) : null],
        ["Controlling faction", faction],
        ["Permit", o?.permit],
        ["Coordinates", system.pos.map((v) => v.toFixed(2)).join(", ")],
        ["From Sol", `${num(distance(system.pos, SOL), 2)} ly`],
        ["From you", you ? `${num(distance(system.pos, you), 2)} ly` : null],
        ["id64", system.id64 ? `${system.id64} (mass code ${massCode(system.id64)})` : null],
      ]),
      h("div", "bookmark-row", h("span", "label", "Mark as"), this.marks),
    );
  }

  private powerplay(d: SystemDetails): HTMLElement | null {
    const p = d.powerplay;
    if (!p || (!p.controllingPower && p.powers.length === 0 && p.conflict.length === 0 && !p.state)) return null;
    const lines: Child[] = [];
    if (p.controllingPower) {
      lines.push(
        h("div", "power-line", h("span", "power-name", p.controllingPower), p.state && chip(p.state, "state")),
      );
    } else if (p.state) {
      lines.push(h("div", "power-line", chip(p.state, "state")));
    }
    if (p.controlProgress != null) {
      lines.push(h("div", "meter-row", h("span", "muted", "Control progress"), bar(p.controlProgress), h("span", undefined, pct(p.controlProgress))));
    }
    if (p.reinforcement != null || p.undermining != null) {
      lines.push(
        rows([
          ["Reinforcement", p.reinforcement != null ? num(p.reinforcement) : null],
          ["Undermining", p.undermining != null ? num(p.undermining) : null],
        ]),
      );
    }
    if (p.conflict.length) {
      const list = h("ul", "power-conflict");
      for (const c of [...p.conflict].sort((a, b) => b.progress - a.progress)) {
        list.append(h("li", "meter-row", h("span", undefined, c.power), bar(c.progress), h("span", undefined, pct(c.progress))));
      }
      lines.push(h("div", "muted small", "Contested by"), list);
    }
    const others = p.powers.filter((x) => x !== p.controllingPower && !p.conflict.some((c) => c.power === x));
    if (others.length) lines.push(h("div", "small", h("span", "muted", "Also present: "), others.join(", ")));
    return section("Powerplay", stamp(p), ...lines);
  }

  private factions(d: SystemDetails): HTMLElement | null {
    const f = d.factions;
    if (!f || f.factions.length === 0) return null;
    const list = h("ul", "factions");
    const sorted = [...f.factions].sort((a, b) => (b.influence ?? 0) - (a.influence ?? 0));
    for (const faction of sorted) list.append(factionRow(faction));
    const conflicts = f.conflicts.length > 0 && h("ul", "conflicts", ...f.conflicts.map(conflictRow));
    return section(`Factions (${f.factions.length})`, stamp(f), conflicts, list);
  }

  private stations(d: SystemDetails): HTMLElement | null {
    if (d.stations.length === 0) return null;
    const isCarrier = (s: StationInfo) => s.kind === "Fleet carrier";
    const byDistance = (a: StationInfo, b: StationInfo) => (a.distanceLs ?? Infinity) - (b.distanceLs ?? Infinity);
    const ports = d.stations.filter((s) => !isCarrier(s)).sort(byDistance);
    const carriers = d.stations.filter(isCarrier).sort(byDistance);
    const body: Child[] = [];
    if (ports.length) {
      const list = h("ul", "stations");
      for (const s of ports) list.append(stationRow(s));
      body.push(list);
    }
    if (carriers.length) {
      const list = h("ul", "stations");
      for (const s of carriers) list.append(stationRow(s));
      const more = h("details", "carriers", h("summary", undefined, `Fleet carriers (${carriers.length})`), list);
      body.push(more);
    }
    const title = `Stations (${ports.length}${carriers.length ? ` + ${carriers.length} carriers` : ""})`;
    return section(title, null, ...body);
  }

  private bodies(d: SystemDetails): HTMLElement | null {
    const b = d.bodies;
    if (!b || (b.count == null && !b.primaryStar)) return null;
    const star = b.primaryStar ? `${b.primaryStar}${b.scoopable ? " (scoopable)" : b.scoopable === false ? " (not scoopable)" : ""}` : null;
    return section("Bodies", stamp(b), rows([["Main star", star], ["Bodies", b.count != null ? num(b.count) : null]]));
  }

  private visits(system: SystemInfo): HTMLElement | null {
    const v = system.id64 ? this.deps.visited(system.id64) : undefined;
    if (!v) return null;
    // The cache only stores a calendar day: show that day as is (it's UTC
    // midnight here, so format in UTC) and count whole days back.
    const last = new Date((v.lastVisitDay - DAYS_1601_TO_1970) * 86_400_000);
    const today = Math.floor(Date.now() / 86_400_000) + DAYS_1601_TO_1970;
    return section(
      "Your visits",
      null,
      rows([
        ["Visits", num(v.visits)],
        ["Last visit", `${last.toLocaleDateString(undefined, { timeZone: "UTC" })} (${daysAgo(today - v.lastVisitDay)})`],
      ]),
    );
  }

  private footer(d: SystemDetails): HTMLElement {
    const notes: Child[] = [];
    for (const r of d.remote) {
      if (r.missing) notes.push(h("div", "muted small", `${SOURCE_NAMES[r.source]} has no record of this system.`));
      else if (!r.ok) notes.push(h("div", "problem small", `${SOURCE_NAMES[r.source]} unavailable: ${r.error ?? "unknown error"}`));
    }
    const nothing = !d.overview && !d.factions && !d.powerplay && d.stations.length === 0 && !d.bodies;
    if (nothing && d.remote.every((r) => r.ok)) notes.push(h("div", "muted small", "No details recorded for this system."));
    const links = h("div", "links", ...d.links.map((link) => this.link(link.label, link.url)));
    return h("footer", undefined, ...notes, links);
  }
}

function factionRow(f: FactionInfo): HTMLLIElement {
  const name = h("span", "name", f.controlling ? "★ " : "", f.name);
  if (f.controlling) name.title = "Controlling faction";
  const meta = [f.government, f.allegiance, f.happiness].filter(Boolean).join(" · ");
  const chips = h("div", "chips");
  for (const s of f.activeStates) chips.append(chip(s, "active"));
  for (const s of f.pendingStates) chips.append(chip(`${s} (pending)`, "pending"));
  for (const s of f.recoveringStates) chips.append(chip(`${s} (recovering)`, "recovering"));
  if (chips.childElementCount === 0 && f.state && f.state !== "None") chips.append(chip(f.state, "active"));
  return h(
    "li",
    f.controlling ? "faction controlling" : "faction",
    h("div", "row", name, f.influence != null && h("span", "value", pct(f.influence))),
    f.influence != null && bar(f.influence),
    meta && h("div", "muted small", meta),
    f.myReputation != null && h("div", "muted small", `Your reputation: ${num(f.myReputation, 1)}`),
    chips.childElementCount > 0 && chips,
  );
}

function conflictRow(c: ConflictInfo): HTMLLIElement {
  const title = [c.kind, c.status && c.status !== "Active" ? `(${c.status.toLowerCase()})` : ""].filter(Boolean).join(" ");
  const sides = c.sides.map((s) => {
    const days = s.wonDays != null ? ` ${s.wonDays} ${s.wonDays === 1 ? "day" : "days"}` : "";
    return h("div", "side", h("span", "name", s.faction), h("span", "value", days.trim()), s.stake && h("div", "muted small", `Stake: ${s.stake}`));
  });
  return h("li", "conflict", h("div", "conflict-title", title), ...sides);
}

function stationRow(s: StationInfo): HTMLLIElement {
  const pad = h("span", `pad ${s.maxPad ? `pad-${s.maxPad}` : "pad-unknown"}`, s.maxPad ?? "?");
  pad.title = s.maxPad ? `Largest landing pad: ${s.maxPad}` : "Landing pads unknown";
  const meta = [s.kind, s.economy, s.faction].filter(Boolean).join(" · ");
  const li = h(
    "li",
    "station",
    h("div", "row", pad, h("span", "name", s.name), s.distanceLs != null && h("span", "value", `${num(s.distanceLs)} ls`)),
    meta && h("div", "muted small", meta),
    s.services.length > 0 && h("div", "chips", ...s.services.map((x) => chip(x))),
  );
  li.title = s.asOf ? `From ${SOURCE_NAMES[s.source]}, ${ago(s.asOf)}` : `From ${SOURCE_NAMES[s.source]} (not dated)`;
  return li;
}
