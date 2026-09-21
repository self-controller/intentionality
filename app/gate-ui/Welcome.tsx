import React, { useEffect, useState } from "react";
import { Button, TextInput, SectionHeading } from "../src/ui/primitives";
import TaskRow from "./TaskRow";
import DetailsPanel from "./DetailsPanel";
import { post } from "./bridge";
import type { Welcome as WelcomeState, PanelDraft } from "./types";

const EMPTY_DRAFT: PanelDraft = { due: "", notes: "", labels: [], pending: "" };

/** The gate's one screen.
 *
 *  The list itself is Python's: ui.WelcomeList decides what a tick means, what
 *  a refusal says and whether Start is allowed. React holds only the two text
 *  boxes that have not been committed yet, and hands them over on submit --
 *  which is what preserves the rule that a task still sitting in the add box
 *  counts towards starting. */
export default function Welcome({ state }: { state: WelcomeState }) {
  const [draft, setDraft] = useState("");
  const [minutes, setMinutes] = useState(state.minutes);
  const [panelDraft, setPanelDraft] = useState<PanelDraft>(EMPTY_DRAFT);

  // Reseed the panel only when Python says to -- it bumps token when a panel
  // opens or saves, and at no other time.
  const token = state.panel?.token ?? -1;
  useEffect(() => {
    setPanelDraft(
      state.panel
        ? { due: state.panel.due, notes: state.panel.notes, labels: state.panel.labels, pending: "" }
        : EMPTY_DRAFT,
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token]);

  const add = () => {
    if (draft.trim()) {
      post("add", { title: draft });
      setDraft("");
    }
  };

  const edit = (next: string) => {
    setDraft(next);
    // Python owns the count in the status line and the Start rule, so it needs
    // to know a task is sitting in the box uncommitted.
    post("add_text", { text: next });
  };

  // Everything uncommitted goes over in one message, so Start saves an open
  // panel first the way the GTK gate did, and a refusal stops the start.
  const start = () =>
    post("submit", {
      add_text: draft,
      minutes,
      details: state.panel ? { index: state.panel.index, ...panelDraft } : null,
    });

  return (
    <div className="flex h-full flex-col items-center">
      {/* The list scrolls; the footer does not, so a long backlog can never
          push Start off the screen. */}
      <div className="w-full flex-1 overflow-y-auto">
        {/* min-h-full + justify-center centres a short list in the screen and
            lets a long one scroll from the top, instead of always hugging it. */}
        <div className="mx-auto flex min-h-full w-[36rem] max-w-[calc(100%-4rem)]
                        flex-col justify-center py-[3rem]">
          <h1 className="gate-rise text-[2.1rem] font-semibold tracking-tight text-white">
            {state.greeting}
          </h1>
          <p className="mt-[0.15rem] text-[0.95rem] text-muted">{state.date}</p>

          {state.notes && (
            <p className="mt-[1.4rem] text-[0.85rem] leading-relaxed text-muted">
              {state.notes}
            </p>
          )}

          <SectionHeading className="mt-[2.2rem]">Your tasks</SectionHeading>

          {state.rows.length === 0 ? (
            <p className="mt-[0.8rem] text-[0.95rem] text-muted">
              No tasks yet. Add one below.
            </p>
          ) : (
            <ul className="mt-[0.5rem]">
              {state.rows.map((row) => (
                <React.Fragment key={row.index}>
                  <TaskRow row={row} open={state.panel?.index === row.index} />
                  {state.panel?.index === row.index && (
                    <DetailsPanel
                      panel={state.panel}
                      labels={state.labels}
                      draft={panelDraft}
                      setDraft={setPanelDraft}
                    />
                  )}
                </React.Fragment>
              ))}
            </ul>
          )}

          <div className="mt-[1.1rem] flex gap-[0.5rem]">
            <TextInput
              value={draft}
              placeholder="Add a task…"
              autoFocus
              onChange={(e) => edit(e.target.value)}
              onKeyDown={(e) => {
                if (e.key !== "Enter") return;
                e.preventDefault();
                // Enter on an empty box moves on, the way a blank line does on
                // the terminal.
                if (draft.trim()) add();
                else document.getElementById("gate-minutes")?.focus();
              }}
              className="flex-1"
            />
            <Button onClick={add} disabled={!draft.trim()}>Add</Button>
          </div>
        </div>
      </div>

      <div className="w-full border-t border-line/60 bg-bg">
        <div className="mx-auto flex w-[36rem] max-w-[calc(100%-4rem)] flex-col gap-[0.6rem] py-[1.2rem]">
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
              onKeyDown={(e) => { if (e.key === "Enter") start(); }}
              className="w-[7.5rem] text-center"
            />
            <span className="text-[0.9rem] text-muted">minutes</span>
            <span className="flex-1" />
            <Button tone="primary" onClick={start} disabled={!state.start_enabled && !draft.trim()}>
              Start session
            </Button>
          </div>
          <p className={"text-[0.8rem] " + (state.error ? "text-bad" : "text-muted")}>
            {state.error || state.status}
          </p>
        </div>
      </div>
    </div>
  );
}
