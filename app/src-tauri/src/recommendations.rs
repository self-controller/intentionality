//! The catalog of things to do when the time is up.
//!
//! Jev picks an `id` from this list (one choice question, judged on what
//! each entry `fits`) rather than advice being written fresh, for two
//! reasons. It keeps the advice stable across checkpoints — the same
//! situation gets the same recommendation, which is what makes it worth
//! trusting — and it puts every claim in one auditable place. Only the id is
//! stored on the analysis row, so rewording an entry (or replacing it with
//! something evidence-backed) applies to every checkpoint already recorded.
//!
//! `source` is the citation slot and is None on every entry today. These are
//! sensible defaults, not research findings, and the empty slot is there to
//! stay honest about which is which until real citations go in.

use serde::Serialize;

#[derive(Serialize, Clone, Copy)]
pub struct Recommendation {
    pub id: &'static str,
    /// What to do, in the imperative. Shown as the headline of the advice.
    pub advice: &'static str,
    /// Why it is worth doing. One sentence, no hedging.
    pub why: &'static str,
    /// Roughly how long it takes, for the screen to show. 0 = not timed.
    pub minutes: u32,
    /// Citation. None means "no evidence behind this yet" — say so, don't imply.
    pub source: Option<&'static str>,
    /// The situation this entry is for, in the terms the checkpoint's state
    /// uses. What Jev matches the stretch against; never shown.
    #[serde(skip)]
    pub fits: &'static str,
}

/// Offered to the model, in this order. `QUIET_WINDOW` is deliberately not in
/// here: it is the fallback for when there was nothing to judge, and letting
/// the model choose it would give it an escape hatch from every hard call.
pub const CATALOG: &[Recommendation] = &[
    Recommendation {
        id: "reset_break",
        advice: "Step away for five minutes with no screen, then start one card.",
        why: "A short break away from the machine breaks the pull of whatever \
              caught your attention, and coming back to a named card removes the \
              choice that usually restarts the drift.",
        minutes: 5,
        source: None,
        fits: "Most of the last stretch went to pages no task needed: attention drifted away from \
              the board.",
    },
    Recommendation {
        id: "pick_one",
        advice: "Close everything but one card and work only that for 25 minutes.",
        why: "The cost here is switching, not effort — a single card and a short \
              fixed run makes switching the thing you have to opt into.",
        minutes: 25,
        source: None,
        fits: "The last stretch was split: some time on tasks and a real share elsewhere, or time \
              spread thinly over several tasks.",
    },
    Recommendation {
        id: "shrink_scope",
        advice: "Pick the smallest card on the board and do only that.",
        why: "Nothing has started, which usually means the first step is too big \
              to begin rather than too hard to finish.",
        minutes: 15,
        source: None,
        fits: "Nothing on the board is in progress or done, and little of the last stretch went to \
              any task: the work has not started.",
    },
    Recommendation {
        id: "wrap_up",
        advice: "Resolve the board — move what is done to Done — and stop here.",
        why: "You said this was the stopping point and the work went where you \
              meant it to. Closing the board now is what makes the next session's \
              backlog true.",
        minutes: 5,
        source: None,
        fits: "Most of the last stretch went to tasks and there is finished work on the board: the \
              planned stopping point arrived with the work where it was meant to go.",
    },
    Recommendation {
        id: "keep_going",
        advice: "Take five minutes back, then continue on the same card.",
        why: "The work is going where you intended and the thread is still warm; \
              a short pause costs less than reloading it later.",
        minutes: 5,
        source: None,
        fits: "Most of the last stretch went to tasks and a task is still in progress, with the \
              session at or only a little past its intended length.",
    },
    Recommendation {
        id: "stop_for_today",
        advice: "Stop now rather than pushing on.",
        why: "This has run long and the last stretch was worse than the ones \
              before it — continuing tends to spend tomorrow rather than buy \
              anything today.",
        minutes: 0,
        source: None,
        fits: "The session has run well past its intended length and the last stretch went worse \
              than the check before it.",
    },
];

/// Shown when the checkpoint had nothing to judge: too little tracked activity,
/// or the check could not run at all. Never offered to the model.
pub const QUIET_WINDOW: Recommendation = Recommendation {
    id: "quiet_window",
    advice: "Decide for yourself whether to stop or keep going.",
    why: "There was too little tracked activity to say anything useful about the \
          last stretch, so this is the clock talking, not an assessment.",
    minutes: 0,
    source: None,
    fits: "",
};

/// Where an unrecognized id lands. A choice answers with one of the options
/// it was offered, but the id is still validated here: the catalog, not the
/// response, decides what can be stored.
pub const FALLBACK: Recommendation = CATALOG[1]; // pick_one

pub fn get(id: &str) -> Option<Recommendation> {
    if id == QUIET_WINDOW.id {
        return Some(QUIET_WINDOW);
    }
    CATALOG.iter().find(|r| r.id == id).copied()
}

/// Resolve what the model returned, falling back rather than failing.
pub fn resolve(id: &str) -> Recommendation {
    CATALOG.iter().find(|r| r.id == id).copied().unwrap_or(FALLBACK)
}

/// The options, as Jev's choice question offers them: each id with what it
/// asks of the user and the situation it is for.
pub fn criteria() -> serde_json::Value {
    CATALOG
        .iter()
        .map(|r| (r.id.to_string(), serde_json::json!({ "advice": r.advice, "fits_when": r.fits })))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jev_is_offered_every_entry_and_never_the_fallback() {
        let c = criteria();
        let ids: Vec<&str> = c.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(ids.len(), CATALOG.len());
        assert!(CATALOG.iter().all(|r| ids.contains(&r.id) && !r.fits.is_empty()));
        assert!(!ids.contains(&QUIET_WINDOW.id));
        assert_eq!(resolve("nonsense").id, FALLBACK.id);
        assert_eq!(get("quiet_window").map(|r| r.id), Some("quiet_window"));
    }
}
