// The key list: every keyboard action with its two keys. Click a key, then
// press the new one; Esc cancels and Backspace clears it.

import { ACTIONS, GROUPS, action, chordLabel, chordOf, isModifier, type ActionId, type Keyboard } from "../keys";
import { button, h } from "./dom";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

export interface KeysDeps {
  keyboard: Keyboard;
  /** The keys changed (they're in `keyboard.bindings`): save them. */
  onChange: () => void;
}

/** "W or ↑", "Ctrl+,", "no key". */
function keysText(keys: readonly string[]): string {
  const named = keys.filter(Boolean).map(chordLabel);
  return named.length ? named.join(" or ") : "no key";
}

export class KeysCard {
  private readonly el = $("keys");
  private readonly list = $("keys-list");
  private readonly note = $("keys-note");
  /** The slot waiting for a new key. */
  private listening: { id: ActionId; slot: 0 | 1 } | null = null;

  constructor(private readonly deps: KeysDeps) {
    $("keys-close").addEventListener("click", () => this.close());
    $("keys-reset").addEventListener("click", () => {
      this.listening = null;
      this.bindings.resetAll();
      this.say("Every key is back to its default.");
      this.changed();
    });
    // Ahead of everything else, so the new key does nothing but get assigned.
    window.addEventListener("keydown", (e) => this.capture(e), true);
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !e.defaultPrevented && this.shown) this.close();
    });
    // Clicking anywhere but another key stops listening.
    document.addEventListener("pointerdown", (e) => {
      if (this.listening && !(e.target instanceof Element && e.target.closest(".key-slot"))) this.stopListening();
    });
  }

  get shown(): boolean {
    return !this.el.hidden;
  }

  open(): void {
    this.say("");
    this.el.hidden = false;
    this.render();
  }

  close(): void {
    this.listening = null;
    this.el.hidden = true;
  }

  toggle(): void {
    if (this.shown) this.close();
    else this.open();
  }

  render(): void {
    if (!this.shown) return;
    this.list.replaceChildren(
      ...GROUPS.map((group) =>
        h("section", undefined, h("h2", undefined, group), ...ACTIONS.filter((a) => a.group === group).map((a) => this.row(a.id))),
      ),
    );
  }

  private get bindings() {
    return this.deps.keyboard.bindings;
  }

  private row(id: ActionId): HTMLElement {
    const keys = this.bindings.get(id);
    const label = action(id).label;
    const slots = ([0, 1] as const).map((slot) => {
      const listening = this.listening?.id === id && this.listening.slot === slot;
      const b = button(listening ? "Press a key…" : chordLabel(keys[slot]) || "—", () => this.listen(id, slot), "key-slot");
      b.dataset.slot = `${id}:${slot}`;
      b.classList.toggle("listening", listening);
      b.classList.toggle("empty", !keys[slot] && !listening);
      b.setAttribute("aria-label", `${label}, ${slot === 0 ? "first" : "second"} key: ${chordLabel(keys[slot]) || "none"}`);
      return b;
    });
    const reset = button(
      "↺",
      () => {
        this.listening = null;
        this.say(this.bindings.reset(id).map(({ key, from }) => this.tookFrom(key, from)).join(" "));
        this.changed();
      },
      "icon key-reset",
    );
    reset.title = `Back to ${keysText(action(id).keys)}`;
    reset.setAttribute("aria-label", `Put ${label} back to ${keysText(action(id).keys)}`);
    reset.hidden = this.bindings.isDefault(id);
    return h("div", "key-row", h("span", "key-action", label), ...slots, reset);
  }

  private listen(id: ActionId, slot: 0 | 1): void {
    const same = this.listening?.id === id && this.listening.slot === slot;
    this.listening = same ? null : { id, slot };
    this.render();
    this.focusSlot(id, slot);
  }

  private stopListening(): void {
    const l = this.listening;
    if (!l) return;
    this.listening = null;
    this.render();
    this.focusSlot(l.id, l.slot);
  }

  private capture(e: KeyboardEvent): void {
    const l = this.listening;
    if (!l) return;
    // Tab still moves on through the list.
    if (e.code === "Tab") return this.stopListening();
    e.preventDefault();
    e.stopImmediatePropagation();
    // Wait for the key a modifier goes with.
    if (e.repeat || !e.code || isModifier(e.code)) return;
    if (e.code === "Escape") return this.stopListening();
    const plain = !(e.ctrlKey || e.altKey || e.shiftKey || e.metaKey);
    const chord = e.code === "Backspace" && plain ? "" : chordOf(e);
    this.listening = null;
    const from = this.bindings.set(l.id, l.slot, chord);
    this.say(from ? this.tookFrom(chord, from) : "");
    this.changed();
    this.focusSlot(l.id, l.slot);
  }

  /** "Took Shift+1 from Galaxy glow on/off (which has no key now)." */
  private tookFrom(key: string, from: ActionId): string {
    const bare = !this.bindings.get(from).some(Boolean);
    return `Took ${chordLabel(key)} from ${action(from).label}${bare ? " (which has no key now)" : ""}.`;
  }

  private say(text: string): void {
    this.note.textContent = text;
    this.note.hidden = !text;
  }

  private changed(): void {
    // A key that moved may be held down right now.
    this.deps.keyboard.release();
    this.render();
    this.deps.onChange();
  }

  private focusSlot(id: ActionId, slot: 0 | 1): void {
    this.list.querySelector<HTMLButtonElement>(`[data-slot="${id}:${slot}"]`)?.focus();
  }
}
