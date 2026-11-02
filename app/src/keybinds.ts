import { useEffect, useRef } from "react";

/**
 * Keyboard shortcuts, for the app and the gate both (gate-ui imports this
 * file). The one list of what can be bound, its defaults, and what a combo
 * means.
 *
 * A combo is modifiers in a fixed order, then a KeyboardEvent.code:
 * "Ctrl+Enter", "Ctrl+Shift+Tab", "KeyE", "ArrowLeft". `code`, not `key`:
 * on X11/Wayland Shift+Tab is the ISO_Left_Tab keysym, so `key` need not say
 * "Tab". A modifier on its own ("Super") is a tap: pressed and let go with
 * nothing else in between, the way GNOME reads Super. "" is unbound.
 *
 * What is stored (meta `keybinds`, keybinds.rs) is overrides only, so a
 * default changed here reaches everyone who never touched that row.
 */

export type Section = "gate" | "app";

export interface Action {
  id: string;
  section: Section;
  label: string;
  /** Where it works, when that is not everywhere. */
  where?: string;
  default: string;
}

/** Keys that are not rebindable, listed so the page shows the whole map. */
export interface Fixed {
  section: Section;
  label: string;
  keys: string[][];
}

export const ACTIONS: Action[] = [
  { id: "gate.focusAdd", section: "gate", label: "Type a new task", where: "Jumps to the add box", default: "Super" },
  { id: "gate.start", section: "gate", label: "Start the session", default: "Ctrl+Enter" },
  { id: "gate.search", section: "gate", label: "Search the backlog", default: "Ctrl+KeyF" },
  {
    id: "gate.addDoing",
    section: "gate",
    label: "Add straight into Doing",
    where: "In the add box or the search box",
    default: "Alt+Enter",
  },

  { id: "app.newTask", section: "app", label: "New task", default: "Ctrl+KeyN" },
  { id: "app.nextTab", section: "app", label: "Next tab", default: "Ctrl+Tab" },
  { id: "app.prevTab", section: "app", label: "Previous tab", default: "Ctrl+Shift+Tab" },
  { id: "app.editTask", section: "app", label: "Edit the open task", where: "With a task open", default: "KeyE" },
  { id: "app.saveTask", section: "app", label: "Save the task", where: "In the task editor", default: "Ctrl+Enter" },
  { id: "app.prevDay", section: "app", label: "Previous day", where: "On the Dashboard", default: "ArrowLeft" },
  { id: "app.nextDay", section: "app", label: "Next day", where: "On the Dashboard", default: "ArrowRight" },
];

export const FIXED: Fixed[] = [
  { section: "gate", label: "Move through search results", keys: [["ArrowUp"], ["ArrowDown"]] },
  { section: "gate", label: "Add the task, or the search pick, to Today", keys: [["Enter"]] },
  { section: "gate", label: "Clear the search", keys: [["Escape"]] },
  { section: "gate", label: "Answer a question", keys: [["the letter shown"]] },
  { section: "app", label: "Move a held card", keys: [["ArrowUp"], ["ArrowDown"], ["ArrowLeft"], ["ArrowRight"]] },
  { section: "app", label: "Put a held card down", keys: [["Enter"]] },
  { section: "app", label: "Close, cancel, put a held card back", keys: [["Escape"]] },
];

/** Combos the fixed keys own, per section: binding one would fight them. */
const RESERVED: Record<Section, string[]> = {
  gate: ["Enter", "Escape", "ArrowUp", "ArrowDown"],
  app: ["Enter", "Escape"],
};

export type Keymap = Record<string, string>;

const BY_ID = new Map(ACTIONS.map((a) => [a.id, a]));

/** Every action's combo: the override where there is a string one, the
 *  default otherwise. Junk (a non-object, an unknown id) is ignored. */
export function resolve(overrides: unknown): Keymap {
  const out: Keymap = {};
  const given = overrides && typeof overrides === "object" ? (overrides as Record<string, unknown>) : {};
  for (const a of ACTIONS) {
    const v = given[a.id];
    out[a.id] = typeof v === "string" ? v : a.default;
  }
  return out;
}

/** Only what differs from the defaults: what gets stored. */
export function overridesOf(map: Keymap): Keymap {
  const out: Keymap = {};
  for (const [id, combo] of Object.entries(map)) {
    const a = BY_ID.get(id);
    if (a && combo !== a.default) out[id] = combo;
  }
  return out;
}

// ---------------------------------------------------------------- events

type Mod = "Ctrl" | "Alt" | "Shift" | "Super";
const MODS: Mod[] = ["Ctrl", "Alt", "Shift", "Super"];

type KeyLike = Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey">;

/** Which modifier a key is, if it is one. WebKitGTK and Chromium have both
 *  called Super "Meta" and "OS" over the years, so every spelling counts. */
export function modifierOf(e: Pick<KeyboardEvent, "key" | "code">): Mod | null {
  const k = e.key;
  const c = e.code;
  if (k === "Control" || c.startsWith("Control")) return "Ctrl";
  if (k === "Alt" || k === "AltGraph" || c.startsWith("Alt")) return "Alt";
  if (k === "Shift" || c.startsWith("Shift")) return "Shift";
  if (k === "Meta" || k === "OS" || k === "Super" || k === "Hyper" || c.startsWith("Meta") || c.startsWith("OS"))
    return "Super";
  return null;
}

function held(e: KeyLike): Mod[] {
  return MODS.filter(
    (m) => ({ Ctrl: e.ctrlKey, Alt: e.altKey, Shift: e.shiftKey, Super: e.metaKey })[m],
  );
}

/** The combo a keydown spells, or null for a modifier pressed on its own
 *  (that one is known only when it is let go: see tapOf). */
export function comboOf(e: KeyLike): string | null {
  if (modifierOf(e)) return null;
  const code = e.code === "NumpadEnter" ? "Enter" : e.code;
  if (!code) return null;
  return [...held(e), code].join("+");
}

/** Whether a keydown is this combo: exactly these modifiers, this key. A
 *  tap combo never matches a keydown. */
export function matches(e: KeyLike, combo: string | undefined): boolean {
  if (!combo || isTap(combo)) return false;
  return comboOf(e) === combo;
}

export const isTap = (combo: string) => (MODS as string[]).includes(combo);

/**
 * Calls `fn` when `combo` is a tap ("Super") and that modifier is pressed
 * and let go with no other key in between. Does nothing for a chord: those
 * go through matches() in the screen's own key handler. Listens in the
 * capture phase so a field cannot swallow it.
 */
export function useTap(combo: string | undefined, fn: () => void, enabled = true): void {
  const fnRef = useRef(fn);
  fnRef.current = fn;
  useEffect(() => {
    if (!enabled || !combo || !isTap(combo)) return;
    let armed = false;
    const down = (e: KeyboardEvent) => {
      // Armed only by the modifier alone; anything else pressed on the way
      // (Super+L, a second modifier) makes it a chord, not a tap.
      if (e.repeat && modifierOf(e) === combo) return;
      armed = modifierOf(e) === combo && held(e).every((m) => m === combo);
    };
    const up = (e: KeyboardEvent) => {
      if (armed && modifierOf(e) === combo) {
        armed = false;
        fnRef.current();
      }
    };
    const disarm = () => (armed = false);
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    window.addEventListener("pointerdown", disarm, true);
    window.addEventListener("blur", disarm);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
      window.removeEventListener("pointerdown", disarm, true);
      window.removeEventListener("blur", disarm);
    };
  }, [combo, enabled]);
}

// ---------------------------------------------------------------- words

const NAMES: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Escape: "Esc",
  Space: "Space",
  Backquote: "`",
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
  PageUp: "Page Up",
  PageDown: "Page Down",
};

/** One key's name as printed on a keycap. */
export function keyName(part: string): string {
  if (NAMES[part]) return NAMES[part];
  if (/^Key[A-Z]$/.test(part)) return part.slice(3);
  if (/^Digit\d$/.test(part)) return part.slice(5);
  if (/^Numpad/.test(part)) return "Num " + part.slice(6);
  return part;
}

/** A combo as keycaps, in order; none when unbound. */
export function keycaps(combo: string): string[] {
  return combo ? combo.split("+").map(keyName) : [];
}

// ------------------------------------------------------------- problems

/** For each action, what else in its section already wants its combo: the
 *  other action's label, or the fixed key's. Absent when nothing does. */
export function clashes(map: Keymap): Record<string, string> {
  const out: Record<string, string> = {};
  for (const a of ACTIONS) {
    const combo = map[a.id];
    if (!combo) continue;
    const other = ACTIONS.find((b) => b.id !== a.id && b.section === a.section && map[b.id] === combo);
    if (other) out[a.id] = other.label;
    else if (RESERVED[a.section].includes(combo)) out[a.id] = `${keyName(combo)} (fixed)`;
  }
  return out;
}

/** Whether a combo has Super in it -- which GNOME keeps for itself. */
export const usesSuper = (combo: string) => combo.split("+").includes("Super");
