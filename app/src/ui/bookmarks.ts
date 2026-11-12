// The Home and Squadron jump buttons: which system each goes to. The user
// picks them in Settings → Jump buttons; the choice is kept in settings.json,
// and a button with nothing behind it stays hidden.

import { setBookmark, type Bookmark, type SavedSystem, type Settings, type SystemInfo } from "../api";
import { errorText, type Toasts } from "./toast";

export const BOOKMARKS: readonly Bookmark[] = ["home", "squadron"];

/** What each is called in a sentence. */
export const BOOKMARK_NAMES: Record<Bookmark, string> = {
  home: "home system",
  squadron: "squadron capital",
};

export class Bookmarks {
  private readonly saved: Record<Bookmark, SavedSystem | null> = { home: null, squadron: null };
  /** Called whenever what's behind a button changes. */
  onChange: () => void = () => {};

  constructor(private readonly toasts: Toasts) {}

  /** Takes the systems from the settings read at startup. */
  load(settings: Settings): void {
    this.saved.home = settings.homeSystem ?? null;
    this.saved.squadron = settings.squadronSystem ?? null;
    this.onChange();
  }

  get(slot: Bookmark): SavedSystem | null {
    return this.saved[slot];
  }

  /** The system behind `slot`, ready to select. */
  system(slot: Bookmark): SystemInfo | null {
    const s = this.saved[slot];
    return s ? { rowid: null, ...s } : null;
  }

  /** Saves `system` behind `slot` (null clears it) and says what changed. */
  async set(slot: Bookmark, system: SystemInfo | null): Promise<void> {
    const next: SavedSystem | null = system && { id64: system.id64, name: system.name, pos: [...system.pos] };
    const long = BOOKMARK_NAMES[slot];
    try {
      await setBookmark(slot, next);
    } catch (e) {
      this.toasts.error(`Couldn't save the ${long}: ${errorText(e)}`);
      return;
    }
    const was = this.saved[slot];
    this.saved[slot] = next;
    this.onChange();
    const message = next
      ? `${next.name} is now your ${long}${was ? ` (was ${was.name})` : ""}.`
      : `${was?.name ?? "That system"} is no longer your ${long}.`;
    this.toasts.show(message, "info", { timeout: 4 });
  }
}
