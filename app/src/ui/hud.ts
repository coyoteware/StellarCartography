import type { CloudStats } from "../renderer/pointcloud";
import type { Vec3 } from "../api";

const BYTES_PER_POINT = 11; // tile layout: u32 rowid + 3x u16 position + u8 mass code

const m = (n: number) => (n / 1e6).toFixed(2) + "M";

/** Bottom-left readout: frame rate, LOD stats, camera target. */
export class Hud {
  private fps = 60;
  private lastText = 0;

  constructor(private readonly el: HTMLElement) {}

  update(now: number, dt: number, stats: CloudStats | null, totalPoints: number, target: Vec3, distance: number): void {
    if (dt > 0) this.fps = this.fps * 0.95 + (1 / dt) * 0.05;
    if (now - this.lastText < 0.25) return;
    this.lastText = now;
    const lines = [`${this.fps.toFixed(0)} fps`];
    if (stats) {
      lines.push(
        `drawn   ${m(stats.visiblePoints)} pts in ${stats.visibleNodes} tiles`,
        `loaded  ${m(stats.loadedPoints)} pts (${((stats.loadedPoints * BYTES_PER_POINT) / 2 ** 20).toFixed(0)} MB) of ${m(totalPoints)}` +
          (stats.loading ? `, ${stats.loading} loading` : ""),
      );
    }
    lines.push(`target  ${target.map((v) => v.toFixed(0)).join(", ")}  ·  range ${distance.toFixed(0)} ly`);
    this.el.textContent = lines.join("\n");
  }
}
