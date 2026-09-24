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
import type { Board as BoardData, Status, Task } from "./types";

// The droppable ids: a session status, or the backlog, which is no status at
// all (session_id NULL). Dropped cards are history and aren't shown.
type Lane = "planned" | "doing" | "done" | "backlog";

const LANES: { lane: Lane; label: string }[] = [
  { lane: "planned", label: "To Do" },
  { lane: "doing", label: "Doing" },
  { lane: "done", label: "Done" },
  { lane: "backlog", label: "Backlog" },
];

const laneLabel = (lane: Lane) => LANES.find((l) => l.lane === lane)!.label;

function LabelChips({ task }: { task: Task }) {
  if (!task.labels.length) return null;
  return (
    <span className="mt-2 flex flex-wrap gap-1">
      {task.labels.map((l) => (
        <span
          key={l.name}
          /* The colour comes from the label row, so a tag looks the same on
             every card; the near-black text is what keeps it readable on all
             eight of them. */
          className="rounded-sm px-1.5 py-px text-[11px] font-semibold leading-relaxed text-surface"
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
      className={"mt-1.5 block text-[12px] " + (finished ? "text-muted" : DUE_TEXT[cls])}
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
        "relative mb-2 cursor-pointer touch-none select-none rounded-lg bg-raised/45 " +
        "px-3 py-2.5 shadow-sm transition-colors duration-150 hover:bg-raised/70 " +
        (isDragging ? "cursor-grabbing bg-raised/80 shadow-lg" : "")
      }
      // The sensor's distance constraint is what keeps this from firing at the
      // end of a drag: under 4px of travel is a click, past it is a drag.
      onClick={() => onOpen(task)}
      {...listeners}
      {...attributes}
    >
      <div className="font-medium leading-snug">
        {task.title}
      </div>
      {task.notes.trim() && (
        <p className="mt-1 line-clamp-2 whitespace-pre-line text-[13px] leading-snug text-muted">{task.notes}</p>
      )}
      <DueChip task={task} />
      <LabelChips task={task} />
    </div>
  );
}

function Column({
  lane,
  label,
  tasks,
  first,
  onAdd,
  onOpen,
  empty,
}: {
  lane: Lane;
  label: string;
  tasks: Task[];
  first: boolean;
  // Null where nothing can be added: a session column with no open session.
  onAdd: (() => void) | null;
  onOpen: (t: Task) => void;
  empty?: string;
}) {
  const { setNodeRef, isOver } = useDroppable({ id: lane });
  return (
    <div
      ref={setNodeRef}
      className={
        "min-h-[240px] min-w-0 flex-1 px-3 pb-3 transition-colors duration-150 " +
        (first ? "" : "border-l border-line/70 ") +
        (isOver ? "bg-raised/15" : "")
      }
    >
      <h2 className="mb-3 flex h-7 items-center gap-2 text-[14px] font-medium">
        <span>{label}</span>
        <span className="text-muted">{tasks.length}</span>
        {onAdd && (
          <button
            className="ml-auto rounded border-none px-1.5 text-lg leading-none text-muted transition-colors hover:text-text"
            title={`New task in ${label}`}
            onClick={onAdd}
          >
            +
          </button>
        )}
      </h2>
      {tasks.map((t) => (
        <Card key={t.id} task={t} onOpen={onOpen} />
      ))}
      {empty && !tasks.length && <p className="text-xs text-muted">{empty}</p>}
    </div>
  );
}

// The whole arrangement apply_board wants. Dropped cards aren't shown but
// are still on the board, and pass through untouched.
function arrangement(tasks: Task[]) {
  const ids = (s: Status) => tasks.filter((t) => t.status === s).map((t) => t.id);
  return { todo: ids("planned"), doing: ids("doing"), done: ids("done"), dropped: ids("dropped") };
}

// A card that is already on the board (just pulled, or just made), put in its
// lane from a fresh read: the frontend's copy doesn't have it yet.
function place(id: number, status: Status) {
  return api.getBoard().then((b) =>
    api.applyBoard(arrangement(b.tasks.map((t) => (t.id === id ? { ...t, status } : t)))),
  );
}

export default function Board() {
  const [board, setBoard] = useState<BoardData | null>(null);
  const [editing, setEditing] = useState<Task | null>(null);
  // The column whose + opened the editor for a card that doesn't exist yet.
  const [creating, setCreating] = useState<Lane | null>(null);
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

  const session = board.session;
  const inLane = (lane: Lane) =>
    lane === "backlog" ? board.backlog : board.tasks.filter((t) => t.status === lane);

  // Every move, whichever columns it crosses: optimistic first, then one
  // write (two for a pull into Doing or Done), then a reload.
  const move = (taskId: number, target: Lane) => {
    const onBoard = board.tasks.find((t) => t.id === taskId);
    const inBacklog = board.backlog.find((t) => t.id === taskId);
    setNotice(null);
    if (onBoard) {
      if (onBoard.status === target) return;
      if (target === "backlog") {
        setBoard({
          ...board,
          tasks: board.tasks.filter((t) => t.id !== taskId),
          backlog: [...board.backlog, { ...onBoard, session_id: null, status: "planned" }],
        });
        api.unpullTask(taskId).then(refresh).catch(fail);
        return;
      }
      const next = board.tasks.map((t) => (t.id === taskId ? { ...t, status: target } : t));
      setBoard({ ...board, tasks: next });
      api.applyBoard(arrangement(next)).then(refresh).catch(fail);
    } else if (inBacklog && session && target !== "backlog") {
      setBoard({
        ...board,
        backlog: board.backlog.filter((t) => t.id !== taskId),
        tasks: [...board.tasks, { ...inBacklog, session_id: session.id, status: target }],
      });
      // A pull lands in To Do; anywhere else is a second move.
      api
        .pullTask(taskId)
        .then(() => (target === "planned" ? undefined : place(taskId, target)))
        .then(refresh)
        .catch(fail);
    }
  };

  const onDragEnd = (ev: DragEndEvent) => {
    const target = ev.over?.id as Lane | undefined;
    if (target) move(ev.active.id as number, target);
  };

  return (
    <div className="p-4">
      {notice && <p className="mb-3 text-[13px] text-warn">{notice}</p>}
      {session && (session.statement || session.intended_minutes != null) && (
        <div className="mb-3 flex items-center gap-2.5">
          {/* Sessions committed since the gate stopped asking have no
              statement; the columns below are the intention. */}
          {session.statement && <strong>{session.statement}</strong>}
          {session.intended_minutes != null && (
            <span className="text-muted">
              {session.statement ? " · " : ""}
              intended {session.intended_minutes} min
            </span>
          )}
        </div>
      )}
      <DndContext sensors={sensors} onDragEnd={onDragEnd}>
        <div className="flex">
          {LANES.map(({ lane, label }, i) => {
            const open = lane === "backlog" || session != null;
            return (
              <Column
                key={lane}
                lane={lane}
                label={label}
                tasks={inLane(lane)}
                first={i === 0}
                onAdd={open ? () => setCreating(lane) : null}
                onOpen={setEditing}
                empty={open ? undefined : "No open session — start one at the gate."}
              />
            );
          })}
        </div>
      </DndContext>

      {creating && (
        <TaskEditor
          initial={blankTask("")}
          heading={`New task in ${laneLabel(creating)}`}
          saveText="Add task"
          save={(title, notes, dueDate, labels) =>
            api
              .createTask(title, notes, dueDate, labels, creating === "backlog")
              // A new session card starts in To Do.
              .then((id) =>
                creating === "backlog" || creating === "planned" ? undefined : place(id, creating),
              )
          }
          onClose={() => setCreating(null)}
          onSaved={() => {
            setCreating(null);
            setNotice(null);
            refresh();
          }}
          onDelete={null}
          onLabelsChanged={refresh}
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
          onLabelsChanged={refresh}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
          // A backlog row can be deleted; a session card is history.
          onDelete={
            editing.session_id == null
              ? () => {
                  api
                    .deleteTask(editing.id)
                    .then(() => {
                      setEditing(null);
                      refresh();
                    })
                    .catch(fail);
                }
              : null
          }
        />
      )}
    </div>
  );
}
