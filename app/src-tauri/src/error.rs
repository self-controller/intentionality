use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("store: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("ActivityWatch not reachable: {0}")]
    AwUnavailable(String),
    #[error("bad timestamp: {0}")]
    BadTimestamp(String),
    #[error("{0}")]
    Other(String),
    /// `stop_reason: "refusal"`: the API answered (HTTP 200, billed) and the
    /// model declined on content. Never a key, credit or network problem.
    /// Carries a ready-made suffix: " (category: …) (request …)".
    #[error("the model declined this request{0}")]
    Refused(String),
    /// A write aimed at a session the gate has since closed — the board on
    /// screen was stale. Worded for the user: the frontend shows it as is.
    #[error("session {0} was closed at the gate — showing the current board")]
    SessionClosed(i64),
}

// Commands surface errors to the webview as plain strings; the frontend
// renders them, it never needs to match on them.
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
