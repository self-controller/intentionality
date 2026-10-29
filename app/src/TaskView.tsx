import { useEffect } from "react";
import { Pencil } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { dueInfo } from "./format";
import { PRIORITIES } from "./TaskEditor";
import type { Task } from "./types";

// "- " and "* " count too, so notes typed before the editor had bullets read
// as the lists they were meant to be.
const BULLET = /^(\s*)[•*-] (.*)$/;

type Block = { kind: "text"; text: string } | { kind: "list"; items: { depth: number; text: string }[] };

function blocksOf(notes: string): Block[] {
  const blocks: Block[] = [];
  for (const line of notes.split("\n")) {
    const m = line.match(BULLET);
    const last = blocks[blocks.length - 1];
    if (m) {
      const item = { depth: Math.floor(m[1].replace(/\t/g, "  ").length / 2), text: m[2] };
      if (last?.kind === "list") last.items.push(item);
      else blocks.push({ kind: "list", items: [item] });
    } else if (last?.kind === "text") {
      last.text += "\n" + line;
    } else {
      blocks.push({ kind: "text", text: line });
    }
  }
  return blocks;
}

/** A card's notes as written, with bullet lines drawn as a list. */
function NotesView({ notes }: { notes: string }) {
  return (
    <div className="space-y-1.5 text-sm leading-relaxed">
      {blocksOf(notes).map((b, i) =>
        b.kind === "text" ? (
          <p key={i} className="whitespace-pre-wrap break-words">
            {b.text}
          </p>
        ) : (
          <ul key={i} className="list-disc space-y-0.5 ps-5">
            {b.items.map((item, j) => (
              <li
                key={j}
                className={cn("break-words", item.depth % 2 === 1 && "list-[circle]")}
                style={{ marginInlineStart: `${item.depth * 1.25}rem` }}
              >
                {item.text}
              </li>
            ))}
          </ul>
        ),
      )}
    </div>
  );
}

/**
 * A card's face when it is clicked: everything on it, read-only, with Edit to
 * turn it into the form. Reading a card no longer means being in an editor
 * where a stray Esc throws the visit away.
 */
export function TaskView({
  task,
  today,
  onEdit,
  onClose,
}: {
  task: Task;
  // "YYYY-MM-DD", the board's own idea of today.
  today: string;
  onEdit: () => void;
  onClose: () => void;
}) {
  // Esc closes, as everywhere; E edits.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "Escape") onClose();
      else if (e.key === "e" || e.key === "E") {
        e.preventDefault();
        onEdit();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, onEdit]);

  const priority = PRIORITIES.find((p) => p.value === task.priority)!;
  const [y, m, d] = today.split("-").map(Number);
  const due = task.due_date ? dueInfo(task.due_date, new Date(y, m - 1, d)) : null;
  const done = task.status === "done";

  return (
    <section role="dialog" aria-modal="true" aria-label={task.title}>
      {/* The form's type: its legend, its labels, its descriptions. */}
      <h2 className="text-base font-medium tracking-[-0.01em] break-words">{task.title}</h2>

      <div className="mt-1.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-muted-foreground">
        <span className="inline-flex items-center gap-1.5">
          <span className={cn("size-1.5 rounded-full", priority.dot)} />
          {priority.label} priority
        </span>
        {due && (
          <span
            className={cn(
              !done && due.cls === "overdue" && "text-destructive",
              !done && due.cls === "today" && "text-warn",
            )}
          >
            {done ? `Due ${due.date}` : due.text}
          </span>
        )}
        {task.carry_count > 0 && (
          <span>
            Carried {task.carry_count} time{task.carry_count === 1 ? "" : "s"}
          </span>
        )}
      </div>

      {task.labels.length > 0 && (
        <ul className="mt-3 flex flex-wrap gap-1.5">
          {task.labels.map((l) => (
            <li
              key={l.name.toLowerCase()}
              className="flex items-center gap-1.5 rounded-full border border-border px-2.5 py-0.5 text-xs"
            >
              <span className="size-2 flex-none rounded-full" style={{ background: l.color }} />
              {l.name}
            </li>
          ))}
        </ul>
      )}

      <p className="mb-3 mt-6 text-sm font-medium leading-snug">Notes</p>
      {task.notes.trim() ? (
        <NotesView notes={task.notes} />
      ) : (
        <p className="text-sm text-muted-foreground">No notes.</p>
      )}

      <div className="mt-7 flex items-center gap-2">
        <Button onClick={onEdit}>
          <Pencil />
          Edit
        </Button>
        <Button variant="outline" onClick={onClose}>
          Close
        </Button>
      </div>
    </section>
  );
}
