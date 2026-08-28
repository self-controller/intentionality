//! Two background loops: the 30s heartbeat and the randomized analysis
//! timer. Both live only while a session is open; both stop themselves the
//! moment the store says the session closed elsewhere.
//!
//! The analysis timer ticks every minute against a wall-clock next_fire_at
//! instead of one long sleep: CLOCK_MONOTONIC pauses during suspend, so a
//! single 60-minute sleep silently stretches by however long the lid was
//! closed.

use crate::state::AppState;
use crate::{analysis, db};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::Rng;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const HEARTBEAT_SECS: u64 = 30;
const DEFAULT_MEAN_MINUTES: f64 = 60.0;
const MIN_INTERVAL_MIN: f64 = 15.0;
const MAX_INTERVAL_MIN: f64 = 180.0;
/// Below this much active time in the window, skip: analyzing a locked
/// screen produces a confidently wrong "unfocused" note.
pub const MIN_ACTIVE_SECS: f64 = 300.0;

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(heartbeat_loop(app.clone()));
    tauri::async_runtime::spawn(analysis_loop(app));
}

async fn heartbeat_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(Duration::from_secs(HEARTBEAT_SECS)).await;
        let state = app.state::<AppState>();
        let Some(session_id) = state.session_id() else { continue };
        let alive = {
            let conn = state.conn.lock().unwrap();
            db::heartbeat(&conn, session_id).unwrap_or(false)
        };
        if !alive {
            // Closed elsewhere (gate close / next gate). Flip to no-session
            // mode rather than resurrecting the row.
            state.clear_session();
            let _ = app.emit("session:closed", session_id);
        }
    }
}

fn draw_next(mean_minutes: f64) -> DateTime<Utc> {
    let factor: f64 = rand::thread_rng().gen_range(0.5..1.5);
    let minutes = (mean_minutes * factor).clamp(MIN_INTERVAL_MIN, MAX_INTERVAL_MIN);
    Utc::now() + ChronoDuration::seconds((minutes * 60.0) as i64)
}

fn mean_minutes(app: &AppHandle) -> f64 {
    let state = app.state::<AppState>();
    let conn = state.conn.lock().unwrap();
    db::get_setting(&conn, "analysis_mean_minutes")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MEAN_MINUTES)
}

async fn analysis_loop(app: AppHandle) {
    let mut next_fire = draw_next(mean_minutes(&app));
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        if Utc::now() < next_fire {
            continue;
        }
        let state = app.state::<AppState>();
        if state.session_id().is_none() {
            next_fire = draw_next(mean_minutes(&app));
            continue;
        }
        match analysis::run(&app).await {
            Ok(Some(id)) => {
                let _ = app.emit("analysis:new", id);
            }
            Ok(None) => {} // quiet window — skipped, redrawn below
            Err(err) => eprintln!("analysis skipped: {err}"), // a log line, never a dialog
        }
        next_fire = draw_next(mean_minutes(&app));
    }
}
