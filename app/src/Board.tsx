import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
// Types only: the runtime import is the lazy one below, so three.js stays out
// of the main chunk and loads with the Board tab.
import type { CardMoveEvent, KanbanCardData, KanbanColumnData, KanbanTheme } from "threejs-elements/kanban";
import type { KanbanBoard3DHandle } from "threejs-elements/react";
import * as api from "./api";
import CardEditor3D from "./CardEditor3D";
import { dueInfo, localDate } from "./format";
import { TaskForm, blankTask } from "./TaskEditor";
import type { Board as BoardData, Priority, Status, Task } from "./types";

const KanbanBoard3D = lazy(() =>
  import("threejs-elements/react").then((m) => ({ default: m.KanbanBoard3D })),
);

// The column ids: a session status, or the backlog, which is no status at
// all (session_id NULL). Dropped cards are history and aren't shown.
type Lane = "planned" | "doing" | "done" | "backlog";

const LANES: { lane: Lane; label: string; token: string }[] = [
  { lane: "planned", label: "To Do", token: "muted-foreground" },
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

// app.css's :root is the one source of the palette; these are its values,
// used only if a variable somehow isn't there (a stylesheet that failed to
// load should not leave the board black-on-black).
const FALLBACK: Record<string, string> = {
  foreground: "#f5f5f5",
  "muted-foreground": "#818181",
  warn: "#fcbb00",
  destructive: "#fb414a",
  good: "#00d294",
  violet: "#c27aff",
  "board-panel": "#121212",
  "board-card": "#1c1c1c",
  "board-edge": "#0a0a0a",
};

function token(name: string) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(`--${name}`).trim();
  return v || FALLBACK[name];
}

// The canvas draws its own text, so it is handed the page's font stack (Geist,
// from app.css) and the page's greys. The board's three surfaces are opaque
// tokens of their own: three.js colours take no alpha.
function boardTheme(): Partial<KanbanTheme> {
  return {
    fontFamily: getComputedStyle(document.body).fontFamily,
    panel: token("board-panel"),
    headerText: token("foreground"),
    headerBadge: "rgba(255,255,255,0.06)",
    headerBadgeText: token("muted-foreground"),
    cardFace: token("board-card"),
    cardText: token("foreground"),
    cardMuted: token("muted-foreground"),
    cardEdge: token("board-edge"),
    highlight: 0.035,
  };
}

// The board paints card text onto canvases once, with whatever font is ready
// at that moment, and never repaints for a font arriving later -- so it waits
// for Geist. Never for long: a font that fails to load must not blank the
// board, it just means the fallback face. Started at import, so the wait is
// usually already over by the time the Board tab mounts.
const GEIST = '"Geist Variable"';
const fontsReady: Promise<void> = Promise.race([
  Promise.all([document.fonts.load(`500 16px ${GEIST}`), document.fonts.load(`700 16px ${GEIST}`)]).then(
    () => {},
    () => {},
  ),
  new Promise<void>((resolve) => setTimeout(resolve, 1500)),
]);

// The canvas colours for what a card's footer says, read once from the theme.
type Palette = { muted: string; warn: string; bad: string };

const PRIORITY_TEXT: Record<Priority, string> = { high: "High", medium: "Med", low: "Low" };
const PRIORITY_COLOR: Record<Priority, keyof Palette> = { high: "bad", medium: "warn", low: "muted" };

// One tag fits on a card face; the rest are counted, and all of them are in
// the editor a click away. The footer says the priority and when it is due,
// in words; colour only reinforces them. A card burns on the day it is due,
// unless it is done -- as done cards show their date muted, with no alarm.
function toCard(t: Task, palette: Palette, today: string): KanbanCardData {
  const [first, ...rest] = t.labels;
  const done = t.status === "done";
  const details = [{ text: PRIORITY_TEXT[t.priority], color: palette[PRIORITY_COLOR[t.priority]] }];
  if (t.due_date) {
    const due = dueInfo(t.due_date, dayOf(today));
    details.push(
      done
        ? { text: `Due ${due.date}`, color: palette.muted }
        : {
            text: due.text,
            color: due.cls === "overdue" ? palette.bad : due.cls === "today" ? palette.warn : palette.muted,
          },
    );
  }
  return {
    id: String(t.id),
    title: t.title,
    tag: first ? first.name + (rest.length ? ` +${rest.length}` : "") : undefined,
    color: first?.color ?? palette.muted,
    details,
    burning: !done && t.due_date === today,
  };
}

// "YYYY-MM-DD" as local midnight.
function dayOf(iso: string): Date {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d);
}

// High first. Within a level the user's own order stands: Array.sort is
// stable, and the tasks arrive in position order.
const RANK: Record<Priority, number> = { high: 0, medium: 1, low: 2 };
const byPriority = (tasks: Task[]) => [...tasks].sort((a, b) => RANK[a.priority] - RANK[b.priority]);

// The session's cards per status, in board order: by priority, then position.
// Dropped is carried along because apply_board wants the whole arrangement.
type Lanes = Record<Status, Task[]>;

function lanesOf(tasks: Task[]): Lanes {
  const by = (s: Status) => byPriority(tasks.filter((t) => t.status === s));
  return { planned: by("planned"), doing: by("doing"), done: by("done"), dropped: by("dropped") };
}

// `task` taken out of wherever it was and put into `target` at `index`, then
// the lane re-sorted. The sort is stable, so a drop among cards of its own
// priority stays where it was put, and one among another priority's cards
// goes to the nearest edge of its own group.
function moved(tasks: Task[], task: Task, target: Status, index: number): Task[] {
  const lanes = lanesOf(tasks.filter((t) => t.id !== task.id));
  lanes[target].splice(index, 0, { ...task, status: target });
  lanes[target] = byPriority(lanes[target]);
  return [...lanes.planned, ...lanes.doing, ...lanes.done, ...lanes.dropped];
}

// The same rule for the backlog, which is one list rather than lanes.
function reordered(backlog: Task[], task: Task, index: number): Task[] {
  const rest = byPriority(backlog.filter((t) => t.id !== task.id));
  rest.splice(index, 0, task);
  return byPriority(rest);
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

// The same for a card just sent back to the backlog, which lands at its end.
function placeInBacklog(id: number, index: number) {
  return api.getBoard().then((b) => {
    const task = b.backlog.find((t) => t.id === id);
    if (!task) return;
    return api.orderBacklog(reordered(b.backlog, task, index).map((t) => t.id));
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
  const palette = useMemo<Palette>(
    () => ({ muted: token("muted-foreground"), warn: token("warn"), bad: token("destructive") }),
    [],
  );
  // Checked synchronously too, so coming back to the tab doesn't blink.
  const [fontReady, setFontReady] = useState(() => document.fonts.check(`500 16px ${GEIST}`));
  useEffect(() => {
    let live = true;
    fontsReady.then(() => live && setFontReady(true));
    return () => {
      live = false;
    };
  }, []);

  // The local day, so a card catches fire at midnight (and one due yesterday
  // goes out) without a reload. Polled rather than timed to midnight: a
  // timeout does not count the hours the laptop spent asleep.
  const [today, setToday] = useState(() => localDate(new Date()));
  useEffect(() => {
    const tick = setInterval(() => setToday(localDate(new Date())), 30_000);
    return () => clearInterval(tick);
  }, []);

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
    const lanes = lanesOf(board.tasks);
    return LANES.map(({ lane, label }) => ({
      id: lane,
      title: label,
      color: laneColors[lane],
      // The session lanes' "+" only means something while a session is open.
      canAdd: lane === "backlog" || board.session != null,
      cards: (lane === "backlog" ? byPriority(board.backlog) : lanes[lane]).map((t) => toCard(t, palette, today)),
    }));
  }, [board, laneColors, palette, today]);

  if (err) return <div className="p-12 text-center text-muted-foreground">{err}</div>;
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
  const refuse = (why: string) => {
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
          backlog: reordered(board.backlog, { ...onBoard, session_id: null, status: "planned" }, e.toIndex),
        });
        // An unpull lands at the end of the backlog; the second write puts
        // it where it was dropped.
        api
          .unpullTask(taskId)
          .then(() => placeInBacklog(taskId, e.toIndex))
          .then(refresh)
          .catch(fail);
        return;
      }
      const next = moved(board.tasks, onBoard, target, e.toIndex);
      setBoard({ ...board, tasks: next });
      api.applyBoard(arrangement(next)).then(refresh).catch(fail);
    } else if (inBacklog) {
      if (target === "backlog") {
        const next = reordered(board.backlog, inBacklog, e.toIndex);
        setBoard({ ...board, backlog: next });
        api.orderBacklog(next.map((t) => t.id)).then(refresh).catch(fail);
        return;
      }
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
      boardRef.current?.closeCard(task ? { data: toCard(task, palette, today) } : { remove: true });
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
    <div className="flex flex-1 flex-col">
      {/* The board fills this box, so it needs a definite size: absolute
          inset-0 gets one from the flex-grown wrapper. Nothing sits above it:
          the board is the whole screen under the nav, and the two things
          worth saying float over its top-left corner instead of taking a row. */}
      <div className="relative min-h-[360px] flex-1">
        <div className="pointer-events-none absolute left-4 top-3 z-[5] flex flex-col items-start gap-2">
          {notice && (
            <p className="rounded-md border border-warn/20 bg-warn/10 px-3 py-1.5 text-[13px] text-warn backdrop-blur">
              {notice}
            </p>
          )}
          {/* Says why only the Backlog has a "+". */}
          {!session && <p className="text-xs text-muted-foreground">{NO_SESSION}</p>}
        </div>
        <Suspense fallback={null}>
          {fontReady && (
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
              cameraAngle={85}
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
          )}
        </Suspense>

        {open && (
          <CardEditor3D board={boardRef} shown={open.shown && !open.leaving} onDismiss={cancel}>
            {open.kind === "new" ? (
              <TaskForm
                initial={blankTask("")}
                heading={`New task in ${laneLabel(open.lane)}`}
                saveText="Add task"
                save={(title, notes, dueDate, priority, labels) =>
                  api
                    .createTask(title, notes, dueDate, priority, labels, open.lane === "backlog")
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
                save={(title, notes, dueDate, priority, labels) =>
                  api
                    .updateTask(open.task.id, title, notes, dueDate, priority, labels)
                    .then(() => land(open.task.id))
                }
                onClose={cancel}
                onLabelsChanged={refresh}
                // Any card on this board can be deleted outright, backlog or
                // session (the editor asks first).
                // A refusal (its session closed meanwhile) stays in the editor.
                onDelete={() =>
                  api.deleteTask(open.task.id).then(() => {
                    leave();
                    boardRef.current?.closeCard({ remove: true });
                    refresh();
                  })
                }
              />
            )}
          </CardEditor3D>
        )}
      </div>
    </div>
  );
}
