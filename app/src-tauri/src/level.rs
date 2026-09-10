//! What the microphone is actually hearing, and what is actually uploaded.
//!
//! Meeting 1 produced one sentence from a 27-second recording and there was no
//! way to tell, at the time or afterwards, whether the microphone had heard
//! anything at all. This module is the answer to that: it measures the frames
//! going into the AGC and the frames coming out of it, separately, so the UI
//! can say "the room is quiet" rather than "the level looks fine" about a
//! signal that is only loud because it was amplified.
//!
//! RMS and peak, in dBFS. Deliberately no spectrum: a rolling history of these
//! windows is all the bar visualiser needs, and a Goertzel bank would be work
//! this diagnosis does not require.

use serde::Serialize;

/// Five 20 ms frames. 100 ms is a clean 10 Hz update: fast enough that a
/// bar strip traces speech rhythm, slow enough that it is not a per-frame
/// firehose through a Tauri event and a React tree.
pub const FRAMES_PER_WINDOW: usize = 5;

/// The bottom of the scale. True silence is -inf dBFS, which no display and no
/// JSON number wants; everything quieter than this is reported as this.
pub const FLOOR_DBFS: f32 = -100.0;

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Level {
    /// Before the AGC. The honest one — this is what the microphone heard.
    pub input_rms_dbfs: f32,
    /// After the AGC, i.e. what is uploaded for transcription.
    pub output_rms_dbfs: f32,
    /// After the AGC. RMS says nothing about headroom; this does.
    pub output_peak_dbfs: f32,
    /// How much of the output level is amplification rather than room.
    pub gain_db: f32,
}

/// Accumulates frames into 100 ms windows. One per recording, in the read
/// task, alongside the AGC — so a window is never split across a read.
#[derive(Default)]
pub struct Meter {
    frames: usize,
    acc: Acc,
}

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one frame's raw input, its processed output, and the gain applied.
    /// Returns a `Level` on every fifth frame and `None` otherwise.
    pub fn push(&mut self, input: &[i16], output: &[i16], gain_db: f32) -> Option<Level> {
        self.acc.add(input, output, gain_db);
        self.frames += 1;
        if self.frames < FRAMES_PER_WINDOW {
            return None;
        }
        self.frames = 0;
        Some(std::mem::take(&mut self.acc).level())
    }

    /// The partial window left at end of recording. Without this the last
    /// fraction of a second is silently discarded, which is exactly the part
    /// someone stopping a recording is looking at.
    pub fn flush(&mut self) -> Option<Level> {
        if self.frames == 0 {
            return None;
        }
        self.frames = 0;
        Some(std::mem::take(&mut self.acc).level())
    }
}

/// Sums of squares rather than an average of per-frame RMS values: averaging
/// RMS would under-report a window containing one loud frame among four quiet
/// ones, which is what a single word in a pause looks like.
#[derive(Default)]
struct Acc {
    samples: u64,
    input_sq: f64,
    output_sq: f64,
    output_peak: i32,
    gain_db_sum: f64,
    frames: u32,
}

impl Acc {
    fn add(&mut self, input: &[i16], output: &[i16], gain_db: f32) {
        for &s in input {
            let x = s as f64 / 32768.0;
            self.input_sq += x * x;
        }
        for &s in output {
            let x = s as f64 / 32768.0;
            self.output_sq += x * x;
            let a = (s as i32).abs();
            if a > self.output_peak {
                self.output_peak = a;
            }
        }
        self.samples += input.len() as u64;
        self.gain_db_sum += gain_db as f64;
        self.frames += 1;
    }

    fn level(self) -> Level {
        let n = self.samples.max(1) as f64;
        Level {
            input_rms_dbfs: dbfs((self.input_sq / n).sqrt()),
            output_rms_dbfs: dbfs((self.output_sq / n).sqrt()),
            output_peak_dbfs: dbfs(self.output_peak as f64 / 32768.0),
            gain_db: (self.gain_db_sum / self.frames.max(1) as f64) as f32,
        }
    }
}

/// Amplitude (1.0 = full scale) to dBFS, floored rather than -inf.
pub fn dbfs(amplitude: f64) -> f32 {
    if amplitude <= 0.0 {
        return FLOOR_DBFS;
    }
    ((20.0 * amplitude.log10()) as f32).max(FLOOR_DBFS)
}

/// Per-chunk statistics for the journal. One line every two minutes is enough
/// to answer "was the microphone hearing anything?" after the fact without
/// keeping audio — and note that even this reveals when a room was active, so
/// it stays a summary and never a sample.
#[derive(Clone, Debug)]
pub struct Summary {
    windows: u32,
    input_sq: f64,
    output_sq: f64,
    output_peak: f32,
    gain_max: f32,
    /// Gains, quantised to 0.5 dB, for the median. A histogram rather than a
    /// vector: a chunk is 6 000 frames and this runs for the whole meeting.
    gain_hist: [u32; GAIN_BINS],
}

/// 0.5 dB bins from 0 to +40 dB, which covers the +30 dB cap with room to
/// spare if it is ever raised.
const GAIN_BINS: usize = 81;

// Hand-written because `[u32; 81]` is past the array size std derives Default
// for; everything here starts at zero regardless.
impl Default for Summary {
    fn default() -> Self {
        Self {
            windows: 0,
            input_sq: 0.0,
            output_sq: 0.0,
            output_peak: FLOOR_DBFS,
            gain_max: 0.0,
            gain_hist: [0; GAIN_BINS],
        }
    }
}

impl Summary {
    pub fn add(&mut self, level: &Level) {
        let amp = |db: f32| 10f64.powf(db as f64 / 20.0);
        self.input_sq += amp(level.input_rms_dbfs).powi(2);
        self.output_sq += amp(level.output_rms_dbfs).powi(2);
        if self.windows == 0 || level.output_peak_dbfs > self.output_peak {
            self.output_peak = level.output_peak_dbfs;
        }
        if self.windows == 0 || level.gain_db > self.gain_max {
            self.gain_max = level.gain_db;
        }
        let bin = ((level.gain_db * 2.0).round().max(0.0) as usize).min(GAIN_BINS - 1);
        self.gain_hist[bin] += 1;
        self.windows += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.windows == 0
    }

    fn median_gain_db(&self) -> f32 {
        let half = self.windows / 2;
        let mut seen = 0;
        for (bin, &count) in self.gain_hist.iter().enumerate() {
            seen += count;
            if seen > half {
                return bin as f32 / 2.0;
            }
        }
        0.0
    }

    /// One line, no samples and no transcript text.
    pub fn describe(&self) -> String {
        let n = self.windows.max(1) as f64;
        format!(
            "raw {:.1} dBFS, out {:.1} dBFS, peak {:.1} dBFS, gain {:.1}/{:.1} dB (median/max)",
            dbfs((self.input_sq / n).sqrt()),
            dbfs((self.output_sq / n).sqrt()),
            self.output_peak,
            self.median_gain_db(),
            self.gain_max,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gain::FRAME_SAMPLES;

    fn flat(amplitude: f32) -> Vec<i16> {
        // A square wave: its RMS is exactly its amplitude, so the expected
        // dBFS is arithmetic rather than an approximation.
        (0..FRAME_SAMPLES)
            .map(|i| {
                let v = (amplitude * 32767.0).round() as i16;
                if i % 2 == 0 { v } else { -v }
            })
            .collect()
    }

    #[test]
    fn known_amplitudes_map_to_expected_dbfs() {
        assert!((dbfs(1.0) - 0.0).abs() < 0.01);
        assert!((dbfs(0.5) + 6.02).abs() < 0.01);
        assert!((dbfs(0.1) + 20.0).abs() < 0.01);
        assert!((dbfs(0.001) + 60.0).abs() < 0.01);
        assert_eq!(dbfs(0.0), FLOOR_DBFS);
        assert_eq!(dbfs(1e-12), FLOOR_DBFS);
    }

    #[test]
    fn a_window_is_five_frames_and_one_hundred_milliseconds() {
        let mut meter = Meter::new();
        let frame = flat(0.1);
        for _ in 0..FRAMES_PER_WINDOW - 1 {
            assert!(meter.push(&frame, &frame, 0.0).is_none());
        }
        assert!(meter.push(&frame, &frame, 0.0).is_some());
        assert_eq!(FRAMES_PER_WINDOW * FRAME_SAMPLES, 1_600); // 100 ms at 16 kHz
    }

    /// The whole point of measuring twice: a UI that could only see the output
    /// would show a healthy level for a room nobody could hear.
    #[test]
    fn input_and_output_are_reported_independently() {
        let mut meter = Meter::new();
        let quiet = flat(0.001); // -60 dBFS
        let loud = flat(0.1); // -20 dBFS
        let mut got = None;
        for _ in 0..FRAMES_PER_WINDOW {
            got = meter.push(&quiet, &loud, 40.0);
        }
        let level = got.expect("a window should have closed");
        assert!((level.input_rms_dbfs + 60.0).abs() < 0.2, "{level:?}");
        assert!((level.output_rms_dbfs + 20.0).abs() < 0.2, "{level:?}");
        assert!((level.gain_db - 40.0).abs() < 0.01);
    }

    #[test]
    fn peak_is_reported_separately_from_rms() {
        let mut meter = Meter::new();
        let mut frame = flat(0.01); // -40 dBFS RMS
        frame[7] = 32_767; // one sample at full scale
        let mut got = None;
        for _ in 0..FRAMES_PER_WINDOW {
            got = meter.push(&frame, &frame, 0.0);
        }
        let level = got.unwrap();
        // One sample in 320 at full scale lifts the window's RMS to about
        // -25 dBFS, but the peak is at 0 — a 25 dB gap that an RMS-only meter
        // could not show, and the reason headroom is reported separately.
        assert!(level.output_peak_dbfs > -0.1, "{level:?}");
        assert!(level.output_rms_dbfs < -20.0, "{level:?}");
        assert!(level.output_peak_dbfs - level.output_rms_dbfs > 20.0, "{level:?}");
    }

    #[test]
    fn silence_reports_the_floor_not_infinity() {
        let mut meter = Meter::new();
        let frame = vec![0i16; FRAME_SAMPLES];
        let mut got = None;
        for _ in 0..FRAMES_PER_WINDOW {
            got = meter.push(&frame, &frame, 0.0);
        }
        let level = got.unwrap();
        assert_eq!(level.input_rms_dbfs, FLOOR_DBFS);
        assert!(level.input_rms_dbfs.is_finite());
    }

    #[test]
    fn flush_emits_a_partial_window_once() {
        let mut meter = Meter::new();
        let frame = flat(0.1);
        assert!(meter.push(&frame, &frame, 0.0).is_none());
        assert!(meter.flush().is_some());
        assert!(meter.flush().is_none());
    }

    #[test]
    fn a_summary_reports_median_and_maximum_gain() {
        let mut summary = Summary::default();
        assert!(summary.is_empty());
        for db in [0.0, 6.0, 12.0, 12.0, 30.0] {
            summary.add(&Level {
                input_rms_dbfs: -50.0,
                output_rms_dbfs: -20.0,
                output_peak_dbfs: -6.0,
                gain_db: db,
            });
        }
        assert!(!summary.is_empty());
        assert_eq!(summary.median_gain_db(), 12.0);
        assert_eq!(summary.gain_max, 30.0);
        let line = summary.describe();
        assert!(line.contains("raw -50.0 dBFS"), "{line}");
        assert!(line.contains("12.0/30.0"), "{line}");
    }
}
