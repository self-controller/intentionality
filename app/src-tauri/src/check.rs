//! What a check makes of a stretch, from Jev's verdicts. Pure: analysis.rs
//! gathers the stretch and asks, this decides and words.
//!
//! A check asks the at-risk timer's own question about every page of its
//! stretch (relevance.rs), so "on task" means here exactly what it means to
//! the timer: some task's verdict clears the bar. The alignment is the share
//! of judged active time that was on task. Jev answers with probabilities,
//! never prose, so every sentence on the row is written here.

use crate::models::{
    Judged, JudgedPage, Judgement, Observed, Pick, RiskApp, RiskTime, Session, Task, TaskTime,
};
use crate::{recommendations, relevance, screen_time, subject};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// How many pages one check asks about. The busiest pages carry nearly all
/// of a stretch; the rest is reported as not judged rather than paid for.
pub const MAX_PAGES: usize = 30;
/// Below this share of active time judged, a check has nothing fair to say.
const MIN_JUDGED_SHARE: f64 = 0.5;
/// Mirrors format.ts `band` and scheduler.rs `notify_check`.
const GOOD: i64 = 67;
const MIXED: i64 = 34;
/// A change in alignment smaller than this reads as "about the same".
const TREND_STEP: i64 = 10;
/// Titles quoted in a sentence are cut to this many characters.
const QUOTE_CHARS: usize = 40;

/// Titles that are not pages: observed.rs's rollup of an app's long tail,
/// and the stand-in for a window that had none.
const NOT_PAGES: &[&str] = &["(other)", "unknown"];

/// Every task a check can credit: the session's, except the dropped ones.
/// Done ones count, since time spent finishing a task was time on it.
pub fn judgeable(tasks: &[Task]) -> Vec<Task> {
    tasks.iter().filter(|t| t.status != "dropped").cloned().collect()
}

/// One page of the stretch: every title that is the same page, summed.
#[derive(Debug, Clone, PartialEq)]
pub struct PageTime {
    pub key: (String, String),
    /// The window as recorded, by the title that held the page longest.
    pub app: String,
    pub title: String,
    pub seconds: f64,
}

/// The pages worth asking about, busiest first, and the time left over.
#[derive(Debug, PartialEq)]
pub struct Pages {
    pub pages: Vec<PageTime>,
    pub rest_seconds: f64,
}

pub fn pages(obs: &Observed, max: usize) -> Pages {
    // key -> (seconds, (best title's seconds, app, title))
    let mut by_key: HashMap<(String, String), (f64, (f64, String, String))> = HashMap::new();
    for (app, titles) in &obs.per_title {
        if app == "unknown" {
            continue;
        }
        for (title, secs) in titles {
            // The rollups, and windows nobody judges (this app's own, the
            // file-picker portals): time, but not a page about anything.
            if NOT_PAGES.contains(&title.as_str()) || subject::of(app, title).is_none() {
                continue;
            }
            let key = relevance::page_key(app, title);
            if key.1.is_empty() {
                continue;
            }
            let entry = by_key.entry(key).or_insert((0.0, (0.0, String::new(), String::new())));
            entry.0 += secs;
            if *secs > entry.1 .0 {
                entry.1 = (*secs, app.clone(), title.clone());
            }
        }
    }
    let mut pages: Vec<PageTime> = by_key
        .into_iter()
        .map(|(key, (seconds, (_, app, title)))| PageTime { key, app, title, seconds })
        .collect();
    pages.sort_by(|a, b| b.seconds.total_cmp(&a.seconds).then_with(|| a.key.cmp(&b.key)));
    pages.truncate(max);
    let kept: f64 = pages.iter().map(|p| p.seconds).sum();
    Pages { pages, rest_seconds: (obs.active_seconds - kept).max(0.0) }
}

/// What the verdicts made of the stretch.
#[derive(Debug, Clone, PartialEq)]
pub struct Tally {
    pub pages: Vec<JudgedPage>,
    pub tasks: Vec<TaskTime>,
    pub aligned: f64,
    pub off: f64,
    pub unjudged: f64,
    pub models: Vec<String>,
    /// Why some pages went unjudged, when Jev could not be asked: the first
    /// such error, which is usually every such error.
    pub error: Option<String>,
}

/// Decide each page by its judgement (`judgements[i]` is about `pages[i]`;
/// None = the judging task itself failed). A page is excused when some
/// task's verdict clears `min` -- ties go to the task higher on the board --
/// no task when every task was judged and none did, and unjudged otherwise.
pub fn tally(pages: &Pages, judgements: &[Option<Judgement>], tasks: &[Task], min: f64) -> Tally {
    let mut credit: Vec<f64> = vec![0.0; tasks.len()];
    let index: HashMap<i64, usize> = tasks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    let mut out = Vec::new();
    let (mut aligned, mut off, mut unjudged) = (0.0, 0.0, pages.rest_seconds);
    let mut models: Vec<(String, String)> = Vec::new(); // (created_at, model)
    let mut error = None;
    for (page, judgement) in pages.pages.iter().zip(judgements) {
        let mut row = JudgedPage {
            app: page.app.clone(),
            title: page.title.clone(),
            name: relevance::page_title(&page.app, &page.title),
            page: page.key.1.clone(),
            seconds: page.seconds,
            verdict: "unjudged".into(),
            ..Default::default()
        };
        let Some(j) = judgement else {
            unjudged += page.seconds;
            out.push(row);
            continue;
        };
        if error.is_none() {
            error = j.error.clone();
        }
        for v in &j.verdicts {
            if let (Some(m), Some(at)) = (&v.model, &v.created_at) {
                models.push((at.clone(), m.clone()));
            }
        }
        let excused = relevance::decide(&j.verdicts, min);
        let closest = excused.or_else(|| {
            j.verdicts
                .iter()
                .enumerate()
                .filter(|(_, v)| v.probability.is_some())
                .rev()
                .max_by(|a, b| a.1.probability.partial_cmp(&b.1.probability).unwrap())
                .map(|(i, _)| i)
        });
        if let Some(v) = closest.map(|i| &j.verdicts[i]) {
            row.task_id = Some(v.task_id);
            row.task_title = Some(v.task_title.clone());
            row.probability = v.probability;
            row.source = v.source.clone();
        }
        // Nothing in Doing: nothing for the page to be relevant to, so it is
        // not, the timer's rule. (A check never tallies without tasks.)
        let all_judged = j.no_tasks
            || (!j.verdicts.is_empty() && j.verdicts.iter().all(|v| v.probability.is_some()));
        if let Some(i) = excused {
            row.verdict = "excused".into();
            aligned += page.seconds;
            if let Some(&t) = index.get(&j.verdicts[i].task_id) {
                credit[t] += page.seconds;
            }
        } else if all_judged {
            row.verdict = "no_task".into();
            off += page.seconds;
        } else {
            unjudged += page.seconds;
        }
        out.push(row);
    }
    // Oldest first, each model once.
    models.sort();
    let mut seen = BTreeSet::new();
    let models =
        models.into_iter().filter_map(|(_, m)| seen.insert(m.clone()).then_some(m)).collect();
    let tasks = tasks
        .iter()
        .zip(credit)
        .map(|(t, seconds)| TaskTime { task_id: t.id, title: t.title.clone(), seconds })
        .collect();
    Tally { pages: out, tasks, aligned, off, unjudged, models, error }
}

/// 0-100, or None when too little of the stretch was judged to say.
pub fn alignment(t: &Tally) -> Option<i64> {
    let judged = t.aligned + t.off;
    let active = judged + t.unjudged;
    if judged <= 0.0 || judged < MIN_JUDGED_SHARE * active {
        return None;
    }
    Some((100.0 * t.aligned / judged).round() as i64)
}

/// Why a check found nothing it could judge, for the log line or the
/// manual button.
pub fn nothing_judged(t: &Tally) -> String {
    match &t.error {
        Some(e) => format!("could not ask Jev: {e}"),
        None => "too little of this stretch could be judged".into(),
    }
}

/// The flagged apps' time in the stretch, split by what the check decided
/// about the pages they were on. From the same per-title totals the pages
/// came from, so it asks nothing and needs no second look at ActivityWatch.
pub fn risk(obs: &Observed, apps: &[RiskApp], t: &Tally) -> Vec<RiskTime> {
    // Keyed exactly as pages() keyed them, from the same strings.
    let verdicts: HashMap<(String, String), &str> = t
        .pages
        .iter()
        .map(|p| (relevance::page_key(&p.app, &p.title), p.verdict.as_str()))
        .collect();
    let mut rows: Vec<RiskTime> =
        apps.iter().map(|a| RiskTime { name: a.name.clone(), ..Default::default() }).collect();
    for (app, titles) in &obs.per_title {
        for (title, secs) in titles {
            let Some(flag) = screen_time::current(apps, app, title) else { continue };
            let Some(i) = apps.iter().position(|a| a.id == flag.id) else { continue };
            let row = &mut rows[i];
            row.seconds += secs;
            match verdicts.get(&relevance::page_key(app, title)).copied() {
                Some("excused") => row.excused_seconds += secs,
                Some("no_task") => row.no_task_seconds += secs,
                _ => row.unjudged_seconds += secs,
            }
        }
    }
    rows.retain(|r| r.seconds > 0.0);
    rows
}

/// "41s", "21m": a sub-minute span must not read as "0m".
pub fn dur(secs: f64) -> String {
    if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        format!("{:.0}m", secs / 60.0)
    }
}

/// A title as it sits inside a sentence: quoted, flattened, cut.
fn quote(title: &str) -> String {
    let flat = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = if flat.chars().count() > QUOTE_CHARS {
        format!("{}…", flat.chars().take(QUOTE_CHARS).collect::<String>().trim_end())
    } else {
        flat
    };
    format!("“{cut}”")
}

/// The task holding at least half the on-task time, if one does and there
/// was a minute of it: twelve seconds on a task is not "most time on" it.
fn lead(t: &Tally) -> Option<&TaskTime> {
    if t.aligned < 60.0 {
        return None;
    }
    t.tasks.iter().find(|x| x.seconds * 2.0 >= t.aligned)
}

/// The row's headline and body. Plain, short, no verdict on the person --
/// the same register the Claude-era notes kept.
pub fn word(t: &Tally, alignment: i64, previous: Option<i64>) -> (String, String) {
    let headline = if alignment >= GOOD {
        match lead(t) {
            Some(task) => format!("Mostly on {}", quote(&task.title)),
            None => "Mostly on the board".into(),
        }
    } else if alignment >= MIXED {
        "Split between the board and elsewhere".into()
    } else {
        "Mostly away from the board".into()
    };

    let mut body = String::new();
    let judged = t.aligned + t.off;
    let mut on: Vec<&TaskTime> = t.tasks.iter().filter(|x| x.seconds > 0.0).collect();
    on.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    body.push_str(&format!("{} of {} judged went to tasks", dur(t.aligned), dur(judged)));
    if !on.is_empty() {
        let named: Vec<String> =
            on.iter().take(3).map(|x| format!("{} {}", quote(&x.title), dur(x.seconds))).collect();
        body.push_str(&format!(": {}", named.join(", ")));
    }
    body.push('.');
    if t.off > 0.0 {
        let mut off: Vec<&JudgedPage> = t.pages.iter().filter(|p| p.verdict == "no_task").collect();
        off.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
        let named: Vec<String> =
            off.iter().take(2).map(|p| format!("{} {}", quote(&p.name), dur(p.seconds))).collect();
        body.push_str(&format!(" {} on pages no task needed — {}.", dur(t.off), named.join(", ")));
    }
    if t.unjudged >= 60.0 {
        body.push_str(&format!(" {} not judged.", dur(t.unjudged)));
    }
    if let Some(p) = previous {
        let trend = if alignment - p >= TREND_STEP {
            format!(" Up from {p} at the last check.")
        } else if p - alignment >= TREND_STEP {
            format!(" Down from {p} at the last check.")
        } else {
            format!(" About where the last check was ({p}).")
        };
        body.push_str(&trend);
    }
    (headline, body)
}

/// "mostly on task" and friends: the band in words, for Jev's state.
fn reading(alignment: i64) -> &'static str {
    if alignment >= GOOD {
        "mostly on task"
    } else if alignment >= MIXED {
        "split between tasks and other pages"
    } else {
        "mostly on pages no task needed"
    }
}

/// What Jev is shown to choose the checkpoint's recommendation from: the
/// session's clock, the board, and the stretch as the verdicts read it --
/// numbers, and the same numbers already put in words, because the options
/// are written in words.
pub fn pick_state(
    session: &Session,
    elapsed_minutes: i64,
    tasks: &[Task],
    t: &Tally,
    alignment: i64,
    previous: Option<i64>,
) -> Value {
    let lane = |status: &str| -> Vec<&str> {
        tasks.iter().filter(|x| x.status == status).map(|x| x.title.as_str()).collect()
    };
    let past = session.intended_minutes.map(|m| (elapsed_minutes - m).max(0));
    let mut off: Vec<&JudgedPage> = t.pages.iter().filter(|p| p.verdict == "no_task").collect();
    off.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    let compared = previous.map(|p| {
        if alignment - p >= TREND_STEP {
            "better than at the previous check"
        } else if p - alignment >= TREND_STEP {
            "worse than at the previous check"
        } else {
            "about the same as at the previous check"
        }
    });
    json!({
        "session": {
            "intended_minutes": session.intended_minutes,
            "elapsed_minutes": elapsed_minutes,
            "minutes_past_intended": past,
        },
        "board": { "to_do": lane("planned"), "doing": lane("doing"), "done": lane("done") },
        "last_stretch": {
            "judged_minutes": ((t.aligned + t.off) / 60.0).round() as i64,
            "on_task_minutes": (t.aligned / 60.0).round() as i64,
            "off_task_minutes": (t.off / 60.0).round() as i64,
            "on_task_percent": alignment,
            "reading": reading(alignment),
            "most_time_on_task": lead(t).map(|x| x.title.as_str()),
            "pages_no_task_needed": off.iter().take(3).map(|p| p.name.as_str()).collect::<Vec<_>>(),
            "compared": compared,
        },
    })
}

pub const PICK_KEY: &str = "rec";

/// The checkpoint's one choice question, over the whole catalog.
pub fn pick_question() -> Value {
    json!({
        PICK_KEY: {
            "type": "choice",
            "instructions": "The user set themselves a stopping time for this work session and it \
                has just arrived. Given the clock in `session`, the tasks in `board` and how the \
                last stretch went in `last_stretch`, which of these should they do now? Choose on \
                the evidence, not on kindness: if the stretch went to the tasks, choose for that; \
                never choose an option for a problem the evidence does not show.",
            "criteria": recommendations::criteria(),
        }
    })
}

/// The one sentence under the advice: what in the stretch it answers to.
pub fn pick_note(t: &Tally, session: &Session, elapsed_minutes: i64) -> String {
    let past = session.intended_minutes.map(|m| elapsed_minutes - m).filter(|m| *m > 0);
    let mut note = format!("{} of {} judged went to tasks", dur(t.aligned), dur(t.aligned + t.off));
    if let Some(m) = past {
        note.push_str(&format!("; {m} min past your time"));
    }
    note.push('.');
    note
}

/// The row's snapshot.
pub fn judged(
    t: &Tally,
    risk: Vec<RiskTime>,
    trigger: &str,
    min: f64,
    pick: Option<Pick>,
) -> Judged {
    Judged {
        models: t.models.clone(),
        threshold: min,
        trigger: trigger.into(),
        tasks: t.tasks.clone(),
        pages: t.pages.clone(),
        aligned_seconds: t.aligned,
        off_seconds: t.off,
        unjudged_seconds: t.unjudged,
        risk,
        pick,
    }
}

/// Jev's choice as stored: the id the catalog accepted, and the spread.
pub fn pick(id: &str, probabilities: BTreeMap<String, f64>, confidence: f64) -> Pick {
    Pick { id: id.into(), probabilities, confidence }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Verdict;

    const ZEN: &str = "app.zen_browser.zen";

    fn obs(per_title: &[(&str, &[(&str, f64)])]) -> Observed {
        let per_title: BTreeMap<String, BTreeMap<String, f64>> = per_title
            .iter()
            .map(|(a, ts)| (a.to_string(), ts.iter().map(|(t, s)| (t.to_string(), *s)).collect()))
            .collect();
        let per_app = per_title.iter().map(|(a, ts)| (a.clone(), ts.values().sum())).collect();
        let active_seconds = per_title.values().flat_map(|ts| ts.values()).sum();
        Observed { per_app, per_title, active_seconds, afk_seconds: 0.0 }
    }

    fn task(id: i64, title: &str, status: &str) -> Task {
        Task {
            id,
            session_id: Some(1),
            title: title.into(),
            position: id,
            status: status.into(),
            carry_count: 0,
            notes: String::new(),
            due_date: None,
            priority: "medium".into(),
            labels: Vec::new(),
        }
    }

    fn judgement(ps: &[(i64, &str, Option<f64>)]) -> Judgement {
        Judgement {
            app: String::new(),
            page: String::new(),
            title: String::new(),
            verdicts: ps
                .iter()
                .map(|(id, title, p)| Verdict {
                    task_id: *id,
                    task_title: title.to_string(),
                    probability: *p,
                    source: p.map(|_| "model".into()),
                    model: p.map(|_| "jev-1.14.0".into()),
                    created_at: p.map(|_| "2026-10-27T10:00:00+00:00".into()),
                })
                .collect(),
            relevant: None,
            threshold: 0.7,
            no_tasks: ps.is_empty(),
            error: None,
        }
    }

    fn session(intended: Option<i64>) -> Session {
        Session {
            id: 1,
            started_at: "2026-10-27T09:00:00+00:00".into(),
            ended_at: None,
            close_reason: None,
            statement: String::new(),
            intended_minutes: intended,
            checkpoint_due_at: None,
            first_task: None,
            task_count: 0,
        }
    }

    #[test]
    fn one_page_under_several_titles_is_one_page_and_rollups_are_not_pages() {
        let p = pages(
            &obs(&[
                (ZEN, &[
                    ("(3) Feed | LinkedIn — Zen Browser", 300.0),
                    ("Feed | LinkedIn — Zen Browser", 120.0),
                    ("(other)", 60.0),
                ]),
                ("code", &[("main.rs - intentionality - Visual Studio Code", 600.0)]),
                ("unknown", &[("unknown", 30.0)]),
                ("intentionality", &[("Intentionality", 45.0)]),
            ]),
            MAX_PAGES,
        );
        assert_eq!(p.pages.len(), 2);
        // An editor's page is its project folder (subject.rs).
        assert_eq!(p.pages[0].key.1, "intentionality");
        let feed = &p.pages[1];
        assert_eq!((feed.key.1.as_str(), feed.seconds), ("feed | linkedin", 420.0));
        // The title that held the page longest is the one kept for appeals.
        assert_eq!(feed.title, "(3) Feed | LinkedIn — Zen Browser");
        // The rollup, the untitled window, and this app's own window.
        assert_eq!(p.rest_seconds, 135.0);
    }

    #[test]
    fn past_the_cap_is_rest_not_dropped() {
        let titles: Vec<(String, f64)> = (0..5).map(|i| (format!("t{i}"), 100.0 + i as f64)).collect();
        let refs: Vec<(&str, f64)> = titles.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        let p = pages(&obs(&[("code", &refs)]), 3);
        assert_eq!(p.pages.iter().map(|x| x.key.1.as_str()).collect::<Vec<_>>(), ["t4", "t3", "t2"]);
        assert_eq!(p.rest_seconds, 201.0);
    }

    fn three_pages() -> Pages {
        pages(
            &obs(&[
                ("code", &[("main.rs", 1200.0)]),
                (ZEN, &[("Feed | LinkedIn — Zen Browser", 600.0), ("Docs — Zen Browser", 300.0)]),
            ]),
            MAX_PAGES,
        )
    }

    #[test]
    fn a_page_is_excused_off_task_or_unjudged_and_the_bar_is_inclusive() {
        let p = three_pages(); // main.rs, feed | linkedin, docs
        let tasks = [task(1, "Ship it", "doing"), task(2, "Write docs", "done")];
        let js = vec![
            Some(judgement(&[(1, "Ship it", Some(0.7)), (2, "Write docs", Some(0.2))])),
            Some(judgement(&[(1, "Ship it", Some(0.1)), (2, "Write docs", Some(0.3))])),
            Some(judgement(&[(1, "Ship it", Some(0.2)), (2, "Write docs", None)])),
        ];
        let t = tally(&p, &js, &tasks, 0.7);
        assert_eq!(t.pages.iter().map(|x| x.verdict.as_str()).collect::<Vec<_>>(), ["excused", "no_task", "unjudged"]);
        assert_eq!((t.aligned, t.off, t.unjudged), (1200.0, 600.0, 300.0));
        assert_eq!(t.tasks[0].seconds, 1200.0);
        assert_eq!(t.tasks[1].seconds, 0.0);
        // The off-task page still says which task came closest.
        assert_eq!((t.pages[1].task_id, t.pages[1].probability), (Some(2), Some(0.3)));
        assert_eq!(alignment(&t), Some(67));
        assert_eq!(t.models, ["jev-1.14.0"]);
    }

    /// Nothing in Doing, as Today asks it: every page is off task, which is
    /// what the timer shows on each of them.
    #[test]
    fn with_no_tasks_every_page_is_off_task() {
        let p = three_pages();
        let js = vec![Some(judgement(&[])); 3];
        let t = tally(&p, &js, &[], 0.7);
        assert_eq!(t.pages.iter().map(|x| x.verdict.as_str()).collect::<Vec<_>>(), ["no_task"; 3]);
        assert_eq!((t.aligned, t.off, t.unjudged), (0.0, 2100.0, 0.0));
    }

    #[test]
    fn ties_go_up_the_board_and_a_failed_judgement_is_unjudged() {
        let p = three_pages();
        let tasks = [task(1, "A", "planned"), task(2, "B", "planned")];
        let js = vec![Some(judgement(&[(1, "A", Some(0.9)), (2, "B", Some(0.9))])), None, None];
        let t = tally(&p, &js, &tasks, 0.7);
        assert_eq!(t.pages[0].task_id, Some(1));
        assert_eq!(t.unjudged, 900.0);
        // 1200 of 2100 judged: over half, so it still says something.
        assert_eq!(alignment(&t), Some(100));
    }

    #[test]
    fn too_little_judged_is_no_alignment_and_says_why() {
        let p = three_pages();
        let tasks = [task(1, "A", "planned")];
        let mut failed = judgement(&[(1, "A", None)]);
        failed.error = Some("no TypeSafe key (~/.config/intentionality/typesafe_key)".into());
        let js = vec![Some(failed.clone()), Some(failed.clone()), Some(judgement(&[(1, "A", Some(0.1))]))];
        let t = tally(&p, &js, &tasks, 0.7);
        assert_eq!(alignment(&t), None);
        assert!(nothing_judged(&t).contains("no TypeSafe key"));
    }

    #[test]
    fn dropped_tasks_cannot_excuse_and_done_ones_can() {
        let tasks = [task(1, "a", "planned"), task(2, "b", "done"), task(3, "c", "dropped")];
        assert_eq!(judgeable(&tasks).iter().map(|t| t.id).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn the_at_risk_line_splits_flagged_time_by_verdict() {
        let o = obs(&[
            (ZEN, &[("Feed | LinkedIn — Zen Browser", 600.0), ("Jobs | LinkedIn — Zen Browser", 120.0)]),
            ("discord", &[("#general", 60.0)]),
            ("code", &[("main.rs", 900.0)]),
        ]);
        let p = pages(&o, MAX_PAGES);
        let tasks = [task(1, "Apply to 3 roles", "doing")];
        let js: Vec<Option<Judgement>> = p
            .pages
            .iter()
            .map(|pg| {
                let prob = match pg.key.1.as_str() {
                    "jobs | linkedin" => Some(0.9),
                    "feed | linkedin" | "main.rs" => Some(0.1),
                    _ => None, // #general: Jev could not be asked
                };
                Some(judgement(&[(1, "Apply to 3 roles", prob)]))
            })
            .collect();
        let t = tally(&p, &js, &tasks, 0.7);
        let apps = [
            RiskApp { id: 1, name: "linkedin.com".into() },
            RiskApp { id: 2, name: "discord".into() },
            RiskApp { id: 3, name: "x.com".into() },
        ];
        let r = risk(&o, &apps, &t);
        assert_eq!(r.len(), 2); // x.com had no time: not listed
        assert_eq!(
            (r[0].name.as_str(), r[0].seconds, r[0].excused_seconds, r[0].no_task_seconds),
            ("linkedin.com", 720.0, 120.0, 600.0)
        );
        assert_eq!((r[1].name.as_str(), r[1].unjudged_seconds), ("discord", 60.0));
    }

    #[test]
    fn the_words_follow_the_band_and_stay_short() {
        let p = three_pages();
        let tasks = [task(1, "Ship the analyses rework", "doing"), task(2, "Write docs", "planned")];
        let all = |a: f64, b: f64, c: f64| {
            vec![
                Some(judgement(&[(1, "Ship the analyses rework", Some(a)), (2, "Write docs", Some(0.0))])),
                Some(judgement(&[(1, "Ship the analyses rework", Some(b)), (2, "Write docs", Some(0.0))])),
                Some(judgement(&[(1, "Ship the analyses rework", Some(c)), (2, "Write docs", Some(0.0))])),
            ]
        };
        let good = tally(&p, &all(0.9, 0.9, 0.1), &tasks, 0.7);
        let (h, b) = word(&good, alignment(&good).unwrap(), Some(40));
        assert_eq!(h, "Mostly on “Ship the analyses rework”");
        assert!(b.starts_with("30m of 35m judged went to tasks: “Ship the analyses rework” 30m."), "{b}");
        assert!(b.contains("5m on pages no task needed — “Docs” 5m."), "{b}");
        assert!(b.ends_with("Up from 40 at the last check."), "{b}");

        let mixed = tally(&p, &all(0.9, 0.1, 0.1), &tasks, 0.7);
        assert_eq!(word(&mixed, alignment(&mixed).unwrap(), None).0, "Split between the board and elsewhere");
        let bad = tally(&p, &all(0.1, 0.1, 0.1), &tasks, 0.7);
        let (h, b) = word(&bad, alignment(&bad).unwrap(), Some(5));
        assert_eq!(h, "Mostly away from the board");
        assert!(b.starts_with("0s of 35m judged went to tasks."), "{b}");
        assert!(b.ends_with("About where the last check was (5)."), "{b}");
        for (h, b) in [word(&good, 86, Some(0)), word(&bad, 0, None)] {
            assert!(!h.to_lowercase().contains("time"), "{h}");
            assert!(b.split_whitespace().count() <= 60, "{b}");
        }
    }

    #[test]
    fn a_long_title_is_cut_inside_its_quotes() {
        let q = quote(&"x".repeat(100));
        assert_eq!(q.chars().count(), QUOTE_CHARS + 3);
        assert!(q.ends_with("…”"));
        assert_eq!(quote("  a \n b "), "“a b”");
    }

    #[test]
    fn the_checkpoint_state_carries_the_clock_the_board_and_the_stretch() {
        let p = three_pages();
        let tasks = [task(1, "Ship it", "doing"), task(2, "Docs", "done"), task(3, "Gone", "dropped")];
        let js = vec![
            Some(judgement(&[(1, "Ship it", Some(0.9)), (2, "Docs", Some(0.0))])),
            Some(judgement(&[(1, "Ship it", Some(0.1)), (2, "Docs", Some(0.0))])),
            Some(judgement(&[(1, "Ship it", Some(0.1)), (2, "Docs", Some(0.0))])),
        ];
        let t = tally(&p, &js, &judgeable(&tasks), 0.7);
        let a = alignment(&t).unwrap();
        let s = pick_state(&session(Some(60)), 72, &tasks, &t, a, Some(80));
        assert_eq!(s["session"]["minutes_past_intended"], 12);
        assert_eq!(s["board"]["doing"][0], "Ship it");
        assert_eq!(s["board"]["done"][0], "Docs");
        assert_eq!(s["last_stretch"]["on_task_minutes"], 20);
        assert_eq!(s["last_stretch"]["pages_no_task_needed"][0], "Feed | LinkedIn");
        assert_eq!(s["last_stretch"]["compared"], "worse than at the previous check");
        assert_eq!(s["last_stretch"]["most_time_on_task"], "Ship it");
        // Seconds on a task are not "most time on" it.
        let thin = Tally { aligned: 12.0, ..t.clone() };
        let thin_tasks = Tally {
            tasks: vec![TaskTime { task_id: 1, title: "Ship it".into(), seconds: 12.0 }],
            ..thin
        };
        assert!(pick_state(&session(Some(60)), 72, &tasks, &thin_tasks, 2, None)["last_stretch"]
            ["most_time_on_task"]
            .is_null());
        let open = pick_state(&session(None), 72, &tasks, &t, a, None);
        assert!(open["session"]["minutes_past_intended"].is_null());
        assert_eq!(pick_note(&t, &session(Some(60)), 72), "20m of 35m judged went to tasks; 12 min past your time.");
        let q = pick_question();
        assert_eq!(q[PICK_KEY]["type"], "choice");
        assert!(q[PICK_KEY]["criteria"].get("quiet_window").is_none());
    }

    #[test]
    fn the_snapshot_round_trips() {
        let p = three_pages();
        let tasks = [task(1, "Ship it", "doing")];
        let js = vec![Some(judgement(&[(1, "Ship it", Some(0.9))])), None, None];
        let t = tally(&p, &js, &tasks, 0.7);
        let snap = judged(&t, Vec::new(), "drift", 0.7, Some(pick("keep_going", BTreeMap::new(), 0.5)));
        let back: Judged = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
        assert_eq!(back, snap);
        // A row written before a field existed still reads.
        let old: Judged = serde_json::from_str(r#"{"trigger":"manual"}"#).unwrap();
        assert_eq!(old.trigger, "manual");
        assert!(old.pages.is_empty() && old.pick.is_none());
    }
}
