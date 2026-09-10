//! Microphone capture for the meeting note taker.
//!
//! Shells out to `pw-record` (pipewire-utils) rather than linking an audio
//! crate: that would pull in an alsa or pulse system dependency, and the
//! binary that ships here is deliberately self-contained. One long-lived
//! process writes raw little-endian PCM to its stdout and this module slices
//! that byte stream into fixed-size chunks. Slicing in Rust rather than
//! respawning `pw-record` per chunk is the whole point — a respawn drops
//! whatever is said during the gap.
//!
//! Since 2026-09-10 the same read loop also *processes* that stream, 20 ms at
//! a time: automatic gain (gain.rs) and metering (level.rs) both run here
//! rather than on a finished chunk. A chunk arrives once every two minutes,
//! which is far too late to tell someone whether the microphone is hearing
//! them, and gain applied after the cut could not honestly be reported by a
//! live meter anyway.
//!
//! No network and no database here: this module only produces bytes and
//! numbers.

use crate::error::{AppError, Result};
use crate::gain::{Agc, FRAME_BYTES};
use crate::level::{self, Level, Meter};
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u32 = 1;
pub const BYTES_PER_SAMPLE: u32 = 2; // s16
pub const BYTES_PER_SECOND: u32 = SAMPLE_RATE * CHANNELS * BYTES_PER_SAMPLE;

/// Two minutes per chunk: long enough that the per-request overhead is
/// negligible and short enough that text appears while the meeting is still
/// running. 3.84 MB, comfortably inside the transcription API's 25 MB limit.
pub const CHUNK_SECONDS: u32 = 120;
pub const CHUNK_BYTES: usize = (BYTES_PER_SECOND * CHUNK_SECONDS) as usize;

/// How much of a read buffer to ask for at a time. Unrelated to chunk size —
/// just the granularity at which the pipe is drained.
const READ_BUF: usize = 32 * 1024;

/// Kills the microphone. Separate from `Recorder` because the recorder itself
/// is moved into the task that drains it, while Stop has to reach the child
/// process from the command handler. Dropping this also stops the recording,
/// so a lost handle cannot leave the microphone open.
pub struct Stopper(oneshot::Sender<()>);

impl Stopper {
    pub fn stop(self) {
        let _ = self.0.send(());
    }
}

/// One cut of audio plus what it sounded like. The statistics ride along with
/// the bytes so the log line can name the meeting and segment the chunk became
/// — the correlation that makes "nothing was transcribed" answerable later.
pub struct Chunk {
    pub pcm: Vec<u8>,
    pub levels: level::Summary,
}

pub struct Recorder {
    /// Processed PCM chunks, in order. Closes after the final (short) tail
    /// chunk, which is what lets a consumer drain to completion by looping
    /// until the channel ends rather than coordinating a separate shutdown
    /// signal.
    pub chunks: mpsc::Receiver<Chunk>,
    /// Live level windows at 10 Hz. A *separate* channel from the chunks, and
    /// a lossy one: see the `try_send` below.
    pub levels: mpsc::Receiver<Level>,
    stopper: Option<Stopper>,
}

impl Recorder {
    /// Take the stop handle out, before the recorder is moved into the task
    /// that consumes its chunks. Only the first call returns it.
    pub fn take_stopper(&mut self) -> Option<Stopper> {
        self.stopper.take()
    }

    /// Take the level stream out, so it can be forwarded by a task of its own
    /// while the chunks go to the transcriber. What is left behind is an
    /// already-closed receiver: a second caller gets a stream that ends at
    /// once rather than one that quietly competes for the same updates.
    pub fn take_levels(&mut self) -> mpsc::Receiver<Level> {
        let (_closed, placeholder) = mpsc::channel(1);
        std::mem::replace(&mut self.levels, placeholder)
    }
}

/// Start capturing from `target` — a PipeWire node serial or name, or `None`
/// for the system default source. Fails immediately if `pw-record` is missing,
/// which is the one failure worth surfacing as a dialog rather than a log line.
///
/// `async` despite awaiting nothing: spawning a `tokio::process::Command`
/// registers the child with the tokio reactor, which panics when there is no
/// runtime on the calling thread. Reached from a Tauri command that thread is
/// the GTK main thread, where the panic crosses an `extern "C"` frame and
/// aborts the whole process instead of unwinding into an error dialog. Taking
/// a future makes "call me from the runtime" a compile-time requirement on
/// the caller rather than something the first Start click discovers.
pub async fn start(target: Option<&str>) -> Result<Recorder> {
    let mut cmd = Command::new("pw-record");
    cmd.args([
        "--rate",
        &SAMPLE_RATE.to_string(),
        "--channels",
        &CHANNELS.to_string(),
        "--format",
        "s16",
        // Headerless PCM on stdout: this module writes its own WAV header
        // per chunk, so a container here would only have to be stripped.
        "--raw",
    ]);
    // A separate argument, never interpolated into the ones above: a node name
    // is user data by the time it comes back from the frontend, and pw-record
    // is spawned directly rather than through a shell precisely so it cannot
    // be anything but one argument.
    if let Some(target) = target {
        cmd.args(["--target", target]);
    }
    let mut child = cmd
        .arg("-")
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            AppError::Other(format!(
                "could not start pw-record ({e}) — is pipewire-utils installed?"
            ))
        })?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Other("pw-record produced no stdout".into()))?;

    // The child moves into its own small task so Stop can reach it after the
    // recorder has been handed to the consumer. A oneshot resolves on send and
    // on sender-drop alike, which is what makes a dropped Stopper close the
    // microphone too.
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = stop_rx.await;
        let _ = child.start_kill();
        let _ = child.wait().await; // reap, so no zombie outlives the meeting
    });

    // Bounded: if transcription falls far behind the microphone, block the
    // reader rather than growing a queue of audio without limit. pw-record
    // then blocks on its own pipe, which is the right back-pressure.
    let (tx, rx) = mpsc::channel::<Chunk>(4);
    // Levels get their own small channel and a `try_send`. A display that has
    // stopped reading must cost a dropped meter update and never a dropped
    // sample, so this one never blocks the reader.
    let (level_tx, level_rx) = mpsc::channel::<Level>(8);

    let mut agc = Agc::new();
    if !agc.enabled() {
        eprintln!("meeting: INTENTIONALITY_AGC=off — capturing without gain");
    }

    tokio::spawn(async move {
        let mut meter = Meter::new();
        let mut summary = level::Summary::default();
        let mut pending: Vec<u8> = Vec::with_capacity(CHUNK_BYTES);
        // Whole samples and whole frames only. `stdout.read` can return any
        // number of bytes, including an odd one that splits an s16 sample in
        // half, so what has not yet made a full 20 ms frame waits here.
        let mut frame: Vec<u8> = Vec::with_capacity(FRAME_BYTES);
        let mut buf = vec![0u8; READ_BUF];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break, // the child exited: Stop, or it died
                Ok(n) => {
                    let mut rest = &buf[..n];
                    while !rest.is_empty() {
                        let want = FRAME_BYTES - frame.len();
                        let take = want.min(rest.len());
                        frame.extend_from_slice(&rest[..take]);
                        rest = &rest[take..];
                        if frame.len() < FRAME_BYTES {
                            break;
                        }
                        process(&mut frame, &mut agc, &mut meter, &mut summary, &level_tx, &mut pending);
                    }
                    while pending.len() >= CHUNK_BYTES {
                        let carry = pending.split_off(CHUNK_BYTES);
                        let full = std::mem::replace(&mut pending, carry);
                        let levels = std::mem::take(&mut summary);
                        if tx.send(Chunk { pcm: full, levels }).await.is_err() {
                            return; // consumer gone
                        }
                    }
                }
                Err(err) => {
                    eprintln!("meeting: reading pw-record failed: {err}");
                    break;
                }
            }
        }
        // The last partial frame, so Stop does not lose the final fraction of
        // a second. An odd trailing byte is half a sample and is dropped.
        let whole = frame.len() - frame.len() % 2;
        frame.truncate(whole);
        if !frame.is_empty() {
            process(&mut frame, &mut agc, &mut meter, &mut summary, &level_tx, &mut pending);
        }
        if let Some(level) = meter.flush() {
            summary.add(&level);
            let _ = level_tx.try_send(level);
        }
        // The tail. Anything shorter than this is silence-length noise, not
        // speech, and transcribing it wastes a request.
        if pending.len() >= BYTES_PER_SECOND as usize {
            let _ = tx.send(Chunk { pcm: pending, levels: summary }).await;
        }
    });

    Ok(Recorder { chunks: rx, levels: level_rx, stopper: Some(Stopper(stop_tx)) })
}

/// One frame: decode, meter the raw input, apply gain, meter the output,
/// append the processed bytes. `frame` is left empty, ready to refill.
///
/// Deliberately not a method on anything — it is the seam the read loop and
/// the end-of-stream flush share, and having exactly one of them is what keeps
/// the tail from being processed differently from the body.
fn process(
    frame: &mut Vec<u8>,
    agc: &mut Agc,
    meter: &mut Meter,
    summary: &mut level::Summary,
    level_tx: &mpsc::Sender<Level>,
    pending: &mut Vec<u8>,
) {
    let raw: Vec<i16> = frame
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let mut out = raw.clone();
    let gain_db = agc.process(&mut out);
    if let Some(level) = meter.push(&raw, &out, gain_db) {
        summary.add(&level);
        // Lossy on purpose: a full channel means the display is behind, and
        // the correct thing to lose then is the stale update.
        let _ = level_tx.try_send(level);
    }
    pending.reserve(frame.len());
    for s in &out {
        pending.extend_from_slice(&s.to_le_bytes());
    }
    frame.clear();
}

/// A canonical 44-byte RIFF/WAVE header for mono 16-bit PCM, followed by the
/// samples. Built here rather than asking pw-record for a WAV container
/// because a chunk's length is only known once it has been cut.
pub fn wav(pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = BYTES_PER_SECOND;
    let block_align = (CHANNELS * BYTES_PER_SAMPLE) as u16;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // format: PCM
    out.extend_from_slice(&(CHANNELS as u16).to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&((BYTES_PER_SAMPLE * 8) as u16).to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_header_is_44_bytes_and_declares_the_payload() {
        let pcm = vec![0u8; 320];
        let out = wav(&pcm);
        assert_eq!(out.len(), 44 + 320);
        assert_eq!(&out[0..4], b"RIFF");
        assert_eq!(&out[8..12], b"WAVE");
        assert_eq!(&out[36..40], b"data");
        // RIFF size counts everything after the first 8 bytes.
        assert_eq!(u32::from_le_bytes(out[4..8].try_into().unwrap()), 36 + 320);
        assert_eq!(u32::from_le_bytes(out[40..44].try_into().unwrap()), 320);
    }

    #[test]
    fn the_header_describes_16khz_mono_s16() {
        let out = wav(&[]);
        assert_eq!(u16::from_le_bytes(out[20..22].try_into().unwrap()), 1); // PCM
        assert_eq!(u16::from_le_bytes(out[22..24].try_into().unwrap()), 1); // mono
        assert_eq!(u32::from_le_bytes(out[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(out[28..32].try_into().unwrap()), 32_000); // byte rate
        assert_eq!(u16::from_le_bytes(out[32..34].try_into().unwrap()), 2); // block align
        assert_eq!(u16::from_le_bytes(out[34..36].try_into().unwrap()), 16); // bits
    }

    /// Verified live against pw-record on 2026-09-08: a 2.000 s capture of
    /// `--rate 16000 --channels 1 --format s16 --raw -` produced 63 434 bytes
    /// of headerless PCM, i.e. 32 000 B/s after ~18 ms of stream setup.
    #[test]
    fn a_chunk_is_two_minutes_of_audio() {
        assert_eq!(BYTES_PER_SECOND, 32_000);
        assert_eq!(CHUNK_BYTES, 3_840_000);
        assert_eq!(CHUNK_BYTES as u32 / BYTES_PER_SECOND, 120);
    }

    /// The read loop's framing, exercised without a microphone: feed the same
    /// PCM in different-sized reads and the assembled stream must be identical
    /// byte for byte. Odd read sizes split s16 samples in half, which is the
    /// case that would otherwise swap the two bytes of a sample and turn
    /// speech into noise.
    fn drain(pcm: &[u8], reads: &[usize], agc_on: bool) -> (Vec<u8>, usize) {
        let (level_tx, mut level_rx) = mpsc::channel::<Level>(1024);
        let mut agc = Agc::with_enabled(agc_on);
        let mut meter = Meter::new();
        let mut summary = level::Summary::default();
        let mut pending = Vec::new();
        let mut frame: Vec<u8> = Vec::with_capacity(FRAME_BYTES);

        let mut at = 0;
        let mut sizes = reads.iter().cycle();
        while at < pcm.len() {
            let n = (*sizes.next().unwrap()).min(pcm.len() - at);
            let mut rest = &pcm[at..at + n];
            at += n;
            while !rest.is_empty() {
                let want = FRAME_BYTES - frame.len();
                let take = want.min(rest.len());
                frame.extend_from_slice(&rest[..take]);
                rest = &rest[take..];
                if frame.len() < FRAME_BYTES {
                    break;
                }
                process(&mut frame, &mut agc, &mut meter, &mut summary, &level_tx, &mut pending);
            }
        }
        let whole = frame.len() - frame.len() % 2;
        frame.truncate(whole);
        if !frame.is_empty() {
            process(&mut frame, &mut agc, &mut meter, &mut summary, &level_tx, &mut pending);
        }
        let mut levels = 0;
        while level_rx.try_recv().is_ok() {
            levels += 1;
        }
        (pending, levels)
    }

    /// Sixteen frames of a quiet tone, plus a stray odd byte at the end.
    fn sample_pcm() -> Vec<u8> {
        let mut pcm = Vec::new();
        for i in 0..(FRAME_BYTES / 2 * 16) {
            let v = ((i as f32 * 0.05).sin() * 600.0) as i16;
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        pcm.push(0x7f); // half a sample: must be dropped, not misread
        pcm
    }

    #[test]
    fn framing_is_identical_across_arbitrary_read_boundaries() {
        let pcm = sample_pcm();
        let (reference, _) = drain(&pcm, &[FRAME_BYTES], true);
        for reads in [
            vec![1usize],
            vec![3],
            vec![7, 1, 4095],
            vec![639],
            vec![641],
            vec![FRAME_BYTES * 5 + 1],
            vec![READ_BUF],
        ] {
            let (got, _) = drain(&pcm, &reads, true);
            assert_eq!(got, reference, "read pattern {reads:?} changed the output");
        }
    }

    #[test]
    fn processing_preserves_byte_count_and_drops_only_a_half_sample() {
        let pcm = sample_pcm();
        let (got, _) = drain(&pcm, &[7, 1, 4095], false);
        // AGC off: the bytes must come back exactly, minus the odd trailing
        // one that was never a whole sample.
        assert_eq!(got.len(), pcm.len() - 1);
        assert_eq!(got, pcm[..pcm.len() - 1]);
    }

    #[test]
    fn a_partial_final_frame_is_still_processed() {
        // Two and a half frames: the half must not be silently dropped.
        let mut pcm = vec![0u8; FRAME_BYTES * 2 + FRAME_BYTES / 2];
        for (i, b) in pcm.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let (got, _) = drain(&pcm, &[FRAME_BYTES], false);
        assert_eq!(got.len(), pcm.len());
        assert_eq!(got, pcm);
    }

    /// 10 Hz: sixteen frames is three whole windows plus a partial one that
    /// the flush does not reach here, because `drain` stops at the read loop.
    #[test]
    fn levels_arrive_once_per_five_frames() {
        let pcm = sample_pcm();
        let (_, levels) = drain(&pcm, &[FRAME_BYTES], true);
        assert_eq!(levels, 16 / level::FRAMES_PER_WINDOW);
    }
}
