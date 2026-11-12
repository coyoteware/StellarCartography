// Settings panel: catalogue folder and rebuilds, journal folder, the
// systems behind the jump buttons, the populated systems behind the overlay
// layers, the points of interest, display preferences (point budget,
// brightness), and the way to the keys.

import {
  datasetInfo,
  getSettings,
  journalFriends,
  pickFolder,
  searchSystems,
  setDetailsSource,
  setFriends,
  setUpdateReminders,
  setJournalDir,
  useDataDir,
  type DataStatus,
  type DirOrigin,
  type Bookmark,
  type FriendStatus,
  type OnlineSource,
  type Settings,
  type ViewSettings,
} from "../api";
import { BOOKMARKS, type Bookmarks } from "./bookmarks";
import { DOWNLOAD_SIZES } from "./download";
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
  /** The systems behind the Home and Squadron jump buttons. */
  bookmarks: Bookmarks;
  view: () => ViewSettings;
  onView: (view: ViewSettings) => void;
  onRebuild: () => Promise<void>;
  buildRunning: () => Promise<boolean>;
  /** Status line for the populated systems. */
  populatedStatus: () => string;
  populatedRunning: () => boolean;
  onPopulatedUpdate: () => void;
  /** Status line for the points of interest. */
  poiStatus: () => string;
  poiRunning: () => boolean;
  onPoiUpdate: () => void;
  /** The update-reminders switch changed (it's saved already). */
  onReminders: (on: boolean) => void;
  /** Opens the key list (Settings closes first). */
  onKeys: () => void;
  /** The latest friend lookups, for the status beside each name. */
  friends: () => FriendStatus[];
  /** The friends list changed (it's saved already): refresh the markers. */
  onFriends: () => void;
}

export class SettingsPanel {
  private readonly el = $("settings");
  private settings: Settings | null = null;
  private suggestTimer = 0;
  private suggestSeq = 0;

  constructor(private readonly deps: SettingsDeps) {
    $("set-populated-size").textContent = DOWNLOAD_SIZES.populated;
    $("set-poi-size").textContent = DOWNLOAD_SIZES.poi;
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

    $("set-poi-update").addEventListener("click", () => {
      if (this.deps.poiRunning()) return this.deps.toasts.show("The points of interest are already downloading.");
      $("set-poi-confirm").hidden = false;
    });
    $("set-poi-cancel").addEventListener("click", () => ($("set-poi-confirm").hidden = true));
    $("set-poi-go").addEventListener("click", () => {
      $("set-poi-confirm").hidden = true;
      this.deps.onPoiUpdate();
      this.renderPoi();
    });

    const reminders = $<HTMLInputElement>("set-update-reminders");
    reminders.addEventListener("change", () => {
      setUpdateReminders(reminders.checked)
        .then(() => this.deps.onReminders(reminders.checked))
        .catch((e) => this.deps.toasts.error(`Couldn't save the setting: ${errorText(e)}`));
    });

    const detailsSource = $<HTMLSelectElement>("set-details-source");
    detailsSource.addEventListener("change", () => {
      setDetailsSource(detailsSource.value as OnlineSource).catch((e) =>
        this.deps.toasts.error(`Couldn't save the setting: ${errorText(e)}`),
      );
    });

    $("set-journal-change").addEventListener("click", () => void this.changeJournalDir());
    $("set-journal-reset").addEventListener("click", () => void this.applyJournalDir(null));

    for (const slot of BOOKMARKS) this.bookmarkField(slot);

    const budget = $<HTMLSelectElement>("set-budget");
    budget.addEventListener("change", () => this.deps.onView({ ...this.deps.view(), pointBudget: Number(budget.value) }));
    const exposure = $<HTMLInputElement>("set-exposure");
    exposure.addEventListener("input", () => this.deps.onView({ ...this.deps.view(), exposure: 2 ** Number(exposure.value) }));
    $("set-exposure-reset").addEventListener("click", () => {
      exposure.value = "0";
      this.deps.onView({ ...this.deps.view(), exposure: 1 });
    });

    const friendName = $<HTMLInputElement>("set-friend-name");
    $("set-friend-add").addEventListener("click", () => void this.addFriend());
    friendName.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        void this.addFriend();
      }
    });

    $("set-keys").addEventListener("click", () => {
      this.close();
      this.deps.onKeys();
    });
  }

  get shown(): boolean {
    return !this.el.hidden;
  }

  async open(): Promise<void> {
    try {
      const [settings, status] = await Promise.all([getSettings(), datasetInfo()]);
      this.settings = settings;
      this.el.hidden = false;
      this.render(settings, status);
    } catch (e) {
      this.deps.toasts.error(`Couldn't load the settings: ${errorText(e)}`);
      return;
    }
    // Suggestions can come in after the panel shows.
    try {
      const names = await journalFriends();
      const list = $<HTMLDataListElement>("friend-suggestions");
      list.replaceChildren(
        ...names.map((n) => {
          const opt = document.createElement("option");
          opt.value = n;
          return opt;
        }),
      );
    } catch (e) {
      console.error("friend suggestions:", e);
    }
  }

  close(): void {
    this.el.hidden = true;
    for (const slot of BOOKMARKS) $(`set-bookmark-${slot}-problem`).hidden = true;
    $("set-rebuild-confirm").hidden = true;
    $("set-populated-confirm").hidden = true;
    $("set-poi-confirm").hidden = true;
  }

  /** Shows the systems behind the jump buttons (leaving a field being typed in alone). */
  renderBookmarks(): void {
    if (this.el.hidden) return;
    for (const slot of BOOKMARKS) {
      const system = this.deps.bookmarks.get(slot);
      const input = $<HTMLInputElement>(`set-bookmark-${slot}`);
      if (document.activeElement !== input) input.value = system?.name ?? "";
      $(`set-bookmark-${slot}-clear`).hidden = !system;
    }
  }

  /** Redraws the friends list with each name's latest lookup. */
  renderFriends(): void {
    if (this.el.hidden || !this.settings) return;
    const statuses = this.deps.friends();
    const rows = this.settings.friends.map((name) => {
      const li = document.createElement("li");
      li.className = "friend-row";
      const label = document.createElement("span");
      label.className = "name";
      label.textContent = name;
      const meta = document.createElement("span");
      meta.className = "meta";
      const s = statuses.find((f) => f.name.toLowerCase() === name.toLowerCase());
      if (s?.error) {
        meta.textContent = s.error;
        meta.classList.add("problem");
      } else if (s?.position) {
        const p = s.position;
        if (!p.known) {
          meta.textContent = "Not found on EDSM (no profile, or it's private).";
        } else if (p.system) {
          meta.textContent = p.system + (p.date ? ` · ${p.date}` : "");
        } else {
          meta.textContent = "On EDSM, but no position shared.";
        }
      }
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "close";
      remove.setAttribute("aria-label", `Remove ${name}`);
      remove.textContent = "×";
      remove.addEventListener("click", () => void this.saveFriends(this.settings!.friends.filter((n) => n !== name)));
      li.append(label, meta, remove);
      return li;
    });
    const list = $("set-friends-list");
    list.replaceChildren(...rows);
    list.hidden = rows.length === 0;
  }

  private async addFriend(): Promise<void> {
    const input = $<HTMLInputElement>("set-friend-name");
    const name = input.value.trim();
    if (!name || !this.settings) return;
    if (this.settings.friends.some((n) => n.toLowerCase() === name.toLowerCase())) {
      input.value = "";
      return;
    }
    await this.saveFriends([...this.settings.friends, name]);
    input.value = "";
  }

  private async saveFriends(names: string[]): Promise<void> {
    if (!this.settings) return;
    try {
      await setFriends(names);
      this.settings.friends = names;
      this.renderFriends();
      this.deps.onFriends();
    } catch (e) {
      this.deps.toasts.error(`Couldn't save the friends list: ${errorText(e)}`);
    }
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

  /** Refreshes the points-of-interest line (it changes during a download). */
  renderPoi(): void {
    if (this.el.hidden) return;
    const status = this.deps.poiStatus();
    $("set-poi").textContent = status;
    const missing = status.startsWith("Not downloaded");
    $("set-poi-update").textContent = missing ? "Download from EDAstro…" : "Update from EDAstro…";
    $<HTMLButtonElement>("set-poi-update").disabled = this.deps.poiRunning();
  }

  private render(s: Settings, status: DataStatus): void {
    $("set-dataset").textContent =
      status.state === "ready"
        ? `${Number(status.systems ?? 0).toLocaleString()} systems` +
          (status.sourceLastModified ? ` · EDSM dump of ${new Date(status.sourceLastModified).toLocaleDateString()}` : "") +
          (status.builtAt ? ` · built ${new Date(status.builtAt * 1000).toLocaleDateString()}` : "")
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
    $<HTMLInputElement>("set-update-reminders").checked = s.updateReminders;
    this.renderBookmarks();
    this.renderFriends();
    this.renderPopulated();
    this.renderPoi();

    const view = this.deps.view();
    $<HTMLSelectElement>("set-budget").value = String(view.pointBudget);
    $<HTMLInputElement>("set-exposure").value = String(Math.log2(view.exposure));
  }

  /**
   * A jump button's name field: suggests catalogue systems as you type, and
   * saves once a name is picked from the list or typed out and confirmed
   * (Enter, or leaving the field). Emptying it clears the button.
   */
  private bookmarkField(slot: Bookmark): void {
    const input = $<HTMLInputElement>(`set-bookmark-${slot}`);
    const problem = $(`set-bookmark-${slot}-problem`);
    const list = $("bookmark-systems");
    let saving: string | null = null;

    const commit = async () => {
      const name = input.value.trim();
      const saved = this.deps.bookmarks.get(slot);
      if (name.toLowerCase() === (saved?.name.toLowerCase() ?? "") || name === saving) return;
      problem.hidden = true;
      saving = name;
      try {
        if (!name) return await this.deps.bookmarks.set(slot, null);
        const found = await searchSystems(name).catch(() => []);
        // Cleared or retyped while it looked: that wins.
        if (input.value.trim() !== name) return;
        const system = found.find((s) => s.name.toLowerCase() === name.toLowerCase());
        if (!system) {
          problem.textContent = `There's no system called "${name}" in the catalogue.`;
          problem.hidden = false;
          return;
        }
        await this.deps.bookmarks.set(slot, system);
        // The catalogue's spelling, unless they've moved on to typing something else.
        if (input.value.trim() === name) input.value = system.name;
      } finally {
        saving = null;
      }
    };

    input.addEventListener("input", (e) => {
      problem.hidden = true;
      // Picking from the list replaces the text in one go: take it straight away.
      if (!(e instanceof InputEvent) || e.inputType === "insertReplacementText") return void commit();
      window.clearTimeout(this.suggestTimer);
      this.suggestTimer = window.setTimeout(async () => {
        const query = input.value.trim();
        if (query.length < 2) return;
        const mine = ++this.suggestSeq;
        const found = await searchSystems(query).catch(() => []);
        if (mine !== this.suggestSeq) return;
        list.replaceChildren(
          ...found.slice(0, 10).map((s) => {
            const option = document.createElement("option");
            option.value = s.name;
            return option;
          }),
        );
      }, 150);
    });
    input.addEventListener("change", () => void commit());
    $(`set-bookmark-${slot}-clear`).addEventListener("click", () => {
      input.value = "";
      problem.hidden = true;
      void this.deps.bookmarks.set(slot, null);
    });
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
