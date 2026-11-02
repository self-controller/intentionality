import * as React from "react";
import { cn } from "../lib/utils";

/** The gate's small shared pieces. The desktop app has its own design system
 *  (src/components/ui: shadcn Tabs, ReUI Button) on its own token names and
 *  imports nothing from here; the file stays under src/ui/ because gate.css
 *  scans that directory. The look is the app's: a near-black page, quiet
 *  secondary actions with no fill, one filled primary. */

type Tone = "ghost" | "outline" | "primary" | "danger";
type Size = "sm" | "md" | "lg";

const BUTTON =
  "inline-flex shrink-0 select-none items-center justify-center whitespace-nowrap rounded-full " +
  "font-medium outline-none transition-[color,background-color,border-color,transform,opacity] " +
  "duration-150 focus-visible:ring-2 focus-visible:ring-text/50 disabled:pointer-events-none " +
  "disabled:opacity-40";

const TONES: Record<Tone, string> = {
  // No border: a wash on hover, a stronger one while it is a toggle that is on.
  ghost:
    "text-muted hover:bg-white/[0.06] hover:text-text " +
    "data-[selected=true]:bg-white/[0.09] data-[selected=true]:text-text",
  outline:
    "border border-line text-muted hover:border-white/20 hover:text-text " +
    "data-[selected=true]:border-white/40 data-[selected=true]:text-text",
  // The one action that commits.
  primary: "bg-text text-bg shadow-lift hover:-translate-y-px hover:bg-white",
  danger:
    "border border-line text-muted hover:border-bad/50 hover:text-bad " +
    "data-[selected=true]:border-bad/60 data-[selected=true]:text-bad",
};

const SIZES: Record<Size, string> = {
  sm: "h-[1.9rem] gap-[0.35rem] px-[0.8rem] text-[0.78rem] [&_svg]:size-[0.85rem]",
  md: "h-[2.25rem] gap-[0.4rem] px-[1rem] text-[0.86rem] [&_svg]:size-[0.95rem]",
  lg: "h-[2.7rem] gap-[0.5rem] px-[1.35rem] text-[0.95rem] [&_svg]:size-[1.05rem]",
};

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  tone?: Tone;
  size?: Size;
  /** A toggle that is on: drawn as its hover state, and kept there. */
  selected?: boolean;
};

export function Button({
  tone = "outline",
  size = "md",
  selected = false,
  className,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      data-selected={selected ? "true" : undefined}
      className={cn(BUTTON, SIZES[size], TONES[tone], className)}
      {...rest}
    />
  );
}

/** A square button holding one icon. `label` is required: it is the
 *  accessible name and the tooltip, since the icon alone says little. */
export function IconButton({
  label,
  tone = "ghost",
  size = "md",
  className,
  children,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  label: string;
  tone?: "ghost" | "danger";
  size?: "sm" | "md";
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-[0.55rem] outline-none " +
          "transition-colors duration-150 focus-visible:ring-2 focus-visible:ring-text/50 " +
          "disabled:pointer-events-none disabled:opacity-40",
        size === "sm" ? "size-[1.8rem] [&_svg]:size-[0.95rem]" : "size-[2.1rem] [&_svg]:size-[1.1rem]",
        tone === "danger"
          ? "text-muted hover:bg-bad/10 hover:text-bad"
          : "text-muted hover:bg-white/[0.07] hover:text-text",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

export function TextInput({ className, ...rest }: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...rest}
      className={cn(
        "h-[2.4rem] rounded-[0.65rem] border border-line bg-surface px-[0.85rem] text-[0.9rem] " +
          "text-text outline-none transition-colors duration-150 placeholder:text-muted/60 " +
          "focus:border-white/25 focus:bg-raised",
        className,
      )}
    />
  );
}

/** The mono eyebrow the desktop app sets over every section. */
export function SectionHeading({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <h2 className={cn("font-mono text-[0.68rem] font-medium uppercase tracking-[0.2em] text-muted", className)}>
      {children}
    </h2>
  );
}

/** A key cap, for the shortcuts the screen names. */
export function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="rounded-[0.3rem] border border-line bg-surface px-[0.4rem] py-[0.08rem] font-mono text-[0.68rem] text-muted">
      {children}
    </kbd>
  );
}
