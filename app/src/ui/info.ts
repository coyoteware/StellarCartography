import { massCode, type SystemInfo, type Vec3 } from "../api";
import { SOL, distance } from "../renderer/coords";

const fmt = (v: number) => v.toLocaleString(undefined, { maximumFractionDigits: 2 });

/** Fills the info panel for a system. `commanderPos` adds a distance-from-you row. */
export function showSystem(panel: HTMLElement, system: SystemInfo, commanderPos?: Vec3): void {
  const rows: [string, string][] = [
    ["Coordinates", system.pos.map((v) => v.toFixed(3)).join(", ")],
    ["From Sol", `${fmt(distance(system.pos, SOL))} ly`],
  ];
  if (commanderPos) rows.push(["From you", `${fmt(distance(system.pos, commanderPos))} ly`]);
  if (system.id64) {
    rows.push(["id64", system.id64]);
    rows.push(["Mass code", massCode(system.id64)]);
  }
  const h2 = document.createElement("h2");
  h2.textContent = system.name;
  const dl = document.createElement("dl");
  for (const [k, v] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    dl.append(dt, dd);
  }
  panel.replaceChildren(h2, dl);
  panel.hidden = false;
}
