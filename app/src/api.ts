// The only file that talks to the Rust side.
import { invoke } from "@tauri-apps/api/core";
import type { Analysis, Arrangement, Board, Health, Observed, Session, Task } from "./types";

export const health = () => invoke<Health>("health");
export const getBoard = () => invoke<Board>("get_board");
export const applyBoard = (arrangement: Arrangement) => invoke<void>("apply_board", { arrangement });
export const addTask = (title: string, toBacklog: boolean) =>
  invoke<number>("add_task", { title, toBacklog });
export const renameTask = (id: number, title: string) => invoke<void>("rename_task", { id, title });
export const deleteTask = (id: number) => invoke<void>("delete_task", { id });
export const pullTask = (id: number) => invoke<void>("pull_task", { id });
export const listSessions = (limit: number) => invoke<Session[]>("list_sessions", { limit });
export const getSessionTasks = (sessionId: number) =>
  invoke<Task[]>("get_session_tasks", { sessionId });
export const getObserved = (sessionId: number) => invoke<Observed>("get_observed", { sessionId });
export const listAnalyses = (sessionId: number) =>
  invoke<Analysis[]>("list_analyses", { sessionId });
export const markAnalysisSeen = (id: number) => invoke<void>("mark_analysis_seen", { id });
export const runAnalysisNow = () => invoke<number | null>("run_analysis_now");
