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

/// How long a stretch with no keyboard or mouse input still counts as
/// present. The AFK watcher calls you away after 3 min without input and
/// dates the span back to the last input, so reading a long page or watching
/// Claude work used to vanish from every total -- and took the at-risk clock
/// down with it. GNOME blanks the screen at 5 min (`org.gnome.desktop.session
/// idle-delay`, 300 here); until then someone may well be looking, after it
/// nobody is. So the first IDLE_GRACE of every AFK span is not away time.
pub const IDLE_GRACE_SECS: i64 = 300;

pub fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // report.py treats offset-less timestamps as UTC; keep that insurance.
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .map(|n| n.and_utc())
        .map_err(|_| AppError::BadTimestamp(s.to_string()))
}

pub(crate) fn dur(ev: &aw::AwEvent) -> Duration {
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
    pub(crate) afk: Vec<aw::AwEvent>,
    pub(crate) window: Vec<aw::AwEvent>,
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

/// One window event's share of a span: how long it was focused inside the
/// span with the user not away. `app`/`title` are as the watcher wrote them,
/// possibly empty.
pub struct Active<'a> {
    pub app: &'a str,
    pub title: &'a str,
    pub seconds: f64,
}

/// The AFK spans in [start, end], each less its first IDLE_GRACE_SECS,
/// clipped and merged, so overlapping ones count once.
fn afk_spans(
    events: &Events,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>> {
    let mut spans = Vec::new();
    for ev in &events.afk {
        if ev.data.get("status").and_then(|v| v.as_str()) == Some("afk") {
            let idle = parse_ts(&ev.timestamp)?;
            let e = idle + dur(ev);
            let s = idle + Duration::seconds(IDLE_GRACE_SECS);
            let (s, e) = (s.max(start), e.min(end));
            if e > s {
                spans.push((s, e));
            }
        }
    }
    Ok(merge_spans(spans))
}

fn active_with<'a>(
    events: &'a Events,
    afk_spans: &[(DateTime<Utc>, DateTime<Utc>)],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<Active<'a>>> {
    let mut out = Vec::new();
    for ev in &events.window {
        let ev_start = parse_ts(&ev.timestamp)?;
        let ev_end = ev_start + dur(ev);
        let (cs, ce) = (ev_start.max(start), ev_end.min(end));
        if ce <= cs {
            continue;
        }
        let mut seconds = secs(ce - cs);
        for (a_start, a_end) in afk_spans {
            let overlap = secs(ce.min(*a_end) - cs.max(*a_start));
            if overlap > 0.0 {
                seconds -= overlap;
            }
        }
        if seconds > 0.0 {
            let field = |k: &str| ev.data.get(k).and_then(|v| v.as_str()).unwrap_or("");
            out.push(Active { app: field("app"), title: field("title"), seconds });
        }
    }
    Ok(out)
}

/// Every window event's active time in [start, end]. The one place AFK time
/// (past IDLE_GRACE_SECS) is subtracted: summarize below and the at-risk totals (screen_time.rs)
/// both build on it, so the dashboard and the timer can never disagree about
/// what counted.
pub fn active<'a>(events: &'a Events, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Active<'a>>> {
    active_with(events, &afk_spans(events, start, end)?, start, end)
}

/// Everything in `events` clipped to [start, end]. The span may be any part
/// of what was fetched: clipping is what makes a sub-window exact.
pub fn summarize(events: &Events, start_iso: &str, end_iso: &str) -> Result<Observed> {
    summarize_with(events, start_iso, end_iso, MAX_TITLES_PER_APP)
}

/// `summarize`, keeping up to `keep` titles per app before the rest rolls
/// into "(other)". `usize::MAX` keeps every title, for a caller that judges
/// each page and would otherwise see a browser's long tail as one lump.
pub fn summarize_with(events: &Events, start_iso: &str, end_iso: &str, keep: usize) -> Result<Observed> {
    let start = parse_ts(start_iso)?;
    let end = parse_ts(end_iso)?;

    let afk_spans = afk_spans(events, start, end)?;
    let afk_seconds: f64 = afk_spans.iter().map(|(s, e)| secs(*e - *s)).sum();

    let mut per_app: BTreeMap<String, f64> = BTreeMap::new();
    let mut per_title: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for a in active_with(events, &afk_spans, start, end)? {
        let app = if a.app.is_empty() { "unknown" } else { a.app };
        let title = if a.title.is_empty() { "unknown" } else { a.title };
        *per_app.entry(app.to_string()).or_insert(0.0) += a.seconds;
        *per_title
            .entry(app.to_string())
            .or_default()
            .entry(title.to_string())
            .or_insert(0.0) += a.seconds;
    }

    let per_title = per_title
        .into_iter()
        .map(|(app, titles)| (app, cap_titles(titles, keep)))
        .collect();

    let active_seconds = per_app.values().sum();
    Ok(Observed { per_app, per_title, active_seconds, afk_seconds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(timestamp: &str, duration: f64, data: serde_json::Value) -> aw::AwEvent {
        aw::AwEvent { timestamp: timestamp.into(), duration: Some(duration), data }
    }

    /// A short stretch without input is reading, not away; a long one is
    /// away only past the grace.
    #[test]
    fn the_first_minutes_without_input_still_count() {
        let events = Events {
            window: vec![ev("2026-10-28T09:00:00+00:00", 3600.0, json!({"app": "zen", "title": "a"}))],
            afk: vec![
                ev("2026-10-28T09:10:00+00:00", 240.0, json!({"status": "afk"})),
                ev("2026-10-28T09:30:00+00:00", 900.0, json!({"status": "afk"})),
                ev("2026-10-28T09:00:00+00:00", 600.0, json!({"status": "not-afk"})),
            ],
        };
        let o = summarize(&events, "2026-10-28T09:00:00+00:00", "2026-10-28T10:00:00+00:00").unwrap();
        assert_eq!(o.afk_seconds, 600.0);
        assert_eq!(o.active_seconds, 3000.0);
    }
}
