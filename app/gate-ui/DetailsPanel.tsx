import { useState } from "react";
import { Button, TextInput, SectionHeading } from "../src/ui/primitives";
import Calendar from "./Calendar";
import { post } from "./bridge";
import type { Panel, LabelOpt, PanelDraft } from "./types";

/** The per-task editor, opened in place under its row.
 *
 *  Presentational: the draft lives in Welcome, because Start has to be able to
 *  commit an open panel in the same message that commits everything else --
 *  the GTK gate saved an open panel before starting, and that rule is worth
 *  keeping. Python reseeds the draft only when it bumps panel.token, so an
 *  unrelated state push cannot clobber what is being typed. */
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
    <li className="gate-rise mb-[0.6rem] ml-[3rem] border-l-2 border-line pl-[1.1rem]">
      <SectionHeading>Due</SectionHeading>
      <div className="mt-[0.35rem] flex flex-wrap items-center gap-[0.4rem]">
        <TextInput
          value={draft.due}
          placeholder="YYYY-MM-DD"
          onChange={(e) => edit({ due: e.target.value })}
          className="w-[8.5rem] text-[0.85rem]"
        />
        <div className="flex gap-[0.3rem] text-[0.8rem]">
          <Button onClick={() => edit({ due: "today" })}>Today</Button>
          <Button onClick={() => edit({ due: "tomorrow" })}>Tomorrow</Button>
          <Button onClick={() => edit({ due: "" })}>Clear</Button>
          <Button onClick={() => setCal(!cal)} className={cal ? "!border-accent !text-accent" : ""}>
            Calendar
          </Button>
        </div>
      </div>
      {cal && <Calendar value={draft.due} onPick={(d) => { edit({ due: d }); setCal(false); }} />}

      <SectionHeading className="mt-[1.1rem]">Notes</SectionHeading>
      <textarea
        value={draft.notes}
        onChange={(e) => edit({ notes: e.target.value })}
        rows={4}
        className="mt-[0.35rem] w-full resize-none rounded-[0.45rem] border border-line bg-surface
                   px-[0.7rem] py-[0.5rem] text-[0.85rem] leading-relaxed text-text outline-none
                   transition-colors duration-150 focus:border-accent"
      />

      <SectionHeading className="mt-[1.1rem]">Labels</SectionHeading>
      {offered.length === 0 && (
        <p className="mt-[0.35rem] text-[0.8rem] text-muted">No labels yet. Type one below.</p>
      )}
      <ul className="mt-[0.4rem] flex flex-wrap gap-[0.35rem]">
        {offered.map((l) => {
          const active = has(l.name);
          return (
            <li key={l.name.toLowerCase()}>
              <button
                aria-pressed={active}
                onClick={() => toggle(l.name)}
                className={
                  "flex items-center gap-[0.35rem] rounded-full border px-[0.7rem] py-[0.2rem] " +
                  "text-[0.78rem] transition-colors duration-150 " +
                  (active
                    ? "border-accent text-text"
                    : "border-line text-muted hover:border-muted hover:text-text")
                }
              >
                {/* The tick, not the shading, is what says it is on. */}
                <span className="w-[0.8em] text-accent">{active ? "✓" : ""}</span>
                <span
                  className={"h-[0.5em] w-[0.5em] rounded-full " + (l.color ? "" : "border border-muted")}
                  style={l.color ? { background: l.color } : undefined}
                />
                {l.name}
              </button>
            </li>
          );
        })}
      </ul>
      <div className="mt-[0.5rem] flex gap-[0.4rem]">
        <TextInput
          value={draft.pending}
          placeholder="New label…"
          onChange={(e) => edit({ pending: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === ",") { e.preventDefault(); commitDraftLabel(); }
          }}
          className="w-[10rem] text-[0.82rem]"
        />
        <Button className="text-[0.8rem]" disabled={!draft.pending.trim()} onClick={commitDraftLabel}>
          Add label
        </Button>
      </div>

      {panel.error && <p className="mt-[0.8rem] text-[0.8rem] text-bad">{panel.error}</p>}

      <div className="mb-[0.4rem] mt-[1rem] flex gap-[0.5rem] text-[0.9rem]">
        <Button tone="primary" onClick={() => post("details_save", { index: panel.index, ...draft })}>
          Save
        </Button>
        <Button onClick={() => post("details_close", { index: panel.index })}>Cancel</Button>
      </div>
    </li>
  );
}
