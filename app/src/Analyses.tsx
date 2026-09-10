import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import { band, dayKey, dayLabel, fmt, hhmm, OTHER, rankTitles } from "./format";
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
    <div className="analyses">
      <aside className="analysis-list">
        {groups.map((g) => (
          <div key={g.key} className="analysis-day">
            <h3>{g.label}</h3>
            {g.items.map((a) => {
              const b = band(a.alignment);
              return (
                <button
                  key={a.id}
                  className={`${a.id === selected ? "active" : ""}${a.seen_at ? "" : " unseen"}`}
                  onClick={() => select(a)}
                >
                  <span className="muted">{hhmm(a.created_at)}</span>{" "}
                  {a.kind === "checkpoint" && <span className="kind">time's up</span>}{" "}
                  {a.headline}
                  <span className={`align ${b.cls}`} title={b.label}>
                    {b.glyph} {a.alignment ?? "—"}
                  </span>
                </button>
              );
            })}
          </div>
        ))}
        {items.length === 0 && <p className="muted">No checks yet.</p>}
      </aside>

      <section className="analysis-detail">
        {current ? (
          <>
            <div className="detail-head">
              <div>
                <h2>{current.headline}</h2>
                <p className="muted">
                  {dayLabel(current.created_at)} {hhmm(current.created_at)} · covers{" "}
                  {hhmm(current.window_start)}–{hhmm(current.window_end)} ({fmt(windowSeconds)})
                </p>
              </div>
              <div className={`align-stat align ${band(current.alignment).cls}`}>
                <span className="value">
                  {band(current.alignment).glyph} {current.alignment ?? "—"}
                </span>
                <span className="label">{band(current.alignment).label}</span>
              </div>
            </div>
            <p className="detail-body">{current.body}</p>
            {current.recommendation && (
              <div className="recommend">
                <h3>Recommended</h3>
                <p className="advice">
                  {current.recommendation.advice}
                  {current.recommendation.minutes > 0 && (
                    <span className="muted"> · about {current.recommendation.minutes} min</span>
                  )}
                </p>
                {current.recommendation_note && (
                  <p className="muted note">{current.recommendation_note}</p>
                )}
                <p className="why">{current.recommendation.why}</p>
                <p className="muted source">
                  {current.recommendation.source ?? "Not yet backed by a cited study."}
                </p>
              </div>
            )}
            <p className="muted">
              #{current.session_id}
              {current.session_statement ? ` · ${current.session_statement}` : ""}
            </p>

            <h3>What the model saw</h3>
            {observed && (
              <p className="muted">
                {fmt(observed.active_seconds)} active, {fmt(observed.afk_seconds)} away
              </p>
            )}
            <div className="bars">
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
                  <div key={app} className="bar-group">
                    <div className="bar-row" title={`${app} — ${fmt(secs)}`}>
                      <span className="bar-label">{app}</span>
                      <span className="bar-track">
                        <span className="bar-fill" style={{ width: `${(secs / max) * 100}%` }} />
                      </span>
                      <span className="bar-value">{fmt(secs)}</span>
                    </div>
                    {rows.map((r) => (
                      <div
                        key={r.t}
                        className={`bar-row sub${r.unsent ? " unsent" : ""}`}
                        title={r.unsent ? `${r.t} — not sent to the model` : r.t}
                      >
                        <span className="bar-label">{r.t}</span>
                        <span className="bar-track">
                          <span className="bar-fill" style={{ width: `${(r.s / max) * 100}%` }} />
                        </span>
                        <span className="bar-value">{fmt(r.s)}</span>
                      </div>
                    ))}
                    {hidden.length > 0 && (
                      <button className="bar-more" onClick={() => toggle(app)}>
                        {open
                          ? "▾ hide the rest"
                          : `▸ +${hidden.length} more — recorded, not sent to the model`}
                      </button>
                    )}
                  </div>
                );
              })}
              {observed && apps.length === 0 && <p className="muted">nothing recorded</p>}
            </div>
          </>
        ) : (
          <p className="muted">
            No checks yet — they land at random intervals, and quiet windows are skipped.
          </p>
        )}

        <div className="run-row">
          <button onClick={runNow} disabled={running != null || !hasSession}>
            {running === "check" ? "checking…" : "Run a check now"}
          </button>
          <button onClick={runCheckpoint} disabled={running != null || !hasSession}>
            {running === "checkpoint" ? "checking…" : "Run a checkpoint now"}
          </button>
          {!hasSession && <span className="muted">no open session</span>}
          {runNote && <span className="muted">{runNote}</span>}
        </div>
      </section>
    </div>
  );
}
