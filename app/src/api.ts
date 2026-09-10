// The only file that talks to the Rust side.
import { invoke } from "@tauri-apps/api/core";
import type {
  Analysis,
  Arrangement,
  AudioSource,
  Board,
  Health,
  Meeting,
  MeetingDetail,
  Observed,
  Session,
  Task,
} from "./types";

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
// How far the Analyses tab's history reaches. One constant so the tab and the
// unread badge always count the same rows.
export const RECENT_ANALYSES = 50;
export const listRecentAnalyses = (limit: number) =>
  invoke<Analysis[]>("list_recent_analyses", { limit });
export const getAnalysisObserved = (id: number) =>
  invoke<Observed>("get_analysis_observed", { id });
export const markAnalysisSeen = (id: number) => invoke<void>("mark_analysis_seen", { id });
export const runAnalysisNow = () => invoke<number | null>("run_analysis_now");
export const pendingCheckpoint = () => invoke<Analysis | null>("pending_checkpoint");
export const extendCheckpoint = (minutes: number) =>
  invoke<string>("extend_checkpoint", { minutes });
export const runCheckpointNow = () => invoke<number | null>("run_checkpoint_now");

// How far the Meetings tab's history reaches.
export const RECENT_MEETINGS = 50;
// Empty when PipeWire is unavailable or has nothing to offer; that is a valid
// answer and the screen falls back to the system default rather than failing.
export const listAudioSources = () => invoke<AudioSource[]>("list_audio_sources");
// `target` is a PipeWire node name; undefined means the system default source.
export const startMeeting = (target?: string) => invoke<number>("start_meeting", { target });
export const stopMeeting = () => invoke<number>("stop_meeting");
export const recordingMeeting = () => invoke<number | null>("recording_meeting");
export const listMeetings = (limit: number) => invoke<Meeting[]>("list_meetings", { limit });
export const getMeeting = (meetingId: number) =>
  invoke<MeetingDetail>("get_meeting", { meetingId });
export const approveActions = (meetingId: number, actionIds: number[]) =>
  invoke<number>("approve_actions", { meetingId, actionIds });
export const resummarizeMeeting = (meetingId: number) =>
  invoke<void>("resummarize_meeting", { meetingId });
export const deleteMeeting = (meetingId: number) => invoke<void>("delete_meeting", { meetingId });
