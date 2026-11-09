// A download the backend runs on a worker thread (the populated systems,
// the points of interest): whether it's running, its progress line with a
// Cancel button, and starting it, picking it up after a reload, and hearing
// that it's done.
//
// The worker reports through events, and the reply to the invoke that
// starts it travels separately, so they can arrive in either order: a
// download that fails at once can report "done" before the reply to "start"
// comes back. `start` therefore counts the job as running before it asks,
// and only an error from the invoke itself (no job was started) undoes that.

import type { UnlistenFn } from "@tauri-apps/api/event";
import { button, h } from "./dom";

/** Shown before a download: the dumps' sizes as of November 2026. */
export const DOWNLOAD_SIZES = {
  populated: "about 4.7 GB",
  poi: "about 900 MB",
} as const;

export interface JobDone {
  ok: boolean;
  cancelled: boolean;
  error: string | null;
}

export interface DownloadJobOptions<P, D extends JobDone> {
  start: () => Promise<unknown>;
  cancel: () => Promise<unknown>;
  /** Non-null while a download runs. */
  status: () => Promise<{ progress: P | null } | null>;
  onProgressEvent: (handler: (p: P) => void) => Promise<UnlistenFn>;
  onDoneEvent: (handler: (d: D) => void) => Promise<UnlistenFn>;
  /** The progress line: what's being downloaded and how far along. */
  describe: (p: P | null) => string;
  /** 0..1 for the meter; null when the total isn't known. */
  fraction: (p: P | null) => number | null;
  /** It started or stopped: redraw whatever shows it. */
  onRunningChange: () => void;
  /** The progress line changed (a status line elsewhere may repeat it). */
  onProgress: () => void;
  onDone: (d: D) => void;
  /** The backend refused to start it; nothing is running. */
  onStartError: (e: unknown) => void;
}

export class DownloadJob<P, D extends JobDone> {
  running = false;
  progress: P | null = null;
  /** Progress line, meter and Cancel button; kept across redraws so ticks don't rebuild them. */
  readonly element: HTMLElement;
  private readonly line = h("div", "small");
  private readonly fill = h("div");
  private readonly cancelButton: HTMLButtonElement;
  /** Done events heard so far: a status reply older than the last one is stale. */
  private finished = 0;

  constructor(private readonly o: DownloadJobOptions<P, D>) {
    this.cancelButton = button("Cancel", () => {
      this.cancelButton.disabled = true;
      void o.cancel();
    });
    this.element = h("div", "overlay-progress", this.line, h("div", "meter", this.fill), this.cancelButton);
    void o.onProgressEvent((p) => {
      this.progress = p;
      if (this.running) return this.refresh();
      // Running before this page loaded, and not yet picked up by `resume`.
      this.cancelButton.disabled = false;
      this.setRunning(true);
    });
    void o.onDoneEvent((d) => {
      this.finished++;
      this.progress = null;
      this.setRunning(false);
      o.onDone(d);
    });
  }

  /** Starts the download unless one is running. */
  async start(): Promise<void> {
    if (this.running) return;
    this.progress = null;
    this.cancelButton.disabled = false;
    // Running from here: the job's own events may beat the reply below.
    this.setRunning(true);
    try {
      await this.o.start();
    } catch (e) {
      // Refused, so no job exists and no done event will come.
      this.setRunning(false);
      this.o.onStartError(e);
    }
  }

  /** Picks up a download already running, e.g. after a reload. */
  async resume(): Promise<void> {
    const finished = this.finished;
    const status = await this.o.status().catch(() => null);
    // A done event while asking means the answer is out of date.
    if (!status || this.running || finished !== this.finished) return;
    this.progress = status.progress;
    this.cancelButton.disabled = false;
    this.setRunning(true);
  }

  text(): string {
    return this.o.describe(this.progress);
  }

  /** The progress block, brought up to date. */
  block(): HTMLElement {
    this.refresh();
    return this.element;
  }

  private refresh(): void {
    this.line.textContent = this.text();
    const f = this.o.fraction(this.progress) ?? 0;
    this.fill.style.width = `${(Math.min(Math.max(f, 0), 1) * 100).toFixed(1)}%`;
    this.o.onProgress();
  }

  private setRunning(running: boolean): void {
    this.running = running;
    this.o.onRunningChange();
  }
}
