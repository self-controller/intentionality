//! All SQLite access. Mirrors gate/store.py's contract: schema migrations
//! belong to Python; this side only checks meta.schema_version and refuses
//! politely when the store is older than it understands.

use crate::error::{AppError, Result};
use crate::models::{
    Analysis, Arrangement, Meeting, MeetingAction, MeetingSegment, Observed, Session, Task,
};
use crate::recommendations;
use chrono::{Duration, Utc};
use rusqlite::{params, Connection, TransactionBehavior};
use std::path::PathBuf;

pub const SCHEMA_VERSION: i64 = 4;
pub const MIGRATE_HINT: &str = "store schema is out of date — run: python3 -m gate migrate";

pub fn now() -> String {
    // Match gate/store.py's format exactly (timespec="seconds", +00:00).
    Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

pub fn store_path() -> PathBuf {
    std::env::var("INTENTIONALITY_STORE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local/share/intentionality/store.db")
        })
}

pub fn open() -> Result<Connection> {
    let conn = Connection::open(store_path())?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
    Ok(conn)
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    let v: String = conn
        .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
        .map_err(|_| AppError::Other(MIGRATE_HINT.into()))?;
    v.parse().map_err(|_| AppError::Other(MIGRATE_HINT.into()))
}

pub fn check_schema(conn: &Connection) -> Result<()> {
    if schema_version(conn)? < SCHEMA_VERSION {
        return Err(AppError::Other(MIGRATE_HINT.into()));
    }
    Ok(())
}

// The gate stopped asking for a statement, so a session is identified by its
// first task instead — the SQL twin of gate/store.py::session_label. Every
// query feeding row_session must supply the two derived columns, so they all
// go through this SELECT.
const SESSION_SELECT: &str = "SELECT s.*,
        (SELECT title FROM task WHERE session_id = s.id ORDER BY position LIMIT 1)
            AS first_task,
        (SELECT COUNT(*) FROM task WHERE session_id = s.id) AS task_count
      FROM session s";

fn row_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        close_reason: row.get("close_reason")?,
        statement: row.get("statement")?,
        intended_minutes: row.get("intended_minutes")?,
        checkpoint_due_at: row.get("checkpoint_due_at")?,
        first_task: row.get("first_task")?,
        task_count: row.get("task_count")?,
    })
}

pub fn get_session(conn: &Connection, id: i64) -> Result<Option<Session>> {
    conn.query_row(
        &format!("{SESSION_SELECT} WHERE s.id = ?1"),
        [id],
        row_session,
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        e => Err(e.into()),
    })
}

pub fn latest_open_session(conn: &Connection) -> Result<Option<Session>> {
    conn.query_row(
        &format!(
            "{SESSION_SELECT} WHERE s.ended_at IS NULL AND s.close_reason IS NULL
             ORDER BY s.id DESC LIMIT 1"
        ),
        [],
        row_session,
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        e => Err(e.into()),
    })
}

pub fn list_sessions(conn: &Connection, limit: i64) -> Result<Vec<Session>> {
    let mut stmt =
        conn.prepare(&format!("{SESSION_SELECT} ORDER BY s.id DESC LIMIT ?1"))?;
    let rows = stmt.query_map([limit], row_session)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// carry_count follows the carried_from chain — the read-only twin of
// gate/store.py::carry_count.
const TASK_SELECT: &str = "
    SELECT t.id, t.session_id, t.title, t.position, t.status,
        (WITH RECURSIVE chain(cid) AS (
            SELECT carried_from FROM task WHERE id = t.id AND carried_from IS NOT NULL
            UNION ALL
            SELECT task.carried_from FROM task JOIN chain ON task.id = chain.cid
            WHERE task.carried_from IS NOT NULL)
         SELECT COUNT(*) FROM chain) AS carry_count
    FROM task t";

fn row_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        title: row.get("title")?,
        position: row.get("position")?,
        status: row.get("status")?,
        carry_count: row.get("carry_count")?,
    })
}

pub fn session_tasks(conn: &Connection, session_id: i64) -> Result<Vec<Task>> {
    let mut stmt =
        conn.prepare(&format!("{TASK_SELECT} WHERE t.session_id = ?1 ORDER BY t.position"))?;
    let rows = stmt.query_map([session_id], row_task)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn backlog(conn: &Connection) -> Result<Vec<Task>> {
    let mut stmt =
        conn.prepare(&format!("{TASK_SELECT} WHERE t.session_id IS NULL ORDER BY t.position"))?;
    let rows = stmt.query_map([], row_task)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn apply_board(conn: &mut Connection, session_id: i64, arr: &Arrangement) -> Result<()> {
    let ts = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (status, ids) in [
        ("planned", &arr.todo),
        ("doing", &arr.doing),
        ("done", &arr.done),
        ("dropped", &arr.dropped),
    ] {
        for (pos, id) in ids.iter().enumerate() {
            // Guarded by session_id: a stale frontend can't touch other
            // sessions' history. started_at records the FIRST entry into
            // doing-or-beyond; resolved_at only stands on terminal statuses.
            let changed = tx.execute(
                "UPDATE task SET status = ?1, position = ?2,
                    started_at  = CASE WHEN ?1 IN ('doing','done')
                                       THEN COALESCE(started_at, ?3) ELSE started_at END,
                    resolved_at = CASE WHEN ?1 IN ('done','dropped')
                                       THEN COALESCE(resolved_at, ?3) ELSE NULL END
                 WHERE id = ?4 AND session_id = ?5",
                params![status, (pos + 1) as i64, ts, id, session_id],
            )?;
            if changed == 0 {
                return Err(AppError::Other(format!("task {id} is not on this board")));
            }
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn add_task(conn: &Connection, session_id: Option<i64>, title: &str) -> Result<i64> {
    let next: i64 = match session_id {
        Some(sid) => conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM task WHERE session_id = ?1",
            [sid],
            |r| r.get(0),
        )?,
        None => conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM task WHERE session_id IS NULL",
            [],
            |r| r.get(0),
        )?,
    };
    conn.execute(
        "INSERT INTO task (session_id, title, position, source, created_at)
         VALUES (?1, ?2, ?3, 'mid-session', ?4)",
        params![session_id, title, next, now()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_task(conn: &Connection, id: i64, title: &str) -> Result<()> {
    conn.execute("UPDATE task SET title = ?1 WHERE id = ?2", params![title, id])?;
    Ok(())
}

/// Deletion is for backlog rows only — session cards are history and get
/// dropped, never deleted.
pub fn delete_backlog_task(conn: &Connection, id: i64) -> Result<()> {
    let changed = conn.execute(
        "DELETE FROM task WHERE id = ?1 AND session_id IS NULL",
        [id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("only backlog items can be deleted".into()));
    }
    Ok(())
}

/// Backlog -> current session, mid-session. The IS NULL guard makes moving
/// another session's row impossible.
pub fn pull_task(conn: &Connection, session_id: i64, id: i64) -> Result<()> {
    let next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM task WHERE session_id = ?1",
        [session_id],
        |r| r.get(0),
    )?;
    let changed = conn.execute(
        "UPDATE task SET session_id = ?1, position = ?2, source = 'mid-session'
         WHERE id = ?3 AND session_id IS NULL",
        params![session_id, next, id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("not a backlog item".into()));
    }
    Ok(())
}

/// Returns false once the session is closed — the loop's stop signal. Mirrors
/// gate/store.py::heartbeat: a closed session can never be resurrected.
pub fn heartbeat(conn: &Connection, session_id: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE session SET last_heartbeat = ?1 WHERE id = ?2 AND ended_at IS NULL",
        params![now(), session_id],
    )?;
    Ok(changed > 0)
}

fn row_analysis(row: &rusqlite::Row) -> rusqlite::Result<Analysis> {
    Ok(Analysis {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        created_at: row.get("created_at")?,
        window_start: row.get("window_start")?,
        window_end: row.get("window_end")?,
        headline: row.get("headline")?,
        alignment: row.get("alignment")?,
        body: row.get("body")?,
        seen_at: row.get("seen_at")?,
        session_statement: row.get("session_statement")?,
        kind: row.get("kind")?,
        // get(), not resolve(): a stored id the catalog no longer knows reads
        // as "no recommendation" rather than quietly becoming a different one.
        recommendation: row
            .get::<_, Option<String>>("recommendation_id")?
            .and_then(|id| recommendations::get(&id)),
        recommendation_note: row.get("recommendation_note")?,
    })
}

/// Both listings share row_analysis, so both must supply session_statement.
/// LEFT JOIN rather than INNER: an orphaned analysis should still be readable.
const ANALYSIS_SELECT: &str = "SELECT a.*, s.statement AS session_statement
     FROM analysis a LEFT JOIN session s ON s.id = a.session_id";

pub fn list_analyses(conn: &Connection, session_id: i64) -> Result<Vec<Analysis>> {
    let mut stmt =
        conn.prepare(&format!("{ANALYSIS_SELECT} WHERE a.session_id = ?1 ORDER BY a.id DESC"))?;
    let rows = stmt.query_map([session_id], row_analysis)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The Analyses tab's history: newest first, across every session. Ordered by
/// created_at rather than id — the tab groups rows by day, so it needs true
/// chronological order, which id only happens to give while nothing backfills.
pub fn list_recent_analyses(conn: &Connection, limit: i64) -> Result<Vec<Analysis>> {
    let mut stmt =
        conn.prepare(&format!("{ANALYSIS_SELECT} ORDER BY a.created_at DESC, a.id DESC LIMIT ?1"))?;
    let rows = stmt.query_map([limit], row_analysis)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The AW breakdown this analysis was judged against. A missing row or
/// unparseable JSON reads as an empty window rather than an error — the
/// analysis itself is still worth showing.
pub fn analysis_observed(conn: &Connection, id: i64) -> Result<Observed> {
    let json: Option<String> = conn
        .query_row("SELECT observed_json FROM analysis WHERE id = ?1", [id], |r| r.get(0))
        .ok();
    Ok(json
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default())
}

pub fn unseen_analyses(conn: &Connection, session_id: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM analysis WHERE session_id = ?1 AND seen_at IS NULL",
        [session_id],
        |r| r.get(0),
    )?)
}

pub fn mark_analysis_seen(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE analysis SET seen_at = ?1 WHERE id = ?2 AND seen_at IS NULL",
        params![now(), id],
    )?;
    Ok(())
}

/// Eleven positional arguments is not a function signature anyone can read at
/// a call site, and the checkpoint adds three more to what was already eight.
pub struct NewAnalysis<'a> {
    pub session_id: i64,
    pub kind: &'a str, // "check" | "checkpoint"
    pub window_start: &'a str,
    pub window_end: &'a str,
    pub headline: &'a str,
    pub alignment: Option<i64>,
    pub body: &'a str,
    pub observed_json: &'a str,
    pub recommendation_id: Option<&'a str>,
    pub recommendation_note: Option<&'a str>,
}

pub fn add_analysis(conn: &Connection, a: &NewAnalysis) -> Result<i64> {
    conn.execute(
        "INSERT INTO analysis (session_id, created_at, window_start, window_end,
             headline, alignment, body, observed_json, kind,
             recommendation_id, recommendation_note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            a.session_id, now(), a.window_start, a.window_end, a.headline,
            a.alignment, a.body, a.observed_json, a.kind,
            a.recommendation_id, a.recommendation_note
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn last_analysis_end(conn: &Connection, session_id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT window_end FROM analysis WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
            [session_id],
            |r| r.get(0),
        )
        .ok())
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .ok())
}

/// The checkpoint that is due, if one is. Non-NULL checkpoint_due_at in the
/// past is the whole condition — keeping it in the store rather than in memory
/// is what makes a checkpoint that came due while the app was down fire on the
/// next start instead of being lost.
pub fn checkpoint_due(conn: &Connection, session_id: i64) -> Result<bool> {
    let due: Option<String> = conn
        .query_row(
            "SELECT checkpoint_due_at FROM session WHERE id = ?1 AND ended_at IS NULL",
            [session_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    // Both sides are the store's one timestamp format, so a string compare is
    // a time compare — see gate/store.py::checkpoint_due.
    Ok(due.is_some_and(|at| at <= now()))
}

pub fn clear_checkpoint(conn: &Connection, session_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE session SET checkpoint_due_at = NULL WHERE id = ?1",
        [session_id],
    )?;
    Ok(())
}

/// Re-arm the checkpoint N minutes out. Guarded on ended_at like heartbeat:
/// a closed session must never get a pending checkpoint back.
pub fn extend_checkpoint(conn: &Connection, session_id: i64, minutes: i64) -> Result<String> {
    let at = (Utc::now() + Duration::minutes(minutes))
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string();
    let changed = conn.execute(
        "UPDATE session SET checkpoint_due_at = ?1 WHERE id = ?2 AND ended_at IS NULL",
        params![at, session_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("session is closed".into()));
    }
    Ok(at)
}

/// The checkpoint waiting to be acknowledged, if any. Read on app start so one
/// that fired during a restart is still shown rather than silently missed.
pub fn pending_checkpoint(conn: &Connection, session_id: i64) -> Result<Option<Analysis>> {
    conn.query_row(
        &format!(
            "{ANALYSIS_SELECT} WHERE a.session_id = ?1 AND a.kind = 'checkpoint'
             AND a.seen_at IS NULL ORDER BY a.id DESC LIMIT 1"
        ),
        [session_id],
        row_analysis,
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        e => Err(e.into()),
    })
}

// --- meetings ---------------------------------------------------------------

const MEETING_SELECT: &str = "
    SELECT m.id, m.session_id, m.started_at, m.ended_at, m.title, m.summary,
        m.key_points, m.state, m.error,
        (SELECT COUNT(*) FROM meeting_segment s WHERE s.meeting_id = m.id)
            AS segment_count
    FROM meeting m";

fn row_meeting(row: &rusqlite::Row) -> rusqlite::Result<Meeting> {
    // key_points is a JSON array written by this app. A row that somehow holds
    // anything else reads as no key points rather than failing the whole query:
    // the transcript is the part worth protecting.
    let key_points: Option<String> = row.get("key_points")?;
    let key_points = key_points
        .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
        .unwrap_or_default();
    Ok(Meeting {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        title: row.get("title")?,
        summary: row.get("summary")?,
        key_points,
        state: row.get("state")?,
        error: row.get("error")?,
        segment_count: row.get("segment_count")?,
    })
}

pub fn start_meeting(conn: &Connection, session_id: Option<i64>) -> Result<i64> {
    conn.execute(
        "INSERT INTO meeting (session_id, started_at, state) VALUES (?1, ?2, 'recording')",
        params![session_id, now()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_meeting(conn: &Connection, id: i64) -> Result<Option<Meeting>> {
    conn.query_row(&format!("{MEETING_SELECT} WHERE m.id = ?1"), [id], row_meeting)
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e.into()),
        })
}

pub fn list_meetings(conn: &Connection, limit: i64) -> Result<Vec<Meeting>> {
    let mut stmt =
        conn.prepare(&format!("{MEETING_SELECT} ORDER BY m.id DESC LIMIT ?1"))?;
    let rows = stmt.query_map([limit], row_meeting)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn meeting_segments(conn: &Connection, meeting_id: i64) -> Result<Vec<MeetingSegment>> {
    let mut stmt = conn.prepare(
        "SELECT id, seq, started_at, text FROM meeting_segment
         WHERE meeting_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map([meeting_id], |row| {
        Ok(MeetingSegment {
            id: row.get("id")?,
            seq: row.get("seq")?,
            started_at: row.get("started_at")?,
            text: row.get("text")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The transcript as one block, which is what the model is given. Empty
/// segments (a chunk whose transcription failed) drop out rather than becoming
/// blank lines the model has to interpret.
pub fn transcript(conn: &Connection, meeting_id: i64) -> Result<String> {
    let segments = meeting_segments(conn, meeting_id)?;
    let parts: Vec<String> = segments
        .into_iter()
        .map(|s| s.text.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    Ok(parts.join(" "))
}

pub fn add_segment(conn: &Connection, meeting_id: i64, seq: i64, text: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO meeting_segment (meeting_id, seq, started_at, text)
         VALUES (?1, ?2, ?3, ?4)",
        params![meeting_id, seq, now(), text],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Stop recording. Guarded on 'recording' so the startup orphan sweep and a
/// real Stop cannot both close the same meeting.
pub fn meeting_summarizing(conn: &Connection, id: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE meeting SET state = 'summarizing', ended_at = COALESCE(ended_at, ?1)
         WHERE id = ?2 AND state = 'recording'",
        params![now(), id],
    )?;
    Ok(changed > 0)
}

pub fn meeting_failed(conn: &Connection, id: i64, error: &str) -> Result<()> {
    conn.execute(
        "UPDATE meeting SET state = 'failed', error = ?1,
             ended_at = COALESCE(ended_at, ?2)
         WHERE id = ?3 AND state IN ('recording', 'summarizing')",
        params![error, now(), id],
    )?;
    Ok(())
}

/// The app died mid-meeting. ended_at falls back to the last segment's stamp
/// rather than now(): the meeting stopped when the audio stopped, not when the
/// app happened to be started again. Same principle as inferring a session's
/// ended_at from its last heartbeat.
///
/// 'summarizing' is swept too, and not as an afterthought: Stop now returns as
/// soon as the microphone is shut and finishes the notes in a background task,
/// so a row can sit in that state for a whole model call. A crash in there
/// would otherwise strand it forever — and the record bar keys "writing the
/// notes" off exactly this state, so a stranded row means a Start button that
/// never comes back. Failing it is also the more useful outcome: the transcript
/// is already stored, and 'failed' is the state that offers a retry.
pub fn sweep_orphan_meetings(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        // The CASE reads `state` before the UPDATE writes it, so it still names
        // which of the two ways this meeting was lost.
        "UPDATE meeting SET state = 'failed',
             error = COALESCE(error, CASE state
                 WHEN 'recording' THEN 'recording stopped when the app did'
                 ELSE 'the app closed before the notes were written' END),
             ended_at = COALESCE(
                 ended_at,
                 (SELECT MAX(started_at) FROM meeting_segment s WHERE s.meeting_id = meeting.id),
                 started_at)
         WHERE state IN ('recording', 'summarizing')",
        [],
    )?)
}

pub struct MeetingNotes<'a> {
    pub title: &'a str,
    pub summary: &'a str,
    pub key_points: &'a [String],
    pub action_items: &'a [String],
}

/// The notes and their action items land together — a summary on screen whose
/// action items failed to insert would be a lie about what was saved.
pub fn finish_meeting(conn: &mut Connection, id: i64, notes: &MeetingNotes) -> Result<()> {
    let key_points = serde_json::to_string(notes.key_points).unwrap_or_else(|_| "[]".into());
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE meeting SET title = ?1, summary = ?2, key_points = ?3,
             state = 'done', error = NULL, ended_at = COALESCE(ended_at, ?4)
         WHERE id = ?5",
        params![notes.title, notes.summary, key_points, now(), id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("no such meeting".into()));
    }
    // Re-running the summary replaces the proposals, but never the ones already
    // accepted: those are real tasks now, and deleting the row would orphan them.
    tx.execute("DELETE FROM meeting_action WHERE meeting_id = ?1 AND task_id IS NULL", [id])?;
    let offset: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM meeting_action WHERE meeting_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    for (i, text) in notes.action_items.iter().enumerate() {
        tx.execute(
            "INSERT INTO meeting_action (meeting_id, position, text) VALUES (?1, ?2, ?3)",
            params![id, offset + i as i64, text],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn meeting_actions(conn: &Connection, meeting_id: i64) -> Result<Vec<MeetingAction>> {
    let mut stmt = conn.prepare(
        "SELECT id, position, text, task_id FROM meeting_action
         WHERE meeting_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([meeting_id], |row| {
        Ok(MeetingAction {
            id: row.get("id")?,
            position: row.get("position")?,
            text: row.get("text")?,
            task_id: row.get("task_id")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Turn approved action items into backlog tasks. One transaction, and the
/// `task_id IS NULL` guard means approving the same item twice inserts once —
/// the same shape as carry's INSERT OR IGNORE against a unique index.
/// Returns how many tasks were actually created.
pub fn approve_actions(conn: &mut Connection, meeting_id: i64, ids: &[i64]) -> Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut next: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM task WHERE session_id IS NULL",
        [],
        |r| r.get(0),
    )?;
    let mut created = 0;
    for &action_id in ids {
        let text: Option<String> = tx
            .query_row(
                "SELECT text FROM meeting_action
                 WHERE id = ?1 AND meeting_id = ?2 AND task_id IS NULL",
                params![action_id, meeting_id],
                |r| r.get(0),
            )
            .ok();
        let Some(text) = text else { continue };
        // Backlog, not the current session: an action item is something to do
        // later, and the gate is where it gets pulled into a session.
        tx.execute(
            "INSERT INTO task (session_id, title, position, source, created_at)
             VALUES (NULL, ?1, ?2, 'meeting', ?3)",
            params![text, next, now()],
        )?;
        let task_id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE meeting_action SET task_id = ?1 WHERE id = ?2",
            params![task_id, action_id],
        )?;
        next += 1;
        created += 1;
    }
    tx.commit()?;
    Ok(created)
}

/// Deleting a meeting takes its segments and proposals with it (ON DELETE
/// CASCADE), but tasks already approved into the backlog survive: they stopped
/// belonging to the meeting the moment they became tasks.
pub fn delete_meeting(conn: &Connection, id: i64) -> Result<()> {
    let changed = conn.execute("DELETE FROM meeting WHERE id = ?1", [id])?;
    if changed == 0 {
        return Err(AppError::Other("no such meeting".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real schema, so these tests break if schema.sql and this file drift.
    fn store() -> Connection {
        let schema = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../gate/schema.sql");
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&std::fs::read_to_string(schema).unwrap()).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    #[test]
    fn a_meeting_records_outside_a_session() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        let m = get_meeting(&conn, id).unwrap().unwrap();
        assert_eq!(m.state, "recording");
        assert_eq!(m.session_id, None);
        assert_eq!(m.segment_count, 0);
        assert!(m.key_points.is_empty());
    }

    /// A chunk that failed to transcribe is stored as an empty segment so the
    /// gap is visible, but it must not become a blank run in what the model
    /// reads.
    #[test]
    fn the_transcript_joins_segments_and_skips_failed_ones() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_segment(&conn, id, 0, "First part.").unwrap();
        add_segment(&conn, id, 1, "").unwrap();
        add_segment(&conn, id, 2, "  Third part.  ").unwrap();
        assert_eq!(transcript(&conn, id).unwrap(), "First part. Third part.");
        assert_eq!(get_meeting(&conn, id).unwrap().unwrap().segment_count, 3);
    }

    #[test]
    fn approving_an_action_twice_creates_one_task() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "Migration plan",
            summary: "We agreed a date.",
            key_points: &["Thursday".to_string()],
            action_items: &["Send the doc".to_string(), "Book a slot".to_string()],
        }).unwrap();

        let actions = meeting_actions(&conn, id).unwrap();
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().all(|a| a.task_id.is_none()));

        let first = actions[0].id;
        assert_eq!(approve_actions(&mut conn, id, &[first]).unwrap(), 1);
        // The second approval of the same item is a no-op, not a duplicate.
        assert_eq!(approve_actions(&mut conn, id, &[first]).unwrap(), 0);

        let backlog = backlog(&conn).unwrap();
        assert_eq!(backlog.len(), 1);
        assert_eq!(backlog[0].title, "Send the doc");
        let source: String = conn
            .query_row("SELECT source FROM task WHERE id = ?1", [backlog[0].id], |r| r.get(0))
            .unwrap();
        assert_eq!(source, "meeting");
        assert!(meeting_actions(&conn, id).unwrap()[0].task_id.is_some());
    }

    /// Re-running the summary replaces the proposals nobody accepted, but an
    /// action already approved is a real task now — deleting its row would
    /// orphan that task and offer the same work twice.
    #[test]
    fn resummarizing_keeps_approved_actions_and_replaces_the_rest() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "First pass",
            summary: "s",
            key_points: &[],
            action_items: &["Keep me".to_string(), "Replace me".to_string()],
        }).unwrap();
        let keep = meeting_actions(&conn, id).unwrap()[0].id;
        approve_actions(&mut conn, id, &[keep]).unwrap();

        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "Second pass",
            summary: "s2",
            key_points: &[],
            action_items: &["Brand new".to_string()],
        }).unwrap();

        let after: Vec<String> =
            meeting_actions(&conn, id).unwrap().into_iter().map(|a| a.text).collect();
        assert_eq!(after, vec!["Keep me".to_string(), "Brand new".to_string()]);
        assert_eq!(get_meeting(&conn, id).unwrap().unwrap().title, "Second pass");
        assert_eq!(backlog(&conn).unwrap().len(), 1); // still just the approved one
    }

    /// The app was killed mid-meeting: ended_at comes from the last segment,
    /// not from whenever the app happened to be started again.
    #[test]
    fn the_orphan_sweep_closes_a_meeting_at_its_last_segment() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_segment(&conn, id, 0, "something said").unwrap();
        let seg_at: String = conn
            .query_row("SELECT started_at FROM meeting_segment WHERE seq = 0", [], |r| r.get(0))
            .unwrap();

        assert_eq!(sweep_orphan_meetings(&conn).unwrap(), 1);
        let m = get_meeting(&conn, id).unwrap().unwrap();
        assert_eq!(m.state, "failed");
        assert_eq!(m.ended_at.as_deref(), Some(seg_at.as_str()));
        assert!(m.error.is_some());
        // Idempotent: a second start must not re-close what is already closed.
        assert_eq!(sweep_orphan_meetings(&conn).unwrap(), 0);
    }

    /// The app was killed after Stop but before the notes were written. Stop
    /// hands the summary to a background task, so this window is a whole model
    /// call wide; a row left in 'summarizing' would keep the record bar saying
    /// "writing the notes" forever.
    #[test]
    fn the_orphan_sweep_fails_a_meeting_stranded_while_summarizing() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_segment(&conn, id, 0, "something said").unwrap();
        assert!(meeting_summarizing(&conn, id).unwrap());
        let stopped_at = get_meeting(&conn, id).unwrap().unwrap().ended_at;
        assert!(stopped_at.is_some(), "Stop records ended_at before the notes");

        assert_eq!(sweep_orphan_meetings(&conn).unwrap(), 1);
        let m = get_meeting(&conn, id).unwrap().unwrap();
        assert_eq!(m.state, "failed");
        // The moment recording stopped, not the moment the app came back.
        assert_eq!(m.ended_at, stopped_at);
        assert_eq!(m.error.as_deref(), Some("the app closed before the notes were written"));
        assert_eq!(sweep_orphan_meetings(&conn).unwrap(), 0);
    }

    /// Deleting a meeting takes its transcript with it, but not tasks that
    /// were approved out of it — those stopped belonging to the meeting.
    #[test]
    fn deleting_a_meeting_spares_the_tasks_it_produced() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_segment(&conn, id, 0, "said").unwrap();
        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "t", summary: "s", key_points: &[],
            action_items: &["Do the thing".to_string()],
        }).unwrap();
        let action = meeting_actions(&conn, id).unwrap()[0].id;
        approve_actions(&mut conn, id, &[action]).unwrap();

        delete_meeting(&conn, id).unwrap();
        assert!(get_meeting(&conn, id).unwrap().is_none());
        assert_eq!(meeting_segments(&conn, id).unwrap().len(), 0);
        assert_eq!(meeting_actions(&conn, id).unwrap().len(), 0);
        let backlog = backlog(&conn).unwrap();
        assert_eq!(backlog.len(), 1);
        assert_eq!(backlog[0].title, "Do the thing");
    }

    #[test]
    fn stopping_a_meeting_twice_only_moves_it_once() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        assert!(meeting_summarizing(&conn, id).unwrap());
        assert!(!meeting_summarizing(&conn, id).unwrap());
    }
}
