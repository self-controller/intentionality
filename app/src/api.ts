// The only file that talks to the Rust side.
import { invoke } from "@tauri-apps/api/core";
import type {
  Analysis,
  AnalysisDetail,
  Arrangement,
  Board,
  Health,
  Judgement,
  LabelSummary,
  Observed,
  Priority,
  RiskApp,
  RiskToday,
  Session,
  Task,
  TodayTime,
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
// The at-risk apps: what is flagged, and today's time on each.
export const riskToday = () => invoke<RiskToday>("risk_today");
export const addRiskApp = (name: string) => invoke<RiskApp>("add_risk_app", { name });
export const removeRiskApp = (id: number) => invoke<void>("remove_risk_app", { id });
// A page as the relevance cache knows it; asks the model nothing.
export const pageJudgement = (windowApp: string, title: string) =>
  invoke<Judgement>("page_judgement", { windowApp, title });
// The user's whole case so far, put to Jev with every task; stored.
export const appeal = (windowApp: string, title: string, argument: string[]) =>
  invoke<Judgement>("appeal", { windowApp, title, argument });
// Every cached verdict gone; returns how many there were.
export const forgetRelevance = () => invoke<number>("forget_relevance");
// Today for the Dashboard, from the relevance cache alone: free to poll.
export const todayTime = () => invoke<TodayTime>("today_time");
// Jev asked about today's busiest unjudged pages (billed), then today again.
export const judgeToday = () => invoke<TodayTime>("judge_today");
export const deleteTask = (id: number) => invoke<void>("delete_task", { id });
export const pullTask = (id: number) => invoke<void>("pull_task", { id });
// A card on the current board back to the end of the backlog, unstarted.
export const unpullTask = (id: number) => invoke<void>("unpull_task", { id });
export const listSessions = (limit: number) => invoke<Session[]>("list_sessions", { limit });
export const getSessionTasks = (sessionId: number) =>
  invoke<Task[]>("get_session_tasks", { sessionId });
export const getObserved = (sessionId: number) => invoke<Observed>("get_observed", { sessionId });
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
