// The galaxy catalogue's lifecycle in the UI: first-run setup (build from
// EDSM or point at an existing build), build progress, and rebuilds started
// from the settings panel while the map stays in use.

import {
  buildStatus,
  cancelBuild,
  onBuildDone,
  onBuildProgress,
  pickFolder,
  startBuild,
  useDataDir,
  type BuildDone,
  type BuildPhase,
  type BuildProgress,
  type BuildStatus,
  type DataStatus,
} from "../api";
import { errorText, type Toasts } from "./toast";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const PHASES: BuildPhase[] = ["reading", "octree", "densityMaps", "indexing"];

const gb = (bytes: number) => (bytes / 2 ** 30).toFixed(2) + " GB";
const millions = (n: number) => (n / 1e6).toFixed(1) + "M";
const clock = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

/** First run shows the build full screen; a rebuild runs in a corner card over the map. */
type Mode = "first-run" | "rebuild";

export class CatalogueUi {
  /** Development only: stop builds after this many systems. */
  testLimit: number | undefined;
  private mode: Mode = "first-run";
  private status: DataStatus | null = null;
  private targetDir = "";
  private readonly setup = $("setup");
  private readonly card = $("build");

  constructor(private readonly toasts: Toasts) {
    $("setup-change").addEventListener("click", async () => {
      const dir = await pickFolder("Where should the galaxy data go?", this.targetDir).catch(() => null);
      if (dir) {
        this.targetDir = dir;
        $("setup-dir").textContent = dir;
      }
    });
    $("setup-build").addEventListener("click", () => void this.start("first-run", this.targetDir));
    $("setup-existing").addEventListener("click", () => void this.useExisting());
    $("build-cancel").addEventListener("click", () => {
      $<HTMLButtonElement>("build-cancel").disabled = true;
      $("build-detail").textContent = "Cancelling…";
      void cancelBuild();
    });
    $("build-retry").addEventListener("click", () => void this.start(this.mode, this.targetDir));
    $("build-close").addEventListener("click", () => {
      this.card.hidden = true;
      if (this.mode === "first-run" && this.status) this.showSetup(this.status);
    });
    void onBuildProgress((p) => this.progress(p));
    void onBuildDone((d) => this.done(d));
  }

  /** No usable data: offer to build it or to use an existing build. */
  showSetup(status: DataStatus): void {
    this.status = status;
    this.targetDir ||= status.dir;
    $("setup-dir").textContent = this.targetDir;
    const problem = $("setup-problem");
    problem.hidden = status.state !== "broken";
    if (status.state === "broken") problem.textContent = `The data in ${status.dir} can't be used: ${status.error}`;
    this.card.hidden = true;
    this.setup.hidden = false;
  }

  /** Picks up a build that was already running (the page was reloaded mid-build). */
  resume(build: BuildStatus, mapShown: boolean): void {
    this.targetDir = build.dir;
    this.showBuild(mapShown ? "rebuild" : "first-run");
    if (build.progress) this.progress(build.progress);
  }

  /** Rebuilds the current catalogue from the latest dump. */
  rebuild(): Promise<void> {
    return this.start("rebuild");
  }

  async running(): Promise<boolean> {
    return (await buildStatus().catch(() => null)) !== null;
  }

  private async start(mode: Mode, dir?: string): Promise<void> {
    try {
      this.targetDir = await startBuild(dir || undefined, this.testLimit);
      this.showBuild(mode);
    } catch (e) {
      this.toasts.error(`Couldn't start the build: ${errorText(e)}`);
    }
  }

  private async useExisting(): Promise<void> {
    const dir = await pickFolder("Choose a folder with StellarCartography data").catch(() => null);
    if (!dir) return;
    try {
      await useDataDir(dir);
      location.reload();
    } catch (e) {
      const problem = $("setup-problem");
      problem.textContent = errorText(e);
      problem.hidden = false;
    }
  }

  private showBuild(mode: Mode): void {
    this.mode = mode;
    this.setup.hidden = true;
    this.card.classList.toggle("floating", mode === "rebuild");
    $("build-title").textContent = mode === "rebuild" ? "Rebuilding the galaxy catalogue" : "Building the galaxy catalogue";
    $("build-error").hidden = true;
    $<HTMLButtonElement>("build-cancel").disabled = false;
    $("build-cancel").hidden = false;
    $("build-retry").hidden = true;
    $("build-close").hidden = true;
    $("build-detail").textContent = "Connecting to EDSM…";
    this.setBar(0, false);
    this.markPhase(-1);
    this.card.hidden = false;
  }

  private progress(p: BuildProgress): void {
    if (this.card.hidden) this.showBuild(this.mode);
    const phase = PHASES.indexOf(p.phase);
    this.markPhase(phase);
    if (p.phase === "reading") {
      const f = p.bytesTotal ? p.bytesRead / p.bytesTotal : 0;
      this.setBar(f, false);
      const parts = [
        p.bytesTotal ? `${gb(p.bytesRead)} of ${gb(p.bytesTotal)}` : gb(p.bytesRead),
        `${millions(p.records)} systems`,
        `${clock(p.seconds)} elapsed`,
      ];
      if (f > 0.02) parts.push(`about ${Math.max(1, Math.round((p.seconds * (1 - f)) / f / 60))} min to go`);
      $("build-detail").textContent = parts.join(" · ");
    } else {
      this.setBar(1, true);
      $("build-detail").textContent = `${millions(p.records)} systems read · ${clock(p.seconds)} elapsed`;
    }
  }

  private done(d: BuildDone): void {
    if (d.ok) {
      const minutes = d.seconds ? ` in ${Math.max(1, Math.round(d.seconds / 60))} min` : "";
      this.toasts.show(`Galaxy catalogue built: ${millions(d.systems ?? 0)} systems${minutes}. Reloading…`);
      window.setTimeout(() => location.reload(), 1500);
      return;
    }
    if (d.cancelled) {
      this.card.hidden = true;
      if (this.mode === "first-run" && this.status) this.showSetup(this.status);
      else this.toasts.show("Rebuild cancelled; the existing catalogue is unchanged.");
      return;
    }
    if (this.card.hidden) this.showBuild(this.mode);
    const error = $("build-error");
    error.textContent = `The build failed: ${d.error ?? "unknown error"}`;
    error.hidden = false;
    this.setBar(0, false);
    $("build-detail").textContent = this.mode === "rebuild" ? "The existing catalogue is unchanged." : "";
    $("build-cancel").hidden = true;
    $("build-retry").hidden = false;
    $("build-close").hidden = false;
  }

  private markPhase(active: number): void {
    $("build-steps")
      .querySelectorAll("li")
      .forEach((li, i) => {
        li.classList.toggle("done", i < active);
        li.classList.toggle("active", i === active);
      });
  }

  private setBar(fraction: number, indeterminate: boolean): void {
    const bar = $("build-bar");
    bar.style.width = `${Math.round(fraction * 1000) / 10}%`;
    bar.classList.toggle("indeterminate", indeterminate);
  }
}
