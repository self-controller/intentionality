//! The HTTP clients, built once and shared. A reqwest::Client owns a
//! connection pool and its TLS setup; building one per request threw both
//! away every time -- a fresh handshake per transcription chunk, per model
//! call, per ActivityWatch query.

use std::sync::OnceLock;
use std::time::Duration;

static REMOTE: OnceLock<reqwest::Client> = OnceLock::new();
static LOCAL: OnceLock<reqwest::Client> = OnceLock::new();

fn cached(
    cell: &'static OnceLock<reqwest::Client>,
    builder: reqwest::ClientBuilder,
) -> Result<&'static reqwest::Client, reqwest::Error> {
    if let Some(client) = cell.get() {
        return Ok(client);
    }
    let client = builder.build()?;
    Ok(cell.get_or_init(|| client))
}

/// The Anthropic and OpenAI APIs. Callers set the overall timeout per
/// request, because the spread is real: a two-sentence check is done in
/// seconds, repairing an hour of transcript is not.
pub fn remote() -> Result<&'static reqwest::Client, reqwest::Error> {
    cached(&REMOTE, reqwest::Client::builder().connect_timeout(Duration::from_secs(3)))
}

/// ActivityWatch on localhost. Short, because AW being down is normal and
/// the UI should say so promptly.
pub fn local() -> Result<&'static reqwest::Client, reqwest::Error> {
    cached(&LOCAL, reqwest::Client::builder().timeout(Duration::from_secs(3)))
}
