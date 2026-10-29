import Checkbox from "../src/ui/Checkbox";
import { Button } from "../src/ui/primitives";
import { post } from "./bridge";
import type { Row } from "./types";

/** One task on the welcome screen.
 *
 *  The checkbox says where the task goes: ticked means it is on today's
 *  board (To Do) when you start, unticked means it stays in the backlog.
 *  Delete is its own button and means what it says -- the task is struck
 *  through and removed for good when you start; pressing it again is the
 *  undo. "Done" is separate again: it resolves the carried original. */
export default function TaskRow({
  row,
  open,
  onDetails,
}: {
  row: Row;
  /** This copy of the row has the details panel under it. */
  open: boolean;
  /** Told before a panel opens, so the screen knows which copy asked -- a
   *  task with several labels is listed once per label. */
  onDetails?: () => void;
}) {
  const marked = row.mark !== null;

  return (
    <li className="flex items-start gap-[0.75em] py-[0.45em]">
      <Checkbox
        size="2.1em"
        checked={row.today}
        onChange={() => post("today", { index: row.index })}
        label={row.today ? `Move ${row.title} back to the backlog` : `Do ${row.title} today`}
      />
      <div className="min-w-0 flex-1 pt-[0.25em]">
        <p
          className={
            "line-clamp-2 break-words text-[1.05em] leading-snug transition-colors duration-200 " +
            (marked ? "text-muted line-through" : row.today ? "text-text" : "text-muted")
          }
          title={row.title}
        >
          {row.title}
        </p>
        {row.detail && (
          <p className="mt-[0.15em] truncate text-[0.72em] text-muted">
            {row.detail}
          </p>
        )}
      </div>
      <div className="flex flex-none items-center gap-[0.4em] pt-[0.2em] text-[0.9em]">
        {row.can_finish && !marked && (
          <Button onClick={() => post("mark", { index: row.index, mark: "done" })}>
            Done
          </Button>
        )}
        <Button
          onClick={() => {
            if (open) {
              post("details_close", { index: row.index });
            } else {
              onDetails?.();
              post("details_open", { index: row.index });
            }
          }}
          className={open ? "!border-accent !text-accent" : ""}
        >
          Details
        </Button>
        <Button
          onClick={() => post("mark", { index: row.index, mark: "delete" })}
          className={row.mark === "delete" ? "!border-bad !text-bad" : "hover:!text-bad"}
        >
          {row.mark === "delete" ? "Undo" : "Delete"}
        </Button>
      </div>
    </li>
  );
}
