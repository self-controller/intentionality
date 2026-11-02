export interface Session {
  id: number;
  started_at: string;
  ended_at: string | null;
  close_reason: string | null;
  statement: string;
  intended_minutes: number | null;
  checkpoint_due_at: string | null;
  // Empty statement since the gate stopped asking for one: these label the
  // session by its tasks instead.
  first_task: string | null;
  task_count: number;
}

export type Status = "planned" | "doing" | "done" | "dropped";

// How much a task matters. Every board column is ordered by it, high first,
// keeping the user's own order within each level.
export type Priority = "high" | "medium" | "low";

// A tag a task wears. The colour was fixed when the label was first created,
// so the same word looks the same on every card.
export interface Label {
  name: string;
  color: string;
}

// Every label there is, as the labels panel and the editor list them. A label
// nothing wears (uses 0) is a preset: it stays until it is deleted.
export interface LabelSummary extends Label {
  uses: number;
}

export interface Task {
  id: number;
  session_id: number | null;
  title: string;
  position: number;
  status: Status;
  carry_count: number;
  // Plain text the user wrote. Deliberately not markdown: this is a scratch
  // pad on a card, not a document.
  notes: string;
  // The day it is due, "YYYY-MM-DD" on the local calendar; null = none. A day,
  // not an instant, so it never drifts with the timezone.
  due_date: string | null;
  priority: Priority;
  labels: Label[];
}

export interface Board {
  session: Session | null;
  tasks: Task[];
  backlog: Task[];
}

export interface Observed {
  per_app: Record<string, number>;
  per_title: Record<string, Record<string, number>>;
  active_seconds: number;
  afk_seconds: number;
}

// Resolved from the stored recommendation_id on the Rust side, so this file
// is the only place the frontend knows the shape and no copy of the catalog
// lives here. `source` is the citation slot: null means "no research behind
// this yet", and the screen says so rather than implying otherwise.
export interface Recommendation {
  id: string;
  advice: string;
  why: string;
  minutes: number;
  source: string | null;
}

export interface Analysis {
  id: number;
  session_id: number;
  created_at: string;
  window_start: string;
  window_end: string;
  headline: string;
  alignment: number | null;
  body: string;
  seen_at: string | null;
  session_statement: string | null;
  kind: "check" | "checkpoint";
  recommendation: Recommendation | null;
  recommendation_note: string | null;
}

// Answers "which build is this?" without going looking — autostart runs a
// build artifact, so a stale binary is otherwise indistinguishable from a
// fresh one. stale_since is set only when the source is newer than the binary.
export interface BuildInfo {
  version: string;
  built_at: string | null;
  stale_since: string | null;
}

export interface Health {
  schema_version: number;
  needs_migration: boolean;
  session: Session | null;
  aw_ok: boolean;
  build: BuildInfo;
  // Set when the resume gate's systemd units are missing or differ from the
  // repo copies — the install is a sudo step, easy to leave undone unnoticed.
  resume_gate: string | null;
  // Set when the GNOME Shell extension that draws the at-risk timer is
  // missing or older than this build.
  overlay: string | null;
  // Set when ActivityWatch cannot see Wayland windows (Focused Window D-Bus
  // is off), which blinds the timer and every observation.
  window_tracking: string | null;
  // Set when pages cannot be judged for relevance (no TypeSafe key): the
  // timer then shows for every flagged page, captioned as unchecked.
  relevance: string | null;
}

// An app or website that is never judged (ignore.rs): no Jev request, no
// timer, its time "Not judged". An app is matched by its window classes, a
// site by its host and every part of it ("google.com" covers docs.google.com).
export type IgnoreKind = "app" | "site";

export interface IgnoreRule {
  kind: IgnoreKind;
  // An app: its desktop id or window class, lowercased. A site: its host.
  value: string;
  name: string;
  // An app's window classes, lowercased. Empty for a site.
  classes: string[];
}

// An app or site seen today, as the ignore menu offers it.
export interface IgnoreChoice extends IgnoreRule {
  seconds: number;
  ignored: boolean;
}

// An installed app, from its .desktop file.
export interface InstalledApp {
  id: string;
  name: string;
  classes: string[];
}

// One row of the culprits chart: a site, a tool and project ("Claude Code:
// Pager"), or an app, and today's not-relevant time on it.
export interface Culprit {
  name: string;
  seconds: number;
}

// One day across sessions, as the Dashboard shows it: today so far or a past
// day whole, from the day-start hour (Settings, 4 am by default) to the next.
// A page is relevant when a task it is judged against clears the bar -- the
// timer's own rule -- and is that likeliest task's time. Today that is Doing
// now; a past day, each session's Doing as it was left; either way plus the
// tasks finished that day. unjudged includes outside (active time in no
// session).
export interface DayTime {
  // The day shown and today, "YYYY-MM-DD", each named by the date it began on.
  day: string;
  today: string;
  // The hour (0-23) days start at.
  start_hour: number;
  // The window read: the day's start, and now or the next day's start.
  start: string;
  end: string;
  active_seconds: number;
  afk_seconds: number;
  aligned_seconds: number;
  off_seconds: number;
  unjudged_seconds: number;
  outside_seconds: number;
  // Relevant time by task in board order (Doing, then finished that day), not
  // by size: the Dashboard colours each task by its place. Carried copies
  // are one row.
  tasks: TaskTime[];
  // Unjudged pages a "Judge" press would ask Jev about (up to 30).
  askable: number;
  threshold: number;
  // Why the last judge pass could not judge some pages.
  error: string | null;
  // Where the not-relevant time went, busiest first.
  culprits: Culprit[];
  // Every app and site seen that day, busiest first: the ignore menu's list.
  seen: IgnoreChoice[];
  // Rows in the relevance cache: pages × tasks Jev has judged.
  judged: number;
}

// Jev's verdict on one page for one task on the board. `probability` is null
// while the task has not been judged against this page.
export interface Verdict {
  task_id: number;
  task_title: string;
  probability: number | null;
  source: "model" | "appeal" | null;
  model: string | null;
  created_at: string | null;
}

// A page judged against the session's unfinished tasks, in board order.
// `relevant` indexes the verdict that excuses the page, if one does.
export interface Judgement {
  app: string;
  page: string;
  title: string;
  verdicts: Verdict[];
  relevant: number | null;
  threshold: number;
  no_tasks: boolean;
  error: string | null;
}

// What the clock's arrow was clicked over: the flag, and the window as the
// shell saw it. The payload of the `risk:appeal` event. `from` is the
// frontend's own: "check" when the Analyses tab opened the appeal instead.
export interface PageRef {
  label: string;
  app: string;
  title: string;
  from?: "timer" | "check";
}

// What Jev judged for one check: analysis.judged_json, a snapshot taken when
// the row was written. Null on every row from before v14 (Claude wrote
// those), which the tab shows the old way.
export interface Judged {
  models: string[];
  threshold: number;
  trigger: "scheduled" | "manual" | "drift" | "checkpoint";
  tasks: TaskTime[];
  pages: JudgedPage[];
  aligned_seconds: number;
  off_seconds: number;
  unjudged_seconds: number;
  risk: RiskTime[];
  pick: Pick | null;
}

export interface TaskTime {
  task_id: number;
  title: string;
  seconds: number;
}

export interface JudgedPage {
  app: string;
  title: string; // as recorded: what an appeal is opened for
  name: string; // as shown: unread count and browser name off
  page: string;
  seconds: number;
  verdict: "excused" | "no_task" | "unjudged";
  // The excusing verdict, or else the closest one.
  task_id: number | null;
  task_title: string | null;
  probability: number | null;
  source: "model" | "appeal" | null;
}

export interface RiskTime {
  name: string;
  seconds: number;
  excused_seconds: number;
  no_task_seconds: number;
  unjudged_seconds: number;
}

export interface Pick {
  id: string;
  probabilities: Record<string, number>;
  confidence: number;
}

export interface AnalysisDetail {
  observed: Observed;
  judged: Judged | null;
}

export interface Arrangement {
  todo: number[];
  doing: number[];
  done: number[];
  dropped: number[];
}
