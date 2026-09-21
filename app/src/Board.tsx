import { useCallback, useEffect, useState } from "react";
import {
  DndContext,
  DragEndEvent,
  PointerSensor,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import { listen } from "@tauri-apps/api/event";
import * as api from "./api";
import { DUE_TEXT, dueInfo } from "./format";
import TaskEditor, { blankTask } from "./TaskEditor";
import type { Board as BoardData, LabelSummary, Status, Task } from "./types";

const LANES: { status: Status; label: string }[] = [
  { status: "planned", label: "To Do" },
  { status: "doing", label: "Doing" },
  { status: "done", label: "Done" },
];

function LabelChips({ task }: { task: Task }) {
  if (!task.labels.length) return null;
  return (
    <span className="mt-1.5 flex flex-wrap gap-1">
      {task.labels.map((l) => (
        <span
          key={l.name}
          /* The colour comes from the label row, so a tag looks the same on
             every card; the near-black text is what keeps it readable on all
             eight of them. */
          className="rounded-full px-1.5 text-[10px] font-semibold leading-relaxed text-bg"
          style={{ background: l.color }}
        >
          {l.name}
        </span>
      ))}
    </span>
  );
}

// A finished card keeps its date for the record but loses the alarm: work that
// got done late is done, not overdue.
function DueChip({ task }: { task: Task }) {
  if (!task.due_date) return null;
  const { text, cls, date } = dueInfo(task.due_date);
  const finished = task.status === "done" || task.status === "dropped";
  return (
    <span
      className={
        "ml-1.5 whitespace-nowrap text-[11px] " +
        (finished ? "text-muted" : DUE_TEXT[cls])
      }
      title={`Due ${task.due_date}`}
    >
      {finished ? `Due ${date}` : text}
    </span>
  );
}

function Card({ task, onOpen }: { task: Task; onOpen: (t: Task) => void }) {
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
      className={
        "relative mb-2 cursor-grab touch-none select-none rounded-lg border " +
        "border-line bg-raised px-2.5 py-2 transition-colors duration-150 " +
        "hover:border-muted/50 " +
        (isDragging ? "cursor-grabbing opacity-85" : "")
      }
      // The sensor's distance constraint is what keeps this from firing at the
      // end of a drag: under 4px of travel is a click, past it is a drag.
      onClick={() => onOpen(task)}
      {...listeners}
      {...attributes}
    >
      <span className="cursor-pointer">{task.title}</span>
      {task.notes.trim() && <span className="ml-1.5 inline-block h-[5px] w-[5px] rounded-full bg-muted align-middle" title="Has notes" />}
      {task.carry_count > 0 && <span className="ml-1.5 text-[11px] text-warn">{task.carry_count}×</span>}
      <DueChip task={task} />
      <LabelChips task={task} />
    </div>
  );
}

function Lane({
  status,
  label,
  tasks,
  onOpen,
}: {
  status: Status;
  label: string;
  tasks: Task[];
  onOpen: (t: Task) => void;
}) {
  const { setNodeRef, isOver } = useDroppable({ id: status });
  return (
    <div
      ref={setNodeRef}
      className={
        "min-h-[240px] flex-1 rounded-[10px] border bg-surface p-2.5 " +
        "transition-colors duration-150 " +
        (isOver ? "border-accent" : "border-line")
      }
    >
      <h2 className="mb-2.5 flex items-center justify-between text-[13px] uppercase tracking-[0.06em] text-muted">
        <span>{label}</span>
        <span>{tasks.length}</span>
      </h2>
      {tasks.map((t) => (
        <Card key={t.id} task={t} onOpen={onOpen} />
      ))}
    </div>
  );
}

/**
 * The fourth target. Dropping is a status, not a delete — a session card is
 * history — so the tray is a real drop zone and its contents are real cards:
 * what went in by drag comes back out the same way.
 */
function DroppedTray({
  tasks,
  open,
  setOpen,
  onOpen,
}: {
  tasks: Task[];
  open: boolean;
  setOpen: (v: boolean) => void;
  onOpen: (t: Task) => void;
}) {
  const { setNodeRef, isOver } = useDroppable({ id: "dropped" });
  // Opening it while a card hovers is what makes the target visible at the
  // moment you need to see it.
  const showing = open || isOver;
  return (
    <div
      ref={setNodeRef}
      className={
        "mt-3 rounded-[10px] border p-1 transition-colors duration-150 " +
        (isOver ? "border-bad" : "border-transparent")
      }
    >
      <button onClick={() => setOpen(!open)}>
        {showing ? "▾" : "▸"} dropped ({tasks.length})
      </button>
      {showing && (
        <div className="ml-2.5 mt-1.5 max-w-[420px] opacity-70 [&_span.cursor-pointer]:line-through">
          {tasks.map((t) => (
            <Card key={t.id} task={t} onOpen={onOpen} />
          ))}
          {!tasks.length && <span className="text-xs text-muted">drag a card here to drop it</span>}
        </div>
      )}
    </div>
  );
}

/**
 * The labels there are, worn or not. Presets are made here before anything
 * wears them; deleting one takes it off every card, so a label in use asks
 * first. `stamp` changes whenever the board reloads, which keeps the counts
 * current after an edit.
 */
function LabelsPanel({ stamp, onChanged }: { stamp: unknown; onChanged: () => void }) {
  const [open, setOpen] = useState(false);
  const [labels, setLabels] = useState<LabelSummary[]>([]);
  const [draft, setDraft] = useState("");
  const [confirming, setConfirming] = useState<LabelSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    api.listLabels().then(setLabels).catch((e) => setError(String(e)));
  }, []);
  useEffect(load, [load, stamp]);

  const create = () => {
    const name = draft.trim();
    if (!name) return;
    setError(null);
    api
      .createLabel(name)
      .then(() => {
        setDraft("");
        load();
      })
      .catch((e) => setError(String(e)));
  };

  const remove = (name: string) => {
    setError(null);
    api
      .deleteLabel(name)
      .then(() => {
        setConfirming(null);
        load();
        onChanged();
      })
      .catch((e) => setError(String(e)));
  };

  return (
    <section className="mt-4 border-t border-line pt-2.5">
      <button
        className="mb-2 text-[13px] uppercase tracking-[0.06em] text-muted transition-colors hover:text-text"
        onClick={() => setOpen(!open)}
      >
        {open ? "▾" : "▸"} Labels ({labels.length})
      </button>
      {open && (
        <>
          {labels.length ? (
            <ul className="mb-1 flex flex-wrap gap-1.5">
              {labels.map((l) => (
                <li key={l.name} className="flex max-w-full items-center gap-1.5 rounded-full border border-line bg-raised py-1 pl-2 pr-1 text-xs">
                  <span className="h-2 w-2 flex-none rounded-full" style={{ background: l.color }} />
                  <span className="max-w-[14ch] overflow-hidden text-ellipsis whitespace-nowrap">{l.name}</span>
                  <span className="text-[11px] text-muted" title={`On ${l.uses} task${l.uses === 1 ? "" : "s"}`}>
                    {l.uses}
                  </span>
                  <button
                    className="rounded-full px-1 text-muted transition-colors hover:text-bad"
                    title={`Delete ${l.name}`}
                    onClick={() => (l.uses ? setConfirming(l) : remove(l.name))}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-xs text-muted">No labels yet.</p>
          )}
          {confirming && (
            <div className="my-2 flex flex-wrap items-center gap-1.5">
              <span className="basis-full text-xs text-warn">
                “{confirming.name}” is on {confirming.uses} task{confirming.uses === 1 ? "" : "s"}.
                Deleting it takes it off {confirming.uses === 1 ? "that one" : "all of them"}.
              </span>
              <button onClick={() => remove(confirming.name)}>Delete</button>
              <button onClick={() => setConfirming(null)}>Keep it</button>
            </div>
          )}
          <input
            value={draft}
            placeholder="New label…"
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && create()}
          />
          {error && <p className="mt-2 text-xs text-bad">{error}</p>}
        </>
      )}
    </section>
  );
}

// A card about to be made from an add box, and which box it came from.
type Creating = { toBacklog: boolean; title: string };

export default function Board() {
  const [board, setBoard] = useState<BoardData | null>(null);
  const [newTitle, setNewTitle] = useState("");
  const [backlogTitle, setBacklogTitle] = useState("");
  const [showDropped, setShowDropped] = useState(false);
  const [editing, setEditing] = useState<Task | null>(null);
  const [creating, setCreating] = useState<Creating | null>(null);
  const [err, setErr] = useState<string | null>(null);
  // A write the backend refused, shown above the board it reloaded — not the
  // fatal screen, which hid the board until the tab was remounted.
  const [notice, setNotice] = useState<string | null>(null);

  // Without a travel threshold the whole card is a drag handle and a click on
  // it never lands, so the editor could never be opened.
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }));

  const refresh = useCallback(() => {
    api.getBoard().then(setBoard).catch((e) => setErr(String(e)));
  }, []);
  // The heartbeat loop lets go of a session closed elsewhere and adopts the
  // one a resume gate starts; either way the board on screen is the old one.
  useEffect(() => {
    refresh();
    const unlistenOpened = listen("session:opened", refresh);
    const unlistenClosed = listen("session:closed", refresh);
    return () => {
      unlistenOpened.then((f) => f());
      unlistenClosed.then((f) => f());
    };
  }, [refresh]);

  // Usually the gate closed the session this board belongs to; reloading
  // brings up the one that replaced it.
  const fail = (e: unknown) => {
    setNotice(String(e));
    refresh();
  };

  if (err) return <div className="p-12 text-center text-muted">{err}</div>;
  if (!board) return null;

  const byStatus = (s: Status) => board.tasks.filter((t) => t.status === s);
  const dropped = byStatus("dropped");

  // One write for the whole board, whether the move came from a drag or from
  // the editor's Drop button.
  const move = (taskId: number, target: Status) => {
    const task = board.tasks.find((t) => t.id === taskId);
    if (!task || task.status === target) return;
    setNotice(null);
    // Optimistic move, then the whole arrangement is written atomically.
    const next = board.tasks.map((t) => (t.id === taskId ? { ...t, status: target } : t));
    setBoard({ ...board, tasks: next });
    const ids = (s: Status) => next.filter((t) => t.status === s).map((t) => t.id);
    api
      .applyBoard({ todo: ids("planned"), doing: ids("doing"), done: ids("done"), dropped: ids("dropped") })
      .then(refresh)
      .catch(fail);
  };

  const onDragEnd = (ev: DragEndEvent) => {
    const target = ev.over?.id as Status | undefined;
    if (!target || !board.session) return;
    move(ev.active.id as number, target);
  };

  const add = (toBacklog: boolean) => {
    const title = (toBacklog ? backlogTitle : newTitle).trim();
    if (!title) return;
    setNotice(null);
    api
      .addTask(title, toBacklog)
      .then(() => {
        toBacklog ? setBacklogTitle("") : setNewTitle("");
        refresh();
      })
      .catch(fail);
  };

  return (
    <div className="flex items-start gap-4 p-4">
      <div className="min-w-0 flex-1">
        {notice && <p className="mb-3 text-[13px] text-warn">{notice}</p>}
        {board.session ? (
          <>
            <div className="mb-3 flex items-center gap-2.5">
              {/* Sessions committed since the gate stopped asking have no
                  statement; the lanes below are the intention. */}
              {board.session.statement && <strong>{board.session.statement}</strong>}
              {board.session.intended_minutes != null && (
                <span className="text-muted">
                  {board.session.statement ? " · " : ""}
                  intended {board.session.intended_minutes} min
                </span>
              )}
            </div>
            <DndContext sensors={sensors} onDragEnd={onDragEnd}>
              <div className="flex gap-3">
                {LANES.map(({ status, label }) => (
                  <Lane
                    key={status}
                    status={status}
                    label={label}
                    tasks={byStatus(status)}
                    onOpen={setEditing}
                  />
                ))}
              </div>
              {/* Enter adds the title alone; Details… opens the editor so a
                  new card can start with its date, notes and labels. */}
              <div className="mt-3 flex max-w-[420px] gap-2">
                <input
                  value={newTitle}
                  placeholder="Add a task to this session…"
                  onChange={(e) => setNewTitle(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && add(false)}
                />
                <button onClick={() => setCreating({ toBacklog: false, title: newTitle })}>
                  Details…
                </button>
              </div>
              <DroppedTray
                tasks={dropped}
                open={showDropped}
                setOpen={setShowDropped}
                onOpen={setEditing}
              />
            </DndContext>
          </>
        ) : (
          <p className="text-muted">
            No open session — start one at the gate. The backlog and dashboard still work.
          </p>
        )}
      </div>

      <aside className="w-60 flex-none rounded-[10px] border border-line bg-surface p-2.5">
        <h2 className="mb-2.5 text-[13px] uppercase tracking-[0.06em] text-muted">Backlog</h2>
        {board.backlog.map((t) => (
          <div
            key={t.id}
            className="flex cursor-pointer items-center justify-between gap-1.5 py-[5px] text-muted transition-colors hover:text-text"
            onClick={() => setEditing(t)}
          >
            <span className="min-w-0 flex-1">
              {t.title}
              {t.notes.trim() && <span className="ml-1.5 inline-block h-[5px] w-[5px] rounded-full bg-muted align-middle" title="Has notes" />}
              {t.carry_count > 0 && <span className="ml-1.5 text-[11px] text-warn">{t.carry_count}×</span>}
              <DueChip task={t} />
              <LabelChips task={t} />
            </span>
            {/* The row opens the editor, so its buttons must not. */}
            <span className="flex flex-none gap-0.5" onClick={(e) => e.stopPropagation()}>
              {board.session && (
                <button
                  className="rounded border-none px-1.5 py-0.5 text-muted transition-colors hover:text-text"
                  title="Pull into this session"
                  onClick={() => {
                    setNotice(null);
                    api.pullTask(t.id).then(refresh).catch(fail);
                  }}
                >
                  ←
                </button>
              )}
              <button
                className="rounded border-none px-1.5 py-0.5 text-muted transition-colors hover:text-bad"
                title="Delete"
                onClick={() => api.deleteTask(t.id).then(refresh)}
              >
                ✕
              </button>
            </span>
          </div>
        ))}
        <div className="mt-2 flex items-center gap-1.5 [&_input]:min-w-0 [&_input]:flex-1 [&_button]:flex-none">
          <input
            value={backlogTitle}
            placeholder="Add to backlog…"
            onChange={(e) => setBacklogTitle(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && add(true)}
          />
          <button onClick={() => setCreating({ toBacklog: true, title: backlogTitle })}>
            Details…
          </button>
        </div>
        <LabelsPanel stamp={board} onChanged={refresh} />
      </aside>

      {creating && (
        <TaskEditor
          initial={blankTask(creating.title.trim())}
          heading={creating.toBacklog ? "New backlog task" : "New task for this session"}
          saveText="Add task"
          save={(title, notes, dueDate, labels) =>
            api.createTask(title, notes, dueDate, labels, creating.toBacklog)
          }
          onClose={() => setCreating(null)}
          onSaved={() => {
            // The box it came from was the draft; the card is made now.
            creating.toBacklog ? setBacklogTitle("") : setNewTitle("");
            setCreating(null);
            setNotice(null);
            refresh();
          }}
          onDrop={null}
          onDelete={null}
        />
      )}

      {editing && (
        <TaskEditor
          key={editing.id}
          initial={editing}
          saveText="Save"
          save={(title, notes, dueDate, labels) =>
            api.updateTask(editing.id, title, notes, dueDate, labels)
          }
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
          // A card on the board gets dropped; a backlog row gets deleted.
          onDrop={
            editing.session_id != null && editing.status !== "dropped"
              ? () => {
                  move(editing.id, "dropped");
                  setEditing(null);
                }
              : null
          }
          onDelete={
            editing.session_id == null
              ? () => {
                  api.deleteTask(editing.id).then(() => {
                    setEditing(null);
                    refresh();
                  });
                }
              : null
          }
        />
      )}
    </div>
  );
}
