import { useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { ISO_DAY, localDate } from "../format";

/** The app's month grid, drawn in flow rather than in a popover -- for the
 *  same reason as the gate's (src/ui/Calendar.tsx): WebKitGTK's native date
 *  picker takes an input grab the page never gets back.
 *
 *  A separate component from the gate's on purpose. That one is drawn in the
 *  gate's palette and lives where the gate's stylesheet scans; this one is in
 *  the app's. The grid arithmetic is the same dozen lines in both.
 *
 *  `today` is the day to ring (the calendar date unless the caller reckons
 *  days otherwise -- the Dashboard's start at 4 am), and days after `max`
 *  cannot be picked. */

const DAYS = ["M", "T", "W", "T", "F", "S", "S"];

export default function MonthCalendar({
  value,
  onPick,
  today = localDate(new Date()),
  max,
  className,
}: {
  value: string;
  onPick: (day: string) => void;
  today?: string;
  max?: string;
  className?: string;
}) {
  const start = ISO_DAY.test(value) ? new Date(`${value}T00:00:00`) : new Date();
  const [cursor, setCursor] = useState(new Date(start.getFullYear(), start.getMonth(), 1));

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
    <div className={cn("mt-2 w-64 rounded-lg border border-border bg-background/40 p-2.5", className)}>
      <div className="mb-1.5 flex items-center justify-between">
        <Button variant="ghost" mode="icon" size="sm" onClick={() => shift(-1)} aria-label="Previous month">
          <ChevronLeft />
        </Button>
        <span className="text-[13px] font-medium tracking-[-0.01em]">
          {cursor.toLocaleDateString(undefined, { month: "long", year: "numeric" })}
        </span>
        <Button variant="ghost" mode="icon" size="sm" onClick={() => shift(1)} aria-label="Next month">
          <ChevronRight />
        </Button>
      </div>
      <div className="grid grid-cols-7 gap-0.5 text-center font-mono text-[10px] text-muted-foreground">
        {DAYS.map((d, i) => (
          <span key={i} className="py-1">
            {d}
          </span>
        ))}
      </div>
      <div className="grid grid-cols-7 gap-0.5">
        {cells.map((day, i) =>
          day === null ? (
            <span key={i} />
          ) : (
            <button
              key={i}
              onClick={() => onPick(day)}
              disabled={max !== undefined && day > max}
              className={cn(
                "h-8 rounded-md text-xs tabular-nums transition-colors duration-100 disabled:pointer-events-none disabled:opacity-30",
                day === value
                  ? "bg-primary font-medium text-primary-foreground"
                  : day === today
                    ? "bg-accent font-medium text-foreground ring-1 ring-inset ring-border hover:bg-secondary"
                    : "text-foreground/80 hover:bg-accent hover:text-foreground",
              )}
            >
              {Number(day.slice(8))}
            </button>
          ),
        )}
      </div>
    </div>
  );
}
