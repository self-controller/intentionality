//! A new card, filed under one of the labels there are, by asking Jev.
//!
//! A card created with no labels of its own is shown to Jev beside every
//! label and the tasks already filed under each, and gets at most one of
//! them. The gate does the same for the tasks typed on its welcome screen
//! (gate/autolabel.py). The questions are worded once in each language and
//! tests/test_autolabel.py holds the two to the same text, so a task typed at
//! the gate and one typed on the board are judged alike.
//!
//! One request: a Choice over the labels (which one; its probability is
//! shared out between labels that overlap, like "School" and a course's own
//! label) and a Noul per label (does it belong there at all; each answered on
//! its own, so all can be low). The label goes on only when Jev both picks
//! it (Choice probability >= MIN) and says it belongs (its Noul >= MIN): a
//! wrong label costs a removal, so a doubtful one is not put on. MIN was set
//! on the live store's own labelled tasks (2026-10-29, leave-one-out): at 0.7
//! every clear case was filed and every pick under 0.45 -- the wrong ones --
//! was not.
//!
//! Runs after the card is saved, never in its way: the editor closes at
//! once, and the label lands a moment later with a `task:labelled` event the
//! board refreshes on. A label put on by hand meanwhile wins.

use crate::jev::{self, Typed};
use crate::{db, state::AppState};
use serde_json::{json, Map, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Both probabilities must reach this. Kept equal to gate/autolabel.py's MIN.
pub const MIN: f64 = 0.7;
/// Titles shown per label, newest first. Kept equal to the default of
/// gate/store.py's label_examples.
pub const EXAMPLES: usize = 6;
/// Nobody is waiting on it; a slow answer still lands.
const TIMEOUT: Duration = Duration::from_secs(10);

const CHOICE_QUESTION: &str = "The user files each of their tasks under a label. Which of these \
labels should the task given in `task` (its title, and the user's notes on it if any) be filed \
under? Each label is described by its name and the tasks the user has already filed under it, \
newest first. If more than one label fits, choose the most specific: the one for that particular \
course, project or job rather than a broader one it also falls under.";
const NOUL_QUESTION: &str = "Does the task given in `task` (its title, and the user's notes on it \
if any) belong under the label given in this question's `label`, going by the label's name and \
the tasks the user has already filed under it?";
const CRITERIA_TRUE: &str = "The task is part of the course, project, job or area of life that the \
label's name names or that the tasks already filed under it share, so the user would file it \
there too.";
const CRITERIA_FALSE: &str = "The task belongs to a different course, project, job or area of \
life, or nothing in the label's name or its tasks connects the task to it.";

/// A label a task could go under, and titles already filed there.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub examples: Vec<String>,
}

/// The task as Jev is shown it. Empty notes are left out, not sent blank.
pub fn state(title: &str, notes: &str) -> Value {
    let mut task = json!({ "title": title.trim() });
    if !notes.trim().is_empty() {
        task["notes"] = json!(notes.trim());
    }
    json!({ "task": task })
}

/// The Choice under `label` and a Noul per candidate under `l<index>`. A
/// label nothing wears yet is offered by its name alone.
pub fn questions(candidates: &[Candidate]) -> Value {
    let criteria: Map<String, Value> = candidates
        .iter()
        .map(|c| {
            let about = if c.examples.is_empty() {
                Value::Null
            } else {
                json!({ "tasks_already_filed_here": c.examples })
            };
            (c.name.clone(), about)
        })
        .collect();
    let mut out = Map::new();
    out.insert(
        "label".into(),
        json!({ "type": "choice", "instructions": CHOICE_QUESTION, "criteria": criteria }),
    );
    for (index, c) in candidates.iter().enumerate() {
        let mut label = json!({ "name": c.name });
        if !c.examples.is_empty() {
            label["tasks_already_filed_here"] = json!(c.examples);
        }
        out.insert(
            format!("l{index}"),
            json!({
                "type": "noul",
                "instructions": { "question": NOUL_QUESTION, "label": label },
                "criteria": { "true": CRITERIA_TRUE, "false": CRITERIA_FALSE },
            }),
        );
    }
    Value::Object(out)
}

/// The label to put on, or None. Policy lives here, not in the model.
pub fn decide(response: &jev::Response, candidates: &[Candidate]) -> Option<String> {
    let Some(Typed::Choice { choice, probabilities, .. }) = response.answers.get("label") else {
        return None;
    };
    let index = candidates.iter().position(|c| &c.name == choice)?;
    let picked = probabilities.get(choice).copied().unwrap_or(0.0);
    let belongs = match response.answers.get(&format!("l{index}")) {
        Some(Typed::Noul(p)) => *p,
        _ => 0.0,
    };
    (picked >= MIN && belongs >= MIN).then(|| choice.clone())
}

/// Ask about a card just created and file it if Jev is sure. Spawned by
/// create_task; never fails, only logs: a card without a label is still a
/// card.
pub async fn sort(app: AppHandle, task_id: i64) {
    let read = {
        let state = app.state::<AppState>();
        let conn = state.conn.lock().unwrap();
        db::unlabelled_task(&conn, task_id)
            .and_then(|task| Ok((task, db::label_candidates(&conn, EXAMPLES)?)))
    };
    let ((title, notes), candidates) = match read {
        Ok((Some(task), candidates)) if !candidates.is_empty() => (task, candidates),
        Ok(_) => return,
        Err(e) => return eprintln!("intentionality: could not sort task {task_id}: {e}"),
    };
    let response = match jev::ask(state(&title, &notes), questions(&candidates), TIMEOUT).await {
        Ok(r) => r,
        Err(e) => return eprintln!("intentionality: could not sort task {task_id}: {e}"),
    };
    let Some(label) = decide(&response, &candidates) else {
        return eprintln!("intentionality: task {task_id} left unlabelled");
    };
    let filed = {
        let state = app.state::<AppState>();
        let conn = state.conn.lock().unwrap();
        db::file_under(&conn, task_id, &label)
    };
    match filed {
        Ok(true) => {
            eprintln!("intentionality: task {task_id} filed under {label:?}");
            let _ = app.emit("task:labelled", task_id);
        }
        Ok(false) => {}
        Err(e) => eprintln!("intentionality: could not file task {task_id}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn candidates() -> Vec<Candidate> {
        vec![
            Candidate { name: "School".into(), examples: vec!["HW 6".into(), "Quiz".into()] },
            Candidate { name: "Home".into(), examples: vec![] },
        ]
    }

    fn response(choice: &str, picked: f64, nouls: [f64; 2]) -> jev::Response {
        let probabilities = BTreeMap::from([
            ("School".to_string(), if choice == "School" { picked } else { 1.0 - picked }),
            ("Home".to_string(), if choice == "Home" { picked } else { 1.0 - picked }),
        ]);
        let mut answers = BTreeMap::from([(
            "label".to_string(),
            Typed::Choice { choice: choice.into(), probabilities, confidence: 0.5 },
        )]);
        answers.insert("l0".into(), Typed::Noul(nouls[0]));
        answers.insert("l1".into(), Typed::Noul(nouls[1]));
        jev::Response { model: "jev-1".into(), answers }
    }

    #[test]
    fn a_label_goes_on_only_when_jev_picks_it_and_says_it_belongs() {
        let c = candidates();
        assert_eq!(decide(&response("School", 0.9, [0.95, 0.1]), &c), Some("School".into()));
        assert_eq!(decide(&response("School", 0.7, [0.7, 0.1]), &c), Some("School".into()));
        // Split between two labels: neither is picked clearly.
        assert_eq!(decide(&response("School", 0.6, [0.95, 0.9]), &c), None);
        // Picked, but Jev doubts it belongs anywhere.
        assert_eq!(decide(&response("Home", 0.9, [0.1, 0.5]), &c), None);
        // The noul read is the picked label's own, not another's.
        assert_eq!(decide(&response("Home", 0.9, [0.95, 0.2]), &c), None);
    }

    #[test]
    fn an_answer_that_names_no_candidate_is_no_label() {
        let mut r = response("School", 0.9, [0.95, 0.1]);
        r.answers.insert(
            "label".into(),
            Typed::Choice { choice: "Gone".into(), probabilities: BTreeMap::new(), confidence: 1.0 },
        );
        assert_eq!(decide(&r, &candidates()), None);
        r.answers.remove("label");
        assert_eq!(decide(&r, &candidates()), None);
    }

    #[test]
    fn the_questions_carry_each_label_and_what_is_filed_there() {
        let q = questions(&candidates());
        assert_eq!(q["label"]["type"], "choice");
        assert_eq!(q["label"]["criteria"]["School"]["tasks_already_filed_here"][1], "Quiz");
        assert!(q["label"]["criteria"]["Home"].is_null());
        assert_eq!(q["l0"]["type"], "noul");
        assert_eq!(q["l0"]["instructions"]["label"]["name"], "School");
        assert_eq!(q["l0"]["instructions"]["label"]["tasks_already_filed_here"][0], "HW 6");
        assert_eq!(q["l1"]["instructions"]["label"], json!({ "name": "Home" }));
        assert_eq!(q["l1"]["criteria"]["true"], CRITERIA_TRUE);
        assert_eq!(q.as_object().unwrap().len(), 3);
    }

    /// Not run by default: asks the real Jev (billed) where a few titles would
    /// be filed among the live store's labels, read-only, and prints it.
    /// `cargo test live_sort -- --ignored --nocapture`; set
    /// INTENTIONALITY_SORT_TITLES="one|two" to pick the titles.
    #[test]
    #[ignore]
    fn live_sort() {
        let conn = rusqlite::Connection::open_with_flags(
            db::store_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let candidates = db::label_candidates(&conn, EXAMPLES).unwrap();
        let titles = std::env::var("INTENTIONALITY_SORT_TITLES")
            .unwrap_or_else(|_| "Regression Methods HW 8|Vacuum the hallway|Plan a trip".into());
        for title in titles.split('|') {
            let r = tauri::async_runtime::block_on(jev::ask(
                state(title, ""),
                questions(&candidates),
                TIMEOUT,
            ))
            .unwrap();
            let Some(Typed::Choice { choice, probabilities, .. }) = r.answers.get("label") else {
                panic!("no choice in {:?}", r.answers)
            };
            let index = candidates.iter().position(|c| &c.name == choice).unwrap();
            let Some(Typed::Noul(belongs)) = r.answers.get(&format!("l{index}")) else { panic!() };
            println!(
                "{title:?} -> {:?}  (picked {choice:?} at {:.2}, belongs {belongs:.2}, {})",
                decide(&r, &candidates),
                probabilities[choice],
                r.model
            );
        }
    }

    #[test]
    fn blank_notes_are_left_out() {
        assert_eq!(state(" HW 7 ", "  "), json!({ "task": { "title": "HW 7" } }));
        assert_eq!(state("HW 7", " ch. 3 "), json!({ "task": { "title": "HW 7", "notes": "ch. 3" } }));
    }
}
