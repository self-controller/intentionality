//! The invoke surface. Thin: every command validates, delegates to db/aw,
//! and returns serializable models. Nothing here holds a lock across await.

use crate::error::Result;
use crate::models::{
    Analysis, Arrangement, AudioSource, Board, Health, Meeting, MeetingDetail, Observed, Session,
    Task,
};
use crate::state::AppState;
use crate::{analysis, audio, aw, build_info, db, meeting, observed};
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
        // A directory walk of ~25 files, next to the network probe above.
        build: build_info::current(),
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

/// The Analyses tab reads history across sessions, so it does not take a
/// session id — it works with no session open at all.
#[tauri::command]
pub fn list_recent_analyses(state: State<'_, AppState>, limit: i64) -> Result<Vec<Analysis>> {
    let conn = state.conn.lock().unwrap();
    db::list_recent_analyses(&conn, limit.clamp(1, 200))
}

/// Fetched per selected analysis rather than shipped with the whole list —
/// the blob is far larger than the row it belongs to.
#[tauri::command]
pub fn get_analysis_observed(state: State<'_, AppState>, id: i64) -> Result<Observed> {
    let conn = state.conn.lock().unwrap();
    db::analysis_observed(&conn, id)
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

/// The checkpoint waiting to be acknowledged, if any. Called on app start:
/// a checkpoint that fired while the app was restarting must still be shown.
#[tauri::command]
pub fn pending_checkpoint(state: State<'_, AppState>) -> Result<Option<Analysis>> {
    let Some(session_id) = state.session_id() else { return Ok(None) };
    let conn = state.conn.lock().unwrap();
    db::pending_checkpoint(&conn, session_id)
}

/// "+15 min" / "+30 min" on the checkpoint screen. Re-arms the checkpoint; it
/// never closes or extends the session itself — closing stays the gate's job.
#[tauri::command]
pub fn extend_checkpoint(state: State<'_, AppState>, minutes: i64) -> Result<String> {
    let session_id = state
        .session_id()
        .ok_or_else(|| crate::error::AppError::Other("no open session".into()))?;
    let conn = state.conn.lock().unwrap();
    db::extend_checkpoint(&conn, session_id, minutes.clamp(1, 24 * 60))
}

/// The manual trigger for the checkpoint — the timer without the wait. Without
/// it this can only be exercised by sitting out a real session.
#[tauri::command]
pub async fn run_checkpoint_now(app: AppHandle) -> Result<Option<i64>> {
    // Emits and notifies itself, so unlike run_analysis_now there is nothing
    // to do here afterwards.
    analysis::run_checkpoint(&app).await
}

// --- meetings ---------------------------------------------------------------

/// The PipeWire capture sources that can be recorded from, for the selector
/// on the Meetings screen. Empty is a valid answer and not an error: a machine
/// with no PipeWire, or none it will admit to, must still be able to press
/// Start and get the system default.
///
/// Async because it must be, exactly like `start_meeting` below: a sync
/// command runs on the GTK main thread, where spawning a `tokio::process`
/// panics with "there is no reactor running" inside a non-unwinding frame and
/// aborts the whole process rather than returning an error.
#[tauri::command]
pub async fn list_audio_sources() -> Result<Vec<AudioSource>> {
    Ok(audio::sources().await)
}

/// Open the microphone, optionally on a named PipeWire source. Deliberately
/// the only way recording ever begins: no timer, no scheduler and no startup
/// path calls this.
///
/// Async because it must be: a sync command runs on the main thread, and
/// opening the microphone there panics with no reactor running and aborts the
/// process mid-click. See record::start.
#[tauri::command]
pub async fn start_meeting(app: AppHandle, target: Option<String>) -> Result<i64> {
    meeting::start(&app, target).await
}

/// Close the microphone. Returns as soon as it is shut, with the meeting moved
/// to 'summarizing'; the tail chunk and the notes are finished in the
/// background and land as a `meeting:done` event.
#[tauri::command]
pub async fn stop_meeting(app: AppHandle) -> Result<i64> {
    meeting::stop(&app).await
}

/// Whether a meeting is recording right now, for a UI that has just mounted
/// or switched back to the tab. Recording lives in Rust, so the answer
/// survives the React component being unmounted and remounted.
#[tauri::command]
pub fn recording_meeting(state: State<'_, AppState>) -> Result<Option<i64>> {
    Ok(state.meeting_id())
}

#[tauri::command]
pub fn list_meetings(state: State<'_, AppState>, limit: i64) -> Result<Vec<Meeting>> {
    let conn = state.conn.lock().unwrap();
    db::list_meetings(&conn, limit.clamp(1, 200))
}

#[tauri::command]
pub fn get_meeting(state: State<'_, AppState>, meeting_id: i64) -> Result<MeetingDetail> {
    let conn = state.conn.lock().unwrap();
    let meeting = db::get_meeting(&conn, meeting_id)?
        .ok_or_else(|| crate::error::AppError::Other("no such meeting".into()))?;
    Ok(MeetingDetail {
        segments: db::meeting_segments(&conn, meeting_id)?,
        actions: db::meeting_actions(&conn, meeting_id)?,
        meeting,
    })
}

/// Turn ticked action items into backlog tasks. Returns how many were created,
/// which can be fewer than asked for: an item already approved is skipped
/// rather than inserted twice.
#[tauri::command]
pub fn approve_actions(
    state: State<'_, AppState>,
    meeting_id: i64,
    action_ids: Vec<i64>,
) -> Result<usize> {
    let mut conn = state.conn.lock().unwrap();
    db::approve_actions(&mut conn, meeting_id, &action_ids)
}

/// Ask the model again for a meeting whose notes failed. Costs one call and no
/// audio — the transcript was stored as it was recorded.
#[tauri::command]
pub async fn resummarize_meeting(app: AppHandle, meeting_id: i64) -> Result<()> {
    meeting::resummarize(&app, meeting_id).await
}

#[tauri::command]
pub fn delete_meeting(state: State<'_, AppState>, meeting_id: i64) -> Result<()> {
    if state.meeting_id() == Some(meeting_id) {
        return Err(crate::error::AppError::Other(
            "stop the recording before deleting it".into(),
        ));
    }
    let conn = state.conn.lock().unwrap();
    db::delete_meeting(&conn, meeting_id)
}
