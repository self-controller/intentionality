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

// A calendar day on the local clock, the form task.due_date is stored in. Not
// toISOString(): that is UTC, and in the evening it is already tomorrow there.
export function localDate(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export const ISO_DAY = /^\d{4}-\d{2}-\d{2}$/;

export function addDays(d: Date, n: number): Date {
  const out = new Date(d);
  out.setDate(out.getDate() + n);
  return out;
}

export type DueClass = "overdue" | "today" | "soon" | "later";

// A due date in words, relative to today. The words carry the meaning and the
// class only colours it, the same rule band() follows. `date` is the plain
// short date, for a finished card that should keep the date but not the alarm.
export function dueInfo(due: string): { text: string; cls: DueClass; date: string } {
  // Local midnight: new Date("2026-09-18") would be UTC midnight, which is
  // the previous evening here.
  const [y, m, d] = due.split("-").map(Number);
  const day = new Date(y, m - 1, d);
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  // Rounded, because a DST change makes one day 23 or 25 hours long.
  const days = Math.round((day.getTime() - today.getTime()) / 86_400_000);
  const date = day.toLocaleDateString([], {
    month: "short",
    day: "numeric",
    year: y !== today.getFullYear() ? "numeric" : undefined,
  });
  if (days < 0) return { text: `Overdue · ${date}`, cls: "overdue", date };
  if (days === 0) return { text: "Due today", cls: "today", date };
  if (days === 1) return { text: "Due tomorrow", cls: "soon", date };
  if (days < 7) {
    return { text: `Due ${day.toLocaleDateString([], { weekday: "short" })}`, cls: "soon", date };
  }
  return { text: `Due ${date}`, cls: "later", date };
}

export function dayLabel(ts: string): string {
  const key = dayKey(ts);
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (key === today.toDateString()) return "Today";
  if (key === yesterday.toDateString()) return "Yesterday";
  return new Date(ts).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
}

/// A file size for a chip: no decimals, because the exact byte count of an
/// attachment has never told anyone anything they wanted to know.
export function size(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${Math.round(bytes / (1024 * 1024))} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} kB`;
}

/* Tailwind's scanner only sees class names that appear as literals in the
   source, and band().cls / dueInfo().cls are chosen at runtime. These maps
   are where those runtime choices become literal utility strings -- a lookup
   rather than a safelist, so a band that stops being used stops being built. */

export const ALIGN_TEXT: Record<string, string> = {
  good: "text-good",
  warn: "text-warn",
  bad: "text-bad",
  none: "text-muted",
};

export const DUE_TEXT: Record<DueClass, string> = {
  overdue: "text-bad",
  today: "text-warn",
  soon: "text-text",
  later: "text-muted",
};
