import { useEffect, useMemo, useRef, useState } from "react";
import { Play, Plus, SlidersHorizontal } from "lucide-react";
import { Button } from "../src/ui/primitives";
import { matches, resolve, useTap } from "../src/keybinds";
import DetailsPanel, { DetailsFields } from "./DetailsPanel";
import { groupRows, OTHER } from "./LabelGroup";
import TodayList from "./TodayList";
import Backlog from "./Backlog";
import PlanBar from "./PlanBar";
import { post } from "./bridge";
import { rowMatches, searchTerms } from "./search";
import type { Welcome as WelcomeState, PanelDraft, Row } from "./types";

const EMPTY_DRAFT: PanelDraft = { due: "", notes: "", labels: [], pending: "" };

/** Enter with no modifier but Shift: the fields' own Enter. */
const isBareEnter = (e: { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean }) =>
  e.key === "Enter" && !e.ctrlKey && !e.altKey && !e.metaKey;

/** The add box, caret at the end of whatever is in it. */
function focusAdd() {
  const box = document.getElementById("gate-add") as HTMLInputElement | null;
  if (!box) return;
  box.focus();
  box.setSelectionRange(box.value.length, box.value.length);
  box.scrollIntoView({ block: "nearest" });
}

/** The gate's one screen, in two zones: Today -- the ticked rows, once each,
 *  with the add box under them -- and Backlog, everything else under its
 *  labels. The checkbox is what moves a row across. The plan bar at the
 *  bottom closes: how long, how much, Start.
 *
 *  The list itself is Python's: ui.WelcomeList decides what a tick means, what
 *  a refusal says and whether Start is allowed. React holds only the text
 *  boxes that have not been committed yet, and hands them over on submit --
 *  which is what preserves the rule that a task still sitting in the add box
 *  counts towards starting. */
export default function Welcome({
  state,
  covered = false,
}: {
  state: WelcomeState;
  /** The intro is still in front: a key only dismisses it (Gate.tsx). */
  covered?: boolean;
}) {
  const [draft, setDraft] = useState("");
  // The add box's own details, kept while its editor is folded away, and
  // sent with the title so the task lands already labelled.
  const [composer, setComposer] = useState<PanelDraft>(EMPTY_DRAFT);
  const [composing, setComposing] = useState(false);
  // The add box's Doing switch: its task lands in Doing rather than To Do.
  // Off again after each task lands, so a run of typed tasks does not all
  // start in Doing by accident.
  const [addDoing, setAddDoing] = useState(false);
  const [minutes, setMinutes] = useState(state.minutes);
  const [panelDraft, setPanelDraft] = useState<PanelDraft>(EMPTY_DRAFT);
  // Folded-away backlog groups, by key. Everything starts open.
  const [folded, setFolded] = useState<ReadonlySet<string>>(new Set());
  // Which group's copy of a row asked for the panel. Python knows only the
  // row; a backlog task with two labels is listed twice, and the panel
  // belongs under the copy that was clicked.
  const [panelGroup, setPanelGroup] = useState<string | null>(null);
  // The backlog search, and which of its matches the arrows have picked.
  const [query, setQueryState] = useState("");
  const [cursor, setCursor] = useState(0);
  const setQuery = (q: string) => {
    setQueryState(q);
    setCursor(0);
  };

  // The shortcuts, defaults where Settings changed nothing. The window
  // listener below reads them through a ref, so it is added once.
  const keys = useMemo(() => resolve(state.keys), [state.keys]);
  const keysRef = useRef(keys);
  keysRef.current = keys;

  // Ticked and unmarked is Today; everything else waits.
  const todayRows = state.rows.filter((r) => r.today && r.mark === null);
  const waiting = state.rows.filter((r) => !(r.today && r.mark === null));
  const panelIndex = state.panel?.index ?? null;
  // The search narrows the backlog only. The row with an open panel stays
  // whatever is typed, so its unsaved edits never drop out of sight.
  const terms = searchTerms(query);
  const groups = groupRows(
    waiting.filter((r) => r.index === panelIndex || rowMatches(r, terms)),
    state.labels,
  );
  // What Enter or Alt+Enter in the search box moves: the matches in the order
  // they are listed, each task once (its first copy, should it wear two
  // labels), struck-through rows left out.
  const targets: { index: number; key: string }[] = [];
  if (terms.length > 0) {
    const seen = new Set<number>();
    for (const g of groups) {
      for (const r of g.rows) {
        if (r.mark !== null || seen.has(r.index) || !rowMatches(r, terms)) continue;
        seen.add(r.index);
        targets.push({ index: r.index, key: g.key });
      }
    }
  }
  // Clamped rather than reset: a picked row leaves the backlog, and the same
  // place then holds the next match.
  const active = targets.length ? targets[Math.min(cursor, targets.length - 1)] : null;
  const panelInToday = panelIndex !== null && todayRows.some((r) => r.index === panelIndex);
  const holding = groups.filter((g) => g.rows.some((r) => r.index === panelIndex));
  // After a save moves the row (its labels changed), fall back to its first copy.
  const panelAt = panelInToday
    ? null
    : (holding.find((g) => g.key === panelGroup)?.key ?? holding[0]?.key ?? null);

  const unfold = (key: string) =>
    setFolded((f) => {
      if (!f.has(key)) return f;
      const next = new Set(f);
      next.delete(key);
      return next;
    });
  const toggleFold = (key: string) =>
    setFolded((f) => {
      const next = new Set(f);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  // Reseed the panel only when Python says to -- it bumps token when a panel
  // opens or saves, and at no other time.
  const token = state.panel?.token ?? -1;
  useEffect(() => {
    setPanelDraft(
      state.panel
        ? { due: state.panel.due, notes: state.panel.notes, labels: state.panel.labels, pending: "" }
        : EMPTY_DRAFT,
    );
    // A panel that just opened is shown in full: its group unfolded and its
    // zone's own scroller moved so the editor is in view (the screen around
    // it cannot scroll, so nothing else moves).
    if (state.panel) {
      if (panelAt) unfold(panelAt);
      requestAnimationFrame(() =>
        document.getElementById("gate-panel")?.scrollIntoView({ block: "nearest" }),
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token]);

  // The pick stays in view as the arrows move it through a long list.
  const activeKey = active ? `${active.key}/${active.index}` : "";
  useEffect(() => {
    if (activeKey) document.querySelector('[data-active="true"]')?.scrollIntoView({ block: "nearest" });
  }, [activeKey]);

  const onSearchKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!targets.length) return;
      const at = Math.min(cursor, targets.length - 1);
      setCursor(e.key === "ArrowDown" ? Math.min(at + 1, targets.length - 1) : Math.max(at - 1, 0));
    } else if (matches(e, keys["gate.addDoing"]) || isBareEnter(e)) {
      // Start's chord never gets here (the window takes it first). Add to
      // Doing (Alt+Enter) is the add box's twin: the task goes in Doing
      // rather than To Do.
      e.preventDefault();
      if (!active || e.repeat) return;
      post(isBareEnter(e) ? "today" : "doing", { index: active.index });
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (query) setQuery("");
      else document.getElementById("gate-add")?.focus();
    }
  };

  // The box is emptied only when Python says its task landed, never on
  // posting: a refused add (a due date that is not one) keeps every field.
  // The value `added` had when a task was sent; a push that leaves it there
  // was the refusal, and the editor opens so what was refused is in view.
  const sentAt = useRef<number | null>(null);
  useEffect(() => {
    setDraft("");
    setComposer(EMPTY_DRAFT);
    setComposing(false);
    setAddDoing(false);
    sentAt.current = null;
  }, [state.added]);
  useEffect(() => {
    if (sentAt.current !== null && state.added === sentAt.current) {
      sentAt.current = null;
      setComposing(true);
    }
  }, [state]);

  // Alt+Enter passes doing = true: the task goes in Doing whatever the
  // switch says.
  const add = (doing = addDoing) => {
    if (!draft.trim()) return;
    sentAt.current = state.added;
    post("add", { title: draft, details: composer, doing });
    // A typed task starts ticked, so it lands in Today; should it be unticked
    // later it goes under its labels, so have those open rather than folded.
    const keys = [...composer.labels, composer.pending]
      .map((n) => n.trim().toLowerCase())
      .filter(Boolean);
    if (keys.length) keys.forEach(unfold);
    else unfold(OTHER);
  };

  const edit = (next: string) => {
    setDraft(next);
    // Python owns the Start rule, so it needs to know a task is sitting in
    // the box uncommitted.
    post("add_text", { text: next });
  };

  // Everything uncommitted goes over in one message, so Start saves an open
  // panel first the way the GTK gate did, and a refusal stops the start. The
  // add box's task goes with its details.
  const canStart = state.start_enabled || !!draft.trim();
  const start = () => {
    if (draft.trim()) sentAt.current = state.added;
    post("submit", {
      add_text: draft,
      add_details: composer,
      add_doing: addDoing,
      minutes,
      details: state.panel ? { index: state.panel.index, ...panelDraft } : null,
    });
  };

  // Start (Ctrl+Enter unless Settings says otherwise) from anywhere on the
  // screen. Captured at the window,
  // ahead of every field's own Enter (the add box, a label box, the minutes
  // box) and of the notes box's newline, so it is Start and only Start.
  const startRef = useRef<(() => void) | null>(null);
  startRef.current = canStart ? start : null;
  const coveredRef = useRef(covered);
  coveredRef.current = covered;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      const keys = keysRef.current;
      // Search (Ctrl+F) is the backlog search from anywhere, the intro
      // included: it is not stopped, so the intro still takes it as "skip",
      // and finding a task commits nothing.
      if (matches(e, keys["gate.search"])) {
        e.preventDefault();
        const box = document.getElementById("gate-search") as HTMLInputElement | null;
        box?.focus();
        box?.select();
        return;
      }
      // New task as a chord, should Settings make it one; a tap is useTap's.
      if (matches(e, keys["gate.focusAdd"])) {
        e.preventDefault();
        if (!coveredRef.current) focusAdd();
        return;
      }
      if (!matches(e, keys["gate.start"])) return;
      e.preventDefault();
      // Not stopped, so the intro's own listener still takes it as "skip";
      // the fields' Enter handlers step aside for Ctrl themselves.
      if (coveredRef.current) return;
      e.stopPropagation();
      if (!e.repeat) startRef.current?.();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // Type a new task: Super, tapped, puts the caret in the add box from
  // anywhere on the screen. sway gives the gate every key, Super included.
  useTap(keys["gate.focusAdd"], () => {
    if (!coveredRef.current) focusAdd();
  });

  const holdsDetails =
    composer.due !== "" ||
    composer.notes.trim() !== "" ||
    composer.labels.length > 0 ||
    composer.pending.trim() !== "";

  const renderPanel = (row: Row) =>
    state.panel ? (
      <DetailsPanel
        key={`panel-${row.index}`}
        panel={state.panel}
        labels={state.labels}
        draft={panelDraft}
        setDraft={setPanelDraft}
      />
    ) : null;

  return (
    // Exactly the window's height, and the page never scrolls: the header and
    // the plan bar stay where they are and only the backlog column moves
    // (Today too, should it ever outgrow its zone), so a long backlog or an
    // open panel can never push Start off the screen.
    <div className="flex h-dvh flex-none flex-col overflow-hidden">
      <div className="mx-auto flex min-h-0 w-full max-w-[104rem] flex-1 flex-col px-[4rem] pt-[3.5rem]">
        <header className="gate-rise flex-none">
          <p className="font-mono text-[0.74rem] uppercase tracking-[0.22em] text-muted">{state.date}</p>
          <h1 className="mt-[0.5rem] text-[2.7rem] font-medium leading-none tracking-[-0.025em] text-text">
            {state.greeting}
          </h1>
          {state.notes && (
            <p className="mt-[1rem] max-w-[66ch] text-[0.92rem] leading-relaxed text-muted">{state.notes}</p>
          )}
        </header>

        {/* A tick moves a row across at once (Python pushes the new state and
            the row re-mounts in the other zone with a short fade); nothing
            else animates, so the move reads as instant. The zones share
            whatever height the header and plan bar leave; stacked in a narrow
            window, Today takes what it needs and the backlog keeps at least
            a few rows. */}
        <div
          className="mt-[2.6rem] grid min-h-0 flex-1 grid-cols-1 grid-rows-[minmax(0,max-content)_minmax(8rem,1fr)]
                     gap-x-[2.5rem] gap-y-[3rem] lg:grid-cols-[minmax(0,7fr)_minmax(0,5fr)] lg:grid-rows-[minmax(0,1fr)]"
        >
            <TodayList
              rows={todayRows}
              labels={state.labels}
              panelIndex={panelInToday ? panelIndex : null}
              renderPanel={renderPanel}
              onDetails={() => setPanelGroup(null)}
            >
              <div className="mt-[1rem]">
                <div
                  className="flex items-center gap-[0.5rem] rounded-full border border-line bg-surface/70 py-[0.35rem]
                             pl-[0.9rem] pr-[0.4rem] transition-colors duration-150 focus-within:border-white/25
                             focus-within:bg-surface"
                >
                  <Plus className="size-[1.05rem] flex-none text-muted" aria-hidden="true" />
                  <input
                    id="gate-add"
                    value={draft}
                    placeholder="Add a task for today…"
                    aria-label="Add a task"
                    autoFocus
                    onChange={(e) => edit(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.nativeEvent.isComposing) return;
                      // Start's chord is the window's, even while the intro
                      // still covers it.
                      const doing = matches(e, keys["gate.addDoing"]);
                      if (!doing && !isBareEnter(e)) return;
                      e.preventDefault();
                      // Add to Doing (Alt+Enter) adds the task straight into
                      // Doing; on an empty box it does nothing.
                      if (doing) {
                        if (draft.trim()) add(true);
                        return;
                      }
                      // Enter on an empty box moves on, the way a blank line does on
                      // the terminal.
                      if (draft.trim()) add();
                      else document.getElementById("gate-minutes")?.focus();
                    }}
                    className="h-[2.1rem] min-w-0 flex-1 bg-transparent text-[0.95rem] text-text outline-none placeholder:text-muted/60"
                  />
                  <Button
                    size="sm"
                    tone="ghost"
                    selected={addDoing}
                    aria-pressed={addDoing}
                    title={addDoing ? "This task starts in Doing" : "Start this task in Doing (or add it with Alt+Enter)"}
                    className={addDoing ? "data-[selected=true]:bg-good/15 data-[selected=true]:text-good" : ""}
                    onClick={() => setAddDoing(!addDoing)}
                  >
                    <Play className={addDoing ? "fill-current" : ""} aria-hidden="true" />
                    Doing
                  </Button>
                  <Button
                    size="sm"
                    tone="ghost"
                    // Lit while it holds something, folded or not: it all goes with Add.
                    selected={composing || holdsDetails}
                    aria-expanded={composing}
                    onClick={() => setComposing(!composing)}
                  >
                    <SlidersHorizontal aria-hidden="true" />
                    Details
                  </Button>
                  <Button size="sm" tone={draft.trim() ? "primary" : "ghost"} onClick={() => add()} disabled={!draft.trim()}>
                    Add
                  </Button>
                </div>
                {composing && (
                  // Add is the commit, so there is no Save here.
                  <div
                    id="gate-composer"
                    className="gate-rise mt-[0.6rem] grid grid-cols-[repeat(auto-fit,minmax(15rem,1fr))] gap-x-[1.6rem]
                               gap-y-[1.1rem] rounded-[0.9rem] border border-line bg-surface/70 p-[1.1rem]"
                  >
                    <DetailsFields labels={state.labels} draft={composer} setDraft={setComposer} />
                    {state.add_error && (
                      <p className="col-span-full text-[0.8rem] text-bad">{state.add_error}</p>
                    )}
                  </div>
                )}
              </div>
            </TodayList>

            <Backlog
              groups={groups}
              labels={state.labels}
              folded={folded}
              onToggle={toggleFold}
              panelIndex={panelInToday ? null : panelIndex}
              panelAt={panelAt}
              renderPanel={renderPanel}
              onDetails={(key) => setPanelGroup(key)}
              query={query}
              setQuery={setQuery}
              onSearchKey={onSearchKey}
              terms={terms}
              active={active}
              total={waiting.length}
              inToday={terms.length ? todayRows.filter((r) => rowMatches(r, terms)).length : 0}
            />
          </div>
      </div>

      <PlanBar
        minutes={minutes}
        setMinutes={setMinutes}
        count={todayRows.length + (draft.trim() ? 1 : 0)}
        doing={todayRows.filter((r) => r.doing).length + (draft.trim() && addDoing ? 1 : 0)}
        canStart={canStart}
        start={start}
        error={state.error}
      />
    </div>
  );
}
