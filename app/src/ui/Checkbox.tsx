
/** The shared toggle: one neumorphic checkbox for the gate and the app.
 *
 * All the look lives in the `.nb-check` block in theme.css. Size is the only
 * dimension that varies -- every measurement in that block is a fraction of
 * `--cb`, so the proportions hold from the gate's 58px down to a list row's
 * 26px.
 *
 * It renders a <span>, not a <label>, so it can sit inside a row that is
 * already a <label> without nesting one. The hidden input covers the chip, so
 * clicking the chip toggles it either way. */
export default function Checkbox({
  checked,
  onChange,
  size = 26,
  disabled = false,
  label,
  className = "",
}: {
  checked: boolean;
  onChange?: (next: boolean) => void;
  /** px when a number; any CSS length when a string (the gate uses `em`, so
   *  the chip scales with the gate's root font size and the webview zoom). */
  size?: number | string;
  disabled?: boolean;
  /** Screen-reader name. Omit only when a visible <label> already wraps this. */
  label?: string;
  className?: string;
}) {
  return (
    <span
      className={`nb-check ${className}`}
      style={{ ["--cb" as string]: typeof size === "number" ? `${size}px` : size }}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        aria-label={label}
        onChange={(e) => onChange?.(e.target.checked)}
      />
      <span className="nb-mark" />
    </span>
  );
}
