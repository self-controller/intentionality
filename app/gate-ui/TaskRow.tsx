import { Fragment } from "react";
import { Check, Play, SlidersHorizontal, StickyNote, Undo2, X } from "lucide-react";
import Checkbox from "../src/ui/Checkbox";
import { IconButton } from "../src/ui/primitives";
import { cn } from "../src/lib/utils";
import { post } from "./bridge";
import { matchParts } from "./search";
import type { LabelOpt, Row } from "./types";

export type Density = "today" | "backlog";

/** One task, in either zone.
 *
 *  The checkbox is the move: ticked, the task is in Today and goes to the
 *  board when you start; unticked, it waits in the backlog under its labels.
 *  The other actions sit quiet until the row is pointed at or focused --
 *  Done (only a task carried from an earlier session has something to
 *  finish), Details, and Delete, which strikes the task through and removes
 *  it for good on Start; Undo takes it back. In Today a row also has Start
 *  in Doing (the play button), which stays lit while it is on, so the task
 *  goes to the board's Doing rather than its To Do. Every word on the row is
 *  Python's except the layout of the detail line. */
export default function TaskRow({
  row,
  labels,
  density,
  open,
  onDetails,
  terms,
  active = false,
}: {
  row: Row;
  /** The labels that exist, for the chips' colours. */
  labels: LabelOpt[];
  density: Density;
  /** This copy of the row has the details panel under it. */
  open: boolean;
  /** Told before a panel opens, so the screen knows which copy asked -- a
   *  backlog task with several labels is listed once per label. */
  onDetails?: () => void;
  /** The search's words, lit up in the title. */
  terms?: string[];
  /** The search's pick: what Enter or Alt+Enter in the search box would move. */
  active?: boolean;
}) {
  const marked = row.mark !== null;
  const today = density === "today";
  const doing = !!row.doing && !marked;
  const lift = marked || open || active;
  const toggleMark = (mark: "done" | "delete") => post("mark", { index: row.index, mark });

  return (
    <li
      data-active={active ? "true" : undefined}
      className={cn(
        "gate-pop group flex items-start gap-[0.9rem] border-b border-line last:border-0",
        today ? "py-[0.8rem]" : "py-[0.6rem]",
        // Pulled out to the list's inset padding so the wash has room around
        // the checkbox, and given back so nothing shifts.
        active && "-mx-[0.4rem] rounded-[0.6rem] border-transparent bg-white/[0.06] px-[0.4rem]",
      )}
    >
      <Checkbox
        size={today ? "2.1em" : "1.6em"}
        checked={row.today}
        onChange={() => post("today", { index: row.index })}
        label={row.today ? `Move ${row.title} back to the backlog` : `Do ${row.title} today`}
        className="mt-[0.1em]"
      />
      <div className="min-w-0 flex-1 pt-[0.05rem]">
        <p
          className={cn(
            "break-words leading-snug transition-colors duration-200",
            today ? "text-[1.08rem] text-text" : "text-[0.95rem] text-muted group-hover:text-text",
            active && "text-text",
            marked && "text-muted line-through",
          )}
          title={row.title}
        >
          {terms && terms.length > 0
            ? matchParts(row.title, terms).map((part, i) =>
                part.hit ? (
                  <mark key={i} className="rounded-[0.2rem] bg-white/15 text-text">
                    {part.text}
                  </mark>
                ) : (
                  <Fragment key={i}>{part.text}</Fragment>
                ),
              )
            : row.title}
        </p>
        <Meta row={row} labels={labels} showLabels={today} />
      </div>
      <div className="flex flex-none items-center gap-[0.1rem]">
        {/* Apart from the other actions so it can stay in view while it is on:
            a child cannot be more opaque than its parent. */}
        {today && !marked && (
          <div
            className={cn(
              "flex flex-none items-center transition-opacity duration-150",
              doing || open ? "opacity-100" : "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100",
            )}
          >
            <IconButton
              size="sm"
              label={doing ? "Start in To Do instead" : "Start in Doing"}
              aria-pressed={doing}
              className={doing ? "bg-good/15 text-good hover:bg-good/20 hover:text-good" : ""}
              onClick={() => post("doing", { index: row.index })}
            >
              <Play className={doing ? "fill-current" : ""} />
            </IconButton>
          </div>
        )}
        <div
          className={cn(
            "flex items-center gap-[0.1rem] transition-opacity duration-150",
            lift ? "opacity-100" : "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100",
          )}
        >
          {row.can_finish && !marked ? (
            <IconButton size="sm" label="Done: finished since last time" onClick={() => toggleMark("done")}>
              <Check />
            </IconButton>
          ) : (
            // Done's room, kept in Today so a lit play button sits in the same
            // place on every row, carried or not.
            today && !marked && <span className="size-[1.8rem] flex-none" aria-hidden="true" />
          )}
          {!marked && (
            <IconButton
              size="sm"
              label={open ? "Close details" : "Details: due date, notes, labels"}
              className={open ? "bg-white/[0.08] text-text" : ""}
              onClick={() => {
                if (open) {
                  post("details_close", { index: row.index });
                } else {
                  onDetails?.();
                  post("details_open", { index: row.index });
                }
              }}
            >
              <SlidersHorizontal />
            </IconButton>
          )}
          {row.mark === "delete" ? (
            <IconButton size="sm" label="Undo: keep this task" onClick={() => toggleMark("delete")}>
              <Undo2 />
            </IconButton>
          ) : row.mark === "done" ? (
            <IconButton size="sm" label="Undo: not done after all" onClick={() => toggleMark("done")}>
              <Undo2 />
            </IconButton>
          ) : (
            <IconButton
              size="sm"
              tone="danger"
              label={row.typed ? "Remove" : "Delete for good when you start"}
              onClick={() => toggleMark("delete")}
            >
              <X />
            </IconButton>
          )}
        </div>
      </div>
    </li>
  );
}

/** The due words are Python's (ui.due_label): "overdue, was due Sep 18",
 *  "due today", "due tomorrow", "due Friday", "due Nov 3". The colour only
 *  reinforces what the words already say. */
function dueTone(due: string): string {
  if (due.startsWith("overdue")) return "text-bad";
  if (due === "due today") return "text-warn";
  return "";
}

function colorOf(labels: LabelOpt[], name: string): string | null {
  return labels.find((l) => l.name.toLowerCase() === name.toLowerCase())?.color ?? null;
}

export function LabelChip({ name, color }: { name: string; color: string | null }) {
  return (
    <span className="inline-flex max-w-[16rem] items-center gap-[0.35rem] rounded-full border border-line px-[0.5rem] py-[0.05rem] text-[0.7rem] text-muted">
      <span
        className={cn("size-[0.45rem] flex-none rounded-full", !color && "border border-muted")}
        style={color ? { background: color } : undefined}
      />
      <span className="truncate">{name}</span>
    </span>
  );
}

/** The line under a title: the label chips (in Today only -- in the backlog
 *  the section already names the label), the due date, a notes mark, "new". A
 *  marked row shows the mark's word instead, as Python wrote it. */
function Meta({ row, labels, showLabels }: { row: Row; labels: LabelOpt[]; showLabels: boolean }) {
  if (row.mark) {
    return <p className="mt-[0.2rem] text-[0.74rem] text-muted">{row.detail}</p>;
  }
  const due = row.due ?? "";
  const items: React.ReactNode[] = [];
  if (row.doing) {
    items.push(
      <span key="doing" className="inline-flex items-center gap-[0.35rem] text-good">
        <span className="size-[0.45rem] flex-none rounded-full bg-good" aria-hidden="true" />
        Doing
      </span>,
    );
  }
  if (showLabels) {
    for (const name of row.labels) {
      items.push(<LabelChip key={`l:${name}`} name={name} color={colorOf(labels, name)} />);
    }
  }
  if (due) {
    items.push(
      <span key="due" className={dueTone(due)}>
        {due}
      </span>,
    );
  }
  if (row.notes) {
    items.push(
      <span key="notes" className="inline-flex items-center gap-[0.25rem]">
        <StickyNote className="size-[0.85em]" aria-hidden="true" />
        notes
      </span>,
    );
  }
  if (row.typed) {
    items.push(
      <span key="new" className="text-muted/80">
        new
      </span>,
    );
  }
  if (items.length === 0) return null;
  return (
    <p className="mt-[0.3rem] flex flex-wrap items-center gap-x-[0.55rem] gap-y-[0.25rem] text-[0.74rem] text-muted">
      {items.map((item, i) => (
        <Fragment key={i}>
          {i > 0 && <span className="text-muted/40">·</span>}
          {item}
        </Fragment>
      ))}
    </p>
  );
}
