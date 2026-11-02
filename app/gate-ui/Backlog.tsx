import { SectionHeading } from "../src/ui/primitives";
import LabelGroup, { type Group } from "./LabelGroup";
import SearchBox from "./SearchBox";
import TaskRow from "./TaskRow";
import type { LabelOpt, Row } from "./types";

/** The right zone: everything that waits, grouped by label (a task with two
 *  labels is listed under both; the copies share an index, so a tick in one
 *  is a tick in all). Marked rows stay here, struck through, with Undo.
 *
 *  The search box in the header narrows the list in place: only matching
 *  rows, only the groups holding one, and those open whatever their fold
 *  (the folds come back as they were when the box is cleared).
 *
 *  The list is the welcome screen's one real scroller: the screen around it
 *  stays put, and WebKit scrolls the list itself when the pointer is over
 *  it. */
export default function Backlog({
  groups,
  labels,
  folded,
  onToggle,
  panelIndex,
  panelAt,
  renderPanel,
  onDetails,
  query,
  setQuery,
  onSearchKey,
  terms,
  active,
  total,
  inToday,
}: {
  /** Already filtered by the search. */
  groups: Group[];
  labels: LabelOpt[];
  folded: ReadonlySet<string>;
  onToggle: (key: string) => void;
  /** The row whose panel is open here, if it is in this zone. */
  panelIndex: number | null;
  /** Which group's copy of that row shows the panel. */
  panelAt: string | null;
  renderPanel: (row: Row) => React.ReactNode;
  onDetails: (groupKey: string) => void;
  query: string;
  setQuery: (q: string) => void;
  onSearchKey: (e: React.KeyboardEvent<HTMLInputElement>) => void;
  terms: string[];
  /** The search's pick, by row and by the group whose copy is lit. */
  active: { index: number; key: string } | null;
  /** Tasks waiting before the search narrowed them. */
  total: number;
  /** Rows in Today that the search matches, for the empty result. */
  inToday: number;
}) {
  const searching = terms.length > 0;
  const shown = new Set(groups.flatMap((g) => g.rows.map((r) => r.index))).size;
  const count = searching
    ? `${shown} of ${total} ${shown === 1 ? "matches" : "match"}`
    : total === 0
      ? "empty"
      : `${total} waiting`;

  return (
    <section className="flex min-h-0 min-w-0 flex-col lg:border-l lg:border-line lg:pl-[2.5rem]">
      <header className="flex flex-wrap items-center gap-x-[0.8rem] gap-y-[0.4rem]">
        <SectionHeading>Backlog</SectionHeading>
        <span className="font-mono text-[0.72rem] text-muted">{count}</span>
        <span className="flex-1" />
        <SearchBox value={query} onChange={setQuery} onKeyDown={onSearchKey} />
      </header>
      <p className="mt-[0.35rem] text-[0.82rem] text-muted">
        {searching
          ? "↑ ↓ to pick a task, Enter to do it today, Alt+Enter to start it in Doing, Esc to clear."
          : "Tick a task to do it today. The rest waits here, under its labels."}
      </p>
      {/* The inset padding, given back by the negative margin, keeps focus
          rings at the edges from being clipped; the bottom padding lets the
          last row clear the plan bar. */}
      <div
        id="gate-backlog"
        className="-mx-[0.4rem] mt-[0.3rem] min-h-0 flex-1 overflow-y-auto px-[0.4rem] pb-[1.5rem] pt-[0.3rem]"
      >
        {groups.length === 0 ? (
          <p className="py-[1.2rem] text-[0.9rem] text-muted">
            {searching ? (
              <>
                No task waiting matches “{query.trim()}”.
                {inToday > 0 && (
                  <span className="text-muted/80">
                    {" "}
                    · {inToday} already in Today
                  </span>
                )}
              </>
            ) : (
              "Nothing waiting."
            )}
          </p>
        ) : (
          groups.map((group) => (
            <LabelGroup
              key={group.key}
              group={group}
              open={searching || !folded.has(group.key)}
              onToggle={() => onToggle(group.key)}
            >
              {group.rows.map((row) => {
                const open = row.index === panelIndex && group.key === panelAt;
                return [
                  <TaskRow
                    key={row.index}
                    row={row}
                    labels={labels}
                    density="backlog"
                    open={open}
                    onDetails={() => onDetails(group.key)}
                    terms={terms}
                    active={active !== null && active.index === row.index && active.key === group.key}
                  />,
                  open && renderPanel(row),
                ];
              })}
            </LabelGroup>
          ))
        )}
      </div>
    </section>
  );
}
