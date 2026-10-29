import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button, TextInput, SectionHeading } from "../src/ui/primitives";
import TaskRow from "./TaskRow";
import DetailsPanel, { DetailsFields } from "./DetailsPanel";
import LabelGroup, { groupRows, stack, OTHER } from "./LabelGroup";
import { post } from "./bridge";
import type { Welcome as WelcomeState, PanelDraft } from "./types";

const EMPTY_DRAFT: PanelDraft = { due: "", notes: "", labels: [], pending: "" };

// A label card is never narrower than this; the cards' gap, both ways.
const CARD_MIN_REM = 30;
const CARD_GAP_REM = 0.8;

/** How many card columns fit the element's width. Measured in CSS px against
 *  the root rem, so the webview's zoom does not change the answer. */
function useColumns(ref: React.RefObject<HTMLElement | null>): number {
  const [cols, setCols] = useState(1);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const fit = () => {
      const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      const gap = CARD_GAP_REM * rem;
      setCols(Math.max(1, Math.floor((el.clientWidth + gap) / (CARD_MIN_REM * rem + gap))));
    };
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return cols;
}

/** The gate's one screen.
 *
 *  The list itself is Python's: ui.WelcomeList decides what a tick means, what
 *  a refusal says and whether Start is allowed. React holds only the two text
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
  const [minutes, setMinutes] = useState(state.minutes);
  const [panelDraft, setPanelDraft] = useState<PanelDraft>(EMPTY_DRAFT);
  // Folded-away groups, by key. Everything starts open.
  const [folded, setFolded] = useState<ReadonlySet<string>>(new Set());
  // Which group's copy of a row asked for the panel. Python knows only the
  // row; a task with two labels is listed twice, and the panel belongs under
  // the copy that was clicked.
  const [panelGroup, setPanelGroup] = useState<string | null>(null);

  const groups = groupRows(state.rows, state.labels);
  const column = useRef<HTMLDivElement>(null);
  const fits = useColumns(column);
  // Never more columns than cards: a lone group (only Other, say) takes the
  // whole width instead of half of it.
  const stacks = stack(groups, Math.min(fits, groups.length));
  const panelIndex = state.panel?.index ?? null;
  const holding = groups.filter((g) => g.rows.some((r) => r.index === panelIndex));
  // After a save moves the row (its labels changed), fall back to its first copy.
  const panelAt = holding.find((g) => g.key === panelGroup)?.key ?? holding[0]?.key ?? null;

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
    // A panel that just opened is shown in full: its group unfolded and the
    // page scrolled so the editor is on screen, clear of the footer.
    if (state.panel && panelAt) {
      unfold(panelAt);
      requestAnimationFrame(() =>
        document.getElementById("gate-panel")?.scrollIntoView({ block: "nearest" }),
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token]);

  // The box is emptied only when Python says its task landed, never on
  // posting: a refused add (a due date that is not one) keeps every field.
  // The value `added` had when a task was sent; a push that leaves it there
  // was the refusal, and the editor opens so what was refused is in view.
  const sentAt = useRef<number | null>(null);
  useEffect(() => {
    setDraft("");
    setComposer(EMPTY_DRAFT);
    setComposing(false);
    sentAt.current = null;
  }, [state.added]);
  useEffect(() => {
    if (sentAt.current !== null && state.added === sentAt.current) {
      sentAt.current = null;
      setComposing(true);
    }
  }, [state]);

  const add = () => {
    if (!draft.trim()) return;
    sentAt.current = state.added;
    post("add", { title: draft, details: composer });
    // Keep it in sight where it lands: under its labels, or Other.
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
      minutes,
      details: state.panel ? { index: state.panel.index, ...panelDraft } : null,
    });
  };

  // Ctrl+Enter is Start from anywhere on the screen. Captured at the window,
  // ahead of every field's own Enter (the add box, a label box, the minutes
  // box) and of the notes box's newline, so it is Start and only Start.
  const startRef = useRef<(() => void) | null>(null);
  startRef.current = canStart ? start : null;
  const coveredRef = useRef(covered);
  coveredRef.current = covered;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Enter" || !e.ctrlKey || e.altKey || e.isComposing) return;
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

  return (
    <div className="flex flex-1 flex-col">
      {/* The page scrolls (gate.css); the footer is sticky, so a long backlog
          or an open panel can never push Start off the screen. flex-1 +
          justify-center centres a short list and lets a long one run from the
          top, since centring only ever shares out space that is left over. */}
      <div ref={column} className="mx-auto flex w-[76rem] max-w-[calc(100%-4rem)] flex-1
                      flex-col justify-center py-[3rem]">
        <h1 className="gate-rise text-[2.1rem] font-semibold tracking-tight text-text">
          {state.greeting}
        </h1>
        <p className="mt-[0.15rem] text-[0.95rem] text-muted">{state.date}</p>

        {state.notes && (
          <p className="mt-[1.4rem] max-w-[48rem] text-[0.85rem] leading-relaxed text-muted">
            {state.notes}
          </p>
        )}

        <SectionHeading className="mt-[2.2rem]">Your tasks</SectionHeading>
        <p className="mt-[0.3rem] text-[0.8rem] text-muted">
          Tick what you are doing today. The rest stays in your backlog.
        </p>

        {state.rows.length === 0 ? (
          <p className="mt-[0.8rem] text-[0.95rem] text-muted">
            No tasks yet. Add one below.
          </p>
        ) : (
          // One card per label, stacked in columns (stack() in LabelGroup) so
          // each card sits straight under the one above it in its column.
          <div className="mt-[0.9rem] flex items-start gap-[0.8rem]">
            {stacks.map((cards, i) => (
              <div key={i} className="flex min-w-0 flex-1 flex-col gap-[0.8rem]">
                {cards.map((group) => (
                  <LabelGroup
                    key={group.key}
                    group={group}
                    open={!folded.has(group.key)}
                    onToggle={() => toggleFold(group.key)}
                  >
                    {group.rows.map((row) => {
                      const open = row.index === panelIndex && group.key === panelAt;
                      return [
                        <TaskRow
                          key={row.index}
                          row={row}
                          open={open}
                          onDetails={() => setPanelGroup(group.key)}
                        />,
                        open && state.panel && (
                          <DetailsPanel
                            key="panel"
                            panel={state.panel}
                            labels={state.labels}
                            draft={panelDraft}
                            setDraft={setPanelDraft}
                          />
                        ),
                      ];
                    })}
                  </LabelGroup>
                ))}
              </div>
            ))}
          </div>
        )}

        <div className="mt-[1.1rem] flex gap-[0.5rem]">
          <TextInput
            value={draft}
            placeholder="Add a task…"
            autoFocus
            onChange={(e) => edit(e.target.value)}
            onKeyDown={(e) => {
              // Ctrl+Enter is Start's, even while the intro still covers it.
              if (e.key !== "Enter" || e.ctrlKey) return;
              e.preventDefault();
              // Enter on an empty box moves on, the way a blank line does on
              // the terminal.
              if (draft.trim()) add();
              else document.getElementById("gate-minutes")?.focus();
            }}
            className="flex-1"
          />
          <Button
            aria-expanded={composing}
            onClick={() => setComposing(!composing)}
            // Lit while it holds something, folded or not: it all goes with Add.
            className={
              composing || composer.due || composer.notes.trim() || composer.labels.length || composer.pending.trim()
                ? "!border-accent !text-accent"
                : ""
            }
          >
            Details
          </Button>
          <Button onClick={add} disabled={!draft.trim()}>Add</Button>
        </div>
        {composing && (
          // Columns rather than the row panel's stack: the add box spans the
          // whole screen. Add is the commit, so there is no Save here.
          <div
            id="gate-composer"
            className="gate-rise mt-[0.8rem] grid grid-cols-[repeat(auto-fit,minmax(20rem,1fr))] items-start
                       gap-x-[2rem] gap-y-[1.1rem] rounded-[0.6rem] border border-line bg-surface/60 p-[1.1rem]
                       text-[0.82rem]"
          >
            <DetailsFields labels={state.labels} draft={composer} setDraft={setComposer} />
            {state.add_error && (
              <p className="col-span-full text-[0.8rem] text-bad">{state.add_error}</p>
            )}
          </div>
        )}
      </div>

      <div className="sticky bottom-0 z-20 w-full border-t border-line bg-bg/95">
        <div className="mx-auto flex w-[76rem] max-w-[calc(100%-4rem)] flex-col gap-[0.6rem] py-[1.2rem]">
          <div className="flex items-center gap-[0.6rem]">
            <label htmlFor="gate-minutes" className="text-[0.9rem] text-muted">
              How long will you be here?
            </label>
            <TextInput
              id="gate-minutes"
              value={minutes}
              placeholder="open-ended"
              inputMode="numeric"
              onChange={(e) => setMinutes(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && !e.ctrlKey) start(); }}
              className="w-[7.5rem] text-center"
            />
            <span className="text-[0.9rem] text-muted">minutes</span>
            <span className="flex-1" />
            <span className="text-[0.75rem] text-muted/70">Ctrl+Enter</span>
            <Button tone="primary" onClick={start} disabled={!canStart}>
              Start session
            </Button>
          </div>
          {state.error && <p className="text-[0.8rem] text-bad">{state.error}</p>}
        </div>
      </div>
    </div>
  );
}
