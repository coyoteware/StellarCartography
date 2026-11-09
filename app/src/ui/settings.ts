// Settings panel: catalogue folder and rebuilds, journal folder, the
// populated systems behind the overlay layers, and display preferences
// (point budget, brightness).

import {
  datasetInfo,
  getSettings,
  pickFolder,
  setDetailsSource,
  setJournalDir,
  useDataDir,
  type DataStatus,
  type DirOrigin,
  type OnlineSource,
  type Settings,
  type ViewSettings,
} from "../api";
import { errorText, type Toasts } from "./toast";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const ORIGIN_NOTES: Record<DirOrigin, string> = {
  environment: "Set by the STELLAR_DATA_DIR environment variable.",
  settings: "",
  found: "Found next to the app.",
  default: "The app's default data folder.",
};

export interface SettingsDeps {
  toasts: Toasts;
  view: () => ViewSettings;
  onView: (view: ViewSettings) => void;
  onRebuild: () => Promise<void>;
  buildRunning: () => Promise<boolean>;
  /** Status line for the populated systems. */
  populatedStatus: () => string;
  populatedRunning: () => boolean;
  onPopulatedUpdate: () => void;
}

export class SettingsPanel {
  private readonly el = $("settings");
  private settings: Settings | null = null;

  constructor(private readonly deps: SettingsDeps) {
    $("settings-close").addEventListener("click", () => this.close());
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !this.el.hidden) this.close();
    });

    $("set-data-change").addEventListener("click", () => void this.changeDataDir());
    $("set-rebuild").addEventListener("click", async () => {
      if (await this.deps.buildRunning()) return this.deps.toasts.show("A build is already running.");
      $("set-rebuild-confirm").hidden = false;
    });
    $("set-rebuild-cancel").addEventListener("click", () => ($("set-rebuild-confirm").hidden = true));
    $("set-rebuild-go").addEventListener("click", () => {
      $("set-rebuild-confirm").hidden = true;
      this.close();
      void this.deps.onRebuild();
    });

    $("set-populated-update").addEventListener("click", () => {
      if (this.deps.populatedRunning()) return this.deps.toasts.show("The populated systems are already downloading.");
      $("set-populated-confirm").hidden = false;
    });
    $("set-populated-cancel").addEventListener("click", () => ($("set-populated-confirm").hidden = true));
    $("set-populated-go").addEventListener("click", () => {
      $("set-populated-confirm").hidden = true;
      this.deps.onPopulatedUpdate();
      this.renderPopulated();
    });

    const detailsSource = $<HTMLSelectElement>("set-details-source");
    detailsSource.addEventListener("change", () => {
      setDetailsSource(detailsSource.value as OnlineSource).catch((e) =>
        this.deps.toasts.error(`Couldn't save the setting: ${errorText(e)}`),
      );
    });

    $("set-journal-change").addEventListener("click", () => void this.changeJournalDir());
    $("set-journal-reset").addEventListener("click", () => void this.applyJournalDir(null));

    const budget = $<HTMLSelectElement>("set-budget");
    budget.addEventListener("change", () => this.deps.onView({ ...this.deps.view(), pointBudget: Number(budget.value) }));
    const exposure = $<HTMLInputElement>("set-exposure");
    exposure.addEventListener("input", () => this.deps.onView({ ...this.deps.view(), exposure: 2 ** Number(exposure.value) }));
    $("set-exposure-reset").addEventListener("click", () => {
      exposure.value = "0";
      this.deps.onView({ ...this.deps.view(), exposure: 1 });
    });
  }

  async open(): Promise<void> {
    try {
      const [settings, status] = await Promise.all([getSettings(), datasetInfo()]);
      this.settings = settings;
      this.el.hidden = false;
      this.render(settings, status);
    } catch (e) {
      this.deps.toasts.error(`Couldn't load the settings: ${errorText(e)}`);
    }
  }

  close(): void {
    this.el.hidden = true;
    $("set-rebuild-confirm").hidden = true;
    $("set-populated-confirm").hidden = true;
  }

  /** Refreshes the populated-systems line (it changes during a download). */
  renderPopulated(): void {
    if (this.el.hidden) return;
    const status = this.deps.populatedStatus();
    $("set-populated").textContent = status;
    const missing = status.startsWith("Not downloaded");
    $("set-populated-update").textContent = missing ? "Download from Spansh…" : "Update from Spansh…";
    $<HTMLButtonElement>("set-populated-update").disabled = this.deps.populatedRunning();
  }

  private render(s: Settings, status: DataStatus): void {
    $("set-dataset").textContent =
      status.state === "ready"
        ? `${Number(status.systems ?? 0).toLocaleString()} systems` +
          (status.sourceLastModified ? ` · EDSM dump of ${new Date(status.sourceLastModified).toLocaleDateString()}` : "")
        : status.state === "broken"
          ? `Can't be used: ${status.error}`
          : "Not built yet.";
    $("set-data-dir").textContent = s.dataDir;
    $("set-data-origin").textContent = ORIGIN_NOTES[s.dataDirOrigin];
    $<HTMLButtonElement>("set-data-change").disabled = s.dataDirOrigin === "environment";

    $("set-journal-dir").textContent = s.journalDir ?? "(no default location on this system)";
    $("set-journal-status").textContent = s.journalDirFound
      ? s.journalDirCustom
        ? "Custom folder."
        : "The game's default folder."
      : "Not found: travel history and live location are off until this points at the game's journal folder.";
    $("set-journal-status").classList.toggle("problem", !s.journalDirFound);
    $("set-journal-reset").hidden = !s.journalDirCustom;

    $<HTMLSelectElement>("set-details-source").value = s.detailsSource;
    this.renderPopulated();

    const view = this.deps.view();
    $<HTMLSelectElement>("set-budget").value = String(view.pointBudget);
    $<HTMLInputElement>("set-exposure").value = String(Math.log2(view.exposure));
  }

  private async changeDataDir(): Promise<void> {
    const dir = await pickFolder("Choose a folder with StellarCartography data", this.settings?.dataDir).catch(() => null);
    if (!dir) return;
    try {
      await useDataDir(dir);
      location.reload();
    } catch (e) {
      this.deps.toasts.error(errorText(e));
    }
  }

  private async changeJournalDir(): Promise<void> {
    const dir = await pickFolder("Choose the Elite Dangerous journal folder", this.settings?.journalDir ?? undefined).catch(
      () => null,
    );
    if (dir) await this.applyJournalDir(dir);
  }

  private async applyJournalDir(dir: string | null): Promise<void> {
    try {
      await setJournalDir(dir);
      location.reload();
    } catch (e) {
      this.deps.toasts.error(errorText(e));
    }
  }
}
