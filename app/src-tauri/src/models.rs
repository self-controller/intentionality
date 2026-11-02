use crate::build_info::BuildInfo;
use crate::recommendations::Recommendation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct Session {
    pub id: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub close_reason: Option<String>,
    pub statement: String,
    pub intended_minutes: Option<i64>,
    /// When the time-up checkpoint fires; None once it has, or for an
    /// open-ended session that never gets one.
    pub checkpoint_due_at: Option<String>,
    /// Sessions committed since the gate stopped asking for a statement have
    /// an empty one; these two label them by their tasks instead.
    pub first_task: Option<String>,
    pub task_count: i64,
}

/// A tag a task wears. Free text the user invented; the colour was assigned
/// when the label was first created and never changes, so the same word is the
/// same colour on every card.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Label {
    pub name: String,
    pub color: String,
}

/// A label as the labels panel and the editor list it: every one there is,
/// with how many tasks wear it (0 = a preset nothing uses yet).
#[derive(Serialize, Debug)]
pub struct LabelSummary {
    pub name: String,
    pub color: String,
    pub uses: i64,
}

#[derive(Serialize, Clone)]
pub struct Task {
    pub id: i64,
    pub session_id: Option<i64>,
    pub title: String,
    pub position: i64,
    pub status: String,
    pub carry_count: i64, // 0 for uncarried tasks
    /// Plain text the user wrote on the card. Never rendered as markdown.
    pub notes: String,
    /// The day it is due, `YYYY-MM-DD`; None = no due date.
    pub due_date: Option<String>,
    /// "high", "medium" or "low". The board orders each column by it.
    pub priority: String,
    /// Filled in by a second query, so a row read on its own starts empty.
    pub labels: Vec<Label>,
}

#[derive(Serialize)]
pub struct Board {
    pub session: Option<Session>,
    pub tasks: Vec<Task>,   // current session's cards; empty in no-session mode
    pub backlog: Vec<Task>, // session_id NULL
}

/// The entire post-drop board, written atomically. One command instead of
/// separate move/reorder calls means the board can never drift from the DB.
#[derive(Deserialize)]
pub struct Arrangement {
    pub todo: Vec<i64>,
    pub doing: Vec<i64>,
    pub done: Vec<i64>,
    pub dropped: Vec<i64>,
}

/// Also read back out of analysis.observed_json, which is why it deserializes.
/// `default` is load-bearing there: rows predating per_title have no such key,
/// and add_analysis stores a literal "{}" when serialization fails.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Observed {
    pub per_app: BTreeMap<String, f64>,
    pub per_title: BTreeMap<String, BTreeMap<String, f64>>,
    pub active_seconds: f64,
    pub afk_seconds: f64,
}

#[derive(Serialize)]
pub struct Analysis {
    pub id: i64,
    pub session_id: i64,
    pub created_at: String,
    pub window_start: String,
    pub window_end: String,
    pub headline: String,
    pub alignment: Option<i64>,
    pub body: String,
    pub seen_at: Option<String>,
    pub session_statement: Option<String>, // joined; None only if the session row vanished
    pub kind: String,                      // "check" | "checkpoint"
    /// Resolved from the stored recommendation_id at read time, so the
    /// frontend never carries a copy of the catalog and a reworded entry
    /// applies to every checkpoint already recorded. None on plain checks, and
    /// on a stored id the catalog no longer knows.
    pub recommendation: Option<Recommendation>,
    pub recommendation_note: Option<String>,
}

/// What Jev judged for one check, stored as analysis.judged_json. A
/// snapshot: the relevance cache it was read from changes later (appeals,
/// edits, Forget), and the alignment on the row must keep matching this
/// breakdown. Rows written before v14 have none and show the old way.
/// `default` everywhere so a field added later reads old rows.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Judged {
    /// Every versioned Jev behind the verdicts used, oldest cache row first.
    pub models: Vec<String>,
    /// The bar a verdict had to clear: meta relevance_min at the time.
    pub threshold: f64,
    /// "scheduled", "manual", "drift" (early, on unexcused at-risk time), or
    /// "checkpoint".
    pub trigger: String,
    /// Time on each task the check could credit, board order.
    pub tasks: Vec<TaskTime>,
    /// The stretch's pages, most time first, as far as the check looked.
    pub pages: Vec<JudgedPage>,
    pub aligned_seconds: f64,
    pub off_seconds: f64,
    /// Time no verdict covers: pages past the cap, the "(other)" rollup, and
    /// pages Jev could not be asked about.
    pub unjudged_seconds: f64,
    /// Flagged apps in this stretch, flag order. Empty when none was used.
    pub risk: Vec<RiskTime>,
    /// The checkpoint's recommendation as Jev chose it. None on checks.
    pub pick: Option<Pick>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct TaskTime {
    pub task_id: i64,
    pub title: String,
    pub seconds: f64,
}

/// One page of a check's stretch and how it was decided.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct JudgedPage {
    /// The window as ActivityWatch recorded it: what an appeal is opened for.
    pub app: String,
    pub title: String,
    /// The title as shown: unread count and browser name taken off.
    pub name: String,
    /// The cache key's page half.
    pub page: String,
    pub seconds: f64,
    /// "excused" (a task cleared the bar), "no_task" (every task judged, none
    /// did) or "unjudged".
    pub verdict: String,
    /// The excusing verdict, or else the closest one: what the row shows.
    pub task_id: Option<i64>,
    pub task_title: Option<String>,
    pub probability: Option<f64>,
    /// "model" or "appeal".
    pub source: Option<String>,
}

/// One flagged app's time in a check's stretch, split by what the verdicts
/// said about the pages it was on.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct RiskTime {
    pub name: String,
    pub seconds: f64,
    pub excused_seconds: f64,
    pub no_task_seconds: f64,
    pub unjudged_seconds: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Pick {
    /// The catalog id that was used, after validation.
    pub id: String,
    /// Jev's probability for every option it was offered.
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// One analysis opened in the tab: what was tracked, and what Jev made of it.
#[derive(Serialize)]
pub struct AnalysisDetail {
    pub observed: Observed,
    pub judged: Option<Judged>,
}

#[derive(Serialize)]
pub struct Health {
    pub schema_version: i64,
    pub needs_migration: bool,
    pub session: Option<Session>,
    pub aw_ok: bool,
    pub build: BuildInfo,
    /// Why the resume gate will not fire, when it will not. None = installed
    /// and matching the repo (or no repo to compare against).
    pub resume_gate: Option<String>,
    /// Why the at-risk timer cannot reach the screen, when it cannot: the
    /// GNOME Shell extension that draws it is missing or older than this app.
    pub overlay: Option<String>,
    /// Set when ActivityWatch cannot see Wayland windows, because the Focused
    /// Window D-Bus extension it reads them from is off. Every observation is
    /// then blind, the timer included.
    pub window_tracking: Option<String>,
    /// Set when pages cannot be judged for relevance: no TypeSafe key. The
    /// timer then shows for every flagged page, captioned as unchecked, and
    /// checks are skipped.
    pub relevance: Option<String>,
}

/// An app or website the user flagged as at risk, by the name they typed.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct RiskApp {
    pub id: i64,
    pub name: String,
}

/// One flagged app's time today: every moment it was the focused window and
/// the user was not away.
#[derive(Serialize, Debug, PartialEq)]
pub struct RiskTotal {
    pub id: i64,
    pub name: String,
    pub seconds: f64,
}

/// Today on the at-risk apps. `total` counts a moment once however many flags
/// matched it, so it can be less than the rows' sum, never more.
#[derive(Serialize, Debug)]
pub struct RiskToday {
    pub apps: Vec<RiskTotal>,
    pub total: f64,
    /// Rows in the relevance cache: pages × tasks Jev has judged.
    pub judged: i64,
    /// Why no time could be read (ActivityWatch down). The flags are still
    /// listed, at zero, so one can be removed while AW is away.
    pub error: Option<String>,
}

/// One day (day.rs: from the day-start hour to the next), across sessions,
/// as the Dashboard shows it: where the active time went by the timer's own
/// rule (a page is relevant when one of the tasks it is judged against
/// clears the bar; today.rs says which tasks those are).
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct DayTime {
    /// The day shown, "YYYY-MM-DD", named by the date it began on.
    pub day: String,
    /// Today by the same reckoning, so the Dashboard knows where days end.
    pub today: String,
    /// The hour days start at (0-23).
    pub start_hour: u32,
    /// The window read: the day's start, and now or the next day's start.
    pub start: String,
    pub end: String,
    pub active_seconds: f64,
    pub afk_seconds: f64,
    pub aligned_seconds: f64,
    pub off_seconds: f64,
    /// Time no verdict covers, `outside_seconds` included.
    pub unjudged_seconds: f64,
    /// Active time in no session: no tasks, so nothing to be relevant to.
    pub outside_seconds: f64,
    /// Relevant time by the likeliest task that excused it, in board order:
    /// Doing, then the tasks finished that day (not sorted by time, so the
    /// Dashboard's colour per task holds). Carried copies of one task are
    /// one row.
    pub tasks: Vec<TaskTime>,
    /// Unjudged pages Jev could be asked about now, up to one check's worth.
    pub askable: usize,
    /// The bar a verdict has to clear: meta relevance_min.
    pub threshold: f64,
    /// Why some pages could not be judged on the last ask (no key, Jev down).
    pub error: Option<String>,
    /// Where the not-relevant time went, busiest first: one row per site,
    /// tool and project, or app (subject.rs `source`).
    pub culprits: Vec<Culprit>,
    /// Every app and site seen that day, busiest first, for the ignore menu.
    pub seen: Vec<IgnoreChoice>,
    /// Page–task verdicts in the relevance cache.
    pub judged: i64,
}

/// One row of the Dashboard's culprits chart.
#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct Culprit {
    pub name: String,
    pub seconds: f64,
}

/// An app or website the ignore menu offers, with what an ignore rule for
/// it would hold (ignore.rs `Rule`).
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct IgnoreChoice {
    pub kind: crate::ignore::Kind,
    pub value: String,
    pub name: String,
    pub classes: Vec<String>,
    /// Today's active time on it.
    pub seconds: f64,
    /// Already on the list.
    pub ignored: bool,
}

/// Jev's verdict on one page for one task on the board. `probability` is
/// None while the task has not been judged against this page (a cache miss
/// the model could not be asked about).
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Verdict {
    pub task_id: i64,
    pub task_title: String,
    pub probability: Option<f64>,
    /// "model" (the page alone) or "appeal" (with the user's argument).
    pub source: Option<String>,
    pub model: Option<String>,
    pub created_at: Option<String>,
}

/// A page judged against the session's unfinished tasks, board order.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Judgement {
    /// The cache key: window class and normalised title, lowercased.
    pub app: String,
    pub page: String,
    /// The title as the shell showed it.
    pub title: String,
    pub verdicts: Vec<Verdict>,
    /// Index into `verdicts` of the task that excuses the page, if one does.
    pub relevant: Option<usize>,
    pub threshold: f64,
    /// No open session, or nothing unfinished on it: nothing to be relevant to.
    pub no_tasks: bool,
    /// Why the model could not be asked, when it could not. The verdicts the
    /// cache already held still stand.
    pub error: Option<String>,
}

impl Judgement {
    /// Every task in Doing has a verdict on the page, or there is no task to
    /// have one: nothing is left to ask Jev. The timer can act on a complete
    /// judgement straight from the cache, and only a complete one counts as
    /// judged in today's off-task total.
    pub fn complete(&self) -> bool {
        self.no_tasks
            || (!self.verdicts.is_empty() && self.verdicts.iter().all(|v| v.probability.is_some()))
    }
}
