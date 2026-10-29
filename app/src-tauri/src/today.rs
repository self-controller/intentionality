//! Today since local midnight, as the Dashboard leads with it: how long was
//! active, how long away, and how much of the active time was on pages some
//! task needed.
//!
//! "Relevant" is the at-risk timer's and the checks' own rule, through their
//! own code: every page goes through check.rs `pages`/`tally` and the
//! relevance cache, so the three can never disagree about a page. A day can
//! span several sessions; every page in any of them is judged against the
//! open session's Doing column, the timer's own task set -- today measured
//! against what you are doing now. While nothing is in Doing, nothing is
//! relevant: every page of the day is off task, as the timer shows it. Time
//! in no session is not judged.
//!
//! Reading is free: the cache only. `judge` asks Jev about the busiest pages
//! still unjudged, a check's worth at most, and is what the Dashboard's
//! button calls.

use crate::check::{self, Pages, Tally};
use crate::error::Result;
use crate::models::{Observed, Session, Task, TaskTime, TodayTime};
use crate::relevance::{self, Page};
use crate::state::AppState;
use crate::{db, observed, screen_time};
use chrono::{DateTime, Local, Utc};
use rusqlite::Connection;
use tauri::{AppHandle, Manager};

/// How many recent sessions are read to find today's. A day holds a handful.
const SESSIONS_READ: i64 = 20;

/// One session's share of today.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub session_id: i64,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Each session's part of [day_start, now], in time order. A session ends at
/// its `ended_at`, an open one at now, and none runs past the next one's
/// start. A closed session whose end was never known (recovered before its
/// first heartbeat) claims nothing: its time is outside a session rather
/// than guessed onto its tasks.
pub fn slices(sessions: &[Session], day_start: DateTime<Utc>, now: DateTime<Utc>) -> Vec<Slice> {
    let mut timed: Vec<(DateTime<Utc>, &Session)> = sessions
        .iter()
        .filter_map(|s| observed::parse_ts(&s.started_at).ok().map(|t| (t, s)))
        .collect();
    timed.sort_by_key(|(t, s)| (*t, s.id));
    let mut out = Vec::new();
    for (i, (started, s)) in timed.iter().enumerate() {
        let ended = s.ended_at.as_deref().and_then(|e| observed::parse_ts(e).ok());
        let end = match ended {
            Some(e) => e,
            None if s.close_reason.is_none() => now,
            None => *started,
        };
        let end = timed.get(i + 1).map_or(end, |(next, _)| end.min(*next)).min(now);
        let start = (*started).max(day_start);
        if end > start {
            out.push(Slice { session_id: s.id, start, end });
        }
    }
    out
}

/// What the day's slices add up to. `whole` is the day as one window: its
/// active time less the slices' is time in no session.
pub fn combine(whole: &Observed, tallies: &[Tally], min: f64) -> TodayTime {
    let mut t = TodayTime {
        active_seconds: whole.active_seconds,
        afk_seconds: whole.afk_seconds,
        threshold: min,
        ..Default::default()
    };
    let mut in_sessions = 0.0;
    let mut tasks: Vec<TaskTime> = Vec::new();
    for x in tallies {
        t.aligned_seconds += x.aligned;
        t.off_seconds += x.off;
        t.unjudged_seconds += x.unjudged;
        in_sessions += x.aligned + x.off + x.unjudged;
        for task in x.tasks.iter().filter(|k| k.seconds > 0.0) {
            // A task carried into a later session is a new row with the same
            // title; to the person it is one task.
            match tasks.iter_mut().find(|k| k.title == task.title) {
                Some(k) => k.seconds += task.seconds,
                None => tasks.push(task.clone()),
            }
        }
    }
    t.outside_seconds = (whole.active_seconds - in_sessions).max(0.0);
    t.unjudged_seconds += t.outside_seconds;
    tasks.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    t.tasks = tasks;
    t.askable = tallies
        .iter()
        // No tasks, no question: such a page would be asked about forever.
        .filter(|x| !x.tasks.is_empty())
        .map(|x| x.pages.iter().filter(|p| p.verdict == "unjudged").count())
        .sum::<usize>()
        .min(check::MAX_PAGES);
    t
}

/// One slice ready to be judged: its pages, and the tasks that may excuse them.
struct Part {
    pages: Pages,
    tasks: Vec<Task>,
}

/// Every part tallied from the cache alone.
fn tallies(conn: &Connection, parts: &[Part], min: f64) -> Result<Vec<Tally>> {
    parts
        .iter()
        .map(|part| {
            let judgements = part
                .pages
                .pages
                .iter()
                .map(|p| {
                    let page = Page { app: p.app.clone(), title: p.title.clone() };
                    relevance::cached_for(conn, part.tasks.clone(), &page).map(Some)
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(check::tally(&part.pages, &judgements, &part.tasks, min))
        })
        .collect()
}

/// The pages a judge pass asks about: unjudged ones of slices that have
/// tasks, busiest first, a check's worth.
fn to_ask(parts: &[Part], first: &[Tally]) -> Vec<(Page, Vec<Task>)> {
    let mut asks: Vec<(f64, Page, Vec<Task>)> = Vec::new();
    for (part, tally) in parts.iter().zip(first) {
        if part.tasks.is_empty() {
            continue;
        }
        for (p, row) in part.pages.pages.iter().zip(&tally.pages) {
            if row.verdict == "unjudged" {
                let page = Page { app: p.app.clone(), title: p.title.clone() };
                asks.push((p.seconds, page, part.tasks.clone()));
            }
        }
    }
    asks.sort_by(|a, b| b.0.total_cmp(&a.0));
    asks.truncate(check::MAX_PAGES);
    asks.into_iter().map(|(_, page, tasks)| (page, tasks)).collect()
}

/// Today so far. One ActivityWatch fetch; every slice is cut from it.
pub async fn today(app: &AppHandle, judge: bool) -> Result<TodayTime> {
    let now = Utc::now();
    let day_start = screen_time::today_start(Local::now());
    let (start_iso, now_iso) = (screen_time::iso(day_start), screen_time::iso(now));
    let events = observed::fetch(&start_iso, &now_iso).await?;
    let whole = observed::summarize(&events, &start_iso, &now_iso)?;

    let state = app.state::<AppState>();
    let (cut, min) = {
        let conn = state.conn.lock().unwrap();
        let sessions = db::list_sessions(&conn, SESSIONS_READ)?;
        let doing = relevance::timer_tasks(&conn, state.session_id())?;
        let cut: Vec<_> =
            slices(&sessions, day_start, now).into_iter().map(|s| (s, doing.clone())).collect();
        (cut, db::relevance_min(&conn))
    };
    let mut parts = Vec::with_capacity(cut.len());
    for (s, tasks) in cut {
        // Every title kept: a browser's long tail is pages too, and rolled
        // into "(other)" it could never be judged.
        let obs = observed::summarize_with(
            &events,
            &screen_time::iso(s.start),
            &screen_time::iso(s.end),
            usize::MAX,
        )?;
        parts.push(Part { pages: check::pages(&obs, usize::MAX), tasks });
    }

    let mut error = None;
    if judge {
        let first = {
            let conn = state.conn.lock().unwrap();
            tallies(&conn, &parts, min)?
        };
        let judged = relevance::judge_many(app, to_ask(&parts, &first)).await;
        error = judged.iter().flatten().find_map(|j| j.error.clone());
    }
    let last = {
        let conn = state.conn.lock().unwrap();
        tallies(&conn, &parts, min)?
    };
    Ok(TodayTime { error, ..combine(&whole, &last, min) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::JudgedPage;

    fn at(s: &str) -> DateTime<Utc> {
        observed::parse_ts(s).unwrap()
    }

    fn session(id: i64, started: &str, ended: Option<&str>, reason: Option<&str>) -> Session {
        Session {
            id,
            started_at: started.into(),
            ended_at: ended.map(Into::into),
            close_reason: reason.map(Into::into),
            statement: String::new(),
            intended_minutes: None,
            checkpoint_due_at: None,
            first_task: None,
            task_count: 0,
        }
    }

    const MIDNIGHT: &str = "2026-10-27T04:00:00+00:00";
    const NOW: &str = "2026-10-27T17:00:00+00:00";

    fn cut(sessions: &[Session]) -> Vec<(i64, String, String)> {
        slices(sessions, at(MIDNIGHT), at(NOW))
            .into_iter()
            .map(|s| (s.session_id, screen_time::iso(s.start), screen_time::iso(s.end)))
            .collect()
    }

    #[test]
    fn sessions_are_cut_to_today_and_to_each_other() {
        let got = cut(&[
            // Listed newest first, as db::list_sessions gives them.
            session(4, "2026-10-27T15:00:00+00:00", None, None),
            session(3, "2026-10-27T12:00:00+00:00", Some("2026-10-27T15:30:00+00:00"), Some("debrief")),
            session(2, "2026-10-27T01:00:00+00:00", Some("2026-10-27T09:00:00+00:00"), Some("recovered")),
            session(1, "2026-10-26T20:00:00+00:00", Some("2026-10-26T23:00:00+00:00"), Some("debrief")),
        ]);
        assert_eq!(
            got,
            [
                // Began before midnight: only today's part.
                (2, MIDNIGHT.to_string(), "2026-10-27T09:00:00+00:00".to_string()),
                // Its recorded end runs into the next session: cut there.
                (3, "2026-10-27T12:00:00+00:00".to_string(), "2026-10-27T15:00:00+00:00".to_string()),
                // Open: until now.
                (4, "2026-10-27T15:00:00+00:00".to_string(), NOW.to_string()),
            ]
        );
    }

    #[test]
    fn a_closed_session_with_no_known_end_claims_nothing() {
        let got = cut(&[
            session(6, "2026-10-27T10:00:00+00:00", None, None),
            session(5, "2026-10-27T08:00:00+00:00", None, Some("recovered")),
        ]);
        assert_eq!(got, [(6, "2026-10-27T10:00:00+00:00".to_string(), NOW.to_string())]);
        // An open session nothing follows still stops at now.
        assert!(cut(&[session(7, "2026-10-27T18:00:00+00:00", None, None)]).is_empty());
    }

    fn page(verdict: &str) -> JudgedPage {
        JudgedPage { verdict: verdict.into(), ..Default::default() }
    }

    fn tally(aligned: f64, off: f64, unjudged: f64, tasks: &[(&str, f64)], pages: &[&str]) -> Tally {
        Tally {
            pages: pages.iter().map(|v| page(v)).collect(),
            tasks: tasks
                .iter()
                .enumerate()
                .map(|(i, (title, s))| TaskTime { task_id: i as i64, title: title.to_string(), seconds: *s })
                .collect(),
            aligned,
            off,
            unjudged,
            models: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn the_day_adds_up_with_time_outside_sessions_unjudged() {
        let whole = Observed { active_seconds: 4000.0, afk_seconds: 900.0, ..Default::default() };
        let t = combine(
            &whole,
            &[
                tally(1000.0, 500.0, 100.0, &[("Ship it", 1000.0), ("Docs", 0.0)], &["excused", "no_task", "unjudged"]),
                // The same task carried into the next session: one row.
                tally(800.0, 0.0, 600.0, &[("Ship it", 300.0), ("Write ch. 3", 500.0)], &["excused", "unjudged", "unjudged"]),
            ],
            0.7,
        );
        assert_eq!((t.active_seconds, t.afk_seconds), (4000.0, 900.0));
        assert_eq!((t.aligned_seconds, t.off_seconds), (1800.0, 500.0));
        assert_eq!(t.outside_seconds, 1000.0);
        assert_eq!(t.unjudged_seconds, 1700.0);
        assert_eq!(t.aligned_seconds + t.off_seconds + t.unjudged_seconds, t.active_seconds);
        let tasks: Vec<(&str, f64)> = t.tasks.iter().map(|k| (k.title.as_str(), k.seconds)).collect();
        assert_eq!(tasks, [("Ship it", 1300.0), ("Write ch. 3", 500.0)]);
        assert_eq!(t.askable, 3);
        assert_eq!(t.threshold, 0.7);
    }

    #[test]
    fn askable_skips_taskless_slices_and_stops_at_a_checks_worth() {
        let whole = Observed { active_seconds: 100.0, ..Default::default() };
        let many = vec!["unjudged"; check::MAX_PAGES + 5];
        let t = combine(
            &whole,
            &[tally(0.0, 0.0, 50.0, &[], &["unjudged", "unjudged"]), tally(0.0, 0.0, 50.0, &[("A", 0.0)], &many)],
            0.7,
        );
        assert_eq!(t.askable, check::MAX_PAGES);
        let t = combine(&whole, &[tally(0.0, 0.0, 100.0, &[], &["unjudged"])], 0.7);
        assert_eq!(t.askable, 0);
        // Rounding between the day and its slices never goes negative.
        let t = combine(&Observed { active_seconds: 99.999, ..Default::default() }, &[tally(0.0, 0.0, 100.0, &[], &[])], 0.7);
        assert_eq!(t.outside_seconds, 0.0);
    }
}
