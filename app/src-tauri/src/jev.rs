//! The TypeSafe client. Jev is a System One model: given some state and a
//! set of questions it answers each with a probability, never with text --
//! which is what lets it sit inside the timer loop, and lets the app word
//! every check itself. A window title is untrusted input, and here the worst
//! it can do is sway its own verdict.
//!
//! The timer and the checks ask nouls (is this page relevant to that task?),
//! one request per page, and the cache in the `relevance` table keeps it to
//! that. The time-up checkpoint also asks one choice (which recommendation
//! fits?), and autolabel.rs a choice and a noul per label for each new card
//! saved without one (which label does it go under?). The API key never
//! leaves this module; the gate reads the same key file for its own copy of
//! that last question (gate/autolabel.py).

use crate::error::{AppError, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// An alias that moves with TypeSafe's releases. The versioned id that
/// actually answered comes back in every response and is stored with each
/// verdict, so a cache row always says which model it came from.
pub const MODEL: &str = "jev-latest";
const API_URL: &str = "https://api.typesafe.ai/v1/systemone";
/// Points the client somewhere else: a local stub for testing the whole
/// check without a key or a bill. Unset in normal use.
const API_URL_ENV: &str = "TYPESAFE_API_URL";
const KEY_FILE: &str = ".config/intentionality/typesafe_key";
/// 429 and 529 ask for a backoff; one short wait and one more try is all a
/// timer tick can afford.
const RETRY_AFTER: Duration = Duration::from_millis(400);

fn api_key() -> Result<String> {
    let path = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(KEY_FILE);
    if let Ok(key) = std::fs::read_to_string(&path) {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    std::env::var("TYPESAFE_API_KEY")
        .map_err(|_| AppError::Other(format!("no TypeSafe key (~/{KEY_FILE})")))
}

/// Why pages, checks and new tasks cannot be judged, or None when they can. For the
/// header.
pub fn warning() -> Option<String> {
    api_key()
        .err()
        .map(|_| format!("relevance, checks and label filing off — no TypeSafe key (~/{KEY_FILE})"))
}

/// One typed answer, as the documented response carries it.
#[derive(Debug, Clone, PartialEq)]
pub enum Typed {
    /// The probability the answer is yes.
    Noul(f64),
    /// The most probable option, every option's probability, and how
    /// concentrated that distribution is (1 = all on one option).
    Choice { choice: String, probabilities: BTreeMap<String, f64>, confidence: f64 },
}

/// One response: which model answered, and each question's answer under the
/// key the request gave it.
#[derive(Debug, PartialEq)]
pub struct Response {
    pub model: String,
    pub answers: BTreeMap<String, Typed>,
}

/// A response whose every answer is a noul: what the relevance questions get.
#[derive(Debug, PartialEq)]
pub struct Answer {
    pub model: String,
    pub nouls: BTreeMap<String, f64>,
}

/// Ask Jev a set of noul questions about `state`. They are answered in
/// parallel, so ten questions cost about what one does in time.
pub async fn nouls(state: Value, questions: Value, timeout: Duration) -> Result<Answer> {
    let response = ask(state, questions, timeout).await?;
    let mut nouls = BTreeMap::new();
    for (key, answer) in response.answers {
        match answer {
            Typed::Noul(p) => nouls.insert(key, p),
            Typed::Choice { .. } => {
                return Err(AppError::Other(format!("TypeSafe answer {key} is not a noul")))
            }
        };
    }
    Ok(Answer { model: response.model, nouls })
}

/// Ask any mix of questions about `state`, one request.
pub async fn ask(state: Value, questions: Value, timeout: Duration) -> Result<Response> {
    let key = api_key()?;
    let url = std::env::var(API_URL_ENV).unwrap_or_else(|_| API_URL.to_string());
    let body = json!({ "model": MODEL, "state": state, "questions": questions });
    let client = crate::http::remote().map_err(|e| AppError::Other(e.to_string()))?;
    let mut retried = false;
    loop {
        let resp = client
            .post(&url)
            .timeout(timeout)
            .bearer_auth(&key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("TypeSafe unreachable: {e}")))?;
        let status = resp.status();
        if matches!(status.as_u16(), 429 | 529) && !retried {
            retried = true;
            tokio::time::sleep(RETRY_AFTER).await;
            continue;
        }
        let payload: Value = resp
            .json()
            .await
            .map_err(|e| AppError::Other(format!("bad TypeSafe response: {e}")))?;
        if !status.is_success() {
            return Err(AppError::Other(describe(status.as_u16(), &payload)));
        }
        let answer = parse(&payload)?;
        note_model(&answer.model);
        return Ok(answer);
    }
}

fn describe(status: u16, payload: &Value) -> String {
    match status {
        401 => format!("TypeSafe refused the key in ~/{KEY_FILE}"),
        429 => "TypeSafe rate limit reached".into(),
        529 => "TypeSafe is overloaded".into(),
        _ => {
            let detail = match &payload["detail"] {
                Value::String(s) => s.clone(),
                Value::Null => "unknown error".into(),
                other => other.to_string(),
            };
            format!("TypeSafe {status}: {detail}")
        }
    }
}

/// The documented shapes: `{"model": "jev-1.14.0", "answers": {<key>:
/// {"type": "noul", "noul": 0.83}}, "usage": {...}}`, and for a choice
/// `{"type": "choice", "choice": "a", "confidence": 0.9, "probabilities":
/// {"a": 0.95, "b": 0.05}}`. Those are the two this app asks; anything else
/// is an error. Probabilities land clamped to [0, 1].
pub fn parse(payload: &Value) -> Result<Response> {
    let model = payload["model"].as_str().unwrap_or(MODEL).to_string();
    let answers = payload["answers"]
        .as_object()
        .ok_or_else(|| AppError::Other("TypeSafe answered without answers".into()))?;
    let unit = |v: &Value| v.as_f64().filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0));
    let mut out = BTreeMap::new();
    for (key, answer) in answers {
        let bad = || AppError::Other(format!("TypeSafe answer {key} is not a noul or a choice"));
        let typed = match answer["type"].as_str() {
            Some("choice") => {
                let choice = answer["choice"].as_str().ok_or_else(bad)?.to_string();
                let probabilities: BTreeMap<String, f64> = answer["probabilities"]
                    .as_object()
                    .map(|ps| ps.iter().filter_map(|(k, v)| Some((k.clone(), unit(v)?))).collect())
                    .unwrap_or_default();
                let confidence = unit(&answer["confidence"]).unwrap_or(0.0);
                Typed::Choice { choice, probabilities, confidence }
            }
            // A noul is recognised by its value as well as its type, as
            // before: the type tag is what the docs show, the value is what
            // this code needs.
            _ => Typed::Noul(unit(&answer["noul"]).ok_or_else(bad)?),
        };
        out.insert(key.clone(), typed);
    }
    Ok(Response { model, answers: out })
}

/// Logged on change only: "which Jev was judging pages that afternoon?" is
/// then one grep of the journal.
fn note_model(model: &str) {
    static LAST: Mutex<Option<String>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if last.as_deref() != Some(model) {
        eprintln!("intentionality: judged by {model}");
        *last = Some(model.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_response_parses() {
        let payload = json!({
            "model": "jev-1.14.0",
            "answers": {
                "t1": { "type": "noul", "noul": 0.83 },
                "t2": { "type": "noul", "noul": 0.04 }
            },
            "usage": { "input_tokens": 360, "output_tokens": 39 }
        });
        assert_eq!(
            parse(&payload).unwrap(),
            Response {
                model: "jev-1.14.0".into(),
                answers: BTreeMap::from([("t1".into(), Typed::Noul(0.83)), ("t2".into(), Typed::Noul(0.04))]),
            }
        );
    }

    #[test]
    fn a_choice_parses_beside_a_noul() {
        let payload = json!({
            "model": "jev-1.14.0",
            "answers": {
                "rec": {
                    "type": "choice", "choice": "keep_going", "confidence": 0.62,
                    "probabilities": { "keep_going": 0.7, "wrap_up": 0.25, "pick_one": 0.05 }
                },
                "t1": { "type": "noul", "noul": 0.4 }
            }
        });
        let r = parse(&payload).unwrap();
        assert_eq!(r.answers["t1"], Typed::Noul(0.4));
        let Typed::Choice { choice, probabilities, confidence } = &r.answers["rec"] else {
            panic!("not a choice")
        };
        assert_eq!((choice.as_str(), *confidence), ("keep_going", 0.62));
        assert_eq!(probabilities.len(), 3);
        assert_eq!(probabilities["wrap_up"], 0.25);
    }

    #[test]
    fn an_answer_of_no_known_kind_is_an_error_and_an_odd_value_is_clamped() {
        assert!(parse(&json!({ "answers": { "t1": { "type": "choice" } } })).is_err());
        assert!(parse(&json!({ "answers": { "t1": { "type": "score", "score": 1.4 } } })).is_err());
        assert!(parse(&json!({ "model": "x" })).is_err());
        let clamped = parse(&json!({ "answers": { "t1": { "type": "noul", "noul": 1.2 } } })).unwrap();
        assert_eq!(clamped.answers["t1"], Typed::Noul(1.0));
        assert_eq!(clamped.model, MODEL); // no model named: the alias stands in
    }

    #[test]
    fn errors_are_worded_for_the_user() {
        assert!(describe(401, &json!({})).contains("typesafe_key"));
        assert_eq!(describe(422, &json!({ "detail": "questions: field required" })),
            "TypeSafe 422: questions: field required");
        assert_eq!(describe(400, &json!({ "detail": { "code": "max_tokens_exceeded" } })),
            "TypeSafe 400: {\"code\":\"max_tokens_exceeded\"}");
        assert_eq!(describe(500, &json!({})), "TypeSafe 500: unknown error");
    }
}
