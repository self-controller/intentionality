import { useState } from "react";

type Rgb = [number, number, number];

const HEX = /^#?([\da-f]{2})([\da-f]{2})([\da-f]{2})$/i;

function toRgb(hex: string): Rgb | null {
  const m = HEX.exec(hex.trim());
  return m ? [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)] : null;
}

export function toHex([r, g, b]: Rgb): string {
  return "#" + [r, g, b].map((n) => n.toString(16).padStart(2, "0")).join("");
}

const CHANNELS = [
  { name: "R", tint: "#f7768e" },
  { name: "G", tint: "#9ece6a" },
  { name: "B", tint: "#7aa2f7" },
] as const;

const clamp = (n: number) => Math.max(0, Math.min(255, Math.round(n)));

/**
 * An RGB picker drawn in the page: a slider and a number per channel, a hex
 * field and a swatch. Not <input type="color">: WebKitGTK answers that with a
 * native GTK dialog, and a native popup under GNOME/Wayland takes an input
 * grab the page never gets back (the date picker's trap, see TaskEditor).
 */
export default function ColorPicker({
  title,
  value,
  busy = false,
  onApply,
  onCancel,
}: {
  // What is being coloured, said above the controls.
  title?: string;
  value: string;
  busy?: boolean;
  onApply: (hex: string) => void;
  onCancel: () => void;
}) {
  const [rgb, setRgb] = useState<Rgb>(() => toRgb(value) ?? [128, 128, 128]);
  // What is typed in the hex field, which may be half a colour for a moment.
  const [draft, setDraft] = useState(() => toHex(toRgb(value) ?? [128, 128, 128]));
  const hex = toHex(rgb);

  const setChannel = (i: number, n: number) => {
    const next: Rgb = [...rgb];
    next[i] = clamp(Number.isFinite(n) ? n : 0);
    setRgb(next);
    setDraft(toHex(next));
  };

  return (
    <div className="mt-2 rounded-md border border-line p-3">
      {title && <p className="mb-2 text-xs text-muted">{title}</p>}
      <div className="flex items-start gap-3">
        <span
          className="h-[4.5rem] w-[4.5rem] flex-none rounded-md border border-line"
          style={{ background: hex }}
          aria-label={`New colour ${hex}`}
        />
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          {CHANNELS.map(({ name, tint }, i) => (
            <label key={name} className="flex items-center gap-2 text-xs text-muted">
              <span className="w-3 flex-none">{name}</span>
              <input
                type="range"
                min={0}
                max={255}
                value={rgb[i]}
                disabled={busy}
                className="min-w-0 flex-1"
                style={{ accentColor: tint }}
                aria-label={`${name} channel`}
                onChange={(e) => setChannel(i, Number(e.target.value))}
              />
              <input
                type="number"
                min={0}
                max={255}
                value={rgb[i]}
                disabled={busy}
                className="w-[3.75rem] flex-none"
                aria-label={`${name} value`}
                onChange={(e) => setChannel(i, Number(e.target.value))}
              />
            </label>
          ))}
        </div>
      </div>
      <div className="mt-2.5 flex flex-wrap items-center gap-2">
        <input
          className="w-[6.5rem] font-mono"
          value={draft}
          disabled={busy}
          aria-label="Hex colour"
          onChange={(e) => {
            setDraft(e.target.value);
            const parsed = toRgb(e.target.value);
            if (parsed) setRgb(parsed);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              onApply(hex);
            }
          }}
        />
        <span className="flex-1" />
        <button disabled={busy} onClick={() => onApply(hex)}>
          Apply
        </button>
        <button disabled={busy} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
