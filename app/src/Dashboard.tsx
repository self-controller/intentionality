import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Circle, CircleCheck, CircleDot, CircleX, type LucideIcon } from "lucide-react";
import { BarGroup, BarMore, BarRow, Bars } from "@/components/Bars";
import ListRow from "@/components/ListRow";
import * as api from "./api";
import { fmt, hhmm, rankTitles, sessionLabel } from "./format";
import type { Observed, Session, Task } from "./types";

// The icon carries the meaning; colour reinforces it. Literal utility
// strings, so Tailwind's scanner sees them: the status is chosen at runtime.
const MARK: Record<string, { icon: LucideIcon; cls: string; label: string }> = {
  done: { icon: CircleCheck, cls: "text-good", label: "done" },
  dropped: { icon: CircleX, cls: "text-destructive", label: "dropped" },
  planned: { icon: Circle, cls: "text-muted-foreground", label: "not started" },
  doing: { icon: CircleDot, cls: "text-warn", label: "in progress" },
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
    <div className="flex items-start gap-8 px-6 py-6">
      <aside className="flex max-h-[calc(100vh-104px)] w-[280px] flex-none flex-col gap-0.5 overflow-y-auto">
        <h3 className="mb-1.5 px-3">Recent sessions</h3>
        {sessions.map((s) => (
          <ListRow
            key={s.id}
            selected={s.id === selected}
            className="flex items-center gap-2"
            onClick={() => setSelected(s.id)}
          >
            <span className="flex-none font-mono text-[11px] text-muted-foreground">#{s.id}</span>
            <span className="min-w-0 flex-1 truncate">{sessionLabel(s)}</span>
            <span
              className={
                "flex-none font-mono text-[11px] " + (s.close_reason ? "text-muted-foreground" : "text-good")
              }
            >
              {s.close_reason ?? "open"}
            </span>
          </ListRow>
        ))}
      </aside>
      <section className="min-w-0 max-w-[760px] flex-1">
        {session && (
          <>
            <p className="eyebrow">
              Session #{session.id} · {session.ended_at == null ? "not closed" : session.close_reason ?? "closed"}
            </p>
            <h2 className="mt-2">{sessionLabel(session)}</h2>
            <p className="mt-1.5 font-mono text-xs text-muted-foreground">
              {new Date(session.started_at).toLocaleDateString()} {hhmm(session.started_at)}
              {session.ended_at && `–${hhmm(session.ended_at)}`} ·{" "}
              {session.intended_minutes != null ? `intended ${session.intended_minutes} min` : "open-ended"}
            </p>
            <ul className="surface mt-6 list-none divide-y divide-border p-0">
              {tasks.map((t) => {
                const mark = MARK[t.status];
                const Icon = mark.icon;
                return (
                  <li key={t.id} className="flex items-center gap-3 px-4 py-2.5">
                    <Icon className={"size-4 flex-none " + mark.cls} aria-label={mark.label} />
                    <span className={t.status === "dropped" ? "text-muted-foreground line-through" : ""}>
                      {t.title}
                    </span>
                  </li>
                );
              })}
              {tasks.length === 0 && <li className="px-4 py-2.5 text-muted-foreground">No tasks.</li>}
            </ul>
            <div className="surface mt-6 px-5 py-4">
              <div className="mb-4 flex items-baseline justify-between gap-4">
                <h3>Observed</h3>
                {observed && (
                  <p className="font-mono text-xs text-muted-foreground">
                    {fmt(observed.active_seconds)} active · {fmt(observed.afk_seconds)} away
                  </p>
                )}
              </div>
              {awError && <p className="text-muted-foreground">unavailable — {awError}</p>}
              {observed && (
                <>
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
            </div>
          </>
        )}
      </section>
    </div>
  );
}
