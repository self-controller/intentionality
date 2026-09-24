import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import { fmt, rankTitles, sessionLabel } from "./format";
import { BarGroup, BarMore, BarRow, Bars } from "./ui/Bars";
import type { Observed, Session, Task } from "./types";

const MARK: Record<string, string> = { done: "✓", dropped: "✗", planned: "·", doing: "▸" };
// Literal utility strings, so Tailwind's scanner sees them: the status is
// chosen at runtime. The glyph carries the meaning; colour reinforces it.
const MARK_TEXT: Record<string, string> = {
  done: "text-good",
  dropped: "text-bad",
  planned: "text-muted",
  doing: "text-accent",
};

const TOP_TITLES = 3;

export default function Dashboard() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [observed, setObserved] = useState<Observed | null>(null);
  const [awError, setAwError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  // Fetched again when a session opens or closes, so one the resume gate
  // starts appears without leaving the tab. The selection stays put once there
  // is one: a session opening must not yank the pane out from under a read.
  const loadSessions = useCallback(() => {
    api
      .listSessions(15)
      .then((list) => {
        setSessions(list);
        if (list.length > 0) setSelected((cur) => cur ?? list[0].id);
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    loadSessions();
    const unlistenOpened = listen("session:opened", loadSessions);
    const unlistenClosed = listen("session:closed", loadSessions);
    return () => {
      unlistenOpened.then((f) => f());
      unlistenClosed.then((f) => f());
    };
  }, [loadSessions]);

  useEffect(() => {
    if (selected == null) return;
    setObserved(null);
    setAwError(null);
    setExpanded(new Set());
    // Clicking through sessions quickly leaves earlier answers in flight;
    // only the current selection's may land.
    let current = true;
    api
      .getSessionTasks(selected)
      .then((list) => current && setTasks(list))
      .catch(() => current && setTasks([]));
    api
      .getObserved(selected)
      .then((o) => current && setObserved(o))
      .catch((e) => current && setAwError(String(e)));
    return () => {
      current = false;
    };
  }, [selected]);

  const session = sessions.find((s) => s.id === selected);
  const apps = observed
    ? Object.entries(observed.per_app).sort((a, b) => b[1] - a[1]).slice(0, 8)
    : [];
  const max = apps.length > 0 ? apps[0][1] : 1;

  const titlesFor = (app: string) => rankTitles(observed, app);

  const toggle = (app: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (!next.delete(app)) next.add(app);
      return next;
    });

  return (
    <div className="flex items-start gap-4 p-4">
      <aside className="flex w-[280px] flex-none flex-col gap-1.5">
        {sessions.map((s) => (
          <button
            key={s.id}
            className={
              "block w-full overflow-hidden text-ellipsis whitespace-nowrap rounded-md " +
              "border px-2.5 py-1.5 text-left transition-colors duration-150 " +
              (s.id === selected
                ? "border-accent bg-accent text-bg"
                : "border-line text-text hover:border-muted")
            }
            onClick={() => setSelected(s.id)}
          >
            <span className="opacity-60">#{s.id}</span> {sessionLabel(s)}
            <span className="float-right text-xs opacity-60">{s.close_reason ?? "open"}</span>
          </button>
        ))}
      </aside>
      <section className="min-w-0 flex-1">
        {session && (
          <>
            <h2>{sessionLabel(session)}</h2>
            <p className="text-muted">
              {new Date(session.started_at).toLocaleString()} ·{" "}
              {session.intended_minutes != null ? `intended ${session.intended_minutes} min` : "open-ended"}
              {session.ended_at == null && " · not closed"}
            </p>
            <ul className="list-none p-0">
              {tasks.map((t) => (
                <li key={t.id} className="py-0.5">
                  <span className={MARK_TEXT[t.status]}>{MARK[t.status]}</span> {t.title}
                </li>
              ))}
            </ul>
            <h3>Observed</h3>
            {awError && <p className="text-muted">unavailable — {awError}</p>}
            {observed && (
              <>
                <p className="text-muted">
                  {fmt(observed.active_seconds)} active, {fmt(observed.afk_seconds)} away
                </p>
                <Bars>
                  {apps.map(([app, secs]) => {
                    const titles = titlesFor(app);
                    const open = expanded.has(app);
                    const shown = open ? titles : titles.slice(0, TOP_TITLES);
                    return (
                      <BarGroup key={app}>
                        <BarRow label={app} value={fmt(secs)} pct={(secs / max) * 100} />
                        {shown.map(([t, tsecs]) => (
                          <BarRow key={t} label={t} value={fmt(tsecs)} pct={(tsecs / max) * 100} sub />
                        ))}
                        {titles.length > TOP_TITLES && (
                          <BarMore onClick={() => toggle(app)}>
                            {open ? "▾ show less" : `▸ +${titles.length - TOP_TITLES} more`}
                          </BarMore>
                        )}
                      </BarGroup>
                    );
                  })}
                </Bars>
              </>
            )}
          </>
        )}
      </section>
    </div>
  );
}
