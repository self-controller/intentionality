//! At-risk apps: apps and websites the user flagged. While one is the focused
//! window, a giant timer at the top right of the screen shows today's
//! off-task total -- unless Jev finds the page relevant to a task in Doing
//! (relevance.rs), in which case nothing is shown. The Dashboard shows
//! today's time on each flag.
//!
//! Every other window is judged too: from the cache the moment it comes to
//! the front, by Jev once it has stayed there for ASK. One Jev finds
//! relevant to none of the Doing tasks gets the same clock
//! (subject.rs says what a window is: a tab, an editor's project folder, a
//! terminal's directory). So does every window while nothing is in Doing,
//! no open session included: with no task, nothing is relevant. Otherwise
//! an unflagged window shows nothing -- no key, Jev unreachable -- so the
//! clock only appears there on a verdict. The app's own window is never
//! judged, so the board stays reachable without a clock.
//!
//! There is one clock, shared by every page: a running total for the day
//! that goes up while the clock is on screen, whichever page it is on, and
//! never down. A new off-task site carries on from it, and so do coming back
//! from being away, a change to Doing and a restart (it is kept in the
//! store's meta). It starts over when a new day starts (day.rs: 4 am unless
//! Settings says otherwise), seeded once from ActivityWatch
//! (`off_task_seconds`) when the day has no total yet. Time without input
//! counts for observed::IDLE_GRACE_SECS before it is away, here as
//! everywhere.
//!
//! ActivityWatch stays the tracker and the source of today's total. Which
//! window is in front comes from the GNOME Shell extension behind
//! overlay.rs, which signals every change the moment it happens; when that
//! extension is missing or too old, AW's newest window event is polled
//! instead, a second or two behind.
//!
//! Websites are matched by window title, not URL -- AW records the app and
//! the title only, and "Feed | LinkedIn — Zen Browser" names its site.
//!
//! Time the clock spends saying "Not relevant to any task" is also counted
//! towards an early check (AppState::add_drift, scheduler.rs).

use crate::aw::{self, AwEvent};
use crate::error::{AppError, Result};
use crate::models::{Judgement, RiskApp, RiskToday, RiskTotal};
use crate::observed::{self, Events};
use crate::relevance::{self, Page};
use crate::{day, db, overlay, state::AppState, subject};
use chrono::{DateTime, NaiveDate, Utc};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// The keepalive pace: a Show a second keeps the clock up and its count
/// honest. Changes of window do not wait for it, and nor does a window
/// crossing SETTLE or ASK (`wake_in`).
const TICK: Duration = Duration::from_secs(1);
/// AW unreachable: nothing to show, check back slowly.
const IDLE_TICK: Duration = Duration::from_secs(5);
/// How long a window nobody flagged must stay in front before its clock
/// comes up from hidden: passing through windows, or a title that holds only
/// while a page loads, flashes no clock. A clock already up moves to the
/// next off-task window at once (nothing to flash), and a flagged window
/// shows at once, as it always did.
const SETTLE: Duration = Duration::from_millis(400);
/// How long a window nobody flagged must stay in front before Jev is asked
/// about it: flicking past windows bills nothing. Its cached verdicts are
/// read the moment it is in front, so a page judged before never waits on
/// this. A flagged window is asked at once.
const ASK: Duration = Duration::from_millis(400);
/// Slack on a wake set for SETTLE or ASK, so the pass it starts is past it.
const WAKE_SLACK: Duration = Duration::from_millis(2);
/// How often the day's total is written to the store while the clock is up
/// (and once more as it goes down): a crash loses at most this much.
const SAVE: Duration = Duration::from_secs(30);
/// The newest window event must have been extended this recently to describe
/// the window focused now. awatcher's heartbeats extend it every second; one
/// that has stopped (screen locked, watcher dead) is not "now".
const FRESH_SECS: i64 = 10;
/// A page's judgement is read from the store again this often while the
/// page stays in front: a task edited or an appeal won shows up without a
/// request (the appeal also pokes the loop directly).
const REJUDGE: Duration = Duration::from_secs(5);
/// After a judgement that could not reach the model, wait this long before
/// trying that page again -- not every tick.
const RETRY: Duration = Duration::from_secs(30);
/// While polling AW for the focused window, look this often for the shell
/// extension to have appeared or been updated.
const REPROBE: Duration = Duration::from_secs(30);
/// The most one pass of the loop can add to the drift count. Passes come at
/// least every TICK while a clock is up; anything longer was a suspend or a
/// stall, and was not time spent looking at the page.
const DRIFT_STEP_CAP: Duration = Duration::from_secs(2);

/// Second-level labels that come before the name in a domain: bbc.co.uk.
const SECOND_LEVELS: &[&str] = &["co", "com", "org", "net", "ac", "gov", "edu"];

/// Apps whose window title names the website they show. Matched as words of
/// the app id ("app.zen_browser.zen", "org.mozilla.firefox", "google-chrome").
const BROWSERS: &[&str] = &[
    "zen", "firefox", "librewolf", "floorp", "waterfox", "chromium", "chrome", "brave",
    "vivaldi", "epiphany", "opera", "msedge", "microsoft-edge",
];

fn is_domain(s: &str) -> bool {
    let labels: Vec<&str> = s.split('.').collect();
    labels.len() >= 2
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && labels.last().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(char::is_alphabetic))
}

/// The website a text names, lowercased and without "www.":
/// "https://www.bbc.co.uk/news" -> "bbc.co.uk". None when it names none.
pub(crate) fn host(name: &str) -> Option<String> {
    let lowered = name.trim().to_lowercase();
    let mut rest = lowered.as_str();
    for scheme in ["https://", "http://"] {
        rest = rest.strip_prefix(scheme).unwrap_or(rest);
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    is_domain(host).then(|| host.to_string())
}

/// What a flag is looked for by: a website's name ("linkedin.com" ->
/// "linkedin", "https://www.bbc.co.uk/news" -> "bbc"), or the text itself,
/// lowercased ("Discord" -> "discord"). Empty when nothing is left to match.
pub fn keyword(name: &str) -> String {
    let Some(host) = host(name) else { return name.trim().to_lowercase() };
    let labels: Vec<&str> = host.split('.').collect();
    let mut name_at = labels.len() - 2;
    if name_at > 0 && SECOND_LEVELS.contains(&labels[name_at]) {
        name_at -= 1;
    }
    labels[name_at].to_string()
}

/// `needle` inside `hay` as a whole word: not run into letters or digits on
/// either side. Case-insensitive. Whole words are what keep a short flag
/// honest: x.com's "x" must not match xdg-desktop-portal-gtk.
pub(crate) fn contains_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let hay = hay.to_lowercase();
    hay.match_indices(needle).any(|(at, _)| {
        let before = hay[..at].chars().next_back();
        let after = hay[at + needle.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

pub(crate) fn is_browser(app: &str) -> bool {
    BROWSERS.iter().any(|b| contains_word(app, b))
}

/// How a window matched a flag. A title hit is the more specific one: with
/// both "linkedin.com" and "zen" flagged, a LinkedIn tab is LinkedIn time.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Hit {
    Title,
    App,
}

/// Does a window (AW's `app` and `title`) belong to the flag with this
/// keyword? Its app id names it, or it is a browser and its title does.
///
/// The app id is asked first: a browser's titles end in its own name
/// ("… — Zen Browser"), so a flag on the browser itself would otherwise
/// read as a title hit on every tab and outrank the site actually open.
fn hit(keyword: &str, app: &str, title: &str) -> Option<Hit> {
    if contains_word(app, keyword) {
        Some(Hit::App)
    } else if is_browser(app) && contains_word(title, keyword) {
        Some(Hit::Title)
    } else {
        None
    }
}

/// The flag a window belongs to, if any. `apps` is in flag order (oldest
/// first), which breaks ties between equally good hits.
pub fn current<'a>(apps: &'a [RiskApp], app: &str, title: &str) -> Option<&'a RiskApp> {
    apps.iter()
        .filter_map(|flag| hit(&keyword(&flag.name), app, title).map(|h| (h, flag)))
        .min_by_key(|(h, _)| *h) // min_by_key keeps the first of equals: flag order
        .map(|(_, flag)| flag)
}

/// AW's newest window event as (app, title), if it still describes the
/// window focused now: the watcher must have extended it within FRESH_SECS.
pub fn fresh_window(window: Option<&AwEvent>, now: DateTime<Utc>) -> Option<(String, String)> {
    let window = window?;
    let end = observed::parse_ts(&window.timestamp).ok()? + observed::dur(window);
    if end < now - chrono::Duration::seconds(FRESH_SECS) {
        return None;
    }
    let field = |k: &str| window.data.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some((field("app"), field("title")))
}

/// Is the user away now? AW's AFK event is dated from the last input, and
/// the first IDLE_GRACE_SECS of it still count as present (observed.rs) --
/// the clock stays up while someone reads or watches.
fn is_away(afk: Option<&AwEvent>, now: DateTime<Utc>) -> bool {
    afk.is_some_and(|ev| {
        ev.data.get("status").and_then(|v| v.as_str()) == Some("afk")
            && observed::parse_ts(&ev.timestamp)
                .is_ok_and(|idle| now - idle >= chrono::Duration::seconds(observed::IDLE_GRACE_SECS))
    })
}

/// Each flag's active time in [start, end], plus the time that matched any
/// flag at all -- counted once, so a moment two flags share is not doubled.
pub fn totals(
    events: &Events,
    apps: &[RiskApp],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<RiskToday> {
    let keywords: Vec<String> = apps.iter().map(|a| keyword(&a.name)).collect();
    let mut seconds = vec![0.0; apps.len()];
    let mut total = 0.0;
    for a in observed::active(events, start, end)? {
        let mut any = false;
        for (i, k) in keywords.iter().enumerate() {
            if hit(k, a.app, a.title).is_some() {
                seconds[i] += a.seconds;
                any = true;
            }
        }
        if any {
            total += a.seconds;
        }
    }
    let apps = apps
        .iter()
        .zip(seconds)
        .map(|(a, seconds)| RiskTotal { id: a.id, name: a.name.clone(), seconds })
        .collect();
    Ok(RiskToday { apps, total, judged: 0, error: None })
}

/// The store's one timestamp format (db::now), which is what AW is queried in.
pub(crate) fn iso(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// Today so far on every flag, from ActivityWatch.
pub async fn today(apps: &[RiskApp]) -> Result<RiskToday> {
    let now = Utc::now();
    let start = day::today_start();
    let events = observed::fetch(&iso(start), &iso(now)).await?;
    totals(&events, apps, start, now)
}

/// The off-task time in [start, end]: every moment on a page that would
/// show the clock now. That is a page judged against every Doing task and
/// excused by none, any page at all while nothing is in Doing, or a flagged
/// page no Doing task excuses (its "Relevance unchecked" clock too). `verdict` gives
/// a page's judgement from the cache; each page is asked once. A window
/// nobody judges (subject::of None) counts only when flagged.
pub fn off_task_seconds(
    events: &Events,
    apps: &[RiskApp],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    mut verdict: impl FnMut(&str, &str) -> Result<Judgement>,
) -> Result<f64> {
    // page -> (excused, judged against every task)
    let mut known: HashMap<(String, String), (bool, bool)> = HashMap::new();
    let mut total = 0.0;
    for a in observed::active(events, start, end)? {
        let flagged = current(apps, a.app, a.title).is_some();
        if !flagged && subject::of(a.app, a.title).is_none() {
            continue;
        }
        let key = relevance::page_key(a.app, a.title);
        let (excused, judged) = match known.get(&key) {
            Some(k) => *k,
            None => {
                let j = verdict(a.app, a.title)?;
                // Nothing in Doing counts as judged: every page is off task.
                let k = (j.relevant.is_some(), j.complete());
                known.insert(key, k);
                k
            }
        };
        if !excused && (flagged || judged) {
            total += a.seconds;
        }
    }
    Ok(total)
}

/// Today's off-task time so far, from ActivityWatch and the verdict cache:
/// free, never a model call. Only seeds a day with no total: it judges the
/// whole day against the Doing tasks of now, so a page judged only against
/// earlier ones drops out of it.
async fn day_off_task(app: &AppHandle, apps: &[RiskApp]) -> Result<f64> {
    let now = Utc::now();
    let start = day::today_start();
    let events = observed::fetch(&iso(start), &iso(now)).await?;
    let state = app.state::<AppState>();
    let conn = state.conn.lock().unwrap();
    let tasks = relevance::timer_tasks(&conn, state.session_id())?;
    off_task_seconds(&events, apps, start, now, |app, title| {
        let page = Page { app: app.into(), title: title.into() };
        relevance::cached_for(&conn, tasks.clone(), &page)
    })
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(run(app.clone()));
    tauri::async_runtime::spawn(relay_appeals(app));
}

/// The day's off-task total, as the clock shows it.
struct Ledger {
    day: NaiveDate,
    seconds: f64,
    /// When it was last written to the store.
    saved: Instant,
}

/// Where the focused window is learned from.
enum Source {
    /// The shell extension, as it happens.
    Shell(overlay::FocusStream),
    /// ActivityWatch's newest event, a second or two behind. `since` paces
    /// the look for a (newer) extension.
    Aw { since: Instant },
}

/// Ask the shell for the window in front and a stream of changes. A v1
/// extension has neither method and lands on AW.
async fn probe() -> (Source, Option<(String, String)>) {
    match overlay::focused().await {
        Ok(focus) => match overlay::focus_stream().await {
            Ok(stream) => (Source::Shell(stream), Some(focus)),
            Err(_) => (Source::Aw { since: Instant::now() }, None),
        },
        Err(_) => (Source::Aw { since: Instant::now() }, None),
    }
}

enum FocusEvent {
    Changed((String, String)),
    /// The stream ended: gnome-shell went away or the extension was disabled.
    Lost,
}

async fn next_focus(source: &mut Source) -> FocusEvent {
    match source {
        Source::Shell(stream) => match stream.next().await {
            Some(msg) => match msg.args() {
                Ok(a) => FocusEvent::Changed((a.app().to_string(), a.title().to_string())),
                Err(_) => FocusEvent::Changed((String::new(), String::new())),
            },
            None => FocusEvent::Lost,
        },
        Source::Aw { .. } => std::future::pending().await,
    }
}

/// A judged window in front: what the clock calls it, the flag it matched
/// (None: nobody flagged it), what it is cached under, and the window.
struct Seen {
    name: String,
    flag: Option<i64>,
    key: (String, String),
    page: Page,
}

struct Watch {
    /// (window bucket, AFK bucket). Found once, and again after any error,
    /// since a watcher restart can name a new bucket.
    buckets: Option<(String, Option<String>)>,
    ledger: Option<Ledger>,
    /// The focused window as the shell last reported it.
    focus: Option<(String, String)>,
}

impl Watch {
    /// The window in front, if it is one to judge and the user is here.
    async fn look(&mut self, apps: &[RiskApp], shell: bool) -> Result<Option<Seen>> {
        if self.buckets.is_none() {
            let (window, afk) = aw::find_buckets().await?;
            let window =
                window.ok_or_else(|| AppError::AwUnavailable("no window-watcher bucket".into()))?;
            self.buckets = Some((window, afk));
        }
        let (window_bucket, afk_bucket) = self.buckets.as_ref().expect("just found");
        let focus = if shell {
            self.focus.clone()
        } else {
            let window = aw::latest(window_bucket).await?;
            fresh_window(window.as_ref(), Utc::now())
        };
        let afk = match afk_bucket {
            Some(bucket) => aw::latest(bucket).await?,
            None => None,
        };
        let Some((app, title)) = focus.filter(|_| !is_away(afk.as_ref(), Utc::now())) else {
            return Ok(None);
        };
        let flag = current(apps, &app, &title);
        // A flagged window is judged even where nothing else would be.
        let name = match (flag, subject::of(&app, &title)) {
            (Some(flag), _) => flag.name.clone(),
            (None, Some(subject)) => subject.label(),
            (None, None) => return Ok(None),
        };
        Ok(Some(Seen {
            name,
            flag: flag.map(|f| f.id),
            key: relevance::page_key(&app, &title),
            page: Page { app, title },
        }))
    }

    /// Today's off-task total for the clock, whichever page is in front:
    /// the store's, or a seed from ActivityWatch for a day without one.
    async fn count(&mut self, app: &AppHandle, apps: &[RiskApp]) -> Result<f64> {
        let day = day::today();
        if self.ledger.as_ref().is_none_or(|l| l.day != day) {
            let stored = {
                let state = app.state::<AppState>();
                let conn = state.conn.lock().unwrap();
                db::off_task_today(&conn, day)
            };
            let seconds = match stored {
                Some(seconds) => seconds,
                None => day_off_task(app, apps).await?,
            };
            self.ledger = Some(Ledger { day, seconds, saved: Instant::now() });
            if stored.is_none() {
                self.save(app);
            }
        }
        Ok(self.ledger.as_ref().expect("just set").seconds)
    }

    /// Time the clock was on screen.
    fn add(&mut self, step: Duration) {
        if let Some(ledger) = &mut self.ledger {
            ledger.seconds += step.as_secs_f64();
        }
    }

    fn save(&mut self, app: &AppHandle) {
        let Some(ledger) = &mut self.ledger else { return };
        ledger.saved = Instant::now();
        let state = app.state::<AppState>();
        let conn = state.conn.lock().unwrap();
        if let Err(err) = db::set_off_task_today(&conn, ledger.day, ledger.seconds) {
            eprintln!("intentionality: cannot keep today's off-task total: {err}");
        }
    }
}

/// May the clock show for a window that has been in front this long? `up`:
/// a clock was on screen a pass ago, so this one only relabels it.
fn may_show(flagged: bool, up: bool, in_front: Duration) -> bool {
    flagged || up || in_front >= SETTLE
}

/// Has a window been in front long enough for Jev to be asked about it?
fn may_ask(flagged: bool, in_front: Duration) -> bool {
    flagged || in_front >= ASK
}

/// When the loop must look again for a window in front this long: the
/// moment it crosses SETTLE or ASK, not the next keepalive tick. None once
/// it is past both.
fn wake_in(in_front: Duration) -> Option<Duration> {
    [SETTLE, ASK]
        .into_iter()
        .filter(|at| *at > in_front)
        .min()
        .map(|at| (at - in_front + WAKE_SLACK).min(TICK))
}

/// Logs the task that excuses a page, once per page and task: a page read
/// again every REJUDGE says it once.
fn note_excused(
    excused: &mut Option<((String, String), i64)>,
    key: &(String, String),
    label: &str,
    judgement: &Judgement,
) {
    let Some(v) = judgement.relevant.map(|i| &judgement.verdicts[i]) else { return };
    let said = (key.clone(), v.task_id);
    if excused.as_ref() != Some(&said) {
        eprintln!(
            "intentionality: at risk: {label} excused by task \"{}\" (p={:.2})",
            v.task_title,
            v.probability.unwrap_or(0.0)
        );
        *excused = Some(said);
    }
}

/// A judgement in flight for one page.
struct Pending {
    key: (String, String),
    label: String,
    handle: tokio::task::JoinHandle<Judgement>,
}

async fn finished(
    pending: &mut Option<Pending>,
) -> std::result::Result<Judgement, tokio::task::JoinError> {
    match pending {
        Some(p) => (&mut p.handle).await,
        None => std::future::pending().await,
    }
}

/// The last judgement that landed, for the page it was about.
struct Decided {
    key: (String, String),
    judgement: Judgement,
    at: Instant,
}

/// What goes on screen for a judged page, given its judgement. None =
/// nothing on screen: the page is excused, or nobody flagged it and the
/// verdict is anything but "relevant to none of the Doing tasks".
#[derive(Debug, Clone, PartialEq)]
struct Display {
    label: String,
    caption: String,
    appealable: bool,
    /// A flagged page judged against every Doing task and excused by none:
    /// the time that counts towards an early check. An unflagged page's clock
    /// is drift too, but the early check is about at-risk time; the next
    /// scheduled check scores it.
    unexcused: bool,
}

/// The caption of a page judged against every Doing task and relevant to
/// none. The extension draws it in red, so it compares the text: its
/// extension.js declares the same string (tested below).
pub const OFF_TASK_CAPTION: &str = "Not relevant to any task";

fn display(label: &str, flagged: bool, j: &Judgement) -> Option<Display> {
    if j.relevant.is_some() {
        return None;
    }
    let unjudged = j.verdicts.iter().any(|v| v.probability.is_none());
    // Nothing in Doing (no verdicts, none unjudged) shows on every window,
    // flagged or not: with no task, nothing is relevant.
    if !flagged && unjudged {
        return None;
    }
    let caption = if j.no_tasks {
        "Nothing in Doing"
    } else if unjudged {
        "Relevance unchecked"
    } else {
        OFF_TASK_CAPTION
    };
    Some(Display {
        label: label.into(),
        caption: caption.into(),
        appealable: !j.no_tasks,
        unexcused: flagged && !j.no_tasks && !unjudged,
    })
}

/// The timer loop. Runs for the life of the app, session or not -- an
/// at-risk app is at risk either way. Wakes on a change of focused window
/// from the shell, on the keepalive tick, when a judgement lands, and when
/// an appeal changes the store.
///
/// The overlay hides itself a few seconds after the last Show, so a stalled
/// loop, a dead app or a suspend can never leave a stale clock on screen;
/// Hide here only makes leaving the app prompt.
async fn run(app: AppHandle) {
    // At autostart this reaches the journal, like the build and resume-gate
    // lines in main: "was the timer able to show back then?" stays answerable.
    for warning in [overlay::warning().await, overlay::window_tracking_warning().await]
        .into_iter()
        .flatten()
    {
        eprintln!("intentionality: {warning}");
    }
    // Done now, not while the first page Jev is asked about waits: the
    // won appeals sent with every question are named once (that read the
    // machine and held a rig's first ask back 0.24 s), and the shared
    // client sets up its TLS.
    let warm = app.clone();
    tauri::async_runtime::spawn_blocking(move || relevance::warm_precedents(&warm));
    let _ = crate::http::remote();
    let (mut source, focus) = probe().await;
    eprintln!(
        "intentionality: focused window from {}",
        if matches!(source, Source::Shell(_)) { "the shell" } else { "ActivityWatch" }
    );
    let state = app.state::<AppState>();
    let mut watch = Watch { buckets: None, ledger: None, focus };
    // The page in front and since when, for SETTLE and ASK.
    let mut in_front: Option<((String, String), Instant)> = None;
    // The last "excused by" line, so a page re-read every REJUDGE logs once.
    let mut excused: Option<((String, String), i64)> = None;
    let mut pending: Option<Pending> = None;
    let mut decided: Option<Decided> = None;
    let mut shown: Option<Display> = None;
    let mut overlay_ok = true;
    // Logged once per streak, so AW being down for an hour is one line.
    let mut failing = false;
    let mut wait = IDLE_TICK;
    let mut last_pass = Instant::now();
    loop {
        tokio::select! {
            event = next_focus(&mut source) => match event {
                FocusEvent::Changed(focus) => watch.focus = Some(focus),
                FocusEvent::Lost => {
                    eprintln!("intentionality: lost the shell's focus signal — polling ActivityWatch");
                    source = Source::Aw { since: Instant::now() };
                    watch.focus = None;
                }
            },
            _ = tokio::time::sleep(wait) => {}
            // An appeal was stored: judge the page again now, not in REJUDGE.
            _ = state.relevance_changed.notified() => {
                decided = None;
                if let Some(p) = pending.take() {
                    p.handle.abort();
                }
            }
            result = finished(&mut pending) => {
                let p = pending.take().expect("a judgement finished");
                if let Ok(judgement) = result {
                    note_excused(&mut excused, &p.key, &p.label, &judgement);
                    decided = Some(Decided { key: p.key, judgement, at: Instant::now() });
                }
            }
        }
        if let Source::Aw { since } = &source {
            if since.elapsed() >= REPROBE {
                let (probed, focus) = probe().await;
                if let Source::Shell(_) = probed {
                    eprintln!("intentionality: focused window from the shell");
                    watch.focus = focus;
                }
                source = probed;
            }
        }

        // The pass that just ended had `shown` on screen: if that was an
        // unexcused clock, it was drift.
        let step = last_pass.elapsed().min(DRIFT_STEP_CAP);
        last_pass = Instant::now();
        if shown.is_some() {
            watch.add(step);
        }

        // A store too old for risk_app reads as nothing flagged.
        let (apps, drift_minutes) = {
            let conn = state.conn.lock().unwrap();
            (db::list_risk_apps(&conn).unwrap_or_default(), db::drift_check_minutes(&conn))
        };
        if shown.as_ref().is_some_and(|d| d.unexcused) && state.session_id().is_some() {
            let drift = state.add_drift(step.as_secs_f64());
            if crate::scheduler::drift_due(drift, drift_minutes) {
                state.drift_check.notify_one();
            }
        }
        let seen = watch.look(&apps, matches!(source, Source::Shell(_))).await;
        let mut aw_error = seen.as_ref().err().map(ToString::to_string);
        let seen = seen.unwrap_or(None);
        match &seen {
            Some(seen) if in_front.as_ref().is_some_and(|(k, _)| *k == seen.key) => {}
            Some(seen) => in_front = Some((seen.key.clone(), Instant::now())),
            None => in_front = None,
        }

        // Judge the page in front, from the cache or the model, in its own
        // task: a switch away drops a judgement still in flight.
        let display = match &seen {
            None => {
                if let Some(p) = pending.take() {
                    p.handle.abort();
                }
                None
            }
            Some(seen) => {
                let key = seen.key.clone();
                let flagged = seen.flag.is_some();
                let fresh = decided.as_ref().is_some_and(|d| {
                    let ttl = if d.judgement.error.is_some() { RETRY } else { REJUDGE };
                    d.key == key && d.at.elapsed() < ttl
                });
                let dwelt = in_front.as_ref().map_or(Duration::ZERO, |(_, since)| since.elapsed());
                let stale = !fresh && !pending.as_ref().is_some_and(|p| p.key == key);
                if stale {
                    // Anything still in flight was about the page before.
                    if let Some(p) = pending.take() {
                        p.handle.abort();
                    }
                    // The store first, the moment the page is in front: one
                    // judged before needs no request and no wait for ASK.
                    let known = {
                        let conn = state.conn.lock().unwrap();
                        relevance::cached(&conn, state.session_id(), &seen.page)
                    };
                    match known {
                        Ok(judgement) if judgement.complete() => {
                            note_excused(&mut excused, &key, &seen.name, &judgement);
                            decided = Some(Decided { key: key.clone(), judgement, at: Instant::now() });
                        }
                        // Still passing through: Jev is asked nothing yet. An
                        // incomplete read is not kept -- it would count as
                        // fresh and hold the question back for REJUDGE.
                        _ if !may_ask(flagged, dwelt) => {}
                        _ => {
                            let handle = tokio::spawn(relevance::judge(app.clone(), seen.page.clone()));
                            pending = Some(Pending { key: key.clone(), label: seen.name.clone(), handle });
                        }
                    }
                }
                // Not judged yet: nothing on screen until the verdict. A
                // verdict in hand still waits out SETTLE, unless a clock is
                // already up and only moves to this page -- so passing
                // through windows, or back through one, flashes nothing.
                decided
                    .as_ref()
                    .filter(|d| d.key == key && may_show(flagged, shown.is_some(), dwelt))
                    .and_then(|d| display(&seen.name, flagged, &d.judgement))
            }
        };

        // Today's count, looked up only for a clock that goes up: a relevant
        // page, or one passed through, costs nothing. ActivityWatch is read
        // only to seed a day with no total.
        let seconds = match (&display, &seen) {
            (Some(_), Some(_)) => match watch.count(&app, &apps).await {
                Ok(seconds) => Some(seconds),
                Err(err) => {
                    aw_error.get_or_insert(err.to_string());
                    None
                }
            },
            _ => None,
        };
        if let Some(err) = &aw_error {
            watch.buckets = None;
            if !failing {
                eprintln!("intentionality: at-risk timer cannot read ActivityWatch: {err}");
            }
        }
        failing = aw_error.is_some();
        let display = display.filter(|_| seconds.is_some());

        // Logged on change only, so the journal says what was on screen when,
        // and how long after the page came to the front it got there.
        if display != shown {
            let after = in_front.as_ref().map_or(0.0, |(_, since)| since.elapsed().as_secs_f64());
            match (&display, seconds) {
                (Some(d), Some(seconds)) => eprintln!(
                    "intentionality: at risk: {} ({:.0}s off task today) — {} · {:.2}s after focus",
                    d.label, seconds, d.caption, after
                ),
                _ => eprintln!("intentionality: at risk: none"),
            }
        }
        let sent = match (&display, seconds) {
            (Some(d), Some(seconds)) => {
                Some(overlay::show(&d.label, seconds, &d.caption, d.appealable).await)
            }
            _ if shown.is_some() => Some(overlay::hide().await),
            _ => None,
        };
        if let Some(result) = sent {
            if result.is_ok() != overlay_ok {
                overlay_ok = result.is_ok();
                match result {
                    Ok(_) => eprintln!("intentionality: timer overlay reachable"),
                    Err(err) => eprintln!("intentionality: timer overlay unreachable: {err}"),
                }
            }
        }
        // Kept every SAVE while up, and as the clock goes down.
        let going_down = display.is_none() && shown.is_some();
        let due = display.is_some() && watch.ledger.as_ref().is_some_and(|l| l.saved.elapsed() >= SAVE);
        if going_down || due {
            watch.save(&app);
        }
        shown = display;

        wait = if watch.buckets.is_none() {
            IDLE_TICK
        } else {
            // A page waiting on SETTLE or ASK is looked at again the moment
            // it crosses one, not up to a tick later.
            in_front.as_ref().and_then(|(_, since)| wake_in(since.elapsed())).unwrap_or(TICK)
        };
    }
}

/// A case argued in the box under the clock: put it to Jev with every Doing
/// task, wake the loop (a win hides the clock), and answer under the clock.
/// One case at a time -- the box waits for its answer. Reconnects when the
/// stream ends, so the extension being re-enabled needs no restart here.
async fn relay_appeals(app: AppHandle) {
    loop {
        if let Ok(mut stream) = overlay::appeal_stream().await {
            while let Some(msg) = stream.next().await {
                let Ok(a) = msg.args() else { continue };
                let argument: Vec<String> = a
                    .argument()
                    .iter()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect();
                if argument.is_empty() {
                    continue;
                }
                let page = Page { app: a.app().to_string(), title: a.title().to_string() };
                let j = relevance::appeal(app.clone(), page.clone(), argument).await;
                app.state::<AppState>().relevance_changed.notify_one();
                let said = relevance::reply(&j);
                eprintln!("intentionality: appeal for {}: {said}", page.title);
                let _ = overlay::answer(&said, j.relevant.is_some()).await;
            }
        }
        tokio::time::sleep(REPROBE).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flag(id: i64, name: &str) -> RiskApp {
        RiskApp { id, name: name.into() }
    }

    fn event(timestamp: &str, duration: f64, data: serde_json::Value) -> AwEvent {
        AwEvent { timestamp: timestamp.into(), duration: Some(duration), data }
    }

    fn window(timestamp: &str, duration: f64, app: &str, title: &str) -> AwEvent {
        event(timestamp, duration, json!({ "app": app, "title": title }))
    }

    fn ts(s: &str) -> DateTime<Utc> {
        observed::parse_ts(s).unwrap()
    }

    const ZEN: &str = "app.zen_browser.zen";
    const LINKEDIN: &str = "Feed | LinkedIn — Zen Browser";

    #[test]
    fn a_website_is_looked_for_by_its_name() {
        assert_eq!(keyword("linkedin.com"), "linkedin");
        assert_eq!(keyword("  LinkedIn.com "), "linkedin");
        assert_eq!(keyword("https://www.linkedin.com/feed/?x=1"), "linkedin");
        assert_eq!(keyword("bbc.co.uk"), "bbc");
        assert_eq!(keyword("news.ycombinator.com"), "ycombinator");
        assert_eq!(keyword("x.com"), "x");
    }

    #[test]
    fn anything_else_is_looked_for_as_typed() {
        assert_eq!(keyword("Discord"), "discord");
        assert_eq!(keyword("Hacker News"), "hacker news");
        assert_eq!(keyword("v1.2"), "v1.2"); // a number is no top-level domain
        assert_eq!(keyword("https://"), "https://");
        assert_eq!(keyword("   "), "");
    }

    #[test]
    fn a_site_matches_a_browser_title_that_names_it() {
        assert_eq!(hit("linkedin", ZEN, LINKEDIN), Some(Hit::Title));
        assert_eq!(hit("linkedin", "org.mozilla.firefox", "(3) Feed | LinkedIn"), Some(Hit::Title));
        assert_eq!(hit("youtube", "google-chrome", "a video - YouTube"), Some(Hit::Title));
        assert_eq!(hit("linkedin", ZEN, "Some other page — Zen Browser"), None);
    }

    /// A title only counts inside a browser: an editor with linkedin.py open
    /// is not time on LinkedIn.
    #[test]
    fn a_title_outside_a_browser_does_not_match() {
        assert_eq!(hit("linkedin", "code", "linkedin.py - scraper - Visual Studio Code"), None);
    }

    #[test]
    fn an_app_matches_by_its_id() {
        assert_eq!(hit("discord", "discord", "#general"), Some(Hit::App));
        assert_eq!(hit("spotify", "Spotify", "Spotify Premium"), Some(Hit::App));
        assert_eq!(hit("zen", ZEN, LINKEDIN), Some(Hit::App));
    }

    #[test]
    fn a_short_flag_matches_whole_words_only() {
        assert_eq!(hit("x", "xdg-desktop-portal-gtk", "Open File"), None);
        assert_eq!(hit("x", ZEN, "Explore / X — Zen Browser"), Some(Hit::Title));
        assert_eq!(hit("reddit", ZEN, "r/AskReddit — Zen Browser"), None);
        assert_eq!(hit("zen", "zenity", "Question"), None);
        assert_eq!(hit("", ZEN, LINKEDIN), None);
    }

    #[test]
    fn the_focused_window_names_its_flag() {
        let apps = [flag(1, "discord"), flag(2, "linkedin.com")];
        assert_eq!(current(&apps, ZEN, LINKEDIN).map(|f| f.id), Some(2));
        assert_eq!(current(&apps, "code", "main.rs"), None);
        assert_eq!(current(&apps, "", ""), None);
        assert_eq!(current(&[], ZEN, LINKEDIN), None);
    }

    /// Polling AW: its newest event is the window in front only while the
    /// watcher keeps extending it.
    #[test]
    fn an_aw_event_is_the_focused_window_only_while_fresh() {
        let now = ts("2026-10-19T12:00:00+00:00");
        let on = window("2026-10-19T11:50:00+00:00", 599.5, ZEN, LINKEDIN);
        assert_eq!(fresh_window(Some(&on), now), Some((ZEN.into(), LINKEDIN.into())));
        // The watcher stopped extending it 30 s ago: the screen is locked.
        let stale = window("2026-10-19T11:50:00+00:00", 570.0, ZEN, LINKEDIN);
        assert_eq!(fresh_window(Some(&stale), now), None);
        assert_eq!(fresh_window(None, now), None);
        let here = event("2026-10-19T11:00:00+00:00", 3600.0, json!({"status": "not-afk"}));
        let away = event("2026-10-19T11:54:00+00:00", 360.0, json!({"status": "afk"}));
        assert!(!is_away(Some(&here), now));
        assert!(is_away(Some(&away), now));
        // AFK for 3 min 20 s: still reading, by the grace -- the clock stays.
        let reading = event("2026-10-19T11:56:40+00:00", 200.0, json!({"status": "afk"}));
        assert!(!is_away(Some(&reading), now));
        // No AFK watcher at all: present, as far as anyone can tell.
        assert!(!is_away(None, now));
    }

    #[test]
    fn a_title_hit_beats_an_app_hit_and_ties_go_to_the_oldest_flag() {
        let apps = [flag(1, "zen"), flag(2, "linkedin.com")];
        assert_eq!(current(&apps, ZEN, LINKEDIN).map(|f| f.id), Some(2));
        let apps = [flag(3, "linkedin.com"), flag(4, "LinkedIn")];
        assert_eq!(current(&apps, ZEN, LINKEDIN).map(|f| f.id), Some(3));
    }

    #[test]
    fn totals_subtract_away_time_and_count_a_shared_moment_once() {
        let events = Events {
            window: vec![
                window("2026-10-19T09:00:00+00:00", 600.0, ZEN, LINKEDIN),
                window("2026-10-19T09:10:00+00:00", 300.0, "code", "main.rs"),
                window("2026-10-19T09:15:00+00:00", 120.0, ZEN, "Inbox — Zen Browser"),
                // Before the span starts: clipped to its last 60 s.
                window("2026-10-19T08:58:00+00:00", 120.0, "discord", "#general"),
            ],
            // No input for the last 400 s on LinkedIn: the last 100 s were away.
            afk: vec![event("2026-10-19T09:03:20+00:00", 400.0, json!({"status": "afk"}))],
        };
        let apps = [flag(1, "linkedin.com"), flag(2, "zen"), flag(3, "discord"), flag(4, "x.com")];
        let day = totals(
            &events,
            &apps,
            ts("2026-10-19T08:59:00+00:00"),
            ts("2026-10-19T12:00:00+00:00"),
        )
        .unwrap();
        let seconds: Vec<(i64, f64)> = day.apps.iter().map(|t| (t.id, t.seconds)).collect();
        assert_eq!(seconds, [(1, 500.0), (2, 620.0), (3, 60.0), (4, 0.0)]);
        // The LinkedIn 500 s are zen time too, and count once: 620 + 60.
        assert_eq!(day.total, 680.0);
    }

    fn judgement(verdicts: Vec<(Option<f64>, &str)>, relevant: Option<usize>, no_tasks: bool) -> Judgement {
        Judgement {
            app: ZEN.into(),
            page: "feed | linkedin".into(),
            title: LINKEDIN.into(),
            verdicts: verdicts
                .into_iter()
                .map(|(p, t)| crate::models::Verdict {
                    task_id: 1,
                    task_title: t.into(),
                    probability: p,
                    source: None,
                    model: None,
                    created_at: None,
                })
                .collect(),
            relevant,
            threshold: 0.7,
            no_tasks,
            error: None,
        }
    }

    /// The extension paints this caption red by its text, so the two
    /// languages must agree on it.
    #[test]
    fn the_extension_knows_the_off_task_caption() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../gnome-extension/at-risk-timer@intentionality/extension.js");
        let source = std::fs::read_to_string(path).unwrap();
        let declared = source
            .lines()
            .find_map(|l| l.trim().strip_prefix("const OFF_TASK = '"))
            .and_then(|rest| rest.strip_suffix("';"))
            .expect("extension.js declares const OFF_TASK = '<caption>';");
        assert_eq!(declared, OFF_TASK_CAPTION);
    }

    #[test]
    fn what_the_clock_says_under_each_judgement() {
        // Excused: nothing on screen at all.
        assert_eq!(display("linkedin.com", true, &judgement(vec![(Some(0.9), "apply")], Some(0), false)), None);
        assert_eq!(
            display("linkedin.com", true, &judgement(vec![(Some(0.1), "apply")], None, false)),
            Some(Display {
                label: "linkedin.com".into(),
                caption: "Not relevant to any task".into(),
                appealable: true,
                unexcused: true,
            })
        );
        // A task the model could not be asked about: say so, still appealable,
        // and not drift -- nobody has said the page is irrelevant.
        let unchecked =
            display("linkedin.com", true, &judgement(vec![(Some(0.1), "a"), (None, "b")], None, false)).unwrap();
        assert_eq!((unchecked.caption.as_str(), unchecked.unexcused), ("Relevance unchecked", false));
        let none = display("linkedin.com", true, &judgement(vec![], None, true)).unwrap();
        assert_eq!((none.caption.as_str(), none.appealable, none.unexcused), ("Nothing in Doing", false, false));
    }

    /// A window nobody flagged gets a clock when judged against every Doing
    /// task and relevant to none, or when nothing is in Doing at all. Never
    /// on "no key" or "Jev unreachable" -- and its time is not at-risk drift.
    #[test]
    fn an_unflagged_page_shows_only_when_judged_off_task() {
        let label = "VS Code: cereal";
        assert_eq!(
            display(label, false, &judgement(vec![(Some(0.1), "apply"), (Some(0.2), "write")], None, false)),
            Some(Display {
                label: label.into(),
                caption: "Not relevant to any task".into(),
                appealable: true,
                unexcused: false,
            })
        );
        assert_eq!(display(label, false, &judgement(vec![(Some(0.9), "apply")], Some(0), false)), None);
        assert_eq!(display(label, false, &judgement(vec![(Some(0.1), "a"), (None, "b")], None, false)), None);
        assert_eq!(
            display(label, false, &judgement(vec![], None, true)),
            Some(Display {
                label: label.into(),
                caption: "Nothing in Doing".into(),
                appealable: false,
                unexcused: false,
            })
        );
        let mut failed = judgement(vec![(None, "apply")], None, false);
        failed.error = Some("TypeSafe unreachable".into());
        assert_eq!(display(label, false, &failed), None);
    }

    #[test]
    fn a_flagged_window_is_asked_about_at_once_and_any_other_after_ask() {
        assert!(may_ask(true, Duration::ZERO));
        assert!(!may_ask(false, Duration::ZERO));
        assert!(!may_ask(false, ASK - Duration::from_millis(1)));
        assert!(may_ask(false, ASK));
    }

    /// From hidden, an unflagged window's clock waits out SETTLE; a clock
    /// already up moves to it at once, as does a flagged window's.
    #[test]
    fn a_clock_comes_up_after_settle_unless_one_is_up_already() {
        assert!(may_show(true, false, Duration::ZERO));
        assert!(may_show(false, true, Duration::ZERO));
        assert!(!may_show(false, false, Duration::ZERO));
        assert!(!may_show(false, false, SETTLE - Duration::from_millis(1)));
        assert!(may_show(false, false, SETTLE));
    }

    /// The loop looks again as a window crosses SETTLE or ASK -- just past
    /// it, never more than a tick away -- and not at all once past both.
    #[test]
    fn the_loop_wakes_as_a_window_crosses_settle_or_ask() {
        let first = SETTLE.min(ASK);
        let last = SETTLE.max(ASK);
        assert_eq!(wake_in(Duration::ZERO), Some((first + WAKE_SLACK).min(TICK)));
        let halfway = first / 2;
        assert_eq!(wake_in(halfway), Some((first - halfway + WAKE_SLACK).min(TICK)));
        // A wake lands past the threshold it was set for.
        assert!(halfway + wake_in(halfway).unwrap() > first);
        assert_eq!(wake_in(last), None);
        assert_eq!(wake_in(last + Duration::from_secs(5)), None);
        for ms in (0..2000).step_by(50) {
            assert!(wake_in(Duration::from_millis(ms)).is_none_or(|w| w <= TICK));
        }
    }

    /// Complete = nothing left to ask: every Doing task has a verdict, or
    /// there is no task at all.
    #[test]
    fn a_judgement_is_complete_when_no_task_is_left_to_ask() {
        assert!(judgement(vec![(Some(0.1), "a"), (Some(0.9), "b")], Some(1), false).complete());
        assert!(!judgement(vec![(Some(0.1), "a"), (None, "b")], None, false).complete());
        assert!(judgement(vec![], None, true).complete());
        // Tasks, yet no verdict rows: never judged.
        assert!(!judgement(vec![], None, false).complete());
    }

    fn verdicts(probabilities: &[Option<f64>], relevant: Option<usize>) -> Judgement {
        let mut j = judgement(probabilities.iter().map(|p| (*p, "t")).collect(), relevant, probabilities.is_empty());
        j.threshold = 0.7;
        j
    }

    /// The shared clock's total: every page that would show it, each moment
    /// once, away time excepted.
    #[test]
    fn the_day_total_counts_every_page_that_would_show_the_clock() {
        let events = Events {
            window: vec![
                // Judged off task: counts.
                window("2026-10-19T09:00:00+00:00", 300.0, ZEN, "Some video - YouTube — Zen Browser"),
                // Excused: does not.
                window("2026-10-19T09:05:00+00:00", 200.0, ZEN, "Docs — Zen Browser"),
                // Never judged, nobody flagged it: does not.
                window("2026-10-19T09:08:20+00:00", 100.0, ZEN, "Unknown page — Zen Browser"),
                // Flagged and judged off task: counts, once.
                window("2026-10-19T09:10:00+00:00", 60.0, ZEN, LINKEDIN),
                // Flagged, Jev never asked: counts, as its clock does.
                window("2026-10-19T09:11:00+00:00", 40.0, "discord", "#general"),
                // The app's own window: nobody judges it.
                window("2026-10-19T09:11:40+00:00", 20.0, "intentionality", "intentionality"),
                // The same video again, partly away (no input 09:12:00 on,
                // away from 09:17:00): 300 of its 400 s count.
                window("2026-10-19T09:12:00+00:00", 400.0, ZEN, "Some video - YouTube — Zen Browser"),
            ],
            afk: vec![event("2026-10-19T09:12:00+00:00", 400.0, json!({"status": "afk"}))],
        };
        let apps = [flag(1, "linkedin.com"), flag(2, "discord")];
        let mut asked = Vec::new();
        let total = off_task_seconds(
            &events,
            &apps,
            ts("2026-10-19T08:00:00+00:00"),
            ts("2026-10-19T12:00:00+00:00"),
            |_, title| {
                asked.push(title.to_string());
                Ok(if title.contains("YouTube") || title.contains("LinkedIn") {
                    verdicts(&[Some(0.1), Some(0.2)], None)
                } else if title.starts_with("Docs") {
                    verdicts(&[Some(0.9), Some(0.2)], Some(0))
                } else {
                    verdicts(&[None, None], None)
                })
            },
        )
        .unwrap();
        assert_eq!(total, 300.0 + 60.0 + 40.0 + 300.0);
        // One question per page, however many times it was in front.
        assert_eq!(asked.iter().filter(|t| t.contains("YouTube")).count(), 1);
    }

    /// Nothing in Doing: every window shows the clock, so all of it counts --
    /// except the app's own window, which nobody judges.
    #[test]
    fn with_nothing_in_doing_every_judged_window_counts() {
        let events = Events {
            window: vec![
                window("2026-10-19T09:00:00+00:00", 300.0, ZEN, "Some video - YouTube — Zen Browser"),
                window("2026-10-19T09:05:00+00:00", 60.0, ZEN, LINKEDIN),
                window("2026-10-19T09:06:00+00:00", 30.0, "intentionality", "intentionality"),
            ],
            afk: vec![],
        };
        let total = off_task_seconds(
            &events,
            &[flag(1, "linkedin.com")],
            ts("2026-10-19T08:00:00+00:00"),
            ts("2026-10-19T12:00:00+00:00"),
            |_, _| Ok(verdicts(&[], None)),
        )
        .unwrap();
        assert_eq!(total, 300.0 + 60.0);
    }
}
