import { cn } from "@/lib/utils";

/** "91%", or a dash for a task nobody has asked about yet. */
export const pct = (p: number | null) => (p == null ? "—" : `${Math.round(p * 100)}%`);

/** One verdict as a thin bar, with the bar it has to clear marked across it.
 *  Shared by the appeal and the Analyses tab's page rows, so a 70% bar
 *  looks the same wherever a verdict is shown. */
export function ProbabilityBar({
  probability,
  threshold,
  excused,
  className,
}: {
  probability: number | null;
  threshold: number;
  excused: boolean;
  className?: string;
}) {
  return (
    <div className={cn("relative h-1 overflow-hidden rounded-full bg-secondary", className)}>
      <div
        className={cn("h-full rounded-full", excused ? "bg-primary" : "bg-warn/70")}
        style={{ width: `${(probability ?? 0) * 100}%` }}
      />
      <div className="absolute inset-y-0 w-px bg-foreground/40" style={{ left: `${threshold * 100}%` }} />
    </div>
  );
}
