//! Gathers the stretch for one check, has Jev judge its pages, and persists
//! the result.
//!
//! Two entry points over the same judging. `run` is the check -- on the
//! randomized timer, on the manual button, or early when unexcused at-risk
//! time piles up (Trigger::Drift). It returns Ok(None) when the window was
//! too quiet to judge fairly, and has no side effects beyond the row it
//! writes and the verdicts it caches. `run_checkpoint` fires when the
//! intended time runs out, and never skips — a quiet window or an
//! unreachable TypeSafe still produces a screen, because the point of the
//! checkpoint is the moment, not the analysis.
//!
//! Every page is judged through relevance.rs, the at-risk timer's own path:
//! one cache, one question, one bar. See check.rs for what the verdicts
//! become.

use crate::check::{self, Tally};
use crate::error::{AppError, Result};
use crate::jev::{self, Typed};
use crate::models::{Judged, Observed, Pick, RiskApp, Session, Task};
use crate::recommendations::{self, Recommendation};
use crate::relevance::{self, Page};
use crate::state::AppState;
use crate::{db, notify, observed, scheduler};
use tauri::{AppHandle, Emitter, Manager};

/// What started a check. Stored in the row's snapshot; the row's `kind`
/// stays 'check' for all three.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trigger {
    Scheduled,
    Manual,
    /// Early: the at-risk timer counted enough unexcused time.
    Drift,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Trigger::Scheduled => "scheduled",
            Trigger::Manual => "manual",
            Trigger::Drift => "drift",
        }
    }
}

/// Everything both calls need, gathered once.
struct Prepared {
    session: Session,
    /// The whole board, every status: the checkpoint shows Jev its lanes.
    tasks: Vec<Task>,
    window_start: String,
    window_end: String,
    window: Observed,
    previous: Option<i64>,
    elapsed_minutes: i64,
    risk_apps: Vec<RiskApp>,
    min: f64,
}

impl Prepared {
    fn observed_json(&self) -> String {
        serde_json::to_string(&self.window).unwrap_or_else(|_| "{}".into())
    }
}

async fn prepare(app: &AppHandle, session_id: i64) -> Result<Option<Prepared>> {
    let state = app.state::<AppState>();

    // Everything read up front so no lock is held across awaits.
    let (session, tasks, window_start, previous, risk_apps, min) = {
        let conn = state.conn.lock().unwrap();
        let Some(session) = db::get_session(&conn, session_id)? else { return Ok(None) };
        let tasks = db::session_tasks(&conn, session_id)?;
        let latest = db::latest_analysis(&conn, session_id)?;
        let window_start = latest
            .as_ref()
            .map_or_else(|| session.started_at.clone(), |a| a.window_end.clone());
        let previous = latest.and_then(|a| a.alignment);
        let risk_apps = db::list_risk_apps(&conn).unwrap_or_default();
        (session, tasks, window_start, previous, risk_apps, db::relevance_min(&conn))
    };

    let window_end = db::now();
    // A window AW cannot answer for reads as an empty one. `run` then skips it
    // as too quiet, and the checkpoint falls back — neither has to special-case
    // ActivityWatch being down.
    let window = observed::observed(&window_start, &window_end).await.unwrap_or_default();

    let elapsed_minutes = observed::parse_ts(&window_end)?
        .signed_duration_since(observed::parse_ts(&session.started_at)?)
        .num_minutes();

    Ok(Some(Prepared {
        session,
        tasks,
        window_start,
        window_end,
        window,
        previous,
        elapsed_minutes,
        risk_apps,
        min,
    }))
}

/// The stretch, judged: the tally and its alignment, if there was enough to
/// say anything.
async fn judge(app: &AppHandle, prep: &Prepared, tasks: &[Task]) -> (Tally, Option<i64>) {
    let pages = check::pages(&prep.window, check::MAX_PAGES);
    let asks = pages
        .pages
        .iter()
        .map(|p| (Page { app: p.app.clone(), title: p.title.clone() }, tasks.to_vec()))
        .collect();
    let judgements = relevance::judge_many(app, asks).await;
    let tally = check::tally(&pages, &judgements, tasks, prep.min);
    let alignment = check::alignment(&tally);
    (tally, alignment)
}

fn snapshot_json(judged: &Judged) -> String {
    serde_json::to_string(judged).unwrap_or_else(|_| "{}".into())
}

pub async fn run(app: &AppHandle, trigger: Trigger) -> Result<Option<i64>> {
    let state = app.state::<AppState>();
    let Some(session_id) = state.session_id() else { return Ok(None) };
    // One analysis at a time: the timers plus the manual button could
    // otherwise overlap, racing on where the latest analysis ended (two runs
    // would judge the same window twice).
    let _running = state.analysis_lock.lock().await;

    let Some(prep) = prepare(app, session_id).await? else { return Ok(None) };
    if prep.window.active_seconds < scheduler::MIN_ACTIVE_SECS {
        return Ok(None); // too quiet to judge fairly
    }
    let tasks = check::judgeable(&prep.tasks);
    if tasks.is_empty() {
        return Err(AppError::Other("Skipped — no tasks on the board to judge against".into()));
    }

    let (tally, alignment) = judge(app, &prep, &tasks).await;
    let Some(alignment) = alignment else {
        return Err(AppError::Other(format!("Skipped — {}", check::nothing_judged(&tally))));
    };
    let (headline, body) = check::word(&tally, alignment, prep.previous);
    let risk = check::risk(&prep.window, &prep.risk_apps, &tally);
    let judged = check::judged(&tally, risk, trigger.as_str(), prep.min, None);

    let id = {
        let conn = state.conn.lock().unwrap();
        db::add_analysis(
            &conn,
            &db::NewAnalysis {
                session_id,
                kind: "check",
                window_start: &prep.window_start,
                window_end: &prep.window_end,
                headline: &headline,
                alignment: Some(alignment),
                body: &body,
                observed_json: &prep.observed_json(),
                recommendation_id: None,
                recommendation_note: None,
                judged_json: &snapshot_json(&judged),
            },
        )?
    };
    // A check landed, so the drift count starts over from here.
    state.reset_drift();
    Ok(Some(id))
}

/// Jev's pick from the catalog for this stretch. A failed or unknown answer
/// lands on the catalog's fallback rather than failing the checkpoint.
async fn choose(prep: &Prepared, tally: &Tally, alignment: i64) -> (Recommendation, Option<Pick>) {
    let state = check::pick_state(
        &prep.session,
        prep.elapsed_minutes,
        &prep.tasks,
        tally,
        alignment,
        prep.previous,
    );
    match jev::ask(state, check::pick_question(), relevance::CHECK_TIMEOUT).await {
        Ok(response) => match response.answers.get(check::PICK_KEY) {
            Some(Typed::Choice { choice, probabilities, confidence }) => {
                let rec = recommendations::resolve(choice);
                (rec, Some(check::pick(rec.id, probabilities.clone(), *confidence)))
            }
            _ => (recommendations::FALLBACK, None),
        },
        Err(err) => {
            eprintln!("intentionality: checkpoint pick fell back: {err}");
            (recommendations::FALLBACK, None)
        }
    }
}

/// The time-up checkpoint. Unlike `run` this is self-contained — it clears the
/// pending checkpoint, emits, and notifies — because both callers (the timer
/// and the manual trigger) want exactly the same thing to happen.
pub async fn run_checkpoint(app: &AppHandle) -> Result<Option<i64>> {
    let state = app.state::<AppState>();
    let Some(session_id) = state.session_id() else { return Ok(None) };
    let _running = state.analysis_lock.lock().await;

    let Some(prep) = prepare(app, session_id).await? else { return Ok(None) };
    let tasks = check::judgeable(&prep.tasks);

    // A quiet window is not a reason to stay silent here: the user asked to be
    // told when their time was up, and being told is the deliverable. The same
    // goes for TypeSafe being unreachable — the clock still ran out. Every
    // fallback carries (headline, body); none opens with "time's up", because
    // the notification title and the screen already say that and a history
    // row headlined that way would record the clock, not the reason.
    let judged = if prep.window.active_seconds < scheduler::MIN_ACTIVE_SECS {
        Err((
            "Nothing tracked to judge",
            "Under five minutes of tracked activity in this stretch, so there is \
             nothing here to judge — this is the clock talking, not an assessment."
                .to_string(),
            None,
        ))
    } else if tasks.is_empty() {
        Err((
            "Nothing on the board to judge against",
            "There are no tasks on the board, so there is nothing to hold this \
             stretch up against — this is the clock talking, not an assessment."
                .to_string(),
            None,
        ))
    } else {
        match judge(app, &prep, &tasks).await {
            (tally, Some(alignment)) => Ok((tally, alignment)),
            (tally, None) => Err((
                "Check could not run",
                format!(
                    "{}. Your time is up regardless.",
                    capitalize(&check::nothing_judged(&tally))
                ),
                Some(tally),
            )),
        }
    };

    let (headline, alignment, body, recommendation, note, snapshot) = match judged {
        Ok((tally, alignment)) => {
            let (headline, body) = check::word(&tally, alignment, prep.previous);
            let (recommendation, pick) = choose(&prep, &tally, alignment).await;
            let note = check::pick_note(&tally, &prep.session, prep.elapsed_minutes);
            let risk = check::risk(&prep.window, &prep.risk_apps, &tally);
            let snapshot = check::judged(&tally, risk, "checkpoint", prep.min, pick);
            (headline, Some(alignment), body, recommendation, Some(note), snapshot)
        }
        Err((headline, why, tally)) => {
            // Whatever was judged is still worth keeping on the row.
            let snapshot = match tally {
                Some(tally) => check::judged(&tally, Vec::new(), "checkpoint", prep.min, None),
                None => Judged {
                    trigger: "checkpoint".into(),
                    threshold: prep.min,
                    ..Default::default()
                },
            };
            (headline.to_string(), None, why, recommendations::QUIET_WINDOW, None, snapshot)
        }
    };

    let id = {
        let conn = state.conn.lock().unwrap();
        let id = db::add_analysis(
            &conn,
            &db::NewAnalysis {
                session_id,
                kind: "checkpoint",
                window_start: &prep.window_start,
                window_end: &prep.window_end,
                headline: &headline,
                alignment,
                body: &body,
                observed_json: &prep.observed_json(),
                recommendation_id: Some(recommendation.id),
                recommendation_note: note.as_deref(),
                judged_json: &snapshot_json(&snapshot),
            },
        )?;
        // Cleared only after the row lands: if the insert fails the checkpoint
        // is still due and will be retried on the next tick.
        db::clear_checkpoint(&conn, session_id)?;
        id
    };
    state.reset_drift();

    let subtitle = match prep.session.intended_minutes {
        Some(m) => format!("{m} min intended, {} elapsed", prep.elapsed_minutes),
        None => format!("{} min elapsed", prep.elapsed_minutes),
    };
    notify::send(
        app,
        &format!("Time's up — {subtitle}"),
        &notify::clip(&format!("{headline} · {}", recommendation.advice), 160),
    );
    let _ = app.emit("checkpoint:new", id);
    raise_window(app);
    Ok(Some(id))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Best-effort only, and worth being honest about why: under GNOME on Wayland
/// an application cannot raise itself. set_focus() goes through xdg-activation,
/// and without a valid activation token gnome-shell turns the request into a
/// "demands attention" state rather than a raise. This genuinely helps when the
/// window is minimized or on the current workspace; the notification is the
/// signal that actually reaches you, and the overlay guarantees the checkpoint
/// is what is there when you do look.
fn raise_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
