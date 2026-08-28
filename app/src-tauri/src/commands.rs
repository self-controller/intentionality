//! The invoke surface. Thin: every command validates, delegates to db/aw,
//! and returns serializable models. Nothing here holds a lock across await.

use crate::error::Result;
use crate::models::{Analysis, Arrangement, Board, Health, Observed, Session, Task};
use crate::state::AppState;
use crate::{analysis, aw, db, observed};
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub async fn health(state: State<'_, AppState>) -> Result<Health> {
    let (schema_version, session) = {
        let conn = state.conn.lock().unwrap();
        let version = db::schema_version(&conn)?;
        let session = match state.session_id() {
            Some(id) => db::get_session(&conn, id)?,
            None => None,
        };
        (version, session)
    };
    let aw_ok = aw::find_buckets().await.map(|(w, _)| w.is_some()).unwrap_or(false);
    Ok(Health {
        schema_version,
        needs_migration: schema_version < db::SCHEMA_VERSION,
        session,
        aw_ok,
    })
}

#[tauri::command]
pub fn get_board(state: State<'_, AppState>) -> Result<Board> {
    let conn = state.conn.lock().unwrap();
    db::check_schema(&conn)?;
    let (session, tasks, unseen) = match state.session_id() {
        Some(id) => (
            db::get_session(&conn, id)?,
            db::session_tasks(&conn, id)?,
            db::unseen_analyses(&conn, id)?,
        ),
        None => (None, Vec::new(), 0),
    };
    Ok(Board { session, tasks, backlog: db::backlog(&conn)?, unseen })
}

#[tauri::command]
pub fn apply_board(state: State<'_, AppState>, arrangement: Arrangement) -> Result<()> {
    let session_id = state
        .session_id()
        .ok_or_else(|| crate::error::AppError::Other("no open session".into()))?;
    let mut conn = state.conn.lock().unwrap();
    db::apply_board(&mut conn, session_id, &arrangement)
}

#[tauri::command]
pub fn add_task(state: State<'_, AppState>, title: String, to_backlog: bool) -> Result<i64> {
    let title = title.trim();
    if title.is_empty() {
        return Err(crate::error::AppError::Other("empty title".into()));
    }
    let session_id = if to_backlog { None } else { state.session_id() };
    if session_id.is_none() && !to_backlog {
        return Err(crate::error::AppError::Other("no open session".into()));
    }
    let conn = state.conn.lock().unwrap();
    db::add_task(&conn, session_id, title)
}

#[tauri::command]
pub fn rename_task(state: State<'_, AppState>, id: i64, title: String) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(crate::error::AppError::Other("empty title".into()));
    }
    let conn = state.conn.lock().unwrap();
    db::rename_task(&conn, id, title)
}

#[tauri::command]
pub fn delete_task(state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::delete_backlog_task(&conn, id)
}

#[tauri::command]
pub fn pull_task(state: State<'_, AppState>, id: i64) -> Result<()> {
    let session_id = state
        .session_id()
        .ok_or_else(|| crate::error::AppError::Other("no open session".into()))?;
    let conn = state.conn.lock().unwrap();
    db::pull_task(&conn, session_id, id)
}

#[tauri::command]
pub fn list_sessions(state: State<'_, AppState>, limit: i64) -> Result<Vec<Session>> {
    let conn = state.conn.lock().unwrap();
    db::list_sessions(&conn, limit.clamp(1, 100))
}

#[tauri::command]
pub fn get_session_tasks(state: State<'_, AppState>, session_id: i64) -> Result<Vec<Task>> {
    let conn = state.conn.lock().unwrap();
    db::session_tasks(&conn, session_id)
}

#[tauri::command]
pub async fn get_observed(state: State<'_, AppState>, session_id: i64) -> Result<Observed> {
    let (start, end) = {
        let conn = state.conn.lock().unwrap();
        let session = db::get_session(&conn, session_id)?
            .ok_or_else(|| crate::error::AppError::Other("no such session".into()))?;
        (session.started_at, session.ended_at.unwrap_or_else(db::now))
    };
    observed::observed(&start, &end).await
}

#[tauri::command]
pub fn list_analyses(state: State<'_, AppState>, session_id: i64) -> Result<Vec<Analysis>> {
    let conn = state.conn.lock().unwrap();
    db::list_analyses(&conn, session_id)
}

#[tauri::command]
pub fn mark_analysis_seen(state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::mark_analysis_seen(&conn, id)
}

/// The manual trigger — the timer without the wait. Invaluable for testing.
#[tauri::command]
pub async fn run_analysis_now(app: AppHandle) -> Result<Option<i64>> {
    let id = analysis::run(&app).await?;
    if let Some(id) = id {
        let _ = app.emit("analysis:new", id);
    }
    Ok(id)
}
