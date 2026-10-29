import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
// Types only: the runtime import is the lazy one below, so three.js stays out
// of the main chunk and loads with the Board tab.
import type {
  CardMoveEvent,
  FocusRect,
  KanbanCardData,
  KanbanColumnData,
  KanbanMount,
  KanbanTheme,
} from "threejs-elements/kanban";
import type { KanbanBoard3DHandle } from "threejs-elements/react";
import LabelFilter, { NO_LABEL, labelOptions, matches } from "@/components/LabelFilter";
import * as api from "./api";
import CardEditor3D from "./CardEditor3D";
import { dueInfo, localDate } from "./format";
import { TaskForm, blankTask } from "./TaskEditor";
import { TaskView } from "./TaskView";
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

// threejs-elements' defaultLayout, in board units: a column's width, the gap
// between columns and a panel's padding. For the bottom gap below.
const COLUMN = { width: 2, gap: 0.3, padding: 0.15 };
const BOARD_WIDTH = LANES.length * COLUMN.width + (LANES.length - 1) * COLUMN.gap;

const FILL = { position: "absolute", inset: 0 } as const;

// A few sparks, a thump and one small hop on a slam, but no flash: it lit a
// yellow spot in the middle of the card's face. Cards open instantly (see
// onCardClick); the focus spring is only what a non-instant open would fly
// with. A constant: the board re-applies a new object.
const MOTION = {
  slamFlash: 0,
  sparkCount: 14,
  slamBounce: 0.12,
  focusSpring: { stiffness: 260, damping: 34 },
};

const NO_SESSION = "No open session — start one at the gate.";

// The width, in CSS px, the filter chip needs to say "All labels" in full;
// narrower, and it is its icon alone.
const CHIP_WITH_NAME = 112;

// The backlog's label filter (a LabelFilter key; null = every card). Kept out
// here because Board is unmounted whenever another tab is chosen.
let rememberedFilter: string | null = null;

const sameRect = (a: FocusRect | null, b: FocusRect | null) =>
  a === b ||
  (a != null &&
    b != null &&
    Math.round(a.left) === Math.round(b.left) &&
    Math.round(a.top) === Math.round(b.top) &&
    Math.round(a.width) === Math.round(b.width) &&
    Math.round(a.height) === Math.round(b.height));

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

// The card in front of the camera: one being read (a click opens it this
// way), one being edited (its Edit button), or one being made from a lane's
// "+". `shown` while the face is on it -- at once, as cards open instantly;
// `leaving` once Close, Save, Cancel or Delete has sent it back, until it
// lands.
type Open = ({ kind: "view" | "edit"; task: Task } | { kind: "new"; lane: Lane }) & {
  shown: boolean;
  leaving: boolean;
};

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
  const [filter, setFilterState] = useState(rememberedFilter);
  // Cards a filtered backlog shows although they don't match: ones just
  // dropped, made or edited in it, which must not vanish as they land.
  // Forgotten when the filter changes.
  const [kept, setKept] = useState<ReadonlySet<number>>(new Set());
  // Where the Backlog header's free stretch is on screen, for the filter.
  const [slot, setSlot] = useState<FocusRect | null>(null);
  // CSS px kept clear under the board. fillHeight fits the board to the box's
  // height, so its bottom margin is a few px while its sides keep a wider one;
  // this makes up the difference.
  const [bottom, setBottom] = useState(0);

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
  // A card saved without labels gets one from Jev a moment later
  // (autolabel.rs), after the card has already landed.
  useEffect(() => {
    refresh();
    const unlistenOpened = listen("session:opened", refresh);
    const unlistenClosed = listen("session:closed", refresh);
    const unlistenLabelled = listen("task:labelled", refresh);
    return () => {
      unlistenOpened.then((f) => f());
      unlistenClosed.then((f) => f());
      unlistenLabelled.then((f) => f());
    };
  }, [refresh]);

  const options = useMemo(() => (board ? labelOptions(board.backlog) : []), [board]);
  // A label whose last card has left the backlog filters nothing any more.
  const active = filter != null && options.some((o) => o.key === filter) ? filter : null;
  const shows = useCallback(
    (t: Task) => active == null || matches(t, active) || kept.has(t.id),
    [active, kept],
  );

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
      cards: (lane === "backlog" ? byPriority(board.backlog).filter(shows) : lanes[lane]).map((t) =>
        toCard(t, palette, today),
      ),
    }));
  }, [board, laneColors, palette, today, shows]);

  // The filter sits over the header's free stretch, which moves when the board
  // re-frames (a resize, a layoutchange) or redraws the header (its count).
  // Measured a frame later, once the board has caught up.
  const box = useRef<HTMLDivElement>(null);
  const measureFrame = useRef(0);
  const measure = useCallback(() => {
    cancelAnimationFrame(measureFrame.current);
    measureFrame.current = requestAnimationFrame(() => {
      const next = boardRef.current?.getHeaderSlot("backlog") ?? null;
      setSlot((prev) => (sameRect(prev, next) ? prev : next));
      // The header's on-screen height gives px per board unit; from that, the
      // gap beside the (centered) board and the one above it, which the camera
      // mirrors below. Near overhead, so foreshortening is negligible.
      const width = box.current?.clientWidth;
      if (!next || !width || next.height <= 0) return;
      const unit = next.height / LAYOUT.headerDepth;
      const side = (width - BOARD_WIDTH * unit) / 2;
      const top = next.top - COLUMN.padding * unit;
      const gap = Math.max(0, Math.round(side - top));
      setBottom((prev) => (Math.abs(prev - gap) < 1 ? prev : gap));
    });
  }, []);
  const onReady = useCallback(
    (mount: KanbanMount) => {
      mount.board.addEventListener("layoutchange", measure);
      mount.board.addEventListener("dragend", measure);
      mount.board.addEventListener("focusclose", measure);
      measure();
    },
    [measure],
  );
  const loaded = board != null;
  useEffect(() => {
    if (!box.current) return;
    const ro = new ResizeObserver(measure);
    ro.observe(box.current);
    return () => {
      ro.disconnect();
      cancelAnimationFrame(measureFrame.current);
    };
  }, [measure, loaded]);
  useEffect(() => measure(), [columns, measure]);

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

  const setFilter = (key: string | null) => {
    rememberedFilter = key;
    setFilterState(key);
    setKept(new Set());
  };
  const keep = (id: number) => setKept((k) => new Set(k).add(id));
  // A card made in the filtered backlog starts out wearing the label shown.
  const presetLabel = options.find((o) => o.key === active)?.label ?? null;

  // A drop's index counts the cards the backlog shows; its order is the whole
  // list's. The card goes before the shown card it was dropped above, or just
  // after the last one shown.
  const backlogIndex = (task: Task, index: number) => {
    const rest = byPriority(board.backlog.filter((t) => t.id !== task.id));
    const shown = rest.filter(shows);
    if (index < shown.length) return rest.indexOf(shown[index]);
    return shown.length ? rest.indexOf(shown[shown.length - 1]) + 1 : rest.length;
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
        const index = backlogIndex(onBoard, e.toIndex);
        keep(taskId);
        setBoard({
          ...board,
          tasks: board.tasks.filter((t) => t.id !== taskId),
          backlog: reordered(board.backlog, { ...onBoard, session_id: null, status: "planned" }, index),
        });
        // An unpull lands at the end of the backlog; the second write puts
        // it where it was dropped.
        api
          .unpullTask(taskId)
          .then(() => placeInBacklog(taskId, index))
          .then(refresh)
          .catch(fail);
        return;
      }
      const next = moved(board.tasks, onBoard, target, e.toIndex);
      setBoard({ ...board, tasks: next });
      api.applyBoard(arrangement(next)).then(refresh).catch(fail);
    } else if (inBacklog) {
      if (target === "backlog") {
        const next = reordered(board.backlog, inBacklog, backlogIndex(inBacklog, e.toIndex));
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
    // Instant: no flight up and no growing, the card is just there.
    if (task && boardRef.current?.openCard(card.id, { instant: true }))
      setOpen({ kind: "view", task, shown: true, leaving: false });
  };

  const onAddClick = (lane: Lane) => {
    if (boardRef.current?.openNew(lane, undefined, { instant: true }))
      setOpen({ kind: "new", lane, shown: true, leaving: false });
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

  // Edit and its Cancel swap the card's face; the card stays up.
  const setMode = (kind: "view" | "edit") =>
    setOpen((o) => (o && o.kind !== "new" && !o.leaving ? { ...o, kind } : o));

  // The open card as the board now has it, so a label recoloured in the
  // editor shows in the view it goes back to.
  const current = (task: Task) =>
    board.tasks.find((t) => t.id === task.id) ?? board.backlog.find((t) => t.id === task.id) ?? task;

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
      <div ref={box} className="relative min-h-[360px] flex-1">
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
              style={bottom ? { ...FILL, bottom } : FILL}
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
              onReady={onReady}
              onFocusArrive={() => setOpen((o) => o && { ...o, shown: true })}
              onFocusClose={() => setOpen(null)}
            />
          )}
        </Suspense>

        {/* Hidden while a card is open (the board is dimmed behind it), and
            when there is no label to choose. */}
        {slot && !open && (active != null || options.some((o) => o.key !== NO_LABEL)) && (
          <div
            className="pointer-events-none absolute z-[5] flex items-center justify-end"
            style={{ left: slot.left, top: slot.top, width: slot.width, height: slot.height }}
          >
            <LabelFilter
              options={options}
              value={active}
              total={board.backlog.length}
              compact={slot.width < CHIP_WITH_NAME}
              onChange={setFilter}
            />
          </div>
        )}

        {open && (
          <CardEditor3D board={boardRef} shown={open.shown && !open.leaving} mode={open.kind} onDismiss={cancel}>
            {open.kind === "view" ? (
              <TaskView
                task={current(open.task)}
                today={today}
                onEdit={() => setMode("edit")}
                onClose={cancel}
              />
            ) : open.kind === "new" ? (
              <TaskForm
                initial={blankTask("", open.lane === "backlog" ? presetLabel : null)}
                heading={`New task in ${laneLabel(open.lane)}`}
                saveText="Add task"
                save={(title, notes, dueDate, priority, labels) =>
                  api
                    .createTask(title, notes, dueDate, priority, labels, open.lane === "backlog")
                    // A new session card starts in To Do.
                    .then((id) => {
                      keep(id);
                      return (
                        open.lane === "backlog" || open.lane === "planned" ? Promise.resolve() : place(id, open.lane)
                      ).then(() => land(id));
                    })
                }
                onClose={cancel}
                onDelete={null}
                onLabelsChanged={refresh}
              />
            ) : (
              <TaskForm
                key={open.task.id}
                initial={current(open.task)}
                saveText="Save"
                save={(title, notes, dueDate, priority, labels) =>
                  api
                    .updateTask(open.task.id, title, notes, dueDate, priority, labels)
                    .then(() => {
                      keep(open.task.id);
                      return land(open.task.id);
                    })
                }
                // Back to reading it; the dimmed board around it still closes.
                onClose={() => setMode("view")}
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
