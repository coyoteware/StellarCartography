// Keyboard shortcuts: the actions, their default keys, the user's changes,
// and the listener that runs them.
//
// A key is stored as the physical key (`KeyboardEvent.code`, so WASD stays
// where it is on other layouts) behind any modifiers: "KeyW", "Shift+Digit1",
// "Ctrl+Comma". Each action has two slots, "" being an empty one. Only the
// actions the user changed are saved, so new defaults still reach the rest.

export type Group = "Camera" | "Jump to" | "Layers" | "Panels";
export const GROUPS: readonly Group[] = ["Camera", "Jump to", "Layers", "Panels"];

export type Slots = readonly [string, string];

interface ActionDef {
  readonly id: string;
  readonly group: Group;
  readonly label: string;
  /** Runs for as long as the key is down (camera movement), not once per press. */
  readonly held?: boolean;
  /** Also works while Settings or the key list is open. */
  readonly anywhere?: boolean;
  readonly keys: Slots;
}

export const ACTIONS = [
  { id: "moveForward", group: "Camera", label: "Move forward", held: true, keys: ["KeyW", "ArrowUp"] },
  { id: "moveBack", group: "Camera", label: "Move back", held: true, keys: ["KeyS", "ArrowDown"] },
  { id: "moveLeft", group: "Camera", label: "Move left", held: true, keys: ["KeyA", "ArrowLeft"] },
  { id: "moveRight", group: "Camera", label: "Move right", held: true, keys: ["KeyD", "ArrowRight"] },
  { id: "moveUp", group: "Camera", label: "Move up", held: true, keys: ["KeyR", "PageUp"] },
  { id: "moveDown", group: "Camera", label: "Move down", held: true, keys: ["KeyF", "PageDown"] },
  { id: "orbitLeft", group: "Camera", label: "Orbit left", held: true, keys: ["KeyQ", ""] },
  { id: "orbitRight", group: "Camera", label: "Orbit right", held: true, keys: ["KeyE", ""] },
  { id: "orbitUp", group: "Camera", label: "Orbit up", held: true, keys: ["KeyT", ""] },
  { id: "orbitDown", group: "Camera", label: "Orbit down", held: true, keys: ["KeyG", ""] },
  { id: "zoomIn", group: "Camera", label: "Zoom in", held: true, keys: ["KeyZ", "Equal"] },
  { id: "zoomOut", group: "Camera", label: "Zoom out", held: true, keys: ["KeyX", "Minus"] },

  { id: "goSol", group: "Jump to", label: "Sol", keys: ["Digit1", ""] },
  { id: "goGalaxy", group: "Jump to", label: "Whole galaxy", keys: ["Digit2", ""] },
  { id: "goCommander", group: "Jump to", label: "Commander", keys: ["Digit3", "KeyC"] },
  { id: "goHome", group: "Jump to", label: "Home system", keys: ["Digit4", "Home"] },
  { id: "goSquadron", group: "Jump to", label: "Squadron capital", keys: ["Digit5", ""] },

  { id: "toggleGlow", group: "Layers", label: "Galaxy glow on/off", keys: ["Shift+Digit1", ""] },
  { id: "toggleStars", group: "Layers", label: "Known systems on/off", keys: ["Shift+Digit2", ""] },
  { id: "toggleRoute", group: "Layers", label: "Travel history on/off", keys: ["Shift+Digit3", ""] },
  { id: "togglePoi", group: "Layers", label: "Points of interest on/off", keys: ["Shift+Digit4", ""] },
  { id: "togglePlot", group: "Layers", label: "Plotted route on/off", keys: ["Shift+Digit5", ""] },

  { id: "search", group: "Panels", label: "Search", keys: ["Slash", "Ctrl+KeyF"] },
  { id: "places", group: "Panels", label: "Places open/closed", keys: ["KeyP", ""] },
  { id: "route", group: "Panels", label: "Route planner open/closed", keys: ["KeyO", ""] },
  { id: "settings", group: "Panels", label: "Settings", anywhere: true, keys: ["Ctrl+Comma", ""] },
  { id: "keys", group: "Panels", label: "This list of keys", anywhere: true, keys: ["F1", "Shift+Slash"] },
] as const satisfies readonly ActionDef[];

export type ActionId = (typeof ACTIONS)[number]["id"];

const BY_ID = new Map<string, ActionDef>(ACTIONS.map((a) => [a.id, a]));

export function action(id: ActionId): ActionDef {
  return BY_ID.get(id)!;
}

// --- Chords ------------------------------------------------------------------

const MODIFIER_CODES = new Set([
  "ShiftLeft",
  "ShiftRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
  "CapsLock",
  "NumLock",
  "ScrollLock",
  "Fn",
]);

/** A modifier on its own: wait for the key it modifies. */
export function isModifier(code: string): boolean {
  return MODIFIER_CODES.has(code);
}

/** The chord a key press makes: modifiers in a fixed order, then the key. */
export function chordOf(e: KeyboardEvent): string {
  const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Meta"].filter(Boolean);
  return [...mods, e.code].join("+");
}

const CHORD = /^((Ctrl|Alt|Shift|Meta)\+)*[A-Za-z0-9]+$/;

/** Key names for keys the layout map doesn't cover (it only has the ones that type characters). */
const NAMES: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  PageUp: "Page Up",
  PageDown: "Page Down",
  Escape: "Esc",
  Space: "Space",
  Backspace: "Backspace",
  Delete: "Del",
  Insert: "Ins",
  Enter: "Enter",
  NumpadEnter: "Num Enter",
  NumpadAdd: "Num +",
  NumpadSubtract: "Num −",
  NumpadMultiply: "Num *",
  NumpadDivide: "Num /",
  NumpadDecimal: "Num .",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backquote: "`",
};

/** What each key types on this keyboard's layout, once the browser has said. */
let layout: Map<string, string> | null = null;

interface LayoutKeyboard {
  getLayoutMap(): Promise<Map<string, string>>;
}

/** Asks for the keyboard layout so keys are named as printed (A on AZERTY's Q key). */
export async function loadLayout(): Promise<void> {
  const kb = (navigator as Navigator & { keyboard?: LayoutKeyboard }).keyboard;
  try {
    layout = (await kb?.getLayoutMap()) ?? null;
  } catch {
    // Not offered here: fall back to US names.
  }
}

export function keyName(code: string): string {
  const typed = layout?.get(code);
  if (typed && typed.trim()) return typed.length === 1 ? typed.toUpperCase() : typed;
  if (NAMES[code]) return NAMES[code];
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^Numpad\d$/.test(code)) return `Num ${code.slice(6)}`;
  return code;
}

/** "Shift+Digit1" → "Shift+1"; "" → "". */
export function chordLabel(chord: string): string {
  if (!chord) return "";
  const parts = chord.split("+");
  const code = parts.pop()!;
  return [...parts, keyName(code)].join("+");
}

// --- Bindings ----------------------------------------------------------------

/** Every action's two keys, with the user's changes over the defaults. */
export class Bindings {
  private readonly slots = new Map<ActionId, [string, string]>();

  /** From the saved changes (settings.json `keys`); bad entries are dropped. */
  constructor(saved: Record<string, unknown> = {}) {
    const taken = new Set<string>();
    const claim = (chord: unknown): string => {
      if (typeof chord !== "string" || !CHORD.test(chord) || taken.has(chord)) return "";
      taken.add(chord);
      return chord;
    };
    // The user's keys first, so a default never takes a key they gave elsewhere.
    const mine = new Map<ActionId, [string, string]>();
    for (const a of ACTIONS) {
      const keys = saved[a.id];
      if (Array.isArray(keys)) mine.set(a.id, [claim(keys[0]), claim(keys[1])]);
    }
    for (const a of ACTIONS) this.slots.set(a.id, mine.get(a.id) ?? [claim(a.keys[0]), claim(a.keys[1])]);
  }

  get(id: ActionId): Slots {
    return this.slots.get(id)!;
  }

  /** The action a chord runs, if any. */
  find(chord: string): ActionId | undefined {
    if (!chord) return undefined;
    for (const [id, keys] of this.slots) if (keys.includes(chord)) return id;
    return undefined;
  }

  isDefault(id: ActionId): boolean {
    const keys = this.get(id);
    const defaults = action(id).keys;
    return keys[0] === defaults[0] && keys[1] === defaults[1];
  }

  /**
   * Puts `chord` in one of an action's slots ("" empties it). A chord can
   * only do one thing, so another action holding it loses it: that one is
   * returned.
   */
  set(id: ActionId, slot: 0 | 1, chord: string): ActionId | undefined {
    let displaced: ActionId | undefined;
    if (chord) {
      for (const [other, keys] of this.slots) {
        const i = keys.indexOf(chord);
        if (i < 0 || (other === id && i === slot)) continue;
        keys[i] = "";
        if (other !== id) displaced = other;
      }
    }
    this.slots.get(id)![slot] = chord;
    return displaced;
  }

  /**
   * Puts an action back to its default keys, taking them from any other
   * action that has them meanwhile. Returns each key taken and who from.
   */
  reset(id: ActionId): { key: string; from: ActionId }[] {
    const taken: { key: string; from: ActionId }[] = [];
    for (const slot of [0, 1] as const) {
      const key = action(id).keys[slot];
      const from = this.set(id, slot, key);
      if (from) taken.push({ key, from });
    }
    return taken;
  }

  resetAll(): void {
    for (const a of ACTIONS) this.slots.set(a.id, [a.keys[0], a.keys[1]]);
  }

  /** What to save: the actions whose keys differ from the defaults. */
  saved(): Record<string, string[]> {
    const out: Record<string, string[]> = {};
    for (const a of ACTIONS) if (!this.isDefault(a.id)) out[a.id] = [...this.get(a.id)];
    return out;
  }
}

// --- Listener ----------------------------------------------------------------

/** Typing goes to the field, not to the shortcuts. */
function typingIn(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  return target instanceof HTMLInputElement && !["checkbox", "radio", "button", "submit", "reset"].includes(target.type);
}

/** Enter and Space press a focused button or tick a checkbox. */
function pressing(e: KeyboardEvent): boolean {
  const t = e.target;
  const control = t instanceof HTMLButtonElement || t instanceof HTMLInputElement;
  return control && (e.code === "Enter" || e.code === "NumpadEnter" || e.code === "Space");
}

export interface KeyboardOptions {
  /** True while Settings or the key list is open: only their own keys work then. */
  modal: () => boolean;
}

/** Runs actions from key presses. Held actions are read each frame with `holding`. */
export class Keyboard {
  bindings = new Bindings();
  /** A held action started (the view should start drawing at full rate). */
  onHold: () => void = () => {};
  private readonly handlers = new Map<ActionId, () => void>();
  /** Keys down that are driving a held action, by physical key. */
  private readonly held = new Map<string, ActionId>();

  constructor(private readonly opts: KeyboardOptions) {
    window.addEventListener("keydown", (e) => this.keyDown(e));
    window.addEventListener("keyup", (e) => this.held.delete(e.code));
    // Keys let go elsewhere never send their keyup here.
    window.addEventListener("blur", () => this.release());
    document.addEventListener("visibilitychange", () => this.release());
  }

  on(id: ActionId, run: () => void): void {
    this.handlers.set(id, run);
  }

  /** Swaps in new bindings (anything held is let go). */
  use(bindings: Bindings): void {
    this.bindings = bindings;
    this.release();
  }

  holding(id: ActionId): boolean {
    for (const a of this.held.values()) if (a === id) return true;
    return false;
  }

  /** -1, 0 or 1 from a pair of opposite held actions. */
  axis(minus: ActionId, plus: ActionId): number {
    return Number(this.holding(plus)) - Number(this.holding(minus));
  }

  release(): void {
    this.held.clear();
  }

  private keyDown(e: KeyboardEvent): void {
    if (e.defaultPrevented || e.isComposing || !e.code || isModifier(e.code)) return;
    const id = this.bindings.find(chordOf(e));
    if (!id || typingIn(e.target) || pressing(e)) return;
    const def = action(id);
    if (!def.anywhere && this.opts.modal()) return;
    e.preventDefault();
    if (def.held) {
      if (!this.held.has(e.code)) {
        this.held.set(e.code, id);
        this.onHold();
      }
      return;
    }
    if (!e.repeat) this.handlers.get(id)?.();
  }
}
