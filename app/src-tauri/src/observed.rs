//! Port of the old CLI dashboard's observed() (dashboard/report.py, removed;
//! git history has it) — clip window events to the
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

const MAX_TITLES_PER_APP: usize = 15;

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

/// A browser alone can produce hundreds of distinct titles in one session, so
/// keep only the longest-running ones. The tail collapses into "(other)" rather
/// than being dropped, so an app's titles still sum to its total.
fn cap_titles(titles: BTreeMap<String, f64>, keep: usize) -> BTreeMap<String, f64> {
    if titles.len() <= keep {
        return titles;
    }
    let mut ranked: Vec<(String, f64)> = titles.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let tail: f64 = ranked[keep..].iter().map(|(_, s)| s).sum();
    let mut kept: BTreeMap<String, f64> = ranked.into_iter().take(keep).collect();
    *kept.entry("(other)".to_string()).or_insert(0.0) += tail;
    kept
}

/// ActivityWatch's raw events for one span, fetched once so several windows
/// inside it can be summarized without asking AW again.
pub struct Events {
    afk: Vec<aw::AwEvent>,
    window: Vec<aw::AwEvent>,
}

pub async fn fetch(start_iso: &str, end_iso: &str) -> Result<Events> {
    let (window_bucket, afk_bucket) = aw::find_buckets().await?;
    let window_bucket =
        window_bucket.ok_or_else(|| AppError::AwUnavailable("no window-watcher bucket".into()))?;
    let afk = match afk_bucket {
        Some(bucket) => aw::events(&bucket, start_iso, end_iso).await?,
        None => Vec::new(),
    };
    let window = aw::events(&window_bucket, start_iso, end_iso).await?;
    Ok(Events { afk, window })
}

pub async fn observed(start_iso: &str, end_iso: &str) -> Result<Observed> {
    summarize(&fetch(start_iso, end_iso).await?, start_iso, end_iso)
}

/// Everything in `events` clipped to [start, end]. The span may be any part
/// of what was fetched: clipping is what makes a sub-window exact.
pub fn summarize(events: &Events, start_iso: &str, end_iso: &str) -> Result<Observed> {
    let start = parse_ts(start_iso)?;
    let end = parse_ts(end_iso)?;

    let mut afk_spans = Vec::new();
    for ev in &events.afk {
        if ev.data.get("status").and_then(|v| v.as_str()) == Some("afk") {
            let s = parse_ts(&ev.timestamp)?;
            let e = s + dur(ev);
            let (s, e) = (s.max(start), e.min(end));
            if e > s {
                afk_spans.push((s, e));
            }
        }
    }
    let afk_spans = merge_spans(afk_spans);
    let afk_seconds: f64 = afk_spans.iter().map(|(s, e)| secs(*e - *s)).sum();

    let mut per_app: BTreeMap<String, f64> = BTreeMap::new();
    let mut per_title: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for ev in &events.window {
        let ev_start = parse_ts(&ev.timestamp)?;
        let ev_end = ev_start + dur(ev);
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
            let title = ev
                .data
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|t| !t.is_empty())
                .unwrap_or("unknown");
            *per_app.entry(app.to_string()).or_insert(0.0) += active;
            *per_title
                .entry(app.to_string())
                .or_default()
                .entry(title.to_string())
                .or_insert(0.0) += active;
        }
    }

    let per_title = per_title
        .into_iter()
        .map(|(app, titles)| (app, cap_titles(titles, MAX_TITLES_PER_APP)))
        .collect();

    let active_seconds = per_app.values().sum();
    Ok(Observed { per_app, per_title, active_seconds, afk_seconds })
}
