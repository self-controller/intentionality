import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import type { Level } from "./types";

/// The capture meter: what the microphone is hearing, right now.
///
/// Meeting 1 recorded 27 seconds of a room and produced one sentence, and
/// there was no way to tell whether the microphone had heard anything. This is
/// that missing indication — and it is worth being strict about what it shows,
/// because a display that reassures you while the room is inaudible is worse
/// than no display at all:
///
/// * every bar's height is the latest measured 100 ms window, pushed by a
///   `meeting:level` event, measured against the loudest recent sound (see
///   `CEILING_FALL_DB`) and scaled by that bar's random weight. The animation loop
///   only eases each bar toward that measurement and stops once it gets there;
///   it never invents movement. A frozen strip means the levels stopped
///   arriving, which is true and worth seeing.
/// * which bar is tall is re-drawn at random on every measured window, each
///   bar independently and pushed away from its neighbour; how tall they get
///   is the measurement, auto-ranged to the loudest recent sound. The backend measures one loudness
///   per window, not frequencies, so the draw is decoration: silence is dots
///   whatever it says, and no event means no new draw, so a stalled stream
///   still freezes.
/// * the bars show the *raw* input, before automatic gain, so amplification
///   can never be mistaken for a healthy source.
/// * the quiet warning waits for several seconds of sustained low raw level,
///   not for the gain being high.
///
/// Levels arrive ten times a second and the easing runs every frame, so both
/// write straight to the DOM through refs. `setState` at that rate would
/// re-render the whole meeting list around it.

const BAR_COUNT = 35;

/// The one fixed shape left: a soft bell, so the ends taper like a voice
/// waveform. Which bars are tall within it is random (see `reshuffle`).
const ENVELOPE = Array.from(
  { length: BAR_COUNT },
  (_, i) => 0.4 + 0.6 * Math.sin((Math.PI * (i + 0.5)) / BAR_COUNT),
);

/// The bars auto-range against the loudest recent sound, not a fixed dBFS
/// axis. The fixed -70..0 axis failed in practice: meetings 8 and 9 averaged
/// -12 and -15 dBFS raw, which parked every bar at ~85% and left speech moving
/// them a few pixels. Instead:
///
/// * `ceiling` is full height. It jumps at once to any louder window and falls
///   by CEILING_FALL_DB per window, so the first sound picked up draws tall and
///   the meter only shrinks when something much louder comes along, then
///   slowly re-expands.
/// * the ceiling never sits below MIN_CEILING_DBFS, and a bar is a dot at
///   SPAN_DB below the ceiling or at GATE_DBFS, whichever is louder, so room
///   hiss stays dots instead of being blown up to full height.
///
/// The quiet warning still tests raw dBFS.
const MIN_CEILING_DBFS = -45;
const GATE_DBFS = -60;
const SPAN_DB = 30;
const CEILING_FALL_DB = 0.15; // 1.5 dB/s at ten windows a second

/// Bar width and strip height, matching `.level-bar` and `.level-bars`. The
/// shortest bar is as tall as it is wide — a round dot, never nothing, because
/// a strip of zero-height bars is indistinguishable from a broken component.
const BAR_MIN_PX = 3;
const BAR_MAX_PX = 32;

/// Meter ballistics: bars rise fast, so a word shows at once, and fall back at
/// clearly different speeds, so neighbours do not move in lockstep.
const ATTACK_MS = Array.from({ length: BAR_COUNT }, (_, i) => 40 + ((i * 23) % 7) * 10);
const RELEASE_MS = Array.from({ length: BAR_COUNT }, (_, i) => 110 + ((i * 37) % 11) * 19);

/// Closer than this (as a fraction of full height) a bar snaps to its goal.
/// Once every bar has, the loop stops until the next level arrives.
const SETTLED = 0.002;

/// Below this raw RMS, for this many consecutive windows, the room is not
/// being picked up. 40 windows is 4 s: long enough not to fire between
/// sentences, short enough to notice before a meeting is wasted.
const QUIET_DBFS = -55;
const QUIET_WINDOWS = 40;

export interface LevelBarsHandle {
  push(level: Level): void;
}

export const LevelBars = forwardRef<LevelBarsHandle, { active: boolean }>(function LevelBars(
  { active },
  ref,
) {
  const bars = useRef<(HTMLSpanElement | null)[]>([]);
  // The latest measured level, 0..1, and each bar's currently drawn height.
  const target = useRef(0);
  const ceiling = useRef(-Infinity);
  const shown = useRef(new Float32Array(BAR_COUNT));
  const weights = useRef(Float32Array.from(ENVELOPE));
  const frame = useRef<number | null>(null);
  const lastTick = useRef<number | null>(null);
  const reduceMotion = useRef(false);
  const quietFor = useRef(0);
  const [quiet, setQuiet] = useState(false);

  const paint = useCallback(() => {
    for (let i = 0; i < BAR_COUNT; i++) {
      const el = bars.current[i];
      if (!el) continue;
      el.style.height = `${BAR_MIN_PX + shown.current[i] * (BAR_MAX_PX - BAR_MIN_PX)}px`;
    }
  }, []);

  const tick = useCallback(
    (now: number) => {
      // Clamped so a frame after a long stall (a hidden window) eases rather
      // than jumping.
      const dt = lastTick.current === null ? 16 : Math.min(100, now - lastTick.current);
      lastTick.current = now;
      let moving = false;
      for (let i = 0; i < BAR_COUNT; i++) {
        const goal = target.current * weights.current[i];
        const gap = goal - shown.current[i];
        if (Math.abs(gap) < SETTLED) {
          shown.current[i] = goal;
          continue;
        }
        const tau = gap > 0 ? ATTACK_MS[i] : RELEASE_MS[i];
        shown.current[i] += gap * (1 - Math.exp(-dt / tau));
        moving = true;
      }
      paint();
      if (moving) {
        frame.current = requestAnimationFrame(tick);
      } else {
        frame.current = null;
        lastTick.current = null;
      }
    },
    [paint],
  );

  // A new recording starts from flat dots rather than the last one's final
  // level, which would otherwise read as live audio until the first event.
  useEffect(() => {
    target.current = 0;
    // So the first sound of this recording sets the range, not the last one's.
    ceiling.current = -Infinity;
    shown.current.fill(0);
    weights.current.set(ENVELOPE);
    quietFor.current = 0;
    setQuiet(false);
    reduceMotion.current =
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    if (active) paint();
    return () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      frame.current = null;
      lastTick.current = null;
    };
  }, [active, paint]);

  useImperativeHandle(ref, () => ({
    push(level: Level) {
      const db = level.input_rms_dbfs;
      ceiling.current = Math.max(db, ceiling.current - CEILING_FALL_DB, MIN_CEILING_DBFS);
      const bottom = Math.max(ceiling.current - SPAN_DB, GATE_DBFS);
      target.current = Math.max(0, (db - bottom) / (ceiling.current - bottom));
      if (reduceMotion.current) {
        // Reduced motion removes the easing and the random draws, not the
        // measurements: the envelope scales with the level and snaps.
        for (let i = 0; i < BAR_COUNT; i++) shown.current[i] = target.current * ENVELOPE[i];
        paint();
      } else {
        reshuffle(weights.current);
        if (frame.current === null) frame.current = requestAnimationFrame(tick);
      }
      quietFor.current = level.input_rms_dbfs < QUIET_DBFS ? quietFor.current + 1 : 0;
      // setState only on the transition, so this stays off the 10 Hz path.
      const nowQuiet = quietFor.current >= QUIET_WINDOWS;
      setQuiet((was) => (was === nowQuiet ? was : nowQuiet));
    },
  }));

  // No recorder, no bars. The record indicator is the source of truth for
  // whether the microphone is open; a meter must never be the thing that makes
  // the UI look live.
  if (!active) return null;

  return (
    <div className="level-meter">
      <div className="level-bars" aria-hidden="true">
        {Array.from({ length: BAR_COUNT }, (_, i) => (
          <span
            key={i}
            className="level-bar"
            ref={(el) => {
              bars.current[i] = el;
            }}
          />
        ))}
      </div>
      {quiet && (
        <span className="text-xs text-warn">very quiet — check the mic or move closer</span>
      )}
    </div>
  );
});

/// Re-draw which bars are tall, once per measured window. Each bar is drawn
/// independently, skewed short so the tall ones stand out, and a draw too
/// close to its left neighbour's is flipped so adjacent bars clearly differ.
/// The 0.12 base keeps any bar from vanishing while the others are up.
function reshuffle(weights: Float32Array) {
  let prev = -1;
  for (let i = 0; i < BAR_COUNT; i++) {
    let r = Math.random() ** 1.5;
    if (Math.abs(r - prev) < 0.25) r = 1 - r;
    prev = r;
    weights[i] = ENVELOPE[i] * (0.12 + 0.88 * r);
  }
}
