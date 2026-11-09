import { searchSystems, type SystemInfo } from "../api";
import { categoryInfo, glyph, type Poi } from "../poi";
import { describePoi } from "./places";

type Result = { kind: "system"; system: SystemInfo } | { kind: "poi"; poi: Poi };

/** Points of interest listed above the systems. */
const MAX_PLACES = 5;

export interface SearchDeps {
  onPickSystem: (s: SystemInfo) => void;
  onPickPoi: (p: Poi) => void;
  /** Points of interest matching a query, best first. */
  places: (query: string, limit: number) => Poi[];
  onError: (e: unknown) => void;
}

/** Debounced prefix search over systems and places, with keyboard navigation. */
export function setupSearch(input: HTMLInputElement, list: HTMLUListElement, deps: SearchDeps): void {
  let results: Result[] = [];
  let active = 0;
  let timer = 0;
  let seq = 0;

  const render = () => {
    list.replaceChildren(
      ...results.map((r, i) => {
        const li = document.createElement("li");
        li.className = i === active ? "active" : "";
        const name = document.createElement("div");
        const meta = document.createElement("span");
        if (r.kind === "system") {
          name.textContent = r.system.name;
          meta.textContent = r.system.pos.map((v) => v.toFixed(0)).join(", ");
        } else {
          li.classList.add("place");
          name.append(glyph(categoryInfo(r.poi.category).shape), ` ${r.poi.name}`);
          meta.textContent = describePoi(r.poi);
        }
        li.append(name, meta);
        li.addEventListener("mousedown", (e) => {
          e.preventDefault();
          pick(i);
        });
        return li;
      }),
    );
    list.hidden = results.length === 0;
  };

  const pick = (i: number) => {
    const r = results[i];
    if (!r) return;
    input.value = r.kind === "system" ? r.system.name : r.poi.name;
    results = [];
    render();
    input.blur();
    if (r.kind === "system") deps.onPickSystem(r.system);
    else deps.onPickPoi(r.poi);
  };

  input.addEventListener("input", () => {
    window.clearTimeout(timer);
    const query = input.value;
    timer = window.setTimeout(async () => {
      const mine = ++seq;
      const short = query.trim().length < 2;
      const places = short ? [] : deps.places(query, MAX_PLACES);
      const systems = short
        ? []
        : await searchSystems(query).catch((e) => {
            deps.onError(e);
            return [];
          });
      if (mine !== seq) return; // a newer query is in flight
      results = [...places.map((poi): Result => ({ kind: "poi", poi })), ...systems.map((system): Result => ({ kind: "system", system }))];
      active = 0;
      render();
    }, 120);
  });

  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") active = Math.min(active + 1, results.length - 1);
    else if (e.key === "ArrowUp") active = Math.max(active - 1, 0);
    else if (e.key === "Enter") pick(active);
    else if (e.key === "Escape") results = [];
    else return;
    e.preventDefault();
    render();
  });

  input.addEventListener("blur", () => {
    results = [];
    render();
  });
}
