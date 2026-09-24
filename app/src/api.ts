// The only file that talks to the Rust side.
import { invoke } from "@tauri-apps/api/core";
import type {
  Analysis,
  Arrangement,
  AttachOutcome,
  AudioSource,
  Board,
  Health,
  LabelSummary,
  Meeting,
  MeetingDetail,
  Observed,
  RecordingNow,
  Session,
  Task,
} from "./types";

export const health = () => invoke<Health>("health");
export const getBoard = () => invoke<Board>("get_board");
export const applyBoard = (arrangement: Arrangement) => invoke<void>("apply_board", { arrangement });
export const addTask = (title: string, toBacklog: boolean) =>
  invoke<number>("add_task", { title, toBacklog });
// A new card with the editor's details, written in one go.
export const createTask = (
  title: string,
  notes: string,
  dueDate: string | null,
  labels: string[],
  toBacklog: boolean,
) => invoke<number>("create_task", { title, notes, dueDate, labels, toBacklog });
// The editor saves the whole card at once; `labels` is the full set it should
// end up wearing, not an addition, and a null `dueDate` clears the date.
export const updateTask = (
  id: number,
  title: string,
  notes: string,
  dueDate: string | null,
  labels: string[],
) => invoke<void>("update_task", { id, title, notes, dueDate, labels });
// Every label there is, worn or not, with how many tasks wear each.
export const listLabels = () => invoke<LabelSummary[]>("list_labels");
// Takes the label off every card that wears it.
export const deleteLabel = (name: string) => invoke<void>("delete_label", { name });
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
export const startMeeting = (target?: string) =>
  invoke<RecordingNow>("start_meeting", { target });
// Open the microphone again on a finished meeting. The next Stop rewrites its
// notes from the whole transcript.
export const resumeMeeting = (meetingId: number, target?: string) =>
  invoke<RecordingNow>("resume_meeting", { meetingId, target });
export const stopMeeting = () => invoke<number>("stop_meeting");
export const recordingMeeting = () => invoke<RecordingNow | null>("recording_meeting");
export const listMeetings = (limit: number) => invoke<Meeting[]>("list_meetings", { limit });
export const getMeeting = (meetingId: number) =>
  invoke<MeetingDetail>("get_meeting", { meetingId });
export const approveActions = (meetingId: number, actionIds: number[]) =>
  invoke<number>("approve_actions", { meetingId, actionIds });
export const setMeetingNotes = (meetingId: number, notes: string) =>
  invoke<void>("set_meeting_notes", { meetingId, notes });
// The whole transcript, replaced. Refused while recording; the box is
// read-only then anyway.
export const setMeetingTranscript = (meetingId: number, text: string) =>
  invoke<void>("set_meeting_transcript", { meetingId, text });
// The write-up. Refused while the notes are being written; the editor is
// hidden then anyway.
export const setMeetingSummary = (meetingId: number, summary: string) =>
  invoke<void>("set_meeting_summary", { meetingId, summary });
export const rerunMeetingNotes = (meetingId: number) =>
  invoke<void>("rerun_meeting_notes", { meetingId });
// No paths in either direction: the picker opens on the Rust side and only
// metadata comes back.
export const attachMeetingFiles = (meetingId: number) =>
  invoke<AttachOutcome>("attach_meeting_files", { meetingId });
export const removeMeetingFile = (meetingId: number, fileId: number) =>
  invoke<void>("remove_meeting_file", { meetingId, fileId });
export const deleteMeeting = (meetingId: number) => invoke<void>("delete_meeting", { meetingId });
