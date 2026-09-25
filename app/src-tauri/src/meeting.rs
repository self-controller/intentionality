//! The meeting note taker: microphone -> transcript -> notes.
//!
//! `start` opens the microphone and returns immediately; a background task
//! then transcribes each chunk as it is cut and writes it to the store, so the
//! transcript survives the app being killed mid-meeting. `stop` closes the
//! microphone and returns; draining the tail chunk happens in a task behind
//! it, so the click is never held open by a network call.
//!
//! Stop does not write the notes. The meeting lands in 'done' with no summary,
//! the user reads and corrects the transcript, and **Write notes**
//! (`rerun_notes`) is what asks the model — once, from the transcript as they
//! left it.
//!
//! Nothing here is reachable from a timer. Capture begins on an explicit click
//! and ends on an explicit click — see the note in transcribe.rs about why.

use crate::error::{AppError, Result};
use crate::models::RecordingNow;
use crate::state::{AppState, MeetingHandle};
use crate::{claude, db, record, transcribe};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

/// Begin recording, optionally from a named PipeWire source, into a new
/// meeting.
pub async fn start(app: &AppHandle, target: Option<String>) -> Result<RecordingNow> {
    let state = app.state::<AppState>();
    let recorder = open_microphone(&state, target).await?;
    let id = {
        let conn = state.conn.lock().unwrap();
        db::start_meeting(&conn, state.session_id())?
    };
    Ok(begin(app, id, recorder, 0, String::new()))
}

/// Pick a finished meeting back up: the microphone opens on it again and new
/// chunks carry on after its last segment. Nothing else is special about it
/// from here on — it is an ordinary 'recording' row, and the next Stop leaves
/// the whole transcript, old part and new, ready to review and write up.
pub async fn resume(app: &AppHandle, meeting_id: i64, target: Option<String>) -> Result<RecordingNow> {
    let state = app.state::<AppState>();
    // After the microphone, like `start`: a pw-record that cannot start must
    // leave the meeting as it was, not claiming a recording that never began.
    // An error past this point drops the recorder, which kills pw-record.
    let recorder = open_microphone(&state, target).await?;
    let (first_seq, last_text) = {
        let conn = state.conn.lock().unwrap();
        if !db::meeting_resume(&conn, meeting_id)? {
            return Err(AppError::Other(
                "that meeting is not finished, or no longer exists".into(),
            ));
        }
        db::resume_point(&conn, meeting_id).inspect_err(|err| {
            // The row is already 'recording' with nothing feeding it, so it is
            // failed here rather than left for the next startup's sweep.
            let _ = db::meeting_failed(&conn, meeting_id, &err.to_string());
        })?
    };
    let _ = app.emit(
        "meeting:state",
        json!({"meeting_id": meeting_id, "state": "recording"}),
    );
    Ok(begin(app, meeting_id, recorder, first_seq, transcribe::carry(&last_text)))
}

/// The checks shared by a new recording and a resumed one, then the
/// microphone.
async fn open_microphone(state: &AppState, target: Option<String>) -> Result<record::Recorder> {
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
    record::start(target.as_deref()).await
}

/// Hand an open microphone to the tasks that drain it, and make it the
/// meeting being recorded.
fn begin(
    app: &AppHandle,
    id: i64,
    mut recorder: record::Recorder,
    first_seq: i64,
    context: String,
) -> RecordingNow {
    let state = app.state::<AppState>();
    let stopper = recorder.take_stopper();
    let levels = recorder.take_levels();
    // A task of its own, not a `select!` inside `capture`. That loop awaits
    // the upload of a 3.84 MB chunk in its body, and `select!` interleaves
    // awaits on the channels rather than work inside the body — so a
    // multiplexed meter would freeze for the whole round trip of every chunk,
    // exactly when someone is most likely to be wondering whether recording is
    // still alive. Separating them also makes "the level stream ending must
    // not stop chunks draining" structural instead of a rule to remember.
    tokio::spawn(forward_levels(app.clone(), id, levels));
    let capture = tokio::spawn(capture(app.clone(), id, recorder, first_seq, context));
    let since = db::now();
    state.set_meeting(MeetingHandle::new(id, since.clone(), stopper, capture));
    RecordingNow { meeting_id: id, since }
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
///
/// `first_seq` and `context` are 0 and empty for a new meeting; a resumed one
/// carries on after its last segment, with that segment's tail as context.
async fn capture(
    app: AppHandle,
    meeting_id: i64,
    mut recorder: record::Recorder,
    first_seq: i64,
    mut context: String,
) {
    let state = app.state::<AppState>();
    let mut seq = first_seq;

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

/// Close the microphone and return. The tail chunk is drained in a task of
/// its own.
///
/// Everything after `stop_capture` used to be awaited here, which made Stop
/// take as long as an OpenAI round trip for the tail chunk — during which the
/// click had produced no visible change at all and the recording indicator was
/// still running. The honest fix is to return when the thing the button names
/// has actually happened: the microphone is shut.
///
/// `ended_at` is stamped here, the moment recording stopped, but the row stays
/// 'recording' until the tail chunk has landed: the UI keeps the transcript
/// read-only for exactly that window, so nothing typed can race the last
/// segment. Then it moves to 'done' with no summary — stopped, notes not yet
/// written — and `meeting:done` tells the UI to hand the transcript over.
pub async fn stop(app: &AppHandle) -> Result<i64> {
    let state = app.state::<AppState>();
    // Taken, not read: two Stops racing means only one of them gets the handle.
    let Some(mut handle) = state.take_meeting() else {
        return Err(AppError::Other("no meeting is being recorded".into()));
    };
    let meeting_id = handle.id;

    // The microphone closes here. Everything below is wrap-up.
    handle.stop_capture();

    let stopped = {
        let conn = state.conn.lock().unwrap();
        db::meeting_stopped(&conn, meeting_id)?
    };
    if !stopped {
        return Err(AppError::Other("that meeting is no longer recording".into()));
    }

    // Closing the microphone makes the capture task drain its tail chunk and
    // finish; awaiting that task is what guarantees the last thing said is in
    // the transcript before it becomes editable.
    let capture = handle.take_capture();
    let app = app.clone();
    tokio::spawn(async move {
        if let Some(task) = capture {
            let _ = task.await;
        }
        {
            let state = app.state::<AppState>();
            let conn = state.conn.lock().unwrap();
            // Guarded: a sweep that already failed this row keeps it failed.
            if let Err(err) = db::meeting_state(&conn, meeting_id, "recording", "done") {
                eprintln!("meeting {meeting_id}: could not mark stopped: {err}");
            }
        }
        let _ = app.emit("meeting:done", meeting_id);
    });
    Ok(meeting_id)
}

/// One attachment read off disk, before anything has gone over the network.
enum Loaded {
    Text(String),
    /// A PDF or image, still to be uploaded. The media type is re-derived
    /// from these bytes, so what is declared always matches what is sent.
    Upload { bytes: Vec<u8>, media_type: &'static str },
}

/// Read the stored rows. A file that has gone missing under us is skipped
/// with a log line rather than failing the run: the meeting itself is still
/// worth writing up.
fn read_files(rows: Vec<db::FileRow>) -> Vec<(String, Loaded)> {
    let mut out = Vec::new();
    for row in rows {
        let loaded = match row.kind.as_str() {
            "text" | "office" => match row.extracted {
                Some(text) => Loaded::Text(text),
                None => continue,
            },
            "pdf" | "image" => match std::fs::read(&row.path) {
                Ok(bytes) => {
                    let media_type = if row.kind == "pdf" {
                        "application/pdf"
                    } else {
                        let Some(media_type) = crate::attach::image_media_type(&bytes) else {
                            eprintln!("meeting: {} is no longer a readable image", row.name);
                            continue;
                        };
                        media_type
                    };
                    Loaded::Upload { bytes, media_type }
                }
                Err(err) => {
                    eprintln!("meeting: attachment {} could not be read: {err}", row.name);
                    continue;
                }
            },
            other => {
                eprintln!("meeting: attachment {} has unknown kind {other}", row.name);
                continue;
            }
        };
        out.push((row.name, loaded));
    }
    out
}

/// Turn what was read into what the model takes, uploading PDFs and images
/// to the Files API on the way. Every id that comes back is pushed onto
/// `uploaded` as it arrives, so the caller can delete them all whether this
/// finishes or stops halfway.
///
/// An upload that fails fails the run, the same as an API error would: it
/// says nothing about the file and everything about the network or
/// the key, and writing notes without a file the user attached would quietly
/// be worse notes. Re-run notes is the fix.
async fn upload_files(
    loaded: Vec<(String, Loaded)>,
    uploaded: &mut Vec<String>,
) -> Result<Vec<claude::ContextFile>> {
    let mut files = Vec::new();
    for (name, loaded) in loaded {
        let content = match loaded {
            Loaded::Text(text) => claude::FileContent::Text(text),
            Loaded::Upload { bytes, media_type } => {
                let file_id = claude::upload_file(bytes, media_type)
                    .await
                    .map_err(|e| AppError::Other(format!("could not upload {name}: {e}")))?;
                uploaded.push(file_id.clone());
                if media_type == "application/pdf" {
                    claude::FileContent::Pdf { file_id }
                } else {
                    claude::FileContent::Image { file_id }
                }
            }
        };
        files.push(claude::ContextFile { name, content });
    }
    Ok(files)
}

/// Write the notes from the transcript as it stands. Split out so
/// `rerun_notes` has exactly one place to catch a failure and mark the
/// meeting, whatever went wrong.
///
/// The inputs are snapshotted at the top, in one lock: a note the user saves
/// while the model is thinking belongs to the next run, and **Re-run notes**
/// is how they ask for it.
async fn write_notes(app: &AppHandle, meeting_id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    let (raw, notes, rows, edited) = {
        let conn = state.conn.lock().unwrap();
        (
            db::transcript(&conn, meeting_id)?,
            db::meeting_notes_text(&conn, meeting_id)?,
            db::meeting_file_rows(&conn, meeting_id)?,
            // Read here, with the transcript itself: these notes are about to
            // be written from this exact text, and finish_meeting may only
            // clear the stale mark if nothing was edited in between.
            db::transcript_edited_at(&conn, meeting_id)?,
        )
    };
    // Off the lock and off the async worker both. Reading up to 50 MB is short
    // but it is genuinely blocking, and it sits in front of a call that can
    // take minutes -- there is no reason to hold a runtime thread for it.
    let loaded = tokio::task::spawn_blocking(move || read_files(rows))
        .await
        .map_err(|e| AppError::Other(format!("could not read the attachments: {e}")))?;
    // Only when there is nothing at all. A meeting with no audio but a page of
    // typed notes is a real thing to want notes from, so the old blanket
    // "nothing was transcribed" would now be wrong.
    if raw.trim().is_empty() && notes.trim().is_empty() && loaded.is_empty() {
        return Err(AppError::Other(
            "nothing to write notes from — no transcript, no notes, no files".into(),
        ));
    }

    // Before the lock, not under it: an upload is not a model call, and a
    // 25 MB PDF on a slow connection should not hold up a scheduled check.
    //
    // Whatever happens from here, the uploads are deleted before this
    // returns. A refusal, a timeout or a database error must not leave the
    // user's files sitting in the API workspace; the expiry set at upload is
    // only for the app dying mid-run.
    let mut uploaded = Vec::new();
    let result = match upload_files(loaded, &mut uploaded).await {
        Ok(files) => {
            let ctx = claude::MeetingContext { notes, files };
            notes_from(app, meeting_id, &raw, &ctx, edited.as_deref()).await
        }
        Err(err) => Err(err),
    };
    for id in &uploaded {
        claude::delete_file(id).await;
    }
    result
}

/// The model call and the write after it, with every input already in hand.
/// Split from `write_notes` so that its caller has one place to clean up the
/// uploads on any outcome.
async fn notes_from(
    app: &AppHandle,
    meeting_id: i64,
    raw: &str,
    ctx: &claude::MeetingContext,
    // What `transcript_edited_at` read when `raw` was taken.
    transcript_seen: Option<&str>,
) -> Result<()> {
    let state = app.state::<AppState>();

    // Shared with the two analysis timers: it exists to stop concurrent calls
    // to the model, and a meeting being written up as a scheduled check fires
    // is exactly that. Taken here and not around the recording, which can run
    // for hours.
    let _running = state.analysis_lock.lock().await;

    let notes = claude::summarize_meeting(raw, ctx).await?;

    let mut conn = state.conn.lock().unwrap();
    db::finish_meeting(
        &mut conn,
        meeting_id,
        &db::MeetingNotes {
            title: &notes.title,
            summary: &notes.summary,
            action_items: &notes.action_items,
            transcript_seen,
        },
    )
}

/// Write the notes for a stopped meeting, or re-write them. This is the only
/// way notes get written: Stop leaves the transcript for the user to review
/// first. The transcript is already stored, so this costs one model call and
/// no audio, and it reads the transcript, notes and files exactly as the user
/// has just left them.
pub async fn rerun_notes(app: &AppHandle, meeting_id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    if state.meeting_id() == Some(meeting_id) {
        return Err(AppError::Other("that meeting is still recording".into()));
    }
    // Move the row first, so the record bar shows the same wrap-up states it
    // shows after a Stop rather than sitting on the old failure.
    {
        let conn = state.conn.lock().unwrap();
        if !db::meeting_rerun(&conn, meeting_id)? {
            return Err(AppError::Other("that meeting is not finished yet".into()));
        }
    }
    let _ = app.emit(
        "meeting:state",
        json!({"meeting_id": meeting_id, "state": "summarizing"}),
    );

    let outcome = write_notes(app, meeting_id).await;
    if let Err(err) = &outcome {
        let conn = state.conn.lock().unwrap();
        let _ = db::meeting_failed(&conn, meeting_id, &err.to_string());
    }
    let _ = app.emit("meeting:done", meeting_id);
    outcome
}
