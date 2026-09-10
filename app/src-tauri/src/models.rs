use crate::build_info::BuildInfo;
use crate::recommendations::Recommendation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Clone)]
pub struct Session {
    pub id: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub close_reason: Option<String>,
    pub statement: String,
    pub intended_minutes: Option<i64>,
    /// When the time-up checkpoint fires; None once it has, or for an
    /// open-ended session that never gets one.
    pub checkpoint_due_at: Option<String>,
    /// Sessions committed since the gate stopped asking for a statement have
    /// an empty one; these two label them by their tasks instead.
    pub first_task: Option<String>,
    pub task_count: i64,
}

/// One PipeWire capture node, for the source selector.
#[derive(Serialize, Debug)]
pub struct AudioSource {
    /// What `pw-record --target` is given: the node name. Never the object
    /// serial, which is reassigned when a device reconnects.
    pub target: String,
    /// What a person reads. Never passed to pw-record.
    pub label: String,
}

#[derive(Serialize)]
pub struct Task {
    pub id: i64,
    pub session_id: Option<i64>,
    pub title: String,
    pub position: i64,
    pub status: String,
    pub carry_count: i64, // 0 for uncarried tasks
}

#[derive(Serialize)]
pub struct Board {
    pub session: Option<Session>,
    pub tasks: Vec<Task>,   // current session's cards; empty in no-session mode
    pub backlog: Vec<Task>, // session_id NULL
    pub unseen: i64,        // unread analyses for the badge
}

/// The entire post-drop board, written atomically. One command instead of
/// separate move/reorder calls means the board can never drift from the DB.
#[derive(Deserialize)]
pub struct Arrangement {
    pub todo: Vec<i64>,
    pub doing: Vec<i64>,
    pub done: Vec<i64>,
    pub dropped: Vec<i64>,
}

/// Also read back out of analysis.observed_json, which is why it deserializes.
/// `default` is load-bearing there: rows predating per_title have no such key,
/// and add_analysis stores a literal "{}" when serialization fails.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Observed {
    pub per_app: BTreeMap<String, f64>,
    pub per_title: BTreeMap<String, BTreeMap<String, f64>>,
    pub active_seconds: f64,
    pub afk_seconds: f64,
}

#[derive(Serialize)]
pub struct Analysis {
    pub id: i64,
    pub session_id: i64,
    pub created_at: String,
    pub window_start: String,
    pub window_end: String,
    pub headline: String,
    pub alignment: Option<i64>,
    pub body: String,
    pub seen_at: Option<String>,
    pub session_statement: Option<String>, // joined; None only if the session row vanished
    pub kind: String,                      // "check" | "checkpoint"
    /// Resolved from the stored recommendation_id at read time, so the
    /// frontend never carries a copy of the catalog and a reworded entry
    /// applies to every checkpoint already recorded. None on plain checks, and
    /// on a stored id the catalog no longer knows.
    pub recommendation: Option<Recommendation>,
    pub recommendation_note: Option<String>,
}

#[derive(Serialize)]
pub struct Health {
    pub schema_version: i64,
    pub needs_migration: bool,
    pub session: Option<Session>,
    pub aw_ok: bool,
    pub build: BuildInfo,
}

/// A transcribed meeting. `summary`/`key_points` stay None until the model has
/// run; `state` says why, so a meeting whose notes failed still displays with
/// its transcript rather than looking empty.
#[derive(Serialize)]
pub struct Meeting {
    pub id: i64,
    pub session_id: Option<i64>, // None = recorded between sessions
    pub started_at: String,
    pub ended_at: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub key_points: Vec<String>, // stored as a JSON array; [] until summarized
    pub state: String,           // "recording" | "summarizing" | "done" | "failed"
    pub error: Option<String>,
    pub segment_count: i64,
}

#[derive(Serialize)]
pub struct MeetingSegment {
    pub id: i64,
    pub seq: i64,
    pub started_at: String,
    pub text: String, // "" when that chunk's transcription failed
}

#[derive(Serialize)]
pub struct MeetingAction {
    pub id: i64,
    pub position: i64,
    pub text: String,
    /// None until approved. Some(id) is the record that this action already
    /// became a backlog task, which is what stops a second approval from
    /// inserting it twice.
    pub task_id: Option<i64>,
}

/// One meeting with everything the detail pane needs, in a single command:
/// the transcript arrives as segments so the UI can show which chunk failed.
#[derive(Serialize)]
pub struct MeetingDetail {
    pub meeting: Meeting,
    pub segments: Vec<MeetingSegment>,
    pub actions: Vec<MeetingAction>,
}
