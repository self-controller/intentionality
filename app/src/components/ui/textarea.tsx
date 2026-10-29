import * as React from "react";
import { cn } from "@/lib/utils";

/** shadcn's Textarea: grows with what is typed where the webview can.
 *  forwardRef: on React 18 a function component is not handed `ref`. */
const Textarea = React.forwardRef<HTMLTextAreaElement, React.ComponentProps<"textarea">>(function Textarea(
  { className, ...props },
  ref,
) {
  return (
    <textarea
      ref={ref}
      data-slot="textarea"
      className={cn(
        "flex field-sizing-content min-h-16 w-full rounded-md border border-input bg-input/30 px-3 py-2 text-sm " +
          "text-foreground shadow-xs outline-none transition-[color,box-shadow] placeholder:text-muted-foreground " +
          "focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 " +
          "disabled:cursor-not-allowed disabled:opacity-50 " +
          "aria-invalid:border-destructive aria-invalid:ring-destructive/40",
        className,
      )}
      {...props}
    />
  );
});

export { Textarea };
