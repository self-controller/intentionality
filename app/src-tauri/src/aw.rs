//! ActivityWatch REST client — the Rust twin of the old CLI dashboard's dashboard/aw.py (removed). AW being
//! down is a normal condition; every failure is one AwUnavailable error and
//! the UI shows observations as unavailable.

use crate::error::{AppError, Result};
use serde::Deserialize;
use std::collections::HashMap;

fn base_url() -> String {
    std::env::var("INTENTIONALITY_AW_URL").unwrap_or_else(|_| "http://localhost:5600".into())
}

fn client() -> Result<&'static reqwest::Client> {
    crate::http::local().map_err(|e| AppError::AwUnavailable(e.to_string()))
}

#[derive(Deserialize)]
pub struct AwEvent {
    pub timestamp: String,
    #[serde(default)]
    pub duration: Option<f64>, // float seconds; may be absent or null
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Deserialize)]
struct Bucket {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    last_updated: Option<String>,
}

/// (window_bucket_id, afk_bucket_id) — either may be None.
pub async fn find_buckets() -> Result<(Option<String>, Option<String>)> {
    let url = format!("{}/api/0/buckets/", base_url());
    let buckets: HashMap<String, Bucket> = client()?
        .get(url)
        .send()
        .await
        .map_err(|e| AppError::AwUnavailable(e.to_string()))?
        .json()
        .await
        .map_err(|e| AppError::AwUnavailable(e.to_string()))?;
    Ok((pick(&buckets, "currentwindow"), pick(&buckets, "afkstatus")))
}

/// The bucket of this type a watcher is writing to now. A machine can hold
/// several: each watcher names its buckets after the hostname, so a laptop
/// that once got a different name from DHCP keeps a second, abandoned pair.
/// Taking whichever one a HashMap happened to yield first read that empty
/// pair about half the time. Newest last_updated wins; never-written loses.
fn pick(buckets: &HashMap<String, Bucket>, kind: &str) -> Option<String> {
    buckets
        .iter()
        .filter(|(_, b)| b.kind == kind)
        .max_by_key(|(id, b)| {
            let updated = b.last_updated.as_deref().and_then(|t| crate::observed::parse_ts(t).ok());
            // The id breaks a tie, so the answer never depends on hash order.
            (updated, (*id).clone())
        })
        .map(|(id, _)| id.clone())
}

/// The newest event in a bucket — for a window watcher, the window focused
/// now (or last, if the watcher has stopped reporting). AW returns newest
/// first, so a limit of one is that event.
pub async fn latest(bucket_id: &str) -> Result<Option<AwEvent>> {
    let url = format!("{}/api/0/buckets/{}/events", base_url(), bucket_id);
    let events: Vec<AwEvent> = client()?
        .get(url)
        .query(&[("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::AwUnavailable(e.to_string()))?
        .json()
        .await
        .map_err(|e| AppError::AwUnavailable(e.to_string()))?;
    Ok(events.into_iter().next())
}

pub async fn events(bucket_id: &str, start_iso: &str, end_iso: &str) -> Result<Vec<AwEvent>> {
    let url = format!("{}/api/0/buckets/{}/events", base_url(), bucket_id);
    // .query() percent-encodes; the '+' in '+00:00' would otherwise decode
    // as a space server-side and silently shift the window.
    client()?
        .get(url)
        .query(&[("start", start_iso), ("end", end_iso), ("limit", "-1")])
        .send()
        .await
        .map_err(|e| AppError::AwUnavailable(e.to_string()))?
        .json()
        .await
        .map_err(|e| AppError::AwUnavailable(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(kind: &str, last_updated: Option<&str>) -> Bucket {
        Bucket { kind: kind.into(), last_updated: last_updated.map(Into::into) }
    }

    /// This machine's real layout since Sep 19: the live pair named after
    /// "fedora", and an abandoned pair from a DHCP hostname that was created
    /// and never written to.
    #[test]
    fn the_bucket_being_written_wins_over_an_abandoned_one() {
        let buckets = HashMap::from([
            ("aw-watcher-window_fedora".to_string(),
             bucket("currentwindow", Some("2026-10-19T15:35:50.380000+00:00"))),
            ("aw-watcher-afk_fedora".to_string(),
             bucket("afkstatus", Some("2026-10-19T15:36:10+00:00"))),
            ("aw-watcher-window_sas2e7ba05e3670.rad.rutgers.edu".to_string(),
             bucket("currentwindow", None)),
            ("aw-watcher-afk_sas2e7ba05e3670.rad.rutgers.edu".to_string(),
             bucket("afkstatus", None)),
        ]);
        assert_eq!(pick(&buckets, "currentwindow").as_deref(), Some("aw-watcher-window_fedora"));
        assert_eq!(pick(&buckets, "afkstatus").as_deref(), Some("aw-watcher-afk_fedora"));
        assert_eq!(pick(&buckets, "web.tab.current"), None);
    }

    /// Compared as times, not strings: Python's isoformat drops the
    /// microseconds when they are zero, so the strings differ in length.
    #[test]
    fn last_updated_is_compared_as_a_time() {
        let buckets = HashMap::from([
            ("old".to_string(), bucket("currentwindow", Some("2026-10-19T15:35:59.900000+00:00"))),
            ("new".to_string(), bucket("currentwindow", Some("2026-10-19T15:36:00+00:00"))),
        ]);
        assert_eq!(pick(&buckets, "currentwindow").as_deref(), Some("new"));
    }
}
