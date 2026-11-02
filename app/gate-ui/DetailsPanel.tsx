import { useState } from "react";
import { Check } from "lucide-react";
import { Button, TextInput, SectionHeading } from "../src/ui/primitives";
import Calendar from "../src/ui/Calendar";
import { cn } from "../src/lib/utils";
import { post } from "./bridge";
import type { Panel, LabelOpt, PanelDraft } from "./types";

/** Due, Notes and Labels: the editor's fields, shared by a row's panel and by
 *  the add box's details. Three sibling sections and no wrapper, so each
 *  caller lays them out -- both put them in a grid that folds to one column
 *  when the zone is narrow. */
export function DetailsFields({
  labels,
  draft,
  setDraft,
}: {
  labels: LabelOpt[];
  draft: PanelDraft;
  setDraft: (d: PanelDraft) => void;
}) {
  const [cal, setCal] = useState(false);
  const edit = (patch: Partial<PanelDraft>) => setDraft({ ...draft, ...patch });

  const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
  const has = (n: string) => draft.labels.some((l) => same(l, n));
  const toggle = (n: string) =>
    edit({ labels: has(n) ? draft.labels.filter((l) => !same(l, n)) : [...draft.labels, n] });
  const commitDraftLabel = () => {
    const name = draft.pending.trim();
    if (name) edit({ labels: has(name) ? draft.labels : [...draft.labels, name], pending: "" });
  };

  const offered = [...labels];
  for (const l of draft.labels) {
    if (!offered.some((o) => same(o.name, l))) offered.push({ name: l, color: null, exists: false });
  }

  return (
    <>
      <div>
        <SectionHeading>Due</SectionHeading>
        <TextInput
          value={draft.due}
          placeholder="YYYY-MM-DD"
          aria-label="Due date"
          onChange={(e) => edit({ due: e.target.value })}
          className="mt-[0.5rem] w-[10.5rem] font-mono text-[0.85rem]"
        />
        {/* Words first, the grid second: the buttons are the usual answers. */}
        <div className="mt-[0.4rem] flex flex-wrap items-center gap-[0.25rem]">
          <Button size="sm" tone="ghost" onClick={() => edit({ due: "today" })}>Today</Button>
          <Button size="sm" tone="ghost" onClick={() => edit({ due: "tomorrow" })}>Tomorrow</Button>
          <Button size="sm" tone="ghost" selected={cal} onClick={() => setCal(!cal)}>
            Calendar
          </Button>
          {draft.due && (
            <Button size="sm" tone="ghost" onClick={() => edit({ due: "" })}>Clear</Button>
          )}
        </div>
        {/* Calendar sizes itself in em; this is what it is an em of. */}
        {cal && (
          <div className="text-[1rem]">
            <Calendar value={draft.due} onPick={(d) => { edit({ due: d }); setCal(false); }} />
          </div>
        )}
      </div>

      <div>
        <SectionHeading>Notes</SectionHeading>
        <textarea
          value={draft.notes}
          aria-label="Notes"
          onChange={(e) => edit({ notes: e.target.value })}
          rows={4}
          className="mt-[0.5rem] w-full resize-none rounded-[0.65rem] border border-line bg-surface px-[0.85rem]
                     py-[0.6rem] text-[0.9rem] leading-relaxed text-text outline-none transition-colors
                     duration-150 focus:border-white/25 focus:bg-raised"
        />
      </div>

      <div>
        <SectionHeading>Labels</SectionHeading>
        {offered.length === 0 && (
          <p className="mt-[0.5rem] text-[0.85rem] text-muted">No labels yet. Type one below.</p>
        )}
        <ul className="mt-[0.55rem] flex flex-wrap gap-[0.35rem]">
          {offered.map((l) => {
            const active = has(l.name);
            return (
              <li key={l.name.toLowerCase()}>
                <button
                  type="button"
                  aria-pressed={active}
                  onClick={() => toggle(l.name)}
                  className={cn(
                    "inline-flex items-center gap-[0.4rem] rounded-full border px-[0.7rem] py-[0.25rem] text-[0.8rem]",
                    "outline-none transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-text/50",
                    active
                      ? "border-white/40 bg-white/[0.06] text-text"
                      : "border-line text-muted hover:border-white/20 hover:text-text",
                  )}
                >
                  <span
                    className={cn("size-[0.5rem] flex-none rounded-full", !l.color && "border border-muted")}
                    style={l.color ? { background: l.color } : undefined}
                  />
                  {l.name}
                  {/* The tick, not the shading, is what says it is on. */}
                  {active && <Check className="size-[0.8em]" aria-hidden="true" />}
                </button>
              </li>
            );
          })}
        </ul>
        <div className="mt-[0.6rem] flex gap-[0.4rem]">
          <TextInput
            value={draft.pending}
            placeholder="New label…"
            aria-label="New label"
            onChange={(e) => edit({ pending: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === ",") { e.preventDefault(); commitDraftLabel(); }
            }}
            className="w-[12rem] text-[0.85rem]"
          />
          <Button size="sm" disabled={!draft.pending.trim()} onClick={commitDraftLabel}>
            Add label
          </Button>
        </div>
      </div>
    </>
  );
}

/** The per-task editor, opened in place under its row.
 *
 *  Presentational: the draft lives in Welcome, because Start has to be able to
 *  commit an open panel in the same message that commits everything else.
 *  Python reseeds the draft only when it bumps panel.token, so an unrelated
 *  state push cannot clobber what is being typed. */
export default function DetailsPanel({
  panel,
  labels,
  draft,
  setDraft,
}: {
  panel: Panel;
  labels: LabelOpt[];
  draft: PanelDraft;
  setDraft: (d: PanelDraft) => void;
}) {
  return (
    <li
      id="gate-panel"
      className="gate-pop my-[0.6rem] rounded-[0.9rem] border border-line bg-surface/70 p-[1.1rem]"
    >
      <div className="grid grid-cols-[repeat(auto-fit,minmax(15rem,1fr))] gap-x-[1.6rem] gap-y-[1.1rem]">
        <DetailsFields labels={labels} draft={draft} setDraft={setDraft} />
      </div>

      {panel.error && <p className="mt-[0.8rem] text-[0.8rem] text-bad">{panel.error}</p>}

      <div className="mt-[1rem] flex items-center gap-[0.4rem]">
        <Button tone="primary" size="sm" onClick={() => post("details_save", { index: panel.index, ...draft })}>
          Save
        </Button>
        <Button tone="ghost" size="sm" onClick={() => post("details_close", { index: panel.index })}>
          Cancel
        </Button>
      </div>
    </li>
  );
}
