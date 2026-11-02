//! One day, as the Dashboard shows it -- today so far, or a past day whole
//! (day.rs says where days start): how long was active, how long away, and
//! how much of the active time was on pages some task needed.
//!
//! "Relevant" is the at-risk timer's and the checks' own rule, through their
//! own code: every page goes through check.rs `pages`/`tally` and the
//! relevance cache, so the three can never disagree about a page. A day can
//! span several sessions. Today, every page in any of them is judged against
//! the open session's Doing column, the timer's own task set -- today
//! measured against what you are doing now. A past day has no "now": each
//! session's pages are judged against that session's Doing column as it was
//! left (closed sessions' rows are history), which is what the timer judged
//! them against at the time. Either way the tasks finished that day join
//! in, so an assignment done in the morning keeps the time it took. A
//! finished task only claims pages its cached verdicts give it; it is never
//! waited for, so a page judged against Doing alone stays judged. A relevant
//! page is the time of the likeliest task that clears the bar. While nothing
//! is in Doing and no finished task claims a page, it is off task, as the
//! timer shows it. Time in no session is not judged.
//!
//! Reading is free: the cache only. `judge` asks Jev about the busiest pages
//! still unjudged, a check's worth at most, and is what the Dashboard's
//! button calls.
//!
//! The not-relevant time is also told by where it went (`culprits`): every
//! title of a page judged off task counts under what subject.rs says it is
//! part of -- its site, its tool and project, its app. The ignore menu is
//! offered every app and site seen that day (`choices`).

use crate::check::{self, Pages, Tally};
use crate::error::Result;
use crate::folder::Lookup;
use crate::ignore::{self, Kind, Rule};
use crate::models::{Culprit, DayTime, IgnoreChoice, Observed, Session, Task, TaskTime};
use crate::relevance::{self, Page};
use crate::state::AppState;
use crate::{day, db, desktop, folder, observed, screen_time, subject};
use chrono::{DateTime, Local, NaiveDate, Utc};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Manager};

/// How many sessions are read to find a day's. A day holds a handful.
const SESSIONS_READ: i64 = 20;

/// One session's share of a day.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub session_id: i64,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Each session's part of [day_start, until], in time order: `until` is now
/// for today, the next day's start for a past day. A session ends at its
/// `ended_at`, an open one at `until`, and none runs past the next one's
/// start. A closed session whose end was never known (recovered before its
/// first heartbeat) claims nothing: its time is outside a session rather
/// than guessed onto its tasks.
pub fn slices(sessions: &[Session], day_start: DateTime<Utc>, until: DateTime<Utc>) -> Vec<Slice> {
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
            None if s.close_reason.is_none() => until,
            None => *started,
        };
        let end = timed.get(i + 1).map_or(end, |(next, _)| end.min(*next)).min(until);
        let start = (*started).max(day_start);
        if end > start {
            out.push(Slice { session_id: s.id, start, end });
        }
    }
    out
}

/// What the day's slices add up to. `whole` is the day as one window: its
/// active time less the slices' is time in no session.
pub fn combine(whole: &Observed, tallies: &[Tally], min: f64) -> DayTime {
    let mut t = DayTime {
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
    // Left in board order (Doing, then finished that day), not sorted by time:
    // the Dashboard colours each task by its place here, and a colour that
    // moved whenever two tasks swapped totals would mean nothing.
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

/// One slice ready to be judged: its pages, and the tasks that may excuse
/// them, and every title they came from.
struct Part {
    obs: Observed,
    pages: Pages,
    tasks: Vec<Task>,
}

/// What a moment of not-relevant time is counted under.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A tab on a site the browser's history knows: "linkedin.com".
    Site(String),
    /// A tab the history does not know, named from its title: "LinkedIn".
    Title(String),
    /// Anything else, by subject.rs `source`: "Claude Code: Pager".
    Named(String),
}

/// The culprits chart's rows: time summed by source, busiest first. A tab
/// named only by its title joins the site seen today whose name it is
/// ("LinkedIn" is linkedin.com's), when just one site has that name.
pub fn culprits(spans: Vec<(Source, f64)>) -> Vec<Culprit> {
    let letters = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    // A site's name -> the site, or None when two sites share it
    // (docs.google.com and mail.google.com are both "google").
    let mut named: HashMap<String, Option<String>> = HashMap::new();
    for (source, _) in &spans {
        if let Source::Site(site) = source {
            let entry = named.entry(letters(&screen_time::keyword(site))).or_insert_with(|| Some(site.clone()));
            if entry.as_deref() != Some(site.as_str()) {
                *entry = None;
            }
        }
    }
    let mut rows: Vec<Culprit> = Vec::new();
    for (source, seconds) in spans {
        let name = match source {
            Source::Site(site) | Source::Named(site) => site,
            Source::Title(title) => named.get(&letters(&title)).cloned().flatten().unwrap_or(title),
        };
        match rows.iter_mut().find(|r| r.name == name) {
            Some(r) => r.seconds += seconds,
            None => rows.push(Culprit { name, seconds }),
        }
    }
    rows.retain(|r| r.seconds > 0.0);
    rows.sort_by(|a, b| b.seconds.total_cmp(&a.seconds).then_with(|| a.name.cmp(&b.name)));
    rows
}

/// Every title of the parts' pages judged off task, by what it is part of.
/// Titles, not pages: a page is a verdict's unit, and one verdict can cover
/// a project's Claude Code sessions and its shell, which the chart tells
/// apart.
fn off_task_spans(parts: &[Part], tallies: &[Tally]) -> Vec<(Source, f64)> {
    let mut spans = Vec::new();
    for (part, tally) in parts.iter().zip(tallies) {
        // Keyed exactly as pages() keyed them, from the same strings.
        let off: HashSet<(String, String)> = tally
            .pages
            .iter()
            .filter(|p| p.verdict == "no_task")
            .map(|p| relevance::page_key(&p.app, &p.title))
            .collect();
        for (app, titles) in &part.obs.per_title {
            for (title, secs) in titles {
                let Some(s) = subject::of(app, title) else { continue };
                if !off.contains(&s.key) {
                    continue;
                }
                let source = match (s.kind, s.site) {
                    (_, Some(site)) => Source::Site(site),
                    ("browser tab", None) => Source::Title(s.source),
                    _ => Source::Named(s.source),
                };
                spans.push((source, *secs));
            }
        }
    }
    spans
}

/// Every app and site in `obs`, busiest first, as the ignore menu offers
/// them: an app by its installed name and window classes, a site by the
/// host the browser's history gives its tabs. Asked of the machine without
/// the ignore list, so what is ignored is still offered, marked.
fn choices(obs: &Observed, look: &dyn Lookup, apps: &desktop::Installed, ignored: &[Rule]) -> Vec<IgnoreChoice> {
    let mut out: Vec<IgnoreChoice> = Vec::new();
    let mut add = |kind: Kind, value: String, name: String, classes: Vec<String>, seconds: f64, ignored: bool| {
        match out.iter_mut().find(|c| c.kind == kind && c.value == value) {
            Some(c) => c.seconds += seconds,
            None => out.push(IgnoreChoice { kind, value, name, classes, seconds, ignored }),
        }
    };
    for (app, titles) in &obs.per_title {
        let class = app.trim().to_lowercase();
        if class.is_empty() || class == "unknown" || subject::NEVER.contains(&class.as_str()) {
            continue;
        }
        let total: f64 = titles.values().sum();
        let (value, name, classes) = match apps.of_class(&class) {
            Some(installed) => {
                let mut classes = installed.classes;
                if !classes.contains(&class) {
                    classes.push(class.clone());
                }
                (installed.id.to_lowercase(), installed.name, classes)
            }
            None => (class.clone(), subject::short_name(app).to_string(), vec![class.clone()]),
        };
        add(Kind::App, value, name, classes, total, ignore::hides_app(ignored, &class));
        if !screen_time::is_browser(app) {
            continue;
        }
        for (title, secs) in titles {
            if let Some(site) = subject::of_in(app, title, look).and_then(|s| s.site) {
                let hidden = ignore::hides_site(ignored, &site);
                add(Kind::Site, site.clone(), site, Vec::new(), *secs, hidden);
            }
        }
    }
    out.sort_by(|a, b| b.seconds.total_cmp(&a.seconds).then_with(|| a.name.cmp(&b.name)));
    out
}

/// What a slice's pages are judged against: Doing in board order, then the
/// tasks finished that day, oldest first, and which of them are the finished
/// ones (claim, never waited for). A finished task whose title and notes
/// match a Doing one would share its cache rows, so it is left out.
fn claimants(doing: Vec<Task>, done: Vec<Task>) -> (Vec<Task>, HashSet<i64>) {
    let text = |t: &Task| relevance::task_text(t);
    let current: HashSet<String> = doing.iter().map(text).collect();
    let mut tasks = doing;
    let mut optional = HashSet::new();
    for t in done {
        if !current.contains(&text(&t)) && optional.insert(t.id) {
            tasks.push(t);
        }
    }
    (tasks, optional)
}

/// Every part tallied from the cache alone.
fn tallies(conn: &Connection, parts: &[Part], optional: &HashSet<i64>, min: f64) -> Result<Vec<Tally>> {
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
            Ok(check::tally_with(&part.pages, &judgements, &part.tasks, min, optional))
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

/// One day: today so far (`date` None or today) or a past day whole. One
/// ActivityWatch fetch; every slice is cut from it.
pub async fn day_time(app: &AppHandle, date: Option<NaiveDate>, judge: bool) -> Result<DayTime> {
    let now = Utc::now();
    let hour = day::hour();
    let today = day::of(Local::now(), hour);
    let date = date.unwrap_or(today);
    let (day_start, until) = day::window(date, today, now, hour)?;
    let (start_iso, until_iso) = (screen_time::iso(day_start), screen_time::iso(until));
    let events = observed::fetch(&start_iso, &until_iso).await?;
    // Every title: the ignore menu offers each site seen that day.
    let whole = observed::summarize_with(&events, &start_iso, &until_iso, usize::MAX)?;

    let state = app.state::<AppState>();
    let (cut, optional, min, judged) = {
        let conn = state.conn.lock().unwrap();
        let sessions = db::sessions_started_before(&conn, &until_iso, SESSIONS_READ)?;
        let done = db::done_tasks_between(&conn, &start_iso, &until_iso)?;
        let mut optional = HashSet::new();
        let mut cut = Vec::new();
        // Today: every slice against the Doing column of now. A past day:
        // each session against its own, as it was left.
        let doing_now = if date == today { Some(relevance::timer_tasks(&conn, state.session_id())?) } else { None };
        for s in slices(&sessions, day_start, until) {
            let doing = match &doing_now {
                Some(tasks) => tasks.clone(),
                None => db::doing_session_tasks(&conn, s.session_id)?,
            };
            // Finished tasks are the only optional ones, so one set serves
            // every slice.
            let (tasks, finished) = claimants(doing, done.clone());
            optional.extend(finished);
            cut.push((s, tasks));
        }
        (cut, optional, db::relevance_min(&conn), db::relevance_count(&conn)?)
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
        parts.push(Part { pages: check::pages(&obs, usize::MAX), obs, tasks });
    }

    let mut error = None;
    if judge {
        let first = {
            let conn = state.conn.lock().unwrap();
            tallies(&conn, &parts, &optional, min)?
        };
        let judged = relevance::judge_many(app, to_ask(&parts, &first)).await;
        error = judged.iter().flatten().find_map(|j| j.error.clone());
    }
    let last = {
        let conn = state.conn.lock().unwrap();
        tallies(&conn, &parts, &optional, min)?
    };
    // Machine lookups (folders, browser history, desktop files): off the
    // async runtime's threads.
    let named = tauri::async_runtime::spawn_blocking(move || {
        let culprits = culprits(off_task_spans(&parts, &last));
        let seen = choices(&whole, folder::machine(), desktop::machine(), &ignore::current());
        (combine(&whole, &last, min), culprits, seen)
    })
    .await
    .map_err(|e| crate::error::AppError::Other(e.to_string()))?;
    let (t, culprits, seen) = named;
    Ok(DayTime {
        day: date.to_string(),
        today: today.to_string(),
        start_hour: hour,
        start: start_iso,
        end: until_iso,
        error,
        culprits,
        seen,
        judged,
        ..t
    })
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
    fn a_past_day_ends_at_the_next_days_start() {
        // A day from Oct 26 04:00 to Oct 27 04:00 (UTC stamps).
        let (start, end) = (at("2026-10-26T04:00:00+00:00"), at(MIDNIGHT));
        let got: Vec<_> = slices(
            &[
                // Begun the next day: not this one's.
                session(4, "2026-10-27T09:00:00+00:00", None, None),
                // Still open, begun late that evening: up to the day's end.
                session(3, "2026-10-27T01:00:00+00:00", None, None),
                // Ran past the day's end into the next: cut at the next session.
                session(2, "2026-10-26T20:00:00+00:00", Some("2026-10-27T02:00:00+00:00"), Some("debrief")),
            ],
            start,
            end,
        )
        .into_iter()
        .map(|s| (s.session_id, screen_time::iso(s.start), screen_time::iso(s.end)))
        .collect();
        assert_eq!(
            got,
            [
                (2, "2026-10-26T20:00:00+00:00".to_string(), "2026-10-27T01:00:00+00:00".to_string()),
                (3, "2026-10-27T01:00:00+00:00".to_string(), MIDNIGHT.to_string()),
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
    fn tasks_keep_board_order_not_time_order() {
        let whole = Observed { active_seconds: 1000.0, ..Default::default() };
        let t = combine(
            &whole,
            &[
                tally(900.0, 0.0, 0.0, &[("Small", 100.0), ("Big", 800.0)], &["excused"]),
                tally(300.0, 0.0, 0.0, &[("Small", 0.0), ("Big", 0.0), ("Finished", 300.0)], &["excused"]),
            ],
            0.7,
        );
        let tasks: Vec<(&str, f64)> = t.tasks.iter().map(|k| (k.title.as_str(), k.seconds)).collect();
        assert_eq!(tasks, [("Small", 100.0), ("Big", 800.0), ("Finished", 300.0)]);
    }

    fn task(id: i64, title: &str, notes: &str, status: &str) -> Task {
        Task {
            id,
            session_id: Some(1),
            title: title.into(),
            position: id,
            status: status.into(),
            carry_count: 0,
            notes: notes.into(),
            due_date: None,
            priority: "medium".into(),
            labels: Vec::new(),
        }
    }

    #[test]
    fn finished_tasks_follow_doing_and_only_they_are_optional() {
        let doing = vec![task(1, "Ship it", "", "doing"), task(2, "Essay", "draft", "doing")];
        let done = vec![
            task(3, "Problem set 4", "", "done"),
            // Same title and notes as a Doing card (a duplicate card): its
            // verdicts would be the Doing card's own, so it is left out.
            task(4, "Essay", "draft", "done"),
            // Same title, other notes: a different question to Jev.
            task(5, "Ship it", "v1", "done"),
        ];
        let (tasks, optional) = claimants(doing, done);
        assert_eq!(tasks.iter().map(|t| t.id).collect::<Vec<_>>(), [1, 2, 3, 5]);
        assert_eq!(optional, HashSet::from([3, 5]));
        let (tasks, optional) = claimants(Vec::new(), vec![task(3, "Problem set 4", "", "done")]);
        assert_eq!((tasks.len(), optional.len()), (1, 1));
    }

    fn row(name: &str, seconds: f64) -> Culprit {
        Culprit { name: name.into(), seconds }
    }

    #[test]
    fn culprits_are_summed_by_source_and_a_titles_site_joins_it() {
        let got = culprits(vec![
            (Source::Site("linkedin.com".into()), 300.0),
            (Source::Site("linkedin.com".into()), 120.0),
            // A LinkedIn page the history did not know.
            (Source::Title("LinkedIn".into()), 60.0),
            (Source::Named("Claude Code: Pager".into()), 400.0),
            (Source::Named("Terminal: Pager".into()), 50.0),
            // Two Google sites share a name: a title saying it joins neither.
            (Source::Site("docs.google.com".into()), 30.0),
            (Source::Site("mail.google.com".into()), 20.0),
            (Source::Title("Google".into()), 10.0),
            (Source::Named("Discord".into()), 0.0),
        ]);
        assert_eq!(
            got,
            [
                row("linkedin.com", 480.0),
                row("Claude Code: Pager", 400.0),
                row("Terminal: Pager", 50.0),
                row("docs.google.com", 30.0),
                row("mail.google.com", 20.0),
                row("Google", 10.0),
            ]
        );
        // A name that only looks like a site's stays itself.
        assert_eq!(culprits(vec![(Source::Named("LinkedIn".into()), 5.0), (Source::Site("linkedin.com".into()), 1.0)]).len(), 2);
    }

    /// Today's windows from ActivityWatch by what the culprits chart would
    /// call them (all of today's active time, not only the off-task part:
    /// verdicts live in the store), and the ignore menu's choices. Reads
    /// this machine, sends nothing. `cargo test live_culprits -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_culprits() {
        let now = Utc::now();
        let start = day::today_start();
        let (from, to) = (screen_time::iso(start), screen_time::iso(now));
        let events = tauri::async_runtime::block_on(observed::fetch(&from, &to)).expect("ActivityWatch");
        let obs = observed::summarize_with(&events, &from, &to, usize::MAX).unwrap();
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
        let machine = folder::Machine::at(home.clone());
        let apps = desktop::Installed::new(desktop::dirs(&home));
        let mut spans = Vec::new();
        for (app, titles) in &obs.per_title {
            for (title, secs) in titles {
                match subject::of_in(app, title, &machine) {
                    Some(s) => spans.push((
                        match (s.kind, s.site) {
                            (_, Some(site)) => Source::Site(site),
                            ("browser tab", None) => Source::Title(s.source),
                            _ => Source::Named(s.source),
                        },
                        *secs,
                    )),
                    None => println!("  not judged {secs:>6.0}s  {app} {title:?}"),
                }
            }
        }
        println!("--- sources (all active time today)");
        for c in culprits(spans) {
            println!("{:>6.0}s  {}", c.seconds, c.name);
        }
        println!("--- ignore menu: seen today");
        for c in choices(&obs, &machine, &apps, &[]) {
            println!("{:>6.0}s  {:?} {:30} {:?}", c.seconds, c.kind, c.name, c.classes);
        }
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
