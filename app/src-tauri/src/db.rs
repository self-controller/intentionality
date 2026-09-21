//! All SQLite access. Mirrors gate/store.py's contract: schema migrations
//! belong to Python; this side only checks meta.schema_version and refuses
//! politely when the store is older than it understands.

use crate::error::{AppError, Result};
use crate::models::{
    Analysis, Arrangement, Label, LabelSummary, Meeting, MeetingAction, MeetingFile,
    MeetingSegment, Observed, Session, Task,
};
use crate::recommendations;
use chrono::{Duration, NaiveDate, Utc};
use rusqlite::{params, Connection, TransactionBehavior};
use std::collections::HashMap;
use std::path::PathBuf;

pub const SCHEMA_VERSION: i64 = 8;
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
    SELECT t.id, t.session_id, t.title, t.position, t.status, t.notes, t.due_date,
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
        notes: row.get("notes")?,
        due_date: row.get("due_date")?,
        // attach_labels fills these; a task read on its own has none.
        labels: Vec::new(),
    })
}

/// The colours a new label can be given, cycled by creation order. Stored on
/// the row at creation so a label's colour survives every later change.
/// gate/store.py keeps a copy for the labels the gate creates; a Python test
/// reads this list and fails when the two differ.
const LABEL_COLORS: [&str; 8] = [
    "#7aa2f7", "#9ece6a", "#e0af68", "#f7768e", "#bb9af7", "#2ac3de", "#ff9e64", "#41a6b5",
];

/// One query for the whole join, then distributed over the tasks in hand. The
/// store is deliberately small enough to copy and open in a shell, so reading
/// every label beats building a dynamic `IN (…)` per call.
fn attach_labels(conn: &Connection, tasks: &mut [Task]) -> Result<()> {
    if tasks.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "SELECT tl.task_id, l.name, l.color
         FROM task_label tl JOIN label l ON l.id = tl.label_id
         ORDER BY l.name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>("task_id")?,
            Label { name: r.get("name")?, color: r.get("color")? },
        ))
    })?;
    let mut by_task: HashMap<i64, Vec<Label>> = HashMap::new();
    for row in rows {
        let (task_id, label) = row?;
        by_task.entry(task_id).or_default().push(label);
    }
    for task in tasks.iter_mut() {
        if let Some(labels) = by_task.remove(&task.id) {
            task.labels = labels;
        }
    }
    Ok(())
}

/// Every label there is, worn or not, with how many tasks wear it. A label
/// nothing wears is a preset: it stays until it is deleted.
pub fn list_labels(conn: &Connection) -> Result<Vec<LabelSummary>> {
    let mut stmt = conn.prepare(
        "SELECT l.name, l.color, COUNT(tl.task_id) AS uses
         FROM label l LEFT JOIN task_label tl ON tl.label_id = l.id
         GROUP BY l.id ORDER BY l.name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(LabelSummary { name: r.get("name")?, color: r.get("color")?, uses: r.get("uses")? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Make a label, or find the one there is: label.name is NOCASE, so "Billing"
/// is "billing" and keeps that one's colour. Returns its id.
fn ensure_label(conn: &Connection, name: &str) -> Result<i64> {
    let next: i64 = conn.query_row("SELECT COUNT(*) FROM label", [], |r| r.get(0))?;
    conn.execute(
        "INSERT OR IGNORE INTO label (name, color) VALUES (?1, ?2)",
        params![name, LABEL_COLORS[(next as usize) % LABEL_COLORS.len()]],
    )?;
    Ok(conn.query_row("SELECT id FROM label WHERE name = ?1", [name], |r| r.get(0))?)
}

/// A preset: a label made before anything wears it.
pub fn create_label(conn: &Connection, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Other("a label needs a name".into()));
    }
    ensure_label(conn, name)?;
    Ok(())
}

/// Delete a label everywhere. Its task_label rows go with it by cascade, so
/// every card that wore it lets it go.
pub fn delete_label(conn: &Connection, name: &str) -> Result<()> {
    let changed = conn.execute("DELETE FROM label WHERE name = ?1", [name.trim()])?;
    if changed == 0 {
        return Err(AppError::Other(format!("no label {name:?}")));
    }
    Ok(())
}

pub fn session_tasks(conn: &Connection, session_id: i64) -> Result<Vec<Task>> {
    let mut stmt =
        conn.prepare(&format!("{TASK_SELECT} WHERE t.session_id = ?1 ORDER BY t.position"))?;
    let rows = stmt.query_map([session_id], row_task)?;
    let mut tasks: Vec<Task> = rows.collect::<rusqlite::Result<_>>()?;
    attach_labels(conn, &mut tasks)?;
    Ok(tasks)
}

pub fn backlog(conn: &Connection) -> Result<Vec<Task>> {
    let mut stmt =
        conn.prepare(&format!("{TASK_SELECT} WHERE t.session_id IS NULL ORDER BY t.position"))?;
    let rows = stmt.query_map([], row_task)?;
    let mut tasks: Vec<Task> = rows.collect::<rusqlite::Result<_>>()?;
    attach_labels(conn, &mut tasks)?;
    Ok(tasks)
}

pub fn apply_board(conn: &mut Connection, session_id: i64, arr: &Arrangement) -> Result<()> {
    let ts = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Inside the write lock, so the gate cannot close the session between the
    // check and the moves.
    ensure_open(&tx, session_id)?;
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
    if let Some(sid) = session_id {
        ensure_open(conn, sid)?;
    }
    insert_task(conn, session_id, title)
}

/// A new card with everything the editor holds, in one atomic write: the task
/// and its details land together or not at all.
pub fn create_task(
    conn: &mut Connection,
    session_id: Option<i64>,
    title: &str,
    notes: &str,
    due_date: Option<&str>,
    labels: &[String],
) -> Result<i64> {
    check_due(due_date)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Inside the write lock, as in apply_board.
    if let Some(sid) = session_id {
        ensure_open(&tx, sid)?;
    }
    let id = insert_task(&tx, session_id, title)?;
    write_details(&tx, id, title, notes, due_date, labels)?;
    tx.commit()?;
    Ok(id)
}

/// Numbered after the last card where it lands (the session, or the backlog).
fn insert_task(conn: &Connection, session_id: Option<i64>, title: &str) -> Result<i64> {
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

/// The whole card in one atomic write — title, notes, due date and the exact
/// set of labels it should end up wearing. Same contract as apply_board: the
/// caller sends the end state, not a diff, so a stale editor cannot
/// half-apply, and a None due date clears it.
pub fn update_task(
    conn: &mut Connection,
    id: i64,
    title: &str,
    notes: &str,
    due_date: Option<&str>,
    labels: &[String],
) -> Result<()> {
    check_due(due_date)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    write_details(&tx, id, title, notes, due_date, labels)?;
    tx.commit()?;
    Ok(())
}

/// Canonical form only: a fixed-width ISO day is what makes string order
/// date order. The reformat check refuses "2026-9-1", which chrono parses,
/// as well as "2026-02-30", which it does not.
fn check_due(due_date: Option<&str>) -> Result<()> {
    if let Some(d) = due_date {
        let canonical = NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map(|p| p.format("%Y-%m-%d").to_string() == d)
            .unwrap_or(false);
        if !canonical {
            return Err(AppError::Other(format!("due date must be YYYY-MM-DD, not {d:?}")));
        }
    }
    Ok(())
}

/// Everything but position and status, for a task that exists. Never deletes
/// a label: one its last task lets go of stays, as a preset. gate/store.py's
/// _set_details is the same write for the gate.
fn write_details(
    conn: &Connection,
    id: i64,
    title: &str,
    notes: &str,
    due_date: Option<&str>,
    labels: &[String],
) -> Result<()> {
    let changed = conn.execute(
        "UPDATE task SET title = ?1, notes = ?2, due_date = ?3 WHERE id = ?4",
        params![title, notes, due_date, id],
    )?;
    if changed == 0 {
        return Err(AppError::Other(format!("no task {id}")));
    }
    conn.execute("DELETE FROM task_label WHERE task_id = ?1", [id])?;
    for name in labels {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let label_id = ensure_label(conn, name)?;
        conn.execute(
            "INSERT OR IGNORE INTO task_label (task_id, label_id) VALUES (?1, ?2)",
            params![id, label_id],
        )?;
    }
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
    ensure_open(conn, session_id)?;
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

/// Whether a session can still take writes. A missing row is not open. Same
/// test as latest_open_session: a recovery with no heartbeat to date it closes
/// the session with close_reason set and ended_at still NULL.
pub fn session_is_open(conn: &Connection, session_id: i64) -> Result<bool> {
    match conn.query_row(
        "SELECT ended_at IS NULL AND close_reason IS NULL FROM session WHERE id = ?1",
        [session_id],
        |r| r.get(0),
    ) {
        Ok(open) => Ok(open),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Session history is immutable once closed. The app can still be holding a
/// session the gate has closed — the heartbeat only notices on its next tick —
/// so every write aimed at "the current session" checks here first.
fn ensure_open(conn: &Connection, session_id: i64) -> Result<()> {
    if session_is_open(conn, session_id)? {
        Ok(())
    } else {
        Err(AppError::SessionClosed(session_id))
    }
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

// Deliberately no notes, no clean_transcript and no summary: this drives the
// list as well as the detail header, and listing 50 meetings must not drag
// two transcripts and a write-up each across the boundary. has_clean is the
// boolean the UI actually needs.
const MEETING_SELECT: &str = "
    SELECT m.id, m.session_id, m.started_at, m.ended_at, m.title, m.state, m.error,
        m.clean_transcript IS NOT NULL AS has_clean,
        (SELECT COUNT(*) FROM meeting_segment s WHERE s.meeting_id = m.id)
            AS segment_count,
        (SELECT COUNT(*) FROM meeting_file f WHERE f.meeting_id = m.id)
            AS file_count
    FROM meeting m";

fn row_meeting(row: &rusqlite::Row) -> rusqlite::Result<Meeting> {
    Ok(Meeting {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        title: row.get("title")?,
        state: row.get("state")?,
        error: row.get("error")?,
        segment_count: row.get("segment_count")?,
        file_count: row.get("file_count")?,
        has_clean: row.get("has_clean")?,
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
///
/// 'cleaning', not 'summarizing': the transcript is repaired before the notes
/// are written, and that first pass is the long one.
pub fn meeting_cleaning(conn: &Connection, id: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE meeting SET state = 'cleaning', ended_at = COALESCE(ended_at, ?1)
         WHERE id = ?2 AND state = 'recording'",
        params![now(), id],
    )?;
    Ok(changed > 0)
}

/// A guarded state hop. `from` is what makes this safe to call from a
/// background task: two of them racing, or a sweep that already failed the
/// row, cannot drag a meeting back into a wrap-up state it has left.
pub fn meeting_state(conn: &Connection, id: i64, from: &str, to: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE meeting SET state = ?1 WHERE id = ?2 AND state = ?3",
        params![to, id, from],
    )?;
    Ok(changed > 0)
}

/// Re-run the notes for a meeting that already finished one way or the other.
/// Guarded on the two terminal states so a re-run cannot touch a meeting that
/// is recording or already mid-run, and clears the old error so the UI shows
/// progress rather than the previous failure.
pub fn meeting_rerun(conn: &Connection, id: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE meeting SET state = 'cleaning', error = NULL
         WHERE id = ?1 AND state IN ('done', 'failed')",
        [id],
    )?;
    Ok(changed > 0)
}

/// Pick a finished meeting back up: the microphone is open on it again. Same
/// guard as a re-run, for the same reason.
///
/// Three columns are cleared, each for a reason of its own. `ended_at`,
/// because `meeting_cleaning` only stamps an empty one, so the next Stop would
/// otherwise keep the first Stop's time; `error`, because the sweep keeps an
/// existing one, so a crash mid-resume would report the old failure instead
/// of the recording it lost; and `clean_transcript`, because it no longer
/// covers the meeting and is shown ahead of the raw segments that do.
///
/// The write-up and the action items stay: the next Stop replaces them, and
/// until it has they are still the best notes this meeting has.
pub fn meeting_resume(conn: &Connection, id: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE meeting SET state = 'recording', ended_at = NULL, error = NULL,
             clean_transcript = NULL
         WHERE id = ?1 AND state IN ('done', 'failed')",
        [id],
    )?;
    Ok(changed > 0)
}

/// Where a resumed recording carries on from: the next free `seq`, and the
/// last thing actually transcribed, which becomes the context for its first
/// chunk exactly as the previous chunk's tail would have been.
pub fn resume_point(conn: &Connection, meeting_id: i64) -> Result<(i64, String)> {
    let next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM meeting_segment WHERE meeting_id = ?1",
        [meeting_id],
        |r| r.get(0),
    )?;
    let last: String = conn
        .query_row(
            "SELECT text FROM meeting_segment
             WHERE meeting_id = ?1 AND TRIM(text) != ''
             ORDER BY seq DESC LIMIT 1",
            [meeting_id],
            |r| r.get(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(String::new()),
            e => Err(e),
        })?;
    Ok((next, last))
}

/// Both wrap-up states, not just 'summarizing': a meeting can now fail during
/// the cleaning pass, and a row left in either one is a Start button that
/// never comes back. This guard and the sweep's below have to move together.
pub fn meeting_failed(conn: &Connection, id: i64, error: &str) -> Result<()> {
    conn.execute(
        "UPDATE meeting SET state = 'failed', error = ?1,
             ended_at = COALESCE(ended_at, ?2)
         WHERE id = ?3 AND state IN ('recording', 'cleaning', 'summarizing')",
        params![error, now(), id],
    )?;
    Ok(())
}

/// The app died mid-meeting. ended_at falls back to the last segment's stamp
/// rather than now(): the meeting stopped when the audio stopped, not when the
/// app happened to be started again. Same principle as inferring a session's
/// ended_at from its last heartbeat.
///
/// Both wrap-up states are swept too, and not as an afterthought: Stop now returns as
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
         WHERE state IN ('recording', 'cleaning', 'summarizing')",
        [],
    )?)
}

/// The scratchpad. Writable in any state: it is the user's own text, and
/// locking it because a model call is in flight would lose whatever they were
/// mid-sentence on.
pub fn set_meeting_notes(conn: &Connection, id: i64, notes: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE meeting SET notes = ?1 WHERE id = ?2",
        params![notes, id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("no such meeting".into()));
    }
    Ok(())
}

pub fn meeting_notes_text(conn: &Connection, id: i64) -> Result<String> {
    Ok(conn.query_row("SELECT notes FROM meeting WHERE id = ?1", [id], |r| r.get(0))?)
}

/// The write-up and when the user last edited it, for the detail pane.
pub fn meeting_summary(conn: &Connection, id: i64) -> Result<(Option<String>, Option<String>)> {
    Ok(conn.query_row(
        "SELECT summary, summary_edited_at FROM meeting WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// The user's edit to the write-up. Only once the model is done with it: a
/// save landing while the notes are being written would be overwritten by
/// finish_meeting moments later, and the UI hides the editor in those states
/// anyway -- refusing here is what makes that a guarantee rather than a hope.
/// The stamp is what lets a later re-run ask before throwing the edit away.
pub fn set_meeting_summary(conn: &Connection, id: i64, summary: &str) -> Result<()> {
    let changed = conn.execute(
        "UPDATE meeting SET summary = ?1, summary_edited_at = ?2
         WHERE id = ?3 AND state IN ('done', 'failed')",
        params![summary, now(), id],
    )?;
    if changed == 0 {
        return Err(AppError::Other(
            "that meeting's notes are still being written, or it no longer exists".into(),
        ));
    }
    Ok(())
}

pub fn clean_transcript(conn: &Connection, id: i64) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT clean_transcript FROM meeting WHERE id = ?1", [id], |r| r.get(0))?)
}

pub fn set_clean_transcript(conn: &Connection, id: i64, text: &str) -> Result<()> {
    conn.execute(
        "UPDATE meeting SET clean_transcript = ?1 WHERE id = ?2",
        params![text, id],
    )?;
    Ok(())
}

/// One attached file as the model pipeline needs it: enough to build a
/// content block, including the path the webview never sees.
pub struct FileRow {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub extracted: Option<String>,
}

pub fn meeting_file_rows(conn: &Connection, meeting_id: i64) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare(
        "SELECT name, kind, path, extracted FROM meeting_file
         WHERE meeting_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([meeting_id], |row| {
        Ok(FileRow {
            name: row.get(0)?,
            kind: row.get(1)?,
            path: row.get(2)?,
            extracted: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub struct NewFile<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub kind: &'a str,
    pub bytes: i64,
    pub extracted: Option<&'a str>,
}

/// Insert the row for a staged file, or refuse.
///
/// The aggregate limits are checked here, inside the transaction, and not only
/// in the UI: two attach clicks racing would each see room under the cap and
/// both proceed, and the whole point of a total is that it holds.
///
/// The row goes in before the file is moved into place. That order is the
/// deliberate one — the two cannot share a transaction, so one of them has to
/// be able to fail last, and a row without its file is recoverable (delete the
/// row) while a file without its row is invisible litter.
pub fn add_meeting_file(
    conn: &mut Connection,
    meeting_id: i64,
    file: &NewFile,
    max_files: i64,
    max_total_bytes: i64,
    max_total_chars: i64,
) -> Result<i64> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let exists: bool = tx
        .query_row("SELECT 1 FROM meeting WHERE id = ?1", [meeting_id], |_| Ok(()))
        .is_ok();
    if !exists {
        return Err(AppError::Other("no such meeting".into()));
    }
    let (count, total, chars): (i64, i64, i64) = tx.query_row(
        "SELECT COUNT(*), COALESCE(SUM(bytes), 0), COALESCE(SUM(LENGTH(extracted)), 0)
         FROM meeting_file WHERE meeting_id = ?1",
        [meeting_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if count >= max_files {
        return Err(AppError::Other(format!(
            "that meeting already has {max_files} files — remove one first"
        )));
    }
    if total + file.bytes > max_total_bytes {
        return Err(AppError::Other(format!(
            "that would take the meeting over its {} MB of attachments",
            max_total_bytes / (1024 * 1024)
        )));
    }
    let new_chars = file.extracted.map(|e| e.chars().count() as i64).unwrap_or(0);
    if chars + new_chars > max_total_chars {
        return Err(AppError::Other(
            "that would take the meeting over the amount of text this can send".into(),
        ));
    }
    let position: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM meeting_file WHERE meeting_id = ?1",
        [meeting_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO meeting_file (meeting_id, position, name, path, kind, bytes,
             extracted, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            meeting_id,
            position,
            file.name,
            file.path,
            file.kind,
            file.bytes,
            file.extracted,
            now()
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

/// Remove one attachment's row, returning the copy the caller must now unlink.
/// The row goes first: a file left on disk is swept, a row pointing at nothing
/// is a broken attachment in the UI.
pub fn delete_meeting_file(conn: &Connection, meeting_id: i64, file_id: i64) -> Result<String> {
    let path: String = conn
        .query_row(
            "SELECT path FROM meeting_file WHERE id = ?1 AND meeting_id = ?2",
            params![file_id, meeting_id],
            |r| r.get(0),
        )
        .map_err(|_| AppError::Other("no such file".into()))?;
    conn.execute("DELETE FROM meeting_file WHERE id = ?1", [file_id])?;
    Ok(path)
}

/// What the webview is allowed to know about the attached files. `path` and
/// `extracted` stay in this module -- see the note on `MeetingFile`.
pub fn meeting_files(conn: &Connection, meeting_id: i64) -> Result<Vec<MeetingFile>> {
    let mut stmt = conn.prepare(
        "SELECT id, position, name, kind, bytes, added_at FROM meeting_file
         WHERE meeting_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([meeting_id], |row| {
        Ok(MeetingFile {
            id: row.get(0)?,
            position: row.get(1)?,
            name: row.get(2)?,
            kind: row.get(3)?,
            bytes: row.get(4)?,
            added_at: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub struct MeetingNotes<'a> {
    pub title: &'a str,
    /// The write-up, as markdown.
    pub summary: &'a str,
    pub action_items: &'a [String],
}

/// The notes and their action items land together — a summary on screen whose
/// action items failed to insert would be a lie about what was saved.
///
/// No state guard, deliberately: this is also the re-run path, and a meeting
/// being re-summarized starts from 'done' or 'failed'. `summary_edited_at` is
/// cleared on the same principle: whatever the user had changed, the document
/// is the model's again -- the UI asked before letting a re-run get this far.
pub fn finish_meeting(conn: &mut Connection, id: i64, notes: &MeetingNotes) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE meeting SET title = ?1, summary = ?2, summary_edited_at = NULL,
             state = 'done', error = NULL, ended_at = COALESCE(ended_at, ?3)
         WHERE id = ?4",
        params![notes.title, notes.summary, now(), id],
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
pub(crate) mod tests {
    use super::*;

    /// The real schema, so these tests break if schema.sql and this file drift.
    pub(crate) fn store() -> Connection {
        let schema = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../gate/schema.sql");
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&std::fs::read_to_string(schema).unwrap()).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    /// An open session with one card on it, which is what every task test
    /// below needs before it can do anything.
    fn session_with_task(conn: &Connection) -> (i64, i64) {
        conn.execute(
            "INSERT INTO session (started_at, mode) VALUES (?1, 'manual')",
            [now()],
        )
        .unwrap();
        let session_id = conn.last_insert_rowid();
        let task_id = add_task(conn, Some(session_id), "write the thing").unwrap();
        (session_id, task_id)
    }

    fn label_names(conn: &Connection, task_id: i64) -> Vec<String> {
        let tasks = session_tasks(
            conn,
            conn.query_row("SELECT session_id FROM task WHERE id = ?1", [task_id], |r| {
                r.get(0)
            })
            .unwrap(),
        )
        .unwrap();
        tasks
            .into_iter()
            .find(|t| t.id == task_id)
            .unwrap()
            .labels
            .into_iter()
            .map(|l| l.name)
            .collect()
    }

    #[test]
    fn update_task_round_trips_title_notes_and_labels() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        update_task(
            &mut conn,
            task_id,
            "write the other thing",
            "two paragraphs\nof plain text",
            None,
            &["billing".into(), "urgent".into()],
        )
        .unwrap();

        let task = session_tasks(&conn, session_id).unwrap().pop().unwrap();
        assert_eq!(task.title, "write the other thing");
        assert_eq!(task.notes, "two paragraphs\nof plain text");
        // Sorted by name, so the chips are in the same order every render.
        assert_eq!(
            task.labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["billing", "urgent"]
        );
        assert!(task.labels.iter().all(|l| l.color.starts_with('#')));
        assert_eq!(list_labels(&conn).unwrap().len(), 2);
    }

    /// The store's NOCASE unique index is what makes "Billing" the same tag as
    /// "billing" -- including keeping the colour it was first given.
    #[test]
    fn a_label_is_reused_across_tasks_whatever_its_case() {
        let mut conn = store();
        let (session_id, first) = session_with_task(&conn);
        let second = add_task(&conn, Some(session_id), "another").unwrap();
        update_task(&mut conn, first, "a", "", None, &["billing".into()]).unwrap();
        let color = list_labels(&conn).unwrap()[0].color.clone();

        update_task(&mut conn, second, "b", "", None, &["BILLING".into()]).unwrap();
        let labels = list_labels(&conn).unwrap();
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].name, "billing");
        assert_eq!(labels[0].color, color);
        assert_eq!(label_names(&conn, second), ["billing"]);
    }

    /// A label nothing wears any more is a preset now: it stays offered until
    /// someone deletes it.
    #[test]
    fn a_label_outlives_its_last_task() {
        let mut conn = store();
        let (session_id, first) = session_with_task(&conn);
        let second = add_task(&conn, Some(session_id), "another").unwrap();
        update_task(&mut conn, first, "a", "", None, &["billing".into()]).unwrap();
        update_task(&mut conn, second, "b", "", None, &["billing".into(), "urgent".into()]).unwrap();
        let uses = |conn: &Connection| -> Vec<(String, i64)> {
            list_labels(conn).unwrap().into_iter().map(|l| (l.name, l.uses)).collect()
        };
        assert_eq!(uses(&conn), [("billing".into(), 2), ("urgent".into(), 1)]);

        update_task(&mut conn, first, "a", "", None, &[]).unwrap();
        update_task(&mut conn, second, "b", "", None, &[]).unwrap();
        assert_eq!(uses(&conn), [("billing".into(), 0), ("urgent".into(), 0)]);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM task_label", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn a_preset_is_tagged_by_name_and_keeps_its_colour() {
        let mut conn = store();
        let (_, task_id) = session_with_task(&conn);
        create_label(&conn, "  Reading  ").unwrap();
        let preset = list_labels(&conn).unwrap().pop().unwrap();
        assert_eq!((preset.name.as_str(), preset.uses), ("Reading", 0));
        // Making it again, in another case, is the same label.
        create_label(&conn, "reading").unwrap();
        assert_eq!(list_labels(&conn).unwrap().len(), 1);
        assert!(create_label(&conn, "   ").is_err());

        update_task(&mut conn, task_id, "a", "", None, &["READING".into()]).unwrap();
        let task = session_tasks(&conn, 1).unwrap().pop().unwrap();
        assert_eq!(task.labels, [Label { name: "Reading".into(), color: preset.color }]);
    }

    #[test]
    fn deleting_a_label_takes_it_off_every_card() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        update_task(&mut conn, task_id, "a", "", None, &["billing".into(), "urgent".into()])
            .unwrap();
        delete_label(&conn, "Billing").unwrap();
        assert_eq!(label_names(&conn, task_id), ["urgent"]);
        assert_eq!(list_labels(&conn).unwrap().len(), 1);
        assert!(delete_label(&conn, "billing").is_err());
        assert_eq!(session_tasks(&conn, session_id).unwrap().len(), 1);
    }

    #[test]
    fn create_task_writes_the_whole_card_at_once() {
        let mut conn = store();
        let (session_id, first) = session_with_task(&conn);
        let id = create_task(
            &mut conn,
            Some(session_id),
            "with details",
            "some notes",
            Some("2026-10-01"),
            &["billing".into()],
        )
        .unwrap();
        let tasks = session_tasks(&conn, session_id).unwrap();
        assert_eq!(tasks.iter().map(|t| t.id).collect::<Vec<_>>(), [first, id]);
        let task = &tasks[1];
        assert_eq!(
            (task.title.as_str(), task.notes.as_str(), task.due_date.as_deref(), task.position),
            ("with details", "some notes", Some("2026-10-01"), 2)
        );
        assert_eq!(label_names(&conn, id), ["billing"]);

        let backlog_id =
            create_task(&mut conn, None, "for later", "", None, &["later".into()]).unwrap();
        let rows = backlog(&conn).unwrap();
        assert_eq!(rows.iter().map(|t| t.id).collect::<Vec<_>>(), [backlog_id]);
        assert_eq!(rows[0].labels.len(), 1);
    }

    #[test]
    fn create_task_writes_nothing_when_refused() {
        let mut conn = store();
        let (session_id, _) = session_with_task(&conn);
        let count = |conn: &Connection, table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
        };
        assert!(create_task(&mut conn, Some(session_id), "t", "", Some("2026-9-1"), &[]).is_err());
        conn.execute("UPDATE session SET ended_at = ?1 WHERE id = ?2", params![now(), session_id])
            .unwrap();
        assert!(matches!(
            create_task(&mut conn, Some(session_id), "t", "n", None, &["x".into()]),
            Err(AppError::SessionClosed(_))
        ));
        assert_eq!(count(&conn, "task"), 1);
        assert_eq!(count(&conn, "label"), 0);
    }

    /// Blank and repeated entries are what a text field actually produces.
    #[test]
    fn update_task_ignores_blank_and_repeated_labels() {
        let mut conn = store();
        let (_, task_id) = session_with_task(&conn);
        update_task(
            &mut conn,
            task_id,
            "a",
            "",
            None,
            &["  billing  ".into(), "".into(), "Billing".into()],
        )
        .unwrap();
        assert_eq!(label_names(&conn, task_id), ["billing"]);
    }

    /// The old rename_task reported success for an id that was never there.
    #[test]
    fn update_task_refuses_an_unknown_id() {
        let mut conn = store();
        assert!(update_task(&mut conn, 404, "a", "", None, &[]).is_err());
    }

    /// A due date is a day. Set, read back, then cleared by sending None --
    /// the editor's Clear is the same end-state write as any other save.
    #[test]
    fn update_task_sets_and_clears_the_due_date() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        update_task(&mut conn, task_id, "a", "", Some("2026-09-18"), &[]).unwrap();
        let task = session_tasks(&conn, session_id).unwrap().pop().unwrap();
        assert_eq!(task.due_date.as_deref(), Some("2026-09-18"));

        update_task(&mut conn, task_id, "a", "", None, &[]).unwrap();
        let task = session_tasks(&conn, session_id).unwrap().pop().unwrap();
        assert_eq!(task.due_date, None);
    }

    /// Only the canonical form is stored, because string order has to be date
    /// order. "2026-9-1" is the one chrono would happily parse.
    #[test]
    fn update_task_refuses_a_malformed_due_date() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        for bad in ["tomorrow", "2026-02-30", "2026-9-1", "2026-09-18T10:00", ""] {
            assert!(
                update_task(&mut conn, task_id, "a", "", Some(bad), &[]).is_err(),
                "{bad:?} was accepted"
            );
        }
        // Refused before the transaction: nothing else on the card changed.
        let task = session_tasks(&conn, session_id).unwrap().pop().unwrap();
        assert_eq!(task.title, "write the thing");
        assert_eq!(task.due_date, None);
    }

    /// Dropping a card is a status, not a delete: it stays on the board's
    /// history and comes back with resolved_at cleared.
    #[test]
    fn a_card_can_be_dropped_and_restored() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        let arrange = |conn: &mut Connection, arr: Arrangement| {
            apply_board(conn, session_id, &arr).unwrap();
            session_tasks(conn, session_id).unwrap().pop().unwrap()
        };

        let dropped = arrange(
            &mut conn,
            Arrangement { todo: vec![], doing: vec![], done: vec![], dropped: vec![task_id] },
        );
        assert_eq!(dropped.status, "dropped");
        let resolved: Option<String> = conn
            .query_row("SELECT resolved_at FROM task WHERE id = ?1", [task_id], |r| r.get(0))
            .unwrap();
        assert!(resolved.is_some());

        let restored = arrange(
            &mut conn,
            Arrangement { todo: vec![task_id], doing: vec![], done: vec![], dropped: vec![] },
        );
        assert_eq!(restored.status, "planned");
        let resolved: Option<String> = conn
            .query_row("SELECT resolved_at FROM task WHERE id = ?1", [task_id], |r| r.get(0))
            .unwrap();
        assert!(resolved.is_none());
    }

    /// The board on screen can outlive its session: the resume gate closes it
    /// on another VT and the heartbeat only notices on its next tick. A drag in
    /// that window used to rewrite the closed session's history.
    #[test]
    fn a_closed_session_refuses_board_writes() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        let backlog_id = add_task(&conn, None, "a backlog item").unwrap();
        conn.execute(
            "UPDATE session SET ended_at = ?1 WHERE id = ?2",
            params![now(), session_id],
        )
        .unwrap();

        let moved = Arrangement { todo: vec![], doing: vec![task_id], done: vec![], dropped: vec![] };
        assert!(matches!(
            apply_board(&mut conn, session_id, &moved),
            Err(AppError::SessionClosed(id)) if id == session_id
        ));
        assert!(matches!(add_task(&conn, Some(session_id), "late"), Err(AppError::SessionClosed(_))));
        assert!(matches!(pull_task(&conn, session_id, backlog_id), Err(AppError::SessionClosed(_))));

        let (status, started): (String, Option<String>) = conn
            .query_row("SELECT status, started_at FROM task WHERE id = ?1", [task_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((status.as_str(), started), ("planned", None));
        assert_eq!(session_tasks(&conn, session_id).unwrap().len(), 1);
        assert_eq!(backlog(&conn).unwrap().len(), 1);
        // The backlog belongs to no session and stays writable between them.
        assert!(add_task(&conn, None, "still fine").is_ok());
    }

    #[test]
    fn a_missing_session_is_not_open() {
        assert!(!session_is_open(&store(), 404).unwrap());
    }

    /// What the gate leaves when it recovers a session nothing heartbeated:
    /// closed, with no time to put in ended_at.
    #[test]
    fn a_session_closed_with_an_unknown_end_is_not_open() {
        let conn = store();
        let (session_id, _) = session_with_task(&conn);
        assert!(session_is_open(&conn, session_id).unwrap());
        conn.execute("UPDATE session SET close_reason = 'recovered' WHERE id = ?1", [session_id])
            .unwrap();
        assert!(!session_is_open(&conn, session_id).unwrap());
    }

    /// Deleting a backlog row must not leave a label looking as though
    /// something still wears it. The label itself stays, as a preset.
    #[test]
    fn deleting_a_backlog_task_releases_its_labels() {
        let mut conn = store();
        let task_id = add_task(&conn, None, "a backlog item").unwrap();
        update_task(&mut conn, task_id, "a backlog item", "", None, &["billing".into()]).unwrap();
        assert_eq!(backlog(&conn).unwrap()[0].labels.len(), 1);

        delete_backlog_task(&conn, task_id).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM task_label", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let labels = list_labels(&conn).unwrap();
        assert_eq!((labels[0].name.as_str(), labels[0].uses), ("billing", 0));
    }

    #[test]
    fn a_meeting_records_outside_a_session() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        let m = get_meeting(&conn, id).unwrap().unwrap();
        assert_eq!(m.state, "recording");
        assert_eq!(m.session_id, None);
        assert_eq!(m.segment_count, 0);
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
    #[test]
    fn the_scratchpad_round_trips_in_any_state() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        assert_eq!(meeting_notes_text(&conn, id).unwrap(), "");
        set_meeting_notes(&conn, id, "Ana Kirtsova, not 'on a curt so far'").unwrap();
        assert_eq!(
            meeting_notes_text(&conn, id).unwrap(),
            "Ana Kirtsova, not 'on a curt so far'"
        );
        // Still writable once the meeting is over: a debounced autosave can
        // land after Stop, and rejecting it would lose what was typed last.
        meeting_cleaning(&conn, id).unwrap();
        set_meeting_notes(&conn, id, "and a todo nobody said out loud").unwrap();
        assert_eq!(
            meeting_notes_text(&conn, id).unwrap(),
            "and a todo nobody said out loud"
        );
        assert!(set_meeting_notes(&conn, id + 99, "nowhere").is_err());
    }

    /// The raw segments are the record of what was heard, and the repair pass
    /// must never be able to overwrite them.
    #[test]
    fn cleaning_stores_a_second_transcript_beside_the_raw_one() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_segment(&conn, id, 0, "um so we agreed thursday").unwrap();
        assert!(clean_transcript(&conn, id).unwrap().is_none());
        assert!(!get_meeting(&conn, id).unwrap().unwrap().has_clean);

        set_clean_transcript(&conn, id, "So we agreed Thursday.").unwrap();
        assert_eq!(
            clean_transcript(&conn, id).unwrap().unwrap(),
            "So we agreed Thursday."
        );
        assert!(get_meeting(&conn, id).unwrap().unwrap().has_clean);
        assert_eq!(transcript(&conn, id).unwrap(), "um so we agreed thursday");
    }

    #[test]
    fn a_meeting_moves_through_cleaning_then_summarizing() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        assert!(meeting_cleaning(&conn, id).unwrap());
        assert_eq!(get_meeting(&conn, id).unwrap().unwrap().state, "cleaning");
        assert!(meeting_state(&conn, id, "cleaning", "summarizing").unwrap());
        assert_eq!(get_meeting(&conn, id).unwrap().unwrap().state, "summarizing");
        // Guarded: the same hop cannot be taken twice, so a task that outlived
        // a sweep cannot drag the row back.
        assert!(!meeting_state(&conn, id, "cleaning", "summarizing").unwrap());
    }

    /// A re-run is only for a meeting that has finished. Anything else is
    /// either still recording or already mid-run, and starting a second run
    /// over it would race the first.
    #[test]
    fn a_rerun_only_moves_a_finished_meeting() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        assert!(!meeting_rerun(&conn, id).unwrap()); // recording
        meeting_cleaning(&conn, id).unwrap();
        assert!(!meeting_rerun(&conn, id).unwrap()); // mid-run

        meeting_failed(&conn, id, "the model timed out").unwrap();
        assert!(meeting_rerun(&conn, id).unwrap());
        let m = get_meeting(&conn, id).unwrap().unwrap();
        assert_eq!(m.state, "cleaning");
        assert!(m.error.is_none(), "the old failure must not still be on screen");

        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "t", summary: "s", action_items: &[],
        }).unwrap();
        assert!(meeting_rerun(&conn, id).unwrap()); // done is re-runnable too
    }

    /// Resuming opens the microphone on a meeting again, so it has the same
    /// guard as a re-run: a meeting still recording or mid-run is not finished.
    #[test]
    fn a_resume_only_moves_a_finished_meeting() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        assert!(!meeting_resume(&conn, id).unwrap()); // recording
        meeting_cleaning(&conn, id).unwrap();
        assert!(!meeting_resume(&conn, id).unwrap()); // cleaning
        meeting_state(&conn, id, "cleaning", "summarizing").unwrap();
        assert!(!meeting_resume(&conn, id).unwrap()); // summarizing

        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "t", summary: "s", action_items: &[],
        }).unwrap();
        assert!(meeting_resume(&conn, id).unwrap());
        assert_eq!(get_meeting(&conn, id).unwrap().unwrap().state, "recording");
        // And a second resume of the same meeting is refused, not stacked.
        assert!(!meeting_resume(&conn, id).unwrap());

        meeting_cleaning(&conn, id).unwrap();
        meeting_failed(&conn, id, "the model timed out").unwrap();
        assert!(meeting_resume(&conn, id).unwrap()); // failed is resumable too
    }

    /// What goes stale is cleared; what is still the best notes the meeting
    /// has stays until the next Stop replaces it.
    #[test]
    fn a_resume_clears_what_went_stale_and_keeps_the_notes() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_segment(&conn, id, 0, "we agreed thursday").unwrap();
        meeting_cleaning(&conn, id).unwrap();
        set_clean_transcript(&conn, id, "We agreed Thursday.").unwrap();
        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "Planning",
            summary: "Thursday.",
            action_items: &["Send the doc".to_string()],
        }).unwrap();
        let action = meeting_actions(&conn, id).unwrap()[0].id;
        approve_actions(&mut conn, id, &[action]).unwrap();
        let first_end = get_meeting(&conn, id).unwrap().unwrap().ended_at;
        assert!(first_end.is_some());

        assert!(meeting_resume(&conn, id).unwrap());
        let m = get_meeting(&conn, id).unwrap().unwrap();
        assert!(m.ended_at.is_none(), "the next Stop has to be able to stamp its own end");
        assert!(m.error.is_none());
        assert!(!m.has_clean);
        assert!(clean_transcript(&conn, id).unwrap().is_none());
        assert_eq!(m.title, "Planning");
        assert_eq!(meeting_summary(&conn, id).unwrap().0.as_deref(), Some("Thursday."));
        assert!(meeting_actions(&conn, id).unwrap()[0].task_id.is_some());
        assert_eq!(transcript(&conn, id).unwrap(), "we agreed thursday");

        // The next Stop stamps a fresh end rather than keeping the first one.
        meeting_cleaning(&conn, id).unwrap();
        assert!(get_meeting(&conn, id).unwrap().unwrap().ended_at.is_some());
    }

    /// A failed meeting's error is cleared too — otherwise the sweep, which
    /// keeps an existing error, would blame a crash mid-resume on the old one.
    #[test]
    fn a_crash_mid_resume_is_swept_as_a_lost_recording() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_segment(&conn, id, 0, "first part").unwrap();
        meeting_cleaning(&conn, id).unwrap();
        meeting_failed(&conn, id, "the model timed out").unwrap();

        assert!(meeting_resume(&conn, id).unwrap());
        add_segment(&conn, id, 1, "second part").unwrap();
        let last: String = conn
            .query_row("SELECT started_at FROM meeting_segment WHERE meeting_id = ?1 AND seq = 1",
                [id], |r| r.get(0))
            .unwrap();

        assert_eq!(sweep_orphan_meetings(&conn).unwrap(), 1);
        let m = get_meeting(&conn, id).unwrap().unwrap();
        assert_eq!(m.state, "failed");
        assert_eq!(m.error.as_deref(), Some("recording stopped when the app did"));
        assert_eq!(m.ended_at.as_deref(), Some(last.as_str()));
    }

    /// A resumed recording carries on after the highest seq — the unique index
    /// would refuse a reused one — and its first chunk gets the last thing
    /// actually transcribed as context, not a failed chunk's empty text.
    #[test]
    fn a_resume_carries_on_from_the_last_segment() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        assert_eq!(resume_point(&conn, id).unwrap(), (0, String::new()));

        add_segment(&conn, id, 0, "first part").unwrap();
        add_segment(&conn, id, 1, "second part").unwrap();
        add_segment(&conn, id, 2, "  ").unwrap();
        assert_eq!(resume_point(&conn, id).unwrap(), (3, "second part".to_string()));

        let (next, _) = resume_point(&conn, id).unwrap();
        add_segment(&conn, id, next, "after the break").unwrap();
        assert_eq!(transcript(&conn, id).unwrap(), "first part second part after the break");
    }

    /// Both wrap-up states, not just the second. A row stranded in either one
    /// is a Start button that never comes back.
    #[test]
    fn the_orphan_sweep_fails_a_meeting_stranded_while_cleaning() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_segment(&conn, id, 0, "said something").unwrap();
        meeting_cleaning(&conn, id).unwrap();

        assert_eq!(sweep_orphan_meetings(&conn).unwrap(), 1);
        let m = get_meeting(&conn, id).unwrap().unwrap();
        assert_eq!(m.state, "failed");
        assert_eq!(m.error.unwrap(), "the app closed before the notes were written");
        // The transcript survives, which is what makes a re-run worth offering.
        assert_eq!(transcript(&conn, id).unwrap(), "said something");
    }

    /// The write-up is the user's to edit once the model is done with it, and
    /// the store remembers that it was -- so a re-run can ask first. A re-run
    /// then hands the document back to the model and clears the mark.
    #[test]
    fn editing_the_summary_marks_it_and_a_rerun_clears_the_mark() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        assert_eq!(meeting_summary(&conn, id).unwrap(), (None, None));
        // Not while the notes are being written: finish_meeting would only
        // overwrite it moments later.
        meeting_cleaning(&conn, id).unwrap();
        assert!(set_meeting_summary(&conn, id, "too early").is_err());
        assert_eq!(meeting_summary(&conn, id).unwrap(), (None, None));

        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "Roadmap",
            summary: "We picked Q4.\n\n## Key points\n- Ship in Q4",
            action_items: &[],
        }).unwrap();
        let (doc, edited) = meeting_summary(&conn, id).unwrap();
        assert_eq!(doc.as_deref(), Some("We picked Q4.\n\n## Key points\n- Ship in Q4"));
        assert!(edited.is_none());

        set_meeting_summary(&conn, id, "We picked Q4, and $x^2$.").unwrap();
        let (doc, edited) = meeting_summary(&conn, id).unwrap();
        assert_eq!(doc.as_deref(), Some("We picked Q4, and $x^2$."));
        assert!(edited.is_some());

        // A failed meeting is editable too: the document may be a previous
        // run's, or written by hand from the transcript.
        assert!(meeting_rerun(&conn, id).unwrap());
        meeting_failed(&conn, id, "the model timed out").unwrap();
        set_meeting_summary(&conn, id, "by hand").unwrap();

        // The re-run's notes are the model's again.
        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "Roadmap", summary: "Second pass.", action_items: &[],
        }).unwrap();
        assert_eq!(meeting_summary(&conn, id).unwrap(), (Some("Second pass.".into()), None));
        assert!(set_meeting_summary(&conn, id + 99, "nowhere").is_err());
    }

    fn file<'a>(name: &'a str, path: &'a str, bytes: i64, text: &'a str) -> NewFile<'a> {
        NewFile { name, path, kind: "text", bytes, extracted: Some(text) }
    }

    /// Enforced in SQL under the transaction, not just in the UI: two attach
    /// clicks racing would each see room under the cap and both proceed.
    #[test]
    fn the_attachment_limits_hold_against_the_aggregate() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        for i in 0..3 {
            add_meeting_file(
                &mut conn, id, &file("a.txt", &format!("p{i}"), 100, "x"), 3, 10_000, 10_000,
            )
            .unwrap();
        }
        // The count cap.
        let err = add_meeting_file(
            &mut conn, id, &file("d.txt", "p3", 100, "x"), 3, 10_000, 10_000,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("already has 3 files"), "{err}");

        // The byte cap, on the total rather than the one file.
        let err = add_meeting_file(
            &mut conn, id, &file("e.txt", "p4", 9_000, "x"), 9, 1024 * 1024, 10_000,
        );
        assert!(err.is_ok(), "9 kB fits inside 1 MB");
        let err = add_meeting_file(
            &mut conn, id, &file("f.txt", "p5", 2 * 1024 * 1024, "x"), 9, 1024 * 1024, 10_000,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("attachments"), "{err}");

        // The character cap -- the one that bounds the bill rather than disk.
        let long = "y".repeat(9_000);
        let err = add_meeting_file(
            &mut conn, id, &file("g.txt", "p6", 10, &long), 9, 1024 * 1024, 100,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("text this can send"), "{err}");
    }

    #[test]
    fn attaching_to_a_missing_meeting_is_refused() {
        let mut conn = store();
        assert!(add_meeting_file(
            &mut conn, 999, &file("a.txt", "p", 1, "x"), 8, 10_000, 10_000
        )
        .is_err());
    }

    /// Deleting hands back the copy to unlink -- the row goes first, so a file
    /// left behind is litter the sweep collects rather than a broken chip.
    #[test]
    fn deleting_an_attachment_returns_the_path_to_unlink() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        let file_id = add_meeting_file(
            &mut conn, id, &file("deck.pptx", "/tmp/opaque123", 4096, "slides"), 8, 10_000, 10_000,
        )
        .unwrap();
        assert_eq!(meeting_file_rows(&conn, id).unwrap()[0].path, "/tmp/opaque123");

        let path = delete_meeting_file(&conn, id, file_id).unwrap();
        assert_eq!(path, "/tmp/opaque123");
        assert!(meeting_files(&conn, id).unwrap().is_empty());
        // A second delete is not a second unlink.
        assert!(delete_meeting_file(&conn, id, file_id).is_err());
    }

    /// Positions keep counting up, so removing the first chip does not make
    /// the next attachment collide with the second.
    #[test]
    fn attachment_positions_do_not_collide_after_a_removal() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        let first = add_meeting_file(
            &mut conn, id, &file("a", "p0", 1, "x"), 8, 10_000, 10_000,
        ).unwrap();
        add_meeting_file(&mut conn, id, &file("b", "p1", 1, "x"), 8, 10_000, 10_000).unwrap();
        delete_meeting_file(&conn, id, first).unwrap();
        add_meeting_file(&mut conn, id, &file("c", "p2", 1, "x"), 8, 10_000, 10_000).unwrap();

        let files = meeting_files(&conn, id).unwrap();
        let positions: Vec<i64> = files.iter().map(|f| f.position).collect();
        assert_eq!(positions, vec![1, 2]);
        assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["b", "c"]);
    }

    /// The model pipeline's view carries the path; the webview's does not.
    #[test]
    fn only_the_model_side_sees_the_stored_path() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        add_meeting_file(
            &mut conn, id, &file("spec.md", "/tmp/opaque", 12, "the body"), 8, 10_000, 10_000,
        )
        .unwrap();
        let rows = meeting_file_rows(&conn, id).unwrap();
        assert_eq!(rows[0].path, "/tmp/opaque");
        assert_eq!(rows[0].extracted.as_deref(), Some("the body"));

        let shown = serde_json::to_string(&meeting_files(&conn, id).unwrap()).unwrap();
        assert!(!shown.contains("opaque"), "the path reached the webview: {shown}");
        assert!(!shown.contains("the body"), "the contents reached the webview");
        assert!(shown.contains("spec.md"));
    }

    /// The list row counts attachments without carrying them.
    #[test]
    fn a_meeting_row_counts_its_files() {
        let conn = store();
        let id = start_meeting(&conn, None).unwrap();
        assert_eq!(get_meeting(&conn, id).unwrap().unwrap().file_count, 0);
        conn.execute(
            "INSERT INTO meeting_file (meeting_id, position, name, path, kind, bytes, added_at)
             VALUES (?1, 0, 'deck.pptx', 'ab12cd34', 'office', 4096, ?2)",
            params![id, now()],
        ).unwrap();
        assert_eq!(get_meeting(&conn, id).unwrap().unwrap().file_count, 1);
        let files = meeting_files(&conn, id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "deck.pptx");
        assert_eq!(files[0].kind, "office");
    }

    /// orphan that task and offer the same work twice.
    #[test]
    fn resummarizing_keeps_approved_actions_and_replaces_the_rest() {
        let mut conn = store();
        let id = start_meeting(&conn, None).unwrap();
        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "First pass",
            summary: "s",
            action_items: &["Keep me".to_string(), "Replace me".to_string()],
        }).unwrap();
        let keep = meeting_actions(&conn, id).unwrap()[0].id;
        approve_actions(&mut conn, id, &[keep]).unwrap();

        finish_meeting(&mut conn, id, &MeetingNotes {
            title: "Second pass",
            summary: "s2",
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
        assert!(meeting_cleaning(&conn, id).unwrap());
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
            title: "t", summary: "s",
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
        assert!(meeting_cleaning(&conn, id).unwrap());
        assert!(!meeting_cleaning(&conn, id).unwrap());
    }
}
