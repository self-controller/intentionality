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
  labels: Label[];
}

export interface Board {
  session: Session | null;
  tasks: Task[];
  backlog: Task[];
  unseen: number;
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
}

export interface Arrangement {
  todo: number[];
  doing: number[];
  done: number[];
  dropped: number[];
}

/// A PipeWire capture node. `target` is what the backend hands pw-record —
/// the node name, which survives a device reconnect; `label` is only ever
/// shown to a person.
export interface AudioSource {
  target: string;
  label: string;
}

/// One 100 ms window from the capture meter, as `meeting:level` carries it.
/// Input is measured before the automatic gain and output after it, so a
/// healthy `output_rms_dbfs` over a floor-level `input_rms_dbfs` reads as what
/// it is: amplification, not a microphone that can hear the room.
export interface Level {
  input_rms_dbfs: number;
  output_rms_dbfs: number;
  output_peak_dbfs: number;
  gain_db: number;
}

// Two wrap-up states, not one: the transcript is repaired before the notes are
// written, and the repair is the long half.
export type MeetingState =
  | "recording"
  | "cleaning"
  | "summarizing"
  | "done"
  | "failed";

// A file attached as context. No path and no extracted text: the webview does
// not learn where the copy lives, and never gets the contents back out.
export interface MeetingFile {
  id: number;
  position: number;
  name: string;
  kind: "text" | "pdf" | "image" | "office";
  bytes: number;
  added_at: string;
}

export interface Meeting {
  id: number;
  session_id: number | null; // null = recorded between sessions
  started_at: string;
  ended_at: string | null;
  title: string;
  state: MeetingState;
  error: string | null;
  segment_count: number;
  file_count: number;
  has_clean: boolean;
}

// The microphone is open on this meeting, and has been since `since`. Not the
// meeting's started_at: a meeting picked back up a day later has been
// recording for minutes, not a day.
export interface RecordingNow {
  meeting_id: number;
  since: string;
}

export interface MeetingSegment {
  id: number;
  seq: number;
  started_at: string;
  text: string; // "" when that chunk's transcription failed
}

export interface MeetingAction {
  id: number;
  position: number;
  text: string;
  // null until approved; a number is the backlog task it became, which is why
  // an already-approved item can be shown as done rather than offered again.
  task_id: number | null;
}

// What attaching returned: the meeting's full chip list, plus one readable
// line per file that was refused. Partial success is the normal outcome.
export interface AttachOutcome {
  files: MeetingFile[];
  rejected: string[];
}

export interface MeetingDetail {
  meeting: Meeting;
  segments: MeetingSegment[];
  actions: MeetingAction[];
  // What the user typed. Never model output.
  notes: string;
  // The write-up: one markdown document, the model's draft until the user
  // edits it. null until the model has run.
  summary: string | null;
  // When the user last saved an edit to `summary`; null while it is as the
  // model wrote it. Re-run asks before overwriting an edited document.
  summary_edited_at: string | null;
  // The repair pass's output; null until it has run. The segments above stay
  // the record of what was actually heard.
  clean_transcript: string | null;
  files: MeetingFile[];
}
