// Prevents an extra console window on Windows; harmless on Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod aw;
mod claude;
mod commands;
mod db;
mod error;
mod models;
mod observed;
mod scheduler;
mod state;

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
fn adopt_session(conn: &rusqlite::Connection) -> Option<i64> {
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

fn main() {
    let conn = db::open().expect("cannot open the store");
    // Schema too old is NOT fatal here: health() reports it and the frontend
    // renders the migrate hint. A dead autostart app is invisible; a rendered
    // error screen is not.
    let session_id = match db::check_schema(&conn) {
        Ok(()) => adopt_session(&conn),
        Err(_) => None,
    };

    tauri::Builder::default()
        .manage(AppState::new(conn, session_id))
        .setup(|app| {
            use tauri::Manager;
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
            commands::mark_analysis_seen,
            commands::run_analysis_now,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
