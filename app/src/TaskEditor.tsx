import { type KeyboardEvent as ReactKeyboardEvent, useEffect, useRef, useState } from "react";
import { CalendarDays, Check, List, Trash2, X } from "lucide-react";
import ColorPicker from "@/components/ColorPicker";
import MonthCalendar from "@/components/MonthCalendar";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import * as api from "./api";
import { addDays, ISO_DAY, localDate } from "./format";
import type { Label, LabelSummary, Priority } from "./types";
import { matches } from "./keybinds";
import { keymap } from "./useKeybinds";

// What the editor starts from: a Task has all of these, and a card that does
// not exist yet is the same shape with nothing filled in.
export interface TaskFields {
  title: string;
  notes: string;
  due_date: string | null;
  priority: Priority;
  labels: Label[];
}

export const blankTask = (title: string, label: Label | null = null): TaskFields => ({
  title,
  notes: "",
  due_date: null,
  priority: "medium",
  labels: label ? [label] : [],
});

// High first, as the board orders them. The dot's colour matches the card's
// footer.
export const PRIORITIES: { value: Priority; label: string; dot: string }[] = [
  { value: "high", label: "High", dot: "bg-destructive" },
  { value: "medium", label: "Medium", dot: "bg-warn" },
  { value: "low", label: "Low", dot: "bg-muted-foreground" },
];

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

// Save is Ctrl+Enter unless Settings says otherwise.
const isSave = (e: KeyboardEvent | ReactKeyboardEvent) => matches(e, keymap()["app.saveTask"]);

/**
 * The whole card, editable — an existing one, or a new one before it exists.
 * Just the fields: the board lays it over the card it has opened in front of
 * you (CardEditor3D), rather than expanding the card in place, which would
 * make the lane jump under the cursor you were about to drag with.
 *
 * Everything is saved in one call, so Cancel really discards. Ctrl+Enter
 * saves from anywhere in it.
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
  // The same, for the key handler: state set in this event isn't read until
  // the next render, and a second Ctrl+Enter must not save twice.
  const busyRef = useRef(false);
  // The Delete button was pressed and is waiting on its inline confirm.
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.listLabels().then(setKnown).catch(() => setKnown([]));
    titleRef.current?.focus();
  }, []);

  const startBusy = (on: boolean) => {
    busyRef.current = on;
    setBusy(on);
  };

  const submit = () => {
    if (busyRef.current || !title.trim()) return;
    // The grid and the two buttons only ever produce this shape, so anything
    // else was typed into the field, and is better refused here than in a
    // round trip: db::check_due accepts the canonical form and nothing else.
    if (due && !ISO_DAY.test(due)) {
      setError("The due date has to be a date, like 2026-09-18.");
      return;
    }
    startBusy(true);
    setError(null);
    // Whatever is half-typed in the label box counts as meant.
    const all = draft.trim() && !has(draft.trim()) ? [...labels, draft.trim()] : labels;
    save(title.trim(), notes, due || null, priority, all).catch((e) => {
      setError(String(e));
      startBusy(false);
    });
  };
  const submitRef = useRef(submit);
  submitRef.current = submit;

  // Ctrl+Enter saves from any field. Esc cancels, like every other overlay in
  // the app -- the priority menu or the colour picker first, if one is open,
  // then the card. The menu handles its own Esc and marks it handled.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      if (isSave(e)) {
        e.preventDefault();
        submitRef.current();
      } else if (e.key === "Escape") {
        if (picking) setPicking(null);
        else onClose();
      }
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

  return (
    <FieldSet role="dialog" aria-modal="true" aria-label={heading ?? "Edit task"} disabled={busy}>
      <FieldLegend>{heading ?? "Edit task"}</FieldLegend>
      <FieldDescription>Saved together when you press Save · Ctrl+Enter saves · Esc cancels</FieldDescription>
      <FieldGroup className="gap-5">
        <Field>
          <FieldLabel htmlFor="task-title">Title</FieldLabel>
          <Input
            id="task-title"
            ref={titleRef}
            className="h-10 text-base font-medium tracking-[-0.01em]"
            value={title}
            placeholder="What needs doing"
            onChange={(e) => setTitle(e.target.value)}
            // Plain Enter saves too; Ctrl+Enter is the window's.
            onKeyDown={(e) => e.key === "Enter" && !isSave(e) && submit()}
          />
        </Field>

        <div className="grid grid-cols-2 gap-4">
          {/* The board orders every column by this, high first. */}
          <Field>
            <FieldLabel htmlFor="task-priority">Priority</FieldLabel>
            <Select value={priority} onValueChange={(v) => setPriority(v as Priority)}>
              <SelectTrigger id="task-priority" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {PRIORITIES.map((p) => (
                  <SelectItem key={p.value} value={p.value}>
                    <span className={cn("size-1.5 rounded-full", p.dot)} />
                    {p.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>

          {/* Typed, not <input type="date">. WebKitGTK answers that one with a
              native GTK popup, and under GNOME/Wayland the popup takes an input
              grab the page never gets back: the calendar could only be escaped
              by alt-tabbing out of the app and back. Esc looked broken for the
              same reason -- the handler above is a window keydown listener, and
              a native popup is nowhere near the page's event system. The grid
              is ours and in flow, which is what the gate already does.
              The buttons are the two days that come up most, one click each. */}
          <Field>
            <FieldLabel htmlFor="task-due">Due date</FieldLabel>
            <div className="relative">
              <Input
                id="task-due"
                className="pe-9 font-mono"
                value={due}
                placeholder="YYYY-MM-DD"
                onChange={(e) => setDue(e.target.value)}
              />
              {due && (
                <button
                  type="button"
                  className="absolute end-2 top-1/2 flex size-5 -translate-y-1/2 items-center justify-center rounded text-muted-foreground transition-colors hover:text-foreground"
                  title="No due date"
                  aria-label="Clear the due date"
                  onClick={() => setDue("")}
                >
                  <X className="size-3.5" />
                </button>
              )}
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button variant="outline" size="sm" onClick={() => setDue(localDate(new Date()))}>
                Today
              </Button>
              <Button variant="outline" size="sm" onClick={() => setDue(localDate(addDays(new Date(), 1)))}>
                Tomorrow
              </Button>
              <Button variant="outline" size="sm" selected={cal} aria-pressed={cal} onClick={() => setCal(!cal)}>
                <CalendarDays />
                Calendar
              </Button>
            </div>
          </Field>
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

        <NotesField value={notes} disabled={busy} onChange={setNotes} />

        <Field>
          <FieldLabel htmlFor="task-label-input">Labels</FieldLabel>
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
                        type="button"
                        className={cn(
                          "flex flex-none items-center rounded-s-full border border-e-0 border-border ps-2.5 pe-2 " +
                            "transition-colors hover:bg-accent disabled:cursor-default",
                          picking && same(picking.name, name) && "bg-accent",
                        )}
                        title={`Change the colour of ${stored.name}`}
                        aria-label={`Change the colour of ${stored.name}`}
                        onClick={() => {
                          setConfirming(null);
                          setPicking(picking && same(picking.name, name) ? null : stored);
                        }}
                      >
                        <span className="size-2.5 rounded-full" style={{ background: stored.color }} />
                      </button>
                    )}
                    <button
                      type="button"
                      className={cn(
                        "flex min-w-0 items-center gap-1.5 border border-border py-1 ps-2 text-xs " +
                          "transition-colors duration-150 disabled:cursor-default",
                        stored ? "border-e-0 pe-2" : "rounded-full pe-2.5",
                        // The tick is what says it is on; the fill only reinforces it.
                        on ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground",
                      )}
                      aria-pressed={on}
                      title={on ? `Take ${name} off` : `Tag with ${name}`}
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
                        type="button"
                        className="flex items-center rounded-e-full border border-s-0 border-border ps-1.5 pe-2 text-muted-foreground transition-colors hover:text-destructive disabled:cursor-default"
                        title={`Delete ${stored.name} from every task`}
                        aria-label={`Delete ${stored.name} from every task`}
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
          ) : null}
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
            <div className="flex flex-wrap items-center gap-2">
              <span className="basis-full text-sm text-warn">
                “{confirming.name}” is on {confirming.uses} task{confirming.uses === 1 ? "" : "s"}.
                Deleting it takes it off {confirming.uses === 1 ? "that one" : "all of them"} now, even if you cancel here.
              </span>
              <Button variant="destructive" size="sm" onClick={() => removeLabel(confirming.name)}>
                Delete label
              </Button>
              <Button variant="outline" size="sm" onClick={() => setConfirming(null)}>
                Keep it
              </Button>
            </div>
          )}
          <Input
            id="task-label-input"
            value={draft}
            placeholder="New label…"
            onChange={(e) => {
              // A comma commits, so pasting "a, b" works as typing it does.
              if (e.target.value.endsWith(",")) addLabel(e.target.value);
              else setDraft(e.target.value);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !isSave(e)) {
                e.preventDefault();
                addLabel(draft);
              } else if (e.key === "Backspace" && !draft && labels.length) {
                setLabels(labels.slice(0, -1));
              }
            }}
          />
          <FieldDescription>
            {offered.length ? "Click one to tag this card. Enter or a comma adds a new one." : "No labels yet. Type one to make it."}
          </FieldDescription>
        </Field>
      </FieldGroup>

      <FieldError>{error}</FieldError>

      <div className="flex items-center gap-2">
        <Button disabled={!title.trim()} onClick={submit}>
          {saveText}
        </Button>
        <Button variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <span className="flex-1" />
        {onDelete && !deleting && (
          <Button variant="destructive" onClick={() => setDeleting(true)}>
            <Trash2 />
            Delete
          </Button>
        )}
      </div>
      {onDelete && deleting && (
        <div className="-mt-3 flex flex-wrap items-center justify-end gap-2">
          <span className="basis-full text-right text-sm text-warn">
            Delete this task for good? It can't be brought back.
          </span>
          <Button
            variant="destructive"
            size="sm"
            onClick={() => {
              startBusy(true);
              setError(null);
              onDelete().catch((e) => {
                setError(String(e));
                startBusy(false);
                setDeleting(false);
              });
            }}
          >
            Delete for good
          </Button>
          <Button variant="outline" size="sm" onClick={() => setDeleting(false)}>
            Keep it
          </Button>
        </div>
      )}
    </FieldSet>
  );
}

// A bullet is a line starting "• " after its indent. Notes stay plain text, so
// this is all a list is: the gate and the analysis prompt read it as it is.
const BULLET_LINE = /^(\s*)• (.*)$/;
const INDENT = "  ";

// [start, end) of the line `pos` sits on.
function lineAt(text: string, pos: number): [number, number] {
  const start = text.lastIndexOf("\n", pos - 1) + 1;
  const end = text.indexOf("\n", pos);
  return [start, end < 0 ? text.length : end];
}

// The whole lines a selection touches. A selection ending just after a
// newline doesn't take the line below it.
function linesOf(text: string, from: number, to: number): [number, number] {
  const last = to > from && text[to - 1] === "\n" ? to - 1 : to;
  return [lineAt(text, from)[0], lineAt(text, last)[1]];
}

/**
 * The notes box, with bullets: a toolbar toggle for the selected lines, and
 * the keys a list editor has -- Enter continues a list (and ends it on an
 * empty bullet), "- " or "* " at the start of a line becomes a bullet, Tab
 * and Shift+Tab indent and outdent one.
 */
function NotesField({
  value,
  disabled,
  onChange,
}: {
  value: string;
  disabled: boolean;
  onChange: (notes: string) => void;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);

  // [from, to) becomes `text`, then the selection is set. Through
  // execCommand where the webview has it, so Ctrl+Z undoes a bullet like any
  // typing; its input event reaches onChange the usual way.
  const replace = (from: number, to: number, text: string, selStart: number, selEnd = selStart) => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(from, to);
    if (!document.execCommand("insertText", false, text)) {
      el.setRangeText(text, from, to);
      onChange(el.value);
    }
    el.setSelectionRange(selStart, selEnd);
  };

  // All the touched lines bulleted already: unbullet them. Otherwise bullet
  // every one that isn't, leaving blank lines in a longer selection alone.
  const toggleBullets = () => {
    const el = ref.current;
    if (!el) return;
    const { selectionStart: s0, selectionEnd: s1 } = el;
    const [from, to] = linesOf(value, s0, s1);
    const lines = value.slice(from, to).split("\n");
    const off = lines.every((l) => BULLET_LINE.test(l) || (lines.length > 1 && !l.trim()));
    const next = lines.map((l) => {
      const m = l.match(BULLET_LINE);
      if (off) return m ? m[1] + m[2] : l;
      if (m || (lines.length > 1 && !l.trim())) return l;
      const indent = l.match(/^\s*/)![0];
      return indent + "• " + l.slice(indent.length);
    });
    const block = next.join("\n");
    if (s0 === s1) {
      // The caret keeps its place in the text it was in.
      const caret = Math.max(from, s0 + block.length - (to - from));
      replace(from, to, block, caret);
    } else {
      replace(from, to, block, from, from + block.length);
    }
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing || e.ctrlKey || e.metaKey || e.altKey) return;
    const el = e.currentTarget;
    const { selectionStart: s0, selectionEnd: s1 } = el;
    const [start, end] = lineAt(value, s0);
    const line = value.slice(start, end);
    const m = line.match(BULLET_LINE);

    if (e.key === "Enter" && !e.shiftKey && s0 === s1 && m) {
      // In the indent, before the bullet: an ordinary newline.
      if (s0 - start < m[1].length + 2) return;
      e.preventDefault();
      if (!m[2].trim()) {
        // An empty bullet ends the list -- one level at a time if indented.
        const ended = m[1].length >= INDENT.length ? m[1].slice(INDENT.length) + "• " : "";
        replace(start, end, ended, start + ended.length);
      } else {
        const lead = "\n" + m[1] + "• ";
        replace(s0, s1, lead, s0 + lead.length);
      }
      return;
    }

    if (e.key === " " && s0 === s1 && /^\s*[-*]$/.test(value.slice(start, s0))) {
      e.preventDefault();
      replace(s0 - 1, s0, "• ", s0 + 1);
      return;
    }

    if (e.key === "Tab") {
      const [from, to] = linesOf(value, s0, s1);
      const lines = value.slice(from, to).split("\n");
      // Off a list, Tab moves focus as it always has.
      if (!lines.some((l) => BULLET_LINE.test(l))) return;
      e.preventDefault();
      let first = 0; // how far the caret's own line moved
      const next = lines.map((l, i) => {
        if (!BULLET_LINE.test(l)) return l;
        const moved = e.shiftKey ? l.replace(/^ {1,2}/, "") : INDENT + l;
        if (i === 0) first = moved.length - l.length;
        return moved;
      });
      const block = next.join("\n");
      if (s0 === s1) replace(from, to, block, Math.max(from, s0 + first));
      else replace(from, to, block, from, from + block.length);
    }
  };

  return (
    <Field>
      <div className="flex items-center justify-between">
        <FieldLabel htmlFor="task-notes">Notes</FieldLabel>
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled}
          title="Bulleted list (or start a line with “- ”)"
          // Keeps the selection the toggle is about.
          onMouseDown={(e) => e.preventDefault()}
          onClick={toggleBullets}
        >
          <List />
          Bullets
        </Button>
      </div>
      <Textarea
        ref={ref}
        id="task-notes"
        className="min-h-[84px] resize-y leading-relaxed"
        rows={3}
        value={value}
        placeholder="Anything you want to remember about this one…"
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
      />
    </Field>
  );
}
