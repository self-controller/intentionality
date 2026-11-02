// Prevents an extra console window on Windows; harmless on Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod autolabel;
mod aw;
mod build_info;
mod check;
mod commands;
mod day;
mod db;
mod desktop;
mod error;
mod folder;
mod http;
mod ignore;
mod jev;
mod keybinds;
mod models;
mod notify;
mod observed;
mod overlay;
mod places;
mod recommendations;
mod relevance;
mod resume_units;
mod scheduler;
mod screen_time;
mod state;
mod subject;
mod today;

use state::AppState;

/// Seconds since epoch when this machine booted, from /proc/stat's btime.
/// Read once: boot time cannot change while we run, and adoption asks for it
/// on every no-session poll.
fn boot_time() -> Option<i64> {
    static BOOT: std::sync::OnceLock<Option<i64>> = std::sync::OnceLock::new();
    *BOOT.get_or_init(|| {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        stat.lines()
            .find(|l| l.starts_with("btime "))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    })
}

/// Which session is this desktop running in?
///
/// 1. INTENTIONALITY_SESSION_ID — but only if that session is still open.
///    The systemd user environment persists across logins, so a stale id
///    from yesterday can outlive its session. "Open" is session_is_open's
///    test, not ended_at alone: a session recovered before its first
///    heartbeat keeps ended_at NULL, and trusting that pinned the app to it
///    for good — every later poll came back here and never reached step 2.
/// 2. The latest open session — but only if it started after this boot.
///    After a crash plus a plain GDM login, adopting an older orphan would
///    mean heartbeating a session that isn't running.
/// 3. Neither: no-session mode (backlog + history only, no heartbeats).
pub(crate) fn adopt_session(conn: &rusqlite::Connection) -> Option<i64> {
    let env_id = std::env::var("INTENTIONALITY_SESSION_ID")
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok());
    adopt_session_from(conn, env_id)
}

fn adopt_session_from(conn: &rusqlite::Connection, env_id: Option<i64>) -> Option<i64> {
    if let Some(id) = env_id {
        if db::session_is_open(conn, id).unwrap_or(false) {
            return Some(id);
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
    // Same reasoning: "was the resume gate installed back then?" should be
    // answerable from the journal, not only from a header nobody screenshotted.
    if let Some(warning) = resume_units::warning() {
        eprintln!("intentionality: {warning}");
    }

    let conn = db::open().expect("cannot open the store");
    // Schema too old is NOT fatal here: health() reports it and the frontend
    // renders the migrate hint. A dead autostart app is invisible; a rendered
    // error screen is not.
    let session_id = match db::check_schema(&conn) {
        Ok(()) => adopt_session(&conn),
        Err(_) => None,
    };
    // Before the timer loop's first look: an ignored window must never get
    // a clock, not even the one in front at startup.
    ignore::set(ignore::load(&conn));
    // Where a day starts, before the clock's first look picks its day.
    day::put(day::load(&conn));

    tauri::Builder::default()
        .manage(AppState::new(conn, session_id))
        .setup(|app| {
            scheduler::spawn(app.handle().clone());
            screen_time::spawn(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::health,
            commands::get_board,
            commands::apply_board,
            commands::order_backlog,
            commands::add_task,
            commands::create_task,
            commands::update_task,
            commands::list_labels,
            commands::set_label_color,
            commands::delete_label,
            commands::delete_task,
            commands::pull_task,
            commands::unpull_task,
            commands::list_recent_analyses,
            commands::get_analysis_detail,
            commands::mark_analysis_seen,
            commands::run_analysis_now,
            commands::pending_checkpoint,
            commands::extend_checkpoint,
            commands::run_checkpoint_now,
            commands::risk_today,
            commands::add_risk_app,
            commands::remove_risk_app,
            commands::page_judgement,
            commands::appeal,
            commands::forget_relevance,
            commands::day_time,
            commands::judge_day,
            commands::day_start_hour,
            commands::set_day_start_hour,
            commands::keybinds,
            commands::set_keybinds,
            commands::ignore_list,
            commands::add_ignore,
            commands::remove_ignore,
            commands::installed_apps,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_session(conn: &rusqlite::Connection) -> i64 {
        conn.execute(
            "INSERT INTO session (started_at, mode) VALUES (?1, 'manual')",
            [db::now()],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn the_env_session_is_kept_while_open() {
        let conn = db::tests::store();
        let pinned = open_session(&conn);
        open_session(&conn);
        assert_eq!(adopt_session_from(&conn, Some(pinned)), Some(pinned));
    }

    /// The lid closed before the first heartbeat, so the resume gate's
    /// recovery left ended_at NULL; GNOME's environment still named it.
    #[test]
    fn an_env_session_recovered_without_a_heartbeat_gives_way_to_its_successor() {
        let conn = db::tests::store();
        let old = open_session(&conn);
        conn.execute("UPDATE session SET close_reason = 'recovered' WHERE id = ?1", [old])
            .unwrap();
        let new = open_session(&conn);
        assert_eq!(adopt_session_from(&conn, Some(old)), Some(new));
    }
}
