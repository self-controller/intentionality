import { cn } from "../src/lib/utils";

export interface Segment {
  label: string;
  value: string;
}

/** A row of exclusive choices, drawn as one pill. `value` null = none of
 *  them is on (the plan bar shows a typed length that is not a preset). */
export default function Segmented({
  options,
  value,
  onChange,
  label,
}: {
  options: Segment[];
  value: string | null;
  onChange: (value: string) => void;
  label: string;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="inline-flex items-center gap-[0.15rem] rounded-full border border-line bg-surface p-[0.2rem]"
    >
      {options.map((o) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => onChange(o.value)}
            className={cn(
              "h-[1.8rem] rounded-full px-[0.85rem] text-[0.8rem] font-medium outline-none",
              "transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-text/50",
              on ? "bg-white/[0.1] text-text" : "text-muted hover:text-text",
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
