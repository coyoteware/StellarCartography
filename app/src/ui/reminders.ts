// Weekly update reminders: when the galaxy catalogue, the populated systems
// or the points of interest are over a week old, a toast offers to refresh
// them. Nothing is
// downloaded without a click. Checked a minute after start, then hourly
// (the viewer may stay open for days), at most once a day per dataset.

import type { Toasts } from "./toast";

const DAY = 86_400_000;
const WEEK = 7 * DAY;

export interface ReminderDeps {
  toasts: Toasts;
  enabled: () => boolean;
  /** Unix seconds, or null when unknown or not downloaded. */
  catalogueBuiltAt: () => number | null;
  populatedBuiltAt: () => number | null;
  poiBuiltAt: () => number | null;
  /** True while a build or download runs. */
  busy: () => Promise<boolean>;
  rebuildCatalogue: () => void;
  updatePopulated: () => void;
  updatePoi: () => void;
}

type Dataset = "catalogue" | "populated" | "poi";

export class Reminders {
  private readonly shown = new Map<Dataset, number>();
  private started = false;

  constructor(private readonly deps: ReminderDeps) {}

  start(): void {
    if (this.started) return;
    this.started = true;
    window.setTimeout(() => void this.check(), 60_000);
    window.setInterval(() => void this.check(), 3_600_000);
  }

  async check(): Promise<void> {
    if (!this.deps.enabled() || (await this.deps.busy())) return;
    this.remind(
      "catalogue",
      this.deps.catalogueBuiltAt(),
      (days) =>
        `The galaxy catalogue is ${days} days old. Rebuilding downloads about 3.4 GB and takes 10–15 minutes; the map stays usable.`,
      "Rebuild",
      this.deps.rebuildCatalogue,
    );
    this.remind(
      "populated",
      this.deps.populatedBuiltAt(),
      (days) => `The map layers' data is ${days} days old. Updating downloads about 4.7 GB and takes about 5 minutes.`,
      "Update",
      this.deps.updatePopulated,
    );
    this.remind(
      "poi",
      this.deps.poiBuiltAt(),
      (days) => `The points of interest are ${days} days old. Updating downloads about 900 MB and takes a few minutes.`,
      "Update",
      this.deps.updatePoi,
    );
  }

  private remind(dataset: Dataset, builtAt: number | null, text: (days: number) => string, label: string, run: () => void): void {
    if (builtAt == null) return;
    const now = Date.now();
    const age = now - builtAt * 1000;
    if (age < WEEK) return;
    const last = this.shown.get(dataset);
    if (last !== undefined && now - last < DAY) return;
    this.shown.set(dataset, now);
    this.deps.toasts.show(text(Math.floor(age / DAY)), "info", { timeout: 0, action: { label, run } });
  }
}
