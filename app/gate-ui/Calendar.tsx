import { useState } from "react";
import { Button } from "../src/ui/primitives";

/** A month grid rendered in flow, not in a popover.
 *
 *  WebKitGTK does ship a native date picker, but it opens as a popover and
 *  popover placement under cage is untested -- the GTK gate avoided it for the
 *  same reason. Drawing the grid ourselves removes the question. */

const DAYS = ["M", "T", "W", "T", "F", "S", "S"];

function iso(d: Date): string {
  const m = `${d.getMonth() + 1}`.padStart(2, "0");
  const day = `${d.getDate()}`.padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}

export default function Calendar({
  value,
  onPick,
}: {
  value: string;
  onPick: (day: string) => void;
}) {
  const start = /^\d{4}-\d{2}-\d{2}$/.test(value) ? new Date(`${value}T00:00:00`) : new Date();
  const [cursor, setCursor] = useState(new Date(start.getFullYear(), start.getMonth(), 1));
  const today = iso(new Date());

  const first = new Date(cursor.getFullYear(), cursor.getMonth(), 1);
  const lead = (first.getDay() + 6) % 7; // Monday-first
  const count = new Date(cursor.getFullYear(), cursor.getMonth() + 1, 0).getDate();
  const cells: (string | null)[] = [
    ...Array(lead).fill(null),
    ...Array.from({ length: count }, (_, i) =>
      iso(new Date(cursor.getFullYear(), cursor.getMonth(), i + 1)),
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
                  ? "bg-accent text-black"
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
