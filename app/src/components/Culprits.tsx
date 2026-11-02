import { useState } from "react";
import IgnoreMenu from "@/components/IgnoreMenu";
import { BarMore } from "@/components/Bars";
import { fmt } from "../format";
import type { DayTime } from "../types";

/** Rows shown before "Show all": the few that carry the day. */
const TOP = 8;

// Radix remounts the tab each time it is chosen; "Show all" stays as left.
let showAll = false;

/**
 * Where the day's not-relevant time went, on the Dashboard: one bar per site
 * (every LinkedIn page is linkedin.com), tool and project ("Claude Code:
 * Pager", "Terminal: Pager"), or app, busiest first -- ranked bars on one
 * baseline, in the pie's "Not relevant" colour, since each is a part of that
 * slice. The ignore list sits at the top right (the relevance cache, and the
 * way to empty it, are on the Settings page). `onChange` reads the day again
 * after the ignore list changes. `day` names a past day ("Sat, Oct 31");
 * none is today.
 */
export default function Culprits({
  data,
  day,
  onChange,
}: {
  data: DayTime | null;
  day?: string;
  onChange: () => void;
}) {
  const [all, setAll] = useState(showAll);
  const when = day ? `on ${day}` : "today";

  const rows = data?.culprits ?? [];
  const shown = all ? rows : rows.slice(0, TOP);
  const most = Math.max(1, ...rows.map((r) => r.seconds));
  const toggle = () => {
    showAll = !all;
    setAll(showAll);
  };

  return (
    <section className="surface flex flex-col px-5 py-4" aria-label="Biggest culprits">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h3>Biggest culprits</h3>
          <p className="text-xs text-muted-foreground">Not relevant to any task · {day ?? "today"}</p>
        </div>
        <IgnoreMenu seen={data?.seen ?? []} day={day} onChange={onChange} />
      </div>

      {data == null ? (
        <p className="pb-2 text-[13px] text-muted-foreground">…</p>
      ) : rows.length === 0 ? (
        <p className="pb-2 text-[13px] leading-snug text-muted-foreground">
          Nothing judged off-task {day ? when : "yet today"}. Time Jev finds relevant to none of your tasks shows
          here, by site, app and project.
        </p>
      ) : (
        <ul className="-mx-2 list-none p-0">
          {shown.map((r) => {
            const share = data.off_seconds > 0 ? Math.round((r.seconds / data.off_seconds) * 100) : 0;
            return (
              <li
                key={r.name}
                className="rounded-md px-2 py-1.5 hover:bg-secondary"
                title={`${r.name} — ${fmt(r.seconds)}, ${share}% of the not-relevant time ${when}`}
              >
                <div className="flex items-baseline gap-2 text-[13px]">
                  <span className="min-w-0 flex-1 truncate text-foreground">{r.name}</span>
                  <span className="flex-none font-mono text-xs tabular-nums text-muted-foreground">{fmt(r.seconds)}</span>
                </div>
                <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-secondary">
                  <div className="h-full rounded-full bg-warn opacity-90" style={{ width: `${(r.seconds / most) * 100}%` }} />
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {rows.length > TOP && (
        <div className="mt-1.5">
          <BarMore onClick={toggle}>{all ? "Show fewer" : `Show all ${rows.length}`}</BarMore>
        </div>
      )}
    </section>
  );
}
