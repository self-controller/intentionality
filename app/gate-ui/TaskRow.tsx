import Checkbox from "../src/ui/Checkbox";
import { Button } from "../src/ui/primitives";
import { post } from "./bridge";
import type { Row } from "./types";

/** One task on the welcome screen.
 *
 *  The checkbox is the keep/drop control: ticked means this task comes with
 *  you into the session, unticked means it is struck through and deleted when
 *  you start. That replaces the old Delete/Remove/Undo buttons entirely --
 *  unticking is the delete and reticking is the undo. "Done" stays a separate
 *  button because it means something else: the carried original is resolved. */
export default function TaskRow({ row, open }: { row: Row; open: boolean }) {
  const marked = row.mark !== null;

  const setChecked = (next: boolean) => {
    // toggle() undoes a mark when it is sent the mark the row already has.
    post("mark", { index: row.index, mark: next ? row.mark : "delete" });
  };

  return (
    <li className="flex items-start gap-[0.75em] py-[0.45em]">
      <Checkbox
        size="2.1em"
        checked={!marked}
        onChange={setChecked}
        label={marked ? `Keep ${row.title}` : `Drop ${row.title}`}
      />
      <div className="min-w-0 flex-1 pt-[0.25em]">
        <p
          className={
            "truncate text-[1.05em] leading-snug transition-colors duration-200 " +
            (marked ? "text-muted line-through" : "text-text")
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
          onClick={() => post(open ? "details_close" : "details_open", { index: row.index })}
          className={open ? "!border-accent !text-accent" : ""}
        >
          Details
        </Button>
      </div>
    </li>
  );
}
