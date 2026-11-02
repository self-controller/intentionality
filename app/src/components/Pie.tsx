import { useState } from "react";
import { cn } from "@/lib/utils";
import { fmt } from "../format";

/** What a slice's colour says. The same tones as the bars (Bars.tsx): time
 *  on task, time no task needed, time nobody judged, and series1-5 for one
 *  task each. Whole class names, so Tailwind's scanner sees each one. */
const STROKE = {
  primary: "stroke-primary",
  warn: "stroke-warn",
  muted: "stroke-muted-foreground/60",
  series1: "stroke-series-1",
  series2: "stroke-series-2",
  series3: "stroke-series-3",
  series4: "stroke-series-4",
  series5: "stroke-series-5",
};
const SWATCH = {
  primary: "bg-primary",
  warn: "bg-warn",
  muted: "bg-muted-foreground/60",
  series1: "bg-series-1",
  series2: "bg-series-2",
  series3: "bg-series-3",
  series4: "bg-series-4",
  series5: "bg-series-5",
};

export type PieTone = keyof typeof STROKE;

export interface PieSlice {
  key: string;
  label: string;
  seconds: number;
  tone: PieTone;
  /** Hover text on the legend row, for what the label leaves out. */
  hint?: string;
}

const SIZE = 168;
const WIDTH = 22; // the ring's thickness
const R = (SIZE - WIDTH) / 2 - 3; // room for the hovered slice to grow
const C = 2 * Math.PI * R;
/** Surface showing between slices, in px of arc. */
const GAP = 2;

/**
 * A pie with its middle cut out for the headline, and a legend that carries
 * every slice in words, time and share -- colour is never the only signal
 * (green and amber, or amber and white to some readers, are close).
 * Hovering a slice or its legend row puts that slice in the middle.
 *
 * Plain SVG: one ring of dashed circles, each dash a slice.
 */
export function Pie({
  slices,
  headline,
  caption,
  empty,
}: {
  slices: PieSlice[];
  /** The middle when nothing is hovered. */
  headline: string;
  caption: string;
  /** Said in the middle when there is no time at all. */
  empty: string;
}) {
  const [hover, setHover] = useState<string | null>(null);
  const total = slices.reduce((sum, s) => sum + s.seconds, 0);
  const shown = slices.filter((s) => s.seconds > 0);
  const share = (s: PieSlice) => (total > 0 ? s.seconds / total : 0);
  const percent = (s: PieSlice) => {
    const p = share(s) * 100;
    return p > 0 && p < 1 ? "<1%" : `${Math.round(p)}%`;
  };
  const focus = shown.find((s) => s.key === hover) ?? null;
  // A lone slice is the whole ring: no gap to leave.
  const gap = shown.length > 1 ? GAP : 0;

  let start = 0;
  const arcs = shown.map((s) => {
    const len = share(s) * C;
    const arc = { slice: s, offset: -start, dash: Math.max(0.75, len - gap) };
    start += len;
    return arc;
  });

  return (
    <div className="flex flex-wrap items-center gap-x-8 gap-y-5">
      <div className="relative flex-none" style={{ width: SIZE, height: SIZE }}>
        <svg
          viewBox={`0 0 ${SIZE} ${SIZE}`}
          width={SIZE}
          height={SIZE}
          className="-rotate-90"
          role="img"
          aria-label={shown.map((s) => `${s.label} ${fmt(s.seconds)}`).join(", ") || empty}
        >
          <circle
            cx={SIZE / 2}
            cy={SIZE / 2}
            r={R}
            fill="none"
            strokeWidth={WIDTH}
            className="stroke-secondary"
          />
          {arcs.map(({ slice, offset, dash }) => (
            <circle
              key={slice.key}
              cx={SIZE / 2}
              cy={SIZE / 2}
              r={R}
              fill="none"
              strokeWidth={slice.key === hover ? WIDTH + 6 : WIDTH}
              strokeDasharray={`${dash} ${C}`}
              strokeDashoffset={offset}
              className={cn(
                STROKE[slice.tone],
                "cursor-default transition-[stroke-width,opacity] duration-150",
                hover && slice.key !== hover && "opacity-40",
              )}
              onPointerEnter={() => setHover(slice.key)}
              onPointerLeave={() => setHover(null)}
            />
          ))}
        </svg>
        <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center px-6 text-center">
          {total <= 0 ? (
            <span className="text-[13px] text-muted-foreground">{empty}</span>
          ) : focus ? (
            <>
              <span className="text-2xl font-semibold tracking-tight text-foreground">{percent(focus)}</span>
              <span className="mt-0.5 text-[12px] leading-tight text-muted-foreground">{focus.label}</span>
              <span className="mt-0.5 font-mono text-[11px] text-muted-foreground">{fmt(focus.seconds)}</span>
            </>
          ) : (
            <>
              <span className="text-[32px] font-semibold leading-none tracking-tight text-foreground">
                {headline}
              </span>
              <span className="mt-1.5 text-[12px] leading-tight text-muted-foreground">{caption}</span>
            </>
          )}
        </div>
      </div>

      <ul className="min-w-[220px] flex-1 list-none p-0">
        {slices.map((s) => (
          <li
            key={s.key}
            title={s.hint}
            onPointerEnter={() => s.seconds > 0 && setHover(s.key)}
            onPointerLeave={() => setHover(null)}
            className={cn(
              "flex items-center gap-3 rounded-md px-2 py-1.5 text-[13px] transition-opacity",
              hover && s.key !== hover && "opacity-50",
            )}
          >
            <span className={cn("size-2.5 flex-none rounded-full", SWATCH[s.tone])} />
            <span className="min-w-0 flex-1 truncate text-foreground">{s.label}</span>
            <span className="w-16 flex-none text-right font-mono text-xs tabular-nums text-muted-foreground">
              {fmt(s.seconds)}
            </span>
            <span className="w-10 flex-none text-right font-mono text-xs tabular-nums text-foreground">
              {percent(s)}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
