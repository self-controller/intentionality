//! Port of dashboard/report.py::observed() — clip window events to the
//! session window, subtract AFK, accumulate per app — with one deliberate
//! fix: afk_seconds is the UNION of AFK spans, computed independently of
//! window events. The Python version accumulates AFK per window event, so a
//! locked screen (no window events at all) reports zero away time and
//! overlapping events double-count. Active seconds match Python exactly.

use crate::aw;
use crate::error::{AppError, Result};
use crate::models::Observed;
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use std::collections::BTreeMap;

pub fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // report.py treats offset-less timestamps as UTC; keep that insurance.
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .map(|n| n.and_utc())
        .map_err(|_| AppError::BadTimestamp(s.to_string()))
}

fn dur(ev: &aw::AwEvent) -> Duration {
    Duration::milliseconds((ev.duration.unwrap_or(0.0) * 1000.0) as i64)
}

fn merge_spans(
    mut spans: Vec<(DateTime<Utc>, DateTime<Utc>)>,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    spans.sort_by_key(|(s, _)| *s);
    let mut merged: Vec<(DateTime<Utc>, DateTime<Utc>)> = Vec::new();
    for (s, e) in spans {
        match merged.last_mut() {
            Some((_, le)) if s <= *le => *le = (*le).max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
}

fn secs(d: Duration) -> f64 {
    d.num_milliseconds() as f64 / 1000.0
}

pub async fn observed(start_iso: &str, end_iso: &str) -> Result<Observed> {
    let (window_bucket, afk_bucket) = aw::find_buckets().await?;
    let window_bucket =
        window_bucket.ok_or_else(|| AppError::AwUnavailable("no window-watcher bucket".into()))?;

    let start = parse_ts(start_iso)?;
    let end = parse_ts(end_iso)?;

    let mut afk_spans = Vec::new();
    if let Some(bucket) = afk_bucket {
        for ev in aw::events(&bucket, start_iso, end_iso).await? {
            if ev.data.get("status").and_then(|v| v.as_str()) == Some("afk") {
                let s = parse_ts(&ev.timestamp)?;
                let e = s + dur(&ev);
                let (s, e) = (s.max(start), e.min(end));
                if e > s {
                    afk_spans.push((s, e));
                }
            }
        }
    }
    let afk_spans = merge_spans(afk_spans);
    let afk_seconds: f64 = afk_spans.iter().map(|(s, e)| secs(*e - *s)).sum();

    let mut per_app: BTreeMap<String, f64> = BTreeMap::new();
    for ev in aw::events(&window_bucket, start_iso, end_iso).await? {
        let ev_start = parse_ts(&ev.timestamp)?;
        let ev_end = ev_start + dur(&ev);
        let (cs, ce) = (ev_start.max(start), ev_end.min(end));
        if ce <= cs {
            continue;
        }
        let mut active = secs(ce - cs);
        for (a_start, a_end) in &afk_spans {
            let overlap = secs(ce.min(*a_end) - cs.max(*a_start));
            if overlap > 0.0 {
                active -= overlap;
            }
        }
        if active > 0.0 {
            let app = ev
                .data
                .get("app")
                .and_then(|v| v.as_str())
                .filter(|a| !a.is_empty())
                .unwrap_or("unknown");
            *per_app.entry(app.to_string()).or_insert(0.0) += active;
        }
    }

    let active_seconds = per_app.values().sum();
    Ok(Observed { per_app, active_seconds, afk_seconds })
}
