//! The meeting note taker: microphone -> transcript -> notes.
//!
//! `start` opens the microphone and returns immediately; a background task
//! then transcribes each chunk as it is cut and writes it to the store, so the
//! transcript survives the app being killed mid-meeting and the summary at
//! Stop has only the tail chunk left to wait for. `stop` closes the microphone
//! and returns; draining that tail and asking the model for notes happen in a
//! task behind it, so the click is never held open by a network call.
//!
//! Nothing here is reachable from a timer. Capture begins on an explicit click
//! and ends on an explicit click — see the note in transcribe.rs about why.

use crate::error::{AppError, Result};
use crate::state::{AppState, MeetingHandle};
use crate::{claude, db, record, transcribe};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

/// Begin recording, optionally from a named PipeWire source. Returns the new
/// meeting's id.
pub async fn start(app: &AppHandle, target: Option<String>) -> Result<i64> {
    let state = app.state::<AppState>();
    if state.meeting_id().is_some() {
        return Err(AppError::Other("a meeting is already being recorded".into()));
    }

    // Every precondition before anything is opened or written: a missing key
    // or a mistyped model would otherwise only surface when the first chunk
    // failed, two minutes in.
    transcribe::check_key()?;
    transcribe::config()?;

    // The microphone next: if pw-record cannot start there should be no
    // meeting row left behind claiming a recording that never happened.
    let mut recorder = record::start(target.as_deref()).await?;
    let stopper = recorder.take_stopper();
    let levels = recorder.take_levels();

    let id = {
        let conn = state.conn.lock().unwrap();
        db::start_meeting(&conn, state.session_id())?
    };
    // A task of its own, not a `select!` inside `capture`. That loop awaits
    // the upload of a 3.84 MB chunk in its body, and `select!` interleaves
    // awaits on the channels rather than work inside the body — so a
    // multiplexed meter would freeze for the whole round trip of every chunk,
    // exactly when someone is most likely to be wondering whether recording is
    // still alive. Separating them also makes "the level stream ending must
    // not stop chunks draining" structural instead of a rule to remember.
    tokio::spawn(forward_levels(app.clone(), id, levels));
    let capture = tokio::spawn(capture(app.clone(), id, recorder));
    state.set_meeting(MeetingHandle::new(id, stopper, capture));
    Ok(id)
}

/// Emit `meeting:level` at the meter's natural 10 Hz until the microphone
/// closes. Ephemeral by design: nothing here is written to the store, because
/// a record of when a room was loud is still a record of the room.
async fn forward_levels(
    app: AppHandle,
    meeting_id: i64,
    mut levels: tokio::sync::mpsc::Receiver<crate::level::Level>,
) {
    while let Some(level) = levels.recv().await {
        let _ = app.emit("meeting:level", json!({"meeting_id": meeting_id, "level": level}));
    }
}

/// Transcribe chunks until the microphone closes. Each segment is written and
/// emitted as it lands — this loop is what makes the transcript appear during
/// the meeting rather than after it.
async fn capture(app: AppHandle, meeting_id: i64, mut recorder: record::Recorder) {
    let state = app.state::<AppState>();
    let mut seq: i64 = 0;
    let mut context = String::new();

    while let Some(chunk) = recorder.chunks.recv().await {
        // One line per chunk, levels only — never samples and never transcript
        // text. This is what makes "did the microphone hear anything?"
        // answerable after the fact, given that no audio is kept.
        if !chunk.levels.is_empty() {
            eprintln!("meeting {meeting_id}: chunk {seq} {}", chunk.levels.describe());
        }
        let text = match transcribe::transcribe(record::wav(&chunk.pcm), &context).await {
            Ok(text) => text,
            Err(err) => {
                // One failed chunk is two minutes lost; abandoning the meeting
                // would be all of it. The empty segment is left in place so
                // the gap is visible in the transcript rather than silent.
                eprintln!("meeting {meeting_id}: chunk {seq} not transcribed: {err}");
                String::new()
            }
        };
        {
            let conn = state.conn.lock().unwrap();
            if let Err(err) = db::add_segment(&conn, meeting_id, seq, &text) {
                eprintln!("meeting {meeting_id}: segment {seq} not saved: {err}");
            }
        }
        if !text.is_empty() {
            context = transcribe::carry(&text);
        }
        let _ = app.emit("meeting:segment", json!({"meeting_id": meeting_id, "seq": seq}));
        seq += 1;
    }
}

/// Close the microphone and return. The tail chunk and the notes are finished
/// off in a task of their own.
///
/// Everything after `stop_capture` used to be awaited here, which made Stop
/// take as long as an OpenAI round trip for the tail chunk plus a Claude call
/// for the notes — during which the click had produced no visible change at
/// all and the recording indicator was still running. It read as a Stop that
/// had not worked, and the honest fix is to return when the thing the button
/// names has actually happened: the microphone is shut.
///
/// The row is moved to 'summarizing' *before* the drain rather than after it,
/// so the UI has a state to show for that whole window and `ended_at` is the
/// moment recording stopped rather than the moment the model replied.
///
/// `meeting:done` still fires either way: a meeting whose summary failed keeps
/// its transcript, and the UI has to stop waiting on it regardless.
pub async fn stop(app: &AppHandle) -> Result<i64> {
    let state = app.state::<AppState>();
    // Taken, not read: two Stops racing means only one of them gets the handle.
    let Some(mut handle) = state.take_meeting() else {
        return Err(AppError::Other("no meeting is being recorded".into()));
    };
    let meeting_id = handle.id;

    // The microphone closes here. Everything below is wrap-up.
    handle.stop_capture();

    let moved = {
        let conn = state.conn.lock().unwrap();
        db::meeting_summarizing(&conn, meeting_id)?
    };
    if !moved {
        return Err(AppError::Other("that meeting is no longer recording".into()));
    }
    let _ = app.emit("meeting:state", json!({"meeting_id": meeting_id, "state": "summarizing"}));

    // Closing the microphone makes the capture task drain its tail chunk and
    // finish; awaiting that task is what guarantees the last thing said is in
    // the transcript the model gets — so it is awaited in here, off the click.
    let capture = handle.take_capture();
    let app = app.clone();
    tokio::spawn(async move {
        if let Some(task) = capture {
            let _ = task.await;
        }
        let outcome = summarize(&app, meeting_id).await;
        if let Err(err) = &outcome {
            let state = app.state::<AppState>();
            let conn = state.conn.lock().unwrap();
            let _ = db::meeting_failed(&conn, meeting_id, &err.to_string());
        }
        let _ = app.emit("meeting:done", meeting_id);
    });
    Ok(meeting_id)
}

/// Ask the model for notes and store them. Split out so `stop` has exactly one
/// place to catch a failure and mark the meeting, whatever went wrong.
async fn summarize(app: &AppHandle, meeting_id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    let transcript = {
        let conn = state.conn.lock().unwrap();
        db::transcript(&conn, meeting_id)?
    };
    if transcript.trim().is_empty() {
        return Err(AppError::Other(
            "nothing was transcribed — check the microphone and the OpenAI key".into(),
        ));
    }

    // Shared with the two analysis timers: it exists to stop concurrent calls
    // to the model, and a meeting ending as a scheduled check fires is exactly
    // that. Taken here and not around the recording, which can run for hours.
    let notes = {
        let _running = state.analysis_lock.lock().await;
        claude::summarize_meeting(&transcript).await?
    };

    let mut conn = state.conn.lock().unwrap();
    db::finish_meeting(
        &mut conn,
        meeting_id,
        &db::MeetingNotes {
            title: &notes.title,
            summary: &notes.summary,
            key_points: &notes.key_points,
            action_items: &notes.action_items,
        },
    )
}

/// Re-run the notes for a meeting whose summary failed. The transcript is
/// already stored, so this costs one model call and no audio.
pub async fn resummarize(app: &AppHandle, meeting_id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    if state.meeting_id() == Some(meeting_id) {
        return Err(AppError::Other("that meeting is still recording".into()));
    }
    let outcome = summarize(app, meeting_id).await;
    if let Err(err) = &outcome {
        let conn = state.conn.lock().unwrap();
        let _ = db::meeting_failed(&conn, meeting_id, &err.to_string());
    }
    let _ = app.emit("meeting:done", meeting_id);
    outcome
}
