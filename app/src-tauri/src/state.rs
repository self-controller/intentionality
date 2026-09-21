use crate::db;
use crate::models::RecordingNow;
use crate::record;
use rusqlite::Connection;
use std::sync::Mutex;

pub struct AppState {
    pub conn: Mutex<Connection>,
    /// Held for the whole of one analysis. Two timers plus the manual button
    /// can now all reach the model; without this they can overlap, bill twice,
    /// and race on last_analysis_end — both runs would judge the same window.
    /// A tokio mutex, not a std one: it is held across awaits.
    pub analysis_lock: tokio::sync::Mutex<()>,
    session_id: Mutex<Option<i64>>,
    /// When this process started. The bar a session has to clear to be
    /// adopted mid-flight — see main::adopt_session_after.
    app_started_at: String,
    /// The meeting the microphone is feeding, if any. Its presence is what
    /// refuses a second Start, and holding the Stopper here is what lets the
    /// Stop command reach a pw-record process owned by a background task.
    meeting: Mutex<Option<MeetingHandle>>,
}

pub struct MeetingHandle {
    pub id: i64,
    /// When the microphone opened for this recording — the meeting's start
    /// for a new one, the moment it was picked back up for a resumed one.
    pub since: String,
    stopper: Option<record::Stopper>,
    /// The task draining the microphone. Stop awaits it so the final chunk is
    /// transcribed and stored before the transcript is handed to the model.
    capture: Option<tokio::task::JoinHandle<()>>,
}

impl MeetingHandle {
    pub fn new(
        id: i64,
        since: String,
        stopper: Option<record::Stopper>,
        capture: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self { id, since, stopper, capture: Some(capture) }
    }
    /// Close the microphone. The capture task then sees EOF, flushes its tail
    /// chunk and finishes.
    pub fn stop_capture(&mut self) {
        if let Some(stopper) = self.stopper.take() {
            stopper.stop();
        }
    }
    pub fn take_capture(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        self.capture.take()
    }
}

impl AppState {
    pub fn new(conn: Connection, session_id: Option<i64>) -> Self {
        Self {
            conn: Mutex::new(conn),
            analysis_lock: tokio::sync::Mutex::new(()),
            session_id: Mutex::new(session_id),
            app_started_at: db::now(),
            meeting: Mutex::new(None),
        }
    }
    pub fn session_id(&self) -> Option<i64> {
        *self.session_id.lock().unwrap()
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
        (closed, opened)
    }

    pub fn meeting_id(&self) -> Option<i64> {
        self.meeting.lock().unwrap().as_ref().map(|m| m.id)
    }
    pub fn recording(&self) -> Option<RecordingNow> {
        self.meeting
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| RecordingNow { meeting_id: m.id, since: m.since.clone() })
    }
    pub fn set_meeting(&self, handle: MeetingHandle) {
        *self.meeting.lock().unwrap() = Some(handle);
    }
    /// Take the running meeting out. Taking rather than reading is what makes
    /// two concurrent Stops safe: only one of them gets the handle.
    pub fn take_meeting(&self) -> Option<MeetingHandle> {
        self.meeting.lock().unwrap().take()
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
    fn nothing_held_and_nothing_open_changes_nothing() {
        let state = AppState::new(db::tests::store(), None);
        assert_eq!(state.sync_session(), (None, None));
        assert_eq!(state.session_id(), None);
    }
}
