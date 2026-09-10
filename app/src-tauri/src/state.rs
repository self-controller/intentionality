use crate::db;
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
    stopper: Option<record::Stopper>,
    /// The task draining the microphone. Stop awaits it so the final chunk is
    /// transcribed and stored before the transcript is handed to the model.
    capture: Option<tokio::task::JoinHandle<()>>,
}

impl MeetingHandle {
    pub fn new(id: i64, stopper: Option<record::Stopper>, capture: tokio::task::JoinHandle<()>) -> Self {
        Self { id, stopper, capture: Some(capture) }
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
    pub fn clear_session(&self) {
        *self.session_id.lock().unwrap() = None;
    }
    /// Adopt a session that did not exist at startup — what the resume gate
    /// creates on its spare VT after the machine wakes. Without this the app
    /// could only ever lose a session, never gain one.
    pub fn set_session(&self, id: i64) {
        *self.session_id.lock().unwrap() = Some(id);
    }
    pub fn app_started_at(&self) -> &str {
        &self.app_started_at
    }

    pub fn meeting_id(&self) -> Option<i64> {
        self.meeting.lock().unwrap().as_ref().map(|m| m.id)
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
