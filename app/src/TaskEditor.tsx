import { useEffect, useRef, useState } from "react";
import * as api from "./api";
import { addDays, ISO_DAY, localDate } from "./format";
import Calendar from "./ui/Calendar";
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
 * Just the fields: the board lays it over the card it has flown up to face
 * you (CardEditor3D), rather than expanding the card in place, which would
 * make the lane jump under the cursor you were about to drag with.
 *
 * Everything is saved in one call, so Cancel really discards.
 */
export function TaskForm({
  initial,
  heading,
  saveText,
  save,
  onClose,
  onDelete,
  onLabelsChanged,
}: {
  initial: TaskFields;
  // Says what Save will do, for a card that doesn't exist yet.
  heading?: string;
  saveText: string;
  // The whole card as it should end up; `labels` is the full set. Whatever
  // follows a save (the card landing) is part of this promise: the form stays
  // busy until it settles, and shows its error if it fails.
  save: (title: string, notes: string, dueDate: string | null, labels: string[]) => Promise<unknown>;
  onClose: () => void;
  // Only offered for a backlog row: a session card is history, and the way
  // off the board is a drag back to the Backlog column.
  onDelete: (() => void) | null;
  // A label was deleted outright, so other cards on screen changed too.
  onLabelsChanged?: () => void;
}) {
  const [title, setTitle] = useState(initial.title);
  const [notes, setNotes] = useState(initial.notes);
  // "" = no due date, which is also what an empty date field reports.
  const [due, setDue] = useState(initial.due_date ?? "");
  // The month grid is open. Closed by default: most cards get Today, Tomorrow
  // or nothing at all, and the grid is a third of the dialog's height.
  const [cal, setCal] = useState(false);
  const [labels, setLabels] = useState<string[]>(initial.labels.map((l) => l.name));
  // Names typed here that no label has yet. They stay offered once unticked,
  // so a click can bring one back.
  const [typed, setTyped] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const [known, setKnown] = useState<LabelSummary[]>([]);
  // A label in use waiting on its inline confirm before it is deleted.
  const [confirming, setConfirming] = useState<LabelSummary | null>(null);
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

  // Deleting a label is not part of Save: it takes the label off every card at
  // once, so one worn anywhere asks first.
  const removeLabel = (name: string) => {
    setError(null);
    api
      .deleteLabel(name)
      .then(() => {
        setConfirming(null);
        setLabels((ls) => ls.filter((l) => !same(l, name)));
        setTyped((ts) => ts.filter((t) => !same(t, name)));
        api.listLabels().then(setKnown).catch(() => setKnown([]));
        onLabelsChanged?.();
      })
      .catch((e) => setError(String(e)));
  };

  const submit = () => {
    if (!title.trim()) return;
    // The grid and the two buttons only ever produce this shape, so anything
    // else was typed into the field, and is better refused here than in a
    // round trip: db::check_due accepts the canonical form and nothing else.
    if (due && !ISO_DAY.test(due)) {
      setError("The due date has to be a date, like 2026-09-18.");
      return;
    }
    setBusy(true);
    setError(null);
    // Whatever is half-typed in the label box counts as meant.
    const all = draft.trim() && !has(draft.trim()) ? [...labels, draft.trim()] : labels;
    save(title.trim(), notes, due || null, all).catch((e) => {
      setError(String(e));
      setBusy(false);
    });
  };

  return (
    <section role="dialog" aria-modal="true" aria-label={heading ?? "Edit task"}>
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
      {/* Typed, not <input type="date">. WebKitGTK answers that one with a
          native GTK popup, and under GNOME/Wayland the popup takes an input
          grab the page never gets back: the calendar could only be escaped
          by alt-tabbing out of the app and back. Esc looked broken for the
          same reason -- the handler below is a window keydown listener, and
          a native popup is nowhere near the page's event system. The grid
          is ours and in flow, which is what the gate already does.
          The buttons are the two days that come up most, one click each. */}
      <div className="flex flex-wrap items-center gap-2">
        <input
          id="task-due"
          className="w-[9rem]"
          value={due}
          placeholder="YYYY-MM-DD"
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
        <button
          disabled={busy}
          aria-pressed={cal}
          className={cal ? "border-accent text-accent" : ""}
          onClick={() => setCal(!cal)}
        >
          Calendar
        </button>
      </div>
      {cal && (
        <Calendar
          value={due}
          onPick={(d) => {
            setDue(d);
            setCal(false);
          }}
        />
      )}

      <label className="mb-1.5 mt-4 block text-[13px] uppercase tracking-[0.06em] text-muted" htmlFor="task-notes">
        Notes
      </label>
      <textarea
        id="task-notes"
        className="min-h-[84px] w-full resize-y rounded-md border border-line bg-bg px-2.5 py-2 text-sm leading-relaxed text-text outline-none transition-colors focus:border-accent"
        rows={3}
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
            const stored = known.find((l) => same(l.name, name));
            return (
              <li key={name.toLowerCase()} className="flex max-w-full items-stretch">
                <button
                  className={
                    "flex min-w-0 items-center gap-1.5 border border-line py-1 pl-1.5 text-xs " +
                    "transition-colors duration-150 " +
                    (stored ? "rounded-l-sm border-r-0 pr-1.5 " : "rounded-sm pr-2.5 ") +
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
                      "h-2 w-2 flex-none rounded-[2px]" +
                      // A name typed here that no label wears yet: no colour until saved.
                      (color ? "" : " border border-muted")
                    }
                    style={color ? { background: color } : undefined}
                  />
                  <span className="overflow-hidden text-ellipsis whitespace-nowrap">{name}</span>
                </button>
                {/* Only a stored label can be deleted; a name typed here and
                    not saved yet just gets unticked. */}
                {stored && (
                  <button
                    className="rounded-r-sm border border-l-0 border-line px-1.5 text-xs text-muted transition-colors hover:text-bad disabled:cursor-default"
                    title={`Delete ${stored.name} from every task`}
                    disabled={busy}
                    onClick={() => (stored.uses ? setConfirming(stored) : removeLabel(stored.name))}
                  >
                    ×
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="text-xs text-muted">No labels yet. Type one below to make it.</p>
      )}
      {confirming && (
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span className="basis-full text-xs text-warn">
            “{confirming.name}” is on {confirming.uses} task{confirming.uses === 1 ? "" : "s"}.
            Deleting it takes it off {confirming.uses === 1 ? "that one" : "all of them"} now, even if you cancel here.
          </span>
          <button disabled={busy} onClick={() => removeLabel(confirming.name)}>
            Delete label
          </button>
          <button disabled={busy} onClick={() => setConfirming(null)}>
            Keep it
          </button>
        </div>
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
  );
}
