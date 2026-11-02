import { SectionHeading } from "../src/ui/primitives";
import TaskRow from "./TaskRow";
import type { LabelOpt, Row } from "./types";

/** The left zone: what this session is for. Ticked rows, once each, in the
 *  store's order; the details panel under the row that asked; the add box
 *  (children) at the end. It normally fits and stays put; should it outgrow
 *  its zone (many tasks, an open panel, the add box's details), the list
 *  scrolls on its own so no task is ever out of reach. */
export default function TodayList({
  rows,
  labels,
  panelIndex,
  renderPanel,
  onDetails,
  children,
}: {
  rows: Row[];
  labels: LabelOpt[];
  /** The row whose panel is open here, if it is in this zone. */
  panelIndex: number | null;
  renderPanel: (row: Row) => React.ReactNode;
  onDetails: () => void;
  children?: React.ReactNode;
}) {
  const n = rows.length;
  return (
    <section className="flex min-h-0 min-w-0 flex-col">
      <header className="flex items-baseline gap-[0.8rem]">
        <SectionHeading>Today</SectionHeading>
        <span className="font-mono text-[0.72rem] text-muted">
          {n === 0 ? "nothing yet" : `${n} ${n === 1 ? "task" : "tasks"}`}
        </span>
      </header>
      <p className="mt-[0.35rem] text-[0.82rem] text-muted">
        What you are doing this session. It goes to the board when you start.
      </p>
      {/* Only a scroller while its content is taller than the zone. The
          inset padding, given back by the negative margin, keeps focus rings
          at the edges from being clipped. */}
      <div className="-mx-[0.4rem] mt-[0.3rem] min-h-0 overflow-y-auto px-[0.4rem] pb-[1rem] pt-[0.3rem]">
        <ul>
          {rows.map((row) => [
            <TaskRow
              key={row.index}
              row={row}
              labels={labels}
              density="today"
              open={row.index === panelIndex}
              onDetails={onDetails}
            />,
            row.index === panelIndex && renderPanel(row),
          ])}
          {n === 0 && (
            <li className="gate-pop py-[1.2rem] text-[0.9rem] text-muted">
              Nothing picked yet. Tick a task in the backlog, or add one below.
            </li>
          )}
        </ul>
        {children}
      </div>
    </section>
  );
}
