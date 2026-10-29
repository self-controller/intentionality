import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, ListFilter } from "lucide-react";
import { cn } from "@/lib/utils";
import type { Label, Task } from "../types";

/** The key for cards that wear no label. Cannot collide with a label's key,
 *  which is a lowercased name a person typed. */
export const NO_LABEL = "\0none";

export interface LabelOption {
  key: string;
  name: string;
  // The label itself, to preset on a card made while it is chosen; null for
  // No label.
  label: Label | null;
  count: number;
}

/** What the backlog can be narrowed to: every label its cards wear, by name,
 *  then No label when some card wears none. Keys are lowercased names, the
 *  store's NOCASE rule. */
export function labelOptions(backlog: Task[]): LabelOption[] {
  const byKey = new Map<string, LabelOption>();
  let bare = 0;
  for (const t of backlog) {
    if (t.labels.length === 0) bare++;
    for (const label of t.labels) {
      const key = label.name.toLowerCase();
      const option = byKey.get(key);
      if (option) option.count++;
      else byKey.set(key, { key, name: label.name, label, count: 1 });
    }
  }
  const named = [...byKey.values()].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));
  return bare ? [...named, { key: NO_LABEL, name: "No label", label: null, count: bare }] : named;
}

export function matches(task: Task, key: string): boolean {
  if (key === NO_LABEL) return task.labels.length === 0;
  return task.labels.some((l) => l.name.toLowerCase() === key);
}

function Swatch({ label }: { label: Label | null }) {
  return label ? (
    <span className="size-2 flex-none rounded-full" style={{ background: label.color }} />
  ) : (
    <span className="size-2 flex-none rounded-full border border-muted-foreground" />
  );
}

/**
 * The backlog's label dropdown: a chip laid over the column's header, and the
 * list it opens. Drawn in the page, not a <select>: WebKitGTK answers that with
 * a native popup, and under GNOME/Wayland those take an input grab the page
 * never gets back (see ColorPicker).
 */
export default function LabelFilter({
  options,
  value,
  total,
  compact,
  onChange,
}: {
  options: LabelOption[];
  // An option's key, or null for every card.
  value: string | null;
  // How many cards All stands for.
  total: number;
  // No room for the name beside the header's title: the chip is its icon (or
  // the chosen label's dot) alone, and the name is in its tooltip and list.
  compact: boolean;
  onChange: (key: string | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    // Capture, so a press that starts a drag on the board still closes it.
    const away = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", away, true);
    window.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("pointerdown", away, true);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  const current = options.find((o) => o.key === value) ?? null;
  const pick = (key: string | null) => {
    setOpen(false);
    onChange(key);
  };

  const item = (key: string | null, name: string, count: number, swatch: React.ReactNode) => (
    <button
      key={key ?? ""}
      type="button"
      role="menuitemradio"
      aria-checked={value === key}
      onClick={() => pick(key)}
      className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-foreground transition-colors hover:bg-secondary"
    >
      <span className="flex w-3.5 flex-none justify-center">{swatch}</span>
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <span className="font-mono text-[11px] tabular-nums text-muted-foreground">{count}</span>
      <Check className={cn("size-3.5 flex-none", value === key ? "opacity-100" : "opacity-0")} />
    </button>
  );

  return (
    <div ref={root} className="pointer-events-auto relative min-w-0">
      <button
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`Backlog label: ${current ? current.name : "all labels"}`}
        title={current ? `Showing ${current.name}` : "Show one label's cards"}
        onClick={() => setOpen((o) => !o)}
        className={cn(
          "flex h-7 max-w-full items-center gap-1.5 rounded-full border border-border bg-card/90 px-2.5 text-xs backdrop-blur transition-colors hover:text-foreground",
          current ? "border-foreground/25 text-foreground" : "text-muted-foreground",
        )}
      >
        {current ? <Swatch label={current.label} /> : <ListFilter className="size-3.5 flex-none" />}
        {!compact && <span className="truncate">{current ? current.name : "All labels"}</span>}
        <ChevronDown className={cn("size-3.5 flex-none transition-transform", open && "rotate-180")} />
      </button>
      {open && (
        <div
          role="menu"
          aria-label="Backlog labels"
          className="absolute right-0 top-full z-10 mt-1.5 max-h-80 w-56 overflow-y-auto rounded-lg border border-border bg-card p-1 shadow-xl"
        >
          {item(null, "All labels", total, <ListFilter className="size-3.5 flex-none text-muted-foreground" />)}
          {options.map((o) => item(o.key, o.name, o.count, <Swatch label={o.label} />))}
        </div>
      )}
    </div>
  );
}
