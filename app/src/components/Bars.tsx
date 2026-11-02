/** The observed-time chart, shared by the Dashboard and the Analyses pane:
 *  one bar per activity (a window or page, see format.ts `activities`). One
 *  component, so the two cannot drift. */

// Whole class names, so Tailwind's scanner sees each one.
const TONE = {
  primary: "bg-primary",
  warn: "bg-warn",
  muted: "bg-muted-foreground/50",
  // One task each, matching its slice in the Dashboard's pie.
  series1: "bg-series-1",
  series2: "bg-series-2",
  series3: "bg-series-3",
  series4: "bg-series-4",
  series5: "bg-series-5",
};

export function Bars({ children }: { children: React.ReactNode }) {
  return <div className="flex max-w-[640px] flex-col gap-2">{children}</div>;
}

export function BarRow({
  label,
  value,
  pct,
  hint,
  /** Past the model's cut: shown because the record should be complete, but
   *  quieter, because the model never saw it. */
  unsent = false,
  /** What the bar's colour says: time on task (series1-5: which task, as in
   *  the Dashboard's pie), time no task needed, time nobody judged. Words in
   *  the label carry the meaning; this reinforces. */
  tone = "primary",
}: {
  label: string;
  value: string;
  pct: number;
  /** Hover text, when the label is a shortened form of something longer. */
  hint?: string;
  unsent?: boolean;
  tone?: keyof typeof TONE;
}) {
  const dim = unsent ? "opacity-55 italic" : "";
  return (
    <div className="flex items-center gap-2.5 text-[13px]">
      <span
        className={
          "w-56 flex-none overflow-hidden text-ellipsis whitespace-nowrap text-right " +
          (unsent ? `text-muted-foreground ${dim}` : "text-foreground")
        }
        title={hint ?? label}
      >
        {label}
      </span>
      <span className="h-2 flex-1 overflow-hidden rounded-full bg-secondary">
        <span
          className={"block h-full rounded-full " + TONE[tone] + (unsent ? " opacity-20" : " opacity-90")}
          style={{ width: `${pct}%` }}
        />
      </span>
      <span className={`w-16 flex-none font-mono text-xs tabular-nums text-muted-foreground ${dim}`}>
        {value}
      </span>
    </div>
  );
}

export function BarMore({
  onClick,
  children,
}: {
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      className="self-start text-xs text-muted-foreground transition-colors hover:text-foreground"
      onClick={onClick}
    >
      {children}
    </button>
  );
}
