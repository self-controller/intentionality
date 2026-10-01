
/** The gate's small shared pieces. The app has its own design system now
 *  (src/components/ui: shadcn Tabs, ReUI Button) on its own palette, and
 *  nothing in it imports these. This file stays under src/ui/, which gate.css
 *  scans, so the gate's look is unchanged. */

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  /** ghost: the default, a quiet outline. primary: the one action that
   *  commits. danger: destructive. */
  tone?: "ghost" | "primary" | "danger";
};

const TONES: Record<NonNullable<ButtonProps["tone"]>, string> = {
  ghost:
    "border border-line text-muted hover:text-text hover:border-muted " +
    "disabled:hover:text-muted disabled:hover:border-line",
  primary:
    "bg-accent text-bg border border-accent font-medium " +
    "shadow-lift hover:-translate-y-px disabled:hover:translate-y-0",
  danger:
    "border border-line text-muted hover:text-bad hover:border-bad " +
    "disabled:hover:text-muted disabled:hover:border-line",
};

export function Button({ tone = "ghost", className = "", ...rest }: ButtonProps) {
  return (
    <button
      {...rest}
      className={
        "rounded-[0.45em] px-[0.8em] py-[0.3em] transition-all duration-150 " +
        "outline-none focus-visible:ring-2 focus-visible:ring-accent " +
        "disabled:opacity-40 disabled:cursor-default " +
        `${TONES[tone]} ${className}`
      }
    />
  );
}

export function TextInput({
  className = "",
  ...rest
}: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...rest}
      className={
        "bg-surface border border-line rounded-[0.45em] px-[0.7em] py-[0.35em] " +
        "text-text placeholder:text-muted/70 outline-none " +
        "focus:border-accent transition-colors duration-150 " +
        `${className}`
      }
    />
  );
}

/** The 13px-uppercase-muted recipe that styles.css repeated nine times. */
export function SectionHeading({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <h2
      className={`text-[0.7em] uppercase tracking-[0.18em] text-muted ${className}`}
    >
      {children}
    </h2>
  );
}
