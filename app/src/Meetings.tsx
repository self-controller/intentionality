import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { LevelBars, type LevelBarsHandle } from "./LevelBars";
import Checkbox from "./ui/Checkbox";
import { Button, SectionHeading, Tab as TabButton } from "./ui/primitives";
import * as api from "./api";
import { dayKey, dayLabel, fmt, hhmm, size } from "./format";
import type {
  AudioSource,
  Level,
  Meeting,
  MeetingDetail,
  RecordingNow,
} from "./types";

// The markdown stack (parser, KaTeX, highlight.js) is most of the app's
// JavaScript and only the write-up needs it, so it loads the first time a
// write-up is shown instead of before the board can appear.
const LazyMarkdown = lazy(() => import("./Markdown"));

function Markdown({ source }: { source: string }) {
  return (
    <Suspense fallback={<p className="text-muted">Loading…</p>}>
      <LazyMarkdown source={source} />
    </Suspense>
  );
}

/// The record bar's clock. Its own component so the once-a-second tick
/// re-renders this text and not the whole tab.
///
/// Derived from when the backend says the microphone opened, so it stays
/// right across a tab switch instead of restarting -- and a resumed meeting
/// counts from the resume, not from the day it was first recorded.
function Elapsed({ since }: { since: string }) {
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const began = new Date(since).getTime();
    const tick = () => setElapsed(Math.max(0, Math.round((Date.now() - began) / 1000)));
    tick();
    const timer = setInterval(tick, 1000);
    return () => clearInterval(timer);
  }, [since]);
  return <>{fmt(elapsed)}</>;
}

const KIND_DOT: Record<string, string> = {
  pdf: "bg-bad",
  image: "bg-good",
  office: "bg-warn",
  text: "bg-accent",
};

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

/// Debounced, chained autosave for one text field of one meeting.
///
/// Used three times — the scratchpad, the transcript and the write-up — and
/// the rules are the same for all of them. The draft, not the stored value, is what the textarea shows:
/// `refresh` below replaces the detail wholesale every time a segment lands
/// (roughly every two minutes while recording), and a textarea reading the
/// stored value would have whatever was being typed wiped out mid-sentence.
///
/// Saves are chained rather than fired straight off: two writes in flight at
/// once can land in either order, and the loser is whichever the backend
/// happens to commit second — which would silently restore text the user had
/// deleted. The pending edit is kept as a (meeting, text) pair rather than
/// read back off the draft at save time, so switching meetings with an edit
/// still pending writes it to the meeting it was typed into, not the newly
/// selected one.
function useAutosave(
  save: (id: number, text: string) => Promise<void>,
  onError: (e: unknown) => void,
) {
  const [draft, setDraft] = useState("");
  // Which meeting the draft belongs to.
  const owner = useRef<number | null>(null);
  const timer = useRef<number | null>(null);
  const pending = useRef<{ id: number; text: string } | null>(null);
  const inFlight = useRef<Promise<void> | null>(null);
  // Something has been typed since the draft was last seeded.
  const touched = useRef(false);

  /// Send one save, after any save already in flight.
  const send = useCallback(
    (id: number, text: string) => {
      const run = () => save(id, text).catch(onError);
      const chained = (inFlight.current ?? Promise.resolve()).then(run, run);
      inFlight.current = chained;
      return chained;
    },
    [save, onError],
  );

  /// Write whatever the debounce is holding, and resolve once it has landed.
  ///
  /// Stop and Re-run await this. Without it, typing a name and immediately
  /// clicking Stop races the 600 ms debounce and the run reads the *previous*
  /// text — which is exactly the moment the scratchpad matters most, because
  /// correcting a name you just heard mangled is what it is for.
  const flush = useCallback(async () => {
    if (timer.current != null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
    const edit = pending.current;
    pending.current = null;
    if (edit) send(edit.id, edit.text);
    await inFlight.current;
  }, [send]);

  /// Seed the draft for a meeting. Without `force`, a reload of the same
  /// meeting never re-seeds and a switch to a different one always does.
  const seed = useCallback((id: number, text: string, force = false) => {
    if (force || owner.current !== id) {
      owner.current = id;
      touched.current = false;
      setDraft(text);
    }
  }, []);

  /// Seed, and keep following the stored text until something is typed.
  ///
  /// For the transcript, whose stored text moves under an untouched draft:
  /// the tail chunk lands a moment *after* Stop returns, and a box seeded once
  /// would be missing the last thing said. Once the user has typed, the draft
  /// is the truth and only their own saves change what is stored.
  const follow = useCallback((id: number, text: string) => {
    if (owner.current !== id || !touched.current) {
      owner.current = id;
      touched.current = false;
      setDraft(text);
    }
  }, []);

  /// Go back to following. A resume grows the transcript past the draft, and
  /// the next Stop's text has to reach the box.
  const release = useCallback(() => {
    touched.current = false;
  }, []);

  const edit = (text: string) => {
    setDraft(text);
    touched.current = true;
    const id = owner.current;
    if (id == null) return;
    pending.current = { id, text };
    if (timer.current != null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      timer.current = null;
      const edit = pending.current;
      pending.current = null;
      if (edit) send(edit.id, edit.text);
    }, 600);
  };

  // Leaving the tab is not a reason to lose what was typed. The ref dance is
  // so this runs on unmount only, rather than on every keystroke.
  const flushRef = useRef(flush);
  flushRef.current = flush;
  useEffect(() => () => void flushRef.current(), []);

  return { draft, seed, follow, release, edit, flush };
}

/// Recording lives in Rust, not in this component. Switching to the Board and
/// back unmounts everything here and the microphone keeps running — which is
/// why mount asks the backend what is recording rather than trusting state.
export default function Meetings() {
  const [items, setItems] = useState<Meeting[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [recording, setRecording] = useState<number | null>(null);
  // When the microphone opened, which for a resumed meeting is not its
  // started_at. Always set together with `recording`, through `setNow`.
  const [since, setSince] = useState<string | null>(null);
  const setNow = useCallback((now: RecordingNow | null) => {
    setRecording(now?.meeting_id ?? null);
    setSince(now?.since ?? null);
  }, []);
  const [busy, setBusy] = useState(false);
  // Set the instant Stop is clicked, cleared once the reloaded list has taken
  // over. Stop closes the microphone before it returns, so this is what lets
  // the indicator go down on the click rather than on the round trip.
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ticked, setTicked] = useState<Set<number>>(new Set());
  const [sources, setSources] = useState<AudioSource[]>([]);
  const [source, setSource] = useState<AudioSource | null>(rememberedSource);
  const [sourceNote, setSourceNote] = useState<string | null>(null);

  const fail = useCallback((e: unknown) => setError(String(e)), []);
  // The scratchpad and the write-up, each with a draft of its own. See
  // useAutosave for why the drafts, not `detail`, are what the textareas show.
  const notes = useAutosave(api.setMeetingNotes, fail);
  const transcript = useAutosave(api.setMeetingTranscript, fail);
  const summary = useAutosave(api.setMeetingSummary, fail);

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
  // and the meter must all stop with it even though the last block is still
  // on its way.
  const live = recording != null && !stopping;

  // The notes for some meeting are still being written. Read off the list
  // rather than kept in a state of its own, so it survives this tab being
  // unmounted and remounted — `items` is refreshed by both `meeting:state`
  // and `meeting:done`.
  //
  // This reports; it no longer forbids. Only Write notes gets here now: Stop
  // leaves the transcript to be reviewed first.
  const wrapUpCount = items.filter((m) => m.state === "summarizing").length;
  const notesPending = wrapUpCount > 0;

  // The microphone is free. There is exactly one of it, so a live recording
  // rules out another — but a meeting whose notes are being written does not:
  // Stop shuts the microphone before it returns and hands the rest to a
  // background task. `stopping` is the in-between, covering the Stop round
  // trip so a double-click cannot land on the Start that replaces it.
  const canRecord = recording == null && !stopping;

  const reload = useCallback(() => api.listMeetings(api.RECENT_MEETINGS), []);

  const seedNotes = notes.seed;
  const followTranscript = transcript.follow;
  const load = useCallback(
    (id: number) => {
      setSelected(id);
      setTicked(new Set());
      return api
        .getMeeting(id)
        .then((d) => {
          setDetail(d);
          // The scratchpad's draft is seeded here and nowhere else, and only
          // when the meeting it belongs to changes: while a meeting is
          // selected the draft is the truth and `d.notes` only ever seeds it.
          //
          // The write-up is different. Its draft is seeded when Edit is
          // pressed, not here: unlike the scratchpad the stored document
          // changes under us (a re-run replaces it), and a draft seeded once
          // per meeting would shadow that.
          seedNotes(d.meeting.id, d.notes);
          followTranscript(d.meeting.id, d.transcript);
        })
        .catch(fail);
    },
    [seedNotes, followTranscript, fail],
  );

  useEffect(() => {
    reload()
      .then((list) => {
        setItems(list);
        if (list.length > 0 && selectedRef.current == null) load(list[0].id);
      })
      .catch(fail);
    api.recordingMeeting().then(setNow).catch(() => {});

    const refresh = () => {
      reload().then(setItems).catch(() => {});
      if (selectedRef.current != null) {
        api
          .getMeeting(selectedRef.current)
          .then((d) => {
            // A switch landed while this was in flight: the pane and the
            // transcript draft belong to the newly selected meeting now.
            if (d.meeting.id !== selectedRef.current) return;
            setDetail(d);
            followTranscript(d.meeting.id, d.transcript);
          })
          .catch(() => {});
      }
    };
    const unlistenSegment = listen("meeting:segment", refresh);
    const unlistenState = listen("meeting:state", refresh);
    const unlistenDone = listen<number>("meeting:done", (event) => {
      // Only for the meeting on the microphone. A re-run of some other
      // meeting finishing mid-recording must not take the Stop button away
      // from one that is still live.
      if (event.payload === recordingRef.current) setNow(null);
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
  }, [reload, load, fail, setNow, followTranscript]);

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

  /// The microphone to record from, re-checked against a fresh list first.
  /// `pw-record` does not refuse an unknown --target: measured on 2026-09-10,
  /// it falls back to the default source, streams happily and says nothing. So
  /// a device unplugged since this tab mounted would otherwise record the
  /// built-in microphone while the selector still claimed the USB one — a
  /// wrong recording rather than a failed one, which is worse.
  const pickTarget = () =>
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
      });

  /// The microphone is open: show it, and select the meeting it is feeding.
  const opened = (now: RecordingNow) => {
    setNow(now);
    return reload().then((list) => {
      setItems(list);
      load(now.meeting_id);
    });
  };

  const start = () => {
    setError(null);
    setBusy(true);
    pickTarget()
      .then((target) => api.startMeeting(target))
      .then(opened)
      .catch(fail)
      .finally(() => setBusy(false));
  };

  /// Pick a finished meeting back up. Every draft is flushed first — once the
  /// microphone is open the transcript refuses saves — and the transcript
  /// goes back to following the store, so the next Stop's text reaches it.
  const resume = async (id: number) => {
    setError(null);
    setBusy(true);
    await notes.flush().catch(() => {});
    await transcript.flush().catch(() => {});
    await summary.flush().catch(() => {});
    transcript.release();
    return pickTarget()
      .then((target) => api.resumeMeeting(id, target))
      .then(opened)
      .catch(fail)
      .finally(() => setBusy(false));
  };

  const stop = async () => {
    setError(null);
    setBusy(true);
    // Before the round trip, not after it. The backend shuts the microphone and
    // returns; this is the click's own acknowledgement, and without it the
    // indicator keeps running through a Stop that has already happened.
    setStopping(true);
    // The barrier: whatever is in the textarea is saved before the run reads
    // it. The debounce is 600 ms and a Stop can easily land inside that.
    await notes.flush().catch(() => {});
    return api
      .stopMeeting()
      .then((id) => {
        setNow(null);
        return reload().then((list) => {
          setItems(list);
          load(id);
        });
      })
      // A failed *stop*, which is all this call does: no notes are written
      // until Write notes is pressed.
      .catch((e) => {
        setError(String(e));
        setNow(null);
        return reload()
          .then(setItems)
          .catch(() => {});
      })
      // Cleared only once that reload has landed. Holding Start down until
      // then is what keeps the second click of a double-click meant for Stop
      // off the Start that replaces it.
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
      .catch(fail)
      .finally(() => setBusy(false));
  };

  /// Write notes, or re-write them. The run reads the transcript and the
  /// scratchpad as stored, so both drafts are flushed first — the usual thing
  /// to have done just before pressing this is correcting a name in the
  /// transcript. (The write-up's draft is flushed only to be replaced moments
  /// later; the pane asked before letting an edited document get here.)
  const rerun = async () => {
    if (!detail) return;
    setBusy(true);
    setError(null);
    const id = detail.meeting.id;
    await notes.flush().catch(() => {});
    await transcript.flush().catch(() => {});
    await summary.flush().catch(() => {});
    return api
      .rerunMeetingNotes(id)
      .then(() =>
        reload().then((list) => {
          setItems(list);
          load(id);
        }),
      )
      .catch(fail)
      .finally(() => setBusy(false));
  };

  /// Edit pressed: the write-up's draft starts from whatever is stored now.
  /// Forced, because the same meeting may have been re-run since last time.
  const beginEditSummary = () => {
    if (!detail) return;
    summary.seed(detail.meeting.id, detail.summary ?? "", true);
  };

  /// Done pressed: land the draft, then reload so the reading view shows the
  /// saved document and its "edited" mark rather than the stale one.
  const endEditSummary = async () => {
    if (!detail) return;
    const id = detail.meeting.id;
    await summary.flush().catch(() => {});
    await load(id);
  };

  // Attaching opens a native picker on the Rust side, so this awaits a dialog
  // the user is looking at. `attaching` disables Stop for the same reason
  // `flush` is awaited there: a file being copied when the run starts would
  // otherwise be missed by it.
  const [attaching, setAttaching] = useState(false);

  const attach = async () => {
    if (!detail) return;
    setError(null);
    setAttaching(true);
    const id = detail.meeting.id;
    try {
      const outcome = await api.attachMeetingFiles(id);
      // Partial success is normal: three of four files attaching is a better
      // outcome than all four failing because one was a 30-page PDF.
      if (outcome.rejected.length > 0) setError(outcome.rejected.join("\n"));
      load(id);
      reload().then(setItems).catch(() => {});
    } catch (e) {
      setError(String(e));
    } finally {
      setAttaching(false);
    }
  };

  const detachFile = async (fileId: number) => {
    if (!detail) return;
    setError(null);
    setAttaching(true);
    const id = detail.meeting.id;
    try {
      await api.removeMeetingFile(id, fileId);
      load(id);
      reload().then(setItems).catch(() => {});
    } catch (e) {
      setError(String(e));
    } finally {
      setAttaching(false);
    }
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
      .catch(fail);
  };

  let lastDay = "";

  return (
    <div className="flex items-start gap-4 p-4">
      <aside className="flex max-h-[calc(100vh-90px)] w-[280px] flex-none flex-col gap-1.5 overflow-y-auto">
        <div className="mb-2.5 flex flex-col gap-2">
          {/* Disabled while recording: pw-record is given its source when it
              is spawned, so changing this mid-meeting could only mislead. */}
          <select
            className="w-full rounded-md border border-line bg-surface px-2 py-1.5 text-xs text-text"
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
          {/* Two states, not three. A meeting whose notes are still being
              written no longer occupies this button: the microphone was shut
              the moment Stop returned, the wrap-up is a background task, and
              nothing in Rust ever refused the next recording — only this
              button did. The wrap-up says so on its own line below instead.

              The double-click that the old three-state shape guarded against
              (a second click meant for Stop landing on a Start that had taken
              its place) is covered by `stopping`, which is set before the
              round trip and cleared only once the reloaded list has landed. */}
          {live ? (
            <button className="w-full rounded-md border border-bad p-2.5 text-sm text-bad transition-colors hover:brightness-110 disabled:opacity-40" onClick={stop} disabled={busy || attaching}>
              ■ Stop transcribing
            </button>
          ) : (
            <button className="w-full rounded-md border border-line p-2.5 text-sm transition-colors hover:border-muted disabled:opacity-40 disabled:hover:border-line" onClick={start} disabled={busy || !canRecord}>
              ● Start transcribing
            </button>
          )}
          {notesPending && (
            // Which meeting, when it is not the one on screen, and how many
            // when several are queued — the model calls are serialized, so a
            // second wrap-up genuinely waits for the first.
            <div className="text-xs text-muted">
              Writing the notes…
              {wrapUpCount > 1 && ` (${wrapUpCount} meetings)`}
            </div>
          )}
          {live && (
            <div className="flex items-center gap-2 text-[13px] text-bad">
              <span className="rec-dot" /> recording {since ? <Elapsed since={since} /> : fmt(0)}
            </div>
          )}
          {/* The record indicator above stays the source of truth for whether
              the microphone is open. This only ever says what it is hearing. */}
          <LevelBars ref={meter} active={live} />
        </div>
        {sourceNote && <p className="m-0 text-xs text-muted">{sourceNote}</p>}
        {error && <p className="m-0 whitespace-pre-line text-xs text-bad">{error}</p>}
        {items.length === 0 && <p className="text-muted">No meetings yet.</p>}
        {items.map((m) => {
          const day = dayKey(m.started_at);
          const header = day !== lastDay ? ((lastDay = day), dayLabel(m.started_at)) : null;
          return (
            <div key={m.id}>
              {header && <div className="mb-1 mt-3.5 text-[13px] uppercase tracking-[0.06em] text-muted">{header}</div>}
              <button
                className={
                  "flex w-full flex-col gap-0.5 rounded-md border px-2.5 py-1.5 " +
                  "text-left transition-colors duration-150 " +
                  // On the accent fill every child inherits the dark text: a
                  // muted grey or a state colour on cyan is unreadable.
                  (selected === m.id
                    ? "border-accent bg-accent text-bg"
                    : "border-line text-text hover:border-muted")
                }
                onClick={() => load(m.id)}
              >
                <span className="overflow-hidden text-ellipsis whitespace-nowrap">
                  {m.title || (m.state === "recording" ? "Recording…" : "Untitled meeting")}
                </span>
                <span className={"text-xs " + (selected === m.id ? "opacity-70" : "text-muted")}>
                  {hhmm(m.started_at)}
                  {m.state === "failed" && (
                    <span className={selected === m.id ? "" : "text-bad"}> · notes failed</span>
                  )}
                  {m.state === "summarizing" && <span> · summarizing…</span>}
                  {m.state === "done" && !m.has_summary && <span> · no notes yet</span>}
                  {live && m.id === recording && (
                    <span className={selected === m.id ? "" : "text-bad"}> · live</span>
                  )}
                </span>
              </button>
            </div>
          );
        })}
      </aside>

      <div className="min-w-0 max-w-[1040px] flex-1 [&_section]:mt-5 [&_p]:max-w-[60ch]">
        {!detail && <p className="text-muted">Select a meeting.</p>}
        {/* Keyed by meeting, so the tab and the editor reset on a switch but
            survive the two-minute `refresh` replacements of `detail` for the
            same meeting. */}
        {detail && <Detail
          key={detail.meeting.id}
          detail={detail}
          isRecording={live && detail.meeting.id === recording}
          ticked={ticked}
          setTicked={setTicked}
          onApprove={approve}
          onRerun={rerun}
          onResume={() => resume(detail.meeting.id)}
          canRecord={canRecord}
          onDelete={() => remove(detail.meeting.id)}
          notesDraft={notes.draft}
          onEditNotes={notes.edit}
          onBlurNotes={notes.flush}
          transcriptDraft={transcript.draft}
          onEditTranscript={transcript.edit}
          onBlurTranscript={transcript.flush}
          summaryDraft={summary.draft}
          onEditSummary={summary.edit}
          onBeginEditSummary={beginEditSummary}
          onEndEditSummary={endEditSummary}
          onAttach={attach}
          onDetach={detachFile}
          attaching={attaching}
          busy={busy}
        />}
      </div>
    </div>
  );
}

type Tab = "summary" | "notes" | "transcript";

function Detail({
  detail,
  isRecording,
  ticked,
  setTicked,
  onApprove,
  onRerun,
  onResume,
  canRecord,
  onDelete,
  notesDraft,
  onEditNotes,
  onBlurNotes,
  transcriptDraft,
  onEditTranscript,
  onBlurTranscript,
  summaryDraft,
  onEditSummary,
  onBeginEditSummary,
  onEndEditSummary,
  onAttach,
  onDetach,
  attaching,
  busy,
}: {
  detail: MeetingDetail;
  isRecording: boolean;
  ticked: Set<number>;
  setTicked: (s: Set<number>) => void;
  onApprove: () => void;
  onRerun: () => void;
  onResume: () => void;
  // Nothing else holds the microphone, so this meeting may open it again.
  canRecord: boolean;
  onDelete: () => void;
  notesDraft: string;
  onEditNotes: (text: string) => void;
  onBlurNotes: () => void;
  transcriptDraft: string;
  onEditTranscript: (text: string) => void;
  onBlurTranscript: () => void;
  summaryDraft: string;
  onEditSummary: (text: string) => void;
  onBeginEditSummary: () => void;
  onEndEditSummary: () => Promise<void>;
  onAttach: () => void;
  onDetach: (fileId: number) => void;
  attaching: boolean;
  busy: boolean;
}) {
  const { meeting, segments, actions, files } = detail;
  const running = meeting.state === "summarizing";
  const finished = meeting.state === "done" || meeting.state === "failed";
  // Capture may still be writing segments: the microphone is open, or Stop
  // has shut it and the last block is still on its way. The transcript is
  // shown, not edited, until then — the backend refuses a save as well.
  const capturing = meeting.state === "recording";
  // A stopped meeting with no notes is waiting on its transcript to be
  // reviewed, so that is where it opens.
  const [tab, setTab] = useState<Tab>(detail.summary ? "summary" : "transcript");
  // The preview trails the textarea: typing stays immediate, and the markdown
  // pass runs when React has a moment rather than on every keystroke.
  const previewSource = useDeferredValue(summaryDraft);
  // The write-up was made from the transcript, so an edit to it (or a resume
  // that grew it) puts the write-up behind. Says so rather than re-running by
  // itself: a run is model time, and that is the user's to spend.
  const stale = detail.transcript_edited_at != null && detail.summary != null;
  // The write-up's editor is open. Its draft is only live while this is true,
  // which is what keeps a re-run's fresh document from being shadowed by a
  // draft seeded from the old one.
  const [editing, setEditing] = useState(false);
  // Re-run is asking before it replaces a hand-edited document.
  const [confirm, setConfirm] = useState(false);
  const pending = actions.filter((a) => a.task_id == null);
  const approved = actions.filter((a) => a.task_id != null);
  const toggle = (id: number) => {
    const next = new Set(ticked);
    next.has(id) ? next.delete(id) : next.add(id);
    setTicked(next);
  };

  // The document has been changed by hand: either saved before (the store's
  // mark), or typed just now and not yet landed. Either way a re-run would
  // throw it away, so it asks first.
  const edited =
    detail.summary_edited_at != null ||
    (editing && summaryDraft !== (detail.summary ?? ""));

  const beginEdit = () => {
    onBeginEditSummary();
    setEditing(true);
  };
  const endEdit = async () => {
    await onEndEditSummary();
    setEditing(false);
  };
  const rerun = () => {
    setConfirm(false);
    setEditing(false);
    // Where "Writing the notes…" shows, and where they land.
    setTab("summary");
    onRerun();
  };

  // Write notes, or Re-run over an edited document behind an inline confirm:
  // the webview has no reliable dialog. Offered on both the transcript tab,
  // where the review ends, and the summary tab.
  const writeControls = confirm ? (
    <>
      <span className="text-xs text-warn">This replaces the document you edited.</span>
      <button onClick={rerun} disabled={busy}>
        Re-run anyway
      </button>
      <button onClick={() => setConfirm(false)}>Keep it</button>
    </>
  ) : (
    <Button
      tone={detail.summary ? "ghost" : "primary"}
      onClick={() => (edited ? setConfirm(true) : rerun())}
      disabled={busy}
    >
      {detail.summary ? "Re-run notes" : "Write notes"}
    </Button>
  );

  const notesPane = (
    <section className="max-w-[68ch]">
      <h3>My notes</h3>
      <textarea
        className="min-h-[220px] w-full resize-y rounded-md border border-line bg-surface px-2.5 py-2 text-sm leading-relaxed text-text outline-none transition-colors focus:border-accent read-only:text-muted"
        value={notesDraft}
        onChange={(e) => onEditNotes(e.target.value)}
        onBlur={onBlurNotes}
        readOnly={running}
        spellCheck={false}
      />
    </section>
  );

  const filesPane = (
    <section className="max-w-[68ch]">
      <h3>Context files</h3>
      {files.length > 0 && (
        <ul className="m-0 mb-2 flex list-none flex-wrap gap-1.5 p-0">
          {files.map((f) => (
            <li
              key={f.id}
              className="flex max-w-full items-center gap-1.5 rounded-full border border-line bg-raised py-1 pl-2 pr-1 text-xs"
            >
              {/* A dot per kind, as a real element rather than ::before, so the
                  colour can be a literal class the scanner sees. */}
              <span className={"h-1.5 w-1.5 flex-none rounded-full " + (KIND_DOT[f.kind] ?? "bg-muted")} />
              <span className="max-w-[26ch] overflow-hidden text-ellipsis whitespace-nowrap" title={f.name}>
                {f.name}
              </span>
              <span className="text-muted">{size(f.bytes)}</span>
              <button
                className="border-none px-1 text-sm leading-none text-muted transition-colors hover:enabled:text-bad disabled:text-line"
                onClick={() => onDetach(f.id)}
                disabled={busy || attaching || running}
                aria-label={`Remove ${f.name}`}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
      <button onClick={onAttach} disabled={busy || attaching || running}>
        {attaching ? "Working…" : "Add files"}
      </button>
      {/* Said plainly and next to the control, because it is the one thing
          here that leaves the machine. */}
      <p className="mt-1.5 max-w-[60ch] text-xs text-muted">
        Text, code, PDFs, images and Office files. Their contents are sent to
        Anthropic when the notes are written.
      </p>
    </section>
  );

  // While capture is running: the blocks as they land, timestamped, read
  // only. Once it has stopped: one box holding the whole transcript, which is
  // exactly the text Write notes sends.
  const transcriptPane = (
    <section>
      <h3>Transcript</h3>
      {capturing ? (
        <>
          {segments.length === 0 && (
            <p className="text-muted">
              {isRecording ? "Listening — the first block lands after two minutes." : "Finishing the last block…"}
            </p>
          )}
          <div className="max-w-[68ch] whitespace-pre-line">
            {segments.map((s) =>
              s.text ? (
                <p key={s.id}>
                  <span className="mr-2 text-xs text-muted">{hhmm(s.started_at)}</span> {s.text}
                </p>
              ) : (
                // A visible gap, not a silent one: a chunk that failed to
                // transcribe is two minutes the notes will never see — unless
                // it is typed back in once the meeting has stopped.
                <p key={s.id} className="my-2 text-xs text-muted">
                  <span className="mr-2">{hhmm(s.started_at)}</span> (this stretch could not be transcribed)
                </p>
              ),
            )}
          </div>
          {!isRecording && segments.length > 0 && (
            <p className="text-xs text-muted">Finishing the last block…</p>
          )}
        </>
      ) : (
        <>
          {!running && (
            <p className="my-1.5 text-xs text-muted">
              Fix names and mis-heard words, then write the notes.
            </p>
          )}
          {stale && finished && (
            <p className="my-1.5 text-xs text-warn">
              The notes were written before this text changed. Re-run notes to
              rebuild them from it.
            </p>
          )}
          <textarea
            className="min-h-[60vh] w-full max-w-[80ch] resize-y rounded-md border border-line bg-surface px-3 py-2.5 text-sm leading-relaxed text-text outline-none transition-colors focus:border-accent read-only:text-muted"
            value={transcriptDraft}
            onChange={(e) => onEditTranscript(e.target.value)}
            onBlur={onBlurTranscript}
            readOnly={running}
            placeholder="Nothing was transcribed. Type what was said here and it goes to the notes."
            spellCheck={false}
          />
          {finished && <div className="mt-3 flex flex-wrap items-center gap-2">{writeControls}</div>}
        </>
      )}
    </section>
  );

  // The write-up: the model's markdown until Edit is pressed, then a source
  // editor with a live render under it, so LaTeX and code can be checked as
  // they are typed rather than after Done.
  const summaryPane = (
    <section>
      <div className="flex items-center gap-2.5">
        <h3>Summary</h3>
        {finished && !editing && (
          <button className="border-none bg-transparent p-0 text-accent underline decoration-dotted hover:text-muted" onClick={beginEdit} disabled={busy}>
            Edit
          </button>
        )}
        {editing && (
          <button className="border-none bg-transparent p-0 text-accent underline decoration-dotted hover:text-muted" onClick={endEdit} disabled={busy}>
            Done
          </button>
        )}
        {!editing && detail.summary_edited_at != null && (
          <span className="text-xs italic text-muted">edited by you</span>
        )}
      </div>
      {stale && finished && !editing && (
        <p className="my-1.5 text-xs text-warn">
          Written before the transcript changed. Re-run notes to rebuild it
          from the current text.
        </p>
      )}
      {editing ? (
        <>
          <textarea
            className="min-h-[260px] w-full resize-y rounded-md border border-line bg-surface px-2.5 py-2 font-mono text-[13px] leading-relaxed text-text outline-none transition-colors focus:border-accent [tab-size:2]"
            value={summaryDraft}
            onChange={(e) => onEditSummary(e.target.value)}
            spellCheck={false}
            autoFocus
          />
          <p className="mt-1.5 max-w-[60ch] text-xs text-muted">
            Markdown. Code goes in ``` fences with a language; maths in $…$ inline
            or $$…$$ on its own lines.
          </p>
          <div className="mt-3 border-t border-line pt-3">
            <p className="text-muted">Preview</p>
            {previewSource.trim() ? (
              <Markdown source={previewSource} />
            ) : (
              <p className="text-muted">Nothing to show yet.</p>
            )}
          </div>
        </>
      ) : detail.summary ? (
        <Markdown source={detail.summary} />
      ) : (
        !running &&
        (finished ? (
          <p className="text-muted">
            No notes yet —{" "}
            <button
              className="border-none bg-transparent p-0 text-accent underline decoration-dotted hover:text-muted"
              onClick={() => setTab("transcript")}
            >
              review the transcript
            </button>
            , then write them.
          </p>
        ) : (
          <p className="text-muted">No notes yet.</p>
        ))
      )}
    </section>
  );

  const actionsPane = actions.length > 0 && (
    <section>
      <SectionHeading className="mb-2 mt-4">Action items</SectionHeading>
      {pending.map((a) => (
        // The whole row is the label, so the text is part of the hit target.
        <label
          key={a.id}
          className="flex max-w-[60ch] cursor-pointer items-center gap-2.5 py-1"
        >
          <Checkbox size={26} checked={ticked.has(a.id)} onChange={() => toggle(a.id)} />
          <span className="min-w-0 flex-1">{a.text}</span>
        </label>
      ))}
      {pending.length > 0 && (
        <Button tone="primary" className="mt-3" onClick={onApprove} disabled={busy || ticked.size === 0}>
          Add {ticked.size} to backlog
        </Button>
      )}
      {approved.map((a) => (
        <div key={a.id} className="flex max-w-[60ch] items-center gap-2.5 py-1 text-muted">
          {/* Already a task: the same control, ticked and inert. */}
          <Checkbox size={26} checked disabled label={`${a.text} — already in the backlog`} />
          <span className="min-w-0 flex-1">
            {a.text}
            <span className="text-muted"> · in the backlog</span>
          </span>
        </div>
      ))}
    </section>
  );

  // Any stopped meeting. Resume no longer asks first: its Stop leaves the
  // write-up alone, and only Write notes / Re-run notes replaces it.
  const finishedRow = finished && (
    <div className="mt-5 flex flex-wrap items-center gap-2">
      {!confirm && (
        <button onClick={onResume} disabled={busy || !canRecord}>
          ● Resume transcribing
        </button>
      )}
      {writeControls}
    </div>
  );

  return (
    <>
      <h2>{meeting.title || (isRecording ? "Recording…" : "Untitled meeting")}</h2>
      <p className="text-muted">
        {hhmm(meeting.started_at)}
        {meeting.ended_at ? `–${hhmm(meeting.ended_at)}` : ""} · {meeting.segment_count} segment
        {meeting.segment_count === 1 ? "" : "s"}
        {meeting.session_id == null && " · outside a session"}
      </p>

      {/* While recording there is nothing to summarize, and the transcript
          and the scratchpad sit side by side: the whole point of the
          scratchpad is noting down what you are reading land wrong, so they
          have to be readable at the same time. Once the meeting is over the
          page becomes a document with three tabs. */}
      {isRecording ? (
        <div className="grid grid-cols-1 gap-4 min-[1100px]:grid-cols-2">
          {transcriptPane}
          <div>
            {notesPane}
            {filesPane}
          </div>
        </div>
      ) : (
        <>
          <nav className="mt-3.5 flex gap-2 border-b border-line pb-2.5">
            <TabButton active={tab === "summary"} onClick={() => setTab("summary")}>
              Summary
            </TabButton>
            <TabButton active={tab === "notes"} onClick={() => setTab("notes")}>
              Notes
            </TabButton>
            <TabButton active={tab === "transcript"} onClick={() => setTab("transcript")}>
              Transcript
            </TabButton>
          </nav>

          {tab === "summary" && (
            <>
              {meeting.state === "failed" && (
                <div className="mt-4 rounded-md border border-bad px-3 py-2.5 [&_p]:my-1 [&_p]:text-[13px]">
                  <strong>The notes could not be written.</strong>
                  <p className="text-muted">{meeting.error}</p>
                </div>
              )}
              {running && <p className="text-muted">Writing the notes…</p>}
              {summaryPane}
              {actionsPane}
              {finishedRow}
            </>
          )}
          {tab === "notes" && (
            <>
              {notesPane}
              {filesPane}
            </>
          )}
          {tab === "transcript" && transcriptPane}

          <button className="mt-6 rounded-md border border-line px-2.5 py-1 text-muted transition-colors hover:border-bad hover:text-bad" onClick={onDelete} disabled={busy}>
            Delete meeting
          </button>
        </>
      )}
    </>
  );
}
