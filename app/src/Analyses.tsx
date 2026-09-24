import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import { ALIGN_TEXT, band, dayKey, dayLabel, fmt, hhmm, OTHER, rankTitles } from "./format";
import { BarGroup, BarMore, BarRow, Bars } from "./ui/Bars";
import type { Analysis, Observed } from "./types";

const TOP_APPS = 8;
// Mirrors TOP_TITLES_PER_APP in src-tauri/src/claude.rs. This pane is headed
// "What the model saw", so the cut has to be the prompt's cut, not a taste
// judgement — anything past it is shown separately and labelled as unsent.
const MODEL_TITLES = 5;

export default function Analyses({
  hasSession,
  onSeen,
}: {
  hasSession: boolean;
  onSeen: () => void;
}) {
  const [items, setItems] = useState<Analysis[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [observed, setObserved] = useState<Observed | null>(null);
  const [running, setRunning] = useState<"check" | "checkpoint" | null>(null);
  const [runNote, setRunNote] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  const selectedRef = useRef<number | null>(null);
  useEffect(() => {
    selectedRef.current = selected;
  }, [selected]);
  // Marking is idempotent server-side, but onSeen is not: this keeps the badge
  // honest under StrictMode's double mount and under a double click.
  const marked = useRef(new Set<number>());

  const select = useCallback(
    (a: Analysis) => {
      setSelected(a.id);
      setObserved(null);
      setExpanded(new Set());
      api.getAnalysisObserved(a.id).then(setObserved).catch(() => {});
      if (a.seen_at || marked.current.has(a.id)) return;
      marked.current.add(a.id);
      api
        .markAnalysisSeen(a.id)
        .then(() => {
          // Any non-null stamp does: the UI only reads seen_at as a boolean.
          const at = new Date().toISOString();
          setItems((prev) => prev.map((x) => (x.id === a.id ? { ...x, seen_at: at } : x)));
          onSeen();
        })
        .catch(() => marked.current.delete(a.id));
    },
    [onSeen],
  );

  const reload = useCallback(() => api.listRecentAnalyses(api.RECENT_ANALYSES), []);

  useEffect(() => {
    reload()
      .then((list) => {
        setItems(list);
        if (list.length > 0 && selectedRef.current == null) select(list[0]);
      })
      .catch(() => {});
    // A check landing while the tab is open must not yank the pane out from
    // under a read, so the selection is left alone once there is one.
    const unlisten = listen("analysis:new", () => {
      reload()
        .then((list) => {
          setItems(list);
          if (list.length > 0 && selectedRef.current == null) select(list[0]);
        })
        .catch(() => {});
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [reload, select]);

  const runNow = () => {
    setRunning("check");
    setRunNote(null);
    api
      .runAnalysisNow()
      .then(async (id) => {
        if (id == null) {
          setRunNote("Skipped — under 5 minutes of tracked activity since the last check.");
          return;
        }
        const list = await reload();
        setItems(list);
        const fresh = list.find((a) => a.id === id);
        if (fresh) select(fresh);
      })
      .catch((e) => setRunNote(String(e)))
      .finally(() => setRunning(null));
  };

  // The checkpoint without waiting out the clock. It puts its own screen up
  // (App listens for checkpoint:new), so there is nothing to select here.
  const runCheckpoint = () => {
    setRunning("checkpoint");
    setRunNote(null);
    api
      .runCheckpointNow()
      .then(() => reload().then(setItems))
      .catch((e) => setRunNote(String(e)))
      .finally(() => setRunning(null));
  };

  const current = items.find((a) => a.id === selected) ?? null;
  const apps = observed
    ? Object.entries(observed.per_app)
        .sort((a, b) => b[1] - a[1])
        .slice(0, TOP_APPS)
    : [];
  const max = apps.length > 0 ? apps[0][1] : 1;

  const toggle = (app: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (!next.delete(app)) next.add(app);
      return next;
    });

  const windowSeconds = current
    ? (new Date(current.window_end).getTime() - new Date(current.window_start).getTime()) / 1000
    : 0;

  // Newest first, so same-day rows are already adjacent.
  const groups: { key: string; label: string; items: Analysis[] }[] = [];
  for (const a of items) {
    const key = dayKey(a.created_at);
    const last = groups[groups.length - 1];
    if (last && last.key === key) last.items.push(a);
    else groups.push({ key, label: dayLabel(a.created_at), items: [a] });
  }

  return (
    <div className="flex items-start gap-4 p-4">
      <aside className="max-h-[calc(100vh-90px)] w-[260px] flex-none overflow-y-auto">
        {groups.map((g) => (
          <div key={g.key} className="mb-3.5 flex flex-col gap-1.5">
            <h3>{g.label}</h3>
            {g.items.map((a) => {
              const b = band(a.alignment);
              // On the accent fill every child has to inherit the dark text;
              // a muted grey or a band colour on cyan is unreadable.
              const on = a.id === selected;
              return (
                <button
                  key={a.id}
                  className={
                    "block w-full overflow-hidden text-ellipsis whitespace-nowrap rounded-md " +
                    "border px-2.5 py-1.5 text-left transition-colors duration-150 " +
                    (a.id === selected
                      ? "border-accent bg-accent text-bg "
                      : "border-line text-text hover:border-muted ") +
                    // Unread checks are the ones worth returning to.
                    (a.seen_at ? "" : "font-semibold")
                  }
                  onClick={() => select(a)}
                >
                  <span className={on ? "opacity-70" : "text-muted"}>{hhmm(a.created_at)}</span>{" "}
                  {a.kind === "checkpoint" && <span className={"rounded px-1 text-[11px] " + (on ? "bg-black/20" : "bg-raised text-muted")}>time&rsquo;s up</span>}{" "}
                  {a.headline}
                  <span className={"float-right ml-2 font-normal " + (on ? "" : ALIGN_TEXT[b.cls])} title={b.label}>
                    {b.glyph} {a.alignment ?? "—"}
                  </span>
                </button>
              );
            })}
          </div>
        ))}
        {items.length === 0 && <p className="text-muted">No checks yet.</p>}
      </aside>

      <section className="min-w-0 max-w-[720px] flex-1">
        {current ? (
          <>
            <div className="flex items-start justify-between gap-4">
              <div>
                <h2>{current.headline}</h2>
                <p className="text-muted">
                  {dayLabel(current.created_at)} {hhmm(current.created_at)} · covers{" "}
                  {hhmm(current.window_start)}–{hhmm(current.window_end)} ({fmt(windowSeconds)})
                </p>
              </div>
              <div className={"flex flex-none flex-col items-end " + ALIGN_TEXT[band(current.alignment).cls]}>
                <span className="text-2xl leading-tight">
                  {band(current.alignment).glyph} {current.alignment ?? "—"}
                </span>
                <span className="text-xs">{band(current.alignment).label}</span>
              </div>
            </div>
            <p className="max-w-[60ch]">{current.body}</p>
            {current.recommendation && (
              <div className="mt-5 max-w-[60ch] rounded-lg border border-line bg-surface px-4 py-3.5">
                <h3>Recommended</h3>
                <p className="mb-1.5 text-[15px]">
                  {current.recommendation.advice}
                  {current.recommendation.minutes > 0 && (
                    <span className="text-muted"> · about {current.recommendation.minutes} min</span>
                  )}
                </p>
                {current.recommendation_note && (
                  <p className="mb-2 text-[13px] text-muted">{current.recommendation_note}</p>
                )}
                <p className="mb-1.5 text-text">{current.recommendation.why}</p>
                <p className="text-xs italic text-muted">
                  {current.recommendation.source ?? "Not yet backed by a cited study."}
                </p>
              </div>
            )}
            <p className="text-muted">
              #{current.session_id}
              {current.session_statement ? ` · ${current.session_statement}` : ""}
            </p>

            <h3>What the model saw</h3>
            {observed && (
              <p className="text-muted">
                {fmt(observed.active_seconds)} active, {fmt(observed.afk_seconds)} away
              </p>
            )}
            <Bars>
              {apps.map(([app, secs]) => {
                const titles = rankTitles(observed, app);
                // "(other)" reaches the model too — as the prompt's remainder
                // line — so it counts as seen however many titles it stands
                // for, and it stays pinned last. Only real titles past the
                // prompt's cut are the ones the model never got.
                const real = titles.filter(([t]) => t !== OTHER);
                const other = titles.filter(([t]) => t === OTHER);
                const hidden = real.slice(MODEL_TITLES);
                const open = expanded.has(app);
                const rows = [
                  ...real.slice(0, MODEL_TITLES).map(([t, s]) => ({ t, s, unsent: false })),
                  ...(open ? hidden.map(([t, s]) => ({ t, s, unsent: true })) : []),
                  ...other.map(([t, s]) => ({ t, s, unsent: false })),
                ];
                return (
                  <BarGroup key={app}>
                    <BarRow label={app} value={fmt(secs)} pct={(secs / max) * 100} />
                    {rows.map((r) => (
                      <BarRow
                        key={r.t}
                        label={r.t}
                        value={fmt(r.s)}
                        pct={(r.s / max) * 100}
                        sub
                        unsent={r.unsent}
                      />
                    ))}
                    {hidden.length > 0 && (
                      <BarMore onClick={() => toggle(app)}>
                        {open
                          ? "▾ hide the rest"
                          : `▸ +${hidden.length} more — recorded, not sent to the model`}
                      </BarMore>
                    )}
                  </BarGroup>
                );
              })}
              {observed && apps.length === 0 && <p className="text-muted">nothing recorded</p>}
            </Bars>
          </>
        ) : (
          <p className="text-muted">
            No checks yet — they land at random intervals, and quiet windows are skipped.
          </p>
        )}

        <div className="mt-5 flex items-center gap-2.5">
          <button onClick={runNow} disabled={running != null || !hasSession}>
            {running === "check" ? "checking…" : "Run a check now"}
          </button>
          <button onClick={runCheckpoint} disabled={running != null || !hasSession}>
            {running === "checkpoint" ? "checking…" : "Run a checkpoint now"}
          </button>
          {!hasSession && <span className="text-muted">no open session</span>}
          {runNote && <span className="text-muted">{runNote}</span>}
        </div>
      </section>
    </div>
  );
}
