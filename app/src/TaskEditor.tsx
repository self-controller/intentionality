import { useEffect, useRef, useState } from "react";
import { CalendarDays, Check, Trash2, X } from "lucide-react";
import ColorPicker from "@/components/ColorPicker";
import MonthCalendar from "@/components/MonthCalendar";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import * as api from "./api";
import { addDays, ISO_DAY, localDate } from "./format";
import type { Label, LabelSummary, Priority } from "./types";

// What the editor starts from: a Task has all of these, and a card that does
// not exist yet is the same shape with nothing filled in.
export interface TaskFields {
  title: string;
  notes: string;
  due_date: string | null;
  priority: Priority;
  labels: Label[];
}

export const blankTask = (title: string): TaskFields => ({
  title,
  notes: "",
  due_date: null,
  priority: "medium",
  labels: [],
});

// High first, as the board orders them. The colour matches the card's footer;
// `on` is the chosen segment's text and the bar along its bottom, as the
// active tab has.
const PRIORITIES: { value: Priority; label: string; dot: string; on: string }[] = [
  { value: "high", label: "High", dot: "bg-destructive", on: "text-destructive after:bg-destructive" },
  { value: "medium", label: "Medium", dot: "bg-warn", on: "text-warn after:bg-warn" },
  { value: "low", label: "Low", dot: "bg-muted-foreground", on: "text-foreground after:bg-muted-foreground" },
];

// A field's caption: the app's mono eyebrow.
const FIELD = "eyebrow mb-2 mt-5 block";

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
  save: (
    title: string,
    notes: string,
    dueDate: string | null,
    priority: Priority,
    labels: string[],
  ) => Promise<unknown>;
  onClose: () => void;
  // Removes the task for good, after an inline confirm. Null for a card that
  // doesn't exist yet.
  onDelete: (() => Promise<unknown>) | null;
  // A label was deleted or recoloured outright, so other cards on screen
  // changed too.
  onLabelsChanged?: () => void;
}) {
  const [title, setTitle] = useState(initial.title);
  const [notes, setNotes] = useState(initial.notes);
  // "" = no due date, which is also what an empty date field reports.
  const [due, setDue] = useState(initial.due_date ?? "");
  // The month grid is open. Closed by default: most cards get Today, Tomorrow
  // or nothing at all, and the grid is a third of the dialog's height.
  const [cal, setCal] = useState(false);
  const [priority, setPriority] = useState<Priority>(initial.priority);
  const [labels, setLabels] = useState<string[]>(initial.labels.map((l) => l.name));
  // Names typed here that no label has yet. They stay offered once unticked,
  // so a click can bring one back.
  const [typed, setTyped] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const [known, setKnown] = useState<LabelSummary[]>([]);
  // A label in use waiting on its inline confirm before it is deleted.
  const [confirming, setConfirming] = useState<LabelSummary | null>(null);
  // A stored label whose colour is being picked. Like deleting, recolouring
  // is not part of Save: the colour is the label's, on every card at once.
  const [picking, setPicking] = useState<LabelSummary | null>(null);
  const [busy, setBusy] = useState(false);
  // The Delete button was pressed and is waiting on its inline confirm.
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.listLabels().then(setKnown).catch(() => setKnown([]));
    titleRef.current?.focus();
  }, []);

  // Esc cancels, like every other overlay in the app -- the colour picker
  // first, if one is open, then the card.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (picking) setPicking(null);
      else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, picking]);

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

  const recolor = (name: string, color: string) => {
    setError(null);
    api
      .setLabelColor(name, color)
      .then(() => {
        setPicking(null);
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
    save(title.trim(), notes, due || null, priority, all).catch((e) => {
      setError(String(e));
      setBusy(false);
    });
  };

  return (
    <section role="dialog" aria-modal="true" aria-label={heading ?? "Edit task"}>
      {heading && <p className="eyebrow mb-3">{heading}</p>}
      <input
        ref={titleRef}
        className="h-10 text-base font-medium tracking-[-0.01em]"
        value={title}
        placeholder="Title"
        disabled={busy}
        onChange={(e) => setTitle(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && submit()}
      />

      <p className={FIELD} id="task-priority">
        Priority
      </p>
      {/* The board orders every column by this, high first. Joined segments
          in the tab style; the chosen one carries its colour. */}
      <div className="flex" role="radiogroup" aria-labelledby="task-priority">
        {PRIORITIES.map((p) => {
          const on = priority === p.value;
          return (
            <button
              key={p.value}
              role="radio"
              aria-checked={on}
              disabled={busy}
              className={cn(
                "relative -ms-px inline-flex items-center gap-1.5 overflow-hidden border border-border px-3 py-1.5 " +
                  "text-[13px] font-medium text-muted-foreground transition-colors first:ms-0 first:rounded-s-md " +
                  "last:rounded-e-md hover:text-foreground disabled:cursor-default " +
                  "after:pointer-events-none after:absolute after:inset-x-0 after:bottom-0 after:h-0.5",
                on && "bg-muted " + p.on,
              )}
              onClick={() => setPriority(p.value)}
            >
              <span className={cn("size-1.5 rounded-full", p.dot)} />
              {p.label}
            </button>
          );
        })}
      </div>

      <label className={FIELD} htmlFor="task-due">
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
          className="w-[9rem] font-mono"
          value={due}
          placeholder="YYYY-MM-DD"
          disabled={busy}
          onChange={(e) => setDue(e.target.value)}
        />
        <Button variant="outline" size="sm" disabled={busy} onClick={() => setDue(localDate(new Date()))}>
          Today
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          onClick={() => setDue(localDate(addDays(new Date(), 1)))}
        >
          Tomorrow
        </Button>
        {due && (
          <Button variant="ghost" size="sm" disabled={busy} onClick={() => setDue("")}>
            Clear
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          selected={cal}
          aria-pressed={cal}
          onClick={() => setCal(!cal)}
        >
          <CalendarDays />
          Calendar
        </Button>
      </div>
      {cal && (
        <MonthCalendar
          value={due}
          onPick={(d) => {
            setDue(d);
            setCal(false);
          }}
        />
      )}

      <label className={FIELD} htmlFor="task-notes">
        Notes
      </label>
      <textarea
        id="task-notes"
        className="min-h-[84px] resize-y leading-relaxed"
        rows={3}
        value={notes}
        placeholder="Anything you want to remember about this one…"
        disabled={busy}
        onChange={(e) => setNotes(e.target.value)}
      />

      <label className={FIELD} htmlFor="task-label-input">
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
                {/* A stored label's colour is a button of its own: it opens
                    the picker. A name typed here has no colour until saved. */}
                {stored && (
                  <button
                    className={cn(
                      "flex flex-none items-center rounded-s-md border border-e-0 border-border px-2 " +
                        "transition-colors hover:bg-accent disabled:cursor-default",
                      picking && same(picking.name, name) && "bg-accent",
                    )}
                    title={`Change the colour of ${stored.name}`}
                    aria-label={`Change the colour of ${stored.name}`}
                    disabled={busy}
                    onClick={() => {
                      setConfirming(null);
                      setPicking(picking && same(picking.name, name) ? null : stored);
                    }}
                  >
                    <span className="size-2.5 rounded-full" style={{ background: stored.color }} />
                  </button>
                )}
                <button
                  className={cn(
                    "flex min-w-0 items-center gap-1.5 border border-border py-1 ps-2 text-xs " +
                      "transition-colors duration-150 disabled:cursor-default",
                    stored ? "border-e-0 pe-2" : "rounded-md pe-2.5",
                    // The tick is what says it is on; the fill only reinforces it.
                    on ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground",
                  )}
                  aria-pressed={on}
                  title={on ? `Take ${name} off` : `Tag with ${name}`}
                  disabled={busy}
                  onClick={() => toggle(name)}
                >
                  <span className="flex w-3 flex-none justify-center">{on && <Check className="size-3" />}</span>
                  {!stored && (
                    <span
                      className={cn(
                        "size-2 flex-none rounded-full",
                        // A name typed here that no label wears yet: no colour until saved.
                        !color && "border border-muted-foreground",
                      )}
                      style={color ? { background: color } : undefined}
                    />
                  )}
                  <span className="overflow-hidden text-ellipsis whitespace-nowrap">{name}</span>
                </button>
                {/* Only a stored label can be deleted; a name typed here and
                    not saved yet just gets unticked. */}
                {stored && (
                  <button
                    className="flex items-center rounded-e-md border border-s-0 border-border px-1.5 text-muted-foreground transition-colors hover:text-destructive disabled:cursor-default"
                    title={`Delete ${stored.name} from every task`}
                    aria-label={`Delete ${stored.name} from every task`}
                    disabled={busy}
                    onClick={() => {
                      setPicking(null);
                      if (stored.uses) setConfirming(stored);
                      else removeLabel(stored.name);
                    }}
                  >
                    <X className="size-3" />
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="text-xs text-muted-foreground">No labels yet. Type one below to make it.</p>
      )}
      {picking && (
        <ColorPicker
          // Remounted per label, so it starts from that label's colour.
          key={picking.name.toLowerCase()}
          title={`Colour of “${picking.name}”, on every card that wears it`}
          value={picking.color}
          busy={busy}
          onApply={(hex) => recolor(picking.name, hex)}
          onCancel={() => setPicking(null)}
        />
      )}
      {confirming && (
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span className="basis-full text-xs text-warn">
            “{confirming.name}” is on {confirming.uses} task{confirming.uses === 1 ? "" : "s"}.
            Deleting it takes it off {confirming.uses === 1 ? "that one" : "all of them"} now, even if you cancel here.
          </span>
          <Button variant="destructive" size="sm" disabled={busy} onClick={() => removeLabel(confirming.name)}>
            Delete label
          </Button>
          <Button variant="outline" size="sm" disabled={busy} onClick={() => setConfirming(null)}>
            Keep it
          </Button>
        </div>
      )}
      <input
        id="task-label-input"
        className="mt-2"
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

      {error && <p className="mt-3 text-xs text-destructive">{error}</p>}

      <div className="mt-6 flex items-center gap-2">
        <Button disabled={busy || !title.trim()} onClick={submit}>
          {saveText}
        </Button>
        <Button variant="outline" disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        <span className="flex-1" />
        {onDelete && !deleting && (
          <Button variant="destructive" appearance="ghost" disabled={busy} onClick={() => setDeleting(true)}>
            <Trash2 />
            Delete
          </Button>
        )}
      </div>
      {onDelete && deleting && (
        <div className="mt-2 flex flex-wrap items-center justify-end gap-1.5">
          <span className="basis-full text-right text-xs text-warn">
            Delete this task for good? It can't be brought back.
          </span>
          <Button
            variant="destructive"
            size="sm"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              setError(null);
              onDelete().catch((e) => {
                setError(String(e));
                setBusy(false);
                setDeleting(false);
              });
            }}
          >
            Delete for good
          </Button>
          <Button variant="outline" size="sm" disabled={busy} onClick={() => setDeleting(false)}>
            Keep it
          </Button>
        </div>
      )}
    </section>
  );
}
