// The only file that talks to the Rust side.
import { invoke } from "@tauri-apps/api/core";
import type {
  Analysis,
  AnalysisDetail,
  Arrangement,
  Board,
  DayTime,
  Health,
  Judgement,
  LabelSummary,
  Priority,
  IgnoreKind,
  IgnoreRule,
  InstalledApp,
} from "./types";

export const health = () => invoke<Health>("health");
export const getBoard = () => invoke<Board>("get_board");
export const applyBoard = (arrangement: Arrangement) => invoke<void>("apply_board", { arrangement });
// The backlog's ids, top to bottom.
export const orderBacklog = (ids: number[]) => invoke<void>("order_backlog", { ids });
export const addTask = (title: string, toBacklog: boolean) =>
  invoke<number>("add_task", { title, toBacklog });
// A new card with the editor's details, written in one go.
export const createTask = (
  title: string,
  notes: string,
  dueDate: string | null,
  priority: Priority,
  labels: string[],
  toBacklog: boolean,
) => invoke<number>("create_task", { title, notes, dueDate, priority, labels, toBacklog });
// The editor saves the whole card at once; `labels` is the full set it should
// end up wearing, not an addition, and a null `dueDate` clears the date.
export const updateTask = (
  id: number,
  title: string,
  notes: string,
  dueDate: string | null,
  priority: Priority,
  labels: string[],
) => invoke<void>("update_task", { id, title, notes, dueDate, priority, labels });
// Every label there is, worn or not, with how many tasks wear each.
export const listLabels = () => invoke<LabelSummary[]>("list_labels");
// Recolours the label on every card that wears it. "#rrggbb".
export const setLabelColor = (name: string, color: string) =>
  invoke<void>("set_label_color", { name, color });
// Takes the label off every card that wears it.
export const deleteLabel = (name: string) => invoke<void>("delete_label", { name });
// The ignore list: apps and sites never judged. Each write returns the list.
export const ignoreList = () => invoke<IgnoreRule[]>("ignore_list");
export const addIgnore = (rule: IgnoreRule) => invoke<IgnoreRule[]>("add_ignore", { rule });
export const removeIgnore = (kind: IgnoreKind, value: string) =>
  invoke<IgnoreRule[]>("remove_ignore", { kind, value });
// Every installed app, A to Z, from the .desktop files (scanned afresh).
export const installedApps = () => invoke<InstalledApp[]>("installed_apps");
// A page as the relevance cache knows it; asks the model nothing.
export const pageJudgement = (windowApp: string, title: string) =>
  invoke<Judgement>("page_judgement", { windowApp, title });
// The user's whole case so far, put to Jev with every task; stored.
export const appeal = (windowApp: string, title: string, argument: string[]) =>
  invoke<Judgement>("appeal", { windowApp, title, argument });
// Every cached verdict gone; returns how many there were.
export const forgetRelevance = () => invoke<number>("forget_relevance");
// A day for the Dashboard ("YYYY-MM-DD"; null = today), from the relevance
// cache alone: free to poll.
export const dayTime = (day: string | null) => invoke<DayTime>("day_time", { day });
// Jev asked about the day's busiest unjudged pages (billed), then the day again.
export const judgeDay = (day: string | null) => invoke<DayTime>("judge_day", { day });
// The hour (0-23) a day starts at, for the Dashboard and the clock's total.
export const dayStartHour = () => invoke<number>("day_start_hour");
export const setDayStartHour = (hour: number) => invoke<number>("set_day_start_hour", { hour });
export const deleteTask = (id: number) => invoke<void>("delete_task", { id });
export const pullTask = (id: number) => invoke<void>("pull_task", { id });
// A card on the current board back to the end of the backlog, unstarted.
export const unpullTask = (id: number) => invoke<void>("unpull_task", { id });
// How far the Analyses tab's history reaches. One constant so the tab and the
// unread badge always count the same rows.
export const RECENT_ANALYSES = 50;
export const listRecentAnalyses = (limit: number) =>
  invoke<Analysis[]>("list_recent_analyses", { limit });
export const getAnalysisDetail = (id: number) =>
  invoke<AnalysisDetail>("get_analysis_detail", { id });
export const markAnalysisSeen = (id: number) => invoke<void>("mark_analysis_seen", { id });
export const runAnalysisNow = () => invoke<number | null>("run_analysis_now");
export const pendingCheckpoint = () => invoke<Analysis | null>("pending_checkpoint");
export const extendCheckpoint = (minutes: number) =>
  invoke<string>("extend_checkpoint", { minutes });
export const runCheckpointNow = () => invoke<number | null>("run_checkpoint_now");

/** Keyboard shortcuts changed from their defaults (keybinds.rs). */
export const keybinds = () => invoke<Record<string, string>>("keybinds");
export const setKeybinds = (keybinds: Record<string, string>) =>
  invoke<Record<string, string>>("set_keybinds", { keybinds });
