/** The observed-time chart, shared by the Dashboard and the Analyses pane.
 *
 *  It was duplicated markup in both, down to the 160px/144px label widths
 *  that keep an app row's track aligned with its title rows'. One component
 *  now, so they cannot drift. */

export function Bars({ children }: { children: React.ReactNode }) {
  return <div className="flex max-w-[560px] flex-col gap-2.5">{children}</div>;
}

export function BarGroup({ children }: { children: React.ReactNode }) {
  return <div className="flex flex-col gap-1">{children}</div>;
}

export function BarRow({
  label,
  value,
  pct,
  sub = false,
  /** Past the model's cut: shown because the record should be complete, but
   *  quieter, because the model never saw it. */
  unsent = false,
}: {
  label: string;
  value: string;
  pct: number;
  sub?: boolean;
  unsent?: boolean;
}) {
  const dim = unsent ? "opacity-55 italic" : "";
  return (
    <div className={"flex items-center gap-2.5 " + (sub ? "pl-4 text-xs" : "")}>
      <span
        className={
          "flex-none overflow-hidden text-ellipsis whitespace-nowrap text-right " +
          (sub ? `w-36 text-muted-foreground ${dim}` : "w-40 text-foreground")
        }
        title={label}
      >
        {label}
      </span>
      <span className={"flex-1 overflow-hidden rounded-full bg-secondary " + (sub ? "h-1.5" : "h-2.5")}>
        <span
          className={
            "block h-full rounded-full bg-primary " +
            (unsent ? "opacity-20" : sub ? "opacity-45" : "opacity-90")
          }
          style={{ width: `${pct}%` }}
        />
      </span>
      <span className={`w-16 flex-none font-mono text-xs tabular-nums text-muted-foreground ${sub ? dim : ""}`}>
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
      className="self-start pl-4 text-xs text-muted-foreground transition-colors hover:text-foreground"
      onClick={onClick}
    >
      {children}
    </button>
  );
}
