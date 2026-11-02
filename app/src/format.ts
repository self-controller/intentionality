import type { Observed } from "./types";

// The backend caps each app's titles and rolls the long tail into "(other)",
// which can outweigh every real title — so it is pinned last rather than
// sorted by size. BTreeMap serializes alphabetically, so ranking happens here.
export const OTHER = "(other)";

// Parts of an app id that say nothing about which app it is.
const ID_NOISE = new Set(["app", "org", "com", "net", "www", "desktop"]);

const words = (s: string) =>
  s
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter((w) => w.length >= 3);

/** A window title without the app's own name, which browsers and editors
 *  append ("… — Zen Browser", "… - Visual Studio Code"): the chart is about
 *  what was done, and the app says nothing about that. The last dash-separated
 *  segment goes only when it names the app (shares a word with its id), so
 *  "Board.tsx - intentionality" keeps its project, and "|" never splits. */
export function trimApp(title: string, app: string): string {
  // VS Code's unsaved-changes dot, which would split one file into two rows.
  const t = title.replace(/^[●•]\s+/, "");
  const m = /^(.*\S)\s+[—–-]\s+(\S.*)$/.exec(t);
  if (!m) return t;
  const id = new Set(words(app).filter((w) => !ID_NOISE.has(w)));
  return words(m[2]).some((w) => id.has(w)) ? m[1] : t;
}

export interface Activity {
  label: string;
  // The title as the window had it, for the hover text.
  hint: string;
  secs: number;
  // Inside the analysis prompt's cut; always, when no cut is given.
  sent: boolean;
}

/** Observed time as the things that were done -- each window or page its own
 *  row, ranked across every app -- rather than the apps they were done in. A
 *  browser's three hours could be research or drift; its titles say which.
 *
 *  `cut` is the analysis prompt's: its top `apps` apps, and each one's top
 *  `titles` titles. Rows sharing a label are one row, sent if any part was.
 *  Every app's "(other)" sums into `other`, which callers pin last. */
export function activities(
  observed: Observed,
  cut?: { apps: number; titles: number },
): { ranked: Activity[]; other: Activity | null } {
  const rows = new Map<string, Activity>();
  const add = (label: string, hint: string, secs: number, sent: boolean) => {
    const row = rows.get(label);
    if (row) {
      row.secs += secs;
      row.sent ||= sent;
    } else rows.set(label, { label, hint, secs, sent });
  };
  let other: Activity | null = null;

  const apps = Object.entries(observed.per_app).sort((a, b) => b[1] - a[1]);
  for (const [appRank, [app, appSecs]] of apps.entries()) {
    const appSent = !cut || appRank < cut.apps;
    const titles = Object.entries(observed.per_title?.[app] ?? {}).sort((a, b) => b[1] - a[1]);
    // Rows from before titles were recorded have only the app.
    if (titles.length === 0) {
      add(app, app, appSecs, appSent);
      continue;
    }
    let rank = 0;
    for (const [title, secs] of titles) {
      if (title === OTHER) {
        other ??= { label: "Other", hint: "Windows too brief to list one by one", secs: 0, sent: false };
        other.secs += secs;
        // The prompt sends it as each app's remainder line.
        other.sent ||= appSent;
        continue;
      }
      const sent = appSent && (!cut || rank < cut.titles);
      rank++;
      // observed.rs's stand-in for a window with no title.
      add(title === "unknown" ? app : trimApp(title, app) || app, title, secs, sent);
    }
  }
  return { ranked: [...rows.values()].sort((a, b) => b.secs - a.secs), other };
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

// "YYYY-MM-DD" as a local Date at midnight (new Date("2026-10-31") would be
// UTC midnight, the previous evening here).
export function parseDay(day: string): Date {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d);
}

// The day `by` days from `day`, both "YYYY-MM-DD".
export function shiftDay(day: string, by: number): string {
  return localDate(addDays(parseDay(day), by));
}

// An hour of the clock in words: "midnight", "4 am", "noon", "3 pm".
export function hourLabel(hour: number): string {
  if (hour === 0) return "midnight";
  if (hour === 12) return "noon";
  return hour < 12 ? `${hour} am` : `${hour - 12} pm`;
}

export function addDays(d: Date, n: number): Date {
  const out = new Date(d);
  out.setDate(out.getDate() + n);
  return out;
}

export type DueClass = "overdue" | "today" | "soon" | "later";

// A due date in words, relative to today. The words carry the meaning and the
// class only colours it, the same rule band() follows. `date` is the plain
// short date, for a finished card that should keep the date but not the alarm.
export function dueInfo(due: string, now = new Date()): { text: string; cls: DueClass; date: string } {
  // Local midnight: new Date("2026-09-18") would be UTC midnight, which is
  // the previous evening here.
  const [y, m, d] = due.split("-").map(Number);
  const day = new Date(y, m - 1, d);
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

/* Tailwind's scanner only sees class names that appear as literals in the
   source, and band().cls / dueInfo().cls are chosen at runtime. These maps
   are where those runtime choices become literal utility strings -- a lookup
   rather than a safelist, so a band that stops being used stops being built. */

export const ALIGN_TEXT: Record<string, string> = {
  good: "text-good",
  warn: "text-warn",
  bad: "text-destructive",
  none: "text-muted-foreground",
};

export const DUE_TEXT: Record<DueClass, string> = {
  overdue: "text-destructive",
  today: "text-warn",
  soon: "text-foreground",
  later: "text-muted-foreground",
};
