import { useEffect, useState } from "react";
import * as api from "./api";
import type { Observed, Session, Task } from "./types";

const MARK: Record<string, string> = { done: "✓", dropped: "✗", planned: "·", doing: "▸" };

function fmt(seconds: number): string {
  const m = Math.floor(seconds / 60);
  if (m >= 60) return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
  if (m > 0) return `${m}m`;
  return `${Math.floor(seconds)}s`;
}

export default function Dashboard() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [observed, setObserved] = useState<Observed | null>(null);
  const [awError, setAwError] = useState<string | null>(null);

  useEffect(() => {
    api.listSessions(15).then((list) => {
      setSessions(list);
      if (list.length > 0) setSelected(list[0].id);
    });
  }, []);

  useEffect(() => {
    if (selected == null) return;
    setObserved(null);
    setAwError(null);
    api.getSessionTasks(selected).then(setTasks);
    api.getObserved(selected).then(setObserved).catch((e) => setAwError(String(e)));
  }, [selected]);

  const session = sessions.find((s) => s.id === selected);
  const apps = observed
    ? Object.entries(observed.per_app).sort((a, b) => b[1] - a[1]).slice(0, 8)
    : [];
  const max = apps.length > 0 ? apps[0][1] : 1;

  return (
    <div className="dashboard">
      <aside className="session-list">
        {sessions.map((s) => (
          <button
            key={s.id}
            className={s.id === selected ? "active" : ""}
            onClick={() => setSelected(s.id)}
          >
            <span className="muted">#{s.id}</span> {s.statement || "(no statement)"}
            <span className="state">{s.close_reason ?? "open"}</span>
          </button>
        ))}
      </aside>
      <section className="session-detail">
        {session && (
          <>
            <h2>{session.statement}</h2>
            <p className="muted">
              {new Date(session.started_at).toLocaleString()} ·{" "}
              {session.intended_minutes != null ? `intended ${session.intended_minutes} min` : "open-ended"}
              {session.ended_at == null && " · not closed"}
            </p>
            <ul className="task-list">
              {tasks.map((t) => (
                <li key={t.id}>
                  <span className={`mark ${t.status}`}>{MARK[t.status]}</span> {t.title}
                </li>
              ))}
            </ul>
            <h3>Observed</h3>
            {awError && <p className="muted">unavailable — {awError}</p>}
            {observed && (
              <>
                <p className="muted">
                  {fmt(observed.active_seconds)} active, {fmt(observed.afk_seconds)} away
                </p>
                <div className="bars">
                  {apps.map(([app, secs]) => (
                    <div key={app} className="bar-row">
                      <span className="bar-label">{app}</span>
                      <span className="bar-track">
                        <span className="bar-fill" style={{ width: `${(secs / max) * 100}%` }} />
                      </span>
                      <span className="bar-value">{fmt(secs)}</span>
                    </div>
                  ))}
                </div>
              </>
            )}
          </>
        )}
      </section>
    </div>
  );
}
