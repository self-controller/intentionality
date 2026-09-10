import type { Observed, Session } from "./types";

// What identifies a session now that the gate no longer asks for a statement:
// its own words if it has them (older sessions do), otherwise its tasks. The
// TS twin of gate/store.py::session_label.
export function sessionLabel(s: Session): string {
  if (s.statement) return s.statement;
  if (!s.first_task) return "(no tasks)";
  return s.task_count > 1 ? `${s.first_task} +${s.task_count - 1}` : s.first_task;
}

// The backend caps each app's titles and rolls the long tail into "(other)",
// which can outweigh every real title — so it is pinned last rather than
// sorted by size. BTreeMap serializes alphabetically, so ranking happens here.
export const OTHER = "(other)";

export function rankTitles(observed: Observed | null, app: string): [string, number][] {
  return Object.entries(observed?.per_title?.[app] ?? {}).sort((a, b) => {
    if (a[0] === OTHER) return 1;
    if (b[0] === OTHER) return -1;
    return b[1] - a[1];
  });
}

export function fmt(seconds: number): string {
  const m = Math.floor(seconds / 60);
  if (m >= 60) return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
  if (m > 0) return `${m}m`;
  return `${Math.floor(seconds)}s`;
}

export const hhmm = (ts: string) =>
  new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

// Alignment is a status, not a series: the glyph and the word carry the band,
// colour only reinforces it. Green and amber are near-identical to a deutan
// reader (and close even in full colour), so neither is ever the sole signal.
const BANDS = [
  { min: 67, label: "aligned", glyph: "●", cls: "good" },
  { min: 34, label: "drifting", glyph: "◐", cls: "warn" },
  { min: 0, label: "off track", glyph: "○", cls: "bad" },
];
const NOT_JUDGED = { label: "not judged", glyph: "·", cls: "none" };

export const band = (alignment: number | null) =>
  alignment == null ? NOT_JUDGED : BANDS.find((b) => alignment >= b.min)!;

export const dayKey = (ts: string) => new Date(ts).toDateString();

export function dayLabel(ts: string): string {
  const key = dayKey(ts);
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (key === today.toDateString()) return "Today";
  if (key === yesterday.toDateString()) return "Yesterday";
  return new Date(ts).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
}
