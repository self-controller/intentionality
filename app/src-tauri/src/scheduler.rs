//! Three background loops: the 30s heartbeat, the randomized analysis timer,
//! and the time-up checkpoint. All live only while a session is open; all
//! stop themselves the moment the store says the session closed elsewhere.
//!
//! Both timers tick every minute against a wall-clock target instead of one
//! long sleep: CLOCK_MONOTONIC pauses during suspend, so a single 60-minute
//! sleep silently stretches by however long the lid was closed.
//!
//! The checkpoint's target is the store's session.checkpoint_due_at rather
//! than a value held here, which is what makes one that came due while the app
//! was down fire on the next start instead of being lost.

use crate::state::AppState;
use crate::{analysis, db, notify};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::Rng;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const HEARTBEAT_SECS: u64 = 30;
const DEFAULT_MEAN_MINUTES: f64 = 60.0;
const MIN_INTERVAL_MIN: f64 = 15.0;
const MAX_INTERVAL_MIN: f64 = 180.0;
/// Below this much active time in the window, skip: analyzing a locked
/// screen produces a confidently wrong "unfocused" note.
pub const MIN_ACTIVE_SECS: f64 = 300.0;

/// How often the checkpoint condition is re-read. Also the worst-case lateness
/// of a checkpoint, which is the right trade: a minute of slack costs nothing,
/// and a tighter tick would hammer the store all session for it.
const CHECKPOINT_TICK_SECS: u64 = 60;

/// A wall-clock tick this many seconds longer than the monotonic one means the
/// machine was suspended, not merely busy. CLOCK_MONOTONIC pauses across
/// suspend and the wall clock does not, so over one tick their difference *is*
/// the sleep. Well clear of ordinary scheduling slop at a 30s tick.
const SUSPEND_JUMP_SECS: i64 = 60;

/// While no session is held, look for one this often. The resume gate's new
/// session should be on the board within seconds of coming back from its
/// console, not a whole heartbeat later. One indexed read per tick.
const ADOPT_POLL_SECS: u64 = 5;

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(heartbeat_loop(app.clone()));
    tauri::async_runtime::spawn(analysis_loop(app.clone()));
    tauri::async_runtime::spawn(checkpoint_loop(app));
}

async fn heartbeat_loop(app: AppHandle) {
    let mut last_wall = Utc::now();
    let mut last_mono = Instant::now();
    loop {
        let tick = match app.state::<AppState>().session_id() {
            Some(_) => HEARTBEAT_SECS,
            None => ADOPT_POLL_SECS,
        };
        tokio::time::sleep(Duration::from_secs(tick)).await;
        let (wall, mono) = (Utc::now(), Instant::now());
        let slept =
            (wall - last_wall).num_seconds() - mono.duration_since(last_mono).as_secs() as i64;
        last_wall = wall;
        last_mono = mono;

        let state = app.state::<AppState>();
        let Some(session_id) = state.session_id() else {
            sync_session(&app);
            continue;
        };
        if slept >= SUSPEND_JUMP_SECS {
            // The machine was asleep, and last_heartbeat is meant to be the
            // moment you walked away — it is what the gate turns into
            // ended_at. Stamping it now would date the session's end to the
            // moment you came back, six hours late. Stay quiet and let the
            // resume gate close the session at the real time — but do look:
            // if it already has, hand over to its successor now.
            sync_session(&app);
            continue;
        }
        let alive = {
            let conn = state.conn.lock().unwrap();
            db::heartbeat(&conn, session_id)
        };
        match alive {
            Ok(true) => {}
            // Closed elsewhere (gate close / next gate / the resume gate's
            // recovery sweep). Never resurrect the row; hand over to its
            // successor if it exists yet, else poll for it.
            Ok(false) => sync_session(&app),
            // A busy store is not evidence the session ended. Dropping it here
            // used to strand the app in no-session mode for good, because
            // mid-flight adoption only takes sessions newer than the process.
            Err(err) => eprintln!("heartbeat failed, keeping session {session_id}: {err}"),
        }
    }
}

/// Reconcile the held session with the store and tell the frontend — after
/// the swap, so every listener's refetch already sees the successor.
pub fn sync_session(app: &AppHandle) {
    let (closed, opened) = app.state::<AppState>().sync_session();
    if closed.is_some() || opened.is_some() {
        // To the journal, so "when did the app let go of one session and pick
        // up the next?" is a grep, not a reconstruction from task timestamps.
        eprintln!("intentionality: session {closed:?} -> {opened:?}");
    }
    if let Some(id) = closed {
        let _ = app.emit("session:closed", id);
    }
    if let Some(id) = opened {
        let _ = app.emit("session:opened", id);
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
                // Scheduled checks notify; the manual "Run a check now" button
                // does not — you are already looking at the tab that answers it.
                notify_check(&app, id);
            }
            Ok(None) => {} // quiet window — skipped, redrawn below
            Err(err) => eprintln!("analysis skipped: {err}"), // a log line, never a dialog
        }
        next_fire = draw_next(mean_minutes(&app));
    }
}

/// Reads the row back rather than threading the text out of analysis::run:
/// one round trip, and the notification then says exactly what the Analyses
/// tab will say.
fn notify_check(app: &AppHandle, id: i64) {
    let state = app.state::<AppState>();
    let Some(session_id) = state.session_id() else { return };
    let row = {
        let conn = state.conn.lock().unwrap();
        db::list_analyses(&conn, session_id)
            .ok()
            .and_then(|list| list.into_iter().find(|a| a.id == id))
    };
    if let Some(a) = row {
        let band = match a.alignment {
            Some(v) if v >= 67 => "aligned",
            Some(v) if v >= 34 => "drifting",
            Some(_) => "off track",
            None => "not judged",
        };
        notify::send(
            app,
            &a.headline,
            &notify::clip(&format!("{band} · {}", a.body), 160),
        );
    }
}

async fn checkpoint_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(Duration::from_secs(CHECKPOINT_TICK_SECS)).await;
        let due = {
            let state = app.state::<AppState>();
            let Some(session_id) = state.session_id() else { continue };
            let conn = state.conn.lock().unwrap();
            db::checkpoint_due(&conn, session_id).unwrap_or(false)
        };
        if !due {
            continue;
        }
        // run_checkpoint clears the pending checkpoint itself, so this cannot
        // loop: either the row lands and the column is cleared, or the write
        // failed and it is right to try again next tick.
        if let Err(err) = analysis::run_checkpoint(&app).await {
            eprintln!("checkpoint failed: {err}");
        }
    }
}
