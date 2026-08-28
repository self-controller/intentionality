//! All SQLite access. Mirrors gate/store.py's contract: schema migrations
//! belong to Python; this side only checks meta.schema_version and refuses
//! politely when the store is older than it understands.

use crate::error::{AppError, Result};
use crate::models::{Analysis, Arrangement, Session, Task};
use chrono::Utc;
use rusqlite::{params, Connection, TransactionBehavior};
use std::path::PathBuf;

pub const SCHEMA_VERSION: i64 = 2;
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

fn row_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        close_reason: row.get("close_reason")?,
        statement: row.get("statement")?,
        intended_minutes: row.get("intended_minutes")?,
    })
}

pub fn get_session(conn: &Connection, id: i64) -> Result<Option<Session>> {
    conn.query_row("SELECT * FROM session WHERE id = ?1", [id], row_session)
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e.into()),
        })
}

pub fn latest_open_session(conn: &Connection) -> Result<Option<Session>> {
    conn.query_row(
        "SELECT * FROM session WHERE ended_at IS NULL AND close_reason IS NULL
         ORDER BY id DESC LIMIT 1",
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
    let mut stmt = conn.prepare("SELECT * FROM session ORDER BY id DESC LIMIT ?1")?;
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
    })
}

pub fn list_analyses(conn: &Connection, session_id: i64) -> Result<Vec<Analysis>> {
    let mut stmt =
        conn.prepare("SELECT * FROM analysis WHERE session_id = ?1 ORDER BY id DESC")?;
    let rows = stmt.query_map([session_id], row_analysis)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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

#[allow(clippy::too_many_arguments)]
pub fn add_analysis(
    conn: &Connection,
    session_id: i64,
    window_start: &str,
    window_end: &str,
    headline: &str,
    alignment: Option<i64>,
    body: &str,
    observed_json: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO analysis (session_id, created_at, window_start, window_end,
             headline, alignment, body, observed_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![session_id, now(), window_start, window_end, headline, alignment, body, observed_json],
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
