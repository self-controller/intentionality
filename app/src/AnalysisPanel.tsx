import { useEffect, useState } from "react";
import * as api from "./api";
import type { Analysis } from "./types";

export default function AnalysisPanel({
  sessionId,
  onSeen,
  onClose,
}: {
  sessionId: number;
  onSeen: () => void;
  onClose: () => void;
}) {
  const [items, setItems] = useState<Analysis[]>([]);

  const refresh = () => api.listAnalyses(sessionId).then(setItems).catch(() => {});
  useEffect(() => {
    refresh();
    // Opening the panel is reading it: mark everything seen.
    api.listAnalyses(sessionId).then((list) => {
      Promise.all(list.filter((a) => !a.seen_at).map((a) => api.markAnalysisSeen(a.id))).then(onSeen);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  return (
    <aside className="analysis-panel">
      <header>
        <h2>Analyses</h2>
        <button onClick={onClose}>✕</button>
      </header>
      {items.length === 0 && <p className="muted">Nothing yet — checks land at random intervals.</p>}
      {items.map((a) => (
        <article key={a.id} className={a.seen_at ? "" : "unseen"}>
          <div className="headline">
            {a.headline}
            {a.alignment != null && <span className="alignment">{a.alignment}</span>}
          </div>
          <p>{a.body}</p>
          <time>{new Date(a.created_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time>
        </article>
      ))}
    </aside>
  );
}
