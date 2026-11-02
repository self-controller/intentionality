import { ChevronRight } from "lucide-react";
import { cn } from "../src/lib/utils";
import type { LabelOpt, Row } from "./types";

/** The key of the group for rows that wear no label. Cannot collide with a
 *  label's key, which is a lowercased name a person typed. */
export const OTHER = "\0other";

export interface Group {
  /** Lowercased name, the store's NOCASE rule, or OTHER. */
  key: string;
  name: string;
  color: string | null;
  rows: Row[];
}

/** Rows grouped under their labels, in the order the labels arrive in (the
 *  store's), with Other last. A row with several labels is listed under each
 *  of them; every copy carries the same index, so a tick in one is a tick in
 *  all. A label no list knows yet (typed into a panel just now) gets a group
 *  spelled the way the row spells it. Empty groups are left out. */
export function groupRows(rows: Row[], labels: LabelOpt[]): Group[] {
  const rank = new Map(labels.map((l, i) => [l.name.toLowerCase(), i]));
  const known = new Map(labels.map((l) => [l.name.toLowerCase(), l]));
  const groups = new Map<string, Group>();
  const other: Group = { key: OTHER, name: "Other", color: null, rows: [] };

  for (const row of rows) {
    const keys = [...new Set(row.labels.map((n) => n.toLowerCase()))];
    if (keys.length === 0) other.rows.push(row);
    for (const key of keys) {
      let group = groups.get(key);
      if (!group) {
        const label = known.get(key);
        const name = label?.name ?? row.labels.find((n) => n.toLowerCase() === key)!;
        group = { key, name, color: label?.color ?? null, rows: [] };
        groups.set(key, group);
      }
      group.rows.push(row);
    }
  }

  // Map iteration is insertion order, so unknown labels keep first-seen order
  // after the known ones.
  const sorted = [...groups.values()].sort(
    (a, b) => (rank.get(a.key) ?? Infinity) - (rank.get(b.key) ?? Infinity),
  );
  return other.rows.length ? [...sorted, other] : sorted;
}

/** One label's section of the backlog: a header that folds the rows under
 *  it away. Flat, hairline-separated sections rather than cards -- the
 *  backlog is the quiet half of the screen. */
export default function LabelGroup({
  group,
  open,
  onToggle,
  children,
}: {
  group: Group;
  open: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}) {
  return (
    <section className="border-b border-line last:border-0">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="group flex w-full items-center gap-[0.6rem] rounded-[0.5rem] py-[0.7rem] pr-[0.3rem] text-left
                   outline-none focus-visible:ring-2 focus-visible:ring-text/50"
      >
        <ChevronRight
          aria-hidden="true"
          className={cn(
            "size-[0.9rem] flex-none text-muted transition-transform duration-200 group-hover:text-text",
            open && "rotate-90",
          )}
        />
        <span
          className={cn("size-[0.5rem] flex-none rounded-full", !group.color && "border border-muted")}
          style={group.color ? { background: group.color } : undefined}
        />
        <span className="min-w-0 truncate text-[0.84rem] font-medium text-text">{group.name}</span>
        <span className="flex-1" />
        <span className="font-mono text-[0.72rem] tabular-nums text-muted">{group.rows.length}</span>
      </button>
      {open && <ul className="gate-rise pb-[0.4rem] pl-[1.5rem]">{children}</ul>}
    </section>
  );
}
