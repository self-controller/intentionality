// Prevents an extra console window on Windows; harmless on Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod audio;
mod aw;
mod build_info;
mod claude;
mod commands;
mod db;
mod error;
mod gain;
mod level;
mod meeting;
mod models;
mod notify;
mod observed;
mod recommendations;
mod record;
mod scheduler;
mod state;
mod transcribe;

use state::AppState;

/// Seconds since epoch when this machine booted, from /proc/stat's btime.
fn boot_time() -> Option<i64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    stat.lines()
        .find(|l| l.starts_with("btime "))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Which session is this desktop running in?
///
/// 1. INTENTIONALITY_SESSION_ID — but only if that session is still open.
///    The systemd user environment persists across logins, so a stale id
///    from yesterday can outlive its session.
/// 2. The latest open session — but only if it started after this boot.
///    After a crash plus a plain GDM login, adopting an older orphan would
///    mean heartbeating a session that isn't running.
/// 3. Neither: no-session mode (backlog + history only, no heartbeats).
pub(crate) fn adopt_session(conn: &rusqlite::Connection) -> Option<i64> {
    if let Ok(raw) = std::env::var("INTENTIONALITY_SESSION_ID") {
        if let Ok(id) = raw.parse::<i64>() {
            if let Ok(Some(session)) = db::get_session(conn, id) {
                if session.ended_at.is_none() {
                    return Some(id);
                }
            }
        }
    }
    let session = db::latest_open_session(conn).ok().flatten()?;
    let started = observed::parse_ts(&session.started_at).ok()?.timestamp();
    if started >= boot_time()? {
        Some(session.id)
    } else {
        None
    }
}

/// Adoption re-run while the app is already going, for a session that did not
/// exist at startup — what the resume gate creates on its spare VT after the
/// machine wakes.
///
/// A tighter bar than adopt_session's "started after this boot": the session
/// must have started after *this process* did. Otherwise the first tick after
/// a `gate close` would re-adopt the very session that was just closed and
/// reopened by hand earlier in the day. Both timestamps come from the store's
/// one format, so a string compare is a time compare.
pub(crate) fn adopt_session_after(
    conn: &rusqlite::Connection,
    not_before: &str,
) -> Option<i64> {
    let id = adopt_session(conn)?;
    let session = db::get_session(conn, id).ok().flatten()?;
    (session.started_at.as_str() >= not_before).then_some(id)
}

fn main() {
    // At autostart this goes to the journal, so `journalctl --user` can answer
    // "which build was running at the time?" after the fact — the question
    // that made a day-old binary at login invisible.
    let build = build_info::current();
    eprintln!(
        "intentionality v{} built {}{}",
        build.version,
        build.built_at.as_deref().unwrap_or("unknown"),
        build
            .stale_since
            .as_deref()
            .map(|at| format!(" — STALE, source changed {at}"))
            .unwrap_or_default(),
    );

    let conn = db::open().expect("cannot open the store");
    // Schema too old is NOT fatal here: health() reports it and the frontend
    // renders the migrate hint. A dead autostart app is invisible; a rendered
    // error screen is not.
    let session_id = match db::check_schema(&conn) {
        Ok(()) => {
            // A meeting still marked 'recording' means the app died mid-meeting:
            // the microphone is long gone, but everything transcribed up to that
            // point is in the store and should be readable rather than stuck
            // behind a recording indicator that will never clear.
            match db::sweep_orphan_meetings(&conn) {
                Ok(n) if n > 0 => eprintln!("intentionality: closed {n} interrupted meeting(s)"),
                Err(err) => eprintln!("intentionality: meeting sweep failed: {err}"),
                _ => {}
            }
            adopt_session(&conn)
        }
        Err(_) => None,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::new(conn, session_id))
        .setup(|app| {
            scheduler::spawn(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::health,
            commands::get_board,
            commands::apply_board,
            commands::add_task,
            commands::rename_task,
            commands::delete_task,
            commands::pull_task,
            commands::list_sessions,
            commands::get_session_tasks,
            commands::get_observed,
            commands::list_analyses,
            commands::list_recent_analyses,
            commands::get_analysis_observed,
            commands::mark_analysis_seen,
            commands::run_analysis_now,
            commands::pending_checkpoint,
            commands::extend_checkpoint,
            commands::run_checkpoint_now,
            commands::list_audio_sources,
            commands::start_meeting,
            commands::stop_meeting,
            commands::recording_meeting,
            commands::list_meetings,
            commands::get_meeting,
            commands::approve_actions,
            commands::resummarize_meeting,
            commands::delete_meeting,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
