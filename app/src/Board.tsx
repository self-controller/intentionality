import { useCallback, useEffect, useState } from "react";
import { DndContext, DragEndEvent, useDraggable, useDroppable } from "@dnd-kit/core";
import * as api from "./api";
import type { Board as BoardData, Status, Task } from "./types";
import AnalysisPanel from "./AnalysisPanel";

const LANES: { status: Status; label: string }[] = [
  { status: "planned", label: "To Do" },
  { status: "doing", label: "Doing" },
  { status: "done", label: "Done" },
];

function Card({ task }: { task: Task }) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({
    id: task.id,
  });
  const style = transform
    ? { transform: `translate(${transform.x}px, ${transform.y}px)`, zIndex: 10 }
    : undefined;
  return (
    <div
      ref={setNodeRef}
      style={style}
      className={`card${isDragging ? " dragging" : ""}`}
      {...listeners}
      {...attributes}
    >
      {task.title}
      {task.carry_count > 0 && <span className="carry">{task.carry_count}×</span>}
    </div>
  );
}

function Lane({ status, label, tasks }: { status: Status; label: string; tasks: Task[] }) {
  const { setNodeRef, isOver } = useDroppable({ id: status });
  return (
    <div ref={setNodeRef} className={`lane${isOver ? " over" : ""}`}>
      <h2>
        {label} <span className="count">{tasks.length}</span>
      </h2>
      {tasks.map((t) => (
        <Card key={t.id} task={t} />
      ))}
    </div>
  );
}

export default function Board({ onSeen }: { onSeen: () => void }) {
  const [board, setBoard] = useState<BoardData | null>(null);
  const [newTitle, setNewTitle] = useState("");
  const [backlogTitle, setBacklogTitle] = useState("");
  const [showDropped, setShowDropped] = useState(false);
  const [showPanel, setShowPanel] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.getBoard().then(setBoard).catch((e) => setErr(String(e)));
  }, []);
  useEffect(refresh, [refresh]);

  if (err) return <div className="fatal">{err}</div>;
  if (!board) return null;

  const byStatus = (s: Status) => board.tasks.filter((t) => t.status === s);
  const dropped = byStatus("dropped");

  const onDragEnd = (ev: DragEndEvent) => {
    const target = ev.over?.id as Status | undefined;
    const taskId = ev.active.id as number;
    if (!target || !board.session) return;
    const task = board.tasks.find((t) => t.id === taskId);
    if (!task || task.status === target) return;
    // Optimistic move, then the whole arrangement is written atomically.
    const next = board.tasks.map((t) => (t.id === taskId ? { ...t, status: target } : t));
    setBoard({ ...board, tasks: next });
    const ids = (s: Status) => next.filter((t) => t.status === s).map((t) => t.id);
    api
      .applyBoard({ todo: ids("planned"), doing: ids("doing"), done: ids("done"), dropped: ids("dropped") })
      .then(refresh)
      .catch((e) => {
        setErr(String(e));
        refresh();
      });
  };

  const add = (toBacklog: boolean) => {
    const title = (toBacklog ? backlogTitle : newTitle).trim();
    if (!title) return;
    api.addTask(title, toBacklog).then(() => {
      toBacklog ? setBacklogTitle("") : setNewTitle("");
      refresh();
    });
  };

  return (
    <div className="board-screen">
      <div className="board-main">
        {board.session ? (
          <>
            <div className="session-line">
              <strong>{board.session.statement}</strong>
              {board.session.intended_minutes != null && (
                <span className="muted"> · intended {board.session.intended_minutes} min</span>
              )}
              <button className="panel-toggle" onClick={() => setShowPanel((v) => !v)}>
                analyses{board.unseen > 0 ? ` (${board.unseen})` : ""}
              </button>
            </div>
            <DndContext onDragEnd={onDragEnd}>
              <div className="lanes">
                {LANES.map(({ status, label }) => (
                  <Lane key={status} status={status} label={label} tasks={byStatus(status)} />
                ))}
              </div>
            </DndContext>
            <div className="add-row">
              <input
                value={newTitle}
                placeholder="Add a task to this session…"
                onChange={(e) => setNewTitle(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && add(false)}
              />
            </div>
            <div className="dropped-tray">
              <button onClick={() => setShowDropped((v) => !v)}>
                {showDropped ? "▾" : "▸"} dropped ({dropped.length})
              </button>
              {showDropped &&
                dropped.map((t) => (
                  <span key={t.id} className="dropped-item">
                    {t.title}
                  </span>
                ))}
            </div>
          </>
        ) : (
          <p className="muted">
            No open session — start one at the gate. The backlog and dashboard still work.
          </p>
        )}
      </div>

      <aside className="backlog">
        <h2>Backlog</h2>
        {board.backlog.map((t) => (
          <div key={t.id} className="backlog-item">
            <span>
              {t.title}
              {t.carry_count > 0 && <span className="carry">{t.carry_count}×</span>}
            </span>
            <span className="backlog-actions">
              {board.session && (
                <button title="Pull into this session" onClick={() => api.pullTask(t.id).then(refresh)}>
                  ←
                </button>
              )}
              <button title="Delete" onClick={() => api.deleteTask(t.id).then(refresh)}>
                ✕
              </button>
            </span>
          </div>
        ))}
        <input
          value={backlogTitle}
          placeholder="Add to backlog…"
          onChange={(e) => setBacklogTitle(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && add(true)}
        />
      </aside>

      {showPanel && board.session && (
        <AnalysisPanel sessionId={board.session.id} onSeen={onSeen} onClose={() => setShowPanel(false)} />
      )}
    </div>
  );
}
