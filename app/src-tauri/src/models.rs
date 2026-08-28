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

#[derive(Serialize)]
pub struct Observed {
    pub per_app: BTreeMap<String, f64>,
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
}

#[derive(Serialize)]
pub struct Health {
    pub schema_version: i64,
    pub needs_migration: bool,
    pub session: Option<Session>,
    pub aw_ok: bool,
}
