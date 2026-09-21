import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { LevelBars, type LevelBarsHandle } from "./LevelBars";
import Markdown from "./Markdown";
import Checkbox from "./ui/Checkbox";
import { Button, SectionHeading, Tab as TabButton } from "./ui/primitives";
import * as api from "./api";
import { dayKey, dayLabel, fmt, hhmm, size } from "./format";
import type { AudioSource, Level, Meeting, MeetingDetail, RecordingNow } from "./types";

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
/// Used twice — the scratchpad and the write-up — and the rules are the same
/// for both. The draft, not the stored value, is what the textarea shows:
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
      setDraft(text);
    }
  }, []);

  const edit = (text: string) => {
    setDraft(text);
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

  return { draft, seed, edit, flush };
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
  const [elapsed, setElapsed] = useState(0);
  const [sources, setSources] = useState<AudioSource[]>([]);
  const [source, setSource] = useState<AudioSource | null>(rememberedSource);
  const [sourceNote, setSourceNote] = useState<string | null>(null);

  const fail = useCallback((e: unknown) => setError(String(e)), []);
  // The scratchpad and the write-up, each with a draft of its own. See
  // useAutosave for why the drafts, not `detail`, are what the textareas show.
  const notes = useAutosave(api.setMeetingNotes, fail);
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
  // and the meter must all stop with it even though the notes are still being
  // written.
  const live = recording != null && !stopping;

  // The notes for a meeting are still being written. Read off the list rather
  // than kept in a state of its own, so it survives this tab being unmounted
  // and remounted — `items` is refreshed by both `meeting:state` and
  // `meeting:done`. `stopping` covers the gap before the first of those lands.
  //
  // Both wrap-up states, so the Start button stays down through the repair
  // pass as well as the summary — on a long meeting the repair is the slower
  // of the two.
  const notesPending =
    stopping || items.some((m) => m.state === "cleaning" || m.state === "summarizing");
  const wrapUp = items.find(
    (m) => m.state === "cleaning" || m.state === "summarizing",
  )?.state;

  const reload = useCallback(() => api.listMeetings(api.RECENT_MEETINGS), []);

  const seedNotes = notes.seed;
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
        })
        .catch(fail);
    },
    [seedNotes, fail],
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
        api.getMeeting(selectedRef.current).then(setDetail).catch(() => {});
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
  }, [reload, load, fail, setNow]);

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

  // The elapsed clock is cosmetic and derived from when the backend says the
  // microphone opened, so it stays right across a tab switch instead of
  // restarting — and a resumed meeting counts from the resume, not from the
  // day it was first recorded.
  useEffect(() => {
    if (!live || since == null) {
      setElapsed(0);
      return;
    }
    const began = new Date(since).getTime();
    const tick = () => setElapsed(Math.max(0, Math.round((Date.now() - began) / 1000)));
    tick();
    const timer = setInterval(tick, 1000);
    return () => clearInterval(timer);
  }, [live, since]);

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

  /// Pick a finished meeting back up. Both drafts are flushed first for the
  /// same reason as a re-run: the next Stop reads the notes, and the pane
  /// asked before letting an edited write-up get here.
  const resume = async (id: number) => {
    setError(null);
    setBusy(true);
    await notes.flush().catch(() => {});
    await summary.flush().catch(() => {});
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
      // A failed *stop*, which is all this call does now. A failed summary is
      // not a failed meeting and never reaches here: the transcript is saved,
      // and the detail pane below says what went wrong and offers a retry.
      .catch((e) => {
        setError(String(e));
        setNow(null);
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
      .catch(fail)
      .finally(() => setBusy(false));
  };

  /// Repair and re-summarize a meeting that already finished. The whole point
  /// is that it re-reads the notes, so both drafts are flushed first — the
  /// usual reason to press this is having just corrected a name the model got
  /// wrong. (The write-up's draft is flushed only to be replaced moments
  /// later; the pane asked before letting an edited document get here.)
  const rerun = async () => {
    if (!detail) return;
    setBusy(true);
    setError(null);
    const id = detail.meeting.id;
    await notes.flush().catch(() => {});
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
          {/* Three states, not two. The wrap-up needs one of its own: the
              microphone is already shut but the tail chunk and the notes are
              still in flight, and a Start button offered there would take a
              double-click meant for Stop and open a second recording. */}
          {live ? (
            <button className="w-full rounded-md border border-bad p-2.5 text-sm text-bad transition-colors hover:brightness-110 disabled:opacity-40" onClick={stop} disabled={busy || attaching}>
              ■ Stop transcribing
            </button>
          ) : notesPending ? (
            <button className="w-full rounded-md border border-line p-2.5 text-sm text-muted disabled:opacity-40" disabled>
              {wrapUp === "cleaning" ? "Cleaning the transcript…" : "Writing the notes…"}
            </button>
          ) : (
            <button className="w-full rounded-md border border-line p-2.5 text-sm transition-colors hover:border-muted disabled:opacity-40 disabled:hover:border-line" onClick={start} disabled={busy}>
              ● Start transcribing
            </button>
          )}
          {live && (
            <div className="flex items-center gap-2 text-[13px] text-bad">
              <span className="rec-dot" /> recording {fmt(elapsed)}
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
                    ? "border-accent bg-accent text-black"
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
        {/* Keyed by meeting, so the tab, the editor and the raw-transcript
            toggle reset on a switch but survive the two-minute `refresh`
            replacements of `detail` for the same meeting. */}
        {detail && <Detail
          key={detail.meeting.id}
          detail={detail}
          isRecording={live && detail.meeting.id === recording}
          ticked={ticked}
          setTicked={setTicked}
          onApprove={approve}
          onRerun={rerun}
          onResume={() => resume(detail.meeting.id)}
          canRecord={recording == null && !notesPending}
          onDelete={() => remove(detail.meeting.id)}
          notesDraft={notes.draft}
          onEditNotes={notes.edit}
          onBlurNotes={notes.flush}
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
  // Nothing else holds the microphone or is mid-write-up, so this meeting
  // may open it again.
  canRecord: boolean;
  onDelete: () => void;
  notesDraft: string;
  onEditNotes: (text: string) => void;
  onBlurNotes: () => void;
  summaryDraft: string;
  onEditSummary: (text: string) => void;
  onBeginEditSummary: () => void;
  onEndEditSummary: () => Promise<void>;
  onAttach: () => void;
  onDetach: (fileId: number) => void;
  attaching: boolean;
  busy: boolean;
}) {
  const { meeting, segments, actions, clean_transcript, files } = detail;
  const running = meeting.state === "cleaning" || meeting.state === "summarizing";
  const finished = meeting.state === "done" || meeting.state === "failed";
  const [tab, setTab] = useState<Tab>("summary");
  const [showRaw, setShowRaw] = useState(false);
  // The write-up's editor is open. Its draft is only live while this is true,
  // which is what keeps a re-run's fresh document from being shadowed by a
  // draft seeded from the old one.
  const [editing, setEditing] = useState(false);
  // Which of the two document-replacing actions is asking first, if either.
  const [confirm, setConfirm] = useState<null | "rerun" | "resume">(null);
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
    setConfirm(null);
    setEditing(false);
    onRerun();
  };
  const resume = () => {
    setConfirm(null);
    setEditing(false);
    onResume();
  };

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

  // Never while recording: a resumed meeting's cleaned transcript stops where
  // the last Stop did, and the new segments are what is worth watching land.
  // (The backend clears it on resume; this covers the refresh in between.)
  const hasClean = !!clean_transcript && !isRecording;
  const transcriptPane = (
    <section>
      <h3>
        {hasClean && !showRaw ? "Transcript" : "Raw transcript"}
        {hasClean && (
          // The raw segments are still the record of what was heard, so the
          // repaired version never replaces them — it just gets shown first.
          <button className="border-none bg-transparent p-0 text-accent underline decoration-dotted hover:brightness-110" onClick={() => setShowRaw(!showRaw)}>
            {showRaw ? "Show cleaned" : "Show raw"}
          </button>
        )}
      </h3>
      {hasClean && !showRaw && (
        <div className="max-w-[68ch] whitespace-pre-line">
          {clean_transcript!.split("\n\n").map((para, i) => (
            <p key={i}>{para}</p>
          ))}
        </div>
      )}
      {(!hasClean || showRaw) && (
      <>
      {segments.length === 0 && (
        <p className="text-muted">
          {isRecording ? "Listening — the first block lands after two minutes." : "Nothing was transcribed."}
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
            // transcribe is two minutes the notes below never saw.
            <p key={s.id} className="my-2 block text-xs text-muted">
              <span className="mr-2 text-xs text-muted">{hhmm(s.started_at)}</span> (this stretch could not be
              transcribed)
            </p>
          ),
        )}
      </div>
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
        {!running && !editing && (
          <button className="border-none bg-transparent p-0 text-accent underline decoration-dotted hover:brightness-110" onClick={beginEdit} disabled={busy}>
            Edit
          </button>
        )}
        {editing && (
          <button className="border-none bg-transparent p-0 text-accent underline decoration-dotted hover:brightness-110" onClick={endEdit} disabled={busy}>
            Done
          </button>
        )}
        {!editing && detail.summary_edited_at != null && (
          <span className="text-xs italic text-muted">edited by you</span>
        )}
      </div>
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
            {summaryDraft.trim() ? (
              <Markdown source={summaryDraft} />
            ) : (
              <p className="text-muted">Nothing to show yet.</p>
            )}
          </div>
        </>
      ) : detail.summary ? (
        <Markdown source={detail.summary} />
      ) : (
        !running && <p className="text-muted">No notes yet.</p>
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

  // Any finished meeting, not just a failed one: the usual reason to re-run
  // is having corrected something in the notes, and the reason to resume is
  // that the meeting was not actually over. Both end with the model rewriting
  // the document — a resume at its next Stop — so over a hand-edited one
  // either asks first, inline: the webview has no reliable dialog.
  const finishedRow = finished && (
    <div className="mt-5 flex flex-wrap items-center gap-2">
      {confirm ? (
        <>
          <span className="text-xs text-warn">
            {confirm === "rerun"
              ? "This replaces the document you edited."
              : "Stopping will replace the document you edited."}
          </span>
          <button
            onClick={confirm === "rerun" ? rerun : resume}
            disabled={busy || (confirm === "resume" && !canRecord)}
          >
            {confirm === "rerun" ? "Re-run anyway" : "Resume anyway"}
          </button>
          <button onClick={() => setConfirm(null)}>Keep it</button>
        </>
      ) : (
        <>
          <button
            onClick={() => (edited ? setConfirm("resume") : resume())}
            disabled={busy || !canRecord}
          >
            ● Resume transcribing
          </button>
          <button onClick={() => (edited ? setConfirm("rerun") : rerun())} disabled={busy}>
            Re-run notes
          </button>
        </>
      )}
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
          scratchpad is correcting what you are reading land wrong, so they
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
              {meeting.state === "cleaning" && <p className="text-muted">Cleaning the transcript…</p>}
              {meeting.state === "summarizing" && <p className="text-muted">Writing the notes…</p>}
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

          <button className="rounded-md border border-line px-2.5 py-1 text-muted transition-colors hover:border-bad hover:text-bad" onClick={onDelete} disabled={busy}>
            Delete meeting
          </button>
        </>
      )}
    </>
  );
}
