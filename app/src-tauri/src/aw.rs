//! ActivityWatch REST client — the Rust twin of dashboard/aw.py. AW being
//! down is a normal condition; every failure is one AwUnavailable error and
//! the UI shows observations as unavailable.

use crate::error::{AppError, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

fn base_url() -> String {
    std::env::var("INTENTIONALITY_AW_URL").unwrap_or_else(|_| "http://localhost:5600".into())
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| AppError::AwUnavailable(e.to_string()))
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
    let mut window = None;
    let mut afk = None;
    for (id, b) in buckets {
        match b.kind.as_str() {
            "currentwindow" => window = Some(id),
            "afkstatus" => afk = Some(id),
            _ => {}
        }
    }
    Ok((window, afk))
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
