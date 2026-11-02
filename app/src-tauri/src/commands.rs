//! The invoke surface. Thin: every command validates, delegates to db/aw,
//! and returns serializable models. Nothing here holds a lock across await.

use crate::error::{AppError, Result};
use crate::models::{
    Analysis, AnalysisDetail, Arrangement, Board, DayTime, Health, Judgement, LabelSummary,
    RiskApp, RiskToday, RiskTotal,
};
use crate::relevance::{self, Page};
use crate::state::AppState;
use crate::{
    analysis, autolabel, aw, build_info, day, db, desktop, ignore, jev, keybinds, overlay, resume_units, scheduler,
    screen_time, today,
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
        // Two session-bus round trips to gnome-shell, each well under 1 ms.
        overlay: overlay::warning().await,
        window_tracking: overlay::window_tracking_warning().await,
        relevance: jev::warning(),
    })
}

/// Today's time on every flagged app. AW being down still lists the flags
/// (at zero, with the reason), so the menu can always remove one.
#[tauri::command]
pub async fn risk_today(state: State<'_, AppState>) -> Result<RiskToday> {
    let (apps, judged) = {
        let conn = state.conn.lock().unwrap();
        (db::list_risk_apps(&conn)?, db::relevance_count(&conn)?)
    };
    if apps.is_empty() {
        return Ok(RiskToday { apps: Vec::new(), total: 0.0, judged, error: None });
    }
    match screen_time::today(&apps).await {
        Ok(today) => Ok(RiskToday { judged, ..today }),
        Err(err) => Ok(RiskToday {
            apps: apps
                .into_iter()
                .map(|a| RiskTotal { id: a.id, name: a.name, seconds: 0.0 })
                .collect(),
            total: 0.0,
            judged,
            error: Some(err.to_string()),
        }),
    }
}

/// The page as the cache knows it, for the appeal panel's first render:
/// no request is made here. `window_app` is the window class the shell
/// reported, `title` its title as shown.
#[tauri::command]
pub fn page_judgement(
    state: State<'_, AppState>,
    window_app: String,
    title: String,
) -> Result<Judgement> {
    let conn = state.conn.lock().unwrap();
    relevance::cached(&conn, state.session_id(), &Page { app: window_app, title })
}

/// The user's case that a page serves one of the tasks, put to Jev with the
/// whole argument so far. The verdict is stored, and the timer loop is poked
/// so a page just excused leaves the screen without waiting for its next
/// look. Never an Err for a model problem: the judgement carries it.
#[tauri::command]
pub async fn appeal(
    app: AppHandle,
    window_app: String,
    title: String,
    argument: Vec<String>,
) -> Result<Judgement> {
    let argument: Vec<String> =
        argument.into_iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
    if argument.is_empty() {
        return Err(AppError::Other("say why the page serves a task".into()));
    }
    let judgement =
        relevance::appeal(app.clone(), Page { app: window_app, title }, argument).await;
    app.state::<AppState>().relevance_changed.notify_one();
    Ok(judgement)
}

/// Drop every cached verdict. Nothing is lost but requests: the next visit
/// to each page asks Jev again.
#[tauri::command]
pub fn forget_relevance(app: AppHandle, state: State<'_, AppState>) -> Result<usize> {
    let forgotten = {
        let conn = state.conn.lock().unwrap();
        db::relevance_forget(&conn)?
    };
    app.state::<AppState>().relevance_changed.notify_one();
    Ok(forgotten)
}

/// The ignore list (ignore.rs): apps and sites never judged.
#[tauri::command]
pub fn ignore_list(state: State<'_, AppState>) -> Result<Vec<ignore::Rule>> {
    let conn = state.conn.lock().unwrap();
    Ok(ignore::load(&conn))
}

/// Never judge this app or site again. The timer loop is poked so a clock
/// on the window in front goes at once.
#[tauri::command]
pub fn add_ignore(app: AppHandle, state: State<'_, AppState>, rule: ignore::Rule) -> Result<Vec<ignore::Rule>> {
    let rules = {
        let conn = state.conn.lock().unwrap();
        ignore::add(&conn, rule)?
    };
    app.state::<AppState>().relevance_changed.notify_one();
    Ok(rules)
}

/// Judge this app or site again.
#[tauri::command]
pub fn remove_ignore(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: ignore::Kind,
    value: String,
) -> Result<Vec<ignore::Rule>> {
    let rules = {
        let conn = state.conn.lock().unwrap();
        ignore::remove(&conn, kind, &value)?
    };
    app.state::<AppState>().relevance_changed.notify_one();
    Ok(rules)
}

/// Every installed app, A to Z, scanned afresh: the ignore menu's list.
#[tauri::command]
pub async fn installed_apps() -> Result<Vec<desktop::App>> {
    tauri::async_runtime::spawn_blocking(|| desktop::machine().all(true).as_ref().clone())
        .await
        .map_err(|e| AppError::Other(e.to_string()))
}

/// A day for the Dashboard ("YYYY-MM-DD", none = today), from the relevance
/// cache alone: asks Jev nothing, so it can be polled.
#[tauri::command]
pub async fn day_time(app: AppHandle, day: Option<String>) -> Result<DayTime> {
    let date = day.as_deref().map(day::parse_date).transpose()?;
    today::day_time(&app, date, false).await
}

/// The Dashboard's button: Jev asked about the busiest pages of the day
/// still without a verdict (a check's worth), then the day again. The timer
/// loop is poked so a page just excused leaves the screen at once.
#[tauri::command]
pub async fn judge_day(app: AppHandle, day: Option<String>) -> Result<DayTime> {
    let date = day.as_deref().map(day::parse_date).transpose()?;
    let read = today::day_time(&app, date, true).await;
    app.state::<AppState>().relevance_changed.notify_one();
    read
}

/// The hour a day starts at (day.rs), for Settings.
#[tauri::command]
pub fn day_start_hour() -> u32 {
    day::hour()
}

/// Settings: days start at `hour` (0-23) from now on, for the Dashboard and
/// the clock's daily total.
#[tauri::command]
pub fn set_day_start_hour(state: State<'_, AppState>, hour: u32) -> Result<u32> {
    let conn = state.conn.lock().unwrap();
    day::set(&conn, hour)?;
    Ok(day::hour())
}

/// Settings' keyboard shortcuts: the overrides only (keybinds.rs).
#[tauri::command]
pub fn keybinds(state: State<'_, AppState>) -> keybinds::Keybinds {
    let conn = state.conn.lock().unwrap();
    keybinds::load(&conn)
}

/// Settings: replace the overrides whole. The gate reads them at its next run.
#[tauri::command]
pub fn set_keybinds(state: State<'_, AppState>, keybinds: keybinds::Keybinds) -> Result<keybinds::Keybinds> {
    let conn = state.conn.lock().unwrap();
    keybinds::set(&conn, &keybinds)?;
    Ok(keybinds::load(&conn))
}

#[tauri::command]
pub fn add_risk_app(state: State<'_, AppState>, name: String) -> Result<RiskApp> {
    if screen_time::keyword(&name).is_empty() {
        return Err(AppError::Other("name an app or a website".into()));
    }
    let conn = state.conn.lock().unwrap();
    db::add_risk_app(&conn, &name)
}

#[tauri::command]
pub fn remove_risk_app(state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::remove_risk_app(&conn, id)
}

#[tauri::command]
pub fn get_board(app: AppHandle, state: State<'_, AppState>) -> Result<Board> {
    // Asked for inside the window where the gate has closed the held session
    // and no heartbeat has noticed yet, answer with the session actually open.
    scheduler::sync_session(&app);
    let conn = state.conn.lock().unwrap();
    db::check_schema(&conn)?;
    let (session, tasks) = match state.session_id() {
        Some(id) => (db::get_session(&conn, id)?, db::session_tasks(&conn, id)?),
        None => (None, Vec::new()),
    };
    Ok(Board { session, tasks, backlog: db::backlog(&conn)? })
}

#[tauri::command]
pub fn apply_board(state: State<'_, AppState>, arrangement: Arrangement) -> Result<()> {
    let session_id = state
        .session_id()
        .ok_or_else(|| crate::error::AppError::Other("no open session".into()))?;
    let mut conn = state.conn.lock().unwrap();
    db::apply_board(&mut conn, session_id, &arrangement)
}

/// The backlog's order after a drag. No session needed: the backlog belongs
/// to none.
#[tauri::command]
pub fn order_backlog(state: State<'_, AppState>, ids: Vec<i64>) -> Result<()> {
    let mut conn = state.conn.lock().unwrap();
    db::order_backlog(&mut conn, &ids)
}

#[tauri::command]
pub fn add_task(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
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
    let id = {
        let conn = state.conn.lock().unwrap();
        db::add_task(&conn, session_id, title)?
    };
    tauri::async_runtime::spawn(autolabel::sort(app, id));
    Ok(id)
}

/// A new card from the editor, details and all, in the session or the
/// backlog. Same arguments as update_task, same meaning. A card saved with
/// no labels is then shown to Jev, which may file it under one
/// (autolabel.rs); the board hears `task:labelled` when it does.
#[tauri::command]
pub fn create_task(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
    notes: String,
    due_date: Option<String>,
    priority: String,
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
    let id = {
        let mut conn = state.conn.lock().unwrap();
        db::create_task(&mut conn, session_id, title, notes.trim(), due_date, &priority, &labels)?
    };
    if labels.iter().all(|l| l.trim().is_empty()) {
        tauri::async_runtime::spawn(autolabel::sort(app, id));
    }
    Ok(id)
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
    priority: String,
    labels: Vec<String>,
) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(crate::error::AppError::Other("empty title".into()));
    }
    let due_date = blank_is_none(due_date.as_deref());
    let mut conn = state.conn.lock().unwrap();
    db::update_task(&mut conn, id, title, notes.trim(), due_date, &priority, &labels)
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

/// A label's colour is the label's, not a card's: every card wearing it
/// changes at once, so this is its own write and not part of an editor save.
#[tauri::command]
pub fn set_label_color(state: State<'_, AppState>, name: String, color: String) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::set_label_color(&conn, &name, &color)
}

#[tauri::command]
pub fn delete_label(state: State<'_, AppState>, name: String) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::delete_label(&conn, &name)
}

#[tauri::command]
pub fn delete_task(state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::delete_task(&conn, id)
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
pub fn unpull_task(state: State<'_, AppState>, id: i64) -> Result<()> {
    let session_id = state
        .session_id()
        .ok_or_else(|| crate::error::AppError::Other("no open session".into()))?;
    let mut conn = state.conn.lock().unwrap();
    db::unpull_task(&mut conn, session_id, id)
}

/// The Analyses tab reads history across sessions, so it does not take a
/// session id — it works with no session open at all.
#[tauri::command]
pub fn list_recent_analyses(state: State<'_, AppState>, limit: i64) -> Result<Vec<Analysis>> {
    let conn = state.conn.lock().unwrap();
    db::list_recent_analyses(&conn, limit.clamp(1, 200))
}

/// Fetched per selected analysis rather than shipped with the whole list —
/// the blobs are far larger than the row they belong to.
#[tauri::command]
pub fn get_analysis_detail(state: State<'_, AppState>, id: i64) -> Result<AnalysisDetail> {
    let conn = state.conn.lock().unwrap();
    db::analysis_detail(&conn, id)
}

#[tauri::command]
pub fn mark_analysis_seen(state: State<'_, AppState>, id: i64) -> Result<()> {
    let conn = state.conn.lock().unwrap();
    db::mark_analysis_seen(&conn, id)
}

/// The manual trigger — the timer without the wait. Invaluable for testing.
#[tauri::command]
pub async fn run_analysis_now(app: AppHandle) -> Result<Option<i64>> {
    let id = analysis::run(&app, analysis::Trigger::Manual).await?;
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
