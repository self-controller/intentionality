import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
// Types only: the runtime import is the lazy one below, so three.js stays out
// of the main chunk and loads with the Board tab.
import type { CardMoveEvent, KanbanCardData, KanbanColumnData, KanbanTheme } from "threejs-elements/kanban";
import type { KanbanBoard3DHandle } from "threejs-elements/react";
import * as api from "./api";
import CardEditor3D from "./CardEditor3D";
import { TaskForm, blankTask } from "./TaskEditor";
import type { Board as BoardData, Status, Task } from "./types";

const KanbanBoard3D = lazy(() =>
  import("threejs-elements/react").then((m) => ({ default: m.KanbanBoard3D })),
);

// The column ids: a session status, or the backlog, which is no status at
// all (session_id NULL). Dropped cards are history and aren't shown.
type Lane = "planned" | "doing" | "done" | "backlog";

const LANES: { lane: Lane; label: string; token: string }[] = [
  { lane: "planned", label: "To Do", token: "muted" },
  { lane: "doing", label: "Doing", token: "warn" },
  { lane: "done", label: "Done", token: "good" },
  { lane: "backlog", label: "Backlog", token: "violet" },
];

const laneLabel = (lane: Lane) => LANES.find((l) => l.lane === lane)!.label;

// Read on mount, like the theme; a module constant so it is never a new object.
// rows is the visible height of a column, in default-sized cards; a column
// with more than fits scrolls instead of stretching the board. Cards size to
// their text, so a short title takes about half a row.
const LAYOUT = { headerDepth: 0.52, rows: 4 };

const FILL = { position: "absolute", inset: 0 } as const;

// A few sparks, a thump and one small hop on a slam, but no flash: it lit a
// yellow spot in the middle of the card's face. The card flies up critically
// damped (2√260 ≈ 32), so it arrives without overshooting. A constant: the
// board re-applies a new object.
const MOTION = {
  slamFlash: 0,
  sparkCount: 14,
  slamBounce: 0.12,
  focusSpring: { stiffness: 260, damping: 34 },
};

const NO_SESSION = "No open session — start one at the gate.";

// theme.css is the one source of the palette; these are its values, used only
// if Tailwind didn't emit a variable (it drops tokens nothing references).
const FALLBACK: Record<string, string> = {
  bg: "#343a40",
  surface: "#212529",
  raised: "#495057",
  text: "#f8f9fa",
  muted: "#adb5bd",
  warn: "#e0af68",
  good: "#9ece6a",
  violet: "#bb9af7",
};

function token(name: string) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(`--color-${name}`).trim();
  return v || FALLBACK[name];
}

// The canvas draws its own text, so it is handed the page's font stack (theme.css
// sets none: this is Tailwind's default sans) and the page's greys.
function boardTheme(): Partial<KanbanTheme> {
  return {
    fontFamily: getComputedStyle(document.body).fontFamily,
    panel: token("surface"),
    headerText: token("text"),
    headerBadge: "rgba(255,255,255,0.1)",
    headerBadgeText: token("muted"),
    cardFace: token("raised"),
    cardText: token("text"),
    cardMuted: token("muted"),
    cardEdge: token("bg"),
    highlight: 0.035,
  };
}

// One tag fits on a card face; the rest are counted, and all of them are in
// the editor a click away.
function toCard(t: Task, muted: string): KanbanCardData {
  const [first, ...rest] = t.labels;
  return {
    id: String(t.id),
    title: t.title,
    tag: first ? first.name + (rest.length ? ` +${rest.length}` : "") : undefined,
    color: first?.color ?? muted,
  };
}

// The session's cards per status, in board order. Dropped is carried along
// because apply_board wants the whole arrangement.
type Lanes = Record<Status, Task[]>;

function lanesOf(tasks: Task[]): Lanes {
  const by = (s: Status) => tasks.filter((t) => t.status === s);
  return { planned: by("planned"), doing: by("doing"), done: by("done"), dropped: by("dropped") };
}

// `task` taken out of wherever it was and put into `target` at `index`.
function moved(tasks: Task[], task: Task, target: Status, index: number): Task[] {
  const lanes = lanesOf(tasks.filter((t) => t.id !== task.id));
  lanes[target].splice(index, 0, { ...task, status: target });
  return [...lanes.planned, ...lanes.doing, ...lanes.done, ...lanes.dropped];
}

function arrangement(tasks: Task[]) {
  const ids = (s: Status) => tasks.filter((t) => t.status === s).map((t) => t.id);
  return { todo: ids("planned"), doing: ids("doing"), done: ids("done"), dropped: ids("dropped") };
}

// The card in front of the camera: one being edited, or one being made from a
// lane's "+". `shown` once it has arrived and the form is on it; `leaving` once
// Save, Cancel or Delete has sent it back, until it lands.
type Open = ({ kind: "edit"; task: Task } | { kind: "new"; lane: Lane }) & { shown: boolean; leaving: boolean };

// A card that is already on the board (just pulled, or just made), put in its
// lane from a fresh read: the frontend's copy doesn't have it yet.
function place(id: number, status: Status, index = Infinity) {
  return api.getBoard().then((b) => {
    const task = b.tasks.find((t) => t.id === id);
    if (!task) return;
    return api.applyBoard(arrangement(moved(b.tasks, task, status, index)));
  });
}

export default function Board() {
  const [board, setBoard] = useState<BoardData | null>(null);
  const [open, setOpen] = useState<Open | null>(null);
  const boardRef = useRef<KanbanBoard3DHandle>(null);
  const [err, setErr] = useState<string | null>(null);
  // A write the backend refused, shown above the board it reloaded — not the
  // fatal screen, which hid the board until the tab was remounted.
  const [notice, setNotice] = useState<string | null>(null);

  // Read once: the board takes its theme on mount only.
  const theme = useMemo(boardTheme, []);
  const laneColors = useMemo(() => Object.fromEntries(LANES.map((l) => [l.lane, token(l.token)])), []);

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

  // A new board object is a new columns array, which is also how a refused
  // move is undone: the 3D board animates to whatever it is handed.
  const columns = useMemo<KanbanColumnData[]>(() => {
    if (!board) return [];
    const muted = token("muted");
    return LANES.map(({ lane, label }) => ({
      id: lane,
      title: label,
      color: laneColors[lane],
      // The session lanes' "+" only means something while a session is open.
      canAdd: lane === "backlog" || board.session != null,
      cards: (lane === "backlog" ? board.backlog : board.tasks.filter((t) => t.status === lane)).map((t) =>
        toCard(t, muted),
      ),
    }));
  }, [board, laneColors]);

  if (err) return <div className="p-12 text-center text-muted">{err}</div>;
  if (!board) return null;

  const session = board.session;

  // Usually the gate closed the session this board belongs to; reloading
  // brings up the one that replaced it.
  const fail = (e: unknown) => {
    setNotice(String(e));
    refresh();
  };
  // The board already shows the drop; handing it the unchanged data puts the
  // card back.
  const refuse = (why: string | null) => {
    setNotice(why);
    setBoard({ ...board });
  };

  // Every move: optimistic first, then one write (two for a pull out of the
  // backlog), then a reload.
  const onCardMove = (e: CardMoveEvent) => {
    const taskId = Number(e.cardId);
    const target = e.toColumn as Lane;
    const onBoard = board.tasks.find((t) => t.id === taskId);
    const inBacklog = board.backlog.find((t) => t.id === taskId);
    setNotice(null);
    if (onBoard) {
      if (target === "backlog") {
        setBoard({
          ...board,
          tasks: board.tasks.filter((t) => t.id !== taskId),
          backlog: [...board.backlog, { ...onBoard, session_id: null, status: "planned" }],
        });
        api.unpullTask(taskId).then(refresh).catch(fail);
        return;
      }
      const next = moved(board.tasks, onBoard, target, e.toIndex);
      setBoard({ ...board, tasks: next });
      api.applyBoard(arrangement(next)).then(refresh).catch(fail);
    } else if (inBacklog) {
      // The backlog's order isn't stored, so a reorder inside it can't stick.
      if (target === "backlog") return refuse(null);
      if (!session) return refuse(NO_SESSION);
      setBoard({
        ...board,
        backlog: board.backlog.filter((t) => t.id !== taskId),
        tasks: moved(board.tasks, { ...inBacklog, session_id: session.id }, target, e.toIndex),
      });
      // A pull lands at the end of To Do; the second write puts it where it
      // was dropped.
      api
        .pullTask(taskId)
        .then(() => place(taskId, target, e.toIndex))
        .then(refresh)
        .catch(fail);
    }
  };

  const onCardClick = (card: KanbanCardData) => {
    const id = Number(card.id);
    const task = board.tasks.find((t) => t.id === id) ?? board.backlog.find((t) => t.id === id);
    if (task && boardRef.current?.openCard(card.id)) setOpen({ kind: "edit", task, shown: false, leaving: false });
  };

  const onAddClick = (lane: Lane) => {
    if (boardRef.current?.openNew(lane)) setOpen({ kind: "new", lane, shown: false, leaving: false });
  };

  const leave = () => setOpen((o) => o && { ...o, leaving: true });

  // The card lands with what the store now holds (label colours included),
  // then the board takes the same read. The 3D board holds the new columns
  // until the card is down, so nothing reflows under it mid-slam.
  const land = (id: number) =>
    api.getBoard().then((b) => {
      const task = b.tasks.find((t) => t.id === id) ?? b.backlog.find((t) => t.id === id);
      leave();
      boardRef.current?.closeCard(task ? { data: toCard(task, token("muted")) } : { remove: true });
      setNotice(null);
      setBoard(b);
    });

  const cancel = () => {
    if (!open || open.leaving) return;
    leave();
    // A draft has nothing to go back to, so it shrinks away.
    boardRef.current?.closeCard(open.kind === "new" ? { remove: true } : { slam: false });
  };

  return (
    <div className="flex flex-1 flex-col p-4">
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
      {/* Says why only the Backlog has a "+". */}
      {!session && <p className="mb-3 text-xs text-muted">{NO_SESSION}</p>}

      {/* The board fills this box, so it needs a definite size: absolute
          inset-0 gets one from the flex-grown wrapper. */}
      <div className="relative min-h-[360px] flex-1">
        <Suspense fallback={null}>
          <KanbanBoard3D
            // As a style, not a class: the component's own inline
            // `position: relative` would override an `absolute` class, and the
            // box would then size itself from the canvas instead of filling in.
            style={FILL}
            columns={columns}
            theme={theme}
            // Near overhead, so the board faces the user with just a hint of
            // depth. A narrow fov (a longer lens, further away) keeps the
            // columns from fanning out. No framing margin lets the board use
            // the box.
            cameraAngle={75}
            fov={20}
            framePadding={1.0}
            // Room above the first card for the column title.
            layout={LAYOUT}
            motion={MOTION}
            overflow="scroll"
            // Columns lengthen until the board fills the box's height too.
            fillHeight
            addButtons
            onAddClick={(id) => onAddClick(id as Lane)}
            onCardMove={onCardMove}
            onCardClick={onCardClick}
            ref={boardRef}
            onFocusArrive={() => setOpen((o) => o && { ...o, shown: true })}
            onFocusClose={() => setOpen(null)}
          />
        </Suspense>

        {open && (
          <CardEditor3D board={boardRef} shown={open.shown && !open.leaving} onDismiss={cancel}>
            {open.kind === "new" ? (
              <TaskForm
                initial={blankTask("")}
                heading={`New task in ${laneLabel(open.lane)}`}
                saveText="Add task"
                save={(title, notes, dueDate, labels) =>
                  api
                    .createTask(title, notes, dueDate, labels, open.lane === "backlog")
                    // A new session card starts in To Do.
                    .then((id) =>
                      (open.lane === "backlog" || open.lane === "planned"
                        ? Promise.resolve()
                        : place(id, open.lane)
                      ).then(() => land(id)),
                    )
                }
                onClose={cancel}
                onDelete={null}
                onLabelsChanged={refresh}
              />
            ) : (
              <TaskForm
                key={open.task.id}
                initial={open.task}
                saveText="Save"
                save={(title, notes, dueDate, labels) =>
                  api.updateTask(open.task.id, title, notes, dueDate, labels).then(() => land(open.task.id))
                }
                onClose={cancel}
                onLabelsChanged={refresh}
                // A backlog row can be deleted; a session card is history.
                onDelete={
                  open.task.session_id == null
                    ? () => {
                        const id = open.task.id;
                        api
                          .deleteTask(id)
                          .then(() => {
                            leave();
                            boardRef.current?.closeCard({ remove: true });
                            refresh();
                          })
                          .catch(fail);
                      }
                    : null
                }
              />
            )}
          </CardEditor3D>
        )}
      </div>
    </div>
  );
}
