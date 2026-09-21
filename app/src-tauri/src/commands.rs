//! The invoke surface. Thin: every command validates, delegates to db/aw,
//! and returns serializable models. Nothing here holds a lock across await.

use crate::error::Result;
use crate::models::{
    Analysis, Arrangement, AudioSource, Board, Health, LabelSummary, Meeting, MeetingDetail,
    Observed, RecordingNow, Session, Task,
};
use crate::state::AppState;
use crate::{
    analysis, attach, audio, aw, build_info, db, meeting, observed, resume_units, scheduler,
};
use tauri::{AppHandle, Emitter, Manager, State};

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
        resume_gate: resume_units::warning(),
    })
}

#[tauri::command]
pub fn get_board(app: AppHandle, state: State<'_, AppState>) -> Result<Board> {
    // Asked for inside the window where the gate has closed the held session
    // and no heartbeat has noticed yet, answer with the session actually open.
    scheduler::sync_session(&app);
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

/// A new card from the editor, details and all, in the session or the
/// backlog. Same arguments as update_task, same meaning.
#[tauri::command]
pub fn create_task(
    state: State<'_, AppState>,
    title: String,
    notes: String,
    due_date: Option<String>,
    labels: Vec<String>,
    to_backlog: bool,
) -> Result<i64> {
    let title = title.trim();
    if title.is_empty() {
        return Err(crate::error::AppError::Other("empty title".into()));
    }
    let session_id = if to_backlog { None } else { state.session_id() };
    if session_id.is_none() && !to_backlog {
        return Err(crate::error::AppError::Other("no open session".into()));
    }
    let due_date = blank_is_none(due_date.as_deref());
    let mut conn = state.conn.lock().unwrap();
    db::create_task(&mut conn, session_id, title, notes.trim(), due_date, &labels)
}

/// The task editor saves everything it holds at once — `labels` is the full
/// set the card should wear afterwards, not an addition, and a missing
/// `due_date` clears the card's date.
#[tauri::command]
pub fn update_task(
    state: State<'_, AppState>,
    id: i64,
    title: String,
    notes: String,
    due_date: Option<String>,
    labels: Vec<String>,
) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(crate::error::AppError::Other("empty title".into()));
    }
    let due_date = blank_is_none(due_date.as_deref());
    let mut conn = state.conn.lock().unwrap();
    db::update_task(&mut conn, id, title, notes.trim(), due_date, &labels)
}

/// An empty field means "no due date", however the webview spells it.
fn blank_is_none(due_date: Option<&str>) -> Option<&str> {
    due_date.map(str::trim).filter(|d| !d.is_empty())
}

#[tauri::command]
pub fn list_labels(state: State<'_, AppState>) -> Result<Vec<LabelSummary>> {
    let conn = state.conn.lock().unwrap();
    db::list_labels(&conn)
}

#[tauri::command]
pub fn create_label(state: State<'_, AppState>, name: String) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::create_label(&conn, &name)
}

#[tauri::command]
pub fn delete_label(state: State<'_, AppState>, name: String) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::delete_label(&conn, &name)
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
pub async fn start_meeting(app: AppHandle, target: Option<String>) -> Result<RecordingNow> {
    meeting::start(&app, target).await
}

/// Open the microphone again on a finished meeting; new chunks carry on after
/// its last segment, and the next Stop rewrites its notes from the whole
/// transcript. Async for the same reason as `start_meeting`.
#[tauri::command]
pub async fn resume_meeting(
    app: AppHandle,
    meeting_id: i64,
    target: Option<String>,
) -> Result<RecordingNow> {
    meeting::resume(&app, meeting_id, target).await
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
pub fn recording_meeting(state: State<'_, AppState>) -> Result<Option<RecordingNow>> {
    Ok(state.recording())
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
    let (summary, summary_edited_at) = db::meeting_summary(&conn, meeting_id)?;
    Ok(MeetingDetail {
        segments: db::meeting_segments(&conn, meeting_id)?,
        actions: db::meeting_actions(&conn, meeting_id)?,
        notes: db::meeting_notes_text(&conn, meeting_id)?,
        summary,
        summary_edited_at,
        clean_transcript: db::clean_transcript(&conn, meeting_id)?,
        files: db::meeting_files(&conn, meeting_id)?,
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

/// The scratchpad, saved. Any state: it is the user's own text, and a debounced
/// autosave landing just after Stop must not be rejected.
#[tauri::command]
pub fn set_meeting_notes(
    state: State<'_, AppState>,
    meeting_id: i64,
    notes: String,
) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::set_meeting_notes(&conn, meeting_id, &notes)
}

/// The user's edit to the write-up. Unlike the scratchpad this is refused
/// while the notes are being written -- see db::set_meeting_summary.
#[tauri::command]
pub fn set_meeting_summary(
    state: State<'_, AppState>,
    meeting_id: i64,
    summary: String,
) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::set_meeting_summary(&conn, meeting_id, &summary)
}

/// Open the native picker, then copy, validate and record whatever was chosen.
///
/// The picker runs here rather than in the webview, which is why
/// `capabilities/default.json` still grants nothing but `core:` permissions:
/// the same reasoning as tauri-plugin-notification. It also means a path never
/// crosses into the frontend in either direction — the webview asks for files
/// and gets metadata back.
///
/// Partial success is the normal outcome and is reported as such: picking four
/// files where one is a 30-page PDF should attach three and say why the fourth
/// did not, rather than failing all four.
#[tauri::command]
pub async fn attach_meeting_files(
    app: AppHandle,
    meeting_id: i64,
) -> Result<AttachOutcome> {
    use tauri_plugin_dialog::DialogExt;

    // The blocking picker must not run on the GTK main thread, and a sync
    // command does exactly that -- see the note on list_audio_sources above
    // for what that costs. The oneshot keeps this off it.
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Attach context for this meeting")
        .pick_files(move |paths| {
            let _ = tx.send(paths);
        });
    let picked = rx
        .await
        .map_err(|_| crate::error::AppError::Other("the file picker closed unexpectedly".into()))?
        .unwrap_or_default();

    let mut rejected = Vec::new();
    for path in picked {
        let Some(path) = path.into_path().ok() else {
            rejected.push("that file could not be read".to_string());
            continue;
        };
        let shown = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "that file".into());

        // Copying, sniffing and unzipping are all blocking work on files that
        // can be megabytes, so none of it belongs on an async worker.
        let staged = match tokio::task::spawn_blocking({
            let path = path.clone();
            move || attach::stage(meeting_id, &path)
        })
        .await
        {
            Ok(Ok(staged)) => staged,
            Ok(Err(e)) => {
                rejected.push(format!("{shown}: {e}"));
                continue;
            }
            // A panic in a parser reading someone else's file is contained
            // here rather than taking the app down with it.
            Err(_) => {
                rejected.push(format!("{shown}: could not be read"));
                continue;
            }
        };

        let state = app.state::<AppState>();
        let row = {
            let mut conn = state.conn.lock().unwrap();
            db::add_meeting_file(
                &mut conn,
                meeting_id,
                &db::NewFile {
                    name: &staged.name,
                    path: &staged.path().to_string_lossy(),
                    kind: staged.kind.as_str(),
                    bytes: staged.bytes as i64,
                    extracted: staged.extracted.as_deref(),
                },
                attach::MAX_FILES,
                attach::MAX_TOTAL_FILE_BYTES,
                attach::MAX_EXTRACTED_CHARS_TOTAL as i64,
            )
        };
        match row {
            Ok(id) => {
                // The row committed; now put the file where it says it is. If
                // that fails there is no file to point at, so the row goes.
                if let Err(e) = staged.promote() {
                    let conn = state.conn.lock().unwrap();
                    let _ = db::delete_meeting_file(&conn, meeting_id, id);
                    staged.discard();
                    rejected.push(format!("{shown}: {e}"));
                }
            }
            Err(e) => {
                staged.discard();
                rejected.push(format!("{shown}: {e}"));
            }
        }
    }

    // The whole chip list, not just what this call added: the frontend
    // replaces its list wholesale and a partial one would drop the rest.
    let state = app.state::<AppState>();
    let conn = state.conn.lock().unwrap();
    let files = db::meeting_files(&conn, meeting_id)?;
    Ok(AttachOutcome { files, rejected })
}

#[derive(serde::Serialize)]
pub struct AttachOutcome {
    pub files: Vec<crate::models::MeetingFile>,
    /// One readable line per file that was refused, so the user learns which
    /// of four files was the problem rather than that "something" was.
    pub rejected: Vec<String>,
}

#[tauri::command]
pub fn remove_meeting_file(
    state: State<'_, AppState>,
    meeting_id: i64,
    file_id: i64,
) -> Result<()> {
    let path = {
        let conn = state.conn.lock().unwrap();
        db::delete_meeting_file(&conn, meeting_id, file_id)?
    };
    attach::remove_file(&path);
    Ok(())
}

/// Repair and re-summarize a meeting that already finished one way or the
/// other. Costs two calls and no audio — the transcript was stored as it was
/// recorded, and re-reads whatever notes and files are attached now.
#[tauri::command]
pub async fn rerun_meeting_notes(app: AppHandle, meeting_id: i64) -> Result<()> {
    meeting::rerun_notes(&app, meeting_id).await
}

#[tauri::command]
pub fn delete_meeting(state: State<'_, AppState>, meeting_id: i64) -> Result<()> {
    if state.meeting_id() == Some(meeting_id) {
        return Err(crate::error::AppError::Other(
            "stop the recording before deleting it".into(),
        ));
    }
    // SQLite first, filesystem after. ON DELETE CASCADE takes the rows with
    // the meeting, and the whole directory goes after -- every copy this
    // meeting owns is inside it, which is why there is nothing to look up.
    // A cleanup that fails must not roll back a deletion the user already saw
    // succeed, so it is logged and left to the startup sweep.
    {
        let conn = state.conn.lock().unwrap();
        db::delete_meeting(&conn, meeting_id)?;
    }
    attach::remove_meeting_dir(meeting_id);
    Ok(())
}
