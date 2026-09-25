import { useState } from "react";
import { Button } from "./primitives";
import { ISO_DAY, localDate } from "../format";

/** A month grid rendered in flow, not in a popover.
 *
 *  WebKitGTK does ship a native date picker, but it opens as a popover: under
 *  cage its placement was untested, and in the app it turned out to be worse
 *  than that -- the GTK popup takes an input grab the page never gets back, so
 *  the only way out of the calendar was to alt-tab away and back. Drawing the
 *  grid ourselves removes the question for both front ends. */

const DAYS = ["M", "T", "W", "T", "F", "S", "S"];

export default function Calendar({
  value,
  onPick,
}: {
  value: string;
  onPick: (day: string) => void;
}) {
  const start = ISO_DAY.test(value) ? new Date(`${value}T00:00:00`) : new Date();
  const [cursor, setCursor] = useState(new Date(start.getFullYear(), start.getMonth(), 1));
  const today = localDate(new Date());

  const first = new Date(cursor.getFullYear(), cursor.getMonth(), 1);
  const lead = (first.getDay() + 6) % 7; // Monday-first
  const count = new Date(cursor.getFullYear(), cursor.getMonth() + 1, 0).getDate();
  const cells: (string | null)[] = [
    ...Array(lead).fill(null),
    ...Array.from({ length: count }, (_, i) =>
      localDate(new Date(cursor.getFullYear(), cursor.getMonth(), i + 1)),
    ),
  ];

  const shift = (by: number) =>
    setCursor(new Date(cursor.getFullYear(), cursor.getMonth() + by, 1));

  return (
    <div className="mt-[0.6em] w-[15em] rounded-[0.5em] border border-line bg-surface p-[0.6em]">
      <div className="mb-[0.4em] flex items-center justify-between">
        <Button className="!px-[0.55em]" onClick={() => shift(-1)} aria-label="Previous month">
          &lsaquo;
        </Button>
        <span className="text-[0.8em] text-muted">
          {cursor.toLocaleDateString(undefined, { month: "long", year: "numeric" })}
        </span>
        <Button className="!px-[0.55em]" onClick={() => shift(1)} aria-label="Next month">
          &rsaquo;
        </Button>
      </div>
      <div className="grid grid-cols-7 gap-[0.15em] text-center text-[0.65em] text-muted">
        {DAYS.map((d, i) => (
          <span key={i}>{d}</span>
        ))}
      </div>
      <div className="mt-[0.2em] grid grid-cols-7 gap-[0.15em]">
        {cells.map((day, i) =>
          day === null ? (
            <span key={i} />
          ) : (
            <button
              key={i}
              onClick={() => onPick(day)}
              className={
                "rounded-[0.3em] py-[0.25em] text-[0.72em] transition-colors duration-100 " +
                (day === value
                  ? "bg-accent text-bg"
                  : day === today
                    ? "text-accent hover:bg-raised"
                    : "text-text hover:bg-raised")
              }
            >
              {Number(day.slice(8))}
            </button>
          ),
        )}
      </div>
    </div>
  );
}
