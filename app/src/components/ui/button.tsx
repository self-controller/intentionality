import * as React from "react";
import { cva } from "class-variance-authority";
import { ArrowRight } from "lucide-react";
import { cn } from "@/lib/utils";

// The pasted InteractiveHoverButton, as every button in the app: a bordered
// pill with a dot, which on hover swells into a fill while the label slides
// out and comes back in with an arrow. Changes from the paste:
// - Width follows the label (no w-32), so the padding leaves the overlay room
//   for its arrow, and the dot sits in the left padding rather than at 20%,
//   where a short pill would put it on the label.
// - Children are drawn twice (at rest, and in the overlay), so an icon
//   before the label rides along.
// - `destructive` is red throughout; `selected` (a toggle that is on) holds
//   the hovered look; `mode="icon"` is a round pill whose dot blooms from the
//   middle, with no slide and no arrow.

type Variant = "primary" | "secondary" | "outline" | "ghost" | "destructive";
type Size = "sm" | "md" | "lg";

const pill = cva(
  "group relative inline-flex shrink-0 cursor-pointer select-none items-center justify-center overflow-hidden " +
    "whitespace-nowrap rounded-full border bg-background font-semibold outline-none " +
    "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background " +
    "disabled:pointer-events-none disabled:opacity-50 [&_svg]:shrink-0",
  {
    variants: {
      size: {
        sm: "h-8 text-xs [&_svg:not([class*=size-])]:size-3.5",
        md: "h-9 text-[13px] [&_svg:not([class*=size-])]:size-4",
        lg: "h-10 text-sm [&_svg:not([class*=size-])]:size-4",
      },
      icon: { true: "", false: "" },
      danger: {
        true: "border-destructive/30 text-destructive",
        false: "border-border text-foreground",
      },
    },
    compoundVariants: [
      // Room for the dot on the left, and for the overlay's arrow overall.
      { icon: false, size: "sm", className: "ps-5 pe-4" },
      { icon: false, size: "md", className: "ps-6 pe-4" },
      { icon: false, size: "lg", className: "ps-7 pe-5" },
      { icon: true, size: "sm", className: "size-8" },
      { icon: true, size: "md", className: "size-9" },
      { icon: true, size: "lg", className: "size-10" },
    ],
  },
);

const SLIDE = "transition-all duration-300 motion-reduce:transition-none";

// The fill grows from the dot (or, for an icon, from nothing in the middle).
const DOT_AT = {
  sm: "left-2.5 size-1.5",
  md: "left-3 size-2",
  lg: "left-3.5 size-2",
};

function Button({
  className,
  variant = "primary",
  size = "md",
  mode = "default",
  selected = false,
  type = "button",
  children,
  ...props
}: React.ComponentProps<"button"> & {
  variant?: Variant;
  size?: Size;
  mode?: "default" | "icon";
  // A toggle that is on: shown filled, as on hover.
  selected?: boolean;
}) {
  const icon = mode === "icon";
  const danger = variant === "destructive";
  const fill = danger ? "bg-destructive" : "bg-primary";
  const onFill = danger ? "text-destructive-foreground" : "text-primary-foreground";
  // Spelled out in full: Tailwind finds classes by reading the source.
  const onFillWhenFilled = danger
    ? "group-hover:text-destructive-foreground group-data-[state=open]:text-destructive-foreground"
    : "group-hover:text-primary-foreground group-data-[state=open]:text-primary-foreground";
  return (
    <button
      data-slot="button"
      type={type}
      className={cn(pill({ size, icon, danger }), className)}
      {...(selected && { "data-state": "open" })}
      {...props}
    >
      {icon ? (
        <>
          <span
            aria-hidden
            className={cn(
              "absolute left-1/2 top-1/2 size-0 -translate-x-1/2 -translate-y-1/2 rounded-full",
              fill,
              SLIDE,
              "group-hover:size-full group-hover:scale-[1.8] group-data-[state=open]:size-full group-data-[state=open]:scale-[1.8]",
            )}
          />
          <span
            className={cn(
              "relative z-10 inline-flex items-center justify-center",
              SLIDE,
              onFillWhenFilled,
            )}
          >
            {children}
          </span>
        </>
      ) : (
        <>
          <span
            className={cn(
              "inline-flex translate-x-1 items-center gap-1.5",
              SLIDE,
              "group-hover:translate-x-12 group-hover:opacity-0",
              "group-data-[state=open]:translate-x-12 group-data-[state=open]:opacity-0",
            )}
          >
            {children}
          </span>
          <span
            aria-hidden
            className={cn(
              "absolute inset-0 z-10 flex translate-x-12 items-center justify-center gap-1.5 opacity-0",
              onFill,
              SLIDE,
              "group-hover:translate-x-0 group-hover:opacity-100",
              "group-data-[state=open]:translate-x-0 group-data-[state=open]:opacity-100",
            )}
          >
            {children}
            <ArrowRight />
          </span>
          <span
            aria-hidden
            className={cn(
              "absolute top-1/2 -translate-y-1/2 rounded-lg",
              DOT_AT[size],
              fill,
              SLIDE,
              "group-hover:left-0 group-hover:top-0 group-hover:h-full group-hover:w-full group-hover:translate-y-0 group-hover:scale-[1.8]",
              "group-data-[state=open]:left-0 group-data-[state=open]:top-0 group-data-[state=open]:h-full " +
                "group-data-[state=open]:w-full group-data-[state=open]:translate-y-0 group-data-[state=open]:scale-[1.8]",
            )}
          />
        </>
      )}
    </button>
  );
}

export { Button };
