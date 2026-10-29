import * as React from "react";
import { cn } from "@/lib/utils";

/** shadcn's Label, on a plain <label>: Radix's adds only double-click
 *  selection handling, not worth a dependency. */
function Label({ className, ...props }: React.ComponentProps<"label">) {
  return (
    <label
      data-slot="label"
      className={cn(
        "flex select-none items-center gap-2 text-sm font-medium leading-none " +
          "group-data-[disabled=true]:pointer-events-none group-data-[disabled=true]:opacity-50 " +
          "peer-disabled:cursor-not-allowed peer-disabled:opacity-50",
        className,
      )}
      {...props}
    />
  );
}

export { Label };
