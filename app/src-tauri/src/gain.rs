//! Automatic gain for far-field capture.
//!
//! The internal laptop microphone is already at its hardware ceiling
//! (`Capture 70 [100%] [20.00dB]`, `pw-record --volume` caps at 1.0), so a
//! speaker across the room arrives far below what the transcription model
//! wants. This lifts that signal in the digital domain, one 20 ms frame at a
//! time, while the recording is running — early enough that the meter in
//! section 2 can report the level that is actually uploaded.
//!
//! What this is not: more information. If distant speech and room noise
//! arrived at the same signal-to-noise ratio, both are raised together. A
//! microphone closer to the speaker remains the real fix; see the README.
//!
//! Three properties keep this from making things worse:
//!
//! * gain is held, never raised, below a raw-input noise floor, so a silent
//!   room does not ratchet the gain up and then blast the first cough;
//! * boost is capped, so nothing is amplified without limit;
//! * a soft limiter runs before the s16 conversion, so a loud frame arriving
//!   while the gain is still high compresses instead of clipping.
//!
//! State lives for the whole recording — across reads and across chunk
//! boundaries — because a gain that reset every two minutes would audibly
//! pump at exactly the seams the transcript is stitched at.

/// 20 ms at 16 kHz mono s16: 320 samples, 640 bytes. Short enough to react
/// within a syllable, long enough that its RMS means something.
pub const FRAME_SAMPLES: usize = 320;
pub const FRAME_BYTES: usize = FRAME_SAMPLES * 2;

/// Where the AGC aims. -20 dBFS is conservative on purpose: it leaves ~20 dB
/// of headroom for the peaks an RMS target says nothing about.
const TARGET_RMS: f32 = 0.1; // -20 dBFS

/// The cap on boost. +30 dB turns a -50 dBFS whisper across a room into
/// something around -20 dBFS; past that the noise dominates anyway.
const MAX_GAIN: f32 = 31.622_777; // +30 dB

/// Never attenuate. A source that is already loud enough is left alone — the
/// limiter below is what protects it — so this stage can only ever be the
/// reason quiet audio got louder, never the reason good audio got quieter.
const MIN_GAIN: f32 = 1.0;

/// Below this *raw* input RMS the frame is treated as room tone and the gain
/// is frozen. -60 dBFS is beneath any speech that has a chance of being
/// transcribed and above the noise floor of a working microphone.
const NOISE_FLOOR: f32 = 0.001; // -60 dBFS

/// Per-frame one-pole coefficients. Attack (gain coming down) is fast so a
/// sudden loud talker is caught within ~100 ms; release (gain going up) is
/// slow so the room's silence between sentences is not pumped up into a roar.
const ATTACK: f32 = 0.35; // ~60 ms
const RELEASE: f32 = 0.015; // ~1.3 s

/// Where the soft limiter starts bending. Below this, samples pass through
/// untouched; above it they are compressed toward — but never onto — full
/// scale, so the s16 conversion below cannot wrap.
const LIMIT_KNEE: f32 = 0.75;

pub struct Agc {
    /// Off leaves samples untouched but keeps the metering path identical, so
    /// `INTENTIONALITY_AGC=off` is a controlled A/B and not a different
    /// program. Without it a misbehaving AGC — over-amplified room noise is a
    /// known way to make these models hallucinate — could only be diagnosed
    /// by rebuilding, since no audio is kept.
    enabled: bool,
    gain: f32,
}

impl Agc {
    /// Reads the escape hatch once, at construction: the switch is a property
    /// of a recording, not something that can flip mid-meeting.
    pub fn new() -> Self {
        let enabled = !matches!(
            std::env::var("INTENTIONALITY_AGC").unwrap_or_default().trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        );
        Self::with_enabled(enabled)
    }

    pub fn with_enabled(enabled: bool) -> Self {
        Self { enabled, gain: MIN_GAIN }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Process one frame in place. Returns the gain applied, in dB, which is
    /// what the meter reports so the display can distinguish "the room is
    /// loud" from "the AGC is working hard".
    ///
    /// A short frame (the tail at end of recording) is handled the same way;
    /// only its RMS is over fewer samples.
    pub fn process(&mut self, frame: &mut [i16]) -> f32 {
        let rms = rms(frame);
        if !self.enabled {
            return 0.0;
        }
        // Silence must not move the gain. Holding rather than releasing also
        // means a pause between sentences does not undo the gain that the
        // previous sentence established.
        if rms >= NOISE_FLOOR {
            let desired = (TARGET_RMS / rms).clamp(MIN_GAIN, MAX_GAIN);
            let coeff = if desired < self.gain { ATTACK } else { RELEASE };
            self.gain += (desired - self.gain) * coeff;
            self.gain = self.gain.clamp(MIN_GAIN, MAX_GAIN);
        }
        for s in frame.iter_mut() {
            let x = *s as f32 / 32768.0;
            *s = to_i16(soft_limit(x * self.gain));
        }
        20.0 * self.gain.log10()
    }
}

/// Root mean square of a frame, normalised so 1.0 is full scale.
fn rms(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    // f64 for the accumulation: 320 squared samples is small, but this runs
    // fifty times a second for hours and drift here would show up as a slow
    // wander in the meter.
    let sum: f64 = frame.iter().map(|&s| { let x = s as f64 / 32768.0; x * x }).sum();
    (sum / frame.len() as f64).sqrt() as f32
}

/// Compress everything above the knee toward full scale without reaching it.
/// Continuous and smooth at the knee (the derivative there is exactly 1), so
/// it bends rather than kinks, and asymptotic to 1.0, so `to_i16` below can
/// never wrap a loud frame into the opposite sign.
fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= LIMIT_KNEE {
        return x;
    }
    let head = 1.0 - LIMIT_KNEE;
    let over = a - LIMIT_KNEE;
    let limited = LIMIT_KNEE + head * (over / (over + head));
    if x.is_sign_negative() { -limited } else { limited }
}

fn to_i16(x: f32) -> i16 {
    (x * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of a sine at the given amplitude (0..1 of full scale).
    fn tone(amplitude: f32, samples: usize, phase: &mut f32) -> Vec<i16> {
        (0..samples)
            .map(|_| {
                let v = (*phase).sin() * amplitude;
                *phase += std::f32::consts::TAU * 220.0 / 16_000.0;
                to_i16(v)
            })
            .collect()
    }

    fn peak(frame: &[i16]) -> i16 {
        frame.iter().map(|s| s.saturating_abs()).max().unwrap_or(0)
    }

    #[test]
    fn a_frame_is_twenty_milliseconds() {
        assert_eq!(FRAME_SAMPLES, 320);
        assert_eq!(FRAME_BYTES, 640);
        assert_eq!(FRAME_SAMPLES as u32 * 50, crate::record::SAMPLE_RATE);
    }

    /// The failure this guards against is the loudest one possible: a room
    /// left silent for ten minutes, gain wound up to the cap, and then a chair
    /// scrapes.
    #[test]
    fn silence_does_not_ratchet_the_gain_up() {
        let mut agc = Agc::with_enabled(true);
        for _ in 0..500 {
            let mut frame = vec![0i16; FRAME_SAMPLES];
            let db = agc.process(&mut frame);
            assert_eq!(db, 0.0, "silence moved the gain");
            assert!(frame.iter().all(|&s| s == 0));
        }
    }

    /// Dither-level noise is still below the floor: a microphone that is on
    /// but hearing nothing must not be mistaken for a distant voice.
    #[test]
    fn room_tone_below_the_floor_holds_the_gain() {
        let mut agc = Agc::with_enabled(true);
        let mut phase = 0.0;
        // First establish some gain on real speech-level input.
        for _ in 0..200 {
            let mut frame = tone(0.01, FRAME_SAMPLES, &mut phase);
            agc.process(&mut frame);
        }
        let established = agc.gain;
        assert!(established > 2.0, "quiet speech did not raise the gain: {established}");
        for _ in 0..500 {
            let mut frame = tone(0.0005, FRAME_SAMPLES, &mut phase); // -66 dBFS
            agc.process(&mut frame);
        }
        assert_eq!(agc.gain, established, "room tone moved the gain");
    }

    #[test]
    fn quiet_voiced_frames_gain_within_the_cap() {
        let mut agc = Agc::with_enabled(true);
        let mut phase = 0.0;
        let mut last_db = 0.0;
        for _ in 0..1_000 {
            let mut frame = tone(0.002, FRAME_SAMPLES, &mut phase); // -54 dBFS
            last_db = agc.process(&mut frame);
        }
        // Target is +34 dB away, so it should sit at the cap, not past it.
        assert!(last_db > 29.0 && last_db <= 30.01, "gain settled at {last_db} dB");
        assert!(agc.gain <= MAX_GAIN);
    }

    #[test]
    fn a_quiet_source_converges_toward_the_target() {
        let mut agc = Agc::with_enabled(true);
        let mut phase = 0.0;
        let mut out = Vec::new();
        for _ in 0..1_000 {
            let mut frame = tone(0.02, FRAME_SAMPLES, &mut phase); // -34 dBFS
            agc.process(&mut frame);
            out = frame;
        }
        let got = rms(&out);
        assert!(
            (got - TARGET_RMS).abs() < 0.02,
            "settled at {got}, wanted about {TARGET_RMS}"
        );
    }

    #[test]
    fn loud_frames_do_not_clip() {
        let mut agc = Agc::with_enabled(true);
        let mut phase = 0.0;
        for _ in 0..600 {
            let mut frame = tone(0.98, FRAME_SAMPLES, &mut phase);
            agc.process(&mut frame);
            assert!(peak(&frame) < 32_767, "a loud frame reached full scale");
            // Sign flips are what an unlimited i16 cast produces; the soft
            // limiter exists so this can never happen.
            assert!(frame.iter().all(|&s| s != i16::MIN));
        }
    }

    /// A transient arriving while the gain is high is the worst case: the
    /// limiter has to hold the frame down, then the attack has to bring the
    /// gain back to something sane quickly.
    #[test]
    fn a_transient_is_limited_then_the_gain_recovers() {
        let mut agc = Agc::with_enabled(true);
        let mut phase = 0.0;
        for _ in 0..1_000 {
            let mut frame = tone(0.002, FRAME_SAMPLES, &mut phase);
            agc.process(&mut frame);
        }
        let before = agc.gain;
        assert!(before > 20.0);

        // Half a second of someone shouting into the laptop.
        for _ in 0..25 {
            let mut frame = tone(0.9, FRAME_SAMPLES, &mut phase);
            agc.process(&mut frame);
            assert!(peak(&frame) < 32_767);
        }
        assert!(agc.gain < 2.0, "attack did not pull the gain down: {}", agc.gain);

        // Back to the quiet talker: the gain climbs again rather than
        // stranding the rest of the meeting at unity.
        for _ in 0..1_000 {
            let mut frame = tone(0.002, FRAME_SAMPLES, &mut phase);
            agc.process(&mut frame);
        }
        assert!(agc.gain > 20.0, "release did not recover: {}", agc.gain);
    }

    /// The read boundary property, at the level this module can see it:
    /// framing is the caller's job, but processing must depend only on the
    /// frames, not on how many `process` calls preceded them.
    #[test]
    fn the_same_frames_produce_the_same_output() {
        let mut phase = 0.0;
        let frames: Vec<Vec<i16>> =
            (0..300).map(|_| tone(0.01, FRAME_SAMPLES, &mut phase)).collect();

        let run = |input: &[Vec<i16>]| {
            let mut agc = Agc::with_enabled(true);
            let mut out = Vec::new();
            for f in input {
                let mut f = f.clone();
                agc.process(&mut f);
                out.extend_from_slice(&f);
            }
            out
        };
        assert_eq!(run(&frames), run(&frames));
    }

    #[test]
    fn off_passes_samples_through_untouched() {
        let mut agc = Agc::with_enabled(false);
        let mut phase = 0.0;
        let original = tone(0.002, FRAME_SAMPLES, &mut phase);
        let mut frame = original.clone();
        let db = agc.process(&mut frame);
        assert_eq!(frame, original);
        assert_eq!(db, 0.0);
    }

    #[test]
    fn the_soft_limiter_is_continuous_at_the_knee_and_bounded() {
        assert_eq!(soft_limit(LIMIT_KNEE), LIMIT_KNEE);
        assert!((soft_limit(LIMIT_KNEE + 1e-4) - LIMIT_KNEE).abs() < 1e-3);
        for x in [1.0f32, 2.0, 10.0, 1_000.0] {
            assert!(soft_limit(x) < 1.0, "limiter let {x} through");
            assert!(soft_limit(-x) > -1.0);
        }
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.5), -0.5);
    }
}
