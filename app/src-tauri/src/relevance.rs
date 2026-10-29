//! Is the page in front relevant to any task on the board? Jev is asked once
//! per page and task, the answer is cached in the `relevance` table, and the
//! at-risk timer stays off the screen for a page that at least one unfinished
//! task can excuse.
//!
//! The checks (check.rs) ask the very same question through the same cache
//! about every page of their stretch, which is what keeps the two in step: a
//! page the timer excuses counts as on-task in the next check, a page a check
//! judged is already known to the timer, and an appeal won in either place
//! holds in both.
//!
//! Policy lives here, not in the model: a row stores Jev's raw probability
//! and "relevant" is `probability >= threshold` at read time, so the
//! threshold (meta `relevance_min`, default 0.7) can be tuned without asking
//! anything again. The cache is keyed by the task's text rather than its id,
//! so a carried copy still hits and an edited task is judged afresh.

use crate::db::{self, NewVerdict, Precedent, VerdictRow};
use crate::error::Result;
use crate::models::{Judgement, Task, Verdict};
use crate::{jev, state::AppState, subject};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// At or above this a task excuses the page. The docs' starting point for a
/// yes; `meta.relevance_min` overrides it.
pub const DEFAULT_MIN: f64 = 0.7;
/// A page judged while the timer loop waits: short, so an unreachable
/// TypeSafe costs the switch-away one hide at most.
const JUDGE_TIMEOUT: Duration = Duration::from_secs(3);
/// The user is watching an appeal; give it longer before giving up.
const APPEAL_TIMEOUT: Duration = Duration::from_secs(10);
/// A check runs in the background, or behind a spinner the user started.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// Pages judged at once by `judge_many`. Enough that thirty fresh pages are a
/// few seconds; few enough that the timer's own request is never queued
/// behind a burst, and TypeSafe's rate limit stays out of reach.
const CONCURRENCY: usize = 3;

/// A window as the shell reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub app: String,
    pub title: String,
}

/// What a page is cached under and what a check calls it: subject.rs,
/// where an editor is its project folder and a terminal its directory.
pub use crate::subject::{page_key, page_title};

/// The cache key for a task: what Jev was shown of it. An edit to either
/// line changes the key, which is the point.
pub fn task_text(task: &Task) -> String {
    format!("{}\n{}", task.title.trim(), task.notes.trim())
}

const QUESTION: &str = "Would a person working on the task given in this question's `task` \
(its title, and the user's own notes on it) plausibly need the window described in `page` for \
that work right now?";
const QUESTION_APPEAL: &str = "The user explains in `appeal` why the window described in \
`page` serves the task given in this question's `task` (its title, and the user's own notes on \
it). Taking that explanation as true unless the window plainly contradicts it, would a person \
working on the task plausibly need this window right now?";
const CRITERIA_TRUE: &str = "The page or window is a tool, a source, a contact or a place to \
publish that the task calls for, or the task's title or `notes` name this site, tool, folder or \
material or what the page is about. A code editor or terminal working in a project or folder \
that the task names or describes, or in a folder inside it, is doing the task's own work. A \
browser page that is `part_of` a course, project or workspace the task names or describes is \
that work's own material, even when its own title is as generic as Files, Assignments or \
Settings. Or `precedents` holds a case where the user explained why a window like this one \
served this task or one like it, and the same reasoning holds for this window.";
const CRITERIA_FALSE: &str = "The page is unrelated to the task, or is merely on the same \
website as something the task needs: a feed, home page, inbox, notifications, \
recommendations or entertainment the task does not require; or a code editor or terminal \
working in a different project from the one the task is about; or a page that is part of a \
different course or project from the one the task is about.";

/// How much of one precedent's argument is sent. Twelve of these ride along
/// with every request, so a long one is cut rather than left out.
const PRECEDENT_CHARS: usize = 500;
/// How many won appeals are shown to Jev with each page, newest first.
const PRECEDENTS: i64 = 12;

/// The state every question is asked about: the page, and the appeals the
/// user has won before. The task rides inside each question, so one request
/// judges every task at once. The page's title is what the subject is about:
/// a browser's or an app's title as it was, an editor's or terminal's
/// project -- the thing the verdict is cached for, not the file that was
/// open first or the session that asked.
pub fn state(page: &Page, argument: Option<&[String]>, precedents: &[Precedent]) -> Value {
    let shown = match subject::of(&page.app, &page.title) {
        Some(s) => describe(&page.app, &s),
        None => json!({ "app": page.app, "title": page.title }),
    };
    let mut state = json!({ "page": shown });
    if let Some(argument) = argument {
        state["appeal"] = json!({ "user_explains": argument });
    }
    if !precedents.is_empty() {
        state["precedents"] = precedents
            .iter()
            .map(|p| {
                json!({
                    "window": page_title(&p.app, &p.title),
                    "task": p.task_title,
                    "user_explained": p.argument.chars().take(PRECEDENT_CHARS).collect::<String>(),
                })
            })
            .collect();
    }
    state
}

/// A window as named fields, per TypeSafe's guidance on state: what sort of
/// window, what it is about, for an editor or terminal the folder it is in
/// and the project's own one-line description, and for a browser tab with a
/// bare title its site and the page it sits under. What the machine could
/// not tell is left out rather than sent empty.
fn describe(app: &str, s: &subject::Subject) -> Value {
    let mut page = json!({ "app": app, "kind": s.kind, "title": s.about });
    if let Some(folder) = &s.folder {
        page["folder"] = json!(folder);
    }
    if let Some(p) = &s.project {
        page["project"] = json!({ "name": p.name });
        if let Some(about) = &p.about {
            page["project"]["about"] = json!(about);
        }
    }
    if let Some(place) = &s.place {
        page["site"] = json!(place.site);
        if let Some(above) = &place.part_of {
            page["part_of"] = json!(above);
        }
    }
    page
}

/// One noul per task, keyed `t<id>`, with the same wording and different
/// data -- the docs' pattern for a batch of like questions.
pub fn questions(tasks: &[&Task], appeal: bool) -> Value {
    let mut out = Map::new();
    for task in tasks {
        out.insert(
            format!("t{}", task.id),
            json!({
                "type": "noul",
                "instructions": {
                    "question": if appeal { QUESTION_APPEAL } else { QUESTION },
                    "task": { "title": task.title.trim(), "notes": task.notes.trim() },
                },
                "criteria": { "true": CRITERIA_TRUE, "false": CRITERIA_FALSE },
            }),
        );
    }
    Value::Object(out)
}

/// The verdict that excuses the page: the most probable at or above `min`.
/// Equal probabilities go to the task higher on the board.
pub fn decide(verdicts: &[Verdict], min: f64) -> Option<usize> {
    verdicts
        .iter()
        .enumerate()
        .filter(|(_, v)| v.probability.is_some_and(|p| p >= min))
        .rev() // max_by keeps the last of equals; reversed, that is the first
        .max_by(|a, b| a.1.probability.partial_cmp(&b.1.probability).unwrap())
        .map(|(i, _)| i)
}

/// What one look at the store gives: the tasks that could excuse a page,
/// what is already known about this page, and the bar.
struct Read {
    tasks: Vec<Task>,
    cached: HashMap<String, VerdictRow>,
    min: f64,
}

/// The sentence a judgement becomes under the clock, after an appeal. Jev
/// gives probabilities, not prose.
pub fn reply(j: &Judgement) -> String {
    if j.no_tasks {
        return "Nothing in Doing to argue for".into();
    }
    let pct = |p: Option<f64>| format!("{:.0}%", p.unwrap_or(0.0) * 100.0);
    if let Some(v) = j.relevant.map(|i| &j.verdicts[i]) {
        return format!("Convinced — {} for “{}”", pct(v.probability), v.task_title);
    }
    let best = j
        .verdicts
        .iter()
        .filter(|v| v.probability.is_some())
        .max_by(|a, b| a.probability.partial_cmp(&b.probability).unwrap());
    match (best, &j.error) {
        (_, Some(e)) => format!("Could not ask Jev: {e}"),
        (None, None) => "Not judged yet".into(),
        (Some(v), None) => format!(
            "Not convinced — best {} for “{}”, needs {}",
            pct(v.probability),
            v.task_title,
            pct(Some(j.threshold))
        ),
    }
}

/// The tasks that can excuse a page for the timer and the Dashboard: the
/// open session's Doing column. None open = none.
pub(crate) fn timer_tasks(conn: &Connection, session_id: Option<i64>) -> Result<Vec<Task>> {
    match session_id {
        Some(id) => db::doing_session_tasks(conn, id),
        None => Ok(Vec::new()),
    }
}

fn read(conn: &Connection, tasks: Vec<Task>, app: &str, page: &str) -> Result<Read> {
    let cached = db::relevance_for_page(conn, app, page)?
        .into_iter()
        .map(|row| (row.task_text.clone(), row))
        .collect();
    Ok(Read { tasks, cached, min: db::relevance_min(conn) })
}

fn verdict(task: &Task, row: Option<&VerdictRow>) -> Verdict {
    Verdict {
        task_id: task.id,
        task_title: task.title.clone(),
        probability: row.map(|r| r.probability),
        source: row.map(|r| r.source.clone()),
        model: row.map(|r| r.model.clone()),
        created_at: row.map(|r| r.created_at.clone()),
    }
}

fn assemble(page: &Page, key: (String, String), r: &Read, error: Option<String>) -> Judgement {
    let verdicts: Vec<Verdict> =
        r.tasks.iter().map(|t| verdict(t, r.cached.get(&task_text(t)))).collect();
    Judgement {
        app: key.0,
        page: key.1,
        title: page.title.clone(),
        relevant: decide(&verdicts, r.min),
        verdicts,
        threshold: r.min,
        no_tasks: r.tasks.is_empty(),
        error,
    }
}

/// The page as the store knows it now, without asking anything. For the
/// appeal panel's first render.
pub fn cached(conn: &Connection, session_id: Option<i64>, page: &Page) -> Result<Judgement> {
    cached_for(conn, timer_tasks(conn, session_id)?, page)
}

/// The page against the tasks given, from the cache alone. A task never
/// judged against it is a verdict without a probability.
pub fn cached_for(conn: &Connection, tasks: Vec<Task>, page: &Page) -> Result<Judgement> {
    let key = page_key(&page.app, &page.title);
    let r = read(conn, tasks, &key.0, &key.1)?;
    Ok(assemble(page, key, &r, None))
}

/// Judge a page against every task in Doing: cached verdicts as they are,
/// Jev asked once for the rest, the answers stored. Never fails -- an
/// unreachable model is a judgement with `error` set and the page unexcused,
/// which is the timer's default anyway.
pub async fn judge(app: AppHandle, page: Page) -> Judgement {
    let tasks = open_tasks(&app);
    judge_tasks(app, page, tasks, None, JUDGE_TIMEOUT).await
}

/// The user's case for the page, put to Jev with every task again. Stored
/// as `appeal`, so the next visit to the page remembers the argument won.
pub async fn appeal(app: AppHandle, page: Page, argument: Vec<String>) -> Judgement {
    let tasks = open_tasks(&app);
    judge_tasks(app, page, tasks, Some(argument), APPEAL_TIMEOUT).await
}

/// The timer's tasks, or the store's error carried as no tasks: `judge_tasks`
/// then answers with nothing to be relevant to, which shows as such.
fn open_tasks(app: &AppHandle) -> Result<Vec<Task>> {
    let state = app.state::<AppState>();
    let conn = state.conn.lock().unwrap();
    timer_tasks(&conn, state.session_id())
}

/// Every page judged against its own tasks, cache first, a few at a time.
/// `out[i]` is about `asks[i]`; a page whose judging task died is None.
pub async fn judge_many(app: &AppHandle, asks: Vec<(Page, Vec<Task>)>) -> Vec<Option<Judgement>> {
    let gate = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    let n = asks.len();
    for (i, (page, tasks)) in asks.into_iter().enumerate() {
        let (app, gate) = (app.clone(), gate.clone());
        set.spawn(async move {
            let _turn = gate.acquire_owned().await;
            (i, judge_tasks(app, page, Ok(tasks), None, CHECK_TIMEOUT).await)
        });
    }
    let mut out = vec![None; n];
    while let Some(done) = set.join_next().await {
        if let Ok((i, judgement)) = done {
            out[i] = Some(judgement);
        }
    }
    out
}

/// Judge a page against the tasks given, in their board order. The timer
/// and appeals pass the open session's unfinished tasks; a check passes
/// every task of its session that was not dropped, since time spent finishing
/// a task was time on it.
pub async fn judge_tasks(
    app: AppHandle,
    page: Page,
    tasks: Result<Vec<Task>>,
    argument: Option<Vec<String>>,
    timeout: Duration,
) -> Judgement {
    let key = page_key(&page.app, &page.title);
    let state_handle = app.state::<AppState>();
    let read = tasks.and_then(|tasks| {
        let conn = state_handle.conn.lock().unwrap();
        read(&conn, tasks, &key.0, &key.1)
    });
    let mut r = match read {
        Ok(r) => r,
        Err(e) => {
            return Judgement {
                app: key.0,
                page: key.1,
                title: page.title,
                verdicts: Vec::new(),
                relevant: None,
                threshold: DEFAULT_MIN,
                no_tasks: true,
                error: Some(e.to_string()),
            }
        }
    };
    if r.tasks.is_empty() {
        return assemble(&page, key, &r, None);
    }
    let appealing = argument.is_some();
    let ask: Vec<&Task> = r
        .tasks
        .iter()
        .filter(|t| appealing || !r.cached.contains_key(&task_text(t)))
        .collect();
    if ask.is_empty() {
        return assemble(&page, key, &r, None);
    }
    let precedents = {
        let conn = state_handle.conn.lock().unwrap();
        db::relevance_precedents(&conn, r.min, PRECEDENTS).unwrap_or_default()
    };
    let asked = jev::nouls(
        state(&page, argument.as_deref(), &precedents),
        questions(&ask, appealing),
        timeout,
    )
    .await;
    let answer = match asked {
        Ok(answer) => answer,
        Err(e) => return assemble(&page, key, &r, Some(e.to_string())),
    };
    let argument = argument.map(|a| a.join("\n\n"));
    let source = if appealing { "appeal" } else { "model" };
    let now = db::now();
    let mut fresh: Vec<VerdictRow> = Vec::new();
    for task in &ask {
        let Some(p) = answer.nouls.get(&format!("t{}", task.id)) else { continue };
        fresh.push(VerdictRow {
            task_text: task_text(task),
            task_title: task.title.clone(),
            probability: *p,
            source: source.into(),
            argument: argument.clone(),
            model: answer.model.clone(),
            created_at: now.clone(),
        });
    }
    // An appeal won for a task: every page Jev turned down for it is asked
    // again, now with this case among the precedents.
    let won: Vec<String> = fresh
        .iter()
        .filter(|row| appealing && row.probability >= r.min)
        .map(|row| row.task_text.clone())
        .collect();
    let stored = {
        let conn = state_handle.conn.lock().unwrap();
        fresh.iter().try_for_each(|row| {
            db::relevance_upsert(
                &conn,
                &NewVerdict {
                    app: &key.0,
                    page: &key.1,
                    title: &page.title,
                    task_text: &row.task_text,
                    task_title: &row.task_title,
                    probability: row.probability,
                    source: &row.source,
                    argument: row.argument.as_deref(),
                    model: &row.model,
                },
            )
        })
        // After the upsert: the appeal's own rows are not Jev's to reopen.
        .and_then(|()| db::relevance_reopen(&conn, &won, r.min))
    };
    if let Ok(n @ 1..) = stored {
        eprintln!("intentionality: appeal won; {n} turned-down page verdicts will be asked again");
    }
    // A verdict that could not be written still decides this look; the next
    // one asks again.
    let error = stored.err().map(|e| e.to_string());
    for row in fresh {
        r.cached.insert(row.task_text.clone(), row);
    }
    assemble(&page, key, &r, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: i64, title: &str, notes: &str) -> Task {
        Task {
            id,
            session_id: Some(1),
            title: title.into(),
            position: id,
            status: "planned".into(),
            carry_count: 0,
            notes: notes.into(),
            due_date: None,
            priority: "medium".into(),
            labels: Vec::new(),
        }
    }

    fn v(p: Option<f64>) -> Verdict {
        Verdict {
            task_id: 1,
            task_title: "t".into(),
            probability: p,
            source: None,
            model: None,
            created_at: None,
        }
    }

    #[test]
    fn the_request_has_the_documented_shape() {
        let page = Page { app: "app.zen_browser.zen".into(), title: "Feed | LinkedIn".into() };
        let tasks = [task(7, "Apply to 3 roles", "senior rust"), task(9, "Write ch. 3", "")];
        let refs: Vec<&Task> = tasks.iter().collect();
        let s = state(&page, None, &[]);
        assert_eq!(s["page"]["title"], "Feed | LinkedIn");
        assert_eq!(s["page"]["kind"], "browser tab");
        assert!(s["page"].get("folder").is_none() && s["page"].get("project").is_none());
        assert!(s.get("appeal").is_none());
        // A window never judged is still described by what it says.
        let own = state(&Page { app: "intentionality".into(), title: "intentionality".into() }, None, &[]);
        assert_eq!(own["page"], json!({ "app": "intentionality", "title": "intentionality" }));
        let q = questions(&refs, false);
        assert_eq!(q["t7"]["type"], "noul");
        assert_eq!(q["t7"]["instructions"]["task"]["notes"], "senior rust");
        assert_eq!(q["t9"]["instructions"]["task"]["title"], "Write ch. 3");
        assert_eq!(q["t7"]["instructions"]["question"], QUESTION);
        assert_eq!(q["t7"]["criteria"]["true"], CRITERIA_TRUE);
        assert_eq!(q.as_object().unwrap().len(), 2);
        // An appeal carries the argument and asks the appeal question.
        let argued = state(&page, Some(&["I am applying there".to_string()]), &[]);
        assert_eq!(argued["appeal"]["user_explains"][0], "I am applying there");
        assert_eq!(questions(&refs, true)["t7"]["instructions"]["question"], QUESTION_APPEAL);
        // An editor is asked about as its project, never the file in front.
        let code = Page { app: "code".into(), title: "calendar.ts - cereal - Visual Studio Code".into() };
        assert_eq!(
            state(&code, None, &[])["page"]["title"],
            "Visual Studio Code with the project folder \"cereal\" open"
        );
    }

    /// A terminal in a project: Jev sees where, and what the project is --
    /// never the session's topic, since the verdict is the project's.
    #[test]
    fn a_terminal_is_shown_with_its_folder_and_project() {
        let s = crate::subject::Subject {
            key: ("org.gnome.ptyxis".into(), "~/desktop/projects/intentionality".into()),
            name: "Terminal: intentionality".into(),
            about: "A terminal working in the project \"intentionality\"".into(),
            kind: "terminal",
            folder: Some("~/Desktop/projects/intentionality/app".into()),
            project: Some(crate::folder::Project {
                root: "/home/d/Desktop/projects/intentionality".into(),
                name: "intentionality".into(),
                about: Some("A gate that sits between login and your desktop.".into()),
            }),
            place: None,
        };
        assert_eq!(
            describe("org.gnome.Ptyxis", &s),
            json!({
                "app": "org.gnome.Ptyxis",
                "kind": "terminal",
                "title": "A terminal working in the project \"intentionality\"",
                "folder": "~/Desktop/projects/intentionality/app",
                "project": {
                    "name": "intentionality",
                    "about": "A gate that sits between login and your desktop."
                }
            })
        );
        // A project with no README sends its name alone.
        let bare = crate::subject::Subject {
            project: Some(crate::folder::Project { about: None, ..s.project.clone().unwrap() }),
            ..s
        };
        assert_eq!(describe("x", &bare)["project"], json!({ "name": "intentionality" }));
    }

    /// A Canvas tab titled "Files": Jev sees which course it is the files of.
    #[test]
    fn a_bare_tab_is_shown_with_its_site_and_the_page_above() {
        let s = crate::subject::Subject {
            key: ("app.zen_browser.zen".into(), "files · 2026fa - intro data science 01:198:439:09".into()),
            name: "Files · 2026FA - INTRO DATA SCIENCE 01:198:439:09".into(),
            about: "Files — Zen Browser".into(),
            kind: "browser tab",
            folder: None,
            project: None,
            place: Some(crate::places::Place {
                site: "rutgers.instructure.com".into(),
                part_of: Some("2026FA - INTRO DATA SCIENCE 01:198:439:09".into()),
            }),
        };
        assert_eq!(
            describe("app.zen_browser.zen", &s),
            json!({
                "app": "app.zen_browser.zen",
                "kind": "browser tab",
                "title": "Files — Zen Browser",
                "site": "rutgers.instructure.com",
                "part_of": "2026FA - INTRO DATA SCIENCE 01:198:439:09"
            })
        );
    }

    /// A case won once is shown with every later page, so the same reason
    /// can excuse the next tab like it. None won: no field at all.
    #[test]
    fn won_appeals_ride_along_as_precedents() {
        let page = Page { app: "app.zen_browser.zen".into(), title: "Rust borrow error - Stack Overflow".into() };
        assert!(state(&page, None, &[]).get("precedents").is_none());
        let won = Precedent {
            app: "app.zen_browser.zen".into(),
            title: "(2) lifetimes in closures - Stack Overflow — Zen Browser".into(),
            task_title: "Fix the timer loop".into(),
            argument: "I look up compiler errors there while fixing it. ".repeat(20),
        };
        let s = state(&page, None, &[won]);
        let p = &s["precedents"][0];
        assert_eq!(p["window"], "lifetimes in closures - Stack Overflow");
        assert_eq!(p["task"], "Fix the timer loop");
        assert_eq!(p["user_explained"].as_str().unwrap().chars().count(), PRECEDENT_CHARS);
        // The criteria tell Jev what the notes and the precedents are for.
        assert!(CRITERIA_TRUE.contains("`notes`") && CRITERIA_TRUE.contains("`precedents`"));
    }

    #[test]
    fn an_appeal_is_answered_in_one_line() {
        let judged = |ps: &[(f64, &str)], relevant: Option<usize>, error: Option<&str>| Judgement {
            app: String::new(),
            page: String::new(),
            title: String::new(),
            verdicts: ps
                .iter()
                .map(|(p, t)| Verdict { probability: Some(*p), task_title: (*t).into(), ..v(None) })
                .collect(),
            relevant,
            threshold: 0.7,
            no_tasks: ps.is_empty(),
            error: error.map(Into::into),
        };
        assert_eq!(reply(&judged(&[(0.2, "A"), (0.92, "B")], Some(1), None)), "Convinced — 92% for “B”");
        assert_eq!(
            reply(&judged(&[(0.4, "A"), (0.1, "B")], None, None)),
            "Not convinced — best 40% for “A”, needs 70%"
        );
        assert_eq!(reply(&judged(&[(0.4, "A")], None, Some("timed out"))), "Could not ask Jev: timed out");
        assert_eq!(reply(&judged(&[], None, None)), "Nothing in Doing to argue for");
    }

    #[test]
    fn the_best_verdict_over_the_bar_excuses_the_page_and_ties_go_up_the_board() {
        assert_eq!(decide(&[v(Some(0.2)), v(Some(0.9)), v(Some(0.75))], 0.7), Some(1));
        assert_eq!(decide(&[v(Some(0.69)), v(None)], 0.7), None);
        assert_eq!(decide(&[v(Some(0.8)), v(Some(0.8))], 0.7), Some(0));
        assert_eq!(decide(&[], 0.7), None);
        assert_eq!(decide(&[v(Some(0.7))], 0.7), Some(0)); // at the bar counts
    }

    #[test]
    fn the_cache_key_is_the_text_jev_saw() {
        assert_eq!(task_text(&task(1, "  Ship it ", " notes\n")), "Ship it\nnotes");
        assert_eq!(task_text(&task(1, "Ship it", "")), "Ship it\n");
    }
}
