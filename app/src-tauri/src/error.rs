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
}

// Commands surface errors to the webview as plain strings; the frontend
// renders them, it never needs to match on them.
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
