//! Assembles the context for one productivity check, calls the model, and
//! persists the result. Returns Ok(None) when the window was too quiet to
//! judge fairly.

use crate::error::Result;
use crate::state::AppState;
use crate::{claude, db, observed, scheduler};
use tauri::{AppHandle, Manager};

pub async fn run(app: &AppHandle) -> Result<Option<i64>> {
    let state = app.state::<AppState>();
    let Some(session_id) = state.session_id() else { return Ok(None) };

    // Everything read up front so no lock is held across awaits.
    let (session, tasks, window_start, previous) = {
        let conn = state.conn.lock().unwrap();
        let Some(session) = db::get_session(&conn, session_id)? else { return Ok(None) };
        let tasks = db::session_tasks(&conn, session_id)?;
        let window_start = db::last_analysis_end(&conn, session_id)?
            .unwrap_or_else(|| session.started_at.clone());
        let previous = db::list_analyses(&conn, session_id)?
            .into_iter()
            .next()
            .map(|a| (a.headline, a.alignment));
        (session, tasks, window_start, previous)
    };

    let window_end = db::now();
    let window = observed::observed(&window_start, &window_end).await?;
    if window.active_seconds < scheduler::MIN_ACTIVE_SECS {
        return Ok(None); // too quiet to judge fairly
    }
    let session_total = observed::observed(&session.started_at, &window_end).await.ok();

    let mut lanes = [
        ("todo", Vec::new()),
        ("doing", Vec::new()),
        ("done", Vec::new()),
        ("dropped", Vec::new()),
    ];
    for task in &tasks {
        let lane = match task.status.as_str() {
            "planned" => 0,
            "doing" => 1,
            "done" => 2,
            _ => 3,
        };
        lanes[lane].1.push(task.title.as_str());
    }
    let board_lines = lanes
        .iter()
        .map(|(name, titles)| format!("{name}: {}", if titles.is_empty() { "-".into() } else { titles.join(" | ") }))
        .collect::<Vec<_>>()
        .join("\n");

    let elapsed_minutes = observed::parse_ts(&window_end)?
        .signed_duration_since(observed::parse_ts(&session.started_at)?)
        .num_minutes();
    let window_minutes = observed::parse_ts(&window_end)?
        .signed_duration_since(observed::parse_ts(&window_start)?)
        .num_minutes();

    let result = claude::analyze(&claude::Context {
        statement: &session.statement,
        intended_minutes: session.intended_minutes,
        elapsed_minutes,
        board_lines,
        window: &window,
        session_total: session_total.as_ref(),
        window_minutes,
        previous,
    })
    .await?;

    let observed_json = serde_json::to_string(&window).unwrap_or_else(|_| "{}".into());
    let conn = state.conn.lock().unwrap();
    let id = db::add_analysis(
        &conn,
        session_id,
        &window_start,
        &window_end,
        &result.headline,
        result.alignment,
        &result.body,
        &observed_json,
    )?;
    Ok(Some(id))
}
