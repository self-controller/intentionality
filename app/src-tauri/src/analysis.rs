//! Assembles the context for one productivity check, calls the model, and
//! persists the result.
//!
//! Two entry points over the same context. `run` is the randomized background
//! check: it returns Ok(None) when the window was too quiet to judge fairly,
//! and has no side effects beyond the row it writes. `run_checkpoint` fires
//! when the intended time runs out, and never skips — a quiet window or an
//! unreachable API still produces a screen, because the point of the
//! checkpoint is the moment, not the analysis.

use crate::error::Result;
use crate::models::{Observed, Session, Task};
use crate::recommendations;
use crate::state::AppState;
use crate::{claude, db, notify, observed, scheduler};
use tauri::{AppHandle, Emitter, Manager};

/// Everything both calls need, gathered once.
struct Prepared {
    session: Session,
    board_lines: String,
    window_start: String,
    window_end: String,
    window: Observed,
    session_total: Option<Observed>,
    previous: Option<(String, Option<i64>)>,
    elapsed_minutes: i64,
    window_minutes: i64,
}

impl Prepared {
    fn context(&self) -> claude::Context<'_> {
        claude::Context {
            statement: &self.session.statement,
            intended_minutes: self.session.intended_minutes,
            elapsed_minutes: self.elapsed_minutes,
            board_lines: self.board_lines.clone(),
            window: &self.window,
            session_total: self.session_total.as_ref(),
            window_minutes: self.window_minutes,
            previous: self.previous.clone(),
        }
    }

    fn observed_json(&self) -> String {
        serde_json::to_string(&self.window).unwrap_or_else(|_| "{}".into())
    }
}

fn board_lines(tasks: &[Task]) -> String {
    let mut lanes = [
        ("todo", Vec::new()),
        ("doing", Vec::new()),
        ("done", Vec::new()),
        ("dropped", Vec::new()),
    ];
    for task in tasks {
        let lane = match task.status.as_str() {
            "planned" => 0,
            "doing" => 1,
            "done" => 2,
            _ => 3,
        };
        lanes[lane].1.push(task.title.as_str());
    }
    lanes
        .iter()
        .map(|(name, titles)| {
            format!("{name}: {}", if titles.is_empty() { "-".into() } else { titles.join(" | ") })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn prepare(app: &AppHandle, session_id: i64) -> Result<Option<Prepared>> {
    let state = app.state::<AppState>();

    // Everything read up front so no lock is held across awaits.
    let (session, tasks, window_start, previous) = {
        let conn = state.conn.lock().unwrap();
        let Some(session) = db::get_session(&conn, session_id)? else { return Ok(None) };
        let tasks = db::session_tasks(&conn, session_id)?;
        let latest = db::latest_analysis(&conn, session_id)?;
        let window_start = latest
            .as_ref()
            .map_or_else(|| session.started_at.clone(), |a| a.window_end.clone());
        let previous = latest.map(|a| (a.headline, a.alignment));
        (session, tasks, window_start, previous)
    };

    let window_end = db::now();
    // A window AW cannot answer for reads as an empty one. `run` then skips it
    // as too quiet, and the checkpoint falls back — neither has to special-case
    // ActivityWatch being down.
    //
    // The window is the tail of the session, so one fetch of the session's
    // events answers both. If that larger fetch fails (a long session can
    // outrun AW's timeout), the window alone is still worth asking for.
    let (window, session_total) = match observed::fetch(&session.started_at, &window_end).await {
        Ok(events) => (
            observed::summarize(&events, &window_start, &window_end).unwrap_or_default(),
            observed::summarize(&events, &session.started_at, &window_end).ok(),
        ),
        Err(_) => (
            observed::observed(&window_start, &window_end).await.unwrap_or_default(),
            None,
        ),
    };

    let elapsed_minutes = observed::parse_ts(&window_end)?
        .signed_duration_since(observed::parse_ts(&session.started_at)?)
        .num_minutes();
    let window_minutes = observed::parse_ts(&window_end)?
        .signed_duration_since(observed::parse_ts(&window_start)?)
        .num_minutes();

    Ok(Some(Prepared {
        board_lines: board_lines(&tasks),
        session,
        window_start,
        window_end,
        window,
        session_total,
        previous,
        elapsed_minutes,
        window_minutes,
    }))
}

pub async fn run(app: &AppHandle) -> Result<Option<i64>> {
    let state = app.state::<AppState>();
    let Some(session_id) = state.session_id() else { return Ok(None) };
    // One analysis at a time: the two timers plus the manual button could
    // otherwise overlap, double-billing the API and racing on
    // where the latest analysis ended (two runs would judge the same window twice).
    let _running = state.analysis_lock.lock().await;

    let Some(prep) = prepare(app, session_id).await? else { return Ok(None) };
    if prep.window.active_seconds < scheduler::MIN_ACTIVE_SECS {
        return Ok(None); // too quiet to judge fairly
    }

    let result = claude::analyze(&prep.context()).await?;
    let observed_json = prep.observed_json();

    let conn = state.conn.lock().unwrap();
    let id = db::add_analysis(
        &conn,
        &db::NewAnalysis {
            session_id,
            kind: "check",
            window_start: &prep.window_start,
            window_end: &prep.window_end,
            headline: &result.headline,
            alignment: result.alignment,
            body: &result.body,
            observed_json: &observed_json,
            recommendation_id: None,
            recommendation_note: None,
        },
    )?;
    Ok(Some(id))
}

/// The time-up checkpoint. Unlike `run` this is self-contained — it clears the
/// pending checkpoint, emits, and notifies — because both callers (the timer
/// and the manual trigger) want exactly the same thing to happen.
pub async fn run_checkpoint(app: &AppHandle) -> Result<Option<i64>> {
    let state = app.state::<AppState>();
    let Some(session_id) = state.session_id() else { return Ok(None) };
    let _running = state.analysis_lock.lock().await;

    let Some(prep) = prepare(app, session_id).await? else { return Ok(None) };

    // A quiet window is not a reason to stay silent here: the user asked to be
    // told when their time was up, and being told is the deliverable. The same
    // goes for the API being unreachable — the clock still ran out. Both
    // fallbacks carry (headline, body); neither headline opens with "time's
    // up", because the notification title and the screen already say that and
    // a history row headlined that way would record the clock, not the reason.
    let judged = if prep.window.active_seconds < scheduler::MIN_ACTIVE_SECS {
        Err((
            "Nothing tracked to judge",
            "Under five minutes of tracked activity in this stretch, so there is \
             nothing here to judge — this is the clock talking, not an assessment."
                .to_string(),
        ))
    } else {
        claude::checkpoint(&prep.context())
            .await
            .map_err(|e| ("Check could not run", format!("{e}. Your time is up regardless.")))
    };

    let headline = match &judged {
        Ok(result) => result.headline.clone(),
        Err((headline, _)) => headline.to_string(),
    };
    let (alignment, body, recommendation, note) = match &judged {
        Ok(result) => (
            result.alignment,
            result.body.clone(),
            result.recommendation,
            Some(result.note.clone()).filter(|n| !n.is_empty()),
        ),
        Err((_, why)) => (None, why.clone(), recommendations::QUIET_WINDOW, None),
    };

    let observed_json = prep.observed_json();
    let id = {
        let conn = state.conn.lock().unwrap();
        let id = db::add_analysis(
            &conn,
            &db::NewAnalysis {
                session_id,
                kind: "checkpoint",
                window_start: &prep.window_start,
                window_end: &prep.window_end,
                headline: &headline,
                alignment,
                body: &body,
                observed_json: &observed_json,
                recommendation_id: Some(recommendation.id),
                recommendation_note: note.as_deref(),
            },
        )?;
        // Cleared only after the row lands: if the insert fails the checkpoint
        // is still due and will be retried on the next tick.
        db::clear_checkpoint(&conn, session_id)?;
        id
    };

    let subtitle = match prep.session.intended_minutes {
        Some(m) => format!("{m} min intended, {} elapsed", prep.elapsed_minutes),
        None => format!("{} min elapsed", prep.elapsed_minutes),
    };
    notify::send(
        app,
        &format!("Time's up — {subtitle}"),
        &notify::clip(&format!("{headline} · {}", recommendation.advice), 160),
    );
    let _ = app.emit("checkpoint:new", id);
    raise_window(app);
    Ok(Some(id))
}

/// Best-effort only, and worth being honest about why: under GNOME on Wayland
/// an application cannot raise itself. set_focus() goes through xdg-activation,
/// and without a valid activation token gnome-shell turns the request into a
/// "demands attention" state rather than a raise. This genuinely helps when the
/// window is minimized or on the current workspace; the notification is the
/// signal that actually reaches you, and the overlay guarantees the checkpoint
/// is what is there when you do look.
fn raise_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
