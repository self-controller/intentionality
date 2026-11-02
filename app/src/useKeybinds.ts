import { useSyncExternalStore } from "react";
import * as api from "./api";
import { overridesOf, resolve, type Keymap } from "./keybinds";

/**
 * The app's shortcuts, held once for every screen: loaded at start, replaced
 * whole when Settings saves. Until the store answers, the defaults.
 */
let current: Keymap = resolve({});
const listeners = new Set<() => void>();

function put(next: Keymap) {
  current = next;
  listeners.forEach((l) => l());
}

/** The map in force, read at the moment of a key press by handlers that
 *  are not components. */
export const keymap = (): Keymap => current;

export function loadKeybinds(): void {
  api
    .keybinds()
    .then((o) => put(resolve(o)))
    .catch(() => {});
}

/** Store a whole map (only its differences from the defaults are kept) and
 *  put what the store kept in force. */
export async function saveKeybinds(next: Keymap): Promise<void> {
  const kept = await api.setKeybinds(overridesOf(next));
  put(resolve(kept));
}

export function useKeybinds(): Keymap {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}
