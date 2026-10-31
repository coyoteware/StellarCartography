import { searchSystems, type SystemInfo } from "../api";

/** Debounced prefix search with keyboard navigation. */
export function setupSearch(input: HTMLInputElement, list: HTMLUListElement, onPick: (s: SystemInfo) => void): void {
  let results: SystemInfo[] = [];
  let active = 0;
  let timer = 0;
  let seq = 0;

  const render = () => {
    list.replaceChildren(
      ...results.map((s, i) => {
        const li = document.createElement("li");
        li.className = i === active ? "active" : "";
        const name = document.createElement("div");
        name.textContent = s.name;
        const coords = document.createElement("span");
        coords.textContent = s.pos.map((v) => v.toFixed(0)).join(", ");
        li.append(name, coords);
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
    const s = results[i];
    if (!s) return;
    input.value = s.name;
    results = [];
    render();
    input.blur();
    onPick(s);
  };

  input.addEventListener("input", () => {
    window.clearTimeout(timer);
    const query = input.value;
    timer = window.setTimeout(async () => {
      const mine = ++seq;
      const found = query.trim().length < 2 ? [] : await searchSystems(query).catch(() => []);
      if (mine !== seq) return; // a newer query is in flight
      results = found;
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
