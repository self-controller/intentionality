import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import { dayKey, dayLabel, fmt, hhmm } from "./format";
import type { AudioSource, Level, Meeting, MeetingDetail } from "./types";

/// The chosen microphone, remembered across restarts. The node *name* is what
/// is stored — PipeWire serials are reassigned when a device reconnects, so a
/// remembered serial would eventually point at something else. The label rides
/// along only so a device that has since vanished can be named in the notice
/// below; it is never what gets recorded from.
const SOURCE_KEY = "intentionality.meeting.source";

function rememberedSource(): AudioSource | null {
  try {
    const raw = localStorage.getItem(SOURCE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<AudioSource>;
    return parsed?.target ? { target: parsed.target, label: parsed.label || parsed.target } : null;
  } catch {
    return null; // a corrupt entry is the system default, not a broken screen
  }
}

/// Recording lives in Rust, not in this component. Switching to the Board and
/// back unmounts everything here and the microphone keeps running — which is
/// why mount asks the backend what is recording rather than trusting state.
export default function Meetings() {
  const [items, setItems] = useState<Meeting[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [recording, setRecording] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  // Set the instant Stop is clicked, cleared once the reloaded list has taken
  // over. Stop closes the microphone before it returns, so this is what lets
  // the indicator go down on the click rather than on the round trip.
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ticked, setTicked] = useState<Set<number>>(new Set());
  const [elapsed, setElapsed] = useState(0);
  const [sources, setSources] = useState<AudioSource[]>([]);
  const [source, setSource] = useState<AudioSource | null>(rememberedSource);
  const [sourceNote, setSourceNote] = useState<string | null>(null);

  const selectedRef = useRef<number | null>(null);
  // Levels arrive ten times a second; they go straight into the meter's DOM
  // rather than through state, which would re-render this whole list.
  const meter = useRef<LevelBarsHandle | null>(null);
  // The listener below is registered once and must see the current recording
  // id without being torn down and rebuilt every time it changes.
  const recordingRef = useRef<number | null>(null);
  useEffect(() => {
    recordingRef.current = recording;
  }, [recording]);
  useEffect(() => {
    selectedRef.current = selected;
  }, [selected]);

  // The microphone is open. Not "a meeting is selected and unfinished": once
  // Stop has been clicked the microphone is shut, and the red dot, the clock
  // and the meter must all stop with it even though the notes are still being
  // written.
  const live = recording != null && !stopping;

  // The notes for the meeting just ended are still being written. Read off the
  // list rather than kept in a state of its own, so it survives this tab being
  // unmounted and remounted — `items` is refreshed by both `meeting:state` and
  // `meeting:done`. `stopping` covers the gap before the first of those lands.
  const notesPending = stopping || items.some((m) => m.state === "summarizing");

  const reload = useCallback(() => api.listMeetings(api.RECENT_MEETINGS), []);

  const load = useCallback((id: number) => {
    setSelected(id);
    setTicked(new Set());
    api
      .getMeeting(id)
      .then(setDetail)
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    reload()
      .then((list) => {
        setItems(list);
        if (list.length > 0 && selectedRef.current == null) load(list[0].id);
      })
      .catch((e) => setError(String(e)));
    api.recordingMeeting().then(setRecording).catch(() => {});

    const refresh = () => {
      reload().then(setItems).catch(() => {});
      if (selectedRef.current != null) {
        api.getMeeting(selectedRef.current).then(setDetail).catch(() => {});
      }
    };
    const unlistenSegment = listen("meeting:segment", refresh);
    const unlistenState = listen("meeting:state", refresh);
    const unlistenDone = listen("meeting:done", () => {
      setRecording(null);
      refresh();
    });
    const unlistenLevel = listen<{ meeting_id: number; level: Level }>(
      "meeting:level",
      (event) => {
        // Ignore a stale meeting's levels: a Stop racing the last window in
        // flight must not animate the strip for a recording that has ended.
        if (event.payload.meeting_id !== recordingRef.current) return;
        meter.current?.push(event.payload.level);
      },
    );
    return () => {
      unlistenSegment.then((f) => f());
      unlistenState.then((f) => f());
      unlistenDone.then((f) => f());
      unlistenLevel.then((f) => f());
    };
  }, [reload, load]);

  // Fall back to the system default for a source that is not on offer, and say
  // so. The stored choice is deliberately *not* cleared: the backend reports
  // every pw-dump failure as an empty list, so this path cannot tell "your USB
  // microphone is unplugged" from "PipeWire did not answer just now", and a
  // momentary hiccup should not throw away a deliberate choice. Only an
  // explicit pick writes storage. The consequence is that `source` is always
  // either null or one of `sources`, so the dropdown can always show it.
  const fallBackToDefault = useCallback((want: AudioSource) => {
    setSource(null);
    setSourceNote(`${want.label} isn't available — using the system default.`);
  }, []);

  // The microphones on offer, fetched once per mount. An empty list is not an
  // error — it means PipeWire had nothing to say and only the system default
  // is available.
  useEffect(() => {
    let live = true;
    api
      .listAudioSources()
      .catch(() => [] as AudioSource[])
      .then((list) => {
        if (!live) return;
        setSources(list);
        const want = rememberedSource();
        if (want && !list.some((s) => s.target === want.target)) fallBackToDefault(want);
      });
    return () => {
      live = false;
    };
  }, [fallBackToDefault]);

  const chooseSource = (target: string) => {
    const picked = sources.find((s) => s.target === target) ?? null;
    setSource(picked);
    setSourceNote(null);
    if (picked) localStorage.setItem(SOURCE_KEY, JSON.stringify(picked));
    else localStorage.removeItem(SOURCE_KEY);
  };

  // The elapsed clock is cosmetic and derived from the meeting's own
  // started_at, so it stays right across a tab switch instead of restarting.
  useEffect(() => {
    if (!live || recording == null) {
      setElapsed(0);
      return;
    }
    const row = items.find((m) => m.id === recording);
    if (!row) return;
    const began = new Date(row.started_at).getTime();
    const tick = () => setElapsed(Math.max(0, Math.round((Date.now() - began) / 1000)));
    tick();
    const timer = setInterval(tick, 1000);
    return () => clearInterval(timer);
  }, [live, recording, items]);

  const start = () => {
    setError(null);
    setBusy(true);
    // Re-check the chosen microphone against a fresh list first. `pw-record`
    // does not refuse an unknown --target: measured on 2026-09-10, it falls
    // back to the default source, streams happily and says nothing. So a
    // device unplugged since this tab mounted would otherwise record the
    // built-in microphone while the selector still claimed the USB one — a
    // wrong recording rather than a failed one, which is worse.
    api
      .listAudioSources()
      .catch(() => [] as AudioSource[])
      .then((list) => {
        setSources(list);
        const want = source;
        if (want && !list.some((s) => s.target === want.target)) {
          fallBackToDefault(want);
          return undefined;
        }
        return want?.target;
      })
      .then((target) => api.startMeeting(target))
      .then((id) => {
        setRecording(id);
        return reload().then((list) => {
          setItems(list);
          load(id);
        });
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const stop = () => {
    setError(null);
    setBusy(true);
    // Before the round trip, not after it. The backend shuts the microphone and
    // returns; this is the click's own acknowledgement, and without it the
    // indicator keeps running through a Stop that has already happened.
    setStopping(true);
    api
      .stopMeeting()
      .then((id) => {
        setRecording(null);
        return reload().then((list) => {
          setItems(list);
          load(id);
        });
      })
      // A failed *stop*, which is all this call does now. A failed summary is
      // not a failed meeting and never reaches here: the transcript is saved,
      // and the detail pane below says what went wrong and offers a retry.
      .catch((e) => {
        setError(String(e));
        setRecording(null);
        return reload()
          .then(setItems)
          .catch(() => {});
      })
      // Cleared only once that reload has landed, so `notesPending` has already
      // read 'summarizing' off the fresh list. Clearing earlier would flash an
      // enabled Start between the two.
      .finally(() => {
        setStopping(false);
        setBusy(false);
      });
  };

  const approve = () => {
    if (!detail || ticked.size === 0) return;
    setBusy(true);
    api
      .approveActions(detail.meeting.id, [...ticked])
      .then(() => {
        setTicked(new Set());
        load(detail.meeting.id);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const retry = () => {
    if (!detail) return;
    setBusy(true);
    setError(null);
    api
      .resummarizeMeeting(detail.meeting.id)
      .then(() => load(detail.meeting.id))
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const remove = (id: number) => {
    api
      .deleteMeeting(id)
      .then(() =>
        reload().then((list) => {
          setItems(list);
          if (selectedRef.current === id) {
            setDetail(null);
            setSelected(null);
            if (list.length > 0) load(list[0].id);
          }
        }),
      )
      .catch((e) => setError(String(e)));
  };

  let lastDay = "";

  return (
    <div className="meetings">
      <div className="meeting-list">
        <div className="record-bar">
          {/* Disabled while recording: pw-record is given its source when it
              is spawned, so changing this mid-meeting could only mislead. */}
          <select
            className="source-select"
            value={source?.target ?? ""}
            onChange={(e) => chooseSource(e.target.value)}
            disabled={live || busy}
            aria-label="Microphone"
          >
            <option value="">System default microphone</option>
            {sources.map((s) => (
              <option key={s.target} value={s.target}>
                {s.label}
              </option>
            ))}
          </select>
          {/* Three states, not two. The wrap-up needs one of its own: the
              microphone is already shut but the tail chunk and the notes are
              still in flight, and a Start button offered there would take a
              double-click meant for Stop and open a second recording. */}
          {live ? (
            <button className="record-stop" onClick={stop} disabled={busy}>
              ■ Stop transcribing
            </button>
          ) : notesPending ? (
            <button className="record-wrap" disabled>
              Writing the notes…
            </button>
          ) : (
            <button className="record-start" onClick={start} disabled={busy}>
              ● Start transcribing
            </button>
          )}
          {live && (
            <div className="recording-now">
              <span className="rec-dot" /> recording {fmt(elapsed)}
            </div>
          )}
          {/* The record indicator above stays the source of truth for whether
              the microphone is open. This only ever says what it is hearing. */}
          <LevelBars ref={meter} active={live} />
        </div>
        {sourceNote && <p className="muted source-note">{sourceNote}</p>}
        {error && <p className="record-error">{error}</p>}
        {items.length === 0 && <p className="muted">No meetings yet.</p>}
        {items.map((m) => {
          const day = dayKey(m.started_at);
          const header = day !== lastDay ? ((lastDay = day), dayLabel(m.started_at)) : null;
          return (
            <div key={m.id}>
              {header && <div className="day-head">{header}</div>}
              <button
                className={`meeting-row ${selected === m.id ? "active" : ""}`}
                onClick={() => load(m.id)}
              >
                <span className="meeting-title">
                  {m.title || (m.state === "recording" ? "Recording…" : "Untitled meeting")}
                </span>
                <span className="meeting-meta">
                  {hhmm(m.started_at)}
                  {m.state === "failed" && <span className="meeting-bad"> · notes failed</span>}
                  {m.state === "summarizing" && <span className="muted"> · summarizing…</span>}
                  {live && m.id === recording && <span className="meeting-live"> · live</span>}
                </span>
              </button>
            </div>
          );
        })}
      </div>

      <div className="meeting-detail">
        {!detail && <p className="muted">Select a meeting.</p>}
        {detail && <Detail
          detail={detail}
          isRecording={live && detail.meeting.id === recording}
          ticked={ticked}
          setTicked={setTicked}
          onApprove={approve}
          onRetry={retry}
          onDelete={() => remove(detail.meeting.id)}
          busy={busy}
        />}
      </div>
    </div>
  );
}

function Detail({
  detail,
  isRecording,
  ticked,
  setTicked,
  onApprove,
  onRetry,
  onDelete,
  busy,
}: {
  detail: MeetingDetail;
  isRecording: boolean;
  ticked: Set<number>;
  setTicked: (s: Set<number>) => void;
  onApprove: () => void;
  onRetry: () => void;
  onDelete: () => void;
  busy: boolean;
}) {
  const { meeting, segments, actions } = detail;
  const pending = actions.filter((a) => a.task_id == null);
  const approved = actions.filter((a) => a.task_id != null);
  const toggle = (id: number) => {
    const next = new Set(ticked);
    next.has(id) ? next.delete(id) : next.add(id);
    setTicked(next);
  };

  return (
    <>
      <h2>{meeting.title || (isRecording ? "Recording…" : "Untitled meeting")}</h2>
      <p className="muted">
        {hhmm(meeting.started_at)}
        {meeting.ended_at ? `–${hhmm(meeting.ended_at)}` : ""} · {meeting.segment_count} segment
        {meeting.segment_count === 1 ? "" : "s"}
        {meeting.session_id == null && " · outside a session"}
      </p>

      {meeting.state === "failed" && (
        <div className="meeting-failed">
          <strong>The notes could not be written.</strong>
          <p className="muted">{meeting.error}</p>
          {/* The transcript is already stored, so a retry costs one model
              call and no audio — worth offering rather than losing the notes. */}
          <button onClick={onRetry} disabled={busy}>
            Try the summary again
          </button>
        </div>
      )}
      {meeting.state === "summarizing" && <p className="muted">Writing the notes…</p>}

      {meeting.summary && (
        <section>
          <h3>Summary</h3>
          <p>{meeting.summary}</p>
        </section>
      )}

      {meeting.key_points.length > 0 && (
        <section>
          <h3>Key points</h3>
          <ul className="key-points">
            {meeting.key_points.map((k, i) => (
              <li key={i}>{k}</li>
            ))}
          </ul>
        </section>
      )}

      {actions.length > 0 && (
        <section>
          <h3>Action items</h3>
          {pending.map((a) => (
            <label key={a.id} className="action-row">
              <input
                type="checkbox"
                checked={ticked.has(a.id)}
                onChange={() => toggle(a.id)}
              />
              <span>{a.text}</span>
            </label>
          ))}
          {pending.length > 0 && (
            <button className="approve" onClick={onApprove} disabled={busy || ticked.size === 0}>
              Add {ticked.size} to backlog
            </button>
          )}
          {approved.map((a) => (
            <div key={a.id} className="action-row done">
              <span className="tick">✓</span>
              <span>{a.text}</span>
              <span className="muted"> · in the backlog</span>
            </div>
          ))}
        </section>
      )}

      <section>
        <h3>Transcript</h3>
        {segments.length === 0 && (
          <p className="muted">
            {isRecording ? "Listening — the first block lands after two minutes." : "Nothing was transcribed."}
          </p>
        )}
        <div className="transcript">
          {segments.map((s) =>
            s.text ? (
              <p key={s.id}>
                <span className="seg-time">{hhmm(s.started_at)}</span> {s.text}
              </p>
            ) : (
              // A visible gap, not a silent one: a chunk that failed to
              // transcribe is two minutes the notes below never saw.
              <p key={s.id} className="seg-gap">
                <span className="seg-time">{hhmm(s.started_at)}</span> (this stretch could not be
                transcribed)
              </p>
            ),
          )}
        </div>
      </section>

      {!isRecording && (
        <button className="danger" onClick={onDelete} disabled={busy}>
          Delete meeting
        </button>
      )}
    </>
  );
}
/// The capture meter: what the microphone is hearing, right now.
///
/// Meeting 1 recorded 27 seconds of a room and produced one sentence, and
/// there was no way to tell whether the microphone had heard anything. This is
/// that missing indication — and it is worth being strict about what it shows,
/// because a display that reassures you while the room is inaudible is worse
/// than no display at all:
///
/// * every bar is a measured 100 ms window, pushed by a `meeting:level` event.
///   Nothing here runs on a timer and nothing interpolates. A frozen strip
///   means the levels stopped arriving, which is true and worth seeing.
/// * the bars show the *raw* input, before automatic gain. Gain and output sit
///   in a small secondary line, so amplification can never be mistaken for a
///   healthy source.
/// * the quiet warning waits for several seconds of sustained low raw level,
///   not for the gain being high.
///
/// Updates arrive ten times a second, so they are written straight to the DOM
/// through refs. Ten `setState` calls a second would re-render the whole
/// meeting list around it.
const BAR_COUNT = 24; // 2.4 s of history at 10 Hz

/// The bottom of the drawn scale. Speech across a room lands around -50 dBFS,
/// so a linear amplitude scale would leave it invisible; dBFS is the axis that
/// makes the difference between "quiet" and "nothing" legible.
const FLOOR_DBFS = -70;

/// Bars never fully disappear while recording — a strip of zero-height bars is
/// indistinguishable from a broken component.
const MIN_SCALE = 0.04;

/// Below this raw RMS, for this many consecutive windows, the room is not
/// being picked up. 40 windows is 4 s: long enough not to fire between
/// sentences, short enough to notice before a meeting is wasted.
const QUIET_DBFS = -55;
const QUIET_WINDOWS = 40;

export interface LevelBarsHandle {
  push(level: Level): void;
}

const LevelBars = forwardRef<LevelBarsHandle, { active: boolean }>(function LevelBars(
  { active },
  ref,
) {
  const bars = useRef<(HTMLSpanElement | null)[]>([]);
  const readout = useRef<HTMLSpanElement | null>(null);
  const values = useRef<number[]>(new Array(BAR_COUNT).fill(FLOOR_DBFS));
  const quietFor = useRef(0);
  const [quiet, setQuiet] = useState(false);

  const paint = useCallback(() => {
    for (let i = 0; i < BAR_COUNT; i++) {
      const el = bars.current[i];
      if (!el) continue;
      const db = values.current[i];
      const norm = Math.min(1, Math.max(0, (db - FLOOR_DBFS) / -FLOOR_DBFS));
      el.style.transform = `scaleY(${MIN_SCALE + norm * (1 - MIN_SCALE)})`;
    }
  }, []);

  // A new recording starts from an empty strip rather than the last one's
  // history, which would otherwise read as live audio for the first 2.4 s.
  useEffect(() => {
    values.current = new Array(BAR_COUNT).fill(FLOOR_DBFS);
    quietFor.current = 0;
    setQuiet(false);
    if (active) paint();
  }, [active, paint]);

  useImperativeHandle(ref, () => ({
    push(level: Level) {
      values.current.shift();
      values.current.push(level.input_rms_dbfs);
      paint();
      if (readout.current) {
        // textContent, not state: this line changes ten times a second.
        readout.current.textContent =
          `in ${db(level.input_rms_dbfs)} · out ${db(level.output_rms_dbfs)}` +
          ` · peak ${db(level.output_peak_dbfs)}` +
          (level.gain_db >= 0.5 ? ` · gain +${level.gain_db.toFixed(0)} dB` : "");
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
      <span className="level-readout" ref={readout} />
      {quiet && (
        <span className="level-quiet">very quiet — check the mic or move closer</span>
      )}
    </div>
  );
});

/// dBFS for the readout. The meter floors at -100, which reads better as a
/// symbol than as a number pretending to be a measurement.
function db(value: number): string {
  return value <= -99 ? "—" : `${value.toFixed(0)} dB`;
}
