//! All SQLite access. Mirrors gate/store.py's contract: schema migrations
//! belong to Python; this side only checks meta.schema_version and refuses
//! politely when the store is older than it understands.

use crate::autolabel::Candidate;
use crate::error::{AppError, Result};
use crate::models::{
    Analysis, AnalysisDetail, Arrangement, Label, LabelSummary, RiskApp, Session, Task,
};
use crate::recommendations;
use chrono::{Duration, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::collections::HashMap;
use std::path::PathBuf;

pub const SCHEMA_VERSION: i64 = 14;
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
    SELECT t.id, t.session_id, t.title, t.position, t.status, t.notes, t.due_date, t.priority,
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
        priority: row.get("priority")?,
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
/// `session_id` is the list the tasks came from (None = the backlog), so only
/// that list's labels are read, not every tag ever worn.
fn attach_labels(conn: &Connection, session_id: Option<i64>, tasks: &mut [Task]) -> Result<()> {
    if tasks.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "SELECT tl.task_id, l.name, l.color
         FROM task_label tl JOIN label l ON l.id = tl.label_id
         WHERE tl.task_id IN (SELECT id FROM task WHERE session_id IS ?1)
         ORDER BY l.name",
    )?;
    let rows = stmt.query_map([session_id], |r| {
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

/// Every label by name, with up to `per_label` titles filed under it, newest
/// first, each wording once whatever its case and however often it was
/// carried: what a label means in practice, for autolabel to show Jev.
/// gate/store.py's label_examples is the same query, for the gate.
pub fn label_candidates(conn: &Connection, per_label: usize) -> Result<Vec<Candidate>> {
    let mut stmt = conn.prepare("SELECT name FROM label ORDER BY name")?;
    let mut out: Vec<Candidate> = stmt
        .query_map([], |r| Ok(Candidate { name: r.get(0)?, examples: Vec::new() }))?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(
        "SELECT l.name, t.title, MAX(t.id) AS newest FROM label l
         JOIN task_label tl ON tl.label_id = l.id JOIN task t ON t.id = tl.task_id
         GROUP BY l.id, lower(trim(t.title)) ORDER BY l.name, newest DESC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (name, title) = row?;
        let titles = by_name.entry(name).or_default();
        if titles.len() < per_label {
            titles.push(title.trim().to_string());
        }
    }
    for c in out.iter_mut() {
        c.examples = by_name.remove(&c.name).unwrap_or_default();
    }
    Ok(out)
}

/// A task's title and notes, if it exists and wears no label yet: one that
/// autolabel may still file.
pub fn unlabelled_task(conn: &Connection, id: i64) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT title, notes FROM task t WHERE id = ?1
             AND NOT EXISTS (SELECT 1 FROM task_label WHERE task_id = t.id)",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Put an existing label on a task that still wears none. One statement, so
/// a label put on by hand while Jev was answering wins, as does a label or a
/// task deleted meanwhile; never makes a label. True = it went on.
pub fn file_under(conn: &Connection, id: i64, label: &str) -> Result<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO task_label (task_id, label_id)
         SELECT t.id, l.id FROM task t JOIN label l ON l.name = ?2
         WHERE t.id = ?1 AND NOT EXISTS (SELECT 1 FROM task_label WHERE task_id = t.id)",
        params![id, label],
    )?;
    Ok(changed == 1)
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

/// Recolour a label everywhere it is worn. `#rrggbb` only, stored lowercase,
/// so the palette's own values and a picked colour look alike in the store.
pub fn set_label_color(conn: &Connection, name: &str, color: &str) -> Result<()> {
    let color = color.trim().to_ascii_lowercase();
    let canonical = color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|b| b.is_ascii_hexdigit());
    if !canonical {
        return Err(AppError::Other(format!("colour must be #rrggbb, not {color:?}")));
    }
    let changed = conn.execute("UPDATE label SET color = ?1 WHERE name = ?2", [&color, name.trim()])?;
    if changed == 0 {
        return Err(AppError::Other(format!("no label {name:?}")));
    }
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

/// Longest name a flag may have. Long enough for any domain worth typing.
pub const MAX_RISK_APP_CHARS: usize = 80;

/// The flagged apps and websites, oldest first: when two match one window,
/// the timer names the one flagged first.
pub fn list_risk_apps(conn: &Connection) -> Result<Vec<RiskApp>> {
    let mut stmt = conn.prepare("SELECT id, name FROM risk_app ORDER BY id")?;
    let rows = stmt.query_map([], |r| Ok(RiskApp { id: r.get("id")?, name: r.get("name")? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Flag an app or website, or find the flag there is: risk_app.name is
/// NOCASE, so LinkedIn.com after linkedin.com is a no-op returning the first.
pub fn add_risk_app(conn: &Connection, name: &str) -> Result<RiskApp> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Other("name an app or a website".into()));
    }
    if name.chars().count() > MAX_RISK_APP_CHARS {
        return Err(AppError::Other(format!(
            "keep it under {MAX_RISK_APP_CHARS} characters"
        )));
    }
    conn.execute(
        "INSERT OR IGNORE INTO risk_app (name, created_at) VALUES (?1, ?2)",
        params![name, now()],
    )?;
    Ok(conn.query_row("SELECT id, name FROM risk_app WHERE name = ?1", [name], |r| {
        Ok(RiskApp { id: r.get("id")?, name: r.get("name")? })
    })?)
}

pub fn remove_risk_app(conn: &Connection, id: i64) -> Result<()> {
    if conn.execute("DELETE FROM risk_app WHERE id = ?1", [id])? == 0 {
        return Err(AppError::Other("no such app".into()));
    }
    Ok(())
}

/// The session's cards in Doing: what a page can be relevant to. To Do,
/// Done and dropped ones excuse nothing.
pub fn doing_session_tasks(conn: &Connection, session_id: i64) -> Result<Vec<Task>> {
    let mut tasks = session_tasks(conn, session_id)?;
    tasks.retain(|t| t.status == "doing");
    Ok(tasks)
}

/// A cached verdict: Jev's probability that one page is relevant to one
/// task, with where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct VerdictRow {
    pub task_text: String,
    pub task_title: String,
    pub probability: f64,
    pub source: String,
    pub argument: Option<String>,
    pub model: String,
    pub created_at: String,
}

/// A verdict to store. One row per (app, page, task_text): a newer answer
/// for the same three replaces the old one.
pub struct NewVerdict<'a> {
    pub app: &'a str,
    pub page: &'a str,
    pub title: &'a str,
    pub task_text: &'a str,
    pub task_title: &'a str,
    pub probability: f64,
    pub source: &'a str,
    pub argument: Option<&'a str>,
    pub model: &'a str,
}

/// Everything known about one page, whatever task it was judged against.
pub fn relevance_for_page(conn: &Connection, app: &str, page: &str) -> Result<Vec<VerdictRow>> {
    let mut stmt = conn.prepare(
        "SELECT task_text, task_title, probability, source, argument, model, created_at
           FROM relevance WHERE app = ?1 AND page = ?2 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![app, page], |r| {
        Ok(VerdictRow {
            task_text: r.get("task_text")?,
            task_title: r.get("task_title")?,
            probability: r.get("probability")?,
            source: r.get("source")?,
            argument: r.get("argument")?,
            model: r.get("model")?,
            created_at: r.get("created_at")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn relevance_upsert(conn: &Connection, v: &NewVerdict) -> Result<()> {
    conn.execute(
        "INSERT INTO relevance (app, page, title, task_text, task_title, probability, source,
                                argument, model, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (app, page, task_text) DO UPDATE SET
             title = excluded.title, task_title = excluded.task_title,
             probability = excluded.probability, source = excluded.source,
             argument = excluded.argument, model = excluded.model,
             created_at = excluded.created_at",
        params![
            v.app,
            v.page,
            v.title,
            v.task_text,
            v.task_title,
            v.probability,
            v.source,
            v.argument,
            v.model,
            now()
        ],
    )?;
    Ok(())
}

/// Every verdict gone. Returns how many there were.
pub fn relevance_forget(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM relevance", [])?)
}

/// An appeal the user won: the window, the task it won for, and the case
/// they made. Jev is shown these with every page, so a reason that held for
/// one tab holds for the next one like it.
#[derive(Debug, Clone, PartialEq)]
pub struct Precedent {
    pub app: String,
    pub title: String,
    pub task_title: String,
    pub argument: String,
}

/// The newest won appeals, at most `limit`, one per page and argument -- an
/// appeal is stored once per task it was judged against.
pub fn relevance_precedents(conn: &Connection, min: f64, limit: i64) -> Result<Vec<Precedent>> {
    // A bare column next to MAX() comes from the row holding the max.
    let mut stmt = conn.prepare(
        "SELECT app, title, task_title, argument, MAX(created_at) AS at
           FROM relevance
          WHERE source = 'appeal' AND argument IS NOT NULL AND probability >= ?1
          GROUP BY app, page, argument
          ORDER BY at DESC
          LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![min, limit], |r| {
        Ok(Precedent {
            app: r.get("app")?,
            title: r.get("title")?,
            task_title: r.get("task_title")?,
            argument: r.get("argument")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// After an appeal won for these tasks: drop Jev's own below-the-bar
/// verdicts for them, so every page it turned down is asked again, with the
/// new precedent, the next time it is in front. Returns how many went.
pub fn relevance_reopen(conn: &Connection, task_texts: &[String], min: f64) -> Result<usize> {
    let mut gone = 0;
    for text in task_texts {
        gone += conn.execute(
            "DELETE FROM relevance WHERE source = 'model' AND probability < ?1 AND task_text = ?2",
            params![min, text],
        )?;
    }
    Ok(gone)
}

pub fn relevance_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM relevance", [], |r| r.get(0))?)
}

/// The probability at or above which a task excuses a page: meta
/// `relevance_min`, or the default. Anything unparseable or outside (0, 1]
/// is the default too -- a typo must not turn every page relevant.
pub fn relevance_min(conn: &Connection) -> f64 {
    get_setting(conn, "relevance_min")
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|m| *m > 0.0 && *m <= 1.0)
        .unwrap_or(crate::relevance::DEFAULT_MIN)
}

/// Unexcused at-risk time that starts a check early, in minutes: meta
/// `drift_check_minutes`, default 10. 0 or less turns it off; anything
/// unparseable is the default.
pub fn drift_check_minutes(conn: &Connection) -> f64 {
    get_setting(conn, "drift_check_minutes")
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|m| m.is_finite())
        .map(|m| m.max(0.0))
        .unwrap_or(DEFAULT_DRIFT_CHECK_MINUTES)
}

pub const DEFAULT_DRIFT_CHECK_MINUTES: f64 = 10.0;

pub fn session_tasks(conn: &Connection, session_id: i64) -> Result<Vec<Task>> {
    let mut stmt =
        conn.prepare(&format!("{TASK_SELECT} WHERE t.session_id = ?1 ORDER BY t.position"))?;
    let rows = stmt.query_map([session_id], row_task)?;
    let mut tasks: Vec<Task> = rows.collect::<rusqlite::Result<_>>()?;
    attach_labels(conn, Some(session_id), &mut tasks)?;
    Ok(tasks)
}

pub fn backlog(conn: &Connection) -> Result<Vec<Task>> {
    let mut stmt =
        conn.prepare(&format!("{TASK_SELECT} WHERE t.session_id IS NULL ORDER BY t.position"))?;
    let rows = stmt.query_map([], row_task)?;
    let mut tasks: Vec<Task> = rows.collect::<rusqlite::Result<_>>()?;
    attach_labels(conn, None, &mut tasks)?;
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
    priority: &str,
    labels: &[String],
) -> Result<i64> {
    check_due(due_date)?;
    check_priority(priority)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Inside the write lock, as in apply_board.
    if let Some(sid) = session_id {
        ensure_open(&tx, sid)?;
    }
    let id = insert_task(&tx, session_id, title)?;
    write_details(&tx, id, title, notes, due_date, priority, labels)?;
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
    priority: &str,
    labels: &[String],
) -> Result<()> {
    check_due(due_date)?;
    check_priority(priority)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    write_details(&tx, id, title, notes, due_date, priority, labels)?;
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

/// The three levels task.priority's CHECK allows. Refused here too, so a bad
/// value is a readable error rather than a constraint failure.
fn check_priority(priority: &str) -> Result<()> {
    match priority {
        "high" | "medium" | "low" => Ok(()),
        other => Err(AppError::Other(format!("priority must be high, medium or low, not {other:?}"))),
    }
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
    priority: &str,
    labels: &[String],
) -> Result<()> {
    let changed = conn.execute(
        "UPDATE task SET title = ?1, notes = ?2, due_date = ?3, priority = ?4 WHERE id = ?5",
        params![title, notes, due_date, priority, id],
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

/// Remove a task completely: a backlog row, or a card on a session that is
/// still open. Labels go by cascade, a carried copy keeps living with its
/// `carried_from` cleared, an approved meeting action reads as unapproved
/// again. A closed session's cards are finished history and stay.
///
/// One statement, so the open-session test and the delete cannot be split by
/// the gate closing the session in between (same reason as `ensure_open`
/// running inside `apply_board`'s write lock).
pub fn delete_task(conn: &Connection, id: i64) -> Result<()> {
    let changed = conn.execute(
        "DELETE FROM task WHERE id = ?1 AND (session_id IS NULL OR session_id IN
            (SELECT id FROM session WHERE ended_at IS NULL AND close_reason IS NULL))",
        [id],
    )?;
    if changed == 0 {
        return Err(AppError::Other(
            "that task can't be deleted: it isn't in the backlog or on an open session".into(),
        ));
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

/// Current session -> backlog: pull_task in reverse. The card goes back to
/// being unstarted work at the end of the backlog; its notes, labels, due date
/// and carry chain stay. A dropped card is history and stays where it is.
pub fn unpull_task(conn: &mut Connection, session_id: i64, id: i64) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Inside the write lock, as in apply_board.
    ensure_open(&tx, session_id)?;
    let next: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM task WHERE session_id IS NULL",
        [],
        |r| r.get(0),
    )?;
    let changed = tx.execute(
        "UPDATE task SET session_id = NULL, status = 'planned', position = ?1,
            started_at = NULL, resolved_at = NULL
         WHERE id = ?2 AND session_id = ?3 AND status <> 'dropped'",
        params![next, id, session_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other(format!("task {id} is not on this board")));
    }
    tx.commit()?;
    Ok(())
}

/// The backlog in the order the board shows it, from a drag inside it or a
/// card dropped into it. It belongs to no session, so unlike apply_board
/// there is no session to check: the IS NULL guard is the whole fence, and a
/// row the gate pulled or the board deleted meanwhile fails the write rather
/// than being yanked back. The gate lists the backlog by position too, so
/// this order is also the order it offers.
pub fn order_backlog(conn: &mut Connection, ids: &[i64]) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Everything up by n first, so a row this list doesn't name (added
    // since the board last read) keeps its place after the ones it does.
    tx.execute(
        "UPDATE task SET position = position + ?1 WHERE session_id IS NULL",
        [ids.len() as i64],
    )?;
    for (pos, id) in ids.iter().enumerate() {
        let changed = tx.execute(
            "UPDATE task SET position = ?1 WHERE id = ?2 AND session_id IS NULL",
            params![(pos + 1) as i64, id],
        )?;
        if changed == 0 {
            return Err(AppError::Other(format!("task {id} is not in the backlog")));
        }
    }
    tx.commit()?;
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
// Every column row_analysis reads, and not observed_json or judged_json:
// those blobs are only ever wanted for one row, through analysis_detail.
const ANALYSIS_SELECT: &str = "SELECT a.id, a.session_id, a.created_at, a.window_start,
        a.window_end, a.headline, a.alignment, a.body, a.seen_at, a.kind,
        a.recommendation_id, a.recommendation_note, s.statement AS session_statement
     FROM analysis a LEFT JOIN session s ON s.id = a.session_id";

pub fn get_analysis(conn: &Connection, id: i64) -> Result<Option<Analysis>> {
    Ok(conn
        .query_row(&format!("{ANALYSIS_SELECT} WHERE a.id = ?1"), [id], row_analysis)
        .optional()?)
}

/// The session's most recent analysis: where the next window starts, and
/// what the model said last time.
pub fn latest_analysis(conn: &Connection, session_id: i64) -> Result<Option<Analysis>> {
    Ok(conn
        .query_row(
            &format!("{ANALYSIS_SELECT} WHERE a.session_id = ?1 ORDER BY a.id DESC LIMIT 1"),
            [session_id],
            row_analysis,
        )
        .optional()?)
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

/// The AW breakdown this analysis was judged against, and what Jev judged.
/// A missing row or unparseable observed JSON reads as an empty window
/// rather than an error — the analysis itself is still worth showing. No
/// judged JSON (every row from before v14) reads as None: the tab then shows
/// the row the way it was written.
pub fn analysis_detail(conn: &Connection, id: i64) -> Result<AnalysisDetail> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT observed_json, judged_json FROM analysis WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (observed, judged) = row.unwrap_or_default();
    Ok(AnalysisDetail {
        observed: serde_json::from_str(&observed).unwrap_or_default(),
        judged: judged.and_then(|j| serde_json::from_str(&j).ok()),
    })
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
    /// models::Judged as JSON. Every row written since v14 has one.
    pub judged_json: &'a str,
}

pub fn add_analysis(conn: &Connection, a: &NewAnalysis) -> Result<i64> {
    conn.execute(
        "INSERT INTO analysis (session_id, created_at, window_start, window_end,
             headline, alignment, body, observed_json, kind,
             recommendation_id, recommendation_note, judged_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            a.session_id, now(), a.window_start, a.window_end, a.headline,
            a.alignment, a.body, a.observed_json, a.kind,
            a.recommendation_id, a.recommendation_note, a.judged_json
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .ok())
}

/// The off-task clock's running total for one day (screen_time.rs), kept in
/// meta as "YYYY-MM-DD <seconds>" so a restart carries on from it. Another
/// day's value, or one that does not parse, is no value.
const OFF_TASK_KEY: &str = "off_task_today";

fn parse_off_task(value: &str, day: NaiveDate) -> Option<f64> {
    let (date, seconds) = value.split_once(' ')?;
    if date != day.to_string() {
        return None;
    }
    seconds.parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0)
}

pub fn off_task_today(conn: &Connection, day: NaiveDate) -> Option<f64> {
    get_setting(conn, OFF_TASK_KEY).ok().flatten().and_then(|v| parse_off_task(&v, day))
}

pub fn set_off_task_today(conn: &Connection, day: NaiveDate, seconds: f64) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![OFF_TASK_KEY, format!("{day} {seconds:.1}")],
    )?;
    Ok(())
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

    /// A check's snapshot comes back as written; a row from before v14 has
    /// none, and reads as None rather than an empty snapshot, which is what
    /// tells the tab to show it the old way.
    #[test]
    fn an_analysis_keeps_its_snapshot_and_an_old_row_reads_as_unjudged() {
        let conn = store();
        let (session_id, _) = session_with_task(&conn);
        let judged = crate::models::Judged {
            models: vec!["jev-1.14.0".into()],
            threshold: 0.7,
            trigger: "drift".into(),
            aligned_seconds: 600.0,
            ..Default::default()
        };
        fn row(session_id: i64, judged_json: &str) -> NewAnalysis<'_> {
            NewAnalysis {
                session_id,
                kind: "check",
                window_start: "2026-10-27T09:00:00+00:00",
                window_end: "2026-10-27T10:00:00+00:00",
                headline: "Mostly on the board",
                alignment: Some(80),
                body: "b",
                observed_json: "{}",
                recommendation_id: None,
                recommendation_note: None,
                judged_json,
            }
        }
        let json = serde_json::to_string(&judged).unwrap();
        let id = add_analysis(&conn, &row(session_id, &json)).unwrap();
        assert_eq!(analysis_detail(&conn, id).unwrap().judged, Some(judged));

        let old = add_analysis(&conn, &row(session_id, "{}")).unwrap();
        conn.execute("UPDATE analysis SET judged_json = NULL WHERE id = ?1", [old]).unwrap();
        let detail = analysis_detail(&conn, old).unwrap();
        assert!(detail.judged.is_none());
        assert_eq!(detail.observed.active_seconds, 0.0);
        // A row that does not exist is an empty detail, not an error.
        assert!(analysis_detail(&conn, 999).unwrap().judged.is_none());
    }

    #[test]
    fn the_off_task_clock_keeps_one_day_only() {
        let conn = store();
        let day = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        assert_eq!(off_task_today(&conn, day), None);
        set_off_task_today(&conn, day, 1981.5).unwrap();
        assert_eq!(off_task_today(&conn, day), Some(1981.5));
        assert_eq!(off_task_today(&conn, day.succ_opt().unwrap()), None);
        for bad in ["2026-10-29", "2026-10-29 soon", "2026-10-29 -4", "2026-10-29 inf"] {
            assert_eq!(parse_off_task(bad, day), None, "{bad}");
        }
    }

    #[test]
    fn drift_minutes_default_to_ten_and_turn_off_at_zero() {
        let conn = store();
        assert_eq!(drift_check_minutes(&conn), DEFAULT_DRIFT_CHECK_MINUTES);
        for (value, expect) in [("5", 5.0), ("0", 0.0), ("-3", 0.0), ("soon", 10.0)] {
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('drift_check_minutes', ?1)",
                [value],
            )
            .unwrap();
            assert_eq!(drift_check_minutes(&conn), expect, "{value}");
        }
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
    fn a_risk_app_is_flagged_once_whatever_its_case() {
        let conn = store();
        let first = add_risk_app(&conn, "  linkedin.com ").unwrap();
        assert_eq!(first.name, "linkedin.com");
        assert_eq!(add_risk_app(&conn, "LinkedIn.com").unwrap(), first);
        let second = add_risk_app(&conn, "discord").unwrap();
        assert_eq!(list_risk_apps(&conn).unwrap(), [first.clone(), second.clone()]);

        remove_risk_app(&conn, first.id).unwrap();
        assert_eq!(list_risk_apps(&conn).unwrap(), [second]);
        assert!(remove_risk_app(&conn, first.id).is_err());
    }

    fn verdict<'a>(task_text: &'a str, probability: f64, source: &'a str) -> NewVerdict<'a> {
        NewVerdict {
            app: "app.zen_browser.zen",
            page: "feed | linkedin",
            title: "(3) Feed | LinkedIn — Zen Browser",
            task_text,
            task_title: task_text.lines().next().unwrap_or(""),
            probability,
            source,
            argument: None,
            model: "jev-1.14.0",
        }
    }

    #[test]
    fn a_verdict_is_one_row_per_page_and_task_and_the_newest_wins() {
        let conn = store();
        relevance_upsert(&conn, &verdict("apply to 3 roles\n", 0.2, "model")).unwrap();
        relevance_upsert(&conn, &verdict("write ch. 3\n", 0.05, "model")).unwrap();
        let mut argued = verdict("apply to 3 roles\n", 0.9, "appeal");
        argued.argument = Some("I am applying there");
        relevance_upsert(&conn, &argued).unwrap();
        assert_eq!(relevance_count(&conn).unwrap(), 2);
        let rows = relevance_for_page(&conn, "app.zen_browser.zen", "feed | linkedin").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].task_text, "apply to 3 roles\n");
        assert_eq!((rows[0].probability, rows[0].source.as_str()), (0.9, "appeal"));
        assert_eq!(rows[0].argument.as_deref(), Some("I am applying there"));
        assert_eq!(rows[1].probability, 0.05);
        // Another page knows nothing of this one.
        assert!(relevance_for_page(&conn, "app.zen_browser.zen", "inbox").unwrap().is_empty());
        assert_eq!(relevance_forget(&conn).unwrap(), 2);
        assert_eq!(relevance_count(&conn).unwrap(), 0);
    }

    #[test]
    fn precedents_are_won_appeals_newest_first_once_per_page_and_argument() {
        let conn = store();
        let on = |page: &'static str, task: &'static str, p: f64, source: &'static str, arg: Option<&'static str>| {
            NewVerdict { page, title: page, argument: arg, ..verdict(task, p, source) }
        };
        // One appeal, judged against two tasks: won for one only.
        relevance_upsert(&conn, &on("feed | linkedin", "apply\n", 0.9, "appeal", Some("recruiters"))).unwrap();
        relevance_upsert(&conn, &on("feed | linkedin", "write\n", 0.1, "appeal", Some("recruiters"))).unwrap();
        // Lost, and Jev's own verdicts, are not precedents.
        relevance_upsert(&conn, &on("youtube", "apply\n", 0.3, "appeal", Some("a talk"))).unwrap();
        relevance_upsert(&conn, &on("docs", "write\n", 0.95, "model", None)).unwrap();
        conn.execute(
            "UPDATE relevance SET created_at = '2026-10-28T09:00:00+00:00' WHERE page = 'feed | linkedin'",
            [],
        )
        .unwrap();
        relevance_upsert(&conn, &on("stack overflow", "write\n", 0.8, "appeal", Some("the bug"))).unwrap();

        let got = relevance_precedents(&conn, 0.7, 12).unwrap();
        let seen: Vec<(&str, &str, &str)> =
            got.iter().map(|p| (p.title.as_str(), p.task_title.as_str(), p.argument.as_str())).collect();
        assert_eq!(seen, [("stack overflow", "write", "the bug"), ("feed | linkedin", "apply", "recruiters")]);
        assert_eq!(relevance_precedents(&conn, 0.7, 1).unwrap().len(), 1);
    }

    #[test]
    fn reopening_drops_only_jevs_own_turn_downs_for_the_tasks_won() {
        let conn = store();
        let on = |page: &'static str, task: &'static str, p: f64, source: &'static str| NewVerdict {
            page,
            ..verdict(task, p, source)
        };
        relevance_upsert(&conn, &on("a", "apply\n", 0.2, "model")).unwrap(); // goes
        relevance_upsert(&conn, &on("b", "apply\n", 0.9, "model")).unwrap(); // relevant: stays
        relevance_upsert(&conn, &on("c", "apply\n", 0.1, "appeal")).unwrap(); // argued: stays
        relevance_upsert(&conn, &on("a", "write\n", 0.2, "model")).unwrap(); // other task: stays
        assert_eq!(relevance_reopen(&conn, &["apply\n".into()], 0.7).unwrap(), 1);
        assert!(relevance_for_page(&conn, "app.zen_browser.zen", "a")
            .unwrap()
            .iter()
            .all(|r| r.task_text == "write\n"));
        assert_eq!(relevance_count(&conn).unwrap(), 3);
    }

    #[test]
    fn the_relevance_bar_is_the_setting_or_the_default_never_a_typo() {
        let conn = store();
        assert_eq!(relevance_min(&conn), crate::relevance::DEFAULT_MIN);
        for (value, want) in [("0.55", 0.55), ("1", 1.0), ("0", crate::relevance::DEFAULT_MIN),
                              ("1.5", crate::relevance::DEFAULT_MIN), ("high", crate::relevance::DEFAULT_MIN)] {
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('relevance_min', ?1)",
                [value],
            )
            .unwrap();
            assert_eq!(relevance_min(&conn), want, "{value}");
        }
    }

    #[test]
    fn only_doing_cards_can_excuse_a_page() {
        let conn = store();
        let (sid, _planned) = session_with_task(&conn);
        let doing = add_task(&conn, Some(sid), "doing one").unwrap();
        let done = add_task(&conn, Some(sid), "done one").unwrap();
        let dropped = add_task(&conn, Some(sid), "dropped one").unwrap();
        for (id, status) in [(doing, "doing"), (done, "done"), (dropped, "dropped")] {
            conn.execute("UPDATE task SET status = ?1 WHERE id = ?2", params![status, id]).unwrap();
        }
        let ids: Vec<i64> = doing_session_tasks(&conn, sid).unwrap().iter().map(|t| t.id).collect();
        assert_eq!(ids, [doing]);
    }

    #[test]
    fn a_risk_app_needs_a_name_of_sane_length() {
        let conn = store();
        assert!(add_risk_app(&conn, "   ").is_err());
        assert!(add_risk_app(&conn, &"x".repeat(MAX_RISK_APP_CHARS + 1)).is_err());
        assert!(add_risk_app(&conn, &"x".repeat(MAX_RISK_APP_CHARS)).is_ok());
        assert_eq!(list_risk_apps(&conn).unwrap().len(), 1);
    }

    /// Labels are read per list: the backlog (session_id IS NULL) must get
    /// its own tags and not a session task's, and the other way round.
    #[test]
    fn each_list_reads_only_its_own_labels() {
        let mut conn = store();
        let (_, task_id) = session_with_task(&conn);
        let later = add_task(&conn, None, "later").unwrap();
        update_task(&mut conn, later, "later", "", None, "medium", &["someday".into()]).unwrap();
        update_task(&mut conn, task_id, "write the thing", "", None, "medium", &["now".into()]).unwrap();

        let backlog = backlog(&conn).unwrap();
        assert_eq!(backlog.len(), 1);
        assert_eq!(
            backlog[0].labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["someday"]
        );
        assert_eq!(label_names(&conn, task_id), ["now"]);
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
            "medium",
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
        update_task(&mut conn, first, "a", "", None, "medium", &["billing".into()]).unwrap();
        let color = list_labels(&conn).unwrap()[0].color.clone();

        update_task(&mut conn, second, "b", "", None, "medium", &["BILLING".into()]).unwrap();
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
        update_task(&mut conn, first, "a", "", None, "medium", &["billing".into()]).unwrap();
        update_task(&mut conn, second, "b", "", None, "medium", &["billing".into(), "urgent".into()]).unwrap();
        let uses = |conn: &Connection| -> Vec<(String, i64)> {
            list_labels(conn).unwrap().into_iter().map(|l| (l.name, l.uses)).collect()
        };
        assert_eq!(uses(&conn), [("billing".into(), 2), ("urgent".into(), 1)]);

        update_task(&mut conn, first, "a", "", None, "medium", &[]).unwrap();
        update_task(&mut conn, second, "b", "", None, "medium", &[]).unwrap();
        assert_eq!(uses(&conn), [("billing".into(), 0), ("urgent".into(), 0)]);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM task_label", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn a_label_is_reused_by_name_and_keeps_its_colour() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        let other = add_task(&conn, Some(session_id), "another").unwrap();
        update_task(&mut conn, task_id, "a", "", None, "medium", &["  Reading  ".into()]).unwrap();
        let made = list_labels(&conn).unwrap().pop().unwrap();
        assert_eq!((made.name.as_str(), made.uses), ("Reading", 1));
        // Tagging with it again, in another case, is the same label.
        update_task(&mut conn, other, "b", "", None, "medium", &["READING".into()]).unwrap();
        let labels = list_labels(&conn).unwrap();
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].uses, 2);
        let task = session_tasks(&conn, session_id).unwrap().pop().unwrap();
        assert_eq!(task.labels, [Label { name: "Reading".into(), color: made.color }]);
    }

    #[test]
    fn deleting_a_label_takes_it_off_every_card() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        update_task(&mut conn, task_id, "a", "", None, "medium", &["billing".into(), "urgent".into()])
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
            "medium",
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
            create_task(&mut conn, None, "for later", "", None, "medium", &["later".into()]).unwrap();
        let rows = backlog(&conn).unwrap();
        assert_eq!(rows.iter().map(|t| t.id).collect::<Vec<_>>(), [backlog_id]);
        assert_eq!(rows[0].labels.len(), 1);
    }

    #[test]
    fn label_candidates_are_every_label_with_its_newest_distinct_titles() {
        let mut conn = store();
        let (session_id, first) = session_with_task(&conn);
        update_task(&mut conn, first, "HW 5", "", None, "medium", &["School".into()]).unwrap();
        for title in ["hw 5 ", "HW 6", "Quiz"] {
            create_task(&mut conn, Some(session_id), title, "", None, "medium", &["school".into()])
                .unwrap();
        }
        conn.execute("INSERT INTO label (name, color) VALUES ('Home', '#123456')", []).unwrap();
        let got = label_candidates(&conn, 2).unwrap();
        assert_eq!(
            got,
            [
                Candidate { name: "Home".into(), examples: vec![] },
                // "HW 5" twice in two cases is one wording; the newest three
                // cut to two.
                Candidate { name: "School".into(), examples: vec!["Quiz".into(), "HW 6".into()] },
            ]
        );
        assert_eq!(label_candidates(&conn, 6).unwrap()[1].examples, ["Quiz", "HW 6", "hw 5"]);
    }

    #[test]
    fn file_under_only_labels_a_task_that_wears_none() {
        let mut conn = store();
        let (session_id, first) = session_with_task(&conn);
        conn.execute("INSERT INTO label (name, color) VALUES ('School', '#123456')", []).unwrap();
        assert_eq!(unlabelled_task(&conn, first).unwrap(), Some(("write the thing".into(), "".into())));
        assert!(file_under(&conn, first, "school").unwrap());
        assert_eq!(label_names(&conn, first), ["School"]);
        assert_eq!(unlabelled_task(&conn, first).unwrap(), None);
        // Labelled by hand meanwhile, or the label is gone: nothing happens.
        let other = create_task(&mut conn, Some(session_id), "b", "", None, "medium", &["mine".into()])
            .unwrap();
        assert!(!file_under(&conn, other, "School").unwrap());
        assert_eq!(label_names(&conn, other), ["mine"]);
        let bare = create_task(&mut conn, Some(session_id), "c", "", None, "medium", &[]).unwrap();
        assert!(!file_under(&conn, bare, "Nowhere").unwrap());
        assert!(!file_under(&conn, 9999, "School").unwrap());
        assert_eq!(list_labels(&conn).unwrap().len(), 2);
    }

    #[test]
    fn create_task_writes_nothing_when_refused() {
        let mut conn = store();
        let (session_id, _) = session_with_task(&conn);
        let count = |conn: &Connection, table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
        };
        assert!(create_task(&mut conn, Some(session_id), "t", "", Some("2026-9-1"), "medium", &[]).is_err());
        conn.execute("UPDATE session SET ended_at = ?1 WHERE id = ?2", params![now(), session_id])
            .unwrap();
        assert!(matches!(
            create_task(&mut conn, Some(session_id), "t", "n", None, "medium", &["x".into()]),
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
            "medium",
            &["  billing  ".into(), "".into(), "Billing".into()],
        )
        .unwrap();
        assert_eq!(label_names(&conn, task_id), ["billing"]);
    }

    /// The old rename_task reported success for an id that was never there.
    #[test]
    fn update_task_refuses_an_unknown_id() {
        let mut conn = store();
        assert!(update_task(&mut conn, 404, "a", "", None, "medium", &[]).is_err());
    }

    /// A due date is a day. Set, read back, then cleared by sending None --
    /// the editor's Clear is the same end-state write as any other save.
    #[test]
    fn update_task_sets_and_clears_the_due_date() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        update_task(&mut conn, task_id, "a", "", Some("2026-09-18"), "medium", &[]).unwrap();
        let task = session_tasks(&conn, session_id).unwrap().pop().unwrap();
        assert_eq!(task.due_date.as_deref(), Some("2026-09-18"));

        update_task(&mut conn, task_id, "a", "", None, "medium", &[]).unwrap();
        let task = session_tasks(&conn, session_id).unwrap().pop().unwrap();
        assert_eq!(task.due_date, None);
    }

    /// Only the canonical form is stored, because string order has to be date
    /// order. "2026-9-1" is the one chrono would happily parse.
    #[test]
    fn update_task_sets_the_priority_and_create_task_takes_one() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        assert_eq!(session_tasks(&conn, session_id).unwrap()[0].priority, "medium");
        update_task(&mut conn, task_id, "a", "", None, "high", &[]).unwrap();
        assert_eq!(session_tasks(&conn, session_id).unwrap()[0].priority, "high");
        let id = create_task(&mut conn, None, "later", "", None, "low", &[]).unwrap();
        let backlog = backlog(&conn).unwrap();
        assert_eq!(backlog.iter().find(|t| t.id == id).unwrap().priority, "low");
    }

    #[test]
    fn a_priority_outside_the_three_levels_is_refused_before_any_write() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        for bad in ["urgent", "HIGH", ""] {
            assert!(update_task(&mut conn, task_id, "changed", "", None, bad, &[]).is_err(), "{bad:?}");
            assert!(create_task(&mut conn, Some(session_id), "new", "", None, bad, &[]).is_err(), "{bad:?}");
        }
        let tasks = session_tasks(&conn, session_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_ne!(tasks[0].title, "changed");
    }

    #[test]
    fn set_label_color_recolours_every_card_wearing_it() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        update_task(&mut conn, task_id, "a", "", None, "medium", &["billing".into()]).unwrap();
        set_label_color(&conn, "BILLING", "#12AB9f").unwrap();
        let task = &session_tasks(&conn, session_id).unwrap()[0];
        assert_eq!(task.labels[0].color, "#12ab9f");
        assert_eq!(list_labels(&conn).unwrap()[0].color, "#12ab9f");
    }

    #[test]
    fn set_label_color_refuses_a_bad_colour_or_an_unknown_label() {
        let mut conn = store();
        let (_, task_id) = session_with_task(&conn);
        update_task(&mut conn, task_id, "a", "", None, "medium", &["billing".into()]).unwrap();
        let before = list_labels(&conn).unwrap()[0].color.clone();
        for bad in ["12ab9f", "#12ab9", "#12ab9fz", "#gggggg", "red", ""] {
            assert!(set_label_color(&conn, "billing", bad).is_err(), "{bad:?}");
        }
        assert_eq!(list_labels(&conn).unwrap()[0].color, before);
        assert!(set_label_color(&conn, "nobody", "#000000").is_err());
    }

    #[test]
    fn update_task_refuses_a_malformed_due_date() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        for bad in ["tomorrow", "2026-02-30", "2026-9-1", "2026-09-18T10:00", ""] {
            assert!(
                update_task(&mut conn, task_id, "a", "", Some(bad), "medium", &[]).is_err(),
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

    /// Dragging a card to the Backlog column: it lands last, unstarted, with
    /// its details intact.
    #[test]
    fn unpull_sends_a_card_back_to_the_end_of_the_backlog() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        let first = add_task(&conn, None, "already waiting").unwrap();
        update_task(&mut conn, task_id, "write the thing", "a note", Some("2026-10-01"), "medium", &["school".into()]).unwrap();
        let doing = Arrangement { todo: vec![], doing: vec![task_id], done: vec![], dropped: vec![] };
        apply_board(&mut conn, session_id, &doing).unwrap();

        unpull_task(&mut conn, session_id, task_id).unwrap();

        assert!(session_tasks(&conn, session_id).unwrap().is_empty());
        let back = backlog(&conn).unwrap();
        assert_eq!(back.iter().map(|t| t.id).collect::<Vec<_>>(), vec![first, task_id]);
        let t = &back[1];
        assert_eq!(t.status, "planned");
        assert_eq!(t.notes, "a note");
        assert_eq!(t.due_date.as_deref(), Some("2026-10-01"));
        assert_eq!(t.labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["school"]);
        let started: Option<String> = conn
            .query_row("SELECT started_at FROM task WHERE id = ?1", [task_id], |r| r.get(0))
            .unwrap();
        assert!(started.is_none());
    }

    /// Only this board's cards, and never a dropped one or a backlog row.
    #[test]
    fn unpull_refuses_what_is_not_on_this_board() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        let backlog_id = add_task(&conn, None, "a backlog item").unwrap();
        assert!(unpull_task(&mut conn, session_id, backlog_id).is_err());
        assert!(unpull_task(&mut conn, session_id + 1, task_id).is_err());
        let dropped = Arrangement { todo: vec![], doing: vec![], done: vec![], dropped: vec![task_id] };
        apply_board(&mut conn, session_id, &dropped).unwrap();
        assert!(unpull_task(&mut conn, session_id, task_id).is_err());
        assert_eq!(session_tasks(&conn, session_id).unwrap().len(), 1);
    }

    fn backlog_ids(conn: &Connection) -> Vec<i64> {
        backlog(conn).unwrap().iter().map(|t| t.id).collect()
    }

    /// A drag inside the Backlog column sticks, and a row the board didn't
    /// know about stays after the ones it named, in its own order.
    #[test]
    fn order_backlog_writes_the_board_order() {
        let mut conn = store();
        let a = add_task(&conn, None, "a").unwrap();
        let b = add_task(&conn, None, "b").unwrap();
        let c = add_task(&conn, None, "c").unwrap();

        order_backlog(&mut conn, &[c, a, b]).unwrap();
        assert_eq!(backlog_ids(&conn), vec![c, a, b]);

        // `a` sits at position 2, inside the 1..=2 this list rewrites: left
        // alone it would tie with `c` rather than follow it.
        order_backlog(&mut conn, &[b, c]).unwrap();
        assert_eq!(backlog_ids(&conn), vec![b, c, a]);
    }

    /// A row that left the backlog meanwhile refuses the whole write, and
    /// nothing is half-applied.
    #[test]
    fn order_backlog_refuses_what_is_not_in_the_backlog() {
        let mut conn = store();
        let (_, on_board) = session_with_task(&conn);
        let a = add_task(&conn, None, "a").unwrap();
        let b = add_task(&conn, None, "b").unwrap();

        assert!(order_backlog(&mut conn, &[b, on_board, a]).is_err());
        assert_eq!(backlog_ids(&conn), vec![a, b]);
        let pos: i64 = conn
            .query_row("SELECT position FROM task WHERE id = ?1", [on_board], |r| r.get(0))
            .unwrap();
        assert_eq!(pos, 1);
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
        assert!(matches!(unpull_task(&mut conn, session_id, task_id), Err(AppError::SessionClosed(_))));

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
        update_task(&mut conn, task_id, "a backlog item", "", None, "medium", &["billing".into()]).unwrap();
        assert_eq!(backlog(&conn).unwrap()[0].labels.len(), 1);

        delete_task(&conn, task_id).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM task_label", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let labels = list_labels(&conn).unwrap();
        assert_eq!((labels[0].name.as_str(), labels[0].uses), ("billing", 0));
    }

    /// A card on an open session can be deleted outright; once the session
    /// is closed it is history and refuses. A backlog copy carried from the
    /// deleted card survives, its `carried_from` cleared.
    #[test]
    fn a_session_card_can_be_deleted_only_while_its_session_is_open() {
        let mut conn = store();
        let (session_id, task_id) = session_with_task(&conn);
        update_task(&mut conn, task_id, "t", "", None, "medium", &["billing".into()]).unwrap();
        conn.execute(
            "INSERT INTO task (session_id, title, position, carried_from, created_at)
             VALUES (NULL, 'copy', 1, ?1, 'x')",
            [task_id],
        )
        .unwrap();

        delete_task(&conn, task_id).unwrap();
        assert!(session_tasks(&conn, session_id).unwrap().is_empty());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM task_label", [], |r| r.get::<_, i64>(0)).unwrap(),
            0
        );
        assert_eq!(backlog(&conn).unwrap().len(), 1);
        assert_eq!(
            conn.query_row("SELECT carried_from FROM task WHERE title = 'copy'", [], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .unwrap(),
            None
        );

        let (closed_session, closed_task) = session_with_task(&conn);
        conn.execute("UPDATE session SET close_reason = 'clean' WHERE id = ?1", [closed_session])
            .unwrap();
        assert!(delete_task(&conn, closed_task).is_err());
        assert_eq!(session_tasks(&conn, closed_session).unwrap().len(), 1);
    }
}
