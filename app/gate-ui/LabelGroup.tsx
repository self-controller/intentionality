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

/** The groups dealt into `cols` stacks, masonry style: each in turn goes to
 *  the lightest stack so far (the leftmost on a tie), so the cards in a stack
 *  sit straight under one another and a short card leaves no hole beside a
 *  tall one. Weighed by row count, never by measured height, so folding a
 *  card or opening a Details panel cannot move anything to the other stack;
 *  only a task arriving or leaving can. */
export function stack(groups: Group[], cols: number): Group[][] {
  const stacks: Group[][] = Array.from({ length: Math.max(1, cols) }, () => []);
  const weight = stacks.map(() => 0);
  for (const group of groups) {
    const i = weight.indexOf(Math.min(...weight));
    stacks[i].push(group);
    weight[i] += 1 + group.rows.length;
  }
  return stacks;
}

/** One label's card: a header that folds the rows under it away. Cards are
 *  stacked in columns (stack()), so a card that grows (an open Details panel)
 *  pushes down only the cards under it, never the ones beside it. */
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
    <section className="min-w-0 rounded-[0.7rem] border border-line bg-surface">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="group flex w-full items-center gap-[0.6rem] rounded-[0.7rem] px-[0.9rem] py-[0.5rem]
                   text-left text-[0.95rem] text-text outline-none
                   focus-visible:ring-2 focus-visible:ring-accent"
      >
        <svg
          viewBox="0 0 16 16"
          aria-hidden="true"
          className={
            "h-[0.85em] w-[0.85em] flex-none text-muted transition-transform duration-200 " +
            "group-hover:text-text " +
            (open ? "rotate-90" : "")
          }
        >
          <path d="M6 3.5 10.5 8 6 12.5" fill="none" stroke="currentColor"
                strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
        <span
          className={"h-[0.55em] w-[0.55em] flex-none rounded-full " + (group.color ? "" : "border border-muted")}
          style={group.color ? { background: group.color } : undefined}
        />
        <span className="min-w-0 truncate font-medium">{group.name}</span>
        <span className="flex-1" />
        <span className="flex-none text-[0.8em] tabular-nums text-muted">{group.rows.length}</span>
      </button>
      {open && (
        <ul className="gate-rise border-t border-line px-[0.9rem] pb-[0.4rem] pt-[0.25rem]">
          {children}
        </ul>
      )}
    </section>
  );
}
