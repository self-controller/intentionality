import type { State } from "./types";

declare global {
  interface Window {
    webkit?: { messageHandlers: { gate: { postMessage(s: string): void } } };
    __gate?: { push: (s: State) => void };
  }
}

/** JS -> Python. The handler is registered as "gate" on the
 *  WebKit.UserContentManager; a JSON string keeps the Python side to one
 *  json.loads and avoids walking JSC objects. */
export function post(op: string, payload: Record<string, unknown> = {}): void {
  const body = JSON.stringify({ op, ...payload });
  if (window.webkit?.messageHandlers?.gate) {
    window.webkit.messageHandlers.gate.postMessage(body);
  } else {
    // Opened in a plain browser for design work; there is no gate to answer.
    console.log("post", body);
  }
}

/** Python -> JS, via evaluate_javascript("window.__gate.push(...)"). */
export function onPush(cb: (s: State) => void): void {
  window.__gate = { push: cb };
}
