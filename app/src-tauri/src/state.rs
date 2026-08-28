use rusqlite::Connection;
use std::sync::Mutex;

pub struct AppState {
    pub conn: Mutex<Connection>,
    session_id: Mutex<Option<i64>>,
}

impl AppState {
    pub fn new(conn: Connection, session_id: Option<i64>) -> Self {
        Self { conn: Mutex::new(conn), session_id: Mutex::new(session_id) }
    }
    pub fn session_id(&self) -> Option<i64> {
        *self.session_id.lock().unwrap()
    }
    pub fn clear_session(&self) {
        *self.session_id.lock().unwrap() = None;
    }
}
