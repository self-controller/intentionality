use crate::db;
use rusqlite::Connection;
use std::sync::Mutex;

pub struct AppState {
    pub conn: Mutex<Connection>,
    /// Held for the whole of one analysis. Two timers plus the manual button
    /// can now all reach the model; without this they can overlap, bill twice,
    /// and race on where the latest analysis ended — both runs would judge the same window.
    /// A tokio mutex, not a std one: it is held across awaits.
    pub analysis_lock: tokio::sync::Mutex<()>,
    /// Poked when an appeal writes a verdict, so the timer loop judges the
    /// page in front again at once instead of on its next scheduled look.
    pub relevance_changed: tokio::sync::Notify,
    /// Poked by the timer loop when unexcused at-risk time since the last
    /// check reaches meta `drift_check_minutes`: the analysis loop then runs
    /// a check early rather than waiting for its draw.
    pub drift_check: tokio::sync::Notify,
    /// Seconds a "Not relevant to any task" clock has been on screen since
    /// the last check landed (or the session changed).
    drift: Mutex<f64>,
    session_id: Mutex<Option<i64>>,
    /// When this process started. The bar a session has to clear to be
    /// adopted mid-flight — see main::adopt_session_after.
    app_started_at: String,
}

impl AppState {
    pub fn new(conn: Connection, session_id: Option<i64>) -> Self {
        Self {
            conn: Mutex::new(conn),
            analysis_lock: tokio::sync::Mutex::new(()),
            relevance_changed: tokio::sync::Notify::new(),
            drift_check: tokio::sync::Notify::new(),
            drift: Mutex::new(0.0),
            session_id: Mutex::new(session_id),
            app_started_at: db::now(),
        }
    }
    pub fn session_id(&self) -> Option<i64> {
        *self.session_id.lock().unwrap()
    }
    /// Count `secs` more of unexcused at-risk time; the new total.
    pub fn add_drift(&self, secs: f64) -> f64 {
        let mut drift = self.drift.lock().unwrap();
        *drift += secs.max(0.0);
        *drift
    }
    pub fn drift_seconds(&self) -> f64 {
        *self.drift.lock().unwrap()
    }
    pub fn reset_drift(&self) {
        *self.drift.lock().unwrap() = 0.0;
    }
    /// Make the held session match the store: let go of one closed elsewhere
    /// and, in the same step, adopt its successor — what the resume gate
    /// leaves behind on its spare VT. Doing both before anyone is told is what
    /// keeps the board from showing "No open session" for a whole heartbeat
    /// while the new session already exists.
    ///
    /// Returns (the session let go of, the session adopted). Runs under the
    /// connection lock, so two callers cannot both swap.
    pub fn sync_session(&self) -> (Option<i64>, Option<i64>) {
        let conn = self.conn.lock().unwrap();
        let mut held = self.session_id.lock().unwrap();
        let mut closed = None;
        if let Some(id) = *held {
            // An error is not evidence of a close; the next tick asks again.
            if db::session_is_open(&conn, id).unwrap_or(true) {
                return (None, None);
            }
            closed = Some(id);
        }
        let opened = crate::adopt_session_after(&conn, &self.app_started_at);
        *held = opened;
        // Drift belongs to the session it was counted in.
        self.reset_drift();
        (closed, opened)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn open_session(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO session (started_at, mode) VALUES (?1, 'manual')",
            [db::now()],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn close(state: &AppState, id: i64) {
        state
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE session SET ended_at = ?1 WHERE id = ?2", params![db::now(), id])
            .unwrap();
    }

    #[test]
    fn an_open_session_is_kept() {
        let conn = db::tests::store();
        let id = open_session(&conn);
        let state = AppState::new(conn, Some(id));
        assert_eq!(state.sync_session(), (None, None));
        assert_eq!(state.session_id(), Some(id));
    }

    /// The empty board after a resume gate: letting go of the closed session
    /// and finding its successor used to take two heartbeat ticks, 30 s apart.
    #[test]
    fn a_closed_session_hands_over_to_its_successor_in_one_step() {
        let conn = db::tests::store();
        let old = open_session(&conn);
        let state = AppState::new(conn, Some(old));
        close(&state, old);
        let new = open_session(&state.conn.lock().unwrap());
        assert_eq!(state.sync_session(), (Some(old), Some(new)));
        assert_eq!(state.session_id(), Some(new));
    }

    #[test]
    fn a_closed_session_with_no_successor_leaves_none() {
        let conn = db::tests::store();
        let old = open_session(&conn);
        let state = AppState::new(conn, Some(old));
        close(&state, old);
        assert_eq!(state.sync_session(), (Some(old), None));
        assert_eq!(state.session_id(), None);
    }

    #[test]
    fn drift_counts_up_and_starts_over_with_a_new_session() {
        let conn = db::tests::store();
        let old = open_session(&conn);
        let state = AppState::new(conn, Some(old));
        assert_eq!(state.add_drift(1.0), 1.0);
        assert_eq!(state.add_drift(-5.0), 1.0); // a clock running backwards adds nothing
        assert_eq!(state.add_drift(2.5), 3.5);
        state.sync_session(); // still open: nothing changes, drift included
        assert_eq!(state.drift_seconds(), 3.5);
        close(&state, old);
        state.sync_session();
        assert_eq!(state.drift_seconds(), 0.0);
        state.add_drift(4.0);
        state.reset_drift();
        assert_eq!(state.drift_seconds(), 0.0);
    }

    #[test]
    fn nothing_held_and_nothing_open_changes_nothing() {
        let state = AppState::new(db::tests::store(), None);
        assert_eq!(state.sync_session(), (None, None));
        assert_eq!(state.session_id(), None);
    }
}
