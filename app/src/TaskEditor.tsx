import { useEffect, useRef, useState } from "react";
import * as api from "./api";
import { addDays, ISO_DAY, localDate } from "./format";
import type { Label, LabelSummary } from "./types";

// What the editor starts from: a Task has all of these, and a card that does
// not exist yet is the same shape with nothing filled in.
export interface TaskFields {
  title: string;
  notes: string;
  due_date: string | null;
  labels: Label[];
}

export const blankTask = (title: string): TaskFields => ({
  title,
  notes: "",
  due_date: null,
  labels: [],
});

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/**
 * The whole card, editable — an existing one, or a new one before it exists.
 * An overlay rather than an inline expand: editing a card in place makes the
 * lane jump under the cursor you were about to drag with, and the notes field
 * wants more room than a lane is wide.
 *
 * Everything is saved in one call, so Cancel really discards.
 */
export default function TaskEditor({
  initial,
  heading,
  saveText,
  save,
  onClose,
  onSaved,
  onDrop,
  onDelete,
}: {
  initial: TaskFields;
  // Says what Save will do, for a card that doesn't exist yet.
  heading?: string;
  saveText: string;
  // The whole card as it should end up; `labels` is the full set.
  save: (title: string, notes: string, dueDate: string | null, labels: string[]) => Promise<unknown>;
  onClose: () => void;
  onSaved: () => void;
  // Only offered for a card on the board: a session row is history and gets
  // dropped, never deleted. Backlog rows are the other way round.
  onDrop: (() => void) | null;
  onDelete: (() => void) | null;
}) {
  const [title, setTitle] = useState(initial.title);
  const [notes, setNotes] = useState(initial.notes);
  // "" = no due date, which is also what an empty date input reports.
  const [due, setDue] = useState(initial.due_date ?? "");
  const [labels, setLabels] = useState<string[]>(initial.labels.map((l) => l.name));
  // Names typed here that no label has yet. They stay offered once unticked,
  // so a click can bring one back.
  const [typed, setTyped] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const [known, setKnown] = useState<LabelSummary[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.listLabels().then(setKnown).catch(() => setKnown([]));
    titleRef.current?.focus();
  }, []);

  // Esc cancels, like every other overlay in the app.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Every label there is, then what this card wears that the list lacks (one
  // deleted meanwhile), then what was typed here. One chip per name whatever
  // its case, matching the store's NOCASE unique index.
  const offered: string[] = [];
  for (const name of [
    ...known.map((l) => l.name),
    ...initial.labels.map((l) => l.name),
    ...typed,
  ]) {
    if (!offered.some((o) => same(o, name))) offered.push(name);
  }

  // The colour a label already has, so a chip looks the same here as on the
  // card. A label being typed for the first time has none yet.
  const colorOf = (name: string) =>
    known.find((l) => same(l.name, name))?.color ??
    initial.labels.find((l) => same(l.name, name))?.color;

  const has = (name: string) => labels.some((l) => same(l, name));
  const toggle = (name: string) =>
    setLabels(has(name) ? labels.filter((l) => !same(l, name)) : [...labels, name]);

  // Enter or a comma: an existing label is ticked (in its own spelling), a
  // new name is added and ticked.
  const addLabel = (raw: string) => {
    const name = raw.trim().replace(/,$/, "").trim();
    setDraft("");
    if (!name) return;
    const existing = offered.find((o) => same(o, name));
    if (!existing) setTyped([...typed, name]);
    if (!has(name)) setLabels([...labels, existing ?? name]);
  };

  const submit = () => {
    if (!title.trim()) return;
    // The picker only ever produces this shape; anything else was typed, and
    // is better refused here than in a round trip.
    if (due && !ISO_DAY.test(due)) {
      setError("The due date has to be a date, like 2026-09-18.");
      return;
    }
    setBusy(true);
    setError(null);
    // Whatever is half-typed in the label box counts as meant.
    const all = draft.trim() && !has(draft.trim()) ? [...labels, draft.trim()] : labels;
    save(title.trim(), notes, due || null, all)
      .then(onSaved)
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
  };

  return (
    <div
      className="fixed inset-0 z-20 flex items-center justify-center overflow-y-auto bg-black/80 p-6"
      onMouseDown={onClose}
    >
      <section
        className="max-h-full w-full max-w-[560px] overflow-y-auto rounded-[10px] border border-line bg-surface px-6 py-5"
        role="dialog"
        aria-modal="true"
        aria-label={heading ?? "Edit task"}
        // The scrim closes on click; the dialog itself must not.
        onMouseDown={(e) => e.stopPropagation()}
      >
        {heading && <p className="mb-2.5 text-[13px] text-muted">{heading}</p>}
        <input
          ref={titleRef}
          className="w-full rounded-md border border-line bg-bg px-2.5 py-2 text-base text-text outline-none transition-colors focus:border-accent"
          value={title}
          placeholder="Title"
          disabled={busy}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
        />

        <label className="mb-1.5 mt-4 block text-[13px] uppercase tracking-[0.06em] text-muted" htmlFor="task-due">
          Due
        </label>
        {/* The picker covers any day; the buttons are the two that come up
            most, one click each. */}
        <div className="flex flex-wrap items-center gap-2">
          <input
            id="task-due"
            type="date"
            value={due}
            disabled={busy}
            onChange={(e) => setDue(e.target.value)}
          />
          <button disabled={busy} onClick={() => setDue(localDate(new Date()))}>
            Today
          </button>
          <button disabled={busy} onClick={() => setDue(localDate(addDays(new Date(), 1)))}>
            Tomorrow
          </button>
          {due && (
            <button disabled={busy} onClick={() => setDue("")}>
              Clear
            </button>
          )}
        </div>

        <label className="mb-1.5 mt-4 block text-[13px] uppercase tracking-[0.06em] text-muted" htmlFor="task-notes">
          Notes
        </label>
        <textarea
          id="task-notes"
          className="min-h-[180px] w-full resize-y rounded-md border border-line bg-bg px-2.5 py-2 text-sm leading-relaxed text-text outline-none transition-colors focus:border-accent"
          rows={10}
          value={notes}
          placeholder="Anything you want to remember about this one…"
          disabled={busy}
          onChange={(e) => setNotes(e.target.value)}
        />

        <label className="mb-1.5 mt-4 block text-[13px] uppercase tracking-[0.06em] text-muted" htmlFor="task-label-input">
          Labels
        </label>
        {/* Every label is a chip to click; the tick, not the shading, is what
            says it is on. */}
        {offered.length ? (
          <ul className="flex flex-wrap gap-1.5">
            {offered.map((name) => {
              const on = has(name);
              const color = colorOf(name);
              return (
                <li key={name.toLowerCase()}>
                  <button
                    className={
                      "flex max-w-full items-center gap-1.5 rounded-full border border-line " +
                      "py-1 pl-1.5 pr-2.5 text-xs transition-colors duration-150 " +
                      "disabled:cursor-default " +
                      // The tick is what says it is on; the fill only reinforces it.
                      (on ? "bg-raised text-text" : "bg-transparent text-muted hover:text-text")
                    }
                    aria-pressed={on}
                    title={on ? `Take ${name} off` : `Tag with ${name}`}
                    disabled={busy}
                    onClick={() => toggle(name)}
                  >
                    <span className="w-[1em] flex-none text-center text-accent">{on ? "✓" : ""}</span>
                    <span
                      className={
                        "h-2 w-2 flex-none rounded-full" +
                        // A name typed here that no label wears yet: no colour until saved.
                        (color ? "" : " border border-muted")
                      }
                      style={color ? { background: color } : undefined}
                    />
                    <span className="overflow-hidden text-ellipsis whitespace-nowrap">{name}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="text-xs text-muted">No labels yet. Type one below to make it.</p>
        )}
        <input
          id="task-label-input"
          value={draft}
          placeholder="New label…"
          disabled={busy}
          onChange={(e) => {
            // A comma commits, so pasting "a, b" works as typing it does.
            if (e.target.value.endsWith(",")) addLabel(e.target.value);
            else setDraft(e.target.value);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              addLabel(draft);
            } else if (e.key === "Backspace" && !draft && labels.length) {
              setLabels(labels.slice(0, -1));
            }
          }}
        />

        {error && <p className="mt-3 text-xs text-bad">{error}</p>}

        <div className="mt-5 flex items-center gap-2">
          <button disabled={busy || !title.trim()} onClick={submit}>
            {saveText}
          </button>
          <button disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <span className="flex-1" />
          {onDrop && (
            <button
              className="rounded-md border border-line px-2.5 py-1 text-muted transition-colors hover:border-bad hover:text-bad"
              disabled={busy}
              title="Move to the dropped tray"
              onClick={onDrop}
            >
              Drop
            </button>
          )}
          {onDelete && (
            <button
              className="rounded-md border border-line px-2.5 py-1 text-muted transition-colors hover:border-bad hover:text-bad"
              disabled={busy}
              onClick={onDelete}
            >
              Delete
            </button>
          )}
        </div>
      </section>
    </div>
  );
}
